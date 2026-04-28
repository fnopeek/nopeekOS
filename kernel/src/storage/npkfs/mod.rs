//! npkFS — content-addressed filesystem.
//!
//! Public API surface preserved for the rest of the kernel
//! (`crate::npkfs::fetch`, `npkfs::store`, …) but the v1 path-as-key
//! backend is gone. All calls route to v2 (Git-style trees) underneath.
//! Existing callers see the same shape; the on-disk reality is the
//! new content-addressed format.
//!
//! Mount-time guard: if a v1 superblock is detected at boot, the
//! kernel halts with a reinstall message rather than trying to use
//! it (no in-place migration — clean break by design).

mod types;
mod cache;
mod bitmap;
mod superblock;
#[allow(dead_code)]
mod journal;
#[allow(dead_code)]
mod btree;
pub mod v2;

pub use types::{FsError, BLOCK_SIZE};

use alloc::string::String;
use alloc::vec::Vec;

use crate::kprintln;

// ── v1-detection guard at mount time ──────────────────────────────────

/// First eight bytes of a v1 superblock. v2 uses `npkFS\x02\0\0`. The
/// only place these can both appear is the 8-slot SB ring at block 1+.
const V1_MAGIC: [u8; 8] = *b"npkFS\x01\0\0";

/// Read the first SB slot and check for v1 magic. Used as a pre-mount
/// guard so we never auto-mkfs over a user's existing v1 data.
fn v1_disk_present() -> bool {
    let mut buf = [0u8; BLOCK_SIZE];
    if crate::blkdev::read_block(types::SUPERBLOCK_START, &mut buf).is_err() {
        return false;
    }
    buf[..8] == V1_MAGIC
}

fn halt_for_v1_disk() -> ! {
    kprintln!("");
    kprintln!("[npk] ┌──────────────────────────────────────────────────────────┐");
    kprintln!("[npk] │ This disk is formatted as npkFS v1.                      │");
    kprintln!("[npk] │                                                          │");
    kprintln!("[npk] │ npkFS v2 is incompatible by design (clean break, no      │");
    kprintln!("[npk] │ migration). Boot the installer USB and reinstall to      │");
    kprintln!("[npk] │ continue. Your previous data is unrecoverable from this  │");
    kprintln!("[npk] │ kernel — restore from backup if you have one.            │");
    kprintln!("[npk] └──────────────────────────────────────────────────────────┘");
    kprintln!("");
    loop { unsafe { core::arch::asm!("cli; hlt"); } }
}

// ── Bridge: v1 surface routed to v2 ───────────────────────────────────

/// Format the disk to npkFS v2.
pub fn mkfs() -> Result<(), FsError> {
    v2::storage::mkfs()
}

/// Mount the disk. Halts with a reinstall message if v1 magic is found.
pub fn mount() -> Result<(), FsError> {
    if v1_disk_present() {
        halt_for_v1_disk();
    }
    v2::storage::mount()
}

pub fn is_mounted() -> bool { v2::storage::is_mounted() }

pub fn install_salt() -> Option<[u8; 16]> { v2::storage::install_salt() }

pub fn stats() -> Option<(u64, u64, u64, u64)> { v2::storage::stats() }

/// Strict create: errors with `ObjectExists` if `name` is already present.
/// Mirrors v1's `store` semantics. `cap_id` is accepted for ABI compat
/// and ignored by v2.
pub fn store(name: &str, data: &[u8], _cap_id: [u8; 32]) -> Result<[u8; 32], FsError> {
    let path = clean_path(name);
    validate(path)?;
    if v2_exists(path) { return Err(FsError::ObjectExists); }
    write_with_parents(path, data)?;
    Ok(*blake3::hash(data).as_bytes())
}

/// Insert-or-replace. Mirrors v1's `upsert` semantics.
pub fn upsert(name: &str, data: &[u8], _cap_id: [u8; 32]) -> Result<[u8; 32], FsError> {
    let path = clean_path(name);
    validate(path)?;
    write_with_parents(path, data)?;
    Ok(*blake3::hash(data).as_bytes())
}

