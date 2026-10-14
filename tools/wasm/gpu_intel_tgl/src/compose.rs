//! The gpudev scene executor (docs/plan/GPU_STACK.md §3.3 v1, `SCENE`): a
//! scene list from the kernel, checked and rewritten with the GPU addresses
//! of its buffers, run by the programs compose.comp, blur_down.comp and
//! blur_box.comp in one batch on the render engine. Not a Linux file.
//!
//! The kernel's CPU executor is the reference (kernel/src/shade/scene.rs):
//! for each damage rectangle, every item in order. A panel blurs what lies
//! beneath it, so the list is cut at every panel: the items before it are
//! composed, then the blur of the target around the panel is computed
//! (background::blur_region: 4x4 block means, three box passes per axis),
//! then the items from the panel on.

use crate::gpgpu;
use crate::gpgpu_gen as g;
use crate::i915_drv::drm_i915_private;
use crate::linux;
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
/// Blocks one plane buffer holds: the blur grid of the largest panel.
const PLANE_PAGES: u32 = 128;
const PLANE_BLOCKS: u32 = PLANE_PAGES * PAGE / 4;

const BLUR_DOWN: u32 = 4;
const BLUR_PASSES: u32 = 3;
const SIMD_W: u32 = 16;

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

#[derive(Clone, Copy)]
struct rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl rect {
    fn x1(&self) -> u32 {
        self.x + self.w
    }

    fn y1(&self) -> u32 {
        self.y + self.h
    }

    fn intersect(&self, o: &rect) -> rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.x1().min(o.x1());
        let y1 = self.y1().min(o.y1());
        if x1 <= x0 || y1 <= y0 { rect { x: x0, y: y0, w: 0, h: 0 } } else { rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } }
    }

    fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    fn union(&self, o: &rect) -> rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        rect { x: x0, y: y0, w: self.x1().max(o.x1()) - x0, h: self.y1().max(o.y1()) - y0 }
    }

    fn grow(&self, m: u32) -> rect {
        let x0 = self.x.saturating_sub(m);
        let y0 = self.y.saturating_sub(m);
        rect { x: x0, y: y0, w: self.x1() + m - x0, h: self.y1() + m - y0 }
    }
}

const EMPTY: rect = rect { x: 0, y: 0, w: 0, h: 0 };

pub struct composer {
    scene: Dma,
    scene_va: u64,
    planes: Dma,
    planes_va: u64,
    tmp_va: u64,
}

