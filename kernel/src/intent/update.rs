//! OTA update intent.
//!
//! Downloads kernel from GitHub, verifies ECDSA P-384 signature,
//! writes to ESP FAT32 partition.

use crate::{kprintln, kprint};
use alloc::string::String;
use alloc::vec::Vec;

const UPDATE_HOST: &str = "raw.githubusercontent.com";
const UPDATE_BASE: &str = "/fnopeek/nopeekOS/main/release";
const MAX_KERNEL_SIZE: usize = 4 * 1024 * 1024; // 4 MB
const MAX_MANIFEST_SIZE: usize = 4096;
const MAX_ASSET_MANIFEST_SIZE: usize = 16 * 1024;
const MAX_ASSET_SIZE: usize = 32 * 1024 * 1024; // bzImage is ~12 MB today
const MAX_SIG_SIZE: usize = 512;

/// Mapping from asset-manifest section header to (remote filename,
/// npkFS path). Keep in sync with `build.sh` ASSET_MANIFEST writer
/// and `kernel/src/install_data/assets/mod.rs` BUNDLED entries.
struct AssetSpec {
    section: &'static str,
    remote_filename: &'static str,
    npkfs_path: &'static str,
}

const ASSETS: &[AssetSpec] = &[
    AssetSpec { section: "font:inter-variable", remote_filename: "inter-variable.ttf",        npkfs_path: "sys/fonts/inter-variable" },
    AssetSpec { section: "icons:phosphor",      remote_filename: "phosphor.atlas",            npkfs_path: "sys/icons/phosphor" },
    AssetSpec { section: "microvm:initramfs",   remote_filename: "microvm-initramfs.cpio.gz", npkfs_path: "sys/microvm/initramfs.cpio.gz" },
    AssetSpec { section: "microvm:linux-virt",  remote_filename: "linux-virt.bzImage",        npkfs_path: "sys/microvm/linux-virt.bzImage" },
];

struct AssetEntry {
    section: String,
    size: usize,
    sha384: [u8; 48],
}

struct Manifest {
    version: String,
    size: usize,
    sha384: [u8; 48],
}

fn parse_manifest(data: &[u8]) -> Result<Manifest, &'static str> {
    let text = core::str::from_utf8(data).map_err(|_| "manifest: invalid UTF-8")?;
    let mut version = None;
    let mut size = None;
    let mut sha384 = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Some((key, val)) = line.split_once('=') {
            match key.trim() {
                "version" => version = Some(String::from(val.trim())),
                "size" => size = val.trim().parse::<usize>().ok(),
                "sha384" => sha384 = Some(hex_to_bytes48(val.trim())?),
                _ => {}
            }
        }
    }

    Ok(Manifest {
        version: version.ok_or("manifest: missing version")?,
        size: size.ok_or("manifest: missing size")?,
        sha384: sha384.ok_or("manifest: missing sha384")?,
    })
}

fn hex_to_bytes48(hex: &str) -> Result<[u8; 48], &'static str> {
    if hex.len() != 96 { return Err("sha384: expected 96 hex chars"); }
    let mut out = [0u8; 48];
    for i in 0..48 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "sha384: invalid hex")?;
    }
    Ok(out)
}

