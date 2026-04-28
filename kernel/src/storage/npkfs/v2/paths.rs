//! v2 path layer — read & mutate via `(root_tree_hash, slash_path)`.
//!
//! Pure functions over the storage layer: each mutation takes the
//! current root Tree hash + a path + payload, and returns a NEW root
//! Tree hash. Old roots remain walkable (content-addressed snapshots).
//!
//! Atomicity: nothing in this layer touches the superblock's
//! `root_tree_hash`. The high-level `fs` API (Step 5+) decides when to
//! flip the superblock to the new root, which is what makes the user-
//! visible mutation atomic. A crash mid-mutation here just leaves
//! orphan Tree blobs in the object store — they're collected by GC.
//!
//! Empty-root convention: a hash of all zeros is the sentinel for
//! "no root Tree exists yet". Reading it returns an empty directory;
//! the first mutation creates the actual on-disk Tree object. This
//! keeps `mkfs` from having to choose an encryption mode before the
//! master key is known.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;

use super::object::{EntryKind, Object, TreeEntry, MAX_NAME_LEN};
use super::storage;
use super::super::types::FsError;

/// Sentinel for "the root Tree doesn't exist yet". `walk` of any path
/// against this returns NotFound; mutations create the real Tree.
pub const EMPTY_ROOT: [u8; 32] = [0u8; 32];

#[derive(Debug)]
pub enum PathError {
    /// Path is empty where it shouldn't be, contains `.`/`..`/empty
    /// segments / NUL bytes, or a name exceeds MAX_NAME_LEN.
    InvalidPath,
    /// A path component is missing in its parent Tree.
    NotFound,
    /// Path tried to descend through a File.
    NotADirectory,
    /// Inserting where a name is already present (and overwrite is
    /// disallowed for that variant — e.g. mkdir over an existing
    /// entry, or rename onto an existing target).
    AlreadyExists,
    /// `delete` / `rename`-out of a non-empty directory.
    NotEmpty,
    /// Encoding/decoding of a Tree object failed, or a hash that
    /// should reference a Tree references a Blob (FS corruption).
    Corrupt,
    Storage(FsError),
}

impl From<FsError> for PathError {
    fn from(e: FsError) -> Self { PathError::Storage(e) }
}

#[derive(Clone, Copy, Debug)]
pub struct WalkOk {
    pub hash: [u8; 32],
    pub kind: EntryKind,
    /// File: byte size of the referenced Blob.
    /// Dir : recursive sum of File sizes underneath.
    pub size: u64,
}

// ── Path parsing ──────────────────────────────────────────────────────

/// Split a slash-separated path into validated segments. Empty input
/// (or just slashes) returns an empty Vec, which means "the root".
pub fn parse_path(path: &str) -> Result<Vec<&str>, PathError> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() { return Ok(Vec::new()); }

    let mut out = Vec::new();
    for s in trimmed.split('/') {
        if s.is_empty() { return Err(PathError::InvalidPath); }
        if s.bytes().any(|b| b == 0) { return Err(PathError::InvalidPath); }
        if s == "." || s == ".." { return Err(PathError::InvalidPath); }
        if s.len() > MAX_NAME_LEN { return Err(PathError::InvalidPath); }
        out.push(s);
    }
    Ok(out)
}

// ── Tree fetch / store helpers ────────────────────────────────────────

/// Read a Tree object by hash. The empty-root sentinel returns Vec::new()
/// without touching storage.
fn fetch_tree(hash: &[u8; 32]) -> Result<Vec<TreeEntry>, PathError> {
    if *hash == EMPTY_ROOT {
        return Ok(Vec::new());
    }
    let bytes = storage::get(hash)?.ok_or(PathError::Corrupt)?;
    match Object::decode(&bytes).map_err(|_| PathError::Corrupt)? {
        Object::Tree(entries) => Ok(entries),
        Object::Blob(_) => Err(PathError::NotADirectory),
    }
}

