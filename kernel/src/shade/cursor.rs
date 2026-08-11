//! Software mouse cursor — OVERLAY approach.
//!
//! The cursor is NEVER drawn on the shadow buffer. Instead:
//! - Shadow buffer stays clean (scene only)
//! - Cursor is drawn directly on the MMIO framebuffer
//! - On move: restore old area from shadow→MMIO, draw cursor at new pos on MMIO
//! - No save/restore array needed, no ghost cursors possible
//!
//! Lock-free fast path: mouse position is stored as atomics.
//! Core 0 (input) writes position in ~2ns without any lock.
//! Cursor overlay reads atomics and draws directly to MMIO.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, Ordering};

/// Cursor dimensions.
const CURSOR_W: u32 = 15;
const CURSOR_H: u32 = 22;

/// Effective cursor size (w, h) — for callers that blit just the cursor rect.
pub fn cursor_size() -> (u32, u32) { eff_dims() }

// ── Save-under cursor (no recomposite on a pure move) ───────────────────
//
// The render path keeps the scene in the shadow buffer with the cursor
// BAKED in (atomic blit → no flicker). To move the cursor without
// recompositing the whole scene we remember the clean pixels under it
// (`SAVE_UNDER`): on a move we restore them (erase the old cursor), then
// save + bake at the new spot and blit only the small affected region.
// Core-0 / CONSOLE-lock serialized; the Mutex just satisfies the borrow
// checker for the static array.
// Sized for the largest cursor (SIZE_MAX_PCT) so scaling never overflows it.
const SU_N: usize = ((CURSOR_W * SIZE_MAX_PCT / 100) * (CURSOR_H * SIZE_MAX_PCT / 100)) as usize;
static SAVE_UNDER: spin::Mutex<[u32; SU_N]> = spin::Mutex::new([0; SU_N]);
static SU_X: AtomicI32 = AtomicI32::new(0);
static SU_Y: AtomicI32 = AtomicI32::new(0);
static SU_W: AtomicU32 = AtomicU32::new(0);   // dims the cursor was baked at
static SU_H: AtomicU32 = AtomicU32::new(0);
static SU_VALID: AtomicBool = AtomicBool::new(false);

/// Position the cursor is currently baked at (the rect to erase on a move).
pub fn saved_pos() -> (i32, i32) { (SU_X.load(Ordering::Relaxed), SU_Y.load(Ordering::Relaxed)) }
pub fn save_valid() -> bool { SU_VALID.load(Ordering::Relaxed) }

/// Save the clean pixels under the cursor's CURRENT position from `buf`,
/// then bake the cursor there. `buf` is a shadow buffer (plain RAM).
pub fn save_under_and_bake(buf: *mut u8, info: &crate::framebuffer::FbInfo) {
    if buf.is_null() || !crate::xhci::mouse_available() { return; }
    let pitch = info.pitch as usize;
    let sw = info.width as i32;
    let sh = info.height as i32;
    let (cx, cy) = atomic_pos();
    let (ew, eh) = eff_dims();
    let mut su = SAVE_UNDER.lock();
    for row in 0..eh {
        let py = cy + row as i32;
        for col in 0..ew {
            let px = cx + col as i32;
            let idx = (row * ew + col) as usize;
            if px < 0 || px >= sw || py < 0 || py >= sh { su[idx] = 0; continue; }
            let off = py as usize * pitch + px as usize * 4;
            // SAFETY: bounds-checked offset into a kernel-owned shadow buffer.
            unsafe {
                let p = buf.add(off) as *mut u32;
                let bg = *p;
                su[idx] = bg;
                let (color, a) = cursor_sample_aa(col, row, ew, eh);
                if a > 0 { *p = blend(bg, color, a); }
            }
        }
    }
    SU_X.store(cx, Ordering::Relaxed);
    SU_Y.store(cy, Ordering::Relaxed);
    SU_W.store(ew, Ordering::Relaxed);
    SU_H.store(eh, Ordering::Relaxed);
    SU_VALID.store(true, Ordering::Relaxed);
}

