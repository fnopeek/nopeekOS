//! amdgpu/amdgpu_ctx.c (Linux 6.18.55): a client's submission contexts,
//! an entity per engine and ring, created on first use.

use crate::amdgpu_device::amdgpu_device;
use crate::dma_fence::{self, Fence};
use crate::drm_sched;
use crate::linux::Line;
use crate::regs::*;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::Cell;

/// The context priorities, signed as the UAPI has them.
const PRIORITY_UNSET: i32 = AMDGPU_CTX_PRIORITY_UNSET as i32;
pub const PRIORITY_NORMAL: i32 = AMDGPU_CTX_PRIORITY_NORMAL as i32;

/// `amdgpu_sched_jobs`: fences kept per entity, a power of two.
const AMDGPU_SCHED_JOBS: usize = 32;

/// `amdgpu_ctx_num_entities`, by `AMDGPU_HW_IP_*`.
pub const AMDGPU_CTX_NUM_ENTITIES: [u32; AMDGPU_HW_IP_NUM as usize] = [1, 4, 2, 1, 1, 1, 1, 1, 1, 1];

/// `struct amdgpu_ctx_entity`.
pub struct amdgpu_ctx_entity {
    pub hw_ip: u32,
    pub sequence: u64,
    /// The scheduler entity, by key.
    pub entity: usize,
    pub fences: Vec<Option<Fence>>,
}

/// `struct amdgpu_ctx`.
pub struct amdgpu_ctx {
    pub refcount: u32,
    pub reset_counter: u32,
    pub reset_counter_query: u32,
    pub generation: u64,
    pub init_priority: i32,
    pub override_priority: i32,
    pub guilty: Rc<Cell<i32>>,
    pub preamble_presented: bool,
    pub entities: Vec<Vec<Option<amdgpu_ctx_entity>>>,
    /// The pstate to restore when this context gives the stable pstate up.
    pub stable_pstate: u32,
    /// This context among all clients' (`adev.next_ctx_serial`).
    pub serial: u64,
}

/// `struct amdgpu_ctx_mgr`: the contexts by handle.
#[derive(Default)]
pub struct amdgpu_ctx_mgr {
    pub ctx_handles: alloc::collections::BTreeMap<u32, amdgpu_ctx>,
}

/// `amdgpu_ctx_to_drm_sched_prio`.
fn amdgpu_ctx_to_drm_sched_prio(ctx_prio: i32) -> u32 {
    match ctx_prio {
        PRIORITY_UNSET => drm_sched::DRM_SCHED_PRIORITY_NORMAL,
        p if p < 0 => drm_sched::DRM_SCHED_PRIORITY_LOW,
        PRIORITY_NORMAL => drm_sched::DRM_SCHED_PRIORITY_NORMAL,
        // HIGH and VERY_HIGH
        _ => drm_sched::DRM_SCHED_PRIORITY_HIGH,
    }
}

/// `amdgpu_ctx_prio_to_gfx_pipe_prio`.
fn amdgpu_ctx_prio_to_gfx_pipe_prio(prio: i32) -> u32 {
    if prio > PRIORITY_NORMAL { AMDGPU_GFX_PIPE_PRIO_HIGH } else { AMDGPU_GFX_PIPE_PRIO_NORMAL }
}

fn ctx_prio(ctx: &amdgpu_ctx) -> i32 {
    if ctx.override_priority == PRIORITY_UNSET { ctx.init_priority } else { ctx.override_priority }
}

/// `amdgpu_ctx_sched_prio_to_ring_prio`.
fn amdgpu_ctx_sched_prio_to_ring_prio(prio: i32) -> u32 {
    match prio {
        p if p == AMDGPU_CTX_PRIORITY_HIGH as i32 => AMDGPU_RING_PRIO_1,
        p if p == AMDGPU_CTX_PRIORITY_VERY_HIGH as i32 => AMDGPU_RING_PRIO_2,
        _ => AMDGPU_RING_PRIO_0,
    }
}

