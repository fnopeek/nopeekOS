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

/// Slack frames `boot_frames_for` adds so `allocate_contiguous` can be
/// rounded up to a 2 MB boundary. Mirrors `ept`.
const GUEST_RAM_ALIGN_SLACK: usize = 511;

/// Bits [51:12] of an NPT entry hold the next-level / page phys addr.
const NPT_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

const TWO_MB: u64 = 2 * 1024 * 1024;
const ONE_GB: u64 = 1024 * 1024 * 1024;
/// Maximum guest RAM we can map: PDPT slots [0, 1, 2] hold guest-RAM
/// PDs (1 GiB each), PDPT[3] is reserved for the MMIO scratch range.
/// Mirrors `ept::MAX_GUEST_BYTES`.
const MAX_GUEST_BYTES: u64 = 3 * ONE_GB;
// Canonical size lives in `guest_mem`; this is its NPT-window twin.
const GUEST_WINDOW_BYTES: u64 = crate::microvm::devices::guest_mem::GUEST_RAM_BYTES;

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

/// Build a fresh NPT root that identity-maps `GUEST_WINDOW_BYTES` of guest
/// physical to host physical via 2 MB pages. Returns the physical
/// address of the PML4 page, suitable for VMCB.NCR3.
///
/// Allocates 3 frames per call (PML4 + PDPT + PD). Frames are leaked
/// alongside the rest of the per-call substrate-test allocations.
pub fn allocate_identity_npt() -> Result<u64, &'static str> {
    // Substrate test: the whole guest window.
    build_npt(0, GUEST_WINDOW_BYTES, /* with_mmio_scratch */ false)
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
/// shifts the leaf addresses by `host_base`. Maps exactly
/// `guest_bytes` (one PD per GiB, up to `MAX_GUEST_BYTES`).
fn build_npt(host_base: u64, guest_bytes: u64, with_mmio_scratch: bool) -> Result<u64, &'static str> {
    if guest_bytes == 0 || guest_bytes & (TWO_MB - 1) != 0 {
        return Err("NPT: guest_bytes must be a non-zero multiple of 2 MB");
    }
    if guest_bytes > MAX_GUEST_BYTES {
        return Err("NPT: guest_bytes > 3 GiB (PDPT[3] reserved for MMIO)");
    }
    let guest_leaves = (guest_bytes / TWO_MB) as u64;
    // Identity substrate test (host_base = 0) stays all-2-MB; the real
    // window path is hybrid (contiguous boot + 4-KB demand region).
    let boot_leaves = if host_base == 0 {
        guest_leaves
    } else {
        boot_window_bytes(guest_bytes) / TWO_MB
    };
    let num_pds = ((guest_bytes + ONE_GB - 1) / ONE_GB) as usize;

    // Fixed tables: PML4, PDPT, the guest-RAM PDs, and with the MMIO
    // scratch PD_HIGH, PT_DUMMY, PT_LAPIC and the scratch page. All or none.
    let nfixed = 2 + num_pds + if with_mmio_scratch { 4 } else { 0 };
    let mut fixed = [0u64; 9];
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

        // PDPT[0..num_pds] → guest-RAM PDs (each covers 1 GiB).
        let pdpt = pdpt_phys as *mut u64;
        for p in 0..num_pds {
            pdpt.add(p).write_volatile(pd_physs[p] | NPT_P | NPT_RW | NPT_US);
        }

        if with_mmio_scratch {
            let m = 2 + num_pds;
            let (pd_high_phys, pt_dummy_phys, pt_lapic_phys, dummy_page_phys) =
                (fixed[m], fixed[m + 1], fixed[m + 2], fixed[m + 3]);

            // PDPT[3] → PD_HIGH (covers [3 GB, 4 GB), MMIO range).
            pdpt.add(3).write_volatile(pd_high_phys | NPT_P | NPT_RW | NPT_US);

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

        // Populate each PD: [base_leaf, end_leaf) in this PD's window. The
        // tree is linked and walkable from here on, so a failed demand-PT
        // allocation unwinds through `release`.
        const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512
        for p in 0..num_pds {
            let pd = pd_physs[p] as *mut u64;
            let base_leaf = (p as u64) * LEAVES_PER_PD;
            let end_leaf = ((p as u64 + 1) * LEAVES_PER_PD).min(guest_leaves);
            for leaf in base_leaf..end_leaf {
                let local = (leaf - base_leaf) as usize;
                if leaf < boot_leaves {
                    let host_target = host_base + leaf * TWO_MB;
                    let entry = host_target | NPT_P | NPT_RW | NPT_US | NPT_PS;
                    pd.add(local).write_volatile(entry);
                } else {
                    let Some(pt) = memory::allocate_frame() else {
                        release(pml4_phys, guest_bytes);
                        return Err("OOM allocating NPT demand PT");
                    };
                    core::ptr::write_bytes(pt as *mut u8, 0, 4096);
                    pd.add(local).write_volatile(pt | NPT_P | NPT_RW | NPT_US);
                }
            }
        }
    }

    Ok(pml4_phys)
}

