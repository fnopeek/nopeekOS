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
    /// `abm`: the adaptive backlight (DMUB-run on Renoir with a DMCUB).
    pub abm: Option<Box<crate::dmub_abm::dce_abm>>,
    /// `irqs`: the interrupt service.
    pub irqs: Option<Box<crate::irq_service::irq_service>>,
    /// `psr`: the DMUB panel self refresh object.
    pub psr: Option<Box<crate::dmub_abm::dmub_psr>>,
    /// `pp_smu`: the power-play SMU interface (None if the DM offers no
    /// Renoir table — Linux zeroes it then).
    pub pp_smu: Option<crate::dm_pp_smu::pp_smu_funcs>,
    /// `link_encoders[MAX_DIG_LINK_ENCODERS]`: per DIG, the index of the link
    /// whose encoder it is (Linux stores the encoder pointer the link owns).
    pub link_encoders: [Option<u32>; MAX_DIG_LINK_ENCODERS],
    pub dig_link_enc_count: u32,
    pub usb4_dpia_count: u32,
}

/// `MAX_DIG_LINK_ENCODERS` (hw_shared.h).
pub const MAX_DIG_LINK_ENCODERS: usize = 7;

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
        if dc.ctx.dc_bios.base.fw_info_valid {
            pool.ref_clocks.xtalin_clock_inKhz = dc.ctx.dc_bios.base.fw_info.pll_info.crystal_frequency;
            pool.ref_clocks.dccg_ref_clock_inKhz = pool.ref_clocks.xtalin_clock_inKhz;
            pool.ref_clocks.dchub_ref_clock_inKhz = pool.ref_clocks.xtalin_clock_inKhz;
        }
    }

    res_pool
}

impl stream_enc_slot {
    /// The shared `stream_encoder` base.
    pub fn base(&self) -> &stream_encoder {
        match self {
            stream_enc_slot::Dcn10(e) => &e.base,
            stream_enc_slot::Virtual(e) => e,
        }
    }
}

// The commit half of dc_resource.c: mapping a stream onto the pool's
// blocks and building a pipe's scaler data. A stream or plane is an index
// into dc's arenas, a pipe an index into its context's `pipe_ctx`.

use crate::dc_hw_types::{rect, scaler_data, scaling_taps};
use crate::dc_state::{clock_source_ref, dc_state, pipe_ctx, resource_context};
use crate::dc_stream::{bit_depth_reduction_params, dc_plane_state, dc_stream_state};
use crate::dc_types::dc_status;
use crate::fixpt31_32::*;
use crate::link_detection::{dc_is_audio_capable_signal, dc_is_dp_signal, dc_is_embedded_signal, dc_is_hdmi_signal,
    dc_is_virtual_signal};
use crate::regs::*;

/// `FREE_PIPE_INDEX_NOT_FOUND` (resource.h).
const FREE_PIPE_INDEX_NOT_FOUND: i32 = -1;

fn not_ported(name: &str) {
    crate::linux::Line::new().s("dc_resource: ").s(name).s(" not ported").say();
}

/// `enum pipe_type` (resource.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum pipe_type {
    OTG_MASTER,
    OPP_HEAD,
    DPP_PIPE,
    FREE_PIPE,
}

/// `resource_is_pipe_type`.
pub fn resource_is_pipe_type(pipe_ctx: &pipe_ctx, r#type: pipe_type) -> bool {
    match r#type {
        pipe_type::OTG_MASTER => {
            pipe_ctx.prev_odm_pipe.is_none() && pipe_ctx.top_pipe.is_none() && pipe_ctx.stream.is_some()
        }
        pipe_type::OPP_HEAD => pipe_ctx.top_pipe.is_none() && pipe_ctx.stream.is_some(),
        pipe_type::DPP_PIPE => pipe_ctx.plane_state.is_some() && pipe_ctx.stream.is_some(),
        pipe_type::FREE_PIPE => pipe_ctx.plane_state.is_none() && pipe_ctx.stream.is_none(),
    }
}

/// `resource_get_otg_master_for_stream`.
pub fn resource_get_otg_master_for_stream(res_ctx: &resource_context, stream: usize) -> Option<usize> {
    (0..MAX_PIPES).find(|&i| {
        res_ctx.pipe_ctx[i].stream == Some(stream) && resource_is_pipe_type(&res_ctx.pipe_ctx[i], pipe_type::OTG_MASTER)
    })
}

/// `recource_find_free_pipe_not_used_in_cur_res_ctx`.
fn recource_find_free_pipe_not_used_in_cur_res_ctx(cur_res_ctx: &resource_context, new_res_ctx: &resource_context,
                                                   pool: &resource_pool) -> i32 {
    for i in 0..pool.pipe_count as usize {
        if resource_is_pipe_type(&cur_res_ctx.pipe_ctx[i], pipe_type::FREE_PIPE)
            && resource_is_pipe_type(&new_res_ctx.pipe_ctx[i], pipe_type::FREE_PIPE)
        {
            return i as i32;
        }
    }
    FREE_PIPE_INDEX_NOT_FOUND
}

/// `recource_find_free_pipe_used_as_otg_master_in_cur_res_ctx`.
fn recource_find_free_pipe_used_as_otg_master_in_cur_res_ctx(cur_res_ctx: &resource_context,
                                                             new_res_ctx: &resource_context,
                                                             pool: &resource_pool) -> i32 {
    for i in 0..pool.pipe_count as usize {
        if resource_is_pipe_type(&cur_res_ctx.pipe_ctx[i], pipe_type::OTG_MASTER)
            && resource_is_pipe_type(&new_res_ctx.pipe_ctx[i], pipe_type::FREE_PIPE)
        {
            return i as i32;
        }
    }
    FREE_PIPE_INDEX_NOT_FOUND
}

/// `resource_find_free_pipe_used_as_cur_sec_dpp`.
pub fn resource_find_free_pipe_used_as_cur_sec_dpp(cur_res_ctx: &resource_context, new_res_ctx: &resource_context,
                                                   pool: &resource_pool) -> i32 {
    for i in 0..pool.pipe_count as usize {
        let cur_pipe = &cur_res_ctx.pipe_ctx[i];
        if resource_is_pipe_type(cur_pipe, pipe_type::DPP_PIPE)
            && !resource_is_pipe_type(cur_pipe, pipe_type::OPP_HEAD)
            && resource_is_pipe_type(&new_res_ctx.pipe_ctx[i], pipe_type::FREE_PIPE)
        {
            return i as i32;
        }
    }
    FREE_PIPE_INDEX_NOT_FOUND
}

/// `resource_find_any_free_pipe`.
pub fn resource_find_any_free_pipe(new_res_ctx: &resource_context, pool: &resource_pool) -> i32 {
    for i in 0..pool.pipe_count as usize {
        if resource_is_pipe_type(&new_res_ctx.pipe_ctx[i], pipe_type::FREE_PIPE) {
            return i as i32;
        }
    }
    FREE_PIPE_INDEX_NOT_FOUND
}

/// `acquire_first_split_pipe`.
fn acquire_first_split_pipe(dc: &mut dc, res_ctx: &mut resource_context, stream: usize) -> i32 {
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;
    for i in 0..pipe_count {
        let split_pipe = res_ctx.pipe_ctx[i];

        let Some(top) = split_pipe.top_pipe else { continue };
        if res_ctx.pipe_ctx[top].plane_state != split_pipe.plane_state {
            continue;
        }
        res_ctx.pipe_ctx[top].bottom_pipe = split_pipe.bottom_pipe;
        if let Some(bottom) = split_pipe.bottom_pipe {
            res_ctx.pipe_ctx[bottom].top_pipe = split_pipe.top_pipe;
        }

        if res_ctx.pipe_ctx[top].plane_state.is_some() {
            resource_build_scaling_params(dc, res_ctx, top);
        }

        let pool = dc.res_pool.as_deref().unwrap();
        let p = &mut res_ctx.pipe_ctx[i];
        *p = pipe_ctx::default();
        p.stream_res.tg = pool.timing_generators[i].as_ref().map(|_| i);
        p.plane_res.hubp = pool.hubps[i].as_ref().map(|_| i);
        p.plane_res.ipp = pool.ipps[i].as_ref().map(|_| i);
        p.plane_res.dpp = pool.dpps[i].as_ref().map(|_| i);
        p.stream_res.opp = pool.opps[i].as_ref().map(|_| i);
        p.plane_res.mpcc_inst = pool.dpps[i].as_deref().unwrap().base.inst as u8;
        p.pipe_idx = i as u8;

        p.stream = Some(stream);
        return i as i32;
    }
    FREE_PIPE_INDEX_NOT_FOUND
}

