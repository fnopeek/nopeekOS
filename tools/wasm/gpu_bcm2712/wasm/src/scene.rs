//! The gpudev scene executor on V3D: the scene checked and planned by
//! gpu_scene, run by compose.comp, blur_down.comp and blur_box.comp as v3dv
//! compiled them (`shaders_gen`), one CSD job per dispatch with its
//! cache-clean job, queued on V3D's scheduler
//! (`gpu_bcm2712_core::v3d_sched`) and finished by its interrupts. The
//! module is also V3D's runtime PM (50 ms autosuspend, as v3d_drv.c sets
//! it), its overflow-memory worker and its reset path. Not a Linux file.
//!
//! Every buffer V3D reads or writes is mapped by the kernel
//! (`npk_v3d_map`): the module's own (programs, uniforms, the scene, bins,
//! blur planes, spill) from DMA pages, the kernel's from their gpudev
//! pages. The module writes only V3D addresses the kernel handed out.

use crate::hw::PiHw;
use alloc::vec::Vec;
use drm_core::linux::Line;
use gpu_bcm2712_core::hw::{Hw, Spi};
use gpu_bcm2712_core::shaders_gen::{self as sg, Program, U};
use gpu_bcm2712_core::v3d::{self, CsdProgram};
use gpu_bcm2712_core::v3d_sched::{Event, Fence, Queue, Sched};
use gpu_scene::plan::{self, bin_table, grid, rect};

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const EIO: i32 = 5;
const PAGE: u32 = 4096;

