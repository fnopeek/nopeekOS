//! display/dc/core/dc_state.c + core/dc_link_enc_cfg.c (link_enc_cfg_init)
//! (Linux 6.18.55): the DC state `dc_construct` ends with. Grown as reached:
//! the DML copy, the clock manager it refers to, the DIG link encoder
//! assignment context, and the streams, planes and pipes of a commit.
#![allow(dead_code)]

use crate::dc::dc;
use crate::dc_resource::MAX_DIG_LINK_ENCODERS;
use crate::dc_dp_types::dc_link_settings;
use crate::dc_hw_types::scaler_data;
use crate::dc_resource::MAX_CLOCK_SOURCES;
use crate::dc_types::{graphics_object_id, MAX_PIPES, MAX_SURFACES};
use crate::dce_clock_source::{pixel_clk_params, pll_settings};
use crate::dcn21_link_encoder::ENGINE_ID_UNKNOWN;
use alloc::boxed::Box;

/// `enum link_enc_cfg_mode` (link_encoder.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum link_enc_cfg_mode {
    #[default]
    LINK_ENC_CFG_STEADY,
    LINK_ENC_CFG_TRANSIENT,
}

/// `struct display_endpoint_id` (dc_types.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct display_endpoint_id {
    pub link_id: graphics_object_id,
    pub ep_type: u32,
}

/// `struct link_enc_assignment` (link_encoder.h). `stream` is a stream
/// reference; there are none before the first commit.
#[derive(Clone, Copy, Debug, Default)]
pub struct link_enc_assignment {
    pub valid: bool,
    pub ep_id: display_endpoint_id,
    pub eng_id: u32,
    pub stream: Option<u32>,
}

/// `struct link_enc_cfg_context` (core_types.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct link_enc_cfg_context {
    pub mode: link_enc_cfg_mode,
    pub link_enc_assignments: [link_enc_assignment; MAX_PIPES],
    pub link_enc_avail: [u32; MAX_DIG_LINK_ENCODERS],
    pub transient_assignments: [link_enc_assignment; MAX_PIPES],
}

/// `struct stream_resource` (core_types.h), the members reached. The
/// blocks are named by their index in the resource pool's arrays; `abm`
/// says whether it holds the pool's one.
#[derive(Clone, Copy, Debug, Default)]
pub struct stream_resource {
    pub opp: Option<usize>,
    pub tg: Option<usize>,
    pub stream_enc: Option<usize>,
    pub audio: Option<usize>,
    pub pix_clk_params: pixel_clk_params,
    pub abm: bool,
    pub gsl_group: i32,
    pub encoder_info_frame: crate::dc_stream::encoder_info_frame,
    /// `dsc`: the pool index of the stream's DSC.
    pub dsc: Option<usize>,
}

/// `struct plane_resource` (core_types.h), the members reached. DCN has no
/// `mi` or `xfm`.
#[derive(Clone, Copy, Debug, Default)]
pub struct plane_resource {
    pub scl_data: scaler_data,
    /// `bw.dppclk_khz` (struct dcn_fe_bandwidth).
    pub bw_dppclk_khz: i32,
    pub hubp: Option<usize>,
    pub ipp: Option<usize>,
    pub dpp: Option<usize>,
    pub mpcc_inst: u8,
}

/// `struct link_resource` (core_types.h): `dio_link_enc` is the index of
/// the link owning the encoder.
#[derive(Clone, Copy, Debug, Default)]
pub struct link_resource {
    pub dio_link_enc: Option<u32>,
}

/// `struct dc_tunnel_settings` (dc_dp_types.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct dc_tunnel_settings {
    pub should_enable_dp_tunneling: bool,
    pub should_use_dp_bw_allocation: bool,
    pub cm_id: u8,
    pub group_id: u8,
    pub bw_granularity: u32,
    pub estimated_bw: u32,
    pub allocated_bw: u32,
}

/// `struct link_config` (core_types.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct link_config {
    pub dp_link_settings: dc_link_settings,
    pub dp_tunnel_settings: dc_tunnel_settings,
}

/// A `struct clock_source *` of a pipe: one of the pool's PLLs, or its DP
/// DTO source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum clock_source_ref {
    Pll(usize),
    Dp,
}

