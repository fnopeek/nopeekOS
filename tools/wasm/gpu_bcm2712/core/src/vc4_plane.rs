//! `drivers/gpu/drm/vc4/vc4_plane.c` (Linux rpi-6.18), the gen 6 paths: a
//! plane's display list entry (`vc6_plane_mode_set`), its line buffer
//! (LBM) and unified pixel memory (UPM), and its load on the HVS.
//!
//! A plane's state is built at check time and written into the CRTC's
//! dlist at flush time (`vc4_plane_write_dlist`).

use crate::hw::Hw;
use crate::linux::{field_prep, EINVAL, ENOMEM};
use crate::regs::*;
use crate::vc4_hvs::{ida_alloc_range, Gen, Hvs, MmRefcount};
use alloc::vec::Vec;
use drm_core::drm_fourcc::{
    fourcc_mod_broadcom_mod, fourcc_mod_broadcom_param, DRM_FORMAT_MOD_BROADCOM_SAND128,
    DRM_FORMAT_MOD_BROADCOM_SAND256,
};
use drm_core::drm_modes::{drm_display_mode, drm_mode_vrefresh};
use drm_core::drm_plane::{drm_atomic_helper_check_plane_state, drm_plane_state, drm_rotation_simplify};
use drm_core::regs::{
    DRM_BLEND_ALPHA_OPAQUE, DRM_COLOR_YCBCR_BT2020, DRM_COLOR_YCBCR_BT601, DRM_COLOR_YCBCR_FULL_RANGE,
    DRM_COLOR_YCBCR_LIMITED_RANGE, DRM_FORMAT_MOD_LINEAR, DRM_FORMAT_P030, DRM_FORMAT_YUV444, DRM_FORMAT_YVU444,
    DRM_MODE_BLEND_COVERAGE, DRM_MODE_BLEND_PIXEL_NONE, DRM_MODE_BLEND_PREMULTI, DRM_MODE_REFLECT_X,
    DRM_MODE_REFLECT_Y, DRM_MODE_ROTATE_0, DRM_SCALING_FILTER_NEAREST_NEIGHBOR,
};

/// `struct hvs_format`, the columns gen 6 reads.
#[derive(Clone, Copy, Debug)]
pub struct HvsFormat {
    pub drm: u32,
    pub hvs: u32,
    pub pixel_order_hvs5: u32,
    pub hvs6_swap_chroma_pointers: bool,
}

/// `vc4_get_hvs_format`.
pub fn vc4_get_hvs_format(drm_format: u32) -> Option<HvsFormat> {
    (0..HVS_FORMATS as usize).find(|&i| HVS_FORMAT_DRM[i] == drm_format).map(|i| HvsFormat {
        drm: HVS_FORMAT_DRM[i],
        hvs: HVS_FORMAT_HVS[i],
        pixel_order_hvs5: HVS_FORMAT_PIXEL_ORDER_HVS5[i],
        hvs6_swap_chroma_pointers: HVS_FORMAT_HVS6_SWAP_CHROMA_POINTERS[i] != 0,
    })
}

/// `struct vc4_plane_state`.
#[derive(Clone, Debug, Default)]
pub struct Vc4PlaneState {
    pub base: drm_plane_state,
    /// The entry; its length is `dlist_count`.
    pub dlist: Vec<u32>,
    pub dlist_initialized: bool,
    pub pos0_offset: u32,
    pub pos2_offset: u32,
    pub ptr0_offset: [u32; 3],
    pub lbm_offset: u32,
    pub crtc_x: i32,
    pub crtc_y: i32,
    pub crtc_w: u32,
    pub crtc_h: u32,
    pub src_x: u32,
    pub src_y: u32,
    pub src_w: [u32; 2],
    pub src_h: [u32; 2],
    pub x_scaling: [u32; 2],
    pub y_scaling: [u32; 2],
    pub is_unity: bool,
    pub is_yuv: bool,
    pub is_yuv444_unity: bool,
    pub vdownsample: u32,
    pub needs_bg_fill: bool,
    pub lbm_handle: u32,
    pub upm_handle: [u32; 4],
    pub upm_buffer_lines: u32,
    pub hvs_load: u64,
    pub membus_load: u64,
}

/// What a plane's check reads of its CRTC's new state.
pub struct PlaneCrtc<'a> {
    pub enable: bool,
    pub mode: &'a drm_display_mode,
    pub adjusted_mode: &'a drm_display_mode,
    /// `vc4_crtc_get_margins`: left, right, top, bottom.
    pub margins: [u32; 4],
}

/// `plane_enabled`.
fn plane_enabled(state: &drm_plane_state) -> bool {
    state.fb.is_some() && state.crtc.is_some()
}

impl Vc4PlaneState {
    /// `vc4_dlist_counter_increment`: a slot filled in later.
    fn dlist_counter_increment(&mut self) {
        self.dlist.push(0);
    }

    /// `vc4_dlist_write`.
    fn dlist_write(&mut self, val: u32) {
        self.dlist.push(val);
    }

    /// `vc4_plane_dlist_size`.
    pub fn dlist_size(&self) -> u32 {
        self.dlist.len() as u32
    }

    /// `vc4_plane_duplicate_state`: the copy holds its own references on
    /// the LBM and UPM space, and its dlist is built again.
    pub fn duplicate(&self, hvs: &mut Hvs) -> Self {
        let mut s = self.clone();

        for i in 0..4 {
            if s.upm_handle[i] != 0 {
                hvs.upm_refcounts[s.upm_handle[i] as usize].refcount += 1;
            }
        }
        if s.lbm_handle != 0 {
            hvs.lbm_refcounts[s.lbm_handle as usize].refcount += 1;
        }

        s.dlist_initialized = false;
        s.is_yuv444_unity = false;

        s
    }

