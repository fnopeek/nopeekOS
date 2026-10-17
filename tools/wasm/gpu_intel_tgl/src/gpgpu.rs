//! The GPGPU pipeline of the render engine: our own compute programs,
//! compiled at build time by Mesa's Intel compiler (gpgpu_gen.rs,
//! shaders/gen_shaders.py), dispatched from an unprivileged batch in the
//! kernel context's PPGTT, the way a userspace execbuf runs.
//!
//! Not a Linux file: i915 dispatches no compute of its own on TGL. The
//! batch is what anv emits for an internal compute kernel on Gen12.0 —
//! `genX(emit_simple_shader_dispatch)` (MEDIA_VFE_STATE, interface
//! descriptor, CURBE, GPGPU_WALKER) with the state an anv command buffer
//! sets up around it (`flush_pipeline_select`,
//! `cmd_buffer_emit_state_base_address`, `emit_pipe_control`), since a batch
//! here starts from an unknown pipeline as every anv command buffer does.
//! Command layouts, MOCS, the thread count and the workaround flags come
//! from Mesa's genxml and device tables through the generator.
//!
//! One heap holds everything a dispatch reads: the batch, the dynamic state
//! (an interface descriptor and CURBE per dispatch), a zeroed surface-state
//! page for the surface and bindless bases, and the programs. A batch holds
//! any number of dispatches, each waiting for the one before (the scene
//! executor in compose.rs runs one batch per scene).

use crate::gen8_engine_cs::Cs;
use crate::gpgpu_gen::{self as g, Field, Kernel};
use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::linux::{self, Line};
use crate::regs::*;
use npk_sys::Dma;

/// The programs, in the order `kernel_off` holds them.
pub const KERNELS: [&Kernel; 4] = [&g::FILL_PATTERN, &g::COMPOSE, &g::BLUR_DOWN, &g::BLUR_BOX];
pub const K_FILL_PATTERN: usize = 0;
pub const K_COMPOSE: usize = 1;
pub const K_BLUR_DOWN: usize = 2;
pub const K_BLUR_BOX: usize = 3;

const PAGE: u32 = 4096;
const BATCH_PAGES: u32 = 16;
const DYN_PAGES: u32 = 16;
const BATCH_OFF: u32 = 0;
const DYN_OFF: u32 = BATCH_PAGES * PAGE;
const SURF_OFF: u32 = DYN_OFF + DYN_PAGES * PAGE;
/// GPU timestamps: one before the first dispatch, one after each.
const TS_OFF: u32 = SURF_OFF + PAGE;
const INSTR_OFF: u32 = TS_OFF + PAGE;
/// Dynamic state per dispatch: the interface descriptor, then at
/// `CURBE_OFF` the CURBE data.
const DISPATCH_DYN: u32 = 256;
const CURBE_OFF: u32 = 64;
/// Dispatches one batch can hold.
pub const MAX_DISPATCHES: u32 = DYN_PAGES * PAGE / DISPATCH_DYN;
/// RING_TIMESTAMP is 36 bits wide on Gen12.
const TIMESTAMP_MASK: u64 = (1 << 36) - 1;
/// Room the batch end needs (closing PIPE_CONTROL, end, padding).
const BATCH_TAIL: u32 = 16 * 4;
const SIMD: u32 = 16;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
const ETIME: i32 = 62;
const E2BIG: i32 = 7;
const DISPATCH_TIMEOUT_US: u64 = 80_000;

/// `_3D` and `GPGPU` of PIPELINE_SELECT's Pipeline Selection.
const PIPELINE_3D: u64 = 0;
const PIPELINE_GPGPU: u64 = 2;

pub struct gpgpu {
    heap: Dma,
    va: u64,
    instr_pages: u32,
    kernel_off: [u32; KERNELS.len()],
    /// The render engine's index in `gt.engine`.
    e: usize,
}

/// A batch being written into the heap.
pub struct batch {
    /// Bytes of commands written.
    off: u32,
    dispatches: u32,
    subslices: u32,
}

