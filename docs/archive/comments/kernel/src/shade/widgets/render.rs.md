# `kernel/src/shade/widgets/render.rs` @ 5e0102684

## L1-13 · `#![allow(dead_code)]`

```
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
```

## L25-27 · `pub fn render(`

```
/// Render `widget` + `layout` (trees in lockstep) into `target` using
/// `rast`. Default-state entry — used by paths that don't track any
/// pseudo state (e.g. one-shot debug renders).
```

## L37-49 · `pub fn render_with_state(`

```
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
```

## L60-62 · `scroll_y: u32,`

```
// Window vertical scroll offset (px). Applied to a focused TextArea's
// line window (wheel scroll); Widget::Scroll handles its own offset in
// layout, so this only matters at the TextArea leaf.
```

## L64-65 · `scroll_x: u32,`

```
// Horizontal offset of a focused TextArea (px). Only an editor scrolls
// sideways; every other widget ignores it.
```

## L67-71 · `inherited_tint: Option<Token>,`

```
// Colour inherited from the nearest ancestor carrying `Modifier::Tint`,
// like CSS `color`. A `Tint` on a Row is what apps reach for to say
// "this whole row is accent now"; without inheritance the Row's own
// paint would take it and every Text/Icon inside would silently fall
// back to the default. `None` = no ancestor set one.
```

## L79 · `let subtree_tint = eff.iter().rev()`

```
// This node's own Tint wins for its whole subtree.
```

## L85-88 · `let edit_for_node: Option<&InputEditState> =`

```
// `is_focused && Some([])` (focus exactly here) is the only case
// where the editor caret is painted — descended-focus paths are
// ancestors, not the input itself. paint_node_eff applies that
// check.
```

## L93-95 · `let saved_clip = target.clip;`

```
// A Scroll clips its subtree to its viewport rect so overflowing
// content is masked (and, for a vertical scroll, an overlay scrollbar
// is drawn after). Save/restore the previous clip around the children.
```

## L109 · `let kids = widget_children(widget);`

```
// Recurse — at most one child sits on each path.
```

## L127-131 · `let op = find_opacity_in(&eff);`

```
// Modifier::Opacity acts as a post-paint dampening over the node's
// rect — blend everything already painted there towards the
// Surface token, weighted by (255 - opacity). Lets the SDK
// express "show this at 70 % visibility" without the rasterizer
// trait needing a new parameter.
```

## L138 · `fn descend(path: Option<&[u32]>, i: u32) -> Option<&[u32]> {`

```
/// Helper: peel one index off a state path to descend into child `i`.
```

## L146-149 · `fn paint_scrollbar(`

```
/// Thin overlay scrollbar for a vertical `Widget::Scroll`. Drawn only
/// when the content overflows the viewport — otherwise nothing shows
/// (macOS/GTK overlay-scrollbar idiom). Painted over the content, it
/// reserves no layout space, so toggling it never reflows anything.
```

## L162 · `let off = (viewport.y - child.rect.y).max(0) as u64;     // scrolled px`

```
// scrolled px
```

## L165 · `let thumb_h = ((track_h * track_h) / content_h as u64).max(24).min(track_h) as u32;`

```
// Thumb height proportional to the visible fraction, with a floor.
```

## L181-182 · `fn ceil_u32_local(x: f32) -> u32 {`

```
/// Local mirror of `layout::ceil_u32` — kept private so the caret-paint
/// path doesn't import a layout-module helper.
```

## L189-191 · `fn span_token_at(spans: &[super::abi::Span], off: usize, default: Token) -> Token {`

```
/// Colour token for byte offset `off` from a TextArea's spans, or
/// `default` if no span covers it. Spans are sorted by `start`, so we
/// can stop once a span begins past the offset.
```

## L201 · `fn spans_of(mods: &[super::abi::Modifier]) -> &[super::abi::Span] {`

```
/// Die Farb-Laeufe aus `Modifier::Spans`, oder leer.
```

## L209-212 · `fn leaf_padding(mods: &[Modifier]) -> (u32, u32) {`

```
/// Sum every `Modifier::Padding` in the effective list. Mirrors
/// `layout::padding` so leaf glyph placement matches the layout-side
/// outer-size growth — a single canonical source for "how much
/// padding does this leaf carry".
```

## L231-240 · `fn effective_modifiers(`

```
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
```

## L251 · `for m in base {`

```
// Base: keep all non-pseudo-state modifiers verbatim.
```

## L262 · `for m in base {`

```
// Density (always applies — orthogonal to interactive states).
```

## L270-271 · `if is_disabled {`

```
// Disabled wins over interactive states — when an app marks a
// widget disabled, the user-state visuals shouldn't show through.
```

## L304-305 · `fn find_opacity_in(mods: &[Modifier]) -> u8 {`

