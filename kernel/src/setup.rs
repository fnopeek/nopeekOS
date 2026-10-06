//! First-Boot Setup Wizard
//!
//! Runs on first boot (no keyslot in the superblock).
//! Collects: storage, name, passphrase, timezone, keyboard, language.

use crate::{kprint, kprintln, serial, config, npkfs};

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
    // thing that creates them. See docs/archive/NPKFS_V2.md for the spec.
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

    kprint!("[npk]   Language  [en]: ");
    let lang = read_line_default("en");
    config::set("lang", &lang);

    // Default autostart: the app dock + the top bar. npk_sleep runs pending
    // scheduler work while it waits, so several looping panels coexist
    // without starving intents. A config seed; `set autostart ...`
    // overrides it.
    config::set("autostart", "dock bar");

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