    /// `vc4_plane_destroy_state`.
    pub fn destroy(self, hvs: &mut Hvs) {
        let mut s = self;
        vc4_plane_free_lbm(&mut s, hvs);
        vc6_plane_free_upm(&mut s, hvs);
    }

    /// `vc4_plane_write_dlist`: the entry at `dlist` in the HVS's memory.
    pub fn write_dlist(&self, hw: &mut impl Hw, hvs: &Hvs, dlist: u32) -> u32 {
        for (i, &w) in self.dlist.iter().enumerate() {
            hvs.dlist_write(hw, dlist + i as u32, w);
        }

        self.dlist.len() as u32
    }
}

/// `vc4_plane_release_lbm_ida` / `vc4_plane_release_upm_ida`.
fn release_ida(hvs_mm: &mut crate::drm_mm::DrmMm, refcounts: &mut [MmRefcount], handle: u32) {
    let refcount = &mut refcounts[handle as usize];

    hvs_mm.remove_node(refcount.node);
    refcount.node = Default::default();
    refcount.size = 0;
    refcount.allocated = false;
}

/// `vc4_plane_free_lbm`.
fn vc4_plane_free_lbm(s: &mut Vc4PlaneState, hvs: &mut Hvs) {
    let lbm_handle = s.lbm_handle;
    if lbm_handle == 0 {
        return;
    }

    let r = &mut hvs.lbm_refcounts[lbm_handle as usize];
    r.refcount -= 1;
    if r.refcount == 0 {
        release_ida(&mut hvs.lbm_mm, &mut hvs.lbm_refcounts, lbm_handle);
    }
    s.lbm_handle = 0;
}

/// `vc6_plane_free_upm`.
fn vc6_plane_free_upm(s: &mut Vc4PlaneState, hvs: &mut Hvs) {
    for i in 0..4 {
        let upm_handle = s.upm_handle[i];
        if upm_handle == 0 {
            continue;
        }

        let r = &mut hvs.upm_refcounts[upm_handle as usize];
        r.refcount -= 1;
        if r.refcount == 0 {
            release_ida(&mut hvs.upm_mm, &mut hvs.upm_refcounts, upm_handle);
        }
        s.upm_handle[i] = 0;
    }
}

/// `vc4_get_scaling_mode`: src in 16.16, dst in pixels.
fn vc4_get_scaling_mode(src: u32, dst: u32) -> u32 {
    if dst == src >> 16 {
        return VC4_SCALING_NONE;
    }

    if src <= 1 << 16 {
        VC4_SCALING_TPZ
    } else if 3 * dst >= 2 * (src >> 16) {
        VC4_SCALING_PPF
    } else {
        VC4_SCALING_TPZ
    }
}

/// `DIV_ROUND_CLOSEST` of unsigned operands.
fn div_round_closest(a: u32, b: u32) -> u32 {
    (a + b / 2) / b
}

/// `vc4_plane_margins_adj`: the destination scaled into the margins.
fn vc4_plane_margins_adj(s: &mut Vc4PlaneState, crtc: &PlaneCrtc) -> i32 {
    let [left, right, top, bottom] = crtc.margins;
    let (hdisplay, vdisplay) = (crtc.mode.hdisplay as u32, crtc.mode.vdisplay as u32);

    if left == 0 && right == 0 && top == 0 && bottom == 0 {
        return 0;
    }

    if left + right >= hdisplay || top + bottom >= vdisplay {
        return -EINVAL;
    }

    let adjhdisplay = hdisplay - (left + right);
    s.crtc_x = div_round_closest(s.crtc_x as u32 * adjhdisplay, hdisplay) as i32;
    s.crtc_x += left as i32;
    if s.crtc_x > (hdisplay - right) as i32 {
        s.crtc_x = (hdisplay - right) as i32;
    }

    let adjvdisplay = vdisplay - (top + bottom);
    s.crtc_y = div_round_closest(s.crtc_y as u32 * adjvdisplay, vdisplay) as i32;
    s.crtc_y += top as i32;
    if s.crtc_y > (vdisplay - bottom) as i32 {
        s.crtc_y = (vdisplay - bottom) as i32;
    }

    s.crtc_w = div_round_closest(s.crtc_w * adjhdisplay, hdisplay);
    s.crtc_h = div_round_closest(s.crtc_h * adjvdisplay, vdisplay);

    if s.crtc_w == 0 || s.crtc_h == 0 {
        return -EINVAL;
    }

    0
}

/// `vc4_plane_setup_clipping_and_scaling`.
fn vc4_plane_setup_clipping_and_scaling(s: &mut Vc4PlaneState, crtc: &PlaneCrtc) -> i32 {
    let fb = s.base.fb.expect("plane_enabled");
    let num_planes = fb.format.num_planes;
    let (h_subsample, v_subsample) = (fb.format.hsub, fb.format.vsub);

    let ret = drm_atomic_helper_check_plane_state(&mut s.base, crtc.enable, crtc.mode, 1, i32::MAX, true, true);
    if ret != 0 {
        return ret;
    }

    s.src_x = s.base.src.x1 as u32;
    s.src_y = s.base.src.y1 as u32;
    s.src_w[0] = (s.base.src.x2 as u32).wrapping_sub(s.src_x);
    s.src_h[0] = (s.base.src.y2 as u32).wrapping_sub(s.src_y);

    s.crtc_x = s.base.dst.x1;
    s.crtc_y = s.base.dst.y1;
    s.crtc_w = (s.base.dst.x2 - s.base.dst.x1) as u32;
    s.crtc_h = (s.base.dst.y2 - s.base.dst.y1) as u32;

    let ret = vc4_plane_margins_adj(s, crtc);
    if ret != 0 {
        return ret;
    }

    s.x_scaling[0] = vc4_get_scaling_mode(s.src_w[0], s.crtc_w);
    s.y_scaling[0] = vc4_get_scaling_mode(s.src_h[0], s.crtc_h);

    s.is_unity = s.x_scaling[0] == VC4_SCALING_NONE && s.y_scaling[0] == VC4_SCALING_NONE;

    if num_planes > 1 {
        s.is_yuv = true;

        s.src_w[1] = s.src_w[0] / h_subsample;
        s.src_h[1] = s.src_h[0] / v_subsample;

        s.x_scaling[1] = vc4_get_scaling_mode(s.src_w[1], s.crtc_w);
        s.y_scaling[1] = vc4_get_scaling_mode(s.src_h[1], s.crtc_h);

        if s.x_scaling[1] == VC4_SCALING_NONE {
            s.x_scaling[1] = VC4_SCALING_PPF;
        }

        if s.y_scaling[1] == VC4_SCALING_NONE {
            s.y_scaling[1] = VC4_SCALING_PPF;
        }
    } else {
        s.is_yuv = false;
        s.x_scaling[1] = VC4_SCALING_NONE;
        s.y_scaling[1] = VC4_SCALING_NONE;
    }

    0
}

