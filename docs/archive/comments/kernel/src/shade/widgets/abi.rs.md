# `kernel/src/shade/widgets/abi.rs` @ 5e0102684

## L1-16 · `#![allow(dead_code)]`

```
//! Widget ABI — frozen v1 wire contract.
//!
//! This module defines the shape of the widget tree as it crosses the WASM
//! sandbox boundary. Every type here is part of the persistent ABI: enum
//! variant order, #[repr] discriminants, and struct field order are all
//! frozen at v1 (WIRE_VERSION = 0x01).
//!
//! Rules (see docs/archive/PHASE10_WIDGETS.md "ABI stability & future-proofing"):
//!   - All ABI-visible enums carry #[non_exhaustive]
//!   - New variants appended only — never inserted, never reordered
//!   - Removing a variant = wire-version bump
//!   - Reserved variants use #[allow(dead_code)] to hold the slot
//!   - #[repr(u8)] or #[repr(u16)] where the variant index is the ABI
//!
//! P10.0 scope: signatures + constants, no logic. Serialization lands in
//! P10.1, deserialization in P10.2.
```

## L23 · `pub const WIRE_VERSION: u8 = 0x01;`

```
// ── Wire version ──────────────────────────────────────────────────────
```

## L25-29 · `pub const WIRE_VERSION: u8 = 0x01;`

```
/// Wire protocol version byte. Prefixed to every `npk_scene_commit` payload.
/// Compositor rejects unknown versions with `-1`.
///
/// Bump when the wire contract changes incompatibly. Forward-compatible
/// additions (Option<T> at struct tail, appended variants) do not bump.
```

## L32 · `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`

```
// ── Geometry ──────────────────────────────────────────────────────────
```

## L34 · `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`

```
/// Point in window coordinates (px).
```

## L41 · `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`

```
/// Size in pixels.
```

## L48 · `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`

```
/// Rectangle in window coordinates (px).
```

## L57-59 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`

```
/// A coloured run inside a `Widget::TextArea`'s `value` (byte offsets +
/// colour token). Mirror of the SDK `Span`. The compositor colours each
/// byte of the live buffer by the span covering it; uncovered → default.
```

## L67 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]`

```
// ── Identifiers ───────────────────────────────────────────────────────
```

## L69-70 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]`

```
/// App-defined action identifier. Attached to `on_click`, `on_submit`,
/// `on_toggle` modifiers. Echoed back via `Event::Action(ActionId)`.
```

## L74-75 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]`

```
/// Canvas leaf identifier. Matches the `canvas_id` passed to
/// `npk_canvas_commit` for pixel delivery.
```

## L79 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]`

```
/// Node identifier inside a tree (structural path hash, compositor-assigned).
```

## L83 · `#[repr(u8)]`

```
// ── Theme tokens ──────────────────────────────────────────────────────
```

## L85-89 · `#[repr(u8)]`

```
/// Theme tokens. Apps never specify hex colors — the compositor resolves
/// tokens against the active palette at raster time.
///
/// Integer values frozen on v1 release. New tokens **appended only** —
/// existing values never reassigned.
```

## L94 · `Surface         = 0,`

```
// Surfaces
```

## L99 · `OnSurface       = 3,`

```
// Text
```

## L104 · `Accent          = 6,`

```
// Accent
```

## L108 · `Border          = 8,`

```
// Semantic
```

## L114-115 · `Page            = 12,`

```
// UI-Refresh ramp extension (see docs/spec/UI_REFRESH.md §1).
/// Content canvas — below `Surface`. Editor body, page, terminal.
```

## L117 · `SurfaceHover    = 13,`

```
/// Hover fill / chips — above `SurfaceMuted`.
```

## L119 · `OnSurfaceFaint  = 14,`

```
/// Third text level: section headings, meta columns, disabled.
```

## L121 · `AccentRing      = 15,`

```
/// Accent at 22 % over `Surface` — focus rings.
```

## L123 · `AccentLine      = 16,`

```
/// Accent at 45 % over `Surface` — focused window border.
```

## L126-134 · `CodeKeyword     = 17,`

```
// ── Code tokens (syntax highlighting) ─────────────────────────────
//
// A second, independent ramp. The tokens above describe *chrome*;
// these describe *source text*. An editor needs both at once, and
// reusing `Accent`/`Warning` for keywords and strings tied the
// syntax colours to the wallpaper — a whole language got three
// colours. Resolved from the active code scheme (`set code.scheme`),
// never from the accent.
/// Declaration / storage keywords: `fn` `let` `def` `class` `int`.
```

