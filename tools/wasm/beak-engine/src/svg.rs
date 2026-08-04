//! svg.rs — from-scratch SVG rasteriser for `<img src=*.svg>`.
//!
//! Parses SVG (own XML reader — SVG is case-sensitive and self-close heavy),
//! walks shapes/paths/groups with inherited presentation attrs + transforms,
//! maps `viewBox` → viewport (xMidYMid meet), and fills paths with a
//! supersampled scanline coverage rasteriser (nonzero / evenodd) into a
//! straight-BGRA `Image`. Colours reuse `color::parse_color` (named + Color 4).
//!
//! v1 = fills only. Stroke, gradients, `<use>`/`<defs>`, inline `<svg>` in the
//! DOM come next (see the SVG WPT oracle).

use crate::color::parse_color;
use crate::image::Image;
use crate::layout::Rgb;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libm::{ceilf, cosf, fabsf, floorf, sinf, sqrtf, tanf};

const MAX_SIDE: u32 = 1024;
const MAX_PIXELS: usize = 1_000_000; // caps the f32 accumulation buffer (~16 MB)
const SS: usize = 4; // vertical supersampling for anti-aliasing

/// Detect an SVG document: skip a UTF-8 BOM / leading whitespace / an XML
/// declaration / a doctype / comments, then look for `<svg`.
pub fn looks_like_svg(bytes: &[u8]) -> bool {
    let s = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let head = &s[..s.len().min(1024)];
    head.contains("<svg")
}

/// Render SVG bytes into an `Image` (straight BGRA, alpha=0 where nothing is
/// painted so it composites over the page). `None` on parse failure / no size.
pub fn render(bytes: &[u8]) -> Option<Image> {
    let text = core::str::from_utf8(bytes).ok()?;
    let root = parse_xml(text)?;
    if !root.tag.ends_with("svg") {
        // The document element must be <svg> (allow a namespace prefix).
        return None;
    }

    // Intrinsic size + user→device root matrix.
    let vb = attr(&root, "viewBox").and_then(parse_view_box);
    let aw = attr(&root, "width").and_then(|v| parse_len(&v));
    let ah = attr(&root, "height").and_then(|v| parse_len(&v));
    let (dev_w, dev_h) = match (aw, ah, vb) {
        (Some(w), Some(h), _) => (w, h),
        (_, _, Some((_, _, vw, vh))) => (vw, vh),
        _ => (300.0, 150.0),
    };
    if !(dev_w > 0.0) || !(dev_h > 0.0) {
        return None;
    }
    // Clamp raster size to bounds, keeping aspect.
    let mut w = dev_w;
    let mut h = dev_h;
    let scale_cap = (MAX_SIDE as f32 / w.max(h)).min(1.0);
    w *= scale_cap;
    h *= scale_cap;
    let iw = (ceilf(w) as u32).clamp(1, MAX_SIDE);
    let ih = (ceilf(h) as u32).clamp(1, MAX_SIDE);
    if (iw as usize).checked_mul(ih as usize)? > MAX_PIXELS {
        return None;
    }

    let root_mat = view_box_matrix(vb, iw as f32, ih as f32);

    // Walk the tree into an ordered fill list.
    let mut fills: Vec<Fill> = Vec::new();
    let base = Paint {
        fill: Some(Rgb(0, 0, 0)),
        fill_opacity: 1.0,
        evenodd: false,
        opacity: 1.0,
        stroke: None,
        stroke_width: 1.0,
        stroke_opacity: 1.0,
        cap: Cap::Butt,
    };
    let grads = gradient_colors(text);
    walk(&root, &root_mat, &base, &mut fills, &grads);
    if fills.is_empty() {
        // Nothing drawable — still return a transparent box so the <img> box
        // is sized (better than a placeholder for an empty/def-only SVG).
    }

    let bgra = rasterize(iw, ih, &fills)?;
    Some(Image { bgra, w: iw, h: ih })
}

// ── affine matrix: x' = a*x + c*y + e ; y' = b*x + d*y + f ────────────────────

