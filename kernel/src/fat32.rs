//! Minimal FAT32 writer for EFI System Partition
//!
//! Creates a FAT32 filesystem and writes GRUB EFI, kernel, and grub.cfg.
//! Write-once, no modify/delete needed.

use crate::nvme;

const SECTOR_SIZE: usize = 512;
const RESERVED_SECTORS: u32 = 32;
const NUM_FATS: u8 = 2;
const SECTORS_PER_CLUSTER: u32 = 1;
const ROOT_CLUSTER: u32 = 2;

/// FAT32 entry: end of chain
const FAT_EOC: u32 = 0x0FFF_FFFF;

struct Fat32Writer {
    /// Absolute start sector on NVMe
    part_start: u64,
    /// Total sectors in partition
    total_sectors: u32,
    /// Sectors per FAT
    fat_size: u32,
    /// First data sector (relative to partition start)
    data_start: u32,
    /// Next free cluster
    next_cluster: u32,
}

impl Fat32Writer {
    fn new(part_start: u64, total_sectors: u32) -> Self {
        // Calculate FAT size: each cluster needs 4 bytes in FAT
        // data_clusters ≈ (total - reserved - 2*fat) / spc
        // fat_size = ceil(data_clusters * 4 / 512)
        // Approximate: over-allocate FAT, it's fine
        let fat_size = ((total_sectors as u64 * 4 + 511) / 512) as u32;
        let data_start = RESERVED_SECTORS + NUM_FATS as u32 * fat_size;

        Fat32Writer {
            part_start,
            total_sectors,
            fat_size,
            data_start,
            next_cluster: ROOT_CLUSTER,
        }
    }

