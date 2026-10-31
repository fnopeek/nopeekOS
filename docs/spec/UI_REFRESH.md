# UI design tokens

The colours, sizes and states that `shade` draws and that apps reference
through the `nopeek_widgets` SDK. Apps name tokens and scale steps; the
compositor resolves them. The token table lives in
`kernel/src/shade/widgets/palette.rs`, the SDK mirror in
`tools/wasm/sdk/widgets/src/abi.rs` and `style.rs`.

All lengths are logical px. The compositor multiplies every length, radius,
border width and font size by the UI scale (`shade.scale`, 1.0 to 3.0 in
1/120 steps; unset: 2.0 on screens wider than 2560 px, else 1.0). A
non-zero length never rounds to zero.

---

## 1. Colour ramps

### Theme

| `theme` | Palette |
|---|---|
| `dark` | dark |
| `light` | light |
| `auto` (setup default) | light if the wallpaper's darkest palette colour has luma > 128, else dark; dark without a wallpaper |

Luma is `(299 R + 587 G + 114 B) / 1000` throughout.

Panels (strut panels such as `deck`, their popups) and the `loop` terminal
are glass and always use the **dark** palette, whatever `theme` says.
Every other window uses the active theme.

### Surface, text and status tokens

Token values are fixed per theme. The wire id is the token's `u8` value
and is frozen; tokens are only appended.

| Id | Token | Dark | Light | Role |
|---|---|---|---|---|
| 12 | `Page` | `#131517` | `#FFFFFF` | Content canvas: editor body, page, terminal |
| 0 | `Surface` | `#17191B` | `#FAF9F7` | Window body; clear colour of an opaque scene |
| 1 | `SurfaceElevated` | `#1C1F21` | `#F0F0EC` | Menu bar, tab strip, menus, cards |
| 2 | `SurfaceMuted` | `#1E2124` | `#F1F0EC` | Sunken: sidebar, field, inactive-tab hover |
| 13 | `SurfaceHover` | `#262A2E` | `#E4E3DE` | Hover fill, chips |
| 8 | `Border` | `#2C3034` | `#DEDCD6` | 1 px separators and outlines, `Divider` |
| 3 | `OnSurface` | `#E6E8EA` | `#1B1D1F` | Primary text, default glyph colour, caret |
| 4 | `OnSurfaceMuted` | `#A2A8AE` | `#4E5358` | Secondary text, `TextStyle::Muted`, scrollbars |
| 14 | `OnSurfaceFaint` | `#6F767C` | `#6E747A` | Section heads, meta, placeholders, line numbers |
| 9 | `Success` | `#8FBF9F` | `#4D8A63` | Success, TLS lock |
| 10 | `Warning` | `#E0B877` | `#B07A28` | Warning |
| 11 | `Danger` | `#E07B7B` | `#C04C4C` | Destructive actions, failures |

Every token resolves to an opaque colour. The rasterizer ignores a
token's alpha byte; translucency comes only from a panel's fill opacity
(section 4).

An app that paints its own pixels reads a resolved token with
`npk_theme_token(id)` (needs the `RENDER` right; 0 for an unknown id). It
answers from the palette the caller's window is drawn with: the glass
palette for a panel, the active theme otherwise.

## 1a. Code ramp

A second ramp for source text, independent of the accent. A
`TextArea` span or an `Input` span names one of these; bytes without a
span render in `OnSurface` on `Page`.

| Id | Token | Role |
|---|---|---|
| 17 | `CodeKeyword` | Declaration and storage keywords |
| 18 | `CodeControl` | Control flow and imports |
| 19 | `CodeString` | String and character literals |
| 20 | `CodeComment` | Comments |
| 21 | `CodeNumber` | Numeric literals |
| 22 | `CodeFunction` | Function names |
| 23 | `CodeType` | Type and class names, markup tag names |
| 24 | `CodeVariable` | Attribute names, JSON keys, decorators |
| 25 | `CodeConstant` | Language constants, escapes |

