//! The module side of gpudev v0 (docs/plan/GPU_STACK.md §3.3): after the
//! probe the module stays resident as the GPU service of its card. It binds
//! the kernel's buffers into the kernel context's PPGTT, proves a bcs0 copy
//! with a read-back, reports ready and then serves copy jobs. Not a Linux
//! file: Linux has no in-kernel blit on TGL; the copy is encoded as
//! `emit_copy` in gt/intel_migrate.c does it.

use crate::i915_drv::drm_i915_private;
use crate::intel_ggtt::i915_ggtt;
use crate::intel_gt::intel_gt;
use crate::linux::{self, Line};
use crate::regs::*;
use npk_sys::Dma;

const WAIT_IRQ: i32 = 2;
const WAIT_GPU_CMD: i32 = 64;
const OP_COPY_RECT: u32 = 1;
const JOB_BYTES: usize = 32;
const NUM_BOS: usize = 3;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
const ETIME: i32 = 62;
/// Below the kernel's 100 ms, so a late copy is ours to report.
const JOB_TIMEOUT_US: u64 = 80_000;
/// Self-test surface: 64x64 XRGB8888.
const TEST_W: u32 = 64;
const TEST_H: u32 = 64;
const TEST_PITCH: u32 = TEST_W * 4;
const STATS_EVERY: u32 = 600;

/// A kernel buffer bound into the PPGTT.
#[derive(Clone, Copy, Default)]
struct bo {
    phys: u64,
    size: u64,
    pitch: u32,
    width: u32,
    height: u32,
    va: u64,
    bound: bool,
}

struct service {
    bos: [bo; NUM_BOS],
    e: usize,
    jobs: u32,
    sum_us: u64,
    max_us: u64,
    logged_idle: bool,
}

/// The bcs0 engine's index in `gt.engine`.
fn bcs0(gt: &intel_gt) -> Option<usize> {
    gt.engine.iter().position(|x| x.is_some_and(|x| x.class == COPY_ENGINE_CLASS && x.instance == 0))
}

/// Binds `size` bytes of contiguous bus addresses from `phys` into the
/// kernel PPGTT (`i915_vma_pin` with PIN_USER: best fit from the pin bias,
/// then `gen8_ppgtt_alloc` and `gen8_ppgtt_insert`). Returns the address.
fn ppgtt_bind(gt: &mut intel_gt, phys: u64, size: u64, pat_index: u32) -> Result<u64, i32> {
    let vm = gt.vm.as_mut().ok_or(-EINVAL)?;
    let size = size.next_multiple_of(I915_GTT_PAGE_SIZE);
    // The kernel names its buffers by address and treats 0 as unset, and
    // takes only 32-bit names.
    let node = vm.mm.drm_mm_insert_node_in_range_best(size, I915_GTT_PAGE_SIZE, 0, I915_GTT_PAGE_SIZE, 1 << 32)?;
    vm.gen8_ppgtt_alloc(node.start, size)?;
    vm.gen8_ppgtt_insert(node.start, phys, size, pat_index, 0)?;
    Ok(node.start)
}

/// Runs everything the interrupt delivered: the handler, the engines'
/// tasklets, then a retire pass, which parks the GT once it is idle.
fn service_interrupts(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    crate::i915_irq::intel_irq_service(i915);
    crate::intel_gt_requests::intel_engines_cs_irq(i915, gt);
    for e in 0..gt.engine.len() {
        if gt.engine[e].is_some() && gt.engines_initialized && crate::intel_gt_pm::intel_gt_pm_is_awake(gt) {
            crate::intel_gt_requests::intel_engine_flush_submission(i915, gt, e);
        }
    }
    crate::intel_gt_requests::intel_gt_retire_requests(i915, gt);
}

