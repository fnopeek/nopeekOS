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
//! Identity-map shape: PML4[0] → PDPT, PDPT[0] → PD, PD[0..127] →
//! 2 MB pages identity-mapped from guest 0..256 MB → host 0..256 MB.
//! Total NPT footprint: 3 pages.
//!
//! 2 MB pages were picked over 1 GB after KVM nested SVM returned
//! an unexpected exit-code 0 with the larger-page variant — KVM's
//! nested-NPT shadow path appears to treat 1 GB-leaf entries
//! differently than real hardware. 2 MB stays safely inside the
//! commonly-tested path.

use crate::mm::memory;

/// Number of 2 MB pages to identity-map. 128 × 2 MB = 256 MB,
/// matching VMX `ept::GUEST_WINDOW_BYTES`. Enough for the substrate
/// stub + a future Linux guest in 12.1.1c-svm.
const NPT_2MB_COUNT: usize = 128;

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

/// Build a fresh NPT root that identity-maps `0..256 MB` of guest
/// physical to host physical via 2 MB pages. Returns the physical
/// address of the PML4 page, suitable for VMCB.NCR3.
///
/// Allocates 3 frames per call (PML4 + PDPT + PD). Frames are leaked
/// alongside the rest of the per-call substrate-test allocations.
pub fn allocate_identity_npt() -> Result<u64, &'static str> {
    let pml4_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PML4")?;
    let pdpt_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PDPT")?;
    let pd_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PD")?;

    // SAFETY: freshly allocated, identity-mapped (host paging),
    // exclusive. All three pages are 4 KB aligned (frame allocator
    // guarantee).
    unsafe {
        core::ptr::write_bytes(pml4_phys as *mut u8, 0, 4096);
        core::ptr::write_bytes(pdpt_phys as *mut u8, 0, 4096);
        core::ptr::write_bytes(pd_phys as *mut u8, 0, 4096);

        // PML4[0] → PDPT (non-leaf, no PS bit).
        let pml4 = pml4_phys as *mut u64;
        pml4.write_volatile(pdpt_phys | NPT_P | NPT_RW | NPT_US);

        // PDPT[0] → PD (non-leaf).
        let pdpt = pdpt_phys as *mut u64;
        pdpt.write_volatile(pd_phys | NPT_P | NPT_RW | NPT_US);

        // PD[i] = (i × 2 MB) | leaf flags (PS=1) for i in 0..128.
        let pd = pd_phys as *mut u64;
        for i in 0..NPT_2MB_COUNT {
            let phys = (i as u64) * (1 << 21); // 2 MB
            let entry = phys | NPT_P | NPT_RW | NPT_US | NPT_PS;
            pd.add(i).write_volatile(entry);
        }
    }

    Ok(pml4_phys)
}