## L136 · `CodeControl     = 18,`

```
/// Control flow and imports: `if` `for` `return` `import` `match`.
```

## L138 · `CodeString      = 19,`

```
/// String and character literals, quotes included.
```

## L140 · `CodeComment     = 20,`

```
/// Comments, any syntax.
```

## L142 · `CodeNumber      = 21,`

```
/// Numeric literals.
```

## L144 · `CodeFunction    = 22,`

```
/// Function names — declaration and call site.
```

## L146 · `CodeType        = 23,`

```
/// Type / class names and markup tag names.
```

## L148 · `CodeVariable    = 24,`

```
/// Attribute names, JSON keys, decorators.
```

## L150 · `CodeConstant    = 25,`

```
/// Language constants (`true` `None` `null`) and escape sequences.
```

## L152 · `}`

```
// Appended only — values frozen forever.
```

## L155 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]`

```
// ── Icons ─────────────────────────────────────────────────────────────
```

## L157 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]`

```
/// Icon identifier — an index into the Phosphor atlas (P10.9).
```

## L162-171 · `macro_rules! icon_ids {`

```
/// The named icons. `IconId` is a NUMBER, not a closed enum: a module that
/// only passes an icon on (the dock showing an app's icon) must not have to
/// know every icon there is — with an enum, one new icon meant rebuilding
/// dock, drun and bar, or they showed a blank file instead. The atlas is
/// looked up by number, and a number it lacks draws nothing.
///
/// Wire-identical to the enum this replaced: postcard writes a variant
/// index and a `u16` as the same varint, and the numbers are the old
/// discriminants. Values frozen; append only — and the atlas
/// (`tools/regen-icons`) must carry every number named here.
```

## L177 · `pub const ALL: &'static [(&'static str, IconId)] =`

```
/// Every named icon, for tests and tooling.
```

## L239 · `#[repr(u8)]`

```
// ── Accessibility roles ───────────────────────────────────────────────
```

## L241-242 · `#[repr(u8)]`

```
/// A11y role tag. v1 stores but does not consume. Freezing the enum now
/// avoids a wire-version bump when screen readers / UI automation land.
```

## L247 · `None      = 0,`

```
/// Decorative only — skip in traversal.
```

## L259 · `}`

```
// Appended only.
```

## L262 · `#[repr(u8)]`

```
// ── Text style ────────────────────────────────────────────────────────
```

## L264-265 · `#[repr(u8)]`

```
/// Typographic style token. Maps to Inter Variable weight/size tuple at
/// raster time. `Mono` routes to the Spleen bitmap font (terminal look).
```

## L275 · `Heading = 5,`

```
/// 18 px regular weight (vocab-v3 append).
```

## L277 · `}`

```
// Appended only.
```

## L280 · `#[non_exhaustive]`

```
// ── Fill (rasterizer-side only) ───────────────────────────────────────
```

## L282-283 · `#[non_exhaustive]`

```
/// Fill description passed to the rasterizer. Never appears in the wire
/// tree directly — constructed from Modifier tokens during raster setup.
```

## L288 · `}`

```
// Appended only (gradients etc. in future versions).
```

## L291 · `#[repr(u16)]`

```
// ── Effect IDs (reserved) ─────────────────────────────────────────────
```

## L293-294 · `#[repr(u16)]`

```
/// Named GPU effect reference. Populated in Phase 12 (Xe render engine).
/// CPU rasterizer treats `.effect(_)` as no-op.
```

## L299 · `None = 0,`

```
/// Reserved placeholder — no effects registered in v1.
```

## L301 · `}`

```
// Appended only.
```

## L304 · `#[repr(u8)]`

```
// ── Layout primitives ─────────────────────────────────────────────────
```

## L306 · `#[repr(u8)]`

```
/// Row/Column alignment on the cross axis.
```

## L315 · `}`

```
// Appended only.
```

## L318 · `#[repr(u8)]`

```
/// Scroll container axis.
```

## L326 · `}`

```
// Appended only.
```

## L329 · `#[repr(u8)]`

```
// ── Container-query density ───────────────────────────────────────────
```

## L331-334 · `#[repr(u8)]`

