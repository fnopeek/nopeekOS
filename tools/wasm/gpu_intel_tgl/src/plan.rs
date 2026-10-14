//! How a checked scene becomes dispatches (compose.rs): per damage
//! rectangle the item list is cut at every panel, since a panel blurs what
//! lies beneath it (background::blur_region: 4x4 block means, three box
//! passes per axis). Each segment is composed over the union of what its
//! items reach, with a bin table: per 64x64 bin of the screen grid the
//! items that reach the bin, in order, starting at the last one that covers
//! it opaquely. No GPU access here; the caller's `sink` turns each step into
//! commands and memory writes. Not a Linux file.

pub const EINVAL: i32 = 22;
pub const E2BIG: i32 = 7;

pub const KIND_BACKGROUND: u8 = 1;
pub const KIND_GLOW: u8 = 2;
pub const KIND_CONTENT: u8 = 5;
pub const KIND_PANEL: u8 = 6;

/// A bin is 64x64 pixels.
pub const BIN: u32 = 64;
/// Bins of one compose dispatch.
pub const MAX_BINS: usize = 4096;
/// Words after a table's lists that compose.comp's 16-word loads may touch.
pub const BLOCK_SLACK: u32 = 16;
pub const BLUR_DOWN: u32 = 4;
pub const BLUR_PASSES: u32 = 3;
/// Item indices in a bin list are 16-bit.
pub const MAX_ITEMS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

pub const EMPTY: rect = rect { x: 0, y: 0, w: 0, h: 0 };

impl rect {
    pub fn x1(&self) -> u32 {
        self.x + self.w
    }

    pub fn y1(&self) -> u32 {
        self.y + self.h
    }

