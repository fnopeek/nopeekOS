//! Extended Page Tables (EPT) — Phase 12.1.1a/c-1/c-3.
//!
//! Maps a 64-MB guest-physical window [0, 64 MB) onto a contiguous
//! 64-MB host-physical region using 2-MB EPT large pages (32 leaf
//! entries in a single PD). The host backing region is allocated
//! once via `memory::allocate_contiguous(GUEST_RAM_FRAMES + slack)`;
//! the caller rounds the result up to a 2-MB boundary and passes
//! that base in.
//!
//! Why non-identity (12.1.1c-1 vs the v0.97 1-GB identity map):
//! the guest will copy Linux's bzImage into its address space at
//! guest-phys 0x10000 (setup) and 0x100000 (protected-mode kernel)
//! — but host_phys 0x100000 is the kernel.bin's own load address
//! (Multiboot2 puts us at 1 MB). A non-identity EPT separates the
//! two so the guest can write freely without corrupting host code.
//!
//! Why 64 MB: Alpine 6.18 linux-virt's `init_size` field reports
//! 0x25ff000 ≈ 38 MB — that's how much memory Linux's early boot
//! needs for decompression buffers + brk + page tables before
//! it sees its own memory map. 16 MB (12.1.1c-1) was enough for
//! the real-mode HLT-test substrate but cannot host real Linux.
//! 64 MB rounded up gives Linux some headroom and stays in a
//! single PD's range (32 × 2-MB leaves; one PML4 + one PDPT + one
//! PD covers everything). Larger windows would need a second PD.
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
const GUEST_WINDOW_BYTES: u64 = 256 * 1024 * 1024;
const PD_LEAVES: u64 = GUEST_WINDOW_BYTES / TWO_MB; // 128 entries (max 512 = 1 GB)

/// Number of 4 KB host frames the caller must allocate contiguously
/// for the guest RAM backing.
pub const GUEST_RAM_FRAMES: usize = (GUEST_WINDOW_BYTES / 4096) as usize;

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

/// Build the EPT. Maps:
///   - guest-physical [0, 64 MB) → host-physical [host_base, +64 MB)
///     via 32 × 2-MB leaf entries (PD)
///   - guest-physical [0xFEC00000, 0xFF000000) → 4 KB dummy scratch
///     page (aliased): 4 MB of guest-phys → same single host page
///     via PT-level mapping. Covers IOAPIC (0xFEC00000), HPET
///     (0xFED00000), and LAPIC (0xFEE00000). Reads return scratch
///     contents (initially zero), writes land in scratch — not real
///     MMIO semantics, but enough to absorb Linux's early MMIO
///     probes without EPT-violating. With `nolapic noapic acpi=off
///     pci=off` cmdline, Linux barely touches this anyway; mapping
///     it is just defence in depth.
/// Returns the EPTP value to be VMWRITE'd into VMCS::EPT_POINTER.
pub fn install_window(host_base: u64) -> Result<u64, &'static str> {
    if host_base & (TWO_MB - 1) != 0 {
        return Err("EPT: host_base must be 2-MB aligned for 2-MB EPT pages");
    }

    let pml4_phys = memory::allocate_frame().ok_or("OOM: EPT PML4")?;
    let pdpt_phys = memory::allocate_frame().ok_or("OOM: EPT PDPT")?;
    let pd_phys = memory::allocate_frame().ok_or("OOM: EPT PD")?;
    let pd_high_phys = memory::allocate_frame().ok_or("OOM: EPT PD_HIGH")?;
    let pt_dummy_phys = memory::allocate_frame().ok_or("OOM: EPT PT_DUMMY")?;
    let dummy_page_phys = memory::allocate_frame().ok_or("OOM: EPT dummy page")?;

    // SAFETY: identity-mapped, freshly allocated, exclusive.
    unsafe {
        // PML4[0] → PDPT, R/W/X for sub-tree.
        let pml4 = pml4_phys as *mut u64;
        core::ptr::write_bytes(pml4 as *mut u8, 0, 4096);
        pml4.add(0).write_volatile(pdpt_phys | EPT_RWX);

        // PDPT[0] → PD (covers [0, 1 GB), of which we use 64 MB).
        let pdpt = pdpt_phys as *mut u64;
        core::ptr::write_bytes(pdpt as *mut u8, 0, 4096);
        pdpt.add(0).write_volatile(pd_phys | EPT_RWX);
        // PDPT[3] → PD_HIGH (covers [3 GB, 4 GB), MMIO range lives here).
        pdpt.add(3).write_volatile(pd_high_phys | EPT_RWX);

        // PD[0..PD_LEAVES] = 2-MB leaf entries covering 64 MB.
        let pd = pd_phys as *mut u64;
        core::ptr::write_bytes(pd as *mut u8, 0, 4096);
        for i in 0..PD_LEAVES {
            let host_target = host_base + i * TWO_MB;
            pd.add(i as usize)
                .write_volatile(host_target | EPT_RWX | EPT_MEM_TYPE_WB | EPT_LEAF);
        }

        // PD_HIGH[502] + [503] → same PT_DUMMY (covers [0xFEC00000,
        // 0xFF000000) = IOAPIC + HPET + LAPIC). Two PD entries
        // aliased to one PT, so 4 MB of guest-phys all walk through
        // the same 512-entry PT.
        let pd_high = pd_high_phys as *mut u64;
        core::ptr::write_bytes(pd_high as *mut u8, 0, 4096);
        pd_high.add(502).write_volatile(pt_dummy_phys | EPT_RWX);
        pd_high.add(503).write_volatile(pt_dummy_phys | EPT_RWX);

        // PT_DUMMY[0..512] all → dummy_page (4 KB). 4 MB of guest-phys
        // → 4 KB host scratch. PT entries are always 4-KB leaves;
        // no leaf-bit needed (bit 7 must be 0 in EPT PTEs).
        let pt_dummy = pt_dummy_phys as *mut u64;
        core::ptr::write_bytes(pt_dummy as *mut u8, 0, 4096);
        core::ptr::write_bytes(dummy_page_phys as *mut u8, 0, 4096);
        for i in 0..512usize {
            pt_dummy
                .add(i)
                .write_volatile(dummy_page_phys | EPT_RWX | EPT_MEM_TYPE_WB);
        }
    }

    Ok(pml4_phys | EPTP_WALK_LENGTH_4 | EPTP_MEM_TYPE_WB)
}