The values come from the scheme named by `code.scheme`. `auto` (default)
takes `dark-plus` under the dark theme and `light-plus` under the light
theme. A scheme sets only these nine colours; canvas and plain text stay
theme tokens.

| Scheme | Canvas | Keyword | Control | String | Comment | Number | Function | Type | Variable | Constant |
|---|---|---|---|---|---|---|---|---|---|---|
| `dark-plus` | dark | `#569CD6` | `#C586C0` | `#CE9178` | `#6A9955` | `#B5CEA8` | `#DCDCAA` | `#4EC9B0` | `#9CDCFE` | `#569CD6` |
| `light-plus` | light | `#0000FF` | `#AF00DB` | `#A31515` | `#008000` | `#098658` | `#795E26` | `#267F99` | `#001080` | `#0000FF` |
| `monokai` | dark | `#66D9EF` | `#F92672` | `#E6DB74` | `#88846F` | `#AE81FF` | `#A6E22E` | `#A6E22E` | `#F8F8F2` | `#AE81FF` |
| `solarized-dark` | dark | `#93A1A1` | `#859900` | `#2AA198` | `#586E75` | `#D33682` | `#268BD2` | `#CB4B16` | `#93A1A1` | `#B58900` |
| `solarized-light` | light | `#586E75` | `#859900` | `#2AA198` | `#93A1A1` | `#D33682` | `#268BD2` | `#CB4B16` | `#93A1A1` | `#B58900` |
| `abyss` | dark | `#9966B8` | `#225588` | `#22AA44` | `#384887` | `#F280D0` | `#DDBB88` | `#FFEEBB` | `#6688CC` | `#F280D0` |
| `kimbie-dark` | dark | `#98676A` | `#98676A` | `#889B4A` | `#A57A4C` | `#F79A32` | `#8AB1B0` | `#F06431` | `#DC3958` | `#F79A32` |
| `quiet-light` | light | `#7A3E9D` | `#4B69C6` | `#448C27` | `#AAAAAA` | `#9C5D27` | `#AA3731` | `#7A3E9D` | `#7A3E9D` | `#9C5D27` |

`set code.scheme` rejects an unknown name and lists the known ones. A
scheme whose canvas differs from the active theme is applied, and `set`
prints a note.

## 2. Accent

### Choice

`accent` takes a preset, a `#RRGGBB` literal, or `auto`.

| `accent` | Raw accent |
|---|---|
| `rose` | `#E39BAB` |
| `sage` | `#8FBF9F` |
| `blue` | `#9FB8E0` |
| `amber` | `#E0B877` |
| `#RRGGBB` | that colour |
| `auto` or unset, wallpaper set | wallpaper palette colour 1, each channel raised by `100 - luma` when its luma is below 100 |
| `auto` or unset, no wallpaper | `#E39BAB` |

Preset names match case-insensitively.

### Contrast adjustment

The raw accent is adjusted against the theme's `Surface` before use:

- Dark: kept if its luma differs from `Surface` by at least 80, else every
  channel is raised by `0x60`.
- Light: darkened in steps of `0x10` per channel (at most 16 steps) until
  its luma is 95 or lower, so it reads as text.

### Accent tokens

| Id | Token | Value |
|---|---|---|
| 6 | `Accent` | adjusted accent |
| 7 | `AccentMuted` | `Accent` mixed over `Surface` at 38/255 (15 %) |
| 15 | `AccentRing` | `Accent` mixed over `Surface` at 56/255 (22 %) |
| 16 | `AccentLine` | `Accent` mixed over `Surface` at 115/255 (45 %) |
| 5 | `OnAccent` | Light: `#FFFFFF`. Dark: `#101010` mixed with `Accent` at 24/255 if `Accent` luma > 128, else `#FFFFFF` |

Resolved values for the presets:

