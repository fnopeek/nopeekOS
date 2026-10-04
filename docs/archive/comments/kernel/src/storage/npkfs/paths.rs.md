# `kernel/src/storage/npkfs/paths.rs` @ 5e0102684

## L1-17 · `#![allow(dead_code)]`

```
//! npkFS path layer — read & mutate via `(root_tree_hash, slash_path)`.
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
```

## L28-29 · `pub const EMPTY_ROOT: [u8; 32] = [0u8; 32];`

```
/// Sentinel for "the root Tree doesn't exist yet". `walk` of any path
/// against this returns NotFound; mutations create the real Tree.
```

## L34-35 · `InvalidPath,`

```
/// Path is empty where it shouldn't be, contains `.`/`..`/empty
/// segments / NUL bytes, or a name exceeds MAX_NAME_LEN.
```

## L37 · `NotFound,`

```
/// A path component is missing in its parent Tree.
```

## L39 · `NotADirectory,`

```
/// Path tried to descend through a File.
```

## L41-43 · `AlreadyExists,`

```
/// Inserting where a name is already present (and overwrite is
/// disallowed for that variant — e.g. mkdir over an existing
/// entry, or rename onto an existing target).
```

## L45 · `NotEmpty,`

```
/// `delete` / `rename`-out of a non-empty directory.
```

## L47-48 · `Corrupt,`

```
/// Encoding/decoding of a Tree object failed, or a hash that
/// should reference a Tree references a Blob (FS corruption).
```

## L61-62 · `pub size:  u64,`

```
/// File: byte size of the referenced Blob.
/// Dir : recursive sum of File sizes underneath.
```

## L64-66 · `pub mtime: u64,`

```
/// File / Dir: UTC seconds since the Unix epoch, captured at write
/// time. Zero for the root tree (no parent TreeEntry to read it
/// from) and for entries created when the RTC was unreadable.
```

## L70 · `pub fn parse_path(path: &str) -> Result<Vec<&str>, PathError> {`

```
// ── Path parsing ──────────────────────────────────────────────────────
```

## L72-73 · `pub fn parse_path(path: &str) -> Result<Vec<&str>, PathError> {`

```
/// Split a slash-separated path into validated segments. Empty input
/// (or just slashes) returns an empty Vec, which means "the root".
```

## L89 · `fn fetch_tree(hash: &[u8; 32]) -> Result<Vec<TreeEntry>, PathError> {`

```
// ── Tree fetch / store helpers ────────────────────────────────────────
```

## L91-92 · `fn fetch_tree(hash: &[u8; 32]) -> Result<Vec<TreeEntry>, PathError> {`

```
/// Read a Tree object by hash. The empty-root sentinel returns Vec::new()
/// without touching storage.
```

## L104-112 · `fn store_tree(entries: Vec<TreeEntry>) -> Result<([u8; 32], u64), PathError> {`

```
/// Encode + put a Tree object. Returns its content hash and the
/// recursive byte size (sum of `entry.size` for File + Dir entries —
/// for File these are byte sizes, for Dir already-recursive sums, so
/// the sum stays correct by induction).
///
/// `saturating_add` instead of `.sum()`: 9.2 EB is unreachable in
/// practice, but `Iterator::sum` on `u64` panics in debug and wraps
/// silently in release. Saturating is the disciplined kernel-side
/// move and costs nothing.
```

## L118-121 · `storage::put(&hash, &bytes, /* encrypt */ false)?;`

```
// Trees stay plaintext: boot-time `fs::exists` has to walk them
// before the user has logged in, so AEAD on Trees would brick the
// first-vs-subsequent-boot decision. File contents (Blobs) are
// encrypted separately in `paths::store` below.
```

## L122 · `storage::put(&hash, &bytes, /* encrypt */ false)?;`

```
/* encrypt */
```

## L126 · `pub fn walk(root: &[u8; 32], path: &str) -> Result<WalkOk, PathError> {`

```
// ── Walk ──────────────────────────────────────────────────────────────
```

## L128-132 · `pub fn walk(root: &[u8; 32], path: &str) -> Result<WalkOk, PathError> {`

```
/// Walk `path` from `root`, returning what's at the end.
///
/// - Empty path → root itself (kind=Dir, size=root's recursive size)
/// - Missing component → `NotFound`
/// - Descending through a File → `NotADirectory`
```

## L168 · `fn walk_for_mutation<'a>(`

```
// ── Mutation helpers ──────────────────────────────────────────────────
```

## L170-172 · `fn walk_for_mutation<'a>(`

```
/// Walk to the parent of a target path, returning the chain of trees
/// from root to parent (inclusive). Used by mutations to rebuild
/// bottom-up after editing the leaf.
```

## L190-191 · `fn rebuild_up(`

```
/// Replace the leaf of a `walk_for_mutation` chain and rebuild every
/// ancestor up to (and including) the new root. Returns new root hash.
```

## L224-229 · `fn insert_entry_at(`

```
/// Insert `new_entry` at `segs` (path components, last = name).
///
/// Collision policy:
///   - `name` not present → insert
///   - `name` present, kinds match, and `allow_replace` set → replace
///   - any other collision → AlreadyExists
```

## L254-256 · `fn remove_entry_at(`

```
/// Remove the entry named at `segs` and return it + the new root.
/// `require_empty_dir`: if the target is a Dir, its Tree must be empty
/// (POSIX rmdir semantics) — set to false for `rename`'s detach phase.
```

