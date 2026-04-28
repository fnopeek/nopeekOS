//! v2 high-level filesystem API.
//!
//! This is the layer the rest of the kernel talks to: take a `&str`
//! path, do the right thing, atomically flip the superblock to the
//! resulting root Tree hash on writes. Internally it composes the
//! three lower layers:
//!
//!   `fs::write(path, data)` → `paths::store(root, path, data)` →
//!     N × `storage::put(blob_or_tree)` → `storage::commit_root(new)`
//!
//! Concurrency: `ROOT_MUTEX` serializes mutations so two writers can't
//! observe the same root and clobber each other. Reads don't take the
//! mutex — they walk against whichever root is current at lock-grab.

#![allow(dead_code)]

use alloc::vec::Vec;
use spin::Mutex;

use super::object::{EntryKind, TreeEntry};
use super::paths::{self, PathError, WalkOk};
use super::storage;
use super::super::types::FsError;

/// Public error: a path-layer error or a storage-layer error.
/// Wire-shape choice — most callers care about "is the file there?"
/// more than "did postcard fail?", so we expose `PathError` directly.
pub use super::paths::PathError as Error;

/// Serializes root-mutating calls. Held only across `current_root +
/// paths::* + commit_root`. Storage layer takes its own (separate)
/// FS lock per `put`, so this isn't reentrant against itself.
static ROOT_MUTEX: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatResult {
    pub kind: EntryKind,
    pub size: u64,
}

fn current_root() -> Result<[u8; 32], Error> {
    storage::current_root().ok_or(Error::Storage(FsError::NotMounted))
}

fn commit(new_root: [u8; 32]) -> Result<(), Error> {
    storage::commit_root(new_root).map_err(Error::Storage)
}

// ── Reads (no SB mutation) ────────────────────────────────────────────

