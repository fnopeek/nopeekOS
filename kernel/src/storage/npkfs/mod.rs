//! npkFS — content-addressed filesystem.
//!
//! Single flat layer (no `v1/` `v2/` namespacing — those got
//! consolidated when the v1 path-as-key backend was retired and v2
//! Git-style trees became the canonical implementation).
//!
//! Submodules:
//!   `object`  — `Blob`/`Tree` wire format (postcard + BLAKE3)
//!   `format`  — on-disk superblock + B-tree node layout
//!   `sb_io`   — 8-slot rotating superblock read/write
//!   `btree`   — COW B-tree keyed by 32-byte hashes
//!   `storage` — mkfs/mount/put/get/has/remove + 4-phase commit
//!   `paths`   — slash-path walker + tree mutations on top of storage
//!   `fs`      — high-level API that flips SB.root_tree_hash atomically
//!               and exposes the GC.
//!
//! Public surface lives in this `mod.rs`: `mkfs`, `mount`, `fetch`,
//! `store`, `upsert`, `delete`, `exists`, `list`, plus `stats`,
//! `install_salt`, `is_mounted`, `validate_user_name`. The submodules
//! are `pub` so the kernel can reach into typed APIs (e.g.
//! `npkfs::fs::list` for per-directory listings, `npkfs::object`
//! types for typed iteration).

mod types;
mod cache;
mod bitmap;
mod journal;

pub mod object;
mod format;
mod sb_io;
mod btree;
pub mod storage;
pub mod paths;
pub mod fs;

pub use types::{FsError, BLOCK_SIZE};

use alloc::string::String;
use alloc::vec::Vec;

// ── Public surface ────────────────────────────────────────────────────

/// Format the disk to npkFS.
pub fn mkfs() -> Result<(), FsError> {
    storage::mkfs()
}

/// Mount the disk.
pub fn mount() -> Result<(), FsError> {
    storage::mount()
}

pub fn is_mounted() -> bool { storage::is_mounted() }

pub fn install_salt() -> Option<[u8; 16]> { storage::install_salt() }

pub fn stats() -> Option<(u64, u64, u64, u64)> { storage::stats() }

/// Strict create: errors with `ObjectExists` if `name` is already present.
/// `cap_id` is accepted for ABI compat and ignored by the content-
/// addressed backend.
pub fn store(name: &str, data: &[u8], _cap_id: [u8; 32]) -> Result<[u8; 32], FsError> {
    let path = clean_path(name);
    validate(path)?;
    if exists_inner(path) { return Err(FsError::ObjectExists); }
    write_with_parents(path, data)?;
    Ok(*blake3::hash(data).as_bytes())
}

/// Insert-or-replace.
pub fn upsert(name: &str, data: &[u8], _cap_id: [u8; 32]) -> Result<[u8; 32], FsError> {
    let path = clean_path(name);
    validate(path)?;
    write_with_parents(path, data)?;
    Ok(*blake3::hash(data).as_bytes())
}

/// Read an object. Returns `(plaintext, content_hash)`. The hash is
/// the walk hash from the tree (BLAKE3 of the encoded Blob); already
/// verified against the on-disk integrity by `storage::get` before
/// the bytes are handed back. We don't re-hash the plaintext — that
/// was a 0.6 ms tax per 1 MB read for no security gain.
pub fn fetch(name: &str) -> Result<(Vec<u8>, [u8; 32]), FsError> {
    let path = clean_path(name);
    validate(path)?;
    match fs::read_with_hash(path) {
        Ok(Some((data, hash))) => Ok((data, hash)),
        Ok(None) => Err(FsError::ObjectNotFound),
        Err(e) => Err(path_to_fs_err(e)),
    }
}

/// Remove an object. Errors with `ObjectNotFound` if missing.
pub fn delete(name: &str) -> Result<(), FsError> {
    let path = clean_path(name);
    validate(path)?;
    if !exists_inner(path) { return Err(FsError::ObjectNotFound); }
    fs::delete(path).map_err(path_to_fs_err)
}

/// Flat list of every File in the tree, recursively. Format:
/// `(slash_path, byte_size, blake3_hash)`. Walks the entire root tree.
/// Acceptable until callers migrate to per-directory `fs::list(path)`.
pub fn list() -> Result<Vec<(String, u64, [u8; 32])>, FsError> {
    let mut out = Vec::new();
    walk_recursive(String::new(), &mut out)?;
    Ok(out)
}

pub fn exists(name: &str) -> bool {
    let path = clean_path(name);
    if validate(path).is_err() { return false; }
    exists_inner(path)
}

/// Reject reserved names that would clash with kernel-managed paths.
/// `.system/` is reserved for boot config + keycheck.
pub fn validate_user_name(name: &str) -> Result<(), FsError> {
    let path = clean_path(name);
    validate(path)?;
    if path.starts_with(".system/") || path == ".system" {
        return Err(FsError::ReservedName);
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────

fn clean_path(name: &str) -> &str {
    name.trim_matches('/')
}

fn validate(path: &str) -> Result<(), FsError> {
    if path.is_empty() { return Err(FsError::InvalidName); }
    if path.bytes().any(|b| b == 0) { return Err(FsError::InvalidName); }
    Ok(())
}

fn exists_inner(path: &str) -> bool {
    matches!(fs::exists(path), Ok(true))
}

fn write_with_parents(path: &str, data: &[u8]) -> Result<(), FsError> {
    if let Some(slash) = path.rfind('/') {
        let parent = &path[..slash];
        fs::ensure_dirs(parent).map_err(path_to_fs_err)?;
    }
    fs::write(path, data).map_err(path_to_fs_err)
}

fn path_to_fs_err(e: paths::PathError) -> FsError {
    use paths::PathError as P;
    match e {
        P::InvalidPath    => FsError::InvalidName,
        P::NotFound       => FsError::ObjectNotFound,
        P::NotADirectory  => FsError::InvalidName,
        P::AlreadyExists  => FsError::ObjectExists,
        P::NotEmpty       => FsError::InvalidName,
        P::Corrupt        => FsError::Corrupt,
        P::Storage(inner) => inner,
    }
}

/// DFS the root Tree, appending every File entry as a flat
/// `(slash_path, size, hash)` tuple. Skips `.system/` (kernel-internal).
fn walk_recursive(prefix: String, out: &mut Vec<(String, u64, [u8; 32])>) -> Result<(), FsError> {
    let listing = match fs::list(&prefix) {
        Ok(Some(v)) => v,
        Ok(None)    => return Ok(()),
        Err(e)      => return Err(path_to_fs_err(e)),
    };
    for entry in listing {
        let mut path = prefix.clone();
        if !path.is_empty() { path.push('/'); }
        path.push_str(&entry.name);

        // Don't surface kernel-internal storage to user-space listings.
        if path == ".system" || path.starts_with(".system/") {
            continue;
        }

        match entry.kind {
            object::EntryKind::File => {
                out.push((path, entry.size, entry.hash));
            }
            object::EntryKind::Dir => {
                walk_recursive(path, out)?;
            }
        }
    }
    Ok(())
}
