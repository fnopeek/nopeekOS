//! gt/intel_gt_pm.c (Linux 6.18.55): the GT wakeref (park/unpark), PM
//! init, resume and suspend.
//!
//! The wakeref is a counter; the last put parks synchronously, as
//! `intel_wakeref_put` does when it gets the mutex.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::linux;
use crate::regs::*;

/// `__gt_unpark`: hold the GT IRQ display power domain while the GT is
/// busy (keeps DMC from cycling DC states during submission), then
/// unpark RC6 and RPS. PMU, GuC busyness and request retirement are
/// software.
fn __gt_unpark(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    crate::intel_display_power::intel_display_power_get(i915, POWER_DOMAIN_GT_IRQ);
    gt.awake = true;

    crate::intel_rc6::intel_rc6_unpark(i915, gt);
    crate::intel_rps::intel_rps_unpark(i915, gt);
}

/// `__gt_park`.
fn __gt_park(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let wakeref = core::mem::replace(&mut gt.awake, false);

    crate::intel_rps::intel_rps_park(i915, gt);
    crate::intel_rc6::intel_rc6_park(i915, gt);

    if wakeref {
        crate::intel_display_power::intel_display_power_put_async(i915, POWER_DOMAIN_GT_IRQ);
    }
}

/// `intel_gt_pm_get`.
pub fn intel_gt_pm_get(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    gt.wakeref_count += 1;
    if gt.wakeref_count == 1 {
        __gt_unpark(i915, gt);
    }
}

/// `intel_gt_pm_put`.
pub fn intel_gt_pm_put(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if gt.wakeref_count == 0 {
        linux::say("warning: GT wakeref underflow");
        return;
    }
    gt.wakeref_count -= 1;
    if gt.wakeref_count == 0 {
        __gt_park(i915, gt);
    }
}

/// `intel_gt_pm_is_awake`.
pub fn intel_gt_pm_is_awake(gt: &intel_gt) -> bool {
    gt.wakeref_count != 0
}

/// `user_forcewake`: forcewake references userspace took (none here).
fn user_forcewake(i915: &mut drm_i915_private, gt: &mut intel_gt, suspend: bool) {
    let count = gt.user_wakeref;
    if count == 0 {
        return;
    }
    intel_gt_pm_get(i915, gt);
    if suspend {
        gt.wakeref_count -= count;
    } else {
        gt.wakeref_count += count;
    }
    intel_gt_pm_put(i915, gt);
}

/// `intel_gt_pm_init`.
pub fn intel_gt_pm_init(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    crate::intel_rc6::intel_rc6_init(i915, gt);
    crate::intel_rps::intel_rps_init(i915, gt);
}

/// `reset_engines`.
fn reset_engines(i915: &mut drm_i915_private, gt: &intel_gt) -> bool {
    if crate::intel_reset::intel_gt_gpu_reset_clobbers_display() {
        return false;
    }
    crate::intel_reset::intel_gt_reset_all_engines(i915, gt).is_ok()
}

/// `gt_sanitize`: assume the hardware came back at defaults and reset
/// it. No GuC to prepare; `intel_gt_check_clock_frequency` is a debug
/// check.
fn gt_sanitize(i915: &mut drm_i915_private, gt: &mut intel_gt, force: bool) {
    i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);

    if crate::intel_gt::intel_gt_is_wedged(gt) {
        linux::say("not ported yet: intel_gt_unset_wedged");
    }

    if gt.engines_initialized {
        for e in 0..gt.engine.len() {
            if gt.engine[e].is_some() {
                crate::intel_execlists_submission::execlists_reset_prepare(i915, gt, e);
                crate::intel_execlists_submission::execlists_sanitize(i915, gt, e);
            }
        }
    }

    if reset_engines(i915, gt) || force {
        // __intel_engine_reset: reset.rewind, and the kernel context lost.
        for e in 0..gt.engine.len() {
            if gt.engine[e].is_none() {
                continue;
            }
            if gt.engines_initialized {
                crate::intel_execlists_submission::execlists_reset_rewind(i915, gt, e);
            }
            if let Some(x) = gt.engine[e].as_mut() {
                x.serial = x.serial.wrapping_add(1);
            }
        }
    }

    if gt.engines_initialized {
        for e in 0..gt.engine.len() {
            if gt.engine[e].is_some() {
                crate::intel_execlists_submission::execlists_reset_finish(i915, gt, e);
            }
        }
    }

    crate::intel_rps::intel_rps_sanitize(i915, gt);

    i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_ALL);
}

