//! amdgpu/amdgpu_ring.c and amdgpu_fence.c (Linux 6.18.55): a ring's
//! driver side as `amdgpu_ring_init` sets it up: its writeback slots, its
//! fence driver and its buffer.
//!
//! The rings live in `adev.rings`, in the order `amdgpu_ring_init` numbers
//! them (`ring->idx`); a block keeps the index where Linux keeps the ring.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object::amdgpu_kptr;
use crate::linux::Line;
use crate::regs::*;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// `struct amdgpu_ring_funcs`, the fields `amdgpu_ring_init` reads.
pub struct amdgpu_ring_funcs {
    pub name: &'static str,
    pub type_: u32,
    pub align_mask: u32,
    pub nop: u32,
    pub support_64bit_ptrs: bool,
    pub emit_frame_size: u32,
    pub emit_ib_size: u32,
    pub extra_bytes: u32,
    /// `soft_recovery` is set.
    pub soft_recovery: bool,
    /// Which table it is, for the function pointers.
    pub ops: ring_ops,
}

/// The function pointers of a funcs table, by the table they belong to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ring_ops {
    gfx_v9_0_gfx,
    gfx_v9_0_compute,
    gfx_v9_0_kiq,
    sdma_v4_0,
    sdma_v4_0_page,
    vcn_v2_0_dec,
    vcn_v2_0_enc,
    jpeg_v2_0_dec,
}

macro_rules! ring_funcs {
    ($id:ident, $name:literal, $ops:ident, $t:ident, $a:ident, $n:ident, $s:ident, $f:ident, $i:ident, $e:ident, $r:ident) => {
        pub static $id: amdgpu_ring_funcs = amdgpu_ring_funcs {
            name: $name,
            type_: $t,
            align_mask: $a,
            nop: $n,
            support_64bit_ptrs: $s != 0,
            emit_frame_size: $f,
            emit_ib_size: $i,
            extra_bytes: $e,
            soft_recovery: $r != 0,
            ops: ring_ops::$ops,
        };
    };
}

ring_funcs!(GFX_V9_0_RING_FUNCS_GFX, "gfx_v9_0_ring_funcs_gfx", gfx_v9_0_gfx, RF_gfx_v9_0_ring_funcs_gfx_type,
    RF_gfx_v9_0_ring_funcs_gfx_align_mask, RF_gfx_v9_0_ring_funcs_gfx_nop,
    RF_gfx_v9_0_ring_funcs_gfx_support_64bit_ptrs, RF_gfx_v9_0_ring_funcs_gfx_emit_frame_size,
    RF_gfx_v9_0_ring_funcs_gfx_emit_ib_size, RF_gfx_v9_0_ring_funcs_gfx_extra_bytes,
    RF_gfx_v9_0_ring_funcs_gfx_soft_recovery);
ring_funcs!(GFX_V9_0_RING_FUNCS_COMPUTE, "gfx_v9_0_ring_funcs_compute", gfx_v9_0_compute, RF_gfx_v9_0_ring_funcs_compute_type,
    RF_gfx_v9_0_ring_funcs_compute_align_mask, RF_gfx_v9_0_ring_funcs_compute_nop,
    RF_gfx_v9_0_ring_funcs_compute_support_64bit_ptrs, RF_gfx_v9_0_ring_funcs_compute_emit_frame_size,
    RF_gfx_v9_0_ring_funcs_compute_emit_ib_size, RF_gfx_v9_0_ring_funcs_compute_extra_bytes,
    RF_gfx_v9_0_ring_funcs_compute_soft_recovery);
ring_funcs!(GFX_V9_0_RING_FUNCS_KIQ, "gfx_v9_0_ring_funcs_kiq", gfx_v9_0_kiq, RF_gfx_v9_0_ring_funcs_kiq_type,
    RF_gfx_v9_0_ring_funcs_kiq_align_mask, RF_gfx_v9_0_ring_funcs_kiq_nop,
    RF_gfx_v9_0_ring_funcs_kiq_support_64bit_ptrs, RF_gfx_v9_0_ring_funcs_kiq_emit_frame_size,
    RF_gfx_v9_0_ring_funcs_kiq_emit_ib_size, RF_gfx_v9_0_ring_funcs_kiq_extra_bytes,
    RF_gfx_v9_0_ring_funcs_kiq_soft_recovery);
ring_funcs!(SDMA_V4_0_RING_FUNCS, "sdma_v4_0_ring_funcs", sdma_v4_0, RF_sdma_v4_0_ring_funcs_type,
    RF_sdma_v4_0_ring_funcs_align_mask, RF_sdma_v4_0_ring_funcs_nop, RF_sdma_v4_0_ring_funcs_support_64bit_ptrs,
    RF_sdma_v4_0_ring_funcs_emit_frame_size, RF_sdma_v4_0_ring_funcs_emit_ib_size,
    RF_sdma_v4_0_ring_funcs_extra_bytes,
    RF_sdma_v4_0_ring_funcs_soft_recovery);
ring_funcs!(SDMA_V4_0_PAGE_RING_FUNCS, "sdma_v4_0_page_ring_funcs", sdma_v4_0_page, RF_sdma_v4_0_page_ring_funcs_type,
    RF_sdma_v4_0_page_ring_funcs_align_mask, RF_sdma_v4_0_page_ring_funcs_nop,
    RF_sdma_v4_0_page_ring_funcs_support_64bit_ptrs, RF_sdma_v4_0_page_ring_funcs_emit_frame_size,
    RF_sdma_v4_0_page_ring_funcs_emit_ib_size, RF_sdma_v4_0_page_ring_funcs_extra_bytes,
    RF_sdma_v4_0_page_ring_funcs_soft_recovery);
ring_funcs!(VCN_V2_0_DEC_RING_VM_FUNCS, "vcn_v2_0_dec_ring_vm_funcs", vcn_v2_0_dec, RF_vcn_v2_0_dec_ring_vm_funcs_type,
    RF_vcn_v2_0_dec_ring_vm_funcs_align_mask, RF_vcn_v2_0_dec_ring_vm_funcs_nop,
    RF_vcn_v2_0_dec_ring_vm_funcs_support_64bit_ptrs, RF_vcn_v2_0_dec_ring_vm_funcs_emit_frame_size,
    RF_vcn_v2_0_dec_ring_vm_funcs_emit_ib_size, RF_vcn_v2_0_dec_ring_vm_funcs_extra_bytes,
    RF_vcn_v2_0_dec_ring_vm_funcs_soft_recovery);
