# `tools/wasm/beak-engine/src/color.rs` @ 5e0102684

## L1-17 · `use crate::layout::{Rgb, Rgba};`

```
//! color.rs — CSS `<color>` parsing.
//!
//! Owns ALL colour syntax: hex (#rgb/#rgba/#rrggbb/#rrggbbaa), the functional
//! forms rgb()/rgba()/hsl()/hsla() (comma- and space/slash-separated, int and
//! %), and the full CSS Color Module Level 4 named-colour set. Host-testable,
//! no OS in the loop.
//!
//! Alpha is parsed and KEPT. It travels as `Rgba` to the display list, where
//! the rasteriser composites it over whatever is already in the buffer — the
//! backdrop is only known there, never here. Alpha ZERO stays a case of its
//! own: it is not a shade of a colour, it is the absence of one. Pages reserve
//! a frame's space with `border: 1px solid rgba(0,0,0,0)`, and that must paint
//! nothing rather than blend nothing.
//!
//! `no_std`-safe: only `core`/`alloc`. No libm — the float helpers avoid
//! `round`/`floor`/`abs`/`rem_euclid` (std-only) and use casts + `%` + a
//! hand-rolled `fabs`.
```

## L22-24 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// A parsed `<color>`. `Transparent` is a VALUE, not an absence — see the
/// module note. Callers that own a "paint nothing" state (a border side, a
/// background) must honour it; the rest can use [`parse_color`].
```

## L29-38 · `CurrentColor,`

```
/// `currentcolor` — the element's OWN computed `color`, and NOT resolved
/// here. It cannot be: css-color-4 §6.2 resolves it at used-value time, so
/// a computed value that still says `currentcolor` is what a descendant
/// inherits, and the descendant resolves it against its own `color`. That
/// is the whole of `background-color: inherit` under a `currentcolor`
/// background — resolving early makes the child wear its parent's colour.
///
/// Kept apart from `None` for the same reason `Transparent` is: `None`
/// means "this declaration said nothing usable, keep the previous one",
/// which is the opposite of what `currentcolor` asks for.
```

## L42-44 · `pub fn parse_color_val(v: &str) -> Option<ColorVal> {`

```
/// Parse a CSS `<color>`, keeping "fully transparent" apart from "no value".
/// `None` means "keep the inherited value" — `currentcolor`/`inherit` and
/// unparseable input (forward-compatible, like a browser).
```

## L57-69 · `fn parse_relative(v: &str) -> Option<ColorVal> {`

```
/// CSS Color 5 relative syntax — `<fn>(from <origin> <channels>)`.
///
/// Only the IDENTITY channel list is accepted: the three channel keywords of
/// that function, in order, and an optional `/ alpha`. Then the result IS the
/// origin, whatever colour space the function names, and the origin's own
/// `currentcolor`-ness travels with it — which is exactly what the sixteen
/// `css-color/relative-currentcolor-*` reftests are about (measured: fourteen
/// of them write the identity, and relative syntax appears NOWHERE else in the
/// corpus, so this is the whole of what the form is worth here).
///
/// Anything else — a substituted channel (`hsl(from C 120 s l)`), a
/// permutation (`rgb(from C g r b)`), a `calc()` over a channel — returns
/// `None`, so the declaration is left alone rather than painted as its origin.
```

## L77-78 · `let want: &[&str] = if name == "color" {`

```
// `color()` names its space before the channels, and the channel letters
// depend on it: an xyz space is `x y z`, every rgb-like one is `r g b`.
```

## L93 · `let (channels, alpha) = match args.split_once('/') {`

```
// `/ alpha` is the identity for the alpha channel; a number there is not.
```

## L108-109 · `parse_color_val(origin)`

```
// A concrete origin resolves now; a relative colour over one is not
// deferred by anything.
```

## L113-115 · `fn split_origin(s: &str) -> Option<(&str, &str)> {`

```
/// Split `<origin> <rest>` where the origin may itself be a parenthesised
/// function (`rgb(from rgb(0 0 0) r g b)`), so the split has to count parens
/// rather than take the first space.
```

## L129-131 · `pub fn parse_color(v: &str) -> Option<Rgba> {`

```
/// Parse a CSS `<color>`. `None` means "keep the inherited value" — the caller
/// relies on this for `currentcolor`/`inherit`/`transparent` and for
/// unparseable input (forward-compatible, like a browser).
```

## L135-136 · `ColorVal::Transparent => None,`

```
// A caller with no transparent state keeps the inherited value rather
// than painting the alpha-0 carrier colour (usually black).
```

## L138-139 · `ColorVal::CurrentColor => None,`

```
// A caller with no deferral either — same rule: keep the inherited
// value rather than freeze `currentcolor` to the wrong element's.
```

## L144 · `fn parse_rgba(v: &str) -> Option<(Rgb, u8)> {`

```
/// The one dispatcher: every `<color>` syntax, with its alpha.
```

## L155-156 · `if let Some(inner) = fn_body(l, "rgba") {`

```
// Functional notation. `rgba(` never matches `rgb(` because the `(` is part
// of the probe, so declaration order here is irrelevant.
```

## L169 · `if let Some(inner) = fn_body(l, "hwb") {`

```
// CSS Color 4 functional forms — alpha is always the `/ a` tail.
```

## L188-189 · `if l == "transparent" {`

```
// `transparent` is `rgba(0,0,0,0)` per CSS Color 4 — a value, so it belongs
// here and not in a caller's string compare.
```

## L193-194 · `named_color(l).map(|c| (c, 255))`

```
// `currentcolor`/`inherit` are absent from the table → `None` (= keep
// inherited), as the contract wants.
```

## L198-199 · `fn alpha_255(tok: &str) -> u8 {`

```
/// An alpha token: `0`–`1` number or `0%`–`100%`, clamped. Anything we cannot
/// read is OPAQUE — we never make content vanish on a guess.
```

## L215 · `fn parse_hex(hex: &str) -> Option<(Rgb, u8)> {`

```
// ── hex ──────────────────────────────────────────────────────────────────
```

## L220 · `3 => Some((`

```
// #rgb — each nibble ×17 (0x0..0xF → 0x00..0xFF).
```

## L229 · `4 => {`

```
// #rgba — the 4th nibble is alpha, same ×17 scale.
```

## L237 · `6 => Some((Rgb(hex2(b, 0)?, hex2(b, 2)?, hex2(b, 4)?), 255)),`

```
// #rrggbb
```

## L239 · `8 => {`

```
// #rrggbbaa
```

## L264 · `fn fn_body<'a>(s: &'a str, name: &str) -> Option<&'a str> {`

```
// ── functional notation helpers ──────────────────────────────────────────
```

## L266-267 · `fn fn_body<'a>(s: &'a str, name: &str) -> Option<&'a str> {`

```
/// Strip `name(` … `)`, returning the inner argument string, or `None` if `s`
/// is not that function call.
```

## L272 · `fn tokens(s: &str) -> Vec<&str> {`

```
/// Split channel tokens on commas and/or ASCII whitespace, dropping empties.
```

## L279-280 · `fn parse_rgb(inner: &str) -> Option<(Rgb, u8)> {`

```
/// `rgb()`/`rgba()`. Comma- OR space-separated, modern `r g b / a` slash-alpha,
/// channels as int (0–255) or percentage (0–100%).
```

## L288 · `let ok = if slash.is_some() {`

```
// 3 channels; a 4th is the legacy comma-form alpha (only when no slash).
```

## L304 · `fn parse_hsl(inner: &str) -> Option<(Rgb, u8)> {`

```
/// `hsl()`/`hsla()`. Same separator rules; hue in degrees, s/l as percentages.
```

## L327-328 · `fn legacy_or_slash_alpha(slash: Option<(&str, &str)>, t: &[&str]) -> u8 {`

```
/// The alpha of a legacy `rgb()`/`hsl()` form: the `/ a` tail if present, else
/// a 4th comma token, else opaque. Both spellings meet here so they cannot drift.
```

## L337 · `fn channel_255(tok: &str) -> Option<u8> {`

```
/// An rgb() channel: `0..255` int/float, or `0%..100%` → `0..255`. Clamped.
```

## L349 · `fn parse_hue(tok: &str) -> Option<f32> {`

```
/// A hue token: bare number or `<n>deg`, wrapped into `[0, 360)`.
```

## L358 · `fn unit_pct(tok: &str) -> Option<f32> {`

```
/// A required percentage (`s`/`l`) → unit interval `[0, 1]`.
```

## L366 · `fn clamp(x: f32, lo: f32, hi: f32) -> f32 {`

```
// ── float helpers (no libm) ──────────────────────────────────────────────
```

## L376 · `fn round_u8(f: f32) -> u8 {`

```
/// Round a non-negative-ish float to a `u8`, clamping to `0..=255`.
```

## L378-379 · `(clamp(f, 0.0, 255.0) + 0.5) as u8`

```
// Clamp first, then round-half-up; the saturating f32→u8 cast handles the
// 255.5 edge (truncates to 255). No `round()` (std-only) needed.
```

## L383 · `fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {`

```
/// HSL → RGB (CSS Color 4 algorithm). `h` in `[0,360)`, `s`/`l` in `[0,1]`.
```

## L386 · `let hp = h / 60.0; // in [0, 6)`

```
// in [0, 6)
```

## L395 · `_ => (c, 0.0, x), // 5 (and clamp for any float edge)`

```
// 5 (and clamp for any float edge)
```

## L404-409 · `fn split_slash(inner: &str) -> &str {`

```
// ── CSS Color 4: hwb / lab / lch / oklab / oklch / color() ─────────────────
//
// Each functional colour is converted to linear sRGB, gamma-encoded, and
// simple-clipped into gamut. f32 throughout — the oracle's ±20/255 tolerance
// is far looser than f32 rounding. Matrices/constants are the CSS Color 4
// "Sample code for color conversions" values (drafts.csswg.org/css-color-4).
```

## L411 · `fn split_slash(inner: &str) -> &str {`

```
/// Split off an optional `/ alpha` tail, keeping the channels.
```

## L419 · `fn slash_alpha(inner: &str) -> u8 {`

```
/// The other half of [`split_slash`]: the `/ alpha` tail, absent = opaque.
```

## L427 · `fn parse_hwb(inner: &str) -> Option<Rgb> {`

```
/// `hwb(H W B)` — hue + whiteness + blackness. W/B as `%` or fraction.
```

## L440 · `let base = hsl_to_rgb(h, 1.0, 0.5);`

```
// Tint/shade a pure-hue colour: c·(1−W−B) + W.
```

## L446 · `fn parse_lab(inner: &str, ok: bool) -> Option<Rgb> {`

```
/// `lab(L a b)` / `oklab(L a b)`.
```

## L458 · `fn parse_lch(inner: &str, ok: bool) -> Option<Rgb> {`

```
/// `lch(L C H)` / `oklch(L C H)` — polar form of lab/oklab.
```

## L472 · `fn parse_color_fn(inner: &str) -> Option<Rgb> {`

```
/// `color(<space> c1 c2 c3 [/ a])` — predefined + xyz colour spaces.
```

## L480 · `"srgb" => return Some(gamma_to_rgb(c)),`

```
// `color(srgb …)` gives display (gamma) values directly.
```

## L486-488 · `"display-p3-linear" => xyz_to_lin_srgb(mat(&P3_TO_XYZ_D65, c)),`

```
// The same primaries with no transfer function — the channels are
// already linear, so the only difference from `display-p3` is that
// there is no gamma to undo.
```

## L508 · `fn frac(tok: &str) -> Option<f32> {`

```
// — channel parsers —
```

## L510 · `fn frac(tok: &str) -> Option<f32> {`

```
/// `%`→fraction, bare number as-is, `none`→0.
```

## L521 · `fn num_or_pct(tok: &str) -> Option<f32> {`

```
/// `color()` channel: `%`→fraction (100%=1.0), bare number as-is, `none`→0.
```

## L526-531 · `fn lab_l(tok: &str, ok: bool) -> Option<f32> {`

```
/// lab/oklab lightness. lab: `%`=0..100 or number; oklab: `%`=0..1 or number.
///
/// CLAMPED, which is not the same as clipping the result: css-color-4 §9.2
/// makes lightness itself range-limited, so `lab(150 …)` IS `lab(100 …)` and
/// the two must come out byte-identical. Letting 150 through fed a lightness
/// no colour has into the conversion and landed somewhere else entirely.
```

## L544 · `fn lab_ab(tok: &str, ok: bool) -> Option<f32> {`

```
/// lab/oklab a/b axis. `%` reference: lab ±125, oklab ±0.4.
```

## L555 · `fn lch_c(tok: &str, ok: bool) -> Option<f32> {`

```
/// lch/oklch chroma (≥0). `%` reference: lch 150, oklch 0.4.
```

## L567 · `fn lab_to_rgb(l: f32, a: f32, b: f32) -> Rgb {`

```
// — colour-space conversions —
```

## L578 · `let xyz_d50 = [xr * 0.9642956, yr, zr * 0.8251046];`

```
// Scale by the D50 whitepoint, adapt to D65, project to linear sRGB.
```

## L596 · `fn mat(m: &[f32; 9], v: [f32; 3]) -> [f32; 3] {`

```
/// 3×3 (row-major) × vec3.
```

## L612 · `fn lin_srgb_to_rgb(lin: [f32; 3]) -> Rgb {`

```
/// Linear sRGB → gamma sRGB → clipped `Rgb`.
```

## L621 · `fn gamma_to_rgb(c: [f32; 3]) -> Rgb {`

```
/// Already-gamma sRGB fractions → clipped `Rgb`.
```

## L630 · `fn srgb_gamma(c: f32) -> f32 {`

```
// — transfer functions (gamma ↔ linear) —
```

## L655 · `static XYZ_D65_TO_LIN_SRGB: [f32; 9] = [`

```
// — matrices (linear space → XYZ, and XYZ → linear sRGB) —
```

## L688 · `fn named_color(name: &str) -> Option<Rgb> {`

```
// ── named colours ────────────────────────────────────────────────────────
```

## L704-707 · `static SYSTEM: &[(&str, u8, u8, u8)] = &[`

```
/// CSS system colours (CSS Color 4 §system-colors) with light-theme values.
/// Deprecated keywords are aliased to their modern equivalent's value so the
/// WPT "deprecated-sameas" reftests (which compare a deprecated colour to its
/// modern target) render identically. Names are pre-lowercased for lookup.
```

## L709 · `("canvas", 255, 255, 255),`

```
// modern
```

## L729 · `("activeborder", 118, 118, 118),      // ButtonBorder`

```
// deprecated → aliased to a modern value (must match for -sameas reftests)
```

## L730 · `("activeborder", 118, 118, 118),      // ButtonBorder`

```
// ButtonBorder
```

## L731 · `("activecaption", 255, 255, 255),     // Canvas`

```
// Canvas
```

## L732 · `("appworkspace", 255, 255, 255),      // Canvas`

```
// Canvas
```

## L733 · `("background", 255, 255, 255),        // Canvas`

```
// Canvas
```

## L734 · `("buttonhighlight", 240, 240, 240),   // ButtonFace`

```
// ButtonFace
```

## L735 · `("buttonshadow", 240, 240, 240),      // ButtonFace`

```
// ButtonFace
```

## L736 · `("captiontext", 0, 0, 0),             // CanvasText`

```
// CanvasText
```

## L737 · `("inactiveborder", 118, 118, 118),    // ButtonBorder`

```
// ButtonBorder
```

## L738 · `("inactivecaption", 255, 255, 255),   // Canvas`

```
// Canvas
```

## L739 · `("inactivecaptiontext", 128, 128, 128), // GrayText`

```
// GrayText
```

## L740 · `("infobackground", 255, 255, 255),    // Canvas`

```
// Canvas
```

## L741 · `("infotext", 0, 0, 0),                // CanvasText`

```
// CanvasText
```

## L742 · `("menu", 255, 255, 255),              // Canvas`

```
// Canvas
```

## L743 · `("menutext", 0, 0, 0),                // CanvasText`

```
// CanvasText
```

## L744 · `("scrollbar", 255, 255, 255),         // Canvas`

```
// Canvas
```

## L745 · `("threeddarkshadow", 118, 118, 118),  // ButtonBorder`

```
// ButtonBorder
```

## L746 · `("threedface", 240, 240, 240),        // ButtonFace`

```
// ButtonFace
```

## L747 · `("threedhighlight", 118, 118, 118),   // ButtonBorder`

```
// ButtonBorder
```

## L748 · `("threedlightshadow", 118, 118, 118), // ButtonBorder`

```
// ButtonBorder
```

## L749 · `("threedshadow", 118, 118, 118),      // ButtonBorder`

```
// ButtonBorder
```

## L750 · `("window", 255, 255, 255),            // Canvas`

```
// Canvas
```

## L751 · `("windowframe", 118, 118, 118),       // ButtonBorder`

```
// ButtonBorder
```

## L752 · `("windowtext", 0, 0, 0),              // CanvasText`

```
// CanvasText
```

## L755-758 · `static NAMED: &[(&str, u8, u8, u8)] = &[`

```
/// The full CSS Color Module Level 4 extended colour keywords (148 entries,
/// incl. the `aqua`/`cyan`, `fuchsia`/`magenta`, `gray`/`grey` synonyms and
/// `rebeccapurple`). `transparent`/`currentcolor`/`inherit` are intentionally
/// absent → they resolve to `None` (keep inherited).
```

## L910 · `#[cfg(test)]`

```
// ── tests ────────────────────────────────────────────────────────────────
```

## L916-918 · `fn parse_color(v: &str) -> Option<Rgb> {`

```
/// Most assertions below predate alpha and are about the CHANNELS; keep
/// them speaking plain `Rgb` rather than restating `Rgba::opaque` 80 times.
/// The alpha-specific tests call `parse_color_val` and see it.
```

## L923-925 · `#[test]`

```
/// `currentcolor` is a VALUE, not a failure to parse — the difference is
/// what lets a background follow the element's own text colour instead of
/// keeping whatever the previous declaration said.
```

## L930 · `assert_eq!(parse_color("currentcolor"), None);`

```
// A caller that cannot defer still reads it as "keep what you had".
```

## L934-936 · `#[test]`

```
/// Relative syntax with the IDENTITY channel list is its origin, in every
/// colour space — including when the origin is `currentcolor`, whose
/// deferral has to survive the trip.
```

## L953 · `assert_eq!(parse_color("rgb(from red r g b)"), Some(Rgb(255, 0, 0)));`

```
// A concrete origin resolves right here — nothing to defer.
```

## L956-957 · `for v in [`

```
// Anything but the identity is left alone rather than painted as its
// origin: a swapped channel, a substituted one, a numeric alpha.
```

## L969-970 · `#[test]`

```
/// Lightness is range-limited by the spec, so an over-range one is the
/// same colour as the limit — not a different one that happens to clip.
```

## L978-979 · `#[test]`

```
/// `display-p3-linear` is `display-p3`'s primaries with no transfer
/// function — sRGB green is 0.0383 0.2087 0.0156 there.
```

## L997 · `assert_eq!(parse_color("#f008"), Some(Rgb(255, 0, 0)));`

```
// #rgba — alpha nibble parsed and discarded, opaque RGB returned.
```

## L1018 · `assert_eq!(parse_color("#12345"), None); // length 5 invalid`

```
// length 5 invalid
```

## L1032 · `assert_eq!(parse_color("rgb(50%,50%,50%)"), Some(Rgb(128, 128, 128)));`

```
// 50% of 255 = 127.5 → rounds to 128 (matches browsers).
```

## L1052-1053 · `for v in [`

```
// Alpha 0 is not a shade of a colour, it is the absence of one. Every
// syntax that can carry it must arrive at the same answer.
```

## L1068-1069 · `assert_eq!(parse_color(v), None, "{v}");`

```
// The opaque-only view keeps the inherited value rather than
// painting the alpha-0 carrier colour.
```

## L1074-1078 · `#[test]`

```
/// A partial alpha is carried, not flattened and not dropped. All three
/// spellings of "1 % red" mean the same colour, and the rasteriser is what
/// finally composites it — duckduckgo.com draws its searchbox outline as
/// `rgba(0,0,0,.08)`, which painted opaque is a hard black rule where the
/// page asked for a hairline you can barely see.
```

## L1087 · `assert_eq!(parse_color_val("#f00"), Some(ColorVal::Rgb(Rgba::opaque(Rgb(255, 0, 0)))));`

```
// …and a colour written without one is still fully opaque.
```

## L1093 · `assert_eq!(`

```
// We never make content disappear on a guess.
```

## L1105 · `assert_eq!(parse_color("hsl(0, 0%, 50%)"), Some(Rgb(128, 128, 128)));`

```
// achromatic: any hue, 0 saturation → grey by lightness.
```

## L1115 · `assert_eq!(parse_color("hsl(480, 100%, 50%)"), Some(Rgb(0, 255, 0)));`

```
// 480 wraps to 120.
```

## L1127 · `assert_eq!(parse_color("hsl(180, 100%, 50%)"), Some(Rgb(0, 255, 255)));`

```
// cyan / magenta / yellow at full sat, mid lightness.
```

## L1159 · `assert_eq!(parse_color("rgb(1,2)"), None); // too few channels`

```
// too few channels
```

## L1160 · `assert_eq!(parse_color("rgb(1,2,3,4,5)"), None); // too many`

```
// too many
```

## L1161 · `assert_eq!(parse_color("hsl(0, 100, 50)"), None); // s/l need %`

```
// s/l need %
```

## L1171 · `fn close(got: Rgb, want: Rgb, tol: i32) {`

```
// — CSS Color 4 —
```

## L1189 · `close(parse_color("lab(46.2775% -47.5621 48.5837)").unwrap(), Rgb(0, 128, 0), 3);`

```
// CSS Color 4 sample: lab(46.2775% -47.5621 48.5837) ≈ #008000.
```

## L1196 · `close(parse_color("oklch(0.628 0.2577 29.23)").unwrap(), Rgb(255, 0, 0), 4);`

```
// oklch red ≈ #ff0000.
```

## L1205 · `close(parse_color("color(display-p3 0 1 0)").unwrap(), Rgb(0, 255, 0), 6);`

```
// display-p3 green is out of sRGB gamut → clips near pure green.
```

## L1214 · `assert_eq!(parse_color("ActiveBorder"), parse_color("ButtonBorder"));`

```
// Deprecated keyword renders identically to its modern target.
```

## L1220 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
// ── filter: the colour functions (filter-effects-1 §18.1) ─────────────────
```

## L1222-1236 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// A `filter` chain reduced to ONE colour transform.
///
/// Every colour filter the spec defines — `grayscale`, `sepia`, `saturate`,
/// `hue-rotate`, `invert`, `brightness`, `contrast`, `opacity` — is given
/// there as a matrix over the RGB triple, and matrices compose. So a chain of
/// any length costs one 3x4 multiply per pixel rather than a walk over the
/// list, and `ComputedStyle` carries a fixed 52 bytes instead of a `Vec`.
///
/// `blur` and `drop-shadow` are deliberately NOT here: they MOVE pixels rather
/// than recolour them, so no matrix can express them. Neither appears in
/// `assets/bootstrap.min.css` or Wikipedia's `resolved.css` — measured, all 46
/// `filter` declarations there are `invert`, `grayscale`, `brightness` and
/// `hue-rotate` — and between them they carry two reftests. A declaration that
/// names one is dropped whole (the chain is all-or-nothing), so a page gets
/// its unfiltered pixels rather than a wrong approximation of a blur.
```

## L1239 · `pub m: [f32; 12],`

```
/// Three rows of `[r, g, b, offset]`, over channels in 0..1.
```

## L1241 · `pub a: f32,`

```
/// `opacity()` — the one filter that touches alpha instead of colour.
```

## L1249 · `pub fn then(self, next: ColorFilter) -> ColorFilter {`

```
/// `self` applied first, then `next`.
```

## L1262 · `pub fn apply(&self, c: Rgba) -> Rgba {`

```
/// The transform, on 8-bit channels. Alpha is scaled by `opacity()`.
```

## L1271-1273 · `pub fn apply_bgra(&self, px: [u8; 4]) -> [u8; 4] {`

```
/// The same transform on one BGRA source pixel, for the image paths — the
/// only place the pixels exist at all, since an image travels to the
/// display list as a key.
```

## L1280-1286 · `pub fn parse_filter(v: &str) -> Option<ColorFilter> {`

```
/// Parse a `filter` list into one composed transform.
///
/// All-or-nothing: an unknown function, or one this engine cannot express as a
/// colour matrix (`blur`, `drop-shadow`, `url()`), invalidates the whole
/// declaration — a half-applied chain would be a colour the page never asked
/// for. `none` parses to the identity, which is how a page turns an inherited
/// filter back off.
```

## L1314-1315 · `fn filter_fn(name: &str, arg: &str) -> Option<ColorFilter> {`

```
/// One filter function as a matrix. `amount` is a `<number>` or a
/// `<percentage>`; the default when omitted differs per function.
```

## L1317 · `if name == "hue-rotate" {`

```
// `hue-rotate` takes an angle, everything else a number/percentage.
```

## L1331-1332 · `let amount = if arg.is_empty() { 1.0 } else { num_or_pct_val(arg)? };`

```
// The default amount is 1 for every one of these — `invert()` with no
// argument is a full inversion.
```

## L1334-1336 · `let unit = clamp(amount, 0.0, 1.0);`

```
// `grayscale`, `sepia`, `invert` and `opacity` saturate at 1; `saturate`,
// `brightness` and `contrast` have no upper bound. Bootstrap writes
// `grayscale(100)` — a plain number, not a percentage — and means 1.
```

## L1343-1344 · `"grayscale" => saturate_matrix(1.0 - unit),`

```
// `grayscale(x)` is `saturate(1 - x)`, and `sepia` interpolates the
// same way towards its own fixed matrix.
```

## L1358 · `"invert" => diag(1.0 - 2.0 * unit, unit),`

```
// c' = a(1-c) + (1-a)c
```

## L1363 · `_ => return None,`

```
// `blur`, `drop-shadow`, `url` — see the type's note.
```

## L1379-1380 · `fn num_or_pct_val(tok: &str) -> Option<f32> {`

```
/// A filter amount: `<number>` or `<percentage>`. Unlike `unit_pct` this does
/// not clamp — `brightness(200%)` is a legal 2.
```