/// `acquire_otg_master_pipe_for_stream`: the current context's pipes are
/// read from `dc.current_state`.
fn acquire_otg_master_pipe_for_stream(dc: &mut dc, new_ctx: &mut dc_state, stream: usize) -> bool {
    let cur_res_ctx = &dc.current_state.as_deref().unwrap().res_ctx;
    let pool = dc.res_pool.as_deref().unwrap();

    // Upper level code is responsible to optimize unnecessary addition and
    // removal for unchanged streams. So unchanged stream will keep the same
    // OTG master instance allocated. When current stream is removed and a
    // new stream is added, we want to reuse the OTG instance made available
    // by the removed stream first. If not found, we try to avoid of using
    // any free pipes already used in current context as this could tear
    // down exiting ODM/MPC/MPO configuration unnecessarily.

    // Try to acquire the same OTG master already in use. This is not
    // optimal because resetting an enabled OTG master pipe for a new stream
    // requires an extra frame of wait. However there are test automation
    // and eDP assumptions that rely on reusing the same OTG master pipe
    // during mode change. We have to keep this logic as is for now.
    let mut pipe_idx = recource_find_free_pipe_used_as_otg_master_in_cur_res_ctx(cur_res_ctx, &new_ctx.res_ctx, pool);
    // Try to acquire a pipe not used in current resource context to avoid
    // pipe swapping.
    if pipe_idx == FREE_PIPE_INDEX_NOT_FOUND {
        pipe_idx = recource_find_free_pipe_not_used_in_cur_res_ctx(cur_res_ctx, &new_ctx.res_ctx, pool);
    }
    // If pipe swapping is unavoidable, try to acquire pipe used as
    // secondary DPP pipe in current state as we prioritize to support more
    // streams over supporting MPO planes.
    if pipe_idx == FREE_PIPE_INDEX_NOT_FOUND {
        pipe_idx = resource_find_free_pipe_used_as_cur_sec_dpp(cur_res_ctx, &new_ctx.res_ctx, pool);
    }
    if pipe_idx == FREE_PIPE_INDEX_NOT_FOUND {
        pipe_idx = resource_find_any_free_pipe(&new_ctx.res_ctx, pool);
    }
    if pipe_idx != FREE_PIPE_INDEX_NOT_FOUND {
        let i = pipe_idx as usize;
        let p = &mut new_ctx.res_ctx.pipe_ctx[i];
        *p = pipe_ctx::default();
        p.pipe_idx = i as u8;
        p.stream_res.tg = pool.timing_generators[i].as_ref().map(|_| i);
        p.plane_res.hubp = pool.hubps[i].as_ref().map(|_| i);
        p.plane_res.ipp = pool.ipps[i].as_ref().map(|_| i);
        p.plane_res.dpp = pool.dpps[i].as_ref().map(|_| i);
        p.stream_res.opp = pool.opps[i].as_ref().map(|_| i);
        if let Some(dpp) = pool.dpps[i].as_deref() {
            p.plane_res.mpcc_inst = dpp.base.inst as u8;
        }

        if i >= pool.timing_generator_count as usize && pool.timing_generator_count != 0 {
            let tg_inst = pool.timing_generator_count as usize - 1;

            p.stream_res.tg = pool.timing_generators[tg_inst].as_ref().map(|_| tg_inst);
            p.stream_res.opp = pool.opps[tg_inst].as_ref().map(|_| tg_inst);
        }

        p.stream = Some(stream);
    } else {
        pipe_idx = acquire_first_split_pipe(dc, &mut new_ctx.res_ctx, stream);
    }

    pipe_idx != FREE_PIPE_INDEX_NOT_FOUND
}

/// `get_norm_pix_clk`.
fn get_norm_pix_clk(timing: &crate::dc_hw_types::dc_crtc_timing) -> u32 {
    let mut pix_clk = timing.pix_clk_100hz;
    let mut normalized_pix_clk = pix_clk;

    if timing.pixel_encoding == PIXEL_ENCODING_YCBCR420 {
        pix_clk /= 2;
    }
    if timing.pixel_encoding != PIXEL_ENCODING_YCBCR422 {
        match timing.display_color_depth {
            COLOR_DEPTH_666 | COLOR_DEPTH_888 => normalized_pix_clk = pix_clk,
            COLOR_DEPTH_101010 => normalized_pix_clk = (pix_clk * 30) / 24,
            COLOR_DEPTH_121212 => normalized_pix_clk = (pix_clk * 36) / 24,
            COLOR_DEPTH_141414 => normalized_pix_clk = (pix_clk * 42) / 24,
            COLOR_DEPTH_161616 => normalized_pix_clk = (pix_clk * 48) / 24,
            _ => {}
        }
    }
    normalized_pix_clk
}

/// `calculate_phy_pix_clks`.
fn calculate_phy_pix_clks(stream: &mut dc_stream_state) {
    // update actual pixel clock on all streams
    if dc_is_hdmi_signal(stream.signal) {
        stream.phy_pix_clk = get_norm_pix_clk(&stream.timing) / 10;
    } else {
        stream.phy_pix_clk = stream.timing.pix_clk_100hz / 10;
    }

    if stream.timing.timing_3d_format == TIMING_3D_FORMAT_HW_FRAME_PACKING {
        stream.phy_pix_clk *= 2;
    }
}

/// `mark_seamless_boot_stream`. Seamless boot is a DCN 3.0+ feature of the
/// DM; the VBIOS query and `dc_validate_boot_timing` behind the flag are
/// named, not ported.
fn mark_seamless_boot_stream(dc: &dc, stream: &mut dc_stream_state) {
    if stream.apply_seamless_boot_optimization {
        return;
    }
    if !dc.config.allow_seamless_boot_optimization {
        return;
    }
    not_ported("mark_seamless_boot_stream: is_accelerated_mode, dc_validate_boot_timing");
}

/// `update_stream_engine_usage`: `stream_enc` is the pool slot.
fn update_stream_engine_usage(res_ctx: &mut resource_context, pool: &resource_pool, stream_enc: usize, acquired: bool) {
    for i in 0..pool.stream_enc_count as usize {
        if i == stream_enc {
            res_ctx.is_stream_enc_acquired[i] = acquired;
        }
    }
}

/// `update_audio_usage`.
pub fn update_audio_usage(res_ctx: &mut resource_context, pool: &resource_pool, audio: usize, acquired: bool) {
    for i in 0..pool.audio_count as usize {
        if i == audio {
            res_ctx.is_audio_acquired[i] = acquired;
        }
    }
}

/// `find_first_free_audio`.
fn find_first_free_audio(res_ctx: &resource_context, pool: &resource_pool, id: u32) -> Option<usize> {
    if id == crate::dcn21_link_encoder::ENGINE_ID_UNKNOWN {
        return None;
    }

    let available_audio_count = pool.audio_count as usize;

    for i in 0..available_audio_count {
        if !res_ctx.is_audio_acquired[i] && res_ctx.is_stream_enc_acquired[i] {
            // we have enough audio endpoint, find the matching inst
            if id as usize != i {
                continue;
            }
            return pool.audios[i].as_ref().map(|_| i);
        }
    }

    // use engine id to find free audio
    if (id as usize) < available_audio_count && !res_ctx.is_audio_acquired[id as usize] {
        return pool.audios[id as usize].as_ref().map(|_| id as usize);
    }
    // not found the matching one, first come first serve
    for i in 0..available_audio_count {
        if !res_ctx.is_audio_acquired[i] {
            return pool.audios[i].as_ref().map(|_| i);
        }
    }
    None
}

/// `dcn10_find_first_free_match_stream_enc_for_link` (dcn10_resource.c),
/// the `find_first_free_match_stream_enc_for_link` of DCN 2.1.
pub fn dcn10_find_first_free_match_stream_enc_for_link(res_ctx: &resource_context, pool: &resource_pool,
                                                       link: &crate::link_factory::dc_link) -> Option<usize> {
    use crate::dcn21_link_encoder::ENGINE_ID_VIRTUAL;
    let mut j: i32 = -1;

    for i in 0..pool.stream_enc_count as usize {
        let Some(enc) = pool.stream_enc[i].as_ref() else { continue };
        if res_ctx.is_stream_enc_acquired[i] {
            continue;
        }
        let id = enc.base().id as u32;
        // Store first available for MST second display
        // in daisy chain use case
        if id != ENGINE_ID_VIRTUAL {
            j = i as i32;
        }

        if link.ep_type == LF_DISPLAY_ENDPOINT_PHY && id == link.link_enc.as_ref().unwrap().base().preferred_engine {
            return Some(i);
        }

        if link.ep_type == LF_DISPLAY_ENDPOINT_USB4_DPIA && id == link.dpia_preferred_eng_id {
            return Some(i);
        }
    }

    // For CZ and later, we can allow DIG FE and BE to differ for all display types
    if j >= 0 {
        return Some(j as usize);
    }

    None
}

/// `resource_map_pool_resources`.
pub fn resource_map_pool_resources(dc: &mut dc, context: &mut dc_state, s: usize) -> dc_status {
    let is_dio_encoder = true;

    {
        let stream = dc.streams[s].as_deref_mut().unwrap();
        calculate_phy_pix_clks(stream);
    }

    {
        let mut stream = dc.streams[s].take();
        mark_seamless_boot_stream(dc, stream.as_deref_mut().unwrap());
        dc.streams[s] = stream;
    }

    let mut acquired = false;
    if dc.streams[s].as_deref().unwrap().apply_seamless_boot_optimization {
        not_ported("acquire_resource_from_hw_enabled_state");
        // hw resource was assigned to other stream
        dc.streams[s].as_deref_mut().unwrap().apply_seamless_boot_optimization = false;
    }

    if !acquired {
        // acquire new resources
        acquired = acquire_otg_master_pipe_for_stream(dc, context, s);
    }
    let _ = acquired;

    let pool = dc.res_pool.as_deref().unwrap();
    let stream = dc.streams[s].as_deref().unwrap();
    let link = &dc.links[stream.link];

    let Some(pi) = resource_get_otg_master_for_stream(&context.res_ctx, s) else {
        return dc_status::DC_NO_CONTROLLER_RESOURCE;
    };
    if context.res_ctx.pipe_ctx[pi].stream_res.tg.is_none() {
        return dc_status::DC_NO_CONTROLLER_RESOURCE;
    }

    let stream_enc = dcn10_find_first_free_match_stream_enc_for_link(&context.res_ctx, pool, link);
    context.res_ctx.pipe_ctx[pi].stream_res.stream_enc = stream_enc;

    let Some(stream_enc) = stream_enc else {
        return dc_status::DC_NO_STREAM_ENC_RESOURCE;
    };

    update_stream_engine_usage(&mut context.res_ctx, pool, stream_enc, true);

    // Allocate DP HPO Stream Encoder based on signal, hw capabilities
    // and link settings
    if dc_is_dp_signal(stream.signal) || dc_is_virtual_signal(stream.signal) {
        let pipe = &mut context.res_ctx.pipe_ctx[pi];
        if !crate::link_dp_capability::link_decide_link_settings(dc, stream, &mut pipe.link_config.dp_link_settings) {
            return dc_status::DC_FAIL_DP_LINK_BANDWIDTH;
        }

        crate::link_dp_capability::link_decide_dp_tunnel_settings(dc, stream, &mut pipe.link_config.dp_tunnel_settings);

        if crate::link_dp_capability::link_dp_get_encoding_format(&pipe.link_config.dp_link_settings)
            == crate::dc_dp_types::dp_link_encoding::DP_128b_132b_ENCODING
        {
            // find_first_free_match_hpo_dp_stream_enc_for_link: DCN 2.1 has
            // no HPO DP stream encoder
            return dc_status::DC_NO_STREAM_ENC_RESOURCE;
        }
    }

    if dc.config.unify_link_enc_assignment && is_dio_encoder {
        not_ported("add_dio_link_enc_to_ctx");
        return dc_status::DC_NO_LINK_ENC_RESOURCE;
    }

    // TODO: Add check if ASIC support and EDID audio
    if !stream.converter_disable_audio
        && dc_is_audio_capable_signal(stream.signal)
        && stream.audio_info.mode_count != 0
        && (stream.audio_info.flags != 0
            || stream.sink.as_deref().is_some_and(|s| s.edid_caps.panel_patch.skip_audio_sab_check != 0))
    {
        let enc_id = pool.stream_enc[stream_enc].as_ref().unwrap().base().id as u32;
        let audio = find_first_free_audio(&context.res_ctx, pool, enc_id);
        context.res_ctx.pipe_ctx[pi].stream_res.audio = audio;

        // Audio assigned in order first come first get.
        // There are asics which has number of audio
        // resources less then number of pipes
        if let Some(audio) = audio {
            update_audio_usage(&mut context.res_ctx, pool, audio, true);
        }
    }

    // Add ABM to the resource if on EDP
    if dc_is_embedded_signal(stream.signal) {
        // a single ABM: DCN 2.1 has no multiple_abms
        context.res_ctx.pipe_ctx[pi].stream_res.abm = pool.abm.is_some();
    }

    let pipe = context.res_ctx.pipe_ctx[pi];
    for i in 0..context.stream_count as usize {
        if context.streams[i] == Some(s) {
            let tg = pipe.stream_res.tg.unwrap();
            let st = &mut context.stream_status[i];
            st.primary_otg_inst = pool.timing_generators[tg].as_deref().unwrap().base.inst as i32;
            st.stream_enc_inst = pool.stream_enc[stream_enc].as_ref().unwrap().base().stream_enc_inst as i32;
            st.audio_inst = pipe.stream_res.audio.map_or(-1, |a| pool.audios[a].as_deref().unwrap().base.inst as i32);

            return dc_status::DC_OK;
        }
    }

    crate::linux::Line::new().s("resource_map_pool_resources: Stream not found in new ctx!").say();
    dc_status::DC_ERROR_UNEXPECTED
}

