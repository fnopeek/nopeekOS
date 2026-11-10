//! Kernel mappings in 4 KiB pages above the identity map, with x86's
//! `mm::paging` interface: the same `PageFlags` meanings, `map_page`,
//! `unmap_page`, `translate`, `sync_tlb`.

use super::mmu::page;
use bitflags::bitflags;

bitflags! {
    /// What a mapping allows; translated to page descriptor bits here.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct PageFlags: u64 {
        const PRESENT       = 1 << 0;
        const WRITABLE      = 1 << 1;
        const USER          = 1 << 2;
        /// Device memory (x86 `PCD`, uncached).
        const NO_CACHE      = 1 << 4;
        const GLOBAL        = 1 << 8;
        /// Normal memory, not cached (x86 write-combining).
        const WRITE_COMBINE = 1 << 9;
        const NO_EXECUTE    = 1 << 63;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagingError {
    NotAligned,
    AlreadyMapped,
    NotMapped,
    FrameAllocationFailed,
    /// Inside the identity map or past 48 bits.
    OutOfRange,
}

impl From<page::Error> for PagingError {
    fn from(e: page::Error) -> Self {
        match e {
            page::Error::OutOfRange => PagingError::OutOfRange,
            page::Error::NoMemory => PagingError::FrameAllocationFailed,
            page::Error::AlreadyMapped => PagingError::AlreadyMapped,
            page::Error::NotMapped => PagingError::NotMapped,
        }
    }
}

fn attrs(flags: PageFlags) -> u64 {
    let mut a = page::PAGE;
    a |= if flags.contains(PageFlags::NO_CACHE) {
        page::ATTR_DEVICE_IDX
    } else if flags.contains(PageFlags::WRITE_COMBINE) {
        page::ATTR_NC_IDX
    } else {
        page::ATTR_NORMAL_IDX
    };
    if !flags.contains(PageFlags::WRITABLE) { a |= page::RO; }
    if flags.contains(PageFlags::USER) { a |= page::EL0; }
    // The kernel never executes from EL0 mappings; NO_EXECUTE also
    // forbids the kernel.
    a |= page::UXN_BIT;
    if flags.contains(PageFlags::NO_EXECUTE) || flags.contains(PageFlags::NO_CACHE) {
        a |= page::PXN_BIT;
    }
    a
}

/// Map the 4 KiB page at `vaddr` to the frame `paddr`.
pub fn map_page(vaddr: u64, paddr: u64, flags: PageFlags) -> Result<(), PagingError> {
    if vaddr % 4096 != 0 || paddr % 4096 != 0 { return Err(PagingError::NotAligned); }
    if !flags.contains(PageFlags::PRESENT) { return Err(PagingError::NotMapped); }
    page::map(vaddr, paddr, attrs(flags)).map_err(Into::into)
}

/// Unmap the page at `vaddr`, on every core; returns the frame.
pub fn unmap_page(vaddr: u64) -> Result<u64, PagingError> {
    page::unmap(vaddr).map_err(Into::into)
}

/// The physical address `vaddr` maps to: itself inside the identity map.
pub fn translate(vaddr: u64) -> Option<u64> {
    if vaddr < super::IDENTITY_MAX { return Some(vaddr); }
    page::translate(vaddr)
}

/// On x86 cores catch up on unmaps lazily; here `unmap_page` broadcasts
/// its TLB invalidation, so there is nothing to do.
pub fn sync_tlb() {}
