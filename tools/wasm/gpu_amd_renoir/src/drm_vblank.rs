//! drm/drm_vblank.c (Linux 6.18.55), the part a CRTC of this driver needs:
//! the reference-counted vblank interrupt with its disable timer, the
//! count kept with the hardware frame counter and high-precision
//! timestamps from the scanout position, and page-flip completion events.
//! The time base is microseconds; Linux' ktime nanoseconds are derived
//! from it where the code computes in them.
#![allow(dead_code, non_camel_case_types)]

use crate::amdgpu_device::amdgpu_device;
use crate::linux::Line;

/// `DRM_TIMESTAMP_MAXRETRIES`.
const DRM_TIMESTAMP_MAXRETRIES: i32 = 3;

/// `drm_timestamp_precision`: the default, in microseconds.
const drm_timestamp_precision: i32 = 20;

/// `dev->max_vblank_count` as `amdgpu_irq_init` sets it: DCN's 24-bit
/// frame counter.
const DEV_MAX_VBLANK_COUNT: u32 = 0x00ff_ffff;

/// `DRM_MODE_FLAG_INTERLACE`.
pub const DRM_MODE_FLAG_INTERLACE: u32 = 1 << 4;

/// `struct drm_display_mode`, the CRTC timings vblank handling reads.
#[derive(Clone, Copy, Default, Debug)]
pub struct drm_display_mode {
    /// In kHz.
    pub crtc_clock: i32,
    pub crtc_htotal: i32,
    pub crtc_vdisplay: i32,
    pub crtc_vtotal: i32,
    pub flags: u32,
}

/// `struct drm_vblank_crtc`, the members reached.
#[derive(Clone, Copy, Default, Debug)]
pub struct drm_vblank_crtc {
    pub count: u64,
    /// The timestamp of the last vblank, in microseconds; 0 is invalid.
    pub time: u64,
    pub refcount: u32,
    /// `last`: the hardware counter at the last update.
    pub last: u32,
    pub max_vblank_count: u32,
    pub enabled: bool,
    pub inmodeset: bool,
    pub linedur_ns: i32,
    pub framedur_ns: i32,
    pub hwmode: drm_display_mode,
    pub config: drm_vblank_crtc_config,
}

/// `struct drm_vblank_crtc_config`.
#[derive(Clone, Copy, Default, Debug)]
pub struct drm_vblank_crtc_config {
    pub offdelay_ms: i32,
    pub disable_immediate: bool,
}

/// A completed page flip, as `send_vblank_event` delivers it: the event,
/// the vblank count and time it completed at.
#[derive(Clone, Copy, Debug)]
pub struct drm_flip_event {
    pub event: u32,
    pub crtc: usize,
    pub sequence: u64,
    pub time_us: u64,
}

fn vblank(adev: &mut amdgpu_device, pipe: usize) -> &mut drm_vblank_crtc {
    &mut adev.mode_info.crtcs[pipe].as_mut().unwrap().vblank
}

/// `store_vblank`.
fn store_vblank(adev: &mut amdgpu_device, pipe: usize, vblank_count_inc: u32, t_vblank: u64, last: u32) {
    let v = vblank(adev, pipe);

    v.last = last;

    v.time = t_vblank;
    v.count += vblank_count_inc as u64;
}

/// `drm_max_vblank_count`.
fn drm_max_vblank_count(adev: &mut amdgpu_device, pipe: usize) -> u32 {
    let v = vblank(adev, pipe);

    if v.max_vblank_count != 0 { v.max_vblank_count } else { DEV_MAX_VBLANK_COUNT }
}

/// `__get_vblank_counter` → `amdgpu_get_vblank_counter_kms`.
fn __get_vblank_counter(adev: &amdgpu_device, pipe: usize) -> u32 {
    crate::amdgpu_display::amdgpu_get_vblank_counter_kms(adev, pipe)
}

