//! display/dc/dc.h + dc/inc/core_types.h (Linux 6.18.55): the Display Core
//! spine — dc_context, dc_init_data and `struct dc`. Ported grow-as-reached:
//! fields appear as the construct/create path reaches them, so the struct
//! tracks the port rather than carrying forward-declared blocks that are not
//! built yet (the oracle compares the values set, not struct completeness).
//! `driver_context` (Linux' amdgpu_device pointer) is not stored here — the
//! module threads `&amdgpu_device` to the register helpers, as the rest of it
//! does.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::clk_mgr::dc_clk_mgr_create;
use crate::dc_state::dc_state_create;
use crate::dcn20_fpu::dcn21_update_bw_bounding_box;
use crate::bios_parser2::{bios_parser_get_connector_id, bios_parser_get_connectors_number};
use crate::link_factory::{create_virtual_link, dc_link, get_num_of_internal_disp, link_create, link_ctx, link_init_data};
use crate::regs::{LF_ENUM_ID_COUNT, LF_MAX_LINKS};
use alloc::vec::Vec;
use crate::dc_resource::{dc_create_resource_pool, resource_parse_asic_id, resource_pool, NO_UNDERLAY_PIPE};
use crate::gpio_service::{dal_gpio_service_create, gpio_service};
use crate::bios_parser2::{bios_parser, bp_init_data, dal_bios_parser_create};
use crate::dc_types::{
    dc_caps, dce_environment, dce_version, dc_versions, dp_protocol_version, hw_asic_id, DC_VER,
};
use alloc::boxed::Box;

/// `struct dc_context` (dc_types.h): the per-device DC context. Sub-block
/// handles (dmub_srv, gpio_service, …) join as their stages land. `dc_bios`
/// owns the parser (Linux points at `bios_parser.base`).
#[derive(Default)]
pub struct dc_context {
    pub dce_environment: dce_environment,
    pub asic_id: hw_asic_id,
    pub dce_version: dce_version,
    pub dc_bios: Box<bios_parser>,
    pub created_bios: bool,
    pub gpio_service: Option<Box<gpio_service>>,
    pub dc_sink_id_count: u32,
    pub dc_stream_id_count: u32,
    pub dc_edp_id_count: u32,
    pub fbc_gpu_addr: u64,
    pub perf_trace: Option<Box<dc_perf_trace>>,
    /// `cgs_device`: created by dm_sw_init; nothing in the port reads through it.
    pub cgs_device: bool,
    /// The DCE / NBIO / CLK IP base offsets of the discovery table. DCN 2.1
    /// addresses its registers through compile-time bases and reads none.
    pub dcn_reg_offsets: Vec<u32>,
    pub nbio_reg_offsets: Vec<u32>,
    pub clk_reg_offsets: Vec<u32>,
    /// `dmub_srv`: DC's handle on the DM's DMUB service (dm_dmub_hw_init).
    pub dmub_srv: Option<Box<crate::dc_dmub_srv::dc_dmub_srv>>,
}

/// `struct dc_perf_trace` (dc_types.h): the register-access counters.
#[derive(Default)]
pub struct dc_perf_trace {
    pub read_count: u64,
    pub write_count: u64,
    pub last_entry_read: u64,
    pub last_entry_write: u64,
}

/// `dc_perf_trace_create` (dc.c).
fn dc_perf_trace_create() -> Box<dc_perf_trace> {
    Box::new(dc_perf_trace::default())
}

impl Default for dce_environment {
    fn default() -> Self { dce_environment::DCE_ENV_PRODUCTION_DRV }
}
impl Default for dce_version {
    fn default() -> Self { dce_version::DCE_VERSION_UNKNOWN }
}

/// `struct dc_init_data` (dc.h): what `dm_hw_init` hands `dc_create`. The
/// complex override/config members join when their consumers are ported.
#[derive(Default)]
pub struct dc_init_data {
    pub asic_id: hw_asic_id,
    pub num_virtual_links: i32,
    pub dce_environment: dce_environment,
    pub log_mask: u64,
    pub flags: dc_config,
    pub force_smu_not_present: bool,
    pub cgs_device: bool,
    pub dcn_reg_offsets: Vec<u32>,
    pub nbio_reg_offsets: Vec<u32>,
    pub clk_reg_offsets: Vec<u32>,
    /// `bb_from_dmub`: the DMUB bounding box, DCN 4.0.1 only.
    pub bb_from_dmub: bool,
}

