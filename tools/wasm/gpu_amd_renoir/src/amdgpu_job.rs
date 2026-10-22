//! amdgpu/amdgpu_job.c (Linux 6.18.55) and the job side of
//! amdgpu_device.c (gang switch, enforced isolation): what the scheduler
//! runs. A job lives in `adev.jobs` under its key until `free_job`.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ib::amdgpu_ib;
use crate::amdgpu_sync::amdgpu_sync;
use crate::dma_fence::{self, Fence};
use crate::drm_sched::{self, drm_gpu_sched_stat, drm_sched_job};
use crate::linux::Line;
use crate::regs::*;
use alloc::vec::Vec;

/// `struct amdgpu_job`.
#[derive(Default)]
pub struct amdgpu_job {
    pub base: drm_sched_job,
    /// The VM, by its key in `adev.vms`.
    pub vm: Option<usize>,
    pub explicit_sync: amdgpu_sync,
    /// `hw_fence` once `amdgpu_fence_emit` has initialized it.
    pub hw_fence: Option<Fence>,
    pub gang_submit: Option<Fence>,
    pub preamble_status: u32,
    pub preemption_status: u32,
    pub vm_needs_flush: bool,
    pub gds_switch_needed: bool,
    pub spm_update_needed: bool,
    pub vm_pd_addr: u64,
    pub vmid: u32,
    pub pasid: u32,
    pub gds_base: u32,
    pub gds_size: u32,
    pub gws_base: u32,
    pub gws_size: u32,
    pub oa_base: u32,
    pub oa_size: u32,
    pub generation: u64,
    pub uf_addr: u64,
    pub uf_sequence: u64,
    pub shadow_va: u64,
    pub csa_va: u64,
    pub gds_va: u64,
    pub init_shadow: bool,
    pub job_run_counter: u32,
    pub enforce_isolation: bool,
    pub run_cleaner_shader: bool,
    pub num_ibs: u32,
    /// Room for the IBs; the first `num_ibs` are in use.
    pub ibs: Vec<amdgpu_ib>,
}

/// `struct amdgpu_isolation`.
pub struct amdgpu_isolation {
    pub owner: u64,
    pub spearhead: Fence,
    pub active: amdgpu_sync,
    pub prev: amdgpu_sync,
}

impl Default for amdgpu_isolation {
    fn default() -> Self {
        amdgpu_isolation {
            owner: 0,
            spearhead: dma_fence::dma_fence_get_stub(),
            active: amdgpu_sync::default(),
            prev: amdgpu_sync::default(),
        }
    }
}

pub fn job(adev: &mut amdgpu_device, j: u64) -> &mut amdgpu_job {
    adev.jobs.get_mut(&j).unwrap()
}

/// `AMDGPU_JOB_GET_VMID`.
pub fn amdgpu_job_get_vmid(adev: &amdgpu_device, j: Option<u64>) -> u32 {
    j.map_or(0, |j| adev.jobs.get(&j).unwrap().vmid)
}

/// `amdgpu_job_do_core_dump`: the IP blocks with a `dump_ip_state` (VCN
/// and JPEG are ported), then `amdgpu_coredump`.
fn amdgpu_job_do_core_dump(adev: &mut amdgpu_device, r: usize) {
    Line::new().s("Dumping IP State").say();
    for i in 0..adev.ip_blocks.len() {
        match adev.ip_blocks[i].version.type_ {
            AMD_IP_BLOCK_TYPE_VCN => crate::vcn_v2_0::amdgpu_vcn_dump_ip_state(adev),
            AMD_IP_BLOCK_TYPE_JPEG => crate::jpeg_v2_0::amdgpu_jpeg_dump_ip_state(adev),
            _ => {}
        }
    }
    Line::new().s("Dumping IP State Completed").say();
    amdgpu_coredump(adev, r);
}

/// The ring contents a dump prints: the dwords before the read pointer and
/// up to the write pointer.
const COREDUMP_RING_DWORDS_BEFORE_RPTR: u64 = 32;
const COREDUMP_RING_DWORDS_MAX: u64 = 256;

