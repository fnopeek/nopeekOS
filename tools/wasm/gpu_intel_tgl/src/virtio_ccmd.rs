//! The ccmd dispatcher of a native context, after virglrenderer
//! src/drm/drm_context.c (`drm_context_submit_cmd_dispatch`) and
//! src/drm/i915/i915_ccmd.c: each command is checked as virglrenderer
//! checks it, then run as the i915 ioctl it stands for (gem/
//! i915_gem_context.c, i915_gem_tiling.c, i915_gem_domain.c, i915_gem.c,
//! intel_uncore.c `i915_reg_read_ioctl`). GEM_CREATE(_EXT) and
//! SET_MMAP_MODE are the kernel's; they never arrive here.
//!
//! A command's response starts with its length; `ret` is a positive errno.
//! Err is a malformed request: the kernel then refuses the submit.

use crate::i915_gem_context::*;
use crate::i915_uapi::*;

/// The response of IOCTL_SIMPLE: length, `ret`, the payload from 8.
const SIMPLE_RSP: usize = 8;
/// `ret` after a pad word (GETPARAM, QUERYPARAM, CONTEXT_CREATE, BUSY).
const RET8: usize = 8;
/// Context ids in `banned_ctx_mask` and in `GEM_EXECBUFFER2`.
const RENDER_RING_BASE: u32 = 0x2000;

/// Sizes of the request structs (`ccmd->size`): a shorter request is
/// zero-extended to them.
fn req_size(cmd: u32) -> usize {
    match cmd {
        I915_CCMD_IOCTL_SIMPLE => 24,
        I915_CCMD_GETPARAM => 24,
        I915_CCMD_QUERYPARAM => 32,
        I915_CCMD_GEM_CONTEXT_CREATE => 24,
        I915_CCMD_GEM_EXECBUFFER2 | I915_CCMD_GEM_EXECBUFFER2_V2 => 48,
        I915_CCMD_GEM_BUSY => 24,
        I915_CCMD_NPK_CONTEXT_SETPARAM | I915_CCMD_NPK_CONTEXT_GETPARAM => 48,
        I915_CCMD_NPK_GEM_EXECBUFFER2 => 64,
        _ => 0,
    }
}

/// `drm_context_submit_cmd_dispatch`: `req` holds the guest's `len` bytes
/// and is zero beyond them up to its length. Returns the response length.
pub fn dispatch(st: &mut nctx, d: &mut drm, c: u8, req: &mut [u8], len: usize, rsp: &mut [u8]) -> Result<usize, i32> {
    if len < VDRM_CCMD_REQ || rd32(req, 4) as usize != len {
        return Err(-EINVAL);
    }
    let cmd = rd32(req, 0);
    let size = req_size(cmd);
    if size == 0 || req.len() < size.max(len) {
        return Err(-EINVAL);
    }
    rsp[..16].fill(0);
    match cmd {
        I915_CCMD_IOCTL_SIMPLE => ioctl_simple(st, d, c, req, len, rsp),
        I915_CCMD_GETPARAM => {
            let param = rd32(req, 16);
            let (ret, mut value) = match crate::i915_getparam::i915_getparam(d.i915, d.gt, d.ggtt, param) {
                Ok(v) => (0, v),
                Err(e) => (-e, -1),
            };
            if param == I915_PARAM_MMAP_GTT_VERSION && value >= 5 {
                value = 4;
            }
            wr32(rsp, 0, 16);
            wr32(rsp, RET8, ret as u32);
            wr32(rsp, 12, value as u32);
            Ok(16)
        }
        I915_CCMD_QUERYPARAM => {
            let (query_id, length, flags) = (rd32(req, 16), rd32(req, 20) as usize, rd32(req, 24));
            let total = 16usize.checked_add(length).filter(|&t| t <= rsp.len()).ok_or(-ENOMEM)?;
            rsp[16..total].fill(0);
            let n = crate::i915_getparam::i915_query(d.gt, query_id, flags, &mut rsp[16..total]);
            wr32(rsp, 0, total as u32);
            wr32(rsp, RET8, 0);
            wr32(rsp, 12, n as u32);
            Ok(total)
        }
        I915_CCMD_GEM_CONTEXT_CREATE => gem_context_create(st, d, c, req, len, rsp),
        I915_CCMD_GEM_EXECBUFFER2 | I915_CCMD_GEM_EXECBUFFER2_V2 => {
            let kind = if cmd == I915_CCMD_GEM_EXECBUFFER2_V2 {
                crate::i915_gem_execbuffer::EXEC_V2
            } else {
                crate::i915_gem_execbuffer::EXEC_V1
            };
            crate::i915_gem_execbuffer::i915_ccmd_gem_execbuffer2(st, d, c, req, kind, rsp)
        }
        I915_CCMD_NPK_CONTEXT_SETPARAM | I915_CCMD_NPK_CONTEXT_GETPARAM => {
            npk_context_param(st, d, c, req, len, cmd == I915_CCMD_NPK_CONTEXT_GETPARAM, rsp)
        }
        I915_CCMD_NPK_GEM_EXECBUFFER2 => crate::i915_gem_execbuffer::i915_ccmd_npk_gem_execbuffer2(st, d, c, req, rsp),
        I915_CCMD_GEM_BUSY => {
            wr32(rsp, 0, 16);
            match st.obj(c, rd32(req, 16)) {
                Some(i) => wr32(rsp, 12, st.obj_busy_flags(d.gt, i)),
                None => wr32(rsp, RET8, EINVAL as u32),
            }
            Ok(16)
        }
        _ => Err(-EINVAL),
    }
}

