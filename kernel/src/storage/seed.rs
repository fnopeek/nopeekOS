//! The standard installation a board's boot partition carries, written
//! into npkFS on the first boot: what the USB installer bundles on x86,
//! for a board set up without it (the Pi's SD card, `build.sh pi5-img`).

use crate::{fat32, kprintln, npkfs};

/// The standard installation from the boot partition:
/// `SEED.BIN` in the boot partition's root, `NPKSEED1` then entries of
/// `path_len: u16, path, len: u32, bytes` (little endian). A path that
/// starts `~/` is in the user's home. Written into npkFS on the first boot,
/// after `setup::run_fresh_install` created the disk key, as the
/// installer's bundled assets are; what exists already is kept.
pub fn seed_from_esp() {
    const MAGIC: &[u8; 8] = b"NPKSEED1";
    let Some(esp) = fat32::esp_start() else {
        kprintln!("[npk] SEED.BIN: no boot partition found on the SD card");
        return;
    };
    let seed = match fat32::read_root_file(esp, "SEED.BIN") {
        Ok(Some(s)) => s,
        Ok(None) => { kprintln!("[npk] SEED.BIN: not on the boot partition"); return; }
        Err(e) => { kprintln!("[npk] SEED.BIN: {}", e); return; }
    };
    if seed.len() < MAGIC.len() || &seed[..MAGIC.len()] != MAGIC {
        kprintln!("[npk] SEED.BIN: not a seed, ignored");
        return;
    }
    let home = crate::config::get("name").filter(|n| !n.is_empty())
        .map(|n| alloc::format!("home/{}", n));
    kprintln!("[npk] Seeding npkFS from the SD card ({} MB)...", seed.len() / (1024 * 1024));

    let mut at = MAGIC.len();
    let (mut stored, mut bytes) = (0usize, 0usize);
    while at < seed.len() {
        let Some(path_len) = seed.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize) else { break };
        at += 2;
        let Some(path) = seed.get(at..at + path_len).and_then(|p| core::str::from_utf8(p).ok()) else { break };
        at += path_len;
        let Some(len) = seed.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) else { break };
        at += 4;
        let Some(data) = seed.get(at..at + len) else { break };
        at += len;

        let target = match (path.strip_prefix("~/"), &home) {
            (Some(rest), Some(home)) => alloc::format!("{}/{}", home, rest),
            (Some(_), None) => continue,
            (None, _) => alloc::string::String::from(path),
        };
        if npkfs::exists(&target) { continue; }
        if let Some((dir, _)) = target.rsplit_once('/') {
            let _ = npkfs::fs::ensure_dirs(dir);
        }
        match store(&target, data) {
            Ok(()) => { stored += 1; bytes += len; }
            Err(e) => kprintln!("[npk]   FAILED: {} ({:?})", target, e),
        }
    }
    kprintln!("[npk] Seeded {} files ({} KB)", stored, bytes / 1024);

    // The first boot shows a chosen picture, as the installer's does.
    if let Some(home) = home {
        let first = alloc::format!("{}/pictures/wallpapers/{}", home, INITIAL_WALLPAPER);
        if crate::config::get("wallpaper").is_none() && npkfs::exists(&first) {
            crate::config::set("wallpaper", &first);
        }
    }
}

/// The wallpaper of a fresh install (`install_data/assets`): the calmest
/// of the set, glass reads best over it in both themes.
const INITIAL_WALLPAPER: &str = "ice-cosmic-waves.png";

/// Store one file as the installer's `store_asset` does: anything larger
/// than a chunk through the streaming writer, as an OTA download.
fn store(path: &str, bytes: &[u8]) -> Result<(), npkfs::FsError> {
    use crate::security::capability::CAP_NULL;
    if bytes.len() <= npkfs::STREAMING_CHUNK_SIZE {
        return npkfs::store(path, bytes, CAP_NULL).map(|_| ());
    }
    let mut w = npkfs::open_streaming_write(path)?;
    w.write(bytes).map_err(|_| npkfs::FsError::Corrupt)?;
    w.finish().map(|_| ()).map_err(|_| npkfs::FsError::Corrupt)
}
