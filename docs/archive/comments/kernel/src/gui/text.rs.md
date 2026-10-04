# `kernel/src/gui/text.rs` @ 5e0102684

## L1-19 · `#![allow(dead_code)]`

```
//! Inter Variable text rendering + metrics (Phase 10).
//!
//! Owns the system UI font — `Inter Variable` — loaded at boot, BLAKE3-
//! verified against a frozen hash, parsed via `fontdue`. Provides real
//! metrics (advance width, line height, ascent/descent, x-height, cap-
//! height) so the widget layout engine can measure text without
//! rasterizing glyphs.
//!
//! Inter Variable ships weights 100–900 in one file. fontdue v0.9 reads
//! the default instance (weight 400); weight-axis switching per
//! TextStyle is a v2 task (needs ttf-parser + custom outline extraction
//! or rustybuzz). All metrics returned here reflect the default weight
//! but use the real font's `hhea` / `OS/2` tables — no hardcoded values.
//!
//! Glyph atlas is heap-backed in P10.1 (HashMap); P10.4 migrates it into
//! the GGTT glyph region (see `gpu/ggtt_layout.rs`).
//!
//! P10.1 scope: loader + metrics + scaffold cache. P10.5 wires it into
//! `CpuRasterizer`.
```

## L33 · `const FONT_FS_PATH: &str = "sys/fonts/inter-variable";`

```
// ── Font source ───────────────────────────────────────────────────────
```

## L35-40 · `const FONT_FS_PATH: &str = "sys/fonts/inter-variable";`

```
/// npkFS path of the system UI font — Inter Variable v4.1 (OFL).
///
/// Seeded on fresh install by `install::bundled_assets::bootstrap_into_npkfs`,
/// thereafter updatable via the OTA path (`intent::install`). The kernel
/// binary itself does **not** embed the font — this keeps normal kernel
/// releases small and makes font updates free of kernel rebuilds.
```

## L43-48 · `const INTER_VARIABLE_BLAKE3: &str =`

```
/// Frozen BLAKE3 digest of the expected font bytes. Checked at load time
/// — a mismatch means the on-disk font was tampered or replaced with an
/// unexpected version; loading refuses.
///
/// Recompute after a font update with `b3sum sys/fonts/inter-variable.ttf`
/// and ship the new hash alongside the new font in a coordinated release.
```

## L52-57 · `const MONO_FS_PATH: &str = "sys/fonts/ibm-plex-mono";`

```
/// IBM Plex Mono Regular — the face behind `TextStyle::Mono`. Inter is
/// proportional, so before this landed every "mono" run (clock digits,
/// file-size columns, source code) was drawn with variable advances and
/// nothing lined up. Shipped UNMODIFIED: the OFL reserves the font name
/// "Plex" for unmodified versions, and subsetting would count as a
/// modification, forcing a rename. See sys/fonts/LICENSE-IBM-Plex.txt.
```

## L62 · `pub const fn is_mono(style: TextStyle) -> bool {`

```
/// Which face a style is drawn in. Only `Mono` takes the monospace face.
```

## L67 · `#[derive(Clone, Copy, Debug)]`

```
// ── TextStyle → (size, weight) mapping ────────────────────────────────
```

## L69-75 · `#[derive(Clone, Copy, Debug)]`

```
/// Logical pixel size + OpenType weight for a TextStyle. Frozen per
/// docs/archive/PHASE10_WIDGETS.md "Typography" table.
///
/// NOTE: fontdue v0.9 renders the default weight (~400) regardless of
/// the `weight` field — variable-axis switching is deferred to v2.
/// Returned metrics still use the real font (Inter), so layout is
/// correct; only visual weight differentiation is temporarily missing.
```

## L84 · `TextStyle::Title   => StyleDesc { size_px: 22, weight: 600 },`

```
// Per the Typography table in docs/archive/PHASE10_WIDGETS.md.
```

## L88 · `TextStyle::Muted   => StyleDesc { size_px: 13, weight: 400 }, // body + 60% alpha at raster`

```
// body + 60% alpha at raster
```

