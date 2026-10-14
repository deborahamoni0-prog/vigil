//! Risk Engine core: applies events to positions exactly once, undoes them on retraction, and
//! emits liquidation signals (RS-05 to RS-09). Pure except for the store, which is a trait so
//! the logic is tested without Postgres.

use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use vigil_common::messages::{
    BlockMessage, EventMessage, LiquidationSignal, MessageKind, PoolEvent,
};
use vigil_common::model::{self, PoolParams, Position, HF_NO_DEBT, MAX_COLLATERAL};

pub type EventKey = (B256, u64);

#[async_trait]
pub trait RiskStore: Send + Sync {
    async fn load_position(&self, borrower: Address) -> anyhow::Result<Position>;
    async fn all_with_debt(&self) -> anyhow::Result<Vec<(Address, Position)>>;
    /// Records the event and the new position in one transaction. Returns false when the key was
    /// already processed, which is the exactly-once check (RS-05).
    async fn commit_applied(
        &self,
        key: EventKey,
        borrower: Address,
        before: Position,
        after: Position,
    ) -> anyhow::Result<bool>;
    /// Restores the position as it was before the event, and forgets the event. Returns false if
    /// the event was never applied.
    async fn undo(&self, key: EventKey) -> anyhow::Result<bool>;
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("event for unknown collateral asset {0}")]
    UnknownAsset(Address),
    #[error("balance underflow: {0}")]
    Underflow(&'static str),
}

/// The borrower an event belongs to, if the event changes a position.
pub fn borrower_of(event: &PoolEvent) -> Option<Address> {
    match event {
        PoolEvent::CollateralDeposited { user, .. }
        | PoolEvent::CollateralWithdrawn { user, .. }
        | PoolEvent::CollateralEnabled { user, .. }
        | PoolEvent::CollateralDisabled { user, .. }
        | PoolEvent::Borrowed { user, .. }
        | PoolEvent::Repaid { user, .. } => Some(*user),
        PoolEvent::Liquidated { borrower, .. } | PoolEvent::BadDebt { borrower, .. } => {
            Some(*borrower)
        }
        PoolEvent::Other { .. } => None,
    }
}

fn bit_of(params: &PoolParams, asset: Address) -> Result<usize, EngineError> {
    params
        .collateral_by_asset(asset)
        .map(|c| c.bit_index as usize)
        .ok_or(EngineError::UnknownAsset(asset))
}

/// Applies one event to a position. Mirrors the state changes in `LendingPool`.
pub fn apply_to_position(
    pos: &mut Position,
    event: &PoolEvent,
    params: &PoolParams,
) -> Result<(), EngineError> {
    match event {
        PoolEvent::CollateralDeposited { asset, amount, .. } => {
            let bit = bit_of(params, *asset)?;
            pos.amounts[bit] += *amount;
            pos.bitmap |= 1 << bit;
        }
        PoolEvent::CollateralWithdrawn { asset, amount, .. } => {
            let bit = bit_of(params, *asset)?;
            pos.amounts[bit] = pos.amounts[bit]
                .checked_sub(*amount)
                .ok_or(EngineError::Underflow("collateral"))?;
            // The contract clears the bit only when the whole balance leaves.
            if pos.amounts[bit].is_zero() {
                pos.bitmap &= !(1 << bit);
            }
        }
        PoolEvent::CollateralEnabled { asset, .. } => {
            pos.bitmap |= 1 << bit_of(params, *asset)?;
        }
        PoolEvent::CollateralDisabled { asset, .. } => {
            pos.bitmap &= !(1 << bit_of(params, *asset)?);
        }
        PoolEvent::Borrowed { scaled, .. } => {
            pos.debt_scaled += *scaled;
        }
        PoolEvent::Repaid { scaled, .. } => {
            pos.debt_scaled = pos
                .debt_scaled
                .checked_sub(*scaled)
                .ok_or(EngineError::Underflow("debt"))?;
        }
        PoolEvent::Liquidated {
            asset,
            seized,
            scaled_repaid,
            ..
        } => {
            pos.debt_scaled = pos
                .debt_scaled
                .checked_sub(*scaled_repaid)
                .ok_or(EngineError::Underflow("debt"))?;
            let bit = bit_of(params, *asset)?;
            pos.amounts[bit] = pos.amounts[bit]
                .checked_sub(*seized)
                .ok_or(EngineError::Underflow("collateral"))?;
            if pos.amounts[bit].is_zero() {
                pos.bitmap &= !(1 << bit);
            }
        }
        PoolEvent::BadDebt { scaled_debt, .. } => {
            pos.debt_scaled = pos
                .debt_scaled
                .checked_sub(*scaled_debt)
                .ok_or(EngineError::Underflow("debt"))?;
        }
        PoolEvent::Other { .. } => {}
    }
    debug_assert!(pos.bitmap < (1 << MAX_COLLATERAL));
    Ok(())
}

