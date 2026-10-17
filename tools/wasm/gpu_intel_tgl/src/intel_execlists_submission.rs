//! gt/intel_execlists_submission.c (Linux 6.18.55): submission through the
//! execlist submit queue (ELSQ, GRAPHICS_VER 11+), the context status
//! buffer, and the engine reset hooks, for GRAPHICS_VER 12 below 12.55.
//!
//! The tasklet runs synchronously wherever Linux schedules it; the module
//! is single-threaded; a tasklet kicked while it runs runs again after it,
//! as a scheduled tasklet would. The timeslice and preemption timers are
//! deadlines the module's loop waits for (`execlists_next_timer`) and
//! fires (`execlists_run_timers`). Not kept: parallel contexts, GVT, error
//! capture, and the engine reset: a preemption that times out waits on, as
//! Linux does without `intel_has_reset_engine`, and an engine error (a CS
//! error interrupt or an impossible CSB event) wedges the GT.
//!
//! Virtual engines (`execlists_create_virtual`) live in
//! `gt.virtual_engines`; their ids start at `VIRTUAL_ENGINE_BASE`, where
//! Linux gives them none. A request on one has that id until a sibling
//! takes it in `execlists_dequeue`.

use crate::i915_drv::drm_i915_private;
use crate::i915_request::{__i915_request_is_complete, i915_request_get, i915_request_put};
use crate::intel_gt::intel_gt;
use crate::linux::{self, Line};
use crate::intel_workarounds::{masked_bit_disable, masked_bit_enable};
use crate::regs::*;

const PORTS: usize = 2;
/// Requests an engine's queue and active list hold: every request that can
/// exist, so neither can overflow (Linux's priority lists are unbounded).
pub const SCHED_MAX: usize = crate::i915_request::MAX_REQUESTS;

/// `struct intel_engine_execlists`, as far as the hardware sees it.
#[derive(Clone, Copy, Default)]
pub struct intel_engine_execlists {
    /// ELSP contents not yet acknowledged, and those the CSB promoted;
    /// both terminated by `None`.
    pub pending: [Option<u16>; PORTS + 1],
    pub inflight: [Option<u16>; PORTS + 1],
    /// `execlists->active`: index into `inflight`.
    pub active: usize,
    pub csb_head: u32,
    pub csb_size: u32,
    /// RING_EXECLIST_SQ_CONTENTS and RING_EXECLIST_CONTROL.
    pub submit_reg: u32,
    pub ctrl_reg: u32,
    pub ccid: u32,
    pub error_interrupt: u32,
    pub yield_: u32,
    pub reset_ccid: u32,
    pub port_mask: u32,
    /// `execlists->timer` (the timeslice) and `execlists->preempt`.
    pub timer: el_timer,
    pub preempt: el_timer,
    /// `execlists->preempt_target`: the request a preemption must move.
    pub preempt_target: Option<u16>,
    /// For the report: CSB events read, interrupts seen.
    pub csb_events: u32,
    pub irqs: u32,
}

/// `struct i915_sched_engine`: the priority queue (highest first, FIFO
/// within a priority) and the requests submitted to the engine.
#[derive(Clone, Copy)]
pub struct i915_sched_engine {
    pub queue: [u16; SCHED_MAX],
    pub nqueue: usize,
    pub requests: [u16; SCHED_MAX],
    pub nrequests: usize,
    pub queue_priority_hint: i32,
    /// The tasklet disabled (reset in progress), and replaced by
    /// `nop_submission_tasklet` once wedged.
    pub tasklet_disabled: u32,
    pub tasklet_nop: bool,
    /// `engine->submit_request = nop_submit_request` once wedged.
    pub submit_nop: bool,
    /// The tasklet is running, and was kicked meanwhile.
    pub running: bool,
    pub rerun: bool,
}

impl Default for i915_sched_engine {
    fn default() -> Self {
        Self {
            queue: [0; SCHED_MAX],
            nqueue: 0,
            requests: [0; SCHED_MAX],
            nrequests: 0,
            queue_priority_hint: i32::MIN,
            tasklet_disabled: 0,
            tasklet_nop: false,
            submit_nop: false,
            running: false,
            rerun: false,
        }
    }
}

const ERROR_CSB: u32 = 1 << 31;
const ERROR_PREEMPT: u32 = 1 << 30;

/// A `timer_list` of i915_utils.h: `expires` 0 when inactive, else a
/// deadline in us; `fired` once its callback (a tasklet kick) has run.
#[derive(Clone, Copy, Default)]
pub struct el_timer {
    pub expires: u64,
    pub fired: bool,
}

impl el_timer {
    /// `timer_active`.
    fn active(&self) -> bool {
        self.expires != 0
    }
    /// `timer_pending`: armed and not yet run.
    fn pending(&self) -> bool {
        self.active() && !self.fired
    }
    /// `timer_expired`: armed, and its time has come.
    fn expired(&self) -> bool {
        self.active() && (self.fired || linux::now_us() >= self.expires)
    }
    /// `cancel_timer`.
    fn cancel(&mut self) {
        *self = el_timer::default();
    }
    /// `set_timer_ms`: 0 cancels.
    fn set_ms(&mut self, ms: u64) {
        if ms == 0 {
            self.cancel();
            return;
        }
        self.expires = linux::now_us() + ms * 1000;
        self.fired = false;
    }
}

/// Engine ids from here name `gt.virtual_engines[id - VIRTUAL_ENGINE_BASE]`.
pub const VIRTUAL_ENGINE_BASE: u32 = 0x100;
const MAX_SIBLINGS: usize = crate::i915_gem_context::MAX_SIBLINGS;
const NUM_ENGINES: usize = I915_NUM_ENGINES as usize;
const EINVAL: i32 = 22;
const ENODEV: i32 = 19;

pub fn is_virtual(e: u32) -> bool {
    e >= VIRTUAL_ENGINE_BASE
}

/// `struct virtual_engine`: what of `ve->base` the module needs, the
/// context, the one request let through at a time, and its place in each
/// sibling's tree of virtual engines (`ve->nodes`).
#[derive(Clone, Copy)]
pub struct virtual_engine {
    pub used: bool,
    /// `ve->context`, in `gt.contexts`.
    pub context: u16,
    /// `ve->request`.
    pub request: Option<u16>,
    /// `ve->base.sched_engine->queue_priority_hint`, `ve->base.mask`,
    /// `ve->base.class`.
    pub queue_priority_hint: i32,
    pub mask: u64,
    pub class: u32,
    pub num_siblings: usize,
    pub siblings: [u8; MAX_SIBLINGS],
    /// `ve->nodes[engine->id]`: in that sibling's `execlists->virtual`
    /// tree, at a priority; the sequence number places it after the equal
    /// priorities already there, as the rb-tree insertion does.
    pub node: [Option<(i32, u64)>; NUM_ENGINES],
    /// The tasklet is running, and was kicked meanwhile.
    pub running: bool,
    pub rerun: bool,
}

impl virtual_engine {
    const EMPTY: virtual_engine = virtual_engine { used: false, context: u16::MAX, request: None,
                                                   queue_priority_hint: i32::MIN, mask: 0, class: OTHER_CLASS,
                                                   num_siblings: 0, siblings: [u8::MAX; MAX_SIBLINGS],
                                                   node: [None; NUM_ENGINES], running: false, rerun: false };
}

fn ve_index(e: u32) -> usize {
    (e - VIRTUAL_ENGINE_BASE) as usize
}

/// `ve->base` for building a request: the first sibling's emission and
/// flags, the virtual engine's mask, and no id (`ve->base.id = -1`), so
/// no engine-specific register is named.
pub fn virtual_base(gt: &intel_gt, e: u32) -> Option<crate::intel_gt::intel_engine_cs> {
    let ve = gt.virtual_engines.get(ve_index(e))?;
    let mut b = gt.engine[ve.siblings[0] as usize]?;
    b.id = u32::MAX;
    b.mask = ve.mask;
    b.status_page = None;
    b.kernel_context = None;
    Some(b)
}

/// What `execlists_create_virtual` refuses: an engine listed twice
/// ("duplicate entry in load balancer"), or engines of two classes.
/// `too_many` is a list longer than a slot keeps, which repeats one.
pub fn virtual_siblings_ok(gt: &intel_gt, siblings: &[u8], too_many: bool) -> Result<(), i32> {
    if too_many {
        return Err(-EINVAL);
    }
    let (mut mask, mut class) = (0u64, OTHER_CLASS);
    for &s in siblings {
        let e = gt.engine[s as usize].ok_or(-EINVAL)?;
        if e.mask & mask != 0 {
            return Err(-EINVAL);
        }
        mask |= e.mask;
        if class != OTHER_CLASS && class != e.class {
            return Err(-EINVAL);
        }
        class = e.class;
    }
    Ok(())
}