/// One copy on bcs0 from the kernel context (`intel_engine_create_kernel_request`):
/// TLB invalidate, XY_FAST_COPY_BLT, then the closing breadcrumb's flush.
/// Waits for the request on the interrupt.
fn copy_rect(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize, src: u64, src_pitch: u32,
             dst: u64, dst_pitch: u32, x: u32, y: u32, w: u32, h: u32) -> Result<(), i32> {
    if crate::intel_gt::intel_gt_is_wedged(gt) {
        return Err(-EIO);
    }
    let ce = gt.engine[e].and_then(|x| x.kernel_context).ok_or(-EINVAL)? as usize;

    crate::intel_engine_pm::intel_engine_pm_get(i915, gt, e);
    let rq = crate::i915_request::i915_request_create(i915, gt, ce);
    crate::intel_engine_pm::intel_engine_pm_put(i915, gt, e);
    let rq = rq?;

    let mut err = crate::gen8_engine_cs::emit_flush(i915, gt, rq, EMIT_INVALIDATE);
    if err.is_ok() {
        let mut cs = crate::gen8_engine_cs::Cs::new();
        cs.push(GEN9_XY_FAST_COPY_BLT_CMD | (10 - 2));
        cs.push(BLT_DEPTH_32 | dst_pitch);
        cs.push(y << 16 | x);
        cs.push((y + h) << 16 | (x + w));
        cs.push(dst as u32);
        cs.push((dst >> 32) as u32);
        cs.push(y << 16 | x);
        cs.push(src_pitch);
        cs.push(src as u32);
        cs.push((src >> 32) as u32);
        err = crate::i915_request::intel_ring_emit(i915, gt, rq, &cs);
    }
    let rq = crate::i915_request::i915_request_get(gt, rq);
    crate::i915_request::i915_request_add(i915, gt, rq);

    let t0 = linux::now_us();
    while !crate::i915_request::__i915_request_is_complete(gt, rq) {
        if linux::now_us() - t0 > JOB_TIMEOUT_US {
            crate::i915_request::i915_request_put(gt, rq);
            linux::say("gpudev: copy on bcs0 timed out, wedging");
            crate::intel_engine_cs::intel_engine_print_registers(i915, gt, e);
            crate::intel_gt::gen8_check_faults(i915);
            crate::intel_gt::intel_gt_set_wedged(i915, gt);
            return Err(-ETIME);
        }
        crate::i915_irq::intel_irq_wait(i915, 2);
        crate::intel_gt_requests::intel_engines_cs_irq(i915, gt);
        crate::intel_gt_requests::intel_engine_flush_submission(i915, gt, e);
    }
    let failed = gt.requests[rq].error;
    crate::i915_request::i915_request_put(gt, rq);
    crate::intel_gt_requests::intel_gt_retire_requests(i915, gt);
    err?;
    if failed != 0 { Err(failed) } else { Ok(()) }
}

/// The read-back self-test: a pattern copied by bcs0 between two module
/// buffers must arrive.
fn self_test(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) -> bool {
    let pages = (TEST_PITCH * TEST_H).div_ceil(4096) as i32;
    let (Some(src), Some(dst)) = (Dma::alloc(pages), Dma::alloc(pages)) else {
        linux::say("gpudev: self-test buffers could not be allocated");
        return false;
    };
    let mut row = [0u8; TEST_PITCH as usize];
    let zero = [0u8; TEST_PITCH as usize];
    for y in 0..TEST_H {
        for x in 0..TEST_W {
            let v = 0xa500_0000 | (y << 8) | x;
            row[(x * 4) as usize..(x * 4 + 4) as usize].copy_from_slice(&v.to_le_bytes());
        }
        if !src.write(y * TEST_PITCH, &row) || !dst.write(y * TEST_PITCH, &zero) {
            return false;
        }
    }
    let size = (pages as u64) * 4096;
    let (Ok(sva), Ok(dva)) = (ppgtt_bind(gt, src.phys, size, TGL_PAT_CACHE_LLC),
                              ppgtt_bind(gt, dst.phys, size, TGL_PAT_CACHE_LLC)) else {
        linux::say("gpudev: self-test buffers could not be bound");
        return false;
    };
    walk_from_context(gt, e, sva, src.phys);
    walk_from_context(gt, e, dva, dst.phys);
    if let Err(x) = copy_rect(i915, gt, e, sva, TEST_PITCH, dva, TEST_PITCH, 0, 0, TEST_W, TEST_H) {
        Line::new().s("gpudev: self-test copy failed (").dec((-x) as u64).s(")").say();
        return false;
    }
    let mut got = [0u8; TEST_PITCH as usize];
    for y in 0..TEST_H {
        if !dst.read(y * TEST_PITCH, &mut got) {
            return false;
        }
        for x in 0..TEST_W {
            let want = 0xa500_0000 | (y << 8) | x;
            let v = u32::from_le_bytes(got[(x * 4) as usize..(x * 4 + 4) as usize].try_into().unwrap());
            if v != want {
                Line::new().s("gpudev: self-test read back ").hex(v).s(" at ").dec(x as u64).s(",")
                    .dec(y as u64).s(", expected ").hex(want).say();
                return false;
            }
        }
    }
    true
}

