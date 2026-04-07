//! Software mouse cursor — drawn on shadow buffer with save/restore.
//!
//! Arrow cursor (12x19) rendered at mouse position. The area under the cursor
//! is saved before drawing and restored before redrawing at the new position.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

/// Cursor dimensions.
const CURSOR_W: u32 = 12;
const CURSOR_H: u32 = 19;
const CURSOR_PIXELS: usize = (CURSOR_W * CURSOR_H) as usize;

/// Arrow cursor bitmap (1 = white outline, 2 = black fill, 0 = transparent).
static CURSOR_BITMAP: [u8; CURSOR_PIXELS] = [
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

/// Mouse state — position and buttons.
pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
    pub prev_buttons: u8,
    pub screen_w: u32,
    pub screen_h: u32,
    /// Saved pixels under cursor (ARGB u32 values).
    saved: [u32; CURSOR_PIXELS],
    /// Position where saved pixels were captured.
    pub saved_x: i32,
    pub saved_y: i32,
    /// Whether cursor is currently drawn on the shadow buffer.
    pub visible: bool,
}

impl MouseState {
    pub const fn new() -> Self {
        MouseState {
            x: 0,
            y: 0,
            buttons: 0,
            prev_buttons: 0,
            screen_w: 0,
            screen_h: 0,
            saved: [0; CURSOR_PIXELS],
            saved_x: -1,
            saved_y: -1,
            visible: false,
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

    /// Left button just pressed (edge).
    pub fn left_clicked(&self) -> bool {
        (self.buttons & 1) != 0 && (self.prev_buttons & 1) == 0
    }

    /// Right button just pressed (edge).
    pub fn right_clicked(&self) -> bool {
        (self.buttons & 2) != 0 && (self.prev_buttons & 2) == 0
    }

    /// Left button held.
    pub fn left_held(&self) -> bool {
        (self.buttons & 1) != 0
    }

    /// Right button held.
    pub fn right_held(&self) -> bool {
        (self.buttons & 2) != 0
    }

    /// Left button just released (edge).
    pub fn left_released(&self) -> bool {
        (self.buttons & 1) == 0 && (self.prev_buttons & 1) != 0
    }

    /// Right button just released (edge).
    pub fn right_released(&self) -> bool {
        (self.buttons & 2) == 0 && (self.prev_buttons & 2) != 0
    }

    /// Erase cursor from shadow buffer (restore saved pixels).
    pub fn erase(&mut self, shadow: *mut u8, info: &crate::framebuffer::FbInfo) {
        if !self.visible { return; }
        self.visible = false;

        let stride = (info.pitch / 4) as usize; // pixels per row
        let sw = info.width as i32;
        let sh = info.height as i32;

        for row in 0..CURSOR_H as i32 {
            let py = self.saved_y + row;
            if py < 0 || py >= sh { continue; }
            for col in 0..CURSOR_W as i32 {
                let px = self.saved_x + col;
                if px < 0 || px >= sw { continue; }
                let idx = (row as usize) * CURSOR_W as usize + col as usize;
                if CURSOR_BITMAP[idx] == 0 { continue; }
                let off = (py as usize * stride + px as usize) * 4;
                // SAFETY: writing to shadow buffer within bounds
                unsafe {
                    let p = shadow.add(off) as *mut u32;
                    core::ptr::write_volatile(p, self.saved[idx]);
                }
            }
        }
    }

    /// Draw cursor onto shadow buffer (saves pixels underneath first).
    pub fn draw(&mut self, shadow: *mut u8, info: &crate::framebuffer::FbInfo) {
        self.saved_x = self.x;
        self.saved_y = self.y;
        self.visible = true;

        let stride = (info.pitch / 4) as usize; // pixels per row
        let sw = info.width as i32;
        let sh = info.height as i32;

        for row in 0..CURSOR_H as i32 {
            let py = self.y + row;
            if py < 0 || py >= sh { continue; }
            for col in 0..CURSOR_W as i32 {
                let px = self.x + col;
                if px < 0 || px >= sw { continue; }
                let idx = (row as usize) * CURSOR_W as usize + col as usize;
                let bmp = CURSOR_BITMAP[idx];
                if bmp == 0 { continue; }
                let off = (py as usize * stride + px as usize) * 4;
                // SAFETY: reading/writing shadow buffer within bounds
                unsafe {
                    let p = shadow.add(off) as *mut u32;
                    self.saved[idx] = core::ptr::read_volatile(p);
                    let color = if bmp == 1 { 0x00FFFFFF } else { 0x00000000 };
                    core::ptr::write_volatile(p, color);
                }
            }
        }
    }

    /// Blit region covering old + new cursor positions to framebuffer.
    pub fn blit_cursor_regions(&self, fb: &mut crate::framebuffer::FbConsole) {
        // Blit new cursor region
        let x = self.x.max(0) as u32;
        let y = self.y.max(0) as u32;
        let w = CURSOR_W.min(self.screen_w.saturating_sub(x));
        let h = CURSOR_H.min(self.screen_h.saturating_sub(y));
        if w > 0 && h > 0 {
            crate::framebuffer::blit_rect(fb, x, y, w, h);
        }

        // Blit old cursor region if different
        if self.saved_x != self.x || self.saved_y != self.y {
            let ox = self.saved_x.max(0) as u32;
            let oy = self.saved_y.max(0) as u32;
            let ow = CURSOR_W.min(self.screen_w.saturating_sub(ox));
            let oh = CURSOR_H.min(self.screen_h.saturating_sub(oy));
            if ow > 0 && oh > 0 {
                crate::framebuffer::blit_rect(fb, ox, oy, ow, oh);
            }
        }
    }
}

/// Cursor width (for hit-test offsets).
pub const fn cursor_width() -> u32 { CURSOR_W }
/// Cursor height.
pub const fn cursor_height() -> u32 { CURSOR_H }
