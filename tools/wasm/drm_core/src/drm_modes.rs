//! drivers/gpu/drm/drm_modes.c + include/drm/drm_modes.h (Linux rpi-6.18):
//! a display mode, its CVT and GTF formulas, matching, validation and
//! sorting. A mode list is a `Vec` in list order; C's u16 timing fields
//! keep their width, and arithmetic stays in C's types.

use crate::regs::*;
use alloc::string::String;
use core::fmt::Write;

/// `enum drm_mode_status`.
pub type drm_mode_status = i32;

/// `struct drm_display_mode`, without the list head.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct drm_display_mode {
    pub clock: i32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub flags: u32,
    pub crtc_clock: i32,
    pub crtc_hdisplay: u16,
    pub crtc_hblank_start: u16,
    pub crtc_hblank_end: u16,
    pub crtc_hsync_start: u16,
    pub crtc_hsync_end: u16,
    pub crtc_htotal: u16,
    pub crtc_hskew: u16,
    pub crtc_vdisplay: u16,
    pub crtc_vblank_start: u16,
    pub crtc_vblank_end: u16,
    pub crtc_vsync_start: u16,
    pub crtc_vsync_end: u16,
    pub crtc_vtotal: u16,
    pub width_mm: u16,
    pub height_mm: u16,
    pub r#type: u8,
    pub expose_to_userspace: bool,
    /// `char name[DRM_DISPLAY_MODE_LEN]`: at most 31 bytes.
    pub name: String,
    pub status: drm_mode_status,
    /// `enum hdmi_picture_aspect`.
    pub picture_aspect_ratio: u32,
}

