//! Nested Page Tables — AMD's equivalent of Intel EPT.
//!
//! Reference: AMD64 APM Vol. 2 §15.25 "Nested Paging".
//!
//! NPT uses the standard 4-level x86_64 page-table format (PML4 →
//! PDPT → PD → PT), unlike EPT which has its own permission bit
//! layout. That makes NPT noticeably easier to bring up: the same
//! page-table walker the kernel uses for host paging would, in
//! principle, work for guest NPT too.
//!
//! Two modes are used by the SVM backend:
//!
//! * `allocate_identity_npt()` — guest window → host, identity.
//!   Used by the substrate test where the guest stub lives wherever
//!   the frame allocator hands it out. Only safe when the stub
//!   happens to fall inside the window.
//!
//! * `allocate_window_npt(host_base)` — guest RAM → host
//!   `host_base..`. Used by `run_linux` so the guest can write freely at
//!   GPA 0x10000/0x90000/0x100000/... without stomping on the host
//!   kernel image at the same host addresses. Mirrors
//!   `vmx::ept::install_window`.
//!
//! The boot window uses 2 MB pages; 1 GB pages are unreliable under KVM
//! nested SVM (exit code 0).
//!
//! `allocate_window_npt` additionally maps the high MMIO region
//! [0xFEC00000, 0xFF000000) (IOAPIC + HPET + LAPIC) to an aliased
//! scratch page, except the IOAPIC and LAPIC pages, which stay
//! not-present so accesses fault into the emulators.

use crate::mm::memory;
use crate::microvm::devices::guest_mem::RamLayout;

/// Slack frames `boot_frames_for` adds so `allocate_contiguous` can be
/// rounded up to a 2 MB boundary. Mirrors `ept`.
const GUEST_RAM_ALIGN_SLACK: usize = 511;

/// Bits [51:12] of an NPT entry hold the next-level / page phys addr.
const NPT_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
/// Bits [51:21] of a 2 MB leaf hold the page's phys addr.
const NPT_2M_ADDR_MASK: u64 = 0x000F_FFFF_FFE0_0000;
/// Frames in one 2 MB page.
const FRAMES_PER_2M: usize = (TWO_MB / 4096) as usize;

/// Demand blocks mapped as one 2 MB leaf, and those that fell back to a PT
/// of 4 KB pages (no free 2 MB-aligned host block).
pub static DEMAND_2M: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static DEMAND_4K_BLOCKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

const TWO_MB: u64 = 2 * 1024 * 1024;
const ONE_GB: u64 = 1024 * 1024 * 1024;
/// PDPT slot of the MMIO hole [3 GiB, 4 GiB). Mirrors `ept`.
const MMIO_PDPT_SLOT: usize = 3;
/// Guest physical the substrate test maps identity.
const SUBSTRATE_BYTES: u64 = 2 * ONE_GB;

// ── NPT page-table flags ───────────────────────────────────────────

/// Present.
const NPT_P: u64 = 1 << 0;
/// Writable.
const NPT_RW: u64 = 1 << 1;
/// User-mode accessible. Must be set in NPT entries — otherwise the
/// CPU treats the page as kernel-only, and any guest access NPT-
/// faults with a permission mismatch (APM §15.25.6).
const NPT_US: u64 = 1 << 2;
/// Page Size — leaf entries at PD level (2 MB pages). Cleared at
/// PML4 + PDPT (those point to the next level).
const NPT_PS: u64 = 1 << 7;

/// Contiguous boot window; mirrors `ept`. See `ept::BOOT_WINDOW_BYTES`.
pub const BOOT_WINDOW_BYTES: u64 = 256 * 1024 * 1024;

/// Round a raw `allocate_contiguous` base up to the next 2 MB
/// boundary so it can be passed to `allocate_window_npt`.
pub fn round_up_to_2mb(raw_base: u64) -> u64 {
    (raw_base + (TWO_MB - 1)) & !(TWO_MB - 1)
}

