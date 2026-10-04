# `kernel/src/drivers/framebuffer.rs` @ 5e0102684

## L1-4 · `use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};`

```
//! Framebuffer Console
//!
//! Pixel-based text output using framebuffer from GPU subsystem.
//! Replaces VGA text mode for UEFI systems without legacy VGA.
```

## L9 · `#[derive(Clone, Copy)]`

```
/// Framebuffer info parsed from Multiboot2.
```

## L21 · `shadow: *mut u8,`

```
/// Shadow buffer A in RAM (double-buffer: one of the two render targets)
```

## L23 · `shadow_b: *mut u8,`

```
/// Shadow buffer B in RAM (double-buffer: the other render target)
```

## L26-27 · `front: usize,`

```
/// Which buffer is front (0 = shadow/A, 1 = shadow_b/B).
/// Front is displayed (blit source + cursor restore). Back is render target.
```

## L33 · `scale: u32,`

```
/// Pixel scale factor (2 for 4K, 1 otherwise). Each glyph pixel becomes scale×scale.
```

## L35 · `pub shadow_a_ggtt: u32,`

```
/// GGTT offset of shadow A (0 = not mapped for GPU blit)
```

## L37 · `pub shadow_b_ggtt: u32,`

```
/// GGTT offset of shadow B (0 = not mapped for GPU blit)
```

## L41-42 · `unsafe impl Send for FbConsole {}`

```
// SAFETY: FbConsole is only accessed under a spinlock (CONSOLE mutex).
// The shadow pointers are heap-allocated and exclusively owned.
```

## L45-46 · `static SHADOW_FRONT_CACHED: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());`

```
/// Cached front-buffer pointer for lock-free cursor drawing.
/// Updated on swap_buffers() and init. Cursor reads this without CONSOLE lock.
```

## L49 · `pub fn cached_shadow_front() -> *mut u8 {`

```
/// Read cached front-buffer pointer (lock-free, for cursor overlay).
```

## L59 · `pub fn front_ptr(&self) -> *mut u8 {`

```
/// Front buffer pointer (blit source, cursor restore, GPU-composited cursor).
```

## L64 · `pub fn shadow_back(&self) -> *mut u8 {`

```
/// Back buffer pointer (render target for full redraws).
```

## L69 · `pub fn shadow_ptr(&self) -> (*mut u8, usize) {`

```
/// Front buffer pointer + size (for blitting and partial updates).
```

## L74-76 · `pub fn swap_buffers(&mut self) {`

```
/// Swap front ↔ back. Call after rendering to back is complete.
/// Does NOT update the cached cursor pointer — call commit_front() after blit.
/// This ensures lock-free cursor reads from the buffer that matches MMIO.
```

## L81-82 · `pub fn commit_front(&self) {`

```
/// Update cached front pointer for lock-free cursor. Call AFTER blit to MMIO.
/// Until this is called, lock-free cursor reads from the old front (matching MMIO).
```

## L87 · `pub fn shadow_phys_info(&self) -> (u64, u64, u32) {`

```
/// Physical addresses + page count of shadow buffers (identity-mapped).
```

## L92 · `pub fn front_ggtt(&self) -> u32 {`

```
/// GGTT offset of the current front buffer (for GPU blit source).
```

## L98 · `pub fn with_fb<F, R>(f: F) -> Option<R>`

```
/// Execute a closure with exclusive access to the framebuffer console.
```

## L109-110 · `static GUI_MODE: AtomicBool = AtomicBool::new(false);`

```
/// When true, write_str/write_byte skip the framebuffer (GUI owns it).
/// kprintln still goes to serial.
```

## L117 · `pub fn get_resolution() -> (u32, u32) {`

```
/// Get the current framebuffer resolution.
```

## L126 · `pub fn get_info() -> FbInfo {`

```
/// Get a copy of the current FbInfo.
```

## L141 · `pub fn shadow_phys_info() -> Option<(u64, u64, u32)> {`

```
/// Get shadow buffer physical addresses + page count (for GPU GGTT mapping).
```

## L146 · `pub fn front_ggtt() -> u32 {`

