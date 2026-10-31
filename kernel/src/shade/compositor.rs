//! Compositor — manages windows, Z-order, tiling layout, and rendering.
//!
//! No per-window pixel buffers. Windows are metadata (position, size, state).
//! The compositor renders directly to the framebuffer shadow buffer.
//! Uses dwindle layout (Hyprland-style recursive binary split).

use alloc::vec::Vec;
use crate::framebuffer::FbInfo;
use super::buffers::PixelBuffer;
use super::scene::{self, Fit, Item, Rect, Scene, Source};

use super::window::{Window, WindowId, WindowState};
use super::terminal;
use super::cursor::MouseState;
use super::controls;

/// Focus halo. The focused tile bleeds a little accent into the gap around
/// it: a 1 px border in a wallpaper-derived colour is invisible on half the
/// wallpapers, a halo reads on all of them because it does not compete with
/// the colour underneath. It settles at `STEADY`; the moment focus lands it
/// starts at `FLASH` and fades down over `FLASH_TICKS` (100 Hz timer), which
/// is what makes a focus change catch the eye anywhere on screen.
const GLOW_STEADY_ALPHA: u32 = 100;
const GLOW_FLASH_ALPHA: u32 = 255;
const GLOW_FLASH_TICKS: u64 = 24;
/// Border blend of the focused / unfocused tile (0..256). The gap between the
/// two carries the steady-state distinction together with the halo.
const BORDER_ACTIVE_OPACITY: u32 = 235;
const BORDER_INACTIVE_OPACITY: u32 = 130;

/// What to paint around a window: `band` is the strip reserved for the halo
/// (restored from the wallpaper on every repaint, so a tile that lost focus
/// erases its old halo), `alpha` is how strongly it is drawn right now.
#[derive(Clone, Copy)]
pub struct Glow {
    pub band: u32,
    pub alpha: u32,
    pub color: u32,
}

impl Glow {
    pub const NONE: Glow = Glow { band: 0, alpha: 0, color: 0 };
}

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

/// Drag mode: swap windows, resize a split, move a floating window, or
/// pull at the edges (`controls::EDGE_*` bits) of one.
#[derive(Clone, Copy, PartialEq)]
pub enum DragMode { Swap, Resize, Move, Edge(u8) }

/// Window + geometry under the cursor for a scrollbar hit-test/drag.
#[derive(Clone, Copy)]
pub struct ScrollHit {
    pub window: u32,
    pub is_terminal: bool,
    pub term_idx: u8,
    /// Terminal text rect `(x, y, w, h)` in screen coords (content minus
    /// the terminal padding) — the scrollbar track for terminals.
    pub text_rect: (i32, i32, u32, u32),
    /// Monospace cell height (px) — terminal rows = text_rect.h / char_h.
    pub char_h: u32,
    /// The window controls' corner, to exclude from the bar strip.
    pub close_rect: Option<(u32, u32, u32, u32)>,
}

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
    /// Which split line the drag moves per axis — resolved once at drag start
    /// (see `resize_target`). The dragged window is rarely the one holding the
    /// delta.
    pub w_target: Option<WindowId>,
    pub h_target: Option<WindowId>,
    /// The window's `(x, y, w, h)` when a Move or Edge drag started.
    pub start_geom: (u32, u32, u32, u32),
}

/// What the panel names a window by: its caption if it set one, else its title.
fn display_title(w: &Window) -> alloc::string::String {
    match (w.caption.is_empty(), w.kind) {
        (true, _) => w.title.clone(),
        // A guest picks its caption freely: the host's name for the
        // window goes first, so it cannot pass as a system window.
        (false, crate::shade::window::WindowKind::Surface) => {
            alloc::format!("{} · {}", w.title, w.caption)
        }
        (false, _) => w.caption.clone(),
    }
}

/// Bottom strut panel registered via `npk_window_set_panel(Bottom, Strut)`.
/// Reserves a `margin + pill_h` band at the bottom; its menus are popups.
#[derive(Clone, Copy)]
pub struct BottomStrut {
    pub id: WindowId,
    /// Band height in px (set_panel `h`).
    pub pill_h: u32,
    /// Gap to the screen edge / tiles (px).
    pub margin: u32,
}

/// Compositor manages all windows, the panel, and rendering state.
#[allow(dead_code)]
pub struct Compositor {
    /// Screen dimensions.
    pub screen_w: u32,
    pub screen_h: u32,
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
    /// Number of workspaces.
    pub workspace_count: u8,
    /// Gap between tiled windows (in pixels, scaled).
    pub gaps: u32,
    /// Window border width (in pixels, scaled).
    pub border: u32,
    /// Active window border color — `None` = follow the palette's
    /// `AccentLine`, so a wallpaper/theme/accent switch moves it live.
    pub border_active: Option<u32>,
    /// Inactive window border color — `None` = palette `Border`.
    pub border_inactive: Option<u32>,
    /// Config generation the chrome lengths and border colours were read at.
    chrome_gen: u32,
    /// The widget window last told `Event::Focus(true)`: the focused window,
    /// or the parent of the focused popup.
    focus_told: Option<WindowId>,
    /// Corner radius (in pixels, scaled).
    pub rounding: u32,
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
    /// Screen flash: the tick it started at. A shutter blink confirming a
    /// screenshot — the file itself lands seconds later, so without this
    /// the button feels dead. Painted over the finished frame, so it can
    /// never end up inside a capture.
    pub flash: Option<u64>,
    /// Focus flash: (window, tick it gained focus). The halo starts bright
    /// and settles into the steady one over `GLOW_FLASH_TICKS`, so a focus
    /// change is visible even when you were not looking at that corner.
    pub focus_glow: Option<(WindowId, u64)>,
    /// Timer tick the halo was last repainted at — the callers can run far
    /// more often than the animation has frames.
    glow_last_tick: u64,
    /// Bottom strut panel, if an app has registered one.
    pub bottom_strut: Option<BottomStrut>,
    /// The window that had the keyboard before the first popup took it.
    pub popup_prev_focus: Option<WindowId>,
    /// Text layer of each terminal window, by window id.
    pub term_layers: alloc::collections::BTreeMap<u32, PixelBuffer>,
    /// Pointer state of the window controls.
    pub controls: controls::State,
    /// A left press on a floating window's free bar: the window, and where
    /// the press was. Becomes a Move drag once the pointer travels.
    pub pending_move: Option<(WindowId, i32, i32)>,
    /// Screen areas to repaint on the next partial render that no dirty
    /// window covers (a hint or menu of the window controls).
    pub extra_damage: Vec<Rect>,
}

#[allow(dead_code)]
impl Compositor {
    pub fn new(screen_w: u32, screen_h: u32) -> Self {
        let (gaps, border, rounding) = chrome_lengths();
        let border_active = crate::config::get("shade.border_active")
            .and_then(|s| parse_hex_color(&s));
        let border_inactive = crate::config::get("shade.border_inactive")
            .and_then(|s| parse_hex_color(&s));

        Compositor {
            screen_w,
            screen_h,
            windows: Vec::new(),
            z_order: Vec::new(),
            next_id: 1,
            focused: None,
            active_workspace: 0,
            workspace_count: 4,
            gaps,
            border,
            border_active,
            border_inactive,
            chrome_gen: crate::config::generation(),
            focus_told: None,
            rounding,
            needs_full_redraw: true,
            aurora_drawn: false,
            mouse: {
                let mut m = MouseState::new();
                m.init(screen_w, screen_h);
                m
            },
            drag: None,
            animation: None,
            flash: None,
            focus_glow: None,
            glow_last_tick: 0,
            bottom_strut: None,
            popup_prev_focus: None,
            term_layers: alloc::collections::BTreeMap::new(),
            controls: controls::State::default(),
            pending_move: None,
            extra_damage: Vec::new(),
        }
    }

    /// Border of the focused tile. `AccentLine` (accent at 45 % over the
    /// surface) unless `shade.border_active` pins a hex value.
    pub fn border_active(&self) -> u32 {
        self.border_active.unwrap_or_else(|| {
            super::widgets::palette::resolve(super::widgets::abi::Token::AccentLine) & 0x00FF_FFFF
        })
    }

    /// Border of unfocused tiles. Palette `Border` unless pinned.
    pub fn border_inactive(&self) -> u32 {
        self.border_inactive.unwrap_or_else(|| {
            super::widgets::palette::resolve(super::widgets::abi::Token::Border) & 0x00FF_FFFF
        })
    }

    /// Border colour actually used for the focused / unfocused tile: a
    /// wallpaper theme overrides the palette tokens.
    fn active_border_color(&self) -> u32 {
        if let Some(c) = self.border_active {
            c
        } else if crate::theme::is_active() {
            crate::gui::background::accent_color()
        } else {
            self.border_active()
        }
    }

    fn inactive_border_color(&self) -> u32 {
        if let Some(c) = self.border_inactive {
            c
        } else if crate::theme::is_active() {
            crate::theme::inactive_border()
        } else {
            self.border_inactive()
        }
    }

    /// Width of the focus halo. `shade.glow` in px (0 disables), capped at
    /// the full gap: only one tile is ever focused, so two halos can never
    /// meet, and the band stops one pixel short of the neighbour's rect
    /// (tiles stand `gaps` apart, band columns are `[edge, edge + gaps)`).
    /// The workspace area is inset by `gaps` on all four sides, so the band
    /// stays on wallpaper at the screen edges and above the panel too.
    fn glow_width(&self) -> u32 {
        let w = crate::config::get("shade.glow")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(6);
        crate::shade::widgets::scale::len(w).min(self.gaps)
    }

    /// Halo for this window right now. Unfocused tiles keep the `band` (they
    /// have to repaint over their own former halo) but draw nothing.
    pub fn glow_for(&self, win: &Window) -> Glow {
        if win.is_overlay || win.state != WindowState::Tiled { return Glow::NONE }
        // The halo tells which tile has the keyboard; alone on the workspace
        // there is nothing to tell apart, and it only frames the screen.
        let tiles = self.windows.iter()
            .filter(|w| w.workspace == self.active_workspace && w.visible
                && w.state == WindowState::Tiled && !w.is_overlay)
            .count();
        if tiles < 2 { return Glow::NONE }
        let band = self.glow_width();
        if band == 0 { return Glow::NONE }
        if !win.focused { return Glow { band, alpha: 0, color: 0 } }
        let alpha = match self.focus_glow {
            Some((id, start)) if id == win.id => {
                let t = crate::interrupts::ticks().saturating_sub(start)
                    .min(GLOW_FLASH_TICKS) as u32;
                GLOW_FLASH_ALPHA
                    - (GLOW_FLASH_ALPHA - GLOW_STEADY_ALPHA) * t / GLOW_FLASH_TICKS as u32
            }
            _ => GLOW_STEADY_ALPHA,
        };
        Glow { band, alpha, color: self.active_border_color() }
    }

    /// Advance the focus flash. True while it still has a frame to draw —
    /// the last one settles the halo to its steady strength.
    pub fn tick_focus_glow(&mut self) -> bool {
        let Some((id, start)) = self.focus_glow else { return false };
        if !self.is_tiled(id) || self.focused != Some(id) {
            self.focus_glow = None;
            return false;
        }
        let now = crate::interrupts::ticks();
        // At most one repaint per timer tick. The callers are not a frame
        // clock: `poll_render` runs from a download's recv loop thousands of
        // times a second, and repainting the band that often would burn the
        // core for frames nobody can see — the alpha only moves once a tick.
        if now == self.glow_last_tick { return false }
        self.glow_last_tick = now;
        if now.saturating_sub(start) >= GLOW_FLASH_TICKS {
            self.focus_glow = None;
        }
        true
    }

