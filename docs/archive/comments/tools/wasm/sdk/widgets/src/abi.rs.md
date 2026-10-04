# `tools/wasm/sdk/widgets/src/abi.rs` @ 5e0102684

## L1-10 · `use alloc::boxed::Box;`

```
//! Widget ABI — wire contract mirror of `kernel/src/shade/widgets/abi.rs`.
//!
//! **Every change here must be mirrored to the kernel side, and vice
//! versa.** Variant order, struct-variant field order, and `#[repr]`
//! discriminants are all part of the wire format. Postcard serializes by
//! declaration position, so drift between the two copies would produce
//! silent deserialization corruption.
//!
//! The `check_abi` module at the crate root enforces ordering invariants
//! at compile time (same mechanism as the kernel's check_abi.rs).
```

## L17 · `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]`

```
// ── Geometry ──────────────────────────────────────────────────────────
```

## L39-44 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]`

```
/// A coloured run inside a `Widget::TextArea`'s `value`. `start`/`len`
/// are byte offsets into the (UTF-8) buffer; `token` is the colour. The
/// app recomputes spans on every edit (syntax highlighting); the
/// compositor renders the live buffer and colours each byte by the span
/// covering it (uncovered bytes use the default text colour). Spans
/// should be sorted by `start` and non-overlapping.
```

## L52 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]`

```
// ── Identifiers ───────────────────────────────────────────────────────
```

## L63 · `#[repr(u8)]`

```
// ── Theme tokens ──────────────────────────────────────────────────────
```

## L81 · `Page            = 12,`

```
/// Content canvas — below `Surface`. Editor body, page, terminal.
```

## L83 · `SurfaceHover    = 13,`

```
/// Hover fill / chips — above `SurfaceMuted`.
```

## L85 · `OnSurfaceFaint  = 14,`

```
/// Third text level: section headings, meta columns, disabled.
```

## L87 · `AccentRing      = 15,`

```
/// Accent at 22 % over `Surface` — focus rings.
```

## L89 · `CodeKeyword     = 17,`

```
/// Accent at 45 % over `Surface` — focused window border.
```

## L91-99 · `CodeKeyword     = 17,`

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

## L101 · `CodeControl     = 18,`

```
/// Control flow and imports: `if` `for` `return` `import` `match`.
```

## L103 · `CodeString      = 19,`

```
/// String and character literals, quotes included.
```

## L105 · `CodeComment     = 20,`

```
/// Comments, any syntax.
```

## L107 · `CodeNumber      = 21,`

```
/// Numeric literals.
```

## L109 · `CodeFunction    = 22,`

```
/// Function names — declaration and call site.
```

## L111 · `CodeType        = 23,`

```
/// Type / class names and markup tag names.
```

## L113 · `CodeVariable    = 24,`

```
/// Attribute names, JSON keys, decorators.
```

## L115 · `CodeConstant    = 25,`

```
/// Language constants (`true` `None` `null`) and escape sequences.
```

## L118 · `}`

```
// Appended only.
```

## L121 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]`

```
// ── Icons ─────────────────────────────────────────────────────────────
```

## L127-136 · `macro_rules! icon_ids {`

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

## L142 · `pub const ALL: &'static [(&'static str, IconId)] =`

```
/// Every named icon, for tests and tooling.
```

## L204 · `#[repr(u8)]`

```
// ── Accessibility roles ───────────────────────────────────────────────
```

## L221 · `}`

```
// Appended only.
```

## L224 · `#[repr(u8)]`

```
// ── Text style ────────────────────────────────────────────────────────
```

## L235-238 · `Heading = 5,`

```
/// 18 px regular weight — between `Body` (14) and `Title` (24, bold).
/// Used for non-bold display text such as input placeholders /
/// values where Body reads too small but Title's 600-weight bold
/// is too heavy. (Appended for vocab-v3.)
```

## L240 · `}`

```
// Appended only.
```

## L243 · `#[non_exhaustive]`

```
// ── Fill (rasterizer-side only) ───────────────────────────────────────
```

## L249 · `}`

```
// Appended only.
```

## L252 · `#[repr(u16)]`

```
// ── Effect IDs (reserved) ─────────────────────────────────────────────
```

## L259 · `}`