#[derive(Clone, Copy)]
struct Mat {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}
impl Mat {
    fn id() -> Mat {
        Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }
    /// `self.mul(o).apply(p) == self.apply(o.apply(p))` — `o` applied first.
    fn mul(&self, o: &Mat) -> Mat {
        Mat {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }
    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
    /// Approx uniform scale of this matrix (for adaptive curve flattening).
    fn scale_hint(&self) -> f32 {
        sqrtf(fabsf(self.a * self.d - self.b * self.c)).max(0.001)
    }
}

fn view_box_matrix(vb: Option<(f32, f32, f32, f32)>, iw: f32, ih: f32) -> Mat {
    match vb {
        None => Mat::id(),
        Some((minx, miny, vw, vh)) if vw > 0.0 && vh > 0.0 => {
            // xMidYMid meet: uniform scale, centre the shorter axis.
            let s = (iw / vw).min(ih / vh);
            let tx = (iw - vw * s) * 0.5 - minx * s;
            let ty = (ih - vh * s) * 0.5 - miny * s;
            Mat { a: s, b: 0.0, c: 0.0, d: s, e: tx, f: ty }
        }
        _ => Mat::id(),
    }
}

// ── presentation state (inherited) ───────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy)]
struct Paint {
    fill: Option<Rgb>,
    fill_opacity: f32,
    evenodd: bool,
    opacity: f32,
    stroke: Option<Rgb>,
    stroke_width: f32,
    stroke_opacity: f32,
    cap: Cap,
}

/// A flattened contour in device space; `closed` distinguishes joins vs caps
/// for stroking (fill closes every contour implicitly).
struct SubPath {
    pts: Vec<(f32, f32)>,
    closed: bool,
}

struct Fill {
    subs: Vec<Vec<(f32, f32)>>, // device-space polylines
    r: u8,
    g: u8,
    b: u8,
    a: f32, // 0..1
    evenodd: bool,
}

fn walk(el: &XmlEl, ctm: &Mat, parent: &Paint, out: &mut Vec<Fill>, grads: &[(String, Rgb)]) {
    let ctm = match attr(el, "transform") {
        Some(t) => ctm.mul(&parse_transform(&t)),
        None => *ctm,
    };
    let paint = resolve_paint(el, parent, grads);

    let tag = local_name(&el.tag);
    match tag {
        "svg" | "g" | "a" | "switch" | "symbol" => {
            for ch in &el.children {
                if let XmlNode::El(c) = ch {
                    walk(c, &ctm, &paint, out, grads);
                }
            }
        }
        "defs" | "clipPath" | "mask" | "title" | "desc" | "metadata" | "style" | "use"
        | "linearGradient" | "radialGradient" | "filter" | "pattern" => {
            // v1: not rendered (defs/gradients/use handled in a later iteration).
        }
        _ => {
            let subs = shape_subpaths(el, tag, &ctm);
            if subs.is_empty() {
                return;
            }
            // fill first (paints under the stroke)
            if let Some(fill) = paint.fill {
                let polys: Vec<Vec<(f32, f32)>> = subs.iter().map(|s| s.pts.clone()).collect();
                let a = (paint.fill_opacity * paint.opacity).clamp(0.0, 1.0);
                out.push(Fill { subs: polys, r: fill.0, g: fill.1, b: fill.2, a, evenodd: paint.evenodd });
            }
            // stroke on top
            if let Some(sc) = paint.stroke {
                let dw = paint.stroke_width * ctm.scale_hint();
                if dw > 0.02 {
                    let pieces = stroke_polys(&subs, dw / 2.0, paint.cap);
                    if !pieces.is_empty() {
                        let a = (paint.stroke_opacity * paint.opacity).clamp(0.0, 1.0);
                        out.push(Fill { subs: pieces, r: sc.0, g: sc.1, b: sc.2, a, evenodd: false });
                    }
                }
            }
        }
    }
}

/// Average colour of every gradient in the document, by id.
///
/// v1 paints a gradient as ONE flat colour: the mean of its stops. A real
/// gradient needs per-pixel interpolation in the rasteriser; the flat stand-in
/// is what turns Wikipedia's logo from a black disc into a light sphere, and at
/// icon size the difference from the real thing is small. Scanned off the raw
/// text rather than the parsed tree because `<defs>` is skipped there.
fn gradient_colors(text: &str) -> Vec<(String, Rgb)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("Gradient") {
        // `<linearGradient` / `<radialGradient` — anything else is not one.
        let head = &rest[..i];
        if !(head.ends_with("<linear") || head.ends_with("<radial")) {
            rest = &rest[i + 8..];
            continue;
        }
        let body = &rest[i..];
        let Some(end) = body.find("Gradient>") else { break };
        let block = &body[..end];
        rest = &body[end + 9..];
        let Some(id) = attr_value(block, "id") else { continue };
        let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
        for stop in block.split("<stop").skip(1) {
            let c = attr_value(stop, "stop-color")
                .or_else(|| style_value(stop, "stop-color"))
                .and_then(|v| parse_color(&v));
            if let Some(Rgb(cr, cg, cb)) = c {
                r += cr as u32;
                g += cg as u32;
                b += cb as u32;
                n += 1;
            }
        }
        if n > 0 {
            out.push((id, Rgb((r / n) as u8, (g / n) as u8, (b / n) as u8)));
        }
    }
    out
}

/// The value of `name="…"` in a raw tag slice.
fn attr_value(s: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = s[from..].find(name) {
        let at = from + i;
        let before = s[..at].chars().next_back();
        let after = s[at + name.len()..].trim_start();
        from = at + name.len();
        if !matches!(before, Some(c) if c.is_whitespace() || c == '<') || !after.starts_with('=') {
            continue;
        }
        let v = after[1..].trim_start();
        let q = v.chars().next()?;
        if q != '"' && q != '\'' {
            continue;
        }
        let end = v[1..].find(q)?;
        return Some(v[1..1 + end].to_string());
    }
    None
}

