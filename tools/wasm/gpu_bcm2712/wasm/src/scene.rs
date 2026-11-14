//! The gpudev scene executor on V3D: the scene checked and planned by
//! gpu_scene (`plan_draws`), each damage rectangle drawn as one render pass
//! the way v3dv records it (`gpu_bcm2712_core::v3d_draw`): one draw per
//! item with draw/draw.frag for its kind, which reads the pixel the tile
//! buffer holds and paints the item over it exactly as the CPU executor
//! does. The self-test dispatches a compute program as one CSD job with
//! its cache-clean job. Everything is queued on
//! V3D's scheduler (`gpu_bcm2712_core::v3d_sched`) and finished by its
//! interrupts. The module is also V3D's runtime PM (50 ms autosuspend, as
//! v3d_drv.c sets it), its overflow-memory worker and its reset path. Not a
//! Linux file.
//!
//! Every buffer V3D reads or writes is mapped by the kernel
//! (`npk_v3d_map`): the module's own (programs, uniforms, the scene,
//! spill, the render jobs' lists and tile memory) from DMA pages,
//! the kernel's from their gpudev pages. The module writes only V3D
//! addresses the kernel handed out.

use crate::hw::PiHw;
use alloc::vec::Vec;
use drm_core::linux::Line;
use gpu_bcm2712_core::hw::{Hw, Spi};
use gpu_bcm2712_core::shaders_gen::{self as sg, Program, U};
use gpu_bcm2712_core::v3d::{self, CsdProgram};
use gpu_bcm2712_core::v3d_draw::{self, BoAlloc, BoKind};
use gpu_bcm2712_core::v3d_sched::{Event, Fence, Queue, Sched};
use gpu_scene::plan::{self, rect};

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const EIO: i32 = 5;
const PAGE: u32 = 4096;

const SCENE_PAGES: u32 = 16;
/// Render-job buffers kept for reuse once their job finished (v3dv's BO
/// cache); more are given back.
const FREE_BOS_KEPT: usize = 24;
/// draw.frag's item kinds, 1..=9; the pipeline of kind k is at k - 1.
const DRAW_KINDS: usize = 9;
/// Programs start at this alignment in the code buffer.
const CODE_ALIGN: u32 = 256;
/// `pm_runtime_set_autosuspend_delay(dev, 50)` in v3d_platform_drm_probe.
const AUTOSUSPEND_US: u64 = 50_000;
/// The performance counters a scene is measured with (`PERF_COUNTERS_71`
/// names; at most 32, as one perfmon holds).
pub const PERF_COUNTERS: [&str; 32] = [
    "cycle-count", "compute-active-cycles", "QPU-active", "QPU-stalls", "QPU-wait-bubble",
    "QPU-ic-miss-bubble", "QPU-stalls-TMU", "QPU-stalls-uniforms", "QPU-stalls-SFU", "QPU-stalls-other",
    "QPU-total-instr-cache-hit", "QPU-total-instr-cache-miss", "QPU-total-uniform-cache-hit",
    "QPU-total-uniform-cache-miss", "TMU-active-cycles", "TMU-stalled-cycles", "TMU-total-text-quads-access",
    "L2T-total-cache-hit", "L2T-total-cache-miss", "L2T-TMU-read-hits", "L2T-TMU-read-miss", "L2T-stalls-mem",
    "L2T-writeback", "core-memory-reads", "core-memory-writes", "L2T-memory-reads", "L2T-memory-writes",
    "AXI-read-trans", "AXI-write-trans", "AXI-read-wait-cycles", "AXI-write-wait-cycles",
    "AXI-max-outstanding-reads",
];
/// Finished jobs whose time `take_job_us` can still collect.
const JOB_US_KEPT: usize = 64;
/// Uniform streams: one slot per dispatch that may be in flight.
const UNIFORM_SLOTS: u32 = 16;
/// `v3d_overflow_mem_work`'s buffer: 256 KiB.
const OVERFLOW_PAGES: u32 = 64;

/// A DMA buffer of the module, mapped for V3D.
pub struct Buf {
    dma: i32,
    pub va: u32,
    pub bytes: u32,
}

impl Buf {
    fn new(pages: u32) -> Option<Buf> {
        let dma = npk_sys::dma_alloc(pages as i32);
        if dma < 0 {
            return None;
        }
        let phys = npk_sys::dma_phys_addr(dma) as u64;
        let list: Vec<u64> = (0..pages as u64).map(|i| phys + i * PAGE as u64).collect();
        let va = npk_sys::v3d_map(&list);
        if va <= 0 {
            npk_sys::dma_free(dma);
            return None;
        }
        Some(Buf { dma, va: va as u32, bytes: pages * PAGE })
    }

    fn write(&self, off: u32, data: &[u8]) -> bool {
        off as usize + data.len() <= self.bytes as usize && npk_sys::dma_write(self.dma, off as i32, data) >= 0
    }

    fn read(&self, off: u32, out: &mut [u8]) -> bool {
        off as usize + out.len() <= self.bytes as usize && npk_sys::dma_read(self.dma, off as i32, out) >= 0
    }

