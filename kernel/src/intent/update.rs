//! OTA update intent.
//!
//! Downloads kernel from GitHub, verifies ECDSA P-384 signature,
//! writes to ESP FAT32 partition.
//!
//! Trust comes from the three release manifests (kernel, modules, assets),
//! each signed as a whole (`fetch_manifest`). A manifest names every
//! artifact with its version, size, sha384 and URL, so a downloaded blob is
//! accepted when its hash matches its signed entry. Each manifest carries
//! `issued=` (release time); an older one than the last accepted is refused,
//! so a replayed old release cannot downgrade. Withdrawing a release stays
//! possible: a new manifest may name an older version.

use crate::kprintln;
use alloc::string::String;
use alloc::vec::Vec;

const UPDATE_HOST: &str = "raw.githubusercontent.com";
const UPDATE_BASE: &str = "/fnopeek/nopeekOS/main/release";

/// Where a signed artifact lives under its own hash. A new version is a new
/// name, so a CDN cache can never pair a fresh manifest with a stale file.
pub(super) fn blob_path(sha384: &[u8; 48]) -> String {
    let mut p = alloc::format!("{}/blobs/", UPDATE_BASE);
    for b in sha384 {
        let _ = core::fmt::Write::write_fmt(&mut p, format_args!("{:02x}", b));
    }
    p
}
/// Hard ceiling on a kernel image we are willing to buffer. Not the download
/// bound: that comes from the signed manifest, so this only has to be
/// implausibly large. A fixed cap close to the real size would eventually
/// truncate a grown kernel and leave the updater unable to install any fix.
const MAX_KERNEL_SIZE: usize = 64 * 1024 * 1024;
const MAX_MANIFEST_SIZE: usize = 4096;
const MAX_ASSET_MANIFEST_SIZE: usize = 16 * 1024;
/// 1 GiB ceiling for OTA assets. The download streams into npkFS one chunk
/// at a time, so the cap bounds disk use, not RAM. raw-githubusercontent
/// caps at ~50–100 MB per file, so large assets ship via GitHub Releases
/// (asset manifest carries an explicit `url=` line for those;
/// redirect-following lives in `https_get`).
const MAX_ASSET_SIZE: usize = 1024 * 1024 * 1024;
const MAX_SIG_SIZE: usize = 512;

/// Mapping from asset-manifest section header to npkFS path. Keep in sync with `build.sh` ASSET_MANIFEST writer
/// and `kernel/src/install_data/assets/mod.rs` BUNDLED entries.
struct AssetSpec {
    section: &'static str,
    npkfs_path: &'static str,
}

const ASSETS: &[AssetSpec] = &[
    AssetSpec { section: "font:inter-variable", npkfs_path: "sys/fonts/inter-variable" },
    AssetSpec { section: "font:ibm-plex-mono",  npkfs_path: "sys/fonts/ibm-plex-mono" },
    // Both faces are SIL OFL 1.1: the licence must accompany every copy,
    // so an OTA-updated system pulls it alongside the font rather than
    // only fresh installs getting it from the bundled assets.
    AssetSpec { section: "font:LICENSE-Inter",   npkfs_path: "sys/fonts/LICENSE-Inter" },
    AssetSpec { section: "font:LICENSE-IBM-Plex", npkfs_path: "sys/fonts/LICENSE-IBM-Plex" },
    AssetSpec { section: "icons:phosphor",      npkfs_path: "sys/icons/phosphor" },
    AssetSpec { section: "microvm:initramfs",   npkfs_path: "sys/microvm/initramfs.cpio.gz" },
    AssetSpec { section: "microvm:linux-virt",  npkfs_path: "sys/microvm/linux-virt.bzImage" },
    // Optional userspace bundle — Alpine minirootfs + busybox + (future)
    // Wayland/Mesa/LibreWolf. Built by `microvm-userspace/build.sh`.
    // Distinct from `microvm:initramfs` (which is just our PID-1, always
    // present). If a release ships this asset, OTA pulls it; if not, the
    // entry is absent in the asset manifest and we keep whatever's
    // already installed (or nothing).
    //
    // Small bundles live in `release/assets/` on the `main` branch and
    // ship via raw.githubusercontent.com. Larger bundles live on GitHub
    // Releases; the asset
    // manifest carries a `url=` override per entry and `https_get`
    // follows the 302 redirect chain to objects.githubusercontent.com.
    AssetSpec { section: "microvm:userspace",   npkfs_path: "sys/microvm/userspace.cpio.gz" },
    // Squashfs form of the userspace bundle — read-only, mounted by
    // PID-1 from /dev/vdb (slot-5 virtio-blk) instead of unpacked into
    // a tmpfs initramfs. The RAM-efficient path; supersedes the cpio entry
    // above once it is the only shipped form.
    AssetSpec { section: "microvm:userspace-sqfs", npkfs_path: "sys/microvm/userspace.sqfs" },
    // CPython's standard library, as one zip. Stored uncompressed on
    // purpose: this interpreter has no zlib, so a deflated zip raises
    // ZipImportError at the first import. It also saves decompressing on
    // every import.
    //
    // Not bundled into the installer: like microvm:userspace, Python is
    // something you fetch, not something every USB stick carries.
    AssetSpec { section: "python:stdlib",       npkfs_path: "sys/python/lib/python313.zip" },
];