ring_funcs!(VCN_V2_0_ENC_RING_VM_FUNCS, "vcn_v2_0_enc_ring_vm_funcs", vcn_v2_0_enc, RF_vcn_v2_0_enc_ring_vm_funcs_type,
    RF_vcn_v2_0_enc_ring_vm_funcs_align_mask, RF_vcn_v2_0_enc_ring_vm_funcs_nop,
    RF_vcn_v2_0_enc_ring_vm_funcs_support_64bit_ptrs, RF_vcn_v2_0_enc_ring_vm_funcs_emit_frame_size,
    RF_vcn_v2_0_enc_ring_vm_funcs_emit_ib_size, RF_vcn_v2_0_enc_ring_vm_funcs_extra_bytes,
    RF_vcn_v2_0_enc_ring_vm_funcs_soft_recovery);
ring_funcs!(JPEG_V2_0_DEC_RING_VM_FUNCS, "jpeg_v2_0_dec_ring_vm_funcs", jpeg_v2_0_dec, RF_jpeg_v2_0_dec_ring_vm_funcs_type,
    RF_jpeg_v2_0_dec_ring_vm_funcs_align_mask, RF_jpeg_v2_0_dec_ring_vm_funcs_nop,
    RF_jpeg_v2_0_dec_ring_vm_funcs_support_64bit_ptrs, RF_jpeg_v2_0_dec_ring_vm_funcs_emit_frame_size,
    RF_jpeg_v2_0_dec_ring_vm_funcs_emit_ib_size, RF_jpeg_v2_0_dec_ring_vm_funcs_extra_bytes,
    RF_jpeg_v2_0_dec_ring_vm_funcs_soft_recovery);

/// The funcs table a block's early_init named.
pub fn ring_funcs(name: Option<&str>) -> Option<&'static amdgpu_ring_funcs> {
    [
        &GFX_V9_0_RING_FUNCS_GFX,
        &GFX_V9_0_RING_FUNCS_COMPUTE,
        &GFX_V9_0_RING_FUNCS_KIQ,
        &SDMA_V4_0_RING_FUNCS,
        &SDMA_V4_0_PAGE_RING_FUNCS,
        &VCN_V2_0_DEC_RING_VM_FUNCS,
        &VCN_V2_0_ENC_RING_VM_FUNCS,
        &JPEG_V2_0_DEC_RING_VM_FUNCS,
    ]
    .into_iter()
    .find(|f| Some(f.name) == name)
}

/// The `struct amdgpu_irq_src` a ring's fences signal through.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum amdgpu_irq_src_id {
    gfx_eop,
    kiq,
    sdma_trap,
    vcn(u8),
    jpeg,
}

/// `struct amdgpu_fence_driver`; `cpu_addr` is a dword offset into the
/// writeback buffer.
#[derive(Default)]
pub struct amdgpu_fence_driver {
    pub cpu_addr: Option<u32>,
    pub gpu_addr: u64,
    pub sync_seq: u32,
    pub last_seq: u32,
    pub initialized: bool,
    pub irq_src: Option<amdgpu_irq_src_id>,
    pub irq_type: u32,
    pub num_fences_mask: u32,
    /// `fences`: the fences outstanding, `num_hw_submission * 2` slots.
    pub fences: Vec<Option<crate::dma_fence::Fence>>,
    /// `signalled_wptr`: the ring's write pointer after the last fence
    /// signaled.
    pub signalled_wptr: u64,
    /// `fallback_timer`: when it fires, if armed.
    pub fallback_deadline_us: Option<u64>,
}

/// `struct amdgpu_ring`.
pub struct amdgpu_ring {
    pub funcs: &'static amdgpu_ring_funcs,
    pub name: String,
    pub idx: u32,
    pub me: u32,
    pub pipe: u32,
    pub queue: u32,
    pub xcc_id: u32,
    pub use_doorbell: bool,
    pub doorbell_index: u32,
    pub vm_hub: u32,
    pub no_scheduler: bool,
    pub eop_gpu_addr: u64,
    pub num_hw_submission: u32,
    pub fence_drv: amdgpu_fence_driver,
    pub rptr_offs: u32,
    pub wptr_offs: u32,
    pub fence_offs: u32,
    pub trail_fence_offs: u32,
    pub cond_exe_offs: u32,
    pub fence_gpu_addr: u64,
    pub rptr_gpu_addr: u64,
    pub wptr_gpu_addr: u64,
    pub trail_fence_gpu_addr: u64,
    pub cond_exe_gpu_addr: u64,
    pub ring_size: u32,
    pub buf_mask: u32,
    pub ptr_mask: u64,
    pub cached_rptr: u64,
    pub wptr: u64,
    pub max_dw: u32,
    pub hw_prio: u32,
    pub ring_obj: Option<usize>,
    pub gpu_addr: u64,
    pub ring: Option<amdgpu_kptr>,
    pub mqd_obj: Option<usize>,
    pub mqd_gpu_addr: u64,
    pub mqd_ptr: Option<amdgpu_kptr>,
    pub mqd_size: u32,
    /// `ring_backup`: the ring's contents kept across a queue reset.
    pub ring_backup: Vec<u32>,
    pub ring_backup_entries_to_copy: u32,
    pub count_dw: i32,
    pub wptr_old: u64,
    pub sched: crate::drm_sched::drm_gpu_scheduler,
    pub vmid_wait: crate::dma_fence::Fence,
    pub has_compute_vm_bug: bool,
    pub current_ctx: u64,
    pub vm_inv_eng: u32,
}

