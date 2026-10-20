//! amdgpu/amdgpu_vm.c (Linux 6.18.55): the VM size GMC derives, and the
//! per-client GPU address spaces. A VM lives in `adev.vms` under its key.

use crate::amdgpu_device::amdgpu_device;
use crate::linux::{fls64, ilog2, roundup_pow_of_two, Line};
use crate::regs::*;
use crate::tables;
use crate::dma_fence::{self, Fence};
use crate::amdgpu_object as obj;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

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

/// The `vm_status` lists of a VM.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum vm_status {
    evicted,
    relocated,
    moved,
    idle,
    evicted_user,
    invalidated,
    done,
}

/// `struct amdgpu_bo_va` around its base.
pub struct amdgpu_bo_va {
    pub ref_count: u32,
    pub last_pt_update: Fence,
    /// Mappings by key, newest first.
    pub valids: Vec<usize>,
    pub invalids: Vec<usize>,
    pub cleared: bool,
    pub is_xgmi: bool,
}

/// `struct amdgpu_vm_bo_base`; a buffer's base in a VM is an
/// `amdgpu_bo_va`, a page table's is an entry of its directory.
pub struct amdgpu_vm_bo_base {
    pub vm: usize,
    pub bo: Option<usize>,
    pub next: Option<usize>,
    /// The list `vm_status` is on.
    pub status: Option<vm_status>,
    pub shared: bool,
    pub moved: bool,
    /// An entry's index in its directory's `entries`.
    pub pt_index: usize,
    pub bo_va: Option<amdgpu_bo_va>,
}

/// `struct amdgpu_bo_va_mapping`, in GPU pages.
pub struct amdgpu_bo_va_mapping {
    pub start: u64,
    pub last: u64,
    pub offset: u64,
    pub flags: u32,
    /// The base of its `amdgpu_bo_va`, by key.
    pub bo_va: Option<usize>,
}

/// `struct amdgpu_vm`.
pub struct amdgpu_vm {
    /// `va`: the mappings by start page.
    pub va: BTreeMap<u64, usize>,
    /// The `vm_status` lists, by `vm_status`, of base keys.
    pub lists: [Vec<usize>; 7],
    /// Mappings by key, newest first.
    pub freed: Vec<usize>,
    /// The root directory's base.
    pub root: usize,
    pub last_update: Fence,
    /// The scheduler entities for page table updates, by key.
    pub immediate: usize,
    pub delayed: usize,
    /// `tlb_seq`: bumped by every update that needs a TLB flush.
    pub tlb_seq: u64,
    pub last_tlb_flush: Fence,
    pub tlb_fence_context: u64,
    pub generation: u64,
    pub last_unlocked: Fence,
    pub pasid: u32,
    pub reserved_vmid: [Option<usize>; AMDGPU_MAX_VMHUBS as usize],
    pub use_cpu_for_update: bool,
    pub evicting: bool,
    pub is_compute_context: bool,
    pub need_tlb_fence: bool,
}

pub fn base(adev: &amdgpu_device, b: usize) -> &amdgpu_vm_bo_base {
    adev.vm_bases[b].as_ref().unwrap()
}

pub fn base_mut(adev: &mut amdgpu_device, b: usize) -> &mut amdgpu_vm_bo_base {
    adev.vm_bases[b].as_mut().unwrap()
}

pub fn bo_va(adev: &mut amdgpu_device, b: usize) -> &mut amdgpu_bo_va {
    base_mut(adev, b).bo_va.as_mut().unwrap()
}

pub fn mapping(adev: &amdgpu_device, m: usize) -> &amdgpu_bo_va_mapping {
    adev.vm_mappings[m].as_ref().unwrap()
}

pub fn mapping_mut(adev: &mut amdgpu_device, m: usize) -> &mut amdgpu_bo_va_mapping {
    adev.vm_mappings[m].as_mut().unwrap()
}

fn slot<T>(v: &mut Vec<Option<T>>, x: T) -> usize {
    match v.iter().position(|e| e.is_none()) {
        Some(i) => {
            v[i] = Some(x);
            i
        }
        None => {
            v.push(Some(x));
            v.len() - 1
        }
    }
}

/// A new base for `vm`, on no list.
pub fn amdgpu_vm_bo_base_alloc(adev: &mut amdgpu_device, vm: usize, pt_index: usize) -> usize {
    slot(&mut adev.vm_bases, amdgpu_vm_bo_base {
        vm,
        bo: None,
        next: None,
        status: None,
        shared: false,
        moved: false,
        pt_index,
        bo_va: None,
    })
}

/// The VM's owner value for fences: its key past the reserved owners.
pub fn amdgpu_vm_owner(vm: usize) -> u64 {
    0x1000 + vm as u64
}

/// `list_del(&base->vm_status)`.
pub fn status_del(adev: &mut amdgpu_device, b: usize) {
    let (vm, st) = (base(adev, b).vm, base(adev, b).status);
    if let Some(st) = st {
        adev.vm_mut(vm).lists[st as usize].retain(|&x| x != b);
    }
    base_mut(adev, b).status = None;
}

/// `list_move` (to the head) or `list_move_tail`.
fn status_move(adev: &mut amdgpu_device, b: usize, to: vm_status, tail: bool) {
    status_del(adev, b);
    let vm = base(adev, b).vm;
    let l = &mut adev.vm_mut(vm).lists[to as usize];
    if tail { l.push(b) } else { l.insert(0, b) }
    base_mut(adev, b).status = Some(to);
}

