//! Alloy-backed `ChainReader` for the Indexer. Reads blocks, decodes pool logs, and reads the
//! oracle and pool state at a given block.

use alloy::eips::BlockId;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use async_trait::async_trait;
use vigil_common::contracts::{ILendingPool, IOracle};
use vigil_common::messages::{BlockMessage, PoolEvent};
use vigil_common::model::{PoolState, Quote, MAX_COLLATERAL};

use crate::engine::{BlockInfo, ChainReader, DecodedLog};
use crate::reorg::BlockRef;

pub struct RpcChain {
    provider: DynProvider,
    pool: Address,
    oracle: Address,
    debt_asset: Address,
    /// Collateral assets by bit index, so the quote is built in a fixed order.
    collateral_by_bit: [Option<Address>; MAX_COLLATERAL],
}

impl RpcChain {
    pub fn new(
        provider: DynProvider,
        pool: Address,
        oracle: Address,
        debt_asset: Address,
        collateral_by_bit: [Option<Address>; MAX_COLLATERAL],
    ) -> Self {
        Self {
            provider,
            pool,
            oracle,
            debt_asset,
            collateral_by_bit,
        }
    }
}

fn decode(topics_and_data: &alloy::primitives::Log) -> Option<PoolEvent> {
    let sig = *topics_and_data.topics().first()?;
    Some(
        if sig == ILendingPool::CollateralDeposited::SIGNATURE_HASH {
            let e = ILendingPool::CollateralDeposited::decode_log(topics_and_data)
                .ok()?
                .data;
            PoolEvent::CollateralDeposited {
                user: e.user,
                asset: e.asset,
                amount: e.amount,
            }
        } else if sig == ILendingPool::CollateralWithdrawn::SIGNATURE_HASH {
            let e = ILendingPool::CollateralWithdrawn::decode_log(topics_and_data)
                .ok()?
                .data;
            PoolEvent::CollateralWithdrawn {
                user: e.user,
                asset: e.asset,
                amount: e.amount,
            }
        } else if sig == ILendingPool::CollateralEnabled::SIGNATURE_HASH {
            let e = ILendingPool::CollateralEnabled::decode_log(topics_and_data)
                .ok()?
                .data;
            PoolEvent::CollateralEnabled {
                user: e.user,
                asset: e.asset,
            }
        } else if sig == ILendingPool::CollateralDisabled::SIGNATURE_HASH {
            let e = ILendingPool::CollateralDisabled::decode_log(topics_and_data)
                .ok()?
                .data;
            PoolEvent::CollateralDisabled {
                user: e.user,
                asset: e.asset,
            }
        } else if sig == ILendingPool::Borrowed::SIGNATURE_HASH {
            let e = ILendingPool::Borrowed::decode_log(topics_and_data)
                .ok()?
                .data;
            PoolEvent::Borrowed {
                user: e.user,
                amount: e.amount,
                scaled: e.scaled,
            }
        } else if sig == ILendingPool::Repaid::SIGNATURE_HASH {
            let e = ILendingPool::Repaid::decode_log(topics_and_data).ok()?.data;
            PoolEvent::Repaid {
                user: e.user,
                amount: e.amount,
                scaled: e.scaled,
            }
        } else if sig == ILendingPool::Liquidated::SIGNATURE_HASH {
            let e = ILendingPool::Liquidated::decode_log(topics_and_data)
                .ok()?
                .data;
            PoolEvent::Liquidated {
                borrower: e.borrower,
                liquidator: e.liquidator,
                asset: e.collateralAsset,
                repaid: e.repaid,
                scaled_repaid: e.scaledRepaid,
                seized: e.seized,
            }
        } else if sig == ILendingPool::BadDebtWrittenOff::SIGNATURE_HASH {
            let e = ILendingPool::BadDebtWrittenOff::decode_log(topics_and_data)
                .ok()?
                .data;
            PoolEvent::BadDebt {
                borrower: e.borrower,
                scaled_debt: e.scaledDebt,
            }
        } else {
            PoolEvent::Other {
                name: format!("{sig:#x}"),
            }
        },
    )
}

#[async_trait]
impl ChainReader for RpcChain {
    async fn head_number(&self) -> anyhow::Result<u64> {
        Ok(self.provider.get_block_number().await?)
    }

    async fn block_by_number(&self, number: u64) -> anyhow::Result<Option<BlockInfo>> {
        let block = self.provider.get_block_by_number(number.into()).await?;
        Ok(block.map(|b| BlockInfo {
            block: BlockRef {
                number,
                hash: b.header.hash,
                parent: b.header.parent_hash,
            },
            timestamp: b.header.timestamp,
        }))
    }

    async fn logs_by_hash(&self, hash: B256) -> anyhow::Result<Vec<DecodedLog>> {
        let filter = Filter::new().address(self.pool).at_block_hash(hash);
        let logs = self.provider.get_logs(&filter).await?;
        let mut out = Vec::with_capacity(logs.len());
        for log in logs {
            let Some(log_index) = log.log_index else {
                anyhow::bail!("log without index in block {hash:#x}")
            };
            let Some(event) = decode(&log.inner) else {
                continue;
            };
            out.push(DecodedLog { log_index, event });
        }
        Ok(out)
    }

    async fn block_message(&self, info: &BlockInfo) -> anyhow::Result<BlockMessage> {
        // Read at the block itself, so the quote matches what that block's state says.
        let at = BlockId::hash(info.block.hash);
        let pool = ILendingPool::new(self.pool, &self.provider);
        let oracle = IOracle::new(self.oracle, &self.provider);

        let totals = pool.totals().block(at).call().await?;
        let stored = pool.storedIndexes().block(at).call().await?;
        let last_accrual = pool.lastAccrual().block(at).call().await?;
        // Projected debt index at this block. The Risk Engine evaluates health factors with it.
        let projected = pool.previewIndexes().block(at).call().await?;

        let frozen = oracle.isFrozen().block(at).call().await?;
        let mut collateral_prices = [U256::ZERO; MAX_COLLATERAL];
        let mut debt_price = U256::ZERO;
        if !frozen {
            debt_price = oracle.getPrice(self.debt_asset).block(at).call().await?;
            for (bit, asset) in self.collateral_by_bit.iter().enumerate() {
                if let Some(asset) = asset {
                    collateral_prices[bit] = oracle.getPrice(*asset).block(at).call().await?;
                }
            }
        }

        Ok(BlockMessage {
            block_number: info.block.number,
            block_hash: info.block.hash,
            timestamp: info.timestamp,
            frozen,
            state: PoolState {
                total_supply_scaled: totals.totalSupplyScaled,
                total_debt_scaled: totals.totalDebtScaled,
                supply_index: stored.supplyIndex,
                debt_index: stored.debtIndex,
                last_accrual,
            },
            quote: Quote {
                debt_index: projected.debtIndex,
                debt_price,
                collateral_prices,
            },
            trace_id: format!("block:{:#x}", info.block.hash),
        })
    }
}
