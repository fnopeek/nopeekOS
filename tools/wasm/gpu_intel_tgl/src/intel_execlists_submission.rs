//! gt/intel_execlists_submission.c (Linux 6.18.55): submission through the
//! execlist submit queue (ELSQ, GRAPHICS_VER 11+), the context status
//! buffer, and the engine reset hooks, for GRAPHICS_VER 12 below 12.55.
//!
//! The tasklet runs synchronously wherever Linux schedules it; the module
//! is single-threaded. Not kept: virtual engines, timeslicing and the
//! preemption timer (no timers run here; a request is only queued behind
//! completed ones, so neither comes up), GVT, error capture. An engine
//! error (a CS error interrupt or an impossible CSB event) wedges the GT
//! instead of Linux's per-engine reset.

use crate::i915_drv::drm_i915_private;
use crate::i915_request::{__i915_request_is_complete, i915_request_get, i915_request_put};
use crate::intel_gt::intel_gt;
use crate::linux::{self, Line};
use crate::intel_workarounds::{masked_bit_disable, masked_bit_enable};
use crate::regs::*;

const PORTS: usize = 2;
/// Requests an engine's queue and active list hold.
const SCHED_MAX: usize = 16;

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
        }
    }
}

const ERROR_CSB: u32 = 1 << 31;

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

/// `add_to_engine`.
pub fn add_to_engine(gt: &mut intel_gt, e: usize, rq: usize) {
    if let Some(x) = gt.engine[e].as_mut() {
        let s = &mut x.sched;
        if s.nrequests < SCHED_MAX {
            s.requests[s.nrequests] = rq as u16;
            s.nrequests += 1;
        }
    }
}

/// `remove_from_engine`.
pub fn remove_from_engine(gt: &mut intel_gt, e: usize, rq: usize) {
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
    let prio = gt.requests[rq].prio;
    let reqs = gt.requests;
    let Some(x) = gt.engine[e].as_mut() else { return };
    let s = &mut x.sched;
    if s.nqueue == SCHED_MAX {
        linux::say("warning: engine queue full");
        return;
    }
    let at = s.queue[..s.nqueue].iter().position(|&r| reqs[r as usize].prio < prio).unwrap_or(s.nqueue);
    s.queue.copy_within(at..s.nqueue, at + 1);
    s.queue[at] = rq as u16;
    s.nqueue += 1;
    gt.requests[rq].pqueue = true;
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
}

/// `execlists_schedule_in`.
fn execlists_schedule_in(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize) {
    let ce = gt.requests[rq].ce as usize;
    if gt.contexts[ce].inflight == 0 {
        __execlists_schedule_in(i915, gt, e, rq);
        gt.contexts[ce].inflight_engine = true;
    }
    gt.contexts[ce].inflight += 1;
}

/// `__execlists_schedule_out`.
fn __execlists_schedule_out(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, rq: usize) {
    let ce = gt.requests[rq].ce as usize;
    let mut ccid = gt.contexts[ce].ccid;
    ccid >>= GEN11_SW_CTX_ID_SHIFT - 32;
    ccid &= GEN12_MAX_CONTEXT_HW_ID;

    let Some(x) = gt.engine[e].as_mut() else { return };
    if ccid < 64 && ccid != 0 {
        x.context_tag |= 1u64 << (ccid - 1);
    }
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
        // need_preempt / timeslice_expired: requests are only queued behind
        // completed ones here, so neither triggers.
        let hint = gt.engine[e].map(|y| y.sched.queue_priority_hint).unwrap_or(i32::MIN);
        if gt.requests[l].prio.max(I915_PRIORITY_NORMAL as i32 - 1) < hint
            && gt.engine[e].map(|y| y.flags & I915_ENGINE_HAS_SEMAPHORES != 0).unwrap_or(false) {
            linux::say("warning: preemption wanted, not ported; continuing in order");
        }
        if x.inflight[active + 1].is_some() {
            return;
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
    set_el(gt, e, x);
}

/// `post_process_csb`.
fn post_process_csb(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, inactive: &[u16]) {
    for &r in inactive {
        execlists_schedule_out(i915, gt, e, r as usize);
    }
}

/// `execlists_submission_tasklet` (or `nop_submission_tasklet` once wedged).
pub fn execlists_submission_tasklet(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) {
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

    let err = el(gt, e).error_interrupt;
    if err != 0 {
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
    }

    post_process_csb(i915, gt, e, &inactive[..n]);
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
/// completed goes back to the queue.
fn __unwind_incomplete_requests(gt: &mut intel_gt, e: usize) {
    let s = gt.engine[e].map(|x| x.sched).unwrap_or_default();
    for i in (0..s.nrequests).rev() {
        let rq = s.requests[i] as usize;
        remove_from_engine(gt, e, rq);
        if __i915_request_is_complete(gt, rq) {
            continue;
        }
        crate::i915_request::__i915_request_unsubmit(gt, rq);
        queue_request(gt, e, rq);
        let ce = gt.requests[rq].ce as usize;
        let ring = gt.contexts[ce].ring;
        if ring.intel_ring_direction(gt.requests[rq].tail, ring.tail.wrapping_add(8)) > 0 {
            gt.contexts[ce].lrca |= CTX_DESC_FORCE_RESTORE as u32;
        }
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

/// `execlists_park`: no timers; the priority hint reset.
pub fn execlists_park(gt: &mut intel_gt, e: usize) {
    if let Some(x) = gt.engine[e].as_mut() {
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
