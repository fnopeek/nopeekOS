//! i915_request.c (Linux 6.18.55): a request on a context — its seqno,
//! its span of the ring, committing it, handing it to the backend,
//! completion and retirement.
//!
//! Requests sit in a fixed table in `intel_gt` and are referred to by
//! index. Fences, the scheduler's dependency graph and RCU are bookkeeping
//! and not kept: requests here are created and submitted one after the
//! other, so the order a dependency would enforce is the submission order.
//! A table entry is freed when the request is retired and nothing (the
//! execlists ports, a caller) holds it any more.

use crate::gen8_engine_cs::Cs;
use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::linux::Line;
use crate::regs::*;

pub const MAX_REQUESTS: usize = 32;

const ENOMEM: i32 = 12;
const ENOSPC: i32 = 28;
const EIO: i32 = 5;

/// `struct i915_request`.
#[derive(Clone, Copy, Default)]
pub struct i915_request {
    pub used: bool,
    /// `rq->context`, `rq->engine` (by id).
    pub ce: u16,
    pub engine: u32,
    /// `rq->fence.seqno`.
    pub seqno: u32,
    /// `rq->hwsp_seqno`: the timeline's HWSP slot when the request was made.
    pub hwsp_seqno: u32,
    pub head: u32,
    pub infix: u32,
    pub postfix: u32,
    pub tail: u32,
    pub wa_tail: u32,
    pub reserved_space: u32,
    /// `rq->sched.attr.priority`.
    pub prio: i32,
    /// `rq->fence.error`.
    pub error: i32,
    /// I915_FENCE_FLAG_PQUEUE, I915_FENCE_FLAG_ACTIVE, the fence signaled,
    /// retired.
    pub pqueue: bool,
    pub active: bool,
    pub signaled: bool,
    pub retired: bool,
    /// I915_FENCE_FLAG_BOOST.
    pub boost: bool,
    /// References besides the timeline's (`i915_request_get`).
    pub refs: u32,
}

/// `i915_seqno_passed`.
pub fn i915_seqno_passed(seq1: u32, seq2: u32) -> bool {
    (seq1.wrapping_sub(seq2) as i32) >= 0
}

/// `*rq->hwsp_seqno`.
fn hwsp_seqno(gt: &intel_gt, rq: usize) -> u32 {
    let ce = gt.requests[rq].ce as usize;
    gt.contexts[ce].timeline.hwsp_obj.and_then(|o| o.map()).map(|d| d.r32(gt.requests[rq].hwsp_seqno)).unwrap_or(0)
}

/// `__i915_request_is_complete`.
pub fn __i915_request_is_complete(gt: &intel_gt, rq: usize) -> bool {
    i915_seqno_passed(hwsp_seqno(gt, rq), gt.requests[rq].seqno)
}

/// `__i915_request_has_started`: the seqno before ours has been written.
pub fn __i915_request_has_started(gt: &intel_gt, rq: usize) -> bool {
    i915_seqno_passed(hwsp_seqno(gt, rq), gt.requests[rq].seqno.wrapping_sub(1))
}

/// `i915_request_get`.
pub fn i915_request_get(gt: &mut intel_gt, rq: usize) -> usize {
    gt.requests[rq].refs += 1;
    rq
}

/// `i915_request_put`: the entry is free once retired and unreferenced.
pub fn i915_request_put(gt: &mut intel_gt, rq: usize) {
    let r = &mut gt.requests[rq];
    if r.refs > 0 {
        r.refs -= 1;
    }
    if r.refs == 0 && r.retired {
        r.used = false;
    }
}

/// `i915_request_mark_complete`: the seqno is written into the HWSP slot.
fn i915_request_mark_complete(gt: &mut intel_gt, rq: usize) {
    let ce = gt.requests[rq].ce as usize;
    if let Some(d) = gt.contexts[ce].timeline.hwsp_obj.and_then(|o| o.map()) {
        d.w32(gt.requests[rq].hwsp_seqno, gt.requests[rq].seqno);
    }
}

