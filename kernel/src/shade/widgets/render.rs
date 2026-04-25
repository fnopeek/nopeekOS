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

use alloc::vec::Vec;

use super::abi::{
    Density, Fill, Modifier, Point, RasterTarget, Rasterizer, Rect, Token, Widget,
};
use super::layout::LayoutNode;

/// Render `widget` + `layout` (trees in lockstep) into `target` using
/// `rast`. Default-state entry — used by paths that don't track hover
/// (e.g. one-shot debug renders).
pub fn render(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
) {
    render_with_state(rast, target, widget, layout, None, Density::Regular);
}

/// Render with explicit pseudo-state context.
///
/// `hover_path` interpretation:
///   - `None`        → this subtree contains no hovered node
///   - `Some([])`    → THIS node is the hover target; merge `Hover` mods
///   - `Some([i,…])` → child `i` is on the hover path; recurse with tail
///
/// `density` is the compositor-classified container size bucket; widgets
/// matching `WhenDensity(d, …)` apply their inner mods on match.
pub fn render_with_state(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
    hover_path: Option<&[u32]>,
    density: Density,
) {
    // Hover-state matches CSS `:hover` semantics: the cursor is over
    // this node *or any of its descendants*. Both cases are encoded as
    // `Some(_)` (with `Some([])` = "this is the deepest", `Some([...])`
    // = "in a child"). `None` = cursor is in a different subtree.
    let is_hovered = hover_path.is_some();
    let base = modifiers_of(widget);
    let eff = effective_modifiers(base, is_hovered, density);

    paint_modifiers_eff(rast, target, &eff, layout.rect);
    paint_node_eff(rast, target, widget, layout, &eff);

    // Recurse — at most one child sits on the hover path.
    let kids = widget_children(widget);
    for (i, (cw, cl)) in kids.iter().zip(layout.children.iter()).enumerate() {
        let child_path: Option<&[u32]> = match hover_path {
            Some(p) if !p.is_empty() && p[0] == i as u32 => Some(&p[1..]),
            _ => None,
        };
        render_with_state(rast, target, cw, cl, child_path, density);
    }

    // Modifier::Opacity acts as a post-paint dampening over the node's
    // rect — blend everything already painted there towards the
    // Surface token, weighted by (255 - opacity). Lets the SDK
    // express "show this at 70 % visibility" without the rasterizer
    // trait needing a new parameter.
    let op = find_opacity_in(&eff);
    if op < 255 {
        apply_rect_opacity(target, layout.rect, op);
    }
}

/// Build the modifier list that applies to `widget` after merging the
/// active pseudo-states and density-conditional mods. Wrapper variants
/// are stripped so downstream paint code never sees nested modifier
/// lists.
///
/// Order matters: base modifiers first, then state mods (so a `.hover`
/// background overrides the base background — `paint_modifiers_eff`
/// reads the *last* matching modifier).
fn effective_modifiers(
    base: &[Modifier],
    is_hovered: bool,
    density: Density,
) -> Vec<Modifier> {
    let mut out: Vec<Modifier> = Vec::with_capacity(base.len());
    // First pass: keep all non-pseudo-state modifiers verbatim.
    for m in base {
        match m {
            Modifier::Hover(_)
            | Modifier::Focus(_)
            | Modifier::Active(_)
            | Modifier::Disabled(_)
            | Modifier::WhenDensity(_, _) => {}
            _ => out.push(m.clone()),
        }
    }
    // Second pass: append matching state mods. Hover wins over density
    // because hover is the more specific signal at any given moment.
    for m in base {
        match m {
            Modifier::WhenDensity(d, inner) if *d == density => {
                for inner_m in inner { out.push(inner_m.clone()); }
            }
            _ => {}
        }
    }
    if is_hovered {
        for m in base {
            if let Modifier::Hover(inner) = m {
                for inner_m in inner { out.push(inner_m.clone()); }
            }
        }
    }
    out
}

