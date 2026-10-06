//! RTC — Real-Time Clock (CMOS 0x70/0x71)
//!
//! Reads the hardware clock as fallback time source.
//! BCD-encoded registers, no IRQ — just polled reads.

use crate::serial::{inb, outb};

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

// CMOS register indices
const REG_SECONDS: u8 = 0x00;
const REG_MINUTES: u8 = 0x02;
const REG_HOURS: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;

/// Serialises the index/data pair (Linux `rtc_lock`): 0x70 selects what
/// 0x71 reads, and the callers (clock host calls, TLS, npkFS) run on any core.
static CMOS_LOCK: spin::Mutex<()> = spin::Mutex::new(());

fn read_cmos(reg: u8) -> u8 {
    crate::interrupts::without_interrupts(|| {
        let _g = CMOS_LOCK.lock();
        // SAFETY: CMOS ports are standard x86 I/O, always present; the lock
        // keeps the index/data pair together.
        unsafe {
            outb(CMOS_ADDR, reg | 0x80); // bit 7 = disable NMI
            inb(CMOS_DATA)
        }
    })
}

fn bcd_to_bin(val: u8) -> u8 {
    (val & 0x0F) + ((val >> 4) * 10)
}

/// Wait until the RTC update-in-progress bit clears. An update takes under
/// 2 ms; false after 10 ms, which is what a missing RTC reading 0xFF gives.
fn wait_ready() -> bool {
    let freq = crate::interrupts::tsc_freq();
    let deadline = crate::interrupts::rdtsc() + (freq / 100).max(1);
    while read_cmos(REG_STATUS_A) & 0x80 != 0 {
        if crate::interrupts::rdtsc() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Read current time from CMOS RTC and return as Unix timestamp.
/// Returns None only if the clock reads nonsensical values.
pub fn read_unix_time() -> Option<u64> {
    // Read twice and compare to avoid torn reads during update
    for _ in 0..8 {
        if !wait_ready() { return None; }
        let s1 = read_cmos(REG_SECONDS);
        let m1 = read_cmos(REG_MINUTES);
        let h1 = read_cmos(REG_HOURS);
        let d1 = read_cmos(REG_DAY);
        let mo1 = read_cmos(REG_MONTH);
        let y1 = read_cmos(REG_YEAR);

        if !wait_ready() { return None; }
        let s2 = read_cmos(REG_SECONDS);
        let m2 = read_cmos(REG_MINUTES);
        let h2 = read_cmos(REG_HOURS);
        let d2 = read_cmos(REG_DAY);
        let mo2 = read_cmos(REG_MONTH);
        let y2 = read_cmos(REG_YEAR);

        if s1 == s2 && m1 == m2 && h1 == h2 && d1 == d2 && mo1 == mo2 && y1 == y2 {
            let status_b = read_cmos(REG_STATUS_B);
            let is_bcd = status_b & 0x04 == 0;
            let is_24h = status_b & 0x02 != 0;
            // In 12-hour mode bit 7 of the hour is PM.
            let pm = !is_24h && h1 & 0x80 != 0;
            let h1 = h1 & 0x7F;

            let (sec, min, hour, day, month, year_raw) = if is_bcd {
                (
                    bcd_to_bin(s1),
                    bcd_to_bin(m1),
                    bcd_to_bin(h1),
                    bcd_to_bin(d1),
                    bcd_to_bin(mo1),
                    bcd_to_bin(y1),
                )
            } else {
                (s1, m1, h1, d1, mo1, y1)
            };

            // 12 AM is hour 0, 12 PM is hour 12.
            let hour = if is_24h { hour } else { hour % 12 + if pm { 12 } else { 0 } };
            if sec > 59 || min > 59 || hour > 23 || !(1..=31).contains(&day)
                || !(1..=12).contains(&month) || year_raw > 99
            {
                return None;
            }

            // CMOS year is 0-99, assume 2000+
            let year = 2000u64 + year_raw as u64;

            return Some(datetime_to_unix(year, month as u64, day as u64,
                                         hour as u64, min as u64, sec as u64));
        }
        // Values changed during read, retry
    }
    None
}

/// Convert date/time to Unix timestamp (seconds since 1970-01-01 00:00:00 UTC).
fn datetime_to_unix(year: u64, month: u64, day: u64, hour: u64, min: u64, sec: u64) -> u64 {
    // Days from 1970 to start of given year
    let mut days = 0u64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    // Days from start of year to start of given month
    let month_days: [u64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    for m in 0..(month as usize).saturating_sub(1) {
        if m < 12 {
            days += month_days[m];
        }
    }

    days += day.saturating_sub(1);

    days * 86400 + hour * 3600 + min * 60 + sec
}

fn is_leap(y: u64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