/// `vc4_write_tpz`.
fn vc4_write_tpz(s: &mut Vc4PlaneState, src: u32, dst: u32) {
    let (scale, recip);

    if (dst << 16) < src {
        scale = src / dst;

        recip = !0u32 / scale;
    } else {
        scale = (1 << 16) + 1;
        recip = (1 << 16) - 1;
    }

    s.dlist_write(field_prep(SCALER_TPZ0_SCALE_MASK, scale) | field_prep(SCALER_TPZ0_IPHASE_MASK, 0));
    s.dlist_write(field_prep(SCALER_TPZ1_RECIP_MASK, recip));
}

/// `vc4_write_ppf`.
#[allow(clippy::too_many_arguments)]
fn vc4_write_ppf(s: &mut Vc4PlaneState, src: u32, dst: u32, xy: u32, channel: usize, subsample: u32,
    chroma_offset: i32, no_interpolate: bool)
{
    let sub_shift = if subsample == 2 { 1 } else { 0 };
    let mut scale = src / dst;

    if channel == 0 {
        scale &= !1;
    }

    let mut offset = ((xy & ((0x10000 << sub_shift) - 1)) >> (16 - PHASE_BITS)) as i32;
    offset -= chroma_offset >> (16 - PHASE_BITS);
    offset -= 1 << PHASE_BITS >> 1;
    offset >>= sub_shift;

    offset += (scale >> (17 - PHASE_BITS)) as i32;

    let mut offset2 = src.wrapping_sub(dst.wrapping_mul(scale)) as i32;
    offset2 >>= 16 - PHASE_BITS;
    let mut phase = offset + (offset2 >> 1);

    if phase >= 1 << PHASE_BITS {
        phase = (1 << PHASE_BITS) - 1;
    }

    let phase = phase as u32 & (SCALER_PPF_IPHASE_MASK >> SCALER_PPF_IPHASE_MASK.trailing_zeros());

    s.dlist_write(
        (if no_interpolate { SCALER_PPF_NOINTERP } else { 0 })
            | SCALER_PPF_AGC
            | field_prep(SCALER_PPF_SCALE_MASK, scale)
            | field_prep(SCALER_PPF_IPHASE_MASK, phase),
    );
}

/// `vc4_write_scaling_parameters`.
fn vc4_write_scaling_parameters(s: &mut Vc4PlaneState, channel: usize) {
    let info = s.base.fb.expect("plane_enabled").format;
    let mut no_interpolate = s.base.scaling_filter == DRM_SCALING_FILTER_NEAREST_NEIGHBOR;
    let hsub = if channel != 0 { info.hsub } else { 1 };
    let vsub = if channel != 0 { info.vsub } else { 1 };
    let siting_h = if hsub > 1 { s.base.chroma_siting_h } else { 0 };
    let siting_v = if vsub > 1 { s.base.chroma_siting_v } else { 0 };

    if s.is_yuv444_unity {
        no_interpolate = true;
    }

    if s.x_scaling[channel] == VC4_SCALING_PPF {
        let (src, dst, xy) = (s.src_w[channel], s.crtc_w, s.src_x);
        vc4_write_ppf(s, src, dst, xy, channel, hsub, siting_h, no_interpolate);
    }

    if s.y_scaling[channel] == VC4_SCALING_PPF {
        let (src, dst, xy) = (s.src_h[channel], s.crtc_h, s.src_y);
        vc4_write_ppf(s, src, dst, xy, channel, vsub, siting_v, no_interpolate);
        s.dlist_write(0xc0c0c0c0);
    }

    if s.x_scaling[channel] == VC4_SCALING_TPZ {
        let (src, dst) = (s.src_w[channel], s.crtc_w);
        vc4_write_tpz(s, src, dst);
    }

    if s.y_scaling[channel] == VC4_SCALING_TPZ {
        let (src, dst) = (s.src_h[channel] / s.vdownsample, s.crtc_h);
        vc4_write_tpz(s, src, dst);
        s.dlist_write(0xc0c0c0c0);
    }
}

/// `vc4_get_scl_field`.
fn vc4_get_scl_field(s: &Vc4PlaneState, plane: usize) -> u32 {
    const N: u32 = VC4_SCALING_NONE;
    const T: u32 = VC4_SCALING_TPZ;
    const P: u32 = VC4_SCALING_PPF;

    match (s.x_scaling[plane], s.y_scaling[plane]) {
        (P, P) => SCALER_CTL0_SCL_H_PPF_V_PPF,
        (T, P) => SCALER_CTL0_SCL_H_TPZ_V_PPF,
        (P, T) => SCALER_CTL0_SCL_H_PPF_V_TPZ,
        (T, T) => SCALER_CTL0_SCL_H_TPZ_V_TPZ,
        (P, N) => SCALER_CTL0_SCL_H_PPF_V_NONE,
        (N, P) => SCALER_CTL0_SCL_H_NONE_V_PPF,
        (N, T) => SCALER_CTL0_SCL_H_NONE_V_TPZ,
        (T, N) => SCALER_CTL0_SCL_H_TPZ_V_NONE,
        _ => 0,
    }
}