/// `find_matching_clock_source`.
fn find_matching_clock_source(pool: &resource_pool, clock_source: clock_source_ref) -> i32 {
    for i in 0..pool.clk_src_count as usize {
        if clock_source == clock_source_ref::Pll(i) {
            return i as i32;
        }
    }
    -1
}

/// `resource_reference_clock_source`.
pub fn resource_reference_clock_source(res_ctx: &mut resource_context, pool: &resource_pool,
                                       clock_source: clock_source_ref) {
    let i = find_matching_clock_source(pool, clock_source);

    if i > -1 {
        res_ctx.clock_source_ref_count[i as usize] += 1;
    }

    if clock_source == clock_source_ref::Dp {
        res_ctx.dp_clock_source_ref_count += 1;
    }
}

/// The clock source object a pipe's reference names.
pub fn clock_source_of(pool: &resource_pool, cs: clock_source_ref) -> &crate::dce_clock_source::dce110_clk_src {
    match cs {
        clock_source_ref::Pll(i) => pool.clock_sources[i].as_deref().unwrap(),
        clock_source_ref::Dp => pool.dp_clock_source.as_deref().unwrap(),
    }
}

/// `find_matching_pll` (dce112_resource.c).
fn find_matching_pll(pool: &resource_pool, link: &crate::link_factory::dc_link) -> Option<clock_source_ref> {
    // DCE112_CLK_SRC_PLL0..5
    let i = match link.link_enc.as_ref().unwrap().base().transmitter {
        LF_TRANSMITTER_UNIPHY_A => 0,
        LF_TRANSMITTER_UNIPHY_B => 1,
        LF_TRANSMITTER_UNIPHY_C => 2,
        LF_TRANSMITTER_UNIPHY_D => 3,
        LF_TRANSMITTER_UNIPHY_E => 4,
        LF_TRANSMITTER_UNIPHY_F => 5,
        _ => return None,
    };
    pool.clock_sources[i].as_ref().map(|_| clock_source_ref::Pll(i))
}

/// `resource_map_phy_clock_resources` (dce112_resource.c).
pub fn resource_map_phy_clock_resources(dc: &dc, context: &mut dc_state, s: usize) -> dc_status {
    let pool = dc.res_pool.as_deref().unwrap();
    let stream = dc.streams[s].as_deref().unwrap();

    // acquire new resources
    let Some(pi) = resource_get_otg_master_for_stream(&context.res_ctx, s) else {
        return dc_status::DC_ERROR_UNEXPECTED;
    };

    if dc_is_dp_signal(stream.signal) || dc_is_virtual_signal(stream.signal) {
        context.res_ctx.pipe_ctx[pi].clock_source = pool.dp_clock_source.as_ref().map(|_| clock_source_ref::Dp);
    } else if dc.links[stream.link].link_enc.is_some() {
        context.res_ctx.pipe_ctx[pi].clock_source = find_matching_pll(pool, &dc.links[stream.link]);
    }

    let Some(cs) = context.res_ctx.pipe_ctx[pi].clock_source else {
        return dc_status::DC_NO_CLOCK_SOURCE_RESOURCE;
    };

    resource_reference_clock_source(&mut context.res_ctx, pool, cs);

    dc_status::DC_OK
}

/// `resource_build_bit_depth_reduction_params`.
pub fn resource_build_bit_depth_reduction_params(stream: &dc_stream_state, fmt_bit_depth: &mut bit_depth_reduction_params) {
    let mut option = stream.dither_option;
    let pixel_encoding = stream.timing.pixel_encoding;

    *fmt_bit_depth = Default::default();

    if option == DITHER_OPTION_DEFAULT {
        option = match stream.timing.display_color_depth {
            COLOR_DEPTH_666 => DITHER_OPTION_SPATIAL6,
            COLOR_DEPTH_888 => DITHER_OPTION_SPATIAL8,
            COLOR_DEPTH_101010 => DITHER_OPTION_SPATIAL10,
            _ => DITHER_OPTION_DISABLE,
        };
    }

    if option == DITHER_OPTION_DISABLE {
        return;
    }

    if option == DITHER_OPTION_TRUN6 {
        fmt_bit_depth.TRUNCATE_ENABLED = true;
        fmt_bit_depth.TRUNCATE_DEPTH = 0;
    } else if option == DITHER_OPTION_TRUN8 || option == DITHER_OPTION_TRUN8_SPATIAL6 || option == DITHER_OPTION_TRUN8_FM6 {
        fmt_bit_depth.TRUNCATE_ENABLED = true;
        fmt_bit_depth.TRUNCATE_DEPTH = 1;
    } else if option == DITHER_OPTION_TRUN10
        || option == DITHER_OPTION_TRUN10_SPATIAL6
        || option == DITHER_OPTION_TRUN10_SPATIAL8
        || option == DITHER_OPTION_TRUN10_FM8
        || option == DITHER_OPTION_TRUN10_FM6
        || option == DITHER_OPTION_TRUN10_SPATIAL8_FM6
    {
        fmt_bit_depth.TRUNCATE_ENABLED = true;
        fmt_bit_depth.TRUNCATE_DEPTH = 2;
        if option == DITHER_OPTION_TRUN10 {
            fmt_bit_depth.TRUNCATE_MODE = true;
        }
    }

    // special case - Formatter can only reduce by 4 bits at most.
    // When reducing from 12 to 6 bits,
    // HW recommends we use trunc with round mode
    // (if we did nothing, trunc to 10 bits would be used)
    // note that any 12->10 bit reduction is ignored prior to DCE8,
    // as the input was 10 bits.
    if option == DITHER_OPTION_SPATIAL6_FRAME_RANDOM || option == DITHER_OPTION_SPATIAL6 || option == DITHER_OPTION_FM6 {
        fmt_bit_depth.TRUNCATE_ENABLED = true;
        fmt_bit_depth.TRUNCATE_DEPTH = 2;
        fmt_bit_depth.TRUNCATE_MODE = true;
    }

    // spatial dither
    // note that spatial modes 1-3 are never used
    if option == DITHER_OPTION_SPATIAL6_FRAME_RANDOM
        || option == DITHER_OPTION_SPATIAL6
        || option == DITHER_OPTION_TRUN10_SPATIAL6
        || option == DITHER_OPTION_TRUN8_SPATIAL6
    {
        fmt_bit_depth.SPATIAL_DITHER_ENABLED = true;
        fmt_bit_depth.SPATIAL_DITHER_DEPTH = 0;
        fmt_bit_depth.HIGHPASS_RANDOM = true;
        fmt_bit_depth.RGB_RANDOM = pixel_encoding == PIXEL_ENCODING_RGB;
    } else if option == DITHER_OPTION_SPATIAL8_FRAME_RANDOM
        || option == DITHER_OPTION_SPATIAL8
        || option == DITHER_OPTION_SPATIAL8_FM6
        || option == DITHER_OPTION_TRUN10_SPATIAL8
        || option == DITHER_OPTION_TRUN10_SPATIAL8_FM6
    {
        fmt_bit_depth.SPATIAL_DITHER_ENABLED = true;
        fmt_bit_depth.SPATIAL_DITHER_DEPTH = 1;
        fmt_bit_depth.HIGHPASS_RANDOM = true;
        fmt_bit_depth.RGB_RANDOM = pixel_encoding == PIXEL_ENCODING_RGB;
    } else if option == DITHER_OPTION_SPATIAL10_FRAME_RANDOM
        || option == DITHER_OPTION_SPATIAL10
        || option == DITHER_OPTION_SPATIAL10_FM8
        || option == DITHER_OPTION_SPATIAL10_FM6
    {
        fmt_bit_depth.SPATIAL_DITHER_ENABLED = true;
        fmt_bit_depth.SPATIAL_DITHER_DEPTH = 2;
        fmt_bit_depth.HIGHPASS_RANDOM = true;
        fmt_bit_depth.RGB_RANDOM = pixel_encoding == PIXEL_ENCODING_RGB;
    }

    if option == DITHER_OPTION_SPATIAL6 || option == DITHER_OPTION_SPATIAL8 || option == DITHER_OPTION_SPATIAL10 {
        fmt_bit_depth.FRAME_RANDOM = false;
    } else {
        fmt_bit_depth.FRAME_RANDOM = true;
    }

    // temporal dither
    if option == DITHER_OPTION_FM6
        || option == DITHER_OPTION_SPATIAL8_FM6
        || option == DITHER_OPTION_SPATIAL10_FM6
        || option == DITHER_OPTION_TRUN10_FM6
        || option == DITHER_OPTION_TRUN8_FM6
        || option == DITHER_OPTION_TRUN10_SPATIAL8_FM6
    {
        fmt_bit_depth.FRAME_MODULATION_ENABLED = true;
        fmt_bit_depth.FRAME_MODULATION_DEPTH = 0;
    } else if option == DITHER_OPTION_FM8 || option == DITHER_OPTION_SPATIAL10_FM8 || option == DITHER_OPTION_TRUN10_FM8 {
        fmt_bit_depth.FRAME_MODULATION_ENABLED = true;
        fmt_bit_depth.FRAME_MODULATION_DEPTH = 1;
    } else if option == DITHER_OPTION_FM10 {
        fmt_bit_depth.FRAME_MODULATION_ENABLED = true;
        fmt_bit_depth.FRAME_MODULATION_DEPTH = 2;
    }

    fmt_bit_depth.pixel_encoding = pixel_encoding;
}