/// Contiguous boot window size for `guest_bytes`; mirrors `ept`.
pub fn boot_window_bytes(guest_bytes: u64) -> u64 {
    if crate::microvm::devices::guest_mem::DEMAND_ENABLED {
        guest_bytes.min(BOOT_WINDOW_BYTES)
    } else {
        // Demand off → whole guest contiguous.
        guest_bytes
    }
}

/// 4 KB host frames to allocate contiguously for the boot window
/// (+ 2-MB-align slack). The demand region is faulted 4 KB at a time;
/// mirrors `ept::boot_frames_for`.
pub fn boot_frames_for(guest_bytes: u64) -> usize {
    (boot_window_bytes(guest_bytes) / 4096) as usize + GUEST_RAM_ALIGN_SLACK
}

/// Build a fresh NPT root that identity-maps `SUBSTRATE_BYTES` of guest
/// physical to host physical via 2 MB pages. Returns the physical
/// address of the PML4 page, suitable for VMCB.NCR3.
///
/// Allocates 3 frames per call (PML4 + PDPT + PD). Frames are leaked
/// alongside the rest of the per-call substrate-test allocations.
pub fn allocate_identity_npt() -> Result<u64, &'static str> {
    // Substrate test: the whole guest window.
    build_npt(0, SUBSTRATE_BYTES, /* with_mmio_scratch */ false)
}

/// Build a fresh NPT root that maps guest RAM to host physical
/// `host_base..` (contiguous boot window + 4 KB demand region), plus a
/// scratch alias for [0xFEC00000, 0xFF000000) (IOAPIC + HPET + LAPIC).
/// Returns NCR3.
///
/// `host_base` must be 2 MB aligned.
pub fn allocate_window_npt(host_base: u64, guest_bytes: u64) -> Result<u64, &'static str> {
    if host_base & (TWO_MB - 1) != 0 {
        return Err("NPT: host_base must be 2 MB aligned");
    }
    build_npt(host_base, guest_bytes, /* with_mmio_scratch */ true)
}

