//! amdgpu/amdgpu_display.c and amdgpu_kms.c (Linux 6.18.55), the CRTC
//! queries vblank handling and page-flip throttling make: the scanout
//! position against the vblank, and the hardware frame counter cooked to
//! count from the start of vblank. The display funcs are DM's
//! (`dm_vblank_get_counter`, `dm_crtc_get_scanoutpos`).

use crate::amdgpu_device::amdgpu_device;
use crate::drm_vblank::drm_display_mode;
use crate::linux::Line;

/// `DRM_SCANOUTPOS_VALID` and its siblings (amdgpu_mode.h).
pub const DRM_SCANOUTPOS_VALID: u32 = 1 << 0;
pub const DRM_SCANOUTPOS_IN_VBLANK: u32 = 1 << 1;
pub const DRM_SCANOUTPOS_ACCURATE: u32 = 1 << 2;
pub const USE_REAL_VBLANKSTART: u32 = 1 << 30;
pub const GET_DISTANCE_TO_VBLANKSTART: u32 = 1 << 31;

/// `amdgpu_display_get_crtc_scanoutpos`: the scanout position of CRTC
/// `pipe`, `vpos` positive in active scanout and negative inside vblank
/// (the lines until scanout starts), with the times before and after the
/// query in microseconds. Returns the `DRM_SCANOUTPOS_*` flags.
#[allow(clippy::too_many_arguments)]
pub fn amdgpu_display_get_crtc_scanoutpos(adev: &amdgpu_device, pipe: usize, flags: u32, vpos: &mut i32,
                                          hpos: &mut i32, stime: Option<&mut u64>, etime: Option<&mut u64>,
                                          mode: &drm_display_mode) -> u32 {
    let (mut vbl, mut position) = (0u32, 0u32);
    let vbl_end;
    let mut ret = 0;
    let mut in_vbl = true;

    // Get optional system timestamp before query.
    if let Some(t) = stime {
        *t = crate::linux::now_us();
    }

    if crate::amdgpu_dm::dm_crtc_get_scanoutpos(adev, pipe as i32, &mut vbl, &mut position) == 0 {
        ret |= DRM_SCANOUTPOS_VALID;
    }

    // Get optional system timestamp after query.
    if let Some(t) = etime {
        *t = crate::linux::now_us();
    }

    // Decode into vertical and horizontal scanout position.
    *vpos = (position & 0x1fff) as i32;
    *hpos = ((position >> 16) & 0x1fff) as i32;

    // Valid vblank area boundaries from gpu retrieved?
    let mut vbl_start = if vbl > 0 {
        // Yes: Decode.
        ret |= DRM_SCANOUTPOS_ACCURATE;
        vbl_end = ((vbl >> 16) & 0x1fff) as i32;
        (vbl & 0x1fff) as i32
    } else {
        // No: Fake something reasonable which gives at least ok results.
        vbl_end = 0;
        mode.crtc_vdisplay
    };

    // Called from driver internal vblank counter query code?
    if flags & GET_DISTANCE_TO_VBLANKSTART != 0 {
        // Caller wants distance from real vbl_start in *hpos
        *hpos = *vpos - vbl_start;
    }

    // Fudge vblank to start a few scanlines earlier to handle the problem
    // that vblank irqs fire a few scanlines before start of vblank. Some
    // driver internal callers need the true vblank start to be used and
    // signal this via the USE_REAL_VBLANKSTART flag.
    //
    // The cause of the "early" vblank irq is that the irq is triggered by
    // the line buffer logic when the line buffer read position enters the
    // vblank, whereas our crtc scanout position naturally lags the line
    // buffer read position.
    if flags & USE_REAL_VBLANKSTART == 0 {
        vbl_start -= adev.mode_info.crtcs[pipe].unwrap().lb_vblank_lead_lines as i32;
    }

    // Test scanout position against vblank region.
    if *vpos < vbl_start && *vpos >= vbl_end {
        in_vbl = false;
    }

    // In vblank?
    if in_vbl {
        ret |= DRM_SCANOUTPOS_IN_VBLANK;
    }

    // Called from driver internal vblank counter query code?
    if flags & GET_DISTANCE_TO_VBLANKSTART != 0 {
        // Caller wants distance from fudged earlier vbl_start
        *vpos -= vbl_start;
        return ret;
    }

    // Check if inside vblank area and apply corrective offsets: vpos will
    // then be >=0 in video scanout area, but negative within vblank area,
    // counting down the number of lines until start of scanout.

    // Inside "upper part" of vblank area? Apply corrective offset if so:
    if in_vbl && *vpos >= vbl_start {
        let vtotal = mode.crtc_vtotal;

        // With variable refresh rate displays the vpos can exceed the
        // vtotal value. Clamp to 0 to return -vbl_end instead of guessing
        // the remaining number of lines until scanout.
        *vpos = if *vpos < vtotal { *vpos - vtotal } else { 0 };
    }

    // Correct for shifted end of vbl at vbl_end.
    *vpos -= vbl_end;

    ret
}

