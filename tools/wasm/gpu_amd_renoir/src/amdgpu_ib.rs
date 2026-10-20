//! drm/drm_suballoc.c, amdgpu/amdgpu_sa.c and amdgpu_ib.c (Linux 6.18.55):
//! the indirect buffers a ring runs, carved out of the IB pools, and the
//! IB test of every ring.
//!
//! The suballocator's two lists are kept as vectors: `olist` in offset
//! order, `hole` as the index of the entry the hole follows (`None` for
//! the list head), each `flist` as a queue of entries waiting on their
//! fence.

use crate::amdgpu_device::amdgpu_device;
use crate::dma_fence::{dma_fence_is_signaled, Fence};
use crate::linux::Line;
use crate::regs::*;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// `struct drm_suballoc`.
#[derive(Clone)]
struct drm_suballoc {
    soffset: u32,
    eoffset: u32,
    fence: Option<Fence>,
}

/// `struct drm_suballoc_manager`.
#[derive(Default)]
pub struct drm_suballoc_manager {
    size: u32,
    align: u32,
    /// Entries in offset order, by slot in `sa`.
    olist: Vec<usize>,
    hole: Option<usize>,
    flist: [VecDeque<usize>; DRM_SUBALLOC_MAX_QUEUES as usize],
    sa: Vec<Option<drm_suballoc>>,
}

/// A suballocation: its pool and its slot there.
#[derive(Clone, Copy)]
pub struct sa_bo {
    pub pool: usize,
    slot: usize,
}

impl drm_suballoc_manager {
    /// `drm_suballoc_manager_init`.
    pub fn new(size: u32, align: u32) -> Self {
        let mut align = if align == 0 { 1 } else { align };
        // alignment must be a power of 2
        if !align.is_power_of_two() {
            align = align.next_power_of_two();
        }
        drm_suballoc_manager { size, align, ..Default::default() }
    }

    fn get(&self, slot: usize) -> &drm_suballoc {
        self.sa[slot].as_ref().unwrap()
    }

    /// The position of the entry after the hole, if any.
    fn hole_next(&self) -> Option<usize> {
        let n = self.hole.map_or(0, |h| h + 1);
        (n < self.olist.len()).then_some(n)
    }

    /// `drm_suballoc_remove_locked`.
    fn remove(&mut self, slot: usize) {
        if let Some(pos) = self.olist.iter().position(|&s| s == slot) {
            if self.hole == Some(pos) {
                self.hole = pos.checked_sub(1);
            } else if self.hole.is_some_and(|h| h > pos) {
                self.hole = self.hole.map(|h| h - 1);
            }
            self.olist.remove(pos);
        }
        for q in self.flist.iter_mut() {
            q.retain(|&s| s != slot);
        }
        self.sa[slot] = None;
    }

    /// `drm_suballoc_try_free`.
    fn try_free(&mut self, adev: &mut amdgpu_device) {
        let Some(pos) = self.hole_next() else { return };
        while pos < self.olist.len() {
            let slot = self.olist[pos];
            match self.get(slot).fence.clone() {
                Some(f) if dma_fence_is_signaled(adev, &f) => self.remove(slot),
                _ => return,
            }
            // the removal moved the next entry to `pos`
        }
    }

    /// `drm_suballoc_hole_soffset`.
    fn hole_soffset(&self) -> u32 {
        self.hole.map_or(0, |h| self.get(self.olist[h]).eoffset)
    }

    /// `drm_suballoc_hole_eoffset`.
    fn hole_eoffset(&self) -> u32 {
        self.hole_next().map_or(self.size, |n| self.get(self.olist[n]).soffset)
    }