/// `i915_ccmd_ioctl_simple`: the allow-list, then the ioctl.
fn ioctl_simple(st: &mut nctx, d: &mut drm, c: u8, req: &mut [u8], len: usize, rsp: &mut [u8]) -> Result<usize, i32> {
    let cmd = rd32(req, 16);
    let payload_len = ioc_size(cmd) as usize;
    if len != 24 + payload_len || payload_len > 128 {
        return Err(-EINVAL);
    }
    let mut rsp_len = SIMPLE_RSP;
    if cmd & IOC_OUT != 0 {
        rsp_len += payload_len;
    }
    let mut p = [0u8; 128];
    p[..payload_len].copy_from_slice(&req[24..24 + payload_len]);
    wr32(rsp, 0, rsp_len as u32);

    let ret = match cmd {
        DRM_IOCTL_I915_REG_READ => reg_read(d, &mut p),
        DRM_IOCTL_I915_GEM_GET_APERTURE => {
            // i915_gem_get_aperture_ioctl: the GGTT and what is not pinned.
            let pinned: u64 = d.ggtt.vm_mm.nodes().iter().map(|n| n.size).sum();
            wr64(&mut p, 0, d.ggtt.total);
            wr64(&mut p, 8, d.ggtt.total.saturating_sub(pinned));
            0
        }
        DRM_IOCTL_I915_GEM_SET_TILING | DRM_IOCTL_I915_GEM_GET_TILING | DRM_IOCTL_I915_GEM_SET_DOMAIN => {
            match st.obj(c, rd32(&p, 0)) {
                None => EINVAL,
                Some(i) => match cmd {
                    DRM_IOCTL_I915_GEM_SET_TILING => set_tiling(st, d, i, &mut p),
                    DRM_IOCTL_I915_GEM_GET_TILING => {
                        // i915_gem_get_tiling_ioctl; TGL does no bit-6 swizzling.
                        // The tiling is the buffer's, set by any of its clients.
                        let o = st.objs[i];
                        wr32(&mut p, 4, o.tiling);
                        wr32(&mut p, 8, I915_BIT_6_SWIZZLE_NONE);
                        wr32(&mut p, 12, I915_BIT_6_SWIZZLE_NONE);
                        0
                    }
                    _ => set_domain(st, d, i, &p),
                },
            }
        }
        DRM_IOCTL_I915_GET_RESET_STATS => {
            // i915_gem_context_reset_stats_ioctl; the global count needs
            // CAP_SYS_ADMIN.
            if rd32(&p, 4) != 0 {
                EINVAL
            } else {
                match st.ctx_lookup(d, c, rd32(&p, 0)) {
                    Err(e) => -e,
                    Ok(x) => {
                        wr32(&mut p, 8, 0);
                        wr32(&mut p, 12, st.ctxs[x].guilty);
                        wr32(&mut p, 16, st.ctxs[x].innocent);
                        0
                    }
                }
            }
        }
        DRM_IOCTL_I915_GEM_CONTEXT_CREATE => {
            // i915_gem_context_create_ioctl with the legacy struct: its pad
            // is the flags, and there are no extensions to follow.
            let flags = rd32(&p, 4);
            if flags & !(I915_CONTEXT_CREATE_FLAGS_USE_EXTENSIONS | I915_CONTEXT_CREATE_FLAGS_SINGLE_TIMELINE) != 0 {
                EINVAL
            } else if crate::intel_gt::intel_gt_is_wedged(d.gt) {
                EIO
            } else {
                let id = st.next_ctx_id(c);
                match st.proto_create(c, id, flags & I915_CONTEXT_CREATE_FLAGS_SINGLE_TIMELINE != 0) {
                    Ok(_) => {
                        wr32(&mut p, 0, id);
                        0
                    }
                    Err(e) => -e,
                }
            }
        }
        DRM_IOCTL_I915_GEM_CONTEXT_DESTROY => {
            let id = rd32(&p, 0);
            if rd32(&p, 4) != 0 {
                EINVAL
            } else if id == 0 {
                ENOENT
            } else {
                match st.ctx_or_proto(c, id) {
                    None => ENOENT,
                    Some(x) if st.ctxs[x].proto => {
                        st.proto_close(x);
                        0
                    }
                    Some(x) => {
                        st.ctx_destroy(d.gt, x);
                        0
                    }
                }
            }
        }
        DRM_IOCTL_I915_GEM_CONTEXT_GETPARAM | DRM_IOCTL_I915_GEM_CONTEXT_SETPARAM => {
            // virglrenderer passes these four only; a parameter with data
            // comes as I915_CCMD_NPK_CONTEXT_*PARAM.
            let param = ctx_param { size: rd32(&p, 4), param: rd64(&p, 8), value: rd64(&p, 16), data: &[] };
            if !matches!(param.param, I915_CONTEXT_PARAM_PRIORITY | I915_CONTEXT_PARAM_RECOVERABLE
                         | I915_CONTEXT_PARAM_GTT_SIZE | I915_CONTEXT_PARAM_VM) {
                return Err(-EINVAL);
            }
            let ctx_id = rd32(&p, 0);
            if cmd == DRM_IOCTL_I915_GEM_CONTEXT_GETPARAM {
                match context_getparam(st, d, c, ctx_id, &param, &mut []) {
                    Ok((size, value)) => {
                        wr32(&mut p, 4, size);
                        wr64(&mut p, 16, value);
                        0
                    }
                    Err(e) => e,
                }
            } else {
                context_setparam(st, d, c, ctx_id, &param)
            }
        }
        DRM_IOCTL_I915_GEM_VM_CREATE | DRM_IOCTL_I915_GEM_VM_DESTROY => {
            if rd64(&p, 0) != 0 || rd32(&p, 8) != 0 {
                return Err(-EINVAL);
            }
            if cmd == DRM_IOCTL_I915_GEM_VM_CREATE {
                // i915_gem_vm_create_ioctl: a new address space and its id.
                match st.vm_create(d.i915, c) {
                    Ok(vm) => {
                        let r = st.vm_id_add(c, vm);
                        st.vm_put(vm);
                        match r {
                            Ok(id) => {
                                wr32(&mut p, 12, id);
                                0
                            }
                            Err(e) => -e,
                        }
                    }
                    Err(e) => -e,
                }
            } else {
                match st.vm_id_remove(c, rd32(&p, 12)) {
                    Ok(()) => 0,
                    Err(e) => -e,
                }
            }
        }
        _ => return Err(-EINVAL),
    };
    wr32(rsp, 4, ret as u32);
    if cmd & IOC_OUT != 0 {
        rsp[SIMPLE_RSP..SIMPLE_RSP + payload_len].copy_from_slice(&p[..payload_len]);
    }
    Ok(rsp_len)
}

