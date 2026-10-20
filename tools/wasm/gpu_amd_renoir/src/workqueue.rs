//! The kernel's workqueues and timers on the module's one thread: work is
//! queued as a value and runs when the driver loop or a waiter calls
//! `run_pending`, timers are deadlines checked there. Work therefore runs
//! no earlier than in Linux and at the latest when someone waits for its
//! result, which is when Linux's worker would have run it too.

use crate::amdgpu_device::amdgpu_device;
use alloc::vec::Vec;

/// A work item, in place of `struct work_struct` and the structure
/// embedding it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum work {
    /// `drm_sched_run_job_work` of the scheduler of ring `ring`.
    sched_run_job { ring: usize },
    /// `drm_sched_free_job_work`.
    sched_free_job { ring: usize },
    /// `drm_sched_job_timedout` (the `work_tdr` delayed work).
    sched_timedout { ring: usize },
    /// `amdgpu_device_delay_enable_gfx_off` (`gfx.gfx_off_delay_work`).
    gfx_off_delay,
}

/// Queued work and armed timers.
#[derive(Default)]
pub struct workqueue {
    queued: Vec<work>,
    /// Delayed work: when it is due.
    delayed: Vec<(u64, work)>,
}

/// `queue_work`: false when it was queued already.
pub fn queue_work(adev: &mut amdgpu_device, w: work) -> bool {
    if adev.wq.queued.contains(&w) {
        return false;
    }
    adev.wq.queued.push(w);
    true
}

/// `mod_delayed_work`: due `delay_us` from now, rearmed if armed.
pub fn mod_delayed_work(adev: &mut amdgpu_device, w: work, delay_us: u64) {
    let due = crate::linux::now_us().saturating_add(delay_us);
    adev.wq.delayed.retain(|e| e.1 != w);
    adev.wq.delayed.push((due, w));
}

/// `cancel_delayed_work`: true when it was pending.
pub fn cancel_delayed_work(adev: &mut amdgpu_device, w: work) -> bool {
    let n = adev.wq.delayed.len();
    adev.wq.delayed.retain(|e| e.1 != w);
    adev.wq.queued.retain(|e| *e != w);
    n != adev.wq.delayed.len()
}

/// `cancel_work`: true when it was queued.
pub fn cancel_work(adev: &mut amdgpu_device, w: work) -> bool {
    let n = adev.wq.queued.len();
    adev.wq.queued.retain(|e| *e != w);
    n != adev.wq.queued.len()
}

/// `work_tdr.timer.expires`: when the delayed work is due, if armed.
pub fn delayed_work_expires(adev: &amdgpu_device, w: work) -> Option<u64> {
    adev.wq.delayed.iter().find(|e| e.1 == w).map(|e| e.0)
}

/// `delayed_work_pending`.
pub fn delayed_work_pending(adev: &amdgpu_device, w: work) -> bool {
    adev.wq.delayed.iter().any(|e| e.1 == w)
}

/// What is due: the timers that expired, then the queued work, until none
/// is left.
pub fn run_pending(adev: &mut amdgpu_device) {
    let now = crate::linux::now_us();
    for r in 0..adev.rings.len() {
        // amdgpu_fence_fallback
        if adev.rings[r].fence_drv.fallback_deadline_us.is_some_and(|d| now >= d) {
            adev.rings[r].fence_drv.fallback_deadline_us = None;
            crate::amdgpu_fence::amdgpu_fence_fallback(adev, r);
        }
    }
    let mut due = Vec::new();
    adev.wq.delayed.retain(|e| {
        if now >= e.0 {
            due.push(e.1);
            false
        } else {
            true
        }
    });
    for w in due {
        queue_work(adev, w);
    }
    while !adev.wq.queued.is_empty() {
        let w = adev.wq.queued.remove(0);
        match w {
            work::sched_run_job { ring } => crate::drm_sched::drm_sched_run_job_work(adev, ring),
            work::sched_free_job { ring } => crate::drm_sched::drm_sched_free_job_work(adev, ring),
            work::sched_timedout { ring } => crate::drm_sched::drm_sched_job_timedout(adev, ring),
            work::gfx_off_delay => crate::gfx_v9_0::amdgpu_device_delay_enable_gfx_off(adev),
        }
    }
}

/// Until `cond` holds or `timeout_us` passes (`i64::MAX` for never): the
/// time left, at least 1, or 0. Between checks the pending work runs and
/// the interrupt handler waits for the card, which is how every completion
/// reaches the module.
pub fn wait_until(adev: &mut amdgpu_device, timeout_us: i64, mut cond: impl FnMut(&mut amdgpu_device) -> bool)
                  -> i64 {
    let t0 = crate::linux::now_us();
    loop {
        run_pending(adev);
        let elapsed = (crate::linux::now_us() - t0) as i64;
        if cond(adev) {
            return (timeout_us - elapsed).max(1);
        }
        if elapsed >= timeout_us {
            return 0;
        }
        let left_ms = ((timeout_us - elapsed) / 1000).clamp(1, 10) as i32;
        crate::amdgpu_irq::amdgpu_irq_wait(adev, left_ms);
    }
}