/// `struct dc_config` (dc.h): the members a reached path reads.
#[derive(Default, Clone, Copy)]
pub struct dc_config {
    pub disable_dmcu: bool,
    pub is_single_rank_dimm: bool,
    pub is_asymmetric_memory: bool,
    pub force_bios_fixed_vs: u32,
    pub ignore_dpref_ss: bool,
    pub allow_edp_hotplug_detection: u32,
    pub smart_mux_version: i32,
    pub gpu_vm_support: bool,
    pub fbc_support: bool,
    pub multi_mon_pp_mclk_switch: bool,
    pub disable_fractional_pwm: bool,
    pub edp_no_power_sequencing: bool,
    /// `allow_lttpr_non_transparent_mode.bits.DP1_4A` / `.DP2_0`.
    pub allow_lttpr_non_transparent_mode_dp1_4a: bool,
    pub allow_lttpr_non_transparent_mode_dp2_0: bool,
    pub seamless_boot_edp_requested: bool,
    pub skip_riommu_prefetch_wa: bool,
    pub allow_seamless_boot_optimization: bool,
    pub enable_mipi_converter_optimization: bool,
    /// `enum dmub_ips_disable_type`.
    pub disable_ips: u32,
    pub disable_ips_in_vpb: u32,
    pub allow_0_dtb_clk: bool,
    pub support_edp0_on_dp1: bool,
    pub unify_link_enc_assignment: bool,
    pub enable_dpia_pre_training: bool,
}

/// `enum pipe_split_policy` (dc.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum pipe_split_policy {
    #[default]
    MPC_SPLIT_DYNAMIC = 0,
    MPC_SPLIT_AVOID = 1,
    MPC_SPLIT_AVOID_MULT_DISP = 2,
}

/// `enum dcc_option` (dc.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum dcc_option {
    #[default]
    DCC_ENABLE = 0,
    DCC_DISABLE = 1,
    DCC_HALF_REQ_DISALBE = 2,
}

/// `struct dc_debug_options` (dc.h): the members DCN 2.1's
/// `debug_defaults_drv` sets.
#[derive(Default, Clone, Copy)]
pub struct dc_debug_options {
    pub disable_dmcu: bool,
    pub force_abm_enable: bool,
    pub clock_trace: bool,
    pub disable_pplib_clock_request: bool,
    pub min_disp_clk_khz: i32,
    pub pipe_split_policy: pipe_split_policy,
    pub force_single_disp_pipe_split: bool,
    pub disable_dcc: dcc_option,
    pub vsr_support: bool,
    pub performance_trace: bool,
    pub max_downscale_src_width: i32,
    pub disable_pplib_wm_range: bool,
    pub scl_reset_length10: bool,
    pub sanity_checks: bool,
    pub disable_48mhz_pwrdwn: bool,
    pub usbc_combo_phy_reset_wa: bool,
    pub dmub_command_table: bool,
    pub use_max_lb: bool,
    pub enable_legacy_fast_update: bool,
    pub using_dml2: bool,
    pub hdmi20_disable: bool,
    pub disable_stutter: bool,
    pub disable_dsc: bool,
    pub disable_clock_gate: bool,
    pub force_subvp_mclk_switch: bool,
    pub force_disable_subvp: bool,
    /// `fams2_config.bits.enable`.
    pub fams2_config_enable: bool,
    pub using_dml21: bool,
    pub hdcp_lc_force_fw_enable: bool,
    pub hdcp_lc_enable_sw_fallback: bool,
    pub skip_detection_link_training: bool,
    /// `enum visual_confirm`.
    pub visual_confirm: u32,
    pub ignore_cable_id: bool,
    pub enable_dmub_aux_for_legacy_ddc: bool,
    pub disable_fixed_vs_aux_timeout_wa: bool,
    pub disable_uhbr: bool,
    /// `dpia_debug.bits.disable_dpia`.
    pub dpia_debug_disable_dpia: bool,
    pub disable_timeout: bool,
    pub disable_dpp_power_gate: bool,
    pub disable_hubp_power_gate: bool,
    pub disable_dsc_power_gate: bool,
    pub always_scale: bool,
    pub force_chroma_subsampling_1tap: bool,
    pub visual_confirm_rect_height: u32,
}

/// Which `dc_cap_funcs` entry is bound (dcn21 sets only
/// `get_dcc_compression_cap`; its body and `get_subvp_en` are later stages).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum cap_func_id {
    dcn20_get_dcc_compression_cap,
}