/// `execlists_create_virtual` + `virtual_engine_initial_hint`: a virtual
/// engine over `siblings`, the first sibling picked at random to spread
/// the load. Returns its id; its context is attached by the caller.
pub fn execlists_create_virtual(gt: &mut intel_gt, siblings: &[u8]) -> Result<u32, i32> {
    virtual_siblings_ok(gt, siblings, false)?;
    if siblings.is_empty() || siblings.len() > MAX_SIBLINGS {
        return Err(-EINVAL);
    }
    let mut ve = virtual_engine { used: true, num_siblings: siblings.len(), ..virtual_engine::EMPTY };
    ve.siblings[..siblings.len()].copy_from_slice(siblings);
    for &s in siblings {
        let e = gt.engine[s as usize].ok_or(-EINVAL)?;
        ve.mask |= e.mask;
        ve.class = e.class;
    }
    let mut r = [0u8; 4];
    if npk_sys::random_bytes(&mut r) < 0 {
        r = [0; 4];
    }
    let swp = u32::from_le_bytes(r) as usize % ve.num_siblings;
    ve.siblings.swap(0, swp);
    let k = match gt.virtual_engines.iter().position(|v| !v.used) {
        Some(k) => {
            gt.virtual_engines[k] = ve;
            k
        }
        None => {
            gt.virtual_engines.push(ve);
            gt.virtual_engines.len() - 1
        }
    };
    Ok(VIRTUAL_ENGINE_BASE + k as u32)
}

/// `virtual_context_destroy`: the engine goes; trailing free slots too.
pub fn virtual_engine_destroy(gt: &mut intel_gt, e: u32) {
    let k = ve_index(e);
    if let Some(v) = gt.virtual_engines.get_mut(k) {
        *v = virtual_engine::EMPTY;
    }
    while gt.virtual_engines.last().is_some_and(|v| !v.used) {
        gt.virtual_engines.pop();
    }
}

/// `ve->context` is the hardware context `ce`.
pub fn virtual_set_context(gt: &mut intel_gt, e: u32, ce: usize) {
    if let Some(v) = gt.virtual_engines.get_mut(ve_index(e)) {
        v.context = ce as u16;
    }
}

/// The virtual engine's first sibling, which the context's image and
/// emission follow (`virtual_context_alloc`/`_pin` use `siblings[0]`).
pub fn virtual_first_sibling(gt: &intel_gt, e: u32) -> Option<usize> {
    gt.virtual_engines.get(ve_index(e)).filter(|v| v.used).map(|v| v.siblings[0] as usize)
}

/// The siblings of a virtual engine (`virtual_context_enter`/`_exit` take
/// each one's wakeref).
pub fn virtual_siblings(gt: &intel_gt, e: u32) -> ([u8; MAX_SIBLINGS], usize) {
    gt.virtual_engines.get(ve_index(e)).map(|v| (v.siblings, v.num_siblings)).unwrap_or(([0; MAX_SIBLINGS], 0))
}

/// Puts sibling `sib`, if present, first, so the context's image is that
/// engine's when its last use was there.
pub fn virtual_prefer(gt: &mut intel_gt, e: u32, sib: u8) {
    if let Some(v) = gt.virtual_engines.get_mut(ve_index(e)) {
        if let Some(n) = v.siblings[..v.num_siblings].iter().position(|&s| s == sib) {
            v.siblings.swap(0, n);
        }
    }
}

/// `virtual_matches`: the request may run on `engine`, and the context is
/// not in flight on another one (its image would still be saved there).
fn virtual_matches(gt: &intel_gt, k: usize, engine: usize) -> bool {
    let ve = &gt.virtual_engines[k];
    if ve.request.is_none() {
        return false;
    }
    let emask = gt.engine[engine].map(|x| x.mask).unwrap_or(0);
    if ve.mask & emask == 0 {
        return false;
    }
    gt.contexts[ve.context as usize].inflight_on.is_none_or(|on| on as usize == engine)
}

/// The first virtual engine in `engine`'s tree: highest priority, then
/// first inserted.
fn virtual_tree_first(gt: &intel_gt, engine: usize) -> Option<usize> {
    let mut best: Option<(usize, i32, u64)> = None;
    for (k, v) in gt.virtual_engines.iter().enumerate() {
        if let Some((prio, seq)) = v.used.then_some(v.node[engine]).flatten() {
            if best.is_none_or(|(_, bp, bs)| prio > bp || (prio == bp && seq < bs)) {
                best = Some((k, prio, seq));
            }
        }
    }
    best.map(|b| b.0)
}

/// `first_virtual_engine`: stale entries (handled by another sibling, or
/// not runnable here) are dropped on the way.
fn first_virtual_engine(gt: &mut intel_gt, engine: usize) -> Option<usize> {
    while let Some(k) = virtual_tree_first(gt, engine) {
        if !virtual_matches(gt, k, engine) {
            gt.virtual_engines[k].node[engine] = None;
            continue;
        }
        return Some(k);
    }
    None
}

/// `virtual_xfer_context`: the sibling that takes the request goes first
/// for the next one. TGL engines address their registers relative to the
/// engine, so the image needs no new offsets (`lrc_update_offsets`).
fn virtual_xfer_context(gt: &mut intel_gt, k: usize, engine: usize) {
    let ve = &mut gt.virtual_engines[k];
    if ve.siblings[0] as usize == engine {
        return;
    }
    if let Some(n) = (1..ve.num_siblings).find(|&n| ve.siblings[n] as usize == engine) {
        ve.siblings.swap(0, n);
    }
}

/// `virtual_submit_request`: the request becomes the virtual engine's one,
/// and the siblings are told. Once wedged it fails at once, as a request on
/// a physical engine does through `nop_submit_request`.
pub fn virtual_submit_request(i915: &mut drm_i915_private, gt: &mut intel_gt, e: u32, rq: usize) {
    let k = ve_index(e);
    if gt.wedged {
        crate::i915_request::i915_request_mark_eio(gt, rq);
    }
    if __i915_request_is_complete(gt, rq) {
        crate::i915_request::__i915_request_submit(i915, gt, rq);
        return;
    }
    if let Some(prev) = gt.virtual_engines[k].request.take() {
        // Background completion from preempt-to-busy.
        crate::i915_request::__i915_request_submit(i915, gt, prev as usize);
        i915_request_put(gt, prev as usize);
    }
    gt.virtual_engines[k].queue_priority_hint = gt.requests[rq].prio;
    gt.virtual_engines[k].request = Some(i915_request_get(gt, rq) as u16);
    virtual_submission_tasklet(i915, gt, k);
}

/// `virtual_submission_tasklet`: the virtual engine goes into each
/// sibling's tree at the request's priority, and a sibling it now leads is
/// kicked. It stops once the context is in flight: only that sibling may
/// take the next request.
fn virtual_submission_tasklet(i915: &mut drm_i915_private, gt: &mut intel_gt, k: usize) {
    if gt.virtual_engines[k].running {
        gt.virtual_engines[k].rerun = true;
        return;
    }
    gt.virtual_engines[k].running = true;
    loop {
        gt.virtual_engines[k].rerun = false;
        __virtual_submission_tasklet(i915, gt, k);
        if !gt.virtual_engines[k].rerun {
            break;
        }
    }
    gt.virtual_engines[k].running = false;
}

fn __virtual_submission_tasklet(i915: &mut drm_i915_private, gt: &mut intel_gt, k: usize) {
    if gt.virtual_engines[k].request.is_none() {
        return;
    }
    let prio = gt.virtual_engines[k].queue_priority_hint;
    // virtual_submission_mask: rq->execution_mask is the virtual engine's.
    let mask = gt.virtual_engines[k].mask;
    for n in 0..gt.virtual_engines[k].num_siblings {
        let ve = gt.virtual_engines[k];
        if ve.request.is_none() {
            break;
        }
        let sibling = ve.siblings[n] as usize;
        let smask = gt.engine[sibling].map(|x| x.mask).unwrap_or(0);
        if mask & smask == 0 {
            gt.virtual_engines[k].node[sibling] = None;
        } else {
            let mut first = false;
            let keep = match ve.node[sibling] {
                Some((p, _)) => {
                    first = virtual_tree_first(gt, sibling) == Some(k);
                    prio == p || (prio > p && first)
                }
                None => false,
            };
            if !keep {
                // Inserted after every node of at least its priority.
                gt.virtual_engines[k].node[sibling] = None;
                first = !gt.virtual_engines.iter().any(|v| v.used && v.node[sibling].is_some_and(|(p, _)| p >= prio));
                gt.virtual_seq += 1;
                let seq = gt.virtual_seq;
                gt.virtual_engines[k].node[sibling] = Some((prio, seq));
            }
            if let Some((_, seq)) = gt.virtual_engines[k].node[sibling] {
                gt.virtual_engines[k].node[sibling] = Some((prio, seq));
            }
            let hint = gt.engine[sibling].map(|x| x.sched.queue_priority_hint).unwrap_or(i32::MIN);
            if first && prio > hint {
                execlists_submission_tasklet(i915, gt, sibling);
            }
        }
        let ce = gt.virtual_engines[k].context as usize;
        if gt.contexts[ce].inflight_on.is_some() {
            break;
        }
    }
}

