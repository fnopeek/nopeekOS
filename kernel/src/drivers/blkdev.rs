//! Block Device Abstraction
//!
//! The system disk, whichever backend serves it: NVMe or virtio-blk on a
//! PC, the SD card on the Raspberry Pi 5 (`use_sd`). Provides a single API
//! for npkFS and other consumers; every address is shifted by the npkFS
//! partition's offset.

#[cfg(target_arch = "x86_64")]
use crate::{virtio_blk, nvme};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const SECTOR_SIZE: usize = 512;
pub const BLOCK_SIZE: usize = 4096;
const SECTORS_PER_BLOCK: u64 = (BLOCK_SIZE / SECTOR_SIZE) as u64;

/// How a request to the system disk failed, whichever backend serves it.
#[derive(Debug)]
pub enum BlkError {
    NotInitialized,
    OutOfRange,
    QueueFull,
    IoError,
    Unsupported,
    Timeout,
    Unknown(u8),
}

impl core::fmt::Display for BlkError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            BlkError::NotInitialized => write!(f, "disk not initialized"),
            BlkError::OutOfRange => write!(f, "sector out of range"),
            BlkError::QueueFull => write!(f, "request queue full"),
            BlkError::IoError => write!(f, "I/O error"),
            BlkError::Unsupported => write!(f, "unsupported operation"),
            BlkError::Timeout => write!(f, "request timed out"),
            BlkError::Unknown(s) => write!(f, "unknown status: {}", s),
        }
    }
}

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

/// The SD card serving as the system disk (`mmc_sd` disk id), or none.
const NO_SD: u32 = u32::MAX;
static SD_DISK: AtomicU32 = AtomicU32::new(NO_SD);

/// Make SD card `id` the system disk.
pub fn use_sd(id: u32) {
    SD_DISK.store(id, Ordering::Release);
}

#[derive(Clone, Copy)]
enum Backend {
    None,
    #[cfg(target_arch = "x86_64")]
    Nvme,
    #[cfg(target_arch = "x86_64")]
    Virtio,
    Sd(u32),
}

fn backend() -> Backend {
    let sd = SD_DISK.load(Ordering::Acquire);
    if sd != NO_SD {
        return Backend::Sd(sd);
    }
    #[cfg(target_arch = "x86_64")]
    {
        if nvme::is_available() { return Backend::Nvme; }
        if virtio_blk::is_available() { return Backend::Virtio; }
    }
    Backend::None
}

fn offset() -> u64 {
    PARTITION_OFFSET.load(Ordering::Acquire)
}

fn sd_err(e: crate::drivers::disk::Error) -> BlkError {
    use crate::drivers::disk::Error;
    match e {
        Error::Gone => BlkError::NotInitialized,
        Error::Range => BlkError::OutOfRange,
        Error::ReadOnly => BlkError::Unsupported,
        Error::Io | Error::Sense(..) => BlkError::IoError,
    }
}

/// `count` 4 KiB blocks from disk block `block` into `out`, one request.
fn sd_read(id: u32, block: u64, out: &mut [u8]) -> Result<(), BlkError> {
    crate::drivers::mmc_sd::read(id, block * SECTORS_PER_BLOCK, out).map_err(sd_err)
}

/// Every SD write returns once the card has programmed it, so it is
/// durable on completion.
fn sd_write(id: u32, block: u64, data: &[u8]) -> Result<(), BlkError> {
    crate::drivers::mmc_sd::write(id, block * SECTORS_PER_BLOCK, data).map_err(sd_err)
}

pub fn read_block(block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let actual = block + offset();
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::read_block(actual, buf),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::read_block(actual, buf),
        Backend::Sd(id) => sd_read(id, actual, buf),
        Backend::None => Err(BlkError::NotInitialized),
    }
}

pub fn write_block(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let actual = block + offset();
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::write_block(actual, buf),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::write_block(actual, buf),
        Backend::Sd(id) => sd_write(id, actual, buf),
        Backend::None => Err(BlkError::NotInitialized),
    }
}

