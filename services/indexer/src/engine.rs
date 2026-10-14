//! The Indexer's main loop, written against traits so the reorg path can be tested with a fake
//! chain (RS-01 to RS-04).
//!
//! Ordering rule (section 6.2): on a reorg, every Retract is published before any log from the new
//! canonical blocks. Cursor is saved after publishing, so a crash replays events (at-least-once),
//! and consumers deduplicate on (block_hash, log_index).

use alloy::primitives::B256;
use async_trait::async_trait;
use vigil_common::messages::{trace_id_for, BlockMessage, EventMessage, MessageKind, PoolEvent};

use crate::reorg::{self, BlockRef, Continuity, RecentBlocks, REORG_DEPTH};

/// A block as the chain reports it, with its timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInfo {
    pub block: BlockRef,
    pub timestamp: u64,
}

/// A decoded pool log, before it is wrapped into a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedLog {
    pub log_index: u64,
    pub event: PoolEvent,
}

#[async_trait]
pub trait ChainReader: Send + Sync {
    async fn head_number(&self) -> anyhow::Result<u64>;
    async fn block_by_number(&self, number: u64) -> anyhow::Result<Option<BlockInfo>>;
    /// Logs of one block, fetched by hash so a reorg cannot mix blocks (section 6.2).
    async fn logs_by_hash(&self, hash: B256) -> anyhow::Result<Vec<DecodedLog>>;
    /// Oracle and pool state as of the given block.
    async fn block_message(&self, info: &BlockInfo) -> anyhow::Result<BlockMessage>;
}

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn publish_event(&self, msg: &EventMessage) -> anyhow::Result<()>;
    async fn publish_block(&self, msg: &BlockMessage) -> anyhow::Result<()>;
}

/// Durable state: the cursor and the logs published for each recent block, so a reorg after a
/// restart can still retract them.
#[async_trait]
pub trait IndexerStore: Send + Sync {
    async fn load_cursor(&self) -> anyhow::Result<Option<BlockRef>>;
    async fn save_cursor(&self, block: &BlockRef) -> anyhow::Result<()>;
    /// Stores the messages published for a block, replacing any earlier copy for that number.
    async fn save_block_logs(
        &self,
        block: &BlockRef,
        messages: &[EventMessage],
    ) -> anyhow::Result<()>;
    /// Messages stored for blocks above `number`, returned in ascending order.
    async fn logs_above(&self, number: u64) -> anyhow::Result<Vec<EventMessage>>;
    /// Drops stored logs for blocks above `number`.
    async fn forget_above(&self, number: u64) -> anyhow::Result<()>;
}

pub struct Indexer<C, S, D> {
    pub chain: C,
    pub sink: S,
    pub store: D,
    pub recent: RecentBlocks,
    pub next: u64,
}

impl<C: ChainReader, S: EventSink, D: IndexerStore> Indexer<C, S, D> {
    /// Resumes from the stored cursor. Rebuilds the recent-block ring from the chain.
    pub async fn resume(chain: C, sink: S, store: D, start_block: u64) -> anyhow::Result<Self> {
        let mut recent = RecentBlocks::new(REORG_DEPTH);
        let next = match store.load_cursor().await? {
            Some(cursor) => {
                // Rebuild the ring from the chain up to the cursor, so the next head is checked.
                let from = cursor
                    .number
                    .saturating_sub(REORG_DEPTH as u64 - 1)
                    .max(start_block);
                for n in from..=cursor.number {
                    if let Some(info) = chain.block_by_number(n).await? {
                        recent.push(info.block);
                    }
                }
                cursor.number + 1
            }
            None => start_block,
        };
        Ok(Self {
            chain,
            sink,
            store,
            recent,
            next,
        })
    }

    /// Processes every block up to the current head. Returns how many blocks were handled.
    pub async fn catch_up(&mut self) -> anyhow::Result<usize> {
        let head = self.chain.head_number().await?;
        let mut handled = 0;
        while self.next <= head {
            let Some(info) = self.chain.block_by_number(self.next).await? else {
                break; // the node has not served this block yet; try again next poll
            };
            match reorg::continuity(&self.recent, &info.block) {
                Continuity::Extends | Continuity::Fresh => {
                    self.apply_block(info).await?;
                    handled += 1;
                }
                Continuity::Reorged => {
                    self.handle_reorg().await?;
                    // Re-enter the loop from the ancestor. The new block is re-read then.
                }
            }
        }
        Ok(handled)
    }

    async fn apply_block(&mut self, info: BlockInfo) -> anyhow::Result<()> {
        let block = info.block;
        let logs = self.chain.logs_by_hash(block.hash).await?;
        let messages: Vec<EventMessage> = logs
            .into_iter()
            .map(|l| EventMessage {
                kind: MessageKind::Log,
                block_number: block.number,
                block_hash: block.hash,
                log_index: l.log_index,
                trace_id: trace_id_for(block.hash, l.log_index),
                event: l.event,
            })
            .collect();

        // Publish first, then save the cursor. A crash in between replays, which is safe because
        // consumers deduplicate on the idempotency key.
        for m in &messages {
            self.sink.publish_event(m).await?;
        }
        let state = self.chain.block_message(&info).await?;
        self.sink.publish_block(&state).await?;

        self.store.save_block_logs(&block, &messages).await?;
        self.store.save_cursor(&block).await?;
        self.recent.push(block);
        self.next = block.number + 1;
        Ok(())
    }

