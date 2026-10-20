//! amdgpu/amdgpu_vm.c (Linux 6.18.55): the VM size GMC derives, and the
//! per-client GPU address spaces. A VM lives in `adev.vms` under its key.

use crate::amdgpu_device::amdgpu_device;
use crate::linux::{fls64, ilog2, roundup_pow_of_two, Line};
use crate::regs::*;
use crate::tables;
use crate::dma_fence;

/// `amdgpu_vm_get_block_size`.
fn amdgpu_vm_get_block_size(vm_size: u64) -> u32 {
    // Total bits covered by PD + PTs
    let bits = ilog2(vm_size) + 18;
    // Make sure the PD is 4K in size up to 8GB address space. Above that
    // split equal between PD and PTs
    if vm_size <= 8 { bits - 9 } else { (bits + 3) / 2 }
}

/// `amdgpu_vm_adjust_size`.
pub fn amdgpu_vm_adjust_size(adev: &mut amdgpu_device, min_vm_size: u32, fragment_size_default: u32,
                             max_level: u32, max_bits: u32) {
    let max_size: u32 = 1 << (max_bits - 30);
    let vm_size: u32;
    // adjust vm size first
    if tables::amdgpu_vm_size[0] != -1 {
        let mut v = tables::amdgpu_vm_size[0] as u32;
        if v > max_size {
            Line::new().s("VM size (").dec(tables::amdgpu_vm_size[0] as u64).s(") too large, max is ")
                .dec(max_size as u64).s(" GB").say();
            v = max_size;
        }
        vm_size = v;
    } else {
        // si_meminfo: the RAM the kernel manages.
        let total_mb = npk_sys::sys_info(2).max(0) as u64;
        let phys_ram_gb = ((total_mb << 20) + (1 << 30) - 1) >> 30;
        let clamped = (phys_ram_gb * 3).clamp(min_vm_size as u64, max_size as u64);
        vm_size = roundup_pow_of_two(clamped) as u32;
    }

    adev.vm_manager.max_pfn = (vm_size as u64) << 18;

    let mut tmp = roundup_pow_of_two(adev.vm_manager.max_pfn);
    if tables::amdgpu_vm_block_size[0] != -1 {
        tmp >>= tables::amdgpu_vm_block_size[0] as u32 - 9;
    }
    let tmp = ((fls64(tmp) - 1) as u64).div_ceil(9) - 1;
    adev.vm_manager.num_level = max_level.min(tmp as u32);
    match adev.vm_manager.num_level {
        3 => adev.vm_manager.root_level = AMDGPU_VM_PDB2,
        2 => adev.vm_manager.root_level = AMDGPU_VM_PDB1,
        1 => adev.vm_manager.root_level = AMDGPU_VM_PDB0,
        _ => Line::new().s("VMPT only supports 2~4+1 levels").say(),
    }
    // block size depends on vm size and hw setup
    adev.vm_manager.block_size = if tables::amdgpu_vm_block_size[0] != -1 {
        (tables::amdgpu_vm_block_size[0] as u32)
            .min(max_bits - AMDGPU_GPU_PAGE_SHIFT - 9 * adev.vm_manager.num_level)
    } else if adev.vm_manager.num_level > 1 {
        9
    } else {
        amdgpu_vm_get_block_size(tmp)
    };
    adev.vm_manager.fragment_size = if tables::amdgpu_vm_fragment_size[0] == -1 {
        fragment_size_default
    } else {
        tables::amdgpu_vm_fragment_size[0] as u32
    };
    Line::new().s("vm size is ").dec(vm_size as u64).s(" GB, ").dec(adev.vm_manager.num_level as u64 + 1)
        .s(" levels, block size is ").dec(adev.vm_manager.block_size as u64).s("-bit, fragment size is ")
        .dec(adev.vm_manager.fragment_size as u64).s("-bit").say();
}

/// `struct amdgpu_vm`.
#[derive(Default)]
pub struct amdgpu_vm {
    /// The scheduler entities for page table updates, by key.
    pub immediate: usize,
    pub delayed: usize,
    pub use_cpu_for_update: bool,
    /// `tlb_seq`: bumped by every update that needs a TLB flush.
    pub tlb_seq: u64,
    pub reserved_vmid: [Option<usize>; AMDGPU_MAX_VMHUBS as usize],
    pub pasid: u32,
    pub generation: u64,
}

/// `amdgpu_vm_tlb_seq`.
pub fn amdgpu_vm_tlb_seq(vm: &amdgpu_vm) -> u64 {
    vm.tlb_seq
}

/// `vm->immediate.fence_context`, a VMID's owner.
pub fn immediate_context(adev: &amdgpu_device, vm: usize) -> u64 {
    adev.sched_entities[adev.vm(vm).immediate].as_ref().unwrap().fence_context
}

/// `amdgpu_vm_generation`.
pub fn amdgpu_vm_generation(adev: &mut amdgpu_device, vm: Option<usize>) -> u64 {
    let mut result = (adev.vram_lost_counter as u64) << 32;
    let Some(vm) = vm else { return result };
    result += adev.vm(vm).generation as u32 as u64;
    // Add one if the page tables will be re-generated on next CS
    let delayed = adev.vm(vm).delayed;
    if crate::drm_sched::drm_sched_entity_error(adev, delayed) != 0 {
        result += 1;
    }
    result
}

