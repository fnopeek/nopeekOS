//! Guest physical memory accessors.
//!
//! `GuestMem` is the single translation point between a guest-physical
//! address and host memory. Every device-side guest access goes through
//! here, so callers must not assume linearity or that a multi-page
//! buffer is contiguous in host memory; use the accessors.
//!
//! All accessors bounds-check against `len` (the advertised guest RAM
//! size) so a buggy/malicious guest descriptor can't drag us into
//! kernel memory.
//!
//! `GUEST_RAM_BYTES` is the canonical guest-RAM size; `guest_fetch`,
//! `vmx::ept`, svm `npt` and `bzimage` reference it rather than keeping
//! copies, since a stale copy silently turns every virtio DMA / insn
//! fetch above its bound into a no-op.

#![allow(dead_code)]

extern crate alloc;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use core::ptr;

/// The active microvm's `GuestMem`, held outside `VmShared` so the off-vCPU
/// workers can share `&GuestMem` across cores without a `VmShared` handle.
/// Sound because `GuestMem` is `&self`-only + `Sync` (its only interior
/// mutability is the atomic software TLB). One instance (one microvm at a
/// time); a future multi-VM world keys this per VM.
static ACTIVE_GM: AtomicPtr<GuestMem> = AtomicPtr::new(ptr::null_mut());

/// Install `gm` as the active guest memory and return a `'static` handle to it.
/// The `'static` is self-managed: `clear_active()` frees it at VM close, after
/// all vCPUs + the backend fiber have stopped touching it.
pub fn set_active(gm: GuestMem) -> &'static GuestMem {
    let p = Box::into_raw(Box::new(gm));
    ACTIVE_GM.store(p, Ordering::Release);
    // SAFETY: `p` was just allocated and is non-null; it stays live until
    // `clear_active()` (called only at close, after the VM threads stop).
    unsafe { &*p }
}

/// Frames guest RAM may still take before the host's reserve: the same
/// `KERNEL_RESERVE_MB` a module's `memory.grow` keeps free. Demand paging
/// stops there; the guest then dies on the unbacked page instead of the
/// host running out of frames under its own allocations.
pub fn frames_spare() -> usize {
    let (free, _) = crate::memory::stats();
    let reserve = crate::forge_rt::KERNEL_RESERVE_MB * (1024 * 1024 / crate::memory::PAGE_SIZE);
    let spare = free.saturating_sub(reserve);
    if spare == 0 && !RESERVE_HIT.swap(true, Ordering::Relaxed) {
        crate::kprintln!("[microvm] guest RAM not backed: host memory is down to its reserve");
    }
    spare
}

static RESERVE_HIT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The active guest memory, if a microvm is running. Used by the off-vCPU
/// backend fiber (which has no `VmShared` handle).
pub fn active() -> Option<&'static GuestMem> {
    let p = ACTIVE_GM.load(Ordering::Acquire);
    if p.is_null() { None } else {
        // SAFETY: non-null ⇒ set_active installed a live Box not yet cleared.
        unsafe { Some(&*p) }
    }
}

/// Free the active guest memory at VM close. Idempotent. Caller guarantees no
/// vCPU or backend fiber still holds a handle (teardown order: stop threads →
/// release page tables → clear_active).
pub fn clear_active() {
    let p = ACTIVE_GM.swap(ptr::null_mut(), Ordering::AcqRel);
    if !p.is_null() {
        // SAFETY: `p` came from `Box::into_raw` in set_active and is freed once.
        unsafe { drop(Box::from_raw(p)); }
    }
}

/// Software-TLB size (direct-mapped, power of two). 1024 slots cover a 4 MiB
/// working set of distinct guest pages — far more than the RX/TX buffer pool the
/// net hot path reuses. 8 KiB per VM.
const TLB_SIZE: usize = 1024;

/// Canonical guest-RAM size: 2 GiB. Browsers (Firefox/LibreWolf with
/// e10s + content sandboxing) routinely reach 1 GiB resident with a
/// couple of moderate tabs. With demand paging on, only the 256 MiB
/// boot window is committed contiguously at vm_open; the rest is
/// faulted in 4 KiB at a time as the guest touches it.
pub const GUEST_RAM_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Demand-paging master switch. `false` → the whole guest is one
/// contiguous block (boot window = full guest, no demand PTs, no
/// scatter walk). `true` → the 256 MiB hybrid (contiguous boot
/// window + 4 KiB demand region). A contiguous multi-GiB block is
/// fragile on a small host; demand paging commits only touched pages.
pub const DEMAND_ENABLED: bool = true;

/// Which second-level paging format backs the demand region, so
/// `GuestMem` can fault a page in without pulling in `cpu::Vendor`.
#[derive(Clone, Copy)]
pub enum SecondLevel {
    Ept,
    Npt,
}

