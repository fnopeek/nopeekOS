# `kernel/src/storage/npkfs/btree.rs` @ 5e0102684

## L1-8 · `use alloc::vec::Vec;`

```
//! Copy-on-Write B-tree, keyed by 32-byte content hashes.
//!
//! Forked from v1's btree.rs (variable-length 64-byte names) with the
//! key shape changed to fixed 32-byte hashes and the leaf-entry shape
//! to `BTreeEntryRaw`. COW + fixup-path + node-split + node-checksum logic
//! carries over verbatim, which is deliberate — those are the parts
//! with hard-won correctness fixes (see v1 commit history) and porting
//! them by hand would re-open old bugs.
```

## L26 · `fn compute_node_checksum(buf: &[u8; BLOCK_SIZE]) -> [u8; 32] {`

```
// ── Node-level checksum (BLAKE3 over block - last 32 B) ───────────────
```

## L43 · `fn read_header(buf: &[u8; BLOCK_SIZE]) -> BTreeNodeHeader {`

```
// ── Header helpers ────────────────────────────────────────────────────
```

## L63 · `fn internal_key(buf: &[u8; BLOCK_SIZE], idx: usize) -> &[u8] {`

```
// ── Internal node entry: [key:[u8;32], child:u64] ─────────────────────
```

## L81 · `fn leaf_entry(buf: &[u8; BLOCK_SIZE], idx: usize) -> BTreeEntryRaw {`

```
// ── Leaf node entry: BTreeEntryRaw (repr(C), 112 B) ──────────────────────
```

## L86-88 · `unsafe { core::ptr::read_unaligned(src.as_ptr() as *const BTreeEntryRaw) }`

```
// SAFETY: BTreeEntryRaw is repr(C) and exactly LEAF_ENTRY_SIZE bytes
// (asserted at compile time). read_unaligned tolerates the leaf-grid
// alignment.
```

## L100 · `fn read_node(cache: &mut BlockCache, block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {`

```
// ── Disk I/O wrappers (with checksum verify on read, write on write) ──
```

## L126 · `pub fn lookup(`

```
// ══ Public API ════════════════════════════════════════════════════════
```

## L128 · `pub fn lookup(`

```
/// Lookup an object by hash. Returns None if the hash is not in the tree.
```

## L157-161 · `pub fn insert(`

```
/// Insert an entry. Returns `(new_root, old_blocks_to_free, was_new)`.
///
/// `was_new == false` means the hash was already present and the call
/// was a no-op (content-addressed dedup). In that case `new_root` equals
/// the input `root` and `old_blocks_to_free` is empty.
```

## L169 · `let new_block = bitmap.alloc(1)?;`

```
// Empty tree → fresh leaf with this one entry.
```

## L192-194 · `for i in 0..hdr.num_entries as usize {`

```
// Idempotent: same hash already present is a no-op. Caller
// either had this object before us or just put the same
// bytes — either way the on-disk state is already correct.
```

## L236 · `pub fn delete(`

```
/// Delete an entry by hash. Returns `(new_root, old_blocks_to_free)`.
```

## L289-292 · `#[allow(dead_code)]`

```
/// Walk every entry in the tree in key order, calling `f` for each.
/// Used by GC + diagnostic intents (Steps 5 + 9). Reachable from no
/// caller in Step 2 yet — kept here so Step 3+ doesn't need to revisit
/// the btree module.
```

## L301-305 · `pub fn collect_for_fsck(`

```
/// fsck enumeration: collect every B-tree node block id (root + every child
/// pointer — including ones that turn out unreadable, so a node overwritten by
/// data is still counted and surfaces as a double-alloc) into `nodes`, and
/// every leaf entry into `entries`. Corruption-tolerant like `iter_all`: a bad
/// node is skipped, never fatal. Read-only.
```

## L348-354 · `#[allow(dead_code)]`

```
/// **Best-effort** traversal used only by `iter_all` (GC enumeration):
/// a node that is out-of-range, unreadable, has a bad magic / type, or
/// a corrupt `num_entries` is logged and skipped rather than aborting
/// the whole sweep — one bad pointer must not wedge GC forever. Depth-
/// guarded against corrupt cycles, and `num_entries` is clamped to the
/// node capacity so a garbage count can't index past the 4 KiB buffer
/// (which would panic). `lookup`/`insert`/`delete` stay strict.
```

## L403 · `fn find_child_block(buf: &[u8; BLOCK_SIZE], hdr: &BTreeNodeHeader, key: &[u8; 32]) -> u64 {`

```
// ══ Internal helpers ══════════════════════════════════════════════════
```

## L502 · `fn fixup_path(`

```
/// After a COW on a child, propagate the new pointer up the path.
```

## L529 · `new_buf[8..16].copy_from_slice(&child_block.to_le_bytes());`

```
// Rightmost child sits in the header field.
```

## L541-542 · `fn fixup_path_split(`

```
/// After a leaf or internal split, propagate `(split_key, right_child)`
/// up the path, splitting parents as needed.
```

## L550 · `let root_block = bitmap.alloc(1)?;`

```
// Tree grows: new internal root with one key.
```

## L576 · `let mut new_hdr = hdr;`

```
// New key at end: right_child slot moves to `right`.
```

## L583 · `for i in (pos..n).rev() {`

```
// Shift entries [pos..n) right by one slot.
```

## L590-591 · `let off = NODE_HEADER_SIZE + (pos + 1) * INTERNAL_ENTRY_SIZE + 32;`

```
// The entry that was at `pos` (now at `pos+1`) keeps its key,
// but its child pointer must become `right` — same fix as v1.
```

## L697 · `let remaining = if child_idx == 0 {`

```
// Root with one key shrinks: promote the surviving child.
```

## L711-713 · `if child_idx < n {`

```
// Same fix as v1: removing the rightmost child means promoting the
// child at slot n-1 into the right_child header field. Forgetting
// this leaves right_child dangling on a freed leaf.
```

