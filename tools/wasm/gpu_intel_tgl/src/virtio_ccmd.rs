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
            crate::i915_gem_execbuffer::i915_ccmd_gem_execbuffer2(st, d, c, req, cmd == I915_CCMD_GEM_EXECBUFFER2_V2, rsp)
        }
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
                None => {
                    wr32(rsp, 4, EINVAL as u32);
                    return Ok(rsp_len);
                }
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
                match st.ctx(c, rd32(&p, 0)) {
                    None => ENOENT,
                    Some(x) => {
                        wr32(&mut p, 8, 0);
                        wr32(&mut p, 12, st.ctxs[x].guilty);
                        wr32(&mut p, 16, st.ctxs[x].innocent);
                        0
                    }
                }
            }
        }
        DRM_IOCTL_I915_GEM_CONTEXT_CREATE => {
            // The legacy create: no flags, no extensions.
            if rd32(&p, 4) & !(I915_CONTEXT_CREATE_FLAGS_USE_EXTENSIONS | I915_CONTEXT_CREATE_FLAGS_SINGLE_TIMELINE) != 0 {
                EINVAL
            } else {
                let id = st.next_ctx_id(c);
                match st.ctx_create(d.i915, c, None, id) {
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
                match st.ctx(c, id) {
                    None => ENOENT,
                    Some(x) => {
                        st.ctx_destroy(d.gt, x);
                        0
                    }
                }
            }
        }
        DRM_IOCTL_I915_GEM_CONTEXT_GETPARAM | DRM_IOCTL_I915_GEM_CONTEXT_SETPARAM => {
            let param = rd64(&p, 8);
            let value = rd64(&p, 16);
            if param == I915_CONTEXT_PARAM_PRIORITY && value as i64 > I915_CONTEXT_DEFAULT_PRIORITY
                && cmd == DRM_IOCTL_I915_GEM_CONTEXT_SETPARAM {
                wr32(rsp, 4, EPERM as u32);
                return Ok(rsp_len);
            }
            if !matches!(param, I915_CONTEXT_PARAM_PRIORITY | I915_CONTEXT_PARAM_RECOVERABLE
                         | I915_CONTEXT_PARAM_GTT_SIZE | I915_CONTEXT_PARAM_VM) {
                return Err(-EINVAL);
            }
            match st.ctx(c, rd32(&p, 0)) {
                None => ENOENT,
                Some(x) if cmd == DRM_IOCTL_I915_GEM_CONTEXT_GETPARAM => ctx_getparam(st, x, &mut p),
                Some(x) => ctx_setparam(st, x, &p),
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

/// `I915_CONTEXT_PARAM_*` read (`i915_gem_context_getparam_ioctl`).
fn ctx_getparam(st: &mut nctx, x: usize, p: &mut [u8]) -> i32 {
    let param = rd64(p, 8);
    let value: u64 = match param {
        I915_CONTEXT_PARAM_GTT_SIZE => st.vms[st.ctxs[x].vm as usize].ppgtt.as_ref().map(|v| v.total).unwrap_or(0),
        I915_CONTEXT_PARAM_RECOVERABLE => st.ctxs[x].recoverable as u64,
        I915_CONTEXT_PARAM_PRIORITY => st.ctxs[x].priority as i64 as u64,
        // get_ppgtt: a new id for the context's address space.
        _ => {
            let (c, vm) = (st.ctxs[x].client, st.ctxs[x].vm);
            match st.vm_id_add(c, vm) {
                Ok(id) => id as u64,
                Err(e) => return -e,
            }
        }
    };
    wr32(p, 4, 0);
    wr64(p, 16, value);
    0
}

/// `I915_CONTEXT_PARAM_*` written (`ctx_setparam`).
fn ctx_setparam(st: &mut nctx, x: usize, p: &[u8]) -> i32 {
    let (size, param, value) = (rd32(p, 4), rd64(p, 8), rd64(p, 16));
    match param {
        I915_CONTEXT_PARAM_PRIORITY => match set_priority(value as i64) {
            Ok(v) => {
                st.ctxs[x].priority = v;
                0
            }
            Err(e) => e,
        },
        I915_CONTEXT_PARAM_RECOVERABLE => {
            if size != 0 {
                EINVAL
            } else {
                st.ctxs[x].recoverable = value != 0;
                0
            }
        }
        _ => EINVAL,
    }
}

/// `validate_priority`; above the default needs CAP_SYS_NICE.
fn set_priority(prio: i64) -> Result<i32, i32> {
    if !(I915_CONTEXT_MIN_USER_PRIORITY..=I915_CONTEXT_MAX_USER_PRIORITY).contains(&prio) {
        return Err(EINVAL);
    }
    if prio > I915_CONTEXT_DEFAULT_PRIORITY {
        return Err(EPERM);
    }
    Ok(prio as i32)
}

/// The settings a proto context collects (`struct i915_gem_proto_context`).
struct proto {
    vm: Option<u8>,
    priority: i32,
    recoverable: bool,
    bannable: bool,
    persistent: bool,
    no_error_capture: bool,
    user_engines: bool,
    n_engines: u8,
    engines: [u8; MAX_USER_ENGINES],
}

/// `i915_ccmd_gem_context_create`: virglrenderer walks the setparam chain
/// in the request; then `i915_gem_context_create_ioctl` with each
/// `set_proto_ctx_param`.
fn gem_context_create(st: &mut nctx, d: &mut drm, c: u8, req: &mut [u8], len: usize, rsp: &mut [u8]) -> Result<usize, i32> {
    let flags = rd32(req, 16);
    let mut params_size = rd32(req, 20) as i64;
    if len as i64 != 24 + params_size {
        return Err(-EINVAL);
    }
    if flags > 3 {
        return Err(-EINVAL);
    }
    wr32(rsp, 0, 16);
    let mut pc = proto { vm: None, priority: 0, recoverable: true, bannable: true, persistent: true,
                         no_error_capture: false, user_engines: false, n_engines: 0, engines: [NONE8; MAX_USER_ENGINES] };
    let mut ptr = 24usize;
    let mut err = 0i32;
    if flags & I915_CONTEXT_CREATE_FLAGS_USE_EXTENSIONS != 0 {
        loop {
            if params_size < SETPARAM_BYTES as i64 {
                return Err(-EINVAL);
            }
            let sp = ptr;
            ptr += SETPARAM_BYTES;
            params_size -= SETPARAM_BYTES as i64;
            let size = rd32(req, sp + 36) as i64;
            let param = rd64(req, sp + 40);
            let value = rd64(req, sp + 48);
            if size > params_size || size % 4 != 0 || size > 128 {
                return Err(-EINVAL);
            }
            match param {
                I915_CONTEXT_PARAM_PRIORITY if value as i64 > I915_CONTEXT_DEFAULT_PRIORITY => {
                    wr32(rsp, RET8, EPERM as u32);
                    return Ok(16);
                }
                I915_CONTEXT_PARAM_ENGINES => {
                    if size < 8 {
                        return Err(-EINVAL);
                    }
                    if rd64(req, ptr) != 0 {
                        return Err(-EINVAL);
                    }
                }
                I915_CONTEXT_PARAM_NO_ERROR_CAPTURE | I915_CONTEXT_PARAM_BANNABLE | I915_CONTEXT_PARAM_SSEU
                | I915_CONTEXT_PARAM_RECOVERABLE | I915_CONTEXT_PARAM_VM | I915_CONTEXT_PARAM_PERSISTENCE
                | I915_CONTEXT_PARAM_PRIORITY | I915_CONTEXT_PARAM_PROTECTED_CONTENT => {}
                _ => return Err(-EINVAL),
            }
            // The base extension's name is I915_CONTEXT_CREATE_EXT_SETPARAM,
            // and a setparam names no context.
            if err == 0 && (rd32(req, sp + 8) != 0 || rd32(req, sp + 32) != 0) {
                err = EINVAL;
            }
            if err == 0 {
                err = set_proto_ctx_param(st, d, c, &mut pc, size as usize, param, value, &req[ptr..ptr + size as usize]);
            }
            ptr += size as usize;
            params_size -= size;
            if params_size <= 0 {
                break;
            }
        }
    }
    if params_size != 0 {
        return Err(-EINVAL);
    }
    if err != 0 {
        wr32(rsp, RET8, err as u32);
        return Ok(16);
    }
    let id = st.next_ctx_id(c);
    match st.ctx_create(d.i915, c, pc.vm, id) {
        Ok(x) => {
            let cx = &mut st.ctxs[x];
            cx.priority = pc.priority;
            cx.recoverable = pc.recoverable;
            cx.bannable = pc.bannable;
            cx.persistent = pc.persistent;
            cx.no_error_capture = pc.no_error_capture;
            cx.user_engines = pc.user_engines;
            cx.n_engines = pc.n_engines;
            cx.engines = pc.engines;
            wr32(rsp, 12, id);
        }
        Err(e) => wr32(rsp, RET8, (-e) as u32),
    }
    Ok(16)
}

/// `set_proto_ctx_param`; the positive errno, or 0.
#[allow(clippy::too_many_arguments)]
fn set_proto_ctx_param(st: &mut nctx, d: &mut drm, c: u8, pc: &mut proto, size: usize, param: u64, value: u64,
                       data: &[u8]) -> i32 {
    match param {
        I915_CONTEXT_PARAM_NO_ERROR_CAPTURE => {
            if size != 0 {
                return EINVAL;
            }
            pc.no_error_capture = value != 0;
        }
        I915_CONTEXT_PARAM_BANNABLE => {
            if size != 0 {
                return EINVAL;
            }
            // Clearing it needs CAP_SYS_ADMIN.
            if value == 0 {
                return EPERM;
            }
            pc.bannable = true;
        }
        I915_CONTEXT_PARAM_RECOVERABLE => {
            if size != 0 {
                return EINVAL;
            }
            pc.recoverable = value != 0;
        }
        I915_CONTEXT_PARAM_PRIORITY => match set_priority(value as i64) {
            Ok(v) => pc.priority = v,
            Err(e) => return e,
        },
        I915_CONTEXT_PARAM_VM => {
            if size != 0 {
                return EINVAL;
            }
            match st.vm_lookup(c, value as u32) {
                Some(vm) => pc.vm = Some(vm),
                None => return ENOENT,
            }
        }
        I915_CONTEXT_PARAM_ENGINES => {
            // set_proto_ctx_engines; no load balancing, bonds or parallel
            // submission (their extensions are refused above).
            if size == 0 {
                pc.user_engines = false;
                return 0;
            }
            if (size - 8) % 4 != 0 {
                return EINVAL;
            }
            let n = (size - 8) / 4;
            if n > MAX_USER_ENGINES {
                return EINVAL;
            }
            for k in 0..n {
                let class = u16::from_le_bytes([data[8 + k * 4], data[9 + k * 4]]);
                let instance = u16::from_le_bytes([data[10 + k * 4], data[11 + k * 4]]);
                if class == I915_ENGINE_CLASS_INVALID_NONE && instance == I915_ENGINE_CLASS_INVALID_NONE {
                    pc.engines[k] = NONE8;
                    continue;
                }
                match intel_engine_lookup_user(d.gt, class, instance) {
                    Some(id) => pc.engines[k] = id,
                    None => return EINVAL,
                }
            }
            pc.user_engines = true;
            pc.n_engines = n as u8;
        }
        I915_CONTEXT_PARAM_PERSISTENCE => {
            if size != 0 {
                return EINVAL;
            }
            // A non-persistent context is cancelled by preempt-to-idle,
            // which needs I915_SCHEDULER_CAP_PREEMPTION.
            if value == 0 {
                return ENODEV;
            }
            pc.persistent = true;
        }
        // proto_context_set_protected: no PXP.
        I915_CONTEXT_PARAM_PROTECTED_CONTENT => return ENODEV,
        // set_proto_ctx_sseu reconfigures the render slice set, which is
        // not ported.
        I915_CONTEXT_PARAM_SSEU => return EINVAL,
        _ => return EINVAL,
    }
    0
}