/// `i915_reg_read_ioctl` with the whitelist of intel_uncore.c: the render
/// ring's timestamp, 8 bytes, read as two halves with I915_REG_READ_8B_WA.
fn reg_read(d: &mut drm, p: &mut [u8]) -> i32 {
    let offset = rd64(p, 0);
    let ldw = RENDER_RING_BASE + 0x358;
    let flags = offset & 7;
    if offset & !7 != ldw as u64 {
        return EINVAL;
    }
    let u = &mut d.i915.uncore;
    let val = if flags == I915_REG_READ_8B_WA {
        // intel_uncore_read64_2x32
        let mut upper = u.intel_uncore_read(ldw + 4);
        let mut lower;
        let mut tries = 2;
        loop {
            let old = upper;
            lower = u.intel_uncore_read(ldw);
            upper = u.intel_uncore_read(ldw + 4);
            tries -= 1;
            if upper == old || tries == 0 {
                break;
            }
        }
        (upper as u64) << 32 | lower as u64
    } else if flags == 0 {
        u.intel_uncore_read64(ldw)
    } else {
        return EINVAL;
    };
    wr64(p, 8, val);
    0
}

/// `i915_gem_set_tiling_ioctl` → `i915_tiling_ok` for GRAPHICS_VER 12
/// (X and Y tiles, a stride a multiple of the tile width up to 256K).
fn set_tiling(st: &mut nctx, d: &mut drm, i: usize, p: &mut [u8]) -> i32 {
    if d.ggtt.num_fences == 0 {
        return EINVAL;
    }
    let mode = rd32(p, 4);
    let mut stride = rd32(p, 8);
    let ok = match mode {
        I915_TILING_NONE => true,
        I915_TILING_X | I915_TILING_Y => {
            let tile_width = if mode == I915_TILING_Y { 128 } else { 512 };
            stride != 0 && stride <= 256 * 1024 && stride % tile_width == 0
        }
        _ => false,
    };
    if !ok {
        return EINVAL;
    }
    if mode == I915_TILING_NONE {
        stride = 0;
    }
    if st.obj_busy_flags(d.gt, i) != 0 {
        return EBUSY;
    }
    // One GEM object in Linux: every client's object on the buffer.
    st.set_tiling_shared(i, mode, stride);
    wr32(p, 4, mode);
    wr32(p, 8, stride);
    wr32(p, 12, I915_BIT_6_SWIZZLE_NONE);
    0
}

