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

/// Human-readable size, three significant-ish digits, no float formatting.
fn fmt_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        let tenths = (bytes as u64 * 10 / (1024 * 1024)) as usize;
        alloc::format!("({}.{} MB)", tenths / 10, tenths % 10)
    } else {
        alloc::format!("({} KB)", (bytes + 1023) / 1024)
    }
}

/// Everything `update` found to do — built without changing a thing, so it
/// can be shown before it is applied.
struct Plan {
    kernel: Option<Manifest>,
    modules: Vec<super::install::ModulePlan>,
    modules_current: usize,
    assets: Vec<AssetJob>,
    assets_current: usize,
    /// Wallpapers whose system copy is current but whose user copy is gone.
    /// Restoring it is a local repair, but it still writes — so it is part of
    /// the plan instead of a side effect of looking.
    wallpaper_copies: Vec<String>,
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.kernel.is_none()
            && self.modules.is_empty()
            && self.assets.is_empty()
            && self.wallpaper_copies.is_empty()
    }

    fn count(&self) -> usize {
        self.kernel.iter().count() + self.modules.len()
            + self.assets.len() + self.wallpaper_copies.len()
    }

    /// "kernel v0.240.0, 18 modules, 14 assets" — everything that needs nothing.
    fn current_summary(&self) -> String {
        let mut bits = Vec::new();
        if self.kernel.is_none() {
            bits.push(alloc::format!("kernel v{}", env!("CARGO_PKG_VERSION")));
        }
        if self.modules_current > 0 { bits.push(alloc::format!("{} modules", self.modules_current)); }
        if self.assets_current > 0 { bits.push(alloc::format!("{} assets", self.assets_current)); }
        bits.join(", ")
    }
}

pub fn intent_update(args: &str) {
    super::clear_cancel(); // arm Ctrl+C cancel for this OTA run
    let mut assume_yes = false;
    let mut verbose = false;
    for tok in args.split_whitespace() {
        match tok {
            "-y" | "yes" | "apply" | "force" => assume_yes = true,
            "-v" | "verbose" => verbose = true,
            _ => {}
        }
    }

    // Answering "what changed" takes three manifest fetches plus one request
    // per item — their connect timings and status lines say nothing about the
    // question and buried the answer. `-v` puts them back.
    let _quiet = (!verbose).then(super::http::quiet);

    let Some(plan) = build_plan() else { return };

    if plan.is_empty() {
        // The whole point of the rewrite: nothing to do is ONE line, not one
        // line per module and per asset with the four that matter buried in it.
        kprintln!("[npk]   * everything current — {}", plan.current_summary());
        return;
    }

    kprintln!("[npk]");
    print_plan(&plan);
    kprintln!("[npk]");

    let n = plan.count();
    let question = if n == 1 { String::from("Apply 1 change?") }
                   else { alloc::format!("Apply {} changes?", n) };
    if !assume_yes && !super::confirm(&question) {
        kprintln!("[npk]   . nothing changed");
        return;
    }

    kprintln!("[npk]");
    apply_plan(plan);
}

/// Fetch the three manifests and diff them against what is installed.
/// Reads only — nothing here writes to the ESP or npkFS.
fn build_plan() -> Option<Plan> {
    kprintln!("[npk] update — {}{}", UPDATE_HOST, UPDATE_BASE);

    let manifest_path = alloc::format!("{}/manifest", UPDATE_BASE);
    let manifest_data = match super::http::https_get(UPDATE_HOST, &manifest_path, MAX_MANIFEST_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk]   ! manifest: {}", e); return None; }
    };
    let manifest = match parse_manifest(&manifest_data) {
        Ok(m) => m,
        Err(e) => { kprintln!("[npk]   ! {}", e); return None; }
    };

    let current = env!("CARGO_PKG_VERSION");
    // The manifest is not authenticated yet — the SHA-384 and signature
    // checks in `apply_kernel` are what make it trustworthy. Here it may only
    // *lower* our appetite, never raise it past MAX_KERNEL_SIZE.
    let kernel = if manifest.version == current {
        None
    } else if manifest.size == 0 || manifest.size > MAX_KERNEL_SIZE {
        kprintln!("[npk]   ! implausible kernel size {} (max {})", manifest.size, MAX_KERNEL_SIZE);
        None
    } else {
        Some(manifest)
    };

    let (modules, modules_current) = super::install::plan_modules();
    let (assets, assets_current, wallpaper_copies) = plan_assets();

    Some(Plan { kernel, modules, modules_current, assets, assets_current, wallpaper_copies })
}

