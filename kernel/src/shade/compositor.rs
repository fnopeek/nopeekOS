//! Compositor — manages windows, Z-order, tiling layout, and rendering.
//!
//! The compositor owns all windows and the shadebar. It handles:
//! - Window creation/destruction
//! - Focus management
//! - Tiling layout (binary split, Hyprland-style)
//! - Damage tracking (only redraw changed regions)
//! - Compositing window buffers onto the main shadow buffer

use alloc::vec::Vec;
use crate::framebuffer::FbInfo;
use crate::gui::{background, color::Theme, render};

use super::window::{Window, WindowId, WindowState};
use super::bar::ShadeBar;

/// Compositor manages all windows, the bar, and rendering state.
#[allow(dead_code)]
pub struct Compositor {
    /// Screen dimensions.
    pub screen_w: u32,
    pub screen_h: u32,
    /// Pixel scale (1x or 2x for 4K).
    pub scale: u32,
    /// All managed windows.
    pub windows: Vec<Window>,
    /// Z-order: front-to-back window IDs. First = topmost.
    pub z_order: Vec<WindowId>,
    /// Next window ID counter.
    next_id: u32,
    /// Currently focused window.
    pub focused: Option<WindowId>,
    /// Active workspace (0-based).
    pub active_workspace: u8,
    /// Status bar.
    pub bar: ShadeBar,
    /// Gap between tiled windows (in pixels, scaled).
    pub gaps: u32,
    /// Window border width (in pixels, scaled).
    pub border: u32,
    /// Active window border color.
    pub border_active: u32,
    /// Inactive window border color.
    pub border_inactive: u32,
    /// Full redraw needed.
    pub needs_full_redraw: bool,
}

#[allow(dead_code)]
impl Compositor {
    pub fn new(screen_w: u32, screen_h: u32, scale: u32) -> Self {
        let gaps = crate::config::get("shade.gaps")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(8) * scale;
        let border = crate::config::get("shade.border")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(2) * scale;
        let border_active = crate::config::get("shade.border_active")
            .and_then(|s| parse_hex_color(&s))
            .unwrap_or_else(|| background::accent_color());
        let border_inactive = crate::config::get("shade.border_inactive")
            .and_then(|s| parse_hex_color(&s))
            .unwrap_or(0x003A2555);

        Compositor {
            screen_w,
            screen_h,
            scale,
            windows: Vec::new(),
            z_order: Vec::new(),
            next_id: 1,
            focused: None,
            active_workspace: 0,
            bar: ShadeBar::new(scale),
            gaps,
            border,
            border_active,
            border_inactive,
            needs_full_redraw: true,
        }
    }

    /// Usable workspace area (excluding bar).
    fn workspace_area(&self) -> (u32, u32, u32, u32) {
        let x = self.gaps;
        let y = self.bar.workspace_y() + self.gaps;
        let w = self.screen_w.saturating_sub(self.gaps * 2);
        let h = self.bar.workspace_height(self.screen_h).saturating_sub(self.gaps * 2);
        (x, y, w, h)
    }

    /// Create a new window and add it to the current workspace.
    pub fn create_window(&mut self, title: &str, x: u32, y: u32, w: u32, h: u32) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;

        let mut win = Window::new(id, title, x, y, w, h, self.border);
        win.workspace = self.active_workspace;
        win.clear(0x00101018); // Dark default background

        self.windows.push(win);
        self.z_order.insert(0, id); // New window on top
        self.focus_window(id);
        self.retile();
        self.needs_full_redraw = true;

