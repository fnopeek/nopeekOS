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

pub use types::{FsError, ObjectEntry, Extent, BLOCK_SIZE, MAX_NAME_LEN, DIRECT_EXTENTS, EXTENTS_PER_INDIRECT};

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use cache::BlockCache;
use bitmap::Bitmap;
use journal::Journal;
use types::*;
use crate::{kprintln, blkdev, crypto};

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
    let total_blocks = blkdev::block_count().ok_or(FsError::Disk(crate::virtio_blk::BlkError::NotInitialized))?;
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

    // Generate random installation salt (unique per format)
    let install_salt = crate::csprng::random_256();
    let mut salt_16 = [0u8; 16];
    salt_16.copy_from_slice(&install_salt[..16]);

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
        install_salt: salt_16,
        _reserved: [0u8; 3952],
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

    let generation = sb.generation;
    let jrnl = Journal::new(sb.journal_head, sb.journal_seq);

    *FS.lock() = Some(NpkFs { cache, sb, bitmap: bmap, journal: jrnl, generation });

    kprintln!("[npk] npkfs: mounted (gen={}, {} objects, {} free blocks)",
        generation, sb.object_count, sb.free_blocks);
    Ok(())
}

/// Store or replace an object. Deletes existing object first if present.
pub fn upsert(name: &str, data: &[u8], cap_id: [u8; 32]) -> Result<[u8; 32], FsError> {
    if exists(name) {
        delete(name)?;
    }
    store(name, data, cap_id)
}

/// Store an object. Data is encrypted at rest with ChaCha20-Poly1305 AEAD.
/// Returns BLAKE3 hash of the plaintext.
pub fn store(name: &str, data: &[u8], cap_id: [u8; 32]) -> Result<[u8; 32], FsError> {
    let name = clean_name(name);
    validate_name(name)?;
    let hash = *blake3::hash(data).as_bytes();
    let tick = crate::interrupts::ticks();

    // Encrypt data if master key is available
    let encrypted = if let Some(master_key) = crypto::get_master_key() {
        let obj_key = crypto::derive_object_key(&master_key, &hash);
        let nonce = crypto::derive_nonce(&hash);
        Some(crypto::aead_encrypt(&obj_key, &nonce, data))
    } else {
        None
    };
    let write_data = encrypted.as_deref().unwrap_or(data);

    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    // Allocate data blocks (contiguous first, halve on failure, indirect for overflow)
    let blocks_needed = (write_data.len() as u64 + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64;
    let blocks_needed = blocks_needed.max(1);

    let mut all_extents = Vec::new();
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
                        let e: &Extent = ext;
                        fs.bitmap.free(e.start_block, e.block_count);
                    }
                    return Err(FsError::DiskFull);
                }
            }
        };
        all_extents.push(Extent { start_block: start, block_count: try_size });
        allocated += try_size;
    }

    // Write data blocks across all extents
    let mut data_offset = 0usize;
    for ext in &all_extents {
        for b in 0..ext.block_count {
            let mut buf = [0u8; BLOCK_SIZE];
            let end = (data_offset + BLOCK_SIZE).min(write_data.len());
            if data_offset < write_data.len() {
                buf[..end - data_offset].copy_from_slice(&write_data[data_offset..end]);
            }
            fs.cache.write(ext.start_block + b, &buf)?;
            data_offset += BLOCK_SIZE;
        }
    }

    // Build direct extents + indirect blocks if needed
    let mut direct = [Extent::ZERO; DIRECT_EXTENTS];
    let direct_count = all_extents.len().min(DIRECT_EXTENTS);
    for i in 0..direct_count {
        direct[i] = all_extents[i];
    }

    let indirect_block = if all_extents.len() > DIRECT_EXTENTS {
        write_indirect_extents(&mut fs.cache, &mut fs.bitmap, &all_extents[DIRECT_EXTENTS..])?
    } else {
        0
    };

    // Build entry
    let mut entry_name = [0u8; 64];
    let name_bytes = name.as_bytes();
    entry_name[..name_bytes.len()].copy_from_slice(name_bytes);

    let entry = ObjectEntry {
        name: entry_name,
        content_hash: hash,
        size: write_data.len() as u64,
        cap_id,
        created_tick: tick,
        extent_count: all_extents.len() as u32,
        extents: direct,
        indirect_block,
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

    // Phase 1: write journal entries (committed=0, safe on crash)
    fs.journal.prepare(&mut fs.cache)?;
    fs.sb.journal_seq = fs.journal.seq();

    // Phase 2: persist bitmap + superblock
    fs.bitmap.sync(&mut fs.cache)?;
    superblock::write_next(&mut fs.cache, &mut fs.sb)?;
    fs.cache.flush()?;

    // Phase 3: mark journal committed (superblock is now safe)
    fs.journal.finalize(&mut fs.cache)?;
    fs.cache.flush()?;

    // Phase 4: free old blocks + TRIM (deferred, safe)
    for b in &old_blocks {
        fs.bitmap.free(*b, 1);
    }
    fs.bitmap.flush_trims();

    Ok(hash)
}

