//! What a removable disk driver (USB mass storage, SD) reports and how its
//! requests fail.

use alloc::string::String;

/// One disk: a LUN that answered, or a card, with a medium.
#[derive(Clone)]
pub struct DiskInfo {
    pub id: u32,
    pub port: String,
    pub name: String,
    pub vendor: String,
    pub product: String,
    pub block_size: u32,
    pub blocks: u64,
    pub removable: bool,
    pub read_only: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// The disk is gone.
    Gone,
    /// Transport failure; the device was reset.
    Io,
    /// The device refused the command (sense key, ASC, ASCQ).
    Sense(u8, u8, u8),
    /// Blocks outside the disk, or a buffer of the wrong size.
    Range,
    ReadOnly,
}

