# UI_SCALE.md — one scale factor for the whole UI, fractions included

Status: **step 1 built** (2026-10-20, kernel 0.656.0, dock 0.8.3,
snap 0.5.0), device test open. `set shade.scale 1.25` applies at once
(step 6, minus a range check per output): every scene is laid out again,
panels/dock/overlays are converted, open popups close with `PopupDone`.
Step 4 built (kernel 0.657.0, gpu_intel_tgl 0.34.0): `loop` draws IBM
Plex Mono at 14 logical px, one cell source (`terminal::cells`), glyph
coverage blended by `scene::text` and `compose.comp` (gpu_intel_tgl 0.34.0,
gpu_amd_renoir 0.12.0). Step 2 built (kernel 0.658.0): `regen-icons`
writes atlas version 2 with every icon's fill outline (strokes stroked by
tiny-skia, curves flattened, 1/64 units of the 256 grid); the kernel fills
it at the exact size asked for (`gui/icon_raster.rs`, nonzero, 16 sample
rows, exact horizontal coverage) and caches it; the bitmaps stay as the
fallback. Mean difference to resvg 1.04/255; at 16 px the outline is ~17 %
darker because tiny-skia draws 1 px strokes as hairlines. The window
close X follows the factor (part of step 3). Steps 3 (rest) and 5 open.

## Why

On the 4K NUC every widget app — deck, dock, loft, spell, beak, tune — is
drawn at the pixel sizes it was written for: 14 px body text, 16 px icons,
44 px panels. On a 27" 4K screen that is about half the size it should be.
A person who finds the native size too small has no knob at all, and one
who wants 1.25 or 1.5 rather than 2 has no knob either.

## What there is today

| Part | Scale | Where |
|---|---|---|
| Window chrome, halo, gaps | integer `Compositor::scale`, 1 or 2 | `shade/compositor.rs` |
| The scale itself | `font::scale_for(width)`: > 2560 px → 2, else 1; not settable | `gui/font.rs` |
| Widget layout and raster | none — `RasterTarget.scale` exists and is always 1 | `shade/widgets/` |
| Widget text | vector (Inter, IBM Plex Mono), `font_size_of` px | `shade/widgets/layout.rs` |
| Icons | atlas at 16/24/32/48/64 px; other sizes resampled (soft) | `gui/icons.rs`, `tools/regen-icons` |
| Terminal (`loop`) | bitmap fonts 8x16 and 16x32 only | `gui/font.rs` |
| Canvas apps (beak, tune) | own pixels; beak reports `devicePixelRatio` 1.0 | `npk_canvas_rect`, beak-engine |

## The model (after Wayland `fractional-scale-v1` + `wp_viewporter`)

- **Logical pixels.** Every size an app writes into its tree is logical:
  `MinWidth(340)`, `FontSize(15)`, `Icon { size: 16 }`, `PaddingXY`, radii,
  borders, `Spacing`. Apps do not change; they never learn the factor
  unless they ask for it (canvas apps, below).
