//! How a checked scene becomes dispatches (scene.rs): per damage
//! rectangle one compose over the union of what the items reach, with a
//! bin table: per 64x64 bin of the screen grid the items that reach the
//! bin, in order, starting at the last one that covers it opaquely (the
//! background, a window's content, a window's glass). No GPU access here;
//! the caller's `sink` turns each step into commands and memory writes.
//! Not a Linux file.

pub const EINVAL: i32 = 22;
pub const E2BIG: i32 = 7;

pub const KIND_BACKGROUND: u8 = 1;
pub const KIND_GLOW: u8 = 2;
pub const KIND_CHROME: u8 = 4;
pub const KIND_CONTENT: u8 = 5;
pub const KIND_PANEL: u8 = 6;

/// A bin is 64x64 pixels.
pub const BIN: u32 = 64;
/// Bins of one compose dispatch.
pub const MAX_BINS: usize = 4096;
/// Words after a table's lists that compose.comp's 16-word loads may touch.
pub const BLOCK_SLACK: u32 = 16;
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
    /// From the item's words (kind, x, y, w, h, ...) on a `screen`;
    /// `glass`: the scene has a glass source (blurred or sharp wallpaper).
    pub fn of(w: &[u32], screen: &rect, glass: bool) -> item_shape {
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
            KIND_CHROME => (chrome_interior(w, glass), EMPTY),
            _ => (EMPTY, EMPTY),
        };
        item_shape { kind, reach, cover }
    }
}

/// Where render::fill_rounded_chrome_aa takes its straight-row path and
/// writes a value that does not depend on what is beneath: the glass of a
/// window that paints its content, opaque (`bg`) or tinted from the glass
/// source. Without a glass source a translucent fill blends over what is
/// beneath, and the ring, the corner rows and the two fringe columns do
/// everywhere.
fn chrome_interior(w: &[u32], glass: bool) -> rect {
    let paint_content = w[12] != 0;
    let go = w[11].min(255);
    if !paint_content || (go < 255 && !glass) {
        return EMPTY;
    }
    chrome_straight(w)
}

