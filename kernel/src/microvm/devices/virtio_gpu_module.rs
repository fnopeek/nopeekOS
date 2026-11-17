//! The native-context backend whose ccmds the GPU driver module runs: the
//! host side of virglrenderer `src/drm/amdgpu/amdgpu_renderer.c` with the
//! module in the place of the host's amdgpu, and of the V3D protocol
//! (`docs/spec/V3D_NATIVE_CONTEXT.md`) with the Pi's module. The module
//! allocates the GEM objects. Its reply to a ccmd starts with
//! `REPLY_HEAD`: the client's `async_error` and whether the handler failed
//! (virglrenderer then drops the rest of the SUBMIT_3D), then the guest's
//! response. This side writes the shmem header (`async_error`; for amdgpu
//! the heaps, as `update_heap_info_in_shmem` does) and keeps the fences
//! per ring. What differs between the protocols is a `Proto`.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::virtio_gpu_ctx::{Backend, Timelines};
#[cfg(target_arch = "x86_64")]
use super::virtio_gpu_ctx::CONTEXT_AMDGPU;
#[cfg(target_arch = "aarch64")]
use super::virtio_gpu_ctx::CONTEXT_V3D;
use crate::gpu::gpudev::{self, Answer};
use crate::shade::buffers::ForeignPixels;

const EINVAL: i32 = 22;
const EIO: i32 = 5;

/// What sets one card's protocol apart.
pub struct Proto {
    pub name: &'static str,
    /// `context_type` of the capset.
    context: u32,
    /// ccmds are numbered 1 to this.
    last_ccmd: u32,
    /// `vdrm_shmem.rsp_mem_offset`: the size of the shmem header.
    rsp_mem_offset: usize,
    /// Where the header holds `async_error`.
    async_error: usize,
    /// The response of ccmd `cmd` whose driver call failed with `-errno`;
    /// empty for a ccmd without one.
    error_response: fn(cmd: u32, req: &[u8], errno: i32) -> Vec<u8>,
    /// A request refused before the module sees it, and why.
    refuse: fn(cmd: u32, req: &[u8]) -> Option<&'static str>,
    /// The header carries amdgpu's heaps.
    heaps: bool,
}

#[cfg(target_arch = "x86_64")]
pub static AMDGPU: Proto = Proto {
    name: "amdgpu",
    context: CONTEXT_AMDGPU,
    last_ccmd: CCMD_CS_QUERY_FENCE_STATUS,
    rsp_mem_offset: SHMEM_SIZE,
    async_error: SHMEM_ASYNC_ERROR,
    error_response: amdgpu_error_response,
    refuse: amdgpu_refuse,
    heaps: true,
};

#[cfg(target_arch = "aarch64")]
pub static V3D: Proto = Proto {
    name: "v3d",
    context: CONTEXT_V3D,
    last_ccmd: V3D_CCMD_GET_PARAM,
    rsp_mem_offset: V3D_SHMEM_SIZE,
    async_error: SHMEM_ASYNC_ERROR,
    error_response: v3d_error_response,
    refuse: |_, _| None,
    heaps: false,
};

/// `docs/spec/V3D_NATIVE_CONTEXT.md`: the last ccmd, the responses, and
/// the shmem header (`vdrm_shmem`, `async_error`, a pad word).
#[cfg(target_arch = "aarch64")]
const V3D_CCMD_GET_BO_OFFSET: u32 = 3;
#[cfg(target_arch = "aarch64")]
const V3D_CCMD_WAIT_BO: u32 = 4;
#[cfg(target_arch = "aarch64")]
const V3D_CCMD_GET_PARAM: u32 = 8;
#[cfg(target_arch = "aarch64")]
const V3D_SHMEM_SIZE: usize = 16;

#[cfg(target_arch = "aarch64")]
fn v3d_error_response(cmd: u32, _req: &[u8], errno: i32) -> Vec<u8> {
    let len = match cmd {
        V3D_CCMD_GET_BO_OFFSET | V3D_CCMD_GET_PARAM => 16,
        V3D_CCMD_WAIT_BO => 8,
        _ => return Vec::new(),
    };
    response(len, errno)
}

fn response(len: usize, errno: i32) -> Vec<u8> {
    let mut rsp = alloc::vec![0u8; len];
    rsp[..4].copy_from_slice(&(len as u32).to_le_bytes());
    rsp[4..8].copy_from_slice(&(-errno).to_le_bytes());
    rsp
}

/// Mesa `amdgpu_virtio_proto.h` ccmd numbers.
const CCMD_QUERY_INFO: u32 = 1;
#[cfg(target_arch = "x86_64")]
const CCMD_BO_VA_OP: u32 = 3;
#[cfg(target_arch = "x86_64")]
const CCMD_CS_SUBMIT: u32 = 4;
#[cfg(target_arch = "x86_64")]
const CCMD_SET_METADATA: u32 = 5;
#[cfg(target_arch = "x86_64")]
const CCMD_BO_QUERY_INFO: u32 = 6;
#[cfg(target_arch = "x86_64")]
const CCMD_CREATE_CTX: u32 = 7;
#[cfg(target_arch = "x86_64")]
const CCMD_RESERVE_VMID: u32 = 8;
#[cfg(target_arch = "x86_64")]
const CCMD_SET_PSTATE: u32 = 9;
#[cfg(target_arch = "x86_64")]
const CCMD_CS_QUERY_FENCE_STATUS: u32 = 10;

