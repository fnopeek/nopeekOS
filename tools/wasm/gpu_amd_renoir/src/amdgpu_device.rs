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

/// `struct amdgpu_mman`: what discovery and atomfirmware fill, and the
/// buffer objects with their managers.
#[derive(Default)]
pub struct amdgpu_mman {
    pub discovery_tmr_size: u32,
    pub discovery_bin: Vec<u8>,
    pub fw_vram_usage_start_offset: u64,
    pub fw_vram_usage_size: u64,
    pub drv_vram_usage_start_offset: u64,
    pub drv_vram_usage_size: u64,
    pub stolen_reserved_offset: u64,
    pub stolen_reserved_size: u64,
    pub keep_stolen_vga_memory: bool,
    pub stolen_vga_size: u64,
    pub stolen_extended_size: u64,
    pub buffer_funcs: Option<&'static str>,
    pub buffer_funcs_ring: Option<&'static str>,
    pub initialized: bool,
    pub buffer_funcs_enabled: bool,
    /// The buffer functions' scheduler entities, by key.
    pub high_pr: Option<usize>,
    pub low_pr: Option<usize>,
    /// The resource managers by placement (`bdev->man_drv`).
    pub vram_mgr: Option<crate::amdgpu_vram_mgr::amdgpu_vram_mgr>,
    pub gtt_mgr: Option<crate::amdgpu_ttm::amdgpu_gtt_mgr>,
    pub preempt_mgr: Option<crate::ttm::ttm_resource_manager>,
    /// `ttm_range_man_init` managers, by mem_type (doorbell, MMIO remap, GDS, GWS, OA).
    pub range_mgrs: alloc::vec::Vec<(u32, crate::ttm::ttm_resource_manager, crate::drm_mm::drm_mm)>,
    /// The system manager TTM always has.
    pub sys_mgr: crate::ttm::ttm_resource_manager,
    /// Every buffer object, by handle.
    pub bos: alloc::vec::Vec<Option<crate::amdgpu_object::amdgpu_bo>>,
    /// `aper_base_kaddr`: the stolen memory, mapped.
    pub aper_base_kaddr: Option<npk_sys::Mmio>,
    pub fw_vram_usage_reserved_bo: Option<usize>,
    pub drv_vram_usage_reserved_bo: Option<usize>,
    pub fw_reserved_memory: Option<usize>,
    pub stolen_vga_memory: Option<usize>,
    pub stolen_extended_memory: Option<usize>,
    pub stolen_reserved_memory: Option<usize>,
    pub sdma_access_bo: Option<usize>,
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
    pub gpu_vm_support: bool,
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
    /// `power_supply_is_system_supplied() > 0`; `None` while the kernel
    /// offers no power-supply query to a module.
    pub ac_power: Option<bool>,
    pub pcie_gen_mask: u32,
    pub pcie_mlw_mask: u32,
    pub smu_prv_buffer_size: u32,
    pub no_fan: bool,
    pub dpm_enabled: bool,
    pub fw_version: u32,
    /// `pwr_state[]`: POWER_STATE_UNKNOWN (0) until a block is gated.
    pub pwr_state: [u32; AMD_IP_BLOCK_TYPE_NUM as usize],
    /// `dpm.forced_level`, `dpm.thermal_active`.
    pub dpm_forced_level: u32,
    pub dpm_thermal_active: bool,
    /// `stable_pstate_ctx`: the serial of the context that holds the
    /// stable pstate.
    pub stable_pstate_ctx: Option<u64>,
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

/// `struct amdgpu_df_hash_status`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct amdgpu_df_hash_status {
    pub hash_64k: bool,
    pub hash_2m: bool,
    pub hash_1g: bool,
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

/// `struct amdgpu_virt`, the part bare metal sets.
#[derive(Default)]
pub struct amdgpu_virt {
    pub caps: u32,
}

/// `struct amdgpu_doorbell`.
#[derive(Default)]
pub struct amdgpu_doorbell {
    pub base: u64,
    pub size: u64,
    pub num_kernel_doorbells: u32,
    pub kernel_doorbells: Option<usize>,
    pub cpu_addr: Option<crate::amdgpu_object::amdgpu_kptr>,
}

/// `struct amdgpu_doorbell_index`, as `vega10_doorbell_index_init` fills it.
#[derive(Default)]
pub struct amdgpu_doorbell_index {
    pub kiq: u32,
    pub mec_ring0: u32,
    pub mec_ring1: u32,
    pub mec_ring2: u32,
    pub mec_ring3: u32,
    pub mec_ring4: u32,
    pub mec_ring5: u32,
    pub mec_ring6: u32,
    pub mec_ring7: u32,
    pub userqueue_start: u32,
    pub userqueue_end: u32,
    pub gfx_ring0: u32,
    pub sdma_engine: [u32; 8],
    pub ih: u32,
    pub uvd_ring0_1: u32,
    pub uvd_ring2_3: u32,
    pub uvd_ring4_5: u32,
    pub uvd_ring6_7: u32,
    pub vce_ring0_1: u32,
    pub vce_ring2_3: u32,
    pub vce_ring4_5: u32,
    pub vce_ring6_7: u32,
    pub vcn_ring0_1: u32,
    pub vcn_ring2_3: u32,
    pub vcn_ring4_5: u32,
    pub vcn_ring6_7: u32,
    pub first_non_cp: u32,
    pub last_non_cp: u32,
    pub max_assignment: u32,
    pub sdma_doorbell_range: u32,
}

/// `adev->rmmio_remap`.
#[derive(Default)]
pub struct amdgpu_mmio_remap {
    pub reg_offset: u32,
    pub bus_addr: u64,
    pub bo: Option<usize>,
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
    pub max_hw_contexts: u32,
    pub sc_prim_fifo_size_frontend: u32,
    pub sc_prim_fifo_size_backend: u32,
    pub sc_hiz_tile_fifo_size: u32,
    pub sc_earlyz_tile_fifo_size: u32,
    pub gb_addr_config: u32,
    pub gb_addr_config_fields: amdgpu_gb_addr_config,
    pub max_tile_pipes: u32,
    pub backend_enable_mask: u32,
    pub num_rbs: u32,
    pub db_debug2: u32,
}

/// `struct gb_addr_config`.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_gb_addr_config {
    pub pipe_interleave_size: u32,
    pub num_pipes: u32,
    pub max_compress_frags: u32,
    pub num_banks: u32,
    pub num_se: u32,
    pub num_rb_per_se: u32,
}

/// `struct amdgpu_cu_info`, the part the GC info table fills.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_cu_info {
    pub wave_front_size: u32,
    pub max_waves_per_simd: u32,
    pub max_scratch_slots_per_cu: u32,
    pub lds_size: u32,
    pub number: u32,
    pub ao_cu_mask: u32,
    pub simd_per_cu: u32,
    pub bitmap: [[[u32; 4]; 4]; 1],
    pub ao_cu_bitmap: [[u32; 4]; 4],
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
    pub gfx_off_req_count: u32,
    pub gfx_off_state: bool,
    pub gfx_off_residency: u32,
    pub gfx_off_entrycount: u64,
    pub mec: crate::gfx_v9_0::amdgpu_mec,
    /// `mec_bitmap[0].queue_bitmap`.
    pub mec_queue_bitmap: [u64; (AMDGPU_MAX_COMPUTE_QUEUES as usize).div_ceil(64)],
    pub kiq: crate::gfx_v9_0::amdgpu_kiq,
    /// Indices into `adev.rings`.
    pub gfx_ring: Vec<usize>,
    pub compute_ring: Vec<usize>,
    pub gfx_current_status: u32,
    pub enable_cleaner_shader: bool,
    pub cleaner_shader_size: u32,
    pub cleaner_shader_obj: Option<usize>,
    pub cleaner_shader_gpu_addr: u64,
    pub cleaner_shader_cpu_ptr: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub gfx_supported_reset: u32,
    pub compute_supported_reset: u32,
    pub ce_ram_size: u32,
    pub ip_dump_core: Vec<u32>,
    pub ip_dump_compute_queues: Vec<u32>,
}

/// `struct amdgpu_xgmi`, the part early_init reads and sets.
#[derive(Default)]
pub struct amdgpu_xgmi {
    pub supported: bool,
    pub connected_to_cpu: bool,
    pub ras: Option<&'static str>,
    pub num_physical_nodes: u32,
    pub physical_node_id: u32,
    pub node_segment_size: u64,
}

/// `adev->mm_stats`; its spinlock guards nothing on one thread.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_mm_stats {
    pub last_update_us: i64,
    /// accumulated microseconds
    pub accum_us: i64,
    /// for visible VRAM
    pub accum_us_vis: i64,
    pub log2_max_MBps: u32,
}

/// `struct amdgpu_gmc`.
#[derive(Default)]
pub struct amdgpu_gmc {
    /// `fault_ring`, `fault_hash[].idx` and `last_fault` of
    /// `amdgpu_gmc_filter_faults`; empty until the first retry fault.
    pub fault_ring: Vec<crate::gmc_v9_0::amdgpu_gmc_fault>,
    pub fault_hash: Vec<u64>,
    pub last_fault: u64,
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
    pub gart_size: u64,
    pub tmz_enabled: bool,
    pub noretry: u32,
    pub flush_pasid_uses_kiq: bool,
    pub flush_tlb_needs_extra_type_0: bool,
    pub flush_tlb_needs_extra_type_2: bool,
    pub vram_width: u32,
    pub vram_type: u32,
    pub vram_vendor: u32,
    pub translate_further: bool,
    /// The GPU's internal address space and where VRAM, GART and AGP sit in it.
    pub mc_mask: u64,
    pub mc_vram_size: u64,
    pub real_vram_size: u64,
    pub visible_vram_size: u64,
    pub aper_base: u64,
    pub aper_size: u64,
    pub fb_start: u64,
    pub fb_end: u64,
    pub vram_start: u64,
    pub vram_end: u64,
    pub gart_start: u64,
    pub gart_end: u64,
    pub agp_start: u64,
    pub agp_end: u64,
    pub agp_size: u64,
    pub vmid0_page_table_depth: u32,
    pub vmid0_page_table_block_size: u32,
}

/// `struct amdgpu_vmhub`, the registers `*_init` records.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_vmhub {
    pub ctx0_ptb_addr_lo32: u32,
    pub ctx0_ptb_addr_hi32: u32,
    pub vm_inv_eng0_sem: u32,
    pub vm_inv_eng0_req: u32,
    pub vm_inv_eng0_ack: u32,
    pub vm_context0_cntl: u32,
    pub vm_l2_pro_fault_status: u32,
    pub vm_l2_pro_fault_cntl: u32,
    pub ctx_distance: u32,
    pub ctx_addr_distance: u32,
    pub eng_distance: u32,
    pub eng_addr_distance: u32,
}

/// `struct amdgpu_vm_manager`, the part early_init and GMC set.
#[derive(Default)]
pub struct amdgpu_vm_manager {
    pub vm_pte_funcs: Option<&'static str>,
    pub vm_pte_num_scheds: u32,
    pub max_pfn: u64,
    pub num_level: u32,
    pub root_level: u32,
    pub block_size: u32,
    pub fragment_size: u32,
    pub vram_base_offset: u64,
    pub first_kfd_vmid: u32,
    pub concurrent_flush: bool,
    pub vm_update_mode: u32,
    pub id_mgr: [crate::amdgpu_ids::amdgpu_vmid_mgr; AMDGPU_MAX_VMHUBS as usize],
    /// The contexts of the VMID-wait fences, one per ring.
    pub fence_context: u64,
    pub seqno: Vec<u32>,
    /// `pasids`: the VM of each PASID.
    pub pasids: alloc::collections::BTreeMap<u32, usize>,
    /// The `amdgpu_pasid_xa` allocator: PASIDs in use and the next to try.
    pub pasids_used: Vec<u32>,
    pub pasid_xa_next: u32,
    /// `fault_info`: the last fault of any VM.
    pub fault_info: crate::amdgpu_vm::amdgpu_vm_fault_info,
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
    pub ip_dump: Vec<u32>,
    /// sdma_v4_0 leaves it 0.
    pub supported_reset: u32,
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
    pub indirect_sram: bool,
    pub cur_state: u32,
    pub using_unified_queue: bool,
    pub fw_version: u32,
    pub vcpu_bo: Option<usize>,
    pub gpu_addr: u64,
    pub cpu_addr: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub fw_shared: amdgpu_vcn_fw_shared,
    pub dpg_sram_bo: Option<usize>,
    pub dpg_sram_gpu_addr: u64,
    pub dpg_sram_cpu_addr: Option<crate::amdgpu_object::amdgpu_kptr>,
    /// Indices into `adev.rings`.
    pub ring_dec: Option<usize>,
    pub ring_enc: [Option<usize>; AMDGPU_MAX_VCN_ENC_RINGS as usize],
    pub internal: amdgpu_vcn_reg,
    pub external: amdgpu_vcn_reg,
    /// `pause_state.fw_based`.
    pub pause_state: u32,
    pub total_submission_cnt: u32,
    pub dpg_enc_submission_cnt: u32,
    /// `dpg_sram_curr_addr`: a byte offset into the DPG SRAM's mapping.
    pub dpg_sram_curr: u64,
}