pub fn intent_update(_args: &str) {
    kprintln!("[npk] Checking for updates...");

    // 1. Download manifest
    kprintln!("[npk] Fetching manifest...");
    let manifest_path = alloc::format!("{}/manifest", UPDATE_BASE);
    let manifest_data = match super::http::https_get(UPDATE_HOST, &manifest_path, MAX_MANIFEST_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk] Failed to fetch manifest: {}", e); return; }
    };

    let manifest = match parse_manifest(&manifest_data) {
        Ok(m) => m,
        Err(e) => { kprintln!("[npk] {}", e); return; }
    };

    let current = env!("CARGO_PKG_VERSION");
    kprintln!("[npk] Available: v{} (current: v{})", manifest.version, current);

    let mut kernel_updated = false;

    if manifest.version == current {
        kprintln!("[npk] Kernel up to date.");
    } else {
        kprintln!("[npk] Size: {} bytes", manifest.size);

        // 2. Download kernel
        kprintln!("[npk] Downloading kernel.bin ({} KB)...", manifest.size / 1024);
        let kernel_path = alloc::format!("{}/kernel.bin", UPDATE_BASE);
        let kernel_data = match super::http::https_get(UPDATE_HOST, &kernel_path, MAX_KERNEL_SIZE) {
            Ok(d) => d,
            Err(e) => { kprintln!("[npk] Download failed: {}", e); return; }
        };

        if kernel_data.len() != manifest.size {
            kprintln!("[npk] Size mismatch: got {} expected {}", kernel_data.len(), manifest.size);
            return;
        }

        // 3. Verify SHA-384
        kprint!("[npk] Verifying SHA-384... ");
        let hash = crate::tls::sha256::sha384(&kernel_data);
        if hash != manifest.sha384 {
            kprintln!("FAILED");
            kprintln!("[npk] Checksum mismatch! Update rejected.");
            return;
        }
        kprintln!("OK");

        // 4. Download signature
        kprintln!("[npk] Downloading signature...");
        let sig_path = alloc::format!("{}/kernel.sig", UPDATE_BASE);
        let sig_data = match super::http::https_get(UPDATE_HOST, &sig_path, MAX_SIG_SIZE) {
            Ok(d) => d,
            Err(e) => { kprintln!("[npk] Signature download failed: {}", e); return; }
        };

        // 5. Verify ECDSA P-384 signature (reuse SHA-384 hash from step 3)
        kprint!("[npk] Verifying ECDSA P-384 signature... ");
        let pubkey = &crate::update_key::UPDATE_PUB_KEY;
        if !crate::tls::certstore::verify_p384_prehash_384(pubkey, &hash, &sig_data) {
            kprintln!("FAILED");
            kprintln!("[npk] Invalid signature! Update rejected.");
            return;
        }
        kprintln!("OK");

        // 6. Find ESP partition
        kprint!("[npk] Locating ESP partition... ");
        let esp_start = match crate::gpt::detect_esp_offset() {
            Some(s) => { kprintln!("sector {}", s); s }
            None => { kprintln!("not found"); kprintln!("[npk] No ESP partition. Is this a GPT disk?"); return; }
        };

        // 7. Write to ESP
        kprintln!("[npk] Writing kernel to ESP...");
        match crate::fat32::update_kernel(esp_start, &kernel_data) {
            Ok(()) => {}
            Err(e) => {
                kprintln!("[npk] ESP write failed: {}", e);
                return;
            }
        }

        kprintln!("[npk] Kernel v{} installed.", manifest.version);
        kernel_updated = true;
    }

    // 8. Update installed WASM modules
    kprintln!("[npk] Checking modules...");
    let mod_count = super::install::update_all_modules();
    if mod_count > 0 {
        kprintln!("[npk] {} module(s) updated.", mod_count);
    } else {
        kprintln!("[npk] Modules up to date.");
    }

    // 9. Update bundled assets (fonts, icons, microvm payloads)
    kprintln!("[npk] Checking assets...");
    let asset_count = update_all_assets();
    if asset_count > 0 {
        kprintln!("[npk] {} asset(s) updated.", asset_count);
    } else {
        kprintln!("[npk] Assets up to date.");
    }

    if kernel_updated {
        kprintln!("[npk] ====================================");
        kprintln!("[npk]  Type 'reboot' to apply.");
        kprintln!("[npk] ====================================");
    }
}