/// `struct amdgpu_ccmd_bo_query_info_rsp` and the other fixed responses.
#[cfg(target_arch = "x86_64")]
const RSP_BO_QUERY_INFO: usize = 320;
#[cfg(target_arch = "x86_64")]
const RSP_WITH_WORD: usize = 16;
const RSP_PLAIN: usize = 8;
/// `struct amdgpu_ccmd_query_info_req`: `drm_amdgpu_info.return_size` and
/// `.query` behind the 16-byte header.
const QUERY_RETURN_SIZE: usize = 24;
const QUERY_ID: usize = 28;
const QUERY_REQ: usize = 48;
/// Largest `return_size` a query may ask for (the response must fit a reply).
#[cfg(target_arch = "x86_64")]
const MAX_QUERY_RETURN: u32 = (gpudev::MAX_REPLY - RSP_PLAIN) as u32;

/// `struct amdvgpu_shmem`: `vdrm_shmem` (seqno, rsp_mem_offset),
/// `async_error`, a pad word, then `amdgpu_heap_info` (heap_size,
/// heap_usage, max_allocation) for GTT, VRAM and CPU-visible VRAM.
const SHMEM_ASYNC_ERROR: usize = 8;
const SHMEM_GTT: usize = 16;
const SHMEM_VRAM: usize = 40;
const SHMEM_VIS_VRAM: usize = 64;
#[cfg(target_arch = "x86_64")]
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

pub struct ModuleBackend {
    proto: &'static Proto,
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

impl ModuleBackend {
    /// With the capset the module reported, when it is `proto`'s.
    pub fn new(proto: &'static Proto, capset: Vec<u8>) -> Option<Self> {
        let ctx_type = u32::from_le_bytes(capset.get(16..20)?.try_into().ok()?);
        (ctx_type == proto.context).then_some(ModuleBackend {
            proto, capset, blobs: BTreeMap::new(), clients: BTreeMap::new(), imports: Default::default(),
            failed_lines: 0,
        })
    }

    /// `drm_err` of a failed ccmd, with why; a fixed number of lines per VM.
    fn ccmd_failed(&mut self, cmd: u32, why: &str, e: i32) -> i32 {
        const MAX_LINES: u32 = 16;
        if self.failed_lines < MAX_LINES {
            self.failed_lines += 1;
            crate::kprintln!("[gpu] native context ({}): ccmd {} failed ({}): {}", self.proto.name, cmd, e, why);
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

}

/// The response of amdgpu ccmd `cmd` whose driver call failed with
/// `-errno`, in the layout of its `amdgpu_ccmd_*_rsp` (`ret` after the
/// length).
#[cfg(target_arch = "x86_64")]
fn amdgpu_error_response(cmd: u32, req: &[u8], errno: i32) -> Vec<u8> {
    let len = match cmd {
        CCMD_QUERY_INFO => RSP_PLAIN + (d32(req, QUERY_RETURN_SIZE).min(MAX_QUERY_RETURN) as usize),
        CCMD_BO_QUERY_INFO => RSP_BO_QUERY_INFO,
        CCMD_CREATE_CTX | CCMD_SET_PSTATE | CCMD_CS_QUERY_FENCE_STATUS => RSP_WITH_WORD,
        CCMD_BO_VA_OP | CCMD_CS_SUBMIT | CCMD_SET_METADATA | CCMD_RESERVE_VMID => RSP_PLAIN,
        _ => return Vec::new(),
    };
    response(len, errno)
}

#[cfg(target_arch = "x86_64")]
fn amdgpu_refuse(cmd: u32, req: &[u8]) -> Option<&'static str> {
    (cmd == CCMD_QUERY_INFO && d32(req, QUERY_RETURN_SIZE) > MAX_QUERY_RETURN).then_some("query larger than a reply")
}

impl Backend for ModuleBackend {
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
        self.proto.rsp_mem_offset
    }

    fn shmem_update(&mut self, ctx: u32, heap: bool) -> Vec<(usize, u32)> {
        let mut out = alloc::vec![(self.proto.async_error, self.clients.get(&ctx).map_or(0, |c| c.async_error))];
        if heap && self.proto.heaps {
            out.extend(self.heap_words(ctx));
        }
        out
    }

    fn ccmd(&mut self, ctx: u32, req: &[u8], rsp: &mut Vec<u8>, deadline: u64) -> Result<(), i32> {
        if req.len() < 16 || self.clients.get(&ctx).is_none_or(|c| c.closed) {
            return Err(-EINVAL);
        }
        let cmd = d32(req, 0);
        if !(1..=self.proto.last_ccmd).contains(&cmd) {
            return Err(self.ccmd_failed(cmd, "unknown command", -EINVAL));
        }
        if let Some(why) = (self.proto.refuse)(cmd, req) {
            return Err(self.ccmd_failed(cmd, why, -EINVAL));
        }
        let reply = match gpudev::ccmd(ctx, req, deadline) {
            Answer::Reply(r) => r,
            Answer::Malformed => return Err(self.ccmd_failed(cmd, "malformed", -EINVAL)),
            Answer::Failed => {
                *rsp = (self.proto.error_response)(cmd, req, EIO);
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

    /// A scene that reads a guest frame waits, by implicit sync in the
    /// module, for every submission on its object; one that waits behind a
    /// long guest frame misses its deadline. A Linux compositor latches a
    /// client's buffer only once its fences signaled.
    fn timelines(&self) -> Timelines {
        Timelines { done: gpudev::fence_done, open: gpudev::open_work }
    }
}

impl Drop for ModuleBackend {
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