/// `amdgpu_vm_need_pipeline_sync`.
pub fn amdgpu_vm_need_pipeline_sync(adev: &amdgpu_device, r: usize, j: u64) -> bool {
    let hub = adev.rings[r].vm_hub as usize;
    let job = adev.jobs.get(&j).unwrap();
    if job.vmid == 0 {
        return false;
    }
    if job.vm_needs_flush || adev.rings[r].has_compute_vm_bug {
        return true;
    }
    if crate::amdgpu_ring::has_emit_gds_switch(adev, r) && job.gds_switch_needed {
        return true;
    }
    crate::amdgpu_ids::amdgpu_vmid_had_gpu_reset(adev, hub, job.vmid as usize)
}

/// `amdgpu_vm_flush`: the VM flush, PASID mapping and GDS switch a job
/// needs before its IBs. Renoir has no cleaner shader.
pub fn amdgpu_vm_flush(adev: &mut amdgpu_device, r: usize, j: u64, need_pipe_sync: bool) -> i32 {
    use crate::amdgpu_ring as ring;
    let hub = adev.rings[r].vm_hub as usize;
    let (vmid, pasid, vm_pd_addr, mut spm_update_needed, mut gds_switch_needed, mut vm_flush_needed, finished) = {
        let job = adev.jobs.get(&j).unwrap();
        (job.vmid, job.pasid, job.vm_pd_addr, job.spm_update_needed,
         ring::has_emit_gds_switch(adev, r) && job.gds_switch_needed, job.vm_needs_flush, job.base.s_fence.clone())
    };
    let id = vmid as usize;
    let mut pasid_mapping_needed = false;
    if crate::amdgpu_ids::amdgpu_vmid_had_gpu_reset(adev, hub, id) {
        gds_switch_needed = true;
        vm_flush_needed = true;
        pasid_mapping_needed = true;
        spm_update_needed = true;
    }
    let mapping = adev.vm_manager.id_mgr[hub].ids[id].pasid_mapping.clone();
    let mapped = mapping.as_ref().is_some_and(|f| dma_fence::dma_fence_is_signaled(adev, f));
    if adev.vm_manager.id_mgr[hub].ids[id].pasid != pasid || !mapped {
        pasid_mapping_needed = true;
    }
    gds_switch_needed &= ring::has_emit_gds_switch(adev, r);
    vm_flush_needed &= ring::has_emit_vm_flush(adev, r) && vm_pd_addr != AMDGPU_BO_INVALID_OFFSET;
    // gmc_v9_0 has emit_pasid_mapping
    pasid_mapping_needed &= ring::has_emit_wreg(adev, r);
    if !vm_flush_needed && !gds_switch_needed && !need_pipe_sync {
        return 0;
    }
    let patch = ring::amdgpu_ring_init_cond_exec(adev, r);
    if need_pipe_sync {
        ring::amdgpu_ring_emit_pipeline_sync(adev, r);
    }
    if vm_flush_needed {
        ring::amdgpu_ring_emit_vm_flush(adev, r, vmid, vm_pd_addr);
    }
    if pasid_mapping_needed {
        crate::gmc_v9_0::gmc_v9_0_emit_pasid_mapping(adev, r, vmid, pasid);
    }
    if spm_update_needed {
        crate::gfx_v9_0::gfx_v9_0_update_spm_vmid(adev, vmid);
    }
    if gds_switch_needed {
        let job = adev.jobs.get(&j).unwrap();
        let (a, b, c, d, e, f) = (job.gds_base, job.gds_size, job.gws_base, job.gws_size, job.oa_base, job.oa_size);
        ring::amdgpu_ring_emit_gds_switch(adev, r, vmid, a, b, c, d, e, f);
    }
    let mut fence = None;
    if vm_flush_needed || pasid_mapping_needed {
        let ctx = finished.as_ref().map_or(0, |f| f.context);
        match crate::amdgpu_fence::amdgpu_fence_emit(adev, r, None, ctx, 0) {
            Ok(f) => fence = Some(f),
            Err(e) => return e,
        }
    }
    if vm_flush_needed {
        let count = adev.gpu_reset_counter;
        let x = &mut adev.vm_manager.id_mgr[hub].ids[id];
        x.last_flush = fence.clone();
        x.current_gpu_reset_count = count;
    }
    if pasid_mapping_needed {
        let x = &mut adev.vm_manager.id_mgr[hub].ids[id];
        x.pasid = pasid;
        x.pasid_mapping = fence.clone();
    }
    ring::amdgpu_ring_patch_cond_exec(adev, r, patch);
    if adev.rings[r].funcs.ops == ring::ring_ops::gfx_v9_0_gfx {
        // the GFX ring's emit_switch_buffer; it takes no job here
        Line::new().s("WARNING: amdgpu_vm_flush on the GFX ring").say();
    }
    0
}