```
/// Get front buffer's GGTT offset (for GPU blit source).
```

## L151 · `pub fn set_shadow_ggtt(a: u32, b: u32) {`

```
/// Store GGTT offsets after GPU mapping.
```

## L161 · `const FG_COLOR: u32 = 0x00E8E8E8; // Near-white (was light gray)`

```
// Near-white (was light gray)
```

## L162 · `const BG_COLOR: u32 = 0x00000000; // Black`

```
// Black
```

## L163-164 · `static NPK_TAG_COLOR: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0x00909090);`

```
/// Colour of the `[npk]` tag. Neutral grey: dim enough to read as a tag
/// rather than as content, without putting a hue on every boot line.
```

## L167-175 · `const SCROLL_ROWS: u32 = 8;`

```
/// Rows the console scrolls at once when it fills up.
///
/// A scroll repaints everything, and on real hardware the framebuffer is
/// effectively uncached (firmware MTRR — see `diagnose_fb_memory_type`), so
/// that full blit runs at ~155 MB/s: ~50 ms per scroll at 1080p, four times
/// that at 4K. Scrolling one row per line spends it on EVERY line of boot
/// output — measured 268 lines over 67 rows = ~200 full blits. Moving
/// several rows at a time amortises it; the text jumps instead of sliding,
/// which is what framebuffer consoles have always done at boot.
```

## L182-183 · `pub fn init_from_gpu() {`

```
/// Initialize framebuffer console from GPU subsystem.
/// Call after gpu::init() has detected and configured a display.
```

## L202-208 · `if !crate::gpu::is_native() {`

```
// Map framebuffer pages (for GOP; Intel Xe uses identity-mapped RAM).
// WRITE_COMBINE is essential: the GOP MMIO framebuffer is otherwise
// effectively uncached on real Intel HW (firmware MTRR), so every CPU
// blit write is a separate UC transaction → brutally slow scrolling /
// dragging / cursor on bare metal (QEMU's fb is RAM, so it never showed
// there). WC batches writes into burst transactions. PAT index 5 = WC is
// programmed in paging::init.
```

## L220-222 · `diagnose_fb_memory_type(addr);`

```
// Diagnose the EFFECTIVE memory type of the FB: a firmware MTRR of
// type UC over this region overrides our PAT WC (UC always wins) →
// the blit runs at ~150 MB/s instead of GB/s. Read-only.
```

## L227-228 · `let scale = if width > 2560 { 2u32 } else { 1 };`

```
// 2x pixel scaling for 4K (each glyph pixel → 2×2 block).
// Threshold matches gui::font::scale_for — keep both in step.
```

## L235 · `{`

```
// Free old shadow buffers before allocating new ones (prevents OOM on mode switch)
```

## L251-252 · `let shadow_size = pitch as usize * height as usize;`

```
// Allocate two shadow buffers for double-buffering (fast WB-cached RAM)
// Page-aligned (4096) so they can be GGTT-mapped for GPU BCS blit
```

## L268 · `clear_screen(&info);`

```
// Clear MMIO framebuffer
```

## L271 · `SHADOW_FRONT_CACHED.store(shadow, Ordering::Release);`

```
// Initialize cached pointer for lock-free cursor
```

## L281-283 · `pub fn init_from_boot_info(boot_info: &crate::boot_info::BootInfo) {`

```
/// Initialize the framebuffer from BootInfo. Delegates to `gpu::init`
/// (which wraps the GOP-via-UEFI framebuffer driver) then sets up our
/// shadow + scaling state.
```

## L295 · `fn put_pixel_shadow(console: &FbConsole, x: u32, y: u32, color: u32) {`

```
/// Write pixel to shadow buffer (RAM, fast).
```

## L300 · `let offset = (y * info.pitch + x * 4) as usize;`

```
// Single aligned 32-bit write (BGRA/RGBA, 99% of UEFI framebuffers)
```

## L302 · `unsafe { *(console.shadow.add(offset) as *mut u32) = color; }`

```
// SAFETY: shadow buffer is large enough, offset checked by bounds above
```

## L315-316 · `fn draw_char_colored(console: &FbConsole, c: u8, col: u32, row: u32, fg: u32) {`

