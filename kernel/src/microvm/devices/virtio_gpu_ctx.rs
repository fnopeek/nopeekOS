//! virtio-gpu DRM native context, device side (docs/plan/GPU_IN_VM.md §2,
//! GPU_STACK.md stage 6).
//!
//! The guest's Mesa driver for the host's real GPU sends its kernel calls
//! as ccmds in SUBMIT_3D; the answers go into a response buffer the guest
//! maps (a HOST3D blob with blob id 0). This file is the generic part of
//! virglrenderer `src/drm/drm_context.c`: contexts, the ccmd stream, the
//! shadowed response copy, HOST3D blobs and their pages. What a ccmd means
//! for the card is the backend's (`Backend`, registered by the GPU driver's
//! side in stage 7); without one, nothing here is offered to the guest.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::shade::buffers::ForeignPixels;

/// `VIRTIO_GPU_CAPSET_DRM`.
pub const CAPSET_DRM: u32 = 6;
/// Size the device reports for `struct virgl_renderer_capset_drm` (Mesa
/// `drm_hw.h`). Its amdgpu member depends on libdrm's structs, so the guest
/// build decides the real size; the guest kernel copies the smaller of the
/// two, and the tail past the card's member is zero, which the struct
/// defines as "not supported by the host".
pub const CAPSET_DRM_SIZE: u32 = 1024;
/// `VIRTGPU_DRM_CONTEXT_AMDGPU`, `VIRTGPU_DRM_CONTEXT_I915`.
pub const CONTEXT_AMDGPU: u32 = 2;
pub const CONTEXT_I915: u32 = 3;
/// virtio `map_info` cache types (`VIRTIO_GPU_MAP_CACHE_*`).
pub const MAP_CACHE_CACHED: u32 = 1;
pub const MAP_CACHE_UNCACHED: u32 = 2;
pub const MAP_CACHE_WC: u32 = 3;
/// Rings a context may name (`VIRTGPU_CONTEXT_PARAM_NUM_RINGS` upper bound
/// in the guest driver).
pub const MAX_RINGS: u32 = 64;
/// Largest response buffer and object a guest may ask for.
const MAX_SHMEM: u64 = 1 << 20;
const MAX_OBJECT: u64 = 1 << 32;
/// Contexts per guest.
const MAX_CONTEXTS: usize = 64;
/// `vdrm_shmem.rsp_mem_offset` of i915: its header (seqno, rsp_mem_offset,
/// banned_ctx_mask) is 16 bytes.
pub const I915_RSP_MEM_OFFSET: usize = 16;

