# `tools/wasm/beak-engine/tests/diag.rs` @ 5e0102684

## L1-2 · `use std::fs;`

```
//! Throwaway diagnostic — dump the display list for a WPT reftest + its ref.
//! DIAG=CSS2/height-012.xht cargo test --release --test diag -- --nocapture
```

## L36-39 · `if let Ok(hp) = std::env::var("DCSSIMG") {`

```
// DCSSIMG=<html> DCSS=<css> [DW=w] — for every element that WINS a
// background-image or mask-image, report what would stop us painting it:
// the display type (an inline box has no box decoration) and whether there
// is a background-colour for a mask to stencil.
```

## L89 · `if let Ok(fp) = std::env::var("DJPEG") {`

```
// DJPEG=<file> — decode one JPEG and print the decoder's own error.
```

## L111-113 · `if let Ok(cp) = std::env::var("DPOS") {`

```
// DPOS=<css> [DW=w] — dump the position/height/display/visibility cascade
// for a `.vector-dropdown-content` div nested under a `.vector-dropdown`,
// with a realistic ancestor chain (html.client-js …).
```

## L119-121 · `let dom = beak_engine::dom::parse(`

```
// ElemInfo borrows a live element now (so a selector can see children
// and element state), so the chain is built by parsing a snippet with
// the shape we want rather than by hand-filling a struct.
```

## L133 · `let mut chain: Vec<&beak_engine::dom::Element> = Vec::new();`

```
// root → … → parent, then the subject and its preceding siblings.
```

## L168-170 · `if let Ok(hp) = std::env::var("DOPS") {`

```
// DOPS=<html> DCSS=<css> DW=<w> — the WHOLE display list, in paint order.
// Use when a widget is visibly wrong and you need to see which rect is the
// stray one, not just where the text landed.
```

## L197-198 · `if let Ok(hp) = std::env::var("DDUMP") {`

```
// DDUMP=<html> DCSS=<css> DW=<w> — dump every TEXT op with its y, plus the
// page height. Tells you where a marker text lands (push-down debugging).
```

## L216 · `if let Ok(hp) = std::env::var("DWIDTHS") {`

```
// DWIDTHS=<html> DCSS=<css> DW=<w> — trace box widths / overflow.
```

## L241-242 · `if let Ok(cp) = std::env::var("DVARS") {`

```
// Parse a CSS file and report whether a given class's declarations survive.
// DGRID=<css> [DCLASS=mw-page-container-inner] [DW=1200] cargo test --test diag
```

## L248 · `let mut nums: Vec<f64> = Vec::new();`

```
// report the biggest bare numbers in the output
```

## L272-273 · `let cls = class.split(',').map(|s| s.trim()).collect::<Vec<_>>().join(" ");`

```
// ElemInfo borrows a live element — build one instead of filling a
// struct by hand.
```

## L313-315 · `if let Ok(hp) = std::env::var("DCTRL") {`

```
// Render a real fetched page (HTML file + concatenated CSS file) to a BMP.
// DCTRL=<html> DCSS=<css> DW=<w> — list the page's form controls (rect,
// kind, and the text painted inside each) + what a submit would send.
```

## L339-342 · `if let Ok(hp) = std::env::var("DIMG") {`

```
// DIMG=<html> DCSS=<css> DIMGDIR=<dir> DW=<w> DOUT=<bmp> — render a real
// page WITH its images, to check that decoded pixels actually reach the
// canvas (the device showed grey placeholder boxes). Files in DIMGDIR are
// named after the src with '/' and ':' replaced by '_'.
```

## L349-353 · `eng.set_theme(if std::env::var("DTHEME").as_deref() == Ok("dark") {`

```
// DTHEME=dark reproduces what the device does when the compositor
// palette is dark: the PAGE stays whatever colour its CSS says, but
// anything we derive from the theme (form-control chrome, placeholder
// text, the default text colour) flips. Device-only colour reports are
// otherwise impossible to reproduce here.
```

## L394-395 · `let masks = lay.ops.iter().filter(|o| matches!(o, beak_engine::layout::DrawOp::BgImage { tint: Some(_), .. })).count();`

```
// CSS images (background-image / mask-image), split by how they are
// sourced: data: URIs need no network, the rest is the fetch backlog.
```

## L400-402 · `let mut css_ok = 0;`

```
// Feed the CSS-image backlog from DIMGDIR too, the way the shell would.
// A background cannot move a box, so this needs no relayout — the ops
// are already in the list, only their pixels were missing.
```

