//! Block Allocation Bitmap
//!
//! In-memory bitmap for fast alloc/free. Flushed to disk on sync.
//! Batched TRIM/DISCARD for SSD friendliness.

use alloc::vec::Vec;
use super::types::*;
use super::cache::BlockCache;

pub struct Bitmap {
    data: Vec<u8>,
    bitmap_start: u64,
    bitmap_count: u64,
    data_start: u64,
    total_blocks: u64,
    free_count: u64,
    dirty: bool,
    trim_pending: Vec<(u64, u64)>,
    alloc_cursor: u64, // Next-fit: start searching here
}

impl Bitmap {
    /// Load bitmap from disk
    pub fn load(cache: &mut BlockCache, sb: &SuperblockRaw) -> Result<Self, FsError> {
        let byte_count = (sb.total_blocks as usize + 7) / 8;
        let mut data = alloc::vec![0u8; byte_count];

        for i in 0..sb.bitmap_count {
            let mut buf = [0u8; BLOCK_SIZE];
            cache.read(sb.bitmap_start + i, &mut buf)?;
            let offset = i as usize * BLOCK_SIZE;
            let copy_len = BLOCK_SIZE.min(byte_count - offset);
            data[offset..offset + copy_len].copy_from_slice(&buf[..copy_len]);
        }

        let free_count = count_free(&data, sb.total_blocks);

        Ok(Bitmap {
            data,
            bitmap_start: sb.bitmap_start,
            bitmap_count: sb.bitmap_count,
            data_start: sb.data_start,
            total_blocks: sb.total_blocks,
            free_count,
            dirty: false,
            trim_pending: Vec::new(),
            alloc_cursor: sb.data_start,
        })
    }

    /// Create fresh bitmap for mkfs. Marks metadata blocks as used.
    pub fn new_for_mkfs(total_blocks: u64, bitmap_start: u64, bitmap_count: u64, data_start: u64) -> Self {
        let byte_count = (total_blocks as usize + 7) / 8;
        let mut data = alloc::vec![0u8; byte_count];

        // Mark all blocks before data_start as used (superblock, journal, bitmap)
        for b in 0..data_start {
            set_used(&mut data, b);
        }

        let free_count = total_blocks.saturating_sub(data_start);

        Bitmap {
            data, bitmap_start, bitmap_count, data_start,
            total_blocks, free_count, dirty: true,
            trim_pending: Vec::new(),
            alloc_cursor: data_start,
        }
    }

    pub fn free_count(&self) -> u64 { self.free_count }

    /// Allocate `count` contiguous blocks. Returns start block.
    /// Uses next-fit: starts searching from last allocation point (amortized O(1)).
    pub fn alloc(&mut self, count: u64) -> Result<u64, FsError> {
        if count == 0 || count > self.free_count { return Err(FsError::DiskFull); }

        // Search from cursor to end, then wrap around to data_start
        if let Some(block) = self.find_run(self.alloc_cursor, self.total_blocks, count) {
            return Ok(block);
        }
        // Wrap around
        if self.alloc_cursor > self.data_start {
            if let Some(block) = self.find_run(self.data_start, self.alloc_cursor, count) {
                return Ok(block);
            }
        }
        Err(FsError::DiskFull)
    }

    fn find_run(&mut self, from: u64, to: u64, count: u64) -> Option<u64> {
        let mut run_start = from;
        let mut run_len = 0u64;

        for b in from..to {
            if is_free(&self.data, b) {
                if run_len == 0 { run_start = b; }
                run_len += 1;
                if run_len == count {
                    for i in 0..count {
                        set_used(&mut self.data, run_start + i);
                    }
                    self.free_count -= count;
                    self.dirty = true;
                    self.alloc_cursor = run_start + count;
                    if self.alloc_cursor >= self.total_blocks {
                        self.alloc_cursor = self.data_start;
                    }
                    return Some(run_start);
                }
            } else {
                run_len = 0;
            }
        }
        None
    }

    /// Free a range of blocks. Queues TRIM for later.
    pub fn free(&mut self, start: u64, count: u64) {
        for i in 0..count {
            let b = start + i;
            if b < self.total_blocks && !is_free(&self.data, b) {
                set_free(&mut self.data, b);
                self.free_count += 1;
            }
        }
        if count > 0 {
            self.trim_pending.push((start, count));
            self.dirty = true;
        }
    }

    /// Write bitmap to disk via cache.
    pub fn sync(&self, cache: &mut BlockCache) -> Result<(), FsError> {
        if !self.dirty { return Ok(()); }
        for i in 0..self.bitmap_count {
            let mut buf = [0u8; BLOCK_SIZE];
            let offset = i as usize * BLOCK_SIZE;
            let copy_len = BLOCK_SIZE.min(self.data.len() - offset);
            buf[..copy_len].copy_from_slice(&self.data[offset..offset + copy_len]);
            cache.write(self.bitmap_start + i, &buf)?;
        }
        Ok(())
    }

    /// Issue batched TRIM for all freed blocks, then clear pending list.
    pub fn flush_trims(&mut self) {
        // Merge adjacent ranges
        self.trim_pending.sort_by_key(|&(s, _)| s);
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for &(start, count) in &self.trim_pending {
            if let Some(last) = merged.last_mut() {
                if last.0 + last.1 == start {
                    last.1 += count;
                    continue;
                }
            }
            merged.push((start, count));
        }
        for (start, count) in &merged {
            if let Err(e) = crate::blkdev::discard_blocks(*start, *count) {
                crate::kprintln!("[npk] TRIM failed at block {}: {}", start, e);
            }
        }
        self.trim_pending.clear();
    }

    #[allow(dead_code)]
    pub fn mark_dirty(&mut self) { self.dirty = true; }
}

fn is_free(data: &[u8], block: u64) -> bool {
    let b = block as usize;
    data[b / 8] & (1 << (b % 8)) == 0
}

fn set_used(data: &mut [u8], block: u64) {
    let b = block as usize;
    data[b / 8] |= 1 << (b % 8);
}

fn set_free(data: &mut [u8], block: u64) {
    let b = block as usize;
    data[b / 8] &= !(1 << (b % 8));
}

fn count_free(data: &[u8], total: u64) -> u64 {
    let mut free = 0u64;
    for b in 0..total {
        if is_free(data, b) { free += 1; }
    }
    free
}
