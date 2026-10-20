//! The gpudev scene executor (docs/plan/GPU_STACK.md §3.3 v1, `SCENE`) on
//! the render engine: the scene checked and planned by gpu_scene, run by
//! the programs compose.comp, blur_down.comp and blur_box.comp in one
//! batch. Not a Linux file.

use crate::gpgpu;
use crate::gpgpu_gen as g;
use crate::i915_drv::drm_i915_private;
use crate::linux::{self, Line};
use gpu_scene::plan::{self, EMPTY, bin_table, grid, item_shape, rect};
use crate::intel_gt::intel_gt;
use crate::regs::*;
use npk_sys::Dma;

const ENOMEM: i32 = 12;
const EIO: i32 = 5;
const PAGE: u32 = 4096;

const SCENE_PAGES: u32 = 16;
/// Bin tables of one batch: per bin a header, then the item lists.
const BINS_PAGES: u32 = 128;
const BINS_WORDS: u32 = BINS_PAGES * PAGE / 4;
/// Blocks one plane buffer holds: the blur grid of the largest panel.
const PLANE_PAGES: u32 = 128;
const PLANE_BLOCKS: u32 = PLANE_PAGES * PAGE / 4;

const SIMD_W: u32 = 16;

pub use gpu_scene::scene::{buffers, surface};

pub struct composer {
    scene: Dma,
    scene_va: u64,
    planes: Dma,
    planes_va: u64,
    tmp_va: u64,
    bins: Dma,
    bins_va: u64,
}

/// The scene buffer, the two blur plane buffers and the bin tables, bound
/// into the kernel PPGTT.
pub fn composer_init(gt: &mut intel_gt) -> Option<composer> {
    let scene = Dma::alloc(SCENE_PAGES as i32)?;
    let planes = Dma::alloc(PLANE_PAGES as i32)?;
    let tmp = Dma::alloc(PLANE_PAGES as i32)?;
    let bins = Dma::alloc(BINS_PAGES as i32)?;
    let bind = |gt: &mut intel_gt, d: Dma, pages: u32| {
        crate::gpudev::ppgtt_bind(gt, d.phys, (pages * PAGE) as u64, TGL_PAT_CACHE_LLC).ok()
    };
    let scene_va = bind(gt, scene, SCENE_PAGES)?;
    let planes_va = bind(gt, planes, PLANE_PAGES)?;
    let tmp_va = bind(gt, tmp, PLANE_PAGES)?;
    let bins_va = bind(gt, bins, BINS_PAGES)?;
    Some(composer { scene, scene_va, planes, planes_va, tmp_va, bins, bins_va })
}

fn put(push: &mut [u8], (off, size): (usize, usize), v: u64) {
    push[off..off + size].copy_from_slice(&v.to_le_bytes()[..size]);
}

/// Reads `out.len()` bytes of the job's payload from `off`.
fn job_bytes(seq: u32, off: u32, out: &mut [u8]) -> Result<(), i32> {
    if npk_sys::gpudev_job_data(seq as i32, off as i32, out) != out.len() as i32 {
        return Err(-EIO);
    }
    Ok(())
}

/// The job's payload, for gpu_scene.
struct job_payload(u32);

impl gpu_scene::scene::payload for job_payload {
    fn read(&self, off: u32, out: &mut [u8]) -> Result<(), i32> {
        job_bytes(self.0, off, out)
    }
}

/// The composer's scene buffer, for gpu_scene.
struct scene_mem<'a>(&'a composer);

impl gpu_scene::scene::scene_buffer for scene_mem<'_> {
    fn va(&self) -> u64 {
        self.0.scene_va
    }

    fn bytes(&self) -> u32 {
        SCENE_PAGES * PAGE
    }

    fn write(&mut self, off: u32, data: &[u8]) -> bool {
        self.0.scene.write(off, data)
    }
}

/// Runs the scene of job `seq` (`bytes` of payload) into `target`.
pub fn submit_scene(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu::gpgpu, c: &composer, bufs: &dyn buffers,
                    seq: u32, target_id: u32, bytes: u32) -> Result<scene_inflight, i32> {
    let ck = gpu_scene::scene::check_scene(&job_payload(seq), bytes, bufs, &mut scene_mem(c), target_id)?;
    let (screen, target, width, height, radius) = (ck.screen, ck.target, ck.width, ck.height, ck.radius);
    let kept = ck.shapes.len() as u32;

    // The batch.
    let mut log = dispatch_log::default();
    let mut s = gpu_sink { gp, b: gp.begin(gt)?, c, target, width, height, radius, log: &mut log };
    let lim = plan::limits { bins_words: BINS_WORDS, plane_blocks: PLANE_BLOCKS };
    plan::plan(&mut s, &lim, &screen, &ck.damage, &ck.shapes, radius)?;
    let mut b = s.b;
    let t0 = linux::now_us();
    let f = gpgpu::gpgpu_submit(i915, gt, gp, &mut b)?;
    let shapes = ck.shapes;
    Ok(scene_inflight { f, b, log, shapes, items: kept, t0 })
}

