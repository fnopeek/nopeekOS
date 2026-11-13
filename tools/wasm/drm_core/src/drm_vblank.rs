//! drivers/gpu/drm/drm_vblank.c (Linux rpi-6.18): a CRTC's vblank
//! counter and timestamps for a driver without a hardware frame counter
//! (`max_vblank_count` 0): the count follows the timestamps the driver's
//! scanout position gives (`drm_crtc_vblank_helper_get_vblank_timestamp`).
//! Single-threaded, so the locks and the seqlock guard nothing. There are
//! no userspace vblank waits or queued vblank events; a page flip's event
//! is the driver's and goes out with [`drm_crtc_send_vblank_event`].

use crate::drm_modes::drm_display_mode;
use crate::regs::*;

/// `DRM_TIMESTAMP_MAXRETRIES`.
const DRM_TIMESTAMP_MAXRETRIES: u32 = 3;

/// `drm_timestamp_precision` (usecs), the module parameter's default.
const DRM_TIMESTAMP_PRECISION: i32 = 20;

/// `drm_vblank_offdelay` (msecs), the module parameter's default.
pub const DRM_VBLANK_OFFDELAY: i32 = 5000;

/// What the core asks of the driver's CRTC (`drm_crtc_funcs`).
pub trait drm_vblank_funcs {
    /// `get_scanout_position` with its system timestamps (ns) before and
    /// after the query: Some((vpos, hpos, stime, etime)) when the
    /// position means something.
    fn get_scanout_position(&mut self, in_vblank_irq: bool, mode: &drm_display_mode)
        -> Option<(i32, i32, i64, i64)>;

    /// `enable_vblank`.
    fn enable_vblank(&mut self) -> i32;

    /// `disable_vblank`.
    fn disable_vblank(&mut self);

    /// `ktime_get`, in ns.
    fn ktime_get(&mut self) -> i64;
}

/// `struct drm_vblank_crtc_config`.
#[derive(Clone, Copy, Debug, Default)]
pub struct drm_vblank_crtc_config {
    pub offdelay_ms: i32,
    pub disable_immediate: bool,
}

/// `struct drm_vblank_crtc`.
#[derive(Clone, Debug, Default)]
pub struct drm_vblank_crtc {
    pub count: u64,
    /// ns.
    pub time: i64,
    pub refcount: u32,
    pub last: u32,
    pub enabled: bool,
    pub inmodeset: u32,
    pub framedur_ns: i32,
    pub linedur_ns: i32,
    pub hwmode: drm_display_mode,
    pub config: drm_vblank_crtc_config,
}

/// `drm_vblank_no_hw_counter`.
fn __get_vblank_counter() -> u32 {
    0
}

/// `drm_crtc_vblank_helper_get_vblank_timestamp_internal`: the time the
/// current vblank ended, from where the scanout is.
fn get_vblank_timestamp(vblank: &drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs, max_error: i32,
                        in_vblank_irq: bool) -> Option<i64> {
    let mode = &vblank.hwmode;

    if mode.crtc_clock == 0 {
        return None;
    }

    let mut pos = (0, 0, 0, 0);
    for _ in 0..DRM_TIMESTAMP_MAXRETRIES {
        pos = funcs.get_scanout_position(in_vblank_irq, mode)?;

        let duration_ns = pos.3 - pos.2;

        if duration_ns <= max_error as i64 {
            break;
        }
    }

    let (vpos, hpos, _stime, etime) = pos;

    // Convert scanout position into elapsed time at raw_time query
    // since start of scanout at first display scanline. delta_ns
    // can be negative if start of scanout hasn't happened yet.
    let delta_ns = 1_000_000i64 * (vpos as i64 * mode.crtc_htotal as i64 + hpos as i64) / mode.crtc_clock as i64;

    // Subtract time delta from raw timestamp to get final
    // vblank_time timestamp for end of vblank.
    Some(etime - delta_ns)
}

