//! Deferred Free Journal
//!
//! Circular WAL that tracks blocks freed during COW operations.
//! On crash recovery: replay committed frees to prevent space leaks.
//! Journal position advances forward (SSD-friendly, no repeated overwrites).
//!
//! Crash safety: journal entries are written with committed=0, then the
//! superblock is written, then entries are marked committed=1. On replay,
//! only committed=1 entries are processed, so a crash between journal write
//! and superblock write does NOT corrupt the filesystem.

use alloc::vec::Vec;
use super::types::*;
use super::cache::BlockCache;

pub struct Journal {
    head: u64,  // next write block within journal area
    seq: u64,
    pending_frees: Vec<(u64, u64)>,
    // Track which journal blocks were written this commit (for post-commit mark)
    uncommitted_blocks: Vec<u64>,
}

impl Journal {
    pub fn new(head: u64, seq: u64) -> Self {
        Journal { head, seq, pending_frees: Vec::new(), uncommitted_blocks: Vec::new() }
    }

    pub fn head(&self) -> u64 { self.head }
    pub fn seq(&self) -> u64 { self.seq }

    /// Record blocks to be freed after next commit.
    pub fn record_free(&mut self, start: u64, count: u64) {
        if count > 0 {
            self.pending_frees.push((start, count));
        }
    }

    /// Phase 1: Write pending frees to journal with committed=0.
    /// Called BEFORE writing the new superblock.
    pub fn prepare(&mut self, cache: &mut BlockCache) -> Result<(), FsError> {
        if self.pending_frees.is_empty() { return Ok(()); }

        self.seq += 1;
        self.uncommitted_blocks.clear();

        // Write ALL pending frees across multiple journal blocks if needed
        let mut offset = 0;
        while offset < self.pending_frees.len() {
            let remaining = self.pending_frees.len() - offset;
            let count = remaining.min(MAX_JOURNAL_ENTRIES);

            let mut buf = [0u8; BLOCK_SIZE];
            // Header
            buf[0..8].copy_from_slice(&JOURNAL_MAGIC);
            buf[8..16].copy_from_slice(&self.seq.to_le_bytes());
            buf[16..20].copy_from_slice(&(count as u32).to_le_bytes());
            buf[20..24].copy_from_slice(&0u32.to_le_bytes()); // committed = 0 (NOT YET!)

            // Entries
            for i in 0..count {
                let (start, cnt) = self.pending_frees[offset + i];
                let off = 24 + i * 16;
                buf[off..off + 8].copy_from_slice(&start.to_le_bytes());
                buf[off + 8..off + 16].copy_from_slice(&cnt.to_le_bytes());
            }

            let journal_block = JOURNAL_START + self.head;
            cache.write(journal_block, &buf)?;
            self.uncommitted_blocks.push(journal_block);

            self.head = (self.head + 1) % JOURNAL_BLOCKS;
            offset += count;
        }

        self.pending_frees.clear();
        Ok(())
    }

    /// Phase 2: Mark journal entries as committed.
    /// Called AFTER writing the new superblock.
    pub fn finalize(&mut self, cache: &mut BlockCache) -> Result<(), FsError> {
        for &journal_block in &self.uncommitted_blocks {
            let mut buf = [0u8; BLOCK_SIZE];
            cache.read(journal_block, &mut buf)?;
            // Set committed = 1
            buf[20..24].copy_from_slice(&1u32.to_le_bytes());
            cache.write(journal_block, &buf)?;
        }
        self.uncommitted_blocks.clear();
        Ok(())
    }

    /// Check for committed journal entries that need replay (called during mount).
    /// Scans backwards from head for entries with matching seq.
    pub fn replay(cache: &mut BlockCache, head: u64, expected_seq: u64) -> Result<Vec<(u64, u64)>, FsError> {
        let mut frees = Vec::new();

        // Scan backwards from head, looking for committed entries with seq > expected
        // (entries the superblock acknowledges but whose frees weren't executed yet)
        for i in 0..JOURNAL_BLOCKS {
            let check_pos = if head >= 1 + i {
                head - 1 - i
            } else {
                JOURNAL_BLOCKS - 1 - i + head
            };
            let journal_block = JOURNAL_START + check_pos;

            let mut buf = [0u8; BLOCK_SIZE];
            cache.read(journal_block, &mut buf)?;

            if buf[0..8] != JOURNAL_MAGIC { break; }

            let seq = u64::from_le_bytes(buf[8..16].try_into().unwrap());
            let count = u32::from_le_bytes(buf[16..20].try_into().unwrap()) as usize;
            let committed = u32::from_le_bytes(buf[20..24].try_into().unwrap());

            // Only replay committed entries with seq matching what superblock recorded
            if committed != 1 { continue; }
            if seq < expected_seq { break; } // seq == expected: replay to recover Phase 4 frees

            let entry_count = count.min(MAX_JOURNAL_ENTRIES);
            for j in 0..entry_count {
                let off = 24 + j * 16;
                let start = u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
                let cnt = u64::from_le_bytes(buf[off + 8..off + 16].try_into().unwrap());
                if start > 0 && cnt > 0 {
                    frees.push((start, cnt));
                }
            }
        }
        Ok(frees)
    }
}
