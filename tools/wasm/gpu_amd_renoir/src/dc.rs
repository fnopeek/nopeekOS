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
    pub force_smu_not_present: bool,
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
}

/// `DC_BUILD_ID` (dc.c).
const DC_BUILD_ID: &str = "production-build";

/// `dc_construct_ctx` (dc.c). The context is a member of `dc` here, so there
/// is no allocation to fail. `dc_stream_init_rmcm_3dlut` loops over
/// `caps.color.mpc.num_rmcm_3dluts`, which is 0 on DCN 2.1; the logger is the
/// module's own.
fn dc_construct_ctx(dc: &mut dc, init_params: &dc_init_data) -> bool {
    let dc_ctx = &mut dc.ctx;
    dc_ctx.asic_id = init_params.asic_id;
    dc_ctx.dc_sink_id_count = 0;
    dc_ctx.dc_stream_id_count = 0;
    dc_ctx.dce_environment = init_params.dce_environment;

    dc_ctx.dce_version = resource_parse_asic_id(&init_params.asic_id);

    dc_ctx.perf_trace = Some(dc_perf_trace_create());

    // dc->link_srv = link_create_link_service(): the link/ subsystem.
    true
}

/// `dc_construct` (dc.c).
fn dc_construct(adev: &amdgpu_device, dc: &mut dc, init_params: &dc_init_data) -> bool {
    // dc->config, vm_helper, bb_overrides and the DCN 1.0 bw_dceip/bw_vbios/
    // dcn_soc/dcn_ip blocks join with their first reader.
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

    // dc->clk_mgr = dc_clk_mgr_create(): rn_clk_mgr (stage B), then
    // update_bw_bounding_box and the soc/ip translator.
    // create_links(), create_link_encoders(): the link/ subsystem.
    // dc->current_state = dc_state_create(dc, NULL).
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
