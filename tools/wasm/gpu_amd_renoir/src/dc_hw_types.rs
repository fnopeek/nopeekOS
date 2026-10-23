//! display/dc/dc_hw_types.h (Linux 6.18.55): timings, rectangles and the
//! surface description DC programs. Bitfield flag unions are structs of
//! bools; enums are their values as Linux numbers them.
#![allow(dead_code)]

use crate::fixpt31_32::fixed31_32;

/// `struct rect`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// `struct dc_crtc_timing_flags`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_crtc_timing_flags {
    pub INTERLACE: bool,
    pub HSYNC_POSITIVE_POLARITY: bool,
    pub VSYNC_POSITIVE_POLARITY: bool,
    pub HORZ_COUNT_BY_TWO: bool,
    pub EXCLUSIVE_3D: bool,
    pub RIGHT_EYE_3D_POLARITY: bool,
    pub SUB_SAMPLE_3D: bool,
    pub USE_IN_3D_VIEW_ONLY: bool,
    pub STEREO_3D_PREFERENCE: bool,
    pub Y_ONLY: bool,
    pub YCBCR420: bool,
    pub DTD_COUNTER: u8,
    pub FORCE_HDR: bool,
    pub LTE_340MCSC_SCRAMBLE: bool,
    pub DSC: bool,
    pub VBLANK_SYNCHRONIZABLE: bool,
}

/// `struct dc_dsc_config`; `rc_params_ovrd` is a DM override the port has
/// none of.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_dsc_config {
    pub num_slices_h: u32,
    pub num_slices_v: u32,
    pub bits_per_pixel: u32,
    pub block_pred_enable: bool,
    pub linebuf_depth: u32,
    pub version_minor: u32,
    pub ycbcr422_simple: bool,
    pub rc_buffer_size: i32,
    pub is_frl: bool,
    pub is_dp: bool,
    pub mst_pbn: u32,
}

/// `struct dc_crtc_timing`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_crtc_timing {
    pub h_total: u32,
    pub h_border_left: u32,
    pub h_addressable: u32,
    pub h_border_right: u32,
    pub h_front_porch: u32,
    pub h_sync_width: u32,
    pub v_total: u32,
    pub v_border_top: u32,
    pub v_addressable: u32,
    pub v_border_bottom: u32,
    pub v_front_porch: u32,
    pub v_sync_width: u32,
    pub pix_clk_100hz: u32,
    pub min_refresh_in_uhz: u32,
    pub max_refresh_in_uhz: u32,
    pub vic: u32,
    pub hdmi_vic: u32,
    pub rid: u32,
    pub fr_index: u32,
    pub frl_uncompressed_video_bandwidth_in_kbps: u32,
    /// `enum dc_timing_3d_format`.
    pub timing_3d_format: u32,
    /// `enum dc_color_depth`.
    pub display_color_depth: u32,
    /// `enum dc_pixel_encoding`.
    pub pixel_encoding: u32,
    /// `enum dc_aspect_ratio`.
    pub aspect_ratio: u32,
    /// `enum scanning_type`.
    pub scan_type: u32,
    pub flags: dc_crtc_timing_flags,
    pub dsc_fixed_bits_per_pixel_x16: u32,
    pub dsc_cfg: dc_dsc_config,
    pub expanded_hblank: u32,
}

/// `struct plane_size`: the graphics or video surface's size and pitch.
#[derive(Clone, Copy, Default, Debug)]
pub struct plane_size {
    pub surface_size: rect,
    pub chroma_size: rect,
    pub surface_pitch: i32,
    pub chroma_pitch: i32,
}

/// `union dc_tiling_info`'s gfx9 view (DCN 1.x–3.x).
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_tiling_info_gfx9 {
    pub num_pipes: u32,
    pub num_banks: u32,
    pub pipe_interleave: u32,
    pub num_shader_engines: u32,
    pub num_rb_per_se: u32,
    pub max_compressed_frags: u32,
    pub shaderEnable: bool,
    /// `enum swizzle_mode_values`.
    pub swizzle: u32,
    pub meta_linear: bool,
    pub rb_aligned: bool,
    pub pipe_aligned: bool,
    pub num_pkrs: u32,
}

/// `struct dc_plane_address`, the graphics view (`PLN_ADDR_TYPE_GRAPHICS`).
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_plane_address {
    /// `enum dc_plane_addr_type`.
    pub r#type: u32,
    pub tmz_surface: bool,
    pub grph_addr: u64,
    pub grph_meta_addr: u64,
    pub vmid: u32,
}

/// `struct scaling_taps`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct scaling_taps {
    pub v_taps: u32,
    pub h_taps: u32,
    pub v_taps_c: u32,
    pub h_taps_c: u32,
    pub integer_scaling: bool,
}

/// `struct scaling_ratios`.
#[derive(Clone, Copy, Default, Debug)]
pub struct scaling_ratios {
    pub horz: fixed31_32,
    pub vert: fixed31_32,
    pub horz_c: fixed31_32,
    pub vert_c: fixed31_32,
}

/// `struct scl_inits`.
#[derive(Clone, Copy, Default, Debug)]
pub struct scl_inits {
    pub h: fixed31_32,
    pub h_c: fixed31_32,
    pub v: fixed31_32,
    pub v_c: fixed31_32,
}

/// `struct line_buffer_params`.
#[derive(Clone, Copy, Default, Debug)]
pub struct line_buffer_params {
    pub alpha_en: bool,
    pub pixel_expan_mode: bool,
    pub interleave_en: bool,
    pub dynamic_pixel_depth: i32,
    /// `enum lb_pixel_depth`.
    pub depth: u32,
}

/// `struct scaler_data` (transform.h); `sharpness` and `dscl_prog_data` are
/// the SPL path's, which DCN 2.1 does not take.
#[derive(Clone, Copy, Default, Debug)]
pub struct scaler_data {
    pub h_active: i32,
    pub v_active: i32,
    pub taps: scaling_taps,
    pub viewport: rect,
    pub viewport_c: rect,
    pub recout: rect,
    pub ratios: scaling_ratios,
    pub inits: scl_inits,
    /// `enum pixel_format`.
    pub format: u32,
    pub lb_params: line_buffer_params,
}