/// Inner builder. `host_base = 0` gives identity mapping; non-zero
/// shifts the leaf addresses by `host_base`. Maps the RAM of
/// `RamLayout::new(guest_bytes)`, one PD per GiB.
fn build_npt(host_base: u64, guest_bytes: u64, with_mmio_scratch: bool) -> Result<u64, &'static str> {
    if guest_bytes == 0 || guest_bytes & (TWO_MB - 1) != 0 {
        return Err("NPT: guest_bytes must be a non-zero multiple of 2 MB");
    }
    let layout = RamLayout::new(guest_bytes);
    if with_mmio_scratch
        && crate::microvm::devices::hostmem::pdpt_slots().start < layout.top().div_ceil(ONE_GB) as usize
    {
        return Err("NPT: guest RAM overlaps the host-visible window");
    }
    let guest_leaves = (guest_bytes / TWO_MB) as u64;
    // Identity substrate test (host_base = 0) stays all-2-MB; the real
    // window path is hybrid (contiguous boot + 4-KB demand region).
    let boot_leaves = if host_base == 0 {
        guest_leaves
    } else {
        boot_window_bytes(guest_bytes) / TWO_MB
    };
    let slots: alloc::vec::Vec<usize> = layout.pdpt_slots().collect();
    let num_pds = slots.len();

    // Fixed tables: PML4, PDPT, the guest-RAM PDs, and with the MMIO
    // scratch PD_HIGH, PT_DUMMY, PT_LAPIC and the scratch page. All or none.
    let nfixed = 2 + num_pds + if with_mmio_scratch { 4 } else { 0 };
    let mut fixed = alloc::vec![0u64; nfixed];
    for i in 0..nfixed {
        match memory::allocate_frame() {
            Some(f) => fixed[i] = f,
            None => {
                for &f in &fixed[..i] { memory::deallocate_frame(f); }
                return Err("OOM allocating NPT tables");
            }
        }
    }
    let pml4_phys = fixed[0];
    let pdpt_phys = fixed[1];
    let pd_physs = &fixed[2..2 + num_pds];

    // SAFETY: freshly allocated, identity-mapped (host paging),
    // exclusive. All pages are 4 KB aligned (frame allocator
    // guarantee).
    unsafe {
        for &f in &fixed[..nfixed] {
            core::ptr::write_bytes(f as *mut u8, 0, 4096);
        }

        // PML4[0] → PDPT (non-leaf, no PS bit).
        let pml4 = pml4_phys as *mut u64;
        pml4.write_volatile(pdpt_phys | NPT_P | NPT_RW | NPT_US);

        // The RAM slots → guest-RAM PDs (each covers 1 GiB).
        let pdpt = pdpt_phys as *mut u64;
        for (i, &p) in slots.iter().enumerate() {
            pdpt.add(p).write_volatile(pd_physs[i] | NPT_P | NPT_RW | NPT_US);
        }

        if with_mmio_scratch {
            let m = 2 + num_pds;
            let (pd_high_phys, pt_dummy_phys, pt_lapic_phys, dummy_page_phys) =
                (fixed[m], fixed[m + 1], fixed[m + 2], fixed[m + 3]);

            // PDPT[3] → PD_HIGH (covers [3 GB, 4 GB), MMIO range).
            pdpt.add(MMIO_PDPT_SLOT).write_volatile(pd_high_phys | NPT_P | NPT_RW | NPT_US);

            // PD_HIGH[502] → PT_DUMMY (covers [0xFEC00000, 0xFEE00000):
            // IOAPIC/HPET). PD_HIGH[503] → PT_LAPIC (covers
            // [0xFEE00000, 0xFF000000): the local APIC). Separate PTs so
            // we can punch a single not-present page at the LAPIC base.
            let pd_high = pd_high_phys as *mut u64;
            pd_high.add(502).write_volatile(pt_dummy_phys | NPT_P | NPT_RW | NPT_US);
            pd_high.add(503).write_volatile(pt_lapic_phys | NPT_P | NPT_RW | NPT_US);

            // PT_DUMMY[0..512] all → dummy_page. 2 MB → 4 KB scratch.
            let pt_dummy = pt_dummy_phys as *mut u64;
            for i in 0..512usize {
                pt_dummy.add(i).write_volatile(dummy_page_phys | NPT_P | NPT_RW | NPT_US);
            }
            // [0]: the I/O APIC page (0xFEC00000) not present → trap and
            // emulate (`devices::ioapic`), like the LAPIC page below.
            pt_dummy.add(0).write_volatile(0);
            // PT_LAPIC: entry [0] = the LAPIC MMIO page (0xFEE00000) left
            // not present → guest LAPIC accesses #NPF → trap-and-emulate
            // (svm::lapic). The rest of the 2 MB →
            // dummy scratch (harmless if ever touched).
            let pt_lapic = pt_lapic_phys as *mut u64;
            pt_lapic.add(0).write_volatile(0); // LAPIC page: trap on access
            for i in 1..512usize {
                pt_lapic.add(i).write_volatile(dummy_page_phys | NPT_P | NPT_RW | NPT_US);
            }
        }

        // Populate each PD: the boot window as 2 MB leaves. The demand region
        // stays not present (the PDs are zeroed); its first touch installs
        // either a 2 MB leaf or a PT (`demand_fault_in`). Either is a
        // not-present -> present change, which needs no TLB flush, so a
        // present entry is never replaced.
        const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512
        for (i, &p) in slots.iter().enumerate() {
            let pd = pd_physs[i] as *mut u64;
            let base_leaf = (p as u64) * LEAVES_PER_PD;
            let end_leaf = ((p as u64 + 1) * LEAVES_PER_PD).min(boot_leaves);
            for leaf in base_leaf..end_leaf {
                let local = (leaf - base_leaf) as usize;
                let host_target = host_base + leaf * TWO_MB;
                let entry = host_target | NPT_P | NPT_RW | NPT_US | NPT_PS;
                pd.add(local).write_volatile(entry);
            }
        }
    }

    Ok(pml4_phys)
}

