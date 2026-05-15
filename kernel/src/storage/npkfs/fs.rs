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
use super::types::FsError;

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
    pub kind:  EntryKind,
    pub size:  u64,
    /// UTC seconds since the Unix epoch, captured at write time.
    /// Zero means "unknown" (RTC was unreadable when the entry was
    /// created). Always zero for the root tree.
    pub mtime: u64,
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
        Ok(WalkOk { kind, size, mtime, .. }) => {
            Ok(Some(StatResult { kind, size, mtime }))
        }
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
    Ok(read_with_hash(path)?.map(|(b, _)| b))
}

/// Same as [`read`] but also returns the file's content-address (the
/// walk hash from the tree, which is BLAKE3 of the encoded Blob). Used
/// by the v1-compat bridge to avoid recomputing a fresh BLAKE3 over
/// every read — `storage::get` already verifies integrity against the
/// walk hash before handing the plaintext back, so re-hashing is pure
/// overhead. On a 1 MB read with AVX2 BLAKE3, the redundant hash costs
/// ~0.6 ms — measurable in the testdisk huge-file numbers.
pub fn read_with_hash(path: &str) -> Result<Option<(Vec<u8>, [u8; 32])>, Error> {
    use crate::interrupts::{rdtsc, tsc_freq};
    let t0 = rdtsc();

    let root = current_root()?;
    let walk = match paths::walk(&root, path) {
        Ok(w) => w,
        Err(PathError::NotFound) => return Ok(None),
        Err(e) => return Err(e),
    };
    let t_walk = rdtsc();

    if walk.kind != EntryKind::File {
        return Err(PathError::NotADirectory);
    }
    let bytes = storage::get(&walk.hash)
        .map_err(Error::Storage)?
        .ok_or(PathError::Corrupt)?;
    let encoded_len = bytes.len();
    let t_get = rdtsc();

    // Fast path: in-place blob decoder (saves ~0.9 ms / MB vs full
    // postcard decode). Falls back to a full decode if the variant
    // tag isn't `Blob(0)` — typically `Chunked(2)` for files written
    // via the streaming writer.
    let blob = if bytes.first() == Some(&0) {
        super::object::decode_blob_inplace(bytes)
            .map_err(|_| PathError::Corrupt)?
    } else {
        match super::object::Object::decode(&bytes).map_err(|_| PathError::Corrupt)? {
            super::object::Object::Blob(b) => b,
            super::object::Object::Chunked { total_size, chunks } => {
                stitch_chunked(total_size, &chunks)?
            }
            super::object::Object::Tree(_) => return Err(PathError::Corrupt),
        }
    };
    let t_decode = rdtsc();

    if encoded_len >= 256 * 1024 {
        let mhz = tsc_freq().max(1) / 1_000_000;
        crate::kprintln!("[fs::read] {}KB walk={} get={} decode={} (us)",
            encoded_len / 1024,
            (t_walk.saturating_sub(t0)) / mhz,
            (t_get.saturating_sub(t_walk)) / mhz,
            (t_decode.saturating_sub(t_get)) / mhz,
        );
    }

    Ok(Some((blob, walk.hash)))
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
    use crate::interrupts::{rdtsc, tsc_freq};
    let t0 = rdtsc();
    let _g = ROOT_MUTEX.lock();
    let t_lock = rdtsc();
    let cur = current_root()?;
    let new = paths::store(&cur, path, data)?;
    let t_store = rdtsc();
    let result = commit(new);
    let t_commit = rdtsc();

    if data.len() >= 256 * 1024 {
        let mhz = tsc_freq().max(1) / 1_000_000;
        crate::kprintln!("[fs::write] {}KB lock={} store={} commit={} (us)",
            data.len() / 1024,
            (t_lock.saturating_sub(t0)) / mhz,
            (t_store.saturating_sub(t_lock)) / mhz,
            (t_commit.saturating_sub(t_store)) / mhz,
        );
    }

    result
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
        // Tree + Chunked objects expand the work-list; Blobs are leaves.
        match super::object::Object::decode(&bytes) {
            Ok(super::object::Object::Tree(entries)) => {
                for e in entries {
                    if e.hash != paths::EMPTY_ROOT {
                        work.push(e.hash);
                    }
                }
            }
            Ok(super::object::Object::Chunked { chunks, .. }) => {
                for h in chunks {
                    if h != paths::EMPTY_ROOT { work.push(h); }
                }
            }
            Ok(super::object::Object::Blob(_)) | Err(_) => {}
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
            Err(super::types::FsError::ObjectNotFound) => {}
            Err(e) => return Err(Error::Storage(e)),
        }
    }

    // Drain accumulated TRIM-pending ranges in one batch. Removed from
    // the per-commit hot path for IOPS reasons; piggy-backing on `gc`
    // means the SSD's FTL gets the free-space update at the same time
    // we'd typically be running maintenance anyway.
    storage::trim().map_err(Error::Storage)?;

    Ok(GcStats { kept: reachable.len(), removed })
}

// ── Streaming writer (chunked blobs) ─────────────────────────────────
//
// For huge inputs (a multi-GB download, a movie, an ISO) buffering
// the whole payload in a single `Vec<u8>` would blow the kernel heap
// budget — and AES-GCM is one-shot in the audited `aes-gcm` crate, so
// the encrypt step doubles the residency on top. The streaming writer
// avoids that: it accumulates an in-RAM chunk buffer (default 16 MiB),
// flushes it as a regular content-addressed `Blob` once full, and at
// `finish` emits an `Object::Chunked` manifest pointing at all the
// chunk hashes. Peak download RAM is therefore one chunk + manifest
// overhead, independent of total size.
//
// Reads are transparent: `fs::read` detects `Object::Chunked` and
// stitches the chunks back into one `Vec<u8>` for consumers. A future
// streaming-reader variant can iterate the manifest without
// materializing the full payload, but is out of scope here — the
// write-side fix is what unblocks `http <huge-url> > path` and
// >100 MB OTA bundles today.