/// Writes `v` into field `f` of a command. Values of address and offset
/// fields keep their position (low bits are the alignment); the rest are
/// shifted to the field's start, as the genxml packers do.
fn pack(dw: &mut [u32], f: Field, v: u64) {
    let d0 = (f.start / 32) as usize;
    let lo = f.start - d0 as u32 * 32;
    let hi = f.end - d0 as u32 * 32;
    let mask: u128 = ((1u128 << (hi + 1)) - 1) & !((1u128 << lo) - 1);
    let val: u128 = if f.in_place { (v as u128) & mask } else { ((v as u128) << lo) & mask };
    for i in 0..4 {
        if d0 + i < dw.len() && (mask >> (32 * i)) as u32 != 0 {
            dw[d0 + i] = (dw[d0 + i] & !((mask >> (32 * i)) as u32)) | (val >> (32 * i)) as u32;
        }
    }
}

/// One command: its genxml defaults (opcode, length), then `fields`.
fn emit(cs: &mut Cs, length: usize, defaults: &[(Field, u64)], fields: &[(Field, u64)]) {
    let mut dw = [0u32; 32];
    for &(f, v) in defaults.iter().chain(fields) {
        pack(&mut dw, f, v);
    }
    for v in &dw[..length] {
        cs.push(*v);
    }
}

/// `emit_pipe_control` with no post-sync operation, on Gen12.0: a depth
/// flush takes a depth stall (Wa_1409600907), a texture invalidate in GPGPU
/// mode takes a CS stall.
fn pipe_control(cs: &mut Cs, gpgpu_mode: bool, bits: &[Field]) {
    use g::PIPE_CONTROL as P;
    let mut f = [(P::CommandStreamerStallEnable, 0u64); 12];
    let mut n = 0;
    let has = |x: Field| bits.iter().any(|b| b.start == x.start);
    for &b in bits {
        f[n] = (b, 1);
        n += 1;
    }
    if gpgpu_mode && has(P::TextureCacheInvalidationEnable) && !has(P::CommandStreamerStallEnable) {
        f[n] = (P::CommandStreamerStallEnable, 1);
        n += 1;
    }
    if g::WA_1409600907 && has(P::DepthCacheFlushEnable) && !has(P::DepthStallEnable) {
        f[n] = (P::DepthStallEnable, 1);
        n += 1;
    }
    emit(cs, P::LENGTH, P::DEFAULTS, &f[..n]);
}

/// A stalling PIPE_CONTROL that writes the GPU timestamp to `addr` once
/// everything before it is done (anv's `emit_query_pc_availability` with
/// `WriteTimestamp`).
fn pipe_control_timestamp(cs: &mut Cs, addr: u64) {
    use g::PIPE_CONTROL as P;
    emit(cs, P::LENGTH, P::DEFAULTS, &[
        (P::CommandStreamerStallEnable, 1),
        (P::PostSyncOperation, 3),
        (P::DestinationAddressType, 0),
        (P::Address, addr),
    ]);
}

/// `emit_pipeline_select`: Gen12 masks, the media sampler DOP clock gate on.
fn pipeline_select(cs: &mut Cs, pipeline: u64) {
    use g::PIPELINE_SELECT as S;
    emit(cs, S::LENGTH, S::DEFAULTS, &[
        (S::MaskBits, 0x13),
        (S::MediaSamplerDOPClockGateEnable, 1),
        (S::PipelineSelection, pipeline),
    ]);
}

/// `flush_pipeline_select` from `from` (None: unknown, the start of a
/// batch) to `to`, Gen12.0: CS stall and HDC flush, plus render target and
/// depth flushes leaving 3D or an unknown pipeline.
fn flush_pipeline_select(cs: &mut Cs, from: Option<u64>, to: u64) {
    use g::PIPE_CONTROL as P;
    let gpgpu_mode = from == Some(PIPELINE_GPGPU);
    if from == Some(PIPELINE_GPGPU) {
        pipe_control(cs, gpgpu_mode, &[P::CommandStreamerStallEnable, P::HDCPipelineFlushEnable]);
    } else {
        pipe_control(cs, gpgpu_mode, &[P::CommandStreamerStallEnable, P::HDCPipelineFlushEnable,
                                       P::RenderTargetCacheFlushEnable, P::DepthCacheFlushEnable]);
    }
    pipeline_select(cs, to);
}

impl gpgpu {
    fn addr(&self, off: u32) -> u64 {
        self.va + off as u64
    }

