//! The gpudev scene executor (docs/plan/GPU_STACK.md §3.3 v1, `SCENE`): a
//! scene list from the kernel, checked and rewritten with the GPU addresses
//! of its buffers, run by the programs compose.comp, blur_down.comp and
//! blur_box.comp in one batch on the render engine. Not a Linux file.
//!
//! The kernel's CPU executor is the reference (kernel/src/shade/scene.rs):
//! for each damage rectangle, every item in order. plan.rs turns the
//! checked items into dispatches and bin tables; this file checks the
//! scene, writes the GPU's view of it and emits the commands.

use crate::gpgpu;
use crate::gpgpu_gen as g;
use crate::i915_drv::drm_i915_private;
use crate::linux::{self, Line};
use crate::plan::{self, EMPTY, bin_table, grid, item_shape, rect};
use crate::intel_gt::intel_gt;
use crate::regs::*;
use npk_sys::Dma;

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const E2BIG: i32 = 7;
const EIO: i32 = 5;
const PAGE: u32 = 4096;

const MAGIC: u32 = 0x314E_4353;
pub const HEADER_BYTES: u32 = 64;
const ITEM_WORDS: usize = 16;
const ITEM_BYTES: u32 = 64;
const NONE: u32 = 0xFFFF_FFFF;
const MAX_DAMAGE: usize = 64;
const MAX_PANELS: usize = 64;
/// Coordinates and sizes beyond this are refused; the programs compute in
/// 32-bit Q24.8.
const MAX_COORD: u32 = 1 << 16;

const KIND_BACKGROUND: u32 = 1;
const KIND_GLOW: u32 = 2;
const KIND_GLASS_BACKDROP: u32 = 3;
const KIND_CHROME: u32 = 4;
const KIND_CONTENT: u32 = 5;
const KIND_PANEL: u32 = 6;
const KIND_TEXT: u32 = 7;
const KIND_FILL: u32 = 8;
const KIND_CLOSE_GLYPH: u32 = 9;

/// The frame block at the start of the scene buffer, in words (compose.comp).
const F_TARGET: usize = 0;
const F_TARGET_PITCH: usize = 2;
const F_FLAGS: usize = 3;
const F_WALL: usize = 4;
const F_WALL_PITCH: usize = 6;
const F_BLUR: usize = 7;
const F_BLUR_PITCH: usize = 9;
const F_GLASS: usize = 10;
const F_GLASS_PITCH: usize = 12;
const F_INK_CEIL: usize = 13;
const F_INK_TINT: usize = 14;
const F_INK_TINT_W: usize = 15;
const F_WIDTH: usize = 16;
const F_HEIGHT: usize = 17;
const F_GLYPHS: usize = 18;
const FRAME_BYTES: u32 = 128;
const FLAG_WALL: u32 = 1;
const FLAG_BLURRED: u32 = 2;
const FLAG_GLASS: u32 = 4;

/// Header flags of the scene (GPU_STACK.md §3.3).
const HDR_WALL: u32 = 1;
const HDR_BLURRED: u32 = 2;

const SCENE_PAGES: u32 = 16;
/// Items a scene buffer holds after its frame block.
const MAX_ITEMS: usize = ((SCENE_PAGES * PAGE - FRAME_BYTES) / ITEM_BYTES) as usize;
/// Bin tables of one batch: per bin a header, then the item lists.
const BINS_PAGES: u32 = 128;
const BINS_WORDS: u32 = BINS_PAGES * PAGE / 4;
/// Blocks one plane buffer holds: the blur grid of the largest panel.
const PLANE_PAGES: u32 = 128;
const PLANE_BLOCKS: u32 = PLANE_PAGES * PAGE / 4;

const SIMD_W: u32 = 16;
/// Rows a compose thread runs (compose.comp ROWS).
const ROWS: u32 = 16;

/// A buffer as a scene reads or writes it.
#[derive(Clone, Copy)]
pub struct surface {
    /// GPU address of the first pixel.
    pub va: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    /// Bytes from the first pixel.
    pub size: u64,
}

