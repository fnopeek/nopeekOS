//! Security subsystem
//!
//! Capability vault, audit log, cryptographic PRNG.

#[cfg(target_arch = "x86_64")]
pub mod capability;
#[cfg(target_arch = "x86_64")]
pub mod audit;
pub mod csprng;
#[cfg(target_arch = "x86_64")]
pub mod disk_key;
#[cfg(target_arch = "x86_64")]
pub mod keyvault;