/// `intel_gt_pm_fini`.
pub fn intel_gt_pm_fini(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    crate::intel_rc6::intel_rc6_fini(i915, gt);
}

/// `intel_gt_resume`: sanitize, re-initialise the hardware, then enable
/// RPS, the LLC table and RC6, with the GT held awake throughout.
pub fn intel_gt_resume(i915: &mut drm_i915_private, gt: &mut intel_gt) -> Result<(), i32> {
    const EIO: i32 = 5;

    if crate::intel_gt::intel_gt_has_unrecoverable_error(gt) {
        return Err(-EIO);
    }

    gt_sanitize(i915, gt, true);

    intel_gt_pm_get(i915, gt);

    i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);
    crate::intel_rc6::intel_rc6_sanitize(i915, gt);
    let mut err = Ok(());
    if crate::intel_gt::intel_gt_is_wedged(gt) {
        err = Err(-EIO);
    } else {
        match crate::intel_gt::intel_gt_init_hw(i915, gt) {
            Err(e) => {
                linux::drm_err("Failed to initialize GPU, declaring it wedged!");
                err = Err(e);
                crate::intel_gt::intel_gt_set_wedged(i915, gt);
            }
            Ok(()) => {
                crate::intel_rps::intel_rps_enable(i915, gt);
                crate::intel_llc::intel_llc_enable(i915, gt);

                let mut ok = true;
                for e in 0..gt.engine.len() {
                    if gt.engine[e].is_none() || !gt.engines_initialized {
                        continue;
                    }
                    crate::intel_engine_pm::intel_engine_pm_get(i915, gt, e);
                    // The kernel context is lost.
                    if let Some(x) = gt.engine[e].as_mut() {
                        x.serial = x.serial.wrapping_add(1);
                    }
                    let r = crate::intel_engine_cs::intel_engine_resume(i915, gt, e);
                    crate::intel_engine_pm::intel_engine_pm_put(i915, gt, e);
                    if let Err(x) = r {
                        crate::linux::Line::new().s("Failed to restart ")
                            .s(crate::intel_execlists_submission::engine_name(gt, e)).s(" (")
                            .dec((-x) as u64).s(")").say();
                        err = Err(x);
                        crate::intel_gt::intel_gt_set_wedged(i915, gt);
                        ok = false;
                        break;
                    }
                }

                if ok {
                    crate::intel_rc6::intel_rc6_enable(i915, gt);

                    user_forcewake(i915, gt, false);
                }
            }
        }
    }

    i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_ALL);
    intel_gt_pm_put(i915, gt);
    // intel_gt_bind_context_set_ready: MTL's bind context only.
    err
}

/// `wait_for_suspend`: retire everything; if the GT does not idle in time,
/// wedge it so the engines are quiet, and retire what that cancelled.
fn wait_for_suspend(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if !intel_gt_pm_is_awake(gt) {
        return;
    }
    if crate::intel_gt_requests::intel_gt_wait_for_idle(i915, gt, crate::intel_gt_requests::I915_GT_SUSPEND_IDLE_TIMEOUT_MS)
        .is_err() {
        linux::say("GT did not idle for suspend; wedging");
        crate::intel_gt::intel_gt_set_wedged(i915, gt);
        let _ = crate::intel_gt_requests::intel_gt_wait_for_idle(i915, gt, 0);
    }
    // intel_gt_pm_wait_for_idle: the last put parks synchronously here.
}

/// `intel_gt_suspend_prepare`.
pub fn intel_gt_suspend_prepare(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    user_forcewake(i915, gt, true);
    wait_for_suspend(i915, gt);
}

/// `intel_gt_suspend_late` with `pm_suspend_target() == PM_SUSPEND_TO_IDLE`:
/// the device stays powered (the display keeps scanning out), so power
/// management stays on — RC6 armed and parked, RPS at the idle frequency.
/// The full-suspend branch (RPS, RC6 and LLC off, then `gt_sanitize`)
/// would leave the GT awake for good once the module is gone.
pub fn intel_gt_suspend_late(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    wait_for_suspend(i915, gt);
    if gt.awake {
        linux::say("warning: GT still awake at suspend");
    }
}
