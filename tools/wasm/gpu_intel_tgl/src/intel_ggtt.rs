//! gt/intel_ggtt.c, gt/intel_ggtt_fencing.c, gem/i915_gem_stolen.c and the
//! Intel part of arch/x86/kernel/early-quirks.c (Linux 6.18.55): the GGTT
//! probe and the stolen memory, on Tiger Lake.
//!
//! The GGTT is written through the register BAR, uncached: TGL drops GTT
//! writes wider than 64 bits (`needs_wc_ggtt_mapping` is false), and every
//! entry is one `writeq`. `gen8_ggtt_invalidate` writes nothing on TGL.
//! Page-table pages and the scratch page are DMA buffers the kernel
//! hands out (Linux allocates them as internal GEM objects).

use crate::drm_mm::drm_mm;
use crate::i915_drv::drm_i915_private;
use crate::linux::{self, Line};
use crate::regs::*;
use npk_sys::{Dma, Mmio};

/// `struct i915_ggtt`, as far as the probe sets it.
pub struct i915_ggtt {
    /// The register BAR; the GSM (GGTT entries) sits at `gsm_offset` in it.
    pub regs: Mmio,
    pub gsm_offset: u32,
    /// GMADR (the aperture): base and size.
    pub gmadr_start: u64,
    pub mappable_end: u64,
    pub total: u64,
    pub scratch: Option<Dma>,
    pub scratch_encode: u64,
    pub num_fences: u32,
    /// `ggtt->vm.mm`.
    pub vm_mm: drm_mm,
}

/// A stolen-memory GEM object bound into the GGTT (`struct i915_vma` with
/// its `drm_i915_gem_object`), as far as a preallocated object needs it.
#[derive(Clone, Copy, Default)]
pub struct i915_vma {
    /// `vma->node` in `ggtt->vm.mm`.
    pub node: crate::drm_mm::drm_mm_node,
    /// `obj->stolen`, offsets from the stolen region's start.
    pub stolen: crate::drm_mm::drm_mm_node,
    /// `obj->tiling_and_stride`.
    pub tiling_and_stride: u32,
    pub pat_index: u32,
}

/// `struct intel_graphics_stolen` / `i915->dsm`: stolen memory and the
/// reserved part at its top.
#[derive(Clone, Copy, Default)]
pub struct i915_dsm {
    pub stolen_start: u64,
    pub stolen_end: u64,
    pub reserved_start: u64,
    pub reserved_size: u64,
    pub usable_size: u64,
    /// `i915->mm.stolen_region->region`: the stolen range without the
    /// reserved part.
    pub region_start: u64,
    pub region_end: u64,
    /// `i915->mm.stolen`, offsets from `region_start`.
    pub mm: drm_mm,
}

const SZ_16M: u32 = 16 << 20;
const MB: u64 = 1 << 20;

/// `gen6_gttmmadr_size` (GRAPHICS_VER >= 8): 16 MB.
fn gen6_gttmmadr_size() -> u32 {
    SZ_16M
}

/// `gen6_gttadr_offset`: the GSM starts halfway into GTTMMADR.
fn gen6_gttadr_offset() -> u32 {
    gen6_gttmmadr_size() / 2
}

/// `gen8_get_total_gtt_size`.
fn gen8_get_total_gtt_size(mut bdw_gmch_ctl: u16) -> u32 {
    bdw_gmch_ctl >>= BDW_GMCH_GGMS_SHIFT;
    bdw_gmch_ctl &= BDW_GMCH_GGMS_MASK as u16;
    if bdw_gmch_ctl != 0 {
        bdw_gmch_ctl = 1 << bdw_gmch_ctl;
    }
    (bdw_gmch_ctl as u32) << 20
}

/// `gen8_ggtt_pte_encode`.
pub fn gen8_ggtt_pte_encode(addr: u64, _pat_index: u32, flags: u32) -> u64 {
    let mut pte = addr | GEN8_PAGE_PRESENT as u64;
    if flags & PTE_LM as u32 != 0 {
        pte |= GEN12_GGTT_PTE_LM as u64;
    }
    pte
}

impl i915_ggtt {
    fn pte_offset(&self, offset: u64) -> u32 {
        self.gsm_offset + ((offset / I915_GTT_PAGE_SIZE) * 8) as u32
    }