/// `amdgpu_vm_bo_evicted`.
fn amdgpu_vm_bo_evicted(adev: &mut amdgpu_device, b: usize) {
    base_mut(adev, b).moved = true;
    let kernel = base(adev, b).bo.is_some_and(|h| obj::bo(adev, h).tbo.type_ == ttm_bo_type_kernel);
    status_move(adev, b, vm_status::evicted, !kernel);
}

/// `amdgpu_vm_bo_moved`.
fn amdgpu_vm_bo_moved(adev: &mut amdgpu_device, b: usize) {
    status_move(adev, b, vm_status::moved, false);
}

/// `amdgpu_vm_bo_idle`.
fn amdgpu_vm_bo_idle(adev: &mut amdgpu_device, b: usize) {
    status_move(adev, b, vm_status::idle, false);
    base_mut(adev, b).moved = false;
}

/// `amdgpu_vm_bo_invalidated`.
fn amdgpu_vm_bo_invalidated(adev: &mut amdgpu_device, b: usize) {
    status_move(adev, b, vm_status::invalidated, false);
}

/// `amdgpu_vm_bo_relocated`.
pub fn amdgpu_vm_bo_relocated(adev: &mut amdgpu_device, b: usize) {
    let has_parent = base(adev, b).bo.is_some_and(|h| obj::bo(adev, h).parent.is_some());
    if has_parent {
        status_move(adev, b, vm_status::relocated, false);
    } else {
        amdgpu_vm_bo_idle(adev, b);
    }
}

/// `amdgpu_vm_bo_done`.
fn amdgpu_vm_bo_done(adev: &mut amdgpu_device, b: usize) {
    status_move(adev, b, vm_status::done, false);
}

/// `amdgpu_vm_bo_base_init`: fdinfo's memory statistics and the LRU bulk
/// move are not kept.
pub fn amdgpu_vm_bo_base_init(adev: &mut amdgpu_device, b: usize, vm: usize, bo: Option<usize>) {
    {
        let x = base_mut(adev, b);
        x.vm = vm;
        x.bo = bo;
        x.next = None;
        x.status = None;
    }
    let Some(h) = bo else { return };
    let head = obj::bo(adev, h).vm_bo;
    base_mut(adev, b).next = head;
    obj::bo_mut(adev, h).vm_bo = Some(b);
    if !amdgpu_vm_is_bo_always_valid(adev, vm, Some(h)) {
        return;
    }
    let (kernel, parent) = {
        let x = obj::bo(adev, h);
        (x.tbo.type_ == ttm_bo_type_kernel, x.parent.is_some())
    };
    if kernel && parent {
        amdgpu_vm_bo_relocated(adev, b);
    } else {
        amdgpu_vm_bo_idle(adev, b);
    }
    let x = obj::bo(adev, h);
    let mem_type = x.tbo.resource.as_ref().map_or(0, |r| r.mem_type);
    if x.preferred_domains & obj::amdgpu_mem_type_to_domain(mem_type) != 0 {
        return;
    }
    // we checked all the prerequisites, but it looks like this per vm bo
    // is currently evicted. add the bo to the evicted list to make sure
    // it is validated on next vm use to avoid fault.
    amdgpu_vm_bo_evicted(adev, b);
}

/// `amdgpu_vm_init_entities`: on the schedulers of the SDMA rings.
fn amdgpu_vm_init_entities(adev: &mut amdgpu_device, vm: usize) -> i32 {
    let scheds = amdgpu_vm_pte_scheds(adev);
    let e = match crate::drm_sched::drm_sched_entity_init(adev, crate::drm_sched::DRM_SCHED_PRIORITY_NORMAL, &scheds,
                                                          None) {
        Ok(e) => e,
        Err(r) => return r,
    };
    adev.vm_mut(vm).immediate = e;
    match crate::drm_sched::drm_sched_entity_init(adev, crate::drm_sched::DRM_SCHED_PRIORITY_NORMAL, &scheds, None) {
        Ok(d) => {
            adev.vm_mut(vm).delayed = d;
            0
        }
        Err(r) => {
            crate::drm_sched::drm_sched_entity_destroy(adev, e);
            r
        }
    }
}

/// `vm_manager.vm_pte_scheds`: each SDMA instance's ring, there being no
/// page queue (`sdma_v4_0_set_vm_pte_funcs`).
pub fn amdgpu_vm_pte_scheds(adev: &amdgpu_device) -> Vec<usize> {
    (0..adev.vm_manager.vm_pte_num_scheds as usize).filter_map(|i| adev.sdma.instance[i].ring).collect()
}

/// `amdgpu_vm_fini_entities`.
fn amdgpu_vm_fini_entities(adev: &mut amdgpu_device, vm: usize) {
    let (i, d) = (adev.vm(vm).immediate, adev.vm(vm).delayed);
    crate::drm_sched::drm_sched_entity_destroy(adev, i);
    crate::drm_sched::drm_sched_entity_destroy(adev, d);
}