/// `amdgpu_ctx_get_hw_prio`.
fn amdgpu_ctx_get_hw_prio(adev: &amdgpu_device, ctx: &amdgpu_ctx, hw_ip: u32) -> u32 {
    let mut hw_prio = match hw_ip {
        AMDGPU_HW_IP_GFX | AMDGPU_HW_IP_COMPUTE => amdgpu_ctx_prio_to_gfx_pipe_prio(ctx_prio(ctx)),
        AMDGPU_HW_IP_VCE | AMDGPU_HW_IP_VCN_ENC => amdgpu_ctx_sched_prio_to_ring_prio(ctx_prio(ctx)),
        _ => AMDGPU_RING_PRIO_DEFAULT,
    };
    if adev.gpu_sched[hw_ip as usize][hw_prio as usize].is_empty() {
        hw_prio = AMDGPU_RING_PRIO_DEFAULT;
    }
    hw_prio
}

/// `amdgpu_ctx_init_entity`: no partitions.
fn amdgpu_ctx_init_entity(adev: &mut amdgpu_device, ctx: &mut amdgpu_ctx, hw_ip: u32, ring: u32) -> i32 {
    let hw_prio = amdgpu_ctx_get_hw_prio(adev, ctx, hw_ip);
    let drm_prio = amdgpu_ctx_to_drm_sched_prio(ctx_prio(ctx));
    let mut scheds = adev.gpu_sched[hw_ip as usize][hw_prio as usize].clone();
    // disable load balance if the hw engine retains context among dependent jobs
    if matches!(hw_ip, AMDGPU_HW_IP_VCN_ENC | AMDGPU_HW_IP_VCN_DEC | AMDGPU_HW_IP_UVD_ENC | AMDGPU_HW_IP_UVD) {
        scheds = drm_sched::drm_sched_pick_best(adev, &scheds).into_iter().collect();
    }
    match drm_sched::drm_sched_entity_init(adev, drm_prio, &scheds, Some(ctx.guilty.clone())) {
        Ok(e) => {
            ctx.entities[hw_ip as usize][ring as usize] = Some(amdgpu_ctx_entity {
                hw_ip,
                sequence: 1,
                entity: e,
                fences: (0..AMDGPU_SCHED_JOBS).map(|_| None).collect(),
            });
            0
        }
        Err(r) => r,
    }
}

/// `amdgpu_ctx_init`.
fn amdgpu_ctx_init(adev: &mut amdgpu_device, vm: usize, priority: i32) -> amdgpu_ctx {
    let generation = crate::amdgpu_vm::amdgpu_vm_generation(adev, Some(vm));
    let serial = adev.next_ctx_serial;
    adev.next_ctx_serial += 1;
    amdgpu_ctx {
        refcount: 1,
        reset_counter: adev.gpu_reset_counter,
        reset_counter_query: adev.gpu_reset_counter,
        generation,
        init_priority: priority,
        override_priority: PRIORITY_UNSET,
        guilty: Rc::new(Cell::new(0)),
        preamble_presented: false,
        entities: AMDGPU_CTX_NUM_ENTITIES.iter().map(|&n| (0..n).map(|_| None).collect()).collect(),
        stable_pstate: AMDGPU_CTX_STABLE_PSTATE_NONE,
        serial,
    }
}

/// `amdgpu_ctx_get_stable_pstate`.
fn amdgpu_ctx_get_stable_pstate(adev: &amdgpu_device) -> u32 {
    match crate::amdgpu_smu::amdgpu_dpm_get_performance_level(adev) {
        AMD_DPM_FORCED_LEVEL_PROFILE_STANDARD => AMDGPU_CTX_STABLE_PSTATE_STANDARD,
        AMD_DPM_FORCED_LEVEL_PROFILE_MIN_SCLK => AMDGPU_CTX_STABLE_PSTATE_MIN_SCLK,
        AMD_DPM_FORCED_LEVEL_PROFILE_MIN_MCLK => AMDGPU_CTX_STABLE_PSTATE_MIN_MCLK,
        AMD_DPM_FORCED_LEVEL_PROFILE_PEAK => AMDGPU_CTX_STABLE_PSTATE_PEAK,
        _ => AMDGPU_CTX_STABLE_PSTATE_NONE,
    }
}