/// Serialises demand fault-in (see `page_host`).
static DEMAND_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// Translates guest-physical addresses to host memory for one VM.
///
/// Hybrid: `[0, boot_bytes)` is one contiguous host block
/// (`boot_base`, 2-MB EPT/NPT leaves — fast, fault-free early boot);
/// `[boot_bytes, len)` is demand-paged 4 KB on first touch. The
/// page-table tree is the gpa→host map — `page_host` walks/faults via
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
    /// Software TLB for the demand region: caches the page-table walk
    /// (`demand_fault_in`) that `page_host` would otherwise repeat on every
    /// access. The demand map is stable once faulted (a guest page → one
    /// host frame for the VM's life), so entries never need invalidation.
    /// Each slot is one atomic u64 packing `(page>>12)<<32 | (host>>12)` →
    /// lock-free, no torn reads, safe for concurrent AP-vCPU access.
    /// 0 = empty. RX buffers are reused, so the hit rate is near 100%.
    tlb: [AtomicU64; TLB_SIZE],
}

impl GuestMem {
    pub fn new(
        boot_base: u64,
        boot_bytes: u64,
        len: u64,
        table_root: u64,
        sl: SecondLevel,
    ) -> Self {
        Self {
            boot_base, boot_bytes, len, table_root, sl,
            tlb: [const { AtomicU64::new(0) }; TLB_SIZE],
        }
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
        // Software TLB: the demand map is stable once faulted, so a hit skips the
        // EPT/NPT walk entirely. One atomic load, no lock, no torn read (tag+host
        // packed in a single u64). pn ≥ 65536 here (page ≥ 256 MiB boot window),
        // so a packed entry is never 0 → 0 reliably means "empty".
        let pn = page >> 12;
        let slot = (pn as usize) & (TLB_SIZE - 1);
        let cached = self.tlb[slot].load(Ordering::Relaxed);
        if cached != 0 && (cached >> 32) == pn {
            return Some((cached & 0xFFFF_FFFF) << 12);
        }
        // One fault-in at a time: the vCPUs (#NPF) and the net worker (DMA
        // into an untouched page) can race to the same empty PTE, and two
        // frames for one page lose whatever the loser wrote. The walk
        // re-checks the PTE under the lock, so the second one just reads it.
        let _g = DEMAND_LOCK.lock();
        let host = match self.sl {
            SecondLevel::Ept => {
                crate::microvm::cpu::vmx::ept::demand_fault_in(self.table_root, page)
            }
            SecondLevel::Npt => {
                crate::microvm::cpu::svm::npt::demand_fault_in(self.table_root, page, self.len)
            }
        }?;
        // host is page-aligned (a fresh demand frame), so host>>12<<12 == host.
        self.tlb[slot].store((pn << 32) | (host >> 12), Ordering::Relaxed);
        Some(host)
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

    /// `contig`, and only when `gpa` is aligned to `n`: a guest may place a
    /// ring field at an odd address, and the typed volatile access needs
    /// alignment. Unaligned accesses take the byte copy instead.
    #[inline]
    fn contig_aligned(&self, gpa: u64, n: u64) -> Option<u64> {
        if gpa & (n - 1) != 0 { return None; }
        self.contig(gpa, n)
    }

    pub fn read_u8(&self, gpa: u64) -> Option<u8> {
        let mut b = [0u8; 1];
        if !self.read_bytes(gpa, &mut b) { return None; }
        Some(b[0])
    }

    pub fn read_u16(&self, gpa: u64) -> Option<u16> {
        if let Some(h) = self.contig_aligned(gpa, 2) {
            // SAFETY: `contig_aligned` returned an in-window host address
            // aligned for `u16`; the boot window stays mapped for the VM's life.
            return Some(unsafe { core::ptr::read_volatile(h as *const u16) });
        }
        let mut b = [0u8; 2];
        if !self.read_bytes(gpa, &mut b) { return None; }
        Some(u16::from_le_bytes(b))
    }

    pub fn read_u32(&self, gpa: u64) -> Option<u32> {
        if let Some(h) = self.contig_aligned(gpa, 4) {
            // SAFETY: `contig_aligned` returned an in-window host address
            // aligned for `u32`; the boot window stays mapped for the VM's life.
            return Some(unsafe { core::ptr::read_volatile(h as *const u32) });
        }
        let mut b = [0u8; 4];
        if !self.read_bytes(gpa, &mut b) { return None; }
        Some(u32::from_le_bytes(b))
    }

    pub fn read_u64(&self, gpa: u64) -> Option<u64> {
        if let Some(h) = self.contig_aligned(gpa, 8) {
            // SAFETY: `contig_aligned` returned an in-window host address
            // aligned for `u64`; the boot window stays mapped for the VM's life.
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
        if let Some(h) = self.contig_aligned(gpa, 2) {
            // SAFETY: as in `read_u16`.
            unsafe { core::ptr::write_volatile(h as *mut u16, val) };
            return true;
        }
        self.write_bytes(gpa, &val.to_le_bytes())
    }

    pub fn write_u32(&self, gpa: u64, val: u32) -> bool {
        if let Some(h) = self.contig_aligned(gpa, 4) {
            // SAFETY: as in `read_u32`.
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
            // SAFETY: `contig` checked `[gpa, gpa+len)` lies in the mapped boot
            // window; `dst` is a distinct host buffer.
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
            // SAFETY: `host` is the mapped frame of this guest page and
            // `off + take <= 4096`; `done + take <= dst.len()`.
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
            // SAFETY: as in `read_bytes`.
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
            // SAFETY: as in `read_bytes`.
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
