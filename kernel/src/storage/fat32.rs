//! Minimal FAT32 support for the EFI System Partition
//!
//! Creates the ESP at install, overwrites `/EFI/BOOT/BOOTX64.EFI` during an
//! OTA update, and keeps small files in `/EFI/NOPEEK` that must be readable
//! before npkFS is unlocked. Everything read back from the ESP is untrusted.

use crate::nvme;

const SECTOR_SIZE: usize = 512;
const RESERVED_SECTORS: u32 = 32;
const NUM_FATS: u8 = 2;
const SECTORS_PER_CLUSTER: u32 = 1;
const ROOT_CLUSTER: u32 = 2;
/// Floor for the kernel's reserved cluster run (16 MiB at 512 B/cluster).
/// Generous on purpose: the ESP is 64 MB and re-laying-out the FAT during an
/// OTA is not something we want to implement.
const MIN_KERNEL_RESERVED: u32 = 32768;

/// FAT32 entry: end of chain
const FAT_EOC: u32 = 0x0FFF_FFFF;

const ATTR_VOLUME: u8 = 0x08;
const ATTR_DIR: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
/// Long-file-name entries carry all four low attribute bits.
const ATTR_LFN: u8 = 0x0F;

/// Largest file `write_small_file` stores: one sector, so it fits any cluster.
pub const SMALL_FILE_MAX: usize = 512;

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

    /// Write file data starting at `first_cluster`, refusing to run past the
    /// `max_clusters` that were reserved for it.
    ///
/// Without that bound, the directory entry would advertise the full size
/// while the chain stops short, so UEFI firmware following the chain reads
/// a truncated image and refuses to boot. Fail loudly instead.
    fn write_file_data(
        &self,
        first_cluster: u32,
        data: &[u8],
        max_clusters: u32,
    ) -> Result<(), &'static str> {
        let sectors = (data.len() + 511) / 512;
        let needed = (sectors as u32 + SECTORS_PER_CLUSTER - 1) / SECTORS_PER_CLUSTER;
        if needed > max_clusters {
            return Err("file larger than its reserved cluster chain");
        }
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

/// Create FAT32 ESP and write the UEFI bootloader.
/// `part_start`: absolute sector on NVMe where ESP begins
/// `part_sectors`: size of ESP in sectors
/// `kernel_efi`: the kernel as a UEFI PE+ application (built by
///     `objcopy -O pei-x86-64 --subsystem=efi-app`). UEFI firmware
///     auto-discovers `/EFI/BOOT/BOOTX64.EFI` — no GRUB needed since
///     our kernel IS the EFI application.
///
/// Layout written:
/// ```text
/// ESP/
/// └── EFI/
///     └── BOOT/
///         └── BOOTX64.EFI   ← kernel_efi
/// ```
pub fn create_esp(
    part_start: u64,
    part_sectors: u32,
    kernel_efi: &[u8],
) -> Result<(), &'static str> {
    let mut fs = Fat32Writer::new(part_start, part_sectors);

    fs.init_fat()?;
    fs.write_boot_sector()?;

    // Directory clusters
    let root_cl = fs.alloc_clusters(1);   // cluster 2: root
    let efi_cl = fs.alloc_clusters(1);    // cluster 3: /EFI
    let efiboot_cl = fs.alloc_clusters(1); // cluster 4: /EFI/BOOT

    // Reserve a contiguous run for the kernel image, so a later OTA can
    // overwrite it in place without re-laying-out the FAT.
    //
    // Derived from the image (double the current size, with a floor) so the
    // reservation leaves room for the kernel to grow.
    let kernel_clusters = ((kernel_efi.len() + 511) / 512) as u32;
    let reserved = (kernel_clusters.saturating_mul(2)).max(MIN_KERNEL_RESERVED);

    // The FAT itself only spans so many clusters; refuse rather than lay down
    // a filesystem whose chains point past the data area.
    let usable = fs.total_sectors.saturating_sub(fs.data_start) / SECTORS_PER_CLUSTER;
    if reserved + 3 >= usable {
        return Err("ESP too small for the kernel image");
    }
    let kernel_cl = fs.alloc_clusters(reserved);

    // FAT chains
    fs.write_fat_chain(root_cl, 1)?;
    fs.write_fat_chain(efi_cl, 1)?;
    fs.write_fat_chain(efiboot_cl, 1)?;
    fs.write_fat_chain(kernel_cl, reserved)?;

    // Root: /EFI, marker file
    fs.write_directory(root_cl, &[
        Fat32Writer::make_dir_entry(&make_name(b"EFI", b""), 0x10, efi_cl, 0),
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
            &make_name(b"BOOTX64", b"EFI"), 0x20, kernel_cl, kernel_efi.len() as u32,
        ),
    ])?;

    fs.write_file_data(kernel_cl, kernel_efi, reserved)?;

    Ok(())
}