/// `__amdgpu_ctx_set_stable_pstate`.
fn __amdgpu_ctx_set_stable_pstate(adev: &mut amdgpu_device, ctx: &mut amdgpu_ctx, stable_pstate: u32) -> i32 {
    let level = match stable_pstate {
        AMDGPU_CTX_STABLE_PSTATE_NONE => AMD_DPM_FORCED_LEVEL_AUTO,
        AMDGPU_CTX_STABLE_PSTATE_STANDARD => AMD_DPM_FORCED_LEVEL_PROFILE_STANDARD,
        AMDGPU_CTX_STABLE_PSTATE_MIN_SCLK => AMD_DPM_FORCED_LEVEL_PROFILE_MIN_SCLK,
        AMDGPU_CTX_STABLE_PSTATE_MIN_MCLK => AMD_DPM_FORCED_LEVEL_PROFILE_MIN_MCLK,
        AMDGPU_CTX_STABLE_PSTATE_PEAK => AMD_DPM_FORCED_LEVEL_PROFILE_PEAK,
        _ => return -22,
    };
    let current_ctx = adev.pm.stable_pstate_ctx;
    if current_ctx.is_some_and(|s| s != ctx.serial) {
        return -16;
    }
    let current_stable_pstate = amdgpu_ctx_get_stable_pstate(adev);
    if current_stable_pstate == stable_pstate {
        return 0;
    }
    let r = crate::amdgpu_smu::amdgpu_dpm_force_performance_level(adev, level);
    if r != 0 {
        return r;
    }
    if current_ctx.is_none() {
        adev.pm.stable_pstate_ctx = Some(ctx.serial);
        ctx.stable_pstate = current_stable_pstate;
    }
    0
}

/// `amdgpu_ctx_fini`'s part for the stable pstate: the context that holds
/// it puts back what it found.
fn amdgpu_ctx_fini_stable_pstate(adev: &mut amdgpu_device, ctx: &mut amdgpu_ctx) {
    if adev.pm.stable_pstate_ctx == Some(ctx.serial) {
        let p = ctx.stable_pstate;
        __amdgpu_ctx_set_stable_pstate(adev, ctx, p);
        adev.pm.stable_pstate_ctx = None;
    }
}

/// `amdgpu_ctx_ioctl` for AMDGPU_CTX_OP_GET/SET_STABLE_PSTATE
/// (`amdgpu_ctx_stable_pstate`): `out.pstate.flags`.
pub fn amdgpu_ctx_ioctl_stable_pstate(adev: &mut amdgpu_device, mgr: &mut amdgpu_ctx_mgr, id: u32, op: u32, flags: u32)
                                      -> Result<u32, i32> {
    let set = match op {
        AMDGPU_CTX_OP_GET_STABLE_PSTATE => {
            if flags != 0 {
                return Err(-22);
            }
            false
        }
        AMDGPU_CTX_OP_SET_STABLE_PSTATE => {
            if flags & !AMDGPU_CTX_STABLE_PSTATE_FLAGS_MASK != 0 {
                return Err(-22);
            }
            if flags & AMDGPU_CTX_STABLE_PSTATE_FLAGS_MASK > AMDGPU_CTX_STABLE_PSTATE_PEAK {
                return Err(-22);
            }
            true
        }
        _ => return Err(-22),
    };
    let Some(ctx) = mgr.ctx_handles.get_mut(&id) else { return Err(-22) };
    if set {
        let r = __amdgpu_ctx_set_stable_pstate(adev, ctx, flags & AMDGPU_CTX_STABLE_PSTATE_FLAGS_MASK);
        // Nothing is written back: the union's out reads as the in's op.
        if r != 0 { Err(r) } else { Ok(op) }
    } else {
        Ok(amdgpu_ctx_get_stable_pstate(adev))
    }
}