/// `drm_get_last_vbltimestamp`: the driver's timestamp, else now.
fn drm_get_last_vbltimestamp(vblank: &drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs, in_vblank_irq: bool)
    -> (bool, i64) {
    // Define requested maximum error on timestamps (nanoseconds).
    let max_error = DRM_TIMESTAMP_PRECISION * 1000;

    match get_vblank_timestamp(vblank, funcs, max_error, in_vblank_irq) {
        Some(t) => (true, t),
        None => (false, funcs.ktime_get()),
    }
}

/// `store_vblank`.
fn store_vblank(vblank: &mut drm_vblank_crtc, vblank_count_inc: u32, t_vblank: i64, last: u32) {
    vblank.last = last;
    vblank.time = t_vblank;
    vblank.count += vblank_count_inc as u64;
}

/// `drm_reset_vblank_timestamp`.
fn drm_reset_vblank_timestamp(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs) {
    let cur_vblank = __get_vblank_counter();
    let (rc, mut t_vblank) = drm_get_last_vbltimestamp(vblank, funcs, false);

    if !rc {
        t_vblank = 0;
    }

    // Only reinitialize corresponding vblank timestamp if high-precision
    // query available and didn't fail. Otherwise reinitialize delayed at
    // next vblank interrupt and assign 0 for now, to mark the vblanktimestamp
    // as invalid.
    store_vblank(vblank, 1, t_vblank, cur_vblank);
}

/// `drm_update_vblank_count`: the frames since the last update, from the
/// timestamps.
fn drm_update_vblank_count(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs, in_vblank_irq: bool) {
    let framedur_ns = vblank.framedur_ns;

    let cur_vblank = __get_vblank_counter();
    let (rc, mut t_vblank) = drm_get_last_vbltimestamp(vblank, funcs, in_vblank_irq);

    let mut diff = if rc && framedur_ns != 0 {
        let diff_ns = t_vblank.wrapping_sub(vblank.time) as u64;

        // DIV_ROUND_CLOSEST_ULL
        ((diff_ns + framedur_ns as u64 / 2) / framedur_ns as u64) as u32
    } else if in_vblank_irq {
        1
    } else {
        0
    };

    // Within a drm_vblank_pre_modeset - drm_vblank_post_modeset
    // interval? If so then vblank irqs keep running and it will likely
    // happen that the hardware vblank counter is not trustworthy as it
    // might reset at some point in that interval and vblank timestamps
    // are not trustworthy either in that interval. Iow. this can result
    // in a bogus diff >> 1 which must be avoided as it would cause
    // random large forward jumps of the software vblank counter.
    if diff > 1 && vblank.inmodeset & 0x2 != 0 {
        diff = 1;
    }

    if diff == 0 {
        return;
    }

    // Only reinitialize corresponding vblank timestamp if high-precision
    // query available and didn't fail, or we were called from the vblank
    // interrupt and we're in the middle of a pageflip.
    if !rc && !in_vblank_irq {
        t_vblank = 0;
    }

    store_vblank(vblank, diff, t_vblank, cur_vblank);
}

/// `drm_vblank_count_and_time`.
pub fn drm_vblank_count_and_time(vblank: &drm_vblank_crtc) -> (u64, i64) {
    (vblank.count, vblank.time)
}

/// `drm_vblank_disable_and_save`.
fn drm_vblank_disable_and_save(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs) {
    // Update vblank count and disable vblank interrupts only if the
    // interrupts were enabled.
    if !vblank.enabled {
        return;
    }

    // Update the count and timestamp to maintain the appearance that the
    // counter has been ticking all along.
    drm_update_vblank_count(vblank, funcs, false);
    funcs.disable_vblank();
    vblank.enabled = false;
}

/// `vblank_disable_fn`: the disable timer, or an immediate disable.
pub fn vblank_disable_fn(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs) {
    if vblank.refcount == 0 && vblank.enabled {
        drm_vblank_disable_and_save(vblank, funcs);
    }
}

/// `drm_vblank_enable`.
fn drm_vblank_enable(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs) -> i32 {
    let mut ret = 0;

    if !vblank.enabled {
        ret = funcs.enable_vblank();
        if ret != 0 {
            vblank.refcount -= 1;
        } else {
            drm_update_vblank_count(vblank, funcs, false);
            vblank.enabled = true;
        }
    }

    ret
}

