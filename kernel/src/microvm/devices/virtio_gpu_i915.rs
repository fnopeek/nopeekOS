//! The i915 backend of native contexts (docs/plan/GPU_IN_VM.md §4b): the
//! host side of virglrenderer `src/drm/i915/` with the GPU driver module in
//! the place of the host's i915. GEM objects are kernel frames (GEM_CREATE,
//! GEM_CREATE_EXT, GEM_SET_MMAP_MODE are served here, as
//! `i915_ccmd_gem_create*` and `i915_ccmd_gem_set_mmap_mode` do); every
//! other ccmd goes to the module (`gpudev::ccmd`), and fences follow the
//! submission counts it reports per ring.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::virtio_gpu_ctx::{i915_capset, i915_error_response, Backend, MAP_CACHE_CACHED, MAP_CACHE_UNCACHED, MAP_CACHE_WC};
use crate::drivers::pci::PciAddr;
use crate::gpu::gpudev::{self, Answer};
use crate::shade::buffers::ForeignPixels;

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const EIO: i32 = 5;
const PAGE: u64 = 4096;
/// GEM bytes one VM may hold: protection against a runaway guest, the
/// host's reserve is checked per allocation as well.
const MAX_GEM_BYTES: u64 = 4 << 30;
const MAX_OBJECT: u64 = 1 << 32;

/// Mesa `i915_proto.h` ccmd numbers (and virglrenderer's EXECBUFFER2_V2).
const CCMD_IOCTL_SIMPLE: u32 = 1;
const CCMD_GEM_CREATE: u32 = 4;
const CCMD_GEM_CREATE_EXT: u32 = 5;
const CCMD_GEM_EXECBUFFER2: u32 = 7;
const CCMD_GEM_SET_MMAP_MODE: u32 = 8;
const CCMD_GEM_EXECBUFFER2_V2: u32 = 10;
/// `DRM_IOCTL_I915_GEM_CONTEXT_DESTROY`.
const IOCTL_GEM_CONTEXT_DESTROY: u32 = 0x4008_646e;
/// `I915_GEM_CREATE_EXT_*`.
const EXT_MEMORY_REGIONS: u32 = 0;
const EXT_PROTECTED_CONTENT: u32 = 1;
const EXT_SET_PAT: u32 = 2;
const MEMORY_CLASS_DEVICE: u16 = 1;
/// `I915_MMAP_OFFSET_*`.
const MMAP_GTT: u32 = 0;
const MMAP_WC: u32 = 1;
const MMAP_WB: u32 = 2;
const MMAP_UC: u32 = 3;
const MMAP_FIXED: u32 = 4;

/// Client handles are unique for the boot, so a late report about a closed
/// client never meets a new one.
static NEXT_CLIENT: AtomicU32 = AtomicU32::new(1);

/// `struct i915_object`, with the frames it lives in.
struct Object {
    /// The gpudev buffer of the frames.
    id: u32,
    frames: Arc<ForeignPixels>,
    size: u64,
    map_info: u32,
    mmap_configured: bool,
    /// The guest mapped the resource (`res->mapped`).
    mapped: bool,
}

#[derive(Default)]
struct Client {
    /// Objects a GEM_CREATE made, by blob id, until their resource exists.
    parked: BTreeMap<u32, Object>,
    /// Objects by resource id.
    objects: BTreeMap<u32, Object>,
    /// `banned_ctx_mask` changes of the last ccmd.
    ban_set: u64,
    ban_clear: u64,
    /// CTX_DESTROY came; the entry stays until its resources are given back.
    closed: bool,
}

pub struct GpudevBackend {
    dev: PciAddr,
    clients: BTreeMap<u32, Client>,
    gem_bytes: u64,
    refused_logged: bool,
}

impl GpudevBackend {
    pub fn new(dev: PciAddr) -> Self {
        GpudevBackend { dev, clients: BTreeMap::new(), gem_bytes: 0, refused_logged: false }
    }

