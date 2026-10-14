//! gt/intel_engine_cs.c (Linux 6.18.55), the part after the MMIO probe:
//! the engines' common setup (status page, workaround lists, execlists
//! state), their kernel contexts, resume, and release; with the SSEU
//! helpers of gt/intel_sseu.c the contexts need.

use crate::i915_drv::drm_i915_private;
use crate::i915_vma::{i915_gem_object_create_internal, i915_ggtt_pin, pin_flags};
use crate::intel_context::intel_sseu;
use crate::intel_ggtt::i915_ggtt;
use crate::intel_gt::intel_gt;
use crate::linux::Line;
use crate::intel_workarounds::masked_bit_enable;
use crate::regs::*;

/// `engine->tlb_inv`.
#[derive(Clone, Copy, Default)]
pub struct intel_engine_tlb_inv {
    pub reg: u32,
    pub done: u32,
    pub request: u32,
}

/// `intel_sseu_from_device_info`.
pub fn intel_sseu_from_device_info(gt: &intel_gt) -> intel_sseu {
    let s = &gt.info.sseu;
    intel_sseu {
        slice_mask: s.slice_mask as u8,
        subslice_mask: s.subslice_mask[0] as u8,
        min_eus_per_subslice: s.max_eus_per_subslice as u8,
        max_eus_per_subslice: s.max_eus_per_subslice as u8,
    }
}

/// `intel_sseu_make_rpcs`: TGL power-gates whole slices only, so the
/// request is the slice count (no i915/perf stream pins another).
pub fn intel_sseu_make_rpcs(gt: &intel_gt, req: &intel_sseu) -> u32 {
    let sseu = &gt.info.sseu;
    let slices = req.slice_mask.count_ones();
    let mut rpcs = 0;
    if sseu.has_slice_pg {
        let val = (slices << GEN11_RPCS_S_CNT_SHIFT) & GEN11_RPCS_S_CNT_MASK;
        rpcs |= GEN8_RPCS_ENABLE as u32 | GEN8_RPCS_S_CNT_ENABLE | val;
    }
    rpcs
}

/// `intel_engine_init_tlb_invalidation`, the GRAPHICS_VER 12.0 table.
fn intel_engine_init_tlb_invalidation(class: u32, instance: u32) -> Result<intel_engine_tlb_inv, i32> {
    let reg = match class {
        RENDER_CLASS => GEN12_GFX_TLB_INV_CR,
        VIDEO_DECODE_CLASS => GEN12_VD_TLB_INV_CR,
        VIDEO_ENHANCEMENT_CLASS => GEN12_VE_TLB_INV_CR,
        COPY_ENGINE_CLASS => GEN12_BLT_TLB_INV_CR,
        COMPUTE_CLASS => GEN12_COMPCTX_TLB_INV_CR,
        _ => return Err(-34),
    };
    let val = 1u32 << instance;
    let request = if matches!(class, VIDEO_DECODE_CLASS | VIDEO_ENHANCEMENT_CLASS | COMPUTE_CLASS | OTHER_CLASS) {
        masked_bit_enable(val)
    } else {
        val
    };
    Ok(intel_engine_tlb_inv { reg, done: val, request })
}

/// `intel_engine_init_execlists`.
fn intel_engine_init_execlists(gt: &mut intel_gt, e: usize) {
    if let Some(x) = gt.engine[e].as_mut() {
        x.execlists.port_mask = 1;
        x.execlists.pending = [None; 3];
        x.execlists.inflight = [None; 3];
        x.execlists.active = 0;
    }
}

/// `init_status_page`: a cached internal page, pinned PIN_HIGH (an LLC
/// platform), cleared.
fn init_status_page(gt: &mut intel_gt, ggtt: &mut i915_ggtt, e: usize) -> Result<(), i32> {
    let obj = match i915_gem_object_create_internal(I915_GTT_PAGE_SIZE) {
        Ok(o) => o,
        Err(err) => {
            crate::linux::drm_err("Failed to allocate status page");
            return Err(err);
        }
    };
    let vma = i915_ggtt_pin(ggtt, obj, pin_flags { high: true, ..Default::default() })?;
    if let Some(d) = obj.map() {
        let _ = d.write(0, &[0u8; 4096]);
    }
    if let Some(x) = gt.engine[e].as_mut() {
        x.status_page = Some(vma);
    }
    Ok(())
}