| Theme | Preset | `Accent` | `AccentMuted` | `AccentRing` | `AccentLine` | `OnAccent` |
|---|---|---|---|---|---|---|
| dark | `rose` | `#E39BAB` | `#352C30` | `#43353A` | `#73535B` | `#231D1E` |
| dark | `sage` | `#8FBF9F` | `#28312E` | `#313D37` | `#4D6356` | `#1B201D` |
| dark | `blue` | `#9FB8E0` | `#2B3038` | `#343B46` | `#546073` | `#1D1F23` |
| dark | `amber` | `#E0B877` | `#343028` | `#433B2F` | `#716044` | `#231F19` |
| light | `rose` | `#833B4B` | `#E8DCDD` | `#DFCFD1` | `#C4A3A9` | `#FFFFFF` |
| light | `sage` | `#3F6F4F` | `#DEE4DD` | `#D0DAD2` | `#A5BAAB` | `#FFFFFF` |
| light | `blue` | `#3F5880` | `#DEE1E5` | `#D0D5DC` | `#A5B0C1` | `#FFFFFF` |
| light | `amber` | `#805817` | `#E7E1D5` | `#DFD5C5` | `#C2B091` | `#FFFFFF` |

## 3. Widget states

### How states apply

A widget's effective modifier list is built in this order, later entries
winning for single-valued modifiers (`Background`, `Border`, `Rounded`,
`Ring`, `Tint`):

1. base modifiers
2. `WhenDensity(d, ..)` matching the window's density
3. `Disabled(..)`
4. `Hover(..)` while the pointer is over the widget or a descendant
5. `Focus(..)` while the widget or a descendant has focus
6. `Active(..)` while the widget or a descendant is pressed

`Disabled` is present-or-absent on the widget. While present, `Hover`,
`Focus` and `Active` are skipped, clicks on the widget and its subtree are
swallowed, and the subtree is left out of Tab order.

| Modifier | Rendering |
|---|---|
| `Background(t)` | filled rect, rounded by the corner radius |
| `Border { t, w, r }` | stroke of `w` inside the rect |
| `Rounded(r)` | corner radius; wins over a `Border` radius |
| `Ring { t, w }` | stroke of `w` outside the rect, outer radius = radius + `w`; no layout space |
| `Tint(t)` | colour of text and icons; inherited by the subtree |
| `Opacity(o)` | blends the painted rect towards `Surface` by `255 - o` |
| `Scale(q)` | icon scale, Q8.8 (256 = 1.0), inherited by icons below |
| `Shadow`, `Blur`, `Effect`, `Transition` | accepted, not rendered |

Radii are clamped to half the shorter side, so `Radius::Pill` (255) gives
a pill.

### Density

| `Density` | Logical window width |
|---|---|
| `Compact` | < 600 |
| `Regular` | 600 to 1199 |
| `Spacious` | >= 1200 |

### Scales (SDK `style.rs`)

| Step | `Radius` | `Spacing` | `Padding` |
|---|---|---|---|
| `None` | 0 | 0 | 0 |
| `Xxs` | | 2 | 2 |
| `Xs` | | 4 | 4 |
| `Sm` | 4 | 8 | 8 |
| `Md` | 8 | 12 | 12 |
| `Lg` | 12 | 16 | 16 |
| `Xl` | 16 | 24 | 24 |
| `Xxl` | | 32 | |
| `Pill` | 255 | | |

### Text styles

| `TextStyle` | Size | Default colour |
|---|---|---|
| `Title` | 22 | `OnSurface` |
| `Heading` | 15 | `OnSurface` |
| `Body` | 13 | `OnSurface` |
| `Muted` | 13 | `OnSurfaceMuted` |
| `Caption` | 10 | `OnSurface` |
| `Mono` | 13 (monospace face) | `OnSurface` |

`Modifier::FontSize` overrides the size, clamped to 6..64. Each style
declares a weight, but every style renders at the font's default weight.
Text wider than its box is ellipsized.

### `toolbar_button`

Built by `prefab::icon_button`; beak and iris build the same cell by hand.

| Property | Value |
|---|---|
| Cell | 28 x 28, icon centred |
| Radius | 7 |
| Rest | no fill, icon `OnSurface` |
| Hover | `Background(SurfaceHover)` |
| Active | `Background(AccentMuted)`, `Tint(Accent)` |
| Focus | `Ring { AccentRing, 2 }` (prefab only) |