/// Encode + put a Tree object. Returns its content hash and the
/// recursive byte size (sum of `entry.size` for File + Dir entries —
/// for File these are byte sizes, for Dir already-recursive sums, so
/// the sum stays correct by induction).
///
/// `saturating_add` instead of `.sum()`: 9.2 EB is unreachable in
/// practice, but `Iterator::sum` on `u64` panics in debug and wraps
/// silently in release. Saturating is the disciplined kernel-side
/// move and costs nothing.
fn store_tree(entries: Vec<TreeEntry>) -> Result<([u8; 32], u64), PathError> {
    let recursive_size: u64 = entries.iter()
        .fold(0u64, |acc, e| acc.saturating_add(e.size));
    let obj = Object::tree_sorted(entries).map_err(|_| PathError::Corrupt)?;
    let (bytes, hash) = obj.encode_and_hash().map_err(|_| PathError::Corrupt)?;
    // Trees stay plaintext: boot-time `fs::exists` has to walk them
    // before the user has logged in, so AEAD on Trees would brick the
    // first-vs-subsequent-boot decision. File contents (Blobs) are
    // encrypted separately in `paths::store` below.
    storage::put(&hash, &bytes, /* encrypt */ false)?;
    Ok((hash, recursive_size))
}

// ── Walk ──────────────────────────────────────────────────────────────

/// Walk `path` from `root`, returning what's at the end.
///
/// - Empty path → root itself (kind=Dir, size=root's recursive size)
/// - Missing component → `NotFound`
/// - Descending through a File → `NotADirectory`
pub fn walk(root: &[u8; 32], path: &str) -> Result<WalkOk, PathError> {
    let segs = parse_path(path)?;

    if segs.is_empty() {
        let entries = fetch_tree(root)?;
        let size: u64 = entries.iter()
            .fold(0u64, |acc, e| acc.saturating_add(e.size));
        return Ok(WalkOk { hash: *root, kind: EntryKind::Dir, size });
    }

    let mut cur_hash = *root;
    let last = segs.len() - 1;
    for (i, seg) in segs.iter().enumerate() {
        let entries = fetch_tree(&cur_hash)?;
        let entry = entries
            .iter()
            .find(|e| e.name == *seg)
            .ok_or(PathError::NotFound)?;

        if i == last {
            return Ok(WalkOk { hash: entry.hash, kind: entry.kind, size: entry.size });
        }
        if entry.kind != EntryKind::Dir {
            return Err(PathError::NotADirectory);
        }
        cur_hash = entry.hash;
    }
    unreachable!("loop returned on last seg")
}

// ── Mutation helpers ──────────────────────────────────────────────────

/// Walk to the parent of a target path, returning the chain of trees
/// from root to parent (inclusive). Used by mutations to rebuild
/// bottom-up after editing the leaf.
fn walk_for_mutation<'a>(
    root: &[u8; 32],
    parent_segs: &[&'a str],
) -> Result<Vec<Vec<TreeEntry>>, PathError> {
    let mut trees: Vec<Vec<TreeEntry>> = Vec::with_capacity(parent_segs.len() + 1);
    trees.push(fetch_tree(root)?);

    for seg in parent_segs {
        let cur = trees.last().unwrap();
        let entry = cur.iter().find(|e| e.name == *seg).ok_or(PathError::NotFound)?;
        if entry.kind != EntryKind::Dir { return Err(PathError::NotADirectory); }
        let next = fetch_tree(&entry.hash)?;
        trees.push(next);
    }
    Ok(trees)
}