/// `drm_reset_vblank_timestamp`: the stored timestamp for the current
/// count set to the last vblank that occurred.
fn drm_reset_vblank_timestamp(adev: &mut amdgpu_device, pipe: usize) {
    let mut cur_vblank;
    let mut rc;
    let mut t_vblank = 0;
    let mut count = DRM_TIMESTAMP_MAXRETRIES;

    // sample the current counter to avoid random jumps when
    // drm_vblank_enable() applies the diff
    loop {
        cur_vblank = __get_vblank_counter(adev, pipe);
        rc = drm_get_last_vbltimestamp(adev, pipe, &mut t_vblank, false);
        count -= 1;
        if cur_vblank == __get_vblank_counter(adev, pipe) || count <= 0 {
            break;
        }
    }

    // Only reinitialize corresponding vblank timestamp if high-precision
    // query available and didn't fail. Otherwise reinitialize delayed at
    // next vblank interrupt and assign 0 for now, to mark the
    // vblanktimestamp as invalid.
    if !rc {
        t_vblank = 0;
    }

    // +1 to make sure user will never see the same vblank counter value
    // before and after a modeset
    store_vblank(adev, pipe, 1, t_vblank, cur_vblank);
}

/// `drm_update_vblank_count`: the count advanced by the hardware counter
/// since the last update, wrap included.
fn drm_update_vblank_count(adev: &mut amdgpu_device, pipe: usize, in_vblank_irq: bool) {
    let mut cur_vblank;
    let mut rc;
    let mut t_vblank = 0;
    let mut count = DRM_TIMESTAMP_MAXRETRIES;
    let framedur_ns = vblank(adev, pipe).framedur_ns;
    let max_vblank_count = drm_max_vblank_count(adev, pipe);

    // We repeat the hardware vblank counter & timestamp query until we get
    // consistent results. This to prevent races between gpu updating its
    // hardware counter while we are retrieving the corresponding vblank
    // timestamp.
    loop {
        cur_vblank = __get_vblank_counter(adev, pipe);
        rc = drm_get_last_vbltimestamp(adev, pipe, &mut t_vblank, in_vblank_irq);
        count -= 1;
        if cur_vblank == __get_vblank_counter(adev, pipe) || count <= 0 {
            break;
        }
    }

    let v = *vblank(adev, pipe);
    let diff = if max_vblank_count != 0 {
        // trust the hw counter when it's around
        cur_vblank.wrapping_sub(v.last) & max_vblank_count
    } else if rc && framedur_ns != 0 {
        // Figure out how many vblanks we've missed based on the difference
        // in the timestamps and the frame/field duration.
        let diff_ns = t_vblank.wrapping_sub(v.time) * 1000;
        ((diff_ns + framedur_ns as u64 / 2) / framedur_ns as u64) as u32
    } else {
        // some kind of default for drivers w/o accurate vbl timestamping
        in_vblank_irq as u32
    };

    // The clamp within a drm_vblank_pre_modeset - post_modeset interval
    // (inmodeset & 0x2) is legacy modesetting's.

    if diff == 0 {
        if cur_vblank != v.last {
            Line::new().s("WARNING: drm_update_vblank_count: crtc ").dec(pipe as u64).s(" hw ").dec(cur_vblank as u64)
                .s(" != last ").dec(v.last as u64).say();
        }
        return;
    }

    // Only reinitialize corresponding vblank timestamp if high-precision
    // query available and didn't fail, or we were called from the vblank
    // interrupt. Otherwise reinitialize delayed at next vblank interrupt
    // and assign 0 for now, to mark the vblanktimestamp as invalid.
    if !rc && !in_vblank_irq {
        t_vblank = 0;
    }

    store_vblank(adev, pipe, diff, t_vblank, cur_vblank);
}

/// `drm_vblank_count`.
pub fn drm_vblank_count(adev: &mut amdgpu_device, pipe: usize) -> u64 {
    vblank(adev, pipe).count
}

/// `drm_crtc_accurate_vblank_count`.
pub fn drm_crtc_accurate_vblank_count(adev: &mut amdgpu_device, crtc: usize) -> u64 {
    drm_update_vblank_count(adev, crtc, false);
    drm_vblank_count(adev, crtc)
}

/// `__disable_vblank` → `amdgpu_dm_crtc_disable_vblank`.
fn __disable_vblank(adev: &mut amdgpu_device, pipe: usize) {
    crate::amdgpu_dm::amdgpu_dm_crtc_set_vblank(adev, pipe, false);
}

