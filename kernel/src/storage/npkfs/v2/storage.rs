//! v2 storage entry points: mkfs / mount / unmount / put / get / has / remove.
//!
//! Step-2 scope: a content-addressed object store. Caller hands in
//! `(hash, payload)` where `hash == BLAKE3(payload)`; we encrypt on
//! disk (if a master key is set), index by hash in the v2 B-tree, and
//! verify on read. No path layer, no Tree-walker — those land in
//! Steps 3-4.

use alloc::vec::Vec;
use spin::Mutex;

use super::super::bitmap::Bitmap;
use super::super::cache::BlockCache;
use super::super::journal::Journal;
use super::super::types::{BLOCK_SIZE, Extent, FsError};
use super::btree;
use super::format::{
    V2EntryRaw, V2SuperblockRaw,
    JOURNAL_BLOCKS, JOURNAL_START, META_END,
    V2_DIRECT_EXTENTS, V2_EXTENTS_PER_INDIRECT, V2_MAGIC, V2_VERSION,
};
#[allow(dead_code)]
const AEAD_TAG_LEN_DOC: usize = 16; // tag size appended by aead_encrypt; documented for future readers
use super::sb_io;
use crate::{blkdev, crypto, kprintln};

struct State {
    cache: BlockCache,
    sb: V2SuperblockRaw,
    bitmap: Bitmap,
    journal: Journal,
    generation: u64,
    /// B-tree COW old blocks accumulated across uncommitted `put`s.
    /// Drained, journaled, and freed in one shot by `commit_root` —
    /// turns the per-write cost from 5× 4-phase commits into 1.
    pending_old_blocks: Vec<u64>,
}

static FS: Mutex<Option<State>> = Mutex::new(None);

// ── Lifecycle ─────────────────────────────────────────────────────────

/// Format the entire disk to npkFS v2. **Destructive**: any v1 data is
/// gone after this call. Step 8 will add the boot-time refusal so this
/// only runs deliberately (installer / explicit intent).
pub fn mkfs() -> Result<(), FsError> {
    let total_blocks = blkdev::block_count()
        .ok_or(FsError::Disk(crate::virtio_blk::BlkError::NotInitialized))?;
    if total_blocks < META_END + 16 {
        kprintln!("[npk] npkfs2: disk too small ({} blocks)", total_blocks);
        return Err(FsError::DiskFull);
    }

    let mut cache = BlockCache::new()?;

    // Wipe the journal area so any leftover v1 (or stale v2) entries
    // can't be replayed on first mount. cache.flush() forces them to disk.
    let zero = [0u8; BLOCK_SIZE];
    for i in 0..JOURNAL_BLOCKS {
        cache.write(JOURNAL_START + i, &zero)?;
    }

    let bitmap_start = META_END;
    let bitmap_count = (total_blocks + BLOCK_SIZE as u64 * 8 - 1) / (BLOCK_SIZE as u64 * 8);
    let data_start = bitmap_start + bitmap_count;

    let mut bmap = Bitmap::new_for_mkfs(total_blocks, bitmap_start, bitmap_count, data_start);

    // Empty B-tree: btree_root = 0 means "no root yet"; first put()
    // allocates a leaf. Same convention as v1.
    let install_salt = crate::csprng::random_256();
    let mut salt_16 = [0u8; 16];
    salt_16.copy_from_slice(&install_salt[..16]);

    let mut sb = V2SuperblockRaw {
        magic: V2_MAGIC,
        version: V2_VERSION,
        flags: 0,
        generation: 1,
        total_blocks,
        free_blocks: bmap.free_count(),
        bitmap_start,
        bitmap_count,
        data_start,
        btree_root: 0,
        root_tree_hash: [0u8; 32],
        object_count: 0,
        journal_head: 0,
        journal_seq: 0,
        install_salt: salt_16,
        _reserved: [0u8; 3920],
        checksum: [0u8; 32],
    };

    bmap.sync(&mut cache)?;
    sb_io::write_all(&mut cache, &mut sb)?;
    cache.flush()?;

    kprintln!(
        "[npk] npkfs2: formatted {} blocks ({} MB), data starts at block {}",
        total_blocks, total_blocks * BLOCK_SIZE as u64 / (1024 * 1024), data_start,
    );
    Ok(())
}

