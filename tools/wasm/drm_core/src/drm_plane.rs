//! A plane's state and its framebuffer (include/drm/drm_plane.h,
//! drm_framebuffer.h) with the helpers that check them:
//! `drm_atomic_helper_check_plane_state` (drm_atomic_helper.c) and
//! `drm_rotation_simplify` (drm_blend.c), Linux rpi-6.18.

use crate::drm_fourcc::drm_format_info;
use crate::drm_modes::{drm_display_mode, drm_mode_get_hv_timing};
use crate::drm_rect::*;
use crate::linux::ERANGE;
use crate::regs::*;

/// `struct drm_framebuffer`. A plane's memory is a bus address the
/// display engine reads (`drm_fb_dma_get_gem_obj(fb, i)->dma_addr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct drm_framebuffer {
    pub format: drm_format_info,
    pub width: u32,
    pub height: u32,
    pub pitches: [u32; 4],
    pub offsets: [u32; 4],
    pub modifier: u64,
    pub dma_addr: [u64; 4],
}

/// `struct drm_plane_state`, the members the display paths read. The
/// CRTC is its index in the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct drm_plane_state {
    pub crtc: Option<u32>,
    pub fb: Option<drm_framebuffer>,
    pub crtc_x: i32,
    pub crtc_y: i32,
    pub crtc_w: u32,
    pub crtc_h: u32,
    /// 16.16 fixed point.
    pub src_x: u32,
    pub src_y: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub alpha: u16,
    pub pixel_blend_mode: u16,
    pub rotation: u32,
    pub zpos: u32,
    pub normalized_zpos: u32,
    /// `enum drm_color_encoding` / `enum drm_color_range`.
    pub color_encoding: u32,
    pub color_range: u32,
    /// `enum drm_scaling_filter`.
    pub scaling_filter: u32,
    pub chroma_siting_h: i32,
    pub chroma_siting_v: i32,
    pub src: drm_rect,
    pub dst: drm_rect,
    pub visible: bool,
}

impl Default for drm_plane_state {
    /// `__drm_atomic_helper_plane_state_reset`.
    fn default() -> Self {
        drm_plane_state {
            crtc: None,
            fb: None,
            crtc_x: 0,
            crtc_y: 0,
            crtc_w: 0,
            crtc_h: 0,
            src_x: 0,
            src_y: 0,
            src_w: 0,
            src_h: 0,
            alpha: DRM_BLEND_ALPHA_OPAQUE as u16,
            pixel_blend_mode: DRM_MODE_BLEND_PREMULTI as u16,
            rotation: DRM_MODE_ROTATE_0,
            zpos: 0,
            normalized_zpos: 0,
            color_encoding: DRM_COLOR_YCBCR_BT601,
            color_range: DRM_COLOR_YCBCR_LIMITED_RANGE,
            scaling_filter: DRM_SCALING_FILTER_DEFAULT,
            chroma_siting_h: 0,
            chroma_siting_v: 0,
            src: drm_rect::default(),
            dst: drm_rect::default(),
            visible: false,
        }
    }
}

/// `drm_plane_state_src`.
pub fn drm_plane_state_src(state: &drm_plane_state) -> drm_rect {
    drm_rect {
        x1: state.src_x as i32,
        y1: state.src_y as i32,
        x2: state.src_x.wrapping_add(state.src_w) as i32,
        y2: state.src_y.wrapping_add(state.src_h) as i32,
    }
}

/// `drm_plane_state_dest`.
pub fn drm_plane_state_dest(state: &drm_plane_state) -> drm_rect {
    drm_rect {
        x1: state.crtc_x,
        y1: state.crtc_y,
        x2: state.crtc_x.wrapping_add(state.crtc_w as i32),
        y2: state.crtc_y.wrapping_add(state.crtc_h as i32),
    }
}

/// `drm_rotation_simplify`: a rotation the plane does not support,
/// expressed through the ones it does.
pub fn drm_rotation_simplify(rotation: u32, supported_rotations: u32) -> u32 {
    let mut rotation = rotation;

    if rotation & !supported_rotations != 0 {
        rotation ^= DRM_MODE_REFLECT_X | DRM_MODE_REFLECT_Y;
        let r = rotation & DRM_MODE_ROTATE_MASK;
        let ffs = if r == 0 { 0 } else { r.trailing_zeros() + 1 };
        rotation = (rotation & DRM_MODE_REFLECT_MASK) | 1 << ((ffs + 1) % 4);
    }

    rotation
}

/// `drm_atomic_helper_check_plane_state`: `src` and `dst` computed and
/// clipped to the CRTC's mode, the scale checked. `crtc_enable` and
/// `crtc_mode` are the new CRTC state's.
pub fn drm_atomic_helper_check_plane_state(plane_state: &mut drm_plane_state, crtc_enable: bool,
                                           crtc_mode: &drm_display_mode, min_scale: i32, max_scale: i32,
                                           can_position: bool, can_update_disabled: bool) -> i32 {
    let rotation = plane_state.rotation;
    let mut clip = drm_rect::default();

    plane_state.src = drm_plane_state_src(plane_state);
    plane_state.dst = drm_plane_state_dest(plane_state);

    let Some(fb) = plane_state.fb else {
        plane_state.visible = false;
        return 0;
    };

    // crtc should only be NULL when disabling (i.e., !fb)
    if plane_state.crtc.is_none() {
        plane_state.visible = false;
        return 0;
    }

    if !crtc_enable && !can_update_disabled {
        return -crate::linux::EINVAL;
    }

    let (fw, fh) = ((fb.width << 16) as i32, (fb.height << 16) as i32);
    drm_rect_rotate(&mut plane_state.src, fw, fh, rotation);

    // Check scaling
    let hscale = drm_rect_calc_hscale(&plane_state.src, &plane_state.dst, min_scale, max_scale);
    let vscale = drm_rect_calc_vscale(&plane_state.src, &plane_state.dst, min_scale, max_scale);
    if hscale < 0 || vscale < 0 {
        return -ERANGE;
    }

    if crtc_enable {
        let (x2, y2) = drm_mode_get_hv_timing(crtc_mode);
        clip.x2 = x2;
        clip.y2 = y2;
    }

    let (mut src, mut dst) = (plane_state.src, plane_state.dst);
    plane_state.visible = drm_rect_clip_scaled(&mut src, &mut dst, &clip);
    plane_state.src = src;
    plane_state.dst = dst;

    drm_rect_rotate_inv(&mut plane_state.src, fw, fh, rotation);

    if !plane_state.visible {
        // Plane isn't visible; some drivers can handle this
        // so we just return success here.
        return 0;
    }

    if !can_position && !drm_rect_equals(&plane_state.dst, &clip) {
        return -crate::linux::EINVAL;
    }

    0
}
