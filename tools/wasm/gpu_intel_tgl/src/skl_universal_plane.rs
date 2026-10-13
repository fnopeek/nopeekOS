//! display/skl_universal_plane.c (Linux 6.18.55): the universal planes of a
//! display version 12 pipe as far as the readout needs them — identity,
//! FBC instance, enable state — and `skl_get_initial_plane_config`.
//!
//! Format and modifier lists, rotation and blend properties are DRM object
//! registration and are not kept.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_display::REG_FIELD_GET;
use crate::intel_display_power::*;
use crate::intel_display_types::*;
use crate::intel_fb;
use crate::intel_plane_initial::intel_initial_plane_config;
use crate::linux::Line;
use crate::regs::*;

/// `skl_fbc_id_for_pipe`.
fn skl_fbc_id_for_pipe(pipe: u32) -> u32 {
    pipe - PIPE_A + INTEL_FBC_A
}

/// `skl_plane_has_fbc` (display version below 20).
fn skl_plane_has_fbc(i915: &drm_i915_private, fbc_id: u32, plane_id: u32) -> bool {
    if i915.display.runtime.fbc_mask & (1 << fbc_id) == 0 {
        return false;
    }
    plane_id == PLANE_1
}

/// `INTEL_FRONTBUFFER`.
pub fn INTEL_FRONTBUFFER(pipe: u32, plane_id: u32) -> u32 {
    1 << (plane_id + INTEL_FRONTBUFFER_BITS_PER_PIPE * pipe)
}

/// `skl_universal_plane_create`: the hardware identity of the plane.
pub fn skl_universal_plane_create(i915: &drm_i915_private, pipe: u32, plane_id: u32) -> intel_plane {
    let fbc_id = skl_fbc_id_for_pipe(pipe);
    intel_plane {
        pipe,
        id: plane_id,
        i9xx_plane: 0,
        frontbuffer_bit: INTEL_FRONTBUFFER(pipe, plane_id),
        fbc_id: if skl_plane_has_fbc(i915, fbc_id, plane_id) { Some(fbc_id) } else { None },
        kind: plane_kind::skl_universal,
        state: intel_plane_state::default(),
    }
}

/// `skl_plane_get_hw_state`.
pub fn skl_plane_get_hw_state(i915: &mut drm_i915_private, plane: &intel_plane, pipe: &mut u32) -> bool {
    let power_domain = POWER_DOMAIN_PIPE(plane.pipe);
    if !intel_display_power_get_if_enabled(i915, power_domain) {
        return false;
    }

    let ret = intel_de_read(i915, PLANE_CTL(plane.pipe, plane.id)) & PLANE_CTL_ENABLE != 0;
    *pipe = plane.pipe;

    intel_display_power_put(i915, power_domain);
    ret
}

/// `skl_format_to_fourcc`.
pub fn skl_format_to_fourcc(format: u32, rgb_order: bool, alpha: bool) -> u32 {
    let pick = |abgr: u32, xbgr: u32, argb: u32, xrgb: u32| match (rgb_order, alpha) {
        (true, true) => abgr,
        (true, false) => xbgr,
        (false, true) => argb,
        (false, false) => xrgb,
    };
    match format {
        PLANE_CTL_FORMAT_RGB_565 => DRM_FORMAT_RGB565,
        PLANE_CTL_FORMAT_NV12 => DRM_FORMAT_NV12,
        PLANE_CTL_FORMAT_XYUV => DRM_FORMAT_XYUV8888,
        PLANE_CTL_FORMAT_P010 => DRM_FORMAT_P010,
        PLANE_CTL_FORMAT_P012 => DRM_FORMAT_P012,
        PLANE_CTL_FORMAT_P016 => DRM_FORMAT_P016,
        PLANE_CTL_FORMAT_Y210 => DRM_FORMAT_Y210,
        PLANE_CTL_FORMAT_Y212 => DRM_FORMAT_Y212,
        PLANE_CTL_FORMAT_Y216 => DRM_FORMAT_Y216,
        PLANE_CTL_FORMAT_Y410 => DRM_FORMAT_XVYU2101010,
        PLANE_CTL_FORMAT_Y412 => DRM_FORMAT_XVYU12_16161616,
        PLANE_CTL_FORMAT_Y416 => DRM_FORMAT_XVYU16161616,
        PLANE_CTL_FORMAT_XRGB_2101010 => {
            pick(DRM_FORMAT_ABGR2101010, DRM_FORMAT_XBGR2101010, DRM_FORMAT_ARGB2101010, DRM_FORMAT_XRGB2101010)
        }
        PLANE_CTL_FORMAT_XRGB_16161616F => {
            pick(DRM_FORMAT_ABGR16161616F, DRM_FORMAT_XBGR16161616F, DRM_FORMAT_ARGB16161616F,
                 DRM_FORMAT_XRGB16161616F)
        }
        // PLANE_CTL_FORMAT_XRGB_8888 and anything unknown
        _ => pick(DRM_FORMAT_ABGR8888, DRM_FORMAT_XBGR8888, DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888),
    }
}

/// `skl_plane_stride_mult` (no 90/270 rotation is read out).
fn skl_plane_stride_mult(modifier: u64, cpp: u32) -> u32 {
    // Linear buffers count in 64-byte chunks, tiled ones in tiles.
    if modifier == DRM_FORMAT_MOD_LINEAR {
        64
    } else {
        intel_fb::intel_tile_width_bytes(modifier, cpp)
    }
}