/// What the GPU driver side does for native contexts. Every call comes
/// from the virtio-gpu worker with the device lock held.
pub trait Backend: Send {
    /// The capset: header fields and the card's union member, written into
    /// `out` (zeroed, `CAPSET_DRM_SIZE` bytes) from offset 0.
    fn capset(&self, out: &mut [u8]);
    /// A new guest context; its handle.
    fn create(&mut self) -> Result<u32, i32>;
    fn destroy(&mut self, ctx: u32);
    /// Byte alignment of ccmds and response offsets (4 for i915).
    fn ccmd_alignment(&self) -> u32;
    /// `vdrm_shmem.rsp_mem_offset`: the size of the card's shmem header.
    /// Kept by this side, never read back from the guest-writable buffer.
    fn rsp_mem_offset(&self) -> usize;
    /// Words of the card's shmem header to write now (offset, value), after
    /// each ccmd and, with `heap`, when an object becomes a resource
    /// (amdgpu's `async_error` and heap usage).
    fn shmem_update(&mut self, _ctx: u32, _heap: bool) -> Vec<(usize, u32)> {
        Vec::new()
    }
    /// Run one ccmd: `req` is exactly the guest's bytes, starting with its
    /// `vdrm_ccmd_req` header; a shorter request than the backend knows is
    /// zero-extended by the backend. A response, if any, is pushed to `rsp`
    /// starting with its `vdrm_ccmd_rsp` (len) header, by `deadline` (TSC,
    /// shared by the ccmds of one SUBMIT_3D). A failed driver call
    /// is an `Ok` with the positive errno in the response's `ret`, as
    /// `intel_ioctl` reports it; `Err` is a malformed request only: the
    /// sequence number then stays put and the guest's waiter never returns.
    fn ccmd(&mut self, ctx: u32, req: &[u8], rsp: &mut Vec<u8>, deadline: u64) -> Result<(), i32>;
    /// The pages of object `blob_id` a ccmd created in `ctx`, taken out of
    /// the backend's blob table (a blob id names one resource) to become
    /// resource `res_id`, with its cache type for the guest mapping.
    fn take_blob(&mut self, ctx: u32, blob_id: u64, res_id: u32) -> Option<(Arc<ForeignPixels>, u32)>;
    /// The cache type of resource `res_id` as it is mapped now (MAP_BLOB),
    /// when the backend decides it; ccmds may change it until then.
    fn map_info(&mut self, _ctx: u32, _res_id: u32) -> Option<u32> {
        None
    }
    /// Bits of `banned_ctx_mask` the ccmd just run sets and clears.
    fn banned(&mut self, _ctx: u32) -> (u64, u64) {
        (0, 0)
    }
    /// The guest let go of a resource that was object `blob_id`.
    fn release_blob(&mut self, ctx: u32, pages: &Arc<ForeignPixels>);
    /// Context `ctx` uses resource `res_id`, an object of another context
    /// (`i915_renderer_attach_resource`: imported under the same res_id).
    fn attach_resource(&mut self, _ctx: u32, _res_id: u32, _pages: &Arc<ForeignPixels>) {}
    /// Context `ctx` no longer uses that resource.
    fn detach_resource(&mut self, _ctx: u32, _res_id: u32) {}
    /// Whether `fence` queued on `ring` of `ctx` is done; `submit_fence`
    /// starts one after the ccmds of a SUBMIT_3D.
    fn submit_fence(&mut self, ctx: u32, ring: u32) -> u64;
    fn fence_done(&mut self, ctx: u32, ring: u32, fence: u64) -> bool;
}

static BACKEND: Mutex<Option<Box<dyn Backend>>> = Mutex::new(None);

/// The GPU driver side offers native contexts (stage 7).
pub fn set_backend(b: Option<Box<dyn Backend>>) {
    *BACKEND.lock() = b;
}

pub fn backend_ready() -> bool {
    BACKEND.lock().is_some()
}

/// The capset the device offers, if any.
pub fn capset() -> Option<Vec<u8>> {
    let b = BACKEND.lock();
    let b = b.as_ref()?;
    let mut out = alloc::vec![0u8; CAPSET_DRM_SIZE as usize];
    b.capset(&mut out);
    Some(out)
}

/// A HOST3D resource: pages the host owns, mapped into the guest's host-
/// visible window on MAP_BLOB.
pub struct Host3d {
    /// The context that made it. The guest reuses a destroyed context's id,
    /// so the id alone does not name that context once it is gone.
    pub ctx: u32,
    /// The backend client of `ctx`: the resource outlives its context
    /// (virglrenderer keeps resources per device), and is released there.
    handle: u32,
    pub pages: Arc<ForeignPixels>,
    pub size: u64,
    pub map_info: u32,
    /// Offset in the host-visible window while mapped.
    pub mapped: Option<u64>,
    /// Frames this file allocated (the response buffer), freed with it.
    owned: bool,
    /// The guest's response buffer of `ctx`.
    shmem: bool,
}

struct Ctx {
    handle: u32,
    rings: u32,
    /// Resource id of the response buffer.
    shmem: Option<u32>,
}

/// Per-device native-context state.
pub struct Contexts {
    ctxs: BTreeMap<u32, Ctx>,
    pub host3d: BTreeMap<u32, Host3d>,
}

impl Contexts {
    pub const fn new() -> Self {
        Contexts { ctxs: BTreeMap::new(), host3d: BTreeMap::new() }
    }

    /// CTX_CREATE: `context_init` names the capset (low 8 bits).
    pub fn create(&mut self, ctx_id: u32, context_init: u32) -> Result<(), ()> {
        if context_init & 0xff != CAPSET_DRM || self.ctxs.contains_key(&ctx_id) || self.ctxs.len() >= MAX_CONTEXTS {
            return Err(());
        }
        let handle = BACKEND.lock().as_mut().ok_or(())?.create().map_err(|_| ())?;
        self.ctxs.insert(ctx_id, Ctx { handle, rings: MAX_RINGS, shmem: None });
        Ok(())
    }

