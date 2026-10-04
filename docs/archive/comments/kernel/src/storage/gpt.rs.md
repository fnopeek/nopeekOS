# `kernel/src/storage/gpt.rs` @ 5e0102684

## L1-5 · `use crate::nvme;`

```
//! Minimal GPT writer for NVMe installation
//!
//! Creates a GUID Partition Table with two partitions:
//!   1. EFI System Partition (ESP) — 64 MB, FAT32
//!   2. npkFS data partition — rest of disk
```

## L12 · `pub const ESP_SECTORS: u64 = 131072;`

```
/// ESP size in sectors (64 MB)
```

## L14 · `pub const ESP_START: u64 = 2048;`

```
/// ESP starts at sector 2048 (1 MB alignment)
```

## L16 · `pub const NPKFS_START: u64 = ESP_START + ESP_SECTORS;`

```
/// npkFS partition starts right after ESP
```

## L19 · `const fn guid(d1: u32, d2: u16, d3: u16, d4: u16, d5: u64) -> [u8; 16] {`

```
/// Convert GUID components to mixed-endian byte array (as per GPT spec).
```

## L22 · `g[0] = d1 as u8; g[1] = (d1 >> 8) as u8;`

```
// d1: little-endian
```

## L25 · `g[4] = d2 as u8; g[5] = (d2 >> 8) as u8;`

```
// d2: little-endian
```

## L27 · `g[6] = d3 as u8; g[7] = (d3 >> 8) as u8;`

```
// d3: little-endian
```

## L29 · `g[8] = (d4 >> 8) as u8; g[9] = d4 as u8;`

```
// d4: big-endian
```

## L31 · `g[10] = (d5 >> 40) as u8; g[11] = (d5 >> 32) as u8;`

```
// d5: big-endian (6 bytes)
```

## L44 · `g[6] = (g[6] & 0x0F) | 0x40;`

```
// Set version 4 (random) and variant 1
```

## L73-74 · `pub fn detect_npkfs_offset() -> Option<u64> {`

```
/// Detect existing GPT and return npkFS partition block offset.
/// Returns Some(block_offset) if GPT found with 2+ partitions, None otherwise.
```

## L79-85 · `pub fn has_gpt_header() -> Option<bool> {`

```
/// Is there a GPT header on this device at all? `Some(true)` = yes,
/// `Some(false)` = read fine, no signature, `None` = the read failed.
///
/// `detect_npkfs_partition` collapses all three into `None`, and boot then
/// could not tell "raw disk, offset 0 is ours" from "our partition table is
/// there but I could not read it" — the second one must never lead to a
/// format at offset 0, which is where the GPT and the ESP live.
```

## L93-97 · `pub fn detect_npkfs_partition() -> Option<(u64, u64)> {`

```
/// Detect existing GPT and return npkFS partition (block_offset, block_count).
/// Both values are in 4 KB blocks. Used by the boot path so blkdev knows
/// both the start and the size of our partition — without the size,
/// allocations past the partition end land in the backup-GPT region
/// at the disk tail and hit `BlkError::OutOfRange`.
```

## L109 · `let off = 128;`

```
// Second partition entry starts at offset 128 within the sector.
```

## L126 · `let usable_sectors = end_lba + 1 - start_lba;`

```
// end_lba is inclusive. Block count = floor of usable sector count / 8.
```

## L132 · `pub fn detect_esp_offset() -> Option<u64> {`

```
/// Detect existing GPT and return ESP (EFI System Partition) sector offset.
```

## L139 · `if &hdr[0..8] != b"EFI PART" { return None; }`

```
// Check GPT signature
```

## L142 · `let mut entry_sec = [0u8; 512];`

```
// Read first partition entry (sector 2, offset 0 = first entry = ESP)
```

## L146 · `if &entry_sec[0..16] != &ESP_TYPE_GUID { return None; }`

```
// Check type GUID matches ESP
```

## L149 · `let start_lba = u64::from_le_bytes([`

```
// Read starting LBA (offset 32 within entry)
```

## L159 · `pub fn write_gpt() -> Result<u64, &'static str> {`

```
/// Write GPT to NVMe. Returns the sector where npkFS partition starts.
```

## L163 · `let last_usable = last_sector - 33; // backup GPT entries + header`

```
// backup GPT entries + header
```