/// The value of `name:` inside this tag's `style="…"`.
fn style_value(s: &str, name: &str) -> Option<String> {
    let style = attr_value(s, "style")?;
    for decl in style.split(';') {
        let (k, v) = decl.split_once(':')?;
        if k.trim() == name {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// `url(#id)` → the id it names.
fn url_ref(v: &str) -> Option<&str> {
    let inner = v.strip_prefix("url(")?.strip_suffix(')')?.trim();
    inner.strip_prefix('#').map(|s| s.trim_matches(|c| c == '"' || c == '\''))
}

fn resolve_paint(el: &XmlEl, parent: &Paint, grads: &[(String, Rgb)]) -> Paint {
    let mut p = *parent;
    p.opacity = parent.opacity; // opacity does NOT inherit; applied multiplicatively
    // opacity is a property of THIS element only (reset), fill/fill-* inherit.
    let mut own_opacity = 1.0;

    let mut apply = |name: &str, val: &str| {
        let val = val.trim();
        match name {
            "fill" => {
                p.fill = match val {
                    "none" => None,
                    "currentColor" | "context-fill" => Some(Rgb(0, 0, 0)),
                    v => url_ref(v)
                        .and_then(|id| grads.iter().find(|(g, _)| g == id).map(|(_, c)| *c))
                        .or_else(|| parse_color(v))
                        .or(p.fill),
                }
            }
            "fill-opacity" => {
                if let Some(o) = parse_opacity(val) {
                    p.fill_opacity = o;
                }
            }
            "fill-rule" => p.evenodd = val == "evenodd",
            "stroke" => {
                p.stroke = match val {
                    "none" => None,
                    "currentColor" | "context-stroke" => Some(Rgb(0, 0, 0)),
                    v => parse_color(v).or(p.stroke),
                }
            }
            "stroke-width" => {
                if let Some(w) = parse_len(val) {
                    p.stroke_width = w;
                }
            }
            "stroke-opacity" => {
                if let Some(o) = parse_opacity(val) {
                    p.stroke_opacity = o;
                }
            }
            "stroke-linecap" => {
                p.cap = match val {
                    "round" => Cap::Round,
                    "square" => Cap::Square,
                    _ => Cap::Butt,
                }
            }
            "opacity" => {
                if let Some(o) = parse_opacity(val) {
                    own_opacity = o;
                }
            }
            _ => {}
        }
    };

    // Presentation attributes first, then style="" overrides.
    for (k, v) in &el.attrs {
        apply(local_name(k), v);
    }
    if let Some(style) = attr(el, "style") {
        for decl in style.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                apply(k.trim(), v.trim());
            }
        }
    }
    p.opacity = parent.opacity * own_opacity;
    p
}

fn parse_opacity(v: &str) -> Option<f32> {
    if let Some(pct) = v.strip_suffix('%') {
        pct.trim().parse::<f32>().ok().map(|f| (f / 100.0).clamp(0.0, 1.0))
    } else {
        v.parse::<f32>().ok().map(|f| f.clamp(0.0, 1.0))
    }
}

// ── shapes → device-space subpaths ───────────────────────────────────────────

fn shape_subpaths(el: &XmlEl, tag: &str, ctm: &Mat) -> Vec<SubPath> {
    let num = |n: &str| attr(el, n).and_then(|v| parse_len(&v)).unwrap_or(0.0);
    let closed = |pts: Vec<(f32, f32)>| alloc::vec![SubPath { pts, closed: true }];
    match tag {
        "path" => attr(el, "d").map(|d| parse_path(&d, ctm)).unwrap_or_default(),
        "rect" => {
            let (x, y, w, h) = (num("x"), num("y"), num("width"), num("height"));
            if w <= 0.0 || h <= 0.0 {
                return Vec::new();
            }
            let mut rx = attr(el, "rx").and_then(|v| parse_len(&v));
            let mut ry = attr(el, "ry").and_then(|v| parse_len(&v));
            if rx.is_none() {
                rx = ry;
            }
            if ry.is_none() {
                ry = rx;
            }
            let rx = rx.unwrap_or(0.0).clamp(0.0, w / 2.0);
            let ry = ry.unwrap_or(0.0).clamp(0.0, h / 2.0);
            if rx > 0.0 && ry > 0.0 {
                rounded_rect(x, y, w, h, rx, ry, ctm)
            } else {
                let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
                closed(pts.iter().map(|&(px, py)| ctm.apply(px, py)).collect())
            }
        }
        "circle" => {
            let r = num("r");
            if r <= 0.0 {
                return Vec::new();
            }
            ellipse_sub(num("cx"), num("cy"), r, r, ctm)
        }
        "ellipse" => {
            let (rx, ry) = (num("rx"), num("ry"));
            if rx <= 0.0 || ry <= 0.0 {
                return Vec::new();
            }
            ellipse_sub(num("cx"), num("cy"), rx, ry, ctm)
        }
        "polygon" | "polyline" => {
            let pts = attr(el, "points").map(|p| parse_points(&p)).unwrap_or_default();
            if pts.len() < 2 {
                return Vec::new();
            }
            let dev: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| ctm.apply(x, y)).collect();
            alloc::vec![SubPath { pts: dev, closed: tag == "polygon" }]
        }
        "line" => {
            // no fill area, but strokeable
            let a = ctm.apply(num("x1"), num("y1"));
            let b = ctm.apply(num("x2"), num("y2"));
            alloc::vec![SubPath { pts: alloc::vec![a, b], closed: false }]
        }
        _ => Vec::new(),
    }
}

fn ellipse_sub(cx: f32, cy: f32, rx: f32, ry: f32, ctm: &Mat) -> Vec<SubPath> {
    let n = seg_count(rx.max(ry) * ctm.scale_hint() * 6.28);
    let mut sub = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i as f32 / n as f32) * core::f32::consts::TAU;
        sub.push(ctm.apply(cx + rx * cosf(t), cy + ry * sinf(t)));
    }
    alloc::vec![SubPath { pts: sub, closed: true }]
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, rx: f32, ry: f32, ctm: &Mat) -> Vec<SubPath> {
    let mut sub: Vec<(f32, f32)> = Vec::new();
    // corner arcs sampled; quarter ellipse per corner (clockwise from top-left).
    let corner = |sub: &mut Vec<(f32, f32)>, cx: f32, cy: f32, a0: f32| {
        let n = seg_count((rx.max(ry)) * ctm.scale_hint() * 1.6).max(3);
        for i in 0..=n {
            let t = a0 + (i as f32 / n as f32) * core::f32::consts::FRAC_PI_2;
            sub.push(ctm.apply(cx + rx * cosf(t), cy + ry * sinf(t)));
        }
    };
    corner(&mut sub, x + rx, y + ry, core::f32::consts::PI); // TL
    corner(&mut sub, x + w - rx, y + ry, core::f32::consts::PI * 1.5); // TR
    corner(&mut sub, x + w - rx, y + h - ry, 0.0); // BR
    corner(&mut sub, x + rx, y + h - ry, core::f32::consts::FRAC_PI_2); // BL
    alloc::vec![SubPath { pts: sub, closed: true }]
}

fn seg_count(approx_len_px: f32) -> usize {
    ((approx_len_px / 3.0) as usize).clamp(8, 240)
}

// ── path data (M L H V C S Q T A Z, absolute + relative) ──────────────────────

