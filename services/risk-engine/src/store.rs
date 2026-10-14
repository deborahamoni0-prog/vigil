//! Postgres implementation of `RiskStore`. Each commit and each undo is one transaction, so a
//! crash can never leave a position updated without its processed-event record (RS-05, RS-06).

use std::str::FromStr;

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use vigil_common::model::Position;

use crate::engine::{EventKey, RiskStore};

pub struct PgRiskStore {
    pool: PgPool,
}

impl PgRiskStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn u(s: &str) -> anyhow::Result<U256> {
    Ok(U256::from_str(s)?)
}

fn position_from_row(row: &sqlx::postgres::PgRow) -> anyhow::Result<Position> {
    let bitmap: i16 = row.try_get("bitmap")?;
    Ok(Position {
        bitmap: bitmap as u8,
        debt_scaled: u(&row.try_get::<String, _>("debt_scaled")?)?,
        amounts: [
            u(&row.try_get::<String, _>("amount_0")?)?,
            u(&row.try_get::<String, _>("amount_1")?)?,
            u(&row.try_get::<String, _>("amount_2")?)?,
        ],
    })
}

async fn write_position(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    borrower: Address,
    p: &Position,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO positions (borrower, bitmap, debt_scaled, amount_0, amount_1, amount_2)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (borrower) DO UPDATE SET bitmap = EXCLUDED.bitmap, debt_scaled = EXCLUDED.debt_scaled,
           amount_0 = EXCLUDED.amount_0, amount_1 = EXCLUDED.amount_1, amount_2 = EXCLUDED.amount_2",
    )
    .bind(format!("{borrower:#x}"))
    .bind(p.bitmap as i16)
    .bind(p.debt_scaled.to_string())
    .bind(p.amounts[0].to_string())
    .bind(p.amounts[1].to_string())
    .bind(p.amounts[2].to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[async_trait]
impl RiskStore for PgRiskStore {
    async fn load_position(&self, borrower: Address) -> anyhow::Result<Position> {
        let row = sqlx::query(
            "SELECT bitmap, debt_scaled, amount_0, amount_1, amount_2 FROM positions WHERE borrower = $1",
        )
        .bind(format!("{borrower:#x}"))
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => position_from_row(&r),
            None => Ok(Position::default()),
        }
    }

    async fn all_with_debt(&self) -> anyhow::Result<Vec<(Address, Position)>> {
        let rows = sqlx::query(
            "SELECT borrower, bitmap, debt_scaled, amount_0, amount_1, amount_2 FROM positions WHERE debt_scaled <> '0'",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                let borrower: String = r.try_get("borrower")?;
                Ok((borrower.parse::<Address>()?, position_from_row(r)?))
            })
            .collect()
    }

    async fn commit_applied(
        &self,
        key: EventKey,
        borrower: Address,
        before: Position,
        after: Position,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query("INSERT INTO processed_events (block_hash, log_index) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(format!("{:#x}", key.0))
            .bind(key.1 as i64)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if inserted == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query("INSERT INTO applied_events (block_hash, log_index, borrower, before) VALUES ($1, $2, $3, $4)")
            .bind(format!("{:#x}", key.0))
            .bind(key.1 as i64)
            .bind(format!("{borrower:#x}"))
            .bind(serde_json::to_value(before)?)
            .execute(&mut *tx)
            .await?;
        write_position(&mut tx, borrower, &after).await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn undo(&self, key: EventKey) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT borrower, before FROM applied_events WHERE block_hash = $1 AND log_index = $2 FOR UPDATE")
            .bind(format!("{:#x}", key.0))
            .bind(key.1 as i64)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(false);
        };
        let borrower: String = row.try_get("borrower")?;
        let before: Position = serde_json::from_value(row.try_get("before")?)?;
        write_position(&mut tx, borrower.parse::<Address>()?, &before).await?;
        sqlx::query("DELETE FROM applied_events WHERE block_hash = $1 AND log_index = $2")
            .bind(format!("{:#x}", key.0))
            .bind(key.1 as i64)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM processed_events WHERE block_hash = $1 AND log_index = $2")
            .bind(format!("{:#x}", key.0))
            .bind(key.1 as i64)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}