/// `drm_vblank_disable_and_save`: the interrupt off, with the count kept
/// as if it had been ticking all along.
fn drm_vblank_disable_and_save(adev: &mut amdgpu_device, pipe: usize) {
    // Update vblank count and disable vblank interrupts only if the
    // interrupts were enabled. This avoids calling the ->disable_vblank()
    // operation in atomic context with the hardware potentially runtime
    // suspended.
    if !vblank(adev, pipe).enabled {
        return;
    }

    // Update the count and timestamp to maintain the appearance that the
    // counter has been ticking all along until this time. This makes the
    // count account for the entire time between drm_crtc_vblank_on() and
    // drm_crtc_vblank_off().
    drm_update_vblank_count(adev, pipe, false);
    __disable_vblank(adev, pipe);
    vblank(adev, pipe).enabled = false;
}

/// `vblank_disable_fn`: the disable timer.
pub fn vblank_disable_fn(adev: &mut amdgpu_device, pipe: usize) {
    let v = vblank(adev, pipe);
    if v.refcount == 0 && v.enabled {
        drm_vblank_disable_and_save(adev, pipe);
    }
}

/// `drm_calc_timestamping_constants`: line and frame duration of `mode`,
/// which becomes the vblank's hardware mode.
pub fn drm_calc_timestamping_constants(adev: &mut amdgpu_device, crtc: usize, mode: &drm_display_mode) {
    let (mut linedur_ns, mut framedur_ns) = (0, 0);
    let dotclock = mode.crtc_clock;

    // Valid dotclock?
    if dotclock > 0 {
        let frame_size = mode.crtc_htotal * mode.crtc_vtotal;

        // Convert scanline length in pixels and video dot clock to line
        // duration and frame duration in nanoseconds:
        linedur_ns = (mode.crtc_htotal as u64 * 1_000_000 / dotclock as u64) as i32;
        framedur_ns = (frame_size as u64 * 1_000_000 / dotclock as u64) as i32;

        // Fields of interlaced scanout modes are only half a frame
        // duration.
        if mode.flags & DRM_MODE_FLAG_INTERLACE != 0 {
            framedur_ns /= 2;
        }
    } else {
        Line::new().s("crtc ").dec(crtc as u64).s(": Can't calculate constants, dotclock = 0!").say();
    }

    let v = vblank(adev, crtc);
    v.linedur_ns = linedur_ns;
    v.framedur_ns = framedur_ns;
    v.hwmode = *mode;
}

/// `drm_crtc_vblank_helper_get_vblank_timestamp_internal` with
/// `amdgpu_crtc_get_scanout_position`: the time of the end of the last
/// vblank, from the scanout position and the time it was read.
fn drm_crtc_vblank_helper_get_vblank_timestamp_internal(adev: &mut amdgpu_device, pipe: usize, max_error: &mut i32,
                                                        vblank_time: &mut u64, in_vblank_irq: bool) -> bool {
    let mode = vblank(adev, pipe).hwmode;
    let (mut vpos, mut hpos) = (0, 0);
    let (mut stime, mut etime) = (0u64, 0u64);
    let mut duration_ns = 0;

    // If mode timing undefined, just return as no-op: Happens during
    // initial modesetting of a crtc.
    if mode.crtc_clock == 0 {
        Line::new().s("WARNING: crtc ").dec(pipe as u64).s(": Noop due to uninitialized mode.").say();
        return false;
    }

    // Get current scanout position with system timestamp. Repeat query up
    // to DRM_TIMESTAMP_MAXRETRIES times if single query takes longer than
    // max_error nanoseconds.
    let mut i = 0;
    while i < DRM_TIMESTAMP_MAXRETRIES {
        let vbl_status = crate::amdgpu_display::amdgpu_crtc_get_scanout_position(
            adev, pipe, in_vblank_irq, &mut vpos, &mut hpos, Some(&mut stime), Some(&mut etime), &mode);

        // Return as no-op if scanout query unsupported or failed.
        if !vbl_status {
            return false;
        }

        // Compute uncertainty in timestamp of scanout position query.
        duration_ns = (etime - stime) as i32 * 1000;

        // Accept result with <  max_error nsecs timing uncertainty.
        if duration_ns <= *max_error {
            break;
        }
        i += 1;
    }

    // Return upper bound of timestamp precision error.
    *max_error = duration_ns;

    // Convert scanout position into elapsed time at raw_time query since
    // start of scanout at first display scanline. delta_ns can be negative
    // if start of scanout hasn't happened yet.
    let delta_ns = 1_000_000i64 * (vpos as i64 * mode.crtc_htotal as i64 + hpos as i64) / mode.crtc_clock as i64;

    // Subtract time delta from raw timestamp to get final vblank_time
    // timestamp for end of vblank.
    *vblank_time = (etime as i64 * 1000 - delta_ns).max(0) as u64 / 1000;

    true
}