    /// A zeroed object of `size` bytes, its frames granted to the card.
    fn object_create(&mut self, size: u64) -> Option<Object> {
        let size = size.div_ceil(PAGE).checked_mul(PAGE)?;
        if size == 0 || size > MAX_OBJECT || self.gem_bytes + size > MAX_GEM_BYTES {
            self.log_refused(size);
            return None;
        }
        let pages = (size / PAGE) as usize;
        if super::guest_mem::frames_spare() < pages {
            self.log_refused(size);
            return None;
        }
        let mut list = Vec::new();
        list.try_reserve_exact(pages).ok()?;
        for _ in 0..pages {
            let Some(p) = crate::memory::allocate_frame() else {
                for &q in &list {
                    crate::memory::deallocate_frame(q);
                }
                return None;
            };
            // SAFETY: a frame just allocated, identity-mapped, owned here.
            unsafe { core::ptr::write_bytes(p as *mut u8, 0, PAGE as usize) };
            list.push(p);
        }
        let frames = Arc::new(ForeignPixels::new(list));
        let Some(id) = gpudev::object_add(&frames, size) else {
            for &q in &frames.pages {
                crate::memory::deallocate_frame(q);
            }
            return None;
        };
        self.gem_bytes += size;
        Some(Object { id, frames, size, map_info: MAP_CACHE_CACHED, mmap_configured: false, mapped: false })
    }

    fn log_refused(&mut self, size: u64) {
        if !self.refused_logged {
            self.refused_logged = true;
            crate::kprintln!("[gpu] native context: a GEM object of {} KB refused ({} MB held)",
                size >> 10, self.gem_bytes >> 20);
        }
    }

    fn object_free(&mut self, o: Object, attached: Option<(u32, u32)>) {
        self.gem_bytes -= o.size;
        gpudev::object_remove(o.id, attached, o.frames);
    }

    /// `drm_context_blob_id_valid`.
    fn blob_id_valid(&self, ctx: u32, blob_id: u32) -> bool {
        blob_id != 0 && self.clients.get(&ctx).is_some_and(|c| !c.parked.contains_key(&blob_id))
    }

    /// `i915_ccmd_gem_create`.
    fn gem_create(&mut self, ctx: u32, req: &[u8]) -> Result<(), i32> {
        let (size, blob_id) = (q64(req, 16), d32(req, 24));
        if !self.blob_id_valid(ctx, blob_id) {
            return Err(-EINVAL);
        }
        let o = self.object_create(size).ok_or(-ENOMEM)?;
        self.park(ctx, blob_id, o);
        Ok(())
    }

    /// `i915_ccmd_gem_create_ext`: the extensions decide the cache type.
    fn gem_create_ext(&mut self, ctx: u32, req: &[u8]) -> Result<(), i32> {
        const HDR: usize = 40;
        let (size, blob_id, ext_size) = (q64(req, 16), d32(req, 24), d32(req, 32) as usize);
        if d32(req, 4) as usize != HDR + ext_size || req.len() < HDR + ext_size {
            return Err(-EINVAL);
        }
        if !self.blob_id_valid(ctx, blob_id) {
            return Err(-EINVAL);
        }
        let payload = &req[HDR..HDR + ext_size];
        let mut ext_size = ext_size as i64;
        let mut ext = 0usize;
        let mut ptr = 0usize;
        let mut wc_mapping = false;
        let mut pat_index = -1i64;
        while ext_size > 0 {
            // `struct i915_user_extension`: next_extension, name, flags, rsvd[4].
            if ext_size < 32 || ext + 32 > payload.len() {
                return Err(-EINVAL);
            }
            match d32(payload, ext + 8) {
                EXT_MEMORY_REGIONS => {
                    // base, pad, num_regions, regions; then the class/instance pairs.
                    ptr += 48;
                    ext_size -= 48;
                    if ext_size >= 0 {
                        let n = d32(payload, ext + 36) as usize;
                        let start = ptr;
                        ptr = ptr.saturating_add(n.saturating_mul(4));
                        ext_size -= n.saturating_mul(4) as i64;
                        if ext_size >= 0 {
                            wc_mapping |= (0..n).any(|i| {
                                let o = start + i * 4;
                                o + 2 <= payload.len() && u16::from_le_bytes([payload[o], payload[o + 1]]) == MEMORY_CLASS_DEVICE
                            });
                        }
                    }
                }
                EXT_PROTECTED_CONTENT => {
                    ext_size -= 40;
                    ptr += 40;
                }
                EXT_SET_PAT => {
                    ext_size -= 40;
                    if ext_size >= 0 && ptr + 36 <= payload.len() {
                        pat_index = d32(payload, ptr + 32) as i64;
                    }
                    ptr += 40;
                }
                _ => return Err(-EINVAL),
            }
            ext = ptr;
        }
        if ext_size != 0 {
            return Err(-EINVAL);
        }
        let mut o = self.object_create(size).ok_or(-ENOMEM)?;
        if pat_index > -1 {
            o.mmap_configured = true;
            o.map_info = if pat_index == 1 { MAP_CACHE_WC } else { MAP_CACHE_CACHED };
        } else if wc_mapping {
            o.mmap_configured = true;
            o.map_info = MAP_CACHE_WC;
        }
        self.park(ctx, blob_id, o);
        Ok(())
    }