/// `amdgpu_ctx_priority_is_valid`.
pub fn amdgpu_ctx_priority_is_valid(ctx_prio: i32) -> bool {
    [AMDGPU_CTX_PRIORITY_VERY_LOW as i64 as i32, AMDGPU_CTX_PRIORITY_LOW as i64 as i32, PRIORITY_NORMAL,
     AMDGPU_CTX_PRIORITY_HIGH as i32, AMDGPU_CTX_PRIORITY_VERY_HIGH as i32].contains(&ctx_prio)
}

/// `amdgpu_ctx_priority_permit`: above NORMAL needs CAP_SYS_NICE or the DRM
/// master; `privileged` says whether the client has either.
fn amdgpu_ctx_priority_permit(privileged: bool, priority: i32) -> i32 {
    // NORMAL and below are accessible by everyone
    if priority <= PRIORITY_NORMAL || privileged {
        return 0;
    }
    -13
}

/// `amdgpu_ctx_alloc` with the permission check of `amdgpu_ctx_init`: the
/// new context's handle.
pub fn amdgpu_ctx_alloc_checked(adev: &mut amdgpu_device, mgr: &mut amdgpu_ctx_mgr, vm: usize, priority: i32,
                                privileged: bool) -> Result<u32, i32> {
    let r = amdgpu_ctx_priority_permit(privileged, priority);
    if r != 0 {
        return Err(r);
    }
    amdgpu_ctx_alloc(adev, mgr, vm, priority)
}

/// `amdgpu_ctx_free` with `amdgpu_ctx_do_release`: the entities go with
/// the context.
pub fn amdgpu_ctx_free(adev: &mut amdgpu_device, mgr: &mut amdgpu_ctx_mgr, id: u32) -> i32 {
    let Some(mut ctx) = mgr.ctx_handles.remove(&id) else { return -22 };
    amdgpu_ctx_fini_stable_pstate(adev, &mut ctx);
    for c in ctx.entities.into_iter().flatten().flatten() {
        drm_sched::drm_sched_entity_destroy(adev, c.entity);
        adev.sched_entities[c.entity] = None;
    }
    0
}

/// `amdgpu_ctx_get_fence`: the fence of submission `seq` of `entity`;
/// Err for one not made yet, None for one too old to be kept.
pub fn amdgpu_ctx_get_fence(ctx: &amdgpu_ctx, hw_ip: u32, ring: u32, seq: u64) -> Result<Option<Fence>, i32> {
    let Some(c) = ctx.entities.get(hw_ip as usize).and_then(|e| e.get(ring as usize)).and_then(|c| c.as_ref()) else {
        return Ok(None);
    };
    let seq = if seq == !0 { c.sequence - 1 } else { seq };
    if seq >= c.sequence {
        return Err(-22);
    }
    if seq + (AMDGPU_SCHED_JOBS as u64) < c.sequence {
        return Ok(None);
    }
    Ok(c.fences[(seq as usize) & (AMDGPU_SCHED_JOBS - 1)].clone())
}

/// `amdgpu_ctx_alloc`: the new context's handle.
pub fn amdgpu_ctx_alloc(adev: &mut amdgpu_device, mgr: &mut amdgpu_ctx_mgr, vm: usize, priority: i32)
                        -> Result<u32, i32> {
    let id = (1..AMDGPU_VM_MAX_NUM_CTX).find(|i| !mgr.ctx_handles.contains_key(i)).ok_or(-28)?;
    let ctx = amdgpu_ctx_init(adev, vm, priority);
    mgr.ctx_handles.insert(id, ctx);
    Ok(id)
}