/// `drm_get_last_vbltimestamp` / `drm_crtc_get_last_vbltimestamp`: the
/// high-precision timestamp, or now when it failed.
fn drm_get_last_vbltimestamp(adev: &mut amdgpu_device, pipe: usize, tvblank: &mut u64, in_vblank_irq: bool) -> bool {
    // Define requested maximum error on timestamps (nanoseconds).
    let mut max_error = drm_timestamp_precision * 1000;

    // Query driver if possible and precision timestamping enabled.
    let ret = max_error > 0
        && drm_crtc_vblank_helper_get_vblank_timestamp_internal(adev, pipe, &mut max_error, tvblank, in_vblank_irq);

    // GPU high precision timestamp query unsupported or failed. Return
    // current monotonic/gettimeofday timestamp as best estimate.
    if !ret {
        *tvblank = crate::linux::now_us();
    }

    ret
}

/// `drm_crtc_vblank_count`.
pub fn drm_crtc_vblank_count(adev: &mut amdgpu_device, crtc: usize) -> u64 {
    drm_vblank_count(adev, crtc)
}

/// `drm_vblank_count_and_time`.
fn drm_vblank_count_and_time(adev: &mut amdgpu_device, pipe: usize, vblanktime: &mut u64) -> u64 {
    let v = vblank(adev, pipe);
    *vblanktime = v.time;
    v.count
}

/// `send_vblank_event`: the event with its count and time, for the
/// driver's loop to hand on.
fn send_vblank_event(adev: &mut amdgpu_device, pipe: usize, event: u32, seq: u64, now: u64) {
    adev.dm.flip_events.push(drm_flip_event { event, crtc: pipe, sequence: seq, time_us: now });
}

/// `drm_crtc_send_vblank_event`.
pub fn drm_crtc_send_vblank_event(adev: &mut amdgpu_device, crtc: usize, event: u32) {
    let mut now = 0;
    let seq = drm_vblank_count_and_time(adev, crtc, &mut now);
    send_vblank_event(adev, crtc, event, seq, now);
}

/// `__enable_vblank` → `amdgpu_dm_crtc_enable_vblank`.
fn __enable_vblank(adev: &mut amdgpu_device, pipe: usize) -> i32 {
    crate::amdgpu_dm::amdgpu_dm_crtc_set_vblank(adev, pipe, true)
}

/// `drm_vblank_enable`.
fn drm_vblank_enable(adev: &mut amdgpu_device, pipe: usize) -> i32 {
    let mut ret = 0;

    if !vblank(adev, pipe).enabled {
        // Enable vblank irqs under vblank_time_lock protection. All vblank
        // count & timestamp updates are held off until we are done
        // reinitializing master counter and timestamps. Filtercode in
        // drm_handle_vblank() will prevent double-accounting of same
        // vblank interval.
        ret = __enable_vblank(adev, pipe);
        if ret != 0 {
            vblank(adev, pipe).refcount -= 1;
        } else {
            drm_update_vblank_count(adev, pipe, false);
            vblank(adev, pipe).enabled = true;
        }
    }

    ret
}

/// `drm_vblank_get`.
fn drm_vblank_get(adev: &mut amdgpu_device, pipe: usize) -> i32 {
    let mut ret = 0;

    // Going from 0->1 means we have to enable interrupts again
    vblank(adev, pipe).refcount += 1;
    if vblank(adev, pipe).refcount == 1 {
        ret = drm_vblank_enable(adev, pipe);
    } else if !vblank(adev, pipe).enabled {
        vblank(adev, pipe).refcount -= 1;
        ret = -22;
    }

    ret
}