/// The rectangle lies inside the buffer.
fn rect_fits(b: &bo, x: u32, y: u32, w: u32, h: u32) -> bool {
    let (Some(x2), Some(y2)) = (x.checked_add(w), y.checked_add(h)) else { return false };
    if w == 0 || h == 0 || x2 > b.width || y2 > b.height || x2 as u64 * 4 > b.pitch as u64 {
        return false;
    }
    (y2 as u64 - 1) * b.pitch as u64 + x2 as u64 * 4 <= b.size
}

fn log_wakerefs(gt: &intel_gt, when: &str) {
    let mut l = Line::new();
    l.s("gpudev: GT wakeref ").s(when).s(": ").dec(gt.wakeref_count as u64).s(", engines");
    for x in gt.engine.iter().flatten() {
        l.s(" ").s(crate::intel_gt::intel_engine_class_repr(x.class)).dec(x.instance as u64).s("=")
            .dec(x.wakeref_count as u64);
    }
    l.say();
}

/// One job; the status for `gpudev_complete`.
fn run_job(i915: &mut drm_i915_private, gt: &mut intel_gt, svc: &mut service, job: &[u8]) -> i32 {
    let w = |i: usize| u32::from_le_bytes(job[i * 4..i * 4 + 4].try_into().unwrap());
    let (op, a) = (w(1), [w(2), w(3), w(4), w(5), w(6), w(7)]);
    if op != OP_COPY_RECT {
        return -EINVAL;
    }
    let (s, d) = (a[0] as usize, a[1] as usize);
    if s >= NUM_BOS || d >= NUM_BOS || !svc.bos[s].bound || !svc.bos[d].bound {
        return -EINVAL;
    }
    let (src, dst) = (svc.bos[s], svc.bos[d]);
    let (x, y, wd, ht) = (a[2], a[3], a[4], a[5]);
    if !rect_fits(&src, x, y, wd, ht) || !rect_fits(&dst, x, y, wd, ht) {
        return -EINVAL;
    }
    let t0 = linux::now_us();
    let r = copy_rect(i915, gt, svc.e, src.va, src.pitch, dst.va, dst.pitch, x, y, wd, ht);
    let us = linux::now_us() - t0;
    svc.jobs += 1;
    svc.sum_us += us;
    svc.max_us = svc.max_us.max(us);
    if svc.jobs == 1 {
        Line::new().s("gpudev: first job, copy ").dec(wd as u64).s("x").dec(ht as u64).s(" bo ")
            .dec(s as u64).s(" -> bo ").dec(d as u64).s(" in ").dec(us).s(" us").say();
        log_wakerefs(gt, "after the first job");
    } else if svc.jobs % STATS_EVERY == 0 {
        Line::new().s("gpudev: ").dec(svc.jobs as u64).s(" jobs, average ")
            .dec(svc.sum_us / STATS_EVERY as u64).s(" us, max ").dec(svc.max_us).s(" us").say();
        svc.sum_us = 0;
        svc.max_us = 0;
    }
    match r {
        Ok(()) => 0,
        Err(x) => x,
    }
}

