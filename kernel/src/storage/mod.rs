//! Storage subsystem
//!
//! npkFS (content-addressed, COW B-tree), GPT partition detection, FAT32.

pub mod npkfs;
#[allow(dead_code)]
pub mod fat32;
#[allow(dead_code)]
pub mod gpt;
/// External volumes: the `ext/` namespace.
#[cfg(target_arch = "x86_64")]
pub mod ext;
/// Removable disks of every transport.
#[cfg(target_arch = "x86_64")]
pub mod disk;