/// `resource_get_opp_head`.
pub fn resource_get_opp_head(res_ctx: &resource_context, pipe: usize) -> usize {
    let mut opp_head = pipe;
    while let Some(top) = res_ctx.pipe_ctx[opp_head].top_pipe {
        opp_head = top;
    }
    opp_head
}

/// `resource_get_otg_master`.
pub fn resource_get_otg_master(res_ctx: &resource_context, pipe: usize) -> usize {
    let mut otg_master = resource_get_opp_head(res_ctx, pipe);
    while let Some(prev) = res_ctx.pipe_ctx[otg_master].prev_odm_pipe {
        otg_master = prev;
    }
    otg_master
}

/// `resource_get_odm_slice_count`.
pub fn resource_get_odm_slice_count(res_ctx: &resource_context, pipe: usize) -> i32 {
    let mut odm_split_count = 1;
    let mut pipe = resource_get_otg_master(res_ctx, pipe);

    while let Some(next) = res_ctx.pipe_ctx[pipe].next_odm_pipe {
        odm_split_count += 1;
        pipe = next;
    }
    odm_split_count
}

/// `resource_get_odm_slice_index`.
pub fn resource_get_odm_slice_index(res_ctx: &resource_context, pipe: usize) -> i32 {
    let mut index = 0;
    let mut pipe = resource_get_opp_head(res_ctx, pipe);

    while let Some(prev) = res_ctx.pipe_ctx[pipe].prev_odm_pipe {
        index += 1;
        pipe = prev;
    }
    index
}

/// `resource_get_odm_slice_dst_width`. `stream` is the OTG master's.
pub fn resource_get_odm_slice_dst_width(res_ctx: &resource_context, otg_master: usize, stream: &dc_stream_state,
                                        is_last_segment: bool) -> i32 {
    let om = &res_ctx.pipe_ctx[otg_master];
    if om.stream.is_none() {
        return 0;
    }

    let timing = &stream.timing;
    let count = resource_get_odm_slice_count(res_ctx, otg_master);
    let h_active = (timing.h_addressable + timing.h_border_left + timing.h_border_right) as i32
        + om.dsc_hactive_padding as i32;
    let mut width = h_active / count;

    let mut two_pixel_alignment_required = false;
    if om.stream_res.tg.is_some() {
        two_pixel_alignment_required = crate::dcn20_optc::optc1_is_two_pixels_per_container(timing)
            // 422 is sub-sampled horizontally. 1 set of chromas
            // (Cb/Cr) is shared for 2 lumas (i.e 2 Y values).
            // Therefore even if 422 is still 1 pixel per container,
            // ODM segment width still needs to be 2 pixel aligned.
            || timing.pixel_encoding == PIXEL_ENCODING_YCBCR422;
    }
    if width % 2 != 0 && two_pixel_alignment_required {
        width += 1;
    }

    if is_last_segment { h_active - width * (count - 1) } else { width }
}

/// `resource_get_odm_slice_dst_rect`.
pub fn resource_get_odm_slice_dst_rect(res_ctx: &resource_context, pipe: usize, stream: &dc_stream_state) -> rect {
    let is_last_odm_slice = res_ctx.pipe_ctx[pipe].next_odm_pipe.is_none();
    let otg_master = resource_get_otg_master(res_ctx, pipe);
    let odm_slice_idx = resource_get_odm_slice_index(res_ctx, pipe);
    let odm_segment_offset = resource_get_odm_slice_dst_width(res_ctx, otg_master, stream, false);

    rect {
        x: odm_segment_offset * odm_slice_idx,
        width: resource_get_odm_slice_dst_width(res_ctx, otg_master, stream, is_last_odm_slice),
        y: 0,
        height: (stream.timing.v_addressable + stream.timing.v_border_bottom + stream.timing.v_border_top) as i32,
    }
}

/// `resource_get_odm_slice_src_rect`.
pub fn resource_get_odm_slice_src_rect(dc_debug: &crate::dc::dc_debug_options, res_ctx: &resource_context, pipe: usize,
                                       stream: &dc_stream_state) -> rect {
    let opp_head = resource_get_opp_head(res_ctx, pipe);

    let odm_slice_dst = resource_get_odm_slice_dst_rect(res_ctx, opp_head, stream);
    let mut odm_slice_src = odm_slice_dst;

    let left_edge_extra_pixel_count = if res_ctx.pipe_ctx[opp_head].stream_res.opp.is_some() {
        crate::dcn20_opp::opp2_get_left_edge_extra_pixel_count(dc_debug, stream.timing.pixel_encoding,
            resource_is_pipe_type(&res_ctx.pipe_ctx[opp_head], pipe_type::OTG_MASTER))
    } else {
        0
    };

    odm_slice_src.x -= left_edge_extra_pixel_count as i32;
    odm_slice_src.width += left_edge_extra_pixel_count as i32;

    odm_slice_src
}

/// `resource_get_mpc_slice_count`.
pub fn resource_get_mpc_slice_count(res_ctx: &resource_context, pipe: usize) -> i32 {
    let mut mpc_split_count = 1;
    let plane = res_ctx.pipe_ctx[pipe].plane_state;
    let mut other_pipe = res_ctx.pipe_ctx[pipe].bottom_pipe;

    while let Some(o) = other_pipe {
        if res_ctx.pipe_ctx[o].plane_state != plane {
            break;
        }
        mpc_split_count += 1;
        other_pipe = res_ctx.pipe_ctx[o].bottom_pipe;
    }
    other_pipe = res_ctx.pipe_ctx[pipe].top_pipe;
    while let Some(o) = other_pipe {
        if res_ctx.pipe_ctx[o].plane_state != plane {
            break;
        }
        mpc_split_count += 1;
        other_pipe = res_ctx.pipe_ctx[o].top_pipe;
    }

    mpc_split_count
}

/// `resource_get_mpc_slice_index`.
pub fn resource_get_mpc_slice_index(res_ctx: &resource_context, pipe: usize) -> i32 {
    let mut split_pipe = res_ctx.pipe_ctx[pipe].top_pipe;
    let mut index = 0;

    while let Some(sp) = split_pipe {
        if res_ctx.pipe_ctx[sp].plane_state != res_ctx.pipe_ctx[pipe].plane_state {
            break;
        }
        index += 1;
        split_pipe = res_ctx.pipe_ctx[sp].top_pipe;
    }

    index
}

/// `convert_pixel_format_to_dalsurface`.
fn convert_pixel_format_to_dalsurface(surface_pixel_format: u32) -> u32 {
    match surface_pixel_format {
        SURFACE_PIXEL_FORMAT_GRPH_PALETA_256_COLORS => PIXEL_FORMAT_INDEX8,
        SURFACE_PIXEL_FORMAT_GRPH_ARGB1555 => PIXEL_FORMAT_RGB565,
        SURFACE_PIXEL_FORMAT_GRPH_RGB565 => PIXEL_FORMAT_RGB565,
        SURFACE_PIXEL_FORMAT_GRPH_ARGB8888 => PIXEL_FORMAT_ARGB8888,
        SURFACE_PIXEL_FORMAT_GRPH_ABGR8888 => PIXEL_FORMAT_ARGB8888,
        SURFACE_PIXEL_FORMAT_GRPH_ARGB2101010 => PIXEL_FORMAT_ARGB2101010,
        SURFACE_PIXEL_FORMAT_GRPH_ABGR2101010 => PIXEL_FORMAT_ARGB2101010,
        SURFACE_PIXEL_FORMAT_GRPH_ABGR2101010_XR_BIAS => PIXEL_FORMAT_ARGB2101010_XRBIAS,
        SURFACE_PIXEL_FORMAT_GRPH_ABGR16161616F | SURFACE_PIXEL_FORMAT_GRPH_ARGB16161616F => PIXEL_FORMAT_FP16,
        SURFACE_PIXEL_FORMAT_VIDEO_420_YCbCr | SURFACE_PIXEL_FORMAT_VIDEO_420_YCrCb => PIXEL_FORMAT_420BPP8,
        SURFACE_PIXEL_FORMAT_VIDEO_420_10bpc_YCbCr | SURFACE_PIXEL_FORMAT_VIDEO_420_10bpc_YCrCb => PIXEL_FORMAT_420BPP10,
        _ => PIXEL_FORMAT_UNKNOWN,
    }
}