/// `skl_get_initial_plane_config` for display version 12, up to the
/// framebuffer description (`intel_framebuffer_alloc` and the GEM side
/// are not part of this).
pub fn skl_get_initial_plane_config(i915: &mut drm_i915_private, c: usize,
                                    plane_config: &mut intel_initial_plane_config) {
    let crtc_pipe = i915.display.crtcs[c].pipe;
    let Some(p) = crate::intel_crtc::primary_plane(i915, crtc_pipe) else {
        return;
    };
    let plane = i915.display.planes[p];
    let plane_id = plane.id;
    let mut pipe = PIPE_A;

    if !skl_plane_get_hw_state(i915, &plane, &mut pipe) {
        return;
    }
    if pipe != crtc_pipe {
        crate::linux::say("warning: primary plane on another pipe");
    }

    if i915.display.crtcs[c].state.joiner_pipes != 0 {
        Line::new().s("pipe ").dec(crtc_pipe as u64).s(": unsupported joiner configuration for initial FB").say();
        return;
    }

    let val = intel_de_read(i915, PLANE_CTL(pipe, plane_id));
    let pixel_format = val & PLANE_CTL_FORMAT_MASK_ICL;

    let color_ctl = intel_de_read(i915, PLANE_COLOR_CTL(pipe, plane_id));
    let alpha = REG_FIELD_GET(PLANE_COLOR_ALPHA_MASK, color_ctl);

    let fourcc = skl_format_to_fourcc(pixel_format, val & PLANE_CTL_ORDER_RGBX != 0, alpha != 0);
    let Some(cpp) = intel_fb::drm_format_info_cpp(fourcc) else {
        Line::new().s("no format info for fourcc ").hex(fourcc).say();
        return;
    };

    let tiling = val & PLANE_CTL_TILED_MASK;
    let modifier = match tiling {
        PLANE_CTL_TILED_LINEAR => DRM_FORMAT_MOD_LINEAR,
        PLANE_CTL_TILED_X => {
            plane_config.tiling = I915_TILING_X;
            I915_FORMAT_MOD_X_TILED
        }
        PLANE_CTL_TILED_Y => {
            plane_config.tiling = I915_TILING_Y;
            if val & PLANE_CTL_RENDER_DECOMPRESSION_ENABLE != 0 {
                I915_FORMAT_MOD_Y_TILED_GEN12_RC_CCS
            } else if val & PLANE_CTL_MEDIA_DECOMPRESSION_ENABLE != 0 {
                I915_FORMAT_MOD_Y_TILED_GEN12_MC_CCS
            } else {
                I915_FORMAT_MOD_Y_TILED
            }
        }
        // PLANE_CTL_TILED_YF; no 4-tiling before display version 13.
        PLANE_CTL_TILED_YF => {
            if val & PLANE_CTL_RENDER_DECOMPRESSION_ENABLE != 0 {
                I915_FORMAT_MOD_Yf_TILED_CCS
            } else {
                I915_FORMAT_MOD_Yf_TILED
            }
        }
        _ => {
            Line::new().s("MISSING_CASE tiling ").hex(tiling).say();
            return;
        }
    };

    // intel_fb_modifier_uses_dpt: no DPT before display version 13.

    // DRM_MODE_ROTATE_ is counter clockwise, the hardware clockwise.
    plane_config.rotation = match val & PLANE_CTL_ROTATE_MASK {
        PLANE_CTL_ROTATE_90 => DRM_MODE_ROTATE_270,
        PLANE_CTL_ROTATE_180 => DRM_MODE_ROTATE_180,
        PLANE_CTL_ROTATE_270 => DRM_MODE_ROTATE_90,
        _ => DRM_MODE_ROTATE_0,
    };
    if val & PLANE_CTL_FLIP_HORIZONTAL != 0 {
        plane_config.rotation |= DRM_MODE_REFLECT_X;
    }

    // 90/270 degree rotation would require extra work
    if plane_config.rotation & (DRM_MODE_ROTATE_90 | DRM_MODE_ROTATE_270) != 0 {
        return;
    }

    let base = intel_de_read(i915, PLANE_SURF(pipe, plane_id)) & PLANE_SURF_ADDR_MASK;
    plane_config.base = base;

    let offset = intel_de_read(i915, PLANE_OFFSET(pipe, plane_id));
    if offset != 0 {
        Line::new().s("warning: initial plane offset ").hex(offset).say();
    }

    let val = intel_de_read(i915, PLANE_SIZE(pipe, plane_id));
    let height = REG_FIELD_GET(PLANE_HEIGHT_MASK, val) + 1;
    let width = REG_FIELD_GET(PLANE_WIDTH_MASK, val) + 1;

    let val = intel_de_read(i915, PLANE_STRIDE(pipe, plane_id));
    let stride_mult = skl_plane_stride_mult(modifier, cpp);
    let pitch = REG_FIELD_GET(PLANE_STRIDE__MASK, val) * stride_mult;

    let aligned_height = intel_fb::intel_fb_align_height(modifier, cpp, height);
    plane_config.size = pitch * aligned_height;

    plane_config.fb = Some(crate::intel_plane_initial::intel_initial_fb {
        width,
        height,
        pitch,
        format: fourcc,
        cpp,
        modifier,
    });

    Line::new().s("pipe ").dec(crtc_pipe as u64).s(" plane ").dec(plane_id as u64 + 1).s(" with fb: size=")
        .dec(width as u64).s("x").dec(height as u64).s("@").dec(cpp as u64 * 8).s(", offset=").hex(base)
        .s(", pitch ").dec(pitch as u64).s(", size ").hex(plane_config.size).say();
}
