//! display/intel_plane_initial.c (Linux 6.18.55): reading the firmware's
//! framebuffer off each active pipe and taking it over — the stolen memory
//! under it is reserved, it is bound into the GGTT at a fixed offset, and
//! the primary plane's state points at it, so the GGTT setup of
//! `i915_gem_init` leaves it in place.
//!
//! `intel_framebuffer_init` (the DRM framebuffer object, its views and
//! frontbuffer tracking) belongs to the plane commit machinery and is not
//! part of this; the framebuffer stays as the readout describes it.

use crate::drm_mm::drm_mm_node;
use crate::i915_drv::drm_i915_private;
use crate::intel_ggtt::{i915_dsm, i915_ggtt, i915_vma};
use crate::linux::{self, Line};
use crate::regs::*;

/// The framebuffer the plane scans out (`plane_config->fb`).
#[derive(Clone, Copy, Default)]
pub struct intel_initial_fb {
    pub width: u32,
    pub height: u32,
    /// `fb->pitches[0]`, bytes.
    pub pitch: u32,
    /// DRM fourcc.
    pub format: u32,
    /// Bytes per pixel of colour plane 0.
    pub cpp: u32,
    pub modifier: u64,
}

/// `struct intel_initial_plane_config`.
#[derive(Clone, Copy, Default)]
pub struct intel_initial_plane_config {
    pub fb: Option<intel_initial_fb>,
    pub vma: Option<i915_vma>,
    /// `PLANE_SURF`: the GGTT offset the plane scans out from.
    pub base: u32,
    /// Bytes: pitch times the tile-aligned height.
    pub size: u32,
    pub tiling: u32,
    pub rotation: u32,
    /// Offset of the framebuffer in the stolen region.
    pub phys_base: u64,
}

/// `mem->min_page_size` of the stolen-system region (PAGE_SIZE).
const STOLEN_MIN_PAGE_SIZE: u64 = 4096;

/// `intel_plane_initial_vblank_wait`.
fn intel_plane_initial_vblank_wait(i915: &mut drm_i915_private, pipe: u32) {
    crate::intel_display_irq::intel_crtc_wait_for_next_vblank(i915, pipe);
}

/// `intel_reuse_initial_plane_obj`: another active pipe already took over
/// a framebuffer at the same GGTT offset.
fn intel_reuse_initial_plane_obj(i915: &drm_i915_private, this_pipe: u32,
                                 plane_configs: &[intel_initial_plane_config])
                                 -> Option<(intel_initial_fb, i915_vma)> {
    for c in 0..I915_MAX_PIPES as usize {
        if !i915.display.crtc_valid[c] {
            continue;
        }
        let crtc = &i915.display.crtcs[c];
        let Some(p) = crate::intel_crtc::primary_plane(i915, crtc.pipe) else {
            continue;
        };
        let plane_state = &i915.display.planes[p].state;
        if !crtc.state.uapi_active {
            continue;
        }
        let Some(vma) = plane_state.ggtt_vma else {
            continue;
        };
        if plane_configs[this_pipe as usize].base == plane_configs[crtc.pipe as usize].base {
            return plane_state.fb.map(|fb| (fb, vma));
        }
    }
    None
}

/// `initial_plane_phys`: the framebuffer's place in stolen memory, from
/// the GGTT entry the plane's base points at. The memory type is
/// INTEL_MEMORY_STOLEN_SYSTEM (integrated, no LMEMBAR stolen).
fn initial_plane_phys(ggtt: &i915_ggtt, dsm: Option<&i915_dsm>,
                      plane_config: &mut intel_initial_plane_config) -> bool {
    let Some(mem) = dsm else {
        linux::say("Initial plane memory region (type stolen-system) not initialized");
        return false;
    };

    let base = plane_config.base as u64 & !(I915_GTT_MIN_ALIGNMENT - 1);

    let (dma_addr, is_present, is_local) = ggtt.gen8_ggtt_read_entry(base);

    if !is_present {
        linux::drm_err("Initial plane FB PTE not present");
        return false;
    }

    // intel_memory_type_is_local(stolen-system) is false.
    if is_local {
        linux::drm_err("Initial plane FB PTE unsuitable for stolen-system");
        return false;
    }

    if dma_addr < mem.region_start || dma_addr > mem.region_end {
        Line::new().s("error: Initial plane programming using invalid range, dma_addr=").hex((dma_addr >> 32) as u32)
            .s("_").hex(dma_addr as u32).s(" (stolen-system [").hex(mem.region_start as u32).s("-")
            .hex(mem.region_end as u32).s("])").say();
        return false;
    }

    Line::new().s("Using dma_addr=").hex((dma_addr >> 32) as u32).s("_").hex(dma_addr as u32)
        .s(", based on initial plane programming").say();

    plane_config.phys_base = dma_addr - mem.region_start;
    true
}