/// The scene buffer and the two blur plane buffers, bound into the kernel
/// PPGTT.
pub fn composer_init(gt: &mut intel_gt) -> Option<composer> {
    let scene = Dma::alloc(SCENE_PAGES as i32)?;
    let planes = Dma::alloc(PLANE_PAGES as i32)?;
    let tmp = Dma::alloc(PLANE_PAGES as i32)?;
    let bind = |gt: &mut intel_gt, d: Dma, pages: u32| {
        crate::gpudev::ppgtt_bind(gt, d.phys, (pages * PAGE) as u64, TGL_PAT_CACHE_LLC).ok()
    };
    let scene_va = bind(gt, scene, SCENE_PAGES)?;
    let planes_va = bind(gt, planes, PLANE_PAGES)?;
    let tmp_va = bind(gt, tmp, PLANE_PAGES)?;
    Some(composer { scene, scene_va, planes, planes_va, tmp_va })
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
pub fn run_scene(i915: &mut drm_i915_private, gt: &mut intel_gt, gp: &gpgpu::gpgpu, c: &composer, bufs: &dyn buffers,
                 seq: u32, target_id: u32, bytes: u32) -> Result<(u32, u32, u64), i32> {
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
    let mut kinds = [0u8; (SCENE_PAGES * PAGE / ITEM_BYTES) as usize];
    // Where each kept item can paint: a segment's dispatch covers only the
    // union of its items (pixels outside keep what is beneath).
    let mut reach = [EMPTY; (SCENE_PAGES * PAGE / ITEM_BYTES) as usize];
    let mut panels = [rect { x: 0, y: 0, w: 0, h: 0 }; 64];
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
            if n_panels == panels.len() {
                return Err(-E2BIG);
            }
            panels[n_panels] = rect { x: w[1], y: w[2], w: w[3], h: w[4] };
            n_panels += 1;
        }
        kinds[kept as usize] = kind as u8;
        let r = rect { x: w[1], y: w[2], w: w[3], h: w[4] };
        reach[kept as usize] = match kind {
            KIND_BACKGROUND => rect { x: 0, y: 0, w: width, h: height },
            KIND_GLOW => r.grow(w[7]),
            _ => r,
        };
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

    // The batch: per damage rectangle, the segments between panels.
    let screen = rect { x: 0, y: 0, w: width, h: height };
    let mut b = gp.begin(gt)?;
    let mut dispatches = 0u32;
    for d in damage.iter().take(n_damage as usize) {
        let clip = d.intersect(&screen);
        if clip.is_empty() {
            continue;
        }
        let mut seg = 0u32;
        let mut seg_blur = None;
        let mut panel = 0usize;
        let segment = |b: &mut gpgpu::batch, first: u32, end: u32, blur: Option<grid>| -> Result<u32, i32> {
            let area = reach[first as usize..end as usize].iter().fold(EMPTY, |a, r| a.union(r)).intersect(&clip);
            if area.is_empty() {
                return Ok(0);
            }
            compose(gp, b, c, &area, first, end - first, blur)?;
            Ok(1)
        };
        for i in 0..kept {
            if kinds[i as usize] as u32 != KIND_PANEL {
                continue;
            }
            if i > seg {
                dispatches += segment(&mut b, seg, i, seg_blur)?;
            }
            let dst = panels[panel];
            panel += 1;
            seg_blur = if radius > 0 && !dst.intersect(&clip).is_empty() {
                dispatches += 7;
                blur_region(gp, &mut b, c, &target, width, height, &dst, radius)?
            } else {
                None
            };
            seg = i;
        }
        if kept > seg {
            dispatches += segment(&mut b, seg, kept, seg_blur)?;
        }
    }
    let t0 = linux::now_us();
    gpgpu::gpgpu_run(i915, gt, gp, &mut b)?;
    Ok((kept, dispatches, linux::now_us() - t0))
}

/// The block grid a panel's blur reads: `(bx0, by0, sw, sh)`.
type grid = (u32, u32, u32, u32);

/// One compose dispatch: items `first..first + count` over `clip`.
fn compose(gp: &gpgpu::gpgpu, b: &mut gpgpu::batch, c: &composer, clip: &rect, first: u32, count: u32,
           blur: Option<grid>) -> Result<(), i32> {
    let mut push = [0u8; 64];
    put(&mut push, g::COMPOSE_PUSH_FRAME, c.scene_va);
    put(&mut push, g::COMPOSE_PUSH_PLANES, c.planes_va);
    put(&mut push, g::COMPOSE_PUSH_CLIP_X, clip.x as u64);
    put(&mut push, g::COMPOSE_PUSH_CLIP_Y, clip.y as u64);
    put(&mut push, g::COMPOSE_PUSH_CLIP_W, clip.w as u64);
    put(&mut push, g::COMPOSE_PUSH_CLIP_H, clip.h as u64);
    put(&mut push, g::COMPOSE_PUSH_FIRST, first as u64);
    put(&mut push, g::COMPOSE_PUSH_COUNT, count as u64);
    let (bx0, by0, sw, sh) = blur.unwrap_or((0, 0, 0, 0));
    put(&mut push, g::COMPOSE_PUSH_BX0, bx0 as u64);
    put(&mut push, g::COMPOSE_PUSH_BY0, by0 as u64);
    put(&mut push, g::COMPOSE_PUSH_BSW, sw as u64);
    put(&mut push, g::COMPOSE_PUSH_BSH, sh as u64);
    let k = &g::COMPOSE;
    gp.dispatch(b, gpgpu::K_COMPOSE, &push[..k.push_size as usize], [clip.w.div_ceil(SIMD_W), clip.h, 1])
}