    /// CTX_DESTROY: the backend context and every HOST3D resource it made.
    /// Returns the resources the caller must unmap and drop first.
    /// CTX_ATTACH_RESOURCE: a HOST3D resource of another context becomes
    /// usable in `ctx_id` (`drm_context` attach_resource).
    pub fn attach_resource(&mut self, ctx_id: u32, res_id: u32) {
        let (Some(c), Some(r)) = (self.ctxs.get(&ctx_id), self.host3d.get(&res_id)) else { return };
        if r.handle != c.handle && !r.shmem {
            if let Some(b) = BACKEND.lock().as_mut() {
                b.attach_resource(c.handle, res_id, &r.pages);
            }
        }
    }

    /// CTX_DETACH_RESOURCE (`drm_context_detach_resource`).
    pub fn detach_resource(&mut self, ctx_id: u32, res_id: u32) {
        let (Some(c), Some(r)) = (self.ctxs.get(&ctx_id), self.host3d.get(&res_id)) else { return };
        if r.handle != c.handle {
            if let Some(b) = BACKEND.lock().as_mut() {
                b.detach_resource(c.handle, res_id);
            }
        }
    }

    /// CTX_DESTROY (`drm_context_destroy`): the context goes, its
    /// resources stay until the guest unrefs them.
    pub fn destroy(&mut self, ctx_id: u32) {
        let Some(c) = self.ctxs.remove(&ctx_id) else { return };
        if let Some(b) = BACKEND.lock().as_mut() {
            b.destroy(c.handle);
        }
    }

    /// RESOURCE_CREATE_BLOB with `BLOB_MEM_HOST3D` in context `ctx_id`.
    /// Blob id 0 is the context's response buffer, which this side
    /// allocates; any other names an object a ccmd created.
    pub fn create_blob(&mut self, ctx_id: u32, res_id: u32, blob_id: u64, size: u64) -> Result<(), u32> {
        use super::virtio_gpu_pci::{RESP_ERR_INVALID_PARAM, RESP_ERR_OUT_OF_MEMORY, RESP_ERR_INVALID_CONTEXT_ID};
        let Some(c) = self.ctxs.get_mut(&ctx_id) else { return Err(RESP_ERR_INVALID_CONTEXT_ID) };
        if self.host3d.contains_key(&res_id) || size == 0 || size > MAX_OBJECT {
            return Err(RESP_ERR_INVALID_PARAM);
        }
        if blob_id == 0 {
            let mut guard = BACKEND.lock();
            let b = guard.as_mut().ok_or(RESP_ERR_INVALID_PARAM)?;
            let rsp_mem_offset = b.rsp_mem_offset();
            if c.shmem.is_some() || size > MAX_SHMEM || size <= rsp_mem_offset as u64 {
                return Err(RESP_ERR_INVALID_PARAM);
            }
            let pages = size.div_ceil(4096) as usize;
            let phys = crate::memory::allocate_contiguous(pages).ok_or(RESP_ERR_OUT_OF_MEMORY)?;
            // SAFETY: freshly allocated, identity-mapped frames.
            unsafe { core::ptr::write_bytes(phys as *mut u8, 0, pages * 4096) };
            // SAFETY: as above, within the first page.
            unsafe { core::ptr::write_volatile((phys + 4) as *mut u32, rsp_mem_offset as u32) };
            let list = (0..pages as u64).map(|i| phys + i * 4096).collect();
            c.shmem = Some(res_id);
            let handle = c.handle;
            let pages = Arc::new(ForeignPixels::new(list));
            shm_apply(&pages, rsp_mem_offset, &b.shmem_update(handle, true));
            self.host3d.insert(res_id, Host3d {
                ctx: ctx_id, handle, pages, size, map_info: MAP_CACHE_CACHED, mapped: None, owned: true, shmem: true,
            });
            return Ok(());
        }
        let handle = c.handle;
        let shmem = c.shmem.and_then(|id| self.host3d.get(&id)).map(|r| r.pages.clone());
        let mut guard = BACKEND.lock();
        let b = guard.as_mut().ok_or(RESP_ERR_INVALID_PARAM)?;
        let (pages, map_info) = b.take_blob(handle, blob_id, res_id).ok_or(RESP_ERR_INVALID_PARAM)?;
        if pages.bytes() < size {
            b.release_blob(handle, &pages);
            return Err(RESP_ERR_INVALID_PARAM);
        }
        if let Some(sh) = shmem {
            let words = b.shmem_update(handle, true);
            shm_apply(&sh, b.rsp_mem_offset(), &words);
        }
        self.host3d.insert(res_id, Host3d {
            ctx: ctx_id, handle, pages, size, map_info, mapped: None, owned: false, shmem: false,
        });
        Ok(())
    }

