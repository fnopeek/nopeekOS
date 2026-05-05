//! v2 Copy-on-Write B-tree, keyed by 32-byte content hashes.
//!
//! Forked from v1's btree.rs (variable-length 64-byte names) with the
//! key shape changed to fixed 32-byte hashes and the leaf-entry shape
//! to `V2EntryRaw`. COW + fixup-path + node-split + node-checksum logic
//! carries over verbatim, which is deliberate — those are the parts
//! with hard-won correctness fixes (see v1 commit history) and porting
//! them by hand would re-open old bugs.

use alloc::vec::Vec;

use super::bitmap::Bitmap;
use super::cache::BlockCache;
use super::types::{FsError, BLOCK_SIZE};
use super::format::{
    V2EntryRaw, V2NodeHeader,
    V2_BTREE_INTERNAL, V2_BTREE_LEAF, V2_BTREE_MAGIC,
    V2_INTERNAL_ENTRY_SIZE, V2_LEAF_ENTRY_SIZE,
    V2_MAX_INTERNAL_KEYS, V2_MAX_LEAF_ENTRIES,
    V2_NODE_HEADER_SIZE,
};

const MAX_TREE_DEPTH: usize = 8;
const CHECKSUM_OFFSET: usize = BLOCK_SIZE - 32;

// ── Node-level checksum (BLAKE3 over block - last 32 B) ───────────────

fn compute_node_checksum(buf: &[u8; BLOCK_SIZE]) -> [u8; 32] {
    *blake3::hash(&buf[..CHECKSUM_OFFSET]).as_bytes()
}

fn write_node_checksum(buf: &mut [u8; BLOCK_SIZE]) {
    let cs = compute_node_checksum(buf);
    buf[CHECKSUM_OFFSET..].copy_from_slice(&cs);
}

fn verify_node_checksum(buf: &[u8; BLOCK_SIZE]) -> bool {
    let stored = &buf[CHECKSUM_OFFSET..];
    let expected = compute_node_checksum(buf);
    stored == expected
}

// ── Header helpers ────────────────────────────────────────────────────

fn read_header(buf: &[u8; BLOCK_SIZE]) -> V2NodeHeader {
    V2NodeHeader {
        magic:       u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        node_type:   buf[4],
        _pad:        buf[5],
        num_entries: u16::from_le_bytes(buf[6..8].try_into().unwrap()),
        right_child: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
    }
}

fn write_header(buf: &mut [u8; BLOCK_SIZE], hdr: &V2NodeHeader) {
    buf[0..4].copy_from_slice(&hdr.magic.to_le_bytes());
    buf[4] = hdr.node_type;
    buf[5] = hdr._pad;
    buf[6..8].copy_from_slice(&hdr.num_entries.to_le_bytes());
    buf[8..16].copy_from_slice(&hdr.right_child.to_le_bytes());
}

// ── Internal node entry: [key:[u8;32], child:u64] ─────────────────────

fn internal_key(buf: &[u8; BLOCK_SIZE], idx: usize) -> &[u8] {
    let off = V2_NODE_HEADER_SIZE + idx * V2_INTERNAL_ENTRY_SIZE;
    &buf[off..off + 32]
}

