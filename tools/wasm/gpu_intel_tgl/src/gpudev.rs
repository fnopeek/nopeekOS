//! The module side of gpudev v1 (docs/plan/GPU_STACK.md §3.3): after the
//! probe the module stays resident as the GPU service of its card. It binds
//! the kernel's fixed buffers into the kernel context's PPGTT, proves a bcs0
//! copy and the GPGPU path with read-backs, reports what it can serve, and
//! then serves jobs: copies on bcs0, buffers the kernel registers and
//! retires, and scene lists run by the compose programs on rcs0
//! (compose.rs). Not a Linux file: Linux has no in-kernel blit on TGL; the
//! copy is encoded as `emit_copy` in gt/intel_migrate.c does it.

use crate::drm_mm::drm_mm_node;
use crate::gen8_ppgtt::i915_ppgtt;
use crate::i915_drv::drm_i915_private;
use crate::intel_ggtt::i915_ggtt;
use crate::intel_gt::intel_gt;
use crate::linux::{self, Line};
use crate::regs::*;
use npk_sys::Dma;

const WAIT_IRQ: i32 = 2;
const WAIT_GPU_CMD: i32 = 64;
const VERSION: i32 = 1;
const OP_COPY_RECT: u32 = 1;
const OP_BO_ADD: u32 = 2;
const OP_BO_REMOVE: u32 = 3;
const OP_SCENE: u32 = 4;
const CAP_COPY: i32 = 1;
const CAP_SCENE: i32 = 2;
const JOB_BYTES: usize = 32;
const BO_INFO_BYTES: usize = 40;
/// Buffers 0..NUM_BOS are fixed; the kernel numbers the rest from there.
const NUM_BOS: usize = 3;
/// Registered buffers the module holds at once.
const MAX_DYN: usize = 256;
/// Where dynamic buffers are bound in the PPGTT: above the fixed ones,
/// which the kernel names by 32-bit address.
const DYN_START: u64 = 1 << 32;
const DYN_END: u64 = 1 << 47;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
const ENOSPC: i32 = 28;
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
    id: u32,
    /// Page of the first pixel.
    phys: u64,
    /// Bytes from the first pixel.
    size: u64,
    pitch: u32,
    width: u32,
    height: u32,
    /// Byte offset of the first pixel in its page.
    offset: u32,
    pages: u32,
    /// GPU address of the first pixel.
    va: u64,
    /// The PPGTT range of a dynamic buffer.
    node: drm_mm_node,
    bound: bool,
}

struct service {
    bos: [bo; NUM_BOS],
    dyn_bos: [Option<bo>; MAX_DYN],
    e: usize,
    copy_ok: bool,
    jobs: u32,
    sum_us: u64,
    max_us: u64,
    scenes: u32,
    scene_sum_us: u64,
    scene_max_us: u64,
    logged_idle: bool,
    gpgpu: Option<crate::gpgpu::gpgpu>,
    composer: Option<crate::compose::composer>,
}

impl service {
    /// A bound buffer by kernel id.
    fn bo(&self, id: u32) -> Option<bo> {
        let b = if (id as usize) < NUM_BOS {
            Some(self.bos[id as usize])
        } else {
            self.dyn_bos.iter().flatten().find(|b| b.id == id).copied()
        };
        b.filter(|b| b.bound)
    }
}

impl crate::compose::buffers for service {
    fn surface(&self, id: u32) -> Option<crate::compose::surface> {
        self.bo(id).map(|b| crate::compose::surface {
            va: b.va,
            pitch: b.pitch,
            width: b.width,
            height: b.height,
            size: b.size,
        })
    }
}

/// `npk_gpudev_bo_info`, parsed.
fn bo_info(id: u32) -> Option<bo> {
    let mut out = [0u8; BO_INFO_BYTES];
    if npk_sys::gpudev_bo_info(id as i32, &mut out) != 0 {
        return None;
    }
    let q = |o: usize| u64::from_le_bytes(out[o..o + 8].try_into().unwrap());
    let d = |o: usize| u32::from_le_bytes(out[o..o + 4].try_into().unwrap());
    Some(bo { id, phys: q(0), size: q(8), pitch: d(16), width: d(20), height: d(24), offset: d(32), pages: d(36),
              ..Default::default() })
}