pub struct RiskEngine<S> {
    pub store: S,
    pub params: PoolParams,
    last_block: Option<BlockMessage>,
}

impl<S: RiskStore> RiskEngine<S> {
    pub fn new(store: S, params: PoolParams) -> Self {
        Self {
            store,
            params,
            last_block: None,
        }
    }

    pub fn last_block(&self) -> Option<&BlockMessage> {
        self.last_block.as_ref()
    }

    /// Applies a log or a retraction. Duplicates are no-ops (RS-05). Returns true if state changed.
    pub async fn handle_event(&self, msg: &EventMessage) -> anyhow::Result<bool> {
        let key = msg.idempotency_key();
        match msg.kind {
            MessageKind::Retract => self.store.undo(key).await,
            MessageKind::Log => {
                let Some(borrower) = borrower_of(&msg.event) else {
                    return Ok(false);
                };
                let before = self.store.load_position(borrower).await?;
                let mut after = before;
                apply_to_position(&mut after, &msg.event, &self.params)?;
                self.store
                    .commit_applied(key, borrower, before, after)
                    .await
            }
        }
    }

    /// Records the block's quote and returns signals for unhealthy positions (RS-08, RS-09).
    /// While the oracle is Frozen the engine stays silent.
    pub async fn handle_block(
        &mut self,
        block: BlockMessage,
    ) -> anyhow::Result<Vec<LiquidationSignal>> {
        let mut signals = Vec::new();
        if !block.frozen {
            for (borrower, position) in self.store.all_with_debt().await? {
                let hf = model::health_factor(&position, &self.params, &block.quote)
                    .map_err(|e| anyhow::anyhow!("health factor for {borrower}: {e}"))?;
                if hf != HF_NO_DEBT && model::is_liquidatable(hf) {
                    signals.push(LiquidationSignal {
                        borrower,
                        block_number: block.block_number,
                        health_factor: hf,
                        trace_id: block.trace_id.clone(),
                    });
                }
            }
        }
        self.last_block = Some(block);
        Ok(signals)
    }

    /// Health factor now, from the last block the engine saw. `None` before the first block.
    pub async fn health_factor_of(&self, borrower: Address) -> anyhow::Result<Option<(U256, u64)>> {
        let Some(block) = &self.last_block else {
            return Ok(None);
        };
        let position = self.store.load_position(borrower).await?;
        let hf = model::health_factor(&position, &self.params, &block.quote)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Some((hf, block.block_number)))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use alloy::primitives::address;
    use vigil_common::math::RateParams;
    use vigil_common::math::{RAY, WAD};
    use vigil_common::messages::{trace_id_for, BlockMessage};
    use vigil_common::model::{CollateralParams, PoolState, Quote};

    use super::*;

    const FEE: Address = address!("1111111111111111111111111111111111111111");
    const SIX: Address = address!("2222222222222222222222222222222222222222");
    const DEBT: Address = address!("3333333333333333333333333333333333333333");
    const ALICE: Address = address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    const BOB: Address = address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");