/// `i915_gem_set_domain_ioctl`: it waits for the GPU (the whole
/// reservation, another client's requests on a shared buffer too); here a
/// busy object answers EAGAIN, and Mesa asks again.
fn set_domain(st: &mut nctx, d: &mut drm, i: usize, p: &[u8]) -> i32 {
    let (read, write) = (rd32(p, 4), rd32(p, 8));
    // I915_GEM_GPU_DOMAINS may not be asked for.
    const GPU_DOMAINS: u32 = 0x02 | 0x04 | 0x08 | 0x10 | 0x20;
    if (write | read) & GPU_DOMAINS != 0 || (write != 0 && read != write) {
        return EINVAL;
    }
    // A read waits for the writers, a write for everyone.
    let busy = st.obj_busy_flags(d.gt, i);
    if (write != 0 && busy != 0) || busy & 0xffff != 0 {
        return EAGAIN;
    }
    0
}

/// `struct drm_i915_gem_context_param`, with the data a pointer-valued
/// parameter points at inline (`I915_CCMD_NPK_CONTEXT_*PARAM`; empty from
/// IOCTL_SIMPLE). For an engine map the data is the map, then its
/// extension chain, each extension whole and in order.
pub struct ctx_param<'a> {
    pub size: u32,
    pub param: u64,
    pub value: u64,
    pub data: &'a [u8],
}

const EFAULT: i32 = 14;
const E2BIG: i32 = 7;
/// `i915_user_extensions`' limit on a chain.
const MAX_EXTENSIONS: u32 = 512;

/// `i915_gem_context_getparam_ioctl` on the finalized context `x`: Ok is
/// (size, value); `out` takes what a pointer-valued parameter writes back.
fn ctx_getparam(st: &mut nctx, d: &drm, x: usize, p: &ctx_param, out: &mut [u8]) -> Result<(u32, u64), i32> {
    let cx = st.ctxs[x];
    let value = match p.param {
        I915_CONTEXT_PARAM_GTT_SIZE => {
            st.vms.get(cx.vm as usize).and_then(|v| v.as_ref()).map(|v| v.ppgtt.total).unwrap_or(0)
        }
        I915_CONTEXT_PARAM_NO_ERROR_CAPTURE => cx.no_error_capture as u64,
        I915_CONTEXT_PARAM_BANNABLE => cx.bannable as u64,
        I915_CONTEXT_PARAM_RECOVERABLE => cx.recoverable as u64,
        I915_CONTEXT_PARAM_PRIORITY => cx.priority as i64 as u64,
        I915_CONTEXT_PARAM_SSEU => return get_sseu(st, d, x, p, out).map(|size| (size, p.value)),
        // get_ppgtt: a new id for the context's address space.
        I915_CONTEXT_PARAM_VM => match st.vm_id_add(cx.client, cx.vm) {
            Ok(id) => id as u64,
            Err(e) => return Err(-e),
        },
        I915_CONTEXT_PARAM_PERSISTENCE => cx.persistent as u64,
        // get_protected: no context uses protected content.
        I915_CONTEXT_PARAM_PROTECTED_CONTENT => 0,
        _ => return Err(EINVAL),
    };
    Ok((0, value))
}