/// `calculate_plane_rec_in_timing_active`: a plane rect from Stream Source
/// space into Timing Active space, rounding left/right and top/bottom to
/// the adjacent integer and dividing only after the multiplication.
fn calculate_plane_rec_in_timing_active(stream: &dc_stream_state, rec_in: &rect) -> rect {
    let mut rec_out = rect::default();

    let temp = dc_fixpt_from_fraction(rec_in.x as i64 * stream.dst.width as i64, stream.src.width as i64);
    rec_out.x = stream.dst.x + dc_fixpt_round(temp);

    let temp = dc_fixpt_from_fraction((rec_in.x + rec_in.width) as i64 * stream.dst.width as i64,
        stream.src.width as i64);
    rec_out.width = stream.dst.x + dc_fixpt_round(temp) - rec_out.x;

    let temp = dc_fixpt_from_fraction(rec_in.y as i64 * stream.dst.height as i64, stream.src.height as i64);
    rec_out.y = stream.dst.y + dc_fixpt_round(temp);

    let temp = dc_fixpt_from_fraction((rec_in.y + rec_in.height) as i64 * stream.dst.height as i64,
        stream.src.height as i64);
    rec_out.height = stream.dst.y + dc_fixpt_round(temp) - rec_out.y;

    rec_out
}

/// `intersect_rec`.
fn intersect_rec(r0: &rect, r1: &rect) -> rect {
    let r0_x_end = r0.x + r0.width;
    let r1_x_end = r1.x + r1.width;
    let r0_y_end = r0.y + r0.height;
    let r1_y_end = r1.y + r1.height;

    let mut rec = rect::default();
    rec.x = if r0.x > r1.x { r0.x } else { r1.x };
    rec.width = if r0_x_end > r1_x_end { r1_x_end - rec.x } else { r0_x_end - rec.x };
    rec.y = if r0.y > r1.y { r0.y } else { r1.y };
    rec.height = if r0_y_end > r1_y_end { r1_y_end - rec.y } else { r0_y_end - rec.y };

    // in case that there is no intersection
    if rec.width < 0 || rec.height < 0 {
        rec = rect::default();
    }

    rec
}

/// `shift_rec`.
fn shift_rec(rec_in: &rect, x: i32, y: i32) -> rect {
    let mut rec_out = *rec_in;

    rec_out.x += x;
    rec_out.y += y;

    rec_out
}

/// `calculate_mpc_slice_in_timing_active`.
fn calculate_mpc_slice_in_timing_active(res_ctx: &resource_context, pipe: usize, stream: &dc_stream_state,
                                        plane_clip_rec: &rect) -> rect {
    let mpc_slice_count = resource_get_mpc_slice_count(res_ctx, pipe);
    let mpc_slice_idx = resource_get_mpc_slice_index(res_ctx, pipe);
    let epimo = mpc_slice_count - plane_clip_rec.width % mpc_slice_count - 1;
    let mut mpc_rec = rect::default();

    mpc_rec.width = plane_clip_rec.width / mpc_slice_count;
    mpc_rec.x = plane_clip_rec.x + mpc_rec.width * mpc_slice_idx;
    mpc_rec.height = plane_clip_rec.height;
    mpc_rec.y = plane_clip_rec.y;

    if stream.view_format == VIEW_3D_FORMAT_SIDE_BY_SIDE {
        mpc_rec.x -= mpc_rec.width * mpc_slice_idx;
    }

    // extra pixels in the division remainder need to go to pipes after
    // the extra pixel index minus one(epimo) defined here as:
    if mpc_slice_idx > epimo {
        mpc_rec.x += mpc_slice_idx - epimo - 1;
        mpc_rec.width += 1;
    }

    if stream.view_format == VIEW_3D_FORMAT_TOP_AND_BOTTOM {
        mpc_rec.height /= 2;
    }
    mpc_rec
}

/// `calculate_adjust_recout_for_visual_confirm`: (base, dpp) offsets.
fn calculate_adjust_recout_for_visual_confirm(dc_debug: &crate::dc::dc_debug_options, pool: &resource_pool,
                                              pipe: &pipe_ctx, stream: &dc_stream_state) -> (i32, i32) {
    if dc_debug.visual_confirm == VISUAL_CONFIRM_DISABLE {
        return (0, 0);
    }
    let Some(dpp) = pipe.plane_res.dpp else { return (0, 0) };

    let mut dpp_offset = (stream.timing.v_addressable / VISUAL_CONFIRM_DPP_OFFSET_DENO) as i32;
    dpp_offset *= pool.dpps[dpp].as_deref().unwrap().base.inst as i32;

    let base_offset = if dc_debug.visual_confirm_rect_height >= VISUAL_CONFIRM_BASE_MIN
        && dc_debug.visual_confirm_rect_height <= VISUAL_CONFIRM_BASE_MAX
    {
        dc_debug.visual_confirm_rect_height as i32
    } else {
        VISUAL_CONFIRM_BASE_DEFAULT as i32
    };
    (base_offset, dpp_offset)
}

/// `adjust_recout_for_visual_confirm`.
fn adjust_recout_for_visual_confirm(recout: &mut rect, dc_debug: &crate::dc::dc_debug_options, pool: &resource_pool,
                                    pipe: &pipe_ctx, stream: &dc_stream_state) {
    let (base_offset, dpp_offset) = calculate_adjust_recout_for_visual_confirm(dc_debug, pool, pipe, stream);
    recout.height -= base_offset;
    recout.height -= dpp_offset;
}

/// `calculate_recout`: the part of the plane clip, in Timing Active space,
/// this pipe's MPC slice draws within its ODM slice, relative to that
/// slice (dc_resource.c draws the spaces at length).
fn calculate_recout(dc_debug: &crate::dc::dc_debug_options, pool: &resource_pool, res_ctx: &mut resource_context,
                    pipe: usize, stream: &dc_stream_state, plane_state: &dc_plane_state) {
    let plane_clip = calculate_plane_rec_in_timing_active(stream, &plane_state.clip_rect);
    // guard plane clip from drawing beyond stream dst here
    let plane_clip = intersect_rec(&plane_clip, &stream.dst);
    let mpc_slice_of_plane_clip = calculate_mpc_slice_in_timing_active(res_ctx, pipe, stream, &plane_clip);
    let odm_slice_src = resource_get_odm_slice_src_rect(dc_debug, res_ctx, pipe, stream);
    let overlapping_area = intersect_rec(&mpc_slice_of_plane_clip, &odm_slice_src);
    if overlapping_area.height > 0 && overlapping_area.width > 0 {
        // shift the overlapping area so it is with respect to current
        // ODM slice source's position
        let mut recout = shift_rec(&overlapping_area, -odm_slice_src.x, -odm_slice_src.y);
        adjust_recout_for_visual_confirm(&mut recout, dc_debug, pool, &res_ctx.pipe_ctx[pipe], stream);
        res_ctx.pipe_ctx[pipe].plane_res.scl_data.recout = recout;
    } else {
        // if there is no overlap, zero recout
        res_ctx.pipe_ctx[pipe].plane_res.scl_data.recout = rect::default();
    }
}

/// `calculate_scaling_ratios`.
fn calculate_scaling_ratios(scl_data: &mut scaler_data, stream: &dc_stream_state, plane_state: &dc_plane_state) {
    let mut surf_src = plane_state.src_rect;
    let in_w = stream.src.width as i64;
    let in_h = stream.src.height as i64;
    let out_w = stream.dst.width as i64;
    let out_h = stream.dst.height as i64;

    // Swap surf_src height and width since scaling ratios are in recout rotation
    if plane_state.rotation == ROTATION_ANGLE_90 || plane_state.rotation == ROTATION_ANGLE_270 {
        core::mem::swap(&mut surf_src.height, &mut surf_src.width);
    }

    scl_data.ratios.horz = dc_fixpt_from_fraction(surf_src.width as i64, plane_state.dst_rect.width as i64);
    scl_data.ratios.vert = dc_fixpt_from_fraction(surf_src.height as i64, plane_state.dst_rect.height as i64);

    if stream.view_format == VIEW_3D_FORMAT_SIDE_BY_SIDE {
        scl_data.ratios.horz.value *= 2;
    } else if stream.view_format == VIEW_3D_FORMAT_TOP_AND_BOTTOM {
        scl_data.ratios.vert.value *= 2;
    }

    scl_data.ratios.vert.value = scl_data.ratios.vert.value * in_h / out_h;
    scl_data.ratios.horz.value = scl_data.ratios.horz.value * in_w / out_w;

    scl_data.ratios.horz_c = scl_data.ratios.horz;
    scl_data.ratios.vert_c = scl_data.ratios.vert;

    if scl_data.format == PIXEL_FORMAT_420BPP8 || scl_data.format == PIXEL_FORMAT_420BPP10 {
        scl_data.ratios.horz_c.value /= 2;
        scl_data.ratios.vert_c.value /= 2;
    }
    scl_data.ratios.horz = dc_fixpt_truncate(scl_data.ratios.horz, 19);
    scl_data.ratios.vert = dc_fixpt_truncate(scl_data.ratios.vert, 19);
    scl_data.ratios.horz_c = dc_fixpt_truncate(scl_data.ratios.horz_c, 19);
    scl_data.ratios.vert_c = dc_fixpt_truncate(scl_data.ratios.vert_c, 19);
}

/// `get_vp_scan_direction`: (orthogonal_rotation, flip_vert_scan_dir,
/// flip_horz_scan_dir).
fn get_vp_scan_direction(rotation: u32, horizontal_mirror: bool) -> (bool, bool, bool) {
    let mut orthogonal_rotation = false;
    let mut flip_vert_scan_dir = false;
    let mut flip_horz_scan_dir = false;
    if rotation == ROTATION_ANGLE_180 {
        flip_vert_scan_dir = true;
        flip_horz_scan_dir = true;
    } else if rotation == ROTATION_ANGLE_90 {
        orthogonal_rotation = true;
        flip_horz_scan_dir = true;
    } else if rotation == ROTATION_ANGLE_270 {
        orthogonal_rotation = true;
        flip_vert_scan_dir = true;
    }

    if horizontal_mirror {
        flip_horz_scan_dir = !flip_horz_scan_dir;
    }
    (orthogonal_rotation, flip_vert_scan_dir, flip_horz_scan_dir)
}

