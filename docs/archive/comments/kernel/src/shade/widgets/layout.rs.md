# `kernel/src/shade/widgets/layout.rs` @ 5e0102684

## L1-32 · `#![allow(dead_code)]`

```
//! Widget layout — flexbox-lite.
//!
//! Takes a deserialized `Widget` tree + a container rect, returns a
//! parallel `LayoutNode` tree where every node carries an **absolute**
//! `Rect` in window coordinates. The rasterizer (P10.5) walks both
//! trees in lockstep.
//!
//! Strict subset of flexbox (no floats, no percent units, no absolute
//! positioning, no z-index beyond `Stack`):
//!
//!   - Row / Column with `spacing` + `align: Start|Center|End|Stretch`
//!   - `Spacer { flex: u8 }` eats remaining main-axis space
//!   - `Padding(n)` shrinks the inner content rect by `2n` on both axes
//!   - `Margin(n)` is reserved for v2 — logged but ignored in v1
//!   - Text measurement uses real Inter Variable metrics via `gui::text`
//!     (no stubs). Line-height comes from the `hhea` table.
//!   - Reserved widget slots (Popover/Tooltip/Menu) lay out as a zero-
//!     sized placeholder — the compositor logs + rejects them.
//!
//! All sizes are **logical px at 1× HiDPI**. The rasterizer multiplies
//! by the scale factor at raster time (per docs/archive/PHASE10_WIDGETS.md).
//!
//! Two-pass algorithm — cheap, fits on the stack:
//!
//!   Pass 1 (`measure`): recursively compute each node's intrinsic
//!                       (width, height). Text asks `gui::text::measure`;
//!                       Row/Column sum/max their children with spacing.
//!
//!   Pass 2 (`place`):   assign absolute rects top-down. Container
//!                       distributes remaining main-axis space among
//!                       `Spacer`s by flex weight; cross-axis alignment
//!                       follows `align`.
```

## L44-45 · `#[derive(Debug, Clone)]`

```
/// Geometry result for one widget node. Mirrors the widget tree shape
/// so callers (debug printer, rasterizer) can walk in lockstep.
```

## L48 · `pub rect:     Rect,`

```
/// Absolute rect in window coordinates (logical px).
```

## L50-52 · `pub baseline: u32,`

```
/// Distance from `rect.y` down to the text baseline — used for
/// multi-style row alignment (Title + Body in one Row → aligned on
/// the same baseline). Zero for non-text leaves.
```

## L54 · `pub children: Vec<LayoutNode>,`

```
/// Per-child layout, same order as the widget's children.
```

## L67-74 · `#[derive(Debug, Clone)]`

```
/// Floating overlay laid out at the end of the main pass.
/// `anchor_rect` is captured at lookup time so the hit-tester knows
/// which screen region to treat as "still inside the popover" for
/// dismissal purposes (clicks on the anchor should NOT dismiss —
/// the anchor's own OnClick handles toggle). `child` holds a clone
/// of the popover's content widget so the rasterizer + click router
/// can walk it without re-finding the source `Widget::Popover` in
/// the main tree.
```

## L83-84 · `#[derive(Debug, Clone)]`

```
/// Output of a full layout pass — main tree plus floating overlays
/// plus the NodeId→Rect lookup table the popovers used.
```

## L90-93 · `pub max_scroll_y: u32,`

```
/// Largest scroll offset any vertical `Widget::Scroll` in the tree
/// can take (content height − viewport height). The compositor clamps
/// the window's stored scroll offset to this so a wheel-up at the
/// bottom responds immediately. Zero → nothing scrolls.
```

## L95-96 · `pub max_scroll_rect: Rect,`

```
/// Screen rect of the scrollable viewport (for the scrollbar hit-test /
/// drag). Zero-sized when nothing scrolls.
```

## L98-100 · `pub max_scroll_x: u32,`

```
/// Sideways counterpart: how far the widest `TextArea` line overruns its
/// text column. Only an editor scrolls sideways — a `Widget::Scroll`
/// resolves its own horizontal axis during placement.
```

