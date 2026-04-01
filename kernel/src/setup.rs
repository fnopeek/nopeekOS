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

/// Common identity + settings setup (used by both fresh install and legacy first boot)
fn setup_identity_and_settings(salt: &[u8; 16]) -> bool {
    // === Identity ===
    kprintln!("[npk] Identity:");

    // Name
    kprint!("[npk]   Your name: ");
    let name = read_line();
    if !name.is_empty() {
        config::set("name", &name);
    }

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

    // Store keycheck
    match npkfs::store(".npk-keycheck", b"nopeekOS.keycheck.v1.valid", capability::CAP_NULL) {
        Ok(_) => {}
        Err(e) => kprintln!("[npk]   WARNING: Could not store keycheck: {}", e),
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
