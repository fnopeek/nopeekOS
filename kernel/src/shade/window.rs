//! Window management for shade compositor.
//!
//! Each window owns a pixel buffer (shadow buffer region).
//! Windows are stacked in Z-order; the compositor blits them onto the
//! main shadow buffer during render.

use alloc::string::String;
use alloc::vec::Vec;

/// Unique window identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowId(pub u32);

/// Window state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    /// Normal tiled window.
    Tiled,
    /// Floating (manual position).
    Floating,
    /// Fullscreen (covers entire workspace area).
    Fullscreen,
}

/// A single window managed by the compositor.
#[allow(dead_code)]
pub struct Window {
    pub id: WindowId,
    pub title: String,
    /// Position relative to screen origin (set by WM layout).
    pub x: u32,
    pub y: u32,
    /// Outer dimensions (including border).
    pub width: u32,
    pub height: u32,
    /// Window content buffer (RGBA pixels, width*height*4 bytes).
    pub buffer: Vec<u8>,
    /// Buffer dimensions (content area, excluding border).
    pub buf_w: u32,
    pub buf_h: u32,
    pub state: WindowState,
    pub visible: bool,
    pub focused: bool,
    /// True if content changed since last render.
    pub dirty: bool,
    /// Workspace index (0-based).
    pub workspace: u8,
}

#[allow(dead_code)]
impl Window {
    /// Create a new window with an empty (black) content buffer.
    pub fn new(id: WindowId, title: &str, x: u32, y: u32, w: u32, h: u32, border: u32) -> Self {
        let buf_w = w.saturating_sub(border * 2);
        let buf_h = h.saturating_sub(border * 2);
        let buf_size = (buf_w * buf_h * 4) as usize;

        Window {
            id,
            title: String::from(title),
            x,
            y,
            width: w,
            height: h,
            buffer: alloc::vec![0u8; buf_size],
            buf_w,
            buf_h,
            state: WindowState::Tiled,
            visible: true,
            focused: false,
            dirty: true,
            workspace: 0,
        }
    }

    /// Content area origin (inside border).
    pub fn content_x(&self, border: u32) -> u32 {
        self.x + border
    }

    pub fn content_y(&self, border: u32) -> u32 {
        self.y + border
    }

    /// Write a pixel into the window's content buffer.
    pub fn put_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x >= self.buf_w || y >= self.buf_h { return; }
        let offset = (y * self.buf_w + x) as usize * 4;
        if offset + 4 <= self.buffer.len() {
            // Store as 0x00RRGGBB (same format as framebuffer)
            self.buffer[offset] = (color & 0xFF) as u8;          // B
            self.buffer[offset + 1] = ((color >> 8) & 0xFF) as u8;  // G
            self.buffer[offset + 2] = ((color >> 16) & 0xFF) as u8; // R
            self.buffer[offset + 3] = 0xFF;                         // A
            self.dirty = true;
        }
    }

    /// Fill a rectangle in the window's content buffer.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        let x_end = (x + w).min(self.buf_w);
        let y_end = (y + h).min(self.buf_h);
        let b = (color & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let r = ((color >> 16) & 0xFF) as u8;

        for row in y..y_end {
            for col in x..x_end {
                let offset = (row * self.buf_w + col) as usize * 4;
                if offset + 4 <= self.buffer.len() {
                    self.buffer[offset] = b;
                    self.buffer[offset + 1] = g;
                    self.buffer[offset + 2] = r;
                    self.buffer[offset + 3] = 0xFF;
                }
            }
        }
        self.dirty = true;
    }

    /// Clear the entire content buffer to a color.
    pub fn clear(&mut self, color: u32) {
        self.fill_rect(0, 0, self.buf_w, self.buf_h, color);
    }

    /// Blit the window's content buffer onto the compositor shadow buffer.
    pub fn blit_to(&self, shadow: *mut u8, info: &crate::framebuffer::FbInfo, border: u32) {
        if !self.visible { return; }

        let cx = self.content_x(border);
        let cy = self.content_y(border);

        for row in 0..self.buf_h {
            let dst_y = cy + row;
            if dst_y >= info.height { break; }
            let src_offset = (row * self.buf_w) as usize * 4;
            let dst_offset = (dst_y * info.pitch + cx * 4) as usize;

            for col in 0..self.buf_w {
                let dst_x = cx + col;
                if dst_x >= info.width { break; }
                let si = src_offset + col as usize * 4;
                if si + 3 >= self.buffer.len() { break; }
                let color = self.buffer[si] as u32
                    | ((self.buffer[si + 1] as u32) << 8)
                    | ((self.buffer[si + 2] as u32) << 16);
                let di = dst_offset + col as usize * 4;
                // SAFETY: bounds checked via info.width/height
                unsafe { *(shadow.add(di) as *mut u32) = color; }
            }
        }
    }
}