/// `kick_execlists` on a virtual engine: its request was raised to `prio`,
/// so its place in the siblings' trees moves up.
pub fn virtual_kick(i915: &mut drm_i915_private, gt: &mut intel_gt, e: u32, rq: usize, prio: i32) {
    let k = ve_index(e);
    let Some(ve) = gt.virtual_engines.get(k) else { return };
    if ve.request != Some(rq as u16) || prio <= ve.queue_priority_hint {
        return;
    }
    gt.virtual_engines[k].queue_priority_hint = prio;
    virtual_submission_tasklet(i915, gt, k);
}

/// `kick_siblings`: the virtual context left `engine`. A request of it put
/// back in this engine's queue (by a reset) returns to the virtual engine,
/// and a waiting request is offered to the siblings again.
fn kick_siblings(i915: &mut drm_i915_private, gt: &mut intel_gt, engine: usize, rq: usize, ve_id: u32) {
    let k = ve_index(ve_id);
    if gt.requests[rq].pqueue {
        // resubmit_virtual_request.
        if let Some(x) = gt.engine[engine].as_mut() {
            let s = &mut x.sched;
            if let Some(at) = s.queue[..s.nqueue].iter().position(|&r| r as usize == rq) {
                s.queue.copy_within(at + 1..s.nqueue, at);
                s.nqueue -= 1;
            }
        }
        gt.requests[rq].pqueue = false;
        gt.requests[rq].engine = ve_id;
        virtual_submit_request(i915, gt, ve_id, rq);
    }
    if gt.virtual_engines.get(k).is_some_and(|v| v.request.is_some()) {
        virtual_submission_tasklet(i915, gt, k);
    }
}

fn el(gt: &intel_gt, e: usize) -> intel_engine_execlists {
    gt.engine[e].map(|x| x.execlists).unwrap_or_default()
}

fn set_el(gt: &mut intel_gt, e: usize, v: intel_engine_execlists) {
    if let Some(x) = gt.engine[e].as_mut() {
        x.execlists = v;
    }
}

fn status_page(gt: &intel_gt, e: usize) -> Option<npk_sys::Dma> {
    gt.engine[e].and_then(|x| x.status_page).and_then(|v| v.obj.map())
}

fn mmio_base(gt: &intel_gt, e: usize) -> u32 {
    gt.engine[e].map(|x| x.mmio_base).unwrap_or(0)
}

/// `ring_set_paused`: the closing breadcrumb's semaphore spins while the
/// engine's HWS_PREEMPT dword is set.
fn ring_set_paused(gt: &intel_gt, e: usize, state: u32) {
    if let Some(d) = status_page(gt, e) {
        d.w32(I915_GEM_HWS_PREEMPT * 4, state);
    }
}

/// Whether request `rq` is what engine `e` runs: the context in the active
/// port has it as its last request (`*execlists->active`).
pub fn execlists_running(gt: &intel_gt, e: usize, rq: usize) -> bool {
    let x = el(gt, e);
    x.inflight.get(x.active).copied().flatten() == Some(rq as u16)
}

/// `add_to_engine`. A request completed while still on a virtual engine
/// joins no physical engine's list.
pub fn add_to_engine(gt: &mut intel_gt, e: usize, rq: usize) {
    if is_virtual(e as u32) {
        return;
    }
    if let Some(x) = gt.engine[e].as_mut() {
        let s = &mut x.sched;
        if s.nrequests < SCHED_MAX {
            s.requests[s.nrequests] = rq as u16;
            s.nrequests += 1;
            return;
        }
    }
    linux::say("BUG: engine request list full");
}

/// `remove_from_engine`.
pub fn remove_from_engine(gt: &mut intel_gt, e: usize, rq: usize) {
    if is_virtual(e as u32) {
        return;
    }
    if let Some(x) = gt.engine[e].as_mut() {
        let s = &mut x.sched;
        if let Some(i) = s.requests[..s.nrequests].iter().position(|&r| r as usize == rq) {
            s.requests.copy_within(i + 1..s.nrequests, i);
            s.nrequests -= 1;
        }
    }
}

/// `queue_prio`.
fn queue_prio(gt: &intel_gt, e: usize) -> i32 {
    let s = gt.engine[e].map(|x| x.sched).unwrap_or_default();
    if s.nqueue == 0 { i32::MIN } else { gt.requests[s.queue[0] as usize].prio }
}

/// `queue_request`: into the queue after every request of at least its
/// priority.
fn queue_request(gt: &mut intel_gt, e: usize, rq: usize) {
    queue_request_at(gt, e, rq, false);
}

/// Into the queue at the tail of its priority, or at its head (`head`).
fn queue_request_at(gt: &mut intel_gt, e: usize, rq: usize, head: bool) {
    let prio = gt.requests[rq].prio;
    if gt.engine[e].is_some_and(|x| x.sched.nqueue == SCHED_MAX) {
        // Cannot happen (SCHED_MAX holds every request); never lose one
        // silently: it fails and completes.
        linux::say("BUG: engine queue full, the request fails");
        crate::i915_request::i915_request_mark_eio(gt, rq);
        return;
    }
    let reqs = gt.requests;
    let Some(x) = gt.engine[e].as_mut() else { return };
    let s = &mut x.sched;
    let at = s.queue[..s.nqueue].iter()
        .position(|&r| if head { reqs[r as usize].prio <= prio } else { reqs[r as usize].prio < prio })
        .unwrap_or(s.nqueue);
    s.queue.copy_within(at..s.nqueue, at + 1);
    s.queue[at] = rq as u16;
    s.nqueue += 1;
    gt.requests[rq].pqueue = true;
}

/// `ipi_priority` / `__i915_schedule` for a queued request whose priority
/// was raised: it moves up the queue, and the engine is kicked when it now
/// beats the queue's hint.
pub fn requeue_request(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize) {
    let Some(x) = gt.engine[e].as_mut() else { return };
    let s = &mut x.sched;
    let Some(at) = s.queue[..s.nqueue].iter().position(|&r| r as usize == rq) else { return };
    s.queue.copy_within(at + 1..s.nqueue, at);
    s.nqueue -= 1;
    queue_request(gt, e, rq);
    kick_execlists(i915, gt, e, rq, gt.requests[rq].prio);
}

/// `kick_execlists`: a request raised to `prio` is worth a dequeue when it
/// beats the hint and is not the running context; low priority does not
/// preempt low priority.
fn kick_execlists(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize, prio: i32) {
    let hint = gt.engine[e].map(|x| x.sched.queue_priority_hint).unwrap_or(i32::MIN);
    if prio <= hint {
        return;
    }
    let x = el(gt, e);
    let Some(inflight) = x.inflight[x.active].map(|r| r as usize) else { return };
    if gt.requests[inflight].ce == gt.requests[rq].ce {
        return;
    }
    if let Some(y) = gt.engine[e].as_mut() {
        y.sched.queue_priority_hint = prio;
    }
    if prio >= (I915_PRIORITY_NORMAL as i32).max(gt.requests[inflight].prio) {
        execlists_submission_tasklet(i915, gt, e);
    }
}

/// `engine->submit_request`: `execlists_submit_request`, or once wedged
/// `nop_submit_request`.
pub fn submit_request(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize) {
    if gt.engine[e].map(|x| x.sched.submit_nop).unwrap_or(true) {
        nop_submit_request(i915, gt, e, rq);
        return;
    }
    queue_request(gt, e, rq);
    // submit_queue: kick when the request beats the queue's hint.
    let prio = gt.requests[rq].prio;
    let hint = gt.engine[e].map(|x| x.sched.queue_priority_hint).unwrap_or(i32::MIN);
    if prio > hint {
        if let Some(x) = gt.engine[e].as_mut() {
            x.sched.queue_priority_hint = prio;
        }
        execlists_submission_tasklet(i915, gt, e);
    }
}

/// `nop_submit_request`: fail the request and pass it through.
fn nop_submit_request(i915: &mut drm_i915_private, gt: &mut intel_gt, _e: usize, rq: usize) {
    if crate::i915_request::i915_request_mark_eio(gt, rq) {
        crate::i915_request::__i915_request_submit(i915, gt, rq);
    }
}

/// `execlists_request_alloc`: the 4-level PPGTT needs no PDP reload, so the
/// preamble is the unconditional cache and TLB invalidate.
pub fn execlists_request_alloc(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize) -> Result<(), i32> {
    gt.requests[rq].reserved_space += EXECLISTS_REQUEST_SIZE;
    crate::gen8_engine_cs::emit_flush(i915, gt, rq, EMIT_INVALIDATE)?;
    gt.requests[rq].reserved_space -= EXECLISTS_REQUEST_SIZE;
    Ok(())
}

