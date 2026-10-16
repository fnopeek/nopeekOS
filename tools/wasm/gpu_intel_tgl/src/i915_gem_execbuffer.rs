//! gem/i915_gem_execbuffer.c (Linux 6.18.55), the softpin path, behind
//! virglrenderer's `i915_ccmd_gem_execbuffer2` (src/drm/i915/i915_ccmd.c).
//!
//! Every object comes with EXEC_OBJECT_PINNED at the address the guest's
//! Mesa chose; it is bound there in the context's address space, and the
//! batch runs unprivileged from it. Not kept: relocations (an object that
//! would need one is refused: the module cannot write the guest's pages),
//! placing an unpinned object, the command parser (Gen12 has none),
//! fences in and out of the guest (the guest kernel waits for its in-fences;
//! the out-fence is the ring's counter, `npk_gpudev_ring`). Implicit sync
//! is kept: a request waits for the fences in its objects' reservations,
//! shared with every client that imported the buffer. Where Linux
//! would wait for an object, a ring or the request table, the guest is told
//! EAGAIN, which Mesa's `intel_ioctl` repeats.

use crate::i915_gem_context::*;
use crate::i915_uapi::*;
use crate::linux;
use crate::regs::I915_GTT_PAGE_SIZE;

/// `sizeof(struct i915_ccmd_gem_execbuffer2_req)`.
const REQ_BYTES: usize = 48;
/// The address width of the PPGTT (48 bits).
const VM_TOTAL: u64 = 1 << 48;

/// `gen8_noncanonical_addr` / `gen8_canonical_addr`.
fn noncanonical(a: u64) -> u64 {
    a & (VM_TOTAL - 1)
}
fn canonical(a: u64) -> u64 {
    ((a << 16) as i64 >> 16) as u64
}

/// `user_ring_map` and `eb_select_legacy_ring`: the engine id for a context
/// without an engine map. A BSD request without a choice takes the first
/// video engine.
fn eb_select_legacy_ring(gt: &crate::intel_gt::intel_gt, flags: u64) -> Result<u8, i32> {
    let user_ring_id = flags & I915_EXEC_RING_MASK;
    let bsd_idx = flags & I915_EXEC_BSD_MASK;
    if user_ring_id != I915_EXEC_BSD && bsd_idx != 0 {
        return Err(EINVAL);
    }
    let (class, instance) = match user_ring_id {
        I915_EXEC_DEFAULT | I915_EXEC_RENDER => (I915_ENGINE_CLASS_RENDER, 0),
        I915_EXEC_BLT => (I915_ENGINE_CLASS_COPY, 0),
        I915_EXEC_BSD => match bsd_idx {
            I915_EXEC_BSD_DEFAULT | I915_EXEC_BSD_RING1 => (I915_ENGINE_CLASS_VIDEO, 0),
            I915_EXEC_BSD_RING2 => (I915_ENGINE_CLASS_VIDEO, 1),
            _ => return Err(EINVAL),
        },
        I915_EXEC_VEBOX => (I915_ENGINE_CLASS_VIDEO_ENHANCE, 0),
        _ => return Err(EINVAL),
    };
    intel_engine_lookup_user(gt, class, instance).ok_or(EINVAL)
}

/// One object of the request.
#[derive(Clone, Copy)]
struct eb_vma {
    obj: usize,
    offset: u64,
    flags: u64,
}

/// `I915_CCMD_GEM_EXECBUFFER2(_V2)`. Writes the response (`ret` at 4) and
/// returns its length; Err is a malformed request.
pub fn i915_ccmd_gem_execbuffer2(st: &mut nctx, d: &mut drm, c: u8, req: &[u8], v2: bool, rsp: &mut [u8])
                                 -> Result<usize, i32> {
    let flags = rd64(req, 16);
    let context_id = rd64(req, 24);
    let buffer_count = rd32(req, 32) as usize;
    let batch_start_offset = rd32(req, 36);
    let batch_len = rd32(req, 40);
    let relocs_count = rd32(req, 44) as usize;
    let len = rd32(req, 4) as usize;
    let buffers_size = buffer_count.checked_mul(EXEC_OBJECT2_BYTES).ok_or(-EINVAL)?;
    let relocs_size = relocs_count.checked_mul(RELOCATION_ENTRY_BYTES).ok_or(-EINVAL)?;
    if len != REQ_BYTES + buffers_size + relocs_size || req.len() < len {
        return Err(-EINVAL);
    }
    if context_id == 0 || context_id > 8 {
        return Err(-EINVAL);
    }
    let mut ring = 1 + (flags & I915_EXEC_RING_MASK);
    if v2 {
        ring += (context_id - 1) * 8;
    }
    if ring >= MAX_RINGS as u64 {
        return Err(-EINVAL);
    }
    wr32(rsp, 0, 8);
    *REASON.lock() = "";
    let ret = match execbuffer(st, d, c, req, flags, context_id as u32, buffer_count, batch_start_offset, batch_len,
                               relocs_count, ring as u8) {
        Ok(()) => 0,
        Err(e) => e,
    };
    wr32(rsp, 4, ret as u32);
    log_execbuffer(c, ring as u32, buffer_count, batch_len, ret);
    Ok(8)
}

