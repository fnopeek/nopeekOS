//! The scene list from the kernel, checked and rewritten for the GPU: the
//! frame block, the items with their sources' GPU addresses, the glyphs,
//! all into the module's scene buffer as compose.comp reads it. And the
//! push constants and grid of each dispatch plan.rs asks for, by the
//! module's workgroup width. Not a Linux file.
//!
//! The kernel's CPU executor is the reference (kernel/src/shade/scene.rs):
//! for each damage rectangle, every item in order.

use crate::plan::{bin_table, grid, item_shape, rect};
use alloc::vec::Vec;

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const E2BIG: i32 = 7;
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
pub const FRAME_BYTES: u32 = 128;
const FLAG_WALL: u32 = 1;
const FLAG_BLURRED: u32 = 2;
const FLAG_GLASS: u32 = 4;

/// Header flags of the scene (GPU_STACK.md §3.3).
const HDR_WALL: u32 = 1;
const HDR_BLURRED: u32 = 2;

/// Rows a compose thread runs (compose.comp ROWS).
pub const ROWS: u32 = 16;

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
    pub fn holds(&self, w: u32, h: u32) -> bool {
        if w == 0 || h == 0 || self.pitch % 4 != 0 || (self.pitch as u64) < w as u64 * 4 {
            return false;
        }
        (h as u64 - 1) * self.pitch as u64 + w as u64 * 4 <= self.size
    }
}

/// What the items of one scene read, looked up by kernel buffer id.
pub trait buffers {
    fn surface(&self, id: u32) -> Option<surface>;
}

/// The job's payload as the kernel holds it.
pub trait payload {
    /// `out.len()` bytes from `off`.
    fn read(&self, off: u32, out: &mut [u8]) -> Result<(), i32>;
}

/// The module's scene buffer: the frame block, the items, the glyphs.
pub trait scene_buffer {
    /// GPU address of its first byte.
    fn va(&self) -> u64;
    fn bytes(&self) -> u32;
    /// `data` at byte `off`; false if it does not fit.
    fn write(&mut self, off: u32, data: &[u8]) -> bool;
}

/// A checked scene, written to the scene buffer.
pub struct checked {
    pub screen: rect,
    pub width: u32,
    pub height: u32,
    pub radius: u32,
    pub target: surface,
    pub damage: Vec<rect>,
    /// The items kept, in order.
    pub shapes: Vec<item_shape>,
}

fn set64(f: &mut [u32], w: usize, v: u64) {
    f[w] = v as u32;
    f[w + 1] = (v >> 32) as u32;
}

fn rd(b: &[u8], w: usize) -> u32 {
    u32::from_le_bytes(b[w * 4..w * 4 + 4].try_into().unwrap())
}

