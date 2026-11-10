//! Memory management
//!
//! Physical frame allocator, heap allocator, virtual memory (paging).

pub mod memory;
pub mod heap;
#[cfg(target_arch = "x86_64")]
pub mod paging;
#[cfg(target_arch = "aarch64")]
pub use crate::arch::aarch64::paging;
pub mod stack;
#[cfg(target_arch = "x86_64")]
mod cache;
#[cfg(target_arch = "x86_64")]
pub use cache::clflush_range;