// ============================================================
// FAT32 Reader — for OTA update (find + overwrite kernel.bin)
// ============================================================

struct Fat32Reader {
    /// Absolute start sector of ESP on NVMe
    part_start: u64,
    /// Reserved sectors before the first FAT (BPB RsvdSecCnt)
    reserved: u32,
    /// Number of FAT copies (BPB NumFATs)
    num_fats: u32,
    /// Sectors per FAT
    fat_size: u32,
    /// First data sector (relative to partition start)
    data_start: u32,
    /// Sectors per cluster
    spc: u32,
    /// Total sectors in the partition (BPB TotSec32) — the bound for
    /// allocating additional clusters.
    total_sectors: u32,
    /// First cluster of the root directory (BPB RootClus)
    root_cluster: u32,
    /// Highest cluster number that exists in the data area and in the FAT.
    max_cluster: u32,
}

/// One 32-byte directory entry, located on disk.
#[derive(Clone, Copy)]
struct DirEntry {
    first_cluster: u32,
    size: u32,
    attr: u8,
    /// Directory cluster holding the entry, so a write can be checked against
    /// the clusters that must not be touched.
    cluster: u32,
    sector: u32,
    offset: usize,
}

struct DirScan {
    found: Option<DirEntry>,
    /// First free slot (0x00 or 0xE5); its cluster/sector/offset are valid,
    /// the other fields are zero.
    free: Option<DirEntry>,
}

/// Clusters `write_small_file` must never write or allocate.
struct Protected {
    /// BOOTX64.EFI's reserved run: `[start, end)`.
    run_start: u32,
    run_end: u32,
    /// Root, /EFI and /EFI/BOOT.
    dirs: [u32; 3],
}

impl Protected {
    fn in_run(&self, c: u32) -> bool {
        c >= self.run_start && c < self.run_end
    }

    /// A directory cluster whose entries may be rewritten.
    fn dir_writable(&self, fs: &Fat32Reader, c: u32) -> bool {
        fs.valid_cluster(c) && !self.in_run(c)
    }

    /// A cluster that may hold small-file or NOPEEK-directory contents.
    fn data_writable(&self, fs: &Fat32Reader, c: u32) -> bool {
        fs.valid_cluster(c) && !self.in_run(c) && !self.dirs.contains(&c)
    }
}

impl Fat32Reader {
    /// Parse the boot sector (BPB) to initialize the reader.
    fn from_esp(part_start: u64) -> Result<Self, &'static str> {
        let mut bs = [0u8; 512];
        nvme::read_sector(part_start, &mut bs).map_err(|_| "ESP read error")?;

        // Verify FAT32 signature
        if bs[510] != 0x55 || bs[511] != 0xAA { return Err("ESP: bad signature"); }

        if u16::from_le_bytes([bs[11], bs[12]]) as usize != SECTOR_SIZE {
            return Err("ESP: sector size is not 512");
        }
        let spc = bs[13] as u32;
        if spc == 0 || !spc.is_power_of_two() { return Err("ESP: bad SPC"); }
        let reserved = u16::from_le_bytes([bs[14], bs[15]]) as u32;
        if reserved == 0 { return Err("ESP: no reserved sectors"); }
        let num_fats = bs[16] as u32;
        if num_fats == 0 || num_fats > 4 { return Err("ESP: bad FAT count"); }
        let fat_size = u32::from_le_bytes([bs[36], bs[37], bs[38], bs[39]]);
        if fat_size == 0 { return Err("ESP: not FAT32"); }
        let total_sectors = u32::from_le_bytes([bs[32], bs[33], bs[34], bs[35]]);
        let data_start = num_fats.checked_mul(fat_size)
            .and_then(|f| f.checked_add(reserved))
            .filter(|&d| d < total_sectors)
            .ok_or("ESP: FATs overrun the partition")?;
        let clusters = (total_sectors - data_start) / spc;
        // The FAT may describe fewer clusters than the data area holds; only
        // clusters with a FAT entry exist. 0x0FFF_FFF7 and up are markers.
        let fat_entries = fat_size.saturating_mul((SECTOR_SIZE / 4) as u32);
        let max_cluster = (clusters + 1).min(fat_entries.saturating_sub(1)).min(0x0FFF_FFF6);
        let root_cluster = u32::from_le_bytes([bs[44], bs[45], bs[46], bs[47]]);
        if max_cluster < 2 || root_cluster < 2 || root_cluster > max_cluster {
            return Err("ESP: bad root cluster");
        }