impl amdgpu_ring {
    pub fn new(funcs: &'static amdgpu_ring_funcs, name: String) -> Self {
        amdgpu_ring {
            funcs,
            name,
            idx: 0,
            me: 0,
            pipe: 0,
            queue: 0,
            xcc_id: 0,
            use_doorbell: false,
            doorbell_index: 0,
            vm_hub: 0,
            no_scheduler: false,
            eop_gpu_addr: 0,
            num_hw_submission: 0,
            fence_drv: amdgpu_fence_driver::default(),
            rptr_offs: 0,
            wptr_offs: 0,
            fence_offs: 0,
            trail_fence_offs: 0,
            cond_exe_offs: 0,
            fence_gpu_addr: 0,
            rptr_gpu_addr: 0,
            wptr_gpu_addr: 0,
            trail_fence_gpu_addr: 0,
            cond_exe_gpu_addr: 0,
            ring_size: 0,
            buf_mask: 0,
            ptr_mask: 0,
            cached_rptr: 0,
            wptr: 0,
            max_dw: 0,
            hw_prio: 0,
            ring_obj: None,
            gpu_addr: 0,
            ring: None,
            mqd_obj: None,
            mqd_gpu_addr: 0,
            mqd_ptr: None,
            mqd_size: 0,
            ring_backup: Vec::new(),
            ring_backup_entries_to_copy: 0,
            count_dw: 0,
            wptr_old: 0,
            sched: Default::default(),
            vmid_wait: crate::dma_fence::dma_fence_get_stub(),
            has_compute_vm_bug: false,
            current_ctx: 0,
            vm_inv_eng: 0,
        }
    }
}

/// `amdgpu_device_wb_get`: a free slot, as a dword offset.
pub fn amdgpu_device_wb_get(adev: &mut amdgpu_device) -> Result<u32, i32> {
    let wb = &mut adev.wb;
    for offset in 0..wb.num_wb as usize {
        if wb.used[offset / 64] & (1 << (offset % 64)) == 0 {
            wb.used[offset / 64] |= 1 << (offset % 64);
            // convert to dw offset
            return Ok((offset as u32) << 3);
        }
    }
    Err(-22)
}

/// `amdgpu_device_wb_free`: `wb` is the dword offset `wb_get` gave.
pub fn amdgpu_device_wb_free(adev: &mut amdgpu_device, wb: u32) {
    let wb = (wb >> 3) as usize;
    if wb < adev.wb.num_wb as usize {
        adev.wb.used[wb / 64] &= !(1 << (wb % 64));
    }
}

/// `amdgpu_ring_max_ibs`.
pub fn amdgpu_ring_max_ibs(type_: u32) -> u32 {
    match type_ {
        // Need to keep at least 192 on GFX7+ for old radv.
        t if t == AMDGPU_RING_TYPE_GFX => 192,
        t if t == AMDGPU_RING_TYPE_COMPUTE => 125,
        t if t == AMDGPU_RING_TYPE_VCN_JPEG => 16,
        _ => 49,
    }
}

/// `amdgpu_fence_driver_init_ring`.
fn amdgpu_fence_driver_init_ring(ring: &mut amdgpu_ring) -> i32 {
    if !ring.num_hw_submission.is_power_of_two() {
        return -22;
    }
    ring.fence_drv = amdgpu_fence_driver {
        num_fences_mask: ring.num_hw_submission * 2 - 1,
        fences: vec![None; ring.num_hw_submission as usize * 2],
        ..amdgpu_fence_driver::default()
    };
    0
}

/// The writeback dword at `offs`.
fn wb_w32(adev: &amdgpu_device, offs: u32, v: u32) {
    if let Some(k) = adev.wb.wb {
        crate::amdgpu_gart::kptr_w32(adev, k, offs as u64 * 4, v);
    }
}

/// `amdgpu_fence_driver_start_ring`: no UVD ring on Renoir.
fn amdgpu_fence_driver_start_ring(adev: &amdgpu_device, ring: &mut amdgpu_ring, irq_src: amdgpu_irq_src_id,
                                  irq_type: u32) -> i32 {
    ring.fence_drv.cpu_addr = Some(ring.fence_offs);
    ring.fence_drv.gpu_addr = ring.fence_gpu_addr;
    // amdgpu_fence_write
    wb_w32(adev, ring.fence_offs, ring.fence_drv.last_seq);
    ring.fence_drv.irq_src = Some(irq_src);
    ring.fence_drv.irq_type = irq_type;
    ring.fence_drv.initialized = true;
    0
}