/// `amdgpu_vm_is_bo_always_valid`.
pub fn amdgpu_vm_is_bo_always_valid(adev: &amdgpu_device, vm: usize, bo: Option<usize>) -> bool {
    let Some(h) = bo else { return false };
    let Some(root) = base(adev, adev.vm(vm).root).bo else { return false };
    obj::resv_owner(adev, h) == obj::resv_owner(adev, root)
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

/// `struct amdgpu_vm_update_params`.
pub struct amdgpu_vm_update_params {
    pub vm: usize,
    pub immediate: bool,
    pub unlocked: bool,
    /// `pages_addr` while the update goes through the system pages.
    pub pages_addr: Option<alloc::rc::Rc<Vec<u64>>>,
    pub job: Option<u64>,
    pub num_dw_left: u32,
    pub needs_flush: bool,
    pub allow_override: bool,
    /// Base keys of the tables to free after the TLB flush.
    pub tlb_flush_waitlist: Vec<usize>,
}

impl amdgpu_vm_update_params {
    pub fn new(vm: usize, immediate: bool) -> Self {
        amdgpu_vm_update_params {
            vm,
            immediate,
            unlocked: false,
            pages_addr: None,
            job: None,
            num_dw_left: 0,
            needs_flush: false,
            allow_override: false,
            tlb_flush_waitlist: Vec::new(),
        }
    }
}

/// `amdgpu_vm_validate`: the evicted page tables and per-VM buffers
/// validated with `validate`, and the VM's entities renewed if its
/// generation moved on.
pub fn amdgpu_vm_validate(adev: &mut amdgpu_device, vm: usize, validate: fn(&mut amdgpu_device, usize) -> i32) -> i32 {
    let new_vm_generation = amdgpu_vm_generation(adev, Some(vm));
    if adev.vm(vm).generation != new_vm_generation {
        adev.vm_mut(vm).generation = new_vm_generation;
        amdgpu_vm_bo_reset_state_machine(adev, vm);
        amdgpu_vm_fini_entities(adev, vm);
        let r = amdgpu_vm_init_entities(adev, vm);
        if r != 0 {
            return r;
        }
    }
    while let Some(&b) = adev.vm(vm).lists[vm_status::evicted as usize].first() {
        let h = base(adev, b).bo.unwrap();
        let r = validate(adev, h);
        if r != 0 {
            return r;
        }
        if obj::bo(adev, h).tbo.type_ != ttm_bo_type_kernel {
            amdgpu_vm_bo_moved(adev, b);
        } else {
            crate::amdgpu_vm_sdma::amdgpu_vm_sdma_map_table(adev, h);
            amdgpu_vm_bo_relocated(adev, b);
        }
    }
    adev.vm_mut(vm).evicting = false;
    0
}

/// `amdgpu_vm_bo_reset_state_machine`.
fn amdgpu_vm_bo_reset_state_machine(adev: &mut amdgpu_device, vm: usize) {
    // list_splice_init(&vm->done, &vm->invalidated)
    let done = core::mem::take(&mut adev.vm_mut(vm).lists[vm_status::done as usize]);
    for &b in &done {
        base_mut(adev, b).status = Some(vm_status::invalidated);
    }
    adev.vm_mut(vm).lists[vm_status::invalidated as usize].splice(0..0, done);
    for b in adev.vm(vm).lists[vm_status::invalidated as usize].clone() {
        base_mut(adev, b).moved = true;
    }
    for b in adev.vm(vm).lists[vm_status::idle as usize].clone() {
        base_mut(adev, b).moved = true;
        let bo = base(adev, b).bo;
        match bo {
            Some(h) if obj::bo(adev, h).tbo.type_ == ttm_bo_type_kernel => {
                if obj::bo(adev, h).parent.is_some() {
                    status_move(adev, b, vm_status::relocated, false);
                }
            }
            _ => status_move(adev, b, vm_status::moved, false),
        }
    }
}

/// `amdgpu_vm_ready`.
pub fn amdgpu_vm_ready(adev: &mut amdgpu_device, vm: usize) -> bool {
    let v = adev.vm(vm);
    let mut ret = !v.evicting;
    ret &= v.lists[vm_status::evicted as usize].is_empty();
    let (i, d) = (v.immediate, v.delayed);
    ret &= !crate::drm_sched::drm_sched_entity_stopped(adev, i);
    ret &= !crate::drm_sched::drm_sched_entity_stopped(adev, d);
    ret
}

/// `amdgpu_vm_update_pdes`: the directory entries of the relocated tables.
pub fn amdgpu_vm_update_pdes(adev: &mut amdgpu_device, vm: usize, immediate: bool) -> i32 {
    let relocated = core::mem::take(&mut adev.vm_mut(vm).lists[vm_status::relocated as usize]);
    if relocated.is_empty() {
        return 0;
    }
    let mut params = amdgpu_vm_update_params::new(vm, immediate);
    let mut r = crate::amdgpu_vm_sdma::amdgpu_vm_sdma_prepare(adev, &mut params, None);
    let mut flush_tlb_needed = false;
    if r == 0 {
        for &e in &relocated {
            flush_tlb_needed |= base(adev, e).moved;
            r = crate::amdgpu_vm_pt::amdgpu_vm_pde_update(adev, &mut params, e);
            if r != 0 {
                break;
            }
        }
    }
    if r == 0 {
        match crate::amdgpu_vm_sdma::amdgpu_vm_sdma_commit(adev, &mut params, true) {
            Ok(f) => {
                if let Some(f) = f {
                    adev.vm_mut(vm).last_update = f;
                }
                if flush_tlb_needed {
                    adev.vm_mut(vm).tlb_seq += 1;
                }
                for &e in &relocated {
                    amdgpu_vm_bo_idle(adev, e);
                }
                return 0;
            }
            Err(e) => r = e,
        }
    }
    // the entries stay on the spliced-off list, as in Linux
    for &e in &relocated {
        base_mut(adev, e).status = None;
    }
    r
}

/// `amdgpu_vm_tlb_seq_cb`.
pub fn amdgpu_vm_tlb_seq_cb(adev: &mut amdgpu_device, vm: usize) {
    if let Some(Some(v)) = adev.vms.get_mut(vm) {
        v.tlb_seq += 1;
    }
}

/// `amdgpu_vm_tlb_flush`: no TLB fence without user queues.
fn amdgpu_vm_tlb_flush(adev: &mut amdgpu_device, vm: usize, fence: Option<&Fence>) {
    let Some(f) = fence else {
        amdgpu_vm_tlb_seq_cb(adev, vm);
        return;
    };
    match dma_fence::dma_fence_add_callback(adev, f, dma_fence::dma_fence_func::vm_tlb_seq { vm }) {
        Ok(_) => adev.vm_mut(vm).last_tlb_flush = f.clone(),
        Err(_) => amdgpu_vm_tlb_seq_cb(adev, vm),
    }
}

/// `amdgpu_vm_update_range`: pages `start..=last` pointing at `res` from
/// byte `offset` (or at `pages_addr`), with `flags`. `fence` is the slot
/// the update's fence goes to.
pub fn amdgpu_vm_update_range(adev: &mut amdgpu_device, vm: usize, immediate: bool, unlocked: bool, mut flush_tlb: bool,
                              allow_override: bool, sync: Option<&mut crate::amdgpu_sync::amdgpu_sync>,
                              mut start: u64, last: u64, flags: u64, offset: u64, vram_base: u64,
                              res: Option<&crate::ttm::ttm_resource>, pages_addr: Option<alloc::rc::Rc<Vec<u64>>>,
                              fence: Option<&mut Option<Fence>>) -> i32 {
    // On GFX8 and older any 8 PTE block with a valid bit set enters the
    // TLB, while GFX9 and newer only clear the TLB on a flush.
    flush_tlb |= adev.ip_version(GC_HWIP, 0) < 9 << 24;
    let mut params = amdgpu_vm_update_params::new(vm, immediate);
    params.pages_addr = pages_addr.clone();
    params.unlocked = unlocked;
    params.needs_flush = flush_tlb;
    params.allow_override = allow_override;
    if adev.vm(vm).evicting {
        return -16;
    }
    let lu = adev.vm(vm).last_unlocked.clone();
    if !unlocked && !dma_fence::dma_fence_is_signaled(adev, &lu) {
        let root = base(adev, adev.vm(vm).root).bo.unwrap();
        obj::amdgpu_bo_fence(adev, root, &lu, true);
        adev.vm_mut(vm).last_unlocked = dma_fence::dma_fence_get_stub();
    }
    let r = crate::amdgpu_vm_sdma::amdgpu_vm_sdma_prepare(adev, &mut params, sync);
    if r != 0 {
        return r;
    }
    let cur_res = if pages_addr.is_some() { None } else { res };
    let mut cursor = crate::amdgpu_res_cursor::amdgpu_res_first(adev, cur_res, offset,
                                                                (last - start + 1) * AMDGPU_GPU_PAGE_SIZE as u64);
    while cursor.remaining != 0 {
        let mut num_entries = cursor.size >> AMDGPU_GPU_PAGE_SHIFT;
        let addr;
        if let Some(pages) = &pages_addr {
            let mut contiguous = true;
            if num_entries > AMDGPU_GPU_PAGES_IN_CPU_PAGE as u64 {
                let pfn = (cursor.start >> 12) as usize;
                contiguous = pages[pfn + 1] == pages[pfn] + 4096;
                let tmp = num_entries / AMDGPU_GPU_PAGES_IN_CPU_PAGE as u64;
                let mut count = 2u64;
                while count < tmp {
                    let idx = pfn + count as usize;
                    if contiguous != (pages[idx] == pages[idx - 1] + 4096) {
                        break;
                    }
                    count += 1;
                }
                if !contiguous {
                    count -= 1;
                }
                num_entries = count * AMDGPU_GPU_PAGES_IN_CPU_PAGE as u64;
            }
            if !contiguous {
                addr = cursor.start;
                params.pages_addr = Some(pages.clone());
            } else {
                addr = pages[(cursor.start >> 12) as usize];
                params.pages_addr = None;
            }
        } else if flags & (AMDGPU_PTE_VALID as u64 | AMDGPU_PTE_PRT) != 0 {
            addr = vram_base + cursor.start;
        } else {
            addr = 0;
        }
        let tmp = start + num_entries;
        let r = crate::amdgpu_vm_pt::amdgpu_vm_ptes_update(adev, &mut params, start, tmp, addr, flags);
        if r != 0 {
            return r;
        }
        crate::amdgpu_res_cursor::amdgpu_res_next(adev, cur_res, &mut cursor, num_entries * AMDGPU_GPU_PAGE_SIZE as u64);
        start = tmp;
    }
    let want = fence.is_some();
    let f = match crate::amdgpu_vm_sdma::amdgpu_vm_sdma_commit(adev, &mut params, want) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let mut fence = fence;
    if let (Some(slot), Some(f)) = (fence.as_deref_mut(), f) {
        *slot = Some(f);
    }
    if params.needs_flush {
        let cur = fence.as_deref().and_then(|s| s.clone());
        amdgpu_vm_tlb_flush(adev, vm, cur.as_ref());
    }
    crate::amdgpu_vm_pt::amdgpu_vm_pt_free_list(adev, &mut params);
    0
}

/// `amdgpu_vm_bo_add`: `bo` (or none, for PRT) in `vm`; the bo_va's key.
pub fn amdgpu_vm_bo_add(adev: &mut amdgpu_device, vm: usize, bo: Option<usize>) -> usize {
    let b = amdgpu_vm_bo_base_alloc(adev, vm, 0);
    amdgpu_vm_bo_base_init(adev, b, vm, bo);
    base_mut(adev, b).bo_va = Some(amdgpu_bo_va {
        ref_count: 1,
        last_pt_update: dma_fence::dma_fence_get_stub(),
        valids: Vec::new(),
        invalids: Vec::new(),
        cleared: false,
        is_xgmi: false,
    });
    b
}

/// `amdgpu_vm_it_iter_first`: the first mapping overlapping `start..=last`.
fn amdgpu_vm_it_iter_first(adev: &amdgpu_device, vm: usize, start: u64, last: u64) -> Option<usize> {
    let va = &adev.vm(vm).va;
    if let Some((_, &m)) = va.range(..start).next_back() {
        if mapping(adev, m).last >= start {
            return Some(m);
        }
    }
    va.range(start..=last).next().map(|(_, &m)| m)
}

/// `amdgpu_vm_bo_insert_map`.
fn amdgpu_vm_bo_insert_map(adev: &mut amdgpu_device, b: usize, m: usize) {
    let vm = base(adev, b).vm;
    let bo = base(adev, b).bo;
    mapping_mut(adev, m).bo_va = Some(b);
    bo_va(adev, b).invalids.insert(0, m);
    let start = mapping(adev, m).start;
    adev.vm_mut(vm).va.insert(start, m);
    if amdgpu_vm_is_bo_always_valid(adev, vm, bo) && !base(adev, b).moved {
        amdgpu_vm_bo_moved(adev, b);
    }
}

/// `amdgpu_vm_verify_parameters`.
fn amdgpu_vm_verify_parameters(adev: &amdgpu_device, bo: Option<usize>, saddr: u64, offset: u64, size: u64) -> i32 {
    // validate the parameters
    if saddr & AMDGPU_GPU_PAGE_MASK as u64 != 0 || offset & AMDGPU_GPU_PAGE_MASK as u64 != 0
        || size & AMDGPU_GPU_PAGE_MASK as u64 != 0 {
        return -22;
    }
    if saddr.checked_add(size).is_none() || offset.checked_add(size).is_none() || size == 0 {
        return -22;
    }
    // make sure object fit at this offset
    if bo.is_some_and(|h| offset + size > obj::amdgpu_bo_size(adev, h)) {
        return -22;
    }
    // Ensure last pfn not exceed max_pfn
    let lpfn = (saddr + size - 1) >> AMDGPU_GPU_PAGE_SHIFT;
    if lpfn >= adev.vm_manager.max_pfn {
        return -22;
    }
    0
}

/// `amdgpu_vm_bo_map`.
pub fn amdgpu_vm_bo_map(adev: &mut amdgpu_device, b: usize, saddr: u64, offset: u64, size: u64, flags: u32) -> i32 {
    let bo = base(adev, b).bo;
    let vm = base(adev, b).vm;
    let r = amdgpu_vm_verify_parameters(adev, bo, saddr, offset, size);
    if r != 0 {
        return r;
    }
    let saddr = saddr / AMDGPU_GPU_PAGE_SIZE as u64;
    let eaddr = saddr + (size - 1) / AMDGPU_GPU_PAGE_SIZE as u64;
    if let Some(tmp) = amdgpu_vm_it_iter_first(adev, vm, saddr, eaddr) {
        // bo and tmp overlap, invalid addr
        let t = mapping(adev, tmp);
        Line::new().s("va 0x").hexw((saddr >> 32) as u32, 2).hexw(saddr as u32, 8).s("-0x")
            .hexw((eaddr >> 32) as u32, 2).hexw(eaddr as u32, 8).s(" conflict with 0x").hex64(t.start).s("-0x")
            .hex64(t.last + 1).say();
        return -22;
    }
    let m = slot(&mut adev.vm_mappings, amdgpu_bo_va_mapping { start: saddr, last: eaddr, offset, flags, bo_va: None });
    amdgpu_vm_bo_insert_map(adev, b, m);
    0
}

/// The bus address of each page of a TT buffer (`ttm->dma_address`).
fn tt_dma_addresses(adev: &amdgpu_device, h: usize) -> Vec<u64> {
    let b = obj::bo(adev, h);
    let tt = b.tbo.ttm.unwrap();
    match b.tbo.sg.as_ref() {
        Some(sg) if tt.page_flags & TTM_TT_FLAG_EXTERNAL != 0 => sg[..tt.num_pages as usize].to_vec(),
        _ => (0..tt.num_pages).map(|i| tt.dma_address(i)).collect(),
    }
}

/// `amdgpu_vm_bo_update`: the bo_va's invalid mappings written into the
/// page tables, cleared instead if `clear`.
pub fn amdgpu_vm_bo_update(adev: &mut amdgpu_device, b: usize, clear: bool) -> i32 {
    use crate::amdgpu_sync::amdgpu_sync_mode::*;
    let bo = base(adev, b).bo;
    let vm = base(adev, b).vm;
    let mut sync = crate::amdgpu_sync::amdgpu_sync_create();
    let mut flush_tlb = clear;
    let mut mem = None;
    let mut pages_addr = None;
    let r = 'sync: {
        if clear {
            let root = base(adev, adev.vm(vm).root).bo.unwrap();
            let resv = obj::amdgpu_bo_resv(adev, root).clone();
            let r = crate::amdgpu_sync::amdgpu_sync_resv(adev, &mut sync, &resv, AMDGPU_SYNC_EQ_OWNER, amdgpu_vm_owner(vm));
            if r != 0 {
                break 'sync r;
            }
            // amdgpu_sync_kfd: no KFD fences here
        } else if let Some(h) = bo {
            mem = obj::bo(adev, h).tbo.resource.clone();
            if mem.as_ref().is_some_and(|m| m.mem_type == TTM_PL_TT || m.mem_type == AMDGPU_PL_PREEMPT) {
                pages_addr = Some(alloc::rc::Rc::new(tt_dma_addresses(adev, h)));
            }
            let resv = obj::amdgpu_bo_resv(adev, h).clone();
            let r = crate::amdgpu_sync::amdgpu_sync_resv(adev, &mut sync, &resv, AMDGPU_SYNC_EXPLICIT, amdgpu_vm_owner(vm));
            if r != 0 {
                break 'sync r;
            }
        }
        0
    };
    if r != 0 {
        crate::amdgpu_sync::amdgpu_sync_free(&mut sync);
        return r;
    }
    let (flags, vram_base, uncached) = match bo {
        Some(h) => {
            let x = obj::bo(adev, h);
            let flags = crate::amdgpu_ttm::amdgpu_ttm_tt_pte_flags(adev, x.tbo.ttm.as_ref(), mem.as_ref());
            (flags, adev.vm_manager.vram_base_offset, x.flags & AMDGPU_GEM_CREATE_UNCACHED as u64 != 0)
        }
        None => (0, 0, false),
    };
    let always_valid = amdgpu_vm_is_bo_always_valid(adev, vm, bo);
    let to_vm = clear || always_valid;
    let mut last_update = Some(if to_vm { adev.vm(vm).last_update.clone() } else { bo_va(adev, b).last_pt_update.clone() });
    if !clear && base(adev, b).moved {
        flush_tlb = true;
        let v = core::mem::take(&mut bo_va(adev, b).valids);
        bo_va(adev, b).invalids.splice(0..0, v);
    } else if bo_va(adev, b).cleared != clear {
        let v = core::mem::take(&mut bo_va(adev, b).valids);
        bo_va(adev, b).invalids.splice(0..0, v);
    }
    let invalids = bo_va(adev, b).invalids.clone();
    for m in invalids {
        let (mflags, start, last, offset) = {
            let x = mapping(adev, m);
            (x.flags, x.start, x.last, x.offset)
        };
        let mut update_flags = flags;
        // normally,bo_va->flags only contians READABLE and WIRTEABLE bit
        // go here but in case of something, we filter the flags in first
        // place
        if mflags & AMDGPU_VM_PAGE_READABLE == 0 {
            update_flags &= !(AMDGPU_PTE_READABLE as u64);
        }
        if mflags & AMDGPU_VM_PAGE_WRITEABLE == 0 {
            update_flags &= !(AMDGPU_PTE_WRITEABLE as u64);
        }
        // Apply ASIC specific mapping flags
        crate::gmc_v9_0::gmc_v9_0_get_vm_pte(adev, bo, mflags, &mut update_flags);
        let r = amdgpu_vm_update_range(adev, vm, false, false, flush_tlb, !uncached, Some(&mut sync), start, last,
                                       update_flags, offset, vram_base, mem.as_ref(), pages_addr.clone(),
                                       Some(&mut last_update));
        if r != 0 {
            crate::amdgpu_sync::amdgpu_sync_free(&mut sync);
            return r;
        }
    }
    let lu = last_update.unwrap();
    if to_vm { adev.vm_mut(vm).last_update = lu } else { bo_va(adev, b).last_pt_update = lu }
    // If the BO is not in its preferred location add it back to the
    // evicted list so that it gets validated again on the next command
    // submission.
    if always_valid {
        let h = bo.unwrap();
        let x = obj::bo(adev, h);
        let evicted = x.tbo.resource.as_ref()
            .is_some_and(|r| x.preferred_domains & obj::amdgpu_mem_type_to_domain(r.mem_type) == 0);
        if evicted { amdgpu_vm_bo_evicted(adev, b) } else { amdgpu_vm_bo_idle(adev, b) }
    } else {
        amdgpu_vm_bo_done(adev, b);
    }
    let i = core::mem::take(&mut bo_va(adev, b).invalids);
    bo_va(adev, b).valids.splice(0..0, i);
    bo_va(adev, b).cleared = clear;
    base_mut(adev, b).moved = false;
    crate::amdgpu_sync::amdgpu_sync_free(&mut sync);
    0
}

