# `kernel/src/gui/font.rs` @ 5e0102684

## L1-6 · `use crate::framebuffer::FbInfo;`

```
//! Font rendering using Spleen bitmap fonts.
//!
//! Auto-selects font size based on screen resolution:
//!   8x16  for <= 1920px width
//!   16x32 for > 1920px width
//! Clock uses 32x64 (or 16x32 at 1080p).
```

## L12 · `struct FontDesc {`

```
/// Font descriptor for a specific size.
```

## L35-37 · `pub fn scale_for(screen_width: u32) -> u32 {`

```
/// Auto-detect scale: 2x above 2560px width, else 1x.
/// 2560x1440 is a normal-DPI desktop mode, not HiDPI — scaling it would
/// leave 1280x720 of usable area. Only 4K-class panels get 2x.
```

## L42 · `fn font_for_scale(scale: u32) -> &'static FontDesc {`

```
/// Get the appropriate font for a given scale.
```

## L47 · `fn clock_font(scale: u32) -> &'static FontDesc {`

```
/// Get the large clock font.
```

## L52 · `pub fn char_size(scale: u32) -> (u32, u32) {`

```
/// Character cell size for UI text at given scale.
```

## L58 · `pub fn clock_char_size(scale: u32) -> (u32, u32) {`

```
/// Character cell size for large clock text.
```

## L64 · `fn draw_char_with(shadow: *mut u8, info: &FbInfo, font: &FontDesc,`

```
/// Draw one character using a specific font descriptor.
```

## L92 · `#[allow(dead_code)]`

```
/// Draw a single character at pixel position (UI scale).
```

## L100 · `pub fn draw_str(shadow: *mut u8, info: &FbInfo,`

```
/// Draw a string. Returns the X position after the last character.
```

## L115 · `pub fn measure_str(s: &str, scale: u32) -> u32 {`

```
/// Measure string width in pixels.
```

## L122 · `pub fn draw_str_centered(shadow: *mut u8, info: &FbInfo,`

```
/// Draw a string centered horizontally within a given region.
```

## L135 · `pub fn draw_clock_str_centered(shadow: *mut u8, info: &FbInfo,`

```
/// Draw a large clock string (uses the bigger font).
```