    /// Repaint only the halo band of the focused tile. The flash animates
    /// nothing inside the window, so only the four gap strips and the four
    /// corner squares (where the halo wraps the arc) are composed.
    pub fn render_focus_glow(&mut self, target: *mut u8, info: &FbInfo) -> Vec<Rect> {
        let Some(fid) = self.focused else { return Vec::new() };
        let Some(win) = self.windows.iter()
            .find(|w| w.id == fid && w.workspace == self.active_workspace && w.visible)
            else { return Vec::new() };
        let glow = self.glow_for(win);
        if glow.band == 0 { return Vec::new() }
        let (x, y, w, h) = (win.x, win.y, win.width, win.height);
        let g = glow.band;
        let r = self.rounding.min(w / 2).min(h / 2);
        let screen = Rect::new(0, 0, info.width, info.height);
        let mut damage = alloc::vec![
            Rect::new(x.saturating_sub(g), y.saturating_sub(g), w + 2 * g, g),
            Rect::new(x.saturating_sub(g), y + h, w + 2 * g, g),
            Rect::new(x.saturating_sub(g), y, g, h),
            Rect::new(x + w, y, g, h),
        ];
        if r > 0 {
            for (cx, cy) in [(x, y), (x + w - r, y), (x, y + h - r), (x + w - r, y + h - r)] {
                damage.push(Rect::new(cx, cy, r, r));
            }
        }
        let damage: Vec<Rect> = damage.iter().map(|d| d.intersect(&screen)).filter(|d| !d.is_empty()).collect();
        self.compose(target, info, damage)
    }

    /// Height reserved at the bottom for the panel strut (0 if none).
    fn bottom_band(&self) -> u32 {
        self.bottom_strut.map(|s| s.margin + s.pill_h).unwrap_or(0)
    }

    /// Usable workspace area (excluding the bottom panel band).
    /// `workspace_area` for the controls.
    pub(super) fn workspace_area_pub(&self) -> (u32, u32, u32, u32) {
        self.workspace_area()
    }

    fn workspace_area(&self) -> (u32, u32, u32, u32) {
        let x = self.gaps;
        let y = self.gaps;
        let w = self.screen_w.saturating_sub(self.gaps * 2);
        let h = self.screen_h
            .saturating_sub(self.bottom_band())
            .saturating_sub(self.gaps * 2);
        (x, y, w, h)
    }

    /// Where a newly opened window hangs in the dwindle tree: it splits the
    /// focused window, so every tile, including the first, can be split
    /// again.
    ///
    /// Direction comes from the focused tile's shape: a wide one splits
    /// side-by-side, a tall one stacks. That is why the first split on a
    /// landscape screen goes to the right.
    fn dwindle_parent(&self) -> (Option<WindowId>, bool) {
        // 1. The focused window, but only if it really is a tile.
        if let Some(fid) = self.focused {
            if let Some(f) = self.windows.iter().find(|w| w.id == fid && self.is_tile(w)) {
                return (Some(fid), f.width >= f.height);
            }
        }

        // 2. Focus is on an overlay (launcher, menu). An overlay is not a
        //    place to tile into, but returning no parent would make the new
        //    window a second root: both roots cover the whole screen, their
        //    centres coincide, and `swap_direction` finds no neighbour in
        //    any direction. Take the topmost real tile instead, i.e. the
        //    window the user worked in before opening the launcher.
        for &id in &self.z_order {
            if let Some(f) = self.windows.iter().find(|w| w.id == id && self.is_tile(w)) {
                return (Some(id), f.width >= f.height);
            }
        }
        (None, true)
    }

    /// Can this window be the parent of a new tile?
    fn is_tile(&self, w: &Window) -> bool {
        w.workspace == self.active_workspace
            && w.state == WindowState::Tiled
            && w.visible
            && !w.is_overlay
            && !w.is_panel
    }

    /// Is this window part of the active workspace's tiling right now?
    fn is_tiled(&self, id: WindowId) -> bool {
        self.windows.iter().any(|w| w.id == id
            && w.workspace == self.active_workspace
            && w.state == WindowState::Tiled
            && w.visible
            && !w.is_overlay)
    }

    /// The fullscreen window on the active workspace, if any. A fullscreen
    /// window covers the whole screen over the panel and every tile;
    /// at most one is ever fullscreen at a time (focusing or toggling another
    /// steals the state). While one is active the panel and chrome are
    /// suppressed.
    pub fn active_fullscreen(&self) -> Option<WindowId> {
        self.windows.iter()
            .find(|w| w.state == WindowState::Fullscreen
                && w.workspace == self.active_workspace
                && w.visible)
            .map(|w| w.id)
    }

    /// Border actually painted around `win`: none for a fullscreen window (it
    /// owns the whole screen, no chrome), the configured width otherwise.
    /// Panels carry their own zero-border rule inside `build_scene`.
    pub fn effective_border(&self, win: &Window) -> u32 {
        if win.state == WindowState::Fullscreen { 0 } else { self.border }
    }

    /// Drop the fullscreen state of every fullscreen window except `keep`,
    /// back to a normal tile. Returns the windows that changed, so a caller
    /// can tell their guest the mode ended (WIN_STATE off). Does not retile —
    /// the caller lays the freed window out again.
    fn clear_fullscreen_except(&mut self, keep: Option<WindowId>) -> Vec<WindowId> {
        let mut changed = Vec::new();
        for w in self.windows.iter_mut() {
            if w.state == WindowState::Fullscreen && Some(w.id) != keep {
                w.state = WindowState::Tiled;
                w.dirty = true;
                changed.push(w.id);
            }
        }
        changed
    }

    /// Which stored delta a resize gesture on `id` moves, for one axis
    /// (`beside` = the vertical line between side-by-side tiles, else the
    /// horizontal one). `None` = no such line bounds it.
    ///
    /// The delta lives on the window that drew the line, so a window owns the
    /// line at only one of its edges; the opposite edge belongs to the last
    /// child that split it. A root window has no line of its own.
    ///
    /// A delta always sits on the child that drew the line and always pushes
    /// it towards the parent's origin (left / up), so "move the line the way
    /// the arrow points" is a fixed `- delta` for both callers — see
    /// `resize_focused`.
    fn resize_target(&self, id: WindowId, beside: bool) -> Option<WindowId> {
        let mut cur = id;
        // Bounded walk: a corrupt parent chain must not spin the compositor.
        for _ in 0..self.windows.len() {
            let w = self.windows.iter().find(|w| w.id == cur)?;
            let parent = w.split_from.filter(|p| self.is_tiled(*p));
            if parent.is_some() && w.split_beside == beside {
                return Some(cur);
            }
            if let Some(kid) = self.windows.iter().rev()
                .find(|c| c.split_from == Some(cur) && c.split_beside == beside && self.is_tiled(c.id))
                .map(|c| c.id)
            {
                return Some(kid);
            }
            cur = parent?;
        }
        None
    }

    /// The window holding the split line the focused tile sits on, whatever
    /// its orientation — its own if it has one, else the one its last child
    /// drew. Used by `toggle_split`.
    pub(super) fn split_line_owner(&self, id: WindowId) -> Option<WindowId> {
        let w = self.windows.iter().find(|w| w.id == id)?;
        if w.split_from.filter(|p| self.is_tiled(*p)).is_some() {
            return Some(id);
        }
        self.windows.iter().rev()
            .find(|c| c.split_from == Some(id) && self.is_tiled(c.id))
            .map(|c| c.id)
    }

    /// Create a new window and add it to the current workspace.
    /// Returns None if no terminal slots available.
    pub fn create_window(&mut self, title: &str, x: u32, y: u32, w: u32, h: u32) -> Option<WindowId> {
        let terminal_idx = terminal::allocate()?;

        let id = WindowId(self.next_id);
        self.next_id += 1;

        let mut win = Window::new(id, title, x, y, w, h);
        win.workspace = self.active_workspace;
        (win.split_from, win.split_beside) = self.dwindle_parent();
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

    /// Create a widget-kind window for a GUI app. Doesn't
    /// allocate a terminal buffer (widget apps aren't text-driven).
    /// Focus stays on the current window so the spawning shell keeps
    /// receiving the user's input.
    pub fn create_widget_window(&mut self, title: &str) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;

        let mut win = Window::new(id, title, 0, 0, 100, 100);
        win.workspace = self.active_workspace;
        (win.split_from, win.split_beside) = self.dwindle_parent();
        win.terminal_idx = 255; // sentinel — no terminal buffer owned
        win.kind = crate::shade::window::WindowKind::Widget;
        win.pid = 0;

        self.windows.push(win);
        self.z_order.insert(0, id);
        // Deliberately not focus_window(id) — keep focus on the shell
        // that spawned us, so the user's next keystroke lands there.
        // But this insert(0) put us above the panel, so re-pin it on top.
        self.retile();
        self.pin_overlays_to_front();
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
        (win.split_from, win.split_beside) = self.dwindle_parent();
        win.terminal_idx = 255; // sentinel — no terminal buffer owned
        win.kind = crate::shade::window::WindowKind::Surface;
        win.pid = 0;

        self.windows.push(win);
        self.z_order.insert(0, id);
        // Tile it once so the guest learns the size it will have, then hide
        // it and tile the rest back: the window appears, and its neighbours
        // move aside, only with the app's first frame (`reveal_surface`).
        // Both passes run before any render, so nothing shows in between.
        self.retile();
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.visible = false;
        }
        self.retile();
        self.pin_overlays_to_front();
        self.needs_full_redraw = true;

