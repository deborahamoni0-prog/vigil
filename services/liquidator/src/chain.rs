//! What the Liquidator needs from the chain, behind a trait so the executor can be tested
//! against a fake. The alloy implementation is in `rpc.rs`.

use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;

/// Fee and placement of one outgoing transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxRequest {
    pub nonce: u64,
    pub to: Address,
    pub data: Vec<u8>,
    pub value: U256,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub gas_limit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    /// Mined in a block that is still canonical and succeeded.
    Confirmed { block: u64 },
    /// Mined in a block that is still canonical, but the call reverted. The nonce is used up.
    Reverted { block: u64 },
    /// Known to the node but not yet mined.
    Pending,
    /// Unknown to the node. Either dropped or never arrived.
    Unknown,
}

#[async_trait]
pub trait TxChain: Send + Sync {
    fn address(&self) -> Address;
    async fn block_number(&self) -> anyhow::Result<u64>;
    async fn block_timestamp(&self) -> anyhow::Result<u64>;
    /// Transaction count including the mempool.
    async fn pending_nonce(&self) -> anyhow::Result<u64>;
    /// Transaction count at the latest block.
    async fn latest_nonce(&self) -> anyhow::Result<u64>;
    async fn send(&self, req: &TxRequest) -> anyhow::Result<B256>;
    async fn status(&self, hash: B256) -> anyhow::Result<TxStatus>;
    async fn health_factor(&self, borrower: Address) -> anyhow::Result<U256>;
    async fn bonus_bps(&self, borrower: Address, collateral: Address) -> anyhow::Result<u64>;
    async fn auction_start(&self, borrower: Address) -> anyhow::Result<u64>;
    async fn gas_price(&self) -> anyhow::Result<u128>;
    /// Whether the pool has already consumed this borrower-signed intent nonce.
    async fn nonce_used(&self, borrower: Address, nonce: U256) -> anyhow::Result<bool>;
}
