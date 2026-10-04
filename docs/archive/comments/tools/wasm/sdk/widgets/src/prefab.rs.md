# `tools/wasm/sdk/widgets/src/prefab.rs` @ 5e0102684

## L1-2 · `use alloc::boxed::Box;`

```
//! Prefab components — app-facing "how to build a nopeek UI" cookbook.
//! Apps assemble screens from these, never from raw Row/Column/Modifier.
```

## L14-26 · `pub fn center_box(child: Widget, modifiers: Vec<Modifier>) -> Widget {`

```
// Small uniform inset (4 px) so children — including dividers — get
// breathing room from the window chrome instead of butting against
// the rounded border. The vertical inset combines with the leading /
// trailing zero-size widgets that `prefab::input` and `prefab::footer`
// install at their wrap-Column ends to keep the search row + footer
// row vertically symmetric between chrome and divider.
/// Centre a single child inside a fixed-size box.
///
/// `MinWidth` widens the box but leaves the child at the leading edge —
/// `Align` on a Row is the CROSS axis (vertical), not the main one. The
/// flex spacers are what actually centre it. Every fixed-size cell in the
/// design (workspace pill, tray icon, dock tile, toolbar button) needs
/// this; without it the glyph sits left and the fill runs off to the right.
```

## L40-44 · `pub fn mark(w: u16, h: u16, token: Option<Token>) -> Widget {`

```
/// Solid rectangle of an exact size — the design's fixed-size marks:
/// the dock's running dashes, a list row's 2 px selection edge, a
/// vertical separator inside a toolbar. `token: None` renders nothing
/// but still occupies the space, so a group of marks stays aligned
/// whether or not each one is shown.
```

## L75-81 · `pub fn list_row(`

```
// Selected rows are styled as a subtle elevated card with an accent
// border — the colour cue lives in the border + the icon tint, not in
// a strong fill that would clash with body text on top. Matches the
// "card-style highlight" that AI-generated UIs and modern launchers
// (Raycast, macOS Spotlight) reach for. Padding is Lg so the icon /
// title / subtitle / arrow have visible breathing room inside the
// border on a selected row instead of hugging the stroke.
```

## L102-105 · `row_mods.push(Modifier::Hover(vec![`

```
// Non-selected rows get a subtle hover highlight + a focus
// outline (Tab-nav). Selected rows skip both so hovering /
// focusing an already-selected row doesn't compete with its
// accent fill — keeps visual hierarchy stable.
```

## L115-119 · `let subtitle_text = if subtitle.is_empty() { " ".to_string() } else { subtitle.to_string() };`

```
// Always render subtitle (even when empty) so every row gets the
// same Body+Muted line-height — keeps the hover bar, dividers and
// the footer on a stable grid regardless of which entries have
// descriptions. Empty string → fontdue emits zero glyphs but the
// layout still reserves the Muted line-height slot.
```

## L194-199 · `Widget::Column {`

