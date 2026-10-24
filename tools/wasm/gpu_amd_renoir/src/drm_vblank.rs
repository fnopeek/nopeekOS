//! drm/drm_vblank.c (Linux 6.18.55), the part a CRTC of this driver needs:
//! the reference-counted vblank interrupt with its disable timer, the
//! count, and page-flip completion events.
#![allow(dead_code, non_camel_case_types)]

use crate::amdgpu_device::amdgpu_device;
use crate::linux::Line;

/// `struct drm_vblank_crtc`, the members reached. `count` counts the
/// vblanks handled; the hardware frame counter (`get_vblank_counter`) joins
/// with vblank waits.
#[derive(Clone, Copy, Default, Debug)]
pub struct drm_vblank_crtc {
    pub count: u64,
    /// The timestamp of the last vblank, in microseconds.
    pub time: u64,
    pub refcount: u32,
    pub enabled: bool,
    pub inmodeset: bool,
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

/// `__enable_vblank` → `amdgpu_dm_crtc_enable_vblank`.
fn __enable_vblank(adev: &mut amdgpu_device, pipe: usize) -> i32 {
    crate::amdgpu_dm::amdgpu_dm_crtc_set_vblank(adev, pipe, true)
}

/// `__disable_vblank` → `amdgpu_dm_crtc_disable_vblank`.
fn __disable_vblank(adev: &mut amdgpu_device, pipe: usize) {
    crate::amdgpu_dm::amdgpu_dm_crtc_set_vblank(adev, pipe, false);
}

/// `drm_update_vblank_count`: a handled vblank interrupt counts one.
fn drm_update_vblank_count(adev: &mut amdgpu_device, pipe: usize, in_vblank_irq: bool) {
    let v = vblank(adev, pipe);
    if in_vblank_irq {
        v.count += 1;
        v.time = crate::linux::now_us();
    }
}

/// `drm_vblank_enable`.
fn drm_vblank_enable(adev: &mut amdgpu_device, pipe: usize) -> i32 {
    let mut ret = 0;

    if !vblank(adev, pipe).enabled {
        // Enable vblank irqs under vblank_time_lock protection. All vblank
        // count & timestamp updates are held off until we are done
        // reinitializing master counter and timestamps.
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

/// `vblank_disable_fn`: the disable timer.
pub fn vblank_disable_fn(adev: &mut amdgpu_device, pipe: usize) {
    let v = vblank(adev, pipe);
    if v.refcount == 0 && v.enabled {
        drm_vblank_disable_and_save(adev, pipe);
    }
}

/// `drm_vblank_disable_and_save`.
fn drm_vblank_disable_and_save(adev: &mut amdgpu_device, pipe: usize) {
    // Update vblank count and disable vblank interrupts only if the
    // interrupts were enabled.
    if !vblank(adev, pipe).enabled {
        return;
    }

    drm_update_vblank_count(adev, pipe, false);
    __disable_vblank(adev, pipe);
    vblank(adev, pipe).enabled = false;
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

    // drm_reset_vblank_timestamp: the count carries on from the last.

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

/// `drm_crtc_send_vblank_event`: the event with the current vblank count
/// and time, for the driver's loop to hand on.
pub fn drm_crtc_send_vblank_event(adev: &mut amdgpu_device, crtc: usize, event: u32) {
    let v = *vblank(adev, crtc);
    adev.dm.flip_events.push(drm_flip_event { event, crtc, sequence: v.count, time_us: v.time });
}