```
/// Compositor-classified window size bucket. Apps reference these via
/// `Modifier::WhenDensity(Density, ...)` to adapt layout to the available
/// space without picking pixel breakpoints. Thresholds live once in the
/// compositor (Compact <600 px, Regular 600–1200 px, Spacious >1200 px).
```

## L342 · `}`

```
// Appended only.
```

## L345 · `#[non_exhaustive]`

```
// ── Animation ─────────────────────────────────────────────────────────
```

## L347-348 · `#[non_exhaustive]`

```
/// Transition curve. Deterministic fixed-point math lives in the
/// compositor; the wire form just carries the curve choice.
```

## L352 · `Spring,`

```
/// Spring physics with default stiffness/damping (compositor-owned).
```

## L354 · `Linear { ms: u16 },`

```
/// Linear interpolation over `ms` milliseconds.
```

## L356 · `}`

```
// Appended only.
```

## L359 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`

```
/// Drop-shadow parameters (reserved — Modifier::Shadow).
```

## L367 · `#[non_exhaustive]`

```
// ── Modifier ──────────────────────────────────────────────────────────
```

## L369-374 · `#[non_exhaustive]`

```
/// Modifier applied to a widget node. Modifiers are carried as a `Vec` on
/// each `Widget` so ordering is preserved (affects rendering: padding
/// outside background, border on top, etc.).
///
/// Variant order frozen at v1. Reserved slots below are declared so their
/// wire indices do not shift when v2 implements them.
```

## L378-379 · `Padding(u16),`

```
// ── Active in v1 ──────────────────────────────────────────────────
/// Inner padding (px at 1× scale, scaled at raster time).
```

## L381 · `Margin(u16),`

```
/// Outer margin (px at 1× scale).
```

## L383 · `Background(Token),`

```
/// Background token fill.
```

## L385 · `Border {`

```
/// Border: token, width (px), corner radius (px).
```

## L391-392 · `Opacity(u8),`

```
/// Opacity 0..=255 (0 = fully transparent, 255 = opaque). Fixed-point
/// on purpose — no float non-determinism.
```

## L394 · `Transition(Transition),`

```
/// Declares that this widget animates when its props change.
```

## L396 · `OnClick(ActionId),`

```
/// Click handler — compositor synthesizes `Event::Action(id)` on hit.
```

## L398 · `OnHover(ActionId),`

```
/// Hover handler.
```

## L401-404 · `#[allow(dead_code)]`

```
// ── Reserved slots (v2+) ──────────────────────────────────────────
// CPU rasterizer treats these as no-ops in v1. Slots declared now so
// wire indices do not shift when the GPU rasterizer (Phase 12)
// implements them. Do not insert before these.
```

## L411 · `#[allow(dead_code)]`

```
/// A11y role override (v1 reads but does not consume).
```

## L414 · `Tint(Token),`

```
/// Paint an Icon in the given Token color instead of OnSurface.
```

## L417-420 · `Hover(Vec<Modifier>),`

```
// ── Vocab v2 (Tailwind-style modifiers) ───────────────────────────
// Pseudo-state modifier lists — compositor merges the inner list onto
// the widget when the state matches; the tree itself stays static
// across hovers (no app round-trip).
```

## L425 · `WhenDensity(Density, Vec<Modifier>),`

```
/// Container query — apply inner modifiers only at the given density.
```

## L427 · `Scale(u16),`

```
/// Uniform scale, Q8.8 fixed-point. 256 = 1.0× (identity).
```

## L429 · `MinWidth(u16),`

```
/// Layout minimum width (px at 1× scale).
```

## L431 · `MaxWidth(u16),`

```
/// Layout maximum width (px at 1× scale).
```

## L433 · `Rounded(u8),`

```
/// Corner radius (px at 1× scale) without a Border.
```

## L435-438 · `Flex(u8),`

```
/// CSS-style flex-grow on the main axis of the parent Row/Column.
/// Adds the widget's intrinsic main size as a basis and absorbs a
/// proportional share of the leftover alongside any
/// `Spacer { flex }` siblings. `Flex(0)` = no flex.
```

## L440-442 · `NodeId(NodeId),`

```
/// Tag a widget with an app-chosen `NodeId`. Layout records the
/// tagged widget's rect into a side table; `Widget::Popover`'s
/// `anchor` field looks rects up there at render time.
```

## L444-447 · `Ring { token: Token, width: u8 },`