```
// Appended only.
```

## L262 · `#[repr(u8)]`

```
// ── Layout primitives ─────────────────────────────────────────────────
```

## L272 · `}`

```
// Appended only.
```

## L282 · `}`

```
// Appended only.
```

## L285 · `#[repr(u8)]`

```
// ── Container-query density ───────────────────────────────────────────
```

## L287-291 · `#[repr(u8)]`

```
/// Compositor-classified window size bucket. Apps reference these via
/// `Modifier::WhenDensity(Density, ...)` to adapt layout to the available
/// space without picking pixel breakpoints. The compositor owns the
/// thresholds (Compact <600 px, Regular 600–1200 px, Spacious >1200 px)
/// so apps never see raw pixel widths.
```

## L299 · `}`

```
// Appended only.
```

## L302 · `#[non_exhaustive]`

```
// ── Animation ─────────────────────────────────────────────────────────
```

## L309 · `}`

```
// Appended only.
```

## L319 · `#[non_exhaustive]`

```
// ── Modifier ──────────────────────────────────────────────────────────
```

## L324 · `Padding(u16),`

```
// Active in v1
```

## L337 · `Blur(u8),`

```
// Reserved (v2+) — CPU rasterizer treats as no-op.
```

## L343-345 · `Hover(Vec<Modifier>),`

```
// ── Vocab v2 (Tailwind-style modifiers) ───────────────────────────
// Pseudo-state modifier lists. Compositor merges the inner list onto
// the widget when the state matches; tree stays static across hovers.
```

## L350 · `WhenDensity(Density, Vec<Modifier>),`

```
/// Container query — apply inner modifiers only at the given density.
```

## L352 · `Scale(u16),`

```
/// Uniform scale, Q8.8 fixed-point. 256 = 1.0× (identity).
```

## L354-356 · `MinWidth(u16),`

```
/// Layout minimum width (px at 1× scale). Compositor honors as a hard
/// floor; if the parent allots less, the widget overflows visibly
/// rather than collapsing.
```

## L358 · `MaxWidth(u16),`

```
/// Layout maximum width (px at 1× scale).
```

## L360-361 · `Rounded(u8),`

```
/// Corner radius (px at 1× scale) without a Border. Use this when
/// rounding is needed without a stroked outline.
```

## L363-370 · `Flex(u8),`

```
/// CSS-style flex-grow on the main axis of the parent Row/Column.
/// The widget keeps its intrinsic main size as a basis and absorbs
/// a proportional share of the leftover space alongside any
/// `Spacer { flex }` siblings (Spacer = Flex with intrinsic 0 in
/// this scheme). Use case: a body Row that should fill the
/// remaining vertical space below the toolbar so its sidebar bg
/// reaches the footer divider, even when the grid content is
/// short. `Flex(0)` is identical to no Flex at all (intrinsic only).
```

## L372-378 · `NodeId(NodeId),`

```
/// Tag a widget with an app-chosen `NodeId`. The compositor's
/// layout pass records the laid-out rect of every NodeId-tagged
/// widget into a side table; `Widget::Popover { anchor }` then
/// looks the rect up to position itself relative to the anchor.
/// IDs are app-private — the compositor only echoes them back
/// internally for anchor lookups, never to other apps. Multiple
/// widgets with the same id is undefined behavior (last wins).
```

## L380-382 · `Ring { token: Token, width: u8 },`

```
/// Focus ring — a stroke of `width` px drawn just OUTSIDE the node's
/// rect, under any Border. Mirrors CSS `box-shadow: 0 0 0 Npx`. Costs
/// no layout space; leave room yourself (a row gap suffices at ≤ 3).
```

## L384 · `MinHeight(u16),`

```
/// Layout minimum height (px at 1× scale).
```

## L386 · `MaxHeight(u16),`

```
/// Layout maximum height (px at 1× scale).
```

## L388 · `LineNumbers(bool),`

```
/// Draw a line-number gutter down the left edge of a `TextArea`.
```

## L390-392 · `PaddingXY { x: u16, y: u16 },`

```
/// Per-axis inner padding (px at 1× scale). Sums with `Padding`.
/// Appended AFTER LineNumbers — inserting ahead of a shipped variant
/// renumbers it on the wire and breaks the running app.
```

