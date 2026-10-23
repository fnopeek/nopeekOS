//! display/dc/core/dc_stream.c + dc_surface.c and the stream and plane
//! types of dc_stream.h / dc.h (Linux 6.18.55): what the DM hands DC to
//! show — a stream on a link and the planes composed into it. Linux shares
//! them between contexts by reference count; the port keeps them in `dc`'s
//! arenas (`dc.streams`, `dc.planes`) and a context names them by index.
#![allow(dead_code)]

use crate::dc::dc;
use crate::dc_hw_types::*;
use crate::dc_sink::{dc_panel_patch, dc_sink, AUDIO_INFO_DISPLAY_NAME_SIZE_IN_CHARS, DC_MAX_AUDIO_DESC_COUNT};
use crate::regs::*;
use alloc::boxed::Box;

/// `struct audio_mode` (audio_types.h); `sample_rates` is its byte.
#[derive(Clone, Copy, Default, Debug)]
pub struct audio_mode {
    pub format_code: u32,
    pub channel_count: u8,
    pub sample_rates: u8,
    pub sample_size: u8,
}

/// `struct audio_info` (dc_types.h); `flags` is its byte.
#[derive(Clone, Copy, Default, Debug)]
pub struct audio_info {
    pub flags: u32,
    pub video_latency: u32,
    pub audio_latency: u32,
    pub display_index: u32,
    pub display_name: [u8; AUDIO_INFO_DISPLAY_NAME_SIZE_IN_CHARS],
    pub manufacture_id: u32,
    pub product_id: u32,
    pub port_id: [u32; 2],
    pub mode_count: u32,
    pub modes: [audio_mode; DC_MAX_AUDIO_DESC_COUNT],
}

/// `struct dc_transfer_func`, the predefined kinds; the distributed points
/// and PWL of a custom curve join with colour management.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_transfer_func {
    /// `enum dc_transfer_func_type`.
    pub r#type: u32,
    /// `enum dc_transfer_func_predefined`.
    pub tf: u32,
    pub sdr_ref_white_level: u32,
}

/// `struct bit_depth_reduction_params` (dc_hw_types.h); `flags` are the
/// bitfield union's members.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct bit_depth_reduction_params {
    pub TRUNCATE_ENABLED: bool,
    pub TRUNCATE_DEPTH: u8,
    pub TRUNCATE_MODE: bool,
    pub SPATIAL_DITHER_ENABLED: bool,
    pub SPATIAL_DITHER_DEPTH: u8,
    pub SPATIAL_DITHER_MODE: u8,
    pub RGB_RANDOM: bool,
    pub FRAME_RANDOM: bool,
    pub HIGHPASS_RANDOM: bool,
    pub FRAME_MODULATION_ENABLED: bool,
    pub FRAME_MODULATION_DEPTH: u8,
    pub TEMPORAL_LEVEL: bool,
    pub FRC25: u8,
    pub FRC50: u8,
    pub FRC75: u8,
    /// `enum dc_pixel_encoding`.
    pub pixel_encoding: u32,
}

/// `struct clamping_and_pixel_encoding_params` (dc_hw_types.h).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct clamping_and_pixel_encoding_params {
    pub pixel_encoding: u32,
    pub clamping_level: u32,
    pub c_depth: u32,
}

/// `struct dc_stream_state`, the members the commit path reaches. `link` is
/// the link's index; `sink` the sink it was made for.
#[derive(Clone, Debug)]
pub struct dc_stream_state {
    pub sink: Option<Box<dc_sink>>,
    pub link: usize,
    pub sink_patches: dc_panel_patch,
    pub timing: dc_crtc_timing,
    pub src: rect,
    pub dst: rect,
    pub out_transfer_func: dc_transfer_func,
    /// `enum dc_color_space`.
    pub output_color_space: u32,
    /// `enum dc_dither_option`.
    pub dither_option: u32,
    /// `enum view_3d_format`.
    pub view_format: u32,
    pub audio_info: audio_info,
    pub qs_bit: u8,
    pub qy_bit: u8,
    pub converter_disable_audio: bool,
    /// `enum signal_type`.
    pub signal: u32,
    pub phy_pix_clk: u32,
    pub apply_seamless_boot_optimization: bool,
    pub clamping: clamping_and_pixel_encoding_params,
    pub bit_depth_params: bit_depth_reduction_params,
    pub stream_id: u32,
    pub use_dynamic_meta: bool,
    pub adjust: dc_crtc_timing_adjust,
    pub writeback_info: [dc_writeback_info; MAX_DWB_PIPES],
    pub ignore_msa_timing_param: bool,
    /// `adaptive_sync_infopacket.valid`.
    pub adaptive_sync_infopacket_valid: bool,
    pub refcount: u32,
}

