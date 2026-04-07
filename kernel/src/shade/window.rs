//! Window management for shade compositor.
//!
//! Windows are metadata (position, size, title, state). No per-window pixel
//! buffers — the compositor renders directly to the framebuffer shadow buffer.
//! This is efficient for tiling WMs where windows don't overlap.

use alloc::string::String;

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
/// No pixel buffer — rendering goes directly to the shadow buffer.
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
    /// Background color for the content area.
    pub bg_color: u32,
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
    /// Create a new window (metadata only, no pixel buffer).
    pub fn new(id: WindowId, title: &str, x: u32, y: u32, w: u32, h: u32) -> Self {
        Window {
            id,
            title: String::from(title),
            x,
            y,
            width: w,
            height: h,
            bg_color: 0x00101018,
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

    /// Content area dimensions (excluding border).
    pub fn content_w(&self, border: u32) -> u32 {
        self.width.saturating_sub(border * 2)
    }

    pub fn content_h(&self, border: u32) -> u32 {
        self.height.saturating_sub(border * 2)
    }

    /// Render the window content area directly to the shadow buffer.
    pub fn render_to(&self, shadow: *mut u8, info: &crate::framebuffer::FbInfo, border: u32) {
        if !self.visible { return; }

        let cx = self.content_x(border);
        let cy = self.content_y(border);
        let cw = self.content_w(border);
        let ch = self.content_h(border);

        // Fill content area with background color
        crate::gui::render::fill_rect(shadow, info, cx, cy, cw, ch, self.bg_color);
    }
}