/// First Opacity in an explicit modifier list. Used by the post-paint
/// opacity dampening pass.
fn find_opacity_in(mods: &[Modifier]) -> u8 {
    for m in mods {
        if let Modifier::Opacity(v) = m { return *v; }
    }
    255
}

/// Recursively check whether `tree` contains any pseudo-state modifier
/// or density-conditional modifier. Compositor uses this to skip
/// re-renders on MouseMove when the result wouldn't change anyway.
pub fn tree_has_pseudo_state(tree: &Widget) -> bool {
    for m in modifiers_of(tree) {
        if matches!(
            m,
            Modifier::Hover(_)
                | Modifier::Focus(_)
                | Modifier::Active(_)
                | Modifier::Disabled(_)
                | Modifier::WhenDensity(_, _)
        ) {
            return true;
        }
    }
    for c in widget_children(tree) {
        if tree_has_pseudo_state(c) { return true; }
    }
    false
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

fn paint_modifiers_eff(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    mods: &[Modifier],
    rect: Rect,
) {
    // Last write wins so state mods (hover, etc.) appended after the
    // base list override base values cleanly.
    let mut bg: Option<Token> = None;
    let mut border: Option<(Token, u8, u8)> = None;
    let mut rounded: Option<u8> = None;
    for m in mods {
        match m {
            Modifier::Background(t) => bg = Some(*t),
            Modifier::Border { token, width, radius } => border = Some((*token, *width, *radius)),
            Modifier::Rounded(r) => rounded = Some(*r),
            _ => {}
        }
    }

    // Rounded modifier wins for the outer corner radius. Border's own
    // radius applies only as a fallback so existing apps (which set the
    // radius via Border) keep their look without code changes.
    let radius = rounded.unwrap_or_else(|| border.map(|(_, _, r)| r).unwrap_or(0));

    if let Some(tok) = bg {
        if radius > 0 {
            rast.rect_rounded(target, rect, Fill::Solid(tok), radius);
        } else {
            rast.rect(target, rect, Fill::Solid(tok));
        }
    }

    if let Some((tok, width, _)) = border {
        if width > 0 {
            rast.stroke_rounded(target, rect, Fill::Solid(tok), width, radius);
        }
    }
}

/// Paint the node's own visible content (leaves only; containers are
/// pure layout). Reads node-affecting modifiers (Tint, …) from the
/// effective list so pseudo-state changes (hover-tinted icons, etc.)
/// take effect.
fn paint_node_eff(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
    eff: &[Modifier],
) {
    let rect = layout.rect;

    match widget {
        Widget::Text { content, style, .. } => {
            rast.text(target, content, *style, Point { x: rect.x, y: rect.y });
        }

        Widget::Icon { id, size, .. } => {
            let mut color = Token::OnSurface;
            for m in eff {
                if let Modifier::Tint(tok) = m { color = *tok; }
            }
            rast.icon(target, *id, *size, color, Point { x: rect.x, y: rect.y });
        }

        Widget::Button { label, icon, .. } => {
            // If `paint_modifiers_eff` already painted a Background or
            // Border for this button, the chrome is done — skip the
            // hardcoded Accent fill so prefab::button(Destructive) /
            // (Ghost) styles render correctly. Fall back to Accent only
            // when the button has no explicit background.
            let has_bg = eff.iter().any(|m| matches!(m, Modifier::Background(_)));
            if !has_bg {
                rast.rect(target, rect, Fill::Solid(Token::Accent));
            }
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
            // Same pattern as Button: paint a SurfaceElevated background
            // only if no Modifier::Background is on the eff list. Lets a
            // wrapping prefab (e.g. prefab::input) own the chrome —
            // background, rounded, focus border — without a double-fill.
            let has_bg = eff.iter().any(|m| matches!(m, Modifier::Background(_)));
            if !has_bg {
                rast.rect(target, rect, Fill::Solid(Token::SurfaceElevated));
            }
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
