//! npk install — module package manager.
//!
//! Downloads WASM modules from GitHub release/modules/,
//! verifies ECDSA P-384 signature + SHA-384 hash,
//! stores in npkFS under sys/wasm/<name>.

use crate::{kprintln, kprint};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const MODULE_HOST: &str = "raw.githubusercontent.com";
const MODULE_BASE: &str = "/fnopeek/nopeekOS/main/release/modules";
const MAX_MODULE_SIZE: usize = 16 * 1024 * 1024; // 16 MB (buffer is min(content_length, cap), so no waste)
const MAX_MANIFEST_SIZE: usize = 8192;
const MAX_SIG_SIZE: usize = 512;


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
    // Bound the fetch by the manifest's own size rather than a fixed cap, so a
    // growing module can never be silently truncated at a constant nobody
    // remembered to raise (that is exactly how OTA broke when the kernel
    // crossed 4 MiB). The manifest is unauthenticated here — SHA-384 and the
    // signature below are the real gate — so it may only lower the bound.
    if entry.size == 0 || entry.size > MAX_MODULE_SIZE {
        kprintln!("[npk] Refusing implausible module size: {} bytes (max {})",
            entry.size, MAX_MODULE_SIZE);
        return;
    }
    let wasm_data = match super::http::https_get(MODULE_HOST, &wasm_path, entry.size) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk] Download failed: {}", e); return; }
    };

    if wasm_data.len() != entry.size {
        kprintln!("[npk] Short download: got {} of {} bytes", wasm_data.len(), entry.size);
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

    kprintln!("[npk] ✓ {} v{} installed.", name, entry.version);
}

/// One installed module that the release has a newer build of.
pub struct ModulePlan {
    pub name: String,
    /// Installed version — `None` when the module has no `.version` sidecar.
    pub local: Option<String>,
    pub remote: String,
    pub size: usize,
    sha384: [u8; 48],
}

/// Compare installed modules against the release manifest WITHOUT touching
/// anything. Returns what needs updating plus how many were already current,
/// so the caller can show a plan before asking to apply it.
pub fn plan_modules() -> (Vec<ModulePlan>, usize) {
    let manifest_path = alloc::format!("{}/manifest", MODULE_BASE);
    let manifest_data = match super::http::https_get(MODULE_HOST, &manifest_path, MAX_MANIFEST_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk]   ! module manifest: {}", e); return (Vec::new(), 0); }
    };

    let modules = match parse_manifest(&manifest_data) {
        Ok(m) => m,
        Err(e) => { kprintln!("[npk]   ! module manifest: {}", e); return (Vec::new(), 0); }
    };

    // List installed modules straight from `sys/wasm` instead of
    // walking the entire FS.
    let installed: Vec<String> =
        match crate::npkfs::fs::list("sys/wasm") {
            Ok(Some(v)) => v.iter()
                .filter(|e| matches!(e.kind, crate::npkfs::object::EntryKind::File))
                .map(|e| e.name.clone())
                .collect(),
            Ok(None) => Vec::new(),
            Err(_) => return (Vec::new(), 0),
        };

    let mut plan = Vec::new();
    let mut current = 0usize;

    for remote in &modules {
        // Only update modules that are already installed (skip
        // `.version` sidecars; we want bare `<name>` to be present).
        if !installed.iter().any(|n| n.as_str() == remote.name) { continue; }

        let version_key = alloc::format!("sys/wasm/{}.version", remote.name);
        let local = crate::npkfs::fetch(&version_key).ok()
            .and_then(|(data, _)| core::str::from_utf8(&data).ok().map(|s| String::from(s.trim())));
        let remote_ver = String::from(remote.version.trim());

        if local.as_deref() == Some(remote_ver.as_str()) {
            current += 1;
            continue;
        }
        plan.push(ModulePlan {
            name: remote.name.clone(),
            local,
            remote: remote_ver,
            size: remote.size,
            sha384: remote.sha384,
        });
    }

    (plan, current)
}

