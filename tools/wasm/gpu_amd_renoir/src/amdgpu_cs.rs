//! amdgpu/amdgpu_cs.c and the client side of amdgpu_kms.c (Linux
//! 6.18.55): a client with its own VM, contexts and buffer list, and its
//! command submission, the path of `amdgpu_cs_ioctl` for IB chunks.

use crate::amdgpu_ctx::{self, amdgpu_ctx_mgr};
use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_job;
use crate::amdgpu_object as obj;
use crate::amdgpu_sync::amdgpu_sync;
use crate::amdgpu_vm;
use crate::dma_fence::Fence;
use crate::drm_sched;
use crate::linux::Line;
use crate::regs::*;
use alloc::vec::Vec;

/// `struct amdgpu_fpriv`: a client.
pub struct amdgpu_fpriv {
    pub vm: usize,
    /// The bo_vas, by base key.
    pub prt_va: usize,
    pub seq64_va: Option<usize>,
    pub ctx_mgr: amdgpu_ctx_mgr,
}

/// `struct drm_amdgpu_cs_chunk_ib`.
#[derive(Clone, Copy, Default)]
pub struct drm_amdgpu_cs_chunk_ib {
    pub flags: u32,
    pub va_start: u64,
    pub ib_bytes: u32,
    pub ip_type: u32,
    pub ip_instance: u32,
    pub ring: u32,
}

/// `amdgpu_seq64_get_va_base`.
fn amdgpu_seq64_get_va_base(adev: &amdgpu_device) -> u64 {
    // AMDGPU_VA_RESERVED_SEQ64_START
    let csa_start = (adev.vm_manager.max_pfn << AMDGPU_GPU_PAGE_SHIFT) - AMDGPU_VA_RESERVED_CSA_SIZE as u64;
    obj::amdgpu_gmc_sign_extend(csa_start - AMDGPU_VA_RESERVED_SEQ64_SIZE as u64)
}

/// `amdgpu_seq64_map`.
fn amdgpu_seq64_map(adev: &mut amdgpu_device, vm: usize) -> Result<usize, i32> {
    let Some(bo) = adev.seq64.sbo else { return Err(-22) };
    let bo_va = amdgpu_vm::amdgpu_vm_bo_add(adev, vm, Some(bo));
    let seq64_addr = amdgpu_seq64_get_va_base(adev) & AMDGPU_GMC_HOLE_MASK;
    let r = amdgpu_vm::amdgpu_vm_bo_map(adev, bo_va, seq64_addr, 0, AMDGPU_VA_RESERVED_SEQ64_SIZE as u64,
                                        AMDGPU_VM_PAGE_READABLE | AMDGPU_VM_MTYPE_UC);
    if r != 0 {
        Line::new().s("failed to do bo_map on userq sem, err=").dec((-r) as u64).say();
        amdgpu_vm::amdgpu_vm_bo_del(adev, bo_va);
        return Err(r);
    }
    let r = amdgpu_vm::amdgpu_vm_bo_update(adev, bo_va, false);
    if r != 0 {
        Line::new().s("failed to do vm_bo_update on userq sem").say();
        amdgpu_vm::amdgpu_vm_bo_del(adev, bo_va);
        return Err(r);
    }
    Ok(bo_va)
}

/// `amdgpu_driver_open_kms`: no partitions, no CSA without MCBP, no user
/// queues.
pub fn amdgpu_driver_open_kms(adev: &mut amdgpu_device) -> Result<amdgpu_fpriv, i32> {
    let mut pasid = crate::amdgpu_ids::amdgpu_pasid_alloc(adev, 16);
    if pasid < 0 {
        Line::new().s("No more PASIDs available!").say();
        pasid = 0;
    }
    let vm = match amdgpu_vm::amdgpu_vm_init(adev, pasid as u32) {
        Ok(vm) => vm,
        Err(r) => {
            if pasid != 0 {
                crate::amdgpu_ids::amdgpu_pasid_free(adev, pasid as u32);
            }
            return Err(r);
        }
    };
    let prt_va = amdgpu_vm::amdgpu_vm_bo_add(adev, vm, None);
    let seq64_va = match amdgpu_seq64_map(adev, vm) {
        Ok(v) => v,
        Err(r) => {
            amdgpu_vm::amdgpu_vm_fini(adev, vm);
            if pasid != 0 {
                crate::amdgpu_ids::amdgpu_pasid_free(adev, pasid as u32);
            }
            return Err(r);
        }
    };
    Ok(amdgpu_fpriv { vm, prt_va, seq64_va: Some(seq64_va), ctx_mgr: amdgpu_ctx_mgr::default() })
}