```
// Wrap with a trailing zero-size widget so the wrap-Column's
// internal `Sm` spacing acts as BOTTOM-margin on the last row.
// `row.Padding(Md=12) + this.spacing(Sm=8) + panel.Padding(Xs=4)
// = 24 px` matches the symmetric 24 px above the footer text
// (panel.spacing Md 12 + row top padding Md 12), keeping the
// footer text centred between divider and chrome bottom.
```

## L240 · `pub fn title_bar(title: &str) -> Widget {`

```
// Convenience converters — many apps format numbers into helper strings.
```

## L265 · `pub fn icon_button(icon: IconId, size: u16, on_click: Option<ActionId>, on_hover: Option<ActionId>) -> Widget {`

```
// ── File-browser / multi-pane prefabs (P10.11 loft) ───────────────────
```

## L267-268 · `pub fn icon_button(icon: IconId, size: u16, on_click: Option<ActionId>, on_hover: Option<ActionId>) -> Widget {`

```
/// Square tap-target with a single centred icon. Used for toolbar chrome
/// (back/forward/up, refresh) and in-row actions.
```

## L271-273 · `mods.push(Modifier::MinWidth(TOOLBAR_BTN));`

```
// A fixed square cell, not a padded glyph: the hit target then stays
// the same whatever glyph size the caller asks for, and a row of
// these lines up (docs/spec/UI_REFRESH.md §3 `toolbar_button`).
```

## L297-299 · `pub fn sidebar_section(label: &str, items: Vec<Widget>) -> Widget {`

```
/// Small uppercase section label above a group of `nav_row`s. Mono and
/// `OnSurfaceFaint` so it reads as structure, not content
/// (docs/spec/UI_REFRESH.md §5).
```

## L319-321 · `pub fn nav_row(`

```
/// One entry inside a sidebar. Selection is an accent tint plus accent
/// text and icon — no border, no full-strength fill (docs/spec/UI_REFRESH.md §3
/// `list_row`).
```

## L355-357 · `Widget::Icon { id: icon, size: 16, modifiers: icon_mods },`

```
// 16, not 24: the row is 30 px tall and padding + a 24 px
// glyph alone overshoots it. 16 is atlas-native, so it stays
// crisp (24 -> 17 would be a scaled blur).
```

## L371 · `pub fn toolbar(children: Vec<Widget>) -> Widget {`

```
/// Horizontal toolbar with built-in padding. Children align centred.
```

## L383-385 · `pub fn breadcrumb(segments: &[(String, ActionId)]) -> Widget {`

```
/// Horizontal row of path segments joined by caret separators.
/// `segments` is a slice of (label, ActionId) — caller supplies a distinct
/// ActionId per segment so clicking one jumps to that depth.
```

## L396-397 · `let here = i + 1 == segments.len();`

```
// The last segment is where you are: a filled chip. The ones
// behind it are the way back, and stay quiet.
```

## L431-432 · `pub fn grid_item(`

```
/// One cell in a grid. Centred large icon above a single-line label.
/// Accent tint + filled background when selected.
```

## L474-475 · `pub fn grid(items: Vec<Widget>, per_row: usize) -> Widget {`

```
/// Wrap a flat list of `grid_item` widgets into fixed-width rows.
/// `per_row` controls how many cells fit horizontally.
```

## L494-495 · `for _ in end..(cursor + per_row) {`

```
// Pad incomplete trailing rows with flex spacers so cells keep
// the same width as full rows.
```

## L515-522 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
// ── Vocab v2 archetypes — modern Tailwind-style prefabs ─────────────
//
// These are the primary building blocks for new apps and AI-generated
// UI. They use the v2 modifier set (Hover, Rounded, WhenDensity) so
// callers get hover-feedback, responsive padding, and consistent
// elevation by default. The earlier prefabs above (panel, list_row,
// nav_row, ...) remain for backward compat with drun + loft and have
// been polished with hover-state in place.
```

## L524-526 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// Visual weight tier for `card`. Maps semantically to design tokens
/// rather than concrete pixel values so a future theme can retune all
/// cards in one place.
```

## L529 · `Inset,`

```
/// Flat surface with a border. Use for inline groupings.
```

## L531 · `Panel,`

```
/// Elevated surface above the window background. Default.
```

## L533 · `Sheet,`

```
/// Strongly elevated, e.g. modal dialogs. Maps to Floating elevation.
```

## L537-543 · `pub fn card(content: Widget, kind: CardKind) -> Widget {`

```
/// Container with consistent padding, rounded corners, and surface
/// background. The visual workhorse of the v2 vocabulary — every
/// non-trivial app screen should contain at least one card.
///
/// Ignores `Elevation` for now (no shadow rendering yet); the kind
/// still determines the surface token + border treatment so card
/// hierarchy is visible even without shadows.
```

## L570-572 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// Visual variant for `button`. Defines the colour pair only; the
/// rest of the chrome (rounded corners, padding, hover lift) is
/// shared so all button styles feel like the same family.
```

## L575 · `Primary,`

```
/// Solid accent fill — primary action of the screen.
```

## L577 · `Secondary,`

```
/// Soft elevated surface — secondary action.
```

## L579 · `Ghost,`

```
/// No background, just the label — tertiary action.
```

## L581 · `Destructive,`

```
/// Danger-coloured fill — destructive action.
```

## L585-587 · `pub fn button(label: &str, style: ButtonStyle, on_click: ActionId) -> Widget {`

```
/// Themed button. Wraps `Widget::Button` with a coherent default
/// chrome plus interactive states (hover, active, focus) so every
/// call site feels like the same button family.
```

## L595-597 · `let label_tint = match style {`

```
// Text colour has to follow the fill, or a filled button paints
// body-coloured text on its own Accent and reads as disabled. Ghost
// and Secondary sit on surface colours, so they keep the default.
```

## L624-626 · `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`

```
/// Semantic kind for `input`. Search adds a leading magnifier icon;
/// Password is a placeholder for masked rendering once the rasterizer
/// supports it (today renders as plain text).
```

## L634-641 · `pub fn input(`

```
/// Themed text input with consistent padding, rounded corners, and
/// elevated surface. Search variant prepends a magnifier icon. An
/// optional `trailing` widget is rendered right-aligned (typical
/// uses: an app-name badge inside a search bar, a clear button).
///
/// `on_submit` is the ActionId fired when the user submits the input
/// (Enter while focused). Pass [`NO_ACTION`] to opt out — apps that
/// route Enter themselves (e.g. drun's launcher) typically do.
```

## L652-655 · `pub fn input_autofocus(`

```
/// `input` that takes focus as soon as its window appears — for a
/// launcher or a dialog whose whole purpose is the field. Everything
/// else should stay unfocused so the window's own keys (arrows, Esc)
/// reach the app instead of a text box the user never clicked.
```

## L680-683 · `let mut wrap_mods: Vec<Modifier> = Vec::with_capacity(3);`

```
// No background — the input blends with the dialog so the search bar
// reads as part of the panel rather than as a stacked card on top.
// Apps that want the elevated-card look can wrap the result in a
// `card(..., CardKind::Inset)` or add `Modifier::Background` themselves.
```

## L695-697 · `size:      24,`

```
// 24 is the atlas-native size — picks the unscaled glyph
// for crisp 4K rendering (smaller sizes scale down from
// the 24 px atlas slot and look fuzzy).
```

## L714-720 · `Widget::Column {`

```
// Wrap with a leading zero-size widget so the wrap-Column's
// internal `Sm` spacing acts as TOP-margin on the search row.
// `panel.Padding(Xs=4) + this.spacing(Sm=8) + row.Padding(Md=12)
// = 24 px` matches the symmetric 24 px below the search text
// (row bottom padding 12 + panel.spacing Md 12), so the search
// row stays vertically centred between chrome top and the
// divider underneath. Mirror trick to `prefab::footer`.
```

## L732-734 · `pub const NO_ACTION: ActionId = ActionId(u32::MAX);`

```
/// Sentinel ActionId for "no action wired up" — useful as a default
/// for `on_submit` etc. when the app routes the event itself. Apps
/// must not use this id for their own actions.
```

## L737-741 · `pub fn dialog(`

```
/// Modal dialog wrapper — title bar at the top, body in the middle,
/// optional footer hint at the bottom. Uses Sheet card styling.
///
/// `min_size` becomes a hard layout constraint so the dialog doesn't
/// collapse below readable dimensions even in a small tile.
```

## L776-777 · `Modifier::WhenDensity(crate::abi::Density::Compact, vec![`

```
// Compact density: tighter padding so the dialog still fits
// in a narrow tile.
```

## L785-788 · `pub fn sidebar_pane(sections: Vec<Widget>) -> Widget {`

```
/// Vertical sidebar container — `SurfaceMuted` background with
/// consistent padding. Children are typically `sidebar_section`s and
/// `nav_row`s; a trailing flex-Spacer is appended automatically so the
/// sections stack to the top and don't stretch.
```

## L803-808 · `pub fn menu_bar(labels: &[(String, ActionId)]) -> Widget {`

```
/// Top menu-bar — flat row of clickable labels.
///
/// Each label gets its own `Padding(Sm)` so the click hit-rect is
/// generous; the Row's `Spacing::Md` keeps a clear gap *between*
/// labels so they don't read as one squished string. A trailing
/// flex-Spacer absorbs any leftover width on the right edge.
```

## L813-816 · `pub fn menu_bar_with_anchors(`

```
/// Menu bar variant that tags each label with a `NodeId` from
/// `anchors[i]`, so the app can attach a `Widget::Popover` to it
/// for the dropdown. `anchors` is matched positionally; pass `&[]`
/// for the no-NodeId case.
```

## L824-826 · `pub fn menu_bar_with_icon(`

```
/// Menu bar with the app's own glyph at the leading edge — the window's
/// identity mark, the way every window in the design carries one
/// (docs/spec/UI_REFRESH.md §5). Pass `IconId::None` for a bare bar.
```

## L877 · `const MENU_BAR_H: u16 = 36;`

```
/// Menu-bar band height (docs/spec/UI_REFRESH.md §5).
```

## L880-885 · `pub fn popover_menu(`

```
/// Build a popover-content surface from a list of menu items.
/// Each item is `(label, action_id)`. The wrapper is a SurfaceElevated
/// card with a 1 px Border and Md radius — same visual language as
/// `card(.., CardKind::Sheet)` but tighter padding for menu density.
/// `selected_index` flags the currently-active option (e.g. View →
/// Grid is the active mode) with an Accent tint.
```

## L893-896 · `pub fn popover_menu_shortcuts(`

```
/// `popover_menu` with a right-hand shortcut column ("Ctrl+S"). `hints`
/// is indexed alongside `items`; a short list or an empty string just
/// leaves that row without one. A menu is where people *learn* the
/// shortcut, so an app that binds keys should show them here.
```

## L908-909 · `}`

```
// No background highlight — the leading Check icon is the
// selection cue, matching macOS-style menu checkmarks.
```

## L924-925 · `Widget::Spacer { flex: 1 },`

```
// Push the shortcut to the right edge. The spacer is here
// even without one, so labels line up across a mixed menu.
```

