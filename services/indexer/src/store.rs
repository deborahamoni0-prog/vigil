//! Postgres-backed `IndexerStore`: the cursor and the logs published per recent block (RS-03).

use alloy::primitives::B256;
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use vigil_common::messages::EventMessage;

use crate::engine::IndexerStore;
use crate::reorg::BlockRef;

pub struct PgIndexerStore {
    pool: PgPool,
}

impl PgIndexerStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl IndexerStore for PgIndexerStore {
    async fn load_cursor(&self) -> anyhow::Result<Option<BlockRef>> {
        let row = sqlx::query(
            "SELECT block_number, block_hash, parent_hash FROM indexer_cursor WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let number: i64 = row.try_get("block_number")?;
        let hash: String = row.try_get("block_hash")?;
        let parent: String = row.try_get("parent_hash")?;
        Ok(Some(BlockRef {
            number: number as u64,
            hash: hash.parse::<B256>()?,
            parent: parent.parse::<B256>()?,
        }))
    }

    async fn save_cursor(&self, block: &BlockRef) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO indexer_cursor (id, block_number, block_hash, parent_hash) VALUES (1, $1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET block_number = EXCLUDED.block_number,
               block_hash = EXCLUDED.block_hash, parent_hash = EXCLUDED.parent_hash",
        )
        .bind(block.number as i64)
        .bind(format!("{:#x}", block.hash))
        .bind(format!("{:#x}", block.parent))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn save_block_logs(
        &self,
        block: &BlockRef,
        messages: &[EventMessage],
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM indexer_block_logs WHERE block_number = $1")
            .bind(block.number as i64)
            .execute(&mut *tx)
            .await?;
        for m in messages {
            sqlx::query(
                "INSERT INTO indexer_block_logs (block_number, block_hash, log_index, message)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(block.number as i64)
            .bind(format!("{:#x}", block.hash))
            .bind(m.log_index as i64)
            .bind(serde_json::to_value(m)?)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn logs_above(&self, number: u64) -> anyhow::Result<Vec<EventMessage>> {
        let rows = sqlx::query(
            "SELECT message FROM indexer_block_logs WHERE block_number > $1 ORDER BY block_number, log_index",
        )
        .bind(number as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let v: serde_json::Value = r.try_get("message")?;
                Ok(serde_json::from_value(v)?)
            })
            .collect()
    }

    async fn forget_above(&self, number: u64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM indexer_block_logs WHERE block_number > $1")
            .bind(number as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
