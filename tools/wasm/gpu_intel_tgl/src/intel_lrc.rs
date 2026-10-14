//! gt/intel_lrc.c (Linux 6.18.55): the logical ring context image for
//! GRAPHICS_VER 12 below 12.55 — the register state page, the indirect and
//! per-context workaround batches, the descriptor, and allocating the
//! image with its ring and timeline.
//!
//! The image lives in a DMA buffer; the register state (`lrc_reg_state`)
//! is its second page and is read and written dword by dword.

use crate::i915_drv::drm_i915_private;
use crate::i915_vma::i915_gem_object_create_internal;
use crate::intel_gt::{intel_engine_cs, intel_gt};
use crate::intel_workarounds::{masked_bit_disable, masked_bit_enable};
use crate::regs::*;
use crate::tables;
use npk_sys::Dma;

const PAGE: u32 = 4096;

/// The image's register state page, as a CPU view.
struct lrc_regs {
    d: Dma,
}

impl lrc_regs {
    fn r(&self, i: u32) -> u32 {
        self.d.r32(LRC_STATE_OFFSET as u32 + i * 4)
    }
    fn w(&self, i: u32, v: u32) {
        self.d.w32(LRC_STATE_OFFSET as u32 + i * 4, v);
    }
}

fn regs_of(gt: &intel_gt, ce: usize) -> Option<lrc_regs> {
    gt.contexts[ce].state_obj.and_then(|o| o.map()).map(|d| lrc_regs { d })
}

/// `reg_offsets`: TGL is GRAPHICS_VER 12 below 12.55.
fn reg_offsets(engine: &intel_engine_cs) -> &'static [u8] {
    if engine.flags & I915_ENGINE_HAS_RCS_REG_STATE != 0 {
        &tables::gen12_rcs_offsets
    } else {
        &tables::gen12_xcs_offsets
    }
}

/// `set_offsets`: decode the per-platform byte table into LRI headers and
/// register offsets (relative to the engine's MMIO base); values untouched.
fn set_offsets(regs: &mut [u32; 1024], data: &[u8], base: u32, close: bool) {
    let mut d = 0usize;
    let mut r = 0usize;
    while d < data.len() && data[d] != 0 {
        if data[d] & 0x80 != 0 {
            r += (data[d] & 0x7f) as usize;
            d += 1;
            continue;
        }
        let mut count = data[d] & 0x3f;
        let flags = data[d] >> 6;
        d += 1;

        regs[r] = MI_LOAD_REGISTER_IMM(count as u32);
        if flags & 1 != 0 {
            regs[r] |= MI_LRI_FORCE_POSTED;
        }
        // GRAPHICS_VER >= 11
        regs[r] |= MI_LRI_LRM_CS_MMIO;
        r += 1;

        while count > 0 {
            let mut offset: u32 = 0;
            loop {
                let v = data[d];
                d += 1;
                offset <<= 7;
                offset |= (v & 0x7f) as u32;
                if v & 0x80 == 0 {
                    break;
                }
            }
            regs[r] = base + (offset << 2);
            r += 2;
            count -= 1;
        }
    }
    if close {
        // MI_BATCH_BUFFER_END, with bit 0 from GRAPHICS_VER 11.
        regs[r] = MI_BATCH_BUFFER_END | 1;
    }
}

fn lrc_ring_mi_mode() -> u32 {
    0x60
}
fn lrc_ring_bb_offset() -> u32 {
    0x70
}
fn lrc_ring_gpr0() -> u32 {
    0x74
}
fn lrc_ring_wa_bb_per_ctx() -> u32 {
    0x12
}
fn lrc_ring_indirect_ptr() -> u32 {
    lrc_ring_wa_bb_per_ctx() + 2
}
fn lrc_ring_indirect_offset() -> u32 {
    lrc_ring_indirect_ptr() + 2
}
/// `lrc_ring_cmd_buf_cctl`: render only on GRAPHICS_VER 12.
fn lrc_ring_cmd_buf_cctl(engine: &intel_engine_cs) -> Option<u32> {
    (engine.class == RENDER_CLASS).then_some(0xb6)
}

