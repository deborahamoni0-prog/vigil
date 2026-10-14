//! Liquidator executor: turns signals into transactions, keeps the nonce book honest, and handles
//! replacement, gap-filling and reorg recovery (RS-14 to RS-16, section 6.4).

use std::collections::HashMap;

use alloy::primitives::{Address, B256, U256};
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use vigil_common::contracts::{ILendingPool, LiquidationIntent};
use vigil_common::messages::{LiquidationSignal, MAX_SIGNAL_AGE_BLOCKS};

use crate::chain::{TxChain, TxRequest, TxStatus};
use crate::nonce::{Action, InFlight, NonceBook, TxPurpose};
use crate::strategy::{Decision, MarketView, Strategy};

/// How many blocks a confirmed transaction is watched for a reorg (RS-15).
pub const REORG_WATCH_BLOCKS: u64 = 10;
/// After a liquidation reverts, the borrower is not struck again for this many blocks. Health
/// moves every block (interest accrues), so an exact health-factor match would never hold.
pub const REVERT_COOLDOWN_BLOCKS: u64 = 10;
const WAD: u128 = 1_000_000_000_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIntent {
    pub intent: LiquidationIntent,
    pub signature: Vec<u8>,
}

#[async_trait]
pub trait IntentStore: Send + Sync {
    /// Every signed intent for the borrower, oldest nonce first.
    async fn intents_for(&self, borrower: Address) -> anyhow::Result<Vec<StoredIntent>>;
}

/// Durable record of what the Liquidator sent, for the gateway and for audits.
#[async_trait]
pub trait TxLog: Send + Sync {
    async fn sent(&self, tx: &InFlight, kind: &str) -> anyhow::Result<()>;
    async fn set_status(&self, nonce: u64, status: &str) -> anyhow::Result<()>;
    /// Rows still marked pending, with their transaction hashes. Left over from a previous run.
    async fn pending(&self) -> anyhow::Result<Vec<(u64, B256)>> {
        Ok(Vec::new())
    }
}

/// Discards records. Used by the unit tests.
pub struct NoopLog;