/// `amdgpu_ring_init`: the ring is new (`ring->adev == NULL`), so it gets
/// the next index. Returns that index.
pub fn amdgpu_ring_init(adev: &mut amdgpu_device, mut ring: amdgpu_ring, max_dw: u32, irq_src: amdgpu_irq_src_id,
                        irq_type: u32, hw_prio: u32) -> Result<usize, i32> {
    let mut max_dw = max_dw;
    let mut sched_hw_submission = crate::tables::amdgpu_sched_hw_submission[0] as u32;
    // Set the hw submission limit higher for KIQ because it's used for a
    // number of gfx/compute tasks by both KFD and KGD which may have
    // outstanding fences and it doesn't really use the gpu scheduler
    // anyway; KIQ tasks get submitted directly to the ring.
    if ring.funcs.type_ == AMDGPU_RING_TYPE_KIQ {
        sched_hw_submission = sched_hw_submission.max(256);
    }
    if ring.funcs.type_ == AMDGPU_RING_TYPE_MES {
        sched_hw_submission = 8;
    } else if core::ptr::eq(ring.funcs, &SDMA_V4_0_PAGE_RING_FUNCS) && ring.me == 0 {
        // ring == &adev->sdma.instance[0].page
        sched_hw_submission = 256;
    }

    if adev.rings.len() >= AMDGPU_MAX_RINGS as usize {
        return Err(-22);
    }
    ring.num_hw_submission = sched_hw_submission;
    ring.idx = adev.rings.len() as u32;
    let r = amdgpu_fence_driver_init_ring(&mut ring);
    if r != 0 {
        return Err(r);
    }

    for (what, slot) in [
        ("rptr_offs", 0),
        ("wptr_offs", 1),
        ("fence_offs", 2),
        ("trail_fence_offs", 3),
        ("cond_exec_polling", 4),
    ] {
        let offs = match amdgpu_device_wb_get(adev) {
            Ok(o) => o,
            Err(r) => {
                Line::new().s("(").dec((-r) as u64).s(") ring ").s(what).s(" wb alloc failed").say();
                return Err(r);
            }
        };
        match slot {
            0 => ring.rptr_offs = offs,
            1 => ring.wptr_offs = offs,
            2 => ring.fence_offs = offs,
            3 => ring.trail_fence_offs = offs,
            _ => ring.cond_exe_offs = offs,
        }
    }
    let gpu = |offs: u32| adev.wb.gpu_addr + offs as u64 * 4;
    ring.fence_gpu_addr = gpu(ring.fence_offs);
    ring.rptr_gpu_addr = gpu(ring.rptr_offs);
    ring.wptr_gpu_addr = gpu(ring.wptr_offs);
    ring.trail_fence_gpu_addr = gpu(ring.trail_fence_offs);
    ring.cond_exe_gpu_addr = gpu(ring.cond_exe_offs);

    // always set cond_exec_polling to CONTINUE
    wb_w32(adev, ring.cond_exe_offs, 1);

    if ring.funcs.type_ != AMDGPU_RING_TYPE_CPER {
        let r = amdgpu_fence_driver_start_ring(adev, &mut ring, irq_src, irq_type);
        if r != 0 {
            Line::new().s("failed initializing fences (").dec((-r) as u64).s(").").say();
            return Err(r);
        }
        let f = ring.funcs;
        let mut max_ibs_dw = f.emit_frame_size + amdgpu_ring_max_ibs(f.type_) * f.emit_ib_size;
        max_ibs_dw = (max_ibs_dw + f.align_mask) & !f.align_mask;
        if max_ibs_dw > max_dw {
            Line::new().s("WARNING: max_ibs_dw > max_dw").say();
            max_dw = max_ibs_dw;
        }
        ring.ring_size = crate::linux::roundup_pow_of_two((max_dw * 4 * sched_hw_submission) as u64) as u32;
    } else {
        ring.ring_size = crate::linux::roundup_pow_of_two(max_dw as u64 * 4) as u32;
        ring.wptr = 0;
        wb_w32(adev, ring.rptr_offs, 0);
    }

    ring.buf_mask = (ring.ring_size / 4) - 1;
    ring.ptr_mask = if ring.funcs.support_64bit_ptrs { u64::MAX } else { ring.buf_mask as u64 };
    // Initialize cached_rptr to 0
    ring.cached_rptr = 0;

    if ring.ring_backup.is_empty() {
        ring.ring_backup = vec![0; ring.ring_size as usize / 4];
    }

    // Allocate ring buffer
    if ring.ring_obj.is_none() {
        let size = ring.ring_size as u64 + ring.funcs.extra_bytes as u64;
        match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, crate::amdgpu_object::PAGE_SIZE,
            AMDGPU_GEM_DOMAIN_GTT, true) {
            Ok(Some(h)) => {
                ring.ring_obj = Some(h);
                ring.gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
                ring.ring = crate::amdgpu_object::bo(adev, h).kptr;
            }
            Ok(None) => return Err(-12),
            Err(r) => {
                Line::new().s("(").dec((-r) as u64).s(") ring create failed").say();
                return Err(r);
            }
        }
        // amdgpu_ring_clear_ring
        if let Some(k) = ring.ring {
            crate::amdgpu_gart::kptr_fill32(adev, k, 0, (ring.buf_mask as u64 + 1) * 4, ring.funcs.nop);
        }
    }

    ring.max_dw = max_dw;
    ring.hw_prio = hw_prio;

    let idx = adev.rings.len();
    if !ring.no_scheduler && ring.funcs.type_ < AMDGPU_HW_IP_NUM {
        adev.gpu_sched[ring.funcs.type_ as usize][hw_prio as usize].push(idx);
    }
    adev.rings.push(ring);
    Ok(idx)
}

/// `amdgpu_device_should_recover_gpu`: Renoir is not in the list recovery
/// is off for by default, and has no RAS poison mode.
pub fn amdgpu_device_should_recover_gpu() -> bool {
    if crate::tables::amdgpu_gpu_recovery[0] == 0 {
        Line::new().s("GPU recovery disabled.").say();
        return false;
    }
    true
}

/// `amdgpu_ring_alloc_reemit`: no `max_dw` check, as several submissions
/// may come back.
fn amdgpu_ring_alloc_reemit(adev: &mut amdgpu_device, r: usize, ndw: u32) {
    let ring = &mut adev.rings[r];
    // Align requested size with padding so unlock_commit can pad safely
    let ndw = (ndw + ring.funcs.align_mask) & !ring.funcs.align_mask;
    ring.count_dw = ndw as i32;
    ring.wptr_old = ring.wptr;
}

/// `amdgpu_ring_reset`: `funcs->reset`. Of Renoir's rings with one, only
/// the compute rings ever run a job here (VCN and JPEG get no hw_init).
pub fn amdgpu_ring_reset(adev: &mut amdgpu_device, r: usize, vmid: u32, guilty: &crate::dma_fence::Fence) -> i32 {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_compute => crate::gfx_v9_0::gfx_v9_0_reset_kcq(adev, r, vmid, guilty),
        _ => -95,
    }
}

/// Whether the ring's funcs have `reset`.
pub fn amdgpu_ring_has_reset(adev: &amdgpu_device, r: usize) -> bool {
    matches!(adev.rings[r].funcs.ops, ring_ops::gfx_v9_0_compute | ring_ops::vcn_v2_0_dec | ring_ops::vcn_v2_0_enc
        | ring_ops::jpeg_v2_0_dec)
}

/// `amdgpu_ring_reset_helper_begin`.
pub fn amdgpu_ring_reset_helper_begin(adev: &mut amdgpu_device, r: usize, guilty: Option<&crate::dma_fence::Fence>) {
    // Stop the scheduler to prevent anybody else from touching the ring
    // buffer.
    crate::drm_sched::drm_sched_wqueue_stop(adev, r);
    // back up the non-guilty commands
    crate::amdgpu_fence::amdgpu_ring_backup_unprocessed_commands(adev, r, guilty);
}

/// `amdgpu_ring_reset_helper_end`.
pub fn amdgpu_ring_reset_helper_end(adev: &mut amdgpu_device, r: usize, guilty: Option<&crate::dma_fence::Fence>)
                                    -> i32 {
    // verify that the ring is functional
    let ret = amdgpu_ring_test_ring(adev, r);
    if ret != 0 {
        return ret;
    }
    // signal the guilty fence and set an error on all fences from the
    // context
    if let Some(g) = guilty {
        crate::amdgpu_fence::amdgpu_fence_driver_guilty_force_completion(adev, g);
    }
    // Re-emit the non-guilty commands
    let n = adev.rings[r].ring_backup_entries_to_copy;
    if n != 0 {
        amdgpu_ring_alloc_reemit(adev, r, n);
        for i in 0..n as usize {
            let v = adev.rings[r].ring_backup[i];
            amdgpu_ring_write(adev, r, v);
        }
        amdgpu_ring_commit(adev, r);
    }
    // Start the scheduler again
    crate::drm_sched::drm_sched_wqueue_start(adev, r);
    0
}