## L282 · `pub fn store(root: &[u8; 32], path: &str, data: &[u8]) -> Result<[u8; 32], PathError> {`

```
// ── Public mutations ──────────────────────────────────────────────────
```

## L284-286 · `pub fn store(root: &[u8; 32], path: &str, data: &[u8]) -> Result<[u8; 32], PathError> {`

```
/// Store `data` as a Blob at `path`. Parent must exist and be a Dir.
/// If `path` already names a File, it's replaced; if it names a Dir,
/// errors with `AlreadyExists`.
```

## L291-294 · `let blob_hash = super::object::blob_content_hash(data);`

```
// Stream-hash the would-be Blob's content address (no alloc, no
// encode) so we can skip `data.to_vec() + encode_and_hash() +
// storage::put` entirely when the blob already exists. ~1 ms/MB
// saved on dedup hits.
```

## L298 · `let blob = Object::Blob(data.to_vec());`

```
// Cache-miss path: full encode + AEAD + put.
```

## L304 · `storage::put(&full_hash, &blob_bytes, /* encrypt */ true)?;`

```
/* encrypt */
```

## L313 · `flags: TreeEntry::FLAG_BLOB,`

```
// Single Blob → leaf; lets GC skip reading the body.
```

## L316 · `insert_entry_at(root, &segs, entry, /* allow_replace */ true)`

```
/* allow_replace */
```

## L319-323 · `pub fn install_chunked_file(`

```
/// Publish a pre-built `Object::Chunked` manifest as a File at
/// `path`. Used by `fs::StreamingWriter::finish` after it has
/// `storage::put`'d every chunk and the manifest itself. Parent must
/// exist; if `path` already names a File it's replaced, if it names a
/// Dir errors with `AlreadyExists`.
```

## L339 · `flags: TreeEntry::FLAG_CHUNKED,`

```
// Chunked manifest → GC must read it to reach the chunk blobs.
```

## L342 · `insert_entry_at(root, &segs, entry, /* allow_replace */ true)`

```
/* allow_replace */
```

## L345-346 · `pub fn mkdir(root: &[u8; 32], path: &str) -> Result<[u8; 32], PathError> {`

```
/// Create an empty directory at `path`. Parent must exist; `path` must
/// not already exist.
```

## L353 · `storage::put(&hash, &bytes, /* encrypt */ false)?;`

```
// Tree → unencrypted (same reasoning as `store_tree`).
```

## L354 · `storage::put(&hash, &bytes, /* encrypt */ false)?;`

```
/* encrypt */
```

## L364 · `insert_entry_at(root, &segs, entry, /* allow_replace */ false)`

```
/* allow_replace */
```

## L367-369 · `fn now_unix() -> u64 {`

```
/// Capture current Unix time from the host RTC. Returns 0 ("unknown")
/// if the RTC isn't readable yet — safe default that the loft listing
/// renders as "—" rather than "1970-01-01".
```

## L374-375 · `pub fn delete(root: &[u8; 32], path: &str) -> Result<[u8; 32], PathError> {`

```
/// Remove `path`. Files are dropped unconditionally; directories must
/// be empty (`NotEmpty` if not).
```

## L379 · `let (_, new_root) = remove_entry_at(root, &segs, /* require_empty_dir */ true)?;`

```
/* require_empty_dir */
```

## L383-384 · `pub fn rename(root: &[u8; 32], old: &str, new: &str) -> Result<[u8; 32], PathError> {`

```
/// Move `old_path` to `new_path`. Same parent or cross-parent both work.
/// `new_path` must NOT already exist (no implicit overwrite).
```

## L394 · `if new_segs.len() > old_segs.len() && new_segs[..old_segs.len()] == *old_segs {`

```
// Refuse moving a directory into its own subtree (would create a cycle).
```

## L399-400 · `let (entry, root1) = remove_entry_at(root, &old_segs, /* require_empty_dir */ false)?;`

```
// Detach phase: dirs need NOT be empty here — we're carrying the
// subtree across, not rmdir'ing it.
```

## L401 · `let (entry, root1) = remove_entry_at(root, &old_segs, /* require_empty_dir */ false)?;`

```
/* require_empty_dir */
```

## L406 · `insert_entry_at(&root1, &new_segs, renamed, /* allow_replace */ false)`

```
/* allow_replace */
```

## L409-412 · `pub fn copy(root: &[u8; 32], old: &str, new: &str) -> Result<[u8; 32], PathError> {`

```
/// Copy `old_path` to `new_path`. Content-addressed: the copy shares the
/// source's `hash`, so a File aliases the same Blob and a Dir aliases the
/// whole subtree — O(1), no data duplication, dedup keeps both alive.
/// `new_path` must NOT already exist (no implicit overwrite).
```

## L423 · `let (parent_segs, name_seg) = old_segs.split_at(old_segs.len() - 1);`

```
// Read the source entry (no mutation — walk_for_mutation only fetches).
```

## L433 · `let mut copied = src;`

```
// Clone the entry under the new name; a copy is "new", so stamp mtime.
```

## L438 · `insert_entry_at(root, &new_segs, copied, /* allow_replace */ false)`

```
/* allow_replace */
```

## L441-442 · `pub fn list(root: &[u8; 32], path: &str) -> Result<Vec<TreeEntry>, PathError> {`

```
/// List the entries at `path`. Returns owned entries in sort order
/// (Tree objects are stored sorted by `Object::tree_sorted`).
```

