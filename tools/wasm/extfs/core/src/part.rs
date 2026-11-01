//! Partition tables, after Linux block/partitions: `efi_partition` (GPT,
//! efi.c) is tried before `msdos_partition` (msdos.c), as `check_part`
//! orders them. The BSD, Solaris and Unixware sub-labels msdos.c also
//! reads are not here: no removable drive formatted today carries them.
//!
//! Units are device blocks (Linux multiplies by `sector_size` because it
//! counts in 512-byte sectors; the block handle counts in blocks).

use alloc::vec::Vec;

use crate::crc32::crc32;
use crate::{Dev, IoError};

/// `MSDOS_LABEL_MAGIC`.
const MBR_MAGIC: [u8; 2] = [0x55, 0xAA];
const DOS_EXTENDED_PARTITION: u8 = 0x05;
const WIN98_EXTENDED_PARTITION: u8 = 0x0F;
const LINUX_EXTENDED_PARTITION: u8 = 0x85;
const EFI_PMBR_OSTYPE_EFI_GPT: u8 = 0xEE;
/// `DISK_MAX_PARTS` is larger; the kernel takes 16 volumes per disk.
pub const MAX_PARTS: usize = 16;

const GPT_HEADER_SIGNATURE: u64 = 0x5452_4150_2049_4645; // "EFI PART"
const GPT_PRIMARY_PARTITION_TABLE_LBA: u64 = 1;
/// `sizeof(gpt_header)` up to and including the entry-array CRC.
const GPT_HEADER_MIN: u32 = 92;
const GPT_ENTRY_SIZE: u32 = 128;
/// Bound on the entry array read (`KMALLOC_MAX_SIZE` in Linux).
const GPT_MAX_TABLE: u64 = 4 << 20;

/// One partition: first block and block count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub start: u64,
    pub count: u64,
}

struct Mbr<'a>(&'a [u8]);

impl Mbr<'_> {
    fn magic(&self) -> bool { self.0.len() >= 512 && self.0[510..512] == MBR_MAGIC }
    fn entry(&self, i: usize) -> &[u8] { &self.0[0x1BE + 16 * i..0x1BE + 16 * (i + 1)] }
}

fn boot_ind(p: &[u8]) -> u8 { p[0] }
fn sys_ind(p: &[u8]) -> u8 { p[4] }
fn start_sect(p: &[u8]) -> u64 { u32::from_le_bytes([p[8], p[9], p[10], p[11]]) as u64 }
fn nr_sects(p: &[u8]) -> u64 { u32::from_le_bytes([p[12], p[13], p[14], p[15]]) as u64 }

fn is_extended_partition(p: &[u8]) -> bool {
    matches!(sys_ind(p), DOS_EXTENDED_PARTITION | WIN98_EXTENDED_PARTITION | LINUX_EXTENDED_PARTITION)
}

/// `fat_valid_media`.
pub fn fat_valid_media(media: u8) -> bool {
    media >= 0xF8 || media == 0xF0
}

fn read_block<D: Dev>(dev: &mut D, lba: u64) -> Result<Vec<u8>, IoError> {
    let mut b = alloc::vec![0u8; dev.block_size() as usize];
    if lba >= dev.blocks() { return Err(IoError); }
    dev.read(lba, &mut b)?;
    Ok(b)
}

/// The partitions of the disk, or the whole disk as one when it carries no
/// table (a "superfloppy": a file system written straight to the device).
/// Partitions past the end are cut at the end, as `blk_add_partition` does;
/// one that overlaps an earlier one is left out.
pub fn scan<D: Dev>(dev: &mut D) -> Result<Vec<Part>, IoError> {
    let mut parts = Vec::new();
    if dev.block_size() < 512 || dev.blocks() == 0 { return Err(IoError); }
    if !efi_partition(dev, &mut parts)? {
        msdos_partition(dev, &mut parts)?;
    }
    let total = dev.blocks();
    let mut out: Vec<Part> = Vec::new();
    for p in parts {
        if p.count == 0 || p.start >= total { continue; }
        let count = p.count.min(total - p.start);
        let end = p.start + count;
        if out.iter().any(|q| p.start < q.start + q.count && q.start < end) { continue; }
        if out.len() == MAX_PARTS { break; }
        out.push(Part { start: p.start, count });
    }
    if out.is_empty() {
        out.push(Part { start: 0, count: total });
    }
    Ok(out)
}

