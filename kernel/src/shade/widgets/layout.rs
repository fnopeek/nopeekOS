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
//! by the scale factor at raster time (per PHASE10_WIDGETS.md).
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

#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::abi::{
    ActionId, Align, Axis, Modifier, Point, Rect, Size, TextStyle, Widget,
};

/// Geometry result for one widget node. Mirrors the widget tree shape
/// so callers (debug printer, rasterizer) can walk in lockstep.
#[derive(Debug, Clone)]
pub struct LayoutNode {
    /// Absolute rect in window coordinates (logical px).
    pub rect:     Rect,
    /// Distance from `rect.y` down to the text baseline — used for
    /// multi-style row alignment (Title + Body in one Row → aligned on
    /// the same baseline). Zero for non-text leaves.
    pub baseline: u32,
    /// Per-child layout, same order as the widget's children.
    pub children: Vec<LayoutNode>,
}

impl LayoutNode {
    fn leaf(rect: Rect) -> Self {
        Self { rect, baseline: 0, children: Vec::new() }
    }
    fn empty() -> Self {
        Self { rect: Rect::default(), baseline: 0, children: Vec::new() }
    }
}

/// Floating overlay laid out at the end of the main pass.
/// `anchor_rect` is captured at lookup time so the hit-tester knows
/// which screen region to treat as "still inside the popover" for
/// dismissal purposes (clicks on the anchor should NOT dismiss —
/// the anchor's own OnClick handles toggle). `child` holds a clone
/// of the popover's content widget so the rasterizer + click router
/// can walk it without re-finding the source `Widget::Popover` in
/// the main tree.
#[derive(Debug, Clone)]
pub struct PopoverLayout {
    pub on_dismiss:  ActionId,
    pub anchor_rect: Rect,
    pub child:       Box<Widget>,
    pub layout:      LayoutNode,
}

/// Output of a full layout pass — main tree plus floating overlays
/// plus the NodeId→Rect lookup table the popovers used.
#[derive(Debug, Clone)]
pub struct LayoutOutput {
    pub tree:     LayoutNode,
    pub anchors:  BTreeMap<u32, Rect>,
    pub popovers: Vec<PopoverLayout>,
}

/// Lay out `root` inside `container` (absolute px). Returns the
/// main layout tree, a NodeId→Rect lookup, and any floating popover
/// overlays positioned via anchor lookups.
pub fn layout(root: &Widget, container: Rect) -> LayoutOutput {
    // Pass 1: main tree.
    let (_, inner) = unpack_modifiers(root, container);
    let tree = place(root, inner);

    // Pass 2: walk widget+layout in lockstep, record NodeId-tagged
    // rects so popovers (which always come after their anchor in
    // tree order — apps' contract) can look them up.
    let mut anchors: BTreeMap<u32, Rect> = BTreeMap::new();
    record_anchors(root, &tree, &mut anchors);

    // Pass 3: place every popover in the tree as a floating overlay.
    // A popover whose anchor isn't in the table is silently dropped
    // (nothing to attach to).
    let mut popovers: Vec<PopoverLayout> = Vec::new();
    collect_popovers(root, container, &anchors, &mut popovers);

    LayoutOutput { tree, anchors, popovers }
}

/// Walk widget+layout trees in lockstep, recording (NodeId, rect)
/// pairs for any widget carrying `Modifier::NodeId`.
fn record_anchors(
    w: &Widget,
    n: &LayoutNode,
    out: &mut BTreeMap<u32, Rect>,
) {
    for m in mods_of_widget(w) {
        if let Modifier::NodeId(id) = m {
            out.insert(id.0, n.rect);
        }
    }
    let kids = widget_children(w);
    for (cw, cl) in kids.iter().zip(n.children.iter()) {
        record_anchors(cw, cl, out);
    }
}

