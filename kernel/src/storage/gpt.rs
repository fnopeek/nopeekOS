//! Minimal GPT writer for NVMe installation
//!
//! Creates a GUID Partition Table with two partitions:
//!   1. EFI System Partition (ESP) — 64 MB, FAT32
//!   2. npkFS data partition — rest of disk

use crate::nvme;

const ESP_TYPE_GUID: [u8; 16] = guid(0xC12A7328, 0xF81F, 0x11D2, 0xBA4B, 0x00A0C93EC93B);
const DATA_TYPE_GUID: [u8; 16] = guid(0x0FC63DAF, 0x8483, 0x4772, 0x8E79, 0x3D69D8477DE4);

/// ESP size in sectors (64 MB)
pub const ESP_SECTORS: u64 = 131072;
/// ESP starts at sector 2048 (1 MB alignment)
pub const ESP_START: u64 = 2048;
/// npkFS partition starts right after ESP
pub const NPKFS_START: u64 = ESP_START + ESP_SECTORS;

/// Convert GUID components to mixed-endian byte array (as per GPT spec).
const fn guid(d1: u32, d2: u16, d3: u16, d4: u16, d5: u64) -> [u8; 16] {
    let mut g = [0u8; 16];
    // d1: little-endian
    g[0] = d1 as u8; g[1] = (d1 >> 8) as u8;
    g[2] = (d1 >> 16) as u8; g[3] = (d1 >> 24) as u8;
    // d2: little-endian
    g[4] = d2 as u8; g[5] = (d2 >> 8) as u8;
    // d3: little-endian
    g[6] = d3 as u8; g[7] = (d3 >> 8) as u8;
    // d4: big-endian
    g[8] = (d4 >> 8) as u8; g[9] = d4 as u8;
    // d5: big-endian (6 bytes)
    g[10] = (d5 >> 40) as u8; g[11] = (d5 >> 32) as u8;
    g[12] = (d5 >> 24) as u8; g[13] = (d5 >> 16) as u8;
    g[14] = (d5 >> 8) as u8; g[15] = d5 as u8;
    g
}

fn random_guid() -> [u8; 16] {
    let mut g = [0u8; 16];
    let r1 = crate::csprng::random_u64();
    let r2 = crate::csprng::random_u64();
    g[..8].copy_from_slice(&r1.to_le_bytes());
    g[8..].copy_from_slice(&r2.to_le_bytes());
    // Set version 4 (random) and variant 1
    g[6] = (g[6] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn put_u32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

fn put_u64(buf: &mut [u8], off: usize, val: u64) {
    buf[off..off + 8].copy_from_slice(&val.to_le_bytes());
}

/// Detect existing GPT and return npkFS partition block offset.
/// Returns Some(block_offset) if GPT found with 2+ partitions, None otherwise.
pub fn detect_npkfs_offset() -> Option<u64> {
    Some(detect_npkfs_partition()?.0)
}

/// Is there a GPT header on this device at all? `Some(true)` = yes,
/// `Some(false)` = read fine, no signature, `None` = the read failed.
///
/// `detect_npkfs_partition` collapses all three into `None`, and boot then
/// could not tell "raw disk, offset 0 is ours" from "our partition table is
/// there but I could not read it" — the second one must never lead to a
/// format at offset 0, which is where the GPT and the ESP live.
pub fn has_gpt_header() -> Option<bool> {
    if !nvme::is_available() { return Some(false); }
    let mut hdr = [0u8; 512];
    nvme::read_sector(1, &mut hdr).ok()?;
    Some(&hdr[0..8] == b"EFI PART")
}

/// A partition table that passed the checks of UEFI 2.10 §5.3.2: the
/// header's signature, size and CRC, its own LBA, and the CRC of the entry
/// array. The primary header first, the backup at the disk's last sector
/// when the primary fails.
struct Table {
    /// Last usable LBA from the header; partitions must end at or before it.
    last_usable: u64,
    /// The first four entries (one sector), the only ones this kernel reads.
    entries: [u8; 512],
}

/// Entry arrays beyond this are not ones this kernel wrote or reads
/// (128 entries of 128 bytes is the UEFI minimum and our layout).
const MAX_ENTRY_BYTES: usize = 128 * 128;

fn read_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn read_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7]])
}

