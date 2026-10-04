# `kernel/src/storage/npkfs/storage.rs` @ 5e0102684

## L1-7 · `use alloc::vec::Vec;`

```
//! npkFS storage entry points: mkfs / mount / unmount / put / get / has / remove.
//!
//! Step-2 scope: a content-addressed object store. Caller hands in
//! `(hash, payload)` where `hash == BLAKE3(payload)`; we encrypt on
//! disk (if a master key is set), index by hash in the B-tree, and
//! verify on read. No path layer, no Tree-walker — those land in
//! Steps 3-4.
```

## L23 · `const AEAD_TAG_LEN_DOC: usize = 16; // tag size appended by aead_encrypt; documented for future readers`

```
// tag size appended by aead_encrypt; documented for future readers
```

## L34-36 · `pending_old_blocks: Vec<u64>,`

```
/// B-tree COW old blocks accumulated across uncommitted `put`s.
/// Drained, journaled, and freed in one shot by `commit_root` —
/// turns the per-write cost from 5× 4-phase commits into 1.
```

## L42 · `fn halt_for_legacy_disk(version: u8) -> ! {`

```
// ── Lifecycle ─────────────────────────────────────────────────────────
```

## L44-47 · `fn halt_for_legacy_disk(version: u8) -> ! {`

```
/// Halt with an explicit reinstall message when a legacy npkFS magic
/// is detected on disk. Mirrors the v1 → v2 break that landed at
/// v0.83 — no in-place migration was the design then, same story
/// now for v2 → v3.
```

## L63-72 · `pub fn halt_for_unmountable_disk(p: &sb_io::SbProbe) -> ! {`

```
/// Halt when the disk holds an npkFS that would not mount. NOTHING has been
/// written at this point, and nothing will be — that is the whole purpose of
/// this function.
///
/// Boot used to answer this situation with `mkfs()`. A hung download followed
/// by `halt`, one unreadable superblock, a GPT probe that came back empty
/// because NVMe was not ready yet — each of them ended in a formatted disk
/// and a setup wizard, which reads from the outside exactly like "the
/// filesystem broke". It did not break; we overwrote it, and with it every
/// trace of why the mount failed.
```

## L95-98 · `pub fn mkfs() -> Result<(), FsError> {`

```
/// Format the entire disk to npkFS v3. **Destructive**: any pre-v3
/// data is gone after this call. The boot-time mount guard refuses
/// older formats with a reinstall message, so this only runs
/// deliberately (installer / explicit intent).
```

## L109-110 · `let zero = [0u8; BLOCK_SIZE];`

```
// Wipe the journal area so any leftover v1 (or stale v2) entries
// can't be replayed on first mount. cache.flush() forces them to disk.
```

## L122-123 · `let install_salt = crate::csprng::random_256();`

```
// Empty B-tree: btree_root = 0 means "no root yet"; first put()
// allocates a leaf. Same convention as v1.
```

## L159-162 · `pub fn probe_disk() -> Result<sb_io::SbProbe, FsError> {`

```
/// Classify the superblock ring without mounting and without changing a
/// byte. Boot calls this after a failed `mount()`: "the disk is blank" and
/// "there is an npkFS here that I could not read" are different facts, and
/// only the first one makes formatting harmless.
```

## L168-171 · `pub fn mount() -> Result<(), FsError> {`

```
/// Mount an existing v3 disk. Errors with `NotFormatted` if no v3
/// superblock validates AND no recognized legacy version is present.
/// If a legacy (v2) magic is detected the kernel halts with an
/// explicit reinstall message — there is no in-place migration.
```

## L211-212 · `pub fn unmount() {`

```
/// Drop in-memory state. Used by the self-test to simulate a remount.
/// Does **not** flush — caller is responsible for a successful prior commit.
```

## L221 · `pub fn stats() -> Option<(u64, u64, u64, u64)> {`

```
/// (total_blocks, free_blocks, object_count, generation)
```

## L228-229 · `pub fn install_salt() -> Option<[u8; 16]> {`

```
/// Per-installation 128-bit salt baked at mkfs time. Used by callers
/// to derive deterministic-per-install secrets that survive remounts.
```

## L235-241 · `pub fn raw_blk_bench() -> Option<(u64, u64)> {`