/// `drm_crtc_vblank_get`.
pub fn drm_crtc_vblank_get(adev: &mut amdgpu_device, crtc: usize) -> i32 {
    drm_vblank_get(adev, crtc)
}

/// `drm_vblank_put`: the last user schedules the interrupt disable.
fn drm_vblank_put(adev: &mut amdgpu_device, pipe: usize) {
    let v = vblank(adev, pipe);
    let vblank_offdelay = v.config.offdelay_ms;

    if v.refcount == 0 {
        Line::new().s("WARNING: drm_vblank_put on crtc ").dec(pipe as u64).s(" without a reference").say();
        return;
    }

    // Last user schedules interrupt disable
    v.refcount -= 1;
    if v.refcount == 0 {
        if vblank_offdelay == 0 {
            return;
        } else if vblank_offdelay < 0 {
            vblank_disable_fn(adev, pipe);
        } else if !v.config.disable_immediate {
            crate::workqueue::mod_delayed_work(adev, crate::workqueue::work::vblank_disable { pipe },
                                               vblank_offdelay as u64 * 1000);
        }
    }
}

/// `drm_crtc_vblank_put`.
pub fn drm_crtc_vblank_put(adev: &mut amdgpu_device, crtc: usize) {
    drm_vblank_put(adev, crtc);
}

/// `drm_crtc_vblank_off`: the interrupt off and kept off until
/// `drm_crtc_vblank_on`. Its event list holds what
/// `drm_crtc_arm_vblank_event` queued, which DCN's flips do not use: their
/// event waits on the CRTC (`acrtc->event`).
pub fn drm_crtc_vblank_off(adev: &mut amdgpu_device, crtc: usize) {
    // Avoid redundant vblank disables without previous
    // drm_crtc_vblank_on(); an atomic driver always disables.
    drm_vblank_disable_and_save(adev, crtc);

    // Prevent subsequent drm_vblank_get() from re-enabling the vblank
    // interrupt by bumping the refcount.
    if !vblank(adev, crtc).inmodeset {
        vblank(adev, crtc).refcount += 1;
        vblank(adev, crtc).inmodeset = true;
    }

    // Will be reset by the modeset helpers when re-enabling the crtc by
    // calling drm_calc_timestamping_constants().
    vblank(adev, crtc).hwmode.crtc_clock = 0;
}

/// `drm_crtc_vblank_on_config`: the configuration, and the interrupt back
/// on when it has users or is never to be disabled.
pub fn drm_crtc_vblank_on_config(adev: &mut amdgpu_device, crtc: usize, config: &drm_vblank_crtc_config) {
    let v = vblank(adev, crtc);
    v.config = *config;

    // Drop our private "prevent drm_vblank_get" refcount
    if v.inmodeset {
        v.refcount -= 1;
        v.inmodeset = false;
    }

    drm_reset_vblank_timestamp(adev, crtc);

    // re-enable interrupts if there are users left, or the user wishes
    // vblank interrupts to be enabled all the time.
    let v = *vblank(adev, crtc);
    if (v.refcount != 0 || v.config.offdelay_ms == 0) && drm_vblank_enable(adev, crtc) != 0 {
        Line::new().s("WARNING: drm_crtc_vblank_on_config: vblank enable failed").say();
    }
}

/// `drm_handle_vblank` for `drm_crtc_handle_vblank`: count it. Vblank wait
/// queues and their events join with vblank waits.
pub fn drm_crtc_handle_vblank(adev: &mut amdgpu_device, crtc: usize) -> bool {
    // Vblank irq handling disabled. Nothing to do.
    if !vblank(adev, crtc).enabled {
        return false;
    }

    drm_update_vblank_count(adev, crtc, true);

    // With instant-off, we defer disabling the interrupt until after we
    // finish processing the following vblank after all events have been
    // signaled.
    let v = *vblank(adev, crtc);
    let disable_irq = v.config.disable_immediate && v.config.offdelay_ms > 0 && v.refcount == 0;

    if disable_irq {
        vblank_disable_fn(adev, crtc);
    }

    true
}