        Ok(Fat32Reader {
            part_start, reserved, num_fats, fat_size, data_start, spc, total_sectors,
            root_cluster, max_cluster,
        })
    }

    fn valid_cluster(&self, cluster: u32) -> bool {
        cluster >= 2 && cluster <= self.max_cluster
    }

    /// Sector (relative to the first FAT) and byte offset of a FAT entry.
    fn fat_pos(&self, cluster: u32) -> Result<(u32, usize), &'static str> {
        if cluster > self.max_cluster { return Err("ESP: cluster past the FAT"); }
        let offset = cluster as usize * 4;
        Ok(((offset / SECTOR_SIZE) as u32, offset % SECTOR_SIZE))
    }

    fn read_sector(&self, rel_sector: u32, buf: &mut [u8; 512]) -> Result<(), &'static str> {
        nvme::read_sector(self.part_start + rel_sector as u64, buf)
            .map_err(|_| "FAT32 read error")
    }

    /// Every raw block write on this path lands outside npkFS, so a sector
    /// number past the partition writes into whatever follows it, which is
    /// the filesystem. The chain is followed through the on-disk FAT, and
    /// one bad entry there can send `cluster_to_sector` anywhere. Refusing is
    /// always better than scribbling: a failed kernel update leaves the old
    /// one bootable, a scribbled npkFS does not.
    fn write_sector(&self, rel_sector: u32, buf: &[u8; 512]) -> Result<(), &'static str> {
        if rel_sector >= self.total_sectors {
            crate::kprintln!(
                "[npk] ESP: REFUSING write at sector {} — partition holds {}",
                rel_sector, self.total_sectors);
            return Err("ESP write past the end of the partition");
        }
        nvme::write_sector(self.part_start + rel_sector as u64, buf)
            .map_err(|_| "FAT32 write error")
    }

    fn cluster_to_sector(&self, cluster: u32) -> u32 {
        // Saturating: cluster 0/1 are reserved and would underflow, and an
        // overflow would wrap into a plausible-looking sector.
        self.data_start
            .saturating_add(cluster.saturating_sub(2).saturating_mul(self.spc))
    }

    /// Next cluster of a chain, `None` at its end. A free, reserved, bad or
    /// out-of-range entry is an error: following it would land on another
    /// file or, for 0 and 1, on the root directory.
    fn fat_next(&self, cluster: u32) -> Result<Option<u32>, &'static str> {
        if !self.valid_cluster(cluster) { return Err("ESP: cluster out of range"); }
        let val = self.fat_entry(cluster)?;
        if val >= 0x0FFF_FFF8 { return Ok(None); }
        if !self.valid_cluster(val) { return Err("ESP: corrupt FAT chain"); }
        Ok(Some(val))
    }

    /// Append one free cluster to the chain ending at `last`, and return it.
    ///
    /// Scans the FAT for an entry that is actually 0 rather than assuming
    /// `last + 1` is free; a wrong guess would splice another file's
    /// cluster into the kernel image.
    fn extend_chain(&self, last: u32) -> Result<u32, &'static str> {
        let mut cand = last + 1;
        while cand <= self.max_cluster {
            if self.fat_entry(cand)? == 0 {
                self.fat_write(last, cand)?;
                self.fat_write(cand, FAT_EOC)?;
                return Ok(cand);
            }
            cand += 1;
        }
        Err("ESP full: no free cluster to grow the kernel into")
    }

    /// Raw FAT entry for a cluster (0 = free).
    fn fat_entry(&self, cluster: u32) -> Result<u32, &'static str> {
        let (sector, o) = self.fat_pos(cluster)?;
        let mut sec = [0u8; 512];
        self.read_sector(self.reserved + sector, &mut sec)?;
        Ok(u32::from_le_bytes([sec[o], sec[o + 1], sec[o + 2], sec[o + 3]]) & 0x0FFF_FFFF)
    }

    /// Write a FAT entry in every FAT copy. The top four bits are reserved
    /// and kept (Microsoft FAT spec, "FAT32 FAT entry").
    fn fat_write(&self, cluster: u32, value: u32) -> Result<(), &'static str> {
        if !self.valid_cluster(cluster) { return Err("ESP: cluster out of range"); }
        let (fat_sector, o) = self.fat_pos(cluster)?;

        for fat_num in 0..self.num_fats {
            let abs = self.reserved + fat_num * self.fat_size + fat_sector;
            let mut sec = [0u8; 512];
            self.read_sector(abs, &mut sec)?;
            let old = u32::from_le_bytes([sec[o], sec[o + 1], sec[o + 2], sec[o + 3]]);
            let new = (old & 0xF000_0000) | (value & 0x0FFF_FFFF);
            sec[o..o + 4].copy_from_slice(&new.to_le_bytes());
            self.write_sector(abs, &sec)?;
        }
        Ok(())
    }

    /// Walk a directory for an 8.3 name and for the first free slot. Stops at
    /// the 0x00 end marker. LFN entries and the volume label never match.
    fn scan_dir(&self, dir_cluster: u32, name: &[u8; 11]) -> Result<DirScan, &'static str> {
        if !self.valid_cluster(dir_cluster) { return Err("ESP: directory cluster out of range"); }
        let mut scan = DirScan { found: None, free: None };
        let mut cluster = dir_cluster;
        // A crafted FAT can form a cycle; bound the walk.
        let mut steps = 0u32;
        loop {
            let sector = self.cluster_to_sector(cluster);
            for s in 0..self.spc {
                let mut sec = [0u8; 512];
                self.read_sector(sector + s, &mut sec)?;
                for off in (0..SECTOR_SIZE).step_by(32) {
                    let slot = DirEntry {
                        first_cluster: 0, size: 0, attr: 0,
                        cluster, sector: sector + s, offset: off,
                    };
                    let first = sec[off];
                    if first == 0x00 || first == 0xE5 {
                        if scan.free.is_none() { scan.free = Some(slot); }
                        if first == 0x00 { return Ok(scan); }
                        continue;
                    }
                    let attr = sec[off + 11];
                    if attr & ATTR_LFN == ATTR_LFN || attr & ATTR_VOLUME != 0 { continue; }
                    if &sec[off..off + 11] == name {
                        let cl_lo = u16::from_le_bytes([sec[off + 26], sec[off + 27]]) as u32;
                        let cl_hi = u16::from_le_bytes([sec[off + 20], sec[off + 21]]) as u32;
                        scan.found = Some(DirEntry {
                            first_cluster: (cl_hi << 16) | cl_lo,
                            size: u32::from_le_bytes([
                                sec[off + 28], sec[off + 29], sec[off + 30], sec[off + 31],
                            ]),
                            attr,
                            ..slot
                        });
                        return Ok(scan);
                    }
                }
            }
            match self.fat_next(cluster)? {
                Some(next) => cluster = next,
                None => return Ok(scan),
            }
            steps += 1;
            if steps > 100_000 { return Err("FAT dir chain too long"); }
        }
    }

    /// Find a directory entry by 8.3 name in a given directory cluster.
    /// Returns (first_cluster, file_size, dir_sector, entry_offset).
    fn find_entry(&self, dir_cluster: u32, name: &[u8; 11])
        -> Result<(u32, u32, u32, usize), &'static str>
    {
        let e = self.scan_dir(dir_cluster, name)?.found.ok_or("entry not found")?;
        Ok((e.first_cluster, e.size, e.sector, e.offset))
    }

    /// Cluster of the subdirectory `name` in `parent`, `None` if absent.
    fn subdir(&self, parent: u32, name: &[u8; 11]) -> Result<Option<u32>, &'static str> {
        match self.scan_dir(parent, name)?.found {
            None => Ok(None),
            Some(e) if e.attr & ATTR_DIR == 0 => Err("ESP: expected a directory"),
            Some(e) if !self.valid_cluster(e.first_cluster) => {
                Err("ESP: directory cluster out of range")
            }
            Some(e) => Ok(Some(e.first_cluster)),
        }
    }

    /// Replace the 32-byte entry at `slot`.
    fn write_entry(&self, slot: &DirEntry, entry: &[u8; 32]) -> Result<(), &'static str> {
        let mut sec = [0u8; 512];
        self.read_sector(slot.sector, &mut sec)?;
        sec[slot.offset..slot.offset + 32].copy_from_slice(entry);
        self.write_sector(slot.sector, &sec)
    }

    /// Set first cluster and size of the entry at `slot`, keeping the rest.
    fn set_entry_cluster_size(&self, slot: &DirEntry, cluster: u32, size: u32)
        -> Result<(), &'static str>
    {
        let mut sec = [0u8; 512];
        self.read_sector(slot.sector, &mut sec)?;
        let o = slot.offset;
        sec[o + 20..o + 22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        sec[o + 26..o + 28].copy_from_slice(&(cluster as u16).to_le_bytes());
        sec[o + 28..o + 32].copy_from_slice(&size.to_le_bytes());
        self.write_sector(slot.sector, &sec)
    }

    /// Write `first` into the first sector of `cluster` and zero the rest.
    fn write_cluster(&self, cluster: u32, first: &[u8; 512]) -> Result<(), &'static str> {
        let base = self.cluster_to_sector(cluster);
        self.write_sector(base, first)?;
        let zero = [0u8; 512];
        for s in 1..self.spc {
            self.write_sector(base + s, &zero)?;
        }
        Ok(())
    }

    /// A free cluster outside `keep`, searched from the end of the partition
    /// so the space behind the kernel's run stays free for it to grow into.
    fn alloc_cluster(&self, keep: &Protected) -> Result<u32, &'static str> {
        let per_sector = (SECTOR_SIZE / 4) as u32;
        let (last_sector, _) = self.fat_pos(self.max_cluster)?;
        for fat_sector in (0..=last_sector).rev() {
            let mut sec = [0u8; 512];
            self.read_sector(self.reserved + fat_sector, &mut sec)?;
            for i in (0..per_sector).rev() {
                let c = fat_sector * per_sector + i;
                if !keep.data_writable(self, c) { continue; }
                let o = i as usize * 4;
                let v = u32::from_le_bytes([sec[o], sec[o + 1], sec[o + 2], sec[o + 3]]);
                if v & 0x0FFF_FFFF == 0 { return Ok(c); }
            }
        }
        Err("ESP full")
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

