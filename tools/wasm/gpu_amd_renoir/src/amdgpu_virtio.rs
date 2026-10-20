//! Native contexts for guests (docs/plan/GPU_AMD_NATIVE.md): the ccmd
//! handlers of virglrenderer `src/drm/amdgpu/amdgpu_renderer.c`, each
//! calling this driver where virglrenderer calls libdrm and the host's
//! amdgpu, and `amdgpu_renderer_probe`'s capset. A guest context is a
//! client of its own (`amdgpu_driver_open_kms`), as virglrenderer opens a
//! `drm_file` per context.
//!
//! The reply to a ccmd starts with `REPLY_HEAD`: the client's
//! `async_error`, then flags (`REPLY_HANDLER_FAILED`: the handler returned
//! an error, and virglrenderer drops the rest of the SUBMIT_3D); the
//! guest's response follows.

use crate::amdgpu_cs::{self as cs, amdgpu_fpriv};
use crate::amdgpu_device::amdgpu_device;
use crate::linux::Line;
use crate::native_gen::*;
use crate::regs::*;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

const EINVAL: i32 = -22;
const ENOMEM: i32 = -12;

const REPLY_HEAD: usize = 8;
const REPLY_HANDLER_FAILED: u32 = 1;

/// libdrm's `amdgpu_get_marketing_name` for a device `amdgpu.ids` does not
/// list, as none of Renoir's is.
const DEFAULT_MARKETING_NAME: &[u8] = b"AMD Radeon Graphics";

/// `struct amdgpu_object`: a GEM object of a guest context.
struct vobject {
    bo: usize,
    has_metadata: bool,
    enable_cache_wc: bool,
}

/// A guest context.
pub struct vclient {
    /// The kernel's handle of it.
    pub id: u32,
    pub fpriv: amdgpu_fpriv,
    /// `amdvgpu_shmem.async_error`.
    async_error: u32,
    /// `drm_context`'s blob table (objects a GEM_NEW made, until their
    /// resource exists) and its resource table.
    blobs: BTreeMap<u32, vobject>,
    resources: BTreeMap<u32, vobject>,
    /// The timelines (`drm_timeline`), one per ring the guest may name
    /// (`ring_idx` - 1): the fences of its submissions not yet done, and
    /// how many it has had and finished.
    timelines: Vec<timeline>,
}

#[derive(Default)]
struct timeline {
    pending: alloc::collections::VecDeque<crate::dma_fence::Fence>,
    submitted: u64,
    completed: u64,
}

/// A CS_QUERY_FENCE_STATUS waiting for its fence or its timeout: the job
/// stays open meanwhile (`amdgpu_cs_wait_ioctl` blocks only its caller).
pub struct fence_wait {
    pub seq: u32,
    client: u32,
    fence: Option<crate::dma_fence::Fence>,
    deadline_ns: u64,
}

/// What a ccmd came to: a reply now, or a wait.
pub enum ccmd_result {
    reply(Vec<u8>),
    wait(fence_wait),
}

/// Every guest context of the service.
#[derive(Default)]
pub struct vclients {
    clients: BTreeMap<u32, vclient>,
}

/// `OP_CLIENT` open: `amdgpu_renderer_create`'s device, a client of its own.
pub fn client_open(adev: &mut amdgpu_device, vc: &mut vclients, id: u32) -> i32 {
    if vc.clients.contains_key(&id) {
        return EINVAL;
    }
    let fpriv = match cs::amdgpu_driver_open_kms(adev) {
        Ok(f) => f,
        Err(e) => return e,
    };
    // amdgpu_renderer_create: a timeline per available ring of every IP.
    let mut count = 0;
    for ip in 0..AMDGPU_HW_IP_NUM {
        if let Ok(v) = query(adev, &fpriv, AMDGPU_INFO_HW_IP_INFO, SIZEOF_drm_amdgpu_info_hw_ip,
                             &[(UAPI_INFO_QUERY_HW_IP_TYPE, ip), (UAPI_INFO_QUERY_HW_IP_INSTANCE, 0)]) {
            count += d32(&v, UAPI_HWIP_AVAILABLE_RINGS).count_ones();
        }
    }
    if count == 0 {
        Line::new().s("native: no available rings").say();
        cs::amdgpu_driver_postclose_kms(adev, fpriv);
        return EINVAL;
    }
    let timelines = (0..count).map(|_| timeline::default()).collect();
    vc.clients.insert(id, vclient { id, fpriv, async_error: 0, blobs: BTreeMap::new(), resources: BTreeMap::new(),
                                    timelines });
    0
}

/// `OP_CLIENT` close: `amdgpu_renderer_destroy`, its contexts with it.
pub fn client_close(adev: &mut amdgpu_device, vc: &mut vclients, id: u32) -> i32 {
    let Some(mut c) = vc.clients.remove(&id) else { return EINVAL };
    // drm_context_deinit: every object, then the device.
    let objs: Vec<vobject> = core::mem::take(&mut c.blobs).into_values()
        .chain(core::mem::take(&mut c.resources).into_values()).collect();
    for o in objs {
        free_object(adev, &c.fpriv, o);
    }
    cs::amdgpu_driver_postclose_kms(adev, c.fpriv);
    0
}

