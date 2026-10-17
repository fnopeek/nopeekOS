//! Guest physical memory accessors.
//!
//! `GuestMem` is the single translation point between a guest-physical
//! address and host memory. Every device-side guest access goes through
//! here, so callers must not assume linearity or that a multi-page
//! buffer is contiguous in host memory; use the accessors.
//!
//! All accessors bounds-check against the RAM layout (`RamLayout`) so a
//! buggy/malicious guest descriptor can't drag us into kernel memory.
//!
//! `RamLayout` is the one description of where guest RAM lies; `vmx::ept`,
//! svm `npt`, `bzimage` and `hostmem` derive from it rather than keeping
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

const ONE_GB: u64 = 1 << 30;

/// Guest RAM below the MMIO hole ends here at most; [3 GiB, 4 GiB) holds
/// the virtio BARs, IOAPIC, HPET and LAPIC.
pub const LOW_RAM_MAX: u64 = 3 * ONE_GB;
/// The rest of a larger guest starts here (QEMU's above-4G RAM).
pub const HIGH_RAM_BASE: u64 = 4 * ONE_GB;

/// Where a guest of `total` bytes of RAM has it: `[0, low)` and, past the
/// hole, `[HIGH_RAM_BASE, high_end)` — QEMU's below_4g/above_4g split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RamLayout {
    pub low: u64,
    pub high_end: u64,
}

impl RamLayout {
    pub fn new(total: u64) -> Self {
        let low = total.min(LOW_RAM_MAX);
        Self { low, high_end: HIGH_RAM_BASE + (total - low) }
    }

    /// RAM above the hole, in bytes.
    pub fn high(&self) -> u64 {
        self.high_end - HIGH_RAM_BASE
    }

    pub fn total(&self) -> u64 {
        self.low + self.high()
    }

    /// One past the highest RAM address.
    pub fn top(&self) -> u64 {
        if self.high() == 0 { self.low } else { self.high_end }
    }

    pub fn contains(&self, gpa: u64) -> bool {
        gpa < self.low || (HIGH_RAM_BASE..self.high_end).contains(&gpa)
    }

    /// `[gpa, gpa + n)` lies in one RAM region; a range across the hole
    /// is no buffer of the guest.
    pub fn contains_range(&self, gpa: u64, n: u64) -> bool {
        match gpa.checked_add(n) {
            Some(end) => end <= self.low || (gpa >= HIGH_RAM_BASE && end <= self.high_end),
            None => false,
        }
    }

    /// The 1-GiB PDPT slots that hold RAM, low ones first.
    pub fn pdpt_slots(&self) -> impl Iterator<Item = usize> {
        let low = self.low.div_ceil(ONE_GB) as usize;
        let first_high = (HIGH_RAM_BASE / ONE_GB) as usize;
        (0..low).chain(first_high..first_high + self.high().div_ceil(ONE_GB) as usize)
    }
}

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

/// Serialises `reclaim` and `pin_page`: a frame is pinned only while it is
/// mapped, and reclaim decides with the pins it sees.
static RECLAIM_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// Host frames of guest pages the host keeps using beyond one access (a
/// guest blob the GPU reads), with their count: the page refcount KVM's
/// free page reporting respects. `reclaim` leaves them mapped.
static PINS: spin::Mutex<alloc::collections::BTreeMap<u64, u32>> =
    spin::Mutex::new(alloc::collections::BTreeMap::new());

/// Guest pages given back to the host by free page reporting, this boot.
pub static RECLAIMED_PAGES: AtomicU64 = AtomicU64::new(0);