/// `i915_request_set_error_once`.
fn i915_request_set_error_once(gt: &mut intel_gt, rq: usize, error: i32) -> bool {
    if gt.requests[rq].signaled {
        return false;
    }
    let old = gt.requests[rq].error;
    if old != 0 && old != -11 && old != -110 {
        return false;
    }
    gt.requests[rq].error = error;
    true
}

/// `i915_request_mark_eio`: an unfinished request fails with -EIO and is
/// marked complete. Returns whether it was unfinished.
pub fn i915_request_mark_eio(gt: &mut intel_gt, rq: usize) -> bool {
    if __i915_request_is_complete(gt, rq) {
        return false;
    }
    i915_request_set_error_once(gt, rq, -EIO);
    i915_request_mark_complete(gt, rq);
    true
}

/// `intel_ring_begin` + writing the block + `intel_ring_advance`: room is
/// made for `cs.len()` dwords plus the request's reserved space, wrapping
/// the ring with zeros (MI_NOOP) if the block does not fit before its end.
/// Waiting for space (`wait_for_space`) retires what has completed on the
/// context's timeline first.
pub fn intel_ring_emit(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize, cs: &Cs) -> Result<(), i32> {
    let at = intel_ring_begin(i915, gt, rq, cs.len() as u32)?;
    let ce = gt.requests[rq].ce as usize;
    if !gt.contexts[ce].ring.write(at, cs.as_slice()) {
        return Err(-ENOMEM);
    }
    Ok(())
}

/// `intel_ring_begin`: returns the byte offset the block goes to.
pub fn intel_ring_begin(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize, num_dwords: u32)
                        -> Result<u32, i32> {
    let ce = gt.requests[rq].ce as usize;
    let reserved = gt.requests[rq].reserved_space;
    let ring = gt.contexts[ce].ring;
    let remain_usable = ring.effective_size - ring.emit;
    let bytes = num_dwords * 4;
    let mut need_wrap = 0u32;
    let mut total_bytes = bytes + reserved;

    if total_bytes > remain_usable {
        let remain_actual = ring.size - ring.emit;
        if bytes > remain_usable {
            total_bytes += remain_actual;
            need_wrap = remain_actual | 1;
        } else {
            total_bytes = reserved + remain_actual;
        }
    }

    if total_bytes > gt.contexts[ce].ring.space {
        wait_for_space(i915, gt, ce, total_bytes)?;
    }

    let ring = &mut gt.contexts[ce].ring;
    if need_wrap != 0 {
        need_wrap &= !1;
        let zeros = [0u32; 256];
        let mut left = need_wrap / 4;
        let mut at = ring.emit;
        while left > 0 {
            let n = left.min(256);
            ring.write(at, &zeros[..n as usize]);
            at += n * 4;
            left -= n;
        }
        ring.space -= need_wrap;
        ring.emit = 0;
    }

    let at = ring.emit;
    ring.emit += bytes;
    ring.space -= bytes;
    Ok(at)
}

/// `wait_for_space`: retire completed requests on the context's timeline
/// until the ring has `bytes` free; Linux would wait for the request that
/// frees enough, which here would only arrive by retiring it.
fn wait_for_space(i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize, bytes: u32) -> Result<(), i32> {
    if gt.contexts[ce].ring.intel_ring_update_space() >= bytes {
        return Ok(());
    }
    crate::intel_gt_requests::retire_requests(i915, gt, ce);
    if gt.contexts[ce].ring.intel_ring_update_space() >= bytes {
        return Ok(());
    }
    Line::new().s("ring full: ").dec(bytes as u64).s(" bytes wanted, ")
        .dec(gt.contexts[ce].ring.space as u64).s(" free").say();
    Err(-ENOSPC)
}

