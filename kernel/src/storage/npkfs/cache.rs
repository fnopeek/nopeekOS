//! LRU Block Cache with write coalescing
//!
//! Caches 4KB blocks in physical memory. Dirty blocks batched on flush.
//! SSD-friendly: minimizes write ops, groups flushes.

use alloc::vec::Vec;
use crate::{blkdev, memory};
use super::types::{BLOCK_SIZE, FsError};

const CACHE_SLOTS: usize = 64;

struct CacheMeta {
    block: u64,
    dirty: bool,
    valid: bool,
    last_used: u64,
}

pub struct BlockCache {
    meta: Vec<CacheMeta>,
    data_base: u64, // physical base of CACHE_SLOTS * BLOCK_SIZE contiguous region
    counter: u64,
}

impl BlockCache {
    pub fn new() -> Result<Self, FsError> {
        let pages = (CACHE_SLOTS * BLOCK_SIZE) / 4096;
        let data_base = memory::allocate_contiguous(pages)
            .ok_or(FsError::Disk(crate::virtio_blk::BlkError::NotInitialized))?;
        // SAFETY: Zeroing freshly allocated identity-mapped memory
        unsafe { core::ptr::write_bytes(data_base as *mut u8, 0, pages * 4096); }

        let mut meta = Vec::with_capacity(CACHE_SLOTS);
        for _ in 0..CACHE_SLOTS {
            meta.push(CacheMeta { block: 0, dirty: false, valid: false, last_used: 0 });
        }

        Ok(BlockCache { meta, data_base, counter: 0 })
    }

    fn slot_ptr(&self, slot: usize) -> *mut u8 {
        (self.data_base + (slot * BLOCK_SIZE) as u64) as *mut u8
    }

    fn find_slot(&self, block: u64) -> Option<usize> {
        self.meta.iter().position(|m| m.valid && m.block == block)
    }

    fn evict_slot(&mut self) -> Result<usize, FsError> {
        // Find invalid slot first
        if let Some(i) = self.meta.iter().position(|m| !m.valid) {
            return Ok(i);
        }
        // LRU: find slot with lowest last_used
        let mut victim = 0;
        let mut oldest = u64::MAX;
        for (i, m) in self.meta.iter().enumerate() {
            if m.last_used < oldest {
                oldest = m.last_used;
                victim = i;
            }
        }
        // Write back if dirty
        if self.meta[victim].dirty {
            self.writeback(victim)?;
        }
        self.meta[victim].valid = false;
        Ok(victim)
    }

    fn writeback(&mut self, slot: usize) -> Result<(), FsError> {
        if !self.meta[slot].valid || !self.meta[slot].dirty { return Ok(()); }
        let block = self.meta[slot].block;
        let ptr = self.slot_ptr(slot);
        let buf = unsafe { &*(ptr as *const [u8; BLOCK_SIZE]) };
        blkdev::write_block(block, buf)?;
        self.meta[slot].dirty = false;
        Ok(())
    }

    fn touch(&mut self, slot: usize) {
        self.counter += 1;
        self.meta[slot].last_used = self.counter;
    }

    /// Read a block into buf. Uses cache, loads from disk on miss.
    pub fn read(&mut self, block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {
        if let Some(slot) = self.find_slot(block) {
            self.touch(slot);
            unsafe { core::ptr::copy_nonoverlapping(self.slot_ptr(slot), buf.as_mut_ptr(), BLOCK_SIZE); }
            return Ok(());
        }

        // Cache miss: load from disk
        blkdev::read_block(block, buf)?;

        // Cache it
        let slot = self.evict_slot()?;
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), self.slot_ptr(slot), BLOCK_SIZE); }
        self.meta[slot] = CacheMeta { block, dirty: false, valid: true, last_used: 0 };
        self.touch(slot);
        Ok(())
    }

    /// Write a block (goes to cache, flushed later).
    pub fn write(&mut self, block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), FsError> {
        let slot = if let Some(s) = self.find_slot(block) {
            s
        } else {
            let s = self.evict_slot()?;
            self.meta[s] = CacheMeta { block, dirty: false, valid: true, last_used: 0 };
            s
        };
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), self.slot_ptr(slot), BLOCK_SIZE); }
        self.meta[slot].dirty = true;
        self.touch(slot);
        Ok(())
    }

    /// Flush all dirty blocks to disk.
    ///
    /// Reverted to per-block sequential submission. The batched-NVMe
    /// path (`write_blocks_batch`) ships a regression we haven't
    /// located yet — second run of testdisk locked the system. The
    /// batch primitive stays available in `nvme::write_blocks_batch`
    /// for future debugging; cache.flush sits on the proven path.
    pub fn flush(&mut self) -> Result<(), FsError> {
        for i in 0..self.meta.len() {
            if self.meta[i].valid && self.meta[i].dirty {
                self.writeback(i)?;
            }
        }
        Ok(())
    }

    /// Remove a block from cache (used after freeing blocks).
    pub fn invalidate(&mut self, block: u64) {
        if let Some(slot) = self.find_slot(block) {
            self.meta[slot].valid = false;
            self.meta[slot].dirty = false;
        }
    }
}
