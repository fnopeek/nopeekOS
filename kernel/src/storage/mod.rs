//! Storage subsystem
//!
//! npkFS (content-addressed, COW B-tree), GPT partition detection, FAT32.

pub mod npkfs;
#[allow(dead_code)]
pub mod fat32;
#[allow(dead_code)]
pub mod gpt;
