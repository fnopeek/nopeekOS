//! npkFS – Capability-native, content-addressed filesystem
//!
//! SSD-optimized: COW, TRIM, rotating superblock, write coalescing.
//! No paths, no tree. Objects identified by name + BLAKE3 hash.

mod types;
mod cache;
mod bitmap;
mod superblock;
mod journal;
mod btree;

pub use types::{FsError, ObjectEntry, Extent, BLOCK_SIZE, MAX_NAME_LEN, MAX_EXTENTS};

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use cache::BlockCache;
use bitmap::Bitmap;
use journal::Journal;
use types::*;
use crate::{kprintln, virtio_blk};

struct NpkFs {
    cache: BlockCache,
    sb: SuperblockRaw,
    bitmap: Bitmap,
    journal: Journal,
    generation: u64,
}

static FS: Mutex<Option<NpkFs>> = Mutex::new(None);

/// Format the disk with npkFS.
pub fn mkfs() -> Result<(), FsError> {
    let total_blocks = virtio_blk::block_count().ok_or(FsError::Disk(virtio_blk::BlkError::NotInitialized))?;
    if total_blocks < META_END + 16 {
        kprintln!("[npk] npkfs: disk too small ({} blocks)", total_blocks);
        return Err(FsError::DiskFull);
    }

    let mut cache = BlockCache::new()?;

    let bitmap_start = META_END;
    let bitmap_count = (total_blocks + BLOCK_SIZE as u64 * 8 - 1) / (BLOCK_SIZE as u64 * 8);
    let data_start = bitmap_start + bitmap_count;

    // Create empty B-tree root leaf
    let mut bmap = Bitmap::new_for_mkfs(total_blocks, bitmap_start, bitmap_count, data_start);
    let root_block = bmap.alloc(1)?;

    let root_buf = {
        let mut buf = [0u8; BLOCK_SIZE];
        let hdr = BTreeNodeHeader {
            magic: BTREE_MAGIC, node_type: BTREE_LEAF,
            _pad: 0, num_entries: 0, next_leaf: 0,
        };
        btree_write_header(&mut buf, &hdr);
        buf
    };
    cache.write(root_block, &root_buf)?;

    let mut sb = SuperblockRaw {
        magic: MAGIC,
        version: VERSION,
        flags: 0,
        generation: 1,
        total_blocks,
        free_blocks: bmap.free_count(),
        bitmap_start,
        bitmap_count,
        data_start,
        btree_root: root_block,
        object_count: 0,
        journal_head: 0,
        journal_seq: 0,
        _reserved: [0u8; 3968],
        checksum: [0u8; 32],
    };

    bmap.sync(&mut cache)?;
    superblock::write_all(&mut cache, &mut sb)?;
    cache.flush()?;

    kprintln!("[npk] npkfs: formatted {} blocks ({} MB), data starts at block {}",
        total_blocks, total_blocks * BLOCK_SIZE as u64 / (1024 * 1024), data_start);
    Ok(())
}

/// Mount the filesystem. Reads superblock, loads bitmap, replays journal.
pub fn mount() -> Result<(), FsError> {
    let mut cache = BlockCache::new()?;

    let sb = superblock::read_best(&mut cache)?.ok_or(FsError::NotFormatted)?;

    // Replay journal for crash recovery
    let frees = journal::Journal::replay(&mut cache, sb.journal_head, sb.journal_seq)?;
    let mut bmap = Bitmap::load(&mut cache, &sb)?;

    if !frees.is_empty() {
        kprintln!("[npk] npkfs: journal replay: {} free ops", frees.len());
        for (start, count) in &frees {
            bmap.free(*start, *count);
        }
        bmap.sync(&mut cache)?;
        cache.flush()?;
    }

    let gen = sb.generation;
    let jrnl = Journal::new(sb.journal_head, sb.journal_seq);

    *FS.lock() = Some(NpkFs { cache, sb, bitmap: bmap, journal: jrnl, generation: gen });

    kprintln!("[npk] npkfs: mounted (gen={}, {} objects, {} free blocks)",
        gen, sb.object_count, sb.free_blocks);
    Ok(())
}