/// `vc4_hvs6_get_alpha_mask_mode`.
fn vc4_hvs6_get_alpha_mask_mode(s: &Vc4PlaneState, generation: Gen) -> u32 {
    let has_alpha = s.base.fb.expect("plane_enabled").format.has_alpha;

    if generation == Gen::Gen6D && (!has_alpha || s.base.pixel_blend_mode as u32 == DRM_MODE_BLEND_PIXEL_NONE) {
        return field_prep(SCALER6_CTL0_ALPHA_MASK_MASK, SCALER6D_CTL0_ALPHA_MASK_FIXED);
    }

    field_prep(SCALER6_CTL0_ALPHA_MASK_MASK, SCALER6_CTL0_ALPHA_MASK_NONE)
}

/// `vc6_plane_get_csc_mode`.
fn vc6_plane_get_csc_mode(s: &Vc4PlaneState, generation: Gen) -> u32 {
    let mut ret = 0;

    if s.is_yuv {
        let mut color_encoding = s.base.color_encoding;
        let mut color_range = s.base.color_range;

        if color_encoding > DRM_COLOR_YCBCR_BT2020 {
            color_encoding = DRM_COLOR_YCBCR_BT601;
        }
        if color_range > DRM_COLOR_YCBCR_FULL_RANGE {
            color_range = DRM_COLOR_YCBCR_LIMITED_RANGE;
        }

        if generation == Gen::Gen6C {
            ret |= SCALER6C_CTL2_CSC_ENABLE;
            ret |= field_prep(SCALER6C_CTL2_BRCM_CFC_CONTROL_MASK, color_encoding + color_range * 3);
        } else {
            ret |= SCALER6D_CTL2_CSC_ENABLE;
            ret |= field_prep(SCALER6D_CTL2_BRCM_CFC_CONTROL_MASK, color_encoding + color_range * 3);
        }
    }

    ret
}

/// `vc4_hvs5_get_alpha_blend_mode`.
fn vc4_hvs5_get_alpha_blend_mode(s: &Vc4PlaneState, generation: Gen) -> u32 {
    let has_alpha = s.base.fb.expect("plane_enabled").format.has_alpha;
    let blend = s.base.pixel_blend_mode as u32;

    match generation {
        Gen::Gen6C => {
            if !has_alpha {
                return field_prep(SCALER5_CTL2_ALPHA_MODE_MASK, SCALER5_CTL2_ALPHA_MODE_FIXED);
            }

            match blend {
                DRM_MODE_BLEND_PIXEL_NONE => field_prep(SCALER5_CTL2_ALPHA_MODE_MASK, SCALER5_CTL2_ALPHA_MODE_FIXED),
                DRM_MODE_BLEND_COVERAGE => field_prep(SCALER5_CTL2_ALPHA_MODE_MASK, SCALER5_CTL2_ALPHA_MODE_PIPELINE),
                _ => field_prep(SCALER5_CTL2_ALPHA_MODE_MASK, SCALER5_CTL2_ALPHA_MODE_PIPELINE)
                    | SCALER5_CTL2_ALPHA_PREMULT,
            }
        }
        Gen::Gen6D => {
            if has_alpha && blend == DRM_MODE_BLEND_PREMULTI {
                SCALER5_CTL2_ALPHA_PREMULT
            } else {
                0
            }
        }
    }
}

/// `vc6_get_plane_idx`.
fn vc6_get_plane_idx(format: &HvsFormat, plane: usize) -> usize {
    if plane == 0 || !format.hvs6_swap_chroma_pointers {
        return plane;
    }

    if plane == 1 { 2 } else { 1 }
}

/// `vc4_plane_calc_load`.
fn vc4_plane_calc_load(s: &mut Vc4PlaneState, crtc: &PlaneCrtc) {
    let fb = s.base.fb.expect("plane_enabled");
    let vrefresh = drm_mode_vrefresh(crtc.adjusted_mode) as u64;

    let hvs_load_shift = if s.x_scaling[0] != VC4_SCALING_NONE
        || s.x_scaling[1] != VC4_SCALING_NONE
        || s.y_scaling[0] != VC4_SCALING_NONE
        || s.y_scaling[1] != VC4_SCALING_NONE
    {
        1
    } else {
        2
    };

    s.membus_load = 0;
    s.hvs_load = 0;
    for i in 0..fb.format.num_planes as usize {
        let vscale_factor = (s.src_h[i] >> 16).div_ceil(s.crtc_h) as u64;
        s.membus_load +=
            (s.src_w[i] >> 16) as u64 * (s.src_h[i] >> 16) as u64 * vscale_factor * fb.format.cpp[i] as u64;
        s.hvs_load += s.crtc_h as u64 * s.crtc_w as u64;
    }

    s.hvs_load *= vrefresh;
    s.hvs_load >>= hvs_load_shift;
    s.membus_load *= vrefresh;
}

