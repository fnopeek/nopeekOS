//! gt/intel_context.c (Linux 6.18.55): a logical ring context — its state
//! image, ring and timeline — and pinning it. The contexts live in a fixed
//! table in `intel_gt` and are referred to by index; reference counting
//! and the `i915_active` trackers are bookkeeping and not kept.

use crate::i915_drv::drm_i915_private;
use crate::i915_vma::{drm_i915_gem_object, i915_vma};
use crate::intel_ggtt::i915_ggtt;
use crate::intel_gt::intel_gt;
use crate::intel_ring::intel_ring;
use crate::intel_timeline::intel_timeline;

/// Contexts the table holds: a kernel context and a recording context per
/// engine, with room to spare.
pub const MAX_CONTEXTS: usize = 16;

const ENOSPC: i32 = 28;

/// `struct intel_sseu`.
#[derive(Clone, Copy, Default)]
pub struct intel_sseu {
    pub slice_mask: u8,
    pub subslice_mask: u8,
    pub min_eus_per_subslice: u8,
    pub max_eus_per_subslice: u8,
}

/// `struct intel_context`.
#[derive(Clone, Copy, Default)]
pub struct intel_context {
    pub used: bool,
    /// `ce->engine`, by engine id.
    pub engine: u32,
    pub ring_size: u32,
    pub ring: intel_ring,
    pub timeline: intel_timeline,
    /// `page_pack_bits(NULL, hwsp)`: a perma-pinned context's timeline sits
    /// at this offset of the engine's status page.
    pub timeline_hwsp: Option<u32>,
    /// `ce->state`: the context image object, and its binding once pinned.
    pub state_obj: Option<drm_i915_gem_object>,
    pub state: Option<i915_vma>,
    pub wa_bb_page: u32,
    /// CONTEXT_ALLOC_BIT, CONTEXT_INIT_BIT, CONTEXT_VALID_BIT,
    /// CONTEXT_BARRIER_BIT, CONTEXT_IS_PARKING.
    pub alloc: bool,
    pub init: bool,
    pub valid: bool,
    pub barrier: bool,
    pub parking: bool,
    pub pin_count: u32,
    pub active_count: u32,
    /// `intel_context_inflight_count`, and whether the context is on an engine.
    pub inflight: u32,
    pub inflight_engine: bool,
    /// `ce->lrc.lrca`, `ce->lrc.ccid`; `ce->lrc.desc` is the two together.
    pub lrca: u32,
    pub ccid: u32,
    /// `ce->default_state`: the engine's recorded image.
    pub default_state: Option<drm_i915_gem_object>,
    pub sseu: intel_sseu,
    /// `ce->stats.runtime.last`.
    pub runtime_last: u32,
    /// `ce->tag`.
    pub tag: u32,
    /// `ce->wakeref`: the GT wakeref taken on entering.
    pub wakeref: bool,
}

impl intel_context {
    /// `ce->lrc.desc`.
    pub fn desc(&self) -> u64 {
        (self.ccid as u64) << 32 | self.lrca as u64
    }

    pub fn set_desc(&mut self, desc: u64) {
        self.lrca = desc as u32;
        self.ccid = (desc >> 32) as u32;
    }

    /// `intel_context_is_pinned`.
    pub fn intel_context_is_pinned(&self) -> bool {
        self.pin_count > 0
    }
}

/// `intel_context_create` → `intel_context_init`: ring size 4K, the
/// engine's SSEU, the GT's address space (`gt->vm`).
pub fn intel_context_create(gt: &mut intel_gt, engine: u32) -> Result<usize, i32> {
    let i = gt.contexts.iter().position(|c| !c.used).ok_or(-ENOSPC)?;
    let sseu = gt.engine[engine as usize].map(|e| e.sseu).unwrap_or_default();
    gt.contexts[i] = intel_context { used: true, engine, ring_size: 4096, sseu, ..Default::default() };
    Ok(i)
}

