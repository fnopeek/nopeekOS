# `kernel/src/gui/login.rs` @ 5e0102684

## L1-4 · `use alloc::format;`

```
//! Graphical login screen (Hyprlock-inspired).
//!
//! Aurora background, large centered clock, greeting, light input field
//! with centered dots. No card container — clean, minimal.
```

## L10 · `struct Layout {`

```
/// Layout computed from screen dimensions.
```

## L15 · `clock_y: u32,`

```
// Vertical positions (all centered horizontally)
```

## L31 · `let center_y = sh / 2;`

```
// Center vertically with clock above, input in middle
```

## L47 · `const BAYER4: [[u8; 4]; 4] = [`

```
/// 4x4 Bayer dithering matrix (ordered dithering to eliminate gradient banding).
```

## L55 · `fn draw_background(shadow: *mut u8, info: &FbInfo) {`

```
/// Draw the login background — smooth dark gray gradient with dithering.
```

## L67 · `fn draw_login_region(shadow: *mut u8, info: &FbInfo, rx: u32, ry: u32, rw: u32, rh: u32) {`

```
/// Redraw a region of the login gradient background.
```

## L78 · `fn login_pixel(x: u32, y: u32, h: u32) -> u32 {`

```
/// Compute a single pixel for the login gradient with dithering.
```

## L80 · `let t = y as u64 * 65536 / h.max(1) as u64;`

```
// Gradient value in fixed-point (0..65536 maps to 0x0C..0x20 = 20 levels)
```

## L82 · `let val256 = 0x0C * 256 + (t as u32 * 0x14 / 256); // range 0x0C..0x20`

```
// Base value * 256 for sub-pixel precision
```

## L83 · `let val256 = 0x0C * 256 + (t as u32 * 0x14 / 256); // range 0x0C..0x20`

```
// range 0x0C..0x20
```

## L85 · `let frac = val256 % 256; // fractional part (0..255)`

```
// fractional part (0..255)
```

## L87 · `let threshold = BAYER4[(y % 4) as usize][(x % 4) as usize] as u32 * 16;`

```
// Ordered dithering: compare fractional part against Bayer threshold
```

## L94 · `fn draw_clock(shadow: *mut u8, info: &FbInfo, l: &Layout) {`

```
/// Draw the large clock display.
```

## L99 · `.unwrap_or(2); // Default CEST (UTC+2), config overrides after auth`

```
// Default CEST (UTC+2), config overrides after auth
```

## L106 · `font::draw_clock_str_centered(shadow, info,`

```
// Large clock using dedicated clock font (32x64 or 16x32)
```

## L112 · `fn draw_greeting(shadow: *mut u8, info: &FbInfo, l: &Layout) {`

```
/// Draw the greeting text.
```

## L114 · `font::draw_str_centered(shadow, info,`

```
// Before auth we don't have the name yet, show generic greeting
```

## L120 · `fn draw_input_box(shadow: *mut u8, info: &FbInfo, l: &Layout, focused: bool) {`

```
/// Draw the input field (light background, dark outline, rounded).
```

## L122 · `let radius = l.input_h / 2; // Full pill shape (semicircle ends)`

```
// Full pill shape (semicircle ends)
```

## L125 · `let border_color = if focused { 0x00444444 } else { Theme::INPUT_OUTER };`

```
// Outer border (neutral gray, no accent color on login)
```

## L131 · `render::fill_rounded_rect_aa(shadow, info,`

```
// Inner fill (light)
```

## L138 · `fn draw_dots(shadow: *mut u8, info: &FbInfo, l: &Layout, count: usize) {`

```
/// Draw passphrase dots centered inside the input field.
```

## L141 · `let inner_r = (l.input_h - 2 * outline) / 2; // Pill shape matching draw_input_box`

```
// Pill shape matching draw_input_box
```

## L143 · `render::fill_rounded_rect_aa(shadow, info,`

```
// Clear input interior (redraw inner fill)
```

## L151 · `let dot_r = 3 * l.scale;`

```
// Small dots: radius = 3px at 1080p, 6px at 4K
```

## L153 · `let dot_gap = 6 * l.scale; // gap between dots`

```
// gap between dots
```

## L157 · `let outer_r = l.input_h / 2;`

```
// Clamp: don't let dots exceed field width
```

## L163 · `let vis_w = visible * dot_diameter + visible.saturating_sub(1) * dot_gap;`

```
// Center dots horizontally in input field
```

## L170 · `let r2 = (dot_r * dot_r) as i32;`

