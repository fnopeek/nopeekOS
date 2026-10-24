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
use crate::regs::{LF_ENUM_ID_COUNT, LF_MAX_LINKS, LF_SIGNAL_TYPE_EDP};
use crate::dc_types::{dc_status, MAX_PIPES};
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
    pub set_pipe_unlock_order: bool,
    pub enable_4to1MPC: bool,
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
    pub disable_timing_sync: bool,
    pub min_dram_clk_khz: i32,
    pub force_odm_combine: u32,
    pub force_odm_combine_4to1: u32,
    pub minimum_z8_residency_time: u32,
    pub force_chroma_subsampling_1tap: bool,
    pub visual_confirm_rect_height: u32,
    pub enable_driver_sequence_debug: bool,
    pub disable_idle_power_optimizations: bool,
    /// `enum lttpr_mode`.
    pub lttpr_mode_override: u32,
    pub avoid_vbios_exec_table: bool,
    pub disable_extblankadj: bool,
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
    pub dml: Option<crate::dml_structs::DmlBox>,
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
    pub idle_optimizations_allowed: bool,
    pub work_arounds: dc_work_arounds,
    /// `vm_pa_config`: the system context `dc_setup_system_context` keeps.
    pub vm_pa_config: dc_phy_addr_space_config,
    pub vendor_signature: dpcd_vendor_signature,
    pub bb_overrides: dc_bounding_box_overrides,
    /// The streams the DM created (`dc_create_stream_for_sink`); a context
    /// names one by its index here. `None` once released.
    pub streams: Vec<Option<Box<crate::dc_stream::dc_stream_state>>>,
    /// The plane states the DM created (`dc_create_plane_state`).
    pub planes: Vec<Option<Box<crate::dc_stream::dc_plane_state>>>,
}

/// `struct dc_work_arounds` (dc.h), the members reached.
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_work_arounds {
    pub skip_clock_update: bool,
}

/// `struct dc_bounding_box_overrides` (dc.h): bounding-box values the DM
/// may impose; zero means none.
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_bounding_box_overrides {
    pub sr_exit_time_ns: i32,
    pub sr_enter_plus_exit_time_ns: i32,
    pub sr_exit_z8_time_ns: i32,
    pub sr_enter_plus_exit_z8_time_ns: i32,
    pub urgent_latency_ns: i32,
    pub percent_of_ideal_drambw: i32,
    pub dram_clock_change_latency_ns: i32,
    pub dummy_clock_change_latency_ns: i32,
    pub fclk_clock_change_latency_ns: i32,
    pub min_dcfclk_mhz: i32,
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

fn not_ported(name: &str) {
    crate::linux::Line::new().s("dc: ").s(name).s(" not ported").say();
}

/// `MAX_STREAMS` (dc.h).
pub const MAX_STREAMS: usize = 6;

/// `struct dc_validation_set` (dc.h): a stream and its planes, as indices
/// into dc's arenas.
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_validation_set {
    pub stream: usize,
    pub plane_states: [Option<usize>; crate::dc_types::MAX_SURFACES],
    pub plane_count: u8,
}

/// `struct dc_commit_streams_params` (dc.h).
pub struct dc_commit_streams_params<'a> {
    pub streams: &'a [usize],
    pub power_source: crate::dc_state::dc_power_source_type,
}

/// `streams_changed` (dc.c).
fn streams_changed(dc: &dc, streams: &[usize]) -> bool {
    let cur = dc.current_state.as_deref().unwrap();

    if streams.len() as u32 != cur.stream_count {
        return true;
    }

    for i in 0..cur.stream_count as usize {
        if cur.streams[i] != Some(streams[i]) {
            return true;
        }
        let link = dc.streams[streams[i]].as_deref().unwrap().link;
        if !dc.links[link].link_state_valid {
            return true;
        }
    }

    false
}

/// `dc_exit_ips_for_hw_access` (dc.c). DCN 2.1 has no IPS, so this does
/// nothing; `dc_allow_idle_optimizations_internal` would be its body.
pub fn dc_exit_ips_for_hw_access(dc: &dc) {
    if dc.caps.ips_support {
        not_ported("dc_allow_idle_optimizations_internal");
    }
}

/// `dc_stream_get_status` (dc_stream.c): the slot of `stream` in the
/// current state.
pub fn dc_stream_get_status(dc: &dc, stream: usize) -> Option<usize> {
    crate::dc_state::dc_state_get_stream_status(dc.current_state.as_deref().unwrap(), stream)
}

