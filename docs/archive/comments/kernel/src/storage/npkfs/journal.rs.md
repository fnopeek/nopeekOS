# `kernel/src/storage/npkfs/journal.rs` @ 5e0102684

## L1-10 · `use alloc::vec::Vec;`

```
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
```

## L17 · `head: u64,  // next write block within journal area`

```
// next write block within journal area
```

## L20 · `uncommitted_blocks: Vec<u64>,`

```
// Track which journal blocks were written this commit (for post-commit mark)
```

## L32 · `pub fn record_free(&mut self, start: u64, count: u64) {`

```
/// Record blocks to be freed after next commit.
```

## L39-40 · `pub fn prepare(&mut self, cache: &mut BlockCache) -> Result<(), FsError> {`

```
/// Phase 1: Write pending frees to journal with committed=0.
/// Called BEFORE writing the new superblock.
```

## L47 · `let mut offset = 0;`

```
// Write ALL pending frees across multiple journal blocks if needed
```

## L54 · `buf[0..8].copy_from_slice(&JOURNAL_MAGIC);`

```
// Header
```

## L58 · `buf[20..24].copy_from_slice(&0u32.to_le_bytes()); // committed = 0 (NOT YET!)`

```
// committed = 0 (NOT YET!)
```

## L60 · `for i in 0..count {`

```
// Entries
```

## L80-81 · `pub fn finalize(&mut self, cache: &mut BlockCache) -> Result<(), FsError> {`

```
/// Phase 2: Mark journal entries as committed.
/// Called AFTER writing the new superblock.
```

## L86 · `buf[20..24].copy_from_slice(&1u32.to_le_bytes());`

```
// Set committed = 1
```

## L94-95 · `pub fn replay(cache: &mut BlockCache, head: u64, expected_seq: u64) -> Result<Vec<(u64, u64)>, FsError> {`

```
/// Check for committed journal entries that need replay (called during mount).
/// Scans backwards from head for entries with matching seq.
```

## L99-100 · `for i in 0..JOURNAL_BLOCKS {`

```
// Scan backwards from head, looking for committed entries with seq > expected
// (entries the superblock acknowledges but whose frees weren't executed yet)
```

## L118 · `if committed != 1 { continue; }`

```
// Only replay committed entries with seq matching what superblock recorded
```

## L120 · `if seq < expected_seq { break; } // seq == expected: replay to recover Phase 4 frees`

```
// seq == expected: replay to recover Phase 4 frees
```