/// `vc6_plane_mode_set`: the plane's dlist entry.
fn vc6_plane_mode_set(s: &mut Vc4PlaneState, hvs: &Hvs, crtc: &PlaneCrtc) -> i32 {
    let fb = s.base.fb.expect("plane_enabled");
    let Some(format) = vc4_get_hvs_format(fb.format.format) else { return -EINVAL };
    let base_format_mod = fourcc_mod_broadcom_mod(fb.modifier);
    let num_planes = fb.format.num_planes as usize;
    let (h_subsample, v_subsample) = (fb.format.hsub, fb.format.vsub);
    let mut pitch = [0u32; 2];
    let mut hvs_format = format.hvs;
    let mut offsets = [0u32; 3];
    let tiling;

    if s.dlist_initialized {
        return 0;
    }

    let ret = vc4_plane_setup_clipping_and_scaling(s, crtc);
    if ret != 0 {
        return ret;
    }

    if fb.format.format == DRM_FORMAT_YUV444 || fb.format.format == DRM_FORMAT_YVU444 {
        // Similar to YUV422 requiring the chroma scaler to always be
        // enabled, GEN6 requires the scaler for the luma channel to be
        // enabled for YUV444.
        if s.x_scaling[0] == VC4_SCALING_NONE {
            s.x_scaling[0] = VC4_SCALING_PPF;
            s.is_unity = false;
            s.is_yuv444_unity = true;
        }

        if s.y_scaling[0] == VC4_SCALING_NONE {
            s.y_scaling[0] = VC4_SCALING_PPF;
            s.is_unity = false;
        } else {
            // Ensure that resizing vertically but not horizontally
            // doesn't switch the scaling filter.
            s.is_yuv444_unity = false;
        }
    }

    if s.src_w[0] == 0 || s.src_h[0] == 0 || s.crtc_w == 0 || s.crtc_h == 0 {
        // 0 source size probably means the plane is offscreen.
        // 0 destination size is a redundant plane.
        s.dlist_initialized = true;
        return 0;
    }

    let mut width = s.src_w[0] >> 16;
    let mut height = s.src_h[0] >> 16;

    if s.y_scaling[0] == VC4_SCALING_TPZ && s.src_h[0] / s.crtc_h > (3 << 16) {
        // Downscaling by more than x3. Reduce the number of lines read
        // to avoid exceeding SDRAM bandwidth.
        s.vdownsample = ((s.src_h[0] / (s.crtc_h * 3)) >> 16) + 1;
    } else {
        s.vdownsample = 1;
    }

    // SCL1 is used for Cb/Cr scaling of planar formats. For RGB
    // and 4:4:4, scl1 should be set to scl0 so both channels of
    // the scaler do the same thing. For YUV, the Y plane needs
    // to be put in channel 1 and Cb/Cr in channel 0, so we swap
    // the scl fields here.
    let (scl0, scl1) = if num_planes == 1 {
        let scl0 = vc4_get_scl_field(s, 0);
        (scl0, scl0)
    } else {
        (vc4_get_scl_field(s, 1), vc4_get_scl_field(s, 0))
    };

    let rotation = drm_rotation_simplify(s.base.rotation, DRM_MODE_ROTATE_0 | DRM_MODE_REFLECT_X | DRM_MODE_REFLECT_Y);

    // We must point to the last line when Y reflection is enabled.
    let mut src_y = s.src_y >> 16;
    if rotation & DRM_MODE_REFLECT_Y != 0 {
        src_y += height - 1;
    }

    let src_x = s.src_x >> 16;

    // fetch an extra pixel if we don't actually line up with the left edge.
    if s.src_x & 0xffff != 0 && s.src_x < fb.width << 16 {
        width += 1;
    }

    // same for the right side
    if (s.src_x + s.src_w[0]) & 0xffff != 0 && s.src_x + s.src_w[0] < fb.width << 16 {
        width += 1;
    }

    // now for the top
    if s.src_y & 0xffff != 0 && s.src_y < fb.height << 16 {
        height += 1;
    }

    // and the bottom
    if (s.src_y + s.src_h[0]) & 0xffff != 0 && s.src_y + s.src_h[0] < fb.height << 16 {
        height += 1;
    }

    if base_format_mod == DRM_FORMAT_MOD_LINEAR as u64 {
        tiling = SCALER6_CTL0_ADDR_MODE_LINEAR;

        // Adjust the base pointer to the first pixel to be scanned
        // out.
        for i in 0..num_planes {
            offsets[i] += src_y / if i != 0 { v_subsample } else { 1 } * fb.pitches[i];
            offsets[i] += src_x / if i != 0 { h_subsample } else { 1 } * fb.format.cpp[i];
        }
    } else if base_format_mod == DRM_FORMAT_MOD_BROADCOM_SAND128 || base_format_mod == DRM_FORMAT_MOD_BROADCOM_SAND256
    {
        let param = fourcc_mod_broadcom_param(fb.modifier) as u32;
        let tile_width;

        if param > SCALER_TILE_HEIGHT_MASK >> SCALER_TILE_HEIGHT_MASK.trailing_zeros() {
            return -EINVAL;
        }

        if fb.format.format == DRM_FORMAT_P030 {
            hvs_format = HVS_PIXEL_FORMAT_YCBCR_10BIT;
            tiling = SCALER6_CTL0_ADDR_MODE_128B;
            tile_width = 128;
        } else {
            hvs_format = HVS_PIXEL_FORMAT_YCBCR_YUV420_2PLANE;

            if base_format_mod == DRM_FORMAT_MOD_BROADCOM_SAND128 {
                tiling = SCALER6_CTL0_ADDR_MODE_128B;
                tile_width = 128;
            } else {
                tiling = SCALER6_CTL0_ADDR_MODE_256B;
                tile_width = 256;
            }
        }

        let components_per_word = if fb.format.format == DRM_FORMAT_P030 { 24 } else { 32 };
        let starting_offset = src_x % components_per_word;
        let fetch_count = (width + starting_offset + components_per_word - 1) / components_per_word;

        // Adjust the base pointer to the first pixel to be scanned
        // out. For P030, [31:4] is the 128-bit word of the start pixel
        // and [3:0] the pixel (0-11) within it.
        for i in 0..num_planes {
            pitch[i] = match param {
                0 => fb.pitches[i],
                _ => field_prep(SCALER_TILE_HEIGHT_MASK, param),
            };

            let (x_off, pix_per_tile);
            if fb.format.format == DRM_FORMAT_P030 {
                // Bits [31:5] of the address point to the 256-bit word
                // containing the starting pixel, bits [4:0] say which
                // of its 24 pixels comes first.
                let remaining_pixels = src_x % 96;
                let aligned = remaining_pixels / 24;
                let last_bits = remaining_pixels % 24;

                x_off = aligned * 32 + last_bits;
                pix_per_tile = 96;
            } else {
                pix_per_tile = tile_width / fb.format.cpp[0];
                x_off = (src_x % pix_per_tile) / if i != 0 { h_subsample } else { 1 } * fb.format.cpp[i];
            }

            let tile = src_x / pix_per_tile;

            offsets[i] += pitch[i] * tile * tile_width;
            offsets[i] += src_y / if i != 0 { v_subsample } else { 1 } * tile_width;
            offsets[i] += x_off & !(if i != 0 { 1 } else { 0 });

            // Finished using the pitch as a pitch, so pack it as the
            // register value.
            pitch[i] = field_prep(SCALER6_PTR2_TILE_HEIGHT_MASK, pitch[i])
                | field_prep(SCALER6_PTR2_TILE_FETCH_COUNT_MASK, fetch_count - 1)
                | field_prep(SCALER6_PTR2_TILE_LSKIP_MASK, s.vdownsample - 1);
        }
    } else {
        return -EINVAL;
    }

    // for YUV444 hardware wants double the width, otherwise it doesn't
    // fetch full width of chroma
    if format.drm == DRM_FORMAT_YUV444 || format.drm == DRM_FORMAT_YVU444 {
        width <<= 1;
    }

    // Don't waste cycles mixing with plane alpha if the set alpha
    // is opaque or there is no per-pixel alpha information.
    // In any case we use the alpha property value as the fixed alpha.
    let mix_plane_alpha = s.base.alpha as u32 != DRM_BLEND_ALPHA_OPAQUE
        && fb.format.has_alpha
        && s.base.pixel_blend_mode as u32 != DRM_MODE_BLEND_PIXEL_NONE;

    // Control Word 0: Scaling Configuration & Element Validity
    let w = SCALER6_CTL0_VALID
        | field_prep(SCALER6_CTL0_ADDR_MODE_MASK, tiling)
        | vc4_hvs6_get_alpha_mask_mode(s, hvs.generation)
        | (if s.is_unity { SCALER6_CTL0_UNITY } else { 0 })
        | field_prep(SCALER6_CTL0_ORDERRGBA_MASK, format.pixel_order_hvs5)
        | field_prep(SCALER6_CTL0_SCL1_MODE_MASK, scl1)
        | field_prep(SCALER6_CTL0_SCL0_MODE_MASK, scl0)
        | field_prep(SCALER6_CTL0_PIXEL_FORMAT_MASK, hvs_format);
    s.dlist_write(w);

    // Position Word 0: Image Position
    s.pos0_offset = s.dlist.len() as u32;
    s.dlist_write(
        field_prep(SCALER6_POS0_START_Y_MASK, s.crtc_y as u32)
            | (if rotation & DRM_MODE_REFLECT_X != 0 { SCALER6_POS0_HFLIP } else { 0 })
            | field_prep(SCALER6_POS0_START_X_MASK, s.crtc_x as u32),
    );

    // Control Word 2: Alpha Value & CSC
    let w = vc6_plane_get_csc_mode(s, hvs.generation)
        | vc4_hvs5_get_alpha_blend_mode(s, hvs.generation)
        | (if mix_plane_alpha { SCALER6_CTL2_ALPHA_MIX } else { 0 })
        | field_prep(SCALER5_CTL2_ALPHA_MASK, s.base.alpha as u32 >> 4);
    s.dlist_write(w);

    // Position Word 1: Scaled Image Dimensions
    if !s.is_unity {
        s.dlist_write(
            field_prep(SCALER6_POS1_SCL_LINES_MASK, s.crtc_h - 1) | field_prep(SCALER6_POS1_SCL_WIDTH_MASK, s.crtc_w - 1),
        );
    }

    // Position Word 2: Source Image Size
    s.pos2_offset = s.dlist.len() as u32;
    s.dlist_write(
        field_prep(SCALER6_POS2_SRC_LINES_MASK, height / s.vdownsample - 1)
            | field_prep(SCALER6_POS2_SRC_WIDTH_MASK, width - 1),
    );

    // Position Word 3: Context
    s.dlist_write(0xc0c0c0c0);

    // This only covers Raster Scan Order planes.
    for i in 0..num_planes {
        let idx = vc6_get_plane_idx(&format, i);
        let paddr = fb.dma_addr[idx] + fb.offsets[idx] as u64 + offsets[idx] as u64;

        // Pointer Word 0; the UPM buffer is allocated in
        // vc6_plane_allocate_upm().
        s.ptr0_offset[i] = s.dlist.len() as u32;
        s.dlist_write(
            (if rotation & DRM_MODE_REFLECT_Y != 0 { SCALER6_PTR0_VFLIP } else { 0 })
                | field_prep(SCALER6_PTR0_UPPER_ADDR_MASK, (paddr >> 32) as u32 & 0xff),
        );

        // Pointer Word 1
        s.dlist_write(paddr as u32);

        // Pointer Word 2
        if base_format_mod != DRM_FORMAT_MOD_BROADCOM_SAND128 && base_format_mod != DRM_FORMAT_MOD_BROADCOM_SAND256 {
            s.dlist_write(field_prep(SCALER6_PTR2_PITCH_MASK, fb.pitches[i] * s.vdownsample));
        } else {
            s.dlist_write(pitch[i]);
        }
    }

    // No palette word and no trans word: neither mode is used.

    s.lbm_offset = 0;

    if !s.is_unity || fb.format.is_yuv {
        // Reserve a slot for the LBM Base Address. The real value will
        // be set when calling vc4_plane_allocate_lbm().
        if s.y_scaling[0] != VC4_SCALING_NONE || s.y_scaling[1] != VC4_SCALING_NONE {
            s.lbm_offset = s.dlist.len() as u32;
            s.dlist_counter_increment();
        }

        if s.x_scaling[0] != VC4_SCALING_NONE
            || s.x_scaling[1] != VC4_SCALING_NONE
            || s.y_scaling[0] != VC4_SCALING_NONE
            || s.y_scaling[1] != VC4_SCALING_NONE
        {
            if num_planes > 1 {
                // Emit Cb/Cr as channel 0 and Y as channel 1. This
                // matches how we set up scl0/scl1 above.
                vc4_write_scaling_parameters(s, 1);
            }

            vc4_write_scaling_parameters(s, 0);
        }

        // If any PPF setup was done, then all the kernel pointers get
        // uploaded.
        if s.x_scaling[0] == VC4_SCALING_PPF
            || s.y_scaling[0] == VC4_SCALING_PPF
            || s.x_scaling[1] == VC4_SCALING_PPF
            || s.y_scaling[1] == VC4_SCALING_PPF
        {
            let mut filter = if s.base.scaling_filter == DRM_SCALING_FILTER_NEAREST_NEIGHBOR {
                hvs.nearest_neighbour_filter
            } else {
                hvs.mitchell_netravali_filter
            };
            if s.is_yuv444_unity {
                filter = hvs.nearest_neighbour_filter;
            }

            let kernel = field_prep(SCALER_PPF_KERNEL_OFFSET_MASK, filter.start as u32);

            // HPPF plane 0
            s.dlist_write(kernel);
            // VPPF plane 0
            s.dlist_write(kernel);
            // HPPF plane 1
            s.dlist_write(kernel);
            // VPPF plane 1
            s.dlist_write(kernel);
        }
    }

    s.dlist_write(SCALER6_CTL0_END);

    let count = s.dlist.len() as u32;
    s.dlist[0] |= field_prep(SCALER6_CTL0_NEXT_MASK, count);

    // crtc_* are already clipped coordinates.
    let covers_screen = s.crtc_x == 0
        && s.crtc_y == 0
        && s.crtc_w == crtc.mode.hdisplay as u32
        && s.crtc_h == crtc.mode.vdisplay as u32;

    // Background fill might be necessary when the plane has per-pixel
    // alpha content or a non-opaque plane alpha and could blend from the
    // background or does not cover the entire screen.
    s.needs_bg_fill = fb.format.has_alpha || !covers_screen || s.base.alpha as u32 != DRM_BLEND_ALPHA_OPAQUE;

    // Flag the dlist as initialized to avoid checking it twice.
    s.dlist_initialized = true;

    vc4_plane_calc_load(s, crtc);

    0
}

