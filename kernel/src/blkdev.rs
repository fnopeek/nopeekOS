//! Block Device Abstraction
//!
//! Dispatches to virtio-blk or NVMe, whichever is available.
//! Provides a single API for npkFS and other consumers.

use crate::{virtio_blk, nvme};
use crate::virtio_blk::BlkError;

pub const SECTOR_SIZE: usize = 512;
pub const BLOCK_SIZE: usize = 4096;

pub fn read_block(block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    if nvme::is_available() {
        nvme::read_block(block, buf)
    } else {
        virtio_blk::read_block(block, buf)
    }
}

pub fn write_block(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    if nvme::is_available() {
        nvme::write_block(block, buf)
    } else {
        virtio_blk::write_block(block, buf)
    }
}

pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    if nvme::is_available() {
        nvme::read_sector(sector, buf)
    } else {
        virtio_blk::read_sector(sector, buf)
    }
}

pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    if nvme::is_available() {
        nvme::write_sector(sector, buf)
    } else {
        virtio_blk::write_sector(sector, buf)
    }
}

pub fn block_count() -> Option<u64> {
    if nvme::is_available() {
        nvme::block_count()
    } else {
        virtio_blk::block_count()
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
    if nvme::is_available() {
        // TODO: NVMe Dataset Management
        Err(BlkError::Unsupported)
    } else {
        virtio_blk::discard_blocks(start, count)
    }
}