/// A mode of Linux's static tables (regs.rs): clock, the ten timings,
/// flags, type, aspect ratio and name, as `DRM_MODE` fills them.
pub type table_mode = (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, &'static str);

/// The `drm_display_mode` a table entry initialises.
pub fn mode_from_table(t: &table_mode) -> drm_display_mode {
    drm_display_mode {
        clock: t.0 as i32,
        hdisplay: t.1 as u16,
        hsync_start: t.2 as u16,
        hsync_end: t.3 as u16,
        htotal: t.4 as u16,
        hskew: t.5 as u16,
        vdisplay: t.6 as u16,
        vsync_start: t.7 as u16,
        vsync_end: t.8 as u16,
        vtotal: t.9 as u16,
        vscan: t.10 as u16,
        flags: t.11,
        r#type: t.12 as u8,
        picture_aspect_ratio: t.13,
        name: String::from(t.14),
        ..Default::default()
    }
}

/// `KHZ2PICOS` (uapi/linux/fb.h), on an unsigned long.
pub fn KHZ2PICOS(a: i32) -> u64 {
    1_000_000_000u64 / (a as i64 as u64)
}

/// `DIV_ROUND_CLOSEST` for non-negative operands.
fn div_round_closest(x: u64, d: u64) -> u64 {
    (x + d / 2) / d
}

/// `drm_mode_set_name`.
pub fn drm_mode_set_name(mode: &mut drm_display_mode) {
    let interlaced = mode.flags & DRM_MODE_FLAG_INTERLACE != 0;
    let mut s = String::new();
    let _ = write!(s, "{}x{}{}", mode.hdisplay, mode.vdisplay, if interlaced { "i" } else { "" });
    s.truncate(DRM_DISPLAY_MODE_LEN as usize - 1);
    mode.name = s;
}

/// `drm_mode_vrefresh`.
pub fn drm_mode_vrefresh(mode: &drm_display_mode) -> i32 {
    let mut num: u32 = 1;
    let mut den: u32 = 1;

    if mode.htotal == 0 || mode.vtotal == 0 {
        return 0;
    }

    if mode.flags & DRM_MODE_FLAG_INTERLACE != 0 {
        num *= 2;
    }
    if mode.flags & DRM_MODE_FLAG_DBLSCAN != 0 {
        den *= 2;
    }
    if mode.vscan > 1 {
        den *= mode.vscan as u32;
    }

    // check_mul_overflow(int, unsigned int, unsigned int *)
    let Some(n) = (mode.clock as i64).checked_mul(num as i64).filter(|v| (0..=u32::MAX as i64).contains(v)) else {
        return 0;
    };
    num = n as u32;

    // htotal * vtotal is an int product of two u16.
    let hv = mode.htotal as i32 as i64 * mode.vtotal as i32 as i64;
    if hv > i32::MAX as i64 {
        return 0;
    }
    let Some(d) = hv.checked_mul(den as i64).filter(|v| (0..=u32::MAX as i64).contains(v)) else {
        return 0;
    };
    den = d as u32;

    div_round_closest(num as u64 * 1000, den as u64) as i32
}

/// `drm_mode_set_crtcinfo`.
pub fn drm_mode_set_crtcinfo(p: &mut drm_display_mode, adjust_flags: u32) {
    p.crtc_clock = p.clock;
    p.crtc_hdisplay = p.hdisplay;
    p.crtc_hsync_start = p.hsync_start;
    p.crtc_hsync_end = p.hsync_end;
    p.crtc_htotal = p.htotal;
    p.crtc_hskew = p.hskew;
    p.crtc_vdisplay = p.vdisplay;
    p.crtc_vsync_start = p.vsync_start;
    p.crtc_vsync_end = p.vsync_end;
    p.crtc_vtotal = p.vtotal;

    if p.flags & DRM_MODE_FLAG_INTERLACE != 0 && adjust_flags & CRTC_INTERLACE_HALVE_V != 0 {
        p.crtc_vdisplay /= 2;
        p.crtc_vsync_start /= 2;
        p.crtc_vsync_end /= 2;
        p.crtc_vtotal /= 2;
    }

    if adjust_flags & CRTC_NO_DBLSCAN == 0 && p.flags & DRM_MODE_FLAG_DBLSCAN != 0 {
        p.crtc_vdisplay = p.crtc_vdisplay.wrapping_mul(2);
        p.crtc_vsync_start = p.crtc_vsync_start.wrapping_mul(2);
        p.crtc_vsync_end = p.crtc_vsync_end.wrapping_mul(2);
        p.crtc_vtotal = p.crtc_vtotal.wrapping_mul(2);
    }

    if adjust_flags & CRTC_NO_VSCAN == 0 && p.vscan > 1 {
        p.crtc_vdisplay = p.crtc_vdisplay.wrapping_mul(p.vscan);
        p.crtc_vsync_start = p.crtc_vsync_start.wrapping_mul(p.vscan);
        p.crtc_vsync_end = p.crtc_vsync_end.wrapping_mul(p.vscan);
        p.crtc_vtotal = p.crtc_vtotal.wrapping_mul(p.vscan);
    }

    if adjust_flags & CRTC_STEREO_DOUBLE != 0 && p.flags & DRM_MODE_FLAG_3D_MASK == DRM_MODE_FLAG_3D_FRAME_PACKING {
        p.crtc_clock = p.crtc_clock.wrapping_mul(2);
        p.crtc_vdisplay = p.crtc_vdisplay.wrapping_add(p.crtc_vtotal);
        p.crtc_vsync_start = p.crtc_vsync_start.wrapping_add(p.crtc_vtotal);
        p.crtc_vsync_end = p.crtc_vsync_end.wrapping_add(p.crtc_vtotal);
        p.crtc_vtotal = p.crtc_vtotal.wrapping_add(p.crtc_vtotal);
    }

    p.crtc_vblank_start = p.crtc_vsync_start.min(p.crtc_vdisplay);
    p.crtc_vblank_end = p.crtc_vsync_end.max(p.crtc_vtotal);
    p.crtc_hblank_start = p.crtc_hsync_start.min(p.crtc_hdisplay);
    p.crtc_hblank_end = p.crtc_hsync_end.max(p.crtc_htotal);
}

/// `drm_mode_get_hv_timing`.
pub fn drm_mode_get_hv_timing(mode: &drm_display_mode) -> (i32, i32) {
    let mut adjusted = mode.clone();
    drm_mode_set_crtcinfo(&mut adjusted, CRTC_STEREO_DOUBLE_ONLY);
    (adjusted.crtc_hdisplay as i32, adjusted.crtc_vdisplay as i32)
}

fn drm_mode_match_timings(mode1: &drm_display_mode, mode2: &drm_display_mode) -> bool {
    mode1.hdisplay == mode2.hdisplay
        && mode1.hsync_start == mode2.hsync_start
        && mode1.hsync_end == mode2.hsync_end
        && mode1.htotal == mode2.htotal
        && mode1.hskew == mode2.hskew
        && mode1.vdisplay == mode2.vdisplay
        && mode1.vsync_start == mode2.vsync_start
        && mode1.vsync_end == mode2.vsync_end
        && mode1.vtotal == mode2.vtotal
        && mode1.vscan == mode2.vscan
}

fn drm_mode_match_clock(mode1: &drm_display_mode, mode2: &drm_display_mode) -> bool {
    // do clock check convert to PICOS so fb modes get matched the same
    if mode1.clock != 0 && mode2.clock != 0 {
        KHZ2PICOS(mode1.clock) == KHZ2PICOS(mode2.clock)
    } else {
        mode1.clock == mode2.clock
    }
}

fn drm_mode_match_flags(mode1: &drm_display_mode, mode2: &drm_display_mode) -> bool {
    (mode1.flags & !DRM_MODE_FLAG_3D_MASK) == (mode2.flags & !DRM_MODE_FLAG_3D_MASK)
}

fn drm_mode_match_3d_flags(mode1: &drm_display_mode, mode2: &drm_display_mode) -> bool {
    (mode1.flags & DRM_MODE_FLAG_3D_MASK) == (mode2.flags & DRM_MODE_FLAG_3D_MASK)
}

fn drm_mode_match_aspect_ratio(mode1: &drm_display_mode, mode2: &drm_display_mode) -> bool {
    mode1.picture_aspect_ratio == mode2.picture_aspect_ratio
}

/// `drm_mode_match`.
pub fn drm_mode_match(mode1: &drm_display_mode, mode2: &drm_display_mode, match_flags: u32) -> bool {
    if match_flags & DRM_MODE_MATCH_TIMINGS != 0 && !drm_mode_match_timings(mode1, mode2) {
        return false;
    }
    if match_flags & DRM_MODE_MATCH_CLOCK != 0 && !drm_mode_match_clock(mode1, mode2) {
        return false;
    }
    if match_flags & DRM_MODE_MATCH_FLAGS != 0 && !drm_mode_match_flags(mode1, mode2) {
        return false;
    }
    if match_flags & DRM_MODE_MATCH_3D_FLAGS != 0 && !drm_mode_match_3d_flags(mode1, mode2) {
        return false;
    }
    if match_flags & DRM_MODE_MATCH_ASPECT_RATIO != 0 && !drm_mode_match_aspect_ratio(mode1, mode2) {
        return false;
    }
    true
}

/// `drm_mode_equal`.
pub fn drm_mode_equal(mode1: &drm_display_mode, mode2: &drm_display_mode) -> bool {
    drm_mode_match(
        mode1,
        mode2,
        DRM_MODE_MATCH_TIMINGS
            | DRM_MODE_MATCH_CLOCK
            | DRM_MODE_MATCH_FLAGS
            | DRM_MODE_MATCH_3D_FLAGS
            | DRM_MODE_MATCH_ASPECT_RATIO,
    )
}

/// `drm_mode_equal_no_clocks`.
pub fn drm_mode_equal_no_clocks(mode1: &drm_display_mode, mode2: &drm_display_mode) -> bool {
    drm_mode_match(mode1, mode2, DRM_MODE_MATCH_TIMINGS | DRM_MODE_MATCH_FLAGS | DRM_MODE_MATCH_3D_FLAGS)
}

/// `drm_mode_equal_no_clocks_no_stereo`.
pub fn drm_mode_equal_no_clocks_no_stereo(mode1: &drm_display_mode, mode2: &drm_display_mode) -> bool {
    drm_mode_match(mode1, mode2, DRM_MODE_MATCH_TIMINGS | DRM_MODE_MATCH_FLAGS)
}

/// `drm_cvt_mode`. C's mix of int and unsigned int is kept: the divisions
/// with `hperiod` are unsigned. None where Linux returns NULL, and where
/// its unsigned divisor would be zero.
pub fn drm_cvt_mode(hdisplay: i32, vdisplay: i32, vrefresh: i32, reduced: bool, interlaced: bool,
                    margins: bool) -> Option<drm_display_mode> {
    const HV_FACTOR: i32 = 1000;
    // 1) top/bottom margin size (% of height) - default: 1.8,
    const CVT_MARGIN_PERCENTAGE: i32 = 18;
    // 2) character cell horizontal granularity (pixels) - default 8
    const CVT_H_GRANULARITY: i32 = 8;
    // 3) Minimum vertical porch (lines) - default 3
    const CVT_MIN_V_PORCH: i32 = 3;
    // 4) Minimum number of vertical back porch lines - default 6
    const CVT_MIN_V_BPORCH: i32 = 6;
    // Pixel Clock step (kHz)
    const CVT_CLOCK_STEP: i32 = 250;

    if hdisplay == 0 || vdisplay == 0 {
        return None;
    }

    let mut drm_mode = drm_display_mode::default();

    // the CVT default refresh rate is 60Hz
    let vrefresh = if vrefresh == 0 { 60 } else { vrefresh };

    // the required field fresh rate
    let vfieldrate: u32 = if interlaced { (vrefresh * 2) as u32 } else { vrefresh as u32 };

    // horizontal pixels
    let hdisplay_rnd = hdisplay - (hdisplay % CVT_H_GRANULARITY);

    // determine the left&right borders
    let mut hmargin = 0;
    if margins {
        hmargin = hdisplay_rnd * CVT_MARGIN_PERCENTAGE / 1000;
        hmargin -= hmargin % CVT_H_GRANULARITY;
    }
    // find the total active pixels
    drm_mode.hdisplay = (hdisplay_rnd + 2 * hmargin) as u16;

    // find the number of lines per field
    let vdisplay_rnd = if interlaced { vdisplay / 2 } else { vdisplay };

    // find the top & bottom borders
    let mut vmargin = 0;
    if margins {
        vmargin = vdisplay_rnd * CVT_MARGIN_PERCENTAGE / 1000;
    }

    drm_mode.vdisplay = (vdisplay + 2 * vmargin) as u16;

    // Interlaced
    let interlace = interlaced as i32;

    // Determine VSync Width from aspect ratio
    let vsync = if vdisplay % 3 == 0 && vdisplay * 4 / 3 == hdisplay {
        4
    } else if vdisplay % 9 == 0 && vdisplay * 16 / 9 == hdisplay {
        5
    } else if vdisplay % 10 == 0 && vdisplay * 16 / 10 == hdisplay {
        6
    } else if vdisplay % 4 == 0 && vdisplay * 5 / 4 == hdisplay {
        7
    } else if vdisplay % 9 == 0 && vdisplay * 15 / 9 == hdisplay {
        7
    } else {
        // custom
        10
    };

    let hperiod: u32;
    if !reduced {
        // simplify the GTF calculation
        // 4) Minimum time of vertical sync + back porch interval (µs) default 550.0
        const CVT_MIN_VSYNC_BP: i32 = 550;
        // 3) Nominal HSync width (% of line period) - default 8
        const CVT_HSYNC_PERCENTAGE: i32 = 8;
        // 5) Definition of Horizontal blanking time limitation
        // Gradient (%/kHz) - default 600
        const CVT_M_FACTOR: i32 = 600;
        // Offset (%) - default 40
        const CVT_C_FACTOR: i32 = 40;
        // Blanking time scaling factor - default 128
        const CVT_K_FACTOR: i32 = 128;
        // Scaling factor weighting - default 20
        const CVT_J_FACTOR: i32 = 20;
        const CVT_M_PRIME: i32 = CVT_M_FACTOR * CVT_K_FACTOR / 256;
        const CVT_C_PRIME: i32 = (CVT_C_FACTOR - CVT_J_FACTOR) * CVT_K_FACTOR / 256 + CVT_J_FACTOR;

        // estimated the horizontal period
        let tmp1 = ((HV_FACTOR * 1_000_000) as u32)
            .wrapping_sub(((CVT_MIN_VSYNC_BP * HV_FACTOR) as u32).wrapping_mul(vfieldrate));
        let tmp2 = (vdisplay_rnd + 2 * vmargin + CVT_MIN_V_PORCH) * 2 + interlace;
        let d = (tmp2 as u32).wrapping_mul(vfieldrate);
        if d == 0 {
            return None;
        }
        hperiod = (tmp1 as i32).wrapping_mul(2) as u32 / d;
        if hperiod == 0 {
            return None;
        }

        let tmp1 = ((CVT_MIN_VSYNC_BP * HV_FACTOR) as u32 / hperiod + 1) as i32;
        // 9. Find number of lines in sync + backporch
        let vsyncandback_porch = if tmp1 < vsync + CVT_MIN_V_PORCH { vsync + CVT_MIN_V_PORCH } else { tmp1 };
        // 10. Find number of lines in back porch (unused)
        drm_mode.vtotal =
            (vdisplay_rnd + 2 * vmargin).wrapping_add(vsyncandback_porch).wrapping_add(CVT_MIN_V_PORCH) as u16;
        // 12. Find ideal blanking duty cycle from formula
        let mut hblank_percentage =
            ((CVT_C_PRIME * HV_FACTOR) as u32).wrapping_sub((CVT_M_PRIME as u32).wrapping_mul(hperiod) / 1000);
        // 13. Blanking time
        if hblank_percentage < (20 * HV_FACTOR) as u32 {
            hblank_percentage = (20 * HV_FACTOR) as u32;
        }
        let d = ((100 * HV_FACTOR) as u32).wrapping_sub(hblank_percentage);
        if d == 0 {
            return None;
        }
        let mut hblank = ((drm_mode.hdisplay as u32).wrapping_mul(hblank_percentage) / d) as i32;
        hblank -= hblank % (2 * CVT_H_GRANULARITY);
        // 14. find the total pixels per line
        drm_mode.htotal = (drm_mode.hdisplay as i32).wrapping_add(hblank) as u16;
        drm_mode.hsync_end = (drm_mode.hdisplay as i32).wrapping_add(hblank / 2) as u16;
        drm_mode.hsync_start =
            (drm_mode.hsync_end as i32 - (drm_mode.htotal as i32 * CVT_HSYNC_PERCENTAGE) / 100) as u16;
        drm_mode.hsync_start = drm_mode.hsync_start.wrapping_add(
            (CVT_H_GRANULARITY - drm_mode.hsync_start as i32 % CVT_H_GRANULARITY) as u16,
        );
        // fill the Vsync values
        drm_mode.vsync_start = (drm_mode.vdisplay as i32 + CVT_MIN_V_PORCH) as u16;
        drm_mode.vsync_end = (drm_mode.vsync_start as i32 + vsync) as u16;
    } else {
        // Reduced blanking
        // Minimum vertical blanking interval time (µs)- default 460
        const CVT_RB_MIN_VBLANK: i32 = 460;
        // Fixed number of clocks for horizontal sync
        const CVT_RB_H_SYNC: i32 = 32;
        // Fixed number of clocks for horizontal blanking
        const CVT_RB_H_BLANK: i32 = 160;
        // Fixed number of lines for vertical front porch - default 3
        const CVT_RB_VFPORCH: i32 = 3;
        // 8. Estimate Horizontal period.
        let tmp1 = ((HV_FACTOR * 1_000_000) as u32)
            .wrapping_sub(((CVT_RB_MIN_VBLANK * HV_FACTOR) as u32).wrapping_mul(vfieldrate));
        let tmp2 = vdisplay_rnd + 2 * vmargin;
        let d = (tmp2 as u32).wrapping_mul(vfieldrate);
        if d == 0 {
            return None;
        }
        hperiod = tmp1 / d;
        if hperiod == 0 {
            return None;
        }
        // 9. Find number of lines in vertical blanking
        let mut vbilines = ((CVT_RB_MIN_VBLANK * HV_FACTOR) as u32 / hperiod + 1) as i32;
        // 10. Check if vertical blanking is sufficient
        if vbilines < CVT_RB_VFPORCH + vsync + CVT_MIN_V_BPORCH {
            vbilines = CVT_RB_VFPORCH + vsync + CVT_MIN_V_BPORCH;
        }
        // 11. Find total number of lines in vertical field
        drm_mode.vtotal = (vdisplay_rnd + 2 * vmargin).wrapping_add(vbilines) as u16;
        // 12. Find total number of pixels in a line
        drm_mode.htotal = (drm_mode.hdisplay as i32 + CVT_RB_H_BLANK) as u16;
        // Fill in HSync values
        drm_mode.hsync_end = (drm_mode.hdisplay as i32 + CVT_RB_H_BLANK / 2) as u16;
        drm_mode.hsync_start = (drm_mode.hsync_end as i32 - CVT_RB_H_SYNC) as u16;
        // Fill in VSync values
        drm_mode.vsync_start = (drm_mode.vdisplay as i32 + CVT_RB_VFPORCH) as u16;
        drm_mode.vsync_end = (drm_mode.vsync_start as i32 + vsync) as u16;
    }
    // 15/13. Find pixel clock frequency (kHz for xf86)
    let mut tmp: u64 = drm_mode.htotal as u64; // perform intermediate calcs in u64
    tmp *= (HV_FACTOR * 1000) as u64;
    tmp /= hperiod as u64;
    tmp = tmp.wrapping_sub((drm_mode.clock % CVT_CLOCK_STEP) as i64 as u64);
    drm_mode.clock = tmp as i32;
    // 18/16. Find actual vertical frame frequency
    // ignore - just set the mode flag for interlaced
    if interlaced {
        drm_mode.vtotal = drm_mode.vtotal.wrapping_mul(2);
        drm_mode.flags |= DRM_MODE_FLAG_INTERLACE;
    }
    // Fill the mode line name
    drm_mode_set_name(&mut drm_mode);
    if reduced {
        drm_mode.flags |= DRM_MODE_FLAG_PHSYNC | DRM_MODE_FLAG_NVSYNC;
    } else {
        drm_mode.flags |= DRM_MODE_FLAG_PVSYNC | DRM_MODE_FLAG_NHSYNC;
    }

    Some(drm_mode)
}

/// `drm_gtf_mode_complex`. None where Linux returns NULL, and where one of
/// its unsigned divisors would be zero.
#[allow(clippy::too_many_arguments)]
pub fn drm_gtf_mode_complex(hdisplay: i32, vdisplay: i32, vrefresh: i32, interlaced: bool, margins: i32,
                            GTF_M: i32, GTF_2C: i32, GTF_K: i32, GTF_2J: i32) -> Option<drm_display_mode> {
    // 1) top/bottom margin size (% of height) - default: 1.8,
    const GTF_MARGIN_PERCENTAGE: u32 = 18;
    // 2) character cell horizontal granularity (pixels) - default 8
    const GTF_CELL_GRAN: u32 = 8;
    // 3) Minimum vertical porch (lines) - default 3
    const GTF_MIN_V_PORCH: u32 = 1;
    // width of vsync in lines
    const V_SYNC_RQD: u32 = 3;
    // width of hsync as % of total line
    const H_SYNC_PERCENT: u32 = 8;
    // min time of vsync + back porch (microsec)
    const MIN_VSYNC_PLUS_BP: u32 = 550;
    // C' and M' are part of the Blanking Duty Cycle computation
    let GTF_C_PRIME = (((GTF_2C - GTF_2J) * GTF_K / 256) + GTF_2J) / 2;
    let GTF_M_PRIME = GTF_K * GTF_M / 256;

    if hdisplay == 0 || vdisplay == 0 {
        return None;
    }

    let mut drm_mode = drm_display_mode::default();

    // 1. In order to give correct results, the number of horizontal
    // pixels requested is first processed to ensure that it is divisible
    // by the character size, by rounding it to the nearest character
    // cell boundary:
    let mut hdisplay_rnd = (hdisplay.wrapping_add((GTF_CELL_GRAN / 2) as i32) / GTF_CELL_GRAN as i32) as u32;
    hdisplay_rnd = hdisplay_rnd.wrapping_mul(GTF_CELL_GRAN);

    // 2. If interlace is requested, the number of vertical lines assumed
    // by the calculation must be halved, as the computation calculates
    // the number of vertical lines per field.
    let vdisplay_rnd: u32 = if interlaced { (vdisplay / 2) as u32 } else { vdisplay as u32 };

    // 3. Find the frame rate required:
    let vfieldrate_rqd: u32 = if interlaced { (vrefresh * 2) as u32 } else { vrefresh as u32 };

    // 4. Find number of lines in Top margin:
    let mut top_margin: u32 = 0;
    if margins != 0 {
        top_margin = vdisplay_rnd.wrapping_mul(GTF_MARGIN_PERCENTAGE).wrapping_add(500) / 1000;
    }
    // 5. Find number of lines in bottom margin:
    let bottom_margin = top_margin;

    // 6. If interlace is required, then set variable interlace:
    let interlace: u32 = interlaced as u32;

    // 7. Estimate the Horizontal frequency
    let tmp1 = 1_000_000u32.wrapping_sub(MIN_VSYNC_PLUS_BP.wrapping_mul(vfieldrate_rqd)) / 500;
    let tmp2 = vdisplay_rnd.wrapping_add(2 * top_margin).wrapping_add(GTF_MIN_V_PORCH).wrapping_mul(2) + interlace;
    if tmp1 == 0 {
        return None;
    }
    let hfreq_est = tmp2.wrapping_mul(1000).wrapping_mul(vfieldrate_rqd) / tmp1;

    // 8. Find the number of lines in V sync + back porch
    // [V SYNC+BP] = RINT(([MIN VSYNC+BP] * hfreq_est / 1000000))
    let mut vsync_plus_bp = (MIN_VSYNC_PLUS_BP.wrapping_mul(hfreq_est) / 1000) as i32;
    vsync_plus_bp = vsync_plus_bp.wrapping_add(500) / 1000;
    // 9. Find the number of lines in V back porch alone (unused)
    // 10. Find the total number of lines in Vertical field period:
    let vtotal_lines = vdisplay_rnd
        .wrapping_add(top_margin)
        .wrapping_add(bottom_margin)
        .wrapping_add(vsync_plus_bp as u32)
        .wrapping_add(GTF_MIN_V_PORCH);
    // 11.-14. vfieldrate_est, hperiod, vfield_rate, vframe_rate: unused
    // 15. Find number of pixels in left margin:
    let left_margin: i32 = if margins != 0 {
        (hdisplay_rnd.wrapping_mul(GTF_MARGIN_PERCENTAGE).wrapping_add(500) / 1000) as i32
    } else {
        0
    };

    // 16.Find number of pixels in right margin:
    let right_margin = left_margin;
    // 17.Find total number of active pixels in image and left and right
    let total_active_pixels = hdisplay_rnd.wrapping_add(left_margin as u32).wrapping_add(right_margin as u32);
    // 18.Find the ideal blanking duty cycle from blanking duty cycle
    if hfreq_est == 0 {
        return None;
    }
    let ideal_duty_cycle = ((GTF_C_PRIME * 1000) as u32)
        .wrapping_sub(((GTF_M_PRIME as i64 * 1_000_000) as i32 as u32) / hfreq_est);
    // 19.Find the number of pixels in the blanking time to the nearest
    // double character cell:
    let d = 100_000u32.wrapping_sub(ideal_duty_cycle);
    if d == 0 {
        return None;
    }
    let mut hblank = total_active_pixels.wrapping_mul(ideal_duty_cycle) / d;
    hblank = hblank.wrapping_add(GTF_CELL_GRAN) / (2 * GTF_CELL_GRAN);
    hblank = hblank.wrapping_mul(2 * GTF_CELL_GRAN);
    // 20.Find total number of pixels:
    let total_pixels = total_active_pixels.wrapping_add(hblank);
    // 21.Find pixel clock frequency:
    let pixel_freq = total_pixels.wrapping_mul(hfreq_est) / 1000;
    // 17. Find the number of pixels in the horizontal sync period:
    let mut hsync = (H_SYNC_PERCENT.wrapping_mul(total_pixels) / 100) as i32;
    hsync = hsync.wrapping_add((GTF_CELL_GRAN / 2) as i32) / GTF_CELL_GRAN as i32;
    hsync = hsync.wrapping_mul(GTF_CELL_GRAN as i32);
    // 18. Find the number of pixels in horizontal front porch period
    let hfront_porch = (hblank / 2).wrapping_sub(hsync as u32) as i32;
    // 36. Find the number of lines in the odd front porch period:
    let vodd_front_porch_lines = GTF_MIN_V_PORCH as i32;

    // finally, pack the results in the mode struct
    drm_mode.hdisplay = hdisplay_rnd as u16;
    drm_mode.hsync_start = (hdisplay_rnd as i32).wrapping_add(hfront_porch) as u16;
    drm_mode.hsync_end = (drm_mode.hsync_start as i32).wrapping_add(hsync) as u16;
    drm_mode.htotal = total_pixels as u16;
    drm_mode.vdisplay = vdisplay_rnd as u16;
    drm_mode.vsync_start = (vdisplay_rnd as i32 + vodd_front_porch_lines) as u16;
    drm_mode.vsync_end = (drm_mode.vsync_start as u32 + V_SYNC_RQD) as u16;
    drm_mode.vtotal = vtotal_lines as u16;

    drm_mode.clock = pixel_freq as i32;

    if interlaced {
        drm_mode.vtotal = drm_mode.vtotal.wrapping_mul(2);
        drm_mode.flags |= DRM_MODE_FLAG_INTERLACE;
    }

    drm_mode_set_name(&mut drm_mode);
    if GTF_M == 600 && GTF_2C == 80 && GTF_K == 128 && GTF_2J == 40 {
        drm_mode.flags = DRM_MODE_FLAG_NHSYNC | DRM_MODE_FLAG_PVSYNC;
    } else {
        drm_mode.flags = DRM_MODE_FLAG_PHSYNC | DRM_MODE_FLAG_NVSYNC;
    }

    Some(drm_mode)
}

/// `drm_gtf_mode`: the default GTF parameters.
pub fn drm_gtf_mode(hdisplay: i32, vdisplay: i32, vrefresh: i32, interlaced: bool, margins: i32)
    -> Option<drm_display_mode> {
    drm_gtf_mode_complex(hdisplay, vdisplay, vrefresh, interlaced, margins, 600, 40 * 2, 128, 20 * 2)
}

/// `drm_mode_validate_basic`.
pub fn drm_mode_validate_basic(mode: &drm_display_mode) -> drm_mode_status {
    if mode.r#type as u32 & !DRM_MODE_TYPE_ALL != 0 {
        return MODE_BAD as i32;
    }

    if mode.flags & !DRM_MODE_FLAG_ALL != 0 {
        return MODE_BAD as i32;
    }

    if (mode.flags & DRM_MODE_FLAG_3D_MASK) > DRM_MODE_FLAG_3D_MAX {
        return MODE_BAD as i32;
    }

    if mode.clock == 0 {
        return MODE_CLOCK_LOW as i32;
    }

    if mode.hdisplay == 0
        || mode.hsync_start < mode.hdisplay
        || mode.hsync_end < mode.hsync_start
        || mode.htotal < mode.hsync_end
    {
        return MODE_H_ILLEGAL as i32;
    }

    if mode.vdisplay == 0
        || mode.vsync_start < mode.vdisplay
        || mode.vsync_end < mode.vsync_start
        || mode.vtotal < mode.vsync_end
    {
        return MODE_V_ILLEGAL as i32;
    }

    MODE_OK as i32
}