    fn park(&mut self, ctx: u32, blob_id: u32, o: Object) {
        match self.clients.get_mut(&ctx) {
            Some(c) => {
                c.parked.insert(blob_id, o);
            }
            None => self.object_free(o, None),
        }
    }

    /// `i915_ccmd_gem_set_mmap_mode`.
    fn gem_set_mmap_mode(&mut self, ctx: u32, req: &[u8]) -> Result<(), i32> {
        let (res_id, flags) = (d32(req, 16), d32(req, 20));
        let Some(o) = self.clients.get_mut(&ctx).and_then(|c| c.objects.get_mut(&res_id)) else {
            return Ok(());
        };
        if !o.mmap_configured && o.mapped {
            o.mmap_configured = true;
            return Ok(());
        }
        let map_info = match flags {
            MMAP_GTT | MMAP_WC | MMAP_FIXED => MAP_CACHE_WC,
            MMAP_WB => MAP_CACHE_CACHED,
            MMAP_UC => MAP_CACHE_UNCACHED,
            _ => return Err(-EINVAL),
        };
        if o.mmap_configured && o.map_info != map_info {
            return Ok(());
        }
        o.mmap_configured = true;
        o.map_info = map_info;
        Ok(())
    }

    /// A ccmd the module runs; a failed or missing answer is EIO in the
    /// reply, as a failed ioctl would be.
    fn forward(&mut self, ctx: u32, cmd: u32, req: &[u8], rsp: &mut Vec<u8>) -> Result<(), i32> {
        if cmd == CCMD_IOCTL_SIMPLE && d32(req, 16) == IOCTL_GEM_CONTEXT_DESTROY {
            let id = d32(req, 24);
            if id < 64 {
                if let Some(c) = self.clients.get_mut(&ctx) {
                    c.ban_clear |= 1 << id;
                }
            }
        }
        match gpudev::ccmd(ctx, req) {
            Answer::Reply(r) => *rsp = r,
            Answer::Malformed => return Err(-EINVAL),
            Answer::Failed => *rsp = i915_error_response(cmd, EIO),
        }
        // `i915_ccmd_gem_execbuffer2`: EIO bans the context.
        if (cmd == CCMD_GEM_EXECBUFFER2 || cmd == CCMD_GEM_EXECBUFFER2_V2) && d32(rsp, 4) as i32 == EIO {
            let id = q64(req, 24);
            if id < 64 {
                if let Some(c) = self.clients.get_mut(&ctx) {
                    c.ban_set |= 1 << id;
                }
            }
        }
        Ok(())
    }
}

impl Backend for GpudevBackend {
    fn capset(&self, out: &mut [u8]) {
        i915_capset(self.dev, out);
    }

    fn create(&mut self) -> Result<u32, i32> {
        let handle = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
        if !gpudev::client_open(handle) {
            return Err(-19);
        }
        self.clients.insert(handle, Client::default());
        Ok(handle)
    }