/// `__execlists_schedule_in`: a context tag for the CCID, the GT awake and
/// the engine's forcewake held while the context is in flight.
fn __execlists_schedule_in(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize) {
    let ce = gt.requests[rq].ce as usize;
    let Some(x) = gt.engine[e].as_mut() else { return };

    // GRAPHICS_VER 11 up to 12.55: (1 + tag) << (GEN11_SW_CTX_ID_SHIFT - 32).
    let tag = x.context_tag.trailing_zeros();
    if tag >= 63 {
        linux::say("warning: no context tag left");
        return;
    }
    x.context_tag &= !(1u64 << tag);
    let mut ccid = (1 + tag) << (GEN11_SW_CTX_ID_SHIFT - 32);
    ccid |= x.execlists.ccid;
    if gt.contexts[ce].tag != 0 {
        ccid = gt.contexts[ce].tag | x.execlists.ccid;
    }
    gt.contexts[ce].ccid = ccid;

    let fw = x.fw_domain;
    let first = x.fw_active == 0;
    x.fw_active += 1;
    crate::intel_gt_pm::intel_gt_pm_get(i915, gt);
    if fw != 0 && first {
        i915.uncore.intel_uncore_forcewake_get(fw);
    }
    intel_engine_context_in(gt, e);
}

/// `execlists_schedule_in`.
fn execlists_schedule_in(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize) {
    let ce = gt.requests[rq].ce as usize;
    if gt.contexts[ce].inflight == 0 {
        __execlists_schedule_in(i915, gt, e, rq);
        gt.contexts[ce].inflight_engine = true;
        gt.contexts[ce].inflight_on = Some(e as u8);
    }
    gt.contexts[ce].inflight += 1;
}

/// `__execlists_schedule_out`.
fn __execlists_schedule_out(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize) {
    let ce = gt.requests[rq].ce as usize;
    let mut ccid = gt.contexts[ce].ccid;
    ccid >>= GEN11_SW_CTX_ID_SHIFT - 32;
    ccid &= GEN12_MAX_CONTEXT_HW_ID;

    if ccid < 64 && ccid != 0 {
        if let Some(x) = gt.engine[e].as_mut() {
            x.context_tag |= 1u64 << (ccid - 1);
        }
    }
    intel_engine_context_out(gt, e);
    let Some(x) = gt.engine[e].as_mut() else { return };
    let fw = x.fw_domain;
    if x.fw_active > 0 {
        x.fw_active -= 1;
    }
    let last = x.fw_active == 0;
    if fw != 0 && last {
        i915.uncore.intel_uncore_forcewake_put(fw);
    }
    crate::intel_gt_pm::intel_gt_pm_put(i915, gt);
    gt.contexts[ce].inflight_engine = false;
    gt.contexts[ce].inflight_on = None;
    // A virtual context's next request may have waited for it to leave
    // this engine (after `ce->inflight` is cleared: the kick runs as a
    // tasklet would, later).
    let owner = gt.contexts[ce].engine;
    if is_virtual(owner) {
        kick_siblings(i915, gt, e, rq, owner);
    }
}

/// `execlists_schedule_out`.
fn execlists_schedule_out(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize) {
    let ce = gt.requests[rq].ce as usize;
    if gt.contexts[ce].inflight > 0 {
        gt.contexts[ce].inflight -= 1;
    }
    if gt.contexts[ce].inflight == 0 {
        __execlists_schedule_out(i915, gt, e, rq);
    }
    i915_request_put(gt, rq);
}

/// `execlists_update_context`: the context's RING_TAIL moves to the end of
/// the request; going backwards forces a full restore.
fn execlists_update_context(gt: &mut intel_gt, e: usize, rq: usize) -> u64 {
    let ce = gt.requests[rq].ce as usize;
    let mut desc = gt.contexts[ce].desc();
    let engine_flags = gt.engine[e].map(|x| x.flags).unwrap_or(0);
    if engine_flags & I915_ENGINE_HAS_EU_PRIORITY != 0 {
        let prio = gt.requests[rq].prio;
        desc |= if prio > I915_PRIORITY_NORMAL as i32 {
            GEN12_CTX_PRIORITY_HIGH
        } else if prio < I915_PRIORITY_NORMAL as i32 {
            GEN12_CTX_PRIORITY_LOW
        } else {
            GEN12_CTX_PRIORITY_NORMAL
        } as u64;
    }

    let prev = gt.contexts[ce].ring.tail;
    let tail = gt.contexts[ce].ring.intel_ring_set_tail(gt.requests[rq].tail);
    if gt.contexts[ce].ring.intel_ring_direction(tail, prev) <= 0 {
        desc |= CTX_DESC_FORCE_RESTORE as u64;
    }
    crate::intel_lrc::lrc_set_reg(gt, ce, CTX_RING_TAIL, tail);
    gt.requests[rq].tail = gt.requests[rq].wa_tail;

    gt.contexts[ce].lrca &= !(CTX_DESC_FORCE_RESTORE as u32);
    desc
}

/// `write_desc`: with ELSQ, low then high dword of each port.
fn write_desc(i915: &mut drm_i915_private, el: &intel_engine_execlists, desc: u64, port: u32) {
    i915.uncore.__raw_uncore_write32(el.submit_reg + port * 8, desc as u32);
    i915.uncore.__raw_uncore_write32(el.submit_reg + port * 8 + 4, (desc >> 32) as u32);
}

/// `execlists_submit_ports`: every port is written (the submit queue is
/// not cleared by the hardware), then the queue is loaded.
fn execlists_submit_ports(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let x = el(gt, e);
    for n in (0..PORTS).rev() {
        let desc = match x.pending[n] {
            Some(rq) => execlists_update_context(gt, e, rq as usize),
            None => 0,
        };
        write_desc(i915, &x, desc, n as u32);
    }
    if x.ctrl_reg != 0 {
        i915.uncore.__raw_uncore_write32(x.ctrl_reg, EL_CTRL_LOAD);
    }
}

/// `can_merge_rq`: same context (no NOPREEMPT/SENTINEL requests here), or
/// the next one already complete.
fn can_merge_rq(gt: &intel_gt, prev: usize, next: usize) -> bool {
    if __i915_request_is_complete(gt, next) {
        return true;
    }
    gt.requests[prev].ce == gt.requests[next].ce
}

/// `execlists_dequeue`.
fn execlists_dequeue(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let mut x = el(gt, e);
    let mut ports: [Option<u16>; PORTS + 1] = [None; PORTS + 1];
    let mut port = 0usize;
    let last_port = x.port_mask as usize;
    let mut submit = false;

    let mut active = x.active;
    while let Some(r) = x.inflight[active] {
        if !__i915_request_is_complete(gt, r as usize) {
            break;
        }
        active += 1;
    }
    let mut last = x.inflight[active].map(|r| r as usize);

    if let Some(l) = last {
        if need_preempt(gt, e, l) {
            // RING_HEAD stays before the breadcrumb while the requests are
            // unwound and until they are submitted again; some of them may
            // still complete meanwhile.
            ring_set_paused(gt, e, 1);
            __unwind_incomplete_requests(gt, e);
            last = None;
        } else if timeslice_expired(gt, e, l) {
            // This timeslice is used up; submitting the same pair again
            // gets a full new one.
            x.timer.cancel();
            set_el(gt, e, x);
            ring_set_paused(gt, e, 1);
            defer_active(gt, e);
            last = None;
        } else if x.inflight[active + 1].is_some() {
            // Wait for the next CS event before queueing more.
            return;
        }
    }

    // A virtual engine's request goes first ("XXX virtual is always taking
    // precedence"); one per dequeue.
    while let Some(k) = first_virtual_engine(gt, e) {
        let Some(rq) = gt.virtual_engines[k].request.map(|r| r as usize) else { break };
        if gt.requests[rq].prio < queue_prio(gt, e) {
            break;
        }
        if let Some(l) = last {
            if !can_merge_rq(gt, l, rq) {
                // Left for another sibling.
                return;
            }
        }
        gt.virtual_engines[k].request = None;
        gt.virtual_engines[k].queue_priority_hint = i32::MIN;
        gt.virtual_engines[k].node[e] = None;
        gt.requests[rq].engine = e as u32;
        if crate::i915_request::__i915_request_submit(i915, gt, rq) {
            virtual_xfer_context(gt, k, e);
            submit = true;
            last = Some(rq);
        }
        i915_request_put(gt, rq);
        if submit {
            break;
        }
    }

    loop {
        let s = gt.engine[e].map(|y| y.sched).unwrap_or_default();
        if s.nqueue == 0 {
            break;
        }
        let rq = s.queue[0] as usize;
        let mut merge = true;
        if let Some(l) = last {
            if !can_merge_rq(gt, l, rq) {
                if port == last_port {
                    break;
                }
                if gt.requests[l].ce == gt.requests[rq].ce {
                    break;
                }
                // A virtual request stays out of the second port, free to
                // move to another engine instead of waiting here.
                if is_virtual(gt.contexts[gt.requests[rq].ce as usize].engine) {
                    break;
                }
                merge = false;
            }
        }

        // Off the queue before submitting.
        if let Some(y) = gt.engine[e].as_mut() {
            y.sched.queue.copy_within(1..y.sched.nqueue, 0);
            y.sched.nqueue -= 1;
        }

        if crate::i915_request::__i915_request_submit(i915, gt, rq) {
            if !merge {
                if let Some(l) = last.take() {
                    ports[port] = Some(i915_request_get(gt, l) as u16);
                    port += 1;
                }
            }
            submit = true;
            last = Some(rq);
        }
    }

    if let Some(l) = last {
        ports[port] = Some(i915_request_get(gt, l) as u16);
        port += 1;
    }

    let qp = queue_prio(gt, e);
    if let Some(y) = gt.engine[e].as_mut() {
        y.sched.queue_priority_hint = qp;
    }

    // Skip the hardware when the set is what is already running.
    let same = (0..port).all(|i| x.inflight.get(active + i).copied().flatten() == ports[i]);
    if submit && !same {
        x.pending = ports;
        set_el(gt, e, x);
        for i in (0..port).rev() {
            if let Some(r) = ports[i] {
                execlists_schedule_in(i915, gt, e, r as usize);
            }
        }
        let mut x = el(gt, e);
        x.yield_ = u32::MAX;
        set_el(gt, e, x);
        set_preempt_timeout(gt, e, x.inflight[x.active].map(|r| r as usize));
        execlists_submit_ports(i915, gt, e);
    } else {
        ring_set_paused(gt, e, 0);
        for p in ports.iter().take(port).flatten() {
            i915_request_put(gt, *p as usize);
        }
        let mut x = el(gt, e);
        x.pending[0] = None;
        set_el(gt, e, x);
    }
}

