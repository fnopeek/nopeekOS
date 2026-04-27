# npkFS v2 — Real Content-Addressed Directories

**Status:** future work, target Phase 11.5 (before AI integration drops
serious load on the filesystem). Scoped on Florian's call: this is
tech debt we must repay before it cascades into worse decisions.

> "Das fliegt uns um die Ohren" — and it already does, in subtle
> ways: cwd tracking in the intent loop, breadcrumb logic in loft,
> wallpaper-path resolution, every `.dir` marker we juggle.

---

## The problem (npkFS v1, today)

npkFS v1 is **content-addressed but flat**: every object is keyed by
its full UTF-8 path string (`home/florian/pictures/wallpapers/aurora`),
stored verbatim in the COW B-tree. Directories are pure convention
— `/` is just a byte in the key, npkFS itself does not understand
hierarchy. Listing a directory is a B-tree scan + prefix filter.

To make directories visible (so empty dirs show up in `loft`,
`list`, etc.), we synthesise `.dir` marker files: write a zero-byte
key at `<path>/.dir` whenever a path needs to "exist as a directory".
Every host fn (`npk_fs_list`, `npk_fs_stat`) and every intent
(`setup_home`, `wallpaper_dir`, `ensure_parents`) carries logic to
ignore / synthesise these.

### What this costs us

1. **Storage overhead** — every key is the full path. A wallpaper
   file at `home/florian/pictures/wallpapers/aurora` carries a 41-byte
   key for a single conceptual filename `aurora`. At 1k files in a
   nested tree the redundancy is measurable. At 100k files (which
   real user data plus AI-generated content easily reaches) it's
   embarrassing — entire MBs of duplicated path prefixes.

2. **Listing is `O(N_total)`** — `npk_fs_list("home/florian/pictures")`
   has to walk the entire B-tree, filter by prefix, and synthesise
   subdirectory entries from observed children. That's a full B-tree
   scan **per `list_dir` call**. Loft re-fetches on every navigation.
   At 10k objects this is sluggish; at 100k it's a freeze.

3. **Rename / move is structurally impossible** — renaming a directory
   would require rewriting every nested key. We don't even expose a
   rename intent; it's deferred forever because the cost is
   prohibitive in the current shape.

4. **`.dir` markers are bookkeeping the FS shouldn't see** — every
   layer above the B-tree pretends they're not there: `npk_fs_list`
   filters them out, `intent::wallpaper::list_wallpapers` filters
   them out, `loft::parse_entry` would have to filter them, etc.
   They leak into encryption (each .dir is a chacha20-encrypted
   zero-byte object — non-trivial overhead per directory).

5. **Path joining is implicit and easy to get wrong** — apps build
   paths with `format!("{}/{}", parent, name)` then trust npkFS to
   parse. Off-by-one slashes, leading slashes, duplicate slashes
   (`home//florian`) all silently produce different keys. The intent
   loop's `cwd` tracking + `resolve_path` exists to paper over this
   and is itself a source of bugs.

6. **Encryption is per-key** — every path component leaks via the
   key (BLAKE3 hash of the path is in the B-tree, deterministic).
   With real trees we could use the **structural shape** of the FS as
   privacy boundary: only the root tree's hash is exposed externally,
   inner tree hashes never need to leave the encrypted region.

### Where it bites today (concrete)

- `loft` opens, calls `list_dir("home/florian")` → full B-tree scan
  every time, even though the subtree probably hasn't changed.
- `wallpaper random` calls `npkfs::list()` → same full scan, just to
  filter by `home/<name>/pictures/wallpapers/` prefix.
- `setup_home` writes 6+ `.dir` markers on every boot (we made them
  idempotent in `0.82.0`, but they still hit the encrypt path
  unnecessarily).
- Loop's cwd-relative path resolution (`resolve_path`) handles edge
  cases that wouldn't exist with a real tree walk.
- Breadcrumb logic in loft assumes string-split-by-`/` reflects
  hierarchy — it does today, but only by convention.

---

## The design (npkFS v2)

Borrow the proven idea from Git / IPFS / OSTree: **directories are
content-addressed objects too**.

### Object types

```rust
enum Object {
    Blob(Vec<u8>),        // file contents (encrypted)
    Tree(Vec<TreeEntry>), // directory listing
    // Symlink, etc. later
}

struct TreeEntry {
    name:  String,        // short name only, no slashes
    hash:  [u8; 32],      // BLAKE3 of the referenced object
    kind:  EntryKind,     // File / Dir / (later: Symlink, …)
    size:  u64,           // recursive byte count for Dir, file size for File
    flags: u8,            // reserved for permissions / timestamps
}
```

The B-tree keys become **content hashes**, not paths. Keys are
fixed 32 bytes. Values are the encrypted object payload.

### Path resolution

`fetch("home/florian/pictures/wallpapers/aurora")`:
1. Read `superblock.root_tree` → 32-byte hash.
2. Fetch root tree, find entry `name == "home"` → next tree hash.
3. Fetch that tree, find `"florian"` → next.
4. … recurse 5 levels down.
5. Final entry: `kind == File`, `hash == X` → fetch blob X.

