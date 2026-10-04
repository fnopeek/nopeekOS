# `kernel/src/storage/npkfs/fs.rs` @ 5e0102684

## L1-13 · `#![allow(dead_code)]`

```
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
```

## L25-27 · `pub use super::paths::PathError as Error;`

```
/// Public error: a path-layer error or a storage-layer error.
/// Wire-shape choice — most callers care about "is the file there?"
/// more than "did postcard fail?", so we expose `PathError` directly.
```

## L30-32 · `static ROOT_MUTEX: Mutex<()> = Mutex::new(());`

```
/// Serializes root-mutating calls. Held only across `current_root +
/// paths::* + commit_root`. Storage layer takes its own (separate)
/// FS lock per `put`, so this isn't reentrant against itself.
```

## L39-41 · `pub mtime: u64,`

```
/// UTC seconds since the Unix epoch, captured at write time.
/// Zero means "unknown" (RTC was unreadable when the entry was
/// created). Always zero for the root tree.
```

## L53 · `pub fn stat(path: &str) -> Result<Option<StatResult>, Error> {`

```
// ── Reads (no SB mutation) ────────────────────────────────────────────
```

## L55-57 · `pub fn stat(path: &str) -> Result<Option<StatResult>, Error> {`

```
/// Look up `path` and return whether it exists, what kind, and its size.
/// `Ok(None)` for missing paths. Errors only for genuine FS issues
/// (NotADirectory mid-path, decode failures, NotMounted).
```

## L69-70 · `pub fn exists(path: &str) -> Result<bool, Error> {`

```
/// True iff `path` resolves to a File or Dir. Equivalent to `stat().is_some()`
/// but handy for booleans where the caller doesn't care about the kind.
```

## L75-77 · `pub fn read(path: &str) -> Result<Option<Vec<u8>>, Error> {`

```
/// Read a File's bytes. Returns `Ok(None)` if missing. Errors with
/// `NotADirectory` when used on a directory (intentional asymmetry —
/// directories aren't readable as Blobs; use `list`).
```

## L82-88 · `pub fn read_with_hash(path: &str) -> Result<Option<(Vec<u8>, [u8; 32])>, Error> {`

```
/// Same as [`read`] but also returns the file's content-address (the
/// walk hash from the tree, which is BLAKE3 of the encoded Blob). Used
/// by the v1-compat bridge to avoid recomputing a fresh BLAKE3 over
/// every read — `storage::get` already verifies integrity against the
/// walk hash before handing the plaintext back, so re-hashing is pure
/// overhead. On a 1 MB read with AVX2 BLAKE3, the redundant hash costs
/// ~0.6 ms — measurable in the testdisk huge-file numbers.
```

## L110-113 · `let blob = if bytes.first() == Some(&0) {`

```
// Fast path: in-place blob decoder (saves ~0.9 ms / MB vs full
// postcard decode). Falls back to a full decode if the variant
// tag isn't `Blob(0)` — typically `Chunked(2)` for files written
// via the streaming writer.
```

## L141-142 · `pub fn list(path: &str) -> Result<Option<Vec<TreeEntry>>, Error> {`

```
/// List the entries directly under `path`. `path` must resolve to a Dir.
/// Returns `Ok(None)` if `path` is missing.
```

## L152 · `pub fn write(path: &str, data: &[u8]) -> Result<(), Error> {`

```
// ── Mutations (commit a new root via SB) ──────────────────────────────
```

## L154-155 · `pub fn write(path: &str, data: &[u8]) -> Result<(), Error> {`

```
/// Write `data` to `path` (creating or overwriting a File). Parent must
/// exist; if `path` exists as a Dir, errors with AlreadyExists.
```

## L183-184 · `pub fn mkdir(path: &str) -> Result<(), Error> {`

```
/// Create an empty directory at `path`. Parent must exist; `path` must
/// not.
```

## L192-194 · `pub fn ensure_dir(path: &str) -> Result<(), Error> {`

```
/// Idempotent variant: returns Ok if `path` already exists as a Dir,
/// errors with AlreadyExists only if it's a File. Useful for installer
/// + setup paths that may run more than once.
```

## L208-209 · `pub fn delete(path: &str) -> Result<(), Error> {`

```
/// Remove `path`. Files: dropped unconditionally. Dirs: must be empty.
/// Idempotent on missing — `Ok(())` if `path` already absent.
```

## L224 · `pub fn rename(old: &str, new: &str) -> Result<(), Error> {`

```
/// Move `old` to `new`. Cross-parent supported. `new` must not exist.
```

## L237-238 · `pub fn copy(old: &str, new: &str) -> Result<(), Error> {`

```
/// Copy `old` to `new`. Content-addressed alias (see `paths::copy`), so
/// even a whole directory is an O(1) subtree share. `new` must not exist.
```

## L248 · `pub fn ensure_dirs(path: &str) -> Result<(), Error> {`

```
// ── Convenience: ensure a chain of dirs (mkdir -p) ────────────────────
```

## L250-252 · `pub fn ensure_dirs(path: &str) -> Result<(), Error> {`

```
/// Ensure every parent dir of `path` exists. `path` itself is treated
/// as a directory chain — call as `ensure_dirs("a/b/c")` to create
/// `a`, `a/b`, `a/b/c`. Idempotent.
```

## L266 · `#[derive(Clone, Copy, Debug)]`

```
// ── Garbage collection ────────────────────────────────────────────────
```

## L270 · `pub kept: usize,`

```
/// Number of objects reachable from any live SB slot.
```

## L272 · `pub removed: usize,`

```
/// Number of objects removed because no SB referenced them.
```

## L274-275 · `pub skipped: bool,`

```
/// True if GC backed off without sweeping because a streaming write
/// was in flight (its uncommitted chunks would look like orphans).
```

## L279-288 · `static ACTIVE_STREAMS: core::sync::atomic::AtomicUsize =`

```
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
```

## L292 · `pub fn stream_active() -> bool {`

```
/// True while ≥1 `StreamingWriter` is alive (open, not yet finished/dropped).
```

## L299-309 · `static MUTATIONS_SINCE_GC: core::sync::atomic::AtomicUsize =`

```
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
```

## L313-315 · `pub const GC_PRESSURE_THRESHOLD: usize = 128;`

```
/// Mutations to accumulate before the idle trigger runs GC. ~Git's
/// loose-object threshold, scaled down: enough churn to be worth a sweep,
/// low enough that orphans never pile up unbounded on an active disk.
```

## L318 · `pub(crate) fn note_commit() {`

```
/// Note a committed mutation (called from `storage::commit_root`).
```

## L323 · `pub fn gc_pressure() -> usize {`

```
/// Mutations committed since the last successful GC.
```

## L328-330 · `fn read_tree_for_gc(hash: &[u8; 32]) -> Result<Option<Vec<TreeEntry>>, ()> {`

```
/// Read a Tree object for GC marking. `Ok(Some(entries))` for a Tree,
/// `Ok(None)` for a dangling hash or a hash that isn't a Tree (skip),
/// `Err(())` after logging an unreadable/out-of-range object (skip).
```

## L343 · `Ok(_) | Err(_) => Ok(None),`

```
// A root/Dir hash that decodes to a non-Tree is corruption — skip.
```

## L348-357 · `#[must_use]`

```
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
```

## L362 · `Ok(None) => return true, // genuinely absent → nothing to mark, not an error`

```
// genuinely absent → nothing to mark, not an error
```

## L376 · `Ok(_) => true,`

```
// A legacy single Blob decodes to non-Chunked — fine, it's a leaf.
```

## L378-379 · `Err(_) => false,`

```
// Decode failure on an object we could read = corruption; its chunk
// refs are unknown → incomplete, don't risk sweeping them.
```

## L384-393 · `pub fn gc() -> Result<GcStats, Error> {`

```
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
```

## L399-406 · `if stream_active() {`

```
// Race guard: never sweep while a streaming write is in flight — its
// flushed-but-uncommitted chunks are visible to the orphan
// enumeration but unreachable from any committed root, so we'd delete
// the in-progress download. We hold ROOT_MUTEX, which blocks
// `StreamingWriter::finish` (it also takes ROOT_MUTEX to commit), so a
// stream cannot commit mid-GC; thus any stream that touches the store
// during our run stays alive (counter > 0) and is caught here or at
// the post-snapshot re-check below.
```

## L414-418 · `let mut tree_work: alloc::vec::Vec<[u8; 32]> = roots;`

```
// Work-list holds Tree hashes ONLY (roots + Dir entries). File
// entries are marked reachable straight from their parent Tree's
// `kind` + `flags` — so GC never reads a single-Blob file's body
// (the bulk of the disk). Only Trees and Chunked manifests are read.
// This is what makes GC cheap enough to run automatically.
```

## L421-427 · `let mut mark_incomplete = false;`

```
// Whether the reachability walk read EVERY object it needed. If any tree
// or manifest came back missing/unreadable/corrupt, the set is INCOMPLETE
// and we must NOT sweep: the unread subtree's objects would look like
// orphans and we'd free blocks the committed root still references —
// turning a localized corruption into an unmountable filesystem (boots to
// installer). Leaking orphan blocks until the fs is healthy is the safe
// failure mode; deleting live data is not.
```

## L436-438 · `Ok(None) | Err(()) => { mark_incomplete = true; continue; }`

```
// A hash we reached from a root/Dir must resolve to a Tree. Missing
// (Ok(None)), non-Tree, or unreadable (Err) all mean this subtree's
// children are UNKNOWN → the mark is incomplete → skip the sweep.
```

## L446 · `reachable.insert(e.hash);`

```
// Mark the file object itself reachable WITHOUT reading it.
```

## L450-454 · `if chunked || !known_blob {`

```
// Chunked manifest → read it for the chunk hashes.
// Legacy entries (flags==0) fall here too: read to
// classify, so a pre-flag chunked file's chunks are
// never mistaken for orphans. Single-Blob files
// (FLAG_BLOB) are leaves and skip the read entirely.
```

## L465 · `if mark_incomplete {`

```
// Never sweep on an incomplete mark — see `mark_incomplete` above.
```

## L477-483 · `if stream_active() {`

```
// Re-check: a stream may have opened during the mark phase and flushed
// chunks that landed in `all` but were marked after our reachability
// walk. `finish` is blocked on ROOT_MUTEX (we hold it), so such a
// stream is still alive (counter > 0) — back off before sweeping its
// chunks. If the counter is zero here, no stream touched the store
// across the whole start→snapshot window, so `all` reflects only
// committed state and is safe to sweep.
```

## L493-495 · `match storage::remove(&h) {`

```
// remove returns ObjectNotFound only if the entry vanished
// between our enumeration and the delete — impossible under
// ROOT_MUTEX, but tolerate it defensively.
```

## L499-501 · `Err(e) => {`

```
// A corrupt extent / indirect pointer can make remove fail
// (out-of-range read). Log + skip so the remaining orphans
// still get reclaimed instead of one bad object wedging GC.
```

## L513-516 · `storage::trim().map_err(Error::Storage)?;`

```
// Drain accumulated TRIM-pending ranges in one batch. Removed from
// the per-commit hot path for IOPS reasons; piggy-backing on `gc`
// means the SSD's FTL gets the free-space update at the same time
// we'd typically be running maintenance anyway.
```

## L519 · `MUTATIONS_SINCE_GC.store(0, core::sync::atomic::Ordering::Relaxed);`

```
// Reset pressure — this sweep covered everything orphaned so far.
```

## L522-523 · `when_unix: crate::drivers::rtc::read_unix_time().unwrap_or(0),`

```
// Record this run so `disk` can report when GC last completed and
// whether it was clean (no corrupt objects skipped).
```

## L534-535 · `#[derive(Clone, Copy, Debug)]`

```
/// Summary of the most recent `gc()` completion (this session only —
/// resets on reboot). `failed == 0` means a clean sweep.
```

## L538 · `pub when_unix: u64,`

```
/// UTC seconds since epoch at completion (0 if RTC unreadable).
```

## L542 · `pub failed: usize,`

```
/// Orphans that couldn't be removed (corrupt) — left in place.
```

## L548 · `pub fn last_gc() -> Option<LastGc> { *LAST_GC.lock() }`

```
/// The most recent `gc()` result, or `None` if GC hasn't run since boot.
```

## L551-568 · `pub const STREAMING_CHUNK_SIZE: usize = 1024 * 1024;`

```
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
```

## L570-578 · `pub const STREAMING_CHUNK_SIZE: usize = 1024 * 1024;`

```
/// Default streaming-write chunk size. Kept at 1 MiB because each chunk is
/// hashed in one shot with `blake3::hash(&chunk)`, and BLAKE3's one-shot path
/// (`compress_subtree_wide`) recurses divide-and-conquer over the whole buffer
/// with per-level on-stack arrays. At 16 MiB that recursion (depth ~11) blew
/// the kernel/task stack → smashed return addresses → wild RIP/RSP crash
/// (observed on the 16 GB notebook: RSP ran past physical RAM, #PF in
/// blake3_hash_many). 1 MiB keeps the recursion shallow (~depth 7, a few KB of
/// stack). Manifest overhead is still negligible (32 B per 1 MiB). Reads stitch
/// chunks transparently, so existing 16 MiB-chunk objects stay readable.
```

## L581-583 · `fn stitch_chunked(total_size: u64, chunks: &[[u8; 32]]) -> Result<Vec<u8>, PathError> {`

```
/// Stitch a `Chunked` manifest back into a single `Vec<u8>`. Used by
/// `read_with_hash` so callers see a transparent file regardless of
/// whether it was stored as a single `Blob` or as a chunked blob.
```

## L604-613 · `const STREAM_COMMIT_EVERY: usize = 16;`

```
/// Append-only streaming writer for a single file. Created via
/// [`open_streaming_write`]; feed it with [`StreamingWriter::write`]
/// chunks (any size) and call [`StreamingWriter::finish`] when done.
///
/// On drop without `finish`, any already-flushed chunk blobs leak
/// into storage and get reclaimed by the next `gc()` cycle — the
/// path tree is only updated atomically at `finish`. This is the
/// failure mode we want: a partial download never appears as a
/// half-written file.
/// Chunks between deferred-batch flushes during a streaming write.
```

## L625 · `pub fn written(&self) -> u64 { self.written + self.buf.len() as u64 }`

```
/// Bytes already written across all flushed chunks.
```

## L628-629 · `pub fn write(&mut self, data: &[u8]) -> Result<(), Error> {`

```
/// Append data. May flush one or more chunks synchronously when
/// the buffer fills.
```

## L646-648 · `let bytes = core::mem::replace(&mut self.buf, Vec::with_capacity(self.chunk_size));`

```
// Move the chunk into an `Object::Blob`, encode + store. We
// take(buf) so the next chunk reuses the same allocation
// (Vec::take + Vec::with_capacity).
```

## L653 · `storage::put(&hash, &encoded, /* encrypt */ true)?;`

```
/* encrypt */
```

## L657-664 · `if self.chunk_hashes.len() % STREAM_COMMIT_EVERY == 0 {`

```
// Drain the deferred batch every so often. Without this a 249 MB
// download runs its ENTIRE length with the bitmap, the object btree
// and `pending_old_blocks` uncommitted — and the first unrelated
// writer to commit (over WiFi that is wifid's log, constantly) takes
// that whole batch with it. Committing here costs one four-phase
// flush per `STREAM_COMMIT_EVERY` chunks, which is noise next to the
// transfer itself, and it does NOT publish the file: the path tree is
// still only touched by `finish`.
```

## L671-673 · `pub fn finish(mut self) -> Result<u64, Error> {`

```
/// Flush the final chunk, write the manifest, and atomically
/// publish the file at the configured path. Returns the total
/// number of bytes written.
```

## L679-681 · `chunks: core::mem::take(&mut self.chunk_hashes),`

```
// `take` rather than move: StreamingWriter now has a Drop impl
// (the GC race-guard decrement), and you can't move a field
// out of a Drop type. Leaves an empty Vec for Drop to reap.
```

## L686-689 · `if !storage::has(&manifest_hash) {`

```
// Manifest carries the chunk hashes in cleartext — it doesn't
// hurt to encrypt it for uniformity with regular blobs, and
// dedup still works because the manifest itself is
// content-addressed.
```

## L691 · `storage::put(&manifest_hash, &encoded, /* encrypt */ true)?;`

```
/* encrypt */
```

## L702-704 · `pub fn open_streaming_write(path: &str) -> StreamingWriter {`

```
/// Open a streaming write to `path`. Parent dir must exist. If `path`
/// already names a File it's replaced atomically at `finish`; if it
/// names a Dir, `finish` errors with `AlreadyExists`.
```

## L706-708 · `stream_begin();`

```
// Mark a stream in flight so GC backs off until `finish`/drop. Done
// here (at construction) so BOTH the public wrapper and the direct
// 9p caller are covered. Balanced by `Drop`.
```

## L720-722 · `fn drop(&mut self) {`

```
/// Clear the GC race guard whether the writer finished or was
/// abandoned (a dropped-without-finish writer leaks its flushed
/// chunks until the next GC — which is now free to run again).
```