```
/// Focus ring — a stroke of `width` px drawn just OUTSIDE the node's
/// rect, under any Border. Mirrors CSS `box-shadow: 0 0 0 Npx`, which
/// is how the design expresses focus. Costs no layout space; the
/// caller must leave room (a row's gap is enough at width ≤ 3).
```

## L449-451 · `MinHeight(u16),`

```
/// Layout minimum height (px at 1× scale). The design pins row
/// heights (30/34/36/44); without this a row's height is whatever
/// its tallest glyph plus padding happens to be.
```

## L453 · `MaxHeight(u16),`

```
/// Layout maximum height (px at 1× scale).
```

## L455-458 · `LineNumbers(bool),`

```
/// Draw a line-number gutter down the left edge of a `TextArea`.
/// The compositor owns that widget's viewport, so only it can keep
/// the numbers in step with scrolling — an app-drawn column would
/// drift the moment the buffer scrolls.
```

## L460-469 · `PaddingXY { x: u16, y: u16 },`

```
/// Per-axis inner padding (px at 1× scale). `Padding` is uniform;
/// the design routinely wants different insets per axis (a search
/// field is `padding: 0 10px` with its height set by a clamp), and
/// forcing one value makes the horizontal air hostage to the row
/// height. Sums with any `Padding` on the same node.
///
/// APPENDED after LineNumbers, not before it: LineNumbers had already
/// shipped, and inserting ahead of a live variant renumbers it on the
/// wire — the running app keeps sending the old index and the
/// compositor decodes garbage (spell rendered an empty window).
```

## L471-475 · `Autofocus,`

```
/// This text widget takes focus when its window first appears.
/// Opt-in: the compositor used to auto-focus the first `Input` it
/// found, which is right for a launcher and wrong for anything with
/// a search box — a file browser's list lost every arrow key to a
/// field the user never clicked.
```

## L477-481 · `FontSize(u16),`

```
/// Font size override in px for a `Widget::Text` — replaces the size
/// its `TextStyle` resolves to; the face (proportional vs mono) still
/// comes from the style. Layout measures at the override, so the text
/// keeps a correct box. Clamped to `FONT_SIZE_RANGE`; ignored on every
/// other widget.
```

## L483-487 · `CanvasOffset { x: i32, y: i32 },`

```
/// Pan a `Widget::Canvas`'s content by this many px, relative to the
/// centred contain-fit position. Pairs with `Modifier::Scale`: scale
/// decides how big the image is drawn, this decides which part of it
/// the rect shows. Clamped to the overhang, so it can never push the
/// content out of view. Ignored on every other widget.
```

## L489-502 · `Spans(alloc::vec::Vec<Span>),`

```
/// Farbige LAEUFE ueber den Text eines `Widget::Input`, in Byte-
/// Offsets — dasselbe, was `Widget::TextArea.spans` fuer den Editor tut.
///
/// **Wofuer es gebaut wurde: die Adresszeile.** Ein Browser hebt die
/// registrierbare Domain hervor und blendet den Rest ab, und das ist
/// keine Zierde, sondern die Anti-Phishing-Anzeige: in
/// `https://paypal.com.betrug.ru/login` heisst die Domain `betrug.ru`,
/// und ohne die Hervorhebung liest das Auge das erste, was wie ein Name
/// aussieht. Die App rechnet die Spanne selbst aus (beak nimmt die echte
/// Public Suffix List) — der Baukasten faerbt nur, was ihm gesagt wird,
/// und weiss nichts von URLs.
///
/// Nicht abgedeckte Bytes behalten die Vorgabefarbe. Ueberlappende oder
/// unsortierte Spannen sind erlaubt; die spaetere gewinnt.
```

## L504-510 · `OnMotion(ActionId),`

```
/// Fire `Event::Action(id)` while the pointer MOVES over this widget,
/// at most every [`MOTION_INTERVAL_MS`] — and at once when the pointer
/// moves onto it from another `OnMotion` target (deepest wins, like
/// `OnClick`). For "show the controls while the mouse is being used":
/// apps never get raw pointer motion, and `OnHover` fires only on
/// entering, so a pointer that rests and then moves again inside the
/// same widget would go unnoticed.
```

## L512 · `}`

```
// Appended only.
```

## L515-516 · `pub const FONT_SIZE_MIN: u16 = 6;`

```
/// Accepted range for `Modifier::FontSize`. The ceiling keeps a module
/// from filling the glyph cache with oversized bitmaps.
```

## L520 · `pub const MOTION_INTERVAL_MS: u32 = 200;`

```
// ── Widget ────────────────────────────────────────────────────────────
```

## L522-524 · `pub const MOTION_INTERVAL_MS: u32 = 200;`

```
/// Upper end of `Widget::Slider::value` — per mille, fine enough for a
/// seek bar across a full-width window.
/// Throttle of `Modifier::OnMotion`.
```

## L529-536 · `#[non_exhaustive]`

