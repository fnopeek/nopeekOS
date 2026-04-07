//! Compositor — manages windows, Z-order, tiling layout, and rendering.
//!
//! No per-window pixel buffers. Windows are metadata (position, size, state).
//! The compositor renders directly to the framebuffer shadow buffer.
//! Uses dwindle layout (Hyprland-style recursive binary split).

use alloc::vec::Vec;
use crate::framebuffer::FbInfo;
use crate::gui::{background, render};

use super::window::{Window, WindowId, WindowState};
use super::bar::ShadeBar;
use super::terminal;

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
    /// Corner radius (in pixels, scaled).
    pub rounding: u32,
    /// Window background opacity (0=transparent, 256=opaque).
    pub opacity: u32,
    /// Full redraw needed (including aurora background).
    pub needs_full_redraw: bool,
    /// Aurora background has been drawn (skip on partial updates).
    aurora_drawn: bool,
}

#[allow(dead_code)]
impl Compositor {
    pub fn new(screen_w: u32, screen_h: u32, scale: u32) -> Self {
        let gaps = crate::config::get("shade.gaps")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(8) * scale;
        let border = crate::config::get("shade.border")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1) * scale;
        let border_active = crate::config::get("shade.border_active")
            .and_then(|s| parse_hex_color(&s))
            .unwrap_or_else(|| background::accent_color());
        let border_inactive = crate::config::get("shade.border_inactive")
            .and_then(|s| parse_hex_color(&s))
            .unwrap_or(0x003A2555);
        let rounding = crate::config::get("shade.rounding")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(10) * scale;
        let opacity = crate::config::get("shade.opacity")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(160);

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
            rounding,
            opacity,
            needs_full_redraw: true,
            aurora_drawn: false,
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

        let mut win = Window::new(id, title, x, y, w, h);
        win.workspace = self.active_workspace;
        win.terminal_idx = terminal::allocate().unwrap_or(0);

        self.windows.push(win);
        self.z_order.insert(0, id);
        self.focus_window(id);
        self.retile();
        self.needs_full_redraw = true;

