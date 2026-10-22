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

/// A chunk of `drm_amdgpu_cs`, as `amdgpu_cs_pass1` copies it in.
pub enum cs_chunk {
    ib(drm_amdgpu_cs_chunk_ib),
    /// The buffers of the submission (`AMDGPU_CHUNK_ID_BO_HANDLES`).
    bo_handles(Vec<usize>),
    /// The user fence: a one-page buffer and the offset in it.
    fence { bo: usize, offset: u32 },
    /// `drm_amdgpu_cs_chunk_dep`s; `scheduled` for SCHEDULED_DEPENDENCIES.
    dependencies { deps: Vec<cs_dep>, scheduled: bool },
}

/// `struct drm_amdgpu_cs_chunk_dep`.
#[derive(Clone, Copy)]
pub struct cs_dep {
    pub ip_type: u32,
    pub ip_instance: u32,
    pub ring: u32,
    pub ctx_id: u32,
    pub handle: u64,
}

/// `struct amdgpu_cs_parser`.
struct amdgpu_cs_parser {
    /// The user fence buffer, referenced.
    uf_bo: Option<usize>,
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
    /// buffer moves
    bytes_moved_threshold: u64,
    bytes_moved_vis_threshold: u64,
    bytes_moved: u64,
    bytes_moved_vis: u64,
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

const AMDGPU_GFX_SDMA_IB_PACKET_SIZE_MAX_DW: u32 = 0x0fffff;
const AMDGPU_MM_IB_PACKET_SIZE_MAX_DW: u32 = 0x7ffff0;

/// `amdgpu_cs_ib_packet_size_max_dw`.
fn amdgpu_cs_ib_packet_size_max_dw(type_: u32) -> u32 {
    match type_ {
        AMDGPU_RING_TYPE_GFX | AMDGPU_RING_TYPE_COMPUTE | AMDGPU_RING_TYPE_SDMA | AMDGPU_HW_IP_VPE => {
            AMDGPU_GFX_SDMA_IB_PACKET_SIZE_MAX_DW
        }
        _ => AMDGPU_MM_IB_PACKET_SIZE_MAX_DW,
    }
}

/// `us_to_bytes`: convert microseconds to bytes.
fn us_to_bytes(adev: &amdgpu_device, us: i64) -> u64 {
    if us <= 0 || adev.mm_stats.log2_max_MBps == 0 {
        return 0;
    }
    // Since accum_us is incremented by a million per second, just multiply
    // it by the number of MB/s to get the number of bytes.
    (us as u64) << adev.mm_stats.log2_max_MBps
}

/// `bytes_to_us`.
fn bytes_to_us(adev: &amdgpu_device, bytes: u64) -> i64 {
    if adev.mm_stats.log2_max_MBps == 0 {
        return 0;
    }
    (bytes >> adev.mm_stats.log2_max_MBps) as i64
}

/// `amdgpu_cs_get_threshold_for_moves`: how many bytes TTM can move right
/// now, and how many of them into CPU-visible VRAM. A non-zero value lets
/// at least one buffer move, which may go over it once; the debt stops
/// further moves until the clock has repaid it. The currency is time in
/// microseconds, converted to bytes.
fn amdgpu_cs_get_threshold_for_moves(adev: &mut amdgpu_device) -> (u64, u64) {
    // Allow a maximum of 200 accumulated ms. This is basically per-IB
    // throttling.
    let us_upper_bound: i64 = 200000;
    let vram = adev.mman.vram_mgr.as_ref().map(|m| m.manager).filter(|m| m.use_type);
    let Some(vram) = vram.filter(|_| adev.mm_stats.log2_max_MBps != 0) else { return (0, 0) };
    let total_vram = adev.gmc.real_vram_size.wrapping_sub(adev.vram_pin_size);
    let used_vram = vram.usage;
    let free_vram = if used_vram >= total_vram { 0 } else { total_vram - used_vram };

    // Increase the amount of accumulated us.
    let time_us = crate::linux::now_us() as i64;
    let increment_us = time_us - adev.mm_stats.last_update_us;
    adev.mm_stats.last_update_us = time_us;
    adev.mm_stats.accum_us = (adev.mm_stats.accum_us + increment_us).min(us_upper_bound);

    // If 128 MB or 1/8th of VRAM is free, start filling it now by setting
    // accum_us to a positive number.
    if free_vram >= 128 * 1024 * 1024 || free_vram >= total_vram / 8 {
        // Be more aggressive on dGPUs. Try to fill a portion of free VRAM
        // now.
        let min_us = if adev.flags & AMD_IS_APU == 0 {
            bytes_to_us(adev, free_vram / 4)
        } else {
            // Reset accum_us on APUs.
            0
        };
        adev.mm_stats.accum_us = min_us.max(adev.mm_stats.accum_us);
    }

    // This is set to 0 if the driver is in debt to disallow (optional)
    // buffer moves.
    let max_bytes = us_to_bytes(adev, adev.mm_stats.accum_us);

    // Do the same for visible VRAM if half of it is free
    let max_vis_bytes = if !crate::amdgpu_gmc::amdgpu_gmc_vram_full_visible(&adev.gmc) {
        let total_vis_vram = adev.gmc.visible_vram_size;
        let used_vis_vram = adev.mman.vram_mgr.as_ref().unwrap().vis_usage;
        if used_vis_vram < total_vis_vram {
            let free_vis_vram = total_vis_vram - used_vis_vram;
            adev.mm_stats.accum_us_vis = (adev.mm_stats.accum_us_vis + increment_us).min(us_upper_bound);
            if free_vis_vram >= total_vis_vram / 2 {
                adev.mm_stats.accum_us_vis = bytes_to_us(adev, free_vis_vram / 2).max(adev.mm_stats.accum_us_vis);
            }
        }
        us_to_bytes(adev, adev.mm_stats.accum_us_vis)
    } else {
        0
    };
    (max_bytes, max_vis_bytes)
}

/// `amdgpu_cs_report_moved_bytes`: how many bytes the last command
/// submission really moved. This can leave a debt that stops buffer
/// migrations for a while.
pub fn amdgpu_cs_report_moved_bytes(adev: &mut amdgpu_device, num_bytes: u64, num_vis_bytes: u64) {
    adev.mm_stats.accum_us -= bytes_to_us(adev, num_bytes);
    adev.mm_stats.accum_us_vis -= bytes_to_us(adev, num_vis_bytes);
}

/// `amdgpu_cs_bo_validate`. No buffer here carries a dma-buf with
/// attachments, so only the threshold decides.
fn amdgpu_cs_bo_validate(adev: &mut amdgpu_device, p: &mut amdgpu_cs_parser, bo: usize) -> i32 {
    let mut ctx = crate::ttm::ttm_operation_ctx { no_wait_gpu: false, bytes_moved: 0 };
    if obj::bo(adev, bo).tbo.pin_count != 0 {
        return 0;
    }
    let full_visible = crate::amdgpu_gmc::amdgpu_gmc_vram_full_visible(&adev.gmc);
    let (preferred, allowed, flags) = {
        let b = obj::bo(adev, bo);
        (b.preferred_domains, b.allowed_domains, b.flags)
    };
    // Don't move this buffer if we have depleted our allowance to move it.
    // Don't move anything if the threshold is zero.
    let mut domain = if p.bytes_moved < p.bytes_moved_threshold {
        if !full_visible && flags & AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED as u64 != 0 {
            // And don't move a CPU_ACCESS_REQUIRED BO to limited visible
            // VRAM if we've depleted our allowance to do that.
            if p.bytes_moved_vis < p.bytes_moved_vis_threshold { preferred } else { allowed }
        } else {
            preferred
        }
    } else {
        allowed
    };
    loop {
        obj::amdgpu_bo_placement_from_domain(adev, bo, domain);
        let placement = obj::bo(adev, bo).placements.clone();
        let r = crate::amdgpu_ttm::ttm_bo_validate(adev, bo, &placement, &mut ctx);
        p.bytes_moved += ctx.bytes_moved;
        if !full_visible && crate::amdgpu_ttm::amdgpu_res_cpu_visible(adev, obj::bo(adev, bo).tbo.resource.as_ref()) {
            p.bytes_moved_vis += ctx.bytes_moved;
        }
        if r == -12 && domain != obj::bo(adev, bo).allowed_domains {
            domain = obj::bo(adev, bo).allowed_domains;
            continue;
        }
        return r;
    }
}

/// `amdgpu_cs_ioctl` for `ibs` on context `ctx_id`, with the buffers in
/// `bo_list`: the submission's sequence number and fence.
/// `amdgpu_cs_ioctl` with IB chunks and a buffer list, as the compositor
/// submits.
pub fn amdgpu_cs_ioctl(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, ctx_id: u32,
                       ibs: &[drm_amdgpu_cs_chunk_ib], bo_list: &[usize]) -> Result<(u64, Fence), i32> {
    let mut chunks: Vec<cs_chunk> = ibs.iter().map(|&i| cs_chunk::ib(i)).collect();
    chunks.push(cs_chunk::bo_handles(bo_list.to_vec()));
    amdgpu_cs_ioctl_chunks(adev, fpriv, ctx_id, &chunks)
}

/// `amdgpu_cs_ioctl`: the sequence number of the gang leader's job and
/// its fence.
pub fn amdgpu_cs_ioctl_chunks(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, ctx_id: u32, chunks: &[cs_chunk])
                              -> Result<(u64, Fence), i32> {
    if !adev.accel_working {
        return Err(-16);
    }
    // amdgpu_cs_parser_init
    let ctx = fpriv.ctx_mgr.ctx_handles.get(&ctx_id).ok_or(-22)?;
    if ctx.guilty.get() != 0 {
        return Err(-125);
    }
    let mut p = amdgpu_cs_parser {
        uf_bo: None,
        ctx_id,
        entities: Vec::new(),
        jobs: Vec::new(),
        gang_leader_idx: 0,
        sync: crate::amdgpu_sync::amdgpu_sync_create(),
        fence: None,
        bo_list: Vec::new(),
        locked: Vec::new(),
        bytes_moved_threshold: 0,
        bytes_moved_vis_threshold: 0,
        bytes_moved: 0,
        bytes_moved_vis: 0,
    };
    let r = (|| -> Result<u64, i32> {
        amdgpu_cs_pass1(adev, fpriv, &mut p, chunks)?;
        amdgpu_cs_pass2(adev, fpriv, &mut p, chunks)?;
        amdgpu_cs_parser_bos(adev, fpriv, &mut p)?;
        amdgpu_cs_patch_jobs(adev, fpriv, &mut p)?;
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
fn amdgpu_cs_pass1(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser, chunks: &[cs_chunk])
                   -> Result<(), i32> {
    let mut num_ibs = [0u32; AMDGPU_CS_GANG_SIZE as usize];
    let mut uf_offset = 0;
    for c in chunks {
        match c {
            cs_chunk::ib(chunk_ib) => {
                // amdgpu_cs_p1_ib
                let r = amdgpu_cs_job_idx(adev, fpriv, p, chunk_ib)?;
                // the IP type is taken as a ring type, as Linux does
                if num_ibs[r] >= crate::amdgpu_ring::amdgpu_ring_max_ibs(chunk_ib.ip_type) {
                    return Err(-22);
                }
                num_ibs[r] += 1;
                p.gang_leader_idx = r;
            }
            cs_chunk::fence { bo, offset } => {
                // Only a single user fence is allowed to simplify handling.
                if p.uf_bo.is_some() {
                    return Err(-22);
                }
                // amdgpu_cs_p1_user_fence
                p.uf_bo = Some(obj::amdgpu_bo_ref(adev, *bo));
                let size = obj::amdgpu_bo_size(adev, *bo);
                if size != obj::PAGE_SIZE || *offset as u64 > size - 8 {
                    return Err(-22);
                }
                uf_offset = *offset;
            }
            // amdgpu_cs_p1_bo_handles
            cs_chunk::bo_handles(list) => p.bo_list = list.iter().map(|&b| (b, None)).collect(),
            cs_chunk::dependencies { .. } => {}
        }
    }
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
    if p.uf_bo.is_some() {
        amdgpu_job::job(adev, leader).uf_addr = uf_offset as u64;
    }
    Ok(())
}

/// `amdgpu_cs_pass2` with `amdgpu_cs_p2_ib`: the IBs point at the
/// client's buffer, nothing is copied.
fn amdgpu_cs_pass2(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser, chunks: &[cs_chunk])
                   -> Result<(), i32> {
    let (mut ce_preempt, mut de_preempt) = (0, 0);
    for c in chunks {
        let chunk_ib = match c {
            cs_chunk::ib(i) => i,
            cs_chunk::dependencies { deps, scheduled } => {
                amdgpu_cs_p2_dependencies(adev, fpriv, p, deps, *scheduled)?;
                continue;
            }
            _ => continue,
        };
        let r = amdgpu_cs_job_idx(adev, fpriv, p, chunk_ib)?;
        let j = p.jobs[r].unwrap();
        let e = p.entities[r];
        let ring = adev.sched_entities[e].as_ref().unwrap().rq.unwrap().0;
        // MM engine doesn't support user fences
        if p.uf_bo.is_some() && adev.rings[ring].funcs.no_user_fence() {
            return Err(-22);
        }
        if chunk_ib.flags & AMDGPU_IB_FLAG_CE != 0 {
            Line::new().s("CE CS is blocked, use debug=0x400 to override").say();
            return Err(-22);
        }
        if chunk_ib.ip_type == AMDGPU_HW_IP_GFX && chunk_ib.flags & AMDGPU_IB_FLAG_PREEMPT != 0 {
            if chunk_ib.flags & AMDGPU_IB_FLAG_CE != 0 { ce_preempt += 1 } else { de_preempt += 1 }
            // Each GFX command submit allows only 1 IB max preemptible for CE & DE
            if ce_preempt > 1 || de_preempt > 1 {
                return Err(-22);
            }
        }
        if chunk_ib.ib_bytes / 4 > amdgpu_cs_ib_packet_size_max_dw(adev.rings[ring].funcs.type_) {
            return Err(-22);
        }
        let job = amdgpu_job::job(adev, j);
        let n = job.num_ibs as usize;
        job.num_ibs += 1;
        if chunk_ib.flags & AMDGPU_IB_FLAG_PREAMBLE != 0 {
            job.preamble_status |= AMDGPU_PREAMBLE_IB_PRESENT;
        }
        let mut ib = core::mem::replace(&mut job.ibs[n], crate::amdgpu_ib::amdgpu_ib::new());
        let size = if adev.rings[ring].funcs.parse_cs() { chunk_ib.ib_bytes } else { 0 };
        let r = crate::amdgpu_ib::amdgpu_ib_get(adev, size, AMDGPU_IB_POOL_DELAYED, &mut ib);
        let job = amdgpu_job::job(adev, j);
        job.ibs[n] = ib;
        if r != 0 {
            Line::new().s("Failed to get ib !").say();
            return Err(r);
        }
        let ib = &mut job.ibs[n];
        ib.gpu_addr = chunk_ib.va_start;
        ib.length_dw = chunk_ib.ib_bytes / 4;
        ib.flags = chunk_ib.flags;
    }
    Ok(())
}

/// `amdgpu_cs_parser_bos`: the buffers locked and validated, the VM's
/// evicted tables too.
/// `amdgpu_cs_p2_dependencies`.
fn amdgpu_cs_p2_dependencies(adev: &mut amdgpu_device, fpriv: &mut amdgpu_fpriv, p: &mut amdgpu_cs_parser,
                             deps: &[cs_dep], scheduled: bool) -> Result<(), i32> {
    for d in deps {
        let ctx = fpriv.ctx_mgr.ctx_handles.get_mut(&d.ctx_id).ok_or(-22)?;
        crate::amdgpu_ctx::amdgpu_ctx_get_entity(adev, ctx, d.ip_type, d.ip_instance, d.ring)?;
        let Some(fence) = crate::amdgpu_ctx::amdgpu_ctx_get_fence(ctx, d.ip_type, d.ring, d.handle)? else { continue };
        let fence = if scheduled {
            crate::drm_sched::to_drm_sched_fence(&fence).map_or(fence.clone(), |s| s.scheduled.clone())
        } else {
            fence
        };
        let r = crate::amdgpu_sync::amdgpu_sync_fence(adev, &mut p.sync, Some(&fence));
        if r != 0 {
            return Err(r);
        }
    }
    Ok(())
}

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
    if let Some(uf) = p.uf_bo {
        if !p.locked.contains(&uf) {
            p.locked.push(uf);
        }
    }
    let (threshold, vis_threshold) = amdgpu_cs_get_threshold_for_moves(adev);
    p.bytes_moved_threshold = threshold;
    p.bytes_moved_vis_threshold = vis_threshold;
    p.bytes_moved = 0;
    p.bytes_moved_vis = 0;
    let r = amdgpu_vm::amdgpu_vm_validate(adev, vm, &mut |adev, bo| amdgpu_cs_bo_validate(adev, p, bo));
    if r != 0 {
        Line::new().s("amdgpu_vm_validate() failed.").say();
        return Err(r);
    }
    for &bo in &p.locked.clone() {
        let r = amdgpu_cs_bo_validate(adev, p, bo);
        if r != 0 {
            return Err(r);
        }
    }
    if let Some(uf) = p.uf_bo {
        let r = crate::amdgpu_ttm::amdgpu_ttm_alloc_gart(adev, uf);
        if r != 0 {
            return Err(r);
        }
        let leader = p.jobs[p.gang_leader_idx].unwrap();
        amdgpu_job::job(adev, leader).uf_addr += obj::amdgpu_bo_gpu_offset(adev, uf);
    }
    amdgpu_cs_report_moved_bytes(adev, p.bytes_moved, p.bytes_moved_vis);
    Ok(())
}

/// `amdgpu_cs_find_mapping`: the buffer and mapping at `addr`, a buffer
/// this CS reserved, contiguous if in VRAM and bound to the GART.
fn amdgpu_cs_find_mapping(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, p: &amdgpu_cs_parser, addr: u64)
                          -> Result<(usize, usize), i32> {
    let addr = addr / AMDGPU_GPU_PAGE_SIZE as u64;
    let m = amdgpu_vm::amdgpu_vm_bo_lookup_mapping(adev, fpriv.vm, addr).ok_or(-22)?;
    let bo_va = amdgpu_vm::mapping(adev, m).bo_va.ok_or(-22)?;
    let bo = amdgpu_vm::base(adev, bo_va).bo.ok_or(-22)?;
    // Double check that the BO is reserved by this CS
    if !p.locked.contains(&bo) {
        return Err(-22);
    }
    // Make sure VRAM is allocated contigiously
    obj::bo_mut(adev, bo).flags |= AMDGPU_GEM_CREATE_VRAM_CONTIGUOUS as u64;
    let res = obj::bo(adev, bo).tbo.resource.clone().ok_or(-22)?;
    if res.mem_type == TTM_PL_VRAM && res.placement & TTM_PL_FLAG_CONTIGUOUS == 0 {
        let allowed = obj::bo(adev, bo).allowed_domains;
        obj::amdgpu_bo_placement_from_domain(adev, bo, allowed);
        for pl in obj::bo_mut(adev, bo).placements.iter_mut() {
            pl.flags |= TTM_PL_FLAG_CONTIGUOUS;
        }
        let placement = obj::bo(adev, bo).placements.clone();
        let mut ctx = crate::ttm::ttm_operation_ctx::default();
        let r = crate::amdgpu_ttm::ttm_bo_validate(adev, bo, &placement, &mut ctx);
        if r != 0 {
            return Err(r);
        }
    }
    let r = crate::amdgpu_ttm::amdgpu_ttm_alloc_gart(adev, bo);
    if r != 0 {
        return Err(r);
    }
    Ok((bo, m))
}

/// `amdgpu_cs_patch_ibs`: for a ring with `parse_cs`, every IB copied
/// from the client's buffer into the kernel IB and checked there; the
/// engine then runs the copy. No ring here has `patch_cs_in_place`.
fn amdgpu_cs_patch_ibs(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, p: &amdgpu_cs_parser, i: usize)
                       -> Result<(), i32> {
    let ring = adev.sched_entities[p.entities[i]].as_ref().unwrap().rq.unwrap().0;
    if !adev.rings[ring].funcs.parse_cs() {
        return Ok(());
    }
    let j = p.jobs[i].unwrap();
    for n in 0..amdgpu_job::job(adev, j).num_ibs as usize {
        let (gpu_addr, length_dw, ptr) = {
            let ib = &amdgpu_job::job(adev, j).ibs[n];
            (ib.gpu_addr, ib.length_dw, ib.ptr)
        };
        let va_start = gpu_addr & AMDGPU_GMC_HOLE_MASK;
        let Ok((aobj, m)) = amdgpu_cs_find_mapping(adev, fpriv, p, va_start) else {
            Line::new().s("IB va_start is invalid").say();
            return Err(-22);
        };
        if va_start + length_dw as u64 * 4 > (amdgpu_vm::mapping(adev, m).last + 1) * AMDGPU_GPU_PAGE_SIZE as u64 {
            Line::new().s("IB va_start+ib_bytes is invalid").say();
            return Err(-22);
        }
        // the IB should be reserved at this point
        let kptr = obj::amdgpu_bo_kmap(adev, aobj)?;
        let at = va_start - amdgpu_vm::mapping(adev, m).start * AMDGPU_GPU_PAGE_SIZE as u64;
        let (dst, dst_off) = ptr.ok_or(-22)?;
        let mut copy = alloc::vec![0u8; length_dw as usize * 4];
        crate::amdgpu_gart::kptr_read(adev, kptr, at, &mut copy);
        crate::amdgpu_gart::kptr_write(adev, dst, dst_off, &copy);
        let dws: Vec<u32> = copy.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        let r = crate::jpeg_v2_0::amdgpu_jpeg_dec_parse_cs(&dws);
        if r != 0 {
            return Err(r);
        }
        if let Some(sa) = amdgpu_job::job(adev, j).ibs[n].sa_bo {
            let a = crate::amdgpu_ib::amdgpu_sa_bo_gpu_addr(adev, &sa);
            amdgpu_job::job(adev, j).ibs[n].gpu_addr = a;
        }
    }
    Ok(())
}

/// `amdgpu_cs_patch_jobs`.
fn amdgpu_cs_patch_jobs(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, p: &mut amdgpu_cs_parser)
                        -> Result<(), i32> {
    for i in 0..p.jobs.len() {
        amdgpu_cs_patch_ibs(adev, fpriv, p, i)?;
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
    // `ttm_bo_move_to_lru_tail_unlocked` here and `amdgpu_vm_move_to_lru_tail`
    // after the push order the eviction walk, and TTM here evicts nothing.
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
    if let Some(uf) = p.uf_bo.take() {
        obj::amdgpu_bo_unref(adev, uf);
    }
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
        client: None,
        no_wait_gpu: false,
    };
    obj::amdgpu_bo_create(adev, &bp)
}

/// `amdgpu_gem_are_domains_valid`: the CPU, GTT and VRAM domains in any
/// combination, or one special domain alone.
fn amdgpu_gem_are_domains_valid(domains: u32) -> bool {
    let normal = AMDGPU_GEM_DOMAIN_CPU | AMDGPU_GEM_DOMAIN_GTT | AMDGPU_GEM_DOMAIN_VRAM;
    let special = AMDGPU_GEM_DOMAIN_MASK & !normal;
    let (normal_mask, special_mask) = (domains & normal, domains & special);
    if special_mask == 0 {
        return true;
    }
    if normal_mask != 0 {
        return false;
    }
    special_mask & (special_mask - 1) == 0
}

/// `amdgpu_gem_create_ioctl` of guest client `client`: the new object,
/// opened in the client's VM (`drm_gem_handle_create`).
pub fn amdgpu_gem_create_ioctl(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, client: u32, size: u64,
                               alignment: u64, domains: u64, domain_flags: u64) -> Result<usize, i32> {
    let mut flags = domain_flags;
    // reject invalid gem flags
    if flags & !(AMDGPU_GEM_CREATE_SETTABLE_MASK as u64) != 0 {
        return Err(-22);
    }
    // reject invalid gem domains
    if domains & !(AMDGPU_GEM_DOMAIN_MASK as u64) != 0 || !amdgpu_gem_are_domains_valid(domains as u32) {
        return Err(-22);
    }
    if !adev.gmc.tmz_enabled && flags & AMDGPU_GEM_CREATE_ENCRYPTED as u64 != 0 {
        Line::new().s("Cannot allocate secure buffer since TMZ is disabled").say();
        return Err(-22);
    }
    // always clear VRAM
    flags |= AMDGPU_GEM_CREATE_VRAM_CLEARED as u64;
    if domains & AMDGPU_GEM_DOMAIN_MMIO_REMAP as u64 != 0 {
        return Err(-22);
    }
    // create a gem object to contain this object in
    if domains & (AMDGPU_GEM_DOMAIN_GDS | AMDGPU_GEM_DOMAIN_GWS | AMDGPU_GEM_DOMAIN_OA) as u64 != 0 {
        if flags & AMDGPU_GEM_CREATE_VM_ALWAYS_VALID as u64 != 0 {
            // if gds bo is created from user space, it must be passed to bo list
            Line::new().s("GDS bo cannot be per-vm-bo").say();
            return Err(-22);
        }
        flags |= AMDGPU_GEM_CREATE_NO_CPU_ACCESS as u64;
    }
    let root = amdgpu_vm::base(adev, adev.vm(fpriv.vm).root).bo;
    let resv = if flags & AMDGPU_GEM_CREATE_VM_ALWAYS_VALID as u64 != 0 { root } else { None };
    let mut initial_domain = domains as u32;
    let r = loop {
        let bp = obj::amdgpu_bo_param {
            size,
            byte_align: alignment,
            type_: ttm_bo_type_device,
            resv,
            preferred_domain: initial_domain,
            flags: flags | AMDGPU_GEM_CREATE_VRAM_WIPE_ON_RELEASE as u64,
            domain: initial_domain,
            client: Some(client),
            no_wait_gpu: false,
        };
        match obj::amdgpu_bo_create(adev, &bp) {
            Ok(h) => break Ok(h),
            Err(e) => {
                if flags & AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED as u64 != 0 {
                    flags &= !(AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED as u64);
                    continue;
                }
                if initial_domain == AMDGPU_GEM_DOMAIN_VRAM {
                    initial_domain |= AMDGPU_GEM_DOMAIN_GTT;
                    continue;
                }
                break Err(e);
            }
        }
    };
    let h = r?;
    if resv.is_some() {
        obj::bo_mut(adev, h).parent = root.map(|r| obj::amdgpu_bo_ref(adev, r));
    }
    amdgpu_gem_object_open(adev, fpriv, h);
    Ok(h)
}

/// `amdgpu_gem_metadata_ioctl`, SET: tiling flags and the UMD's metadata.
pub fn amdgpu_gem_set_metadata(adev: &mut amdgpu_device, h: usize, tiling_info: u64, flags: u64, data: &[u8]) -> i32 {
    if data.len() > 256 {
        return -22;
    }
    // amdgpu_bo_set_tiling_flags: the tile split rule is CZ and older.
    if adev.family <= AMDGPU_FAMILY_CZ && (tiling_info >> AMDGPU_TILING_TILE_SPLIT_SHIFT) & AMDGPU_TILING_TILE_SPLIT_MASK as u64 > 6 {
        return -22;
    }
    let b = obj::bo_mut(adev, h);
    b.tiling_flags = tiling_info;
    // amdgpu_bo_set_metadata
    if data.is_empty() {
        b.metadata.clear();
        return 0;
    }
    b.metadata_flags = flags;
    b.metadata = data.to_vec();
    0
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
/// `amdgpu_gem_va_update_vm`: the fence of the last update this
/// operation is about.
fn amdgpu_gem_va_update_vm(adev: &mut amdgpu_device, vm: usize, b: Option<usize>, operation: u32) -> Fence {
    // Always start from the VM's existing last update fence.
    let mut fence = Some(adev.vm(vm).last_update.clone());
    if !amdgpu_vm::amdgpu_vm_ready(adev, vm) {
        return fence.unwrap();
    }
    let map = operation == AMDGPU_VA_OP_MAP || operation == AMDGPU_VA_OP_REPLACE;
    let mut r = amdgpu_vm::amdgpu_vm_clear_freed(adev, vm, Some(&mut fence));
    if r == 0 && map {
        r = amdgpu_vm::amdgpu_vm_bo_update(adev, b.unwrap(), false);
    }
    if r == 0 {
        r = amdgpu_vm::amdgpu_vm_update_pdes(adev, vm, false);
    }
    if r == 0 && map {
        let b = b.unwrap();
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

/// `amdgpu_gem_va_ioctl` without input syncobjs or a VM timeline (no caller
/// here passes them): `bo` None is a PRT range or a CLEAR. The fence of the
/// update, None when it was delayed.
pub fn amdgpu_gem_va_ioctl_op(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, bo: Option<usize>, operation: u32,
                              va_address: u64, offset: u64, map_size: u64, flags: u32)
                              -> Result<Option<Fence>, i32> {
    let valid_flags = AMDGPU_VM_DELAY_UPDATE | AMDGPU_VM_PAGE_READABLE | AMDGPU_VM_PAGE_WRITEABLE
        | AMDGPU_VM_PAGE_EXECUTABLE | AMDGPU_VM_MTYPE_MASK | AMDGPU_VM_PAGE_NOALLOC;
    let prt_flags = AMDGPU_VM_DELAY_UPDATE | AMDGPU_VM_PAGE_PRT;
    // Validate virtual address range against reserved regions.
    if va_address < AMDGPU_VA_RESERVED_BOTTOM as u64 {
        return Err(-22);
    }
    if va_address >= AMDGPU_GMC_HOLE_START && va_address < AMDGPU_GMC_HOLE_END {
        return Err(-22);
    }
    let va_address = va_address & AMDGPU_GMC_HOLE_MASK;
    let vm_size = adev.vm_manager.max_pfn * AMDGPU_GPU_PAGE_SIZE as u64 - AMDGPU_VA_RESERVED_TOP as u64;
    if va_address.checked_add(map_size).is_none_or(|e| e > vm_size) {
        return Err(-22);
    }
    if flags & !valid_flags != 0 && flags & !prt_flags != 0 {
        return Err(-22);
    }
    if !matches!(operation, AMDGPU_VA_OP_MAP | AMDGPU_VA_OP_UNMAP | AMDGPU_VA_OP_CLEAR | AMDGPU_VA_OP_REPLACE) {
        return Err(-22);
    }
    let needs_bo = operation != AMDGPU_VA_OP_CLEAR && flags & AMDGPU_VM_PAGE_PRT == 0;
    let abo = if needs_bo { Some(bo.ok_or(-2)?) } else { None };
    // Resolve the BO-VA mapping for this VM/BO combination.
    let b = match abo {
        Some(h) => Some(amdgpu_vm_bo_find(adev, fpriv.vm, h).ok_or(-2)?),
        None if operation != AMDGPU_VA_OP_CLEAR => Some(fpriv.prt_va),
        None => None,
    };
    let r = match operation {
        AMDGPU_VA_OP_MAP => amdgpu_vm::amdgpu_vm_bo_map(adev, b.unwrap(), va_address, offset, map_size, flags),
        AMDGPU_VA_OP_UNMAP => amdgpu_vm::amdgpu_vm_bo_unmap(adev, b.unwrap(), va_address),
        AMDGPU_VA_OP_CLEAR => amdgpu_vm::amdgpu_vm_bo_clear_mappings(adev, fpriv.vm, va_address, map_size),
        _ => amdgpu_vm::amdgpu_vm_bo_replace_map(adev, b.unwrap(), va_address, offset, map_size, flags),
    };
    if r != 0 {
        return Err(r);
    }
    if flags & AMDGPU_VM_DELAY_UPDATE != 0 {
        return Ok(None);
    }
    Ok(Some(amdgpu_gem_va_update_vm(adev, fpriv.vm, b, operation)))
}

/// `amdgpu_gem_va_ioctl` for one mapping of `bo`, as the compositor uses it.
pub fn amdgpu_gem_va_ioctl(adev: &mut amdgpu_device, fpriv: &amdgpu_fpriv, bo: usize, map: bool, va_address: u64,
                           offset: u64, map_size: u64, flags: u32) -> Result<Fence, i32> {
    let op = if map { AMDGPU_VA_OP_MAP } else { AMDGPU_VA_OP_UNMAP };
    let f = amdgpu_gem_va_ioctl_op(adev, fpriv, Some(bo), op, va_address, offset, map_size, flags)?;
    Ok(f.unwrap_or_else(|| adev.vm(fpriv.vm).last_update.clone()))
}