```
/// Widget tree node. A single `Widget` = root of a render commit.
///
/// Variant order frozen at v1. Reserved slots (`Popover`/`Tooltip`/`Menu`)
/// are declared so their wire indices do not shift when v2 implements
/// them — compositor rejects them with a log until then.
///
/// Struct-variant field order is also part of the ABI (postcard serializes
/// fields in declaration order).
```

## L540 · `Column {`

```
// ── Containers ────────────────────────────────────────────────────
```

## L563 · `Text {`

```
// ── Leaves ────────────────────────────────────────────────────────
```

## L575 · `label:     String,`

```
/// Text label (empty for icon-only buttons).
```

## L577 · `icon:      IconId,`

```
/// Optional icon (IconId::None = no icon).
```

## L604-607 · `Popover {`

```
// ── Overlay widgets ───────────────────────────────────────────────
// Out-of-flow rendering on top of the main tree. Cannot be
// retrofitted without breaking every serialized tree. Do not
// insert before these.
```

## L609-614 · `Popover {`

```
/// Floating overlay anchored to a `Modifier::NodeId`-tagged widget
/// elsewhere in the tree. Renders on top of everything (z-order)
/// at `(anchor.x, anchor.y + anchor.h)`, flipping above the anchor
/// when there is no room below. `on_dismiss` fires whenever a
/// click lands outside both the popover content and the anchor
/// rect.
```

## L632-637 · `TextArea {`

```
/// Multi-line text editor. The compositor owns a 2-D caret (arrows
/// move within/across lines, Enter inserts a newline, Home/End are
/// line-relative, PageUp/PageDown scroll a viewport). `value` is the
/// whole `\n`-separated document; buffer mutations emit
/// `Event::InputChange { value }` like `Input`. No `on_submit` —
/// Enter is a newline. See the SDK mirror for the full contract.
```

## L641 · `spans:       Vec<Span>,`

```
/// Syntax-highlight colour runs over `value` (byte offsets).
```

## L645-652 · `Slider {`

```
/// Horizontal value track with a draggable thumb. `value` runs from
/// 0 to [`SLIDER_MAX`]. The compositor owns the drag: the thumb follows
/// the pointer without a round trip, and the app hears
/// `Event::Slide { action: on_change, .. }` — `done: false` while the
/// value moves, once more with `done: true` on release. While a drag
/// is in progress the compositor keeps its own value on screen, so an
/// app that re-commits mid-drag (a player advancing its position)
/// does not yank the thumb back.
```

## L658 · `}`

```
// Appended only.
```

## L661 · `#[non_exhaustive]`

```
// ── Events (compositor → app) ─────────────────────────────────────────
```

## L663-668 · `#[non_exhaustive]`

```
/// Input event delivered to a WASM app via `npk_event_poll` /
/// `npk_event_wait`.
///
/// Note: `InputChange` carries an owned `String`, so this enum is
/// `Clone`-only — not `Copy`. All call sites pass `Event` by value
/// or clone explicitly; no fast-path lost.
```

## L672 · `Key(crate::input::KeyCode),`

```
/// Keyboard input. Uses the existing Phase 8 KeyCode (already stable).
```

## L674 · `Action(ActionId),`

```
/// User-defined action (synthesized from `OnClick` / `OnHover`).
```

## L676 · `MouseMove { x: i32, y: i32 },`

```
/// Mouse pointer moved (window-local coords).
```

## L678 · `MouseButton {`

```
/// Mouse button pressed/released at position.
```

## L685 · `Focus(bool),`

```
/// Window focus changed.
```

## L687-692 · `InputChange { value: alloc::string::String },`