/// Back the 4 KB guest page at `gpa` (demand region of a guest of
/// `guest_bytes`) and return its host address. The first touch of a 2 MB
/// block maps the whole block as one 2 MB leaf from a 2 MB-aligned host
/// block (one allocation, one TLB entry for the guest, like a THP-backed
/// KVM memslot); without such a block it falls back to a PT of 4 KB pages.
/// `None` outside the demand region or without host memory to spare.
pub fn demand_fault_in(pml4_phys: u64, gpa: u64, guest_bytes: u64) -> Option<u64> {
    if gpa < boot_window_bytes(guest_bytes) || !RamLayout::new(guest_bytes).contains(gpa) {
        return None;
    }
    // SAFETY: our own freshly-built NPT, identity-mapped tables; `gpa` lies in
    // the demand region, whose PDs exist (`build_npt` maps one per GiB).
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt = (pml4.read_volatile() & NPT_ADDR_MASK) as *const u64;
        // `gpa` is RAM, so not the MMIO hole.
        let pdpt_idx = (gpa / ONE_GB) as usize;
        let pdpt_entry = pdpt.add(pdpt_idx).read_volatile();
        if pdpt_entry == 0 {
            return None;
        }
        let pd = (pdpt_entry & NPT_ADDR_MASK) as *mut u64;
        let pd_idx = ((gpa % ONE_GB) / TWO_MB) as usize;
        let in_block = gpa % TWO_MB & !0xFFF;
        let pde = pd.add(pd_idx).read_volatile();
        if pde & NPT_P != 0 && pde & NPT_PS != 0 {
            return Some((pde & NPT_2M_ADDR_MASK) + in_block); // already a 2 MB leaf
        }
        let pt = if pde & NPT_P != 0 {
            (pde & NPT_ADDR_MASK) as *mut u64
        } else {
            let spare = crate::microvm::devices::guest_mem::frames_spare();
            if spare > FRAMES_PER_2M {
                if let Some(block) = memory::allocate_contiguous_aligned(FRAMES_PER_2M, FRAMES_PER_2M) {
                    core::ptr::write_bytes(block as *mut u8, 0, TWO_MB as usize);
                    pd.add(pd_idx).write_volatile(block | NPT_P | NPT_RW | NPT_US | NPT_PS);
                    DEMAND_2M.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    return Some(block + in_block);
                }
            }
            if spare < 2 {
                return None;
            }
            let pt = memory::allocate_frame()?;
            core::ptr::write_bytes(pt as *mut u8, 0, 4096);
            pd.add(pd_idx).write_volatile(pt | NPT_P | NPT_RW | NPT_US);
            DEMAND_4K_BLOCKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            pt as *mut u64
        };
        let pt_idx = (in_block / 4096) as usize;
        let pte = pt.add(pt_idx).read_volatile();
        if pte & NPT_P != 0 {
            return Some(pte & NPT_ADDR_MASK); // already faulted in
        }
        let spare = crate::microvm::devices::guest_mem::frames_spare();
        if spare == 0 {
            return None;
        }
        let frame = memory::allocate_frame()?;
        core::ptr::write_bytes(frame as *mut u8, 0, 4096);
        pt.add(pt_idx)
            .write_volatile(frame | NPT_P | NPT_RW | NPT_US);
        // Fault-around: back the rest of this 2 MB block now, so a guest
        // walking fresh memory takes one exit per 2 MB, not one per 4 KB.
        // Stops at the host reserve; the remaining pages fault in singly.
        let mut left = spare - 1;
        for j in 0..512usize {
            if left == 0 { break; }
            if j == pt_idx || pt.add(j).read_volatile() & NPT_P != 0 { continue; }
            left -= 1;
            let Some(f) = memory::allocate_frame() else { break };
            core::ptr::write_bytes(f as *mut u8, 0, 4096);
            pt.add(j).write_volatile(f | NPT_P | NPT_RW | NPT_US);
        }
        Some(frame)
    }
}

