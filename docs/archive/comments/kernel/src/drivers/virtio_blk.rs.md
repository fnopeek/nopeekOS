# `kernel/src/drivers/virtio_blk.rs` @ 5e0102684

## L1-5 · `use core::sync::atomic::{fence, Ordering};`

```
//! VirtIO Block Device Driver
//!
//! Legacy (0.9.5) VirtIO PCI transport with split virtqueue.
//! Provides sector-level and block-level (4KB) I/O with TRIM/DISCARD.
//! SSD-friendly: negotiates DISCARD when available.
```

## L103 · `unsafe {`

```
// SAFETY: All port I/O targets the VirtIO device's I/O BAR
```

## L192 · `pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {`

```
// === Sector-level API ===
```

## L208 · `pub fn read_block(block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), BlkError> {`

```
// === Block-level API (4KB, for npkFS) ===
```

## L226-229 · `pub fn flush() -> Result<(), BlkError> {`

```
/// Durability barrier. We negotiate write-through (no VIRTIO_BLK_F_FLUSH in
/// `init`), so per the virtio spec the device keeps no volatile write cache and
/// there is nothing to flush — an explicit no-op so `blkdev::flush()` has a
/// backend on the QEMU path. (HW durability testing targets NVMe regardless.)
```

## L234-236 · `pub fn write_block_fua(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {`

```
/// No separate FUA path under write-through negotiation — a completed
/// `write_block` is already as durable as the device gets. Mirrors
/// `write_block` so `blkdev` has a backend.
```

## L241 · `pub fn discard_blocks(start: u64, count: u64) -> Result<(), BlkError> {`

```
/// TRIM/DISCARD blocks. Silent no-op if device doesn't support DISCARD.
```

## L257 · `let seg_addr = dev.req_hdrs + d1 as u64 * 16; // reuse header slot for discard segment`

```
// reuse header slot for discard segment
```

## L260 · `unsafe {`

```
// SAFETY: DMA buffers in identity-mapped range
```

## L302 · `0 | 2 => Ok(()), // UNSUPPORTED treated as OK (graceful degradation)`

```
// UNSUPPORTED treated as OK (graceful degradation)
```

## L309 · `pub fn block_count() -> Option<u64> {`

```
/// Total 4KB blocks on device
```

## L314 · `pub fn capacity() -> Option<u64> {`

```
/// Total 512-byte sectors
```

## L327 · `impl VirtioBlk {`

```
// === Internal ===
```

## L359 · `unsafe {`

```
// SAFETY: DMA buffers in identity-mapped range
```

## L401 · `unsafe {`

```
// SAFETY: Volatile writes to available ring, volatile reads from used ring
```