/// `vc4_lbm_words_per_component`.
fn vc4_lbm_words_per_component(s: &Vc4PlaneState, channel: usize) -> u32 {
    match s.y_scaling[channel] {
        VC4_SCALING_PPF => 4,
        VC4_SCALING_TPZ => 2,
        _ => 0,
    }
}

/// `vc4_lbm_components`.
fn vc4_lbm_components(s: &Vc4PlaneState, channel: usize) -> u32 {
    let info = s.base.fb.expect("plane_enabled").format;

    if s.y_scaling[channel] == VC4_SCALING_NONE {
        return 0;
    }

    if info.is_yuv {
        return if channel != 0 { 2 } else { 1 };
    }

    if s.y_scaling[channel] == VC4_SCALING_TPZ {
        return 4;
    }

    if info.has_alpha && s.base.alpha as u32 == DRM_BLEND_ALPHA_OPAQUE {
        return 4;
    }

    3
}

/// `vc4_lbm_channel_size`: from the plane state's own `src_w` and
/// `crtc_w` (not the clipped ones).
fn vc4_lbm_channel_size(s: &Vc4PlaneState, channel: usize) -> u32 {
    let info = s.base.fb.expect("plane_enabled").format;

    // LBM is meant to use the smaller of source or dest width, but there
    // is a issue with UV scaling that the size required for the second
    // channel is based on the source width only.
    let width = if info.hsub > 1 && channel == 1 {
        s.base.src_w >> 16
    } else {
        (s.base.src_w >> 16).min(s.base.crtc_w)
    };
    let width = (width / info.hsub).next_multiple_of(4);

    let wpc = vc4_lbm_words_per_component(s, channel);
    if wpc == 0 {
        return 0;
    }

    let components = vc4_lbm_components(s, channel);
    if components == 0 {
        return 0;
    }

    let words = width * wpc * components;

    words.div_ceil(128 / info.hsub)
}

