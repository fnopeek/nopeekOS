# `kernel/src/storage/npkfs/bitmap.rs` @ 5e0102684

## L1-4 · `use alloc::vec::Vec;`

```
//! Block Allocation Bitmap
//!
//! In-memory bitmap for fast alloc/free. Flushed to disk on sync.
//! Batched TRIM/DISCARD for SSD friendliness.
```

## L18-21 · `dirty_blocks: Vec<bool>,`

```
/// Per-bitmap-block dirty flag. `sync()` only writes blocks marked
/// dirty here, instead of pushing all `bitmap_count` blocks
/// through the 64-slot block cache (which used to thrash every
/// commit, capping write IOPS at ~17 on a multi-GB partition).
```

## L24 · `alloc_cursor: u64, // Next-fit: start searching here`

```
// Next-fit: start searching here
```

## L28-29 · `pub fn load_args(`

```
/// Load bitmap from disk. the superblock passes layout primitives
/// directly; bitmap layout is identical between v1 and v2.
```

## L64 · `pub fn new_for_mkfs(total_blocks: u64, bitmap_start: u64, bitmap_count: u64, data_start: u64) -> Self {`

```
/// Create fresh bitmap for mkfs. Marks metadata blocks as used.
```

## L69 · `for b in 0..data_start {`

```
// Mark all blocks before data_start as used (superblock, journal, bitmap)
```

## L76 · `Bitmap {`

```
// Fresh format: every bitmap block is dirty (needs to land on disk).
```

## L90-93 · `pub fn is_allocated(&self, block: u64) -> bool {`

```
/// True iff `block` is marked allocated. Out-of-range blocks report
/// `false` (they can't be validly allocated). Used by the fsck self-check
/// to flag blocks the tree references but the allocator believes are free
/// (→ they'd be handed out again = a future double-alloc).
```

## L99-100 · `pub fn alloc(&mut self, count: u64) -> Result<u64, FsError> {`

```
/// Allocate `count` contiguous blocks. Returns start block.
/// Uses next-fit: starts searching from last allocation point (amortized O(1)).
```

## L104 · `if let Some(block) = self.find_run(self.alloc_cursor, self.total_blocks, count) {`

```
// Search from cursor to end, then wrap around to data_start
```

## L108 · `if self.alloc_cursor > self.data_start {`

```
// Wrap around
```

## L146 · `pub fn free(&mut self, start: u64, count: u64) {`

```
/// Free a range of blocks. Queues TRIM for later.
```

## L162-166 · `fn mark_block_dirty(&mut self, block: u64) {`

```
/// Mark the bitmap-block that owns `block`'s allocation bit as
/// needing flush. With ~32K bits per 4 KB bitmap-block, a typical
/// fs::write (5-10 newly allocated blocks clustered together) only
/// dirties 1-2 bitmap-blocks — vs. the previous "rewrite all
/// `bitmap_count` blocks every commit" which thrashed the cache.
```

## L175-178 · `pub fn sync(&mut self, cache: &mut BlockCache) -> Result<(), FsError> {`

```
/// Write bitmap blocks that have changed since the last sync to
/// the cache. `dirty_blocks` tracks per-block changes; only those
/// land in the cache, keeping the 64-slot LRU available for the
/// payload data + B-tree nodes that actually benefit from caching.
```

## L198 · `pub fn flush_trims(&mut self) {`

```
/// Issue batched TRIM for all freed blocks, then clear pending list.
```

## L200 · `self.trim_pending.sort_by_key(|&(s, _)| s);`

```
// Merge adjacent ranges
```

