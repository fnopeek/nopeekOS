# `kernel/src/storage/fat32.rs` @ 5e0102684

## L1-4 · `use crate::nvme;`

```
//! Minimal FAT32 writer for EFI System Partition
//!
//! Creates a FAT32 filesystem and writes GRUB EFI, kernel, and grub.cfg.
//! Write-once, no modify/delete needed.
```

## L13-15 · `const MIN_KERNEL_RESERVED: u32 = 32768;`

```
/// Floor for the kernel's reserved cluster run (16 MiB at 512 B/cluster).
/// Generous on purpose: the ESP is 64 MB and re-laying-out the FAT during an
/// OTA is not something we want to implement.
```

## L18 · `const FAT_EOC: u32 = 0x0FFF_FFFF;`

```
/// FAT32 entry: end of chain
```

## L22 · `part_start: u64,`

```
/// Absolute start sector on NVMe
```

## L24 · `total_sectors: u32,`

```
/// Total sectors in partition
```

## L26 · `fat_size: u32,`

```
/// Sectors per FAT
```

## L28 · `data_start: u32,`

```
/// First data sector (relative to partition start)
```

## L30 · `next_cluster: u32,`

```
/// Next free cluster
```

## L36-39 · `let fat_size = ((total_sectors as u64 * 4 + 511) / 512) as u32;`

```
// Calculate FAT size: each cluster needs 4 bytes in FAT
// data_clusters ≈ (total - reserved - 2*fat) / spc
// fat_size = ceil(data_clusters * 4 / 512)
// Approximate: over-allocate FAT, it's fine
```

## L52 · `fn write_sector(&self, rel_sector: u32, data: &[u8; SECTOR_SIZE]) -> Result<(), &'static str> {`

```
/// Write a sector at absolute NVMe position
```

## L58 · `fn cluster_to_sector(&self, cluster: u32) -> u32 {`

```
/// Sector number for a given cluster
```

## L63-64 · `fn alloc_clusters(&mut self, count: u32) -> u32 {`

```
/// Allocate N contiguous clusters starting from next_cluster.
/// Returns the first cluster number.
```

## L71 · `fn write_boot_sector(&self) -> Result<(), &'static str> {`

```
/// Write the FAT32 boot sector (BPB) and FSInfo
```

## L75 · `bs[0] = 0xEB; bs[1] = 0x58; bs[2] = 0x90;`

```
// Jump boot code
```

## L77 · `bs[3..11].copy_from_slice(b"NOPEEKOS");`

```
// OEM name
```

## L79 · `bs[11..13].copy_from_slice(&512u16.to_le_bytes());       // BytsPerSec`

```
// BPB
```

## L80 · `bs[11..13].copy_from_slice(&512u16.to_le_bytes());       // BytsPerSec`

```
// BytsPerSec
```

## L81 · `bs[13] = SECTORS_PER_CLUSTER as u8;                       // SecPerClus`

```
// SecPerClus
```

## L82 · `bs[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes()); // RsvdSecCnt`

```
// RsvdSecCnt
```

## L83 · `bs[16] = NUM_FATS;                                        // NumFATs`

```
// NumFATs
```

## L84 · `bs[17..19].copy_from_slice(&0u16.to_le_bytes());          // RootEntCnt (0 for FAT32)`

```
// RootEntCnt (0 for FAT32)
```

## L85 · `bs[19..21].copy_from_slice(&0u16.to_le_bytes());          // TotSec16 (0, use 32)`

```
// TotSec16 (0, use 32)
```

## L86 · `bs[21] = 0xF8;                                            // Media`

```
// Media
```

## L87 · `bs[22..24].copy_from_slice(&0u16.to_le_bytes());          // FATSz16 (0, use 32)`

```
// FATSz16 (0, use 32)
```

## L88 · `bs[24..26].copy_from_slice(&63u16.to_le_bytes());         // SecPerTrk`

```
// SecPerTrk
```

## L89 · `bs[26..28].copy_from_slice(&255u16.to_le_bytes());        // NumHeads`

```
// NumHeads
```

## L90 · `bs[28..32].copy_from_slice(&(self.part_start as u32).to_le_bytes()); // HiddSec`