/// Batched read — fills `output` with blocks read from disk in parallel
/// (NVMe). `output.len() == blocks.len() * BLOCK_SIZE`; block i lands at
/// `output[i*B..(i+1)*B]`. The others read one block after the other.
pub fn read_blocks_batch(blocks: &[u64], output: &mut [u8]) -> Result<(), BlkError> {
    if blocks.is_empty() { return Ok(()); }
    #[cfg(target_arch = "x86_64")]
    if let Backend::Nvme = backend() {
        let off = offset();
        let translated: alloc::vec::Vec<u64> = blocks.iter().map(|&b| b + off).collect();
        return nvme::read_blocks_batch(&translated, output);
    }
    for (i, &block) in blocks.iter().enumerate() {
        let dst: &mut [u8; BLOCK_SIZE] = (&mut output[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE]).try_into().unwrap();
        read_block(block, dst)?;
    }
    Ok(())
}

/// Read a contiguous extent of `count` blocks starting at `start_block`
/// into `output`. NVMe uses one PRP-list command per `MAX_BLOCKS_PER_CMD`
/// chunk and the SD card one multi-block request; virtio-blk reads block
/// by block.
pub fn read_extent(start_block: u64, count: u64, output: &mut [u8]) -> Result<(), BlkError> {
    if count == 0 { return Ok(()); }
    let actual = start_block + offset();
    let bytes = count as usize * BLOCK_SIZE;
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::read_extent(actual, count, output),
        Backend::Sd(id) => sd_read(id, actual, &mut output[..bytes]),
        _ => {
            for i in 0..count {
                let dst: &mut [u8; BLOCK_SIZE] = (&mut output[(i as usize) * BLOCK_SIZE..((i as usize) + 1) * BLOCK_SIZE])
                    .try_into().unwrap();
                read_block(start_block + i, dst)?;
            }
            Ok(())
        }
    }
}

/// Read a list of (start_block, count) extents into `output`. Extents
/// are concatenated in order (extent 0 → output[0..], etc.). NVMe
/// submits up to `MAX_INFLIGHT_MULTI` cmds across multiple extents in
/// parallel — critical for fragmented blobs where `read_extent` would
/// otherwise serialise. The others read extent after extent.
pub fn read_multi_extent(extents: &[(u64, u64)], output: &mut [u8]) -> Result<(), BlkError> {
    if extents.is_empty() { return Ok(()); }
    #[cfg(target_arch = "x86_64")]
    if let Backend::Nvme = backend() {
        let off = offset();
        let translated: alloc::vec::Vec<(u64, u64)> = extents.iter().map(|&(s, c)| (s + off, c)).collect();
        return nvme::read_multi_extent(&translated, output);
    }
    let mut out_off = 0usize;
    for &(start, count) in extents {
        let bytes = (count as usize) * BLOCK_SIZE;
        read_extent(start, count, &mut output[out_off..out_off + bytes])?;
        out_off += bytes;
    }
    Ok(())
}

/// Write a contiguous extent. Mirror of [`read_extent`].
pub fn write_extent(start_block: u64, count: u64, input: &[u8]) -> Result<(), BlkError> {
    if count == 0 { return Ok(()); }
    let actual = start_block + offset();
    let bytes = count as usize * BLOCK_SIZE;
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::write_extent(actual, count, input),
        Backend::Sd(id) => sd_write(id, actual, &input[..bytes]),
        _ => {
            for i in 0..count {
                let src: &[u8; BLOCK_SIZE] = input[(i as usize) * BLOCK_SIZE..((i as usize) + 1) * BLOCK_SIZE]
                    .try_into().unwrap();
                write_block(start_block + i, src)?;
            }
            Ok(())
        }
    }
}

/// Batched write — submits all payloads in parallel where the backend
/// supports it (NVMe). Caller passes (block, buf) pairs; offsets are
/// applied here. The others write one block after the other.
pub fn write_blocks_batch(items: &[(u64, &[u8; BLOCK_SIZE])]) -> Result<(), BlkError> {
    if items.is_empty() { return Ok(()); }
    #[cfg(target_arch = "x86_64")]
    if let Backend::Nvme = backend() {
        let off = offset();
        let translated: alloc::vec::Vec<(u64, &[u8; BLOCK_SIZE])> = items.iter().map(|&(b, buf)| (b + off, buf)).collect();
        return nvme::write_blocks_batch(&translated);
    }
    for &(block, buf) in items {
        write_block(block, buf)?;
    }
    Ok(())
}

