//! How the board describes itself: ACPI on PCs, the device tree elsewhere.

#[cfg(target_arch = "aarch64")]
pub mod dt;
#[cfg(target_arch = "aarch64")]
pub mod simplefb;
#[cfg(target_arch = "aarch64")]
pub mod led;
