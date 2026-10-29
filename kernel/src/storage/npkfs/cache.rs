//! Block cache for the store's metadata (B-tree nodes, bitmap, journal,
//! indirect extent chains), with write coalescing.
//!
//! As Linux's page cache it holds as much as free memory allows: a miss
//! takes a fresh frame while the free frames stay above the high
//! watermark, else it reuses the least recently used block. When an
//! allocation elsewhere finds no memory, the shrinker gives clean blocks
//! back from the cold end (`shrink`). Lookup is hashed and the LRU order
//! is a linked list, so every operation is O(1) at any size.

use alloc::vec::Vec;
use hashbrown::HashMap;
use crate::{blkdev, memory};
use super::types::{BLOCK_SIZE, FsError};

const NIL: u32 = u32::MAX;

struct Slot {
    block: u64,
    /// The frame holding the block's bytes (identity mapped).
    frame: u64,
    dirty: bool,
    /// Neighbours in the LRU list: `prev` towards the most recent end.
    prev: u32,
    next: u32,
}

pub struct BlockCache {
    slots: Vec<Slot>,
    map: HashMap<u64, u32>,
    /// Most and least recently used slot.
    head: u32,
    tail: u32,
    /// Slots that hold a frame but no block.
    idle: Vec<u32>,
    /// Slots that hold neither: their frame went back to memory.
    spare: Vec<u32>,
    /// Slots written since the last flush.
    dirty: Vec<u32>,
}

/// Free frames below which the cache stops growing: Linux's high
/// watermark, one and a half times `min_free_kbytes`.
fn high_watermark() -> usize {
    let min = crate::heap::min_free_frames();
    min + min / 2
}

impl BlockCache {
    pub fn new() -> Result<Self, FsError> {
        Ok(BlockCache {
            slots: Vec::new(),
            map: HashMap::new(),
            head: NIL,
            tail: NIL,
            idle: Vec::new(),
            spare: Vec::new(),
            dirty: Vec::new(),
        })
    }

    fn bytes(&self, slot: u32) -> &[u8; BLOCK_SIZE] {
        // SAFETY: a slot in use owns its frame, a whole identity-mapped page
        // of BLOCK_SIZE bytes that nothing else references; `&self` keeps it
        // from being written meanwhile.
        unsafe { &*(self.slots[slot as usize].frame as *const [u8; BLOCK_SIZE]) }
    }

    fn bytes_mut(&mut self, slot: u32) -> &mut [u8; BLOCK_SIZE] {
        // SAFETY: as `bytes`, with `&mut self` for exclusive access.
        unsafe { &mut *(self.slots[slot as usize].frame as *mut [u8; BLOCK_SIZE]) }
    }

    fn unlink(&mut self, s: u32) {
        let (prev, next) = (self.slots[s as usize].prev, self.slots[s as usize].next);
        if prev == NIL { self.head = next; } else { self.slots[prev as usize].next = next; }
        if next == NIL { self.tail = prev; } else { self.slots[next as usize].prev = prev; }
    }

    fn push_front(&mut self, s: u32) {
        self.slots[s as usize].prev = NIL;
        self.slots[s as usize].next = self.head;
        if self.head != NIL { self.slots[self.head as usize].prev = s; }
        self.head = s;
        if self.tail == NIL { self.tail = s; }
    }

    fn touch(&mut self, s: u32) {
        if self.head != s {
            self.unlink(s);
            self.push_front(s);
        }
    }

    /// A slot with a frame for `block`, at the front: an idle one, a new
    /// frame while memory is plentiful, else the least recently used block,
    /// written back first if dirty.
    fn take(&mut self, block: u64) -> Result<u32, FsError> {
        let s = if let Some(s) = self.idle.pop() {
            s
        } else if let Some(frame) = (memory::stats().0 > high_watermark())
            .then(memory::allocate_frame).flatten()
        {
            match self.spare.pop() {
                Some(s) => { self.slots[s as usize].frame = frame; s }
                None => {
                    self.slots.push(Slot { block: 0, frame, dirty: false, prev: NIL, next: NIL });
                    // Room for every slot in `spare`, so `shrink` never
                    // allocates.
                    let room = self.slots.len() - self.spare.len();
                    self.spare.reserve(room);
                    (self.slots.len() - 1) as u32
                }
            }
        } else if self.tail != NIL {
            let s = self.tail;
            self.writeback(s)?;
            self.unlink(s);
            self.map.remove(&self.slots[s as usize].block);
            s
        } else {
            return Err(FsError::OutOfMemory);
        };
        self.slots[s as usize].block = block;
        self.slots[s as usize].dirty = false;
        self.map.insert(block, s);
        self.push_front(s);
        Ok(s)
    }