/// Default streaming-write chunk size. 16 MiB balances:
///  - NVMe efficiency (single extent typically covers a 16 MiB chunk)
///  - Heap peak (16 MiB plaintext + ~16 B AES-GCM tag during encrypt)
///  - Manifest overhead (1 chunk hash per 16 MiB → 32 bytes / 16 MiB
///    ≈ 2 ppm of disk overhead on top of the data itself)
pub const STREAMING_CHUNK_SIZE: usize = 16 * 1024 * 1024;

/// Stitch a `Chunked` manifest back into a single `Vec<u8>`. Used by
/// `read_with_hash` so callers see a transparent file regardless of
/// whether it was stored as a single `Blob` or as a chunked blob.
fn stitch_chunked(total_size: u64, chunks: &[[u8; 32]]) -> Result<Vec<u8>, PathError> {
    let total = total_size as usize;
    let mut out = Vec::with_capacity(total);
    for h in chunks {
        let bytes = storage::get(h)
            .map_err(Error::Storage)?
            .ok_or(PathError::Corrupt)?;
        let chunk = super::object::decode_blob_inplace(bytes)
            .map_err(|_| PathError::Corrupt)?;
        out.extend_from_slice(&chunk);
        if out.len() > total {
            return Err(PathError::Corrupt);
        }
    }
    if out.len() != total {
        return Err(PathError::Corrupt);
    }
    Ok(out)
}

/// Append-only streaming writer for a single file. Created via
/// [`open_streaming_write`]; feed it with [`StreamingWriter::write`]
/// chunks (any size) and call [`StreamingWriter::finish`] when done.
///
/// On drop without `finish`, any already-flushed chunk blobs leak
/// into storage and get reclaimed by the next `gc()` cycle — the
/// path tree is only updated atomically at `finish`. This is the
/// failure mode we want: a partial download never appears as a
/// half-written file.
pub struct StreamingWriter {
    path: alloc::string::String,
    chunk_size: usize,
    buf: Vec<u8>,
    chunk_hashes: Vec<[u8; 32]>,
    written: u64,
}

impl StreamingWriter {
    /// Bytes already written across all flushed chunks.
    pub fn written(&self) -> u64 { self.written + self.buf.len() as u64 }

    /// Append data. May flush one or more chunks synchronously when
    /// the buffer fills.
    pub fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        let mut remaining = data;
        while !remaining.is_empty() {
            let take = core::cmp::min(self.chunk_size - self.buf.len(), remaining.len());
            self.buf.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            if self.buf.len() >= self.chunk_size {
                self.flush_chunk()?;
            }
        }
        Ok(())
    }

    fn flush_chunk(&mut self) -> Result<(), Error> {
        if self.buf.is_empty() { return Ok(()); }
        let chunk_len = self.buf.len();
        // Move the chunk into an `Object::Blob`, encode + store. We
        // take(buf) so the next chunk reuses the same allocation
        // (Vec::take + Vec::with_capacity).
        let bytes = core::mem::replace(&mut self.buf, Vec::with_capacity(self.chunk_size));
        let blob = super::object::Object::Blob(bytes);
        let (encoded, hash) = blob.encode_and_hash().map_err(|_| PathError::Corrupt)?;
        if !storage::has(&hash) {
            storage::put(&hash, &encoded, /* encrypt */ true)?;
        }
        self.chunk_hashes.push(hash);
        self.written += chunk_len as u64;
        Ok(())
    }

    /// Flush the final chunk, write the manifest, and atomically
    /// publish the file at the configured path. Returns the total
    /// number of bytes written.
    pub fn finish(mut self) -> Result<u64, Error> {
        self.flush_chunk()?;
        let total_size = self.written;
        let manifest = super::object::Object::Chunked {
            total_size,
            chunks: self.chunk_hashes,
        };
        let (encoded, manifest_hash) =
            manifest.encode_and_hash().map_err(|_| PathError::Corrupt)?;
        // Manifest carries the chunk hashes in cleartext — it doesn't
        // hurt to encrypt it for uniformity with regular blobs, and
        // dedup still works because the manifest itself is
        // content-addressed.
        if !storage::has(&manifest_hash) {
            storage::put(&manifest_hash, &encoded, /* encrypt */ true)?;
        }

        let _g = ROOT_MUTEX.lock();
        let cur = current_root()?;
        let new_root = paths::install_chunked_file(&cur, &self.path, manifest_hash, total_size)?;
        commit(new_root)?;
        Ok(total_size)
    }
}

/// Open a streaming write to `path`. Parent dir must exist. If `path`
/// already names a File it's replaced atomically at `finish`; if it
/// names a Dir, `finish` errors with `AlreadyExists`.
pub fn open_streaming_write(path: &str) -> StreamingWriter {
    StreamingWriter {
        path: alloc::string::String::from(path),
        chunk_size: STREAMING_CHUNK_SIZE,
        buf: Vec::with_capacity(STREAMING_CHUNK_SIZE),
        chunk_hashes: Vec::new(),
        written: 0,
    }
}
