//! Messages on NATS JetStream. Every event carries (block_hash, log_index) as its idempotency
//! key and a trace id, so consumers can deduplicate and one liquidation can be traced end to end
//! (RS-04, section 6.5).

use alloy::primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

use crate::model::{PoolState, Quote};

pub const STREAM_NAME: &str = "VIGIL";
pub const SUBJECT_EVENTS: &str = "vigil.events";
pub const SUBJECT_BLOCKS: &str = "vigil.blocks";
pub const SUBJECT_SIGNALS: &str = "vigil.signals";
/// Every signal, for a consumer that wants all borrowers.
pub const SUBJECT_SIGNALS_FILTER: &str = "vigil.signals.>";

/// One subject per borrower, so a consumer can ask for only the latest signal of each.
pub fn signal_subject(borrower: alloy::primitives::Address) -> String {
    format!("{SUBJECT_SIGNALS}.{borrower:#x}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// The log was observed on the canonical chain.
    Log,
    /// The log was on a block that has since been orphaned. Undo it.
    Retract,
}

/// Decoded pool events, carrying only what the Risk Engine needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PoolEvent {
    CollateralDeposited {
        user: Address,
        asset: Address,
        amount: U256,
    },
    CollateralWithdrawn {
        user: Address,
        asset: Address,
        amount: U256,
    },
    CollateralEnabled {
        user: Address,
        asset: Address,
    },
    CollateralDisabled {
        user: Address,
        asset: Address,
    },
    Borrowed {
        user: Address,
        amount: U256,
        scaled: U256,
    },
    Repaid {
        user: Address,
        amount: U256,
        scaled: U256,
    },
    Liquidated {
        borrower: Address,
        liquidator: Address,
        asset: Address,
        repaid: U256,
        scaled_repaid: U256,
        seized: U256,
    },
    BadDebt {
        borrower: Address,
        scaled_debt: U256,
    },
    /// Logs the Risk Engine does not act on, kept so the stream is complete (for gap checks).
    Other {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMessage {
    pub kind: MessageKind,
    pub block_number: u64,
    pub block_hash: B256,
    pub log_index: u64,
    pub trace_id: String,
    pub event: PoolEvent,
}

impl EventMessage {
    /// Idempotency key (block_hash, log_index). Retract messages share the key of the log they undo.
    pub fn idempotency_key(&self) -> (B256, u64) {
        (self.block_hash, self.log_index)
    }
}

/// Per-block read of the oracle and pool views, evaluated at that block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockMessage {
    pub block_number: u64,
    pub block_hash: B256,
    pub timestamp: u64,
    pub frozen: bool,
    pub state: PoolState,
    pub quote: Quote,
    pub trace_id: String,
}

/// Emitted by the Risk Engine when a position is unhealthy and the oracle is Normal (RS-08, RS-09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidationSignal {
    pub borrower: Address,
    /// Block the health factor was computed at. Signals older than 2 blocks are stale (RS-09).
    pub block_number: u64,
    pub health_factor: U256,
    pub trace_id: String,
}

pub const MAX_SIGNAL_AGE_BLOCKS: u64 = 2;

pub fn trace_id_for(block_hash: B256, log_index: u64) -> String {
    format!("{block_hash:#x}:{log_index}")
}
