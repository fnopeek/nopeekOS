//! display/dc/inc/resource.h (Linux 6.18.55): shared resource types used by
//! every DCN resource pool. Only `resource_caps` for now — the per-ASIC count
//! table a pool points `res_pool->res_cap` at (e.g. `res_cap_rn` for Renoir).
#![allow(dead_code)]

use crate::dccg::dcn_dccg;
use crate::dcn10_ipp::dcn10_ipp;
use crate::dcn21_hubp::dcn21_hubp;
use crate::dce_clock_source::dce110_clk_src;
use crate::dc_types::MAX_PIPES;
use alloc::boxed::Box;

/// `#define MAX_CLOCK_SOURCES` (core_types.h).
pub const MAX_CLOCK_SOURCES: usize = 7;

/// `struct resource_caps` (resource.h): how many of each block the ASIC has.
#[derive(Default, Clone, Copy)]
pub struct resource_caps {
    pub num_timing_generator: i32,
    pub num_opp: i32,
    pub num_dpp: i32,
    pub num_video_plane: i32,
    pub num_audio: i32,
    pub num_stream_encoder: i32,
    pub num_pll: i32,
    pub num_dwb: i32,
    pub num_ddc: i32,
    pub num_vmid: i32,
    pub num_dsc: i32,
    /// Total number of DIGs (digital encoders) in DIO.
    pub num_dig_link_enc: u32,
    /// Total number of USB4 DPIA (DisplayPort Input Adapters).
    pub num_usb4_dpia: u32,
    pub num_hpo_dp_stream_encoder: i32,
    pub num_hpo_dp_link_encoder: i32,
    pub num_mpc_3dlut: i32,
}

/// `#define NO_UNDERLAY_PIPE` (core_types.h): assigned to the unsigned
/// `underlay_pipe_index`, so Linux' `-1` reads back as `u32::MAX`.
pub const NO_UNDERLAY_PIPE: u32 = u32::MAX;

/// `struct resource_pool` (core_types.h): everything a DCN pool owns. Ported
/// grow-as-reached — only the fields `dcn21_resource_construct` has reached so
/// far; the block arrays (hubps/dpps/opps/stream_enc/…) join as each IP-block
/// constructor lands.
#[derive(Default)]
pub struct resource_pool {
    pub pipe_count: u32,
    pub underlay_pipe_index: u32,
    pub stream_enc_count: u32,
    pub audio_count: u32,
    pub clk_src_count: u32,
    /// `clock_sources[MAX_CLOCK_SOURCES]`: every Renoir clock source is a
    /// `dce110_clk_src`, so the pool holds the concrete type (Linux stores the
    /// `clock_source` base pointer).
    pub clock_sources: [Option<Box<dce110_clk_src>>; MAX_CLOCK_SOURCES],
    pub dp_clock_source: Option<Box<dce110_clk_src>>,
    /// `dccg`: the display clock generator (Renoir → `dcn_dccg`).
    pub dccg: Option<Box<dcn_dccg>>,
    /// `hubps[MAX_PIPES]`: the hub pixel fetchers, one per active pipe.
    pub hubps: [Option<Box<dcn21_hubp>>; MAX_PIPES],
    /// `ipps[MAX_PIPES]`: the input pixel processors, one per active pipe.
    pub ipps: [Option<Box<dcn10_ipp>>; MAX_PIPES],
    /// `res_cap`: the per-ASIC count table (Renoir → `RES_CAP_RN`).
    pub res_cap: Option<&'static resource_caps>,
}
