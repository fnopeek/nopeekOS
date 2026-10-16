//! The drm_file side of i915 for native-context clients
//! (docs/plan/GPU_IN_VM.md §4b): gem/i915_gem_context.c (contexts, their
//! engines and address spaces, `I915_GEM_VM_*`), the object bookkeeping of
//! gem/i915_gem_object.c and i915_vma.c (an object bound at the offset a
//! client chose), and the fences virglrenderer's drm_timeline keeps per
//! ring (src/drm/drm_fence.c).
//!
//! A client is one guest virtio-gpu context, as a drm_file is one open of
//! the device. Its objects are kernel buffers (`OP_OBJECT`); their pages
//! come from `npk_gpudev_bo_pages`. The module has no heap that frees, so
//! address spaces and hardware contexts sit in fixed tables and are kept
//! for the next client when one goes: an address space is emptied (its
//! page tables go to its stash), a context waits idle in a pool.

use crate::gen8_ppgtt::i915_ppgtt;
use crate::i915_drv::drm_i915_private;
use crate::i915_uapi::*;
use crate::intel_ggtt::i915_ggtt;
use crate::intel_gt::intel_gt;
use crate::linux::{self, Line};
use crate::regs::{COPY_ENGINE_CLASS, I915_GTT_PAGE_SIZE, I915_NUM_ENGINES, RENDER_CLASS, TGL_PAT_CACHE_LLC, VIDEO_DECODE_CLASS, VIDEO_ENHANCEMENT_CLASS};
use npk_sys::cell::Single;

pub const MAX_CLIENTS: usize = 8;
pub const MAX_VMS: usize = 8;
pub const MAX_GEM_CTX: usize = 64;
pub const MAX_OBJECTS: usize = 8192;
/// Fence rings per client (virglrenderer's 64 timelines).
pub const MAX_RINGS: usize = 64;
/// Slots of `I915_CONTEXT_PARAM_ENGINES` (I915_EXEC_RING_MASK + 1).
pub const MAX_USER_ENGINES: usize = 64;
/// In-flight requests of all clients.
pub const MAX_PENDING: usize = crate::i915_request::MAX_REQUESTS;
const MAX_VM_IDS: usize = 16;
const HASH: usize = 4096;
const ENGINES: usize = I915_NUM_ENGINES as usize;
pub const NONE16: u16 = u16::MAX;
pub const NONE8: u8 = u8::MAX;
/// Requests of a client context are given this long once running; then the
/// GT is wedged, since an engine reset is not ported.
const HANG_FALLBACK_MS: u64 = 6000;

/// The counters of one fence ring (`npk_gpudev_ring`).
#[derive(Clone, Copy)]
pub struct fence_ring {
    pub submitted: u64,
    pub completed: u64,
}

/// `struct drm_i915_file_private`.
#[derive(Clone, Copy)]
pub struct client {
    pub used: bool,
    /// The kernel's handle.
    pub id: u32,
    /// The address space of context 0.
    pub default_vm: u8,
    /// `file_priv->vm_xa`: (id, vm slot).
    pub vm_ids: [(u32, u8); MAX_VM_IDS],
    pub rings: [fence_ring; MAX_RINGS],
    /// Closed by the kernel; torn down once its requests are done.
    pub closing: bool,
    /// When it was closed, and whether what holds it was reported.
    pub closing_us: u64,
    pub closing_told: bool,
}

impl client {
    const EMPTY: client = client { used: false, id: 0, default_vm: NONE8, vm_ids: [(0, NONE8); MAX_VM_IDS],
                                   rings: [fence_ring { submitted: 0, completed: 0 }; MAX_RINGS],
                                   closing: false, closing_us: 0, closing_told: false };
}

/// `struct i915_gem_context`.
#[derive(Clone, Copy)]
pub struct gem_ctx {
    pub used: bool,
    /// Destroyed by the client; recycled once its requests are done.
    pub closed: bool,
    pub client: u8,
    pub id: u32,
    pub vm: u8,
    pub priority: i32,
    pub recoverable: bool,
    pub bannable: bool,
    pub persistent: bool,
    pub no_error_capture: bool,
    pub banned: bool,
    /// `ctx->guilty_count`, `ctx->active_count`.
    pub guilty: u32,
    pub innocent: u32,
    /// `I915_CONTEXT_PARAM_ENGINES`: slot → engine id, or the legacy map.
    pub user_engines: bool,
    pub n_engines: u8,
    pub engines: [u8; MAX_USER_ENGINES],
    /// The hardware context per engine slot (`ctx->engines`), in
    /// `gt.contexts`: a slot of the user engine map, or the engine id
    /// for the legacy map. Two slots naming one engine have two contexts.
    pub ce: [u16; MAX_USER_ENGINES],
}

impl gem_ctx {
    const EMPTY: gem_ctx = gem_ctx { used: false, closed: false, client: 0, id: 0, vm: NONE8, priority: 0, recoverable: true,
                                     bannable: true, persistent: true, no_error_capture: false, banned: false, guilty: 0,
                                     innocent: 0, user_engines: false, n_engines: 0, engines: [NONE8; MAX_USER_ENGINES],
                                     ce: [NONE16; MAX_USER_ENGINES] };
}

/// `struct i915_address_space`: a client's PPGTT. `ppgtt` stays when the
/// slot is freed and is reused empty.
pub struct vm_slot {
    pub used: bool,
    pub client: u8,
    /// Context 0, the id table and every context using it.
    pub refs: u32,
    pub ppgtt: Option<i915_ppgtt>,
}