        id
    }

    /// Close a window by ID.
    pub fn close_window(&mut self, id: WindowId) {
        self.windows.retain(|w| w.id != id);
        self.z_order.retain(|&wid| wid != id);

        if self.focused == Some(id) {
            // Focus next window in Z-order
            self.focused = self.z_order.first()
                .and_then(|&top_id| {
                    self.windows.iter().find(|w| w.id == top_id && w.workspace == self.active_workspace)
                })
                .map(|w| w.id);
            if let Some(fid) = self.focused {
                self.set_focused_flag(fid);
            }
        }

        self.retile();
        self.needs_full_redraw = true;
    }

    /// Set focus to a window.
    pub fn focus_window(&mut self, id: WindowId) {
        self.focused = Some(id);
        self.set_focused_flag(id);

        // Move to front of Z-order
        self.z_order.retain(|&wid| wid != id);
        self.z_order.insert(0, id);

        // Update bar title
        if let Some(win) = self.windows.iter().find(|w| w.id == id) {
            self.bar.set_title(&win.title);
        }

        self.needs_full_redraw = true;
    }

    /// Switch to workspace.
    pub fn switch_workspace(&mut self, ws: u8) {
        if ws == self.active_workspace { return; }
        self.active_workspace = ws;
        self.bar.set_workspace(ws);

        // Focus first window on new workspace
        self.focused = self.z_order.iter()
            .find(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == ws))
            .copied();

        if let Some(fid) = self.focused {
            self.set_focused_flag(fid);
            if let Some(win) = self.windows.iter().find(|w| w.id == fid) {
                self.bar.set_title(&win.title);
            }
        } else {
            self.bar.set_title("");
        }

        self.retile();
        self.needs_full_redraw = true;
    }

    /// Move the focused window to a different workspace.
    pub fn move_to_workspace(&mut self, ws: u8) {
        if let Some(fid) = self.focused {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == fid) {
                win.workspace = ws;
                win.dirty = true;
            }
            self.retile();
            self.needs_full_redraw = true;
        }
    }

    /// Recalculate tiled window positions for the active workspace.
    /// Uses a simple binary split layout (master-stack like Hyprland default).
    pub fn retile(&mut self) {
        let (area_x, area_y, area_w, area_h) = self.workspace_area();

        // Collect tiled windows on active workspace
        let tiled: Vec<WindowId> = self.windows.iter()
            .filter(|w| w.workspace == self.active_workspace
                     && w.state == WindowState::Tiled
                     && w.visible)
            .map(|w| w.id)
            .collect();

        if tiled.is_empty() { return; }

        let gap = self.gaps;
        let n = tiled.len();

        let border = self.border;

        if n == 1 {
            // Single window: fill entire workspace area
            for win in &mut self.windows {
                if win.id == tiled[0] {
                    win.x = area_x;
                    win.y = area_y;
                    win.width = area_w;
                    win.height = area_h;
                    Self::resize_window_buffer(win, border);
                    break;
                }
            }
        } else {
            // Master-stack: first window gets left half, rest stack on right
            let master_w = (area_w - gap) / 2;
            let stack_w = area_w - master_w - gap;
            let stack_count = n - 1;
            let stack_h = (area_h - gap * (stack_count as u32 - 1)) / stack_count as u32;

            for win in &mut self.windows {
                if win.id == tiled[0] {
                    // Master window
                    win.x = area_x;
                    win.y = area_y;
                    win.width = master_w;
                    win.height = area_h;
                    Self::resize_window_buffer(win, border);
                } else if let Some(si) = tiled[1..].iter().position(|&t| t == win.id) {
                    // Stack window
                    win.x = area_x + master_w + gap;
                    win.y = area_y + si as u32 * (stack_h + gap);
                    win.width = stack_w;
                    win.height = stack_h;
                    Self::resize_window_buffer(win, border);
                }
            }
        }
    }

    /// Resize a window's content buffer to match its current dimensions.
    fn resize_window_buffer(win: &mut Window, border: u32) {
        let new_buf_w = win.width.saturating_sub(border * 2);
        let new_buf_h = win.height.saturating_sub(border * 2);
        if new_buf_w != win.buf_w || new_buf_h != win.buf_h {
            win.buf_w = new_buf_w;
            win.buf_h = new_buf_h;
            let new_size = (new_buf_w * new_buf_h * 4) as usize;
            win.buffer = alloc::vec![0u8; new_size];
            win.clear(0x00101018);
        }
    }

    /// Render the full compositor scene.
    pub fn render(&mut self, shadow: *mut u8, info: &FbInfo) {
        // Background (aurora)
        background::draw_aurora(shadow, info);

        // Render windows (back to front)
        for &wid in self.z_order.iter().rev() {
            if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                if win.workspace != self.active_workspace || !win.visible { continue; }

                // Draw border
                let border_color = if win.focused { self.border_active } else { self.border_inactive };
                render::fill_rect(shadow, info, win.x, win.y, win.width, win.height, border_color);

                // Blit window content
                win.blit_to(shadow, info, self.border);
            }
        }

        // Shadebar
        self.bar.render(shadow, info, self.screen_w, self.screen_h);

        // Mark all windows as clean
        for win in &mut self.windows {
            win.dirty = false;
        }
        self.needs_full_redraw = false;
    }

    /// Render only changed regions. Returns list of (x, y, w, h) blitted.
    pub fn render_damaged(&mut self, shadow: *mut u8, info: &FbInfo) -> Vec<(u32, u32, u32, u32)> {
        if self.needs_full_redraw {
            self.render(shadow, info);
            return alloc::vec![(0, 0, self.screen_w, self.screen_h)];
        }

        let mut regions = Vec::new();

        // Check for dirty windows
        for wid_idx in (0..self.z_order.len()).rev() {
            let wid = self.z_order[wid_idx];
            let needs_render = self.windows.iter()
                .find(|w| w.id == wid)
                .map(|w| w.dirty && w.workspace == self.active_workspace && w.visible)
                .unwrap_or(false);

            if needs_render {
                if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                    // Redraw aurora background for this region
                    background::draw_aurora_region(shadow, info, win.x, win.y, win.width, win.height);

                    // Redraw border
                    let border_color = if win.focused { self.border_active } else { self.border_inactive };
                    render::fill_rect(shadow, info, win.x, win.y, win.width, win.height, border_color);

                    // Blit content
                    win.blit_to(shadow, info, self.border);
                    regions.push((win.x, win.y, win.width, win.height));
                }
            }
        }

        // Redraw bar if dirty
        if self.bar.dirty {
            self.bar.render(shadow, info, self.screen_w, self.screen_h);
            let bar_y = self.bar.y(self.screen_h);
            regions.push((0, bar_y, self.screen_w, self.bar.height));
        }

        // Mark all clean
        for win in &mut self.windows {
            win.dirty = false;
        }

        regions
    }

    /// Get a mutable reference to a window by ID.
    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Get a reference to a window by ID.
    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Count of windows on the active workspace.
    pub fn window_count(&self) -> usize {
        self.windows.iter()
            .filter(|w| w.workspace == self.active_workspace && w.visible)
            .count()
    }

    /// Update focused flag on all windows.
    fn set_focused_flag(&mut self, focused_id: WindowId) {
        for win in &mut self.windows {
            win.focused = win.id == focused_id;
            win.dirty = true;
        }
    }

    /// Focus next window on active workspace (cycle).
    pub fn focus_next(&mut self) {
        let ws_windows: Vec<WindowId> = self.z_order.iter()
            .filter(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == self.active_workspace && w.visible))
            .copied()
            .collect();

        if ws_windows.len() < 2 { return; }

        let current_idx = self.focused
            .and_then(|fid| ws_windows.iter().position(|&wid| wid == fid))
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % ws_windows.len();
        self.focus_window(ws_windows[next_idx]);
    }

    /// Focus previous window on active workspace (cycle).
    pub fn focus_prev(&mut self) {
        let ws_windows: Vec<WindowId> = self.z_order.iter()
            .filter(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == self.active_workspace && w.visible))
            .copied()
            .collect();

        if ws_windows.len() < 2 { return; }

        let current_idx = self.focused
            .and_then(|fid| ws_windows.iter().position(|&wid| wid == fid))
            .unwrap_or(0);
        let prev_idx = if current_idx == 0 { ws_windows.len() - 1 } else { current_idx - 1 };
        self.focus_window(ws_windows[prev_idx]);
    }
}

/// Parse a hex color string ("RRGGBB") to u32.
fn parse_hex_color(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches('#');
    if s.len() != 6 { return None; }
    u32::from_str_radix(s, 16).ok()
}