    /// The cache type HOST3D resource `res_id` is mapped with now.
    pub fn map_info(&mut self, res_id: u32) -> Option<u32> {
        let r = self.host3d.get(&res_id)?;
        if r.shmem {
            return Some(r.map_info);
        }
        let own = r.map_info;
        Some(BACKEND.lock().as_mut().and_then(|b| b.map_info(r.handle, res_id)).unwrap_or(own))
    }

    /// The guest dropped HOST3D resource `res_id`, which must be unmapped:
    /// it leaves the guest's namespace (its id may be reused at once); its
    /// pages go with `free_resource`.
    pub fn take_resource(&mut self, res_id: u32) -> Option<Host3d> {
        let r = self.host3d.remove(&res_id)?;
        if r.shmem
            && let Some(c) = self.ctxs.get_mut(&r.ctx)
            && c.shmem == Some(res_id) {
            c.shmem = None;
        }
        Some(r)
    }

    /// Free a resource `take_resource` gave out.
    pub fn free_resource(&mut self, r: Host3d) {
        if r.owned {
            if let Some(&first) = r.pages.pages.first() {
                crate::memory::deallocate_contiguous(first, r.pages.pages.len());
            }
        } else if let Some(b) = BACKEND.lock().as_mut() {
            b.release_blob(r.handle, &r.pages);
        }
    }

    /// What SUBMIT_3D on `ring` of context `ctx_id` needs to run its ccmds
    /// later, without the device: None if there is no such context or ring.
    pub fn prepare_submit(&self, ctx_id: u32, ring: u32) -> Option<SubmitJob> {
        let c = self.ctxs.get(&ctx_id)?;
        if ring >= c.rings {
            return None;
        }
        let shmem = c.shmem.and_then(|id| self.host3d.get(&id)).map(|r| r.pages.clone());
        Some(SubmitJob { handle: c.handle, ring, shmem })
    }

    pub fn fence_done(&mut self, ctx_id: u32, ring: u32, fence: u64) -> bool {
        let Some(c) = self.ctxs.get(&ctx_id) else { return true };
        BACKEND.lock().as_mut().is_none_or(|b| b.fence_done(c.handle, ring, fence))
    }

    /// Every context gone (device reset, VM end); the HOST3D resources,
    /// which the caller unmaps and drops.
    pub fn clear(&mut self) -> Vec<u32> {
        let ids: Vec<u32> = self.ctxs.keys().copied().collect();
        for id in ids {
            self.destroy(id);
        }
        self.host3d.keys().copied().collect()
    }
}

/// A SUBMIT_3D taken off the controlq, to run without the device lock.
pub struct SubmitJob {
    handle: u32,
    ring: u32,
    /// The context's response buffer, if it has one yet.
    shmem: Option<Arc<ForeignPixels>>,
}

/// SUBMIT_3D: the ccmd stream of a context (port of `drm_context_submit_cmd`),
/// all by `deadline`. Takes only the backend lock, never the device's: the
/// ccmds wait for the GPU driver, and vCPUs take the device lock on their
/// exits. Returns the fence started after it on the job's ring.
pub fn run_submit(job: &SubmitJob, stream: &[u8], deadline: u64) -> Result<u64, ()> {
    use super::gpu_backend::{note, us_since, STAT_SUBMIT, STAT_SUBMIT_US};
    let t0 = crate::interrupts::rdtsc();
    let r = run_submit_inner(job, stream, deadline);
    note(STAT_SUBMIT, 1);
    note(STAT_SUBMIT_US, us_since(t0));
    r
}