fn parse_path(d: &str, ctm: &Mat) -> Vec<SubPath> {
    let mut sc = NumScan::new(d);
    let mut subs: Vec<SubPath> = Vec::new();
    let mut cur = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    let mut cur_sub: Vec<(f32, f32)> = Vec::new();
    let mut last_ctrl: Option<(f32, f32)> = None; // for S/T reflection (user space)
    let mut last_cmd = b' ';
    let hint = ctm.scale_hint();

    let push_sub = |subs: &mut Vec<SubPath>, cur_sub: &mut Vec<(f32, f32)>, closed: bool| {
        if cur_sub.len() >= 2 {
            subs.push(SubPath { pts: core::mem::take(cur_sub), closed });
        } else {
            cur_sub.clear();
        }
    };

    loop {
        sc.skip_sep();
        let cmd = match sc.peek_cmd() {
            Some(c) => c,
            None => break,
        };
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        // A command letter is consumed only when explicit; repeated coords reuse it.
        if sc.peek_is_cmd() {
            sc.bump();
        }
        // A drawing command after Z (no M) restarts a subpath at the current point.
        if cur_sub.is_empty() && up != b'M' && up != b'Z' {
            cur_sub.push(ctm.apply(cur.0, cur.1));
        }

        match up {
            b'M' => {
                let (mut px, mut py) = match sc.pair() {
                    Some(p) => p,
                    None => break,
                };
                if rel {
                    px += cur.0;
                    py += cur.1;
                }
                push_sub(&mut subs, &mut cur_sub, false);
                cur = (px, py);
                start = cur;
                cur_sub.push(ctm.apply(cur.0, cur.1));
                // subsequent implicit pairs are lineto
                while let Some((mut lx, mut ly)) = sc.pair_if_num() {
                    if rel {
                        lx += cur.0;
                        ly += cur.1;
                    }
                    cur = (lx, ly);
                    cur_sub.push(ctm.apply(cur.0, cur.1));
                }
                last_ctrl = None;
            }
            b'L' => {
                while let Some((mut lx, mut ly)) = sc.pair_if_num() {
                    if rel {
                        lx += cur.0;
                        ly += cur.1;
                    }
                    cur = (lx, ly);
                    cur_sub.push(ctm.apply(cur.0, cur.1));
                }
                last_ctrl = None;
            }
            b'H' => {
                while let Some(mut hx) = sc.num_if() {
                    if rel {
                        hx += cur.0;
                    }
                    cur = (hx, cur.1);
                    cur_sub.push(ctm.apply(cur.0, cur.1));
                }
                last_ctrl = None;
            }
            b'V' => {
                while let Some(mut vy) = sc.num_if() {
                    if rel {
                        vy += cur.1;
                    }
                    cur = (cur.0, vy);
                    cur_sub.push(ctm.apply(cur.0, cur.1));
                }
                last_ctrl = None;
            }
            b'C' => {
                while let Some(((mut x1, mut y1), (mut x2, mut y2), (mut x, mut y))) = sc.triple_if() {
                    if rel {
                        x1 += cur.0;
                        y1 += cur.1;
                        x2 += cur.0;
                        y2 += cur.1;
                        x += cur.0;
                        y += cur.1;
                    }
                    flatten_cubic(ctm, cur, (x1, y1), (x2, y2), (x, y), hint, &mut cur_sub);
                    last_ctrl = Some((x2, y2));
                    cur = (x, y);
                }
            }
            b'S' => {
                while let Some(((mut x2, mut y2), (mut x, mut y))) = sc.pair2_if() {
                    if rel {
                        x2 += cur.0;
                        y2 += cur.1;
                        x += cur.0;
                        y += cur.1;
                    }
                    let (x1, y1) = reflect(last_ctrl, cur, last_cmd, b'C');
                    flatten_cubic(ctm, cur, (x1, y1), (x2, y2), (x, y), hint, &mut cur_sub);
                    last_ctrl = Some((x2, y2));
                    cur = (x, y);
                }
            }
            b'Q' => {
                while let Some(((mut x1, mut y1), (mut x, mut y))) = sc.pair2_if() {
                    if rel {
                        x1 += cur.0;
                        y1 += cur.1;
                        x += cur.0;
                        y += cur.1;
                    }
                    flatten_quad(ctm, cur, (x1, y1), (x, y), hint, &mut cur_sub);
                    last_ctrl = Some((x1, y1));
                    cur = (x, y);
                }
            }
            b'T' => {
                while let Some((mut x, mut y)) = sc.pair_if_num() {
                    if rel {
                        x += cur.0;
                        y += cur.1;
                    }
                    let (x1, y1) = reflect(last_ctrl, cur, last_cmd, b'Q');
                    flatten_quad(ctm, cur, (x1, y1), (x, y), hint, &mut cur_sub);
                    last_ctrl = Some((x1, y1));
                    cur = (x, y);
                }
            }
            b'A' => {
                while let Some((rx, ry, rot, large, sweep, mut x, mut y)) = sc.arc_if() {
                    if rel {
                        x += cur.0;
                        y += cur.1;
                    }
                    flatten_arc(ctm, cur, rx, ry, rot, large, sweep, (x, y), hint, &mut cur_sub);
                    cur = (x, y);
                    last_ctrl = None;
                }
            }
            b'Z' => {
                push_sub(&mut subs, &mut cur_sub, true);
                cur = start;
                last_ctrl = None;
            }
            _ => {
                // Unknown command: stop rather than loop forever.
                break;
            }
        }
        last_cmd = up;
    }
    push_sub(&mut subs, &mut cur_sub, false);
    subs
}

fn reflect(last_ctrl: Option<(f32, f32)>, cur: (f32, f32), last_cmd: u8, want: u8) -> (f32, f32) {
    match last_ctrl {
        Some((cx, cy)) if last_cmd == want || last_cmd == want + 1 => {
            (2.0 * cur.0 - cx, 2.0 * cur.1 - cy)
        }
        _ => cur,
    }
}

fn flatten_cubic(ctm: &Mat, p0: (f32, f32), c1: (f32, f32), c2: (f32, f32), p1: (f32, f32), hint: f32, sub: &mut Vec<(f32, f32)>) {
    let approx = (dist(p0, c1) + dist(c1, c2) + dist(c2, p1)) * hint;
    let n = seg_count(approx);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let x = u * u * u * p0.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * p1.0;
        let y = u * u * u * p0.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * p1.1;
        sub.push(ctm.apply(x, y));
    }
}

fn flatten_quad(ctm: &Mat, p0: (f32, f32), c: (f32, f32), p1: (f32, f32), hint: f32, sub: &mut Vec<(f32, f32)>) {
    let approx = (dist(p0, c) + dist(c, p1)) * hint;
    let n = seg_count(approx);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let x = u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0;
        let y = u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1;
        sub.push(ctm.apply(x, y));
    }
}

