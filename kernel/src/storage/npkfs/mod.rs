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

/// Per-operation perf-timing logs (`[put]`/`[get]`/`[fs::read]`/
/// `[fs::write]` with µs breakdowns). Profiling instrumentation for
/// npkFS throughput-tuning sessions — off by default because a large
/// streaming download flushes one 16 MiB chunk after another and
/// each would emit a `[put]` line, drowning the console. Flip to
/// `true` when actively profiling the FS.
pub(crate) const FS_PERF_LOG: bool = false;

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

/// Make everything durable, then return. For shutdown and reboot.
///
/// `halt` used to power the machine off with `out dx, al` three lines after
/// printing "Goodbye" — no drain, no cache flush, no device flush. The COW
/// design is meant to survive exactly that, and mostly it does. But during a
/// streaming download hundreds of megabytes of allocation, a moved object
/// btree root and the whole deferred `pending_old_blocks` batch live only in
/// memory, and pulling the power there has never been tested. Florian reaches
/// for `halt` precisely when a download has hung — i.e. always in that state.
///
/// Draining first costs one four-phase commit and removes the entire question.
pub fn sync() {
    if !storage::is_mounted() { return; }
    if let Err(e) = storage::flush_pending() {
        crate::kprintln!("[npk] npkfs: sync failed ({:?}) — powering off anyway", e);
    }
    let _ = crate::blkdev::flush();
}

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

/// Open a streaming writer for `name`. Use this for inputs that don't
/// fit comfortably in a single `Vec<u8>` (downloads, ISO images, media
/// blobs). The writer accumulates 16 MiB chunks, encrypt-stores each
/// as its own content-addressed `Blob`, and on `finish` emits an
/// `Object::Chunked` manifest pointing at every chunk. Peak heap
/// during a multi-GB write stays at one chunk + manifest overhead.
///
/// Read-side is transparent: `fetch(name)` stitches the chunks back
/// into a single `Vec<u8>` for consumers that expect the legacy
/// shape.
///
/// Drop-without-finish leaks already-flushed chunks into storage;
/// the next `gc()` reclaims them. The path tree is only updated
/// atomically at `finish`, so partial downloads never appear as
/// half-written files.
pub fn open_streaming_write(name: &str) -> Result<fs::StreamingWriter, FsError> {
    let path = clean_path(name);
    validate(path)?;
    if let Some(slash) = path.rfind('/') {
        let parent = &path[..slash];
        fs::ensure_dirs(parent).map_err(path_to_fs_err)?;
    }
    Ok(fs::open_streaming_write(path))
}

/// Remove an object. Errors with `ObjectNotFound` if missing.
pub fn delete(name: &str) -> Result<(), FsError> {
    let path = clean_path(name);
    validate(path)?;
    if !exists_inner(path) { return Err(FsError::ObjectNotFound); }
    fs::delete(path).map_err(path_to_fs_err)
}

/// Move `old` to `new` (rename / cross-directory move). `new` must not
/// already exist; `old` must. Works for files and whole directories.
pub fn rename(old: &str, new: &str) -> Result<(), FsError> {
    let old = clean_path(old);
    let new = clean_path(new);
    validate(old)?;
    validate(new)?;
    if !exists_inner(old) { return Err(FsError::ObjectNotFound); }
    if exists_inner(new) { return Err(FsError::ObjectExists); }
    fs::rename(old, new).map_err(path_to_fs_err)
}

/// Copy `old` to `new`. Content-addressed alias — no data duplication,
/// even for a whole directory. `new` must not exist; `old` must.
pub fn copy(old: &str, new: &str) -> Result<(), FsError> {
    let old = clean_path(old);
    let new = clean_path(new);
    validate(old)?;
    validate(new)?;
    if !exists_inner(old) { return Err(FsError::ObjectNotFound); }
    if exists_inner(new) { return Err(FsError::ObjectExists); }
    fs::copy(old, new).map_err(path_to_fs_err)
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