/// `amdgpu_coredump` with `amdgpu_devcoredump_read`: no devcoredump file,
/// so the parts that say where a ring stands go to the log — the timed-out
/// ring, the last VM fault, the IP dump, and the timed-out ring's buffer.
fn amdgpu_coredump(adev: &amdgpu_device, r: usize) {
    let ring = &adev.rings[r];
    Line::new().s("Ring timed out details").say();
    Line::new().s("IP Type: ").dec(ring.funcs.type_ as u64).s(" Ring Name: ").s(&ring.name).say();
    let f = &adev.vm_manager.fault_info;
    Line::new().s("[").s(if f.vmhub != 0 { "mmhub" } else { "gfxhub" }).s("] Page fault observed").say();
    Line::new().s("Faulty page starting at address: 0x").hexw((f.addr >> 32) as u32, 8).hexw(f.addr as u32, 8).say();
    Line::new().s("Protection fault status register: 0x").hexu(f.status).say();
    Line::new().s("IP Dump").say();
    for b in adev.ip_blocks.iter() {
        let print: fn(&amdgpu_device) = match b.version.type_ {
            AMD_IP_BLOCK_TYPE_VCN => crate::vcn_v2_0::amdgpu_vcn_print_ip_state,
            AMD_IP_BLOCK_TYPE_JPEG => crate::jpeg_v2_0::amdgpu_jpeg_print_ip_state,
            _ => continue,
        };
        Line::new().s("IP: ").bytes(b.version.funcs_name).say();
        print(adev);
    }
    Line::new().s("Ring buffer information").say();
    Line::new().s("ring name: ").s(&ring.name).say();
    let Some((rptr, wptr)) = crate::amdgpu_ring::amdgpu_ring_get_rptr_wptr(adev, r) else { return };
    Line::new().s("Rptr: 0x").hexu(rptr as u32).s(" Wptr: 0x").hexu(wptr as u32).s(" RB mask: ").hexu(ring.buf_mask)
        .say();
    Line::new().s("Ring size in dwords: ").dec((ring.ring_size / 4) as u64).say();
    let Some(k) = ring.ring else { return };
    let mask = ring.buf_mask as u64;
    let first = rptr.saturating_sub(COREDUMP_RING_DWORDS_BEFORE_RPTR);
    let n = wptr.wrapping_sub(first) & mask;
    Line::new().s("Ring contents").say();
    for d in 0..n.min(COREDUMP_RING_DWORDS_MAX) {
        let at = (first + d) & mask;
        Line::new().s("0x").hexu((at * 4) as u32).s(" 0x").hexu(crate::amdgpu_gart::kptr_r32(adev, k, at * 4)).say();
    }
}

