//! i915_request.c (Linux 6.18.55): a request on a context — its seqno,
//! its span of the ring, committing it, handing it to the backend,
//! completion and retirement.
//!
//! Requests sit in a fixed table in `intel_gt` and are referred to by
//! index. A fence is named by its timeline (the context) and seqno; a
//! request's submit fence waits for the foreign fences it was given
//! (`i915_request_await`), and until they signal the committed request
//! waits in `gt.waiters` instead of the engine's queue. Later requests of
//! the same timeline wait behind it, as `__i915_request_add_to_timeline`
//! orders them. RCU is not kept. A table entry is freed when the request is
//! retired and nothing (the execlists ports, a caller) holds it any more.

use crate::gen8_engine_cs::Cs;
use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::linux::Line;
use crate::regs::*;

pub const MAX_REQUESTS: usize = 128;
/// Requests only the module's own work may use: a guest filling the table
/// must not keep the desktop's scenes and copies out.
pub const KERNEL_RESERVE: usize = 8;
/// `I915_PRIORITY_DISPLAY` (i915_priolist_types.h:24):
/// I915_CONTEXT_MAX_USER_PRIORITY + 3.
pub const I915_PRIORITY_DISPLAY: i32 = 1026;
/// Foreign fences one request may wait for (one per timeline) from its
/// objects' reservations; a request keeps two more, for its context's
/// previous request (single timeline, virtual engine).
pub const MAX_DEPS: usize = 8;
const REQ_DEPS: usize = MAX_DEPS + 2;

const ENOMEM: i32 = 12;
const ENOSPC: i32 = 28;
const EIO: i32 = 5;
const ECANCELED: i32 = 125;

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
    /// `rq->submit`: the foreign fences (context, seqno) it waits for, and
    /// whether it is held in `gt.waiters`.
    pub deps: [(u16, u32); REQ_DEPS],
    pub ndeps: u8,
    pub waiting: bool,
    /// `i915_request_await_execution`: the request (context, seqno) it is
    /// submitted together with, once that one is in an execlist port and has
    /// started (I915_EXEC_FENCE_SUBMIT).
    pub exec_dep: Option<(u16, u32)>,
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