```
/// Raw blkdev throughput measurement — write+read 1 MB through
/// `blkdev::{write,read}_extent`, no AEAD, no BLAKE3, no B-tree.
/// Allocates 256 contiguous blocks via the FS bitmap, runs the
/// transfer N times, then frees the blocks. Used by `disk` /
/// `testdisk` to find the HW ceiling beneath our crypto+FS stack.
/// Returns `(write_mbs, read_mbs)` or `None` if the FS isn't mounted
/// or the bench-region allocation fails.
```

## L244 · `const BLOCKS: u64 = 256; // 1 MB`

```
// 1 MB
```

## L282-283 · `fs.bitmap.free(start, BLOCKS);`

```
// Free; TRIM is deferred (gc / idle drain) so the bench cost stays
// bounded.
```

## L289 · `#[derive(Clone, Copy)]`

```
// ── fsck: read-only integrity self-check ──────────────────────────────
```

## L324-332 · `pub fn self_check() -> Result<FsckReport, FsError> {`

```
/// Read-only filesystem integrity scan. Walks the committed B-tree, builds a
/// block-level refcount over every node + every object's data extents +
/// indirect-chain blocks, and reports:
///   - `double_alloc`: a block reachable from two places — THE corruption we
///     keep chasing (data written over a B-tree node / another object),
///   - `out_of_range`: a child/extent pointer past the device end,
///   - `free_but_referenced`: a referenced block the allocator thinks is free
///     (it will be handed out again → a future double-alloc).
/// No writes, no repair — pure diagnosis. Safe to call any time.
```

## L347 · `let root = fs.sb.btree_root;`

```
// Phase A — collect the tree structure (node blocks) + all leaf entries.
```

## L355 · `for &b in &nodes {`

```
// Mark every B-tree node block.
```

## L360-362 · `for e in &entries {`

```
// Mark every object's data extents + indirect-chain blocks. The chain
// walk already yields `indirect_block` as its first element, so we never
// mark it separately (that would be a false self-overlap).
```

## L384 · `for w in 0..words {`

```
// Phase C — referenced-but-free (serious: the allocator will re-hand it out).
```

## L400-405 · `pub fn trim() -> Result<(), FsError> {`

```
/// Drain accumulated TRIM-pending ranges and issue them to the SSD.
/// Each `bitmap.free` adds to an in-memory list; this drains it in
/// one batch (merged + sorted ranges → fewer NVMe DEALLOCATE commands).
/// Pulled out of the per-commit hot path because synchronous TRIMs
/// were capping write IOPS; call this from `gc` or a periodic idle
/// sweep so the SSD's FTL eventually learns which blocks are free.
```

## L413-415 · `pub fn all_root_hashes() -> Result<Vec<[u8; 32]>, FsError> {`

```
/// Read every valid superblock slot and return their `root_tree_hash`
/// values. Used by GC for the snapshot guarantee — anything reachable
/// from any of the 8 rotating slots stays alive.
```

## L426-427 · `let sb = unsafe { &*(buf.0.as_ptr() as *const SuperblockRaw) };`

```
// SAFETY: AlignedBlock is BLOCK_SIZE-bytes + 16-aligned, and
// SuperblockRaw is repr(C) BLOCK_SIZE.
```

## L438 · `pub fn all_object_hashes() -> Result<Vec<[u8; 32]>, FsError> {`

```
/// Iterate every B-tree entry's content hash. Used by GC sweep.
```

## L449-450 · `pub fn current_root() -> Option<[u8; 32]> {`

```
/// Current root Tree hash from the superblock. The path layer reads this
/// to know where to start its walk; `commit_root` writes a new one.
```

## L456-471 · `pub fn flush_pending() -> Result<(), FsError> {`

```
/// Atomically flip the superblock to a new root Tree hash AND commit
/// every accumulated `put` from this batch. One 4-phase commit covers
/// the SB-flip plus all the Tree/Blob writes, B-tree COW old blocks,
/// and bitmap+journal updates from the batch. A crash before this
/// returns leaves the SB on disk pointing at the previous root —
/// the in-cache uncommitted changes vanish, FS is consistent.
/// Commit everything `put` has deferred, WITHOUT touching the path tree.
///
/// `put` defers its commit on the documented assumption of "N puts, then
/// exactly one `commit_root`" — true for a file write, wildly false for a
/// streaming download, which does hundreds of puts and commits once at the
/// very end. Until then the in-memory bitmap, the object btree and
/// `pending_old_blocks` all run far ahead of the disk, and any UNRELATED
/// writer that commits in that window takes the whole batch with it.
/// Bounding the batch keeps the two views close and makes a long download's
/// progress durable as it goes.
```