/// `amdgpu_driver_postclose_kms`: the PASID goes once the root's fences
/// signal (`amdgpu_pasid_free_delayed`).
pub fn amdgpu_driver_postclose_kms(adev: &mut amdgpu_device, mut fpriv: amdgpu_fpriv) {
    if let Some(v) = fpriv.seq64_va.take() {
        amdgpu_vm::amdgpu_vm_bo_del(adev, v);
    }
    let vm = fpriv.vm;
    let pasid = adev.vm(vm).pasid;
    let pd = amdgpu_vm::base(adev, adev.vm(vm).root).bo.unwrap();
    let pd = obj::amdgpu_bo_ref(adev, pd);
    amdgpu_vm::amdgpu_vm_bo_del(adev, fpriv.prt_va);
    amdgpu_ctx::amdgpu_ctx_mgr_fini(adev, &mut fpriv.ctx_mgr);
    amdgpu_vm::amdgpu_vm_fini(adev, vm);
    if pasid != 0 {
        // amdgpu_pasid_free_delayed
        let resv = obj::amdgpu_bo_resv(adev, pd).clone();
        match crate::dma_resv::dma_resv_get_singleton(adev, &resv,
                                                      crate::dma_resv::dma_resv_usage::DMA_RESV_USAGE_BOOKKEEP) {
            None => crate::amdgpu_ids::amdgpu_pasid_free(adev, pasid),
            Some(f) => {
                crate::dma_fence::dma_fence_wait(adev, &f);
                crate::amdgpu_ids::amdgpu_pasid_free(adev, pasid);
            }
        }
    }
    obj::amdgpu_bo_unref(adev, pd);
}

/// `amdgpu_vm_bo_find`: the bo_va of `bo` in `vm`.
pub fn amdgpu_vm_bo_find(adev: &amdgpu_device, vm: usize, bo: usize) -> Option<usize> {
    let mut b = obj::bo(adev, bo).vm_bo;
    while let Some(x) = b {
        let base = amdgpu_vm::base(adev, x);
        if base.vm == vm {
            return Some(x);
        }
        b = base.next;
    }
    None
}

/// `struct amdgpu_cs_parser`, the part IB chunks use.
struct amdgpu_cs_parser {
    ctx_id: u32,
    entities: Vec<usize>,
    jobs: Vec<Option<u64>>,
    gang_leader_idx: usize,
    sync: amdgpu_sync,
    fence: Option<Fence>,
    /// `bo_list`: each buffer and its bo_va.
    bo_list: Vec<(usize, Option<usize>)>,
    /// The objects `exec` locked: the page directory, then the list.
    locked: Vec<usize>,
}

/// `amdgpu_cs_job_idx`.
fn amdgpu_cs_job_idx(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser,
                     chunk_ib: &drm_amdgpu_cs_chunk_ib) -> Result<usize, i32> {
    let ctx = fpriv.ctx_mgr.ctx_handles.get_mut(&p.ctx_id).ok_or(-22)?;
    let entity = amdgpu_ctx::amdgpu_ctx_get_entity(adev, ctx, chunk_ib.ip_type, chunk_ib.ip_instance, chunk_ib.ring)?;
    if adev.sched_entities[entity].as_ref().unwrap().rq.is_none() {
        return Err(-22);
    }
    // Check if we can add this IB to some existing job
    if let Some(i) = p.entities.iter().position(|&e| e == entity) {
        return Ok(i);
    }
    // If not increase the gang size if possible
    if p.entities.len() == AMDGPU_CS_GANG_SIZE as usize {
        return Err(-22);
    }
    p.entities.push(entity);
    Ok(p.entities.len() - 1)
}

