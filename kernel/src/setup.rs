//! First-Boot Setup Wizard
//!
//! Runs on first boot (no .npk-keycheck found).
//! Collects: storage, name, passphrase, timezone, keyboard, language.

use crate::{kprint, kprintln, serial, crypto, config, npkfs, blkdev, capability};

/// Read a line from serial (with echo). Returns trimmed string.
fn read_line() -> alloc::string::String {
    let mut buf = [0u8; 128];
    let len = serial::SERIAL.lock().read_line(&mut buf);
    let s = core::str::from_utf8(&buf[..len]).unwrap_or("").trim();
    alloc::string::String::from(s)
}

/// Read a line, return default if empty.
fn read_line_default(default: &str) -> alloc::string::String {
    let input = read_line();
    if input.is_empty() {
        alloc::string::String::from(default)
    } else {
        input
    }
}

/// Read masked passphrase from serial.
fn read_passphrase() -> (alloc::vec::Vec<u8>, usize) {
    let mut buf = [0u8; 128];
    let len = serial::SERIAL.lock().read_line_masked(&mut buf);
    let mut vec = alloc::vec![0u8; len];
    vec.copy_from_slice(&buf[..len]);
    for b in buf.iter_mut() { *b = 0; }
    (vec, len)
}

/// Run fresh install setup (npkFS already formatted and mounted).
/// Collects identity + settings only, no storage questions.
pub fn run_fresh_install(salt: &[u8; 16]) -> bool {
    kprintln!();
    kprintln!("[npk] ══════════════════════════════════");
    kprintln!("[npk]  Welcome to nopeekOS.");
    kprintln!("[npk]  Choose your identity.");
    kprintln!("[npk] ══════════════════════════════════");
    kprintln!();
    setup_identity_and_settings(salt)
}

#[allow(dead_code)]
/// Run the first-boot setup wizard (legacy, with storage questions).
pub fn run_first_boot(salt: &[u8; 16]) -> bool {
    kprintln!();
    kprintln!("[npk] ══════════════════════════════════");
    kprintln!("[npk]  Welcome to nopeekOS.");
    kprintln!("[npk]  First-time setup.");
    kprintln!("[npk] ══════════════════════════════════");
    kprintln!();

    // === Storage ===
    if blkdev::is_available() {
        if let Some(blocks) = blkdev::block_count() {
            let mb = (blocks * 4096) / (1024 * 1024);
            let gb = mb / 1024;
            let dev = if crate::nvme::is_available() {
                let model = crate::nvme::model_name().unwrap_or_default();
                alloc::format!("NVMe: {}", model)
            } else {
                alloc::string::String::from("virtio-blk")
            };

            kprintln!("[npk] Storage:");
            if gb > 0 {
                kprintln!("[npk]   {} ({} GB, {} blocks)", dev, gb, blocks);
            } else {
                kprintln!("[npk]   {} ({} MB, {} blocks)", dev, mb, blocks);
            }
        }

        // Check if already formatted
        match npkfs::mount() {
            Ok(()) => {
                kprintln!("[npk]   npkFS: already formatted, mounted.");
            }
            Err(_) => {
                kprint!("[npk]   Format as npkFS? [Y/n] ");
                let answer = read_line();
                if answer.is_empty() || answer == "y" || answer == "Y" || answer == "yes" {
                    kprint!("[npk]   Formatting...");
                    match npkfs::mkfs().and_then(|_| npkfs::mount()) {
                        Ok(()) => kprintln!(" done."),
                        Err(e) => {
                            kprintln!(" failed: {}", e);
                            return false;
                        }
                    }
                } else {
                    kprintln!("[npk]   Skipped. No storage available.");
                    return false;
                }
            }
        }
    } else {
        kprintln!("[npk] No block device found. Cannot continue setup.");
        return false;
    }

    kprintln!();
    setup_identity_and_settings(salt)
}