## L481 · `commit_root(root)`

```
// Same root = no path-tree flip; this only drains the deferred batch.
```

## L504 · `let r = commit(fs, &pending);`

```
// journal_head is set inside commit(), AFTER prepare() advances it.
```

## L507-510 · `super::fs::note_commit();`

```
// Feed the auto-GC pressure counter (Git `gc --auto` model). Only
// real commits count; GC's own sweep removes go through the
// storage-internal `commit()` directly, not here, so GC can't
// feed its own trigger.
```

## L516 · `pub fn put(hash: &[u8; 32], payload: &[u8], encrypt: bool) -> Result<(), FsError> {`

```
// ── Object operations ─────────────────────────────────────────────────
```

## L518-532 · `pub fn put(hash: &[u8; 32], payload: &[u8], encrypt: bool) -> Result<(), FsError> {`

```
/// Store `payload` indexed by `hash`. `hash` must equal BLAKE3(payload);
/// the call verifies this and returns `InvalidName` on mismatch (the
/// closest existing error variant — the contract is "the address you
/// claim is the address we'd compute").
///
/// `encrypt`: caller decides whether to AEAD-wrap the payload. Tree
/// objects must be readable pre-master-key (boot-time `exists` walks
/// them before the user has logged in), so the path layer passes
/// `encrypt=false` for trees and `encrypt=true` for file content
/// blobs. Encryption only actually happens if a master key is set;
/// otherwise the payload is stored plaintext regardless of the flag.
///
/// Idempotent: putting the same hash twice is a no-op (content-addressed
/// dedup). The first put owns the on-disk extents; subsequent ones see
/// the entry already present and return Ok.
```

## L537-542 · `{`

```
// Cheap dedup-check FIRST. Content-addressing means equal payload
// ⇒ equal hash; if `hash` is already in the btree the rest of this
// function is wasted work — blake3-integrity (~0.6 ms / MB) and
// AES-GCM encrypt (~1.6 ms / MB) on the same MB we'd then throw
// away. Used to live AFTER encrypt, costing 2.2 ms per duplicate
// 1 MB write.
```

## L558-565 · `let encrypted = if encrypt {`

```
// Encrypt only if the caller asked AND we have a master key. The
// result's length (= payload.len() + 16 AEAD tag) is what the read
// path uses to infer "this object was AEAD-wrapped".
//
// AES-256-GCM via aes-gcm crate. With SSE + AES-NI enabled in
// boot.s/trampoline.s, the crate's cpufeatures runtime detects
// AES-NI on the N100 and dispatches to the hardware path
// (AESENC + PCLMULQDQ for GHASH).
```

## L581-584 · `if btree::lookup(&mut fs.cache, fs.sb.btree_root, hash)?.is_some() {`

```
// Race-check: someone else could have inserted the same hash while
// we did the encrypt without holding the lock. ROOT_MUTEX in
// fs::write serialises all path-layer mutations, so this is
// defensive in single-threaded operation but cheap.
```

## L589 · `let blocks_needed = ((write_data.len() as u64 + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64).max(1);`

```
// Allocate extents (contiguous-first, halve on failure, indirect for overflow)
```

## L613-624 · `for ext in &all_extents {`

```
// Write payload bytes across the extents — direct to disk via the
// single-cmd PRP-list extent path, bypassing the 64-slot block
// cache. Pushing 4096 blocks of a 16 MB blob through that cache
// thrashes it completely (every block evicts a metadata block,
// synchronous writebacks one-at-a-time). Crash-safe because the
// data isn't referenced from any committed Tree until commit_root
// flips the SB; a crash mid-write just leaves orphan blocks in the
// bitmap (reclaimed at next gc / not visible to any walk).
//
// One `write_extent` per FS extent → one NVMe cmd per extent (or
// per `MAX_BLOCKS_PER_CMD` chunk for very large extents). A 1 MB
// contiguous extent drops from 256 single-page cmds to ~2 cmds.
```

## L639-646 · `let res = if raw_len == span {`

```
// A failed device write must give the blocks BACK. Every other
// error path in this function rolls the allocation back (the
// DiskFull loop frees, the btree-insert failure calls
// rollback_alloc) — this one used to return through `?` and leak
// every extent it had just reserved. On a link that aborts
// transfers, that leak repeats per retry: a 249 MB asset can
// strand its blocks several times over, and the next symptom is
// DiskFull on a disk that looks half empty.
```

