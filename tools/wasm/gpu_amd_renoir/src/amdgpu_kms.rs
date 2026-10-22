//! `amdgpu_info_ioctl` (amdgpu_kms.c) and what it reads: the queries a
//! client's Mesa makes. The answer is the bytes Linux copies to user space,
//! at most `return_size` of them.

use crate::amdgpu_device::amdgpu_device;
use crate::regs::*;
use crate::tables;
use alloc::vec;
use alloc::vec::Vec;

const EINVAL: i32 = -22;
const ENOENT: i32 = -2;
const ENODATA: i32 = -61;
const EOPNOTSUPP: i32 = -95;

/// The answer of one query, laid out as its uapi struct.
struct answer(Vec<u8>);

impl answer {
    fn new(size: u32) -> Self {
        answer(vec![0; size as usize])
    }

    fn u32(&mut self, off: u32, v: u32) {
        self.0[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, off: u32, v: u64) {
        self.0[off as usize..off as usize + 8].copy_from_slice(&v.to_le_bytes());
    }

    fn bytes(&mut self, off: u32, b: &[u8]) {
        self.0[off as usize..off as usize + b.len()].copy_from_slice(b);
    }

    /// `copy_to_user(out, &x, min(size, sizeof(x)))`.
    fn copy(mut self, size: u32) -> Vec<u8> {
        self.0.truncate(size as usize);
        self.0
    }
}

fn scalar32(v: u32, size: u32) -> Vec<u8> {
    let mut a = answer::new(4);
    a.u32(0, v);
    a.copy(size)
}

fn scalar64(v: u64, size: u32) -> Vec<u8> {
    let mut a = answer::new(8);
    a.u64(0, v);
    a.copy(size)
}

fn rd32(b: &[u8], off: u32) -> u32 {
    u32::from_le_bytes(b[off as usize..off as usize + 4].try_into().unwrap())
}

/// `amdgpu_ip_get_block_type`.
fn amdgpu_ip_get_block_type(adev: &amdgpu_device, ip: u32) -> u32 {
    match ip {
        AMDGPU_HW_IP_GFX | AMDGPU_HW_IP_COMPUTE => AMD_IP_BLOCK_TYPE_GFX,
        AMDGPU_HW_IP_DMA => AMD_IP_BLOCK_TYPE_SDMA,
        AMDGPU_HW_IP_UVD | AMDGPU_HW_IP_UVD_ENC => AMD_IP_BLOCK_TYPE_UVD,
        AMDGPU_HW_IP_VCE => AMD_IP_BLOCK_TYPE_VCE,
        AMDGPU_HW_IP_VCN_DEC | AMDGPU_HW_IP_VCN_ENC => AMD_IP_BLOCK_TYPE_VCN,
        AMDGPU_HW_IP_VCN_JPEG => {
            if amdgpu_device_ip_get_ip_block(adev, AMD_IP_BLOCK_TYPE_JPEG).is_some() {
                AMD_IP_BLOCK_TYPE_JPEG
            } else {
                AMD_IP_BLOCK_TYPE_VCN
            }
        }
        AMDGPU_HW_IP_VPE => AMD_IP_BLOCK_TYPE_VPE,
        _ => AMD_IP_BLOCK_TYPE_NUM,
    }
}

/// `amdgpu_device_ip_get_ip_block`.
fn amdgpu_device_ip_get_ip_block(adev: &amdgpu_device, type_: u32) -> Option<usize> {
    adev.ip_blocks.iter().position(|b| b.version.type_ == type_)
}

/// Whether ring `r` takes jobs. `no_user_submission` is never set for a
/// GFX9, SDMA 4, VCN 2 or JPEG 2 ring.
fn ring_ready(adev: &amdgpu_device, r: usize) -> bool {
    adev.rings.get(r).is_some_and(|r| r.sched.ready)
}

/// `amdgpu_hw_ip_info`. Renoir has no MES, so there are no user queue
/// slots.
fn amdgpu_hw_ip_info(adev: &amdgpu_device, info: &[u8]) -> Result<answer, i32> {
    let mut out = answer::new(SIZEOF_drm_amdgpu_info_hw_ip);
    let (hw_ip, inst) = (rd32(info, UAPI_INFO_QUERY_HW_IP_TYPE), rd32(info, UAPI_INFO_QUERY_HW_IP_INSTANCE));
    if inst >= AMDGPU_HW_IP_INSTANCE_MAX_COUNT {
        return Err(EINVAL);
    }
    let mut num_rings = 0u32;
    let (type_, ib_start_alignment, ib_size_alignment) = match hw_ip {
        AMDGPU_HW_IP_GFX => {
            num_rings = adev.gfx.gfx_ring.iter().filter(|&&r| ring_ready(adev, r)).count() as u32;
            (AMD_IP_BLOCK_TYPE_GFX, 32, 32)
        }
        AMDGPU_HW_IP_COMPUTE => {
            num_rings = adev.gfx.compute_ring.iter().filter(|&&r| ring_ready(adev, r)).count() as u32;
            (AMD_IP_BLOCK_TYPE_GFX, 32, 32)
        }
        AMDGPU_HW_IP_DMA => {
            num_rings = (0..adev.sdma.num_instances as usize)
                .filter(|&i| adev.sdma.instance[i].ring.is_some_and(|r| ring_ready(adev, r)))
                .count() as u32;
            (AMD_IP_BLOCK_TYPE_SDMA, 256, 4)
        }
        AMDGPU_HW_IP_UVD => (AMD_IP_BLOCK_TYPE_UVD, 256, 64),
        AMDGPU_HW_IP_VCE => (AMD_IP_BLOCK_TYPE_VCE, 256, 4),
        AMDGPU_HW_IP_UVD_ENC => (AMD_IP_BLOCK_TYPE_UVD, 256, 4),
        AMDGPU_HW_IP_VCN_DEC => {
            num_rings = (0..adev.vcn.num_vcn_inst as usize)
                .filter(|&i| adev.vcn.harvest_config & (1 << i) == 0)
                .filter(|&i| adev.vcn.inst[i].ring_dec.is_some_and(|r| ring_ready(adev, r)))
                .count() as u32;
            (AMD_IP_BLOCK_TYPE_VCN, 256, 64)
        }
        AMDGPU_HW_IP_VCN_ENC => {
            num_rings = (0..adev.vcn.num_vcn_inst as usize)
                .filter(|&i| adev.vcn.harvest_config & (1 << i) == 0)
                .map(|i| {
                    let v = &adev.vcn.inst[i];
                    v.ring_enc[..v.num_enc_rings as usize].iter().filter(|r| r.is_some_and(|r| ring_ready(adev, r)))
                        .count() as u32
                })
                .sum();
            (AMD_IP_BLOCK_TYPE_VCN, 256, 4)
        }
        AMDGPU_HW_IP_VCN_JPEG => {
            // Only inst[0].ring_dec[0] exists: JPEG 2.0 has one of each.
            for i in 0..adev.jpeg.num_jpeg_inst {
                if adev.jpeg.harvest_config & (1 << i) != 0 {
                    continue;
                }
                for j in 0..adev.jpeg.num_jpeg_rings {
                    let r = if i == 0 && j == 0 { adev.jpeg.inst0_ring_dec } else { None };
                    if r.is_some_and(|r| ring_ready(adev, r)) {
                        num_rings += 1;
                    }
                }
            }
            (amdgpu_ip_get_block_type(adev, AMDGPU_HW_IP_VCN_JPEG), 256, 64)
        }
        AMDGPU_HW_IP_VPE => (AMD_IP_BLOCK_TYPE_VPE, 256, 4),
        _ => return Err(EINVAL),
    };
    let Some(b) = adev.ip_blocks.iter().find(|b| b.version.type_ == type_ && b.status.valid) else {
        return Ok(out);
    };
    num_rings = num_rings.min(crate::amdgpu_ctx::AMDGPU_CTX_NUM_ENTITIES[hw_ip as usize]);
    out.u32(UAPI_HWIP_HW_IP_VERSION_MAJOR, b.version.major);
    out.u32(UAPI_HWIP_HW_IP_VERSION_MINOR, b.version.minor);
    let disc = match type_ {
        AMD_IP_BLOCK_TYPE_GFX => adev.ip_version(GC_HWIP, 0),
        AMD_IP_BLOCK_TYPE_SDMA => adev.ip_version(SDMA0_HWIP, 0),
        AMD_IP_BLOCK_TYPE_UVD | AMD_IP_BLOCK_TYPE_VCN | AMD_IP_BLOCK_TYPE_JPEG => adev.ip_version(UVD_HWIP, 0),
        AMD_IP_BLOCK_TYPE_VCE => adev.ip_version(VCE_HWIP, 0),
        AMD_IP_BLOCK_TYPE_VPE => adev.ip_version(VPE_HWIP, 0),
        _ => 0,
    };
    // IP_VERSION_MAJ_MIN_REV
    out.u32(UAPI_HWIP_IP_DISCOVERY_VERSION, disc >> 8);
    out.u64(UAPI_HWIP_CAPABILITIES_FLAGS, 0);
    out.u32(UAPI_HWIP_AVAILABLE_RINGS, (1u32 << num_rings) - 1);
    out.u32(UAPI_HWIP_USERQ_NUM_SLOTS, 0);
    out.u32(UAPI_HWIP_IB_START_ALIGNMENT, ib_start_alignment);
    out.u32(UAPI_HWIP_IB_SIZE_ALIGNMENT, ib_size_alignment);
    Ok(out)
}

/// `amdgpu_firmware_info`.
fn amdgpu_firmware_info(adev: &amdgpu_device, info: &[u8]) -> Result<answer, i32> {
    let q = UAPI_INFO_QUERY_FW;
    let (fw_type, index) = (rd32(info, q + UAPI_QUERY_FW_FW_TYPE), rd32(info, q + UAPI_QUERY_FW_INDEX));
    let g = &adev.gfx;
    let psp = &adev.psp;
    let (ver, feature) = match fw_type {
        AMDGPU_INFO_FW_VCE | AMDGPU_INFO_FW_UVD => (0, 0),
        AMDGPU_INFO_FW_VCN => (adev.vcn.fw_version, 0),
        AMDGPU_INFO_FW_GMC => (0, 0),
        AMDGPU_INFO_FW_GFX_ME => (g.me_fw_version, g.me_feature_version),
        AMDGPU_INFO_FW_GFX_PFP => (g.pfp_fw_version, g.pfp_feature_version),
        AMDGPU_INFO_FW_GFX_CE => (g.ce_fw_version, g.ce_feature_version),
        AMDGPU_INFO_FW_GFX_RLC => (g.rlc_fw_version, g.rlc_feature_version),
        AMDGPU_INFO_FW_GFX_RLC_RESTORE_LIST_CNTL => (g.rlc_srlc_fw_version, g.rlc_srlc_feature_version),
        AMDGPU_INFO_FW_GFX_RLC_RESTORE_LIST_GPM_MEM => (g.rlc_srlg_fw_version, g.rlc_srlg_feature_version),
        AMDGPU_INFO_FW_GFX_RLC_RESTORE_LIST_SRM_MEM => (g.rlc_srls_fw_version, g.rlc_srls_feature_version),
        AMDGPU_INFO_FW_GFX_RLCP => (g.rlcp_ucode_version, g.rlcp_ucode_feature_version),
        AMDGPU_INFO_FW_GFX_RLCV => (g.rlcv_ucode_version, g.rlcv_ucode_feature_version),
        AMDGPU_INFO_FW_GFX_MEC => match index {
            0 => (g.mec_fw_version, g.mec_feature_version),
            1 => (g.mec2_fw_version, g.mec2_feature_version),
            _ => return Err(EINVAL),
        },
        AMDGPU_INFO_FW_SMC => (adev.pm.fw_version, 0),
        AMDGPU_INFO_FW_TA => {
            let ctx = match index {
                TA_FW_TYPE_PSP_XGMI => &psp.xgmi_context,
                TA_FW_TYPE_PSP_RAS => &psp.ras_context,
                TA_FW_TYPE_PSP_HDCP => &psp.hdcp_context,
                TA_FW_TYPE_PSP_DTM => &psp.dtm_context,
                TA_FW_TYPE_PSP_RAP => &psp.rap_context,
                TA_FW_TYPE_PSP_SECUREDISPLAY => &psp.securedisplay_context,
                _ => return Err(EINVAL),
            };
            (ctx.context.bin_desc.fw_version, ctx.context.bin_desc.feature_version)
        }
        AMDGPU_INFO_FW_SDMA => {
            if index >= adev.sdma.num_instances {
                return Err(EINVAL);
            }
            let i = &adev.sdma.instance[index as usize];
            (i.fw_version, i.feature_version)
        }
        AMDGPU_INFO_FW_SOS => (psp.sos.fw_version, psp.sos.feature_version),
        AMDGPU_INFO_FW_ASD => (psp.asd_context.bin_desc.fw_version, psp.asd_context.bin_desc.feature_version),
        AMDGPU_INFO_FW_DMCU => (0, 0),
        AMDGPU_INFO_FW_DMCUB => (adev.dm.dmcub_fw_version, 0),
        AMDGPU_INFO_FW_TOC => (psp.toc.fw_version, psp.toc.feature_version),
        AMDGPU_INFO_FW_CAP => (0, 0),
        AMDGPU_INFO_FW_MES_KIQ | AMDGPU_INFO_FW_MES | AMDGPU_INFO_FW_IMU | AMDGPU_INFO_FW_VPE => (0, 0),
        _ => return Err(EINVAL),
    };
    let mut out = answer::new(SIZEOF_drm_amdgpu_info_firmware);
    out.u32(UAPI_FW_VER, ver);
    out.u32(UAPI_FW_FEATURE, feature);
    Ok(out)
}

/// `soc15_read_register`: only the registers of
/// `soc15_allowed_read_registers`.
fn soc15_read_register(adev: &mut amdgpu_device, se_num: u32, sh_num: u32, reg_offset: u32) -> Option<u32> {
    for e in tables::soc15_allowed_read_registers.chunks(5) {
        let (hwip, inst, seg, off, indexed) = (e[0], e[1], e[2], e[3], e[4] != 0);
        let Some(&base) = adev.reg_offset[hwip as usize][inst as usize].get(seg as usize) else { continue };
        if reg_offset != base + off {
            continue;
        }
        return Some(soc15_get_register_value(adev, indexed, se_num, sh_num, reg_offset));
    }
    None
}

/// `soc15_get_register_value`.
fn soc15_get_register_value(adev: &mut amdgpu_device, indexed: bool, se_num: u32, sh_num: u32, reg_offset: u32)
                            -> u32 {
    if indexed {
        if se_num != 0xffffffff || sh_num != 0xffffffff {
            crate::gfx_v9_0::gfx_v9_0_select_se_sh(adev, se_num, sh_num, 0xffffffff);
        }
        let v = adev.rreg32(reg_offset).unwrap_or(0);
        if se_num != 0xffffffff || sh_num != 0xffffffff {
            crate::gfx_v9_0::gfx_v9_0_select_se_sh(adev, 0xffffffff, 0xffffffff, 0xffffffff);
        }
        return v;
    }
    if reg_offset == adev.reg_offset[GC_HWIP as usize][0][mmGB_ADDR_CONFIG_BASE_IDX as usize] + mmGB_ADDR_CONFIG {
        return adev.gfx.config.gb_addr_config;
    }
    if reg_offset == adev.reg_offset[GC_HWIP as usize][0][GC_mmDB_DEBUG2_BASE_IDX as usize] + GC_mmDB_DEBUG2 {
        return adev.gfx.config.db_debug2;
    }
    adev.rreg32(reg_offset).unwrap_or(0)
}

/// `gfx_v9_0_get_gpu_clock_counter` for GC 9.3.0: the SMUIO golden TSC.
fn gfx_v9_0_get_gpu_clock_counter(adev: &amdgpu_device) -> u64 {
    let smuio = |reg: u32| {
        adev.rreg32(adev.reg_offset[SMUIO_HWIP as usize][0][mmGOLDEN_TSC_COUNT_UPPER_Renoir_BASE_IDX as usize] + reg)
            .unwrap_or(0)
    };
    let mut clock_hi = smuio(mmGOLDEN_TSC_COUNT_UPPER_Renoir);
    let mut clock_lo = smuio(mmGOLDEN_TSC_COUNT_LOWER_Renoir);
    let hi_check = smuio(mmGOLDEN_TSC_COUNT_UPPER_Renoir);
    // The SMUIO TSC clock frequency is 100MHz, which sets 32-bit carry over
    // roughly every 42 seconds.
    if hi_check != clock_hi {
        clock_lo = smuio(mmGOLDEN_TSC_COUNT_LOWER_Renoir);
        clock_hi = hi_check;
    }
    clock_lo as u64 | (clock_hi as u64) << 32
}

/// `soc15_get_xclk`: 10000 for MP1 12.0.0.
fn soc15_get_xclk(adev: &amdgpu_device) -> u32 {
    let mp1 = adev.ip_version(MP1_HWIP, 0);
    let v = crate::amdgpu_discovery::ip_version;
    if [v(12, 0, 0), v(12, 0, 1), v(13, 0, 6), v(13, 0, 12), v(13, 0, 14)].contains(&mp1) {
        return 10000;
    }
    if mp1 == v(10, 0, 0) || mp1 == v(10, 0, 1) {
        return adev.clock.spll.reference_freq / 4;
    }
    adev.clock.spll.reference_freq
}

/// `amdgpu_ring_max_ibs`.
fn amdgpu_ring_max_ibs(type_: u32) -> u32 {
    match type_ {
        AMDGPU_RING_TYPE_GFX => 192,
        AMDGPU_RING_TYPE_COMPUTE => 125,
        AMDGPU_RING_TYPE_VCN_JPEG => 16,
        _ => 49,
    }
}

/// `fls`.
fn fls(x: u32) -> u32 {
    32 - x.leading_zeros()
}

fn dev_info(adev: &mut amdgpu_device) -> answer {
    let mut d = answer::new(SIZEOF_drm_amdgpu_info_device);
    let c = &adev.gfx.config;
    d.u32(UAPI_DEV_DEVICE_ID, adev.device as u32);
    d.u32(UAPI_DEV_CHIP_REV, adev.rev_id);
    d.u32(UAPI_DEV_EXTERNAL_REV, adev.external_rev_id);
    d.u32(UAPI_DEV_PCI_REV, adev.revision as u32);
    d.u32(UAPI_DEV_FAMILY, adev.family);
    d.u32(UAPI_DEV_NUM_SHADER_ENGINES, c.max_shader_engines);
    d.u32(UAPI_DEV_NUM_SHADER_ARRAYS_PER_ENGINE, c.max_sh_per_se);
    // return all clocks in KHz
    d.u32(UAPI_DEV_GPU_COUNTER_FREQ, soc15_get_xclk(adev) * 10);
    let (max_e, max_m, min_e, min_m) = if adev.pm.dpm_enabled {
        (crate::amdgpu_smu::amdgpu_dpm_get_sclk(adev, false) * 10, crate::amdgpu_smu::amdgpu_dpm_get_mclk(adev, false) * 10,
         crate::amdgpu_smu::amdgpu_dpm_get_sclk(adev, true) * 10, crate::amdgpu_smu::amdgpu_dpm_get_mclk(adev, true) * 10)
    } else {
        let (s, m) = (adev.clock.default_sclk * 10, adev.clock.default_mclk * 10);
        (s, m, s, m)
    };
    d.u64(UAPI_DEV_MAX_ENGINE_CLOCK, max_e as u64);
    d.u64(UAPI_DEV_MAX_MEMORY_CLOCK, max_m as u64);
    d.u64(UAPI_DEV_MIN_ENGINE_CLOCK, min_e as u64);
    d.u64(UAPI_DEV_MIN_MEMORY_CLOCK, min_m as u64);
    let c = &adev.gfx.config;
    d.u32(UAPI_DEV_ENABLED_RB_PIPES_MASK, c.backend_enable_mask);
    d.u32(UAPI_DEV_NUM_RB_PIPES, c.max_backends_per_se * c.max_shader_engines);
    d.u32(UAPI_DEV_NUM_HW_GFX_CONTEXTS, c.max_hw_contexts);
    let mut ids_flags = 0u64;
    if adev.flags & AMD_IS_APU != 0 {
        ids_flags |= AMDGPU_IDS_FLAGS_FUSION as u64;
    }
    if adev.gfx.mcbp {
        ids_flags |= AMDGPU_IDS_FLAGS_PREEMPTION as u64;
    }
    if adev.gmc.tmz_enabled {
        ids_flags |= AMDGPU_IDS_FLAGS_TMZ as u64;
    }
    // Gang submit is not supported under SRIOV currently; bare metal, and
    // no passthrough.
    ids_flags |= AMDGPU_IDS_FLAGS_GANG_SUBMIT as u64;
    d.u64(UAPI_DEV_IDS_FLAGS, ids_flags);
    let mut vm_size = adev.vm_manager.max_pfn * AMDGPU_GPU_PAGE_SIZE as u64;
    vm_size -= AMDGPU_VA_RESERVED_TOP as u64;
    d.u64(UAPI_DEV_VIRTUAL_ADDRESS_OFFSET, AMDGPU_VA_RESERVED_BOTTOM as u64);
    d.u64(UAPI_DEV_VIRTUAL_ADDRESS_MAX, vm_size.min(AMDGPU_GMC_HOLE_START));
    if vm_size > AMDGPU_GMC_HOLE_START {
        d.u64(UAPI_DEV_HIGH_VA_OFFSET, AMDGPU_GMC_HOLE_END);
        d.u64(UAPI_DEV_HIGH_VA_MAX, AMDGPU_GMC_HOLE_END | vm_size);
    }
    let page = (crate::amdgpu_object::PAGE_SIZE as u32).max(AMDGPU_GPU_PAGE_SIZE);
    d.u32(UAPI_DEV_VIRTUAL_ADDRESS_ALIGNMENT, page);
    d.u32(UAPI_DEV_PTE_FRAGMENT_SIZE, (1 << adev.vm_manager.fragment_size) * AMDGPU_GPU_PAGE_SIZE);
    d.u32(UAPI_DEV_GART_PAGE_SIZE, page);
    let cu = &adev.gfx.cu_info;
    d.u32(UAPI_DEV_CU_ACTIVE_NUMBER, cu.number);
    d.u32(UAPI_DEV_CU_AO_MASK, cu.ao_cu_mask);
    d.u32(UAPI_DEV_CE_RAM_SIZE, adev.gfx.ce_ram_size);
    for (i, row) in cu.ao_cu_bitmap.iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            d.u32(UAPI_DEV_CU_AO_BITMAP + ((i * 4 + j) * 4) as u32, v);
        }
    }
    for (i, row) in cu.bitmap[0].iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            d.u32(UAPI_DEV_CU_BITMAP + ((i * 4 + j) * 4) as u32, v);
        }
    }
    d.u32(UAPI_DEV_VRAM_TYPE, adev.gmc.vram_type);
    d.u32(UAPI_DEV_VRAM_BIT_WIDTH, adev.gmc.vram_width);
    d.u32(UAPI_DEV_VCE_HARVEST_CONFIG, 0);
    let c = &adev.gfx.config;
    d.u32(UAPI_DEV_GC_DOUBLE_OFFCHIP_LDS_BUF, c.double_offchip_lds_buf);
    d.u32(UAPI_DEV_WAVE_FRONT_SIZE, cu.wave_front_size);
    d.u32(UAPI_DEV_NUM_SHADER_VISIBLE_VGPRS, c.max_gprs);
    d.u32(UAPI_DEV_NUM_CU_PER_SH, c.max_cu_per_sh);
    d.u32(UAPI_DEV_NUM_TCC_BLOCKS, c.max_texture_channel_caches);
    d.u32(UAPI_DEV_GS_VGT_TABLE_DEPTH, c.gs_vgt_table_depth);
    d.u32(UAPI_DEV_GS_PRIM_BUFFER_DEPTH, c.gs_prim_buffer_depth);
    d.u32(UAPI_DEV_MAX_GS_WAVES_PER_VGT, c.max_gs_threads);
    // pa_sc_tile_steering_override is AMDGPU_FAMILY_NV and later; the TCC
    // disable mask is not read on GFX9.
    d.u64(UAPI_DEV_TCC_DISABLED_MASK, 0);
    // Combine the chip gen mask with the platform (CPU/mobo) mask.
    let pm = &adev.pm;
    let pcie_gen_mask = pm.pcie_gen_mask & (pm.pcie_gen_mask >> CAIL_PCIE_LINK_SPEED_SUPPORT_SHIFT);
    let pcie_width_mask = pm.pcie_mlw_mask & (pm.pcie_mlw_mask >> CAIL_PCIE_LINK_WIDTH_SUPPORT_SHIFT);
    d.u32(UAPI_DEV_PCIE_GEN, fls(pcie_gen_mask));
    let lanes = [
        (CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X32, 32),
        (CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X16, 16),
        (CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X12, 12),
        (CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X8, 8),
        (CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X4, 4),
        (CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X2, 2),
    ];
    d.u32(UAPI_DEV_PCIE_NUM_LANES, lanes.iter().find(|(m, _)| pcie_width_mask & m != 0).map_or(1, |l| l.1));
    d.u32(UAPI_DEV_TCP_CACHE_SIZE, c.gc_tcp_l1_size);
    d.u32(UAPI_DEV_NUM_SQC_PER_WGP, c.gc_num_sqc_per_wgp);
    d.u32(UAPI_DEV_SQC_DATA_CACHE_SIZE, c.gc_l1_data_cache_size_per_sqc);
    d.u32(UAPI_DEV_SQC_INST_CACHE_SIZE, c.gc_l1_instruction_cache_size_per_sqc);
    d.u32(UAPI_DEV_GL1C_CACHE_SIZE, c.gc_gl1c_size_per_instance * c.gc_gl1c_per_sa);
    d.u32(UAPI_DEV_GL2C_CACHE_SIZE, c.gc_gl2c_per_gpu);
    d.u64(UAPI_DEV_MALL_SIZE, adev.gmc.mall_size);
    // GFX9 has no `get_gfx_shadow_info` and no user queues.
    d.u32(UAPI_DEV_USERQ_IP_MASK, 0);
    d
}