fn print_plan(plan: &Plan) {
    let current = env!("CARGO_PKG_VERSION");
    if let Some(k) = &plan.kernel {
        kprintln!("[npk]   + kernel   v{} -> v{}  {}", current, k.version, fmt_size(k.size));
    }
    for m in &plan.modules {
        match &m.local {
            Some(v) => kprintln!("[npk]   + module   {:<10} {} -> {}", m.name, v, m.remote),
            None => kprintln!("[npk]   + module   {:<10} {}  (new)", m.name, m.remote),
        }
    }
    for a in &plan.assets {
        let what = if a.present { "" } else { "  (new)" };
        kprintln!("[npk]   + asset    {:<28} {}{}", a.npkfs_path, fmt_size(a.entry.size), what);
    }
    for w in &plan.wallpaper_copies {
        kprintln!("[npk]   + copy     {:<28} (user copy missing)", w);
    }

    // One line for everything that needs nothing — this used to be one line
    // per module and per asset, which buried the few that mattered.
    let rest = plan.current_summary();
    if !rest.is_empty() {
        kprintln!("[npk]   . {} current", rest);
    }
}

fn apply_plan(plan: Plan) {
    let mut kernel_done = false;
    if let Some(k) = &plan.kernel {
        kernel_done = apply_kernel(k);
    }

    let mut mods = 0;
    for m in &plan.modules {
        if super::install::apply_module(m) { mods += 1; }
    }

    let mut assets = 0;
    for a in &plan.assets {
        if apply_asset(a) { assets += 1; }
    }

    for name in &plan.wallpaper_copies {
        sync_wallpaper_to_user(name, false);
    }

    let plural = |n: usize, one: &str, many: &str| if n == 1 { String::from(one) } else { alloc::format!("{} {}", n, many) };
    kprintln!("[npk]");
    kprintln!("[npk]   * done — {}, {}",
        plural(mods, "1 module", "modules"), plural(assets, "1 asset", "assets"));
    if kernel_done {
        kprintln!("[npk]   * kernel installed — type 'reboot' to apply");
    }
}