/// `amdgpu_job_timedout`: the device is not unplugged and not a VF. The
/// core dump first, then soft recovery, then a reset of the ring's queue,
/// then the device reset.
pub fn amdgpu_job_timedout(adev: &mut amdgpu_device, j: u64) -> drm_gpu_sched_stat {
    let r = job(adev, j).base.sched.unwrap();
    let name = adev.rings[r].sched.name.clone();
    // Do the coredump immediately after a job timeout to get a very close
    // dump/snapshot/representation of GPU's current error status
    amdgpu_job_do_core_dump(adev, r);
    let (vmid, parent) = (job(adev, j).vmid, job(adev, j).hw_fence.clone());
    if crate::tables::amdgpu_gpu_recovery[0] != 0
        && crate::amdgpu_ring::amdgpu_ring_is_reset_type_supported(adev, r, AMDGPU_RESET_TYPE_SOFT_RESET)
        && crate::amdgpu_ring::amdgpu_ring_soft_recovery(adev, r, vmid, parent.as_ref())
    {
        Line::new().s("ring ").s(&name).s(" timeout, but soft recovered").say();
        return drm_gpu_sched_stat::RESET;
    }
    let drv = &adev.rings[r].fence_drv;
    Line::new().s("ring ").s(&name).s(" timeout, signaled seq=").dec(drv.last_seq as u64).s(", emitted seq=")
        .dec(drv.sync_seq as u64).say();
    // attempt a per ring reset
    if let Some(hw) = parent.as_ref().filter(|_| {
        crate::tables::amdgpu_gpu_recovery[0] != 0
            && crate::amdgpu_ring::amdgpu_ring_is_reset_type_supported(adev, r, AMDGPU_RESET_TYPE_PER_QUEUE)
            && crate::amdgpu_ring::amdgpu_ring_has_reset(adev, r)
    }) {
        Line::new().s("Starting ").s(&name).s(" ring reset").say();
        if crate::amdgpu_ring::amdgpu_ring_reset(adev, r, vmid, hw) == 0 {
            adev.gpu_reset_counter += 1;
            Line::new().s("Ring ").s(&name).s(" reset succeeded").say();
            return drm_gpu_sched_stat::RESET;
        }
        Line::new().s("Ring ").s(&name).s(" reset failed").say();
    }
    let finished = job(adev, j).base.s_fence.clone().unwrap();
    if dma_fence::dma_fence_get_status(adev, &finished) == 0 {
        dma_fence::dma_fence_set_error(&finished, -62);
    }
    if crate::amdgpu_ring::amdgpu_device_should_recover_gpu() {
        // method NONE, source JOB, no forced full reset, coredump skipped
        let r = crate::amdgpu_device::amdgpu_device_gpu_recover(adev, Some(j));
        if r != 0 {
            Line::new().s("GPU Recovery Failed: ").dec((-r) as u64).say();
        }
    } else {
        drm_sched::drm_sched_suspend_timeout(adev, r);
    }
    drm_gpu_sched_stat::RESET
}

/// `amdgpu_job_alloc`: the new job's key.
pub fn amdgpu_job_alloc(adev: &mut amdgpu_device, vm: Option<usize>, entity: Option<usize>, owner: u64,
                        num_ibs: u32) -> Result<u64, i32> {
    if num_ibs == 0 {
        return Err(-22);
    }
    adev.next_job += 1;
    let j = adev.next_job;
    let generation = crate::amdgpu_vm::amdgpu_vm_generation(adev, vm);
    adev.jobs.insert(j, amdgpu_job {
        vm,
        explicit_sync: crate::amdgpu_sync::amdgpu_sync_create(),
        generation,
        vm_pd_addr: AMDGPU_BO_INVALID_OFFSET,
        ibs: (0..num_ibs).map(|_| amdgpu_ib::new()).collect(),
        ..Default::default()
    });
    let Some(e) = entity else { return Ok(j) };
    let r = drm_sched::drm_sched_job_init(adev, j, e, 1, owner);
    if r != 0 {
        adev.jobs.remove(&j);
        return Err(r);
    }
    Ok(j)
}

/// `amdgpu_job_alloc_with_ib`.
pub fn amdgpu_job_alloc_with_ib(adev: &mut amdgpu_device, entity: Option<usize>, owner: u64, size: u32,
                                pool_type: u32) -> Result<u64, i32> {
    let j = amdgpu_job_alloc(adev, None, entity, owner, 1)?;
    job(adev, j).num_ibs = 1;
    let mut ib = amdgpu_ib::new();
    let r = crate::amdgpu_ib::amdgpu_ib_get(adev, size, pool_type, &mut ib);
    if r != 0 {
        if entity.is_some() {
            drm_sched::drm_sched_job_cleanup(adev, j);
        }
        adev.jobs.remove(&j);
        return Err(r);
    }
    job(adev, j).ibs[0] = ib;
    Ok(j)
}

/// `amdgpu_job_free_resources`: the IBs go back once the job's fence
/// signals.
pub fn amdgpu_job_free_resources(adev: &mut amdgpu_device, j: u64) {
    let f = job(adev, j).base.s_fence.clone().or_else(|| job(adev, j).hw_fence.clone());
    let mut ibs = core::mem::take(&mut job(adev, j).ibs);
    let n = job(adev, j).num_ibs as usize;
    for ib in ibs[..n].iter_mut() {
        crate::amdgpu_ib::amdgpu_ib_free(adev, ib, f.clone());
    }
    job(adev, j).ibs = ibs;
}