```
// HiddSec
```

## L91 · `bs[32..36].copy_from_slice(&self.total_sectors.to_le_bytes()); // TotSec32`

```
// TotSec32
```

## L92 · `bs[36..40].copy_from_slice(&self.fat_size.to_le_bytes()); // FATSz32`

```
// FAT32 specific
```

## L93 · `bs[36..40].copy_from_slice(&self.fat_size.to_le_bytes()); // FATSz32`

```
// FATSz32
```

## L94 · `bs[40..42].copy_from_slice(&0u16.to_le_bytes());          // ExtFlags`

```
// ExtFlags
```

## L95 · `bs[42..44].copy_from_slice(&0u16.to_le_bytes());          // FSVer`

```
// FSVer
```

## L96 · `bs[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());  // RootClus`

```
// RootClus
```

## L97 · `bs[48..50].copy_from_slice(&1u16.to_le_bytes());          // FSInfo sector`

```
// FSInfo sector
```

## L98 · `bs[50..52].copy_from_slice(&6u16.to_le_bytes());          // BkBootSec`

```
// BkBootSec
```

## L99 · `bs[64] = 0x80;                                            // DrvNum`

```
// Drive number, boot sig, volume info
```

## L100 · `bs[64] = 0x80;                                            // DrvNum`

```
// DrvNum
```

## L101 · `bs[66] = 0x29;                                            // BootSig`

```
// BootSig
```

## L102 · `let serial = crate::csprng::random_u64() as u32;`

```
// Volume serial (random)
```

## L105 · `bs[71..82].copy_from_slice(b"NOPEEKOS   ");              // VolLab`

```
// VolLab
```

## L106 · `bs[82..90].copy_from_slice(b"FAT32   ");                  // FilSysType`

```
// FilSysType
```

## L109 · `self.write_sector(0, &bs)?;`

```
// Write boot sector
```

## L111 · `self.write_sector(6, &bs)?;`

```
// Write backup boot sector at sector 6
```

## L114 · `let mut fsi = [0u8; 512];`

```
// FSInfo sector (sector 1)
```

## L116 · `fsi[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());     // LeadSig`

```
// LeadSig
```

## L117 · `fsi[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes()); // StrucSig`

```
// StrucSig
```

## L119 · `fsi[488..492].copy_from_slice(&free.to_le_bytes());            // Free_Count`

```
// Free_Count
```

## L120 · `fsi[492..496].copy_from_slice(&self.next_cluster.to_le_bytes()); // Nxt_Free`

```
// Nxt_Free
```

## L127 · `fn write_fat_entry(&self, cluster: u32, value: u32) -> Result<(), &'static str> {`

```
/// Write a FAT entry. Writes to both FAT copies.
```

## L136 · `let mut sec = [0u8; 512];`

```
// Read existing sector
```

## L148 · `fn write_fat_chain(&self, first: u32, count: u32) -> Result<(), &'static str> {`

```
/// Write FAT chain for contiguous clusters
```

## L158 · `fn init_fat(&self) -> Result<(), &'static str> {`

```
/// Initialize FAT: entries 0 and 1
```

## L160 · `let zero = [0u8; 512];`

```
// Zero out reserved sectors area (clean slate)
```

## L165 · `for fat_num in 0..NUM_FATS as u32 {`

```
// Zero out FAT sectors
```

## L172 · `self.write_fat_entry(0, 0x0FFF_FFF8)?; // Media byte`

```
// Media byte
```

## L173 · `self.write_fat_entry(1, FAT_EOC)?;       // Reserved`

```
// Reserved
```

## L177 · `fn make_dir_entry(name: &[u8; 11], attr: u8, cluster: u32, size: u32) -> [u8; 32] {`

```
/// Create a directory entry (32 bytes)
```

## L182 · `e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());`

```
// First cluster low
```

## L184 · `e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());`

```
// First cluster high
```

## L186 · `e[28..32].copy_from_slice(&size.to_le_bytes());`

```
// File size
```

## L191 · `fn write_directory(&self, cluster: u32, entries: &[[u8; 32]]) -> Result<(), &'static str> {`

```
/// Write a directory sector with given entries
```