/// Download, verify and write the kernel to the ESP.
fn apply_kernel(manifest: &Manifest) -> bool {
    kprint!("[npk]   + kernel   v{} {} ", manifest.version, fmt_size(manifest.size));
    let kernel_path = alloc::format!("{}/kernel.efi", UPDATE_BASE);
    let kernel_data = match super::http::https_get(UPDATE_HOST, &kernel_path, manifest.size) {
        Ok(d) => d,
        Err(e) => { kprintln!(""); kprintln!("[npk]   ! kernel download: {}", e); return false; }
    };
    if kernel_data.len() != manifest.size {
        kprintln!("");
        kprintln!("[npk]   ! kernel short download ({} of {})", kernel_data.len(), manifest.size);
        return false;
    }

    let hash = crate::tls::sha256::sha384(&kernel_data);
    if hash != manifest.sha384 {
        kprintln!("");
        kprintln!("[npk]   ! kernel checksum mismatch — rejected");
        return false;
    }

    let sig_path = alloc::format!("{}/kernel.sig", UPDATE_BASE);
    let sig_data = match super::http::https_get(UPDATE_HOST, &sig_path, MAX_SIG_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!(""); kprintln!("[npk]   ! kernel signature: {}", e); return false; }
    };
    let pubkey = &crate::update_key::UPDATE_PUB_KEY;
    if !crate::tls::certstore::verify_p384_prehash_384(pubkey, &hash, &sig_data) {
        kprintln!("");
        kprintln!("[npk]   ! kernel signature invalid — rejected");
        return false;
    }

    let esp_start = match crate::gpt::detect_esp_offset() {
        Some(s) => s,
        None => {
            kprintln!("");
            kprintln!("[npk]   ! no ESP partition found — is this a GPT disk?");
            return false;
        }
    };
    match crate::fat32::update_kernel(esp_start, &kernel_data) {
        Ok(()) => { kprintln!("OK"); true }
        Err(e) => {
            kprintln!("");
            kprintln!("[npk]   ! ESP write: {}", e);
            false
        }
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

/// One asset the release has a different build of, resolved to its paths.
struct AssetJob {
    entry: AssetEntry,
    npkfs_path: String,
    remote_filename: String,
    /// Whether a copy already exists locally (only differs in wording).
    present: bool,
}

/// Diff release/assets/manifest against npkFS-resident assets. Reads only:
/// returns the jobs to run, how many were already current, and the
/// wallpapers whose user copy needs restoring.
fn plan_assets() -> (Vec<AssetJob>, usize, Vec<String>) {
    let manifest_path = alloc::format!("{}/assets/manifest", UPDATE_BASE);
    let manifest_data = match super::http::https_get(UPDATE_HOST, &manifest_path, MAX_ASSET_MANIFEST_SIZE) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk]   ! asset manifest: {}", e); return (Vec::new(), 0, Vec::new()); }
    };

    let entries = match parse_asset_manifest(&manifest_data) {
        Ok(e) => e,
        Err(e) => { kprintln!("[npk]   ! asset manifest: {}", e); return (Vec::new(), 0, Vec::new()); }
    };

    let mut jobs = Vec::new();
    let mut current = 0usize;
    let mut wallpaper_copies = Vec::new();

    for entry in entries {
        // A `[wallpaper:<name>]` section needs NO compile-time entry: the
        // section name IS the filename, so shipping a new wallpaper is
        // dropping a file into `release/assets/wallpapers/` — no kernel
        // change and no reinstall. The trust chain is unchanged: size and
        // sha384 come from the manifest and every asset is still checked
        // against its own detached signature on apply.
        let (npkfs_path, remote_filename) = match ASSETS.iter().find(|s| s.section == entry.section) {
            Some(s) => (String::from(s.npkfs_path), String::from(s.remote_filename)),
            None => match entry.section.strip_prefix("wallpaper:").filter(|n| safe_asset_name(n)) {
                Some(name) => (
                    alloc::format!("sys/wallpapers/{}", name),
                    alloc::format!("wallpapers/{}", name),
                ),
                None => {
                    kprintln!("[npk]   . unknown asset [{}] (skipped)", entry.section);
                    continue;
                }
            },
        };

        let local_hash = crate::npkfs::fetch(&npkfs_path).ok()
            .map(|(data, _)| crate::tls::sha256::sha384(&data));

        if local_hash.as_ref() == Some(&entry.sha384) {
            current += 1;
            // The SYSTEM copy is current — the copy the user actually sees
            // may not be. `wallpaper list`/`set` read only the home folder,
            // and this hash check never looks there, so a deleted or missing
            // user copy stays missing however often `update` runs.
            if let Some(name) = npkfs_path.strip_prefix("sys/wallpapers/") {
                if user_wallpaper_missing(name) {
                    wallpaper_copies.push(String::from(name));
                }
            }
            continue;
        }

        let present = local_hash.is_some();
        jobs.push(AssetJob { entry, npkfs_path, remote_filename, present });
    }

    (jobs, current, wallpaper_copies)
}

