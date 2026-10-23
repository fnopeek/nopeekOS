//! display/dc/inc/resource.h (Linux 6.18.55): shared resource types used by
//! every DCN resource pool. Only `resource_caps` for now — the per-ASIC count
//! table a pool points `res_pool->res_cap` at (e.g. `res_cap_rn` for Renoir).
#![allow(dead_code)]

use crate::dccg::dcn_dccg;
use crate::dcn10_ipp::dcn10_ipp;
use crate::dcn20_dpp::dcn20_dpp;
use crate::dcn20_dsc::dcn20_dsc;
use crate::dcn20_dwb::{dcn20_dwbc, MAX_DWB_PIPES};
use crate::dcn20_hubbub::dcn20_hubbub;
use crate::dcn20_mmhubbub::dcn20_mmhubbub;
use crate::dcn20_mpc::dcn20_mpc;
use crate::dcn20_opp::dcn20_opp;
use crate::dcn20_optc::optc;
use crate::dcn20_stream_encoder::{dcn10_stream_encoder, stream_encoder};
use crate::dcn21_hubp::dcn21_hubp;
use crate::dce_audio::dce_audio;
use crate::dce_aux::aux_engine_dce110;
use crate::dce_clock_source::dce110_clk_src;
use crate::dce_i2c_hw::dce_i2c_hw;
use crate::amdgpu_device::amdgpu_device;
use crate::dc::{dc, dc_init_data};
use crate::dc_types::{dce_version, hw_asic_id, MAX_PIPES};
use crate::dcn21_resource::dcn21_create_resource_pool;
use alloc::boxed::Box;

/// A `stream_encoder *` slot of the pool: a real DIG encoder (dcn10) or a
/// virtual one (a bare `stream_encoder`). Linux stores the shared base pointer;
/// the port keeps the concrete object and tags it.
pub enum stream_enc_slot {
    Dcn10(Box<dcn10_stream_encoder>),
    Virtual(Box<stream_encoder>),
}

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

/// `struct resource_straps` (resource.h): the board straps read from HW.
/// `read_dce_straps` fills only `dc_pinstraps_audio` on dcn21; the rest are 0.
#[derive(Default, Clone, Copy)]
pub struct resource_straps {
    pub hdmi_disable: u32,
    pub dc_pinstraps_audio: u32,
    pub audio_stream_number: u32,
}

/// `struct audio_support` (core_types.h): which audio outputs the board offers.
#[derive(Default, Clone, Copy)]
pub struct audio_support {
    pub dp_audio: bool,
    pub hdmi_audio_on_dongle: bool,
    pub hdmi_audio_native: bool,
}

/// `update_num_audio` (core/dc_resource.c): derive the audio-output support and
/// the usable stream count from the straps. `audio_stream_number == 0` means
/// multi-stream, so `num_audio` is left at the cap.
pub fn update_num_audio(straps: &resource_straps, num_audio: &mut u32, aud_support: &mut audio_support) {
    aud_support.dp_audio = true;
    aud_support.hdmi_audio_native = false;
    aud_support.hdmi_audio_on_dongle = false;

    if straps.hdmi_disable == 0 && straps.dc_pinstraps_audio & 0x2 != 0 {
        aud_support.hdmi_audio_on_dongle = true;
        aud_support.hdmi_audio_native = true;
    }

    match straps.audio_stream_number {
        0 => {}          // multi streams supported
        1 => *num_audio = 1, // multi streams not supported
        _ => {}          // Linux logs "unexpected audio fuse"
    }
}