    fn writeback(&mut self, s: u32) -> Result<(), FsError> {
        if !self.slots[s as usize].dirty { return Ok(()); }
        blkdev::write_block(self.slots[s as usize].block, self.bytes(s))?;
        self.slots[s as usize].dirty = false;
        Ok(())
    }

    /// Read a block into buf. Uses cache, loads from disk on miss.
    pub fn read(&mut self, block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {
        use core::sync::atomic::Ordering::Relaxed;
        if let Some(&s) = self.map.get(&block) {
            self.touch(s);
            buf.copy_from_slice(self.bytes(s));
            super::bench::CACHE_HITS.fetch_add(1, Relaxed);
            return Ok(());
        }
        super::bench::CACHE_MISSES.fetch_add(1, Relaxed);
        blkdev::read_block(block, buf)?;
        let s = self.take(block)?;
        self.bytes_mut(s).copy_from_slice(buf);
        Ok(())
    }

    /// Write a block (goes to cache, flushed later).
    pub fn write(&mut self, block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), FsError> {
        let s = match self.map.get(&block) {
            Some(&s) => { self.touch(s); s }
            None => self.take(block)?,
        };
        self.bytes_mut(s).copy_from_slice(buf);
        if !self.slots[s as usize].dirty {
            self.slots[s as usize].dirty = true;
            self.dirty.push(s);
        }
        Ok(())
    }

    /// Flush all dirty blocks to disk in a single batched submission
    /// (NVMe queue-depth N → 1 disk round-trip instead of N).
    pub fn flush(&mut self) -> Result<(), FsError> {
        // A slot evicted or invalidated since it was listed is clean now.
        let mut list: Vec<u32> = self.dirty.iter().copied()
            .filter(|&s| self.slots[s as usize].dirty)
            .collect();
        // A slot dirtied, evicted and dirtied again is listed twice.
        list.sort_unstable();
        list.dedup();
        if list.is_empty() {
            self.dirty.clear();
            return Ok(());
        }
        let batch: Vec<(u64, &[u8; BLOCK_SIZE])> = list.iter()
            .map(|&s| (self.slots[s as usize].block, self.bytes(s)))
            .collect();
        blkdev::write_blocks_batch(&batch)?;
        for &s in &list {
            self.slots[s as usize].dirty = false;
        }
        self.dirty.clear();
        Ok(())
    }

    /// Write back every dirty block, then forget all of them and give
    /// their frames back.
    pub fn drop_all(&mut self) -> Result<(), FsError> {
        self.flush()?;
        while self.tail != NIL {
            let s = self.tail;
            self.unlink(s);
            self.map.remove(&self.slots[s as usize].block);
            self.idle.push(s);
        }
        self.release_idle(usize::MAX);
        Ok(())
    }

    /// Remove a block from cache (used after freeing blocks).
    pub fn invalidate(&mut self, block: u64) {
        if let Some(s) = self.map.remove(&block) {
            self.unlink(s);
            self.slots[s as usize].dirty = false;
            self.idle.push(s);
        }
    }

    /// Give up to `want` frames back to memory, idle ones first, then clean
    /// blocks from the cold end; returns how many. Allocates nothing.
    pub fn shrink(&mut self, want: usize) -> usize {
        let mut freed = self.release_idle(want);
        let mut s = self.tail;
        while freed < want && s != NIL {
            let prev = self.slots[s as usize].prev;
            if !self.slots[s as usize].dirty {
                self.unlink(s);
                self.map.remove(&self.slots[s as usize].block);
                memory::deallocate_frame(self.slots[s as usize].frame);
                self.slots[s as usize].frame = 0;
                self.spare.push(s);
                freed += 1;
            }
            s = prev;
        }
        freed
    }

    /// Return up to `want` idle frames to memory.
    fn release_idle(&mut self, want: usize) -> usize {
        let mut freed = 0;
        while freed < want {
            let Some(s) = self.idle.pop() else { break };
            memory::deallocate_frame(self.slots[s as usize].frame);
            self.slots[s as usize].frame = 0;
            self.spare.push(s);
            freed += 1;
        }
        freed
    }

    /// Blocks held, and how many of them are dirty.
    pub fn held(&self) -> (usize, usize) {
        (self.map.len(), self.slots.iter().filter(|s| s.frame != 0 && s.dirty).count())
    }
}

impl Drop for BlockCache {
    /// Frames back to memory; dirty blocks are dropped unwritten, as with
    /// `unmount`.
    fn drop(&mut self) {
        for slot in &self.slots {
            if slot.frame != 0 {
                memory::deallocate_frame(slot.frame);
            }
        }
    }
}