    /// `gen8_ggtt_read_entry` + `gen8_ggtt_pte_decode`: (address,
    /// is_present, is_local).
    pub fn gen8_ggtt_read_entry(&self, offset: u64) -> (u64, bool, bool) {
        let pte = self.regs.readq(self.pte_offset(offset));
        (pte & GEN12_GGTT_PTE_ADDR_MASK, pte & GEN8_PAGE_PRESENT != 0, pte & GEN12_GGTT_PTE_LM != 0)
    }

    /// `gen8_ggtt_insert_entries` for a physically contiguous object (a
    /// stolen one is one block): `node_size` bytes from `start`, the object's
    /// `obj_size` bytes from `addr`, the rest of the node on scratch. No
    /// guard pages (`vma_res->guard` is 0 without PIN_OFFSET_GUARD).
    pub fn gen8_ggtt_insert_entries(&self, start: u64, node_size: u64, addr: u64, obj_size: u64,
                                    pat_index: u32, flags: u32) {
        let pte_encode = gen8_ggtt_pte_encode(0, pat_index, flags);
        let mut gte = start;
        let end = start + node_size;
        let mut a = addr;
        while a < addr + obj_size {
            self.regs.writeq(self.pte_offset(gte), pte_encode | a);
            gte += I915_GTT_PAGE_SIZE;
            a += I915_GTT_PAGE_SIZE;
        }
        while gte < end {
            self.regs.writeq(self.pte_offset(gte), self.scratch_encode);
            gte += I915_GTT_PAGE_SIZE;
        }
    }

    /// `gen8_ggtt_insert_page`; `gen8_ggtt_invalidate` is a no-op on TGL.
    pub fn gen8_ggtt_insert_page(&self, addr: u64, offset: u64, pat_index: u32, flags: u32) {
        self.regs.writeq(self.pte_offset(offset), gen8_ggtt_pte_encode(addr, pat_index, flags));
    }

    /// `ggtt_total_entries`.
    fn ggtt_total_entries(&self) -> u64 {
        self.total / I915_GTT_PAGE_SIZE
    }

    /// `gen8_ggtt_clear_range`: point the range at the scratch page.
    pub fn gen8_ggtt_clear_range(&self, start: u64, length: u64) {
        let first_entry = start / I915_GTT_PAGE_SIZE;
        let mut num_entries = length / I915_GTT_PAGE_SIZE;
        let max_entries = self.ggtt_total_entries() - first_entry;
        if num_entries > max_entries {
            Line::new().s("warning: first entry ").dec(first_entry).s(", num entries ")
                .dec(num_entries).s(" (max ").dec(max_entries).s(")").say();
            num_entries = max_entries;
        }
        for i in 0..num_entries {
            self.regs.writeq(self.gsm_offset + ((first_entry + i) * 8) as u32, self.scratch_encode);
        }
    }
}

/// `setup_scratch_page` for the GGTT: a 4 KB page (the GGTT is not 4-level),
/// cleared (`poison_scratch_page` writes 0 without DRM_I915_DEBUG_GEM).
fn setup_scratch_page() -> Option<Dma> {
    let obj = Dma::alloc(1)?;
    if obj.phys & (I915_GTT_PAGE_SIZE_4K as u64 - 1) != 0 {
        return None;
    }
    Some(obj)
}

/// `ggtt_probe_common`.
fn ggtt_probe_common(ggtt: &mut i915_ggtt) -> Result<(), ()> {
    // i915_direct_stolen_access is Meteor Lake's; the GSM is reached
    // through GTTMMADR at gen6_gttadr_offset.
    ggtt.gsm_offset = gen6_gttadr_offset();
    let Some(scratch) = setup_scratch_page() else {
        linux::drm_err("Scratch setup failed");
        return Err(());
    };
    ggtt.scratch_encode = gen8_ggtt_pte_encode(scratch.phys, TGL_PAT_CACHE_NONE, 0);
    ggtt.scratch = Some(scratch);
    Ok(())
}