#[allow(clippy::too_many_arguments)]
fn flatten_arc(ctm: &Mat, p0: (f32, f32), mut rx: f32, mut ry: f32, rot_deg: f32, large: bool, sweep: bool, p1: (f32, f32), hint: f32, sub: &mut Vec<(f32, f32)>) {
    // SVG arc → centre parametrisation (impl notes F.6.5/F.6.6).
    if rx == 0.0 || ry == 0.0 || (p0.0 == p1.0 && p0.1 == p1.1) {
        sub.push(ctm.apply(p1.0, p1.1));
        return;
    }
    rx = fabsf(rx);
    ry = fabsf(ry);
    let phi = rot_deg * core::f32::consts::PI / 180.0;
    let (cp, sp) = (cosf(phi), sinf(phi));
    let dx = (p0.0 - p1.0) / 2.0;
    let dy = (p0.1 - p1.1) / 2.0;
    let x1p = cp * dx + sp * dy;
    let y1p = -sp * dx + cp * dy;
    // radius correction
    let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lam > 1.0 {
        let s = sqrtf(lam);
        rx *= s;
        ry *= s;
    }
    let num = (rx * rx * ry * ry) - (rx * rx * y1p * y1p) - (ry * ry * x1p * x1p);
    let den = (rx * rx * y1p * y1p) + (ry * ry * x1p * x1p);
    let mut co = sqrtf((num / den).max(0.0));
    if large == sweep {
        co = -co;
    }
    let cxp = co * rx * y1p / ry;
    let cyp = -co * ry * x1p / rx;
    let cx = cp * cxp - sp * cyp + (p0.0 + p1.0) / 2.0;
    let cy = sp * cxp + cp * cyp + (p0.1 + p1.1) / 2.0;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
        let dot = ux * vx + uy * vy;
        let len = sqrtf((ux * ux + uy * uy) * (vx * vx + vy * vy)).max(1e-6);
        let mut a = (dot / len).clamp(-1.0, 1.0);
        a = libm::acosf(a);
        if ux * vy - uy * vx < 0.0 {
            -a
        } else {
            a
        }
    };
    let theta1 = ang(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dtheta = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    if !sweep && dtheta > 0.0 {
        dtheta -= core::f32::consts::TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += core::f32::consts::TAU;
    }
    let n = seg_count(fabsf(dtheta) * rx.max(ry) * hint).max(4);
    for i in 1..=n {
        let t = theta1 + dtheta * (i as f32 / n as f32);
        let (ct, st) = (cosf(t), sinf(t));
        let x = cx + rx * ct * cp - ry * st * sp;
        let y = cy + rx * ct * sp + ry * st * cp;
        sub.push(ctm.apply(x, y));
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    sqrtf(dx * dx + dy * dy)
}

// ── stroke → fill (union of segment quads + round joins/caps, nonzero) ───────
//
// Each contour is widened to a set of convex pieces (segment rectangles, plus a
// disc at every join and round cap) — all wound the same way so a nonzero fill
// unions them cleanly, no outline-intersection maths. Joins/round-caps are
// exact; miter/bevel are approximated as round for v1 (the common icon style).

fn stroke_polys(subs: &[SubPath], r: f32, cap: Cap) -> Vec<Vec<(f32, f32)>> {
    let mut pieces: Vec<Vec<(f32, f32)>> = Vec::new();
    if r <= 0.0 {
        return pieces;
    }
    for sp in subs {
        let pts = &sp.pts;
        let n = pts.len();
        if n == 0 {
            continue;
        }
        if n == 1 {
            if cap == Cap::Round {
                pieces.push(disc(pts[0], r));
            }
            continue;
        }
        let seg_end = if sp.closed { n } else { n - 1 };
        for i in 0..seg_end {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            if let Some(nrm) = perp(a, b, r) {
                let mut q = alloc::vec![
                    (a.0 + nrm.0, a.1 + nrm.1),
                    (b.0 + nrm.0, b.1 + nrm.1),
                    (b.0 - nrm.0, b.1 - nrm.1),
                    (a.0 - nrm.0, a.1 - nrm.1),
                ];
                ensure_ccw(&mut q);
                pieces.push(q);
            }
        }
        // joins: a disc at each corner vertex (all corners if closed, interior if open)
        let (js, je) = if sp.closed { (0, n) } else { (1, n - 1) };
        for i in js..je {
            pieces.push(disc(pts[i % n], r));
        }
        if !sp.closed {
            match cap {
                Cap::Round => {
                    pieces.push(disc(pts[0], r));
                    pieces.push(disc(pts[n - 1], r));
                }
                Cap::Square => {
                    if let Some(q) = square_cap(pts[1], pts[0], r) {
                        pieces.push(q);
                    }
                    if let Some(q) = square_cap(pts[n - 2], pts[n - 1], r) {
                        pieces.push(q);
                    }
                }
                Cap::Butt => {}
            }
        }
    }
    pieces
}

/// Perpendicular offset of length `r` for the segment a→b (None if degenerate).
fn perp(a: (f32, f32), b: (f32, f32), r: f32) -> Option<(f32, f32)> {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let l = sqrtf(dx * dx + dy * dy);
    if l < 1e-4 {
        return None;
    }
    Some((-dy / l * r, dx / l * r))
}

fn disc(c: (f32, f32), r: f32) -> Vec<(f32, f32)> {
    let n = ((r * 1.6) as usize).clamp(10, 28);
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i as f32 / n as f32) * core::f32::consts::TAU;
        v.push((c.0 + r * cosf(t), c.1 + r * sinf(t)));
    }
    ensure_ccw(&mut v);
    v
}

/// Square cap: extend `end` (reached from `from`) outward by `r`.
fn square_cap(from: (f32, f32), end: (f32, f32), r: f32) -> Option<Vec<(f32, f32)>> {
    let dx = end.0 - from.0;
    let dy = end.1 - from.1;
    let l = sqrtf(dx * dx + dy * dy);
    if l < 1e-4 {
        return None;
    }
    let (ux, uy) = (dx / l, dy / l);
    let (nx, ny) = (-uy * r, ux * r);
    let (ex, ey) = (ux * r, uy * r);
    let mut q = alloc::vec![
        (end.0 + nx, end.1 + ny),
        (end.0 + nx + ex, end.1 + ny + ey),
        (end.0 - nx + ex, end.1 - ny + ey),
        (end.0 - nx, end.1 - ny),
    ];
    ensure_ccw(&mut q);
    Some(q)
}