/// `init_common_regs`. No run-alone (PXP from 12.70 only).
fn init_common_regs(regs: &mut [u32; 1024], runtime_last: u32, inhibit: bool) {
    let mut ctl = masked_bit_enable(CTX_CTRL_INHIBIT_SYN_CTX_SWITCH);
    ctl |= masked_bit_disable(CTX_CTRL_ENGINE_CTX_RESTORE_INHIBIT);
    if inhibit {
        ctl |= CTX_CTRL_ENGINE_CTX_RESTORE_INHIBIT;
    }
    regs[CTX_CONTEXT_CONTROL as usize] = ctl;
    regs[CTX_TIMESTAMP as usize] = runtime_last;
    regs[lrc_ring_bb_offset() as usize + 1] = 0;
}

/// `init_ppgtt_regs` → `ASSIGN_CTX_PML4`: the 4-level table's top directory.
fn init_ppgtt_regs(regs: &mut [u32; 1024], pd_dma: u64) {
    regs[CTX_PDP0_UDW as usize + 1] = (pd_dma >> 32) as u32;
    regs[CTX_PDP0_LDW as usize + 1] = pd_dma as u32;
}

/// `__reset_stop_ring`.
fn __reset_stop_ring(regs: &mut [u32; 1024]) {
    let x = lrc_ring_mi_mode() as usize;
    regs[x + 1] &= !STOP_RING;
    regs[x + 1] |= STOP_RING << 16;
}

/// `__lrc_init_regs`. `init_wa_bb_regs` has nothing to set: the engine-wide
/// wa_ctx is only built below GRAPHICS_VER 11.
fn __lrc_init_regs(regs: &mut [u32; 1024], engine: &intel_engine_cs, runtime_last: u32, pd_dma: u64,
                   inhibit: bool) {
    if inhibit {
        regs.fill(0);
    }
    set_offsets(regs, reg_offsets(engine), engine.mmio_base, inhibit);
    init_common_regs(regs, runtime_last, inhibit);
    init_ppgtt_regs(regs, pd_dma);
    __reset_stop_ring(regs);
}

fn read_page(d: Dma, off: u32, page: &mut [u32; 1024]) {
    let mut b = [0u8; 4096];
    let _ = d.read(off, &mut b);
    for (i, w) in page.iter_mut().enumerate() {
        *w = u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
    }
}

fn write_page(d: Dma, off: u32, page: &[u32; 1024]) {
    let mut b = [0u8; 4096];
    for (i, w) in page.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    let _ = d.write(off, &b);
}

/// `lrc_init_regs`.
pub fn lrc_init_regs(gt: &intel_gt, ce: usize, inhibit: bool) {
    let Some(regs) = regs_of(gt, ce) else { return };
    let Some(engine) = gt.engine[gt.contexts[ce].engine as usize] else { return };
    let pd = gt.vm.map(|v| v.pd_dma()).unwrap_or(0);
    let mut page = [0u32; 1024];
    read_page(regs.d, LRC_STATE_OFFSET as u32, &mut page);
    __lrc_init_regs(&mut page, &engine, gt.contexts[ce].runtime_last, pd, inhibit);
    write_page(regs.d, LRC_STATE_OFFSET as u32, &page);
}

/// `lrc_reset_regs`.
pub fn lrc_reset_regs(gt: &intel_gt, ce: usize) {
    let Some(regs) = regs_of(gt, ce) else { return };
    let x = lrc_ring_mi_mode() + 1;
    let v = regs.r(x);
    regs.w(x, (v & !STOP_RING) | STOP_RING << 16);
}