/// Walk the widget tree, find every Widget::Popover, look up its
/// anchor rect, and lay out its child as a floating overlay below
/// the anchor. Apps' contract: declare a Popover only AFTER its
/// anchor in tree order so the lookup succeeds.
fn collect_popovers(
    w: &Widget,
    window: Rect,
    anchors: &BTreeMap<u32, Rect>,
    out: &mut Vec<PopoverLayout>,
) {
    if let Widget::Popover { anchor, child, on_dismiss, modifiers: _ } = w {
        if let Some(&anchor_rect) = anchors.get(&anchor.0) {
            // Measure child's intrinsic size, then position it just
            // below the anchor. Flip above when there's no room.
            let csize = measure(child);
            let below_y = anchor_rect.y + anchor_rect.h as i32;
            let fits_below = below_y + csize.h as i32 <= window.y + window.h as i32;
            let y = if fits_below {
                below_y
            } else {
                (anchor_rect.y - csize.h as i32).max(window.y)
            };
            // Horizontally, anchor-left clamped to window-right.
            let max_x = window.x + window.w as i32 - csize.w as i32;
            let x = anchor_rect.x.min(max_x.max(window.x));
            let frect = Rect { x, y, w: csize.w, h: csize.h };
            let layout = place(child, frect);
            out.push(PopoverLayout {
                on_dismiss:  *on_dismiss,
                anchor_rect,
                child:       child.clone(),
                layout,
            });
        }
        // Popover's own children stop here — the child is placed via
        // floating layout above; recursion below skips the wrapped
        // tree to avoid double-placing.
        return;
    }
    for c in widget_children(w) {
        collect_popovers(c, window, anchors, out);
    }
}

/// Children of a widget — used by anchor + popover walkers.
fn widget_children(w: &Widget) -> &[Widget] {
    match w {
        Widget::Column { children, .. }
        | Widget::Row    { children, .. }
        | Widget::Stack  { children, .. }
        | Widget::Menu   { items: children, .. } => children,
        Widget::Scroll  { child, .. } => core::slice::from_ref(&**child),
        Widget::Popover { child, .. } => core::slice::from_ref(&**child),
        _ => &[],
    }
}

// ── Pass 1: intrinsic measurement ─────────────────────────────────────

/// Compute the node's preferred size with no container constraints.
/// `Spacer` reports (0, 0) here — flex distribution happens in `place`.
///
/// MinWidth/MaxWidth modifiers clamp the width *after* intrinsic
/// measurement so containers see the constrained value during flex
/// distribution.
fn measure(w: &Widget) -> Size {
    let intrinsic = measure_intrinsic(w);
    apply_size_constraints(intrinsic, mods_of_widget(w))
}

fn apply_size_constraints(size: Size, mods: &[Modifier]) -> Size {
    let mut min_w = 0u32;
    let mut max_w = u32::MAX;
    for m in mods {
        match m {
            Modifier::MinWidth(v) => { let v = *v as u32; if v > min_w { min_w = v; } }
            Modifier::MaxWidth(v) => { let v = *v as u32; if v < max_w { max_w = v; } }
            _ => {}
        }
    }
    Size {
        w: size.w.max(min_w).min(max_w),
        h: size.h,
    }
}

/// Return the highest `Modifier::Flex(n)` weight on `mods`, or `None`
/// if no Flex modifier is present. `Flex(0)` is treated as "no flex"
/// (matches the SDK doc and avoids zero-weight bookkeeping).
fn flex_modifier(mods: &[Modifier]) -> Option<u8> {
    let mut max: u8 = 0;
    let mut found = false;
    for m in mods {
        if let Modifier::Flex(n) = m {
            if *n > 0 { found = true; if *n > max { max = *n; } }
        }
    }
    if found { Some(max) } else { None }
}

fn mods_of_widget(w: &Widget) -> &[Modifier] {
    match w {
        Widget::Column  { modifiers, .. } |
        Widget::Row     { modifiers, .. } |
        Widget::Stack   { modifiers, .. } |
        Widget::Scroll  { modifiers, .. } |
        Widget::Text    { modifiers, .. } |
        Widget::Icon    { modifiers, .. } |
        Widget::Button  { modifiers, .. } |
        Widget::Input   { modifiers, .. } |
        Widget::Checkbox{ modifiers, .. } |
        Widget::Canvas  { modifiers, .. } |
        Widget::Popover { modifiers, .. } |
        Widget::Tooltip { modifiers, .. } |
        Widget::Menu    { modifiers, .. } => modifiers,
        _ => &[],
    }
}