/// Drop one pin of each of `frames` (`GuestMem::pin_page`).
pub fn unpin_frames(frames: &[u64]) {
    let mut pins = PINS.lock();
    for f in frames {
        if let Some(n) = pins.get_mut(f) {
            *n -= 1;
            if *n == 0 {
                pins.remove(f);
            }
        }
    }
}

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
    layout: RamLayout,
    table_root: u64,
    sl: SecondLevel,
    /// Software TLB for the demand region: caches the page-table walk
    /// (`demand_fault_in`) that `page_host` would otherwise repeat on every
    /// access. A guest page keeps its frame until free page reporting takes
    /// it back (`reclaim`, which clears its slot).
    /// Each slot is one atomic u64 packing `(page>>12)<<32 | (host>>12)` →
    /// lock-free, no torn reads, safe for concurrent AP-vCPU access.
    /// 0 = empty. RX buffers are reused, so the hit rate is near 100%.
    tlb: [AtomicU64; TLB_SIZE],
    /// Host accesses through a demand-region translation, counted per
    /// epoch: `reclaim` frees a frame only after every access that may
    /// have translated it before its entries went has ended.
    epoch: AtomicU64,
    accessing: [core::sync::atomic::AtomicU32; 2],
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
            boot_base, boot_bytes, len, layout: RamLayout::new(len), table_root, sl,
            tlb: [const { AtomicU64::new(0) }; TLB_SIZE],
            epoch: AtomicU64::new(0),
            accessing: [const { core::sync::atomic::AtomicU32::new(0) }; 2],
        }
    }

    /// A host access through demand-region translations begins; `leave`
    /// with the returned index when it is over.
    fn enter(&self) -> usize {
        let i = (self.epoch.load(Ordering::SeqCst) & 1) as usize;
        self.accessing[i].fetch_add(1, Ordering::SeqCst);
        i
    }

    fn leave(&self, i: usize) {
        self.accessing[i].fetch_sub(1, Ordering::SeqCst);
    }

    /// Free page reporting (virtio-balloon `F_REPORTING`): the guest no
    /// longer uses `[gpa, gpa + len)`. Its demand pages go back to the host
    /// and fault in zeroed on the next touch, as `MADV_DONTNEED` does under
    /// KVM. The boot window stays (one contiguous host block, freed whole
    /// with the VM), and so do pinned frames. The frames are freed once no
    /// vCPU and no host access can still reach them. Returns the pages
    /// given back.
    pub fn reclaim(&self, gpa: u64, len: u64) -> u64 {
        let _r = RECLAIM_LOCK.lock();
        let mut freed = alloc::vec::Vec::new();
        let taken = {
            let _g = DEMAND_LOCK.lock();
            let pins = PINS.lock();
            let pinned = |f: u64| pins.contains_key(&f);
            let taken = match self.sl {
                SecondLevel::Ept => crate::microvm::cpu::vmx::ept::demand_reclaim(
                    self.table_root, gpa, len, self.len, &pinned, &mut freed),
                SecondLevel::Npt => crate::microvm::cpu::svm::npt::demand_reclaim(
                    self.table_root, gpa, len, self.len, &pinned, &mut freed),
            };
            if taken != 0 {
                let (first, last) = (gpa >> 12, gpa.saturating_add(len) >> 12);
                for slot in &self.tlb {
                    let pn = slot.load(Ordering::Relaxed) >> 32;
                    if (first..last).contains(&pn) {
                        slot.store(0, Ordering::Relaxed);
                    }
                }
            }
            taken
        };
        if freed.is_empty() {
            return taken;
        }
        crate::microvm::devices::hostmem::flush_guest_translations();
        // Accesses that began before the entries went counted in the old
        // epoch; later ones translate afresh.
        let old = (self.epoch.fetch_add(1, Ordering::SeqCst) & 1) as usize;
        while self.accessing[old].load(Ordering::SeqCst) != 0 {
            core::hint::spin_loop();
        }
        for f in freed {
            crate::memory::deallocate_frame(f);
        }
        RECLAIMED_PAGES.fetch_add(taken, Ordering::Relaxed);
        taken
    }

    /// `host_page`, and the frame stays the guest page's until the pin is
    /// dropped (`unpin_frames`): free page reporting will not take it.
    pub fn pin_page(&self, gpa: u64) -> Option<u64> {
        let _r = RECLAIM_LOCK.lock();
        let host = self.host_page(gpa)?;
        if gpa >= self.boot_bytes {
            *PINS.lock().entry(host).or_insert(0) += 1;
        }
        Some(host)
    }

    /// Advertised guest RAM size in bytes.
    #[inline]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Where that RAM lies in guest-physical space.
    #[inline]
    pub fn layout(&self) -> RamLayout {
        self.layout
    }

    /// Host phys of the 4 KB page containing `page` (must be
    /// page-aligned). Boot window → linear; demand region → walk +
    /// fault-in. `None` only if `page` is outside the window.
    #[inline]
    fn page_host(&self, page: u64) -> Option<u64> {
        if !self.layout.contains(page) {
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
                crate::microvm::cpu::vmx::ept::demand_fault_in(self.table_root, page, self.len)
            }
            SecondLevel::Npt => {
                crate::microvm::cpu::svm::npt::demand_fault_in(self.table_root, page, self.len)
            }
        }?;
        // host is page-aligned (a fresh demand frame), so host>>12<<12 == host.
        self.tlb[slot].store((pn << 32) | (host >> 12), Ordering::Relaxed);
        Some(host)
    }

    /// The second-level paging format and its root (PML4), for the
    /// host-visible window (`hostmem`).
    pub fn second_level(&self) -> (SecondLevel, u64) {
        (self.sl, self.table_root)
    }

    /// Host physical address of the guest page at `gpa` (page-aligned),
    /// faulted in if the guest has not touched it. The frame belongs to the
    /// VM until its memory is freed at close.
    pub fn host_page(&self, gpa: u64) -> Option<u64> {
        if gpa & 0xFFF != 0 {
            return None;
        }
        self.page_host(gpa)
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

    /// Atomic exchange of one guest byte, against the guest's own atomics on
    /// it from another vCPU.
    pub fn xchg_u8(&self, gpa: u64, val: u8) -> Option<u8> {
        if !self.layout.contains(gpa) {
            return None;
        }
        let e = self.enter();
        let r = self.page_host(gpa & !0xFFF).map(|h| {
            let host = h + (gpa & 0xFFF);
            // SAFETY: `host` lies in the mapped frame of this guest page,
            // which `reclaim` does not free while the access counts; a byte
            // has no alignment.
            let a = unsafe { core::sync::atomic::AtomicU8::from_ptr(host as *mut u8) };
            a.swap(val, Ordering::AcqRel)
        });
        self.leave(e);
        r
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
    /// first touch). `false` iff `[gpa, gpa+len)` is not within one RAM region.
    pub fn read_bytes(&self, gpa: u64, dst: &mut [u8]) -> bool {
        let total = dst.len() as u64;
        if let Some(h) = self.contig(gpa, total) {
            // SAFETY: `contig` checked `[gpa, gpa+len)` lies in the mapped boot
            // window; `dst` is a distinct host buffer.
            unsafe { core::ptr::copy_nonoverlapping(h as *const u8, dst.as_mut_ptr(), dst.len()) };
            return true;
        }
        if !self.layout.contains_range(gpa, total) {
            return false;
        }
        let e = self.enter();
        let mut done: usize = 0;
        while (done as u64) < total {
            let cur = gpa + done as u64;
            let page = cur & !0xFFF;
            let off = (cur & 0xFFF) as usize;
            let host = match self.page_host(page) {
                Some(h) => h,
                None => {
                    self.leave(e);
                    return false;
                }
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
        self.leave(e);
        true
    }

    pub fn write_bytes(&self, gpa: u64, src: &[u8]) -> bool {
        let total = src.len() as u64;
        if let Some(h) = self.contig(gpa, total) {
            // SAFETY: as in `read_bytes`.
            unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), h as *mut u8, src.len()) };
            return true;
        }
        if !self.layout.contains_range(gpa, total) {
            return false;
        }
        let e = self.enter();
        let mut done: usize = 0;
        while (done as u64) < total {
            let cur = gpa + done as u64;
            let page = cur & !0xFFF;
            let off = (cur & 0xFFF) as usize;
            let host = match self.page_host(page) {
                Some(h) => h,
                None => {
                    self.leave(e);
                    return false;
                }
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
        self.leave(e);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_splits_at_the_hole() {
        let small = RamLayout::new(2 * ONE_GB);
        assert_eq!((small.low, small.high(), small.top()), (2 * ONE_GB, 0, 2 * ONE_GB));
        assert!(!small.contains(HIGH_RAM_BASE));
        assert_eq!(small.pdpt_slots().collect::<alloc::vec::Vec<_>>(), [0, 1]);

        let big = RamLayout::new(8 * ONE_GB);
        assert_eq!((big.low, big.high(), big.top()), (3 * ONE_GB, 5 * ONE_GB, 9 * ONE_GB));
        assert!(big.contains(3 * ONE_GB - 1) && !big.contains(3 * ONE_GB));
        assert!(!big.contains(4 * ONE_GB - 1) && big.contains(4 * ONE_GB));
        assert!(!big.contains(9 * ONE_GB));
        assert!(!big.contains_range(3 * ONE_GB - 4096, 8192));
        assert!(big.contains_range(4 * ONE_GB, 4096));
        assert_eq!(big.pdpt_slots().collect::<alloc::vec::Vec<_>>(), [0, 1, 2, 4, 5, 6, 7, 8]);
        assert_eq!(big.total(), 8 * ONE_GB);
    }
}