/// `struct amdgpu_vcn_fw_shared`: `cpu_addr` is a byte offset into the
/// VCPU buffer's mapping.
#[derive(Default)]
pub struct amdgpu_vcn_fw_shared {
    pub cpu_off: u64,
    pub gpu_addr: u64,
    pub mem_size: u32,
    pub log_offset: u32,
}

/// `struct amdgpu_vcn_reg`.
#[derive(Default)]
pub struct amdgpu_vcn_reg {
    pub data0: u32,
    pub data1: u32,
    pub cmd: u32,
    pub nop: u32,
    pub context_id: u32,
    pub ib_vmid: u32,
    pub ib_bar_low: u32,
    pub ib_bar_high: u32,
    pub ib_size: u32,
    pub gp_scratch8: u32,
    pub scratch9: u32,
}

/// `struct amdgpu_vcn`.
#[derive(Default)]
pub struct amdgpu_vcn {
    pub num_vcn_inst: u32,
    pub inst_mask: u32,
    pub harvest_config: u32,
    pub inst: [amdgpu_vcn_inst; AMDGPU_MAX_VCN_INSTANCES as usize],
    pub fw_version: u32,
    pub supported_reset: u32,
    pub ip_dump: Vec<u32>,
    pub reg_count: u32,
    pub workload_profile_active: bool,
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
    pub indirect_sram: bool,
    /// `inst[0].ring_dec[0]`, an index into `adev.rings`.
    pub inst0_ring_dec: Option<usize>,
    /// `internal.jpeg_pitch[0]`, `inst[0].external.jpeg_pitch[0]`.
    pub internal_jpeg_pitch0: u32,
    pub cur_state: u32,
    pub inst0_external_jpeg_pitch0: u32,
    pub ip_dump: Vec<u32>,
    pub reg_count: u32,
    pub supported_reset: u32,
    pub total_submission_cnt: u32,
}

/// `struct amdgpu_irq`, the part early_init and the sw_inits set.
#[derive(Default)]
pub struct amdgpu_irq {
    pub ih_funcs: Option<&'static str>,
    pub self_irq: amdgpu_irq_src,
    /// `client[client_id]`, filled by `amdgpu_irq_add_id`.
    pub client: Vec<crate::amdgpu_irq::amdgpu_irq_client>,
    pub ih: crate::vega10_ih::amdgpu_ih_ring,
    pub ih_soft: crate::vega10_ih::amdgpu_ih_ring,
    pub msi_enabled: bool,
    pub installed: bool,
    /// `src->enabled_types`, by the source's name.
    pub enabled_types: alloc::collections::BTreeMap<&'static str, Vec<u32>>,
    /// IVs dispatched, for the log.
    pub dispatched: u32,
    /// The interrupt vector the kernel gave.
    pub irq: u32,
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
    pub pdev: crate::linux::pci_dev,
    pub shutdown: bool,
    pub asic_type: u32,
    pub usec_timeout: u32,
    pub accel_working: bool,
    pub num_rings: u32,
    /// `adev->gfx_timeout` and its siblings, in milliseconds.
    pub gfx_timeout: u64,
    pub compute_timeout: u64,
    pub sdma_timeout: u64,
    pub video_timeout: u64,
    /// `adev->init_lvl`, by the name of the level.
    pub init_lvl: &'static str,
    /// `adev->pcie_rreg`/`pcie_wreg` are `amdgpu_device_indirect_*`.
    pub pcie_reg_indirect: bool,
    pub ras_hw_enabled: u32,
    pub vmhub: [amdgpu_vmhub; AMDGPU_MAX_VMHUBS as usize],
    pub vmhubs_mask: u64,
    pub need_swiotlb: bool,
    pub gart: crate::amdgpu_gart::amdgpu_gart,
    pub dummy_page_addr: u64,
    /// BAR2 mapped, for the doorbell buffer's kernel mapping.
    pub doorbell_mmio: Option<npk_sys::Mmio>,
    pub apu_prefer_gtt: bool,
    pub scpm_enabled: bool,
    pub in_s0ix: bool,
    /// `reset_magic`: the first bytes of the GART table after late_init.
    pub reset_magic: [u8; AMDGPU_RESET_MAGIC_NUM as usize],
    pub in_suspend: bool,
    pub scpm_status: u32,
    pub wb: amdgpu_wb,
    pub seq64: amdgpu_seq64,
    /// `adev->mem_scratch`: the buffer, its GPU address and mapping.
    pub mem_scratch_robj: Option<usize>,
    pub mem_scratch_gpu_addr: u64,
    pub mem_scratch_ptr: Option<crate::amdgpu_object::amdgpu_kptr>,
    /// `adev->ras_list`: the RAS blocks registered, by name.
    pub ras_list: Vec<&'static str>,
    /// `adev->mes.db_start_dw_offset`.
    pub mes_db_start_dw_offset: u32,
    /// `pci_resource_start/len(pdev, 0)`: the VRAM BAR.
    pub bar0: (u64, u64),
    pub ras_enabled: u32,
    /// `adev->df.hash_status`.
    pub df_hash_status: amdgpu_df_hash_status,
    pub enforce_isolation: [u32; MAX_XCP as usize],
    pub virt: amdgpu_virt,
    pub have_atomics_support: bool,
    pub has_hw_reset: bool,
    pub doorbell: amdgpu_doorbell,
    pub doorbell_index: amdgpu_doorbell_index,
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
    pub dm: crate::amdgpu_dm::amdgpu_display_manager,
    pub ib_pool_ready: bool,
    pub ib_pools: [amdgpu_sa_manager; AMDGPU_IB_POOL_MAX as usize],
    /// The `drm_suballoc_manager` each IB pool embeds.
    pub ib_sa: [crate::amdgpu_ib::drm_suballoc_manager; AMDGPU_IB_POOL_MAX as usize],
    pub enable_mes_kiq: bool,
    pub debug_disable_gpu_ring_reset: bool,
    pub debug_disable_soft_recovery: bool,
    /// `adev->rings`, by `ring->idx`.
    pub rings: Vec<crate::amdgpu_ring::amdgpu_ring>,
    /// `gpu_sched[hw_ip][hw_prio]`: the rings each scheduler list holds.
    pub gpu_sched: [[Vec<usize>; AMDGPU_RING_PRIO_MAX as usize]; AMDGPU_HW_IP_NUM as usize],
    pub ip_blocks: Vec<amdgpu_ip_block>,
    /// `adev->bios`, from `amdgpu_get_bios`.
    pub bios: Vec<u8>,
    pub is_atom_fw: bool,
    pub mode_info: amdgpu_mode_info,
    pub bios_scratch_reg_offset: u32,
    pub clock: amdgpu_clock,
    pub pm: amdgpu_pm,
    /// `adev->fence_context`: the hardware fences' contexts, one per ring.
    pub fence_context: u64,
    pub wq: crate::workqueue::workqueue,
    /// What `amdgpu_device_delayed_init_work_handler`'s IB tests returned,
    /// once they ran.
    pub delayed_init_result: Option<i32>,
    /// The scheduler entities, by key.
    pub sched_entities: Vec<Option<crate::drm_sched::drm_sched_entity>>,
    /// The jobs between allocation and `free_job`, by key.
    pub jobs: alloc::collections::BTreeMap<u64, crate::amdgpu_job::amdgpu_job>,
    pub next_job: u64,
    /// The client VMs, by key.
    pub vms: Vec<Option<crate::amdgpu_vm::amdgpu_vm>>,
    /// The VMs' buffer bases and mappings, by key.
    pub vm_bases: Vec<Option<crate::amdgpu_vm::amdgpu_vm_bo_base>>,
    pub vm_mappings: Vec<Option<crate::amdgpu_vm::amdgpu_bo_va_mapping>>,
    pub gpu_reset_counter: u32,
    /// The reset domain is locked (`amdgpu_in_reset`).
    pub in_gpu_reset: bool,
    pub asic_reset_res: i32,
    pub mp1_state: u32,
    /// The serial the next context gets (a context's identity across
    /// clients, where Linux compares pointers).
    pub next_ctx_serial: u64,
    pub vram_lost_counter: u32,
    /// `mm_stats`: data for buffer migration throttling.
    pub mm_stats: amdgpu_mm_stats,
    /// TTM's move statistics (`amdgpu_bo_move`): only a copying move
    /// counts, and there is none.
    pub num_bytes_moved: u64,
    pub num_evictions: u64,
    pub num_vram_cpu_page_faults: u64,
    /// What `amdgpu_bo_pin` holds in VRAM, CPU-visible VRAM and GART.
    pub vram_pin_size: u64,
    pub visible_pin_size: u64,
    pub gart_pin_size: u64,
    pub gang_submit: crate::dma_fence::Fence,
    pub isolation: crate::amdgpu_job::amdgpu_isolation,
}

impl amdgpu_device {
    pub fn vm(&self, vm: usize) -> &crate::amdgpu_vm::amdgpu_vm {
        self.vms[vm].as_ref().unwrap()
    }

    pub fn vm_mut(&mut self, vm: usize) -> &mut crate::amdgpu_vm::amdgpu_vm {
        self.vms[vm].as_mut().unwrap()
    }

    pub fn new(device: u16, revision: u8, rmmio: Mmio, rmmio_size: u32) -> Self {
        amdgpu_device {
            device,
            revision,
            subsystem_vendor: 0,
            subsystem_device: 0,
            apu_flags: 0,
            rmmio,
            rmmio_size,
            mman: amdgpu_mman::default(),
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
            pdev: crate::linux::pci_dev::new(0, 0, 0),
            shutdown: false,
            asic_type: 0,
            usec_timeout: 0,
            accel_working: false,
            num_rings: 0,
            gfx_timeout: 0,
            compute_timeout: 0,
            sdma_timeout: 0,
            video_timeout: 0,
            init_lvl: "",
            pcie_reg_indirect: false,
            ras_hw_enabled: 0,
            vmhub: [amdgpu_vmhub::default(); AMDGPU_MAX_VMHUBS as usize],
            vmhubs_mask: 0,
            need_swiotlb: false,
            gart: crate::amdgpu_gart::amdgpu_gart::default(),
            dummy_page_addr: 0,
            doorbell_mmio: None,
            apu_prefer_gtt: false,
            scpm_enabled: false,
            in_s0ix: false,
            reset_magic: [0; AMDGPU_RESET_MAGIC_NUM as usize],
            in_suspend: false,
            scpm_status: 0,
            wb: amdgpu_wb::default(),
            seq64: amdgpu_seq64::default(),
            mem_scratch_robj: None,
            mem_scratch_gpu_addr: 0,
            mem_scratch_ptr: None,
            ras_list: Vec::new(),
            mes_db_start_dw_offset: 0,
            bar0: (0, 0),
            ras_enabled: 0,
            df_hash_status: amdgpu_df_hash_status::default(),
            enforce_isolation: [0; MAX_XCP as usize],
            virt: amdgpu_virt::default(),
            have_atomics_support: false,
            has_hw_reset: false,
            doorbell: amdgpu_doorbell::default(),
            doorbell_index: amdgpu_doorbell_index::default(),
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
            dm: Default::default(),
            ib_pool_ready: false,
            ib_pools: Default::default(),
            ib_sa: Default::default(),
            enable_mes_kiq: false,
            debug_disable_gpu_ring_reset: false,
            debug_disable_soft_recovery: false,
            rings: Vec::new(),
            gpu_sched: Default::default(),
            ip_blocks: Vec::new(),
            bios: Vec::new(),
            is_atom_fw: false,
            mode_info: amdgpu_mode_info::default(),
            bios_scratch_reg_offset: 0,
            clock: amdgpu_clock::default(),
            pm: amdgpu_pm::default(),
            fence_context: 0,
            wq: Default::default(),
            delayed_init_result: None,
            sched_entities: Vec::new(),
            jobs: Default::default(),
            next_job: 0,
            vms: Vec::new(),
            vm_bases: Vec::new(),
            vm_mappings: Vec::new(),
            gpu_reset_counter: 0,
            in_gpu_reset: false,
            asic_reset_res: 0,
            mp1_state: 0,
            next_ctx_serial: 0,
            vram_lost_counter: 0,
            mm_stats: amdgpu_mm_stats::default(),
            num_bytes_moved: 0,
            num_evictions: 0,
            num_vram_cpu_page_faults: 0,
            vram_pin_size: 0,
            visible_pin_size: 0,
            gart_pin_size: 0,
            gang_submit: crate::dma_fence::dma_fence_get_stub(),
            isolation: Default::default(),
        }
    }