struct AssetEntry {
    section: String,
    size: usize,
    sha384: [u8; 48],
    /// Optional explicit URL — when present, fetched verbatim instead of
    /// the blob named by `sha384`.
    url: Option<String>,
}

struct Manifest {
    version: String,
    size: usize,
    sha384: [u8; 48],
    /// npkFS version the kernel reads (`disk_format=`). Manifests from
    /// before the field existed were v3.
    disk_format: u32,
}

/// Which release manifest; part of the signed message, so one kind's
/// manifest cannot stand in for another's.
#[derive(Clone, Copy)]
pub(super) enum ManifestKind {
    Kernel,
    Modules,
    Assets,
}

impl ManifestKind {
    fn tag(self) -> &'static str {
        match self {
            ManifestKind::Kernel => "kernel",
            ManifestKind::Modules => "modules",
            ManifestKind::Assets => "assets",
        }
    }
}

/// Prefix of every signed manifest message (`build.sh` `sign_manifest`).
const MANIFEST_SIG_TAG: &str = "nopeekOS-ota-manifest-v1";

/// Fetch a release manifest and its detached signature, verify both and
/// the `issued=` order. Returns the manifest bytes, now authenticated.
pub(super) fn fetch_manifest(host: &str, path: &str, max: usize, kind: ManifestKind)
    -> Result<Vec<u8>, String>
{
    let data = super::http::https_get(host, path, max).map_err(String::from)?;
    let sig_path = alloc::format!("{}.sig", path);
    let sig = super::http::https_get(host, &sig_path, MAX_SIG_SIZE)
        .map_err(|e| alloc::format!("signature: {}", e))?;

    let mut h = crate::tls::sha256::Sha384::new();
    h.update(MANIFEST_SIG_TAG.as_bytes());
    h.update(b"\n");
    h.update(kind.tag().as_bytes());
    h.update(b"\n");
    h.update(&data);
    let digest = h.finalize();
    if !crate::tls::certstore::verify_p384_prehash_384(&crate::update_key::UPDATE_PUB_KEY, &digest, &sig) {
        return Err(String::from("signature invalid — rejected"));
    }

    let issued = manifest_issued(&data).ok_or_else(|| String::from("no issued= line — rejected"))?;
    let key = alloc::format!("sys/ota/issued-{}", kind.tag());
    let seen = crate::npkfs::fetch(&key).ok()
        .and_then(|(d, _)| core::str::from_utf8(&d).ok().and_then(|t| t.trim().parse::<u64>().ok()))
        .unwrap_or(0);
    if issued < seen {
        return Err(alloc::format!(
            "older than the release already installed (issued {} < {}) — rejected", issued, seen));
    }
    if issued > seen {
        let _ = crate::npkfs::upsert(&key, alloc::format!("{}", issued).as_bytes(),
            crate::capability::CAP_NULL);
    }
    Ok(data)
}

/// The `issued=` value, from the lines before the first `[section]`.
fn manifest_issued(data: &[u8]) -> Option<u64> {
    let text = core::str::from_utf8(data).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') { break; }
        if let Some(v) = line.strip_prefix("issued=") {
            return v.trim().parse().ok();
        }
    }
    None
}