/// `struct pipe_ctx` (core_types.h), the members reached. A stream and a
/// plane state are indices into dc's arenas, the neighbouring pipes indices
/// into the same context's `pipe_ctx`.
#[derive(Clone, Copy, Debug, Default)]
pub struct pipe_ctx {
    pub plane_state: Option<usize>,
    pub stream: Option<usize>,
    pub plane_res: plane_resource,
    pub stream_res: stream_resource,
    pub link_res: link_resource,
    pub clock_source: Option<clock_source_ref>,
    pub pll_settings: pll_settings,
    pub link_config: link_config,
    pub pipe_idx: u8,
    pub top_pipe: Option<usize>,
    pub bottom_pipe: Option<usize>,
    pub next_odm_pipe: Option<usize>,
    pub prev_odm_pipe: Option<usize>,
    /// `dsc_padding_params.dsc_hactive_padding`.
    pub dsc_hactive_padding: u8,
    pub dlg_regs: crate::display_mode_structs::_vcs_dpi_display_dlg_regs_st,
    pub ttu_regs: crate::display_mode_structs::_vcs_dpi_display_ttu_regs_st,
    pub rq_regs: crate::display_mode_structs::_vcs_dpi_display_rq_regs_st,
    pub pipe_dlg_param: crate::display_mode_structs::_vcs_dpi_display_pipe_dest_params_st,
    pub det_buffer_size_kb: i32,
    pub unbounded_req: bool,
    /// `update_flags.raw` (union pipe_update_flags).
    pub update_flags: u32,
}

/// `union pipe_update_flags` (core_types.h) bits.
pub const PIPE_UPDATE_ENABLE: u32 = 1 << 0;
pub const PIPE_UPDATE_DISABLE: u32 = 1 << 1;
pub const PIPE_UPDATE_ODM: u32 = 1 << 2;
pub const PIPE_UPDATE_GLOBAL_SYNC: u32 = 1 << 3;
pub const PIPE_UPDATE_OPP_CHANGED: u32 = 1 << 4;
pub const PIPE_UPDATE_TG_CHANGED: u32 = 1 << 5;
pub const PIPE_UPDATE_MPCC: u32 = 1 << 6;
pub const PIPE_UPDATE_DPPCLK: u32 = 1 << 7;
pub const PIPE_UPDATE_HUBP_INTERDEPENDENT: u32 = 1 << 8;
pub const PIPE_UPDATE_HUBP_RQ_DLG_TTU: u32 = 1 << 9;
pub const PIPE_UPDATE_GAMUT_REMAP: u32 = 1 << 10;
pub const PIPE_UPDATE_SCALER: u32 = 1 << 11;
pub const PIPE_UPDATE_VIEWPORT: u32 = 1 << 12;
pub const PIPE_UPDATE_PLANE_CHANGED: u32 = 1 << 13;
pub const PIPE_UPDATE_DET_SIZE: u32 = 1 << 14;
pub const PIPE_UPDATE_UNBOUNDED_REQ: u32 = 1 << 15;
pub const PIPE_UPDATE_TEST_PATTERN_CHANGED: u32 = 1 << 16;

/// `struct resource_context` (core_types.h), the members reached.
#[derive(Clone, Copy, Debug, Default)]
pub struct resource_context {
    pub pipe_ctx: [pipe_ctx; MAX_PIPES],
    pub is_stream_enc_acquired: [bool; MAX_PIPES * 2],
    pub is_audio_acquired: [bool; MAX_PIPES],
    pub clock_source_ref_count: [u8; MAX_CLOCK_SOURCES],
    pub dp_clock_source_ref_count: u8,
    pub link_enc_cfg_ctx: link_enc_cfg_context,
}

/// `struct dc_stream_status` (dc_stream.h), the members reached; the plane
/// states are indices into dc's arena.
#[derive(Clone, Copy, Debug, Default)]
pub struct dc_stream_status {
    pub primary_otg_inst: i32,
    pub stream_enc_inst: i32,
    pub plane_count: i32,
    pub audio_inst: i32,
    pub plane_states: [Option<usize>; MAX_SURFACES],
    pub is_abm_supported: bool,
    /// `mall_stream_config.subvp_limit_cursor_size`.
    pub subvp_limit_cursor_size: bool,
}

/// `struct cstate_pstate_watermarks_st` (inc/hw/mem_input.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct cstate_pstate_watermarks_st {
    pub cstate_exit_ns: u32,
    pub cstate_exit_z8_ns: u32,
    pub cstate_enter_plus_exit_z8_ns: u32,
    pub cstate_enter_plus_exit_ns: u32,
    pub pstate_change_ns: u32,
    pub fclk_pstate_change_ns: u32,
}