/// `engine_setup_common`. Breadcrumbs, the scheduler engine and the retire
/// worker are software; there is no command parser from GRAPHICS_VER 9 on.
fn engine_setup_common(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt, e: usize)
                       -> Result<(), i32> {
    let (class, instance, base) = gt.engine[e].map(|x| (x.class, x.instance, x.mmio_base)).unwrap_or_default();
    let tlb = intel_engine_init_tlb_invalidation(class, instance)?;
    init_status_page(gt, ggtt, e)?;

    intel_engine_init_execlists(gt, e);
    let sseu = intel_sseu_from_device_info(gt);
    if let Some(x) = gt.engine[e].as_mut() {
        x.tlb_inv = tlb;
        x.sseu = sseu;
        x.sched = Default::default();
    }

    let mut wal = crate::intel_workarounds::i915_wa_list::default();
    crate::intel_workarounds::intel_engine_init_workarounds(i915, gt, class, instance, base, &mut wal);
    let mut white = crate::intel_workarounds::i915_wa_list::default();
    crate::intel_workarounds::intel_engine_init_whitelist(i915, gt, class, instance, base, &mut white);
    let mut ctx = crate::intel_workarounds::i915_wa_list::default();
    crate::intel_workarounds::intel_engine_init_ctx_wa(i915, gt, class, instance, base, &mut ctx);
    if let Some(x) = gt.engine[e].as_mut() {
        x.wa_list = wal;
        x.whitelist = white;
        x.ctx_wa_list = ctx;
        // GRAPHICS_VER 12: register offsets in the image are relative.
        x.flags |= I915_ENGINE_HAS_RELATIVE_MMIO;
    }
    Ok(())
}

/// `measure_breadcrumb_dw`: the closing breadcrumb's length.
fn measure_breadcrumb_dw(gt: &intel_gt, e: usize) -> u32 {
    let engine = gt.engine[e].unwrap_or_default();
    crate::gen8_engine_cs::emit_fini_breadcrumb(&engine, 0, 0, 0).cs.len() as u32
}

/// `intel_engine_create_pinned_context`: a barrier context on the GT's
/// address space with its timeline in the status page at `hwsp`, pinned
/// for good.
fn intel_engine_create_pinned_context(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt,
                                      e: usize, ring_size: u32, hwsp: u32) -> Result<usize, i32> {
    let ce = crate::intel_context::intel_context_create(gt, e as u32)?;
    gt.contexts[ce].barrier = true;
    gt.contexts[ce].timeline_hwsp = Some(hwsp);
    gt.contexts[ce].ring_size = ring_size;
    if let Err(err) = crate::intel_context::intel_context_pin(i915, gt, ggtt, ce) {
        gt.contexts[ce].used = false;
        return Err(err);
    }
    Ok(ce)
}

/// `engine_init_common`: the default submission, the kernel context (no
/// GGTT bind context: `i915_ggtt_require_binder` is MTL only), and the
/// breadcrumb length.
fn engine_init_common(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt, e: usize)
                      -> Result<(), i32> {
    // execlists_set_default_submission
    if let Some(x) = gt.engine[e].as_mut() {
        x.sched.submit_nop = false;
        x.sched.tasklet_nop = false;
    }

    let ce = intel_engine_create_pinned_context(i915, gt, ggtt, e, 4096, I915_GEM_HWS_SEQNO_ADDR as u32)?;
    let dw = measure_breadcrumb_dw(gt, e);
    if let Some(x) = gt.engine[e].as_mut() {
        x.emit_fini_breadcrumb_dw = dw;
        x.kernel_context = Some(ce as u16);
    }
    Ok(())
}

/// `intel_engines_init`: execlists submission (no GuC on TGL).
pub fn intel_engines_init(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt) -> Result<(), i32> {
    for e in 0..gt.engine.len() {
        if gt.engine[e].is_none() {
            continue;
        }
        engine_setup_common(i915, gt, ggtt, e)?;
        crate::intel_execlists_submission::intel_execlists_submission_setup(i915, gt, e);
        engine_init_common(i915, gt, ggtt, e)?;
    }
    gt.engines_initialized = true;
    crate::intel_gt_irq::gen11_gt_irq_init_engines(i915, gt);
    report(gt);
    Ok(())
}

fn report(gt: &intel_gt) {
    for e in 0..gt.engine.len() {
        let Some(x) = gt.engine[e] else { continue };
        let Some(kc) = x.kernel_context else { continue };
        let ce = &gt.contexts[kc as usize];
        Line::new().s(crate::intel_execlists_submission::engine_name(gt, e)).s(" @").hex(x.mmio_base)
            .s(": context image ").dec(x.context_size as u64).s(" bytes, status page GGTT ")
            .hex(x.status_page.map(|v| v.ggtt_offset()).unwrap_or(0))
            .s(", kernel context state GGTT ").hex(ce.state.map(|v| v.ggtt_offset()).unwrap_or(0))
            .s(", ring (system memory) GGTT ").hex(ce.ring.ggtt_offset())
            .s(", breadcrumb ").dec(x.emit_fini_breadcrumb_dw as u64).s(" dw, workarounds ")
            .dec(x.wa_list.count as u64).s("/").dec(x.whitelist.count as u64).s("/").dec(x.ctx_wa_list.count as u64)
            .say();
    }
}