/// `MAX_DWB_PIPES` (dc.h).
pub const MAX_DWB_PIPES: usize = 1;

/// `struct dc_crtc_timing_adjust` (dc_stream.h), the members reached.
#[derive(Clone, Copy, Debug, Default)]
pub struct dc_crtc_timing_adjust {
    pub v_total_min: u32,
    pub v_total_max: u32,
    pub v_total_mid: u32,
    pub v_total_mid_frame_num: u32,
}

/// `struct dc_writeback_info` (dc_stream.h), the members bandwidth
/// validation reads; the DWB programming joins with writeback.
#[derive(Clone, Copy, Debug, Default)]
pub struct dc_writeback_info {
    pub wb_enabled: bool,
    pub dwb_pipe_inst: i32,
    /// `dwb_params.cnv_params.crop_width` / `crop_height`.
    pub crop_width: u32,
    pub crop_height: u32,
    pub dest_width: u32,
    pub dest_height: u32,
    /// `dwb_params.scaler_taps.h_taps_c` / `v_taps_c`.
    pub h_taps_c: u32,
    pub v_taps_c: u32,
    /// `enum dwb_scaler_mode`, `enum dwb_output_depth`.
    pub out_format: u32,
    pub output_depth: u32,
}

/// `struct dc_plane_state`, the members the commit path reaches.
#[derive(Clone, Copy, Debug, Default)]
pub struct dc_plane_state {
    pub address: dc_plane_address,
    pub scaling_quality: scaling_taps,
    pub src_rect: rect,
    pub dst_rect: rect,
    pub clip_rect: rect,
    pub plane_size: plane_size,
    pub tiling_info: dc_tiling_info_gfx9,
    pub gamma_correction_is_identity: bool,
    pub in_transfer_func: dc_transfer_func,
    pub in_shaper_func: dc_transfer_func,
    pub blend_tf: dc_transfer_func,
    /// `lut3d_func.state.raw`.
    pub lut3d_state: u32,
    /// `enum dc_color_space`.
    pub color_space: u32,
    /// `enum surface_pixel_format`.
    pub format: u32,
    /// `enum dc_rotation_angle`.
    pub rotation: u32,
    pub horizontal_mirror: bool,
    pub visible: bool,
    pub per_pixel_alpha: bool,
    pub pre_multiplied_alpha: bool,
    pub global_alpha: bool,
    pub global_alpha_value: i32,
    pub is_phantom: bool,
    pub flip_immediate: bool,
    /// `enum plane_stereo_format`.
    pub stereo_format: u32,
    pub dcc: dc_plane_dcc_param,
    pub refcount: u32,
}

/// `struct dc_plane_dcc_param` (dc_hw_types.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct dc_plane_dcc_param {
    pub enable: bool,
    pub meta_pitch: i32,
    pub independent_64b_blks: bool,
    pub dcc_ind_blk: u32,
    pub meta_pitch_c: i32,
    pub independent_64b_blks_c: bool,
    pub dcc_ind_blk_c: u32,
}

/// `update_stream_signal`.
pub fn update_stream_signal(dc: &dc, stream: &mut dc_stream_state, sink: &dc_sink) {
    if sink.sink_signal == LF_SIGNAL_TYPE_NONE {
        stream.signal = dc.links[stream.link].connector_signal;
    } else {
        stream.signal = sink.sink_signal;
    }

    if crate::link_detection::dc_is_dvi_signal(stream.signal) {
        if dc.caps.dual_link_dvi
            && (stream.timing.pix_clk_100hz / 10) > TMDS_MAX_PIXEL_CLOCK
            && sink.sink_signal != LF_SIGNAL_TYPE_DVI_SINGLE_LINK
        {
            stream.signal = LF_SIGNAL_TYPE_DVI_DUAL_LINK;
        } else {
            stream.signal = LF_SIGNAL_TYPE_DVI_SINGLE_LINK;
        }
    }
}

/// `dc_stream_assign_stream_id`.
fn dc_stream_assign_stream_id(dc: &mut dc, stream: &mut dc_stream_state) {
    // MSB is reserved to indicate phantoms
    stream.stream_id = dc.ctx.dc_stream_id_count;
    dc.ctx.dc_stream_id_count += 1;
}