/// Store an object. Returns BLAKE3 hash.
pub fn store(name: &str, data: &[u8], cap_id: u128) -> Result<[u8; 32], FsError> {
    validate_name(name)?;
    let hash = *blake3::hash(data).as_bytes();
    let tick = crate::interrupts::ticks();

    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    // Allocate data blocks
    let blocks_needed = (data.len() as u64 + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64;
    let blocks_needed = blocks_needed.max(1);

    let data_start = fs.bitmap.alloc(blocks_needed)?;

    // Write data blocks
    for i in 0..blocks_needed {
        let mut buf = [0u8; BLOCK_SIZE];
        let offset = i as usize * BLOCK_SIZE;
        let end = (offset + BLOCK_SIZE).min(data.len());
        if offset < data.len() {
            buf[..end - offset].copy_from_slice(&data[offset..end]);
        }
        fs.cache.write(data_start + i, &buf)?;
    }

    // Build entry
    let mut entry_name = [0u8; 64];
    let name_bytes = name.as_bytes();
    entry_name[..name_bytes.len()].copy_from_slice(name_bytes);

    let mut extents = [Extent::ZERO; MAX_EXTENTS];
    extents[0] = Extent { start_block: data_start, block_count: blocks_needed };

    let entry = ObjectEntry {
        name: entry_name,
        content_hash: hash,
        size: data.len() as u64,
        cap_id,
        created_tick: tick,
        extent_count: 1,
        extents,
    };

    // Insert into B-tree (COW)
    let root = fs.sb.btree_root;
    let (new_root, old_blocks) = btree::insert(&mut fs.cache, &mut fs.bitmap, root, &entry)?;

    // Record old blocks for deferred free
    for b in &old_blocks {
        fs.journal.record_free(*b, 1);
    }

    // Commit
    fs.generation += 1;
    fs.sb.btree_root = new_root;
    fs.sb.object_count += 1;
    fs.sb.free_blocks = fs.bitmap.free_count();
    fs.sb.generation = fs.generation;
    fs.sb.journal_head = fs.journal.head();

    fs.journal.commit(&mut fs.cache)?;
    fs.sb.journal_seq = fs.journal.seq();

    fs.bitmap.sync(&mut fs.cache)?;
    superblock::write_next(&mut fs.cache, &mut fs.sb)?;
    fs.cache.flush()?;

    // Now safe to free old blocks + TRIM
    for b in &old_blocks {
        fs.bitmap.free(*b, 1);
    }
    fs.bitmap.flush_trims();

    Ok(hash)
}

/// Fetch an object by name. Returns (data, hash).
pub fn fetch(name: &str) -> Result<(Vec<u8>, [u8; 32]), FsError> {
    validate_name(name)?;

    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    let mut key = [0u8; 64];
    key[..name.len()].copy_from_slice(name.as_bytes());

    let entry = btree::lookup(&mut fs.cache, fs.sb.btree_root, &key)?
        .ok_or(FsError::ObjectNotFound)?;

    let mut data = Vec::with_capacity(entry.size as usize);

    for ext_i in 0..entry.extent_count as usize {
        let ext = &entry.extents[ext_i];
        for b in 0..ext.block_count {
            let mut buf = [0u8; BLOCK_SIZE];
            fs.cache.read(ext.start_block + b, &mut buf)?;
            let remaining = entry.size as usize - data.len();
            let copy_len = BLOCK_SIZE.min(remaining);
            data.extend_from_slice(&buf[..copy_len]);
        }
    }

    // Verify integrity
    let hash = *blake3::hash(&data).as_bytes();
    if hash != entry.content_hash {
        kprintln!("[npk] npkfs: INTEGRITY FAILURE for '{}'", name);
        return Err(FsError::Corrupt);
    }

    Ok((data, hash))
}

/// Delete an object by name.
pub fn delete(name: &str) -> Result<(), FsError> {
    validate_name(name)?;

    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    let mut key = [0u8; 64];
    key[..name.len()].copy_from_slice(name.as_bytes());

    // Find the entry first to get its data extents
    let entry = btree::lookup(&mut fs.cache, fs.sb.btree_root, &key)?
        .ok_or(FsError::ObjectNotFound)?;

    // Delete from B-tree (COW)
    let root = fs.sb.btree_root;
    let (new_root, old_blocks) = btree::delete(&mut fs.cache, &mut fs.bitmap, root, &key)?;

    // Record all old blocks + data extents for deferred free
    for b in &old_blocks {
        fs.journal.record_free(*b, 1);
    }
    for i in 0..entry.extent_count as usize {
        let ext = &entry.extents[i];
        fs.journal.record_free(ext.start_block, ext.block_count);
    }

    // Commit
    fs.generation += 1;
    fs.sb.btree_root = new_root;
    fs.sb.object_count = fs.sb.object_count.saturating_sub(1);
    fs.sb.free_blocks = fs.bitmap.free_count();
    fs.sb.generation = fs.generation;
    fs.sb.journal_head = fs.journal.head();

    fs.journal.commit(&mut fs.cache)?;
    fs.sb.journal_seq = fs.journal.seq();

    fs.bitmap.sync(&mut fs.cache)?;
    superblock::write_next(&mut fs.cache, &mut fs.sb)?;
    fs.cache.flush()?;

    // Free old B-tree blocks + data blocks + TRIM
    for b in &old_blocks {
        fs.bitmap.free(*b, 1);
    }
    for i in 0..entry.extent_count as usize {
        let ext = &entry.extents[i];
        fs.bitmap.free(ext.start_block, ext.block_count);
        fs.cache.invalidate(ext.start_block); // evict freed data from cache
    }
    fs.bitmap.flush_trims();

    Ok(())
}

/// List all objects. Returns Vec of (name, size, hash).
pub fn list() -> Result<Vec<(String, u64, [u8; 32])>, FsError> {
    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    let mut result = Vec::new();
    btree::iter_all(&mut fs.cache, fs.sb.btree_root, &mut |entry| {
        let name = String::from(entry.name_str());
        result.push((name, entry.size, entry.content_hash));
    })?;
    Ok(result)
}

/// Get filesystem stats: (total_blocks, free_blocks, object_count, generation)
pub fn stats() -> Option<(u64, u64, u64, u64)> {
    let lock = FS.lock();
    let fs = lock.as_ref()?;
    Some((fs.sb.total_blocks, fs.sb.free_blocks, fs.sb.object_count, fs.generation))
}

pub fn is_mounted() -> bool {
    FS.lock().is_some()
}

fn validate_name(name: &str) -> Result<(), FsError> {
    if name.is_empty() { return Err(FsError::InvalidName); }
    if name.len() > MAX_NAME_LEN { return Err(FsError::NameTooLong); }
    if name.bytes().any(|b| b == 0 || b == b'/') { return Err(FsError::InvalidName); }
    Ok(())
}

fn btree_write_header(buf: &mut [u8; BLOCK_SIZE], hdr: &BTreeNodeHeader) {
    buf[0..4].copy_from_slice(&hdr.magic.to_le_bytes());
    buf[4] = hdr.node_type;
    buf[5] = hdr._pad;
    buf[6..8].copy_from_slice(&hdr.num_entries.to_le_bytes());
    buf[8..16].copy_from_slice(&hdr.next_leaf.to_le_bytes());
}