    /// `drm_suballoc_try_alloc`.
    fn try_alloc(&mut self, size: u32, align: u32) -> Option<usize> {
        let mut soffset = self.hole_soffset();
        let eoffset = self.hole_eoffset();
        let wasted = soffset.next_multiple_of(align) - soffset;
        if eoffset - soffset >= size + wasted {
            soffset += wasted;
            let e = drm_suballoc { soffset, eoffset: soffset + size, fence: None };
            let slot = match self.sa.iter().position(|s| s.is_none()) {
                Some(i) => {
                    self.sa[i] = Some(e);
                    i
                }
                None => {
                    self.sa.push(Some(e));
                    self.sa.len() - 1
                }
            };
            let at = self.hole.map_or(0, |h| h + 1);
            self.olist.insert(at, slot);
            self.hole = Some(at);
            return Some(slot);
        }
        None
    }

    /// `drm_suballoc_next_hole`.
    fn next_hole(&mut self, adev: &mut amdgpu_device, fences: &mut [Option<Fence>], tries: &mut [u32]) -> bool {
        // if hole points to the end of the buffer
        if self.hole_next().is_none() {
            // try again with its beginning
            self.hole = None;
            return true;
        }
        let soffset = self.hole_soffset();
        // to handle wrap around we add sa_manager->size
        let mut best = self.size * 2;
        let mut best_bo = None;
        // go over all fence list and try to find the closest sa of the
        // current last
        for i in 0..DRM_SUBALLOC_MAX_QUEUES as usize {
            fences[i] = None;
            let Some(&slot) = self.flist[i].front() else { continue };
            let sa = self.get(slot).clone();
            let f = sa.fence.unwrap();
            if !dma_fence_is_signaled(adev, &f) {
                fences[i] = Some(f);
                continue;
            }
            // limit the number of tries each freelist gets
            if tries[i] > 2 {
                continue;
            }
            let mut tmp = sa.soffset;
            if tmp < soffset {
                // wrap around, pretend it's after
                tmp += self.size;
            }
            tmp -= soffset;
            if tmp < best {
                // this sa bo is the closest one
                best = tmp;
                best_bo = Some((i, slot));
            }
        }
        if let Some((i, slot)) = best_bo {
            tries[i] += 1;
            let pos = self.olist.iter().position(|&s| s == slot).unwrap();
            self.hole = pos.checked_sub(1);
            // We know that this one is signaled, so it's safe to remove it.
            self.remove(slot);
            return true;
        }
        false
    }
}

/// `drm_suballoc_new` (`amdgpu_sa_bo_new`): not interruptible, the
/// manager's alignment.
fn drm_suballoc_new(adev: &mut amdgpu_device, pool: usize, size: u32) -> Result<usize, i32> {
    let m = &adev.ib_sa[pool];
    if size > m.size || size == 0 {
        Line::new().s("WARNING: drm_suballoc_new of ").dec(size as u64).s(" bytes").say();
        return Err(-22);
    }
    let align = m.align;
    loop {
        let mut tries = [0u32; DRM_SUBALLOC_MAX_QUEUES as usize];
        let mut fences: [Option<Fence>; DRM_SUBALLOC_MAX_QUEUES as usize] = Default::default();
        loop {
            let mut m = core::mem::take(&mut adev.ib_sa[pool]);
            m.try_free(adev);
            let got = m.try_alloc(size, align);
            let more = got.is_none() && m.next_hole(adev, &mut fences, &mut tries);
            adev.ib_sa[pool] = m;
            if let Some(slot) = got {
                return Ok(slot);
            }
            // see if we can skip over some allocations
            if !more {
                break;
            }
        }
        let waiting: Vec<Fence> = fences.iter().flatten().cloned().collect();
        if waiting.is_empty() {
            // wait_event for a free by another thread: there is none.
            Line::new().s("drm_suballoc_new: nothing to wait for and no room for ").dec(size as u64).s(" bytes")
                .say();
            return Err(-12);
        }
        // dma_fence_wait_any_timeout, MAX_SCHEDULE_TIMEOUT
        crate::workqueue::wait_until(adev, i64::MAX, |adev| waiting.iter().any(|f| dma_fence_is_signaled(adev, f)));
    }
}