```
/// The focused `Widget::Input` had its value mutated by the
/// compositor's editor (printable key, Backspace, Delete). Carries
/// the new buffer contents — the app typically mirrors `value` into
/// its own state and re-commits the tree with the matching `value`.
/// Cursor-only navigation (Left / Right / Home / End) does **not**
/// fire this event; the caret is purely compositor-side state.
```

## L694-696 · `ContextAction(ActionId),`

```
/// Right-click hit-test result. Same hit-test as `Action`, but fired
/// for `MouseButton::Right` instead of `Left`. Apps use it to open
/// context menus (Popover) without consuming the primary click.
```

## L698-700 · `Open(alloc::string::String),`

```
/// "Open this resource" — delivered when `npk_open` targets an app
/// already running (instead of spawning a duplicate). Payload is the
/// launch argument (e.g. a file path). Enables singleton-with-tabs.
```

## L702-704 · `Wheel { dy: i32 },`

```
/// Mouse wheel over a focused app with no `Widget::Scroll` to consume it.
/// `dy` is pixel-scaled (positive = scroll down). Canvas/surface apps
/// scroll their own viewport; others ignore it.
```

## L706-711 · `Clipboard(ClipKind),`

```
/// A clipboard chord (Ctrl+C / Ctrl+X / Ctrl+V) that no focused text
/// widget consumed, delivered to the focused app so it can act on its
/// own selection (e.g. loft copies/moves the selected file). Apps
/// built against an older SDK fail to decode this appended variant and
/// skip it — harmless. MUST stay in lockstep with the SDK copy in
/// `tools/wasm/sdk/widgets/src/abi.rs` (postcard variant order).
```

## L713-718 · `Picked { path: alloc::string::String, tag: u32 },`

```
/// A file-picker request this app started via `npk_pick` finished.
/// `path` is the npkFS path the user chose, or **empty if they
/// cancelled**. `tag` is the caller's own value from `npk_pick`,
/// returned unchanged — the picker roundtrip is asynchronous, so an
/// app running several dialogs (open / save-as / …) uses it to tell
/// which one came back. The kernel never interprets it.
```

## L720-727 · `CloseRequest,`

```
/// The user asked to close this window (Mod+Q, the title-bar X) and
/// the app opted into being asked via `npk_window_set_close_guard`.
/// The window is still open: save, prompt, then call
/// `npk_close_widget` to go — or ignore it to stay.
///
/// Not a promise of veto power. A second close gesture, or a few
/// seconds of silence, closes the window anyway: an app must never be
/// able to make its window unclosable.
```

## L729-739 · `Chord { letter: u8, shift: bool, alt: bool },`

```
/// A Ctrl chord the text editor doesn't own — Ctrl+S, Ctrl+O, … The
/// editor keeps Ctrl+A/C/X/V for text; everything else reaches the app
/// here. **Ctrl is implied**; `shift`/`alt` say what else was held, so
/// Ctrl+Shift+S is distinguishable from Ctrl+S.
///
/// `letter` is the lowercase ASCII letter, already normalized from the
/// control byte some keyboard paths produce (0x13 → 's').
///
/// Exists because `Event::Key` carries no modifiers: an app could not
/// otherwise tell Ctrl+S from a typed "s" — and while a text widget is
/// focused it never saw the keystroke at all.
```

## L741-748 · `Zoom { delta: i32 },`

```
/// Ctrl+wheel over the focused app — a zoom request. `delta` is
/// positive for "bigger" (wheel up), negative for smaller; its
/// magnitude is notches, not pixels, so the app picks the step.
///
/// Separate from `Wheel` because that one carries no modifiers, and
/// because Ctrl+wheel must NOT scroll: the compositor skips its own
/// scroll handling and sends this instead. An app that ignores it
/// simply doesn't zoom.
```

## L750-759 · `WheelX { dx: i32 },`

```
/// Waagrechtes Rollen ueber der fokussierten App, das kein
/// `Widget::Scroll` mit waagrechter Achse verbraucht hat. `dx` ist
/// pixelskaliert, positiv = nach RECHTS.
///
/// Eigene Variante und kein Feld an `Wheel`: ein angehaengter Wert
/// waere eine ABI-Aenderung an einer bestehenden Variante, und die
/// bricht jede App, die gegen das alte SDK gebaut ist. Angehaengt
/// scheitert bei ihnen nur das Dekodieren DIESES Ereignisses, und es
/// wird uebersprungen. MUSS im Gleichschritt mit der SDK-Kopie in
/// `tools/wasm/sdk/widgets/src/abi.rs` bleiben.
```