/// Why the last execbuffer was refused.
static REASON: spin::Mutex<&'static str> = spin::Mutex::new("");

/// `e`, with `reason` kept for the log line.
fn why(e: i32, reason: &'static str) -> i32 {
    *REASON.lock() = reason;
    e
}

/// The first execbuffers of the boot, then only the failed ones, up to a
/// fixed number of lines; a refusal names its reason.
fn log_execbuffer(c: u8, ring: u32, count: usize, batch_len: u32, ret: i32) {
    use core::sync::atomic::{AtomicU32, Ordering};
    const FIRST: u32 = 8;
    const MAX_LINES: u32 = 40;
    static SEEN: AtomicU32 = AtomicU32::new(0);
    static LINES: AtomicU32 = AtomicU32::new(0);
    let n = SEEN.fetch_add(1, Ordering::Relaxed);
    if (n < FIRST || ret != 0) && LINES.fetch_add(1, Ordering::Relaxed) < MAX_LINES {
        let mut l = linux::Line::new();
        l.s("native context: execbuffer ").dec(n as u64 + 1).s(" client slot ").dec(c as u64)
            .s(" ring ").dec(ring as u64).s(", ").dec(count as u64).s(" objects, batch ").dec(batch_len as u64)
            .s(" bytes -> ret ").dec(ret as u64);
        if ret != 0 {
            l.s(" (").s(*REASON.lock()).s(")");
        }
        l.say();
    }
}

/// Earlier batches of address space `vm` that wrote through an unbound
/// address, seen in its poisoned scratch page; a fixed number of lines.
fn report_scratch_writes(st: &nctx, vm: u8, c: u8, ctx_id: u32) {
    use core::sync::atomic::{AtomicU32, Ordering};
    const MAX_LINES: u32 = 8;
    static LINES: AtomicU32 = AtomicU32::new(0);
    let Some(p) = st.vms[vm as usize].ppgtt.as_ref() else { return };
    if let Some((off, val)) = p.scratch_written()
        && LINES.fetch_add(1, Ordering::Relaxed) < MAX_LINES {
        linux::Line::new().s("native context: client slot ").dec(c as u64).s(" context ").dec(ctx_id as u64)
            .s(": an earlier batch wrote through an unbound address (scratch page byte ").hex(off)
            .s(" = ").hex(val as u32).s(")").say();
    }
}