/// `amdgpu_vm_free_mapping`: no PRT here.
fn amdgpu_vm_free_mapping(adev: &mut amdgpu_device, m: usize) {
    adev.vm_mappings[m] = None;
}

/// `amdgpu_vm_bo_unmap`: the mapping at `saddr` goes to `freed`.
pub fn amdgpu_vm_bo_unmap(adev: &mut amdgpu_device, b: usize, saddr: u64) -> i32 {
    let vm = base(adev, b).vm;
    let saddr = saddr / AMDGPU_GPU_PAGE_SIZE as u64;
    let find = |adev: &mut amdgpu_device, valid: bool| {
        let l = if valid { &bo_va(adev, b).valids } else { &bo_va(adev, b).invalids };
        l.clone().into_iter().find(|&m| mapping(adev, m).start == saddr)
    };
    let (m, valid) = match find(adev, true) {
        Some(m) => (m, true),
        None => match find(adev, false) {
            Some(m) => (m, false),
            None => return -2,
        },
    };
    bo_va(adev, b).valids.retain(|&x| x != m);
    bo_va(adev, b).invalids.retain(|&x| x != m);
    let start = mapping(adev, m).start;
    adev.vm_mut(vm).va.remove(&start);
    mapping_mut(adev, m).bo_va = None;
    if valid {
        adev.vm_mut(vm).freed.insert(0, m);
    } else {
        amdgpu_vm_free_mapping(adev, m);
    }
    0
}