/// One object, with the requests that last used it per engine class.
#[derive(Clone, Copy)]
pub struct gem_obj {
    pub used: bool,
    pub client: u8,
    pub res_id: u32,
    pub bo: u32,
    pub size: u64,
    pub pages: u32,
    pub tiling: u32,
    pub stride: u32,
    /// Bindings, at most one per address space (a VMA per object and VM):
    /// vm slot (NONE8 when unbound) and start.
    pub vm: [u8; MAX_VMS],
    pub start: [u64; MAX_VMS],
    /// `obj->base.resv`: the fences of this client's requests on it.
    pub fences: [resv_fence; RESV_FENCES],
    /// The next object on the same kernel buffer, a ring (itself when the
    /// buffer is not shared). An import is the same GEM object in Linux, so
    /// the fences of the whole ring are its reservation.
    pub share: u16,
    pub next: u16,
    /// Detached while busy: no longer named by its client, unbound and
    /// reported released (`npk_gpudev_released`) once idle.
    pub zombie: bool,
}

impl gem_obj {
    const EMPTY: gem_obj = gem_obj { used: false, client: 0, res_id: 0, bo: 0, size: 0, pages: 0, tiling: 0, stride: 0,
                                     vm: [NONE8; MAX_VMS], start: [0; MAX_VMS], fences: [resv_fence::EMPTY; RESV_FENCES],
                                     share: NONE16, next: NONE16, zombie: false };
}

/// Fences one object keeps: one per timeline and usage.
pub const RESV_FENCES: usize = 8;

/// A fence in a reservation (`dma_resv_add_fence` with
/// DMA_RESV_USAGE_WRITE or _READ): the request's context and seqno and the
/// uabi class of its engine.
#[derive(Clone, Copy)]
pub struct resv_fence {
    pub ce: u16,
    pub seqno: u32,
    pub class: u8,
    pub write: bool,
}

impl resv_fence {
    const EMPTY: resv_fence = resv_fence { ce: NONE16, seqno: 0, class: 0, write: false };
}

/// Fences a new request has to wait for, one per timeline.
#[derive(Clone, Copy)]
pub struct await_list {
    pub n: usize,
    pub f: [(u16, u32); crate::i915_request::MAX_DEPS],
}

impl await_list {
    pub const EMPTY: await_list = await_list { n: 0, f: [(NONE16, 0); crate::i915_request::MAX_DEPS] };

    /// One per timeline, the later seqno; false when full.
    fn add(&mut self, ce: u16, seqno: u32) -> bool {
        if let Some(d) = self.f[..self.n].iter_mut().find(|d| d.0 == ce) {
            if crate::i915_request::i915_seqno_passed(seqno, d.1) {
                d.1 = seqno;
            }
            return true;
        }
        if self.n == self.f.len() {
            return false;
        }
        self.f[self.n] = (ce, seqno);
        self.n += 1;
        true
    }
}

/// A client request whose fence is not signalled yet.
#[derive(Clone, Copy)]
pub struct pending {
    pub used: bool,
    pub client: u8,
    pub ring: u8,
    pub ticket: u64,
    pub rq: u16,
    pub ctx: u16,
    pub engine: u8,
    /// When it was first seen running.
    pub started_us: u64,
}

impl pending {
    const EMPTY: pending = pending { used: false, client: 0, ring: 0, ticket: 0, rq: 0, ctx: 0, engine: 0, started_us: 0 };
}

/// An execbuffer waiting for room: where Linux blocks the caller
/// (`eb_throttle`, `i915_gem_evict_for_node`, the request's sw fences), it
/// waits here in ring order. Its ticket is taken, so the guest's fence
/// stays open until it has run, and its objects count as busy meanwhile.
pub struct held_exec {
    pub client: u8,
    pub ring: u8,
    pub ticket: u64,
    pub v2: bool,
    pub req: alloc::vec::Vec<u8>,
    pub objs: alloc::vec::Vec<u16>,
}

pub struct nctx {
    pub clients: [client; MAX_CLIENTS],
    pub ctxs: [gem_ctx; MAX_GEM_CTX],
    pub vms: [vm_slot; MAX_VMS],
    pub objs: [gem_obj; MAX_OBJECTS],
    hash: [u16; HASH],
    pub pend: [pending; MAX_PENDING],
    pub held: alloc::vec::Vec<held_exec>,
    /// Idle client contexts in `gt.contexts`, for any engine.
    pool: [u16; crate::intel_context::MAX_CONTEXTS],
    npool: usize,
    /// Zombie objects, and clients closing.
    nzombies: u32,
    nclosing: u32,
    /// PTEs were cleared since the last TLB invalidation (`tlb_flush`).
    tlb_dirty: bool,
}

pub static NCTX: Single<nctx> = Single::new(nctx {
    clients: [client::EMPTY; MAX_CLIENTS],
    ctxs: [gem_ctx::EMPTY; MAX_GEM_CTX],
    vms: [const { vm_slot { used: false, client: 0, refs: 0, ppgtt: None } }; MAX_VMS],
    objs: [gem_obj::EMPTY; MAX_OBJECTS],
    hash: [NONE16; HASH],
    pend: [pending::EMPTY; MAX_PENDING],
    held: alloc::vec::Vec::new(),
    pool: [0; crate::intel_context::MAX_CONTEXTS],
    npool: 0,
    nzombies: 0,
    nclosing: 0,
    tlb_dirty: false,
});

/// The GPU side every call needs.
pub struct drm<'a> {
    pub i915: &'a mut drm_i915_private,
    pub gt: &'a mut intel_gt,
    pub ggtt: &'a mut i915_ggtt,
}

fn hash_of(client: u8, res_id: u32) -> usize {
    (res_id.wrapping_mul(2654435761) ^ client as u32) as usize % HASH
}

