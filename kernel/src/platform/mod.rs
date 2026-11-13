//! How the board describes itself: ACPI on PCs, the device tree elsewhere.

#[cfg(target_arch = "aarch64")]
pub mod dt;
#[cfg(target_arch = "aarch64")]
pub mod simplefb;
#[cfg(target_arch = "aarch64")]
pub mod gpio;
#[cfg(target_arch = "aarch64")]
pub mod led;
#[cfg(target_arch = "aarch64")]
pub mod pi5;
#[cfg(target_arch = "aarch64")]
pub mod pi5_eth;
#[cfg(target_arch = "aarch64")]
pub mod pi5_sd;
#[cfg(target_arch = "aarch64")]
pub mod rng200;
#[cfg(target_arch = "aarch64")]
pub mod rpi_fw;
#[cfg(target_arch = "aarch64")]
pub mod pi5_display;
#[cfg(target_arch = "aarch64")]
pub mod pi5_display_set;