/// `csb_read` with `wa_csb_read` (tgl HSDES#22011327657): a stale entry
/// (all ones) is waited on for 10 us, then read from the MMIO copy; the
/// entry is consumed by writing all ones back.
fn csb_read(i915: &mut drm_i915_private, gt: &intel_gt, e: usize, idx: u32) -> u64 {
    let Some(d) = status_page(gt, e) else { return 0 };
    let off = (I915_HWS_CSB_BUF0_INDEX + idx * 2) * 4;
    let read = || (d.r32(off + 4) as u64) << 32 | d.r32(off) as u64;
    let mut entry = read();
    if entry == u64::MAX {
        if linux::wait_for_atomic_us(10, || read() != u64::MAX).is_ok() {
            entry = read();
        } else {
            let (status, i) = if idx >= 6 {
                (GEN11_EXECLISTS_STATUS_BUF2, idx - 6)
            } else {
                (GEN8_EXECLISTS_STATUS_BUF, idx)
            };
            entry = i915.uncore.intel_uncore_read64(mmio_base(gt, e) + status + 8 * i);
        }
    }
    d.w32(off, u32::MAX);
    d.w32(off + 4, u32::MAX);
    entry
}

/// `gen12_csb_parse`: a promotion when the switched-away half is idle or
/// the switch was to a new queue.
fn gen12_csb_parse(csb: u64) -> bool {
    let valid = |dw: u32| reg_field_get(GEN12_CSB_SW_CTX_ID_MASK as u32, dw) != GEN12_IDLE_CTX_ID;
    let ctx_away_valid = valid((csb >> 32) as u32);
    let new_queue = csb as u32 & GEN12_CTX_STATUS_SWITCHED_TO_NEW_QUEUE != 0;
    !ctx_away_valid || new_queue
}

fn reg_field_get(mask: u32, v: u32) -> u32 {
    (v & mask) >> mask.trailing_zeros()
}

/// `process_csb`: follow the hardware's context-switch events; promoted
/// ports become the active ones, completed ones are handed back in
/// `inactive` for `post_process_csb`.
fn process_csb(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, inactive: &mut [u16; 2 * PORTS],
               ninactive: &mut usize) {
    let mut x = el(gt, e);
    let Some(d) = status_page(gt, e) else { return };
    let num_entries = x.csb_size;
    let mut head = x.csb_head;
    let tail = d.r32(ICL_HWS_CSB_WRITE_INDEX * 4) & 0xff;
    if head == tail {
        return;
    }
    x.csb_head = tail;
    let prev = x.inflight[x.active];

    loop {
        head += 1;
        if head == num_entries {
            head = 0;
        }
        let csb = csb_read(i915, gt, e, head);
        x.csb_events += 1;

        if gen12_csb_parse(csb) {
            if x.pending[0].is_none() {
                x.error_interrupt |= ERROR_CSB;
                break;
            }
            ring_set_paused(gt, e, 0);
            let mut a = x.active;
            while let Some(r) = x.inflight[a] {
                if *ninactive < inactive.len() {
                    inactive[*ninactive] = r;
                    *ninactive += 1;
                }
                a += 1;
            }
            x.inflight = x.pending;
            x.active = 0;
            // The magic delay for tgl.
            let _ = i915.uncore.intel_uncore_read(mmio_base(gt, e) + RING_CONTEXT_STATUS_PTR(0));
            x.pending[0] = None;
        } else {
            let Some(r) = x.inflight[x.active] else {
                x.error_interrupt |= ERROR_CSB;
                break;
            };
            if *ninactive < inactive.len() {
                inactive[*ninactive] = r;
                *ninactive += 1;
            }
            x.active += 1;
        }
        if head == tail {
            break;
        }
    }
    // new_timeslice, unless it was a lite-restore of the same request.
    if prev != x.inflight[x.active] {
        x.timer.cancel();
    }
    set_el(gt, e, x);
}

/// `post_process_csb`.
fn post_process_csb(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, inactive: &[u16]) {
    for &r in inactive {
        execlists_schedule_out(i915, gt, e, r as usize);
    }
}

/// `execlists_submission_tasklet` (or `nop_submission_tasklet` once wedged).
/// Kicked while it runs, it runs once more after.
pub fn execlists_submission_tasklet(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let Some(x) = gt.engine[e].as_mut() else { return };
    if x.sched.running {
        x.sched.rerun = true;
        return;
    }
    x.sched.running = true;
    loop {
        if let Some(x) = gt.engine[e].as_mut() {
            x.sched.rerun = false;
        }
        __execlists_submission_tasklet(i915, gt, e);
        if !gt.engine[e].is_some_and(|x| x.sched.rerun) {
            break;
        }
    }
    if let Some(x) = gt.engine[e].as_mut() {
        x.sched.running = false;
    }
}

fn __execlists_submission_tasklet(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let Some(s) = gt.engine[e].map(|x| x.sched) else { return };
    if s.tasklet_disabled > 0 {
        return;
    }
    if s.tasklet_nop {
        if let Some(x) = gt.engine[e].as_mut() {
            x.sched.queue_priority_hint = i32::MIN;
        }
        return;
    }

    let mut inactive = [0u16; 2 * PORTS];
    let mut n = 0;
    process_csb(i915, gt, e, &mut inactive, &mut n);

    // preempt_timeout: still on the request the preemption was meant to
    // move, or a switch happened and the new context gets its own time.
    let mut x = el(gt, e);
    if x.preempt.expired() && x.pending[0].is_some() {
        x.preempt.cancel();
        let rq = x.inflight[x.active];
        if rq.is_some() && rq == x.preempt_target {
            x.error_interrupt |= ERROR_PREEMPT;
        } else {
            set_el(gt, e, x);
            set_preempt_timeout(gt, e, rq.map(|r| r as usize));
            x = el(gt, e);
        }
    }
    set_el(gt, e, x);

    let err = el(gt, e).error_interrupt;
    if err == ERROR_PREEMPT {
        // execlists_reset without intel_has_reset_engine: nothing; the
        // context leaves at its next arbitration point.
        let mut x = el(gt, e);
        x.error_interrupt = 0;
        set_el(gt, e, x);
        use core::sync::atomic::{AtomicU32, Ordering};
        static LOGGED: AtomicU32 = AtomicU32::new(0);
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 4 {
            Line::new().s("engine ").s(engine_name(gt, e))
                .s(": preemption time out; no engine reset is ported, waiting on").say();
        }
    } else if err != 0 {
        let msg = if err & 0xffff != 0 {
            "CS error"
        } else if err & ERROR_CSB != 0 {
            "invalid CSB event"
        } else {
            "internal error"
        };
        let mut x = el(gt, e);
        x.error_interrupt = 0;
        set_el(gt, e, x);
        Line::new().s("engine ").s(engine_name(gt, e)).s(": ").s(msg).s(" (").hex(err)
            .s("), the engine reset is not ported; wedging").say();
        post_process_csb(i915, gt, e, &inactive[..n]);
        crate::intel_gt::intel_gt_set_wedged(i915, gt);
        return;
    }

    if el(gt, e).pending[0].is_none() {
        execlists_dequeue(i915, gt, e);
        start_timeslice(i915, gt, e);
    }

    post_process_csb(i915, gt, e, &inactive[..n]);
}

