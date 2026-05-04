//! Linux guest plumbing for the MicroVM subsystem.
//!
//! Platform-agnostic — knows nothing about VMX or SVM. The Linux
//! Boot Protocol loader writes into a host-physical "guest RAM"
//! window that the CPU backend has already mapped (EPT on Intel,
//! NPT on AMD); from this module's perspective it's just memory.

pub mod bzimage;
