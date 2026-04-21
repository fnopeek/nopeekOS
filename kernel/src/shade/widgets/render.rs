//! Widget render walker — drives the rasterizer over a widget+layout
//! tree pair.
//!
//! Walks the Widget tree and the LayoutNode tree in lockstep (same
//! structural shape). For each node:
//!   1. Apply container decorations (Background, Border modifiers) as
//!      a filled rect at the node's laid-out rect.
//!   2. Dispatch the node's own paint op (Text/Icon/Button/... → rast
//!      trait methods, or containers → just recurse).
//!   3. Recurse into children.
//!
//! Clipping, coordinate transforms, and glyph compositing are all the
//! rasterizer's problem. This file only *schedules* calls.

#![allow(dead_code)]

use super::abi::{
    Fill, Modifier, Point, RasterTarget, Rasterizer, Rect, Token, Widget,
};
use super::layout::LayoutNode;

/// Render `widget` + `layout` (trees in lockstep) into `target` using
/// `rast`. Call at window-level; recurses into children.
pub fn render(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
) {
    paint_modifiers(rast, target, widget, layout.rect);
    paint_node(rast, target, widget, layout);

    // Recurse.
    let kids = widget_children(widget);
    for (cw, cl) in kids.iter().zip(layout.children.iter()) {
        render(rast, target, cw, cl);
    }

    // Modifier::Opacity acts as a post-paint dampening over the node's
    // rect — blend everything already painted there towards the
    // Surface token, weighted by (255 - opacity). Lets the SDK
    // express "show this at 70 % visibility" without the rasterizer
    // trait needing a new parameter.
    let op = find_opacity(widget);
    if op < 255 {
        apply_rect_opacity(target, layout.rect, op);
    }
}

/// First Opacity modifier on the widget, or 255 if none.
fn find_opacity(w: &Widget) -> u8 {
    for m in modifiers_of(w) {
        if let Modifier::Opacity(v) = m { return *v; }
    }
    255
}

/// Blend every pixel in `rect` towards the Surface token by
/// `255 - opacity`. Rectangle is in window coordinates.
fn apply_rect_opacity(target: &mut RasterTarget, rect: Rect, opacity: u8) {
    if opacity == 255 { return; }
    let bg = target.palette.colors[super::abi::Token::Surface as usize];
    let x0 = (rect.x - target.origin.x).max(0);
    let y0 = (rect.y - target.origin.y).max(0);
    let x1 = (x0 + rect.w as i32).min(target.size.w as i32);
    let y1 = (y0 + rect.h as i32).min(target.size.h as i32);
    if x0 >= x1 || y0 >= y1 { return; }

    let weight = 255u32 - opacity as u32;
    let stride = target.stride as usize;
    for py in y0..y1 {
        let base = py as usize * stride;
        for px in x0..x1 {
            let cur = target.pixels[base + px as usize];
            target.pixels[base + px as usize] = blend_towards(cur, bg, weight);
        }
    }
}

fn blend_towards(src: u32, dst: u32, weight: u32) -> u32 {
    if weight == 0 { return src; }
    let inv = 255u32.saturating_sub(weight);
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8)  & 0xFF;
    let sb =  src        & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8)  & 0xFF;
    let db =  dst        & 0xFF;
    let r = (sr * inv + dr * weight) / 255;
    let g = (sg * inv + dg * weight) / 255;
    let b = (sb * inv + db * weight) / 255;
    0xFF_00_00_00 | (r << 16) | (g << 8) | b
}

/// Paint any Background / Border modifiers attached to the node.
fn paint_modifiers(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    rect: Rect,
) {
    let mods = modifiers_of(widget);
    for m in mods {
        match m {
            Modifier::Background(tok) => {
                rast.rect(target, rect, Fill::Solid(*tok));
            }
            Modifier::Border { token, width, radius: _ } => {
                // Stroke as 4 thin rects (top/bottom/left/right).
                // Radius is future work (needs per-pixel AA).
                let w = *width as u32;
                if w == 0 { continue; }
                let tok = *token;
                rast.rect(target, Rect {
                    x: rect.x, y: rect.y, w: rect.w, h: w,
                }, Fill::Solid(tok));
                rast.rect(target, Rect {
                    x: rect.x, y: rect.y + rect.h as i32 - w as i32,
                    w: rect.w, h: w,
                }, Fill::Solid(tok));
                rast.rect(target, Rect {
                    x: rect.x, y: rect.y, w: w, h: rect.h,
                }, Fill::Solid(tok));
                rast.rect(target, Rect {
                    x: rect.x + rect.w as i32 - w as i32, y: rect.y,
                    w: w, h: rect.h,
                }, Fill::Solid(tok));
            }
            _ => {}
        }
    }
}