/// `struct dc_cap_funcs` (dc.h): the capability-query vtable.
#[derive(Default)]
pub struct dc_cap_funcs {
    pub get_dcc_compression_cap: Option<cap_func_id>,
}

/// `struct dc` (dc.h): the Display Core instance.
#[derive(Default)]
pub struct dc {
    pub ctx: dc_context,
    pub caps: dc_caps,
    pub res_pool: Option<Box<resource_pool>>,
    pub link_count: u32,
    pub versions: dc_versions,
    /// `hwseq`: the hw-sequencer register bank (`resource_construct` creates it).
    pub hwseq: Option<Box<crate::dce_hwseq::dce_hwseq>>,
    /// `dml`: the DML instance (`dml_init_instance`). Boxed because the SoC
    /// bounding box is not Default (its `clock_limits` array is > 32 entries).
    pub dml: Option<Box<crate::dml_structs::display_mode_lib>>,
    /// `cap_funcs`: the capability-query vtable (set at the end of the construct).
    pub cap_funcs: dc_cap_funcs,
    pub build_id: &'static str,
    pub config: dc_config,
    pub debug: dc_debug_options,
    /// `clk_mgr`: Renoir's is the `clk_mgr_internal` of rn_clk_mgr.
    pub clk_mgr: Option<Box<crate::clk_mgr::clk_mgr_internal>>,
    /// `links[MAX_LINKS]`, `link_count` of them.
    pub links: Vec<Box<dc_link>>,
    pub lowest_dpia_link_index: u32,
    pub current_state: Option<Box<crate::dc_state::dc_state>>,
    /// `hwss`: the hw sequencer vtable (`dcn21_hw_sequencer_construct`).
    pub hwss: crate::hw_sequencer::hw_sequencer_funcs,
    pub optimized_required: bool,
    /// `vm_pa_config`: the system context `dc_setup_system_context` keeps.
    pub vm_pa_config: dc_phy_addr_space_config,
    pub vendor_signature: dpcd_vendor_signature,
    /// The streams the DM created (`dc_create_stream_for_sink`); a context
    /// names one by its index here. `None` once released.
    pub streams: Vec<Option<Box<crate::dc_stream::dc_stream_state>>>,
    /// The plane states the DM created (`dc_create_plane_state`).
    pub planes: Vec<Option<Box<crate::dc_stream::dc_plane_state>>>,
}

/// `struct dpcd_vendor_signature` (dc_dp_types.h): the source OUI a DM may
/// set; `data` is its 12 raw bytes.
#[derive(Default, Clone, Copy, Debug)]
pub struct dpcd_vendor_signature {
    pub is_valid: bool,
    pub data: [u8; 12],
}

/// The `system_aperture` of `struct dc_phy_addr_space_config` (dc.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_system_aperture {
    pub start_addr: u64,
    pub end_addr: u64,
    pub fb_top: u64,
    pub fb_offset: u64,
    pub fb_base: u64,
    pub agp_top: u64,
    pub agp_bot: u64,
    pub agp_base: u64,
}

/// The `gart_config` of `struct dc_phy_addr_space_config` (dc.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_gart_config {
    pub page_table_start_addr: u64,
    pub page_table_end_addr: u64,
    pub page_table_base_addr: u64,
    pub base_addr_is_mc_addr: bool,
}

/// `struct dc_phy_addr_space_config` (dc.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_phy_addr_space_config {
    pub system_aperture: dc_system_aperture,
    pub gart_config: dc_gart_config,
    pub valid: bool,
    pub is_hvm_enabled: bool,
    pub page_table_default_page_addr: u64,
}

/// `dc_setup_system_context` (dc_vm_helper.c): program the DCHUB address
/// space and keep it. DML2 (`dml2_options.gpuvm_enable`) is not used on
/// DCN 2.1 and `hwss.z10_save_init` is not a member of dcn21_funcs.
pub fn dc_setup_system_context(adev: &amdgpu_device, dc: &mut dc, pa_config: &dc_phy_addr_space_config) -> i32 {
    let mut num_vmids = 0;

    // Call HWSS to setup HUBBUB for address config
    if let Some(f) = dc.hwss.init_sys_ctx {
        num_vmids = f(adev, dc, pa_config);

        dc.vm_pa_config = *pa_config;
        dc.vm_pa_config.valid = true;
    }

    num_vmids
}