/// Mount an existing v2 disk. Errors with `NotFormatted` if no v2
/// superblock validates (the disk may be unformatted, v1, or corrupt).
pub fn mount() -> Result<(), FsError> {
    let mut cache = BlockCache::new()?;
    let sb = sb_io::read_best(&mut cache)?.ok_or(FsError::NotFormatted)?;

    let frees = Journal::replay(&mut cache, sb.journal_head, sb.journal_seq)?;
    let mut bmap = Bitmap::load_args(
        &mut cache, sb.total_blocks, sb.bitmap_start, sb.bitmap_count, sb.data_start,
    )?;

    if !frees.is_empty() {
        kprintln!("[npk] npkfs2: journal replay: {} free ops", frees.len());
        for (start, count) in &frees {
            bmap.free(*start, *count);
        }
        bmap.sync(&mut cache)?;
        cache.flush()?;
    }

    let generation = sb.generation;
    let jrnl = Journal::new(sb.journal_head, sb.journal_seq);

    *FS.lock() = Some(State {
        cache, sb, bitmap: bmap, journal: jrnl, generation,
        pending_old_blocks: Vec::new(),
    });

    kprintln!(
        "[npk] npkfs2: mounted (gen={}, {} objects, {} free blocks)",
        generation, sb.object_count, sb.free_blocks,
    );
    Ok(())
}

/// Drop in-memory state. Used by the self-test to simulate a remount.
/// Does **not** flush — caller is responsible for a successful prior commit.
pub fn unmount() {
    *FS.lock() = None;
}

pub fn is_mounted() -> bool {
    FS.lock().is_some()
}

/// (total_blocks, free_blocks, object_count, generation)
pub fn stats() -> Option<(u64, u64, u64, u64)> {
    let lock = FS.lock();
    let s = lock.as_ref()?;
    Some((s.sb.total_blocks, s.sb.free_blocks, s.sb.object_count, s.generation))
}

/// Per-installation 128-bit salt baked at mkfs time. Used by callers
/// to derive deterministic-per-install secrets that survive remounts.
pub fn install_salt() -> Option<[u8; 16]> {
    let lock = FS.lock();
    Some(lock.as_ref()?.sb.install_salt)
}

/// Raw blkdev throughput measurement — write+read 1 MB through
/// `blkdev::{write,read}_extent`, no AEAD, no BLAKE3, no B-tree.
/// Allocates 256 contiguous blocks via the FS bitmap, runs the
/// transfer N times, then frees the blocks. Used by `disk` /
/// `testdisk` to find the HW ceiling beneath our crypto+FS stack.
/// Returns `(write_mbs, read_mbs)` or `None` if the FS isn't mounted
/// or the bench-region allocation fails.
pub fn raw_blk_bench() -> Option<(u64, u64)> {
    use crate::interrupts::{rdtsc, tsc_freq};
    const BLOCKS: u64 = 256; // 1 MB
    const ITERS: u64 = 4;

    let hz = tsc_freq();
    if hz == 0 { return None; }

    let mut lock = FS.lock();
    let fs = lock.as_mut()?;

    let start = fs.bitmap.alloc(BLOCKS).ok()?;

    let buf = alloc::vec![0xC3u8; (BLOCKS as usize) * BLOCK_SIZE];
    let mut read_buf = alloc::vec![0u8; (BLOCKS as usize) * BLOCK_SIZE];

    let bytes = BLOCKS * BLOCK_SIZE as u64 * ITERS;

    let t0 = rdtsc();
    let mut write_ok = true;
    for _ in 0..ITERS {
        if crate::blkdev::write_extent(start, BLOCKS, &buf).is_err() {
            write_ok = false;
            break;
        }
    }
    let dt = rdtsc().saturating_sub(t0).max(1);
    let write_mbs = if write_ok { (bytes * hz) / (dt * 1024 * 1024) } else { 0 };

    let t0 = rdtsc();
    let mut read_ok = true;
    for _ in 0..ITERS {
        if crate::blkdev::read_extent(start, BLOCKS, &mut read_buf).is_err() {
            read_ok = false;
            break;
        }
    }
    let dt = rdtsc().saturating_sub(t0).max(1);
    let read_mbs = if read_ok { (bytes * hz) / (dt * 1024 * 1024) } else { 0 };

    // Free; TRIM is deferred (gc / idle drain) so the bench cost stays
    // bounded.
    fs.bitmap.free(start, BLOCKS);

    Some((write_mbs, read_mbs))
}