/// `amdgpu_cs_ib_packet_size_max_dw` for the engines with IB packets.
const AMDGPU_GFX_SDMA_IB_PACKET_SIZE_MAX_DW: u32 = 0x0fffff;

/// `amdgpu_cs_bo_validate`: pinned buffers stay, others go to their
/// preferred domain (no move threshold to keep: VRAM is all visible).
fn amdgpu_cs_bo_validate(adev: &mut amdgpu_device, bo: usize) -> i32 {
    if obj::bo(adev, bo).tbo.pin_count != 0 {
        return 0;
    }
    let domain = obj::bo(adev, bo).preferred_domains;
    obj::amdgpu_bo_placement_from_domain(adev, bo, domain);
    let placement = obj::bo(adev, bo).placements.clone();
    let mut r = crate::amdgpu_ttm::ttm_bo_validate(adev, bo, &placement);
    let allowed = obj::bo(adev, bo).allowed_domains;
    if r == -12 && domain != allowed {
        obj::amdgpu_bo_placement_from_domain(adev, bo, allowed);
        let placement = obj::bo(adev, bo).placements.clone();
        r = crate::amdgpu_ttm::ttm_bo_validate(adev, bo, &placement);
    }
    r
}

/// `amdgpu_cs_ioctl` for `ibs` on context `ctx_id`, with the buffers in
/// `bo_list`: the submission's sequence number and fence.
pub fn amdgpu_cs_ioctl(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, ctx_id: u32,
                       ibs: &[drm_amdgpu_cs_chunk_ib], bo_list: &[usize]) -> Result<(u64, Fence), i32> {
    if !adev.accel_working {
        return Err(-16);
    }
    // amdgpu_cs_parser_init
    if ibs.is_empty() {
        return Err(-22);
    }
    let ctx = fpriv.ctx_mgr.ctx_handles.get(&ctx_id).ok_or(-22)?;
    if ctx.guilty.get() != 0 {
        return Err(-125);
    }
    let mut p = amdgpu_cs_parser {
        ctx_id,
        entities: Vec::new(),
        jobs: Vec::new(),
        gang_leader_idx: 0,
        sync: crate::amdgpu_sync::amdgpu_sync_create(),
        fence: None,
        bo_list: Vec::new(),
        locked: Vec::new(),
    };
    let r = (|| -> Result<u64, i32> {
        amdgpu_cs_pass1(adev, fpriv, &mut p, ibs, bo_list)?;
        amdgpu_cs_pass2(adev, fpriv, &mut p, ibs)?;
        amdgpu_cs_parser_bos(adev, fpriv, &mut p)?;
        // amdgpu_cs_patch_jobs: compute and SDMA rings neither parse nor
        // patch the IBs.
        amdgpu_cs_vm_handling(adev, fpriv, &mut p)?;
        amdgpu_cs_sync_rings(adev, fpriv, &mut p)?;
        amdgpu_cs_submit(adev, fpriv, &mut p)
    })();
    let fence = p.fence.clone();
    amdgpu_cs_parser_fini(adev, &mut p);
    let seq = r?;
    Ok((seq, fence.unwrap()))
}

/// `amdgpu_cs_pass1`: the jobs, one per entity the IBs name.
fn amdgpu_cs_pass1(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser,
                   ibs: &[drm_amdgpu_cs_chunk_ib], bo_list: &[usize]) -> Result<(), i32> {
    let mut num_ibs = [0u32; AMDGPU_CS_GANG_SIZE as usize];
    for chunk_ib in ibs {
        // amdgpu_cs_p1_ib
        let r = amdgpu_cs_job_idx(adev, fpriv, p, chunk_ib)?;
        // the IP type is taken as a ring type, as Linux does
        if num_ibs[r] >= crate::amdgpu_ring::amdgpu_ring_max_ibs(chunk_ib.ip_type) {
            return Err(-22);
        }
        num_ibs[r] += 1;
        p.gang_leader_idx = r;
    }
    // amdgpu_cs_p1_bo_handles
    p.bo_list = bo_list.iter().map(|&b| (b, None)).collect();
    if p.entities.is_empty() {
        return Err(-22);
    }
    let vm = fpriv.vm;
    for i in 0..p.entities.len() {
        let j = amdgpu_job::amdgpu_job_alloc(adev, Some(vm), Some(p.entities[i]), amdgpu_vm::amdgpu_vm_owner(vm),
                                             num_ibs[i])?;
        p.jobs.push(Some(j));
        // AMDGPU_ENFORCE_ISOLATION_DISABLE
        let job = amdgpu_job::job(adev, j);
        job.enforce_isolation = false;
        job.run_cleaner_shader = false;
    }
    let leader = p.jobs[p.gang_leader_idx].unwrap();
    let generation = fpriv.ctx_mgr.ctx_handles.get(&p.ctx_id).unwrap().generation;
    if generation != amdgpu_job::job(adev, leader).generation {
        return Err(-125);
    }
    Ok(())
}