fn parse_asset_manifest(data: &[u8]) -> Result<Vec<AssetEntry>, &'static str> {
    let text = core::str::from_utf8(data).map_err(|_| "asset manifest: invalid UTF-8")?;
    let mut entries = Vec::new();
    let mut section: Option<String> = None;
    let mut size: Option<usize> = None;
    let mut sha384: Option<[u8; 48]> = None;

    let flush = |section: &mut Option<String>, size: &mut Option<usize>, sha: &mut Option<[u8; 48]>, out: &mut Vec<AssetEntry>| {
        if let (Some(s), Some(sz), Some(sh)) = (section.take(), size.take(), sha.take()) {
            out.push(AssetEntry { section: s, size: sz, sha384: sh });
        }
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            flush(&mut section, &mut size, &mut sha384, &mut entries);
            section = Some(String::from(&line[1..line.len() - 1]));
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            match key.trim() {
                "size" => size = val.trim().parse::<usize>().ok(),
                "sha384" => sha384 = hex_to_bytes48(val.trim()).ok(),
                _ => {}
            }
        }
    }
    flush(&mut section, &mut size, &mut sha384, &mut entries);
    Ok(entries)
}

/// Diff release/assets/manifest against npkFS-resident assets and
/// re-fetch any whose sha384 differs (or that aren't present yet).
/// Each asset is verified against its detached ECDSA P-384 signature
/// using the same update key as kernel/modules. Returns the count of
/// assets actually written.
pub fn update_all_assets() -> usize {
    let manifest_path = alloc::format!("{}/assets/manifest", UPDATE_BASE);
    let manifest_data = match super::http::https_get(UPDATE_HOST, &manifest_path, MAX_ASSET_MANIFEST_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk] Asset manifest fetch failed: {}", e); return 0; }
    };

    let entries = match parse_asset_manifest(&manifest_data) {
        Ok(e) => e,
        Err(e) => { kprintln!("[npk] Asset manifest parse error: {}", e); return 0; }
    };

    let mut updated = 0usize;

    for entry in &entries {
        let spec = match ASSETS.iter().find(|s| s.section == entry.section) {
            Some(s) => s,
            None => {
                kprintln!("[npk]   unknown asset [{}] (skipping)", entry.section);
                continue;
            }
        };

        let local_hash = crate::npkfs::fetch(spec.npkfs_path).ok()
            .map(|(data, _)| crate::tls::sha256::sha384(&data));

        if local_hash.as_ref() == Some(&entry.sha384) {
            kprintln!("[npk]   {} (up to date)", spec.npkfs_path);
            continue;
        }

        if local_hash.is_none() {
            kprintln!("[npk]   {} (not in npkFS — installing)", spec.npkfs_path);
        } else {
            kprintln!("[npk]   {} (out of date — refreshing)", spec.npkfs_path);
        }

        kprint!("[npk]   downloading {} ({} KB)... ", spec.remote_filename, entry.size / 1024);
        let asset_path = alloc::format!("{}/assets/{}", UPDATE_BASE, spec.remote_filename);
        let asset_data = match super::http::https_get(UPDATE_HOST, &asset_path, MAX_ASSET_SIZE) {
            Ok(d) => d,
            Err(e) => { kprintln!("failed: {}", e); continue; }
        };

        if asset_data.len() != entry.size {
            kprintln!("size mismatch (got {} expected {})", asset_data.len(), entry.size);
            continue;
        }

        let hash = crate::tls::sha256::sha384(&asset_data);
        if hash != entry.sha384 {
            kprintln!("checksum failed");
            continue;
        }

        let sig_path = alloc::format!("{}/assets/{}.sig", UPDATE_BASE, spec.remote_filename);
        let sig_data = match super::http::https_get(UPDATE_HOST, &sig_path, MAX_SIG_SIZE) {
            Ok(d) => d,
            Err(e) => { kprintln!("sig failed: {}", e); continue; }
        };

        let pubkey = &crate::update_key::UPDATE_PUB_KEY;
        if !crate::tls::certstore::verify_p384_prehash_384(pubkey, &hash, &sig_data) {
            kprintln!("signature invalid");
            continue;
        }

        let _ = crate::npkfs::delete(spec.npkfs_path);
        if let Err(e) = crate::npkfs::store(spec.npkfs_path, &asset_data, crate::capability::CAP_NULL) {
            kprintln!("store failed: {:?}", e);
            continue;
        }

        kprintln!("OK");
        updated += 1;
    }

    updated
}