// === msdos.c ===

/// `msdos_partition`. Returns whether the disk had an MS-DOS label.
fn msdos_partition<D: Dev>(dev: &mut D, parts: &mut Vec<Part>) -> Result<bool, IoError> {
    let data = read_block(dev, 0)?;
    let mbr = Mbr(&data);
    if !mbr.magic() { return Ok(false); }

    // The 55aa signature is there: a FAT boot sector or a partition table.
    // A boot indicator other than 0 or 0x80 is no table.
    for slot in 0..4 {
        let p = mbr.entry(slot);
        if boot_ind(p) != 0 && boot_ind(p) != 0x80 {
            // Maybe a FAT file system without a partition table.
            let reserved = u16::from_le_bytes([data[14], data[15]]);
            let fats = data[16];
            let media = data[21];
            return Ok(slot == 0 && reserved != 0 && fats != 0 && fat_valid_media(media));
        }
    }
    // An EFI GPT disk is left to efi.c.
    for slot in 0..4 {
        if sys_ind(mbr.entry(slot)) == EFI_PMBR_OSTYPE_EFI_GPT { return Ok(false); }
    }

    for slot in 0..4 {
        let p = mbr.entry(slot);
        let (start, size) = (start_sect(p), nr_sects(p));
        if size == 0 { continue; }
        if is_extended_partition(p) {
            parse_extended(dev, start, size, parts)?;
            continue;
        }
        parts.push(Part { start, count: size });
    }
    Ok(true)
}

/// `parse_extended`: the chain of logical partitions.
fn parse_extended<D: Dev>(dev: &mut D, first_sector: u64, first_size: u64, parts: &mut Vec<Part>) -> Result<(), IoError> {
    let mut this_sector = first_sector;
    let mut this_size = first_size;
    // Links followed without finding a data partition.
    let mut loopct = 0;
    loop {
        loopct += 1;
        if loopct > 100 || parts.len() >= MAX_PARTS { return Ok(()); }
        let Ok(data) = read_block(dev, this_sector) else { return Ok(()) };
        let mbr = Mbr(&data);
        if !mbr.magic() { return Ok(()); }

        // Usually entry 1 is the data partition and entry 2 the next link;
        // DRDOS and OS/2 use the others too, which may hold garbage.
        for i in 0..4 {
            let p = mbr.entry(i);
            if nr_sects(p) == 0 || is_extended_partition(p) { continue; }
            let offs = start_sect(p);
            let size = nr_sects(p);
            let Some(next) = this_sector.checked_add(offs) else { continue };
            if i >= 2 {
                if offs + size > this_size { continue; }
                if next < first_sector { continue; }
                if next + size > first_sector + first_size { continue; }
            }
            parts.push(Part { start: next, count: size });
            loopct = 0;
            if parts.len() >= MAX_PARTS { return Ok(()); }
        }
        // The first extended entry links to the next logical partition.
        let Some(p) = (0..4).map(|i| mbr.entry(i)).find(|p| nr_sects(p) != 0 && is_extended_partition(p)) else {
            return Ok(());
        };
        let Some(s) = first_sector.checked_add(start_sect(p)) else { return Ok(()) };
        this_sector = s;
        this_size = nr_sects(p);
    }
}

// === efi.c ===

const GPT_MBR_PROTECTIVE: u8 = 1;
const GPT_MBR_HYBRID: u8 = 2;

