# `kernel/src/storage/npkfs/cache.rs` @ 5e0102684

## L1-4 · `use alloc::vec::Vec;`

```
//! LRU Block Cache with write coalescing
//!
//! Caches 4KB blocks in physical memory. Dirty blocks batched on flush.
//! SSD-friendly: minimizes write ops, groups flushes.
```

## L21 · `data_base: u64, // physical base of CACHE_SLOTS * BLOCK_SIZE contiguous region`

```
// physical base of CACHE_SLOTS * BLOCK_SIZE contiguous region
```

## L30 · `unsafe { core::ptr::write_bytes(data_base as *mut u8, 0, pages * 4096); }`

```
// SAFETY: Zeroing freshly allocated identity-mapped memory
```

## L50 · `if let Some(i) = self.meta.iter().position(|m| !m.valid) {`

```
// Find invalid slot first
```

## L54 · `let mut victim = 0;`

```
// LRU: find slot with lowest last_used
```

## L63 · `if self.meta[victim].dirty {`

```
// Write back if dirty
```

## L86 · `pub fn read(&mut self, block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {`

```
/// Read a block into buf. Uses cache, loads from disk on miss.
```

## L94 · `blkdev::read_block(block, buf)?;`

```
// Cache miss: load from disk
```

## L97 · `let slot = self.evict_slot()?;`

```
// Cache it
```

## L105 · `pub fn write(&mut self, block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), FsError> {`

```
/// Write a block (goes to cache, flushed later).
```

## L120-125 · `pub fn flush(&mut self) -> Result<(), FsError> {`

```
/// Flush all dirty blocks to disk in a single batched submission
/// (NVMe queue-depth N → 1 disk round-trip instead of N).
///
/// Re-enabled with diagnostic kprintlns in `nvme::write_blocks_batch`
/// — we're chasing a deadlock that triggered on the second testdisk
/// run; the trace will show the exact SQ/CQ state where it sticks.
```

## L132-134 · `let buf: &[u8; BLOCK_SIZE] = unsafe {`

```
// SAFETY: each slot is BLOCK_SIZE bytes at slot_ptr(i),
// alive for the lifetime of `self`. Batch call only
// reads from these slices.
```

## L153 · `pub fn invalidate(&mut self, block: u64) {`

```
/// Remove a block from cache (used after freeing blocks).
```