/// Free page reporting: take the demand pages of `[gpa, gpa + len)` out of
/// the guest. Their entries go not present and their frames, with every
/// page table left empty, are pushed to `freed`; the caller frees them once
/// no translation of them is left, and the next touch faults in a zeroed
/// frame again. A 2-MB leaf covered in part, or holding a page `pinned`
/// says the host still reaches, is split into 4 KB pages first. Pages
/// outside the demand region, unbacked or pinned stay. Returns the pages
/// taken. The caller serializes with fault-in.
pub fn demand_reclaim(pml4_phys: u64, gpa: u64, len: u64, guest_bytes: u64, pinned: &dyn Fn(u64) -> bool,
                      freed: &mut alloc::vec::Vec<u64>) -> u64 {
    use core::sync::atomic::Ordering::Relaxed;
    let layout = RamLayout::new(guest_bytes);
    let start = gpa.max(boot_window_bytes(guest_bytes)).next_multiple_of(4096);
    let end = gpa.saturating_add(len).min(layout.top()) & !0xFFF;
    let mut taken = 0;
    let mut block = start & !(TWO_MB - 1);
    // SAFETY: our own NPT, identity-mapped tables; the caller holds the
    // fault-in lock, so no entry changes under us.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt = (pml4.read_volatile() & NPT_ADDR_MASK) as *const u64;
        while block < end {
            let (lo, hi) = (start.max(block), end.min(block + TWO_MB));
            if !layout.contains(block) {
                block += TWO_MB;
                continue;
            }
            let pdpt_idx = (block / ONE_GB) as usize;
            let pdpt_entry = pdpt.add(pdpt_idx).read_volatile();
            let pd = (pdpt_entry & NPT_ADDR_MASK) as *mut u64;
            let slot = if pdpt_entry == 0 { None } else { Some(pd.add(((block % ONE_GB) / TWO_MB) as usize)) };
            block += TWO_MB;
            let Some(slot) = slot else { continue };
            let pde = slot.read_volatile();
            if pde & NPT_P == 0 {
                continue;
            }
            if pde & NPT_PS != 0 {
                let host = pde & NPT_2M_ADDR_MASK;
                let whole = lo == block - TWO_MB && hi == block;
                if whole && !(0..512u64).any(|j| pinned(host + j * 4096)) {
                    slot.write_volatile(0);
                    freed.extend((0..512u64).map(|j| host + j * 4096));
                    taken += 512;
                    DEMAND_2M.fetch_sub(1, Relaxed);
                    continue;
                }
                let Some(pt) = memory::allocate_frame() else { continue };
                let t = pt as *mut u64;
                for j in 0..512u64 {
                    t.add(j as usize).write_volatile((host + j * 4096) | NPT_P | NPT_RW | NPT_US);
                }
                slot.write_volatile(pt | NPT_P | NPT_RW | NPT_US);
                DEMAND_2M.fetch_sub(1, Relaxed);
                DEMAND_4K_BLOCKS.fetch_add(1, Relaxed);
            }
            let pt_phys = slot.read_volatile() & NPT_ADDR_MASK;
            let pt = pt_phys as *mut u64;
            let mut p = lo;
            while p < hi {
                let e = pt.add(((p % TWO_MB) / 4096) as usize);
                let pte = e.read_volatile();
                if pte & NPT_P != 0 && !pinned(pte & NPT_ADDR_MASK) {
                    e.write_volatile(0);
                    freed.push(pte & NPT_ADDR_MASK);
                    taken += 1;
                }
                p += 4096;
            }
            if (0..512usize).all(|j| pt.add(j).read_volatile() & NPT_P == 0) {
                slot.write_volatile(0);
                freed.push(pt_phys);
                DEMAND_4K_BLOCKS.fetch_sub(1, Relaxed);
            }
        }
    }
    taken
}