/// `get_seamless_boot_stream_count` (dc.c).
fn get_seamless_boot_stream_count(dc: &dc, ctx: &crate::dc_state::dc_state) -> u32 {
    let mut seamless_boot_stream_count = 0;

    for i in 0..ctx.stream_count as usize {
        if dc.streams[ctx.streams[i].unwrap()].as_deref().unwrap().apply_seamless_boot_optimization {
            seamless_boot_stream_count += 1;
        }
    }

    seamless_boot_stream_count
}

/// `dc_z10_restore` (dc.c). `z10_restore` is not a member of dcn21_funcs.
fn dc_z10_restore(_dc: &dc) {}

/// `dc_allow_idle_optimizations` (dc.c): with neither a DMUB IPS nor an
/// `apply_idle_power_optimizations` in dcn21_funcs, and the debug flags
/// at their defaults, this leaves `idle_optimizations_allowed` alone; on
/// DCN 2.1 it only logs.
fn dc_allow_idle_optimizations(dc: &mut dc, allow: bool) {
    if dc.debug.disable_idle_power_optimizations {
        return;
    }
    if allow == dc.idle_optimizations_allowed {
        return;
    }
    // dc->hwss.apply_idle_power_optimizations is NULL on DCN 2.1.
}

/// `disable_vbios_mode_if_required` (dc.c): a lit eDP whose pixel clock is
/// not the one the new state asks for is switched off first.
fn disable_vbios_mode_if_required(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    use crate::dcn21_link_encoder::ENGINE_ID_UNKNOWN;
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;

    // check if timing_changed, disable stream
    for i in 0..pipe_count {
        let pipe = context.res_ctx.pipe_ctx[i];
        let Some(s) = pipe.stream else { continue };
        let stream = dc.streams[s].as_deref().unwrap();

        if stream.apply_seamless_boot_optimization {
            continue;
        }

        // only looking for first odm pipe
        if pipe.prev_odm_pipe.is_some() {
            continue;
        }

        let li = stream.link;
        let link = (dc.links[li].local_sink.as_deref().is_some_and(|s| s.sink_signal == LF_SIGNAL_TYPE_EDP))
            .then_some(li);

        let Some(li) = link else { continue };
        let enc = dc.links[li].link_enc.as_ref().unwrap();
        if (enc.funcs().is_dig_enabled.unwrap())(adev, enc) {
            let mut tg_inst = 0;
            let mut pix_clk_100hz = 0;

            let enc_inst = (enc.funcs().get_dig_frontend.unwrap())(adev, enc);
            if enc_inst != ENGINE_ID_UNKNOWN {
                let pool = dc.res_pool.as_deref().unwrap();
                for j in 0..pool.stream_enc_count as usize {
                    let se = pool.stream_enc[j].as_ref().unwrap();
                    if se.base().id as u32 == enc_inst {
                        tg_inst = (se.funcs().dig_source_otg.unwrap())(adev, se);
                        break;
                    }
                }

                let dp = pool.dp_clock_source.as_deref().unwrap();
                (dp.funcs().get_pixel_clk_frequency_100hz.unwrap())(adev, dp, tg_inst, &mut pix_clk_100hz);

                if dc.links[li].link_status.link_active {
                    let requested_pix_clk_100hz = pipe.stream_res.pix_clk_params.requested_pix_clk_100hz;

                    if pix_clk_100hz != requested_pix_clk_100hz {
                        crate::link_dpms::link_set_dpms_off(adev, dc, context, i);
                        dc.streams[s].as_deref_mut().unwrap().dpms_off = false;
                    }
                }
            }
        }
    }
}