/// `drm_crtc_vblank_get`: a reference that keeps the interrupt on.
pub fn drm_crtc_vblank_get(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs) -> i32 {
    vblank.refcount += 1;
    if vblank.refcount == 1 {
        return drm_vblank_enable(vblank, funcs);
    }

    if !vblank.enabled {
        vblank.refcount -= 1;
        return -crate::linux::EINVAL;
    }

    0
}

/// `drm_crtc_vblank_put`. Ok(true) when the caller should arm the
/// disable timer (`offdelay_ms` from now, then [`vblank_disable_fn`]).
pub fn drm_crtc_vblank_put(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs) -> bool {
    let vblank_offdelay = vblank.config.offdelay_ms;

    if vblank.refcount == 0 {
        return false;
    }

    vblank.refcount -= 1;
    if vblank.refcount == 0 {
        if vblank_offdelay == 0 {
            return false;
        } else if vblank_offdelay < 0 {
            vblank_disable_fn(vblank, funcs);
        } else if !vblank.config.disable_immediate {
            return true;
        }
    }

    false
}

/// `drm_crtc_vblank_on_config`.
pub fn drm_crtc_vblank_on_config(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs,
                                 config: drm_vblank_crtc_config) {
    vblank.config = config;

    // Drop our private "prevent drm_vblank_get" refcount
    if vblank.inmodeset != 0 {
        vblank.refcount -= 1;
        vblank.inmodeset = 0;
    }

    drm_reset_vblank_timestamp(vblank, funcs);

    // re-enable interrupts if there are users left, or the
    // user wishes vblank interrupts to be enabled all the time.
    if vblank.refcount != 0 || vblank.config.offdelay_ms == 0 {
        drm_vblank_enable(vblank, funcs);
    }
}

/// `drm_crtc_vblank_on` with the device's `vblank_disable_immediate`.
pub fn drm_crtc_vblank_on(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs, disable_immediate: bool) {
    let config = drm_vblank_crtc_config { offdelay_ms: DRM_VBLANK_OFFDELAY, disable_immediate };

    drm_crtc_vblank_on_config(vblank, funcs, config);
}

/// `drm_handle_vblank`: the count updated for an interrupt. False when
/// vblank is off.
pub fn drm_crtc_handle_vblank(vblank: &mut drm_vblank_crtc, funcs: &mut dyn drm_vblank_funcs) -> bool {
    if !vblank.enabled {
        return false;
    }

    drm_update_vblank_count(vblank, funcs, true);

    let disable_irq = vblank.config.disable_immediate && vblank.config.offdelay_ms > 0 && vblank.refcount == 0;

    if disable_irq {
        vblank_disable_fn(vblank, funcs);
    }

    true
}

/// `drm_calc_timestamping_constants`: line and frame duration of the
/// mode the CRTC now runs.
pub fn drm_calc_timestamping_constants(vblank: &mut drm_vblank_crtc, mode: &drm_display_mode) {
    let mut linedur_ns = 0;
    let mut framedur_ns = 0;
    let dotclock = mode.crtc_clock;

    // Valid dotclock? Even 1 Hz refresh would be ok here.
    if dotclock > 0 {
        let frame_size = mode.crtc_htotal as i32 * mode.crtc_vtotal as i32;

        // Convert scanline length in pixels and video dot clock to line
        // duration and frame duration in nanoseconds:
        linedur_ns = (mode.crtc_htotal as u64 * 1_000_000 / dotclock as u64) as i32;
        framedur_ns = (frame_size as u64 * 1_000_000 / dotclock as u64) as i32;

        // Fields of interlaced scanout modes are only half a frame
        // duration.
        if mode.flags & DRM_MODE_FLAG_INTERLACE != 0 && mode.crtc_vtotal >= mode.vtotal {
            framedur_ns /= 2;
        }
    }

    vblank.linedur_ns = linedur_ns;
    vblank.framedur_ns = framedur_ns;
    vblank.hwmode = mode.clone();
}

/// `drm_crtc_send_vblank_event`: the sequence and time a page flip's
/// completion carries.
pub fn drm_crtc_send_vblank_event(vblank: &drm_vblank_crtc) -> (u64, i64) {
    drm_vblank_count_and_time(vblank)
}
