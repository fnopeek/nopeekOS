//! The amdgpu backend of native contexts (docs/plan/GPU_AMD_NATIVE.md): the
//! host side of virglrenderer `src/drm/amdgpu/amdgpu_renderer.c` with the
//! GPU driver module in the place of the host's amdgpu. The ccmd handlers
//! run in the module, which also allocates the GEM objects (with their VRAM
//! and GTT domains, as `amdgpu_gem_create_ioctl` does). Its reply to a ccmd
//! starts with `REPLY_HEAD`: the client's `async_error` and whether the
//! handler failed (virglrenderer then drops the rest of the SUBMIT_3D),
//! then the guest's response. This side writes the shmem header
//! (`async_error`, the heaps as `update_heap_info_in_shmem` does) and keeps
//! the fences per ring.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::virtio_gpu_ctx::{Backend, CONTEXT_AMDGPU};
use crate::drivers::pci::PciAddr;
use crate::gpu::gpudev::{self, Answer};
use crate::shade::buffers::ForeignPixels;

const EINVAL: i32 = 22;
const EIO: i32 = 5;

/// Mesa `amdgpu_virtio_proto.h` ccmd numbers.
const CCMD_QUERY_INFO: u32 = 1;
const CCMD_BO_VA_OP: u32 = 3;
const CCMD_CS_SUBMIT: u32 = 4;
const CCMD_SET_METADATA: u32 = 5;
const CCMD_BO_QUERY_INFO: u32 = 6;
const CCMD_CREATE_CTX: u32 = 7;
const CCMD_RESERVE_VMID: u32 = 8;
const CCMD_SET_PSTATE: u32 = 9;
const CCMD_CS_QUERY_FENCE_STATUS: u32 = 10;

/// `struct amdgpu_ccmd_bo_query_info_rsp` and the other fixed responses.
const RSP_BO_QUERY_INFO: usize = 320;
const RSP_WITH_WORD: usize = 16;
const RSP_PLAIN: usize = 8;
/// `struct amdgpu_ccmd_query_info_req`: `drm_amdgpu_info.return_size` and
/// `.query` behind the 16-byte header.
const QUERY_RETURN_SIZE: usize = 24;
const QUERY_ID: usize = 28;
const QUERY_REQ: usize = 48;
/// Largest `return_size` a query may ask for (the response must fit a reply).
const MAX_QUERY_RETURN: u32 = (gpudev::MAX_REPLY - RSP_PLAIN) as u32;

/// `struct amdvgpu_shmem`: `vdrm_shmem` (seqno, rsp_mem_offset),
/// `async_error`, a pad word, then `amdgpu_heap_info` (heap_size,
/// heap_usage, max_allocation) for GTT, VRAM and CPU-visible VRAM.
const SHMEM_ASYNC_ERROR: usize = 8;
const SHMEM_GTT: usize = 16;
const SHMEM_VRAM: usize = 40;
const SHMEM_VIS_VRAM: usize = 64;
const SHMEM_SIZE: usize = 88;

/// `AMDGPU_INFO_*` the heaps come from (`amdgpu_query_heap_info`).
const INFO_VRAM_USAGE: u32 = 0x10;
const INFO_GTT_USAGE: u32 = 0x11;
const INFO_VRAM_GTT: u32 = 0x14;
const INFO_VIS_VRAM_USAGE: u32 = 0x17;

/// Client handles are unique for the boot, so a late report about a closed
/// client never meets a new one.
static NEXT_CLIENT: AtomicU32 = AtomicU32::new(0x8000_0001);

/// The module's reply head: `async_error`, then flags.
const REPLY_HEAD: usize = 8;
/// The handler returned an error (`drm_context_submit_cmd_dispatch`).
const REPLY_HANDLER_FAILED: u32 = 1;

#[derive(Default)]
struct Client {
    /// `amdvgpu_shmem.async_error` as the module last reported it.
    async_error: u32,
    closed: bool,
}

pub struct AmdgpuBackend {
    capset: Vec<u8>,
    /// Resources the module handed over, by the serial of their pages:
    /// the client and the resource id (`amdgpu_object.base.res_id`).
    blobs: BTreeMap<u64, (u32, u32)>,
    clients: BTreeMap<u32, Client>,
    /// Resources of other contexts attached to a context: (client, res_id).
    imports: alloc::collections::BTreeSet<(u32, u32)>,
    /// Lines `ccmd_failed` has written for this VM.
    failed_lines: u32,
}

impl AmdgpuBackend {
    /// For the card at `dev`, with the capset its module reported.
    pub fn new(_dev: PciAddr, capset: Vec<u8>) -> Option<Self> {
        let ctx_type = u32::from_le_bytes(capset.get(16..20)?.try_into().ok()?);
        (ctx_type == CONTEXT_AMDGPU).then_some(AmdgpuBackend {
            capset, blobs: BTreeMap::new(), clients: BTreeMap::new(), imports: Default::default(), failed_lines: 0,
        })
    }

