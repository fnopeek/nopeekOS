//! drivers/gpu/drm/drm_rect.c + include/drm/drm_rect.h (Linux rpi-6.18):
//! rectangles, clipped and scaled in 16.16 fixed point for sources.

use crate::linux::{EINVAL, ERANGE};
use crate::regs::*;

/// `struct drm_rect`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct drm_rect {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

/// `drm_rect_width`.
pub fn drm_rect_width(r: &drm_rect) -> i32 {
    r.x2.wrapping_sub(r.x1)
}

/// `drm_rect_height`.
pub fn drm_rect_height(r: &drm_rect) -> i32 {
    r.y2.wrapping_sub(r.y1)
}

/// `drm_rect_visible`.
pub fn drm_rect_visible(r: &drm_rect) -> bool {
    drm_rect_width(r) > 0 && drm_rect_height(r) > 0
}

/// `drm_rect_equals`.
pub fn drm_rect_equals(r1: &drm_rect, r2: &drm_rect) -> bool {
    r1 == r2
}

/// `drm_rect_intersect`.
pub fn drm_rect_intersect(r1: &mut drm_rect, r2: &drm_rect) -> bool {
    r1.x1 = r1.x1.max(r2.x1);
    r1.y1 = r1.y1.max(r2.y1);
    r1.x2 = r1.x2.min(r2.x2);
    r1.y2 = r1.y2.min(r2.y2);

    drm_rect_visible(r1)
}

/// `clip_scaled`.
fn clip_scaled(src: i32, dst: i32, clip: &mut i32) -> u32 {
    if dst == 0 {
        return 0;
    }

    // Only clip what we have. Keeps the result bounded.
    *clip = (*clip).min(dst);

    let tmp = src as u32 as u64 * (dst - *clip) as u32 as u64;

    // Round toward 1.0 when clipping so that we don't accidentally
    // change upscaling to downscaling or vice versa.
    if src < (dst << 16) {
        tmp.div_ceil(dst as u64) as u32
    } else {
        (tmp / dst as u64) as u32
    }
}

/// `drm_rect_clip_scaled`: `dst` clipped to `clip`, `src` (16.16) by the
/// same proportion. Is `dst` still visible?
pub fn drm_rect_clip_scaled(src: &mut drm_rect, dst: &mut drm_rect, clip: &drm_rect) -> bool {
    let mut diff = clip.x1 - dst.x1;
    if diff > 0 {
        let new_src_w = clip_scaled(drm_rect_width(src), drm_rect_width(dst), &mut diff);

        src.x1 = src.x2.wrapping_sub(new_src_w as i32);
        dst.x1 += diff;
    }
    diff = clip.y1 - dst.y1;
    if diff > 0 {
        let new_src_h = clip_scaled(drm_rect_height(src), drm_rect_height(dst), &mut diff);

        src.y1 = src.y2.wrapping_sub(new_src_h as i32);
        dst.y1 += diff;
    }
    diff = dst.x2 - clip.x2;
    if diff > 0 {
        let new_src_w = clip_scaled(drm_rect_width(src), drm_rect_width(dst), &mut diff);

        src.x2 = src.x1.wrapping_add(new_src_w as i32);
        dst.x2 -= diff;
    }
    diff = dst.y2 - clip.y2;
    if diff > 0 {
        let new_src_h = clip_scaled(drm_rect_height(src), drm_rect_height(dst), &mut diff);

        src.y2 = src.y1.wrapping_add(new_src_h as i32);
        dst.y2 -= diff;
    }

    drm_rect_visible(dst)
}

/// `drm_calc_scale`.
fn drm_calc_scale(src: i32, dst: i32) -> i32 {
    if src < 0 || dst < 0 {
        return -EINVAL;
    }

    if dst == 0 {
        return 0;
    }

    if src as i64 > (dst as i64) << 16 {
        return ((src as i64 + dst as i64 - 1) / dst as i64) as i32;
    }

    src / dst
}

