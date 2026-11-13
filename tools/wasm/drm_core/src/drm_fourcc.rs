//! drivers/gpu/drm/drm_fourcc.c + include/uapi/drm/drm_fourcc.h (Linux
//! rpi-6.18): what a pixel format is made of, and the Broadcom format
//! modifiers.

use crate::regs::*;

/// `struct drm_format_info`, the members drivers read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct drm_format_info {
    pub format: u32,
    pub num_planes: u32,
    pub cpp: [u32; 3],
    pub hsub: u32,
    pub vsub: u32,
    pub has_alpha: bool,
    pub is_yuv: bool,
}

/// `drm_format_info` (`__drm_format_info`): the format's entry in DRM's
/// table.
pub fn drm_format_info(format: u32) -> Option<drm_format_info> {
    drm_formats.iter().find(|f| f.0 == format).map(|f| drm_format_info {
        format: f.0,
        num_planes: f.1,
        cpp: [f.2, f.3, f.4],
        hsub: f.5,
        vsub: f.6,
        has_alpha: f.7 != 0,
        is_yuv: f.8 != 0,
    })
}

/// `fourcc_mod_code(vendor, val)`.
pub const fn fourcc_mod_code(vendor: u32, val: u64) -> u64 {
    ((vendor as u64) << 56) | (val & 0x00ff_ffff_ffff_ffff)
}

/// `DRM_FORMAT_MOD_BROADCOM_SAND128` / `SAND256` without parameter.
pub const DRM_FORMAT_MOD_BROADCOM_SAND128: u64 =
    fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_BROADCOM, DRM_FORMAT_MOD_BROADCOM_SAND128_CODE as u64);
pub const DRM_FORMAT_MOD_BROADCOM_SAND256: u64 =
    fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_BROADCOM, DRM_FORMAT_MOD_BROADCOM_SAND256_CODE as u64);

/// `fourcc_mod_broadcom_param`.
pub const fn fourcc_mod_broadcom_param(m: u64) -> i32 {
    ((m >> DRM_FORMAT_MOD_BROADCOM_PARAM_SHIFT) & ((1u64 << DRM_FORMAT_MOD_BROADCOM_PARAM_BITS) - 1)) as i32
}

/// `fourcc_mod_broadcom_mod`: the modifier without its parameter.
pub const fn fourcc_mod_broadcom_mod(m: u64) -> u64 {
    m & !(((1u64 << DRM_FORMAT_MOD_BROADCOM_PARAM_BITS) - 1) << DRM_FORMAT_MOD_BROADCOM_PARAM_SHIFT)
}