## L761-764 · `Slide { action: ActionId, value: u16, done: bool },`

```
/// A `Widget::Slider` moved. `action` is its `on_change`, `value` in
/// 0..=[`SLIDER_MAX`]. `done: false` while the pointer drags (only
/// when the value changed), `done: true` once on release — apps that
/// do expensive work (seeking) wait for that one.
```

## L766 · `}`

```
// Appended only.
```

## L769 · `#[repr(u8)]`

```
/// Which clipboard chord fired. See `Event::Clipboard`.
```

## L777 · `}`

```
// Appended only.
```

## L787 · `}`

```
// Appended only.
```

## L790 · `#[non_exhaustive]`

```
// ── Actions (app → compositor, via App trait return value) ────────────
```

## L792-794 · `#[non_exhaustive]`

```
/// Action returned by `App::handle(event) -> Action`. The SDK uses this
/// to decide whether to re-render; it does not cross the wire verbatim.
/// Declared here so the enum lives alongside its counterpart `Event`.
```

## L798 · `Idle,`

```
/// No state change — do not re-render.
```

## L800 · `Rerender,`

```
/// State changed — SDK calls render() and commits a new tree.
```

## L802 · `Exit,`

```
/// App wants to exit (window close).
```

## L804 · `}`

```
// Appended only.
```

## L807 · `#[derive(Clone, Copy, Debug)]`

```
// ── Rasterizer abstraction ────────────────────────────────────────────
```

## L809-811 · `#[derive(Clone, Copy, Debug)]`

```
/// Palette lookup — maps Token → concrete BGRA32 at raster time.
/// Defined here as a shape placeholder; the concrete `Palette` type
/// lives in `gui/color.rs` and is re-exported when the rasterizer lands.
```

## L814-815 · `pub colors: [u32; PALETTE_SLOTS],`

```
/// BGRA32 color per Token, indexed by `Token as u8`.
/// Slot 0 = Token::Surface, slot 16 = Token::AccentLine, …
```

## L819-820 · `pub const PALETTE_SLOTS: usize = 32;`

```
/// Slots in a `Palette`. Must stay > the highest `Token` discriminant —
/// the rasterizer indexes with `token as usize` and does not bounds-check.
```

## L823-833 · `pub struct I420Ref<'a> {`

```
/// A raster destination. Either a tile in the GGTT slab, or a composition
/// layer — from the rasterizer's perspective they are identical: a BGRA32
/// pixel buffer with an origin offset in window coordinates.
///
/// The rasterizer receives `Rect`s and `Point`s in **window** coordinates
/// and subtracts `origin` internally to get the target-local position.
/// Draws are clipped to `size`. This is what makes tile-boundary drawing
/// Just Work — the left tile clips the right half away, and vice versa.
/// A planar 4:2:0 frame, borrowed from the canvas store. Every field was
/// validated by `canvas::commit_i420`, which is what lets the blit index
/// the planes without re-deriving a single bound.
```

## L838 · `pub ys: usize,`

```
/// Row strides in bytes — a decoder pads rows, so these are not `w`.
```

## L845 · `pub pixels:  &'a mut [u32],`

```
/// Backing pixel buffer (BGRA32, packed u32 per pixel).
```

## L847 · `pub stride:  u32,`

```
/// Pixels per row (may exceed `size.w` for aligned allocations).
```

## L849 · `pub size:    Size,`

```
/// Target size in pixels.
```

## L851-853 · `pub origin:  Point,`

```
/// Target top-left in window coordinates.
/// Tiles:  `(tx * TILE_SIZE_PX, ty * TILE_SIZE_PX)`
/// Layers: node's layout rect top-left.
```

## L855 · `pub scale:   u8,`

```
/// HiDPI factor (1 or 2).
```

## L857 · `pub palette: &'a Palette,`

```
/// Active theme palette — Token → concrete BGRA.
```

## L859-863 · `pub bg_alpha: u8,`

```
/// Alpha (0..=255) applied to Background/Border fills. 255 = opaque
/// (normal scenes, unchanged). A panel scene cleared transparent sets
/// this to the chrome opacity so its pill backgrounds are translucent
/// while glyphs stay full-coverage — the compositor then composites the
/// scene over the wallpaper by per-pixel alpha (no halo).
```

## L865-866 · `pub window_id: u32,`