#[async_trait]
impl TxLog for NoopLog {
    async fn sent(&self, _tx: &InFlight, _kind: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn set_status(&self, _nonce: u64, _status: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutorConfig {
    pub pool: Address,
    pub replace_after_blocks: u64,
    pub gas_limit: u64,
    pub gas_cost_debt: U256,
}

/// Outcome of handling one signal, for logs and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Skipped(&'static str),
    StartedAuction { nonce: u64, hash: B256 },
    Struck { nonce: u64, hash: B256, repay: U256 },
    Waiting,
}

pub struct Executor<C, I> {
    pub chain: C,
    pub intents: I,
    pub strategy: Box<dyn Strategy>,
    pub cfg: ExecutorConfig,
    pub book: NonceBook,
    pub log: Box<dyn TxLog>,
    /// Confirmed transactions still inside the reorg-watch window, keyed by nonce.
    watching: HashMap<u64, (InFlight, u64)>,
    /// Borrowers whose liquidation reverted, with the block of the revert. A reverted
    /// liquidation is not retried until REVERT_COOLDOWN_BLOCKS have passed.
    reverted_at: HashMap<Address, u64>,
}

impl<C: TxChain, I: IntentStore> Executor<C, I> {
    pub fn new(chain: C, intents: I, strategy: Box<dyn Strategy>, cfg: ExecutorConfig) -> Self {
        Self {
            chain,
            intents,
            strategy,
            cfg,
            book: NonceBook::default(),
            log: Box::new(NoopLog),
            watching: HashMap::new(),
            reverted_at: HashMap::new(),
        }
    }

    /// Records a sent transaction in the nonce book and in the durable log.
    async fn track(&mut self, tx: InFlight) -> anyhow::Result<()> {
        let kind = match &tx.purpose {
            TxPurpose::StartAuction { .. } => "start_auction",
            TxPurpose::Liquidate { .. } => "liquidate",
            TxPurpose::GapFill => "gap_fill",
        };
        self.log.sent(&tx, kind).await?;
        self.book.record_sent(tx);
        Ok(())
    }

    /// Current block number, for the main loop's block watcher.
    pub async fn chain_block(&self) -> anyhow::Result<u64> {
        self.chain.block_number().await
    }

    /// Startup: sync the nonce book with the chain (RS-13), then settle database rows that were
    /// left pending by an earlier process.
    pub async fn reconcile(&mut self) -> anyhow::Result<()> {
        let latest = self.chain.latest_nonce().await?;
        let pending = self.chain.pending_nonce().await?;
        self.book.reconcile(latest, pending);
        tracing::info!(
            latest,
            pending,
            next = self.book.next_nonce(),
            "nonce book reconciled"
        );
        self.settle_pending().await
    }

    /// Records the real outcome of rows that a previous process left as pending. A row whose
    /// transaction is still in the mempool stays pending. A row whose transaction is unknown to
    /// the node is marked reorged, since it did not stay on chain.
    async fn settle_pending(&mut self) -> anyhow::Result<()> {
        for (nonce, hash) in self.log.pending().await? {
            let status = match self.chain.status(hash).await? {
                TxStatus::Confirmed { .. } => "confirmed",
                TxStatus::Reverted { .. } => "failed",
                TxStatus::Unknown => "reorged",
                TxStatus::Pending => continue,
            };
            tracing::info!(nonce, %hash, status, "settled leftover transaction record");
            self.log.set_status(nonce, status).await?;
        }
        Ok(())
    }

    /// Handles one liquidation signal, re-checking freshness and health first (RS-09, RS-16).
    pub async fn handle_signal(&mut self, signal: &LiquidationSignal) -> anyhow::Result<Outcome> {
        let current = self.chain.block_number().await?;
        if current > signal.block_number + MAX_SIGNAL_AGE_BLOCKS {
            return Ok(Outcome::Skipped("stale signal"));
        }
        // A redelivered signal must not produce a second transaction for the same borrower.
        if self
            .book
            .in_flight()
            .any(|t| purpose_borrower(&t.purpose) == Some(signal.borrower))
        {
            return Ok(Outcome::Skipped("already in flight"));
        }
        // Fresh health factor from the chain, not from the signal.
        // A frozen oracle makes the read revert. That is a skip, not a retry: redelivering the
        // same signal would just fail the same way until the oracle recovers.
        let hf = match self.chain.health_factor(signal.borrower).await {
            Ok(hf) => hf,
            Err(e) => {
                tracing::info!(borrower = %signal.borrower, error = %e, "price unavailable, skipping");
                return Ok(Outcome::Skipped("price unavailable"));
            }
        };
        if hf >= U256::from(WAD) {
            return Ok(Outcome::Skipped("healthy on chain"));
        }
        if let Some(&at) = self.reverted_at.get(&signal.borrower) {
            if current < at + REVERT_COOLDOWN_BLOCKS {
                return Ok(Outcome::Skipped("liquidation reverted recently"));
            }
            self.reverted_at.remove(&signal.borrower);
        }
        let candidates = self.intents.intents_for(signal.borrower).await?;
        if candidates.is_empty() {
            return Ok(Outcome::Skipped("no signed intent"));
        }
        // The pool consumes an intent's nonce when it is used, so a spent intent reverts with
        // NonceUsed. Use the oldest intent that is still live and unspent, not just the oldest.
        let now = U256::from(self.chain.block_timestamp().await?);
        let mut usable = None;
        let mut reason = "no usable signed intent";
        for candidate in candidates {
            if candidate.intent.deadline < now {
                reason = "intent expired";
            } else if self
                .chain
                .nonce_used(signal.borrower, candidate.intent.nonce)
                .await?
            {
                reason = "intent already used";
            } else {
                usable = Some(candidate);
                break;
            }
        }
        let Some(stored) = usable else {
            return Ok(Outcome::Skipped(reason));
        };

        if self.chain.auction_start(signal.borrower).await? == 0 {
            return match self
                .send_start_auction(signal.borrower, &signal.trace_id)
                .await
            {
                Ok((nonce, hash)) => Ok(Outcome::StartedAuction { nonce, hash }),
                Err(e) => self.skip_after_failed_send(e).await,
            };
        }

        let bonus = self
            .chain
            .bonus_bps(signal.borrower, stored.intent.collateralAsset)
            .await?;
        let market = MarketView {
            current_block: current,
            bonus_bps: bonus,
            repay: stored.intent.maxRepay,
            gas_cost_debt: self.cfg.gas_cost_debt,
        };
        match self.strategy.decide(signal, &market) {
            Decision::Strike { repay } => match self.send_liquidate(signal, &stored, repay).await {
                Ok((nonce, hash)) => Ok(Outcome::Struck { nonce, hash, repay }),
                Err(e) => self.skip_after_failed_send(e).await,
            },
            Decision::Wait { .. } => Ok(Outcome::Waiting),
            Decision::Skip { reason } => Ok(Outcome::Skipped(reason)),
        }
    }

    /// A rejected send (for example "replacement transaction underpriced") means the nonce book
    /// no longer matches the node. Re-sync it from the chain and skip the signal. Redelivering
    /// it would reject again with the same stale nonce.
    async fn skip_after_failed_send(&mut self, err: anyhow::Error) -> anyhow::Result<Outcome> {
        tracing::warn!(error = %err, "send rejected, resyncing nonce book");
        self.reconcile().await?;
        Ok(Outcome::Skipped("send rejected, nonce resynced"))
    }

    /// Advances every in-flight transaction by one block: confirm, replace, fill, or recover.
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        let current = self.chain.block_number().await?;

        // 1. Classify what is in flight.
        let mut dropped = Vec::new();
        let tracked: Vec<InFlight> = self.book.in_flight().cloned().collect();
        for t in &tracked {
            let Some(hash) = t.hash else { continue };
            match self.chain.status(hash).await? {
                TxStatus::Confirmed { block } => {
                    if let Some(done) = self.book.confirmed(t.nonce) {
                        tracing::info!(trace_id = %done.trace_id, nonce = t.nonce, block, "transaction confirmed");
                        self.log.set_status(t.nonce, "confirmed").await?;
                        self.watching.insert(t.nonce, (done, block));
                    }
                }
                TxStatus::Reverted { block } => {
                    // The nonce is used, but the call did not happen. Free the slot so a later signal can retry.
                    if let Some(done) = self.book.confirmed(t.nonce) {
                        tracing::warn!(trace_id = %done.trace_id, nonce = t.nonce, block, "transaction reverted");
                        self.log.set_status(t.nonce, "failed").await?;
                        if let Some(borrower) = purpose_borrower(&done.purpose) {
                            self.reverted_at.insert(borrower, current);
                        }
                    }
                }
                TxStatus::Pending => {}
                TxStatus::Unknown => dropped.push(t.nonce),
            }
        }

        // 2. Reorg watch: a confirmed transaction that vanished must be sent again (RS-15).
        let watched: Vec<(u64, InFlight, u64)> = self
            .watching
            .iter()
            .map(|(n, (t, b))| (*n, t.clone(), *b))
            .collect();
        for (nonce, tx, block) in watched {
            if current > block + REORG_WATCH_BLOCKS {
                self.watching.remove(&nonce);
                continue;
            }
            if let Some(hash) = tx.hash {
                if self.chain.status(hash).await? == TxStatus::Unknown {
                    tracing::warn!(trace_id = %tx.trace_id, nonce, "confirmed transaction reorged out, resubmitting");
                    self.watching.remove(&nonce);
                    self.book.reorged_out(InFlight { hash: None, ..tx });
                    dropped.push(nonce);
                }
            }
        }

        // 3. Act on the plan.
        for action in self
            .book
            .plan(current, self.cfg.replace_after_blocks, &dropped)
        {
            match action {
                Action::Wait => {}
                Action::Replace {
                    nonce,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                } => {
                    self.resend(nonce, max_fee_per_gas, max_priority_fee_per_gas, current)
                        .await?;
                }
                Action::GapFill { nonce } => {
                    self.recover_dropped(nonce, current).await?;
                }
            }
        }
        Ok(())
    }

    /// A dropped transaction is resent when it is still needed, and filled with a self-transfer
    /// when it is not. Either way, no nonce is left as a hole (RS-15).
    async fn recover_dropped(&mut self, nonce: u64, current: u64) -> anyhow::Result<()> {
        let Some(tx) = self.book.get(nonce).cloned() else {
            return Ok(());
        };
        let still_needed = match &tx.purpose {
            TxPurpose::Liquidate { borrower, .. } => {
                let hf = self.chain.health_factor(*borrower).await?;
                hf < U256::from(WAD)
            }
            TxPurpose::StartAuction { borrower } => {
                let hf = self.chain.health_factor(*borrower).await?;
                hf < U256::from(WAD) && self.chain.auction_start(*borrower).await? == 0
            }
            TxPurpose::GapFill => false,
        };
        if still_needed && !matches!(tx.purpose, TxPurpose::GapFill) {
            tracing::warn!(
                nonce,
                "dropped transaction still needed, resending with the same nonce"
            );
            let fee = tx.max_fee_per_gas;
            let prio = tx.max_priority_fee_per_gas;
            let req = self.build_request(nonce, &tx.purpose, fee, prio)?;
            let hash = self.chain.send(&req).await?;
            self.track(InFlight {
                hash: Some(hash),
                sent_block: current,
                ..tx
            })
            .await?;
        } else {
            tracing::warn!(
                nonce,
                "dropped transaction no longer needed, filling the nonce"
            );
            let fee = tx.max_fee_per_gas;
            let prio = tx.max_priority_fee_per_gas;
            let req = TxRequest {
                nonce,
                to: self.chain.address(),
                data: Vec::new(),
                value: U256::ZERO,
                max_fee_per_gas: fee,
                max_priority_fee_per_gas: prio,
                gas_limit: 21_000,
            };
            let hash = self.chain.send(&req).await?;
            self.track(InFlight {
                nonce,
                hash: Some(hash),
                max_fee_per_gas: fee,
                max_priority_fee_per_gas: prio,
                sent_block: current,
                purpose: TxPurpose::GapFill,
                trace_id: tx.trace_id,
                replacements: 0,
            })
            .await?;
        }
        Ok(())
    }

    async fn resend(
        &mut self,
        nonce: u64,
        max_fee: u128,
        prio: u128,
        current: u64,
    ) -> anyhow::Result<()> {
        let Some(tx) = self.book.get(nonce).cloned() else {
            return Ok(());
        };
        tracing::info!(trace_id = %tx.trace_id, nonce, max_fee, "replacing unconfirmed transaction with a fee bump");
        let req = match &tx.purpose {
            TxPurpose::GapFill => TxRequest {
                nonce,
                to: self.chain.address(),
                data: Vec::new(),
                value: U256::ZERO,
                max_fee_per_gas: max_fee,
                max_priority_fee_per_gas: prio,
                gas_limit: 21_000,
            },
            purpose => self.build_request(nonce, purpose, max_fee, prio)?,
        };
        let hash = self.chain.send(&req).await?;
        self.book.replaced(nonce, max_fee, prio, current);
        self.book.set_hash(nonce, hash);
        Ok(())
    }

    async fn send_start_auction(
        &mut self,
        borrower: Address,
        trace_id: &str,
    ) -> anyhow::Result<(u64, B256)> {
        let gas = self.chain.gas_price().await?;
        let (fee, prio) = (gas * 2, gas / 10 + 1);
        let nonce = self.book.reserve();
        let purpose = TxPurpose::StartAuction { borrower };
        let req = self.build_request(nonce, &purpose, fee, prio)?;
        let hash = self.chain.send(&req).await?;
        let current = self.chain.block_number().await?;
        self.track(InFlight {
            nonce,
            hash: Some(hash),
            max_fee_per_gas: fee,
            max_priority_fee_per_gas: prio,
            sent_block: current,
            purpose,
            trace_id: trace_id.to_string(),
            replacements: 0,
        })
        .await?;
        Ok((nonce, hash))
    }

    async fn send_liquidate(
        &mut self,
        signal: &LiquidationSignal,
        stored: &StoredIntent,
        repay: U256,
    ) -> anyhow::Result<(u64, B256)> {
        let gas = self.chain.gas_price().await?;
        let (fee, prio) = (gas * 2, gas / 10 + 1);
        let nonce = self.book.reserve();
        let purpose = TxPurpose::Liquidate {
            borrower: signal.borrower,
            intent: Box::new(stored.intent.clone()),
            signature: stored.signature.clone(),
            repay,
        };
        let req = self.build_request(nonce, &purpose, fee, prio)?;
        let hash = self.chain.send(&req).await?;
        let current = self.chain.block_number().await?;
        self.track(InFlight {
            nonce,
            hash: Some(hash),
            max_fee_per_gas: fee,
            max_priority_fee_per_gas: prio,
            sent_block: current,
            purpose,
            trace_id: signal.trace_id.clone(),
            replacements: 0,
        })
        .await?;
        Ok((nonce, hash))
    }

    fn build_request(
        &self,
        nonce: u64,
        purpose: &TxPurpose,
        fee: u128,
        prio: u128,
    ) -> anyhow::Result<TxRequest> {
        let data = match purpose {
            TxPurpose::StartAuction { borrower } => ILendingPool::startAuctionCall {
                borrower: *borrower,
            }
            .abi_encode(),
            TxPurpose::Liquidate {
                intent,
                signature,
                repay,
                ..
            } => ILendingPool::liquidateCall {
                intent: (**intent).clone(),
                signature: signature.clone().into(),
                repayAmount: *repay,
            }
            .abi_encode(),
            TxPurpose::GapFill => Vec::new(),
        };
        Ok(TxRequest {
            nonce,
            to: self.cfg.pool,
            data,
            value: U256::ZERO,
            max_fee_per_gas: fee,
            max_priority_fee_per_gas: prio,
            gas_limit: self.cfg.gas_limit,
        })
    }
}

fn purpose_borrower(p: &TxPurpose) -> Option<Address> {
    match p {
        TxPurpose::StartAuction { borrower } | TxPurpose::Liquidate { borrower, .. } => {
            Some(*borrower)
        }
        TxPurpose::GapFill => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use alloy::primitives::address;
    use vigil_common::messages::LiquidationSignal;

    use super::*;
    use crate::strategy::{ImmediateStrike, WaitForMargin};

    const POOL: Address = address!("5555555555555555555555555555555555555555");
    const FEE: Address = address!("1111111111111111111111111111111111111111");
    const BORROWER: Address = address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    const LIQUIDATOR: Address = address!("cccccccccccccccccccccccccccccccccccccccc");

    #[derive(Default)]
    struct FakeChain {
        block: Mutex<u64>,
        timestamp: Mutex<u64>,
        latest: Mutex<u64>,
        pending: Mutex<u64>,
        hf: Mutex<U256>,
        auction: Mutex<u64>,
        bonus: Mutex<u64>,
        statuses: Mutex<HashMap<B256, TxStatus>>,
        sent: Mutex<Vec<TxRequest>>,
        /// Intent nonces the fake pool reports as already used.
        used: Mutex<Vec<U256>>,
    }

    fn hash_of(n: usize) -> B256 {
        B256::from([n as u8 + 1; 32])
    }

    #[async_trait]
    impl TxChain for FakeChain {
        fn address(&self) -> Address {
            LIQUIDATOR
        }
        async fn block_number(&self) -> anyhow::Result<u64> {
            Ok(*self.block.lock().unwrap())
        }
        async fn block_timestamp(&self) -> anyhow::Result<u64> {
            Ok(*self.timestamp.lock().unwrap())
        }
        async fn pending_nonce(&self) -> anyhow::Result<u64> {
            Ok(*self.pending.lock().unwrap())
        }
        async fn latest_nonce(&self) -> anyhow::Result<u64> {
            Ok(*self.latest.lock().unwrap())
        }
        async fn send(&self, req: &TxRequest) -> anyhow::Result<B256> {
            let mut sent = self.sent.lock().unwrap();
            sent.push(req.clone());
            let h = hash_of(sent.len());
            self.statuses.lock().unwrap().insert(h, TxStatus::Pending);
            Ok(h)
        }
        async fn status(&self, hash: B256) -> anyhow::Result<TxStatus> {
            Ok(self
                .statuses
                .lock()
                .unwrap()
                .get(&hash)
                .copied()
                .unwrap_or(TxStatus::Unknown))
        }
        async fn health_factor(&self, _b: Address) -> anyhow::Result<U256> {
            Ok(*self.hf.lock().unwrap())
        }
        async fn bonus_bps(&self, _b: Address, _c: Address) -> anyhow::Result<u64> {
            Ok(*self.bonus.lock().unwrap())
        }
        async fn auction_start(&self, _b: Address) -> anyhow::Result<u64> {
            Ok(*self.auction.lock().unwrap())
        }
        async fn gas_price(&self) -> anyhow::Result<u128> {
            Ok(1_000_000_000)
        }
        async fn nonce_used(&self, _b: Address, nonce: U256) -> anyhow::Result<bool> {
            Ok(self.used.lock().unwrap().contains(&nonce))
        }
    }

    struct FixedIntents(Vec<StoredIntent>);

    #[async_trait]
    impl IntentStore for FixedIntents {
        async fn intents_for(&self, _b: Address) -> anyhow::Result<Vec<StoredIntent>> {
            Ok(self.0.clone())
        }
    }

    fn stored() -> StoredIntent {
        StoredIntent {
            intent: LiquidationIntent {
                borrower: BORROWER,
                collateralAsset: FEE,
                maxRepay: U256::from(5_000u64),
                minCollateralOut: U256::ZERO,
                nonce: U256::from(1u64),
                deadline: U256::from(u64::MAX),
            },
            signature: vec![7u8; 65],
        }
    }

    fn signal(block: u64) -> LiquidationSignal {
        LiquidationSignal {
            borrower: BORROWER,
            block_number: block,
            health_factor: U256::from(9u64),
            trace_id: "t".to_string(),
        }
    }

    fn cfg() -> ExecutorConfig {
        ExecutorConfig {
            pool: POOL,
            replace_after_blocks: 3,
            gas_limit: 400_000,
            gas_cost_debt: U256::from(1u64),
        }
    }

    fn exec(chain: FakeChain, intent: Option<StoredIntent>) -> Executor<FakeChain, FixedIntents> {
        Executor::new(
            chain,
            FixedIntents(intent.into_iter().collect()),
            Box::new(ImmediateStrike),
            cfg(),
        )
    }

    fn unhealthy(chain: &FakeChain, block: u64) {
        *chain.block.lock().unwrap() = block;
        *chain.timestamp.lock().unwrap() = 1_000;
        *chain.hf.lock().unwrap() = U256::from(9u64) * U256::from(10u64).pow(U256::from(17u64));
    }

    #[tokio::test]
    async fn stale_signal_is_ignored() {
        let chain = FakeChain::default();
        unhealthy(&chain, 20);
        let mut e = exec(chain, Some(stored()));
        assert_eq!(
            e.handle_signal(&signal(10)).await.unwrap(),
            Outcome::Skipped("stale signal")
        );
        assert!(e.chain.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn healthy_position_is_rechecked_and_skipped() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        *chain.hf.lock().unwrap() = U256::from(2u64) * U256::from(10u64).pow(U256::from(18u64));
        let mut e = exec(chain, Some(stored()));
        assert_eq!(
            e.handle_signal(&signal(10)).await.unwrap(),
            Outcome::Skipped("healthy on chain")
        );
    }

    #[tokio::test]
    async fn missing_intent_means_no_transaction() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        let mut e = exec(chain, None);
        assert_eq!(
            e.handle_signal(&signal(10)).await.unwrap(),
            Outcome::Skipped("no signed intent")
        );
    }

    #[tokio::test]
    async fn expired_intent_is_skipped() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        let mut s = stored();
        s.intent.deadline = U256::from(999u64);
        let mut e = exec(chain, Some(s));
        assert_eq!(
            e.handle_signal(&signal(10)).await.unwrap(),
            Outcome::Skipped("intent expired")
        );
    }

    #[tokio::test]
    async fn spent_intent_is_skipped_without_sending() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        *chain.used.lock().unwrap() = vec![U256::from(1u64)];
        let mut e = exec(chain, Some(stored()));
        assert_eq!(
            e.handle_signal(&signal(10)).await.unwrap(),
            Outcome::Skipped("intent already used")
        );
        assert!(e.chain.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn spent_intent_falls_through_to_the_next_unused_one() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        *chain.used.lock().unwrap() = vec![U256::from(1u64)];
        *chain.auction.lock().unwrap() = 10;
        *chain.bonus.lock().unwrap() = 500;
        let mut second = stored();
        second.intent.nonce = U256::from(2u64);
        let mut e = Executor::new(
            chain,
            FixedIntents(vec![stored(), second]),
            Box::new(ImmediateStrike),
            cfg(),
        );
        let outcome = e.handle_signal(&signal(10)).await.unwrap();
        assert!(matches!(outcome, Outcome::Struck { .. }), "got {outcome:?}");
        assert_eq!(e.chain.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn starts_auction_then_strikes_with_the_next_nonce() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        let mut e = exec(chain, Some(stored()));

        let first = e.handle_signal(&signal(10)).await.unwrap();
        let Outcome::StartedAuction { nonce: 0, hash } = first else {
            panic!("got {first:?}")
        };

        // The auction transaction confirms before the next signal is handled.
        e.chain
            .statuses
            .lock()
            .unwrap()
            .insert(hash, TxStatus::Confirmed { block: 10 });
        e.tick().await.unwrap();

        *e.chain.auction.lock().unwrap() = 10;
        *e.chain.bonus.lock().unwrap() = 500;
        let second = e.handle_signal(&signal(10)).await.unwrap();
        assert!(
            matches!(second, Outcome::Struck { nonce: 1, .. }),
            "got {second:?}"
        );
    }

    #[tokio::test]
    async fn reverted_liquidation_is_not_retried_during_cooldown() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        *chain.auction.lock().unwrap() = 10;
        *chain.bonus.lock().unwrap() = 500;
        let mut e = exec(chain, Some(stored()));

        let first = e.handle_signal(&signal(10)).await.unwrap();
        let Outcome::Struck { nonce: 0, hash, .. } = first else {
            panic!("got {first:?}")
        };
        e.chain
            .statuses
            .lock()
            .unwrap()
            .insert(hash, TxStatus::Reverted { block: 10 });
        e.tick().await.unwrap();

        // Inside the cooldown: nothing is sent, even as health keeps moving.
        let during = e
            .handle_signal(&signal(10 + REVERT_COOLDOWN_BLOCKS - 1))
            .await
            .unwrap();
        assert_eq!(during, Outcome::Skipped("liquidation reverted recently"));
        assert_eq!(e.chain.sent.lock().unwrap().len(), 1);

        // Cooldown over: the borrower is eligible again.
        *e.chain.block.lock().unwrap() = 10 + REVERT_COOLDOWN_BLOCKS;
        let retry = e
            .handle_signal(&signal(10 + REVERT_COOLDOWN_BLOCKS))
            .await
            .unwrap();
        assert!(
            matches!(retry, Outcome::Struck { nonce: 1, .. }),
            "got {retry:?}"
        );
    }

    #[tokio::test]
    async fn wait_strategy_does_not_send() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        *chain.auction.lock().unwrap() = 10;
        *chain.bonus.lock().unwrap() = 0;
        let mut e = Executor::new(
            chain,
            FixedIntents(vec![stored()]),
            Box::new(WaitForMargin { margin_bps: 12_000 }),
            cfg(),
        );
        assert_eq!(
            e.handle_signal(&signal(10)).await.unwrap(),
            Outcome::Waiting
        );
        assert!(e.chain.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unconfirmed_strike_is_replaced_with_same_nonce_and_higher_fee() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        *chain.auction.lock().unwrap() = 10;
        *chain.bonus.lock().unwrap() = 500;
        let mut e = exec(chain, Some(stored()));
        e.handle_signal(&signal(10)).await.unwrap();
        let original_fee = e.chain.sent.lock().unwrap()[0].max_fee_per_gas;

        *e.chain.block.lock().unwrap() = 13; // 3 blocks later, still pending
        e.tick().await.unwrap();

        let sent = e.chain.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].nonce, sent[0].nonce, "replacement reuses the nonce");
        assert!(
            sent[1].max_fee_per_gas * 8 >= original_fee * 9,
            "fee rose by at least 12.5%"
        );
    }

    #[tokio::test]
    async fn dropped_but_still_needed_transaction_is_resent_with_same_nonce() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        *chain.auction.lock().unwrap() = 10;
        *chain.bonus.lock().unwrap() = 500;
        let mut e = exec(chain, Some(stored()));
        e.handle_signal(&signal(10)).await.unwrap();

        // The node forgets the transaction.
        e.chain.statuses.lock().unwrap().clear();
        *e.chain.block.lock().unwrap() = 11;
        e.tick().await.unwrap();

        let sent = e.chain.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].nonce, sent[0].nonce);
        assert_eq!(
            sent[1].to, POOL,
            "a resend is the original call, not a self-transfer"
        );
        assert!(!sent[1].data.is_empty());
    }

    #[tokio::test]
    async fn dropped_and_no_longer_needed_transaction_is_gap_filled() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        *chain.auction.lock().unwrap() = 10;
        *chain.bonus.lock().unwrap() = 500;
        let mut e = exec(chain, Some(stored()));
        e.handle_signal(&signal(10)).await.unwrap();

        // Someone else liquidated the position, so it is healthy again.
        e.chain.statuses.lock().unwrap().clear();
        *e.chain.hf.lock().unwrap() = U256::from(2u64) * U256::from(10u64).pow(U256::from(18u64));
        *e.chain.block.lock().unwrap() = 11;
        e.tick().await.unwrap();

        let sent = e.chain.sent.lock().unwrap();
        let last = sent.last().unwrap();
        assert_eq!(last.nonce, sent[0].nonce, "the hole is filled, not skipped");
        assert_eq!(
            last.to, LIQUIDATOR,
            "gap fill is a zero-value self-transfer"
        );
        assert!(last.data.is_empty());
    }

    #[tokio::test]
    async fn redelivered_signal_does_not_send_twice() {
        let chain = FakeChain::default();
        unhealthy(&chain, 10);
        let mut e = exec(chain, Some(stored()));
        assert!(matches!(
            e.handle_signal(&signal(10)).await.unwrap(),
            Outcome::StartedAuction { .. }
        ));
        // The same signal arrives again before the auction transaction confirms.
        assert_eq!(
            e.handle_signal(&signal(10)).await.unwrap(),
            Outcome::Skipped("already in flight")
        );
        assert_eq!(e.chain.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reconcile_keeps_nonces_moving_forward_after_restart() {
        let chain = FakeChain::default();
        *chain.latest.lock().unwrap() = 17;
        *chain.pending.lock().unwrap() = 19;
        unhealthy(&chain, 10);
        *chain.auction.lock().unwrap() = 10;
        *chain.bonus.lock().unwrap() = 500;
        let mut e = exec(chain, Some(stored()));
        e.reconcile().await.unwrap();
        e.handle_signal(&signal(10)).await.unwrap();
        assert_eq!(e.chain.sent.lock().unwrap()[0].nonce, 19);
    }
}