```
/// First Opacity in an explicit modifier list. Used by the post-paint
/// opacity dampening pass.
```

## L313-315 · `pub fn tree_has_pseudo_state(tree: &Widget) -> bool {`

```
/// Recursively check whether `tree` contains any pseudo-state modifier
/// or density-conditional modifier. Compositor uses this to skip
/// re-renders on MouseMove when the result wouldn't change anyway.
```

## L335-336 · `fn apply_rect_opacity(target: &mut RasterTarget, rect: Rect, opacity: u8) {`

```
/// Blend every pixel in `rect` towards the Surface token by
/// `255 - opacity`. Rectangle is in window coordinates.
```

## L372-374 · `pub(super) const INPUT_STYLE: super::abi::TextStyle = super::abi::TextStyle::Body;`

```
/// Text style inside a `Widget::Input`. Shared with the click-to-caret
/// hit test in `widgets::offset_at` — measuring with a different style
/// there puts the caret on the wrong character.
```

## L377 · `const GUTTER_PAD_R: i32 = 10;`

```
/// Gap between the widest line number and the gutter's hairline.
```

## L379 · `const GUTTER_PAD_L: i32 = 10;`

```
/// Gap between the hairline and the left edge of the number column.
```

## L382-386 · `pub(super) fn textarea_gutter_w(mods: &[Modifier], total_lines: usize) -> u32 {`

```
/// Width a `TextArea`'s line-number gutter occupies, or 0 when the app
/// didn't ask for one. Sized to the highest line number the buffer can
/// show, so the text doesn't shift sideways as you scroll past 99.
/// **Shared by the renderer and the click-to-caret hit test** — if these
/// two disagree, clicks land on the wrong column.
```

## L404-406 · `const TEXTAREA_BAR_W: i32 = 6;`

```
/// Width the TextArea's overlay scrollbar occupies on the right. Always
/// reserved, drawn or not, so the text column doesn't change width the
/// moment a line pushes the document past the viewport.
```

## L409-412 · `pub(super) fn textarea_text_column(rect: Rect, mods: &[Modifier], total_lines: usize)`

```
/// A `TextArea`'s text column: left edge and visible width, both in screen
/// px. **Shared by the renderer, the click-to-caret hit test and the
/// caret-follow scroll** — `scroll_x` is derived from these, so if they
/// disagree the caret scrolls to a different place than it is drawn.
```

## L418-419 · `let right = rect.x + rect.w as i32 - pad.max(TEXTAREA_BAR_W);`

```
// The scrollbar overlays the right padding band rather than sitting
// beside it, so the wider of the two is what the text must keep clear.
```

## L424-431 · `pub(super) fn textarea_content_w(value: &str, mods: &[Modifier]) -> u32 {`

```
/// Width of the widest line in a `TextArea`, in px. **Shared by the layout
/// (which turns it into the scrollable range) and the renderer (which sizes
/// the scrollbar thumb from it).**
///
/// A `TextArea` is always Mono, and mono means a uniform advance — so the
/// longest line by character count IS the widest one. Find it by counting
/// and measure only that line, instead of measuring every line of the
/// document on every keystroke.
```

## L444-445 · `let mut bg: Option<Token> = None;`

```
// Last write wins so state mods (hover, etc.) appended after the
// base list override base values cleanly.
```

## L460-462 · `let radius = rounded.unwrap_or_else(|| border.map(|(_, _, r)| r).unwrap_or(0));`

```
// Rounded modifier wins for the outer corner radius. Border's own
// radius applies only as a fallback so existing apps (which set the
// radius via Border) keep their look without code changes.
```

## L465-466 · `if let Some((tok, width)) = ring {`

```
// Focus ring first: it sits OUTSIDE the node rect, so the background
// and border paint over its inner edge and leave a clean band.
```

## L496-505 · `fn paint_node_eff(`

```
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
```

## L518-524 · `let leaf_pad = leaf_padding(eff);`

```
// Inner-rect origin for leaf glyph placement. The OUTER rect is
// sized to include any `Modifier::Padding` (see layout.rs leaf
// measure paths); the actual text/icon must shift in by the
// padding amount so the glyphs sit centred inside the padded
// band — without this `prefab::menu_bar` and `prefab::badge`
// would render with their text glued to the left edge of their
// own padded background instead of inside it.
```

## L531-532 · `let mut color = inherited_tint.unwrap_or(`

```
// Style default, overridable by Modifier::Tint (e.g. an active
// workspace pill tinted OnAccent so it reads on the Accent fill).
```

## L542-543 · `let px = super::layout::font_size_of(*style, eff);`

```
// Same resolver the layout pass used — otherwise the glyphs
// would outgrow the box that was measured for them.
```

## L550-556 · `let mut q88: u32 = 256;`