/// `hwsp_offset(rq)`: the GGTT address of the request's seqno slot, the
/// timeline page's address and the slot.
pub fn hwsp_offset(gt: &intel_gt, rq: usize) -> u32 {
    let tl = gt.contexts[gt.requests[rq].ce as usize].timeline;
    let page = tl.hwsp_offset & !(I915_GTT_PAGE_SIZE as u32 - 1);
    page + (gt.requests[rq].hwsp_seqno & (I915_GTT_PAGE_SIZE as u32 - 1))
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

/// `i915_request_cancel` for a request still in the engine's queue: it
/// fails, and `__i915_request_submit` turns its payload into MI_NOOPs, so
/// it runs only its breadcrumbs and retires as usual. False once it went
/// to the hardware (no preemption is ported to take it back).
pub fn i915_request_cancel(gt: &mut intel_gt, rq: usize) -> bool {
    if !gt.requests[rq].pqueue || __i915_request_is_complete(gt, rq) {
        return false;
    }
    i915_request_set_error_once(gt, rq, -ECANCELED)
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
    if gt.contexts[ce].timeline.nrequests >= crate::intel_timeline::TIMELINE_MAX_REQUESTS {
        return Err(-ENOSPC);
    }
    crate::intel_context::__intel_context_pin(gt, ce);

    let Some(rq) = gt.requests.iter().position(|r| !r.used) else {
        crate::intel_context::intel_context_unpin(gt, ce);
        return Err(-ENOMEM);
    };
    let engine = gt.contexts[ce].engine;
    let seqno = crate::intel_timeline::intel_timeline_get_seqno(&mut gt.contexts[ce].timeline);
    let fini_dw = crate::intel_context::ce_engine(gt, ce).and_then(|e| gt.engine[e])
        .map(|e| e.emit_fini_breadcrumb_dw).unwrap_or(0);

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
    let fini_dw = crate::intel_context::rq_engine_cs(gt, rq).map(|e| e.emit_fini_breadcrumb_dw).unwrap_or(0);
    gt.requests[rq].reserved_space = 0;
    let at = intel_ring_begin(i915, gt, rq, fini_dw)?;
    let ce = gt.requests[rq].ce as usize;
    gt.requests[rq].postfix = gt.contexts[ce].ring.intel_ring_wrap(at);
    // __i915_request_add_to_timeline: on a virtual engine the previous
    // request may run on another sibling, so this one waits for it to
    // complete (only one request goes through a virtual engine at a time).
    if let Some(prev) = gt.contexts[ce].timeline.last_request.map(|p| p as usize)
        && crate::intel_execlists_submission::is_virtual(gt.requests[rq].engine)
        && !__i915_request_is_complete(gt, prev) {
        let (pce, pseq) = (gt.requests[prev].ce, gt.requests[prev].seqno);
        let r = &mut gt.requests[rq];
        if (r.ndeps as usize) < REQ_DEPS {
            r.deps[r.ndeps as usize] = (pce, pseq);
            r.ndeps += 1;
        }
    }
    gt.contexts[ce].timeline.last_request = Some(rq as u16);
    Ok(())
}

/// `__i915_request_queue` → `submit_notify` → `engine->submit_request`:
/// `virtual_submit_request` for a virtual engine.
pub fn __i915_request_queue(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize) {
    let engine = gt.requests[rq].engine;
    if crate::intel_execlists_submission::is_virtual(engine) {
        crate::intel_execlists_submission::virtual_submit_request(i915, gt, engine, rq);
    } else {
        crate::intel_execlists_submission::submit_request(i915, gt, engine as usize, rq);
    }
}

/// `i915_request_add`: queued now, or held until its submit fence signals
/// when it waits for a foreign fence or an earlier request of its timeline
/// is held.
pub fn i915_request_add(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize) {
    if __i915_request_commit(i915, gt, rq).is_err() {
        crate::linux::say("warning: no ring space for the closing breadcrumb");
    }
    // __i915_request_ensure_ordering: the timeline's earlier requests run
    // first, so they run at this one's priority at least.
    let prio = gt.requests[rq].prio;
    i915_request_boost(i915, gt, rq, prio);
    let ce = gt.requests[rq].ce;
    let behind = gt.waiters[..gt.nwaiters].iter().any(|&w| gt.requests[w as usize].ce == ce);
    if !behind && deps_done(gt, rq) {
        gt.requests[rq].ndeps = 0;
        gt.requests[rq].exec_dep = None;
        __i915_request_queue(i915, gt, rq);
        return;
    }
    gt.requests[rq].waiting = true;
    gt.waiters[gt.nwaiters] = rq as u16;
    gt.nwaiters += 1;
}

/// Whether the fence (context `ce`, `seqno`) has signalled: its request
/// is complete or gone. A fence is its request (`rq->fence`), not a place
/// on a timeline: a hardware context goes back to the pool and its timeline
/// may start over, and a seqno compared against that would never pass.
pub fn fence_signaled(gt: &intel_gt, ce: u16, seqno: u32) -> bool {
    ce == u16::MAX
        || !gt.requests.iter().enumerate().any(|(i, r)| {
            r.used && !r.retired && r.ce == ce && r.seqno == seqno && !__i915_request_is_complete(gt, i)
        })
}

fn deps_done(gt: &intel_gt, rq: usize) -> bool {
    let r = &gt.requests[rq];
    gt.wedged || (r.deps[..r.ndeps as usize].iter().all(|&(ce, s)| fence_signaled(gt, ce, s))
                  && r.exec_dep.is_none_or(|(ce, s)| executing(gt, ce, s)))
}

/// Whether request (context `ce`, `seqno`) runs or has run: the execute
/// callback fires when it is submitted to the hardware (`__await_execution`)
/// and the request before it on its timeline has completed
/// (`i915_request_await_start`).
fn executing(gt: &intel_gt, ce: u16, seqno: u32) -> bool {
    match gt.requests.iter().position(|r| r.used && !r.retired && r.ce == ce && r.seqno == seqno) {
        None => true,
        Some(i) => __i915_request_is_complete(gt, i) || (gt.requests[i].active && __i915_request_has_started(gt, i)),
    }
}

/// `i915_request_await_execution` with an i915 request of another context:
/// `rq` goes to the hardware once that one is there and has started.
pub fn i915_request_await_execution(gt: &mut intel_gt, rq: usize, signal: usize) {
    let (ce, seqno) = (gt.requests[signal].ce, gt.requests[signal].seqno);
    if ce == gt.requests[rq].ce || __i915_request_is_complete(gt, signal) {
        return;
    }
    gt.requests[rq].exec_dep = Some((ce, seqno));
}

/// `i915_request_await_dma_fence`: `rq` is submitted only after the fence
/// (context `ce`, `seqno`) signals. A fence of its own timeline is ordered
/// already; one per timeline is kept, the later. False when `rq` cannot
/// take another timeline.
pub fn i915_request_await(gt: &mut intel_gt, rq: usize, ce: u16, seqno: u32) -> bool {
    if ce == gt.requests[rq].ce || fence_signaled(gt, ce, seqno) {
        return true;
    }
    let r = &mut gt.requests[rq];
    let n = r.ndeps as usize;
    if let Some(d) = r.deps[..n].iter_mut().find(|d| d.0 == ce) {
        if i915_seqno_passed(seqno, d.1) {
            d.1 = seqno;
        }
        return true;
    }
    if n >= REQ_DEPS - 1 {
        return false;
    }
    r.deps[n] = (ce, seqno);
    r.ndeps += 1;
    true
}

/// `__i915_schedule`: request `rq` and everything it depends on run at
/// `prio` at least — the requests before it on its timeline and, for a held
/// request, the fences it waits for, transitively. A raised request that is
/// queued moves up its engine's queue.
fn i915_request_boost(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize, prio: i32) {
    let mut stack = [0u16; MAX_REQUESTS];
    let mut n = 1;
    stack[0] = rq as u16;
    while n > 0 {
        n -= 1;
        let w = gt.requests[stack[n] as usize];
        // The timeline's earlier requests not yet submitted.
        for d in 0..MAX_REQUESTS {
            let r = gt.requests[d];
            if !r.used || r.retired || r.ce != w.ce || !i915_seqno_passed(w.seqno, r.seqno) || r.seqno == w.seqno
                || r.prio >= prio || !(r.pqueue || r.waiting) {
                continue;
            }
            raise(i915, gt, d, prio, &mut stack, &mut n);
        }
        // The foreign fences it waits for, and the request it is submitted
        // with (a weak dependency in Linux, raised all the same).
        let mut all = w.deps;
        let mut na = w.ndeps as usize;
        if let Some(d) = w.exec_dep
            && na < all.len() {
            all[na] = d;
            na += 1;
        }
        if na != 0 {
            for &(ce, seqno) in &all[..na] {
                if let Some(d) = gt.requests.iter().position(|r| r.used && !r.retired && r.ce == ce && r.seqno == seqno) {
                    if gt.requests[d].prio < prio {
                        raise(i915, gt, d, prio, &mut stack, &mut n);
                    }
                }
            }
        }
    }
}

/// One request of `i915_request_boost` raised to `prio`, and kept for its
/// own dependencies.
fn raise(i915: &mut drm_i915_private, gt: &mut intel_gt, d: usize, prio: i32, stack: &mut [u16; MAX_REQUESTS],
         n: &mut usize) {
    gt.requests[d].prio = prio;
    let e = gt.requests[d].engine;
    if gt.requests[d].pqueue {
        crate::intel_execlists_submission::requeue_request(i915, gt, e as usize, d);
    } else if crate::intel_execlists_submission::is_virtual(e) {
        crate::intel_execlists_submission::virtual_kick(i915, gt, e, d, prio);
    }
    if *n < stack.len() {
        stack[*n] = d as u16;
        *n += 1;
    }
}

/// `i915_sw_fence` completion for the held requests: in submission order,
/// each whose fences have signalled and that has no held request of its
/// timeline before it goes to the engine. Once wedged every one goes (to
/// the nop submission).
pub fn i915_request_release_waiters(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let mut held_ce = [0u16; MAX_REQUESTS];
    let mut nheld = 0;
    let mut i = 0;
    while i < gt.nwaiters {
        let rq = gt.waiters[i] as usize;
        let ce = gt.requests[rq].ce;
        if held_ce[..nheld].contains(&ce) || !deps_done(gt, rq) {
            if !held_ce[..nheld].contains(&ce) {
                held_ce[nheld] = ce;
                nheld += 1;
            }
            i += 1;
            continue;
        }
        gt.waiters.copy_within(i + 1..gt.nwaiters, i);
        gt.nwaiters -= 1;
        gt.requests[rq].waiting = false;
        gt.requests[rq].ndeps = 0;
        gt.requests[rq].exec_dep = None;
        __i915_request_queue(i915, gt, rq);
    }
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
        let hwsp = hwsp_offset(gt, rq);
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