/// Download, verify and store one planned asset. Prints its own result line;
/// returns whether the asset was written.
fn apply_asset(job: &AssetJob) -> bool {
    let entry = &job.entry;
    let spec = AssetRef { npkfs_path: &job.npkfs_path, remote_filename: &job.remote_filename };
    let local_present = job.present;
    {
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
                    kprintln!("[npk]   . gc reclaimed {} orphaned object(s)", g.removed);
                }
            }
        }
        // 2. Still short and an old copy exists → drop it first so we
        //    don't need 2× the asset size. We're replacing it anyway;
        //    on a disk this tight, keeping both isn't an option. The
        //    path unlink orphans the chunk blobs; gc reclaims them.
        if free_bytes() < need && local_present {
            if crate::npkfs::delete(spec.npkfs_path).is_ok() {
                let _ = crate::storage::npkfs::fs::gc();
                kprintln!("[npk]   . freed old {} to make room", spec.npkfs_path);
            }
        }
        // 3. Truly out of space — fail clearly instead of 40 MB in.
        if free_bytes() < need {
            kprintln!("[npk]   ! {} disk full (need {} MB, {} MB free)",
                spec.npkfs_path, need / (1024 * 1024), free_bytes() / (1024 * 1024));
            return false;
        }

        kprint!("[npk]   + asset    {} {} ", spec.npkfs_path, fmt_size(entry.size));

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
                None => { kprintln!("bad url"); return false; }
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
            Err(e) => { kprintln!("npkfs open failed: {:?}", e); return false; }
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
                return false;
            }
        }
        if total_bytes != entry.size {
            kprintln!("size mismatch (got {} expected {})", total_bytes, entry.size);
            return false;
        }

        let hash = hasher.finalize();
        if hash != entry.sha384 {
            kprintln!("checksum failed");
            // Drop the writer without finishing — flushed chunks
            // remain in storage but are unreachable from the path
            // tree, so the next `gc()` cycle reclaims them.
            return false;
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
                None => { kprintln!("bad sig url"); return false; }
            }
        } else {
            sig_host_owned = String::from(UPDATE_HOST);
            sig_path_owned = alloc::format!("{}/assets/{}.sig", UPDATE_BASE, spec.remote_filename);
            (sig_host_owned.as_str(), sig_path_owned.as_str())
        };

        let sig_data = match super::http::https_get(sig_host_str, sig_path_str, MAX_SIG_SIZE) {
            Ok(d) => d,
            Err(e) => { kprintln!("sig failed: {}", e); return false; }
        };

        let pubkey = &crate::update_key::UPDATE_PUB_KEY;
        if !crate::tls::certstore::verify_p384_prehash_384(pubkey, &hash, &sig_data) {
            kprintln!("signature invalid");
            // Writer is dropped without finish; chunks become
            // unreachable, gc reclaims them on next pass.
            return false;
        }

        // Commit: writer.finish() publishes the chunked file
        // atomically. Replaces any existing entry at the same path.
        match writer.finish() {
            Ok(_) => {}
            Err(e) => { kprintln!("publish failed: {:?}", e); return false; }
        }

        kprintln!("OK");
        // A system wallpaper is only reachable through the user's own
        // wallpapers/ folder — that is the single directory `wallpaper
        // list` and `wallpaper set` read. Refresh the user copy so an OTA
        // wallpaper actually appears, and so REPLACING npk01 replaces what
        // the user sees rather than leaving the install-time copy behind.
        if let Some(name) = spec.npkfs_path.strip_prefix("sys/wallpapers/") {
            sync_wallpaper_to_user(name, true);
        }
        true
    }
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

/// Does the user's own copy of a system wallpaper need restoring? Read-only,
/// so the plan phase can ask without changing anything.
fn user_wallpaper_missing(name: &str) -> bool {
    let Some(user) = crate::config::get("name").filter(|n| !n.is_empty()) else { return false };
    !crate::npkfs::exists(&alloc::format!("home/{}/pictures/wallpapers/{}", user, name))
}

/// Mirror `sys/wallpapers/<name>` into the user's wallpapers folder — the only
/// directory `wallpaper list` and `wallpaper set` read.
///
/// `force` says whether an EXISTING user copy may be replaced. A wallpaper that
/// just changed over OTA overwrites (the point of shipping one is that the
/// picture changes); an unchanged one only fills a gap, so a copy the user
/// edited or renamed survives every later `update`.
fn sync_wallpaper_to_user(name: &str, force: bool) {
    use crate::security::capability::CAP_NULL;
    let Some(user) = crate::config::get("name").filter(|n| !n.is_empty()) else { return };
    let target = alloc::format!("home/{}/pictures/wallpapers/{}", user, name);
    if !force && crate::npkfs::exists(&target) {
        return;
    }
    let Ok((bytes, _)) = crate::npkfs::fetch(&alloc::format!("sys/wallpapers/{}", name)) else { return };
    match crate::npkfs::store(&target, &bytes, CAP_NULL) {
        Ok(_) => kprintln!("[npk]   {} ({})", target, if force { "user copy refreshed" } else { "user copy restored" }),
        Err(e) => kprintln!("[npk]   user copy failed: {} — {:?}", target, e),
    }
}

/// One asset's two paths, borrowed — the static table and the dynamic
/// wallpaper case produce the same shape.
struct AssetRef<'a> {
    npkfs_path: &'a str,
    remote_filename: &'a str,
}