/// `calculate_init_and_vp`.
#[allow(clippy::too_many_arguments)]
fn calculate_init_and_vp(flip_scan_dir: bool, recout_offset_within_recout_full: i32, recout_size: i32, src_size: i32,
                         taps: i32, ratio: fixed31_32, init: &mut fixed31_32, vp_offset: &mut i32, vp_size: &mut i32) {
    // First of the taps starts sampling pixel number <init_int_part> corresponding to recout
    // pixel 1. Next recout pixel samples int part of <init + scaling ratio> and so on.
    // All following calculations are based on this logic.
    //
    // Init calculated according to formula:
    //	init = (scaling_ratio + number_of_taps + 1) / 2
    //	init_bot = init + scaling_ratio
    //	to get pixel perfect combine add the fraction from calculating vp offset
    let mut temp = dc_fixpt_mul_int(ratio, recout_offset_within_recout_full);
    *vp_offset = dc_fixpt_floor(temp);
    temp.value &= 0xffffffff;
    *init = dc_fixpt_truncate(dc_fixpt_add(dc_fixpt_div_int(dc_fixpt_add_int(ratio, taps + 1), 2), temp), 19);
    // If viewport has non 0 offset and there are more taps than covered by init then
    // we should decrease the offset and increase init so we are never sampling
    // outside of viewport.
    let mut int_part = dc_fixpt_floor(*init);
    if int_part < taps {
        int_part = taps - int_part;
        if int_part > *vp_offset {
            int_part = *vp_offset;
        }
        *vp_offset -= int_part;
        *init = dc_fixpt_add_int(*init, int_part);
    }
    // If taps are sampling outside of viewport at end of recout and there are more pixels
    // available in the surface we should increase the viewport size, regardless set vp to
    // only what is used.
    let temp = dc_fixpt_add(*init, dc_fixpt_mul_int(ratio, recout_size - 1));
    *vp_size = dc_fixpt_floor(temp);
    if *vp_size + *vp_offset > src_size {
        *vp_size = src_size - *vp_offset;
    }

    // We did all the math assuming we are scanning same direction as display does,
    // however mirror/rotation changes how vp scans vs how it is offset. If scan direction
    // is flipped we simply need to calculate offset from the other side of plane.
    // Note that outside of viewport all scaling hardware works in recout space.
    if flip_scan_dir {
        *vp_offset = src_size - *vp_offset - *vp_size;
    }
}

/// `calculate_inits_and_viewports`.
fn calculate_inits_and_viewports(dc_debug: &crate::dc::dc_debug_options, res_ctx: &mut resource_context, pipe: usize,
                                 stream: &dc_stream_state, plane_state: &dc_plane_state) {
    let mut src = plane_state.src_rect;
    let odm_slice_src = resource_get_odm_slice_src_rect(dc_debug, res_ctx, pipe, stream);
    let data = &mut res_ctx.pipe_ctx[pipe].plane_res.scl_data;
    let vpc_div = if data.format == PIXEL_FORMAT_420BPP8 || data.format == PIXEL_FORMAT_420BPP10 { 2 } else { 1 };

    let recout_clip_in_active_timing = shift_rec(&data.recout, odm_slice_src.x, odm_slice_src.y);
    let recout_dst_in_active_timing = calculate_plane_rec_in_timing_active(stream, &plane_state.dst_rect);
    let overlap_in_active_timing = intersect_rec(&recout_clip_in_active_timing, &recout_dst_in_active_timing);
    let recout_clip_in_recout_dst = if overlap_in_active_timing.width > 0 && overlap_in_active_timing.height > 0 {
        shift_rec(&overlap_in_active_timing, -recout_dst_in_active_timing.x, -recout_dst_in_active_timing.y)
    } else {
        rect::default()
    };

    // Work in recout rotation since that requires less transformations
    let (orthogonal_rotation, mut flip_vert_scan_dir, mut flip_horz_scan_dir) =
        get_vp_scan_direction(plane_state.rotation, plane_state.horizontal_mirror);

    if orthogonal_rotation {
        core::mem::swap(&mut src.width, &mut src.height);
        core::mem::swap(&mut flip_vert_scan_dir, &mut flip_horz_scan_dir);
    }

    let (recout, taps, ratios) = (data.recout, data.taps, data.ratios);
    calculate_init_and_vp(flip_horz_scan_dir, recout_clip_in_recout_dst.x, recout.width, src.width,
        taps.h_taps as i32, ratios.horz, &mut data.inits.h, &mut data.viewport.x, &mut data.viewport.width);
    calculate_init_and_vp(flip_horz_scan_dir, recout_clip_in_recout_dst.x, recout.width, src.width / vpc_div,
        taps.h_taps_c as i32, ratios.horz_c, &mut data.inits.h_c, &mut data.viewport_c.x,
        &mut data.viewport_c.width);
    calculate_init_and_vp(flip_vert_scan_dir, recout_clip_in_recout_dst.y, recout.height, src.height,
        taps.v_taps as i32, ratios.vert, &mut data.inits.v, &mut data.viewport.y, &mut data.viewport.height);
    calculate_init_and_vp(flip_vert_scan_dir, recout_clip_in_recout_dst.y, recout.height, src.height / vpc_div,
        taps.v_taps_c as i32, ratios.vert_c, &mut data.inits.v_c, &mut data.viewport_c.y,
        &mut data.viewport_c.height);
    if orthogonal_rotation {
        core::mem::swap(&mut data.viewport.x, &mut data.viewport.y);
        core::mem::swap(&mut data.viewport.width, &mut data.viewport.height);
        core::mem::swap(&mut data.viewport_c.x, &mut data.viewport_c.y);
        core::mem::swap(&mut data.viewport_c.width, &mut data.viewport_c.height);
    }
    data.viewport.x += src.x;
    data.viewport.y += src.y;
    data.viewport_c.x += src.x / vpc_div;
    data.viewport_c.y += src.y / vpc_div;
}

/// `resource_build_scaling_params`. DCN 2.1 does not take the SPL path
/// (`config.use_spl` is false), so this is the DC scaler math.
pub fn resource_build_scaling_params(dc: &mut dc, res_ctx: &mut resource_context, pipe: usize) -> bool {
    let p = res_ctx.pipe_ctx[pipe];
    let Some(plane) = p.plane_state else { return false };
    let plane_state = *dc.planes[plane].as_deref().unwrap();
    let dce_version = dc.ctx.dce_version;
    let pool = dc.res_pool.as_deref().unwrap();
    let dc_debug = &dc.debug;
    let stream = dc.streams[p.stream.unwrap()].as_deref_mut().unwrap();
    let timing = stream.timing;
    let mut res = false;

    // Invalid input
    if plane_state.dst_rect.width == 0
        || plane_state.dst_rect.height == 0
        || plane_state.src_rect.width == 0
        || plane_state.src_rect.height == 0
    {
        return false;
    }

    let odm_slice_src = resource_get_odm_slice_src_rect(dc_debug, res_ctx, pipe, stream);

    // Timing borders are part of vactive that we are also supposed to skip in addition
    // to any stream dst offset. Since dm logic assumes dst is in addressable
    // space we need to add the left and top borders to dst offsets temporarily.
    // TODO: fix in DM, stream dst is supposed to be in vactive
    stream.dst.x += timing.h_border_left as i32;
    stream.dst.y += timing.v_border_top as i32;
    let stream: &dc_stream_state = stream;

    // Calculate H and V active size
    {
        let scl = &mut res_ctx.pipe_ctx[pipe].plane_res.scl_data;
        scl.h_active = odm_slice_src.width;
        scl.v_active = odm_slice_src.height;
        scl.format = convert_pixel_format_to_dalsurface(plane_state.format);
    }

    // depends on h_active
    calculate_recout(dc_debug, pool, res_ctx, pipe, stream, &plane_state);
    // depends on pixel format
    calculate_scaling_ratios(&mut res_ctx.pipe_ctx[pipe].plane_res.scl_data, stream, &plane_state);

    // LB calculations depend on vp size, h/v_active and scaling ratios
    // Setting line buffer pixel depth to 24bpp yields banding
    // on certain displays, such as the Sharp 4k. 36bpp is needed
    // to support SURFACE_PIXEL_FORMAT_GRPH_ARGB16161616 and
    // SURFACE_PIXEL_FORMAT_GRPH_ABGR16161616 with actual > 10 bpc
    // precision on DCN display engines, but apparently not for DCE, as
    // far as testing on DCE-11.2 and DCE-8 showed. Various DCE parts have
    // problems: Carrizo with DCE_VERSION_11_0 does not like 36 bpp lb depth,
    // neither do DCE-8 at 4k resolution, or DCE-11.2 (broken identify pixel
    // passthrough). Therefore only use 36 bpp on DCN where it is actually needed.
    let dpp = p.plane_res.dpp.map(|d| pool.dpps[d].as_deref().unwrap());
    {
        let scl = &mut res_ctx.pipe_ctx[pipe].plane_res.scl_data;
        if (dce_version as i32) > (dce_version::DCE_VERSION_MAX as i32) {
            scl.lb_params.depth = crate::dcn20_dpp::LB_PIXEL_DEPTH_36BPP as u32;
        } else {
            scl.lb_params.depth = crate::dcn20_dpp::LB_PIXEL_DEPTH_30BPP as u32;
        }

        scl.lb_params.alpha_en = plane_state.per_pixel_alpha;

        // get TAP value with 100x100 dummy data for max scaling qualify, override
        // if a new scaling quality required
        scl.viewport.width = 100;
        scl.viewport.height = 100;
        scl.viewport_c.width = 100;
        scl.viewport_c.height = 100;
        if let Some(dpp) = dpp {
            res = crate::dcn20_dpp::dpp1_get_optimal_number_of_taps(dpp, dc_debug, scl, &plane_state.scaling_quality);
        }
    }

    let temp: scaling_taps = res_ctx.pipe_ctx[pipe].plane_res.scl_data.taps;

    calculate_inits_and_viewports(dc_debug, res_ctx, pipe, stream, &plane_state);

    {
        let scl = &mut res_ctx.pipe_ctx[pipe].plane_res.scl_data;
        if let Some(dpp) = dpp {
            res = crate::dcn20_dpp::dpp1_get_optimal_number_of_taps(dpp, dc_debug, scl, &plane_state.scaling_quality);
        }

        if !res {
            // Try 24 bpp linebuffer
            scl.lb_params.depth = crate::dcn20_dpp::LB_PIXEL_DEPTH_24BPP as u32;

            if let Some(dpp) = dpp {
                res = crate::dcn20_dpp::dpp1_get_optimal_number_of_taps(dpp, dc_debug, scl,
                    &plane_state.scaling_quality);
            }
        }
    }

    // Ignore scaler failure if pipe context plane is phantom plane
    if !res && plane_state.is_phantom {
        res = true;
    }

    let taps = res_ctx.pipe_ctx[pipe].plane_res.scl_data.taps;
    if res
        && (taps.v_taps != temp.v_taps
            || taps.h_taps != temp.h_taps
            || taps.v_taps_c != temp.v_taps_c
            || taps.h_taps_c != temp.h_taps_c)
    {
        calculate_inits_and_viewports(dc_debug, res_ctx, pipe, stream, &plane_state);
    }

    // Handle side by side and top bottom 3d recout offsets after vp calculation
    // since 3d is special and needs to calculate vp as if there is no recout offset
    // This may break with rotation, good thing we aren't mixing hw rotation and 3d
    if let Some(top) = p.top_pipe {
        if res_ctx.pipe_ctx[top].plane_state == Some(plane) {
            let scl = &mut res_ctx.pipe_ctx[pipe].plane_res.scl_data;
            if stream.view_format == VIEW_3D_FORMAT_TOP_AND_BOTTOM {
                scl.recout.y += scl.recout.height;
            } else if stream.view_format == VIEW_3D_FORMAT_SIDE_BY_SIDE {
                scl.recout.x += scl.recout.width;
            }
        }
    }

    // Clamp minimum viewport size
    {
        let scl = &mut res_ctx.pipe_ctx[pipe].plane_res.scl_data;
        if scl.viewport.height < MIN_VIEWPORT_SIZE as i32 {
            scl.viewport.height = MIN_VIEWPORT_SIZE as i32;
        }
        if scl.viewport.width < MIN_VIEWPORT_SIZE as i32 {
            scl.viewport.width = MIN_VIEWPORT_SIZE as i32;
        }
    }

    let stream = dc.streams[p.stream.unwrap()].as_deref_mut().unwrap();
    stream.dst.x -= timing.h_border_left as i32;
    stream.dst.y -= timing.v_border_top as i32;

    res
}

