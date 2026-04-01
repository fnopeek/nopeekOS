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
const MAX_SIG_SIZE: usize = 512;

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
    kprintln!("[npk] Size: {} bytes", manifest.size);

    if manifest.version == current {
        kprintln!("[npk] Already up to date.");
        return;
    }

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

    // 5. Verify ECDSA P-384 signature (signs raw kernel data with SHA-384)
    kprint!("[npk] Verifying ECDSA P-384 signature... ");
    let pubkey = &crate::update_key::UPDATE_PUB_KEY;
    if !crate::tls::certstore::verify_p384_sha384(pubkey, &kernel_data, &sig_data) {
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

    kprintln!("[npk] ====================================");
    kprintln!("[npk]  Update v{} installed!", manifest.version);
    kprintln!("[npk]  Type 'reboot' to apply.");
    kprintln!("[npk] ====================================");
}