/// `amdgpu_ring_is_reset_type_supported`.
pub fn amdgpu_ring_is_reset_type_supported(adev: &amdgpu_device, r: usize, reset_type: u32) -> bool {
    let mask = match adev.rings[r].funcs.type_ {
        AMDGPU_RING_TYPE_GFX => adev.gfx.gfx_supported_reset,
        AMDGPU_RING_TYPE_COMPUTE => adev.gfx.compute_supported_reset,
        AMDGPU_RING_TYPE_SDMA => adev.sdma.supported_reset,
        AMDGPU_RING_TYPE_VCN_DEC | AMDGPU_RING_TYPE_VCN_ENC => adev.vcn.supported_reset,
        AMDGPU_RING_TYPE_VCN_JPEG => adev.jpeg.supported_reset,
        _ => 0,
    };
    mask & reset_type != 0
}

/// `amdgpu_ring_soft_recovery`: the ring's `soft_recovery` again and
/// again for up to 10 ms, until `fence` signals.
pub fn amdgpu_ring_soft_recovery(adev: &mut amdgpu_device, r: usize, vmid: u32, fence: Option<&crate::dma_fence::Fence>)
                                 -> bool {
    let deadline = crate::linux::now_us() + 10_000;
    let Some(fence) = fence else { return false };
    if !adev.rings[r].funcs.soft_recovery {
        return false;
    }
    if !crate::dma_fence::dma_fence_is_signaled(adev, fence) {
        crate::dma_fence::dma_fence_set_error(fence, -61);
    }
    while !crate::dma_fence::dma_fence_is_signaled(adev, fence) && crate::linux::now_us() < deadline {
        // gfx_v9_0_ring_funcs_gfx and _compute: the only ones with one
        crate::gfx_v9_0::gfx_v9_0_ring_soft_recovery(adev, vmid);
    }
    let ret = crate::dma_fence::dma_fence_is_signaled(adev, fence);
    // increment the counter only if soft reset worked
    if ret {
        adev.gpu_reset_counter += 1;
    }
    ret
}

/// `amdgpu_get_soft_full_reset_mask`.
pub fn amdgpu_get_soft_full_reset_mask(adev: &amdgpu_device, ring: Option<usize>) -> u32 {
    let Some(ring) = ring.and_then(|i| adev.rings.get(i)) else { return 0 };
    let mut size = 0;
    if amdgpu_device_should_recover_gpu() {
        size |= AMDGPU_RESET_TYPE_FULL;
    }
    if !adev.debug_disable_soft_recovery && ring.funcs.soft_recovery {
        size |= AMDGPU_RESET_TYPE_SOFT_RESET;
    }
    size
}

/// `amdgpu_ring_alloc`. `begin_use` is the GFX rings'
/// `amdgpu_gfx_enforce_isolation_ring_begin_use` (and the compute rings'
/// wrapper around it): with isolation disabled, as `amdgpu_enforce_isolation`
/// -1 leaves it, it schedules nothing. The VCN and JPEG rings have
/// `amdgpu_vcn_ring_begin_use` and `amdgpu_jpeg_ring_begin_use`.
pub fn amdgpu_ring_alloc(adev: &mut amdgpu_device, r: usize, ndw: u32) -> i32 {
    let ring = &mut adev.rings[r];
    // Align requested size with padding so unlock_commit can pad safely
    let ndw = (ndw + ring.funcs.align_mask) & !ring.funcs.align_mask;
    // Make sure we aren't trying to allocate more space than the maximum
    // for one submission
    if ndw > ring.max_dw {
        Line::new().s("WARNING: amdgpu_ring_alloc ").dec(ndw as u64).s(" > max_dw ").dec(ring.max_dw as u64).say();
        return -12;
    }
    ring.count_dw = ndw as i32;
    ring.wptr_old = ring.wptr;
    match adev.rings[r].funcs.ops {
        ring_ops::vcn_v2_0_dec | ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::amdgpu_vcn_ring_begin_use(adev, r),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::amdgpu_jpeg_ring_begin_use(adev, r),
        _ => {}
    }
    0
}

/// `amdgpu_ring_write`.
pub fn amdgpu_ring_write(adev: &mut amdgpu_device, r: usize, v: u32) {
    let ring = &mut adev.rings[r];
    let at = ring.wptr & ring.buf_mask as u64;
    ring.wptr = (ring.wptr + 1) & ring.ptr_mask;
    ring.count_dw -= 1;
    if let Some(k) = ring.ring {
        crate::amdgpu_gart::kptr_w32(adev, k, at * 4, v);
    }
}

/// `amdgpu_ring_insert_nop`.
pub fn amdgpu_ring_insert_nop(adev: &mut amdgpu_device, r: usize, count: u32) {
    let ring = &adev.rings[r];
    let occupied = (ring.wptr & ring.buf_mask as u64) as u32;
    let chunk1 = (ring.buf_mask + 1 - occupied).min(count);
    let chunk2 = count - chunk1;
    let (k, nop) = (ring.ring, ring.funcs.nop);
    if let Some(k) = k {
        if chunk1 != 0 {
            crate::amdgpu_gart::kptr_fill32(adev, k, occupied as u64 * 4, chunk1 as u64 * 4, nop);
        }
        if chunk2 != 0 {
            crate::amdgpu_gart::kptr_fill32(adev, k, 0, chunk2 as u64 * 4, nop);
        }
    }
    let ring = &mut adev.rings[r];
    ring.wptr = (ring.wptr + count as u64) & ring.ptr_mask;
    ring.count_dw -= count as i32;
}

/// `amdgpu_ring_clear_ring`.
pub fn amdgpu_ring_clear_ring(adev: &mut amdgpu_device, r: usize) {
    let ring = &adev.rings[r];
    if let Some(k) = ring.ring {
        crate::amdgpu_gart::kptr_fill32(adev, k, 0, (ring.buf_mask as u64 + 1) * 4, ring.funcs.nop);
    }
}