## L203-210 · `fn write_file_data(`

```
/// Write file data starting at `first_cluster`, refusing to run past the
/// `max_clusters` that were reserved for it.
///
/// Without that bound this wrote happily past the end of the file's FAT
/// chain: the directory entry then advertised the full size while the
/// chain stopped short, so anything following the chain — i.e. the UEFI
/// firmware — read a truncated image and refused to boot, with the file
/// still listing at the right size. Silent corruption; fail loudly instead.
```

## L234 · `fn make_name(name: &[u8], ext: &[u8]) -> [u8; 11] {`

```
/// 8.3 filename from components (name max 8 chars, ext max 3 chars)
```

## L246-260 · `pub fn create_esp(`

```
/// Create FAT32 ESP and write the UEFI bootloader.
/// `part_start`: absolute sector on NVMe where ESP begins
/// `part_sectors`: size of ESP in sectors
/// `kernel_efi`: the kernel as a UEFI PE+ application (built by
///     `objcopy -O pei-x86-64 --subsystem=efi-app`). UEFI firmware
///     auto-discovers `/EFI/BOOT/BOOTX64.EFI` — no GRUB needed since
///     our kernel IS the EFI application.
///
/// Layout written:
/// `​``text
/// ESP/
/// └── EFI/
///     └── BOOT/
///         └── BOOTX64.EFI   ← kernel_efi
/// `​``
```

## L271 · `let root_cl = fs.alloc_clusters(1);   // cluster 2: root`

```
// Directory clusters
```

## L272 · `let root_cl = fs.alloc_clusters(1);   // cluster 2: root`

```
// cluster 2: root
```

## L273 · `let efi_cl = fs.alloc_clusters(1);    // cluster 3: /EFI`

```
// cluster 3: /EFI
```

## L274 · `let efiboot_cl = fs.alloc_clusters(1); // cluster 4: /EFI/BOOT`

```
// cluster 4: /EFI/BOOT
```

## L276-283 · `let kernel_clusters = ((kernel_efi.len() + 511) / 512) as u32;`

```
// Reserve a contiguous run for the kernel image, so a later OTA can
// overwrite it in place without re-laying-out the FAT.
//
// This used to be a flat 8192 clusters = 4 MiB, which the kernel quietly
// outgrew: the image was written past the end of its own chain and the
// machine stopped booting. Derive it from the image instead — double the
// current size, with a floor — so the reservation tracks reality and
// still leaves room to grow.
```

## L287-288 · `let usable = fs.total_sectors.saturating_sub(fs.data_start) / SECTORS_PER_CLUSTER;`

```
// The FAT itself only spans so many clusters; refuse rather than lay down
// a filesystem whose chains point past the data area.
```

## L295 · `fs.write_fat_chain(root_cl, 1)?;`

```
// FAT chains
```

## L301 · `fs.write_directory(root_cl, &[`

```
// Root: /EFI, marker file
```

## L307 · `fs.write_directory(efi_cl, &[`

```
// /EFI: ., .., BOOT/
```

## L314 · `fs.write_directory(efiboot_cl, &[`

```
// /EFI/BOOT: ., .., BOOTX64.EFI
```

## L328-330 · `struct Fat32Reader {`

```
// ============================================================
// FAT32 Reader — for OTA update (find + overwrite kernel.bin)
// ============================================================
```

## L333 · `part_start: u64,`

```
/// Absolute start sector of ESP on NVMe
```

## L335 · `fat_size: u32,`

```
/// Sectors per FAT
```

## L337 · `data_start: u32,`

```
/// First data sector (relative to partition start)
```

## L339 · `spc: u32,`

```
/// Sectors per cluster
```

## L341-342 · `total_sectors: u32,`

```
/// Total sectors in the partition (BPB TotSec32) — the bound for
/// allocating additional clusters.
```

## L347 · `fn from_esp(part_start: u64) -> Result<Self, &'static str> {`

```
/// Parse the boot sector (BPB) to initialize the reader.
```

## L352 · `if bs[510] != 0x55 || bs[511] != 0xAA { return Err("ESP: bad signature"); }`

```
// Verify FAT32 signature
```

