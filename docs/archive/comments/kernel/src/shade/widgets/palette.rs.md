# `kernel/src/shade/widgets/palette.rs` @ 5e0102684

## L1-7 · `use core::sync::atomic::{AtomicU32, Ordering};`

```
//! Token → concrete BGRA color.
//!
//! Two curated palettes (DARK + LIGHT) with fixed surface/border/text
//! values. Only the Accent and AccentMuted tokens derive from the
//! wallpaper's extracted theme. The mode is picked from the
//! `theme` config key (`dark` | `light` | `auto`). `auto` uses the
//! wallpaper's background luminance to decide.
```

## L52-53 · `on_surface_muted: 0xFF4E5358,`

```
// Darker than they would need to be on the bare surface: text sits on
// glass over the wallpaper, which is darker than `surface` says.
```

## L61-62 · `const ACCENT_PRESETS: [(&str, u32); 4] = [`

```
/// Named accent presets from the design. `accent = auto` (the default)
/// keeps deriving the accent from the wallpaper instead.
```

## L73-81 · `struct CodeScheme {`

```
// ── Code schemes (syntax colours) ─────────────────────────────────────
//
// Values taken verbatim from VSCodium's built-in theme JSONs, resolved
// the way TextMate resolves scopes (most specific scope wins). A scheme
// supplies ONLY these nine colours — the canvas stays `Page` and plain
// text stays `OnSurface`. Importing a scheme's own background too would
// fight the glass surfaces, and a dark scheme picked under a light theme
// would then paint dark-on-white. Preference and canvas are two separate
// things; this knob only moves the preference.
```

## L85-86 · `light:    bool,`

```
/// True if the scheme was authored for a light canvas. Only used to
/// warn on an obvious mismatch — an explicit choice is still honoured.
```

## L99-100 · `const CODE_SCHEMES: [CodeScheme; 8] = [`

```
/// Every scheme `set code.scheme <name>` accepts. `auto` (the default)
/// picks `dark-plus` or `light-plus` from the active theme.
```

## L136 · `pub fn code_scheme_names() -> &'static [&'static str] {`

```
/// Scheme names, for `set code.scheme` and its error message.
```

## L145 · `pub fn code_scheme_exists(name: &str) -> bool {`

```
/// Is `name` a scheme we know? `auto` counts.
```

## L150 · `pub fn code_scheme_is_light(name: &str) -> Option<bool> {`

```
/// Does `name` target a light canvas? `None` for unknown / `auto`.
```

## L163 · `let idx = if is_light_theme() { 1 } else { 0 };`

```
// auto — follow the theme.
```

## L172-173 · `pub fn current_in(light: bool) -> Palette {`

```
/// The palette of one theme, whichever is active. Glass (loop, dock, bar)
/// is always dark; only ordinary apps follow the theme.
```

## L182 · `pub fn for_window(window_id: u32) -> Palette {`

```
/// Palette for a widget window: dock and bar are glass and stay dark.
```

## L192-194 · `pub fn chrome_opacity() -> u32 {`

```
/// Opacity (0..255) of floating chrome — the bar card and the dock tray.
/// Derived from the wallpaper (`shade::glass`), `shade.chrome_opacity`
/// overrides it.
```

## L205-207 · `static BAR_WINDOW:  AtomicU32 = AtomicU32::new(0);`

```
// The two panel windows, published by the compositor. The rasterizer runs
// while the SCENES lock is held and must not reach for the compositor
// lock, so the ids travel as plain atomics. 0 = no such panel.
```

## L219-223 · `pub fn clear_panel_opacity_overrides() -> bool {`

```
/// Drop both per-panel overrides. Called when `shade.chrome_opacity` is
/// set: the shared knob is the master, so setting it always moves both
/// panels again — otherwise a value tried once on the bar would silently
/// outrank every later shared setting, with no way back short of editing
/// the config blob. Returns true if anything was actually cleared.
```

## L230-233 · `pub fn panel_opacity(window_id: u32) -> u32 {`

```
/// Opacity for one panel window: its own knob if set, else the shared
/// `shade.chrome_opacity`, else the theme default. Lets the bar stay
/// legible while the dock reads more like glass (or the other way round).
/// Setting the shared knob clears these — see above.
```

## L249-250 · `pub fn resolve_glass(token: Token) -> u32 {`

```
/// A token as drawn on glass — loop, dock and bar are dark glass in both
/// themes (light glass over a busy wallpaper was never as legible).
```

## L278 · `Token::CodeKeyword     => code_scheme().keyword,`

```
// Code tokens come from the scheme, not the theme ramp.
```

## L297 · `if crate::theme::is_active() {`

```
// auto: wallpaper bg luminance decides. No wallpaper → dark.
```

## L307-309 · `pub fn accent_raw() -> u32 {`

```
/// The user's accent choice. `auto` (default) keeps deriving it from the
/// wallpaper; a preset name or a `#RRGGBB` literal pins it. Set live with
/// `set accent <rose|sage|blue|amber|auto|#RRGGBB>`.
```

## L331-334 · `fn accent_adjusted(surface: u32) -> u32 {`

```
/// Accent adjusted for minimum contrast against the active surface.
/// Extracted wallpaper accents can be close in luminance to the chosen
/// theme surface (e.g. mid-grey wallpaper accent + LIGHT surface both
/// bright) — we darken/lighten to keep Accent readable.
```

## L339-342 · `if surf_lum > 128 {`

```
// Light: the accent is also INK (the loop prompt, links), and it stands
// on glass over the wallpaper, not on the near-white `surface` — one
// fixed step left a pastel accent barely visible. Darken until it reads
// as text.
```

## L355 · `const LIGHT_ACCENT_MAX_LUM: u32 = 95;`

```
/// Brightest the accent may be in light mode, so it still works as text.
```

## L358-360 · `fn accent_over(surface: u32, weight: u32) -> u32 {`

```
/// Accent pre-mixed over the surface at `weight`/255. The design writes
/// these as `rgba(accent, .15/.22/.45)`; the rasterizer ignores a token's
/// alpha byte (opacity comes from `bg_alpha`), so they are flattened here.
```

## L365-369 · `fn on_accent(surface: u32, is_light: bool) -> u32 {`

```
/// Ink on an Accent fill. Light mode always takes white — `accent_adjusted`
/// has already darkened the pastel accent against the bright surface, so
/// white carries. Dark mode takes a near-black tinted with the accent hue
/// (the design's per-preset `--accent-ink`), falling back to white if the
/// accent is itself dark.
```

## L414-416 · `pub fn token_from_id(id: usize) -> Option<Token> {`

```
/// Wire id → token. The single table: `current()` fills the palette
/// through it and `npk_theme_token` answers apps through it, so an
/// appended token reaches the rasterizer and the SDK in one edit.
```

