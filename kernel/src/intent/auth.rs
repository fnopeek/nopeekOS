//! Authentication intents: lock, passwd

use crate::{kprint, kprintln, serial};

pub fn intent_lock() {
    kprintln!("[npk] System locked.");
    crate::disk_key::lock();

    // Use GUI login screen if framebuffer available
    if crate::framebuffer::is_available() {
        crate::gui::login::run();
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

            let ok = crate::disk_key::unlock(&buf[..len]);
            for b in buf.iter_mut() { *b = 0; }

            if ok {
                crate::config::load();
                if let Some(name) = crate::config::get("name") {
                    kprintln!("[npk] Welcome back, {}.", name);
                } else {
                    kprintln!("[npk] Unlocked.");
                }
                return;
            }
            kprintln!("[npk] Wrong passphrase.");
            attempts += 1;
            if attempts >= 10 {
                kprintln!("[npk] Too many failed attempts.");
                crate::intent::system::intent_halt();
            }
        }
    }
}

/// `passwd` — rewrap the disk key under a new passphrase. The data stays as
/// it is: it is encrypted under the data key, which does not change.
pub fn intent_passwd() {
    kprint!("[npk] Current passphrase: ");
    let mut old = [0u8; 128];
    let old_len = { serial::SERIAL.lock().read_line_masked(&mut old) };
    if old_len == 0 {
        kprintln!("[npk] Cancelled.");
        return;
    }

    let mut new = [0u8; 128];
    let new_len = loop {
        kprint!("[npk] New passphrase: ");
        let len1 = { serial::SERIAL.lock().read_line_masked(&mut new) };
        if len1 < 8 {
            kprintln!("[npk] Too short. Minimum 8 characters.");
            continue;
        }

        kprint!("[npk] Confirm passphrase: ");
        let mut confirm = [0u8; 128];
        let len2 = { serial::SERIAL.lock().read_line_masked(&mut confirm) };
        let same = len1 == len2 && new[..len1] == confirm[..len2];
        confirm.fill(0);
        if !same {
            kprintln!("[npk] Passphrases do not match. Try again.");
            new.fill(0);
            continue;
        }
        break len1;
    };

    let r = crate::disk_key::change(&old[..old_len], &new[..new_len]);
    old.fill(0);
    new.fill(0);
    match r {
        Ok(()) => kprintln!("[npk] Passphrase changed."),
        Err(e) => kprintln!("[npk] Passphrase not changed: {}.", e),
    }
}