/// `__i915_request_create`: seqno from the timeline, the ring position, the
/// backend's preamble (`execlists_request_alloc`), and onto the timeline.
pub fn __i915_request_create(i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize) -> Result<usize, i32> {
    crate::intel_context::__intel_context_pin(gt, ce);

    let Some(rq) = gt.requests.iter().position(|r| !r.used) else {
        crate::intel_context::intel_context_unpin(gt, ce);
        return Err(-ENOMEM);
    };
    let engine = gt.contexts[ce].engine;
    let seqno = crate::intel_timeline::intel_timeline_get_seqno(&mut gt.contexts[ce].timeline);
    let fini_dw = gt.engine[engine as usize].map(|e| e.emit_fini_breadcrumb_dw).unwrap_or(0);

    gt.requests[rq] = i915_request {
        used: true,
        ce: ce as u16,
        engine,
        seqno,
        hwsp_seqno: gt.contexts[ce].timeline.hwsp_seqno,
        reserved_space: 2 * fini_dw * 4,
        head: gt.contexts[ce].ring.emit,
        ..Default::default()
    };

    if let Err(e) = crate::intel_execlists_submission::execlists_request_alloc(i915, gt, rq) {
        gt.contexts[ce].ring.emit = gt.requests[rq].head;
        gt.requests[rq].used = false;
        crate::intel_context::intel_context_unpin(gt, ce);
        return Err(e);
    }

    gt.requests[rq].infix = gt.contexts[ce].ring.emit;

    crate::intel_context::intel_context_mark_active(gt, ce);
    if !gt.contexts[ce].timeline.add_request(rq as u16) {
        crate::linux::say("warning: timeline request list full");
    }
    Ok(rq)
}

/// `i915_request_create`: an older completed request on the timeline is
/// retired first; the context is entered around the creation.
pub fn i915_request_create(i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize) -> Result<usize, i32> {
    let tl = gt.contexts[ce].timeline;
    if tl.nrequests > 1 {
        if let Some(first) = tl.first_request() {
            i915_request_retire(i915, gt, first as usize);
        }
    }
    crate::intel_context::intel_context_enter(i915, gt, ce);
    let rq = __i915_request_create(i915, gt, ce);
    crate::intel_context::intel_context_exit(i915, gt, ce);
    rq
}

/// `__i915_request_commit`: the closing breadcrumb's space is taken now
/// (written at submission); the request becomes the timeline's last.
pub fn __i915_request_commit(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize) -> Result<(), i32> {
    let engine = gt.requests[rq].engine as usize;
    let fini_dw = gt.engine[engine].map(|e| e.emit_fini_breadcrumb_dw).unwrap_or(0);
    gt.requests[rq].reserved_space = 0;
    let at = intel_ring_begin(i915, gt, rq, fini_dw)?;
    let ce = gt.requests[rq].ce as usize;
    gt.requests[rq].postfix = gt.contexts[ce].ring.intel_ring_wrap(at);
    gt.contexts[ce].timeline.last_request = Some(rq as u16);
    Ok(())
}

/// `__i915_request_queue` → `submit_notify` → `engine->submit_request`.
pub fn __i915_request_queue(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize) {
    let engine = gt.requests[rq].engine as usize;
    crate::intel_execlists_submission::submit_request(i915, gt, engine, rq);
}

/// `i915_request_add`.
pub fn i915_request_add(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize) {
    if __i915_request_commit(i915, gt, rq).is_err() {
        crate::linux::say("warning: no ring space for the closing breadcrumb");
    }
    __i915_request_queue(i915, gt, rq);
}

