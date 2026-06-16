//! High-level filesystem API.
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

    if super::FS_PERF_LOG && encoded_len >= 256 * 1024 {
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

    if super::FS_PERF_LOG && data.len() >= 256 * 1024 {
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
    /// True if GC backed off without sweeping because a streaming write
    /// was in flight (its uncommitted chunks would look like orphans).
    pub skipped: bool,
}

// ── Streaming-write activity (GC race guard) ──────────────────────────
//
// A `StreamingWriter` flushes chunk Blobs via `storage::put`, which
// updates the in-memory B-tree root WITHOUT committing (only `finish`
// commits, under ROOT_MUTEX). So mid-stream, the flushed chunks are
// visible to GC's orphan enumeration yet unreachable from any committed
// root — GC would delete the in-progress download's data. This counter
// lets GC detect an in-flight stream and back off. Incremented when a
// writer is constructed, decremented on its Drop (finish OR abort), so
// it's balanced regardless of how the writer ends.
static ACTIVE_STREAMS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// True while ≥1 `StreamingWriter` is alive (open, not yet finished/dropped).
pub fn stream_active() -> bool {
    ACTIVE_STREAMS.load(core::sync::atomic::Ordering::Acquire) > 0
}
fn stream_begin() { ACTIVE_STREAMS.fetch_add(1, core::sync::atomic::Ordering::Release); }
fn stream_end()   { ACTIVE_STREAMS.fetch_sub(1, core::sync::atomic::Ordering::Release); }

// ── GC pressure counter (auto-GC trigger) ─────────────────────────────
//
// Every committed mutation orphans something: an overwrite orphans the
// old blob, a delete orphans the entry's blob, and *any* write orphans
// the old ancestor Tree objects it rebuilt (content-addressed COW). So a
// commit count is a good proxy for "orphans waiting to be reclaimed" —
// the Git `gc --auto` model (threshold on loose objects). `commit_root`
// bumps it on each real commit; the idle trigger in the shell run-loop
// fires `gc()` once it crosses `GC_PRESSURE_THRESHOLD`, and `gc()` resets
// it on a successful sweep. GC's own sweep deletes go through the
// storage-internal commit (not `commit_root`), so they don't self-feed.
static MUTATIONS_SINCE_GC: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Mutations to accumulate before the idle trigger runs GC. ~Git's
/// loose-object threshold, scaled down: enough churn to be worth a sweep,
/// low enough that orphans never pile up unbounded on an active disk.
pub const GC_PRESSURE_THRESHOLD: usize = 128;

/// Note a committed mutation (called from `storage::commit_root`).
pub(crate) fn note_commit() {
    MUTATIONS_SINCE_GC.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Mutations committed since the last successful GC.
pub fn gc_pressure() -> usize {
    MUTATIONS_SINCE_GC.load(core::sync::atomic::Ordering::Relaxed)
}

/// Read a Tree object for GC marking. `Ok(Some(entries))` for a Tree,
/// `Ok(None)` for a dangling hash or a hash that isn't a Tree (skip),
/// `Err(())` after logging an unreadable/out-of-range object (skip).
fn read_tree_for_gc(hash: &[u8; 32]) -> Result<Option<Vec<TreeEntry>>, ()> {
    let bytes = match storage::get(hash) {
        Ok(Some(b)) => b,
        Ok(None) => return Ok(None),
        Err(e) => {
            crate::kprintln!("[npk] gc: unreadable tree {:02x}{:02x}.. ({:?}) — skipping",
                hash[0], hash[1], e);
            return Err(());
        }
    };
    match super::object::Object::decode(&bytes) {
        Ok(super::object::Object::Tree(entries)) => Ok(Some(entries)),
        // A root/Dir hash that decodes to a non-Tree is corruption — skip.
        Ok(_) | Err(_) => Ok(None),
    }
}

/// Mark a `File` object's transitive chunks reachable. Only called for
/// Chunked manifests (and legacy flag==0 entries): reads the object and,
/// if it's a `Chunked` manifest, marks every chunk hash. A single Blob
/// here (legacy path) is just read + discarded. Single-Blob files with
/// `FLAG_BLOB` never reach this.
///
/// Returns `false` if the object (manifest) could not be read — its chunks
/// are then UNKNOWN, so the caller must mark the whole GC walk incomplete and
/// skip the sweep. A corrupt manifest's chunks are NOT orphans: the live
/// committed file still references them, so freeing them corrupts the fs.
#[must_use]
fn mark_file_object(hash: &[u8; 32], reachable: &mut hashbrown::HashSet<[u8; 32]>) -> bool {
    let bytes = match storage::get(hash) {
        Ok(Some(b)) => b,
        Ok(None) => return true, // genuinely absent → nothing to mark, not an error
        Err(e) => {
            crate::kprintln!("[npk] gc: unreadable file obj {:02x}{:02x}.. ({:?}) — mark incomplete",
                hash[0], hash[1], e);
            return false;
        }
    };
    match super::object::Object::decode(&bytes) {
        Ok(super::object::Object::Chunked { chunks, .. }) => {
            for c in chunks {
                if c != paths::EMPTY_ROOT { reachable.insert(c); }
            }
            true
        }
        // A legacy single Blob decodes to non-Chunked — fine, it's a leaf.
        Ok(_) => true,
        // Decode failure on an object we could read = corruption; its chunk
        // refs are unknown → incomplete, don't risk sweeping them.
        Err(_) => false,
    }
}

/// Mark-and-sweep GC over the object store.
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

    // Race guard: never sweep while a streaming write is in flight — its
    // flushed-but-uncommitted chunks are visible to the orphan
    // enumeration but unreachable from any committed root, so we'd delete
    // the in-progress download. We hold ROOT_MUTEX, which blocks
    // `StreamingWriter::finish` (it also takes ROOT_MUTEX to commit), so a
    // stream cannot commit mid-GC; thus any stream that touches the store
    // during our run stays alive (counter > 0) and is caught here or at
    // the post-snapshot re-check below.
    if stream_active() {
        crate::kprintln!("[npk] gc: skipped — streaming write in progress");
        return Ok(GcStats { kept: 0, removed: 0, skipped: true });
    }

    let roots = storage::all_root_hashes().map_err(Error::Storage)?;
    let mut reachable: HashSet<[u8; 32]> = HashSet::new();
    // Work-list holds Tree hashes ONLY (roots + Dir entries). File
    // entries are marked reachable straight from their parent Tree's
    // `kind` + `flags` — so GC never reads a single-Blob file's body
    // (the bulk of the disk). Only Trees and Chunked manifests are read.
    // This is what makes GC cheap enough to run automatically.
    let mut tree_work: alloc::vec::Vec<[u8; 32]> = roots;

    // Whether the reachability walk read EVERY object it needed. If any tree
    // or manifest came back missing/unreadable/corrupt, the set is INCOMPLETE
    // and we must NOT sweep: the unread subtree's objects would look like
    // orphans and we'd free blocks the committed root still references —
    // turning a localized corruption into an unmountable filesystem (boots to
    // installer). Leaking orphan blocks until the fs is healthy is the safe
    // failure mode; deleting live data is not.
    let mut mark_incomplete = false;

    while let Some(hash) = tree_work.pop() {
        if hash == paths::EMPTY_ROOT { continue; }
        if !reachable.insert(hash) { continue; }

        let entries = match read_tree_for_gc(&hash) {
            Ok(Some(e)) => e,
            // A hash we reached from a root/Dir must resolve to a Tree. Missing
            // (Ok(None)), non-Tree, or unreadable (Err) all mean this subtree's
            // children are UNKNOWN → the mark is incomplete → skip the sweep.
            Ok(None) | Err(()) => { mark_incomplete = true; continue; }
        };
        for e in entries {
            if e.hash == paths::EMPTY_ROOT { continue; }
            match e.kind {
                EntryKind::Dir => tree_work.push(e.hash),
                EntryKind::File => {
                    // Mark the file object itself reachable WITHOUT reading it.
                    reachable.insert(e.hash);
                    let chunked = e.flags & super::object::TreeEntry::FLAG_CHUNKED != 0;
                    let known_blob = e.flags & super::object::TreeEntry::FLAG_BLOB != 0;
                    // Chunked manifest → read it for the chunk hashes.
                    // Legacy entries (flags==0) fall here too: read to
                    // classify, so a pre-flag chunked file's chunks are
                    // never mistaken for orphans. Single-Blob files
                    // (FLAG_BLOB) are leaves and skip the read entirely.
                    if chunked || !known_blob {
                        if !mark_file_object(&e.hash, &mut reachable) {
                            mark_incomplete = true;
                        }
                    }
                }
            }
        }
    }

    // Never sweep on an incomplete mark — see `mark_incomplete` above.
    if mark_incomplete {
        crate::kprintln!(
            "[npk] gc: mark incomplete (unreadable/corrupt objects) — \
             skipping sweep to avoid freeing live data. Filesystem needs \
             a check / reinstall."
        );
        return Ok(GcStats { kept: reachable.len(), removed: 0, skipped: true });
    }

    let all = storage::all_object_hashes().map_err(Error::Storage)?;

    // Re-check: a stream may have opened during the mark phase and flushed
    // chunks that landed in `all` but were marked after our reachability
    // walk. `finish` is blocked on ROOT_MUTEX (we hold it), so such a
    // stream is still alive (counter > 0) — back off before sweeping its
    // chunks. If the counter is zero here, no stream touched the store
    // across the whole start→snapshot window, so `all` reflects only
    // committed state and is safe to sweep.
    if stream_active() {
        crate::kprintln!("[npk] gc: aborted — streaming write started mid-sweep");
        return Ok(GcStats { kept: reachable.len(), removed: 0, skipped: true });
    }

    let mut removed = 0usize;
    let mut failed = 0usize;
    for h in all {
        if reachable.contains(&h) { continue; }
        // remove returns ObjectNotFound only if the entry vanished
        // between our enumeration and the delete — impossible under
        // ROOT_MUTEX, but tolerate it defensively.
        match storage::remove(&h) {
            Ok(()) => removed += 1,
            Err(super::types::FsError::ObjectNotFound) => {}
            // A corrupt extent / indirect pointer can make remove fail
            // (out-of-range read). Log + skip so the remaining orphans
            // still get reclaimed instead of one bad object wedging GC.
            Err(e) => {
                crate::kprintln!("[npk] gc: failed to remove {:02x}{:02x}.. ({:?}) — skipping",
                    h[0], h[1], e);
                failed += 1;
            }
        }
    }
    if failed > 0 {
        crate::kprintln!("[npk] gc: {} orphan(s) could not be removed (corrupt) — left in place", failed);
    }

    // Drain accumulated TRIM-pending ranges in one batch. Removed from
    // the per-commit hot path for IOPS reasons; piggy-backing on `gc`
    // means the SSD's FTL gets the free-space update at the same time
    // we'd typically be running maintenance anyway.
    storage::trim().map_err(Error::Storage)?;

    // Reset pressure — this sweep covered everything orphaned so far.
    MUTATIONS_SINCE_GC.store(0, core::sync::atomic::Ordering::Relaxed);

    // Record this run so `disk` can report when GC last completed and
    // whether it was clean (no corrupt objects skipped).
    *LAST_GC.lock() = Some(LastGc {
        when_unix: crate::drivers::rtc::read_unix_time().unwrap_or(0),
        kept: reachable.len(),
        removed,
        failed,
    });

    Ok(GcStats { kept: reachable.len(), removed, skipped: false })
}

/// Summary of the most recent `gc()` completion (this session only —
/// resets on reboot). `failed == 0` means a clean sweep.
#[derive(Clone, Copy, Debug)]
pub struct LastGc {
    /// UTC seconds since epoch at completion (0 if RTC unreadable).
    pub when_unix: u64,
    pub kept: usize,
    pub removed: usize,
    /// Orphans that couldn't be removed (corrupt) — left in place.
    pub failed: usize,
}

static LAST_GC: Mutex<Option<LastGc>> = Mutex::new(None);

/// The most recent `gc()` result, or `None` if GC hasn't run since boot.
pub fn last_gc() -> Option<LastGc> { *LAST_GC.lock() }

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

/// Default streaming-write chunk size. Kept at 1 MiB because each chunk is
/// hashed in one shot with `blake3::hash(&chunk)`, and BLAKE3's one-shot path
/// (`compress_subtree_wide`) recurses divide-and-conquer over the whole buffer
/// with per-level on-stack arrays. At 16 MiB that recursion (depth ~11) blew
/// the kernel/task stack → smashed return addresses → wild RIP/RSP crash
/// (observed on the 16 GB notebook: RSP ran past physical RAM, #PF in
/// blake3_hash_many). 1 MiB keeps the recursion shallow (~depth 7, a few KB of
/// stack). Manifest overhead is still negligible (32 B per 1 MiB). Reads stitch
/// chunks transparently, so existing 16 MiB-chunk objects stay readable.
pub const STREAMING_CHUNK_SIZE: usize = 1024 * 1024;

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
            // `take` rather than move: StreamingWriter now has a Drop impl
            // (the GC race-guard decrement), and you can't move a field
            // out of a Drop type. Leaves an empty Vec for Drop to reap.
            chunks: core::mem::take(&mut self.chunk_hashes),
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
    // Mark a stream in flight so GC backs off until `finish`/drop. Done
    // here (at construction) so BOTH the public wrapper and the direct
    // 9p caller are covered. Balanced by `Drop`.
    stream_begin();
    StreamingWriter {
        path: alloc::string::String::from(path),
        chunk_size: STREAMING_CHUNK_SIZE,
        buf: Vec::with_capacity(STREAMING_CHUNK_SIZE),
        chunk_hashes: Vec::new(),
        written: 0,
    }
}

impl Drop for StreamingWriter {
    /// Clear the GC race guard whether the writer finished or was
    /// abandoned (a dropped-without-finish writer leaks its flushed
    /// chunks until the next GC — which is now free to run again).
    fn drop(&mut self) {
        stream_end();
    }
}