    /// Write a sector at absolute NVMe position
    fn write_sector(&self, rel_sector: u32, data: &[u8; SECTOR_SIZE]) -> Result<(), &'static str> {
        nvme::write_sector(self.part_start + rel_sector as u64, data)
            .map_err(|_| "FAT32 write error")
    }

    /// Sector number for a given cluster
    fn cluster_to_sector(&self, cluster: u32) -> u32 {
        self.data_start + (cluster - 2) * SECTORS_PER_CLUSTER
    }

    /// Allocate N contiguous clusters starting from next_cluster.
    /// Returns the first cluster number.
    fn alloc_clusters(&mut self, count: u32) -> u32 {
        let first = self.next_cluster;
        self.next_cluster += count;
        first
    }

    /// Write the FAT32 boot sector (BPB) and FSInfo
    fn write_boot_sector(&self) -> Result<(), &'static str> {
        let mut bs = [0u8; 512];

        // Jump boot code
        bs[0] = 0xEB; bs[1] = 0x58; bs[2] = 0x90;
        // OEM name
        bs[3..11].copy_from_slice(b"NOPEEKOS");
        // BPB
        bs[11..13].copy_from_slice(&512u16.to_le_bytes());       // BytsPerSec
        bs[13] = SECTORS_PER_CLUSTER as u8;                       // SecPerClus
        bs[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes()); // RsvdSecCnt
        bs[16] = NUM_FATS;                                        // NumFATs
        bs[17..19].copy_from_slice(&0u16.to_le_bytes());          // RootEntCnt (0 for FAT32)
        bs[19..21].copy_from_slice(&0u16.to_le_bytes());          // TotSec16 (0, use 32)
        bs[21] = 0xF8;                                            // Media
        bs[22..24].copy_from_slice(&0u16.to_le_bytes());          // FATSz16 (0, use 32)
        bs[24..26].copy_from_slice(&63u16.to_le_bytes());         // SecPerTrk
        bs[26..28].copy_from_slice(&255u16.to_le_bytes());        // NumHeads
        bs[28..32].copy_from_slice(&(self.part_start as u32).to_le_bytes()); // HiddSec
        bs[32..36].copy_from_slice(&self.total_sectors.to_le_bytes()); // TotSec32
        // FAT32 specific
        bs[36..40].copy_from_slice(&self.fat_size.to_le_bytes()); // FATSz32
        bs[40..42].copy_from_slice(&0u16.to_le_bytes());          // ExtFlags
        bs[42..44].copy_from_slice(&0u16.to_le_bytes());          // FSVer
        bs[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());  // RootClus
        bs[48..50].copy_from_slice(&1u16.to_le_bytes());          // FSInfo sector
        bs[50..52].copy_from_slice(&6u16.to_le_bytes());          // BkBootSec
        // Drive number, boot sig, volume info
        bs[64] = 0x80;                                            // DrvNum
        bs[66] = 0x29;                                            // BootSig
        // Volume serial (random)
        let serial = crate::csprng::random_u64() as u32;
        bs[67..71].copy_from_slice(&serial.to_le_bytes());
        bs[71..82].copy_from_slice(b"NOPEEKOS   ");              // VolLab
        bs[82..90].copy_from_slice(b"FAT32   ");                  // FilSysType
        bs[510] = 0x55; bs[511] = 0xAA;

        // Write boot sector
        self.write_sector(0, &bs)?;
        // Write backup boot sector at sector 6
        self.write_sector(6, &bs)?;

        // FSInfo sector (sector 1)
        let mut fsi = [0u8; 512];
        fsi[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());     // LeadSig
        fsi[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes()); // StrucSig
        let free = (self.total_sectors - self.data_start) / SECTORS_PER_CLUSTER - 2;
        fsi[488..492].copy_from_slice(&free.to_le_bytes());            // Free_Count
        fsi[492..496].copy_from_slice(&self.next_cluster.to_le_bytes()); // Nxt_Free
        fsi[510] = 0x55; fsi[511] = 0xAA;
        self.write_sector(1, &fsi)?;

        Ok(())
    }

    /// Write a FAT entry. Writes to both FAT copies.
    fn write_fat_entry(&self, cluster: u32, value: u32) -> Result<(), &'static str> {
        let offset_bytes = cluster as u64 * 4;
        let fat_sector = (offset_bytes / 512) as u32;
        let fat_offset = (offset_bytes % 512) as usize;

        for fat_num in 0..NUM_FATS as u32 {
            let abs_sector = RESERVED_SECTORS + fat_num * self.fat_size + fat_sector;

            // Read existing sector
            let mut sec = [0u8; 512];
            nvme::read_sector(self.part_start + abs_sector as u64, &mut sec)
                .map_err(|_| "FAT read")?;

            sec[fat_offset..fat_offset + 4].copy_from_slice(&value.to_le_bytes());

            self.write_sector(abs_sector, &sec)?;
        }
        Ok(())
    }

    /// Write FAT chain for contiguous clusters
    fn write_fat_chain(&self, first: u32, count: u32) -> Result<(), &'static str> {
        for i in 0..count {
            let cluster = first + i;
            let value = if i == count - 1 { FAT_EOC } else { cluster + 1 };
            self.write_fat_entry(cluster, value)?;
        }
        Ok(())
    }

    /// Initialize FAT: entries 0 and 1
    fn init_fat(&self) -> Result<(), &'static str> {
        // Zero out reserved sectors area (clean slate)
        let zero = [0u8; 512];
        for s in 0..RESERVED_SECTORS {
            self.write_sector(s, &zero)?;
        }
        // Zero out FAT sectors
        for fat_num in 0..NUM_FATS as u32 {
            for s in 0..self.fat_size {
                self.write_sector(RESERVED_SECTORS + fat_num * self.fat_size + s, &zero)?;
            }
        }

        self.write_fat_entry(0, 0x0FFF_FFF8)?; // Media byte
        self.write_fat_entry(1, FAT_EOC)?;       // Reserved
        Ok(())
    }

    /// Create a directory entry (32 bytes)
    fn make_dir_entry(name: &[u8; 11], attr: u8, cluster: u32, size: u32) -> [u8; 32] {
        let mut e = [0u8; 32];
        e[0..11].copy_from_slice(name);
        e[11] = attr;
        // First cluster low
        e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        // First cluster high
        e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        // File size
        e[28..32].copy_from_slice(&size.to_le_bytes());
        e
    }

    /// Write a directory sector with given entries
    fn write_directory(&self, cluster: u32, entries: &[[u8; 32]]) -> Result<(), &'static str> {
        let mut sec = [0u8; 512];
        for (i, entry) in entries.iter().enumerate() {
            let off = i * 32;
            if off + 32 > 512 { break; }
            sec[off..off + 32].copy_from_slice(entry);
        }
        let sector = self.cluster_to_sector(cluster);
        self.write_sector(sector, &sec)
    }

    /// Write file data starting at given cluster
    fn write_file_data(&self, first_cluster: u32, data: &[u8]) -> Result<(), &'static str> {
        let sectors = (data.len() + 511) / 512;
        for i in 0..sectors {
            let mut sec = [0u8; 512];
            let start = i * 512;
            let end = (start + 512).min(data.len());
            sec[..end - start].copy_from_slice(&data[start..end]);
            let sector = self.cluster_to_sector(first_cluster + i as u32);
            self.write_sector(sector, &sec)?;
        }
        Ok(())
    }
}

