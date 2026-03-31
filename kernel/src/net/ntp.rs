//! NTP — Network Time Protocol (SNTP client)
//!
//! Simple NTP query to get wall-clock time.
//! Uses UDP port 123, parses NTP v4 response.

use spin::Mutex;
use super::udp;

const NTP_PORT: u16 = 123;
const LOCAL_PORT: u16 = 10123;
const NTP_EPOCH_OFFSET: u64 = 2_208_988_800; // seconds from 1900 to 1970

/// Stored wall-clock time: Unix timestamp at the tick when it was set
static WALL_CLOCK: Mutex<Option<(u64, u64)>> = Mutex::new(None); // (unix_secs, tick_at_sync)

/// Set wall clock directly (e.g. from RTC).
pub fn set_time(unix_secs: u64) {
    let tick = crate::interrupts::ticks();
    *WALL_CLOCK.lock() = Some((unix_secs, tick));
}

/// Resolve NTP server hostname via DNS, then sync.
pub fn sync_via_dns(hostname: &str) -> bool {
    if let Some(ip) = super::dns::resolve(hostname) {
        sync(ip)
    } else {
        // Fallback: QEMU user-mode gateway
        sync([10, 0, 2, 3])
    }
}

/// Sync time from an NTP server. Blocking.
pub fn sync(server_ip: [u8; 4]) -> bool {
    // Build SNTP request (48 bytes)
    let mut req = [0u8; 48];
    req[0] = 0x23; // LI=0, Version=4, Mode=3 (client)

    // Ensure ARP
    super::arp::request(server_ip);
    let arp_wait = crate::interrupts::ticks() + 10; // ~100ms
    while crate::interrupts::ticks() < arp_wait {
        super::poll();
        core::hint::spin_loop();
    }

    udp::listen(LOCAL_PORT);
    udp::send(server_ip, LOCAL_PORT, NTP_PORT, &req);

    let t0 = crate::interrupts::ticks();
    let mut result = false;

    loop {
        super::poll();
        if let Some((_src, _port, data)) = udp::recv(LOCAL_PORT) {
            if data.len() >= 48 {
                // Transmit timestamp at offset 40 (seconds since 1900-01-01)
                let secs = u32::from_be_bytes([data[40], data[41], data[42], data[43]]) as u64;
                if secs > NTP_EPOCH_OFFSET {
                    let unix = secs - NTP_EPOCH_OFFSET;
                    let tick = crate::interrupts::ticks();
                    *WALL_CLOCK.lock() = Some((unix, tick));
                    result = true;
                }
                break;
            }
        }
        if crate::interrupts::ticks() - t0 > 300 { break; } // 3s timeout
        core::hint::spin_loop();
    }

    udp::unlisten(LOCAL_PORT);
    result
}

/// Get current Unix timestamp (seconds since 1970-01-01).
/// Returns None if NTP hasn't synced yet.
pub fn unix_time() -> Option<u64> {
    let lock = WALL_CLOCK.lock();
    let (base_unix, base_tick) = (*lock)?;
    let now_tick = crate::interrupts::ticks();
    let elapsed_secs = (now_tick - base_tick) / 100; // 100Hz ticks
    Some(base_unix + elapsed_secs)
}

/// Format Unix timestamp as local time using timezone config.
/// Shows "YYYY-MM-DD HH:MM:SS UTC+N" or "UTC" if no offset.
pub fn format_time(unix: u64) -> alloc::string::String {
    let offset_mins = crate::config::timezone_offset_minutes();
    let local_secs = (unix as i64 + offset_mins as i64 * 60) as u64;

    let secs = local_secs % 60;
    let mins = (local_secs / 60) % 60;
    let hours = (local_secs / 3600) % 24;
    let days = local_secs / 86400;

    let (year, month, day) = days_to_date(days);

    if offset_mins == 0 {
        alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            year, month, day, hours, mins, secs)
    } else {
        let sign = if offset_mins >= 0 { '+' } else { '-' };
        let abs_hours = (offset_mins.abs() / 60) as u64;
        let abs_mins = (offset_mins.abs() % 60) as u64;
        if abs_mins == 0 {
            alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC{}{}",
                year, month, day, hours, mins, secs, sign, abs_hours)
        } else {
            alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC{}{}:{:02}",
                year, month, day, hours, mins, secs, sign, abs_hours, abs_mins)
        }
    }
}

fn days_to_date(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year { break; }
        days -= days_in_year;
        year += 1;
    }

    let month_days: [u64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u64;
    for &md in &month_days {
        if days < md { break; }
        days -= md;
        month += 1;
    }

    (year, month, days + 1)
}

fn is_leap(y: u64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
