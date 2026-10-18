//! amdgpu/mmhub_v1_0.c (Linux 6.18.55): the multimedia VM hub, MMHUB 1.x.

use crate::amdgpu_device::amdgpu_device;
use crate::nbio_v7_0::rreg32_soc15;
use crate::regs::*;

/// `SOC15_REG_OFFSET(MMHUB, 0, reg)`.
fn mm_offset(adev: &amdgpu_device, base_idx: u32, reg: u32) -> u32 {
    adev.reg_offset[MMHUB_HWIP as usize][0].get(base_idx as usize).copied().unwrap_or(0) + reg
}

/// `mmhub_v1_0_get_fb_location`.
pub fn mmhub_v1_0_get_fb_location(adev: &mut amdgpu_device) -> u64 {
    let mut base = rreg32_soc15(adev, MMHUB_HWIP, 0, MM_mmMC_VM_FB_LOCATION_BASE_BASE_IDX, MM_mmMC_VM_FB_LOCATION_BASE)
        .unwrap_or(0) as u64;
    let mut top = rreg32_soc15(adev, MMHUB_HWIP, 0, MM_mmMC_VM_FB_LOCATION_TOP_BASE_IDX, MM_mmMC_VM_FB_LOCATION_TOP)
        .unwrap_or(0) as u64;
    base &= MM_MC_VM_FB_LOCATION_BASE__FB_BASE_MASK as u64;
    base <<= 24;
    top &= MM_MC_VM_FB_LOCATION_TOP__FB_TOP_MASK as u64;
    top <<= 24;
    adev.gmc.fb_start = base;
    adev.gmc.fb_end = top;
    base
}

/// `mmhub_v1_0_init`.
pub fn mmhub_v1_0_init(adev: &mut amdgpu_device) {
    let o = |i, r| mm_offset(adev, i, r);
    let hub = crate::amdgpu_device::amdgpu_vmhub {
        ctx0_ptb_addr_lo32: o(MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32),
        ctx0_ptb_addr_hi32: o(MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32),
        vm_inv_eng0_sem: o(MM_mmVM_INVALIDATE_ENG0_SEM_BASE_IDX, MM_mmVM_INVALIDATE_ENG0_SEM),
        vm_inv_eng0_req: o(MM_mmVM_INVALIDATE_ENG0_REQ_BASE_IDX, MM_mmVM_INVALIDATE_ENG0_REQ),
        vm_inv_eng0_ack: o(MM_mmVM_INVALIDATE_ENG0_ACK_BASE_IDX, MM_mmVM_INVALIDATE_ENG0_ACK),
        vm_context0_cntl: o(MM_mmVM_CONTEXT0_CNTL_BASE_IDX, MM_mmVM_CONTEXT0_CNTL),
        vm_l2_pro_fault_status: o(MM_mmVM_L2_PROTECTION_FAULT_STATUS_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_STATUS),
        vm_l2_pro_fault_cntl: o(MM_mmVM_L2_PROTECTION_FAULT_CNTL_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_CNTL),
        ctx_distance: MM_CTX_DISTANCE,
        ctx_addr_distance: MM_CTX_ADDR_DISTANCE,
        eng_distance: MM_ENG_DISTANCE,
        eng_addr_distance: MM_ENG_ADDR_DISTANCE,
    };
    adev.vmhub[AMDGPU_MMHUB0_0 as usize] = hub;
}