/// `amdgpu_cs_pass2` with `amdgpu_cs_p2_ib`: the IBs point at the
/// client's buffer, nothing is copied.
fn amdgpu_cs_pass2(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser,
                   ibs: &[drm_amdgpu_cs_chunk_ib]) -> Result<(), i32> {
    for chunk_ib in ibs {
        let r = amdgpu_cs_job_idx(adev, fpriv, p, chunk_ib)?;
        let j = p.jobs[r].unwrap();
        let e = p.entities[r];
        let ring = adev.sched_entities[e].as_ref().unwrap().rq.unwrap().0;
        if chunk_ib.flags & AMDGPU_IB_FLAG_CE != 0 {
            Line::new().s("CE CS is blocked, use debug=0x400 to override").say();
            return Err(-22);
        }
        if chunk_ib.ib_bytes / 4 > AMDGPU_GFX_SDMA_IB_PACKET_SIZE_MAX_DW {
            return Err(-22);
        }
        let t = adev.rings[ring].funcs.type_;
        if t != AMDGPU_RING_TYPE_COMPUTE && t != AMDGPU_RING_TYPE_SDMA {
            return Err(-22);
        }
        let job = amdgpu_job::job(adev, j);
        let n = job.num_ibs as usize;
        job.num_ibs += 1;
        if chunk_ib.flags & AMDGPU_IB_FLAG_PREAMBLE != 0 {
            job.preamble_status |= AMDGPU_PREAMBLE_IB_PRESENT;
        }
        // amdgpu_ib_get with size 0: the IB is the client's
        let ib = &mut job.ibs[n];
        ib.gpu_addr = chunk_ib.va_start;
        ib.length_dw = chunk_ib.ib_bytes / 4;
        ib.flags = chunk_ib.flags;
    }
    Ok(())
}

/// `amdgpu_cs_parser_bos`: the buffers locked and validated, the VM's
/// evicted tables too.
fn amdgpu_cs_parser_bos(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser)
                        -> Result<(), i32> {
    let vm = fpriv.vm;
    let pd = amdgpu_vm::base(adev, adev.vm(vm).root).bo.unwrap();
    p.locked.push(pd);
    for i in 0..p.bo_list.len() {
        let bo = p.bo_list[i].0;
        if !p.locked.contains(&bo) {
            p.locked.push(bo);
        }
        p.bo_list[i].1 = amdgpu_vm_bo_find(adev, vm, bo);
    }
    let r = amdgpu_vm::amdgpu_vm_validate(adev, vm, amdgpu_cs_bo_validate);
    if r != 0 {
        Line::new().s("amdgpu_vm_validate() failed.").say();
        return Err(r);
    }
    for &bo in &p.locked.clone() {
        let r = amdgpu_cs_bo_validate(adev, bo);
        if r != 0 {
            return Err(r);
        }
    }
    Ok(())
}