fn d32(b: &[u8], at: u32) -> u32 {
    let mut w = [0u8; 4];
    for (i, x) in w.iter_mut().enumerate() {
        *x = b.get(at as usize + i).copied().unwrap_or(0);
    }
    u32::from_le_bytes(w)
}

/// A response of `len` bytes: `vdrm_ccmd_rsp.len`, then `ret`.
fn rsp(len: u32, ret: i32) -> Vec<u8> {
    let mut r = vec![0u8; len as usize];
    r[0..4].copy_from_slice(&len.to_le_bytes());
    r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&ret.to_le_bytes());
    r
}

/// `amdgpu_ccmd_query_info`.
fn ccmd_query_info(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<Vec<u8>, i32> {
    let info = &req[QI_INFO as usize..QI_REQ_SIZE as usize];
    let return_size = d32(info, UAPI_INFO_RETURN_SIZE);
    let len = QI_RSP_PAYLOAD.checked_add(return_size).ok_or(EINVAL)?;
    let mut r = rsp(len, 0);
    match crate::amdgpu_kms::amdgpu_info_ioctl(adev, c.fpriv.vm, info) {
        Ok(v) => r[QI_RSP_PAYLOAD as usize..QI_RSP_PAYLOAD as usize + v.len()].copy_from_slice(&v),
        Err(e) => r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&e.to_le_bytes()),
    }
    Ok(r)
}

/// `amdgpu_ccmd_create_ctx`: `amdgpu_cs_ctx_create2` (an unprivileged
/// client) or `amdgpu_cs_ctx_free`.
fn ccmd_create_ctx(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<Vec<u8>, i32> {
    let mut r = rsp(CC_RSP_SIZE, 0);
    let flags = d32(req, CC_FLAGS);
    if flags & !AMDGPU_CCMD_CREATE_CTX_DESTROY != 0 {
        Line::new().s("native: create_ctx: invalid flags 0x").hexu(flags).say();
        r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&EINVAL.to_le_bytes());
        return Err(EINVAL);
    }
    if flags & AMDGPU_CCMD_CREATE_CTX_DESTROY == 0 {
        // amdgpu_ctx_ioctl: garbage priorities are NORMAL.
        let mut priority = d32(req, CC_PRIORITY) as i32;
        if !crate::amdgpu_ctx::amdgpu_ctx_priority_is_valid(priority) {
            priority = crate::amdgpu_ctx::PRIORITY_NORMAL;
        }
        match crate::amdgpu_ctx::amdgpu_ctx_alloc_checked(adev, &mut c.fpriv.ctx_mgr, c.fpriv.vm, priority, false) {
            Ok(id) => r[CC_RSP_CTX_ID as usize..CC_RSP_CTX_ID as usize + 4].copy_from_slice(&id.to_le_bytes()),
            Err(e) => r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&e.to_le_bytes()),
        }
    } else {
        let id = d32(req, CC_PRIORITY);
        if crate::amdgpu_ctx::amdgpu_ctx_free(adev, &mut c.fpriv.ctx_mgr, id) != 0 {
            Line::new().s("native: Failed to find ctx_id: ").dec(id as u64).say();
        }
    }
    Ok(r)
}

/// `amdgpu_renderer_free_object`: `amdgpu_bo_free`, the GEM handle closed.
fn free_object(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, o: vobject) {
    cs::amdgpu_gem_object_close(adev, fpriv, o.bo);
    crate::amdgpu_object::amdgpu_bo_unref(adev, o.bo);
}

fn q64(b: &[u8], at: u32) -> u64 {
    d32(b, at) as u64 | (d32(b, at + 4) as u64) << 32
}

