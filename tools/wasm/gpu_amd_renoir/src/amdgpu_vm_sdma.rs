//! amdgpu/amdgpu_vm_sdma.c (Linux 6.18.55): page table updates as SDMA
//! jobs on the VM's entities.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object as obj;
use crate::amdgpu_vm::amdgpu_vm_update_params;
use crate::dma_fence::Fence;
use crate::linux::Line;
use crate::regs::*;

/// `amdgpu_vm_sdma_map_table`.
pub fn amdgpu_vm_sdma_map_table(adev: &mut amdgpu_device, h: usize) -> i32 {
    crate::amdgpu_ttm::amdgpu_ttm_alloc_gart(adev, h)
}

/// `amdgpu_vm_sdma_alloc_job`.
fn amdgpu_vm_sdma_alloc_job(adev: &mut amdgpu_device, p: &mut amdgpu_vm_update_params, count: u32) -> i32 {
    let pool = if p.immediate { AMDGPU_IB_POOL_IMMEDIATE } else { AMDGPU_IB_POOL_DELAYED };
    let entity = if p.immediate { adev.vm(p.vm).immediate } else { adev.vm(p.vm).delayed };
    let mut ndw = AMDGPU_VM_SDMA_MIN_NUM_DW;
    if p.pages_addr.is_some() {
        ndw += count * 2;
    }
    ndw = ndw.min(AMDGPU_VM_SDMA_MAX_NUM_DW);
    match crate::amdgpu_job::amdgpu_job_alloc_with_ib(adev, Some(entity), crate::amdgpu_sync::AMDGPU_FENCE_OWNER_VM,
                                                      ndw * 4, pool) {
        Ok(j) => p.job = Some(j),
        Err(r) => return r,
    }
    p.num_dw_left = ndw;
    0
}

/// `amdgpu_vm_sdma_prepare`.
pub fn amdgpu_vm_sdma_prepare(adev: &mut amdgpu_device, p: &mut amdgpu_vm_update_params,
                              sync: Option<&mut crate::amdgpu_sync::amdgpu_sync>) -> i32 {
    let r = amdgpu_vm_sdma_alloc_job(adev, p, 0);
    if r != 0 {
        return r;
    }
    let Some(sync) = sync else { return 0 };
    let j = p.job.unwrap();
    let r = crate::amdgpu_sync::amdgpu_sync_push_to_job(adev, sync, j);
    if r != 0 {
        p.num_dw_left = 0;
        crate::amdgpu_job::amdgpu_job_free(adev, j);
    }
    r
}

/// `amdgpu_vm_sdma_commit`: the job's fence when one is asked for and the
/// update is not immediate (Linux swaps it into `*fence`).
pub fn amdgpu_vm_sdma_commit(adev: &mut amdgpu_device, p: &mut amdgpu_vm_update_params, want_fence: bool)
                             -> Result<Option<Fence>, i32> {
    let j = p.job.take().unwrap();
    let delayed = adev.vm(p.vm).delayed;
    let ring = adev.sched_entities[delayed].as_ref().unwrap().rq.unwrap().0;
    let mut ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
    if ib.length_dw == 0 {
        Line::new().s("WARNING: empty page table update").say();
    }
    crate::sdma_v4_0::sdma_v4_0_ring_pad_ib(adev, ring, &mut ib);
    if p.needs_flush {
        adev.vm_mut(p.vm).tlb_seq += 1;
    }
    if ib.length_dw > p.num_dw_left {
        Line::new().s("WARNING: page table update overran its IB").say();
    }
    crate::amdgpu_job::job(adev, j).ibs[0] = ib;
    let f = crate::amdgpu_job::amdgpu_job_submit(adev, j);
    if p.unlocked {
        adev.vm_mut(p.vm).last_unlocked = f.clone();
    } else {
        let root = crate::amdgpu_vm::base(adev, adev.vm(p.vm).root).bo.unwrap();
        let mut resv = obj::amdgpu_bo_resv_take(adev, root);
        crate::dma_resv::dma_resv_add_fence(adev, &mut resv, &f, crate::dma_resv::dma_resv_usage::DMA_RESV_USAGE_BOOKKEEP);
        obj::amdgpu_bo_resv_put(adev, root, resv);
    }
    if want_fence && !p.immediate {
        f.set_flag(crate::drm_sched::DRM_SCHED_FENCE_DONT_PIPELINE);
        return Ok(Some(f));
    }
    Ok(None)
}