/// The bcs0 engine's index in `gt.engine`.
fn bcs0(gt: &intel_gt) -> Option<usize> {
    gt.engine.iter().position(|x| x.is_some_and(|x| x.class == COPY_ENGINE_CLASS && x.instance == 0))
}

/// Binds `size` bytes of contiguous bus addresses from `phys` into the
/// kernel PPGTT (`i915_vma_pin` with PIN_USER: best fit from the pin bias,
/// then `gen8_ppgtt_alloc` and `gen8_ppgtt_insert`). Returns the address.
pub(crate) fn ppgtt_bind(gt: &mut intel_gt, phys: u64, size: u64, pat_index: u32) -> Result<u64, i32> {
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

    // The compositor stalls on this copy: boost a request that has not
    // started, as a waiter in `i915_gem_object_boost` does.
    if !crate::i915_request::__i915_request_has_started(gt, rq) {
        crate::intel_rps::intel_rps_boost(gt, rq);
    }
    crate::intel_rps::intel_rps_run_pending(i915, gt);

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
        crate::intel_rps::intel_rps_run_pending(i915, gt);
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

/// Binds `pages` pages of buffer `id`, from the kernel's page list, at
/// `start`: the tree first, then one insert per physically consecutive run.
fn bind_pages(vm: &mut i915_ppgtt, id: u32, start: u64, pages: u32) -> Result<(), i32> {
    vm.gen8_ppgtt_alloc(start, pages as u64 * I915_GTT_PAGE_SIZE)?;
    let mut list = [0u64; 64];
    let mut at = 0u32;
    // (first page, its bus address, pages)
    let mut run: Option<(u32, u64, u32)> = None;
    while at < pages {
        let want = ((pages - at) as usize).min(list.len());
        let n = npk_sys::gpudev_bo_pages(id as i32, at as i32, &mut list[..want]);
        if n <= 0 || n as usize > want {
            return Err(-EINVAL);
        }
        for &phys in &list[..n as usize] {
            if phys & (I915_GTT_PAGE_SIZE - 1) != 0 {
                return Err(-EINVAL);
            }
            run = match run {
                Some((f, p0, len)) if p0 + len as u64 * I915_GTT_PAGE_SIZE == phys => Some((f, p0, len + 1)),
                prev => {
                    if let Some((f, p0, len)) = prev {
                        vm.gen8_ppgtt_insert(start + f as u64 * I915_GTT_PAGE_SIZE, p0,
                                             len as u64 * I915_GTT_PAGE_SIZE, TGL_PAT_CACHE_LLC, 0)?;
                    }
                    Some((at, phys, 1))
                }
            };
            at += 1;
        }
    }
    if let Some((f, p0, len)) = run {
        vm.gen8_ppgtt_insert(start + f as u64 * I915_GTT_PAGE_SIZE, p0, len as u64 * I915_GTT_PAGE_SIZE,
                             TGL_PAT_CACHE_LLC, 0)?;
    }
    Ok(())
}

/// `OP_BO_ADD`: the buffer bound into the PPGTT and its address reported.
fn bo_add(gt: &mut intel_gt, svc: &mut service, id: u32) -> Result<(), i32> {
    if (id as usize) < NUM_BOS || svc.dyn_bos.iter().flatten().any(|b| b.id == id) {
        return Err(-EINVAL);
    }
    let slot = svc.dyn_bos.iter().position(|b| b.is_none()).ok_or(-ENOSPC)?;
    let mut b = bo_info(id).ok_or(-EINVAL)?;
    if b.pages == 0 || b.offset as u64 >= I915_GTT_PAGE_SIZE
        || b.size + b.offset as u64 > b.pages as u64 * I915_GTT_PAGE_SIZE {
        return Err(-EINVAL);
    }
    let vm = gt.vm.as_mut().ok_or(-EINVAL)?;
    let size = b.pages as u64 * I915_GTT_PAGE_SIZE;
    let node = vm.mm.drm_mm_insert_node_in_range_best(size, I915_GTT_PAGE_SIZE, 0, DYN_START, DYN_END)?;
    if let Err(x) = bind_pages(vm, id, node.start, b.pages) {
        // A complete tree can be cleared; a partial one keeps its tables.
        let _ = vm.gen8_ppgtt_clear(node.start, size);
        vm.mm.drm_mm_remove_node(&node);
        return Err(x);
    }
    b.node = node;
    b.va = node.start + b.offset as u64;
    b.bound = true;
    svc.dyn_bos[slot] = Some(b);
    npk_sys::gpudev_bo_bound(id as i32, b.va as i64);
    Ok(())
}

/// `OP_BO_REMOVE`: unbound (`gen8_ppgtt_clear`) and forgotten. A buffer
/// whose `OP_BO_ADD` failed is not known and needs nothing.
fn bo_remove(gt: &mut intel_gt, svc: &mut service, id: u32) -> Result<(), i32> {
    if (id as usize) < NUM_BOS {
        return Err(-EINVAL);
    }
    let Some(slot) = svc.dyn_bos.iter().position(|b| b.is_some_and(|b| b.id == id)) else { return Ok(()) };
    let b = svc.dyn_bos[slot].take().unwrap();
    let vm = gt.vm.as_mut().ok_or(-EINVAL)?;
    let r = vm.gen8_ppgtt_clear(b.node.start, b.node.size);
    vm.mm.drm_mm_remove_node(&b.node);
    r
}

/// One copy job.
fn run_copy(i915: &mut drm_i915_private, gt: &mut intel_gt, svc: &mut service, a: [u32; 6]) -> i32 {
    if !svc.copy_ok {
        return -EINVAL;
    }
    let (Some(src), Some(dst)) = (svc.bo(a[0]), svc.bo(a[1])) else { return -EINVAL };
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
            .dec(a[0] as u64).s(" -> bo ").dec(a[1] as u64).s(" in ").dec(us).s(" us").say();
        log_wakerefs(gt, "after the first job");
    } else if svc.jobs % STATS_EVERY == 0 {
        Line::new().s("gpudev: ").dec(svc.jobs as u64).s(" jobs, average ")
            .dec(svc.sum_us / STATS_EVERY as u64).s(" us, max ").dec(svc.max_us).s(" us, GPU requested ")
            .dec(crate::intel_rps::intel_rps_requested_mhz(gt) as u64).s(" MHz").say();
        svc.sum_us = 0;
        svc.max_us = 0;
    }
    match r {
        Ok(()) => 0,
        Err(x) => x,
    }
}