/// Create the npkFS v2 locked default tree. Idempotent.
///
/// `home/<name>/` mirrors loft's sidebar; `sys/` holds system-managed
/// read-mostly content; `.system/` holds boot-time metadata that the
/// kernel reads before `validate_user_name` filters it from listings.
/// Falls back to "florian" if `name` is empty so the tree still has
/// a usable home dir.
fn setup_default_tree(name: &str) -> Result<(), npkfs::fs::Error> {
    let user = if name.is_empty() { "florian" } else { name };

    let dirs: [alloc::string::String; 13] = [
        alloc::string::String::from("sys"),
        alloc::string::String::from("sys/config"),
        alloc::string::String::from("sys/wasm"),
        alloc::string::String::from("sys/fonts"),
        alloc::string::String::from("sys/icons"),
        alloc::string::String::from(".system"),
        alloc::string::String::from("home"),
        alloc::format!("home/{}", user),
        alloc::format!("home/{}/documents", user),
        alloc::format!("home/{}/downloads", user),
        alloc::format!("home/{}/pictures", user),
        alloc::format!("home/{}/pictures/wallpapers", user),
        alloc::format!("home/{}/projects", user),
    ];
    for d in &dirs {
        npkfs::fs::ensure_dir(d)?;
    }
    // Trailing extras under the user dir (kept separate so the array
    // above stays a fixed-length slice, easier to spot when reviewing
    // the canonical layout).
    npkfs::fs::ensure_dir(&alloc::format!("home/{}/music", user))?;
    npkfs::fs::ensure_dir(&alloc::format!("home/{}/videos", user))?;
    npkfs::fs::ensure_dir(&alloc::format!("home/{}/.trash", user))?;
    Ok(())
}

/// Common identity + settings setup (used by both fresh install and legacy first boot)
fn setup_identity_and_settings(salt: &[u8; 16]) -> bool {
    // === Identity ===
    kprintln!("[npk] Identity:");

    // Name — captured into a local but NOT persisted yet. Writing
    // before the master key is set would land a plaintext config blob
    // on disk; later config::set calls overwrite to encrypted but the
    // plaintext blob lingers as an orphan in the v2 B-tree, and `gc`
    // would crash trying to decrypt it during the mark phase.
    kprint!("[npk]   Your name: ");
    let name = read_line();

    // Passphrase
    let master_key = loop {
        kprint!("[npk]   Passphrase: ");
        let (pass1, len1) = read_passphrase();
        if len1 < 8 {
            kprintln!("[npk]   Too short (min 8 chars). Try again.");
            continue;
        }

        kprint!("[npk]   Confirm:    ");
        let (pass2, len2) = read_passphrase();

        if len1 != len2 || pass1 != pass2 {
            kprintln!("[npk]   Passphrases don't match. Try again.");
            continue;
        }

        let key = crypto::derive_master_key(&pass1, salt);
        drop(pass1);
        drop(pass2);
        break key;
    };

    crypto::set_master_key(master_key);

    // From here on every FS write goes through the AEAD path, so it's
    // safe to persist the name.
    if !name.is_empty() {
        config::set("name", &name);
    }

    // Store keycheck (encrypted with the new master key, lives at
    // .system/keycheck per the v2 locked tree).
    match npkfs::store(config::KEYCHECK_PATH, config::KEYCHECK_VALUE, capability::CAP_NULL) {
        Ok(_) => {}
        Err(e) => kprintln!("[npk]   WARNING: Could not store keycheck: {}", e),
    }

    // Lay down the locked default tree once. Idempotent — re-runs are
    // a no-op. Apps assume these dirs exist; the installer is the only
    // thing that creates them. See NPKFS_V2.md for the spec.
    if let Err(e) = setup_default_tree(&name) {
        kprintln!("[npk]   WARNING: Could not lay down default tree: {:?}", e);
    }

    kprintln!();

    // === Settings ===
    kprintln!("[npk] Settings (Enter = default):");

    kprint!("[npk]   Timezone  [+1]: ");
    let tz = read_line_default("+1");
    config::set("timezone", &tz);

    kprint!("[npk]   Keyboard  [de_CH]: ");
    let kb = read_line_default("de_CH");
    config::set("keyboard", &kb);

    kprint!("[npk]   Language  [de]: ");
    let lang = read_line_default("de");
    config::set("lang", &lang);

    kprintln!();
    if !name.is_empty() {
        kprintln!("[npk] ══════════════════════════════════");
        kprintln!("[npk]  Welcome, {}.", name);
        kprintln!("[npk]  Setup complete. System is yours.", );
        kprintln!("[npk] ══════════════════════════════════");
    } else {
        kprintln!("[npk] ══════════════════════════════════");
        kprintln!("[npk]  Setup complete. System is yours.");
        kprintln!("[npk] ══════════════════════════════════");
    }

    true
}