impl surface {
    /// `w` x `h` pixels from the first fit.
    fn holds(&self, w: u32, h: u32) -> bool {
        if w == 0 || h == 0 || self.pitch % 4 != 0 || (self.pitch as u64) < w as u64 * 4 {
            return false;
        }
        (h as u64 - 1) * self.pitch as u64 + w as u64 * 4 <= self.size
    }
}


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

fn set64(f: &mut [u32], w: usize, v: u64) {
    f[w] = v as u32;
    f[w + 1] = (v >> 32) as u32;
}

fn rd(b: &[u8], w: usize) -> u32 {
    u32::from_le_bytes(b[w * 4..w * 4 + 4].try_into().unwrap())
}

/// Reads `out.len()` bytes of the job's payload from `off`.
fn job_bytes(seq: u32, off: u32, out: &mut [u8]) -> Result<(), i32> {
    if npk_sys::gpudev_job_data(seq as i32, off as i32, out) != out.len() as i32 {
        return Err(-EIO);
    }
    Ok(())
}

/// What the items of one scene read, looked up by kernel buffer id.
pub trait buffers {
    fn surface(&self, id: u32) -> Option<surface>;
}

/// Runs the scene of job `seq` (`bytes` of payload) into `target`.
pub fn submit_scene(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu::gpgpu, c: &composer, bufs: &dyn buffers,
                    seq: u32, target_id: u32, bytes: u32) -> Result<scene_inflight, i32> {
    let mut hdr = [0u8; HEADER_BYTES as usize];
    if bytes < HEADER_BYTES {
        return Err(-EINVAL);
    }
    job_bytes(seq, 0, &mut hdr)?;
    if rd(&hdr, 0) != MAGIC {
        return Err(-EINVAL);
    }
    let (n_items, n_damage, glyph_bytes) = (rd(&hdr, 1), rd(&hdr, 2), rd(&hdr, 3));
    if n_damage as usize > MAX_DAMAGE {
        return Err(-E2BIG);
    }
    let glyph_padded = glyph_bytes.checked_add(3).ok_or(-EINVAL)? & !3;
    let need = (HEADER_BYTES as u64) + n_damage as u64 * 16 + n_items as u64 * ITEM_BYTES as u64
        + glyph_padded as u64;
    if need > bytes as u64 {
        return Err(-EINVAL);
    }
    let items_at = FRAME_BYTES;
    let glyphs_at = items_at as u64 + n_items as u64 * ITEM_BYTES as u64;
    if glyphs_at + glyph_padded as u64 > (SCENE_PAGES * PAGE) as u64 {
        return Err(-E2BIG);
    }
    let (width, height) = (rd(&hdr, 12), rd(&hdr, 13));
    if width == 0 || height == 0 || width > MAX_COORD || height > MAX_COORD {
        return Err(-EINVAL);
    }

    // The frame block.
    let target = bufs.surface(target_id).ok_or(-EINVAL)?;
    if target.width != width || target.height != height || !target.holds(width, height) {
        return Err(-EINVAL);
    }
    let screen_src = |id: u32| -> Result<Option<surface>, i32> {
        if id == NONE {
            return Ok(None);
        }
        let s = bufs.surface(id).ok_or(-EINVAL)?;
        if s.width < width || s.height < height || !s.holds(width, height) {
            return Err(-EINVAL);
        }
        Ok(Some(s))
    };
    let hflags = rd(&hdr, 4);
    let wall = screen_src(rd(&hdr, 5))?;
    let blur = screen_src(rd(&hdr, 6))?;
    let glass = screen_src(rd(&hdr, 7))?;
    let radius = rd(&hdr, 8);
    if radius > 64 {
        return Err(-EINVAL);
    }
    let mut frame = [0u32; FRAME_BYTES as usize / 4];
    set64(&mut frame, F_TARGET, target.va);
    frame[F_TARGET_PITCH] = target.pitch / 4;
    let mut flags = 0;
    if let (true, Some(s)) = (hflags & HDR_WALL != 0, wall) {
        flags |= FLAG_WALL;
        set64(&mut frame, F_WALL, s.va);
        frame[F_WALL_PITCH] = s.pitch / 4;
    }
    if hflags & HDR_BLURRED != 0 {
        let s = blur.ok_or(-EINVAL)?;
        flags |= FLAG_BLURRED;
        set64(&mut frame, F_BLUR, s.va);
        frame[F_BLUR_PITCH] = s.pitch / 4;
    }
    if let Some(s) = glass {
        flags |= FLAG_GLASS;
        set64(&mut frame, F_GLASS, s.va);
        frame[F_GLASS_PITCH] = s.pitch / 4;
    }
    frame[F_FLAGS] = flags;
    frame[F_INK_CEIL] = rd(&hdr, 9);
    frame[F_INK_TINT] = rd(&hdr, 10);
    frame[F_INK_TINT_W] = rd(&hdr, 11);
    frame[F_WIDTH] = width;
    frame[F_HEIGHT] = height;
    set64(&mut frame, F_GLYPHS, c.scene_va + glyphs_at);
    let mut fb = [0u8; FRAME_BYTES as usize];
    for (i, v) in frame.iter().enumerate() {
        fb[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    if !c.scene.write(0, &fb) {
        return Err(-ENOMEM);
    }

    // Damage.
    let mut damage = [rect { x: 0, y: 0, w: 0, h: 0 }; MAX_DAMAGE];
    let mut db = [0u8; MAX_DAMAGE * 16];
    job_bytes(seq, HEADER_BYTES, &mut db[..n_damage as usize * 16])?;
    for (i, d) in damage.iter_mut().take(n_damage as usize).enumerate() {
        let w = |k: usize| rd(&db, i * 4 + k);
        let r = rect { x: w(0), y: w(1), w: w(2), h: w(3) };
        if r.x > MAX_COORD || r.y > MAX_COORD || r.w > MAX_COORD || r.h > MAX_COORD {
            return Err(-EINVAL);
        }
        *d = r;
    }

    // Items, rewritten with the sources' GPU addresses. An item whose
    // source has no pixels paints nothing on the CPU and is dropped.
    let screen = rect { x: 0, y: 0, w: width, h: height };
    let mut shapes = [item_shape { kind: 0, reach: EMPTY, cover: (EMPTY, EMPTY) }; MAX_ITEMS];
    let mut n_panels = 0usize;
    let mut kept = 0u32;
    let items_off = HEADER_BYTES + n_damage * 16;
    let mut chunk = [0u8; PAGE as usize];
    let mut in_chunk = 0usize;
    let mut chunk_at = items_at;
    for i in 0..n_items {
        let mut raw = [0u8; ITEM_BYTES as usize];
        job_bytes(seq, items_off + i * ITEM_BYTES, &mut raw)?;
        let mut w = [0u32; ITEM_WORDS];
        for (k, v) in w.iter_mut().enumerate() {
            *v = rd(&raw, k);
        }
        let kind = w[0];
        if !(KIND_BACKGROUND..=KIND_CLOSE_GLYPH).contains(&kind) {
            return Err(-EINVAL);
        }
        if kind != KIND_BACKGROUND && w[1..5].iter().any(|&v| v > MAX_COORD) {
            return Err(-EINVAL);
        }
        let src = |id: u32| -> Result<Option<surface>, i32> {
            let s = bufs.surface(id).ok_or(-EINVAL)?;
            if s.width == 0 || s.height == 0 {
                return Ok(None);
            }
            if !s.holds(s.width, s.height) {
                return Err(-EINVAL);
            }
            Ok(Some(s))
        };
        let mut out = w;
        match kind {
            KIND_GLOW => {
                if w[5] > MAX_COORD || w[7] > 4096 || w[8] > 65536 {
                    return Err(-EINVAL);
                }
            }
            KIND_GLASS_BACKDROP => {
                if w[5] > MAX_COORD {
                    return Err(-EINVAL);
                }
            }
            KIND_CHROME => {
                if w[8] > MAX_COORD || w[9] > MAX_COORD {
                    return Err(-EINVAL);
                }
            }
            KIND_CONTENT => {
                if w[5] > MAX_COORD || w[6] > 1 {
                    return Err(-EINVAL);
                }
                let Some(s) = src(w[7])? else { continue };
                out[7] = s.va as u32;
                out[8] = (s.va >> 32) as u32;
                out[9] = s.width;
                out[10] = s.height;
                out[11] = s.pitch / 4;
            }
            KIND_PANEL | KIND_TEXT => {
                let Some(s) = src(w[5])? else { continue };
                out[5] = s.va as u32;
                out[6] = (s.va >> 32) as u32;
                out[7] = s.width;
                out[8] = s.height;
                out[9] = s.pitch / 4;
                out[10..].fill(0);
            }
            KIND_FILL => {
                if w[7] > MAX_COORD {
                    return Err(-EINVAL);
                }
            }
            KIND_CLOSE_GLYPH => {
                let (asz, off) = (w[5], w[7]);
                if asz > 256 || off as u64 + (asz as u64 * asz as u64) > glyph_bytes as u64 {
                    return Err(-EINVAL);
                }
            }
            _ => {}
        }
        if kind == KIND_PANEL {
            if n_panels == MAX_PANELS {
                return Err(-E2BIG);
            }
            n_panels += 1;
        }
        shapes[kept as usize] = item_shape::of(&w, &screen, glass.is_some());
        for (k, v) in out.iter().enumerate() {
            chunk[in_chunk + k * 4..in_chunk + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        in_chunk += ITEM_BYTES as usize;
        kept += 1;
        if in_chunk == chunk.len() {
            if !c.scene.write(chunk_at, &chunk) {
                return Err(-ENOMEM);
            }
            chunk_at += in_chunk as u32;
            in_chunk = 0;
        }
    }
    if in_chunk > 0 && !c.scene.write(chunk_at, &chunk[..in_chunk]) {
        return Err(-ENOMEM);
    }

    // Glyphs, after the items as the frame block says.
    let glyph_src = items_off + n_items * ITEM_BYTES;
    let mut off = 0;
    while off < glyph_padded {
        let n = (glyph_padded - off).min(PAGE);
        job_bytes(seq, glyph_src + off, &mut chunk[..n as usize])?;
        if !c.scene.write(glyphs_at as u32 + off, &chunk[..n as usize]) {
            return Err(-ENOMEM);
        }
        off += n;
    }

    // The batch.
    let mut log = dispatch_log::default();
    let mut s = gpu_sink { gp, b: gp.begin(gt)?, c, target, width, height, radius, log: &mut log };
    let lim = plan::limits { bins_words: BINS_WORDS, plane_blocks: PLANE_BLOCKS };
    plan::plan(&mut s, &lim, &screen, &damage[..n_damage as usize], &shapes[..kept as usize], radius)?;
    let mut b = s.b;
    let t0 = linux::now_us();
    let f = gpgpu::gpgpu_submit(i915, gt, gp, &mut b)?;
    let shapes = shapes[..kept as usize].to_vec();
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
        let mut push = [0u8; 128];
        put(&mut push, g::COMPOSE_PUSH_FRAME, self.c.scene_va);
        put(&mut push, g::COMPOSE_PUSH_PLANES, self.c.planes_va);
        put(&mut push, g::COMPOSE_PUSH_BINS, self.c.bins_va + t.at as u64 * 4);
        put(&mut push, g::COMPOSE_PUSH_CLIP_X, area.x as u64);
        put(&mut push, g::COMPOSE_PUSH_CLIP_Y, area.y as u64);
        put(&mut push, g::COMPOSE_PUSH_CLIP_W, area.w as u64);
        put(&mut push, g::COMPOSE_PUSH_CLIP_H, area.h as u64);
        put(&mut push, g::COMPOSE_PUSH_GX0, (area.x / SIMD_W) as u64);
        put(&mut push, g::COMPOSE_PUSH_GY0, (area.y / ROWS) as u64);
        put(&mut push, g::COMPOSE_PUSH_BIN_X0, t.bin_x0 as u64);
        put(&mut push, g::COMPOSE_PUSH_BIN_Y0, t.bin_y0 as u64);
        put(&mut push, g::COMPOSE_PUSH_NBX, t.nbx as u64);
        let (bx0, by0, sw, sh) = blur.unwrap_or((0, 0, 0, 0));
        put(&mut push, g::COMPOSE_PUSH_BX0, bx0 as u64);
        put(&mut push, g::COMPOSE_PUSH_BY0, by0 as u64);
        put(&mut push, g::COMPOSE_PUSH_BSW, sw as u64);
        put(&mut push, g::COMPOSE_PUSH_BSH, sh as u64);
        let cols = area.x1().div_ceil(SIMD_W) - area.x / SIMD_W;
        let runs = area.y1().div_ceil(ROWS) - area.y / ROWS;
        self.gp.dispatch(&mut self.b, gpgpu::K_COMPOSE, &push[..g::COMPOSE.push_size as usize], [cols, runs, 1])?;
        self.log.push(gpgpu::K_COMPOSE, area, t.listed);
        Ok(())
    }

    fn blur_down(&mut self, (bx0, by0, sw, sh): grid) -> Result<(), i32> {
        let mut push = [0u8; 64];
        put(&mut push, g::BLUR_DOWN_PUSH_TARGET, self.target.va);
        put(&mut push, g::BLUR_DOWN_PUSH_PLANES, self.c.planes_va);
        put(&mut push, g::BLUR_DOWN_PUSH_PITCH_PX, (self.target.pitch / 4) as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_WIDTH, self.width as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_HEIGHT, self.height as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_BX0, bx0 as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_BY0, by0 as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_SW, sw as u64);
        put(&mut push, g::BLUR_DOWN_PUSH_SH, sh as u64);
        self.gp.dispatch(&mut self.b, gpgpu::K_BLUR_DOWN, &push[..g::BLUR_DOWN.push_size as usize],
                         [sw.div_ceil(SIMD_W), sh, 1])?;
        self.log.push(gpgpu::K_BLUR_DOWN, &rect { x: bx0, y: by0, w: sw, h: sh }, 0);
        Ok(())
    }

    fn blur_box(&mut self, to_tmp: bool, lines: u32, len: u32, stride: u32, step: u32) -> Result<(), i32> {
        let (src, dst) = if to_tmp { (self.c.planes_va, self.c.tmp_va) } else { (self.c.tmp_va, self.c.planes_va) };
        let mut push = [0u8; 64];
        put(&mut push, g::BLUR_BOX_PUSH_SRC, src);
        put(&mut push, g::BLUR_BOX_PUSH_DST, dst);
        put(&mut push, g::BLUR_BOX_PUSH_LINES, lines as u64);
        put(&mut push, g::BLUR_BOX_PUSH_LEN, len as u64);
        put(&mut push, g::BLUR_BOX_PUSH_STRIDE, stride as u64);
        put(&mut push, g::BLUR_BOX_PUSH_STEP, step as u64);
        put(&mut push, g::BLUR_BOX_PUSH_RADIUS, self.radius as u64);
        self.gp.dispatch(&mut self.b, gpgpu::K_BLUR_BOX, &push[..g::BLUR_BOX.push_size as usize],
                         [len.div_ceil(SIMD_W), lines, 1])?;
        self.log.push(gpgpu::K_BLUR_BOX, &rect { x: 0, y: 0, w: len, h: lines }, 0);
        Ok(())
    }
}