    /// Unmapped for V3D, the pages given back.
    fn free(self) {
        let _ = npk_sys::v3d_unmap(self.va as i64);
        npk_sys::dma_free(self.dma);
    }
}

/// What the module knows of V3D and its programs.
pub struct Composer {
    qpus: u32,
    rev: u32,
    code: Buf,
    /// Per `sg::PROGRAMS` entry, its V3D address.
    code_va: Vec<u32>,
    /// `UNIFORM_SLOTS` uniform streams of `uslot` bytes; the fence that last
    /// used each slot (0: never).
    uniforms: Buf,
    uslot: u32,
    uslots: [Fence; UNIFORM_SLOTS as usize],
    sched: Sched,
    /// The fence of the last submission, which the next one waits for.
    last: Fence,
    /// Dispatches by fence, for the trace.
    names: Vec<(Fence, &'static str, [u32; 3])>,
    /// Overflow buffers handed to the binner, by handle.
    overflow: Vec<(u32, Buf)>,
    next_overflow: u32,
    /// Runtime PM: powered, and idle since (us) while powered.
    powered: bool,
    idle_since: u64,
    /// `logged_error` of v3d_hub_irq: an MMU fault is logged once.
    mmu_logged: bool,
    /// Time spent powering V3D up since `take_power_us` (us).
    power_us: u64,
    /// Count `PERF_COUNTERS` over the next submission's jobs (`v3d_perfmon`
    /// on the job), and whether they are counting.
    pub perf_armed: bool,
    perf_active: bool,
    /// Finished jobs and their time from start to the fence seen, until
    /// `take_job_us` collects them; the newest `JOB_US_KEPT`.
    job_us: Vec<(Fence, u64)>,
    scene: Buf,
    /// The draw programs (`sg::DRAW_PIPELINES`) and a pipeline per kind
    /// with their addresses.
    draw_code: Buf,
    pipelines: [v3d_draw::Pipeline; DRAW_KINDS],
    bos: BoPool,
    spill: Option<Buf>,
    /// Dispatches since the composer started.
    pub dispatches: u64,
    /// While set, each dispatch's program, grid and time (submit to done,
    /// caches cleaned) are kept here.
    pub trace: Option<Vec<(&'static str, [u32; 3], u64)>>,
}

/// The draw programs into one buffer, each at `CODE_ALIGN`, and the
/// pipeline of every kind pointing at its three.
fn upload_draw_programs() -> Option<(Buf, [v3d_draw::Pipeline; DRAW_KINDS])> {
    let mut code: Vec<u8> = Vec::new();
    let mut placed: Vec<(*const u64, u32)> = Vec::new();
    let mut place = |qpu: &'static [u64]| -> u32 {
        if let Some(&(_, at)) = placed.iter().find(|(p, _)| *p == qpu.as_ptr()) {
            return at;
        }
        let at = code.len() as u32;
        code.extend(qpu.iter().flat_map(|q| q.to_le_bytes()));
        code.resize(code.len().div_ceil(CODE_ALIGN as usize) * CODE_ALIGN as usize, 0);
        placed.push((qpu.as_ptr(), at));
        at
    };
    let mut at = [[0u32; 3]; DRAW_KINDS];
    for (k, g) in sg::DRAW_PIPELINES.iter().enumerate().take(DRAW_KINDS) {
        if g.kind as usize != k + 1 {
            return None;
        }
        at[k] = [place(g.vs.qpu), place(g.cs.qpu), place(g.fs.qpu)];
    }
    let buf = Buf::new((code.len() as u32).div_ceil(PAGE))?;
    if !buf.write(0, &code) {
        return None;
    }
    let mut pipelines = [v3d_draw::Pipeline::default(); DRAW_KINDS];
    for (k, g) in sg::DRAW_PIPELINES.iter().enumerate().take(DRAW_KINDS) {
        pipelines[k] = v3d_draw::Pipeline {
            vs: v3d_draw::Variant { code: buf.va + at[k][0], prog: g.vs.prog },
            cs: v3d_draw::Variant { code: buf.va + at[k][1], prog: g.cs.prog },
            fs: v3d_draw::Variant { code: buf.va + at[k][2], prog: g.fs.prog },
            // VK_COLOR_COMPONENT_R_BIT: the one channel of R32_UINT.
            color_write_mask: 0x1,
        };
    }
    Some((buf, pipelines))
}

/// The render jobs' buffers (`v3dv_bo_alloc` behind v3dv's BO cache):
/// handed out for the job being recorded, held until its fence signals,
/// then kept for the next job of the same size.
struct BoPool {
    free: Vec<Buf>,
    /// The buffers of the job being recorded.
    job: Vec<Buf>,
    held: Vec<(Fence, Vec<Buf>)>,
}

impl BoAlloc for BoPool {
    fn alloc(&mut self, _kind: BoKind, size: u32) -> Option<u32> {
        let pages = size.div_ceil(PAGE);
        let b = match self.free.iter().position(|b| b.bytes == pages * PAGE) {
            Some(i) => self.free.swap_remove(i),
            None => Buf::new(pages)?,
        };
        let va = b.va;
        self.job.push(b);
        Some(va)
    }
}

impl BoPool {
    /// The recorded job's buffers back to the pool (it was not submitted).
    fn drop_job(&mut self) {
        let job = core::mem::take(&mut self.job);
        self.keep(job);
    }

