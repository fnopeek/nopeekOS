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
//! * `allocate_identity_npt()` — guest 0..256 MB → host 0..256 MB.
//!   Used by the substrate test where the guest stub lives wherever
//!   the frame allocator hands it out. Only safe when the stub
//!   happens to fall below 256 MB.
//!
//! * `allocate_window_npt(host_base)` — guest 0..256 MB → host
//!   `host_base..host_base+256 MB`. Used by `run_linux` so the guest
//!   can write freely at GPA 0x10000/0x90000/0x100000/... without
//!   stomping on the host kernel image (which sits at host_phys 1 MB
//!   from Multiboot2). Mirrors `vmx::ept::install_window`.
//!
//! Both modes use 2 MB pages (3-frame NPT footprint: PML4+PDPT+PD).
//! 1 GB pages tested unstable on KVM nested SVM (exit-code 0); 2 MB
//! stays inside the well-shadowed path.
//!
//! `allocate_window_npt` additionally maps the high MMIO region
//! [0xFEC00000, 0xFF000000) (IOAPIC + HPET + LAPIC) to a single
//! aliased scratch page. With `nolapic noapic acpi=off` Linux barely
//! touches it — the mapping is defence-in-depth so an early MMIO
//! probe doesn't NPF before we've added a real exit handler.

use crate::mm::memory;

/// Number of 4 KB host frames the caller must allocate contiguously
/// for the guest RAM backing of `allocate_window_npt`. 256 MB = 65536
/// frames. Matches `vmx::ept::GUEST_RAM_FRAMES`.
pub const GUEST_RAM_FRAMES: usize = (GUEST_WINDOW_BYTES / 4096) as usize;

/// Slack frames added so `memory::allocate_contiguous` can be rounded
/// up to a 2 MB boundary for the NPT large-page leaf entries. Up to
/// 511 frames (≈ 2 MB) at the front of the contiguous range may go
/// unused. Mirrors `vmx::ept::GUEST_RAM_ALIGN_SLACK`.
pub const GUEST_RAM_ALIGN_SLACK: usize = 511;

/// Number of 2 MB pages to identity-map. 128 × 2 MB = 256 MB,
/// matching VMX `ept::GUEST_WINDOW_BYTES`. Enough for the substrate
/// stub + a Linux guest in 12.1.1c-svm.
const NPT_2MB_COUNT: usize = 128;

const TWO_MB: u64 = 2 * 1024 * 1024;
const GUEST_WINDOW_BYTES: u64 = 256 * 1024 * 1024;

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

/// Round a raw `allocate_contiguous` base up to the next 2 MB
/// boundary so it can be passed to `allocate_window_npt`.
pub fn round_up_to_2mb(raw_base: u64) -> u64 {
    (raw_base + (TWO_MB - 1)) & !(TWO_MB - 1)
}

/// Build a fresh NPT root that identity-maps `0..256 MB` of guest
/// physical to host physical via 2 MB pages. Returns the physical
/// address of the PML4 page, suitable for VMCB.NCR3.
///
/// Allocates 3 frames per call (PML4 + PDPT + PD). Frames are leaked
/// alongside the rest of the per-call substrate-test allocations.
pub fn allocate_identity_npt() -> Result<u64, &'static str> {
    build_npt(0, /* with_mmio_scratch */ false)
}

/// Build a fresh NPT root that maps `0..256 MB` of guest physical to
/// host physical `host_base..host_base+256 MB` via 2 MB pages, plus a
/// scratch alias for [0xFEC00000, 0xFF000000) (IOAPIC + HPET +
/// LAPIC). Returns NCR3.
///
/// `host_base` must be 2 MB aligned. Allocates 6 frames (PML4 + PDPT
/// + PD + PD_HIGH + PT_DUMMY + dummy_page). Frames are leaked.
pub fn allocate_window_npt(host_base: u64) -> Result<u64, &'static str> {
    if host_base & (TWO_MB - 1) != 0 {
        return Err("NPT: host_base must be 2 MB aligned");
    }
    build_npt(host_base, /* with_mmio_scratch */ true)
}

/// Inner builder. `host_base = 0` gives identity mapping; non-zero
/// shifts the leaf addresses by `host_base`.
fn build_npt(host_base: u64, with_mmio_scratch: bool) -> Result<u64, &'static str> {
    let pml4_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PML4")?;
    let pdpt_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PDPT")?;
    let pd_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PD")?;

    // SAFETY: freshly allocated, identity-mapped (host paging),
    // exclusive. All pages are 4 KB aligned (frame allocator
    // guarantee).
    unsafe {
        core::ptr::write_bytes(pml4_phys as *mut u8, 0, 4096);
        core::ptr::write_bytes(pdpt_phys as *mut u8, 0, 4096);
        core::ptr::write_bytes(pd_phys as *mut u8, 0, 4096);

        // PML4[0] → PDPT (non-leaf, no PS bit).
        let pml4 = pml4_phys as *mut u64;
        pml4.write_volatile(pdpt_phys | NPT_P | NPT_RW | NPT_US);

        // PDPT[0] → PD (non-leaf, covers [0, 1 GB)).
        let pdpt = pdpt_phys as *mut u64;
        pdpt.write_volatile(pd_phys | NPT_P | NPT_RW | NPT_US);

        // PD[i] = (host_base + i × 2 MB) | leaf flags for i in 0..128.
        let pd = pd_phys as *mut u64;
        for i in 0..NPT_2MB_COUNT {
            let host_target = host_base + (i as u64) * TWO_MB;
            let entry = host_target | NPT_P | NPT_RW | NPT_US | NPT_PS;
            pd.add(i).write_volatile(entry);
        }

        if with_mmio_scratch {
            // PDPT[3] → PD_HIGH (covers [3 GB, 4 GB), MMIO range).
            let pd_high_phys = memory::allocate_frame()
                .ok_or("OOM allocating NPT PD_HIGH")?;
            let pt_dummy_phys = memory::allocate_frame()
                .ok_or("OOM allocating NPT PT_DUMMY")?;
            let dummy_page_phys = memory::allocate_frame()
                .ok_or("OOM allocating NPT dummy page")?;

            core::ptr::write_bytes(pd_high_phys as *mut u8, 0, 4096);
            core::ptr::write_bytes(pt_dummy_phys as *mut u8, 0, 4096);
            core::ptr::write_bytes(dummy_page_phys as *mut u8, 0, 4096);

            pdpt.add(3).write_volatile(pd_high_phys | NPT_P | NPT_RW | NPT_US);

            // PD_HIGH[502] + [503] → same PT_DUMMY (covers
            // [0xFEC00000, 0xFF000000)). Two PD entries aliased to
            // one PT, so 4 MB of guest-phys all walk through the
            // same 512-entry PT.
            let pd_high = pd_high_phys as *mut u64;
            pd_high.add(502).write_volatile(pt_dummy_phys | NPT_P | NPT_RW | NPT_US);
            pd_high.add(503).write_volatile(pt_dummy_phys | NPT_P | NPT_RW | NPT_US);

            // PT_DUMMY[0..512] all → dummy_page. 4 MB → 4 KB scratch.
            let pt_dummy = pt_dummy_phys as *mut u64;
            for i in 0..512usize {
                pt_dummy.add(i).write_volatile(dummy_page_phys | NPT_P | NPT_RW | NPT_US);
            }
        }
    }

    Ok(pml4_phys)
}