/// `drm_rect_calc_hscale`.
pub fn drm_rect_calc_hscale(src: &drm_rect, dst: &drm_rect, min_hscale: i32, max_hscale: i32) -> i32 {
    let src_w = drm_rect_width(src);
    let dst_w = drm_rect_width(dst);
    let hscale = drm_calc_scale(src_w, dst_w);

    if hscale < 0 || dst_w == 0 {
        return hscale;
    }

    if hscale < min_hscale || hscale > max_hscale {
        return -ERANGE;
    }

    hscale
}

/// `drm_rect_calc_vscale`.
pub fn drm_rect_calc_vscale(src: &drm_rect, dst: &drm_rect, min_vscale: i32, max_vscale: i32) -> i32 {
    let src_h = drm_rect_height(src);
    let dst_h = drm_rect_height(dst);
    let vscale = drm_calc_scale(src_h, dst_h);

    if vscale < 0 || dst_h == 0 {
        return vscale;
    }

    if vscale < min_vscale || vscale > max_vscale {
        return -ERANGE;
    }

    vscale
}

/// `drm_rect_rotate`: `r` inside a `width` x `height` area, rotated.
pub fn drm_rect_rotate(r: &mut drm_rect, width: i32, height: i32, rotation: u32) {
    if rotation & (DRM_MODE_REFLECT_X | DRM_MODE_REFLECT_Y) != 0 {
        let tmp = *r;

        if rotation & DRM_MODE_REFLECT_X != 0 {
            r.x1 = width - tmp.x2;
            r.x2 = width - tmp.x1;
        }

        if rotation & DRM_MODE_REFLECT_Y != 0 {
            r.y1 = height - tmp.y2;
            r.y2 = height - tmp.y1;
        }
    }

    let tmp = *r;
    match rotation & DRM_MODE_ROTATE_MASK {
        DRM_MODE_ROTATE_90 => {
            r.x1 = tmp.y1;
            r.x2 = tmp.y2;
            r.y1 = width - tmp.x2;
            r.y2 = width - tmp.x1;
        }
        DRM_MODE_ROTATE_180 => {
            r.x1 = width - tmp.x2;
            r.x2 = width - tmp.x1;
            r.y1 = height - tmp.y2;
            r.y2 = height - tmp.y1;
        }
        DRM_MODE_ROTATE_270 => {
            r.x1 = height - tmp.y2;
            r.x2 = height - tmp.y1;
            r.y1 = tmp.x1;
            r.y2 = tmp.x2;
        }
        _ => {}
    }
}

/// `drm_rect_rotate_inv`: the inverse of [`drm_rect_rotate`].
pub fn drm_rect_rotate_inv(r: &mut drm_rect, width: i32, height: i32, rotation: u32) {
    let tmp = *r;
    match rotation & DRM_MODE_ROTATE_MASK {
        DRM_MODE_ROTATE_90 => {
            r.x1 = width - tmp.y2;
            r.x2 = width - tmp.y1;
            r.y1 = tmp.x1;
            r.y2 = tmp.x2;
        }
        DRM_MODE_ROTATE_180 => {
            r.x1 = width - tmp.x2;
            r.x2 = width - tmp.x1;
            r.y1 = height - tmp.y2;
            r.y2 = height - tmp.y1;
        }
        DRM_MODE_ROTATE_270 => {
            r.x1 = tmp.y1;
            r.x2 = tmp.y2;
            r.y1 = height - tmp.x2;
            r.y2 = height - tmp.x1;
        }
        _ => {}
    }

    if rotation & (DRM_MODE_REFLECT_X | DRM_MODE_REFLECT_Y) != 0 {
        let tmp = *r;

        if rotation & DRM_MODE_REFLECT_X != 0 {
            r.x1 = width - tmp.x2;
            r.x2 = width - tmp.x1;
        }

        if rotation & DRM_MODE_REFLECT_Y != 0 {
            r.y1 = height - tmp.y2;
            r.y2 = height - tmp.y1;
        }
    }
}
