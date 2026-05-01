//! Extended Page Tables (EPT) — Phase 12.1.1a.
//!
//! Identity-maps the first 1 GB of guest-physical → host-physical
//! address space using a single 1 GB EPT large page if the CPU
//! supports it (IA32_VMX_EPT_VPID_CAP bit 17), otherwise falls back
//! to 512 × 2 MB pages via a third-level PD.
//!
//! With EPT enabled, the CPU does a two-stage translation on every
//! guest memory access: guest CR3 walk yields guest-physical, then
//! the EPT walk yields host-physical. Identity-mapping both stages
//! (guest CR3 = host CR3 for our HLT-loop guest, EPT identity map
//! here) means every guest virtual address ultimately resolves to
//! the same host-physical numerical value — simple and sufficient
//! for the HLT-exit demonstration.
//!
//! Tables are leaked deliberately, same lifecycle as VMXON / VMCS
//! regions in `vmx/enable.rs`.
//!
//! Reference: Intel SDM Vol. 3C §28.2 (The Extended Page Table
//! Mechanism), §28.3.3.2 (EPT Misconfigurations), Vol. 3D
//! Appendix A.10 (VPID and EPT Capabilities).

use super::rdmsr;
use crate::mm::memory;

const IA32_VMX_EPT_VPID_CAP: u32 = 0x48C;
const EPT_VPID_CAP_1GB_PAGE: u64 = 1 << 17;

// EPT entry permission + attribute bits.
const EPT_R: u64 = 1 << 0;
const EPT_W: u64 = 1 << 1;
const EPT_X: u64 = 1 << 2;
const EPT_RWX: u64 = EPT_R | EPT_W | EPT_X;
// Memory type for leaf entries: 6 = WB (write-back). Matches the
// MTRR setting our kernel uses for normal RAM.
const EPT_MEM_TYPE_WB: u64 = 6 << 3;
// Bit 7: leaf entry (page) vs pointer to next-level table.
const EPT_LEAF: u64 = 1 << 7;

// EPTP fields (the value VMWRITE'd into VMCS::EPT_POINTER).
const EPTP_MEM_TYPE_WB: u64 = 6;
// Walk-length minus 1: 4-level table → value 3.
const EPTP_WALK_LENGTH_4: u64 = 3 << 3;

const TWO_MB: u64 = 2 * 1024 * 1024;

/// Allocate + initialise the EPT, identity-mapping the first 1 GB.
/// Returns the EPTP value to be VMWRITE'd into VMCS::EPT_POINTER.
pub fn install_identity_1gb() -> Result<u64, &'static str> {
    // SAFETY: IA32_VMX_EPT_VPID_CAP is architectural when VMX is
    // present (gated by `vmx::probe()` upstream).
    let supports_1gb = unsafe { rdmsr(IA32_VMX_EPT_VPID_CAP) } & EPT_VPID_CAP_1GB_PAGE != 0;

    let pml4_phys = memory::allocate_frame().ok_or("OOM: EPT PML4")?;
    let pdpt_phys = memory::allocate_frame().ok_or("OOM: EPT PDPT")?;

    // SAFETY: identity-mapped, freshly allocated, exclusive.
    unsafe {
        let pml4 = pml4_phys as *mut u64;
        core::ptr::write_bytes(pml4 as *mut u8, 0, 4096);
        // PML4[0] points to PDPT, with R/W/X permissions for the
        // sub-tree. Non-leaf entries don't carry a memory type.
        pml4.add(0).write_volatile(pdpt_phys | EPT_RWX);

        let pdpt = pdpt_phys as *mut u64;
        core::ptr::write_bytes(pdpt as *mut u8, 0, 4096);

        if supports_1gb {
            // PDPT[0] = leaf 1-GB EPT page covering [0, 1 GB).
            pdpt.add(0).write_volatile(EPT_RWX | EPT_MEM_TYPE_WB | EPT_LEAF);
        } else {
            // Fall back to 2-MB leaves: one PD with 512 entries.
            let pd_phys = memory::allocate_frame().ok_or("OOM: EPT PD")?;
            pdpt.add(0).write_volatile(pd_phys | EPT_RWX);
            let pd = pd_phys as *mut u64;
            core::ptr::write_bytes(pd as *mut u8, 0, 4096);
            for i in 0..512u64 {
                let base = i * TWO_MB;
                pd.add(i as usize)
                    .write_volatile(base | EPT_RWX | EPT_MEM_TYPE_WB | EPT_LEAF);
            }
        }
    }

    Ok(pml4_phys | EPTP_WALK_LENGTH_4 | EPTP_MEM_TYPE_WB)
}