/// `amdgpu_cs_vm_handling`.
fn amdgpu_cs_vm_handling(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser)
                         -> Result<(), i32> {
    let vm = fpriv.vm;
    if p.entities.len() > 1 && !adev.vm_manager.concurrent_flush {
        for &e in &p.entities {
            let ring = adev.sched_entities[e].as_ref().unwrap().rq.unwrap().0;
            if crate::amdgpu_ids::amdgpu_vmid_uses_reserved(adev, vm, adev.rings[ring].vm_hub as usize) {
                return Err(-22);
            }
        }
    }
    if !amdgpu_vm::amdgpu_vm_ready(adev, vm) {
        return Err(-22);
    }
    let r = amdgpu_vm::amdgpu_vm_clear_freed(adev, vm, None);
    if r != 0 {
        return Err(r);
    }
    let sync_bo_va = |adev: &mut amdgpu_device, p: &mut amdgpu_cs_parser, b: usize| -> Result<(), i32> {
        let r = amdgpu_vm::amdgpu_vm_bo_update(adev, b, false);
        if r != 0 {
            return Err(r);
        }
        let f = amdgpu_vm::bo_va(adev, b).last_pt_update.clone();
        let r = crate::amdgpu_sync::amdgpu_sync_fence(adev, &mut p.sync, Some(&f));
        if r != 0 { Err(r) } else { Ok(()) }
    };
    sync_bo_va(adev, p, fpriv.prt_va)?;
    for i in 0..p.bo_list.len() {
        if let Some(b) = p.bo_list[i].1 {
            sync_bo_va(adev, p, b)?;
        }
    }
    let r = amdgpu_vm::amdgpu_vm_handle_moved(adev, vm);
    if r != 0 {
        return Err(r);
    }
    let r = amdgpu_vm::amdgpu_vm_update_pdes(adev, vm, false);
    if r != 0 {
        return Err(r);
    }
    let lu = adev.vm(vm).last_update.clone();
    let r = crate::amdgpu_sync::amdgpu_sync_fence(adev, &mut p.sync, Some(&lu));
    if r != 0 {
        return Err(r);
    }
    let pd = amdgpu_vm::base(adev, adev.vm(vm).root).bo.unwrap();
    let pd_addr = crate::amdgpu_gmc::amdgpu_gmc_pd_addr(adev, pd);
    for j in p.jobs.iter().flatten() {
        let job = amdgpu_job::job(adev, *j);
        if job.vm.is_some() {
            job.vm_pd_addr = pd_addr;
        }
    }
    Ok(())
}

/// `amdgpu_cs_sync_rings`.
fn amdgpu_cs_sync_rings(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser)
                        -> Result<(), i32> {
    let leader_entity = p.entities[p.gang_leader_idx];
    let ctx = fpriv.ctx_mgr.ctx_handles.get_mut(&p.ctx_id).unwrap();
    let r = amdgpu_ctx::amdgpu_ctx_wait_prev_fence(adev, ctx, leader_entity);
    if r != 0 {
        Line::new().s("amdgpu_ctx_wait_prev_fence failed.").say();
        return Err(r);
    }
    let owner = amdgpu_vm::amdgpu_vm_owner(fpriv.vm);
    for &bo in &p.locked.clone() {
        let explicit = obj::bo(adev, bo).flags & AMDGPU_GEM_CREATE_EXPLICIT_SYNC as u64 != 0;
        let mode = if explicit {
            crate::amdgpu_sync::amdgpu_sync_mode::AMDGPU_SYNC_EXPLICIT
        } else {
            crate::amdgpu_sync::amdgpu_sync_mode::AMDGPU_SYNC_NE_OWNER
        };
        let resv = obj::amdgpu_bo_resv(adev, bo).clone();
        let r = crate::amdgpu_sync::amdgpu_sync_resv(adev, &mut p.sync, &resv, mode, owner);
        if r != 0 {
            return Err(r);
        }
    }
    for j in p.jobs.clone().into_iter().flatten() {
        let r = crate::amdgpu_sync::amdgpu_sync_push_to_job(adev, &mut p.sync, j);
        if r != 0 {
            return Err(r);
        }
    }
    let leader = p.jobs[p.gang_leader_idx].unwrap();
    let sched = adev.sched_entities[leader_entity].as_ref().unwrap().rq.unwrap().0;
    while let Some(fence) = crate::amdgpu_sync::amdgpu_sync_get_fence(adev, &mut p.sync) {
        let same = drm_sched::to_drm_sched_fence(&fence).is_some_and(|s| s.sched == sched);
        if !same {
            continue;
        }
        // Use the explicit sync of the gang leader for fences of the same
        // ring.
        let mut es = core::mem::take(&mut amdgpu_job::job(adev, leader).explicit_sync);
        let r = crate::amdgpu_sync::amdgpu_sync_fence(adev, &mut es, Some(&fence));
        amdgpu_job::job(adev, leader).explicit_sync = es;
        if r != 0 {
            return Err(r);
        }
    }
    Ok(())
}