/// Restore the saved clean pixels to `buf` at the saved position — erases
/// the baked cursor so the scene under it is intact again.
pub fn restore_under(buf: *mut u8, info: &crate::framebuffer::FbInfo) {
    if buf.is_null() || !SU_VALID.load(Ordering::Relaxed) { return; }
    let pitch = info.pitch as usize;
    let sw = info.width as i32;
    let sh = info.height as i32;
    let sx = SU_X.load(Ordering::Relaxed);
    let sy = SU_Y.load(Ordering::Relaxed);
    let ew = SU_W.load(Ordering::Relaxed);
    let eh = SU_H.load(Ordering::Relaxed);
    let su = SAVE_UNDER.lock();
    for row in 0..eh {
        let py = sy + row as i32;
        for col in 0..ew {
            let px = sx + col as i32;
            if px < 0 || px >= sw || py < 0 || py >= sh { continue; }
            let idx = (row * ew + col) as usize;
            let off = py as usize * pitch + px as usize * 4;
            // SAFETY: bounds-checked offset into a kernel-owned shadow buffer.
            unsafe { *(buf.add(off) as *mut u32) = su[idx]; }
        }
    }
}

// ── Lock-free mouse position (written by Core 0, read by anyone) ──

static ATOMIC_X: AtomicI32 = AtomicI32::new(0);
static ATOMIC_Y: AtomicI32 = AtomicI32::new(0);
static ATOMIC_BUTTONS: AtomicU8 = AtomicU8::new(0);
static ATOMIC_PREV_BUTTONS: AtomicU8 = AtomicU8::new(0);
static MOUSE_DIRTY: AtomicBool = AtomicBool::new(false);
static SCREEN_W: AtomicI32 = AtomicI32::new(1920);
static SCREEN_H: AtomicI32 = AtomicI32::new(1080);

/// Pointer speed in percent (100 = 1:1 with the device deltas). Touchpads
/// deliver small deltas, so the default scales up. Tunable via the `mouse`
/// intent / `mouse_speed` config. Sub-100 (slow-down) is smooth because the
/// fractional remainder is accumulated below.
static SPEED: AtomicI32 = AtomicI32::new(220);
static ACC_X: AtomicI32 = AtomicI32::new(0);
static ACC_Y: AtomicI32 = AtomicI32::new(0);

/// Set pointer speed (percent, clamped 25..=600).
pub fn set_speed(percent: i32) { SPEED.store(percent.clamp(25, 600), Ordering::Relaxed); }
/// Current pointer speed (percent).
pub fn speed() -> i32 { SPEED.load(Ordering::Relaxed) }

/// Cursor size in percent of the base bitmap (100 = 16×22). Clamped 50..=MAX.
const SIZE_MAX_PCT: u32 = 300;
static SIZE: AtomicI32 = AtomicI32::new(100);

/// Set cursor size (percent, clamped 50..=300).
pub fn set_size(percent: i32) { SIZE.store(percent.clamp(50, SIZE_MAX_PCT as i32), Ordering::Relaxed); }
/// Current cursor size (percent).
pub fn size() -> i32 { SIZE.load(Ordering::Relaxed) }

/// Effective (scaled) cursor dimensions in pixels.
fn eff_dims() -> (u32, u32) {
    let s = SIZE.load(Ordering::Relaxed) as u32;
    ((CURSOR_W * s / 100).max(1), (CURSOR_H * s / 100).max(1))
}

/// Alpha-blend `fg` over `bg` at coverage `a` (0..=255).
fn blend(bg: u32, fg: u32, a: u32) -> u32 {
    let na = 255 - a;
    let r = (((bg >> 16) & 0xff) * na + ((fg >> 16) & 0xff) * a) / 255;
    let g = (((bg >> 8) & 0xff) * na + ((fg >> 8) & 0xff) * a) / 255;
    let b = ((bg & 0xff) * na + (fg & 0xff) * a) / 255;
    (r << 16) | (g << 8) | b
}