/// `ring->funcs->insert_nop`.
fn insert_nop(adev: &mut amdgpu_device, r: usize, count: u32) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute => crate::gfx_v9_0::gfx_v9_ring_insert_nop(adev, r, count),
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => crate::sdma_v4_0::sdma_v4_0_ring_insert_nop(adev, r, count),
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_insert_nop(adev, r, count),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_nop(adev, r, count),
        _ => amdgpu_ring_insert_nop(adev, r, count),
    }
}

/// `amdgpu_ring_set_wptr`.
pub fn amdgpu_ring_set_wptr(adev: &mut amdgpu_device, r: usize) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx => crate::gfx_v9_0::gfx_v9_0_ring_set_wptr_gfx(adev, r),
        ring_ops::gfx_v9_0_compute | ring_ops::gfx_v9_0_kiq => crate::gfx_v9_0::gfx_v9_0_ring_set_wptr_compute(adev, r),
        ring_ops::sdma_v4_0 => crate::sdma_v4_0::sdma_v4_0_ring_set_wptr(adev, r),
        ring_ops::sdma_v4_0_page => crate::sdma_v4_0::sdma_v4_0_page_ring_set_wptr(adev, r),
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_set_wptr(adev, r),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::vcn_v2_0_enc_ring_set_wptr(adev, r),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_set_wptr(adev, r),
    }
}

/// `amdgpu_ring_commit`. `end_use` is VCN's and JPEG's (see `amdgpu_ring_alloc`).
pub fn amdgpu_ring_commit(adev: &mut amdgpu_device, r: usize) {
    let ring = &adev.rings[r];
    if ring.count_dw < 0 {
        Line::new().s("amdgpu: writing more dwords to the ring than expected!").say();
    }
    // We pad to match fetch size
    let mut count = ring.funcs.align_mask + 1 - (ring.wptr as u32 & ring.funcs.align_mask);
    count &= ring.funcs.align_mask;
    if count != 0 {
        insert_nop(adev, r, count);
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    amdgpu_ring_set_wptr(adev, r);
    match adev.rings[r].funcs.ops {
        ring_ops::vcn_v2_0_dec | ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::amdgpu_vcn_ring_end_use(adev, r),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::amdgpu_jpeg_ring_end_use(adev, r),
        _ => {}
    }
}

/// `amdgpu_ring_undo`.
pub fn amdgpu_ring_undo(adev: &mut amdgpu_device, r: usize) {
    let ring = &mut adev.rings[r];
    ring.wptr = ring.wptr_old;
}

/// `amdgpu_ring_test_ring`.
pub fn amdgpu_ring_test_ring(adev: &mut amdgpu_device, r: usize) -> i32 {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute | ring_ops::gfx_v9_0_kiq => {
            crate::gfx_v9_0::gfx_v9_0_ring_test_ring(adev, r)
        }
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => crate::sdma_v4_0::sdma_v4_0_ring_test_ring(adev, r),
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_test_ring(adev, r),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::amdgpu_vcn_enc_ring_test_ring(adev, r),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::amdgpu_jpeg_dec_ring_test_ring(adev, r),
    }
}

/// `amdgpu_ring_test_helper`.
pub fn amdgpu_ring_test_helper(adev: &mut amdgpu_device, r: usize) -> i32 {
    let ret = amdgpu_ring_test_ring(adev, r);
    if ret != 0 {
        Line::new().s("ring ").s(&adev.rings[r].name.clone()).s(" test failed (").dec((-ret) as u64).s(")").say();
    } else {
        Line::new().s("ring test on ").s(&adev.rings[r].name.clone()).s(" succeeded").dbg();
    }
    adev.rings[r].sched.ready = ret == 0;
    ret
}

/// `atomic64_set(ring->wptr_cpu_addr, v)`: the ring's writeback wptr slot.
pub fn ring_wptr_cpu_set(adev: &amdgpu_device, r: usize, v: u64) {
    let offs = adev.rings[r].wptr_offs;
    if let Some(k) = adev.wb.wb {
        crate::amdgpu_gart::kptr_writeq(adev, k, offs as u64 * 4, v);
    }
}

/// `*ring->rptr_cpu_addr` / `atomic64_read(ring->wptr_cpu_addr)`.
pub fn ring_wb_read64(adev: &amdgpu_device, offs: u32) -> u64 {
    let Some(k) = adev.wb.wb else { return 0 };
    let lo = crate::amdgpu_gart::kptr_r32(adev, k, offs as u64 * 4) as u64;
    let hi = crate::amdgpu_gart::kptr_r32(adev, k, offs as u64 * 4 + 4) as u64;
    lo | hi << 32
}

/// `amdgpu_mm_wdoorbell`.
pub fn amdgpu_mm_wdoorbell(adev: &amdgpu_device, index: u32, v: u32) {
    if index < adev.doorbell.num_kernel_doorbells {
        if let Some(m) = adev.doorbell_mmio {
            m.w32(index * 4, v);
        }
    } else {
        Line::new().s("writing beyond doorbell aperture: 0x").hexw(index, 8).say();
    }
}

/// `amdgpu_mm_wdoorbell64`.
pub fn amdgpu_mm_wdoorbell64(adev: &amdgpu_device, index: u32, v: u64) {
    if index < adev.doorbell.num_kernel_doorbells {
        if let Some(m) = adev.doorbell_mmio {
            m.writeq(index * 4, v);
        }
    } else {
        Line::new().s("writing beyond doorbell aperture: 0x").hexw(index, 8).say();
    }
}

impl amdgpu_ring_funcs {
    /// `secure_submission_supported`.
    pub fn secure(&self) -> bool {
        matches!(self.ops, ring_ops::gfx_v9_0_gfx | ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page
            | ring_ops::vcn_v2_0_dec)
    }

    /// `parse_cs`: JPEG's ring reads an IB through VMID 0, so the IB a
    /// user names is copied into a kernel IB and checked first.
    pub fn parse_cs(&self) -> bool {
        self.ops == ring_ops::jpeg_v2_0_dec
    }