```
/// Draw character to shadow buffer with specified foreground color.
/// At scale > 1, each glyph pixel becomes a scale×scale block.
```

## L328 · `for sy in 0..s {`

```
// Write scale×scale block
```

## L338 · `fn draw_char(console: &FbConsole, c: u8, col: u32, row: u32) {`

```
/// Draw character to shadow buffer.
```

## L343 · `fn blit_region(console: &FbConsole, y_start: u32, y_end: u32) {`

```
/// Blit a region of the front buffer to the MMIO framebuffer.
```

## L360 · `fn blit_all(console: &FbConsole) {`

```
/// Blit entire front buffer to MMIO framebuffer in 256-row chunks.
```

## L380 · `fn blit_char(console: &FbConsole, col: u32, row: u32) {`

```
/// Blit a single character cell from front buffer to MMIO.
```

## L404-406 · `fn scroll_shadow(console: &mut FbConsole, n: u32) -> u32 {`

```
/// Scroll shadow buffer only (no MMIO blit — caller handles batching).
/// Move the console up by `n` text rows, clearing the ones freed at the
/// bottom. Returns the row the cursor lands on.
```

## L430 · `fn scroll(console: &mut FbConsole) {`

```
/// Scroll with immediate blit (for single-byte writes like write_byte).
```

## L437 · `pub fn blit_rect(console: &FbConsole, x: u32, y: u32, w: u32, h: u32) {`

```
/// Blit a rectangular region from front buffer to MMIO framebuffer.
```

## L446 · `unsafe {`

```
// SAFETY: front buffer and fb are valid for shadow_size bytes, bounds checked above
```

## L457-458 · `pub fn dump_memory_type() {`

```
/// On-demand version of the boot-time MTRR diagnostic (the `mtrr` intent),
/// so it can be read without catching the fast-scrolling boot log.
```

## L467-470 · `if let Some(dev) = crate::pci::find_by_class(0x03, 0x00) {`

```
// Which display device is this, and did the native (display-only) Xe
// driver claim it? On a non-ADL-N GPU it falls back to the slow GOP UC
// blit — the device ID tells us if it's a trivial ID-add (Gen12) or a
// new display-gen port.
```

## L482-485 · `fn diagnose_fb_memory_type(fb_addr: u64) {`

```
/// Read-only: report the framebuffer's EFFECTIVE memory type (firmware
/// MTRRs vs our PAT WC). A UC MTRR over this region overrides PAT WC (UC
/// always wins) → the ~150 MB/s blit we measured. Confirms the root cause
/// before we touch MTRR programming.
```

## L487 · `unsafe fn rdmsr(msr: u32) -> u64 {`

```
// SAFETY: rdmsr on architectural MTRR/PAT MSRs, all read-only.
```

## L491-492 · `unsafe {`

```
// SAFETY: architectural MTRR/PAT MSRs, read-only; the caller
// guarantees `msr` is one of them.
```

## L512-513 · `let mut eff: Option<u8> = None;`

```
// Effective MTRR type for fb_addr: scan valid variable MTRRs (UC wins,
// else WT over WB, else the matching type, else the default type).
```

## L518 · `if mask & (1 << 11) == 0 { continue; } // V bit clear → not valid`

```
// V bit clear → not valid
```

## L541 · `if let Some((entry, level)) = crate::paging::leaf_entry(fb_addr) {`

```
// Read back the FB PTE and decode its PAT index — confirm WE asked WC.
```

## L552 · `fn emit_char(console: &mut FbConsole, byte: u8, fg: u32,`

```
/// Emit a single character to the console with a given foreground color.
```

## L591 · `pub fn write_str(s: &str) {`

```
/// Write a string to the framebuffer console.
```

## L593 · `if GUI_MODE.load(Ordering::Relaxed) { return; }`

```
// GUI owns the framebuffer — skip text console writes
```

## L598 · `None => return, // No framebuffer`

```
// No framebuffer
```

## L601 · `let mut dirty_min_row: u32 = console.rows;`

```
// Draw everything to shadow buffer first (no MMIO writes yet)
```

