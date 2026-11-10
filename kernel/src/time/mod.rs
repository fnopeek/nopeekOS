//! Time: the monotonic clock since boot (`clock`) and the wall clock
//! (`wall`, with its time zones in `zone` and its sources in `ntp`).

pub mod clock;
pub mod wall;
pub mod zone;
#[cfg(target_arch = "x86_64")]
pub mod ntp;

pub use clock::{counter, counter_hz, counter_to_ns, delay_ms, delay_us, init_clock, ticks, uptime_ns, uptime_secs, uptime_us};
pub use wall::*;
#[cfg(target_arch = "x86_64")]
pub use ntp::*;
