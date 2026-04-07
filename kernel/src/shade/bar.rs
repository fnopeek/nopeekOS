//! ShadeBar — status bar for shade compositor.
//!
//! Waybar-inspired: workspace indicators, window title, system info (clock, etc.).
//! Rendered natively in kernel; position and content configurable.

use alloc::format;
use alloc::string::String;
use crate::framebuffer::FbInfo;
use crate::gui::{background, color::Theme, font, render};

/// Bar position on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarPosition {
    Top,
    Bottom,
}

/// ShadeBar state.
#[allow(dead_code)]
pub struct ShadeBar {
    pub position: BarPosition,
    /// Bar height in pixels (scaled).
    pub height: u32,
    /// Scale factor (1x or 2x).
    pub scale: u32,
    /// Number of workspaces.
    pub workspace_count: u8,
    /// Currently active workspace (0-based).
    pub active_workspace: u8,
    /// Title of the focused window.
    pub focused_title: String,
    /// Whether bar needs redraw.
    pub dirty: bool,
}

#[allow(dead_code)]
impl ShadeBar {
    pub fn new(scale: u32) -> Self {
        let base_height = crate::config::get("shade.bar_height")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(28);
        let position = match crate::config::get("shade.bar_position").as_deref() {
            Some("bottom") => BarPosition::Bottom,
            _ => BarPosition::Top,
        };

        ShadeBar {
            position,
            height: base_height * scale,
            scale,
            workspace_count: 4,
            active_workspace: 0,
            focused_title: String::new(),
            dirty: true,
        }
    }

    /// Y coordinate of the bar on screen.
    pub fn y(&self, screen_h: u32) -> u32 {
        match self.position {
            BarPosition::Top => 0,
            BarPosition::Bottom => screen_h.saturating_sub(self.height),
        }
    }

    /// Usable area start Y (below/above bar).
    pub fn workspace_y(&self) -> u32 {
        match self.position {
            BarPosition::Top => self.height,
            BarPosition::Bottom => 0,
        }
    }

    /// Usable area height (screen minus bar).
    pub fn workspace_height(&self, screen_h: u32) -> u32 {
        screen_h.saturating_sub(self.height)
    }

    /// Set active workspace.
    pub fn set_workspace(&mut self, ws: u8) {
        if ws != self.active_workspace {
            self.active_workspace = ws;
            self.dirty = true;
        }
    }

    /// Set focused window title.
    pub fn set_title(&mut self, title: &str) {
        if self.focused_title != title {
            self.focused_title = String::from(title);
            self.dirty = true;
        }
    }

    /// Render the bar onto the shadow buffer.
    pub fn render(&mut self, shadow: *mut u8, info: &FbInfo, screen_w: u32, screen_h: u32) {
        let bar_y = self.y(screen_h);
        let accent = background::accent_color();

        // Background: dark semi-transparent strip
        render::fill_rect(shadow, info, 0, bar_y, screen_w, self.height, 0x00101018);

        // Bottom border line (1px accent)
        let border_y = match self.position {
            BarPosition::Top => bar_y + self.height - self.scale,
            BarPosition::Bottom => bar_y,
        };
        render::fill_rect(shadow, info, 0, border_y, screen_w, self.scale, accent);

        let padding = 8 * self.scale;
        let font_scale = 1.min(self.scale); // bar uses small font

        // === Left: Workspace indicators ===
        let ws_size = 12 * self.scale;
        let ws_gap = 6 * self.scale;
        let ws_y = bar_y + (self.height.saturating_sub(ws_size)) / 2;
        let mut x = padding;

        for i in 0..self.workspace_count {
            let color = if i == self.active_workspace {
                accent
            } else {
                0x00333344
            };
            // Rounded workspace indicator
            let radius = 3 * self.scale;
            render::fill_rounded_rect_aa(shadow, info, x, ws_y, ws_size, ws_size, color, radius);

            // Workspace number
            let num = format!("{}", i + 1);
            let text_color = if i == self.active_workspace { 0x00FFFFFF } else { 0x00888899 };
            let (cw, ch) = font::char_size(1);
            let tx = x + (ws_size.saturating_sub(cw)) / 2;
            let ty = ws_y + (ws_size.saturating_sub(ch)) / 2;
            font::draw_str(shadow, info, &num, tx, ty, text_color, None, 1);

            x += ws_size + ws_gap;
        }

        // === Center: Focused window title ===
        if !self.focused_title.is_empty() {
            let (_, ch) = font::char_size(1);
            let ty = bar_y + (self.height.saturating_sub(ch)) / 2;
            font::draw_str_centered(shadow, info,
                &self.focused_title, 0, screen_w, ty,
                Theme::FG_PRIMARY, None, 1);
        }

        // === Right: Clock + system info ===
        let time_str = self.format_time();
        let (_, ch) = font::char_size(1);
        let time_w = font::measure_str(&time_str, 1);
        let tx = screen_w.saturating_sub(time_w + padding);
        let ty = bar_y + (self.height.saturating_sub(ch)) / 2;
        font::draw_str(shadow, info, &time_str, tx, ty, Theme::FG_PRIMARY, None, 1);

        self.dirty = false;
    }

    /// Format current time for display.
    fn format_time(&self) -> String {
        let unix = crate::rtc::read_unix_time().unwrap_or(0);
        let tz_minutes = crate::config::timezone_offset_minutes();
        let local = unix as i64 + tz_minutes as i64 * 60;
        let secs_today = ((local % 86400) + 86400) % 86400;
        let hour = secs_today / 3600;
        let min = (secs_today % 3600) / 60;
        format!("{:02}:{:02}", hour, min)
    }
}