/// Force a polygon to a consistent (positive-area) winding so nonzero unions.
fn ensure_ccw(p: &mut [(f32, f32)]) {
    let mut a = 0.0f32;
    for i in 0..p.len() {
        let j = (i + 1) % p.len();
        a += p[i].0 * p[j].1 - p[j].0 * p[i].1;
    }
    if a < 0.0 {
        p.reverse();
    }
}

// ── rasteriser: SS-vertical + analytic-horizontal scanline coverage ──────────

fn rasterize(w: u32, h: u32, fills: &[Fill]) -> Option<Vec<u8>> {
    let (wi, hi) = (w as usize, h as usize);
    let px_count = wi.checked_mul(hi)?;
    // premultiplied accumulation (r,g,b in 0..1 already * a, a in 0..1)
    let mut acc: Vec<[f32; 4]> = Vec::new();
    acc.try_reserve_exact(px_count).ok()?;
    acc.resize(px_count, [0.0; 4]);

    let mut cov = alloc::vec![0.0f32; wi]; // one pixel-row's coverage, reused

    for f in fills {
        // Build edges once.
        let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
        for sp in &f.subs {
            if sp.len() < 2 {
                continue;
            }
            for i in 0..sp.len() {
                let (x0, y0) = sp[i];
                let (x1, y1) = sp[(i + 1) % sp.len()];
                if y0 != y1 {
                    edges.push((x0, y0, x1, y1));
                }
            }
        }
        if edges.is_empty() {
            continue;
        }
        let (sr, sg, sb) = (f.r as f32 / 255.0, f.g as f32 / 255.0, f.b as f32 / 255.0);

        for py in 0..hi {
            for c in cov.iter_mut() {
                *c = 0.0;
            }
            let mut any = false;
            for k in 0..SS {
                let ys = py as f32 + (k as f32 + 0.5) / SS as f32;
                // crossings at ys
                let mut xs: Vec<(f32, i32)> = Vec::new();
                for &(x0, y0, x1, y1) in &edges {
                    let (lo, hi2) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                    if ys >= lo && ys < hi2 {
                        let t = (ys - y0) / (y1 - y0);
                        let x = x0 + t * (x1 - x0);
                        let dir = if y1 > y0 { 1 } else { -1 };
                        xs.push((x, dir));
                    }
                }
                if xs.len() < 2 {
                    continue;
                }
                xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
                let mut wind = 0;
                for i in 0..xs.len() - 1 {
                    wind += xs[i].1;
                    let inside = if f.evenodd { wind % 2 != 0 } else { wind != 0 };
                    if inside {
                        add_span(&mut cov, xs[i].0, xs[i + 1].0, 1.0 / SS as f32);
                        any = true;
                    }
                }
            }
            if !any {
                continue;
            }
            let row = py * wi;
            for px in 0..wi {
                let c = cov[px];
                if c <= 0.0 {
                    continue;
                }
                let sa = c.min(1.0) * f.a;
                if sa <= 0.0 {
                    continue;
                }
                let d = &mut acc[row + px];
                let ia = 1.0 - sa;
                d[0] = sr * sa + d[0] * ia;
                d[1] = sg * sa + d[1] * ia;
                d[2] = sb * sa + d[2] * ia;
                d[3] = sa + d[3] * ia;
            }
        }
    }

    // premultiplied f32 → straight BGRA u8
    let mut bgra: Vec<u8> = Vec::new();
    bgra.try_reserve_exact(px_count * 4).ok()?;
    for p in &acc {
        let a = p[3].clamp(0.0, 1.0);
        let (r, g, b) = if a > 0.0001 {
            ((p[0] / a).clamp(0.0, 1.0), (p[1] / a).clamp(0.0, 1.0), (p[2] / a).clamp(0.0, 1.0))
        } else {
            (0.0, 0.0, 0.0)
        };
        bgra.push((b * 255.0 + 0.5) as u8);
        bgra.push((g * 255.0 + 0.5) as u8);
        bgra.push((r * 255.0 + 0.5) as u8);
        bgra.push((a * 255.0 + 0.5) as u8);
    }
    Some(bgra)
}

fn add_span(cov: &mut [f32], xa: f32, xb: f32, weight: f32) {
    let w = cov.len() as f32;
    let xa = xa.clamp(0.0, w);
    let xb = xb.clamp(0.0, w);
    if xb <= xa {
        return;
    }
    let p0 = floorf(xa) as usize;
    let p1 = (ceilf(xb) as usize).min(cov.len());
    for px in p0..p1 {
        let l = (px as f32).max(xa);
        let r = ((px + 1) as f32).min(xb);
        if r > l {
            cov[px] += (r - l) * weight;
        }
    }
}

// ── attribute helpers + value parsers ────────────────────────────────────────

fn attr(el: &XmlEl, name: &str) -> Option<String> {
    el.attrs.iter().find(|(k, _)| local_name(k) == name).map(|(_, v)| v.clone())
}

/// Strip an XML namespace prefix (`svg:rect` → `rect`, `xlink:href` → `href`).
fn local_name(name: &str) -> &str {
    match name.rsplit_once(':') {
        Some((_, l)) => l,
        None => name,
    }
}

/// Parse a length: leading number, ignore a `px` unit; `%` → None (v1).
fn parse_len(v: &str) -> Option<f32> {
    let v = v.trim();
    if v.ends_with('%') {
        return None;
    }
    let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')).unwrap_or(v.len());
    v[..end].parse::<f32>().ok()
}

fn parse_view_box(v: String) -> Option<(f32, f32, f32, f32)> {
    let mut sc = NumScan::new(&v);
    let a = sc.num_if()?;
    let b = sc.num_if()?;
    let c = sc.num_if()?;
    let d = sc.num_if()?;
    Some((a, b, c, d))
}

fn parse_points(v: &str) -> Vec<(f32, f32)> {
    let mut sc = NumScan::new(v);
    let mut out = Vec::new();
    while let Some((x, y)) = sc.pair_if_num() {
        out.push((x, y));
    }
    out
}