/// `amdgpu_ccmd_gem_new`: libdrm's `amdgpu_bo_alloc`, parked under the
/// blob id until `get_blob`.
fn ccmd_gem_new(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<Vec<u8>, i32> {
    let blob_id = q64(req, GN_BLOB_ID);
    let (size, align, heap, flags) = (q64(req, GN_ALLOC_SIZE), q64(req, GN_PHYS_ALIGNMENT), d32(req, GN_PREFERRED_HEAP),
                                      q64(req, GN_FLAGS));
    let fail = |c: &mut vclient, why: &str, e: i32| {
        Line::new().s("native: gem_new: ").s(why).say();
        c.async_error = c.async_error.wrapping_add(1);
        Err(e)
    };
    if d32(req, GN_PAD) != 0 {
        return fail(c, "padding not zero", EINVAL);
    }
    // drm_context_blob_id_valid
    if blob_id == 0 || blob_id >> 32 != 0 || c.blobs.contains_key(&(blob_id as u32)) {
        return fail(c, "invalid blob id", EINVAL);
    }
    let bo = match cs::amdgpu_gem_create_ioctl(adev, &c.fpriv, c.id, size, align, heap as u64, flags) {
        Ok(h) => h,
        Err(e) => return fail(c, "amdgpu_bo_alloc failed", e),
    };
    // Enable Write-Combine except for GTT buffers with WC disabled.
    let enable_cache_wc = heap != AMDGPU_GEM_DOMAIN_GTT || flags & AMDGPU_GEM_CREATE_CPU_GTT_USWC as u64 != 0;
    c.blobs.insert(blob_id as u32, vobject { bo, has_metadata: false, enable_cache_wc });
    Ok(Vec::new())
}

/// `amdgpu_ccmd_bo_va_op`: libdrm's `amdgpu_bo_va_op_raw`.
fn ccmd_bo_va_op(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<Vec<u8>, i32> {
    let mut r = rsp(RSP_SIZE, 0);
    let flags2 = q64(req, VA_FLAGS2);
    if flags2 & !(AMDGPU_CCMD_BO_VA_OP_SPARSE_BO as u64) != 0 {
        Line::new().s("native: bo_va_op: forbidden flags").say();
        r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&EINVAL.to_le_bytes());
        return Err(EINVAL);
    }
    let bo = if flags2 & AMDGPU_CCMD_BO_VA_OP_SPARSE_BO as u64 != 0 {
        None
    } else {
        // The guest may have closed the GEM already.
        Some(c.resources.get(&d32(req, VA_RES_ID)).ok_or(EINVAL)?.bo)
    };
    let ret = match cs::amdgpu_gem_va_ioctl_op(adev, &c.fpriv, bo, d32(req, VA_OP), q64(req, VA_VA), q64(req, VA_OFFSET),
                                               q64(req, VA_VM_MAP_SIZE), q64(req, VA_FLAGS) as u32) {
        Ok(_) => 0,
        Err(e) => {
            c.async_error = c.async_error.wrapping_add(1);
            Line::new().s("native: amdgpu_bo_va_op_raw failed: op ").dec(d32(req, VA_OP) as u64).s(" r=")
                .dec((-e) as u64).say();
            e
        }
    };
    r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&ret.to_le_bytes());
    Ok(r)
}

/// `amdgpu_ccmd_set_metadata`: libdrm's `amdgpu_bo_set_metadata`.
fn ccmd_set_metadata(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<Vec<u8>, i32> {
    let mut r = rsp(RSP_SIZE, 0);
    let len = d32(req, 4);
    let Some(o) = c.resources.get_mut(&d32(req, SM_RES_ID)) else {
        r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&EINVAL.to_le_bytes());
        return Err(EINVAL);
    };
    let size = d32(req, SM_SIZE_METADATA);
    if size > 256 || (size != 0 && SM_UMD_METADATA as u64 + size as u64 > len as u64) {
        r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&EINVAL.to_le_bytes());
        return Err(EINVAL);
    }
    let data = &req[SM_UMD_METADATA as usize..SM_UMD_METADATA as usize + size as usize];
    let ret = cs::amdgpu_gem_set_metadata(adev, o.bo, q64(req, SM_TILING_INFO), q64(req, SM_FLAGS), data);
    r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&ret.to_le_bytes());
    o.has_metadata = true;
    Ok(r)
}

/// `amdgpu_ccmd_bo_query_info`: libdrm's `amdgpu_bo_query_info`, its
/// GEM_METADATA GET and GEM_OP GET_GEM_CREATE_INFO.
fn ccmd_bo_query_info(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<Vec<u8>, i32> {
    let mut r = rsp(BQ_RSP_SIZE, 0);
    if d32(req, BQ_PAD) != 0 {
        r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&EINVAL.to_le_bytes());
        return Err(EINVAL);
    }
    let Some(o) = c.resources.get(&d32(req, BQ_RES_ID)) else {
        r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&EINVAL.to_le_bytes());
        return Ok(r);
    };
    let b = crate::amdgpu_object::bo(adev, o.bo);
    let put32 = |r: &mut [u8], at: u32, v: u32| r[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
    let put64 = |r: &mut [u8], at: u32, v: u64| r[at as usize..at as usize + 8].copy_from_slice(&v.to_le_bytes());
    put64(&mut r, BQ_ALLOC_SIZE, b.tbo.size);
    put64(&mut r, BQ_PHYS_ALIGNMENT, (b.tbo.page_alignment as u64) << 12);
    put32(&mut r, BQ_PREFERRED_HEAP, b.preferred_domains);
    put64(&mut r, BQ_ALLOC_FLAGS, b.flags);
    put64(&mut r, BQ_MD_FLAGS, b.metadata_flags);
    put64(&mut r, BQ_MD_TILING_INFO, b.tiling_flags);
    put32(&mut r, BQ_MD_SIZE_METADATA, b.metadata.len() as u32);
    let at = BQ_MD_UMD_METADATA as usize;
    r[at..at + b.metadata.len()].copy_from_slice(&b.metadata);
    Ok(r)
}