/// Drain accumulated TRIM-pending ranges and issue them to the SSD.
/// Each `bitmap.free` adds to an in-memory list; this drains it in
/// one batch (merged + sorted ranges → fewer NVMe DEALLOCATE commands).
/// Pulled out of the per-commit hot path because synchronous TRIMs
/// were capping write IOPS; call this from `gc` or a periodic idle
/// sweep so the SSD's FTL eventually learns which blocks are free.
pub fn trim() -> Result<(), FsError> {
    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;
    fs.bitmap.flush_trims();
    Ok(())
}

/// Read every valid v2 superblock slot and return their `root_tree_hash`
/// values. Used by GC for the snapshot guarantee — anything reachable
/// from any of the 8 rotating slots stays alive.
pub fn all_root_hashes() -> Result<Vec<[u8; 32]>, FsError> {
    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    let mut out = Vec::new();
    for slot in 0..super::format::SUPERBLOCK_SLOTS {
        let mut buf = super::super::types::AlignedBlock::zeroed();
        if fs.cache.read(super::format::SUPERBLOCK_START + slot, &mut buf.0).is_err() {
            continue;
        }
        // SAFETY: AlignedBlock is BLOCK_SIZE-bytes + 16-aligned, and
        // V2SuperblockRaw is repr(C) BLOCK_SIZE.
        let sb = unsafe { &*(buf.0.as_ptr() as *const V2SuperblockRaw) };
        if sb.magic != V2_MAGIC || sb.version != V2_VERSION { continue; }
        if sb.checksum != sb.compute_checksum() { continue; }
        if sb.root_tree_hash != [0u8; 32] {
            out.push(sb.root_tree_hash);
        }
    }
    Ok(out)
}

/// Iterate every B-tree entry's content hash. Used by GC sweep.
pub fn all_object_hashes() -> Result<Vec<[u8; 32]>, FsError> {
    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;
    let mut out = Vec::new();
    super::btree::iter_all(&mut fs.cache, fs.sb.btree_root, &mut |entry| {
        out.push(entry.hash);
    })?;
    Ok(out)
}

/// Current root Tree hash from the superblock. The path layer reads this
/// to know where to start its walk; `commit_root` writes a new one.
pub fn current_root() -> Option<[u8; 32]> {
    let lock = FS.lock();
    Some(lock.as_ref()?.sb.root_tree_hash)
}

/// Atomically flip the superblock to a new root Tree hash AND commit
/// every accumulated `put` from this batch. One 4-phase commit covers
/// the SB-flip plus all the Tree/Blob writes, B-tree COW old blocks,
/// and bitmap+journal updates from the batch. A crash before this
/// returns leaves the SB on disk pointing at the previous root —
/// the in-cache uncommitted changes vanish, FS is consistent.
pub fn commit_root(new_root: [u8; 32]) -> Result<(), FsError> {
    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    let need_root_flip = fs.sb.root_tree_hash != new_root;
    let pending = core::mem::take(&mut fs.pending_old_blocks);
    if !need_root_flip && pending.is_empty() {
        return Ok(());
    }

    if need_root_flip {
        fs.sb.root_tree_hash = new_root;
    }
    fs.generation += 1;
    fs.sb.generation = fs.generation;
    fs.sb.free_blocks = fs.bitmap.free_count();
    for &b in &pending {
        fs.journal.record_free(b, 1);
    }
    fs.sb.journal_head = fs.journal.head();
    commit(fs, &pending)
}

