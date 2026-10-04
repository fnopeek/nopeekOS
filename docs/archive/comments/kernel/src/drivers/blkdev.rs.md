# `kernel/src/drivers/blkdev.rs` @ 5e0102684

## L1-4 · `use crate::{virtio_blk, nvme};`

```
//! Block Device Abstraction
//!
//! Dispatches to virtio-blk or NVMe, whichever is available.
//! Provides a single API for npkFS and other consumers.
```

## L13-15 · `static PARTITION_OFFSET: AtomicU64 = AtomicU64::new(0);`

```
/// Partition offset in 4KB blocks. All blkdev operations are shifted by this.
/// Set by the installer when NVMe is partitioned (npkFS starts after ESP).
/// Default 0 = whole disk (for virtio-blk / unpartitioned NVMe).
```

## L18-22 · `static PARTITION_SIZE: AtomicU64 = AtomicU64::new(0);`

```
/// Partition size in 4KB blocks. Bounds `block_count()` so we don't
/// allocate past the partition end into the backup-GPT reserved area
/// (final 34 sectors of the disk). Default 0 = "use whole disk minus
/// offset" — fine for virtio-blk where there's no partition table at
/// all.
```

## L25 · `pub fn set_partition_offset(blocks: u64) {`

```
/// Set partition offset (in 4KB blocks).
```

## L30-31 · `pub fn set_partition_size(blocks: u64) {`

```
/// Set partition size (in 4KB blocks). Pair with `set_partition_offset`
/// at install time so `block_count()` returns the right upper bound.
```

## L37 · `pub fn partition_offset() -> u64 {`

```
/// Get current partition offset (in 4KB blocks).
```

## L60-62 · `pub fn read_blocks_batch(blocks: &[u64], output: &mut [u8]) -> Result<(), BlkError> {`

```
/// Batched read — fills `output` with blocks read from disk in parallel
/// (NVMe). `output.len() == blocks.len() * BLOCK_SIZE`; block i lands at
/// `output[i*B..(i+1)*B]`. virtio-blk falls back to sequential.
```

## L82-86 · `pub fn read_extent(start_block: u64, count: u64, output: &mut [u8]) -> Result<(), BlkError> {`

```
/// Read a contiguous extent of `count` blocks starting at `start_block`
/// into `output`. NVMe path uses a single PRP-list cmd per
/// `MAX_BLOCKS_PER_CMD` chunk — a 256-block read drops from 256 cmds
/// (one per page) to ~2 cmds. virtio-blk path falls back to per-block
/// reads.
```

## L102-106 · `pub fn read_multi_extent(extents: &[(u64, u64)], output: &mut [u8]) -> Result<(), BlkError> {`

```
/// Read a list of (start_block, count) extents into `output`. Extents
/// are concatenated in order (extent 0 → output[0..], etc.). NVMe path
/// submits up to `MAX_INFLIGHT_MULTI` cmds across multiple extents in
/// parallel — critical for fragmented blobs where `read_extent` would
/// otherwise serialise. virtio-blk path falls back to sequential.
```

## L111 · `let translated: alloc::vec::Vec<(u64, u64)> = extents.iter()`

```
// Translate FS-relative blocks to disk-absolute.
```

## L132 · `pub fn write_extent(start_block: u64, count: u64, input: &[u8]) -> Result<(), BlkError> {`

```
/// Write a contiguous extent. Mirror of [`read_extent`].
```

## L149-152 · `pub fn write_blocks_batch(items: &[(u64, &[u8; BLOCK_SIZE])]) -> Result<(), BlkError> {`

```
/// Batched write — submits all payloads in parallel where the backend
/// supports it (NVMe). Caller passes (block, buf) pairs; offsets are
/// applied here. Falls back to sequential `write_block` for virtio-blk
/// (QEMU testing) where queue-depth gains are negligible.
```

## L158-161 · `let mut translated: alloc::vec::Vec<(u64, &[u8; BLOCK_SIZE])> =`

```
// Translate FS-relative blocks to disk-absolute and forward.
// Tiny stack-friendly buffer: cache flushes top out at ~16
// dirty slots in practice; up to 32 is supported (DMA pool
// size in nvme.rs).
```

## L176-180 · `pub fn flush() -> Result<(), BlkError> {`

```
/// Force everything previously written to stable media (durability barrier).
/// NVMe issues an NVM Flush; virtio-blk is a no-op (write-through negotiated).
/// The npkFS commit calls this between the data/bitmap writes and the
/// superblock so a power-loss can never expose an SB pointing at not-yet-
/// persisted blocks (= block double-alloc on remount).
```

## L189-192 · `pub fn write_block_fua(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {`

```
/// Write a single block with Force Unit Access (durable on completion). Used
/// for the npkFS superblock so it lands after the preceding `flush()` barrier
/// without a second full-cache FLUSH. Partition offset applied as in
/// `write_block`.
```

## L230-234 · `if part_size == 0 {`

```
// 0 = unset → fall back to the historical "whole disk minus offset"
// behaviour. With a real GPT partition the installer sets a positive
// size and we cap there so the bitmap can't allocate into the
// backup-GPT region at the end of the disk (writes would hit
// BlkError::OutOfRange).
```

## L263-266 · `let actual = start + PARTITION_OFFSET.load(Ordering::Acquire);`

```
// Same partition_offset correction as read_block / write_block —
// without it, TRIM commands land in the ESP / GPT area in front of
// our partition and slowly shred the bootloader + previously-
// written kernel.bin. Every delete's flush_trims() was doing this.
```

