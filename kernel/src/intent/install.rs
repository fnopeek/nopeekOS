//! npk install — module package manager.
//!
//! Downloads WASM modules from GitHub release/modules/,
//! verifies ECDSA P-384 signature + SHA-384 hash,
//! stores in npkFS under sys/wasm/<name>.

use crate::{kprintln, kprint};
use alloc::string::String;
use alloc::vec::Vec;

const MODULE_HOST: &str = "raw.githubusercontent.com";
const MODULE_BASE: &str = "/fnopeek/nopeekOS/main/release/modules";
const MAX_MODULE_SIZE: usize = 2 * 1024 * 1024; // 2 MB
const MAX_MANIFEST_SIZE: usize = 8192;
const MAX_SIG_SIZE: usize = 512;

pub const APP_META_SECTION: &str = ".npk.app_meta";

pub fn cache_app_meta(name: &str, wasm_data: &[u8]) {
    let meta_key = alloc::format!("sys/meta/{}", name);
    let _ = crate::npkfs::delete(&meta_key);

    let bytes = match crate::wasm_meta::extract_custom_section(wasm_data, APP_META_SECTION) {
        Some(b) => b,
        None    => return,
    };

    if let Err(e) = crate::npkfs::store(&meta_key, bytes, crate::capability::CAP_NULL) {
        kprintln!("[npk] Warning: failed to cache app meta for {}: {:?}", name, e);
    }
}

struct ModuleEntry {
    name: String,
    version: String,
    size: usize,
    sha384: [u8; 48],
}

/// Parse the module manifest (one module per block, separated by blank lines).
/// Format:
/// ```
/// [wallpaper]
/// version=0.1.0
/// size=12345
/// sha384=abcdef...
/// ```
fn parse_manifest(data: &[u8]) -> Result<Vec<ModuleEntry>, &'static str> {
    let text = core::str::from_utf8(data).map_err(|_| "manifest: invalid UTF-8")?;
    let mut modules = Vec::new();
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut size: Option<usize> = None;
    let mut sha384: Option<[u8; 48]> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            // End of block — flush if complete
            if let (Some(n), Some(v), Some(s), Some(h)) = (name.take(), version.take(), size.take(), sha384.take()) {
                modules.push(ModuleEntry { name: n, version: v, size: s, sha384: h });
            }
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            // Flush previous if any
            if let (Some(n), Some(v), Some(s), Some(h)) = (name.take(), version.take(), size.take(), sha384.take()) {
                modules.push(ModuleEntry { name: n, version: v, size: s, sha384: h });
            }
            name = Some(String::from(&line[1..line.len() - 1]));
            version = None;
            size = None;
            sha384 = None;
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            match key.trim() {
                "version" => version = Some(String::from(val.trim())),
                "size" => size = val.trim().parse::<usize>().ok(),
                "sha384" => sha384 = hex_to_bytes48(val.trim()).ok(),
                _ => {}
            }
        }
    }
    // Flush last block
    if let (Some(n), Some(v), Some(s), Some(h)) = (name, version, size, sha384) {
        modules.push(ModuleEntry { name: n, version: v, size: s, sha384: h });
    }

    Ok(modules)
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

