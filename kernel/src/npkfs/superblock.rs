//! Superblock Ring
//!
//! 8-slot rotating superblock to avoid SSD wear on fixed blocks.
//! Each slot has a generation counter. Highest valid generation wins.

use super::types::*;
use super::cache::BlockCache;

/// Read the most recent valid superblock from the 8-slot ring.
pub fn read_best(cache: &mut BlockCache) -> Result<Option<SuperblockRaw>, FsError> {
    let mut best: Option<SuperblockRaw> = None;
    let mut best_gen: u64 = 0;

    for slot in 0..SUPERBLOCK_SLOTS {
        let mut buf = AlignedBlock::zeroed();
        if cache.read(slot, &mut buf.0).is_err() { continue; }

        // SAFETY: AlignedBlock is 16-byte aligned, SuperblockRaw is repr(C) and BLOCK_SIZE
        let sb = unsafe { &*(buf.0.as_ptr() as *const SuperblockRaw) };

        if sb.magic != MAGIC || sb.version != VERSION { continue; }
        if sb.checksum != sb.compute_checksum() { continue; }

        if sb.generation >= best_gen {
            best_gen = sb.generation;
            best = Some(*sb);
        }
    }
    Ok(best)
}

/// Write superblock to the next ring slot. Returns the slot used.
pub fn write_next(cache: &mut BlockCache, sb: &mut SuperblockRaw) -> Result<u64, FsError> {
    sb.set_checksum();
    let slot = sb.generation % SUPERBLOCK_SLOTS;
    let buf = unsafe { &*(sb as *const SuperblockRaw as *const [u8; BLOCK_SIZE]) };
    cache.write(slot, buf)?;
    Ok(slot)
}

/// Write superblock to all 8 slots (used by mkfs).
pub fn write_all(cache: &mut BlockCache, sb: &mut SuperblockRaw) -> Result<(), FsError> {
    sb.set_checksum();
    let buf = unsafe { &*(sb as *const SuperblockRaw as *const [u8; BLOCK_SIZE]) };
    for slot in 0..SUPERBLOCK_SLOTS {
        cache.write(slot, buf)?;
    }
    Ok(())
}
