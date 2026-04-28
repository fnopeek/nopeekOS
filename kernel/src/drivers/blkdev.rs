//! Block Device Abstraction
//!
//! Dispatches to virtio-blk or NVMe, whichever is available.
//! Provides a single API for npkFS and other consumers.

use crate::{virtio_blk, nvme};
use crate::virtio_blk::BlkError;
use core::sync::atomic::{AtomicU64, Ordering};

pub const SECTOR_SIZE: usize = 512;
pub const BLOCK_SIZE: usize = 4096;

/// Partition offset in 4KB blocks. All blkdev operations are shifted by this.
/// Set by the installer when NVMe is partitioned (npkFS starts after ESP).
/// Default 0 = whole disk (for virtio-blk / unpartitioned NVMe).
static PARTITION_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Partition size in 4KB blocks. Bounds `block_count()` so we don't
/// allocate past the partition end into the backup-GPT reserved area
/// (final 34 sectors of the disk). Default 0 = "use whole disk minus
/// offset" — fine for virtio-blk where there's no partition table at
/// all.
static PARTITION_SIZE: AtomicU64 = AtomicU64::new(0);

/// Set partition offset (in 4KB blocks).
pub fn set_partition_offset(blocks: u64) {
    PARTITION_OFFSET.store(blocks, Ordering::Release);
}

/// Set partition size (in 4KB blocks). Pair with `set_partition_offset`
/// at install time so `block_count()` returns the right upper bound.
pub fn set_partition_size(blocks: u64) {
    PARTITION_SIZE.store(blocks, Ordering::Release);
}

#[allow(dead_code)]
/// Get current partition offset (in 4KB blocks).
pub fn partition_offset() -> u64 {
    PARTITION_OFFSET.load(Ordering::Acquire)
}

pub fn read_block(block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let actual = block + PARTITION_OFFSET.load(Ordering::Acquire);
    if nvme::is_available() {
        nvme::read_block(actual, buf)
    } else {
        virtio_blk::read_block(actual, buf)
    }
}

pub fn write_block(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let actual = block + PARTITION_OFFSET.load(Ordering::Acquire);
    if nvme::is_available() {
        nvme::write_block(actual, buf)
    } else {
        virtio_blk::write_block(actual, buf)
    }
}

/// Batched read — fills `output` with blocks read from disk in parallel
/// (NVMe). `output.len() == blocks.len() * BLOCK_SIZE`; block i lands at
/// `output[i*B..(i+1)*B]`. virtio-blk falls back to sequential.
pub fn read_blocks_batch(blocks: &[u64], output: &mut [u8]) -> Result<(), BlkError> {
    if blocks.is_empty() { return Ok(()); }
    let offset = PARTITION_OFFSET.load(Ordering::Acquire);

    if nvme::is_available() {
        let mut translated: alloc::vec::Vec<u64> =
            alloc::vec::Vec::with_capacity(blocks.len());
        for &b in blocks { translated.push(b + offset); }
        nvme::read_blocks_batch(&translated, output)
    } else {
        for (i, &block) in blocks.iter().enumerate() {
            let dst = &mut output[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE];
            let dst_arr: &mut [u8; BLOCK_SIZE] = dst.try_into().unwrap();
            virtio_blk::read_block(block + offset, dst_arr)?;
        }
        Ok(())
    }
}

/// Batched write — submits all payloads in parallel where the backend
/// supports it (NVMe). Caller passes (block, buf) pairs; offsets are
/// applied here. Falls back to sequential `write_block` for virtio-blk
/// (QEMU testing) where queue-depth gains are negligible.
pub fn write_blocks_batch(items: &[(u64, &[u8; BLOCK_SIZE])]) -> Result<(), BlkError> {
    if items.is_empty() { return Ok(()); }
    let offset = PARTITION_OFFSET.load(Ordering::Acquire);

    if nvme::is_available() {
        // Translate FS-relative blocks to disk-absolute and forward.
        // Tiny stack-friendly buffer: cache flushes top out at ~16
        // dirty slots in practice; up to 32 is supported (DMA pool
        // size in nvme.rs).
        let mut translated: alloc::vec::Vec<(u64, &[u8; BLOCK_SIZE])> =
            alloc::vec::Vec::with_capacity(items.len());
        for &(block, buf) in items {
            translated.push((block + offset, buf));
        }
        nvme::write_blocks_batch(&translated)
    } else {
        for &(block, buf) in items {
            virtio_blk::write_block(block + offset, buf)?;
        }
        Ok(())
    }
}

pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let offset_sectors = PARTITION_OFFSET.load(Ordering::Acquire) * 8;
    let actual = sector + offset_sectors;
    if nvme::is_available() {
        nvme::read_sector(actual, buf)
    } else {
        virtio_blk::read_sector(actual, buf)
    }
}

pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let offset_sectors = PARTITION_OFFSET.load(Ordering::Acquire) * 8;
    let actual = sector + offset_sectors;
    if nvme::is_available() {
        nvme::write_sector(actual, buf)
    } else {
        virtio_blk::write_sector(actual, buf)
    }
}

pub fn block_count() -> Option<u64> {
    let total = if nvme::is_available() {
        nvme::block_count()
    } else {
        virtio_blk::block_count()
    };
    let after_offset = total.map(|t| t.saturating_sub(PARTITION_OFFSET.load(Ordering::Acquire)))?;
    let part_size = PARTITION_SIZE.load(Ordering::Acquire);
    // 0 = unset → fall back to the historical "whole disk minus offset"
    // behaviour. With a real GPT partition the installer sets a positive
    // size and we cap there so the bitmap can't allocate into the
    // backup-GPT region at the end of the disk (writes would hit
    // BlkError::OutOfRange).
    if part_size == 0 {
        Some(after_offset)
    } else {
        Some(after_offset.min(part_size))
    }
}

pub fn capacity() -> Option<u64> {
    if nvme::is_available() {
        nvme::capacity()
    } else {
        virtio_blk::capacity()
    }
}

pub fn is_available() -> bool {
    nvme::is_available() || virtio_blk::is_available()
}

pub fn has_discard() -> bool {
    if nvme::is_available() {
        nvme::has_discard()
    } else {
        virtio_blk::has_discard()
    }
}

pub fn discard_blocks(start: u64, count: u64) -> Result<(), BlkError> {
    // Same partition_offset correction as read_block / write_block —
    // without it, TRIM commands land in the ESP / GPT area in front of
    // our partition and slowly shred the bootloader + previously-
    // written kernel.bin. Every delete's flush_trims() was doing this.
    let actual = start + PARTITION_OFFSET.load(Ordering::Acquire);
    if nvme::is_available() {
        nvme::discard_blocks(actual, count)
    } else {
        virtio_blk::discard_blocks(actual, count)
    }
}
