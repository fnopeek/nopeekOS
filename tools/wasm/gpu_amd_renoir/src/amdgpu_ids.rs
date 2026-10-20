//! amdgpu/amdgpu_ids.c (Linux 6.18.55): PASIDs and the VMIDs a job runs
//! its VM under.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_sync::amdgpu_sync;
use crate::dma_fence::{self, Fence};
use crate::regs::*;
use alloc::vec::Vec;

/// `struct amdgpu_vmid`.
#[derive(Default)]
pub struct amdgpu_vmid {
    pub active: amdgpu_sync,
    pub last_flush: Option<Fence>,
    /// The `immediate` entity's fence context of the VM it belongs to.
    pub owner: u64,
    pub pd_gpu_addr: u64,
    pub flushed_updates: u64,
    pub current_gpu_reset_count: u32,
    pub gds_base: u32,
    pub gds_size: u32,
    pub gws_base: u32,
    pub gws_size: u32,
    pub oa_base: u32,
    pub oa_size: u32,
    pub pasid: u32,
    pub pasid_mapping: Option<Fence>,
}

/// `struct amdgpu_vmid_mgr`.
#[derive(Default)]
pub struct amdgpu_vmid_mgr {
    pub num_ids: u32,
    /// `ids_lru`: least recently used first.
    pub ids_lru: Vec<usize>,
    pub ids: [amdgpu_vmid; AMDGPU_NUM_VMID as usize],
    pub reserved_vmid: bool,
}

/// `amdgpu_pasid_alloc`: `xa_alloc_cyclic` over 1 .. 2^bits - 1.
pub fn amdgpu_pasid_alloc(adev: &mut amdgpu_device, bits: u32) -> i32 {
    if bits == 0 {
        return -22;
    }
    let max = (1u32 << bits) - 1;
    let used = &mut adev.vm_manager.pasids_used;
    let mut next = adev.vm_manager.pasid_xa_next.clamp(1, max);
    for _ in 0..max {
        if !used.contains(&next) {
            used.push(next);
            adev.vm_manager.pasid_xa_next = if next == max { 1 } else { next + 1 };
            return next as i32;
        }
        next = if next == max { 1 } else { next + 1 };
    }
    -16
}

/// `amdgpu_pasid_free`.
pub fn amdgpu_pasid_free(adev: &mut amdgpu_device, pasid: u32) {
    adev.vm_manager.pasids_used.retain(|&p| p != pasid);
}

/// `amdgpu_vmid_had_gpu_reset`.
pub fn amdgpu_vmid_had_gpu_reset(adev: &amdgpu_device, hub: usize, vmid: usize) -> bool {
    adev.vm_manager.id_mgr[hub].ids[vmid].current_gpu_reset_count != adev.gpu_reset_counter
}

/// `amdgpu_vmid_gds_switch_needed`.
fn amdgpu_vmid_gds_switch_needed(id: &amdgpu_vmid, job: &crate::amdgpu_job::amdgpu_job) -> bool {
    id.gds_base != job.gds_base || id.gds_size != job.gds_size || id.gws_base != job.gws_base
        || id.gws_size != job.gws_size || id.oa_base != job.oa_base || id.oa_size != job.oa_size
}

/// `amdgpu_vmid_compatible`.
fn amdgpu_vmid_compatible(id: &amdgpu_vmid, job: &crate::amdgpu_job::amdgpu_job) -> bool {
    id.pd_gpu_addr == job.vm_pd_addr && !amdgpu_vmid_gds_switch_needed(id, job)
}