/// `resource_add_otg_master_for_stream_output`: the pool's
/// `add_stream_to_ctx`, `dcn20_add_stream_to_ctx` on DCN 2.1.
pub fn resource_add_otg_master_for_stream_output(dc: &mut dc, new_ctx: &mut dc_state, stream: usize) -> dc_status {
    crate::dcn20_resource::dcn20_add_stream_to_ctx(dc, new_ctx, stream)
}

/// `struct audio_check` (dc_types.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct audio_check {
    pub audio_packet_type: u32,
    pub max_audiosample_rate: u32,
    pub acat: u32,
}

/// `get_max_audio_sample_rate`: `sample_rates` is the union's byte, RATE_32
/// in bit 0 up to RATE_192 in bit 6.
fn get_max_audio_sample_rate(modes: &crate::dc_stream::audio_mode) -> u32 {
    let r = modes.sample_rates;
    if r & (1 << 6) != 0 {
        return 192000;
    }
    if r & (1 << 5) != 0 {
        return 176400;
    }
    if r & (1 << 4) != 0 {
        return 96000;
    }
    if r & (1 << 3) != 0 {
        return 88200;
    }
    if r & (1 << 2) != 0 {
        return 48000;
    }
    if r & (1 << 1) != 0 {
        return 44100;
    }
    if r & 1 != 0 {
        return 32000;
    }
    // original logic when no audio info
    441000
}

/// `get_audio_check`.
pub fn get_audio_check(aud_modes: &crate::dc_stream::audio_info, audio_chk: &mut audio_check) {
    audio_chk.audio_packet_type = 0x2; // audio sample packet AP = .25 for layout0, 1 for layout1

    audio_chk.max_audiosample_rate = 0;
    for i in 0..aud_modes.mode_count as usize {
        let max_sample_rate = get_max_audio_sample_rate(&aud_modes.modes[i]);
        if audio_chk.max_audiosample_rate < max_sample_rate {
            audio_chk.max_audiosample_rate = max_sample_rate;
        }
        // dts takes the same as type 2: AP = 0.25
    }
    // check which one take more bandwidth
    if audio_chk.max_audiosample_rate > 192000 {
        audio_chk.audio_packet_type = 0x9; // AP =1
    }
    audio_chk.acat = 0; // not support
}

/// `resource_are_streams_timing_synchronizable`.
pub fn resource_are_streams_timing_synchronizable(_dc: &dc, stream1: &dc_stream_state, stream2: &dc_stream_state)
                                                  -> bool {
    let (t1, t2) = (&stream1.timing, &stream2.timing);
    if t1.h_total != t2.h_total
        || t1.v_total != t2.v_total
        || t1.h_addressable != t2.h_addressable
        || t1.v_addressable != t2.v_addressable
        || t1.v_front_porch != t2.v_front_porch
        || t1.pix_clk_100hz != t2.pix_clk_100hz
        || stream1.clamping.c_depth != stream2.clamping.c_depth
    {
        return false;
    }

    if stream1.phy_pix_clk != stream2.phy_pix_clk
        && (!dc_is_dp_signal(stream1.signal) || !dc_is_dp_signal(stream2.signal))
    {
        return false;
    }

    if stream1.view_format != stream2.view_format {
        return false;
    }

    if stream1.ignore_msa_timing_param || stream2.ignore_msa_timing_param {
        return false;
    }

    true
}

/// `resource_are_vblanks_synchronizable`: DCN 2.1 has no
/// `vblank_alignment_dto_params`, so streams of different timing never are.
pub fn resource_are_vblanks_synchronizable(_dc: &dc, _stream1: &dc_stream_state, _stream2: &dc_stream_state) -> bool {
    false
}

/// `find_pll_sharable_stream`.
fn find_pll_sharable_stream(dc: &dc, stream_needs_pll: usize, context: &dc_state) -> Option<usize> {
    let s1 = dc.streams[stream_needs_pll].as_deref().unwrap();
    for i in 0..context.stream_count as usize {
        let sh = context.streams[i].unwrap();
        let stream_has_pll = dc.streams[sh].as_deref().unwrap();

        // We are looking for non dp, non virtual stream
        if resource_are_streams_timing_synchronizable(dc, s1, stream_has_pll)
            && !dc_is_dp_signal(stream_has_pll.signal)
            && dc.links[stream_has_pll.link].connector_signal != LF_SIGNAL_TYPE_VIRTUAL
        {
            return Some(sh);
        }
    }

    None
}

/// `resource_unreference_clock_source`.
pub fn resource_unreference_clock_source(res_ctx: &mut resource_context, pool: &resource_pool,
                                         clock_source: Option<clock_source_ref>) {
    let Some(cs) = clock_source else { return };
    let i = find_matching_clock_source(pool, cs);

    if i > -1 {
        res_ctx.clock_source_ref_count[i as usize] -= 1;
    }

    if cs == clock_source_ref::Dp {
        res_ctx.dp_clock_source_ref_count -= 1;
    }
}

/// `resource_build_scaling_params_for_context`.
pub fn resource_build_scaling_params_for_context(dc: &mut dc, context: &mut dc_state) -> dc_status {
    for i in 0..MAX_PIPES {
        if context.res_ctx.pipe_ctx[i].plane_state.is_some() && context.res_ctx.pipe_ctx[i].stream.is_some()
            && !resource_build_scaling_params(dc, &mut context.res_ctx, i)
        {
            return dc_status::DC_FAIL_SCALING;
        }
    }

    dc_status::DC_OK
}

/// `dc_validate_global_state`. DCN 2.1's pool has no `validate_global` and
/// no `patch_unknown_plane_state`, and `enable_hblank_borrow` is off.
pub fn dc_validate_global_state(dc: &mut dc, new_ctx: &mut dc_state, validate_mode: u32) -> dc_status {
    let pipe_count = dc.res_pool.as_deref().unwrap().pipe_count as usize;

    for i in 0..new_ctx.stream_count as usize {
        let stream = new_ctx.streams[i].unwrap();

        for j in 0..pipe_count {
            if new_ctx.res_ctx.pipe_ctx[j].stream != Some(stream) {
                continue;
            }

            // Switch to dp clock source only if there is
            // no non dp stream that shares the same timing
            // with the dp stream.
            let signal = dc.streams[stream].as_deref().unwrap().signal;
            if dc_is_dp_signal(signal) && find_pll_sharable_stream(dc, stream, new_ctx).is_none() {
                let pool = dc.res_pool.as_deref().unwrap();
                let cs = new_ctx.res_ctx.pipe_ctx[j].clock_source;
                resource_unreference_clock_source(&mut new_ctx.res_ctx, pool, cs);

                new_ctx.res_ctx.pipe_ctx[j].clock_source = pool.dp_clock_source.as_ref().map(|_| clock_source_ref::Dp);
                if let Some(cs) = new_ctx.res_ctx.pipe_ctx[j].clock_source {
                    resource_reference_clock_source(&mut new_ctx.res_ctx, pool, cs);
                }
            }
        }
    }

    let mut result = resource_build_scaling_params_for_context(dc, new_ctx);

    if result == dc_status::DC_OK {
        result = crate::dcn21_resource::dcn21_validate_bandwidth(dc, new_ctx, validate_mode);
    }

    result
}