    /// `drm_err` of a failed ccmd, with why; a fixed number of lines per VM.
    fn ccmd_failed(&mut self, cmd: u32, why: &str, e: i32) -> i32 {
        const MAX_LINES: u32 = 16;
        if self.failed_lines < MAX_LINES {
            self.failed_lines += 1;
            crate::kprintln!("[gpu] native context (amdgpu): ccmd {} failed ({}): {}", cmd, e, why);
        }
        e
    }

    /// One `DRM_AMDGPU_INFO` of this side's own (`amdgpu_query_info`), as
    /// a QUERY_INFO ccmd to the module; the returned bytes.
    fn query_info(&mut self, ctx: u32, query: u32, size: u32) -> Option<Vec<u8>> {
        let mut req = alloc::vec![0u8; QUERY_REQ];
        req[0..4].copy_from_slice(&CCMD_QUERY_INFO.to_le_bytes());
        req[4..8].copy_from_slice(&(QUERY_REQ as u32).to_le_bytes());
        req[QUERY_RETURN_SIZE..QUERY_RETURN_SIZE + 4].copy_from_slice(&size.to_le_bytes());
        req[QUERY_ID..QUERY_ID + 4].copy_from_slice(&query.to_le_bytes());
        let Answer::Reply(r) = gpudev::ccmd(ctx, &req, gpudev::submit_deadline()) else { return None };
        let rsp = r.get(REPLY_HEAD..)?;
        (d32(&r, 4) == 0 && rsp.len() >= RSP_PLAIN + size as usize && d32(rsp, 4) == 0)
            .then(|| rsp[RSP_PLAIN..RSP_PLAIN + size as usize].to_vec())
    }

    /// `update_heap_info_in_shmem`: libdrm's `amdgpu_query_heap_info` for
    /// CPU-visible VRAM, VRAM and GTT.
    fn heap_words(&mut self, ctx: u32) -> Vec<(usize, u32)> {
        let mut out = Vec::new();
        let Some(vg) = self.query_info(ctx, INFO_VRAM_GTT, 24) else { return out };
        // `drm_amdgpu_info_vram_gtt`: vram_size, vram_cpu_accessible_size, gtt_size.
        let (vram, vis, gtt) = (q64(&vg, 0), q64(&vg, 8), q64(&vg, 16));
        let usage = |s: &mut Self, q| s.query_info(ctx, q, 8).map(|b| q64(&b, 0));
        let heaps = [
            (SHMEM_VIS_VRAM, vis, usage(self, INFO_VIS_VRAM_USAGE)),
            (SHMEM_VRAM, vram, usage(self, INFO_VRAM_USAGE)),
            (SHMEM_GTT, gtt, usage(self, INFO_GTT_USAGE)),
        ];
        for (at, size, used) in heaps {
            let Some(used) = used else { continue };
            for (i, v) in [size, used, vis].into_iter().enumerate() {
                out.push((at + i * 8, v as u32));
                out.push((at + i * 8 + 4, (v >> 32) as u32));
            }
        }
        out
    }

    /// The response of ccmd `cmd` whose driver call failed with `-errno`,
    /// in the layout of its `amdgpu_ccmd_*_rsp` (`ret` after the length).
    fn error_response(cmd: u32, req: &[u8], errno: i32) -> Vec<u8> {
        let len = match cmd {
            CCMD_QUERY_INFO => RSP_PLAIN + (d32(req, QUERY_RETURN_SIZE).min(MAX_QUERY_RETURN) as usize),
            CCMD_BO_QUERY_INFO => RSP_BO_QUERY_INFO,
            CCMD_CREATE_CTX | CCMD_SET_PSTATE | CCMD_CS_QUERY_FENCE_STATUS => RSP_WITH_WORD,
            CCMD_BO_VA_OP | CCMD_CS_SUBMIT | CCMD_SET_METADATA | CCMD_RESERVE_VMID => RSP_PLAIN,
            _ => return Vec::new(),
        };
        let mut rsp = alloc::vec![0u8; len];
        rsp[..4].copy_from_slice(&(len as u32).to_le_bytes());
        rsp[4..8].copy_from_slice(&(-errno).to_le_bytes());
        rsp
    }
}

impl Backend for AmdgpuBackend {
    fn capset(&self, out: &mut [u8]) {
        let n = self.capset.len().min(out.len());
        out[..n].copy_from_slice(&self.capset[..n]);
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
        if let Some(c) = self.clients.get_mut(&ctx) {
            c.closed = true;
        }
        self.clients.remove(&ctx);
        self.imports.retain(|&(c, _)| c != ctx);
    }

    fn ccmd_alignment(&self) -> u32 {
        8
    }

    fn rsp_mem_offset(&self) -> usize {
        SHMEM_SIZE
    }