// ── Object operations ─────────────────────────────────────────────────

/// Store `payload` indexed by `hash`. `hash` must equal BLAKE3(payload);
/// the call verifies this and returns `InvalidName` on mismatch (the
/// closest existing error variant — the contract is "the address you
/// claim is the address we'd compute").
///
/// `encrypt`: caller decides whether to AEAD-wrap the payload. Tree
/// objects must be readable pre-master-key (boot-time `exists` walks
/// them before the user has logged in), so the path layer passes
/// `encrypt=false` for trees and `encrypt=true` for file content
/// blobs. Encryption only actually happens if a master key is set;
/// otherwise the payload is stored plaintext regardless of the flag.
///
/// Idempotent: putting the same hash twice is a no-op (content-addressed
/// dedup). The first put owns the on-disk extents; subsequent ones see
/// the entry already present and return Ok.
pub fn put(hash: &[u8; 32], payload: &[u8], encrypt: bool) -> Result<(), FsError> {
    let computed = *blake3::hash(payload).as_bytes();
    if computed != *hash {
        return Err(FsError::InvalidName);
    }

    // Encrypt only if the caller asked AND we have a master key. The
    // result's length (= payload.len() + 16 AEAD tag) is what the read
    // path uses to infer "this object was AEAD-wrapped".
    //
    // AES-256-GCM via aes-gcm crate. With SSE + AES-NI enabled in
    // boot.s/trampoline.s, the crate's cpufeatures runtime detects
    // AES-NI on the N100 and dispatches to the hardware path
    // (AESENC + PCLMULQDQ for GHASH).
    let encrypted = if encrypt {
        crypto::get_master_key().map(|master_key| {
            let obj_key = crypto::derive_object_key(&master_key, hash);
            let nonce = crypto::derive_nonce(hash);
            crypto::aead_encrypt_aes(&obj_key, &nonce, payload)
        })
    } else {
        None
    };
    let write_data: &[u8] = encrypted.as_deref().unwrap_or(payload);

    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    // Fast-path: hash already present → nothing to do.
    if btree::lookup(&mut fs.cache, fs.sb.btree_root, hash)?.is_some() {
        return Ok(());
    }

    // Allocate extents (contiguous-first, halve on failure, indirect for overflow)
    let blocks_needed = ((write_data.len() as u64 + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64).max(1);

    let mut all_extents: Vec<Extent> = Vec::new();
    let mut allocated = 0u64;
    while allocated < blocks_needed {
        let remaining = blocks_needed - allocated;
        let mut try_size = remaining;
        let start = loop {
            match fs.bitmap.alloc(try_size) {
                Ok(s) => break s,
                Err(_) if try_size > 1 => { try_size = (try_size + 1) / 2; }
                Err(_) => {
                    for ext in &all_extents {
                        fs.bitmap.free(ext.start_block, ext.block_count);
                    }
                    return Err(FsError::DiskFull);
                }
            }
        };
        all_extents.push(Extent { start_block: start, block_count: try_size });
        allocated += try_size;
    }

    // Write payload bytes across the extents — direct to disk via the
    // single-cmd PRP-list extent path, bypassing the 64-slot block
    // cache. Pushing 4096 blocks of a 16 MB blob through that cache
    // thrashes it completely (every block evicts a metadata block,
    // synchronous writebacks one-at-a-time). Crash-safe because the
    // data isn't referenced from any committed Tree until commit_root
    // flips the SB; a crash mid-write just leaves orphan blocks in the
    // bitmap (reclaimed at next gc / not visible to any walk).
    //
    // One `write_extent` per FS extent → one NVMe cmd per extent (or
    // per `MAX_BLOCKS_PER_CMD` chunk for very large extents). A 1 MB
    // contiguous extent drops from 256 single-page cmds to ~2 cmds.
    for ext in &all_extents {
        for b in 0..ext.block_count {
            fs.cache.invalidate(ext.start_block + b);
        }
    }

    let mut consumed = 0usize;
    for ext in &all_extents {
        let blocks = ext.block_count as usize;
        let span = blocks * BLOCK_SIZE;
        let raw_end = write_data.len().min(consumed + span);
        let raw_len = raw_end.saturating_sub(consumed);

        if raw_len == span {
            crate::blkdev::write_extent(
                ext.start_block, ext.block_count, &write_data[consumed..raw_end],
            )?;
        } else {
            // Trailing extent contains a partial last block — pad with
            // zeros so the extent writer always sees a block-aligned
            // input. `disk_size` recorded in the entry below is the
            // pre-padding length, so the read path strips the padding.
            let mut padded = alloc::vec![0u8; span];
            padded[..raw_len].copy_from_slice(&write_data[consumed..raw_end]);
            crate::blkdev::write_extent(ext.start_block, ext.block_count, &padded)?;
        }
        consumed += span;
    }

    // Pack into V2EntryRaw: first V2_DIRECT_EXTENTS inline, rest in
    // an indirect chain.
    let mut direct = [Extent::ZERO; V2_DIRECT_EXTENTS];
    let direct_count = all_extents.len().min(V2_DIRECT_EXTENTS);
    for i in 0..direct_count {
        direct[i] = all_extents[i];
    }
    let indirect_block = if all_extents.len() > V2_DIRECT_EXTENTS {
        write_indirect_extents(&mut fs.cache, &mut fs.bitmap, &all_extents[V2_DIRECT_EXTENTS..])?
    } else {
        0
    };

    let entry = V2EntryRaw {
        hash: *hash,
        plaintext_size: payload.len() as u64,
        disk_size: write_data.len() as u64,
        extent_count: all_extents.len() as u32,
        _pad: 0,
        extents: direct,
        indirect_block,
    };

    let root = fs.sb.btree_root;
    let (new_root, old_blocks, was_new) =
        match btree::insert(&mut fs.cache, &mut fs.bitmap, root, &entry) {
            Ok(v) => v,
            Err(e) => {
                rollback_alloc(fs, &all_extents, indirect_block);
                return Err(e);
            }
        };

    if !was_new {
        // Defensive: the fast-path lookup above should have caught this
        // and short-circuited before any allocation. The btree itself is
        // also idempotent on duplicate hashes, so reaching here means a
        // racy state that the spinlock ought to prevent. Roll back the
        // wasted allocation cleanly anyway.
        rollback_alloc(fs, &all_extents, indirect_block);
        return Ok(());
    }

    // Defer commit. Path-layer mutations (`fs::write` etc.) call N puts
    // — one per blob + one per rebuilt Tree up to the root — followed
    // by exactly one `commit_root`. Running a 4-phase commit per put
    // means N synchronous NVMe-flush cycles for what's logically one
    // user-visible write; deferring lets `commit_root` cover the lot
    // in a single 4-phase. The cache holds the dirty data + B-tree
    // nodes, the bitmap reflects the new allocations, and the SB's
    // in-memory mirror tracks the new btree_root so subsequent puts
    // in the same batch see it.
    fs.sb.btree_root = new_root;
    fs.sb.object_count += 1;
    fs.sb.free_blocks = fs.bitmap.free_count();
    fs.pending_old_blocks.extend(old_blocks);
    Ok(())
}

/// Fetch the payload for `hash`. Returns Ok(None) if not present.
/// Verifies BLAKE3(plaintext) == hash before returning; mismatch = Corrupt.
pub fn get(hash: &[u8; 32]) -> Result<Option<Vec<u8>>, FsError> {
    use crate::interrupts::{rdtsc, tsc_freq};
    let t0 = rdtsc();

    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;
    let t_lock = rdtsc();

    let entry = match btree::lookup(&mut fs.cache, fs.sb.btree_root, hash)? {
        Some(e) => e,
        None => return Ok(None),
    };
    let t_btree = rdtsc();

    let direct_count = (entry.extent_count as usize).min(V2_DIRECT_EXTENTS);
    let mut all_extents: Vec<Extent> = entry.extents[..direct_count].to_vec();
    if entry.indirect_block != 0 {
        all_extents.extend(read_indirect_extents(&mut fs.cache, entry.indirect_block)?);
    }

    let total_blocks: u64 = all_extents.iter().map(|e| e.block_count).sum();
    let total_bytes = (total_blocks as usize) * BLOCK_SIZE;

    // Avoid the zero-init that `vec![0u8; N]` does — the DMA path is
    // about to overwrite every byte. `Vec::with_capacity + set_len` is
    // safe because the immediately-following `read_extent` writes into
    // [0..total_bytes]; if it errors we never expose the uninit Vec to
    // the caller.
    let mut staging: Vec<u8> = Vec::with_capacity(total_bytes);
    unsafe { staging.set_len(total_bytes); }
    let t_alloc = rdtsc();

    let mut buf_offset = 0usize;
    for ext in &all_extents {
        let bytes = (ext.block_count as usize) * BLOCK_SIZE;
        let dst = &mut staging[buf_offset..buf_offset + bytes];
        crate::blkdev::read_extent(ext.start_block, ext.block_count, dst)?;
        buf_offset += bytes;
    }
    let t_dma = rdtsc();

    staging.truncate(entry.disk_size as usize);

    let was_encrypted = entry.disk_size > entry.plaintext_size;

    if was_encrypted {
        let master_key = match crypto::get_master_key() {
            Some(k) => k,
            None => {
                kprintln!("[npk] npkfs2: encrypted blob {:02x}{:02x}… requested but no master key",
                    hash[0], hash[1]);
                return Err(FsError::Corrupt);
            }
        };
        let obj_key = crypto::derive_object_key(&master_key, hash);
        let nonce = crypto::derive_nonce(hash);
        if crypto::aead_decrypt_aes_in_place(&obj_key, &nonce, &mut staging).is_none() {
            kprintln!("[npk] npkfs2: decrypt failed for hash {:02x}{:02x}…",
                hash[0], hash[1]);
            return Err(FsError::Corrupt);
        }
    }
    let t_dec = rdtsc();
    let plaintext = staging;

    let computed = *blake3::hash(&plaintext).as_bytes();
    if computed != *hash {
        kprintln!("[npk] npkfs2: integrity failure for hash {:02x}{:02x}…",
            hash[0], hash[1]);
        return Err(FsError::Corrupt);
    }
    let t_verify = rdtsc();

    // Phase profile — only emitted for sizable reads so the small-file
    // path doesn't spam the serial log. ≥ 256 KB covers the 1 MB and
    // 16 MB testdisk buckets, where the per-stage breakdown matters.
    if total_bytes >= 256 * 1024 {
        let mhz = tsc_freq().max(1) / 1_000_000;
        kprintln!("[get] {}KB lock={} btree={} alloc={} dma={} dec={} verify={} (us)",
            total_bytes / 1024,
            (t_lock.saturating_sub(t0)) / mhz,
            (t_btree.saturating_sub(t_lock)) / mhz,
            (t_alloc.saturating_sub(t_btree)) / mhz,
            (t_dma.saturating_sub(t_alloc)) / mhz,
            (t_dec.saturating_sub(t_dma)) / mhz,
            (t_verify.saturating_sub(t_dec)) / mhz,
        );
    }

    Ok(Some(plaintext))
}

/// True iff the hash is present in the index. No data read.
pub fn has(hash: &[u8; 32]) -> bool {
    let mut lock = FS.lock();
    let fs = match lock.as_mut() {
        Some(s) => s,
        None => return false,
    };
    btree::lookup(&mut fs.cache, fs.sb.btree_root, hash).ok().flatten().is_some()
}

/// Drop the entry for `hash` and free its extents (deferred via journal).
pub fn remove(hash: &[u8; 32]) -> Result<(), FsError> {
    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    let entry = btree::lookup(&mut fs.cache, fs.sb.btree_root, hash)?
        .ok_or(FsError::ObjectNotFound)?;

    let root = fs.sb.btree_root;
    let (new_root, old_blocks) = btree::delete(&mut fs.cache, &mut fs.bitmap, root, hash)?;

    for b in &old_blocks {
        fs.journal.record_free(*b, 1);
    }
    let direct_count = (entry.extent_count as usize).min(V2_DIRECT_EXTENTS);
    for i in 0..direct_count {
        fs.journal.record_free(entry.extents[i].start_block, entry.extents[i].block_count);
    }
    let (indirect_extents, indirect_chain_blocks) = if entry.indirect_block != 0 {
        read_indirect_chain(&mut fs.cache, entry.indirect_block).unwrap_or_default()
    } else {
        (Vec::new(), Vec::new())
    };
    for ext in &indirect_extents {
        fs.journal.record_free(ext.start_block, ext.block_count);
    }
    for &cb in &indirect_chain_blocks {
        fs.journal.record_free(cb, 1);
    }

    fs.generation += 1;
    fs.sb.btree_root = new_root;
    fs.sb.object_count = fs.sb.object_count.saturating_sub(1);
    fs.sb.free_blocks = fs.bitmap.free_count();
    fs.sb.generation = fs.generation;
    fs.sb.journal_head = fs.journal.head();

    commit(fs, &old_blocks)?;

    // Phase 4: free + invalidate cache for direct, indirect-data, and
    // indirect-chain blocks. Same staging as v1.
    for i in 0..direct_count {
        let ext = &entry.extents[i];
        fs.bitmap.free(ext.start_block, ext.block_count);
        for b in 0..ext.block_count {
            fs.cache.invalidate(ext.start_block + b);
        }
    }
    for ext in &indirect_extents {
        fs.bitmap.free(ext.start_block, ext.block_count);
        for b in 0..ext.block_count {
            fs.cache.invalidate(ext.start_block + b);
        }
    }
    for &cb in &indirect_chain_blocks {
        fs.bitmap.free(cb, 1);
        fs.cache.invalidate(cb);
    }
    // TRIM deferred — see commit() below for why.

    Ok(())
}

/// Roll back a partial put: free every allocated data extent + the
/// indirect chain (if any), invalidate the cache slots we wrote into,
/// flush TRIM. Called from both the btree-insert error path and the
/// (defensive) duplicate-detected path; matches what `remove()` does
/// for symmetry — without this, freed blocks linger in `trim_pending`
/// and the cache holds stale dirty bytes that get written back on the
/// next flush.
fn rollback_alloc(fs: &mut State, extents: &[Extent], indirect_block: u64) {
    for ext in extents {
        fs.bitmap.free(ext.start_block, ext.block_count);
        for b in 0..ext.block_count {
            fs.cache.invalidate(ext.start_block + b);
        }
    }
    if indirect_block != 0 {
        free_indirect_chain(&mut fs.cache, &mut fs.bitmap, indirect_block);
    }
    // TRIM deferred — see commit() below for why.
}

// ── 4-phase commit (journal → bitmap+sb → finalize → free) ────────────

fn commit(fs: &mut State, old_blocks: &[u64]) -> Result<(), FsError> {
    // Phase 1: write journal entries with committed=0 (safe to crash).
    fs.journal.prepare(&mut fs.cache)?;
    fs.sb.journal_seq = fs.journal.seq();

    // Phase 2: persist bitmap + superblock to next ring slot.
    fs.bitmap.sync(&mut fs.cache)?;
    sb_io::write_next(&mut fs.cache, &mut fs.sb)?;
    fs.cache.flush()?;

    // Phase 3: mark journal committed (the new SB is durable now).
    fs.journal.finalize(&mut fs.cache)?;
    fs.cache.flush()?;

    // Phase 4: actually free the old B-tree blocks (data extents are
    // freed by the caller — they have scope to invalidate cache too).
    for b in old_blocks {
        fs.bitmap.free(*b, 1);
        fs.cache.invalidate(*b);
    }
    // TRIM deferred. Each NVMe DEALLOCATE is synchronous (FTL flush)
    // and a typical fs::write triggers ~5 commits with a few freed
    // blocks each — running flush_trims here capped write IOPS at ~3.
    // Freed blocks stay tracked in `trim_pending` (memory only) until
    // a future idle-task drains them in big batches.
    Ok(())
}

// ══ Indirect extent chain (same wire format as v1) ════════════════════
//
// Per 4 KB block:
//   [0..4]   count: u32 (extents in this block)
//   [4..12]  next:  u64 (next chain block, 0 = end)
//   [12..]   extents: [Extent; count] (up to V2_EXTENTS_PER_INDIRECT)

fn write_indirect_extents(
    cache: &mut BlockCache, bitmap: &mut Bitmap, extents: &[Extent],
) -> Result<u64, FsError> {
    let mut blocks: Vec<u64> = Vec::new();
    let mut offset = 0;
    while offset < extents.len() {
        blocks.push(bitmap.alloc(1)?);
        offset += V2_EXTENTS_PER_INDIRECT;
    }

    offset = 0;
    for (i, &block) in blocks.iter().enumerate() {
        let count = (extents.len() - offset).min(V2_EXTENTS_PER_INDIRECT);
        let next = if i + 1 < blocks.len() { blocks[i + 1] } else { 0u64 };

        let mut buf = [0u8; BLOCK_SIZE];
        buf[0..4].copy_from_slice(&(count as u32).to_le_bytes());
        buf[4..12].copy_from_slice(&next.to_le_bytes());
        for j in 0..count {
            let off = 12 + j * 16;
            buf[off..off + 8].copy_from_slice(&extents[offset + j].start_block.to_le_bytes());
            buf[off + 8..off + 16].copy_from_slice(&extents[offset + j].block_count.to_le_bytes());
        }
        cache.write(block, &buf)?;
        offset += count;
    }

    Ok(blocks[0])
}

fn read_indirect_extents(
    cache: &mut BlockCache, first_block: u64,
) -> Result<Vec<Extent>, FsError> {
    Ok(read_indirect_chain(cache, first_block)?.0)
}

fn read_indirect_chain(
    cache: &mut BlockCache, first_block: u64,
) -> Result<(Vec<Extent>, Vec<u64>), FsError> {
    let mut extents = Vec::new();
    let mut chain_blocks = Vec::new();
    let mut block = first_block;

    while block != 0 {
        chain_blocks.push(block);
        let mut buf = [0u8; BLOCK_SIZE];
        cache.read(block, &mut buf)?;
        let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        let next = u64::from_le_bytes(buf[4..12].try_into().unwrap());

        for j in 0..count.min(V2_EXTENTS_PER_INDIRECT) {
            let off = 12 + j * 16;
            let start = u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
            let cnt = u64::from_le_bytes(buf[off + 8..off + 16].try_into().unwrap());
            extents.push(Extent { start_block: start, block_count: cnt });
        }
        block = next;
    }
    Ok((extents, chain_blocks))
}

fn free_indirect_chain(
    cache: &mut BlockCache, bitmap: &mut Bitmap, first_block: u64,
) {
    let mut block = first_block;
    while block != 0 {
        let mut buf = [0u8; BLOCK_SIZE];
        if cache.read(block, &mut buf).is_err() { break; }
        let next = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        bitmap.free(block, 1);
        cache.invalidate(block);
        block = next;
    }
}