/// Replace the leaf of a `walk_for_mutation` chain and rebuild every
/// ancestor up to (and including) the new root. Returns new root hash.
fn rebuild_up(
    parent_segs: &[&str],
    mut trees: Vec<Vec<TreeEntry>>,
    new_leaf_entries: Vec<TreeEntry>,
) -> Result<[u8; 32], PathError> {
    *trees.last_mut().unwrap() = new_leaf_entries;

    let leaf = trees.pop().unwrap();
    let (mut cur_hash, mut cur_size) = store_tree(leaf)?;

    for i in (0..parent_segs.len()).rev() {
        let mut anc = trees.pop().unwrap();
        let seg = parent_segs[i];
        let mut found = false;
        for entry in &mut anc {
            if entry.name == seg {
                entry.hash = cur_hash;
                entry.size = cur_size;
                found = true;
                break;
            }
        }
        if !found { return Err(PathError::Corrupt); }
        let (h, s) = store_tree(anc)?;
        cur_hash = h;
        cur_size = s;
    }

    debug_assert!(trees.is_empty());
    Ok(cur_hash)
}

/// Insert `new_entry` at `segs` (path components, last = name).
///
/// Collision policy:
///   - `name` not present → insert
///   - `name` present, kinds match, and `allow_replace` set → replace
///   - any other collision → AlreadyExists
fn insert_entry_at(
    root: &[u8; 32],
    segs: &[&str],
    new_entry: TreeEntry,
    allow_replace: bool,
) -> Result<[u8; 32], PathError> {
    if segs.is_empty() { return Err(PathError::InvalidPath); }
    let (parent_segs, name_seg) = segs.split_at(segs.len() - 1);
    let name = name_seg[0];

    let trees = walk_for_mutation(root, parent_segs)?;
    let mut leaf = trees.last().cloned().unwrap();

    if let Some(existing) = leaf.iter().find(|e| e.name == name) {
        let same_kind = existing.kind == new_entry.kind;
        if !(allow_replace && same_kind) {
            return Err(PathError::AlreadyExists);
        }
        leaf.retain(|e| e.name != name);
    }
    leaf.push(new_entry);
    rebuild_up(parent_segs, trees, leaf)
}

/// Remove the entry named at `segs` and return it + the new root.
/// `require_empty_dir`: if the target is a Dir, its Tree must be empty
/// (POSIX rmdir semantics) — set to false for `rename`'s detach phase.
fn remove_entry_at(
    root: &[u8; 32],
    segs: &[&str],
    require_empty_dir: bool,
) -> Result<(TreeEntry, [u8; 32]), PathError> {
    if segs.is_empty() { return Err(PathError::InvalidPath); }
    let (parent_segs, name_seg) = segs.split_at(segs.len() - 1);
    let name = name_seg[0];

    let trees = walk_for_mutation(root, parent_segs)?;
    let mut leaf = trees.last().cloned().unwrap();

    let pos = leaf.iter().position(|e| e.name == name).ok_or(PathError::NotFound)?;
    let entry = leaf[pos].clone();

    if require_empty_dir && entry.kind == EntryKind::Dir {
        let dir_tree = fetch_tree(&entry.hash)?;
        if !dir_tree.is_empty() { return Err(PathError::NotEmpty); }
    }

    leaf.remove(pos);
    let new_root = rebuild_up(parent_segs, trees, leaf)?;
    Ok((entry, new_root))
}

// ── Public mutations ──────────────────────────────────────────────────

