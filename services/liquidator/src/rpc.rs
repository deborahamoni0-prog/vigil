//! Alloy-backed `TxChain`. Reads come from the node. Sends go through the signing wallet with the
//! nonce set explicitly, so the nonce book stays the only source of nonces.

use alloy::eips::BlockId;
use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use async_trait::async_trait;
use vigil_common::contracts::ILendingPool;

use crate::chain::{TxChain, TxRequest, TxStatus};
use crate::signer::KeystoreSigner;

pub struct RpcTxChain {
    provider: DynProvider,
    address: Address,
    pool: Address,
    chain_id: u64,
}

impl RpcTxChain {
    pub async fn connect(
        rpc_url: &str,
        signer: &KeystoreSigner,
        pool: Address,
    ) -> anyhow::Result<Self> {
        let provider = ProviderBuilder::new()
            .wallet(signer.wallet())
            .connect_http(rpc_url.parse()?)
            .erased();
        let chain_id = provider.get_chain_id().await?;
        Ok(Self {
            provider,
            address: signer.address(),
            pool,
            chain_id,
        })
    }
}

#[async_trait]
impl TxChain for RpcTxChain {
    fn address(&self) -> Address {
        self.address
    }

    async fn block_number(&self) -> anyhow::Result<u64> {
        Ok(self.provider.get_block_number().await?)
    }

    async fn block_timestamp(&self) -> anyhow::Result<u64> {
        let b = self
            .provider
            .get_block_by_number(alloy::eips::BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no latest block"))?;
        Ok(b.header.timestamp)
    }

    async fn pending_nonce(&self) -> anyhow::Result<u64> {
        Ok(self
            .provider
            .get_transaction_count(self.address)
            .block_id(BlockId::pending())
            .await?)
    }

    async fn latest_nonce(&self) -> anyhow::Result<u64> {
        Ok(self
            .provider
            .get_transaction_count(self.address)
            .block_id(BlockId::latest())
            .await?)
    }

    async fn send(&self, req: &TxRequest) -> anyhow::Result<B256> {
        let tx = TransactionRequest::default()
            .with_to(req.to)
            .with_input(req.data.clone())
            .with_value(req.value)
            .with_nonce(req.nonce)
            .with_chain_id(self.chain_id)
            .with_gas_limit(req.gas_limit)
            .with_max_fee_per_gas(req.max_fee_per_gas)
            .with_max_priority_fee_per_gas(req.max_priority_fee_per_gas);
        let pending = self.provider.send_transaction(tx).await?;
        Ok(*pending.tx_hash())
    }

    async fn status(&self, hash: B256) -> anyhow::Result<TxStatus> {
        if let Some(receipt) = self.provider.get_transaction_receipt(hash).await? {
            return Ok(match receipt.block_number {
                Some(block) if receipt.status() => TxStatus::Confirmed { block },
                Some(block) => TxStatus::Reverted { block },
                None => TxStatus::Pending,
            });
        }
        if self.provider.get_transaction_by_hash(hash).await?.is_some() {
            return Ok(TxStatus::Pending);
        }
        Ok(TxStatus::Unknown)
    }

    async fn health_factor(&self, borrower: Address) -> anyhow::Result<U256> {
        let pool = ILendingPool::new(self.pool, &self.provider);
        Ok(pool.getHealthFactor(borrower).call().await?)
    }

    async fn bonus_bps(&self, borrower: Address, collateral: Address) -> anyhow::Result<u64> {
        let pool = ILendingPool::new(self.pool, &self.provider);
        let bps = pool.bonusBpsAt(borrower, collateral).call().await?;
        Ok(bps.try_into().unwrap_or(0))
    }

    async fn auction_start(&self, borrower: Address) -> anyhow::Result<u64> {
        let pool = ILendingPool::new(self.pool, &self.provider);
        Ok(pool
            .auctionStartOf(borrower)
            .call()
            .await?
            .try_into()
            .unwrap_or(0))
    }

    async fn gas_price(&self) -> anyhow::Result<u128> {
        Ok(self.provider.get_gas_price().await?)
    }

    async fn nonce_used(&self, borrower: Address, nonce: U256) -> anyhow::Result<bool> {
        let pool = ILendingPool::new(self.pool, &self.provider);
        Ok(pool.nonceUsed(borrower, nonce).call().await?)
    }
}
