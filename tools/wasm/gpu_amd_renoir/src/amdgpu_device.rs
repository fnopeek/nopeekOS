//! amdgpu/amdgpu_device.c (Linux 6.18.55): the device, register access and
//! VRAM access as they work before the IP blocks are known.

use crate::amdgpu_psp::psp_context;
use crate::amdgpu_smu::smu_context;
use crate::amdgpu_ucode::amdgpu_firmware;
use crate::atom::atom_context;
use crate::gfx_v9_0::{amdgpu_gds, amdgpu_rlc};
use crate::gmc_v9_0::amdgpu_irq_src;
use crate::linux::firmware;
use crate::sdma_v4_0::amdgpu_sdma_instance;
use crate::ip_blocks::{amdgpu_ip_block_version, ip_block_name};
use crate::linux::Line;
use crate::regs::*;
use crate::tables::amdgpu_dc;
use alloc::vec::Vec;
use npk_sys::Mmio;

/// The table sizes of `adev->ip_versions` and `adev->reg_offset` (amdgpu.h).
pub const MAX_HWIP: usize = crate::regs::MAX_HWIP as usize;
pub const HWIP_MAX_INSTANCE: usize = crate::regs::HWIP_MAX_INSTANCE as usize;

/// `struct amdgpu_mman`, the part `amdgpu_discovery_init` and
/// `amdgpu_atomfirmware_allocate_fb_scratch` fill.
pub struct amdgpu_mman {
    pub discovery_tmr_size: u32,
    pub discovery_bin: Vec<u8>,
    pub fw_vram_usage_start_offset: u64,
    pub fw_vram_usage_size: u64,
    pub drv_vram_usage_start_offset: u64,
    pub drv_vram_usage_size: u64,
    pub buffer_funcs: Option<&'static str>,
    pub buffer_funcs_ring: Option<&'static str>,
}

/// `struct amdgpu_mode_info`, the ATOM part and what DM sets.
#[derive(Default)]
pub struct amdgpu_mode_info {
    pub atom_context: Option<atom_context>,
    pub firmware_flags: u32,
    pub num_crtc: u32,
    pub num_hpd: u32,
    pub num_dig: u32,
    pub funcs: Option<&'static str>,
}

/// `struct amdgpu_pll`, the fields `amdgpu_atomfirmware_get_clock_info` sets.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_pll {
    pub reference_freq: u32,
    pub reference_div: u32,
    pub min_post_div: u32,
    pub max_post_div: u32,
    pub min_ref_div: u32,
    pub max_ref_div: u32,
    pub min_feedback_div: u32,
    pub max_feedback_div: u32,
    pub best_vco: u32,
}

/// `struct amdgpu_clock`: boot clocks in 10 kHz.
#[derive(Default)]
pub struct amdgpu_clock {
    pub spll: amdgpu_pll,
    pub mpll: amdgpu_pll,
    pub default_sclk: u32,
    pub default_mclk: u32,
}

/// `struct amdgpu_pm`, the current clocks and the PowerPlay features.
#[derive(Default)]
pub struct amdgpu_pm {
    pub current_sclk: u32,
    pub current_mclk: u32,
    pub pp_feature: u32,
}

/// `struct amdgpu_ip_block_status`.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_ip_block_status {
    pub valid: bool,
    pub sw: bool,
    pub hw: bool,
    pub late_initialized: bool,
    pub hang: bool,
}

/// `struct amdgpu_ip_block`.
pub struct amdgpu_ip_block {
    pub status: amdgpu_ip_block_status,
    pub version: &'static amdgpu_ip_block_version,
}

/// The `funcs` tables `amdgpu_discovery_set_ip_blocks` picks by IP version,
/// each the one Renoir's version selects.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum amdgpu_nbio_funcs { nbio_v7_0 }
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum amdgpu_hdp_funcs { hdp_v4_0 }
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum amdgpu_df_funcs { df_v1_7 }
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum amdgpu_smuio_funcs { smuio_v9_0 }
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum amdgpu_asic_funcs { soc15 }

/// `adev->rmmio_remap`.
#[derive(Default)]
pub struct amdgpu_mmio_remap {
    pub reg_offset: u32,
    pub bus_addr: u64,
}

