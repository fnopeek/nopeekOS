//! gt/intel_timeline.c (Linux 6.18.55): a context's seqno and the HWSP
//! slot the GPU writes it to. A kernel context's timeline lives in the
//! engine's status page; every other timeline gets a page of its own.

use crate::i915_vma::{drm_i915_gem_object, i915_gem_object_create_internal, i915_ggtt_pin, i915_vma, pin_flags};
use crate::intel_ggtt::i915_ggtt;
use crate::regs::*;

const TIMELINE_SEQNO_BYTES: u32 = 8;
/// Requests one timeline holds before they are retired.
pub const TIMELINE_MAX_REQUESTS: usize = 16;

/// `struct intel_timeline`.
#[derive(Clone, Copy, Default)]
pub struct intel_timeline {
    /// `tl->hwsp_ggtt->obj`: the HWSP page.
    pub hwsp_obj: Option<drm_i915_gem_object>,
    /// `tl->hwsp_ggtt` once pinned (for an engine's timeline, the status
    /// page's binding).
    pub hwsp_vma: Option<i915_vma>,
    /// The page is the engine's status page (`intel_timeline_create_from_engine`).
    pub engine_hwsp: bool,
    /// `tl->hwsp_offset`: the page offset before pinning, the GGTT address after.
    pub hwsp_offset: u32,
    /// `offset_in_page(tl->hwsp_seqno)`.
    pub hwsp_seqno: u32,
    pub seqno: u32,
    pub has_initial_breadcrumb: bool,
    pub pin_count: u32,
    pub active_count: u32,
    /// `tl->requests`, oldest first.
    pub requests: [u16; TIMELINE_MAX_REQUESTS],
    pub nrequests: usize,
    /// `tl->last_request`.
    pub last_request: Option<u16>,
}

impl intel_timeline {
    /// `*tl->hwsp_seqno`.
    pub fn read_hwsp_seqno(&self) -> u32 {
        self.hwsp_obj.and_then(|o| o.map()).map(|d| d.r32(self.hwsp_seqno)).unwrap_or(0)
    }

    /// `intel_timeline_reset_seqno`: the slot's second dword cleared, the
    /// first set to the timeline's seqno.
    pub fn intel_timeline_reset_seqno(&self) {
        if let Some(d) = self.hwsp_obj.and_then(|o| o.map()) {
            d.w32(self.hwsp_seqno + 4, 0);
            d.w32(self.hwsp_seqno, self.seqno);
        }
    }

    /// `list_add_tail(&rq->link, &tl->requests)`.
    pub fn add_request(&mut self, rq: u16) -> bool {
        if self.nrequests == TIMELINE_MAX_REQUESTS {
            return false;
        }
        self.requests[self.nrequests] = rq;
        self.nrequests += 1;
        true
    }

    /// The oldest request (`list_first_entry(&tl->requests)`).
    pub fn first_request(&self) -> Option<u16> {
        (self.nrequests > 0).then(|| self.requests[0])
    }

    /// `__list_del_entry` of the oldest request.
    pub fn pop_first(&mut self) {
        if self.nrequests > 0 {
            self.requests.copy_within(1..self.nrequests, 0);
            self.nrequests -= 1;
        }
    }
}

/// `intel_timeline_init` without a HWSP: `hwsp_alloc`, a cached internal page.
pub fn intel_timeline_create() -> Result<intel_timeline, i32> {
    Ok(intel_timeline {
        has_initial_breadcrumb: true,
        hwsp_obj: Some(i915_gem_object_create_internal(I915_GTT_PAGE_SIZE)?),
        ..Default::default()
    })
}

/// `intel_timeline_create_from_engine`: the slot at `offset` in the
/// engine's status page; the engine resets it on sanitize.
pub fn intel_timeline_create_from_engine(status_page: i915_vma, offset: u32) -> intel_timeline {
    intel_timeline {
        hwsp_obj: Some(status_page.obj),
        hwsp_vma: Some(status_page),
        engine_hwsp: true,
        hwsp_offset: offset,
        hwsp_seqno: offset,
        ..Default::default()
    }
}

/// `intel_timeline_pin`: `intel_timeline_pin_map` clears the slot, the page
/// is pinned PIN_HIGH, and `hwsp_offset` becomes the slot's GGTT address.
pub fn intel_timeline_pin(tl: &mut intel_timeline, ggtt: &mut i915_ggtt) -> Result<(), i32> {
    if tl.pin_count > 0 {
        tl.pin_count += 1;
        return Ok(());
    }
    let ofs = tl.hwsp_offset & (I915_GTT_PAGE_SIZE as u32 - 1);
    if let Some(d) = tl.hwsp_obj.and_then(|o| o.map()) {
        d.w32(ofs, 0);
        d.w32(ofs + 4, 0);
    }
    tl.hwsp_seqno = ofs;
    if tl.hwsp_vma.is_none() {
        let obj = tl.hwsp_obj.ok_or(-12)?;
        tl.hwsp_vma = Some(i915_ggtt_pin(ggtt, obj, pin_flags { high: true, ..Default::default() })?);
    }
    tl.hwsp_offset = tl.hwsp_vma.map(|v| v.ggtt_offset()).unwrap_or(0) + ofs;
    tl.pin_count = 1;
    Ok(())
}

/// `intel_timeline_unpin`.
pub fn intel_timeline_unpin(tl: &mut intel_timeline) {
    if tl.pin_count > 0 {
        tl.pin_count -= 1;
    }
}

/// `intel_timeline_enter`: the first entry resets the HWSP slot to the
/// timeline's seqno.
pub fn intel_timeline_enter(tl: &mut intel_timeline) {
    if tl.active_count > 0 {
        tl.active_count += 1;
        return;
    }
    tl.active_count = 1;
    tl.intel_timeline_reset_seqno();
}

/// `intel_timeline_exit`.
pub fn intel_timeline_exit(tl: &mut intel_timeline) {
    if tl.active_count > 0 {
        tl.active_count -= 1;
    }
}

/// `timeline_advance` / `intel_timeline_get_seqno`. A seqno wrap onto a fresh
/// HWSP cacheline (`__intel_timeline_get_seqno`) moves the slot within the page.
pub fn intel_timeline_get_seqno(tl: &mut intel_timeline) -> u32 {
    tl.seqno = tl.seqno.wrapping_add(1 + tl.has_initial_breadcrumb as u32);
    if tl.seqno == 0 && tl.has_initial_breadcrumb {
        let mut next_ofs = (tl.hwsp_offset + TIMELINE_SEQNO_BYTES) & (I915_GTT_PAGE_SIZE as u32 - 1);
        if TIMELINE_SEQNO_BYTES <= 32 && next_ofs & 32 != 0 {
            next_ofs = (next_ofs + 32) & (I915_GTT_PAGE_SIZE as u32 - 1);
        }
        tl.hwsp_offset = tl.hwsp_vma.map(|v| v.ggtt_offset()).unwrap_or(0) + next_ofs;
        tl.hwsp_seqno = next_ofs;
        tl.intel_timeline_reset_seqno();
    }
    tl.seqno
}
