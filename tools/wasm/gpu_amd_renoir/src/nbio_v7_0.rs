//! amdgpu/nbio_v7_0.c (Linux 6.18.55): the NBIO functions of NBIO 7.0.0,
//! 7.0.1 and 2.5.0 (`amdgpu_discovery_set_ip_blocks` sets `nbio_v7_0_funcs`
//! for them).

use crate::amdgpu_device::amdgpu_device;
use crate::regs::*;

/// `RREG32_SOC15(ip, inst, reg)`: the register at the IP's base for its
/// segment (`SOC15_REG_OFFSET`).
pub fn rreg32_soc15(adev: &amdgpu_device, hwip: u32, inst: usize, base_idx: u32, reg: u32) -> Option<u32> {
    let base = *adev.reg_offset[hwip as usize][inst].get(base_idx as usize)?;
    adev.rreg32(base + reg)
}

/// `nbio_v7_0_get_rev_id`.
pub fn nbio_v7_0_get_rev_id(adev: &amdgpu_device) -> Option<u32> {
    let tmp = rreg32_soc15(adev, NBIO_HWIP, 0, mmRCC_DEV0_EPF0_STRAP0_BASE_IDX, mmRCC_DEV0_EPF0_STRAP0)?;
    Some((tmp & RCC_DEV0_EPF0_STRAP0__STRAP_ATI_REV_ID_DEV0_F0_MASK) >> RCC_DEV0_EPF0_STRAP0__STRAP_ATI_REV_ID_DEV0_F0__SHIFT)
}

/// `nbio_v7_0_set_reg_remap`: bare metal with 4 KiB pages, so the MMIO hole.
pub fn nbio_v7_0_set_reg_remap(adev: &mut amdgpu_device) {
    adev.rmmio_remap.reg_offset = MMIO_REG_HOLE_OFFSET;
    adev.rmmio_remap.bus_addr = adev.rmmio_base + MMIO_REG_HOLE_OFFSET as u64;
}
