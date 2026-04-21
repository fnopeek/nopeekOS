//! NVMe installation: GPT + FAT32 ESP + npkFS partitioning
//!
//! Embedded data (GRUB EFI binary + kernel) is included at build time
//! via the `installer` Cargo feature. The two-pass build ensures:
//!   Pass 1: kernel without embedded data (installed to NVMe)
//!   Pass 2: kernel with embedded data (USB installation medium)

use crate::{kprintln, kprint, serial, blkdev, nvme, gpt, fat32, npkfs};

#[cfg(feature = "installer")]
static GRUB_EFI: &[u8] = include_bytes!("install_data/grub.efi");

#[cfg(feature = "installer")]
static INSTALL_KERNEL: &[u8] = include_bytes!("install_data/kernel.bin");

#[cfg(feature = "installer")]
static GRUB_CFG: &[u8] = include_bytes!("install_data/grub.cfg");

/// Bundled assets embedded into the installer kernel — font + WASM
/// modules written into npkFS on fresh install.
#[cfg(feature = "installer")]
#[path = "install_data/assets/mod.rs"]
mod bundled_assets;

/// Check if this build has installer capability
pub fn has_installer() -> bool {
    cfg!(feature = "installer")
}

/// Run the NVMe installation.
/// Partitions the NVMe, creates ESP with GRUB+kernel, sets blkdev offset for npkFS.
#[cfg(feature = "installer")]
pub fn install_to_nvme() -> Result<(), &'static str> {
    if !nvme::is_available() {
        return Err("NVMe not available");
    }

    let total = nvme::capacity().ok_or("cannot read NVMe capacity")?;
    let total_gb = (total * 512) / (1024 * 1024 * 1024);

    let model = nvme::model_name().unwrap_or_default();
    kprintln!("[npk] Install target: {} ({} GB)", model, total_gb);
    kprintln!("[npk] Partition layout:");
    kprintln!("[npk]   ESP:   64 MB  (FAT32, GRUB + kernel)");
    kprintln!("[npk]   npkFS: {} GB  (encrypted storage)", total_gb.saturating_sub(1));
    kprintln!();

    kprint!("[npk] Install nopeekOS to NVMe? [y/N] ");
    let mut buf = [0u8; 16];
    let len = serial::SERIAL.lock().read_line(&mut buf);
    if len == 0 || (buf[0] != b'y' && buf[0] != b'Y') {
        return Err("Installation cancelled");
    }

    // Step 1: Write GPT
    kprint!("[npk] Writing partition table...");
    let npkfs_start_sector = gpt::write_gpt()?;
    kprintln!(" done.");

    // Step 2: Create FAT32 ESP
    kprint!("[npk] Creating EFI boot partition...");
    fat32::create_esp(
        gpt::ESP_START,
        gpt::ESP_SECTORS as u32,
        GRUB_EFI,
        INSTALL_KERNEL,
        GRUB_CFG,
    )?;
    kprintln!(" done. (GRUB {} KB, kernel {} KB)",
        GRUB_EFI.len() / 1024, INSTALL_KERNEL.len() / 1024);

    // Step 3: Set blkdev partition offset so npkFS uses the right partition
    let npkfs_block_offset = npkfs_start_sector / 8; // sectors to 4KB blocks
    blkdev::set_partition_offset(npkfs_block_offset);
    kprintln!("[npk] npkFS partition at sector {} (block offset {})",
        npkfs_start_sector, npkfs_block_offset);

    // Step 4: Format npkFS
    kprint!("[npk] Formatting npkFS...");
    npkfs::mkfs().map_err(|_| "npkFS format failed")?;
    npkfs::mount().map_err(|_| "npkFS mount failed")?;
    kprintln!(" done.");

    // NOTE: Seeding of bundled assets happens LATER, after
    // setup::run_fresh_install has derived + installed the master key
    // (see main.rs). If we wrote them here, npkfs::store would take the
    // "no master key" path and write plaintext; subsequent fetches on
    // normal boots (with master key set) would then fail AEAD decrypt
    // with "crypt key fail". See seed_bundled_assets() below.

    kprintln!("[npk] Installation complete.");
    kprintln!();

    Ok(())
}

/// Write the bundled font + WASM modules into npkFS, encrypting each
/// with the active master key (ChaCha20-Poly1305 AEAD). Must be called
/// after setup::run_fresh_install has called crypto::set_master_key.
#[cfg(feature = "installer")]
pub fn seed_bundled_assets() {
    bundled_assets::bootstrap_into_npkfs();
}

/// Stub for non-installer builds — main.rs calls this unconditionally,
/// but outside the installer there's nothing to seed.
#[cfg(not(feature = "installer"))]
pub fn seed_bundled_assets() {}

/// Stub when installer feature is not enabled
#[cfg(not(feature = "installer"))]
pub fn install_to_nvme() -> Result<(), &'static str> {
    Err("This kernel was not built with installer support")
}