/// Even-odd point-in-polygon test in reference space.
fn in_poly(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let n = poly.len();
    let mut c = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

/// Supersampled coverage (0..=1) of `poly` at output pixel (col,row), mapping
/// the pixel into the REF_W×REF_H reference space. SS×SS subsamples.
fn poly_cov(poly: &[(f32, f32)], col: u32, row: u32, ew: u32, eh: u32) -> f32 {
    const SS: u32 = 4;
    let sx = REF_W / ew as f32;
    let sy = REF_H / eh as f32;
    let mut hit = 0u32;
    for sj in 0..SS {
        for si in 0..SS {
            let x = (col as f32 + (si as f32 + 0.5) / SS as f32) * sx;
            let y = (row as f32 + (sj as f32 + 0.5) / SS as f32) * sy;
            if in_poly(poly, x, y) { hit += 1; }
        }
    }
    hit as f32 / (SS * SS) as f32
}

/// Anti-aliased cursor sample at output pixel (col,row) for effective dims
/// (ew,eh). Rasterizes the vector arrow (OUTER outline + INNER dark fill)
/// with supersampled coverage → (grayscale color, alpha 0..=255); alpha 0 =
/// transparent. Resolution-independent: smooth at any cursor size.
fn cursor_sample_aa(col: u32, row: u32, ew: u32, eh: u32) -> (u32, u32) {
    let a = poly_cov(&OUTER, col, row, ew, eh);      // outline silhouette → alpha
    if a <= 0.0 { return (0, 0); }
    let i = poly_cov(&INNER, col, row, ew, eh);      // dark-fill fraction
    // Outline ≈ white (235), interior ≈ near-black (30), mixed by fill cover.
    let lum = ((235.0 * (1.0 - i) + 30.0 * i) as u32) & 0xff;
    ((lum << 16) | (lum << 8) | lum, ((a * 255.0) as u32).min(255))
}

/// Update mouse position atomically. NO LOCK needed.
/// Called from Core 0 input polling — takes ~2 nanoseconds.
pub fn update_atomic(dx: i8, dy: i8, buttons: u8) {
    let sw = SCREEN_W.load(Ordering::Relaxed);
    let sh = SCREEN_H.load(Ordering::Relaxed);
    let old_btn = ATOMIC_BUTTONS.load(Ordering::Relaxed);

    // Scale device deltas by SPEED%, accumulating the fractional remainder so
    // slow speeds still move on small deltas and fast speeds stay smooth.
    let s = SPEED.load(Ordering::Relaxed);
    let ax = ACC_X.load(Ordering::Relaxed) + dx as i32 * s;
    let ay = ACC_Y.load(Ordering::Relaxed) + dy as i32 * s;
    let mvx = ax / 100;
    let mvy = ay / 100;
    ACC_X.store(ax - mvx * 100, Ordering::Relaxed);
    ACC_Y.store(ay - mvy * 100, Ordering::Relaxed);

    let x = (ATOMIC_X.load(Ordering::Relaxed) + mvx).clamp(0, sw - 1);
    let y = (ATOMIC_Y.load(Ordering::Relaxed) + mvy).clamp(0, sh - 1);

    ATOMIC_X.store(x, Ordering::Relaxed);
    ATOMIC_Y.store(y, Ordering::Relaxed);
    ATOMIC_PREV_BUTTONS.store(old_btn, Ordering::Relaxed);
    ATOMIC_BUTTONS.store(buttons, Ordering::Release);
    MOUSE_DIRTY.store(true, Ordering::Release);
}

/// Read current atomic mouse position
pub fn atomic_pos() -> (i32, i32) {
    (ATOMIC_X.load(Ordering::Relaxed), ATOMIC_Y.load(Ordering::Relaxed))
}

/// Read atomic buttons (current, previous)
pub fn atomic_buttons() -> (u8, u8) {
    (ATOMIC_BUTTONS.load(Ordering::Acquire), ATOMIC_PREV_BUTTONS.load(Ordering::Relaxed))
}

/// Was left button just clicked? (lock-free)
#[allow(dead_code)]
pub fn atomic_left_clicked() -> bool {
    let (cur, prev) = atomic_buttons();
    (cur & 1) != 0 && (prev & 1) == 0
}

/// Was right button just clicked? (lock-free)
#[allow(dead_code)]
pub fn atomic_right_clicked() -> bool {
    let (cur, prev) = atomic_buttons();
    (cur & 2) != 0 && (prev & 2) == 0
}

/// Any button action that needs compositor attention? (click, release)
pub fn has_button_event() -> bool {
    let (cur, prev) = atomic_buttons();
    cur != prev
}

/// Set screen dimensions for atomic clamping
pub fn set_screen_size(w: u32, h: u32) {
    SCREEN_W.store(w as i32, Ordering::Relaxed);
    SCREEN_H.store(h as i32, Ordering::Relaxed);
}

/// Initialize atomic position (centered)
pub fn init_atomic(screen_w: u32, screen_h: u32) {
    set_screen_size(screen_w, screen_h);
    ATOMIC_X.store((screen_w / 2) as i32, Ordering::Relaxed);
    ATOMIC_Y.store((screen_h / 2) as i32, Ordering::Relaxed);
}

/// Vector arrow cursor. A classic `left_ptr`: tip (hotspot) at (0,0), a
/// light outline (OUTER silhouette) around a dark fill (INNER = OUTER inset
/// by the outline width, computed offline). Both are rasterized with
/// supersampled point-in-polygon coverage in a REF_W×REF_H reference space,
/// so the cursor is resolution-independent — smooth at any size, unlike the
/// old 16×22 bitmap that pixelated at 1:1. REF is exactly 2× the base dims
/// (15×22) so the default maps 1 ref = 0.5 px with no aspect distortion.
const REF_W: f32 = 30.0;
const REF_H: f32 = 44.0;

/// Outline silhouette (outer boundary of the light stroke), tip at (0,0).
/// A tail-less arrowhead: vertical left edge, concave bottom notch, wing.
static OUTER: [(f32, f32); 4] = [
    (0.0,  0.0),   // tip / hotspot
    (0.0, 29.0),   // bottom of the left edge
    (9.5, 21.5),   // notch (concave bottom)
    (22.5, 21.5),  // right wing
];

/// Dark fill — OUTER inset by the outline width (precomputed miter offset).
static INNER: [(f32, f32); 4] = [
    (2.30,  5.38),
    (2.30, 24.25),
    (8.70, 19.20),
    (16.76, 19.20),
];

/// Mouse state — position, buttons, and overlay tracking.
#[allow(dead_code)]
pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
    pub prev_buttons: u8,
    pub screen_w: u32,
    pub screen_h: u32,
    /// Last position where cursor was drawn on MMIO (for restore).
    drawn_x: i32,
    drawn_y: i32,
    /// Whether cursor is currently drawn on MMIO framebuffer.
    drawn: bool,
}