/// `amdgpu_vmid_grab_idle`: an idle VMID, or the fence to wait for until
/// there is one.
fn amdgpu_vmid_grab_idle(adev: &mut amdgpu_device, r: usize) -> Result<(Option<usize>, Option<Fence>), i32> {
    let hub = adev.rings[r].vm_hub as usize;
    let vmid_wait = adev.rings[r].vmid_wait.clone();
    if !dma_fence::dma_fence_is_signaled(adev, &vmid_wait) {
        return Ok((None, Some(vmid_wait)));
    }
    let mut fences = Vec::new();
    // Check if we have an idle VMID
    let lru = adev.vm_manager.id_mgr[hub].ids_lru.clone();
    let ring = if adev.vm_manager.concurrent_flush { None } else { Some(r) };
    for &id in &lru {
        let mut active = core::mem::take(&mut adev.vm_manager.id_mgr[hub].ids[id].active);
        let f = crate::amdgpu_sync::amdgpu_sync_peek_fence(adev, &mut active, ring);
        adev.vm_manager.id_mgr[hub].ids[id].active = active;
        match f {
            Some(f) => fences.push(f),
            None => return Ok((Some(id), None)),
        }
    }
    // If we can't find a idle VMID to use, wait till one becomes
    // available
    let fence_context = adev.vm_manager.fence_context + adev.rings[r].idx as u64;
    let i = adev.rings[r].idx as usize;
    adev.vm_manager.seqno[i] = adev.vm_manager.seqno[i].wrapping_add(1);
    let seqno = adev.vm_manager.seqno[i];
    let array = dma_fence::dma_fence_array_create(fences, fence_context, seqno, true);
    adev.rings[r].vmid_wait = array.clone();
    Ok((None, Some(array)))
}

/// `amdgpu_vmid_grab_reserved`.
fn amdgpu_vmid_grab_reserved(adev: &mut amdgpu_device, vm: usize, r: usize, j: u64)
                             -> Result<(Option<usize>, Option<Fence>), i32> {
    let hub = adev.rings[r].vm_hub as usize;
    let fence_context = adev.fence_context + adev.rings[r].idx as u64;
    let v = adev.vm(vm);
    let mut needs_flush = v.use_cpu_for_update;
    let updates = crate::amdgpu_vm::amdgpu_vm_tlb_seq(v);
    let owner = crate::amdgpu_vm::immediate_context(adev, vm);
    let id = adev.vm(vm).reserved_vmid[hub].unwrap();
    let last_flush = adev.vm_manager.id_mgr[hub].ids[id].last_flush.clone();
    let lf_pending = match &last_flush {
        Some(f) => f.context != fence_context && !dma_fence::dma_fence_is_signaled(adev, f),
        None => true,
    };
    {
        let job = adev.jobs.get(&j).unwrap();
        let x = &adev.vm_manager.id_mgr[hub].ids[id];
        if x.owner != owner || !amdgpu_vmid_compatible(x, job) || x.flushed_updates < updates || lf_pending {
            needs_flush = true;
        }
    }
    if adev.vm_manager.id_mgr[hub].ids[id].owner != owner || (!adev.vm_manager.concurrent_flush && needs_flush) {
        let ring = if adev.vm_manager.concurrent_flush { None } else { Some(r) };
        adev.vm_manager.id_mgr[hub].ids[id].pd_gpu_addr = 0;
        let mut active = core::mem::take(&mut adev.vm_manager.id_mgr[hub].ids[id].active);
        let tmp = crate::amdgpu_sync::amdgpu_sync_peek_fence(adev, &mut active, ring);
        adev.vm_manager.id_mgr[hub].ids[id].active = active;
        if let Some(tmp) = tmp {
            return Ok((None, Some(tmp)));
        }
    }
    // Good we can use this VMID. Remember this submission as user of the
    // VMID.
    let finished = adev.jobs.get(&j).unwrap().base.s_fence.clone();
    let mut active = core::mem::take(&mut adev.vm_manager.id_mgr[hub].ids[id].active);
    let ret = crate::amdgpu_sync::amdgpu_sync_fence(adev, &mut active, finished.as_ref());
    adev.vm_manager.id_mgr[hub].ids[id].active = active;
    if ret != 0 {
        return Err(ret);
    }
    let job = adev.jobs.get_mut(&j).unwrap();
    job.vm_needs_flush = needs_flush;
    job.spm_update_needed = true;
    Ok((Some(id), None))
}