### `text_field`

The field row in loft and beak: a `Row` holding a 16 px leading icon and an
`Input`.

| Property | Value |
|---|---|
| Height | 30 (`MinHeight`) |
| Padding | x 8, y 0 |
| Rest | `Background(SurfaceMuted)`, `Border { Border, 1, 8 }` |
| Focus | `Border { Accent, 1, 8 }`, `Ring { AccentRing, 3 }` |

`prefab::input` is lighter: padding 12, radius 8, no fill, focus
`Border { Accent, 1, 8 }` and no ring; the `Search` kind leads with a
24 px magnifier in `Accent`.

### `list_row`

The data row in loft and pick. Selection is a tint plus a 2 px leading
edge, never an outline.

| Property | Value |
|---|---|
| Height | 34, padding 4 |
| Leading edge | 2 px wide, 26 px tall (`prefab::mark`), present on every row |
| Rest | no fill, edge empty, icon `OnSurfaceMuted` |
| Hover | `Background(SurfaceHover)` |
| Selected | `Background(AccentMuted)`, edge `Accent`, icon `Tint(Accent)` |
| Metadata columns | `Mono`, `OnSurfaceMuted` |
| Column headers | 30 high, `Mono`, `OnSurfaceFaint`; active sort column `OnSurfaceMuted`; hover `OnSurface` |

`prefab::nav_row` (sidebars) follows the same language: height 30, radius
7, padding 4, 16 px icon. Selected: `Background(AccentMuted)`,
`Tint(Accent)`. Otherwise icon `OnSurfaceMuted`, hover
`Background(SurfaceHover)`, focus `Ring { AccentRing, 2 }`.

### `tab`

Fixed width, label ellipsized, height 30, radius 8 on all corners (the ABI
has no per-corner radius). Tab strip: `SurfaceElevated`, tabs
bottom-aligned, 2 px between tabs. The active tab takes the colour of the
content below it.

| | beak | spell |
|---|---|---|
| Strip height | 36 | tab height + 2 x 4 padding |
| Tab width | 160 | 200 |
| Active fill | `Surface` | `Page` |
| Active marker | 2 px `Accent` bar inside the tab's top edge | none |
| Inactive | no fill, label `OnSurfaceMuted` | no fill, label `OnSurfaceMuted` |
| Hover (inactive) | `Background(SurfaceMuted)` | `Background(SurfaceMuted)` |
| Close | 22 x 22 `X` button on every tab, radius 4, hover `SurfaceHover` | `X` on the active tab only, `Accent` if unsaved else `OnSurfaceMuted` |
| Unsaved, inactive | | 8 x 8 `Accent` dot in the close slot |

### Other prefabs

| Prefab | Rest | Hover | Active | Focus |
|---|---|---|---|---|
| `button` `Primary` | `Accent`, label `OnAccent` | `AccentMuted` | `AccentMuted` | `Border { Accent, 2, 8 }` |
| `button` `Secondary` | `SurfaceElevated` | `SurfaceMuted` | `Border` | same |
| `button` `Ghost` | `Surface` | `SurfaceMuted` | `Border` | same |
| `button` `Destructive` | `Danger`, label `OnAccent` | `Danger`, `Opacity(220)` | `Danger`, `Opacity(190)` | same |
| `menu_bar_with_icon` label | `Body`, `OnSurfaceMuted`, padding 8 | `SurfaceHover`, `OnSurface`, radius 4 | | |
| `popover_menu` row | `Body`, padding 8, shortcut `Caption` `OnSurfaceFaint` | `SurfaceMuted`, radius 4 | | |
| `breadcrumb` segment | `Mono`, `OnSurfaceMuted`; last segment `SurfaceHover` | `SurfaceHover`, radius 4 | | |
| `list_row` | the `list_row` state above: min height 34, padding 4, 2 px leading edge, 24 px icon `OnSurfaceMuted` | `SurfaceHover`, radius 4 | | `Ring { AccentRing, 2 }` |
| `list_row` selected | `AccentMuted`, edge and icon `Accent` | | | |
| `grid_item` | padding 8, 64 px icon, radius 8 | `SurfaceHover` | | `Ring { AccentRing, 2 }` |
| `grid_item` selected | `AccentMuted`, icon `Accent` | | | |