/// `struct amdgpu_gfx_config`, the part the GC info table fills.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_gfx_config {
    pub max_shader_engines: u32,
    pub max_cu_per_sh: u32,
    pub max_sh_per_se: u32,
    pub max_backends_per_se: u32,
    pub max_texture_channel_caches: u32,
    pub max_gprs: u32,
    pub max_gs_threads: u32,
    pub gs_vgt_table_depth: u32,
    pub gs_prim_buffer_depth: u32,
    pub double_offchip_lds_buf: u32,
    pub num_sc_per_sh: u32,
    pub num_packer_per_sc: u32,
    pub gc_num_tcp_per_sa: u32,
    pub gc_num_sdp_interface: u32,
    pub gc_num_tcps: u32,
    pub gc_num_tcp_per_wpg: u32,
    pub gc_tcp_l1_size: u32,
    pub gc_num_sqc_per_wgp: u32,
    pub gc_l1_instruction_cache_size_per_sqc: u32,
    pub gc_l1_data_cache_size_per_sqc: u32,
    pub gc_gl1c_per_sa: u32,
    pub gc_gl1c_size_per_instance: u32,
    pub gc_gl2c_per_gpu: u32,
    pub gc_tcp_size_per_cu: u32,
    pub gc_tcp_cache_line_size: u32,
    pub gc_instruction_cache_size_per_sqc: u32,
    pub gc_instruction_cache_line_size: u32,
    pub gc_scalar_data_cache_size_per_sqc: u32,
    pub gc_scalar_data_cache_line_size: u32,
    pub gc_tcc_size: u32,
    pub gc_tcc_cache_line_size: u32,
    pub gc_num_cu_per_sqc: u32,
}

/// `struct amdgpu_cu_info`, the part the GC info table fills.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_cu_info {
    pub wave_front_size: u32,
    pub max_waves_per_simd: u32,
    pub max_scratch_slots_per_cu: u32,
    pub lds_size: u32,
}