/// Fetch an object by name. Returns (data, hash).
pub fn fetch(name: &str) -> Result<(Vec<u8>, [u8; 32]), FsError> {
    let name = clean_name(name);
    validate_name(name)?;

    let mut lock = FS.lock();
    let fs = lock.as_mut().ok_or(FsError::NotMounted)?;

    let mut key = [0u8; 64];
    key[..name.len()].copy_from_slice(name.as_bytes());

    let entry = btree::lookup(&mut fs.cache, fs.sb.btree_root, &key)?
        .ok_or(FsError::ObjectNotFound)?;

    let mut data = Vec::with_capacity(entry.size as usize);

    // Collect all extents (direct + indirect)
    let direct_count = (entry.extent_count as usize).min(DIRECT_EXTENTS);
    let mut all_extents: Vec<Extent> = entry.extents[..direct_count].to_vec();
    if entry.indirect_block != 0 {
        all_extents.extend(read_indirect_extents(&mut fs.cache, entry.indirect_block)?);
    }

    for ext in &all_extents {
        for b in 0..ext.block_count {
            let mut buf = [0u8; BLOCK_SIZE];
            fs.cache.read(ext.start_block + b, &mut buf)?;
            let remaining = entry.size as usize - data.len();
            let copy_len = BLOCK_SIZE.min(remaining);
            data.extend_from_slice(&buf[..copy_len]);
        }
    }

    // Decrypt if master key is available (encrypted data includes 16-byte AEAD tag)
    let plaintext = if let Some(master_key) = crypto::get_master_key() {
        let obj_key = crypto::derive_object_key(&master_key, &entry.content_hash);
        let nonce = crypto::derive_nonce(&entry.content_hash);
        match crypto::aead_decrypt(&obj_key, &nonce, &data) {
            Some(pt) => pt,
            None => {
                kprintln!("[npk] npkfs: DECRYPTION FAILED for '{}' (wrong key or corrupt)", name);
                return Err(FsError::Corrupt);
            }
        }
    } else {
        data
    };

    // Verify integrity (BLAKE3 of plaintext)
    let hash = *blake3::hash(&plaintext).as_bytes();
    if hash != entry.content_hash {
        kprintln!("[npk] npkfs: INTEGRITY FAILURE for '{}'", name);
        return Err(FsError::Corrupt);
    }

    Ok((plaintext, hash))
}

