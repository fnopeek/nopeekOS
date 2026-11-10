//! The wall clock's sources: the CMOS RTC at boot, NTP whenever an
//! interface gets an address and then periodically, written back to the
//! RTC in UTC after each sync (`hwclock --systohc`).

use core::sync::atomic::{AtomicBool, Ordering};
use super::wall::{format, mark_synced, now, set};

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

/// Step the clock to an NTP answer and write it to the RTC, so the next
/// boot starts close to right even without a network.
fn step(unix: u64) {
    let before = now();
    set(unix);
    let first = mark_synced();
    #[cfg(target_arch = "x86_64")]
    if !crate::rtc::write_unix_time(unix) {
        crate::kprintln!("[npk] time: RTC write failed");
    }
    let off = before.map(|b| unix as i64 - b as i64).unwrap_or(0);
    // The periodic check stays quiet while the clock holds.
    if first || off != 0 {
        crate::kprintln!("[npk] NTP: {} (stepped {:+} s)", format(unix), off);
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
    let t0 = crate::time::ticks();
    let mut asked: alloc::vec::Vec<[u8; 4]> = alloc::vec::Vec::new();
    let mut answers: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    for host in NTP_HOSTS {
        let Some(ip) = crate::net::dns::resolve(host) else { continue };
        if asked.contains(&ip) { continue; }
        asked.push(ip);
        if let Some(t) = crate::net::ntp::query(ip) {
            // Back to the moment the sync began, so slow answers compare.
            answers.push(t.saturating_sub(crate::time::ticks().saturating_sub(t0) / 100));
        }
    }
    answers.sort_unstable();
    let Some(&median) = answers.get(answers.len() / 2) else {
        crate::kprintln!("[npk] NTP: no answer");
        return false;
    };
    let agree = answers.iter().filter(|&&t| t.abs_diff(median) <= AGREE_SECS).count();
    if agree < 2 || agree * 2 <= answers.len() {
        crate::kprintln!("[npk] NTP: {} of {} answers agree, clock left alone", agree, answers.len());
        return false;
    }
    let floor = proven_floor();
    if median < floor {
        crate::kprintln!("[npk] NTP: answer is before {} (a signed release), clock left alone",
            format(floor));
        return false;
    }
    step(median.saturating_add(crate::time::ticks().saturating_sub(t0) / 100));
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
    if crate::smp::scheduler::worker_count() > 0 {
        crate::smp::scheduler::spawn("ntp", sync_task, 0);
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