/// `i915_gem_object_create_region_at` + `_i915_gem_object_stolen_init` for
/// the stolen-system region: the range is reserved in `i915->mm.stolen`.
fn i915_gem_object_create_stolen_at(dsm: &mut i915_dsm, offset: u64, size: u64) -> Option<drm_mm_node> {
    if size % STOLEN_MIN_PAGE_SIZE != 0 || offset % STOLEN_MIN_PAGE_SIZE != 0 {
        linux::say("warning: initial fb not aligned to the region's page size");
        return None;
    }
    let region_size = dsm.region_end - dsm.region_start + 1;
    if offset > region_size || size > region_size - offset {
        return None;
    }
    // The io check (offset + size beyond the CPU-visible part) only refuses
    // when the GGTT has no aperture; TGL has one.
    if !dsm.mm.initialized() || size == 0 {
        return None;
    }
    Line::new().s("creating preallocated stolen object: stolen_offset=").hex(offset as u32)
        .s(", size=").hex(size as u32).say();
    let stolen = drm_mm_node { start: offset, size, color: 0 };
    dsm.mm.drm_mm_reserve_node(stolen).ok()?;
    Some(stolen)
}

/// `i915_vma_pin` with PIN_GLOBAL | PIN_OFFSET_FIXED | PIN_MAPPABLE
/// (`i915_vma_insert` with a fixed offset, then the GGTT bind:
/// `intel_ggtt_bind_vma` → `gen8_ggtt_insert_entries`).
fn i915_vma_pin_fixed(ggtt: &mut i915_ggtt, dsm: &i915_dsm, vma: &mut i915_vma, offset: u64) -> Result<(), i32> {
    const ENOSPC: i32 = 28;
    const EINVAL: i32 = 22;
    let size = vma.stolen.size;
    if offset % I915_GTT_MIN_ALIGNMENT != 0 {
        return Err(-EINVAL);
    }
    // PIN_MAPPABLE: the node must lie inside the aperture.
    if offset + size > ggtt.mappable_end {
        return Err(-ENOSPC);
    }
    // i915_gem_gtt_reserve; the GGTT has no cache colouring on LLC parts.
    let node = drm_mm_node { start: offset, size, color: 0 };
    ggtt.vm_mm.drm_mm_reserve_node(node)?;
    vma.node = node;
    // Stolen-system objects are neither read-only nor local memory.
    ggtt.gen8_ggtt_insert_entries(node.start, node.size, dsm.region_start + vma.stolen.start,
                                  vma.stolen.size, vma.pat_index, 0);
    Ok(())
}

