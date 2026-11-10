//! Symmetric multiprocessing: the per-core scheduler (`per_core`,
//! `scheduler`, `fiber`) and, in `hw`, the architecture's half: how cores
//! are found, started, told apart, woken and halted.
//!
//! Design: Core 0 = Kernel/IRQ (fixed), Cores 1..N = Worker Pool.

pub mod fiber;
pub mod par;
pub mod per_core;
pub mod scheduler;

#[cfg_attr(target_arch = "x86_64", path = "x86.rs")]
#[cfg_attr(target_arch = "aarch64", path = "aarch64.rs")]
mod hw;

pub use hw::init;
#[cfg(target_arch = "x86_64")]
pub use hw::{kick_host_core, kick_host_core_posted, send_wake_ipi};
