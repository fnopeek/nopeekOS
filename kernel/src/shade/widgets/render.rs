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
    Axis, Density, Fill, Modifier, Point, RasterTarget, Rasterizer, Rect, Token, Widget,
};
use super::layout::LayoutNode;
use super::InputEditState;

/// Render `widget` + `layout` (trees in lockstep) into `target` using
/// `rast`. Default-state entry — used by paths that don't track any
/// pseudo state (e.g. one-shot debug renders).
pub fn render(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
) {
    render_with_state(rast, target, widget, layout, None, None, None, Density::Regular, None, 0, None);
}

/// Render with explicit pseudo-state context.
///
/// Each `*_path: Option<&[u32]>` follows the same protocol:
///   - `None`        → this subtree contains no node in this state
///   - `Some([])`    → THIS node IS the state target — merge inner mods
///   - `Some([i,…])` → child `i` is on the path; descend with tail
///
/// CSS `:hover` / `:focus` / `:active` ancestor semantics: any
/// `Some(_)` value (empty path or deeper) marks the current node as
/// matching, so a Hover-modifier on a Row triggers when the cursor is
/// over a descendant Icon.
///
/// `density` drives `WhenDensity(d, …)` matching.
pub fn render_with_state(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
    hover_path: Option<&[u32]>,
    focus_path: Option<&[u32]>,
    active_path: Option<&[u32]>,
    density: Density,
    input_edit: Option<&InputEditState>,
    // Window vertical scroll offset (px). Applied to a focused TextArea's
    // line window (wheel scroll); Widget::Scroll handles its own offset in
    // layout, so this only matters at the TextArea leaf.
    scroll_y: u32,
    // Colour inherited from the nearest ancestor carrying `Modifier::Tint`,
    // like CSS `color`. A `Tint` on a Row is what apps reach for to say
    // "this whole row is accent now"; without inheritance the Row's own
    // paint would take it and every Text/Icon inside would silently fall
    // back to the default. `None` = no ancestor set one.
    inherited_tint: Option<Token>,
) {
    let is_hovered = hover_path.is_some();
    let is_focused = focus_path.is_some();
    let is_active  = active_path.is_some();
    let base = modifiers_of(widget);
    let eff = effective_modifiers(base, is_hovered, is_focused, is_active, density);
    // This node's own Tint wins for its whole subtree.
    let subtree_tint = eff.iter().rev()
        .find_map(|m| if let Modifier::Tint(t) = m { Some(*t) } else { None })
        .or(inherited_tint);

    paint_modifiers_eff(rast, target, &eff, layout.rect);
    // `is_focused && Some([])` (focus exactly here) is the only case
    // where the editor caret is painted — descended-focus paths are
    // ancestors, not the input itself. paint_node_eff applies that
    // check.
    let edit_for_node: Option<&InputEditState> =
        if matches!(focus_path, Some(p) if p.is_empty()) { input_edit } else { None };
    paint_node_eff(rast, target, widget, layout, &eff, edit_for_node, scroll_y, inherited_tint);

    // A Scroll clips its subtree to its viewport rect so overflowing
    // content is masked (and, for a vertical scroll, an overlay scrollbar
    // is drawn after). Save/restore the previous clip around the children.
    let saved_clip = target.clip;
    let is_scroll = matches!(widget, Widget::Scroll { .. });
    if is_scroll {
        let r = layout.rect;
        let lx0 = r.x - target.origin.x;
        let ly0 = r.y - target.origin.y;
        let (nx0, ny0, nx1, ny1) = (lx0, ly0, lx0 + r.w as i32, ly0 + r.h as i32);
        target.clip = Some(match saved_clip {
            Some((a, b, c, d)) => (a.max(nx0), b.max(ny0), c.min(nx1), d.min(ny1)),
            None => (nx0, ny0, nx1, ny1),
        });
    }

    // Recurse — at most one child sits on each path.
    let kids = widget_children(widget);
    for (i, (cw, cl)) in kids.iter().zip(layout.children.iter()).enumerate() {
        let child_hover  = descend(hover_path,  i as u32);
        let child_focus  = descend(focus_path,  i as u32);
        let child_active = descend(active_path, i as u32);
        render_with_state(
            rast, target, cw, cl,
            child_hover, child_focus, child_active, density, input_edit, scroll_y,
            subtree_tint,
        );
    }

    if is_scroll {
        target.clip = saved_clip;
        paint_scrollbar(rast, target, widget, layout);
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

/// Helper: peel one index off a state path to descend into child `i`.
fn descend(path: Option<&[u32]>, i: u32) -> Option<&[u32]> {
    match path {
        Some(p) if !p.is_empty() && p[0] == i => Some(&p[1..]),
        _ => None,
    }
}

/// Thin overlay scrollbar for a vertical `Widget::Scroll`. Drawn only
/// when the content overflows the viewport — otherwise nothing shows
/// (macOS/GTK overlay-scrollbar idiom). Painted over the content, it
/// reserves no layout space, so toggling it never reflows anything.
fn paint_scrollbar(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
) {
    if !matches!(widget, Widget::Scroll { axis: Axis::Vertical, .. }) { return; }
    let viewport = layout.rect;
    let child = match layout.children.first() { Some(c) => c, None => return };
    let content_h = child.rect.h;
    if content_h <= viewport.h || viewport.h == 0 { return; }

    let off = (viewport.y - child.rect.y).max(0) as u64;     // scrolled px
    let max_off = (content_h - viewport.h) as u64;
    let track_h = viewport.h as u64;
    // Thumb height proportional to the visible fraction, with a floor.
    let thumb_h = ((track_h * track_h) / content_h as u64).max(24).min(track_h) as u32;
    let travel = track_h - thumb_h as u64;
    let thumb_y = viewport.y + if max_off == 0 { 0 } else { (off * travel / max_off) as i32 };

    let thumb_w: i32 = 4;
    let margin: i32 = 2;
    let thumb_x = viewport.x + viewport.w as i32 - thumb_w - margin;
    rast.rect_rounded(
        target,
        Rect { x: thumb_x, y: thumb_y, w: thumb_w as u32, h: thumb_h },
        Fill::Solid(Token::OnSurfaceMuted),
        2,
    );
}

/// Local mirror of `layout::ceil_u32` — kept private so the caret-paint
/// path doesn't import a layout-module helper.
fn ceil_u32_local(x: f32) -> u32 {
    if !x.is_finite() || x <= 0.0 { return 0; }
    let i = x as u32;
    if (i as f32) < x { i.saturating_add(1) } else { i }
}

/// Colour token for byte offset `off` from a TextArea's spans, or
/// `default` if no span covers it. Spans are sorted by `start`, so we
/// can stop once a span begins past the offset.
fn span_token_at(spans: &[super::abi::Span], off: usize, default: Token) -> Token {
    for s in spans {
        let start = s.start as usize;
        if start > off { break; }
        if off < start + s.len as usize { return s.token; }
    }
    default
}

/// Sum every `Modifier::Padding` in the effective list. Mirrors
/// `layout::padding` so leaf glyph placement matches the layout-side
/// outer-size growth — a single canonical source for "how much
/// padding does this leaf carry".
fn leaf_padding(mods: &[Modifier]) -> (u32, u32) {
    let mut p: u32 = 0;
    for m in mods {
        if let Modifier::Padding(n) = m {
            p = p.saturating_add(*n as u32);
        }
    }
    (p, p)
}

/// Build the modifier list that applies to `widget` after merging the
/// active pseudo-states and density-conditional mods. Wrapper variants
/// are stripped so downstream paint code never sees nested modifier
/// lists.
///
/// Application order (later wins for last-write-wins fields like
/// Background): base → density → hover → focus → active → disabled.
/// Disabled is presence-based (the *modifier itself* on the widget,
/// not a compositor-tracked external state) and overrides interactive
/// states because it represents an explicit app decision.
fn effective_modifiers(
    base: &[Modifier],
    is_hovered: bool,
    is_focused: bool,
    is_active: bool,
    density: Density,
) -> Vec<Modifier> {
    let is_disabled = base.iter().any(|m| matches!(m, Modifier::Disabled(_)));

    let mut out: Vec<Modifier> = Vec::with_capacity(base.len());
    // Base: keep all non-pseudo-state modifiers verbatim.
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
    // Density (always applies — orthogonal to interactive states).
    for m in base {
        if let Modifier::WhenDensity(d, inner) = m {
            if *d == density {
                for inner_m in inner { out.push(inner_m.clone()); }
            }
        }
    }
    // Disabled wins over interactive states — when an app marks a
    // widget disabled, the user-state visuals shouldn't show through.
    if is_disabled {
        for m in base {
            if let Modifier::Disabled(inner) = m {
                for inner_m in inner { out.push(inner_m.clone()); }
            }
        }
    } else {
        if is_hovered {
            for m in base {
                if let Modifier::Hover(inner) = m {
                    for inner_m in inner { out.push(inner_m.clone()); }
                }
            }
        }
        if is_focused {
            for m in base {
                if let Modifier::Focus(inner) = m {
                    for inner_m in inner { out.push(inner_m.clone()); }
                }
            }
        }
        if is_active {
            for m in base {
                if let Modifier::Active(inner) = m {
                    for inner_m in inner { out.push(inner_m.clone()); }
                }
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

/// Gap between the widest line number and the gutter's hairline.
const GUTTER_PAD_R: i32 = 10;
/// Gap between the hairline and the left edge of the number column.
const GUTTER_PAD_L: i32 = 10;

/// Width a `TextArea`'s line-number gutter occupies, or 0 when the app
/// didn't ask for one. Sized to the highest line number the buffer can
/// show, so the text doesn't shift sideways as you scroll past 99.
/// **Shared by the renderer and the click-to-caret hit test** — if these
/// two disagree, clicks land on the wrong column.
pub(super) fn textarea_gutter_w(mods: &[Modifier], total_lines: usize) -> u32 {
    let on = mods.iter().any(|m| matches!(m, Modifier::LineNumbers(true)));
    if !on { return 0; }
    let digits = {
        let mut n = total_lines.max(1);
        let mut d = 0;
        while n > 0 { d += 1; n /= 10; }
        d.max(2)
    };
    let mut widest = alloc::string::String::new();
    for _ in 0..digits { widest.push('0'); }
    let num_w = ceil_u32_local(
        crate::gui::text::measure(&widest, super::abi::TextStyle::Mono));
    num_w + GUTTER_PAD_L as u32 + GUTTER_PAD_R as u32
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
    let mut ring: Option<(Token, u8)> = None;
    for m in mods {
        match m {
            Modifier::Background(t) => bg = Some(*t),
            Modifier::Border { token, width, radius } => border = Some((*token, *width, *radius)),
            Modifier::Rounded(r) => rounded = Some(*r),
            Modifier::Ring { token, width } => ring = Some((*token, *width)),
            _ => {}
        }
    }

    // Rounded modifier wins for the outer corner radius. Border's own
    // radius applies only as a fallback so existing apps (which set the
    // radius via Border) keep their look without code changes.
    let radius = rounded.unwrap_or_else(|| border.map(|(_, _, r)| r).unwrap_or(0));

    // Focus ring first: it sits OUTSIDE the node rect, so the background
    // and border paint over its inner edge and leave a clean band.
    if let Some((tok, width)) = ring {
        if width > 0 {
            let w = width as i32;
            let outer = Rect {
                x: rect.x - w,
                y: rect.y - w,
                w: rect.w.saturating_add(width as u32 * 2),
                h: rect.h.saturating_add(width as u32 * 2),
            };
            let outer_radius = radius.saturating_add(width);
            rast.stroke_rounded(target, outer, Fill::Solid(tok), width, outer_radius);
        }
    }

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
///
/// `edit_state` is `Some` iff this node is the focused `Widget::Input`
/// AND the compositor has a live editor for it — in which case the
/// rendered text comes from the editor buffer (not the widget's
/// `value`, which lags by one round-trip) and a caret is painted at
/// the editor cursor's x-position.
fn paint_node_eff(
    rast: &mut dyn Rasterizer,
    target: &mut RasterTarget,
    widget: &Widget,
    layout: &LayoutNode,
    eff: &[Modifier],
    edit_state: Option<&InputEditState>,
    scroll_y: u32,
    inherited_tint: Option<Token>,
) {
    let rect = layout.rect;
    // Inner-rect origin for leaf glyph placement. The OUTER rect is
    // sized to include any `Modifier::Padding` (see layout.rs leaf
    // measure paths); the actual text/icon must shift in by the
    // padding amount so the glyphs sit centred inside the padded
    // band — without this `prefab::menu_bar` and `prefab::badge`
    // would render with their text glued to the left edge of their
    // own padded background instead of inside it.
    let leaf_pad = leaf_padding(eff);
    let inner_x = rect.x + leaf_pad.0 as i32;
    let inner_y = rect.y + leaf_pad.1 as i32;

    match widget {
        Widget::Text { content, style, .. } => {
            // Style default, overridable by Modifier::Tint (e.g. an active
            // workspace pill tinted OnAccent so it reads on the Accent fill).
            let mut color = inherited_tint.unwrap_or(
                if matches!(style, super::abi::TextStyle::Muted) {
                    Token::OnSurfaceMuted
                } else {
                    Token::OnSurface
                });
            for m in eff {
                if let Modifier::Tint(tok) = m { color = *tok; }
            }
            rast.text(target, content, *style, color, Point { x: inner_x, y: inner_y });
        }

        Widget::Icon { id, size, .. } => {
            let mut color = inherited_tint.unwrap_or(Token::OnSurface);
            // Q8.8 fixed-point scale: 256 = 1.0×. Resolved from the
            // effective modifier list so Hover/Focus/Active states can
            // inflate the icon (dock cells use this for a Mac-style
            // hover bump). The scaled glyph stays centred inside the
            // original cell rect — layout doesn't change, so neighbours
            // don't shift; a small overflow can paint over the cell
            // background (the dock's Tray catches it cleanly).
            let mut q88: u32 = 256;
            for m in eff {
                match m {
                    Modifier::Tint(tok)  => color = *tok,
                    Modifier::Scale(v)   => q88 = *v as u32,
                    _ => {}
                }
            }
            let scaled = (((*size as u32) * q88) / 256).max(1).min(u16::MAX as u32) as u16;
            let off_x = (*size as i32 - scaled as i32) / 2;
            let off_y = off_x;
            rast.icon(target, *id, scaled, color,
                Point { x: inner_x + off_x, y: inner_y + off_y });
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
                rast.text(target, label, super::abi::TextStyle::Body, Token::OnSurface,
                          Point { x, y: rect.y + pad_y });
            }
        }

        Widget::Input { value, placeholder, .. } => {
            // No hardcoded fallback bg — the prefab or the app puts a
            // `Modifier::Background` on a wrapping container if it wants
            // chrome. Otherwise the input blends with the dialog
            // (matches modern launcher / spotlight visuals).
            //
            // Both placeholder and typed value render at Heading metrics
            // so the search bar reads at the same visual weight whether
            // empty or filled. The font size doesn't jump on first
            // keystroke.
            //
            // When focused and the compositor's editor owns this Input,
            // render `edit_state.value` instead of the tree's `value`
            // — the editor buffer leads the tree by one round-trip
            // until the app echoes the InputChange event back.
            let live_value: &str = match edit_state {
                Some(e) => e.value.as_str(),
                None    => value.as_str(),
            };
            let shown = if live_value.is_empty() { placeholder.as_str() } else { live_value };
            // Built-in 4 px chrome + the modifier's own padding.
            let text_x = inner_x + 4;
            let text_y = inner_y + 4;
            rast.text(target, shown, super::abi::TextStyle::Heading, Token::OnSurface,
                      Point { x: text_x, y: text_y });

            // Paint the caret. Only when an editor exists (focused) —
            // unfocused inputs render flat text.
            if let Some(e) = edit_state {
                let style = super::abi::TextStyle::Heading;
                let prefix = match e.value.get(..e.cursor) {
                    Some(s) => s,
                    // Cursor mis-aligned (defensive: shouldn't happen)
                    // → drop to end of value.
                    None    => e.value.as_str(),
                };
                let advance = ceil_u32_local(crate::gui::text::measure(prefix, style));
                let line_h  = ceil_u32_local(crate::gui::text::line_height(style));
                let caret_w = 2u32;
                let caret_rect = Rect {
                    x: text_x + advance as i32,
                    y: text_y,
                    w: caret_w,
                    h: line_h,
                };
                rast.rect(target, caret_rect, Fill::Solid(Token::OnSurface));
            }
        }

        Widget::TextArea { value, placeholder, spans, .. } => {
            // The editing surface. Background / border come from the
            // app's modifiers (handled in paint_modifiers_eff). When
            // focused, the live buffer leads the tree's `value` by one
            // round-trip — render from `edit_state` so typing is instant.
            // `spans` (syntax-highlight colours, byte offsets over the
            // committed value) are applied to the live buffer; freshly
            // typed bytes beyond span coverage fall back to the default
            // colour for one frame until the app re-commits.
            let live: &str = match edit_state {
                Some(e) => e.value.as_str(),
                None    => value.as_str(),
            };
            let style = super::abi::TextStyle::Mono;
            let line_h = ceil_u32_local(crate::gui::text::line_height(style)).max(1);
            // Resolve default text colour: OnSurface, Tint overrides.
            let mut color = Token::OnSurface;
            for m in eff { if let Modifier::Tint(tok) = m { color = *tok; } }

            let total_lines_all = live.split('\n').count();
            let gutter_w = textarea_gutter_w(eff, total_lines_all) as i32;
            let text_x = inner_x + 4 + gutter_w;
            let top_y  = inner_y + 4;
            let visible = (rect.h / line_h).max(1) as usize;

            // Empty buffer → muted placeholder on the first line.
            if live.is_empty() {
                rast.text(target, placeholder, style, Token::OnSurfaceMuted,
                          Point { x: text_x, y: top_y });
            }

            // Caret line/column (byte prefix within the caret's line).
            let (caret_line, caret_prefix) = match edit_state {
                Some(e) => {
                    let cur = e.cursor.min(live.len());
                    let cline = live[..cur].matches('\n').count();
                    let lstart = live[..cur].rfind('\n').map(|i| i + 1).unwrap_or(0);
                    (Some(cline), &live[lstart..cur])
                }
                None => (None, ""),
            };

            // Line window from the stored scroll offset (wheel / drag /
            // caret-follow). The view is authoritative here: caret-follow
            // happens on caret MOVES (handle_input_key adjusts scroll_y), so
            // the render must NOT re-pull to the caret every frame — that
            // would defeat manual wheel/drag scrolling.
            let total_lines = total_lines_all;
            let max_scroll = total_lines.saturating_sub(visible);
            let scroll = ((scroll_y / line_h) as usize).min(max_scroll);

            // Line-number gutter: a right-aligned column of numbers and a
            // hairline separating it from the text. Drawn here rather than
            // by the app because only the compositor knows the scroll
            // position of its own viewport.
            if gutter_w > 0 {
                let rule_x = inner_x + gutter_w - 1;
                rast.rect(target,
                          Rect { x: rule_x, y: rect.y, w: 1, h: rect.h },
                          Fill::Solid(Token::Border));
                for row in 0..visible {
                    let li = scroll + row;
                    if li >= total_lines { break; }
                    let mut num = alloc::string::String::new();
                    let _ = core::fmt::Write::write_fmt(
                        &mut num, format_args!("{}", li + 1));
                    let w = ceil_u32_local(crate::gui::text::measure(&num, style)) as i32;
                    let y = top_y + (row as u32 * line_h) as i32;
                    rast.text(target, &num, style, Token::OnSurfaceFaint,
                              Point { x: rule_x - GUTTER_PAD_R - w, y });
                }
            }

            // Selected byte range (anchor↔caret), painted as a highlight
            // block under the text on each line it covers.
            let selection = edit_state.and_then(|e| e.selection());

            // Paint the visible window of lines, colouring each line by
            // the spans covering its bytes (uncovered → default colour).
            let mut line_byte = 0usize;
            for (li, line) in live.split('\n').enumerate() {
                let line_start = line_byte;
                line_byte += line.len() + 1; // include the '\n'
                if li < scroll { continue; }
                let row = li - scroll;
                if row >= visible { break; }
                let y = top_y + (row as u32 * line_h) as i32;
                // Selection highlight under the text (before glyphs so they
                // stay on top). A selection crossing this line's end (the
                // '\n') or an empty selected line gets a small trailing block.
                if let Some((sel_s, sel_e)) = selection {
                    let line_lo = line_start;
                    let line_hi = line_start + line.len();
                    if sel_e > line_lo && sel_s <= line_hi {
                        let a = sel_s.max(line_lo) - line_lo;
                        let b = sel_e.min(line_hi) - line_lo;
                        let ax = ceil_u32_local(crate::gui::text::measure(&line[..a], style));
                        let mut w = ceil_u32_local(crate::gui::text::measure(&line[a..b], style));
                        if sel_e > line_hi { w += 6; } // newline included
                        if w < 2 { w = 6; }            // empty line / zero-width
                        rast.rect(target, Rect { x: text_x + ax as i32, y, w, h: line_h },
                                  Fill::Solid(Token::AccentMuted));
                    }
                }
                if line.is_empty() { continue; }
                if spans.is_empty() {
                    rast.text(target, line, style, color, Point { x: text_x, y });
                    continue;
                }
                // Split the line into coloured runs.
                let mut x = text_x;
                let mut run_start = 0usize;
                let mut run_tok = span_token_at(spans, line_start, color);
                for (b, _) in line.char_indices() {
                    let tok = span_token_at(spans, line_start + b, color);
                    if tok != run_tok && b > run_start {
                        let seg = &line[run_start..b];
                        rast.text(target, seg, style, run_tok, Point { x, y });
                        x += ceil_u32_local(crate::gui::text::measure(seg, style)) as i32;
                        run_start = b;
                        run_tok = tok;
                    }
                }
                let seg = &line[run_start..];
                rast.text(target, seg, style, run_tok, Point { x, y });
            }

            // Caret (only when focused / editor present).
            if let Some(cl) = caret_line {
                if cl >= scroll && cl - scroll < visible {
                    let advance = ceil_u32_local(crate::gui::text::measure(caret_prefix, style));
                    let y = top_y + ((cl - scroll) as u32 * line_h) as i32;
                    let caret_rect = Rect {
                        x: text_x + advance as i32,
                        y,
                        w: 2,
                        h: line_h,
                    };
                    rast.rect(target, caret_rect, Fill::Solid(Token::OnSurface));
                }
            }

            // Overlay scrollbar — only when the document overflows. Mirrors
            // paint_scrollbar (Widget::Scroll) so the editor and file views
            // look identical; thumb position tracks the line window.
            if total_lines > visible && rect.h > 0 {
                let content_h = (total_lines as u32) * line_h;
                let track_h = rect.h as u64;
                let thumb_h = ((track_h * track_h) / content_h.max(1) as u64).max(24).min(track_h) as u32;
                let travel = track_h - thumb_h as u64;
                let max_off = (total_lines - visible) as u64;
                let thumb_y = rect.y + if max_off == 0 { 0 } else { (scroll as u64 * travel / max_off) as i32 };
                let thumb_x = rect.x + rect.w as i32 - 6;
                rast.rect_rounded(target,
                    Rect { x: thumb_x, y: thumb_y, w: 4, h: thumb_h },
                    Fill::Solid(Token::OnSurfaceMuted), 2);
            }
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

        Widget::Canvas { id, .. } => {
            // P10.10: the app uploads BGRA pixels via npk_canvas_commit,
            // stored keyed by (window_id, canvas_id). Blit it contain-fit
            // into this rect; muted placeholder until something commits.
            let cid = id.0;
            let wid = target.window_id;
            // Record the actual rect so the app can query it (npk_canvas_rect)
            // and paint 1:1 / map click coordinates.
            super::canvas::record_rect(wid, cid, rect.x, rect.y, rect.w, rect.h);
            let drawn = super::canvas::with_bitmap(wid, cid, |px, w, h| {
                rast.canvas_blit(target, px, w, h, rect);
            }).is_some();
            if !drawn {
                rast.rect(target, rect, Fill::Solid(Token::SurfaceMuted));
            }
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
        Widget::TextArea{ modifiers, .. } |
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