/// `amdgpu_vm_clear_freed`: the freed mappings cleared from the tables.
pub fn amdgpu_vm_clear_freed(adev: &mut amdgpu_device, vm: usize, fence: Option<&mut Option<Fence>>) -> i32 {
    let mut sync = crate::amdgpu_sync::amdgpu_sync_create();
    let root = base(adev, adev.vm(vm).root).bo.unwrap();
    let resv = obj::amdgpu_bo_resv(adev, root).clone();
    let mut r = crate::amdgpu_sync::amdgpu_sync_resv(adev, &mut sync, &resv,
                                                     crate::amdgpu_sync::amdgpu_sync_mode::AMDGPU_SYNC_EQ_OWNER,
                                                     amdgpu_vm_owner(vm));
    let mut f: Option<Fence> = None;
    while r == 0 {
        let Some(m) = adev.vm(vm).freed.first().copied() else { break };
        adev.vm_mut(vm).freed.remove(0);
        let (start, last) = (mapping(adev, m).start, mapping(adev, m).last);
        r = amdgpu_vm_update_range(adev, vm, false, false, true, false, Some(&mut sync), start, last, 0, 0, 0, None,
                                   None, Some(&mut f));
        amdgpu_vm_free_mapping(adev, m);
    }
    if r == 0 {
        if let (Some(slot), Some(f)) = (fence, f) {
            *slot = Some(f);
        }
    }
    crate::amdgpu_sync::amdgpu_sync_free(&mut sync);
    r
}