    fn shmem_update(&mut self, ctx: u32, heap: bool) -> Vec<(usize, u32)> {
        let mut out = alloc::vec![(SHMEM_ASYNC_ERROR, self.clients.get(&ctx).map_or(0, |c| c.async_error))];
        if heap {
            out.extend(self.heap_words(ctx));
        }
        out
    }

    fn ccmd(&mut self, ctx: u32, req: &[u8], rsp: &mut Vec<u8>, deadline: u64) -> Result<(), i32> {
        if req.len() < 16 || self.clients.get(&ctx).is_none_or(|c| c.closed) {
            return Err(-EINVAL);
        }
        let cmd = d32(req, 0);
        if !(CCMD_QUERY_INFO..=CCMD_CS_QUERY_FENCE_STATUS).contains(&cmd) {
            return Err(self.ccmd_failed(cmd, "unknown command", -EINVAL));
        }
        if cmd == CCMD_QUERY_INFO && d32(req, QUERY_RETURN_SIZE) > MAX_QUERY_RETURN {
            return Err(self.ccmd_failed(cmd, "query larger than a reply", -EINVAL));
        }
        let reply = match gpudev::ccmd(ctx, req, deadline) {
            Answer::Reply(r) => r,
            Answer::Malformed => return Err(self.ccmd_failed(cmd, "malformed", -EINVAL)),
            Answer::Failed => {
                *rsp = Self::error_response(cmd, req, EIO);
                return Ok(());
            }
        };
        if reply.len() < REPLY_HEAD {
            return Err(self.ccmd_failed(cmd, "reply without its head", -EINVAL));
        }
        if let Some(c) = self.clients.get_mut(&ctx) {
            c.async_error = d32(&reply, 0);
        }
        if d32(&reply, 4) & REPLY_HANDLER_FAILED != 0 {
            return Err(self.ccmd_failed(cmd, "handler failed", -EINVAL));
        }
        *rsp = reply[REPLY_HEAD..].to_vec();
        Ok(())
    }

    /// `amdgpu_renderer_get_blob`: the module checks the blob id and that
    /// the object is exported once, and names its pages.
    fn take_blob(&mut self, ctx: u32, blob_id: u64, res_id: u32) -> Option<(Arc<ForeignPixels>, u32)> {
        if blob_id >> 32 != 0 || self.clients.get(&ctx).is_none_or(|c| c.closed) {
            return None;
        }
        let (pages, map_info) = gpudev::blob(ctx, blob_id as u32, res_id)?;
        self.blobs.insert(pages.serial, (ctx, res_id));
        Some((pages, map_info))
    }

    /// `drm_context_free_object` → `amdgpu_renderer_free_object`.
    fn release_blob(&mut self, _ctx: u32, pages: &Arc<ForeignPixels>) {
        if let Some((ctx, res_id)) = self.blobs.remove(&pages.serial) {
            gpudev::blob_release(ctx, res_id, pages);
        }
    }

    /// `amdgpu_renderer_attach_resource`: the owner's object, imported into
    /// `ctx` under the same resource id.
    fn attach_resource(&mut self, ctx: u32, res_id: u32, pages: &Arc<ForeignPixels>) {
        if self.imports.contains(&(ctx, res_id)) || self.clients.get(&ctx).is_none_or(|c| c.closed) {
            return;
        }
        let Some(&(owner, owner_res)) = self.blobs.get(&pages.serial) else { return };
        if owner == ctx {
            return;
        }
        gpudev::blob_attach(ctx, res_id, owner, owner_res, true);
        self.imports.insert((ctx, res_id));
    }

    /// `drm_context_detach_resource` → `amdgpu_renderer_free_object`.
    fn detach_resource(&mut self, ctx: u32, res_id: u32) {
        if self.imports.remove(&(ctx, res_id)) {
            gpudev::blob_attach(ctx, res_id, 0, 0, false);
        }
    }

    fn device_memory(&self, pages: &ForeignPixels) -> bool {
        self.blobs.contains_key(&pages.serial) && gpudev::blob_in_vram(pages)
    }

    fn submit_fence(&mut self, ctx: u32, ring: u32) -> u64 {
        gpudev::ring_state(ctx, ring).0
    }

    fn fence_done(&mut self, ctx: u32, ring: u32, fence: u64) -> bool {
        gpudev::native_device().is_none() || gpudev::ring_state(ctx, ring).1 >= fence
    }

    /// A scene that reads a guest frame waits, by implicit sync in the
    /// module, for every submission on its object; one that waits behind a
    /// long guest frame misses its deadline. A Linux compositor latches a
    /// client's buffer only once its fences signaled.
    fn open_work(&mut self, ctx: u32) -> Vec<(u32, u64)> {
        if gpudev::native_device().is_none() {
            return Vec::new();
        }
        gpudev::open_rings(ctx)
    }
}

impl Drop for AmdgpuBackend {
    fn drop(&mut self) {
        let ids: Vec<u32> = self.clients.keys().copied().collect();
        for id in ids {
            self.destroy(id);
        }
    }
}

/// Little-endian fields of a request, zero past its end.
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