fn run_submit_inner(job: &SubmitJob, stream: &[u8], deadline: u64) -> Result<u64, ()> {
    let mut guard = BACKEND.lock();
    let b = guard.as_mut().ok_or(())?;
    let align = b.ccmd_alignment() as usize;
    let rsp_mem_offset = b.rsp_mem_offset();
    let mut off = 0;
    while stream.len() - off >= 16 {
        let hdr = &stream[off..];
        let word = |o: usize| u32::from_le_bytes([hdr[o], hdr[o + 1], hdr[o + 2], hdr[o + 3]]);
        let (len, seqno, rsp_off) = (word(4) as usize, word(8), word(12) as usize);
        if len > stream.len() - off || len < 16 || len % align != 0 || rsp_off % align != 0 {
            return Err(());
        }
        let mut rsp = Vec::new();
        super::gpu_backend::note(super::gpu_backend::STAT_CCMD, 1);
        let ran = b.ccmd(job.handle, &stream[off..off + len], &mut rsp, deadline);
        // The header (amdgpu's `async_error`) is written even when the
        // handler failed: the guest reads it to see the context lost.
        let words = b.shmem_update(job.handle, false);
        if let Some(sh) = &job.shmem {
            shm_apply(sh, rsp_mem_offset, &words);
        }
        ran.map_err(|_| ())?;
        let (set, clear) = b.banned(job.handle);
        if let Some(sh) = &job.shmem {
            write_response(sh, rsp_mem_offset, rsp_off, &rsp);
            if set | clear != 0 {
                let mask = (shm_read32(sh, 8) as u64 | (shm_read32(sh, 12) as u64) << 32) & !clear | set;
                shm_write32(sh, 8, mask as u32);
                shm_write32(sh, 12, (mask >> 32) as u32);
            }
            shm_write32(sh, 0, seqno);
        }
        off += len;
    }
    if off != stream.len() {
        return Err(());
    }
    Ok(b.submit_fence(job.handle, job.ring))
}

fn shm_write32(sh: &ForeignPixels, off: usize, v: u32) {
    let page = sh.pages[off / 4096];
    // SAFETY: `off` lies in the response buffer's identity-mapped frames
    // (callers bound it); 4-byte aligned.
    unsafe { core::ptr::write_volatile((page + (off % 4096) as u64) as *mut u32, v) };
}

fn shm_read32(sh: &ForeignPixels, off: usize) -> u32 {
    let page = sh.pages[off / 4096];
    // SAFETY: as in `shm_write32`.
    unsafe { core::ptr::read_volatile((page + (off % 4096) as u64) as *const u32) }
}

/// Header words of the shmem buffer, each inside the header.
fn shm_apply(sh: &ForeignPixels, rsp_mem_offset: usize, words: &[(usize, u32)]) {
    for &(off, v) in words {
        if off % 4 == 0 && off + 4 <= rsp_mem_offset {
            shm_write32(sh, off, v);
        }
    }
}

/// Copy a response into the guest's response memory at `rsp_off`, bounded
/// by the buffer and by the length the guest left in the slot (read once:
/// the guest may change it meanwhile), as `drm_context_submit_cmd_dispatch`
/// does with its shadow buffer.
fn write_response(sh: &ForeignPixels, rsp_mem_offset: usize, rsp_off: usize, rsp: &[u8]) {
    if rsp.len() < 4 {
        return;
    }
    let total = sh.pages.len() * 4096;
    let rsp_mem = total - rsp_mem_offset;
    if rsp_off > rsp_mem || rsp.len() > rsp_mem - rsp_off {
        return;
    }
    let start = rsp_mem_offset + rsp_off;
    let untrusted = shm_read32(sh, start) as usize;
    let len = rsp.len().min(untrusted);
    for (i, &byte) in rsp[..len].iter().enumerate() {
        let at = start + i;
        let page = sh.pages[at / 4096];
        // SAFETY: `at` < total, inside the buffer's identity-mapped frames.
        unsafe { core::ptr::write_volatile((page + (at % 4096) as u64) as *mut u8, byte) };
    }
    shm_write32(sh, start, len as u32);
}

