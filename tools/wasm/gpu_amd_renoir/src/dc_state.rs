//! display/dc/core/dc_state.c + core/dc_link_enc_cfg.c (link_enc_cfg_init)
//! (Linux 6.18.55): the DC state `dc_construct` ends with. Grown as reached:
//! the DML copy, the clock manager it refers to and the DIG link encoder
//! assignment context; streams, planes and pipes join with the commit path.
#![allow(dead_code)]

use crate::dc::dc;
use crate::dc_resource::MAX_DIG_LINK_ENCODERS;
use crate::dc_types::{graphics_object_id, MAX_PIPES};
use crate::dcn21_link_encoder::ENGINE_ID_UNKNOWN;
use crate::dml_structs::display_mode_lib;
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

/// `struct resource_context` (core_types.h), the members reached.
#[derive(Clone, Copy, Debug, Default)]
pub struct resource_context {
    pub link_enc_cfg_ctx: link_enc_cfg_context,
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
    pub dml: Option<Box<display_mode_lib>>,
    pub has_clk_mgr: bool,
    pub res_ctx: resource_context,
    pub power_source: dc_power_source_type,
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
