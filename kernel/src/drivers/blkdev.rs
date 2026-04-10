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

/// Set partition offset (in 4KB blocks).
pub fn set_partition_offset(blocks: u64) {
    PARTITION_OFFSET.store(blocks, Ordering::Release);
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
    total.map(|t| t.saturating_sub(PARTITION_OFFSET.load(Ordering::Acquire)))
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
    if nvme::is_available() {
        nvme::discard_blocks(start, count)
    } else {
        virtio_blk::discard_blocks(start, count)
    }
}
