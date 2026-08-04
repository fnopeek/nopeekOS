//! OTA update intent.
//!
//! Downloads kernel from GitHub, verifies ECDSA P-384 signature,
//! writes to ESP FAT32 partition.

use crate::{kprintln, kprint};
use alloc::string::String;
use alloc::vec::Vec;

const UPDATE_HOST: &str = "raw.githubusercontent.com";
const UPDATE_BASE: &str = "/fnopeek/nopeekOS/main/release";
/// Hard ceiling on a kernel image we are willing to buffer. NOT the download
/// bound — that comes from the signed manifest (see below), so this only has
/// to be "implausible", not "current size plus guesswork".
///
/// It used to be 4 MiB and used directly as the download cap. When the kernel
/// crossed 4 MiB the fetch was silently truncated there, and OTA failed with a
/// confusing `Size mismatch` — the updater on the device could no longer
/// install any kernel, including the one that fixes this. Deriving the bound
/// from the manifest means the cap can never again drift away from reality.
const MAX_KERNEL_SIZE: usize = 64 * 1024 * 1024;
const MAX_MANIFEST_SIZE: usize = 4096;
const MAX_ASSET_MANIFEST_SIZE: usize = 16 * 1024;
/// 512 MB ceiling for OTA assets. The userspace bundle with Mesa/Wayland
/// runs ~270 MB; raw-githubusercontent caps at ~50–100 MB per file, so
/// anything above ~32 MB ships via GitHub Releases (asset manifest carries
/// an explicit `url=` line for those; redirect-following lives in
/// `https_get`).
const MAX_ASSET_SIZE: usize = 512 * 1024 * 1024;
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
    AssetSpec { section: "font:ibm-plex-mono",  remote_filename: "ibm-plex-mono.ttf",         npkfs_path: "sys/fonts/ibm-plex-mono" },
    // Both faces are SIL OFL 1.1: the licence must accompany every copy,
    // so an OTA-updated system pulls it alongside the font rather than
    // only fresh installs getting it from the bundled assets.
    AssetSpec { section: "font:LICENSE-Inter",   remote_filename: "LICENSE-Inter.txt",        npkfs_path: "sys/fonts/LICENSE-Inter" },
    AssetSpec { section: "font:LICENSE-IBM-Plex", remote_filename: "LICENSE-IBM-Plex.txt",   npkfs_path: "sys/fonts/LICENSE-IBM-Plex" },
    AssetSpec { section: "icons:phosphor",      remote_filename: "phosphor.atlas",            npkfs_path: "sys/icons/phosphor" },
    AssetSpec { section: "microvm:initramfs",   remote_filename: "microvm-initramfs.cpio.gz", npkfs_path: "sys/microvm/initramfs.cpio.gz" },
    AssetSpec { section: "microvm:linux-virt",  remote_filename: "linux-virt.bzImage",        npkfs_path: "sys/microvm/linux-virt.bzImage" },
    // Optional userspace bundle — Alpine minirootfs + busybox + (future)
    // Wayland/Mesa/LibreWolf. Built by `microvm-userspace/build.sh`.
    // Distinct from `microvm:initramfs` (which is just our PID-1, always
    // present). If a release ships this asset, OTA pulls it; if not, the
    // entry is absent in the asset manifest and we keep whatever's
    // already installed (or nothing).
    //
    // Small bundles (<~30 MB) live in `release/assets/` on the `main`
    // branch and ship via raw.githubusercontent.com. Larger bundles
    // (Mesa+Wayland is ~270 MB) live on GitHub Releases — the asset
    // manifest carries a `url=` override per entry and `https_get`
    // follows the 302 redirect chain to objects.githubusercontent.com.
    AssetSpec { section: "microvm:userspace",   remote_filename: "microvm-userspace.cpio.gz", npkfs_path: "sys/microvm/userspace.cpio.gz" },
    // Squashfs form of the userspace bundle — read-only, mounted by
    // PID-1 from /dev/vdb (slot-5 virtio-blk) instead of unpacked into
    // a tmpfs initramfs. The RAM-efficient daily-driver path; supersedes
    // the cpio entry above once it's the only shipped form.
    AssetSpec { section: "microvm:userspace-sqfs", remote_filename: "microvm-userspace.sqfs",  npkfs_path: "sys/microvm/userspace.sqfs" },
];