/// `drm_suballoc_free` (`amdgpu_sa_bo_free`).
fn drm_suballoc_free(adev: &mut amdgpu_device, bo: sa_bo, fence: Option<Fence>) {
    let signaled = fence.as_ref().is_none_or(|f| dma_fence_is_signaled(adev, f));
    let m = &mut adev.ib_sa[bo.pool];
    match fence {
        Some(f) if !signaled => {
            let idx = (f.context % DRM_SUBALLOC_MAX_QUEUES as u64) as usize;
            if let Some(e) = m.sa[bo.slot].as_mut() {
                e.fence = Some(f);
            }
            m.flist[idx].push_back(bo.slot);
        }
        _ => m.remove(bo.slot),
    }
}

/// `struct amdgpu_ib`.
#[derive(Clone)]
pub struct amdgpu_ib {
    pub sa_bo: Option<sa_bo>,
    pub length_dw: u32,
    pub gpu_addr: u64,
    /// `ptr`: the pool's mapping and the IB's offset in it.
    pub ptr: Option<(crate::amdgpu_object::amdgpu_kptr, u64)>,
    pub flags: u32,
}

impl amdgpu_ib {
    pub fn new() -> Self {
        amdgpu_ib { sa_bo: None, length_dw: 0, gpu_addr: 0, ptr: None, flags: 0 }
    }

    /// `ib.ptr[i] = v`.
    pub fn set(&self, adev: &amdgpu_device, i: u32, v: u32) {
        if let Some((k, off)) = self.ptr {
            crate::amdgpu_gart::kptr_w32(adev, k, off + i as u64 * 4, v);
        }
    }
}

/// `amdgpu_ib_get` without a VM.
pub fn amdgpu_ib_get(adev: &mut amdgpu_device, size: u32, pool_type: u32, ib: &mut amdgpu_ib) -> i32 {
    if size != 0 {
        let pool = pool_type as usize;
        let slot = match drm_suballoc_new(adev, pool, size) {
            Ok(s) => s,
            Err(r) => {
                Line::new().s("failed to get a new IB (").dec((-r) as u64).s(")").say();
                return r;
            }
        };
        let soffset = adev.ib_sa[pool].get(slot).soffset as u64;
        let m = &adev.ib_pools[pool];
        ib.sa_bo = Some(sa_bo { pool, slot });
        // amdgpu_sa_bo_cpu_addr
        ib.ptr = m.cpu_ptr.map(|k| (k, soffset));
        // flush the cache before commit the IB
        ib.flags = AMDGPU_IB_FLAG_EMIT_MEM_SYNC;
        // amdgpu_sa_bo_gpu_addr
        ib.gpu_addr = m.gpu_addr + soffset;
    }
    0
}

/// `amdgpu_ib_free`.
pub fn amdgpu_ib_free(adev: &mut amdgpu_device, ib: &mut amdgpu_ib, f: Option<Fence>) {
    if let Some(bo) = ib.sa_bo.take() {
        drm_suballoc_free(adev, bo, f);
    }
}