/// `dc_validate_boot_timing` (dc.c): whether the eDP the VBIOS lit runs
/// the timing `crtc_timing` asks for, so the first mode set can keep it.
pub fn dc_validate_boot_timing(adev: &amdgpu_device, dc: &mut dc, sink_link: usize, sink_signal: u32,
                               crtc_timing: &crate::dc_hw_types::dc_crtc_timing) -> bool {
    use crate::dcn21_link_encoder::ENGINE_ID_UNKNOWN;
    let mut hw_crtc_timing = crate::dc_hw_types::dc_crtc_timing::default();

    // Support seamless boot on EDP displays only
    if sink_signal != LF_SIGNAL_TYPE_EDP {
        return false;
    }

    if dc.debug.force_odm_combine != 0 {
        return false;
    }

    let link = &dc.links[sink_link];
    let enc = link.link_enc.as_ref().unwrap();

    // Check for enabled DIG to identify enabled display
    if !(enc.funcs().is_dig_enabled.unwrap())(adev, enc) {
        return false;
    }

    let enc_inst = (enc.funcs().get_dig_frontend.unwrap())(adev, enc);

    if enc_inst == ENGINE_ID_UNKNOWN {
        return false;
    }

    let pool = dc.res_pool.as_deref().unwrap();
    let mut tg_inst = 0;
    let mut se = None;
    for i in 0..pool.stream_enc_count as usize {
        let e = pool.stream_enc[i].as_ref().unwrap();
        if e.base().id as u32 == enc_inst {
            se = Some(i);
            tg_inst = (e.funcs().dig_source_otg.unwrap())(adev, e);
            break;
        }
    }

    // tg_inst not found
    let Some(se) = se else { return false };

    if tg_inst >= pool.timing_generator_count {
        return false;
    }

    if tg_inst != enc.base().preferred_engine {
        return false;
    }

    let tg = pool.timing_generators[tg_inst as usize].as_deref().unwrap();

    let Some(get_hw_timing) = tg.funcs().get_hw_timing else { return false };

    if !get_hw_timing(adev, tg, &mut hw_crtc_timing) {
        return false;
    }

    let (c, h) = (crtc_timing, &hw_crtc_timing);
    if c.h_total != h.h_total || c.h_border_left != h.h_border_left || c.h_addressable != h.h_addressable
        || c.h_border_right != h.h_border_right || c.h_front_porch != h.h_front_porch
        || c.h_sync_width != h.h_sync_width || c.v_total != h.v_total || c.v_border_top != h.v_border_top
        || c.v_addressable != h.v_addressable || c.v_border_bottom != h.v_border_bottom
        || c.v_front_porch != h.v_front_porch || c.v_sync_width != h.v_sync_width
    {
        return false;
    }

    // block DSC for now, as VBIOS does not currently support DSC timings
    if crtc_timing.flags.DSC {
        return false;
    }

    if crate::link_detection::dc_is_dp_signal(link.connector_signal) {
        let mut pix_clk_100hz = 0;
        let mut num_odm_pipes = 1;
        let mut id_src = [0u32; 4];

        let dp = pool.dp_clock_source.as_deref().unwrap();
        (dp.funcs().get_pixel_clk_frequency_100hz.unwrap())(adev, dp, tg_inst, &mut pix_clk_100hz);

        if let Some(f) = tg.funcs().get_optc_source {
            let (a, b) = id_src.split_at_mut(1);
            f(adev, tg, &mut num_odm_pipes, &mut a[0], &mut b[0]);
        }

        if num_odm_pipes == 2 {
            pix_clk_100hz *= 2;
        } else if num_odm_pipes == 4 {
            pix_clk_100hz *= 4;
        }
        // dcn20_str_enc_funcs has no get_pixels_per_cycle.

        // Note: In rare cases, HW pixclk may differ from crtc's pixclk
        // slightly due to rounding issues in 10 kHz units.
        if crtc_timing.pix_clk_100hz != pix_clk_100hz {
            return false;
        }

        let e = pool.stream_enc[se].as_ref().unwrap();
        let Some(dp_get_pixel_format) = e.funcs().dp_get_pixel_format else { return false };

        if !dp_get_pixel_format(adev, e, &mut hw_crtc_timing.pixel_encoding, &mut hw_crtc_timing.display_color_depth) {
            return false;
        }

        if hw_crtc_timing.display_color_depth != crtc_timing.display_color_depth {
            return false;
        }

        if hw_crtc_timing.pixel_encoding != crtc_timing.pixel_encoding {
            return false;
        }
    }

    if crate::dc_dp_types::bits(link.dpcd_caps.dprx_feature, 3, 1) != 0 {
        return false;
    }

    if crate::dc_dp_types::channel_coding_cap_DP_128b_132b_SUPPORTED(link.dpcd_caps.channel_coding_cap) != 0 {
        return false;
    }

    if crate::link_dp_training::edp_is_ilr_optimization_required(adev, dc, sink_link, crtc_timing) {
        return false;
    }

    true
}

