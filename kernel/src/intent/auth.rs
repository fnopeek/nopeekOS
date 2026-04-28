//! Authentication intents: lock, passwd

use crate::{kprint, kprintln, crypto, serial};

pub fn intent_lock() {
    kprintln!("[npk] System locked.");
    crypto::clear_master_key();

    let salt = crate::npkfs::install_salt().unwrap_or([0u8; 16]);

    // Use GUI login screen if framebuffer available
    if crate::framebuffer::is_available() {
        let _key = crate::gui::login::run(&salt);
    } else {
        // Fallback: text-mode unlock
        let mut attempts: u32 = 0;
        loop {
            if attempts > 0 {
                let delay_secs = 1u64 << attempts.min(5);
                kprintln!("[npk] Wait {} seconds...", delay_secs);
                let start = crate::interrupts::ticks();
                let delay_ticks = delay_secs * 100;
                while crate::interrupts::ticks() - start < delay_ticks {
                    core::hint::spin_loop();
                }
            }

            kprint!("[npk] Passphrase: ");
            let mut buf = [0u8; 128];
            let len = { serial::SERIAL.lock().read_line_masked(&mut buf) };
            if len == 0 { continue; }

            let key = crypto::derive_master_key(&buf[..len], &salt);
            for b in buf.iter_mut() { *b = 0; }

            crypto::set_master_key(key);

            match crate::npkfs::fetch(crate::config::KEYCHECK_PATH) {
                Ok((data, _)) if &data[..] == crate::config::KEYCHECK_VALUE => {
                    crate::config::load();
                    if let Some(name) = crate::config::get("name") {
                        kprintln!("[npk] Welcome back, {}.", name);
                    } else {
                        kprintln!("[npk] Unlocked.");
                    }
                    return;
                }
                _ => {
                    crypto::clear_master_key();
                    kprintln!("[npk] Wrong passphrase.");
                    attempts += 1;
                    if attempts >= 10 {
                        kprintln!("[npk] Too many failed attempts.");
                        crate::intent::system::intent_halt();
                    }
                }
            }
        }
    }
}

pub fn intent_passwd() {
    let salt = crate::npkfs::install_salt().unwrap_or([0u8; 16]);

    // Verify current passphrase
    kprint!("[npk] Current passphrase: ");
    let mut buf = [0u8; 128];
    let len = { serial::SERIAL.lock().read_line_masked(&mut buf) };
    if len == 0 {
        kprintln!("[npk] Cancelled.");
        return;
    }

    let old_key = crypto::derive_master_key(&buf[..len], &salt);
    for b in buf.iter_mut() { *b = 0; }

    // Temporarily set old key to verify
    let saved_key = crypto::get_master_key();
    crypto::set_master_key(old_key);

    match crate::npkfs::fetch(crate::config::KEYCHECK_PATH) {
        Ok((data, _)) if &data[..] == crate::config::KEYCHECK_VALUE => {}
        _ => {
            // Restore original key
            if let Some(k) = saved_key { crypto::set_master_key(k); }
            kprintln!("[npk] Wrong passphrase. Aborted.");
            return;
        }
    }

    // Delete old keycheck (still encrypted with old key)
    let _ = crate::npkfs::delete(crate::config::KEYCHECK_PATH);

    // Get new passphrase
    let new_key = loop {
        kprint!("[npk] New passphrase: ");
        let mut buf1 = [0u8; 128];
        let len1 = { serial::SERIAL.lock().read_line_masked(&mut buf1) };
        if len1 < 8 {
            kprintln!("[npk] Too short. Minimum 8 characters.");
            continue;
        }

        kprint!("[npk] Confirm passphrase: ");
        let mut buf2 = [0u8; 128];
        let len2 = { serial::SERIAL.lock().read_line_masked(&mut buf2) };

        if len1 != len2 || buf1[..len1] != buf2[..len2] {
            kprintln!("[npk] Passphrases do not match. Try again.");
            for b in buf1.iter_mut() { *b = 0; }
            for b in buf2.iter_mut() { *b = 0; }
            continue;
        }

        let key = crypto::derive_master_key(&buf1[..len1], &salt);
        for b in buf1.iter_mut() { *b = 0; }
        for b in buf2.iter_mut() { *b = 0; }
        break key;
    };

    // Set new key and re-encrypt keycheck
    crypto::set_master_key(new_key);
    match crate::npkfs::store(crate::config::KEYCHECK_PATH, crate::config::KEYCHECK_VALUE, crate::capability::CAP_NULL) {
        Ok(_) => kprintln!("[npk] Passphrase changed successfully."),
        Err(e) => kprintln!("[npk] ERROR: Could not store new keycheck: {}", e),
    }

    kprintln!("[npk] NOTE: Existing objects remain encrypted with the old key.");
    kprintln!("[npk]       They will be re-encrypted on next fetch+store cycle.");
}
