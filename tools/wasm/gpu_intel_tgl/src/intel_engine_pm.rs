//! gt/intel_engine_pm.c (Linux 6.18.55): an engine's wakeref. Unparking
//! takes the GT awake and scrubs the kernel context; parking first switches
//! the engine to its kernel context with a barrier request (so the last
//! user context's image is saved), and only parks once that has retired.
//! The last put parks synchronously, as `intel_wakeref_put` does when it
//! gets the mutex; heartbeats are not run.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::regs::*;

/// `__engine_unpark`.
fn __engine_unpark(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    crate::intel_gt_pm::intel_gt_pm_get(i915, gt);
    if let Some(x) = gt.engine[e].as_mut() {
        x.wakeref_gt = true;
    }

    if let Some(ce) = gt.engine[e].and_then(|x| x.kernel_context) {
        let ce = ce as usize;
        // The CSB is read from the status page, so flushing makes progress
        // without the interrupt; bounded, so a hung engine cannot hold this.
        let t0 = crate::linux::now_us();
        while gt.contexts[ce].inflight > 0 {
            if !crate::intel_gt_requests::intel_engine_flush_submission(i915, gt, e) {
                break;
            }
            if crate::linux::now_us() - t0 > crate::intel_gt_requests::I915_GEM_IDLE_TIMEOUT_MS * 1000 {
                crate::linux::say("warning: kernel context still in flight at unpark");
                break;
            }
        }
        crate::intel_lrc::lrc_reset(i915, gt, ce);
        let tl = gt.contexts[ce].timeline;
        if tl.seqno != tl.read_hwsp_seqno() {
            crate::linux::say("warning: kernel context seqno and HWSP disagree at unpark");
        }
    }
    // engine->unpark is NULL for execlists; breadcrumbs and heartbeat are
    // software.
}

/// `switch_to_kernel_context`: true when the engine may park now.
fn switch_to_kernel_context(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) -> bool {
    if crate::intel_gt::intel_gt_is_wedged(gt) {
        return true;
    }
    let Some(x) = gt.engine[e] else { return true };
    let Some(ce) = x.kernel_context else { return true };
    let ce = ce as usize;
    if x.wakeref_serial == x.serial {
        return true;
    }

    gt.contexts[ce].parking = true;
    let rq = match crate::i915_request::__i915_request_create(i915, gt, ce) {
        Ok(rq) => rq,
        Err(_) => {
            gt.contexts[ce].parking = false;
            return true;
        }
    };
    if let Some(y) = gt.engine[e].as_mut() {
        y.wakeref_serial = y.serial.wrapping_add(1);
    }
    gt.requests[rq].prio = I915_PRIORITY_BARRIER as i32;
    if crate::i915_request::__i915_request_commit(i915, gt, rq).is_err() {
        crate::linux::say("warning: no ring space for the parking request");
    }

    // __queue_and_release_pm: the request holds the GT awake and the
    // timeline active; the engine's park is deferred until it retires.
    crate::intel_gt_pm::intel_gt_pm_get(i915, gt);
    gt.contexts[ce].wakeref = true;
    gt.contexts[ce].timeline.active_count += 1;
    crate::i915_request::__i915_request_queue(i915, gt, rq);
    if let Some(y) = gt.engine[e].as_mut() {
        y.wakeref_count = 1;
    }

    gt.contexts[ce].parking = false;
    false
}

/// `__engine_park`.
fn __engine_park(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) -> bool {
    if !switch_to_kernel_context(i915, gt, e) {
        return false;
    }
    crate::intel_execlists_submission::execlists_park(gt, e);
    let held = gt.engine[e].as_mut().map(|x| core::mem::replace(&mut x.wakeref_gt, false)).unwrap_or(false);
    if held {
        crate::intel_gt_pm::intel_gt_pm_put(i915, gt);
    }
    true
}

/// `intel_engine_pm_get`.
pub fn intel_engine_pm_get(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let first = match gt.engine[e].as_mut() {
        Some(x) => {
            x.wakeref_count += 1;
            x.wakeref_count == 1
        }
        None => false,
    };
    if first {
        __engine_unpark(i915, gt, e);
    }
}

/// `intel_engine_pm_put`: the last put parks, unless parking had to queue
/// the switch to the kernel context first.
pub fn intel_engine_pm_put(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let last = match gt.engine[e].as_mut() {
        Some(x) if x.wakeref_count > 0 => {
            x.wakeref_count -= 1;
            x.wakeref_count == 0
        }
        _ => {
            crate::linux::say("warning: engine wakeref underflow");
            false
        }
    };
    if last {
        __engine_park(i915, gt, e);
    }
}

/// `intel_engine_pm_is_awake`.
pub fn intel_engine_pm_is_awake(gt: &intel_gt, e: usize) -> bool {
    gt.engine[e].map(|x| x.wakeref_count != 0).unwrap_or(false)
}
