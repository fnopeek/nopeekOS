# `kernel/src/install.rs` @ 5e0102684

## L1-10 · `use crate::{kprintln, kprint, serial, blkdev, nvme, gpt, fat32, npkfs};`

```
//! NVMe installation: GPT + FAT32 ESP + npkFS partitioning
//!
//! UEFI-only. The kernel is itself a PE+ UEFI Application — firmware
//! finds `/EFI/BOOT/BOOTX64.EFI` on the ESP and runs it directly, no
//! GRUB indirection.
//!
//! Embedded data (the kernel.efi binary) is included at build time
//! via the `installer` Cargo feature. The two-pass build ensures:
//!   Pass 1: kernel without embedded data (the one we install to NVMe)
//!   Pass 2: kernel with embedded data (the USB / installer ISO image)
```

## L17-18 · `#[cfg(feature = "installer")]`

```
/// Bundled assets embedded into the installer kernel — font + WASM
/// modules written into npkFS on fresh install.
```

## L23 · `pub fn has_installer() -> bool {`

```
/// Check if this build has installer capability
```

## L28-29 · `#[cfg(feature = "installer")]`

```
/// Run the NVMe installation.
/// Partitions the NVMe, creates ESP with GRUB+kernel, sets blkdev offset for npkFS.
```

## L53 · `kprint!("[npk] Writing partition table...");`

```
// Step 1: Write GPT
```

## L58-60 · `kprint!("[npk] Creating EFI boot partition...");`

```
// Step 2: Create FAT32 ESP. The kernel.efi *is* the UEFI Boot
// Application — firmware loads /EFI/BOOT/BOOTX64.EFI directly,
// no GRUB needed.
```

## L69-71 · `let (npkfs_block_offset, npkfs_block_count) = match gpt::detect_npkfs_partition() {`

```
// Step 3: Set blkdev partition offset + size so npkFS uses the right
// region. Re-detect via GPT so we get both ends consistently — the
// installer just wrote the table, so this round-trip can't disagree.
```

## L75-76 · `return Err("partition detection failed after write");`

```
// Fall back to manual offset; size unset means "whole disk minus offset",
// which is exactly the bug we're trying to avoid, so bail loudly.
```

## L86 · `kprint!("[npk] Formatting npkFS...");`

```
// Step 4: Format npkFS
```

## L92-97 · `kprintln!("[npk] Installation complete.");`

```
// NOTE: Seeding of bundled assets happens LATER, after
// setup::run_fresh_install has derived + installed the master key
// (see main.rs). If we wrote them here, npkfs::store would take the
// "no master key" path and write plaintext; subsequent fetches on
// normal boots (with master key set) would then fail AEAD decrypt
// with "crypt key fail". See seed_bundled_assets() below.
```

## L105-107 · `#[cfg(feature = "installer")]`

```
/// Write the bundled font + WASM modules into npkFS, encrypting each
/// with the active master key (ChaCha20-Poly1305 AEAD). Must be called
/// after setup::run_fresh_install has called crypto::set_master_key.
```

## L113-114 · `#[cfg(not(feature = "installer"))]`

```
/// Stub for non-installer builds — main.rs calls this unconditionally,
/// but outside the installer there's nothing to seed.
```

## L118 · `#[cfg(not(feature = "installer"))]`

```
/// Stub when installer feature is not enabled
```