/// One scene job.
fn run_scene(i915: &mut drm_i915_private, gt: &mut intel_gt, svc: &mut service, seq: u32, a: [u32; 6]) -> i32 {
    let (Some(gp), Some(c)) = (svc.gpgpu.as_ref(), svc.composer.as_ref()) else { return -EINVAL };
    let t0 = linux::now_us();
    let r = crate::compose::run_scene(i915, gt, gp, c, &*svc, seq, a[0], a[1]);
    let us = linux::now_us() - t0;
    svc.scenes += 1;
    svc.scene_sum_us += us;
    svc.scene_max_us = svc.scene_max_us.max(us);
    match r {
        Ok(items) if svc.scenes == 1 => {
            Line::new().s("gpudev: first scene, ").dec(items as u64).s(" items into bo ").dec(a[0] as u64)
                .s(" in ").dec(us).s(" us").say();
        }
        Err(x) => {
            Line::new().s("gpudev: scene into bo ").dec(a[0] as u64).s(" failed (").dec((-x) as u64).s(")").say();
        }
        _ => {}
    }
    if svc.scenes % STATS_EVERY == 0 {
        Line::new().s("gpudev: ").dec(svc.scenes as u64).s(" scenes, average ")
            .dec(svc.scene_sum_us / STATS_EVERY as u64).s(" us, max ").dec(svc.scene_max_us).s(" us").say();
        svc.scene_sum_us = 0;
        svc.scene_max_us = 0;
    }
    match r {
        Ok(_) => 0,
        Err(x) => x,
    }
}