/// `dc_stream_construct`. The sink has no container id, so the port id is
/// the fixed one Linux uses.
fn dc_stream_construct(dc: &mut dc, li: usize, dc_sink_data: &dc_sink) -> dc_stream_state {
    let caps = &dc_sink_data.edid_caps;
    let mut stream = dc_stream_state {
        // dc_sink_retain
        sink: Some(Box::new(dc_sink_data.clone())),
        link: li,
        sink_patches: caps.panel_patch,
        timing: dc_crtc_timing::default(),
        src: rect::default(),
        dst: rect::default(),
        out_transfer_func: dc_transfer_func::default(),
        output_color_space: 0,
        dither_option: 0,
        view_format: 0,
        audio_info: audio_info::default(),
        qs_bit: caps.qs_bit,
        qy_bit: caps.qy_bit,
        converter_disable_audio: dc_sink_data.converter_disable_audio,
        signal: 0,
        phy_pix_clk: 0,
        apply_seamless_boot_optimization: false,
        clamping: Default::default(),
        bit_depth_params: Default::default(),
        stream_id: 0,
        use_dynamic_meta: false,
        adjust: Default::default(),
        writeback_info: Default::default(),
        ignore_msa_timing_param: false,
        adaptive_sync_infopacket_valid: false,
        refcount: 0,
    };

    // Copy audio modes
    for i in 0..caps.audio_mode_count as usize {
        stream.audio_info.modes[i].channel_count = caps.audio_modes[i].channel_count;
        stream.audio_info.modes[i].format_code = caps.audio_modes[i].format_code as u32;
        stream.audio_info.modes[i].sample_rates = caps.audio_modes[i].sample_rate;
        stream.audio_info.modes[i].sample_size = caps.audio_modes[i].sample_size;
    }
    stream.audio_info.mode_count = caps.audio_mode_count;
    stream.audio_info.audio_latency = caps.audio_latency;
    stream.audio_info.video_latency = caps.video_latency;
    stream.audio_info.display_name = caps.display_name;
    stream.audio_info.manufacture_id = caps.manufacturer_id as u32;
    stream.audio_info.product_id = caps.product_id as u32;
    stream.audio_info.flags = caps.speaker_flags as u32;

    // TODO - WindowDM has implemented,
    // other DMs need Unhardcode port_id
    stream.audio_info.port_id[0] = 0x5558859e;
    stream.audio_info.port_id[1] = 0xd989449;

    // EDID CAP translation for HDMI 2.0
    stream.timing.flags.LTE_340MCSC_SCRAMBLE = caps.lte_340mcsc_scramble;

    stream.timing.dsc_cfg = dc_dsc_config::default();
    stream.timing.dsc_cfg.num_slices_h = 0;
    stream.timing.dsc_cfg.num_slices_v = 0;
    stream.timing.dsc_cfg.bits_per_pixel = 128;
    stream.timing.dsc_cfg.block_pred_enable = true;
    stream.timing.dsc_cfg.linebuf_depth = 9;
    stream.timing.dsc_cfg.version_minor = 2;
    stream.timing.dsc_cfg.ycbcr422_simple = false;

    update_stream_signal(dc, &mut stream, dc_sink_data);

    stream.out_transfer_func.r#type = TF_TYPE_BYPASS;

    dc_stream_assign_stream_id(dc, &mut stream);

    stream
}

/// `dc_create_stream_for_sink`: a stream for link `li`'s local sink, in
/// dc's arena with one reference (the caller's).
pub fn dc_create_stream_for_sink(dc: &mut dc, li: usize) -> Option<usize> {
    let sink = dc.links[li].local_sink.as_deref()?.clone();

    let mut stream = dc_stream_construct(dc, li, &sink);

    // kref_init
    stream.refcount = 1;

    dc.streams.push(Some(Box::new(stream)));
    Some(dc.streams.len() - 1)
}

/// `dc_stream_retain`.
pub fn dc_stream_retain(dc: &mut dc, stream: usize) {
    dc.streams[stream].as_deref_mut().unwrap().refcount += 1;
}

/// `dc_plane_construct` (dc_surface.c).
fn dc_plane_construct(plane_state: &mut dc_plane_state) {
    plane_state.gamma_correction_is_identity = true;

    plane_state.in_transfer_func.r#type = TF_TYPE_BYPASS;

    plane_state.in_shaper_func.r#type = TF_TYPE_BYPASS;

    plane_state.lut3d_state = 0;

    plane_state.blend_tf.r#type = TF_TYPE_BYPASS;

    plane_state.pre_multiplied_alpha = true;
}

/// `dc_create_plane_state` (dc_surface.c): in dc's arena, one reference.
pub fn dc_create_plane_state(dc: &mut dc) -> usize {
    let mut plane_state = dc_plane_state { refcount: 1, ..Default::default() };

    dc_plane_construct(&mut plane_state);

    dc.planes.push(Some(Box::new(plane_state)));
    dc.planes.len() - 1
}

/// `dc_plane_state_retain` (dc_surface.c).
pub fn dc_plane_state_retain(dc: &mut dc, plane: usize) {
    dc.planes[plane].as_deref_mut().unwrap().refcount += 1;
}
