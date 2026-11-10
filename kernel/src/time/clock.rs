//! Monotonic time since boot, from the architecture's counter
//! (`arch::counter`, `arch::counter_hz`).

use core::sync::atomic::{AtomicU64, Ordering};

pub use crate::arch::{counter, counter_hz};

/// Counter value at boot: the zero point of every clock below.
static BOOT: AtomicU64 = AtomicU64::new(0);

/// Call once at boot, after `arch::calibrate_counter`.
pub fn init_clock() {
    BOOT.store(counter(), Ordering::Relaxed);
}

/// 100 Hz ticks since boot, derived from the counter.
///
/// An interrupt count would stop while Core 0 runs with interrupts off and
/// could not survive the tick going away. The unit is 10 ms.
pub fn ticks() -> u64 {
    let period = counter_hz() / 100;
    let boot = BOOT.load(Ordering::Relaxed);
    if period == 0 || boot == 0 { return 0; }
    counter().saturating_sub(boot) / period
}

/// Seconds since boot (approximate)
pub fn uptime_secs() -> u64 {
    ticks() / 100
}

/// Microseconds since boot.
///
/// `ticks()` is 100 Hz, too coarse for a receive-side RTT. Linux' DRS
/// (`tcp_rcv_space_adjust`) compares an elapsed time against
/// `rcv_rtt_est.rtt_us`, in microseconds; this is that clock.
pub fn uptime_us() -> u64 {
    let per_us = (counter_hz() / 1_000_000).max(1);
    let boot = BOOT.load(Ordering::Relaxed);
    counter().saturating_sub(boot) / per_us
}

/// Nanoseconds since boot (Linux `CLOCK_MONOTONIC`).
pub fn uptime_ns() -> u64 {
    let freq = counter_hz().max(1);
    let boot = BOOT.load(Ordering::Relaxed);
    let cycles = counter().saturating_sub(boot) as u128;
    (cycles * 1_000_000_000 / freq as u128) as u64
}

/// Counter units converted to nanoseconds, for measurements below a
/// microsecond.
pub fn counter_to_ns(cycles: u64) -> u64 {
    let freq = counter_hz().max(1);
    cycles.saturating_mul(1_000_000_000) / freq
}

/// Busy-wait for at least `us` microseconds.
#[cfg(target_arch = "aarch64")]
pub fn delay_us(us: u64) {
    let target = counter() + (us * counter_hz()).div_ceil(1_000_000);
    while counter() < target {
        core::hint::spin_loop();
    }
}

/// Busy-wait for approximately `ms` milliseconds.
pub fn delay_ms(ms: u64) {
    let per_ms = counter_hz() / 1000;
    let target = counter() + ms * per_ms;
    while counter() < target {
        core::hint::spin_loop();
    }
}
