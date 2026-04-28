//! v2 superblock ring I/O (read_best / write_next / write_all).
//!
//! 8-slot rotating layout shared with v1, but the structs and magic
//! bytes are v2-specific so the two cannot be confused at boot.

use super::super::cache::BlockCache;
use super::super::types::{AlignedBlock, BLOCK_SIZE, FsError};
use super::format::{V2SuperblockRaw, SUPERBLOCK_SLOTS, SUPERBLOCK_START, V2_MAGIC, V2_VERSION};

/// Read the highest-generation valid v2 superblock from the 8-slot ring.
/// Returns `Ok(None)` if no slot validates as v2 — caller decides whether
/// that means "not formatted as v2" or "this is a v1 disk, refuse" (Step 8
/// adds the v1-detection branch).
pub fn read_best(cache: &mut BlockCache) -> Result<Option<V2SuperblockRaw>, FsError> {
    let mut best: Option<V2SuperblockRaw> = None;
    let mut best_gen: u64 = 0;

    for slot in 0..SUPERBLOCK_SLOTS {
        let mut buf = AlignedBlock::zeroed();
        if cache.read(SUPERBLOCK_START + slot, &mut buf.0).is_err() { continue; }

        // SAFETY: AlignedBlock is 16-byte aligned, V2SuperblockRaw is
        // repr(C) and exactly BLOCK_SIZE bytes (asserted at compile time).
        let sb = unsafe { &*(buf.0.as_ptr() as *const V2SuperblockRaw) };

        if sb.magic != V2_MAGIC || sb.version != V2_VERSION { continue; }
        if sb.checksum != sb.compute_checksum() { continue; }

        if sb.generation >= best_gen {
            best_gen = sb.generation;
            best = Some(*sb);
        }
    }
    Ok(best)
}

/// Write the next generation to slot `gen % SUPERBLOCK_SLOTS`.
pub fn write_next(cache: &mut BlockCache, sb: &mut V2SuperblockRaw) -> Result<u64, FsError> {
    sb.set_checksum();
    let slot = SUPERBLOCK_START + (sb.generation % SUPERBLOCK_SLOTS);
    let buf = unsafe { &*(sb as *const V2SuperblockRaw as *const [u8; BLOCK_SIZE]) };
    cache.write(slot, buf)?;
    Ok(slot)
}

/// Write the same superblock to all 8 slots (used by mkfs).
pub fn write_all(cache: &mut BlockCache, sb: &mut V2SuperblockRaw) -> Result<(), FsError> {
    sb.set_checksum();
    let buf = unsafe { &*(sb as *const V2SuperblockRaw as *const [u8; BLOCK_SIZE]) };
    for slot in 0..SUPERBLOCK_SLOTS {
        cache.write(SUPERBLOCK_START + slot, buf)?;
    }
    Ok(())
}
