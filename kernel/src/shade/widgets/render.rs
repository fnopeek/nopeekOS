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