    /// `amdgpu_ip_version`: major, minor and revision, without variant
    /// and subrevision.
    pub fn ip_version(&self, ip: u32, inst: usize) -> u32 {
        self.ip_versions[ip as usize][inst] & !0xFF
    }

    /// `amdgpu_device_rreg`: a register inside BAR5 is read there; one past
    /// it goes through the PCIe index/data pair (`adev->pcie_rreg`), which
    /// `soc15_common_early_init` sets. None before that: Linux has
    /// `amdgpu_invalid_rreg` there.
    pub fn rreg32(&self, reg: u32) -> Option<u32> {
        if (reg as u64) * 4 < self.rmmio_size as u64 {
            Some(self.rmmio.r32(reg * 4))
        } else if self.pcie_reg_indirect {
            amdgpu_device_indirect_rreg(self, reg.wrapping_mul(4))
        } else {
            None
        }
    }

    /// `amdgpu_device_wreg` (see `rreg32`).
    pub fn wreg32(&self, reg: u32, v: u32) -> bool {
        if (reg as u64) * 4 < self.rmmio_size as u64 {
            self.rmmio.w32(reg * 4, v);
            true
        } else if self.pcie_reg_indirect {
            amdgpu_device_indirect_wreg(self, reg.wrapping_mul(4), v)
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

/// `amdgpu_device_check_arguments`. The module parameters are Linux's
/// defaults (tables.rs); a check that rewrites one only warns here.
fn amdgpu_device_check_arguments(adev: &mut amdgpu_device) -> i32 {
    use crate::tables::*;
    let warn = |s: &str| Line::new().s(s).say();
    let sj = amdgpu_sched_jobs[0];
    if sj < 4 {
        warn("sched jobs must be at least 4");
    } else if !(sj as u32).is_power_of_two() {
        warn("sched jobs must be a power of 2");
    }
    if amdgpu_gart_size[0] != -1 && amdgpu_gart_size[0] < 32 {
        warn("gart size too small");
    }
    if amdgpu_gtt_size[0] != -1 && amdgpu_gtt_size[0] < 32 {
        warn("gtt size too small");
    }
    let vf = amdgpu_vm_fragment_size[0];
    if vf != -1 && !(4..=9).contains(&vf) {
        warn("valid range is between 4 and 9");
    }
    let hw = amdgpu_sched_hw_submission[0];
    if hw < 2 {
        warn("sched hw submission jobs must be at least 2");
    } else if !(hw as u32).is_power_of_two() {
        warn("sched hw submission jobs must be a power of 2");
    }
    if !(-1..=4).contains(&amdgpu_reset_method[0]) {
        warn("invalid option for reset method, reverting to default");
    }

    // amdgpu_device_check_smu_prv_buffer_size: amdgpu_smu_memory_pool_size 0
    if amdgpu_smu_memory_pool_size[0] != 0 {
        warn("smu memory pool size: not ported for a non-zero amdgpu_smu_memory_pool_size");
    }
    // amdgpu_device_check_vm_size, amdgpu_device_check_block_size: -1
    if amdgpu_vm_size[0] != -1 && amdgpu_vm_size[0] < 1 {
        warn("VM size too small, min is 1GB");
    }
    if amdgpu_vm_block_size[0] != -1 && amdgpu_vm_block_size[0] < 9 {
        warn("VM page table size too small");
    }

    adev.firmware.load_type = amdgpu_ucode_get_load_type(amdgpu_fw_load_type[0]);

    for i in 0..MAX_XCP as usize {
        adev.enforce_isolation[i] = match amdgpu_enforce_isolation[0] {
            1 => AMDGPU_ENFORCE_ISOLATION_ENABLE,
            2 => AMDGPU_ENFORCE_ISOLATION_ENABLE_LEGACY,
            3 => AMDGPU_ENFORCE_ISOLATION_NO_CLEANER_SHADER,
            // -1, 0, default: disable
            _ => AMDGPU_ENFORCE_ISOLATION_DISABLE,
        };
    }
    0
}

/// `amdgpu_virt_init`: CHIP_RENOIR has no SR-IOV register
/// (`amdgpu_virt_init_detect_asic` reads none, 0), and nopeekOS runs on
/// the bare machine, so `is_virtual_machine()` is false: no caps.
fn amdgpu_virt_init(adev: &mut amdgpu_device) {
    adev.virt.caps = 0;
}

/// `amdgpu_device_partner_bandwidth`: the first upstream bridge that is
/// not ATI's.
fn amdgpu_device_partner_bandwidth(adev: &amdgpu_device) -> (u32, u32) {
    let mut speed = PCI_SPEED_UNKNOWN;
    let mut width = PCIE_LNK_WIDTH_UNKNOWN;
    if amdgpu_device_pcie_dynamic_switching_supported(adev) {
        let mut parent = adev.pdev;
        while let Some(p) = parent.upstream_bridge() {
            parent = p;
            // skip upstream/downstream switches internal to dGPU
            if p.vendor() as u32 == PCI_VENDOR_ID_ATI {
                continue;
            }
            speed = p.pcie_get_speed_cap();
            width = p.pcie_get_width_cap();
            break;
        }
    }
    (speed, width)
}

/// `amdgpu_device_gpu_bandwidth`.
fn amdgpu_device_gpu_bandwidth(adev: &amdgpu_device) -> (u32, u32) {
    match adev.pdev.upstream_bridge() {
        Some(p) if p.vendor() as u32 == PCI_VENDOR_ID_ATI => {
            // use the upstream/downstream switches internal to dGPU
            let (mut speed, mut width) = (p.pcie_get_speed_cap(), p.pcie_get_width_cap());
            let mut parent = p;
            while let Some(q) = parent.upstream_bridge() {
                parent = q;
                if q.vendor() as u32 == PCI_VENDOR_ID_ATI {
                    speed = q.pcie_get_speed_cap();
                    width = q.pcie_get_width_cap();
                }
            }
            (speed, width)
        }
        // use the device itself
        _ => (adev.pdev.pcie_get_speed_cap(), adev.pdev.pcie_get_width_cap()),
    }
}

/// `amdgpu_device_get_pcie_info`. Bare metal, not passthrough.
fn amdgpu_device_get_pcie_info(adev: &mut amdgpu_device) {
    use crate::tables::{amdgpu_pcie_gen_cap, amdgpu_pcie_lane_cap};
    if amdgpu_pcie_gen_cap[0] != 0 {
        adev.pm.pcie_gen_mask = amdgpu_pcie_gen_cap[0];
    }
    if amdgpu_pcie_lane_cap[0] != 0 {
        adev.pm.pcie_mlw_mask = amdgpu_pcie_lane_cap[0];
    }
    // covers APUs as well
    if adev.pdev.on_root_bus() {
        if adev.pm.pcie_gen_mask == 0 {
            adev.pm.pcie_gen_mask = AMDGPU_DEFAULT_PCIE_GEN_MASK;
        }
        if adev.pm.pcie_mlw_mask == 0 {
            adev.pm.pcie_mlw_mask = AMDGPU_DEFAULT_PCIE_MLW_MASK;
        }
        return;
    }
    if adev.pm.pcie_gen_mask != 0 && adev.pm.pcie_mlw_mask != 0 {
        return;
    }
    let (platform_speed_cap, platform_link_width) = amdgpu_device_partner_bandwidth(adev);
    let (speed_cap, link_width) = amdgpu_device_gpu_bandwidth(adev);

    let asic_gen = [CAIL_ASIC_PCIE_LINK_SPEED_SUPPORT_GEN1, CAIL_ASIC_PCIE_LINK_SPEED_SUPPORT_GEN2,
                    CAIL_ASIC_PCIE_LINK_SPEED_SUPPORT_GEN3, CAIL_ASIC_PCIE_LINK_SPEED_SUPPORT_GEN4,
                    CAIL_ASIC_PCIE_LINK_SPEED_SUPPORT_GEN5];
    let plat_gen = [CAIL_PCIE_LINK_SPEED_SUPPORT_GEN1, CAIL_PCIE_LINK_SPEED_SUPPORT_GEN2,
                    CAIL_PCIE_LINK_SPEED_SUPPORT_GEN3, CAIL_PCIE_LINK_SPEED_SUPPORT_GEN4,
                    CAIL_PCIE_LINK_SPEED_SUPPORT_GEN5];
    // Gen 1 up to the generation of a speed.
    let gens = |speed: u32| -> usize {
        if speed == PCIE_SPEED_32_0GT { 5 }
        else if speed == PCIE_SPEED_16_0GT { 4 }
        else if speed == PCIE_SPEED_8_0GT { 3 }
        else if speed == PCIE_SPEED_5_0GT { 2 }
        else { 1 }
    };
    if adev.pm.pcie_gen_mask == 0 {
        // asic caps
        let n = if speed_cap == PCI_SPEED_UNKNOWN { 3 } else { gens(speed_cap) };
        for g in &asic_gen[..n] {
            adev.pm.pcie_gen_mask |= g;
        }
        // platform caps
        let n = if platform_speed_cap == PCI_SPEED_UNKNOWN { 2 } else { gens(platform_speed_cap) };
        for g in &plat_gen[..n] {
            adev.pm.pcie_gen_mask |= g;
        }
    }
    if adev.pm.pcie_mlw_mask == 0 {
        let asic_w = [CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X1, CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X2,
                      CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X4, CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X8,
                      CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X12, CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X16,
                      CAIL_ASIC_PCIE_LINK_WIDTH_SUPPORT_X32];
        let plat_w = [CAIL_PCIE_LINK_WIDTH_SUPPORT_X1, CAIL_PCIE_LINK_WIDTH_SUPPORT_X2,
                      CAIL_PCIE_LINK_WIDTH_SUPPORT_X4, CAIL_PCIE_LINK_WIDTH_SUPPORT_X8,
                      CAIL_PCIE_LINK_WIDTH_SUPPORT_X12, CAIL_PCIE_LINK_WIDTH_SUPPORT_X16,
                      CAIL_PCIE_LINK_WIDTH_SUPPORT_X32];
        // X1 up to a width; 0 for a width the switch has no case for.
        let widths = |w: u32| -> usize {
            if w == PCIE_LNK_X32 { 7 } else if w == PCIE_LNK_X16 { 6 } else if w == PCIE_LNK_X12 { 5 }
            else if w == PCIE_LNK_X8 { 4 } else if w == PCIE_LNK_X4 { 3 } else if w == PCIE_LNK_X2 { 2 }
            else if w == PCIE_LNK_X1 { 1 } else { 0 }
        };
        // asic caps
        if link_width == PCIE_LNK_WIDTH_UNKNOWN {
            adev.pm.pcie_mlw_mask |= AMDGPU_DEFAULT_ASIC_PCIE_MLW_MASK;
        } else {
            for w in &asic_w[..widths(link_width)] {
                adev.pm.pcie_mlw_mask |= w;
            }
        }
        // platform caps
        if platform_link_width == PCIE_LNK_WIDTH_UNKNOWN {
            adev.pm.pcie_mlw_mask |= AMDGPU_DEFAULT_PCIE_MLW_MASK;
        } else {
            for w in &plat_w[..widths(platform_link_width)] {
                adev.pm.pcie_mlw_mask |= w;
            }
        }
    }
}

/// `amdgpu_device_get_job_timeout_settings`: `amdgpu_lockup_timeout` is
/// empty, so 10 s for every kind of job.
fn amdgpu_device_get_job_timeout_settings(adev: &mut amdgpu_device) -> i32 {
    adev.gfx_timeout = 10000;
    adev.compute_timeout = 10000;
    adev.sdma_timeout = adev.gfx_timeout;
    adev.video_timeout = adev.gfx_timeout;
    if crate::tables::amdgpu_lockup_timeout[0] != 0 {
        Line::new().s("lockup_timeout parameter: not ported for a set value").say();
        return -22;
    }
    0
}

/// `amdgpu_gmc_tmz_set` (amdgpu_gmc.c), GC 9.3.0: "RENOIR looks like RAVEN".
fn amdgpu_gmc_tmz_set(adev: &mut amdgpu_device) {
    if crate::tables::amdgpu_tmz[0] == 0 {
        adev.gmc.tmz_enabled = false;
        Line::new().s("Trusted Memory Zone (TMZ) feature disabled (cmd line)").say();
    } else {
        adev.gmc.tmz_enabled = true;
        Line::new().s("Trusted Memory Zone (TMZ) feature enabled").say();
    }
}

/// `amdgpu_gmc_noretry_set` (amdgpu_gmc.c). Bare metal, so
/// `amdgpu_sriov_xnack_support` is true; GC 9.3.0 is not in the
/// noretry-by-default list.
fn amdgpu_gmc_noretry_set(adev: &mut amdgpu_device) {
    let noretry_default = false;
    let n = crate::tables::amdgpu_noretry[0];
    adev.gmc.noretry = if n == -1 { noretry_default as u32 } else { n as u32 };
}

/// `vega10_doorbell_index_init` (vega10_reg_init.c), soc15's
/// `init_doorbell_index`.
fn vega10_doorbell_index_init(adev: &mut amdgpu_device) {
    let d = &mut adev.doorbell_index;
    d.kiq = AMDGPU_DOORBELL64_KIQ;
    d.mec_ring0 = AMDGPU_DOORBELL64_MEC_RING0;
    d.mec_ring1 = AMDGPU_DOORBELL64_MEC_RING1;
    d.mec_ring2 = AMDGPU_DOORBELL64_MEC_RING2;
    d.mec_ring3 = AMDGPU_DOORBELL64_MEC_RING3;
    d.mec_ring4 = AMDGPU_DOORBELL64_MEC_RING4;
    d.mec_ring5 = AMDGPU_DOORBELL64_MEC_RING5;
    d.mec_ring6 = AMDGPU_DOORBELL64_MEC_RING6;
    d.mec_ring7 = AMDGPU_DOORBELL64_MEC_RING7;
    d.userqueue_start = AMDGPU_DOORBELL64_USERQUEUE_START;
    d.userqueue_end = AMDGPU_DOORBELL64_USERQUEUE_END;
    d.gfx_ring0 = AMDGPU_DOORBELL64_GFX_RING0;
    d.sdma_engine[0] = AMDGPU_DOORBELL64_sDMA_ENGINE0;
    d.sdma_engine[1] = AMDGPU_DOORBELL64_sDMA_ENGINE1;
    d.ih = AMDGPU_DOORBELL64_IH;
    d.uvd_ring0_1 = AMDGPU_DOORBELL64_UVD_RING0_1;
    d.uvd_ring2_3 = AMDGPU_DOORBELL64_UVD_RING2_3;
    d.uvd_ring4_5 = AMDGPU_DOORBELL64_UVD_RING4_5;
    d.uvd_ring6_7 = AMDGPU_DOORBELL64_UVD_RING6_7;
    d.vce_ring0_1 = AMDGPU_DOORBELL64_VCE_RING0_1;
    d.vce_ring2_3 = AMDGPU_DOORBELL64_VCE_RING2_3;
    d.vce_ring4_5 = AMDGPU_DOORBELL64_VCE_RING4_5;
    d.vce_ring6_7 = AMDGPU_DOORBELL64_VCE_RING6_7;
    d.vcn_ring0_1 = AMDGPU_DOORBELL64_VCN0_1;
    d.vcn_ring2_3 = AMDGPU_DOORBELL64_VCN2_3;
    d.vcn_ring4_5 = AMDGPU_DOORBELL64_VCN4_5;
    d.vcn_ring6_7 = AMDGPU_DOORBELL64_VCN6_7;
    d.first_non_cp = AMDGPU_DOORBELL64_FIRST_NON_CP;
    d.last_non_cp = AMDGPU_DOORBELL64_LAST_NON_CP;
    // In unit of dword doorbell
    d.max_assignment = AMDGPU_DOORBELL64_MAX_ASSIGNMENT << 1;
    d.sdma_doorbell_range = 4;
}

/// `amdgpu_doorbell_init` (amdgpu_doorbell_mgr.c): BAR2.
fn amdgpu_doorbell_init(adev: &mut amdgpu_device, bar2_start: u64, bar2_len: u64) -> i32 {
    // pci_resource_flags(pdev, 2) & IORESOURCE_UNSET: an unassigned BAR
    if bar2_start == 0 || bar2_len == 0 {
        return -22;
    }
    vega10_doorbell_index_init(adev);
    adev.doorbell.base = bar2_start;
    adev.doorbell.size = bar2_len;
    adev.doorbell.num_kernel_doorbells =
        ((adev.doorbell.size / 4).min(u32::MAX as u64) as u32).min(adev.doorbell_index.max_assignment + 1);
    if adev.doorbell.num_kernel_doorbells == 0 {
        return -22;
    }
    // For Vega, reserve and map two pages on doorbell BAR since SDMA
    // paging queue doorbell use the second page.
    adev.doorbell.num_kernel_doorbells += 0x400;
    0
}

/// `soc15_asic_reset_method` for MP1 12.0.0 with `amdgpu_reset_method`
/// forced to NONE by the caller: MODE2.
pub fn soc15_asic_reset_method(adev: &amdgpu_device) -> u32 {
    if adev.ip_version(MP1_HWIP, 0) == 12 << 24 {
        AMD_RESET_METHOD_MODE2
    } else {
        AMD_RESET_METHOD_MODE1
    }
}

/// `amdgpu_atombios_scratch_need_asic_init` (amdgpu_atombios.c).
fn amdgpu_atombios_scratch_need_asic_init(adev: &amdgpu_device) -> bool {
    let tmp = adev.rreg32(adev.bios_scratch_reg_offset + 7).unwrap_or(0);
    tmp & ATOM_S7_ASIC_INIT_COMPLETE_MASK == 0
}

/// `amdgpu_device_need_post`. Bare metal, no AIDs (vbios flags 0), not
/// passthrough, default init level, CIK+: the BIOS scratch register says.
pub fn amdgpu_device_need_post(adev: &mut amdgpu_device) -> bool {
    if adev.has_hw_reset {
        adev.has_hw_reset = false;
        return true;
    }
    amdgpu_atombios_scratch_need_asic_init(adev)
}

/// The step `amdgpu_device_init` takes when `amdgpu_asic_need_reset_on_init`
/// is true, decided as Linux decides it, and whether it was taken.
pub struct reset_on_init {
    pub needed: bool,
    pub method: u32,
    pub taken: Option<i32>,
}

/// `soc15_asic_reset`: not Raven, not resuming from S3, so the reset method.
pub fn soc15_asic_reset(adev: &mut amdgpu_device) -> i32 {
    match soc15_asic_reset_method(adev) {
        AMD_RESET_METHOD_MODE2 => {
            Line::new().s("MODE2 reset").say();
            crate::amdgpu_smu::amdgpu_dpm_mode2_reset(adev)
        }
        _ => {
            Line::new().s("MODE1 reset: not ported").say();
            -95
        }
    }
}

/// `amdgpu_device_init` up to `amdgpu_device_ip_init`, Renoir's path.
/// `flags` is the pci id table's `CHIP_RENOIR | AMD_IS_APU`. The reset on
/// init (`soc15_need_reset_on_init` is true for CHIP_RENOIR) is a MODE2
/// reset through the SMU; what it did is returned.
pub fn amdgpu_device_init(adev: &mut amdgpu_device, flags: u32, bdf: (u8, u8, u8), bar2: (u64, u64))
                          -> Result<reset_on_init, i32>
{
    adev.shutdown = false;
    adev.flags = flags;
    adev.asic_type = if crate::tables::amdgpu_force_asic_type[0] >= 0 {
        crate::tables::amdgpu_force_asic_type[0] as u32
    } else {
        flags & AMD_ASIC_MASK
    };
    adev.usec_timeout = AMDGPU_MAX_USEC_TIMEOUT;
    if crate::tables::amdgpu_emu_mode[0] == 1 {
        adev.usec_timeout *= 10;
    }
    adev.gmc.gart_size = 512 * 1024 * 1024;
    adev.accel_working = false;
    adev.num_rings = 0;
    adev.mman.buffer_funcs = None;
    adev.mman.buffer_funcs_ring = None;
    adev.vm_manager.vm_pte_funcs = None;
    adev.vm_manager.vm_pte_num_scheds = 0;
    adev.gmc.gmc_funcs = None;
    adev.harvest_ip_mask = 0;
    adev.fence_context = crate::dma_fence::dma_fence_context_alloc(AMDGPU_MAX_RINGS);
    // The register access hooks start as amdgpu_invalid_rreg/_wreg; the
    // port calls the access functions directly.
    adev.pdev = crate::linux::pci_dev::new(bdf.0, bdf.1, bdf.2);

    Line::new().s("initializing kernel modesetting (")
        .bytes(crate::tables::amdgpu_asic_name.get(adev.asic_type as usize).copied().unwrap_or(b"?"))
        .s(" 0x1002:0x").hexw(adev.device as u32, 4).s(" 0x").hexw(adev.subsystem_vendor as u32, 4).s(":0x")
        .hexw(adev.subsystem_device as u32, 4).s(" 0x").hexw(adev.revision as u32, 2).s(").").say();

    // Mutexes, spinlocks, lists, work items and the throttling ratelimit
    // guard and schedule what one thread does here in turn.
    adev.isolation = crate::amdgpu_job::amdgpu_isolation::default();

    amdgpu_device_init_apu_flags(adev);
    let r = amdgpu_device_check_arguments(adev);
    if r != 0 {
        return Err(r);
    }

    adev.gfx.gfx_off_req_count = 1;
    adev.gfx.gfx_off_residency = 0;
    adev.gfx.gfx_off_entrycount = 0;
    adev.pm.ac_power = None;

    // Registers mapping: BAR5 (CHIP_BONAIRE and later), mapped by the caller.
    Line::new().s("register mmio base: ").hex(adev.rmmio_base as u32).say();
    Line::new().s("register mmio size: ").dec(adev.rmmio_size as u64).say();

    // amdgpu_reset_create_reset_domain: one device, one thread.

    // detect hw virtualization here
    amdgpu_virt_init(adev);
    amdgpu_device_get_pcie_info(adev);
    let r = amdgpu_device_get_job_timeout_settings(adev);
    if r != 0 {
        Line::new().s("invalid lockup_timeout parameter syntax").say();
        return Err(r);
    }
    amdgpu_device_set_mcbp(adev);
    adev.init_lvl = "amdgpu_init_default";

    // early init functions
    amdgpu_device_ip_early_init(adev, bdf)?;

    // aperture_remove_conflicting_pci_devices: the kernel's framebuffer
    // stays until DC takes the display over.

    // Enable TMZ based on IP_VERSION
    amdgpu_gmc_tmz_set(adev);
    amdgpu_gmc_noretry_set(adev);
    // No XGMI: gfxhub get_xgmi_info is not called.

    // APUs w/ gfx9 onwards doesn't reply on PCIe atomics, rather it is a
    // internal path natively support atomics, set have_atomics_support to true.
    if adev.flags & AMD_IS_APU != 0 && adev.ip_version(GC_HWIP, 0) > 9 << 24 {
        adev.have_atomics_support = true;
    }
    if !adev.have_atomics_support {
        Line::new().s("PCIE atomic ops is not supported").say();
    }

    // doorbell bar mapping and doorbell index init
    let r = amdgpu_doorbell_init(adev, bar2.0, bar2.1);
    if r != 0 {
        Line::new().s("amdgpu_doorbell_init failed (").dec((-r) as u64).s(")").say();
    }

    // amdgpu_reset_init: MP1 12.0.0 has no reset handler of its own.
    // amdgpu_device_detect_sriov_bios: bare metal.

    // check if we need to reset the asic: soc15_need_reset_on_init,
    // "CP hangs in IGT reloading test on RN, reset to WA"
    let mut reset = reset_on_init {
        needed: adev.asic_type == CHIP_RENOIR,
        method: soc15_asic_reset_method(adev),
        taken: None,
    };
    if reset.needed {
        // No XGMI hive, MP1 is not 13.0.10: amdgpu_asic_reset with
        // amdgpu_reset_method forced to NONE for the call.
        let r = soc15_asic_reset(adev);
        reset.taken = Some(r);
        if r != 0 {
            Line::new().s("asic reset on init failed").say();
            return Err(r);
        }
    }

    // Post card if necessary
    if amdgpu_device_need_post(adev) {
        Line::new().s("GPU posting now... (amdgpu_device_asic_init: the ATOM interpreter is not ported)").say();
        return Err(-22);
    }

    if adev.mode_info.atom_context.is_some() && adev.is_atom_fw {
        // Initialize clocks
        let r = crate::amdgpu_atomfirmware::amdgpu_atomfirmware_get_clock_info(adev);
        if r != 0 {
            Line::new().s("amdgpu_atomfirmware_get_clock_info failed").say();
            return Err(r);
        }
    }

    // amdgpu_fence_driver_sw_init: 0. drm_mode_config_init: DRM's.
    Ok(reset)
}

/// `amdgpu_ip_member_of_hwini`: the default init level has every block.
fn amdgpu_ip_member_of_hwini(adev: &amdgpu_device, _block: u32) -> bool {
    adev.init_lvl == "amdgpu_init_default"
}

/// `amdgpu_device_ip_init` up to the end of the COMMON block: RAS, then the
/// sw_init of COMMON and, because it is COMMON, its hw_init right away
/// ("need to do common hw init early so everything is set up for gmc").
/// The loop stops before the GMC block.
pub fn amdgpu_device_ip_init_common(adev: &mut amdgpu_device) -> i32 {
    let r = crate::amdgpu_ras::amdgpu_ras_init(adev);
    if r != 0 {
        return r;
    }
    for i in 0..adev.ip_blocks.len() {
        if !adev.ip_blocks[i].status.valid {
            continue;
        }
        let t = adev.ip_blocks[i].version.type_;
        if t != AMD_IP_BLOCK_TYPE_COMMON {
            break;
        }
        let r = crate::soc15::soc15_common_sw_init(adev);
        if r != 0 {
            Line::new().s("sw_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s("> failed ").dec((-r) as u64).say();
            return r;
        }
        adev.ip_blocks[i].status.sw = true;
        if !amdgpu_ip_member_of_hwini(adev, t) {
            continue;
        }
        let r = crate::soc15::soc15_common_hw_init(adev);
        if r != 0 {
            Line::new().s("hw_init ").dec(i as u64).s(" failed ").dec((-r) as u64).say();
            return r;
        }
        adev.ip_blocks[i].status.hw = true;
    }
    0
}

/// The PCIe index and data registers, `nbio.funcs->get_pcie_index_offset`
/// and `get_pcie_data_offset`.
fn pcie_index_data(adev: &amdgpu_device) -> Option<(u32, u32)> {
    match adev.nbio_funcs? {
        amdgpu_nbio_funcs::nbio_v7_0 => Some((
            crate::nbio_v7_0::nbio_v7_0_get_pcie_index_offset(adev)?,
            crate::nbio_v7_0::nbio_v7_0_get_pcie_data_offset(adev)?,
        )),
    }
}

/// `amdgpu_device_indirect_rreg`.
pub fn amdgpu_device_indirect_rreg(adev: &amdgpu_device, reg_addr: u32) -> Option<u32> {
    let (pcie_index, pcie_data) = pcie_index_data(adev)?;
    adev.rmmio.w32(pcie_index * 4, reg_addr);
    adev.rmmio.r32(pcie_index * 4);
    Some(adev.rmmio.r32(pcie_data * 4))
}

/// `amdgpu_device_indirect_wreg`.
pub fn amdgpu_device_indirect_wreg(adev: &amdgpu_device, reg_addr: u32, reg_data: u32) -> bool {
    let Some((pcie_index, pcie_data)) = pcie_index_data(adev) else { return false };
    adev.rmmio.w32(pcie_index * 4, reg_addr);
    adev.rmmio.r32(pcie_index * 4);
    adev.rmmio.w32(pcie_data * 4, reg_data);
    adev.rmmio.r32(pcie_data * 4);
    true
}

/// `amdgpu_doorbell_create_kernel_doorbells` (amdgpu_doorbell_mgr.c).
pub fn amdgpu_doorbell_create_kernel_doorbells(adev: &mut amdgpu_device) -> i32 {
    // SI HW does not have doorbells, skip allocation
    if adev.doorbell.num_kernel_doorbells == 0 {
        return 0;
    }
    // Reserve first num_kernel_doorbells (page-aligned) for kernel ops
    let mut size = crate::linux::align(adev.doorbell.num_kernel_doorbells as u64 * 4, 4096);
    // Allocate an extra page for MES kernel usages (ring test)
    adev.mes_db_start_dw_offset = (size / 4) as u32;
    size += 4096;
    match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, 4096, AMDGPU_GEM_DOMAIN_DOORBELL, true) {
        Ok(h) => {
            adev.doorbell.kernel_doorbells = h;
            adev.doorbell.cpu_addr = h.and_then(|h| crate::amdgpu_object::bo(adev, h).kptr);
        }
        Err(r) => {
            Line::new().s("Failed to allocate kernel doorbells, err=").dec((-r) as u64).say();
            return r;
        }
    }
    adev.doorbell.num_kernel_doorbells = (size / 4) as u32;
    0
}

/// `amdgpu_device_ip_init` from the GMC block on, as far as the port
/// goes: GMC's sw_init.
pub fn amdgpu_device_ip_init_gmc(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        if !adev.ip_blocks[i].status.valid || adev.ip_blocks[i].version.type_ != AMD_IP_BLOCK_TYPE_GMC {
            continue;
        }
        let r = crate::gmc_v9_0::gmc_v9_0_sw_init(adev);
        if r != 0 {
            Line::new().s("sw_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s("> failed ").dec((-r) as u64).say();
            return r;
        }
        adev.ip_blocks[i].status.sw = true;
        // need to do gmc hw init early so we can allocate gpu mem
        let r = amdgpu_device_mem_scratch_init(adev);
        if r != 0 {
            Line::new().s("amdgpu_mem_scratch_init failed ").dec((-r) as u64).say();
            return r;
        }
        let r = crate::gmc_v9_0::gmc_v9_0_hw_init(adev);
        if r != 0 {
            Line::new().s("hw_init ").dec(i as u64).s(" failed ").dec((-r) as u64).say();
            return r;
        }
        let r = amdgpu_device_wb_init(adev);
        if r != 0 {
            Line::new().s("amdgpu_device_wb_init failed ").dec((-r) as u64).say();
            return r;
        }
        adev.ip_blocks[i].status.hw = true;
        // right after GMC hw init, we create CSA: only with MCBP.
        if adev.gfx.mcbp {
            Line::new().s("allocate CSA: MCBP is not Renoir's default, not ported").say();
            return -22;
        }
        let r = amdgpu_seq64_init(adev);
        if r != 0 {
            Line::new().s("allocate seq64 failed ").dec((-r) as u64).say();
            return r;
        }
        return 0;
    }
    -22
}

/// `amdgpu_device_ip_init`, the sw_init loop after the GMC block: IH, PSP,
/// SMU, DM, GFX, SDMA, VCN, JPEG, each by its block type.
pub fn amdgpu_device_ip_init_sw(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        if !adev.ip_blocks[i].status.valid || adev.ip_blocks[i].status.sw {
            continue;
        }
        let r = match adev.ip_blocks[i].version.type_ {
            AMD_IP_BLOCK_TYPE_IH => crate::vega10_ih::vega10_ih_sw_init(adev),
            AMD_IP_BLOCK_TYPE_PSP => crate::amdgpu_psp::psp_sw_init(adev),
            AMD_IP_BLOCK_TYPE_SMC => crate::amdgpu_smu::smu_sw_init(adev),
            AMD_IP_BLOCK_TYPE_DCE => crate::amdgpu_dm::dm_sw_init(adev),
            AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_sw_init(adev),
            AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_sw_init(adev),
            AMD_IP_BLOCK_TYPE_VCN => crate::vcn_v2_0::vcn_v2_0_sw_init(adev),
            AMD_IP_BLOCK_TYPE_JPEG => crate::jpeg_v2_0::jpeg_v2_0_sw_init(adev),
            t => {
                Line::new().s("sw_init of IP block type ").dec(t as u64).s(": not ported").say();
                -22
            }
        };
        if r != 0 {
            Line::new().s("sw_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s("> failed ").dec((-r) as u64).say();
            return r;
        }
        adev.ip_blocks[i].status.sw = true;
    }
    0
}

/// `struct amdgpu_sa_manager` with the `drm_suballoc_manager` it embeds,
/// as `drm_suballoc_manager_init` leaves it: nothing handed out yet.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_sa_manager {
    pub bo: Option<usize>,
    pub gpu_addr: u64,
    pub cpu_ptr: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub size: u32,
    pub align: u32,
}

/// `amdgpu_sa_bo_manager_init`.
fn amdgpu_sa_bo_manager_init(adev: &mut amdgpu_device, size: u32, suballoc_align: u32, domain: u32)
                             -> Result<amdgpu_sa_manager, i32> {
    let h = match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size as u64, AMDGPU_GPU_PAGE_SIZE as u64, domain, true) {
        Ok(Some(h)) => h,
        Ok(None) => return Err(-12),
        Err(r) => {
            Line::new().s("(").dec((-r) as u64).s(") failed to allocate bo for manager").say();
            return Err(r);
        }
    };
    let m = amdgpu_sa_manager {
        bo: Some(h),
        gpu_addr: crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h),
        cpu_ptr: crate::amdgpu_object::bo(adev, h).kptr,
        size,
        align: suballoc_align,
    };
    if let Some(k) = m.cpu_ptr {
        crate::amdgpu_gart::kptr_fill32(adev, k, 0, size as u64, 0);
    }
    Ok(m)
}