impl MouseState {
    pub const fn new() -> Self {
        MouseState {
            x: 0, y: 0,
            buttons: 0, prev_buttons: 0,
            screen_w: 0, screen_h: 0,
            drawn_x: 0, drawn_y: 0,
            drawn: false,
        }
    }

    /// Initialize with screen dimensions. Centers the cursor.
    pub fn init(&mut self, screen_w: u32, screen_h: u32) {
        self.screen_w = screen_w;
        self.screen_h = screen_h;
        self.x = (screen_w / 2) as i32;
        self.y = (screen_h / 2) as i32;
    }

    /// Update position from relative mouse movement. Clamps to screen.
    pub fn update(&mut self, dx: i8, dy: i8, buttons: u8) {
        self.prev_buttons = self.buttons;
        self.buttons = buttons;
        self.x = (self.x + dx as i32).clamp(0, self.screen_w as i32 - 1);
        self.y = (self.y + dy as i32).clamp(0, self.screen_h as i32 - 1);
    }

    pub fn left_clicked(&self) -> bool {
        (self.buttons & 1) != 0 && (self.prev_buttons & 1) == 0
    }
    pub fn right_clicked(&self) -> bool {
        (self.buttons & 2) != 0 && (self.prev_buttons & 2) == 0
    }
    pub fn left_held(&self) -> bool { (self.buttons & 1) != 0 }
    pub fn right_held(&self) -> bool { (self.buttons & 2) != 0 }
    #[allow(dead_code)]
    pub fn left_released(&self) -> bool {
        (self.buttons & 1) == 0 && (self.prev_buttons & 1) != 0
    }
    #[allow(dead_code)]
    pub fn right_released(&self) -> bool {
        (self.buttons & 2) == 0 && (self.prev_buttons & 2) != 0
    }

}