/// `vc4_lbm_size` (`__vc6_lbm_size`): no LBM without vertical scaling.
fn vc4_lbm_size(s: &Vc4PlaneState) -> u32 {
    if s.y_scaling[0] == VC4_SCALING_NONE && s.y_scaling[1] == VC4_SCALING_NONE {
        return 0;
    }

    let info = s.base.fb.expect("plane_enabled").format;
    if info.hsub > 1 {
        vc4_lbm_channel_size(s, 0).max(vc4_lbm_channel_size(s, 1))
    } else {
        vc4_lbm_channel_size(s, 0)
    }
}

/// `vc4_plane_allocate_lbm`.
fn vc4_plane_allocate_lbm(s: &mut Vc4PlaneState, hvs: &mut Hvs) -> i32 {
    let lbm_size = vc4_lbm_size(s);
    if lbm_size == 0 {
        return 0;
    }

    if s.lbm_offset == 0 {
        return -EINVAL;
    }

    let mut lbm_handle = s.lbm_handle;
    if lbm_handle != 0 && hvs.lbm_refcounts[lbm_handle as usize].size == lbm_size {
        s.lbm_handle = lbm_handle;
    } else {
        if lbm_handle != 0 {
            let r = &mut hvs.lbm_refcounts[lbm_handle as usize];
            r.refcount -= 1;
            if r.refcount == 0 {
                release_ida(&mut hvs.lbm_mm, &mut hvs.lbm_refcounts, lbm_handle);
                s.lbm_handle = 0;
            }
        }

        let Some(h) = ida_alloc_range(&mut hvs.lbm_refcounts, VC4_NUM_LBM_HANDLES) else {
            drm_core::linux::Line::new().s("Out of lbm_handles").say();
            return -ENOMEM;
        };
        lbm_handle = h;
        s.lbm_handle = lbm_handle;

        match hvs.lbm_mm.insert_node_generic(lbm_size as u64, 1) {
            Ok(node) => {
                let r = &mut hvs.lbm_refcounts[lbm_handle as usize];
                r.refcount = 1;
                r.size = lbm_size;
                r.node = node;
            }
            Err(ret) => {
                drm_core::linux::Line::new().s("Failed to allocate LBM entry").say();
                let r = &mut hvs.lbm_refcounts[lbm_handle as usize];
                r.refcount = 0;
                r.allocated = false;
                s.lbm_handle = 0;
                return ret;
            }
        }
    }

    s.dlist[s.lbm_offset as usize] = hvs.lbm_refcounts[lbm_handle as usize].node.start as u32;

    0
}