/// Page-level write-through, cache disable and PAT bits of a 4 KB leaf:
/// they select the host PAT entry (`paging`: 0 WB, 3 UC, 5 WC).
const NPT_PWT: u64 = 1 << 3;
const NPT_PCD: u64 = 1 << 4;
const NPT_PAT_4K: u64 = 1 << 7;
pub const CACHE_WB: u64 = 0;
pub const CACHE_UC: u64 = NPT_PCD | NPT_PWT;
pub const CACHE_WC: u64 = NPT_PAT_4K | NPT_PWT;
/// No execute, honored with EFER.NXE (the host sets it).
const NPT_NX: u64 = 1 << 63;

/// The leaf entry for window page `gpa`, creating the PD and PT above it
/// when `create`. Not-present to present needs no TLB flush.
///
/// # Safety
/// `pml4_phys` is the root of this VM's live NPT; the caller serializes
/// window changes.
unsafe fn window_leaf(pml4_phys: u64, gpa: u64, create: bool) -> Option<*mut u64> {
    // SAFETY: identity-mapped tables of this NPT (caller contract); new
    // tables are fresh frames, zeroed before they are linked.
    unsafe {
        let pdpt = ((pml4_phys as *const u64).read_volatile() & NPT_ADDR_MASK) as *mut u64;
        let slot = pdpt.add((gpa / (1 << 30)) as usize);
        let mut pdpte = slot.read_volatile();
        if pdpte & NPT_P == 0 {
            if !create { return None; }
            let pd = memory::allocate_frame()?;
            core::ptr::write_bytes(pd as *mut u8, 0, 4096);
            pdpte = pd | NPT_P | NPT_RW | NPT_US;
            slot.write_volatile(pdpte);
        }
        let pd = (pdpte & NPT_ADDR_MASK) as *mut u64;
        let pd_slot = pd.add(((gpa % ONE_GB) / TWO_MB) as usize);
        let mut pde = pd_slot.read_volatile();
        if pde & NPT_P == 0 {
            if !create { return None; }
            let pt = memory::allocate_frame()?;
            core::ptr::write_bytes(pt as *mut u8, 0, 4096);
            pde = pt | NPT_P | NPT_RW | NPT_US;
            pd_slot.write_volatile(pde);
        }
        Some(((pde & NPT_ADDR_MASK) as *mut u64).add(((gpa % TWO_MB) / 4096) as usize))
    }
}

/// Map host frame `host` read/write, not executable, at window page `gpa`
/// with the host PAT entry `cache` selects (`CACHE_*`); the guest's PAT
/// combines with it. Fails if the page is mapped already.
pub fn window_map(pml4_phys: u64, gpa: u64, host: u64, cache: u64) -> Result<(), &'static str> {
    // SAFETY: EFER is readable on every x86_64 CPU.
    let nx = if unsafe { super::rdmsr(0xC000_0080) } & (1 << 11) != 0 { NPT_NX } else { 0 };
    // SAFETY: caller contract of `devices::hostmem` (live root, serialized).
    unsafe {
        let leaf = window_leaf(pml4_phys, gpa, true).ok_or("OOM: NPT window tables")?;
        if leaf.read_volatile() & NPT_P != 0 {
            return Err("window page already mapped");
        }
        leaf.write_volatile((host & NPT_ADDR_MASK) | NPT_P | NPT_RW | NPT_US | cache | nx);
    }
    Ok(())
}

/// Clear window page `gpa`. True if it was mapped; the caller then makes
/// every vCPU drop the translation (`devices::hostmem::unmap`).
pub fn window_unmap(pml4_phys: u64, gpa: u64) -> bool {
    // SAFETY: as in `window_map`.
    unsafe {
        let Some(leaf) = window_leaf(pml4_phys, gpa, false) else { return false };
        let was = leaf.read_volatile() & NPT_P != 0;
        leaf.write_volatile(0);
        was
    }
}

/// Whether window page `gpa` is mapped.
pub fn window_mapped(pml4_phys: u64, gpa: u64) -> bool {
    // SAFETY: as in `window_map`; read only.
    unsafe { window_leaf(pml4_phys, gpa, false).is_some_and(|l| l.read_volatile() & NPT_P != 0) }
}

