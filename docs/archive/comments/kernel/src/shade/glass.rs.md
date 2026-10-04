# `kernel/src/shade/glass.rs` @ 5e0102684

## L1-10 · `use spin::Mutex;`

```
//! Glass parameters — computed from the wallpaper and the theme.
//!
//! Glass (loop, dock, bar) is a fill blended over the blurred wallpaper.
//! How much blur, how much fill, and where the fill must hold the text
//! legible depend on the picture: a calm dark image wants a light touch, a
//! bright busy one wants more of everything. One fixed set per theme was
//! right for one wallpaper at a time. So the wallpaper is measured once
//! when it is set (`background::compute_blur`), and every value below is
//! derived from that measurement. A key set with `set` wins over the
//! derived value; `unset` returns it to automatic.
```

## L17 · `#[derive(Clone, Copy)]`

```
/// What the wallpaper looks like, measured on its 1/4-size copy.
```

## L20 · `pub p10: u32,`

```
/// Luma percentiles of the BLURRED image (what glass sits on).
```

## L24-25 · `pub detail: u32,`

```
/// Mean luma standard deviation inside 4x4 blocks of the SHARP image —
/// how much fine detail would compete with text.
```

## L27 · `pub mean_rgb: u32,`

```
/// Mean colour of the blurred image (0xRRGGBB).
```

## L36 · `#[derive(Clone, Copy)]`

```
/// The values glass is drawn with.
```

## L39 · `pub blur: usize,`

```
/// Box radius on the 1/4-size image; 0 = no blur.
```

## L41 · `pub loop_opacity: u32,`

```
/// Fill weight of the loop glass, 0..256.
```

## L43 · `pub chrome_opacity: u32,`

```
/// Fill weight of dock and bar, 0..255.
```

## L48-49 · `const CONTRAST_X10: u64 = 60;`

```
/// Contrast the glass must keep to its text colour, ×10 (60 = 6:1 —
/// between WCAG AA 4.5 and AAA 7 for body text).
```

## L51-52 · `const TINT_WEIGHT: u32 = 26;`

```
/// Wallpaper colour mixed into the fill (×256): ~10 % ties the glass to
/// the picture; text keeps its neutral colour.
```

## L54-56 · `const MARGIN: u32 = 20;`

```
/// How far below the brightness ceiling the median of the picture is
/// taken: the ceiling is the least legible glass, the margin makes it
/// comfortable.
```

## L58-59 · `const FILL_MIN: u32 = 160;`

```
/// Fill weight bounds (×256). The floor keeps glass calm on every picture —
/// see-through more than this is a choice someone makes with `set`.
```

## L63 · `const NO_WALLPAPER: Stats = Stats { p10: 24, p50: 24, p90: 24, detail: 0, mean_rgb: 0x181820 };`

```
/// Measurement used before any wallpaper is set (flat grey background).
```

## L66-67 · `pub fn auto_blur(detail: u32) -> usize {`

```
/// Blur radius for a measured detail level: the busier the picture, the
/// more it has to be calmed before text can sit on it.
```

## L69-70 · `(2 + detail / 4).clamp(2, 5) as usize`

```
// Florian's set measures 5-8 here and still carries fine grain that
// fights text, so the scale starts at 2 (~10 px), not 1.
```

## L78-80 · `pub fn params() -> Params {`

```
/// Glass is dark in both themes — loop, dock and bar are the only glass,
/// and dark glass with light text reads over any wallpaper; only ordinary
/// apps follow the theme.
```

## L87-88 · `let lt = lin(text);`

```
// Most brightness glass may have under the text, from the contrast
// ratio. Gamma ~2: L = v² / 255².
```

## L94-96 · `let m = st.p50;`

```
// Fill weight: enough to take the MEDIAN of the picture a margin under
// the ceiling (m + a·(fill − m) = target); the ceiling then holds the
// remaining bright patches per pixel.
```

## L117 · `pub fn report() {`

```
/// `shade glass`: what was measured and what it led to.
```

## L145 · `fn lin(v: u32) -> u64 { (v as u64) * (v as u64) }`

```
/// Linear light ×65025 with a gamma of 2 — close enough to sRGB for a bound.
```