/// `need_preempt`: the queue (or the second port, or a virtual engine)
/// holds something of higher priority than the running request; not at
/// equal priority, which keeps FIFO order.
fn need_preempt(gt: &intel_gt, e: usize, rq: usize) -> bool {
    let Some(engine) = gt.engine[e] else { return false };
    if engine.flags & I915_ENGINE_HAS_SEMAPHORES == 0 {
        return false;
    }
    let last_prio = gt.requests[rq].prio.max(I915_PRIORITY_NORMAL as i32 - 1);
    if engine.sched.queue_priority_hint <= last_prio {
        return false;
    }
    let s = engine.sched;
    if let Some(i) = s.requests[..s.nrequests].iter().position(|&r| r as usize == rq) {
        if i + 1 < s.nrequests && gt.requests[s.requests[i + 1] as usize].prio > last_prio {
            return true;
        }
    }
    virtual_prio(gt, e).max(queue_prio(gt, e)) > last_prio
}

/// `virtual_prio`: the first virtual engine of the engine's tree.
fn virtual_prio(gt: &intel_gt, e: usize) -> i32 {
    virtual_tree_first(gt, e).and_then(|k| gt.virtual_engines[k].node[e]).map(|(p, _)| p).unwrap_or(i32::MIN)
}

/// `timeslice_yield`: the running context once busy-waited on a semaphore.
fn timeslice_yield(gt: &intel_gt, e: usize, rq: usize) -> bool {
    gt.contexts[gt.requests[rq].ce as usize].ccid == el(gt, e).yield_
}

/// `needs_timeslice`: another context is waiting for this engine.
fn needs_timeslice(gt: &intel_gt, e: usize, rq: Option<usize>) -> bool {
    let Some(engine) = gt.engine[e] else { return false };
    if engine.flags & I915_ENGINE_HAS_TIMESLICES == 0 {
        return false;
    }
    let Some(rq) = rq.filter(|&r| !__i915_request_is_complete(gt, r)) else { return false };
    if engine.execlists.pending[0].is_some() {
        return false;
    }
    let s = engine.sched;
    if s.requests[..s.nrequests].last().is_some_and(|&r| r as usize != rq)
        && s.requests[..s.nrequests].iter().any(|&r| r as usize == rq) {
        return true;
    }
    if s.nqueue != 0 {
        return true;
    }
    virtual_tree_first(gt, e).is_some()
}

/// `timeslice_expired`.
fn timeslice_expired(gt: &intel_gt, e: usize, rq: usize) -> bool {
    if !needs_timeslice(gt, e, Some(rq)) {
        return false;
    }
    el(gt, e).timer.expired() || timeslice_yield(gt, e, rq)
}

/// `start_timeslice`: armed while another context waits, and not prolonged
/// while running; a slice already used up kicks the tasklet again.
fn start_timeslice(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let mut x = el(gt, e);
    let mut duration = 0;
    if needs_timeslice(gt, e, x.inflight[x.active].map(|r| r as usize)) {
        if x.timer.active() {
            if !x.timer.pending() {
                execlists_submission_tasklet(i915, gt, e);
            }
            return;
        }
        duration = gt.engine[e].map(|y| y.timeslice_duration_ms).unwrap_or(0);
    }
    x.timer.set_ms(duration);
    set_el(gt, e, x);
}

/// `set_preempt_timeout` with `active_preempt_timeout`: the preemption of
/// the running request must happen within the engine's timeout.
fn set_preempt_timeout(gt: &mut intel_gt, e: usize, rq: Option<usize>) {
    let mut x = el(gt, e);
    let ms = match rq {
        None => 0,
        Some(r) => {
            x.preempt_target = Some(r as u16);
            gt.engine[e].map(|y| y.preempt_timeout_ms).unwrap_or(0)
        }
    };
    x.preempt.set_ms(ms);
    set_el(gt, e, x);
}

/// The earliest deadline of an engine's timeslice or preemption timer that
/// has not fired, in us.
pub fn execlists_next_timer(gt: &intel_gt) -> Option<u64> {
    gt.engine.iter().flatten()
        .flat_map(|y| [y.execlists.timer, y.execlists.preempt])
        .filter(|t| t.pending())
        .map(|t| t.expires)
        .min()
}

/// `execlists_timeslice` / `execlists_preempt`: an expired timer kicks its
/// engine's tasklet once.
pub fn execlists_run_timers(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let now = linux::now_us();
    for e in 0..gt.engine.len() {
        let Some(y) = gt.engine[e].as_mut() else { continue };
        let mut kick = false;
        for t in [&mut y.execlists.timer, &mut y.execlists.preempt] {
            if t.pending() && now >= t.expires {
                t.fired = true;
                kick = true;
            }
        }
        if kick {
            execlists_submission_tasklet(i915, gt, e);
        }
    }
}

/// `execlists_irq_handler`.
pub fn execlists_irq_handler(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, iir: u16) {
    let mut tasklet = false;
    let base = mmio_base(gt, e);
    let mut x = el(gt, e);
    x.irqs += 1;

    if iir as u32 & GT_CS_MASTER_ERROR_INTERRUPT != 0 {
        // The upper 16 bits are the enabling mask.
        let eir = i915.uncore.intel_uncore_read(RING_EIR(base)) & 0xffff;
        if eir != 0 {
            i915.uncore.intel_uncore_write(RING_EMR(base), !0);
            i915.uncore.intel_uncore_write(RING_EIR(base), eir);
            x.error_interrupt = eir;
            tasklet = true;
        }
    }
    if iir as u32 & GT_WAIT_SEMAPHORE_INTERRUPT != 0 {
        x.yield_ = i915.uncore.intel_uncore_read_fw(RING_EXECLIST_STATUS_HI(base));
        // timer_delete: a pending slice ends now.
        if x.timer.pending() {
            x.timer.fired = true;
            tasklet = true;
        }
    }
    if iir as u32 & GT_CONTEXT_SWITCH_INTERRUPT != 0 {
        tasklet = true;
    }
    // GT_RENDER_USER_INTERRUPT signals the breadcrumbs, which are read
    // from the HWSP when retiring.
    set_el(gt, e, x);

    if tasklet {
        execlists_submission_tasklet(i915, gt, e);
    }
}

/// The engine's name for the log.
pub fn engine_name(gt: &intel_gt, e: usize) -> &'static str {
    let x = gt.engine[e].unwrap_or_default();
    match (x.class, x.instance) {
        (RENDER_CLASS, 0) => "rcs0",
        (COPY_ENGINE_CLASS, 0) => "bcs0",
        (VIDEO_DECODE_CLASS, 0) => "vcs0",
        (VIDEO_DECODE_CLASS, 2) => "vcs2",
        (VIDEO_ENHANCEMENT_CLASS, 0) => "vecs0",
        _ => crate::intel_gt::intel_engine_class_repr(x.class),
    }
}

/// `reset_csb_pointers`: after a reset the hardware writes entry 0 next, so
/// our head and the HWSP's write pointer start at the last entry, and every
/// entry is marked stale.
pub fn reset_csb_pointers(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let mut x = el(gt, e);
    let base = mmio_base(gt, e);
    let reset_value = x.csb_size - 1;

    ring_set_paused(gt, e, 0);

    // Icelake sometimes forgets to reset its pointers on a GPU reset.
    i915.uncore.intel_uncore_write(RING_CONTEXT_STATUS_PTR(base), 0xffff << 16 | reset_value << 8 | reset_value);
    let _ = i915.uncore.intel_uncore_read(RING_CONTEXT_STATUS_PTR(base));

    x.csb_head = reset_value;
    if let Some(d) = status_page(gt, e) {
        d.w32(ICL_HWS_CSB_WRITE_INDEX * 4, reset_value);
        for i in 0..=reset_value {
            d.w32((I915_HWS_CSB_BUF0_INDEX + i * 2) * 4, u32::MAX);
            d.w32((I915_HWS_CSB_BUF0_INDEX + i * 2) * 4 + 4, u32::MAX);
        }
    }

    i915.uncore.intel_uncore_write(RING_CONTEXT_STATUS_PTR(base), 0xffff << 16 | reset_value << 8 | reset_value);
    let _ = i915.uncore.intel_uncore_read(RING_CONTEXT_STATUS_PTR(base));
    set_el(gt, e, x);
}

/// `execlists_sanitize`: CSB pointers, the kernel timeline's HWSP slot,
/// and the other perma-pinned contexts (none besides the kernel's).
pub fn execlists_sanitize(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    reset_csb_pointers(i915, gt, e);
    // sanitize_hwsp: the timelines in the status page.
    for c in 0..gt.contexts.len() {
        let ce = gt.contexts[c];
        if ce.used && ce.engine as usize == e && ce.alloc && ce.timeline.engine_hwsp {
            ce.timeline.intel_timeline_reset_seqno();
        }
    }
}