/// Look up `path` and return whether it exists, what kind, and its size.
/// `Ok(None)` for missing paths. Errors only for genuine FS issues
/// (NotADirectory mid-path, decode failures, NotMounted).
pub fn stat(path: &str) -> Result<Option<StatResult>, Error> {
    let root = current_root()?;
    match paths::walk(&root, path) {
        Ok(WalkOk { kind, size, .. }) => Ok(Some(StatResult { kind, size })),
        Err(PathError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// True iff `path` resolves to a File or Dir. Equivalent to `stat().is_some()`
/// but handy for booleans where the caller doesn't care about the kind.
pub fn exists(path: &str) -> Result<bool, Error> {
    Ok(stat(path)?.is_some())
}

/// Read a File's bytes. Returns `Ok(None)` if missing. Errors with
/// `NotADirectory` when used on a directory (intentional asymmetry —
/// directories aren't readable as Blobs; use `list`).
pub fn read(path: &str) -> Result<Option<Vec<u8>>, Error> {
    let root = current_root()?;
    let walk = match paths::walk(&root, path) {
        Ok(w) => w,
        Err(PathError::NotFound) => return Ok(None),
        Err(e) => return Err(e),
    };
    if walk.kind != EntryKind::File {
        return Err(PathError::NotADirectory);
    }
    let bytes = storage::get(&walk.hash)
        .map_err(Error::Storage)?
        .ok_or(PathError::Corrupt)?;
    match super::object::Object::decode(&bytes).map_err(|_| PathError::Corrupt)? {
        super::object::Object::Blob(b) => Ok(Some(b)),
        super::object::Object::Tree(_) => Err(PathError::Corrupt),
    }
}

/// List the entries directly under `path`. `path` must resolve to a Dir.
/// Returns `Ok(None)` if `path` is missing.
pub fn list(path: &str) -> Result<Option<Vec<TreeEntry>>, Error> {
    let root = current_root()?;
    match paths::list(&root, path) {
        Ok(v) => Ok(Some(v)),
        Err(PathError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

// ── Mutations (commit a new root via SB) ──────────────────────────────

/// Write `data` to `path` (creating or overwriting a File). Parent must
/// exist; if `path` exists as a Dir, errors with AlreadyExists.
pub fn write(path: &str, data: &[u8]) -> Result<(), Error> {
    let _g = ROOT_MUTEX.lock();
    let cur = current_root()?;
    let new = paths::store(&cur, path, data)?;
    commit(new)
}

/// Create an empty directory at `path`. Parent must exist; `path` must
/// not.
pub fn mkdir(path: &str) -> Result<(), Error> {
    let _g = ROOT_MUTEX.lock();
    let cur = current_root()?;
    let new = paths::mkdir(&cur, path)?;
    commit(new)
}

/// Idempotent variant: returns Ok if `path` already exists as a Dir,
/// errors with AlreadyExists only if it's a File. Useful for installer
/// + setup paths that may run more than once.
pub fn ensure_dir(path: &str) -> Result<(), Error> {
    let _g = ROOT_MUTEX.lock();
    let cur = current_root()?;
    match paths::walk(&cur, path) {
        Ok(WalkOk { kind: EntryKind::Dir, .. }) => return Ok(()),
        Ok(_) => return Err(PathError::AlreadyExists),
        Err(PathError::NotFound) => {}
        Err(e) => return Err(e),
    }
    let new = paths::mkdir(&cur, path)?;
    commit(new)
}

/// Remove `path`. Files: dropped unconditionally. Dirs: must be empty.
/// Idempotent on missing — `Ok(())` if `path` already absent.
pub fn delete(path: &str) -> Result<(), Error> {
    let _g = ROOT_MUTEX.lock();
    let cur = current_root()?;
    match paths::delete(&cur, path) {
        Ok(new) => commit(new),
        Err(PathError::NotFound) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Move `old` to `new`. Cross-parent supported. `new` must not exist.
pub fn rename(old: &str, new: &str) -> Result<(), Error> {
    let _g = ROOT_MUTEX.lock();
    let cur = current_root()?;
    let new_root = paths::rename(&cur, old, new)?;
    commit(new_root)
}

// ── Convenience: ensure a chain of dirs (mkdir -p) ────────────────────

/// Ensure every parent dir of `path` exists. `path` itself is treated
/// as a directory chain — call as `ensure_dirs("a/b/c")` to create
/// `a`, `a/b`, `a/b/c`. Idempotent.
pub fn ensure_dirs(path: &str) -> Result<(), Error> {
    let segs = paths::parse_path(path)?;
    if segs.is_empty() { return Ok(()); }

    let mut acc = alloc::string::String::new();
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 { acc.push('/'); }
        acc.push_str(seg);
        ensure_dir(&acc)?;
    }
    Ok(())
}

// ── Garbage collection ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct GcStats {
    /// Number of objects reachable from any live SB slot.
    pub kept: usize,
    /// Number of objects removed because no SB referenced them.
    pub removed: usize,
}

/// Mark-and-sweep GC over the v2 object store.
///
/// Reachability roots: every valid SB slot's `root_tree_hash`. The 8
/// rotating slots give us "last 8 commits" snapshots automatically —
/// anything reachable from any of them is preserved.
///
/// Concurrency: holds ROOT_MUTEX so no path-layer mutation runs during
/// GC. Sweep deletes use the regular `storage::remove` path so each
/// goes through the journal + 4-phase commit; safe but slow on large
/// orphan sets. Battle-test (Step 10) decides whether to batch.
pub fn gc() -> Result<GcStats, Error> {
    use hashbrown::HashSet;

    let _g = ROOT_MUTEX.lock();

    let roots = storage::all_root_hashes().map_err(Error::Storage)?;
    let mut reachable: HashSet<[u8; 32]> = HashSet::new();
    let mut work: alloc::vec::Vec<[u8; 32]> = roots;

    while let Some(hash) = work.pop() {
        if hash == paths::EMPTY_ROOT { continue; }
        if !reachable.insert(hash) { continue; }

        let bytes = match storage::get(&hash).map_err(Error::Storage)? {
            Some(b) => b,
            // A dangling reference means an upstream commit referenced
            // an object that's not in the B-tree. Either pre-GC partial
            // state or genuine corruption — either way, we can't recurse
            // into it. Skip and keep going.
            None => continue,
        };
        // Tree objects expand the work-list; Blobs are leaves.
        if let Ok(super::object::Object::Tree(entries)) = super::object::Object::decode(&bytes) {
            for e in entries {
                if e.hash != paths::EMPTY_ROOT {
                    work.push(e.hash);
                }
            }
        }
    }

    let all = storage::all_object_hashes().map_err(Error::Storage)?;
    let mut removed = 0usize;
    for h in all {
        if reachable.contains(&h) { continue; }
        // remove returns ObjectNotFound only if the entry vanished
        // between our enumeration and the delete — impossible under
        // ROOT_MUTEX, but tolerate it defensively.
        match storage::remove(&h) {
            Ok(()) => removed += 1,
            Err(super::super::types::FsError::ObjectNotFound) => {}
            Err(e) => return Err(Error::Storage(e)),
        }
    }

    Ok(GcStats { kept: reachable.len(), removed })
}