/// `intel_engine_resume`: engine workarounds, the whitelist, then
/// `execlists_resume`.
pub fn intel_engine_resume(i915: &mut drm_i915_private, gt: &mut intel_gt, e: usize) -> Result<(), i32> {
    let Some(x) = gt.engine[e] else { return Ok(()) };
    crate::intel_workarounds::intel_engine_apply_workarounds(i915, gt, &x.wa_list);
    crate::intel_workarounds::intel_engine_apply_whitelist(i915, gt, x.mmio_base, &x.whitelist);
    crate::intel_execlists_submission::execlists_resume(i915, gt, e)
}

/// `intel_engines_release`: a full GT reset first, in case the engines
/// were left running, then `execlists_release` per engine: of
/// `intel_engine_cleanup_common` what reaches the hardware is
/// `cleanup_status_page` masking HWSP writes (HWSTAM). The status page,
/// contexts and rings stay where they are, as the kernel keeps a display
/// controller's DMA buffers.
pub fn intel_engines_release(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if crate::intel_gt_pm::intel_gt_pm_is_awake(gt) {
        crate::linux::say("warning: GT awake at engine release");
    }
    if !crate::intel_reset::intel_gt_gpu_reset_clobbers_display() {
        let _ = crate::intel_reset::intel_gt_reset_all_engines(i915, gt);
    }
    for e in 0..gt.engine.len() {
        let Some(x) = gt.engine[e] else { continue };
        if x.wakeref_count != 0 {
            Line::new().s("warning: ").s(crate::intel_execlists_submission::engine_name(gt, e))
                .s(" awake at release").say();
        }
        i915.uncore.intel_uncore_write(RING_HWSTAM(x.mmio_base), !0);
        if let Some(y) = gt.engine[e].as_mut() {
            y.kernel_context = None;
            y.sched.tasklet_disabled += 1;
        }
    }
    gt.engines_initialized = false;
}

/// `intel_engine_print_registers` for an execlists engine on TGL, without
/// the CSB walk and the request listing. `ENGINE_READ64` is read high,
/// low, high until the high half is stable, as `__intel_uncore_read64_2x32`
/// does.
pub fn intel_engine_print_registers(i915: &mut drm_i915_private, gt: &intel_gt, e: usize) {
    let Some(engine) = gt.engine[e] else { return };
    let b = engine.mmio_base;
    let u = &mut i915.uncore;
    let mut read64 = |lo: u32, hi: u32| {
        let mut old = u.intel_uncore_read(hi);
        loop {
            let l = u.intel_uncore_read(lo);
            let h = u.intel_uncore_read(hi);
            if h == old {
                return (h as u64) << 32 | l as u64;
            }
            old = h;
        }
    };
    let acthd = read64(RING_ACTHD(b), RING_ACTHD_UDW(b));
    let bbaddr = read64(RING_BBADDR(b), RING_BBADDR_UDW(b));
    let faddr = read64(RING_DMA_FADD(b), RING_DMA_FADD_UDW(b));
    let name = crate::intel_gt::intel_engine_class_repr(engine.class);
    let ctl = u.intel_uncore_read(RING_CTL(b));
    let mode = u.intel_uncore_read(RING_MI_MODE(b));
    Line::new().s(name).dec(engine.instance as u64).s(": EL_STAT ").hex(u.intel_uncore_read(RING_EXECLIST_STATUS_HI(b)))
        .s("_").hex(u.intel_uncore_read(RING_EXECLIST_STATUS_LO(b)))
        .s(", RING_START ").hex(u.intel_uncore_read(RING_START(b)))
        .s(", HEAD ").hex(u.intel_uncore_read(RING_HEAD(b)) & HEAD_ADDR)
        .s(", TAIL ").hex(u.intel_uncore_read(RING_TAIL(b)) & TAIL_ADDR)
        .s(", CTL ").hex(ctl).s(if ctl & (RING_WAIT | RING_WAIT_SEMAPHORE) != 0 { " [waiting]" } else { "" })
        .s(", MODE ").hex(mode).s(if mode & MODE_IDLE != 0 { " [idle]" } else { "" }).say();
    Line::new().s(name).dec(engine.instance as u64).s(": IMR ").hex(u.intel_uncore_read(RING_IMR(b)))
        .s(", ESR ").hex(u.intel_uncore_read(RING_ESR(b)))
        .s(", EMR ").hex(u.intel_uncore_read(RING_EMR(b)))
        .s(", EIR ").hex(u.intel_uncore_read(RING_EIR(b)))
        .s(", IPEIR ").hex(u.intel_uncore_read(RING_IPEIR(b)))
        .s(", IPEHR ").hex(u.intel_uncore_read(RING_IPEHR(b)))
        .s(", INSTDONE ").hex(u.intel_uncore_read(RING_INSTDONE(b))).say();
    Line::new().s(name).dec(engine.instance as u64).s(": ACTHD ").hex((acthd >> 32) as u32).s("_").hex(acthd as u32)
        .s(", BBADDR ").hex((bbaddr >> 32) as u32).s("_").hex(bbaddr as u32)
        .s(", DMA_FADDR ").hex((faddr >> 32) as u32).s("_").hex(faddr as u32).say();
}