`O(depth)` B-tree lookups, each O(log N). Compare to today's O(N)
scan-with-filter. With depth 5 + 100k objects: 5 × log₂(100k) ≈ **85
node accesses** vs **100k**. Three orders of magnitude.

### Listing

`list_dir("home/florian/pictures")`:
1. Walk the path → arrive at the `pictures` tree.
2. Return its entries verbatim.

`O(depth + tree_size)`. The tree IS the listing. No filter pass, no
synthesised .dir markers, no scan.

### Mutations (write path)

`store("home/florian/notes.txt", data)`:
1. Walk to the parent tree (`home/florian`).
2. Hash + write the new blob → blob_hash.
3. Build a new `home/florian` tree with the additional entry; write
   it → florian_hash.
4. Build a new `home` tree referencing florian_hash → home_hash.
5. Build a new root tree referencing home_hash → root_hash.
6. Update superblock.root_tree atomically.

`O(depth)` B-tree inserts. **Old tree objects stay around** until
GC because the COW B-tree never overwrites — perfect for snapshots
(every superblock generation is a complete FS snapshot).

`rename("a/b", "a/c")` — same path: rewrite parent tree (one entry
name change), propagate up. **O(depth)**, no nested-data touched.

### Encryption layering

The whole tree blob is encrypted under the master key. The B-tree's
keys are content hashes (random-looking by construction), so no
path information leaks into key bytes. Only the **root tree hash**
in the superblock needs external visibility — every internal hash
stays inside the encrypted region.

This is strictly better privacy than v1, where every path component
becomes a deterministic part of an addressable key.

### Garbage collection

With COW + content-addressed trees, deletes don't free immediately.
Need a mark-and-sweep GC pass: walk reachable tree from current
root, anything not visited is garbage. Run on schedule, on idle,
or on demand (`gc` intent). Same model OSTree uses.

The 8-slot rotating superblock keeps prior generations alive, so
GC respects the "n previous snapshots" guarantee npkFS already
exposes.

---

## Migration

v1 → v2 is **disruptive** — different storage layout entirely.

Path forward:

1. Add v2 alongside v1: separate B-tree namespace under a different
   superblock slot. New code paths target v2; v1 remains read-only.
2. **One-shot migration intent** — `migrate-fs` walks every v1 key,
   parses its slash-segmented path, builds the v2 tree structure,
   writes blobs + trees, finalises a v2 root.
3. Bump kernel ABI to indicate v2 mounts. Old kernel reading a v2
   superblock refuses (rather than corrupting).
4. Once migration is verified on real installs (NUC + notebook):
   delete v1 code in a follow-up release.

### Compatibility

- Host-fn surface stays the same: `npk_fetch(path)`, `npk_fs_list(prefix)`,
  `npk_fs_stat(path)`, `npk_fs_delete(path)`. Apps don't rewrite —
  they keep handing in path strings, the FS handles them faster /
  better. **No app rebuild needed for v2 except possibly the FS
  test suite.**
- New host fns added as `npk_fs_rename(old, new)`,
  `npk_fs_snapshot()`, `npk_fs_gc()` — purely additive.
- Wire format / scene_commit ABI: untouched.

---

## When

**Not before P10 widget polish wraps + the file browser feature set
is stable** — the rewrite touches every read+write path in the
kernel; doing it during active UI work is a recipe for spurious
regressions.

**Before P11 AI integration ships in earnest** — AI agents will
generate dozens to hundreds of small files per task (working memory,
draft documents, chat logs). v1's listing performance dies under
that load.

**Realistic slot:** Phase 11.5, between "P10 final polish + Canvas"
and "P11 LLM-in-the-loop". Maybe 1.5–2 weeks of focused work for a
disciplined v2: format spec, kernel impl, migration tool, new
intents, test suite, OTA path.

---

## Hooks for "now"

While we wait, we keep these as constraints to avoid making things
harder:

1. Apps **never** parse paths themselves to derive structure beyond
   "split by `/`". Treat path-as-string as opaque to the rest of
   the system.
2. Don't pile on more `.dir`-marker logic. New features that need
   "is this a directory?" go through `npk_fs_stat` already; keep
   that the only gate.
3. Resist adding host fns that depend on the flat structure (e.g.
   "scan everything matching this glob"). We will not have those
   capabilities cheaply in v2 either, and any code that grows them
   in v1 is a migration blocker later.
4. `intent::resolve_path` and the cwd tracking in loop are special
   — they're working around v1's edges. Keep them isolated; don't
   let path-resolution logic spread into apps.
5. When in doubt about whether something belongs in npkFS or in
   intent code, lean toward intent — the FS layer should stay as
   thin as possible until v2 lands.

---

*Last updated: 2026-04-27 — parked after the loft 0.2.x rewrite
session. Nobody is working on this; the doc is the placeholder.*