/// `disable_dangling_plane` (dc.c): every stream of the current state the
/// new one drops loses its planes before the new state is applied. The
/// planes are reprogrammed through `program_front_end_for_ctx` on a copy
/// of the current state without them.
fn disable_dangling_plane(adev: &amdgpu_device, dc: &mut dc, context: &crate::dc_state::dc_state) {
    use crate::dc_state::*;
    let mut dangling_context = dc_state_create_current_copy(dc);
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;

    for i in 0..pipe_count {
        let cur = dc.current_state.as_deref().unwrap();
        let old_stream = cur.res_ctx.pipe_ctx[i].stream;
        let mut should_disable = true;

        let new_top = context.res_ctx.pipe_ctx[i].top_pipe;
        let old_top = cur.res_ctx.pipe_ctx[i].top_pipe;
        let pipe_split_change = match (new_top, old_top) {
            (Some(n), Some(o)) => context.res_ctx.pipe_ctx[n].pipe_idx != cur.res_ctx.pipe_ctx[o].pipe_idx,
            _ => new_top != old_top,
        };

        for j in 0..context.stream_count as usize {
            if old_stream == context.streams[j] {
                should_disable = false;
                break;
            }
        }
        if !should_disable && pipe_split_change && cur.stream_count != context.stream_count {
            should_disable = true;
        }

        if old_stream.is_some() && cur.res_ctx.pipe_ctx[i].top_pipe.is_none()
            && cur.res_ctx.pipe_ctx[i].prev_odm_pipe.is_none()
        {
            let old_pipe = &cur.res_ctx.pipe_ctx[i];
            let new_pipe = &context.res_ctx.pipe_ctx[i];

            if old_pipe.plane_state.is_some() && new_pipe.plane_state.is_none() {
                should_disable = true;
            }
        }

        if should_disable && let Some(old_stream) = old_stream {
            // A stream of the current state is never a SubVP phantom on
            // DCN 2.1.
            dc_state_rem_all_planes_for_stream(dc, old_stream, &mut dangling_context);
            // disable_all_writeback_pipes_for_stream
            not_ported("disable_dangling_plane: disable_all_writeback_pipes_for_stream");

            let pipe = dc.current_state.as_deref().unwrap().res_ctx.pipe_ctx[i];
            if pipe.stream.is_some() && pipe.plane_state.is_some() {
                // set_p_state_switch_method is a DCN 3.2 FAMS path;
                // dc_update_visual_confirm_color with visual confirm off.
                not_ported("disable_dangling_plane: dc_update_visual_confirm_color");
            }

            // apply_ctx_for_surface is NULL on DCN 2.1, and so is
            // prepare_mcache_programming.
            (dc.hwss.interdependent_update_lock.unwrap())(adev, dc, None, true);
            (dc.hwss.program_front_end_for_ctx.unwrap())(adev, dc, &mut dangling_context);
            (dc.hwss.interdependent_update_lock.unwrap())(adev, dc, None, false);
            (dc.hwss.post_unlock_program_front_end.unwrap())(adev, dc, &mut dangling_context);
        }
    }

    let current_ctx = dc.current_state.replace(dangling_context).unwrap();
    dc_state_release(dc, current_ctx);
}

/// `dc_trigger_sync` (dc.c): with `enable_timing_synchronization` the
/// timing generators of synchronizable streams are locked together.
fn dc_trigger_sync(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state) {
    if context.stream_count > 1 && !dc.debug.disable_timing_sync {
        not_ported("dc_trigger_sync: enable_timing_synchronization");
    }
    let _ = adev;
}

/// `determine_pipe_unlock_order` (dc.c): without `dc->config.
/// set_pipe_unlock_order` nothing is ordered.
fn determine_pipe_unlock_order(dc: &dc, context: &mut crate::dc_state::dc_state) {
    if !dc.config.set_pipe_unlock_order {
        return;
    }
    let _ = context;
    not_ported("determine_pipe_unlock_order");
}

/// `should_update_pipe_for_stream` (dc.c).
fn should_update_pipe_for_stream(context: &crate::dc_state::dc_state, pipe: usize, stream: usize) -> bool {
    context.res_ctx.pipe_ctx[pipe].stream == Some(stream)
}