fn parse_manifest(data: &[u8]) -> Result<Manifest, &'static str> {
    let text = core::str::from_utf8(data).map_err(|_| "manifest: invalid UTF-8")?;
    let mut version = None;
    let mut size = None;
    let mut sha384 = None;
    let mut disk_format = 3;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Some((key, val)) = line.split_once('=') {
            match key.trim() {
                "disk_format" => disk_format = val.trim().parse().map_err(|_| "manifest: bad disk_format")?,
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
        disk_format,
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
/// Parenthesised on purpose: the terminal draws `(…)` on a status line in the
/// faint colour, so sizes step back behind names and versions.
pub(super) fn fmt_size(bytes: usize) -> String {
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
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.kernel.is_none()
            && self.modules.is_empty()
            && self.assets.is_empty()
    }

    fn count(&self) -> usize {
        self.kernel.iter().count() + self.modules.len() + self.assets.len()
    }

    /// One summary line for everything that needs nothing (kernel, module
    /// and asset counts).
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

    // The per-request connect timings and status lines say nothing about
    // what changed, so they are hidden unless `-v` is given.
    let _quiet = (!verbose).then(super::http::quiet);

    let Some(plan) = build_plan() else { return };

    if plan.is_empty() {
        // Nothing to do is one line, not one line per module and asset.
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
    let manifest_data = match fetch_manifest(UPDATE_HOST, &manifest_path, MAX_MANIFEST_SIZE, ManifestKind::Kernel) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk]   ! manifest: {}", e); return None; }
    };
    let manifest = match parse_manifest(&manifest_data) {
        Ok(m) => m,
        Err(e) => { kprintln!("[npk]   ! {}", e); return None; }
    };

    let current = env!("CARGO_PKG_VERSION");
    // The manifest is signed, but a ceiling still guards the buffer.
    let kernel = if manifest.version == current {
        None
    } else if manifest.disk_format != crate::npkfs::DISK_VERSION {
        // A kernel for another disk format would halt at the next boot on
        // this disk, newer or older alike. That change is a reinstall.
        kprintln!("[npk]   ! kernel v{} reads npkFS v{}, this disk is v{} — not installed (reinstall from USB)",
            manifest.version, manifest.disk_format, crate::npkfs::DISK_VERSION);
        None
    } else if manifest.size == 0 || manifest.size > MAX_KERNEL_SIZE {
        kprintln!("[npk]   ! implausible kernel size {} (max {})", manifest.size, MAX_KERNEL_SIZE);
        None
    } else {
        Some(manifest)
    };

    let (modules, modules_current) = super::install::plan_modules();
    let (assets, assets_current) = plan_assets();

    Some(Plan { kernel, modules, modules_current, assets, assets_current })
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

    // One line for everything that needs nothing, so the few items that
    // matter are not buried.
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
    let mut certs_changed = false;
    for a in &plan.assets {
        if apply_asset(a) {
            assets += 1;
            certs_changed |= a.entry.section.starts_with("cert:");
        }
    }
    // New anchors are inert until reloaded — the store is held in memory so
    // handshakes never touch npkFS. Without this, a freshly delivered CA
    // would only take effect after the next reboot.
    if certs_changed {
        let n = crate::tls::certstore::load_store();
        kprintln!("[npk]   * trust store reloaded — {} stored anchor(s)", n);
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
    kprintln!("[npk]   + kernel   v{} {}", manifest.version, fmt_size(manifest.size));
    let kernel_path = blob_path(&manifest.sha384);
    let kernel_data = match super::http::https_get_resumable(UPDATE_HOST, &kernel_path, manifest.size) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk]   ! kernel     download: {}", e); return false; }
    };
    if kernel_data.len() != manifest.size {
        kprintln!("[npk]   ! kernel     short download ({} of {})", kernel_data.len(), manifest.size);
        return false;
    }

    // The signed manifest names this hash; nothing else is needed.
    let hash = crate::tls::sha256::sha384(&kernel_data);
    if hash != manifest.sha384 {
        kprintln!("[npk]   ! kernel     checksum mismatch — rejected");
        return false;
    }

    let esp_start = match crate::gpt::detect_esp_offset() {
        Some(s) => s,
        None => {
            kprintln!("[npk]   ! kernel     no ESP partition found — is this a GPT disk?");
            return false;
        }
    };
    match crate::fat32::update_kernel(esp_start, &kernel_data) {
        Ok(()) => true,
        Err(e) => { kprintln!("[npk]   ! kernel     ESP write: {}", e); false }
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
    /// Whether a copy already exists locally (only differs in wording).
    present: bool,
}