/// `install <name>` — download and install a WASM module.
pub fn intent_install(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: install <module>");
        kprintln!("[npk] Use 'modules' to list available modules.");
        return;
    }

    kprintln!("[npk] Fetching module manifest...");
    let manifest_path = alloc::format!("{}/manifest", MODULE_BASE);
    let manifest_data = match super::http::https_get(MODULE_HOST, &manifest_path, MAX_MANIFEST_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk] Failed to fetch manifest: {}", e); return; }
    };

    let modules = match parse_manifest(&manifest_data) {
        Ok(m) => m,
        Err(e) => { kprintln!("[npk] {}", e); return; }
    };

    let entry = match modules.iter().find(|m| m.name == name) {
        Some(e) => e,
        None => {
            kprintln!("[npk] Module '{}' not found.", name);
            kprintln!("[npk] Available: {}", modules.iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", "));
            return;
        }
    };

    // Check if already installed with same version
    let store_name = alloc::format!("sys/wasm/{}", name);
    let version_key = alloc::format!("sys/wasm/{}.version", name);
    if let Ok((ver_data, _)) = crate::npkfs::fetch(&version_key) {
        if let Ok(installed_ver) = core::str::from_utf8(&ver_data) {
            if installed_ver.trim() == entry.version.trim() {
                kprintln!("[npk] {} v{} already installed.", name, entry.version);
                return;
            }
            kprintln!("[npk] Updating {} v{} -> v{}", name, installed_ver.trim(), entry.version);
        }
    } else {
        kprintln!("[npk] Installing {} v{} ({} bytes)...", name, entry.version, entry.size);
    }

    // Download module
    let wasm_path = alloc::format!("{}/{}.wasm", MODULE_BASE, name);
    let wasm_data = match super::http::https_get(MODULE_HOST, &wasm_path, MAX_MODULE_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk] Download failed: {}", e); return; }
    };

    if wasm_data.len() != entry.size {
        kprintln!("[npk] Size mismatch: got {} expected {}", wasm_data.len(), entry.size);
        return;
    }

    // Verify SHA-384
    kprint!("[npk] Verifying SHA-384... ");
    let hash = crate::tls::sha256::sha384(&wasm_data);
    if hash != entry.sha384 {
        kprintln!("FAILED");
        kprintln!("[npk] Checksum mismatch! Install rejected.");
        return;
    }
    kprintln!("OK");

    // Verify ECDSA P-384 signature
    kprint!("[npk] Verifying signature... ");
    let sig_path = alloc::format!("{}/{}.sig", MODULE_BASE, name);
    let sig_data = match super::http::https_get(MODULE_HOST, &sig_path, MAX_SIG_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("FAILED ({})", e); return; }
    };

    let pubkey = &crate::update_key::UPDATE_PUB_KEY;
    if !crate::tls::certstore::verify_p384_prehash_384(pubkey, &hash, &sig_data) {
        kprintln!("FAILED");
        kprintln!("[npk] Invalid signature! Install rejected.");
        return;
    }
    kprintln!("OK");

    // Delete old version before storing (npkFS doesn't overwrite)
    let _ = crate::npkfs::delete(&store_name);
    let _ = crate::npkfs::delete(&version_key);

    // Store in npkFS
    if let Err(e) = crate::npkfs::store(&store_name, &wasm_data, crate::capability::CAP_NULL) {
        kprintln!("[npk] Failed to store module: {:?}", e);
        return;
    }

    // Store version metadata
    let _ = crate::npkfs::store(&version_key, entry.version.as_bytes(), crate::capability::CAP_NULL);

    // Cache app metadata (icon/display_name/description) from WASM custom section.
    cache_app_meta(name, &wasm_data);

    kprintln!("[npk] ✓ {} v{} installed.", name, entry.version);
}