/// `amdgpu_vm_bo_del`: the bo_va gone, its valid mappings to `freed`.
pub fn amdgpu_vm_bo_del(adev: &mut amdgpu_device, b: usize) {
    let vm = base(adev, b).vm;
    if let Some(h) = base(adev, b).bo {
        // unlink from the buffer's vm_bo list
        let next = base(adev, b).next;
        if obj::bo(adev, h).vm_bo == Some(b) {
            obj::bo_mut(adev, h).vm_bo = next;
        } else {
            let mut cur = obj::bo(adev, h).vm_bo;
            while let Some(c) = cur {
                if base(adev, c).next == Some(b) {
                    base_mut(adev, c).next = next;
                    break;
                }
                cur = base(adev, c).next;
            }
        }
    }
    status_del(adev, b);
    for m in core::mem::take(&mut bo_va(adev, b).valids) {
        let start = mapping(adev, m).start;
        adev.vm_mut(vm).va.remove(&start);
        mapping_mut(adev, m).bo_va = None;
        adev.vm_mut(vm).freed.insert(0, m);
    }
    for m in core::mem::take(&mut bo_va(adev, b).invalids) {
        let start = mapping(adev, m).start;
        adev.vm_mut(vm).va.remove(&start);
        amdgpu_vm_free_mapping(adev, m);
    }
    adev.vm_bases[b] = None;
}

