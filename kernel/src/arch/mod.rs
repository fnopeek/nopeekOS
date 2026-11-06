//! The architecture layer: everything that differs between CPU
//! architectures. Code outside `arch/` calls these functions and never
//! uses `asm!` or `cfg(target_arch)` itself.

#[cfg(target_arch = "x86_64")]
pub mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;
