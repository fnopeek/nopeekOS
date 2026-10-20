//! Fill an icon outline at any pixel size (docs/plan/UI_SCALE.md, step 2).
//!
//! The outline comes from `tools/regen-icons`: closed polygons in 1/64 units
//! of the 256 grid, strokes already turned into fills, curves flattened. So
//! this is a polygon filler and nothing else: nonzero winding, `ROWS`
//! sample rows per pixel, exact horizontal coverage within each row.

use alloc::vec::Vec;

/// Outline units per edge of the icon square.
pub const GRID: f32 = 256.0 * 64.0;
/// Sample rows per pixel row.
const ROWS: usize = 16;

/// One icon's outline: `points` as (x, y) in grid units, and where each
/// contour ends in it (exclusive).
pub struct Outline {
    pub points: Vec<(i16, i16)>,
    pub ends:   Vec<u32>,
}

/// Alpha coverage of `o` drawn into a `size`×`size` square, row-major.
pub fn rasterize(o: &Outline, size: u16) -> Vec<u8> {
    let n = size as usize;
    let mut out = alloc::vec![0u8; n * n];
    if n == 0 { return out; }
    let k = size as f32 / GRID;

    // Edges in pixel space, direction kept for the winding number.
    let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut start = 0usize;
    for &end in &o.ends {
        let end = (end as usize).min(o.points.len());
        let c = &o.points[start..end];
        for i in 0..c.len() {
            let a = c[i];
            let b = c[(i + 1) % c.len()];
            if a.1 != b.1 {
                edges.push((a.0 as f32 * k, a.1 as f32 * k, b.0 as f32 * k, b.1 as f32 * k));
            }
        }
        start = end;
    }

    let mut acc = alloc::vec![0f32; n];
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for py in 0..n {
        acc.iter_mut().for_each(|v| *v = 0.0);
        for sub in 0..ROWS {
            let y = py as f32 + (sub as f32 + 0.5) / ROWS as f32;
            xs.clear();
            for &(x0, y0, x1, y1) in &edges {
                let (lo, hi, dir) = if y0 < y1 { (y0, y1, 1) } else { (y1, y0, -1) };
                if y < lo || y >= hi { continue; }
                xs.push((x0 + (y - y0) * (x1 - x0) / (y1 - y0), dir));
            }
            xs.sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
            // Between two consecutive crossings the winding number is
            // constant; that piece is inside when it is not zero.
            let mut wind = 0i32;
            for i in 0..xs.len().saturating_sub(1) {
                wind += xs[i].1;
                if wind != 0 { span(&mut acc, xs[i].0, xs[i + 1].0); }
            }
        }
        let row = &mut out[py * n..py * n + n];
        for (o, a) in row.iter_mut().zip(acc.iter()) {
            *o = ((a / ROWS as f32) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Add the coverage of [x0, x1) on one sample row to `acc`.
fn span(acc: &mut [f32], x0: f32, x1: f32) {
    let n = acc.len() as f32;
    let (x0, x1) = (x0.clamp(0.0, n), x1.clamp(0.0, n));
    if x1 <= x0 { return; }
    let (p0, p1) = (x0 as usize, (x1 as usize + 1).min(acc.len()));
    for (p, v) in acc.iter_mut().enumerate().take(p1).skip(p0) {
        let l = x0.max(p as f32);
        let r = x1.min(p as f32 + 1.0);
        if r > l { *v += r - l; }
    }
}
