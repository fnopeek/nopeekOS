# `nopeek_widgets`: Vocabulary Reference

Single-file reference for app developers and AI code-generators. Build
GUI apps for nopeekOS from the prefabs and modifiers below; never reach
for raw pixels, fonts, or RGB values.

The vocabulary mirrors Tailwind/shadcn at the conceptual level: a small
spacing/radius scale, a tokenized colour palette, pseudo-state
modifiers (`Hover`/`Focus`/`Active`/`Disabled`), container queries via
`WhenDensity`. AI familiar with that pattern will produce idiomatic
nopeekOS UI on the first try.

All lengths in a tree are logical px. The compositor multiplies them by
the UI scale (`shade.scale`, 1.0 to 3.0) on the way in.

---

## App lifecycle

An app is a `no_std` `cdylib` WASM module that depends on
`nopeek_widgets` (and on it again as a build dependency, for the
metadata). The pattern below is what `tools/wasm/volume` and
`tools/wasm/link` do.

```rust
#![no_std]
extern crate alloc;

use nopeek_widgets::{events, host, prefab, wire, *};

// Launcher metadata (name, description, icon). `build.rs` writes it with
// `nopeek_widgets::app_meta::encode(&AppMeta { .. })` into OUT_DIR.
#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// The rights this app is granted. Without the section: read + exec + render.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::RENDER];

// A bump heap reset before every frame (or the `heap` feature for a
// growing one).
#[global_allocator]
static ALLOCATOR: nopeek_widgets::bump::Bump<{ 256 * 1024 }> =
    nopeek_widgets::bump::Bump::new();

nopeek_widgets::panic_handler!("[myapp]");

const WAIT_INPUT: u32 = 1;   // an event for this app
const WAIT_STATE: u32 = 32;  // a system state change (volume, network, ...)

enum Outcome { Idle, Rerender, Exit }   // the app's own type

fn commit(st: &MyApp) {
    ALLOCATOR.reset(st.heap_base);           // the previous tree is dead
    if let Ok(bytes) = wire::encode(&render(st)) {
        host::scene_commit(&bytes);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut st = MyApp::load();
    st.heap_base = ALLOCATOR.mark();
    commit(&st);                              // first commit opens the window
    let mut buf = [0u8; 256];
    loop {
        let mut redraw = false;
        loop {
            match events::poll(&mut buf) {
                events::Poll::Event(ev) => match handle(&mut st, ev) {
                    Outcome::Idle     => {}
                    Outcome::Rerender => redraw = true,
                    Outcome::Exit     => { host::close_widget(); return; }
                },
                events::Poll::Empty => break,
                events::Poll::Gone  => return,   // window was closed
            }
        }
        if redraw { commit(&st); }
        host::wait(WAIT_INPUT | WAIT_STATE, -1);  // park; -1 = no timeout
    }
}
```

The app builds a fresh `Widget` tree for every commit. `wire::encode`
prefixes the postcard body with `WIRE_VERSION` (`0x01`); the compositor
refuses another version, a payload over 16 MiB, or a tree nested deeper
than 128 levels. Hover, focus, the text caret and selection, scroll
position and a slider drag are compositor state: they change without a
commit.

`events::poll` also reads an event larger than `buf` (it allocates a
buffer of the needed size for that one read). `host` wraps the host
calls with `Option`/`bool` results; `npk_sys` holds the raw FFI.

---

## Layout containers

| Widget                                   | Use                                              |
|------------------------------------------|--------------------------------------------------|
| `Column { children, spacing, align, modifiers }` | Stack children vertically, `spacing` px apart |
| `Row { children, spacing, align, modifiers }`    | Same, horizontal                         |
| `Stack { children, modifiers }`          | Z-order overlay; every child gets the same rect  |
| `Scroll { child, axis, modifiers }`      | Clip + scroll a child along `axis` (`Vertical`, `Horizontal`, `Both`) |

`Spacer { flex }` inside a `Row`/`Column` eats remaining main-axis
space; flex weights distribute pro-rata (`flex: 0` counts as 1).

`Align::Start | Center | End | Stretch` controls cross-axis placement.

A `Scroll` reports only a 24 px floor on its scroll axis, so a flex
parent sizes it to the viewport. A vertical `Scroll` draws an overlay
scrollbar when its content overflows. A window has one scroll position
per axis, shared by its scrollers; use one vertical `Scroll` (or
`TextArea`) per window. Wheel input that no scroller consumes reaches
the app as `Event::Wheel` / `Event::WheelX`.

