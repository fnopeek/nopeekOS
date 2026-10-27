//! Wall clock: one system time for every reader, as Linux's
//! `CLOCK_REALTIME`. Set from the CMOS RTC at boot, stepped by NTP whenever
//! an interface gets an address, and written back to the RTC in UTC after
//! each sync (`hwclock --systohc`). Local time goes through [`zone`].

pub mod zone;

use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// UTC seconds at the tick they were taken (`interrupts::ticks`, 100 Hz).
static WALL: Mutex<Option<(u64, u64)>> = Mutex::new(None);

/// True once NTP has set the clock in this boot.
static SYNCED: AtomicBool = AtomicBool::new(false);

/// An NTP sync is queued or running.
static SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

const NTP_HOST: &str = "pool.ntp.org";

/// Take the RTC as the starting time. Called once at boot.
pub fn init_from_rtc() -> Option<u64> {
    let t = crate::rtc::read_unix_time()?;
    set(t);
    Some(t)
}

fn set(unix: u64) {
    *WALL.lock() = Some((unix, crate::interrupts::ticks()));
}

/// Current UTC seconds, or `None` before any source set the clock.
pub fn now() -> Option<u64> {
    let (base, at) = (*WALL.lock())?;
    Some(base + crate::interrupts::ticks().saturating_sub(at) / 100)
}

/// [`now`], or 0 when the clock is unset.
pub fn now_or_zero() -> u64 {
    now().unwrap_or(0)
}

/// Whether NTP has set the clock in this boot.
pub fn synced() -> bool {
    SYNCED.load(Ordering::Acquire)
}

/// Step the clock to an NTP answer and write it to the RTC, so the next
/// boot starts close to right even without a network.
fn step(unix: u64) {
    let before = now();
    set(unix);
    SYNCED.store(true, Ordering::Release);
    if !crate::rtc::write_unix_time(unix) {
        crate::kprintln!("[npk] time: RTC write failed");
    }
    let off = before.map(|b| unix as i64 - b as i64).unwrap_or(0);
    crate::kprintln!("[npk] NTP: {} (stepped {:+} s)", format(unix), off);
}

/// Query NTP and step the clock. Blocks for up to the DNS and NTP
/// timeouts (several seconds); call it where blocking is allowed.
pub fn sync_now() -> bool {
    match crate::net::ntp::query_via_dns(NTP_HOST) {
        Some(t) => { step(t); true }
        None => {
            crate::kprintln!("[npk] NTP: no answer from {}", NTP_HOST);
            false
        }
    }
}

/// Sync in the background. Called when an interface gets an address. A
/// worker runs it; a machine without workers syncs in place.
pub fn request_sync() {
    if SYNC_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    if crate::smp::scheduler::worker_count() > 0 {
        crate::smp::scheduler::spawn("ntp", sync_task, 0);
    } else {
        sync_task(0);
    }
}

fn sync_task(_: u64) {
    sync_now();
    SYNC_RUNNING.store(false, Ordering::Release);
}

/// Local seconds (UTC plus the zone's offset at that instant).
pub fn local(unix: u64) -> i64 {
    unix as i64 + zone::offset_at(unix) as i64
}

/// `YYYY-MM-DD HH:MM:SS <zone abbreviation>` in local time.
pub fn format(unix: u64) -> String {
    let (off, abbr) = zone::at(unix);
    let l = unix as i64 + off as i64;
    let (y, mo, d) = civil_from_days(l.div_euclid(86_400));
    let s = l.rem_euclid(86_400);
    alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}",
        y, mo, d, s / 3600, (s / 60) % 60, s % 60, abbr)
}

/// `YYYY-MM-DD` in local time.
pub fn format_date(unix: u64) -> String {
    let (y, mo, d) = civil_from_days(local(unix).div_euclid(86_400));
    alloc::format!("{:04}-{:02}-{:02}", y, mo, d)
}

/// Days since 1970-01-01 to (year, month 1..12, day 1..31), proleptic
/// Gregorian (Howard Hinnant's `civil_from_days`).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

/// (year, month 1..12, day 1..31) to days since 1970-01-01 (Hinnant's
/// `days_from_civil`).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Day of the week of a day number, 0 = Sunday.
pub fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}