/// `dc_get_edp_links` (dc_link_exports.c): the eDP links, connected or not,
/// as indices into `dc.links`.
pub fn dc_get_edp_links(dc: &dc) -> Vec<usize> {
    let mut edp_links = Vec::new();
    for (i, link) in dc.links.iter().enumerate() {
        if link.connector_signal == crate::regs::LF_SIGNAL_TYPE_EDP {
            edp_links.push(i);
            if edp_links.len() == crate::regs::MAX_NUM_EDP as usize {
                break;
            }
        }
    }
    edp_links
}

/// `detect_edp_presence` (dc.c).
pub fn detect_edp_presence(adev: &amdgpu_device, dc: &mut dc) {
    let edp_links = dc_get_edp_links(dc);
    for li in edp_links {
        // dc.config.edp_not_connected is never set on this path.
        let mut r#type = crate::regs::dc_connection_none;
        crate::link_detection::link_detect_connection_type(adev, dc, li, &mut r#type);
        dc.links[li].edp_sink_present = r#type != crate::regs::dc_connection_none;
    }
}

/// `dc_hardware_init` (dc.c): the eDP presence test, the hw sequencer's
/// init and the D0 notification to the DMCUB.
pub fn dc_hardware_init(adev: &amdgpu_device, dc: &mut dc) {
    detect_edp_presence(adev, dc);
    if dc.ctx.dce_environment != crate::dc_types::dce_environment::DCE_ENV_VIRTUAL_HW {
        (dc.hwss.init_hw.unwrap())(adev, dc);
    }
    crate::dc_dmub_srv::dc_dmub_srv_notify_fw_dc_power_state(adev, dc, crate::regs::DC_ACPI_CM_POWER_STATE_D0);
}

/// `create_links` (dc.c). The VBIOS reports the number of valid connector
/// indices, not which ones carry a connector, so an index without one is
/// skipped. USB4 DPIA links: `usb4_dpia_count` is 0 on Renoir.
pub fn create_links(adev: &amdgpu_device, dc: &mut dc, num_virtual_links: u32) -> bool {
    let bios = &adev.bios[..];
    dc.link_count = 0;

    let connectors_num = bios_parser_get_connectors_number(&dc.ctx.dc_bios, bios) as u32;
    if connectors_num > LF_ENUM_ID_COUNT {
        return false;
    }

    let (Some(gs), Some(pool)) = (dc.ctx.gpio_service.as_deref_mut(), dc.res_pool.as_deref_mut()) else {
        return false;
    };
    let mut cx = link_ctx {
        adev,
        bp: &dc.ctx.dc_bios,
        gpio_service: gs,
        dc_edp_id_count: &mut dc.ctx.dc_edp_id_count,
        link_encoders: &mut pool.link_encoders,
        dig_link_enc_count: &mut pool.dig_link_enc_count,
        is_apu: dc.caps.is_apu,
        smart_mux_version: dc.config.smart_mux_version,
        allow_edp_hotplug_detection: dc.config.allow_edp_hotplug_detection,
        hdmi20_disable: dc.debug.hdmi20_disable,
    };

    let mut i = 0;
    while dc.link_count < connectors_num && i < LF_MAX_LINKS {
        let connector_id = bios_parser_get_connector_id(cx.bp, bios, i as u8);
        if connector_id.id != 0 {
            let link_init_params = link_init_data { connector_index: i, link_index: dc.link_count, is_dpia_link: false };
            if let Some(link) = link_create(&mut cx, &link_init_params) {
                dc.links.push(link);
                dc.link_count += 1;
            }
        }
        i += 1;
    }

    dc.lowest_dpia_link_index = LF_MAX_LINKS;

    for _ in 0..num_virtual_links {
        dc.links.push(create_virtual_link(dc.link_count));
        dc.link_count += 1;
    }

    dc.caps.num_of_internal_disp = get_num_of_internal_disp(&dc.links);

    true
}

/// `DC_BUILD_ID` (dc.c).
const DC_BUILD_ID: &str = "production-build";

/// `dc_construct_ctx` (dc.c). The context is a member of `dc` here, so there
/// is no allocation to fail. `dc_stream_init_rmcm_3dlut` loops over
/// `caps.color.mpc.num_rmcm_3dluts`, which is 0 on DCN 2.1; the logger is the
/// module's own.
fn dc_construct_ctx(dc: &mut dc, init_params: &dc_init_data) -> bool {
    let dc_ctx = &mut dc.ctx;
    dc_ctx.cgs_device = init_params.cgs_device;
    dc_ctx.asic_id = init_params.asic_id;
    dc_ctx.dc_sink_id_count = 0;
    dc_ctx.dc_stream_id_count = 0;
    dc_ctx.dce_environment = init_params.dce_environment;
    dc_ctx.dcn_reg_offsets = init_params.dcn_reg_offsets.clone();
    dc_ctx.nbio_reg_offsets = init_params.nbio_reg_offsets.clone();
    dc_ctx.clk_reg_offsets = init_params.clk_reg_offsets.clone();

    dc_ctx.dce_version = resource_parse_asic_id(&init_params.asic_id);

    dc_ctx.perf_trace = Some(dc_perf_trace_create());

    // dc->link_srv = link_create_link_service(): the link/ subsystem.
    true
}

/// `dc_construct` (dc.c).
fn dc_construct(adev: &amdgpu_device, dc: &mut dc, init_params: &dc_init_data) -> bool {
    dc.config = init_params.flags;

    // vm_helper, bb_overrides and the DCN 1.0 bw_dceip/bw_vbios/dcn_soc/dcn_ip
    // blocks join with their first reader.
    if !dc_construct_ctx(dc, init_params) {
        return false;
    }

    // Create BIOS parser (no vbios_override on this path).
    let bp_init_data = bp_init_data { bios: &adev.bios };
    match dal_bios_parser_create(adev, &bp_init_data, dce_version::DCN_VERSION_2_1) {
        Some(bp) => dc.ctx.dc_bios = bp,
        None => return false,
    }
    dc.ctx.created_bios = true;

    // Create GPIO service
    dc.ctx.gpio_service = dal_gpio_service_create(dce_version::DCN_VERSION_2_1, dc.ctx.dce_environment);
    if dc.ctx.gpio_service.is_none() {
        return false;
    }

    dc.res_pool = dc_create_resource_pool(adev, dc, init_params);
    if dc.res_pool.is_none() {
        return false;
    }

    // set i2c speed if not done by the respective dcnxxx__resource.c
    if dc.caps.i2c_speed_in_khz_hdcp == 0 {
        dc.caps.i2c_speed_in_khz_hdcp = dc.caps.i2c_speed_in_khz;
    }
    if dc.caps.max_optimizable_video_width == 0 {
        dc.caps.max_optimizable_video_width = 5120;
    }

    dc.clk_mgr = dc_clk_mgr_create(adev, dc);
    if dc.clk_mgr.is_none() {
        return false;
    }
    if let Some(clk_mgr) = dc.clk_mgr.as_deref_mut() {
        clk_mgr.base.force_smu_not_present = init_params.force_smu_not_present;
    }
    // res_pool->funcs->update_bw_bounding_box
    dcn21_update_bw_bounding_box(dc);
    // dc_create_soc_and_ip_translator binds a translator for DCN 4.01 only; on
    // 2.1 it is an empty allocation.
    if !create_links(adev, dc, init_params.num_virtual_links as u32) {
        return false;
    }

    // create_link_encoders: a platform without USB4 DPIA endpoints (Renoir:
    // num_usb4_dpia 0) maps DIG encoders to endpoints fixed; nothing to add.
    // current_state is created after dc->dml, which it copies.
    dc.current_state = Some(dc_state_create(dc, None));
    true
}

/// `dc_create` (dc.c), the hardware path (`dce_environment` is not
/// `DCE_ENV_VIRTUAL_HW` on Renoir).
pub fn dc_create(adev: &amdgpu_device, init_params: &dc_init_data) -> Option<Box<dc>> {
    let mut dc = Box::new(dc::default());

    if !dc_construct(adev, &mut dc, init_params) {
        return None;
    }

    let res_pool = dc.res_pool.as_deref()?;
    let mut full_pipe_count = res_pool.pipe_count;
    if res_pool.underlay_pipe_index != NO_UNDERLAY_PIPE {
        full_pipe_count -= 1;
    }
    let stream_enc_count = res_pool.stream_enc_count;
    let audio_count = res_pool.audio_count;
    let num_timing_generator = res_pool.res_cap.map_or(0, |c| c.num_timing_generator as u32);

    dc.caps.max_streams = full_pipe_count.min(stream_enc_count);
    dc.caps.max_links = dc.link_count;
    dc.caps.max_audios = audio_count;
    dc.caps.linear_pitch_alignment = 64;

    dc.caps.max_dp_protocol_version = dp_protocol_version::DP_VERSION_1_4;

    dc.caps.max_otg_num = num_timing_generator;

    // res_pool->dmcu is NULL on DCN 2.1: versions.dmcu_version stays zero.

    dc.versions.dc_ver = DC_VER;

    dc.build_id = DC_BUILD_ID;

    Some(dc)
}
