# `kernel/src/storage/npkfs/mod.rs` @ 5e0102684

## L1-22 · `mod types;`

```
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
```

## L39-44 · `pub(crate) const FS_PERF_LOG: bool = false;`

```
/// Per-operation perf-timing logs (`[put]`/`[get]`/`[fs::read]`/
/// `[fs::write]` with µs breakdowns). Profiling instrumentation for
/// npkFS throughput-tuning sessions — off by default because a large
/// streaming download flushes one 16 MiB chunk after another and
/// each would emit a `[put]` line, drowning the console. Flip to
/// `true` when actively profiling the FS.
```

## L50 · `pub fn mkfs() -> Result<(), FsError> {`

```
// ── Public surface ────────────────────────────────────────────────────
```

## L52 · `pub fn mkfs() -> Result<(), FsError> {`

```
/// Format the disk to npkFS.
```

## L57 · `pub fn mount() -> Result<(), FsError> {`

```
/// Mount the disk.
```

## L62 · `pub fn halt_for_unmountable_disk(p: &storage::SbProbe) -> ! {`

```
/// Refuse to continue on a disk that holds data we could not mount.
```

## L67 · `pub fn probe_disk() -> Result<storage::SbProbe, FsError> {`

```
/// What the superblock ring holds, without mounting or writing.
```

## L74-84 · `pub fn sync() {`

```
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
```

## L97-99 · `pub fn store(name: &str, data: &[u8], _cap_id: [u8; 32]) -> Result<[u8; 32], FsError> {`

```
/// Strict create: errors with `ObjectExists` if `name` is already present.
/// `cap_id` is accepted for ABI compat and ignored by the content-
/// addressed backend.
```

## L108 · `pub fn upsert(name: &str, data: &[u8], _cap_id: [u8; 32]) -> Result<[u8; 32], FsError> {`

```
/// Insert-or-replace.
```

## L116-120 · `pub fn fetch(name: &str) -> Result<(Vec<u8>, [u8; 32]), FsError> {`

```
/// Read an object. Returns `(plaintext, content_hash)`. The hash is
/// the walk hash from the tree (BLAKE3 of the encoded Blob); already
/// verified against the on-disk integrity by `storage::get` before
/// the bytes are handed back. We don't re-hash the plaintext — that
/// was a 0.6 ms tax per 1 MB read for no security gain.
```

## L131-145 · `pub fn open_streaming_write(name: &str) -> Result<fs::StreamingWriter, FsError> {`

```
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
```

## L156 · `pub fn delete(name: &str) -> Result<(), FsError> {`

```
/// Remove an object. Errors with `ObjectNotFound` if missing.
```

## L164-165 · `pub fn rename(old: &str, new: &str) -> Result<(), FsError> {`

```
/// Move `old` to `new` (rename / cross-directory move). `new` must not
/// already exist; `old` must. Works for files and whole directories.
```

## L176-177 · `pub fn copy(old: &str, new: &str) -> Result<(), FsError> {`

```
/// Copy `old` to `new`. Content-addressed alias — no data duplication,
/// even for a whole directory. `new` must not exist; `old` must.
```

## L188-190 · `pub fn list() -> Result<Vec<(String, u64, [u8; 32])>, FsError> {`

```
/// Flat list of every File in the tree, recursively. Format:
/// `(slash_path, byte_size, blake3_hash)`. Walks the entire root tree.
/// Acceptable until callers migrate to per-directory `fs::list(path)`.
```

## L203-204 · `pub fn validate_user_name(name: &str) -> Result<(), FsError> {`

```
/// Reject reserved names that would clash with kernel-managed paths.
/// `.system/` is reserved for boot config + keycheck.
```

## L214 · `fn clean_path(name: &str) -> &str {`

```
// ── Helpers ───────────────────────────────────────────────────────────
```

## L251-252 · `fn walk_recursive(prefix: String, out: &mut Vec<(String, u64, [u8; 32])>) -> Result<(), FsError> {`

```
/// DFS the root Tree, appending every File entry as a flat
/// `(slash_path, size, hash)` tuple. Skips `.system/` (kernel-internal).
```

## L264 · `if path == ".system" || path.starts_with(".system/") {`

```
// Don't surface kernel-internal storage to user-space listings.
```