fn parse_transform(v: &str) -> Mat {
    let mut m = Mat::id();
    let bytes = v.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // read a function name
        while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        let ns = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        if ns == i {
            break;
        }
        let name = &v[ns..i];
        // find (...)
        while i < bytes.len() && bytes[i] != b'(' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let open = i + 1;
        while i < bytes.len() && bytes[i] != b')' {
            i += 1;
        }
        let args_str = &v[open..i.min(v.len())];
        i += 1; // past ')'
        let mut sc = NumScan::new(args_str);
        let mut a: Vec<f32> = Vec::new();
        while let Some(n) = sc.num_if() {
            a.push(n);
        }
        let t = match name {
            "translate" => Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: a.first().copied().unwrap_or(0.0), f: a.get(1).copied().unwrap_or(0.0) },
            "scale" => {
                let sx = a.first().copied().unwrap_or(1.0);
                let sy = a.get(1).copied().unwrap_or(sx);
                Mat { a: sx, b: 0.0, c: 0.0, d: sy, e: 0.0, f: 0.0 }
            }
            "rotate" => {
                let deg = a.first().copied().unwrap_or(0.0);
                let r = deg * core::f32::consts::PI / 180.0;
                let (cr, srot) = (cosf(r), sinf(r));
                let rot = Mat { a: cr, b: srot, c: -srot, d: cr, e: 0.0, f: 0.0 };
                if a.len() >= 3 {
                    let (cx, cy) = (a[1], a[2]);
                    let t1 = Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: cx, f: cy };
                    let t2 = Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: -cx, f: -cy };
                    t1.mul(&rot).mul(&t2)
                } else {
                    rot
                }
            }
            "skewX" => {
                let t = tanf(a.first().copied().unwrap_or(0.0) * core::f32::consts::PI / 180.0);
                Mat { a: 1.0, b: 0.0, c: t, d: 1.0, e: 0.0, f: 0.0 }
            }
            "skewY" => {
                let t = tanf(a.first().copied().unwrap_or(0.0) * core::f32::consts::PI / 180.0);
                Mat { a: 1.0, b: t, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
            }
            "matrix" if a.len() >= 6 => Mat { a: a[0], b: a[1], c: a[2], d: a[3], e: a[4], f: a[5] },
            _ => Mat::id(),
        };
        m = m.mul(&t);
    }
    m
}

/// SVG number scanner: whitespace/comma separated, honours implicit separators
/// (`-`/`+` starting a new number, a second `.` starting a new number).
struct NumScan<'a> {
    b: &'a [u8],
    i: usize,
}
impl<'a> NumScan<'a> {
    fn new(s: &'a str) -> Self {
        NumScan { b: s.as_bytes(), i: 0 }
    }
    fn skip_sep(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b' ' || c == b',' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.i += 1;
            } else {
                break;
            }
        }
    }
    fn peek_cmd(&mut self) -> Option<u8> {
        self.skip_sep();
        if self.i < self.b.len() {
            Some(self.b[self.i])
        } else {
            None
        }
    }
    fn peek_is_cmd(&self) -> bool {
        self.i < self.b.len() && self.b[self.i].is_ascii_alphabetic()
    }
    fn bump(&mut self) {
        self.i += 1;
    }
    fn num_if(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.i;
        let mut seen_dot = false;
        let mut seen_digit = false;
        let mut j = self.i;
        if j < self.b.len() && (self.b[j] == b'+' || self.b[j] == b'-') {
            j += 1;
        }
        while j < self.b.len() {
            let c = self.b[j];
            if c.is_ascii_digit() {
                seen_digit = true;
                j += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                j += 1;
            } else if (c == b'e' || c == b'E') && seen_digit {
                j += 1;
                if j < self.b.len() && (self.b[j] == b'+' || self.b[j] == b'-') {
                    j += 1;
                }
            } else {
                break;
            }
        }
        if !seen_digit {
            return None;
        }
        let s = core::str::from_utf8(&self.b[start..j]).ok()?;
        let val = s.parse::<f32>().ok()?;
        self.i = j;
        Some(val)
    }
    fn pair(&mut self) -> Option<(f32, f32)> {
        let x = self.num_if()?;
        let y = self.num_if()?;
        Some((x, y))
    }
    fn pair_if_num(&mut self) -> Option<(f32, f32)> {
        let save = self.i;
        match self.pair() {
            Some(p) => Some(p),
            None => {
                self.i = save;
                None
            }
        }
    }
    fn pair2_if(&mut self) -> Option<((f32, f32), (f32, f32))> {
        let save = self.i;
        let a = self.pair();
        let b = self.pair();
        match (a, b) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => {
                self.i = save;
                None
            }
        }
    }
    fn triple_if(&mut self) -> Option<((f32, f32), (f32, f32), (f32, f32))> {
        let save = self.i;
        match (self.pair(), self.pair(), self.pair()) {
            (Some(a), Some(b), Some(c)) => Some((a, b, c)),
            _ => {
                self.i = save;
                None
            }
        }
    }
    fn arc_if(&mut self) -> Option<(f32, f32, f32, bool, bool, f32, f32)> {
        let save = self.i;
        let res = (|| {
            let rx = self.num_if()?;
            let ry = self.num_if()?;
            let rot = self.num_if()?;
            let large = self.flag()?;
            let sweep = self.flag()?;
            let x = self.num_if()?;
            let y = self.num_if()?;
            Some((rx, ry, rot, large, sweep, x, y))
        })();
        if res.is_none() {
            self.i = save;
        }
        res
    }
    /// Arc flags are a single `0`/`1` with no separator required after them.
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        if self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b'0' {
                self.i += 1;
                return Some(false);
            } else if c == b'1' {
                self.i += 1;
                return Some(true);
            }
        }
        None
    }
}

// ── minimal XML reader ───────────────────────────────────────────────────────

struct XmlEl {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<XmlNode>,
}
enum XmlNode {
    El(XmlEl),
    #[allow(dead_code)]
    Text(String),
}