```
// Q8.8 fixed-point scale: 256 = 1.0×. Resolved from the
// effective modifier list so Hover/Focus/Active states can
// inflate the icon (dock cells use this for a Mac-style
// hover bump). The scaled glyph stays centred inside the
// original cell rect — layout doesn't change, so neighbours
// don't shift; a small overflow can paint over the cell
// background (the dock's Tray catches it cleanly).
```

## L573-577 · `let has_bg = eff.iter().any(|m| matches!(m, Modifier::Background(_)));`

```
// If `paint_modifiers_eff` already painted a Background or
// Border for this button, the chrome is done — skip the
// hardcoded Accent fill so prefab::button(Destructive) /
// (Ghost) styles render correctly. Fall back to Accent only
// when the button has no explicit background.
```

## L582-586 · `let mut color = inherited_tint.unwrap_or(Token::OnSurface);`

```
// Label + icon follow `Modifier::Tint` like Text and Icon do.
// They used to be hardcoded — OnSurface for the label, OnAccent
// for the icon — so a filled button painted dark-on-dark text on
// its own Accent fill and the two halves disagreed with each
// other. The default stays OnSurface for the unfilled styles.
```

## L606-619 · `let live_value: &str = match edit_state {`

```
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
```

## L624-628 · `let focused = edit_state.is_some();`

```
// A focused empty field shows the caret ALONE. Drawing the
// placeholder too puts both at the same x — the caret ends up
// welded to the first letter ("|search"), which reads as a
// glitch rather than as a hint. The design never shows the two
// together either: rest = placeholder, focus = text + caret.
```

## L630 · `let text_x = inner_x + 4;   // built-in chrome + the node's padding`

```
// built-in chrome + the node's padding
```

## L632-638 · `let sel = edit_state.and_then(|e| e.selection());`

```
// **Die Auswahl UNTER dem Text, vor den Glyphen.** Ziehen,
// Shift+Pfeil und Strg+C arbeiteten hier laengst — nur sah man
// nichts davon, weil die `Input` als einziges Textfeld ihre
// Auswahl nie gemalt hat. Eine Auswahl, die es gibt und die man
// nicht sieht, ist schlimmer als keine: man markiert, kopiert und
// weiss nicht, was in der Ablage liegt. Dieselbe Rechnung und
// dasselbe Token wie in `TextArea` weiter unten.
```

## L654-657 · `let spans = spans_of(eff);`

```
// `Modifier::Spans` faerbt LAEUFE des Textes — die
// Adresszeile hebt damit die registrierbare Domain hervor
// und blendet den Rest ab. Ohne Spannen bleibt es der eine
// billige Aufruf von vorher.
```

## L663-666 · `let mut x = text_x;`

```
// In Laeufe gleicher Farbe zerlegen und einzeln setzen —
// dieselbe Rechnung wie in `TextArea`. Gemessen wird je
// Lauf, damit der naechste dort anfaengt, wo der vorige
// wirklich endete.
```

## L689 · `rast.text(target, placeholder.as_str(), INPUT_STYLE,`

```
// Placeholder is a hint, not content — faint, like the design.
```

## L694-695 · `if let Some(e) = edit_state {`

```
// Paint the caret. Only when an editor exists (focused) —
// unfocused inputs render flat text.
```

## L700-701 · `None    => e.value.as_str(),`

```
// Cursor mis-aligned (defensive: shouldn't happen)
// → drop to end of value.
```

## L718-725 · `let live: &str = match edit_state {`

```
// The editing surface. Background / border come from the
// app's modifiers (handled in paint_modifiers_eff). When
// focused, the live buffer leads the tree's `value` by one
// round-trip — render from `edit_state` so typing is instant.
// `spans` (syntax-highlight colours, byte offsets over the
// committed value) are applied to the live buffer; freshly
// typed bytes beyond span coverage fall back to the default
// colour for one frame until the app re-commits.
```

## L731-733 · `let px = super::layout::font_size_of(style, eff);`

```
// Zoom: every metric below has to come from the SAME px, or the
// caret, the selection blocks and the gutter drift apart from
// the glyphs they belong to.
```

## L736 · `let mut color = Token::OnSurface;`

```
// Resolve default text colour: OnSurface, Tint overrides.
```

## L743-746 · `let text_x = col_x - scroll_x as i32;`

```
// Sideways offset: the glyphs move left, the column does not.
// Everything drawn in text coordinates below goes through
// `text_x`, so the caret and the selection blocks can never
// drift apart from the glyphs they belong to.
```

## L751 · `let (caret_line, caret_prefix) = match edit_state {`

```
// Caret line/column (byte prefix within the caret's line).
```

## L762-766 · `let total_lines = total_lines_all;`

