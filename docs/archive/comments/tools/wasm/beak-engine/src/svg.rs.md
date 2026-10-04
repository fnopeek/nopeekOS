# `tools/wasm/beak-engine/src/svg.rs` @ 5e0102684

## L1-14 · `use crate::color::parse_color;`

```
//! svg.rs — from-scratch SVG rasteriser for `<img src=*.svg>`.
//!
//! Parses SVG (own XML reader — SVG is case-sensitive and self-close heavy),
//! walks shapes/paths/groups with inherited presentation attrs + transforms,
//! maps `viewBox` → viewport (xMidYMid meet), and fills paths with a
//! supersampled scanline coverage rasteriser (nonzero / evenodd) into a
//! straight-BGRA `Image`. Colours reuse `color::parse_color` (named + Color 4).
//!
//! Fills, strokes and (as one flat average colour) gradients. Renders both a
//! standalone `<img src=*.svg>` and an inline `<svg>` straight out of the HTML
//! DOM — the latter through `render_element`, which takes the element's
//! computed `color` because `currentColor` is what icon sets paint with.
//! Still missing: `<use>`, real gradient interpolation, `clip-path` (an icon's
//! clip is its own box, so ignoring it is invisible; a clipping mask is not).
```

## L25 · `const MAX_PIXELS: usize = 1_000_000; // caps the f32 accumulation buffer (~16 MB)`

```
// caps the f32 accumulation buffer (~16 MB)
```

## L26 · `const SS: usize = 4; // vertical supersampling for anti-aliasing`

```
// vertical supersampling for anti-aliasing
```

## L28-34 · `pub fn adjust_tag_name(lower: &str) -> &str {`

```
// ── foreign-content name adjustment (HTML tree construction) ───────────────
//
// The HTML tokenizer lowercases every tag and attribute name. SVG is
// case-SENSITIVE, so the spec puts the camel case back when it inserts a
// foreign element ("adjust SVG tag names" / "adjust SVG attributes"). Without
// it `viewBox` arrives as `viewbox` and an inline icon has no coordinate
// system at all. The tables are the spec's, verbatim.
```

## L36-37 · `pub fn adjust_tag_name(lower: &str) -> &str {`

```
/// Lowercased SVG element name → its real spelling. Unlisted names are already
/// all-lowercase (`path`, `circle`, `g`, …) and pass through.
```

## L81 · `pub fn adjust_attr_name(lower: &str) -> &str {`

```
/// Lowercased SVG attribute name → its real spelling.
```

## L146-147 · `fn lookup<'a>(table: &[(&'static str, &'static str)], lower: &'a str) -> &'a str {`

```
/// The tables are sorted, so this is a binary search; an unlisted name is
/// returned unchanged (it is already spelled correctly in lower case).
```

## L151-152 · `table[i].1`

```
// SAFETY of the cast-free kind: the table value is 'static, which
// outlives 'a.
```

## L159-160 · `pub fn looks_like_svg(bytes: &[u8]) -> bool {`

```
/// Detect an SVG document: skip a UTF-8 BOM / leading whitespace / an XML
/// declaration / a doctype / comments, then look for `<svg`.
```

## L170-171 · `pub fn render(bytes: &[u8]) -> Option<Image> {`

```
/// Render SVG bytes into an `Image` (straight BGRA, alpha=0 where nothing is
/// painted so it composites over the page). `None` on parse failure / no size.
```

## L176 · `return None;`

```
// The document element must be <svg> (allow a namespace prefix).
```

## L179-180 · `render_tree(&root, Rgb(0, 0, 0), None)`

```
// A standalone document has no CSS around it, so `currentColor` is the
// initial `color`.
```

## L184-190 · `pub fn render_element(el: &crate::dom::Element, current: Rgb, box_px: Option<(u32, u32)>) -> Option<Image> {`