## L412 · `let mut rects: Vec<(i32,i32,i32,i32,beak_engine::layout::Rgba)> = lay.ops.iter().filter_map(|o| match o {`

```
// The tallest filled rects — an enormous empty box shows up here.
```

## L455-457 · `if let Ok(hp) = std::env::var("DTIME") {`

```
// DTIME=<html> DCSS=<css> DW=<width> [DN=<runs>] — how long do parse+
// cascade+layout and paint actually take? Native, so it is a LOWER bound
// for the device, which runs the same code under the wasmi interpreter.
```

## L469-471 · `let t_dom = std::time::Instant::now();`

```
// Break the "layout" number into its three real phases — on the
// device this whole call is 13 s, so knowing WHICH part decides
// what to fix.
```

## L502-506 · `if let Ok(hp) = std::env::var("DPAINT") {`

```
// DPAINT=<html> DCSS=<css> DW=<width> DVH=<viewport height> — measure the
// paint the DEVICE actually does: one viewport-sized buffer, repainted at a
// series of scroll offsets, which is what a scroll costs. DTIME paints the
// whole document once, so it hides both the per-frame canvas clear and the
// fact that a scrolled frame still walks the entire display list.
```

## L516-519 · `let (mut nr, mut nt, mut ni, mut glyphs) = (0u64, 0u64, 0u64, 0u64);`

```
// What the display list asks the rasteriser to touch, per frame: how
// many ops it walks, and how many pixels the rects alone cover once
// clipped to the viewport. Overdraw > 1 viewport means the same pixel
// is written several times before the text lands on top.
```

## L530 · `println!("guessed image boxes: {}", lay.guessed_image_srcs.len());`

```
// Every guessed src costs a FULL re-layout when its pixels land.
```

## L537 · `let mut px = 0i64;`

```
// Rect pixels this frame would write, clipped to the viewport.
```

## L546-547 · `eng.paint(&lay, w, vh, scroll, &mut buf);`

```
// Warm the glyph cache first: the device keeps it across frames, so
// the steady-state scroll cost is what matters, not the first paint.
```

## L560 · `if let Ok(hp) = std::env::var("DPAGE") {`

```
// DPAGE=<html> DCSS=<css> DW=<width> DOUT=<bmp> cargo test --test diag
```

## L566-567 · `eng.set_theme(if std::env::var("DDARK").is_ok() {`

```
// DDARK=1 renders on the DARK palette — the device default, and the one
// difference that makes a page look fine here and black there.
```

## L578 · `let row = (w * 3 + 3) & !3;`

```
// BMP (24-bit, bottom-up) — same writer as lib.rs demos.
```

## L615 · `if let Some(i) = html.find("rel=\"match\"").or_else(|| html.find("rel='match'")).or_else(|| html.find("rel=match")) {`

```
// ref via rel=match
```

## L630 · `#[test]`

```
/// DRECT=<html> DW=<w> — dump every RECT op (backgrounds, borders, stripes).
```

## L652-655 · `#[test]`

```
/// DPHASE=<html> DCSS=<css> [DW=w] [DH=h] — split the one "parse+cascade+
/// layout" number the device reports into its three phases, and time what a
/// pure viewport-HEIGHT change actually costs. The dock bar shifting beak by a
/// few pixels re-ran all three on device (~6.4 s each, twice per hover).
```

## L674 · `let mut eng = Engine::new();`

```
// Whole-pipeline runs through the public entry, the way the app calls it.
```

## L683 · `let t = std::time::Instant::now();`

```
// Same size again: everything cached that can be.
```

## L688 · `eng.set_viewport_h(h - 40);`

```
// ONLY the viewport height changes — the dock-hover case.
```

## L703-707 · `#[test]`

```
/// DHOVER=<html> DCSS=<css> [DW=] — the census that decides how `:hover`
/// should invalidate: lay the page out at rest, then with the pointer on each
/// of a few real links, and count how many elements actually get a DIFFERENT
/// computed style. If that is a handful, targeted invalidation is the answer;
/// if it is hundreds, only a cheaper layout is.
```

## L733-734 · `let (vw, vh) = (w, 1000u32);`

```
// Probe a grid over the VISIBLE area — a box below the fold cannot change
// a pixel, and the first census wasted every probe that way.
```

## L785-786 · `fn pixels_differ(`

