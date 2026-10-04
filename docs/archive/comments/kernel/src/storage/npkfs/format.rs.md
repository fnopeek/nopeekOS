# `kernel/src/storage/npkfs/format.rs` @ 5e0102684

## L1-15 · `#![allow(dead_code)]`

```
//! npkFS on-disk format (v3).
//!
//! Disk layout: block 0 reserved (MBR/GPT/UEFI), blocks 1–8 = SB slots,
//! blocks 9–264 = journal area, block 265+ = bitmap & data.
//!
//! v3 (this kernel) extends the v2 `TreeEntry` shape by adding an
//! `mtime` field (UTC seconds since the Unix epoch). Old v2 disks
//! cannot be read — the on-disk magic shifted from `npkFS\x02\0\0`
//! to `npkFS\x03\0\0` and the postcard wire shape of `Tree` payloads
//! changed. Mount-time guard halts with a reinstall message when v2
//! magic is detected.
//!
//! The B-tree node layout (block-level) is unchanged across v2 → v3
//! — only the typed Object payload (TreeEntry list) shifted shape. So
//! `BTREE_NODE_MAGIC` stays `"NPK2"` even at superblock version 3.
```

## L21 · `pub use super::types::{`

```
// ── Layout (block geometry, unchanged since v1) ───────────────────────
```

## L30 · `pub const DISK_MAGIC: [u8; 8] = *b"npkFS\x03\0\0";`

```
// ── Format identity ───────────────────────────────────────────────────
```

## L32-34 · `pub const DISK_MAGIC: [u8; 8] = *b"npkFS\x03\0\0";`

```
/// v3 superblock magic. Byte 5 carries the schema version (`0x01` v1,
/// `0x02` v2, `0x03` v3) so `dd | xxd` shows the generation directly
/// next to the ASCII tag.
```

## L37-38 · `pub const DISK_MAGIC_V2: [u8; 8] = *b"npkFS\x02\0\0";`

```
/// v2 superblock magic. Kept here so the mount-time guard can detect
/// pre-mtime disks and surface a clear reinstall message.
```

## L41 · `pub const DISK_VERSION: u32 = 3;`

```
/// On-disk format version field of the superblock.
```

## L44-48 · `pub const BTREE_NODE_MAGIC: u32 = 0x324B504E;`

```
/// B-tree node magic. ASCII "NPK2" little-endian. Unchanged across
/// v2 → v3: the block-level node layout (header + leaf entries keyed
/// by 32-byte hash, internal entries with 32-byte key + 8-byte child
/// pointer) didn't shift; only the typed Object payload above the
/// storage layer changed shape.
```

## L54 · `pub const DIRECT_EXTENTS: usize = 3;`

```
// ── Per-leaf entry ────────────────────────────────────────────────────
```

## L56-57 · `pub const DIRECT_EXTENTS: usize = 3;`

```
/// Direct extents stored inline in a leaf entry. More extents go through
/// the indirect chain (same format as v1).
```

## L60 · `pub const EXTENTS_PER_INDIRECT: usize = 255;`

```
/// Extents per indirect block (see v1's identical layout).
```

## L63-68 · `#[derive(Clone, Copy)]`

```
/// B-tree leaf entry. Keyed by `hash` (the BLAKE3 of the plaintext
/// payload — same value the caller passed to `put`). Stores location +
/// sizes of the on-disk (encrypted) bytes.
///
/// Layout chosen so 36 entries fit in a 4 KB leaf (after 16 B header +
/// 32 B checksum).
```

## L72 · `pub hash: [u8; 32],`

```
/// Primary key. Equals BLAKE3(plaintext). Verified on read.
```

## L74 · `pub plaintext_size: u64,`

```
/// Caller's payload size (decrypted).
```

## L76-78 · `pub disk_size: u64,`

```
/// Bytes actually stored across `extents` + indirect chain. Equals
/// `plaintext_size` if the FS was formatted without a master key,
/// `plaintext_size + 16` if the AEAD tag is appended.
```

## L80 · `pub extent_count: u32,`

```
/// Total extents (direct + indirect).
```

## L83 · `pub extents: [Extent; DIRECT_EXTENTS],`

```
/// First DIRECT_EXTENTS extents stored inline.
```

## L85 · `pub indirect_block: u64,`

```
/// Address of the first indirect block (0 = none).
```

## L92 · `pub const INTERNAL_ENTRY_SIZE: usize = 40;`

```
// ── Per-internal entry ────────────────────────────────────────────────
```

## L94 · `pub const INTERNAL_ENTRY_SIZE: usize = 40;`

```
/// Internal node entry: 32-byte key + 8-byte child pointer.
```

## L97 · `pub const NODE_HEADER_SIZE: usize = 16;`

```
// ── Node capacities ───────────────────────────────────────────────────
```

## L100 · `pub const CHECKSUM_SIZE: usize = 32;`

```
/// 32-byte checksum trailer, BLAKE3 over the rest of the block.
```

## L103-109 · `pub const MAX_INTERNAL_KEYS: usize =`

```
// Reserve the 32-byte BLAKE3 checksum trailer (at BLOCK_SIZE-32), same as
// MAX_LEAF_ENTRIES does. Without it the constant was 102, but entry 101 sits at
// offset 16+101*40 = 4056..4096 and overlaps the checksum at 4064 — writing the
// checksum then clobbers that entry's child pointer with hash bytes (a
// high-entropy garbage block number → OutOfRange on the next descent), and it
// slips past read_node's checksum verify because the clobbered bytes ARE the
// checksum. 101 keeps the last entry below 4064.
```

## L111 · `(BLOCK_SIZE - NODE_HEADER_SIZE - CHECKSUM_SIZE) / INTERNAL_ENTRY_SIZE; // 101`

```
// 101
```

## L113 · `(BLOCK_SIZE - NODE_HEADER_SIZE - CHECKSUM_SIZE) / LEAF_ENTRY_SIZE; // 36`

```
// 36
```

## L115 · `#[derive(Clone, Copy)]`

```
// ── Node header (same shape as v1 but distinct magic) ─────────────────
```

## L124 · `pub right_child: u64,`

```
/// Internal nodes: rightmost child block. Leaf nodes: reserved (0).
```

## L128 · `#[derive(Clone, Copy)]`

```
// ── Superblock ────────────────────────────────────────────────────────
```

## L130-131 · `#[derive(Clone, Copy)]`

```
/// 4096-byte superblock. Adds `root_tree_hash` for Step 4 (path
/// walker / mutations) — populated 0 in Step 2 since no path layer exists.
```

## L144 · `pub btree_root: u64,`

```
/// B-tree root block address (Step 2 entry point: hash → BTreeEntryRaw).
```

## L146 · `pub root_tree_hash: [u8; 32],`

```
/// Hash of the root Tree object (Step 4+). Zero in Step 2.
```