/// `amdgpu_cs_submit`: the jobs armed and pushed, their fence on every
/// locked buffer.
fn amdgpu_cs_submit(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser)
                    -> Result<u64, i32> {
    let leader = p.jobs[p.gang_leader_idx].unwrap();
    for j in p.jobs.clone().into_iter().flatten() {
        drm_sched::drm_sched_job_arm(adev, j);
    }
    for j in p.jobs.clone().into_iter().flatten() {
        if j == leader {
            continue;
        }
        let f = amdgpu_job::job(adev, j).base.s_fence.as_ref().map(|f| drm_sched::to_drm_sched_fence(f).unwrap().scheduled);
        let r = drm_sched::drm_sched_job_add_dependency(adev, leader, f);
        if r != 0 {
            return Err(r);
        }
    }
    if p.jobs.len() > 1 {
        for j in p.jobs.clone().into_iter().flatten() {
            amdgpu_job::amdgpu_job_set_gang_leader(adev, j, leader);
        }
    }
    let vm = fpriv.vm;
    if !adev.vm(vm).lists[amdgpu_vm::vm_status::invalidated as usize].is_empty() {
        return Err(-11);
    }
    let fence = amdgpu_job::job(adev, leader).base.s_fence.clone().unwrap();
    p.fence = Some(fence.clone());
    for &bo in &p.locked.clone() {
        let mut resv = obj::amdgpu_bo_resv_take(adev, bo);
        for j in p.jobs.clone().into_iter().flatten() {
            if j == leader {
                continue;
            }
            let f = amdgpu_job::job(adev, j).base.s_fence.clone().unwrap();
            crate::dma_resv::dma_resv_add_fence(adev, &mut resv, &f, crate::dma_resv::dma_resv_usage::DMA_RESV_USAGE_READ);
        }
        crate::dma_resv::dma_resv_add_fence(adev, &mut resv, &fence, crate::dma_resv::dma_resv_usage::DMA_RESV_USAGE_WRITE);
        obj::amdgpu_bo_resv_put(adev, bo, resv);
    }
    let ctx = fpriv.ctx_mgr.ctx_handles.get_mut(&p.ctx_id).unwrap();
    let seq = amdgpu_ctx::amdgpu_ctx_add_fence(adev, ctx, p.entities[p.gang_leader_idx], &fence);
    let job = amdgpu_job::job(adev, leader);
    if job.preamble_status & AMDGPU_PREAMBLE_IB_PRESENT != 0 && !ctx.preamble_presented {
        job.preamble_status |= AMDGPU_PREAMBLE_IB_PRESENT_FIRST;
        ctx.preamble_presented = true;
    }
    job.uf_sequence = seq;
    for i in 0..p.jobs.len() {
        let j = p.jobs[i].take().unwrap();
        amdgpu_job::amdgpu_job_free_resources(adev, j);
        drm_sched::drm_sched_entity_push_job(adev, j);
    }
    Ok(seq)
}

/// `amdgpu_cs_parser_fini`: jobs not pushed are freed.
fn amdgpu_cs_parser_fini(adev: &mut amdgpu_device, p: &mut amdgpu_cs_parser) {
    crate::amdgpu_sync::amdgpu_sync_free(&mut p.sync);
    p.fence = None;
    for j in p.jobs.iter_mut().filter_map(|j| j.take()) {
        amdgpu_job::amdgpu_job_free(adev, j);
    }
}

/// `amdgpu_gem_object_create`: a client buffer.
pub fn amdgpu_gem_object_create(adev: &mut amdgpu_device, size: u64, alignment: u64, initial_domain: u32, flags: u64)
                                -> Result<usize, i32> {
    let bp = obj::amdgpu_bo_param {
        size,
        byte_align: alignment,
        type_: ttm_bo_type_device,
        resv: None,
        preferred_domain: initial_domain,
        flags: flags | AMDGPU_GEM_CREATE_VRAM_WIPE_ON_RELEASE as u64,
        domain: initial_domain,
    };
    obj::amdgpu_bo_create(adev, &bp)
}