/// `dc_enable_stereo` (dc.c).
fn dc_enable_stereo(adev: &amdgpu_device, dc: &mut dc, context: &mut crate::dc_state::dc_state,
                    streams: &[Option<usize>]) {
    dc_exit_ips_for_hw_access(dc);

    for i in 0..MAX_PIPES {
        for &stream in streams {
            let Some(stream) = stream else { continue };
            if !should_update_pipe_for_stream(context, i, stream) {
                continue;
            }
            if let Some(setup_stereo) = dc.hwss.setup_stereo {
                setup_stereo(adev, dc, context, i);
            }
        }
    }
}

/// `get_stream_mask` (dc.c): a bit for each pipe with a stream.
fn get_stream_mask(dc: &dc, context: &crate::dc_state::dc_state) -> u32 {
    let mut stream_mask = 0u32;

    for i in 0..dc.res_pool.as_deref().unwrap().pipe_count as usize {
        if context.res_ctx.pipe_ctx[i].stream.is_some() {
            stream_mask |= 1 << i;
        }
    }

    stream_mask as u8 as u32
}

/// `dc_commit_state_no_check` (dc.c): apply `context` to the hardware and
/// make it the current state. On success the context is moved into
/// `dc.current_state` (one reference, as Linux' retain and the caller's
/// release leave it); on a failure of `apply_ctx_to_hw` it stays with the
/// caller.
fn dc_commit_state_no_check(adev: &amdgpu_device, dc: &mut dc, context_slot: &mut Option<Box<crate::dc_state::dc_state>>)
                            -> dc_status {
    use crate::dc_state::*;
    let mut context = context_slot.take().unwrap();
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;

    dc_z10_restore(dc);
    dc_allow_idle_optimizations(dc, false);

    // Check old context for SubVP: DCN 2.1 has none.
    let subvp_prev_use = false;

    let mut dc_streams = [None; MAX_STREAMS];
    for i in 0..context.stream_count as usize {
        dc_streams[i] = context.streams[i];
    }

    if !crate::bios_parser2::bios_parser_is_accelerated_mode(adev, &dc.ctx.dc_bios.base) {
        disable_vbios_mode_if_required(adev, dc, &mut context);
        (dc.hwss.enable_accelerated_mode.unwrap())(adev, dc, &mut context);
    }

    // wait_for_pipe_update_if_needed is not a member of dcn21_private_funcs.

    if context.stream_count > get_seamless_boot_stream_count(dc, &context) || context.stream_count == 0 {
        (dc.hwss.prepare_bandwidth.unwrap())(adev, dc, &mut context);
    }

    // subvp_pipe_control_lock, fams2_global_control_lock and update_dsc_pg
    // are not members of dcn21_funcs.
    let _ = subvp_prev_use;

    disable_dangling_plane(adev, dc, &context);
    // apply_ctx_for_surface is NULL on DCN 2.1.

    // Program hardware
    for i in 0..pipe_count {
        (dc.hwss.wait_for_mpcc_disconnect.unwrap())(adev, dc, &context.res_ctx.pipe_ctx[i]);
    }

    let result = (dc.hwss.apply_ctx_to_hw.unwrap())(adev, dc, &mut context);

    if result != dc_status::DC_OK {
        // Application of dc_state to hardware stopped.
        dc.current_state.as_deref_mut().unwrap().res_ctx.link_enc_cfg_ctx.mode = link_enc_cfg_mode::LINK_ENC_CFG_STEADY;
        *context_slot = Some(context);
        return result;
    }

    dc_trigger_sync(adev, dc, &mut context);

    // Full update should unconditionally be triggered when
    // dc_commit_state_no_check is called
    for i in 0..context.stream_count as usize {
        let s = dc.streams[context.streams[i].unwrap()].as_deref_mut().unwrap();
        let prev_dsc_changed = s.update_flags & crate::dc_stream::UPDATE_FLAG_DSC_CHANGED;

        s.update_flags = 0xFFFF_FFFF & !crate::dc_stream::UPDATE_FLAG_DSC_CHANGED | prev_dsc_changed;
    }

    determine_pipe_unlock_order(dc, &mut context);
    // Program all planes within new context; prepare_mcache_programming is
    // a DCN 4 member of the pool.
    if let Some(program_front_end_for_ctx) = dc.hwss.program_front_end_for_ctx {
        (dc.hwss.interdependent_update_lock.unwrap())(adev, dc, Some(&*context), true);
        program_front_end_for_ctx(adev, dc, &mut context);

        // set_wait_for_update_needed_for_pipe is not a member of
        // dcn21_private_funcs.

        (dc.hwss.interdependent_update_lock.unwrap())(adev, dc, Some(&*context), false);
        (dc.hwss.post_unlock_program_front_end.unwrap())(adev, dc, &mut context);
    }

    // commit_subvp_config, subvp_pipe_control_lock and
    // fams2_global_control_lock are not members of dcn21_funcs.

    for i in 0..context.stream_count as usize {
        let s = context.streams[i].unwrap();
        if !dc.streams[s].as_deref().unwrap().mode_changed {
            continue;
        }

        // apply_ctx_for_surface is NULL on DCN 2.1.

        // enable stereo
        for k in 0..MAX_PIPES {
            for l in 0..context.stream_count as usize {
                if context.streams[l].is_some() && context.streams[l] == context.res_ctx.pipe_ctx[k].stream
                    && let Some(setup_stereo) = dc.hwss.setup_stereo
                {
                    setup_stereo(adev, dc, &mut context, k);
                }
            }
        }
    }

    let n = context.stream_count as usize;
    dc_enable_stereo(adev, dc, &mut context, &dc_streams[..n]);

    if get_seamless_boot_stream_count(dc, &context) == 0 || context.stream_count == 0 {
        // Must wait for no flips to be pending before doing optimize bw
        crate::hw_sequencer::hwss_wait_for_no_pipes_pending(adev, dc, &context);
        // optimized dispclk depends on ODM setup. Need to wait for ODM
        // update pending complete before optimizing bandwidth.
        crate::hw_sequencer::hwss_wait_for_odm_update_pending_complete(adev, dc, &context);
        // pplib is notified if disp_num changed
        (dc.hwss.optimize_bandwidth.unwrap())(adev, dc, &mut context);
        // Need to do otg sync again as otg could be out of sync due to otg
        // workaround applied during clock update
        dc_trigger_sync(adev, dc, &mut context);
    }

    // update_dsc_pg is not a member of dcn21_funcs.

    context.stream_mask = get_stream_mask(dc, &context);

    if context.stream_mask != dc.current_state.as_deref().unwrap().stream_mask {
        crate::dc_dmub_srv::dc_dmub_srv_notify_stream_mask(adev, dc, context.stream_mask);
    }

    for i in 0..context.stream_count as usize {
        dc.streams[context.streams[i].unwrap()].as_deref_mut().unwrap().mode_changed = false;
    }

    // Clear update flags that were set earlier to avoid redundant programming
    for i in 0..context.stream_count as usize {
        dc.streams[context.streams[i].unwrap()].as_deref_mut().unwrap().update_flags = 0;
    }

    let old_state = dc.current_state.replace(context).unwrap();

    dc_state_release(dc, old_state);

    dc_state_retain(dc.current_state.as_deref_mut().unwrap());

    result
}