Buttons have padding 12 and radius 8. Containers:

| Prefab | Fill | Border | Radius | Padding |
|---|---|---|---|---|
| `card` `Inset` | `Surface` | `Border` 1 | 12 | 16 |
| `card` `Panel`, `Sheet` | `SurfaceElevated` | none | 12 | 16 |
| `dialog` | `SurfaceElevated` | none | 12 | 16 (12 at `Compact`) |
| `popover_menu` | `SurfaceElevated` | `Border` 1 | 8 | 4, min width 180 |
| `sidebar_pane` | `SurfaceMuted` | none | 0 | 8 |
| `sidebar_section` label | `Mono`, `OnSurfaceFaint`, padding 8 | | | |
| `badge` | `SurfaceMuted`, `Caption` | none | 0 | 4 |

### Built-in leaf painting

What the compositor draws for a leaf without app modifiers:

| Widget | Drawing |
|---|---|
| `Button` | `Accent` fill unless a `Background` is set; label `Body`, `OnSurface` or the tint; 16 px icon; inset 8 x 4 |
| `Input` | no fill; text inset 4; value `Body` `OnSurface`; placeholder `OnSurfaceFaint`; width from the placeholder, min 120 |
| `TextArea` | no fill; `Mono` 13; placeholder `OnSurfaceMuted`; gutter numbers `OnSurfaceFaint` with a 1 px `Border` rule |
| `Checkbox` | 16 x 16, `Border` frame, 2 px inset filled `Accent` when checked, else `Surface` |
| `Slider` | 4 px track `SurfaceMuted`, fill and 14 px thumb `Accent`, radius 2 |
| `Divider` | 1 px `Border` |
| `Canvas` | `SurfaceMuted` until the app commits pixels |
| `Scroll` scrollbar | overlay thumb 4 px wide, 2 px from the edge, `OnSurfaceMuted`, radius 2, min length 24; only when content overflows |

## 3a. Focus and text input

| Event | Behaviour |
|---|---|
| Window's first commit | Focus is on nothing, unless a text widget carries `Modifier::Autofocus` |
| A commit while nothing is focused | An `Autofocus` text widget takes focus |
| `FocusRequest(n)` with a new `n` | That `Input`/`TextArea` takes focus with its whole text selected |
| Click on an `Input`/`TextArea` | Focus moves there; the caret goes to the clicked position |
| Click on a field row (a subtree with exactly one text widget and no other click target) | Focus moves to its text widget; the caret goes to the end |
| Click anywhere else in the window | Focus is released |
| Tab / Shift+Tab | Next / previous focusable widget (`Button`, `Input`, `TextArea`, `Checkbox`, anything with `OnClick`), wrapping |
| Focus leaves a field | The typed buffer stays on screen and is parked under that field |
| Focus returns to that field | The parked buffer, caret and selection resume |

Only a focused `Input` or `TextArea` has an editor. Its buffer leads the
app's tree by one round trip, which is why it is parked and not dropped.

| Element | Value |
|---|---|
| Caret | 2 px wide, one line high, `OnSurface`; drawn only while focused; no blink |
| Selection | `AccentMuted` under the glyphs, min width 2 |
| Focused empty `Input` | caret only, no placeholder |
| Masked `Input` (`Modifier::Mask`) | one bullet per character |
| Double / triple click | select word / line |
| Enter | `TextArea`: newline. `Input`: submit if it has an `on_submit`, else passed on as a key |
| Tab in a `TextArea` | inserts 4 spaces |
| Ctrl+C / X / V / A | copy, cut, paste, select all |

## 4. Window chrome