/// `amdgpu_job_free_cb`.
pub fn amdgpu_job_free_cb(adev: &mut amdgpu_device, j: u64) {
    drm_sched::drm_sched_job_cleanup(adev, j);
    crate::amdgpu_sync::amdgpu_sync_free(&mut job(adev, j).explicit_sync);
    adev.jobs.remove(&j);
}

/// `amdgpu_job_set_gang_leader`.
pub fn amdgpu_job_set_gang_leader(adev: &mut amdgpu_device, j: u64, leader: u64) {
    let fence = job(adev, leader).base.s_fence.as_ref()
        .map(|f| crate::drm_sched::to_drm_sched_fence(f).unwrap().scheduled);
    if job(adev, j).gang_submit.is_some() {
        Line::new().s("WARNING: amdgpu_job_set_gang_leader on a job in a gang").say();
    }
    job(adev, j).gang_submit = fence;
}

/// `amdgpu_job_free`.
pub fn amdgpu_job_free(adev: &mut amdgpu_device, j: u64) {
    if job(adev, j).base.entity.is_some() {
        drm_sched::drm_sched_job_cleanup(adev, j);
    }
    amdgpu_job_free_resources(adev, j);
    crate::amdgpu_sync::amdgpu_sync_free(&mut job(adev, j).explicit_sync);
    adev.jobs.remove(&j);
}

/// `amdgpu_job_submit`: the job's finished fence.
pub fn amdgpu_job_submit(adev: &mut amdgpu_device, j: u64) -> Fence {
    drm_sched::drm_sched_job_arm(adev, j);
    let f = job(adev, j).base.s_fence.clone().unwrap();
    amdgpu_job_free_resources(adev, j);
    drm_sched::drm_sched_entity_push_job(adev, j);
    f
}

/// `amdgpu_job_submit_direct`.
pub fn amdgpu_job_submit_direct(adev: &mut amdgpu_device, j: u64, r: usize) -> Result<Fence, i32> {
    job(adev, j).base.sched = Some(r);
    let n = job(adev, j).num_ibs as usize;
    let ibs = job(adev, j).ibs[..n].to_vec();
    let fence = crate::amdgpu_ib::amdgpu_ib_schedule(adev, r, &ibs, Some(j))?;
    amdgpu_job_free(adev, j);
    Ok(fence)
}

/// `amdgpu_device_get_gang`.
fn amdgpu_device_get_gang(adev: &amdgpu_device) -> Fence {
    adev.gang_submit.clone()
}

/// `amdgpu_device_switch_gang`: the gang to wait for first, if any.
pub fn amdgpu_device_switch_gang(adev: &mut amdgpu_device, gang: &Fence) -> Option<Fence> {
    let old = amdgpu_device_get_gang(adev);
    if alloc::rc::Rc::ptr_eq(&old, gang) {
        return None;
    }
    if !dma_fence::dma_fence_is_signaled(adev, &old) {
        return Some(old);
    }
    adev.gang_submit = gang.clone();
    None
}