/// 8.3 filename from components (name max 8 chars, ext max 3 chars)
fn make_name(name: &[u8], ext: &[u8]) -> [u8; 11] {
    let mut n = [b' '; 11];
    for (i, &b) in name.iter().enumerate().take(8) {
        n[i] = b.to_ascii_uppercase();
    }
    for (i, &b) in ext.iter().enumerate().take(3) {
        n[8 + i] = b.to_ascii_uppercase();
    }
    n
}

/// Create FAT32 ESP and write GRUB + kernel + config.
/// `part_start`: absolute sector on NVMe where ESP begins
/// `part_sectors`: size of ESP in sectors
pub fn create_esp(
    part_start: u64,
    part_sectors: u32,
    grub_efi: &[u8],
    kernel: &[u8],
    grub_cfg: &[u8],
) -> Result<(), &'static str> {
    let mut fs = Fat32Writer::new(part_start, part_sectors);

    // Initialize filesystem structures
    fs.init_fat()?;
    fs.write_boot_sector()?;

    // Allocate directory clusters
    let root_cl = fs.alloc_clusters(1);   // cluster 2: root dir
    let efi_cl = fs.alloc_clusters(1);    // cluster 3: /EFI
    let efiboot_cl = fs.alloc_clusters(1); // cluster 4: /EFI/BOOT
    let boot_cl = fs.alloc_clusters(1);    // cluster 5: /boot
    let grubdir_cl = fs.alloc_clusters(1); // cluster 6: /boot/grub

    // Allocate file clusters
    let grub_sectors = ((grub_efi.len() + 511) / 512) as u32;
    let grub_cl = fs.alloc_clusters(grub_sectors);

    let kernel_sectors = ((kernel.len() + 511) / 512) as u32;
    let kernel_cl = fs.alloc_clusters(kernel_sectors);

    let cfg_cl = fs.alloc_clusters(1);

    // Write FAT chains
    fs.write_fat_chain(root_cl, 1)?;
    fs.write_fat_chain(efi_cl, 1)?;
    fs.write_fat_chain(efiboot_cl, 1)?;
    fs.write_fat_chain(boot_cl, 1)?;
    fs.write_fat_chain(grubdir_cl, 1)?;
    fs.write_fat_chain(grub_cl, grub_sectors)?;
    fs.write_fat_chain(kernel_cl, kernel_sectors)?;
    fs.write_fat_chain(cfg_cl, 1)?;

    // Write directories
    // Root: /EFI, /boot, marker
    fs.write_directory(root_cl, &[
        Fat32Writer::make_dir_entry(&make_name(b"EFI", b""), 0x10, efi_cl, 0),
        Fat32Writer::make_dir_entry(&make_name(b"BOOT", b""), 0x10, boot_cl, 0),
        Fat32Writer::make_dir_entry(&make_name(b"NPKBOOT", b""), 0x20, 0, 0),
    ])?;

    // /EFI: ., .., BOOT/
    fs.write_directory(efi_cl, &[
        Fat32Writer::make_dir_entry(b".          ", 0x10, efi_cl, 0),
        Fat32Writer::make_dir_entry(b"..         ", 0x10, 0, 0),
        Fat32Writer::make_dir_entry(&make_name(b"BOOT", b""), 0x10, efiboot_cl, 0),
    ])?;

    // /EFI/BOOT: ., .., BOOTX64.EFI
    fs.write_directory(efiboot_cl, &[
        Fat32Writer::make_dir_entry(b".          ", 0x10, efiboot_cl, 0),
        Fat32Writer::make_dir_entry(b"..         ", 0x10, efi_cl, 0),
        Fat32Writer::make_dir_entry(
            &make_name(b"BOOTX64", b"EFI"), 0x20, grub_cl, grub_efi.len() as u32,
        ),
    ])?;

    // /boot: ., .., grub/, kernel.bin
    fs.write_directory(boot_cl, &[
        Fat32Writer::make_dir_entry(b".          ", 0x10, boot_cl, 0),
        Fat32Writer::make_dir_entry(b"..         ", 0x10, 0, 0),
        Fat32Writer::make_dir_entry(&make_name(b"GRUB", b""), 0x10, grubdir_cl, 0),
        Fat32Writer::make_dir_entry(
            &make_name(b"KERNEL", b"BIN"), 0x20, kernel_cl, kernel.len() as u32,
        ),
    ])?;

    // /boot/grub: ., .., grub.cfg
    fs.write_directory(grubdir_cl, &[
        Fat32Writer::make_dir_entry(b".          ", 0x10, grubdir_cl, 0),
        Fat32Writer::make_dir_entry(b"..         ", 0x10, boot_cl, 0),
        Fat32Writer::make_dir_entry(
            &make_name(b"GRUB", b"CFG"), 0x20, cfg_cl, grub_cfg.len() as u32,
        ),
    ])?;

    // Write file data
    fs.write_file_data(grub_cl, grub_efi)?;
    fs.write_file_data(kernel_cl, kernel)?;
    fs.write_file_data(cfg_cl, grub_cfg)?;

    Ok(())
}