/// `dc_commit_streams` (dc.c): validate `params.streams` against a copy
/// of the current state and commit it. The ODM 2:1 exit and the minimal
/// transition state are DCN 3.x paths and named, not ported.
pub fn dc_commit_streams(adev: &amdgpu_device, dc: &mut dc, params: &dc_commit_streams_params) -> dc_status {
    use crate::dc_state::*;
    let mut res = dc_status::DC_OK;
    let mut set = [dc_validation_set::default(); MAX_STREAMS];
    let stream_count = params.streams.len();

    if dc.ctx.dce_environment == crate::dc_types::dce_environment::DCE_ENV_VIRTUAL_HW {
        return res;
    }

    if !streams_changed(dc, params.streams) && dc.current_state.as_deref().unwrap().power_source == params.power_source {
        return res;
    }

    dc_exit_ips_for_hw_access(dc);

    for i in 0..stream_count {
        let stream = params.streams[i];
        let status = dc_stream_get_status(dc, stream);
        let sink_signal = dc.streams[stream].as_deref().unwrap().sink.as_deref().unwrap().sink_signal;

        // revalidate streams
        if !crate::link_detection::dc_is_virtual_signal(sink_signal) {
            res = crate::dc_resource::dc_validate_stream(adev, dc, stream);
            if res != dc_status::DC_OK {
                return res;
            }
        }

        // dc_stream_log logs only.

        set[i].stream = stream;

        if let Some(status) = status {
            let st = &dc.current_state.as_deref().unwrap().stream_status[status];
            set[i].plane_count = st.plane_count as u8;
            for j in 0..st.plane_count as usize {
                set[i].plane_states[j] = st.plane_states[j];
            }
        }
    }

    // ODM Combine 2:1 power optimization is only applied for single stream
    // scenario, it uses extra pipes than needed to reduce power consumption
    // We need to switch off this feature to make room for new streams.
    let cur = dc.current_state.as_deref().unwrap();
    if stream_count as u32 > cur.stream_count && cur.stream_count == 1 {
        let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;
        if (0..pipe_count).any(|i| cur.res_ctx.pipe_ctx[i].next_odm_pipe.is_some()) {
            not_ported("dc_commit_streams: commit_minimal_transition_state");
        }
    }

    let mut context = dc_state_create_current_copy(dc);

    context.power_source = params.power_source;

    res = crate::dc_resource::dc_validate_with_context(adev, dc, &set[..stream_count], &mut context,
        crate::regs::DC_VALIDATE_MODE_AND_PROGRAMMING);

    // link_encs_assign is a DCN 3.1+ member of the pool;
    // is_pipe_topology_transition_seamless a DCN 3.2 member of the hwss.

    let mut slot = Some(context);
    if res == dc_status::DC_OK {
        res = dc_commit_state_no_check(adev, dc, &mut slot);

        let context = match slot.as_deref() {
            Some(c) => c,
            None => dc.current_state.as_deref().unwrap(),
        };
        let mut abm = [None; MAX_STREAMS];
        let mut otg = [None; MAX_STREAMS];
        for i in 0..stream_count {
            for j in 0..context.stream_count as usize {
                let ps = params.streams[i];
                let cs = context.streams[j].unwrap();
                if dc.streams[ps].as_deref().unwrap().stream_id == dc.streams[cs].as_deref().unwrap().stream_id {
                    otg[i] = Some(context.stream_status[j].primary_otg_inst);
                }

                if crate::link_detection::dc_is_embedded_signal(dc.streams[ps].as_deref().unwrap().signal) {
                    let Some(status) = dc_state_get_stream_status(context, ps) else { continue };

                    abm[i] = Some((status, match dc.hwss.is_abm_supported {
                        Some(f) => f(dc, context, ps),
                        None => true,
                    }));
                }
            }
        }
        for i in 0..stream_count {
            if let Some(o) = otg[i] {
                dc.streams[params.streams[i]].as_deref_mut().unwrap().out_otg_offset = o;
            }
        }
        let context = match slot.as_deref_mut() {
            Some(c) => c,
            None => dc.current_state.as_deref_mut().unwrap(),
        };
        for (status, supported) in abm.into_iter().flatten() {
            context.stream_status[status].is_abm_supported = supported;
        }
    }

    // fail:
    match slot {
        Some(context) => dc_state_release(dc, context),
        None => dc.current_state.as_deref_mut().unwrap().refcount -= 1,
    }

    res
}

