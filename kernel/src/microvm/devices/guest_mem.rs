//! Guest physical memory accessors.
//!
//! `GuestMem` is the single translation point between a guest-physical
//! address and host memory. In B1 the guest RAM window is still one
//! contiguous host range, so translation is `host_phys = base + gpa`.
//! B3 turns this into a scattered, demand-paged lookup *without*
//! touching any caller — every device-side guest access already goes
//! through here. Callers therefore MUST NOT assume linearity or that a
//! multi-page buffer is contiguous in host memory; use the accessors.
//!
//! All accessors bounds-check against `len` (the advertised guest RAM
//! size) so a buggy/malicious guest descriptor can't drag us into
//! kernel memory.
//!
//! `GUEST_RAM_BYTES` is the canonical guest-RAM size. It used to be
//! duplicated in five places (this file, `guest_fetch`,
//! `vmx::ept::GUEST_WINDOW_BYTES`, svm npt window,
//! `bzimage::GUEST_RAM_TOTAL`); a stale copy from the 256 MB→1 GB bump
//! silently turned every virtio DMA / insn-fetch above the stale bound
//! into a no-op (→ SQUASHFS/EIO corruption under a memory-hungry
//! guest). Those four now reference this constant; B2 replaces it with
//! a runtime value chosen at `vm_open`.

#![allow(dead_code)]

/// Canonical guest-RAM size: 1 GiB. Referenced by
/// `vmx::ept::GUEST_WINDOW_BYTES`, `svm::npt::GUEST_WINDOW_BYTES`,
/// `guest_fetch`, and `bzimage` so the size lives in exactly one
/// place. B2 turns the per-VM size into a runtime value; this stays as
/// the default/cap.
pub const GUEST_RAM_BYTES: u64 = 1024 * 1024 * 1024;

/// Translates guest-physical addresses to host memory for one VM.
///
/// B1: `{ base, len }`, linear. B3: gains a scattered gpa→hpa map; the
/// public accessor surface does not change. Not `Copy` — B3 makes it
/// own per-VM mapping state, and a stale base must never be silently
/// duplicated. Threaded as `&GuestMem` everywhere `host_base: u64`
/// used to flow.
pub struct GuestMem {
    base: u64,
    len: u64,
}

impl GuestMem {
    pub fn new(base: u64, len: u64) -> Self {
        Self { base, len }
    }

    /// Advertised guest RAM size in bytes.
    #[inline]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Raw contiguous window base. B1-only escape hatch for window
    /// construction and the substrate self-tests, which write the
    /// guest image before any VM-exit. B3 reworks those few callers;
    /// no device/DMA path may use this.
    #[inline]
    pub fn base(&self) -> u64 {
        self.base
    }

    /// gpa→host translation + bounds check. The single point B3
    /// changes from `base + gpa` to a scattered lookup. Returns the
    /// host address of `gpa`, or `None` if `[gpa, gpa+len)` leaves the
    /// window. In B1 a non-`None` result spans `len` contiguous host
    /// bytes; B3 callers handling >1 page must split per page.
    #[inline]
    fn xlate(&self, gpa: u64, len: u64) -> Option<u64> {
        if gpa.checked_add(len).is_some_and(|end| end <= self.len) {
            Some(self.base + gpa)
        } else {
            None
        }
    }

    pub fn read_u8(&self, gpa: u64) -> Option<u8> {
        let h = self.xlate(gpa, 1)?;
        Some(unsafe { core::ptr::read_volatile(h as *const u8) })
    }

    pub fn read_u16(&self, gpa: u64) -> Option<u16> {
        let h = self.xlate(gpa, 2)?;
        Some(unsafe { core::ptr::read_volatile(h as *const u16) })
    }

    pub fn read_u32(&self, gpa: u64) -> Option<u32> {
        let h = self.xlate(gpa, 4)?;
        Some(unsafe { core::ptr::read_volatile(h as *const u32) })
    }

    pub fn read_u64(&self, gpa: u64) -> Option<u64> {
        let h = self.xlate(gpa, 8)?;
        Some(unsafe { core::ptr::read_volatile(h as *const u64) })
    }

    pub fn write_u8(&self, gpa: u64, val: u8) -> bool {
        match self.xlate(gpa, 1) {
            Some(h) => {
                unsafe { core::ptr::write_volatile(h as *mut u8, val) };
                true
            }
            None => false,
        }
    }

    pub fn write_u16(&self, gpa: u64, val: u16) -> bool {
        match self.xlate(gpa, 2) {
            Some(h) => {
                unsafe { core::ptr::write_volatile(h as *mut u16, val) };
                true
            }
            None => false,
        }
    }

    pub fn write_u32(&self, gpa: u64, val: u32) -> bool {
        match self.xlate(gpa, 4) {
            Some(h) => {
                unsafe { core::ptr::write_volatile(h as *mut u32, val) };
                true
            }
            None => false,
        }
    }

    pub fn read_bytes(&self, gpa: u64, dst: &mut [u8]) -> bool {
        match self.xlate(gpa, dst.len() as u64) {
            Some(h) => {
                unsafe {
                    core::ptr::copy_nonoverlapping(h as *const u8, dst.as_mut_ptr(), dst.len());
                }
                true
            }
            None => false,
        }
    }

    pub fn write_bytes(&self, gpa: u64, src: &[u8]) -> bool {
        match self.xlate(gpa, src.len() as u64) {
            Some(h) => {
                unsafe {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), h as *mut u8, src.len());
                }
                true
            }
            None => false,
        }
    }
}