/// background::blur_region of `rect` in the target: block means, then the
/// box passes; the result is in the planes buffer. None where the CPU
/// returns None (the panel then sits on the pixel beneath).
fn blur_region(gp: &gpgpu::gpgpu, b: &mut gpgpu::batch, c: &composer, target: &surface, width: u32, height: u32,
               r: &rect, radius: u32) -> Result<Option<grid>, i32> {
    if r.is_empty() {
        return Ok(None);
    }
    let m = (radius * BLUR_PASSES + 2) * BLUR_DOWN;
    let bx0 = r.x.saturating_sub(m) / BLUR_DOWN;
    let by0 = r.y.saturating_sub(m) / BLUR_DOWN;
    let bx1 = (r.x1() + m).min(width).div_ceil(BLUR_DOWN);
    let by1 = (r.y1() + m).min(height).div_ceil(BLUR_DOWN);
    if bx1 <= bx0 || by1 <= by0 {
        return Ok(None);
    }
    let (sw, sh) = (bx1 - bx0, by1 - by0);
    if sw * sh > PLANE_BLOCKS {
        return Err(-E2BIG);
    }

    let mut push = [0u8; 64];
    put(&mut push, g::BLUR_DOWN_PUSH_TARGET, target.va);
    put(&mut push, g::BLUR_DOWN_PUSH_PLANES, c.planes_va);
    put(&mut push, g::BLUR_DOWN_PUSH_PITCH_PX, (target.pitch / 4) as u64);
    put(&mut push, g::BLUR_DOWN_PUSH_WIDTH, width as u64);
    put(&mut push, g::BLUR_DOWN_PUSH_HEIGHT, height as u64);
    put(&mut push, g::BLUR_DOWN_PUSH_BX0, bx0 as u64);
    put(&mut push, g::BLUR_DOWN_PUSH_BY0, by0 as u64);
    put(&mut push, g::BLUR_DOWN_PUSH_SW, sw as u64);
    put(&mut push, g::BLUR_DOWN_PUSH_SH, sh as u64);
    gp.dispatch(b, gpgpu::K_BLUR_DOWN, &push[..g::BLUR_DOWN.push_size as usize], [sw.div_ceil(SIMD_W), sh, 1])?;

    let pass = |b: &mut gpgpu::batch, src: u64, dst: u64, lines: u32, len: u32, stride: u32, step: u32| {
        let mut push = [0u8; 64];
        put(&mut push, g::BLUR_BOX_PUSH_SRC, src);
        put(&mut push, g::BLUR_BOX_PUSH_DST, dst);
        put(&mut push, g::BLUR_BOX_PUSH_LINES, lines as u64);
        put(&mut push, g::BLUR_BOX_PUSH_LEN, len as u64);
        put(&mut push, g::BLUR_BOX_PUSH_STRIDE, stride as u64);
        put(&mut push, g::BLUR_BOX_PUSH_STEP, step as u64);
        put(&mut push, g::BLUR_BOX_PUSH_RADIUS, radius as u64);
        gp.dispatch(b, gpgpu::K_BLUR_BOX, &push[..g::BLUR_BOX.push_size as usize], [lines.div_ceil(SIMD_W), 1, 1])
    };
    for _ in 0..BLUR_PASSES {
        // Columns, then rows (box_pass in compute_blur and blur_region).
        pass(b, c.planes_va, c.tmp_va, sw, sh, 1, sw)?;
        pass(b, c.tmp_va, c.planes_va, sh, sw, sw, 1)?;
    }
    Ok(Some((bx0, by0, sw, sh)))
}
