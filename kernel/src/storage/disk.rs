//! Removable disks of every transport in one id space: what `ext` and the
//! `drives` intent see. Each transport hands out its own ids; a range of
//! the id space tells which one serves a disk.

use alloc::vec::Vec;
use crate::drivers::{mmc_sd, usb_storage};

pub use usb_storage::{DiskInfo, Error};

fn is_sd(id: u32) -> bool {
    id >= mmc_sd::SD_ID_BASE
}

/// Every disk present now.
pub fn disks() -> Vec<DiskInfo> {
    let mut v = usb_storage::disks();
    v.extend(mmc_sd::disks());
    v
}

pub fn info(id: u32) -> Option<DiskInfo> {
    disks().into_iter().find(|d| d.id == id)
}

pub fn read(id: u32, lba: u64, out: &mut [u8]) -> Result<(), Error> {
    if is_sd(id) { mmc_sd::read(id, lba, out) } else { usb_storage::read(id, lba, out) }
}

pub fn write(id: u32, lba: u64, data: &[u8]) -> Result<(), Error> {
    if is_sd(id) { mmc_sd::write(id, lba, data) } else { usb_storage::write(id, lba, data) }
}

pub fn flush(id: u32) -> Result<(), Error> {
    if is_sd(id) { mmc_sd::flush(id) } else { usb_storage::flush(id) }
}

/// Flush and stop the disk, then drop it from the list.
pub fn eject(id: u32) -> Result<(), Error> {
    if is_sd(id) { mmc_sd::eject(id) } else { usb_storage::eject(id) }
}
