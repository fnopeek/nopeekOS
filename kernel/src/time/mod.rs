//! Time: the monotonic clock since boot (`clock`) and the wall clock
//! (`wall`, with its time zones in `zone`).

pub mod clock;
#[cfg(target_arch = "x86_64")]
pub mod wall;
#[cfg(target_arch = "x86_64")]
pub mod zone;

pub use clock::{counter, counter_hz, counter_to_ns, delay_ms, init_clock, ticks, uptime_ns, uptime_secs, uptime_us};
#[cfg(target_arch = "aarch64")]
pub use clock::delay_us;
#[cfg(target_arch = "x86_64")]
pub use wall::*;