## L394-395 · `Autofocus,`

```
/// This text widget takes focus when its window first appears.
/// Without it a window opens with nothing focused.
```

## L397-401 · `FontSize(u16),`

```
/// Font size override in px for a `Widget::Text` — replaces the size
/// its `TextStyle` resolves to; the face (proportional vs mono) still
/// comes from the style. Layout measures at the override, so the text
/// keeps a correct box. Clamped to 6..=64 by the compositor; ignored
/// on every other widget.
```

## L403-407 · `CanvasOffset { x: i32, y: i32 },`

```
/// Pan a `Widget::Canvas`'s content by this many px, relative to the
/// centred contain-fit position. Pairs with `Modifier::Scale`: scale
/// decides how big the image is drawn, this decides which part of it
/// the rect shows. The compositor clamps it to the overhang, so it can
/// never push the content out of view. Ignored on every other widget.
```

## L409-422 · `Spans(alloc::vec::Vec<Span>),`

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

## L424-430 · `OnMotion(ActionId),`

```
/// Fire `Event::Action(id)` while the pointer MOVES over this widget,
/// at most every [`MOTION_INTERVAL_MS`] — and at once when the pointer
/// moves onto it from another `OnMotion` target (deepest wins, like
/// `OnClick`). For "show the controls while the mouse is being used":
/// apps never get raw pointer motion, and `OnHover` fires only on
/// entering, so a pointer that rests and then moves again inside the
/// same widget would go unnoticed.
```

## L432 · `}`

```
// Appended only.
```

## L435-436 · `pub const FONT_SIZE_MIN: u16 = 6;`

```
/// Accepted range for `Modifier::FontSize`; the compositor clamps to it.
/// Mirror of the kernel copy.
```

## L440 · `pub const MONO_SIZE_PX: u16 = 13;`

```
/// Default size of `TextStyle::Mono` — the editor's starting point.
```

## L443 · `pub const MOTION_INTERVAL_MS: u32 = 200;`

```
// ── Widget ────────────────────────────────────────────────────────────
```

## L445-447 · `pub const MOTION_INTERVAL_MS: u32 = 200;`

```
/// Upper end of `Widget::Slider::value` — per mille, fine enough for a
/// seek bar across a full-width window.
/// Throttle of `Modifier::OnMotion`.
```

## L455 · `Column {`

```
// Containers
```

## L478 · `Text {`

```
// Leaves
```

## L517-524 · `Popover {`

```
/// Floating overlay anchored to a `Modifier::NodeId`-tagged
/// widget elsewhere in the tree. Renders on top of everything
/// (z-order) at `(anchor.x, anchor.y + anchor.h)` — flips above
/// the anchor when there is no room below. Apps emit a Popover
/// only while the overlay should be visible; toggle by adding /
/// removing it from the tree. `on_dismiss` fires whenever the
/// user clicks outside both the popover content AND the anchor
/// rect — apps route this to their "close" state transition.
```

## L540-550 · `TextArea {`

```
/// Multi-line text editor. Unlike `Input` (single line, on_submit on
/// Enter), the compositor owns a 2-D caret: arrows move within / across
/// lines, Enter inserts a newline, Home/End are line-relative,
/// PageUp/PageDown scroll by a viewport. `value` is the whole document
/// (`\n`-separated). Buffer mutations emit `Event::InputChange { value }`
/// (the entire document) exactly like `Input`; only one widget is ever
/// focused, so there is no ambiguity. There is intentionally no
/// `on_submit` — Enter is a newline, not a submit. Rendered with
/// `TextStyle::Mono`; the visible window scrolls to keep the caret in
/// view. Apps typically wrap it in `Modifier::Flex(1)` so it fills the
/// space between toolbar and footer.
```

## L554-555 · `spans:       Vec<Span>,`

```
/// Syntax-highlight colour runs over `value` (byte offsets). The
/// app recomputes these on every edit; empty = plain text.
```

## L559-566 · `Slider {`

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

## L572 · `}`

```
// Appended only.
```

## L575 · `#[non_exhaustive]`

```
// ── Events / Actions ──────────────────────────────────────────────────
```

## L577-578 · `#[non_exhaustive]`