/// `amdgpu_ib_pool_init`.
pub fn amdgpu_ib_pool_init(adev: &mut amdgpu_device) -> i32 {
    if adev.ib_pool_ready {
        return 0;
    }
    for i in 0..AMDGPU_IB_POOL_MAX as usize {
        match amdgpu_sa_bo_manager_init(adev, AMDGPU_IB_POOL_SIZE, 256, AMDGPU_GEM_DOMAIN_GTT) {
            Ok(m) => {
                // drm_suballoc_manager_init
                adev.ib_sa[i] = crate::amdgpu_ib::drm_suballoc_manager::new(m.size, m.align);
                adev.ib_pools[i] = m;
            }
            Err(r) => {
                for j in (0..i).rev() {
                    // amdgpu_sa_bo_manager_fini
                    let mut bo = adev.ib_pools[j].bo.take();
                    crate::amdgpu_object::amdgpu_bo_free_kernel(adev, &mut bo);
                    adev.ib_pools[j] = amdgpu_sa_manager::default();
                }
                return r;
            }
        }
    }
    adev.ib_pool_ready = true;
    0
}

/// `amdgpu_ib_pool_init` with the message `amdgpu_device_ip_init` gives.
pub fn amdgpu_device_ib_pool_init_logged(adev: &mut amdgpu_device) -> i32 {
    let r = amdgpu_ib_pool_init(adev);
    if r != 0 {
        Line::new().s("IB initialization failed (").dec((-r) as u64).s(").").say();
    }
    r
}