/// `amdgpu_ib_schedule`: the IBs, after the VM flush of `job`, then a
/// fence. HDP flush and invalidate do nothing on an APU, and no ring of
/// Renoir's has `insert_start`, `insert_end`, a software ring or GFX
/// shadowing. The GFX ring takes no job here.
pub fn amdgpu_ib_schedule(adev: &mut amdgpu_device, r: usize, ibs: &[amdgpu_ib], job: Option<u64>)
                          -> Result<Fence, i32> {
    use crate::amdgpu_ring as ring;
    if ibs.is_empty() {
        return Err(-22);
    }
    let mut ib = &ibs[0];
    let vmid = crate::amdgpu_job::amdgpu_job_get_vmid(adev, job);
    let (vm, fence_ctx, af_context) = match job {
        Some(j) => {
            let jb = adev.jobs.get(&j).unwrap();
            let s = jb.base.s_fence.as_ref().map(|f| crate::drm_sched::to_drm_sched_fence(f).unwrap());
            (jb.vm, s.as_ref().map_or(0, |s| s.scheduled.context), s.as_ref().map_or(0, |s| s.scheduled.context + 1))
        }
        None => (None, 0, 0),
    };
    let name = adev.rings[r].name.clone();
    if !adev.rings[r].sched.ready {
        Line::new().s("couldn't schedule ib on ring <").s(&name).s(">").say();
        return Err(-22);
    }
    if vm.is_some() && vmid == 0 {
        Line::new().s("VM IB without ID").say();
        return Err(-22);
    }
    if ib.flags & AMDGPU_IB_FLAGS_SECURE != 0 && !adev.rings[r].funcs.secure() {
        Line::new().s("secure submissions not supported on ring <").s(&name).s(">").say();
        return Err(-22);
    }
    let f = adev.rings[r].funcs;
    let alloc_size = f.emit_frame_size + ibs.len() as u32 * f.emit_ib_size;
    let ret = ring::amdgpu_ring_alloc(adev, r, alloc_size);
    if ret != 0 {
        Line::new().s("scheduling IB failed (").dec((-ret) as u64).s(").").say();
        return Err(ret);
    }
    let need_ctx_switch = adev.rings[r].current_ctx != fence_ctx;
    let mut need_pipe_sync = false;
    if let Some(j) = job {
        if ring::has_emit_vm_flush(adev, r) {
            let mut sync = core::mem::take(&mut adev.jobs.get_mut(&j).unwrap().explicit_sync);
            let tmp = crate::amdgpu_sync::amdgpu_sync_get_fence(adev, &mut sync);
            adev.jobs.get_mut(&j).unwrap().explicit_sync = sync;
            if tmp.is_some() || need_ctx_switch || crate::amdgpu_vm::amdgpu_vm_need_pipeline_sync(adev, r, j) {
                need_pipe_sync = true;
            }
        }
        let ret = crate::amdgpu_vm::amdgpu_vm_flush(adev, r, j, need_pipe_sync);
        if ret != 0 {
            ring::amdgpu_ring_undo(adev, r);
            return Err(ret);
        }
    }

    if ib.flags & AMDGPU_IB_FLAG_EMIT_MEM_SYNC != 0 {
        ring::amdgpu_ring_emit_mem_sync(adev, r);
    }
    let high = adev.rings[r].hw_prio == AMDGPU_GFX_PIPE_PRIO_HIGH;
    if high {
        ring::amdgpu_ring_emit_wave_limit(adev, r, true);
    }
    let cond_exec = ring::amdgpu_ring_init_cond_exec(adev, r);

    // amdgpu_device_flush_hdp: nothing on an APU.
    let mut status = 0;
    if need_ctx_switch {
        status |= AMDGPU_HAVE_CTX_SWITCH;
    }
    // emit_cntxcntl and emit_frame_cntl are the GFX ring's.
    let gfx = adev.rings[r].funcs.ops == ring::ring_ops::gfx_v9_0_gfx;
    let (preamble, preemption, gds) = match job {
        Some(j) => {
            let jb = adev.jobs.get(&j).unwrap();
            (jb.preamble_status, jb.preemption_status, jb.gds_size > 0 && jb.gds_base != 0)
        }
        None => (0, 0, false),
    };
    if job.is_some() && gfx {
        status |= preamble;
        status |= preemption;
        crate::gfx_v9_0::gfx_v9_ring_emit_cntxcntl(adev, r, status);
    }
    // Setup initial TMZiness and send it off.
    let mut secure = false;
    if job.is_some() && gfx {
        secure = ib.flags & AMDGPU_IB_FLAGS_SECURE != 0;
        crate::gfx_v9_0::gfx_v9_0_ring_emit_frame_cntl(adev, r, true, secure);
    }
    for i in ibs {
        ib = i;
        if job.is_some() && gfx && secure != (ib.flags & AMDGPU_IB_FLAGS_SECURE != 0) {
            crate::gfx_v9_0::gfx_v9_0_ring_emit_frame_cntl(adev, r, false, secure);
            secure = !secure;
            crate::gfx_v9_0::gfx_v9_0_ring_emit_frame_cntl(adev, r, true, secure);
        }
        ring::amdgpu_ring_emit_ib(adev, r, vmid, gds, ib, status);
        status &= !AMDGPU_HAVE_CTX_SWITCH;
    }
    if job.is_some() && gfx {
        crate::gfx_v9_0::gfx_v9_0_ring_emit_frame_cntl(adev, r, false, secure);
    }
    // amdgpu_device_invalidate_hdp: nothing on an APU.
    let mut fence_flags = 0;
    if ib.flags & AMDGPU_IB_FLAG_TC_WB_NOT_INVALIDATE != 0 {
        fence_flags |= AMDGPU_FENCE_FLAG_TC_WB_ONLY;
    }
    if let Some(j) = job {
        let (uf_addr, uf_sequence) = {
            let jb = adev.jobs.get(&j).unwrap();
            (jb.uf_addr, jb.uf_sequence)
        };
        if uf_addr != 0 {
            ring::amdgpu_ring_emit_fence(adev, r, uf_addr, uf_sequence, fence_flags | AMDGPU_FENCE_FLAG_64BIT);
        }
    }
    let fence = match crate::amdgpu_fence::amdgpu_fence_emit(adev, r, job, af_context, fence_flags) {
        Ok(f) => f,
        Err(e) => {
            Line::new().s("failed to emit fence (").dec((-e) as u64).s(")").say();
            if job.is_some() && vmid != 0 {
                let hub = adev.rings[r].vm_hub as usize;
                crate::amdgpu_ids::amdgpu_vmid_reset(adev, hub, vmid as usize);
            }
            ring::amdgpu_ring_undo(adev, r);
            return Err(e);
        }
    };
    ring::amdgpu_ring_patch_cond_exec(adev, r, cond_exec);
    adev.rings[r].current_ctx = fence_ctx;
    if high {
        ring::amdgpu_ring_emit_wave_limit(adev, r, false);
    }
    crate::amdgpu_fence::amdgpu_fence_save_wptr(adev, &fence);
    ring::amdgpu_ring_commit(adev, r);
    Ok(fence)
}