```
/// Paint both layouts and count differing pixels, plus their bounding box —
/// exactly what a damage-driven repaint would have to redraw.
```

## L815-822 · `fn op_full(op: &DrawOp) -> String {`

```
// ── DHOPS: the op-level hover census ───────────────────────────────────────
// The pixel census (DHOVER) said HOW MUCH changes. This says WHAT changes in
// the display list, which is what decides the shape of a paint-only path:
// if the op COUNT is stable and only colour fields move, a patch is enough;
// if ops appear/disappear, the list has to be re-emitted.
//
//   DHOPS=<html> DCSS=<css> [DW=1880] [DN=40]
//     cargo test --release --test diag hover_op_census -- --nocapture
```

## L824-825 · `fn op_full(op: &DrawOp) -> String {`

```
/// Everything about an op that the rasteriser reads, as text — so two ops
/// compare field-by-field without the engine needing `PartialEq`.
```

## L844-845 · `fn op_shape(op: &DrawOp) -> String {`

```
/// The part of an op that a paint-only change must NOT be able to move: kind
/// plus geometry. Two ops with the same shape differ only in appearance.
```

## L864-866 · `{`

```
// What do the sheet's `:hover` rules even DECLARE? A text census over the
// whole sheet is an upper bound (not every rule matches), but it is the
// cheap half of the answer and it names the properties to classify first.
```

## L874 · `let Some(open) = css[at..].find('{') else { break };`

```
// the selector this compound belongs to ends at the next `{`
```

## L877-878 · `if css[at..open].contains('}') || css[at..open].contains(';') {`

```
// …but only if no `}` or `;` intervenes (else the `:hover` was in
// a value or a comment, not a selector)
```

## L925 · `let (vw, vh) = (w, 1000u32);`

```
// Probe the visible area, same grid as DHOVER, but stop after DN hits.
```

## L944-945 · `eng.set_hover(Vec::new());`

```
// Lay the page out at rest, then hot, then PATCH the resting one
// and see whether it came out the same. That is the whole claim.
```

## L970-972 · `idle += 1;`

```
// The pointer is inside something a rule COULD match, but no
// rule applies: a full layout would produce the same list, so
// answering it without one is the whole point.
```

## L1003-1004 · `let al = lcs(&rest_shape, &hot_shape);`

```
// Align on GEOMETRY: ops that keep their kind+rect are the same box
// painted again. What is left over is a true insert or delete.
```

## L1012-1014 · `let moved = al.iter().any(|&(i, j)| rest_shape[i] != hot_shape[j]);`

```
// The engine's verdict against what the pixels actually did. An
// op that MOVED would prove the "paint only" claim wrong; ops
// added or removed at unchanged rects would not.
```

## L1103-1106 · `fn lcs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {`

```
/// Longest common subsequence as index pairs. The op lists are in document
/// order and edits are local, so this aligns "the same box, painted again"
/// against "an op that genuinely appeared" — an index-wise diff cannot, it
/// reports every op after an insertion as changed.
```

## L1134-1136 · `#[test]`

```
/// DHBOX=<html> DCSS=<css> [DW=] [DX= DY=] — dump the hover boxes that contain
/// a point, with their rects. A box whose rect contains the point but whose
/// paint is elsewhere means the hit-test geometry is wrong.
```

## L1152 · `let hit: Vec<u32> = lay.hover_at(x, y);`

```
// …and the same seqs' OTHER boxes, if any (an inline box spanning lines).
```

## L1162-1169 · `#[test]`

```
/// How much does the "is it even visible?" test before a repaint save?
///
/// DIMG=<page.html> DCSS=<page.css> DW=1902 DVH=1000 \
///   cargo test --release --test diag -- --nocapture img_visibility_census
///
/// Counts image boxes by where they sit: a batch that lands entirely below the
/// fold used to cost a full-viewport repaint (~50 ms on the device) and show
/// nothing.
```

## L1191-1194 · `println!("  guessed boxes  {:>4} of {} distinct srcs: {:?}",`

```
// A GUESSED box is the expensive kind: when its pixels land the page moves
// and the shell pays a FULL re-layout, wherever the image sits. On the
// device that was 1110-1710 ms on an article — more than the fetch and the
// repaints together.
```

## L1209-1210 · `let batch: usize = std::env::var("DBATCH").ok().and_then(|s| s.parse().ok()).unwrap_or(4);`

```
// The shell fetches in batches, in document order — so simulate the batches
// and count how many of them would have repainted for nothing.
```