const SCENE_PAGES: u32 = 16;
const BINS_PAGES: u32 = 128;
const BINS_WORDS: u32 = BINS_PAGES * PAGE / 4;
const PLANE_PAGES: u32 = 128;
const PLANE_BLOCKS: u32 = PLANE_PAGES * PAGE / 4;
/// Programs start at this alignment in the code buffer.
const CODE_ALIGN: u32 = 256;
/// `pm_runtime_set_autosuspend_delay(dev, 50)` in v3d_platform_drm_probe.
const AUTOSUSPEND_US: u64 = 50_000;
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
    scene: Buf,
    bins: Buf,
    planes: Buf,
    tmp: Buf,
    spill: Option<Buf>,
    /// Dispatches since the composer started.
    pub dispatches: u64,
    /// While set, each dispatch's program, grid and time (submit to done,
    /// caches cleaned) are kept here.
    pub trace: Option<Vec<(&'static str, [u32; 3], u64)>>,
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

    let mut offs = Vec::new();
    let mut total = 0u32;
    for p in sg::PROGRAMS {
        offs.push(total);
        total += (p.qpu.len() as u32 * 8).div_ceil(CODE_ALIGN) * CODE_ALIGN;
    }
    let code = Buf::new(total.div_ceil(PAGE))?;
    for (p, &off) in sg::PROGRAMS.iter().zip(offs.iter()) {
        let bytes: Vec<u8> = p.qpu.iter().flat_map(|q| q.to_le_bytes()).collect();
        if !code.write(off, &bytes) {
            return None;
        }
    }
    let max_uniforms = sg::PROGRAMS.iter().map(|p| p.uniforms.len() as u32).max().unwrap_or(0);
    let uslot = (max_uniforms * 4).div_ceil(CODE_ALIGN) * CODE_ALIGN;
    let spill_size = sg::PROGRAMS.iter().map(|p| p.spill_size).max().unwrap_or(0);
    // pipeline_check_spill_size: the thread index picks the area, (core <<
    // 6) | (qpu << 2) | thread, so four threads per QPU at any thread count.
    let spill = if spill_size > 0 { Some(Buf::new((4 * qpus * spill_size).div_ceil(PAGE))?) } else { None };
    let c = Composer {
        qpus,
        rev,
        code_va: offs.iter().map(|&o| code.va + o).collect(),
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
        scene: Buf::new(SCENE_PAGES)?,
        bins: Buf::new(BINS_PAGES)?,
        planes: Buf::new(PLANE_PAGES)?,
        tmp: Buf::new(PLANE_PAGES)?,
        spill,
        dispatches: 0,
        trace: None,
    };
    Line::new().s("v3d: ").dec(qpus as u64).s(" QPUs, rev ").dec(rev as u64).s(", ")
        .dec(total as u64).s(" bytes of programs at ").hex(c.code.va).say();
    for p in sg::PROGRAMS {
        Line::new().s("v3d:   ").s(p.name).s(" ").dec(p.qpu.len() as u64).s(" instructions, ")
            .dec(p.threads as u64).s(" threads, ").dec(p.uniforms.len() as u64).s(" uniforms, spills ")
            .dec(p.spill_size as u64).s(" bytes a thread").say();
    }
    Some(c)
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
            if npk_sys::v3d_power(1) != 0 {
                Line::new().s("v3d: power-up failed").say();
                return Err(-EIO);
            }
            v3d::irq_enable(hw);
            self.powered = true;
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
        let core = v3d::core_irq(hw);
        let hub = v3d::hub_irq(hw);
        self.sched.irq(core, hub, now, &mut ev);
        self.sched.check_timeouts(hw, now, &mut ev);
        self.handle(hw, ev);
        self.pump(hw);
        if self.sched.idle() {
            self.idle_since = now;
        }
    }

    fn handle(&mut self, hw: &mut PiHw, ev: Vec<Event>) {
        for e in ev {
            match e {
                Event::Done { fence, us, unref, .. } => {
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

    /// Has fence `f` signalled, and did its job fail?
    pub fn fence_state(&self, f: Fence) -> (bool, bool) {
        (self.sched.signaled(f), self.sched.failed(f))
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
        let lim = plan::limits { bins_words: BINS_WORDS, plane_blocks: PLANE_BLOCKS };
        let mut s = Sink { c: self, hw, target: ck.target, width: ck.width, height: ck.height, radius: ck.radius };
        plan::plan(&mut s, &lim, &ck.screen, &ck.damage, &ck.shapes, ck.radius)?;
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

/// plan.rs's steps as dispatches.
struct Sink<'a> {
    c: &'a mut Composer,
    hw: &'a mut PiHw,
    target: gpu_scene::scene::surface,
    width: u32,
    height: u32,
    radius: u32,
}

impl plan::sink for Sink<'_> {
    fn bins(&mut self, at: u32, words: &[u32]) -> Result<(), i32> {
        let b: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        if !self.c.bins.write(at * 4, &b) {
            return Err(-ENOMEM);
        }
        Ok(())
    }

    fn compose(&mut self, area: &rect, t: &bin_table, blur: Option<grid>) -> Result<(), i32> {
        let (a, grid) = gpu_scene::scene::compose(sg::WIDTH, self.c.scene.va as u64, self.c.planes.va as u64,
                                                  self.c.bins.va as u64 + t.at as u64 * 4, area, t, blur);
        let push = push_words(&sg::COMPOSE, &[
            ("frame", &a.frame.to_le_bytes()),
            ("planes", &a.planes.to_le_bytes()),
            ("bins", &a.bins.to_le_bytes()),
            ("clip_x", &a.clip_x.to_le_bytes()),
            ("clip_y", &a.clip_y.to_le_bytes()),
            ("clip_w", &a.clip_w.to_le_bytes()),
            ("clip_h", &a.clip_h.to_le_bytes()),
            ("gx0", &a.gx0.to_le_bytes()),
            ("gy0", &a.gy0.to_le_bytes()),
            ("bin_x0", &a.bin_x0.to_le_bytes()),
            ("bin_y0", &a.bin_y0.to_le_bytes()),
            ("nbx", &a.nbx.to_le_bytes()),
            ("bx0", &a.bx0.to_le_bytes()),
            ("by0", &a.by0.to_le_bytes()),
            ("bsw", &a.bsw.to_le_bytes()),
            ("bsh", &a.bsh.to_le_bytes()),
        ])?;
        self.c.submit_dispatch(self.hw, &sg::COMPOSE, &push, grid).map(|_| ())
    }

    fn blur_down(&mut self, g0: grid) -> Result<(), i32> {
        let (a, grid) = gpu_scene::scene::blur_down(sg::WIDTH, &self.target, self.c.planes.va as u64, self.width,
                                                    self.height, g0);
        let push = push_words(&sg::BLUR_DOWN, &[
            ("target", &a.target.to_le_bytes()),
            ("planes", &a.planes.to_le_bytes()),
            ("pitch_px", &a.pitch_px.to_le_bytes()),
            ("width", &a.width.to_le_bytes()),
            ("height", &a.height.to_le_bytes()),
            ("bx0", &a.bx0.to_le_bytes()),
            ("by0", &a.by0.to_le_bytes()),
            ("sw", &a.sw.to_le_bytes()),
            ("sh", &a.sh.to_le_bytes()),
        ])?;
        self.c.submit_dispatch(self.hw, &sg::BLUR_DOWN, &push, grid).map(|_| ())
    }

    fn blur_box(&mut self, to_tmp: bool, lines: u32, len: u32, stride: u32, step: u32) -> Result<(), i32> {
        let (src, dst) = if to_tmp { (self.c.planes.va, self.c.tmp.va) } else { (self.c.tmp.va, self.c.planes.va) };
        let (a, grid) = gpu_scene::scene::blur_box(sg::WIDTH, src as u64, dst as u64, lines, len, stride, step,
                                                   self.radius);
        let push = push_words(&sg::BLUR_BOX, &[
            ("src", &a.src.to_le_bytes()),
            ("dst", &a.dst.to_le_bytes()),
            ("lines", &a.lines.to_le_bytes()),
            ("len", &a.len.to_le_bytes()),
            ("stride", &a.stride.to_le_bytes()),
            ("step", &a.step.to_le_bytes()),
            ("radius", &a.radius.to_le_bytes()),
        ])?;
        self.c.submit_dispatch(self.hw, &sg::BLUR_BOX, &push, grid).map(|_| ())
    }
}