    fn params() -> PoolParams {
        PoolParams {
            debt_asset: DEBT,
            debt_decimals: 6,
            reserve_factor: U256::ZERO,
            rates: RateParams {
                base_rate: U256::ZERO,
                slope1: U256::ZERO,
                slope2: U256::ZERO,
                optimal_util: U256::from(1),
            },
            collaterals: vec![
                CollateralParams {
                    asset: FEE,
                    bit_index: 0,
                    decimals: 18,
                    liq_threshold_bps: 8000,
                    liq_bonus_bps: 500,
                },
                CollateralParams {
                    asset: SIX,
                    bit_index: 1,
                    decimals: 6,
                    liq_threshold_bps: 8500,
                    liq_bonus_bps: 500,
                },
            ],
        }
    }

    #[derive(Default)]
    struct MemStore {
        positions: Mutex<HashMap<Address, Position>>,
        applied: Mutex<HashMap<EventKey, (Address, Position)>>,
    }

    #[async_trait]
    impl RiskStore for MemStore {
        async fn load_position(&self, b: Address) -> anyhow::Result<Position> {
            Ok(self
                .positions
                .lock()
                .unwrap()
                .get(&b)
                .copied()
                .unwrap_or_default())
        }
        async fn all_with_debt(&self) -> anyhow::Result<Vec<(Address, Position)>> {
            Ok(self
                .positions
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, p)| !p.debt_scaled.is_zero())
                .map(|(a, p)| (*a, *p))
                .collect())
        }
        async fn commit_applied(
            &self,
            key: EventKey,
            b: Address,
            before: Position,
            after: Position,
        ) -> anyhow::Result<bool> {
            let mut applied = self.applied.lock().unwrap();
            if applied.contains_key(&key) {
                return Ok(false);
            }
            applied.insert(key, (b, before));
            self.positions.lock().unwrap().insert(b, after);
            Ok(true)
        }
        async fn undo(&self, key: EventKey) -> anyhow::Result<bool> {
            let mut applied = self.applied.lock().unwrap();
            let Some((b, before)) = applied.remove(&key) else {
                return Ok(false);
            };
            self.positions.lock().unwrap().insert(b, before);
            Ok(true)
        }
    }

    fn log(n: u64, idx: u64, event: PoolEvent) -> EventMessage {
        let hash = B256::from([n as u8; 32]);
        EventMessage {
            kind: MessageKind::Log,
            block_number: n,
            block_hash: hash,
            log_index: idx,
            trace_id: trace_id_for(hash, idx),
            event,
        }
    }

    fn retract_of(m: &EventMessage) -> EventMessage {
        EventMessage {
            kind: MessageKind::Retract,
            ..m.clone()
        }
    }

    fn block(n: u64, frozen: bool, debt_price: U256, fee_price: U256) -> BlockMessage {
        BlockMessage {
            block_number: n,
            block_hash: B256::from([n as u8; 32]),
            timestamp: 1_000 + n,
            frozen,
            state: PoolState {
                total_supply_scaled: U256::ZERO,
                total_debt_scaled: U256::ZERO,
                supply_index: RAY,
                debt_index: RAY,
                last_accrual: 1_000 + n,
            },
            quote: Quote {
                debt_index: RAY,
                debt_price,
                collateral_prices: [fee_price, U256::ZERO, U256::ZERO],
            },
            trace_id: String::new(),
        }
    }

    fn wad(x: u64) -> U256 {
        U256::from(x) * U256::from(10).pow(U256::from(18))
    }

    #[tokio::test]
    async fn duplicate_log_is_applied_once() {
        let engine = RiskEngine::new(MemStore::default(), params());
        let dep = log(
            1,
            0,
            PoolEvent::CollateralDeposited {
                user: ALICE,
                asset: FEE,
                amount: wad(10),
            },
        );
        assert!(engine.handle_event(&dep).await.unwrap());
        assert!(
            !engine.handle_event(&dep).await.unwrap(),
            "redelivery must not double-count"
        );
        assert_eq!(
            engine.store.load_position(ALICE).await.unwrap().amounts[0],
            wad(10)
        );
    }

    #[tokio::test]
    async fn retraction_restores_the_pre_orphan_state() {
        let engine = RiskEngine::new(MemStore::default(), params());
        let dep = log(
            1,
            0,
            PoolEvent::CollateralDeposited {
                user: ALICE,
                asset: FEE,
                amount: wad(10),
            },
        );
        let borrow = log(
            2,
            0,
            PoolEvent::Borrowed {
                user: ALICE,
                amount: U256::from(5),
                scaled: U256::from(5),
            },
        );
        engine.handle_event(&dep).await.unwrap();
        engine.handle_event(&borrow).await.unwrap();

        // Block 2 is orphaned. Retract newest first, as the Indexer does.
        engine.handle_event(&retract_of(&borrow)).await.unwrap();
        let p = engine.store.load_position(ALICE).await.unwrap();
        assert!(p.debt_scaled.is_zero());
        assert_eq!(p.amounts[0], wad(10), "the earlier deposit survives");
    }

    #[tokio::test]
    async fn retraction_then_replay_on_new_fork_applies_again() {
        let engine = RiskEngine::new(MemStore::default(), params());
        let borrow = log(
            2,
            0,
            PoolEvent::Borrowed {
                user: ALICE,
                amount: U256::from(7),
                scaled: U256::from(7),
            },
        );
        engine.handle_event(&borrow).await.unwrap();
        engine.handle_event(&retract_of(&borrow)).await.unwrap();
        // The same (block_hash, log_index) reappears on the canonical chain.
        assert!(engine.handle_event(&borrow).await.unwrap());
        assert_eq!(
            engine.store.load_position(ALICE).await.unwrap().debt_scaled,
            U256::from(7)
        );
    }

    #[tokio::test]
    async fn unhealthy_position_produces_a_signal_and_frozen_suppresses_it() {
        let mut engine = RiskEngine::new(MemStore::default(), params());
        engine
            .handle_event(&log(
                1,
                0,
                PoolEvent::CollateralDeposited {
                    user: BOB,
                    asset: FEE,
                    amount: wad(10),
                },
            ))
            .await
            .unwrap();
        engine
            .handle_event(&log(
                1,
                1,
                PoolEvent::Borrowed {
                    user: BOB,
                    amount: wad(10_000),
                    scaled: wad(10_000),
                },
            ))
            .await
            .unwrap();

        // Fee token at $1200: 10 * 1200 * 80% = 9600 < 10000, so unhealthy.
        let signals = engine
            .handle_block(block(3, false, WAD, wad(1200)))
            .await
            .unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].borrower, BOB);
        assert_eq!(signals[0].block_number, 3);

        let signals = engine
            .handle_block(block(4, true, WAD, wad(1200)))
            .await
            .unwrap();
        assert!(signals.is_empty(), "no signals while Frozen (RS-08)");
    }

    #[tokio::test]
    async fn liquidation_reduces_debt_and_collateral_by_the_logged_amounts() {
        let engine = RiskEngine::new(MemStore::default(), params());
        engine
            .handle_event(&log(
                1,
                0,
                PoolEvent::CollateralDeposited {
                    user: BOB,
                    asset: FEE,
                    amount: wad(10),
                },
            ))
            .await
            .unwrap();
        engine
            .handle_event(&log(
                1,
                1,
                PoolEvent::Borrowed {
                    user: BOB,
                    amount: wad(100),
                    scaled: wad(100),
                },
            ))
            .await
            .unwrap();
        engine
            .handle_event(&log(
                2,
                0,
                PoolEvent::Liquidated {
                    borrower: BOB,
                    liquidator: ALICE,
                    asset: FEE,
                    repaid: wad(50),
                    scaled_repaid: wad(50),
                    seized: wad(4),
                },
            ))
            .await
            .unwrap();
        let p = engine.store.load_position(BOB).await.unwrap();
        assert_eq!(p.debt_scaled, wad(50));
        assert_eq!(p.amounts[0], wad(6));
        assert_eq!(p.bitmap & 1, 1);
    }

    #[tokio::test]
    async fn unknown_collateral_asset_is_rejected() {
        let engine = RiskEngine::new(MemStore::default(), params());
        let stray = Address::repeat_byte(9);
        let err = engine
            .handle_event(&log(
                1,
                0,
                PoolEvent::CollateralDeposited {
                    user: ALICE,
                    asset: stray,
                    amount: U256::from(1),
                },
            ))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown collateral"));
    }
}