    pub fn intersect(&self, o: &rect) -> rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.x1().min(o.x1());
        let y1 = self.y1().min(o.y1());
        if x1 <= x0 || y1 <= y0 { rect { x: x0, y: y0, w: 0, h: 0 } } else { rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn union(&self, o: &rect) -> rect {
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

    pub fn grow(&self, m: u32) -> rect {
        let x0 = self.x.saturating_sub(m);
        let y0 = self.y.saturating_sub(m);
        rect { x: x0, y: y0, w: self.x1() + m - x0, h: self.y1() + m - y0 }
    }

    /// `inner` lies within `self`.
    pub fn contains(&self, inner: &rect) -> bool {
        !self.is_empty() && inner.x >= self.x && inner.y >= self.y && inner.x1() <= self.x1() && inner.y1() <= self.y1()
    }
}

/// Where an item paints, and where its result does not depend on what is
/// beneath (a bin inside either rectangle starts its list there).
#[derive(Clone, Copy)]
pub struct item_shape {
    pub kind: u8,
    pub reach: rect,
    pub cover: (rect, rect),
}

impl item_shape {
    /// From the item's words (kind, x, y, w, h, ...) on a `screen`.
    pub fn of(w: &[u32], screen: &rect) -> item_shape {
        let kind = w[0] as u8;
        let r = rect { x: w[1], y: w[2], w: w[3], h: w[4] };
        let reach = match kind {
            KIND_BACKGROUND => *screen,
            KIND_GLOW => r.grow(w[7]),
            _ => r,
        };
        let cover = match kind {
            KIND_BACKGROUND => (*screen, *screen),
            // scene::content writes the source pixel wherever the rounded
            // rect's coverage is 256: everywhere off the four corner squares.
            KIND_CONTENT => {
                let rr = w[5].min(r.w / 2).min(r.h / 2);
                (rect { x: r.x + rr, y: r.y, w: r.w - 2 * rr, h: r.h },
                 rect { x: r.x, y: r.y + rr, w: r.w, h: r.h - 2 * rr })
            }
            _ => (EMPTY, EMPTY),
        };
        item_shape { kind, reach, cover }
    }
}

/// A compose dispatch's bin table: per bin `{list offset, count | opaque <<
/// 31}`, offsets counted in words from the first header, then the lists.
#[derive(Clone, Copy, Debug)]
pub struct bin_table {
    /// Word of the first header in the bins buffer.
    pub at: u32,
    pub words: u32,
    pub bin_x0: u32,
    pub bin_y0: u32,
    pub nbx: u32,
    /// List entries over all bins.
    pub listed: u32,
}

/// The block grid a panel's blur reads: `(bx0, by0, sw, sh)`.
pub type grid = (u32, u32, u32, u32);

/// What the plan asks of the GPU side.
pub trait sink {
    /// Words of a bin table, at word `at` of the bins buffer.
    fn bins(&mut self, at: u32, words: &[u32]) -> Result<(), i32>;
    fn compose(&mut self, area: &rect, t: &bin_table, blur: Option<grid>) -> Result<(), i32>;
    /// Block means of the target over `g` into the planes.
    fn blur_down(&mut self, g: grid) -> Result<(), i32>;
    /// One box pass: planes into the scratch plane (`to_tmp`) or back.
    fn blur_box(&mut self, to_tmp: bool, lines: u32, len: u32, stride: u32, step: u32) -> Result<(), i32>;
}

pub struct limits {
    pub bins_words: u32,
    pub plane_blocks: u32,
}

/// The dispatches of a scene: `items` in order, `damage` already checked.
pub fn plan(s: &mut dyn sink, lim: &limits, screen: &rect, damage: &[rect], items: &[item_shape], radius: u32)
            -> Result<(), i32> {
    if items.len() > MAX_ITEMS {
        return Err(-E2BIG);
    }
    let mut bins_at = 0u32;
    for d in damage {
        let clip = d.intersect(screen);
        if clip.is_empty() {
            continue;
        }
        let mut seg = 0usize;
        let mut seg_blur = None;
        for i in 0..items.len() {
            if items[i].kind != KIND_PANEL {
                continue;
            }
            if i > seg {
                segment(s, lim, &mut bins_at, &clip, items, seg, i, seg_blur)?;
            }
            let dst = items[i].reach;
            seg_blur = if radius > 0 && !dst.intersect(&clip).is_empty() {
                blur_region(s, lim, screen, &dst, radius)?
            } else {
                None
            };
            seg = i;
        }
        if items.len() > seg {
            segment(s, lim, &mut bins_at, &clip, items, seg, items.len(), seg_blur)?;
        }
    }
    Ok(())
}

/// Items `first..end` over the part of `clip` they reach.
fn segment(s: &mut dyn sink, lim: &limits, bins_at: &mut u32, clip: &rect, items: &[item_shape], first: usize,
           end: usize, blur: Option<grid>) -> Result<(), i32> {
    let area = items[first..end].iter().fold(EMPTY, |a, it| a.union(&it.reach)).intersect(clip);
    if area.is_empty() {
        return Ok(());
    }
    let t = write_bins(s, lim, *bins_at, &area, &items[..end], first)?;
    *bins_at += t.words;
    s.compose(&area, &t, blur)
}

/// The bin table of items `first..items.len()` over `area`, at word `at`.
fn write_bins(s: &mut dyn sink, lim: &limits, at: u32, area: &rect, items: &[item_shape], first: usize)
              -> Result<bin_table, i32> {
    let bin_x0 = area.x / BIN;
    let bin_y0 = area.y / BIN;
    let nbx = area.x1().div_ceil(BIN) - bin_x0;
    let nby = area.y1().div_ceil(BIN) - bin_y0;
    let nb = (nbx * nby) as usize;
    if nb > MAX_BINS {
        return Err(-E2BIG);
    }
    let mut hdr = [0u32; MAX_BINS * 2];
    let list_at = at + nb as u32 * 2;
    let mut out = [0u32; 1024];
    let mut out_n = 0usize;
    let mut written = 0u32;
    let mut list = [0u16; MAX_ITEMS];
    let mut listed = 0u32;
    for by in 0..nby {
        for bx in 0..nbx {
            let br = rect { x: (bin_x0 + bx) * BIN, y: (bin_y0 + by) * BIN, w: BIN, h: BIN }.intersect(area);
            let mut n = 0usize;
            let mut opaque = false;
            if !br.is_empty() {
                for (i, it) in items.iter().enumerate().skip(first) {
                    if it.reach.intersect(&br).is_empty() {
                        continue;
                    }
                    if it.cover.0.contains(&br) || it.cover.1.contains(&br) {
                        n = 0;
                        opaque = true;
                    }
                    list[n] = i as u16;
                    n += 1;
                }
            }
            let bi = (by * nbx + bx) as usize;
            hdr[bi * 2] = nb as u32 * 2 + written + out_n as u32;
            hdr[bi * 2 + 1] = n as u32 | (opaque as u32) << 31;
            listed += n as u32;
            for &i in &list[..n] {
                out[out_n] = i as u32;
                out_n += 1;
                if out_n == out.len() {
                    if list_at + written + out_n as u32 + BLOCK_SLACK > lim.bins_words {
                        return Err(-E2BIG);
                    }
                    s.bins(list_at + written, &out)?;
                    written += out_n as u32;
                    out_n = 0;
                }
            }
        }
    }
    if list_at + written + out_n as u32 + BLOCK_SLACK > lim.bins_words {
        return Err(-E2BIG);
    }
    if out_n > 0 {
        s.bins(list_at + written, &out[..out_n])?;
        written += out_n as u32;
    }
    s.bins(at, &hdr[..nb * 2])?;
    Ok(bin_table { at, words: nb as u32 * 2 + written + BLOCK_SLACK, bin_x0, bin_y0, nbx, listed })
}

/// background::blur_region of `r` in the target into the planes; None
/// where the CPU returns None (the panel then sits on the pixel beneath).
fn blur_region(s: &mut dyn sink, lim: &limits, screen: &rect, r: &rect, radius: u32) -> Result<Option<grid>, i32> {
    if r.is_empty() {
        return Ok(None);
    }
    let m = (radius * BLUR_PASSES + 2) * BLUR_DOWN;
    let bx0 = r.x.saturating_sub(m) / BLUR_DOWN;
    let by0 = r.y.saturating_sub(m) / BLUR_DOWN;
    let bx1 = (r.x1() + m).min(screen.w).div_ceil(BLUR_DOWN);
    let by1 = (r.y1() + m).min(screen.h).div_ceil(BLUR_DOWN);
    if bx1 <= bx0 || by1 <= by0 {
        return Ok(None);
    }
    let (sw, sh) = (bx1 - bx0, by1 - by0);
    if sw * sh > lim.plane_blocks {
        return Err(-E2BIG);
    }
    s.blur_down((bx0, by0, sw, sh))?;
    for _ in 0..BLUR_PASSES {
        // Columns, then rows (box_pass in compute_blur and blur_region).
        s.blur_box(true, sw, sh, 1, sw)?;
        s.blur_box(false, sh, sw, sw, 1)?;
    }
    Ok(Some((bx0, by0, sw, sh)))
}
