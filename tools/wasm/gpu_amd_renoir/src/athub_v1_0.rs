//! amdgpu/athub_v1_0.c (Linux 6.18.55): the ATHUB clock gating GMC v9
//! switches with its own.

use crate::amdgpu_device::amdgpu_device;
use crate::nbio_v7_0::{rreg32_soc15, wreg32_soc15};
use crate::regs::*;

/// `athub_update_medium_grain_clock_gating`.
fn athub_update_medium_grain_clock_gating(adev: &amdgpu_device, enable: bool) {
    let def = rreg32_soc15(adev, ATHUB_HWIP, 0, ATHUB_mmATHUB_MISC_CNTL_BASE_IDX, ATHUB_mmATHUB_MISC_CNTL).unwrap_or(0);
    let data = if enable && adev.cg_flags & AMD_CG_SUPPORT_MC_MGCG as u64 != 0 {
        def | ATHUB_ATHUB_MISC_CNTL__CG_ENABLE_MASK
    } else {
        def & !ATHUB_ATHUB_MISC_CNTL__CG_ENABLE_MASK
    };
    if def != data {
        wreg32_soc15(adev, ATHUB_HWIP, 0, ATHUB_mmATHUB_MISC_CNTL_BASE_IDX, ATHUB_mmATHUB_MISC_CNTL, data);
    }
}

/// `athub_update_medium_grain_light_sleep`.
fn athub_update_medium_grain_light_sleep(adev: &amdgpu_device, enable: bool) {
    let def = rreg32_soc15(adev, ATHUB_HWIP, 0, ATHUB_mmATHUB_MISC_CNTL_BASE_IDX, ATHUB_mmATHUB_MISC_CNTL).unwrap_or(0);
    let data = if enable && adev.cg_flags & AMD_CG_SUPPORT_MC_LS as u64 != 0
        && adev.cg_flags & AMD_CG_SUPPORT_HDP_LS as u64 != 0
    {
        def | ATHUB_ATHUB_MISC_CNTL__CG_MEM_LS_ENABLE_MASK
    } else {
        def & !ATHUB_ATHUB_MISC_CNTL__CG_MEM_LS_ENABLE_MASK
    };
    if def != data {
        wreg32_soc15(adev, ATHUB_HWIP, 0, ATHUB_mmATHUB_MISC_CNTL_BASE_IDX, ATHUB_mmATHUB_MISC_CNTL, data);
    }
}

/// `athub_v1_0_set_clockgating`: ATHUB 1.5.0.
pub fn athub_v1_0_set_clockgating(adev: &amdgpu_device, gate: bool) -> i32 {
    let v = adev.ip_version(ATHUB_HWIP, 0);
    if [9 << 24, 9 << 24 | 1 << 16, 9 << 24 | 2 << 16, 9 << 24 | 3 << 16, 9 << 24 | 4 << 16, 1 << 24 | 5 << 16]
        .contains(&v)
    {
        athub_update_medium_grain_clock_gating(adev, gate);
        athub_update_medium_grain_light_sleep(adev, gate);
    }
    0
}