```
/// Mirror of `kernel::input::KeyCode`. Field shape frozen as part of the
/// Phase 8 ABI — kernel-side and SDK-side must stay in sync.
```

## L598 · `}`

```
// Appended only.
```

## L601-603 · `#[non_exhaustive]`

```
/// Note: `InputChange` carries an owned `String`, so this enum is
/// `Clone`-only — not `Copy`. Apps match `Event` by value (move) or
/// clone explicitly when keeping it across iterations.
```

## L617-622 · `InputChange { value: String },`

```
/// The focused `Widget::Input`'s value was mutated by the
/// compositor (printable key, Backspace, Delete). `value` is the
/// new buffer contents — apps typically mirror it into their state
/// and re-commit the tree with `Widget::Input { value, ... }`
/// matching. Cursor-only navigation (Left/Right/Home/End) does
/// not fire this event.
```

## L624-626 · `ContextAction(ActionId),`

```
/// Right-click hit-test result. Same hit-test as `Action`, but
/// fired for `MouseButton::Right`. Apps use it to open context
/// menus (Popover) without consuming the primary click.
```

## L628-631 · `Open(String),`

```
/// "Open this resource" — delivered when `npk_open` targets an app
/// that is ALREADY running (instead of spawning a duplicate). The
/// payload is the launch argument (e.g. a file path). Lets an app
/// be a singleton with tabs: a second open routes here as a new tab.
```

## L633-636 · `Wheel { dy: i32 },`

```
/// Mouse wheel over a focused app that has no `Widget::Scroll` to consume
/// it. `dy` is already scaled to pixels (positive = scroll down). Apps
/// that render their own surface (e.g. the browser's Canvas) scroll their
/// own viewport in response; apps that ignore it are unaffected.
```

## L638-644 · `Clipboard(ClipKind),`

```
/// A clipboard chord (Ctrl+C / Ctrl+X / Ctrl+V) reached the focused
/// app because no focused text widget consumed it. Apps that manage
/// their own selection (e.g. loft's file grid) act on it — copy/cut
/// the selection, or paste into the current context. Apps that don't
/// care ignore it; the variant is append-only, so an app built
/// against an older SDK simply fails to decode it and skips (its
/// `postcard::from_bytes` returns `Err`, treated as "no event").
```

## L646-651 · `Picked { path: String, tag: u32 },`

```
/// A file-picker request this app started via `npk_pick` finished.
/// `path` is the npkFS path the user chose, or **empty if they
/// cancelled**. `tag` is the caller's own value from `npk_pick`,
/// returned unchanged — the picker roundtrip is asynchronous, so an
/// app running several dialogs (open / save-as / …) uses it to tell
/// which one came back. The kernel never interprets it.
```

## L653-660 · `CloseRequest,`

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

## L662-672 · `Chord { letter: u8, shift: bool, alt: bool },`

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

## L674-681 · `Zoom { delta: i32 },`

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

## L683-692 · `WheelX { dx: i32 },`

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

## L694-697 · `Slide { action: ActionId, value: u16, done: bool },`

```
/// A `Widget::Slider` moved. `action` is its `on_change`, `value` in
/// 0..=[`SLIDER_MAX`]. `done: false` while the pointer drags (only
/// when the value changed), `done: true` once on release — apps that
/// do expensive work (seeking) wait for that one.
```

## L699 · `}`

```
// Appended only.
```

## L702 · `#[repr(u8)]`

```
/// Which clipboard chord fired. See `Event::Clipboard`.
```

## L710 · `}`

```
// Appended only.
```

## L720 · `}`

```
// Appended only.
```

## L729 · `}`

```
// Appended only.
```

## L732 · `#[derive(Clone, Copy, Debug, Serialize, Deserialize)]`

```
// ── Palette (for app-side token → color query via npk_theme_token) ────
```

## L734-735 · `#[derive(Clone, Copy, Debug, Serialize, Deserialize)]`

```
/// Received by the app if it queries the active palette. The concrete
/// RGBA values are compositor-resolved; the app never picks hex colors.
```

## L741 · `pub const PALETTE_SLOTS: usize = 32;`

```
/// Slots in a `Palette` — must stay > the highest `Token` discriminant.
```

