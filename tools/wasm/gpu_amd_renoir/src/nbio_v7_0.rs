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

/// `WREG32_SOC15(ip, inst, reg, v)`.
pub fn wreg32_soc15(adev: &amdgpu_device, hwip: u32, inst: usize, base_idx: u32, reg: u32, v: u32) -> bool {
    match adev.reg_offset[hwip as usize][inst].get(base_idx as usize) {
        Some(base) => adev.wreg32(base + reg, v),
        None => false,
    }
}

/// `nbio_v7_0_init_registers`.
pub fn nbio_v7_0_init_registers(adev: &amdgpu_device) {
    if adev.ip_version(NBIO_HWIP, 0) == 2 << 24 | 5 << 16 {
        let Some(data) = rreg32_soc15(adev, NBIO_HWIP, 0, regRCC_DEV0_EPF6_STRAP4_BASE_IDX, regRCC_DEV0_EPF6_STRAP4)
        else { return };
        let data = data & !(1 << 23);
        wreg32_soc15(adev, NBIO_HWIP, 0, regRCC_DEV0_EPF6_STRAP4_BASE_IDX, regRCC_DEV0_EPF6_STRAP4, data);
    }
}

/// `nbio_v7_0_remap_hdp_registers`.
pub fn nbio_v7_0_remap_hdp_registers(adev: &amdgpu_device) {
    wreg32_soc15(adev, NBIO_HWIP, 0, mmREMAP_HDP_MEM_FLUSH_CNTL_BASE_IDX, mmREMAP_HDP_MEM_FLUSH_CNTL,
        adev.rmmio_remap.reg_offset + KFD_MMIO_REMAP_HDP_MEM_FLUSH_CNTL);
    wreg32_soc15(adev, NBIO_HWIP, 0, mmREMAP_HDP_REG_FLUSH_CNTL_BASE_IDX, mmREMAP_HDP_REG_FLUSH_CNTL,
        adev.rmmio_remap.reg_offset + KFD_MMIO_REMAP_HDP_REG_FLUSH_CNTL);
}

/// `nbio_v7_0_enable_doorbell_aperture`: `WREG32_FIELD15`.
pub fn nbio_v7_0_enable_doorbell_aperture(adev: &amdgpu_device, enable: bool) {
    let (idx, reg) = (mmRCC_DOORBELL_APER_EN_BASE_IDX, mmRCC_DOORBELL_APER_EN);
    let Some(v) = rreg32_soc15(adev, NBIO_HWIP, 0, idx, reg) else { return };
    let v = (v & !RCC_DOORBELL_APER_EN__BIF_DOORBELL_APER_EN_MASK)
        | (enable as u32) << RCC_DOORBELL_APER_EN__BIF_DOORBELL_APER_EN__SHIFT;
    wreg32_soc15(adev, NBIO_HWIP, 0, idx, reg, v);
}

/// `REG_SET_FIELD`.
fn reg_set_field(orig: u32, mask: u32, shift: u32, val: u32) -> u32 {
    (orig & !mask) | (mask & (val << shift))
}

/// `nbio_v7_0_sdma_doorbell_range`.
pub fn nbio_v7_0_sdma_doorbell_range(adev: &amdgpu_device, instance: usize, use_doorbell: bool,
                                     doorbell_index: u32, doorbell_size: u32) {
    let (idx, reg) = if instance == 0 {
        (mmBIF_SDMA0_DOORBELL_RANGE_BASE_IDX, mmBIF_SDMA0_DOORBELL_RANGE)
    } else {
        (mmBIF_SDMA1_DOORBELL_RANGE_BASE_IDX, mmBIF_SDMA1_DOORBELL_RANGE)
    };
    let Some(mut doorbell_range) = rreg32_soc15(adev, NBIO_HWIP, 0, idx, reg) else { return };
    if use_doorbell {
        doorbell_range = reg_set_field(doorbell_range, BIF_SDMA0_DOORBELL_RANGE__OFFSET_MASK,
            BIF_SDMA0_DOORBELL_RANGE__OFFSET__SHIFT, doorbell_index);
        doorbell_range = reg_set_field(doorbell_range, BIF_SDMA0_DOORBELL_RANGE__SIZE_MASK,
            BIF_SDMA0_DOORBELL_RANGE__SIZE__SHIFT, doorbell_size);
    } else {
        doorbell_range = reg_set_field(doorbell_range, BIF_SDMA0_DOORBELL_RANGE__SIZE_MASK,
            BIF_SDMA0_DOORBELL_RANGE__SIZE__SHIFT, 0);
    }
    wreg32_soc15(adev, NBIO_HWIP, 0, idx, reg, doorbell_range);
}

/// `nbio_v7_0_get_pcie_index_offset`: `SOC15_REG_OFFSET(NBIO, 0, mmPCIE_INDEX2)`.
pub fn nbio_v7_0_get_pcie_index_offset(adev: &amdgpu_device) -> Option<u32> {
    Some(*adev.reg_offset[NBIO_HWIP as usize][0].get(mmPCIE_INDEX2_BASE_IDX as usize)? + mmPCIE_INDEX2)
}

/// `nbio_v7_0_get_pcie_data_offset`: `SOC15_REG_OFFSET(NBIO, 0, mmPCIE_DATA2)`.
pub fn nbio_v7_0_get_pcie_data_offset(adev: &amdgpu_device) -> Option<u32> {
    Some(*adev.reg_offset[NBIO_HWIP as usize][0].get(mmPCIE_DATA2_BASE_IDX as usize)? + mmPCIE_DATA2)
}