/// `amdgpu_ccmd_cs_submit`: the chunks checked and translated (resource
/// ids to buffers) as virglrenderer does, then `amdgpu_cs_submit_raw2`;
/// the submission's fence goes on the timeline of `ring_idx`.
fn ccmd_cs_submit(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<Vec<u8>, i32> {
    let mut rsp_ = rsp(RSP_SIZE, 0);
    let len = (d32(req, 4) as usize).min(req.len());
    let (ctx_id, num_chunks, ring_idx) = (d32(req, CS_CTX_ID), d32(req, CS_NUM_CHUNKS), d32(req, CS_RING_IDX));
    let fail = |r: &mut Vec<u8>, e: i32| {
        r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&e.to_le_bytes());
        Err(e)
    };
    // Do not allocate arbitrarily large buffer.
    if num_chunks > AMDGPU_CCMD_CS_SUBMIT_MAX_NUM_CHUNKS {
        return fail(&mut rsp_, EINVAL);
    }
    if ring_idx == 0 || ring_idx as usize > c.timelines.len() {
        Line::new().s("native: invalid ring_idx ").dec(ring_idx as u64).say();
        return fail(&mut rsp_, EINVAL);
    }
    let descriptors_len = CS_PAYLOAD as usize + num_chunks as usize * 8;
    if descriptors_len > len {
        return fail(&mut rsp_, EINVAL);
    }
    let mut chunks = Vec::new();
    let mut have_bo_list = false;
    for i in 0..num_chunks as usize {
        let at = CS_PAYLOAD as usize + i * 8;
        let chunk_id = u16::from_le_bytes([req[at], req[at + 1]]) as u32;
        let length_dw = u16::from_le_bytes([req[at + 2], req[at + 3]]) as usize;
        // size_add: saturating, so a huge offset fails the bound.
        let offset = descriptors_len.saturating_add(d32(req, at as u32 + 4) as usize);
        let clen = length_dw * 4;
        if offset.saturating_add(clen) > len {
            return fail(&mut rsp_, EINVAL);
        }
        // validate_chunk_inputs: the entries of a BO list and a user fence
        // are words.
        if matches!(chunk_id, AMDGPU_CHUNK_ID_BO_HANDLES | AMDGPU_CHUNK_ID_FENCE) && offset % 4 != 0 {
            return fail(&mut rsp_, EINVAL);
        }
        let w = |k: usize| d32(req, (offset + k * 4) as u32);
        match chunk_id {
            AMDGPU_CHUNK_ID_BO_HANDLES => {
                if have_bo_list {
                    Line::new().s("native: refusing to allocate multiple BO lists").say();
                    return fail(&mut rsp_, EINVAL);
                }
                have_bo_list = true;
                let mut list = Vec::new();
                for k in 0..clen / 8 {
                    let res_id = w(k * 2);
                    let Some(o) = c.resources.get(&res_id) else {
                        Line::new().s("native: couldn't retrieve bo with res_id ").dec(res_id as u64).say();
                        return fail(&mut rsp_, EINVAL);
                    };
                    list.push(o.bo);
                }
                chunks.push(cs::cs_chunk::bo_handles(list));
            }
            AMDGPU_CHUNK_ID_FENCE => {
                if clen < 8 {
                    return fail(&mut rsp_, EINVAL);
                }
                let (res_id, off) = (w(0), w(1));
                if off % 8 != 0 {
                    return fail(&mut rsp_, EINVAL);
                }
                let Some(o) = c.resources.get(&res_id) else { return fail(&mut rsp_, EINVAL) };
                chunks.push(cs::cs_chunk::fence { bo: o.bo, offset: off });
            }
            AMDGPU_CHUNK_ID_DEPENDENCIES => {
                let n = clen / SIZEOF_drm_amdgpu_cs_chunk_dep as usize;
                let deps = (0..n).map(|k| {
                    let b = k * 6;
                    cs::cs_dep { ip_type: w(b), ip_instance: w(b + 1), ring: w(b + 2), ctx_id: w(b + 3),
                                 handle: w(b + 4) as u64 | (w(b + 5) as u64) << 32 }
                }).collect();
                chunks.push(cs::cs_chunk::dependencies { deps, scheduled: false });
            }
            AMDGPU_CHUNK_ID_IB => {
                if length_dw != 8 {
                    return fail(&mut rsp_, EINVAL);
                }
                chunks.push(cs::cs_chunk::ib(cs::drm_amdgpu_cs_chunk_ib {
                    flags: w(1),
                    va_start: w(2) as u64 | (w(3) as u64) << 32,
                    ib_bytes: w(4),
                    ip_type: w(5),
                    ip_instance: w(6),
                    ring: w(7),
                }));
            }
            _ => {
                Line::new().s("native: unsupported chunk_id ").dec(chunk_id as u64).say();
                return fail(&mut rsp_, EINVAL);
            }
        }
    }
    match cs::amdgpu_cs_ioctl_chunks(adev, &mut c.fpriv, ctx_id, &chunks) {
        Ok((_, fence)) => {
            let t = &mut c.timelines[ring_idx as usize - 1];
            t.submitted += 1;
            t.pending.push_back(fence);
            npk_sys::gpudev_ring(c.id as i32, ring_idx as i32, t.submitted as i64, t.completed as i64);
            Ok(rsp_)
        }
        Err(e) => {
            c.async_error = c.async_error.wrapping_add(1);
            Line::new().s("native: command submission failed (ring ").dec(ring_idx as u64).s(", ").dec((-e) as u64)
                .s(")").say();
            fail(&mut rsp_, e)
        }
    }
}

/// The timelines of every client: submissions whose fences signaled are
/// done (`drm_timeline` retiring in order), and the kernel hears of it.
pub fn poll_rings(adev: &mut amdgpu_device, vc: &mut vclients) {
    for c in vc.clients.values_mut() {
        for (i, t) in c.timelines.iter_mut().enumerate() {
            let before = t.completed;
            while t.pending.front().is_some_and(|f| crate::dma_fence::dma_fence_is_signaled(adev, f)) {
                t.pending.pop_front();
                t.completed += 1;
            }
            if t.completed != before {
                npk_sys::gpudev_ring(c.id as i32, i as i32 + 1, t.submitted as i64, t.completed as i64);
            }
        }
    }
}