fn internal_child(buf: &[u8; BLOCK_SIZE], idx: usize) -> u64 {
    let off = V2_NODE_HEADER_SIZE + idx * V2_INTERNAL_ENTRY_SIZE + 32;
    u64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

fn set_internal_entry(buf: &mut [u8; BLOCK_SIZE], idx: usize, key: &[u8; 32], child: u64) {
    let off = V2_NODE_HEADER_SIZE + idx * V2_INTERNAL_ENTRY_SIZE;
    buf[off..off + 32].copy_from_slice(key);
    buf[off + 32..off + 40].copy_from_slice(&child.to_le_bytes());
}

// ── Leaf node entry: V2EntryRaw (repr(C), 112 B) ──────────────────────

fn leaf_entry(buf: &[u8; BLOCK_SIZE], idx: usize) -> V2EntryRaw {
    let off = V2_NODE_HEADER_SIZE + idx * V2_LEAF_ENTRY_SIZE;
    let src = &buf[off..off + V2_LEAF_ENTRY_SIZE];
    // SAFETY: V2EntryRaw is repr(C) and exactly V2_LEAF_ENTRY_SIZE bytes
    // (asserted at compile time). read_unaligned tolerates the leaf-grid
    // alignment.
    unsafe { core::ptr::read_unaligned(src.as_ptr() as *const V2EntryRaw) }
}

fn set_leaf_entry(buf: &mut [u8; BLOCK_SIZE], idx: usize, entry: &V2EntryRaw) {
    let off = V2_NODE_HEADER_SIZE + idx * V2_LEAF_ENTRY_SIZE;
    let src = unsafe {
        core::slice::from_raw_parts(entry as *const V2EntryRaw as *const u8, V2_LEAF_ENTRY_SIZE)
    };
    buf[off..off + V2_LEAF_ENTRY_SIZE].copy_from_slice(src);
}

// ── Disk I/O wrappers (with checksum verify on read, write on write) ──

fn read_node(cache: &mut BlockCache, block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {
    cache.read(block, buf)?;
    if !verify_node_checksum(buf) {
        return Err(FsError::Corrupt);
    }
    Ok(())
}

fn write_node(cache: &mut BlockCache, block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {
    write_node_checksum(buf);
    cache.write(block, buf)?;
    Ok(())
}

fn make_empty_leaf() -> [u8; BLOCK_SIZE] {
    let mut buf = [0u8; BLOCK_SIZE];
    let hdr = V2NodeHeader {
        magic: V2_BTREE_MAGIC, node_type: V2_BTREE_LEAF,
        _pad: 0, num_entries: 0, right_child: 0,
    };
    write_header(&mut buf, &hdr);
    buf
}

// ══ Public API ════════════════════════════════════════════════════════

/// Lookup an object by hash. Returns None if the hash is not in the tree.
pub fn lookup(
    cache: &mut BlockCache, root: u64, key: &[u8; 32],
) -> Result<Option<V2EntryRaw>, FsError> {
    if root == 0 { return Ok(None); }
    let mut block = root;

    loop {
        let mut buf = [0u8; BLOCK_SIZE];
        read_node(cache, block, &mut buf)?;
        let hdr = read_header(&buf);
        if hdr.magic != V2_BTREE_MAGIC { return Err(FsError::Corrupt); }

        match hdr.node_type {
            V2_BTREE_LEAF => {
                for i in 0..hdr.num_entries as usize {
                    let e = leaf_entry(&buf, i);
                    if e.hash == *key { return Ok(Some(e)); }
                }
                return Ok(None);
            }
            V2_BTREE_INTERNAL => {
                block = find_child_block(&buf, &hdr, key);
            }
            _ => return Err(FsError::Corrupt),
        }
    }
}

/// Insert an entry. Returns `(new_root, old_blocks_to_free, was_new)`.
///
/// `was_new == false` means the hash was already present and the call
/// was a no-op (content-addressed dedup). In that case `new_root` equals
/// the input `root` and `old_blocks_to_free` is empty.
pub fn insert(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    root: u64, entry: &V2EntryRaw,
) -> Result<(u64, Vec<u64>, bool), FsError> {
    let mut old_blocks = Vec::new();

    if root == 0 {
        // Empty tree → fresh leaf with this one entry.
        let new_block = bitmap.alloc(1)?;
        let mut buf = make_empty_leaf();
        let mut hdr = read_header(&buf);
        hdr.num_entries = 1;
        write_header(&mut buf, &hdr);
        set_leaf_entry(&mut buf, 0, entry);
        write_node(cache, new_block, &mut buf)?;
        return Ok((new_block, old_blocks, true));
    }

    let mut path: [(u64, usize); MAX_TREE_DEPTH] = [(0, 0); MAX_TREE_DEPTH];
    let mut depth = 0usize;
    let mut block = root;

    loop {
        let mut buf = [0u8; BLOCK_SIZE];
        read_node(cache, block, &mut buf)?;
        let hdr = read_header(&buf);
        if hdr.magic != V2_BTREE_MAGIC { return Err(FsError::Corrupt); }

        match hdr.node_type {
            V2_BTREE_LEAF => {
                // Idempotent: same hash already present is a no-op. Caller
                // either had this object before us or just put the same
                // bytes — either way the on-disk state is already correct.
                for i in 0..hdr.num_entries as usize {
                    if leaf_entry(&buf, i).hash == entry.hash {
                        return Ok((root, old_blocks, false));
                    }
                }

                if (hdr.num_entries as usize) < V2_MAX_LEAF_ENTRIES {
                    let pos = find_leaf_insert_pos(&buf, &hdr, &entry.hash);
                    let new_block = bitmap.alloc(1)?;
                    let mut new_buf = buf;
                    insert_leaf_entry(&mut new_buf, &hdr, pos, entry);
                    write_node(cache, new_block, &mut new_buf)?;
                    old_blocks.push(block);

                    let (root_out, frees) =
                        fixup_path(cache, bitmap, &path, depth, new_block, &mut old_blocks)?;
                    return Ok((root_out, frees, true));
                } else {
                    let (left, right, split_key) =
                        split_leaf(cache, bitmap, &buf, &hdr, entry)?;
                    old_blocks.push(block);

                    let (root_out, frees) = fixup_path_split(
                        cache, bitmap, &path, depth,
                        left, right, &split_key, &mut old_blocks,
                    )?;
                    return Ok((root_out, frees, true));
                }
            }
            V2_BTREE_INTERNAL => {
                let (child_idx, child_block) = find_child_with_idx(&buf, &hdr, &entry.hash);
                if depth >= MAX_TREE_DEPTH - 1 { return Err(FsError::TreeTooDeep); }
                path[depth] = (block, child_idx);
                depth += 1;
                block = child_block;
            }
            _ => return Err(FsError::Corrupt),
        }
    }
}

/// Delete an entry by hash. Returns `(new_root, old_blocks_to_free)`.
pub fn delete(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    root: u64, key: &[u8; 32],
) -> Result<(u64, Vec<u64>), FsError> {
    if root == 0 { return Err(FsError::ObjectNotFound); }

    let mut old_blocks = Vec::new();
    let mut path: [(u64, usize); MAX_TREE_DEPTH] = [(0, 0); MAX_TREE_DEPTH];
    let mut depth = 0usize;
    let mut block = root;

    loop {
        let mut buf = [0u8; BLOCK_SIZE];
        read_node(cache, block, &mut buf)?;
        let hdr = read_header(&buf);
        if hdr.magic != V2_BTREE_MAGIC { return Err(FsError::Corrupt); }

        match hdr.node_type {
            V2_BTREE_LEAF => {
                let pos = (0..hdr.num_entries as usize)
                    .find(|&i| leaf_entry(&buf, i).hash == *key)
                    .ok_or(FsError::ObjectNotFound)?;

                if hdr.num_entries == 1 && depth > 0 {
                    old_blocks.push(block);
                    return remove_from_parent(cache, bitmap, &path, depth, &mut old_blocks);
                }

                let new_block = bitmap.alloc(1)?;
                let mut new_buf = buf;
                remove_leaf_entry(&mut new_buf, &hdr, pos);
                write_node(cache, new_block, &mut new_buf)?;
                old_blocks.push(block);

                if depth == 0 && hdr.num_entries == 1 {
                    return Ok((0, old_blocks));
                }

                return fixup_path(cache, bitmap, &path, depth, new_block, &mut old_blocks);
            }
            V2_BTREE_INTERNAL => {
                let (child_idx, child_block) = find_child_with_idx(&buf, &hdr, key);
                if depth >= MAX_TREE_DEPTH - 1 { return Err(FsError::TreeTooDeep); }
                path[depth] = (block, child_idx);
                depth += 1;
                block = child_block;
            }
            _ => return Err(FsError::Corrupt),
        }
    }
}

/// Walk every entry in the tree in key order, calling `f` for each.
/// Used by GC + diagnostic intents (Steps 5 + 9). Reachable from no
/// caller in Step 2 yet — kept here so Step 3+ doesn't need to revisit
/// the btree module.
#[allow(dead_code)]
pub fn iter_all<F: FnMut(&V2EntryRaw)>(
    cache: &mut BlockCache, root: u64, f: &mut F,
) -> Result<(), FsError> {
    if root == 0 { return Ok(()); }
    iter_subtree(cache, root, f)
}

#[allow(dead_code)]
fn iter_subtree<F: FnMut(&V2EntryRaw)>(
    cache: &mut BlockCache, block: u64, f: &mut F,
) -> Result<(), FsError> {
    let mut buf = [0u8; BLOCK_SIZE];
    read_node(cache, block, &mut buf)?;
    let hdr = read_header(&buf);
    if hdr.magic != V2_BTREE_MAGIC { return Err(FsError::Corrupt); }

    match hdr.node_type {
        V2_BTREE_LEAF => {
            for i in 0..hdr.num_entries as usize {
                f(&leaf_entry(&buf, i));
            }
            Ok(())
        }
        V2_BTREE_INTERNAL => {
            for i in 0..hdr.num_entries as usize {
                let child = internal_child(&buf, i);
                if child != 0 {
                    iter_subtree(cache, child, f)?;
                }
            }
            if hdr.right_child != 0 {
                iter_subtree(cache, hdr.right_child, f)?;
            }
            Ok(())
        }
        _ => Err(FsError::Corrupt),
    }
}

// ══ Internal helpers ══════════════════════════════════════════════════

fn find_child_block(buf: &[u8; BLOCK_SIZE], hdr: &V2NodeHeader, key: &[u8; 32]) -> u64 {
    find_child_with_idx(buf, hdr, key).1
}

fn find_child_with_idx(buf: &[u8; BLOCK_SIZE], hdr: &V2NodeHeader, key: &[u8; 32]) -> (usize, u64) {
    let n = hdr.num_entries as usize;
    for i in 0..n {
        if key[..] < *internal_key(buf, i) {
            return (i, internal_child(buf, i));
        }
    }
    (n, hdr.right_child)
}

fn find_leaf_insert_pos(buf: &[u8; BLOCK_SIZE], hdr: &V2NodeHeader, key: &[u8; 32]) -> usize {
    for i in 0..hdr.num_entries as usize {
        if key[..] < leaf_entry(buf, i).hash[..] {
            return i;
        }
    }
    hdr.num_entries as usize
}

fn insert_leaf_entry(buf: &mut [u8; BLOCK_SIZE], hdr: &V2NodeHeader, pos: usize, entry: &V2EntryRaw) {
    let n = hdr.num_entries as usize;
    for i in (pos..n).rev() {
        let e = leaf_entry(buf, i);
        set_leaf_entry(buf, i + 1, &e);
    }
    set_leaf_entry(buf, pos, entry);
    let mut new_hdr = *hdr;
    new_hdr.num_entries += 1;
    write_header(buf, &new_hdr);
}

fn remove_leaf_entry(buf: &mut [u8; BLOCK_SIZE], hdr: &V2NodeHeader, pos: usize) {
    let n = hdr.num_entries as usize;
    for i in pos..n - 1 {
        let e = leaf_entry(buf, i + 1);
        set_leaf_entry(buf, i, &e);
    }
    let mut new_hdr = *hdr;
    new_hdr.num_entries -= 1;
    write_header(buf, &new_hdr);
}

fn split_leaf(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    buf: &[u8; BLOCK_SIZE], hdr: &V2NodeHeader,
    new_entry: &V2EntryRaw,
) -> Result<(u64, u64, [u8; 32]), FsError> {
    let n = hdr.num_entries as usize;
    let total = n + 1;
    let mut entries = Vec::with_capacity(total);
    let mut inserted = false;
    for i in 0..n {
        let e = leaf_entry(buf, i);
        if !inserted && new_entry.hash[..] < e.hash[..] {
            entries.push(*new_entry);
            inserted = true;
        }
        entries.push(e);
    }
    if !inserted { entries.push(*new_entry); }

    let mid = total / 2;

    let left_block = bitmap.alloc(1)?;
    let right_block = bitmap.alloc(1)?;

    let mut left_buf = [0u8; BLOCK_SIZE];
    let left_hdr = V2NodeHeader {
        magic: V2_BTREE_MAGIC, node_type: V2_BTREE_LEAF,
        _pad: 0, num_entries: mid as u16, right_child: 0,
    };
    write_header(&mut left_buf, &left_hdr);
    for i in 0..mid {
        set_leaf_entry(&mut left_buf, i, &entries[i]);
    }
    write_node(cache, left_block, &mut left_buf)?;

    let right_count = total - mid;
    let mut right_buf = [0u8; BLOCK_SIZE];
    let right_hdr = V2NodeHeader {
        magic: V2_BTREE_MAGIC, node_type: V2_BTREE_LEAF,
        _pad: 0, num_entries: right_count as u16, right_child: 0,
    };
    write_header(&mut right_buf, &right_hdr);
    for i in 0..right_count {
        set_leaf_entry(&mut right_buf, i, &entries[mid + i]);
    }
    write_node(cache, right_block, &mut right_buf)?;

    let split_key = entries[mid].hash;
    Ok((left_block, right_block, split_key))
}

/// After a COW on a child, propagate the new pointer up the path.
fn fixup_path(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    path: &[(u64, usize); MAX_TREE_DEPTH], depth: usize,
    new_child: u64,
    old_blocks: &mut Vec<u64>,
) -> Result<(u64, Vec<u64>), FsError> {
    if depth == 0 {
        return Ok((new_child, core::mem::take(old_blocks)));
    }

    let mut child_block = new_child;

    for d in (0..depth).rev() {
        let (parent_block, child_idx) = path[d];
        let mut buf = [0u8; BLOCK_SIZE];
        read_node(cache, parent_block, &mut buf)?;
        let hdr = read_header(&buf);

        let new_parent = bitmap.alloc(1)?;
        let mut new_buf = buf;

        let n = hdr.num_entries as usize;
        if child_idx < n {
            let off = V2_NODE_HEADER_SIZE + child_idx * V2_INTERNAL_ENTRY_SIZE + 32;
            new_buf[off..off + 8].copy_from_slice(&child_block.to_le_bytes());
        } else {
            // Rightmost child sits in the header field.
            new_buf[8..16].copy_from_slice(&child_block.to_le_bytes());
        }

        write_node(cache, new_parent, &mut new_buf)?;
        old_blocks.push(parent_block);
        child_block = new_parent;
    }

    Ok((child_block, core::mem::take(old_blocks)))
}

/// After a leaf or internal split, propagate `(split_key, right_child)`
/// up the path, splitting parents as needed.
fn fixup_path_split(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    path: &[(u64, usize); MAX_TREE_DEPTH], depth: usize,
    left: u64, right: u64, split_key: &[u8; 32],
    old_blocks: &mut Vec<u64>,
) -> Result<(u64, Vec<u64>), FsError> {
    if depth == 0 {
        // Tree grows: new internal root with one key.
        let root_block = bitmap.alloc(1)?;
        let mut buf = [0u8; BLOCK_SIZE];
        let hdr = V2NodeHeader {
            magic: V2_BTREE_MAGIC, node_type: V2_BTREE_INTERNAL,
            _pad: 0, num_entries: 1, right_child: right,
        };
        write_header(&mut buf, &hdr);
        set_internal_entry(&mut buf, 0, split_key, left);
        write_node(cache, root_block, &mut buf)?;
        return Ok((root_block, core::mem::take(old_blocks)));
    }

    let (parent_block, _child_idx) = path[depth - 1];
    let mut buf = [0u8; BLOCK_SIZE];
    read_node(cache, parent_block, &mut buf)?;
    let hdr = read_header(&buf);
    let n = hdr.num_entries as usize;

    if n < V2_MAX_INTERNAL_KEYS {
        let new_parent = bitmap.alloc(1)?;
        let mut new_buf = buf;

        let pos = (0..n).find(|&i| split_key[..] < *internal_key(&buf, i)).unwrap_or(n);

        if pos == n {
            // New key at end: right_child slot moves to `right`.
            let mut new_hdr = hdr;
            new_hdr.num_entries += 1;
            new_hdr.right_child = right;
            write_header(&mut new_buf, &new_hdr);
            set_internal_entry(&mut new_buf, n, split_key, left);
        } else {
            // Shift entries [pos..n) right by one slot.
            for i in (pos..n).rev() {
                let key = <[u8; 32]>::try_from(internal_key(&buf, i)).unwrap();
                let child = internal_child(&buf, i);
                set_internal_entry(&mut new_buf, i + 1, &key, child);
            }
            set_internal_entry(&mut new_buf, pos, split_key, left);
            // The entry that was at `pos` (now at `pos+1`) keeps its key,
            // but its child pointer must become `right` — same fix as v1.
            let off = V2_NODE_HEADER_SIZE + (pos + 1) * V2_INTERNAL_ENTRY_SIZE + 32;
            new_buf[off..off + 8].copy_from_slice(&right.to_le_bytes());

            let mut new_hdr = hdr;
            new_hdr.num_entries += 1;
            write_header(&mut new_buf, &new_hdr);
        }

        write_node(cache, new_parent, &mut new_buf)?;
        old_blocks.push(parent_block);

        let remaining_depth = depth - 1;
        return fixup_path(cache, bitmap, path, remaining_depth, new_parent, old_blocks);
    }

    let (int_left, int_right, int_split_key) =
        split_internal(cache, bitmap, &buf, &hdr, split_key, left, right)?;
    old_blocks.push(parent_block);

    let remaining_depth = depth - 1;
    fixup_path_split(cache, bitmap, path, remaining_depth,
        int_left, int_right, &int_split_key, old_blocks)
}

fn split_internal(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    buf: &[u8; BLOCK_SIZE], hdr: &V2NodeHeader,
    new_key: &[u8; 32], new_left_child: u64, new_right_child: u64,
) -> Result<(u64, u64, [u8; 32]), FsError> {
    let n = hdr.num_entries as usize;

    struct KeyChild { key: [u8; 32], child: u64 }
    let mut entries: Vec<KeyChild> = Vec::with_capacity(n + 1);
    let mut inserted = false;
    for i in 0..n {
        let key: [u8; 32] = internal_key(buf, i).try_into().unwrap();
        if !inserted && new_key[..] < key[..] {
            entries.push(KeyChild { key: *new_key, child: new_left_child });
            inserted = true;
        }
        let child = internal_child(buf, i);
        entries.push(KeyChild { key, child });
    }
    if !inserted {
        entries.push(KeyChild { key: *new_key, child: new_left_child });
    }

    let insert_pos = entries.iter().position(|e| e.key == *new_key).unwrap();

    let rightmost = if insert_pos == entries.len() - 1 {
        new_right_child
    } else {
        entries[insert_pos + 1].child = new_right_child;
        hdr.right_child
    };

    let total = entries.len();
    let mid = total / 2;
    let promoted_key = entries[mid].key;

    let left_block = bitmap.alloc(1)?;
    let right_block = bitmap.alloc(1)?;

    let mut left_buf = [0u8; BLOCK_SIZE];
    let left_hdr = V2NodeHeader {
        magic: V2_BTREE_MAGIC, node_type: V2_BTREE_INTERNAL,
        _pad: 0, num_entries: mid as u16,
        right_child: entries[mid].child,
    };
    write_header(&mut left_buf, &left_hdr);
    for i in 0..mid {
        set_internal_entry(&mut left_buf, i, &entries[i].key, entries[i].child);
    }
    write_node(cache, left_block, &mut left_buf)?;

    let right_count = total - mid - 1;
    let mut right_buf = [0u8; BLOCK_SIZE];
    let right_hdr = V2NodeHeader {
        magic: V2_BTREE_MAGIC, node_type: V2_BTREE_INTERNAL,
        _pad: 0, num_entries: right_count as u16,
        right_child: rightmost,
    };
    write_header(&mut right_buf, &right_hdr);
    for i in 0..right_count {
        set_internal_entry(&mut right_buf, i, &entries[mid + 1 + i].key, entries[mid + 1 + i].child);
    }
    write_node(cache, right_block, &mut right_buf)?;

    Ok((left_block, right_block, promoted_key))
}

fn remove_from_parent(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    path: &[(u64, usize); MAX_TREE_DEPTH], depth: usize,
    old_blocks: &mut Vec<u64>,
) -> Result<(u64, Vec<u64>), FsError> {
    if depth == 0 { return Ok((0, core::mem::take(old_blocks))); }

    let (parent_block, child_idx) = path[depth - 1];
    let mut buf = [0u8; BLOCK_SIZE];
    read_node(cache, parent_block, &mut buf)?;
    let hdr = read_header(&buf);
    let n = hdr.num_entries as usize;

    if n <= 1 && depth == 1 {
        // Root with one key shrinks: promote the surviving child.
        let remaining = if child_idx == 0 {
            if n > 0 { hdr.right_child } else { 0 }
        } else {
            internal_child(&buf, 0)
        };
        old_blocks.push(parent_block);
        return Ok((remaining, core::mem::take(old_blocks)));
    }

    let new_parent = bitmap.alloc(1)?;
    let mut new_buf = buf;
    let mut new_hdr = hdr;

    // Same fix as v1: removing the rightmost child means promoting the
    // child at slot n-1 into the right_child header field. Forgetting
    // this leaves right_child dangling on a freed leaf.
    if child_idx < n {
        for i in child_idx..n - 1 {
            let key = <[u8; 32]>::try_from(internal_key(&buf, i + 1)).unwrap();
            let child = internal_child(&buf, i + 1);
            set_internal_entry(&mut new_buf, i, &key, child);
        }
    } else {
        new_hdr.right_child = internal_child(&buf, n - 1);
    }
    new_hdr.num_entries -= 1;
    write_header(&mut new_buf, &new_hdr);

    write_node(cache, new_parent, &mut new_buf)?;
    old_blocks.push(parent_block);

    let remaining_depth = depth - 1;
    fixup_path(cache, bitmap, path, remaining_depth, new_parent, old_blocks)
}
