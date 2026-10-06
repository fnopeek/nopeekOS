//! Typed access to device memory, I/O ports, DMA buffers and MSRs.
//!
//! Each type states its safety contract once, in its `unsafe` constructor.
//! After that every access is a safe, bounds-checked method, so a driver
//! needs no `unsafe` per register. An access outside the window panics
//! instead of touching whatever lies behind it.

use crate::paging::{self, PageFlags, PagingError};

const PAGE: u64 = 4096;

/// A register offset; drivers name their registers in different widths.
pub trait Off: Copy {
    fn get(self) -> u64;
}
impl Off for u32 { fn get(self) -> u64 { self as u64 } }
impl Off for u64 { fn get(self) -> u64 { self } }
impl Off for usize { fn get(self) -> u64 { self as u64 } }

/// A window of memory-mapped device registers.
#[derive(Clone, Copy, Debug)]
pub struct Mmio {
    base: u64,
    len: u64,
}

impl Mmio {
    /// An empty window, for statics that are filled at init. Every access
    /// panics.
    pub const fn empty() -> Self {
        Mmio { base: 0, len: 0 }
    }

    /// Map `len` bytes of device registers at physical `phys` uncached,
    /// identity-mapped.
    ///
    /// # Safety
    /// `phys .. phys + len` must be device registers (a BAR or a fixed
    /// device window) that this driver owns, not RAM.
    pub unsafe fn map(phys: u64, len: u64) -> Result<Self, PagingError> {
        let mut off = 0;
        while off < len {
            match paging::map_page(phys + off, phys + off,
                PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_CACHE) {
                Ok(()) | Err(PagingError::AlreadyMapped) => {}
                Err(e) => return Err(e),
            }
            off += PAGE;
        }
        Ok(Mmio { base: phys, len })
    }

    /// A window that is already mapped.
    ///
    /// # Safety
    /// `base .. base + len` must be mapped device registers that stay mapped
    /// for as long as the value is used.
    pub const unsafe fn from_mapped(base: u64, len: u64) -> Self {
        Mmio { base, len }
    }

    pub fn base(&self) -> u64 { self.base }
    pub fn len(&self) -> u64 { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// The part `off .. off + len` of this window.
    pub fn sub(&self, off: impl Off, len: u64) -> Mmio {
        let off = off.get();
        assert!(off.checked_add(len).is_some_and(|e| e <= self.len), "mmio sub out of range");
        Mmio { base: self.base + off, len }
    }

    fn at(&self, off: impl Off, size: u64) -> u64 {
        let off = off.get();
        assert!(off.checked_add(size).is_some_and(|e| e <= self.len),
            "mmio access {:#x}+{} outside a window of {:#x}", off, size, self.len);
        self.base + off
    }

    pub fn r8(&self, off: impl Off) -> u8 {
        // SAFETY: inside the window (`at`), which the constructor's contract
        // makes mapped device memory.
        unsafe { core::ptr::read_volatile(self.at(off, 1) as *const u8) }
    }
    pub fn r16(&self, off: impl Off) -> u16 {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::read_volatile(self.at(off, 2) as *const u16) }
    }
    pub fn r32(&self, off: impl Off) -> u32 {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::read_volatile(self.at(off, 4) as *const u32) }
    }
    pub fn r64(&self, off: impl Off) -> u64 {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::read_volatile(self.at(off, 8) as *const u64) }
    }
    pub fn w8(&self, off: impl Off, v: u8) {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_volatile(self.at(off, 1) as *mut u8, v) }
    }
    pub fn w16(&self, off: impl Off, v: u16) {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_volatile(self.at(off, 2) as *mut u16, v) }
    }
    pub fn w32(&self, off: impl Off, v: u32) {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_volatile(self.at(off, 4) as *mut u32, v) }
    }
    /// One 64-bit access.
    pub fn w64(&self, off: impl Off, v: u64) {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_volatile(self.at(off, 8) as *mut u64, v) }
    }
    /// Two 32-bit reads, low half first, for devices that decode 64-bit
    /// registers as two dwords.
    pub fn r64_lo_hi(&self, off: impl Off) -> u64 {
        let o = off.get();
        let lo = self.r32(o) as u64;
        let hi = self.r32(o + 4) as u64;
        hi << 32 | lo
    }
    /// Two 32-bit writes, low half first.
    pub fn w64_lo_hi(&self, off: impl Off, v: u64) {
        let o = off.get();
        self.w32(o, v as u32);
        self.w32(o + 4, (v >> 32) as u32);
    }
}

/// One I/O port.
#[derive(Clone, Copy, Debug)]
pub struct Port(u16);

impl Port {
    /// # Safety
    /// The port must belong to a device this code drives; an access to a
    /// foreign port can reset or reprogram hardware.
    pub const unsafe fn new(port: u16) -> Self {
        Port(port)
    }

