//! Copy-on-Write B-Tree
//!
//! On-disk B-tree keyed by object name. All modifications create new blocks
//! (COW), old blocks freed after superblock commit. SSD-friendly: no
//! in-place updates, writes always go to fresh blocks.

use super::types::*;
use super::cache::BlockCache;
use super::bitmap::Bitmap;

const MAX_TREE_DEPTH: usize = 8;
const CHECKSUM_OFFSET: usize = BLOCK_SIZE - 32; // BLAKE3 checksum at end of block

fn compute_node_checksum(buf: &[u8; BLOCK_SIZE]) -> [u8; 32] {
    *blake3::hash(&buf[..CHECKSUM_OFFSET]).as_bytes()
}

fn write_node_checksum(buf: &mut [u8; BLOCK_SIZE]) {
    let cs = compute_node_checksum(buf);
    buf[CHECKSUM_OFFSET..].copy_from_slice(&cs);
}

fn verify_node_checksum(buf: &[u8; BLOCK_SIZE]) -> bool {
    let stored = &buf[CHECKSUM_OFFSET..];
    // All zeros = old format without checksum, accept
    if stored.iter().all(|&b| b == 0) { return true; }
    let expected = compute_node_checksum(buf);
    stored == expected
}

// === Node parsing helpers ===

fn read_header(buf: &[u8; BLOCK_SIZE]) -> BTreeNodeHeader {
    BTreeNodeHeader {
        magic: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        node_type: buf[4],
        _pad: buf[5],
        num_entries: u16::from_le_bytes(buf[6..8].try_into().unwrap()),
        next_leaf: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
    }
}

fn write_header(buf: &mut [u8; BLOCK_SIZE], hdr: &BTreeNodeHeader) {
    buf[0..4].copy_from_slice(&hdr.magic.to_le_bytes());
    buf[4] = hdr.node_type;
    buf[5] = hdr._pad;
    buf[6..8].copy_from_slice(&hdr.num_entries.to_le_bytes());
    buf[8..16].copy_from_slice(&hdr.next_leaf.to_le_bytes());
}

// Internal node: after header, entries are [name: [u8;64], child: u64]
// The header.next_leaf field stores the rightmost child pointer.

fn internal_key(buf: &[u8; BLOCK_SIZE], idx: usize) -> &[u8] {
    let off = NODE_HEADER_SIZE + idx * INTERNAL_ENTRY_SIZE;
    &buf[off..off + 64]
}