```
// Draw filled circle
```

## L186 · `fn draw_cursor(shadow: *mut u8, info: &FbInfo, l: &Layout, pos: usize, visible: bool) {`

```
/// Draw or hide the blinking cursor.
```

## L192 · `let cursor_x = if pos == 0 {`

```
// Cursor position: after last dot (or center if empty)
```

## L207 · `fn draw_status(shadow: *mut u8, info: &FbInfo, l: &Layout, msg: &str, color: u32) {`

```
/// Draw status message below input field.
```

## L209 · `let (_, ch) = font::char_size(l.scale);`

```
// Clear status area (redraw aurora background strip)
```

## L217-221 · `fn idle_halt() {`

```
/// HLT once and report it.
///
/// Every halt site must go through something that calls `record_halt` —
/// the per-core usage figure is `100 − halted%`, so an unreported halt
/// reads as full load (`smp::per_core`).
```

## L223-225 · `let d = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 100;`

```
// One frame at most. Before boot finishes the periodic tick ends it;
// after `lock`, Core 0 has no tick any more (stage 3e) — a bare `hlt`
// would sleep until the next key and freeze the screen meanwhile.
```

## L230-231 · `pub fn run(salt: &[u8; 16]) -> [u8; 32] {`

```
/// Run the graphical login screen.
/// Returns the 256-bit master key on success, or halts on lockout.
```

## L233-234 · `framebuffer::set_gui_mode(true);`

```
// Enable GUI mode (kprintln skips framebuffer, only serial)
// Color scheme already selected in main.rs after csprng::init()
```

## L237 · `let (screen_w, screen_h) = framebuffer::with_fb(|fb| {`

```
// Read screen dimensions
```

## L246 · `framebuffer::with_fb(|fb| {`

```
// Initial full draw
```

## L255 · `let hz = crate::gpu::current_hz();`

```
// Debug: show resolution + refresh rate in bottom-right corner
```

## L269 · `let mut damage = render::DamageTracker::new(info.width, info.height);`

```
// Full blit
```

## L275 · `let mut passphrase = [0u8; 128];`

```
// Input loop
```

## L284 · `let now = crate::interrupts::ticks();`

```
// Update clock every 10s (check if minute changed)
```

## L291 · `let (_, ch) = font::clock_char_size(layout.scale);`

```
// Clear clock area with aurora background
```

## L299 · `if now.wrapping_sub(last_cursor_toggle) >= 50 {`

```
// Cursor blink (toggle every 50 ticks = 500ms at 100Hz)
```

## L313 · `crate::net::poll();`

```
// Poll network while waiting
```

## L316 · `if let Some(key) = crate::keyboard::read_key() {`

```
// Poll keyboard (non-blocking)
```

## L322 · `framebuffer::with_fb(|fb| {`

```
// Show verifying state
```

## L332 · `let key = crate::crypto::derive_master_key(&passphrase[..pos], salt);`

```
// Derive key (OUTSIDE fb lock)
```

## L341 · `crate::config::load();`

```
// Success!
```

## L359 · `let start = crate::interrupts::ticks();`

```
// Quick flash of welcome message (0.5s)
```

## L362 · `idle_halt();`

```
// SAFETY: ring-0, IRQs enabled, APIC timer ticks us.
```

## L366 · `framebuffer::set_gui_mode(false);`

```
// Exit GUI mode, clear screen for loop
```

## L372 · `crate::crypto::clear_master_key();`

```
// Wrong passphrase
```

## L393 · `framebuffer::with_fb(|fb| {`

```
// Change input border to fail color
```

## L397 · `let radius = layout.input_h / 2;`

```
// Redraw input with fail color border (pill shape)
```

## L416 · `let start = crate::interrupts::ticks();`

```
// Wait for backoff
```

## L431 · `idle_halt();`

```
// SAFETY: ring-0, IRQs enabled; APIC timer ticks us each 10ms.
```

## L435 · `framebuffer::with_fb(|fb| {`

```
// Reset input field
```

## L455 · `if pos > 0 {`

```
// Backspace
```

## L474 · `if pos < passphrase.len() {`

```
// Printable character
```

## L495-498 · `idle_halt();`

```
// No key — halt until next IRQ. APIC timer at 100Hz wakes us for
// cursor blink / clock updates; USB is IRQ-driven (APIC timer drains
// xHCI into SPSC ring), so keys wake us too.
// SAFETY: ring-0 idle with interrupts enabled.
```