/// The uabi class (`engine->uabi_class`) of a hardware class.
pub fn uabi_class(class: u32) -> u16 {
    match class {
        RENDER_CLASS => I915_ENGINE_CLASS_RENDER,
        COPY_ENGINE_CLASS => I915_ENGINE_CLASS_COPY,
        VIDEO_DECODE_CLASS => I915_ENGINE_CLASS_VIDEO,
        VIDEO_ENHANCEMENT_CLASS => I915_ENGINE_CLASS_VIDEO_ENHANCE,
        _ => I915_ENGINE_CLASS_INVALID_NONE,
    }
}

/// The user-visible engines in `for_each_uabi_engine` order: by uabi class,
/// then by instance. Returns the engine ids and their count.
pub fn uabi_engines(gt: &intel_gt) -> ([u8; ENGINES], usize) {
    let mut ids = [0u8; ENGINES];
    let mut n = 0;
    for class in [I915_ENGINE_CLASS_RENDER, I915_ENGINE_CLASS_COPY, I915_ENGINE_CLASS_VIDEO,
                  I915_ENGINE_CLASS_VIDEO_ENHANCE] {
        let mut found = [(0u32, 0u8); ENGINES];
        let mut m = 0;
        for (id, e) in gt.engine.iter().enumerate() {
            if let Some(e) = e {
                if uabi_class(e.class) == class {
                    found[m] = (e.instance, id as u8);
                    m += 1;
                }
            }
        }
        found[..m].sort_unstable_by_key(|x| x.0);
        for f in &found[..m] {
            ids[n] = f.1;
            n += 1;
        }
    }
    (ids, n)
}

/// `intel_engine_lookup_user`: the engine id of uabi `class`, `instance`.
pub fn intel_engine_lookup_user(gt: &intel_gt, class: u16, instance: u16) -> Option<u8> {
    let (ids, n) = uabi_engines(gt);
    ids[..n].iter().copied().filter(|&id| gt.engine[id as usize].is_some_and(|e| uabi_class(e.class) == class))
        .nth(instance as usize)
}

/// Whether request `seqno` of context `ce` has completed or is gone.
pub fn seqno_done(gt: &intel_gt, ce: u16, seqno: u32) -> bool {
    crate::i915_request::fence_signaled(gt, ce, seqno)
}

/// The hang limit of an engine: Linux sends a heartbeat every interval and
/// resets after a missed one plus the preemption timeout.
fn hang_us(gt: &intel_gt, engine: usize) -> u64 {
    let ms = gt.engine[engine].map(|e| 2 * e.heartbeat_interval_ms + e.preempt_timeout_ms).unwrap_or(0);
    (if ms == 0 { HANG_FALLBACK_MS } else { ms }) * 1000
}

/// A page-table tree that could not be built: the tables the address space
/// holds and those pooled; a fixed number of lines.
fn log_bind_failure(p: &crate::gen8_ppgtt::i915_ppgtt, vm: u8, start: u64, pages: u32, e: i32) {
    use core::sync::atomic::{AtomicU32, Ordering};
    const MAX_LINES: u32 = 8;
    static LINES: AtomicU32 = AtomicU32::new(0);
    if LINES.fetch_add(1, Ordering::Relaxed) < MAX_LINES {
        crate::linux::Line::new().s("native context: binding ").dec(pages as u64).s(" pages at ")
            .hex((start >> 32) as u32).s("_").hex(start as u32).s(" in address space ").dec(vm as u64)
            .s(" failed (").dec((-e) as u64).s("): ").dec(p.tables() as u64).s(" tables in it, ")
            .dec(crate::gen8_ppgtt::pooled_tables() as u64).s(" pooled").say();
    }
}

impl nctx {
    pub fn client_slot(&self, id: u32) -> Option<usize> {
        self.clients.iter().position(|c| c.used && c.id == id)
    }

    /// The object `res_id` of client slot `c`.
    pub fn obj(&self, c: u8, res_id: u32) -> Option<usize> {
        let mut i = self.hash[hash_of(c, res_id)];
        while i != NONE16 {
            let o = &self.objs[i as usize];
            if o.used && o.client == c && o.res_id == res_id {
                return Some(i as usize);
            }
            i = o.next;
        }
        None
    }

    /// `i915_gem_context_lookup`.
    pub fn ctx(&self, c: u8, id: u32) -> Option<usize> {
        self.ctxs.iter().position(|x| x.used && !x.closed && x.client == c && x.id == id)
    }

    /// `i915_gem_vm_lookup`.
    pub fn vm_lookup(&self, c: u8, id: u32) -> Option<u8> {
        self.clients[c as usize].vm_ids.iter().find(|x| x.1 != NONE8 && x.0 == id).map(|x| x.1)
    }

    /// Whether object `i` is still used by a request of its client (its
    /// bindings and pages are that client's).
    pub fn obj_busy(&self, gt: &intel_gt, i: usize) -> bool {
        // Wedged, no request runs again (Linux fails and retires them all).
        !crate::intel_gt::intel_gt_is_wedged(gt) && self.objs[i].fences.iter().any(|f| !seqno_done(gt, f.ce, f.seqno))
    }

    /// Whether an execbuffer waiting in `held` names the buffer of object `i`.
    pub fn obj_held(&self, i: usize) -> bool {
        !self.held.is_empty() && self.share_ring(i).any(|j| self.held.iter().any(|h| h.objs.contains(&(j as u16))))
    }

    /// `obj_busy`, or named by a waiting execbuffer: what the guest and the
    /// object's release see.
    pub fn obj_in_use(&self, gt: &intel_gt, i: usize) -> bool {
        self.obj_busy(gt, i) || self.obj_held(i)
    }