/// `gen8_gmch_probe`.
fn gen8_gmch_probe(ggtt: &mut i915_ggtt) -> Result<(), ()> {
    // !HAS_LMEM: the aperture is GMADR (BAR 2).
    let gmadr_size = npk_sys::pci_bar_size(GEN4_GMADR_BAR as i32);
    if gmadr_size <= 0 {
        return Err(());
    }
    let lo = npk_sys::pci_read_config(0x10 + GEN4_GMADR_BAR as i32 * 4) as u32;
    let hi = npk_sys::pci_read_config(0x14 + GEN4_GMADR_BAR as i32 * 4) as u32;
    ggtt.gmadr_start = ((hi as u64) << 32) | (lo & !0xF) as u64;
    ggtt.mappable_end = gmadr_size as u64;

    let snb_gmch_ctl = (npk_sys::pci_read_config(SNB_GMCH_CTRL as i32 & !3) as u32
        >> ((SNB_GMCH_CTRL & 3) * 8)) as u16;
    let size = gen8_get_total_gtt_size(snb_gmch_ctl);
    ggtt.total = (size as u64 / 8) * I915_GTT_PAGE_SIZE;

    ggtt_probe_common(ggtt)
}

/// `ggtt_probe_hw` / `i915_ggtt_probe_hw`.
pub fn i915_ggtt_probe_hw(regs: Mmio) -> Result<i915_ggtt, ()> {
    let mut ggtt = i915_ggtt {
        regs,
        gsm_offset: 0,
        gmadr_start: 0,
        mappable_end: 0,
        total: 0,
        scratch: None,
        scratch_encode: 0,
        num_fences: 0,
        vm_mm: drm_mm::default(),
    };
    gen8_gmch_probe(&mut ggtt)?;
    if (ggtt.total - 1) >> 32 != 0 {
        linux::drm_err("We never expected a Global GTT with more than 32bits of address space!");
        ggtt.total = 1 << 32;
        ggtt.mappable_end = ggtt.mappable_end.min(ggtt.total);
    }
    if ggtt.mappable_end > ggtt.total {
        linux::drm_err("mappable aperture extends past end of GGTT");
        ggtt.mappable_end = ggtt.total;
    }
    Ok(ggtt)
}

/// `i965_write_fence_reg` for an unused fence (`fence->tiling` == 0): the
/// register is turned off, then written in two halves, low half last.
fn i965_write_fence_reg(i915: &mut drm_i915_private, id: u32) {
    let fence_reg_lo = FENCE_REG_GEN6_LO(id);
    let fence_reg_hi = FENCE_REG_GEN6_HI(id);
    let val: u64 = 0;
    let u = &mut i915.uncore;
    u.intel_uncore_write_fw(fence_reg_lo, 0);
    let _ = u.intel_uncore_read_fw(fence_reg_lo);
    u.intel_uncore_write_fw(fence_reg_hi, (val >> 32) as u32);
    u.intel_uncore_write_fw(fence_reg_lo, val as u32);
    let _ = u.intel_uncore_read_fw(fence_reg_lo);
}

/// `intel_ggtt_init_fences`: no bit-6 swizzling from Gen8 on; 32 fences
/// with an aperture; all written to zero (`intel_ggtt_restore_fences`).
fn intel_ggtt_init_fences(i915: &mut drm_i915_private, ggtt: &mut i915_ggtt) {
    ggtt.num_fences = if ggtt.mappable_end != 0 { 32 } else { 0 };
    for i in 0..ggtt.num_fences {
        i965_write_fence_reg(i915, i);
    }
}

/// `ggtt_init_hw` / `i915_ggtt_init_hw`: the range allocator of
/// `i915_address_space_init`, then the fences. The WC io_mapping of the
/// aperture is the kernel's.
pub fn i915_ggtt_init_hw(i915: &mut drm_i915_private, ggtt: &mut i915_ggtt) {
    ggtt.vm_mm = drm_mm::drm_mm_init(0, ggtt.total);
    intel_ggtt_init_fences(i915, ggtt);
}