/// `pmbr_part_valid`.
fn pmbr_part_valid(p: &[u8]) -> bool {
    sys_ind(p) == EFI_PMBR_OSTYPE_EFI_GPT && start_sect(p) == 1
}

/// `is_pmbr_valid`: 0 none, protective or hybrid.
fn is_pmbr_valid(mbr: &Mbr) -> u8 {
    if !mbr.magic() { return 0; }
    if !(0..4).any(|i| pmbr_part_valid(mbr.entry(i))) { return 0; }
    if (0..4).any(|i| !matches!(sys_ind(mbr.entry(i)), EFI_PMBR_OSTYPE_EFI_GPT | 0)) {
        return GPT_MBR_HYBRID;
    }
    GPT_MBR_PROTECTIVE
}

struct Gpt {
    entries: Vec<u8>,
    num_entries: u32,
}

fn le64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8])) }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap_or([0; 4])) }

/// `is_gpt_valid`: header and entry array at `lba`, both CRCs.
fn is_gpt_valid<D: Dev>(dev: &mut D, lba: u64) -> Option<Gpt> {
    let bs = dev.block_size();
    let last_lba = dev.blocks() - 1;
    let mut h = read_block(dev, lba).ok()?;
    if le64(&h, 0) != GPT_HEADER_SIGNATURE { return None; }
    let header_size = le32(&h, 12);
    if header_size > bs || header_size < GPT_HEADER_MIN { return None; }
    let origcrc = le32(&h, 16);
    h[16..20].copy_from_slice(&[0; 4]);
    if crc32(&h[..header_size as usize]) != origcrc { return None; }
    if le64(&h, 24) != lba { return None; }
    let first_usable = le64(&h, 40);
    let last_usable = le64(&h, 48);
    if first_usable > last_lba || last_usable > last_lba || last_usable < first_usable { return None; }
    if le32(&h, 84) != GPT_ENTRY_SIZE { return None; }
    let num_entries = le32(&h, 80);
    let pt_size = num_entries as u64 * GPT_ENTRY_SIZE as u64;
    if pt_size == 0 || pt_size > GPT_MAX_TABLE { return None; }
    let entry_lba = le64(&h, 72);
    let entries = crate::read_bytes(dev, entry_lba.checked_mul(bs as u64)?, pt_size as usize).ok()?;
    if crc32(&entries) != le32(&h, 88) { return None; }
    Some(Gpt { entries, num_entries })
}

/// `find_valid_gpt` without `force_gpt`: a protective (or hybrid) MBR and
/// a valid primary header. The alternate is read only through the
/// primary, and only compared there, so the primary is the answer.
fn find_valid_gpt<D: Dev>(dev: &mut D) -> Result<Option<Gpt>, IoError> {
    let data = read_block(dev, 0)?;
    if is_pmbr_valid(&Mbr(&data)) == 0 { return Ok(None); }
    Ok(is_gpt_valid(dev, GPT_PRIMARY_PARTITION_TABLE_LBA))
}

/// `efi_partition`. Returns whether the disk had a valid GPT.
fn efi_partition<D: Dev>(dev: &mut D, parts: &mut Vec<Part>) -> Result<bool, IoError> {
    let Some(gpt) = find_valid_gpt(dev)? else { return Ok(false) };
    let last_lba = dev.blocks() - 1;
    for i in 0..gpt.num_entries as usize {
        if parts.len() >= MAX_PARTS { break; }
        let e = &gpt.entries[i * GPT_ENTRY_SIZE as usize..(i + 1) * GPT_ENTRY_SIZE as usize];
        // `is_pte_valid`.
        if e[0..16].iter().all(|&b| b == 0) { continue; }
        let (start, end) = (le64(e, 32), le64(e, 40));
        if start > last_lba || end > last_lba || end < start { continue; }
        parts.push(Part { start, count: end - start + 1 });
    }
    Ok(true)
}