## L606 · `let bytes = s.as_bytes();`

```
// [npk] tag coloring: detect pattern and color those chars
```

## L610 · `let (byte, fg) = if i + 5 <= bytes.len() && &bytes[i..i+5] == b"[npk]" {`

```
// Check for [npk] pattern
```

## L612 · `for j in 0..5 {`

```
// Emit all 5 chars in accent color
```

## L627 · `let s = console.scale;`

```
// Single blit at the end: either full (if scrolled) or just dirty rows
```

## L638 · `pub fn write_byte(byte: u8) {`

```
/// Write a single byte to framebuffer (for echo from serial read).
```

## L678 · `pub fn clear() {`

```
/// Clear the framebuffer screen (both shadow buffers + MMIO).
```

## L696 · `pub fn is_available() -> bool {`

```
/// Check if framebuffer is available.
```

## L701 · `#[allow(dead_code)]`

```
// Early framebuffer state (set before heap is available)
```

## L703 · `static mut EARLY_FB: Option<(u64, u32, u32, u32, u8)> = None; // (addr, pitch, w, h, bpp)`

```
// (addr, pitch, w, h, bpp)
```

## L708-709 · `pub fn early_probe(mb_info_addr: u32) -> bool {`

```
/// Super-early framebuffer probe: finds framebuffer, draws green bar.
/// Works BEFORE heap/paging init (only needs 4GB identity mapping from boot.s).
```

## L729 · `let fb = addr as *mut u8;`

```
// Draw green bar
```

## L753 · `pub fn early_debug(ch: u8) {`

```
/// Write a debug character to the early framebuffer (no heap needed).
```

## L789-791 · `#[rustfmt::skip]`

```
// ============================================================
// Embedded 8x16 Font (CP437, 128 printable ASCII characters)
// ============================================================
```