fn read_table() -> Option<Table> {
    if !nvme::is_available() { return None; }
    read_table_at(1).or_else(|| {
        let last = nvme::capacity()?.checked_sub(1)?;
        let t = read_table_at(last);
        if t.is_some() {
            crate::kprintln!("[npk] GPT: primary header invalid, using the backup");
        }
        t
    })
}

fn read_table_at(lba: u64) -> Option<Table> {
    let mut hdr = [0u8; 512];
    nvme::read_sector(lba, &mut hdr).ok()?;
    if &hdr[0..8] != b"EFI PART" { return None; }
    let size = read_u32(&hdr, 12) as usize;
    if !(92..=512).contains(&size) { return None; }
    let mut copy = hdr;
    copy[16..20].fill(0);
    if crc32(&copy[..size]) != read_u32(&hdr, 16) { return None; }
    if read_u64(&hdr, 24) != lba { return None; }

    let entries_lba = read_u64(&hdr, 72);
    let count = read_u32(&hdr, 80) as usize;
    let entry_size = read_u32(&hdr, 84) as usize;
    let bytes = count.checked_mul(entry_size)?;
    if entry_size < 128 || bytes == 0 || bytes > MAX_ENTRY_BYTES { return None; }
    let mut array = alloc::vec![0u8; bytes.div_ceil(512) * 512];
    for (i, sec) in array.chunks_exact_mut(512).enumerate() {
        let sec: &mut [u8; 512] = sec.try_into().ok()?;
        nvme::read_sector(entries_lba.checked_add(i as u64)?, sec).ok()?;
    }
    if crc32(&array[..bytes]) != read_u32(&hdr, 88) { return None; }
    if entry_size != 128 { return None; }

    let mut entries = [0u8; 512];
    entries.copy_from_slice(&array[..512]);
    Some(Table { last_usable: read_u64(&hdr, 48), entries })
}

/// Detect existing GPT and return npkFS partition (block_offset, block_count).
/// Both values are in 4 KB blocks. Used by the boot path so blkdev knows
/// both the start and the size of our partition — without the size,
/// allocations past the partition end land in the backup-GPT region
/// at the disk tail and hit `BlkError::OutOfRange`.
pub fn detect_npkfs_partition() -> Option<(u64, u64)> {
    let t = read_table()?;
    // Second partition entry.
    let e = &t.entries[128..256];
    if e[0..16] == [0u8; 16] { return None; }
    let start_lba = read_u64(e, 32);
    let end_lba = read_u64(e, 40);
    if start_lba == 0 || end_lba <= start_lba || end_lba > t.last_usable { return None; }

    let block_offset = start_lba / 8;
    // end_lba is inclusive. Block count = floor of usable sector count / 8.
    let usable_sectors = end_lba.checked_add(1)?.checked_sub(start_lba)?;
    Some((block_offset, usable_sectors / 8))
}

/// Detect existing GPT and return ESP (EFI System Partition) sector offset.
pub fn detect_esp_offset() -> Option<u64> {
    let t = read_table()?;
    // First partition entry: the ESP.
    if t.entries[0..16] != ESP_TYPE_GUID { return None; }
    let start_lba = read_u64(&t.entries, 32);
    if start_lba == 0 { return None; }
    Some(start_lba)
}