## L170 · `let mut entries = [0u8; 512 * 32];`

```
// === Partition entries (128 bytes each, 128 entries = 32 sectors) ===
```

## L173 · `entries[0..16].copy_from_slice(&ESP_TYPE_GUID);`

```
// Entry 0: ESP
```

## L178 · `let esp_name = b"E\0F\0I\0 \0S\0y\0s\0t\0e\0m\0";`

```
// Name "EFI System" in UTF-16LE
```

## L182 · `let off = 128;`

```
// Entry 1: npkFS
```

## L193 · `let mut hdr = [0u8; 512];`

```
// === GPT Header (sector 1) ===
```

## L195 · `hdr[0..8].copy_from_slice(b"EFI PART");           // Signature`

```
// Signature
```

## L196 · `put_u32(&mut hdr, 8, 0x0001_0000);                 // Revision 1.0`

```
// Revision 1.0
```

## L197 · `put_u32(&mut hdr, 12, 92);                          // Header size`

```
// Header size
```

## L198 · `put_u64(&mut hdr, 24, 1);                           // My LBA`

```
// CRC32 at offset 16 — filled after
```

## L199 · `put_u64(&mut hdr, 24, 1);                           // My LBA`

```
// My LBA
```

## L200 · `put_u64(&mut hdr, 32, last_sector);                 // Alternate LBA`

```
// Alternate LBA
```

## L201 · `put_u64(&mut hdr, 40, 34);                          // First usable LBA`

```
// First usable LBA
```

## L202 · `put_u64(&mut hdr, 48, last_usable);                 // Last usable LBA`

```
// Last usable LBA
```

## L203 · `hdr[56..72].copy_from_slice(&disk_guid);            // Disk GUID`

```
// Disk GUID
```

## L204 · `put_u64(&mut hdr, 72, 2);                           // Partition entries start`

```
// Partition entries start
```

## L205 · `put_u32(&mut hdr, 80, 128);                         // Number of entries`

```
// Number of entries
```

## L206 · `put_u32(&mut hdr, 84, 128);                         // Entry size`

```
// Entry size
```

## L207 · `put_u32(&mut hdr, 88, entries_crc);                 // Entries CRC32`

```
// Entries CRC32
```

## L208 · `put_u32(&mut hdr, 16, 0);                           // Zero CRC field`

```
// Zero CRC field
```

## L212 · `let mut mbr = [0u8; 512];`

```
// === Protective MBR (sector 0) ===
```

## L214 · `mbr[446] = 0x00;                                    // Not bootable`

```
// Not bootable
```

## L215 · `mbr[447] = 0x00; mbr[448] = 0x02; mbr[449] = 0x00; // CHS first`

```
// CHS first
```

## L216 · `mbr[450] = 0xEE;                                    // GPT protective`

```
// GPT protective
```

## L217 · `mbr[451] = 0xFF; mbr[452] = 0xFF; mbr[453] = 0xFF; // CHS last`

```
// CHS last
```

## L218 · `put_u32(&mut mbr, 454, 1);                          // LBA start`

```
// LBA start
```

## L220 · `put_u32(&mut mbr, 458, mbr_size);                   // Sectors`

```
// Sectors
```

## L223 · `let mut sec = [0u8; 512];`

```
// === Write primary GPT ===
```

## L226 · `nvme::write_sector(0, &mbr).map_err(|_| "write MBR")?;`

```
// Sector 0: Protective MBR
```

## L229 · `nvme::write_sector(1, &hdr).map_err(|_| "write GPT header")?;`

```
// Sector 1: GPT header
```

## L232 · `for i in 0..32u64 {`

```
// Sectors 2-33: Partition entries
```

## L239-240 · `for i in 0..32u64 {`

```
// === Write backup GPT ===
// Backup entries at last_sector - 33 .. last_sector - 2
```

## L247 · `let mut backup_hdr = hdr;`

```
// Backup header at last_sector (swap my/alternate LBA)
```

## L249 · `put_u64(&mut backup_hdr, 24, last_sector);          // My LBA = last`

```
// My LBA = last
```

## L250 · `put_u64(&mut backup_hdr, 32, 1);                    // Alternate = primary`

```
// Alternate = primary
```

## L251 · `put_u64(&mut backup_hdr, 72, last_sector - 32);     // Entries start`

```
// Entries start
```