/// `lrc_init_state`: the recorded default image if there is one (then the
/// context restores, `CONTEXT_VALID_BIT`), the per-process HWSP and the
/// workaround-batch page cleared, then the register state.
pub fn lrc_init_state(_i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize) {
    let Some(state) = gt.contexts[ce].state_obj.and_then(|o| o.map()) else { return };
    let Some(engine) = gt.engine[gt.contexts[ce].engine as usize] else { return };
    let mut inhibit = true;

    if let Some(def) = gt.contexts[ce].default_state.and_then(|o| o.map()) {
        let mut page = [0u32; 1024];
        let mut off = 0;
        while off < engine.context_size {
            read_page(def, off, &mut page);
            write_page(state, off, &page);
            off += PAGE;
        }
        gt.contexts[ce].valid = true;
        inhibit = false;
    }

    let zero = [0u32; 1024];
    write_page(state, 0, &zero);
    if gt.contexts[ce].wa_bb_page != 0 {
        write_page(state, gt.contexts[ce].wa_bb_page * PAGE, &zero);
    }

    let pd = gt.vm.map(|v| v.pd_dma()).unwrap_or(0);
    let mut page = [0u32; 1024];
    read_page(state, LRC_STATE_OFFSET as u32, &mut page);
    __lrc_init_regs(&mut page, &engine, gt.contexts[ce].runtime_last, pd, inhibit);
    write_page(state, LRC_STATE_OFFSET as u32, &page);
}

/// `lrc_indirect_bb`: the GGTT address of the workaround-batch page.
fn lrc_indirect_bb(gt: &intel_gt, ce: usize) -> u32 {
    gt.contexts[ce].state.map(|v| v.ggtt_offset()).unwrap_or(0) + gt.contexts[ce].wa_bb_page * PAGE
}

/// `gen12_emit_timestamp_wa`.
fn gen12_emit_timestamp_wa(cs: &mut crate::gen8_engine_cs::Cs, state_ggtt: u32) {
    cs.push(MI_LOAD_REGISTER_MEM_GEN8 | MI_SRM_LRM_GLOBAL_GTT | MI_LRI_LRM_CS_MMIO);
    cs.push(GEN8_RING_CS_GPR(0, 0));
    cs.push(state_ggtt + LRC_STATE_OFFSET as u32 + CTX_TIMESTAMP * 4);
    cs.push(0);

    cs.push(MI_LOAD_REGISTER_REG | MI_LRR_SOURCE_CS_MMIO | MI_LRI_LRM_CS_MMIO);
    cs.push(GEN8_RING_CS_GPR(0, 0));
    cs.push(RING_CTX_TIMESTAMP(0));

    cs.push(MI_LOAD_REGISTER_REG | MI_LRR_SOURCE_CS_MMIO | MI_LRI_LRM_CS_MMIO);
    cs.push(GEN8_RING_CS_GPR(0, 0));
    cs.push(RING_CTX_TIMESTAMP(0));
}

/// `gen12_emit_restore_scratch`.
fn gen12_emit_restore_scratch(cs: &mut crate::gen8_engine_cs::Cs, state_ggtt: u32) {
    cs.push(MI_LOAD_REGISTER_MEM_GEN8 | MI_SRM_LRM_GLOBAL_GTT | MI_LRI_LRM_CS_MMIO);
    cs.push(GEN8_RING_CS_GPR(0, 0));
    cs.push(state_ggtt + LRC_STATE_OFFSET as u32 + (lrc_ring_gpr0() + 1) * 4);
    cs.push(0);
}

/// `gen12_emit_cmd_buf_wa`.
fn gen12_emit_cmd_buf_wa(cs: &mut crate::gen8_engine_cs::Cs, state_ggtt: u32, cctl: u32) {
    cs.push(MI_LOAD_REGISTER_MEM_GEN8 | MI_SRM_LRM_GLOBAL_GTT | MI_LRI_LRM_CS_MMIO);
    cs.push(GEN8_RING_CS_GPR(0, 0));
    cs.push(state_ggtt + LRC_STATE_OFFSET as u32 + (cctl + 1) * 4);
    cs.push(0);

    cs.push(MI_LOAD_REGISTER_REG | MI_LRR_SOURCE_CS_MMIO | MI_LRI_LRM_CS_MMIO);
    cs.push(GEN8_RING_CS_GPR(0, 0));
    cs.push(RING_CMD_BUF_CCTL(0));
}

/// `gen12_invalidate_state_cache`.
fn gen12_invalidate_state_cache(cs: &mut crate::gen8_engine_cs::Cs) {
    cs.push(MI_LOAD_REGISTER_IMM(1));
    cs.push(GEN12_CS_DEBUG_MODE2);
    cs.push(masked_bit_enable(INSTRUCTION_STATE_CACHE_INVALIDATE));
}