    /// `no_user_fence`.
    pub fn no_user_fence(&self) -> bool {
        matches!(self.ops, ring_ops::vcn_v2_0_dec | ring_ops::vcn_v2_0_enc | ring_ops::jpeg_v2_0_dec)
    }
}

/// `amdgpu_ring_emit_fence`.
pub fn amdgpu_ring_emit_fence(adev: &mut amdgpu_device, r: usize, addr: u64, seq: u64, flags: u32) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute => {
            crate::gfx_v9_0::gfx_v9_0_ring_emit_fence(adev, r, addr, seq, flags)
        }
        ring_ops::gfx_v9_0_kiq => crate::gfx_v9_0::gfx_v9_0_ring_emit_fence_kiq(adev, r, addr, seq, flags),
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => {
            crate::sdma_v4_0::sdma_v4_0_ring_emit_fence(adev, r, addr, seq, flags)
        }
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_emit_fence(adev, r, addr, seq, flags),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::vcn_v2_0_enc_ring_emit_fence(adev, r, addr, seq, flags),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_emit_fence(adev, r, addr, seq, flags),
    }
}

/// `ring->funcs->emit_mem_sync`, where the ring has one.
pub fn amdgpu_ring_emit_mem_sync(adev: &mut amdgpu_device, r: usize) {
    if matches!(adev.rings[r].funcs.ops, ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute) {
        crate::gfx_v9_0::gfx_v9_0_emit_mem_sync(adev, r);
    }
}

/// `ring->funcs->emit_wave_limit`, where the ring has one.
pub fn amdgpu_ring_emit_wave_limit(adev: &mut amdgpu_device, r: usize, enable: bool) {
    if adev.rings[r].funcs.ops == ring_ops::gfx_v9_0_compute {
        crate::gfx_v9_0::gfx_v9_0_emit_wave_limit(adev, r, enable);
    }
}

/// `amdgpu_ring_init_cond_exec` on a ring with `init_cond_exec`; `None`
/// on the others, where `cond_exec` stays unset and unused.
pub fn amdgpu_ring_init_cond_exec(adev: &mut amdgpu_device, r: usize) -> Option<u32> {
    if adev.rings[r].funcs.ops == ring_ops::gfx_v9_0_gfx {
        let addr = adev.rings[r].cond_exe_gpu_addr;
        return Some(crate::gfx_v9_0::gfx_v9_0_ring_emit_init_cond_exec(adev, r, addr));
    }
    None
}

/// `amdgpu_ring_patch_cond_exec`.
pub fn amdgpu_ring_patch_cond_exec(adev: &mut amdgpu_device, r: usize, offset: Option<u32>) {
    let Some(offset) = offset else { return };
    let ring = &adev.rings[r];
    let Some(k) = ring.ring else { return };
    if offset > ring.buf_mask {
        Line::new().s("WARNING: amdgpu_ring_patch_cond_exec offset past the ring").say();
    }
    if crate::amdgpu_gart::kptr_r32(adev, k, offset as u64 * 4) != 0 {
        Line::new().s("WARNING: amdgpu_ring_patch_cond_exec on a used dword").say();
    }
    // amdgpu_ring_get_dw_distance(ring, offset, ring->wptr - 1)
    let start = offset & ring.buf_mask;
    let mut end = (ring.wptr.wrapping_sub(1) as u32) & ring.buf_mask;
    if end < start {
        end += ring.ring_size >> 2;
    }
    crate::amdgpu_gart::kptr_w32(adev, k, offset as u64 * 4, end - start);
}

/// `amdgpu_ring_emit_ib`: `gds` is whether the job uses GDS.
pub fn amdgpu_ring_emit_ib(adev: &mut amdgpu_device, r: usize, vmid: u32, gds: bool, ib: &crate::amdgpu_ib::amdgpu_ib,
                           flags: u32) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx => crate::gfx_v9_0::gfx_v9_0_ring_emit_ib_gfx(adev, r, vmid, gds, ib, flags),
        ring_ops::gfx_v9_0_compute => crate::gfx_v9_0::gfx_v9_0_ring_emit_ib_compute(adev, r, vmid, ib, flags),
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => {
            crate::sdma_v4_0::sdma_v4_0_ring_emit_ib(adev, r, vmid, ib, flags)
        }
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_emit_ib(adev, r, vmid, ib, flags),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::vcn_v2_0_enc_ring_emit_ib(adev, r, vmid, ib, flags),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_emit_ib(adev, r, vmid, ib, flags),
        _ => Line::new().s("emit_ib: ").s(adev.rings[r].funcs.name).s(" is not ported").say(),
    }
}

/// `amdgpu_ring_emit_wreg`.
pub fn amdgpu_ring_emit_wreg(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute | ring_ops::gfx_v9_0_kiq => {
            crate::gfx_v9_0::gfx_v9_0_ring_emit_wreg(adev, r, reg, val)
        }
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => crate::sdma_v4_0::sdma_v4_0_ring_emit_wreg(adev, r, reg, val),
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_emit_wreg(adev, r, reg, val),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::vcn_v2_0_enc_ring_emit_wreg(adev, r, reg, val),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_emit_wreg(adev, r, reg, val),
    }
}

/// Whether `ring->funcs->emit_wreg` is set.
pub fn has_emit_wreg(adev: &amdgpu_device, r: usize) -> bool {
    matches!(adev.rings[r].funcs.ops, ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute | ring_ops::gfx_v9_0_kiq
        | ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page | ring_ops::vcn_v2_0_dec | ring_ops::vcn_v2_0_enc
        | ring_ops::jpeg_v2_0_dec)
}

/// Whether `emit_pipeline_sync` is set: the VCN and JPEG rings have none.
pub fn has_emit_pipeline_sync(adev: &amdgpu_device, r: usize) -> bool {
    matches!(adev.rings[r].funcs.ops, ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute
        | ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page)
}

/// Whether `emit_vm_flush` is set.
pub fn has_emit_vm_flush(adev: &amdgpu_device, r: usize) -> bool {
    matches!(adev.rings[r].funcs.ops, ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute
        | ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page | ring_ops::vcn_v2_0_dec | ring_ops::vcn_v2_0_enc
        | ring_ops::jpeg_v2_0_dec)
}