The compositor draws the border, the focus halo and the window controls.
It draws no title and no shadow; the app fills the inside.

| Property | Value |
|---|---|
| Gap between tiles | `shade.gaps`, default 8 |
| Border width | `shade.border`, default 1 |
| Corner radius | `shade.rounding`, default 10; content radius = rounding - border |
| Fullscreen window | no border, square corners, controls only at the top edge |

### Border colour

A colour pinned with `shade.border_active` / `shade.border_inactive`
(hex) wins in both cases. Otherwise:

| | Wallpaper set | No wallpaper |
|---|---|---|
| Focused | 45 degree gradient between wallpaper palette colours 1 and 2, each raised to luma >= 80 | `AccentLine` |
| Unfocused | wallpaper palette colour 8 | `Border` |
| Blend | 235/256 | 130/256 |

### Focus halo

| Property | Value |
|---|---|
| Width | `shade.glow`, default 6, at most the gap; 0 turns it off |
| Shown | on the focused tile when at least two tiles share the workspace |
| Colour | wallpaper accent (section 2, `auto` row) if a wallpaper is set, else the focused border colour |
| Strength | steady 100/255; on a focus change it starts at 255 and settles over 240 ms |

### Window controls

A pill of buttons at the right end of the window's top band, drawn after
the content so it is always on top. Clicks inside it never reach the
app. Drawn on every tiled or floating window except panels, overlays,
popups and `Surface` windows.

| Button | Does | Key | Glyph |
|---|---|---|---|
| Fullscreen | fullscreen on (off on the top-edge pill) | Mod+F | `ArrowsOut` (`ArrowsIn` when fullscreen) |
| Float | floating, or back into the tiling | Mod+V | the state it leads to: `Browsers`, `SquaresFour` |
| Split | flip the split the window sits on | Mod+J | the orientation it flips to: `SquareSplitVertical`, `SquareSplitHorizontal` |
| Close | close (`request_close_window`) | Mod+Q | `X` |

| Property | Value |
|---|---|
| Buttons | 26 x 26, 2 apart, left to right Fullscreen, Float, Split, Close |
| Position | centred in the 36 px band, right inset equal to the top inset (5) |
| Reserve | 4 x 26 + 3 x 2 + 5 = 115 (`WINDOW_CONTROLS_W`, section 5) |
| Glyph | 16 px, `OnSurfaceMuted` (glass palette on `loop`) |
| Unfocused window | glyphs at 129/255, full while the pointer is on the pill |
| Disabled | Split while the window floats or has no split: drawn at 80/255, not removed |
| Hover | `SurfaceHover` fill, radius 4; Close in `Danger` (glyph, and its fill at 56/255) |
| Hint | after 400 ms on a button: label and key (`Full screen · Super+F`), in `lang`, the key spelled from `shade.mod`; `Surface` text on `OnSurface` at 235/255, radius 6, below the button |
| Narrow window | content narrower than 345: `More` (`DotsThree`) and Close; `More` opens a menu with the other three, their labels and keys |
| Fullscreen | no pill in the band; the pointer resting 300 ms on the top screen row brings the pill in at the top right on a `SurfaceElevated` backdrop, and it leaves with the pointer |

`shade.controls` sets how much is shown: `full` (default) all four,
`compact` only Close until the pointer is over the corner, `none` no
buttons. The reserve is the same in every mode, so a change never moves
the app's layout.

### Glass

`loop` terminal windows and translucent panels are drawn over a blur of
the wallpaper. Glass values are derived from the wallpaper once it is set;
a key set with `set` wins, `unset` returns it to automatic, `shade glass`
reports both.

| Value | Automatic | Key |
|---|---|---|
| Blur radius (on the 1/4-size image) | `2 + detail / 4`, clamped to 2..5 | `shade.blur` (0 off, max 8) |
| `loop` fill weight | enough to keep text at 6:1 contrast over the median wallpaper luma, clamped to 160..208 of 256 | `shade.opacity` (0..256) |
| Panel fill opacity | `loop` weight x 255/256 + 30, max 255 | `shade.chrome_opacity` (0..255) |
| Brightness ceiling under text | from the 6:1 contrast to dark `OnSurface` | `shade.dark_ceil` (0..255) |
| Wallpaper colour in the fill | 26/256 (10 %) | `shade.glass_tint` (0..40 %) |