/// `struct dcn_watermarks` (inc/hw/mem_input.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct dcn_watermarks {
    pub pte_meta_urgent_ns: u32,
    pub urgent_ns: u32,
    pub frac_urg_bw_nom: u32,
    pub frac_urg_bw_flip: u32,
    pub urgent_latency_ns: u32,
    pub cstate_pstate: cstate_pstate_watermarks_st,
    pub usr_retraining_ns: u32,
}

/// `union dcn_watermark_set`, its legacy view (DCN 2.x).
#[derive(Clone, Copy, Debug, Default)]
pub struct dcn_watermark_set {
    pub a: dcn_watermarks,
    pub b: dcn_watermarks,
    pub c: dcn_watermarks,
    pub d: dcn_watermarks,
}

/// `struct dcn_bw_output` (core_types.h), the DCN 2.x members; writeback
/// arbitration joins with writeback.
#[derive(Clone, Copy, Debug, Default)]
pub struct dcn_bw_output {
    pub clk: crate::clk_mgr::dc_clocks,
    pub watermarks: dcn_watermark_set,
    pub compbuf_size_kb: i32,
}

/// `struct bw_context` (core_types.h), its `bw.dcn` view.
#[derive(Clone, Copy, Debug, Default)]
pub struct bw_context {
    pub dcn: dcn_bw_output,
}

/// `enum dc_power_source_type` (dc_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum dc_power_source_type {
    #[default]
    DC_POWER_SOURCE_AC,
    DC_POWER_SOURCE_DC,
}

/// `struct dc_state` (core_types.h). `bw_ctx.dml` is the state's own copy
/// of the DML instance; `clk_mgr` says whether it refers to dc's.
#[derive(Clone, Default)]
pub struct dc_state {
    pub dml: Option<crate::dml_structs::DmlBox>,
    pub has_clk_mgr: bool,
    pub streams: [Option<usize>; MAX_PIPES],
    pub stream_status: [dc_stream_status; MAX_PIPES],
    pub stream_count: u32,
    pub res_ctx: resource_context,
    pub bw_ctx: bw_context,
    pub power_source: dc_power_source_type,
    pub stream_mask: u32,
    pub refcount: u32,
}

/// `clear_enc_assignments` (dc_link_enc_cfg.c). A stream reference would be
/// released here; the port holds none yet.
fn clear_enc_assignments(dc: &dc, state: &mut dc_state) {
    for a in state.res_ctx.link_enc_cfg_ctx.link_enc_assignments.iter_mut() {
        a.valid = false;
        a.eng_id = ENGINE_ID_UNKNOWN;
        a.stream = None;
    }

    let Some(pool) = dc.res_pool.as_deref() else { return };
    let n = pool.res_cap.map_or(0, |c| c.num_dig_link_enc as usize);
    for i in 0..n.min(MAX_DIG_LINK_ENCODERS) {
        state.res_ctx.link_enc_cfg_ctx.link_enc_avail[i] =
            if pool.link_encoders[i].is_some() { i as u32 } else { ENGINE_ID_UNKNOWN };
    }
}

/// `link_enc_cfg_init` (dc_link_enc_cfg.c).
fn link_enc_cfg_init(dc: &dc, state: &mut dc_state) {
    clear_enc_assignments(dc, state);
    state.res_ctx.link_enc_cfg_ctx.mode = link_enc_cfg_mode::LINK_ENC_CFG_STEADY;
}

/// `init_state` (dc_state.c): every context starts from a copy of dc's DML
/// instance.
fn init_state(dc: &dc, state: &mut dc_state) {
    state.dml = dc.dml.clone();
}

/// `dc_state_construct` (dc_state.c).
fn dc_state_construct(dc: &dc, state: &mut dc_state) {
    state.has_clk_mgr = dc.clk_mgr.is_some();

    // Initialise DIG link encoder resource tracking variables.
    if dc.res_pool.is_some() {
        link_enc_cfg_init(dc, state);
    }
}

/// `dc_state_create` (dc_state.c). `debug.using_dml2` is false on DCN 2.1,
/// so no dml2 context is made.
pub fn dc_state_create(dc: &dc, power_source: Option<dc_power_source_type>) -> Box<dc_state> {
    let mut state = Box::new(dc_state::default());

    init_state(dc, &mut state);
    dc_state_construct(dc, &mut state);
    state.power_source = power_source.unwrap_or(dc_power_source_type::DC_POWER_SOURCE_AC);

    state.refcount = 1;
    state
}