/// `struct resource_pool` (core_types.h): everything a DCN pool owns. Ported
/// grow-as-reached — only the fields `dcn21_resource_construct` has reached so
/// far; the block arrays (hubps/dpps/opps/stream_enc/…) join as each IP-block
/// constructor lands.
#[derive(Default)]
pub struct resource_pool {
    pub pipe_count: u32,
    pub mpcc_count: u32,
    pub timing_generator_count: u32,
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
    /// `dpps[MAX_PIPES]`: the display pipe-and-plane processors, one per pipe.
    pub dpps: [Option<Box<dcn20_dpp>>; MAX_PIPES],
    /// `opps[MAX_PIPES]`: the output pixel processors, one per pipe.
    pub opps: [Option<Box<dcn20_opp>>; MAX_PIPES],
    /// `timing_generators[MAX_PIPES]`: the OTGs, one per pipe.
    pub timing_generators: [Option<Box<optc>>; MAX_PIPES],
    /// `mpc`: the multiple pipe/plane combiner (one per device).
    pub mpc: Option<Box<dcn20_mpc>>,
    /// `hubbub`: the hub bottleneck arbiter (one per device).
    pub hubbub: Option<Box<dcn20_hubbub>>,
    /// `dscs[MAX_PIPES]`: the display stream compressors (`num_dsc` used).
    pub dscs: [Option<Box<dcn20_dsc>>; MAX_PIPES],
    /// `dwbc[MAX_DWB_PIPES]`: the display writeback controllers (`num_dwb`).
    pub dwbc: [Option<Box<dcn20_dwbc>>; MAX_DWB_PIPES],
    /// `mcif_wb[MAX_DWB_PIPES]`: the writeback hubs, one per DWBC.
    pub mcif_wb: [Option<Box<dcn20_mmhubbub>>; MAX_DWB_PIPES],
    /// `engines[MAX_PIPES]`: the DP AUX engines (`num_ddc` used).
    pub engines: [Option<Box<aux_engine_dce110>>; MAX_PIPES],
    /// `hw_i2cs[MAX_PIPES]`: the hardware I2C engines (`num_ddc` used).
    pub hw_i2cs: [Option<Box<dce_i2c_hw>>; MAX_PIPES],
    /// `stream_enc[MAX_PIPES]`: the stream encoders (DIG + virtual).
    pub stream_enc: [Option<stream_enc_slot>; MAX_PIPES],
    /// `audios[MAX_PIPES]`: the audio endpoints (`audio_count` valid).
    pub audios: [Option<Box<dce_audio>>; MAX_PIPES],
    /// `audio_support`: which audio outputs the board offers (from the straps).
    pub audio_support: audio_support,
    /// `res_cap`: the per-ASIC count table (Renoir → `RES_CAP_RN`).
    pub res_cap: Option<&'static resource_caps>,
    /// `ref_clocks`: the reference clocks, seeded from the VBIOS crystal by
    /// `dc_create_resource_pool`.
    pub ref_clocks: resource_pool_ref_clocks,
}

/// The anonymous `ref_clocks` member of `struct resource_pool` (core_types.h).
#[derive(Default, Clone, Copy)]
pub struct resource_pool_ref_clocks {
    pub xtalin_clock_inKhz: u32,
    pub dccg_ref_clock_inKhz: u32,
    pub dchub_ref_clock_inKhz: u32,
}

/// `resource_parse_asic_id` (dc_resource.c): FAMILY_RV with a Renoir
/// revision is DCN 2.1.
pub fn resource_parse_asic_id(_asic_id: &hw_asic_id) -> dce_version {
    dce_version::DCN_VERSION_2_1
}

/// `dc_create_resource_pool` (dc_resource.c): the DCN 2.1 pool, then the
/// reference clocks from the VBIOS firmware info.
pub fn dc_create_resource_pool(
    adev: &amdgpu_device,
    dc: &mut dc,
    init_data: &dc_init_data,
) -> Option<Box<resource_pool>> {
    let mut res_pool = dcn21_create_resource_pool(adev, init_data, dc);

    if let Some(pool) = res_pool.as_deref_mut() {
        if dc.ctx.dc_bios.fw_info_valid {
            pool.ref_clocks.xtalin_clock_inKhz = dc.ctx.dc_bios.fw_info.pll_info.crystal_frequency;
            pool.ref_clocks.dccg_ref_clock_inKhz = pool.ref_clocks.xtalin_clock_inKhz;
            pool.ref_clocks.dchub_ref_clock_inKhz = pool.ref_clocks.xtalin_clock_inKhz;
        }
    }

    res_pool
}