/// `amdgpu_vmid_grab_used`: a VMID the VM used before, if one fits.
fn amdgpu_vmid_grab_used(adev: &mut amdgpu_device, vm: usize, r: usize, j: u64) -> Result<Option<usize>, i32> {
    let hub = adev.rings[r].vm_hub as usize;
    let fence_context = adev.fence_context + adev.rings[r].idx as u64;
    let use_cpu = adev.vm(vm).use_cpu_for_update;
    let updates = crate::amdgpu_vm::amdgpu_vm_tlb_seq(adev.vm(vm));
    let owner = crate::amdgpu_vm::immediate_context(adev, vm);
    adev.jobs.get_mut(&j).unwrap().vm_needs_flush = use_cpu;
    // Check if we can use a VMID already assigned to this VM
    let lru = adev.vm_manager.id_mgr[hub].ids_lru.clone();
    for &id in lru.iter().rev() {
        let mut needs_flush = use_cpu;
        {
            let x = &adev.vm_manager.id_mgr[hub].ids[id];
            // Check all the prerequisites to using this VMID
            if x.owner != owner {
                continue;
            }
            if !amdgpu_vmid_compatible(x, adev.jobs.get(&j).unwrap()) {
                continue;
            }
        }
        let last_flush = adev.vm_manager.id_mgr[hub].ids[id].last_flush.clone();
        match &last_flush {
            None => needs_flush = true,
            Some(f) if f.context != fence_context && !dma_fence::dma_fence_is_signaled(adev, f) => {
                needs_flush = true
            }
            _ => {}
        }
        if adev.vm_manager.id_mgr[hub].ids[id].flushed_updates < updates {
            needs_flush = true;
        }
        if needs_flush && !adev.vm_manager.concurrent_flush {
            continue;
        }
        // Good, we can use this VMID. Remember this submission as user of
        // the VMID.
        let finished = adev.jobs.get(&j).unwrap().base.s_fence.clone();
        let mut active = core::mem::take(&mut adev.vm_manager.id_mgr[hub].ids[id].active);
        let ret = crate::amdgpu_sync::amdgpu_sync_fence(adev, &mut active, finished.as_ref());
        adev.vm_manager.id_mgr[hub].ids[id].active = active;
        if ret != 0 {
            return Err(ret);
        }
        adev.jobs.get_mut(&j).unwrap().vm_needs_flush |= needs_flush;
        return Ok(Some(id));
    }
    Ok(None)
}

/// `amdgpu_vmid_grab`: a VMID for job `j` of `vm` on ring `r`, or a fence
/// to wait for first.
pub fn amdgpu_vmid_grab(adev: &mut amdgpu_device, vm: usize, r: usize, j: u64) -> Result<Option<Fence>, i32> {
    let hub = adev.rings[r].vm_hub as usize;
    let (idle, fence) = amdgpu_vmid_grab_idle(adev, r)?;
    let Some(idle) = idle else { return Ok(fence) };
    let id = if amdgpu_vmid_uses_reserved(adev, vm, hub) {
        let (id, fence) = amdgpu_vmid_grab_reserved(adev, vm, r, j)?;
        match id {
            Some(id) => id,
            None => return Ok(fence),
        }
    } else {
        let id = match amdgpu_vmid_grab_used(adev, vm, r, j)? {
            Some(id) => id,
            None => {
                // Still no ID to use? Then use the idle one found earlier
                let finished = adev.jobs.get(&j).unwrap().base.s_fence.clone();
                let mut active = core::mem::take(&mut adev.vm_manager.id_mgr[hub].ids[idle].active);
                let ret = crate::amdgpu_sync::amdgpu_sync_fence(adev, &mut active, finished.as_ref());
                adev.vm_manager.id_mgr[hub].ids[idle].active = active;
                if ret != 0 {
                    return Err(ret);
                }
                adev.jobs.get_mut(&j).unwrap().vm_needs_flush = true;
                idle
            }
        };
        let lru = &mut adev.vm_manager.id_mgr[hub].ids_lru;
        lru.retain(|&x| x != id);
        lru.push(id);
        id
    };
    let gds = amdgpu_vmid_gds_switch_needed(&adev.vm_manager.id_mgr[hub].ids[id], adev.jobs.get(&j).unwrap());
    adev.jobs.get_mut(&j).unwrap().gds_switch_needed = gds;
    if adev.jobs.get(&j).unwrap().vm_needs_flush {
        let seq = crate::amdgpu_vm::amdgpu_vm_tlb_seq(adev.vm(vm));
        let x = &mut adev.vm_manager.id_mgr[hub].ids[id];
        x.flushed_updates = seq;
        x.last_flush = None;
    }
    let pasid = adev.vm(vm).pasid;
    let owner = crate::amdgpu_vm::immediate_context(adev, vm);
    let job = adev.jobs.get_mut(&j).unwrap();
    job.vmid = id as u32;
    job.pasid = pasid;
    let x = &mut adev.vm_manager.id_mgr[hub].ids[id];
    x.gds_base = job.gds_base;
    x.gds_size = job.gds_size;
    x.gws_base = job.gws_base;
    x.gws_size = job.gws_size;
    x.oa_base = job.oa_base;
    x.oa_size = job.oa_size;
    x.pd_gpu_addr = job.vm_pd_addr;
    x.owner = owner;
    Ok(None)
}