    /// `cmd_buffer_emit_state_base_address` (`fill_state_base_addr`) for
    /// this heap: dynamic state and instructions in it, general and
    /// indirect state over the whole address space, the surface and
    /// bindless surface bases on the zeroed page with one state. Returns
    /// the pipeline the batch is in afterwards.
    fn state_base_address(&self, cs: &mut Cs) -> u64 {
        use g::PIPE_CONTROL as P;
        use g::STATE_BASE_ADDRESS as B;
        let mocs = g::MOCS_INTERNAL as u64;
        // Wa_1607854226: non-pipelined state does not apply in GPGPU mode,
        // so STATE_BASE_ADDRESS is programmed in 3D mode.
        let mut current = None;
        if g::WA_1607854226 {
            flush_pipeline_select(cs, current, PIPELINE_3D);
            current = Some(PIPELINE_3D);
        }
        pipe_control(cs, current == Some(PIPELINE_GPGPU), &[P::HDCPipelineFlushEnable,
            P::RenderTargetCacheFlushEnable, P::CommandStreamerStallEnable]);
        emit(cs, B::LENGTH, B::DEFAULTS, &[
            (B::GeneralStateBaseAddress, 0),
            (B::GeneralStateMOCS, mocs),
            (B::GeneralStateBufferSize, 0xfffff),
            (B::GeneralStateBaseAddressModifyEnable, 1),
            (B::GeneralStateBufferSizeModifyEnable, 1),
            (B::StatelessDataPortAccessMOCS, mocs),
            (B::SurfaceStateBaseAddress, self.addr(SURF_OFF)),
            (B::SurfaceStateMOCS, mocs),
            (B::SurfaceStateBaseAddressModifyEnable, 1),
            (B::IndirectObjectBaseAddress, 0),
            (B::IndirectObjectMOCS, mocs),
            (B::IndirectObjectBufferSize, 0xfffff),
            (B::IndirectObjectBaseAddressModifyEnable, 1),
            (B::IndirectObjectBufferSizeModifyEnable, 1),
            (B::InstructionBaseAddress, self.addr(INSTR_OFF)),
            (B::InstructionMOCS, mocs),
            (B::InstructionBufferSize, self.instr_pages as u64),
            (B::InstructionBaseAddressModifyEnable, 1),
            (B::InstructionBuffersizeModifyEnable, 1),
            (B::BindlessSamplerStateBaseAddress, 0),
            (B::BindlessSamplerStateBufferSize, 0),
            (B::BindlessSamplerStateMOCS, mocs),
            (B::BindlessSamplerStateBaseAddressModifyEnable, 1),
            (B::DynamicStateBaseAddress, self.addr(DYN_OFF)),
            (B::DynamicStateBufferSize, DYN_PAGES as u64),
            (B::DynamicStateMOCS, mocs),
            (B::DynamicStateBaseAddressModifyEnable, 1),
            (B::DynamicStateBufferSizeModifyEnable, 1),
            (B::BindlessSurfaceStateBaseAddress, self.addr(SURF_OFF)),
            (B::BindlessSurfaceStateSize, 0),
            (B::BindlessSurfaceStateMOCS, mocs),
            (B::BindlessSurfaceStateBaseAddressModifyEnable, 1),
        ]);
        // anv leaves a pipeline that was unknown before the workaround in
        // 3D; the dispatch selects GPGPU itself.
        flush_pipeline_select(cs, current, PIPELINE_GPGPU);
        let inv = [P::TextureCacheInvalidationEnable, P::ConstantCacheInvalidationEnable,
                   P::StateCacheInvalidationEnable, P::InstructionCacheInvalidateEnable];
        let n = if g::WA_16013000631 { 4 } else { 3 };
        pipe_control(cs, true, &inv[..n]);
        PIPELINE_GPGPU
    }

    /// Appends `cs` to the batch.
    fn put(&self, b: &mut batch, cs: &Cs) -> Result<(), i32> {
        let bytes = cs.len() as u32 * 4;
        if b.off + bytes + BATCH_TAIL > BATCH_PAGES * PAGE {
            return Err(-E2BIG);
        }
        cs.write_to(self.heap, BATCH_OFF + b.off);
        b.off += bytes;
        Ok(())
    }