fn measure_intrinsic(w: &Widget) -> Size {
    match w {
        Widget::Column { children, spacing, modifiers, .. } => {
            let outer_pad = padding(modifiers);
            let mut total_h: u32 = 0;
            let mut max_w: u32 = 0;
            let mut count: u32 = 0;
            for c in children {
                if matches!(c, Widget::Spacer { .. }) { continue; }
                let cs = measure(c);
                total_h = total_h.saturating_add(cs.h);
                if cs.w > max_w { max_w = cs.w; }
                count += 1;
            }
            let gaps = (count.saturating_sub(1)) * (*spacing as u32);
            Size {
                w: max_w + outer_pad.0 * 2,
                h: total_h + gaps + outer_pad.1 * 2,
            }
        }

        Widget::Row { children, spacing, modifiers, .. } => {
            let outer_pad = padding(modifiers);
            let mut total_w: u32 = 0;
            let mut max_h: u32 = 0;
            let mut count: u32 = 0;
            for c in children {
                if matches!(c, Widget::Spacer { .. }) { continue; }
                let cs = measure(c);
                total_w = total_w.saturating_add(cs.w);
                if cs.h > max_h { max_h = cs.h; }
                count += 1;
            }
            let gaps = (count.saturating_sub(1)) * (*spacing as u32);
            Size {
                w: total_w + gaps + outer_pad.0 * 2,
                h: max_h + outer_pad.1 * 2,
            }
        }

        Widget::Stack { children, modifiers } => {
            let outer_pad = padding(modifiers);
            let mut max_w: u32 = 0;
            let mut max_h: u32 = 0;
            for c in children {
                let cs = measure(c);
                if cs.w > max_w { max_w = cs.w; }
                if cs.h > max_h { max_h = cs.h; }
            }
            Size { w: max_w + outer_pad.0 * 2, h: max_h + outer_pad.1 * 2 }
        }

        Widget::Scroll { child, .. } => measure(child),

        Widget::Text { content, style, modifiers } => {
            let w = ceil_u32(crate::gui::text::measure(content, *style));
            let h = ceil_u32(crate::gui::text::line_height(*style));
            // Fallbacks for when font isn't loaded — line_height returns
            // size_px * 1.2, measure returns 0 → conservative 6 px/char.
            let w = if w == 0 {
                content.chars().count() as u32 * 6
            } else { w };
            // Honour a Padding modifier on the leaf so the OUTER rect
            // grows to include the padding band — siblings then space
            // out correctly. Render-side paints glyphs at the inner
            // rect (post-padding) via paint_node_eff. Without this,
            // `prefab::menu_bar` and `prefab::badge` (Text + Padding)
            // were rendered with siblings touching their glyph edges.
            let pad = padding(modifiers);
            Size { w: w + pad.0 * 2, h: h + pad.1 * 2 }
        }

        Widget::Icon { size, modifiers, .. } => {
            let s = *size as u32;
            let pad = padding(modifiers);
            Size { w: s + pad.0 * 2, h: s + pad.1 * 2 }
        }

        Widget::Button { label, icon, .. } => {
            // label + 8 px horizontal padding on each side; icon (if any)
            // precedes the label with 4 px gap. Matches typical toolkit
            // button chrome.
            use super::abi::IconId;
            let label_w = if label.is_empty() {
                0
            } else {
                ceil_u32(crate::gui::text::measure(label, TextStyle::Body))
            };
            let label_h = ceil_u32(crate::gui::text::line_height(TextStyle::Body));
            let icon_slot = if !matches!(icon, IconId::None) { 16 + 4 } else { 0 };
            Size {
                w: icon_slot + label_w + 16,  // 8 px pad × 2
                h: label_h.max(16) + 8,       // 4 px vertical pad × 2
            }
        }

        Widget::Input { value, placeholder, modifiers, .. } => {
            let sample = if value.is_empty() { placeholder } else { value };
            let w = ceil_u32(crate::gui::text::measure(sample, TextStyle::Body));
            let h = ceil_u32(crate::gui::text::line_height(TextStyle::Body));
            // Built-in 4 px chrome + the modifier's own padding. Min-
            // width keeps empty inputs from collapsing in flex rows.
            let pad = padding(modifiers);
            Size { w: w.max(120) + 8 + pad.0 * 2, h: h + 8 + pad.1 * 2 }
        }

        Widget::Checkbox { modifiers, .. } => {
            let pad = padding(modifiers);
            Size { w: 16 + pad.0 * 2, h: 16 + pad.1 * 2 }
        }

        Widget::Spacer { .. } => Size::default(),

        Widget::Divider => Size { w: 0, h: 1 },

        Widget::Canvas { width, height, .. } => {
            Size { w: *width as u32, h: *height as u32 }
        }

        // Reserved slots — compositor rejects at render time; layout
        // reserves zero space so mixed trees still measure consistently.
        Widget::Popover { .. } | Widget::Tooltip { .. } | Widget::Menu { .. } => {
            Size::default()
        }
    }
}

// ── Pass 2: placement ─────────────────────────────────────────────────