/// `amdgpu_device_enforce_isolation`: one partition, `isolation[0]`.
pub fn amdgpu_device_enforce_isolation(adev: &mut amdgpu_device, r: usize, j: u64) -> Option<Fence> {
    let t = adev.rings[r].funcs.type_;
    if t != AMDGPU_RING_TYPE_GFX && t != AMDGPU_RING_TYPE_COMPUTE {
        return None;
    }
    let finished = job(adev, j).base.s_fence.clone().unwrap();
    let s = crate::drm_sched::to_drm_sched_fence(&finished).unwrap();
    let owner = if job(adev, j).enforce_isolation { s.owner } else { !0u64 };
    let spearhead = adev.isolation.spearhead.clone();
    if !alloc::rc::Rc::ptr_eq(&s.scheduled, &spearhead) && !dma_fence::dma_fence_is_signaled(adev, &spearhead) {
        return Some(spearhead);
    }
    if adev.isolation.owner != owner {
        // Wait for any gang to be assembled before switching to a
        // different owner or otherwise we could deadlock the submissions.
        if job(adev, j).gang_submit.is_none() {
            let dep = amdgpu_device_get_gang(adev);
            if !dma_fence::dma_fence_is_signaled(adev, &dep) {
                return Some(dep);
            }
        }
        adev.isolation.spearhead = s.scheduled.clone();
        let mut active = core::mem::take(&mut adev.isolation.active);
        crate::amdgpu_sync::amdgpu_sync_move(&mut active, &mut adev.isolation.prev);
        adev.isolation.owner = owner;
    }
    // Specifying the ring here helps to pipeline submissions even when
    // isolation is enabled. If that is not desired for testing NULL can be
    // used instead of the ring to enforce a CPU round trip while switching
    // between clients.
    let mut prev = core::mem::take(&mut adev.isolation.prev);
    let dep = crate::amdgpu_sync::amdgpu_sync_peek_fence(adev, &mut prev, Some(r));
    adev.isolation.prev = prev;
    let mut active = core::mem::take(&mut adev.isolation.active);
    let ret = crate::amdgpu_sync::amdgpu_sync_fence(adev, &mut active, Some(&finished));
    adev.isolation.active = active;
    if ret != 0 {
        Line::new().s("OOM tracking isolation").say();
    }
    dep
}

/// `amdgpu_job_prepare_job`.
pub fn amdgpu_job_prepare_job(adev: &mut amdgpu_device, j: u64, e: usize) -> Option<Fence> {
    let r = adev.sched_entities[e].as_ref().unwrap().rq.unwrap().0;
    let mut err = drm_sched::drm_sched_entity_error(adev, e);
    if err == 0 {
        if let Some(gang) = job(adev, j).gang_submit.clone() {
            if let Some(f) = amdgpu_device_switch_gang(adev, &gang) {
                return Some(f);
            }
        }
        if let Some(f) = amdgpu_device_enforce_isolation(adev, r, j) {
            return Some(f);
        }
        if let Some(vm) = job(adev, j).vm {
            if job(adev, j).vmid == 0 {
                match crate::amdgpu_ids::amdgpu_vmid_grab(adev, vm, r, j) {
                    Ok(f) => return f,
                    Err(e) => {
                        Line::new().s("Error getting VM ID (").dec((-e) as u64).s(")").say();
                        err = e;
                    }
                }
            }
        }
        if err == 0 {
            return None;
        }
    }
    let finished = job(adev, j).base.s_fence.clone().unwrap();
    dma_fence::dma_fence_set_error(&finished, err);
    None
}

/// `amdgpu_job_run`: the hardware fence, none, or an error.
pub fn amdgpu_job_run(adev: &mut amdgpu_device, j: u64) -> Result<Option<Fence>, i32> {
    let r = job(adev, j).base.sched.unwrap();
    let finished = job(adev, j).base.s_fence.clone().unwrap();
    let vm = job(adev, j).vm;
    // Skip job if VRAM is lost and never resubmit gangs
    let generation = crate::amdgpu_vm::amdgpu_vm_generation(adev, vm);
    if job(adev, j).generation != generation || (job(adev, j).job_run_counter != 0 && job(adev, j).gang_submit.is_some()) {
        dma_fence::dma_fence_set_error(&finished, -125);
    }
    let mut fence = Ok(None);
    if finished.error() < 0 {
        Line::new().s("Skip scheduling IBs in ring(").s(&adev.rings[r].name.clone()).s(")").dbg();
    } else {
        let n = job(adev, j).num_ibs as usize;
        let ibs = job(adev, j).ibs[..n].to_vec();
        fence = crate::amdgpu_ib::amdgpu_ib_schedule(adev, r, &ibs, Some(j)).map(Some);
        if let Err(e) = fence {
            Line::new().s("Error scheduling IBs (").dec((-e) as u64).s(") in ring(").s(&adev.rings[r].name.clone())
                .s(")").say();
        }
    }
    job(adev, j).job_run_counter += 1;
    amdgpu_job_free_resources(adev, j);
    fence
}