- **One factor per output**, `shade.scale`, from 1.0 to 3.0 in steps of
  1/120 (Wayland's unit, so 1.25, 1.5, 1.75 are exact). Default from the
  screen as today: 2.0 above 2560 px wide, else 1.0. Changing it lays every
  window out again; no restart.
- **The compositor scales, once, in one place.** Layout multiplies logical
  lengths by the factor and rounds to physical pixels; raster draws at
  physical size. Hit testing, hover, popups and anchors stay in physical
  pixels — they already are — so input needs no translation.
- **Rounding rule** (what GTK and Qt learned the hard way): a rectangle's
  edges are rounded, not its size — `x0 = round(x * s)`, `x1 = round((x +
  w) * s)`, `w' = x1 - x0`. Neighbours then meet without a gap or an
  overlap. A 1 px logical border becomes `max(1, round(s))` px, never a
  blurred fraction.

### What crosses the ABI in which unit

- **Logical:** the widget tree, `npk_screen_size`, `npk_window_set_overlay`,
  `_overlay_at`, `_dock`, `_panel`, the popup `gap`. An app lays itself out
  against the logical screen.
- **Physical:** `npk_canvas_rect`, `npk_cursor_pos`, `MouseMove`, `Wheel`
  (a canvas app maps them onto its own pixels), and `npk_capture_screen`,
  sized by the new `npk_capture_size`.

## Steps

### 1. Widget layout and raster (the bulk, every app at once)

- `layout.rs`: one `Scale` in the layout context; every length read from a
  modifier or a style goes through it (`MinWidth`, `MaxWidth`, `MinHeight`,
  `MaxHeight`, `Padding*`, `Spacing`, `Rounded`, `Border`, `FontSize`,
  `Icon.size`, `mark` sizes). `measure` returns physical sizes; `place`
  rounds edges by the rule above.
- `render.rs`: radii, borders, rings and shadows at physical size; text at
  `round(px * s)`.
- `RasterTarget.scale` becomes the factor (u8 → 1/120 units).
- `classify_density` keeps working on **logical** width, or a 4K window
  would turn "spacious" just for being drawn bigger.
- Popups (`npk_popup`): `commit_size` measures physical, `gap` is logical
  and scaled. Panels: `set_panel` heights are logical; the compositor
  reserves the scaled band.

### 2. Icons at any size

The atlas is pre-rasterised because the kernel has no SVG renderer.
Options, in order of preference:

- **Rasterise the path at runtime.** The Phosphor icons are strokes and
  rounded rects on a 256 grid. A small path rasteriser (lines, cubic and
  arc segments, round caps/joins, coverage AA) in the kernel draws any size
  exactly; results are cached per (icon, px). This is the clean answer and
  the one that also serves a later theme with filled icons.
- Fallback: ship more atlas sizes (20, 28, 40, 56). Covers 1.25 / 1.75 for
  the common 16 px, but every new size is another 50 KB per icon set and
  still misses 1.1 or 1.4.

Until then, an icon whose physical size is not in the atlas is drawn from
the next larger size, as today (soft, not wrong).

### 3. Window chrome

`Compositor::scale` becomes the same factor: border width, rounding, gaps,
halo band, the close glyph. Integer math today; the rounding rule above
applies.

### 4. The terminal

`loop` draws with bitmap fonts. For fractions it moves to IBM Plex Mono
through the same vector path as widget text (cell size = advance and line
height at the scaled size, snapped to whole pixels so the grid stays
crisp). Until then the terminal keeps 1x or 2x, picked as the integer
nearest to the factor.

### 5. Canvas apps

An app that paints its own pixels needs the factor to draw sharp:

- `npk_canvas_rect` reports physical size (it does) **and** the factor
  (new call or field, appended).
- beak: `window.devicePixelRatio` = the factor; CSS px × factor = canvas
  px. That is exactly the web's model, so pages already know what to do.
- tune: video is scaled by the compositor anyway; its controls are widgets.

### 6. The setting

`set shade.scale 1.25` (stored, read at boot; on change every window is
laid out again and panels re-reserve their band). Range 1.0..=3.0, clamped.
`unset shade.scale` returns to the default from the screen width.

## What it costs

- Layout does a multiply and a round per length — negligible next to text
  shaping.
- Raster: more pixels at higher factors, the same as any HiDPI desktop.
  The GPU compositor scene sees bigger buffers, nothing new in kind.
- One subtle risk: code that assumes `size_px` of a style equals what is
  on screen (e.g. tests comparing pixel sizes). Tests take the factor 1.0
  path unless they ask otherwise.

## Order

1 → 6 is the order of value: step 1 alone makes every app the right size
on 4K, at the price of soft icons at fractional factors; step 2 makes them
sharp; 3 and 4 make the chrome and the terminal agree; 5 serves the
browser; 6 is the knob, but a fixed default can ship with step 1.