    /// The objects sharing the buffer of object `i`, `i` first.
    fn share_ring(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let mut at = Some(i);
        core::iter::from_fn(move || {
            let cur = at?;
            let next = self.objs[cur].share as usize;
            at = if next == i || self.objs[cur].share == NONE16 { None } else { Some(next) };
            Some(cur)
        })
    }

    /// The tiling of the buffer of object `i`, on every object sharing it.
    pub fn set_tiling_shared(&mut self, i: usize, mode: u32, stride: u32) {
        let mut j = i;
        loop {
            self.objs[j].tiling = mode;
            self.objs[j].stride = stride;
            let next = self.objs[j].share;
            if next == NONE16 || next as usize == i {
                break;
            }
            j = next as usize;
        }
    }

    /// `i915_gem_busy_ioctl`'s answer over the reservation: every reader's
    /// uabi class in the high half, a writer's class + 1 in the low.
    pub fn obj_busy_flags(&self, gt: &intel_gt, i: usize) -> u32 {
        if crate::intel_gt::intel_gt_is_wedged(gt) {
            return 0;
        }
        // A waiting execbuffer may write it on the render engine.
        let mut busy = if self.obj_held(i) { 1 << 16 | 1 } else { 0u32 };
        for j in self.share_ring(i) {
            for f in &self.objs[j].fences {
                if !seqno_done(gt, f.ce, f.seqno) {
                    busy |= 1 << (16 + f.class as u32);
                    if f.write {
                        busy = (busy & !0xffff) | (f.class as u32 + 1);
                    }
                }
            }
        }
        busy
    }

    /// `i915_request_await_object`: the fences a request on context `ce`
    /// waits for when it reads (the writers) or writes (all) object `i`.
    /// False when `list` is full.
    pub fn resv_await(&self, gt: &intel_gt, i: usize, ce: u16, write: bool, list: &mut await_list) -> bool {
        for j in self.share_ring(i) {
            for f in &self.objs[j].fences {
                if f.ce != ce && (write || f.write) && !seqno_done(gt, f.ce, f.seqno) && !list.add(f.ce, f.seqno) {
                    return false;
                }
            }
        }
        true
    }

    /// Whether `resv_add` for context `ce` will find a place in object `i`.
    pub fn resv_room(&self, gt: &intel_gt, i: usize, ce: u16, write: bool) -> bool {
        self.objs[i].fences.iter().any(|f| f.ce == NONE16 || seqno_done(gt, f.ce, f.seqno) || (f.ce == ce && (write || !f.write)))
    }

    /// `dma_resv_add_fence`: a fence of the same timeline replaces an older
    /// one of no stronger usage, a signalled one makes room.
    pub fn resv_add(&mut self, gt: &intel_gt, i: usize, ce: u16, seqno: u32, class: u8, write: bool) {
        let new = resv_fence { ce, seqno, class, write };
        let fences = &mut self.objs[i].fences;
        if let Some(f) = fences.iter_mut().find(|f| f.ce == ce && (write || !f.write)) {
            *f = new;
            return;
        }
        if let Some(f) = fences.iter_mut().find(|f| f.ce == NONE16 || seqno_done(gt, f.ce, f.seqno)) {
            *f = new;
        }
    }

    /// `i915_ppgtt_create`, or an emptied address space kept from before.
    pub fn vm_create(&mut self, i915: &drm_i915_private, c: u8) -> Result<u8, i32> {
        let slot = self.vms.iter().position(|v| !v.used && v.ppgtt.is_some())
            .or_else(|| self.vms.iter().position(|v| !v.used))
            .ok_or(-ENOMEM)?;
        if self.vms[slot].ppgtt.is_none() {
            self.vms[slot].ppgtt = Some(crate::gen8_ppgtt::i915_ppgtt_create(i915)?);
        }
        let v = &mut self.vms[slot];
        v.used = true;
        v.client = c;
        v.refs = 1;
        Ok(slot as u8)
    }

    pub fn vm_get(&mut self, vm: u8) {
        self.vms[vm as usize].refs += 1;
    }

    /// `i915_vm_put`: on the last reference every binding goes
    /// (`i915_vm_close`), and the empty address space waits for reuse.
    pub fn vm_put(&mut self, vm: u8) {
        let v = &mut self.vms[vm as usize];
        if v.refs > 1 {
            v.refs -= 1;
            return;
        }
        v.refs = 0;
        for i in 0..MAX_OBJECTS {
                for k in 0..MAX_VMS {
                if self.objs[i].vm[k] == vm {
                    self.obj_unbind(i, k);
                }
            }
        }
        self.vms[vm as usize].used = false;
    }

    /// Binding `k` of object `i` cleared (`gen8_ppgtt_clear`).
    pub fn obj_unbind(&mut self, i: usize, k: usize) {
        let o = self.objs[i];
        let vm = o.vm[k];
        if vm == NONE8 {
            return;
        }
        if let Some(p) = self.vms[vm as usize].ppgtt.as_mut() {
            let _ = p.gen8_ppgtt_clear(o.start[k], o.pages as u64 * I915_GTT_PAGE_SIZE);
        }
        self.objs[i].vm[k] = NONE8;
        self.tlb_dirty = true;
    }

    /// `vma_invalidate_tlb` after unbinding: no engine may keep a
    /// translation to pages that are about to go back.
    pub fn tlb_flush(&mut self, i915: &mut drm_i915_private, gt: &intel_gt) {
        if self.tlb_dirty {
            crate::intel_tlb::intel_gt_invalidate_tlb_full(i915, gt);
            self.tlb_dirty = false;
        }
    }