    pub fn inb(&self) -> u8 {
        // SAFETY: the constructor's contract.
        unsafe { crate::serial::inb(self.0) }
    }
    pub fn inw(&self) -> u16 {
        // SAFETY: the constructor's contract.
        unsafe { crate::serial::inw(self.0) }
    }
    pub fn inl(&self) -> u32 {
        // SAFETY: the constructor's contract.
        unsafe { crate::serial::inl(self.0) }
    }
    pub fn outb(&self, v: u8) {
        // SAFETY: the constructor's contract.
        unsafe { crate::serial::outb(self.0, v) }
    }
    pub fn outw(&self, v: u16) {
        // SAFETY: the constructor's contract.
        unsafe { crate::serial::outw(self.0, v) }
    }
    pub fn outl(&self, v: u32) {
        // SAFETY: the constructor's contract.
        unsafe { crate::serial::outl(self.0, v) }
    }
}

/// A block of consecutive I/O ports, e.g. a legacy BAR.
#[derive(Clone, Copy, Debug)]
pub struct PortRange {
    base: u16,
    len: u16,
}

impl PortRange {
    pub const fn empty() -> Self {
        PortRange { base: 0, len: 0 }
    }

    /// # Safety
    /// As for `Port::new`, for every port in `base .. base + len`.
    pub const unsafe fn new(base: u16, len: u16) -> Self {
        PortRange { base, len }
    }

    pub fn base(&self) -> u16 { self.base }

    /// The port at `off`.
    pub fn port(&self, off: u16) -> Port {
        assert!(off < self.len, "port {:#x} outside a range of {:#x}", off, self.len);
        Port(self.base + off)
    }

    pub fn inb(&self, off: u16) -> u8 { self.port(off).inb() }
    pub fn inw(&self, off: u16) -> u16 { self.port(off).inw() }
    pub fn inl(&self, off: u16) -> u32 { self.port(off).inl() }
    pub fn outb(&self, off: u16, v: u8) { self.port(off).outb(v) }
    pub fn outw(&self, off: u16, v: u16) { self.port(off).outw(v) }
    pub fn outl(&self, off: u16, v: u32) { self.port(off).outl(v) }
}

/// A physically contiguous, identity-mapped buffer a device reads or writes.
///
/// Not freed on drop: a device may still access it. `free` is explicit and
/// the caller states that the device is quiet.
#[derive(Clone, Copy, Debug)]
pub struct DmaRegion {
    phys: u64,
    len: u64,
}

impl DmaRegion {
    pub const fn empty() -> Self {
        DmaRegion { phys: 0, len: 0 }
    }

    /// `pages` zeroed pages anywhere in RAM.
    pub fn alloc_zeroed(pages: usize) -> Option<Self> {
        let phys = crate::memory::allocate_contiguous(pages)?;
        let r = DmaRegion { phys, len: pages as u64 * PAGE };
        r.zero();
        Some(r)
    }

    /// `pages` zeroed pages below `limit` (for 32-bit DMA addresses).
    pub fn alloc_zeroed_below(pages: usize, limit: u64) -> Option<Self> {
        let phys = crate::memory::allocate_contiguous_below(pages, limit)?;
        let r = DmaRegion { phys, len: pages as u64 * PAGE };
        r.zero();
        Some(r)
    }

    /// A region allocated elsewhere.
    ///
    /// # Safety
    /// `phys .. phys + len` must be identity-mapped RAM owned by this driver
    /// for as long as the value is used.
    pub const unsafe fn from_raw(phys: u64, len: u64) -> Self {
        DmaRegion { phys, len }
    }

    /// Return the pages to the frame allocator.
    ///
    /// # Safety
    /// No device may access the region any more, and no other copy of this
    /// value may be used afterwards.
    pub unsafe fn free(self) {
        crate::memory::deallocate_contiguous(self.phys, (self.len / PAGE) as usize);
    }