/// `amdgpu_ccmd_cs_query_fence_status`: libdrm's
/// `amdgpu_cs_query_fence_status`, `amdgpu_cs_wait_ioctl` in the driver.
fn ccmd_query_fence_status(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<ccmd_result, i32> {
    let ctx_id = d32(req, QF_CTX_ID);
    let Some(ctx) = c.fpriv.ctx_mgr.ctx_handles.get_mut(&ctx_id) else {
        Line::new().s("native: couldn't find amdgpu_context with id ").dec(ctx_id as u64).say();
        return Err(EINVAL);
    };
    let (ip_type, inst, ring) = (d32(req, QF_IP_TYPE), d32(req, QF_IP_INSTANCE), d32(req, QF_RING));
    let (timeout, flags, seq) = (q64(req, QF_TIMEOUT_NS), q64(req, QF_FLAGS), q64(req, QF_FENCE));
    let mut early = rsp(QF_RSP_SIZE, 0);
    if ip_type >= AMDGPU_HW_IP_NUM || ring >= AMDGPU_CS_MAX_RINGS {
        early[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&EINVAL.to_le_bytes());
        return Ok(ccmd_result::reply(early));
    }
    if seq == AMDGPU_NULL_SUBMIT_SEQ as u64 {
        early[QF_RSP_EXPIRED as usize..QF_RSP_EXPIRED as usize + 4].copy_from_slice(&1u32.to_le_bytes());
        return Ok(ccmd_result::reply(early));
    }
    // amdgpu_cs_get_fence
    let fence = crate::amdgpu_ctx::amdgpu_ctx_get_entity(adev, ctx, ip_type, inst, ring)
        .and_then(|_| crate::amdgpu_ctx::amdgpu_ctx_get_fence(ctx, ip_type, ring, seq));
    let fence = match fence {
        Ok(f) => f,
        Err(e) => {
            let mut r = rsp(QF_RSP_SIZE, 0);
            r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&e.to_le_bytes());
            return Ok(ccmd_result::reply(r));
        }
    };
    // amdgpu_cs_calculate_timeout: a relative timeout from now.
    let now = crate::linux::now_us().saturating_mul(1000);
    let deadline_ns = if flags & AMDGPU_QUERY_FENCE_TIMEOUT_IS_ABSOLUTE as u64 != 0 || timeout == AMDGPU_TIMEOUT_INFINITE {
        timeout
    } else {
        now.saturating_add(timeout)
    };
    Ok(ccmd_result::wait(fence_wait { seq: 0, client: c.id, fence, deadline_ns }))
}

/// Milliseconds until the first of `waits` runs out of time (a fence
/// signaling wakes the module through its interrupt).
pub fn fence_wait_ms(waits: &[fence_wait]) -> i32 {
    let Some(first) = waits.iter().map(|w| w.deadline_ns).min() else { return i32::MAX };
    let now = crate::linux::now_us().saturating_mul(1000);
    (first.saturating_sub(now).div_ceil(1_000_000)).min(i32::MAX as u64) as i32
}