    /// Object `i` bound at `start` of `vm` (`i915_vma_pin` with
    /// PIN_OFFSET_FIXED): the tree, then the pages from the kernel's list.
    pub fn obj_bind(&mut self, i: usize, vm: u8, start: u64) -> Result<(), i32> {
        let o = self.objs[i];
        let k = o.vm.iter().position(|&x| x == NONE8).ok_or(-ENOSPC)?;
        let size = o.pages as u64 * I915_GTT_PAGE_SIZE;
        let p = self.vms[vm as usize].ppgtt.as_mut().ok_or(-EINVAL)?;
        if let Err(e) = p.gen8_ppgtt_alloc(start, size) {
            log_bind_failure(p, vm, start, o.pages, e);
            return Err(e);
        }
        let mut list = [0u64; 512];
        let mut at = 0u32;
        let mut r = Ok(());
        while at < o.pages {
            let want = ((o.pages - at) as usize).min(list.len());
            let n = npk_sys::gpudev_bo_pages(o.bo as i32, at as i32, &mut list[..want]);
            if n <= 0 || n as usize > want || list[..n as usize].iter().any(|&a| a & (I915_GTT_PAGE_SIZE - 1) != 0) {
                r = Err(-EINVAL);
                break;
            }
            if let Err(x) = p.gen8_ppgtt_insert_pages(start + at as u64 * I915_GTT_PAGE_SIZE, &list[..n as usize],
                                                      TGL_PAT_CACHE_LLC, 0) {
                r = Err(x);
                break;
            }
            at += n as u32;
        }
        if r.is_err() {
            let _ = p.gen8_ppgtt_clear(start, size);
            return r;
        }
        self.objs[i].vm[k] = vm;
        self.objs[i].start[k] = start;
        Ok(())
    }

    /// `OP_OBJECT` attach.
    pub fn obj_attach(&mut self, c: u8, res_id: u32, bo: u32) -> Result<(), i32> {
        if self.obj(c, res_id).is_some() {
            return Err(-EINVAL);
        }
        let mut info = [0u8; 40];
        if npk_sys::gpudev_bo_info(bo as i32, &mut info) != 0 {
            return Err(-EINVAL);
        }
        let size = rd64(&info, 8);
        let pages = rd32(&info, 36);
        if pages == 0 || size > pages as u64 * I915_GTT_PAGE_SIZE {
            return Err(-EINVAL);
        }
        let i = self.objs.iter().position(|o| !o.used).ok_or(-ENOSPC)?;
        let h = hash_of(c, res_id);
        self.objs[i] = gem_obj { used: true, client: c, res_id, bo, size, pages, next: self.hash[h], ..gem_obj::EMPTY };
        self.hash[h] = i as u16;
        // Another client's object on the same buffer: one reservation, one
        // tiling.
        match self.objs.iter().enumerate().position(|(j, o)| j != i && o.used && o.bo == bo) {
            Some(j) => {
                self.objs[i].share = self.objs[j].share;
                self.objs[j].share = i as u16;
                self.objs[i].tiling = self.objs[j].tiling;
                self.objs[i].stride = self.objs[j].stride;
            }
            None => self.objs[i].share = i as u16,
        }
        Ok(())
    }

    /// Object `i` unbound everywhere and forgotten.
    pub fn obj_free(&mut self, i: usize) {
        for k in 0..MAX_VMS {
            self.obj_unbind(i, k);
        }
        let next = self.objs[i].share;
        if next != NONE16 && next as usize != i {
            let mut p = next as usize;
            while self.objs[p].share as usize != i {
                p = self.objs[p].share as usize;
            }
            self.objs[p].share = next;
        }
        if self.objs[i].zombie {
            self.nzombies -= 1;
        } else {
            self.obj_unlink(i);
        }
        self.objs[i] = gem_obj::EMPTY;
    }

    /// Object `i` no longer found by its client's res_id.
    fn obj_unlink(&mut self, i: usize) {
        let o = self.objs[i];
        let h = hash_of(o.client, o.res_id);
        if self.hash[h] == i as u16 {
            self.hash[h] = o.next;
        } else {
            let mut j = self.hash[h];
            while j != NONE16 {
                if self.objs[j as usize].next == i as u16 {
                    self.objs[j as usize].next = o.next;
                    break;
                }
                j = self.objs[j as usize].next;
            }
        }
        self.objs[i].next = NONE16;
    }

    /// A zombie whose requests are done: unbound, freed, and its buffer
    /// handed back to the kernel.
    fn obj_release(&mut self, i915: &mut drm_i915_private, gt: &intel_gt, i: usize) {
        let bo = self.objs[i].bo;
        self.obj_free(i);
        self.tlb_flush(i915, gt);
        npk_sys::gpudev_released(bo as i32);
    }

    /// `i915_gem_create_context` with the proto context's settings: a
    /// context without a VM gets an address space of its own.
    pub fn ctx_create(&mut self, i915: &drm_i915_private, c: u8, vm: Option<u8>, id: u32) -> Result<usize, i32> {
        let slot = self.ctxs.iter().position(|x| !x.used).ok_or(-ENOMEM)?;
        let vm = match vm {
            Some(v) => {
                self.vm_get(v);
                v
            }
            None => self.vm_create(i915, c)?,
        };
        self.ctxs[slot] = gem_ctx { used: true, client: c, id, vm, ..gem_ctx::EMPTY };
        Ok(slot)
    }

    /// The next context id of client `c`: `xa_alloc` in a
    /// `XA_FLAGS_ALLOC1` array, the lowest free id from 1.
    pub fn next_ctx_id(&mut self, c: u8) -> u32 {
        (1..).find(|&id| self.ctx(c, id).is_none()).unwrap_or(1)
    }

