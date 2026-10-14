//! gt/intel_gt_requests.c (Linux 6.18.55): retiring completed requests and
//! waiting for the GT to go idle, with the submission flush and engine
//! interrupt delivery it relies on (`intel_engine_flush_submission`,
//! `intel_engine_cs_irq`). Linux retires from workers and signals from
//! interrupts; here the waiter does both in turn, sleeping on the
//! device's interrupt between rounds.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::linux;

/// `I915_GEM_IDLE_TIMEOUT` (HZ / 5).
pub const I915_GEM_IDLE_TIMEOUT_MS: u64 = 1000 / 5;
/// `I915_GT_SUSPEND_IDLE_TIMEOUT` (HZ / 2).
pub const I915_GT_SUSPEND_IDLE_TIMEOUT_MS: u64 = 1000 / 2;

const ETIME: i32 = 62;

/// `intel_engine_cs_irq` for every engine: the interrupts the GT handler
/// recorded per class and instance go to `execlists_irq_handler`.
pub fn intel_engines_cs_irq(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    for e in 0..gt.engine.len() {
        let Some(x) = gt.engine[e] else { continue };
        let (c, i) = (x.class as usize, x.instance as usize);
        if c >= i915.irq.gt.engine_iir.len() || i >= i915.irq.gt.engine_iir[c].len() {
            continue;
        }
        let iir = core::mem::take(&mut i915.irq.gt.engine_iir[c][i]);
        if iir != 0 && gt.engines_initialized {
            crate::intel_execlists_submission::execlists_irq_handler(i915, gt, e, iir);
        }
    }
}

/// `intel_engine_flush_submission`: run the engine's tasklet now. True if
/// the engine has something on its ports.
pub fn intel_engine_flush_submission(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) -> bool {
    crate::intel_execlists_submission::execlists_submission_tasklet(i915, gt, e);
    let x = gt.engine[e].map(|x| x.execlists).unwrap_or_default();
    x.inflight[x.active].is_some() || x.pending[0].is_some()
}

/// `retire_requests`: the context's timeline, oldest first, up to the
/// first request that has not completed. True if none is left.
pub fn retire_requests(i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize) -> bool {
    while let Some(rq) = gt.contexts[ce].timeline.first_request() {
        if !crate::i915_request::i915_request_retire(i915, gt, rq as usize) {
            return false;
        }
    }
    gt.contexts[ce].timeline.last_request.is_none()
}

/// `engine_active`: the kernel context still has requests.
fn engine_active(gt: &intel_gt, e: usize) -> bool {
    gt.engine[e].and_then(|x| x.kernel_context)
        .map(|ce| gt.contexts[ce as usize].timeline.nrequests != 0).unwrap_or(false)
}

/// `flush_submission`: deliver pending interrupts, run every tasklet.
fn flush_submission(i915: &mut drm_i915_private, gt: &mut intel_gt) -> bool {
    if !crate::intel_gt_pm::intel_gt_pm_is_awake(gt) {
        return false;
    }
    intel_engines_cs_irq(i915, gt);
    let mut active = false;
    for e in 0..gt.engine.len() {
        if gt.engine[e].is_none() || !gt.engines_initialized {
            continue;
        }
        intel_engine_flush_submission(i915, gt, e);
        active |= engine_active(gt, e);
    }
    active
}

/// `intel_gt_retire_requests_timeout`, one round: retire on every active
/// timeline; the number still busy.
pub fn intel_gt_retire_requests(i915: &mut drm_i915_private, gt: &mut intel_gt) -> u32 {
    let mut active_count = 0;
    flush_submission(i915, gt);
    for ce in 0..gt.contexts.len() {
        if !gt.contexts[ce].used || gt.contexts[ce].timeline.active_count == 0 {
            continue;
        }
        if !retire_requests(i915, gt, ce) {
            active_count += 1;
        }
    }
    if flush_submission(i915, gt) {
        active_count += 1;
    }
    active_count
}

/// `intel_gt_wait_for_idle`: retire until nothing is left, sleeping on the
/// interrupt between rounds; -ETIME when `timeout_ms` passes first.
pub fn intel_gt_wait_for_idle(i915: &mut drm_i915_private, gt: &mut intel_gt, timeout_ms: u64) -> Result<(), i32> {
    if !crate::intel_gt_pm::intel_gt_pm_is_awake(gt) {
        return Ok(());
    }
    let t0 = linux::now_us();
    loop {
        if intel_gt_retire_requests(i915, gt) == 0 {
            return Ok(());
        }
        if linux::now_us() - t0 > timeout_ms * 1000 {
            return Err(-ETIME);
        }
        crate::i915_irq::intel_irq_wait(i915, 1);
    }
}