/// `vc6_upm_size`.
fn vc6_upm_size(s: &Vc4PlaneState, plane: usize) -> u32 {
    let fb = s.base.fb.expect("plane_enabled");
    let mut stride = fb.pitches[plane];
    let base_format_mod = fourcc_mod_broadcom_mod(fb.modifier);

    if base_format_mod == DRM_FORMAT_MOD_BROADCOM_SAND128 {
        if fb.format.format == DRM_FORMAT_P030 {
            stride = (fb.width.next_multiple_of(96) * 4) / 3;
        } else {
            stride = fb.width.next_multiple_of(128);
        }
    }
    if plane == 0 && (fb.format.format == DRM_FORMAT_YUV444 || fb.format.format == DRM_FORMAT_YVU444) {
        stride <<= 1;
    }
    // This only works for raster formats, and is sub-optimal for buffers
    // with a stride aligned on 32 bytes.
    let words_per_line = (stride + 62) / 32;
    let fetch_region_size = words_per_line * 32;
    let buffer_lines = 2 << s.upm_buffer_lines;
    let buffer_size = fetch_region_size * buffer_lines;

    buffer_size.next_multiple_of(HVS_UBM_WORD_SIZE)
}

/// `vc6_plane_allocate_upm`.
fn vc6_plane_allocate_upm(s: &mut Vc4PlaneState, hvs: &mut Hvs) -> i32 {
    let num_planes = s.base.fb.expect("plane_enabled").format.num_planes as usize;

    s.upm_buffer_lines = SCALER6_PTR0_UPM_BUFF_SIZE_2_LINES;

    for i in 0..num_planes {
        let upm_size = vc6_upm_size(s, i);
        if upm_size == 0 {
            return -EINVAL;
        }
        let mut upm_handle = s.upm_handle[i];

        if upm_handle != 0 && hvs.upm_refcounts[upm_handle as usize].size == upm_size {
            s.upm_handle[i] = upm_handle;
        } else {
            if upm_handle != 0 {
                let r = &mut hvs.upm_refcounts[upm_handle as usize];
                r.refcount -= 1;
                if r.refcount == 0 {
                    release_ida(&mut hvs.upm_mm, &mut hvs.upm_refcounts, upm_handle);
                    s.upm_handle[i] = 0;
                }
            }

            let Some(h) = ida_alloc_range(&mut hvs.upm_refcounts, VC4_NUM_UPM_HANDLES) else {
                return -ENOMEM;
            };
            upm_handle = h;
            s.upm_handle[i] = upm_handle;

            match hvs.upm_mm.insert_node_generic(upm_size as u64, HVS_UBM_WORD_SIZE as u64) {
                Ok(node) => {
                    let r = &mut hvs.upm_refcounts[upm_handle as usize];
                    r.refcount = 1;
                    r.size = upm_size;
                    r.node = node;
                }
                Err(ret) => {
                    drm_core::linux::Line::new().s("Failed to allocate UPM entry").say();
                    let r = &mut hvs.upm_refcounts[upm_handle as usize];
                    r.refcount = 0;
                    r.allocated = false;
                    s.upm_handle[i] = 0;
                    return ret;
                }
            }
        }

        let start = hvs.upm_refcounts[upm_handle as usize].node.start as u32;
        s.dlist[s.ptr0_offset[i] as usize] |= field_prep(SCALER6_PTR0_UPM_BASE_MASK, start / HVS_UBM_WORD_SIZE)
            | field_prep(SCALER6_PTR0_UPM_HANDLE_MASK, s.upm_handle[i] - 1)
            | field_prep(SCALER6_PTR0_UPM_BUFF_SIZE_MASK, s.upm_buffer_lines);
    }

    0
}

/// `vc4_plane_atomic_check`. `old_enabled`: the old state had a
/// framebuffer on a CRTC. `crtc` is the new CRTC state, when the plane
/// is on one.
pub fn vc4_plane_atomic_check(s: &mut Vc4PlaneState, old_enabled: bool, hvs: &mut Hvs, crtc: Option<&PlaneCrtc>)
    -> i32
{
    s.dlist.clear();

    if !plane_enabled(&s.base) {
        if old_enabled {
            vc6_plane_free_upm(s, hvs);
            vc4_plane_free_lbm(s, hvs);
        }
        return 0;
    }

    let crtc = crtc.expect("an enabled plane has a CRTC");
    let ret = vc6_plane_mode_set(s, hvs, crtc);
    if ret != 0 {
        return ret;
    }

    if s.src_w[0] == 0 || s.src_h[0] == 0 || s.crtc_w == 0 || s.crtc_h == 0 {
        return 0;
    }

    let ret = vc4_plane_allocate_lbm(s, hvs);
    if ret != 0 {
        return ret;
    }

    vc6_plane_allocate_upm(s, hvs)
}