/// Place `w` inside `inner` (already-padded rect). Returns the absolute
/// layout tree rooted at `w`.
fn place(w: &Widget, inner: Rect) -> LayoutNode {
    match w {
        Widget::Column { children, spacing, align, modifiers } => {
            let (container, content) = unpack_modifiers_on(modifiers, inner);
            let mut node = place_axis(children, *spacing, *align, content, /* vertical = */ true);
            node.rect = container;
            node
        }

        Widget::Row { children, spacing, align, modifiers } => {
            let (container, content) = unpack_modifiers_on(modifiers, inner);
            let mut node = place_axis(children, *spacing, *align, content, false);
            node.rect = container;
            node
        }

        Widget::Stack { children, modifiers } => {
            let (container, content) = unpack_modifiers_on(modifiers, inner);
            let mut kids = Vec::with_capacity(children.len());
            for c in children {
                kids.push(place(c, content));
            }
            LayoutNode { rect: container, baseline: 0, children: kids }
        }

        Widget::Scroll { child, axis, .. } => {
            // Scroll: child takes container's cross size, natural size on
            // main axis. Clipping + scroll-offset come in the rasterizer.
            let csize = measure(child);
            let child_rect = match axis {
                Axis::Vertical   => Rect { x: inner.x, y: inner.y, w: inner.w, h: csize.h.max(inner.h) },
                Axis::Horizontal => Rect { x: inner.x, y: inner.y, w: csize.w.max(inner.w), h: inner.h },
                Axis::Both       => Rect { x: inner.x, y: inner.y, w: csize.w.max(inner.w), h: csize.h.max(inner.h) },
            };
            let inner_layout = place(child, child_rect);
            LayoutNode {
                rect: inner,
                baseline: 0,
                children: alloc::vec![inner_layout],
            }
        }

        Widget::Text { style, .. } => {
            let measured = measure(w);
            let baseline = ceil_u32(crate::gui::text::ascent(*style));
            LayoutNode {
                rect: Rect { x: inner.x, y: inner.y, w: measured.w, h: measured.h },
                baseline,
                children: Vec::new(),
            }
        }

        Widget::Icon { .. }
        | Widget::Button { .. }
        | Widget::Input { .. }
        | Widget::Checkbox { .. }
        | Widget::Canvas { .. } => {
            let m = measure(w);
            LayoutNode::leaf(Rect { x: inner.x, y: inner.y, w: m.w, h: m.h })
        }

        Widget::Spacer { .. } => {
            // Spacers are zero-sized unless a Row/Column expands them.
            LayoutNode::leaf(Rect { x: inner.x, y: inner.y, w: 0, h: 0 })
        }

        Widget::Divider => {
            LayoutNode::leaf(Rect { x: inner.x, y: inner.y, w: inner.w, h: 1 })
        }

        Widget::Popover { .. } | Widget::Tooltip { .. } | Widget::Menu { .. } => {
            // Reserved — rasterizer logs + rejects. Layout returns a
            // zero rect at the container origin so dumps stay legible.
            LayoutNode::leaf(Rect { x: inner.x, y: inner.y, w: 0, h: 0 })
        }
    }
}