## L652-655 · `let mut padded = alloc::vec![0u8; span];`

```
// Trailing extent contains a partial last block — pad with
// zeros so the extent writer always sees a block-aligned
// input. `disk_size` recorded in the entry below is the
// pre-padding length, so the read path strips the padding.
```

## L670-671 · `let mut direct = [Extent::ZERO; DIRECT_EXTENTS];`

```
// Pack into BTreeEntryRaw: first DIRECT_EXTENTS inline, rest in
// an indirect chain.
```

## L704-708 · `rollback_alloc(fs, &all_extents, indirect_block);`

```
// Defensive: the fast-path lookup above should have caught this
// and short-circuited before any allocation. The btree itself is
// also idempotent on duplicate hashes, so reaching here means a
// racy state that the spinlock ought to prevent. Roll back the
// wasted allocation cleanly anyway.
```

## L713-721 · `fs.sb.btree_root = new_root;`

```
// Defer commit. Path-layer mutations (`fs::write` etc.) call N puts
// — one per blob + one per rebuilt Tree up to the root — followed
// by exactly one `commit_root`. Running a 4-phase commit per put
// means N synchronous NVMe-flush cycles for what's logically one
// user-visible write; deferring lets `commit_root` cover the lot
// in a single 4-phase. The cache holds the dirty data + B-tree
// nodes, the bitmap reflects the new allocations, and the SB's
// in-memory mirror tracks the new btree_root so subsequent puts
// in the same batch see it.
```

## L744-745 · `pub fn get(hash: &[u8; 32]) -> Result<Option<Vec<u8>>, FsError> {`

```
/// Fetch the payload for `hash`. Returns Ok(None) if not present.
/// Verifies BLAKE3(plaintext) == hash before returning; mismatch = Corrupt.
```

## L766-774 · `let disk_size = entry.disk_size;`

```
// Everything below (NVMe DMA + AES-GCM decrypt, ~10ms/4MB) needs only
// the extent list + a couple of sizes — not `fs`. Copy those out and
// DROP the global FS lock here so unrelated storage users (Core-0
// render reading icons/fonts, worker-core browser save(), gc, any
// WASM npk_fs_read) don't serialize behind this read's slow tail. The
// object is reachable from the committed btree root we just walked, so
// gc (which marks reachable blocks) won't free these extents; the only
// residual race — entry removed + gc + realloc inside this window — is
// caught by the AES-GCM tag below (→ Corrupt), never a crash/escape.
```

## L785-789 · `let mut staging: Vec<u8> = Vec::with_capacity(total_bytes);`

```
// Avoid the zero-init that `vec![0u8; N]` does — the DMA path is
// about to overwrite every byte. `Vec::with_capacity + set_len` is
// safe because the immediately-following `read_extent` writes into
// [0..total_bytes]; if it errors we never expose the uninit Vec to
// the caller.
```

## L794-798 · `crate::blkdev::read_multi_extent(&extent_list, &mut staging[..total_bytes])?;`

```
// Submit all extents through the multi-extent path so the SSD can
// pipeline them in parallel. For a contiguous blob (1 extent) this
// matches the old `read_extent` cost; for a fragmented blob (e.g.
// 1 MB split into 257 single-block extents after lots of churn)
// the parallelism saves seconds-per-MB at the worst.
```

## L826-834 · `if super::FS_PERF_LOG && total_bytes >= 256 * 1024 {`

```
// BLAKE3-verify intentionally elided. AES-GCM's tag already
// authenticates the ciphertext under (key, nonce), and both
// are derived from `hash` via `derive_object_key` /
// `derive_nonce`. Tampering anywhere — hash field in the btree
// entry, ciphertext on disk, AEAD tag — invalidates the tag
// check above and we return Corrupt then. A fresh BLAKE3 over
// the plaintext only catches scenarios already covered by the
// tag (or BLAKE3 collisions, ~2¹²⁸ unreachable). Removing it
// saves ~600 µs per 1 MB read (~25% throughput gain).
```

## L851 · `pub fn has(hash: &[u8; 32]) -> bool {`

```
/// True iff the hash is present in the index. No data read.
```

## L861 · `pub fn remove(hash: &[u8; 32]) -> Result<(), FsError> {`

```
/// Drop the entry for `hash` and free its extents (deferred via journal).
```

## L896 · `commit(fs, &old_blocks)?;`