/// `drm_mode_validate_driver`: `dev->mode_config.funcs->mode_valid` is
/// the caller's (MODE_OK without one).
pub fn drm_mode_validate_driver(mode: &drm_display_mode,
                                mode_valid: &mut dyn FnMut(&drm_display_mode) -> drm_mode_status) -> drm_mode_status {
    let status = drm_mode_validate_basic(mode);
    if status != MODE_OK as i32 {
        return status;
    }

    mode_valid(mode)
}

/// `drm_mode_validate_size`.
pub fn drm_mode_validate_size(mode: &drm_display_mode, maxX: i32, maxY: i32) -> drm_mode_status {
    if maxX > 0 && mode.hdisplay as i32 > maxX {
        return MODE_VIRTUAL_X as i32;
    }

    if maxY > 0 && mode.vdisplay as i32 > maxY {
        return MODE_VIRTUAL_Y as i32;
    }

    MODE_OK as i32
}

/// `drm_mode_prune_invalid`.
pub fn drm_mode_prune_invalid(mode_list: &mut alloc::vec::Vec<drm_display_mode>) {
    mode_list.retain(|m| m.status == MODE_OK as i32);
}

/// `drm_mode_compare`.
fn drm_mode_compare(a: &drm_display_mode, b: &drm_display_mode) -> i32 {
    let diff = ((b.r#type as u32 & DRM_MODE_TYPE_PREFERRED) != 0) as i32
        - ((a.r#type as u32 & DRM_MODE_TYPE_PREFERRED) != 0) as i32;
    if diff != 0 {
        return diff;
    }
    let diff = (b.hdisplay as i32).wrapping_mul(b.vdisplay as i32)
        .wrapping_sub((a.hdisplay as i32).wrapping_mul(a.vdisplay as i32));
    if diff != 0 {
        return diff;
    }

    let diff = drm_mode_vrefresh(b).wrapping_sub(drm_mode_vrefresh(a));
    if diff != 0 {
        return diff;
    }

    b.clock.wrapping_sub(a.clock)
}

/// `drm_mode_sort`: `list_sort` is a stable merge sort, as `sort_by`.
pub fn drm_mode_sort(mode_list: &mut alloc::vec::Vec<drm_display_mode>) {
    mode_list.sort_by(|a, b| drm_mode_compare(a, b).cmp(&0));
}

/// `drm_connector_list_update`: the probed modes merged into `modes`; an
/// equal mode already there takes the type bits, or is replaced when
/// stale, or by a preferred one.
pub fn drm_connector_list_update(connector: &mut crate::drm_connector::drm_connector) {
    let probed = core::mem::take(&mut connector.probed_modes);

    for mut pmode in probed {
        let mut found_it = false;

        // go through current modes checking for the new probed mode
        for mode in connector.modes.iter_mut() {
            if !drm_mode_equal(&pmode, mode) {
                continue;
            }

            found_it = true;

            // If the old matching mode is stale (ie. left over
            // from a previous probe) just replace it outright.
            // Otherwise just merge the type bits between all
            // equal probed modes.
            //
            // If two probed modes are considered equal, pick the
            // actual timings from the one that's marked as
            // preferred (in case the match isn't 100%). If
            // multiple or zero preferred modes are present, favor
            // the mode added to the probed_modes list first.
            if mode.status == MODE_STALE as i32 {
                *mode = pmode.clone();
            } else if (mode.r#type as u32 & DRM_MODE_TYPE_PREFERRED) == 0
                && (pmode.r#type as u32 & DRM_MODE_TYPE_PREFERRED) != 0
            {
                pmode.r#type |= mode.r#type;
                *mode = pmode.clone();
            } else {
                mode.r#type |= pmode.r#type;
            }
            break;
        }

        if !found_it {
            connector.modes.push(pmode);
        }
    }
}

/// `drm_mode_is_420_only`.
pub fn drm_mode_is_420_only(display: &crate::drm_connector::drm_display_info, mode: &drm_display_mode) -> bool {
    let vic = crate::drm_edid::drm_match_cea_mode(mode);

    crate::drm_connector::test_bit(vic, &display.hdmi.y420_vdb_modes)
}

/// `drm_mode_is_420_also`.
pub fn drm_mode_is_420_also(display: &crate::drm_connector::drm_display_info, mode: &drm_display_mode) -> bool {
    let vic = crate::drm_edid::drm_match_cea_mode(mode);

    crate::drm_connector::test_bit(vic, &display.hdmi.y420_cmdb_modes)
}

/// `drm_mode_is_420`.
pub fn drm_mode_is_420(display: &crate::drm_connector::drm_display_info, mode: &drm_display_mode) -> bool {
    drm_mode_is_420_only(display, mode) || drm_mode_is_420_also(display, mode)
}

/// `drm_mode_validate_ycbcr420`.
pub fn drm_mode_validate_ycbcr420(mode: &drm_display_mode, connector: &crate::drm_connector::drm_connector)
    -> drm_mode_status {
    if !connector.ycbcr_420_allowed && drm_mode_is_420_only(&connector.display_info, mode) {
        return MODE_NO_420 as i32;
    }

    MODE_OK as i32
}

/// `drm_set_preferred_mode`.
pub fn drm_set_preferred_mode(connector: &mut crate::drm_connector::drm_connector, hpref: i32, vpref: i32) {
    for mode in connector.probed_modes.iter_mut() {
        if mode.hdisplay as i32 == hpref && mode.vdisplay as i32 == vpref {
            mode.r#type |= DRM_MODE_TYPE_PREFERRED as u8;
        }
    }
}