/// Store `data` as a Blob at `path`. Parent must exist and be a Dir.
/// If `path` already names a File, it's replaced; if it names a Dir,
/// errors with `AlreadyExists`.
pub fn store(root: &[u8; 32], path: &str, data: &[u8]) -> Result<[u8; 32], PathError> {
    let segs = parse_path(path)?;
    if segs.is_empty() { return Err(PathError::InvalidPath); }

    // Stream-hash the would-be Blob's content address (no alloc, no
    // encode) so we can skip `data.to_vec() + encode_and_hash() +
    // storage::put` entirely when the blob already exists. ~1 ms/MB
    // saved on dedup hits.
    let blob_hash = super::object::blob_content_hash(data);

    if !storage::has(&blob_hash) {
        // Cache-miss path: full encode + AEAD + put.
        let blob = Object::Blob(data.to_vec());
        let (blob_bytes, full_hash) =
            blob.encode_and_hash().map_err(|_| PathError::Corrupt)?;
        debug_assert_eq!(blob_hash, full_hash,
            "stream-hashed blob_content_hash diverged from encode_and_hash");
        storage::put(&full_hash, &blob_bytes, /* encrypt */ true)?;
    }

    let entry = TreeEntry {
        name: String::from(*segs.last().unwrap()),
        hash: blob_hash,
        kind: EntryKind::File,
        size: data.len() as u64,
        flags: 0,
    };
    insert_entry_at(root, &segs, entry, /* allow_replace */ true)
}

/// Create an empty directory at `path`. Parent must exist; `path` must
/// not already exist.
pub fn mkdir(root: &[u8; 32], path: &str) -> Result<[u8; 32], PathError> {
    let segs = parse_path(path)?;
    if segs.is_empty() { return Err(PathError::InvalidPath); }

    let empty = Object::Tree(Vec::new());
    let (bytes, hash) = empty.encode_and_hash().map_err(|_| PathError::Corrupt)?;
    // Tree → unencrypted (same reasoning as `store_tree`).
    storage::put(&hash, &bytes, /* encrypt */ false)?;

    let entry = TreeEntry {
        name: String::from(*segs.last().unwrap()),
        hash,
        kind: EntryKind::Dir,
        size: 0,
        flags: 0,
    };
    insert_entry_at(root, &segs, entry, /* allow_replace */ false)
}

/// Remove `path`. Files are dropped unconditionally; directories must
/// be empty (`NotEmpty` if not).
pub fn delete(root: &[u8; 32], path: &str) -> Result<[u8; 32], PathError> {
    let segs = parse_path(path)?;
    if segs.is_empty() { return Err(PathError::InvalidPath); }
    let (_, new_root) = remove_entry_at(root, &segs, /* require_empty_dir */ true)?;
    Ok(new_root)
}

/// Move `old_path` to `new_path`. Same parent or cross-parent both work.
/// `new_path` must NOT already exist (no implicit overwrite).
pub fn rename(root: &[u8; 32], old: &str, new: &str) -> Result<[u8; 32], PathError> {
    let old_segs = parse_path(old)?;
    let new_segs = parse_path(new)?;
    if old_segs.is_empty() || new_segs.is_empty() {
        return Err(PathError::InvalidPath);
    }
    if old_segs == new_segs {
        return Ok(*root);
    }
    // Refuse moving a directory into its own subtree (would create a cycle).
    if new_segs.len() > old_segs.len() && new_segs[..old_segs.len()] == *old_segs {
        return Err(PathError::InvalidPath);
    }

    // Detach phase: dirs need NOT be empty here — we're carrying the
    // subtree across, not rmdir'ing it.
    let (entry, root1) = remove_entry_at(root, &old_segs, /* require_empty_dir */ false)?;

    let mut renamed = entry;
    renamed.name = String::from(*new_segs.last().unwrap());

    insert_entry_at(&root1, &new_segs, renamed, /* allow_replace */ false)
}

/// List the entries at `path`. Returns owned entries in sort order
/// (Tree objects are stored sorted by `Object::tree_sorted`).
pub fn list(root: &[u8; 32], path: &str) -> Result<Vec<TreeEntry>, PathError> {
    let segs = parse_path(path)?;

    let mut cur_hash = *root;
    for seg in &segs {
        let entries = fetch_tree(&cur_hash)?;
        let entry = entries
            .iter()
            .find(|e| e.name == *seg)
            .ok_or(PathError::NotFound)?;
        if entry.kind != EntryKind::Dir { return Err(PathError::NotADirectory); }
        cur_hash = entry.hash;
    }

    fetch_tree(&cur_hash)
}
