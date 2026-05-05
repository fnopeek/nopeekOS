//! Superblock ring I/O (read_best / write_next / write_all).
//!
//! 8-slot rotating layout. `read_best` matches strictly against the
//! current `DISK_MAGIC` + `DISK_VERSION`; older-version disks (e.g.
//! v2) end up in `read_legacy_magic` for the mount-time guard which
//! refuses to mount and asks for a reinstall.

use super::cache::BlockCache;
use super::types::{AlignedBlock, BLOCK_SIZE, FsError};
use super::format::{SuperblockRaw, SUPERBLOCK_SLOTS, SUPERBLOCK_START, DISK_MAGIC, DISK_VERSION, DISK_MAGIC_V2};

/// Read the highest-generation valid v3 superblock from the 8-slot ring.
/// Returns `Ok(None)` if no slot validates as v3 — caller decides whether
/// that means "fresh disk, format it" or "older-version disk, refuse"
/// (`read_legacy_magic` covers the second case).
pub fn read_best(cache: &mut BlockCache) -> Result<Option<SuperblockRaw>, FsError> {
    let mut best: Option<SuperblockRaw> = None;
    let mut best_gen: u64 = 0;

    for slot in 0..SUPERBLOCK_SLOTS {
        let mut buf = AlignedBlock::zeroed();
        if cache.read(SUPERBLOCK_START + slot, &mut buf.0).is_err() { continue; }

        // SAFETY: AlignedBlock is 16-byte aligned, SuperblockRaw is
        // repr(C) and exactly BLOCK_SIZE bytes (asserted at compile time).
        let sb = unsafe { &*(buf.0.as_ptr() as *const SuperblockRaw) };

        if sb.magic != DISK_MAGIC || sb.version != DISK_VERSION { continue; }
        if sb.checksum != sb.compute_checksum() { continue; }

        if sb.generation >= best_gen {
            best_gen = sb.generation;
            best = Some(*sb);
        }
    }
    Ok(best)
}

/// Detect a previous-version superblock magic anywhere in the SB ring.
/// Returns the first version byte found among the legacy magics that
/// matches; `None` if no slot has anything resembling an older npkFS.
/// Used by the mount-time guard to halt with a "reinstall to v3"
/// message instead of trying to parse the old format.
pub fn read_legacy_magic(cache: &mut BlockCache) -> Option<u8> {
    for slot in 0..SUPERBLOCK_SLOTS {
        let mut buf = AlignedBlock::zeroed();
        if cache.read(SUPERBLOCK_START + slot, &mut buf.0).is_err() { continue; }
        if buf.0[..8] == DISK_MAGIC_V2 { return Some(2); }
    }
    None
}

/// Write the next generation to slot `gen % SUPERBLOCK_SLOTS`.
pub fn write_next(cache: &mut BlockCache, sb: &mut SuperblockRaw) -> Result<u64, FsError> {
    sb.set_checksum();
    let slot = SUPERBLOCK_START + (sb.generation % SUPERBLOCK_SLOTS);
    let buf = unsafe { &*(sb as *const SuperblockRaw as *const [u8; BLOCK_SIZE]) };
    cache.write(slot, buf)?;
    Ok(slot)
}

/// Write the same superblock to all 8 slots (used by mkfs).
pub fn write_all(cache: &mut BlockCache, sb: &mut SuperblockRaw) -> Result<(), FsError> {
    sb.set_checksum();
    let buf = unsafe { &*(sb as *const SuperblockRaw as *const [u8; BLOCK_SIZE]) };
    for slot in 0..SUPERBLOCK_SLOTS {
        cache.write(SUPERBLOCK_START + slot, buf)?;
    }
    Ok(())
}