## L102 · `pub max_scroll_x_rect: Rect,`

```
/// The text column that offset scrolls in (bar track + hit-test).
```

## L106-109 · `struct ScrollCtx {`

```
/// Threaded through placement so a `Widget::Scroll` can offset its child
/// by the window's current scroll amount and report back the maximum
/// legal offset. One offset per window (applied to every vertical
/// Scroll); apps in practice have a single scroll region.
```

## L113-114 · `max_rect: Rect,`

```
/// Screen rect of the scrollable node that set `max` (the viewport) —
/// the compositor hit-tests its right edge for scrollbar dragging.
```

## L116-117 · `max_x:      u32,`

```
/// Sideways overflow of the widest TextArea, and the text column it
/// scrolls in (hit-tested along its bottom edge).
```

## L124-127 · `pub fn layout(root: &Widget, container: Rect) -> LayoutOutput {`

```
/// Lay out `root` inside `container` (absolute px). Returns the
/// main layout tree, a NodeId→Rect lookup, and any floating popover
/// overlays positioned via anchor lookups. `scroll_y` shifts vertical
/// `Widget::Scroll` content up by that many pixels (wheel scroll).
```

## L133-138 · `let mut ctx = ScrollCtx {`

```
// Pass 1: main tree. `place` unpacks the root's own padding itself —
// stripping it here first applied it TWICE, which shrank the root's
// rect by 2× its padding. On a full-window app that just cost a few
// px of content; on a panel it cut visibly into the card, because the
// root's Background paints on that rect (a 36 px bar rendered 28 px
// tall and sat 5 px too low).
```

## L148-150 · `let mut anchors: BTreeMap<u32, Rect> = BTreeMap::new();`

```
// Pass 2: walk widget+layout in lockstep, record NodeId-tagged
// rects so popovers (which always come after their anchor in
// tree order — apps' contract) can look them up.
```

## L154-156 · `let mut popovers: Vec<PopoverLayout> = Vec::new();`

```
// Pass 3: place every popover in the tree as a floating overlay.
// A popover whose anchor isn't in the table is silently dropped
// (nothing to attach to).
```

## L164-165 · `fn record_anchors(`

```
/// Walk widget+layout trees in lockstep, recording (NodeId, rect)
/// pairs for any widget carrying `Modifier::NodeId`.
```

## L182-185 · `fn collect_popovers(`

```
/// Walk the widget tree, find every Widget::Popover, look up its
/// anchor rect, and lay out its child as a floating overlay below
/// the anchor. Apps' contract: declare a Popover only AFTER its
/// anchor in tree order so the lookup succeeds.
```

## L194-195 · `let csize = measure(child);`

```
// Measure child's intrinsic size, then position it just
// below the anchor. Flip above when there's no room.
```

## L204 · `let max_x = window.x + window.w as i32 - csize.w as i32;`

```
// Horizontally, anchor-left clamped to window-right.
```

## L208 · `let mut pctx = ScrollCtx {`

```
// Popover content is floating chrome — never scrolled.
```

## L220-222 · `return;`

```
// Popover's own children stop here — the child is placed via
// floating layout above; recursion below skips the wrapped
// tree to avoid double-placing.
```

## L230 · `fn widget_children(w: &Widget) -> &[Widget] {`

```
/// Children of a widget — used by anchor + popover walkers.
```

## L243 · `pub(super) const SLIDER_THUMB: u32 = 14;`

```
/// Thumb diameter of a `Widget::Slider`, and with it the slider's height.
```

## L246 · `fn measure(w: &Widget) -> Size {`

```
// ── Pass 1: intrinsic measurement ─────────────────────────────────────
```

## L248-253 · `fn measure(w: &Widget) -> Size {`

```
/// Compute the node's preferred size with no container constraints.
/// `Spacer` reports (0, 0) here — flex distribution happens in `place`.
///
/// MinWidth/MaxWidth modifiers clamp the width *after* intrinsic
/// measurement so containers see the constrained value during flex
/// distribution.
```