        id
    }

    /// Close a window by ID.
    pub fn close_window(&mut self, id: WindowId) {
        // Free terminal buffer before removing window
        if let Some(win) = self.windows.iter().find(|w| w.id == id) {
            terminal::free(win.terminal_idx);
        }
        self.windows.retain(|w| w.id != id);
        self.z_order.retain(|&wid| wid != id);

        if self.focused == Some(id) {
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

        self.z_order.retain(|&wid| wid != id);
        self.z_order.insert(0, id);

        if let Some(win) = self.windows.iter().find(|w| w.id == id) {
            self.bar.set_title(&win.title);
            terminal::set_active_terminal(win.terminal_idx);
        }
        // Don't set needs_full_redraw — render_damaged handles 2 windows only
    }

    /// Switch to workspace.
    pub fn switch_workspace(&mut self, ws: u8) {
        if ws == self.active_workspace { return; }
        self.active_workspace = ws;
        self.bar.set_workspace(ws);

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

    /// Dwindle tiling: recursively split space in half for each window.
    /// 1 window = full area. 2 = left/right split. 3 = left + right split top/bottom. etc.
    pub fn retile(&mut self) {
        let (area_x, area_y, area_w, area_h) = self.workspace_area();

        let tiled: Vec<WindowId> = self.windows.iter()
            .filter(|w| w.workspace == self.active_workspace
                     && w.state == WindowState::Tiled
                     && w.visible)
            .map(|w| w.id)
            .collect();

        if tiled.is_empty() { return; }

        let gap = self.gaps;
        self.dwindle_layout(&tiled, area_x, area_y, area_w, area_h, gap, true);
    }

    /// Recursive dwindle: assign position to first window, recurse for rest.
    fn dwindle_layout(&mut self, ids: &[WindowId],
                      x: u32, y: u32, w: u32, h: u32,
                      gap: u32, split_horizontal: bool) {
        if ids.is_empty() { return; }

        if ids.len() == 1 {
            for win in &mut self.windows {
                if win.id == ids[0] {
                    win.x = x;
                    win.y = y;
                    win.width = w;
                    win.height = h;
                    win.dirty = true;
                    break;
                }
            }
            return;
        }

        // Split: first window takes one half, rest take the other half
        if split_horizontal {
            let left_w = (w.saturating_sub(gap)) / 2;
            let right_w = w.saturating_sub(left_w + gap);
            // First window: left half
            for win in &mut self.windows {
                if win.id == ids[0] {
                    win.x = x;
                    win.y = y;
                    win.width = left_w;
                    win.height = h;
                    win.dirty = true;
                    break;
                }
            }
            // Rest: right half, split vertically next time
            self.dwindle_layout(&ids[1..], x + left_w + gap, y, right_w, h, gap, false);
        } else {
            let top_h = (h.saturating_sub(gap)) / 2;
            let bottom_h = h.saturating_sub(top_h + gap);
            // First window: top half
            for win in &mut self.windows {
                if win.id == ids[0] {
                    win.x = x;
                    win.y = y;
                    win.width = w;
                    win.height = top_h;
                    win.dirty = true;
                    break;
                }
            }
            // Rest: bottom half, split horizontally next time
            self.dwindle_layout(&ids[1..], x, y + top_h + gap, w, bottom_h, gap, true);
        }
    }

    /// Render the full compositor scene to the shadow buffer.
    pub fn render(&mut self, shadow: *mut u8, info: &FbInfo) {
        // Only redraw aurora when needed (expensive at 4K)
        if !self.aurora_drawn || self.needs_full_redraw {
            background::draw_aurora(shadow, info);
            self.aurora_drawn = true;
        }

        // Render windows (back to front)
        let border = self.border;
        let rounding = self.rounding;
        let opacity = self.opacity;
        let scale = self.scale;
        for &wid in self.z_order.iter().rev() {
            if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                if win.workspace != self.active_workspace || !win.visible { continue; }

                Self::render_window(shadow, info, win, border, rounding, opacity, scale,
                    if win.focused { self.border_active } else { self.border_inactive });
            }
        }

        // Shadebar
        self.bar.render(shadow, info, self.screen_w, self.screen_h);

        for win in &mut self.windows {
            win.dirty = false;
        }
        self.needs_full_redraw = false;
    }

    /// Fast render: only the current input line of the focused window.
    pub fn render_input_line(&self, shadow: *mut u8, info: &FbInfo) -> Option<(u32, u32, u32, u32)> {
        let fid = self.focused?;
        let win = self.windows.iter().find(|w| w.id == fid && w.workspace == self.active_workspace)?;

        let border = self.border;
        let scale = self.scale;
        let pad = 6 * scale;
        let cx = win.content_x(border) + pad;
        let cy = win.content_y(border) + pad;
        let cw = win.content_w(border).saturating_sub(pad * 2);
        let ch = win.content_h(border).saturating_sub(pad * 2);

        terminal::render_input_line(shadow, info,
            cx, cy, cw, ch,
            win.bg_color, self.opacity,
            win.terminal_idx)
    }

    /// Render a single window: aurora overwrite + border blend + content blend + text.
    pub(crate) fn render_window(shadow: *mut u8, info: &FbInfo, win: &Window,
                     border: u32, rounding: u32, opacity: u32, scale: u32,
                     border_color: u32) {
        // 1. FULL OVERWRITE: aurora kills all old pixels (fixes ghost text)
        background::draw_aurora_region(shadow, info,
            win.x, win.y, win.width, win.height);

        // 2. Border blend (on clean aurora)
        render::fill_rounded_rect_blend(shadow, info,
            win.x, win.y, win.width, win.height,
            border_color, rounding, 180);

        // 3. Content blend (on clean border blend)
        let cx = win.content_x(border);
        let cy = win.content_y(border);
        let cw = win.content_w(border);
        let ch = win.content_h(border);
        let inner_r = rounding.saturating_sub(border);
        render::fill_rounded_rect_blend(shadow, info,
            cx, cy, cw, ch,
            win.bg_color, inner_r, opacity);

        // 4. Text on clean background
        let pad = 6 * scale;
        terminal::render_to_window(shadow, info,
            cx + pad, cy + pad,
            cw.saturating_sub(pad * 2), ch.saturating_sub(pad * 2),
            scale, win.terminal_idx);
    }

    /// Render only changed regions. Returns list of (x, y, w, h) to blit.
    pub fn render_damaged(&mut self, shadow: *mut u8, info: &FbInfo) -> Vec<(u32, u32, u32, u32)> {
        if self.needs_full_redraw {
            self.render(shadow, info);
            return alloc::vec![(0, 0, self.screen_w, self.screen_h)];
        }

        let mut regions = Vec::new();
        let border = self.border;
        let rounding = self.rounding;
        let opacity = self.opacity;
        let scale = self.scale;

        for wid_idx in (0..self.z_order.len()).rev() {
            let wid = self.z_order[wid_idx];
            let needs_render = self.windows.iter()
                .find(|w| w.id == wid)
                .map(|w| w.dirty && w.workspace == self.active_workspace && w.visible)
                .unwrap_or(false);

            if needs_render {
                if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                    let border_color = if win.focused { self.border_active } else { self.border_inactive };
                    Self::render_window(shadow, info, win, border, rounding, opacity, scale, border_color);
                    regions.push((win.x, win.y, win.width, win.height));
                }
            }
        }

        if self.bar.dirty {
            self.bar.render(shadow, info, self.screen_w, self.screen_h);
            let bar_y = self.bar.y(self.screen_h);
            regions.push((0, bar_y, self.screen_w, self.bar.height));
        }

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

    /// Update focused flag — only mark changed windows dirty.
    fn set_focused_flag(&mut self, focused_id: WindowId) {
        for win in &mut self.windows {
            let was = win.focused;
            win.focused = win.id == focused_id;
            if win.focused != was {
                win.dirty = true; // Only re-render windows that changed
            }
        }
    }

    /// Focus the nearest window in the given direction from the focused window.
    pub fn focus_direction(&mut self, dx: i32, dy: i32) {
        let fid = match self.focused { Some(id) => id, None => return };
        let focused = match self.windows.iter().find(|w| w.id == fid) {
            Some(w) => (w.x as i32 + w.width as i32 / 2, w.y as i32 + w.height as i32 / 2),
            None => return,
        };

        let mut best: Option<(WindowId, i32)> = None;

        for win in &self.windows {
            if win.id == fid || win.workspace != self.active_workspace || !win.visible { continue; }

            let cx = win.x as i32 + win.width as i32 / 2;
            let cy = win.y as i32 + win.height as i32 / 2;
            let rel_x = cx - focused.0;
            let rel_y = cy - focused.1;

            // Check if this window is in the right direction
            let in_direction = match (dx, dy) {
                (1, 0) => rel_x > 0,   // Right
                (-1, 0) => rel_x < 0,  // Left
                (0, 1) => rel_y > 0,   // Down
                (0, -1) => rel_y < 0,  // Up
                _ => false,
            };
            if !in_direction { continue; }

            // Distance (Manhattan for simplicity)
            let dist = rel_x.abs() + rel_y.abs();
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((win.id, dist));
            }
        }

        if let Some((target_id, _)) = best {
            self.focus_window(target_id);
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
