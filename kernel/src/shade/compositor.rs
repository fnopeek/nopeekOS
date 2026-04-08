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
use super::cursor::MouseState;

/// Swap animation state — windows glide from old to new position.
#[derive(Clone, Copy)]
pub struct SwapAnimation {
    pub win_a: WindowId,
    pub win_b: WindowId,
    pub a_from: (u32, u32, u32, u32), // x, y, w, h
    pub b_from: (u32, u32, u32, u32),
    pub a_to: (u32, u32, u32, u32),
    pub b_to: (u32, u32, u32, u32),
    pub start_tick: u64,
    pub duration: u64, // ticks (100Hz → 25 = 250ms)
}

/// Drag mode: swap windows or resize split.
#[derive(Clone, Copy, PartialEq)]
pub enum DragMode { Swap, Resize }

/// Drag state for Mod+LMB (swap) or Mod+RMB (resize).
#[derive(Clone, Copy)]
pub struct DragState {
    pub window: WindowId,
    pub mode: DragMode,
    /// Last window we swapped with (prevent repeated swaps on same target).
    pub last_target: Option<WindowId>,
    /// Mouse position when drag started (for resize delta).
    pub start_mx: i32,
    pub start_my: i32,
    /// Resize delta when drag started.
    pub start_rw: i32,
    pub start_rh: i32,
}

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
    /// Background has been drawn (skip on partial updates).
    pub aurora_drawn: bool,
    /// Mouse cursor state.
    pub mouse: MouseState,
    /// Drag state: which window is being dragged, and the grab offset.
    pub drag: Option<DragState>,
    /// Active swap animation (windows gliding to new positions).
    pub animation: Option<SwapAnimation>,
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
            .unwrap_or(200);

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
            mouse: {
                let mut m = MouseState::new();
                m.init(screen_w, screen_h);
                m
            },
            drag: None,
            animation: None,
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

        // Split: first window takes one half (+resize delta), rest take the other half
        // Look up first window's resize delta for split adjustment
        let (delta_w, delta_h) = self.windows.iter()
            .find(|w| w.id == ids[0])
            .map(|w| (w.resize_w, w.resize_h))
            .unwrap_or((0, 0));

        if split_horizontal {
            let half = (w.saturating_sub(gap)) / 2;
            let left_w = (half as i32 + delta_w).clamp(100, w.saturating_sub(gap + 100) as i32) as u32;
            let right_w = w.saturating_sub(left_w + gap);
            // First window: left half (adjusted by delta)
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
            let half = (h.saturating_sub(gap)) / 2;
            let top_h = (half as i32 + delta_h).clamp(80, h.saturating_sub(gap + 80) as i32) as u32;
            let bottom_h = h.saturating_sub(top_h + gap);
            // First window: top half (adjusted by delta)
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
        // Only redraw background when needed (expensive at 4K)
        if !self.aurora_drawn || self.needs_full_redraw {
            background::draw_background(shadow, info);
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

                let active_border = if crate::theme::is_active() {
                    crate::gui::background::accent_color()
                } else {
                    self.border_active
                };
                let inactive_border = if crate::theme::is_active() {
                    crate::theme::inactive_border()
                } else {
                    self.border_inactive
                };
                Self::render_window(shadow, info, win, border, rounding, opacity, scale,
                    if win.focused { active_border } else { inactive_border });
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

    /// Render a single window: background overwrite + border blend + content blend + text.
    pub(crate) fn render_window(shadow: *mut u8, info: &FbInfo, win: &Window,
                     border: u32, rounding: u32, opacity: u32, scale: u32,
                     border_color: u32) {
        // 1. FULL OVERWRITE: background kills all old pixels (fixes ghost text)
        background::draw_background_region(shadow, info,
            win.x, win.y, win.width, win.height);

        // 2. Border blend (gradient if theme active + window focused)
        if crate::theme::is_active() && win.focused {
            let (ga, gb) = crate::theme::border_gradient();
            render::fill_rounded_rect_gradient(shadow, info,
                win.x, win.y, win.width, win.height,
                ga, gb, rounding, 200);
        } else {
            render::fill_rounded_rect_blend(shadow, info,
                win.x, win.y, win.width, win.height,
                border_color, rounding, 180);
        }

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

    /// Process a mouse event. Returns true if the scene needs re-rendering.
    pub fn handle_mouse(&mut self, evt: &crate::xhci::MouseEvent) -> bool {
        self.mouse.update(evt.dx, evt.dy, evt.buttons);
        let mx = self.mouse.x;
        let my = self.mouse.y;

        let mod_held = crate::keyboard::is_super_held();

        // Handle active drag (swap or resize)
        if let Some(mut drag) = self.drag {
            let held = match drag.mode {
                DragMode::Swap => self.mouse.left_held(),
                DragMode::Resize => self.mouse.right_held(),
            };
            if held {
                match drag.mode {
                    DragMode::Swap => {
                        if let Some(target) = self.window_at(mx, my) {
                            if target != drag.window && drag.last_target != Some(target) {
                                self.swap_window_order(drag.window, target);
                                drag.last_target = Some(target);
                                self.drag = Some(drag);
                                self.focus_window(drag.window);
                                return true;
                            }
                        }
                        return false;
                    }
                    DragMode::Resize => {
                        let dx = mx - drag.start_mx;
                        let dy = my - drag.start_my;
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == drag.window) {
                            win.resize_w = drag.start_rw + dx;
                            win.resize_h = drag.start_rh + dy;
                        }
                        self.retile();
                        self.needs_full_redraw = true;
                        return true;
                    }
                }
            } else {
                self.drag = None;
                return drag.mode == DragMode::Resize; // resize needs final render
            }
        }

        // Mod+LMB: start swap-drag
        if mod_held && self.mouse.left_clicked() {
            if let Some(wid) = self.window_at(mx, my) {
                self.drag = Some(DragState {
                    window: wid, mode: DragMode::Swap,
                    last_target: None,
                    start_mx: 0, start_my: 0, start_rw: 0, start_rh: 0,
                });
                self.focus_window(wid);
                return true;
            }
        }

        // Mod+RMB: start resize-drag
        if mod_held && self.mouse.right_clicked() {
            if let Some(wid) = self.window_at(mx, my) {
                let (rw, rh) = self.windows.iter()
                    .find(|w| w.id == wid)
                    .map(|w| (w.resize_w, w.resize_h))
                    .unwrap_or((0, 0));
                self.drag = Some(DragState {
                    window: wid, mode: DragMode::Resize,
                    last_target: None,
                    start_mx: mx, start_my: my,
                    start_rw: rw, start_rh: rh,
                });
                self.focus_window(wid);
                return true;
            }
        }

        // Regular LMB click: focus window
        if self.mouse.left_clicked() {
            if let Some(wid) = self.window_at(mx, my) {
                if self.focused != Some(wid) {
                    self.focus_window(wid);
                    return true;
                }
            }
        }

        // Mouse moved — cursor needs redraw (handled by caller)
        evt.dx != 0 || evt.dy != 0
    }

    /// Resize focused window by adjusting its tiling split delta.
    pub fn resize_focused(&mut self, dx: i32, dy: i32) {
        if let Some(fid) = self.focused {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == fid) {
                win.resize_w += dx;
                win.resize_h += dy;
            }
            self.retile();
            self.needs_full_redraw = true;
        }
    }

    /// Swap focused window with the nearest window in the given direction.
    pub fn swap_direction(&mut self, dx: i32, dy: i32) {
        let fid = match self.focused { Some(id) => id, None => return };
        let focused = match self.windows.iter().find(|w| w.id == fid) {
            Some(w) => (w.x as i32 + w.width as i32 / 2, w.y as i32 + w.height as i32 / 2),
            None => return,
        };

        // Find nearest window in direction (same logic as focus_direction)
        let mut best: Option<(WindowId, i32)> = None;
        for win in &self.windows {
            if win.id == fid || win.workspace != self.active_workspace || !win.visible { continue; }
            let cx = win.x as i32 + win.width as i32 / 2;
            let cy = win.y as i32 + win.height as i32 / 2;
            let rel_x = cx - focused.0;
            let rel_y = cy - focused.1;
            let in_direction = match (dx, dy) {
                (1, 0) => rel_x > 0, (-1, 0) => rel_x < 0,
                (0, 1) => rel_y > 0, (0, -1) => rel_y < 0,
                _ => false,
            };
            if !in_direction { continue; }
            let dist = rel_x.abs() + rel_y.abs();
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((win.id, dist));
            }
        }

        if let Some((target_id, _)) = best {
            self.swap_window_order(fid, target_id);
        }
    }

    /// Swap two windows with smooth animation.
    fn swap_window_order(&mut self, a: WindowId, b: WindowId) {
        // Complete any active animation first
        self.finish_animation();

        // Save old positions
        let a_from = self.windows.iter().find(|w| w.id == a)
            .map(|w| (w.x, w.y, w.width, w.height)).unwrap_or((0,0,0,0));
        let b_from = self.windows.iter().find(|w| w.id == b)
            .map(|w| (w.x, w.y, w.width, w.height)).unwrap_or((0,0,0,0));

        // Swap order and retile (calculates new positions)
        let a_idx = self.windows.iter().position(|w| w.id == a);
        let b_idx = self.windows.iter().position(|w| w.id == b);
        if let (Some(ai), Some(bi)) = (a_idx, b_idx) {
            self.windows.swap(ai, bi);
        }
        self.retile();

        // Save new positions
        let a_to = self.windows.iter().find(|w| w.id == a)
            .map(|w| (w.x, w.y, w.width, w.height)).unwrap_or((0,0,0,0));
        let b_to = self.windows.iter().find(|w| w.id == b)
            .map(|w| (w.x, w.y, w.width, w.height)).unwrap_or((0,0,0,0));

        // Start animation: put windows back at old positions, animate to new
        if a_from != a_to || b_from != b_to {
            // Set windows to starting position
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == a) {
                w.x = a_from.0; w.y = a_from.1; w.width = a_from.2; w.height = a_from.3;
            }
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == b) {
                w.x = b_from.0; w.y = b_from.1; w.width = b_from.2; w.height = b_from.3;
            }
            self.animation = Some(SwapAnimation {
                win_a: a, win_b: b,
                a_from, b_from, a_to, b_to,
                start_tick: crate::interrupts::ticks(),
                duration: 25, // 250ms at 100Hz
            });
        }
        self.needs_full_redraw = true;
    }

    /// Advance swap animation. Returns true if a frame was updated.
    pub fn tick_animation(&mut self) -> bool {
        let anim = match self.animation { Some(a) => a, None => return false };
        let now = crate::interrupts::ticks();
        let elapsed = now.saturating_sub(anim.start_tick);

        if elapsed >= anim.duration {
            self.finish_animation();
            return true;
        }

        // Ease-out cubic: t' = 1 - (1-t)³  (fast start, smooth deceleration)
        let t = (elapsed * 1000 / anim.duration) as i64; // 0..1000
        let inv = 1000 - t;
        let t_ease = 1000 - (inv * inv * inv / 1_000_000);

        let lerp = |from: u32, to: u32| -> u32 {
            let f = from as i64;
            let delta = to as i64 - f;
            (f + delta * t_ease / 1000) as u32
        };

        if let Some(w) = self.windows.iter_mut().find(|w| w.id == anim.win_a) {
            w.x = lerp(anim.a_from.0, anim.a_to.0);
            w.y = lerp(anim.a_from.1, anim.a_to.1);
            w.width = lerp(anim.a_from.2, anim.a_to.2);
            w.height = lerp(anim.a_from.3, anim.a_to.3);
            w.dirty = true;
        }
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == anim.win_b) {
            w.x = lerp(anim.b_from.0, anim.b_to.0);
            w.y = lerp(anim.b_from.1, anim.b_to.1);
            w.width = lerp(anim.b_from.2, anim.b_to.2);
            w.height = lerp(anim.b_from.3, anim.b_to.3);
            w.dirty = true;
        }
        self.needs_full_redraw = true;
        true
    }

    /// Instantly complete any active animation.
    fn finish_animation(&mut self) {
        if let Some(anim) = self.animation.take() {
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == anim.win_a) {
                w.x = anim.a_to.0; w.y = anim.a_to.1;
                w.width = anim.a_to.2; w.height = anim.a_to.3;
                w.dirty = true;
            }
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == anim.win_b) {
                w.x = anim.b_to.0; w.y = anim.b_to.1;
                w.width = anim.b_to.2; w.height = anim.b_to.3;
                w.dirty = true;
            }
            self.needs_full_redraw = true;
        }
    }

    /// Find the topmost window at screen coordinates (x, y).
    fn window_at(&self, x: i32, y: i32) -> Option<WindowId> {
        // Z-order: front to back (first match = topmost)
        for &wid in &self.z_order {
            if let Some(win) = self.windows.iter().find(|w| w.id == wid
                && w.workspace == self.active_workspace && w.visible)
            {
                let wx = win.x as i32;
                let wy = win.y as i32;
                let ww = win.width as i32;
                let wh = win.height as i32;
                if x >= wx && x < wx + ww && y >= wy && y < wy + wh {
                    return Some(wid);
                }
            }
        }
        None
    }
}

/// Parse a hex color string ("RRGGBB") to u32.
fn parse_hex_color(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches('#');
    if s.len() != 6 { return None; }
    u32::from_str_radix(s, 16).ok()
}