/// The scene of a job (`bytes` of payload) into `target_id`, checked and
/// written to `mem`.
pub fn check_scene(job: &dyn payload, bytes: u32, bufs: &dyn buffers, mem: &mut dyn scene_buffer, target_id: u32)
                   -> Result<checked, i32> {
    let mut hdr = [0u8; HEADER_BYTES as usize];
    if bytes < HEADER_BYTES {
        return Err(-EINVAL);
    }
    job.read(0, &mut hdr)?;
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
    if glyphs_at + glyph_padded as u64 > mem.bytes() as u64 {
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
    set64(&mut frame, F_GLYPHS, mem.va() + glyphs_at);
    let mut fb = [0u8; FRAME_BYTES as usize];
    for (i, v) in frame.iter().enumerate() {
        fb[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    if !mem.write(0, &fb) {
        return Err(-ENOMEM);
    }

    // Damage.
    let mut damage = Vec::with_capacity(n_damage as usize);
    let mut db = [0u8; MAX_DAMAGE * 16];
    job.read(HEADER_BYTES, &mut db[..n_damage as usize * 16])?;
    for i in 0..n_damage as usize {
        let w = |k: usize| rd(&db, i * 4 + k);
        let r = rect { x: w(0), y: w(1), w: w(2), h: w(3) };
        if r.x > MAX_COORD || r.y > MAX_COORD || r.w > MAX_COORD || r.h > MAX_COORD {
            return Err(-EINVAL);
        }
        damage.push(r);
    }

    // Items, rewritten with the sources' GPU addresses. An item whose
    // source has no pixels paints nothing on the CPU and is dropped.
    let screen = rect { x: 0, y: 0, w: width, h: height };
    let mut shapes = Vec::with_capacity(n_items as usize);
    let mut n_panels = 0usize;
    let items_off = HEADER_BYTES + n_damage * 16;
    let mut chunk = [0u8; PAGE as usize];
    let mut in_chunk = 0usize;
    let mut chunk_at = items_at;
    for i in 0..n_items {
        let mut raw = [0u8; ITEM_BYTES as usize];
        job.read(items_off + i * ITEM_BYTES, &mut raw)?;
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
        shapes.push(item_shape::of(&w, &screen, glass.is_some()));
        for (k, v) in out.iter().enumerate() {
            chunk[in_chunk + k * 4..in_chunk + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        in_chunk += ITEM_BYTES as usize;
        if in_chunk == chunk.len() {
            if !mem.write(chunk_at, &chunk) {
                return Err(-ENOMEM);
            }
            chunk_at += in_chunk as u32;
            in_chunk = 0;
        }
    }
    if in_chunk > 0 && !mem.write(chunk_at, &chunk[..in_chunk]) {
        return Err(-ENOMEM);
    }

    // Glyphs, after the items as the frame block says.
    let glyph_src = items_off + n_items * ITEM_BYTES;
    let mut off = 0;
    while off < glyph_padded {
        let n = (glyph_padded - off).min(PAGE);
        job.read(glyph_src + off, &mut chunk[..n as usize])?;
        if !mem.write(glyphs_at as u32 + off, &chunk[..n as usize]) {
            return Err(-ENOMEM);
        }
        off += n;
    }
    Ok(checked { screen, width, height, radius, target, damage, shapes })
}

/// compose.comp's push constants.
pub struct compose_push {
    pub frame: u64,
    pub planes: u64,
    pub bins: u64,
    pub clip_x: u32,
    pub clip_y: u32,
    pub clip_w: u32,
    pub clip_h: u32,
    pub gx0: u32,
    pub gy0: u32,
    pub bin_x0: u32,
    pub bin_y0: u32,
    pub nbx: u32,
    pub bx0: u32,
    pub by0: u32,
    pub bsw: u32,
    pub bsh: u32,
}

/// A compose dispatch over `area` with workgroups `width` pixels wide:
/// its push constants and grid.
pub fn compose(width: u32, frame: u64, planes: u64, bins: u64, area: &rect, t: &bin_table, blur: Option<grid>)
               -> (compose_push, [u32; 3]) {
    let (bx0, by0, bsw, bsh) = blur.unwrap_or((0, 0, 0, 0));
    let p = compose_push {
        frame,
        planes,
        bins,
        clip_x: area.x,
        clip_y: area.y,
        clip_w: area.w,
        clip_h: area.h,
        gx0: area.x / width,
        gy0: area.y / ROWS,
        bin_x0: t.bin_x0,
        bin_y0: t.bin_y0,
        nbx: t.nbx,
        bx0,
        by0,
        bsw,
        bsh,
    };
    let cols = area.x1().div_ceil(width) - area.x / width;
    let runs = area.y1().div_ceil(ROWS) - area.y / ROWS;
    (p, [cols, runs, 1])
}

/// blur_down.comp's push constants.
pub struct blur_down_push {
    pub target: u64,
    pub planes: u64,
    pub pitch_px: u32,
    pub width: u32,
    pub height: u32,
    pub bx0: u32,
    pub by0: u32,
    pub sw: u32,
    pub sh: u32,
}

/// The block means of `g` in `target` (`screen_w` x `screen_h`).
pub fn blur_down(width: u32, target: &surface, planes: u64, screen_w: u32, screen_h: u32,
                 (bx0, by0, sw, sh): grid) -> (blur_down_push, [u32; 3]) {
    let p = blur_down_push {
        target: target.va,
        planes,
        pitch_px: target.pitch / 4,
        width: screen_w,
        height: screen_h,
        bx0,
        by0,
        sw,
        sh,
    };
    (p, [sw.div_ceil(width), sh, 1])
}

/// blur_box.comp's push constants.
pub struct blur_box_push {
    pub src: u64,
    pub dst: u64,
    pub lines: u32,
    pub len: u32,
    pub stride: u32,
    pub step: u32,
    pub radius: u32,
}

/// One box pass over `lines` lines of `len` blocks.
pub fn blur_box(width: u32, src: u64, dst: u64, lines: u32, len: u32, stride: u32, step: u32, radius: u32)
                -> (blur_box_push, [u32; 3]) {
    let p = blur_box_push { src, dst, lines, len, stride, step, radius };
    (p, [len.div_ceil(width), lines, 1])
}