/// `amdgpu_gem_object_open`: the buffer's bo_va in the client's VM.
pub fn amdgpu_gem_object_open(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, bo: usize) -> usize {
    match amdgpu_vm_bo_find(adev, fpriv.vm, bo) {
        Some(b) => {
            amdgpu_vm::bo_va(adev, b).ref_count += 1;
            b
        }
        None => amdgpu_vm::amdgpu_vm_bo_add(adev, fpriv.vm, Some(bo)),
    }
}

/// `amdgpu_gem_object_close`: the bo_va goes with its last reference.
pub fn amdgpu_gem_object_close(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, bo: usize) {
    let Some(b) = amdgpu_vm_bo_find(adev, fpriv.vm, bo) else { return };
    let v = amdgpu_vm::bo_va(adev, b);
    v.ref_count -= 1;
    if v.ref_count > 0 {
        return;
    }
    amdgpu_vm::amdgpu_vm_bo_del(adev, b);
    if !amdgpu_vm::amdgpu_vm_ready(adev, fpriv.vm) {
        return;
    }
    let mut fence = None;
    let r = amdgpu_vm::amdgpu_vm_clear_freed(adev, fpriv.vm, Some(&mut fence));
    if r != 0 {
        Line::new().s("failed to clear page tables on GEM object close (").dec((-r) as u64).s(")").say();
    }
    if let Some(f) = fence {
        obj::amdgpu_bo_fence(adev, bo, &f, true);
    }
}

/// `amdgpu_gem_va_update_vm`.
fn amdgpu_gem_va_update_vm(adev: &mut amdgpu_device, vm: usize, b: usize, map: bool) -> Fence {
    let mut fence = Some(adev.vm(vm).last_update.clone());
    if !amdgpu_vm::amdgpu_vm_ready(adev, vm) {
        return fence.unwrap();
    }
    let mut r = amdgpu_vm::amdgpu_vm_clear_freed(adev, vm, Some(&mut fence));
    if r == 0 && map {
        r = amdgpu_vm::amdgpu_vm_bo_update(adev, b, false);
    }
    if r == 0 {
        r = amdgpu_vm::amdgpu_vm_update_pdes(adev, vm, false);
    }
    if r == 0 && map {
        let bo = amdgpu_vm::base(adev, b).bo;
        fence = Some(if amdgpu_vm::amdgpu_vm_is_bo_always_valid(adev, vm, bo) {
            adev.vm(vm).last_update.clone()
        } else {
            amdgpu_vm::bo_va(adev, b).last_pt_update.clone()
        });
    }
    if r != 0 {
        Line::new().s("Couldn't update BO_VA (").dec((-r) as u64).s(")").say();
    }
    fence.unwrap()
}

/// `amdgpu_gem_va_ioctl` with `AMDGPU_VA_OP_MAP` or `AMDGPU_VA_OP_UNMAP`:
/// the fence of the page table update.
pub fn amdgpu_gem_va_ioctl(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, bo: usize, map: bool, va_address: u64,
                           offset: u64, map_size: u64, flags: u32) -> Result<Fence, i32> {
    if va_address < AMDGPU_VA_RESERVED_BOTTOM as u64 {
        return Err(-22);
    }
    if va_address >= AMDGPU_GMC_HOLE_START && va_address < AMDGPU_GMC_HOLE_END {
        return Err(-22);
    }
    let va_address = va_address & AMDGPU_GMC_HOLE_MASK;
    let vm_size = adev.vm_manager.max_pfn * AMDGPU_GPU_PAGE_SIZE as u64 - AMDGPU_VA_RESERVED_TOP as u64;
    if va_address + map_size > vm_size {
        return Err(-22);
    }
    let b = amdgpu_vm_bo_find(adev, fpriv.vm, bo).ok_or(-2)?;
    let r = if map {
        amdgpu_vm::amdgpu_vm_bo_map(adev, b, va_address, offset, map_size, flags)
    } else {
        amdgpu_vm::amdgpu_vm_bo_unmap(adev, b, va_address)
    };
    if r != 0 {
        return Err(r);
    }
    Ok(amdgpu_gem_va_update_vm(adev, fpriv.vm, b, map))
}