/// Diff release/assets/manifest against npkFS-resident assets. Reads only:
/// returns the jobs to run and how many were already current.
fn plan_assets() -> (Vec<AssetJob>, usize) {
    let manifest_path = alloc::format!("{}/assets/manifest", UPDATE_BASE);
    let manifest_data = match fetch_manifest(UPDATE_HOST, &manifest_path, MAX_ASSET_MANIFEST_SIZE, ManifestKind::Assets) {
        Ok(d) => d,
        Err(e) => { kprintln!("[npk]   ! asset manifest: {}", e); return (Vec::new(), 0); }
    };

    let entries = match parse_asset_manifest(&manifest_data) {
        Ok(e) => e,
        Err(e) => { kprintln!("[npk]   ! asset manifest: {}", e); return (Vec::new(), 0); }
    };

    let mut jobs = Vec::new();
    let mut current = 0usize;

    for entry in entries {
        let npkfs_path = match ASSETS.iter().find(|s| s.section == entry.section) {
            Some(s) => String::from(s.npkfs_path),
            // Root CA anchors are data, not code: the section name is the
            // filename, so shipping or replacing an anchor is dropping a file
            // into `release/assets/certs/` — no kernel change, no reinstall.
            // The trust chain is unchanged: size and sha384 come from the
            // signed manifest, exactly like the kernel.
            None => match entry.section.strip_prefix("cert:").filter(|n| safe_asset_name(n)) {
                Some(name) => alloc::format!("{}/{}", crate::tls::certstore::STORE_DIR, name),
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
            continue;
        }

        let present = local_hash.is_some();
        jobs.push(AssetJob { entry, npkfs_path, present });
    }

    (jobs, current)
}

/// Download, verify and store one planned asset. Prints its own result line;
/// returns whether the asset was written.
fn apply_asset(job: &AssetJob) -> bool {
    let entry = &job.entry;
    let spec = AssetRef { npkfs_path: &job.npkfs_path };
    let local_present = job.present;
    {
        // ── Make room before a streaming write ──────────────────────
        // The streaming writer keeps the old copy live until finish(),
        // so a refresh transiently needs the new asset's size on top of
        // everything already stored (2x for a same-path replace), and an
        // aborted download leaves orphaned chunks only gc reclaims. Reclaim
        // and free up front; fail with a clear message if it still won't fit.
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
        // 3. Truly out of space: fail clearly up front, not mid-download.
        if free_bytes() < need {
            kprintln!("[npk]   ! {} disk full (need {} MB, {} MB free)",
                spec.npkfs_path, need / (1024 * 1024), free_bytes() / (1024 * 1024));
            return false;
        }

        kprintln!("[npk]   + asset    {:<28} {}", spec.npkfs_path, fmt_size(entry.size));

        // Two URL paths:
        //   (a) entry.url == Some(url)  → fetch verbatim (GitHub Releases).
        //       `https_get_streaming_resumable` follows 302 redirects so
        //       `github.com/.../releases/download/...` → the signed
        //       `objects.githubusercontent.com` CDN URL works transparently.
        //   (b) entry.url == None       → fall back to raw.githubusercontent
        //       on main (small assets).
        let (asset_host, asset_path_owned);
        let (asset_host_str, asset_path_str): (&str, &str) = if let Some(url) = &entry.url {
            match split_url(url) {
                Some((h, p)) => (h, p),
                None => { kprintln!("[npk]   ! asset     bad url"); return false; }
            }
        } else {
            asset_host = String::from(UPDATE_HOST);
            asset_path_owned = blob_path(&entry.sha384);
            (asset_host.as_str(), asset_path_owned.as_str())
        };

        // Streaming download: drive bytes straight into npkFS via the
        // ChunkedWriter, hashing SHA-384 incrementally as they pass.
        // Peak RAM is one 16 MiB chunk regardless of asset size.
        let mut writer = match crate::npkfs::open_streaming_write(spec.npkfs_path) {
            Ok(w) => w,
            Err(e) => { kprintln!("[npk]   ! asset     npkfs open failed: {:?}", e); return false; }
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
        let stream_result = super::http::https_get_streaming_resumable(
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
                kprintln!("[npk]   ! asset     download: {}{}",
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
            kprintln!("[npk]   ! asset     size mismatch (got {} expected {})", total_bytes, entry.size);
            return false;
        }

        // The signed manifest names this hash.
        let hash = hasher.finalize();
        if hash != entry.sha384 {
            kprintln!("[npk]   ! asset     checksum failed");
            // Drop the writer without finishing — flushed chunks
            // remain in storage but are unreachable from the path
            // tree, so the next `gc()` cycle reclaims them.
            return false;
        }

        // Commit: writer.finish() publishes the chunked file
        // atomically. Replaces any existing entry at the same path.
        match writer.finish() {
            Ok(_) => {}
            Err(e) => { kprintln!("[npk]   ! asset     publish failed: {:?}", e); return false; }
        }

        true
    }
}

/// A manifest-supplied name we are willing to turn into a path.
/// Deliberately strict — the name becomes part of an npkFS path, so anything
/// that could climb out of its directory is refused. The manifest is signed,
/// but a name is not a place to be trusting.
pub(super) fn safe_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'_')
}

/// One asset's two paths, borrowed — the static table and the dynamic
/// cert case produce the same shape.
struct AssetRef<'a> {
    npkfs_path: &'a str,
}