    fn destroy(&mut self, ctx: u32) {
        gpudev::client_close(ctx);
        let Some(c) = self.clients.get_mut(&ctx) else { return };
        c.closed = true;
        let parked = core::mem::take(&mut c.parked);
        let empty = c.objects.is_empty();
        for (_, o) in parked {
            self.object_free(o, None);
        }
        if empty {
            self.clients.remove(&ctx);
        }
    }

    fn ccmd_alignment(&self) -> u32 {
        4
    }

    fn ccmd(&mut self, ctx: u32, req: &[u8], rsp: &mut Vec<u8>) -> Result<(), i32> {
        if req.len() < 16 || self.clients.get(&ctx).is_none_or(|c| c.closed) {
            return Err(-EINVAL);
        }
        let cmd = d32(req, 0);
        match cmd {
            CCMD_GEM_CREATE => self.gem_create(ctx, req),
            CCMD_GEM_CREATE_EXT => self.gem_create_ext(ctx, req),
            CCMD_GEM_SET_MMAP_MODE => self.gem_set_mmap_mode(ctx, req),
            1..=CCMD_GEM_EXECBUFFER2_V2 => self.forward(ctx, cmd, req, rsp),
            _ => Err(-EINVAL),
        }
    }

    fn take_blob(&mut self, ctx: u32, blob_id: u64, res_id: u32) -> Option<(Arc<ForeignPixels>, u32)> {
        if blob_id >> 32 != 0 {
            return None;
        }
        let c = self.clients.get_mut(&ctx).filter(|c| !c.closed)?;
        let o = c.parked.remove(&(blob_id as u32))?;
        let out = (o.frames.clone(), o.map_info);
        gpudev::object_attach(ctx, res_id, o.id);
        c.objects.insert(res_id, o);
        Some(out)
    }

    fn map_info(&mut self, ctx: u32, res_id: u32) -> Option<u32> {
        let o = self.clients.get_mut(&ctx)?.objects.get_mut(&res_id)?;
        o.mapped = true;
        Some(o.map_info)
    }

    fn banned(&mut self, ctx: u32) -> (u64, u64) {
        let Some(c) = self.clients.get_mut(&ctx) else { return (0, 0) };
        (core::mem::take(&mut c.ban_set), core::mem::take(&mut c.ban_clear))
    }

    fn release_blob(&mut self, ctx: u32, pages: &Arc<ForeignPixels>) {
        let Some(c) = self.clients.get_mut(&ctx) else { return };
        let Some(res_id) = c.objects.iter().find(|(_, o)| o.frames.serial == pages.serial).map(|(r, _)| *r) else {
            return;
        };
        let o = c.objects.remove(&res_id).unwrap();
        let gone = c.closed && c.objects.is_empty();
        if gone {
            self.clients.remove(&ctx);
        }
        self.object_free(o, Some((ctx, res_id)));
    }

    fn submit_fence(&mut self, ctx: u32, ring: u32) -> u64 {
        gpudev::ring_state(ctx, ring).0
    }

    fn fence_done(&mut self, ctx: u32, ring: u32, fence: u64) -> bool {
        gpudev::native_device().is_none() || gpudev::ring_state(ctx, ring).1 >= fence
    }
}

impl Drop for GpudevBackend {
    /// Clients left over close. An object whose resource was never given
    /// back may still be read where it lies, so its frames are kept.
    fn drop(&mut self) {
        let ids: Vec<u32> = self.clients.keys().copied().collect();
        for id in ids {
            self.destroy(id);
        }
        let kept: usize = self.clients.values().map(|c| c.objects.len()).sum();
        if kept > 0 {
            crate::kprintln!("[gpu] native context: {} GEM object(s) still in use, their frames are kept", kept);
        }
    }
}

/// Little-endian fields of a request, zero past its end (a shorter request
/// from an older guest is zero-extended, as `drm_context_submit_cmd_dispatch`
/// does).
fn d32(b: &[u8], at: usize) -> u32 {
    let mut w = [0u8; 4];
    for (i, x) in w.iter_mut().enumerate() {
        *x = b.get(at + i).copied().unwrap_or(0);
    }
    u32::from_le_bytes(w)
}

fn q64(b: &[u8], at: usize) -> u64 {
    d32(b, at) as u64 | (d32(b, at + 4) as u64) << 32
}
