//! The GEM objects and GGTT bindings the engine side needs (Linux 6.18.55
//! gem/i915_gem_internal.c, gem/i915_gem_stolen.c, i915_vma.c,
//! i915_gem_gtt.c): internal objects, stolen objects, and `i915_ggtt_pin`.
//!
//! This is class B in GPU_INTEL_LINUX_MAP.md (object bookkeeping, the
//! gpudev side later); what is kept is what the GPU sees of it: where
//! an object lands in the GGTT and which entries point at it. An internal
//! object is one contiguous DMA buffer from the kernel (Linux takes single
//! pages; the GPU sees the same through the GGTT). Nothing is unbound: on
//! TGL `clear_range` is `nop_clear_range`, so an unbind writes no entry,
//! and the module has no heap that frees.

use crate::drm_mm::drm_mm_node;
use crate::intel_ggtt::{i915_dsm, i915_ggtt};
use crate::regs::*;
use npk_sys::Dma;

const ENOMEM: i32 = 12;
const ENOSPC: i32 = 28;
const ENODEV: i32 = 19;

/// Where an object's pages are.
#[derive(Clone, Copy)]
pub enum gem_backing {
    /// `i915_gem_object_create_internal` / `_shmem`: system pages.
    dma(Dma),
    /// `i915_gem_object_create_stolen`: the node in `i915->mm.stolen`.
    stolen(drm_mm_node),
}

/// `struct drm_i915_gem_object`, its pages and caching.
#[derive(Clone, Copy)]
pub struct drm_i915_gem_object {
    pub backing: gem_backing,
    pub size: u64,
    /// Bus address of the first page; the object is contiguous.
    pub phys: u64,
    pub pat_index: u32,
}

impl drm_i915_gem_object {
    /// The CPU's view of a system-memory object (`i915_gem_object_pin_map`).
    pub fn map(&self) -> Option<Dma> {
        match self.backing {
            gem_backing::dma(d) => Some(d),
            gem_backing::stolen(_) => None,
        }
    }

    pub fn is_stolen(&self) -> bool {
        matches!(self.backing, gem_backing::stolen(_))
    }
}

/// `i915_gem_object_create_internal`, also standing for `_create_shmem`:
/// cached (`I915_CACHE_LLC`, the platform has an LLC).
pub fn i915_gem_object_create_internal(size: u64) -> Result<drm_i915_gem_object, i32> {
    let size = size.next_multiple_of(I915_GTT_PAGE_SIZE);
    let d = Dma::alloc((size / I915_GTT_PAGE_SIZE) as i32).ok_or(-ENOMEM)?;
    Ok(drm_i915_gem_object { backing: gem_backing::dma(d), size, phys: d.phys, pat_index: TGL_PAT_CACHE_LLC })
}

/// `i915_gem_object_create_stolen`: `i915_gem_stolen_insert_node` (best
/// fit from I915_GEM_STOLEN_BIAS, never the first page), cached like an
/// internal object on an LLC platform.
pub fn i915_gem_object_create_stolen(dsm: Option<&mut i915_dsm>, size: u64) -> Result<drm_i915_gem_object, i32> {
    let dsm = dsm.ok_or(-ENODEV)?;
    if !dsm.mm.initialized() {
        return Err(-ENODEV);
    }
    if size == 0 {
        return Err(-ENOSPC);
    }
    let size = size.next_multiple_of(I915_GTT_PAGE_SIZE);
    let start = (I915_GEM_STOLEN_BIAS as u64).max(4096);
    let node = dsm.mm.drm_mm_insert_node_in_range_best(size, I915_GTT_PAGE_SIZE, 0, start, u64::MAX)?;
    Ok(drm_i915_gem_object {
        backing: gem_backing::stolen(node),
        size,
        phys: dsm.region_start + node.start,
        pat_index: TGL_PAT_CACHE_LLC,
    })
}

/// `struct i915_vma` of an object bound into the GGTT.
#[derive(Clone, Copy)]
pub struct i915_vma {
    pub obj: drm_i915_gem_object,
    pub node: drm_mm_node,
}

impl i915_vma {
    /// `i915_ggtt_offset`.
    pub fn ggtt_offset(&self) -> u32 {
        self.node.start as u32
    }
}

/// The PIN_* flags `i915_ggtt_pin` is called with here.
#[derive(Clone, Copy, Default)]
pub struct pin_flags {
    pub high: bool,
    pub mappable: bool,
    /// PIN_OFFSET_BIAS | offset.
    pub bias: u64,
}

/// `i915_ggtt_pin` → `i915_vma_insert` (`i915_gem_gtt_insert`: PIN_HIGH is
/// DRM_MM_INSERT_HIGHEST, PIN_MAPPABLE DRM_MM_INSERT_LOW, else BEST; a
/// failed HIGHEST retries BEST) → `ggtt_bind_vma`. Nothing is evicted:
/// every binding here is permanent, so a full GGTT is -ENOSPC.
pub fn i915_ggtt_pin(ggtt: &mut i915_ggtt, obj: drm_i915_gem_object, flags: pin_flags) -> Result<i915_vma, i32> {
    let size = obj.size;
    let start = flags.bias;
    let mut end = ggtt.total;
    if flags.mappable {
        end = end.min(ggtt.mappable_end);
    }
    if size > end || start > end - size {
        return Err(-ENOSPC);
    }
    let node = if flags.mappable {
        ggtt.vm_mm.drm_mm_insert_node_in_range_low(size, 0, start, end)?
    } else if flags.high {
        match ggtt.vm_mm.drm_mm_insert_node_in_range_highest(size, 0, 0, start, end) {
            Ok(n) => n,
            Err(_) => ggtt.vm_mm.drm_mm_insert_node_in_range_best(size, 0, 0, start, end)?,
        }
    } else {
        ggtt.vm_mm.drm_mm_insert_node_in_range_best(size, 0, 0, start, end)?
    };
    // ggtt_bind_vma: neither read-only nor local memory.
    ggtt.gen8_ggtt_insert_entries(node.start, node.size, obj.phys, obj.size, obj.pat_index, 0);
    Ok(i915_vma { obj, node })
}