/// `get_sseu`: the slice configuration of one of the context's engines,
/// `ce->sseu` (the engine's, from `intel_context_init`; a virtual engine's
/// is empty). Returns the size written.
fn get_sseu(st: &nctx, d: &drm, x: usize, p: &ctx_param, out: &mut [u8]) -> Result<u32, i32> {
    if p.size == 0 {
        return Ok(PARAM_SSEU_BYTES as u32);
    }
    if (p.size as usize) < PARAM_SSEU_BYTES {
        return Err(EINVAL);
    }
    let u = p.data.get(..PARAM_SSEU_BYTES).ok_or(EFAULT)?;
    let (class, instance, flags) = (rd16(u, 0), rd16(u, 2), rd32(u, 4));
    if rd32(u, 28) != 0 || flags & !I915_CONTEXT_SSEU_FLAG_ENGINE_INDEX != 0 {
        return Err(EINVAL);
    }
    // lookup_user_engine + i915_gem_context_get_engine.
    let cx = &st.ctxs[x];
    let by_index = flags & I915_CONTEXT_SSEU_FLAG_ENGINE_INDEX != 0;
    if by_index != (cx.num_user_engines >= 0) {
        return Err(EINVAL);
    }
    let sseu = if by_index {
        let idx = instance as usize;
        if idx >= cx.num_user_engines as usize {
            return Err(EINVAL);
        }
        match cx.user_engines[idx].kind {
            ENGINE_TYPE_INVALID => return Err(ENOENT),
            ENGINE_TYPE_PHYSICAL => d.gt.engine[cx.user_engines[idx].engine as usize].map(|e| e.sseu).unwrap_or_default(),
            _ => Default::default(),
        }
    } else {
        let e = intel_engine_lookup_user(d.gt, class, instance).ok_or(EINVAL)?;
        d.gt.engine[e as usize].map(|e| e.sseu).unwrap_or_default()
    };
    let o = out.get_mut(..PARAM_SSEU_BYTES).ok_or(EFAULT)?;
    o.copy_from_slice(u);
    wr64(o, 8, sseu.slice_mask as u64);
    wr64(o, 16, sseu.subslice_mask as u64);
    o[24..26].copy_from_slice(&(sseu.min_eus_per_subslice as u16).to_le_bytes());
    o[26..28].copy_from_slice(&(sseu.max_eus_per_subslice as u16).to_le_bytes());
    Ok(PARAM_SSEU_BYTES as u32)
}

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// `ctx_setparam` on the finalized context `x`; the positive errno or 0.
fn ctx_setparam(st: &mut nctx, x: usize, p: &ctx_param) -> i32 {
    let cx = &mut st.ctxs[x];
    match p.param {
        I915_CONTEXT_PARAM_NO_ERROR_CAPTURE => {
            if p.size != 0 {
                return EINVAL;
            }
            cx.no_error_capture = p.value != 0;
        }
        I915_CONTEXT_PARAM_BANNABLE => {
            if p.size != 0 {
                return EINVAL;
            }
            // Clearing it needs CAP_SYS_ADMIN.
            if p.value == 0 {
                return EPERM;
            }
            cx.bannable = true;
        }
        I915_CONTEXT_PARAM_RECOVERABLE => {
            if p.size != 0 {
                return EINVAL;
            }
            cx.recoverable = p.value != 0;
        }
        I915_CONTEXT_PARAM_PRIORITY => match validate_priority(p) {
            Ok(v) => cx.priority = v,
            Err(e) => return e,
        },
        // set_sseu: only GRAPHICS_VER 11 reconfigures slices.
        I915_CONTEXT_PARAM_SSEU => return if (p.size as usize) < PARAM_SSEU_BYTES { EINVAL } else { ENODEV },
        I915_CONTEXT_PARAM_PERSISTENCE => {
            if p.size != 0 {
                return EINVAL;
            }
            // __context_set_persistence: cancelling a context needs
            // I915_SCHEDULER_CAP_PREEMPTION.
            if p.value == 0 && cx.persistent {
                return ENODEV;
            }
        }
        // set_context_image needs CONFIG_DRM_I915_REPLAY_GPU_HANGS_API.
        _ => return EINVAL,
    }
    0
}

/// `validate_priority`; above the default needs CAP_SYS_NICE.
fn validate_priority(p: &ctx_param) -> Result<i32, i32> {
    if p.size != 0 {
        return Err(EINVAL);
    }
    let prio = p.value as i64;
    if !(I915_CONTEXT_MIN_USER_PRIORITY..=I915_CONTEXT_MAX_USER_PRIORITY).contains(&prio) {
        return Err(EINVAL);
    }
    if prio > I915_CONTEXT_DEFAULT_PRIORITY {
        return Err(EPERM);
    }
    Ok(prio as i32)
}

/// `set_proto_ctx_param` on the proto context `x`; the positive errno or 0.
/// Ok carries the size written back (SSEU).
fn set_proto_ctx_param(st: &mut nctx, d: &drm, c: u8, x: usize, p: &ctx_param) -> i32 {
    match p.param {
        I915_CONTEXT_PARAM_NO_ERROR_CAPTURE => {
            if p.size != 0 {
                return EINVAL;
            }
            st.ctxs[x].no_error_capture = p.value != 0;
        }
        I915_CONTEXT_PARAM_BANNABLE => {
            if p.size != 0 {
                return EINVAL;
            }
            if p.value == 0 {
                return EPERM;
            }
            st.ctxs[x].bannable = true;
        }
        // Only with GuC submission.
        I915_CONTEXT_PARAM_LOW_LATENCY => return EINVAL,
        I915_CONTEXT_PARAM_RECOVERABLE => {
            if p.size != 0 {
                return EINVAL;
            }
            st.ctxs[x].recoverable = p.value != 0;
        }
        I915_CONTEXT_PARAM_PRIORITY => match validate_priority(p) {
            Ok(v) => st.ctxs[x].priority = v,
            Err(e) => return e,
        },
        // set_proto_ctx_sseu: only GRAPHICS_VER 11.
        I915_CONTEXT_PARAM_SSEU => return if (p.size as usize) < PARAM_SSEU_BYTES { EINVAL } else { ENODEV },
        // set_proto_ctx_vm.
        I915_CONTEXT_PARAM_VM => {
            if p.size != 0 {
                return EINVAL;
            }
            if p.value >> 32 != 0 {
                return ENOENT;
            }
            let Some(vm) = st.vm_lookup(c, p.value as u32) else { return ENOENT };
            st.vm_get(vm);
            let old = core::mem::replace(&mut st.ctxs[x].vm, vm);
            if old != NONE16 {
                st.vm_put(old);
            }
        }
        I915_CONTEXT_PARAM_ENGINES => return set_proto_ctx_engines(st, d, x, p),
        I915_CONTEXT_PARAM_PERSISTENCE => {
            if p.size != 0 {
                return EINVAL;
            }
            // proto_context_set_persistence: cancelling needs preemption.
            if p.value == 0 {
                return ENODEV;
            }
            st.ctxs[x].persistent = true;
        }
        // proto_context_set_protected: no PXP.
        I915_CONTEXT_PARAM_PROTECTED_CONTENT => {
            if p.value != 0 {
                return ENODEV;
            }
        }
        _ => return EINVAL,
    }
    0
}