## L94 · `static FONT: Mutex<Option<Font>> = Mutex::new(None);`

```
// ── Global font instance ──────────────────────────────────────────────
```

## L97-99 · `static MONO: Mutex<Option<Font>> = Mutex::new(None);`

```
/// The monospace face. `None` until `init` loads it; metrics then fall
/// back to the proportional font so a missing mono file degrades to the
/// pre-0.231 look instead of blank text.
```

## L103-105 · `#[derive(Clone, Copy, PartialEq, Eq, Hash)]`

```
/// Glyph cache key: (glyph index, pixel size, weight).
/// `weight` is stored for the v2 variable-axis path; v1 ignores it
/// (fontdue renders default weight) but the key shape is stable.
```

## L111-113 · `pub mono:    bool,`

```
/// Which face the glyph id belongs to. Without it, Inter's glyph 42
/// and Plex Mono's glyph 42 collide in the cache and text renders as
/// the wrong letters.
```

## L117-124 · `pub struct CachedGlyph {`

```
/// Rasterized glyph — alpha bitmap + metrics.
///
/// P10.4: each cached glyph reserves a slot in the GGTT CompSmall4K
/// bucket via the slab allocator. `ggtt_offset` is the slot's address;
/// the alpha bitmap stays heap-resident for now (the CPU rasterizer in
/// P10.5 reads from the heap copy). The GGTT slot is an address
/// reservation so later phases can upload bytes there without
/// re-keying the cache. LRU eviction happens on the slab side.
```

## L137 · `pub fn init() {`

```
// ── Init ──────────────────────────────────────────────────────────────
```

## L139-145 · `pub fn init() {`

```
/// Load Inter Variable from npkFS. Call after npkfs::mount has succeeded
/// (post-login) and before shade widget pipeline starts raster passes.
///
/// Degrades gracefully — if the font is missing or hash mismatches, logs
/// and returns without installing a font. Widget-side metrics fall back
/// to conservative defaults and rasterization becomes a no-op. The rest
/// of the system (login screen, terminals using Spleen) is unaffected.
```

## L152 · `let bytes = match crate::npkfs::fetch(FONT_FS_PATH) {`

```
// 1. Fetch the font bytes from npkFS.
```

## L164 · `let hex = blake3::hash(&bytes).to_hex();`

```
// 2. BLAKE3 verify against the frozen digest.
```

## L174 · `let settings = FontSettings {`

```
// 3. Parse via fontdue.
```

## L190 · `let body = style_desc(TextStyle::Body);`

```
// 4. Sanity-log a few metrics so we know the load worked end-to-end.
```

## L209-211 · `match crate::npkfs::fetch(MONO_FS_PATH) {`

```
// Monospace face — optional. A miss logs and leaves MONO empty; every
// metric path then falls back to the proportional font, so the UI
// still renders (just without aligned columns).
```

## L241-242 · `pub fn is_ready() -> bool {`

```
/// True once `init` has stored a valid Font. Checked by metrics callers
/// to fail-safe before boot completes.
```

## L247 · `pub fn advance_width(ch: char, style: TextStyle) -> f32 {`

```
// ── Metrics API ───────────────────────────────────────────────────────
```

## L249-250 · `pub fn advance_width(ch: char, style: TextStyle) -> f32 {`

```
/// Horizontal advance of a character in logical pixels (1× HiDPI scale).
/// Returns 0.0 if font not yet loaded or glyph missing.
```

## L256-257 · `pub fn kern(left: char, right: char, style: TextStyle) -> f32 {`

```
/// Pair kerning correction (left, right) in logical pixels.
/// 0.0 if no kerning pair defined or font not loaded.
```

## L262 · `pub fn kern_px(left: char, right: char, style: TextStyle, size_px: u16) -> f32 {`

```
/// `kern` at an explicit pixel size (`Modifier::FontSize`).
```

## L269 · `pub fn measure(s: &str, style: TextStyle) -> f32 {`

```
/// Measure a string's total advance width (logical px), with kerning.
```

## L274 · `pub fn measure_px(s: &str, style: TextStyle, size_px: u16) -> f32 {`

