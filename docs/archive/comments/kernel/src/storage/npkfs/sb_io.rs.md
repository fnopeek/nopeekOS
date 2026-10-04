# `kernel/src/storage/npkfs/sb_io.rs` @ 5e0102684

## L1-6 · `use super::cache::BlockCache;`

```
//! Superblock ring I/O (read_best / write_next_durable / write_all).
//!
//! 8-slot rotating layout. `read_best` matches strictly against the
//! current `DISK_MAGIC` + `DISK_VERSION`; older-version disks (e.g.
//! v2) end up in `read_legacy_magic` for the mount-time guard which
//! refuses to mount and asks for a reinstall.
```

## L12-15 · `pub fn read_best(cache: &mut BlockCache) -> Result<Option<SuperblockRaw>, FsError> {`

```
/// Read the highest-generation valid superblock from the 8-slot ring.
/// Returns `Ok(None)` if no slot validates — caller decides whether
/// that means "fresh disk, format it" or "older-version disk, refuse"
/// (`read_legacy_magic` covers the second case).
```

## L24-25 · `let sb = unsafe { &*(buf.0.as_ptr() as *const SuperblockRaw) };`

```
// SAFETY: AlignedBlock is 16-byte aligned, SuperblockRaw is
// repr(C) and exactly BLOCK_SIZE bytes (asserted at compile time).
```

## L39-42 · `#[derive(Default, Clone, Copy)]`

```
/// What the superblock ring actually looks like, slot by slot. `read_best`
/// answers "did I find one", which is not the same question as "is there a
/// filesystem here" — it says `continue` to a failed READ just as readily as
/// to an empty slot. Boot used that single bit to decide whether to format.
```

## L45 · `pub read_errors: usize,`

```
/// The block could not be read at all — device, offset or partition.
```

## L47 · `pub blank: usize,`

```
/// Read fine, first eight bytes all zero: never written.
```

## L49 · `pub valid: usize,`

```
/// A current-version superblock whose checksum verifies.
```

## L51 · `pub bad_checksum: usize,`

```
/// Current magic + version, checksum does NOT verify: damaged.
```

## L53 · `pub legacy: usize,`

```
/// An npkFS magic from an older on-disk version.
```

## L55 · `pub foreign: usize,`

```
/// Something else entirely — wrong offset, foreign partition, or garbage.
```

## L60-61 · `pub fn is_pristine(&self) -> bool {`

```
/// Every slot readable and every slot empty. The ONLY state in which
/// formatting destroys nothing.
```

## L67 · `pub fn probe(cache: &mut BlockCache) -> SbProbe {`

```
/// Classify all eight superblock slots without deciding anything.
```

## L88-89 · `let sb = unsafe { &*(buf.0.as_ptr() as *const SuperblockRaw) };`

```
// SAFETY: as in read_best — AlignedBlock is 16-byte aligned and
// SuperblockRaw is repr(C) of exactly BLOCK_SIZE bytes.
```

## L102-106 · `pub fn read_legacy_magic(cache: &mut BlockCache) -> Option<u8> {`

```
/// Detect a previous-version superblock magic anywhere in the SB ring.
/// Returns the first version byte found among the legacy magics that
/// matches; `None` if no slot has anything resembling an older npkFS.
/// Used by the mount-time guard to halt with a "reinstall to v3"
/// message instead of trying to parse the old format.
```

## L116-121 · `pub fn write_next_durable(cache: &mut BlockCache, sb: &mut SuperblockRaw) -> Result<u64, FsError> {`

```
/// Commit the next-generation superblock DURABLY: write it straight to disk
/// with FUA (bypassing the write-back cache) so it is on stable media when this
/// returns, and drop any stale cached copy of the slot. The caller MUST have
/// issued a `blkdev::flush()` first so everything the SB references is already
/// durable — otherwise a power-loss could expose an SB pointing at not-yet-
/// persisted data (block double-alloc on remount).
```

## L131 · `pub fn write_all(cache: &mut BlockCache, sb: &mut SuperblockRaw) -> Result<(), FsError> {`

```
/// Write the same superblock to all 8 slots (used by mkfs).
```