/// `dc_state_get_stream_subvp_type` (dc_state.c): the MALL type of `stream`
/// in `state`, SUBVP_NONE for one the state does not hold. The state holds
/// no stream yet.
pub fn dc_state_get_stream_subvp_type(state: &dc_state, stream: Option<usize>) -> u32 {
    let _ = (state.stream_count, stream);
    crate::regs::SUBVP_NONE
}

/// `dc_state_get_pipe_subvp_type` (dc_state.c).
pub fn dc_state_get_pipe_subvp_type(state: &dc_state, pipe_ctx: &pipe_ctx) -> u32 {
    dc_state_get_stream_subvp_type(state, pipe_ctx.stream)
}

/// `dc_state_copy_internal` (dc_state.c): the neighbouring pipes are
/// indices, so the copy needs no relinking; the streams and planes it
/// holds gain a reference.
fn dc_state_copy_internal(dc: &mut dc, src_state: &dc_state) -> dc_state {
    let dst_state = src_state.clone();

    // retain streams and planes
    for i in 0..dst_state.stream_count as usize {
        crate::dc_stream::dc_stream_retain(dc, dst_state.streams[i].unwrap());
        for j in 0..dst_state.stream_status[i].plane_count as usize {
            crate::dc_stream::dc_plane_state_retain(dc, dst_state.stream_status[i].plane_states[j].unwrap());
        }
    }

    dst_state
}

/// `dc_state_create_copy` (dc_state.c). DCN 2.1 has no dml2 context.
pub fn dc_state_create_copy(dc: &mut dc, src_state: &dc_state) -> Box<dc_state> {
    let mut new_state = Box::new(dc_state_copy_internal(dc, src_state));

    new_state.refcount = 1;

    new_state
}

/// `dc_state_create_current_copy` (dc_state.c).
pub fn dc_state_create_current_copy(dc: &mut dc) -> Box<dc_state> {
    let current = dc.current_state.take().unwrap();
    let copy = dc_state_create_copy(dc, &current);
    dc.current_state = Some(current);
    copy
}

/// `dc_state_get_stream_status` (dc_state.c): the slot of `stream`.
pub fn dc_state_get_stream_status(state: &dc_state, stream: usize) -> Option<usize> {
    (0..state.stream_count as usize).find(|&i| state.streams[i] == Some(stream))
}

/// `dc_state_add_stream` (dc_state.c).
pub fn dc_state_add_stream(dc: &mut dc, state: &mut dc_state, stream: usize) -> crate::dc_types::dc_status {
    use crate::dc_types::dc_status;

    if state.stream_count >= dc.res_pool.as_deref().unwrap().timing_generator_count {
        crate::linux::Line::new().s("Max streams reached, can't add stream!").say();
        return dc_status::DC_ERROR_UNEXPECTED;
    }

    state.streams[state.stream_count as usize] = Some(stream);
    crate::dc_stream::dc_stream_retain(dc, stream);
    state.stream_count += 1;

    let res = crate::dc_resource::resource_add_otg_master_for_stream_output(dc, state, stream);
    if res != dc_status::DC_OK {
        crate::linux::Line::new().s("Adding stream to context failed with err ").hex(res as i32 as u32).say();
    }

    res
}

/// `add_plane_to_opp_head_pipes` (dc_resource.c).
fn add_plane_to_opp_head_pipes(otg_master_pipe: usize, plane_state: usize, context: &mut dc_state) -> bool {
    let mut opp_head_pipe = Some(otg_master_pipe);

    while let Some(p) = opp_head_pipe {
        let pipe = &mut context.res_ctx.pipe_ctx[p];
        if pipe.plane_state.is_some() {
            return false;
        }
        pipe.plane_state = Some(plane_state);
        opp_head_pipe = pipe.next_odm_pipe;
    }

    true
}

/// `update_pipe_params_after_mpc_slice_count_change` (dc_resource.c).
fn update_pipe_params_after_mpc_slice_count_change(dc: &mut dc, plane: usize, context: &mut dc_state) -> bool {
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    let mut result = true;

    let mut i = 0;
    while i < pipe_count && result {
        if context.res_ctx.pipe_ctx[i].plane_state == Some(plane) {
            result = crate::dc_resource::resource_build_scaling_params(dc, &mut context.res_ctx, i);
        }
        i += 1;
    }
    result
}

