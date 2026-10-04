# `kernel/src/storage/npkfs/types.rs` @ 5e0102684

## L1-5 · `pub const BLOCK_SIZE: usize = 4096;`

```
//! npkFS on-disk and in-memory types — shared scaffolding (block
//! geometry, journal layout, FsError) used by every other npkfs
//! submodule. Per-format constants (superblock magic, BTree node
//! magic + sizes) live in `format.rs` so they can move together
//! when the schema bumps.
```

## L9 · `#[repr(C, align(16))]`

```
/// Aligned 4KB buffer for safe casting to on-disk structs
```

## L20 · `pub const SUPERBLOCK_START: u64 = 1; // Block 0 reserved (MBR/GPT/UEFI)`

```
// Block 0 reserved (MBR/GPT/UEFI)
```

## L21 · `pub const JOURNAL_START: u64 = SUPERBLOCK_START + SUPERBLOCK_SLOTS; // 9`

```
// 9
```

## L23 · `pub const META_END: u64 = JOURNAL_START + JOURNAL_BLOCKS; // 265`

```
// 265
```

## L36 · `#[derive(Clone, Copy)]`

```
/// On-disk journal header (at JOURNAL_START)
```

## L44 · `pub committed: u32, // 1 = committed, 0 = in-progress`

```
// 1 = committed, 0 = in-progress
```

## L45 · `}`

```
// Followed by entry_count * JournalFreeEntry
```

## L56 · `pub const MAX_JOURNAL_ENTRIES: usize = (BLOCK_SIZE - 24) / 16; // 254`

```
// 254
```