/// `gen12_emit_indirect_ctx_rcs` / `_xcs` for GRAPHICS_VER 12.0: Wa_18022495364
/// (12.0–12.10) applies to render; the DG2/MTL workarounds do not.
fn gen12_emit_indirect_ctx(cs: &mut crate::gen8_engine_cs::Cs, engine: &intel_engine_cs, state_ggtt: u32) {
    gen12_emit_timestamp_wa(cs, state_ggtt);
    if engine.class == RENDER_CLASS {
        if let Some(cctl) = lrc_ring_cmd_buf_cctl(engine) {
            gen12_emit_cmd_buf_wa(cs, state_ggtt, cctl);
        }
    }
    gen12_emit_restore_scratch(cs, state_ggtt);
    crate::gen8_engine_cs::gen12_emit_aux_table_inv(engine, cs);
    if engine.class == RENDER_CLASS {
        gen12_invalidate_state_cache(cs);
    }
}

/// `setup_predicate_disable_wa`.
fn setup_predicate_disable_wa(cs: &mut crate::gen8_engine_cs::Cs, indirect_bb: u32) {
    cs.push(MI_STORE_DWORD_IMM_GEN4 | MI_USE_GGTT | (4 - 2));
    cs.push(indirect_bb + DG2_PREDICATE_RESULT_WA as u32);
    cs.push(0);
    cs.push(0);

    cs.push(MI_BATCH_BUFFER_END | 1 << 15);
    cs.push(MI_SET_PREDICATE | MI_SET_PREDICATE_DISABLE);

    cs.push(MI_STORE_DWORD_IMM_GEN4 | MI_USE_GGTT | (4 - 2));
    cs.push(indirect_bb + DG2_PREDICATE_RESULT_WA as u32);
    cs.push(0);
    cs.push(1);

    cs.push(MI_BATCH_BUFFER_END);
}

/// `lrc_update_regs`: ring registers, RPCS for render, then the indirect
/// and per-context workaround batches (`setup_indirect_ctx_bb`,
/// `setup_per_ctx_bb`). Returns the new `lrca` with CTX_DESC_FORCE_RESTORE.
/// No OA stream is open (`i915_oa_init_reg_state` writes nothing).
pub fn lrc_update_regs(_i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize, head: u32) -> u32 {
    let Some(regs) = regs_of(gt, ce) else { return 0 };
    let Some(engine) = gt.engine[gt.contexts[ce].engine as usize] else { return 0 };
    let ring = gt.contexts[ce].ring;

    regs.w(CTX_RING_START, ring.ggtt_offset());
    regs.w(CTX_RING_HEAD, head);
    regs.w(CTX_RING_TAIL, ring.tail);
    regs.w(CTX_RING_CTL, RING_CTL_SIZE(ring.size) | RING_VALID);

    if engine.class == RENDER_CLASS {
        regs.w(CTX_R_PWR_CLK_STATE, crate::intel_engine_cs::intel_sseu_make_rpcs(gt, &gt.contexts[ce].sseu));
    }

    if gt.contexts[ce].wa_bb_page != 0 {
        let state_ggtt = gt.contexts[ce].state.map(|v| v.ggtt_offset()).unwrap_or(0);
        let indirect_bb = lrc_indirect_bb(gt, ce);
        let wa_off = gt.contexts[ce].wa_bb_page * PAGE;

        // setup_indirect_ctx_bb
        let mut cs = crate::gen8_engine_cs::Cs::new();
        gen12_emit_indirect_ctx(&mut cs, &engine, state_ggtt);
        while (cs.len() * 4) % CACHELINE_BYTES as usize != 0 {
            cs.push(MI_NOOP);
        }
        let size = (cs.len() * 4) as u32;
        cs.write_to(regs.d, wa_off);
        let mut pred = crate::gen8_engine_cs::Cs::new();
        setup_predicate_disable_wa(&mut pred, indirect_bb);
        pred.write_to(regs.d, wa_off + DG2_PREDICATE_RESULT_BB);
        // lrc_setup_indirect_ctx
        regs.w(lrc_ring_indirect_ptr() + 1, indirect_bb | (size / CACHELINE_BYTES));
        regs.w(lrc_ring_indirect_offset() + 1, GEN12_CTX_RCS_INDIRECT_CTX_OFFSET_DEFAULT << 6);

        // setup_per_ctx_bb: xehp_emit_per_ctx_bb adds nothing on TGL.
        let mut per = crate::gen8_engine_cs::Cs::new();
        per.push(MI_BATCH_BUFFER_END);
        per.write_to(regs.d, wa_off + PAGE);
        // lrc_setup_bb_per_ctx
        regs.w(lrc_ring_wa_bb_per_ctx() + 1, (indirect_bb + PAGE) | PER_CTX_BB_FORCE as u32 | PER_CTX_BB_VALID as u32);
    }

    lrc_descriptor(gt, ce) | CTX_DESC_FORCE_RESTORE as u32
}

