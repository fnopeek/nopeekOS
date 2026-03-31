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

fn read_cmos(reg: u8) -> u8 {
    // SAFETY: CMOS ports are standard x86 I/O, always present.
    unsafe {
        outb(CMOS_ADDR, reg | 0x80); // bit 7 = disable NMI
        inb(CMOS_DATA)
    }
}

fn bcd_to_bin(val: u8) -> u8 {
    (val & 0x0F) + ((val >> 4) * 10)
}

/// Wait until the RTC update-in-progress bit clears.
fn wait_ready() {
    // Spin until bit 7 of Status Register A is 0
    while read_cmos(REG_STATUS_A) & 0x80 != 0 {
        core::hint::spin_loop();
    }
}

/// Read current time from CMOS RTC and return as Unix timestamp.
/// Returns None only if the clock reads nonsensical values.
pub fn read_unix_time() -> Option<u64> {
    // Read twice and compare to avoid torn reads during update
    loop {
        wait_ready();
        let s1 = read_cmos(REG_SECONDS);
        let m1 = read_cmos(REG_MINUTES);
        let h1 = read_cmos(REG_HOURS);
        let d1 = read_cmos(REG_DAY);
        let mo1 = read_cmos(REG_MONTH);
        let y1 = read_cmos(REG_YEAR);

        wait_ready();
        let s2 = read_cmos(REG_SECONDS);
        let m2 = read_cmos(REG_MINUTES);
        let h2 = read_cmos(REG_HOURS);
        let d2 = read_cmos(REG_DAY);
        let mo2 = read_cmos(REG_MONTH);
        let y2 = read_cmos(REG_YEAR);

        if s1 == s2 && m1 == m2 && h1 == h2 && d1 == d2 && mo1 == mo2 && y1 == y2 {
            let status_b = read_cmos(REG_STATUS_B);
            let is_bcd = status_b & 0x04 == 0;

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

            // CMOS year is 0-99, assume 2000+
            let year = 2000u64 + year_raw as u64;

            return Some(datetime_to_unix(year, month as u64, day as u64,
                                         hour as u64, min as u64, sec as u64));
        }
        // Values changed during read, retry
    }
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