/// Free the window's PD and PTs. The frames they map belong to GPU objects
/// and are not freed here.
///
/// # Safety
/// As `release`: nothing uses the NPT any more.
unsafe fn release_window(pdpt: *const u64) {
    // SAFETY: caller contract; identity-mapped tables of this table tree.
    unsafe {
        for slot in crate::microvm::devices::hostmem::pdpt_slots() {
            let pd_phys = pdpt.add(slot).read_volatile() & NPT_ADDR_MASK;
            if pd_phys == 0 { continue; }
            let pd = pd_phys as *const u64;
            for i in 0..512usize {
                let pde = pd.add(i).read_volatile();
                if pde & NPT_P != 0 {
                    memory::deallocate_frame(pde & NPT_ADDR_MASK);
                }
            }
            memory::deallocate_frame(pd_phys);
        }
    }
}

/// Free every demand-faulted 4 KB frame + the demand PT pages + the
/// fixed tables. The contiguous boot block is freed by the caller.
/// Mirrors `ept::release`.
pub fn release(pml4_phys: u64, guest_bytes: u64) {
    let boot_leaves = boot_window_bytes(guest_bytes) / TWO_MB;
    const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512
    // SAFETY: our own NPT; nothing references these after teardown.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt_phys = pml4.read_volatile() & NPT_ADDR_MASK;
        let pdpt = pdpt_phys as *const u64;
        // A RAM PD maps nothing but RAM, so every present entry is the guest's.
        for p in RamLayout::new(guest_bytes).pdpt_slots() {
            let pdpt_entry = pdpt.add(p).read_volatile();
            if pdpt_entry == 0 { continue; }
            let pd_phys = pdpt_entry & NPT_ADDR_MASK;
            let pd = pd_phys as *const u64;
            let base_leaf = (p as u64) * LEAVES_PER_PD;
            for leaf in base_leaf..base_leaf + LEAVES_PER_PD {
                if leaf < boot_leaves { continue; }
                let local = (leaf - base_leaf) as usize;
                let pde = pd.add(local).read_volatile();
                if pde & NPT_P == 0 { continue; }
                if pde & NPT_PS != 0 {
                    memory::deallocate_contiguous(pde & NPT_2M_ADDR_MASK, FRAMES_PER_2M);
                    continue;
                }
                let pt_phys = pde & NPT_ADDR_MASK;
                let pt = pt_phys as *const u64;
                for j in 0..512usize {
                    let pte = pt.add(j).read_volatile();
                    if pte & NPT_P != 0 {
                        memory::deallocate_frame(pte & NPT_ADDR_MASK);
                    }
                }
                memory::deallocate_frame(pt_phys);
            }
            memory::deallocate_frame(pd_phys);
        }
        release_window(pdpt);
        let pd_high_phys = pdpt.add(MMIO_PDPT_SLOT).read_volatile() & NPT_ADDR_MASK;
        if pd_high_phys != 0 {
            let pd_high = pd_high_phys as *const u64;
            let pt_dummy_phys = pd_high.add(502).read_volatile() & NPT_ADDR_MASK;
            let pt_lapic_phys = pd_high.add(503).read_volatile() & NPT_ADDR_MASK;
            if pt_lapic_phys != 0 && pt_lapic_phys != pt_dummy_phys {
                memory::deallocate_frame(pt_lapic_phys);
            }
            if pt_dummy_phys != 0 {
                // [0] is the trapped I/O APIC page; [1] maps the scratch page.
                let dummy = (pt_dummy_phys as *const u64).add(1).read_volatile() & NPT_ADDR_MASK;
                if dummy != 0 {
                    memory::deallocate_frame(dummy);
                }
                memory::deallocate_frame(pt_dummy_phys);
            }
            memory::deallocate_frame(pd_high_phys);
        }
        memory::deallocate_frame(pdpt_phys);
        memory::deallocate_frame(pml4_phys);
    }
}