    /// Starts a batch: state base addresses and the VFE state, sized for
    /// the largest CURBE of the programs.
    pub fn begin(&self, gt: &intel_gt) -> Result<batch, i32> {
        use g::PIPE_CONTROL as P;
        let subslices = gt.info.sseu.subslice_mask[0].count_ones();
        let mut b = batch { off: 0, dispatches: 0, subslices };
        let mut cs = Cs::new();
        let current = self.state_base_address(&mut cs);
        // "A stalling PIPE_CONTROL is required before MEDIA_VFE_STATE".
        pipe_control(&mut cs, current == PIPELINE_GPGPU, &[P::CommandStreamerStallEnable]);
        let curbe_regs = KERNELS.iter().map(|k| k.cross_thread_regs).max().unwrap_or(0);
        {
            use g::MEDIA_VFE_STATE as V;
            emit(&mut cs, V::LENGTH, V::DEFAULTS, &[
                (V::StackSize, 0),
                (V::MaximumNumberofThreads, (g::MAX_CS_THREADS * subslices - 1) as u64),
                (V::NumberofURBEntries, 2),
                (V::URBEntryAllocationSize, 2),
                (V::CURBEAllocationSize, (curbe_regs as u64).next_multiple_of(2)),
            ]);
        }
        pipe_control_timestamp(&mut cs, self.addr(TS_OFF));
        // A dispatch that never ends keeps a zero after it.
        if !self.heap.write(TS_OFF, &[0u8; 8 * (MAX_DISPATCHES as usize + 1)]) {
            return Err(-EIO);
        }
        self.put(&mut b, &cs)?;
        Ok(b)
    }