    /// `__assign_ppgtt`'s id: a new entry in `file_priv->vm_xa`.
    pub fn vm_id_add(&mut self, c: u8, vm: u8) -> Result<u32, i32> {
        let cl = &mut self.clients[c as usize];
        let k = cl.vm_ids.iter().position(|x| x.1 == NONE8).ok_or(-ENOMEM)?;
        // xa_alloc in an XA_FLAGS_ALLOC1 array: the lowest free id from 1.
        let id = (1..).find(|&id| !cl.vm_ids.iter().any(|x| x.1 != NONE8 && x.0 == id)).unwrap_or(1);
        cl.vm_ids[k] = (id, vm);
        self.vm_get(vm);
        Ok(id)
    }

    /// `I915_GEM_VM_DESTROY`.
    pub fn vm_id_remove(&mut self, c: u8, id: u32) -> Result<(), i32> {
        let k = self.clients[c as usize].vm_ids.iter().position(|x| x.1 != NONE8 && x.0 == id).ok_or(-ENOENT)?;
        let vm = self.clients[c as usize].vm_ids[k].1;
        self.clients[c as usize].vm_ids[k] = (0, NONE8);
        self.vm_put(vm);
        Ok(())
    }

    /// The hardware context of gem context `x` in engine slot `slot`, on
    /// engine `engine` (`i915_gem_context_get_engine`): created on first
    /// use, from the pool when an idle one is there, with the context's
    /// address space and a 16K ring (`intel_context_set_gem`).
    pub fn ctx_engine(&mut self, d: &mut drm, x: usize, slot: u8, engine: u8) -> Result<usize, i32> {
        let have = self.ctxs[x].ce[slot as usize];
        if have != NONE16 {
            return Ok(have as usize);
        }
        let pd = self.vms[self.ctxs[x].vm as usize].ppgtt.as_ref().map(|p| p.pd_dma()).ok_or(-EINVAL)?;
        let ce = match self.pool[..self.npool].iter().position(|&c| d.gt.contexts[c as usize].engine == engine as u32) {
            Some(k) => {
                let ce = self.pool[k] as usize;
                self.pool.copy_within(k + 1..self.npool, k);
                self.npool -= 1;
                ce
            }
            None => {
                let ce = crate::intel_context::intel_context_create(d.gt, engine as u32)?;
                d.gt.contexts[ce].ring_size = 16 * 1024;
                ce
            }
        };
        d.gt.contexts[ce].vm_pd = pd;
        d.gt.contexts[ce].init = false;
        if let Err(e) = crate::intel_context::intel_context_pin(d.i915, d.gt, d.ggtt, ce) {
            self.pool[self.npool] = ce as u16;
            self.npool += 1;
            return Err(e);
        }
        self.ctxs[x].ce[slot as usize] = ce as u16;
        Ok(ce)
    }

    /// A destroyed context whose requests are all done goes: its hardware
    /// contexts back to the pool once retired, its address space put.
    fn ctx_reap(&mut self, gt: &mut intel_gt, x: usize) -> bool {
        if self.pend.iter().any(|p| p.used && p.ctx as usize == x) {
            return false;
        }
        for &ce in self.ctxs[x].ce.iter() {
            if ce != NONE16 {
                // Its image is reused for another context: not before the
                // hardware has switched it out and saved it (Linux keeps an
                // image until its context is out of the ports).
                let c = &gt.contexts[ce as usize];
                if c.timeline.nrequests != 0 || c.active_count != 0 || c.inflight != 0 {
                    return false;
                }
            }
        }
        for k in 0..MAX_USER_ENGINES {
            let ce = self.ctxs[x].ce[k];
            if ce != NONE16 {
                crate::intel_context::intel_context_unpin(gt, ce as usize);
                self.pool[self.npool] = ce;
                self.npool += 1;
            }
        }
        let vm = self.ctxs[x].vm;
        self.ctxs[x] = gem_ctx::EMPTY;
        self.vm_put(vm);
        true
    }

    /// `i915_gem_context_destroy_ioctl` / `context_close`: the context
    /// can no longer be named; it goes once idle.
    pub fn ctx_destroy(&mut self, gt: &mut intel_gt, x: usize) {
        self.ctxs[x].closed = true;
        self.ctx_reap(gt, x);
    }

    /// The next fence of `ring` of client `c`: an execbuffer takes it when
    /// it comes in, whether it runs, waits or fails.
    pub fn ticket_take(&mut self, c: u8, ring: u8) -> u64 {
        let r = &mut self.clients[c as usize].rings[ring as usize];
        r.submitted += 1;
        r.submitted
    }

    /// The pending fence `ticket` on `ring` of client `c` for request `rq`.
    pub fn pend_add(&mut self, c: u8, ring: u8, rq: usize, ctx: usize, engine: u8, ticket: u64) {
        if let Some(p) = self.pend.iter_mut().find(|p| !p.used) {
            *p = pending { used: true, client: c, ring, ticket, rq: rq as u16, ctx: ctx as u16, engine, started_us: 0 };
        }
    }

    /// Whether a request can be added for client work without overflowing
    /// a table: a pending slot, a request slot outside the module's own
    /// reserve (`KERNEL_RESERVE`), room on the context's timeline.
    pub fn can_submit(&self, gt: &intel_gt, ce: Option<usize>) -> bool {
        use crate::i915_request::KERNEL_RESERVE;
        if !self.pend.iter().any(|p| !p.used) || gt.requests.iter().filter(|r| !r.used).count() <= KERNEL_RESERVE {
            return false;
        }
        if let Some(ce) = ce {
            if gt.contexts[ce].timeline.nrequests >= crate::intel_timeline::TIMELINE_MAX_REQUESTS {
                return false;
            }
        }
        true
    }

    /// The fence counters of client slot `c`, ring `ring`, to the kernel.
    pub fn report_ring(&self, c: u8, ring: u8) {
        let cl = &self.clients[c as usize];
        let r = cl.rings[ring as usize];
        npk_sys::gpudev_ring(cl.id as i32, ring as i32, r.submitted as i64, r.completed as i64);
    }

