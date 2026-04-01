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
