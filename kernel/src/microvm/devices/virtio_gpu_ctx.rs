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
/// `VIRTGPU_DRM_CONTEXT_I915`.
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
/// `vdrm_shmem.rsp_mem_offset` this side sets: the i915 header (seqno,
/// rsp_mem_offset, banned_ctx_mask) is 16 bytes. Kept here, never read back
/// from the guest-writable buffer.
const RSP_MEM_OFFSET: usize = 16;

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
    /// Run one ccmd: `req` is exactly the guest's bytes, starting with its
    /// `vdrm_ccmd_req` header; a shorter request than the backend knows is
    /// zero-extended by the backend. A response, if any, is pushed to `rsp`
    /// starting with its `vdrm_ccmd_rsp` (len) header.
    fn ccmd(&mut self, ctx: u32, req: &[u8], rsp: &mut Vec<u8>) -> Result<(), i32>;
    /// The pages of object `blob_id` a ccmd created in `ctx`, taken out of
    /// the backend's blob table (a blob id names one resource), with its
    /// cache type for the guest mapping.
    fn take_blob(&mut self, ctx: u32, blob_id: u64) -> Option<(Arc<ForeignPixels>, u32)>;
    /// The guest let go of a resource that was object `blob_id`.
    fn release_blob(&mut self, ctx: u32, pages: &Arc<ForeignPixels>);
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
    pub ctx: u32,
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
    pub fn destroy(&mut self, ctx_id: u32) -> Vec<u32> {
        let Some(c) = self.ctxs.remove(&ctx_id) else { return Vec::new() };
        if let Some(b) = BACKEND.lock().as_mut() {
            b.destroy(c.handle);
        }
        self.host3d.iter().filter(|(_, r)| r.ctx == ctx_id).map(|(id, _)| *id).collect()
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
            if c.shmem.is_some() || size > MAX_SHMEM || size < 16 {
                return Err(RESP_ERR_INVALID_PARAM);
            }
            let pages = size.div_ceil(4096) as usize;
            let phys = crate::memory::allocate_contiguous(pages).ok_or(RESP_ERR_OUT_OF_MEMORY)?;
            // SAFETY: freshly allocated, identity-mapped frames.
            unsafe { core::ptr::write_bytes(phys as *mut u8, 0, pages * 4096) };
            // SAFETY: as above, within the first page.
            unsafe { core::ptr::write_volatile((phys + 4) as *mut u32, RSP_MEM_OFFSET as u32) };
            let list = (0..pages as u64).map(|i| phys + i * 4096).collect();
            c.shmem = Some(res_id);
            self.host3d.insert(res_id, Host3d {
                ctx: ctx_id, pages: Arc::new(ForeignPixels::new(list)), size, map_info: MAP_CACHE_CACHED,
                mapped: None, owned: true, shmem: true,
            });
            return Ok(());
        }
        let handle = c.handle;
        let (pages, map_info) = BACKEND.lock().as_mut().and_then(|b| b.take_blob(handle, blob_id))
            .ok_or(RESP_ERR_INVALID_PARAM)?;
        if pages.bytes() < size {
            return Err(RESP_ERR_INVALID_PARAM);
        }
        self.host3d.insert(res_id, Host3d {
            ctx: ctx_id, pages, size, map_info, mapped: None, owned: false, shmem: false,
        });
        Ok(())
    }

    /// The guest dropped HOST3D resource `res_id`; it must be unmapped.
    pub fn drop_resource(&mut self, res_id: u32) {
        let Some(r) = self.host3d.remove(&res_id) else { return };
        if r.shmem {
            if let Some(c) = self.ctxs.get_mut(&r.ctx) {
                c.shmem = None;
            }
        }
        if r.owned {
            if let Some(&first) = r.pages.pages.first() {
                crate::memory::deallocate_contiguous(first, r.pages.pages.len());
            }
        } else if let (Some(c), Some(b)) = (self.ctxs.get(&r.ctx), BACKEND.lock().as_mut()) {
            b.release_blob(c.handle, &r.pages);
        }
    }

    /// SUBMIT_3D: the ccmd stream of context `ctx_id` (port of
    /// `drm_context_submit_cmd`). Returns the fence started after it on
    /// `ring`, if the backend knows the context.
    pub fn submit(&mut self, ctx_id: u32, ring: u32, stream: &[u8]) -> Result<u64, ()> {
        let Some(c) = self.ctxs.get(&ctx_id) else { return Err(()) };
        if ring >= c.rings {
            return Err(());
        }
        let shmem = c.shmem.and_then(|id| self.host3d.get(&id)).map(|r| r.pages.clone());
        let mut guard = BACKEND.lock();
        let b = guard.as_mut().ok_or(())?;
        let align = b.ccmd_alignment() as usize;
        let mut off = 0;
        while stream.len() - off >= 16 {
            let hdr = &stream[off..];
            let word = |o: usize| u32::from_le_bytes([hdr[o], hdr[o + 1], hdr[o + 2], hdr[o + 3]]);
            let (len, seqno, rsp_off) = (word(4) as usize, word(8), word(12) as usize);
            if len > stream.len() - off || len < 16 || len % align != 0 || rsp_off % align != 0 {
                return Err(());
            }
            let mut rsp = Vec::new();
            b.ccmd(c.handle, &stream[off..off + len], &mut rsp).map_err(|_| ())?;
            if let Some(sh) = &shmem {
                write_response(sh, rsp_off, &rsp);
                shm_write32(sh, 0, seqno);
            }
            off += len;
        }
        if off != stream.len() {
            return Err(());
        }
        Ok(b.submit_fence(c.handle, ring))
    }

    pub fn fence_done(&mut self, ctx_id: u32, ring: u32, fence: u64) -> bool {
        let Some(c) = self.ctxs.get(&ctx_id) else { return true };
        BACKEND.lock().as_mut().is_none_or(|b| b.fence_done(c.handle, ring, fence))
    }

    /// Every context gone (device reset, VM end): what the caller unmaps
    /// and drops first.
    pub fn clear(&mut self) -> Vec<u32> {
        let ids: Vec<u32> = self.ctxs.keys().copied().collect();
        let mut res = Vec::new();
        for id in ids {
            res.extend(self.destroy(id));
        }
        res
    }
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