/// A scene on the render engine (`submit_scene`): its batch, buffers and
/// the composer's scene block stay in use until `poll_scene` reports it.
pub struct scene_inflight {
    f: gpgpu::inflight,
    b: gpgpu::batch,
    log: dispatch_log,
    shapes: alloc::vec::Vec<item_shape>,
    items: u32,
    t0: u64,
}

/// The report of scene `s` once it is done, None while it runs.
pub fn poll_scene(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu::gpgpu, s: &mut scene_inflight)
                  -> Option<Result<scene_report, i32>> {
    let (log, shapes) = (s.log, &s.shapes);
    let r = gpgpu::gpgpu_poll(i915, gt, gp, &mut s.f, &s.b, &mut || {
        log.say_items(shapes);
        log.say(None);
    })?;
    let us = linux::now_us() - s.t0;
    Some(r.map(|()| {
        let mut log = s.log;
        log.n_ticks = gp.dispatch_ticks(&s.b, &mut log.ticks);
        scene_report { items: s.items, dispatches: log.n, gpu_us: us, log }
    }))
}

/// What one scene did, for the log.
pub struct scene_report {
    pub items: u32,
    pub dispatches: u32,
    pub gpu_us: u64,
    pub log: dispatch_log,
}

const LOGGED: usize = 64;

/// The dispatches of a batch: program, area, and for a compose the list
/// entries over all bins.
#[derive(Clone, Copy)]
pub struct dispatch_log {
    n: u32,
    kernel: [u8; LOGGED],
    area: [rect; LOGGED],
    listed: [u32; LOGGED],
    ticks: [u64; gpgpu::MAX_DISPATCHES as usize],
    n_ticks: usize,
}

impl Default for dispatch_log {
    fn default() -> Self {
        Self { n: 0, kernel: [0; LOGGED], area: [EMPTY; LOGGED], listed: [0; LOGGED],
               ticks: [0; gpgpu::MAX_DISPATCHES as usize], n_ticks: 0 }
    }
}

impl dispatch_log {
    fn push(&mut self, k: usize, area: &rect, listed: u32) {
        if (self.n as usize) < LOGGED {
            self.kernel[self.n as usize] = k as u8;
            self.area[self.n as usize] = *area;
            self.listed[self.n as usize] = listed;
        }
        self.n += 1;
    }

    /// One line per dispatch; with the GT clock, its GPU time.
    pub fn say(&self, clock_hz: Option<u32>) {
        for i in 0..(self.n as usize).min(LOGGED) {
            let a = self.area[i];
            let mut l = Line::new();
            l.s("gpudev:   ").s(match self.kernel[i] as usize {
                gpgpu::K_COMPOSE => "compose",
                gpgpu::K_BLUR_DOWN => "blur_down",
                gpgpu::K_BLUR_BOX => "blur_box",
                _ => "?",
            }).s(" ").dec(a.w as u64).s("x").dec(a.h as u64).s("+").dec(a.x as u64).s("+").dec(a.y as u64);
            if self.kernel[i] as usize == gpgpu::K_COMPOSE {
                l.s(", ").dec(self.listed[i] as u64).s(" list entries");
            }
            match clock_hz {
                Some(hz) if i < self.n_ticks && self.ticks[i] != u64::MAX => {
                    l.s(": ").dec(self.ticks[i] * 1_000_000 / hz.max(1) as u64).s(" us");
                }
                Some(_) => {
                    l.s(": not finished");
                }
                None => {}
            }
            l.say();
        }
    }

    fn say_items(&self, items: &[item_shape]) {
        for (i, it) in items.iter().enumerate() {
            let r = it.reach;
            Line::new().s("gpudev:   item ").dec(i as u64).s(" kind ").dec(it.kind as u64).s(" reach ")
                .dec(r.w as u64).s("x").dec(r.h as u64).s("+").dec(r.x as u64).s("+").dec(r.y as u64).say();
        }
    }
}

/// Emits plan.rs's steps into one batch.
struct gpu_sink<'a> {
    gp: &'a gpgpu::gpgpu,
    b: gpgpu::batch,
    c: &'a composer,
    target: surface,
    width: u32,
    height: u32,
    radius: u32,
    log: &'a mut dispatch_log,
}