/// `lrc_descriptor`: a 64-bit (4-level) legacy context, valid and privileged.
fn lrc_descriptor(gt: &intel_gt, ce: usize) -> u32 {
    let mut desc = INTEL_LEGACY_64B_CONTEXT << GEN8_CTX_ADDRESSING_MODE_SHIFT;
    desc |= GEN8_CTX_VALID | GEN8_CTX_PRIVILEGE;
    gt.contexts[ce].state.map(|v| v.ggtt_offset()).unwrap_or(0) | desc
}

/// The value at register-state index `i` (`ce->lrc_reg_state[i]`).
pub fn lrc_reg(gt: &intel_gt, ce: usize, i: u32) -> u32 {
    regs_of(gt, ce).map(|r| r.r(i)).unwrap_or(0)
}

/// `ce->lrc_reg_state[i] = v`.
pub fn lrc_set_reg(gt: &intel_gt, ce: usize, i: u32, v: u32) {
    if let Some(r) = regs_of(gt, ce) {
        r.w(i, v);
    }
}

/// `__lrc_alloc_state`: the image rounded to pages, plus the two
/// workaround-batch pages from GRAPHICS_VER 12, in system memory.
fn __lrc_alloc_state(gt: &mut intel_gt, ce: usize, engine: &intel_engine_cs) -> Result<(), i32> {
    let mut context_size = engine.context_size.next_multiple_of(PAGE);
    gt.contexts[ce].wa_bb_page = context_size / PAGE;
    context_size += PAGE * 2;
    gt.contexts[ce].state_obj = Some(i915_gem_object_create_internal(context_size as u64)?);
    Ok(())
}

/// `lrc_alloc`: image, ring and timeline (the engine's status page for a
/// perma-pinned context, a page of its own otherwise).
pub fn lrc_alloc(gt: &mut intel_gt, ce: usize) -> Result<(), i32> {
    let engine_id = gt.contexts[ce].engine as usize;
    let Some(engine) = gt.engine[engine_id] else { return Err(-19) };

    gt.contexts[ce].default_state = engine.default_state;
    __lrc_alloc_state(gt, ce, &engine)?;
    gt.contexts[ce].ring = crate::intel_ring::intel_engine_create_ring(gt.contexts[ce].ring_size)?;

    gt.contexts[ce].timeline = match gt.contexts[ce].timeline_hwsp {
        Some(hwsp) => {
            let sp = engine.status_page.ok_or(-19)?;
            crate::intel_timeline::intel_timeline_create_from_engine(sp, hwsp)
        }
        None => crate::intel_timeline::intel_timeline_create()?,
    };
    Ok(())
}

/// `lrc_reset`: the ring restarts at its emit position, the image is
/// scrubbed back to an inhibited context.
pub fn lrc_reset(i915: &mut drm_i915_private, gt: &mut intel_gt, ce: usize) {
    let emit = gt.contexts[ce].ring.emit;
    gt.contexts[ce].ring.intel_ring_reset(emit);
    lrc_init_regs(gt, ce, true);
    let tail = gt.contexts[ce].ring.tail;
    gt.contexts[ce].lrca = lrc_update_regs(i915, gt, ce, tail);
}