/// `drm_amdgpu_memory_info.vram` and the sizes `AMDGPU_INFO_VRAM_GTT`
/// shares with it.
fn usable_vram(adev: &amdgpu_device) -> u64 {
    adev.gmc.real_vram_size - adev.vram_pin_size - AMDGPU_VM_RESERVED_VRAM as u64
}

fn vram_usage(adev: &amdgpu_device) -> u64 {
    adev.mman.vram_mgr.as_ref().filter(|m| m.manager.use_type).map_or(0, |m| m.manager.usage)
}

fn vis_vram_usage(adev: &amdgpu_device) -> u64 {
    adev.mman.vram_mgr.as_ref().map_or(0, |m| m.vis_usage)
}

fn gtt_manager(adev: &amdgpu_device) -> (u64, u64) {
    adev.mman.gtt_mgr.as_ref().map_or((0, 0), |m| (m.manager.size, m.manager.usage))
}

/// `amdgpu_info_ioctl` of a client with VM `vm`: the bytes for its
/// `return_pointer`, at most `return_size`.
pub fn amdgpu_info_ioctl(adev: &mut amdgpu_device, vm: usize, info: &[u8]) -> Result<Vec<u8>, i32> {
    if info.len() < SIZEOF_drm_amdgpu_info as usize {
        return Err(EINVAL);
    }
    let size = rd32(info, UAPI_INFO_RETURN_SIZE);
    if size == 0 {
        return Err(EINVAL);
    }
    let query = rd32(info, UAPI_INFO_QUERY);
    match query {
        AMDGPU_INFO_ACCEL_WORKING => Ok(scalar32(adev.accel_working as u32, size)),
        // No CRTC is the module's: the display belongs to the firmware.
        AMDGPU_INFO_CRTC_FROM_ID => Err(EINVAL),
        AMDGPU_INFO_HW_IP_INFO => Ok(amdgpu_hw_ip_info(adev, info)?.copy(size)),
        AMDGPU_INFO_HW_IP_COUNT => {
            let type_ = amdgpu_ip_get_block_type(adev, rd32(info, UAPI_INFO_QUERY_HW_IP_TYPE));
            let valid = amdgpu_device_ip_get_ip_block(adev, type_).is_some_and(|i| adev.ip_blocks[i].status.valid);
            if !valid {
                return Err(EINVAL);
            }
            let count = match type_ {
                AMD_IP_BLOCK_TYPE_GFX | AMD_IP_BLOCK_TYPE_VCE => 1,
                AMD_IP_BLOCK_TYPE_SDMA => adev.sdma.num_instances,
                AMD_IP_BLOCK_TYPE_JPEG => adev.jpeg.num_jpeg_inst * adev.jpeg.num_jpeg_rings,
                AMD_IP_BLOCK_TYPE_VCN => adev.vcn.num_vcn_inst,
                AMD_IP_BLOCK_TYPE_UVD => 0,
                AMD_IP_BLOCK_TYPE_VPE => adev.vpe.num_instances,
                _ => 1,
            };
            Ok(scalar32(count, size))
        }
        AMDGPU_INFO_TIMESTAMP => Ok(scalar64(gfx_v9_0_get_gpu_clock_counter(adev), size)),
        AMDGPU_INFO_FW_VERSION => {
            // We only support one instance of each IP block right now.
            if rd32(info, UAPI_INFO_QUERY_FW + UAPI_QUERY_FW_IP_INSTANCE) != 0 {
                return Err(EINVAL);
            }
            Ok(amdgpu_firmware_info(adev, info)?.copy(size))
        }
        AMDGPU_INFO_NUM_BYTES_MOVED => Ok(scalar64(adev.num_bytes_moved, size)),
        AMDGPU_INFO_NUM_EVICTIONS => Ok(scalar64(adev.num_evictions, size)),
        AMDGPU_INFO_NUM_VRAM_CPU_PAGE_FAULTS => Ok(scalar64(adev.num_vram_cpu_page_faults, size)),
        AMDGPU_INFO_VRAM_USAGE => Ok(scalar64(vram_usage(adev), size)),
        AMDGPU_INFO_VIS_VRAM_USAGE => Ok(scalar64(vis_vram_usage(adev), size)),
        AMDGPU_INFO_GTT_USAGE => Ok(scalar64(gtt_manager(adev).1, size)),
        AMDGPU_INFO_GDS_CONFIG => {
            let mut g = answer::new(SIZEOF_drm_amdgpu_info_gds);
            g.u32(UAPI_GDS_COMPUTE_PARTITION_SIZE, adev.gds.gds_size);
            g.u32(UAPI_GDS_GDS_TOTAL_SIZE, adev.gds.gds_size);
            g.u32(UAPI_GDS_GWS_PER_COMPUTE_PARTITION, adev.gds.gws_size);
            g.u32(UAPI_GDS_OA_PER_COMPUTE_PARTITION, adev.gds.oa_size);
            Ok(g.copy(size))
        }
        AMDGPU_INFO_VRAM_GTT => {
            let vram = usable_vram(adev);
            let mut v = answer::new(SIZEOF_drm_amdgpu_info_vram_gtt);
            v.u64(UAPI_VRAMGTT_VRAM_SIZE, vram);
            v.u64(UAPI_VRAMGTT_VRAM_CPU_ACCESSIBLE_SIZE, (adev.gmc.visible_vram_size - adev.visible_pin_size).min(vram));
            v.u64(UAPI_VRAMGTT_GTT_SIZE, gtt_manager(adev).0 - adev.gart_pin_size);
            Ok(v.copy(size))
        }
        AMDGPU_INFO_MEMORY => {
            let mut m = answer::new(SIZEOF_drm_amdgpu_memory_info);
            let heap = |m: &mut answer, at: u32, total: u64, usable: u64, usage: u64| {
                m.u64(at + UAPI_HEAP_TOTAL_HEAP_SIZE, total);
                m.u64(at + UAPI_HEAP_USABLE_HEAP_SIZE, usable);
                m.u64(at + UAPI_HEAP_HEAP_USAGE, usage);
                m.u64(at + UAPI_HEAP_MAX_ALLOCATION, usable * 3 / 4);
            };
            let vram = usable_vram(adev);
            heap(&mut m, UAPI_MEM_VRAM, adev.gmc.real_vram_size, vram, vram_usage(adev));
            heap(&mut m, UAPI_MEM_CPU_ACCESSIBLE_VRAM, adev.gmc.visible_vram_size,
                 (adev.gmc.visible_vram_size - adev.visible_pin_size).min(vram), vis_vram_usage(adev));
            let (gtt_size, gtt_usage) = gtt_manager(adev);
            heap(&mut m, UAPI_MEM_GTT, gtt_size, gtt_size - adev.gart_pin_size, gtt_usage);
            Ok(m.copy(size))
        }
        AMDGPU_INFO_READ_MMR_REG => {
            let instance = rd32(info, UAPI_INFO_READ_MMR_INSTANCE);
            let mut se_num = (instance >> AMDGPU_INFO_MMR_SE_INDEX_SHIFT) & AMDGPU_INFO_MMR_SE_INDEX_MASK;
            let mut sh_num = (instance >> AMDGPU_INFO_MMR_SH_INDEX_SHIFT) & AMDGPU_INFO_MMR_SH_INDEX_MASK;
            // set full masks if the userspace set all bits in the bitfields
            if se_num == AMDGPU_INFO_MMR_SE_INDEX_MASK {
                se_num = 0xffffffff;
            } else if se_num >= AMDGPU_GFX_MAX_SE {
                return Err(EINVAL);
            }
            if sh_num == AMDGPU_INFO_MMR_SH_INDEX_MASK {
                sh_num = 0xffffffff;
            } else if sh_num >= AMDGPU_GFX_MAX_SH_PER_SE {
                return Err(EINVAL);
            }
            let count = rd32(info, UAPI_INFO_READ_MMR_COUNT);
            if count > 128 {
                return Err(EINVAL);
            }
            let first = rd32(info, UAPI_INFO_READ_MMR_DWORD_OFFSET);
            let mut regs = answer::new(count * 4);
            crate::gfx_v9_0::amdgpu_gfx_off_ctrl(adev, false);
            let mut ret = Ok(());
            for i in 0..count {
                match soc15_read_register(adev, se_num, sh_num, first.wrapping_add(i)) {
                    Some(v) => regs.u32(i * 4, v),
                    None => {
                        ret = Err(-14);
                        break;
                    }
                }
            }
            crate::gfx_v9_0::amdgpu_gfx_off_ctrl(adev, true);
            ret.map(|_| regs.copy(size))
        }
        AMDGPU_INFO_DEV_INFO => Ok(dev_info(adev).copy(size)),
        // The SMU has no VCE clock states.
        AMDGPU_INFO_VCE_CLOCK_TABLE => Ok(answer::new(SIZEOF_drm_amdgpu_info_vce_clock_table).copy(size)),
        AMDGPU_INFO_VBIOS => {
            let bios_size = adev.bios.len() as u32;
            match rd32(info, UAPI_INFO_VBIOS_TYPE) {
                AMDGPU_INFO_VBIOS_SIZE => Ok(scalar32(bios_size, size)),
                AMDGPU_INFO_VBIOS_IMAGE => {
                    let bios_offset = rd32(info, UAPI_INFO_VBIOS_OFFSET);
                    if bios_offset >= bios_size {
                        return Err(EINVAL);
                    }
                    let n = size.min(bios_size - bios_offset) as usize;
                    Ok(adev.bios[bios_offset as usize..bios_offset as usize + n].to_vec())
                }
                AMDGPU_INFO_VBIOS_INFO => {
                    let mut v = answer::new(SIZEOF_drm_amdgpu_info_vbios);
                    if let Some(a) = adev.mode_info.atom_context.as_ref() {
                        v.bytes(UAPI_VBIOS_NAME, &a.name);
                        v.bytes(UAPI_VBIOS_VBIOS_PN, &a.vbios_pn);
                        v.u32(UAPI_VBIOS_VERSION, a.version);
                        v.bytes(UAPI_VBIOS_VBIOS_VER_STR, &a.vbios_ver_str);
                        v.bytes(UAPI_VBIOS_DATE, &a.date);
                    }
                    Ok(v.copy(size))
                }
                _ => Err(EINVAL),
            }
        }
        AMDGPU_INFO_NUM_HANDLES => match rd32(info, UAPI_INFO_QUERY_HW_IP_TYPE) {
            // Starting Polaris, we support unlimited UVD handles
            AMDGPU_HW_IP_UVD => Err(ENODATA),
            _ => Err(EINVAL),
        },
        AMDGPU_INFO_SENSOR => {
            if !adev.pm.dpm_enabled {
                return Err(ENOENT);
            }
            let mut v = 0u32;
            let mut read = |adev: &mut amdgpu_device, s| {
                if crate::amdgpu_smu::amdgpu_dpm_read_sensor(adev, s, &mut v) != 0 { Err(EINVAL) } else { Ok(v) }
            };
            let v = match rd32(info, UAPI_INFO_SENSOR_TYPE) {
                // get sclk in Mhz
                AMDGPU_INFO_SENSOR_GFX_SCLK => read(adev, AMDGPU_PP_SENSOR_GFX_SCLK)? / 100,
                // get mclk in Mhz
                AMDGPU_INFO_SENSOR_GFX_MCLK => read(adev, AMDGPU_PP_SENSOR_GFX_MCLK)? / 100,
                // get temperature in millidegrees C
                AMDGPU_INFO_SENSOR_GPU_TEMP => read(adev, AMDGPU_PP_SENSOR_GPU_TEMP)?,
                AMDGPU_INFO_SENSOR_GPU_LOAD => read(adev, AMDGPU_PP_SENSOR_GPU_LOAD)?,
                // get average GPU power, falling back to input power
                AMDGPU_INFO_SENSOR_GPU_AVG_POWER => match read(adev, AMDGPU_PP_SENSOR_GPU_AVG_POWER) {
                    Ok(p) => p >> 8,
                    Err(_) => read(adev, AMDGPU_PP_SENSOR_GPU_INPUT_POWER)? >> 8,
                },
                AMDGPU_INFO_SENSOR_GPU_INPUT_POWER => read(adev, AMDGPU_PP_SENSOR_GPU_INPUT_POWER)? >> 8,
                // in millivolts
                AMDGPU_INFO_SENSOR_VDDNB => read(adev, AMDGPU_PP_SENSOR_VDDNB)?,
                AMDGPU_INFO_SENSOR_VDDGFX => read(adev, AMDGPU_PP_SENSOR_VDDGFX)?,
                AMDGPU_INFO_SENSOR_STABLE_PSTATE_GFX_SCLK => read(adev, AMDGPU_PP_SENSOR_STABLE_PSTATE_SCLK)? / 100,
                AMDGPU_INFO_SENSOR_STABLE_PSTATE_GFX_MCLK => read(adev, AMDGPU_PP_SENSOR_STABLE_PSTATE_MCLK)? / 100,
                AMDGPU_INFO_SENSOR_PEAK_PSTATE_GFX_SCLK => read(adev, AMDGPU_PP_SENSOR_PEAK_PSTATE_SCLK)? / 100,
                AMDGPU_INFO_SENSOR_PEAK_PSTATE_GFX_MCLK => read(adev, AMDGPU_PP_SENSOR_PEAK_PSTATE_MCLK)? / 100,
                _ => return Err(EINVAL),
            };
            Ok(scalar32(v, size))
        }
        AMDGPU_INFO_VRAM_LOST_COUNTER => Ok(scalar32(adev.vram_lost_counter, size)),
        // Renoir is no RAS ASIC: there is no RAS context.
        AMDGPU_INFO_RAS_ENABLED_FEATURES => Err(EINVAL),
        AMDGPU_INFO_VIDEO_CAPS => {
            // soc15_query_video_codecs: no VCE, UVD (VCN) 2.2.0.
            let codecs: &[u32] = match adev.ip_version(UVD_HWIP, 0) {
                v if v == crate::amdgpu_discovery::ip_version(2, 2, 0)
                    || v == crate::amdgpu_discovery::ip_version(2, 5, 0)
                    || v == crate::amdgpu_discovery::ip_version(2, 6, 0) => match rd32(info, UAPI_INFO_VIDEO_CAP_TYPE) {
                    AMDGPU_INFO_VIDEO_CAPS_DECODE => &tables::rn_video_codecs_decode_array,
                    AMDGPU_INFO_VIDEO_CAPS_ENCODE => &tables::vega_video_codecs_encode_array,
                    _ => return Err(EINVAL),
                },
                _ => return Err(EINVAL),
            };
            let mut caps = answer::new(SIZEOF_drm_amdgpu_info_video_caps);
            for c in codecs.chunks(5) {
                let idx = c[0];
                if idx < AMDGPU_INFO_VIDEO_CAPS_CODEC_IDX_COUNT {
                    let at = UAPI_VCAPS_CODEC_INFO + idx * SIZEOF_drm_amdgpu_info_video_codec_info;
                    caps.u32(at + UAPI_CODEC_VALID, 1);
                    caps.u32(at + UAPI_CODEC_MAX_WIDTH, c[1]);
                    caps.u32(at + UAPI_CODEC_MAX_HEIGHT, c[2]);
                    caps.u32(at + UAPI_CODEC_MAX_PIXELS_PER_FRAME, c[3]);
                    caps.u32(at + UAPI_CODEC_MAX_LEVEL, c[4]);
                }
            }
            Ok(caps.copy(size))
        }
        AMDGPU_INFO_MAX_IBS => {
            let mut m = answer::new(AMDGPU_HW_IP_NUM * 4);
            for i in 0..AMDGPU_HW_IP_NUM {
                m.u32(i * 4, amdgpu_ring_max_ibs(i));
            }
            Ok(m.copy(size))
        }
        AMDGPU_INFO_GPUVM_FAULT => {
            let f = adev.vm(vm).fault_info;
            let mut g = answer::new(SIZEOF_drm_amdgpu_info_gpuvm_fault);
            g.u64(UAPI_VMFAULT_ADDR, f.addr);
            g.u32(UAPI_VMFAULT_STATUS, f.status);
            g.u32(UAPI_VMFAULT_VMHUB, f.vmhub);
            Ok(g.copy(size))
        }
        // GFX9 has no `get_gfx_shadow_info`.
        AMDGPU_INFO_UQ_FW_AREAS => match rd32(info, UAPI_INFO_QUERY_HW_IP_TYPE) {
            AMDGPU_HW_IP_GFX => Err(EOPNOTSUPP),
            _ => Err(EINVAL),
        },
        _ => Err(EINVAL),
    }
}
