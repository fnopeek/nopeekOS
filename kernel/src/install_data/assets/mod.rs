//! Bundled asset list — embedded only in the installer kernel.
//!
//! build.sh stages every asset (font, WASM modules + their .version
//! files) from `release/` into `kernel/src/install_data/assets/`
//! before Pass 2 of the installer build. Pass 2 compiles with
//! `--features installer`, which activates the `#[cfg]`-gated
//! `include_bytes!` / `include_str!` calls below.
//!
//! Normal kernel builds (without the installer feature) omit this
//! module entirely — zero bytes bundled, zero size cost.
//!
//! Runtime flow (`install::install_to_nvme`):
//!   1. Partition NVMe, format ESP, write GRUB + kernel.bin.
//!   2. npkfs::mkfs + mount.
//!   3. Iterate BUNDLED_ASSETS, write each entry to npkFS:
//!      - always store `<fs_path>` with the bytes
//!      - if version Some, also store `<fs_path>.version` — so
//!        `intent::install` and `intent::update::update_all_modules`
//!        see the module as up-to-date and skip spurious redownloads.
//!   4. First boot from NVMe: FS is seeded, font + modules are ready.
//!
//! Sig verification is skipped for bundled assets: the installer
//! kernel itself is ECDSA P-384 signed (OTA trust chain), and the
//! bytes are inside its binary. An attacker who controls the
//! installer kernel already controls its embedded data. OTA-path
//! (`intent::install`) still verifies signatures for runtime updates.

#![cfg(feature = "installer")]

/// One bundled asset — filesystem path it lands under in npkFS, plus
/// the embedded bytes. For WASM modules, `version` carries the string
/// that will also be stored at `<fs_path>.version`.
pub struct BundledAsset {
    pub fs_path: &'static str,
    pub bytes:   &'static [u8],
    /// Version string (e.g. "0.2.0\n"), or None for non-versioned
    /// assets like fonts. If Some, we also write `<fs_path>.version`
    /// into npkFS so update/install checks see the right version.
    pub version: Option<&'static str>,
}

/// Assets shipped with the installer. Extend this list when new
/// system fonts or first-party WASM modules become part of the
/// default install.
///
/// Paths follow the npkFS convention `sys/<category>/<name>`.
pub static BUNDLED_ASSETS: &[BundledAsset] = &[
    // ── System UI font ────────────────────────────────────────────
    BundledAsset {
        fs_path: "sys/fonts/inter-variable",
        bytes:   include_bytes!("inter-variable.ttf"),
        version: None,
    },

    // ── First-party WASM modules ──────────────────────────────────
    // Keep in sync with release/modules/ output of build.sh release.
    BundledAsset {
        fs_path: "sys/wasm/top",
        bytes:   include_bytes!("top.wasm"),
        version: Some(include_str!("top.version")),
    },
    BundledAsset {
        fs_path: "sys/wasm/debug",
        bytes:   include_bytes!("debug.wasm"),
        version: Some(include_str!("debug.version")),
    },
    BundledAsset {
        fs_path: "sys/wasm/wallpaper",
        bytes:   include_bytes!("wallpaper.wasm"),
        version: Some(include_str!("wallpaper.version")),
    },
    BundledAsset {
        fs_path: "sys/wasm/wifi",
        bytes:   include_bytes!("wifi.wasm"),
        version: Some(include_str!("wifi.version")),
    },
];

/// Stub invoked by install.rs to avoid conditional compilation at the
/// call site. Writes every bundled asset into npkFS. Must only run
/// after `npkfs::mount` has succeeded **and** the master key is set —
/// otherwise data lands as plaintext (see feedback_encrypt_at_rest_order).
pub fn bootstrap_into_npkfs() {
    use crate::kprintln;
    use crate::security::capability::CAP_NULL;

    crate::kprintln!("[npk] Seeding npkFS with {} bundled asset(s)...", BUNDLED_ASSETS.len());
    let mut total_bytes: usize = 0;
    for a in BUNDLED_ASSETS {
        match crate::npkfs::store(a.fs_path, a.bytes, CAP_NULL) {
            Ok(_) => {
                total_bytes += a.bytes.len();
                kprintln!("[npk]   {} ({} bytes)", a.fs_path, a.bytes.len());
            }
            Err(e) => {
                kprintln!("[npk]   FAILED: {} — {:?}", a.fs_path, e);
                continue;
            }
        }

        // Versioned asset → also write the `.version` sidecar so
        // `intent::install` / `intent::update::update_all_modules`
        // recognise it as installed at the right version.
        if let Some(version) = a.version {
            let ver_path = alloc::format!("{}.version", a.fs_path);
            if let Err(e) = crate::npkfs::store(&ver_path, version.as_bytes(), CAP_NULL) {
                kprintln!("[npk]   FAILED (version): {} — {:?}", ver_path, e);
            }
        }
    }
    kprintln!("[npk] Seeded {} bytes total.", total_bytes);
}