/// `amdgpu_device_ip_hw_init_phase1`: bare metal, COMMON (done already)
/// and IH.
pub fn amdgpu_device_ip_hw_init_phase1(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        let st = adev.ip_blocks[i].status;
        if !st.sw || st.hw {
            continue;
        }
        let t = adev.ip_blocks[i].version.type_;
        if !amdgpu_ip_member_of_hwini(adev, t) {
            continue;
        }
        if t == AMD_IP_BLOCK_TYPE_COMMON || t == AMD_IP_BLOCK_TYPE_IH {
            let r = if t == AMD_IP_BLOCK_TYPE_IH {
                crate::vega10_ih::vega10_ih_hw_init(adev)
            } else {
                crate::soc15::soc15_common_hw_init(adev)
            };
            if r != 0 {
                Line::new().s("hw_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                    .s("> failed ").dec((-r) as u64).say();
                return r;
            }
            adev.ip_blocks[i].status.hw = true;
        }
    }
    0
}

/// `amdgpu_device_fw_loading`: the PSP block's hw_init.
/// `amdgpu_pm_load_smu_firmware` does nothing for an APU on swSMU.
pub fn amdgpu_device_fw_loading(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        if adev.ip_blocks[i].version.type_ != AMD_IP_BLOCK_TYPE_PSP {
            continue;
        }
        if !amdgpu_ip_member_of_hwini(adev, AMD_IP_BLOCK_TYPE_PSP) {
            break;
        }
        if !adev.ip_blocks[i].status.sw {
            continue;
        }
        // no need to do the fw loading again if already done
        if adev.ip_blocks[i].status.hw {
            break;
        }
        let r = crate::amdgpu_psp::psp_hw_init(adev);
        if r != 0 {
            Line::new().s("hw_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s("> failed ").dec((-r) as u64).say();
            return r;
        }
        adev.ip_blocks[i].status.hw = true;
        break;
    }
    0
}