/// `set_proto_ctx_engines`: the engine map and, through its extension
/// chain, balanced, bonded and parallel slots. All or nothing.
fn set_proto_ctx_engines(st: &mut nctx, d: &drm, x: usize, p: &ctx_param) -> i32 {
    if st.ctxs[x].num_user_engines >= 0 {
        return EINVAL;
    }
    let size = p.size as usize;
    if size < 8 || (size - 8) % 4 != 0 {
        return EINVAL;
    }
    let num = (size - 8) / 4;
    if num > MAX_USER_ENGINES {
        return EINVAL;
    }
    let Some(map) = p.data.get(..size) else { return EFAULT };
    let mut set = [proto_engine::INVALID; MAX_USER_ENGINES];
    for (n, pe) in set[..num].iter_mut().enumerate() {
        let (class, instance) = (rd16(map, 8 + n * 4), rd16(map, 10 + n * 4));
        if class == I915_ENGINE_CLASS_INVALID && instance == I915_ENGINE_CLASS_INVALID_NONE {
            continue;
        }
        match intel_engine_lookup_user(d.gt, class, instance) {
            Some(e) => *pe = proto_engine { kind: ENGINE_TYPE_PHYSICAL, engine: e, ..proto_engine::INVALID },
            None => return ENOENT,
        }
    }
    // i915_user_extensions over the chain that follows the map.
    if rd64(map, 0) != 0 {
        let mut pos = size;
        let mut depth = MAX_EXTENSIONS;
        loop {
            if depth == 0 {
                return E2BIG;
            }
            depth -= 1;
            let Some(base) = p.data.get(pos..pos + USER_EXTENSION_BYTES) else { return EFAULT };
            if (12..USER_EXTENSION_BYTES).step_by(4).any(|o| rd32(base, o) != 0) {
                return EINVAL;
            }
            let next = rd64(base, 0);
            let r = match rd32(base, 8) {
                I915_CONTEXT_ENGINES_EXT_LOAD_BALANCE => engines_balance(d, &p.data[pos..], &mut set[..num]),
                I915_CONTEXT_ENGINES_EXT_BOND => engines_bond(d, &p.data[pos..], &set[..num]),
                I915_CONTEXT_ENGINES_EXT_PARALLEL_SUBMIT => engines_parallel(d, &p.data[pos..], &mut set[..num]),
                _ => Err(EINVAL),
            };
            match r {
                Ok(len) => pos += len,
                Err(e) => return e,
            }
            if next == 0 {
                break;
            }
        }
    }
    st.ctxs[x].num_user_engines = num as i16;
    st.ctxs[x].user_engines = set;
    0
}

/// The engines listed at `at` in an extension, looked up: EFAULT past its
/// end, EINVAL for one the GT does not have.
fn ext_engines(d: &drm, ext: &[u8], at: usize, n: usize, out: &mut [u8; MAX_SIBLINGS]) -> Result<(), i32> {
    let list = ext.get(at..at + n * 4).ok_or(EFAULT)?;
    for k in 0..n {
        let e = intel_engine_lookup_user(d.gt, rd16(list, k * 4), rd16(list, k * 4 + 2)).ok_or(EINVAL)?;
        if k < MAX_SIBLINGS {
            out[k] = e;
        }
    }
    Ok(())
}

/// `set_proto_ctx_engines_balance`; Ok is the extension's length.
fn engines_balance(d: &drm, ext: &[u8], set: &mut [proto_engine]) -> Result<usize, i32> {
    let head = ext.get(..LOAD_BALANCE_BYTES).ok_or(EFAULT)?;
    let idx = rd16(head, 32) as usize;
    if idx >= set.len() {
        return Err(EINVAL);
    }
    if set[idx].kind != ENGINE_TYPE_INVALID {
        return Err(EEXIST);
    }
    let num_siblings = rd16(head, 34) as usize;
    if rd32(head, 36) != 0 || rd64(head, 40) != 0 {
        return Err(EINVAL);
    }
    let len = LOAD_BALANCE_BYTES + num_siblings * 4;
    if num_siblings == 0 {
        return Ok(len);
    }
    let mut siblings = [NONE8; MAX_SIBLINGS];
    ext_engines(d, ext, LOAD_BALANCE_BYTES, num_siblings, &mut siblings)?;
    set[idx] = if num_siblings == 1 {
        proto_engine { kind: ENGINE_TYPE_PHYSICAL, engine: siblings[0], ..proto_engine::INVALID }
    } else {
        proto_engine { kind: ENGINE_TYPE_BALANCED, num_siblings: num_siblings.min(MAX_SIBLINGS) as u8, siblings,
                       too_many: num_siblings > MAX_SIBLINGS, ..proto_engine::INVALID }
    };
    Ok(len)
}