struct AssetEntry {
    section: String,
    size: usize,
    sha384: [u8; 48],
    /// Optional explicit URL — when present, fetched verbatim instead of
    /// `https://{UPDATE_HOST}{UPDATE_BASE}/assets/<remote_filename>`. The
    /// `.sig` sidecar URL is derived by appending `.sig` to this URL.
    url: Option<String>,
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
    super::clear_cancel(); // arm Ctrl+C cancel for this OTA run
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

        // 2. Download kernel (UEFI PE+ binary).
        //
        // The bound is the manifest's own size, not a fixed constant, so a
        // growing kernel can never again outgrow its downloader. The manifest
        // is not yet authenticated at this point — the SHA-384 and signature
        // checks below are what make it trustworthy — so it is only allowed to
        // *lower* our appetite, never raise it past MAX_KERNEL_SIZE.
        if manifest.size == 0 || manifest.size > MAX_KERNEL_SIZE {
            kprintln!("[npk] Refusing implausible kernel size: {} bytes (max {})",
                manifest.size, MAX_KERNEL_SIZE);
            return;
        }
        kprintln!("[npk] Downloading kernel.efi ({} KB)...", manifest.size / 1024);
        let kernel_path = alloc::format!("{}/kernel.efi", UPDATE_BASE);
        let kernel_data = match super::http::https_get(UPDATE_HOST, &kernel_path, manifest.size) {
            Ok(d) => d,
            Err(e) => { kprintln!("[npk] Download failed: {}", e); return; }
        };

        if kernel_data.len() != manifest.size {
            kprintln!("[npk] Short download: got {} of {} bytes", kernel_data.len(), manifest.size);
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
    let mut url: Option<String> = None;

    let flush = |section: &mut Option<String>,
                 size: &mut Option<usize>,
                 sha: &mut Option<[u8; 48]>,
                 url: &mut Option<String>,
                 out: &mut Vec<AssetEntry>| {
        if let (Some(s), Some(sz), Some(sh)) = (section.take(), size.take(), sha.take()) {
            out.push(AssetEntry { section: s, size: sz, sha384: sh, url: url.take() });
        } else {
            // Section header without all fields — discard whatever partial
            // state we collected so it doesn't leak into the next entry.
            *url = None;
        }
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            flush(&mut section, &mut size, &mut sha384, &mut url, &mut entries);
            section = Some(String::from(&line[1..line.len() - 1]));
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            match key.trim() {
                "size" => size = val.trim().parse::<usize>().ok(),
                "sha384" => sha384 = hex_to_bytes48(val.trim()).ok(),
                "url" => url = Some(String::from(val.trim())),
                _ => {}
            }
        }
    }
    flush(&mut section, &mut size, &mut sha384, &mut url, &mut entries);
    Ok(entries)
}

