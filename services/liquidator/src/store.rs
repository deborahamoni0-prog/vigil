//! Reads the borrower-signed intents from Postgres. The Liquidator only reads them, never writes.

use std::str::FromStr;

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use vigil_common::contracts::LiquidationIntent;

use crate::executor::{IntentStore, StoredIntent};

pub struct PgIntentStore {
    pool: PgPool,
}

impl PgIntentStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl IntentStore for PgIntentStore {
    async fn intents_for(&self, borrower: Address) -> anyhow::Result<Vec<StoredIntent>> {
        let rows = sqlx::query(
            "SELECT nonce, collateral_asset, max_repay, min_collateral_out, deadline, signature
             FROM intents WHERE borrower = $1 ORDER BY nonce",
        )
        .bind(format!("{borrower:#x}"))
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|row| {
                let signature: String = row.try_get("signature")?;
                Ok(StoredIntent {
                    intent: LiquidationIntent {
                        borrower,
                        collateralAsset: Address::from_str(
                            &row.try_get::<String, _>("collateral_asset")?,
                        )?,
                        maxRepay: U256::from_str(&row.try_get::<String, _>("max_repay")?)?,
                        minCollateralOut: U256::from_str(
                            &row.try_get::<String, _>("min_collateral_out")?,
                        )?,
                        nonce: U256::from_str(&row.try_get::<String, _>("nonce")?)?,
                        deadline: U256::from(row.try_get::<i64, _>("deadline")? as u64),
                    },
                    signature: alloy::hex::decode(signature.trim_start_matches("0x"))?,
                })
            })
            .collect()
    }
}

/// Persists every transaction the Liquidator sends, so the gateway can show them and audits can
/// check nonce continuity after the fact.
pub struct PgTxLog {
    pool: PgPool,
}

impl PgTxLog {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl crate::executor::TxLog for PgTxLog {
    async fn sent(&self, tx: &crate::nonce::InFlight, kind: &str) -> anyhow::Result<()> {
        let borrower = match &tx.purpose {
            crate::nonce::TxPurpose::StartAuction { borrower }
            | crate::nonce::TxPurpose::Liquidate { borrower, .. } => {
                format!("{borrower:#x}")
            }
            crate::nonce::TxPurpose::GapFill => String::new(),
        };
        sqlx::query(
            "INSERT INTO liquidator_txs (nonce, kind, borrower, tx_hash, max_fee_wei, priority_wei, sent_block, status, trace_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', $8)
             ON CONFLICT (nonce) DO UPDATE SET tx_hash = EXCLUDED.tx_hash, max_fee_wei = EXCLUDED.max_fee_wei,
               priority_wei = EXCLUDED.priority_wei, sent_block = EXCLUDED.sent_block, status = 'pending'",
        )
        .bind(tx.nonce as i64)
        .bind(kind)
        .bind(borrower)
        .bind(tx.hash.map(|h| format!("{h:#x}")).unwrap_or_default())
        .bind(tx.max_fee_per_gas.to_string())
        .bind(tx.max_priority_fee_per_gas.to_string())
        .bind(tx.sent_block as i64)
        .bind(&tx.trace_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn set_status(&self, nonce: u64, status: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE liquidator_txs SET status = $2 WHERE nonce = $1")
            .bind(nonce as i64)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn pending(&self) -> anyhow::Result<Vec<(u64, alloy::primitives::B256)>> {
        let rows = sqlx::query(
            "SELECT nonce, tx_hash FROM liquidator_txs WHERE status = 'pending' AND tx_hash <> ''",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let nonce: i64 = sqlx::Row::get(&row, "nonce");
            let hash: String = sqlx::Row::get(&row, "tx_hash");
            out.push((nonce as u64, hash.parse()?));
        }
        Ok(out)
    }
}