/// Download, verify and install one planned module. Prints its own result
/// line; returns whether the module was written.
pub fn apply_module(p: &ModulePlan) -> bool {
    let store_name = alloc::format!("sys/wasm/{}", p.name);
    let version_key = alloc::format!("sys/wasm/{}.version", p.name);

    // A COMPLETE line before the work, never one left open waiting for its
    // `OK`: the download itself prints (redirects, ESP writes, progress), and
    // the tail then landed on some later line. Success adds nothing — the
    // summary counts it; only a failure speaks again, on its own `!` line.
    kprintln!("[npk]   + module   {:<10} {}", p.name, p.remote);
    let fail = |msg: core::fmt::Arguments| {
        kprintln!("[npk]   ! module   {:<10} {}", p.name, msg);
        false
    };

    if p.size == 0 || p.size > MAX_MODULE_SIZE {
        return fail(format_args!("implausible size {} (max {})", p.size, MAX_MODULE_SIZE));
    }
    let wasm_path = alloc::format!("{}/{}.wasm", MODULE_BASE, p.name);
    let wasm_data = match super::http::https_get(MODULE_HOST, &wasm_path, p.size) {
        Ok(d) => d,
        Err(e) => return fail(format_args!("download: {}", e)),
    };

    if wasm_data.len() != p.size {
        return fail(format_args!("short download ({} of {})", wasm_data.len(), p.size));
    }

    let hash = crate::tls::sha256::sha384(&wasm_data);
    if hash != p.sha384 {
        return fail(format_args!("checksum mismatch"));
    }

    let sig_path = alloc::format!("{}/{}.sig", MODULE_BASE, p.name);
    let sig_data = match super::http::https_get(MODULE_HOST, &sig_path, MAX_SIG_SIZE) {
        Ok(d) => d,
        Err(e) => return fail(format_args!("signature: {}", e)),
    };

    let pubkey = &crate::update_key::UPDATE_PUB_KEY;
    if !crate::tls::certstore::verify_p384_prehash_384(pubkey, &hash, &sig_data) {
        return fail(format_args!("signature invalid"));
    }

    // Delete old module + version before storing new one (npkFS doesn't overwrite)
    let _ = crate::npkfs::delete(&store_name);
    let _ = crate::npkfs::delete(&version_key);

    if crate::npkfs::store(&store_name, &wasm_data, crate::capability::CAP_NULL).is_ok() {
        let _ = crate::npkfs::store(&version_key, p.remote.as_bytes(), crate::capability::CAP_NULL);
        true
    } else {
        fail(format_args!("store failed"))
    }
}

/// `uninstall <name> [--force]` — remove a WASM module, with safety
/// guards that prevent the user from bricking their system:
///
///  1. **Hard block:** the module configured as the active launcher
///     (`sys/config/launcher`, default `drun`) cannot be uninstalled —
///     without it, Mod+D / spawn flow has nothing to open. The user
///     must point `sys/config/launcher` somewhere else first.
///
///  2. **--force gate for bundled modules:** every kernel-bundled
///     module (drun, loft, wifi, wallpaper, top, debug, …) is on the
///     OTA recovery path, so removing one is reversible — but easy
///     to do by accident. Without `--force` we refuse and print the
///     reinstall hint. User-installed third-party modules (none yet)
///     skip this check.
///
/// The block + gate are deliberately implemented in the kernel rather
/// than in a wrapper script: the intent loop is the only path that
/// reaches `npkfs::delete` for `sys/wasm/*`, so this is the right
/// place to keep the invariants honest.
pub fn intent_uninstall(args: &str) {
    let mut name = "";
    let mut force = false;
    for tok in args.split_whitespace() {
        if tok == "--force" || tok == "-f" {
            force = true;
        } else if name.is_empty() {
            name = tok;
        } else {
            kprintln!("[npk] Usage: uninstall <module> [--force]");
            return;
        }
    }
    if name.is_empty() {
        kprintln!("[npk] Usage: uninstall <module> [--force]");
        return;
    }

    // Guard 1: never uninstall the configured launcher.
    if is_active_launcher(name) {
        kprintln!("[npk] Cannot uninstall '{}' — it is the active launcher.", name);
        kprintln!("[npk] Point `sys/config/launcher` to a different module first,");
        kprintln!("[npk] then re-run uninstall.");
        return;
    }

    // Same for the file dialog: without it `npk_pick` has nothing to
    // spawn, and every app loses Open and Save at once.
    if is_active_picker(name) {
        kprintln!("[npk] Cannot uninstall '{}' — it is the active file dialog.", name);
        kprintln!("[npk] Point `sys/config/picker` to a different module first,");
        kprintln!("[npk] then re-run uninstall.");
        return;
    }

    // Guard 2: bundled modules need --force.
    let store_name = alloc::format!("sys/wasm/{}", name);
    let bundled = is_bundled_module(name);
    if bundled && !force {
        kprintln!("[npk] '{}' is a system module bundled with the kernel.", name);
        kprintln!("[npk] Removing it leaves the OTA path as the only recovery.");
        kprintln!("[npk] Re-run with `--force` if you really want to remove it:");
        kprintln!("[npk]   uninstall {} --force", name);
        return;
    }

    let version_key = alloc::format!("sys/wasm/{}.version", name);
    match crate::npkfs::delete(&store_name) {
        Ok(()) => {
            let _ = crate::npkfs::delete(&version_key);
            if bundled {
                kprintln!("[npk] {} removed (bundled — recover with `install {}`).",
                          name, name);
            } else {
                kprintln!("[npk] {} removed.", name);
            }
        }
        Err(_) => kprintln!("[npk] Module '{}' not installed.", name),
    }
}

/// True iff `name` matches the module serving `npk_pick`
/// (`sys/config/picker`, default `pick` — same fallback as the kernel).
fn is_active_picker(name: &str) -> bool {
    let configured = match crate::npkfs::fetch("sys/config/picker") {
        Ok((data, _)) => match core::str::from_utf8(&data) {
            Ok(s) => s.trim().to_string(),
            Err(_) => alloc::string::String::from("pick"),
        },
        Err(_) => alloc::string::String::from("pick"),
    };
    let configured = if configured.is_empty() {
        alloc::string::String::from("pick")
    } else {
        configured
    };
    configured == name
}

