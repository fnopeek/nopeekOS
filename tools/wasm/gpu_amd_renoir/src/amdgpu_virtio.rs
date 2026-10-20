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

/// A guest context.
pub struct vclient {
    /// The kernel's handle of it.
    pub id: u32,
    pub fpriv: amdgpu_fpriv,
    /// `amdvgpu_shmem.async_error`.
    async_error: u32,
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
    match cs::amdgpu_driver_open_kms(adev) {
        Ok(fpriv) => {
            vc.clients.insert(id, vclient { id, fpriv, async_error: 0 });
            0
        }
        Err(e) => e,
    }
}

/// `OP_CLIENT` close: `amdgpu_renderer_destroy`, its contexts with it.
pub fn client_close(adev: &mut amdgpu_device, vc: &mut vclients, id: u32) -> i32 {
    let Some(c) = vc.clients.remove(&id) else { return EINVAL };
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

/// A ccmd this driver does not run yet: the handler fails, as
/// virglrenderer's does for a command it cannot serve.
fn not_ported(cmd: u32) -> Result<Vec<u8>, i32> {
    Line::new().s("native: ccmd ").dec(cmd as u64).s(": not ported yet").say();
    Err(EINVAL)
}

/// One guest ccmd of client `id` (`drm_context_submit_cmd_dispatch`): the
/// reply, `REPLY_HEAD` first. Err only for a request too short for its
/// header.
pub fn ccmd(adev: &mut amdgpu_device, vc: &mut vclients, id: u32, req_in: &[u8]) -> Result<Vec<u8>, i32> {
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
        AMDGPU_CCMD_GEM_NEW | AMDGPU_CCMD_BO_VA_OP | AMDGPU_CCMD_CS_SUBMIT | AMDGPU_CCMD_SET_METADATA
        | AMDGPU_CCMD_BO_QUERY_INFO | AMDGPU_CCMD_RESERVE_VMID | AMDGPU_CCMD_SET_PSTATE
        | AMDGPU_CCMD_CS_QUERY_FENCE_STATUS => not_ported(cmd),
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
    Ok(out)
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