## L279-281 · `fn flex_modifier(mods: &[Modifier]) -> Option<u8> {`

```
/// Return the highest `Modifier::Flex(n)` weight on `mods`, or `None`
/// if no Flex modifier is present. `Flex(0)` is treated as "no flex"
/// (matches the SDK doc and avoids zero-weight bookkeeping).
```

## L293-296 · `pub(super) fn font_size_of(style: TextStyle, mods: &[Modifier]) -> u16 {`

```
/// Pixel size a text leaf renders at: `Modifier::FontSize` when present
/// (clamped — a module must not be able to ask for a 60 000 px glyph),
/// else whatever the style resolves to. Layout and render must agree on
/// this, so both go through here.
```

## L380-384 · `let cs = measure(child);`

```
// A scroll container does NOT demand its child's full extent on
// the scroll axis — that's the whole point: it lets a flex
// parent size it to the viewport and clips/scrolls the overflow.
// Report the child's cross size, but only a small floor on the
// scroll axis so siblings (e.g. a footer) keep their space.
```

## L398-399 · `let w = if w == 0 {`

```
// Fallbacks for when font isn't loaded — line_height returns
// size_px * 1.2, measure returns 0 → conservative 6 px/char.
```

## L403-408 · `let pad = padding(modifiers);`

```
// Honour a Padding modifier on the leaf so the OUTER rect
// grows to include the padding band — siblings then space
// out correctly. Render-side paints glyphs at the inner
// rect (post-padding) via paint_node_eff. Without this,
// `prefab::menu_bar` and `prefab::badge` (Text + Padding)
// were rendered with siblings touching their glyph edges.
```

## L420-422 · `use super::abi::IconId;`

```
// label + 8 px horizontal padding on each side; icon (if any)
// precedes the label with 4 px gap. Matches typical toolkit
// button chrome.
```

## L432 · `w: icon_slot + label_w + 16,  // 8 px pad × 2`

```
// 8 px pad × 2
```

## L433 · `h: label_h.max(16) + 8,       // 4 px vertical pad × 2`

```
// 4 px vertical pad × 2
```

## L441-442 · `let pad = padding(modifiers);`

```
// Built-in 4 px chrome + the modifier's own padding. Min-
// width keeps empty inputs from collapsing in flex rows.
```

## L448-452 · `let px = font_size_of(TextStyle::Mono, modifiers);`

```
// Intrinsic floor only — the editing surface is meant to fill
// its parent. Apps wrap it in `Modifier::Flex(1)` (vertical
// stretch) inside an `Align::Stretch` column (horizontal
// stretch); the intrinsic size just keeps it from collapsing
// to nothing in a degenerate layout.
```

## L464-465 · `Widget::Slider { modifiers, .. } => {`

```
// Height = the thumb, so the whole strip around a thin track is
// grabbable. Width is a floor; sliders are meant to Flex/stretch.
```

## L479-480 · `Widget::Popover { .. } | Widget::Tooltip { .. } | Widget::Menu { .. } => {`

```
// Reserved slots — compositor rejects at render time; layout
// reserves zero space so mixed trees still measure consistently.
```

## L487 · `fn place(w: &Widget, inner: Rect, ctx: &mut ScrollCtx) -> LayoutNode {`

```
// ── Pass 2: placement ─────────────────────────────────────────────────
```

## L489-490 · `fn place(w: &Widget, inner: Rect, ctx: &mut ScrollCtx) -> LayoutNode {`

```
/// Place `w` inside `inner` (already-padded rect). Returns the absolute
/// layout tree rooted at `w`.
```

## L495 · `let mut node = place_axis(children, *spacing, *align, content, /* vertical = */ true, ctx);`

```
/* vertical = */
```

## L517-521 · `let csize = measure(child);`

```
// Scroll: child takes the container's cross size and its natural
// size on the scroll axis; the rasterizer clips to `inner`. For a
// vertical scroll we shift the child up by the window's scroll
// offset (clamped to content−viewport) and report that ceiling so
// the compositor can clamp the wheel.
```