/// `amdgpu_crtc_get_scanout_position`, the CRTC's `get_scanout_position`.
#[allow(clippy::too_many_arguments)]
pub fn amdgpu_crtc_get_scanout_position(adev: &amdgpu_device, pipe: usize, _in_vblank_irq: bool, vpos: &mut i32,
                                        hpos: &mut i32, stime: Option<&mut u64>, etime: Option<&mut u64>,
                                        mode: &drm_display_mode) -> bool {
    amdgpu_display_get_crtc_scanoutpos(adev, pipe, 0, vpos, hpos, stime, etime, mode) != 0
}

/// `amdgpu_get_vblank_counter_kms` (amdgpu_kms.c): the frame count of CRTC
/// `pipe`, cooked to have counted at the start of vblank — the hardware
/// counts at the start of vsync.
pub fn amdgpu_get_vblank_counter_kms(adev: &amdgpu_device, pipe: usize) -> u32 {
    let (mut vpos, mut hpos) = (0, 0);
    let mut stat;
    let mut count;

    if pipe >= adev.mode_info.num_crtc as usize {
        Line::new().s("Invalid crtc ").dec(pipe as u64).say();
        return -22i32 as u32;
    }

    // The hw increments its frame counter at start of vsync, not at start
    // of vblank, as is required by DRM core vblank counter handling. Cook
    // the hw count here to make it appear to the caller as if it
    // incremented at start of vblank. We measure distance to start of
    // vblank in vpos. vpos therefore will be >= 0 between start of vblank
    // and start of vsync, so vpos >= 0 means to bump the hw frame counter
    // result by 1 to give the proper appearance to caller.
    if let Some(Some(c)) = adev.mode_info.crtcs.get(pipe) {
        // Repeat readout if needed to provide stable result if we cross
        // start of vsync during the queries.
        loop {
            count = crate::amdgpu_dm::dm_vblank_get_counter(adev, pipe as i32);
            // Ask amdgpu_display_get_crtc_scanoutpos to return vpos as
            // distance to start of vblank, instead of regular vertical
            // scanout pos.
            stat = amdgpu_display_get_crtc_scanoutpos(adev, pipe, GET_DISTANCE_TO_VBLANKSTART, &mut vpos, &mut hpos,
                                                      None, None, &c.hwmode);
            if count == crate::amdgpu_dm::dm_vblank_get_counter(adev, pipe as i32) {
                break;
            }
        }

        if stat & (DRM_SCANOUTPOS_VALID | DRM_SCANOUTPOS_ACCURATE) == DRM_SCANOUTPOS_VALID | DRM_SCANOUTPOS_ACCURATE {
            // Bump counter if we are at >= leading edge of vblank, but
            // before vsync where vpos would turn negative and the hw
            // counter really increments.
            if vpos >= 0 {
                count = count.wrapping_add(1);
            }
        }
    } else {
        // Fallback to use value as is.
        count = crate::amdgpu_dm::dm_vblank_get_counter(adev, pipe as i32);
    }

    count
}