/// `amdgpu_vm_handle_moved`: the moved and invalidated bo_vas updated.
pub fn amdgpu_vm_handle_moved(adev: &mut amdgpu_device, vm: usize) -> i32 {
    while let Some(&b) = adev.vm(vm).lists[vm_status::moved as usize].first() {
        let r = amdgpu_vm_bo_update(adev, b, false);
        if r != 0 {
            return r;
        }
    }
    while let Some(&b) = adev.vm(vm).lists[vm_status::invalidated as usize].first() {
        // the reservation is ours to take: one thread
        let r = amdgpu_vm_bo_update(adev, b, false);
        if r != 0 {
            return r;
        }
    }
    0
}

/// `amdgpu_vm_init`: a VM with its root directory cleared; its key.
pub fn amdgpu_vm_init(adev: &mut amdgpu_device, pasid: u32) -> Result<usize, i32> {
    let stub = dma_fence::dma_fence_get_stub;
    let vm = slot(&mut adev.vms, amdgpu_vm {
        va: BTreeMap::new(),
        lists: Default::default(),
        freed: Vec::new(),
        root: 0,
        last_update: stub(),
        immediate: 0,
        delayed: 0,
        tlb_seq: 0,
        last_tlb_flush: stub(),
        tlb_fence_context: 0,
        generation: 0,
        last_unlocked: stub(),
        pasid: 0,
        reserved_vmid: [None; AMDGPU_MAX_VMHUBS as usize],
        use_cpu_for_update: false,
        evicting: false,
        is_compute_context: false,
        need_tlb_fence: false,
    });
    let root_base = amdgpu_vm_bo_base_alloc(adev, vm, 0);
    adev.vm_mut(vm).root = root_base;
    let r = amdgpu_vm_init_entities(adev, vm);
    if r != 0 {
        adev.vm_bases[root_base] = None;
        adev.vms[vm] = None;
        return Err(r);
    }
    let use_cpu = adev.vm_manager.vm_update_mode & AMDGPU_VM_USE_CPU_FOR_GFX != 0;
    adev.vm_mut(vm).use_cpu_for_update = use_cpu;
    Line::new().s("VM update mode is ").s(if use_cpu { "CPU" } else { "SDMA" }).dbg();
    if use_cpu {
        Line::new().s("CPU updates of the page tables are not ported").say();
    }
    adev.vm_mut(vm).generation = amdgpu_vm_generation(adev, None);
    adev.vm_mut(vm).tlb_fence_context = dma_fence::dma_fence_context_alloc(1);
    let root_level = adev.vm_manager.root_level;
    let r = (|| {
        let root = crate::amdgpu_vm_pt::amdgpu_vm_pt_create(adev, vm, root_level, false)?;
        let root_bo = obj::amdgpu_bo_ref(adev, root);
        amdgpu_vm_bo_base_init(adev, root_base, vm, Some(root_bo));
        let r = crate::amdgpu_vm_pt::amdgpu_vm_pt_clear(adev, vm, root, false);
        if r != 0 {
            obj::amdgpu_bo_unref(adev, root_bo);
            return Err(r);
        }
        if pasid != 0 {
            adev.vm_manager.pasids.insert(pasid, vm);
            adev.vm_mut(vm).pasid = pasid;
        }
        obj::amdgpu_bo_unref(adev, root_bo);
        Ok(())
    })();
    if let Err(r) = r {
        crate::amdgpu_vm_pt::amdgpu_vm_pt_free_root(adev, vm);
        amdgpu_vm_fini_entities(adev, vm);
        adev.vm_bases[root_base] = None;
        adev.vms[vm] = None;
        return Err(r);
    }
    Ok(vm)
}