/// Walk PML4[0]→PDPT[0]→PD→PT for a demand-region `gpa`; allocate +
/// map a zeroed 4 KB frame on first touch, return its host phys.
/// Idempotent. Called from the #NPF handler and `GuestMem` (host DMA
/// to an untouched page). `gpa` must be ≥ the boot window. No
/// INVLPGA needed: the entry was not-present (no stale TLB).
/// Mirrors `ept::demand_fault_in`.
pub fn demand_fault_in(pml4_phys: u64, gpa: u64) -> Option<u64> {
    // SAFETY: our own freshly-built NPT, identity-mapped tables.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt = (pml4.read_volatile() & NPT_ADDR_MASK) as *const u64;
        // Pick the right guest-RAM PD from PDPT[0..3] by gpa.
        let pdpt_idx = (gpa / ONE_GB) as usize;
        if pdpt_idx >= 3 {
            return None; // PDPT[3] is MMIO, never demand-faulted
        }
        let pdpt_entry = pdpt.add(pdpt_idx).read_volatile();
        if pdpt_entry == 0 {
            return None; // PD not present (gpa beyond advertised guest_bytes)
        }
        let pd = (pdpt_entry & NPT_ADDR_MASK) as *mut u64;
        let pd_idx = ((gpa % ONE_GB) / TWO_MB) as usize;
        let pde = pd.add(pd_idx).read_volatile();
        if pde == 0 || pde & NPT_PS != 0 {
            return None; // outside demand region / unexpected 2-MB leaf
        }
        let pt = (pde & NPT_ADDR_MASK) as *mut u64;
        let pt_idx = ((gpa % TWO_MB) / 4096) as usize;
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
        // walking fresh memory takes one exit per 2 MB, not one per 4 KB
        // (KVM gets the same from a THP-backed memslot). Stops at the host
        // reserve; the remaining pages fault in singly.
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

/// Free every demand-faulted 4 KB frame + the demand PT pages + the
/// fixed tables. The contiguous boot block is freed by the caller.
/// Mirrors `ept::release`.
pub fn release(pml4_phys: u64, guest_bytes: u64) {
    let boot_leaves = boot_window_bytes(guest_bytes) / TWO_MB;
    let guest_leaves = guest_bytes / TWO_MB;
    let num_pds = ((guest_bytes + ONE_GB - 1) / ONE_GB) as usize;
    const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512
    // SAFETY: our own NPT; nothing references these after teardown.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt_phys = pml4.read_volatile() & NPT_ADDR_MASK;
        let pdpt = pdpt_phys as *const u64;
        for p in 0..num_pds {
            let pdpt_entry = pdpt.add(p).read_volatile();
            if pdpt_entry == 0 { continue; }
            let pd_phys = pdpt_entry & NPT_ADDR_MASK;
            let pd = pd_phys as *const u64;
            let base_leaf = (p as u64) * LEAVES_PER_PD;
            let end_leaf = ((p as u64 + 1) * LEAVES_PER_PD).min(guest_leaves);
            for leaf in base_leaf..end_leaf {
                if leaf < boot_leaves { continue; }
                let local = (leaf - base_leaf) as usize;
                let pde = pd.add(local).read_volatile();
                if pde == 0 || pde & NPT_PS != 0 { continue; }
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
        let pd_high_phys = pdpt.add(3).read_volatile() & NPT_ADDR_MASK;
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