/// `dc_validate_stream` (dc_resource.c): the timing against the first
/// timing generator, the link encoder and the link.
pub fn dc_validate_stream(adev: &amdgpu_device, dc: &mut dc, s: usize) -> dc_status {
    let mut res = dc_status::DC_OK;

    calculate_phy_pix_clks(dc.streams[s].as_deref_mut().unwrap());

    let stream = dc.streams[s].as_deref().unwrap();
    let li = stream.link;
    let tg = dc.res_pool.as_deref().unwrap().timing_generators[0].as_deref().unwrap();
    if !(tg.funcs().validate_timing.unwrap())(tg, &stream.timing) {
        res = dc_status::DC_FAIL_CONTROLLER_VALIDATE;
    }

    if res == dc_status::DC_OK && dc.links[li].ep_type == LF_DISPLAY_ENDPOINT_PHY {
        let stream = dc.streams[s].as_deref().unwrap();
        let connector_signal = dc.links[li].connector_signal;
        let hdmi20_disable = dc.debug.hdmi20_disable;
        let enc = dc.links[li].link_enc.as_mut().unwrap();
        if !(enc.funcs().validate_output_with_stream.unwrap())(enc, connector_signal, stream, hdmi20_disable) {
            res = dc_status::DC_FAIL_ENC_VALIDATE;
        }
    }

    // TODO: validate audio ASIC caps, encoder

    if res == dc_status::DC_OK {
        let stream = dc.streams[s].as_deref().unwrap();
        res = crate::link_validation::link_validate_mode_timing(dc, stream.signal, li, &stream.timing);
    }

    let _ = adev;
    res
}

/// `add_all_planes_for_stream` (dc_resource.c).
fn add_all_planes_for_stream(dc: &mut dc, stream: usize, set: &[crate::dc::dc_validation_set], state: &mut dc_state)
                             -> bool {
    let Some(i) = set.iter().position(|s| s.stream == stream) else {
        crate::linux::Line::new().s("Stream not found in set!").say();
        return false;
    };

    for j in 0..set[i].plane_count as usize {
        if !crate::dc_state::dc_state_add_plane(dc, stream, set[i].plane_states[j].unwrap(), state) {
            return false;
        }
    }

    true
}

/// `planes_changed_for_existing_stream` (dc_resource.c).
fn planes_changed_for_existing_stream(context: &dc_state, stream: usize, set: &[crate::dc::dc_validation_set]) -> bool {
    let Some(i) = (0..context.stream_count as usize).find(|&i| context.streams[i] == Some(stream)) else {
        crate::linux::Line::new().s("Stream not found!").say();
        return false;
    };
    let stream_status = &context.stream_status[i];

    let Some(j) = set.iter().position(|s| s.stream == stream) else { return false };

    if stream_status.plane_count != set[j].plane_count as i32 {
        return true;
    }

    for i in 0..set[j].plane_count as usize {
        if stream_status.plane_states[i] != set[j].plane_states[i] {
            return true;
        }
    }

    false
}

/// `dc_validate_with_context` (dc_resource.c): bring `context` to the
/// streams and planes of `set` and validate it. Removing a stream (and
/// the writeback data it hands over) is a later stage, and so are SubVP
/// phantoms; `calculate_pix_rate_divider` is not a member of dcn21_funcs.
pub fn dc_validate_with_context(adev: &amdgpu_device, dc: &mut dc, set: &[crate::dc::dc_validation_set],
                                context: &mut dc_state, validate_mode: u32) -> dc_status {
    let mut unchanged_streams = alloc::vec::Vec::new();
    let mut del_streams = alloc::vec::Vec::new();
    let mut add_streams = alloc::vec::Vec::new();
    let old_stream_count = context.stream_count as usize;
    let mut res;

    // First build a list of streams to be remove from current context
    for i in 0..old_stream_count {
        let stream = context.streams[i].unwrap();
        if !set.iter().any(|s| s.stream == stream) {
            del_streams.push(stream);
        }
    }

    // Second, build a list of new streams
    for s in set {
        if !(0..old_stream_count).any(|j| context.streams[j] == Some(s.stream)) {
            add_streams.push(s.stream);
        }
    }

    // Build a list of unchanged streams which is necessary for handling
    // planes change such as added, removed, and updated.
    for s in set {
        if !del_streams.contains(&s.stream) && !add_streams.contains(&s.stream) {
            unchanged_streams.push(s.stream);
        }
    }

    // Remove all planes for unchanged streams if planes changed
    for &u in &unchanged_streams {
        if planes_changed_for_existing_stream(context, u, set)
            && !crate::dc_state::dc_state_rem_all_planes_for_stream(dc, u, context)
        {
            return dc_status::DC_FAIL_DETACH_SURFACES;
        }
    }

    // Remove all planes for removed streams and then remove the streams
    if !del_streams.is_empty() {
        not_ported("dc_validate_with_context: removing a stream");
        return dc_status::DC_ERROR_UNEXPECTED;
    }

    // Swap seamless boot stream to pipe 0 (if needed) to ensure pipe_ctx
    // matches. This may change in the future if seamless_boot_stream can be
    // multiple.
    for i in 0..add_streams.len() {
        let mut st = dc.streams[add_streams[i]].take().unwrap();
        mark_seamless_boot_stream(dc, &mut st);
        dc.streams[add_streams[i]] = Some(st);
        if dc.streams[add_streams[i]].as_deref().unwrap().apply_seamless_boot_optimization && i != 0 {
            add_streams.swap(0, i);
            break;
        }
    }

    // Add new streams and then add all planes for the new stream
    for &a in &add_streams {
        calculate_phy_pix_clks(dc.streams[a].as_deref_mut().unwrap());
        res = crate::dc_state::dc_state_add_stream(dc, context, a);
        if res != dc_status::DC_OK {
            return res;
        }

        if !add_all_planes_for_stream(dc, a, set, context) {
            return dc_status::DC_FAIL_ATTACH_SURFACES;
        }
    }

    // Add all planes for unchanged streams if planes changed
    for &u in &unchanged_streams {
        if planes_changed_for_existing_stream(context, u, set) && !add_all_planes_for_stream(dc, u, set, context) {
            return dc_status::DC_FAIL_ATTACH_SURFACES;
        }
    }

    // clear subvp cursor limitations
    for i in 0..context.stream_count as usize {
        let s = context.streams[i].unwrap();
        crate::dc_state::dc_state_set_stream_subvp_cursor_limit(s, context, false);
    }

    res = dc_validate_global_state(dc, context, validate_mode);

    let _ = adev;
    res
}

/// `resource_get_opp_heads_for_otg_master` (dc_resource.c): the OPP heads
/// of an OTG master's ODM slices, in order.
pub fn resource_get_opp_heads_for_otg_master(res_ctx: &resource_context, otg_master: usize) -> alloc::vec::Vec<usize> {
    let mut opp_heads = alloc::vec::Vec::new();

    if !resource_is_pipe_type(&res_ctx.pipe_ctx[otg_master], pipe_type::OTG_MASTER) {
        crate::linux::Line::new().s("resource_get_opp_heads_for_otg_master called from a non OTG master").say();
        return opp_heads;
    }
    let mut opp_head = Some(res_ctx.pipe_ctx[otg_master].pipe_idx as usize);
    while let Some(h) = opp_head {
        opp_heads.push(h);
        opp_head = res_ctx.pipe_ctx[h].next_odm_pipe;
    }
    opp_heads
}

/// `resource_is_odm_topology_changed` (dc_resource.c): the OPP heads of the
/// two OTG masters differ.
pub fn resource_is_odm_topology_changed(res_a: &resource_context, otg_master_a: &pipe_ctx, res_b: &resource_context,
                                        otg_master_b: &pipe_ctx) -> bool {
    if !resource_is_pipe_type(otg_master_a, pipe_type::OTG_MASTER)
        || !resource_is_pipe_type(otg_master_b, pipe_type::OTG_MASTER)
    {
        return true;
    }

    let mut opp_head_a = Some(otg_master_a);
    let mut opp_head_b = Some(otg_master_b);
    while let (Some(a), Some(b)) = (opp_head_a, opp_head_b) {
        if a.stream_res.opp != b.stream_res.opp {
            return true;
        }
        if a.next_odm_pipe.is_some() != b.next_odm_pipe.is_some() {
            return true;
        }
        opp_head_a = a.next_odm_pipe.map(|n| &res_a.pipe_ctx[n]);
        opp_head_b = b.next_odm_pipe.map(|n| &res_b.pipe_ctx[n]);
    }

    false
}

/// `resource_pixel_format_to_bpp` (dc_resource.c): bits per pixel; -1 for
/// a format with none.
pub fn resource_pixel_format_to_bpp(format: u32) -> u32 {
    match format {
        SURFACE_PIXEL_FORMAT_GRPH_PALETA_256_COLORS => 8,
        SURFACE_PIXEL_FORMAT_VIDEO_420_YCbCr | SURFACE_PIXEL_FORMAT_VIDEO_420_YCrCb => 12,
        SURFACE_PIXEL_FORMAT_GRPH_ARGB1555 | SURFACE_PIXEL_FORMAT_GRPH_RGB565
        | SURFACE_PIXEL_FORMAT_VIDEO_420_10bpc_YCbCr | SURFACE_PIXEL_FORMAT_VIDEO_420_10bpc_YCrCb => 16,
        SURFACE_PIXEL_FORMAT_GRPH_ARGB8888 | SURFACE_PIXEL_FORMAT_GRPH_ABGR8888 | SURFACE_PIXEL_FORMAT_GRPH_ARGB2101010
        | SURFACE_PIXEL_FORMAT_GRPH_ABGR2101010 | SURFACE_PIXEL_FORMAT_GRPH_ABGR2101010_XR_BIAS
        | SURFACE_PIXEL_FORMAT_GRPH_RGBE | SURFACE_PIXEL_FORMAT_GRPH_RGBE_ALPHA => 32,
        SURFACE_PIXEL_FORMAT_GRPH_ARGB16161616 | SURFACE_PIXEL_FORMAT_GRPH_ABGR16161616
        | SURFACE_PIXEL_FORMAT_GRPH_ARGB16161616F | SURFACE_PIXEL_FORMAT_GRPH_ABGR16161616F => 64,
        _ => u32::MAX,
    }
}
