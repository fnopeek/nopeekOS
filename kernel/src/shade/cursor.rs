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
const CURSOR_W: u32 = 12;
const CURSOR_H: u32 = 19;

// ── Lock-free mouse position (written by Core 0, read by anyone) ──

static ATOMIC_X: AtomicI32 = AtomicI32::new(0);
static ATOMIC_Y: AtomicI32 = AtomicI32::new(0);
static ATOMIC_BUTTONS: AtomicU8 = AtomicU8::new(0);
static ATOMIC_PREV_BUTTONS: AtomicU8 = AtomicU8::new(0);
static MOUSE_DIRTY: AtomicBool = AtomicBool::new(false);
static SCREEN_W: AtomicI32 = AtomicI32::new(1920);
static SCREEN_H: AtomicI32 = AtomicI32::new(1080);

/// Update mouse position atomically. NO LOCK needed.
/// Called from Core 0 input polling — takes ~2 nanoseconds.
pub fn update_atomic(dx: i8, dy: i8, buttons: u8) {
    let sw = SCREEN_W.load(Ordering::Relaxed);
    let sh = SCREEN_H.load(Ordering::Relaxed);
    let old_btn = ATOMIC_BUTTONS.load(Ordering::Relaxed);

    let x = (ATOMIC_X.load(Ordering::Relaxed) + dx as i32).clamp(0, sw - 1);
    let y = (ATOMIC_Y.load(Ordering::Relaxed) + dy as i32).clamp(0, sh - 1);

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

/// Check and clear dirty flag
pub fn take_dirty() -> bool {
    MOUSE_DIRTY.swap(false, Ordering::Acquire)
}

/// Was left button just clicked? (lock-free)
pub fn atomic_left_clicked() -> bool {
    let (cur, prev) = atomic_buttons();
    (cur & 1) != 0 && (prev & 1) == 0
}

/// Was right button just clicked? (lock-free)
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

/// Arrow cursor bitmap (1 = white outline, 2 = black fill, 0 = transparent).
static CURSOR_BITMAP: [u8; (CURSOR_W * CURSOR_H) as usize] = [
    1,0,0,0,0,0,0,0,0,0,0,0,
    1,1,0,0,0,0,0,0,0,0,0,0,
    1,2,1,0,0,0,0,0,0,0,0,0,
    1,2,2,1,0,0,0,0,0,0,0,0,
    1,2,2,2,1,0,0,0,0,0,0,0,
    1,2,2,2,2,1,0,0,0,0,0,0,
    1,2,2,2,2,2,1,0,0,0,0,0,
    1,2,2,2,2,2,2,1,0,0,0,0,
    1,2,2,2,2,2,2,2,1,0,0,0,
    1,2,2,2,2,2,2,2,2,1,0,0,
    1,2,2,2,2,2,2,2,2,2,1,0,
    1,2,2,2,2,2,2,2,2,2,2,1,
    1,2,2,2,2,2,1,1,1,1,1,1,
    1,2,2,2,2,2,1,0,0,0,0,0,
    1,2,2,1,2,2,1,0,0,0,0,0,
    1,2,1,0,1,2,2,1,0,0,0,0,
    1,1,0,0,1,2,2,1,0,0,0,0,
    1,0,0,0,0,1,2,2,1,0,0,0,
    0,0,0,0,0,1,1,1,0,0,0,0,
];

/// Mouse state — position, buttons, and overlay tracking.
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
    pub fn left_released(&self) -> bool {
        (self.buttons & 1) == 0 && (self.prev_buttons & 1) != 0
    }
    pub fn right_released(&self) -> bool {
        (self.buttons & 2) == 0 && (self.prev_buttons & 2) != 0
    }

    /// Redraw cursor overlay: restore old position from shadow→MMIO, draw at new position on MMIO.
    /// Call after any scene blit, or when cursor moves.
    pub fn redraw_overlay(&mut self, shadow: *mut u8, info: &crate::framebuffer::FbInfo) {
        let mmio = info.addr as *mut u8;
        let pitch = info.pitch as usize;
        let sw = info.width as i32;
        let sh = info.height as i32;

        // Restore old cursor area: copy shadow → MMIO
        if self.drawn {
            blit_shadow_to_mmio(shadow, mmio, pitch, sw, sh,
                self.drawn_x, self.drawn_y, CURSOR_W, CURSOR_H);
        }

        // Draw cursor at current position directly on MMIO
        for row in 0..CURSOR_H as i32 {
            let py = self.y + row;
            if py < 0 || py >= sh { continue; }
            for col in 0..CURSOR_W as i32 {
                let px = self.x + col;
                if px < 0 || px >= sw { continue; }
                let bmp = CURSOR_BITMAP[(row as usize) * CURSOR_W as usize + col as usize];
                if bmp == 0 { continue; }
                let off = py as usize * pitch + px as usize * 4;
                let color: u32 = if bmp == 1 { 0x00FFFFFF } else { 0x00000000 };
                // SAFETY: writing to MMIO framebuffer within bounds
                unsafe {
                    core::ptr::write_volatile(mmio.add(off) as *mut u32, color);
                }
            }
        }

        self.drawn_x = self.x;
        self.drawn_y = self.y;
        self.drawn = true;
    }
}

