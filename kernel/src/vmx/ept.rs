//! Extended Page Tables (EPT) — Phase 12.1.1a/c-1.
//!
//! Maps a 16-MB guest-physical window [0, 16 MB) onto a contiguous
//! 16-MB host-physical region using 2-MB EPT large pages (8 leaf
//! entries in a single PD). The host backing region is allocated
//! once via `memory::allocate_contiguous(4096)`; the caller passes
//! that host base in.
//!
//! Why non-identity (12.1.1c-1 vs the v0.97 1-GB identity map):
//! the guest will copy Linux's bzImage into its address space at
//! guest-phys 0x10000 (setup) and 0x100000 (protected-mode kernel)
//! — but host_phys 0x100000 is the kernel.bin's own load address
//! (Multiboot2 puts us at 1 MB). A non-identity EPT separates the
//! two so the guest can write freely without corrupting host code.
//!
//! 16 MB is enough for: setup (~8 KB), boot_params (~4 KB), cmdline
//! (~256 B), the Alpine vmlinuz-virt protected-mode kernel
//! (~5-7 MB). A larger window can be plumbed in later by stretching
//! the PD entries; the current 8-entry layout caps at 16 MB.
//!
//! Tables are leaked (same lifecycle as VMXON / VMCS regions in
//! `vmx/enable.rs`).
//!
//! Reference: Intel SDM Vol. 3C §28.2 (EPT Mechanism), Vol. 3D
//! Appendix A.10 (VPID and EPT Capabilities).

use crate::mm::memory;

// EPT entry permission + attribute bits.
const EPT_R: u64 = 1 << 0;
const EPT_W: u64 = 1 << 1;
const EPT_X: u64 = 1 << 2;
const EPT_RWX: u64 = EPT_R | EPT_W | EPT_X;
// Memory type for leaf entries: 6 = WB (write-back).
const EPT_MEM_TYPE_WB: u64 = 6 << 3;
// Bit 7: leaf entry (page) vs pointer to next-level table.
const EPT_LEAF: u64 = 1 << 7;

// EPTP fields VMWRITE'd into VMCS::EPT_POINTER.
const EPTP_MEM_TYPE_WB: u64 = 6;
const EPTP_WALK_LENGTH_4: u64 = 3 << 3; // 4 levels = walk length 3

const TWO_MB: u64 = 2 * 1024 * 1024;
const SIXTEEN_MB: u64 = 16 * 1024 * 1024;

/// Number of 4 KB host frames the caller must allocate contiguously
/// for the guest RAM backing.
pub const GUEST_RAM_FRAMES: usize = (SIXTEEN_MB / 4096) as usize;

/// Slack frames the caller adds to GUEST_RAM_FRAMES so the result of
/// `memory::allocate_contiguous` can be rounded up to a 2-MB boundary
/// for the EPT large-page leaf entries. Up to 511 frames (≈ 2 MB)
/// at the front of the contiguous range may go unused. Cheap on
/// 16 GB systems; alternative would be reworking the allocator to
/// honour alignment.
pub const GUEST_RAM_ALIGN_SLACK: usize = 511;

/// Round a raw `allocate_contiguous` base up to the next 2-MB
/// boundary so it can be passed to `install_window_16mb`.
pub fn round_up_to_2mb(raw_base: u64) -> u64 {
    (raw_base + (TWO_MB - 1)) & !(TWO_MB - 1)
}

/// Build the EPT, mapping guest-physical [0, 16 MB) → host-physical
/// [host_base, host_base + 16 MB) via 8 × 2-MB leaf entries.
/// Returns the EPTP value to be VMWRITE'd into VMCS::EPT_POINTER.
pub fn install_window_16mb(host_base: u64) -> Result<u64, &'static str> {
    if host_base & (TWO_MB - 1) != 0 {
        return Err("EPT: host_base must be 2-MB aligned for 2-MB EPT pages");
    }

    let pml4_phys = memory::allocate_frame().ok_or("OOM: EPT PML4")?;
    let pdpt_phys = memory::allocate_frame().ok_or("OOM: EPT PDPT")?;
    let pd_phys = memory::allocate_frame().ok_or("OOM: EPT PD")?;

    // SAFETY: identity-mapped, freshly allocated, exclusive.
    unsafe {
        // PML4[0] → PDPT, R/W/X for sub-tree.
        let pml4 = pml4_phys as *mut u64;
        core::ptr::write_bytes(pml4 as *mut u8, 0, 4096);
        pml4.add(0).write_volatile(pdpt_phys | EPT_RWX);

        // PDPT[0] → PD, R/W/X for sub-tree.
        let pdpt = pdpt_phys as *mut u64;
        core::ptr::write_bytes(pdpt as *mut u8, 0, 4096);
        pdpt.add(0).write_volatile(pd_phys | EPT_RWX);

        // PD[0..8] = 8 × 2-MB leaf entries covering 16 MB.
        let pd = pd_phys as *mut u64;
        core::ptr::write_bytes(pd as *mut u8, 0, 4096);
        for i in 0..8u64 {
            let host_target = host_base + i * TWO_MB;
            pd.add(i as usize)
                .write_volatile(host_target | EPT_RWX | EPT_MEM_TYPE_WB | EPT_LEAF);
        }
    }

    Ok(pml4_phys | EPTP_WALK_LENGTH_4 | EPTP_MEM_TYPE_WB)
}