## L795 · `0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x00-0x1F: control chars (blank)
```

## L828 · `0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x20 ' '
```

## L830 · `0x00,0x00,0x18,0x3C,0x3C,0x3C,0x18,0x18,0x18,0x00,0x18,0x18,0x00,0x00,0x00,0x00,`

```
// 0x21 '!'
```

## L832 · `0x00,0x66,0x66,0x66,0x24,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x22 '"'
```

## L834 · `0x00,0x00,0x00,0x6C,0x6C,0xFE,0x6C,0x6C,0x6C,0xFE,0x6C,0x6C,0x00,0x00,0x00,0x00,`

```
// 0x23 '#'
```

## L836 · `0x18,0x18,0x7C,0xC6,0xC2,0xC0,0x7C,0x06,0x06,0x86,0xC6,0x7C,0x18,0x18,0x00,0x00,`

```
// 0x24 '$'
```

## L838 · `0x00,0x00,0x00,0x00,0xC2,0xC6,0x0C,0x18,0x30,0x60,0xC6,0x86,0x00,0x00,0x00,0x00,`

```
// 0x25 '%'
```

## L840 · `0x00,0x00,0x38,0x6C,0x6C,0x38,0x76,0xDC,0xCC,0xCC,0xCC,0x76,0x00,0x00,0x00,0x00,`

```
// 0x26 '&'
```

## L842 · `0x00,0x30,0x30,0x30,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x27 '''
```

## L844 · `0x00,0x00,0x0C,0x18,0x30,0x30,0x30,0x30,0x30,0x30,0x18,0x0C,0x00,0x00,0x00,0x00,`

```
// 0x28 '('
```

## L846 · `0x00,0x00,0x30,0x18,0x0C,0x0C,0x0C,0x0C,0x0C,0x0C,0x18,0x30,0x00,0x00,0x00,0x00,`

```
// 0x29 ')'
```

## L848 · `0x00,0x00,0x00,0x00,0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x2A '*'
```

## L850 · `0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x7E,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x2B '+'
```

## L852 · `0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x18,0x30,0x00,0x00,0x00,`

```
// 0x2C ','
```

## L854 · `0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFE,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x2D '-'
```

## L856 · `0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00,0x00,0x00,0x00,`

```
// 0x2E '.'
```

## L858 · `0x00,0x00,0x00,0x00,0x02,0x06,0x0C,0x18,0x30,0x60,0xC0,0x80,0x00,0x00,0x00,0x00,`

```
// 0x2F '/'
```

## L860 · `0x00,0x00,0x7C,0xC6,0xC6,0xCE,0xDE,0xF6,0xE6,0xC6,0xC6,0x7C,0x00,0x00,0x00,0x00,`

```
// 0x30-0x39: '0'-'9'
```

## L871 · `0x00,0x00,0x00,0x00,0x18,0x18,0x00,0x00,0x00,0x18,0x18,0x00,0x00,0x00,0x00,0x00,`

```
// 0x3A ':'
```

## L873 · `0x00,0x00,0x00,0x00,0x18,0x18,0x00,0x00,0x00,0x18,0x18,0x30,0x00,0x00,0x00,0x00,`

```
// 0x3B ';'
```

## L875 · `0x00,0x00,0x00,0x06,0x0C,0x18,0x30,0x60,0x30,0x18,0x0C,0x06,0x00,0x00,0x00,0x00,`

```
// 0x3C '<'
```

## L877 · `0x00,0x00,0x00,0x00,0x00,0x7E,0x00,0x00,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x3D '='
```

## L879 · `0x00,0x00,0x00,0x60,0x30,0x18,0x0C,0x06,0x0C,0x18,0x30,0x60,0x00,0x00,0x00,0x00,`

```
// 0x3E '>'
```

## L881 · `0x00,0x00,0x7C,0xC6,0xC6,0x0C,0x18,0x18,0x18,0x00,0x18,0x18,0x00,0x00,0x00,0x00,`

```
// 0x3F '?'
```

## L883 · `0x00,0x00,0x7C,0xC6,0xC6,0xC6,0xDE,0xDE,0xDE,0xDC,0xC0,0x7C,0x00,0x00,0x00,0x00,`

```
// 0x40 '@'
```

## L885 · `0x00,0x00,0x10,0x38,0x6C,0xC6,0xC6,0xFE,0xC6,0xC6,0xC6,0xC6,0x00,0x00,0x00,0x00,`

```
// 0x41-0x5A: 'A'-'Z'
```

## L912 · `0x00,0x00,0x3C,0x30,0x30,0x30,0x30,0x30,0x30,0x30,0x30,0x3C,0x00,0x00,0x00,0x00,`

```
// 0x5B '['
```

## L914 · `0x00,0x00,0x00,0x80,0xC0,0xE0,0x70,0x38,0x1C,0x0E,0x06,0x02,0x00,0x00,0x00,0x00,`

```
// 0x5C '\'
```

## L916 · `0x00,0x00,0x3C,0x0C,0x0C,0x0C,0x0C,0x0C,0x0C,0x0C,0x0C,0x3C,0x00,0x00,0x00,0x00,`

```
// 0x5D ']'
```

## L918 · `0x10,0x38,0x6C,0xC6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x5E '^'
```

## L920 · `0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF,0x00,0x00,`

```
// 0x5F '_'
```

## L922 · `0x30,0x30,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x60 '`'
```

## L924 · `0x00,0x00,0x00,0x00,0x00,0x78,0x0C,0x7C,0xCC,0xCC,0xCC,0x76,0x00,0x00,0x00,0x00,`

```
// 0x61-0x7A: 'a'-'z'
```

## L951 · `0x00,0x00,0x0E,0x18,0x18,0x18,0x70,0x18,0x18,0x18,0x18,0x0E,0x00,0x00,0x00,0x00,`

```
// 0x7B '{'
```

## L953 · `0x00,0x00,0x18,0x18,0x18,0x18,0x00,0x18,0x18,0x18,0x18,0x18,0x00,0x00,0x00,0x00,`

```
// 0x7C '|'
```

## L955 · `0x00,0x00,0x70,0x18,0x18,0x18,0x0E,0x18,0x18,0x18,0x18,0x70,0x00,0x00,0x00,0x00,`

```
// 0x7D '}'
```

## L957 · `0x00,0x00,0x76,0xDC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x7E '~'
```

## L959 · `0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,`

```
// 0x7F DEL (blank)
```