/// True iff `name` matches the active launcher (`sys/config/launcher`).
/// Empty / unset config falls back to `drun` to mirror the boot path.
fn is_active_launcher(name: &str) -> bool {
    let configured = match crate::npkfs::fetch("sys/config/launcher") {
        Ok((data, _)) => match core::str::from_utf8(&data) {
            Ok(s) => s.trim().to_string(),
            Err(_) => alloc::string::String::from("drun"),
        },
        Err(_) => alloc::string::String::from("drun"),
    };
    let configured = if configured.is_empty() {
        alloc::string::String::from("drun")
    } else {
        configured
    };
    configured == name
}

/// True iff `name` is shipped as a bundled asset by the kernel.
///
/// The canonical list lives in `install_data/assets/mod.rs` as
/// `BUNDLED_ASSETS`, but that module is `#[cfg(feature = "installer")]`
/// — only the installer build embeds the wasm bytes. The runtime
/// kernel (this code path) needs the names but not the bytes, so we
/// keep a parallel string list here. Keep both in sync when adding
/// or removing a bundled module.
fn is_bundled_module(name: &str) -> bool {
    const BUNDLED_NAMES: &[&str] = &[
        "wallpaper",
        "top",
        "debug",
        "drun",
        "loft",
        "dock",
        "bar",
        "testdisk",
        "spell",
        "pick",
        "iris",
        "snap",
    ];
    BUNDLED_NAMES.iter().any(|n| *n == name)
}

/// `modules` — list installed and available modules.
pub fn intent_modules() {
    let entries = match crate::npkfs::fs::list("sys/wasm") {
        Ok(Some(v)) => v,
        Ok(None) | Err(_) => { kprintln!("[npk] modules — none installed"); return; }
    };

    let mut rows: Vec<(String, String, u64)> = Vec::new();
    let mut bytes = 0u64;
    for e in &entries {
        if !matches!(e.kind, crate::npkfs::object::EntryKind::File) { continue; }
        if e.name.ends_with(".version") { continue; }
        let version_key = alloc::format!("sys/wasm/{}.version", e.name);
        // `.trim()` is not cosmetic: the sidecar carries a trailing newline,
        // and printing it un-trimmed put a blank line after every module that
        // had one.
        let version = crate::npkfs::fetch(&version_key).ok()
            .and_then(|(data, _)| core::str::from_utf8(&data).ok().map(|s| String::from(s.trim())))
            .unwrap_or_else(|| String::from("builtin"));
        bytes += e.size;
        rows.push((e.name.clone(), version, e.size));
    }

    if rows.is_empty() {
        kprintln!("[npk] modules — none installed");
        return;
    }

    kprintln!("[npk] modules — {} installed {}", rows.len(),
        super::update::fmt_size(bytes as usize));
    kprintln!("[npk]");
    let w = rows.iter().map(|(n, _, _)| n.len()).max().unwrap_or(8).max(8);
    let v = rows.iter().map(|(_, v, _)| v.len()).max().unwrap_or(6).max(6);
    for (name, version, size) in &rows {
        kprintln!("[npk]   * {:<w$}  {:<v$}  {}", name, version,
            super::update::fmt_size(*size as usize), w = w, v = v);
    }
}

/// `assets` — what the system carries besides modules: fonts, icons, the
/// microvm payloads, wallpapers. They arrive over the same signed OTA path
/// as modules but were invisible until now, so a 261 MB userspace bundle
/// sat on the disk with nothing to show it.
pub fn intent_assets() {
    // Directories rather than the update table: this lists what npkFS
    // actually holds, including anything dropped in by hand.
    const DIRS: &[(&str, &str)] = &[
        ("fonts",      "sys/fonts"),
        ("icons",      "sys/icons"),
        ("microvm",    "sys/microvm"),
        ("wallpapers", "sys/wallpapers"),
    ];

    let mut groups: Vec<(&str, Vec<(String, u64)>)> = Vec::new();
    let mut total = 0u64;
    let mut count = 0usize;
    for (label, path) in DIRS {
        let Ok(Some(entries)) = crate::npkfs::fs::list(path) else { continue };
        let mut rows: Vec<(String, u64)> = entries.iter()
            .filter(|e| matches!(e.kind, crate::npkfs::object::EntryKind::File))
            .map(|e| (e.name.clone(), e.size))
            .collect();
        if rows.is_empty() { continue }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        total += rows.iter().map(|(_, s)| *s).sum::<u64>();
        count += rows.len();
        groups.push((label, rows));
    }

    if groups.is_empty() {
        kprintln!("[npk] assets — none");
        return;
    }

    kprintln!("[npk] assets — {} objects {}", count, super::update::fmt_size(total as usize));
    let w = groups.iter()
        .flat_map(|(_, rows)| rows.iter().map(|(n, _)| n.len()))
        .max().unwrap_or(12).max(12);
    for (label, rows) in &groups {
        kprintln!("[npk]");
        kprintln!("[npk]   {}", label);
        for (name, size) in rows {
            kprintln!("[npk]     * {:<w$}  {}", name,
                super::update::fmt_size(*size as usize), w = w);
        }
    }
}
