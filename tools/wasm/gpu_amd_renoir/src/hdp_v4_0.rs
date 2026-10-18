//! amdgpu/hdp_v4_0.c (Linux 6.18.55): the host data path's init.

use crate::amdgpu_device::amdgpu_device;
use crate::nbio_v7_0::{rreg32_soc15, wreg32_soc15};
use crate::regs::*;

/// `WREG32_FIELD15(HDP, 0, reg, field, val)`.
fn field15(adev: &amdgpu_device, idx: u32, reg: u32, mask: u32, shift: u32, val: u32) {
    let v = rreg32_soc15(adev, HDP_HWIP, 0, idx, reg).unwrap_or(0);
    wreg32_soc15(adev, HDP_HWIP, 0, idx, reg, (v & !mask) | val << shift);
}

/// `hdp_v4_0_init_registers`: HDP 4.1.2 is neither 4.2.1 nor 4.4.0.
pub fn hdp_v4_0_init_registers(adev: &amdgpu_device) {
    let v = adev.ip_version(HDP_HWIP, 0);
    if v == 4 << 24 | 2 << 16 | 1 << 8 {
        field15(adev, HDP_mmHDP_MMHUB_CNTL_BASE_IDX, HDP_mmHDP_MMHUB_CNTL, HDP_HDP_MMHUB_CNTL__HDP_MMHUB_GCC_MASK,
                HDP_HDP_MMHUB_CNTL__HDP_MMHUB_GCC__SHIFT, 1);
    }
    field15(adev, HDP_mmHDP_MISC_CNTL_BASE_IDX, HDP_mmHDP_MISC_CNTL, HDP_HDP_MISC_CNTL__FLUSH_INVALIDATE_CACHE_MASK,
            HDP_HDP_MISC_CNTL__FLUSH_INVALIDATE_CACHE__SHIFT, 1);
    if v == 4 << 24 | 4 << 16 {
        field15(adev, HDP_mmHDP_MISC_CNTL_BASE_IDX, HDP_mmHDP_MISC_CNTL, HDP_HDP_MISC_CNTL__READ_BUFFER_WATERMARK_MASK,
                HDP_HDP_MISC_CNTL__READ_BUFFER_WATERMARK__SHIFT, 2);
    }
    wreg32_soc15(adev, HDP_HWIP, 0, HDP_mmHDP_NONSURFACE_BASE_BASE_IDX, HDP_mmHDP_NONSURFACE_BASE,
                 (adev.gmc.vram_start >> 8) as u32);
    wreg32_soc15(adev, HDP_HWIP, 0, HDP_mmHDP_NONSURFACE_BASE_HI_BASE_IDX, HDP_mmHDP_NONSURFACE_BASE_HI,
                 (adev.gmc.vram_start >> 40) as u32);
}