/// Split a full `https://host/path` URL into (host, path). Used to feed
/// `https_get` (which expects them separate) from a manifest `url=` line.
fn split_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    match rest.find('/') {
        Some(i) => Some((&rest[..i], &rest[i..])),
        None => Some((rest, "/")),
    }
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
        // A `[wallpaper:<name>]` section needs NO compile-time entry: the
        // section name IS the filename, so shipping a new wallpaper is
        // dropping a file into `release/assets/wallpapers/` — no kernel
        // change and no reinstall. The trust chain is unchanged: size and
        // sha384 come from the manifest and every asset is still checked
        // against its own detached signature below.
        let (npkfs_path, remote_filename) = match ASSETS.iter().find(|s| s.section == entry.section) {
            Some(s) => (String::from(s.npkfs_path), String::from(s.remote_filename)),
            None => match entry.section.strip_prefix("wallpaper:").filter(|n| safe_asset_name(n)) {
                Some(name) => (
                    alloc::format!("sys/wallpapers/{}", name),
                    alloc::format!("wallpapers/{}", name),
                ),
                None => {
                    kprintln!("[npk]   unknown asset [{}] (skipping)", entry.section);
                    continue;
                }
            },
        };
        let spec = AssetRef { npkfs_path: &npkfs_path, remote_filename: &remote_filename };

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

        // ── Make room before a streaming write ──────────────────────
        // The streaming writer keeps the OLD copy live until finish(),
        // so a refresh transiently needs the new asset's size ON TOP of
        // everything already stored — 2× for a same-path replace. A
        // previously-aborted download also leaks orphaned chunks that
        // only gc reclaims. On a tight partition a 261 MB bundle can't
        // afford either, which is why a fresh fetch failed ~40 MB in
        // with a bare "npkfs write failed" (= DiskFull). Reclaim and
        // free up front; bail with a clear message if it still won't fit.
        const BLOCK: u64 = 4096;
        let free_bytes = || crate::npkfs::stats().map(|(_, f, _, _)| f * BLOCK).unwrap_or(0);
        let need = entry.size as u64;

        // 1. Reclaim orphans from any earlier aborted streaming download.
        if free_bytes() < need {
            if let Ok(g) = crate::storage::npkfs::fs::gc() {
                if g.removed > 0 {
                    kprintln!("[npk]   gc reclaimed {} orphaned object(s)", g.removed);
                }
            }
        }
        // 2. Still short and an old copy exists → drop it first so we
        //    don't need 2× the asset size. We're replacing it anyway;
        //    on a disk this tight, keeping both isn't an option. The
        //    path unlink orphans the chunk blobs; gc reclaims them.
        if free_bytes() < need && local_hash.is_some() {
            if crate::npkfs::delete(spec.npkfs_path).is_ok() {
                let _ = crate::storage::npkfs::fs::gc();
                kprintln!("[npk]   freed old {} to make room", spec.npkfs_path);
            }
        }
        // 3. Truly out of space — fail clearly instead of 40 MB in.
        if free_bytes() < need {
            kprintln!("[npk]   skipping {}: disk full (need {} MB, {} MB free)",
                spec.npkfs_path, need / (1024 * 1024), free_bytes() / (1024 * 1024));
            continue;
        }

        kprint!("[npk]   downloading {} ({} KB)... ", spec.remote_filename, entry.size / 1024);

        // Two URL paths:
        //   (a) entry.url == Some(url)  → fetch verbatim (GitHub Releases).
        //       `https_get_streaming` follows 302 redirects so
        //       `github.com/.../releases/download/...` → the signed
        //       `objects.githubusercontent.com` CDN URL works transparently.
        //   (b) entry.url == None       → fall back to raw.githubusercontent
        //       on main (the existing flow for <30 MB assets).
        let (asset_host, asset_path_owned);
        let (asset_host_str, asset_path_str): (&str, &str) = if let Some(url) = &entry.url {
            match split_url(url) {
                Some((h, p)) => (h, p),
                None => { kprintln!("bad url"); continue; }
            }
        } else {
            asset_host = String::from(UPDATE_HOST);
            asset_path_owned = alloc::format!("{}/assets/{}", UPDATE_BASE, spec.remote_filename);
            (asset_host.as_str(), asset_path_owned.as_str())
        };

        // Streaming download: drive bytes straight into npkFS via the
        // ChunkedWriter, hashing SHA-384 incrementally as they pass.
        // Peak RAM = one 16 MiB chunk regardless of asset size — a
        // 1 GB userspace bundle no longer needs a 1 GB heap spike.
        let mut writer = match crate::npkfs::open_streaming_write(spec.npkfs_path) {
            Ok(w) => w,
            Err(e) => { kprintln!("npkfs open failed: {:?}", e); continue; }
        };
        let mut hasher = crate::tls::sha256::Sha384::new();
        let mut total_bytes: usize = 0;
        let mut write_err: Option<&'static str> = None;
        // Progress heartbeat every 8 MiB. The asset size is known
        // from the manifest so we can show a percentage — the
        // download blocks the shell, the line proves it's alive.
        const STEP: usize = 8 * 1024 * 1024;
        let expected = entry.size;
        let mut next_report: usize = STEP;
        let stream_result = super::http::https_get_streaming(
            asset_host_str,
            asset_path_str,
            MAX_ASSET_SIZE,
            &mut |chunk: &[u8]| -> Result<(), &'static str> {
                hasher.update(chunk);
                total_bytes = total_bytes.saturating_add(chunk.len());
                if let Err(_) = writer.write(chunk) {
                    write_err = Some("npkfs write failed");
                    return Err("npkfs write failed");
                }
                if total_bytes >= next_report {
                    let pct = if expected > 0 {
                        (total_bytes as u64 * 100 / expected as u64) as usize
                    } else { 0 };
                    kprintln!("[npk]     {} / {} MiB ({}%)",
                        total_bytes / (1024 * 1024),
                        expected / (1024 * 1024),
                        pct);
                    next_report = total_bytes + STEP;
                }
                Ok(())
            },
        );
        match stream_result {
            Ok(_) => {}
            Err(e) => {
                kprintln!("failed: {}{}",
                    e,
                    write_err.map(|w| alloc::format!(" ({})", w)).unwrap_or_default());
                // Drop the partial writer and sweep the chunks it already
                // flushed — they're unreachable (finish() never ran), so a
                // retry starts from a clean slate instead of accumulating
                // orphaned space across attempts.
                drop(writer);
                let _ = crate::storage::npkfs::fs::gc();
                continue;
            }
        }
        if total_bytes != entry.size {
            kprintln!("size mismatch (got {} expected {})", total_bytes, entry.size);
            continue;
        }

        let hash = hasher.finalize();
        if hash != entry.sha384 {
            kprintln!("checksum failed");
            // Drop the writer without finishing — flushed chunks
            // remain in storage but are unreachable from the path
            // tree, so the next `gc()` cycle reclaims them.
            continue;
        }

        // Sig URL: `<asset_url>.sig` if url= override, else default path.
        let sig_host_owned;
        let sig_path_owned;
        let (sig_host_str, sig_path_str): (&str, &str) = if let Some(url) = &entry.url {
            let sig_full = alloc::format!("{}.sig", url);
            match split_url(&sig_full) {
                Some((h, p)) => {
                    sig_host_owned = String::from(h);
                    sig_path_owned = String::from(p);
                    (sig_host_owned.as_str(), sig_path_owned.as_str())
                }
                None => { kprintln!("bad sig url"); continue; }
            }
        } else {
            sig_host_owned = String::from(UPDATE_HOST);
            sig_path_owned = alloc::format!("{}/assets/{}.sig", UPDATE_BASE, spec.remote_filename);
            (sig_host_owned.as_str(), sig_path_owned.as_str())
        };

        let sig_data = match super::http::https_get(sig_host_str, sig_path_str, MAX_SIG_SIZE) {
            Ok(d) => d,
            Err(e) => { kprintln!("sig failed: {}", e); continue; }
        };

        let pubkey = &crate::update_key::UPDATE_PUB_KEY;
        if !crate::tls::certstore::verify_p384_prehash_384(pubkey, &hash, &sig_data) {
            kprintln!("signature invalid");
            // Writer is dropped without finish; chunks become
            // unreachable, gc reclaims them on next pass.
            continue;
        }

        // Commit: writer.finish() publishes the chunked file
        // atomically. Replaces any existing entry at the same path.
        match writer.finish() {
            Ok(_) => {}
            Err(e) => { kprintln!("publish failed: {:?}", e); continue; }
        }

        kprintln!("OK");
        // A system wallpaper is only reachable through the user's own
        // wallpapers/ folder — that is the single directory `wallpaper
        // list` and `wallpaper set` read. Refresh the user copy so an OTA
        // wallpaper actually appears, and so REPLACING npk01 replaces what
        // the user sees rather than leaving the install-time copy behind.
        if let Some(name) = spec.npkfs_path.strip_prefix("sys/wallpapers/") {
            copy_wallpaper_to_user(name);
        }
        updated += 1;
    }

    updated
}