// ============================================================
// FAT32 Reader — for OTA update (find + overwrite kernel.bin)
// ============================================================

struct Fat32Reader {
    /// Absolute start sector of ESP on NVMe
    part_start: u64,
    /// Sectors per FAT
    fat_size: u32,
    /// First data sector (relative to partition start)
    data_start: u32,
    /// Sectors per cluster
    spc: u32,
}

impl Fat32Reader {
    /// Parse the boot sector (BPB) to initialize the reader.
    fn from_esp(part_start: u64) -> Result<Self, &'static str> {
        let mut bs = [0u8; 512];
        nvme::read_sector(part_start, &mut bs).map_err(|_| "ESP read error")?;

        // Verify FAT32 signature
        if bs[510] != 0x55 || bs[511] != 0xAA { return Err("ESP: bad signature"); }

        let spc = bs[13] as u32;
        if spc == 0 { return Err("ESP: bad SPC"); }
        let reserved = u16::from_le_bytes([bs[14], bs[15]]) as u32;
        let num_fats = bs[16] as u32;
        let fat_size = u32::from_le_bytes([bs[36], bs[37], bs[38], bs[39]]);
        let data_start = reserved + num_fats * fat_size;

        Ok(Fat32Reader { part_start, fat_size, data_start, spc })
    }

    fn read_sector(&self, rel_sector: u32, buf: &mut [u8; 512]) -> Result<(), &'static str> {
        nvme::read_sector(self.part_start + rel_sector as u64, buf)
            .map_err(|_| "FAT32 read error")
    }

    fn write_sector(&self, rel_sector: u32, buf: &[u8; 512]) -> Result<(), &'static str> {
        nvme::write_sector(self.part_start + rel_sector as u64, buf)
            .map_err(|_| "FAT32 write error")
    }

    fn cluster_to_sector(&self, cluster: u32) -> u32 {
        self.data_start + (cluster - 2) * self.spc
    }

    /// Read next cluster from FAT chain.
    fn fat_next(&self, cluster: u32) -> Result<Option<u32>, &'static str> {
        let offset_bytes = cluster as u64 * 4;
        let fat_sector = RESERVED_SECTORS + (offset_bytes / 512) as u32;
        let fat_offset = (offset_bytes % 512) as usize;

        let mut sec = [0u8; 512];
        self.read_sector(fat_sector, &mut sec)?;
        let val = u32::from_le_bytes([
            sec[fat_offset], sec[fat_offset + 1],
            sec[fat_offset + 2], sec[fat_offset + 3],
        ]) & 0x0FFF_FFFF;

        if val >= 0x0FFF_FFF8 { Ok(None) } else { Ok(Some(val)) }
    }

    /// Write a FAT entry (both copies).
    fn fat_write(&self, cluster: u32, value: u32) -> Result<(), &'static str> {
        let offset_bytes = cluster as u64 * 4;
        let fat_sector = (offset_bytes / 512) as u32;
        let fat_offset = (offset_bytes % 512) as usize;

        for fat_num in 0..2u32 {
            let abs = RESERVED_SECTORS + fat_num * self.fat_size + fat_sector;
            let mut sec = [0u8; 512];
            self.read_sector(abs, &mut sec)?;
            sec[fat_offset..fat_offset + 4].copy_from_slice(&value.to_le_bytes());
            self.write_sector(abs, &sec)?;
        }
        Ok(())
    }

    /// Find a directory entry by 8.3 name in a given directory cluster.
    /// Returns (first_cluster, file_size, dir_sector, entry_offset).
    fn find_entry(&self, dir_cluster: u32, name: &[u8; 11])
        -> Result<(u32, u32, u32, usize), &'static str>
    {
        let mut cluster = dir_cluster;
        loop {
            let sector = self.cluster_to_sector(cluster);
            for s in 0..self.spc {
                let mut sec = [0u8; 512];
                self.read_sector(sector + s, &mut sec)?;
                for i in 0..16 { // 16 entries per sector
                    let off = i * 32;
                    if sec[off] == 0x00 { return Err("entry not found"); }
                    if sec[off] == 0xE5 { continue; } // deleted
                    if &sec[off..off + 11] == name {
                        let cl_lo = u16::from_le_bytes([sec[off + 26], sec[off + 27]]) as u32;
                        let cl_hi = u16::from_le_bytes([sec[off + 20], sec[off + 21]]) as u32;
                        let first_cl = (cl_hi << 16) | cl_lo;
                        let size = u32::from_le_bytes([
                            sec[off + 28], sec[off + 29], sec[off + 30], sec[off + 31],
                        ]);
                        return Ok((first_cl, size, sector + s, off));
                    }
                }
            }
            match self.fat_next(cluster)? {
                Some(next) => cluster = next,
                None => return Err("entry not found"),
            }
        }
    }

    /// Count clusters in a FAT chain.
    fn chain_len(&self, first: u32) -> Result<u32, &'static str> {
        let mut count = 0;
        let mut cl = first;
        loop {
            count += 1;
            match self.fat_next(cl)? {
                Some(next) => cl = next,
                None => return Ok(count),
            }
            if count > 100_000 { return Err("FAT chain too long"); }
        }
    }

    /// Update directory entry size field.
    fn update_entry_size(&self, dir_sector: u32, entry_offset: usize, new_size: u32)
        -> Result<(), &'static str>
    {
        let mut sec = [0u8; 512];
        self.read_sector(dir_sector, &mut sec)?;
        sec[entry_offset + 28..entry_offset + 32].copy_from_slice(&new_size.to_le_bytes());
        self.write_sector(dir_sector, &sec)
    }
}