/// `i915_gem_do_execbuffer` for softpinned objects; Err is the positive
/// errno for `ret`.
#[allow(clippy::too_many_arguments)]
fn execbuffer(st: &mut nctx, d: &mut drm, c: u8, req: &[u8], flags: u64, ctx_id: u32, count: usize,
              batch_start_offset: u32, batch_len: u32, relocs_count: usize, ring: u8) -> Result<(), i32> {
    // virglrenderer: res_ids to objects, relocation counts against the total.
    let mut vmas = [eb_vma { obj: 0, offset: 0, flags: 0 }; 2048];
    if count > vmas.len() {
        return Err(why(EINVAL, "more than 2048 objects"));
    }
    let mut relocs = 0usize;
    for i in 0..count {
        let e = &req[REQ_BYTES + i * EXEC_OBJECT2_BYTES..];
        let obj = st.obj(c, rd32(e, 0)).ok_or(why(EINVAL, "unknown object"))?;
        let nreloc = rd32(e, 4) as usize;
        relocs = relocs.checked_add(nreloc).filter(|&r| r <= relocs_count).ok_or(why(EINVAL, "relocation count"))?;
        vmas[i] = eb_vma { obj, offset: rd64(e, 24), flags: rd64(e, 32) };
        if nreloc != 0 && flags & I915_EXEC_NO_RELOC == 0 {
            // eb_relocate_parse would write into the object.
            return Err(why(EINVAL, "relocations"));
        }
        let pad_to_size = rd64(e, 40);
        // eb_validate_vma
        let f = vmas[i].flags;
        if f & EXEC_OBJECT_UNKNOWN_FLAGS != 0 {
            return Err(why(EINVAL, "unknown object flags"));
        }
        let alignment = rd64(e, 16);
        if alignment != 0 && !alignment.is_power_of_two() {
            return Err(why(EINVAL, "alignment"));
        }
        if f & EXEC_OBJECT_PINNED == 0 {
            // eb_reserve would place it; the guest could not learn where.
            return Err(why(EINVAL, "object not pinned"));
        }
        if vmas[i].offset != canonical(vmas[i].offset & !(I915_GTT_PAGE_SIZE - 1)) {
            return Err(why(EINVAL, "offset not canonical or not page-aligned"));
        }
        if f & EXEC_OBJECT_PAD_TO_SIZE != 0 && pad_to_size & (I915_GTT_PAGE_SIZE - 1) != 0 {
            return Err(why(EINVAL, "pad_to_size"));
        }
        vmas[i].offset = noncanonical(vmas[i].offset);
        if alignment != 0 && vmas[i].offset & (alignment - 1) != 0 {
            return Err(why(EINVAL, "offset misaligned"));
        }
        if vmas[i].offset + st.objs[obj].pages as u64 * I915_GTT_PAGE_SIZE > VM_TOTAL {
            return Err(why(EINVAL, "object past the address space"));
        }
        // eb_lookup_vmas: an object named twice.
        if vmas[..i].iter().any(|v| v.obj == obj) {
            return Err(why(EINVAL, "object named twice"));
        }
    }

    // i915_gem_check_execbuffer and the flags.
    if flags & I915_EXEC_UNKNOWN_FLAGS != 0 || (batch_start_offset | batch_len) & 7 != 0 {
        return Err(why(EINVAL, "flags or batch alignment"));
    }
    if flags & (I915_EXEC_FENCE_IN | I915_EXEC_FENCE_SUBMIT | I915_EXEC_FENCE_ARRAY | I915_EXEC_USE_EXTENSIONS) != 0 {
        return Err(why(EINVAL, "fences or extensions"));
    }
    if flags & I915_EXEC_SECURE != 0 {
        return Err(EPERM);
    }
    if count < 1 {
        return Err(why(EINVAL, "no objects"));
    }

    // eb_select_context, eb_select_engine.
    let x = st.ctx(c, ctx_id).ok_or(ENOENT)?;
    if st.ctxs[x].banned {
        return Err(EIO);
    }
    // The slot names the hardware context: two slots on one engine are two
    // contexts, each with its own image (Linux creates one per slot).
    let (slot, engine) = if st.ctxs[x].user_engines {
        let idx = (flags & I915_EXEC_RING_MASK) as usize;
        if idx >= st.ctxs[x].n_engines as usize || st.ctxs[x].engines[idx] == NONE8 {
            return Err(why(EINVAL, "engine slot"));
        }
        (idx as u8, st.ctxs[x].engines[idx])
    } else {
        let e = eb_select_legacy_ring(d.gt, flags).map_err(|e| why(e, "legacy ring"))?;
        (e, e)
    };
    if crate::intel_gt::intel_gt_is_wedged(d.gt) {
        return Err(EIO);
    }

    // The batch: first or last, not written, inside its object.
    let bi = if flags & I915_EXEC_BATCH_FIRST != 0 { 0 } else { count - 1 };
    let batch = vmas[bi];
    if batch.flags & EXEC_OBJECT_WRITE != 0 {
        return Err(why(EINVAL, "batch written"));
    }
    let bsize = st.objs[batch.obj].size;
    let blen = if batch_len == 0 { bsize.saturating_sub(batch_start_offset as u64) } else { batch_len as u64 };
    if batch_start_offset as u64 + blen > bsize || blen == 0 {
        return Err(why(EINVAL, "batch outside its object"));
    }

    // eb_reserve with PIN_OFFSET_FIXED: each object at its offset. A
    // binding elsewhere, or another object in the way, is unbound once idle
    // (`i915_gem_evict_for_node`), also one of this request that has not
    // been placed yet (Mesa may give a cached object a new address); one
    // already placed for this request is pinned there, and that is -ENOSPC.
    let vm = st.ctxs[x].vm;
    report_scratch_writes(st, vm, c, ctx_id);
    for i in 0..count {
        let v = vmas[i];
        let size = st.objs[v.obj].pages as u64 * I915_GTT_PAGE_SIZE;
        let o = st.objs[v.obj];
        if (0..2).any(|k| o.vm[k] == vm && o.start[k] == v.offset) {
            continue;
        }
        for k in 0..2 {
            if o.vm[k] == vm {
                if st.obj_busy(d.gt, v.obj) {
                    return Err(why(EAGAIN, "moving a busy object"));
                }
                st.obj_unbind(v.obj, k);
            }
        }
        for j in 0..MAX_OBJECTS {
            let other = st.objs[j];
            if !other.used || j == v.obj {
                continue;
            }
            for k in 0..2 {
                if other.vm[k] != vm {
                    continue;
                }
                let (s, e) = (other.start[k], other.start[k] + other.pages as u64 * I915_GTT_PAGE_SIZE);
                if s < v.offset + size && v.offset < e {
                    if vmas[..i].iter().any(|w| w.obj == j) {
                        return Err(why(ENOSPC, "two objects of the request overlap"));
                    }
                    if st.obj_busy(d.gt, j) {
                        return Err(why(EAGAIN, "a busy object in the way"));
                    }
                    st.obj_unbind(j, k);
                }
            }
        }
        // The old translations of what was unbound go before the new one.
        st.tlb_flush(d.i915, d.gt);
        st.obj_bind(v.obj, vm, v.offset).map_err(|e| why(-e, "bind"))?;
    }

    // eb_pin_engine, eb_throttle: the request must fit without waiting.
    let ce = st.ctx_engine(d, x, slot, engine).map_err(|e| why(-e, "hardware context"))?;
    crate::intel_gt_requests::retire_requests(d.i915, d.gt, ce);
    if !st.can_submit(d.gt, Some(ce)) || d.gt.contexts[ce].ring.intel_ring_update_space() < 4096 {
        return Err(why(EAGAIN, "ring or request table full"));
    }

    // eb_move_to_gpu's implicit sync: the fences each object's reservation
    // makes the request wait for (none for EXEC_OBJECT_ASYNC), and room for
    // the request's own. Where Linux would grow a table, EAGAIN.
    let mut awaits = await_list::EMPTY;
    for v in &vmas[..count] {
        let write = v.flags & EXEC_OBJECT_WRITE != 0;
        if v.flags & EXEC_OBJECT_ASYNC == 0 && !st.resv_await(d.gt, v.obj, ce as u16, write, &mut awaits) {
            return Err(why(EAGAIN, "too many fences to wait for"));
        }
        if !st.resv_room(d.gt, v.obj, ce as u16, write) {
            return Err(why(EAGAIN, "reservation full"));
        }
    }

    // i915_request_create, eb_request_submit, i915_request_add.
    crate::intel_engine_pm::intel_engine_pm_get(d.i915, d.gt, engine as usize);
    let rq = crate::i915_request::i915_request_create(d.i915, d.gt, ce);
    crate::intel_engine_pm::intel_engine_pm_put(d.i915, d.gt, engine as usize);
    let rq = match rq {
        Ok(rq) => rq,
        Err(e) if e == -ENOMEM || e == -ENOSPC => return Err(why(EAGAIN, "no request free")),
        Err(e) => return Err(why(-e, "request")),
    };
    d.gt.requests[rq].prio = st.ctxs[x].priority;
    let mut err = crate::gen8_engine_cs::gen8_emit_init_breadcrumb(d.i915, d.gt, rq);
    if err.is_ok() {
        err = crate::gen8_engine_cs::gen8_emit_bb_start(d.i915, d.gt, rq,
                                                        batch.offset + batch_start_offset as u64, 0);
    }
    if let Err(e) = err {
        // eb_requests_add after a failed submit: the payload is skipped.
        d.gt.requests[rq].error = e;
    }
    for &(fce, fseqno) in &awaits.f[..awaits.n] {
        crate::i915_request::i915_request_await(d.gt, rq, fce, fseqno);
    }
    let rq = crate::i915_request::i915_request_get(d.gt, rq);
    let seqno = d.gt.requests[rq].seqno;
    crate::i915_request::i915_request_add(d.i915, d.gt, rq);

    // eb_move_to_gpu: the request in each object's reservation.
    let class = d.gt.engine[engine as usize].map(|e| uabi_class(e.class)).unwrap_or(0) as u8;
    for v in &vmas[..count] {
        st.resv_add(d.gt, v.obj, ce as u16, seqno, class, v.flags & EXEC_OBJECT_WRITE != 0);
    }
    st.pend_add(c, ring, rq, x, engine);
    st.report_ring(c, ring);
    // Once per boot, with a client's work on the engine: the registers its
    // batches depend on, read back.
    {
        use core::sync::atomic::{AtomicBool, Ordering};
        static VERIFIED: AtomicBool = AtomicBool::new(false);
        if !VERIFIED.swap(true, Ordering::Relaxed) {
            crate::intel_workarounds::verify_all(d.i915, d.gt, "first client batch");
        }
    }
    if let Err(e) = err {
        linux::say("native context: a batch could not be emitted, its request is skipped");
        return Err(-e);
    }
    Ok(())
}