/// Write GPT to NVMe. Returns the sector where npkFS partition starts.
pub fn write_gpt() -> Result<u64, &'static str> {
    let total_sectors = nvme::capacity().ok_or("NVMe not available")?;
    let last_sector = total_sectors - 1;
    let last_usable = last_sector - 33; // backup GPT entries + header
    let npkfs_end = last_usable;

    let disk_guid = random_guid();
    let esp_guid = random_guid();
    let npkfs_guid = random_guid();

    // === Partition entries (128 bytes each, 128 entries = 32 sectors) ===
    let mut entries = [0u8; 512 * 32];

    // Entry 0: ESP
    entries[0..16].copy_from_slice(&ESP_TYPE_GUID);
    entries[16..32].copy_from_slice(&esp_guid);
    put_u64(&mut entries, 32, ESP_START);
    put_u64(&mut entries, 40, ESP_START + ESP_SECTORS - 1);
    // Name "EFI System" in UTF-16LE
    let esp_name = b"E\0F\0I\0 \0S\0y\0s\0t\0e\0m\0";
    entries[56..56 + esp_name.len()].copy_from_slice(esp_name);

    // Entry 1: npkFS
    let off = 128;
    entries[off..off + 16].copy_from_slice(&DATA_TYPE_GUID);
    entries[off + 16..off + 32].copy_from_slice(&npkfs_guid);
    put_u64(&mut entries, off + 32, NPKFS_START);
    put_u64(&mut entries, off + 40, npkfs_end);
    let npk_name = b"n\0p\0k\0F\0S\0";
    entries[off + 56..off + 56 + npk_name.len()].copy_from_slice(npk_name);

    let entries_crc = crc32(&entries);

    // === GPT Header (sector 1) ===
    let mut hdr = [0u8; 512];
    hdr[0..8].copy_from_slice(b"EFI PART");           // Signature
    put_u32(&mut hdr, 8, 0x0001_0000);                 // Revision 1.0
    put_u32(&mut hdr, 12, 92);                          // Header size
    // CRC32 at offset 16 — filled after
    put_u64(&mut hdr, 24, 1);                           // My LBA
    put_u64(&mut hdr, 32, last_sector);                 // Alternate LBA
    put_u64(&mut hdr, 40, 34);                          // First usable LBA
    put_u64(&mut hdr, 48, last_usable);                 // Last usable LBA
    hdr[56..72].copy_from_slice(&disk_guid);            // Disk GUID
    put_u64(&mut hdr, 72, 2);                           // Partition entries start
    put_u32(&mut hdr, 80, 128);                         // Number of entries
    put_u32(&mut hdr, 84, 128);                         // Entry size
    put_u32(&mut hdr, 88, entries_crc);                 // Entries CRC32
    put_u32(&mut hdr, 16, 0);                           // Zero CRC field
    let hdr_crc = crc32(&hdr[..92]);
    put_u32(&mut hdr, 16, hdr_crc);

    // === Protective MBR (sector 0) ===
    let mut mbr = [0u8; 512];
    mbr[446] = 0x00;                                    // Not bootable
    mbr[447] = 0x00; mbr[448] = 0x02; mbr[449] = 0x00; // CHS first
    mbr[450] = 0xEE;                                    // GPT protective
    mbr[451] = 0xFF; mbr[452] = 0xFF; mbr[453] = 0xFF; // CHS last
    put_u32(&mut mbr, 454, 1);                          // LBA start
    let mbr_size = if total_sectors > 0xFFFF_FFFF { 0xFFFF_FFFF } else { total_sectors as u32 - 1 };
    put_u32(&mut mbr, 458, mbr_size);                   // Sectors
    mbr[510] = 0x55; mbr[511] = 0xAA;

    // === Write primary GPT ===
    let mut sec = [0u8; 512];

    // Sector 0: Protective MBR
    nvme::write_sector(0, &mbr).map_err(|_| "write MBR")?;

    // Sector 1: GPT header
    nvme::write_sector(1, &hdr).map_err(|_| "write GPT header")?;

    // Sectors 2-33: Partition entries
    for i in 0..32u64 {
        let start = (i as usize) * 512;
        sec.copy_from_slice(&entries[start..start + 512]);
        nvme::write_sector(2 + i, &sec).map_err(|_| "write GPT entries")?;
    }

    // === Write backup GPT ===
    // Backup entries at last_sector - 32 .. last_sector - 1, right below the
    // backup header, where the header's entries LBA says they are.
    for i in 0..32u64 {
        let start = (i as usize) * 512;
        sec.copy_from_slice(&entries[start..start + 512]);
        nvme::write_sector(last_sector - 32 + i, &sec).map_err(|_| "write backup entries")?;
    }

    // Backup header at last_sector (swap my/alternate LBA)
    let mut backup_hdr = hdr;
    put_u64(&mut backup_hdr, 24, last_sector);          // My LBA = last
    put_u64(&mut backup_hdr, 32, 1);                    // Alternate = primary
    put_u64(&mut backup_hdr, 72, last_sector - 32);     // Entries start
    put_u32(&mut backup_hdr, 16, 0);
    let backup_crc = crc32(&backup_hdr[..92]);
    put_u32(&mut backup_hdr, 16, backup_crc);
    nvme::write_sector(last_sector, &backup_hdr).map_err(|_| "write backup header")?;

    Ok(NPKFS_START)
}