/// Read an object. Returns `(plaintext, blake3_hash)`. Mirrors v1's tuple
/// shape so callers don't need rewriting.
pub fn fetch(name: &str) -> Result<(Vec<u8>, [u8; 32]), FsError> {
    let path = clean_path(name);
    validate(path)?;
    match v2::fs::read(path) {
        Ok(Some(data)) => {
            let h = *blake3::hash(&data).as_bytes();
            Ok((data, h))
        }
        Ok(None) => Err(FsError::ObjectNotFound),
        Err(e) => Err(path_to_fs_err(e)),
    }
}

/// Remove an object. Errors with `ObjectNotFound` if missing (v1 shape).
pub fn delete(name: &str) -> Result<(), FsError> {
    let path = clean_path(name);
    validate(path)?;
    if !v2_exists(path) { return Err(FsError::ObjectNotFound); }
    v2::fs::delete(path).map_err(path_to_fs_err)
}

/// Flat list of every File in the tree, recursively. Format:
/// `(slash_path, byte_size, blake3_hash)`. Mirrors v1's `list()` shape.
/// Cost: walks the entire root Tree subtree. Acceptable until callers
/// migrate to per-directory `v2::fs::list(path)`.
pub fn list() -> Result<Vec<(String, u64, [u8; 32])>, FsError> {
    let mut out = Vec::new();
    walk_recursive(String::new(), &mut out)?;
    Ok(out)
}

pub fn exists(name: &str) -> bool {
    let path = clean_path(name);
    if validate(path).is_err() { return false; }
    v2_exists(path)
}

/// Reject reserved names that would clash with kernel-managed paths.
/// v2 reserves `.system/` for boot config + keycheck. v1's `.npk-` and
/// `/.dir` legacy patterns are also rejected so apps can't trip over
/// transitional debris.
pub fn validate_user_name(name: &str) -> Result<(), FsError> {
    let path = clean_path(name);
    validate(path)?;
    if path.starts_with(".system/") || path == ".system" {
        return Err(FsError::ReservedName);
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    if last.starts_with(".npk-") {
        return Err(FsError::ReservedName);
    }
    if path.ends_with("/.dir") {
        return Err(FsError::ReservedName);
    }
    Ok(())
}

// ── Bridge helpers ────────────────────────────────────────────────────

fn clean_path(name: &str) -> &str {
    name.trim_matches('/')
}

fn validate(path: &str) -> Result<(), FsError> {
    if path.is_empty() { return Err(FsError::InvalidName); }
    if path.bytes().any(|b| b == 0) { return Err(FsError::InvalidName); }
    Ok(())
}

fn v2_exists(path: &str) -> bool {
    matches!(v2::fs::exists(path), Ok(true))
}

fn write_with_parents(path: &str, data: &[u8]) -> Result<(), FsError> {
    if let Some(slash) = path.rfind('/') {
        let parent = &path[..slash];
        v2::fs::ensure_dirs(parent).map_err(path_to_fs_err)?;
    }
    v2::fs::write(path, data).map_err(path_to_fs_err)
}

fn path_to_fs_err(e: v2::paths::PathError) -> FsError {
    use v2::paths::PathError as P;
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

/// DFS the v2 root Tree, appending every File entry as a flat
/// `(slash_path, size, hash)` tuple. Skips `.system/` (kernel-internal).
fn walk_recursive(prefix: String, out: &mut Vec<(String, u64, [u8; 32])>) -> Result<(), FsError> {
    let listing = match v2::fs::list(&prefix) {
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
            v2::object::EntryKind::File => {
                out.push((path, entry.size, entry.hash));
            }
            v2::object::EntryKind::Dir => {
                walk_recursive(path, out)?;
            }
        }
    }
    Ok(())
}
