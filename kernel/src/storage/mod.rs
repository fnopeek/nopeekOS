//! Storage subsystem
//!
//! npkFS (content-addressed, COW B-tree), GPT partition detection, FAT32.

pub mod npkfs;
#[cfg(target_arch = "aarch64")]
pub mod seed;
#[allow(dead_code)]
pub mod fat32;
#[allow(dead_code)]
pub mod gpt;
/// External volumes: the `ext/` namespace.
pub mod ext;
/// Removable disks of every transport.
pub mod disk;