// ── Shared cursor-drawn state (used by both lock-free and inner paths) ──

static DRAWN_LF_X: AtomicI32 = AtomicI32::new(0);
static DRAWN_LF_Y: AtomicI32 = AtomicI32::new(0);
static DRAWN_LF_W: AtomicU32 = AtomicU32::new(0);   // dims of the last MMIO-drawn cursor
static DRAWN_LF_H: AtomicU32 = AtomicU32::new(0);
static DRAWN_LF: AtomicBool = AtomicBool::new(false);

/// Rect (w,h) to erase for the last MMIO-drawn cursor (falls back to current
/// effective size if nothing drawn yet).
fn drawn_erase_dims() -> (u32, u32) {
    let w = DRAWN_LF_W.load(Ordering::Relaxed);
    let h = DRAWN_LF_H.load(Ordering::Relaxed);
    if w == 0 || h == 0 { eff_dims() } else { (w, h) }
}

/// Draw cursor from IRQ context — no locks, no shadow restore.
/// Writes directly to MMIO using cached framebuffer info.
/// Any trail artifact is cleaned up by the next render_frame().
#[allow(dead_code)]
pub fn draw_cursor_irq() {
    let addr = IRQ_FB_ADDR.load(Ordering::Relaxed);
    if addr == 0 { return; }
    let pitch = IRQ_FB_PITCH.load(Ordering::Relaxed) as usize;
    let sw = SCREEN_W.load(Ordering::Relaxed);
    let sh = SCREEN_H.load(Ordering::Relaxed);
    let (cx, cy) = atomic_pos();
    let shadow = crate::framebuffer::cached_shadow_front();
    draw_cursor_on_mmio(addr as *mut u8, shadow, pitch, sw, sh, cx, cy);
}

/// Draw cursor after scene blit. Erases old position ONLY if cursor moved
/// (no blink when stationary, no ghost when moved).
#[allow(dead_code)]
pub fn draw_cursor_after_blit(fb: &mut crate::framebuffer::FbConsole) {
    let info = fb.info();
    let (shadow, _) = fb.shadow_ptr();
    let mmio = info.addr as *mut u8;
    let pitch = info.pitch as usize;
    let sw = info.width as i32;
    let sh = info.height as i32;

    let (cx, cy) = atomic_pos();

    // Erase old cursor only if it moved (avoid blink when stationary)
    if DRAWN_LF.load(Ordering::Relaxed) {
        let dx = DRAWN_LF_X.load(Ordering::Relaxed);
        let dy = DRAWN_LF_Y.load(Ordering::Relaxed);
        if dx != cx || dy != cy {
            blit_shadow_to_mmio(shadow, mmio, pitch, sw, sh, dx, dy, drawn_erase_dims().0, drawn_erase_dims().1);
        }
    }

    draw_cursor_on_mmio(mmio, shadow, pitch, sw, sh, cx, cy);

    DRAWN_LF_X.store(cx, Ordering::Relaxed);
    DRAWN_LF_Y.store(cy, Ordering::Relaxed);
    DRAWN_LF.store(true, Ordering::Relaxed);
}

/// Cached framebuffer MMIO address for IRQ-safe cursor draw
static IRQ_FB_ADDR: AtomicU64 = AtomicU64::new(0);
static IRQ_FB_PITCH: AtomicU32 = AtomicU32::new(0);

/// Cache framebuffer info for IRQ cursor draw. Call after GPU init.
pub fn cache_fb_info(addr: u64, pitch: u32) {
    IRQ_FB_ADDR.store(addr, Ordering::Relaxed);
    IRQ_FB_PITCH.store(pitch, Ordering::Relaxed);
}