/// `initial_plane_vma`.
fn initial_plane_vma(ggtt: &mut i915_ggtt, dsm: Option<&mut i915_dsm>,
                     plane_config: &mut intel_initial_plane_config) -> Option<i915_vma> {
    if plane_config.size == 0 {
        return None;
    }

    if !initial_plane_phys(ggtt, dsm.as_deref(), plane_config) {
        return None;
    }
    let dsm = dsm?;

    let phys_base = plane_config.phys_base;

    let base = plane_config.base as u64 & !(I915_GTT_MIN_ALIGNMENT - 1);
    let mut size = (plane_config.base as u64 + plane_config.size as u64).next_multiple_of(STOLEN_MIN_PAGE_SIZE);
    size -= base;

    // The "exceeds half of stolen" discard is under CONFIG_FRAMEBUFFER_CONSOLE,
    // which the build this port follows does not set.

    let Some(stolen) = i915_gem_object_create_stolen_at(dsm, phys_base, size) else {
        linux::say("Failed to preallocate initial FB in stolen-system");
        return None;
    };

    // Marked WT ahead of time where the platform has it (HAS_WT is
    // HAS_EDRAM; Tiger Lake has none), else uncached.
    let mut vma = i915_vma { stolen, pat_index: TGL_PAT_CACHE_NONE, ..Default::default() };

    let err_obj = |dsm: &mut i915_dsm, ggtt: &mut i915_ggtt, orig_mm: Option<drm_mm_node>| {
        if let Some(n) = orig_mm {
            ggtt.vm_mm.drm_mm_remove_node(&n);
        }
        dsm.mm.drm_mm_remove_node(&stolen);
        None
    };

    match plane_config.tiling {
        I915_TILING_NONE => {}
        I915_TILING_X | I915_TILING_Y => {
            // The object would carry pitch | tiling and need a fence
            // (`i915_vma_is_map_and_fenceable`); fence management is not
            // ported, so a tiled firmware framebuffer is not taken over.
            linux::say("tiled initial FB: fencing not ported, not taken over");
            return err_obj(dsm, ggtt, None);
        }
        t => {
            Line::new().s("MISSING_CASE tiling ").hex(t).say();
            return err_obj(dsm, ggtt, None);
        }
    }

    // Pinned low in the GGTT (MTL's GOP places it high, which collides with
    // the GuC's top reservation).
    let mut base: u64 = 0;
    let mut orig_mm = None;

    if base != plane_config.base as u64 {
        // The original and new locations must not overlap: the original
        // PTEs are still being scanned out.
        let n = drm_mm_node { start: plane_config.base as u64, size, color: I915_COLOR_UNEVICTABLE as u64 };
        if ggtt.vm_mm.drm_mm_reserve_node(n).is_err() {
            return err_obj(dsm, ggtt, None);
        }
        orig_mm = Some(n);
    }

    loop {
        if i915_vma_pin_fixed(ggtt, dsm, &mut vma, base).is_ok() {
            break;
        }
        if let Some(n) = orig_mm.take() {
            ggtt.vm_mm.drm_mm_remove_node(&n);
            // Try again, at the original location.
            base = plane_config.base as u64;
            continue;
        }
        return err_obj(dsm, ggtt, None);
    }

    if let Some(n) = orig_mm {
        ggtt.vm_mm.drm_mm_remove_node(&n);
    }

    Line::new().s("Initial plane fb bound to ").hex(vma.node.start as u32).s(" in the ggtt (original ")
        .hex(plane_config.base).s(")").say();

    Some(vma)
}

/// `intel_alloc_initial_plane_obj`.
fn intel_alloc_initial_plane_obj(ggtt: &mut i915_ggtt, dsm: Option<&mut i915_dsm>,
                                 plane_config: &mut intel_initial_plane_config) -> bool {
    let Some(fb) = plane_config.fb else {
        return false;
    };
    match fb.modifier {
        DRM_FORMAT_MOD_LINEAR | I915_FORMAT_MOD_X_TILED | I915_FORMAT_MOD_Y_TILED | I915_FORMAT_MOD_4_TILED => {}
        m => {
            Line::new().s("Unsupported modifier for initial FB: ").hex((m >> 32) as u32).s("_").hex(m as u32).say();
            return false;
        }
    }

    let Some(vma) = initial_plane_vma(ggtt, dsm, plane_config) else {
        return false;
    };

    plane_config.vma = Some(vma);
    true
}