## L552-558 · `Widget::TextArea { value, modifiers, .. } => {`

```
// TextArea + Canvas are placed as leaves, but unlike the others
// they FILL the rect the parent allotted (Flex / Align::Stretch
// already expanded `inner`) instead of shrinking to their
// intrinsic floor. A non-flex/non-stretch parent allots an
// intrinsic-sized rect, so the small-widget case is unchanged;
// an image viewer that Flex(1)-fills its body gets the whole
// area and the rasterizer contain-fits the bitmap into it.
```

## L560-563 · `let px = font_size_of(TextStyle::Mono, modifiers);`

```
// Report how far the editor can scroll (content lines beyond the
// viewport) so the compositor routes the wheel here; the render
// applies the offset + keeps the caret visible. Mirrors the
// render's `visible`/line-window math (Mono metrics, +4 inset).
```

## L570-572 · `let (col_x, col_w) = super::render::textarea_text_column(inner, modifiers, total);`

```
// Sideways: how far the widest line overruns the text column.
// Lines are never wrapped, so a long one simply runs past the
// right edge — that overrun IS the scrollable range.
```

## L596 · `LayoutNode::leaf(Rect { x: inner.x, y: inner.y, w: 0, h: 0 })`

```
// Spacers are zero-sized unless a Row/Column expands them.
```

## L605-606 · `LayoutNode::leaf(Rect { x: inner.x, y: inner.y, w: 0, h: 0 })`

```
// Reserved — rasterizer logs + rejects. Layout returns a
// zero rect at the container origin so dumps stay legible.
```

## L612 · `fn place_axis(`

```
/// Generic axis placement for Row/Column.
```

## L624-629 · `let mut measured: Vec<Size> = Vec::with_capacity(children.len());`

```
// Pass 1: measure children on the main axis, tally total intrinsic
// and total flex weight. Both `Spacer { flex }` and any non-Spacer
// child carrying `Modifier::Flex(n)` contribute weight; the
// difference is that Spacer's intrinsic is zero (so it's pure
// flex space) while Modifier::Flex children keep their intrinsic
// as a basis and only get the extra distributed share.
```

## L656-658 · `let mut kids = Vec::with_capacity(children.len());`

```
// Pass 2: walk children, place them along the main axis. Spacers
// and Modifier::Flex children absorb `remaining` proportionally
// to their flex weight; everyone else sticks to intrinsic.
```

## L666 · `let main_sz = match c {`

```
// Main-axis size for this child.
```

## L681 · `let cross_sz_intrinsic = if vertical { m.w } else { m.h };`

```
// Cross-axis size + offset based on align.
```

## L690 · `let child_rect = if vertical {`

```
// Compose child's allotted rect in absolute window coords.
```

## L717 · `fn unpack_modifiers(w: &Widget, container: Rect) -> (Rect, Rect) {`

```
// ── Modifier helpers ──────────────────────────────────────────────────
```

## L719-720 · `fn unpack_modifiers(w: &Widget, container: Rect) -> (Rect, Rect) {`

```
/// Read a widget's top-level modifiers once; return the outer rect it
/// claims + the inner rect its children occupy (outer minus padding).
```

## L742-743 · `fn unpack_modifiers_on(mods: &[Modifier], container: Rect) -> (Rect, Rect) {`

```
/// Apply Padding in `mods` to `container`, yielding (outer, inner).
/// Outer == container (Margin currently ignored); inner shrinks by 2×padding.
```

## L755 · `fn padding(mods: &[Modifier]) -> (u32, u32) {`

```
/// Sum of Padding modifiers → (x-pad, y-pad) in logical px.
```

## L774-776 · `fn ceil_u32(x: f32) -> u32 {`

```
/// `f32::ceil` isn't in core (no_std). Positive-only ceil-to-u32,
/// saturating on overflow/negatives. Used to round text widths + line
/// heights up so layout never under-reports size.
```

## L785-786 · `#[allow(dead_code)]`

```
// ── Silence unused warnings on helper types while the rest of the
//    pipeline (Point, Box) lands ────────────────────────────────────
```