`loop` glass is the dark `Surface` at the `loop` weight. A panel is
translucent only when its tree's root is a `Stack`: the scene clears
transparent and fills backgrounds at the panel opacity. The panel
(`deck`) takes `shade.bar_opacity` if set, else `shade.chrome_opacity`.
Setting `shade.chrome_opacity` clears `shade.bar_opacity`. Panel geometry
is in [`PANEL.md`](PANEL.md).

Every other widget window is opaque and clears to the theme's `Surface`.

### Screenshot flash

The screen dips towards black at 110/255 and drains quadratically over
120 ms.
`shade.flash_opacity` sets the peak; 0 turns it off.

## 5. App window structure

The compositor places the window controls in a 36 px top band, so apps
put their top band at that height and leave its right end free.
`prefab::menu_bar_with_icon` builds it:

| Band | Value |
|---|---|
| Menu bar | min height 36, `SurfaceElevated`, padding 4, 2 px between items |
| App icon | 16 px, `Accent`, padding 4 |
| Menu labels | as in section 3, followed by a flex spacer |
| Reserve | `window_controls_reserve()`: empty, exactly `WINDOW_CONTROLS_W` (115) wide, last |

A top band built by hand ends with `window_controls_reserve()` too.

The rest is app code. The values the first-party apps share:

| Element | Value | Apps |
|---|---|---|
| Tab strip | 36, `SurfaceElevated` | beak |
| Toolbar | 44, padding 8, spacing 8 | beak |
| Field | 30 | beak, loft, pick |
| Sidebar width | 176 | loft |
| Content | `Page` | beak, spell |
| Address field lead icon | `Lock` in `Success` for `https://`, else `Bird` in `Accent` | beak |

## 6. Config keys

`set` and `unset` re-render every scene at once. `shade.gaps`,
`shade.border`, `shade.rounding`, `shade.border_active` and
`shade.border_inactive` apply on the next frame; a changed length
re-tiles the workspace.
`theme <dark|light|auto>` is also an intent of its own.

| Key | Values | Default | Changes |
|---|---|---|---|
| `theme` | `dark`, `light`, `auto` | `auto` | Palette of non-glass windows |
| `accent` | `rose`, `sage`, `blue`, `amber`, `#RRGGBB`, `auto` | `auto` | `Accent` and derived tokens |
| `code.scheme` | `auto` or a scheme name | `auto` | Code tokens |
| `shade.scale` | 1.0 to 3.0 | by screen width | All lengths and font sizes |
| `shade.gaps` | px | 8 | Gap between tiles |
| `shade.border` | px | 1 | Window border width |
| `shade.rounding` | px | 10 | Window corner radius |
| `shade.border_active` | hex | `AccentLine` | Focused border, pinned (wins over the wallpaper) |
| `shade.border_inactive` | hex | `Border` | Unfocused border, pinned (wins over the wallpaper) |
| `shade.glow` | px, 0 off | 6 | Focus halo width |
| `shade.controls` | `full`, `compact`, `none` | `full` | Window controls (section 4) |
| `shade.bar_margin` | px | 6 | Gap between a strut panel and the screen edge |
| `shade.blur` | 0..8 | auto | Glass blur |
| `shade.opacity` | 0..256 | auto | `loop` glass fill |
| `shade.chrome_opacity` | 0..255 | auto | Panel fill opacity |
| `shade.bar_opacity` | 0..255 | `shade.chrome_opacity` | Panel fill opacity override |
| `shade.dark_ceil` | 0..255 | auto | Glass brightness ceiling under text |
| `shade.glass_tint` | 0..40 (%) | auto (10) | Wallpaper colour in glass |
| `shade.flash_opacity` | 0..255 | 110 | Screenshot flash |