    async fn handle_reorg(&mut self) -> anyhow::Result<()> {
        let chain = &self.chain;
        let ancestor = reorg::find_ancestor(&self.recent, |n| async move {
            Ok(chain.block_by_number(n).await?.map(|i| i.block.hash))
        })
        .await?;

        // Fail closed: if the fork is deeper than the ring, stop and let an operator look.
        let Some(ancestor) = ancestor else {
            anyhow::bail!("reorg deeper than the {REORG_DEPTH}-block ring; refusing to continue");
        };

        // Retracts first, newest first, then the canonical blocks are re-indexed by the loop.
        let mut orphaned = self.store.logs_above(ancestor).await?;
        orphaned.reverse();
        for old in orphaned {
            let retract = EventMessage {
                kind: MessageKind::Retract,
                ..old
            };
            self.sink.publish_event(&retract).await?;
        }
        self.store.forget_above(ancestor).await?;
        self.recent.truncate_above(ancestor);
        if let Some(b) = self.recent.last().copied() {
            self.store.save_cursor(&b).await?;
        }
        self.next = ancestor + 1;
        tracing::warn!(ancestor, "reorg handled: retracted orphaned logs");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;

    use alloy::primitives::U256;
    use vigil_common::model::{PoolState, Quote};

    use super::*;

    /// Hash for block `n` on fork `fork`. Distinct forks never collide.
    fn hash(n: u64, fork: u8) -> B256 {
        let mut b = [0u8; 32];
        b[0] = fork;
        b[24..32].copy_from_slice(&n.to_be_bytes());
        B256::from(b)
    }

    /// Chain of blocks `from..=to` on `fork`, linked to `parent_fork` below `from`.
    fn fork_blocks(from: u64, to: u64, fork: u8, parent_fork: u8) -> Vec<BlockInfo> {
        (from..=to)
            .map(|n| {
                let parent_f = if n == from { parent_fork } else { fork };
                BlockInfo {
                    block: BlockRef {
                        number: n,
                        hash: hash(n, fork),
                        parent: hash(n - 1, parent_f),
                    },
                    timestamp: 1_000 + n,
                }
            })
            .collect()
    }

    #[derive(Default)]
    struct FakeChain {
        blocks: Mutex<BTreeMap<u64, BlockInfo>>,
        logs: Mutex<HashMap<B256, Vec<DecodedLog>>>,
    }

    impl FakeChain {
        fn install(&self, blocks: &[BlockInfo], logs_per_block: usize, tag: &str) {
            let mut b = self.blocks.lock().unwrap();
            let mut l = self.logs.lock().unwrap();
            for info in blocks {
                b.insert(info.block.number, *info);
                let logs = (0..logs_per_block)
                    .map(|i| DecodedLog {
                        log_index: i as u64,
                        event: PoolEvent::Other {
                            name: format!("{tag}-{}-{i}", info.block.number),
                        },
                    })
                    .collect();
                l.insert(info.block.hash, logs);
            }
        }
    }

    #[async_trait]
    impl ChainReader for FakeChain {
        async fn head_number(&self) -> anyhow::Result<u64> {
            Ok(*self.blocks.lock().unwrap().keys().next_back().unwrap_or(&0))
        }
        async fn block_by_number(&self, number: u64) -> anyhow::Result<Option<BlockInfo>> {
            Ok(self.blocks.lock().unwrap().get(&number).copied())
        }
        async fn logs_by_hash(&self, hash: B256) -> anyhow::Result<Vec<DecodedLog>> {
            Ok(self
                .logs
                .lock()
                .unwrap()
                .get(&hash)
                .cloned()
                .unwrap_or_default())
        }
        async fn block_message(&self, info: &BlockInfo) -> anyhow::Result<BlockMessage> {
            Ok(BlockMessage {
                block_number: info.block.number,
                block_hash: info.block.hash,
                timestamp: info.timestamp,
                frozen: false,
                state: PoolState {
                    total_supply_scaled: U256::ZERO,
                    total_debt_scaled: U256::ZERO,
                    supply_index: U256::ZERO,
                    debt_index: U256::ZERO,
                    last_accrual: info.timestamp,
                },
                quote: Quote {
                    debt_index: U256::ZERO,
                    debt_price: U256::ZERO,
                    collateral_prices: [U256::ZERO; 3],
                },
                trace_id: String::new(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<EventMessage>>,
    }

    #[async_trait]
    impl EventSink for RecordingSink {
        async fn publish_event(&self, msg: &EventMessage) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(msg.clone());
            Ok(())
        }
        async fn publish_block(&self, _msg: &BlockMessage) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemStore {
        cursor: Mutex<Option<BlockRef>>,
        logs: Mutex<BTreeMap<u64, (B256, Vec<EventMessage>)>>,
    }

    #[async_trait]
    impl IndexerStore for MemStore {
        async fn load_cursor(&self) -> anyhow::Result<Option<BlockRef>> {
            Ok(*self.cursor.lock().unwrap())
        }
        async fn save_cursor(&self, block: &BlockRef) -> anyhow::Result<()> {
            *self.cursor.lock().unwrap() = Some(*block);
            Ok(())
        }
        async fn save_block_logs(
            &self,
            block: &BlockRef,
            messages: &[EventMessage],
        ) -> anyhow::Result<()> {
            self.logs
                .lock()
                .unwrap()
                .insert(block.number, (block.hash, messages.to_vec()));
            Ok(())
        }
        async fn logs_above(&self, number: u64) -> anyhow::Result<Vec<EventMessage>> {
            Ok(self
                .logs
                .lock()
                .unwrap()
                .range(number + 1..)
                .flat_map(|(_, (_, m))| m.clone())
                .collect())
        }
        async fn forget_above(&self, number: u64) -> anyhow::Result<()> {
            self.logs.lock().unwrap().retain(|n, _| *n <= number);
            Ok(())
        }
    }

    fn names(events: &[EventMessage]) -> Vec<String> {
        events
            .iter()
            .map(|m| {
                let tag = match m.kind {
                    MessageKind::Log => "log",
                    MessageKind::Retract => "retract",
                };
                let name = match &m.event {
                    PoolEvent::Other { name } => name.clone(),
                    _ => String::new(),
                };
                format!("{tag}:{name}")
            })
            .collect()
    }

    #[tokio::test]
    async fn linear_chain_publishes_in_order_and_saves_cursor() {
        let chain = FakeChain::default();
        chain.install(&fork_blocks(1, 3, 0, 0), 1, "A");
        let mut ix = Indexer::resume(chain, RecordingSink::default(), MemStore::default(), 1)
            .await
            .unwrap();

        assert_eq!(ix.catch_up().await.unwrap(), 3);
        assert_eq!(
            names(&ix.sink.events.lock().unwrap()),
            vec!["log:A-1-0", "log:A-2-0", "log:A-3-0"]
        );
        assert_eq!(ix.store.load_cursor().await.unwrap().unwrap().number, 3);
    }

    #[tokio::test]
    async fn two_block_reorg_retracts_newest_first_before_new_logs() {
        let chain = FakeChain::default();
        chain.install(&fork_blocks(1, 4, 0, 0), 1, "A");
        let mut ix = Indexer::resume(chain, RecordingSink::default(), MemStore::default(), 1)
            .await
            .unwrap();
        ix.catch_up().await.unwrap();

        // Fork B splits after block 2, so blocks 3 and 4 are replaced, and it extends to 5.
        ix.chain.install(&fork_blocks(3, 5, 1, 0), 1, "B");
        ix.chain
            .blocks
            .lock()
            .unwrap()
            .insert(3, fork_blocks(3, 3, 1, 0)[0]);
        ix.chain
            .blocks
            .lock()
            .unwrap()
            .insert(4, fork_blocks(4, 4, 1, 0)[0]);
        ix.chain
            .blocks
            .lock()
            .unwrap()
            .insert(5, fork_blocks(5, 5, 1, 0)[0]);
        // Blocks 3 and 4 on fork B must link to block 2 of fork A (hash(2, 0)).
        {
            let mut b = ix.chain.blocks.lock().unwrap();
            for n in 3..=5 {
                let parent = if n == 3 { hash(2, 0) } else { hash(n - 1, 1) };
                let info = b.get_mut(&n).unwrap();
                info.block.parent = parent;
            }
        }
        ix.sink.events.lock().unwrap().clear();

        ix.catch_up().await.unwrap();
        let published = names(&ix.sink.events.lock().unwrap());
        assert_eq!(
            published,
            vec![
                "retract:A-4-0",
                "retract:A-3-0",
                "log:B-3-0",
                "log:B-4-0",
                "log:B-5-0",
            ],
            "retracts must precede new logs, newest first"
        );
        assert_eq!(ix.recent.last().unwrap().hash, hash(5, 1));
    }

    #[tokio::test]
    async fn restart_resumes_without_duplicates() {
        let chain = FakeChain::default();
        chain.install(&fork_blocks(1, 5, 0, 0), 1, "A");
        let store = MemStore::default();
        {
            let mut ix = Indexer::resume(chain, RecordingSink::default(), store, 1)
                .await
                .unwrap();
            // Index only blocks 1..=3, as if the process stopped there.
            ix.next = 1;
            for n in 1..=3 {
                let info = ix.chain.block_by_number(n).await.unwrap().unwrap();
                ix.apply_block(info).await.unwrap();
            }
            // Move the store and the chain into the restarted indexer.
            let Indexer { chain, store, .. } = ix;
            let mut restarted = Indexer::resume(chain, RecordingSink::default(), store, 1)
                .await
                .unwrap();
            assert_eq!(restarted.next, 4);
            restarted.catch_up().await.unwrap();
            assert_eq!(
                names(&restarted.sink.events.lock().unwrap()),
                vec!["log:A-4-0", "log:A-5-0"],
                "only new blocks are published after a restart"
            );
        }
    }
}