/// Force everything previously written to stable media (durability barrier).
/// NVMe issues an NVM Flush; virtio-blk is a no-op (write-through
/// negotiated); the SD card has nothing in flight once a write returned.
/// The npkFS commit calls this between the data/bitmap writes and the
/// superblock so a power-loss can never expose an SB pointing at not-yet-
/// persisted blocks (= block double-alloc on remount).
pub fn flush() -> Result<(), BlkError> {
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::flush(),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::flush(),
        Backend::Sd(id) => crate::drivers::mmc_sd::flush(id).map_err(sd_err),
        Backend::None => Err(BlkError::NotInitialized),
    }
}

/// Write a single block with Force Unit Access (durable on completion). Used
/// for the npkFS superblock so it lands after the preceding `flush()` barrier
/// without a second full-cache FLUSH. Partition offset applied as in
/// `write_block`.
pub fn write_block_fua(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let actual = block + offset();
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::write_block_fua(actual, buf),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::write_block_fua(actual, buf),
        Backend::Sd(id) => sd_write(id, actual, buf),
        Backend::None => Err(BlkError::NotInitialized),
    }
}

/// Sector `lba` of the whole disk, before any partition: the partition
/// table and the ESP.
pub fn read_disk_sector(lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let actual = lba;
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::read_sector(actual, buf),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::read_sector(actual, buf),
        Backend::Sd(id) => crate::drivers::mmc_sd::read(id, actual, buf).map_err(sd_err),
        Backend::None => Err(BlkError::NotInitialized),
    }
}

/// Write sector `lba` of the whole disk, before any partition.
pub fn write_disk_sector(lba: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let actual = lba;
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::write_sector(actual, buf),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::write_sector(actual, buf),
        Backend::Sd(id) => crate::drivers::mmc_sd::write(id, actual, buf).map_err(sd_err),
        Backend::None => Err(BlkError::NotInitialized),
    }
}

/// Sector `sector` of the npkFS partition.
pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    read_disk_sector(sector + offset() * SECTORS_PER_BLOCK, buf)
}

/// Write sector `sector` of the npkFS partition.
pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    write_disk_sector(sector + offset() * SECTORS_PER_BLOCK, buf)
}

/// The longest wait for the disk since the last call, in microseconds;
/// 0 where the backend does not measure it.
pub fn take_longest_wait_us() -> u64 {
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::take_longest_wait_us(),
        _ => 0,
    }
}

/// The whole disk in 4 KiB blocks.
fn disk_blocks() -> Option<u64> {
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::block_count(),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::block_count(),
        Backend::Sd(id) => crate::drivers::mmc_sd::disks().iter().find(|d| d.id == id)
            .map(|d| d.blocks * d.block_size as u64 / BLOCK_SIZE as u64),
        Backend::None => None,
    }
}

pub fn block_count() -> Option<u64> {
    let after_offset = disk_blocks()?.saturating_sub(offset());
    let part_size = PARTITION_SIZE.load(Ordering::Acquire);
    // 0 = unset → "whole disk minus offset". With a real GPT partition the
    // size is set and caps the count, so the bitmap can't allocate into the
    // backup-GPT region at the end of the disk (writes would hit
    // BlkError::OutOfRange).
    if part_size == 0 {
        Some(after_offset)
    } else {
        Some(after_offset.min(part_size))
    }
}

/// The whole disk in 512-byte sectors.
pub fn capacity() -> Option<u64> {
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::capacity(),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::capacity(),
        Backend::Sd(_) => disk_blocks().map(|b| b * SECTORS_PER_BLOCK),
        Backend::None => None,
    }
}

pub fn is_available() -> bool {
    !matches!(backend(), Backend::None)
}

/// Whether the disk takes discards. The SD driver does not erase.
pub fn has_discard() -> bool {
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::has_discard(),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => virtio_blk::has_discard(),
        _ => false,
    }
}

pub fn discard_ranges(ranges: &[(u64, u64)]) -> Result<(), BlkError> {
    // Same partition_offset correction as read_block / write_block;
    // without it, TRIM would hit the ESP / GPT area in front of our
    // partition and destroy the bootloader and kernel.
    let off = offset();
    let actual: alloc::vec::Vec<(u64, u64)> = ranges.iter().map(|&(s, c)| (s + off, c)).collect();
    match backend() {
        #[cfg(target_arch = "x86_64")]
        Backend::Nvme => nvme::discard_ranges(&actual),
        #[cfg(target_arch = "x86_64")]
        Backend::Virtio => actual.iter().try_for_each(|&(s, c)| virtio_blk::discard_blocks(s, c)),
        _ => Err(BlkError::Unsupported),
    }
}