/// Delete an object by name.
pub fn delete(name: &str) -> Result<(), FsError> {
    let name = clean_name(name);
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
    // Direct extents
    let direct_count = (entry.extent_count as usize).min(DIRECT_EXTENTS);
    for i in 0..direct_count {
        fs.journal.record_free(entry.extents[i].start_block, entry.extents[i].block_count);
    }
    // Indirect extents
    if entry.indirect_block != 0 {
        if let Ok(indirect) = read_indirect_extents(&mut fs.cache, entry.indirect_block) {
            for ext in &indirect {
                fs.journal.record_free(ext.start_block, ext.block_count);
            }
        }
        // Also free the indirect blocks themselves
        free_indirect_chain(&mut fs.cache, &mut fs.bitmap, entry.indirect_block);
    }

    // Commit
    fs.generation += 1;
    fs.sb.btree_root = new_root;
    fs.sb.object_count = fs.sb.object_count.saturating_sub(1);
    fs.sb.free_blocks = fs.bitmap.free_count();
    fs.sb.generation = fs.generation;
    fs.sb.journal_head = fs.journal.head();

    // Phase 1: write journal entries (committed=0)
    fs.journal.prepare(&mut fs.cache)?;
    fs.sb.journal_seq = fs.journal.seq();

    // Phase 2: persist bitmap + superblock
    fs.bitmap.sync(&mut fs.cache)?;
    superblock::write_next(&mut fs.cache, &mut fs.sb)?;
    fs.cache.flush()?;

    // Phase 3: mark journal committed
    fs.journal.finalize(&mut fs.cache)?;
    fs.cache.flush()?;

    // Phase 4: free old B-tree blocks + data blocks + TRIM
    for b in &old_blocks {
        fs.bitmap.free(*b, 1);
    }
    let del_direct = (entry.extent_count as usize).min(DIRECT_EXTENTS);
    for i in 0..del_direct {
        fs.bitmap.free(entry.extents[i].start_block, entry.extents[i].block_count);
        fs.cache.invalidate(entry.extents[i].start_block);
    }
    // Indirect extents already freed above via free_indirect_chain + journal
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

/// Check if an object exists by name (B-tree lookup only, no data read).
pub fn exists(name: &str) -> bool {
    let name = clean_name(name);
    if validate_name(name).is_err() { return false; }
    let mut lock = FS.lock();
    let fs = match lock.as_mut() {
        Some(fs) => fs,
        None => return false,
    };
    let mut key = [0u8; 64];
    key[..name.len()].copy_from_slice(name.as_bytes());
    btree::lookup(&mut fs.cache, fs.sb.btree_root, &key)
        .ok().flatten().is_some()
}

/// Get the installation salt from the superblock.
pub fn install_salt() -> Option<[u8; 16]> {
    let lock = FS.lock();
    let fs = lock.as_ref()?;
    Some(fs.sb.install_salt)
}

pub fn is_mounted() -> bool {
    FS.lock().is_some()
}

/// Strip leading/trailing slashes from a name.
// ============================================================
// Indirect Extent Blocks
// ============================================================
// Format per 4KB block:
//   [0..4]   count: u32 (number of extents in this block)
//   [4..12]  next: u64  (next indirect block, 0 = end)
//   [12..]   extents: [Extent; count] (up to 255)

/// Write overflow extents to indirect blocks. Returns first indirect block address.
fn write_indirect_extents(
    cache: &mut cache::BlockCache, bitmap: &mut bitmap::Bitmap, extents: &[Extent],
) -> Result<u64, FsError> {
    let mut blocks = Vec::new();
    let mut offset = 0;

    // Allocate all indirect blocks first
    while offset < extents.len() {
        let block = bitmap.alloc(1)?;
        blocks.push(block);
        offset += EXTENTS_PER_INDIRECT;
    }

    // Write each indirect block (backwards to set chain pointers)
    offset = 0;
    for (i, &block) in blocks.iter().enumerate() {
        let count = (extents.len() - offset).min(EXTENTS_PER_INDIRECT);
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

/// Read all extents from an indirect block chain.
fn read_indirect_extents(
    cache: &mut cache::BlockCache, first_block: u64,
) -> Result<Vec<Extent>, FsError> {
    let mut extents = Vec::new();
    let mut block = first_block;

    while block != 0 {
        let mut buf = [0u8; BLOCK_SIZE];
        cache.read(block, &mut buf)?;
        let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        let next = u64::from_le_bytes(buf[4..12].try_into().unwrap());

        for j in 0..count.min(EXTENTS_PER_INDIRECT) {
            let off = 12 + j * 16;
            let start = u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
            let cnt = u64::from_le_bytes(buf[off + 8..off + 16].try_into().unwrap());
            extents.push(Extent { start_block: start, block_count: cnt });
        }
        block = next;
    }
    Ok(extents)
}

/// Free all indirect blocks in a chain.
fn free_indirect_chain(
    cache: &mut cache::BlockCache, bitmap: &mut bitmap::Bitmap, first_block: u64,
) {
    let mut block = first_block;
    while block != 0 {
        let mut buf = [0u8; BLOCK_SIZE];
        if cache.read(block, &mut buf).is_err() { break; }
        let next = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        bitmap.free(block, 1);
        block = next;
    }
}

fn clean_name(name: &str) -> &str {
    name.trim_matches('/')
}

fn validate_name(name: &str) -> Result<(), FsError> {
    if name.is_empty() { return Err(FsError::InvalidName); }
    if name.len() > MAX_NAME_LEN { return Err(FsError::NameTooLong); }
    if name.bytes().any(|b| b == 0) { return Err(FsError::InvalidName); }
    Ok(())
}

/// Validate a user-supplied name (rejects internal reserved names).
pub fn validate_user_name(name: &str) -> Result<(), FsError> {
    validate_name(name)?;
    // Check the filename component (after last /)
    let filename = name.rsplit('/').next().unwrap_or(name);
    if filename.starts_with(".npk-") { return Err(FsError::ReservedName); }
    if name.ends_with("/.dir") { return Err(FsError::ReservedName); }
    Ok(())
}

fn btree_write_header(buf: &mut [u8; BLOCK_SIZE], hdr: &BTreeNodeHeader) {
    buf[0..4].copy_from_slice(&hdr.magic.to_le_bytes());
    buf[4] = hdr.node_type;
    buf[5] = hdr._pad;
    buf[6..8].copy_from_slice(&hdr.num_entries.to_le_bytes());
    buf[8..16].copy_from_slice(&hdr.next_leaf.to_le_bytes());
}