/// Update all installed WASM modules to latest versions.
/// Called by `update` intent after kernel update.
/// Returns number of modules updated.
pub fn update_all_modules() -> usize {
    let manifest_path = alloc::format!("{}/manifest", MODULE_BASE);
    let manifest_data = match super::http::https_get(MODULE_HOST, &manifest_path, MAX_MANIFEST_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk] Module manifest fetch failed: {}", e); return 0; }
    };

    let modules = match parse_manifest(&manifest_data) {
        Ok(m) => m,
        Err(e) => { kprintln!("[npk] Module manifest parse error: {}", e); return 0; }
    };

    // Get list of installed modules
    let entries = match crate::npkfs::list() {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut updated = 0;

    for remote in &modules {
        let store_name = alloc::format!("sys/wasm/{}", remote.name);

        // Only update modules that are already installed
        let is_installed = entries.iter().any(|(n, _, _)| n == &store_name);
        if !is_installed { continue; }

        // Check version — skip if already up to date (trim for safety)
        let version_key = alloc::format!("sys/wasm/{}.version", remote.name);
        let installed_ver_str = crate::npkfs::fetch(&version_key).ok()
            .and_then(|(data, _)| core::str::from_utf8(&data).ok().map(|s| String::from(s.trim())));

        let remote_ver = remote.version.trim();

        if let Some(ref local_ver) = installed_ver_str {
            if local_ver == remote_ver {
                kprintln!("[npk]   {} v{} (up to date)", remote.name, local_ver);
                continue;
            }
            kprintln!("[npk]   {} v{} -> v{}", remote.name, local_ver, remote_ver);
        } else {
            kprintln!("[npk]   {} -> v{} (no local version)", remote.name, remote_ver);
        }

        // Download module
        kprint!("[npk]   downloading... ");
        let wasm_path = alloc::format!("{}/{}.wasm", MODULE_BASE, remote.name);
        let wasm_data = match super::http::https_get(MODULE_HOST, &wasm_path, MAX_MODULE_SIZE) {
            Ok(d) => d,
            Err(e) => { kprintln!("failed: {}", e); continue; }
        };

        if wasm_data.len() != remote.size {
            kprintln!("size mismatch (got {} expected {})", wasm_data.len(), remote.size);
            continue;
        }

        // Verify SHA-384
        let hash = crate::tls::sha256::sha384(&wasm_data);
        if hash != remote.sha384 {
            kprintln!("checksum failed");
            continue;
        }

        // Verify ECDSA P-384 signature
        let sig_path = alloc::format!("{}/{}.sig", MODULE_BASE, remote.name);
        let sig_data = match super::http::https_get(MODULE_HOST, &sig_path, MAX_SIG_SIZE) {
            Ok(d) => d,
            Err(e) => { kprintln!("sig failed: {}", e); continue; }
        };

        let pubkey = &crate::update_key::UPDATE_PUB_KEY;
        if !crate::tls::certstore::verify_p384_prehash_384(pubkey, &hash, &sig_data) {
            kprintln!("signature invalid");
            continue;
        }

        // Delete old module + version before storing new one (npkFS doesn't overwrite)
        let _ = crate::npkfs::delete(&store_name);
        let _ = crate::npkfs::delete(&version_key);

        if crate::npkfs::store(&store_name, &wasm_data, crate::capability::CAP_NULL).is_ok() {
            let _ = crate::npkfs::store(&version_key, remote_ver.as_bytes(), crate::capability::CAP_NULL);
            cache_app_meta(&remote.name, &wasm_data);
            kprintln!("OK");
            updated += 1;
        } else {
            kprintln!("store failed");
        }
    }

    updated
}

/// `uninstall <name>` — remove a WASM module.
pub fn intent_uninstall(args: &str) {
    let name = args.trim();
    if name.is_empty() {
        kprintln!("[npk] Usage: uninstall <module>");
        return;
    }

    let store_name = alloc::format!("sys/wasm/{}", name);
    let version_key = alloc::format!("sys/wasm/{}.version", name);
    let meta_key = alloc::format!("sys/meta/{}", name);

    match crate::npkfs::delete(&store_name) {
        Ok(()) => {
            let _ = crate::npkfs::delete(&version_key);
            let _ = crate::npkfs::delete(&meta_key);
            kprintln!("[npk] {} removed.", name);
        }
        Err(_) => kprintln!("[npk] Module '{}' not installed.", name),
    }
}

/// `modules` — list installed and available modules.
pub fn intent_modules() {
    kprintln!("[npk] Installed modules:");

    // List installed modules from npkFS
    let entries = match crate::npkfs::list() {
        Ok(e) => e,
        Err(_) => { kprintln!("  (npkFS error)"); return; }
    };

    let mut found = false;
    for (name, _, _) in &entries {
        if name.starts_with("sys/wasm/") && !name.ends_with(".version") {
            let module_name = &name[9..]; // strip "sys/wasm/"
            let version_key = alloc::format!("{}.version", name);
            let version = crate::npkfs::fetch(&version_key).ok()
                .and_then(|(data, _)| core::str::from_utf8(&data).ok().map(String::from))
                .unwrap_or_else(|| String::from("builtin"));
            kprintln!("  {} v{}", module_name, version);
            found = true;
        }
    }

    if !found {
        kprintln!("  (none)");
    }
}