```
/// Render an INLINE `<svg>` straight out of the HTML DOM.
///
/// `current` is the element's computed `color` — `currentColor` is what icon
/// sets paint with, so without it every icon comes out black. `box_px` is the
/// used box from layout: an inline `<svg>` is sized by CSS, not by its own
/// `width`/`height` attributes, so the raster has to match that box or the
/// icon is drawn at the wrong scale.
```

## L195-197 · `fn from_dom(el: &crate::dom::Element) -> Option<XmlEl> {`

```
/// The HTML DOM and this module's XML tree hold the same data in the same
/// shape; an inline icon is a handful of nodes, so mirroring it beats making
/// every walker generic over two tree types.
```

## L220 · `let vb = attr(root, "viewBox").and_then(parse_view_box);`

```
// Intrinsic size + user→device root matrix.
```

## L225 · `(Some((bw, bh)), ..) if bw > 0 && bh > 0 => (bw as f32, bh as f32),`

```
// Layout already decided the box; the viewBox maps into it.
```

## L234 · `let mut w = dev_w;`

```
// Clamp raster size to bounds, keeping aspect.
```

## L248 · `let mut fills: Vec<Fill> = Vec::new();`

```
// Walk the tree into an ordered fill list.
```

## L264-265 · `}`

```
// Nothing drawable — still return a transparent box so the <img> box
// is sized (better than a placeholder for an empty/def-only SVG).
```

## L272 · `#[derive(Clone, Copy)]`

```
// ── affine matrix: x' = a*x + c*y + e ; y' = b*x + d*y + f ────────────────────
```

## L287 · `fn mul(&self, o: &Mat) -> Mat {`

```
/// `self.mul(o).apply(p) == self.apply(o.apply(p))` — `o` applied first.
```

## L301 · `fn scale_hint(&self) -> f32 {`

```
/// Approx uniform scale of this matrix (for adaptive curve flattening).
```

## L311 · `let s = (iw / vw).min(ih / vh);`

```
// xMidYMid meet: uniform scale, centre the shorter axis.
```

## L321 · `#[derive(Clone, Copy, PartialEq)]`

```
// ── presentation state (inherited) ───────────────────────────────────────────
```

## L342-343 · `struct SubPath {`

```
/// A flattened contour in device space; `closed` distinguishes joins vs caps
/// for stroking (fill closes every contour implicitly).
```

## L350 · `subs: Vec<Vec<(f32, f32)>>, // device-space polylines`

```
// device-space polylines
```

## L354 · `a: f32, // 0..1`

```
// 0..1
```

## L376 · `}`

```
// v1: not rendered (defs/gradients/use handled in a later iteration).
```

## L383 · `if let Some(fill) = paint.fill {`

```
// fill first (paints under the stroke)
```

## L386-388 · `let a = (paint.fill_opacity * paint.opacity * fill.a as f32 / 255.0).clamp(0.0, 1.0);`

```
// A colour's own alpha is a third opacity and multiplies
// with the other two — SVG already had `fill-opacity` and
// `opacity`, `rgba()` just adds one more.
```

## L392 · `if let Some(sc) = paint.stroke {`

```
// stroke on top
```

## L408-417 · `fn gradient_colors(el: &XmlEl, out: &mut Vec<(String, Rgb)>) {`

```
/// Average colour of every gradient in the document, by id.
///
/// v1 paints a gradient as ONE flat colour: the mean of its stops. A real
/// gradient needs per-pixel interpolation in the rasteriser; the flat stand-in
/// is what turns Wikipedia's logo from a black disc into a light sphere, and at
/// icon size the difference from the real thing is small.
///
/// Walks the tree — `walk` skips `<defs>`, but this pass visits everything, so
/// a gradient is found wherever it is declared. An inline `<svg>` has no source
/// text to scan at all.
```

## L450 · `fn attr_value(s: &str, name: &str) -> Option<String> {`

```
/// The value of `name="…"` in a raw tag slice.
```

## L472 · `fn style_value(s: &str, name: &str) -> Option<String> {`