/// Copy a response into the guest's response memory at `rsp_off`, bounded
/// by the buffer and by the length the guest left in the slot (read once:
/// the guest may change it meanwhile), as `drm_context_submit_cmd_dispatch`
/// does with its shadow buffer.
fn write_response(sh: &ForeignPixels, rsp_off: usize, rsp: &[u8]) {
    if rsp.len() < 4 {
        return;
    }
    let total = sh.pages.len() * 4096;
    let rsp_mem = total - RSP_MEM_OFFSET;
    if rsp_off > rsp_mem || rsp.len() > rsp_mem - rsp_off {
        return;
    }
    let start = RSP_MEM_OFFSET + rsp_off;
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
        use crate::drivers::pci;
        let put = |out: &mut [u8], at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        // wire_format_version, drm version 1.6.0 (i915), context_type.
        put(out, 0, 2);
        put(out, 4, 1);
        put(out, 8, 6);
        put(out, 12, 0);
        put(out, 16, CONTEXT_I915);
        // u.intel: bus, dev, func, revision, domain, device id.
        put(out, 24, self.dev.bus as u32);
        put(out, 28, self.dev.device as u32);
        put(out, 32, self.dev.function as u32);
        put(out, 36, pci::read8(self.dev, 0x08) as u32);
        put(out, 40, 0);
        put(out, 44, pci::read16(self.dev, 0x02) as u32);
    }

    fn create(&mut self) -> Result<u32, i32> {
        self.next += 1;
        Ok(self.next)
    }

    fn destroy(&mut self, _ctx: u32) {}

    fn ccmd_alignment(&self) -> u32 {
        4
    }

    fn ccmd(&mut self, _ctx: u32, req: &[u8], _rsp: &mut Vec<u8>) -> Result<(), i32> {
        let cmd = u32::from_le_bytes([req[0], req[1], req[2], req[3]]);
        crate::kprintln!("[gpu] native context probe: ccmd {} ({} bytes) has no backend", cmd, req.len());
        Err(-19)
    }

    fn take_blob(&mut self, _ctx: u32, _blob_id: u64) -> Option<(Arc<ForeignPixels>, u32)> {
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
