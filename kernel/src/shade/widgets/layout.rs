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
use alloc::vec::Vec;

use super::abi::{
    Align, Axis, Modifier, Point, Rect, Size, TextStyle, Widget,
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

/// Lay out `root` inside `container` (absolute px). Returns a layout
/// tree with absolute rects and baselines.
pub fn layout(root: &Widget, container: Rect) -> LayoutNode {
    // Apply root modifiers (Padding) to shrink the effective rect.
    let (_, inner) = unpack_modifiers(root, container);
    place(root, inner)
}

// ── Pass 1: intrinsic measurement ─────────────────────────────────────

/// Compute the node's preferred size with no container constraints.
/// `Spacer` reports (0, 0) here — flex distribution happens in `place`.
fn measure(w: &Widget) -> Size {
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

        Widget::Text { content, style, .. } => {
            let w = ceil_u32(crate::gui::text::measure(content, *style));
            let h = ceil_u32(crate::gui::text::line_height(*style));
            // Fallbacks for when font isn't loaded — line_height returns
            // size_px * 1.2, measure returns 0 → conservative 6 px/char.
            let w = if w == 0 {
                content.chars().count() as u32 * 6
            } else { w };
            Size { w, h }
        }

        Widget::Icon { size, .. } => {
            Size { w: *size as u32, h: *size as u32 }
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

        Widget::Input { value, placeholder, .. } => {
            let sample = if value.is_empty() { placeholder } else { value };
            let w = ceil_u32(crate::gui::text::measure(sample, TextStyle::Body));
            let h = ceil_u32(crate::gui::text::line_height(TextStyle::Body));
            // Minimum-width input so empty ones don't collapse.
            Size { w: w.max(120) + 8, h: h + 8 }
        }

        Widget::Checkbox { .. } => {
            Size { w: 16, h: 16 }
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

        _ => Size::default(),
    }
}

// ── Pass 2: placement ─────────────────────────────────────────────────

/// Place `w` inside `inner` (already-padded rect). Returns the absolute
/// layout tree rooted at `w`.
fn place(w: &Widget, inner: Rect) -> LayoutNode {
    match w {
        Widget::Column { children, spacing, align, modifiers } => {
            let (_, content) = unpack_modifiers_on(modifiers, inner);
            place_axis(children, *spacing, *align, content, /* vertical = */ true)
        }

        Widget::Row { children, spacing, align, modifiers } => {
            let (_, content) = unpack_modifiers_on(modifiers, inner);
            place_axis(children, *spacing, *align, content, false)
        }

        Widget::Stack { children, modifiers } => {
            let (_, content) = unpack_modifiers_on(modifiers, inner);
            let mut kids = Vec::with_capacity(children.len());
            for c in children {
                kids.push(place(c, content));
            }
            LayoutNode { rect: content, baseline: 0, children: kids }
        }

        Widget::Scroll { child, axis, .. } => {
            // Scroll: child takes container's cross size, natural size on
            // main axis. Clipping + scroll-offset come in the rasterizer.
            let csize = measure(child);
            let child_rect = match axis {
                Axis::Vertical   => Rect { x: inner.x, y: inner.y, w: inner.w, h: csize.h.max(inner.h) },
                Axis::Horizontal => Rect { x: inner.x, y: inner.y, w: csize.w.max(inner.w), h: inner.h },
                Axis::Both       => Rect { x: inner.x, y: inner.y, w: csize.w.max(inner.w), h: csize.h.max(inner.h) },
                _                => inner,
            };
            let inner_layout = place(child, child_rect);
            LayoutNode {
                rect: inner,
                baseline: 0,
                children: alloc::vec![inner_layout],
            }
        }

        Widget::Text { content, style, .. } => {
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

        _ => LayoutNode::empty(),
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

    // Pass 1: measure non-Spacer children on the main axis, tally flex.
    let mut measured: Vec<Size> = Vec::with_capacity(children.len());
    let mut total_main: u32 = 0;
    let mut flex_sum: u32 = 0;
    let mut count: u32 = 0;

    for c in children {
        let m = measure(c);
        measured.push(m);
        match c {
            Widget::Spacer { flex } => {
                flex_sum += (*flex).max(1) as u32;
            }
            _ => {
                total_main = total_main.saturating_add(if vertical { m.h } else { m.w });
            }
        }
        count += 1;
    }

    let gaps = count.saturating_sub(1) * spacing as u32;
    let remaining = main_avail.saturating_sub(total_main).saturating_sub(gaps);

    // Pass 2: walk children, place them along the main axis. Spacers
    // absorb `remaining` proportionally to their flex weight.
    let mut kids = Vec::with_capacity(children.len());
    let mut cursor: u32 = 0;

    for (idx, c) in children.iter().enumerate() {
        let m = &measured[idx];

        // Main-axis size for this child.
        let main_sz = match c {
            Widget::Spacer { flex } => {
                if flex_sum == 0 { 0 } else {
                    (remaining * (*flex).max(1) as u32) / flex_sum
                }
            }
            _ => if vertical { m.h } else { m.w },
        };

        // Cross-axis size + offset based on align.
        let cross_sz_intrinsic = if vertical { m.w } else { m.h };
        let (cross_sz, cross_off) = match align {
            Align::Start   => (cross_sz_intrinsic, 0),
            Align::Center  => (cross_sz_intrinsic, (cross_avail.saturating_sub(cross_sz_intrinsic)) / 2),
            Align::End     => (cross_sz_intrinsic, cross_avail.saturating_sub(cross_sz_intrinsic)),
            Align::Stretch => (cross_avail, 0),
            _              => (cross_sz_intrinsic, 0),
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