fn internal_child(buf: &[u8; BLOCK_SIZE], idx: usize) -> u64 {
    let off = NODE_HEADER_SIZE + idx * INTERNAL_ENTRY_SIZE + 64;
    u64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

fn set_internal_entry(buf: &mut [u8; BLOCK_SIZE], idx: usize, key: &[u8; 64], child: u64) {
    let off = NODE_HEADER_SIZE + idx * INTERNAL_ENTRY_SIZE;
    buf[off..off + 64].copy_from_slice(key);
    buf[off + 64..off + 72].copy_from_slice(&child.to_le_bytes());
}

// Leaf node: after header, entries are ObjectEntry (196 bytes each)

fn leaf_entry(buf: &[u8; BLOCK_SIZE], idx: usize) -> ObjectEntry {
    let off = NODE_HEADER_SIZE + idx * LEAF_ENTRY_SIZE;
    let src = &buf[off..off + LEAF_ENTRY_SIZE];
    // SAFETY: ObjectEntry is repr(C) and exactly LEAF_ENTRY_SIZE bytes
    unsafe { core::ptr::read_unaligned(src.as_ptr() as *const ObjectEntry) }
}

fn set_leaf_entry(buf: &mut [u8; BLOCK_SIZE], idx: usize, entry: &ObjectEntry) {
    let off = NODE_HEADER_SIZE + idx * LEAF_ENTRY_SIZE;
    let src = unsafe {
        core::slice::from_raw_parts(entry as *const ObjectEntry as *const u8, LEAF_ENTRY_SIZE)
    };
    buf[off..off + LEAF_ENTRY_SIZE].copy_from_slice(src);
}

/// Read a B-tree node block and verify its checksum.
fn read_node(cache: &mut BlockCache, block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {
    cache.read(block, buf)?;
    if !verify_node_checksum(buf) {
        return Err(FsError::Corrupt);
    }
    Ok(())
}

/// Write a B-tree node block with checksum.
fn write_node(cache: &mut BlockCache, block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {
    write_node_checksum(buf);
    cache.write(block, buf)?;
    Ok(())
}

fn make_empty_leaf() -> [u8; BLOCK_SIZE] {
    let mut buf = [0u8; BLOCK_SIZE];
    let hdr = BTreeNodeHeader {
        magic: BTREE_MAGIC, node_type: BTREE_LEAF,
        _pad: 0, num_entries: 0, next_leaf: 0,
    };
    write_header(&mut buf, &hdr);
    buf
}

// === Public API ===

/// Lookup an object by name. Returns None if not found.
pub fn lookup(
    cache: &mut BlockCache, root: u64, name: &[u8; 64],
) -> Result<Option<ObjectEntry>, FsError> {
    if root == 0 { return Ok(None); }
    let mut block = root;

    loop {
        let mut buf = [0u8; BLOCK_SIZE];
        read_node(cache, block, &mut buf)?;
        let hdr = read_header(&buf);
        if hdr.magic != BTREE_MAGIC { return Err(FsError::Corrupt); }

        match hdr.node_type {
            BTREE_LEAF => {
                for i in 0..hdr.num_entries as usize {
                    let e = leaf_entry(&buf, i);
                    if e.name == *name { return Ok(Some(e)); }
                }
                return Ok(None);
            }
            BTREE_INTERNAL => {
                block = find_child_block(&buf, &hdr, name);
            }
            _ => return Err(FsError::Corrupt),
        }
    }
}

/// Insert an entry. Returns (new_root, old_blocks_to_free).
pub fn insert(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    root: u64, entry: &ObjectEntry,
) -> Result<(u64, alloc::vec::Vec<u64>), FsError> {
    let mut old_blocks = alloc::vec::Vec::new();

    if root == 0 {
        // Empty tree: create a leaf
        let new_block = bitmap.alloc(1)?;
        let mut buf = make_empty_leaf();
        let mut hdr = read_header(&buf);
        hdr.num_entries = 1;
        write_header(&mut buf, &hdr);
        set_leaf_entry(&mut buf, 0, entry);
        write_node(cache, new_block, &mut buf)?;
        return Ok((new_block, old_blocks));
    }

    // Walk to leaf, recording path
    let mut path: [(u64, usize); MAX_TREE_DEPTH] = [(0, 0); MAX_TREE_DEPTH];
    let mut depth = 0usize;
    let mut block = root;

    loop {
        let mut buf = [0u8; BLOCK_SIZE];
        read_node(cache, block, &mut buf)?;
        let hdr = read_header(&buf);
        if hdr.magic != BTREE_MAGIC { return Err(FsError::Corrupt); }

        match hdr.node_type {
            BTREE_LEAF => {
                // Check duplicate
                for i in 0..hdr.num_entries as usize {
                    if leaf_entry(&buf, i).name == entry.name {
                        return Err(FsError::ObjectExists);
                    }
                }

                if (hdr.num_entries as usize) < MAX_LEAF_ENTRIES {
                    // Room in leaf: COW insert
                    let pos = find_leaf_insert_pos(&buf, &hdr, &entry.name);
                    let new_block = bitmap.alloc(1)?;
                    let mut new_buf = buf;
                    insert_leaf_entry(&mut new_buf, &hdr, pos, entry);
                    write_node(cache, new_block, &mut new_buf)?;
                    old_blocks.push(block);

                    return fixup_path(cache, bitmap, &path, depth, new_block, None, &mut old_blocks);
                } else {
                    // Leaf full: split
                    let (left_block, right_block, split_key) =
                        split_leaf(cache, bitmap, &buf, &hdr, entry)?;
                    old_blocks.push(block);

                    return fixup_path_split(cache, bitmap, &path, depth,
                        left_block, right_block, &split_key, &mut old_blocks);
                }
            }
            BTREE_INTERNAL => {
                let (child_idx, child_block) = find_child_with_idx(&buf, &hdr, &entry.name);
                if depth >= MAX_TREE_DEPTH - 1 { return Err(FsError::TreeTooDeep); }
                path[depth] = (block, child_idx);
                depth += 1;
                block = child_block;
            }
            _ => return Err(FsError::Corrupt),
        }
    }
}

/// Delete an entry by name. Returns (new_root, old_blocks_to_free).
pub fn delete(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    root: u64, name: &[u8; 64],
) -> Result<(u64, alloc::vec::Vec<u64>), FsError> {
    if root == 0 { return Err(FsError::ObjectNotFound); }

    let mut old_blocks = alloc::vec::Vec::new();
    let mut path: [(u64, usize); MAX_TREE_DEPTH] = [(0, 0); MAX_TREE_DEPTH];
    let mut depth = 0usize;
    let mut block = root;

    loop {
        let mut buf = [0u8; BLOCK_SIZE];
        read_node(cache, block, &mut buf)?;
        let hdr = read_header(&buf);
        if hdr.magic != BTREE_MAGIC { return Err(FsError::Corrupt); }

        match hdr.node_type {
            BTREE_LEAF => {
                let pos = (0..hdr.num_entries as usize)
                    .find(|&i| leaf_entry(&buf, i).name == *name)
                    .ok_or(FsError::ObjectNotFound)?;

                if hdr.num_entries == 1 && depth > 0 {
                    // Leaf becomes empty, remove from parent
                    old_blocks.push(block);
                    return remove_from_parent(cache, bitmap, &path, depth, &mut old_blocks);
                }

                let new_block = bitmap.alloc(1)?;
                let mut new_buf = buf;
                remove_leaf_entry(&mut new_buf, &hdr, pos);
                write_node(cache, new_block, &mut new_buf)?;
                old_blocks.push(block);

                if depth == 0 && hdr.num_entries == 1 {
                    // Tree becomes empty
                    return Ok((0, old_blocks));
                }

                return fixup_path(cache, bitmap, &path, depth, new_block, None, &mut old_blocks);
            }
            BTREE_INTERNAL => {
                let (child_idx, child_block) = find_child_with_idx(&buf, &hdr, name);
                if depth >= MAX_TREE_DEPTH - 1 { return Err(FsError::TreeTooDeep); }
                path[depth] = (block, child_idx);
                depth += 1;
                block = child_block;
            }
            _ => return Err(FsError::Corrupt),
        }
    }
}

/// Iterate all leaf entries in order. Calls `f` for each ObjectEntry.
pub fn iter_all<F: FnMut(&ObjectEntry)>(
    cache: &mut BlockCache, root: u64, f: &mut F,
) -> Result<(), FsError> {
    if root == 0 { return Ok(()); }
    iter_subtree(cache, root, f)
}

/// Recursively walk every leaf reachable from `block`. Deliberately
/// does NOT use the `next_leaf` sibling chain — that chain is known
/// to break after a leaf split (the previous leaf's next_leaf pointer
/// stays pointing at the freed pre-split block, so iter-via-chain
/// silently drops entries). The tree structure (internal nodes +
/// their children) is the source of truth; we walk that.
///
/// Depth is bounded by tree height — for 64-byte keys and BLOCK_SIZE
/// leaves this stays <5 for any realistic npkFS, so recursion is safe.
fn iter_subtree<F: FnMut(&ObjectEntry)>(
    cache: &mut BlockCache, block: u64, f: &mut F,
) -> Result<(), FsError> {
    let mut buf = [0u8; BLOCK_SIZE];
    read_node(cache, block, &mut buf)?;
    let hdr = read_header(&buf);
    if hdr.magic != BTREE_MAGIC { return Err(FsError::Corrupt); }

    match hdr.node_type {
        BTREE_LEAF => {
            for i in 0..hdr.num_entries as usize {
                f(&leaf_entry(&buf, i));
            }
            Ok(())
        }
        BTREE_INTERNAL => {
            // Internal node layout: num_entries pairs of (key, child)
            // plus the rightmost child in hdr.next_leaf. Total n+1
            // children; visit them left-to-right.
            for i in 0..hdr.num_entries as usize {
                let child = internal_child(&buf, i);
                if child != 0 {
                    iter_subtree(cache, child, f)?;
                }
            }
            if hdr.next_leaf != 0 {
                iter_subtree(cache, hdr.next_leaf, f)?;
            }
            Ok(())
        }
        _ => Err(FsError::Corrupt),
    }
}

// === Internal helpers ===

fn find_child_block(buf: &[u8; BLOCK_SIZE], hdr: &BTreeNodeHeader, name: &[u8; 64]) -> u64 {
    find_child_with_idx(buf, hdr, name).1
}

fn find_child_with_idx(buf: &[u8; BLOCK_SIZE], hdr: &BTreeNodeHeader, name: &[u8; 64]) -> (usize, u64) {
    let n = hdr.num_entries as usize;
    for i in 0..n {
        if name[..] < *internal_key(buf, i) {
            return (i, internal_child(buf, i));
        }
    }
    // Greater than all keys → rightmost child
    (n, hdr.next_leaf)
}

fn find_leaf_insert_pos(buf: &[u8; BLOCK_SIZE], hdr: &BTreeNodeHeader, name: &[u8; 64]) -> usize {
    for i in 0..hdr.num_entries as usize {
        if name[..] < leaf_entry(buf, i).name[..] {
            return i;
        }
    }
    hdr.num_entries as usize
}

fn insert_leaf_entry(buf: &mut [u8; BLOCK_SIZE], hdr: &BTreeNodeHeader, pos: usize, entry: &ObjectEntry) {
    let n = hdr.num_entries as usize;
    // Shift entries right
    for i in (pos..n).rev() {
        let e = leaf_entry(buf, i);
        set_leaf_entry(buf, i + 1, &e);
    }
    set_leaf_entry(buf, pos, entry);
    let mut new_hdr = *hdr;
    new_hdr.num_entries += 1;
    write_header(buf, &new_hdr);
}

fn remove_leaf_entry(buf: &mut [u8; BLOCK_SIZE], hdr: &BTreeNodeHeader, pos: usize) {
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
    buf: &[u8; BLOCK_SIZE], hdr: &BTreeNodeHeader,
    new_entry: &ObjectEntry,
) -> Result<(u64, u64, [u8; 64]), FsError> {
    // Collect all entries + new one, sorted
    let n = hdr.num_entries as usize;
    let total = n + 1;
    let mut entries = alloc::vec::Vec::with_capacity(total);
    let mut inserted = false;
    for i in 0..n {
        let e = leaf_entry(buf, i);
        if !inserted && new_entry.name[..] < e.name[..] {
            entries.push(*new_entry);
            inserted = true;
        }
        entries.push(e);
    }
    if !inserted { entries.push(*new_entry); }

    let mid = total / 2;

    // Left leaf
    let left_block = bitmap.alloc(1)?;
    let right_block = bitmap.alloc(1)?;

    let mut left_buf = [0u8; BLOCK_SIZE];
    // next_leaf on leaf nodes is reserved (always 0). iter_all walks
    // the tree structure, not a sibling chain, so we don't try to
    // maintain one — maintaining it across splits would require a
    // predecessor search on every insert without parent pointers.
    let left_hdr = BTreeNodeHeader {
        magic: BTREE_MAGIC, node_type: BTREE_LEAF,
        _pad: 0, num_entries: mid as u16, next_leaf: 0,
    };
    write_header(&mut left_buf, &left_hdr);
    for i in 0..mid {
        set_leaf_entry(&mut left_buf, i, &entries[i]);
    }
    write_node(cache, left_block, &mut left_buf)?;

    // Right leaf
    let right_count = total - mid;
    let mut right_buf = [0u8; BLOCK_SIZE];
    let right_hdr = BTreeNodeHeader {
        magic: BTREE_MAGIC, node_type: BTREE_LEAF,
        _pad: 0, num_entries: right_count as u16, next_leaf: 0,
    };
    write_header(&mut right_buf, &right_hdr);
    for i in 0..right_count {
        set_leaf_entry(&mut right_buf, i, &entries[mid + i]);
    }
    write_node(cache, right_block, &mut right_buf)?;

    // Split key = first key of right node
    let split_key = entries[mid].name;

    Ok((left_block, right_block, split_key))
}

/// After COW of a leaf/child, update parent pointers up the path.
fn fixup_path(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    path: &[(u64, usize); MAX_TREE_DEPTH], depth: usize,
    new_child: u64, _split: Option<(&[u8; 64], u64)>,
    old_blocks: &mut alloc::vec::Vec<u64>,
) -> Result<(u64, alloc::vec::Vec<u64>), FsError> {
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

        // Update child pointer
        let n = hdr.num_entries as usize;
        if child_idx < n {
            let off = NODE_HEADER_SIZE + child_idx * INTERNAL_ENTRY_SIZE + 64;
            new_buf[off..off + 8].copy_from_slice(&child_block.to_le_bytes());
        } else {
            // Rightmost child
            new_buf[8..16].copy_from_slice(&child_block.to_le_bytes());
        }

        write_node(cache, new_parent, &mut new_buf)?;
        old_blocks.push(parent_block);
        child_block = new_parent;
    }

    Ok((child_block, core::mem::take(old_blocks)))
}

/// After a leaf split, propagate the new key+child up the path.
fn fixup_path_split(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    path: &[(u64, usize); MAX_TREE_DEPTH], depth: usize,
    left: u64, right: u64, split_key: &[u8; 64],
    old_blocks: &mut alloc::vec::Vec<u64>,
) -> Result<(u64, alloc::vec::Vec<u64>), FsError> {
    if depth == 0 {
        // Create new root
        let root_block = bitmap.alloc(1)?;
        let mut buf = [0u8; BLOCK_SIZE];
        let hdr = BTreeNodeHeader {
            magic: BTREE_MAGIC, node_type: BTREE_INTERNAL,
            _pad: 0, num_entries: 1, next_leaf: right, // rightmost child
        };
        write_header(&mut buf, &hdr);
        set_internal_entry(&mut buf, 0, split_key, left);
        write_node(cache, root_block, &mut buf)?;
        return Ok((root_block, core::mem::take(old_blocks)));
    }

    // Insert split key into parent
    let (parent_block, _child_idx) = path[depth - 1];
    let mut buf = [0u8; BLOCK_SIZE];
    read_node(cache, parent_block, &mut buf)?;
    let hdr = read_header(&buf);
    let n = hdr.num_entries as usize;

    if n < MAX_INTERNAL_KEYS {
        // Room in parent
        let new_parent = bitmap.alloc(1)?;
        let mut new_buf = buf;

        // Find insertion position
        let pos = (0..n).find(|&i| split_key[..] < *internal_key(&buf, i)).unwrap_or(n);

        // Shift entries right
        // Update rightmost child if inserting at end
        if pos == n {
            // New key goes at end, right child becomes new rightmost
            let _old_rightmost = hdr.next_leaf;
            let mut new_hdr = hdr;
            new_hdr.num_entries += 1;
            new_hdr.next_leaf = right;
            write_header(&mut new_buf, &new_hdr);
            set_internal_entry(&mut new_buf, n, split_key, left);
        } else {
            // Shift entries at pos..n right by 1
            for i in (pos..n).rev() {
                let key = <[u8; 64]>::try_from(internal_key(&buf, i)).unwrap();
                let child = internal_child(&buf, i);
                set_internal_entry(&mut new_buf, i + 1, &key, child);
            }
            set_internal_entry(&mut new_buf, pos, split_key, left);
            // The child pointer at pos+1 should be right... but the existing
            // child at pos was pointing to the old unsplit node. After split,
            // left replaces it (already set above), and right needs to be
            // the child pointer for keys > split_key.
            // Actually: internal_child(pos) = left (child < split_key)
            // For the next key (pos+1), its child = right
            // But we shifted pos+1..n to pos+2..n+1. The child at pos+1 is
            // the old child at pos, which should now be 'right'.
            let off = NODE_HEADER_SIZE + (pos + 1) * INTERNAL_ENTRY_SIZE + 64;
            new_buf[off..off + 8].copy_from_slice(&right.to_le_bytes());

            let mut new_hdr = hdr;
            new_hdr.num_entries += 1;
            write_header(&mut new_buf, &new_hdr);
        }

        write_node(cache, new_parent, &mut new_buf)?;
        old_blocks.push(parent_block);

        // Continue fixup up the path
        let remaining_depth = depth - 1;
        return fixup_path(cache, bitmap, path, remaining_depth, new_parent, None, old_blocks);
    }

    // Parent full: split the internal node and propagate up
    let (int_left, int_right, int_split_key) =
        split_internal(cache, bitmap, &buf, &hdr, split_key, left, right)?;
    old_blocks.push(parent_block);

    let remaining_depth = depth - 1;
    fixup_path_split(cache, bitmap, path, remaining_depth,
        int_left, int_right, &int_split_key, old_blocks)
}

/// Split a full internal node. Returns (left_block, right_block, split_key).
/// The split_key is promoted to the parent (not stored in either child).
fn split_internal(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    buf: &[u8; BLOCK_SIZE], hdr: &BTreeNodeHeader,
    new_key: &[u8; 64], new_left_child: u64, new_right_child: u64,
) -> Result<(u64, u64, [u8; 64]), FsError> {
    let n = hdr.num_entries as usize;

    // Collect all keys+children including the new entry, sorted
    // Each internal entry: (key, left_child). Rightmost child is separate.
    struct KeyChild { key: [u8; 64], child: u64 }
    let mut entries = alloc::vec::Vec::with_capacity(n + 1);
    let mut inserted = false;
    for i in 0..n {
        let key: [u8; 64] = internal_key(buf, i).try_into().unwrap();
        if !inserted && new_key[..] < key[..] {
            entries.push(KeyChild { key: *new_key, child: new_left_child });
            inserted = true;
        }
        let child = internal_child(buf, i);
        // If we just inserted new_key before this entry, the child pointer
        // for this entry needs to be updated if it was the split point
        if inserted && entries.len() == entries.capacity() - (n - i) {
            entries.push(KeyChild { key, child });
        } else {
            entries.push(KeyChild { key, child });
        }
    }
    if !inserted {
        entries.push(KeyChild { key: *new_key, child: new_left_child });
    }

    // Determine the rightmost child for each half.
    // After inserting new_key: the child left of new_key is new_left_child,
    // the child right of new_key is new_right_child.
    // We need to fix up the child pointers around the insertion point.
    let insert_pos = entries.iter().position(|e| e.key == *new_key).unwrap();

    // Fix child pointer: the entry AFTER the inserted key should have
    // new_right_child as its left child. But in our representation,
    // each entry's child is the LEFT child of that key.
    // After insert_pos, the next entry's child is correct (was shifted).
    // The rightmost child: if inserted at end, new_right_child is rightmost.
    let rightmost = if insert_pos == entries.len() - 1 {
        new_right_child
    } else {
        // The old rightmost child stays, but the entry after insert gets new_right_child
        // Actually: entry[insert_pos].child = new_left_child (already set)
        // entry[insert_pos+1] should have child = new_right_child
        entries[insert_pos + 1].child = new_right_child;
        hdr.next_leaf // old rightmost preserved
    };

    let total = entries.len(); // n + 1
    let mid = total / 2;

    // The median key is promoted (not stored in either child node)
    let promoted_key = entries[mid].key;

    // Left internal node: entries[0..mid]
    let left_block = bitmap.alloc(1)?;
    let right_block = bitmap.alloc(1)?;

    let mut left_buf = [0u8; BLOCK_SIZE];
    let left_hdr = BTreeNodeHeader {
        magic: BTREE_MAGIC, node_type: BTREE_INTERNAL,
        _pad: 0, num_entries: mid as u16,
        next_leaf: entries[mid].child, // rightmost child of left = left child of promoted key
    };
    write_header(&mut left_buf, &left_hdr);
    for i in 0..mid {
        set_internal_entry(&mut left_buf, i, &entries[i].key, entries[i].child);
    }
    write_node(cache, left_block, &mut left_buf)?;

    // Right internal node: entries[mid+1..total]
    let right_count = total - mid - 1;
    let mut right_buf = [0u8; BLOCK_SIZE];
    let right_hdr = BTreeNodeHeader {
        magic: BTREE_MAGIC, node_type: BTREE_INTERNAL,
        _pad: 0, num_entries: right_count as u16,
        next_leaf: rightmost,
    };
    write_header(&mut right_buf, &right_hdr);
    for i in 0..right_count {
        set_internal_entry(&mut right_buf, i, &entries[mid + 1 + i].key, entries[mid + 1 + i].child);
    }
    write_node(cache, right_block, &mut right_buf)?;

    Ok((left_block, right_block, promoted_key))
}

/// Remove a child reference from the parent when a leaf becomes empty.
fn remove_from_parent(
    cache: &mut BlockCache, bitmap: &mut Bitmap,
    path: &[(u64, usize); MAX_TREE_DEPTH], depth: usize,
    old_blocks: &mut alloc::vec::Vec<u64>,
) -> Result<(u64, alloc::vec::Vec<u64>), FsError> {
    if depth == 0 { return Ok((0, core::mem::take(old_blocks))); }

    let (parent_block, child_idx) = path[depth - 1];
    let mut buf = [0u8; BLOCK_SIZE];
    read_node(cache, parent_block, &mut buf)?;
    let hdr = read_header(&buf);
    let n = hdr.num_entries as usize;

    if n <= 1 && depth == 1 {
        // Parent was root with one key, tree shrinks
        // Return the other child as new root
        let remaining = if child_idx == 0 {
            if n > 0 { hdr.next_leaf } else { 0 }
        } else {
            internal_child(&buf, 0)
        };
        old_blocks.push(parent_block);
        return Ok((remaining, core::mem::take(old_blocks)));
    }

    let new_parent = bitmap.alloc(1)?;
    let mut new_buf = buf;

    // Remove the child_idx entry
    if child_idx < n {
        for i in child_idx..n - 1 {
            let key = <[u8; 64]>::try_from(internal_key(&buf, i + 1)).unwrap();
            let child = internal_child(&buf, i + 1);
            set_internal_entry(&mut new_buf, i, &key, child);
        }
    }
    let mut new_hdr = hdr;
    new_hdr.num_entries -= 1;
    write_header(&mut new_buf, &new_hdr);

    write_node(cache, new_parent, &mut new_buf)?;
    old_blocks.push(parent_block);

    let remaining_depth = depth - 1;
    fixup_path(cache, bitmap, path, remaining_depth, new_parent, None, old_blocks)
}