/// `gen11_stolen_base` + `gen9_stolen_size` (early-quirks `gen11_early_ops`,
/// TGL is in its table): base from BSM, size from GGC's GMS field.
pub fn intel_graphics_stolen() -> (u64, u64) {
    let gmch_ctrl = (npk_sys::pci_read_config(SNB_GMCH_CTRL as i32 & !3) as u32
        >> ((SNB_GMCH_CTRL & 3) * 8)) as u16;
    let gms = (gmch_ctrl >> BDW_GMCH_GMS_SHIFT) & BDW_GMCH_GMS_MASK as u16;
    let size = if gms < 0xf0 {
        gms as u64 * 32 * MB
    } else {
        (gms as u64 - 0xf0) * 4 * MB + 4 * MB
    };
    let mut bsm = npk_sys::pci_read_config(INTEL_GEN11_BSM_DW0 as i32) as u32 as u64;
    bsm &= INTEL_BSM_MASK as u64;
    bsm |= (npk_sys::pci_read_config(INTEL_GEN11_BSM_DW1 as i32) as u32 as u64) << 32;
    if size == 0 || bsm == 0 {
        return (0, 0);
    }
    (bsm, size)
}

/// `icl_get_stolen_reserved`.
fn icl_get_stolen_reserved(i915: &mut drm_i915_private, base: &mut u64, size: &mut u64) {
    let reg_val = i915.uncore.intel_uncore_read64(GEN6_STOLEN_RESERVED);
    match reg_val as u32 & GEN8_STOLEN_RESERVED_SIZE_MASK {
        GEN8_STOLEN_RESERVED_1M => *size = MB,
        GEN8_STOLEN_RESERVED_2M => *size = 2 * MB,
        GEN8_STOLEN_RESERVED_4M => *size = 4 * MB,
        GEN8_STOLEN_RESERVED_8M => *size = 8 * MB,
        other => {
            *size = 8 * MB;
            Line::new().s("warning: missing case stolen reserved ").hex(other).say();
        }
    }
    // !HAS_LMEMBAR_SMEM_STOLEN
    *base = reg_val & GEN11_STOLEN_RESERVED_ADDR_MASK;
}

/// `init_reserved_stolen` (GRAPHICS_VER >= 11).
fn init_reserved_stolen(i915: &mut drm_i915_private, dsm: &mut i915_dsm) -> Result<(), ()> {
    let stolen_top = dsm.stolen_end + 1;
    let mut reserved_base = stolen_top;
    let mut reserved_size = 0u64;
    icl_get_stolen_reserved(i915, &mut reserved_base, &mut reserved_size);

    let bail = |dsm: &mut i915_dsm, base: u64| {
        dsm.reserved_start = base;
        dsm.reserved_size = 0;
    };
    if reserved_base == stolen_top {
        bail(dsm, reserved_base);
        return Ok(());
    }
    if reserved_base == 0 {
        linux::drm_err("inconsistent stolen reservation; ignoring");
        bail(dsm, reserved_base);
        return Err(());
    }
    dsm.reserved_start = reserved_base;
    dsm.reserved_size = reserved_size;
    let res_end = reserved_base + reserved_size - 1;
    if !(reserved_base >= dsm.stolen_start && res_end <= dsm.stolen_end) {
        Line::new().s("error: stolen reserved area ").hex((reserved_base >> 32) as u32)
            .hex(reserved_base as u32).s(" outside stolen memory").say();
        bail(dsm, reserved_base);
        return Err(());
    }
    Ok(())
}

/// `i915_gem_stolen_smem_setup` + `init_stolen_smem` + `i915_gem_init_stolen`.
/// `request_smem_stolen` claims the range in the kernel's resource tree;
/// here the range is reported for the kernel to compare with its memory map.
pub fn i915_gem_init_stolen(i915: &mut drm_i915_private) -> Option<i915_dsm> {
    let (base, size) = intel_graphics_stolen();
    let mut dsm = i915_dsm { stolen_start: base, stolen_end: base.wrapping_add(size).wrapping_sub(1), ..Default::default() };
    // adjust_stolen: valid_stolen_size (start != 0, end > start).
    if !(dsm.stolen_start != 0 && dsm.stolen_end > dsm.stolen_start) {
        linux::say("Skip stolen region: failed to setup");
        return None;
    }
    if init_reserved_stolen(i915, &mut dsm).is_err() {
        linux::say("Skip stolen region: failed to setup");
        return None;
    }
    // Exclude the reserved region from driver use.
    dsm.region_start = dsm.stolen_start;
    dsm.region_end = dsm.reserved_start - 1;
    dsm.usable_size = dsm.reserved_start - dsm.stolen_start;
    if dsm.usable_size == 0 {
        return None;
    }
    dsm.mm = drm_mm::drm_mm_init(0, dsm.usable_size);
    Some(dsm)
}
