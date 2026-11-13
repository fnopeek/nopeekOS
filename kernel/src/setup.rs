//! First-Boot Setup Wizard
//!
//! Runs on first boot (no keyslot in the superblock).
//! Collects: storage, name, passphrase, timezone, keyboard, language.

use crate::{kprint, kprintln, serial, config, disk_key, npkfs};
#[cfg(feature = "installer")]
use crate::{install, nvme};

/// Read a line from serial (with echo). Returns trimmed string.
fn read_line() -> alloc::string::String {
    let mut buf = [0u8; 128];
    let len = serial::read_line(&mut buf);
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
    let len = serial::read_line_masked(&mut buf);
    let mut vec = alloc::vec![0u8; len];
    vec.copy_from_slice(&buf[..len]);
    for b in buf.iter_mut() { *b = 0; }
    (vec, len)
}

/// Run fresh install setup (npkFS already formatted and mounted).
/// Collects identity + settings only, no storage questions.
pub fn run_fresh_install() -> bool {
    kprintln!();
    kprintln!("[npk] ══════════════════════════════════");
    kprintln!("[npk]  Welcome to nopeekOS.");
    kprintln!("[npk]  Choose your identity.");
    kprintln!("[npk] ══════════════════════════════════");
    kprintln!();
    setup_identity_and_settings()
}

/// Create the npkFS v3 locked default tree. Idempotent.
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
fn setup_identity_and_settings() -> bool {
    // === Identity ===
    kprintln!("[npk] Identity:");

    // Name — captured into a local but NOT persisted yet: nothing can be
    // written before the disk key exists.
    kprint!("[npk]   Your name: ");
    let name = read_line();

    // Passphrase
    loop {
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

        let r = crate::disk_key::create(&pass1[..len1]);
        let (mut pass1, mut pass2) = (pass1, pass2);
        pass1.fill(0);
        pass2.fill(0);
        match r {
            Ok(()) => break,
            Err(e) => {
                kprintln!("[npk]   Could not create the disk key: {}", e);
                return false;
            }
        }
    }

    // From here on every FS write goes through the AEAD path, so it's
    // safe to persist the name.
    if !name.is_empty() {
        config::set("name", &name);
    }

    // Lay down the locked default tree once. Idempotent — re-runs are
    // a no-op. Apps assume these dirs exist; the installer is the only
    // thing that creates them.
    if let Err(e) = setup_default_tree(&name) {
        kprintln!("[npk]   WARNING: Could not lay down default tree: {:?}", e);
    }

    kprintln!();

    // === Settings ===
    kprintln!("[npk] Settings (Enter = default):");

    kprint!("[npk]   Timezone  [Europe/Zurich]: ");
    let mut tz = read_line_default("Europe/Zurich");
    if crate::time::zone::parse(&tz).is_none() {
        kprintln!("[npk]   unknown zone '{}', using Europe/Zurich", tz);
        tz = alloc::string::String::from("Europe/Zurich");
    }
    config::set("timezone", &tz);

    kprint!("[npk]   Keyboard  [de_CH]: ");
    let kb = read_line_default("de_CH");
    config::set("keyboard", &kb);

    kprint!("[npk]   Language  [en]: ");
    let lang = read_line_default("en");
    config::set("lang", &lang);

    // Default autostart: deck, the bottom panel (launcher, windows, status
    // items, clock), then the drivers every x86 machine needs: acpi (firmware AML:
    // battery, power button, idle states) and audio_hda. Both exit quietly
    // where their hardware is missing. A config seed; `set autostart` edits it.
    #[cfg(target_arch = "x86_64")]
    config::set("autostart", "deck acpi audio_hda");
    // The Pi 5: deck for the desktop to come, and its display.
    #[cfg(target_arch = "aarch64")]
    config::set("autostart", "deck gpu_bcm2712");

    // Theme follows the wallpaper: `auto` picks light/dark from the
    // background luminance (palette::is_light_theme). `theme light|dark`
    // overrides.
    config::set("theme", "auto");

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

/// Unlock the disk with the passphrase typed at the console: the login
/// without a graphical screen.
pub fn text_unlock() {
    kprintln!();
    kprintln!("[npk] ─────────────────────────────────");
    kprintln!("[npk]  Identity required.");
    kprintln!("[npk]  Your passphrase IS your identity.");
    #[cfg(feature = "installer")]
    kprintln!("[npk]  Type RESET to wipe and reinstall.");
    kprintln!("[npk] ─────────────────────────────────");
    kprintln!();

    let mut attempts: u32 = 0;
    loop {
        if attempts > 0 {
            let delay_secs = 1u64 << attempts.min(5);
            kprintln!("[npk] Wait {} seconds...", delay_secs);
            let start = crate::time::ticks();
            let delay_ticks = delay_secs * 100;
            while crate::time::ticks() - start < delay_ticks {
                core::hint::spin_loop();
            }
        }

        kprint!("[npk] Passphrase: ");
        let mut buf = [0u8; 128];
        let len = serial::read_line_masked(&mut buf);
        if len == 0 {
            kprintln!("[npk] Passphrase cannot be empty.");
            continue;
        }

        #[cfg(feature = "installer")]
        if len == 5 && &buf[..5] == b"RESET" {
            kprintln!();
            kprintln!("[npk] !! FACTORY RESET !!");
            kprintln!("[npk] This will ERASE ALL DATA.");
            kprint!("[npk] Type YES to confirm: ");
            let mut confirm = [0u8; 16];
            let clen = serial::read_line(&mut confirm);
            if clen == 3 && &confirm[..3] == b"YES" {
                for b in buf.iter_mut() { *b = 0; }
                if install::has_installer() && nvme::is_available() {
                    match install::install_to_nvme() {
                        Ok(()) => {}
                        Err(e) => {
                            kprintln!("[npk] Installation failed: {}", e);
                            kprintln!("[npk] System halted.");
                            crate::arch::halt_forever()
                        }
                    }
                } else {
                    kprintln!("[npk] Formatting...");
                    let _ = npkfs::mkfs();
                    let _ = npkfs::mount();
                }
                kprintln!();
                if !run_fresh_install() {
                    kprintln!("[npk] Setup failed. System halted.");
                    crate::arch::halt_forever()
                }
                return;
            } else {
                kprintln!("[npk] Reset cancelled.");
                continue;
            }
        }

        let ok = disk_key::unlock(&buf[..len]);
        for b in buf.iter_mut() { *b = 0; }

        if ok {
            config::load();
            serial::set_verbose(config::bootlog_verbose());
            if let Some(name) = config::get("name") {
                kprintln!("[npk] Welcome back, {}.", name);
            } else {
                kprintln!("[npk] Identity verified.");
            }
            return;
        }
        kprintln!("[npk] Wrong passphrase.");
        attempts += 1;

        if attempts >= 10 {
            kprintln!("[npk] Too many failed attempts. System halted.");
            crate::arch::halt_forever()
        }
    }
}
