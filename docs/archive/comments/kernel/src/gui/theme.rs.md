# `kernel/src/gui/theme.rs` @ 5e0102684

## L1-4 · `use core::sync::atomic::{AtomicBool, Ordering};`

```
//! Theme system — 16-color palette derived from wallpaper or aurora.
//!
//! Colors are extracted via Median-Cut quantization from raw pixel data.
//! The palette drives border gradients, shadebar, accent colors, etc.
```

## L8-9 · `static mut PALETTE: [u32; 16] = [0; 16];`

```
/// 16-color theme palette (pywal-compatible ordering).
/// color0 = darkest (background), color1..7 = dominant, color8..15 = bright variants.
```

## L12 · `static mut BORDER_GRADIENT: (u32, u32) = (0, 0);`

```
/// Gradient border colors: start and end color for 45° linear gradient.
```

## L15 · `static THEME_ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// Whether a custom theme is active (vs. aurora default).
```

## L19 · `pub fn set_palette(colors: &[u32; 16]) {`

```
/// Set the full 16-color palette and derive border gradient.
```

## L21 · `unsafe {`

```
// SAFETY: single-core
```

## L24-25 · `BORDER_GRADIENT = (ensure_bright(colors[1]), ensure_bright(colors[2]));`

```
// Border gradient: color1 → color2 (like Hyprland pywal setup)
// Ensure minimum brightness for visibility on dark backgrounds
```

## L31 · `fn ensure_bright(color: u32) -> u32 {`

```
/// Ensure a color has minimum luminance for readability.
```

## L48 · `pub fn palette() -> [u32; 16] {`

```
/// Get the full palette.
```

## L50 · `unsafe { PALETTE }`

```
// SAFETY: single-core
```

## L54 · `pub fn is_active() -> bool {`

```
/// Whether a custom theme is active.
```

## L59 · `pub fn border_gradient() -> (u32, u32) {`

```
/// Get border gradient (start_color, end_color).
```

## L61 · `unsafe { BORDER_GRADIENT }`

```
// SAFETY: single-core
```

## L65 · `pub fn bg_color() -> u32 {`

```
/// Get background color (color0 — darkest).
```

## L70 · `pub fn accent() -> u32 {`

```
/// Get accent color (color1 — primary dominant).
```

## L75 · `pub fn inactive_border() -> u32 {`

```
/// Get inactive border color (color0 with some brightness).
```

## L77 · `unsafe { PALETTE[8] } // bright variant of bg`

```
// bright variant of bg
```

## L80 · `pub fn clear() {`

```
/// Clear the custom theme, revert to aurora defaults.
```

## L85 · `pub fn lerp_color(a: u32, b: u32, t: u32) -> u32 {`

```
/// Interpolate between two colors. t = 0..1000 (0 = a, 1000 = b).
```

## L95 · `pub fn extract_palette(pixels: &[u8], pixel_count: usize) -> [u32; 16] {`

```
// --- Median-Cut Color Extraction ---
```

## L97-98 · `pub fn extract_palette(pixels: &[u8], pixel_count: usize) -> [u32; 16] {`

```
/// Extract a 16-color palette from raw BGRA pixel data.
/// Uses Median-Cut quantization (same algorithm as pywal).
```

## L100 · `let mut samples = alloc::vec::Vec::new();`

```
// Sample pixels (skip transparent, near-black, near-white)
```

## L102 · `let step = (pixel_count / 8192).max(1); // Sample ~8k pixels max`

```
// Sample ~8k pixels max
```

## L103-105 · `for i in (0..pixel_count).step_by(step) {`

```
// Overall brightness = mean luminance over ALL sampled pixels, including
// the near-black / near-white ones the theming pass skips — that is the
// wallpaper's true perceived brightness, which the glass tint scales on.
```

## L113 · `if lum < 15 || lum > 240 { continue; }`

```
// Skip near-black and near-white (not useful for theming)
```

## L121 · `let mut buckets: alloc::vec::Vec<alloc::vec::Vec<(u32, u32, u32)>> = alloc::vec![samples];`

```
// Median-Cut: split into 16 buckets
```

## L125 · `let mut best_idx = 0;`

```
// Find bucket with largest color range
```

## L138 · `let bucket = buckets.remove(best_idx);`

```
// Split along channel with largest range
```

## L145 · `let mut colors = [0u32; 16];`

```
// Average each bucket to get palette color
```

## L152 · `colors[..buckets.len().min(16)].sort_by_key(|c| luminance(*c));`

```
// Sort by luminance (darkest first, like pywal)
```

## L155 · `let count = buckets.len().min(16);`

```
// Fill remaining slots with brightened variants
```

## L177 · `let (mut rmin, mut rmax) = (255u32, 0u32);`

```
// Find which channel has the largest range
```