/// Where render::fill_rounded_chrome_aa takes its straight-row path for
/// the chrome item `w`: inside the border, off the corner rows and the two
/// fringe columns (items.glsl `chrome_inside`).
pub fn chrome_straight(w: &[u32]) -> rect {
    let r = rect { x: w[1], y: w[2], w: w[3], h: w[4] };
    if r.w < 2 || r.h < 2 {
        return EMPTY;
    }
    let r_out = w[8].min(r.w / 2).min(r.h / 2);
    let border_px = w[9].min(r_out).min(r.w / 2).min(r.h / 2);
    let inner_x = r.x + border_px;
    let inner_y = r.y + border_px;
    let inner_w = r.w - 2 * border_px;
    let inner_h = r.h - 2 * border_px;
    let r_in = r_out.saturating_sub(border_px).min(inner_w / 2).min(inner_h / 2);
    let (y0, y1) = (inner_y + r_in, (inner_y + inner_h).saturating_sub(r_in));
    let (x0, x1) = (inner_x + 2, (inner_x + inner_w).saturating_sub(2));
    if x1 <= x0 || y1 <= y0 {
        return EMPTY;
    }
    rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
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

/// What the plan asks of the GPU side.
pub trait sink {
    /// Words of a bin table, at word `at` of the bins buffer.
    fn bins(&mut self, at: u32, words: &[u32]) -> Result<(), i32>;
    fn compose(&mut self, area: &rect, t: &bin_table) -> Result<(), i32>;
}

pub struct limits {
    pub bins_words: u32,
}

/// What the draw plan asks of the GPU side: the items drawn in order.
pub trait draw_sink {
    /// Every item over `area`, each where it reaches.
    fn draw(&mut self, area: &rect) -> Result<(), i32>;
}

/// The draws of a scene: per damage rectangle every item, over the part
/// of the rectangle the items reach. On a GPU that draws into a tile
/// buffer, an item hidden by a later opaque one costs a pass over tile
/// memory, not a read of the target, so no bin skips it.
pub fn plan_draws(s: &mut dyn draw_sink, screen: &rect, damage: &[rect], items: &[item_shape]) -> Result<(), i32> {
    if items.len() > MAX_ITEMS {
        return Err(-E2BIG);
    }
    for d in damage {
        let area = reached(items, &d.intersect(screen));
        if !area.is_empty() {
            s.draw(&area)?;
        }
    }
    Ok(())
}

/// The part of `clip` the items reach.
fn reached(items: &[item_shape], clip: &rect) -> rect {
    if clip.is_empty() {
        return EMPTY;
    }
    items.iter().fold(EMPTY, |a, it| a.union(&it.reach)).intersect(clip)
}

/// Pieces an item's draw is split into at most; a cover that would split
/// it further is not cut out.
pub const MAX_PIECES: usize = 16;

/// Where item `i` of `items[..end]` must be drawn over `area`: its reach
/// within the area, less every part a later item covers
/// opaquely (`cover`: there the later item's result does not depend on
/// what lies beneath). At most `MAX_PIECES` rectangles, disjoint.
pub fn visible_parts(items: &[item_shape], end: usize, i: usize, area: &rect) -> alloc::vec::Vec<rect> {
    let mut parts = alloc::vec::Vec::new();
    let r = items[i].reach.intersect(area);
    if r.is_empty() {
        return parts;
    }
    parts.push(r);
    for it in &items[i + 1..end] {
        for c in [it.cover.0, it.cover.1] {
            if c.is_empty() {
                continue;
            }
            let mut next = alloc::vec::Vec::with_capacity(parts.len() + 4);
            for p in &parts {
                subtract(p, &c, &mut next);
            }
            if next.len() > MAX_PIECES {
                return parts;
            }
            parts = next;
            if parts.is_empty() {
                return parts;
            }
        }
    }
    parts
}

/// `a` less `b`, as up to four disjoint rectangles: the rows above and
/// below `b`, then the columns left and right of it in between.
pub fn subtract(a: &rect, b: &rect, out: &mut alloc::vec::Vec<rect>) {
    let i = a.intersect(b);
    if i.is_empty() {
        out.push(*a);
        return;
    }
    if i.y > a.y {
        out.push(rect { x: a.x, y: a.y, w: a.w, h: i.y - a.y });
    }
    if i.y1() < a.y1() {
        out.push(rect { x: a.x, y: i.y1(), w: a.w, h: a.y1() - i.y1() });
    }
    if i.x > a.x {
        out.push(rect { x: a.x, y: i.y, w: i.x - a.x, h: i.h });
    }
    if i.x1() < a.x1() {
        out.push(rect { x: i.x1(), y: i.y, w: a.x1() - i.x1(), h: i.h });
    }
}

/// The dispatches of a scene: `items` in order, `damage` already checked.
pub fn plan(s: &mut dyn sink, lim: &limits, screen: &rect, damage: &[rect], items: &[item_shape])
            -> Result<(), i32> {
    if items.len() > MAX_ITEMS {
        return Err(-E2BIG);
    }
    let mut bins_at = 0u32;
    for d in damage {
        let area = reached(items, &d.intersect(screen));
        if area.is_empty() {
            continue;
        }
        let t = write_bins(s, lim, bins_at, &area, items)?;
        bins_at += t.words;
        s.compose(&area, &t)?;
    }
    Ok(())
}

/// The bin table of `items` over `area`, at word `at`.
fn write_bins(s: &mut dyn sink, lim: &limits, at: u32, area: &rect, items: &[item_shape])
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
            let bin = rect { x: (bin_x0 + bx) * BIN, y: (bin_y0 + by) * BIN, w: BIN, h: BIN };
            let br = bin.intersect(area);
            let mut n = 0usize;
            let mut opaque = false;
            if !br.is_empty() {
                for (i, it) in items.iter().enumerate() {
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