```
/// `measure` at an explicit pixel size (`Modifier::FontSize`).
```

## L293 · `pub fn line_height(style: TextStyle) -> f32 {`

```
/// Line height (ascent − descent + line_gap) in logical pixels.
```

## L298 · `pub fn line_height_px(style: TextStyle, size_px: u16) -> f32 {`

```
/// `line_height` at an explicit pixel size (`Modifier::FontSize`).
```

## L302 · `.unwrap_or(size_px as f32 * 1.2) // conservative fallback`

```
// conservative fallback
```

## L305 · `pub fn ascent(style: TextStyle) -> f32 {`

```
/// Ascent (baseline → top), always positive.
```

## L310 · `pub fn ascent_px(style: TextStyle, size_px: u16) -> f32 {`

```
/// `ascent` at an explicit pixel size (`Modifier::FontSize`).
```

## L317 · `pub fn descent(style: TextStyle) -> f32 {`

```
/// Descent (baseline → bottom). Conventionally negative.
```

## L325-328 · `pub fn cap_height(style: TextStyle) -> f32 {`

```
/// Cap height — approximated from 'H' glyph height. Inter's OS/2 table
/// has a precise value, but fontdue v0.9 doesn't surface it; glyph-
/// derived is within 1 px. Used for baseline alignment between Title
/// and Body in mixed rows.
```

## L335 · `pub fn x_height(style: TextStyle) -> f32 {`

```
/// X-height — approximated from 'x' glyph height.
```

## L342 · `pub fn rasterize(ch: char, style: TextStyle) -> (Metrics, Vec<u8>) {`

```
// ── Rasterization (used by CpuRasterizer in P10.5) ────────────────────
```

## L344-345 · `pub fn rasterize(ch: char, style: TextStyle) -> (Metrics, Vec<u8>) {`

```
/// Rasterize a glyph. Returns (metrics, alpha bitmap — 1 byte/px).
/// Falls back to zero-size bitmap if font not loaded.
```

## L353-354 · `pub fn rasterize_cached<F, R>(ch: char, style: TextStyle, f: F) -> Option<R>`

```
/// Cached variant of `rasterize`. P10.4 replaces the heap Vec with a
/// GGTT offset; the API stays stable.
```

## L362-364 · `pub fn rasterize_cached_px<F, R>(ch: char, style: TextStyle, size_px: u16, f: F) -> Option<R>`

```
/// `rasterize_cached` at an explicit pixel size (`Modifier::FontSize`).
/// The size is part of the cache key, so an override costs one cache
/// generation, not a re-raster per frame.
```

## L372-373 · `let (font, is_mono_face) = match (is_mono(style), mono_guard.as_ref()) {`

```
// Mono styles take the monospace face when it loaded, else the
// proportional one — same fallback as `with_font_for`.
```

## L387-389 · `let ggtt_offset = crate::gpu::ggtt_slab::alloc(`

```
// Reserve a GGTT slot for the alpha bitmap. CompSmall4K fits
// every glyph we care about (UI text at 11–24 px, max ~32×32
// = 1 KB). Slab handles LRU eviction if the bucket fills up.
```

## L406-407 · `if let Some(cg) = cache.get(&key) {`

```
// Keep warm glyphs alive in LRU. Cheap — linear on the
// bucket's VecDeque but hit rate is high on typical text.
```

## L410 · `let kind = crate::gpu::ggtt_layout::BucketKind::CompSmall4K;`

```
// Rebuild a SlotId from the offset for the LRU touch.
```

## L426 · `pub fn cache_len() -> usize {`

```
/// Current glyph-cache occupancy (for debug + eviction planning in P10.4).
```

## L431 · `fn with_font_for<F, R>(style: TextStyle, f: F) -> Option<R>`

```
// ── Internal ──────────────────────────────────────────────────────────
```

## L433-434 · `fn with_font_for<F, R>(style: TextStyle, f: F) -> Option<R>`

```
/// Run `f` with the face this style is drawn in, or `None` if it isn't
/// loaded. A missing mono face falls back to the proportional one.
```