/// One job; the status for `gpudev_complete`.
fn run_job(i915: &mut drm_i915_private, gt: &mut intel_gt, svc: &mut service, job: &[u8]) -> i32 {
    let w = |i: usize| u32::from_le_bytes(job[i * 4..i * 4 + 4].try_into().unwrap());
    let (seq, op, a) = (w(0), w(1), [w(2), w(3), w(4), w(5), w(6), w(7)]);
    let st = |r: Result<(), i32>| match r {
        Ok(()) => 0,
        Err(x) => x,
    };
    match op {
        OP_COPY_RECT => run_copy(i915, gt, svc, a),
        OP_BO_ADD => {
            let r = bo_add(gt, svc, a[0]);
            if let Err(x) = r {
                Line::new().s("gpudev: buffer ").dec(a[0] as u64).s(" could not be bound (").dec((-x) as u64)
                    .s(")").say();
            }
            st(r)
        }
        OP_BO_REMOVE => st(bo_remove(gt, svc, a[0])),
        OP_SCENE => run_scene(i915, gt, svc, seq, a),
        _ => -EINVAL,
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
    let rc = npk_sys::gpudev_register(VERSION);
    if rc != 0 {
        Line::new().s("gpudev: register failed (").dec((-rc) as u64).s(")").say();
        return;
    }

    let mut svc = service { bos: [bo::default(); NUM_BOS], dyn_bos: [None; MAX_DYN], e, copy_ok: false, jobs: 0,
                            sum_us: 0, max_us: 0, scenes: 0, scene_sum_us: 0, scene_max_us: 0, logged_idle: false,
                            gpgpu: None, composer: None };
    for id in 0..NUM_BOS {
        let Some(b) = bo_info(id as u32) else {
            Line::new().s("gpudev: no buffer ").dec(id as u64).say();
            npk_sys::gpudev_ready(0);
            return;
        };
        svc.bos[id] = b;
        Line::new().s("gpudev: bo ").dec(id as u64).s(": phys ").hex((b.phys >> 32) as u32).s("_")
            .hex(b.phys as u32).s(", size ").hex(b.size as u32).s(", ").dec(b.width as u64).s("x")
            .dec(b.height as u64).s(", pitch ").dec(b.pitch as u64).say();
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
            let off = svc.bos[0].offset as u64;
            if let Ok(va) = ppgtt_bind(gt, phys, svc.bos[0].size + off, v.pat_index) {
                svc.bos[0].va = va + off;
                svc.bos[0].bound = true;
            }
        }
        _ => linux::say("gpudev: no taken-over framebuffer matches buffer 0"),
    }
    for id in 1..NUM_BOS {
        let b = svc.bos[id];
        if b.phys == 0 || b.phys & (I915_GTT_PAGE_SIZE - 1) != 0 {
            continue;
        }
        if let Ok(va) = ppgtt_bind(gt, b.phys, b.size + b.offset as u64, TGL_PAT_CACHE_LLC) {
            svc.bos[id].va = va + b.offset as u64;
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

    // The GPGPU path (gpgpu.rs) proves itself before scenes are offered; a
    // failure is reported and the copies go on without it.
    svc.gpgpu = crate::gpgpu::gpgpu_init(gt);
    let mut scene_ok = false;
    match &svc.gpgpu {
        Some(gp) => {
            let ok = crate::gpgpu::gpgpu_self_test(i915, gt, gp);
            Line::new().s("gpgpu: rcs0 self-test ").s(if ok { "passed" } else { "FAILED" }).say();
            if ok {
                svc.composer = crate::compose::composer_init(gt);
                scene_ok = svc.composer.is_some();
                if !scene_ok {
                    linux::say("gpgpu: no memory for the scene buffers, scenes stay on the CPU");
                }
            }
        }
        None => linux::say("gpgpu: no render engine or heap, not tested"),
    }

    svc.copy_ok = self_test(i915, gt, e);
    Line::new().s("gpudev: bcs0 self-test ").s(if svc.copy_ok { "passed" } else { "FAILED" }).say();
    let caps = if svc.copy_ok { CAP_COPY } else { 0 } | if scene_ok { CAP_SCENE } else { 0 };
    npk_sys::gpudev_ready(caps);
    if caps == 0 {
        return;
    }
    Line::new().s("gpudev: serving").s(if svc.copy_ok { " copies" } else { "" })
        .s(if scene_ok { " scenes" } else { "" }).say();
    log_wakerefs(gt, "at ready");

    let mut jobs = [0u8; JOB_BYTES * 8];
    loop {
        // While the GT is awake a missed interrupt must not keep it so, and
        // the RPS timer must run on time.
        let timeout = if crate::intel_gt_pm::intel_gt_pm_is_awake(gt) {
            match crate::intel_rps::intel_rps_timer_due(gt) {
                Some(t) => (t.saturating_sub(linux::now_us()).div_ceil(1000)).min(100) as i32,
                None => 100,
            }
        } else {
            -1
        };
        npk_sys::wait(WAIT_IRQ | WAIT_GPU_CMD, timeout);
        service_interrupts(i915, gt);
        crate::intel_rps::intel_rps_run_pending(i915, gt);

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
