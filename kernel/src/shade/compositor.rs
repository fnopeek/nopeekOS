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

/// Bottom hot-edge band (px) that arms the dock reveal.
const DOCK_HOT_EDGE_PX: i32 = 2;
/// Ticks (100 Hz) the cursor must hold the hot-edge before revealing.
const DOCK_REVEAL_DWELL_TICKS: u32 = 8;
/// Ticks the cursor must be away from the dock before it hides again.
const DOCK_HIDE_DEBOUNCE_TICKS: u32 = 25;
/// Slack above the dock's top counted as "still over the dock".
const DOCK_HIDE_MARGIN_PX: i32 = 6;
/// Height (1× px, scaled at draw time) of the collapsed-dock presence
/// bar — a thin, dock-width tray-coloured strip that hints "a dock lives
/// here" without reserving space or showing the icons.
const DOCK_HANDLE_H: u32 = 5;
/// Floating gap below the revealed dock (1× px, scaled), so it hovers
/// detached from the bottom edge the way the bar's pills do.
const DOCK_BOTTOM_GAP: u32 = 10;
/// Dock translucency (0..255). A frosted, slightly see-through tray.
const DOCK_OPACITY: u32 = 210;

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

/// Auto-hide bottom dock state. The compositor owns the reveal/hide
/// slide; the dock app (`dock.wasm`) only declares itself via
/// `npk_window_set_dock` and renders its icon row.
#[derive(Clone, Copy)]
pub struct DockState {
    pub id: WindowId,
    /// Dock (tray) height in px.
    pub thickness: u32,
    /// Floating gap below the tray when shown (px) — like the bar margin,
    /// so the revealed dock hovers detached from the bottom edge.
    pub gap: u32,
    /// Reveal target the hot-edge logic drives toward.
    pub target_shown: bool,
    /// Current slide offset: 0 = fully shown (floating), `thickness + gap`
    /// = fully hidden (below the screen edge).
    pub offset: u32,
    /// Ticks the cursor has held the bottom hot-edge (reveal dwell).
    pub dwell: u32,
    /// Ticks the cursor has been away from the dock while shown (hide debounce).
    pub debounce: u32,
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
    /// Auto-hide bottom dock, if a dock app has registered one.
    pub dock: Option<DockState>,
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
            dock: None,
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
    /// Returns None if no terminal slots available.
    pub fn create_window(&mut self, title: &str, x: u32, y: u32, w: u32, h: u32) -> Option<WindowId> {
        let terminal_idx = terminal::allocate()?;

        let id = WindowId(self.next_id);
        self.next_id += 1;

        let mut win = Window::new(id, title, x, y, w, h);
        win.workspace = self.active_workspace;
        win.terminal_idx = terminal_idx;
        win.kind = crate::shade::window::WindowKind::Terminal;
        crate::intent::create_session(terminal_idx);

        // Register window as a process in the process table
        let pid = crate::process::spawn("loop", crate::process::KIND_SYSTEM, terminal_idx, 0);
        win.pid = pid;

        self.windows.push(win);
        self.z_order.insert(0, id);
        self.focus_window(id);
        self.retile();
        self.needs_full_redraw = true;

        Some(id)
    }

    /// Create a widget-kind window for a Phase 10 GUI app. Doesn't
    /// allocate a terminal buffer (widget apps aren't text-driven).
    /// Focus stays on the current window so the spawning shell keeps
    /// receiving the user's input.
    pub fn create_widget_window(&mut self, title: &str) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;

        let mut win = Window::new(id, title, 0, 0, 100, 100);
        win.workspace = self.active_workspace;
        win.terminal_idx = 255; // sentinel — no terminal buffer owned
        win.kind = crate::shade::window::WindowKind::Widget;
        win.pid = 0;

        self.windows.push(win);
        self.z_order.insert(0, id);
        // Deliberately NOT focus_window(id) — keep focus on the shell
        // that spawned us, so the user's next keystroke lands there.
        // But this insert(0) put us above the dock, so re-pin it on top.
        self.retile();
        self.pin_dock_to_front();
        self.needs_full_redraw = true;

