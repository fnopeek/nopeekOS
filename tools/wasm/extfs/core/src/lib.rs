//! Partition tables and file systems of external drives, for the `extfs`
//! module. Everything here reads crafted input: every offset, length and
//! count from the device is checked before it is used, and a structure
//! that does not add up is an error, never a panic.

#![no_std]

extern crate alloc;

pub mod cp437;
mod crc32;
pub mod fat;
pub mod part;

/// The device read failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoError;

/// A block device: the whole disk for the partition scan, one partition
/// for a file system.
pub trait Dev {
    fn block_size(&self) -> u32;
    fn blocks(&self) -> u64;
    /// Read `buf.len() / block_size` blocks from `lba`.
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError>;
}

/// Read `len` bytes at byte offset `off` of the device, any alignment.
pub fn read_bytes<D: Dev>(dev: &mut D, off: u64, len: usize) -> Result<alloc::vec::Vec<u8>, IoError> {
    let bs = dev.block_size() as u64;
    if bs == 0 { return Err(IoError); }
    let first = off / bs;
    let end = off.checked_add(len as u64).ok_or(IoError)?;
    let last = end.div_ceil(bs);
    if last > dev.blocks() { return Err(IoError); }
    let n = usize::try_from((last - first) * bs).map_err(|_| IoError)?;
    let mut buf = alloc::vec![0u8; n];
    dev.read(first, &mut buf)?;
    let skip = (off - first * bs) as usize;
    Ok(buf[skip..skip + len].into())
}