        id
    }

    /// Set what the app inside the window calls it (`Window::caption`).
    pub fn set_caption(&mut self, id: WindowId, caption: alloc::string::String) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.caption = caption;
        }
    }

    /// Show a Surface window hidden by `create_surface_window` and tile it
    /// in. It takes focus only when the user is already with the guest app
    /// (a guest window has focus) or just launched it (no other guest
    /// window). The guest decides when its windows open: focusing every one
    /// would let it pull the keyboard, and with it a passphrase being typed,
    /// away from a host window. No-op for a window that is gone or shown.
    pub fn reveal_surface(&mut self, id: WindowId) {
        use crate::shade::window::WindowKind;
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else { return };
        if w.visible || w.kind != WindowKind::Surface {
            return;
        }
        w.visible = true;
        w.dirty = true;
        let first = !self.windows.iter()
            .any(|w| w.id != id && w.kind == WindowKind::Surface && w.visible);
        let guest_focused = self.focused.is_some_and(|f| {
            self.windows.iter().any(|w| w.id == f && w.kind == WindowKind::Surface)
        });
        self.retile();
        if first || guest_focused {
            self.focus_window(id);
        }
        self.needs_full_redraw = true;
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
        let (id, pid) = {
            let win = self.windows.iter_mut().find(|w| {
                w.terminal_idx == terminal_idx
                    && w.kind == crate::shade::window::WindowKind::Terminal
            })?;
            win.kind = crate::shade::window::WindowKind::Widget;
            win.terminal_idx = 255;
            let pid = win.pid;
            win.pid = 0;
            win.dirty = true;
            (win.id, pid)
        };
        // Drop the "loop" process-table entry that create_window
        // allocated — for a widget the app runs as its own KIND_WASM
        // process (registered by the spawn path), so the loop PID would be
        // a misleading orphan (close_window only frees a pid in its
        // Terminal arm, which a promoted Widget window never reaches).
        // Exiting a pid only touches the PROCS map. We deliberately do not
        // free the session or terminal buffer here: the terminal's intent
        // loop is still live and holds a long-lived `&mut IntentSession`,
        // so freeing them mid-flight is a use-after-free. Their lifecycle
        // stays tied to the window via close_window. Not implemented:
        // freeing the session/terminal slot of a promoted widget.
        if pid != 0 { crate::process::exit(pid); }
        // Re-tile: the promoted window kept whatever geometry it had as a
        // terminal (often fullscreen). Without this a second app launched
        // from the panel would stack behind the first instead of splitting.
        // Overlay/panel apps (drun, deck) call set_overlay/set_panel
        // right after, which un-tiles them again, so this is a no-op for them.
        self.retile();
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

    /// Send `Event::Focus` when the keyboard moved to another app: false to
    /// the widget window that had it, true to the one that has it now. A
    /// popup counts as its parent, so opening a menu changes nothing.
    fn tell_focus(&mut self) {
        let now = self.focused
            .and_then(|f| self.windows.iter().find(|w| w.id == f))
            .map(|w| w.popup_of.unwrap_or(w.id))
            .filter(|id| self.windows.iter().any(|w| w.id == *id
                && w.kind == crate::shade::window::WindowKind::Widget));
        if now == self.focus_told { return; }
        use crate::shade::widgets::abi::Event;
        if let Some(old) = self.focus_told {
            if self.windows.iter().any(|w| w.id == old) {
                crate::shade::widgets::push_event(old.0, Event::Focus(false));
            }
        }
        if let Some(new) = now {
            crate::shade::widgets::push_event(new.0, Event::Focus(true));
        }
        self.focus_told = now;
    }

    /// Take a `set shade.gaps|border|rounding|border_active|border_inactive`
    /// at once: re-read them when the config changed, re-tile on a new
    /// length.
    fn refresh_chrome(&mut self) {
        let generation = crate::config::generation();
        if generation == self.chrome_gen { return; }
        self.chrome_gen = generation;
        self.border_active = crate::config::get("shade.border_active")
            .and_then(|s| parse_hex_color(&s));
        self.border_inactive = crate::config::get("shade.border_inactive")
            .and_then(|s| parse_hex_color(&s));
        let lengths = chrome_lengths();
        if lengths != (self.gaps, self.border, self.rounding) {
            (self.gaps, self.border, self.rounding) = lengths;
            self.retile();
        }
        self.needs_full_redraw = true;
    }

    /// The UI scale changed from `old` to `new` (1/120 units): apps sized
    /// their panel and overlays once, in logical px, so convert the
    /// physical sizes here. Popups close with `PopupDone`; the app opens
    /// them again at the new size.
    pub fn rescale_app_geometry(&mut self, old: u32, new: u32) {
        if old == 0 || old == new { return; }
        (self.gaps, self.border, self.rounding) = chrome_lengths();
        let f = |v: u32| ((v as u64 * new as u64 + old as u64 / 2) / old as u64) as u32;
        let popups: alloc::vec::Vec<WindowId> = self.windows.iter()
            .filter(|w| w.popup_of.is_some()).map(|w| w.id).collect();
        for id in popups { self.close_popup(id, true); }
        if let Some(b) = self.bottom_strut { self.set_bottom_panel(b.id, f(b.pill_h)); }
        let (sw, sh) = (self.screen_w, self.screen_h);
        for win in self.windows.iter_mut()
            .filter(|w| w.is_overlay && !w.is_panel && w.popup_of.is_none())
        {
            win.width = f(win.width).clamp(1, sw);
            win.height = f(win.height).clamp(1, sh);
            win.x = f(win.x).min(sw - win.width);
            win.y = f(win.y).min(sh - win.height);
            win.dirty = true;
        }
        self.retile();
        self.needs_full_redraw = true;
    }

    /// Toggle the `modal` flag on a window.
    pub fn set_modal(&mut self, id: WindowId, modal: bool) -> bool {
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.modal = modal;
            // Raise it right away: from this moment it swallows clicks
            // outside itself, so it had better be the thing on top.
            if modal { self.pin_overlays_to_front(); }
            true
        } else {
            false
        }
    }

    /// Position an overlay window's top-left at `(x, y)` with size `(w, h)`,
    /// clamped to the screen. Like `set_overlay` but app-positioned instead
    /// of centred — for dropdowns that anchor to a screen corner (e.g. the
    /// volume slider next to the panel). Does not touch focus.
    pub fn set_overlay_at(&mut self, id: WindowId, x: i32, y: i32, w: u32, h: u32) -> bool {
        let screen_w = self.screen_w;
        let screen_h = self.screen_h;
        let ow = w.min(screen_w).max(60);
        let oh = h.min(screen_h).max(40);
        let ox = x.clamp(0, screen_w.saturating_sub(ow) as i32) as u32;
        let oy = y.clamp(0, screen_h.saturating_sub(oh) as i32) as u32;
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
            self.retile();
            self.needs_full_redraw = true;
        }
        changed
    }

    /// Toggle light-dismiss (close-on-outside-click) for a window.
    pub fn set_light_dismiss(&mut self, id: WindowId, on: bool) -> bool {
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.light_dismiss = on;
            true
        } else {
            false
        }
    }

    /// A visible light-dismiss window not containing `(x, y)`, if any — the
    /// click-handler closes it so transient overlays vanish on an outside
    /// click. (Clicks inside keep it open to interact with.)
    fn light_dismiss_outside(&self, x: i32, y: i32) -> Option<WindowId> {
        self.windows.iter().find(|w| {
            w.light_dismiss && w.visible && !(
                x >= w.x as i32 && x < (w.x + w.width) as i32 &&
                y >= w.y as i32 && y < (w.y + w.height) as i32)
        }).map(|w| w.id)
    }

    /// `npk_window_set_panel` host fn: configure `id` as the panel.
    /// Only `edge` 0 (bottom) with `behavior` 1 (strut) exists; any other
    /// combination returns false. `w` is advisory.
    pub fn set_panel(&mut self, id: WindowId, edge: u8, behavior: u8, _w: u32, h: u32) -> bool {
        match (edge, behavior) {
            (0, 1) => self.set_bottom_panel(id, h),
            _ => false,
        }
    }

    /// Bottom strut: full width minus the margin, always visible, global
    /// across workspaces, never focused.
    fn set_bottom_panel(&mut self, id: WindowId, h: u32) -> bool {
        let screen_w = self.screen_w;
        let screen_h = self.screen_h;
        let margin = bar_margin();
        let pill_h = h.max(22);
        let active_ws = self.active_workspace;

        let found = if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.state = crate::shade::window::WindowState::Floating;
            win.is_overlay = true;
            win.is_panel = true;
            win.modal = false;
            win.workspace = active_ws;
            win.x = margin;
            win.y = screen_h.saturating_sub(margin + pill_h);
            win.width = screen_w.saturating_sub(margin * 2).max(120);
            win.height = pill_h;
            win.visible = true;
            win.dirty = true;
            true
        } else {
            false
        };
        if !found { return false; }

        self.bottom_strut = Some(BottomStrut { id, pill_h, margin });
        crate::shade::widgets::palette::set_panel_window(Some(id.0));
        if self.focused == Some(id) { self.focused = None; }
        self.retile();
        self.z_order.retain(|&wid| wid != id);
        self.z_order.insert(0, id);
        self.pin_overlays_to_front();
        self.needs_full_redraw = true;
        true
    }

    /// Live state the panel renders: (workspace_count, active_workspace,
    /// focused window title). Fed to WASM via `npk_bar_state`.
    pub fn bar_info(&self) -> (u8, u8, alloc::string::String) {
        // Title of the focused real app window, never a panel/overlay. If
        // nothing real is focused (panels clear focus), fall back to the
        // topmost real window on the active workspace so the panel still
        // shows what's open.
        let is_app = |w: &&Window| !w.is_overlay && !w.is_panel;
        let title = self.focused
            .and_then(|fid| self.windows.iter().find(|w| w.id == fid))
            .filter(is_app)
            .or_else(|| {
                self.z_order.iter()
                    .filter_map(|wid| self.windows.iter().find(|w| w.id == *wid))
                    .find(|w| is_app(w) && w.workspace == self.active_workspace && w.visible)
            })
            .map(display_title)
            .unwrap_or_default();
        (self.workspace_count, self.active_workspace, title)
    }

    /// A fingerprint of everything `bar_info` and `window_lines` report —
    /// focus, workspaces, and each window's identity, place, visibility,
    /// kind and title, in z-order. Compared after every full frame
    /// (`render_frame_layered`); a change notifies `TOPIC_WINDOWS`. No
    /// allocation: FNV-1a over the fields.
    pub fn shell_fingerprint(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |b: &[u8]| {
            for &x in b {
                h ^= x as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(&[self.workspace_count, self.active_workspace]);
        eat(&self.focused.map_or(u32::MAX, |f| f.0).to_le_bytes());
        for wid in &self.z_order {
            let Some(w) = self.windows.iter().find(|w| w.id == *wid) else { continue };
            eat(&w.id.0.to_le_bytes());
            eat(&[w.workspace, w.visible as u8, w.is_overlay as u8, w.is_panel as u8]);
            eat(w.title.as_bytes());
            eat(&[0xFF]);
            eat(w.caption.as_bytes());
            eat(&[0xFF]);
        }
        h
    }

    /// One line per real app window:
    /// `<flags>\t<workspace>\t<title>\t<id>\t<caption>`, flags being a
    /// decimal bitmask (1 = focused, 2 = on the active workspace), `title`
    /// the module name and `caption` what the panel shows for it (tabs and
    /// line breaks blanked). Panels and transient overlays are excluded.
    /// Feeds the panel's window list; the kernel names no app. `id` and `caption` only with `full`: a caption is what
    /// another app shows (a page title), so only the shell may read it.
    pub fn window_lines(&self, full: bool) -> alloc::string::String {
        use core::fmt::Write;
        let mut out = alloc::string::String::new();
        for wid in &self.z_order {
            let Some(w) = self.windows.iter().find(|w| w.id == *wid) else { continue };
            if w.is_overlay || w.is_panel { continue; }
            let mut flags = 0u8;
            if Some(w.id) == self.focused { flags |= 1; }
            if w.workspace == self.active_workspace { flags |= 2; }
            if !full {
                let _ = writeln!(out, "{}\t{}\t{}", flags, w.workspace, w.title);
                continue;
            }
            let caption: alloc::string::String = display_title(w).chars()
                .map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c })
                .collect();
            let _ = writeln!(out, "{}\t{}\t{}\t{}\t{}",
                flags, w.workspace, w.title, w.id.0, caption);
        }
        out
    }

    /// Every window with its real state: the view needed when an app is
    /// visually closed but still lives on in `top` and the panel.
    ///
    /// `window_lines` (what the panel sees) shows only titles and omits
    /// overlays and panels, which hides exactly which window survived and
    /// of which kind.
    pub fn dump_windows(&self) -> alloc::string::String {
        use core::fmt::Write;
        let mut out = alloc::string::String::new();
        let _ = writeln!(out,
            "  ID  KIND      TERM  PID   STATE     FLAGS                 TITLE");
        let _ = writeln!(out,
            "  ──  ────      ────  ───   ─────     ─────                 ─────");
        for wid in &self.z_order {
            let Some(w) = self.windows.iter().find(|w| w.id == *wid) else { continue };
            let kind = match w.kind {
                crate::shade::window::WindowKind::Terminal => "Terminal",
                crate::shade::window::WindowKind::Widget => "Widget  ",
                crate::shade::window::WindowKind::Surface => "Surface ",
            };
            let state = match w.state {
                WindowState::Tiled => "Tiled   ",
                _ => "Floating",
            };
            let mut flags = alloc::string::String::new();
            if Some(w.id) == self.focused { flags.push_str("focused "); }
            if w.is_overlay { flags.push_str("overlay "); }
            if w.is_panel { flags.push_str("panel "); }
            if w.split_from.is_none() { flags.push_str("ROOT "); }
            if !w.visible { flags.push_str("hidden "); }
            let _ = writeln!(out, "  {:>2}  {}  {:>4}  {:>3}   {}  {:<21} {}",
                w.id.0, kind, w.terminal_idx, w.pid, state, flags.trim_end(), w.title);
        }
        out
    }

    /// Something moves on its own and needs the next frame (see
    /// `shade::needs_tick`).
    pub fn needs_tick(&self) -> bool {
        self.animation.is_some() || self.flash.is_some() || self.focus_glow.is_some()
            || self.controls_need_tick()
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

    /// Is `id` the panel? Panels are managed chrome: they never hold
    /// focus and must not be closed by Mod+Q.
    pub fn is_panel(&self, id: WindowId) -> bool {
        self.bottom_strut.map(|s| s.id) == Some(id)
    }

    /// A user gesture asked for `id` to close (Mod+Q, the title-bar X).
    ///
    /// A window that opted into `npk_window_set_close_guard` gets an
    /// `Event::CloseRequest` and stays open so it can prompt about unsaved
    /// work. Asking a second time closes it for real — the guard buys one
    /// round of politeness, not a veto. Everything else closes at once,
    /// round of politeness, not a veto. Everything else closes at once.
    pub fn request_close_window(&mut self, id: WindowId) {
        if crate::shade::widgets::is_close_guarded(id.0) {
            let now = crate::interrupts::ticks();
            if crate::shade::widgets::begin_close_request(id.0, now) {
                crate::shade::widgets::push_event(
                    id.0, crate::shade::widgets::abi::Event::CloseRequest);
                return;
            }
            // Second gesture — the app had its chance.
        }
        self.close_window(id);
    }

    /// Ask every app window to close, as the user's close gesture would: a
    /// guarded one gets `CloseRequest`, a microVM window asks its guest to
    /// end. The panel, popups and terminals stay. Returns how many were asked.
    pub fn request_close_all_apps(&mut self) -> usize {
        let ids: Vec<WindowId> = self.windows.iter()
            .filter(|w| Self::is_app_window(w))
            .map(|w| w.id)
            .collect();
        for &id in &ids {
            if !crate::shade::widgets::close_pending_for(id.0) {
                self.request_close_window(id);
            }
        }
        ids.len()
    }

    /// App windows still open: what a shutdown waits for.
    pub fn app_windows_left(&self) -> usize {
        self.windows.iter().filter(|w| Self::is_app_window(w)).count()
    }

    fn is_app_window(w: &Window) -> bool {
        !w.is_panel && w.popup_of.is_none() && w.kind != crate::shade::window::WindowKind::Terminal
    }

    /// Title of a window, for messages.
    pub fn window_title(&self, id: WindowId) -> Option<alloc::string::String> {
        self.windows.iter().find(|w| w.id == id).map(|w| w.title.clone())
    }

    /// Close guarded windows that never answered their `CloseRequest`, so
    /// a hung app can't leave an immortal window behind.
    pub fn tick_close_requests(&mut self) -> bool {
        let now = crate::interrupts::ticks();
        let expired = crate::shade::widgets::expired_close_requests(now);
        if expired.is_empty() { return false; }
        for id in expired {
            crate::kprintln!("[shade] close guard expired for window {} — closing", id);
            self.close_window(WindowId(id));
        }
        true
    }

    /// Close a window by ID.
    pub fn close_window(&mut self, id: WindowId) {
        // Any file dialog this window opened goes with it. Collected (and
        // deregistered) first so closing them doesn't try to report a
        // cancel back to the window we're about to remove. Recursion
        // terminates: a picker owns no pickers of its own.
        for picker in crate::shade::widgets::take_picks_for_requester(id.0) {
            self.close_window(WindowId(picker));
        }

        // A popup closes as a popup; a window takes its popups with it.
        if self.windows.iter().any(|w| w.id == id && w.popup_of.is_some()) {
            self.close_popup(id, true);
            return;
        }
        self.close_popups_of(id, false);

        // Panel gone: free its strut band; tiles reclaim the height.
        if self.bottom_strut.map(|s| s.id) == Some(id) {
            self.bottom_strut = None;
            crate::shade::widgets::palette::set_panel_window(None);
            self.retile();
            self.needs_full_redraw = true;
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
                    // A promoted-from-terminal widget clears its loop PID
                    // in promote_terminal_to_widget; this is belt-and-
                    // suspenders for any path that leaves a pid set.
                    if win.pid != 0 { crate::process::exit(win.pid); }
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
        // Dwindle: hand this window's children to its own parent, in its
        // place. Without it they would each become a root and spread across
        // the whole screen instead of taking over the space that just freed
        // up — which is the one thing closing a window must do.
        self.reparent_children(id);
        self.windows.retain(|w| w.id != id);
        self.z_order.retain(|&wid| wid != id);

        if self.focused == Some(id) {
            self.refocus_active_workspace();
        }

        self.retile();
        self.needs_full_redraw = true;
    }

    /// Hand `id`'s children to its own parent, in its place — for every path
    /// that takes a window out of a workspace's tree (close, or move away).
    /// Without it they each become a root and spread over the whole screen
    /// instead of taking over the region that just freed up.
    fn reparent_children(&mut self, id: WindowId) {
        let Some((inherited, beside, rw, rh)) = self.windows.iter().find(|w| w.id == id)
            .map(|w| (w.split_from, w.split_beside, w.resize_w, w.resize_h)) else { return };
        let mut first = true;
        for w in &mut self.windows {
            if w.split_from == Some(id) {
                w.split_from = inherited;
                // The eldest child takes over the departing window's slot, so
                // it inherits the direction and the line's offset — the delta
                // belongs to the line, not to the window that happens to hold
                // it. Later children keep their own and chain after it,
                // exactly as they did underneath.
                if first {
                    w.split_beside = beside;
                    w.resize_w = rw;
                    w.resize_h = rh;
                    first = false;
                }
            }
        }
    }

    /// Focus the topmost real window of the active workspace and point
    /// ACTIVE_IDX + the cursor at it. Panels are never focused —
    /// otherwise closing the last app window would focus the panel and the
    /// next Mod+Q would close it. No real window left → focus None (empty
    /// desktop) and Mod+Q no-ops.
    ///
    /// Every path that makes the focused window leave the screen must call
    /// this: leaving ACTIVE_IDX behind means the loop keeps serving a terminal
    /// nobody can see, and its input/output/cursor fall through to serial.
    fn refocus_active_workspace(&mut self) {
        self.focused = self.z_order.iter()
            .filter(|&&wid| !self.is_panel(wid))
            .find_map(|&top_id| {
                self.windows.iter().find(|w| w.id == top_id && w.workspace == self.active_workspace)
            })
            .map(|w| w.id);
        if let Some(fid) = self.focused {
            self.set_focused_flag(fid);
            self.sync_active_terminal(fid);
        }
    }

    /// Point ACTIVE_IDX + the cursor at `fid`'s terminal buffer. Widget and
    /// Surface windows own none, so ACTIVE_IDX stays where it was and kprintln
    /// keeps a valid sink while such a window is focused.
    fn sync_active_terminal(&self, fid: WindowId) {
        if let Some(win) = self.windows.iter().find(|w| w.id == fid) {
            if win.kind == crate::shade::window::WindowKind::Terminal {
                terminal::set_active_terminal(win.terminal_idx);
                terminal::restore_cursor();
            }
        }
    }

    /// Set focus to a window.
    pub fn focus_window(&mut self, id: WindowId) {
        // Panels are never focusable — focusing them would
        // route keyboard input into a window that ignores it (the shell
        // appears to hang). Refuse, no matter which path asked.
        if self.windows.iter().any(|w| w.id == id && (w.is_panel)) {
            return;
        }
        self.focused = Some(id);
        self.set_focused_flag(id);

        self.z_order.retain(|&wid| wid != id);
        self.z_order.insert(0, id);

        if let Some(win) = self.windows.iter().find(|w| w.id == id) {
            // Widget windows don't own a terminal buffer — leave ACTIVE_IDX
            // pointing at the previously-active terminal so kprintln output
            // keeps a valid sink while the widget app is focused.
            if win.kind == crate::shade::window::WindowKind::Terminal {
                terminal::set_active_terminal(win.terminal_idx);
                terminal::restore_cursor();
            }
        }
        // Focusing a different real window drops any other window's
        // fullscreen — it no longer owns the screen. Its guest learns the
        // new (tiled) size through the retile here. Popups and overlays do
        // not steal fullscreen: they float over it.
        let real = self.windows.iter()
            .any(|w| w.id == id && w.popup_of.is_none() && !w.is_overlay);
        if real && !self.clear_fullscreen_except(Some(id)).is_empty() {
            self.retile();
            self.needs_full_redraw = true;
        }
        // Popups and modals must stay above the just-focused window.
        self.pin_overlays_to_front();
        // Don't set needs_full_redraw — render_damaged handles 2 windows only
    }

    /// Bring an app window to the front from a panel's window list: switch
    /// to its workspace and focus it. Panels and overlays are refused.
    pub fn activate_window(&mut self, id: WindowId) -> bool {
        let Some(ws) = self.windows.iter()
            .find(|w| w.id == id && !w.is_overlay && !w.is_panel)
            .map(|w| w.workspace)
        else { return false };
        self.switch_workspace(ws);
        self.focus_window(id);
        self.needs_full_redraw = true;
        true
    }

    // === Popups, after xdg_popup ===

    /// Popup `slot` of `parent`, if it is open.
    pub fn popup_window(&self, parent: WindowId, slot: u8) -> Option<WindowId> {
        self.windows.iter()
            .find(|w| w.popup_of == Some(parent) && w.popup_slot == slot)
            .map(|w| w.id)
    }

    /// Open popup `slot` of `parent`: a floating widget window, hidden until
    /// `place_popup`, holding the keyboard (`xdg_popup.grab`).
    pub fn open_popup(&mut self, parent: WindowId, slot: u8) -> Option<WindowId> {
        let (ws, title, panel) = self.windows.iter().find(|w| w.id == parent)
            .map(|w| (w.workspace, w.title.clone(), w.is_panel))?;
        // A new popup takes the keyboard, so only the app the user is
        // working with may open one: its window or one of its popups has the
        // focus, or it just got a press or key (a panel is never focused).
        let owns_focus = self.focused.is_some_and(|f| f == parent
            || self.windows.iter().any(|w| w.id == f && w.popup_of == Some(parent)));
        if !owns_focus && !crate::shade::widgets::had_recent_input(parent.0) {
            return None;
        }
        let id = WindowId(self.next_id);
        self.next_id += 1;
        let mut win = Window::new(id, &title, 0, 0, 1, 1);
        win.kind = crate::shade::window::WindowKind::Widget;
        win.terminal_idx = 255;
        win.state = WindowState::Floating;
        win.is_overlay = true;
        win.workspace = ws;
        win.visible = false;
        win.popup_of = Some(parent);
        win.popup_slot = slot;
        let first = !self.windows.iter().any(|w| w.popup_of.is_some());
        self.windows.push(win);
        self.z_order.insert(0, id);
        crate::shade::widgets::set_popup_parent(id.0, Some(parent.0));
        crate::shade::widgets::palette::set_popup_panel(id.0, panel);
        if first { self.popup_prev_focus = self.focused; }
        self.focus_window(id);
        self.pin_overlays_to_front();
        Some(id)
    }

    /// Size and place a popup next to `anchor` (screen rectangle): below
    /// it at `gap`, flipped above when it does not fit (`flip_y`), slid
    /// sideways to stay on screen (`slide_x`).
    pub fn place_popup(&mut self, id: WindowId, anchor: (i32, i32, u32, u32), w: u32, h: u32, gap: u32) {
        let (sw, sh) = (self.screen_w as i32, self.screen_h as i32);
        let w = w.clamp(1, self.screen_w);
        let h = h.clamp(1, self.screen_h);
        let (ax, ay, _aw, ah) = anchor;
        let below = ay + ah as i32 + gap as i32;
        let above = ay - gap as i32 - h as i32;
        let y = if below + h as i32 <= sh {
            below
        } else if above >= 0 {
            above
        } else {
            (sh - h as i32).max(0)
        };
        let x = ax.clamp(0, (sw - w as i32).max(0));
        let Some(win) = self.windows.iter_mut().find(|win| win.id == id) else { return };
        // Only a popup that appears, moves or changes size uncovers what was
        // behind it; otherwise its own damage is enough.
        let moved = !win.visible || (win.x, win.y, win.width, win.height) != (x as u32, y as u32, w, h);
        win.x = x as u32;
        win.y = y as u32;
        win.width = w;
        win.height = h;
        win.visible = true;
        win.dirty = true;
        if moved { self.needs_full_redraw = true; }
    }

    /// Take a popup off the screen, telling its app when `done`. Focus is
    /// the caller's business.
    fn drop_popup(&mut self, id: WindowId, done: bool) {
        let Some((parent, slot)) = self.windows.iter().find(|w| w.id == id)
            .and_then(|w| w.popup_of.map(|p| (p, w.popup_slot))) else { return };
        if done {
            crate::shade::widgets::push_event(parent.0,
                crate::shade::widgets::abi::Event::PopupDone { popup: slot as u32 });
        }
        crate::shade::widgets::remove_scene(id.0);
        crate::shade::widgets::remove_event_queue(id.0);
        crate::shade::widgets::set_popup_parent(id.0, None);
        crate::shade::widgets::palette::set_popup_panel(id.0, false);
        self.windows.retain(|w| w.id != id);
        self.z_order.retain(|&w| w != id);
        if self.focused == Some(id) { self.focused = None; }
        self.needs_full_redraw = true;
    }

    /// Close a popup. The keyboard goes to another popup if one is left,
    /// else back to the window that had it before the first one opened.
    pub fn close_popup(&mut self, id: WindowId, done: bool) {
        let had_focus = self.focused == Some(id);
        self.drop_popup(id, done);
        if !had_focus { return; }
        if let Some(other) = self.windows.iter().find(|w| w.popup_of.is_some()).map(|w| w.id) {
            self.focus_window(other);
            return;
        }
        let back = self.popup_prev_focus.take().filter(|f| self.windows.iter()
            .any(|w| w.id == *f && w.workspace == self.active_workspace && w.visible));
        match back {
            Some(f) => self.focus_window(f),
            None => self.refocus_active_workspace(),
        }
    }

    /// Close every popup of `parent`.
    pub fn close_popups_of(&mut self, parent: WindowId, done: bool) {
        let ids: Vec<WindowId> = self.windows.iter()
            .filter(|w| w.popup_of == Some(parent)).map(|w| w.id).collect();
        for id in ids { self.close_popup(id, done); }
    }

    /// A press outside every popup dismisses them all and is consumed.
    /// True when it did.
    fn popup_dismiss_at(&mut self, x: i32, y: i32) -> bool {
        let popups: Vec<(WindowId, bool)> = self.windows.iter()
            .filter(|w| w.popup_of.is_some() && w.visible)
            .map(|w| (w.id, x >= w.x as i32 && x < (w.x + w.width) as i32
                && y >= w.y as i32 && y < (w.y + w.height) as i32))
            .collect();
        if popups.is_empty() || popups.iter().any(|&(_, inside)| inside) { return false; }
        for (id, _) in popups { self.close_popup(id, true); }
        true
    }

    /// Keep popups and the modal window at the front of the z-order. Render
    /// passes iterate `z_order.rev()`, drawing index 0 last → on top.
    fn pin_overlays_to_front(&mut self) {
        self.floats_above_tiles();
        // Popups above every panel and window they open from.
        let popups: Vec<WindowId> = self.z_order.iter().rev().copied()
            .filter(|id| self.windows.iter().any(|w| w.id == *id && w.popup_of.is_some()))
            .collect();
        for id in popups {
            self.z_order.retain(|&wid| wid != id);
            self.z_order.insert(0, id);
        }
        // A modal dialog must sit above everything, and this is not
        // cosmetic: `modal_blocks` swallows every click outside it, so a
        // buried modal would eat the pointer while invisible — the whole
        // screen would go dead. Pinning it last puts it in front of the
        // popups too.
        if let Some(id) = self.modal_window() {
            self.z_order.retain(|&wid| wid != id);
            self.z_order.insert(0, id);
        }
    }

    /// Switch to workspace.
    pub fn switch_workspace(&mut self, ws: u8) {
        if ws == self.active_workspace { return; }
        let popups: Vec<WindowId> = self.windows.iter()
            .filter(|w| w.popup_of.is_some()).map(|w| w.id).collect();
        for id in popups { self.close_popup(id, true); }
        // The keys go up to 9; only the workspaces the panel actually shows
        // exist, and switching past them would leave it with nothing to mark.
        if ws >= self.workspace_count { return; }
        // Leaving a workspace drops any fullscreen on it — a fullscreen
        // window is a here-and-now mode, not a stored layout. The retile
        // below sizes the freed window and tells its guest.
        let _ = self.clear_fullscreen_except(None);
        self.active_workspace = ws;

        // The panel is global: it follows the active workspace and stays
        // visible.
        if let Some(bw) = self.bottom_strut.map(|s| s.id) {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == bw) {
                win.workspace = ws;
                win.visible = true;
                win.dirty = true;
            }
            self.z_order.retain(|&wid| wid != bw);
            self.z_order.insert(0, bw);
        }

        self.focused = self.z_order.iter()
            .find(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == ws && !w.is_panel))
            .copied();

        if let Some(fid) = self.focused {
            self.set_focused_flag(fid);
            // The new workspace serves a different terminal buffer — carry
            // ACTIVE_IDX and the cursor over, or every keystroke after the
            // switch lands in the window you just left behind.
            self.sync_active_terminal(fid);
        }

        self.retile();
        self.pin_overlays_to_front();
        self.needs_full_redraw = true;
    }

    /// Move the focused window to a different workspace: unhook it from this
    /// workspace's tree, hang it into the target's, and go along with it
    /// (Hyprland's `movetoworkspace`, not the `silent` variant).
    pub fn move_to_workspace(&mut self, ws: u8) {
        let Some(fid) = self.focused else { return };
        if ws == self.active_workspace || ws >= self.workspace_count { return }
        // Panels are global chrome — they belong to every workspace at once.
        if self.is_panel(fid) { return }

        // Unhook here first: the children stay behind and have to close the
        // gap, exactly as if the window had been closed.
        self.reparent_children(fid);

        // Hang it into the target's tree — splitting the topmost tile there,
        // or becoming the root when that workspace is empty. Without this it
        // arrives as a second root, is chained beside the existing one and
        // never takes part in the layout properly. Its old delta stays with
        // the line it drew here, so the arriving window starts centred.
        let host = self.z_order.iter().copied()
            .find(|&wid| self.windows.iter().any(|w| w.id == wid
                && w.workspace == ws
                && w.state == WindowState::Tiled
                && w.visible
                && !w.is_overlay));
        let beside = host
            .and_then(|h| self.windows.iter().find(|w| w.id == h))
            .map(|w| w.width >= w.height)
            .unwrap_or(true);
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == fid) {
            win.workspace = ws;
            win.dirty = true;
            win.split_from = host;
            win.split_beside = beside;
            win.resize_w = 0;
            win.resize_h = 0;
        }

        // Go with it. Anything else means the window you just sent away keeps
        // the focus on a workspace nobody is looking at — so if we stay, focus
        // has to be handed back here (`refocus_active_workspace`, what
        // close_window does). Following is the Hyprland default and keeps the
        // window you are working on under your hands. switch_workspace does the
        // retile, the panels and the terminal hand-over; the tree link above
        // has to be set before it, or it would lay the window out as a stray
        // second root.
        self.switch_workspace(ws);
        self.focus_window(fid);
        self.needs_full_redraw = true;
    }

    /// Dwindle tiling: every window splits the one that was focused when it
    /// opened (`Window::split_from`), so the tree is stored on the windows
    /// themselves and needs no separate structure.
    pub fn retile(&mut self) {
        // A fullscreen window covers the whole screen, over the panel and
        // every tile. Placed before the dwindle below and before the
        // empty-workspace early return, so a lone fullscreen window (no tiles
        // behind it) is still sized and its guest still told the full size.
        let (sw, sh) = (self.screen_w, self.screen_h);
        for w in self.windows.iter_mut() {
            if w.workspace == self.active_workspace
                && w.state == WindowState::Fullscreen
                && w.visible
            {
                w.x = 0;
                w.y = 0;
                w.width = sw;
                w.height = sh;
                w.dirty = true;
            }
        }

        let (area_x, area_y, area_w, area_h) = self.workspace_area();

        let tiled: Vec<WindowId> = self.windows.iter()
            .filter(|w| w.workspace == self.active_workspace
                     && w.state == WindowState::Tiled
                     && w.visible
                     && !w.is_overlay)
            .map(|w| w.id)
            .collect();

        if !tiled.is_empty() {
            // Roots: a tiled window whose parent is gone or lives on another
            // workspace. Normally exactly one; a window moved between
            // workspaces can leave a second, and chaining them keeps the
            // screen usable instead of dropping tiles off it.
            let roots: Vec<WindowId> = tiled.iter().copied()
                .filter(|id| {
                    let parent = self.windows.iter().find(|w| w.id == *id).and_then(|w| w.split_from);
                    match parent {
                        None => true,
                        Some(p) => !tiled.contains(&p),
                    }
                })
                .collect();
            let roots = if roots.is_empty() { alloc::vec![tiled[0]] } else { roots };

            let gap = self.gaps;
            self.dwindle_chain(&roots, &tiled, area_x, area_y, area_w, area_h, gap);
        }

        self.sync_surface_tile_sizes();
    }

    /// Lay a window out in `rect` together with everything that split it.
    ///
    /// Children are walked in creation order, each taking half of what is left:
    /// splitting A, then splitting A again, halves A twice — which is exactly
    /// what happens interactively when the focus stays put.
    fn dwindle(&mut self, id: WindowId, tiled: &[WindowId],
               x: u32, y: u32, w: u32, h: u32, gap: u32) {
        let kids: Vec<WindowId> = self.windows.iter()
            .filter(|c| c.split_from == Some(id) && tiled.contains(&c.id))
            .map(|c| c.id)
            .collect();

        // Children only ever eat into the right/bottom, so the origin stays put.
        let (cx, cy, mut cw, mut ch) = (x, y, w, h);
        for kid in kids {
            // The split's position is the child's resize delta: it is the one
            // that arrived and drew the line, so Mod+arrow on it moves the
            // line it created.
            let (beside, dw, dh) = self.windows.iter().find(|c| c.id == kid)
                .map(|c| (c.split_beside, c.resize_w, c.resize_h))
                .unwrap_or((true, 0, 0));
            if beside {
                let half = cw.saturating_sub(gap) / 2;
                let max = cw.saturating_sub(gap).saturating_sub(100).max(100) as i32;
                let keep = (half as i32 - dw).clamp(100, max) as u32;
                self.store_delta(kid, true, half as i32 - keep as i32, dw);
                let give = cw.saturating_sub(keep + gap);
                self.dwindle(kid, tiled, cx + keep + gap, cy, give, ch, gap);
                cw = keep;
            } else {
                let half = ch.saturating_sub(gap) / 2;
                let max = ch.saturating_sub(gap).saturating_sub(80).max(80) as i32;
                let keep = (half as i32 - dh).clamp(80, max) as u32;
                self.store_delta(kid, false, half as i32 - keep as i32, dh);
                let give = ch.saturating_sub(keep + gap);
                self.dwindle(kid, tiled, cx, cy + keep + gap, cw, give, gap);
                ch = keep;
            }
        }

        if let Some(win) = self.windows.iter_mut().find(|win| win.id == id) {
            win.x = cx;
            win.y = cy;
            win.width = cw;
            win.height = ch;
            win.dirty = true;
        }
    }

    /// Write back the offset a split line actually took after clamping, so a
    /// key held against the minimum tile size doesn't pile up an invisible
    /// debt that has to be pressed off again before the line moves back.
    fn store_delta(&mut self, id: WindowId, beside: bool, effective: i32, current: i32) {
        if effective == current { return }
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            if beside { win.resize_w = effective } else { win.resize_h = effective }
        }
    }

    /// Several roots share the area the way siblings would — only reachable
    /// when a window changed workspace and left its parent behind.
    #[allow(clippy::too_many_arguments)]
    fn dwindle_chain(&mut self, roots: &[WindowId], tiled: &[WindowId],
                     x: u32, y: u32, w: u32, h: u32, gap: u32) {
        let (mut cx, mut cw) = (x, w);
        for (i, root) in roots.iter().enumerate() {
            let left = roots.len() - i;
            if left == 1 {
                self.dwindle(*root, tiled, cx, y, cw, h, gap);
            } else {
                let share = cw.saturating_sub(gap * (left as u32 - 1)) / left as u32;
                self.dwindle(*root, tiled, cx, y, share.max(1), h, gap);
                cx += share + gap;
                cw = cw.saturating_sub(share + gap);
            }
        }
    }

    /// Tiles are moving through sizes nobody keeps: a split line is being
    /// dragged.
    fn geometry_in_motion(&self) -> bool {
        self.drag.is_some_and(|d| matches!(d.mode, DragMode::Resize | DragMode::Edge(_)))
    }

    /// Push every Surface window's content rect into the surface
    /// registry: the guest renders at the tile size, no host scaling.
    /// Called at the end of every retile; `set_tile_size` is idempotent
    /// and only flags a change on a real size change. While tiles are in
    /// motion (`geometry_in_motion`) nothing is pushed: a guest redraws its
    /// whole window for every size, so it gets only the one the motion
    /// ends at, and until then its tile clips or extends its picture. The
    /// `border` must match the content rect `build_scene` uses exactly or
    /// the guest would render a few px off.
    fn sync_surface_tile_sizes(&self) {
        if self.geometry_in_motion() {
            return;
        }
        for win in &self.windows {
            if win.kind == crate::shade::window::WindowKind::Surface {
                let border = self.effective_border(win);
                crate::shade::surface::set_tile_size(
                    win.id.0,
                    win.content_w(border),
                    win.content_h(border),
                );
            }
        }
    }

    /// The whole frame, composed into `target`.
    pub fn render(&mut self, target: *mut u8, info: &FbInfo) {
        self.compose(target, info, alloc::vec![Rect::new(0, 0, info.width, info.height)]);
        self.extra_damage.clear();
        for win in &mut self.windows {
            win.dirty = false;
        }
        self.aurora_drawn = true;
        self.needs_full_redraw = false;
    }

    /// Compose the frame into `target` within `damage`, grown where glass
    /// needs what lies beneath it (`scene::expand_damage`). Returns the
    /// damage painted. Window buffers are brought up to date while the
    /// scene is built; dirty flags are cleared by the callers that repaint
    /// those windows (`render`, `render_damaged`).
    pub fn compose(&mut self, target: *mut u8, info: &FbInfo, mut damage: Vec<Rect>) -> Vec<Rect> {
        self.refresh_chrome();
        self.tell_focus();
        let frame = self.build_scene();
        scene::expand_damage(&frame, &mut damage, Rect::new(0, 0, info.width, info.height));
        crate::shade::gpu_scene::compose(&frame, &*self, target, info, &damage);
        damage
    }

    /// Text rect of a terminal window: its content rect less the padding.
    /// A fullscreen terminal has no bar: there are no controls to hold.
    pub(super) fn term_text_rect(&self, win: &Window) -> Rect {
        let pad = term_pad();
        let b = self.effective_border(win);
        let bar = if win.state == WindowState::Fullscreen { 0 } else { controls::loop_bar_h() };
        let ch = win.content_h(b);
        let bar = bar.min(ch);
        Rect::new(win.content_x(b) + pad, win.content_y(b) + bar + pad,
            win.content_w(b).saturating_sub(pad * 2),
            ch.saturating_sub(bar + pad * 2))
    }

    /// Bring a terminal window's text layer up to date. While windows are
    /// in motion a layer of the wrong size is kept (and shown cropped)
    /// rather than redrawn for every intermediate size.
    fn update_term_layer(&mut self, id: u32, terminal_idx: u8, w: u32, h: u32, dirty: bool, in_motion: bool) {
        let stale = self.term_layers.get(&id).is_none_or(|b| b.width() != w || b.height() != h);
        if stale && in_motion && self.term_layers.contains_key(&id) { return; }
        if stale {
            self.term_layers.insert(id, PixelBuffer::new(w, h));
        } else if !dirty && !self.needs_full_redraw {
            return;
        }
        let Some(layer) = self.term_layers.get_mut(&id) else { return };
        layer.fill(0);
        let info = layer.info();
        terminal::render_to_window(layer.ptr(), &info, 0, 0, w, h, terminal_idx);
    }

    /// The scene of the current frame, back to front. Window buffers that
    /// are out of date (terminal text, widget layout) are updated first.
    fn build_scene(&mut self) -> Scene {
        use crate::shade::window::WindowKind;
        struct Plan {
            id: WindowId,
            win_rect: Rect,
            kind: WindowKind,
            panel: bool,
            focused: bool,
            dirty: bool,
            terminal_idx: u8,
            glow: Glow,
            border_color: u32,
            bg: u32,
            content: Rect,
            text: Rect,
            controls: Vec<Item>,
            fullscreen: bool,
        }
        self.controls.begin_frame();
        let border = self.border;
        let rounding = self.rounding;
        let border_pinned = self.border_active.is_some();
        let opacity = crate::shade::glass::params().loop_opacity;
        let in_motion = self.geometry_in_motion() || self.animation.is_some();

        // While a window is fullscreen the panel and every other tile are
        // hidden; only the fullscreen window, its popups and any modal draw.
        let fs = self.active_fullscreen();

        let mut plans = Vec::new();
        for &wid in self.z_order.iter().rev() {
            let Some(win) = self.windows.iter().find(|w| w.id == wid) else { continue };
            if win.workspace != self.active_workspace || !win.visible { continue; }
            if fs.is_some()
                && Some(win.id) != fs
                && win.popup_of.is_none()
                && !win.modal
            {
                continue;
            }
            let panel = win.is_panel || win.popup_of.is_some();
            let fullscreen = win.state == WindowState::Fullscreen;
            let cb = if panel || fullscreen { 0 } else { border };
            plans.push(Plan {
                id: win.id,
                win_rect: Rect::new(win.x, win.y, win.width, win.height),
                kind: win.kind,
                panel,
                fullscreen,
                focused: win.focused,
                dirty: win.dirty,
                terminal_idx: win.terminal_idx,
                glow: self.glow_for(win),
                border_color: if win.focused { self.active_border_color() } else { self.inactive_border_color() },
                bg: win.bg_color,
                content: Rect::new(win.content_x(cb), win.content_y(cb), win.content_w(cb), win.content_h(cb)),
                text: self.term_text_rect(win),
                controls: {
                    let mut v = Vec::new();
                    self.controls_items(win, &mut v);
                    v
                },
            });
        }
        let live: Vec<u32> = plans.iter().filter(|p| p.kind == WindowKind::Terminal).map(|p| p.id.0).collect();
        self.term_layers.retain(|id, _| live.contains(id));

        let mut items = Vec::with_capacity(4 + plans.len() * 6);
        items.push(Item::Background);
        for p in &plans {
            if p.kind == WindowKind::Surface && !crate::shade::surface::has_frame(p.id.0) { continue; }
            if p.glow.alpha > 0 {
                items.push(Item::Glow { rect: p.win_rect, radius: rounding, color: p.glow.color,
                    band: p.glow.band, alpha: p.glow.alpha });
            }
            // Inner shape concentric with the outer at `rounding - border`,
            // so content and the chrome border meet along the same curve.
            // A fullscreen window owns the whole screen with square corners.
            let inner_r = if p.fullscreen { 0 }
                else if p.panel { rounding }
                else { rounding.saturating_sub(border) };
            // Terminal (`loop`) windows are glass with the theme's Surface
            // colour, fullscreen included; widget windows fill themselves.
            let paint_content = p.kind == WindowKind::Terminal;
            if p.fullscreen && paint_content {
                // No border and square corners, but the same glass.
                let bg = crate::shade::widgets::palette::resolve_glass(
                    crate::shade::widgets::abi::Token::Surface) & 0x00FF_FFFF;
                items.push(Item::GlassBackdrop { rect: p.win_rect, radius: 0 });
                items.push(Item::Chrome { rect: p.win_rect, border_a: bg, border_b: bg, bg, rounding: 0,
                    border: 0, border_opacity: 0, bg_opacity: opacity, paint_content: true });
            }
            if !p.panel && !p.fullscreen {
                let (ba, bb, b_op) = if p.focused {
                    if crate::theme::is_active() && !border_pinned {
                        let (ga, gb) = crate::theme::border_gradient();
                        (ga, gb, BORDER_ACTIVE_OPACITY)
                    } else {
                        (p.border_color, p.border_color, BORDER_ACTIVE_OPACITY)
                    }
                } else {
                    (p.border_color, p.border_color, BORDER_INACTIVE_OPACITY)
                };
                let bg = if paint_content {
                    crate::shade::widgets::palette::resolve_glass(
                        crate::shade::widgets::abi::Token::Surface) & 0x00FF_FFFF
                } else {
                    p.bg
                };
                if paint_content {
                    // Glass blends over the blurred wallpaper, not the sharp one.
                    items.push(Item::GlassBackdrop { rect: p.win_rect, radius: rounding });
                }
                items.push(Item::Chrome { rect: p.win_rect, border_a: ba, border_b: bb, bg, rounding, border,
                    border_opacity: b_op, bg_opacity: opacity, paint_content });
            }
            match p.kind {
                WindowKind::Terminal => {
                    if !p.fullscreen {
                        self.loop_bar_items(p.content, p.terminal_idx, &mut items);
                    }
                    self.update_term_layer(p.id.0, p.terminal_idx, p.text.w, p.text.h, p.dirty, in_motion);
                    items.push(Item::Text { src: Source::TermText(p.id.0), dst: p.text });
                    if let Some(bar) = term_scrollbar(p.terminal_idx, p.text) {
                        items.push(bar);
                    }
                }
                WindowKind::Widget => {
                    let c = p.content;
                    // A pure move only updates the origin; a new size lays the
                    // scene out again — except mid-motion, where the old
                    // pixels stand in until the motion ends: cropped or
                    // extended at the bottom and right while a split is
                    // dragged (the content stays where it is),
                    // stretched while windows swap tiles.
                    // Panels do not tile and always lay out at their size.
                    let (lw, lh) = if in_motion && !p.panel {
                        crate::shade::widgets::with_scene(p.id.0, |sc| (sc.width, sc.height)).unwrap_or((c.w, c.h))
                    } else {
                        (c.w, c.h)
                    };
                    let _ = crate::shade::widgets::relayout_scene(p.id.0, c.x as i32, c.y as i32, lw, lh);
                    if p.panel {
                        let (sw, sh) = crate::shade::widgets::with_scene(p.id.0,
                            |sc| (sc.pixels.width(), sc.pixels.height())).unwrap_or((0, 0));
                        items.push(Item::Panel { src: Source::Widget(p.id.0), dst: Rect::new(c.x, c.y, sw, sh) });
                    } else {
                        let fit = if self.animation.is_some() { Fit::Scale } else { Fit::Extend };
                        items.push(Item::Content { src: Source::Widget(p.id.0), dst: c, radius: inner_r, fit });
                    }
                }
                WindowKind::Surface => {
                    items.push(Item::Content { src: Source::Surface(p.id.0), dst: p.content, radius: inner_r,
                        fit: Fit::Extend });
                }
            }
            items.extend_from_slice(&p.controls);
        }
        self.controls_overlay(&mut items);
        self.controls.end_frame();
        // Last of all, over everything: the screenshot flash.
        let flash = self.flash_alpha();
        if flash > 0 {
            items.push(Item::Fill { rect: Rect::new(0, 0, self.screen_w, self.screen_h), color: 0,
                alpha: flash.min(255), radius: 0 });
        }
        Scene { items }
    }

    /// Draw the focused terminal's input line into its text layer; the
    /// screen rect to compose, if any.
    pub fn update_input_line(&mut self) -> Option<Rect> {
        let fid = self.focused?;
        let win = self.windows.iter().find(|w| w.id == fid && w.workspace == self.active_workspace)?;
        if win.kind != crate::shade::window::WindowKind::Terminal { return None; }
        let text = self.term_text_rect(win);
        let layer = self.term_layers.get(&fid.0)?;
        if layer.width() != text.w || layer.height() != text.h { return None; }
        let info = layer.info();
        let (x, y, w, h) = terminal::render_input_line(layer.ptr(), &info, 0, 0, text.w, text.h,
            win.terminal_idx)?;
        Some(Rect::new(text.x + x, text.y + y, w, h))
    }

    /// Compose the dirty windows (and their halo bands) into `target`.
    /// Returns the rects painted.
    pub fn render_damaged(&mut self, target: *mut u8, info: &FbInfo) -> Vec<Rect> {
        if self.needs_full_redraw {
            self.render(target, info);
            return alloc::vec![Rect::new(0, 0, self.screen_w, self.screen_h)];
        }
        let screen = Rect::new(0, 0, info.width, info.height);
        let extra = core::mem::take(&mut self.extra_damage);
        let mut damage: Vec<Rect> = self.windows.iter()
            .filter(|w| w.dirty && w.workspace == self.active_workspace && w.visible)
            .map(|w| {
                // The halo band too: the tile that just lost focus loses its
                // halo there, the one that gained it gets it.
                let g = self.glow_for(w).band;
                Rect::new(w.x, w.y, w.width, w.height).grow(g).intersect(&screen)
            })
            .collect();
        damage.extend(extra.iter().map(|r| r.intersect(&screen)).filter(|r| !r.is_empty()));
        if damage.is_empty() { return damage; }
        let painted = self.compose(target, info, damage);
        for win in &mut self.windows {
            win.dirty = false;
        }
        painted
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
        let mut gained = false;
        for win in &mut self.windows {
            let was = win.focused;
            win.focused = win.id == focused_id;
            if win.focused != was {
                win.dirty = true; // Only re-render windows that changed
                if win.focused { gained = true; }
            }
        }
        // The keyboard went to a window that is not a popup: the user looks
        // elsewhere, and every popup is dismissed (`xdg_popup.popup_done`).
        let to_popup = self.windows.iter().any(|w| w.id == focused_id && w.popup_of.is_some());
        if !to_popup {
            let popups: Vec<WindowId> = self.windows.iter()
                .filter(|w| w.popup_of.is_some()).map(|w| w.id).collect();
            if !popups.is_empty() {
                self.popup_prev_focus = None;
                for p in popups { self.drop_popup(p, true); }
            }
        }
        // Flash only on a real change of focus — every path routes through
        // here, and several of them re-assert the focus that is already set.
        if gained {
            self.focus_glow = Some((focused_id, crate::interrupts::ticks()));
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
            if win.id == fid || win.workspace != self.active_workspace || !win.visible || win.is_panel { continue; }

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
            .filter(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == self.active_workspace && w.visible && !w.is_panel))
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
            .filter(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == self.active_workspace && w.visible && !w.is_panel))
            .copied()
            .collect();

        if ws_windows.len() < 2 { return; }

        let current_idx = self.focused
            .and_then(|fid| ws_windows.iter().position(|&wid| wid == fid))
            .unwrap_or(0);
        let prev_idx = if current_idx == 0 { ws_windows.len() - 1 } else { current_idx - 1 };
        self.focus_window(ws_windows[prev_idx]);
    }

    /// Handle only button events (click, drag, release). Position already in self.mouse.
    /// Called from lock-free input path — only when buttons change.
    pub fn handle_mouse_buttons(&mut self) -> bool {
        if self.drag.is_none()
            && (self.mouse.left_clicked() || self.mouse.right_clicked())
            && self.modal_blocks(self.mouse.x, self.mouse.y)
        {
            return false;
        }
        let mx = self.mouse.x;
        let my = self.mouse.y;
        let mod_held = crate::keyboard::is_super_held();

        if self.drag.is_none() && (self.mouse.left_clicked() || self.mouse.right_clicked())
            && self.popup_dismiss_at(mx, my)
        {
            return true;
        }

        // A press on a floating window's bar turns into a move once the
        // pointer has travelled; a release before that was only a click.
        if let Some((wid, sx, sy)) = self.pending_move {
            if !self.mouse.left_held() {
                self.pending_move = None;
            } else if (mx - sx).abs() + (my - sy).abs() > controls::drag_threshold() {
                self.pending_move = None;
                self.start_float_drag(wid, DragMode::Move, sx, sy);
            }
        }

        // Active drag
        if let Some(mut drag) = self.drag {
            let held = match drag.mode {
                DragMode::Swap => self.mouse.left_held(),
                DragMode::Resize => self.mouse.right_held(),
                DragMode::Move | DragMode::Edge(_) => self.mouse.left_held() || self.mouse.right_held(),
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
                        self.apply_resize_drag(&drag, dx, dy);
                        self.drag = Some(drag); // Keep drag alive
                        self.retile();
                        self.needs_full_redraw = true;
                        return true;
                    }
                    DragMode::Move | DragMode::Edge(_) => {
                        self.apply_float_drag(&drag, mx, my);
                        self.drag = Some(drag);
                        self.needs_full_redraw = true;
                        return true;
                    }
                }
            } else {
                self.drag = None;
                self.sync_surface_tile_sizes();
                if drag.mode != DragMode::Swap { self.needs_full_redraw = true; }
                return drag.mode != DragMode::Swap;
            }
        }

        // Light-dismiss: a window that opted in (npk_window_set_light_dismiss)
        // closes on a fresh click outside it — the volume slider overlay etc.
        // The dismiss click is consumed (it just closes the overlay).
        if self.mouse.left_clicked() || self.mouse.right_clicked() {
            if let Some(id) = self.light_dismiss_outside(mx, my) {
                self.close_window(id);
                return true;
            }
        }

        // Mod+LMB: move a floating window, swap a tile.
        if mod_held && self.mouse.left_clicked() {
            if let Some(wid) = self.window_at(mx, my).filter(|id| self.is_floating_window(*id)) {
                self.start_float_drag(wid, DragMode::Move, mx, my);
                return true;
            }
            if let Some(wid) = self.window_at(mx, my) {
                self.drag = Some(DragState {
                    window: wid, mode: DragMode::Swap,
                    last_target: None,
                    start_mx: 0, start_my: 0, start_rw: 0, start_rh: 0,
                    w_target: None, h_target: None, start_geom: (0, 0, 0, 0),
                });
                self.focus_window(wid);
                return true;
            }
        }

        // Mod+RMB: resize a floating window from its nearest corner, else
        // push the split lines.
        if mod_held && self.mouse.right_clicked() {
            if let Some(wid) = self.window_at(mx, my).filter(|id| self.is_floating_window(*id)) {
                let edges = self.nearest_corner(wid, mx, my);
                self.start_float_drag(wid, DragMode::Edge(edges), mx, my);
                return true;
            }
            if let Some(wid) = self.window_at(mx, my) {
                self.begin_resize_drag(wid, mx, my);
                return true;
            }
        }

        // Plain LMB on the window controls or their menu. Checked before
        // focus/dispatch so the pill always wins over whatever widget sits
        // beneath it; Mod+LMB is the swap-drag above.
        if !mod_held && self.mouse.left_clicked() && self.controls_click(mx, my) {
            return true;
        }

        // Plain LMB on the edge of a floating window: resize it.
        if !mod_held && self.mouse.left_clicked() {
            if let Some((wid, edges)) = self.floating_edge_at(mx, my) {
                self.start_float_drag(wid, DragMode::Edge(edges), mx, my);
                return true;
            }
            // On its free bar: a move, once the pointer travels.
            if let Some(wid) = self.window_at(mx, my).filter(|id| self.bar_grab_at(*id, mx, my)) {
                self.pending_move = Some((wid, mx, my));
            }
        }

        // Regular LMB click: focus window + dispatch widget event
        if self.mouse.left_clicked() {
            if let Some(wid) = self.window_at(mx, my) {
                // The panel must not steal keyboard focus from the tile you
                // were working in; clicking an icon launches/focuses an
                // app, the panel itself stays unfocused. Hit-test/events
                // below still fire.
                let is_panel_win = self.windows.iter()
                    .any(|w| w.id == wid && w.is_panel);
                // Focus on first click into an unfocused window; later
                // clicks on the same focused widget should dispatch.
                let focus_changed = !is_panel_win && self.focused != Some(wid);
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

        // Plain RMB (no Mod): right-click for context menus. Hit-test the
        // widget tree and push ContextAction(id) + raw MouseButton{Right}.
        // Mod+RMB is the resize-drag above; this branch is only reached
        // when `!mod_held`. The panel must not steal keyboard focus (same
        // exclusion as LMB), so we skip the focus shift on it.
        if !mod_held && self.mouse.right_clicked() {
            if let Some(wid) = self.window_at(mx, my) {
                let is_widget = self.windows.iter()
                    .find(|w| w.id == wid)
                    .map(|w| w.kind == crate::shade::window::WindowKind::Widget)
                    .unwrap_or(false);
                if is_widget {
                    use crate::shade::widgets::abi::{Event, MouseButton};
                    crate::shade::widgets::set_context_point(wid.0, mx, my);
                    if let Some(action) = crate::shade::widgets::hit_test(wid.0, mx, my) {
                        crate::shade::widgets::push_event(wid.0, Event::ContextAction(action));
                    }
                    crate::shade::widgets::push_event(wid.0, Event::MouseButton {
                        button: MouseButton::Right,
                        down:   true,
                        x:      mx,
                        y:      my,
                    });
                }
            }
        }

        // Middle click: raw button only, no hit-test.
        //
        // There is deliberately no `Action` for it. A middle click is not
        // a second way to press a button but a gesture on the content (in
        // the browser: open the link under the pointer in a new tab).
        // Routing it through the hit-test would make every button in the
        // system react to the middle button without any app asking for it.
        if !mod_held && self.mouse.middle_clicked() {
            if let Some(wid) = self.window_at(mx, my) {
                let is_widget = self.windows.iter()
                    .find(|w| w.id == wid)
                    .map(|w| w.kind == crate::shade::window::WindowKind::Widget)
                    .unwrap_or(false);
                if is_widget {
                    use crate::shade::widgets::abi::{Event, MouseButton};
                    crate::shade::widgets::push_event(wid.0, Event::MouseButton {
                        button: MouseButton::Middle,
                        down:   true,
                        x:      mx,
                        y:      my,
                    });
                }
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

        // Same for the middle button: an app that pairs press and release
        // must not wait for a release that never comes.
        if !mod_held && self.mouse.middle_released() {
            use crate::shade::widgets::abi::{Event, MouseButton};
            let widget_ids: alloc::vec::Vec<crate::shade::WindowId> = self.windows.iter()
                .filter(|w| w.kind == crate::shade::window::WindowKind::Widget)
                .map(|w| w.id)
                .collect();
            for wid in widget_ids {
                crate::shade::widgets::push_event(wid.0, Event::MouseButton {
                    button: MouseButton::Middle,
                    down:   false,
                    x:      mx,
                    y:      my,
                });
            }
        }

        // Mirror RMB release to widget windows so apps see press+release pairs.
        if !mod_held && self.mouse.right_released() {
            use crate::shade::widgets::abi::{Event, MouseButton};
            let widget_ids: alloc::vec::Vec<crate::shade::WindowId> = self.windows.iter()
                .filter(|w| w.kind == crate::shade::window::WindowKind::Widget)
                .map(|w| w.id)
                .collect();
            for wid in widget_ids {
                crate::shade::widgets::push_event(wid.0, Event::MouseButton {
                    button: MouseButton::Right,
                    down:   false,
                    x:      mx,
                    y:      my,
                });
            }
        }

        false
    }

    /// Resize the focused window by pushing the split line that bounds it.
    /// A positive delta grows it, no matter which of its edges is the movable
    /// one (`resize_target` picks the line and the sign).
    pub fn resize_focused(&mut self, dx: i32, dy: i32) {
        let Some(fid) = self.focused else { return };
        let mut moved = false;
        for (delta, beside) in [(dx, true), (dy, false)] {
            if delta == 0 { continue }
            let Some(tid) = self.resize_target(fid, beside) else { continue };
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == tid) {
                // `- delta`, exactly like the mouse drag: the arrow moves the
                // split line the way it points. "Make the focused window
                // wider" would move the left edge of a right-hand tile, so
                // pressing Right would grow it leftwards. Keyboard and mouse
                // push the same line the same way.
                if beside { win.resize_w -= delta } else { win.resize_h -= delta }
                moved = true;
            }
        }
        if moved {
            self.retile();
            self.needs_full_redraw = true;
        }
    }

    /// Mod+J — flip the split the focused window sits on from side-by-side to
    /// stacked and back (Hyprland's `togglesplit`). It flips the same line a
    /// resize would push, so what you toggle is what you were just adjusting.
    /// The offset is dropped: the delta is per-axis, and the axis just changed.
    pub fn toggle_split(&mut self) {
        let Some(fid) = self.focused else { return };
        let Some(owner) = self.split_line_owner(fid) else { return };
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == owner) {
            win.split_beside = !win.split_beside;
            win.resize_w = 0;
            win.resize_h = 0;
        }
        self.retile();
        self.needs_full_redraw = true;
    }

    /// Resolve the split lines a Mod+RMB drag will move and remember where
    /// they stood, so the drag stays absolute against its start position.
    /// Only which line comes from `resize_target` — the sign doesn't, see
    /// `apply_resize_drag`.
    fn begin_resize_drag(&mut self, wid: WindowId, mx: i32, my: i32) {
        let w_target = self.resize_target(wid, true);
        let h_target = self.resize_target(wid, false);
        let delta_of = |s: &Self, t: Option<WindowId>, beside: bool| -> i32 {
            t.and_then(|id| s.windows.iter().find(|w| w.id == id))
                .map(|w| if beside { w.resize_w } else { w.resize_h })
                .unwrap_or(0)
        };
        self.drag = Some(DragState {
            window: wid, mode: DragMode::Resize,
            last_target: None,
            start_mx: mx, start_my: my,
            start_rw: delta_of(self, w_target, true),
            start_rh: delta_of(self, h_target, false),
            w_target, h_target, start_geom: (0, 0, 0, 0),
        });
        self.focus_window(wid);
    }

    /// Apply a resize drag: absolute against the deltas captured at drag start.
    ///
    /// Direct manipulation: the border follows the hand — mouse right = line
    /// right, whichever side of it the focused tile is on. A delta always sits
    /// on the child that drew the line and always moves it towards its parent's
    /// origin (left / up), so following the cursor is a fixed `- delta` on both
    /// axes. `resize_focused` (Mod+Ctrl+arrow) does the same thing, so a key
    /// and a drag move the line identically.
    fn apply_resize_drag(&mut self, drag: &DragState, dx: i32, dy: i32) {
        if let Some(id) = drag.w_target {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
                win.resize_w = drag.start_rw - dx;
            }
        }
        if let Some(id) = drag.h_target {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
                win.resize_h = drag.start_rh - dy;
            }
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
            if win.id == fid || win.workspace != self.active_workspace || !win.visible || win.is_panel { continue; }
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

        // Swap their places in the tree, not their places in the list — the
        // list order no longer decides geometry. Each takes the other's
        // parent, and each other's children come along, so the two tiles
        // exchange regions with everything nested inside them.
        // The split offset travels with the slot too — it describes where a
        // line stands, not how big a particular window wants to be.
        let slot = |ws: &Vec<crate::shade::window::Window>, id: WindowId| {
            ws.iter().find(|w| w.id == id).map(|w| (w.split_from, w.split_beside, w.resize_w, w.resize_h))
        };
        if let (Some(a_slot), Some(b_slot)) = (slot(&self.windows, a), slot(&self.windows, b)) {
            for w in &mut self.windows {
                if w.id == a {
                    (w.split_from, w.split_beside, w.resize_w, w.resize_h) = b_slot;
                } else if w.id == b {
                    (w.split_from, w.split_beside, w.resize_w, w.resize_h) = a_slot;
                } else if w.split_from == Some(a) {
                    w.split_from = Some(b);
                } else if w.split_from == Some(b) {
                    w.split_from = Some(a);
                }
            }
            // A child of the other is now its own parent — undo that one case.
            if a_slot.0 == Some(b) {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == b) { w.split_from = Some(a); }
            } else if b_slot.0 == Some(a) {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == a) { w.split_from = Some(b); }
            }
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

    // ── Screen flash ──────────────────────────────────────────────────
    //
    // A shutter closing, not a floodlight: the screen dips dark for a
    // moment instead of being washed white. Same mechanism either way —
    // a full-screen blend — but dimming reads as a soft blink while white
    // is genuinely harsh on a dark desktop. Kept to a few frames because
    // the overlay touches every pixel, the expensive operation at 4K.

    /// Ticks (100 Hz) the flash lasts.
    const FLASH_TICKS: u64 = 12;
    /// How far down the screen dips at the peak, 0..255.
    /// `set shade.flash_opacity <0-255>` overrides it; 0 turns it off.
    const FLASH_ALPHA: u32 = 110;

    fn flash_peak() -> u32 {
        crate::config::get("shade.flash_opacity")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .map(|v| v.min(255))
            .unwrap_or(Self::FLASH_ALPHA)
    }

    /// Start (or restart) the shutter blink. Turned off entirely at
    /// opacity 0, so the capture costs no extra frames at all.
    pub fn start_flash(&mut self) {
        if Self::flash_peak() == 0 { return; }
        self.flash = Some(crate::interrupts::ticks());
        self.needs_full_redraw = true;
    }

    /// Advance the flash. True while it still needs frames — including the
    /// final one that clears it, otherwise the last dimmed frame would stay
    /// on screen until something else happened to repaint.
    pub fn flash_tick(&mut self) -> bool {
        let start = match self.flash { Some(s) => s, None => return false };
        let elapsed = crate::interrupts::ticks().saturating_sub(start);
        self.needs_full_redraw = true;
        if elapsed >= Self::FLASH_TICKS {
            self.flash = None;
        }
        true
    }

    /// Current flash opacity, 0 when inactive. Ease-out so it snaps bright
    /// and drains away.
    fn flash_alpha(&self) -> u32 {
        let start = match self.flash { Some(s) => s, None => return 0 };
        let elapsed = crate::interrupts::ticks().saturating_sub(start);
        if elapsed >= Self::FLASH_TICKS { return 0; }
        let remaining = Self::FLASH_TICKS - elapsed;      // 1..=FLASH_TICKS
        // Quadratic falloff: brightest on the first frame, then a quick
        // drain — a blink, not a fade.
        let a = Self::flash_peak() as u64 * remaining * remaining
            / (Self::FLASH_TICKS * Self::FLASH_TICKS);
        a as u32
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

    /// The modal window on this workspace, if any — a dialog that owns the
    /// pointer until it's answered.
    fn modal_window(&self) -> Option<WindowId> {
        self.windows.iter()
            .find(|w| w.modal && w.workspace == self.active_workspace && w.visible)
            .map(|w| w.id)
    }

    /// True if (x, y) lies outside an open modal dialog.
    ///
    /// Clicking there would focus and raise the window behind, burying the
    /// dialog with no way back, so a modal dialog blocks the pointer
    /// instead: nothing behind it reacts. Deliberately not light-dismiss,
    /// which is right for a casual overlay (the volume slider) but would
    /// throw away a half-typed filename on a stray click.
    fn modal_blocks(&self, x: i32, y: i32) -> bool {
        match self.modal_window() {
            None => false,
            Some(id) => match self.windows.iter().find(|w| w.id == id) {
                Some(win) => {
                    let inside = x >= win.x as i32 && x < (win.x + win.width) as i32
                              && y >= win.y as i32 && y < (win.y + win.height) as i32;
                    !inside
                }
                None => false,
            },
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

    /// Geometry for a scrollbar drag at `(x, y)`: the window under the
    /// cursor plus its terminal text rect (for terminals) and the controls'
    /// corner (to exclude from the bar). Widget windows fill the viewport from
    /// their scene; here we just identify the window + kind. `None` for
    /// panels or empty space.
    pub fn scroll_hit_at(&self, x: i32, y: i32) -> Option<ScrollHit> {
        let wid = self.window_at(x, y)?;
        let win = self.windows.iter().find(|w| w.id == wid)?;
        if win.is_panel { return None; }
        let t = self.term_text_rect(win);
        let text_rect = (t.x as i32, t.y as i32, t.w, t.h);
        Some(ScrollHit {
            window: wid.0,
            is_terminal: win.kind == crate::shade::window::WindowKind::Terminal,
            term_idx: win.terminal_idx,
            text_rect,
            char_h: terminal::cells().h,
            close_rect: self.controls_corner(win).map(|r| (r.x, r.y, r.w, r.h)),
        })
    }
}

/// Parse a hex color string ("RRGGBB") to u32.
fn parse_hex_color(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches('#');
    if s.len() != 6 { return None; }
    u32::from_str_radix(s, 16).ok()
}


/// Gaps between tiles, window border and corner radius in physical px:
/// `shade.gaps` / `shade.border` / `shade.rounding` (logical px) times the
/// UI scale.
fn chrome_lengths() -> (u32, u32, u32) {
    let get = |key: &str, default: u32| crate::config::get(key)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(default);
    use crate::shade::widgets::scale::len;
    (len(get("shade.gaps", 8)), len(get("shade.border", 1)), len(get("shade.rounding", 10)))
}

/// Gap between a strut panel and the screen edge, physical px.
fn bar_margin() -> u32 {
    let m = crate::config::get("shade.bar_margin")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(6);
    crate::shade::widgets::scale::len(m)
}

/// Inset of a terminal's text from its content edge, physical px.
fn term_pad() -> u32 { crate::shade::widgets::scale::len(6) }

/// The overlay scrollbar of a terminal, when its scrollback overflows.
/// Geometry must match `scroll_hit_at` so the thumb and the drag area agree.
fn term_scrollbar(terminal_idx: u8, text: Rect) -> Option<Item> {
    use crate::shade::widgets::scale;
    let (total, soff) = terminal::scroll_metrics(terminal_idx as usize)?;
    let char_h = terminal::cells().h;
    let rows = (text.h / char_h.max(1)).max(1) as usize;
    if total <= rows || text.h == 0 { return None; }
    let track_h = text.h as u64;
    let thumb_h = ((track_h * rows as u64) / total as u64).max(scale::len(24) as u64).min(track_h) as u32;
    let travel = track_h - thumb_h as u64;
    let max_scroll = (total - rows) as u64;
    let soff = (soff as u64).min(max_scroll);
    // soff 0 = bottom → thumb at bottom; soff max = top.
    let from_top = if max_scroll == 0 { 0 } else { travel * (max_scroll - soff) / max_scroll };
    let color = crate::shade::widgets::palette::resolve_glass(
        crate::shade::widgets::abi::Token::OnSurfaceMuted) & 0x00FF_FFFF;
    // Alpha 200 of 256, as the thumb has always been blended.
    Some(Item::Fill { rect: Rect::new(text.x + text.w.saturating_sub(scale::len(6)), text.y + from_top as u32,
        scale::len(4), thumb_h),
        color, alpha: 200 * 255 / 256, radius: 0 })
}

impl scene::Sources for Compositor {
    fn with_source(&self, src: Source, f: &mut dyn FnMut(&[u32], u32, u32)) {
        match src {
            Source::Widget(id) => {
                crate::shade::widgets::with_scene(id, |sc| f(&sc.pixels, sc.pixels.width(), sc.pixels.height()));
            }
            Source::Surface(id) => {
                crate::shade::surface::with_front(id, |px, w, h| f(px, w, h));
            }
            Source::TermText(id) => {
                if let Some(b) = self.term_layers.get(&id) {
                    f(b, b.width(), b.height());
                }
            }
            Source::Label(id) => {
                if let Some(b) = self.label_pixels(id) {
                    f(b, b.width(), b.height());
                }
            }
        }
    }

    fn with_source_keyed(&self, src: Source, f: &mut dyn FnMut(&[u32], u32, u32, u64)) {
        match src {
            Source::Surface(id) => {
                crate::shade::surface::with_front_keyed(id, |px, w, h, key| f(px, w, h, key));
            }
            _ => self.with_source(src, &mut |px, w, h| f(px, w, h, px.as_ptr() as u64)),
        }
    }

    fn in_place(&self, src: Source) -> Option<scene::InPlace> {
        match src {
            Source::Surface(id) => crate::shade::surface::front_in_place(id),
            _ => None,
        }
    }
}