/// `amdgpu_vm_sdma_copy_ptes`: from the end of the IB, where the entries
/// were written.
fn amdgpu_vm_sdma_copy_ptes(adev: &mut amdgpu_device, p: &mut amdgpu_vm_update_params, bo: usize, pe: u64,
                            count: u32) {
    let j = p.job.unwrap();
    let mut ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
    let src = ib.gpu_addr + p.num_dw_left as u64 * 4;
    let pe = pe + obj::amdgpu_bo_gpu_offset(adev, bo);
    crate::sdma_v4_0::sdma_v4_0_vm_copy_pte(adev, &mut ib, pe, src, count);
    crate::amdgpu_job::job(adev, j).ibs[0] = ib;
}

/// `amdgpu_vm_sdma_set_ptes`.
fn amdgpu_vm_sdma_set_ptes(adev: &mut amdgpu_device, p: &mut amdgpu_vm_update_params, bo: usize, pe: u64, addr: u64,
                           count: u32, incr: u32, flags: u64) {
    let j = p.job.unwrap();
    let mut ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
    let pe = pe + obj::amdgpu_bo_gpu_offset(adev, bo);
    if count < 3 {
        crate::sdma_v4_0::sdma_v4_0_vm_write_pte(adev, &mut ib, pe, addr | flags, count, incr);
    } else {
        crate::sdma_v4_0::sdma_v4_0_vm_set_pte_pde(adev, &mut ib, pe, addr, count, incr, flags);
    }
    crate::amdgpu_job::job(adev, j).ibs[0] = ib;
}

/// `amdgpu_vm_map_gart`: a system page's bus address.
fn amdgpu_vm_map_gart(pages_addr: &[u64], addr: u64) -> u64 {
    let mut result = pages_addr[(addr >> 12) as usize];
    result |= addr & 0xfff;
    result & 0xFFFF_FFFF_FFFF_F000
}

/// `amdgpu_vm_sdma_update`: `count` entries of table `bo` from byte `pe`.
pub fn amdgpu_vm_sdma_update(adev: &mut amdgpu_device, p: &mut amdgpu_vm_update_params, bo: usize, mut pe: u64,
                             mut addr: u64, mut count: u32, incr: u32, flags: u64) -> i32 {
    // Wait for PD/PT moves to be completed
    let resv = obj::amdgpu_bo_resv(adev, bo).clone();
    for f in crate::dma_resv::dma_resv_get_fences(adev, &resv, crate::dma_resv::dma_resv_usage::DMA_RESV_USAGE_KERNEL) {
        let r = crate::drm_sched::drm_sched_job_add_dependency(adev, p.job.unwrap(), Some(f));
        if r != 0 {
            return r;
        }
    }
    loop {
        let mut ndw = p.num_dw_left;
        ndw -= crate::amdgpu_job::job(adev, p.job.unwrap()).ibs[0].length_dw;
        if ndw < 32 {
            if let Err(r) = amdgpu_vm_sdma_commit(adev, p, false) {
                return r;
            }
            let r = amdgpu_vm_sdma_alloc_job(adev, p, count);
            if r != 0 {
                return r;
            }
        }
        let Some(pages) = p.pages_addr.clone() else {
            amdgpu_vm_sdma_set_ptes(adev, p, bo, pe, addr, count, incr, flags);
            return 0;
        };
        // copy commands needed
        ndw -= SDMA_V4_0_VM_COPY_PTE_NUM_DW;
        // for padding
        ndw -= 7;
        let nptes = count.min(ndw / 2);
        // Put the PTEs at the end of the IB.
        p.num_dw_left -= nptes * 2;
        let j = p.job.unwrap();
        let ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
        for i in 0..nptes {
            let pte = amdgpu_vm_map_gart(&pages, addr) | flags;
            ib.set(adev, p.num_dw_left + i * 2, pte as u32);
            ib.set(adev, p.num_dw_left + i * 2 + 1, (pte >> 32) as u32);
            addr += incr as u64;
        }
        amdgpu_vm_sdma_copy_ptes(adev, p, bo, pe, nptes);
        pe += nptes as u64 * 8;
        count -= nptes;
        if count == 0 {
            return 0;
        }
    }
}

/// `sdma_v4_0_vm_pte_funcs.copy_pte_num_dw`.
const SDMA_V4_0_VM_COPY_PTE_NUM_DW: u32 = 7;