/// `enable_error_interrupt`.
fn enable_error_interrupt(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let base = mmio_base(gt, e);
    let mut x = el(gt, e);
    x.error_interrupt = 0;
    set_el(gt, e, x);
    i915.uncore.intel_uncore_write(RING_EMR(base), !0);
    i915.uncore.intel_uncore_write(RING_EIR(base), !0);

    let status = i915.uncore.intel_uncore_read(RING_ESR(base));
    if status != 0 {
        Line::new().s("engine '").s(engine_name(gt, e)).s("' resumed still in error: ").hex(status).say();
        linux::say("intel_gt_reset_engine is not ported");
    }

    // Only the instruction error is fatal; CP_PRIV also fires for ignored
    // writes to read-only scratch.
    i915.uncore.intel_uncore_write(RING_EMR(base), !I915_ERROR_INSTRUCTION);
}

/// `enable_execlists`: HWSTAM, legacy mode off, STOP_RING cleared, the HWSP
/// address, error interrupts.
fn enable_execlists(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let base = mmio_base(gt, e);
    i915.uncore.intel_uncore_write(RING_HWSTAM(base), !0);
    i915.uncore.intel_uncore_write_fw(RING_MODE_GEN7(base), masked_bit_enable(GEN11_GFX_DISABLE_LEGACY_MODE));
    i915.uncore.intel_uncore_write_fw(RING_MI_MODE(base), masked_bit_disable(STOP_RING));
    let hws = gt.engine[e].and_then(|x| x.status_page).map(|v| v.ggtt_offset()).unwrap_or(0);
    i915.uncore.intel_uncore_write_fw(RING_HWS_PGA(base), hws);
    let _ = i915.uncore.intel_uncore_read(RING_HWS_PGA(base));
    enable_error_interrupt(i915, gt, e);
}

/// `execlists_resume`: MOCS for the engine (render also the L3CC table),
/// `intel_breadcrumbs_reset` (no interrupt mask to restore on GRAPHICS_VER
/// 11+), then execlists on. Not the first render/compute engine's CCS
/// enabling (Xe_HP).
pub fn execlists_resume(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) -> Result<(), i32> {
    let class = gt.engine[e].map(|x| x.class).unwrap_or(0);
    crate::intel_mocs::intel_mocs_init_engine(i915, class);
    enable_execlists(i915, gt, e);
    Ok(())
}

/// `intel_engine_stop_cs` with Wa_22011802037 (GRAPHICS_VER 11 to 12.70).
pub fn intel_engine_stop_cs(i915: &mut drm_i915_private, gt: &intel_gt, e: usize) -> Result<(), i32> {
    let base = mmio_base(gt, e);
    let stop_ms = gt.engine[e].map(|x| x.stop_timeout_ms).unwrap_or(100);
    i915.uncore.intel_uncore_write_fw(RING_MI_MODE(base), masked_bit_enable(STOP_RING));
    i915.uncore.intel_uncore_write_fw(RING_MODE_GEN7(base), masked_bit_enable(GEN12_GFX_PREFETCH_DISABLE));
    let r = i915.uncore.__intel_wait_for_register_fw(RING_MI_MODE(base), MODE_IDLE, MODE_IDLE, 1000, stop_ms as u32, None);
    let _ = i915.uncore.intel_uncore_read_fw(RING_MI_MODE(base));
    if r.is_err() {
        let head = i915.uncore.intel_uncore_read_fw(RING_HEAD(base)) & HEAD_ADDR;
        let tail = i915.uncore.intel_uncore_read_fw(RING_TAIL(base)) & TAIL_ADDR;
        if head != tail {
            return Err(-110);
        }
    }
    Ok(())
}

/// `intel_engine_wait_for_pending_mi_fw`: MI-initiated forcewakes the CS
/// still has outstanding are waited out (Wa_22011802037).
fn intel_engine_wait_for_pending_mi_fw(i915: &mut drm_i915_private, gt: &intel_gt, e: usize) {
    let reg = match gt.engine[e].map(|x| x.id) {
        Some(RCS0) => MSG_IDLE_CS,
        Some(BCS0) => MSG_IDLE_BCS,
        Some(VCS0) => MSG_IDLE_VCS0,
        Some(VCS1) => MSG_IDLE_VCS1,
        Some(VCS2) => MSG_IDLE_VCS2,
        Some(VCS3) => MSG_IDLE_VCS3,
        Some(VECS0) => MSG_IDLE_VECS0,
        Some(VECS1) => MSG_IDLE_VECS1,
        _ => return,
    };
    let val = i915.uncore.intel_uncore_read(reg);
    let fw_pending = (val & (val >> 16) & MSG_IDLE_FW_MASK) >> MSG_IDLE_FW_SHIFT;
    if fw_pending == 0 {
        return;
    }
    linux::udelay(1);
    let _ = i915.uncore.__intel_wait_for_register_fw(GEN9_PWRGT_DOMAIN_STATUS, fw_pending, fw_pending, 5000, 0, None);
    linux::udelay(1);
}

/// `execlists_reset_prepare`: no submission until the reset is done, the
/// ring paused at its breadcrumb, the CS stopped.
pub fn execlists_reset_prepare(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    if let Some(x) = gt.engine[e].as_mut() {
        x.sched.tasklet_disabled += 1;
    }
    ring_set_paused(gt, e, 1);
    let _ = intel_engine_stop_cs(i915, gt, e);
    intel_engine_wait_for_pending_mi_fw(i915, gt, e);
    let base = mmio_base(gt, e);
    let ccid = i915.uncore.intel_uncore_read_fw(RING_EXECLIST_STATUS_HI(base));
    let mut x = el(gt, e);
    x.reset_ccid = ccid;
    set_el(gt, e, x);
}

/// `active_context`: the port whose context carries `ccid`.
fn active_context(gt: &intel_gt, e: usize, ccid: u32) -> Option<usize> {
    let x = el(gt, e);
    let mut a = x.active;
    while let Some(r) = x.inflight[a] {
        if gt.contexts[gt.requests[r as usize].ce as usize].ccid == ccid {
            return Some(r as usize);
        }
        a += 1;
    }
    x.pending.iter().flatten().map(|&r| r as usize)
        .find(|&r| gt.contexts[gt.requests[r].ce as usize].ccid == ccid)
}

/// `execlists_reset_active`: the context that was running restarts at its
/// incomplete request, or past its last one.
fn execlists_reset_active(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let ccid = el(gt, e).reset_ccid;
    let Some(rq) = active_context(gt, e, ccid) else { return };
    let ce = gt.requests[rq].ce as usize;

    let head = if __i915_request_is_complete(gt, rq) {
        gt.contexts[ce].ring.intel_ring_wrap(gt.requests[rq].tail)
    } else {
        // __active_request: the oldest incomplete request of the timeline.
        let tl = gt.contexts[ce].timeline;
        let mut target = rq;
        for &r in &tl.requests[..tl.nrequests] {
            if !__i915_request_is_complete(gt, r as usize) {
                target = r as usize;
                break;
            }
        }
        if crate::i915_request::__i915_request_has_started(gt, target) {
            // __i915_request_reset: the guilty request is skipped.
            linux::say("warning: an engine was reset mid-request; the request is marked failed");
            gt.requests[target].error = -5;
        }
        gt.contexts[ce].ring.intel_ring_wrap(gt.requests[target].head)
    };

    crate::intel_lrc::lrc_reset_regs(gt, ce);
    let lrca = crate::intel_lrc::lrc_update_regs(i915, gt, ce, head);
    gt.contexts[ce].lrca = lrca;
}

/// `cancel_port_requests`.
fn cancel_port_requests(gt: &mut intel_gt, e: usize, inactive: &mut [u16; 2 * PORTS], n: &mut usize) {
    let mut x = el(gt, e);
    for r in x.pending.iter().flatten() {
        if *n < inactive.len() {
            inactive[*n] = *r;
            *n += 1;
        }
    }
    x.pending = [None; PORTS + 1];
    let mut a = x.active;
    while let Some(r) = x.inflight[a] {
        if *n < inactive.len() {
            inactive[*n] = r;
            *n += 1;
        }
        a += 1;
    }
    x.inflight = [None; PORTS + 1];
    x.active = 0;
    set_el(gt, e, x);
}

/// `execlists_reset_csb`.
fn execlists_reset_csb(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let mut inactive = [0u16; 2 * PORTS];
    let mut n = 0;
    // reset_csb: drain the events, then reload the pointers.
    process_csb(i915, gt, e, &mut inactive, &mut n);
    reset_csb_pointers(i915, gt, e);
    execlists_reset_active(i915, gt, e);
    cancel_port_requests(gt, e, &mut inactive, &mut n);
    post_process_csb(i915, gt, e, &inactive[..n]);
}