    /// The GPU time of each dispatch of the batch that ran last, in GT
    /// clock ticks, into `out` (`u64::MAX`: not finished); the number
    /// written.
    pub fn dispatch_ticks(&self, b: &batch, out: &mut [u64]) -> usize {
        let n = (b.dispatches as usize).min(out.len());
        let mut buf = [0u8; 8 * (MAX_DISPATCHES as usize + 1)];
        if !self.heap.read(TS_OFF, &mut buf[..(n + 1) * 8]) {
            return 0;
        }
        let ts = |i: usize| u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap());
        for (i, o) in out.iter_mut().take(n).enumerate() {
            *o = if ts(i + 1) == 0 { u64::MAX } else { ts(i + 1).wrapping_sub(ts(i)) & TIMESTAMP_MASK };
        }
        n
    }

    /// One dispatch of `k` over `groups` workgroups with `push` as its push
    /// constants; it starts after the dispatch before it has written
    /// everything (CS stall, HDC flush).
    pub fn dispatch(&self, b: &mut batch, k: usize, push: &[u8], groups: [u32; 3]) -> Result<(), i32> {
        use g::PIPE_CONTROL as P;
        let kernel = KERNELS[k];
        let threads = (kernel.local_size[0] * kernel.local_size[1] * kernel.local_size[2]).div_ceil(SIMD);
        if threads != 1 || push.len() > kernel.push_size as usize || kernel.push_size > DISPATCH_DYN - CURBE_OFF {
            return Err(-EINVAL);
        }
        if b.dispatches == MAX_DISPATCHES {
            return Err(-E2BIG);
        }
        if groups.iter().any(|&n| n == 0) {
            return Ok(());
        }
        let slot = b.dispatches * DISPATCH_DYN;

        // Interface descriptor and CURBE (`genX(emit_simple_shader_dispatch)`).
        let mut idd = [0u32; 8];
        {
            use g::INTERFACE_DESCRIPTOR_DATA as I;
            for &(f, v) in &[
                (I::KernelStartPointer, (self.kernel_off[k] + kernel.simd16_offset) as u64),
                (I::SamplerCount, 0),
                (I::BindingTableEntryCount, 0),
                (I::BarrierEnable, kernel.uses_barrier as u64),
                (I::SharedLocalMemorySize, 0),
                (I::ConstantURBEntryReadOffset, 0),
                (I::ConstantURBEntryReadLength, 0),
                (I::CrossThreadConstantDataReadLength, kernel.cross_thread_regs as u64),
                (I::ThreadPreemptionDisable, 1),
                (I::NumberofThreadsinGPGPUThreadGroup, threads as u64),
            ] {
                pack(&mut idd, f, v);
            }
        }
        let mut dyn_slot = [0u8; DISPATCH_DYN as usize];
        for (i, v) in idd.iter().enumerate() {
            dyn_slot[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        dyn_slot[CURBE_OFF as usize..CURBE_OFF as usize + push.len()].copy_from_slice(push);
        let curbe_len = (kernel.push_size as u32).next_multiple_of(64);
        if !self.heap.write(DYN_OFF + slot, &dyn_slot) {
            return Err(-EIO);
        }

        let mut cs = Cs::new();
        if b.dispatches > 0 {
            pipe_control(&mut cs, true, &[P::CommandStreamerStallEnable, P::HDCPipelineFlushEnable]);
        }
        {
            use g::MEDIA_INTERFACE_DESCRIPTOR_LOAD as L;
            emit(&mut cs, L::LENGTH, L::DEFAULTS, &[
                (L::InterfaceDescriptorTotalLength, (g::INTERFACE_DESCRIPTOR_DATA::LENGTH * 4) as u64),
                (L::InterfaceDescriptorDataStartAddress, slot as u64),
            ]);
        }
        {
            use g::MEDIA_CURBE_LOAD as C;
            emit(&mut cs, C::LENGTH, C::DEFAULTS, &[
                (C::CURBEDataStartAddress, (slot + CURBE_OFF) as u64),
                (C::CURBETotalDataLength, curbe_len as u64),
            ]);
        }
        {
            use g::GPGPU_WALKER as W;
            let lanes = kernel.local_size[0] * kernel.local_size[1] * kernel.local_size[2];
            let right = if lanes % SIMD == 0 { (1u64 << SIMD) - 1 } else { (1u64 << (lanes % SIMD)) - 1 };
            emit(&mut cs, W::LENGTH, W::DEFAULTS, &[
                (W::SIMDSize, (SIMD / 16) as u64),
                (W::ThreadDepthCounterMaximum, 0),
                (W::ThreadHeightCounterMaximum, 0),
                (W::ThreadWidthCounterMaximum, (threads - 1) as u64),
                (W::ThreadGroupIDXDimension, groups[0] as u64),
                (W::ThreadGroupIDYDimension, groups[1] as u64),
                (W::ThreadGroupIDZDimension, groups[2] as u64),
                (W::RightExecutionMask, right),
                (W::BottomExecutionMask, 0xffff_ffff),
            ]);
        }
        {
            use g::MEDIA_STATE_FLUSH as F;
            emit(&mut cs, F::LENGTH, F::DEFAULTS, &[]);
        }
        pipe_control_timestamp(&mut cs, self.addr(TS_OFF + 8 * (b.dispatches + 1)));
        self.put(b, &cs)?;
        b.dispatches += 1;
        Ok(())
    }

    /// Ends the batch: the programs' writes reach memory before the request
    /// completes.
    fn end(&self, b: &mut batch) -> Result<(), i32> {
        use g::PIPE_CONTROL as P;
        let mut cs = Cs::new();
        pipe_control(&mut cs, true, &[P::CommandStreamerStallEnable, P::HDCPipelineFlushEnable]);
        {
            use g::MI_BATCH_BUFFER_END as E;
            emit(&mut cs, E::LENGTH, E::DEFAULTS, &[]);
        }
        // `genX(emit_simple_shader_end)`: the batch ends on a qword.
        if (b.off / 4 + cs.len() as u32) % 2 != 0 {
            cs.push(MI_NOOP);
        }
        if b.off + cs.len() as u32 * 4 > BATCH_PAGES * PAGE {
            return Err(-E2BIG);
        }
        cs.write_to(self.heap, BATCH_OFF + b.off);
        b.off += cs.len() as u32 * 4;
        Ok(())
    }
}

/// The heap with the programs, bound into the kernel PPGTT; None when the
/// GT has no render engine or the heap cannot be had.
pub fn gpgpu_init(gt: &mut intel_gt) -> Option<gpgpu> {
    let e = gt.engine.iter().position(|x| x.is_some_and(|x| x.class == RENDER_CLASS && x.instance == 0))?;
    let mut kernel_off = [0u32; KERNELS.len()];
    let mut size = 0u32;
    for (i, k) in KERNELS.iter().enumerate() {
        kernel_off[i] = size;
        size += ((k.code.len() * 4) as u32).next_multiple_of(64);
    }
    let instr_pages = size.div_ceil(PAGE);
    let pages = INSTR_OFF / PAGE + instr_pages;
    let heap = Dma::alloc(pages as i32)?;
    for (i, k) in KERNELS.iter().enumerate() {
        let mut b = [0u8; PAGE as usize];
        for (c, chunk) in k.code.chunks(PAGE as usize / 4).enumerate() {
            for (j, w) in chunk.iter().enumerate() {
                b[j * 4..j * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            if !heap.write(INSTR_OFF + kernel_off[i] + c as u32 * PAGE, &b[..chunk.len() * 4]) {
                return None;
            }
        }
    }
    let va = crate::gpudev::ppgtt_bind(gt, heap.phys, (pages * PAGE) as u64, TGL_PAT_CACHE_LLC).ok()?;
    Some(gpgpu { heap, va, instr_pages, kernel_off, e })
}

/// Runs `k` over `groups` workgroups with `push` as its push constants, as
/// one request on the render engine's kernel context, and waits for it.
pub fn gpgpu_dispatch(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu, k: usize, push: &[u8],
                      groups: [u32; 3]) -> Result<(), i32> {
    let mut b = gp.begin(gt)?;
    gp.dispatch(&mut b, k, push, groups)?;
    gpgpu_run(i915, gt, gp, &mut b, &mut || {})
}

/// Ends `b` and runs it as one request on the render engine's kernel
/// context, waiting for it. `on_timeout` runs before a late request wedges
/// the GT, so the caller can say what the batch held.
pub fn gpgpu_run(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu, b: &mut batch,
                 on_timeout: &mut dyn FnMut()) -> Result<(), i32> {
    let mut f = gpgpu_submit(i915, gt, gp, b)?;
    loop {
        if let Some(r) = gpgpu_poll(i915, gt, gp, &mut f, b, on_timeout) {
            return r;
        }
        crate::i915_irq::intel_irq_wait(i915, 2);
        crate::intel_rps::intel_rps_run_pending(i915, gt);
        crate::i915_gem_context::poll_native(i915, gt);
    }
}

/// A batch on the render engine (`gpgpu_submit`) until `gpgpu_poll` says
/// it is done.
pub struct inflight {
    rq: usize,
    t0: u64,
    started: Option<u64>,
    err: Result<(), i32>,
}

/// Ends `b` and adds it as one request on the render engine's kernel
/// context, without waiting for it.
pub fn gpgpu_submit(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu, b: &mut batch) -> Result<inflight, i32> {
    if crate::intel_gt::intel_gt_is_wedged(gt) {
        return Err(-EIO);
    }
    gp.end(b)?;
    let e = gp.e;
    let ce = gt.engine[e].and_then(|x| x.kernel_context).ok_or(-EINVAL)? as usize;

    crate::intel_engine_pm::intel_engine_pm_get(i915, gt, e);
    let rq = crate::i915_request::i915_request_create(i915, gt, ce);
    crate::intel_engine_pm::intel_engine_pm_put(i915, gt, e);
    let rq = rq?;
    // The desktop waits for it (I915_PRIORITY_DISPLAY, as Linux raises
    // work a flip waits for).
    gt.requests[rq].prio = crate::i915_request::I915_PRIORITY_DISPLAY;

    let mut err = crate::gen8_engine_cs::emit_flush(i915, gt, rq, EMIT_INVALIDATE);
    if err.is_ok() {
        err = crate::gen8_engine_cs::gen8_emit_bb_start(i915, gt, rq, gp.addr(BATCH_OFF), 0);
    }
    let rq = crate::i915_request::i915_request_get(gt, rq);
    crate::i915_request::i915_request_add(i915, gt, rq);
    if !crate::i915_request::__i915_request_has_started(gt, rq) {
        crate::intel_rps::intel_rps_boost(gt, rq);
    }
    crate::intel_rps::intel_rps_run_pending(i915, gt);
    // The timeout counts from the start: a client batch may run ahead of it.
    Ok(inflight { rq, t0: linux::now_us(), started: None, err })
}

/// The outcome of `f` once its request is complete, None while it is not.
/// A request still queued behind other work after the job's time is given
/// up (it runs as MI_NOOPs and retires; the kernel does the job on the
/// CPU); one that runs too long wedges the GT after `on_timeout`.
pub fn gpgpu_poll(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu, f: &mut inflight, b: &batch,
                  on_timeout: &mut dyn FnMut()) -> Option<Result<(), i32>> {
    let (e, rq) = (gp.e, f.rq);
    crate::intel_gt_requests::intel_engines_cs_irq(i915, gt);
    crate::intel_gt_requests::intel_engine_flush_submission(i915, gt, e);
    if !crate::i915_request::__i915_request_is_complete(gt, rq) {
        let now = linux::now_us();
        if f.started.is_none() && crate::intel_execlists_submission::execlists_running(gt, e, rq) {
            f.started = Some(now);
        }
        if f.started.is_none() && now - f.t0 > DISPATCH_TIMEOUT_US && crate::i915_request::i915_request_cancel(gt, rq) {
            crate::i915_request::i915_request_put(gt, rq);
            return Some(Err(-ETIME));
        }
        if f.started.is_some_and(|s| now - s > DISPATCH_TIMEOUT_US) || now - f.t0 > crate::gpudev::START_TIMEOUT_US {
            crate::i915_request::i915_request_put(gt, rq);
            linux::say("gpgpu: dispatch on rcs0 timed out, wedging");
            on_timeout();
            let mut ticks = [0u64; MAX_DISPATCHES as usize];
            let n = gp.dispatch_ticks(b, &mut ticks);
            let mut l = Line::new();
            l.s("gpgpu: dispatches done before the timeout (GT ticks):");
            for &t in &ticks[..n] {
                if t == u64::MAX {
                    l.s(" -");
                } else {
                    l.s(" ").dec(t);
                }
            }
            l.dbg();
            crate::intel_engine_cs::intel_engine_print_registers(i915, gt, e);
            crate::intel_gt::gen8_check_faults(i915);
            crate::intel_gt::intel_gt_set_wedged(i915, gt);
            return Some(Err(-ETIME));
        }
        return None;
    }
    let failed = gt.requests[rq].error;
    crate::i915_request::i915_request_put(gt, rq);
    crate::intel_gt_requests::intel_gt_retire_requests(i915, gt);
    Some(f.err.and_then(|_| if failed != 0 { Err(failed) } else { Ok(()) }))
}

/// The self-test: `fill_pattern` writes (seed << 24) | (y << 12) | x into a
/// 64x64 surface of a module buffer, read back and compared.
pub fn gpgpu_self_test(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu) -> bool {
    const W: u32 = 64;
    const H: u32 = 64;
    const SEED: u32 = 0x5a;
    let pitch = W * 4;
    let pages = (pitch * H).div_ceil(PAGE);
    let Some(dst) = Dma::alloc(pages as i32) else { return false };
    let zero = [0u8; PAGE as usize];
    for p in 0..pages {
        if !dst.write(p * PAGE, &zero) {
            return false;
        }
    }
    let Ok(va) = crate::gpudev::ppgtt_bind(gt, dst.phys, (pages * PAGE) as u64, TGL_PAT_CACHE_LLC) else {
        return false;
    };

    let mut push = [0u8; 32];
    let mut put = |(off, size): (usize, usize), v: u64| push[off..off + size].copy_from_slice(&v.to_le_bytes()[..size]);
    put(g::FILL_PATTERN_PUSH_DST, va);
    put(g::FILL_PATTERN_PUSH_WIDTH, W as u64);
    put(g::FILL_PATTERN_PUSH_HEIGHT, H as u64);
    put(g::FILL_PATTERN_PUSH_PITCH_PX, W as u64);
    put(g::FILL_PATTERN_PUSH_SEED, SEED as u64);
    let k = KERNELS[K_FILL_PATTERN];
    let groups = [W.div_ceil(k.local_size[0]), H.div_ceil(k.local_size[1]), 1];

    let t0 = linux::now_us();
    if let Err(x) = gpgpu_dispatch(i915, gt, gp, K_FILL_PATTERN, &push[..k.push_size as usize], groups) {
        Line::new().s("gpgpu: self-test dispatch failed (").dec((-x) as u64).s(")").say();
        return false;
    }
    let us = linux::now_us() - t0;

    let mut row = [0u8; 256];
    for y in 0..H {
        if !dst.read(y * pitch, &mut row) {
            return false;
        }
        for x in 0..W {
            let want = SEED << 24 | y << 12 | x;
            let v = u32::from_le_bytes(row[(x * 4) as usize..(x * 4 + 4) as usize].try_into().unwrap());
            if v != want {
                Line::new().s("gpgpu: self-test read back ").hex(v).s(" at ").dec(x as u64).s(",").dec(y as u64)
                    .s(", expected ").hex(want).dbg();
                return false;
            }
        }
    }
    Line::new().s("gpgpu: fill_pattern 64x64 on rcs0 in ").dec(us).s(" us, SIMD16, ")
        .dec(k.code.len() as u64 * 4).s(" bytes of program, ").dec(gt.info.sseu.subslice_mask[0].count_ones() as u64)
        .s(" subslices").dbg();
    true
}