    /// The buffers of the job that `fence` finished, back to the pool.
    fn done(&mut self, fence: Fence) {
        while let Some(i) = self.held.iter().position(|(f, _)| *f == fence) {
            let (_, bufs) = self.held.swap_remove(i);
            self.keep(bufs);
        }
    }

    fn keep(&mut self, bufs: Vec<Buf>) {
        for b in bufs {
            if self.free.len() < FREE_BOS_KEPT {
                self.free.push(b);
            } else {
                b.free();
            }
        }
    }
}

fn program_index(p: &Program) -> usize {
    sg::PROGRAMS.iter().position(|q| core::ptr::eq(*q, p)).expect("a generated program")
}

/// Brings V3D up (`npk_v3d_open`), its interrupts on, and the programs
/// and buffers in place.
pub fn composer_init(hw: &mut PiHw) -> Option<Composer> {
    let id = npk_sys::v3d_open();
    if id < 0 {
        Line::new().s("v3d: not available").say();
        return None;
    }
    let (qpus, rev) = ((id & 0xff) as u32, ((id >> 8) & 0xff) as u32);
    if !hw.map_v3d() {
        Line::new().s("v3d: its windows or interrupts are missing").say();
        return None;
    }
    v3d::irq_enable(hw);

    // Only the compute programs the module dispatches; an unused one has no
    // address (0).
    let mut offs = Vec::new();
    let mut total = 0u32;
    for p in sg::PROGRAMS {
        offs.push(total);
        if dispatched(p) {
            total += (p.qpu.len() as u32 * 8).div_ceil(CODE_ALIGN) * CODE_ALIGN;
        }
    }
    let code = Buf::new(total.div_ceil(PAGE))?;
    for (p, &off) in sg::PROGRAMS.iter().zip(offs.iter()) {
        if !dispatched(p) {
            continue;
        }
        let bytes: Vec<u8> = p.qpu.iter().flat_map(|q| q.to_le_bytes()).collect();
        if !code.write(off, &bytes) {
            return None;
        }
    }
    let (draw_code, pipelines) = upload_draw_programs()?;
    let used = || sg::PROGRAMS.iter().filter(|p| dispatched(p));
    let max_uniforms = used().map(|p| p.uniforms.len() as u32).max().unwrap_or(0);
    let uslot = (max_uniforms * 4).div_ceil(CODE_ALIGN) * CODE_ALIGN;
    let spill_size = used().map(|p| p.spill_size).max().unwrap_or(0);
    // pipeline_check_spill_size: the thread index picks the area, (core <<
    // 6) | (qpu << 2) | thread, so four threads per QPU at any thread count.
    let spill = if spill_size > 0 { Some(Buf::new((4 * qpus * spill_size).div_ceil(PAGE))?) } else { None };
    let c = Composer {
        qpus,
        rev,
        code_va: sg::PROGRAMS.iter().zip(offs.iter()).map(|(p, &o)| if dispatched(p) { code.va + o } else { 0 })
            .collect(),
        code,
        uniforms: Buf::new((uslot * UNIFORM_SLOTS).div_ceil(PAGE))?,
        uslot,
        uslots: [0; UNIFORM_SLOTS as usize],
        sched: Sched::new(),
        last: 0,
        names: Vec::new(),
        overflow: Vec::new(),
        next_overflow: 1,
        powered: true,
        idle_since: npk_sys::now_us() as u64,
        mmu_logged: false,
        power_us: 0,
        perf_armed: false,
        perf_active: false,
        job_us: Vec::new(),
        scene: Buf::new(SCENE_PAGES)?,
        draw_code,
        pipelines,
        bos: BoPool { free: Vec::new(), job: Vec::new(), held: Vec::new() },
        spill,
        dispatches: 0,
        trace: None,
    };
    Line::new().s("v3d: ").dec(qpus as u64).s(" QPUs, rev ").dec(rev as u64).s(", ")
        .dec(total as u64).s(" bytes of programs at ").hex(c.code.va).say();
    for p in used() {
        Line::new().s("v3d:   ").s(p.name).s(" ").dec(p.qpu.len() as u64).s(" instructions, ")
            .dec(p.threads as u64).s(" threads, ").dec(p.uniforms.len() as u64).s(" uniforms, spills ")
            .dec(p.spill_size as u64).s(" bytes a thread").say();
    }
    Line::new().s("v3d: draw programs at ").hex(c.draw_code.va).s(", one pipeline per item kind").say();
    for g in sg::DRAW_PIPELINES {
        Line::new().s("v3d:   draw kind ").dec(g.kind as u64).s(": fragment ").dec(g.fs.qpu.len() as u64)
            .s(" instructions, ").dec(g.fs.prog.base.threads as u64).s(" threads").say();
    }
    Some(c)
}

impl Composer {
    /// Everything the composer holds given back, V3D idle (a start that
    /// failed its self-test).
    pub fn free(mut self, hw: &mut PiHw) {
        self.drain(hw);
        for (_, b) in self.overflow.drain(..) {
            b.free();
        }
        let held: Vec<Buf> = self.bos.held.drain(..).flat_map(|(_, b)| b).collect();
        for b in self.bos.free.drain(..).chain(self.bos.job.drain(..)).chain(held) {
            b.free();
        }
        for b in [self.code, self.draw_code, self.uniforms, self.scene] {
            b.free();
        }
        if let Some(b) = self.spill {
            b.free();
        }
    }
}

/// The compute programs the module dispatches: the self-test's.
/// compose.comp is the AMD and Intel modules'; here items are drawn.
fn dispatched(p: &Program) -> bool {
    core::ptr::eq(&sg::FILL_PATTERN, p)
}

/// A push-constant block for `p` from its members by name.
fn push_words(p: &Program, members: &[(&str, &[u8])]) -> Result<[u32; 32], i32> {
    let mut bytes = [0u8; sg::PUSH_BYTES as usize];
    for (name, v) in members {
        let &(_, off) = p.push.iter().find(|(n, _)| n == name).ok_or(-EINVAL)?;
        bytes[off as usize..off as usize + v.len()].copy_from_slice(v);
    }
    let mut w = [0u32; 32];
    for (i, c) in bytes.chunks_exact(4).enumerate() {
        w[i] = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    }
    Ok(w)
}

impl Composer {
    /// `v3d_power_resume` as the module's share: V3D powered (the kernel)
    /// and its interrupts on (`pm_runtime_resume_and_get` before a job).
    fn power_up(&mut self, hw: &mut PiHw) -> Result<(), i32> {
        if !self.powered {
            let t0 = npk_sys::now_us() as u64;
            let r = npk_sys::v3d_power(1);
            self.power_us += (npk_sys::now_us() as u64).saturating_sub(t0);
            if r != 0 {
                Line::new().s("v3d: power-up failed").say();
                return Err(-EIO);
            }
            v3d::irq_enable(hw);
            self.powered = true;
            self.idle_since = npk_sys::now_us() as u64;
        }
        Ok(())
    }

