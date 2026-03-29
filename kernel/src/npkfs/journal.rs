//! Deferred Free Journal
//!
//! Circular WAL that tracks blocks freed during COW operations.
//! On crash recovery: replay committed frees to prevent space leaks.
//! Journal position advances forward (SSD-friendly, no repeated overwrites).

use alloc::vec::Vec;
use super::types::*;
use super::cache::BlockCache;

pub struct Journal {
    head: u64,  // next write block within journal area
    seq: u64,
    pending_frees: Vec<(u64, u64)>,
}

impl Journal {
    pub fn new(head: u64, seq: u64) -> Self {
        Journal { head, seq, pending_frees: Vec::new() }
    }

    pub fn head(&self) -> u64 { self.head }
    pub fn seq(&self) -> u64 { self.seq }

    /// Record blocks to be freed after next commit.
    pub fn record_free(&mut self, start: u64, count: u64) {
        if count > 0 {
            self.pending_frees.push((start, count));
        }
    }

    /// Write pending frees to journal and mark committed.
    /// Called BEFORE writing the new superblock.
    pub fn commit(&mut self, cache: &mut BlockCache) -> Result<(), FsError> {
        if self.pending_frees.is_empty() { return Ok(()); }

        let count = self.pending_frees.len().min(MAX_JOURNAL_ENTRIES);
        self.seq += 1;

        let mut buf = [0u8; BLOCK_SIZE];

        // Write header
        buf[0..8].copy_from_slice(&JOURNAL_MAGIC);
        buf[8..16].copy_from_slice(&self.seq.to_le_bytes());
        buf[16..20].copy_from_slice(&(count as u32).to_le_bytes());
        buf[20..24].copy_from_slice(&1u32.to_le_bytes()); // committed = 1

        // Write entries
        for (i, &(start, cnt)) in self.pending_frees.iter().take(count).enumerate() {
            let off = 24 + i * 16;
            buf[off..off + 8].copy_from_slice(&start.to_le_bytes());
            buf[off + 8..off + 16].copy_from_slice(&cnt.to_le_bytes());
        }

        let journal_block = JOURNAL_START + self.head;
        cache.write(journal_block, &buf)?;

        // Advance head (circular)
        self.head = (self.head + 1) % JOURNAL_BLOCKS;
        self.pending_frees.clear();
        Ok(())
    }

    /// Check if there's a committed journal entry that needs replay.
    /// Returns frees to apply (called during mount).
    pub fn replay(cache: &mut BlockCache, head: u64, expected_seq: u64) -> Result<Vec<(u64, u64)>, FsError> {
        // Check the block at head-1 (most recent write)
        let check_pos = if head == 0 { JOURNAL_BLOCKS - 1 } else { head - 1 };
        let journal_block = JOURNAL_START + check_pos;

        let mut buf = [0u8; BLOCK_SIZE];
        cache.read(journal_block, &mut buf)?;

        if buf[0..8] != JOURNAL_MAGIC { return Ok(Vec::new()); }

        let seq = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let count = u32::from_le_bytes(buf[16..20].try_into().unwrap()) as usize;
        let committed = u32::from_le_bytes(buf[20..24].try_into().unwrap());

        // Only replay if seq > what superblock recorded and entry is committed
        if committed != 1 || seq <= expected_seq { return Ok(Vec::new()); }

        let mut frees = Vec::with_capacity(count.min(MAX_JOURNAL_ENTRIES));
        for i in 0..count.min(MAX_JOURNAL_ENTRIES) {
            let off = 24 + i * 16;
            let start = u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
            let cnt = u64::from_le_bytes(buf[off + 8..off + 16].try_into().unwrap());
            frees.push((start, cnt));
        }
        Ok(frees)
    }
}