/// `intel_find_initial_plane_obj`.
fn intel_find_initial_plane_obj(i915: &mut drm_i915_private, ggtt: &mut i915_ggtt, dsm: Option<&mut i915_dsm>,
                                c: usize, plane_configs: &mut [intel_initial_plane_config]) {
    let pipe = i915.display.crtcs[c].pipe;
    let Some(p) = crate::intel_crtc::primary_plane(i915, pipe) else {
        return;
    };

    if plane_configs[pipe as usize].fb.is_none() {
        return;
    }

    let (fb, vma) = if intel_alloc_initial_plane_obj(ggtt, dsm, &mut plane_configs[pipe as usize]) {
        (plane_configs[pipe as usize].fb.unwrap(), plane_configs[pipe as usize].vma.unwrap())
    } else if let Some(shared) = intel_reuse_initial_plane_obj(i915, pipe, plane_configs) {
        shared
    } else {
        // Linux disables the primary plane here (`intel_plane_disable_noatomic`),
        // which waits for vblanks; without the vblank interrupt the plane is
        // left scanning out what the firmware set up.
        Line::new().s("pipe ").dec(pipe as u64)
            .s(": initial FB not reconstructed, plane left as is (intel_plane_disable_noatomic needs vblank)").say();
        return;
    };

    let plane_config = plane_configs[pipe as usize];
    let ps = &mut i915.display.planes[p].state;
    ps.rotation = plane_config.rotation;
    // intel_fb_fill_view: 0/180 degrees and no DPT, so the normal view.

    ps.ggtt_vma = Some(vma);
    // intel_plane_uses_fence holds for the FBC plane, but i915_vma_pin_fence
    // does nothing for an untiled object, so PLANE_HAS_FENCE stays clear;
    // tiled objects are not taken over (initial_plane_vma).

    ps.surf = vma.node.start as u32;

    ps.src = [0, 0, fb.width << 16, fb.height << 16];
    ps.dst = [0, 0, fb.width as i32, fb.height as i32];

    if plane_config.tiling != 0 {
        i915.preserve_bios_swizzle = true;
    }

    ps.fb = Some(fb);
    ps.crtc = Some(pipe);
    // intel_plane_copy_uapi_to_hw_state: hw is uapi for these fields.
}

/// `skl_fixup_initial_plane_config`: the plane follows a framebuffer that
/// moved in the GGTT.
fn skl_fixup_initial_plane_config(i915: &mut drm_i915_private, pipe: u32,
                                  plane_config: &intel_initial_plane_config) -> bool {
    let Some(p) = crate::intel_crtc::primary_plane(i915, pipe) else {
        return false;
    };
    let plane_id = i915.display.planes[p].id;
    let ps = i915.display.planes[p].state;

    if !ps.visible {
        return false;
    }

    if plane_config.base == ps.surf {
        return false;
    }

    crate::intel_de::intel_de_write(i915, PLANE_SURF(pipe, plane_id), ps.surf);
    true
}

/// `intel_initial_plane_config`: per active crtc, read the plane, take
/// its framebuffer over and point the plane at it. The configs are
/// returned for the report; `plane_config_fini` drops references only.
pub fn intel_initial_plane_config(i915: &mut drm_i915_private, ggtt: &mut i915_ggtt,
                                  mut dsm: Option<&mut i915_dsm>)
                                  -> [intel_initial_plane_config; I915_MAX_PIPES as usize] {
    let mut plane_configs = [intel_initial_plane_config::default(); I915_MAX_PIPES as usize];

    for c in 0..I915_MAX_PIPES as usize {
        if !i915.display.crtc_valid[c] || !i915.display.crtcs[c].state.uapi_active {
            continue;
        }
        let pipe = i915.display.crtcs[c].pipe;

        // Reserving the firmware fb up front keeps other stolen allocations
        // (rings and the like) off it.
        crate::skl_universal_plane::skl_get_initial_plane_config(i915, c, &mut plane_configs[pipe as usize]);

        // A fb shared between several pipes is taken from the first one.
        intel_find_initial_plane_obj(i915, ggtt, dsm.as_deref_mut(), c, &mut plane_configs);

        let pc = plane_configs[pipe as usize];
        if skl_fixup_initial_plane_config(i915, pipe, &pc) {
            intel_plane_initial_vblank_wait(i915, pipe);
        }
    }
    plane_configs
}