/// Overwrite the UEFI boot kernel on the ESP partition.
/// The kernel lives at `/EFI/BOOT/BOOTX64.EFI` — UEFI firmware
/// auto-discovers that path, no menu / GRUB indirection. OTA `update`
/// calls this with the verified new kernel.efi.
pub fn update_kernel(esp_start: u64, data: &[u8]) -> Result<(), &'static str> {
    let fs = Fat32Reader::from_esp(esp_start)?;

    // Navigate: root → /EFI → /EFI/BOOT → BOOTX64.EFI
    let efi_name = make_name(b"EFI", b"");
    let (efi_cl, _, _, _) = fs.find_entry(fs.root_cluster, &efi_name)?;

    let boot_name = make_name(b"BOOT", b"");
    let (boot_cl, _, _, _) = fs.find_entry(efi_cl, &boot_name)?;

    let kernel_name = make_name(b"BOOTX64", b"EFI");
    let (kernel_cl, old_size, dir_sec, dir_off) = fs.find_entry(boot_cl, &kernel_name)?;

    let new_sectors = ((data.len() + 511) / 512) as u32;
    let old_clusters = fs.chain_len(kernel_cl)?;
    let new_clusters = (new_sectors + fs.spc - 1) / fs.spc;

    crate::kprintln!("[npk] ESP: BOOTX64.EFI at cluster {}, {} -> {} bytes ({}/{} clusters)",
        kernel_cl, old_size, data.len(), new_clusters, old_clusters);

    // No fixed limit: the chain grows into free clusters when the image
    // outgrows it (see `extend_chain`). What must not happen is the chain
    // shrinking back to the file size afterwards — see below.

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
            // Done writing. Deliberately do not truncate the chain here:
            // shrinking it to the current image would leave no room for a
            // slightly larger kernel on the next update. A chain longer than
            // the file is fine; the directory entry's size defines the file,
            // and firmware reads only that many bytes.
            break;
        }

        match fs.fat_next(cl)? {
            Some(next) => cl = next,
            None => {
                if clusters_used < new_clusters {
                    cl = fs.extend_chain(cl)?;
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
    let mut steps = 0u32;
    loop {
        let next = fs.fat_next(cl)?;
        fs.fat_write(cl, 0)?; // Mark as free
        match next {
            Some(n) => cl = n,
            None => break,
        }
        steps += 1;
        if steps > fs.max_cluster { return Err("FAT chain too long"); }
    }
    Ok(())
}

/// `NAME` or `NAME.EXT` as an 11-byte 8.3 entry name. Accepts ASCII letters,
/// digits, `_` and `-`; lower case is folded to upper case.
fn short_name(name: &str) -> Option<[u8; 11]> {
    let b = name.as_bytes();
    let (base, ext) = match b.iter().position(|&c| c == b'.') {
        Some(i) => (&b[..i], &b[i + 1..]),
        None => (b, &b[..0]),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 { return None; }
    if ext.is_empty() && base.len() != b.len() { return None; }
    let ok = |c: &u8| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-';
    if !base.iter().all(ok) || !ext.iter().all(ok) { return None; }
    Some(make_name(base, ext))
}

const NOPEEK_DIR: [u8; 11] = *b"NOPEEK     ";

/// Absolute start sector of the ESP on the NVMe boot disk, found the same
/// way the OTA kernel update finds it (first GPT entry, ESP type GUID).
/// `None` without NVMe, without GPT, or when the first entry is not an ESP.
pub fn esp_start() -> Option<u64> {
    crate::gpt::detect_esp_offset()
}

/// Read `/EFI/NOPEEK/<name>` (8.3 name, e.g. "TZ") into `buf`; returns the
/// file length copied. `None` if the file is absent, the ESP is unreadable or
/// inconsistent, or the file is larger than `buf` or `SMALL_FILE_MAX`.
/// `esp_start` is what `esp_start()` returns.
pub fn read_small_file(esp_start: u64, name: &str, buf: &mut [u8]) -> Option<usize> {
    let name = short_name(name)?;
    let fs = Fat32Reader::from_esp(esp_start).ok()?;
    let efi = fs.subdir(fs.root_cluster, &make_name(b"EFI", b"")).ok()??;
    let dir = fs.subdir(efi, &NOPEEK_DIR).ok()??;
    let e = fs.scan_dir(dir, &name).ok()?.found?;
    if e.attr & ATTR_DIR != 0 { return None; }
    let len = e.size as usize;
    if len > SMALL_FILE_MAX || len > buf.len() { return None; }
    if len == 0 { return Some(0); }
    if !fs.valid_cluster(e.first_cluster) { return None; }
    let mut sec = [0u8; 512];
    fs.read_sector(fs.cluster_to_sector(e.first_cluster), &mut sec).ok()?;
    buf[..len].copy_from_slice(&sec[..len]);
    Some(len)
}

/// Create or replace `/EFI/NOPEEK/<name>` with `data` (at most
/// `SMALL_FILE_MAX` bytes), creating `/EFI/NOPEEK` when missing. An existing
/// file is overwritten in its own cluster. Never writes into or allocates
/// from BOOTX64.EFI's reserved run.
///
/// Each step writes the data cluster, then the FAT, then the directory entry,
/// so a crash leaves at most an unreferenced allocated cluster. Overwriting
/// an existing file writes data before size, so a crash in between leaves the
/// new bytes under the old length.
///
/// Not implemented: growing `/EFI/NOPEEK` past its first cluster.
pub fn write_small_file(esp_start: u64, name: &str, data: &[u8]) -> Result<(), &'static str> {
    if data.len() > SMALL_FILE_MAX { return Err("ESP small file: larger than one sector"); }
    let name = short_name(name).ok_or("ESP small file: not an 8.3 name")?;
    let fs = Fat32Reader::from_esp(esp_start)?;

    let root = fs.root_cluster;
    let efi = fs.subdir(root, &make_name(b"EFI", b""))?.ok_or("ESP: no /EFI")?;
    let boot = fs.subdir(efi, &make_name(b"BOOT", b""))?.ok_or("ESP: no /EFI/BOOT")?;
    let kernel = fs.scan_dir(boot, &make_name(b"BOOTX64", b"EFI"))?
        .found.ok_or("ESP: no BOOTX64.EFI")?;
    if !fs.valid_cluster(kernel.first_cluster) { return Err("ESP: BOOTX64.EFI cluster out of range"); }
    // Same size rule as `create_esp`, so the bound covers the run it laid down.
    let kernel_clusters = (kernel.size as u64).div_ceil(SECTOR_SIZE as u64 * fs.spc as u64) as u32;
    let run_len = kernel_clusters.saturating_mul(2).max(MIN_KERNEL_RESERVED);
    let keep = Protected {
        run_start: kernel.first_cluster,
        run_end: kernel.first_cluster.saturating_add(run_len),
        dirs: [root, efi, boot],
    };
    if !keep.dir_writable(&fs, efi) { return Err("ESP: /EFI inside the kernel run"); }

    let dir = match fs.subdir(efi, &NOPEEK_DIR)? {
        Some(d) if keep.data_writable(&fs, d) => d,
        Some(_) => return Err("ESP: /EFI/NOPEEK cluster is protected"),
        None => {
            let slot = fs.scan_dir(efi, &NOPEEK_DIR)?.free.ok_or("ESP: /EFI is full")?;
            if !keep.dir_writable(&fs, slot.cluster) { return Err("ESP: /EFI slot is protected"); }
            let d = fs.alloc_cluster(&keep)?;
            let mut sec = [0u8; 512];
            sec[0..32].copy_from_slice(
                &Fat32Writer::make_dir_entry(b".          ", ATTR_DIR, d, 0));
            // `..` of a directory directly below root holds 0 (FAT spec).
            let parent = if efi == root { 0 } else { efi };
            sec[32..64].copy_from_slice(
                &Fat32Writer::make_dir_entry(b"..         ", ATTR_DIR, parent, 0));
            fs.write_cluster(d, &sec)?;
            fs.fat_write(d, FAT_EOC)?;
            fs.write_entry(&slot, &Fat32Writer::make_dir_entry(&NOPEEK_DIR, ATTR_DIR, d, 0))?;
            d
        }
    };

    let mut sec = [0u8; 512];
    sec[..data.len()].copy_from_slice(data);
    let size = data.len() as u32;

    let scan = fs.scan_dir(dir, &name)?;
    let (slot, existing) = match scan.found {
        Some(e) => {
            if e.attr & (ATTR_DIR | ATTR_VOLUME) != 0 { return Err("ESP: name is not a file"); }
            (e, Some(e))
        }
        None => (scan.free.ok_or("ESP: /EFI/NOPEEK is full")?, None),
    };
    if !keep.dir_writable(&fs, slot.cluster) || keep.dirs.contains(&slot.cluster) {
        return Err("ESP: /EFI/NOPEEK slot is protected");
    }

    match existing {
        Some(e) if e.first_cluster != 0 => {
            let c = e.first_cluster;
            if !keep.data_writable(&fs, c) || c == dir {
                return Err("ESP: file cluster is protected");
            }
            // Rejects a cluster the FAT calls free: it may belong to no one,
            // or be handed out again.
            fs.fat_next(c)?;
            fs.write_cluster(c, &sec)?;
            if e.size != size { fs.set_entry_cluster_size(&slot, c, size)?; }
        }
        Some(_) => {
            let c = fs.alloc_cluster(&keep)?;
            fs.write_cluster(c, &sec)?;
            fs.fat_write(c, FAT_EOC)?;
            fs.set_entry_cluster_size(&slot, c, size)?;
        }
        None => {
            let c = fs.alloc_cluster(&keep)?;
            fs.write_cluster(c, &sec)?;
            fs.fat_write(c, FAT_EOC)?;
            fs.write_entry(&slot, &Fat32Writer::make_dir_entry(&name, ATTR_ARCHIVE, c, size))?;
        }
    }
    Ok(())
}