/// `struct amdgpu_gfx`, the part discovery and early_init fill.
#[derive(Default)]
pub struct amdgpu_gfx {
    pub xcc_mask: u32,
    pub config: amdgpu_gfx_config,
    pub cu_info: amdgpu_cu_info,
    pub funcs: Option<&'static str>,
    pub mcbp: bool,
    pub num_gfx_rings: u32,
    pub num_compute_rings: u32,
    pub pfp_fw: Option<firmware>,
    pub me_fw: Option<firmware>,
    pub ce_fw: Option<firmware>,
    pub rlc_fw: Option<firmware>,
    pub mec_fw: Option<firmware>,
    pub mec2_fw: Option<firmware>,
    pub pfp_fw_version: u32,
    pub pfp_feature_version: u32,
    pub me_fw_version: u32,
    pub me_feature_version: u32,
    pub ce_fw_version: u32,
    pub ce_feature_version: u32,
    pub rlc_fw_version: u32,
    pub rlc_feature_version: u32,
    pub rlc_srlc_fw_version: u32,
    pub rlc_srlc_feature_version: u32,
    pub rlc_srlg_fw_version: u32,
    pub rlc_srlg_feature_version: u32,
    pub rlc_srls_fw_version: u32,
    pub rlc_srls_feature_version: u32,
    pub rlcp_ucode_version: u32,
    pub rlcp_ucode_feature_version: u32,
    pub rlcv_ucode_version: u32,
    pub rlcv_ucode_feature_version: u32,
    pub mec_fw_version: u32,
    pub mec_feature_version: u32,
    pub mec2_fw_version: u32,
    pub mec2_feature_version: u32,
    pub me_fw_write_wait: bool,
    pub mec_fw_write_wait: bool,
    pub rlc: amdgpu_rlc,
    pub kiq0_pmf: Option<&'static str>,
    pub kiq0_ring_funcs: Option<&'static str>,
    pub gfx_ring_funcs: [Option<&'static str>; AMDGPU_MAX_GFX_RINGS as usize],
    pub sw_gfx_ring_funcs: [Option<&'static str>; GFX9_NUM_SW_GFX_RINGS as usize],
    pub compute_ring_funcs: [Option<&'static str>; AMDGPU_MAX_COMPUTE_RINGS as usize],
    pub eop_irq: amdgpu_irq_src,
    pub priv_reg_irq: amdgpu_irq_src,
    pub bad_op_irq: amdgpu_irq_src,
    pub priv_inst_irq: amdgpu_irq_src,
    pub cp_ecc_error_irq: amdgpu_irq_src,
}

/// `struct amdgpu_xgmi`, the part early_init reads and sets.
#[derive(Default)]
pub struct amdgpu_xgmi {
    pub supported: bool,
    pub connected_to_cpu: bool,
    pub ras: Option<&'static str>,
}

/// `struct amdgpu_gmc`.
#[derive(Default)]
pub struct amdgpu_gmc {
    pub num_umc: u32,
    pub mall_size: u64,
    pub m_half_use: u32,
    pub xgmi: amdgpu_xgmi,
    pub is_app_apu: bool,
    pub gmc_funcs: Option<&'static str>,
    pub vm_fault: amdgpu_irq_src,
    pub ecc_irq: amdgpu_irq_src,
    pub shared_aperture_start: u64,
    pub shared_aperture_end: u64,
    pub private_aperture_start: u64,
    pub private_aperture_end: u64,
    pub noretry_flags: u64,
}

/// `struct amdgpu_sdma`.
#[derive(Default)]
pub struct amdgpu_sdma {
    pub num_instances: u32,
    pub sdma_mask: u32,
    pub has_page_queue: bool,
    pub instance: [amdgpu_sdma_instance; AMDGPU_MAX_SDMA_INSTANCES as usize],
    pub trap_irq: amdgpu_irq_src,
    pub ecc_irq: amdgpu_irq_src,
    pub illegal_inst_irq: amdgpu_irq_src,
    pub vm_hole_irq: amdgpu_irq_src,
    pub doorbell_invalid_irq: amdgpu_irq_src,
    pub pool_timeout_irq: amdgpu_irq_src,
    pub srbm_write_irq: amdgpu_irq_src,
}

/// `struct amdgpu_umc`.
#[derive(Default)]
pub struct amdgpu_umc {
    pub node_inst_num: u32,
    pub active_mask: u32,
}

/// `struct amdgpu_vcn_inst`, the part discovery and early_init fill.
#[derive(Default)]
pub struct amdgpu_vcn_inst {
    pub vcn_config: u32,
    pub vcn_codec_disable_mask: u32,
    pub inst: u32,
    pub num_enc_rings: u32,
    pub fw: Option<crate::linux::firmware>,
    pub set_pg_state: Option<&'static str>,
    pub ring_dec_funcs: Option<&'static str>,
    pub ring_enc_funcs: [Option<&'static str>; AMDGPU_MAX_VCN_ENC_RINGS as usize],
    pub irq: amdgpu_irq_src,
}

/// `struct amdgpu_vcn`.
#[derive(Default)]
pub struct amdgpu_vcn {
    pub num_vcn_inst: u32,
    pub inst_mask: u32,
    pub harvest_config: u32,
    pub inst: [amdgpu_vcn_inst; AMDGPU_MAX_VCN_INSTANCES as usize],
}

/// `struct amdgpu_jpeg`.
#[derive(Default)]
pub struct amdgpu_jpeg {
    pub inst_mask: u32,
    pub harvest_config: u32,
    pub num_jpeg_inst: u32,
    pub num_jpeg_rings: u32,
    /// `inst[0].ring_dec[0].funcs`, `inst[0].irq`.
    pub inst0_ring_dec_funcs: Option<&'static str>,
    pub inst0_irq: amdgpu_irq_src,
}

/// `struct amdgpu_irq`, the part early_init sets.
#[derive(Default)]
pub struct amdgpu_irq {
    pub ih_funcs: Option<&'static str>,
    pub self_irq: amdgpu_irq_src,
}

/// `struct amdgpu_vm_manager`, the part early_init sets.
#[derive(Default)]
pub struct amdgpu_vm_manager {
    pub vm_pte_funcs: Option<&'static str>,
    pub vm_pte_num_scheds: u32,
}

/// `struct amdgpu_vpe`.
#[derive(Default)]
pub struct amdgpu_vpe {
    pub num_instances: u32,
}

/// `struct amdgpu_device`, the part ported so far.
pub struct amdgpu_device {
    /// `pdev->device`, `pdev->revision`, `pdev->subsystem_*`.
    pub device: u16,
    pub revision: u8,
    pub subsystem_vendor: u16,
    pub subsystem_device: u16,
    pub apu_flags: u32,
    /// `adev->rmmio`, BAR5, and its length.
    pub rmmio: Mmio,
    pub rmmio_size: u32,
    pub mman: amdgpu_mman,
    /// `adev->ip_versions`, `IP_VERSION_FULL` encoded.
    pub ip_versions: [[u32; HWIP_MAX_INSTANCE]; MAX_HWIP],
    /// `adev->reg_offset[hw_ip][inst]`: the IP's base addresses, in dwords.
    pub reg_offset: [[Vec<u32>; HWIP_MAX_INSTANCE]; MAX_HWIP],
    pub harvest_ip_mask: u32,
    pub gfx: amdgpu_gfx,
    pub gmc: amdgpu_gmc,
    pub sdma: amdgpu_sdma,
    pub umc: amdgpu_umc,
    pub vcn: amdgpu_vcn,
    pub jpeg: amdgpu_jpeg,
    pub vpe: amdgpu_vpe,
    /// `adev->flags` (AMD_IS_APU …), `adev->family`.
    pub flags: u32,
    pub family: u32,
    pub rev_id: u32,
    pub external_rev_id: u32,
    pub cg_flags: u64,
    pub pg_flags: u32,
    /// `adev->rmmio_base`: the bus address of BAR5.
    pub rmmio_base: u64,
    pub rmmio_remap: amdgpu_mmio_remap,
    pub nbio_funcs: Option<amdgpu_nbio_funcs>,
    pub hdp_funcs: Option<amdgpu_hdp_funcs>,
    pub df_funcs: Option<amdgpu_df_funcs>,
    pub smuio_funcs: Option<amdgpu_smuio_funcs>,
    pub asic_funcs: Option<amdgpu_asic_funcs>,
    pub enable_virtual_display: bool,
    pub firmware: amdgpu_firmware,
    pub aid_mask: u32,
    pub gds: amdgpu_gds,
    pub irq: amdgpu_irq,
    pub vm_manager: amdgpu_vm_manager,
    pub mmhub_funcs: Option<&'static str>,
    pub gfxhub_funcs: Option<&'static str>,
    pub hdp_ras: Option<&'static str>,
    pub psp: psp_context,
    pub psp_timeout: u32,
    /// `adev->powerplay.pp_handle` and `pp_funcs`.
    pub smu: smu_context,
    pub powerplay_pp_funcs: Option<&'static str>,
    pub dc_enabled: bool,
    /// `adev->dm.dmub_fw`.
    pub dm_dmub_fw: Option<crate::linux::firmware>,
    pub ip_blocks: Vec<amdgpu_ip_block>,
    /// `adev->bios`, from `amdgpu_get_bios`.
    pub bios: Vec<u8>,
    pub is_atom_fw: bool,
    pub mode_info: amdgpu_mode_info,
    pub bios_scratch_reg_offset: u32,
    pub clock: amdgpu_clock,
    pub pm: amdgpu_pm,
}

impl amdgpu_device {
    pub fn new(device: u16, revision: u8, rmmio: Mmio, rmmio_size: u32) -> Self {
        amdgpu_device {
            device,
            revision,
            subsystem_vendor: 0,
            subsystem_device: 0,
            apu_flags: 0,
            rmmio,
            rmmio_size,
            mman: amdgpu_mman {
                discovery_tmr_size: 0,
                discovery_bin: Vec::new(),
                fw_vram_usage_start_offset: 0,
                fw_vram_usage_size: 0,
                drv_vram_usage_start_offset: 0,
                drv_vram_usage_size: 0,
                buffer_funcs: None,
                buffer_funcs_ring: None,
            },
            ip_versions: [[0; HWIP_MAX_INSTANCE]; MAX_HWIP],
            reg_offset: core::array::from_fn(|_| core::array::from_fn(|_| Vec::new())),
            harvest_ip_mask: 0,
            gfx: amdgpu_gfx::default(),
            gmc: amdgpu_gmc::default(),
            sdma: amdgpu_sdma::default(),
            umc: amdgpu_umc::default(),
            vcn: amdgpu_vcn::default(),
            jpeg: amdgpu_jpeg::default(),
            vpe: amdgpu_vpe::default(),
            flags: 0,
            family: 0,
            rev_id: 0,
            external_rev_id: 0,
            cg_flags: 0,
            pg_flags: 0,
            rmmio_base: 0,
            rmmio_remap: amdgpu_mmio_remap::default(),
            nbio_funcs: None,
            hdp_funcs: None,
            df_funcs: None,
            smuio_funcs: None,
            asic_funcs: None,
            enable_virtual_display: false,
            firmware: amdgpu_firmware::default(),
            aid_mask: 0,
            gds: amdgpu_gds::default(),
            irq: amdgpu_irq::default(),
            vm_manager: amdgpu_vm_manager::default(),
            mmhub_funcs: None,
            gfxhub_funcs: None,
            hdp_ras: None,
            psp: psp_context::default(),
            psp_timeout: 0,
            smu: smu_context::default(),
            powerplay_pp_funcs: None,
            dc_enabled: false,
            dm_dmub_fw: None,
            ip_blocks: Vec::new(),
            bios: Vec::new(),
            is_atom_fw: false,
            mode_info: amdgpu_mode_info::default(),
            bios_scratch_reg_offset: 0,
            clock: amdgpu_clock::default(),
            pm: amdgpu_pm::default(),
        }
    }

    /// `amdgpu_ip_version`: major, minor and revision, without variant
    /// and subrevision.
    pub fn ip_version(&self, ip: u32, inst: usize) -> u32 {
        self.ip_versions[ip as usize][inst] & !0xFF
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

/// `amdgpu_device_init_apu_flags`, the CHIP_RENOIR case.
pub fn amdgpu_device_init_apu_flags(adev: &mut amdgpu_device) {
    if adev.device == 0x1636 || adev.device == 0x164c {
        adev.apu_flags |= AMD_APU_IS_RENOIR;
    } else {
        adev.apu_flags |= AMD_APU_IS_GREEN_SARDINE;
    }
}

/// `amdgpu_device_ip_block_add`.
pub fn amdgpu_device_ip_block_add(adev: &mut amdgpu_device, v: &'static amdgpu_ip_block_version) -> i32 {
    if v.type_ == AMD_IP_BLOCK_TYPE_VCN && adev.harvest_ip_mask & AMD_HARVEST_IP_VCN_MASK != 0 {
        return 0;
    }
    if v.type_ == AMD_IP_BLOCK_TYPE_JPEG && adev.harvest_ip_mask & AMD_HARVEST_IP_JPEG_MASK != 0 {
        return 0;
    }
    Line::new().s("detected ip block number ").dec(adev.ip_blocks.len() as u64).s(" <")
        .bytes(ip_block_name(v.type_)).s("_v").dec(v.major as u64).s("_").dec(v.minor as u64).s("_")
        .dec(v.rev as u64).s("> (").bytes(v.funcs_name).s(")").say();
    adev.ip_blocks.push(amdgpu_ip_block { status: amdgpu_ip_block_status::default(), version: v });
    0
}

/// `amdgpu_ucode_get_load_type` (amdgpu_ucode.c), for CHIP_RENOIR: the
/// default branch.
pub fn amdgpu_ucode_get_load_type(load_type: i32) -> u32 {
    if load_type == 0 {
        AMDGPU_FW_LOAD_DIRECT
    } else if load_type == 3 {
        AMDGPU_FW_LOAD_RLC_BACKDOOR_AUTO
    } else {
        AMDGPU_FW_LOAD_PSP
    }
}

/// `amdgpu_device_enable_virtual_display`: off, `amdgpu_virtual_display`
/// is not set.
pub fn amdgpu_device_enable_virtual_display(adev: &mut amdgpu_device) {
    adev.enable_virtual_display = false;
}

/// `amdgpu_device_pcie_dynamic_switching_supported`. `cpu_data(0)` on an
/// APU is the Zen core on the same die: AMD, not Intel and not Pinnacle
/// Ridge (family 0x17 model 0x08), and an APU is not removable.
pub fn amdgpu_device_pcie_dynamic_switching_supported(_adev: &amdgpu_device) -> bool {
    true
}

/// `amdgpu_device_asic_has_dc_support`, CHIP_RENOIR: the default branch.
pub fn amdgpu_device_asic_has_dc_support() -> bool {
    amdgpu_dc[0] != 0
}

/// `amdgpu_device_has_dc_support`.
pub fn amdgpu_device_has_dc_support(adev: &amdgpu_device) -> bool {
    if adev.enable_virtual_display || adev.harvest_ip_mask & AMD_HARVEST_IP_DMU_MASK != 0 {
        return false;
    }
    amdgpu_device_asic_has_dc_support()
}

/// `amdgpu_device_set_mcbp`: bare metal, `amdgpu_mcbp` -1 leaves it as it is.
pub fn amdgpu_device_set_mcbp(adev: &mut amdgpu_device) {
    match crate::tables::amdgpu_mcbp[0] {
        1 => adev.gfx.mcbp = true,
        0 => adev.gfx.mcbp = false,
        _ => {}
    }
    if adev.gfx.mcbp {
        Line::new().s("MCBP is enabled").say();
    }
}

/// The block's `funcs->early_init`, by its type: the ten blocks
/// `amdgpu_discovery_set_ip_blocks` adds on Renoir.
fn early_init(adev: &mut amdgpu_device, type_: u32) -> i32 {
    match type_ {
        AMD_IP_BLOCK_TYPE_COMMON => crate::soc15::soc15_common_early_init(adev),
        AMD_IP_BLOCK_TYPE_GMC => crate::gmc_v9_0::gmc_v9_0_early_init(adev),
        AMD_IP_BLOCK_TYPE_IH => crate::vega10_ih::vega10_ih_early_init(adev),
        AMD_IP_BLOCK_TYPE_PSP => crate::amdgpu_psp::psp_early_init(adev),
        AMD_IP_BLOCK_TYPE_SMC => crate::amdgpu_smu::smu_early_init(adev),
        AMD_IP_BLOCK_TYPE_DCE => crate::amdgpu_dm::dm_early_init(adev),
        AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_early_init(adev),
        AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_early_init(adev),
        AMD_IP_BLOCK_TYPE_VCN => crate::vcn_v2_0::vcn_v2_0_early_init(adev),
        AMD_IP_BLOCK_TYPE_JPEG => crate::jpeg_v2_0::jpeg_v2_0_early_init(adev),
        _ => -22,
    }
}

/// `amdgpu_device_ip_early_init`: the IP blocks, each block's early_init,
/// and the VBIOS after COMMON's. Returns the number of blocks.
pub fn amdgpu_device_ip_early_init(adev: &mut amdgpu_device, bdf: (u8, u8, u8)) -> Result<usize, i32> {
    amdgpu_device_enable_virtual_display(adev);

    // CHIP_RENOIR: the default branch.
    let r = crate::amdgpu_discovery::amdgpu_discovery_set_ip_blocks(adev);
    if r != 0 {
        adev.ip_blocks.clear();
        return Err(r);
    }

    // GC 9.4.3 A0, PX (dGPU only) and PR3 (dGPU only) do not apply.

    adev.pm.pp_feature = crate::tables::amdgpu_pp_feature_mask[0];
    if crate::tables::sched_policy[0] == KFD_SCHED_POLICY_NO_HWS as i32 {
        adev.pm.pp_feature &= !PP_GFXOFF_MASK;
    }
    if !amdgpu_device_pcie_dynamic_switching_supported(adev) {
        adev.pm.pp_feature &= !PP_PCIE_DPM_MASK;
    }

    let mut total = true;
    for i in 0..adev.ip_blocks.len() {
        let t = adev.ip_blocks[i].version.type_;
        if crate::tables::amdgpu_ip_block_mask[0] & (1 << i) == 0 {
            Line::new().s("disabled ip block: ").dec(i as u64).s(" <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s(">").say();
            adev.ip_blocks[i].status.valid = false;
        } else {
            let r = early_init(adev, t);
            if r == -2 {
                adev.ip_blocks[i].status.valid = false;
            } else if r != 0 {
                Line::new().s("early_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                    .s("> failed ").dec((-r) as u64).say();
                total = false;
            } else {
                adev.ip_blocks[i].status.valid = true;
            }
        }

        // get the vbios after the asic_funcs are set up
        if t == AMD_IP_BLOCK_TYPE_COMMON {
            // amdgpu_device_parse_gpu_info_fw: CHIP_RENOIR has no gpu_info
            // firmware. amdgpu_device_get_vbios_flags: no AIDs, so 0.
            if !crate::amdgpu_bios::amdgpu_get_bios(adev, bdf.0, bdf.1, bdf.2) {
                return Err(-22);
            }
            let r = crate::amdgpu_atombios::amdgpu_atombios_init(adev);
            if r != 0 {
                Line::new().s("amdgpu_atombios_init failed").say();
                return Err(r);
            }
        }
    }
    if !total {
        return Err(-19);
    }
    Ok(adev.ip_blocks.len())
}