/// Update kernel.bin on the ESP partition.
/// Finds the existing file, overwrites its data, extends FAT chain if needed.
pub fn update_kernel(esp_start: u64, data: &[u8]) -> Result<(), &'static str> {
    let fs = Fat32Reader::from_esp(esp_start)?;

    // Navigate: root (cluster 2) → /boot → KERNEL.BIN
    let boot_name = make_name(b"BOOT", b"");
    let (boot_cl, _, _, _) = fs.find_entry(ROOT_CLUSTER, &boot_name)?;

    let kernel_name = make_name(b"KERNEL", b"BIN");
    let (kernel_cl, old_size, dir_sec, dir_off) = fs.find_entry(boot_cl, &kernel_name)?;

    let new_sectors = ((data.len() + 511) / 512) as u32;
    let _old_clusters = fs.chain_len(kernel_cl)?;
    let new_clusters = (new_sectors + fs.spc - 1) / fs.spc;

    crate::kprintln!("[npk] ESP: kernel.bin at cluster {}, {} -> {} bytes",
        kernel_cl, old_size, data.len());

    // Write data to existing clusters (follow FAT chain)
    let mut cl = kernel_cl;
    let mut written = 0usize;
    let mut clusters_used = 0u32;
    loop {
        let sector = fs.cluster_to_sector(cl);
        for s in 0..fs.spc {
            if written >= data.len() {
                // Zero-pad remaining sectors in this cluster
                let zero = [0u8; 512];
                fs.write_sector(sector + s, &zero)?;
            } else {
                let mut sec = [0u8; 512];
                let end = (written + 512).min(data.len());
                sec[..end - written].copy_from_slice(&data[written..end]);
                fs.write_sector(sector + s, &sec)?;
                written = end;
            }
        }
        clusters_used += 1;

        if clusters_used >= new_clusters && written >= data.len() {
            // Mark this as end of chain
            fs.fat_write(cl, FAT_EOC)?;
            // Free remaining old clusters
            if let Ok(Some(next)) = fs.fat_next(cl) {
                free_chain(&fs, next)?;
            }
            break;
        }

        match fs.fat_next(cl)? {
            Some(next) => cl = next,
            None => {
                // Need more clusters — allocate after last known cluster
                if clusters_used < new_clusters {
                    let new_cl = cl + 1; // simple next-fit allocation
                    fs.fat_write(cl, new_cl)?;
                    fs.fat_write(new_cl, FAT_EOC)?;
                    cl = new_cl;
                } else {
                    break;
                }
            }
        }
    }

    // Update directory entry with new file size
    fs.update_entry_size(dir_sec, dir_off, data.len() as u32)?;

    Ok(())
}

/// Free a FAT chain starting at the given cluster.
fn free_chain(fs: &Fat32Reader, start: u32) -> Result<(), &'static str> {
    let mut cl = start;
    loop {
        let next = fs.fat_next(cl)?;
        fs.fat_write(cl, 0)?; // Mark as free
        match next {
            Some(n) => cl = n,
            None => break,
        }
    }
    Ok(())
}