/// `__i915_request_skip`: a failed request's payload becomes MI_NOOPs.
fn __i915_request_skip(gt: &mut intel_gt, rq: usize) {
    let r = gt.requests[rq];
    if r.infix == r.postfix {
        return;
    }
    let ce = r.ce as usize;
    let ring = gt.contexts[ce].ring;
    let zeros = [0u32; 256];
    let mut pos = r.infix;
    while pos != r.postfix {
        let end = if r.postfix > pos { r.postfix } else { ring.size };
        let n = ((end - pos) / 4).min(256);
        ring.write(pos, &zeros[..n as usize]);
        pos = ring.intel_ring_wrap(pos + n * 4);
    }
    gt.requests[rq].infix = r.postfix;
}

/// `__i915_request_submit`: a request that has not completed gets its
/// closing breadcrumb and joins the engine's active list. Returns whether
/// it goes to the hardware.
pub fn __i915_request_submit(_i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize) -> bool {
    let engine = gt.requests[rq].engine as usize;
    let mut result = false;

    if !__i915_request_is_complete(gt, rq) {
        let err = gt.requests[rq].error;
        if err != 0 && err != -11 && err != -110 {
            __i915_request_skip(gt, rq);
        }

        let ce = gt.requests[rq].ce as usize;
        let e = gt.engine[engine].unwrap_or_default();
        let tl = gt.contexts[ce].timeline;
        // hwsp_offset(rq): the timeline page's GGTT address and the slot.
        let page = tl.hwsp_offset & !(I915_GTT_PAGE_SIZE as u32 - 1);
        let hwsp = page + (gt.requests[rq].hwsp_seqno & (I915_GTT_PAGE_SIZE as u32 - 1));
        let preempt = e.status_page.map(|v| v.ggtt_offset()).unwrap_or(0) + I915_GEM_HWS_PREEMPT_ADDR as u32;
        let fb = crate::gen8_engine_cs::emit_fini_breadcrumb(&e, gt.requests[rq].seqno, hwsp, preempt);
        let ring = gt.contexts[ce].ring;
        let postfix = gt.requests[rq].postfix;
        ring.write(postfix, fb.cs.as_slice());
        gt.requests[rq].tail = ring.intel_ring_wrap(postfix + fb.tail_dw as u32 * 4);
        gt.requests[rq].wa_tail = ring.intel_ring_wrap(postfix + fb.cs.len() as u32 * 4);

        if let Some(e) = gt.engine[engine].as_mut() {
            e.serial = e.serial.wrapping_add(1);
        }
        result = true;
        crate::intel_execlists_submission::add_to_engine(gt, engine, rq);
    }
    gt.requests[rq].pqueue = false;
    gt.requests[rq].active = true;
    result
}

/// `__i915_request_unsubmit`.
pub fn __i915_request_unsubmit(gt: &mut intel_gt, rq: usize) {
    gt.requests[rq].active = false;
}

/// `i915_request_retire`: a completed request releases its ring space,
/// leaves the engine and the timeline, and lets go of its context.
pub fn i915_request_retire(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize) -> bool {
    if !gt.requests[rq].used || gt.requests[rq].retired || !__i915_request_is_complete(gt, rq) {
        return false;
    }
    let ce = gt.requests[rq].ce as usize;
    if gt.contexts[ce].timeline.first_request() != Some(rq as u16) {
        return false;
    }

    gt.contexts[ce].ring.head = gt.requests[rq].postfix;
    gt.requests[rq].signaled = true;

    if core::mem::replace(&mut gt.requests[rq].boost, true) {
        crate::intel_rps::intel_rps_dec_waiters(gt);
    }

    let engine = gt.requests[rq].engine as usize;
    crate::intel_execlists_submission::remove_from_engine(gt, engine, rq);

    gt.contexts[ce].timeline.pop_first();
    if gt.contexts[ce].timeline.last_request == Some(rq as u16) {
        gt.contexts[ce].timeline.last_request = None;
    }

    crate::intel_context::intel_context_exit(i915, gt, ce);
    crate::intel_context::intel_context_unpin(gt, ce);

    gt.requests[rq].retired = true;
    if gt.requests[rq].refs == 0 {
        gt.requests[rq].used = false;
    }
    true
}