/// Serves until the GT is wedged; returns early (before ready) when
/// registering, binding or the self-test fails. Either way the caller
/// goes on to hand the card back; on module exit the kernel copies on
/// the CPU again.
pub fn gpudev_serve(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt) {
    let Some(e) = bcs0(gt) else {
        linux::say("gpudev: no bcs0, not serving");
        return;
    };
    let rc = npk_sys::gpudev_register(0);
    if rc != 0 {
        Line::new().s("gpudev: register failed (").dec((-rc) as u64).s(")").say();
        return;
    }

    let mut svc = service { bos: [bo::default(); NUM_BOS], e, jobs: 0, sum_us: 0, max_us: 0, logged_idle: false };
    for id in 0..NUM_BOS {
        let mut out = [0u8; 32];
        if npk_sys::gpudev_bo_info(id as i32, &mut out) != 0 {
            Line::new().s("gpudev: no buffer ").dec(id as u64).say();
            npk_sys::gpudev_ready(0);
            return;
        }
        let q = |o: usize| u64::from_le_bytes(out[o..o + 8].try_into().unwrap());
        let d = |o: usize| u32::from_le_bytes(out[o..o + 4].try_into().unwrap());
        svc.bos[id] = bo { phys: q(0), size: q(8), pitch: d(16), width: d(20), height: d(24), ..Default::default() };
        Line::new().s("gpudev: bo ").dec(id as u64).s(": phys ").hex((q(0) >> 32) as u32).s("_").hex(q(0) as u32)
            .s(", size ").hex(q(8) as u32).s(", ").dec(d(20) as u64).s("x").dec(d(24) as u64).s(", pitch ")
            .dec(d(16) as u64).s(", format ").hex(d(28)).say();
    }

    // Buffer 0 is the firmware framebuffer taken over by intel_plane_initial:
    // its pages are the stolen range behind its GGTT node, mapped uncached
    // like the initial plane object.
    let fb = (0..i915.display.num_planes).find_map(|p| i915.display.planes[p].state.ggtt_vma);
    match fb {
        Some(v) if v.node.size >= svc.bos[0].size => {
            let (phys, present, _) = ggtt.gen8_ggtt_read_entry(v.node.start);
            if !present {
                linux::say("gpudev: framebuffer PTE not present");
                npk_sys::gpudev_ready(0);
                return;
            }
            svc.bos[0].phys = phys;
            match ppgtt_bind(gt, phys, svc.bos[0].size, v.pat_index) {
                Ok(va) => {
                    svc.bos[0].va = va;
                    svc.bos[0].bound = true;
                }
                Err(_) => {}
            }
        }
        _ => linux::say("gpudev: no taken-over framebuffer matches buffer 0"),
    }
    for id in 1..NUM_BOS {
        let b = svc.bos[id];
        if b.phys == 0 || b.phys & (I915_GTT_PAGE_SIZE - 1) != 0 {
            continue;
        }
        if let Ok(va) = ppgtt_bind(gt, b.phys, b.size, TGL_PAT_CACHE_LLC) {
            svc.bos[id].va = va;
            svc.bos[id].bound = true;
        }
    }
    for id in 0..NUM_BOS {
        if !svc.bos[id].bound {
            Line::new().s("gpudev: buffer ").dec(id as u64).s(" could not be bound, not serving").say();
            npk_sys::gpudev_ready(0);
            return;
        }
        npk_sys::gpudev_bo_bound(id as i32, svc.bos[id].va as i64);
        Line::new().s("gpudev: bo ").dec(id as u64).s(" at PPGTT ").hex(svc.bos[id].va as u32).say();
    }

    let ok = self_test(i915, gt, e);
    Line::new().s("gpudev: bcs0 self-test ").s(if ok { "passed" } else { "FAILED" }).say();
    npk_sys::gpudev_ready(ok as i32);
    if !ok {
        return;
    }
    log_wakerefs(gt, "at ready");

    let mut jobs = [0u8; JOB_BYTES * 8];
    loop {
        // While the GT is awake a missed interrupt must not keep it so.
        let timeout = if crate::intel_gt_pm::intel_gt_pm_is_awake(gt) { 100 } else { -1 };
        npk_sys::wait(WAIT_IRQ | WAIT_GPU_CMD, timeout);
        service_interrupts(i915, gt);

        if !svc.logged_idle && !crate::intel_gt_pm::intel_gt_pm_is_awake(gt) {
            svc.logged_idle = true;
            log_wakerefs(gt, "idle");
            crate::intel_rc6::report(i915, gt, "idle");
        }

        let n = npk_sys::gpudev_poll(&mut jobs);
        for j in 0..n.max(0) as usize {
            let job = &jobs[j * JOB_BYTES..(j + 1) * JOB_BYTES];
            let seq = u32::from_le_bytes(job[0..4].try_into().unwrap());
            let status = run_job(i915, gt, &mut svc, job);
            npk_sys::gpudev_complete(seq as i32, status);
        }

        if crate::intel_gt::intel_gt_is_wedged(gt) {
            linux::say("gpudev: GT wedged, giving the card back");
            return;
        }
    }
}

/// Logs the PPGTT walk of `va` from the PML4 the engine's kernel context
/// image holds (CTX_PDP0), with the page it should reach.
fn walk_from_context(gt: &intel_gt, e: usize, va: u64, expect: u64) {
    let Some(ce) = gt.engine[e].and_then(|x| x.kernel_context) else { return };
    let Some(state) = gt.contexts[ce as usize].state_obj.and_then(|o| o.map()) else {
        linux::say("gpudev: context image has no CPU mapping, walk skipped");
        return;
    };
    let off = LRC_STATE_OFFSET as u32;
    let lo = state.r32(off + CTX_PDP0_LDW * 4);
    let hi = state.r32(off + CTX_PDP0_UDW * 4);
    Line::new().s("gpudev: va ").hex(va as u32).s(" should reach ").hex((expect >> 32) as u32).s("_")
        .hex(expect as u32).s(", context PDP0 ").hex(hi).s("_").hex(lo).say();
    if let Some(vm) = gt.vm.as_ref() {
        vm.walk_log((hi as u64) << 32 | lo as u64, va);
    }
}