/// `dc_set_generic_gpio_for_stereo` (dc.c): the stereo sync through the
/// generic GPIO mux, on or off.
pub fn dc_set_generic_gpio_for_stereo(adev: &amdgpu_device, dc: &mut dc, enable: bool) -> bool {
    use crate::gpio_base::*;
    use crate::gpio_service::*;
    let service = dc.ctx.gpio_service.as_deref_mut().unwrap();
    let pin_info = dal_gpio_get_generic_pin_info(service, Some(gpio_id::GPIO_ID_GENERIC), 0);

    if pin_info.mask == 0xFFFFFFFF || pin_info.offset == 0xFFFFFFFF {
        return false;
    }
    let Some(mut generic) = dal_gpio_service_create_generic_mux(service, pin_info.offset, pin_info.mask) else {
        return false;
    };

    let mut gpio_result = dal_gpio_open(adev, service, &mut generic, gpio_mode::GPIO_MODE_OUTPUT);

    let config = gpio_generic_mux_config {
        enable_output_from_mux: enable,
        mux_select: crate::regs::GPIO_SIGNAL_SOURCE_PASS_THROUGH_STEREO_SYNC,
    };

    if gpio_result == gpio_result::GPIO_RESULT_OK {
        gpio_result = dal_mux_setup_config(adev, &generic, &config);
    }

    dal_gpio_close(adev, service, &mut generic);
    // dal_gpio_destroy_generic_mux frees the pin.
    gpio_result == gpio_result::GPIO_RESULT_OK
}