        id
    }

    /// Create a Surface-kind window for an external pixel source (a
    /// microvm's virtio-gpu framebuffer). No terminal buffer, no
    /// widget tree — content is a `GuestSurface` keyed by this id.
    /// Like `create_widget_window`, focus stays on the spawning shell.
    pub fn create_surface_window(&mut self, title: &str) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;

        let mut win = Window::new(id, title, 0, 0, 100, 100);
        win.workspace = self.active_workspace;
        win.terminal_idx = 255; // sentinel — no terminal buffer owned
        win.kind = crate::shade::window::WindowKind::Surface;
        win.pid = 0;

        self.windows.push(win);
        self.z_order.insert(0, id);
        self.retile();
        self.pin_dock_to_front();
        self.needs_full_redraw = true;

        id
    }

    /// Convert the Terminal-kind window backing `terminal_idx` into a
    /// Widget-kind window in place. Keeps id, geometry, z-order, and
    /// focus, but releases the terminal buffer (255 sentinel) so key
    /// events flow through the widget event queue instead of the
    /// terminal session.
    ///
    /// Used by widget apps whose spawn path (`npk_spawn_module`) handed
    /// them a terminal window they never meant to use — avoids the
    /// "two windows for one app" seam.
    ///
    /// Returns the WindowId on success, None if no terminal window
    /// owns that terminal_idx. Does not touch the session; the worker's
    /// exit path still cleans it up.
    pub fn promote_terminal_to_widget(&mut self, terminal_idx: u8) -> Option<WindowId> {
        let win = self.windows.iter_mut().find(|w| {
            w.terminal_idx == terminal_idx
                && w.kind == crate::shade::window::WindowKind::Terminal
        })?;
        let id = win.id;
        win.kind = crate::shade::window::WindowKind::Widget;
        win.terminal_idx = 255;
        win.dirty = true;
        self.needs_full_redraw = true;
        Some(id)
    }

    /// Reconfigure an existing window as a centred overlay: floating
    /// state, caller-chosen size, clamped to screen bounds. Used by the
    /// `npk_window_set_overlay` host fn — the compositor stays ignorant
    /// of which app (drun, future launchers, …) requested the change.
    ///
    /// Does not touch focus. Caller decides whether to focus the window
    /// afterwards.
    pub fn set_overlay(&mut self, id: WindowId, w: u32, h: u32) -> bool {
        let screen_w = self.screen_w;
        let screen_h = self.screen_h;
        let ow = w.min(screen_w.saturating_sub(80)).max(120);
        let oh = h.min(screen_h.saturating_sub(80)).max(80);
        let ox = screen_w.saturating_sub(ow) / 2;
        let oy = screen_h.saturating_sub(oh) / 2;

        let changed = if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.state = crate::shade::window::WindowState::Floating;
            win.is_overlay = true;
            win.x = ox;
            win.y = oy;
            win.width = ow;
            win.height = oh;
            win.dirty = true;
            true
        } else {
            false
        };

        if changed {
            // Retile so tiled windows reclaim any space the window used
            // to occupy (if it was previously tiled).
            self.retile();
            self.needs_full_redraw = true;
        }
        changed
    }

    /// Toggle the `modal` flag on a window.
    pub fn set_modal(&mut self, id: WindowId, modal: bool) -> bool {
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.modal = modal;
            true
        } else {
            false
        }
    }

    /// Bottom edge of the bar-free region — the dock's resting baseline.
    /// For a top bar this is `screen_h`; for a bottom bar it sits above
    /// the bar's reserved band.
    fn dock_baseline(&self) -> u32 {
        self.bar.workspace_y() + self.bar.workspace_height(self.screen_h)
    }

    /// `npk_window_set_dock` host fn — turn `id` into a bottom auto-hide
    /// dock. Overlay (no strut, excluded from retile), never modal, never
    /// focused on reveal, global across workspaces. Starts fully hidden.
    pub fn set_dock(&mut self, id: WindowId, w: u32, h: u32) -> bool {
        let screen_w = self.screen_w;
        let screen_h = self.screen_h;
        let baseline = self.dock_baseline();
        let active_ws = self.active_workspace;

        let dw = w.min(screen_w.saturating_sub(40)).max(120);
        let dh = h.min(screen_h / 2).max(40);

        let found = if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.state = crate::shade::window::WindowState::Floating;
            win.is_overlay = true;
            win.is_dock = true;
            win.modal = false;
            win.workspace = active_ws;
            win.width = dw;
            win.height = dh;
            win.x = screen_w.saturating_sub(dw) / 2;
            win.y = baseline;       // fully hidden (below the usable area)
            win.visible = false;
            win.dirty = false;
            true
        } else {
            false
        };

        if found {
            let gap = DOCK_BOTTOM_GAP * self.scale.max(1);
            self.dock = Some(DockState {
                id,
                thickness: dh,
                gap,
                target_shown: false,
                offset: dh + gap,   // fully hidden below the edge
                dwell: 0,
                debounce: 0,
            });
            // Overlay → tiling grid is untouched, but retile reclaims any
            // slot this window held if it was previously tiled.
            self.retile();
            self.pin_dock_to_front();
            self.needs_full_redraw = true;
        }
        found
    }

    /// Drive the dock reveal/hide intent from the current cursor Y.
    /// Called every frame (poll_render) so dwell/debounce advance even
    /// while the cursor is parked. Suppressed during a drag/resize so the
    /// dock never fights a tile being dragged toward the bottom.
    pub fn dock_update_reveal(&mut self, cursor_y: i32) {
        let dragging = self.drag.is_some();
        let baseline = self.dock_baseline() as i32;
        // "Desktop free" = no real (non-overlay) window on this workspace.
        // The dock then stays revealed as a launcher home surface; launching
        // anything (terminal, widget, browser) hides it again — the bottom
        // hot-edge still peeks it back on demand.
        let desktop_empty = !self.windows.iter().any(|w|
            !w.is_overlay && w.workspace == self.active_workspace);
        let Some(dock) = self.dock.as_mut() else { return };

        if dragging {
            dock.dwell = 0;
            return;
        }

        if desktop_empty {
            dock.target_shown = true;
            dock.debounce = 0;
            dock.dwell = 0;
            return;
        }

        if dock.target_shown {
            // Hide once the cursor leaves the dock band for long enough.
            // The shown tray floats `gap` px above the baseline.
            let dock_top = baseline - (dock.thickness + dock.gap) as i32;
            let over_dock = cursor_y >= dock_top - DOCK_HIDE_MARGIN_PX;
            if over_dock {
                dock.debounce = 0;
            } else {
                dock.debounce = dock.debounce.saturating_add(1);
                if dock.debounce >= DOCK_HIDE_DEBOUNCE_TICKS {
                    dock.target_shown = false;
                    dock.dwell = 0;
                }
            }
        } else {
            // Reveal once the cursor holds the bottom hot-edge.
            if cursor_y >= baseline - DOCK_HOT_EDGE_PX {
                dock.dwell = dock.dwell.saturating_add(1);
                if dock.dwell >= DOCK_REVEAL_DWELL_TICKS {
                    dock.target_shown = true;
                    dock.debounce = 0;
                }
            } else {
                dock.dwell = 0;
            }
        }
    }

    /// Advance the dock slide one frame. Returns true while moving (caller
    /// re-renders). Eases the offset toward 0 (shown, floating) /
    /// `thickness + gap` (hidden, below the edge).
    pub fn dock_tick(&mut self) -> bool {
        let baseline = self.dock_baseline() as i32;
        let screen_w = self.screen_w;

        let (id, offset, slide, now_visible) = {
            let Some(dock) = self.dock.as_mut() else { return false };
            let slide = (dock.thickness + dock.gap) as i32;
            let target = if dock.target_shown { 0 } else { slide };
            let cur = dock.offset as i32;
            if cur == target { return false; }
            let delta = target - cur;
            // Ease-out: step a quarter of the remaining distance, min 2 px.
            let step = (delta.abs() / 4).max(2).min(delta.abs());
            let next = if delta > 0 { cur + step } else { cur - step };
            dock.offset = next.max(0) as u32;
            (dock.id, dock.offset, slide, dock.offset < slide as u32)
        };

        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.x = screen_w.saturating_sub(win.width) / 2;
            // Shown (offset 0): top = baseline - slide = baseline - thickness
            // - gap → tray floats `gap` above the edge. Hidden (offset slide):
            // top = baseline → fully below the usable area.
            win.y = (baseline - slide + offset as i32).max(0) as u32;
            win.visible = now_visible;
            win.dirty = true;
        }
        self.needs_full_redraw = true;
        true
    }

    /// True iff any visible window on the active workspace is modal.
    /// Used by shade-action dispatch to lock out focus-shift shortcuts
    /// while modal UI is on-screen.
    pub fn has_modal_window(&self) -> bool {
        self.windows.iter().any(|w|
            w.modal
            && w.workspace == self.active_workspace
            && w.visible)
    }

    /// Close a window by ID.
    pub fn close_window(&mut self, id: WindowId) {
        // If the dock app's window goes away, forget the dock so the
        // reveal/tick machinery no-ops.
        if self.dock.map(|d| d.id) == Some(id) {
            self.dock = None;
        }
        // Free session + terminal buffer + process before removing window
        if let Some(win) = self.windows.iter().find(|w| w.id == id) {
            match win.kind {
                crate::shade::window::WindowKind::Terminal => {
                    crate::intent::destroy_session(win.terminal_idx);
                    terminal::free(win.terminal_idx);
                    if win.pid != 0 { crate::process::exit(win.pid); }
                }
                crate::shade::window::WindowKind::Widget => {
                    // No terminal buffer / session to free. Drop the
                    // per-window widget scene + event queue; their
                    // backing allocations free with the entries.
                    crate::shade::widgets::remove_scene(id.0);
                    crate::shade::widgets::remove_event_queue(id.0);
                }
                crate::shade::window::WindowKind::Surface => {
                    // Drop the bitmap surface; ask the bound microvm
                    // to power off (best-effort — it may already have
                    // exited, which is what closed this window).
                    crate::shade::surface::remove_surface(id.0);
                    crate::microvm::vm_close_for_window(id.0);
                }
            }
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
            // Widget windows don't own a terminal buffer — leave ACTIVE_IDX
            // pointing at the previously-active terminal so kprintln output
            // keeps a valid sink while the widget app is focused.
            if win.kind == crate::shade::window::WindowKind::Terminal {
                terminal::set_active_terminal(win.terminal_idx);
                terminal::restore_cursor();
            }
        }
        // The dock is never focused, so the insert(0) above would bury it
        // behind the just-focused window. Re-pin it to the very top.
        self.pin_dock_to_front();
        // Don't set needs_full_redraw — render_damaged handles 2 windows only
    }

    /// Keep the dock window at the front of the z-order (topmost). Render
    /// passes iterate `z_order.rev()`, drawing index 0 last → on top.
    fn pin_dock_to_front(&mut self) {
        if let Some(dock) = self.dock {
            self.z_order.retain(|&wid| wid != dock.id);
            self.z_order.insert(0, dock.id);
        }
    }

    /// Switch to workspace.
    pub fn switch_workspace(&mut self, ws: u8) {
        if ws == self.active_workspace { return; }
        self.active_workspace = ws;
        self.bar.set_workspace(ws);

        // The dock is global: follow the active workspace and snap shut so
        // it re-reveals on demand rather than popping up mid-slide.
        if let Some(dock) = self.dock {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == dock.id) {
                win.workspace = ws;
                win.visible = false;
                win.y = self.bar.workspace_y() + self.bar.workspace_height(self.screen_h);
            }
            if let Some(d) = self.dock.as_mut() {
                d.target_shown = false;
                d.offset = d.thickness;
                d.dwell = 0;
                d.debounce = 0;
            }
        }

        self.focused = self.z_order.iter()
            .find(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == ws && !w.is_dock))
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
        self.pin_dock_to_front();
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
                     && w.visible
                     && !w.is_overlay)
            .map(|w| w.id)
            .collect();

        if tiled.is_empty() { return; }

        let gap = self.gaps;
        self.dwindle_layout(&tiled, area_x, area_y, area_w, area_h, gap, true);
        self.sync_surface_tile_sizes();
    }

    /// Push every Surface window's content rect into the surface
    /// registry so virtio-gpu can advertise it via GET_DISPLAY_INFO
    /// (D4 — guest renders to the tile size, no host scaling). Called
    /// at the end of every retile; `set_tile_size` is idempotent and
    /// only flags a config-change on a real size change. The `border`
    /// must match render_window's content-rect inset exactly or the
    /// guest would render a few px off.
    fn sync_surface_tile_sizes(&self) {
        let border = self.border;
        for win in &self.windows {
            if win.kind == crate::shade::window::WindowKind::Surface {
                crate::shade::surface::set_tile_size(
                    win.id.0,
                    win.content_w(border),
                    win.content_h(border),
                );
            }
        }
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
            let max_w = w.saturating_sub(gap).saturating_sub(100).max(100) as i32;
            let left_w = (half as i32 + delta_w).clamp(100, max_w) as u32;
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
            let max_h = h.saturating_sub(gap).saturating_sub(80).max(80) as i32;
            let top_h = (half as i32 + delta_h).clamp(80, max_h) as u32;
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

        // Presence handle for a fully-hidden dock.
        self.render_dock_handle(shadow, info);

        for win in &mut self.windows {
            win.dirty = false;
        }
        self.needs_full_redraw = false;
    }

    /// Draw the collapsed-dock presence bar at the resting edge while the
    /// dock is fully hidden, so the user knows a dock lives there. It
    /// mirrors the open dock — same width + same translucent SurfaceElevated
    /// tray colour — but only ~5 px tall: just the grey strip is enough to
    /// signal "a dock lives here". Drawn over the wallpaper; cleared by the
    /// full redraw `dock_tick` forces the moment it slides into view.
    fn render_dock_handle(&self, shadow: *mut u8, info: &FbInfo) {
        let Some(dock) = self.dock.as_ref() else { return };
        // Only while fully hidden — once it slides up the window itself
        // is the affordance.
        if dock.offset < dock.thickness + dock.gap { return; }
        // Mirror the dock window's geometry (centred, same width).
        let Some(win) = self.windows.iter().find(|w| w.id == dock.id) else { return };

        let scale = self.scale.max(1);
        let hh = DOCK_HANDLE_H * scale;
        let w = win.width;
        let x = win.x;
        let baseline = self.dock_baseline();
        let y = baseline.saturating_sub(hh);

        // Tray-coloured bar: same token + translucency as the revealed dock.
        let tray = crate::shade::widgets::palette::resolve(
            crate::shade::widgets::abi::Token::SurfaceElevated);
        render::fill_rounded_rect_alpha(shadow, info, x, y, w, hh,
            tray & 0x00FF_FFFF, hh / 2, DOCK_OPACITY);
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
            win.terminal_idx)
    }

    /// Render a single window: background overwrite + border blend + content blend + text.
    pub(crate) fn render_window(shadow: *mut u8, info: &FbInfo, win: &Window,
                     border: u32, rounding: u32, opacity: u32, scale: u32,
                     border_color: u32) {
        // Overlay windows skip the wallpaper restore — rounded-out
        // corners keep showing whatever app is underneath instead of
        // punching a wallpaper-shaped hole into it.
        if !win.is_overlay {
            background::draw_background_region(shadow, info,
                win.x, win.y, win.width, win.height);
        }

        // 2+3. Single-pass chrome. Terminal windows get the full
        // layered paint (border + bg_color content). Widget windows
        // get border-only — the widget supplies its own content + AA
        // at the inner edge, so the chrome must not bleed `bg_color`
        // into the inner-fringe band.
        // The dock is chrome-less: no hard bordered box around it. It
        // supplies its own soft tray background in the widget tree, so
        // we skip the chrome pass entirely and blit the widget content
        // edge-to-edge with the full rounding. Everything else gets the
        // normal border + bg chrome.
        let (chrome_border, chrome_round) = if win.is_dock {
            (0u32, rounding)
        } else {
            (border, rounding.saturating_sub(border))
        };

        if !win.is_dock {
            let (ba, bb, b_op) = if crate::theme::is_active() && win.focused {
                let (ga, gb) = crate::theme::border_gradient();
                (ga, gb, 200u32)
            } else {
                (border_color, border_color, 180u32)
            };
            let paint_content = matches!(win.kind, crate::shade::window::WindowKind::Terminal);
            render::fill_rounded_chrome_aa(shadow, info,
                win.x, win.y, win.width, win.height,
                ba, bb, win.bg_color,
                rounding, border, b_op, opacity, paint_content);
        }

        let cx = win.content_x(chrome_border);
        let cy = win.content_y(chrome_border);
        let cw = win.content_w(chrome_border);
        let ch = win.content_h(chrome_border);
        // Inner shape is concentric with the outer at radius
        // `rounding - border` — see `fill_rounded_chrome_aa`. The
        // widget-blit AA at the inner edge is computed against this
        // same inner curve so widget pixels and chrome border meet
        // pixel-perfectly along the rounded inner curve.
        let inner_r = chrome_round;

        // 4. Content-kind specific draw.
        match win.kind {
            crate::shade::window::WindowKind::Terminal => {
                let pad = 6 * scale;
                let text_x = cx + pad;
                let text_y = cy + pad;
                let text_w = cw.saturating_sub(pad * 2);
                let text_h = ch.saturating_sub(pad * 2);
                if win.focused {
                    terminal::cache_input_line_bg(shadow, info,
                        text_x, text_y, text_w, text_h, win.terminal_idx);
                }
                terminal::render_to_window(shadow, info,
                    text_x, text_y, text_w, text_h,
                    scale, win.terminal_idx);
            }
            crate::shade::window::WindowKind::Widget => {
                let _ = crate::shade::widgets::relayout_scene(
                    win.id.0, cx as i32, cy as i32, cw, ch,
                );
                // Widget pixels fill the inner rounded rect. Middle rows
                // memcpy; rows that touch a corner curve fall through to
                // a per-pixel SDF blend so widget content and chrome
                // border meet with proper AA at the inner edge.
                crate::shade::widgets::with_scene(win.id.0, |scene| {
                    let pitch = info.pitch as usize;
                    let fb_w = info.width;
                    let fb_h = info.height;
                    let x1 = (cx + scene.width).min(fb_w);
                    let y1 = (cy + scene.height).min(fb_h);
                    let cw_local = x1.saturating_sub(cx);
                    let ch_local = y1.saturating_sub(cy);
                    let r = inner_r.min(cw_local / 2).min(ch_local / 2);

                    // The dock is a frosted, floating tray: blend every
                    // scene pixel over whatever is behind it at
                    // DOCK_OPACITY, fading along the rounded corners via the
                    // SDF coverage. (Opaque memcpy can't be translucent.)
                    // Small surface → a full per-pixel blend is cheap.
                    if win.is_dock {
                        for dy in cy..y1 {
                            let local_y = dy - cy;
                            for dx in cx..x1 {
                                let cov = render::rect_coverage_sdf(
                                    dx, dy, cx, cy, cw_local, ch_local, r);
                                if cov == 0 { continue; }
                                let src_idx = (local_y as usize) * (scene.width as usize)
                                    + (dx - cx) as usize;
                                let px = scene.pixels[src_idx];
                                let a = (DOCK_OPACITY * cov / 255).min(255);
                                render::blend_pixel(shadow, info, dx, dy, px, a);
                            }
                        }
                        return;
                    }

                    for dy in cy..y1 {
                        let local_y = dy - cy;
                        let in_top    = r > 0 && local_y < r;
                        let in_bottom = r > 0 && local_y >= ch_local - r;

                        if !in_top && !in_bottom {
                            // Straight middle: fast memcpy of the full row.
                            let src_base = (local_y as usize) * (scene.width as usize);
                            let dst_off  = dy as usize * pitch + cx as usize * 4;
                            unsafe {
                                let dst = shadow.add(dst_off) as *mut u32;
                                core::ptr::copy_nonoverlapping(
                                    scene.pixels.as_ptr().add(src_base),
                                    dst,
                                    cw_local as usize,
                                );
                            }
                            continue;
                        }

                        // Corner row: r pixels on each side go through
                        // the SDF blend; the middle is still memcpy.
                        let mid_lo = r.min(cw_local);
                        let mid_hi = cw_local.saturating_sub(r).max(mid_lo);

                        for dx in cx..(cx + mid_lo).min(x1) {
                            let local_x = dx - cx;
                            let cov = render::rect_coverage_sdf(dx, dy, cx, cy, cw_local, ch_local, r);
                            if cov == 0 { continue; }
                            let src_idx = (local_y as usize) * (scene.width as usize) + local_x as usize;
                            let widget_pixel = scene.pixels[src_idx];
                            render::blend_pixel(shadow, info, dx, dy, widget_pixel, cov);
                        }

                        if mid_hi > mid_lo {
                            let src_base = (local_y as usize) * (scene.width as usize) + mid_lo as usize;
                            let dst_off  = dy as usize * pitch + (cx + mid_lo) as usize * 4;
                            let span     = (mid_hi - mid_lo) as usize;
                            unsafe {
                                let dst = shadow.add(dst_off) as *mut u32;
                                core::ptr::copy_nonoverlapping(
                                    scene.pixels.as_ptr().add(src_base),
                                    dst,
                                    span,
                                );
                            }
                        }

                        for dx in (cx + mid_hi).min(x1)..x1 {
                            let local_x = dx - cx;
                            let cov = render::rect_coverage_sdf(dx, dy, cx, cy, cw_local, ch_local, r);
                            if cov == 0 { continue; }
                            let src_idx = (local_y as usize) * (scene.width as usize) + local_x as usize;
                            let widget_pixel = scene.pixels[src_idx];
                            render::blend_pixel(shadow, info, dx, dy, widget_pixel, cov);
                        }
                    }
                });
            }
            crate::shade::window::WindowKind::Surface => {
                // Raw guest framebuffer → tile, 1:1 (no scaling). D4:
                // virtio-gpu GET_DISPLAY_INFO advertises this content
                // rect, so the guest (wlroots/cage) reflows to the
                // tile size natively — guest `sw×sh` == `cw×ch` in
                // steady state; a brief mismatch during a resize
                // round-trip just clips (never stretches). Same
                // memcpy-middle + SDF-corner-blend shape as the Widget
                // arm above so the browser tile gets the identical
                // concentric rounded corners as every other window
                // and sits flush in the dwindle layout.
                crate::shade::surface::with_front(win.id.0, |px, sw, sh| {
                    if px.is_empty() || sw == 0 || sh == 0 || cw == 0 || ch == 0 {
                        return;
                    }
                    let pitch = info.pitch as usize;
                    let fb_w = info.width;
                    let fb_h = info.height;
                    // Clip the 1:1 blit to the guest buffer, the
                    // content rect, and the framebuffer — never
                    // overdraw the border or a neighbour tile.
                    let x1 = (cx + sw).min(cx + cw).min(fb_w);
                    let y1 = (cy + sh).min(cy + ch).min(fb_h);
                    let cw_local = x1.saturating_sub(cx);
                    let ch_local = y1.saturating_sub(cy);
                    let r = inner_r.min(cw_local / 2).min(ch_local / 2);

                    for dy in cy..y1 {
                        let local_y = dy - cy;
                        let in_top    = r > 0 && local_y < r;
                        let in_bottom = r > 0 && local_y >= ch_local - r;

                        if !in_top && !in_bottom {
                            // Straight middle: fast memcpy of the row.
                            let src_base = (local_y as usize) * (sw as usize);
                            let dst_off  = dy as usize * pitch + cx as usize * 4;
                            unsafe {
                                let dst = shadow.add(dst_off) as *mut u32;
                                core::ptr::copy_nonoverlapping(
                                    px.as_ptr().add(src_base),
                                    dst,
                                    cw_local as usize,
                                );
                            }
                            continue;
                        }

                        // Corner row: r pixels on each side go through
                        // the SDF blend; the middle is still memcpy.
                        let mid_lo = r.min(cw_local);
                        let mid_hi = cw_local.saturating_sub(r).max(mid_lo);

                        for dx in cx..(cx + mid_lo).min(x1) {
                            let local_x = dx - cx;
                            let cov = render::rect_coverage_sdf(dx, dy, cx, cy, cw_local, ch_local, r);
                            if cov == 0 { continue; }
                            let src_idx = (local_y as usize) * (sw as usize) + local_x as usize;
                            render::blend_pixel(shadow, info, dx, dy, px[src_idx], cov);
                        }

                        if mid_hi > mid_lo {
                            let src_base = (local_y as usize) * (sw as usize) + mid_lo as usize;
                            let dst_off  = dy as usize * pitch + (cx + mid_lo) as usize * 4;
                            let span     = (mid_hi - mid_lo) as usize;
                            unsafe {
                                let dst = shadow.add(dst_off) as *mut u32;
                                core::ptr::copy_nonoverlapping(
                                    px.as_ptr().add(src_base),
                                    dst,
                                    span,
                                );
                            }
                        }

                        for dx in (cx + mid_hi).min(x1)..x1 {
                            let local_x = dx - cx;
                            let cov = render::rect_coverage_sdf(dx, dy, cx, cy, cw_local, ch_local, r);
                            if cov == 0 { continue; }
                            let src_idx = (local_y as usize) * (sw as usize) + local_x as usize;
                            render::blend_pixel(shadow, info, dx, dy, px[src_idx], cov);
                        }
                    }
                });
            }
        }
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
                    let border_color = if win.focused { active_border } else { inactive_border };
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
            if win.id == fid || win.workspace != self.active_workspace || !win.visible || win.is_dock { continue; }

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
            .filter(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == self.active_workspace && w.visible && !w.is_dock))
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
            .filter(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == self.active_workspace && w.visible && !w.is_dock))
            .copied()
            .collect();

        if ws_windows.len() < 2 { return; }

        let current_idx = self.focused
            .and_then(|fid| ws_windows.iter().position(|&wid| wid == fid))
            .unwrap_or(0);
        let prev_idx = if current_idx == 0 { ws_windows.len() - 1 } else { current_idx - 1 };
        self.focus_window(ws_windows[prev_idx]);
    }

    /// Process a mouse event (legacy path). Returns true if the scene needs re-rendering.
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
                        let mut swapped = false;
                        if let Some(target) = self.window_at(mx, my) {
                            if target != drag.window && drag.last_target != Some(target) {
                                self.swap_window_order(drag.window, target);
                                drag.last_target = Some(target);
                                self.focus_window(drag.window);
                                swapped = true;
                            }
                        }
                        self.drag = Some(drag); // Keep drag alive
                        return swapped;
                    }
                    DragMode::Resize => {
                        let dx = mx - drag.start_mx;
                        let dy = my - drag.start_my;
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == drag.window) {
                            win.resize_w = drag.start_rw + dx;
                            win.resize_h = drag.start_rh + dy;
                        }
                        self.drag = Some(drag); // Keep drag alive
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

        // Cursor overlay handled by redraw_overlay — no scene redraw for movement
        false
    }

    /// Handle only button events (click, drag, release). Position already in self.mouse.
    /// Called from lock-free input path — only when buttons change.
    pub fn handle_mouse_buttons(&mut self) -> bool {
        let mx = self.mouse.x;
        let my = self.mouse.y;
        let mod_held = crate::keyboard::is_super_held();

        // Active drag
        if let Some(mut drag) = self.drag {
            let held = match drag.mode {
                DragMode::Swap => self.mouse.left_held(),
                DragMode::Resize => self.mouse.right_held(),
            };
            if held {
                match drag.mode {
                    DragMode::Swap => {
                        let mut swapped = false;
                        if let Some(target) = self.window_at(mx, my) {
                            if target != drag.window && drag.last_target != Some(target) {
                                self.swap_window_order(drag.window, target);
                                drag.last_target = Some(target);
                                self.focus_window(drag.window);
                                swapped = true;
                            }
                        }
                        self.drag = Some(drag); // Keep drag alive
                        return swapped;
                    }
                    DragMode::Resize => {
                        let dx = mx - drag.start_mx;
                        let dy = my - drag.start_my;
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == drag.window) {
                            win.resize_w = drag.start_rw + dx;
                            win.resize_h = drag.start_rh + dy;
                        }
                        self.drag = Some(drag); // Keep drag alive
                        self.retile();
                        self.needs_full_redraw = true;
                        return true;
                    }
                }
            } else {
                self.drag = None;
                return drag.mode == DragMode::Resize;
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

        // Regular LMB click: focus window + dispatch widget event
        if self.mouse.left_clicked() {
            if let Some(wid) = self.window_at(mx, my) {
                // The dock must not steal keyboard focus from the tile you
                // were working in — clicking an icon launches/focuses an
                // app, the dock itself stays unfocused. Hit-test/events
                // below still fire.
                let is_dock_win = self.windows.iter()
                    .any(|w| w.id == wid && w.is_dock);
                // Focus on first click into an unfocused window; later
                // clicks on the same focused widget should dispatch.
                let focus_changed = !is_dock_win && self.focused != Some(wid);
                if focus_changed {
                    self.focus_window(wid);
                }

                // Widget-kind click: hit-test against the scene's
                // layout tree, push Event::Action(id) or
                // Event::MouseButton into the window's queue.
                let is_widget = self.windows.iter()
                    .find(|w| w.id == wid)
                    .map(|w| w.kind == crate::shade::window::WindowKind::Widget)
                    .unwrap_or(false);
                if is_widget {
                    use crate::shade::widgets::abi::{Event, MouseButton};
                    // Move focus + start active state on the deepest
                    // focusable widget under the cursor. press_at must
                    // not touch the compositor (we're inside its lock)
                    // — it returns `true` when it re-rasterized so we
                    // can mark the window dirty here.
                    let pressed_dirty = crate::shade::widgets::press_at(wid.0, mx, my);
                    if pressed_dirty {
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                            win.dirty = true;
                        }
                    }
                    if let Some(action) = crate::shade::widgets::hit_test(wid.0, mx, my) {
                        crate::shade::widgets::push_event(wid.0, Event::Action(action));
                    }
                    // Always queue the raw button too — apps that want
                    // position-sensitive behaviour (canvas, drag) use
                    // it directly.
                    crate::shade::widgets::push_event(wid.0, Event::MouseButton {
                        button: MouseButton::Left,
                        down:   true,
                        x:      mx,
                        y:      my,
                    });
                }
                if focus_changed { return true; }
            }
        }

        // Mouse release: clear active state on every widget window
        // (we don't track which window held the press). Cheap — only
        // does work when the window's `active_path` was actually set.
        if self.mouse.left_released() {
            use crate::shade::widgets::abi::{Event, MouseButton};
            let widget_ids: alloc::vec::Vec<crate::shade::WindowId> = self.windows.iter()
                .filter(|w| w.kind == crate::shade::window::WindowKind::Widget)
                .map(|w| w.id)
                .collect();
            for wid in widget_ids {
                let released_dirty = crate::shade::widgets::release_at(wid.0);
                if released_dirty {
                    if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                        win.dirty = true;
                    }
                }
                crate::shade::widgets::push_event(wid.0, Event::MouseButton {
                    button: MouseButton::Left,
                    down:   false,
                    x:      mx,
                    y:      my,
                });
            }
        }

        false
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
            if win.id == fid || win.workspace != self.active_workspace || !win.visible || win.is_dock { continue; }
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
                duration: 15, // 150ms at 100Hz
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
    pub fn window_at(&self, x: i32, y: i32) -> Option<WindowId> {
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

    // ── Layer-based rendering ──────────────────────────────────────────

    /// Render all windows to layer buffers (Chrome → Layer 1, Text → Layer 2).
    /// Background (Layer 0) is rendered once in shade::init / force_redraw.
    pub fn render_to_layers(&mut self, info: &FbInfo) {
        use crate::layers::{LAYER_CHROME, LAYER_TEXT};

        if self.needs_full_redraw {
            crate::layers::clear(LAYER_CHROME);
            crate::layers::clear(LAYER_TEXT);
        }

        let border = self.border;
        let rounding = self.rounding;
        let opacity = self.opacity;
        let _scale = self.scale;

        // Render windows back to front
        for &wid in self.z_order.iter().rev() {
            if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                if win.workspace != self.active_workspace || !win.visible { continue; }

                let border_color = self.layer_border_color(win);
                Self::render_chrome_to_layer(info, win, border, rounding, opacity, border_color);
                self.render_text_to_layer(info, win);
            }
        }

        // Shadebar → Chrome layer
        if let Some((chrome_buf, _w, _h, _p)) = crate::layers::buffer(LAYER_CHROME) {
            self.bar.render(chrome_buf, info, self.screen_w, self.screen_h);
            let bar_y = self.bar.y(self.screen_h);
            crate::layers::mark_dirty(LAYER_CHROME, 0, bar_y, self.screen_w, self.bar.height);
        }

        for win in &mut self.windows {
            win.dirty = false;
        }
        self.needs_full_redraw = false;
    }

    /// Render only dirty windows to layer buffers.
    pub fn render_damaged_to_layers(&mut self, info: &FbInfo) {
        use crate::layers::LAYER_CHROME;

        if self.needs_full_redraw {
            self.render_to_layers(info);
            return;
        }

        let border = self.border;
        let rounding = self.rounding;
        let opacity = self.opacity;

        for wid_idx in (0..self.z_order.len()).rev() {
            let wid = self.z_order[wid_idx];
            let needs_render = self.windows.iter()
                .find(|w| w.id == wid)
                .map(|w| w.dirty && w.workspace == self.active_workspace && w.visible)
                .unwrap_or(false);

            if needs_render {
                if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                    let border_color = self.layer_border_color(win);
                    Self::render_chrome_to_layer(info, win, border, rounding, opacity, border_color);
                    self.render_text_to_layer(info, win);
                }
            }
        }

        if self.bar.dirty {
            if let Some((chrome_buf, _w, _h, _p)) = crate::layers::buffer(LAYER_CHROME) {
                self.bar.render(chrome_buf, info, self.screen_w, self.screen_h);
                let bar_y = self.bar.y(self.screen_h);
                crate::layers::mark_dirty(LAYER_CHROME, 0, bar_y, self.screen_w, self.bar.height);
            }
        }

        for win in &mut self.windows {
            win.dirty = false;
        }
    }

    /// Render window chrome (border + content bg) to Layer 1.
    /// Uses _alpha variants that write the alpha byte for layer compositing.
    fn render_chrome_to_layer(info: &FbInfo, win: &Window,
                              border: u32, rounding: u32, opacity: u32,
                              border_color: u32) {
        let chrome_buf = match crate::layers::buffer(crate::layers::LAYER_CHROME) {
            Some((buf, _, _, _)) => buf,
            None => return,
        };

        // Clear window region first (transparent)
        let pitch = info.pitch as usize;
        let x1 = (win.x + win.width).min(info.width);
        let y1 = (win.y + win.height).min(info.height);
        for row in win.y..y1 {
            let off = row as usize * pitch + win.x as usize * 4;
            let bytes = (x1 - win.x) as usize * 4;
            // SAFETY: bounds checked above
            unsafe { core::ptr::write_bytes(chrome_buf.add(off), 0, bytes); }
        }

        // Border (gradient or solid) — alpha byte set for compositor
        if crate::theme::is_active() && win.focused {
            let (ga, gb) = crate::theme::border_gradient();
            render::fill_rounded_rect_gradient_alpha(chrome_buf, info,
                win.x, win.y, win.width, win.height,
                ga, gb, rounding, 200);
        } else {
            render::fill_rounded_rect_alpha(chrome_buf, info,
                win.x, win.y, win.width, win.height,
                border_color, rounding, 180);
        }

        // Content area (inner rect with bg color) — alpha byte set
        let cx = win.content_x(border);
        let cy = win.content_y(border);
        let cw = win.content_w(border);
        let ch = win.content_h(border);
        let inner_r = rounding.saturating_sub(border);
        render::fill_rounded_rect_alpha(chrome_buf, info,
            cx, cy, cw, ch,
            win.bg_color, inner_r, opacity);

        crate::layers::mark_dirty(crate::layers::LAYER_CHROME,
            win.x, win.y, win.width, win.height);
    }

    /// Render terminal text for a window to Layer 2.
    pub fn render_text_to_layer(&self, info: &FbInfo, win: &Window) {
        let text_buf = match crate::layers::buffer(crate::layers::LAYER_TEXT) {
            Some((buf, _, _, _)) => buf,
            None => return,
        };

        let border = self.border;
        let scale = self.scale;
        let pad = 6 * scale;
        let cx = win.content_x(border) + pad;
        let cy = win.content_y(border) + pad;
        let cw = win.content_w(border).saturating_sub(pad * 2);
        let ch = win.content_h(border).saturating_sub(pad * 2);

        // Clear text region (transparent black)
        let pitch = info.pitch as usize;
        let x1 = (cx + cw).min(info.width);
        let y1 = (cy + ch).min(info.height);
        for row in cy..y1 {
            let off = row as usize * pitch + cx as usize * 4;
            let bytes = (x1 - cx) as usize * 4;
            // SAFETY: bounds checked
            unsafe { core::ptr::write_bytes(text_buf.add(off), 0, bytes); }
        }

        // Render text characters into text layer
        terminal::render_to_window(text_buf, info, cx, cy, cw, ch, scale, win.terminal_idx);

        crate::layers::mark_dirty(crate::layers::LAYER_TEXT, cx, cy, cw, ch);
    }

    /// Render only the input line to Layer 2 (fast path for typing).
    pub fn render_input_line_to_layer(&self, info: &FbInfo) -> Option<(u32, u32, u32, u32)> {
        let fid = self.focused?;
        let win = self.windows.iter().find(|w| w.id == fid && w.workspace == self.active_workspace)?;

        let text_buf = match crate::layers::buffer(crate::layers::LAYER_TEXT) {
            Some((buf, _, _, _)) => buf,
            None => return None,
        };

        let border = self.border;
        let scale = self.scale;
        let pad = 6 * scale;
        let cx = win.content_x(border) + pad;
        let cy = win.content_y(border) + pad;
        let cw = win.content_w(border).saturating_sub(pad * 2);
        let ch = win.content_h(border).saturating_sub(pad * 2);

        // Render input line directly to text layer (no cache hack needed!)
        terminal::render_input_line_to_layer(text_buf, info, cx, cy, cw, ch, win.terminal_idx)
    }

    /// Get border color for a window (theme-aware).
    fn layer_border_color(&self, win: &Window) -> u32 {
        if win.focused {
            if crate::theme::is_active() {
                crate::gui::background::accent_color()
            } else {
                self.border_active
            }
        } else {
            if crate::theme::is_active() {
                crate::theme::inactive_border()
            } else {
                self.border_inactive
            }
        }
    }
}

/// Parse a hex color string ("RRGGBB") to u32.
fn parse_hex_color(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches('#');
    if s.len() != 6 { return None; }
    u32::from_str_radix(s, 16).ok()
}