## L371-377 · `fn write_sector(&self, rel_sector: u32, buf: &[u8; 512]) -> Result<(), &'static str> {`

```
/// Every raw block write on this path lands OUTSIDE npkFS, so a sector
/// number that has run past the partition writes into whatever follows it
/// — which on this disk is the filesystem. Nothing bounded it: the chain
/// is followed through the on-disk FAT, and one bad entry there is enough
/// to send `cluster_to_sector` anywhere. Refusing is always better than
/// scribbling; a failed kernel update leaves the old one bootable, a
/// scribbled npkFS does not.
```

## L390-391 · `self.data_start`

```
// Saturating: cluster 0/1 are reserved and would underflow, and an
// overflow here used to wrap into a plausible-looking sector.
```

## L396 · `fn fat_next(&self, cluster: u32) -> Result<Option<u32>, &'static str> {`

```
/// Read next cluster from FAT chain.
```

## L412-418 · `fn extend_chain(&self, last: u32) -> Result<u32, &'static str> {`

```
/// Write a FAT entry (both copies).
/// Append one FREE cluster to the chain ending at `last`, and return it.
///
/// The old code took `last + 1` on faith. In our ESP layout that happens
/// to be free, but a wrong guess would splice another file's cluster into
/// the kernel — silent corruption of the one file the machine boots from.
/// So scan the FAT for an entry that is actually 0.
```

## L433 · `fn fat_entry(&self, cluster: u32) -> Result<u32, &'static str> {`

```
/// Raw FAT entry for a cluster (0 = free).
```

## L457-458 · `fn find_entry(&self, dir_cluster: u32, name: &[u8; 11])`

```
/// Find a directory entry by 8.3 name in a given directory cluster.
/// Returns (first_cluster, file_size, dir_sector, entry_offset).
```

## L468 · `for i in 0..16 { // 16 entries per sector`

```
// 16 entries per sector
```

## L471 · `if sec[off] == 0xE5 { continue; } // deleted`

```
// deleted
```

## L490 · `fn chain_len(&self, first: u32) -> Result<u32, &'static str> {`

```
/// Count clusters in a FAT chain.
```

## L504 · `fn update_entry_size(&self, dir_sector: u32, entry_offset: usize, new_size: u32)`

```
/// Update directory entry size field.
```

## L515-518 · `pub fn update_kernel(esp_start: u64, data: &[u8]) -> Result<(), &'static str> {`

```
/// Overwrite the UEFI boot kernel on the ESP partition.
/// The kernel lives at `/EFI/BOOT/BOOTX64.EFI` — UEFI firmware
/// auto-discovers that path, no menu / GRUB indirection. OTA `update`
/// calls this with the verified new kernel.efi.
```

## L522 · `let efi_name = make_name(b"EFI", b"");`

```
// Navigate: root → /EFI → /EFI/BOOT → BOOTX64.EFI
```

## L539-541 · `let mut cl = kernel_cl;`

```
// No fixed limit: the chain grows into free clusters when the image
// outgrows it (see `extend_chain`). What must NOT happen is the chain
// shrinking back to the file size afterwards — see below.
```

## L543 · `let mut cl = kernel_cl;`

```
// Write data to existing clusters (follow FAT chain)
```

## L551 · `let zero = [0u8; 512];`

```
// Zero-pad remaining sectors in this cluster
```

## L565-573 · `break;`

```
// Done writing. Deliberately do NOT truncate the chain here.
//
// This used to set EOC at the last written cluster and free the
// rest, which quietly shrank the reservation to exactly the
// current image on every update — so OTA worked exactly once and
// the next, slightly larger kernel no longer fit ("need 8243
// clusters, reservation is 8241"). The chain being longer than
// the file is fine: the directory entry's size is what defines
// the file, and firmware reads only that many bytes.
```

## L589 · `fs.update_entry_size(dir_sec, dir_off, data.len() as u32)?;`

```
// Update directory entry with new file size
```

## L595 · `fn free_chain(fs: &Fat32Reader, start: u32) -> Result<(), &'static str> {`

```
/// Free a FAT chain starting at the given cluster.
```

## L600 · `fs.fat_write(cl, 0)?; // Mark as free`

```
// Mark as free
```