// ── Shared cursor-drawn state (used by both lock-free and inner paths) ──

static DRAWN_LF_X: AtomicI32 = AtomicI32::new(0);
static DRAWN_LF_Y: AtomicI32 = AtomicI32::new(0);
static DRAWN_LF: AtomicBool = AtomicBool::new(false);

/// Truly lock-free cursor redraw: NO CONSOLE lock, uses cached atomic pointers.
/// Called from Core 0 after update_atomic(). Never blocks on render_frame.
pub fn redraw_overlay_lockfree() {
    if !take_dirty() { return; }

    let shadow = crate::framebuffer::cached_shadow_front();
    if shadow.is_null() { return; }

    let mmio_addr = IRQ_FB_ADDR.load(Ordering::Relaxed);
    if mmio_addr == 0 { return; }
    let mmio = mmio_addr as *mut u8;
    let pitch = IRQ_FB_PITCH.load(Ordering::Relaxed) as usize;
    let sw = SCREEN_W.load(Ordering::Relaxed);
    let sh = SCREEN_H.load(Ordering::Relaxed);

    // Restore old cursor area from shadow_front → MMIO
    if DRAWN_LF.load(Ordering::Relaxed) {
        let dx = DRAWN_LF_X.load(Ordering::Relaxed);
        let dy = DRAWN_LF_Y.load(Ordering::Relaxed);
        blit_shadow_to_mmio(shadow, mmio, pitch, sw, sh, dx, dy, CURSOR_W, CURSOR_H);
    }

    // Draw cursor at current atomic position
    let (cx, cy) = atomic_pos();
    draw_cursor_on_mmio(mmio, pitch, sw, sh, cx, cy);

    DRAWN_LF_X.store(cx, Ordering::Relaxed);
    DRAWN_LF_Y.store(cy, Ordering::Relaxed);
    DRAWN_LF.store(true, Ordering::Relaxed);
}

/// Inner cursor draw — called from render paths that already hold CONSOLE lock.
/// Uses shared DRAWN_LF state so lock-free path and inner path stay in sync.
pub fn redraw_overlay_lockfree_inner(fb: &mut crate::framebuffer::FbConsole) {
    let info = fb.info();
    let (shadow, _) = fb.shadow_ptr();
    let mmio = info.addr as *mut u8;
    let pitch = info.pitch as usize;
    let sw = info.width as i32;
    let sh = info.height as i32;

    // Restore old cursor area from front → MMIO
    if DRAWN_LF.load(Ordering::Relaxed) {
        let dx = DRAWN_LF_X.load(Ordering::Relaxed);
        let dy = DRAWN_LF_Y.load(Ordering::Relaxed);
        blit_shadow_to_mmio(shadow, mmio, pitch, sw, sh, dx, dy, CURSOR_W, CURSOR_H);
    }

    // Draw cursor at current atomic position
    let (cx, cy) = atomic_pos();
    draw_cursor_on_mmio(mmio, pitch, sw, sh, cx, cy);

    DRAWN_LF_X.store(cx, Ordering::Relaxed);
    DRAWN_LF_Y.store(cy, Ordering::Relaxed);
    DRAWN_LF.store(true, Ordering::Relaxed);
}

/// Draw cursor from IRQ context — no locks, no shadow restore.
/// Writes directly to MMIO using cached framebuffer info.
/// Any trail artifact is cleaned up by the next render_frame().
pub fn draw_cursor_irq() {
    let addr = IRQ_FB_ADDR.load(Ordering::Relaxed);
    if addr == 0 { return; }
    let pitch = IRQ_FB_PITCH.load(Ordering::Relaxed) as usize;
    let sw = SCREEN_W.load(Ordering::Relaxed);
    let sh = SCREEN_H.load(Ordering::Relaxed);
    let (cx, cy) = atomic_pos();
    draw_cursor_on_mmio(addr as *mut u8, pitch, sw, sh, cx, cy);
}

/// Draw cursor after scene blit. Erases old position ONLY if cursor moved
/// (no blink when stationary, no ghost when moved).
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
            blit_shadow_to_mmio(shadow, mmio, pitch, sw, sh, dx, dy, CURSOR_W, CURSOR_H);
        }
    }

    draw_cursor_on_mmio(mmio, pitch, sw, sh, cx, cy);

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
fn draw_cursor_on_mmio(mmio: *mut u8, pitch: usize, sw: i32, sh: i32, x: i32, y: i32) {
    for row in 0..CURSOR_H as i32 {
        let py = y + row;
        if py < 0 || py >= sh { continue; }
        for col in 0..CURSOR_W as i32 {
            let px = x + col;
            if px < 0 || px >= sw { continue; }
            let bmp = CURSOR_BITMAP[(row as usize) * CURSOR_W as usize + col as usize];
            if bmp == 0 { continue; }
            let off = py as usize * pitch + px as usize * 4;
            let color: u32 = if bmp == 1 { 0x00FFFFFF } else { 0x00000000 };
            // SAFETY: writing to MMIO framebuffer within bounds
            unsafe { core::ptr::write_volatile(mmio.add(off) as *mut u32, color); }
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