    pub fn phys(&self) -> u64 { self.phys }
    pub fn len(&self) -> u64 { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// The part `off .. off + len` of this region.
    pub fn sub(&self, off: impl Off, len: u64) -> DmaRegion {
        let off = off.get();
        assert!(off.checked_add(len).is_some_and(|e| e <= self.len), "dma sub out of range");
        DmaRegion { phys: self.phys + off, len }
    }

    fn at(&self, off: impl Off, size: u64) -> u64 {
        let off = off.get();
        assert!(off.checked_add(size).is_some_and(|e| e <= self.len),
            "dma access {:#x}+{} outside a region of {:#x}", off, size, self.len);
        self.phys + off
    }

    pub fn r8(&self, off: impl Off) -> u8 {
        // SAFETY: inside the region (`at`), identity-mapped RAM owned by the
        // driver (constructor contract). Volatile: the device writes it too.
        unsafe { core::ptr::read_volatile(self.at(off, 1) as *const u8) }
    }
    pub fn r16(&self, off: impl Off) -> u16 {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::read_volatile(self.at(off, 2) as *const u16) }
    }
    pub fn r32(&self, off: impl Off) -> u32 {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::read_volatile(self.at(off, 4) as *const u32) }
    }
    pub fn r64(&self, off: impl Off) -> u64 {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::read_volatile(self.at(off, 8) as *const u64) }
    }
    pub fn w8(&self, off: impl Off, v: u8) {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_volatile(self.at(off, 1) as *mut u8, v) }
    }
    pub fn w16(&self, off: impl Off, v: u16) {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_volatile(self.at(off, 2) as *mut u16, v) }
    }
    pub fn w32(&self, off: impl Off, v: u32) {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_volatile(self.at(off, 4) as *mut u32, v) }
    }
    pub fn w64(&self, off: impl Off, v: u64) {
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_volatile(self.at(off, 8) as *mut u64, v) }
    }

    /// Copy `src` into the region at `off`.
    pub fn copy_in(&self, off: impl Off, src: &[u8]) {
        let p = self.at(off, src.len() as u64);
        // SAFETY: as in `r8`; `src` is ordinary kernel memory, disjoint.
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), p as *mut u8, src.len()) }
    }

    /// Copy from the region at `off` into `dst`.
    pub fn copy_out(&self, off: impl Off, dst: &mut [u8]) {
        let p = self.at(off, dst.len() as u64);
        // SAFETY: as in `copy_in`.
        unsafe { core::ptr::copy_nonoverlapping(p as *const u8, dst.as_mut_ptr(), dst.len()) }
    }

    /// Set `len` bytes at `off` to `v`.
    pub fn fill(&self, off: impl Off, v: u8, len: u64) {
        let p = self.at(off, len);
        // SAFETY: as in `r8`.
        unsafe { core::ptr::write_bytes(p as *mut u8, v, len as usize) }
    }

    pub fn zero(&self) {
        self.fill(0u64, 0, self.len);
    }
}

/// Read-only bytes at a physical address, e.g. a firmware table.
#[derive(Clone, Copy, Debug)]
pub struct PhysView {
    addr: u64,
    len: u64,
}

impl PhysView {
    /// # Safety
    /// `addr .. addr + len` must be mapped, readable and not change while
    /// the view is used (firmware tables in reserved memory are).
    pub const unsafe fn new(addr: u64, len: u64) -> Self {
        PhysView { addr, len }
    }

    pub fn addr(&self) -> u64 { self.addr }
    pub fn len(&self) -> u64 { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    fn get<T: Copy>(&self, off: impl Off) -> Option<T> {
        let off = off.get();
        let end = off.checked_add(core::mem::size_of::<T>() as u64)?;
        if end > self.len { return None; }
        // SAFETY: inside the view (constructor contract); unaligned is fine
        // for plain integers.
        Some(unsafe { core::ptr::read_unaligned((self.addr + off) as *const T) })
    }

    pub fn u8(&self, off: impl Off) -> Option<u8> { self.get(off) }
    pub fn u16(&self, off: impl Off) -> Option<u16> { self.get(off) }
    pub fn u32(&self, off: impl Off) -> Option<u32> { self.get(off) }
    pub fn u64(&self, off: impl Off) -> Option<u64> { self.get(off) }

    /// The view as a slice.
    pub fn bytes(&self) -> &'static [u8] {
        // SAFETY: the constructor's contract; the memory outlives the kernel.
        unsafe { core::slice::from_raw_parts(self.addr as *const u8, self.len as usize) }
    }

    /// The part `off .. off + len`, or `None` if it leaves the view.
    pub fn sub(&self, off: impl Off, len: u64) -> Option<PhysView> {
        let off = off.get();
        (off.checked_add(len)? <= self.len).then_some(PhysView { addr: self.addr + off, len })
    }
}

/// Model-specific registers.
pub mod msr {
    /// # Safety
    /// `msr` must exist on this CPU (otherwise #GP), and reading it must have
    /// no side effect the caller does not intend.
    pub unsafe fn read(msr: u32) -> u64 {
        let (lo, hi): (u32, u32);
        // SAFETY: the caller's contract.
        unsafe {
            core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
                options(nomem, nostack, preserves_flags));
        }
        (hi as u64) << 32 | lo as u64
    }

    /// # Safety
    /// `msr` must exist and `v` must be a value the CPU accepts; an MSR
    /// write can change paging, interrupts or the CPU's mode.
    pub unsafe fn write(msr: u32, v: u64) {
        // SAFETY: the caller's contract.
        unsafe {
            core::arch::asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
                options(nostack, preserves_flags));
        }
    }
}