/// Generic axis placement for Row/Column.
fn place_axis(
    children: &[Widget],
    spacing: u16,
    align: Align,
    content: Rect,
    vertical: bool,
) -> LayoutNode {
    let main_avail: u32 = if vertical { content.h } else { content.w };
    let cross_avail: u32 = if vertical { content.w } else { content.h };

    // Pass 1: measure children on the main axis, tally total intrinsic
    // and total flex weight. Both `Spacer { flex }` and any non-Spacer
    // child carrying `Modifier::Flex(n)` contribute weight; the
    // difference is that Spacer's intrinsic is zero (so it's pure
    // flex space) while Modifier::Flex children keep their intrinsic
    // as a basis and only get the extra distributed share.
    let mut measured: Vec<Size> = Vec::with_capacity(children.len());
    let mut total_main: u32 = 0;
    let mut flex_sum: u32 = 0;
    let mut count: u32 = 0;

    for c in children {
        let m = measure(c);
        measured.push(m);
        let intrinsic_main = if vertical { m.h } else { m.w };
        match c {
            Widget::Spacer { flex } => {
                flex_sum += (*flex).max(1) as u32;
            }
            _ => {
                total_main = total_main.saturating_add(intrinsic_main);
                if let Some(weight) = flex_modifier(mods_of_widget(c)) {
                    flex_sum += weight as u32;
                }
            }
        }
        count += 1;
    }

    let gaps = count.saturating_sub(1) * spacing as u32;
    let remaining = main_avail.saturating_sub(total_main).saturating_sub(gaps);

    // Pass 2: walk children, place them along the main axis. Spacers
    // and Modifier::Flex children absorb `remaining` proportionally
    // to their flex weight; everyone else sticks to intrinsic.
    let mut kids = Vec::with_capacity(children.len());
    let mut cursor: u32 = 0;

    for (idx, c) in children.iter().enumerate() {
        let m = &measured[idx];
        let intrinsic_main = if vertical { m.h } else { m.w };

        // Main-axis size for this child.
        let main_sz = match c {
            Widget::Spacer { flex } => {
                if flex_sum == 0 { 0 } else {
                    (remaining * (*flex).max(1) as u32) / flex_sum
                }
            }
            _ => match flex_modifier(mods_of_widget(c)) {
                Some(weight) if flex_sum > 0 => {
                    intrinsic_main + (remaining * weight as u32) / flex_sum
                }
                _ => intrinsic_main,
            },
        };

        // Cross-axis size + offset based on align.
        let cross_sz_intrinsic = if vertical { m.w } else { m.h };
        let (cross_sz, cross_off) = match align {
            Align::Start   => (cross_sz_intrinsic, 0),
            Align::Center  => (cross_sz_intrinsic, (cross_avail.saturating_sub(cross_sz_intrinsic)) / 2),
            Align::End     => (cross_sz_intrinsic, cross_avail.saturating_sub(cross_sz_intrinsic)),
            Align::Stretch => (cross_avail, 0),
        };

        // Compose child's allotted rect in absolute window coords.
        let child_rect = if vertical {
            Rect {
                x: content.x + cross_off as i32,
                y: content.y + cursor as i32,
                w: cross_sz,
                h: main_sz,
            }
        } else {
            Rect {
                x: content.x + cursor as i32,
                y: content.y + cross_off as i32,
                w: main_sz,
                h: cross_sz,
            }
        };

        kids.push(place(c, child_rect));
        cursor = cursor.saturating_add(main_sz);
        if idx + 1 < children.len() {
            cursor = cursor.saturating_add(spacing as u32);
        }
    }

    LayoutNode { rect: content, baseline: 0, children: kids }
}

// ── Modifier helpers ──────────────────────────────────────────────────

/// Read a widget's top-level modifiers once; return the outer rect it
/// claims + the inner rect its children occupy (outer minus padding).
fn unpack_modifiers(w: &Widget, container: Rect) -> (Rect, Rect) {
    let mods: &[Modifier] = match w {
        Widget::Column  { modifiers, .. } |
        Widget::Row     { modifiers, .. } |
        Widget::Stack   { modifiers, .. } |
        Widget::Scroll  { modifiers, .. } |
        Widget::Text    { modifiers, .. } |
        Widget::Icon    { modifiers, .. } |
        Widget::Button  { modifiers, .. } |
        Widget::Input   { modifiers, .. } |
        Widget::Checkbox{ modifiers, .. } |
        Widget::Canvas  { modifiers, .. } |
        Widget::Popover { modifiers, .. } |
        Widget::Tooltip { modifiers, .. } |
        Widget::Menu    { modifiers, .. } => modifiers,
        _ => &[],
    };
    unpack_modifiers_on(mods, container)
}

/// Apply Padding in `mods` to `container`, yielding (outer, inner).
/// Outer == container (Margin currently ignored); inner shrinks by 2×padding.
fn unpack_modifiers_on(mods: &[Modifier], container: Rect) -> (Rect, Rect) {
    let (pad_x, pad_y) = padding(mods);
    let inner = Rect {
        x: container.x + pad_x as i32,
        y: container.y + pad_y as i32,
        w: container.w.saturating_sub(pad_x * 2),
        h: container.h.saturating_sub(pad_y * 2),
    };
    (container, inner)
}

/// Sum of Padding modifiers → (x-pad, y-pad) in logical px.
fn padding(mods: &[Modifier]) -> (u32, u32) {
    let mut p: u32 = 0;
    for m in mods {
        if let Modifier::Padding(n) = m {
            p = p.saturating_add(*n as u32);
        }
    }
    (p, p)
}

/// `f32::ceil` isn't in core (no_std). Positive-only ceil-to-u32,
/// saturating on overflow/negatives. Used to round text widths + line
/// heights up so layout never under-reports size.
fn ceil_u32(x: f32) -> u32 {
    if !x.is_finite() || x <= 0.0 {
        return 0;
    }
    let i = x as u32;
    if (i as f32) < x { i.saturating_add(1) } else { i }
}

// ── Silence unused warnings on helper types while the rest of the
//    pipeline (Point, Box) lands ────────────────────────────────────
#[allow(dead_code)]
fn _keep_imports_alive() {
    let _ = Point::default();
    let _: Option<Box<LayoutNode>> = None;
}