```
// journal_head is set inside commit(), AFTER prepare() advances it.
```

## L900-901 · `for i in 0..direct_count {`

```
// Phase 4: free + invalidate cache for direct, indirect-data, and
// indirect-chain blocks. Same staging as v1.
```

## L919 · `Ok(())`

```
// TRIM deferred — see commit() below for why.
```

## L924-930 · `fn rollback_alloc(fs: &mut State, extents: &[Extent], indirect_block: u64) {`

```
/// Roll back a partial put: free every allocated data extent + the
/// indirect chain (if any), invalidate the cache slots we wrote into,
/// flush TRIM. Called from both the btree-insert error path and the
/// (defensive) duplicate-detected path; matches what `remove()` does
/// for symmetry — without this, freed blocks linger in `trim_pending`
/// and the cache holds stale dirty bytes that get written back on the
/// next flush.
```

## L941 · `}`

```
// TRIM deferred — see commit() below for why.
```

## L944 · `fn commit(fs: &mut State, old_blocks: &[u64]) -> Result<(), FsError> {`

```
// ── 4-phase commit (journal → bitmap+sb → finalize → free) ────────────
```

## L947 · `fs.journal.prepare(&mut fs.cache)?;`

```
// Phase 1: write journal entries with committed=0 (safe to crash).
```

## L949-957 · `fs.sb.journal_seq = fs.journal.seq();`

```
// Snapshot head+seq AFTER prepare advanced them — prepare wrote this
// commit's entries at [old_head, head) and bumped `head` past them.
// The SB must record the post-prepare head so `replay` scans BACKWARDS
// from just past this commit's entries and actually finds them. Setting
// journal_head before prepare (the old bug) made the SB point at the
// START of this commit, so replay read the previous commit first
// (seq < expected → break) and never recovered the latest commit's
// frees → freed COW/data blocks leaked after a crash, and the next
// mount resumed the journal at a stale head.
```

## L961-967 · `fs.bitmap.sync(&mut fs.cache)?;`

```
// Phase 2: persist bitmap + the data/metadata the new superblock
// references, with a REAL durability barrier before the SB. `cache.flush()`
// only pushes bytes into the controller's volatile write cache; on real
// hardware the SB could otherwise reach NAND before the btree/bitmap/data
// it points at, and a power-loss there leaves the SB referencing
// stale/garbage blocks — or blocks the bitmap still calls free — i.e.
// block double-alloc on remount. `blkdev::flush()` forces the order.
```

## L969 · `fs.cache.flush()?;        // journal(committed=0) + bitmap + new btree → controller`

```
// journal(committed=0) + bitmap + new btree → controller
```

## L970 · `crate::blkdev::flush()?;  // ▷ BARRIER: those + data extents now on stable media`

```
// ▷ BARRIER: those + data extents now on stable media
```

## L971-973 · `sb_io::write_next_durable(&mut fs.cache, &mut fs.sb)?;`

```
// SB written durably (FUA, straight to disk, bypassing the cache) so it
// lands strictly after the barrier. One FLUSH + one FUA per commit — the
// minimum that preserves the ordering invariant.
```

## L976-980 · `fs.journal.finalize(&mut fs.cache)?;`

```
// Phase 3: mark journal committed. The SB is durable now, so committed=1
// can never reach media before it. We deliberately do NOT force a barrier
// here: if power is lost before this persists, replay simply skips the
// (still committed=0) entry and the freed COW blocks leak — a benign,
// gc-reclaimable space leak, never corruption.
```

## L984-985 · `for b in old_blocks {`

```
// Phase 4: actually free the old B-tree blocks (data extents are
// freed by the caller — they have scope to invalidate cache too).
```

## L990-994 · `Ok(())`

```
// TRIM deferred. Each NVMe DEALLOCATE is synchronous (FTL flush)
// and a typical fs::write triggers ~5 commits with a few freed
// blocks each — running flush_trims here capped write IOPS at ~3.
// Freed blocks stay tracked in `trim_pending` (memory only) until
// a future idle-task drains them in big batches.
```

## L998-1003 · `fn write_indirect_extents(`

```
// ══ Indirect extent chain (same wire format as v1) ════════════════════
//
// Per 4 KB block:
//   [0..4]   count: u32 (extents in this block)
//   [4..12]  next:  u64 (next chain block, 0 = end)
//   [12..]   extents: [Extent; count] (up to EXTENTS_PER_INDIRECT)
```

