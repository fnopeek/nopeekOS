//! Linux guest plumbing for the MicroVM subsystem.
//!
//! Platform-agnostic — knows nothing about VMX or SVM. The Linux
//! Boot Protocol loader writes into a host-physical "guest RAM"
//! window that the CPU backend has already mapped (EPT on Intel,
//! NPT on AMD); from this module's perspective it's just memory.

#[cfg(target_arch = "aarch64")]
pub mod arm64;
#[cfg(target_arch = "aarch64")]
pub mod fdt_writer;
#[cfg(target_arch = "x86_64")]
pub mod bzimage;
#[cfg(target_arch = "x86_64")]
pub mod mptable;
