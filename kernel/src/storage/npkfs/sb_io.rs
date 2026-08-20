//! Superblock ring I/O (read_best / write_next_durable / write_all).
//!
//! 8-slot rotating layout. `read_best` matches strictly against the
//! current `DISK_MAGIC` + `DISK_VERSION`; older-version disks (e.g.
//! v2) end up in `read_legacy_magic` for the mount-time guard which
//! refuses to mount and asks for a reinstall.

use super::cache::BlockCache;
use super::types::{AlignedBlock, BLOCK_SIZE, FsError};
use super::format::{SuperblockRaw, SUPERBLOCK_SLOTS, SUPERBLOCK_START, DISK_MAGIC, DISK_VERSION, DISK_MAGIC_V2};

/// Read the highest-generation valid superblock from the 8-slot ring.
/// Returns `Ok(None)` if no slot validates — caller decides whether
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

/// What the superblock ring actually looks like, slot by slot. `read_best`
/// answers "did I find one", which is not the same question as "is there a
/// filesystem here" — it says `continue` to a failed READ just as readily as
/// to an empty slot. Boot used that single bit to decide whether to format.
#[derive(Default, Clone, Copy)]
pub struct SbProbe {
    /// The block could not be read at all — device, offset or partition.
    pub read_errors: usize,
    /// Read fine, first eight bytes all zero: never written.
    pub blank: usize,
    /// A current-version superblock whose checksum verifies.
    pub valid: usize,
    /// Current magic + version, checksum does NOT verify: damaged.
    pub bad_checksum: usize,
    /// An npkFS magic from an older on-disk version.
    pub legacy: usize,
    /// Something else entirely — wrong offset, foreign partition, or garbage.
    pub foreign: usize,
}

impl SbProbe {
    /// Every slot readable and every slot empty. The ONLY state in which
    /// formatting destroys nothing.
    pub fn is_pristine(&self) -> bool {
        self.blank as u64 == SUPERBLOCK_SLOTS && self.read_errors == 0
    }
}

/// Classify all eight superblock slots without deciding anything.
pub fn probe(cache: &mut BlockCache) -> SbProbe {
    let mut p = SbProbe::default();
    for slot in 0..SUPERBLOCK_SLOTS {
        let mut buf = AlignedBlock::zeroed();
        if cache.read(SUPERBLOCK_START + slot, &mut buf.0).is_err() {
            p.read_errors += 1;
            continue;
        }
        if buf.0[..8] == [0u8; 8] {
            p.blank += 1;
            continue;
        }
        if buf.0[..8] == DISK_MAGIC_V2 {
            p.legacy += 1;
            continue;
        }
        if buf.0[..8] != DISK_MAGIC {
            p.foreign += 1;
            continue;
        }
        // SAFETY: as in read_best — AlignedBlock is 16-byte aligned and
        // SuperblockRaw is repr(C) of exactly BLOCK_SIZE bytes.
        let sb = unsafe { &*(buf.0.as_ptr() as *const SuperblockRaw) };
        if sb.version != DISK_VERSION {
            p.legacy += 1;
        } else if sb.checksum != sb.compute_checksum() {
            p.bad_checksum += 1;
        } else {
            p.valid += 1;
        }
    }
    p
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

/// Commit the next-generation superblock DURABLY: write it straight to disk
/// with FUA (bypassing the write-back cache) so it is on stable media when this
/// returns, and drop any stale cached copy of the slot. The caller MUST have
/// issued a `blkdev::flush()` first so everything the SB references is already
/// durable — otherwise a power-loss could expose an SB pointing at not-yet-
/// persisted data (block double-alloc on remount).
pub fn write_next_durable(cache: &mut BlockCache, sb: &mut SuperblockRaw) -> Result<u64, FsError> {
    sb.set_checksum();
    let slot = SUPERBLOCK_START + (sb.generation % SUPERBLOCK_SLOTS);
    let buf = unsafe { &*(sb as *const SuperblockRaw as *const [u8; BLOCK_SIZE]) };
    cache.invalidate(slot);
    crate::blkdev::write_block_fua(slot, buf)?;
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