/// `intel_context_alloc_state` → `ops->alloc` (`lrc_alloc`).
pub fn intel_context_alloc_state(gt: &mut intel_gt, ce: usize) -> Result<(), i32> {
    if !gt.contexts[ce].alloc {
        crate::intel_lrc::lrc_alloc(gt, ce)?;
        gt.contexts[ce].alloc = true;
    }
    Ok(())
}

/// `__intel_context_do_pin`: allocate, pin ring, timeline and state
/// (`intel_context_pre_pin`), initialise the image on first use
/// (`execlists_context_pre_pin`), then `lrc_pin` on the first pin.
pub fn intel_context_pin(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt,
                         ce: usize) -> Result<(), i32> {
    intel_context_alloc_state(gt, ce)?;

    // intel_context_pre_pin
    crate::intel_ring::intel_ring_pin(&mut gt.contexts[ce].ring, ggtt)?;
    crate::intel_timeline::intel_timeline_pin(&mut gt.contexts[ce].timeline, ggtt)?;
    if gt.contexts[ce].state.is_none() {
        // __context_pin_state: PIN_OFFSET_BIAS | PIN_HIGH.
        let obj = gt.contexts[ce].state_obj.ok_or(-12)?;
        let flags = crate::i915_vma::pin_flags { high: true, mappable: false, bias: ggtt.pin_bias };
        gt.contexts[ce].state = Some(crate::i915_vma::i915_ggtt_pin(ggtt, obj, flags)?);
    }

    // execlists_context_pre_pin → lrc_pre_pin, and the image on first use.
    if !gt.contexts[ce].init {
        gt.contexts[ce].init = true;
        crate::intel_lrc::lrc_init_state(i915, gt, ce);
    }

    if gt.contexts[ce].pin_count == 0 {
        // ops->pin = lrc_pin
        let tail = gt.contexts[ce].ring.tail;
        let lrca = crate::intel_lrc::lrc_update_regs(i915, gt, ce, tail);
        gt.contexts[ce].lrca = lrca;
    }
    gt.contexts[ce].pin_count += 1;
    Ok(())
}

/// `__intel_context_pin`: another pin of a pinned context.
pub fn __intel_context_pin(gt: &mut intel_gt, ce: usize) {
    gt.contexts[ce].pin_count += 1;
}

/// `intel_context_unpin`. Bindings stay; on the last unpin the image's
/// CPU map would go (`lrc_post_unpin`), which here is the DMA buffer.
pub fn intel_context_unpin(gt: &mut intel_gt, ce: usize) {
    let c = &mut gt.contexts[ce];
    if c.pin_count > 0 {
        c.pin_count -= 1;
    }
}

/// `intel_context_enter` → `intel_context_enter_engine`: the engine and GT
/// wakerefs and the timeline.
pub fn intel_context_enter(i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize) {
    gt.contexts[ce].active_count += 1;
    if gt.contexts[ce].active_count > 1 {
        return;
    }
    let engine = gt.contexts[ce].engine as usize;
    crate::intel_engine_pm::intel_engine_pm_get(i915, gt, engine);
    crate::intel_timeline::intel_timeline_enter(&mut gt.contexts[ce].timeline);
    crate::intel_gt_pm::intel_gt_pm_get(i915, gt);
    gt.contexts[ce].wakeref = true;
}

/// `intel_context_mark_active`.
pub fn intel_context_mark_active(gt: &mut intel_gt, ce: usize) {
    gt.contexts[ce].active_count += 1;
}

/// `intel_context_exit` → `intel_context_exit_engine`.
pub fn intel_context_exit(i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize) {
    if gt.contexts[ce].active_count == 0 {
        crate::linux::say("warning: context exit without enter");
        return;
    }
    gt.contexts[ce].active_count -= 1;
    if gt.contexts[ce].active_count > 0 {
        return;
    }
    if core::mem::replace(&mut gt.contexts[ce].wakeref, false) {
        crate::intel_gt_pm::intel_gt_pm_put(i915, gt);
    }
    crate::intel_timeline::intel_timeline_exit(&mut gt.contexts[ce].timeline);
    let engine = gt.contexts[ce].engine as usize;
    crate::intel_engine_pm::intel_engine_pm_put(i915, gt, engine);
}