/// `ring->funcs->insert_start`, where the ring has one.
pub fn amdgpu_ring_insert_start(adev: &mut amdgpu_device, r: usize) {
    match adev.rings[r].funcs.ops {
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_insert_start(adev, r),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_insert_start(adev, r),
        _ => {}
    }
}

/// `ring->funcs->insert_end`, where the ring has one.
pub fn amdgpu_ring_insert_end(adev: &mut amdgpu_device, r: usize) {
    match adev.rings[r].funcs.ops {
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_insert_end(adev, r),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::vcn_v2_0_enc_ring_insert_end(adev, r),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_insert_end(adev, r),
        _ => {}
    }
}

/// Whether `emit_gds_switch` is set.
pub fn has_emit_gds_switch(adev: &amdgpu_device, r: usize) -> bool {
    matches!(adev.rings[r].funcs.ops, ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute)
}

/// `amdgpu_ring_emit_reg_wait`.
pub fn amdgpu_ring_emit_reg_wait(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32, mask: u32) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute | ring_ops::gfx_v9_0_kiq => {
            crate::gfx_v9_0::gfx_v9_0_ring_emit_reg_wait(adev, r, reg, val, mask)
        }
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => {
            crate::sdma_v4_0::sdma_v4_0_ring_emit_reg_wait(adev, r, reg, val, mask)
        }
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_emit_reg_wait(adev, r, reg, val, mask),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::vcn_v2_0_enc_ring_emit_reg_wait(adev, r, reg, val, mask),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_emit_reg_wait(adev, r, reg, val, mask),
    }
}

/// `amdgpu_ring_emit_reg_write_reg_wait_helper`.
pub fn amdgpu_ring_emit_reg_write_reg_wait_helper(adev: &mut amdgpu_device, r: usize, reg0: u32, reg1: u32, ref_: u32,
                                                  mask: u32) {
    amdgpu_ring_emit_wreg(adev, r, reg0, ref_);
    amdgpu_ring_emit_reg_wait(adev, r, reg1, mask, mask);
}

/// `amdgpu_ring_emit_reg_write_reg_wait`.
pub fn amdgpu_ring_emit_reg_write_reg_wait(adev: &mut amdgpu_device, r: usize, reg0: u32, reg1: u32, ref_: u32,
                                           mask: u32) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute | ring_ops::gfx_v9_0_kiq => {
            crate::gfx_v9_0::gfx_v9_0_ring_emit_reg_write_reg_wait(adev, r, reg0, reg1, ref_, mask)
        }
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page | ring_ops::vcn_v2_0_dec | ring_ops::vcn_v2_0_enc
        | ring_ops::jpeg_v2_0_dec => amdgpu_ring_emit_reg_write_reg_wait_helper(adev, r, reg0, reg1, ref_, mask),
    }
}

/// `amdgpu_ring_emit_pipeline_sync`.
pub fn amdgpu_ring_emit_pipeline_sync(adev: &mut amdgpu_device, r: usize) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute => crate::gfx_v9_0::gfx_v9_0_ring_emit_pipeline_sync(adev, r),
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => crate::sdma_v4_0::sdma_v4_0_ring_emit_pipeline_sync(adev, r),
        _ => Line::new().s("emit_pipeline_sync: ").s(adev.rings[r].funcs.name).s(" is not ported").say(),
    }
}

/// `amdgpu_ring_emit_vm_flush`.
pub fn amdgpu_ring_emit_vm_flush(adev: &mut amdgpu_device, r: usize, vmid: u32, pd_addr: u64) {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_gfx | ring_ops::gfx_v9_0_compute => {
            crate::gfx_v9_0::gfx_v9_0_ring_emit_vm_flush(adev, r, vmid, pd_addr)
        }
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => {
            crate::sdma_v4_0::sdma_v4_0_ring_emit_vm_flush(adev, r, vmid, pd_addr)
        }
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::vcn_v2_0_dec_ring_emit_vm_flush(adev, r, vmid, pd_addr),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::vcn_v2_0_enc_ring_emit_vm_flush(adev, r, vmid, pd_addr),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::jpeg_v2_0_dec_ring_emit_vm_flush(adev, r, vmid, pd_addr),
        _ => Line::new().s("emit_vm_flush: ").s(adev.rings[r].funcs.name).s(" is not ported").say(),
    }
}

/// `amdgpu_ring_emit_gds_switch`.
pub fn amdgpu_ring_emit_gds_switch(adev: &mut amdgpu_device, r: usize, vmid: u32, gds_base: u32, gds_size: u32,
                                   gws_base: u32, gws_size: u32, oa_base: u32, oa_size: u32) {
    crate::gfx_v9_0::gfx_v9_0_ring_emit_gds_switch(adev, r, vmid, gds_base, gds_size, gws_base, gws_size, oa_base,
                                                   oa_size);
}

/// Whether `ring->funcs->test_ib` is set: compute, SDMA, VCN and JPEG
/// rings (the GFX ring of GC 9 has none).
pub fn has_test_ib(adev: &amdgpu_device, r: usize) -> bool {
    matches!(adev.rings[r].funcs.ops, ring_ops::gfx_v9_0_compute | ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page
        | ring_ops::vcn_v2_0_dec | ring_ops::vcn_v2_0_enc | ring_ops::jpeg_v2_0_dec)
}

/// `amdgpu_ring_test_ib`.
pub fn amdgpu_ring_test_ib(adev: &mut amdgpu_device, r: usize, timeout: i64) -> i32 {
    match adev.rings[r].funcs.ops {
        ring_ops::gfx_v9_0_compute => crate::gfx_v9_0::gfx_v9_0_ring_test_ib(adev, r, timeout),
        ring_ops::sdma_v4_0 | ring_ops::sdma_v4_0_page => crate::sdma_v4_0::sdma_v4_0_ring_test_ib(adev, r, timeout),
        ring_ops::vcn_v2_0_dec => crate::vcn_v2_0::amdgpu_vcn_dec_ring_test_ib(adev, r, timeout),
        ring_ops::vcn_v2_0_enc => crate::vcn_v2_0::amdgpu_vcn_enc_ring_test_ib(adev, r, timeout),
        ring_ops::jpeg_v2_0_dec => crate::jpeg_v2_0::amdgpu_jpeg_dec_ring_test_ib(adev, r, timeout),
        _ => -22,
    }
}
