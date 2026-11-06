//! Removable disks of every transport in one id space: what `ext` and the
//! `drives` intent see. Each transport hands out its own ids; a range of
//! the id space tells which one serves a disk.

use alloc::vec::Vec;
use crate::drivers::usb_storage;

pub use usb_storage::{DiskInfo, Error};

/// Every disk present now.
pub fn disks() -> Vec<DiskInfo> {
    usb_storage::disks()
}

pub fn info(id: u32) -> Option<DiskInfo> {
    disks().into_iter().find(|d| d.id == id)
}

pub fn read(id: u32, lba: u64, out: &mut [u8]) -> Result<(), Error> {
    usb_storage::read(id, lba, out)
}

pub fn write(id: u32, lba: u64, data: &[u8]) -> Result<(), Error> {
    usb_storage::write(id, lba, data)
}

pub fn flush(id: u32) -> Result<(), Error> {
    usb_storage::flush(id)
}

/// Flush and stop the disk, then drop it from the list.
pub fn eject(id: u32) -> Result<(), Error> {
    usb_storage::eject(id)
}