```
/// The value of `name:` inside this tag's `style="…"`.
```

## L484 · `fn url_ref(v: &str) -> Option<&str> {`

```
/// `url(#id)` → the id it names.
```

## L492 · `p.opacity = parent.opacity; // opacity does NOT inherit; applied multiplicatively`

```
// opacity does NOT inherit; applied multiplicatively
```

## L493 · `let mut own_opacity = 1.0;`

```
// opacity is a property of THIS element only (reset), fill/fill-* inherit.
```

## L550 · `for (k, v) in &el.attrs {`

```
// Presentation attributes first, then style="" overrides.
```

## L573 · `fn shape_subpaths(el: &XmlEl, tag: &str, ctm: &Mat) -> Vec<SubPath> {`

```
// ── shapes → device-space subpaths ───────────────────────────────────────────
```

## L625 · `let a = ctm.apply(num("x1"), num("y1"));`

```
// no fill area, but strokeable
```

## L646 · `let corner = |sub: &mut Vec<(f32, f32)>, cx: f32, cy: f32, a0: f32| {`

```
// corner arcs sampled; quarter ellipse per corner (clockwise from top-left).
```

## L654 · `corner(&mut sub, x + rx, y + ry, core::f32::consts::PI); // TL`

```
// TL
```

## L655 · `corner(&mut sub, x + w - rx, y + ry, core::f32::consts::PI * 1.5); // TR`

```
// TR
```

## L656 · `corner(&mut sub, x + w - rx, y + h - ry, 0.0); // BR`

```
// BR
```

## L657 · `corner(&mut sub, x + rx, y + h - ry, core::f32::consts::FRAC_PI_2); // BL`

```
// BL
```

## L665 · `fn parse_path(d: &str, ctm: &Mat) -> Vec<SubPath> {`

```
// ── path data (M L H V C S Q T A Z, absolute + relative) ──────────────────────
```

## L673 · `let mut last_ctrl: Option<(f32, f32)> = None; // for S/T reflection (user space)`

```
// for S/T reflection (user space)
```

## L693 · `if sc.peek_is_cmd() {`

```
// A command letter is consumed only when explicit; repeated coords reuse it.
```

## L697 · `if cur_sub.is_empty() && up != b'M' && up != b'Z' {`

```
// A drawing command after Z (no M) restarts a subpath at the current point.
```

## L716 · `while let Some((mut lx, mut ly)) = sc.pair_if_num() {`

```
// subsequent implicit pairs are lineto
```

## L829 · `break;`

```
// Unknown command: stop rather than loop forever.
```

## L874 · `if rx == 0.0 || ry == 0.0 || (p0.0 == p1.0 && p0.1 == p1.1) {`

```
// SVG arc → centre parametrisation (impl notes F.6.5/F.6.6).
```

## L887 · `let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);`

```
// radius correction
```

## L938-943 · `fn stroke_polys(subs: &[SubPath], r: f32, cap: Cap) -> Vec<Vec<(f32, f32)>> {`

```
// ── stroke → fill (union of segment quads + round joins/caps, nonzero) ───────
//
// Each contour is widened to a set of convex pieces (segment rectangles, plus a
// disc at every join and round cap) — all wound the same way so a nonzero fill
// unions them cleanly, no outline-intersection maths. Joins/round-caps are
// exact; miter/bevel are approximated as round for v1 (the common icon style).
```

## L977 · `let (js, je) = if sp.closed { (0, n) } else { (1, n - 1) };`

```
// joins: a disc at each corner vertex (all corners if closed, interior if open)
```

## L1003 · `fn perp(a: (f32, f32), b: (f32, f32), r: f32) -> Option<(f32, f32)> {`

```
/// Perpendicular offset of length `r` for the segment a→b (None if degenerate).
```

## L1025 · `fn square_cap(from: (f32, f32), end: (f32, f32), r: f32) -> Option<Vec<(f32, f32)>> {`

```
/// Square cap: extend `end` (reached from `from`) outward by `r`.
```

## L1046 · `fn ensure_ccw(p: &mut [(f32, f32)]) {`

```
/// Force a polygon to a consistent (positive-area) winding so nonzero unions.
```

## L1058 · `fn rasterize(w: u32, h: u32, fills: &[Fill]) -> Option<Vec<u8>> {`

```
// ── rasteriser: SS-vertical + analytic-horizontal scanline coverage ──────────
```

## L1063 · `let mut acc: Vec<[f32; 4]> = Vec::new();`

```
// premultiplied accumulation (r,g,b in 0..1 already * a, a in 0..1)
```

## L1068 · `let mut cov = alloc::vec![0.0f32; wi]; // one pixel-row's coverage, reused`

```
// one pixel-row's coverage, reused
```

## L1071 · `let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();`

```
// Build edges once.
```

## L1097 · `let mut xs: Vec<(f32, i32)> = Vec::new();`

```
// crossings at ys
```

## L1145 · `let mut bgra: Vec<u8> = Vec::new();`

```
// premultiplied f32 → straight BGRA u8
```

## L1181 · `fn attr(el: &XmlEl, name: &str) -> Option<String> {`

```
// ── attribute helpers + value parsers ────────────────────────────────────────
```

## L1187 · `fn local_name(name: &str) -> &str {`

```
/// Strip an XML namespace prefix (`svg:rect` → `rect`, `xlink:href` → `href`).
```

## L1195 · `fn parse_len(v: &str) -> Option<f32> {`

```
/// Parse a length: leading number, ignore a `px` unit; `%` → None (v1).
```

## L1228 · `while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {`

```
// read a function name
```

## L1240 · `while i < bytes.len() && bytes[i] != b'(' {`

```
// find (...)
```

## L1252 · `i += 1; // past ')'`

```
// past ')'
```

## L1295-1296 · `struct NumScan<'a> {`

```
/// SVG number scanner: whitespace/comma separated, honours implicit separators
/// (`-`/`+` starting a new number, a second `.` starting a new number).
```

## L1417 · `fn flag(&mut self) -> Option<bool> {`

```
/// Arc flags are a single `0`/`1` with no separator required after them.
```

## L1434 · `struct XmlEl {`

```
// ── minimal XML reader ───────────────────────────────────────────────────────
```

## L1450 · `let mut stack: Vec<XmlEl> = Vec::new();`

```
// stack of elements under construction
```

## L1456 · `if src[i..].starts_with("<!--") {`

```
// markup
```

## L1487 · `let mut j = i + 2;`

```
// doctype — skip to matching '>', accounting for an internal subset [ ]
```

## L1506 · `let end = src[i..].find('>')? + i;`

```
// close tag
```

## L1509 · `while let Some(top) = stack.pop() {`

```
// pop until matching (tolerant)
```

## L1524 · `let end = src[i..].find('>')? + i;`

```
// open tag
```

## L1544 · `let end = src[i..].find('<').map(|p| p + i).unwrap_or(b.len());`

```
// text run
```

## L1555 · `while let Some(top) = stack.pop() {`

```
// unwind any unclosed
```

## L1576 · `while i < b.len() && b[i].is_ascii_whitespace() {`

```
// skip ws
```

## L1605 · `i += 1; // past closing quote`

```
// past closing quote
```

## L1675 · `let ci = (((50 * 100) + 50) * 4) as usize;`

```
// a centre pixel should be red
```

## L1678 · `let corner = 0;`

```
// a corner pixel should be transparent
```

## L1700 · `let svg = br#"<svg viewBox="0 0 100 100"><path d="M10 50 L90 50" fill="none" stroke="black" stroke-width="8" stroke-line`

```
// A fill:none stroked path (Feather/Phosphor style) must render.
```

## L1705 · `let mid = (((50 * img.w) + 50) * 4) as usize;`

```
// midline pixel is inked, far-corner is not
```

