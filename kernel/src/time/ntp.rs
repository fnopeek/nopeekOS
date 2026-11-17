//! The wall clock's sources: the CMOS RTC at boot, NTP whenever an
//! interface gets an address and then periodically, written back to the
//! RTC in UTC after each sync (`hwclock --systohc`).

use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_arch = "x86_64")]
use super::wall::set;
use super::wall::{format, mark_synced, now_ns, set_ns};

/// An NTP sync is queued or running.
static SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

/// When the next sync is due (ticks, 0 = none planned), and the wait after
/// a failed one (ticks). After systemd-timesyncd: a good clock is checked
/// again after its longest poll interval, 2048 s; a failure is retried after
/// 15 s, growing by 4/3 up to 6 min.
static NEXT_SYNC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RETRY_TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
const POLL_INTERVAL_MAX_TICKS: u64 = 2048 * 100;
const RETRY_INTERVAL_MIN_TICKS: u64 = 15 * 100;
const RETRY_INTERVAL_MAX_TICKS: u64 = 6 * 60 * 100;

/// Separate lookups, so the answers come from separate servers.
const NTP_HOSTS: [&str; 4] = ["0.pool.ntp.org", "1.pool.ntp.org", "2.pool.ntp.org", "3.pool.ntp.org"];
/// Answers this close to their median agree.
const AGREE_SECS: u64 = 2;

/// Take the RTC as the starting time. Called once at boot.
#[cfg(target_arch = "x86_64")]
pub fn init_from_rtc() -> Option<u64> {
    let t = crate::rtc::read_unix_time()?;
    set(t);
    Some(t)
}

/// A step smaller than this (ms) is not reported by the periodic check.
const QUIET_STEP_MS: i64 = 50;

/// Step the clock to an NTP answer, UTC nanoseconds `ns` as of
/// `time::uptime_ns` `at`, and write it to the RTC, so the next boot
/// starts close to right even without a network.
fn step(ns: u64, at: u64) {
    let before = now_ns().map(|b| b.saturating_sub(crate::time::uptime_ns().saturating_sub(at)));
    set_ns(ns, at);
    let first = mark_synced();
    let unix = ns / 1_000_000_000;
    #[cfg(target_arch = "x86_64")]
    if !crate::rtc::write_unix_time(unix) {
        crate::kprintln!("[npk] time: RTC write failed");
    }
    let off_ms = before.map(|b| (ns as i64 - b as i64) / 1_000_000).unwrap_or(0);
    // The periodic check stays quiet while the clock holds.
    if first || off_ms.abs() >= QUIET_STEP_MS {
        crate::kprintln!("[npk] NTP: {} (stepped {:+} ms)", format(unix), off_ms);
    }
}

/// Query NTP and step the clock. Blocks for up to the DNS and NTP
/// timeouts (several seconds); call it where blocking is allowed.
///
/// SNTP is unauthenticated and certificate validity rests on this clock, so
/// one answer is not enough: several servers must agree (a majority within
/// `AGREE_SECS` of the median), and the clock never goes back before a time
/// a signed release has proved.
pub fn sync_now() -> bool {
    let mut asked: alloc::vec::Vec<[u8; 4]> = alloc::vec::Vec::new();
    // Each answer as UTC nanoseconds and the uptime it holds for.
    let mut got: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    for host in NTP_HOSTS {
        let Some(ip) = crate::net::dns::resolve(host) else { continue };
        if asked.contains(&ip) { continue; }
        asked.push(ip);
        if let Some(t) = crate::net::ntp::query(ip) {
            got.push(t);
        }
    }
    // All carried forward to one moment, so answers taken apart compare.
    let at = crate::time::uptime_ns();
    let mut answers: alloc::vec::Vec<u64> = got.iter().map(|&(ns, t)| ns + (at - t)).collect();
    answers.sort_unstable();
    let Some(&median) = answers.get(answers.len() / 2) else {
        crate::kprintln!("[npk] NTP: no answer");
        return false;
    };
    let agree = answers.iter().filter(|&&t| t.abs_diff(median) <= AGREE_SECS * 1_000_000_000).count();
    if agree < 2 || agree * 2 <= answers.len() {
        crate::kprintln!("[npk] NTP: {} of {} answers agree, clock left alone", agree, answers.len());
        return false;
    }
    let floor = proven_floor();
    if median / 1_000_000_000 < floor {
        crate::kprintln!("[npk] NTP: answer is before {} (a signed release), clock left alone",
            format(floor));
        return false;
    }
    step(median, at);
    true
}

/// The latest time a signed release proved: its `issued=` stamp cannot
/// predate its signing.
fn proven_floor() -> u64 {
    ["kernel", "kernel-aarch64", "modules", "assets"].iter()
        .filter_map(|k| crate::npkfs::fetch(&alloc::format!("sys/ota/issued-{}", k)).ok())
        .filter_map(|(d, _)| core::str::from_utf8(&d).ok()?.trim().parse::<u64>().ok())
        .max()
        .unwrap_or(0)
}

/// Sync in the background. Called when an interface gets an address. A
/// worker runs it; a machine without workers syncs in place.
pub fn request_sync() {
    if SYNC_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    // A fiber: its network waits let the core's other fibers run.
    if crate::smp::scheduler::worker_count() > 0 {
        crate::smp::scheduler::spawn_fiber(sync_task, 0);
    } else {
        sync_task(0);
    }
}

fn sync_task(_: u64) {
    let ok = sync_now();
    let wait = if ok {
        RETRY_TICKS.store(0, Ordering::Relaxed);
        POLL_INTERVAL_MAX_TICKS
    } else {
        let r = match RETRY_TICKS.load(Ordering::Relaxed) {
            0 => RETRY_INTERVAL_MIN_TICKS,
            r => (r * 4 / 3).min(RETRY_INTERVAL_MAX_TICKS),
        };
        RETRY_TICKS.store(r, Ordering::Relaxed);
        r
    };
    NEXT_SYNC.store(crate::time::ticks() + wait, Ordering::Relaxed);
    SYNC_RUNNING.store(false, Ordering::Release);
}

/// Start the planned sync when it is due. Cheap; call it about once a
/// second.
pub fn tick() {
    let due = NEXT_SYNC.load(Ordering::Relaxed);
    if due != 0 && crate::time::ticks() >= due && crate::net::arp::our_ip() != [0; 4] {
        NEXT_SYNC.store(0, Ordering::Relaxed);
        request_sync();
    }
}