/// `amdgpu_device_ip_hw_init_phase2`: the remaining blocks in their order.
/// SMU, GFX, SDMA, VCN and JPEG are ported; DM (stage 8) is not yet and
/// stays without hw_init, which the log says.
pub fn amdgpu_device_ip_hw_init_phase2(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        let st = adev.ip_blocks[i].status;
        if !st.sw || st.hw {
            continue;
        }
        let t = adev.ip_blocks[i].version.type_;
        if !amdgpu_ip_member_of_hwini(adev, t) {
            continue;
        }
        let r = match t {
            AMD_IP_BLOCK_TYPE_SMC => crate::amdgpu_smu::smu_hw_init(adev),
            AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_hw_init(adev),
            AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_hw_init(adev),
            AMD_IP_BLOCK_TYPE_VCN => crate::vcn_v2_0::vcn_v2_0_hw_init(adev),
            AMD_IP_BLOCK_TYPE_JPEG => crate::jpeg_v2_0::jpeg_v2_0_hw_init(adev),
            _ => {
                Line::new().s("hw_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                    .s(">: not ported yet, skipped").say();
                continue;
            }
        };
        if r != 0 {
            Line::new().s("hw_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s("> failed ").dec((-r) as u64).say();
            return r;
        }
        adev.ip_blocks[i].status.hw = true;
    }
    0
}

/// `amdgpu_device_wait_on_rreg` (`SOC15_WAIT_ON_RREG`): 0 once `reg & mask`
/// reads `expected`, -ETIMEDOUT after `usec_timeout` reads without change.
pub fn amdgpu_device_wait_on_rreg(adev: &amdgpu_device, inst: u32, reg: u32, name: &str, expected: u32, mask: u32)
                                  -> i32 {
    let mut ret = 0;
    let mut old = 0;
    let mut tmp = adev.rreg32(reg).unwrap_or(0);
    let mut loop_ = adev.usec_timeout;
    while tmp & mask != expected {
        if old != tmp {
            loop_ = adev.usec_timeout;
            old = tmp;
        } else {
            crate::linux::udelay(1);
        }
        tmp = adev.rreg32(reg).unwrap_or(0);
        loop_ = loop_.wrapping_sub(1);
        if loop_ == 0 {
            Line::new().s("Register(").dec(inst as u64).s(") [").s(name).s("] failed to reach value 0x")
                .hexw(expected, 8).s(" != 0x").hexw(tmp & mask, 8).say();
            ret = -110;
            break;
        }
    }
    ret
}

/// `amdgpu_device_mem_scratch_init`.
pub fn amdgpu_device_mem_scratch_init(adev: &mut amdgpu_device) -> i32 {
    match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, AMDGPU_GPU_PAGE_SIZE as u64, 4096,
        AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT, true) {
        Ok(Some(h)) => {
            adev.mem_scratch_robj = Some(h);
            adev.mem_scratch_gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
            adev.mem_scratch_ptr = crate::amdgpu_object::bo(adev, h).kptr;
            0
        }
        Ok(None) => -12,
        Err(r) => r,
    }
}

/// `amdgpu_device_wb_init`: AMDGPU_MAX_WB 256-bit slots in GTT.
fn amdgpu_device_wb_init(adev: &mut amdgpu_device) -> i32 {
    if adev.wb.wb_obj.is_none() {
        let size = AMDGPU_MAX_WB as u64 * 4 * 8;
        match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, 4096, AMDGPU_GEM_DOMAIN_GTT, true) {
            Ok(Some(h)) => {
                adev.wb.wb_obj = Some(h);
                adev.wb.gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
                adev.wb.wb = crate::amdgpu_object::bo(adev, h).kptr;
            }
            Ok(None) => return -12,
            Err(r) => {
                Line::new().s("(").dec((-r) as u64).s(") create WB bo failed").say();
                return r;
            }
        }
        adev.wb.num_wb = AMDGPU_MAX_WB;
        adev.wb.used = [0; (AMDGPU_MAX_WB as usize).div_ceil(64)];
        // clear wb memory
        crate::amdgpu_gart::kptr_fill32(adev, adev.wb.wb.unwrap(), 0, size, 0);
    }
    0
}

/// `amdgpu_seq64_init` (amdgpu_seq64.c).
fn amdgpu_seq64_init(adev: &mut amdgpu_device) -> i32 {
    if adev.seq64.sbo.is_some() {
        return 0;
    }
    let size = AMDGPU_VA_RESERVED_SEQ64_SIZE as u64;
    match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, 4096, AMDGPU_GEM_DOMAIN_GTT, true) {
        Ok(Some(h)) => {
            adev.seq64.sbo = Some(h);
            adev.seq64.gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
            adev.seq64.cpu_base_addr = crate::amdgpu_object::bo(adev, h).kptr;
        }
        Ok(None) => return -12,
        Err(r) => {
            Line::new().s("(").dec((-r) as u64).s(") create seq64 failed").say();
            return r;
        }
    }
    crate::amdgpu_gart::kptr_fill32(adev, adev.seq64.cpu_base_addr.unwrap(), 0, size, 0);
    adev.seq64.num_sem = AMDGPU_MAX_SEQ64_SLOTS;
    adev.seq64.used = alloc::vec![0; (AMDGPU_MAX_SEQ64_SLOTS as usize).div_ceil(64)];
    0
}

/// `struct amdgpu_wb`.
#[derive(Default)]
pub struct amdgpu_wb {
    pub wb_obj: Option<usize>,
    pub wb: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub gpu_addr: u64,
    pub num_wb: u32,
    pub used: [u64; (AMDGPU_MAX_WB as usize).div_ceil(64)],
}

/// `struct amdgpu_seq64`.
#[derive(Default)]
pub struct amdgpu_seq64 {
    pub sbo: Option<usize>,
    pub gpu_addr: u64,
    pub cpu_base_addr: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub num_sem: u32,
    /// The slot bitmap.
    pub used: Vec<u64>,
}

/// `set_clockgating_state` of a block; `None` where its funcs have none.
fn set_clockgating_state(adev: &mut amdgpu_device, type_: u32, gate: bool) -> Option<i32> {
    Some(match type_ {
        AMD_IP_BLOCK_TYPE_COMMON => crate::soc15::soc15_common_set_clockgating_state(adev, gate),
        AMD_IP_BLOCK_TYPE_GMC => crate::gmc_v9_0::gmc_v9_0_set_clockgating_state(adev, gate),
        AMD_IP_BLOCK_TYPE_IH => crate::vega10_ih::vega10_ih_set_clockgating_state(adev, gate),
        // psp_set_clockgating_state and smu_set_clockgating_state return 0.
        AMD_IP_BLOCK_TYPE_PSP | AMD_IP_BLOCK_TYPE_SMC => 0,
        AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_set_clockgating_state(adev, gate),
        AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_set_clockgating_state(adev, gate),
        _ => return None,
    })
}

/// `set_powergating_state` of a block; `None` where its funcs have none.
fn set_powergating_state(adev: &mut amdgpu_device, type_: u32, gate: bool) -> Option<i32> {
    Some(match type_ {
        // soc15_common, gmc_v9_0, vega10_ih, psp and smu return 0.
        AMD_IP_BLOCK_TYPE_COMMON | AMD_IP_BLOCK_TYPE_GMC | AMD_IP_BLOCK_TYPE_IH | AMD_IP_BLOCK_TYPE_PSP
        | AMD_IP_BLOCK_TYPE_SMC => 0,
        AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_set_powergating_state(adev, gate),
        AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_set_powergating_state(adev, gate),
        AMD_IP_BLOCK_TYPE_VCN => {
            crate::vcn_v2_0::vcn_set_powergating_state(adev, if gate { AMD_PG_STATE_GATE } else { AMD_PG_STATE_UNGATE })
        }
        AMD_IP_BLOCK_TYPE_JPEG => {
            crate::jpeg_v2_0::jpeg_v2_0_set_powergating_state(adev, if gate { AMD_PG_STATE_GATE } else { AMD_PG_STATE_UNGATE })
        }
        _ => return None,
    })
}