impl plan::sink for gpu_sink<'_> {
    fn bins(&mut self, at: u32, words: &[u32]) -> Result<(), i32> {
        let mut buf = [0u8; PAGE as usize];
        for (i, chunk) in words.chunks(PAGE as usize / 4).enumerate() {
            for (k, v) in chunk.iter().enumerate() {
                buf[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            if !self.c.bins.write((at + (i * PAGE as usize / 4) as u32) * 4, &buf[..chunk.len() * 4]) {
                return Err(-ENOMEM);
            }
        }
        Ok(())
    }

    fn compose(&mut self, area: &rect, t: &bin_table, blur: Option<grid>) -> Result<(), i32> {
        let (a, grid) = gpu_scene::scene::compose(SIMD_W, self.c.scene_va, self.c.planes_va,
                                                  self.c.bins_va + t.at as u64 * 4, area, t, blur);
        let mut push = [0u8; 128];
        put(&mut push, g::COMPOSE_PUSH_FRAME, a.frame);
        put(&mut push, g::COMPOSE_PUSH_PLANES, a.planes);
        put(&mut push, g::COMPOSE_PUSH_BINS, a.bins);
        put(&mut push, g::COMPOSE_PUSH_CLIP_X, a.clip_x as u64);
        put(&mut push, g::COMPOSE_PUSH_CLIP_Y, a.clip_y as u64);
        put(&mut push, g::COMPOSE_PUSH_CLIP_W, a.clip_w as u64);
        put(&mut push, g::COMPOSE_PUSH_CLIP_H, a.clip_h as u64);
        put(&mut push, g::COMPOSE_PUSH_GX0, a.gx0 as u64);
        put(&mut push, g::COMPOSE_PUSH_GY0, a.gy0 as u64);
        put(&mut push, g::COMPOSE_PUSH_BIN_X0, a.bin_x0 as u64);
        put(&mut push, g::COMPOSE_PUSH_BIN_Y0, a.bin_y0 as u64);
        put(&mut push, g::COMPOSE_PUSH_NBX, a.nbx as u64);
        put(&mut push, g::COMPOSE_PUSH_BX0, a.bx0 as u64);
        put(&mut push, g::COMPOSE_PUSH_BY0, a.by0 as u64);
        put(&mut push, g::COMPOSE_PUSH_BSW, a.bsw as u64);
        put(&mut push, g::COMPOSE_PUSH_BSH, a.bsh as u64);
        self.gp.dispatch(&mut self.b, gpgpu::K_COMPOSE, &push[..g::COMPOSE.push_size as usize], grid)?;
        self.log.push(gpgpu::K_COMPOSE, area, t.listed);
        Ok(())
    }

    fn blur_down(&mut self, g0: grid) -> Result<(), i32> {
        let (a, grid) = gpu_scene::scene::blur_down(SIMD_W, &self.target, self.c.planes_va, self.width, self.height, g0);
        let mut push = [0u8; 64];
        put(&mut push, g::BLUR_DOWN_PUSH_TARGET, a.target);
        put(&mut push, g::BLUR_DOWN_PUSH_PLANES, a.planes);
        put(&mut push, g::BLUR_DOWN_PUSH_PITCH_PX, a.pitch_px as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_WIDTH, a.width as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_HEIGHT, a.height as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_BX0, a.bx0 as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_BY0, a.by0 as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_SW, a.sw as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_SH, a.sh as u64);
        self.gp.dispatch(&mut self.b, gpgpu::K_BLUR_DOWN, &push[..g::BLUR_DOWN.push_size as usize], grid)?;
        self.log.push(gpgpu::K_BLUR_DOWN, &rect { x: a.bx0, y: a.by0, w: a.sw, h: a.sh }, 0);
        Ok(())
    }

    fn blur_box(&mut self, to_tmp: bool, lines: u32, len: u32, stride: u32, step: u32) -> Result<(), i32> {
        let (src, dst) = if to_tmp { (self.c.planes_va, self.c.tmp_va) } else { (self.c.tmp_va, self.c.planes_va) };
        let (a, grid) = gpu_scene::scene::blur_box(SIMD_W, src, dst, lines, len, stride, step, self.radius);
        let mut push = [0u8; 64];
        put(&mut push, g::BLUR_BOX_PUSH_SRC, a.src);
        put(&mut push, g::BLUR_BOX_PUSH_DST, a.dst);
        put(&mut push, g::BLUR_BOX_PUSH_LINES, a.lines as u64);
        put(&mut push, g::BLUR_BOX_PUSH_LEN, a.len as u64);
        put(&mut push, g::BLUR_BOX_PUSH_STRIDE, a.stride as u64);
        put(&mut push, g::BLUR_BOX_PUSH_STEP, a.step as u64);
        put(&mut push, g::BLUR_BOX_PUSH_RADIUS, a.radius as u64);
        self.gp.dispatch(&mut self.b, gpgpu::K_BLUR_BOX, &push[..g::BLUR_BOX.push_size as usize], grid)?;
        self.log.push(gpgpu::K_BLUR_BOX, &rect { x: 0, y: 0, w: len, h: lines }, 0);
        Ok(())
    }
}