/// `amdgpu_ctx_get_entity`: the scheduler entity's key.
pub fn amdgpu_ctx_get_entity(adev: &mut amdgpu_device, ctx: &mut amdgpu_ctx, hw_ip: u32, instance: u32, ring: u32)
                             -> Result<usize, i32> {
    if hw_ip >= AMDGPU_HW_IP_NUM {
        Line::new().s("unknown HW IP type: ").dec(hw_ip as u64).say();
        return Err(-22);
    }
    // Right now all IPs have only one instance - multiple rings.
    if instance != 0 || ring >= AMDGPU_CTX_NUM_ENTITIES[hw_ip as usize] {
        return Err(-22);
    }
    if ctx.entities[hw_ip as usize][ring as usize].is_none() {
        let r = amdgpu_ctx_init_entity(adev, ctx, hw_ip, ring);
        if r != 0 {
            return Err(r);
        }
    }
    let e = ctx.entities[hw_ip as usize][ring as usize].as_ref().unwrap().entity;
    let r = drm_sched::drm_sched_entity_error(adev, e);
    if r != 0 {
        return Err(r);
    }
    Ok(e)
}

fn centity(ctx: &mut amdgpu_ctx, e: usize) -> &mut amdgpu_ctx_entity {
    ctx.entities.iter_mut().flatten().flatten().find(|c| c.entity == e).unwrap()
}

/// `amdgpu_ctx_add_fence`: the fence's sequence number in the context.
pub fn amdgpu_ctx_add_fence(adev: &mut amdgpu_device, ctx: &mut amdgpu_ctx, e: usize, fence: &Fence) -> u64 {
    let c = centity(ctx, e);
    let seq = c.sequence;
    let idx = (seq as usize) & (AMDGPU_SCHED_JOBS - 1);
    let other = c.fences[idx].replace(fence.clone());
    c.sequence += 1;
    if let Some(o) = other {
        if !dma_fence::dma_fence_is_signaled(adev, &o) {
            Line::new().s("WARNING: amdgpu_ctx_add_fence over a pending fence").say();
        }
    }
    seq
}

/// `amdgpu_ctx_wait_prev_fence`: no more than `amdgpu_sched_jobs` in flight.
pub fn amdgpu_ctx_wait_prev_fence(adev: &mut amdgpu_device, ctx: &mut amdgpu_ctx, e: usize) -> i32 {
    let c = centity(ctx, e);
    let idx = (c.sequence as usize) & (AMDGPU_SCHED_JOBS - 1);
    let Some(other) = c.fences[idx].clone() else { return 0 };
    let r = dma_fence::dma_fence_wait(adev, &other);
    if r < 0 {
        Line::new().s("Error (").dec((-r) as u64).s(") waiting for fence!").say();
    }
    r as i32
}

/// `amdgpu_ctx_mgr_entity_flush`.
pub fn amdgpu_ctx_mgr_entity_flush(adev: &mut amdgpu_device, mgr: &mut amdgpu_ctx_mgr, mut timeout: i64) -> i64 {
    for ctx in mgr.ctx_handles.values() {
        for c in ctx.entities.iter().flatten().flatten() {
            timeout = drm_sched::drm_sched_entity_flush(adev, c.entity, timeout);
        }
    }
    timeout
}

/// `amdgpu_ctx_mgr_fini` with `amdgpu_ctx_mgr_entity_fini`.
pub fn amdgpu_ctx_mgr_fini(adev: &mut amdgpu_device, mgr: &mut amdgpu_ctx_mgr) {
    for (_, mut ctx) in core::mem::take(&mut mgr.ctx_handles) {
        if ctx.refcount != 1 {
            Line::new().s("ctx is still alive").say();
            continue;
        }
        amdgpu_ctx_fini_stable_pstate(adev, &mut ctx);
        for c in ctx.entities.into_iter().flatten().flatten() {
            drm_sched::drm_sched_entity_fini(adev, c.entity);
            adev.sched_entities[c.entity] = None;
        }
    }
}
