//! Removable disks of every transport in one id space: what `ext` and the
//! `drives` intent see. Each transport hands out its own ids; a range of
//! the id space tells which one serves a disk. The disk the system runs
//! from is never one of them, whatever its transport.

use alloc::vec::Vec;
use crate::drivers::mmc_sd;
#[cfg(target_arch = "x86_64")]
use crate::drivers::usb_storage;

pub use crate::drivers::disk::{DiskInfo, Error};

fn is_sd(id: u32) -> bool {
    id >= mmc_sd::SD_ID_BASE
}

/// The system disk's id, refused as a removable one.
fn is_system(id: u32) -> bool {
    crate::blkdev::system_sd() == Some(id)
}

/// Every disk present now.
pub fn disks() -> Vec<DiskInfo> {
    #[cfg(target_arch = "x86_64")]
    let mut v = usb_storage::disks();
    #[cfg(not(target_arch = "x86_64"))]
    let mut v = Vec::new();
    v.extend(mmc_sd::disks());
    v.retain(|d| !is_system(d.id));
    v
}

pub fn info(id: u32) -> Option<DiskInfo> {
    disks().into_iter().find(|d| d.id == id)
}

pub fn read(id: u32, lba: u64, out: &mut [u8]) -> Result<(), Error> {
    if is_system(id) { return Err(Error::Gone); }
    if is_sd(id) { return mmc_sd::read(id, lba, out); }
    #[cfg(target_arch = "x86_64")]
    return usb_storage::read(id, lba, out);
    #[cfg(not(target_arch = "x86_64"))]
    Err(Error::Gone)
}

pub fn write(id: u32, lba: u64, data: &[u8]) -> Result<(), Error> {
    if is_system(id) { return Err(Error::Gone); }
    if is_sd(id) { return mmc_sd::write(id, lba, data); }
    #[cfg(target_arch = "x86_64")]
    return usb_storage::write(id, lba, data);
    #[cfg(not(target_arch = "x86_64"))]
    Err(Error::Gone)
}

pub fn flush(id: u32) -> Result<(), Error> {
    if is_system(id) { return Err(Error::Gone); }
    if is_sd(id) { return mmc_sd::flush(id); }
    #[cfg(target_arch = "x86_64")]
    return usb_storage::flush(id);
    #[cfg(not(target_arch = "x86_64"))]
    Err(Error::Gone)
}

/// Flush and stop the disk, then drop it from the list.
pub fn eject(id: u32) -> Result<(), Error> {
    if is_system(id) { return Err(Error::Gone); }
    if is_sd(id) { return mmc_sd::eject(id); }
    #[cfg(target_arch = "x86_64")]
    return usb_storage::eject(id);
    #[cfg(not(target_arch = "x86_64"))]
    Err(Error::Gone)
}