/// `amdgpu_device_ip_set_clockgating_state`.
pub fn amdgpu_device_ip_set_clockgating_state(adev: &mut amdgpu_device, block_type: u32, gate: bool) -> i32 {
    let mut r = 0;
    for i in 0..adev.ip_blocks.len() {
        if !adev.ip_blocks[i].status.valid || adev.ip_blocks[i].version.type_ != block_type {
            continue;
        }
        let Some(v) = set_clockgating_state(adev, block_type, gate) else { continue };
        r = v;
        if r != 0 {
            Line::new().s("set_clockgating_state of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s("> failed ").dec((-r) as u64).say();
        }
    }
    r
}

/// `amdgpu_device_ip_set_powergating_state`.
pub fn amdgpu_device_ip_set_powergating_state(adev: &mut amdgpu_device, block_type: u32, gate: bool) -> i32 {
    let mut r = 0;
    for i in 0..adev.ip_blocks.len() {
        if !adev.ip_blocks[i].status.valid || adev.ip_blocks[i].version.type_ != block_type {
            continue;
        }
        let Some(v) = set_powergating_state(adev, block_type, gate) else { continue };
        r = v;
        if r != 0 {
            Line::new().s("set_powergating_state of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s("> failed ").dec((-r) as u64).say();
        }
    }
    r
}

/// The block types the CG and PG walks leave alone.
fn cg_skipped(type_: u32) -> bool {
    matches!(type_, AMD_IP_BLOCK_TYPE_UVD | AMD_IP_BLOCK_TYPE_VCE | AMD_IP_BLOCK_TYPE_VCN | AMD_IP_BLOCK_TYPE_JPEG)
}

/// `amdgpu_device_set_cg_state`.
pub fn amdgpu_device_set_cg_state(adev: &mut amdgpu_device, gate: bool) -> i32 {
    if crate::tables::amdgpu_emu_mode[0] == 1 {
        return 0;
    }
    let n = adev.ip_blocks.len();
    for j in 0..n {
        let i = if gate { j } else { n - j - 1 };
        if !adev.ip_blocks[i].status.late_initialized {
            continue;
        }
        let t = adev.ip_blocks[i].version.type_;
        // skip CG for GFX, SDMA on S0ix
        if adev.in_s0ix && (t == AMD_IP_BLOCK_TYPE_GFX || t == AMD_IP_BLOCK_TYPE_SDMA) {
            continue;
        }
        // skip CG for VCE/UVD, it's handled specially
        if cg_skipped(t) {
            continue;
        }
        if let Some(r) = set_clockgating_state(adev, t, gate) {
            if r != 0 {
                Line::new().s("set_clockgating_state(gate) of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                    .s("> failed ").dec((-r) as u64).say();
                return r;
            }
        }
    }
    0
}

/// `amdgpu_device_set_pg_state`.
pub fn amdgpu_device_set_pg_state(adev: &mut amdgpu_device, gate: bool) -> i32 {
    if crate::tables::amdgpu_emu_mode[0] == 1 {
        return 0;
    }
    let n = adev.ip_blocks.len();
    for j in 0..n {
        let i = if gate { j } else { n - j - 1 };
        if !adev.ip_blocks[i].status.late_initialized {
            continue;
        }
        let t = adev.ip_blocks[i].version.type_;
        // skip PG for GFX, SDMA on S0ix
        if adev.in_s0ix && (t == AMD_IP_BLOCK_TYPE_GFX || t == AMD_IP_BLOCK_TYPE_SDMA) {
            continue;
        }
        // skip CG for VCE/UVD, it's handled specially
        if cg_skipped(t) {
            continue;
        }
        if let Some(r) = set_powergating_state(adev, t, gate) {
            if r != 0 {
                Line::new().s("set_powergating_state(gate) of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                    .s("> failed ").dec((-r) as u64).say();
                return r;
            }
        }
    }
    0
}

/// `amdgpu_device_ip_late_init`: no RAS blocks, not in recovery, an APU
/// on its own (no fan boost, no XGMI hive).
pub fn amdgpu_device_ip_late_init(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        if !adev.ip_blocks[i].status.hw {
            continue;
        }
        let r = match adev.ip_blocks[i].version.type_ {
            // soc15_common_late_init: enable_doorbell_selfring_aperture is
            // empty on NBIO 7.0. vega10_ih, psp: no late_init.
            AMD_IP_BLOCK_TYPE_GMC => crate::gmc_v9_0::gmc_v9_0_late_init(adev),
            AMD_IP_BLOCK_TYPE_SMC => crate::amdgpu_smu::smu_late_init(adev),
            AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_late_init(adev),
            AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_late_init(adev),
            _ => 0,
        };
        if r != 0 {
            Line::new().s("late_init of IP block <").bytes(adev.ip_blocks[i].version.funcs_name).s("> failed ")
                .dec((-r) as u64).say();
            return r;
        }
        adev.ip_blocks[i].status.late_initialized = true;
    }
    // amdgpu_ras_late_init: no RAS context, no ACA, empty RAS list.
    amdgpu_device_set_cg_state(adev, true);
    amdgpu_device_set_pg_state(adev, true);
    amdgpu_device_fill_reset_magic(adev);
    0
}

/// `amdgpu_device_fill_reset_magic`: the first bytes of the GART table,
/// which a reset that keeps VRAM leaves alone.
fn amdgpu_device_fill_reset_magic(adev: &mut amdgpu_device) {
    if let Some(k) = adev.gart.ptr {
        let mut magic = [0u8; AMDGPU_RESET_MAGIC_NUM as usize];
        crate::amdgpu_gart::kptr_read(adev, k, 0, &mut magic);
        adev.reset_magic = magic;
    }
}

/// `amdgpu_device_check_vram_lost`: MODE2 keeps VRAM unless the magic says
/// otherwise.
fn amdgpu_device_check_vram_lost(adev: &mut amdgpu_device) -> bool {
    if let Some(k) = adev.gart.ptr {
        let mut now = [0u8; AMDGPU_RESET_MAGIC_NUM as usize];
        crate::amdgpu_gart::kptr_read(adev, k, 0, &mut now);
        if now != adev.reset_magic {
            return true;
        }
    }
    if !amdgpu_in_reset(adev) {
        return false;
    }
    // For all ASICs with baco/mode1 reset, the VRAM is always assumed to
    // be lost.
    soc15_asic_reset_method(adev) != AMD_RESET_METHOD_MODE2
}

/// `amdgpu_in_reset`.
pub fn amdgpu_in_reset(adev: &amdgpu_device) -> bool {
    adev.in_gpu_reset
}

/// A block's `suspend`; `None` for the blocks without hw_init here.
fn ip_suspend(adev: &mut amdgpu_device, type_: u32) -> Option<i32> {
    Some(match type_ {
        AMD_IP_BLOCK_TYPE_COMMON => crate::soc15::soc15_common_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_GMC => crate::gmc_v9_0::gmc_v9_0_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_IH => crate::vega10_ih::vega10_ih_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_PSP => crate::amdgpu_psp::psp_suspend(adev),
        AMD_IP_BLOCK_TYPE_SMC => crate::amdgpu_smu::smu_suspend(adev),
        AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_hw_fini(adev),
        // not in S0ix
        AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_VCN => crate::vcn_v2_0::vcn_v2_0_suspend(adev),
        AMD_IP_BLOCK_TYPE_JPEG => crate::jpeg_v2_0::jpeg_v2_0_suspend(adev),
        _ => return None,
    })
}

/// A block's `resume`; `None` for the blocks without hw_init here.
fn ip_resume(adev: &mut amdgpu_device, type_: u32) -> Option<i32> {
    Some(match type_ {
        // soc15_need_reset_on_resume: not resuming from S3
        AMD_IP_BLOCK_TYPE_COMMON => crate::soc15::soc15_common_hw_init(adev),
        AMD_IP_BLOCK_TYPE_GMC => {
            // gmc_v9_0_resume: no NPS mode switch
            let r = crate::gmc_v9_0::gmc_v9_0_hw_init(adev);
            if r == 0 {
                crate::amdgpu_ids::amdgpu_vmid_reset_all(adev);
            }
            r
        }
        AMD_IP_BLOCK_TYPE_IH => crate::vega10_ih::vega10_ih_hw_init(adev),
        AMD_IP_BLOCK_TYPE_PSP => crate::amdgpu_psp::psp_resume(adev),
        AMD_IP_BLOCK_TYPE_SMC => crate::amdgpu_smu::smu_resume(adev),
        AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_hw_init(adev),
        AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_hw_init(adev),
        AMD_IP_BLOCK_TYPE_VCN => crate::vcn_v2_0::vcn_v2_0_resume(adev),
        AMD_IP_BLOCK_TYPE_JPEG => crate::jpeg_v2_0::jpeg_v2_0_resume(adev),
        _ => return None,
    })
}

/// `amdgpu_ip_block_suspend`.
fn amdgpu_ip_block_suspend(adev: &mut amdgpu_device, i: usize) -> i32 {
    let t = adev.ip_blocks[i].version.type_;
    match ip_suspend(adev, t) {
        Some(r) if r != 0 => {
            Line::new().s("suspend of IP block <").bytes(adev.ip_blocks[i].version.funcs_name).s("> failed ")
                .dec((-r) as u64).say();
            return r;
        }
        Some(_) => {}
        None => {
            Line::new().s("suspend of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s(">: never brought up here, skipped").dbg();
        }
    }
    adev.ip_blocks[i].status.hw = false;
    0
}

/// `amdgpu_ip_block_resume`. A block the module never brought up stays
/// down, as at hw_init.
fn amdgpu_ip_block_resume(adev: &mut amdgpu_device, i: usize) -> i32 {
    let t = adev.ip_blocks[i].version.type_;
    match ip_resume(adev, t) {
        Some(r) if r != 0 => {
            Line::new().s("resume of IP block <").bytes(adev.ip_blocks[i].version.funcs_name).s("> failed ")
                .dec((-r) as u64).say();
            return r;
        }
        Some(_) => adev.ip_blocks[i].status.hw = true,
        None => {}
    }
    0
}

/// `amdgpu_device_ip_suspend_phase1`: the displays. The module brings up
/// none, and renoir_ppt has no `set_df_cstate`.
fn amdgpu_device_ip_suspend_phase1(adev: &mut amdgpu_device) -> i32 {
    amdgpu_device_set_pg_state(adev, false);
    amdgpu_device_set_cg_state(adev, false);
    for i in (0..adev.ip_blocks.len()).rev() {
        if !adev.ip_blocks[i].status.valid || adev.ip_blocks[i].version.type_ != AMD_IP_BLOCK_TYPE_DCE {
            continue;
        }
        let r = amdgpu_ip_block_suspend(adev, i);
        if r != 0 {
            return r;
        }
    }
    0
}

/// `amdgpu_device_ip_suspend_phase2`: not S0ix, no RAS interrupt, no IMU.
/// renoir_ppt has no `set_mp1_state`. The GMC stays up while the
/// firmware's scanout reads through the MMHUB (as `amdgpu_device_fini_hw`
/// holds it), and its resume runs hw_init over it as the probe did.
fn amdgpu_device_ip_suspend_phase2(adev: &mut amdgpu_device) -> i32 {
    let display_ours = adev.ip_blocks.iter().any(|b| b.version.type_ == AMD_IP_BLOCK_TYPE_DCE && b.status.hw);
    for i in (0..adev.ip_blocks.len()).rev() {
        let t = adev.ip_blocks[i].version.type_;
        if !adev.ip_blocks[i].status.valid || t == AMD_IP_BLOCK_TYPE_DCE {
            continue;
        }
        // skip unnecessary suspend if we do not initialize them yet
        if !amdgpu_ip_member_of_hwini(adev, t) {
            continue;
        }
        if t == AMD_IP_BLOCK_TYPE_GMC && !display_ours {
            Line::new().s("suspend of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s(">: held back, the display still scans out through the MMHUB").dbg();
            adev.ip_blocks[i].status.hw = false;
            continue;
        }
        amdgpu_ip_block_suspend(adev, i);
        adev.ip_blocks[i].status.hw = false;
    }
    0
}

/// `amdgpu_device_ip_suspend`: bare metal.
fn amdgpu_device_ip_suspend(adev: &mut amdgpu_device) -> i32 {
    crate::amdgpu_ttm::amdgpu_ttm_set_buffer_funcs_status(adev, false);
    let r = amdgpu_device_ip_suspend_phase1(adev);
    if r != 0 {
        return r;
    }
    amdgpu_device_ip_suspend_phase2(adev)
}

/// `amdgpu_device_ip_resume_phase1`: COMMON, GMC, IH.
fn amdgpu_device_ip_resume_phase1(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        let st = adev.ip_blocks[i].status;
        if !st.valid || st.hw {
            continue;
        }
        let t = adev.ip_blocks[i].version.type_;
        if matches!(t, AMD_IP_BLOCK_TYPE_COMMON | AMD_IP_BLOCK_TYPE_GMC | AMD_IP_BLOCK_TYPE_IH) {
            let r = amdgpu_ip_block_resume(adev, i);
            if r != 0 {
                return r;
            }
        }
    }
    0
}

/// `amdgpu_device_ip_resume_phase2`: all but COMMON, GMC, IH, DCE, PSP.
fn amdgpu_device_ip_resume_phase2(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        let st = adev.ip_blocks[i].status;
        if !st.valid || st.hw {
            continue;
        }
        let t = adev.ip_blocks[i].version.type_;
        if matches!(t, AMD_IP_BLOCK_TYPE_COMMON | AMD_IP_BLOCK_TYPE_GMC | AMD_IP_BLOCK_TYPE_IH
            | AMD_IP_BLOCK_TYPE_DCE | AMD_IP_BLOCK_TYPE_PSP)
        {
            continue;
        }
        let r = amdgpu_ip_block_resume(adev, i);
        if r != 0 {
            return r;
        }
    }
    0
}

/// `amdgpu_device_ip_resume_phase3`: the displays.
fn amdgpu_device_ip_resume_phase3(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        let st = adev.ip_blocks[i].status;
        if !st.valid || st.hw || adev.ip_blocks[i].version.type_ != AMD_IP_BLOCK_TYPE_DCE {
            continue;
        }
        let r = amdgpu_ip_block_resume(adev, i);
        if r != 0 {
            return r;
        }
    }
    0
}

/// `amdgpu_atom_asic_init` (atom.c): the vbios ASIC_Init table with the
/// default engine and memory clocks.
fn amdgpu_atom_asic_init(adev: &mut amdgpu_device) -> i32 {
    let Some(ctx) = adev.mode_info.atom_context.as_ref() else { return -22 };
    let hwi = ctx.cu16(ctx.data_table + ATOM_DATA_FWI_PTR) as u32;
    let mut ps = [0u32; 16];
    ps[0] = ctx.cu32(hwi + ATOM_FWI_DEFSCLK_PTR);
    ps[1] = ctx.cu32(hwi + ATOM_FWI_DEFMCLK_PTR);
    if ps[0] == 0 || ps[1] == 0 {
        return 1;
    }
    if ctx.cu16(ctx.cmd_table + 4 + 2 * ATOM_CMD_INIT) == 0 {
        return 1;
    }
    crate::amdgpu_smu::amdgpu_atom_execute_table(adev, ATOM_CMD_INIT, &mut ps, 16)
}

/// `amdgpu_device_asic_init`, GC 9.3.0: `soc15_pre_asic_init`
/// (`gmc_v9_0_restore_registers`, which is for DCN 1.0 only), vbios flags
/// 0, so the ATOM ASIC_Init.
fn amdgpu_device_asic_init(adev: &mut amdgpu_device) -> i32 {
    amdgpu_atom_asic_init(adev)
}

/// `amdgpu_device_fw_loading` in a reset: the PSP block's resume.
fn amdgpu_device_fw_loading_resume(adev: &mut amdgpu_device) -> i32 {
    for i in 0..adev.ip_blocks.len() {
        if adev.ip_blocks[i].version.type_ != AMD_IP_BLOCK_TYPE_PSP {
            continue;
        }
        if !amdgpu_ip_member_of_hwini(adev, AMD_IP_BLOCK_TYPE_PSP) {
            break;
        }
        if !adev.ip_blocks[i].status.sw {
            continue;
        }
        // no need to do the fw loading again if already done
        if adev.ip_blocks[i].status.hw {
            break;
        }
        let r = amdgpu_ip_block_resume(adev, i);
        if r != 0 {
            return r;
        }
        break;
    }
    // amdgpu_pm_load_smu_firmware: nothing for an APU on swSMU.
    0
}

/// `amdgpu_device_pre_asic_reset`: no reset handler for MP1 12.0.0, and
/// `soc15_need_full_reset` is always true, so the blocks are suspended.
fn amdgpu_device_pre_asic_reset(adev: &mut amdgpu_device, job: Option<u64>) -> i32 {
    crate::amdgpu_fence::amdgpu_fence_driver_isr_toggle(adev, true);
    let fence = job.and_then(|j| adev.jobs.get(&j).and_then(|jb| jb.hw_fence.clone()));
    // block all schedulers and reset given job's ring
    for r in 0..adev.rings.len() {
        if !amdgpu_ring_sched_ready(adev, r) {
            continue;
        }
        // Clear job fence from fence drv to avoid force_completion leave
        // NULL and vm flush fence in fence drv
        crate::amdgpu_fence::amdgpu_fence_driver_clear_job_fences(adev, r);
        // after all hw jobs are reset, hw fence is meaningless, so
        // force_completion
        crate::amdgpu_fence::amdgpu_fence_driver_force_completion(adev, r, fence.as_ref());
    }
    crate::amdgpu_fence::amdgpu_fence_driver_isr_toggle(adev, false);
    if let Some(j) = job.filter(|j| adev.jobs.get(j).is_some_and(|jb| jb.vm.is_some())) {
        crate::drm_sched::drm_sched_increase_karma(adev, j);
    }
    // amdgpu_reset_prepare_hwcontext: no reset handler (EOPNOTSUPP).
    // Skipped core dump (AMDGPU_SKIP_COREDUMP).
    amdgpu_device_ip_suspend(adev)
}

/// `amdgpu_device_reinit_after_reset`: a full reset at the recovery init
/// level, which brings up the same blocks as the default one here.
fn amdgpu_device_reinit_after_reset(adev: &mut amdgpu_device) -> i32 {
    let mut r = amdgpu_device_asic_init(adev);
    if r != 0 {
        Line::new().s("asic atom init failed!").say();
    } else {
        Line::new().s("GPU reset succeeded, trying to resume").say();
        match amdgpu_device_reinit_blocks(adev) {
            Ok(v) => r = v,
            // amdgpu_device_fw_loading failed: returned as it is
            Err(e) => return e,
        }
    }
    if r == 0 {
        crate::amdgpu_irq::amdgpu_irq_gpu_reset_resume_helper(adev);
        r = crate::amdgpu_ib::amdgpu_ib_ring_tests(adev);
        if r != 0 {
            Line::new().s("ib ring test failed (").dec((-r) as u64).s(").").say();
            adev.asic_reset_res = -11;
            return -11;
        }
    }
    if r != 0 {
        adev.asic_reset_res = r;
    }
    r
}

/// The resume part of `amdgpu_device_reinit_after_reset`: Err when the
/// firmware loading failed, which returns at once.
fn amdgpu_device_reinit_blocks(adev: &mut amdgpu_device) -> Result<i32, i32> {
    let r = amdgpu_device_ip_resume_phase1(adev);
    if r != 0 {
        return Ok(r);
    }
    let vram_lost = amdgpu_device_check_vram_lost(adev);
    if vram_lost {
        Line::new().s("VRAM is lost due to GPU reset!").say();
        // amdgpu_inc_vram_lost
        adev.vram_lost_counter += 1;
    }
    let r = amdgpu_device_fw_loading_resume(adev);
    if r != 0 {
        return Err(r);
    }
    let r = amdgpu_device_ip_resume_phase2(adev);
    if r != 0 {
        return Ok(r);
    }
    if adev.sdma.instance[0].ring.is_some_and(|s| adev.rings[s].sched.ready) {
        crate::amdgpu_ttm::amdgpu_ttm_set_buffer_funcs_status(adev, true);
    }
    let r = amdgpu_device_ip_resume_phase3(adev);
    if r != 0 {
        return Ok(r);
    }
    if vram_lost {
        amdgpu_device_fill_reset_magic(adev);
    }
    Ok(amdgpu_device_ip_late_init(adev))
}

/// `amdgpu_do_asic_reset`: no reset handler, no XGMI, no RAS interrupt.
fn amdgpu_do_asic_reset(adev: &mut amdgpu_device) -> i32 {
    let r = soc15_asic_reset(adev);
    if r != 0 {
        Line::new().s("ASIC reset failed with error, ").dec((-r) as u64).say();
        return r;
    }
    amdgpu_device_reinit_after_reset(adev)
}

/// `amdgpu_device_asic_reset`: bare metal, again after an EAGAIN.
fn amdgpu_device_asic_reset(adev: &mut amdgpu_device, job: Option<u64>) -> i32 {
    loop {
        let r = amdgpu_device_pre_asic_reset(adev, job);
        if r != 0 {
            Line::new().s("GPU pre asic reset failed with err, ").dec((-r) as u64).say();
            adev.asic_reset_res = r;
        }
        let r = amdgpu_do_asic_reset(adev);
        if r != -11 {
            // amdgpu_device_stop_pending_resets: no reset work queued
            return r;
        }
    }
}

/// `amdgpu_ring_sched_ready`.
fn amdgpu_ring_sched_ready(adev: &amdgpu_device, r: usize) -> bool {
    !adev.rings[r].no_scheduler && adev.rings[r].sched.ready
}

/// `amdgpu_device_set_mp1_state`.
fn amdgpu_device_set_mp1_state(adev: &mut amdgpu_device) {
    adev.mp1_state = match soc15_asic_reset_method(adev) {
        AMD_RESET_METHOD_MODE1 => PP_MP1_STATE_SHUTDOWN,
        AMD_RESET_METHOD_MODE2 => PP_MP1_STATE_RESET,
        _ => PP_MP1_STATE_NONE,
    };
}

/// `amdgpu_device_gpu_recover` for one device on bare metal: no RAS, no
/// XGMI hive, no KFD, no display of its own (the audio function stays as
/// it is: `amdgpu_device_suspend_display_audio` refuses MODE2).
pub fn amdgpu_device_gpu_recover(adev: &mut amdgpu_device, job: Option<u64>) -> i32 {
    Line::new().s("GPU reset begin!. Source:  ").dec(AMDGPU_RESET_SRC_JOB as u64).say();
    // amdgpu_device_health_check
    let status = adev.pdev.read_dword(PCI_COMMAND);
    if status == u32::MAX {
        Line::new().s("device lost from bus!").say();
        Line::new().s("GPU reset end with ret = ").dec(19).say();
        return -19;
    }
    // amdgpu_device_lock_reset_domain
    adev.in_gpu_reset = true;
    // amdgpu_device_halt_activities
    amdgpu_device_set_mp1_state(adev);
    for r in 0..adev.rings.len() {
        if !amdgpu_ring_sched_ready(adev, r) {
            continue;
        }
        crate::drm_sched::drm_sched_stop(adev, r, job);
    }
    adev.gpu_reset_counter += 1;
    // Must check guilty signal here since after this point all old HW
    // fences are force signaled.
    let hw = job.and_then(|j| adev.jobs.get(&j).and_then(|jb| jb.hw_fence.clone()));
    let job_signaled = hw.is_some_and(|f| crate::dma_fence::dma_fence_is_signaled(adev, &f));
    let mut r = 0;
    if job_signaled {
        Line::new().s("Guilty job already signaled, skipping HW reset").say();
    } else {
        r = amdgpu_device_asic_reset(adev, job);
    }
    if r == 0 {
        // amdgpu_device_sched_resume
        for ring in 0..adev.rings.len() {
            if !amdgpu_ring_sched_ready(adev, ring) {
                continue;
            }
            crate::drm_sched::drm_sched_start(adev, ring, 0);
        }
        if adev.asic_reset_res != 0 {
            Line::new().s("GPU reset(").dec(adev.gpu_reset_counter as u64).s(") failed with error ")
                .dec((-adev.asic_reset_res) as u64).say();
            r = adev.asic_reset_res;
            adev.asic_reset_res = 0;
        } else {
            Line::new().s("GPU reset(").dec(adev.gpu_reset_counter as u64).s(") succeeded!").say();
        }
    }
    if r == 0 {
        // amdgpu_device_gpu_resume: amdgpu_device_unset_mp1_state
        adev.mp1_state = PP_MP1_STATE_NONE;
    }
    // amdgpu_device_unlock_reset_domain
    adev.in_gpu_reset = false;
    if r != 0 {
        Line::new().s("GPU reset end with ret = ").dec((-r) as u64).say();
    }
    r
}

/// A block's `hw_fini`; the blocks without hw_init here have none to run.
fn hw_fini(adev: &mut amdgpu_device, type_: u32) -> i32 {
    match type_ {
        AMD_IP_BLOCK_TYPE_COMMON => crate::soc15::soc15_common_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_GMC => crate::gmc_v9_0::gmc_v9_0_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_IH => crate::vega10_ih::vega10_ih_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_PSP => crate::amdgpu_psp::psp_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_SMC => crate::amdgpu_smu::smu_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_GFX => crate::gfx_v9_0::gfx_v9_0_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_SDMA => crate::sdma_v4_0::sdma_v4_0_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_VCN => crate::vcn_v2_0::vcn_v2_0_hw_fini(adev),
        AMD_IP_BLOCK_TYPE_JPEG => crate::jpeg_v2_0::jpeg_v2_0_hw_fini(adev),
        _ => 0,
    }
}

/// `amdgpu_device_init_schedulers`: no UVD or VCE rings, no partitions.
pub fn amdgpu_device_init_schedulers(adev: &mut amdgpu_device) -> i32 {
    for r in 0..adev.rings.len() {
        // No need to setup the GPU scheduler for rings that don't need it
        if adev.rings[r].no_scheduler {
            continue;
        }
        let t = adev.rings[r].funcs.type_;
        let timeout_ms = if t == AMDGPU_RING_TYPE_GFX {
            adev.gfx_timeout
        } else if t == AMDGPU_RING_TYPE_COMPUTE {
            adev.compute_timeout
        } else if t == AMDGPU_RING_TYPE_SDMA {
            adev.sdma_timeout
        } else {
            adev.video_timeout
        };
        let credit_limit = adev.rings[r].num_hw_submission;
        let name = adev.rings[r].name.clone();
        let ret = crate::drm_sched::drm_sched_init(adev, r, credit_limit, timeout_ms as i64 * 1000, name.clone());
        if ret != 0 {
            Line::new().s("Failed to create scheduler on ring ").s(&name).s(".").say();
            return ret;
        }
    }
    0
}

/// `amdgpu_device_fini_hw`, up to `amdgpu_device_ip_fini_early` and
/// `amdgpu_irq_fini_hw`: bare metal, no display, no RAS, no TTM buffer
/// functions enabled, no KFD. The DMA buffers go back with the module.
pub fn amdgpu_device_fini_hw(adev: &mut amdgpu_device) {
    Line::new().s("amdgpu: finishing device.").say();
    if adev.mman.initialized {
        crate::workqueue::drain_ttm(adev);
    }
    adev.shutdown = true;
    amdgpu_device_set_pg_state(adev, false);
    amdgpu_device_set_cg_state(adev, false);
    // disable all interrupts
    crate::amdgpu_irq::amdgpu_irq_disable_all(adev);
    crate::amdgpu_fence::amdgpu_fence_driver_hw_fini(adev);
    crate::amdgpu_ttm::amdgpu_ttm_set_buffer_funcs_status(adev, false);

    // amdgpu_device_ip_fini_early: no block has early_fini;
    // amdgpu_device_smu_fini_early is for GC 9.0.0 and older.
    // DM never came up here, so the scanout the kernel still draws into is
    // the firmware's, and it reads through the MMHUB: gmc_v9_0_hw_fini
    // would switch the hub's TLB and L2 off under it. Linux reaches the GMC
    // only after DM's hw_fini has taken the display down.
    let display_ours = adev.ip_blocks.iter().any(|b| b.version.type_ == AMD_IP_BLOCK_TYPE_DCE && b.status.hw);
    for i in (0..adev.ip_blocks.len()).rev() {
        if !adev.ip_blocks[i].status.hw {
            continue;
        }
        if adev.ip_blocks[i].version.type_ == AMD_IP_BLOCK_TYPE_GMC && !display_ours {
            Line::new().s("hw_fini of IP block <").bytes(adev.ip_blocks[i].version.funcs_name)
                .s(">: held back, the display still scans out through the MMHUB").say();
            continue;
        }
        let r = hw_fini(adev, adev.ip_blocks[i].version.type_);
        if r != 0 {
            Line::new().s("hw_fini of IP block <").bytes(adev.ip_blocks[i].version.funcs_name).s("> failed ")
                .dec((-r) as u64).dbg();
        }
        adev.ip_blocks[i].status.hw = false;
    }
    // amdgpu_irq_fini_hw: free_irq; the rings go with the module.
    adev.irq.installed = false;
}

/// `AMDGPU_RESUME_MS`, in microseconds.
pub const AMDGPU_RESUME_US: u64 = 2_000_000;

/// `amdgpu_device_delayed_init_work_handler`: the IB tests, queued by
/// `amdgpu_device_init` after late init.
pub fn amdgpu_device_delayed_init_work_handler(adev: &mut amdgpu_device) {
    let r = crate::amdgpu_ib::amdgpu_ib_ring_tests(adev);
    if r != 0 {
        Line::new().s("ib ring test failed (").dec((-r) as u64).s(").").say();
    }
    adev.delayed_init_result = Some(r);
}