    /// The autosuspend: idle for `AUTOSUSPEND_US`, `v3d_power_suspend` (the
    /// interrupts off and the caches cleaned here, the rest the kernel's).
    pub fn autosuspend(&mut self, hw: &mut PiHw, now_us: u64) {
        if !self.powered || !self.sched.idle() || now_us < self.idle_since + AUTOSUSPEND_US {
            return;
        }
        v3d::irq_disable(hw);
        v3d::clean_caches(hw);
        if npk_sys::v3d_power(0) == 0 {
            self.powered = false;
        } else {
            v3d::irq_enable(hw);
            self.idle_since = now_us;
        }
    }

    /// When the module's loop must look again: the next job timeout or the
    /// autosuspend, in us from `now_us`.
    pub fn next_due(&self, now_us: u64) -> Option<u64> {
        let pm = (self.powered && self.sched.idle()).then_some(self.idle_since + AUTOSUSPEND_US);
        [self.sched.next_deadline(), pm].into_iter().flatten().min().map(|t| t.saturating_sub(now_us))
    }

    /// One dispatch of `p` over `grid` with `push`, queued after the last
    /// submission: the uniform stream filled as
    /// v3dv_write_uniforms_wg_offsets fills it, the CSD job and its
    /// cache-clean job submitted. The clean job's fence.
    pub fn submit_dispatch(&mut self, hw: &mut PiHw, p: &'static Program, push: &[u32; 32], grid: [u32; 3])
        -> Result<Fence, i32> {
        self.power_up(hw)?;
        if self.perf_armed && !self.perf_active {
            self.perf_start(hw);
        }
        self.perf_armed = false;
        // A uniform slot whose dispatch finished; the oldest one waited for
        // when all are in flight.
        let slot = match self.uslots.iter().position(|&f| f == 0 || self.sched.signaled(f)) {
            Some(i) => i,
            None => {
                let (i, &f) = self.uslots.iter().enumerate().min_by_key(|(_, f)| **f).expect("slots");
                self.wait_fence(hw, f)?;
                i
            }
        };
        let spill_va = self.spill.as_ref().map_or(0, |s| s.va);
        let mut stream: Vec<u8> = Vec::with_capacity(p.uniforms.len() * 4);
        for u in p.uniforms {
            let v = match *u {
                U::Const(v) => v,
                U::Push(n) => *push.get(n as usize).ok_or(-EINVAL)?,
                U::WgBase(_) => 0,
                U::NumWg(a) => *grid.get(a as usize).ok_or(-EINVAL)?,
                U::SpillOffset => spill_va,
                U::SpillSizePerThread => p.spill_size,
                U::ViewportXScale | U::ViewportYScale | U::ViewportZOffset | U::ViewportZScale => return Err(-EINVAL),
            };
            stream.extend_from_slice(&v.to_le_bytes());
        }
        let off = slot as u32 * self.uslot;
        if !self.uniforms.write(off, &stream) {
            return Err(-ENOMEM);
        }
        let cp = CsdProgram {
            threads: p.threads,
            single_seg: p.single_seg,
            local_size: p.local_size,
            can_use_supergroups: p.can_use_supergroups,
            has_control_barrier: p.has_control_barrier,
        };
        let cfg = v3d::csd_cfg(&cp, self.rev, self.qpus, grid, self.code_va[program_index(p)], self.uniforms.va + off);
        let deps: &[Fence] = if self.last != 0 { &[self.last] } else { &[] };
        let deps = deps.to_vec();
        let f = self.sched.submit_csd(cfg, &deps);
        self.uslots[slot] = f;
        self.last = f;
        self.dispatches += 1;
        if self.trace.is_some() {
            self.names.push((f - 1, p.name, grid));
        }
        self.pump(hw);
        Ok(f)
    }

    /// `v3d_perfmon_start` with `PERF_COUNTERS`, for the submission it
    /// was armed for.
    fn perf_start(&mut self, hw: &mut PiHw) {
        let ids: Vec<u8> = PERF_COUNTERS.iter()
            .map(|n| v3d::PERF_COUNTERS_71.iter().position(|m| m == n).expect("a V3D 7.1 counter") as u8)
            .collect();
        v3d::perfmon_start(hw, &ids);
        self.perf_active = true;
    }

    /// One render pass into `image` over `area`, recorded as v3dv records
    /// it, queued after the last submission (bin job, render job, and the
    /// cache clean v3dv asks for when a program wrote through the TMU).
    pub fn submit_draws(&mut self, hw: &mut PiHw, image: v3d_draw::Image, area: v3d_draw::Rect,
                        draws: &[v3d_draw::Draw]) -> Result<Fence, i32> {
        self.power_up(hw)?;
        if self.perf_armed && !self.perf_active {
            self.perf_start(hw);
        }
        self.perf_armed = false;
        let viewport = v3d_draw::Viewport { x: 0.0, y: 0.0, width: image.width as f32, height: image.height as f32,
                                            min_depth: 0.0, max_depth: 1.0 };
        let pipelines = self.pipelines;
        let pass = v3d_draw::Pass {
            rev: self.rev,
            image,
            render_area: area,
            viewport,
            scissor: v3d_draw::Rect { x: 0, y: 0, width: image.width, height: image.height },
            pipelines: &pipelines,
            draws,
        };
        let mut uniforms_ok = true;
        let mut fill = |d: usize, stage: v3d_draw::Stage, words: &mut [u32]| {
            let g = &sg::DRAW_PIPELINES[draws[d].pipeline];
            let list = match stage {
                v3d_draw::Stage::Fragment => g.fs.uniforms,
                v3d_draw::Stage::Vertex => g.vs.uniforms,
                v3d_draw::Stage::Coordinate => g.cs.uniforms,
            };
            uniforms_ok &= v3d_draw::write_uniforms(list, &draws[d].push, &viewport, words);
        };
        let job = match v3d_draw::record(&pass, &mut self.bos, &mut fill) {
            Ok(j) if uniforms_ok => j,
            _ => {
                self.bos.drop_job();
                return Err(-ENOMEM);
            }
        };
        for bo in job.bcl.iter().chain(job.rcl.iter()).chain(job.indirect.iter()) {
            let written = self.bos.job.iter().find(|b| b.va == bo.va).is_some_and(|b| b.write(0, &bo.bytes));
            if !written {
                self.bos.drop_job();
                return Err(-EIO);
            }
        }
        let deps: Vec<Fence> = if self.last != 0 { alloc::vec![self.last] } else { Vec::new() };
        let f = self.sched.submit_cl(Some(job.bin_job()), job.render_list(), job.submit.flush_cache, &deps);
        let bufs = core::mem::take(&mut self.bos.job);
        self.bos.held.push((f, bufs));
        self.last = f;
        self.dispatches += 1;
        if self.trace.is_some() {
            // The bin job, then the render job; a cache clean after it is
            // the submission's last fence.
            let render = if job.submit.flush_cache { f - 1 } else { f };
            self.names.push((render - 1, "bin", [draws.len() as u32, area.width, area.height]));
            self.names.push((render, "render", [draws.len() as u32, area.width, area.height]));
        }
        self.pump(hw);
        Ok(f)
    }

    /// Start what can run, and act on what the queues report.
    fn pump(&mut self, hw: &mut PiHw) {
        let mut ev = Vec::new();
        let now = npk_sys::now_us() as u64;
        self.sched.pump(hw, now, &mut ev);
        self.handle(hw, ev);
    }

    /// V3D's interrupts (`v3d_irq`, `v3d_hub_irq`) and the timeouts, then
    /// whatever may run next.
    pub fn service(&mut self, hw: &mut PiHw) {
        if !self.powered {
            return;
        }
        let mut ev = Vec::new();
        let now = npk_sys::now_us() as u64;
        let busy = !self.sched.idle();
        let core = v3d::core_irq(hw);
        let hub = v3d::hub_irq(hw);
        self.sched.irq(core, hub, now, &mut ev);
        self.sched.check_timeouts(hw, now, &mut ev);
        self.handle(hw, ev);
        self.pump(hw);
        // `pm_runtime_mark_last_busy` as the last job completes.
        if busy && self.sched.idle() {
            self.idle_since = now;
        }
    }

    fn handle(&mut self, hw: &mut PiHw, ev: Vec<Event>) {
        for e in ev {
            match e {
                Event::Done { fence, us, unref, .. } => {
                    self.bos.done(fence);
                    if self.job_us.len() == JOB_US_KEPT {
                        self.job_us.remove(0);
                    }
                    self.job_us.push((fence, us));
                    for h in unref {
                        if let Some(i) = self.overflow.iter().position(|(k, _)| *k == h) {
                            self.overflow.swap_remove(i).1.free();
                        }
                    }
                    if let Some(t) = self.trace.as_mut() {
                        if let Some(&(_, name, grid)) = self.names.iter().find(|n| n.0 == fence) {
                            t.push((name, grid, us));
                        }
                    }
                    self.names.retain(|n| n.0 != fence);
                }
                Event::OutOfMemory => self.overflow_mem_work(hw),
                Event::MmuFault { write_violation, pte_invalid, cap_exceeded } => {
                    if let Some((axi_id, addr)) = npk_sys::v3d_mmu_fault() {
                        if !self.mmu_logged {
                            Line::new().s("v3d: MMU error from client ").s(v3d::axi_client(axi_id)).s(" (")
                                .dec((axi_id & 0x7f) as u64).s(") at ").hex64(addr)
                                .s(if write_violation { ", write violation" } else { "" })
                                .s(if pte_invalid { ", pte invalid" } else { "" })
                                .s(if cap_exceeded { ", cap exceeded" } else { "" }).say();
                        }
                        self.mmu_logged = true;
                    }
                }
                Event::GmpViolation => Line::new().s("v3d: GMP Violation").say(),
                Event::Reset { queue, err_stat } => self.reset(hw, queue, err_stat),
            }
        }
    }

    /// `v3d_overflow_mem_work`: 256 KiB for the binner, given to the
    /// running bin job's render job; dropped when no bin job runs.
    fn overflow_mem_work(&mut self, hw: &mut PiHw) {
        let Some(b) = Buf::new(OVERFLOW_PAGES) else {
            Line::new().s("v3d: Couldn't allocate binner overflow mem").say();
            return;
        };
        let h = self.next_overflow;
        self.next_overflow += 1;
        if !self.sched.attach_overflow(h) {
            b.free();
            return;
        }
        v3d::overflow_mem(hw, b.va, b.bytes);
        self.overflow.push((h, b));
    }

    /// `v3d_gpu_reset_for_timeout` with `v3d_reset`: the module's and the
    /// kernel's share, then the queues again.
    fn reset(&mut self, hw: &mut PiHw, queue: Queue, err_stat: u32) {
        Line::new().s("v3d: Resetting GPU for hang.").say();
        Line::new().s("v3d: V3D_ERR_STAT: ").hex(err_stat).say();
        v3d::irq_disable(hw);
        if npk_sys::v3d_reset() != 0 {
            Line::new().s("v3d: the kernel could not reset V3D").say();
        }
        v3d::irq_reset(hw);
        let mut ev = Vec::new();
        self.sched.reset_done(queue, npk_sys::now_us() as u64, &mut ev);
        self.handle(hw, ev);
    }

    /// Serve V3D until `f` signalled. False when it failed.
    pub fn wait_fence(&mut self, hw: &mut PiHw, f: Fence) -> Result<(), i32> {
        loop {
            if self.sched.signaled(f) {
                return Ok(());
            }
            let since = [hw.irq_arm(Spi::V3dCore), hw.irq_arm(Spi::V3dHub)];
            self.service(hw);
            if self.sched.signaled(f) {
                return Ok(());
            }
            let now = npk_sys::now_us() as u64;
            let ms = self.sched.next_deadline().map_or(10, |d| d.saturating_sub(now).div_ceil(1000).max(1));
            if hw.irq_arm(Spi::V3dCore) == since[0] && hw.irq_arm(Spi::V3dHub) == since[1] {
                npk_sys::wait(crate::WAIT_IRQ, ms.min(1000) as i32);
            }
        }
    }

    /// Serve V3D until everything submitted finished: before a buffer the
    /// queued jobs may still reach goes away (a job holds its BOs in
    /// Linux until it is freed).
    pub fn drain(&mut self, hw: &mut PiHw) {
        if self.last != 0 {
            let _ = self.wait_fence(hw, self.last);
        }
    }

    /// The counters since the submission they were armed for, which stop.
    /// None when none counted (V3D was not powered by it).
    pub fn perf_take(&mut self, hw: &mut PiHw) -> Option<[u64; 32]> {
        if !core::mem::take(&mut self.perf_active) || !self.powered {
            return None;
        }
        let mut v = [0u64; 32];
        v3d::perfmon_stop(hw, &mut v, true);
        Some(v)
    }

    /// The power-up time since the last call (us).
    pub fn take_power_us(&mut self) -> u64 {
        core::mem::take(&mut self.power_us)
    }

    /// The summed time of the finished jobs up to fence `upto` (us), which
    /// are then forgotten.
    pub fn take_job_us(&mut self, upto: Fence) -> u64 {
        let sum = self.job_us.iter().filter(|(f, _)| *f <= upto).map(|(_, us)| us).sum();
        self.job_us.retain(|(f, _)| *f > upto);
        sum
    }

    /// Has fence `f` signalled, and did its job fail?
    pub fn fence_state(&self, f: Fence) -> (bool, bool) {
        (self.sched.signaled(f), self.sched.failed(f))
    }

    /// The draw path proved: one render pass into a 64x48 buffer of the
    /// module's, an opaque fill over all of it and a half-transparent one
    /// over part, read back against render::blend. 48 rows end inside a
    /// tile: a store past the image would fault on the unmapped page after
    /// the buffer.
    pub fn draw_self_test(&mut self, hw: &mut PiHw) -> bool {
        const W: u32 = 64;
        const H: u32 = 48;
        const BG: u32 = 0x0011_2233;
        const FG: u32 = 0x00ff_8040;
        const PART: [u32; 4] = [8, 8, 40, 24];
        let Some(dst) = Buf::new(W * H * 4 / PAGE) else { return false };
        let fill = |r: [u32; 4], color: u32, alpha: u32| {
            let mut push = [0u32; v3d_draw::PUSH_WORDS];
            push[..4].copy_from_slice(&r);
            // Item::Fill: kind, x, y, w, h, color, alpha, radius.
            push[4..12].copy_from_slice(&[8, r[0], r[1], r[2] - r[0], r[3] - r[1], color, alpha, 0]);
            push[30] = W;
            push[31] = H;
            v3d_draw::Draw { pipeline: 7, push, vertex_count: 4 }
        };
        let draws = [fill([0, 0, W, H], BG, 255), fill(PART, FG, 128)];
        let image = v3d_draw::Image { va: dst.va, stride: W * 4, width: W, height: H };
        let area = v3d_draw::Rect { x: 0, y: 0, width: W, height: H };
        let r = (|| -> Result<bool, i32> {
            let t0 = npk_sys::now_us() as u64;
            let faulted = self.mmu_logged;
            let f = self.submit_draws(hw, image, area, &draws)?;
            self.wait_fence(hw, f)?;
            if self.sched.failed(f) {
                return Err(-EIO);
            }
            if self.mmu_logged && !faulted {
                Line::new().s("v3d: draw self-test faulted in V3D's MMU").say();
                return Ok(false);
            }
            let us = npk_sys::now_us() as u64 - t0;
            // render::blend at 128 (Item::Fill: 128 * 256 / 255).
            let blend = |fg: u32, bg: u32, a: u32| {
                let ch = |s: u32| ((((fg >> s) & 255) * a + ((bg >> s) & 255) * (256 - a)) >> 8) << s;
                ch(16) | ch(8) | ch(0)
            };
            let mut row = [0u8; (W * 4) as usize];
            for y in 0..H {
                if !dst.read(y * W * 4, &mut row) {
                    return Err(-EIO);
                }
                for x in 0..W {
                    let inside = x >= PART[0] && x < PART[2] && y >= PART[1] && y < PART[3];
                    let want = if inside { blend(FG, BG, 128) } else { BG };
                    let i = (x * 4) as usize;
                    let v = u32::from_le_bytes([row[i], row[i + 1], row[i + 2], row[i + 3]]);
                    if v != want {
                        Line::new().s("v3d: draw self-test read back ").hex(v).s(" at ").dec(x as u64).s(",")
                            .dec(y as u64).s(", expected ").hex(want).say();
                        return Ok(false);
                    }
                }
            }
            Line::new().s("v3d: two draws over 64x48 in ").dec(us).s(" us").say();
            Ok(true)
        })();
        let _ = npk_sys::v3d_unmap(dst.va as i64);
        npk_sys::dma_free(dst.dma);
        match r {
            Ok(v) => v,
            Err(e) => {
                Line::new().s("v3d: draw self-test failed (").dec(e.unsigned_abs() as u64).s(")").say();
                false
            }
        }
    }

    /// The dispatch path proved: fill_pattern over 64x64 into a buffer of
    /// the module's, read back word for word.
    pub fn self_test(&mut self, hw: &mut PiHw) -> bool {
        const W: u32 = 64;
        const H: u32 = 64;
        const SEED: u32 = 0x5a;
        let Some(dst) = Buf::new(W * H * 4 / PAGE) else { return false };
        let r = (|| -> Result<bool, i32> {
            let push = push_words(&sg::FILL_PATTERN, &[
                ("dst", &(dst.va as u64).to_le_bytes()),
                ("width", &W.to_le_bytes()),
                ("height", &H.to_le_bytes()),
                ("pitch_px", &W.to_le_bytes()),
                ("seed", &SEED.to_le_bytes()),
            ])?;
            let t0 = npk_sys::now_us() as u64;
            let f = self.submit_dispatch(hw, &sg::FILL_PATTERN, &push, [W.div_ceil(sg::WIDTH), H, 1])?;
            self.wait_fence(hw, f)?;
            let us = npk_sys::now_us() as u64 - t0;
            let mut row = [0u8; (W * 4) as usize];
            for y in 0..H {
                if !dst.read(y * W * 4, &mut row) {
                    return Err(-EIO);
                }
                for x in 0..W {
                    let want = SEED << 24 | y << 12 | x;
                    let i = (x * 4) as usize;
                    let v = u32::from_le_bytes([row[i], row[i + 1], row[i + 2], row[i + 3]]);
                    if v != want {
                        Line::new().s("v3d: self-test read back ").hex(v).s(" at ").dec(x as u64).s(",")
                            .dec(y as u64).s(", expected ").hex(want).say();
                        return Ok(false);
                    }
                }
            }
            Line::new().s("v3d: fill_pattern 64x64 in ").dec(us).s(" us").say();
            Ok(true)
        })();
        let _ = npk_sys::v3d_unmap(dst.va as i64);
        npk_sys::dma_free(dst.dma);
        match r {
            Ok(v) => v,
            Err(e) => {
                Line::new().s("v3d: self-test failed (").dec(e.unsigned_abs() as u64).s(")").say();
                false
            }
        }
    }

    /// `OP_SCENE` of job `seq`: `bytes` of scene into buffer `target`,
    /// checked, planned and queued. The dispatches and the fence of the
    /// last; the scene memory is the previous scene's until that finished,
    /// so it is waited for first.
    pub fn run_scene(&mut self, hw: &mut PiHw, bufs: &dyn gpu_scene::scene::buffers, seq: u32, target: u32,
                     bytes: u32) -> Result<(u32, Fence), i32> {
        if self.last != 0 {
            self.wait_fence(hw, self.last)?;
        }
        let mut mem = SceneMem(&self.scene);
        let ck = gpu_scene::scene::check_scene(&JobPayload(seq), bytes, bufs, &mut mem, target)?;
        let before = self.dispatches;
        let mut s = Sink { c: self, hw, ck: &ck };
        if let Err(e) = plan::plan_draws(&mut s, &ck.screen, &ck.damage, &ck.shapes) {
            // Passes queued before the refusal still write the target:
            // the CPU redraws it only once they are done.
            self.drain(hw);
            return Err(e);
        }
        Ok(((self.dispatches - before) as u32, self.last))
    }
}

struct JobPayload(u32);

impl gpu_scene::scene::payload for JobPayload {
    fn read(&self, off: u32, out: &mut [u8]) -> Result<(), i32> {
        if npk_sys::gpudev_job_data(self.0 as i32, off as i32, out) != out.len() as i32 {
            return Err(-EIO);
        }
        Ok(())
    }
}

struct SceneMem<'a>(&'a Buf);

impl gpu_scene::scene::scene_buffer for SceneMem<'_> {
    fn va(&self) -> u64 {
        self.0.va as u64
    }

