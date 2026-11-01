//! Partition tables and file systems of external drives, for the `extfs`
//! module. Everything here reads crafted input: every offset, length and
//! count from the device is checked before it is used, and a structure
//! that does not add up is an error, never a panic.

#![no_std]

extern crate alloc;

pub mod cp437;
mod crc32;
pub mod exfat;
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
    /// Write `buf.len() / block_size` blocks at `lba`.
    fn write(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), IoError> { Err(IoError) }
    /// Make what was written durable (the device's write cache).
    fn flush(&mut self) -> Result<(), IoError> { Ok(()) }
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

/// What a mounted file system offers the `extfs` module, whatever the
/// format. Paths are `/`-separated below the root.
pub trait Volume {
    type Node: Copy;
    fn label(&self) -> &str;
    /// The volume was not cleanly unmounted.
    fn dirty(&self) -> bool;
    fn total_bytes(&self) -> u64;
    fn free_bytes(&self) -> u64;
    /// Folder or not, size, modification time (UTC seconds, 0 unknown).
    fn info(node: &Self::Node) -> (bool, u64, u64);
    fn lookup(&mut self, path: &str) -> Result<Option<Self::Node>, fat::Error>;
    fn list(&mut self, dir: &Self::Node) -> Result<alloc::vec::Vec<(alloc::string::String, Self::Node)>, fat::Error>;
    fn read(&mut self, node: &Self::Node, off: u64, out: &mut [u8]) -> Result<usize, fat::Error>;
    fn set_now(&mut self, unix: u64);
    fn create(&mut self, path: &str) -> Result<(), fat::Error>;
    fn append(&mut self, path: &str, off: u64, data: &[u8]) -> Result<(), fat::Error>;
    fn mkdir(&mut self, path: &str) -> Result<(), fat::Error>;
    fn remove(&mut self, path: &str) -> Result<(), fat::Error>;
    fn rename(&mut self, from: &str, to: &str, replace: bool) -> Result<(), fat::Error>;
    fn sync(&mut self) -> Result<(), fat::Error>;
}
