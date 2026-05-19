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

/// B3 demand-paging master switch. `false` → the whole guest is one
/// contiguous block (boot window = full guest, no demand PTs, no
/// scatter walk): behaviour bit-identical to the validated B2/A2
/// (contiguous). `true` → the 256 MiB hybrid (contiguous boot + 4 KB
/// demand). Default `false` until the demand-path corruption is
/// fixed — the entire B3 architecture stays in the tree; only this
/// flag gates it, so HEAD is never broken while we debug. Flip to
/// `true` for a demand test run.
pub const DEMAND_ENABLED: bool = false;

/// Which second-level paging format backs the demand region, so
/// `GuestMem` can fault a page in without pulling in `cpu::Vendor`.
#[derive(Clone, Copy)]
pub enum SecondLevel {
    Ept,
    Npt,
}

/// Translates guest-physical addresses to host memory for one VM.
///
/// B3 hybrid: `[0, boot_bytes)` is one contiguous host block
/// (`boot_base`, 2-MB EPT/NPT leaves — fast, fault-free early boot);
/// `[boot_bytes, len)` is demand-paged 4 KB on first touch. The
/// page-table tree IS the gpa→host map — `page_host` walks/faults via
/// `ept`/`npt::demand_fault_in`, so a host DMA to a page the guest
/// hasn't touched yet still works (and a guest PT the insn-fetch
/// walker reads simply faults in — no recursion: fault-in is a flat
/// alloc+map). Not `Copy`. Threaded as `&GuestMem`.
pub struct GuestMem {
    boot_base: u64,
    boot_bytes: u64,
    len: u64,
    table_root: u64,
    sl: SecondLevel,
}

impl GuestMem {
    pub fn new(
        boot_base: u64,
        boot_bytes: u64,
        len: u64,
        table_root: u64,
        sl: SecondLevel,
    ) -> Self {
        Self { boot_base, boot_bytes, len, table_root, sl }
    }

    /// Advertised guest RAM size in bytes.
    #[inline]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Host phys of the 4 KB page containing `page` (must be
    /// page-aligned). Boot window → linear; demand region → walk +
    /// fault-in. `None` only if `page` is outside the window.
    #[inline]
    fn page_host(&self, page: u64) -> Option<u64> {
        if page >= self.len {
            return None;
        }
        if page < self.boot_bytes {
            return Some(self.boot_base + page);
        }
        match self.sl {
            SecondLevel::Ept => {
                crate::microvm::cpu::vmx::ept::demand_fault_in(self.table_root, page)
            }
            SecondLevel::Npt => {
                crate::microvm::cpu::svm::npt::demand_fault_in(self.table_root, page)
            }
        }
    }

    /// Fault the 4 KB page containing `gpa` into the demand region
    /// (no-op for the boot window). Called from the EPT-violation /
    /// #NPF handler: `true` → mapped, re-enter the guest; `false` →
    /// `gpa` is outside the window (a real fault → fatal dump).
    pub fn ensure(&self, gpa: u64) -> bool {
        self.page_host(gpa & !0xFFF).is_some()
    }

    /// Fast path: host addr of `[gpa, gpa+n)` iff it lies wholly in
    /// the physically-contiguous boot window (one span, no per-page
    /// walk). The common case for early boot + low virtqueue rings.
    #[inline]
    fn contig(&self, gpa: u64, n: u64) -> Option<u64> {
        if gpa.checked_add(n).is_some_and(|end| end <= self.boot_bytes) {
            Some(self.boot_base + gpa)
        } else {
            None
        }
    }

    pub fn read_u8(&self, gpa: u64) -> Option<u8> {
        let mut b = [0u8; 1];
        if !self.read_bytes(gpa, &mut b) { return None; }
        Some(b[0])
    }

    pub fn read_u16(&self, gpa: u64) -> Option<u16> {
        if let Some(h) = self.contig(gpa, 2) {
            return Some(unsafe { core::ptr::read_volatile(h as *const u16) });
        }
        let mut b = [0u8; 2];
        if !self.read_bytes(gpa, &mut b) { return None; }
        Some(u16::from_le_bytes(b))
    }

    pub fn read_u32(&self, gpa: u64) -> Option<u32> {
        if let Some(h) = self.contig(gpa, 4) {
            return Some(unsafe { core::ptr::read_volatile(h as *const u32) });
        }
        let mut b = [0u8; 4];
        if !self.read_bytes(gpa, &mut b) { return None; }
        Some(u32::from_le_bytes(b))
    }

    pub fn read_u64(&self, gpa: u64) -> Option<u64> {
        if let Some(h) = self.contig(gpa, 8) {
            return Some(unsafe { core::ptr::read_volatile(h as *const u64) });
        }
        let mut b = [0u8; 8];
        if !self.read_bytes(gpa, &mut b) { return None; }
        Some(u64::from_le_bytes(b))
    }

    pub fn write_u8(&self, gpa: u64, val: u8) -> bool {
        self.write_bytes(gpa, &[val])
    }

    pub fn write_u16(&self, gpa: u64, val: u16) -> bool {
        if let Some(h) = self.contig(gpa, 2) {
            unsafe { core::ptr::write_volatile(h as *mut u16, val) };
            return true;
        }
        self.write_bytes(gpa, &val.to_le_bytes())
    }

    pub fn write_u32(&self, gpa: u64, val: u32) -> bool {
        if let Some(h) = self.contig(gpa, 4) {
            unsafe { core::ptr::write_volatile(h as *mut u32, val) };
            return true;
        }
        self.write_bytes(gpa, &val.to_le_bytes())
    }

    /// Per-page scatter copy. The boot window is one contiguous span
    /// (fast memcpy); the demand region is walked page-by-page (each
    /// 4 KB may be a different scattered host frame, faulted in on
    /// first touch). `false` iff `[gpa, gpa+len)` leaves the window.
    pub fn read_bytes(&self, gpa: u64, dst: &mut [u8]) -> bool {
        let total = dst.len() as u64;
        if let Some(h) = self.contig(gpa, total) {
            unsafe { core::ptr::copy_nonoverlapping(h as *const u8, dst.as_mut_ptr(), dst.len()) };
            return true;
        }
        if !gpa.checked_add(total).is_some_and(|end| end <= self.len) {
            return false;
        }
        let mut done: usize = 0;
        while (done as u64) < total {
            let cur = gpa + done as u64;
            let page = cur & !0xFFF;
            let off = (cur & 0xFFF) as usize;
            let host = match self.page_host(page) {
                Some(h) => h,
                None => return false,
            };
            let take = core::cmp::min(total as usize - done, 4096 - off);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (host + off as u64) as *const u8,
                    dst.as_mut_ptr().add(done),
                    take,
                );
            }
            done += take;
        }
        true
    }

    pub fn write_bytes(&self, gpa: u64, src: &[u8]) -> bool {
        let total = src.len() as u64;
        if let Some(h) = self.contig(gpa, total) {
            unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), h as *mut u8, src.len()) };
            return true;
        }
        if !gpa.checked_add(total).is_some_and(|end| end <= self.len) {
            return false;
        }
        let mut done: usize = 0;
        while (done as u64) < total {
            let cur = gpa + done as u64;
            let page = cur & !0xFFF;
            let off = (cur & 0xFFF) as usize;
            let host = match self.page_host(page) {
                Some(h) => h,
                None => return false,
            };
            let take = core::cmp::min(total as usize - done, 4096 - off);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr().add(done),
                    (host + off as u64) as *mut u8,
                    take,
                );
            }
            done += take;
        }
        true
    }
}
