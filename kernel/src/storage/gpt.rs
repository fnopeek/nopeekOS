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
    if !nvme::is_available() { return None; }

    let mut hdr = [0u8; 512];
    nvme::read_sector(1, &mut hdr).ok()?;

    // Check GPT signature
    if &hdr[0..8] != b"EFI PART" { return None; }

    // Read first partition entry (sector 2, offset 128 = second entry)
    let mut entry_sec = [0u8; 512];
    nvme::read_sector(2, &mut entry_sec).ok()?;

    // Second partition entry starts at offset 128
    let off = 128;
    // Check type GUID is not all zeros (partition exists)
    let type_guid = &entry_sec[off..off + 16];
    if type_guid == &[0u8; 16] { return None; }

    // Read starting LBA (offset 32 within entry)
    let start_lba = u64::from_le_bytes([
        entry_sec[off + 32], entry_sec[off + 33], entry_sec[off + 34], entry_sec[off + 35],
        entry_sec[off + 36], entry_sec[off + 37], entry_sec[off + 38], entry_sec[off + 39],
    ]);

    if start_lba == 0 { return None; }

    // Convert sectors to 4KB block offset
    Some(start_lba / 8)
}

/// Detect existing GPT and return ESP (EFI System Partition) sector offset.
pub fn detect_esp_offset() -> Option<u64> {
    if !nvme::is_available() { return None; }

    let mut hdr = [0u8; 512];
    nvme::read_sector(1, &mut hdr).ok()?;

    // Check GPT signature
    if &hdr[0..8] != b"EFI PART" { return None; }

    // Read first partition entry (sector 2, offset 0 = first entry = ESP)
    let mut entry_sec = [0u8; 512];
    nvme::read_sector(2, &mut entry_sec).ok()?;

    // Check type GUID matches ESP
    if &entry_sec[0..16] != &ESP_TYPE_GUID { return None; }

    // Read starting LBA (offset 32 within entry)
    let start_lba = u64::from_le_bytes([
        entry_sec[32], entry_sec[33], entry_sec[34], entry_sec[35],
        entry_sec[36], entry_sec[37], entry_sec[38], entry_sec[39],
    ]);

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
    // Backup entries at last_sector - 33 .. last_sector - 2
    for i in 0..32u64 {
        let start = (i as usize) * 512;
        sec.copy_from_slice(&entries[start..start + 512]);
        nvme::write_sector(last_sector - 33 + i, &sec).map_err(|_| "write backup entries")?;
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
