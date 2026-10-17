//! amdgpu/amdgpu_device.c (Linux 6.18.55): the device, register access and
//! VRAM access as they work before the IP blocks are known.

use crate::regs::*;
use alloc::vec::Vec;
use npk_sys::Mmio;

/// The table sizes of `adev->ip_versions` and `adev->reg_offset` (amdgpu.h).
pub const MAX_HWIP: usize = crate::regs::MAX_HWIP as usize;
pub const HWIP_MAX_INSTANCE: usize = crate::regs::HWIP_MAX_INSTANCE as usize;

/// `struct amdgpu_mman`, the part `amdgpu_discovery_init` fills.
pub struct amdgpu_mman {
    pub discovery_tmr_size: u32,
    pub discovery_bin: Vec<u8>,
}

/// `struct amdgpu_device`, the part the discovery needs.
pub struct amdgpu_device {
    /// `pdev->device`.
    pub device: u16,
    pub revision: u8,
    pub apu_flags: u32,
    /// `adev->rmmio`, BAR5, and its length.
    pub rmmio: Mmio,
    pub rmmio_size: u32,
    pub mman: amdgpu_mman,
    /// `adev->ip_versions`, `IP_VERSION_FULL` encoded.
    pub ip_versions: [[u32; HWIP_MAX_INSTANCE]; MAX_HWIP],
    /// `adev->reg_offset[hw_ip][inst]`: the IP's base addresses, in dwords.
    pub reg_offset: [[Vec<u32>; HWIP_MAX_INSTANCE]; MAX_HWIP],
    pub num_vcn_inst: u32,
    pub vcn_inst_mask: u32,
    pub jpeg_inst_mask: u32,
    pub sdma_num_instances: u32,
    pub sdma_mask: u32,
    pub gfx_xcc_mask: u32,
    pub num_umc: u32,
    pub umc_node_inst_num: u32,
    pub vpe_num_instances: u32,
}

impl amdgpu_device {
    pub fn new(device: u16, revision: u8, rmmio: Mmio, rmmio_size: u32) -> Self {
        amdgpu_device {
            device,
            revision,
            apu_flags: 0,
            rmmio,
            rmmio_size,
            mman: amdgpu_mman { discovery_tmr_size: 0, discovery_bin: Vec::new() },
            ip_versions: [[0; HWIP_MAX_INSTANCE]; MAX_HWIP],
            reg_offset: core::array::from_fn(|_| core::array::from_fn(|_| Vec::new())),
            num_vcn_inst: 0,
            vcn_inst_mask: 0,
            jpeg_inst_mask: 0,
            sdma_num_instances: 0,
            sdma_mask: 0,
            gfx_xcc_mask: 0,
            num_umc: 0,
            umc_node_inst_num: 0,
            vpe_num_instances: 0,
        }
    }

    /// `amdgpu_device_rreg`: a register inside BAR5 is read there; one past
    /// it goes through the PCIe index/data pair (`adev->pcie_rreg`), which
    /// NBIO provides once its IP block is set and is not ported yet.
    pub fn rreg32(&self, reg: u32) -> Option<u32> {
        if (reg as u64) * 4 < self.rmmio_size as u64 {
            Some(self.rmmio.r32(reg * 4))
        } else {
            None
        }
    }

    /// `amdgpu_device_wreg`, the BAR5 part (see `rreg32`).
    pub fn wreg32(&self, reg: u32, v: u32) -> bool {
        if (reg as u64) * 4 < self.rmmio_size as u64 {
            self.rmmio.w32(reg * 4, v);
            true
        } else {
            false
        }
    }

    /// `amdgpu_device_mm_access`: VRAM through MM_INDEX/MM_INDEX_HI/MM_DATA.
    /// `pos` and `buf.len()` are multiples of 4.
    pub fn mm_access_read(&self, mut pos: u64, buf: &mut [u8]) {
        let mut hi: u32 = !0;
        let last = pos + buf.len() as u64;
        let mut at = 0;
        while pos < last {
            let tmp = (pos >> 31) as u32;
            self.wreg32(mmMM_INDEX, (pos as u32) | 0x8000_0000);
            if tmp != hi {
                self.wreg32(mmMM_INDEX_HI, tmp);
                hi = tmp;
            }
            let v = self.rreg32(mmMM_DATA).unwrap_or(0);
            buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
            at += 4;
            pos += 4;
        }
    }

    /// `amdgpu_device_vram_access`, reading: the aperture is not mapped this
    /// early (`adev->mman.aper_base_kaddr` is set by the GMC), so all of it
    /// goes through `amdgpu_device_mm_access`.
    pub fn vram_access_read(&self, pos: u64, buf: &mut [u8]) {
        self.mm_access_read(pos, buf);
    }
}

/// `amdgpu_device_ip_early_init`: the APU flags of CHIP_RENOIR.
pub fn amdgpu_device_renoir_apu_flags(adev: &mut amdgpu_device) {
    if adev.device == 0x1636 || adev.device == 0x164c {
        adev.apu_flags |= AMD_APU_IS_RENOIR;
    } else {
        adev.apu_flags |= AMD_APU_IS_GREEN_SARDINE;
    }
}