    /// What the service loop does after the interrupts: submit held
    /// requests whose fences have signalled, signal the fences of completed
    /// client requests in ring order, note a request that runs
    /// past the hang limit (the GT is wedged then; returns true), and reap
    /// destroyed contexts.
    pub fn poll(&mut self, i915: &mut drm_i915_private, gt: &mut intel_gt) -> bool {
        if gt.nwaiters != 0 {
            crate::i915_request::i915_request_release_waiters(i915, gt);
        }
        let now = linux::now_us();
        let mut hung = None;
        let mut touched = [(NONE8, 0u8); 16];
        let mut nt = 0;
        for k in 0..MAX_PENDING {
            let p = self.pend[k];
            if !p.used {
                continue;
            }
            let rq = p.rq as usize;
            if crate::i915_request::__i915_request_is_complete(gt, rq) {
                if gt.requests[rq].error == -EIO && !gt.wedged {
                    self.ctxs[p.ctx as usize].innocent += 1;
                }
                crate::i915_request::i915_request_put(gt, rq);
                self.pend[k].used = false;
                if nt < touched.len() && !touched[..nt].contains(&(p.client, p.ring)) {
                    touched[nt] = (p.client, p.ring);
                    nt += 1;
                } else if nt == touched.len() {
                    self.advance_all();
                }
                continue;
            }
            if crate::i915_request::__i915_request_has_started(gt, rq) {
                if p.started_us == 0 {
                    self.pend[k].started_us = now.max(1);
                } else if now - p.started_us > hang_us(gt, p.engine as usize) {
                    hung = Some(k);
                }
            }
        }
        for &(c, ring) in &touched[..nt] {
            if self.advance(c, ring) {
                self.report_ring(c, ring);
            }
        }
        for x in 0..MAX_GEM_CTX {
            if self.ctxs[x].used && self.ctxs[x].closed {
                self.ctx_reap(gt, x);
            }
        }
        if self.nzombies != 0 {
            for i in 0..MAX_OBJECTS {
                if self.objs[i].used && self.objs[i].zombie && !self.obj_in_use(gt, i) {
                    self.obj_release(i915, gt, i);
                }
            }
        }
        if self.nclosing != 0 {
            for c in 0..MAX_CLIENTS {
                let c8 = c as u8;
                if !self.clients[c].used || !self.clients[c].closing {
                    continue;
                }
                if !self.pend.iter().any(|p| p.used && p.client == c8) && !self.held.iter().any(|h| h.client == c8) {
                    crate::intel_gt_requests::intel_gt_retire_requests(i915, gt);
                    let id = self.clients[c].id;
                    let held = now.saturating_sub(self.clients[c].closing_us);
                    self.client_teardown(i915, gt, c);
                    if held > CLOSE_REPORT_US {
                        Line::new().s("native context: client ").dec(id as u64).s(" torn down ")
                            .dec(held / 1000).s(" ms after its close").say();
                    }
                } else if !self.clients[c].closing_told && now.saturating_sub(self.clients[c].closing_us) > CLOSE_REPORT_US {
                    self.clients[c].closing_told = true;
                    self.report_closing(gt, c);
                }
            }
        }
        if let Some(k) = hung {
            let p = self.pend[k];
            let cx = &mut self.ctxs[p.ctx as usize];
            cx.guilty += 1;
            if cx.bannable {
                cx.banned = true;
            }
            Line::new().s("native context: client ").dec(self.clients[p.client as usize].id as u64).s(" context ")
                .dec(cx.id as u64).s(" hung on engine ").dec(p.engine as u64)
                .s("; no engine reset is ported, wedging the GT").say();
            crate::intel_engine_cs::intel_engine_print_registers(i915, gt, p.engine as usize);
            crate::intel_gt::gen8_check_faults(i915);
            crate::intel_gt::intel_gt_set_wedged(i915, gt);
            return true;
        }
        false
    }

    /// `completed` of a ring: every ticket below the oldest still pending
    /// or waiting. Returns whether it moved.
    pub fn advance(&mut self, c: u8, ring: u8) -> bool {
        let running = self.pend.iter().filter(|p| p.used && p.client == c && p.ring == ring).map(|p| p.ticket);
        let waiting = self.held.iter().filter(|h| h.client == c && h.ring == ring).map(|h| h.ticket);
        let oldest = running.chain(waiting).min();
        let r = &mut self.clients[c as usize].rings[ring as usize];
        let done = oldest.map(|t| t - 1).unwrap_or(r.submitted);
        if done != r.completed {
            r.completed = done;
            return true;
        }
        false
    }

    fn advance_all(&mut self) {
        for c in 0..MAX_CLIENTS {
            if !self.clients[c].used {
                continue;
            }
            for ring in 0..MAX_RINGS {
                if self.clients[c].rings[ring].completed != self.clients[c].rings[ring].submitted
                    && self.advance(c as u8, ring as u8) {
                    self.report_ring(c as u8, ring as u8);
                }
            }
        }
    }
}

/// `OP_CLIENT` open: `i915_gem_context_open`, context 0 with an address
/// space of its own.
pub fn client_open(st: &mut nctx, i915: &drm_i915_private, id: u32) -> Result<(), i32> {
    if st.client_slot(id).is_some() {
        return Err(-EINVAL);
    }
    let c = st.clients.iter().position(|c| !c.used).ok_or(-ENOMEM)?;
    st.clients[c] = client { used: true, id, ..client::EMPTY };
    match st.ctx_create(i915, c as u8, None, 0) {
        Ok(x) => {
            st.clients[c].default_vm = st.ctxs[x].vm;
            Ok(())
        }
        Err(e) => {
            st.clients[c].used = false;
            Err(e)
        }
    }
}