```
/// Owning widget window id — lets the render walker look up a
/// `Widget::Canvas`'s committed bitmap in the canvas store.
```

## L868-871 · `pub clip: Option<(i32, i32, i32, i32)>,`

```
/// Optional clip rectangle in **target-local** coords `(x0,y0,x1,y1)`.
/// `None` = clip only to the target size (default). A `Widget::Scroll`
/// sets this to its viewport rect for the duration of its subtree so
/// overflowing content is masked instead of bleeding past the panel.
```

## L875-880 · `pub trait Rasterizer: Send + Sync {`

```
/// Rasterizer backend. CPU in v1 (fontdue + gui/render.rs). GPU in v2+
/// (Intel Xe Render engine, SDF text atlas, fragment shaders for blur /
/// shadow / effect).
///
/// Non-negotiable: no call site in the widget pipeline references CPU or
/// GPU specifics. Switching backends = replacing `Box<dyn Rasterizer>`.
```

## L882 · `fn clear(&mut self, t: &mut RasterTarget, color: Token);`

```
/// Fill the entire target with a theme-token color.
```

## L885 · `fn rect(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill);`

```
/// Draw a filled rectangle. `r` is in window coordinates.
```

## L888 · `fn rect_rounded(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill, _radius: u8) {`

```
/// Fill a rounded rectangle. Default falls back to sharp rect.
```

## L893-894 · `fn stroke_rounded(&mut self, t: &mut RasterTarget, r: Rect, fill: Fill, width: u8, _radius: u8) {`

```
/// Stroke a rounded-rect outline `width`-thick. Default draws 4 sharp
/// rects ignoring radius.
```

## L905 · `fn text(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, color: Token, p: Point);`

```
/// Draw text at baseline point `p` (window coordinates) in `color`.
```

## L908-909 · `fn text_px(&mut self, t: &mut RasterTarget, s: &str, style: TextStyle, _size_px: u16,`

```
/// `text` with the style's pixel size overridden (`Modifier::FontSize`).
/// Default ignores the override so a backend can opt in.
```

## L915 · `fn icon(&mut self, t: &mut RasterTarget, id: IconId, size: u16, color: Token, p: Point);`

```
/// Draw an icon from the built-in atlas.
```

## L918 · `fn canvas_copy(&mut self, t: &mut RasterTarget, src: &[u8], w: u16, h: u16);`

```
/// Copy app-supplied Canvas pixels (BGRA32) into the target.
```

## L921-928 · `fn canvas_blit(&mut self, _t: &mut RasterTarget, _src: &[u8], _sw: u32, _sh: u32,`

```
/// Blit a BGRA32 bitmap (`sw`×`sh`) contain-fit into `rect` (window
/// coordinates): scaled to fit while preserving aspect, centred,
/// no background fill outside the fitted image. Default no-op.
/// Blit app-supplied BGRA pixels into `rect`, contain-fit and centred.
/// `zoom_q88` scales that fitted size (256 = 1.0× = plain fit, larger
/// crops to the rect); it comes from `Modifier::Scale` on the Canvas.
/// `pan` shifts the content from centred (`Modifier::CanvasOffset`)
/// and is clamped to the overhang, so the rect never shows a gap.
```

## L932-935 · `fn canvas_blit_i420(&mut self, _t: &mut RasterTarget, _p: &I420Ref, _sw: u32, _sh: u32,`

```
/// The same blit for a planar 4:2:0 frame, converting Y′CbCr to BGRA
/// on the way. Separate from `canvas_blit` rather than a format flag
/// on it because the two walk different memory: one plane of 4-byte
/// pixels against three planes at two resolutions. Default no-op.
```

## L939 · `fn blur(&mut self, _t: &mut RasterTarget, _r: Rect, _radius: u8) {}`

```
// ── Reserved (v2+, default no-op on CPU backend) ──────────────────
```

## L941 · `fn blur(&mut self, _t: &mut RasterTarget, _r: Rect, _radius: u8) {}`

```
/// Gaussian blur behind the given rect (acrylic/glass effect).
```

## L944 · `fn shadow(&mut self, _t: &mut RasterTarget, _r: Rect, _s: Shadow) {}`

```
/// Drop shadow under the given rect.
```

## L947 · `fn effect(&mut self, _t: &mut RasterTarget, _r: Rect, _id: EffectId) {}`

```
/// Named GPU effect.
```