/// Draw cursor bitmap directly to MMIO framebuffer at given position.
fn draw_cursor_on_mmio(mmio: *mut u8, bg_buf: *const u8, pitch: usize, sw: i32, sh: i32, x: i32, y: i32) {
    let (ew, eh) = eff_dims();
    for row in 0..eh {
        let py = y + row as i32;
        if py < 0 || py >= sh { continue; }
        for col in 0..ew {
            let px = x + col as i32;
            if px < 0 || px >= sw { continue; }
            let (color, a) = cursor_sample_aa(col, row, ew, eh);
            if a == 0 { continue; }
            let off = py as usize * pitch + px as usize * 4;
            // Blend over the clean shadow pixel if provided, else read back the
            // current MMIO pixel (slow path; only the IRQ fallback with no shadow).
            let bg = unsafe {
                if bg_buf.is_null() { *(mmio.add(off) as *const u32) }
                else { *(bg_buf.add(off) as *const u32) }
            };
            // SAFETY: writing to MMIO framebuffer within bounds
            unsafe { core::ptr::write_volatile(mmio.add(off) as *mut u32, blend(bg, color, a)); }
        }
    }
    DRAWN_LF_W.store(ew, Ordering::Relaxed);
    DRAWN_LF_H.store(eh, Ordering::Relaxed);
}

/// Paint cursor bitmap into the back-shadow buffer at the current
/// atomic position. Called at the very end of compose so the cursor
/// becomes part of the same shadow that gets blitted to MMIO — no
/// separate post-blit cursor write, no race between blit and cursor.
///
/// Critical for high-frequency surface tiles (microvm browser at 60Hz
/// FLUSH): the previous design re-blitted the entire shadow on every
/// frame, then re-drew the cursor over MMIO. Display refresh could
/// catch the brief window where the blit had landed but the cursor
/// re-write hadn't yet — visible as cursor flicker right after
/// stopping mouse movement (the moving case masked the flicker with
/// the moving cursor's blur).
///
/// Shadow is the single source of truth now: blit copies cursor along
/// with the rest of the scene atomically (from the display's view).
pub fn draw_cursor_on_shadow(shadow: *mut u8, info: &crate::framebuffer::FbInfo) {
    if shadow.is_null() { return; }
    if !crate::xhci::mouse_available() { return; }
    let pitch = info.pitch as usize;
    let sw = info.width as i32;
    let sh = info.height as i32;
    let (cx, cy) = atomic_pos();
    let (ew, eh) = eff_dims();
    for row in 0..eh {
        let py = cy + row as i32;
        if py < 0 || py >= sh { continue; }
        for col in 0..ew {
            let px = cx + col as i32;
            if px < 0 || px >= sw { continue; }
            let (color, a) = cursor_sample_aa(col, row, ew, eh);
            if a == 0 { continue; }
            let off = py as usize * pitch + px as usize * 4;
            // SAFETY: alpha-blend over the existing back-shadow pixel. Shadow
            // is a kernel-owned allocation, not MMIO — plain load/store fine.
            unsafe { let p = shadow.add(off) as *mut u32; *p = blend(*p, color, a); }
        }
    }
}

/// Copy a small rectangle from shadow buffer to MMIO framebuffer (restore clean pixels).
fn blit_shadow_to_mmio(shadow: *mut u8, mmio: *mut u8, pitch: usize,
                       sw: i32, sh: i32, x: i32, y: i32, w: u32, h: u32) {
    for row in 0..h as i32 {
        let py = y + row;
        if py < 0 || py >= sh { continue; }
        let x0 = x.max(0) as usize;
        let x1 = (x + w as i32).min(sw) as usize;
        if x0 >= x1 { continue; }
        let off = py as usize * pitch + x0 * 4;
        let len = (x1 - x0) * 4;
        // SAFETY: copying from shadow buffer to MMIO framebuffer
        unsafe {
            core::ptr::copy_nonoverlapping(shadow.add(off), mmio.add(off), len);
        }
    }
}