/// `OP_CLIENT` close (`i915_gem_context_close`): the client's contexts can
/// no longer be named and take no work; the rest goes once its requests are
/// done (`nctx::poll`), as Linux frees a context's VM after its last request.
pub fn client_close(st: &mut nctx, id: u32) -> Result<(), i32> {
    let Some(c) = st.client_slot(id) else { return Ok(()) };
    if st.clients[c].closing {
        return Ok(());
    }
    st.clients[c].closing = true;
    st.clients[c].closing_us = linux::now_us();
    st.clients[c].closing_told = false;
    st.nclosing += 1;
    for x in 0..MAX_GEM_CTX {
        if st.ctxs[x].used && st.ctxs[x].client == c as u8 {
            st.ctxs[x].closed = true;
        }
    }
    Ok(())
}

/// A closing client still held after this long has what holds it logged.
const CLOSE_REPORT_US: u64 = 2_000_000;

impl nctx {
    /// What keeps closing client `c` from going: each of its pending
    /// fences' request (held, queued, in the ports, started, complete),
    /// and what a held one waits for.
    fn report_closing(&self, gt: &intel_gt, c: usize) {
        let c8 = c as u8;
        let n = self.pend.iter().filter(|p| p.used && p.client == c8).count();
        Line::new().s("native context: client ").dec(self.clients[c].id as u64).s(" closed ")
            .dec(CLOSE_REPORT_US / 1000).s(" ms ago, still ").dec(n as u64).s(" request(s); held ")
            .dec(gt.nwaiters as u64).s(", zombies ").dec(self.nzombies as u64).say();
        for p in self.pend.iter().filter(|p| p.used && p.client == c8).take(6) {
            let rq = p.rq as usize;
            let r = gt.requests[rq];
            let mut l = Line::new();
            l.s("  rq ").dec(rq as u64).s(" ctx ").dec(r.ce as u64).s(" seqno ").dec(r.seqno as u64)
                .s(" engine ").dec(p.engine as u64).s(": used ").dec(r.used as u64).s(" held ").dec(r.waiting as u64)
                .s(" queued ").dec(r.pqueue as u64).s(" active ").dec(r.active as u64)
                .s(" started ").dec(crate::i915_request::__i915_request_has_started(gt, rq) as u64)
                .s(" complete ").dec(crate::i915_request::__i915_request_is_complete(gt, rq) as u64)
                .s(" hwsp ").dec(gt.contexts[r.ce as usize].timeline.read_hwsp_seqno() as u64);
            for &(ce, seqno) in &r.deps[..r.ndeps as usize] {
                l.s(" waits ").dec(ce as u64).s("/").dec(seqno as u64)
                    .s(if crate::i915_request::fence_signaled(gt, ce, seqno) { "(done)" } else { "(open)" });
            }
            l.say();
        }
    }

    /// A closing client with no request left: its contexts, address spaces
    /// and objects go. Objects the kernel already detached (zombies) are
    /// reported released; the others the kernel detaches later and finds
    /// gone.
    fn client_teardown(&mut self, i915: &mut drm_i915_private, gt: &mut intel_gt, c: usize) {
        let c8 = c as u8;
        for x in 0..MAX_GEM_CTX {
            if self.ctxs[x].used && self.ctxs[x].client == c8 && !self.ctx_reap(gt, x) {
                // Not idle (only after a wedge): its hardware contexts are
                // dropped from the pool rather than reused.
                let vm = self.ctxs[x].vm;
                self.ctxs[x] = gem_ctx::EMPTY;
                self.vm_put(vm);
            }
        }
        for k in 0..MAX_VM_IDS {
            let (_, vm) = self.clients[c].vm_ids[k];
            if vm != NONE8 {
                self.clients[c].vm_ids[k] = (0, NONE8);
                self.vm_put(vm);
            }
        }
        for i in 0..MAX_OBJECTS {
            if self.objs[i].used && self.objs[i].client == c8 && !self.objs[i].zombie {
                self.obj_free(i);
            }
        }
        for v in 0..MAX_VMS {
            if self.vms[v].used && self.vms[v].client == c8 {
                self.vms[v].refs = 1;
                self.vm_put(v as u8);
            }
        }
        for i in 0..MAX_OBJECTS {
            if self.objs[i].used && self.objs[i].client == c8 && self.objs[i].zombie {
                self.obj_release(i915, gt, i);
            }
        }
        self.tlb_flush(i915, gt);
        self.clients[c] = client::EMPTY;
        self.nclosing -= 1;
    }
}

/// `OP_OBJECT` detach: an idle object is unbound and forgotten now (0); a
/// busy one stays as a zombie its client can no longer name, and the
/// kernel gets it back with `npk_gpudev_released` once its requests are
/// done (1), as Linux keeps a closed GEM object alive through its active
/// VMAs. Nothing waits for the GPU here.
pub fn obj_detach(st: &mut nctx, i915: &mut drm_i915_private, gt: &intel_gt, id: u32, res_id: u32)
                  -> Result<i32, i32> {
    // Whatever was unbound before is invalidated before the kernel may
    // free pages on a 0 here.
    let Some(c) = st.client_slot(id) else {
        st.tlb_flush(i915, gt);
        return Ok(0);
    };
    let Some(i) = st.obj(c as u8, res_id) else {
        st.tlb_flush(i915, gt);
        return Ok(0);
    };
    if !st.obj_in_use(gt, i) {
        st.obj_free(i);
        st.tlb_flush(i915, gt);
        return Ok(0);
    }
    st.obj_unlink(i);
    st.objs[i].zombie = true;
    st.nzombies += 1;
    Ok(1)
}