/// Paint the node's own visible content (leaves only; containers are
/// pure layout).
fn paint_node(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
) {
    let rect = layout.rect;

    match widget {
        Widget::Text { content, style, .. } => {
            rast.text(target, content, *style, Point { x: rect.x, y: rect.y });
        }

        Widget::Icon { id, size, .. } => {
            rast.icon(target, *id, *size, Token::OnSurface,
                      Point { x: rect.x, y: rect.y });
        }

        Widget::Button { label, icon, .. } => {
            // Soft accent background, white label. Icon first (if
            // present), then label at a fixed x offset.
            rast.rect(target, rect, Fill::Solid(Token::Accent));
            let pad_x = 8i32;
            let pad_y = 4i32;
            let mut x = rect.x + pad_x;
            use super::abi::IconId;
            if !matches!(icon, IconId::None) {
                rast.icon(target, *icon, 16, Token::OnAccent,
                          Point { x, y: rect.y + pad_y });
                x += 20;
            }
            if !label.is_empty() {
                rast.text(target, label, super::abi::TextStyle::Body,
                          Point { x, y: rect.y + pad_y });
            }
        }

        Widget::Input { value, placeholder, .. } => {
            rast.rect(target, rect, Fill::Solid(Token::SurfaceElevated));
            let shown = if value.is_empty() { placeholder.as_str() } else { value.as_str() };
            let style = if value.is_empty() {
                super::abi::TextStyle::Muted
            } else {
                super::abi::TextStyle::Body
            };
            rast.text(target, shown, style,
                      Point { x: rect.x + 4, y: rect.y + 4 });
        }

        Widget::Checkbox { value, .. } => {
            // Outer stroke + inner fill if checked.
            rast.rect(target, rect, Fill::Solid(Token::Border));
            let inset = 2u32;
            let inner = Rect {
                x: rect.x + inset as i32,
                y: rect.y + inset as i32,
                w: rect.w.saturating_sub(inset * 2),
                h: rect.h.saturating_sub(inset * 2),
            };
            let fill = if *value { Token::Accent } else { Token::Surface };
            rast.rect(target, inner, Fill::Solid(fill));
        }

        Widget::Divider => {
            rast.rect(target, rect, Fill::Solid(Token::Border));
        }

        Widget::Canvas { width, height, .. } => {
            // P10.10 hands the app-supplied pixels in via
            // npk_canvas_commit; until then draw a magenta placeholder
            // so the slot is visible during debug.
            let _ = (width, height);
            rast.rect(target, rect, Fill::Solid(Token::Danger));
        }

        // Containers paint nothing themselves — their Background /
        // Border modifiers are already handled above. Children recurse.
        Widget::Column { .. } | Widget::Row { .. } | Widget::Stack { .. }
        | Widget::Scroll { .. } => {}

        // Reserved slots — logged in scene_commit, skipped here.
        Widget::Popover { .. } | Widget::Tooltip { .. } | Widget::Menu { .. } => {}

        // Spacer + unknowns = no paint.
        _ => {}
    }
}

// ── Helpers (mirror debug.rs) ────────────────────────────────────────

fn modifiers_of(w: &Widget) -> &[Modifier] {
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

fn widget_children(w: &Widget) -> alloc::vec::Vec<&Widget> {
    let mut out = alloc::vec::Vec::new();
    match w {
        Widget::Column { children, .. } |
        Widget::Row    { children, .. } |
        Widget::Stack  { children, .. } |
        Widget::Menu   { items: children, .. } => {
            for c in children { out.push(c); }
        }
        Widget::Scroll { child, .. } | Widget::Popover { child, .. } => {
            out.push(child.as_ref());
        }
        _ => {}
    }
    out
}