/// The capset of an i915 card at `dev` (virglrenderer `i915_renderer_probe`).
pub fn i915_capset(dev: crate::drivers::pci::PciAddr, out: &mut [u8]) {
    use crate::drivers::pci;
    let put = |out: &mut [u8], at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    // wire_format_version, drm version 1.6.0 (i915), context_type.
    put(out, 0, 2);
    put(out, 4, 1);
    put(out, 8, 6);
    put(out, 12, 0);
    put(out, 16, CONTEXT_I915);
    // u.intel: bus, dev, func, revision, domain, device id.
    put(out, 24, dev.bus as u32);
    put(out, 28, dev.device as u32);
    put(out, 32, dev.function as u32);
    put(out, 36, pci::read8(dev, 0x08) as u32);
    put(out, 40, 0);
    put(out, 44, pci::read16(dev, 0x02) as u32);
}

/// The response of i915 ccmd `cmd` for a driver call that failed with
/// positive `errno`. Layouts of Mesa's i915_proto.h: IOCTL_SIMPLE (1),
/// GEM_EXECBUFFER2 (7) and GEM_EXECBUFFER2_V2 (10) carry `ret` right after
/// the vdrm header (len), GETPARAM (2), QUERYPARAM (3), GEM_CONTEXT_CREATE
/// (6) and GEM_BUSY (9) after a pad word, with one more word behind it; the
/// others have no response, and it is empty.
pub fn i915_error_response(cmd: u32, errno: i32) -> Vec<u8> {
    // The nopeekOS context-parameter commands answer ret, size, pad, value.
    let (ret_at, len) = match cmd {
        1 | 7 | 10 | 0x102 => (4, 12),
        2 | 3 | 6 | 9 => (8, 16),
        0x100 | 0x101 => (4, 24),
        _ => return Vec::new(),
    };
    let mut rsp = alloc::vec![0u8; len];
    rsp[..4].copy_from_slice(&(len as u32).to_le_bytes());
    rsp[ret_at..ret_at + 4].copy_from_slice(&errno.to_le_bytes());
    rsp
}

/// `set microvm_gpu test`: offers the capset of the host's display
/// controller and answers every ccmd with "no device", so the guest's side
/// (features, capset, contexts, the response buffer, the host-visible
/// window) can be checked before a real backend exists. The guest's Mesa
/// then falls back to the CPU.
pub struct ProbeBackend {
    dev: crate::drivers::pci::PciAddr,
    next: u32,
}

impl ProbeBackend {
    /// For the display controller at 00:02.0, if there is one.
    pub fn new() -> Option<Self> {
        use crate::drivers::pci;
        let dev = pci::PciAddr { bus: 0, device: 2, function: 0 };
        (pci::read16(dev, 0) != 0xFFFF && pci::read8(dev, 0x0B) == 0x03).then_some(ProbeBackend { dev, next: 1 })
    }
}

impl Backend for ProbeBackend {
    fn capset(&self, out: &mut [u8]) {
        i915_capset(self.dev, out);
    }

    fn create(&mut self) -> Result<u32, i32> {
        self.next += 1;
        Ok(self.next)
    }

    fn destroy(&mut self, _ctx: u32) {}

    fn ccmd_alignment(&self) -> u32 {
        4
    }

    fn rsp_mem_offset(&self) -> usize {
        I915_RSP_MEM_OFFSET
    }

    fn ccmd(&mut self, _ctx: u32, req: &[u8], rsp: &mut Vec<u8>, _deadline: u64) -> Result<(), i32> {
        const ENODEV: i32 = 19;
        let cmd = u32::from_le_bytes([req[0], req[1], req[2], req[3]]);
        crate::kprintln!("[gpu] native context probe: ccmd {} ({} bytes) has no backend", cmd, req.len());
        *rsp = i915_error_response(cmd, ENODEV);
        Ok(())
    }

    fn take_blob(&mut self, _ctx: u32, _blob_id: u64, _res_id: u32) -> Option<(Arc<ForeignPixels>, u32)> {
        None
    }

    fn release_blob(&mut self, _ctx: u32, _pages: &Arc<ForeignPixels>) {}

    fn submit_fence(&mut self, _ctx: u32, _ring: u32) -> u64 {
        0
    }

    fn fence_done(&mut self, _ctx: u32, _ring: u32, _fence: u64) -> bool {
        true
    }
}