/// `amdgpu_vm_fini`: no PRT, no KFD.
pub fn amdgpu_vm_fini(adev: &mut amdgpu_device, vm: usize) {
    let root = base(adev, adev.vm(vm).root).bo.unwrap();
    let root = obj::amdgpu_bo_ref(adev, root);
    let pasid = adev.vm(vm).pasid;
    if pasid != 0 {
        adev.vm_manager.pasids.remove(&pasid);
        adev.vm_mut(vm).pasid = 0;
    }
    let lu = adev.vm(vm).last_unlocked.clone();
    dma_fence::dma_fence_wait(adev, &lu);
    let lt = adev.vm(vm).last_tlb_flush.clone();
    dma_fence::dma_fence_wait(adev, &lt);
    for m in core::mem::take(&mut adev.vm_mut(vm).freed) {
        amdgpu_vm_free_mapping(adev, m);
    }
    crate::amdgpu_vm_pt::amdgpu_vm_pt_free_root(adev, vm);
    obj::amdgpu_bo_unref(adev, root);
    amdgpu_vm_fini_entities(adev, vm);
    if !adev.vm(vm).va.is_empty() {
        Line::new().s("still active bo inside vm").say();
    }
    for (_, m) in core::mem::take(&mut adev.vm_mut(vm).va) {
        amdgpu_vm_free_mapping(adev, m);
    }
    for i in 0..AMDGPU_MAX_VMHUBS as usize {
        crate::amdgpu_ids::amdgpu_vmid_free_reserved(adev, vm, i);
    }
    let rb = adev.vm(vm).root;
    adev.vm_bases[rb] = None;
    adev.vms[vm] = None;
}

/// `amdgpu_vm_handle_fault`: a retry fault of a graphics VM pointed at the
/// dummy page (`amdgpu_vm_fault_stop` is never), so the access completes.
/// False, as Linux returns it, so the fault is still reported.
pub fn amdgpu_vm_handle_fault(adev: &mut amdgpu_device, pasid: u32, _vmid: u32, _node_id: u32, addr: u64, _ts: u64,
                              _write_fault: bool) -> bool {
    let Some(&vm) = adev.vm_manager.pasids.get(&pasid) else { return false };
    let Some(root) = base(adev, adev.vm(vm).root).bo else { return false };
    let root = obj::amdgpu_bo_ref(adev, root);
    let is_compute_context = adev.vm(vm).is_compute_context;
    let addr = addr / AMDGPU_GPU_PAGE_SIZE as u64;
    let mut flags = (AMDGPU_PTE_VALID | AMDGPU_PTE_SNOOPED | AMDGPU_PTE_SYSTEM) as u64;
    let value;
    if is_compute_context {
        // Intentionally setting invalid PTE flag combination to force a
        // no-retry-fault
        flags = AMDGPU_VM_NORETRY_FLAGS;
        value = 0;
    } else if tables::amdgpu_vm_fault_stop[0] as u32 == AMDGPU_VM_FAULT_STOP_NEVER {
        // Redirect the access to the dummy page
        value = adev.dummy_page_addr;
        flags |= (AMDGPU_PTE_EXECUTABLE | AMDGPU_PTE_READABLE | AMDGPU_PTE_WRITEABLE) as u64;
    } else {
        // Let the hw retry silently on the PTE
        value = 0;
    }
    let mut r = amdgpu_vm_update_range(adev, vm, true, false, false, false, None, addr, addr, flags, value, 0, None,
                                       None, None);
    if r == 0 {
        r = amdgpu_vm_update_pdes(adev, vm, true);
    }
    if r < 0 {
        Line::new().s("Can't handle page fault (").dec((-r) as u64).s(")").say();
    }
    obj::amdgpu_bo_unref(adev, root);
    false
}
