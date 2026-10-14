//! Reorg bookkeeping for the Indexer (RS-02, section 6.2). Pure logic, so it is unit-tested
//! without a chain.

use std::collections::VecDeque;

use alloy::primitives::B256;

/// Number of recent blocks kept for reorg detection. Only the last ~10 blocks can be retracted.
pub const REORG_DEPTH: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRef {
    pub number: u64,
    pub hash: B256,
    pub parent: B256,
}

/// Ring of the most recent canonical blocks, ascending by number.
#[derive(Debug, Clone)]
pub struct RecentBlocks {
    depth: usize,
    blocks: VecDeque<BlockRef>,
}

impl RecentBlocks {
    pub fn new(depth: usize) -> Self {
        Self { depth, blocks: VecDeque::with_capacity(depth) }
    }

    pub fn last(&self) -> Option<&BlockRef> {
        self.blocks.back()
    }

    pub fn get(&self, number: u64) -> Option<&BlockRef> {
        self.blocks.iter().find(|b| b.number == number)
    }

    pub fn push(&mut self, block: BlockRef) {
        self.blocks.push_back(block);
        while self.blocks.len() > self.depth {
            self.blocks.pop_front();
        }
    }

    /// Drops every block above `number`, keeping `number` itself.
    pub fn truncate_above(&mut self, number: u64) {
        while self.blocks.back().is_some_and(|b| b.number > number) {
            self.blocks.pop_back();
        }
    }

    pub fn lowest(&self) -> Option<u64> {
        self.blocks.front().map(|b| b.number)
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// How a new block relates to the blocks already seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continuity {
    /// The new block extends the recent chain.
    Extends,
    /// The new block's parent does not match what we recorded. The chain reorged.
    Reorged,
    /// Nothing recorded yet, so there is nothing to compare against.
    Fresh,
}

pub fn continuity(recent: &RecentBlocks, new: &BlockRef) -> Continuity {
    match recent.last() {
        None => Continuity::Fresh,
        Some(last) if new.number == last.number + 1 && new.parent == last.hash => Continuity::Extends,
        Some(last) if new.number == last.number + 1 => Continuity::Reorged,
        // Anything other than the next block is a caller bug, and the loop never produces it.
        Some(_) => Continuity::Reorged,
    }
}

/// Walks back from the last recorded block until the chain's hash matches the stored hash.
/// `canonical_hash` returns the chain's current hash for a block number. The returned number is
/// the common ancestor. `None` means no match within the ring, so the caller must fail closed.
pub async fn find_ancestor<F, Fut>(recent: &RecentBlocks, mut canonical_hash: F) -> Result<Option<u64>, anyhow::Error>
where
    F: FnMut(u64) -> Fut,
    Fut: std::future::Future<Output = Result<Option<B256>, anyhow::Error>>,
{
    let Some(last) = recent.last() else { return Ok(None) };
    let floor = recent.lowest().unwrap_or(last.number);
    let mut n = last.number;
    loop {
        if let Some(stored) = recent.get(n) {
            if canonical_hash(n).await? == Some(stored.hash) {
                return Ok(Some(n));
            }
        }
        if n == floor {
            return Ok(None);
        }
        n -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u64, tag: u8) -> B256 {
        let mut b = [0u8; 32];
        b[0] = tag;
        b[31] = n as u8;
        B256::from(b)
    }

    fn block(n: u64, tag: u8) -> BlockRef {
        BlockRef { number: n, hash: h(n, tag), parent: if n == 0 { B256::ZERO } else { h(n - 1, tag) } }
    }

    #[test]
    fn ring_keeps_only_depth() {
        let mut r = RecentBlocks::new(3);
        for n in 1..=5 {
            r.push(block(n, 0));
        }
        assert_eq!(r.lowest(), Some(3));
        assert_eq!(r.last().unwrap().number, 5);
        assert!(r.get(2).is_none());
    }

    #[test]
    fn extension_is_detected() {
        let mut r = RecentBlocks::new(10);
        r.push(block(1, 0));
        assert_eq!(continuity(&r, &block(2, 0)), Continuity::Extends);
        assert_eq!(continuity(&RecentBlocks::new(10), &block(2, 0)), Continuity::Fresh);
    }

    #[test]
    fn parent_mismatch_is_a_reorg() {
        let mut r = RecentBlocks::new(10);
        r.push(block(1, 0));
        r.push(block(2, 0));
        // Block 3 from another fork: its parent is block 2 of fork 1, so the parent hash differs.
        let fork = BlockRef { number: 3, hash: h(3, 9), parent: h(2, 9) };
        assert_eq!(continuity(&r, &fork), Continuity::Reorged);
    }

    #[tokio::test]
    async fn ancestor_is_found_below_the_orphans() {
        // Recorded chain: 1..=5 on fork 0. The canonical chain now uses fork 1 from block 4.
        let mut r = RecentBlocks::new(10);
        for n in 1..=5 {
            r.push(block(n, 0));
        }
        let canonical = |n: u64| async move {
            Ok::<_, anyhow::Error>(Some(if n <= 3 { h(n, 0) } else { h(n, 1) }))
        };
        let ancestor = find_ancestor(&r, canonical).await.unwrap();
        assert_eq!(ancestor, Some(3));
    }

    #[tokio::test]
    async fn deep_reorg_beyond_ring_returns_none() {
        let mut r = RecentBlocks::new(3);
        for n in 1..=3 {
            r.push(block(n, 0));
        }
        let canonical = |n: u64| async move { Ok::<_, anyhow::Error>(Some(h(n, 7))) };
        assert_eq!(find_ancestor(&r, canonical).await.unwrap(), None);
    }
}