/// `__unwind_incomplete_requests`: what was submitted and has not
/// completed goes back to the queue, newest first, each to the head of its
/// priority (`list_move`), so their order is kept ahead of what waited
/// there. Returns the oldest one unwound.
fn __unwind_incomplete_requests(gt: &mut intel_gt, e: usize) -> Option<usize> {
    let s = gt.engine[e].map(|x| x.sched).unwrap_or_default();
    let mut active = None;
    for i in (0..s.nrequests).rev() {
        let rq = s.requests[i] as usize;
        remove_from_engine(gt, e, rq);
        if __i915_request_is_complete(gt, rq) {
            continue;
        }
        crate::i915_request::__i915_request_unsubmit(gt, rq);
        queue_request_at(gt, e, rq, true);
        let ce = gt.requests[rq].ce as usize;
        let ring = gt.contexts[ce].ring;
        if ring.intel_ring_direction(gt.requests[rq].tail, ring.tail.wrapping_add(8)) > 0 {
            gt.contexts[ce].lrca |= CTX_DESC_FORCE_RESTORE as u32;
        }
        active = Some(rq);
    }
    active
}

/// `defer_request`: the interrupted request goes to the back of its
/// priority, and with it the requests on this engine that wait for it and
/// run at its priority or above — here the later requests of its
/// timeline: a foreign fence of a queued request has signalled already.
fn defer_request(gt: &mut intel_gt, e: usize, rq: usize) {
    let mut list = [0u16; SCHED_MAX];
    let (mut head, mut tail) = (0usize, 0usize);
    let mut cur = Some(rq);
    while let Some(r) = cur {
        move_to_tail(gt, e, r);
        let (ce, seqno, prio) = (gt.requests[r].ce, gt.requests[r].seqno, gt.requests[r].prio);
        let s = gt.engine[e].map(|x| x.sched).unwrap_or_default();
        for &w in &s.queue[..s.nqueue] {
            let wr = &gt.requests[w as usize];
            if wr.ce != ce || wr.seqno == seqno || !crate::i915_request::i915_seqno_passed(wr.seqno, seqno)
                || wr.prio < prio || list[..tail].contains(&w) {
                continue;
            }
            if tail < list.len() {
                list[tail] = w;
                tail += 1;
            }
        }
        cur = (head < tail).then(|| {
            head += 1;
            list[head - 1] as usize
        });
    }
}

/// `list_move_tail` onto its priority: after every queued request of at
/// least its priority.
fn move_to_tail(gt: &mut intel_gt, e: usize, rq: usize) {
    if let Some(x) = gt.engine[e].as_mut() {
        let s = &mut x.sched;
        if let Some(at) = s.queue[..s.nqueue].iter().position(|&r| r as usize == rq) {
            s.queue.copy_within(at + 1..s.nqueue, at);
            s.nqueue -= 1;
        }
    }
    queue_request_at(gt, e, rq, false);
}

/// `defer_active`.
fn defer_active(gt: &mut intel_gt, e: usize) {
    if let Some(rq) = __unwind_incomplete_requests(gt, e) {
        defer_request(gt, e, rq);
    }
}

/// `execlists_reset_rewind`.
pub fn execlists_reset_rewind(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    execlists_reset_csb(i915, gt, e);
    __unwind_incomplete_requests(gt, e);
}

/// `execlists_reset_cancel`: everything on or for the engine fails with
/// -EIO; the tasklet becomes `nop_submission_tasklet`.
pub fn execlists_reset_cancel(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    execlists_reset_csb(i915, gt, e);

    let s = gt.engine[e].map(|x| x.sched).unwrap_or_default();
    for &r in &s.requests[..s.nrequests] {
        crate::i915_request::i915_request_mark_eio(gt, r as usize);
    }
    for &r in &s.queue[..s.nqueue] {
        if crate::i915_request::i915_request_mark_eio(gt, r as usize) {
            crate::i915_request::__i915_request_submit(i915, gt, r as usize);
        }
    }
    if let Some(x) = gt.engine[e].as_mut() {
        x.sched.nqueue = 0;
        x.sched.queue_priority_hint = i32::MIN;
        x.sched.tasklet_nop = true;
    }

    // Cancel all attached virtual engines.
    while let Some(k) = virtual_tree_first(gt, e) {
        gt.virtual_engines[k].node[e] = None;
        if let Some(r) = gt.virtual_engines[k].request.take() {
            let r = r as usize;
            gt.requests[r].engine = e as u32;
            crate::i915_request::i915_request_mark_eio(gt, r);
            crate::i915_request::__i915_request_submit(i915, gt, r);
            i915_request_put(gt, r);
            gt.virtual_engines[k].queue_priority_hint = i32::MIN;
        }
    }
}

/// `execlists_reset_finish`: the tasklet back on, and kicked.
pub fn execlists_reset_finish(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let enabled = match gt.engine[e].as_mut() {
        Some(x) if x.sched.tasklet_disabled > 0 => {
            x.sched.tasklet_disabled -= 1;
            x.sched.tasklet_disabled == 0
        }
        _ => false,
    };
    if enabled {
        execlists_submission_tasklet(i915, gt, e);
    }
}

/// `execlists_park`: the timers off, the priority hint reset.
pub fn execlists_park(gt: &mut intel_gt, e: usize) {
    if let Some(x) = gt.engine[e].as_mut() {
        x.execlists.timer.cancel();
        x.execlists.preempt.cancel();
        x.sched.queue_priority_hint = i32::MIN;
    }
}

/// `intel_execlists_submission_setup`: the vfuncs as engine flags, the
/// interrupt masks, ELSQ registers, the CSB in the status page, the CCID
/// engine bits.
pub fn intel_execlists_submission_setup(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
    let fw = {
        let base = mmio_base(gt, e);
        i915.uncore.intel_uncore_forcewake_for_reg(RING_EXECLIST_CONTROL(base), FW_REG_WRITE)
    };
    let Some(x) = gt.engine[e].as_mut() else { return };
    let base = x.mmio_base;

    // logical_ring_default_vfuncs: preemption from GRAPHICS_VER 9,
    // timeslices with CONFIG_DRM_I915_TIMESLICE_DURATION.
    x.flags |= I915_ENGINE_SUPPORTS_STATS;
    x.flags |= I915_ENGINE_HAS_SEMAPHORES;
    x.flags |= I915_ENGINE_HAS_PREEMPTION;
    if CONFIG_DRM_I915_TIMESLICE_DURATION != 0 {
        x.flags |= I915_ENGINE_HAS_TIMESLICES;
    }

    // logical_ring_default_irqs: no shift from GRAPHICS_VER 11.
    x.irq_enable_mask = GT_RENDER_USER_INTERRUPT;
    x.irq_keep_mask = GT_CONTEXT_SWITCH_INTERRUPT | GT_CS_MASTER_ERROR_INTERRUPT | GT_WAIT_SEMAPHORE_INTERRUPT;

    // lrc_init_wa_ctx: nothing from GRAPHICS_VER 11.

    x.execlists.submit_reg = RING_EXECLIST_SQ_CONTENTS(base);
    x.execlists.ctrl_reg = RING_EXECLIST_CONTROL(base);
    x.fw_domain = fw;

    x.execlists.csb_size = GEN11_CSB_ENTRIES;

    x.context_tag = (1u64 << 63) - 1;
    x.execlists.ccid |= x.instance << (GEN11_ENGINE_INSTANCE_SHIFT - 32);
    x.execlists.ccid |= x.class << (GEN11_ENGINE_CLASS_SHIFT - 32);
}

/// `ktime_get`, in ns.
fn ktime_get() -> u64 {
    crate::linux::now_us() * 1000
}

/// `intel_engine_context_in` (gt/intel_engine_stats.h).
fn intel_engine_context_in(gt: &mut intel_gt, e: usize) {
    let Some(x) = gt.engine[e].as_mut() else { return };
    if x.stats_active != 0 {
        x.stats_active += 1;
        return;
    }
    x.stats_start = ktime_get();
    x.stats_active += 1;
}

/// `intel_engine_context_out` (gt/intel_engine_stats.h).
fn intel_engine_context_out(gt: &mut intel_gt, e: usize) {
    let Some(x) = gt.engine[e].as_mut() else { return };
    if x.stats_active == 0 {
        return;
    }
    if x.stats_active > 1 {
        x.stats_active -= 1;
        return;
    }
    x.stats_active -= 1;
    x.stats_total += ktime_get().saturating_sub(x.stats_start);
}

/// `intel_engine_get_busy_time` → `execlists_engine_busyness`: the busy
/// time so far in ns, and the sampling time in `now`.
pub fn intel_engine_get_busy_time(gt: &intel_gt, e: usize, now: &mut u64) -> u64 {
    let Some(x) = gt.engine[e].as_ref() else { return 0 };
    let mut total = x.stats_total;
    *now = ktime_get();
    if x.stats_active != 0 {
        total += now.saturating_sub(x.stats_start);
    }
    total
}