```
// Line window from the stored scroll offset (wheel / drag /
// caret-follow). The view is authoritative here: caret-follow
// happens on caret MOVES (handle_input_key adjusts scroll_y), so
// the render must NOT re-pull to the caret every frame — that
// would defeat manual wheel/drag scrolling.
```

## L771-774 · `if gutter_w > 0 {`

```
// Line-number gutter: a right-aligned column of numbers and a
// hairline separating it from the text. Drawn here rather than
// by the app because only the compositor knows the scroll
// position of its own viewport.
```

## L793-796 · `let saved_clip = target.clip;`

```
// From here on everything is drawn in text coordinates, which
// `scroll_x` can push left of the column — clip to the column so
// no glyph paints over the gutter or past the widget's edge. The
// gutter above and the scrollbar below stay outside the clip.
```

## L809 · `if live.is_empty() {`

```
// Empty buffer → muted placeholder on the first line.
```

## L815-816 · `let selection = edit_state.and_then(|e| e.selection());`

```
// Selected byte range (anchor↔caret), painted as a highlight
// block under the text on each line it covers.
```

## L819-820 · `let mut line_byte = 0usize;`

```
// Paint the visible window of lines, colouring each line by
// the spans covering its bytes (uncovered → default colour).
```

## L824 · `line_byte += line.len() + 1; // include the '\n'`

```
// include the '\n'
```

## L829-831 · `if let Some((sel_s, sel_e)) = selection {`

```
// Selection highlight under the text (before glyphs so they
// stay on top). A selection crossing this line's end (the
// '\n') or an empty selected line gets a small trailing block.
```

## L840 · `if sel_e > line_hi { w += 6; } // newline included`

```
// newline included
```

## L841 · `if w < 2 { w = 6; }            // empty line / zero-width`

```
// empty line / zero-width
```

## L851 · `let mut x = text_x;`

```
// Split the line into coloured runs.
```

## L869 · `if let Some(cl) = caret_line {`

```
// Caret (only when focused / editor present).
```

## L886-888 · `if total_lines > visible && rect.h > 0 {`

```
// Overlay scrollbar — only when the document overflows. Mirrors
// paint_scrollbar (Widget::Scroll) so the editor and file views
// look identical; thumb position tracks the line window.
```

## L902-905 · `let content_w = textarea_content_w(live, eff);`

```
// …and its sideways twin, along the bottom of the text column.
// Lines are never wrapped, so a single long line is enough to
// make it appear. Same content width the layout derived
// `max_scroll_x` from, so bar and offset can't disagree.
```

## L913-915 · `let thumb_x = col_x + if max_off == 0 { 0 }`

```
// The caret may sit a margin past the end while the app has
// not re-committed the longer line yet — clamp the thumb
// rather than the offset, or the view would fight the caret.
```

## L926 · `rast.rect(target, rect, Fill::Solid(Token::Border));`

```
// Outer stroke + inner fill if checked.
```

## L940-942 · `let d = super::layout::SLIDER_THUMB.min(rect.h).min(rect.w);`

```
// A thin track through the middle, filled up to the thumb's
// centre. The thumb travels inside the rect so it is never
// clipped at either end.
```

## L968-970 · `let cid = id.0;`

```
// P10.10: the app uploads BGRA pixels via npk_canvas_commit,
// stored keyed by (window_id, canvas_id). Blit it contain-fit
// into this rect; muted placeholder until something commits.
```

## L973-974 · `super::canvas::record_rect(wid, cid, rect.x, rect.y, rect.w, rect.h);`

```
// Record the actual rect so the app can query it (npk_canvas_rect)
// and paint 1:1 / map click coordinates.
```

## L976-979 · `let mut zoom: u32 = 256;`

```
// `Modifier::Scale` zooms the blit — the rect and the stored
// bitmap are untouched, so an app can zoom without re-uploading
// (or even re-decoding) a single pixel. Same Q8.8 meaning as on
// an Icon: 256 = 1.0×, here relative to the contain-fit size.
```

## L994-996 · `super::canvas::Pixels::I420 { y, u, v, ys, cs, coding } => {`

```
// Planar 4:2:0 converts inside the blit, at destination
// size. See `canvas.rs` for why it is not converted on
// the way in.
```

## L1010-1011 · `Widget::Column { .. } | Widget::Row { .. } | Widget::Stack { .. }`

```
// Containers paint nothing themselves — their Background /
// Border modifiers are already handled above. Children recurse.
```

## L1015 · `Widget::Popover { .. } | Widget::Tooltip { .. } | Widget::Menu { .. } => {}`

```
// Reserved slots — logged in scene_commit, skipped here.
```

## L1018 · `_ => {}`

```
// Spacer + unknowns = no paint.
```

## L1023 · `fn modifiers_of(w: &Widget) -> &[Modifier] {`

```
// ── Helpers (mirror debug.rs) ────────────────────────────────────────
```