### Overlays

| Widget                                        | Use                                   |
|-----------------------------------------------|---------------------------------------|
| `Popover { anchor, child, on_dismiss, modifiers }` | Floating `child` drawn above everything, below the widget tagged `Modifier::NodeId(anchor)`; flips above it when there is no room below. |
| `Tooltip { text, anchor, modifiers }`         | Wire slot only: lays out as zero size, draws nothing. |
| `Menu { items, modifiers }`                   | Wire slot only: lays out as zero size, draws nothing. |

A popover is shown while it is in the tree; remove it to close it. A
click outside the popover and outside its anchor fires `on_dismiss`
(the last-declared popover's); a click on the anchor goes to the
anchor's own `OnClick`. `NodeId::POINTER` anchors it at the window's
last right click (context menus). A popover whose anchor is not in the
tree is dropped. Its own `modifiers` are ignored; style the `child`.

---

## Leaves

| Widget                                              | Notes |
|-----------------------------------------------------|-------|
| `Text { content, style, modifiers }`                | One line in `style: TextStyle`. Cut with "…" when wider than its slot. |
| `Icon { id, size, modifiers }`                      | `id: IconId`, `size` in px (square). |
| `Button { label, icon, on_click, modifiers }`       | Label in `Body` plus an optional 16 px icon (`IconId::None` = none). Fills `Accent` unless a `Background` is given. Fixed inner padding (8 px horizontal, 4 px vertical); `Padding` modifiers do not change it. |
| `Input { value, placeholder, on_submit, modifiers }`| Single-line text field, edited by the compositor (see below). |
| `TextArea { value, placeholder, spans, modifiers }` | Multi-line editor in `Mono`, edited by the compositor. `spans` colour byte ranges of `value` (syntax highlighting). |
| `Checkbox { value, on_toggle, modifiers }`          | 16 px box, `Accent` fill when `value`. A click sends `Action(on_toggle)`; the app flips `value`. |
| `Slider { value, on_change, modifiers }`            | Horizontal track with a draggable thumb; `value` in `0..=SLIDER_MAX` (1000). At least 80 px wide; meant to `Flex`/stretch. |
| `Spacer { flex }`                                   | Flexible empty space (see above). |
| `Divider`                                           | Horizontal 1 px line in the `Border` token across the parent's width. For a vertical line use `prefab::mark`. |
| `Canvas { id, width, height, modifiers }`           | App-supplied pixels. `width`/`height` are its intrinsic size; it fills the rect it is given. |

**Text editing.** A focused `Input` or `TextArea` is edited by the
compositor: printable keys, Backspace, Delete, arrows, Home/End, mouse
selection (double click selects a word) and Ctrl+A/C/X/V. Every change
to the buffer sends `Event::InputChange { value }` with the whole text,
plus `Event::InputChangeAt { node, value }` when the widget carries
`Modifier::NodeId`. The app mirrors `value` into its state and commits
it back; caret moves send nothing. In an `Input`, Enter fires
`Event::Action(on_submit)`; with `on_submit = NO_ACTION` Enter reaches
the app as `Event::Key(KeyCode::Enter)`. Up/Down are not consumed. In a
`TextArea`, Enter inserts a newline, Tab indents, Home/End are
line-relative and Up/Down/PageUp/PageDown move the caret. An `Input` is
sized by its placeholder (at least 120 px wide), never by its value; a
longer value scrolls inside the field. Its text is drawn in `Body`.

**Canvas pixels.** Upload with `host::canvas_commit(id, bgra, w, h)`
(`npk_canvas_commit`, needs `caps::CANVAS`) or as planar 4:2:0 with
`npk_canvas_commit_yuv`. The bitmap is contain-fit and centred in the
rect; `SurfaceMuted` shows until the first upload. `host::canvas_rect(id)`
returns where the canvas was laid out, so the app can paint at 1:1 and
map clicks.

---

## Modifiers: the styling vocabulary

Every widget except `Spacer` and `Divider` carries `modifiers: Vec<Modifier>`.

### Paint

| Modifier                            | Effect                                          |
|-------------------------------------|-------------------------------------------------|
| `Background(Token)`                 | Filled rect under the widget.                   |
| `Border { token, width, radius }`   | Stroked rounded rect on top of the background.  |
| `Rounded(u8)`                       | Corner radius without a stroke. Wins over `Border`'s radius. |
| `Ring { token, width }`             | Focus ring: a stroke of `width` px just outside the rect. Takes no layout space; leave room (a row gap suffices at width 3 or less). |
| `Opacity(u8)`                       | After painting, blends the node's rect towards the `Surface` colour by `255 - value`. The first `Opacity` in the list wins. |
| `Tint(Token)`                       | Colour of `Text`, `Icon` and the `Button` label and icon, for this node and its whole subtree (a descendant's own `Tint` wins). On a `TextArea` it sets the default text colour of that widget only. `Input` text ignores it. |
| `FontSize(u16)`                     | Pixel size of a `Text` or `TextArea`, overriding its style's size (the face still comes from the style). Clamped to `FONT_SIZE_MIN..=FONT_SIZE_MAX` (6..=64). |
| `Scale(u16)`                        | Q8.8 factor, 256 = 1.0. On an `Icon`, or on any ancestor of one (nearest wins), scales the glyph around its centre without changing layout. On a `Canvas`, zooms the blit relative to contain-fit. No effect on anything else. |

Defaults: `Text` is `OnSurface` (`Muted` style: `OnSurfaceMuted`),
`Icon` and `Button` labels `OnSurface`.

### Layout

| Modifier                  | Effect                                                  |
|---------------------------|---------------------------------------------------------|
| `Padding(u16)`            | Inner padding on all sides. Use `Padding::Md.as_u16()`. No effect on `Button` and `Scroll`. |
| `PaddingXY { x, y }`      | Per-axis inner padding. Sums with `Padding`.            |
| `MinWidth(u16)` / `MaxWidth(u16)`   | Clamp the measured width.                     |
| `MinHeight(u16)` / `MaxHeight(u16)` | Clamp the measured height.                    |
| `Flex(u8)`                | Flex-grow on the parent `Row`/`Column` main axis. The widget keeps its intrinsic size as a basis and takes a share of the leftover space alongside `Spacer { flex }` siblings; on overflow it shrinks in proportion to weight times basis. `Flex(0)` = no flex. |
| `NodeId(NodeId)`          | Tags the widget with an app-chosen id: anchor for `Popover`, source id in `Event::InputChangeAt`. Duplicate ids: the last one wins. |

### Widget-specific

| Modifier                  | Applies to | Effect |
|---------------------------|------------|--------|
| `Autofocus`               | `Input`, `TextArea` | Takes focus when the window first appears. Without it a window opens with nothing focused. |
| `FocusRequest(u32)`       | `Input`, `TextArea` | Focus it once, whole text selected, whenever the number differs from the last one honoured in this window. Re-committing the same number does nothing; 0 never requests. |
| `Mask`                    | `Input`     | Draw one bullet per character (password entry). The real text still arrives in `InputChange`. |
| `Spans(Vec<Span>)`        | `Input`     | Colour byte ranges of the text (`Span { start, len, token }`); uncovered bytes keep the default colour, the later span wins on overlap. Ignored together with `Mask`. |
| `LineNumbers(bool)`       | `TextArea`  | Line-number gutter at the left edge. |
| `CanvasOffset { x, y }`   | `Canvas`    | Pan the content from the centred position, clamped so it never leaves the rect. |
| `CanvasNative`            | `Canvas`    | Draw the bitmap 1:1 at the rect's top-left, extending its last row and column over any uncovered part (a stale frame during resize). `Scale` and `CanvasOffset` are ignored while set. |

### Interactive states

| Modifier                                | Trigger                                                  |
|-----------------------------------------|----------------------------------------------------------|
| `Hover(Vec<Modifier>)`                  | Cursor over this node *or any descendant* (CSS `:hover`).|
| `Focus(Vec<Modifier>)`                  | Focus is on this node or a descendant (CSS `:focus-within`). |
| `Active(Vec<Modifier>)`                 | Mouse button held on this node or a descendant.          |
| `Disabled(Vec<Modifier>)`               | Presence-based: this widget is disabled. Wins over Hover/Focus/Active. |
| `WhenDensity(Density, Vec<Modifier>)`   | The window's density class matches (see Density below).  |

State mods carry a nested `Vec<Modifier>` that applies only while the
state holds. The tree stays the same across state changes; the
compositor merges at render time.

The merge is paint-only. Layout reads a widget's base list, so a
`Padding`, `Min*`/`Max*`, `Flex` or `FontSize` inside a state or
density wrapper does not change its size. Use paint modifiers there
(`Background`, `Border`, `Rounded`, `Ring`, `Tint`, `Scale`).

### Click / hover routing

| Modifier               | Effect                                                      |
|------------------------|-------------------------------------------------------------|
| `OnClick(ActionId)`    | Left click fires `Event::Action(id)`, right click `Event::ContextAction(id)`. The deepest target under the pointer wins; `Button.on_click` counts as one. |
| `OnHover(ActionId)`    | Fires `Event::Action(id)` when the hover target (deepest `OnHover` under the pointer) changes to this widget. |
| `OnMotion(ActionId)`   | Fires `Event::Action(id)` while the pointer moves over this widget, at most every `MOTION_INTERVAL_MS` (200 ms), and at once when it moves onto it from another `OnMotion` target. |

### Wire-frozen, ignored by the renderer

`Margin`, `Transition`, `Blur`, `Shadow`, `Effect`, `RoleOverride`.
They serialize and round-trip, and the compositor accepts them, but
neither layout nor the rasterizer reads them. Do not rely on any effect.

---

## Tokens

### `Token` (palette indices)

```
Surface | SurfaceElevated | SurfaceMuted | SurfaceHover | Page
OnSurface | OnSurfaceMuted | OnSurfaceFaint | OnAccent
Accent | AccentMuted | AccentRing | AccentLine
Border | Success | Warning | Danger
CodeKeyword | CodeControl | CodeString | CodeComment | CodeNumber
CodeFunction | CodeType | CodeVariable | CodeConstant
```

| Token            | Use |
|------------------|-----|
| `Page`           | Content canvas below `Surface`: editor body, page, terminal. |
| `SurfaceHover`   | Hover fill, chips; above `SurfaceMuted`. |
| `OnSurfaceFaint` | Third text level: section headings, meta columns, disabled. |
| `AccentRing`     | Accent at 22 % over `Surface`: focus rings. |
| `AccentLine`     | Accent at 45 % over `Surface`: focused window border. |
| `Code*`          | Syntax colours, a ramp of their own: resolved from `set code.scheme`, never from the accent. |

The compositor resolves tokens against the active palette
(`theme dark|light|auto` plus the wallpaper-derived accent). Apps never
specify hex values.

### Spacing scale (`u16` logical px)

| Token         | px   | Token         | px   |
|---------------|-----:|---------------|-----:|
| `Spacing::None` | 0  | `Spacing::Lg`   | 16   |
| `Spacing::Xxs`  | 2  | `Spacing::Xl`   | 24   |
| `Spacing::Xs`   | 4  | `Spacing::Xxl`  | 32   |
| `Spacing::Sm`   | 8  |               |      |
| `Spacing::Md`   | 12 |               |      |

Same scale for `Padding::*` (omits `Xxl`). Both convert with `.as_u16()`.

### Radius scale (`u8`, `.as_u8()`)

```
Radius::None=0  Sm=4  Md=8  Lg=12  Xl=16  Pill=255
```

### Elevation

```
Elevation::Flat | Subtle | Raised | Floating
```

SDK-only enum with no wire form; nothing renders it.

### Motion (transition duration token)

```
Motion::Instant=0  Quick=120  Normal=200  Slow=400  (ms, .as_ms())
```

`Motion::Quick.as_transition()` builds `Transition::Linear { ms: 120 }`
for `Modifier::Transition(...)`, which the compositor ignores (see
above), so nothing animates.

### Density (compositor-classified, logical window width)

```
Density::Compact   < 600 px
Density::Regular   600..1200
Density::Spacious  >= 1200
```

### `IconId`

`IconId(u16)` is a number with named constants, not a closed enum:

```
None Folder File ArrowLeft ArrowRight ArrowUp ArrowDown Home Download
MagnifyingGlass X Check Gear Power Lock Terminal Trash DotsThreeVertical
List Monitor FileText FolderOpen Image HardDrives Code Folders CaretRight
ArrowClockwise Globe Camera BatteryEmpty BatteryLow BatteryMedium
BatteryHigh BatteryFull BatteryCharging BatteryWarning Plug SpeakerHigh
SpeakerLow SpeakerX Minus Plus Bird MusicNotes Play Pause SkipBack
SkipForward FileAudio ArrowCounterClockwise PlayCircle Fire SquaresFour
SlidersHorizontal WifiHigh WifiMedium WifiLow WifiNone WifiSlash Network
```

(`IconId::None` = 0 through `IconId::Network` = 60; `IconId::ALL` lists
them.) The Phosphor atlas carries vector outlines, so an icon is filled
at exactly the requested size; the 16/24/32/48/64 px bitmaps are only
the fallback. `IconId::None` draws nothing; a number the atlas lacks
draws a filled placeholder square.

### `TextStyle`

| Style     | Size (px) | Face          | Default colour   |
|-----------|----------:|---------------|------------------|
| `Title`   | 22        | Inter         | `OnSurface`      |
| `Heading` | 15        | Inter         | `OnSurface`      |
| `Body`    | 13        | Inter         | `OnSurface`      |
| `Muted`   | 13        | Inter         | `OnSurfaceMuted` |
| `Caption` | 10        | Inter         | `OnSurface`      |
| `Mono`    | 13        | IBM Plex Mono | `OnSurface`      |

Inter Variable is rendered at its default weight, so styles differ by
size and colour, not weight. `MONO_SIZE_PX` (13) is the `Mono` default.

---

## Prefabs: the cookbook (prefer over raw widgets)

Prefabs encode the design system. App code should compose these and
only reach for raw `Widget`/`Modifier` when no prefab fits. All live in
`nopeek_widgets::prefab`.

### Containers / surfaces

| Prefab                   | Use                                                         |
|--------------------------|-------------------------------------------------------------|
| `panel(children)`        | Root `Column`: Md spacing, Stretch, Padding Xs.             |
| `card(content, CardKind)`| Padding Lg, Rounded Lg. `Inset`: `Surface` + 1 px `Border`. `Panel` and `Sheet`: `SurfaceElevated`, no border (identical). |
| `sidebar_pane(sections)` | `SurfaceMuted` column with Padding Sm; a trailing flex `Spacer` is appended. |
| `dialog(title, body, footer_hint: Option<&str>, min_w)` | `SurfaceElevated`, Rounded Lg, Padding Lg, `MinWidth(min_w)`: Title, Divider, body, then Divider + Caption hint if given. |
| `center_box(child, modifiers)` | Row that centres one child with flex spacers; for fixed-size cells (`MinWidth` alone leaves the child at the leading edge). |
| `mark(w, h, Option<Token>)` | Solid rectangle of exact size (selection edge, vertical separator). `None` keeps the space but draws nothing. |

### Form / input

| Prefab                                            | Use                                  |
|---------------------------------------------------|--------------------------------------|
| `button(label, ButtonStyle, on_click: ActionId)`  | `Primary` (`Accent`, `OnAccent` label) / `Secondary` (`SurfaceElevated`) / `Ghost` (`Surface`) / `Destructive` (`Danger`, `OnAccent` label). Rounded Md, Hover/Active backgrounds, Focus = 2 px `Accent` border. |
| `input(value, placeholder, InputKind, on_submit, trailing: Option<Widget>)` | Text field in a Padding Md, Rounded Md row with a 1 px `Accent` border on focus, no background. `Search` prepends an `Accent` `MagnifyingGlass`; `Password` adds `Mask`; `Text` is plain. `trailing` is right-aligned. `NO_ACTION` opts out of submit. |
| `input_autofocus(...)`                            | Same arguments; the field takes focus when the window appears. |

### Lists / navigation

| Prefab                                                   | Use                                              |
|----------------------------------------------------------|--------------------------------------------------|
| `list_row(icon, title, subtitle, selected, on_click: Option<ActionId>, on_hover: Option<ActionId>)` | Min height 34, padding Xs: 2 px leading edge, 24 px icon, Body title over Muted subtitle. Selected: `AccentMuted` fill, `Accent` edge and icon, trailing arrow. Otherwise Hover `SurfaceHover`, Focus `Ring { AccentRing, 2 }`. |
| `nav_row(icon, label, selected, on_click, on_hover)`     | 30 px sidebar item, 16 px icon. Selected: `AccentMuted` fill + `Accent` tint. Otherwise Hover `SurfaceHover`, Focus `AccentRing` ring. |
| `sidebar_section(label, items)`                          | `Mono` label tinted `OnSurfaceFaint` above a group of `nav_row`s. |
| `breadcrumb(&[(String, ActionId)])`                      | `Mono` path segments joined by carets; the last is a `SurfaceHover` chip, the others are muted with a hover fill. |
| `scroll_list(items)`                                     | Vertical `Scroll` around a Stretch `Column` (spacing Xxs). |
| `grid_item(icon, label, selected, on_click, on_hover)`   | 64 px icon over a Body label, radius Md. Selected: `AccentMuted` fill, `Accent` icon. Otherwise Hover `SurfaceHover`, Focus `Ring { AccentRing, 2 }`. |
| `grid(items, per_row)`                                   | Wrap a flat list into rows of `per_row` cells.   |
| `popover_menu(&[(String, ActionId)], selected: Option<usize>)` | Popover content: `SurfaceElevated` column with a 1 px `Border`, at least 180 px wide; the selected row gets a leading `Check`. |
| `popover_menu_shortcuts(items, hints: &[&str], selected)` | Same with a right-hand Caption column ("Ctrl+S"), indexed alongside `items`. |

### Chrome

| Prefab                                  | Use                                     |
|-----------------------------------------|-----------------------------------------|
| `toolbar(children)`                     | Centred row, Sm spacing, Padding Sm.    |
| `menu_bar(&[(String, ActionId)])`       | 36 px `SurfaceElevated` strip of clickable Body labels. |
| `menu_bar_with_anchors(labels, &[NodeId])` | Same; tags label `i` with `anchors[i]` so a `Popover` can drop from it. |
| `menu_bar_with_icon(icon, labels, anchors)` | Same with the app's glyph at the leading edge (`IconId::None` = none). |
| `icon_button(icon, size, on_click: Option<ActionId>, on_hover: Option<ActionId>)` | Fixed 28 px square around a centred icon: Hover `SurfaceHover`, Active `AccentMuted` + `Accent` tint, Focus `AccentRing` ring. |
| `footer(left, right)`                   | Bottom row with two Muted texts, Padding Md. |
| `title_bar(title)`                      | Title-styled text with Padding Sm.      |
| `badge(text)` / `text_badge(String)`    | Caption chip, Padding Xs, `SurfaceMuted`. |
| `body(text)` / `muted(text)`            | Plain Body / Muted text.                |
| `empty_state(text)`                     | Muted message with Padding Lg, for empty lists. |

### Constants

`prefab::NO_ACTION`: `ActionId(u32::MAX)`, the sentinel for "no submit /
no callback". Apps must not use this id for their own actions.

---

## Events

`events::poll` yields `Event`. Variants an app does not handle can be
ignored; a variant newer than the app's SDK fails to decode and is
skipped.

| Event | When |
|-------|------|
| `Key(KeyCode)` | A key no focused text widget consumed. Carries no modifiers. `KeyCode`: `Char(u8)`, `Enter`, `Backspace`, `Tab`, `Escape`, `Delete`, `Insert`, `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `F(u8)`. |
| `Action(ActionId)` | `OnClick`, `Button.on_click`, `OnHover`, `OnMotion`, an `Input`'s `on_submit`, a `Popover`'s `on_dismiss`. |
| `ContextAction(ActionId)` | Right click on an `OnClick` target or `Button`. |
| `MouseButton { button, down, x, y }` | Every press and release (`Left`, `Right`, `Middle`) in the window, in screen coordinates, in addition to any `Action`. The middle button never fires an `Action`. |
| `MouseMove { x, y }` | Pointer motion while the left button is held (a drag), to the focused window, in screen coordinates. Plain hover is never reported. |
| `Wheel { dy }` / `WheelX { dx }` | Scrolling, in px, that no `Scroll`/`TextArea` consumed. |
| `Zoom { delta }` | Ctrl+wheel, in notches; positive = bigger. |
| `Chord { letter, shift, alt }` | A Ctrl+letter chord the text editor does not own (Ctrl+S, ...); `letter` is lowercase ASCII. |
| `InputChange { value }` | The focused `Input`/`TextArea` changed; `value` is the whole text. |
| `InputChangeAt { node, value }` | Same, additionally, when that widget carries `Modifier::NodeId(node)`. |
| `Slide { action, value, done }` | A `Slider` moved: `done: false` while dragging (only on change), once `done: true` on release. |
| `Clipboard(ClipKind)` | Ctrl+C/X/V (`Copy`/`Cut`/`Paste`) no text widget consumed; only after `npk_window_set_clipboard_sink`. |
| `Open(String)` | `npk_open` targeted this already running app; the payload is the launch argument. |
| `Picked { path, tag }` | A `host::pick` file dialog finished; empty `path` = cancelled, `tag` as passed. |
| `CloseRequest` | The user asked to close the window after `npk_window_set_close_guard(1)`. Call `host::close_widget()` to go; a second close gesture or a few seconds of silence close it anyway. |
| `PopupDone { popup }` | The compositor closed popup `popup` opened with `npk_popup`. |
| `Focus(bool)` | The app's window (or one of its popups) got (`true`) or lost (`false`) the keyboard. Moving between the window and its own popups sends nothing. |

---

## Pseudo-state semantics

CSS-style `:hover` / `:focus` / `:active` apply to a node **and all of
its ancestors**. So `Modifier::Hover(...)` on a Row triggers when the
cursor is over a descendant Icon: the whole Row gets the hover style.

Focus lands on a widget by Tab, by `Autofocus` / `FocusRequest`, or by
a click on an `Input` or `TextArea` (or on a row whose only text widget
is one, like a bordered field). A click anywhere else clears focus.
`Active` follows the deepest focusable widget under a press until the
button is released.

`Modifier::Disabled(...)` is presence-based: if a widget has it, the
widget IS disabled and renders with the inner mods applied. Disabled
subtrees swallow clicks, hover and motion actions for the whole
subtree, and are skipped by focus.

Merge order (later wins on conflicting mods like `Background`):

1. Base modifiers
2. `WhenDensity` matches
3. `Disabled` (if present; then 4 to 6 are skipped)
4. `Hover`
5. `Focus`
6. `Active`

---

## Tab navigation

The compositor intercepts Tab / Shift-Tab on focused widget windows
and walks focus through every focusable widget in document order,
wrapping at the end. "Focusable" = `Button` / `Input` / `TextArea` /
`Checkbox`, or any widget with `OnClick`. Disabled subtrees are
skipped. A focused `TextArea` takes Tab itself (indent).

If focus cannot move (no focusable widget, or only the one already
focused), Tab reaches the app as `Event::Key(KeyCode::Tab)`. Focus is
visual: Enter or Space on a focused button is not turned into its
action, it reaches the app as `Event::Key`.

---

## What is not supported

- **Animation.** `Modifier::Transition(...)` round-trips; nothing interpolates.
- **Drop shadows, blur, effects, margins.** `Shadow`/`Blur`/`Effect`/`Margin` are wire-frozen and ignored.
- **Tooltip and Menu widgets.** Wire slots only; use `Popover` with `prefab::popover_menu`.
- **Layout changes from state.** Modifiers inside `Hover`/`Focus`/`Active`/`Disabled`/`WhenDensity` only repaint.
- **Font weights.** All Inter styles render at the default weight.
- **Custom fonts / hex colours.** System fonts only, palette tokens only.
- **Free pixel values** compile (lengths are plain `u16`), but padding, radius and spacing should come from the scales above so the UI stays consistent.

---

## Example: minimal launcher

```rust
use nopeek_widgets::{prefab, *};

fn render(state: &State) -> Widget {
    let badge = prefab::badge(env!("CARGO_PKG_NAME"));
    let search = prefab::input_autofocus(
        &state.query,
        "Type to search…",
        prefab::InputKind::Search,
        prefab::NO_ACTION,               // Enter reaches the app as Event::Key
        Some(badge),
    );

    let rows: Vec<Widget> = state.results.iter().enumerate().map(|(i, r)| {
        prefab::list_row(
            r.icon, &r.title, &r.subtitle,
            i == state.selected,
            Some(ActionId(i as u32)),             // click
            Some(ActionId(10_000 + i as u32)),    // hover offset
        )
    }).collect();

    let footer = prefab::footer("↑↓  ↵  esc", &alloc::format!("{} results", state.results.len()));

    prefab::panel(alloc::vec![
        search,
        Widget::Divider,
        prefab::scroll_list(rows),
        Widget::Spacer { flex: 1 },
        Widget::Divider,
        footer,
    ])
}
```

Visible behaviour from this code alone, without writing any CSS or
animation: hover fill on rows, focus borders on Tab nav, accent border
on the selected row, an `Accent` border around the focused search field,
theme changes repaint the whole tree. The styling is in the prefabs.