fn parse_xml(src: &str) -> Option<XmlEl> {
    let b = src.as_bytes();
    let mut i = 0;
    // stack of elements under construction
    let mut stack: Vec<XmlEl> = Vec::new();
    let mut root: Option<XmlEl> = None;

    while i < b.len() {
        if b[i] == b'<' {
            // markup
            if src[i..].starts_with("<!--") {
                if let Some(end) = src[i..].find("-->") {
                    i += end + 3;
                } else {
                    break;
                }
                continue;
            }
            if src[i..].starts_with("<![CDATA[") {
                let s = i + 9;
                if let Some(end) = src[s..].find("]]>") {
                    let text = &src[s..s + end];
                    if let Some(top) = stack.last_mut() {
                        top.children.push(XmlNode::Text(text.to_string()));
                    }
                    i = s + end + 3;
                } else {
                    break;
                }
                continue;
            }
            if src[i..].starts_with("<?") {
                if let Some(end) = src[i..].find("?>") {
                    i += end + 2;
                } else {
                    break;
                }
                continue;
            }
            if src[i..].starts_with("<!") {
                // doctype — skip to matching '>', accounting for an internal subset [ ]
                let mut j = i + 2;
                let mut depth = 0i32;
                while j < b.len() {
                    match b[j] {
                        b'[' => depth += 1,
                        b']' => depth -= 1,
                        b'>' if depth <= 0 => {
                            j += 1;
                            break;
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            if src[i..].starts_with("</") {
                // close tag
                let end = src[i..].find('>')? + i;
                let name = src[i + 2..end].trim();
                // pop until matching (tolerant)
                while let Some(top) = stack.pop() {
                    let matched = local_name(&top.tag) == local_name(name);
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(XmlNode::El(top));
                    } else {
                        root = Some(top);
                    }
                    if matched {
                        break;
                    }
                }
                i = end + 1;
                continue;
            }
            // open tag
            let end = src[i..].find('>')? + i;
            let mut inner = &src[i + 1..end];
            let self_close = inner.ends_with('/');
            if self_close {
                inner = &inner[..inner.len() - 1];
            }
            let (tag, attrs) = parse_open_tag(inner);
            let el = XmlEl { tag, attrs, children: Vec::new() };
            if self_close {
                if let Some(top) = stack.last_mut() {
                    top.children.push(XmlNode::El(el));
                } else {
                    root = Some(el);
                }
            } else {
                stack.push(el);
            }
            i = end + 1;
        } else {
            // text run
            let end = src[i..].find('<').map(|p| p + i).unwrap_or(b.len());
            let text = src[i..end].trim();
            if !text.is_empty() {
                if let Some(top) = stack.last_mut() {
                    top.children.push(XmlNode::Text(decode_entities(text)));
                }
            }
            i = end;
        }
    }
    // unwind any unclosed
    while let Some(top) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(XmlNode::El(top));
        } else {
            root = Some(top);
        }
    }
    root
}

fn parse_open_tag(inner: &str) -> (String, Vec<(String, String)>) {
    let inner = inner.trim();
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() && !b[i].is_ascii_whitespace() {
        i += 1;
    }
    let tag = inner[..i].to_string();
    let mut attrs = Vec::new();
    while i < b.len() {
        // skip ws
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let ns = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        if ns == i {
            break;
        }
        let name = inner[ns..i].to_string();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                let val = decode_entities(&inner[vs..i.min(inner.len())]);
                if i < b.len() {
                    i += 1; // past closing quote
                }
                attrs.push((name, val));
            } else {
                attrs.push((name, String::new()));
            }
        } else {
            attrs.push((name, String::new()));
        }
    }
    (tag, attrs)
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        if let Some(semi) = rest.find(';') {
            let ent = &rest[1..semi];
            let ch = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            match ch {
                Some(c) => {
                    out.push(c);
                    rest = &rest[semi + 1..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(img: &Image) -> usize {
        img.bgra.chunks_exact(4).filter(|p| p[3] > 0).count()
    }

    #[test]
    fn rect_fills() {
        let svg = br##"<svg viewBox="0 0 100 100" width="100" height="100"><rect x="10" y="10" width="80" height="80" fill="#f00"/></svg>"##;
        let img = render(svg).expect("render");
        assert_eq!((img.w, img.h), (100, 100));
        let painted = ink(&img);
        assert!(painted > 5000, "expected a filled rect, got {painted} px");
        // a centre pixel should be red
        let ci = (((50 * 100) + 50) * 4) as usize;
        assert!(img.bgra[ci + 2] > 200 && img.bgra[ci + 1] < 60, "centre should be red");
        // a corner pixel should be transparent
        let corner = 0;
        assert_eq!(img.bgra[corner + 3], 0, "corner should be transparent");
    }

    #[test]
    fn circle_and_path() {
        let svg = br#"<svg viewBox="0 0 20 20"><circle cx="10" cy="10" r="8" fill="green"/></svg>"#;
        let img = render(svg).expect("render");
        assert!(ink(&img) > 100, "circle should paint");
    }

    #[test]
    fn path_triangle_with_transform() {
        let svg = br#"<svg width="50" height="50" viewBox="0 0 50 50">
          <g transform="translate(5,5)"><path d="M0 0 L40 0 L20 40 Z" fill="blue"/></g></svg>"#;
        let img = render(svg).expect("render");
        assert!(ink(&img) > 200, "triangle should paint");
    }

    #[test]
    fn stroke_only_paints() {
        // A fill:none stroked path (Feather/Phosphor style) must render.
        let svg = br#"<svg viewBox="0 0 100 100"><path d="M10 50 L90 50" fill="none" stroke="black" stroke-width="8" stroke-linecap="round"/></svg>"#;
        let img = render(svg).expect("render");
        let painted = img.bgra.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(painted > 300, "stroked line should paint, got {painted}");
        // midline pixel is inked, far-corner is not
        let mid = (((50 * img.w) + 50) * 4) as usize;
        assert!(img.bgra[mid + 3] > 0, "stroke centre inked");
        assert_eq!(img.bgra[3], 0, "corner transparent");
    }

    #[test]
    fn detects_svg() {
        assert!(looks_like_svg(b"  <?xml version=\"1.0\"?><svg></svg>"));
        assert!(looks_like_svg(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"));
        assert!(!looks_like_svg(b"\x89PNG\r\n"));
    }
}
