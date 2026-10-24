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
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct plane_size {
    pub surface_size: rect,
    pub chroma_size: rect,
    pub surface_pitch: i32,
    pub chroma_pitch: i32,
}

/// `union dc_tiling_info`'s gfx9 view (DCN 1.x–3.x).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
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

/// `struct dc_plane_address`. The anonymous union of its views (`grph`,
/// `grph_stereo`, `video_progressive`, …) is `u`, its 64-bit members in
/// memory order; the accessors name the views' members, which alias as in C.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_plane_address {
    /// `enum dc_plane_addr_type`.
    pub r#type: u32,
    pub tmz_surface: bool,
    pub u: [u64; 12],
    pub page_table_base: u64,
    pub vmid: u32,
}

impl dc_plane_address {
    /// `grph.addr`.
    pub fn grph_addr(&self) -> u64 { self.u[0] }
    /// `grph.meta_addr`.
    pub fn grph_meta_addr(&self) -> u64 { self.u[2] }
    /// `grph_stereo.left_addr`.
    pub fn left_addr(&self) -> u64 { self.u[0] }
    /// `grph_stereo.left_meta_addr`.
    pub fn left_meta_addr(&self) -> u64 { self.u[1] }
    /// `grph_stereo.right_addr`.
    pub fn right_addr(&self) -> u64 { self.u[3] }
    /// `grph_stereo.right_meta_addr`.
    pub fn right_meta_addr(&self) -> u64 { self.u[4] }
    /// `video_progressive.luma_addr`.
    pub fn luma_addr(&self) -> u64 { self.u[0] }
    /// `video_progressive.luma_meta_addr`.
    pub fn luma_meta_addr(&self) -> u64 { self.u[1] }
    /// `video_progressive.chroma_addr`.
    pub fn chroma_addr(&self) -> u64 { self.u[3] }
    /// `video_progressive.chroma_meta_addr`.
    pub fn chroma_meta_addr(&self) -> u64 { self.u[4] }
}

/// `PHYSICAL_ADDRESS_LOC.low_part` / `.high_part` of a quad part.
pub fn low_part(quad: u64) -> u32 { quad as u32 }
pub fn high_part(quad: u64) -> u32 { (quad >> 32) as u32 }

/// `struct dc_csc_transform` (dc_hw_types.h).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_csc_transform {
    pub matrix: [u16; 12],
    pub enable_adjustment: bool,
}

/// `struct colorspace_transform` (dc_hw_types.h).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct colorspace_transform {
    pub matrix: [fixed31_32; 12],
    pub enable_remap: bool,
}

/// `struct dc_bias_and_scale` (dc_hw_types.h).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_bias_and_scale {
    pub scale_red: u32,
    pub bias_red: u32,
    pub scale_green: u32,
    pub bias_green: u32,
    pub scale_blue: u32,
    pub bias_blue: u32,
    pub bias_and_scale_valid: bool,
}

/// `struct sharpness_adj` (transform.h).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct sharpness_adj {
    pub horz: i32,
    pub vert: i32,
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
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct scaling_ratios {
    pub horz: fixed31_32,
    pub vert: fixed31_32,
    pub horz_c: fixed31_32,
    pub vert_c: fixed31_32,
}

/// `struct scl_inits`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct scl_inits {
    pub h: fixed31_32,
    pub h_c: fixed31_32,
    pub v: fixed31_32,
    pub v_c: fixed31_32,
}

/// `struct line_buffer_params`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct line_buffer_params {
    pub alpha_en: bool,
    pub pixel_expan_mode: bool,
    pub interleave_en: bool,
    pub dynamic_pixel_depth: i32,
    /// `enum lb_pixel_depth`.
    pub depth: u32,
}

/// `struct scaler_data` (transform.h); `dscl_prog_data` is the SPL path's,
/// which DCN 2.1 does not take.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct scaler_data {
    pub h_active: i32,
    pub v_active: i32,
    pub taps: scaling_taps,
    pub viewport: rect,
    pub viewport_c: rect,
    pub recout: rect,
    pub ratios: scaling_ratios,
    pub inits: scl_inits,
    pub sharpness: sharpness_adj,
    /// `enum pixel_format`.
    pub format: u32,
    pub lb_params: line_buffer_params,
}

/// `struct dc_cursor_attributes` (dc_hw_types.h). `attribute_flags` is
/// `union dc_cursor_attribute_flags`'s value.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_cursor_attributes {
    /// `address.quad_part`.
    pub address: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    /// `enum dc_cursor_color_format`.
    pub color_format: u32,
    pub sdr_white_level: u32,
    /// `enum dc_rotation_angle`.
    pub rotation_angle: u32,
    pub attribute_flags: u32,
}

/// `union dc_cursor_attribute_flags` bits.
pub const DC_CURSOR_ENABLE_MAGNIFICATION: u32 = 1 << 0;
pub const DC_CURSOR_ENABLE_CURSOR_DEGAMMA: u32 = 1 << 7;

/// `struct dc_cursor_position` (dc_hw_types.h).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_cursor_position {
    pub x: u32,
    pub y: u32,
    pub x_hotspot: u32,
    pub y_hotspot: u32,
    pub enable: bool,
    /// Translate cursor x/y by the source rectangle for each plane.
    pub translate_by_source: bool,
}

/// `struct dc_cursor_mi_param` (dc_hw_types.h), `stream` by index.
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_cursor_mi_param {
    pub pixel_clk_khz: u32,
    pub ref_clk_khz: u32,
    pub viewport: rect,
    pub recout: rect,
    pub h_scale_ratio: fixed31_32,
    pub v_scale_ratio: fixed31_32,
    /// `enum dc_rotation_angle`.
    pub rotation: u32,
    pub mirror: bool,
    pub stream: usize,
}

/// `struct dpp_cursor_attributes` (dc_hw_types.h).
#[derive(Clone, Copy, Default, Debug)]
pub struct dpp_cursor_attributes {
    pub bias: i32,
    pub scale: i32,
}

/// `struct crtc_position` (inc/hw/timing_generator.h).
#[derive(Clone, Copy, Default, Debug)]
pub struct crtc_position {
    pub vertical_count: u32,
    pub horizontal_count: u32,
    pub nominal_vcount: u32,
}