/// `resource_append_dpp_pipes_for_plane_composition` (dc_resource.c).
/// Adding a second plane to a stream (MPO) acquires secondary DPP pipes;
/// that half is named, not ported.
pub fn resource_append_dpp_pipes_for_plane_composition(dc: &mut dc, new_ctx: &mut dc_state, otg_master_pipe: usize,
                                                       plane_state: usize) -> bool {
    let mut success = if new_ctx.res_ctx.pipe_ctx[otg_master_pipe].plane_state.is_none() {
        add_plane_to_opp_head_pipes(otg_master_pipe, plane_state, new_ctx)
    } else {
        crate::linux::Line::new().s("acquire_secondary_dpp_pipes_and_add_plane not ported").say();
        false
    };
    if success {
        // when appending a plane mpc slice count changes from 0 to 1
        success = update_pipe_params_after_mpc_slice_count_change(dc, plane_state, new_ctx);
        if !success {
            crate::linux::Line::new().s("resource_remove_dpp_pipes_for_plane_composition not ported").say();
        }
    }

    success
}

/// `dc_state_add_plane` (dc_state.c). Freeing pipes by undoing MPC combine
/// or ODM slices to fit a further plane is named, not ported.
pub fn dc_state_add_plane(dc: &mut dc, stream: usize, plane_state: usize, state: &mut dc_state) -> bool {
    let stream_status = dc_state_get_stream_status(state, stream);
    let otg_master_pipe = crate::dc_resource::resource_get_otg_master_for_stream(&state.res_ctx, stream);
    let Some(ss) = stream_status else {
        crate::linux::Line::new().s("Existing stream not found; failed to attach surface!").say();
        return false;
    };
    if state.stream_status[ss].plane_count as usize == MAX_SURFACES {
        crate::linux::Line::new().s("Surface: can not attach plane_state! Maximum is: ").dec(MAX_SURFACES as u64).say();
        return false;
    }
    let Some(otg_master_pipe) = otg_master_pipe else { return false };

    let mut added = resource_append_dpp_pipes_for_plane_composition(dc, state, otg_master_pipe, plane_state);

    if !added {
        crate::linux::Line::new().s("remove_mpc_combine_for_stream, resource_update_pipes_for_stream_with_slice_count not ported").say();
        added = false;
    }

    if added {
        let st = &mut state.stream_status[ss];
        st.plane_states[st.plane_count as usize] = Some(plane_state);
        st.plane_count += 1;
        crate::dc_stream::dc_plane_state_retain(dc, plane_state);
    }

    added
}

/// `dc_state_retain` (dc_state.c).
pub fn dc_state_retain(state: &mut dc_state) {
    state.refcount += 1;
}

/// `dc_state_destruct` (dc_state.c): drop the references the state holds.
/// DCN 2.1 tracks no phantoms.
pub fn dc_state_destruct(dc: &mut dc, state: &mut dc_state) {
    for i in 0..state.stream_count as usize {
        for j in 0..state.stream_status[i].plane_count as usize {
            crate::dc_stream::dc_plane_state_release(dc, state.stream_status[i].plane_states[j].unwrap());
        }

        state.stream_status[i].plane_count = 0;
        crate::dc_stream::dc_stream_release(dc, state.streams[i].unwrap());
        state.streams[i] = None;
    }
    state.stream_count = 0;

    state.stream_mask = 0;
    state.res_ctx = resource_context::default();
    state.has_clk_mgr = false;
    state.bw_ctx = bw_context::default();
}

/// `dc_state_release` (dc_state.c): the last reference destructs and frees
/// the state; any other is dropped with the Box Linux keeps pointing at.
pub fn dc_state_release(dc: &mut dc, mut state: Box<dc_state>) {
    state.refcount -= 1;
    if state.refcount == 0 {
        dc_state_destruct(dc, &mut state);
    } else {
        crate::linux::Line::new().s("dc_state_release: a state with further references").say();
    }
}

/// `dc_state_set_stream_subvp_cursor_limit` (dc_state.c).
pub fn dc_state_set_stream_subvp_cursor_limit(stream: usize, state: &mut dc_state, limit: bool) {
    if let Some(i) = dc_state_get_stream_status(state, stream) {
        state.stream_status[i].subvp_limit_cursor_size = limit;
    }
}

/// `dc_state_rem_all_planes_for_stream` (dc_state.c): taking the planes of
/// a stream back off its pipes is a later stage.
pub fn dc_state_rem_all_planes_for_stream(_dc: &mut dc, _stream: usize, _state: &mut dc_state) -> bool {
    crate::linux::Line::new().s("dc_state_rem_all_planes_for_stream not ported").say();
    false
}
