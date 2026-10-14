//! Nonce book for the single liquidator wallet (RS-13, RS-14, section 6.4).
//!
//! The rules this encodes:
//! - Nonces are handed out in order and never reused, so there are no gaps or collisions.
//! - Every reserved nonce ends in exactly one of: confirmed, replaced, or gap-filled.
//! - A transaction still unconfirmed after `replace_after_blocks` is resent with the same nonce and
//!   a fee at least 12.5% higher (the minimum most nodes accept for replacement).
//! - A nonce whose transaction vanished is filled with a zero-value self-transfer, so later
//!   transactions are not stuck behind the hole.
//!
//! Pure state, so the rules are tested without a chain. The executor drives it.

use std::collections::BTreeMap;

use alloy::primitives::{Address, B256, U256};
use vigil_common::contracts::LiquidationIntent;

/// Fee bump in basis points. 1250 means +12.5%, the minimum most nodes accept for replacement.
pub const MIN_FEE_BUMP_BPS: u128 = 1250;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxPurpose {
    /// `startAuction(borrower)`
    StartAuction { borrower: Address },
    /// `liquidate(intent, signature, repayAmount)`
    Liquidate {
        borrower: Address,
        intent: Box<LiquidationIntent>,
        signature: Vec<u8>,
        repay: U256,
    },
    /// Zero-value self-transfer that fills a nonce hole.
    GapFill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlight {
    pub nonce: u64,
    pub hash: Option<B256>,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub sent_block: u64,
    pub purpose: TxPurpose,
    pub trace_id: String,
    pub replacements: u32,
}

/// Fee bumped by `MIN_FEE_BUMP_BPS`, rounded up, and always at least one wei higher.
pub fn bump_fee(fee: u128) -> u128 {
    let factor = 10_000 + MIN_FEE_BUMP_BPS;
    let bumped = (fee * factor).div_ceil(10_000);
    bumped.max(fee + 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing to do yet.
    Wait,
    /// Resend the same nonce with these fees.
    Replace {
        nonce: u64,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
    },
    /// The transaction is gone. Fill the nonce with a self-transfer.
    GapFill { nonce: u64 },
}

#[derive(Debug, Default)]
pub struct NonceBook {
    /// Next nonce to hand out.
    next: u64,
    inflight: BTreeMap<u64, InFlight>,
}

impl NonceBook {
    /// Startup reconciliation (RS-13). `latest` is the chain's confirmed count, `pending` includes
    /// the mempool. Anything we tracked below `latest` is settled.
    pub fn reconcile(&mut self, latest: u64, pending: u64) {
        self.next = self.next.max(latest).max(pending);
        self.inflight.retain(|nonce, _| *nonce >= latest);
    }

    /// Reserves the next nonce. Callers must end it with `record_sent` or `gap_fill`.
    pub fn reserve(&mut self) -> u64 {
        let n = self.next;
        self.next += 1;
        n
    }

    pub fn next_nonce(&self) -> u64 {
        self.next
    }

    pub fn record_sent(&mut self, tx: InFlight) {
        self.inflight.insert(tx.nonce, tx);
    }

    pub fn set_hash(&mut self, nonce: u64, hash: B256) {
        if let Some(t) = self.inflight.get_mut(&nonce) {
            t.hash = Some(hash);
        }
    }

    pub fn confirmed(&mut self, nonce: u64) -> Option<InFlight> {
        self.inflight.remove(&nonce)
    }

    pub fn in_flight(&self) -> impl Iterator<Item = &InFlight> {
        self.inflight.values()
    }

    pub fn get(&self, nonce: u64) -> Option<&InFlight> {
        self.inflight.get(&nonce)
    }

    /// Decides what to do with each unconfirmed transaction at `current_block`.
    /// `dropped` lists nonces the node no longer knows about (not mined, not pending).
    pub fn plan(
        &self,
        current_block: u64,
        replace_after_blocks: u64,
        dropped: &[u64],
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        for t in self.inflight.values() {
            if dropped.contains(&t.nonce) {
                actions.push(Action::GapFill { nonce: t.nonce });
            } else if current_block >= t.sent_block + replace_after_blocks {
                actions.push(Action::Replace {
                    nonce: t.nonce,
                    max_fee_per_gas: bump_fee(t.max_fee_per_gas),
                    max_priority_fee_per_gas: bump_fee(t.max_priority_fee_per_gas),
                });
            } else {
                actions.push(Action::Wait);
            }
        }
        actions
    }

    /// A replacement went out with new fees.
    pub fn replaced(&mut self, nonce: u64, max_fee: u128, priority: u128, sent_block: u64) {
        if let Some(t) = self.inflight.get_mut(&nonce) {
            t.max_fee_per_gas = max_fee;
            t.max_priority_fee_per_gas = priority;
            t.sent_block = sent_block;
            t.replacements += 1;
        }
    }

    /// A reorg removed a confirmed transaction. It must be sent again with the same nonce.
    pub fn reorged_out(&mut self, tx: InFlight) {
        self.inflight.insert(tx.nonce, tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(nonce: u64, sent_block: u64) -> InFlight {
        InFlight {
            nonce,
            hash: None,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee_per_gas: 1_000_000,
            sent_block,
            purpose: TxPurpose::GapFill,
            trace_id: String::new(),
            replacements: 0,
        }
    }

    #[test]
    fn nonces_are_handed_out_in_order_without_gaps() {
        let mut b = NonceBook::default();
        let got: Vec<u64> = (0..5).map(|_| b.reserve()).collect();
        assert_eq!(got, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn fee_bump_is_at_least_twelve_point_five_percent() {
        for fee in [1u128, 7, 8, 9, 1_000, 999_999_937, 1_000_000_000] {
            let bumped = bump_fee(fee);
            assert!(bumped > fee, "fee {fee} must rise");
            // bumped/fee >= 1.125, checked in integers: bumped * 8 >= fee * 9
            assert!(
                bumped * 8 >= fee * 9,
                "fee {fee} rose by less than 12.5% (got {bumped})"
            );
        }
        assert_eq!(bump_fee(1_000), 1_125);
    }

    #[test]
    fn unconfirmed_transaction_is_replaced_after_n_blocks() {
        let mut b = NonceBook::default();
        b.record_sent(tx(0, 100));
        assert_eq!(b.plan(101, 3, &[]), vec![Action::Wait]);
        assert_eq!(
            b.plan(103, 3, &[]),
            vec![Action::Replace {
                nonce: 0,
                max_fee_per_gas: bump_fee(1_000_000_000),
                max_priority_fee_per_gas: bump_fee(1_000_000),
            }]
        );
    }

    #[test]
    fn dropped_transaction_is_gap_filled_not_skipped() {
        let mut b = NonceBook::default();
        b.record_sent(tx(0, 100));
        b.record_sent(tx(1, 100));
        // Nonce 0 vanished from the node. Nonce 1 is still pending.
        let plan = b.plan(101, 3, &[0]);
        assert!(plan.contains(&Action::GapFill { nonce: 0 }));
        assert!(plan.contains(&Action::Wait));
    }

    #[test]
    fn reconcile_settles_confirmed_nonces_and_keeps_pending_ones() {
        let mut b = NonceBook::default();
        b.record_sent(tx(0, 10));
        b.record_sent(tx(1, 10));
        b.record_sent(tx(2, 10));
        // Chain says nonces 0 and 1 are mined and 2 is in the mempool.
        b.reconcile(2, 3);
        assert!(b.get(0).is_none());
        assert!(b.get(1).is_none());
        assert!(b.get(2).is_some());
        assert_eq!(
            b.next_nonce(),
            3,
            "never hands out a nonce the chain has already used"
        );
    }

    #[test]
    fn reconcile_never_moves_next_backwards() {
        let mut b = NonceBook::default();
        for _ in 0..5 {
            b.reserve();
        }
        b.reconcile(2, 2);
        assert_eq!(b.next_nonce(), 5);
    }

    #[test]
    fn replacement_updates_fees_and_block() {
        let mut b = NonceBook::default();
        b.record_sent(tx(7, 100));
        b.replaced(7, 2_000, 200, 103);
        let t = b.get(7).unwrap();
        assert_eq!(
            (t.max_fee_per_gas, t.sent_block, t.replacements),
            (2_000, 103, 1)
        );
    }
}