/// `set_proto_ctx_engines_bond`: checked, and nothing kept (bonds are
/// no longer acted upon in Linux).
fn engines_bond(d: &drm, ext: &[u8], set: &[proto_engine]) -> Result<usize, i32> {
    let head = ext.get(..BOND_BYTES).ok_or(EFAULT)?;
    let idx = rd16(head, 36) as usize;
    if idx >= set.len() {
        return Err(EINVAL);
    }
    if set[idx].kind != ENGINE_TYPE_PHYSICAL {
        // Invalid, or a virtual engine: neither may be bonded.
        return Err(EINVAL);
    }
    if rd64(head, 40) != 0 || (48..80).step_by(8).any(|o| rd64(head, o) != 0) {
        return Err(EINVAL);
    }
    intel_engine_lookup_user(d.gt, rd16(head, 32), rd16(head, 34)).ok_or(EINVAL)?;
    let num_bonds = rd16(head, 38) as usize;
    let mut bonds = [NONE8; MAX_SIBLINGS];
    ext_engines(d, ext, BOND_BYTES, num_bonds, &mut bonds)?;
    Ok(BOND_BYTES + num_bonds * 4)
}

/// `set_proto_ctx_engines_parallel_submit` without GuC: one sibling per
/// child, on consecutive logical engines of one class.
fn engines_parallel(d: &drm, ext: &[u8], set: &mut [proto_engine]) -> Result<usize, i32> {
    let head = ext.get(..PARALLEL_SUBMIT_BYTES).ok_or(EFAULT)?;
    let (slot, width, num_siblings) = (rd16(head, 32) as usize, rd16(head, 34) as usize, rd16(head, 36) as usize);
    if num_siblings != 1 {
        return Err(EINVAL);
    }
    if slot >= set.len() || set[slot].kind != ENGINE_TYPE_INVALID {
        return Err(EINVAL);
    }
    if rd64(head, 40) != 0 || (48..72).step_by(8).any(|o| rd64(head, o) != 0) {
        return Err(EINVAL);
    }
    if width < 2 {
        return Err(EINVAL);
    }
    let n = width * num_siblings;
    let list = ext.get(PARALLEL_SUBMIT_BYTES..PARALLEL_SUBMIT_BYTES + n * 4).ok_or(EFAULT)?;
    let mut siblings = [NONE8; MAX_SIBLINGS];
    let (mut prev_class, mut prev_mask) = (0u16, 0u32);
    for i in 0..width {
        let mut mask = 0u32;
        for j in 0..num_siblings {
            let k = i * num_siblings + j;
            let class = rd16(list, k * 4);
            let e = intel_engine_lookup_user(d.gt, class, rd16(list, k * 4 + 2)).ok_or(EINVAL)?;
            let eng = d.gt.engine[e as usize].ok_or(EINVAL)?;
            // No breadcrumb handshake on these classes.
            if eng.class == crate::regs::RENDER_CLASS || eng.class == crate::regs::COMPUTE_CLASS {
                return Err(EINVAL);
            }
            if k != 0 && class != prev_class {
                return Err(EINVAL);
            }
            prev_class = class;
            mask |= eng.logical_mask;
            if k < MAX_SIBLINGS {
                siblings[k] = e;
            }
        }
        if i > 0 && mask != prev_mask << 1 {
            return Err(EINVAL);
        }
        prev_mask = mask;
    }
    set[slot] = proto_engine { kind: ENGINE_TYPE_PARALLEL, num_siblings: num_siblings as u8, width: width as u8,
                               siblings, too_many: n > MAX_SIBLINGS, ..proto_engine::INVALID };
    Ok(PARALLEL_SUBMIT_BYTES + n * 4)
}

/// `i915_gem_context_setparam_ioctl`: a proto context takes it as a
/// creation parameter, a finalized one as `ctx_setparam`.
pub fn context_setparam(st: &mut nctx, d: &drm, c: u8, ctx_id: u32, p: &ctx_param) -> i32 {
    match st.ctx_or_proto(c, ctx_id) {
        None => ENOENT,
        Some(x) if st.ctxs[x].proto => set_proto_ctx_param(st, d, c, x, p),
        Some(x) => ctx_setparam(st, x, p),
    }
}

/// `i915_gem_context_getparam_ioctl`: the context is finalized first.
pub fn context_getparam(st: &mut nctx, d: &mut drm, c: u8, ctx_id: u32, p: &ctx_param, out: &mut [u8])
                        -> Result<(u32, u64), i32> {
    let x = st.ctx_lookup(d, c, ctx_id).map_err(|e| -e)?;
    ctx_getparam(st, d, x, p, out)
}