/// A fence wait's reply once its fence signaled or its time is up; None
/// while it still waits.
pub fn fence_wait_reply(adev: &mut amdgpu_device, vc: &vclients, w: &fence_wait) -> Option<Vec<u8>> {
    let async_error = vc.clients.get(&w.client).map_or(0, |c| c.async_error);
    let now = crate::linux::now_us().saturating_mul(1000);
    // dma_fence_wait_timeout: 1 when signaled, 0 when the time is up.
    let (r, error) = match &w.fence {
        None => (1, 0),
        Some(f) if crate::dma_fence::dma_fence_is_signaled(adev, f) => (1, f.error()),
        Some(_) if now >= w.deadline_ns => (0, 0),
        Some(_) => return None,
    };
    let mut body = rsp(QF_RSP_SIZE, 0);
    if r > 0 && error != 0 {
        body[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&error.to_le_bytes());
    } else {
        // out.status = busy; libdrm: expired = !busy
        let expired = (r != 0) as u32;
        body[QF_RSP_EXPIRED as usize..QF_RSP_EXPIRED as usize + 4].copy_from_slice(&expired.to_le_bytes());
    }
    let mut out = Vec::with_capacity(REPLY_HEAD + body.len());
    out.extend_from_slice(&async_error.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

/// `amdgpu_ccmd_reserve_vmid`: `amdgpu_vm_ioctl`'s (un)reserve, from the GFX hub.
fn ccmd_reserve_vmid(adev: &mut amdgpu_device, c: &mut vclient, req: &[u8]) -> Result<Vec<u8>, i32> {
    let mut r = rsp(RSP_SIZE, 0);
    let flags = q64(req, RV_FLAGS);
    if flags & !(AMDGPU_CCMD_RESERVE_VMID_UNRESERVE as u64) != 0 {
        r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&EINVAL.to_le_bytes());
        return Err(EINVAL);
    }
    let hub = AMDGPU_GFXHUB_0 as usize;
    let ret = if flags & AMDGPU_CCMD_RESERVE_VMID_UNRESERVE as u64 != 0 {
        crate::amdgpu_ids::amdgpu_vmid_free_reserved(adev, c.fpriv.vm, hub);
        0
    } else {
        crate::amdgpu_ids::amdgpu_vmid_alloc_reserved(adev, c.fpriv.vm, hub)
    };
    r[RSP_RET as usize..RSP_RET as usize + 4].copy_from_slice(&ret.to_le_bytes());
    Ok(r)
}

/// `OP_BLOB`, `amdgpu_renderer_get_blob`: the object under `blob_id`
/// becomes resource `res_id`. The reply names its pages for the kernel:
/// map info, kind (0: the GTT pages of `tt`, 1: VRAM ranges in BAR0), the
/// tt id, the number of ranges, then (offset, length) pairs.
pub fn get_blob(adev: &mut amdgpu_device, vc: &mut vclients, id: u32, blob_id: u32, res_id: u32) -> Result<Vec<u8>, i32> {
    let c = vc.clients.get_mut(&id).ok_or(EINVAL)?;
    // drm_context_res_id_unused
    if c.resources.contains_key(&res_id) {
        return Err(EINVAL);
    }
    let o = c.blobs.remove(&blob_id).ok_or(-2)?;
    let b = crate::amdgpu_object::bo(adev, o.bo);
    let map_info = if o.enable_cache_wc { VIRTIO_GPU_MAP_CACHE_WC } else { VIRTIO_GPU_MAP_CACHE_CACHED };
    let mut out = Vec::new();
    let mut put32 = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    let res = b.tbo.resource.clone();
    match res.as_ref().map(|r| r.mem_type) {
        Some(t) if t == TTM_PL_TT => {
            let Some((tt, _)) = b.pool.as_ref() else {
                c.blobs.insert(blob_id, o);
                return Err(EINVAL);
            };
            put32(map_info);
            put32(0);
            put32(*tt);
            put32(0);
        }
        Some(t) if t == TTM_PL_VRAM => {
            let size = crate::amdgpu_object::amdgpu_bo_size(adev, o.bo);
            let mut ranges = Vec::new();
            let mut cur = crate::amdgpu_res_cursor::amdgpu_res_first(adev, res.as_ref(), 0, size);
            while cur.remaining != 0 {
                ranges.push((cur.start, cur.size));
                let n = cur.size;
                crate::amdgpu_res_cursor::amdgpu_res_next(adev, res.as_ref(), &mut cur, n);
            }
            put32(map_info);
            put32(1);
            put32(0);
            put32(ranges.len() as u32);
            for (off, len) in ranges {
                out.extend_from_slice(&off.to_le_bytes());
                out.extend_from_slice(&len.to_le_bytes());
            }
        }
        _ => {
            c.blobs.insert(blob_id, o);
            return Err(EINVAL);
        }
    }
    c.resources.insert(res_id, o);
    Ok(out)
}

/// `OP_BLOB_RELEASE`: the guest let go of resource `res_id`
/// (`drm_context_free_object`).
pub fn release_blob(adev: &mut amdgpu_device, vc: &mut vclients, id: u32, res_id: u32) -> i32 {
    let Some(c) = vc.clients.get_mut(&id) else { return EINVAL };
    let Some(o) = c.resources.remove(&res_id) else { return EINVAL };
    free_object(adev, &c.fpriv, o);
    0
}

/// A ccmd this driver does not run yet: the handler fails, as
/// virglrenderer's does for a command it cannot serve.
fn not_ported(cmd: u32) -> Result<Vec<u8>, i32> {
    Line::new().s("native: ccmd ").dec(cmd as u64).s(": not ported yet").say();
    Err(EINVAL)
}

/// One guest ccmd of client `id` (`drm_context_submit_cmd_dispatch`): the
/// reply, `REPLY_HEAD` first. Err only for a request too short for its
/// header.
pub fn ccmd(adev: &mut amdgpu_device, vc: &mut vclients, id: u32, req_in: &[u8]) -> Result<ccmd_result, i32> {
    if req_in.len() < REQ_HDR_SIZE as usize {
        return Err(EINVAL);
    }
    let Some(c) = vc.clients.get_mut(&id) else { return Err(EINVAL) };
    let cmd = d32(req_in, 0);
    // Copy, zero-extended to the size this side knows (a shorter request
    // from an older guest).
    let size = match cmd {
        AMDGPU_CCMD_QUERY_INFO => QI_REQ_SIZE,
        AMDGPU_CCMD_GEM_NEW => GN_REQ_SIZE,
        AMDGPU_CCMD_BO_VA_OP => VA_REQ_SIZE,
        AMDGPU_CCMD_CS_SUBMIT => CS_REQ_SIZE,
        AMDGPU_CCMD_SET_METADATA => SM_REQ_SIZE,
        AMDGPU_CCMD_BO_QUERY_INFO => BQ_REQ_SIZE,
        AMDGPU_CCMD_CREATE_CTX => CC_REQ_SIZE,
        AMDGPU_CCMD_RESERVE_VMID => RV_REQ_SIZE,
        AMDGPU_CCMD_SET_PSTATE => SP_REQ_SIZE,
        AMDGPU_CCMD_CS_QUERY_FENCE_STATUS => QF_REQ_SIZE,
        _ => 0,
    } as usize;
    let mut req = req_in.to_vec();
    if req.len() < size {
        req.resize(size, 0);
    }
    let r = match cmd {
        AMDGPU_CCMD_QUERY_INFO => ccmd_query_info(adev, c, &req),
        AMDGPU_CCMD_CREATE_CTX => ccmd_create_ctx(adev, c, &req),
        AMDGPU_CCMD_GEM_NEW => ccmd_gem_new(adev, c, &req),
        AMDGPU_CCMD_BO_VA_OP => ccmd_bo_va_op(adev, c, &req),
        AMDGPU_CCMD_SET_METADATA => ccmd_set_metadata(adev, c, &req),
        AMDGPU_CCMD_BO_QUERY_INFO => ccmd_bo_query_info(adev, c, &req),
        AMDGPU_CCMD_CS_SUBMIT => ccmd_cs_submit(adev, c, &req),
        AMDGPU_CCMD_RESERVE_VMID => ccmd_reserve_vmid(adev, c, &req),
        AMDGPU_CCMD_CS_QUERY_FENCE_STATUS => match ccmd_query_fence_status(adev, c, &req) {
            Ok(ccmd_result::wait(w)) => return Ok(ccmd_result::wait(w)),
            Ok(ccmd_result::reply(b)) => Ok(b),
            Err(e) => Err(e),
        },
        AMDGPU_CCMD_SET_PSTATE => not_ported(cmd),
        _ => {
            Line::new().s("native: unknown ccmd ").dec(cmd as u64).say();
            Err(EINVAL)
        }
    };
    let (flags, body) = match r {
        Ok(b) => (0, b),
        Err(_) => (REPLY_HANDLER_FAILED, Vec::new()),
    };
    let mut out = Vec::new();
    out.try_reserve_exact(REPLY_HEAD + body.len()).map_err(|_| ENOMEM)?;
    out.extend_from_slice(&c.async_error.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(ccmd_result::reply(out))
}

/// libdrm `amdgpu_query_info` on client `fpriv`.
fn query(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, q: u32, size: u32, extra: &[(u32, u32)])
         -> Result<Vec<u8>, i32> {
    let mut info = vec![0u8; SIZEOF_drm_amdgpu_info as usize];
    let mut put = |at: u32, v: u32| info[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
    put(UAPI_INFO_RETURN_SIZE, size);
    put(UAPI_INFO_QUERY, q);
    for &(at, v) in extra {
        put(at, v);
    }
    let mut v = crate::amdgpu_kms::amdgpu_info_ioctl(adev, fpriv.vm, &info)?;
    v.resize(size as usize, 0);
    Ok(v)
}

/// `amdgpu_renderer_probe` on a client of its own: libdrm's
/// `amdgpu_query_sw_info(address32_hi)`, `amdgpu_query_buffer_size_alignment`,
/// `amdgpu_query_gpu_info` and `amdgpu_get_marketing_name`, as
/// `struct virgl_renderer_capset_drm` with the guest's layout.
pub fn capset(adev: &mut amdgpu_device) -> Result<Vec<u8>, i32> {
    let fpriv = cs::amdgpu_driver_open_kms(adev)?;
    let r = capset_of(adev, &fpriv);
    cs::amdgpu_driver_postclose_kms(adev, fpriv);
    r
}

fn capset_of(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv) -> Result<Vec<u8>, i32> {
    let mut c = vec![0u8; CAPSET_SIZE as usize];
    let put32 = |c: &mut [u8], at: u32, v: u32| c[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
    let put64 = |c: &mut [u8], at: u32, v: u64| c[at as usize..at as usize + 8].copy_from_slice(&v.to_le_bytes());
    // drm_renderer_probe: the drm version and the context type.
    put32(&mut c, CAPSET_VERSION_MAJOR, KMS_DRIVER_MAJOR);
    put32(&mut c, CAPSET_VERSION_MINOR, KMS_DRIVER_MINOR);
    put32(&mut c, CAPSET_VERSION_PATCHLEVEL, KMS_DRIVER_PATCHLEVEL);
    put32(&mut c, CAPSET_CONTEXT_TYPE, VIRTGPU_DRM_CONTEXT_AMDGPU);

    // amdgpu_query_gpu_info_init
    let di = query(adev, fpriv, AMDGPU_INFO_DEV_INFO, SIZEOF_drm_amdgpu_info_device, &[])?;
    let dev32 = |at: u32| d32(&di, at);
    let dev64 = |at: u32| d32(&di, at) as u64 | (d32(&di, at + 4) as u64) << 32;
    let gi = CAPSET_AMDGPU_GPU_INFO;
    put32(&mut c, gi + GI_ASIC_ID, dev32(UAPI_DEV_DEVICE_ID));
    put32(&mut c, gi + GI_CHIP_REV, dev32(UAPI_DEV_CHIP_REV));
    put32(&mut c, gi + GI_CHIP_EXTERNAL_REV, dev32(UAPI_DEV_EXTERNAL_REV));
    put32(&mut c, gi + GI_FAMILY_ID, dev32(UAPI_DEV_FAMILY));
    put64(&mut c, gi + GI_MAX_ENGINE_CLK, dev64(UAPI_DEV_MAX_ENGINE_CLOCK));
    put64(&mut c, gi + GI_MAX_MEMORY_CLK, dev64(UAPI_DEV_MAX_MEMORY_CLOCK));
    put32(&mut c, gi + GI_GPU_COUNTER_FREQ, dev32(UAPI_DEV_GPU_COUNTER_FREQ));
    put32(&mut c, gi + GI_ENABLED_RB_PIPES_MASK, dev32(UAPI_DEV_ENABLED_RB_PIPES_MASK));
    put32(&mut c, gi + GI_RB_PIPES, dev32(UAPI_DEV_NUM_RB_PIPES));
    put64(&mut c, gi + GI_IDS_FLAGS, dev64(UAPI_DEV_IDS_FLAGS));
    put32(&mut c, gi + GI_NUM_HW_GFX_CONTEXTS, dev32(UAPI_DEV_NUM_HW_GFX_CONTEXTS));
    put32(&mut c, gi + GI_NUM_SHADER_ENGINES, dev32(UAPI_DEV_NUM_SHADER_ENGINES));
    put32(&mut c, gi + GI_NUM_SHADER_ARRAYS_PER_ENGINE, dev32(UAPI_DEV_NUM_SHADER_ARRAYS_PER_ENGINE));
    put32(&mut c, gi + GI_VRAM_TYPE, dev32(UAPI_DEV_VRAM_TYPE));
    put32(&mut c, gi + GI_VRAM_BIT_WIDTH, dev32(UAPI_DEV_VRAM_BIT_WIDTH));
    put32(&mut c, gi + GI_CE_RAM_SIZE, dev32(UAPI_DEV_CE_RAM_SIZE));
    put32(&mut c, gi + GI_VCE_HARVEST_CONFIG, dev32(UAPI_DEV_VCE_HARVEST_CONFIG));
    put32(&mut c, gi + GI_PCI_REV_ID, dev32(UAPI_DEV_PCI_REV));
    if dev32(UAPI_DEV_FAMILY) < AMDGPU_FAMILY_AI {
        // The SI to VI register reads; Renoir is AI and later.
        return Err(EINVAL);
    }
    // amdgpu_read_mm_registers(dev, 0x263e, 1, 0xffffffff, 0): GB_ADDR_CONFIG.
    let gb = query(adev, fpriv, AMDGPU_INFO_READ_MMR_REG, 4,
                   &[(UAPI_INFO_READ_MMR_DWORD_OFFSET, 0x263e), (UAPI_INFO_READ_MMR_COUNT, 1),
                     (UAPI_INFO_READ_MMR_INSTANCE, 0xffffffff), (UAPI_INFO_READ_MMR_FLAGS, 0)])?;
    put32(&mut c, gi + GI_GB_ADDR_CFG, d32(&gb, 0));
    put32(&mut c, gi + GI_CU_ACTIVE_NUMBER, dev32(UAPI_DEV_CU_ACTIVE_NUMBER));
    put32(&mut c, gi + GI_CU_AO_MASK, dev32(UAPI_DEV_CU_AO_MASK));
    for i in 0..16 {
        put32(&mut c, gi + GI_CU_BITMAP + i * 4, dev32(UAPI_DEV_CU_BITMAP + i * 4));
    }

    // amdgpu_query_buffer_size_alignment
    put64(&mut c, CAPSET_AMDGPU_ALIGNMENTS + ALIGN_SIZE_LOCAL, dev32(UAPI_DEV_PTE_FRAGMENT_SIZE) as u64);
    put64(&mut c, CAPSET_AMDGPU_ALIGNMENTS + ALIGN_SIZE_REMOTE, dev32(UAPI_DEV_GART_PAGE_SIZE) as u64);

    // amdgpu_query_sw_info(amdgpu_sw_info_address32_hi), with the ranges
    // amdgpu_va_manager_init2 gives vamgr_32 and vamgr_high_32.
    let low_max = dev64(UAPI_DEV_VIRTUAL_ADDRESS_MAX);
    let (high_off, high_max) = (dev64(UAPI_DEV_HIGH_VA_OFFSET), dev64(UAPI_DEV_HIGH_VA_MAX));
    let high_32_max = high_max.min((high_off & !0xffff_ffff) + 0x1_0000_0000);
    let va_max = if high_32_max != 0 { high_32_max } else { low_max.min(0x1_0000_0000) };
    put32(&mut c, CAPSET_AMDGPU_ADDRESS32_HI, ((va_max - 1) >> 32) as u32);
    // has_vm_always_valid: the bit field after address32_hi.
    put32(&mut c, CAPSET_AMDGPU_ADDRESS32_HI + 4, 1);

    let n = DEFAULT_MARKETING_NAME.len().min(CAPSET_AMDGPU_MARKETING_NAME_LEN as usize - 1);
    c[CAPSET_AMDGPU_MARKETING_NAME as usize..CAPSET_AMDGPU_MARKETING_NAME as usize + n]
        .copy_from_slice(&DEFAULT_MARKETING_NAME[..n]);
    Ok(c)
}
