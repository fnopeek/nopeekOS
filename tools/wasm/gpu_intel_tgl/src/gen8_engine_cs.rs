//! gt/gen8_engine_cs.c and gt/gen8_engine_cs.h (Linux 6.18.55): the
//! commands a request puts into its ring on GRAPHICS_VER 12 below 12.55 —
//! flushes, the AUX table invalidation and the closing breadcrumb.
//!
//! A command block is built in a `Cs` and handed to the ring as one
//! `intel_ring_begin`/`intel_ring_advance` pair, with the dword count of
//! what was built.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::{intel_engine_cs, intel_gt};
use crate::regs::*;
use npk_sys::Dma;

/// A command block (`u32 *cs` between `intel_ring_begin` and `_advance`).
pub struct Cs {
    buf: [u32; 256],
    n: usize,
}

impl Cs {
    pub fn new() -> Self {
        Cs { buf: [0; 256], n: 0 }
    }

    pub fn push(&mut self, v: u32) {
        if self.n < self.buf.len() {
            self.buf[self.n] = v;
            self.n += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn as_slice(&self) -> &[u32] {
        &self.buf[..self.n]
    }

    /// Copies the block into a DMA buffer at `off`.
    pub fn write_to(&self, d: Dma, off: u32) {
        let mut b = [0u8; 1024];
        for (i, v) in self.as_slice().iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let _ = d.write(off, &b[..self.n * 4]);
    }
}

/// `__gen8_emit_pipe_control`.
pub fn gen12_emit_pipe_control(cs: &mut Cs, bit_group_0: u32, bit_group_1: u32, offset: u32) {
    cs.push(GFX_OP_PIPE_CONTROL(6) | bit_group_0);
    cs.push(bit_group_1);
    cs.push(offset);
    cs.push(0);
    cs.push(0);
    cs.push(0);
}

/// `gen8_emit_pipe_control`.
pub fn gen8_emit_pipe_control(cs: &mut Cs, bit_group_1: u32, offset: u32) {
    gen12_emit_pipe_control(cs, 0, bit_group_1, offset);
}

/// `__gen8_emit_write_rcs` via `gen12_emit_ggtt_write_rcs`.
fn gen12_emit_ggtt_write_rcs(cs: &mut Cs, value: u32, gtt_offset: u32, flags0: u32, flags1: u32) {
    cs.push(GFX_OP_PIPE_CONTROL(6) | flags0);
    cs.push(flags1 | PIPE_CONTROL_GLOBAL_GTT_IVB | PIPE_CONTROL_QW_WRITE);
    cs.push(gtt_offset);
    cs.push(0);
    cs.push(value);
    cs.push(0);
}

/// `__gen8_emit_flush_dw`.
fn __gen8_emit_flush_dw(cs: &mut Cs, value: u32, gtt_offset: u32, flags: u32) {
    cs.push((MI_FLUSH_DW + 1) | flags);
    cs.push(gtt_offset);
    cs.push(0);
    cs.push(value);
}

/// `gen8_emit_ggtt_write`.
fn gen8_emit_ggtt_write(cs: &mut Cs, value: u32, gtt_offset: u32, flags: u32) {
    __gen8_emit_flush_dw(cs, value, gtt_offset | MI_FLUSH_DW_USE_GTT, flags | MI_FLUSH_DW_OP_STOREDW);
}

/// `preparser_disable`.
fn preparser_disable(state: bool) -> u32 {
    MI_ARB_CHECK | 1 << 8 | state as u32
}

/// `gen12_get_aux_inv_reg`.
fn gen12_get_aux_inv_reg(engine: &intel_engine_cs) -> Option<u32> {
    match engine.id {
        RCS0 => Some(GEN12_CCS_AUX_INV),
        BCS0 => Some(GEN12_BCS0_AUX_INV),
        VCS0 => Some(GEN12_VD0_AUX_INV),
        VCS2 => Some(GEN12_VD2_AUX_INV),
        VECS0 => Some(GEN12_VE0_AUX_INV),
        CCS0 => Some(GEN12_CCS0_AUX_INV),
        _ => None,
    }
}

/// `gen12_needs_ccs_aux_inv`: TGL has no flat CCS.
fn gen12_needs_ccs_aux_inv(engine: &intel_engine_cs) -> bool {
    gen12_get_aux_inv_reg(engine).is_some() && TGL_HAS_FLAT_CCS == 0
}

/// `gen12_emit_aux_table_inv` (`gsi_offset` is 0 on the primary GT).
pub fn gen12_emit_aux_table_inv(engine: &intel_engine_cs, cs: &mut Cs) {
    if !gen12_needs_ccs_aux_inv(engine) {
        return;
    }
    let inv_reg = gen12_get_aux_inv_reg(engine).unwrap_or(0);

    cs.push(MI_LOAD_REGISTER_IMM(1) | MI_LRI_MMIO_REMAP_EN);
    cs.push(inv_reg);
    cs.push(AUX_INV);

    cs.push(MI_SEMAPHORE_WAIT_TOKEN | MI_SEMAPHORE_REGISTER_POLL | MI_SEMAPHORE_POLL | MI_SEMAPHORE_SAD_EQ_SDD);
    cs.push(0);
    cs.push(inv_reg);
    cs.push(0);
    cs.push(0);
}

/// `engine->emit_flush`: `gen12_emit_flush_rcs` for render,
/// `gen12_emit_flush_xcs` otherwise.
pub fn emit_flush(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize, mode: u64) -> Result<(), i32> {
    let engine = gt.engine[gt.requests[rq].engine as usize].ok_or(-19)?;
    if engine.flags & I915_ENGINE_HAS_RCS_REG_STATE != 0 {
        gen12_emit_flush_rcs(i915, gt, rq, &engine, mode)
    } else {
        gen12_emit_flush_xcs(i915, gt, rq, &engine, mode)
    }
}

/// `gen12_emit_flush_rcs`. The MTL/DG2 dummy pipe control does not apply;
/// TGL has the 3D pipeline, and this is the render engine.
fn gen12_emit_flush_rcs(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize, engine: &intel_engine_cs,
                        mode: u64) -> Result<(), i32> {
    if mode & EMIT_FLUSH != 0 || gen12_needs_ccs_aux_inv(engine) {
        let bit_group_0 = PIPE_CONTROL0_HDC_PIPELINE_FLUSH;
        let mut bit_group_1 = 0;

        // FLUSH_L3 below 12.70.
        if mode & EMIT_FLUSH != 0 {
            bit_group_1 |= PIPE_CONTROL_FLUSH_L3;
        }
        bit_group_1 |= PIPE_CONTROL_TILE_CACHE_FLUSH;
        bit_group_1 |= PIPE_CONTROL_RENDER_TARGET_CACHE_FLUSH;
        bit_group_1 |= PIPE_CONTROL_DEPTH_CACHE_FLUSH;
        // Wa_1409600907:tgl
        bit_group_1 |= PIPE_CONTROL_DEPTH_STALL;
        bit_group_1 |= PIPE_CONTROL_DC_FLUSH_ENABLE;
        bit_group_1 |= PIPE_CONTROL_FLUSH_ENABLE;
        bit_group_1 |= PIPE_CONTROL_STORE_DATA_INDEX;
        bit_group_1 |= PIPE_CONTROL_QW_WRITE;
        bit_group_1 |= PIPE_CONTROL_CS_STALL;

        let mut cs = Cs::new();
        gen12_emit_pipe_control(&mut cs, bit_group_0, bit_group_1, LRC_PPHWSP_SCRATCH_ADDR as u32);
        crate::i915_request::intel_ring_emit(i915, gt, rq, &cs)?;
    }

    if mode & EMIT_INVALIDATE != 0 {
        let mut flags = 0;
        flags |= PIPE_CONTROL_COMMAND_CACHE_INVALIDATE;
        flags |= PIPE_CONTROL_TLB_INVALIDATE;
        flags |= PIPE_CONTROL_INSTRUCTION_CACHE_INVALIDATE;
        flags |= PIPE_CONTROL_TEXTURE_CACHE_INVALIDATE;
        flags |= PIPE_CONTROL_VF_CACHE_INVALIDATE;
        flags |= PIPE_CONTROL_CONST_CACHE_INVALIDATE;
        flags |= PIPE_CONTROL_STATE_CACHE_INVALIDATE;
        flags |= PIPE_CONTROL_STORE_DATA_INDEX;
        flags |= PIPE_CONTROL_QW_WRITE;
        flags |= PIPE_CONTROL_CS_STALL;

        // The pre-parser must not run past the TLB invalidate.
        let mut cs = Cs::new();
        cs.push(preparser_disable(true));
        gen8_emit_pipe_control(&mut cs, flags, LRC_PPHWSP_SCRATCH_ADDR as u32);
        gen12_emit_aux_table_inv(engine, &mut cs);
        cs.push(preparser_disable(false));
        crate::i915_request::intel_ring_emit(i915, gt, rq, &cs)?;
    }
    Ok(())
}

/// `gen12_emit_flush_xcs`.
fn gen12_emit_flush_xcs(i915: &mut drm_i915_private, gt: &mut intel_gt, rq: usize, engine: &intel_engine_cs,
                        mode: u64) -> Result<(), i32> {
    let mut cs = Cs::new();
    if mode & EMIT_INVALIDATE != 0 {
        cs.push(preparser_disable(true));
    }

    // Always a command barrier: what follows (the breadcrumb interrupt) is
    // ordered after the write cache reached memory.
    let mut cmd = (MI_FLUSH_DW + 1) | MI_FLUSH_DW_STORE_INDEX | MI_FLUSH_DW_OP_STOREDW;
    if mode & EMIT_INVALIDATE != 0 {
        cmd |= MI_INVALIDATE_TLB;
        if engine.class == VIDEO_DECODE_CLASS {
            cmd |= MI_INVALIDATE_BSD;
        }
        if gen12_needs_ccs_aux_inv(engine) && engine.class == COPY_ENGINE_CLASS {
            cmd |= MI_FLUSH_DW_CCS;
        }
    }
    cs.push(cmd);
    cs.push(LRC_PPHWSP_SCRATCH_ADDR as u32);
    cs.push(0);
    cs.push(0);

    gen12_emit_aux_table_inv(engine, &mut cs);

    if mode & EMIT_INVALIDATE != 0 {
        cs.push(preparser_disable(false));
    }
    crate::i915_request::intel_ring_emit(i915, gt, rq, &cs)
}

/// `gen12_emit_preempt_busywait`: wait while the engine's HWS_PREEMPT
/// dword is set (`ring_set_paused`).
fn gen12_emit_preempt_busywait(cs: &mut Cs, preempt_address: u32) {
    cs.push(MI_ARB_CHECK);
    cs.push(MI_SEMAPHORE_WAIT_TOKEN | MI_SEMAPHORE_GLOBAL_GTT | MI_SEMAPHORE_POLL | MI_SEMAPHORE_SAD_EQ_SDD);
    cs.push(0);
    cs.push(preempt_address);
    cs.push(0);
    cs.push(0);
}

/// The closing breadcrumb and where the request's `tail` and `wa_tail`
/// fall in it (dword indices).
pub struct fini_breadcrumb {
    pub cs: Cs,
    pub tail_dw: usize,
}

/// `engine->emit_fini_breadcrumb`: `gen12_emit_fini_breadcrumb_rcs` or
/// `_xcs`, then `gen12_emit_fini_breadcrumb_tail` and `gen8_emit_wa_tail`.
/// No GuC; the hold-switchout workaround is DG2/MTL only.
pub fn emit_fini_breadcrumb(engine: &intel_engine_cs, seqno: u32, hwsp_offset: u32, preempt_address: u32)
                            -> fini_breadcrumb {
    let mut cs = Cs::new();
    if engine.flags & I915_ENGINE_HAS_RCS_REG_STATE != 0 {
        let mut flags = PIPE_CONTROL_CS_STALL
            | PIPE_CONTROL_TLB_INVALIDATE
            | PIPE_CONTROL_TILE_CACHE_FLUSH
            | PIPE_CONTROL_RENDER_TARGET_CACHE_FLUSH
            | PIPE_CONTROL_DEPTH_CACHE_FLUSH
            | PIPE_CONTROL_DC_FLUSH_ENABLE
            | PIPE_CONTROL_FLUSH_ENABLE;
        // Below 12.70: FLUSH_L3; GRAPHICS_VER 12 below 12.55: Wa_1409600907.
        flags |= PIPE_CONTROL_FLUSH_L3;
        flags |= PIPE_CONTROL_DEPTH_STALL;
        gen12_emit_pipe_control(&mut cs, PIPE_CONTROL0_HDC_PIPELINE_FLUSH, flags, 0);
        gen12_emit_ggtt_write_rcs(&mut cs, seqno, hwsp_offset, 0, PIPE_CONTROL_FLUSH_ENABLE | PIPE_CONTROL_CS_STALL);
    } else {
        // Stalling flush before the seqno write; the post-sync is not.
        __gen8_emit_flush_dw(&mut cs, 0, 0, 0);
        gen8_emit_ggtt_write(&mut cs, seqno, hwsp_offset, 0);
    }

    cs.push(MI_USER_INTERRUPT);
    cs.push(MI_ARB_ON_OFF | MI_ARB_ENABLE);
    if engine.flags & I915_ENGINE_HAS_SEMAPHORES != 0 {
        gen12_emit_preempt_busywait(&mut cs, preempt_address);
    }
    let tail_dw = cs.len();

    // gen8_emit_wa_tail: at least one preemption point per request.
    cs.push(MI_ARB_CHECK);
    cs.push(MI_NOOP);

    fini_breadcrumb { cs, tail_dw }
}