/// `amdgpu_vmid_uses_reserved`.
pub fn amdgpu_vmid_uses_reserved(adev: &amdgpu_device, vm: usize, hub: usize) -> bool {
    adev.vm(vm).reserved_vmid[hub].is_some()
}

/// `amdgpu_vmid_alloc_reserved`.
pub fn amdgpu_vmid_alloc_reserved(adev: &mut amdgpu_device, vm: usize, hub: usize) -> i32 {
    if adev.vm(vm).reserved_vmid[hub].is_some() {
        return 0;
    }
    let m = &mut adev.vm_manager.id_mgr[hub];
    if m.reserved_vmid {
        return -2;
    }
    let id = m.ids_lru.remove(0);
    m.reserved_vmid = true;
    adev.vm_mut(vm).reserved_vmid[hub] = Some(id);
    0
}

/// `amdgpu_vmid_free_reserved`.
pub fn amdgpu_vmid_free_reserved(adev: &mut amdgpu_device, vm: usize, hub: usize) {
    if let Some(id) = adev.vm_mut(vm).reserved_vmid[hub].take() {
        let m = &mut adev.vm_manager.id_mgr[hub];
        m.ids_lru.insert(0, id);
        m.reserved_vmid = false;
    }
}

/// `amdgpu_vmid_reset`.
pub fn amdgpu_vmid_reset(adev: &mut amdgpu_device, hub: usize, vmid: usize) {
    let id = &mut adev.vm_manager.id_mgr[hub].ids[vmid];
    id.owner = 0;
    id.gds_base = 0;
    id.gds_size = 0;
    id.gws_base = 0;
    id.gws_size = 0;
    id.oa_base = 0;
    id.oa_size = 0;
}

/// `amdgpu_vmid_reset_all`.
pub fn amdgpu_vmid_reset_all(adev: &mut amdgpu_device) {
    for i in 0..AMDGPU_MAX_VMHUBS as usize {
        for j in 1..adev.vm_manager.id_mgr[i].num_ids as usize {
            amdgpu_vmid_reset(adev, i, j);
        }
    }
}

/// `amdgpu_vmid_mgr_init`: for GC < 10 SDMA uses the MMHUB, so both hubs
/// manage only the VMIDs KFD does not use.
pub fn amdgpu_vmid_mgr_init(adev: &mut amdgpu_device) {
    for i in 0..AMDGPU_MAX_VMHUBS as usize {
        let m = &mut adev.vm_manager.id_mgr[i];
        m.ids_lru.clear();
        m.num_ids = adev.vm_manager.first_kfd_vmid;
        for j in 1..m.num_ids as usize {
            amdgpu_vmid_reset(adev, i, j);
            let m = &mut adev.vm_manager.id_mgr[i];
            m.ids[j].active = crate::amdgpu_sync::amdgpu_sync_create();
            m.ids_lru.push(j);
        }
    }
}

/// `amdgpu_vmid_mgr_fini`.
pub fn amdgpu_vmid_mgr_fini(adev: &mut amdgpu_device) {
    for m in adev.vm_manager.id_mgr.iter_mut() {
        for id in m.ids.iter_mut() {
            crate::amdgpu_sync::amdgpu_sync_free(&mut id.active);
            id.last_flush = None;
            id.pasid_mapping = None;
        }
    }
}

/// The ring's `vmid_wait` at `amdgpu_ring_init`.
pub fn vmid_wait_init() -> Fence {
    dma_fence::dma_fence_get_stub()
}