    fn bytes(&self) -> u32 {
        self.0.bytes
    }

    fn write(&mut self, off: u32, data: &[u8]) -> bool {
        self.0.write(off, data)
    }
}

/// plan.rs's steps: each one render pass.
struct Sink<'a> {
    c: &'a mut Composer,
    hw: &'a mut PiHw,
    ck: &'a gpu_scene::scene::checked,
}

impl plan::draw_sink for Sink<'_> {
    fn draw(&mut self, area: &rect) -> Result<(), i32> {
        let ck = self.ck;
        let end = ck.items.len();
        let mut draws = Vec::with_capacity(end);
        for i in 0..end {
            let kind = ck.items[i][0] as usize;
            if !(1..=DRAW_KINDS).contains(&kind) {
                return Err(-EINVAL);
            }
            // Only where no later item paints over it opaquely.
            for r in plan::visible_parts(&ck.shapes, end, i, area) {
                let mut push = [0u32; v3d_draw::PUSH_WORDS];
                push[..gpu_scene::scene::DRAW_PUSH_WORDS].copy_from_slice(&gpu_scene::scene::draw_push(ck, i, &r));
                draws.push(v3d_draw::Draw { pipeline: kind - 1, push, vertex_count: 4 });
            }
        }
        if draws.is_empty() {
            return Ok(());
        }
        let t = &ck.target;
        if t.va > u32::MAX as u64 {
            return Err(-EINVAL);
        }
        let image = v3d_draw::Image { va: t.va as u32, stride: t.pitch, width: ck.width, height: ck.height };
        let area = v3d_draw::Rect { x: area.x, y: area.y, width: area.w, height: area.h };
        self.c.submit_draws(self.hw, image, area, &draws).map(|_| ())
    }
}