/// A manifest-supplied asset name we are willing to turn into a path.
/// Deliberately strict — the name becomes part of an npkFS path, so anything
/// that could climb out of `sys/wallpapers/` is refused. The manifest is
/// signature-checked per asset, but a name is not a place to be trusting.
fn safe_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'_')
}

/// Mirror `sys/wallpapers/<name>` into the user's wallpapers folder. Unlike
/// the installer's copy this OVERWRITES: a system wallpaper is system-managed,
/// and the point of shipping one over OTA is that the picture on screen
/// changes. A wallpaper the user added under a different name is untouched.
fn copy_wallpaper_to_user(name: &str) {
    use crate::security::capability::CAP_NULL;
    let Some(user) = crate::config::get("name").filter(|n| !n.is_empty()) else { return };
    let Ok((bytes, _)) = crate::npkfs::fetch(&alloc::format!("sys/wallpapers/{}", name)) else { return };
    let target = alloc::format!("home/{}/pictures/wallpapers/{}", user, name);
    match crate::npkfs::store(&target, &bytes, CAP_NULL) {
        Ok(_) => kprintln!("[npk]   {} (user copy refreshed)", target),
        Err(e) => kprintln!("[npk]   user copy failed: {} — {:?}", target, e),
    }
}

/// One asset's two paths, borrowed — the static table and the dynamic
/// wallpaper case produce the same shape.
struct AssetRef<'a> {
    npkfs_path: &'a str,
    remote_filename: &'a str,
}
