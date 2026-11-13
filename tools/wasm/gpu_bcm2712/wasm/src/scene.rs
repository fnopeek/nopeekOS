//! The gpudev scene executor on V3D: the scene checked and planned by
//! gpu_scene, run by compose.comp, blur_down.comp and blur_box.comp as v3dv
//! compiled them (`shaders_gen`), one CSD job per dispatch the way
//! `v3d_csd_job_run` starts it, each waited for and the caches cleaned
//! after it (`gpu_bcm2712_core::v3d`). Not a Linux file.
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
use gpu_scene::plan::{self, bin_table, grid, rect};

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const EIO: i32 = 5;
const EFAULT: i32 = 14;
const ETIMEDOUT: i32 = 110;
const PAGE: u32 = 4096;

const SCENE_PAGES: u32 = 16;
const BINS_PAGES: u32 = 128;
const BINS_WORDS: u32 = BINS_PAGES * PAGE / 4;
const PLANE_PAGES: u32 = 128;
const PLANE_BLOCKS: u32 = PLANE_PAGES * PAGE / 4;
/// Programs start at this alignment in the code buffer.
const CODE_ALIGN: u32 = 256;
/// How long one job may run.
const JOB_TIMEOUT_MS: u32 = 1000;

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
}

/// What the module knows of V3D and its programs.
pub struct Composer {
    qpus: u32,
    rev: u32,
    code: Buf,
    /// Per `sg::PROGRAMS` entry, its V3D address.
    code_va: Vec<u32>,
    uniforms: Buf,
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
    let spill_size = sg::PROGRAMS.iter().map(|p| p.spill_size).max().unwrap_or(0);
    // pipeline_check_spill_size: the thread index picks the area, (core <<
    // 6) | (qpu << 2) | thread, so four threads per QPU at any thread count.
    let spill = if spill_size > 0 { Some(Buf::new((4 * qpus * spill_size).div_ceil(PAGE))?) } else { None };
    let c = Composer {
        qpus,
        rev,
        code_va: offs.iter().map(|&o| code.va + o).collect(),
        code,
        uniforms: Buf::new((max_uniforms * 4).div_ceil(PAGE))?,
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
    /// One dispatch of `p` over `grid` with `push`: the uniform stream
    /// filled as v3dv_write_uniforms_wg_offsets fills it, the CSD job run
    /// and waited for, the caches cleaned.
    pub fn dispatch(&mut self, hw: &mut PiHw, p: &Program, push: &[u32; 32], grid: [u32; 3]) -> Result<(), i32> {
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
        if !self.uniforms.write(0, &stream) {
            return Err(-ENOMEM);
        }
        let cp = CsdProgram {
            threads: p.threads,
            single_seg: p.single_seg,
            local_size: p.local_size,
            can_use_supergroups: p.can_use_supergroups,
            has_control_barrier: p.has_control_barrier,
        };
        let cfg = v3d::csd_cfg(&cp, self.rev, self.qpus, grid, self.code_va[program_index(p)], self.uniforms.va);
        let hub = hw.irq_arm(Spi::V3dHub);
        let mut since = hw.irq_arm(Spi::V3dCore);
        let t0 = npk_sys::now_us() as u64;
        if !v3d::csd_job_run(hw, &cfg) {
            return Ok(());
        }
        self.dispatches += 1;
        loop {
            if !hw.irq_wait(Spi::V3dCore, since, JOB_TIMEOUT_MS) {
                Line::new().s("v3d: ").s(p.name).s(" did not finish").say();
                return Err(-ETIMEDOUT);
            }
            since = hw.irq_arm(Spi::V3dCore);
            if v3d::core_irq(hw).csd_done {
                break;
            }
        }
        let clean = v3d::clean_caches(hw);
        if hw.irq_arm(Spi::V3dHub) != hub {
            let h = v3d::hub_irq(hw);
            if h.mmu_fault || h.gmp_violation {
                Line::new().s("v3d: ").s(p.name).s(" faulted on an address the kernel did not map").say();
                return Err(-EFAULT);
            }
        }
        if !clean {
            return Err(-EIO);
        }
        if let Some(t) = self.trace.as_mut() {
            t.push((p.name, grid, npk_sys::now_us() as u64 - t0));
        }
        Ok(())
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
            self.dispatch(hw, &sg::FILL_PATTERN, &push, [W.div_ceil(sg::WIDTH), H, 1])?;
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
    /// checked, planned and composed. Every dispatch is done when it returns.
    pub fn run_scene(&mut self, hw: &mut PiHw, bufs: &dyn gpu_scene::scene::buffers, seq: u32, target: u32,
                     bytes: u32) -> Result<u32, i32> {
        let mut mem = SceneMem(&self.scene);
        let ck = gpu_scene::scene::check_scene(&JobPayload(seq), bytes, bufs, &mut mem, target)?;
        let before = self.dispatches;
        let lim = plan::limits { bins_words: BINS_WORDS, plane_blocks: PLANE_BLOCKS };
        let mut s = Sink { c: self, hw, target: ck.target, width: ck.width, height: ck.height, radius: ck.radius };
        plan::plan(&mut s, &lim, &ck.screen, &ck.damage, &ck.shapes, ck.radius)?;
        Ok((self.dispatches - before) as u32)
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
        self.c.dispatch(self.hw, &sg::COMPOSE, &push, grid)
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
        self.c.dispatch(self.hw, &sg::BLUR_DOWN, &push, grid)
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
        self.c.dispatch(self.hw, &sg::BLUR_BOX, &push, grid)
    }
}