/// `I915_CCMD_NPK_CONTEXT_SETPARAM` / `_GETPARAM`: ctx_id, size, param,
/// value, the data's length and the data. The response is ret, size, value
/// and what a get writes back.
fn npk_context_param(st: &mut nctx, d: &mut drm, c: u8, req: &[u8], len: usize, get: bool, rsp: &mut [u8])
                     -> Result<usize, i32> {
    const HEAD: usize = 48;
    let data_len = rd32(req, 40) as usize;
    if len != HEAD.checked_add(data_len).ok_or(-EINVAL)? {
        return Err(-EINVAL);
    }
    let ctx_id = rd32(req, 16);
    let p = ctx_param { size: rd32(req, 20), param: rd64(req, 24), value: rd64(req, 32), data: &req[HEAD..len] };
    let room = rsp.len().checked_sub(24).ok_or(-EINVAL)?;
    let (ret, size, value, out_len) = if get {
        let mut out = alloc::vec![0u8; data_len.min(room)];
        match context_getparam(st, d, c, ctx_id, &p, &mut out) {
            Ok((size, value)) => {
                let n = (size as usize).min(out.len());
                rsp[24..24 + n].copy_from_slice(&out[..n]);
                (0, size, value, n)
            }
            Err(e) => (e, p.size, p.value, 0),
        }
    } else {
        (context_setparam(st, d, c, ctx_id, &p), p.size, p.value, 0)
    };
    wr32(rsp, 0, (24 + out_len) as u32);
    wr32(rsp, 4, ret as u32);
    wr32(rsp, 8, size);
    wr32(rsp, 12, 0);
    wr64(rsp, 16, value);
    Ok(24 + out_len)
}

/// `i915_ccmd_gem_context_create`: virglrenderer walks the setparam chain
/// in the request; then `i915_gem_context_create_ioctl`, which on
/// GRAPHICS_VER 12 registers a proto context and applies each parameter
/// with `set_proto_ctx_param`. A parameter's data follows it inline; an
/// engine map's extension chain cannot (it is a pointer), and the guest
/// sets such a map afterwards with `I915_CCMD_NPK_CONTEXT_SETPARAM`.
fn gem_context_create(st: &mut nctx, d: &mut drm, c: u8, req: &mut [u8], len: usize, rsp: &mut [u8]) -> Result<usize, i32> {
    let flags = rd32(req, 16);
    let mut params_size = rd32(req, 20) as i64;
    if len as i64 != 24 + params_size {
        return Err(-EINVAL);
    }
    wr32(rsp, 0, 16);
    if flags & !(I915_CONTEXT_CREATE_FLAGS_USE_EXTENSIONS | I915_CONTEXT_CREATE_FLAGS_SINGLE_TIMELINE) != 0 {
        wr32(rsp, RET8, EINVAL as u32);
        return Ok(16);
    }
    if crate::intel_gt::intel_gt_is_wedged(d.gt) {
        wr32(rsp, RET8, EIO as u32);
        return Ok(16);
    }
    let id = st.next_ctx_id(c);
    let x = match st.proto_create(c, id, flags & I915_CONTEXT_CREATE_FLAGS_SINGLE_TIMELINE != 0) {
        Ok(x) => x,
        Err(e) => {
            wr32(rsp, RET8, (-e) as u32);
            return Ok(16);
        }
    };
    let mut ptr = 24usize;
    let mut err = 0i32;
    if flags & I915_CONTEXT_CREATE_FLAGS_USE_EXTENSIONS != 0 && params_size > 0 {
        loop {
            if params_size < SETPARAM_BYTES as i64 {
                st.proto_close(x);
                return Err(-EINVAL);
            }
            let sp = ptr;
            ptr += SETPARAM_BYTES;
            params_size -= SETPARAM_BYTES as i64;
            let size = rd32(req, sp + 36) as i64;
            if size > params_size {
                st.proto_close(x);
                return Err(-EINVAL);
            }
            // create_setparam: the extension's name is SETPARAM and its
            // parameter names no context; the base's flags and rsvd are zero.
            if err == 0 {
                if rd32(req, sp + 8) != 0 || (12..32).step_by(4).any(|o| rd32(req, sp + o) != 0) || rd32(req, sp + 32) != 0 {
                    err = EINVAL;
                } else {
                    let p = ctx_param { size: size as u32, param: rd64(req, sp + 40), value: rd64(req, sp + 48),
                                        data: &req[ptr..ptr + size as usize] };
                    err = set_proto_ctx_param(st, d, c, x, &p);
                }
            }
            ptr += size as usize;
            params_size -= size;
            if params_size <= 0 {
                break;
            }
        }
    }
    if params_size != 0 {
        st.proto_close(x);
        return Err(-EINVAL);
    }
    if err != 0 {
        st.proto_close(x);
        wr32(rsp, RET8, err as u32);
        return Ok(16);
    }
    wr32(rsp, 12, id);
    Ok(16)
}