/// `AMDGPU_IB_TEST_TIMEOUT`: `msecs_to_jiffies(1000)`, in microseconds.
const AMDGPU_IB_TEST_TIMEOUT_US: i64 = 1_000_000;

/// `amdgpu_ib_ring_tests`: bare metal, not in an XGMI hive.
pub fn amdgpu_ib_ring_tests(adev: &mut amdgpu_device) -> i32 {
    let tmo = AMDGPU_IB_TEST_TIMEOUT_US;
    let mut ret = 0;
    for r in 0..adev.rings.len() {
        // KIQ rings don't have an IB test because we never submit IBs to
        // them and they have no interrupt support.
        if !adev.rings[r].sched.ready || !crate::amdgpu_ring::has_test_ib(adev, r) {
            continue;
        }
        let res = crate::amdgpu_ring::amdgpu_ring_test_ib(adev, r, tmo);
        let name = adev.rings[r].name.clone();
        if res == 0 {
            Line::new().s("ib test on ").s(&name).s(" succeeded").dbg();
            continue;
        }
        adev.rings[r].sched.ready = false;
        Line::new().s("IB test failed on ").s(&name).s(" (").dec((-res) as u64).s(").").say();
        if adev.gfx.gfx_ring.first() == Some(&r) {
            // oh, oh, that's really bad
            adev.accel_working = false;
            return res;
        }
        ret = res;
    }
    ret
}
