//! Compositor — manages windows, Z-order, tiling layout, and rendering.
//!
//! No per-window pixel buffers. Windows are metadata (position, size, state).
//! The compositor renders directly to the framebuffer shadow buffer.
//! Uses dwindle layout (Hyprland-style recursive binary split).

use alloc::vec::Vec;
use crate::framebuffer::FbInfo;
use crate::gui::{background, render};

use super::window::{Window, WindowId, WindowState};
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
const DOCK_BOTTOM_GAP: u32 = 12;

/// How much more see-through the terminal is in light mode (0..256).
/// Dark looks perfect at the default `shade.opacity` (~160); a light
/// Surface at that blend reads as solid and hides the wallpaper, so in
/// light mode we drop the content opacity by this much to let the
/// background shine through. Dark mode is untouched.
const LIGHT_TERMINAL_OPACITY_DROP: u32 = 56;

/// Platform close button — the compositor draws a small "X" affordance in
/// the top-right corner of every real (non-panel) window so mouse users
/// can close it without remembering Mod+Q. Not per-app: provided here
/// once for all windows. Edge of the (square) hit box at 1× scale.
const CLOSE_BTN_BOX: u32 = 22;
/// Assumed height of the app's top menu/toolbar band (loft/spell:
/// `Datei Bearbeiten …`) at 1× — the X is vertically centred in this
/// band so it lines up with the menu labels instead of clinging to the
/// very top edge. Windows without a menu bar (browser, terminal) just
/// get the X centred in their top ~band; close enough.
const CLOSE_BTN_BAND: u32 = 40;
/// White close X glyph size at 1× (no disc → a touch larger than the
/// old 16 so the bare glyph reads as a button).
const CLOSE_BTN_GLYPH: u32 = 20;

/// The X tracks the window's CONTENT scale, not the raw screen scale.
/// Widget apps (loft/spell/drun) render their UI at a fixed pixel size — the
/// widget rasterizer runs at `RasterTarget.scale = 1` (apps size via density,
/// not a HiDPI factor), so the content does NOT grow with screen resolution.
/// A screen-scaled X therefore balloons against fixed-px content on 4K (the
/// "only the X is too big" report). Terminals DO scale their text by the
/// screen factor, so they keep it.
fn close_btn_scale(win: &Window, scale: u32) -> u32 {
    if win.kind == crate::shade::window::WindowKind::Terminal { scale.max(1) } else { 1 }
}

/// Screen rect `(x, y, w, h)` of a window's platform close button, or
/// `None` for panels (dock/bar are managed chrome — never closable here)
/// and windows too narrow to host the button. Shared by the renderer and
/// the click hit-test so the drawn disc and the clickable area always
/// coincide.
fn close_button_rect(win: &Window, border: u32, scale: u32)
    -> Option<(u32, u32, u32, u32)>
{
    // No X on managed chrome (dock/bar), on transient overlays (drun and
    // other launchers — dismissed with Esc, not closed), or on Surface
    // windows (the microvm browser owns its own close via its UI).
    if win.is_dock || win.is_bar || win.is_overlay { return None; }
    if win.kind == crate::shade::window::WindowKind::Surface { return None; }
    let scale = close_btn_scale(win, scale);
    let box_px = CLOSE_BTN_BOX * scale;
    // The box is centred vertically in the app's menu-bar band so the X
    // aligns with `Datei / Bearbeiten / …` rather than the top edge. Reuse
    // that same vertical inset as the right-edge margin → equal gap on the
    // top, bottom and right sides (Florian: "rundum gleicher Abstand").
    let band = CLOSE_BTN_BAND * scale;
    let inset = band.saturating_sub(box_px) / 2;
    if win.width <= border * 2 + inset + box_px { return None; }
    let bx = win.x + win.width - border - inset - box_px;
    let by = win.y + border + inset;
    Some((bx, by, box_px, box_px))
}

/// Paint the platform close button — a bare X — into the shadow buffer,
/// vertically centred in the window's top menu-bar band. Drawn last in
// ── Terminal chrome cache ──────────────────────────────────────────────
// The translucent "glass" terminal background (bg_color blended over the
// wallpaper, per pixel) is the dominant compositor cost (~71ms for a
// maximised 4K terminal) and it's STATIC — only geometry / theme / focus /
// wallpaper change it, not the text drawn on top. Cache the rendered chrome
// region (wallpaper+border+glass) and memcpy it back each frame instead of
// re-blending. One entry (the common case is one focused terminal); a second
// terminal just thrashes it (still correct, recomputes on miss).
struct ChromeCache { key: u64, w: u32, h: u32, px: Vec<u32> }
static CHROME_CACHE: spin::Mutex<Option<ChromeCache>> = spin::Mutex::new(None);

#[allow(clippy::too_many_arguments)]
fn chrome_key(x: u32, y: u32, w: u32, h: u32, focused: bool,
              ba: u32, bb: u32, b_op: u32, content_bg: u32, content_opacity: u32,
              rounding: u32, border: u32) -> u64 {
    let mut k = 0xcbf29ce484222325u64;
    for v in [x, y, w, h, focused as u32, ba, bb, b_op, content_bg, content_opacity, rounding, border] {
        k ^= v as u64;
        k = k.wrapping_mul(0x0000_0100_0000_01b3);
    }
    k
}

/// On a cache hit, memcpy the cached chrome region into the back buffer and
/// return true. The text is drawn over it afterwards (every frame), so only
/// the static glass background is cached.
fn chrome_cache_blit(key: u64, x: u32, y: u32, w: u32, h: u32, shadow: *mut u8, info: &FbInfo) -> bool {
    let cache = CHROME_CACHE.lock();
    let Some(c) = cache.as_ref() else { return false };
    if c.key != key || c.w != w || c.h != h { return false; }
    let pitch = info.pitch as usize;
    let rows = h.min(info.height.saturating_sub(y));
    let span = w.min(info.width.saturating_sub(x)) as usize;
    for row in 0..rows {
        let dst_off = (y + row) as usize * pitch + x as usize * 4;
        let src_off = row as usize * w as usize;
        // SAFETY: dst within fb (clamped), src within px (w*h, row<h).
        unsafe {
            core::ptr::copy_nonoverlapping(
                c.px.as_ptr().add(src_off),
                shadow.add(dst_off) as *mut u32,
                span,
            );
        }
    }
    true
}

/// Capture the just-rendered chrome region from the back buffer into the cache.
fn chrome_cache_store(key: u64, x: u32, y: u32, w: u32, h: u32, shadow: *const u8, info: &FbInfo) {
    let mut px = alloc::vec![0u32; (w as usize) * (h as usize)];
    let pitch = info.pitch as usize;
    let rows = h.min(info.height.saturating_sub(y));
    let span = w.min(info.width.saturating_sub(x)) as usize;
    for row in 0..rows {
        let src_off = (y + row) as usize * pitch + x as usize * 4;
        let dst_off = row as usize * w as usize;
        // SAFETY: src within fb (clamped), dst within px (w*h, row<h).
        unsafe {
            core::ptr::copy_nonoverlapping(
                shadow.add(src_off) as *const u32,
                px.as_mut_ptr().add(dst_off),
                span,
            );
        }
    }
    *CHROME_CACHE.lock() = Some(ChromeCache { key, w, h, px });
}

/// `render_window` so it sits over the window content. No disc / colour
/// highlight (Florian's call): just the glyph, a touch larger so it reads
/// as a button. The glyph takes the theme's `OnSurface` colour so it stays
/// visible in light mode (dark X) as well as dark mode (light X).
fn draw_close_button(shadow: *mut u8, info: &FbInfo, win: &Window,
                     border: u32, scale: u32) {
    let Some((bx, by, bw, bh)) = close_button_rect(win, border, scale) else { return };
    use crate::shade::widgets::abi::{IconId, Token};
    let color = crate::shade::widgets::palette::resolve(Token::OnSurface) & 0x00FF_FFFF;
    let req = CLOSE_BTN_GLYPH * close_btn_scale(win, scale);
    if let Some((asz, glyph)) = crate::gui::icons::alpha_for(IconId::X, req as u16) {
        let asz = asz as u32;
        if asz > 0 && glyph.len() >= (asz * asz) as usize {
            let ox = bx + bw.saturating_sub(asz) / 2;
            let oy = by + bh.saturating_sub(asz) / 2;
            for row in 0..asz {
                for col in 0..asz {
                    let a = glyph[(row * asz + col) as usize] as u32;
                    if a > 0 {
                        render::blend_pixel(shadow, info, ox + col, oy + row,
                            color, a + (a >> 7));
                    }
                }
            }
        }
    }
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

/// Drag mode: swap windows or resize split.
#[derive(Clone, Copy, PartialEq)]
pub enum DragMode { Swap, Resize }

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
    /// The window's close-button rect, to exclude from the bar strip.
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

/// Top strut panel registered by a bar app (`bar.wasm`) via
/// `npk_window_set_panel(Top, Strut)`. The app reports its height; the kernel
/// reserves a `margin + pill_h` band at the top and lays tiles below it. The
/// app owns ALL rendering — there is no native fallback, so an unregistered or
/// closed bar simply frees the band (tiles reclaim the full height).
#[derive(Clone, Copy)]
pub struct TopStrut {
    pub id: WindowId,
    /// Visible panel height in px (reported by the app via set_panel `h`).
    pub pill_h: u32,
    /// Gap to the screen edge / tiles (px).
    pub margin: u32,
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
    /// Number of workspaces (was tracked by the old native bar).
    pub workspace_count: u8,
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
    /// Top strut bar, if a bar app (`bar.wasm`) has registered one. `None`
    /// reserves no top band (tiles use the full height). The app renders
    /// itself — there is no native fallback.
    pub top_strut: Option<TopStrut>,
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
            workspace_count: 4,
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
            top_strut: None,
        }
    }

    /// Height reserved at the top for the bar strut (0 if no bar registered).
    fn top_band(&self) -> u32 {
        self.top_strut.map(|s| s.margin + s.pill_h).unwrap_or(0)
    }

    /// Usable workspace area (excluding the top bar band + bottom dock).
    fn workspace_area(&self) -> (u32, u32, u32, u32) {
        let top = self.top_band();
        let x = self.gaps;
        let y = top + self.gaps;
        let w = self.screen_w.saturating_sub(self.gaps * 2);
        let h = self.screen_h.saturating_sub(top)
            .saturating_sub(self.gaps * 2)
            .saturating_sub(self.dock_bottom_reserve());
        (x, y, w, h)
    }

    /// Vertical band the tiling area gives up at the bottom for the
    /// auto-hide dock. Zero when the dock is fully hidden (so tiles reach
    /// their normal extent), ramping to the full dock band + a top gap
    /// matching the dock's floating bottom gap when fully revealed.
    /// Linear in the reveal fraction so a retile per `dock_tick` makes the
    /// tiles glide up/down in lockstep with the sliding dock.
    fn dock_bottom_reserve(&self) -> u32 {
        let Some(dock) = self.dock else { return 0 };
        let slide = dock.thickness + dock.gap;
        if slide == 0 { return 0 }
        // Fully-shown reservation: the dock band plus a top gap equal to its
        // floating bottom gap (→ equal air above and below), minus the tile
        // gap the area already leaves above the baseline.
        let full = (dock.thickness + 2 * dock.gap).saturating_sub(self.gaps);
        // offset: 0 = shown, `slide` = hidden. risen = how far it has slid up.
        let risen = slide.saturating_sub(dock.offset.min(slide));
        (full as u64 * risen as u64 / slide as u64) as u32
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
        // process (registered by the spawn path), so the loop PID is a
        // misleading orphan that otherwise leaks on every drun/dock
        // launch (close_window only frees a pid in its Terminal arm,
        // which a promoted Widget window never reaches). Exiting a pid
        // only touches the PROCS map. We deliberately do NOT free the
        // session or terminal buffer here: the terminal's intent loop is
        // still live and holds a long-lived `&mut IntentSession`, so
        // freeing them mid-flight is a use-after-free (panicked in
        // sync_session_to_terminal). Their lifecycle stays tied to the
        // window via close_window. (Session/terminal-slot leak for
        // promoted widgets is pre-existing — a separate follow-up.)
        if pid != 0 { crate::process::exit(pid); }
        // Re-tile: the promoted window kept whatever geometry it had as a
        // terminal (often fullscreen, if it was the only window when its
        // terminal was created). Without this, launching a second app from
        // the dock left the first app fullscreen and the second stacked
        // behind it instead of splitting — promote is the only window-
        // producing path that wasn't re-tiling. Overlay/panel apps (drun,
        // dock, bar) call set_overlay/set_panel right after, which un-tiles
        // them again, so this is a no-op for them.
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

    /// Toggle the `modal` flag on a window.
    pub fn set_modal(&mut self, id: WindowId, modal: bool) -> bool {
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.modal = modal;
            true
        } else {
            false
        }
    }

    /// Position an overlay window's top-left at `(x, y)` with size `(w, h)`,
    /// clamped to the screen. Like `set_overlay` but app-positioned instead
    /// of centred — for dropdowns that anchor to a screen corner (e.g. the
    /// volume slider just under the bar). Does not touch focus.
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

    /// A visible light-dismiss window NOT containing `(x, y)`, if any — the
    /// click-handler closes it so transient overlays vanish on an outside
    /// click. (Clicks inside keep it open to interact with.)
    fn light_dismiss_outside(&self, x: i32, y: i32) -> Option<WindowId> {
        self.windows.iter().find(|w| {
            w.light_dismiss && w.visible && !(
                x >= w.x as i32 && x < (w.x + w.width) as i32 &&
                y >= w.y as i32 && y < (w.y + w.height) as i32)
        }).map(|w| w.id)
    }

    /// Bottom edge of the dock's resting baseline. The bar is a top strut, so
    /// the dock always rests at the screen bottom.
    fn dock_baseline(&self) -> u32 {
        self.screen_h
    }

    /// `npk_window_set_panel` host fn — configure `id` as an edge panel.
    /// `edge`: 0=Bottom, 1=Top. `behavior`: 0=AutoHide overlay (dock),
    /// 1=Strut (bar). Bottom+AutoHide is the dock (slide + handle);
    /// Top+Strut (the bar) is wired in the bar-render step. Returns false
    /// for not-yet-implemented combos.
    pub fn set_panel(&mut self, id: WindowId, edge: u8, behavior: u8, w: u32, h: u32) -> bool {
        match (edge, behavior) {
            (0, 0) => self.set_dock_panel(id, w, h),
            (1, 1) => self.set_bar_panel(id, w, h),
            _ => false,
        }
    }

    /// Top strut bar. The app reports its height via `h`; the compositor
    /// reserves a `margin + h` band at the top and lays tiles below it.
    /// Full width minus the margin, always visible, global across workspaces,
    /// never focused, rendered with the panel blit. `w` is advisory.
    fn set_bar_panel(&mut self, id: WindowId, _w: u32, h: u32) -> bool {
        let screen_w = self.screen_w;
        let margin = crate::config::get("shade.bar_margin")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(6);
        let pill_h = h.max(22); // floor so a too-small report still fits content
        let active_ws = self.active_workspace;

        let found = if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.state = crate::shade::window::WindowState::Floating;
            win.is_overlay = true;
            win.is_bar = true;
            win.modal = false;
            win.workspace = active_ws;
            win.x = margin;
            win.y = margin;
            win.width = screen_w.saturating_sub(margin * 2).max(120);
            win.height = pill_h;
            win.visible = true;
            win.dirty = true;
            true
        } else {
            false
        };

        if found {
            self.top_strut = Some(TopStrut { id, pill_h, margin });
            // If the spawn path focused it, drop focus — panels never hold it.
            if self.focused == Some(id) { self.focused = None; }
            self.retile();
            // Pin the bar to the front of the z-order; keep the dock pinned too.
            self.z_order.retain(|&wid| wid != id);
            self.z_order.insert(0, id);
            self.pin_dock_to_front();
            self.needs_full_redraw = true;
        }
        found
    }

    /// Back-compat wrapper for the `npk_window_set_dock` host fn.
    pub fn set_dock(&mut self, id: WindowId, w: u32, h: u32) -> bool {
        self.set_panel(id, 0, 0, w, h)
    }

    /// Bottom auto-hide dock. Overlay (no strut, excluded from retile),
    /// never modal, never focused on reveal, global across workspaces.
    /// Starts fully hidden.
    fn set_dock_panel(&mut self, id: WindowId, w: u32, h: u32) -> bool {
        let screen_w = self.screen_w;
        let screen_h = self.screen_h;
        let active_ws = self.active_workspace;

        let dw = w.min(screen_w.saturating_sub(40)).max(120);
        // Cap at full screen height: a popover-bearing dock expands so the
        // menu has room above the tray and click-outside lands inside the
        // dock window. The visible tray still floats at the bottom.
        let dh = h.min(screen_h).max(40);

        // Detect a resize on an already-set-up dock. Preserve the dock state
        // (visible / offset / target_shown) so reopening the popover doesn't
        // re-hide the tray; only the geometry is updated.
        let already_dock = self.dock.map(|d| d.id == id).unwrap_or(false);
        let same_size = self.windows.iter()
            .find(|w| w.id == id)
            .map(|w| w.width == dw && w.height == dh)
            .unwrap_or(false);
        if already_dock && same_size {
            return true;
        }

        let found = if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.state = crate::shade::window::WindowState::Floating;
            win.is_overlay = true;
            win.is_dock = true;
            win.modal = false;
            win.workspace = active_ws;
            win.width = dw;
            win.height = dh;
            win.x = screen_w.saturating_sub(dw) / 2;
            win.dirty = true;
            true
        } else {
            false
        };

        if found {
            let gap = DOCK_BOTTOM_GAP * self.scale.max(1);
            let baseline = self.dock_baseline() as i32;
            // On an in-place resize, keep the existing visibility so a
            // popover-open expand doesn't flicker the dock through a hide
            // cycle. Brand-new docks start hidden as before.
            let (target_shown, offset, visible) = if already_dock {
                let d = self.dock.unwrap();
                let preserved_visible = self.windows.iter()
                    .find(|w| w.id == id)
                    .map(|w| w.visible)
                    .unwrap_or(true);
                (d.target_shown, d.offset.min(dh + gap), preserved_visible)
            } else {
                (false, dh + gap, false)
            };
            // Position the window with the visible tray near the baseline
            // and the popover space stretching upward inside the window.
            // Mirrors `dock_tick`: y = baseline - (thickness + gap) + offset
            // so the floating gap above the screen edge is preserved.
            let slide = dh as i32 + gap as i32;
            let win_y = (baseline - slide + offset as i32).max(0) as u32;
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
                win.y = win_y;
                win.visible = visible;
            }
            self.dock = Some(DockState {
                id,
                thickness: dh,
                gap,
                target_shown,
                offset,
                dwell: 0,
                debounce: 0,
            });
            // If the spawn path focused it, drop focus — panels never hold it.
            if self.focused == Some(id) { self.focused = None; }
            // Overlay → tiling grid is untouched, but retile reclaims any
            // slot this window held if it was previously tiled.
            self.retile();
            self.pin_dock_to_front();
            self.needs_full_redraw = true;
        }
        found
    }

    /// Live state the bar app renders: (workspace_count, active_workspace,
    /// focused window title). Fed to WASM via `npk_bar_state`.
    pub fn bar_info(&self) -> (u8, u8, alloc::string::String) {
        // Title of the focused real app window — never a panel/overlay
        // (the dock window is titled "dock"). If nothing real is focused
        // (panels clear focus), fall back to the topmost real window on the
        // active workspace so the bar still shows what's open.
        let is_app = |w: &&Window| !w.is_overlay && !w.is_dock && !w.is_bar;
        let title = self.focused
            .and_then(|fid| self.windows.iter().find(|w| w.id == fid))
            .filter(is_app)
            .or_else(|| {
                self.z_order.iter()
                    .filter_map(|wid| self.windows.iter().find(|w| w.id == *wid))
                    .find(|w| is_app(w) && w.workspace == self.active_workspace && w.visible)
            })
            .map(|w| w.title.clone())
            .unwrap_or_default();
        (self.workspace_count, self.active_workspace, title)
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
        // Reflow tiles to the dock-reserved area at the current offset so the
        // windows glide up/down together with the sliding dock (the reserve
        // is keyed off `dock.offset`, which we just stepped).
        self.retile();
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
    /// Is `id` a panel (dock or bar)? Panels are managed chrome — they
    /// never hold focus and must not be closed by Mod+Q.
    pub fn is_panel(&self, id: WindowId) -> bool {
        self.dock.map(|d| d.id) == Some(id) || self.top_strut.map(|s| s.id) == Some(id)
    }

    pub fn close_window(&mut self, id: WindowId) {
        // If the dock app's window goes away, forget the dock so the
        // reveal/tick machinery no-ops.
        if self.dock.map(|d| d.id) == Some(id) {
            self.dock = None;
        }
        // Bar window gone → free the top strut band; tiles reclaim the height.
        if self.top_strut.map(|s| s.id) == Some(id) {
            self.top_strut = None;
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
        self.windows.retain(|w| w.id != id);
        self.z_order.retain(|&wid| wid != id);

        if self.focused == Some(id) {
            // Reassign focus to the topmost REAL window in this workspace.
            // Panels (dock/bar) must never hold focus — otherwise closing
            // the last app window would focus a panel, and the next Mod+Q
            // would close the dock/bar. When only panels remain, focus is
            // None (empty desktop) and Mod+Q no-ops.
            self.focused = self.z_order.iter()
                .filter(|&&wid| !self.is_panel(wid))
                .find_map(|&top_id| {
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
        // Panels (dock / bar) are never focusable — focusing them would
        // route keyboard input into a window that ignores it (the shell
        // appears to hang). Refuse, no matter which path asked.
        if self.windows.iter().any(|w| w.id == id && (w.is_dock || w.is_bar)) {
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

        // The dock is global: follow the active workspace and snap shut so
        // it re-reveals on demand rather than popping up mid-slide.
        let baseline = self.dock_baseline();
        if let Some(dock) = self.dock {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == dock.id) {
                win.workspace = ws;
                win.visible = false;
                win.y = baseline;
            }
            if let Some(d) = self.dock.as_mut() {
                d.target_shown = false;
                d.offset = d.thickness;
                d.dwell = 0;
                d.debounce = 0;
            }
        }

        // The bar is global too: follow the active workspace, stay visible.
        if let Some(bw) = self.top_strut.map(|s| s.id) {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == bw) {
                win.workspace = ws;
                win.visible = true;
                win.dirty = true;
            }
            self.z_order.retain(|&wid| wid != bw);
            self.z_order.insert(0, bw);
        }

        self.focused = self.z_order.iter()
            .find(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == ws && !w.is_dock && !w.is_bar))
            .copied();

        if let Some(fid) = self.focused {
            self.set_focused_flag(fid);
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
        let mut wc = [0u32; 4]; // terminal, widget, surface, other(panel)
        for &wid in self.z_order.iter().rev() {
            if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                if win.workspace != self.active_workspace || !win.visible { continue; }
                match win.kind {
                    crate::shade::window::WindowKind::Terminal => wc[0] += 1,
                    crate::shade::window::WindowKind::Widget if !win.is_dock && !win.is_bar => wc[1] += 1,
                    crate::shade::window::WindowKind::Surface => wc[2] += 1,
                    _ => wc[3] += 1,
                }

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
        {
            static COMP_N: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
            let n = COMP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
            if n % 60 == 0 {
                if crate::shade::PERF_LOG {
                crate::kprintln!("[comp] windows: {} terminal | {} widget | {} surface | {} panel/other",
                    wc[0], wc[1], wc[2], wc[3]);
                }
            }
        }

        // The bar (bar.wasm) draws itself in the window loop above — no
        // native bar render.

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
            tray & 0x00FF_FFFF, hh / 2,
            crate::shade::widgets::palette::chrome_opacity());
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
        // [rw-phase] per-window timing: wallpaper restore | chrome | content.
        static RW_BG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        static RW_CHROME: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        static RW_CONTENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        static RW_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let t_rw0 = crate::interrupts::rdtsc();
        // Overlay windows skip the wallpaper restore — rounded-out
        // corners keep showing whatever app is underneath instead of
        // punching a wallpaper-shaped hole into it. The bar is the
        // exception: it's a translucent overlay that repaints every clock
        // tick, so it MUST restore its band from the wallpaper first or
        // successive blends would stack into an opaque smear.
        if !win.is_overlay || win.is_bar {
            background::draw_background_region(shadow, info,
                win.x, win.y, win.width, win.height);
        }
        let t_rw_bg = crate::interrupts::rdtsc();

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
        let (chrome_border, chrome_round) = if win.is_dock || win.is_bar {
            (0u32, rounding)
        } else {
            (border, rounding.saturating_sub(border))
        };

        if !win.is_dock && !win.is_bar {
            let (ba, bb, b_op) = if crate::theme::is_active() && win.focused {
                let (ga, gb) = crate::theme::border_gradient();
                (ga, gb, 200u32)
            } else {
                (border_color, border_color, 180u32)
            };
            let paint_content = matches!(win.kind, crate::shade::window::WindowKind::Terminal);
            // Terminal (`loop`) windows track the active theme like the
            // widget apps — Surface bg + OnSurface text — so light mode is
            // consistent across every window instead of a lone dark tile.
            let content_bg = if paint_content {
                crate::shade::widgets::palette::resolve(
                    crate::shade::widgets::abi::Token::Surface) & 0x00FF_FFFF
            } else {
                win.bg_color
            };
            // Light mode reads as solid and hides the wallpaper at the
            // default opacity; drop it so the background shows through like
            // it does in dark mode (dark unchanged).
            let content_opacity = if paint_content
                && crate::shade::widgets::palette::is_light_theme()
            {
                opacity.saturating_sub(LIGHT_TERMINAL_OPACITY_DROP)
            } else {
                opacity
            };
            if paint_content {
                // Terminal glass bg is static + expensive — cache it.
                let key = chrome_key(win.x, win.y, win.width, win.height, win.focused,
                    ba, bb, b_op, content_bg, content_opacity, rounding, border);
                let hit = chrome_cache_blit(key, win.x, win.y, win.width, win.height, shadow, info);
                {
                    static HITS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
                    static MISS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
                    use core::sync::atomic::Ordering::Relaxed;
                    if hit { HITS.fetch_add(1, Relaxed); } else { MISS.fetch_add(1, Relaxed); }
                    let m = MISS.load(Relaxed);
                    if crate::shade::PERF_LOG && (HITS.load(Relaxed) + m) % 30 == 0 {
                        crate::kprintln!("[chrome-cache] hits {} miss {} | geo {}x{}+{}+{} key {:#x}",
                            HITS.swap(0, Relaxed), MISS.swap(0, Relaxed),
                            win.width, win.height, win.x, win.y, key);
                    }
                }
                if !hit {
                    render::fill_rounded_chrome_aa(shadow, info,
                        win.x, win.y, win.width, win.height,
                        ba, bb, content_bg,
                        rounding, border, b_op, content_opacity, paint_content);
                    chrome_cache_store(key, win.x, win.y, win.width, win.height, shadow, info);
                }
            } else {
                render::fill_rounded_chrome_aa(shadow, info,
                    win.x, win.y, win.width, win.height,
                    ba, bb, content_bg,
                    rounding, border, b_op, content_opacity, paint_content);
            }
        }

        let t_rw_chrome = crate::interrupts::rdtsc();
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

                // Overlay scrollbar — only when the scrollback overflows.
                // Mirrors the widget overlay bar; geometry must match
                // `scroll_hit_at` so the drawn thumb and the drag area agree.
                if let Some((total, soff)) = terminal::scroll_metrics(win.terminal_idx as usize) {
                    let (_, char_h) = crate::gui::font::char_size(scale);
                    let rows = (text_h / char_h.max(1)).max(1) as usize;
                    if total > rows && text_h > 0 {
                        let track_h = text_h as u64;
                        let thumb_h = ((track_h * rows as u64) / total as u64).max(24).min(track_h) as u32;
                        let travel = track_h - thumb_h as u64;
                        let max_scroll = (total - rows) as u64;
                        let soff = (soff as u64).min(max_scroll);
                        // soff 0 = bottom → thumb at bottom; soff max = top.
                        let from_top = if max_scroll == 0 { 0 } else { travel * (max_scroll - soff) / max_scroll };
                        let ty = text_y + from_top as u32;
                        let tx = text_x + text_w.saturating_sub(6);
                        let color = crate::shade::widgets::palette::resolve(
                            crate::shade::widgets::abi::Token::OnSurfaceMuted) & 0x00FF_FFFF;
                        for py in ty..(ty + thumb_h).min(info.height) {
                            for px in tx..(tx + 4).min(info.width) {
                                render::blend_pixel(shadow, info, px, py, color, 200);
                            }
                        }
                    }
                }
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

                    // Panels (dock + bar). Both root their tree in a Stack, so
                    // their scene carries real alpha (transparent where empty,
                    // chrome-opacity backgrounds, full-coverage glyphs — see
                    // rasterize_buffer_with_overlays). Composite the scene over
                    // the wallpaper by per-pixel alpha: translucent tray/pills,
                    // crisp glyphs, AA corners from the rasteriser, wallpaper in
                    // the gaps — no halo, no per-pill detection.
                    if win.is_dock || win.is_bar {
                        for dy in cy..y1 {
                            let local_y = dy - cy;
                            for dx in cx..x1 {
                                let px = scene.pixels[(local_y as usize)
                                    * (scene.width as usize) + (dx - cx) as usize];
                                let a = (px >> 24) & 0xFF;
                                if a == 0 { continue; }  // transparent → wallpaper
                                render::blend_pixel(shadow, info, dx, dy,
                                    px & 0x00FF_FFFF, a);
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

        let t_rw_content = crate::interrupts::rdtsc();

        // Platform close button (top-right). Panels return None and are
        // skipped; everything else gets the mouse-friendly "X".
        draw_close_button(shadow, info, win, border, scale);

        use core::sync::atomic::Ordering::Relaxed;
        let mhz = (crate::interrupts::tsc_freq() / 1_000_000).max(1);
        if win.is_dock || win.is_bar {
            // Panels return early from the widget arm but still flow here.
            // Time them SEPARATELY — averaging them with the terminal made
            // the phase numbers ambiguous (panels carry their own cost).
            static T_PANEL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
            static N_PANEL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
            T_PANEL.fetch_add(t_rw_content.saturating_sub(t_rw0), Relaxed);
            let np = N_PANEL.fetch_add(1, Relaxed) + 1;
            if crate::shade::PERF_LOG && np % 60 == 0 {
                crate::kprintln!("[rw-panel] avg per-panel render: {}us (dock/bar)",
                    T_PANEL.swap(0, Relaxed) / 60 / mhz);
            }
        } else {
            RW_BG.fetch_add(t_rw_bg.saturating_sub(t_rw0), Relaxed);
            RW_CHROME.fetch_add(t_rw_chrome.saturating_sub(t_rw_bg), Relaxed);
            RW_CONTENT.fetch_add(t_rw_content.saturating_sub(t_rw_chrome), Relaxed);
            let n = RW_COUNT.fetch_add(1, Relaxed) + 1;
            if crate::shade::PERF_LOG && n % 60 == 0 {
                let bg = RW_BG.swap(0, Relaxed) / 60 / mhz;
                let ch = RW_CHROME.swap(0, Relaxed) / 60 / mhz;
                let ct = RW_CONTENT.swap(0, Relaxed) / 60 / mhz;
                crate::kprintln!("[rw-phase] terminal avg: wallpaper {}us | chrome {}us | content {}us", bg, ch, ct);
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
            if win.id == fid || win.workspace != self.active_workspace || !win.visible || win.is_dock || win.is_bar { continue; }

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
            .filter(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == self.active_workspace && w.visible && !w.is_dock && !w.is_bar))
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
            .filter(|&&wid| self.windows.iter().any(|w| w.id == wid && w.workspace == self.active_workspace && w.visible && !w.is_dock && !w.is_bar))
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

        // Plain LMB on a window's close button → close it (see the
        // button-event path for the rationale).
        if !mod_held && self.mouse.left_clicked() {
            if let Some(wid) = self.close_button_at(mx, my) {
                self.close_window(wid);
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

        // Light-dismiss: a window that opted in (npk_window_set_light_dismiss)
        // closes on a fresh click outside it — the volume slider overlay etc.
        // The dismiss click is consumed (it just closes the overlay).
        if self.mouse.left_clicked() || self.mouse.right_clicked() {
            if let Some(id) = self.light_dismiss_outside(mx, my) {
                self.close_window(id);
                return true;
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

        // Plain LMB on a window's close button → close it. Checked before
        // focus/dispatch so the corner "X" always wins over whatever widget
        // sits beneath it. Mod+LMB is the swap-drag above, so guard on
        // `!mod_held` to keep the two gestures from overlapping.
        if !mod_held && self.mouse.left_clicked() {
            if let Some(wid) = self.close_button_at(mx, my) {
                self.close_window(wid);
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
                    .any(|w| w.id == wid && (w.is_dock || w.is_bar));
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

        // Plain RMB (no Mod): right-click for context menus. Hit-test the
        // widget tree and push ContextAction(id) + raw MouseButton{Right}.
        // Mod+RMB is the resize-drag above; this branch is only reached
        // when `!mod_held`. The dock must not steal keyboard focus (same
        // exclusion as LMB), so we skip the focus shift on dock/bar.
        if !mod_held && self.mouse.right_clicked() {
            if let Some(wid) = self.window_at(mx, my) {
                let is_widget = self.windows.iter()
                    .find(|w| w.id == wid)
                    .map(|w| w.kind == crate::shade::window::WindowKind::Widget)
                    .unwrap_or(false);
                if is_widget {
                    use crate::shade::widgets::abi::{Event, MouseButton};
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
            if win.id == fid || win.workspace != self.active_workspace || !win.visible || win.is_dock || win.is_bar { continue; }
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

    /// Find the window whose platform close button contains `(x, y)`,
    /// front-to-back so the topmost button wins. Returns `None` if the
    /// point isn't over any close button.
    fn close_button_at(&self, x: i32, y: i32) -> Option<WindowId> {
        let border = self.border;
        let scale = self.scale;
        for &wid in &self.z_order {
            if let Some(win) = self.windows.iter().find(|w| w.id == wid
                && w.workspace == self.active_workspace && w.visible)
            {
                if let Some((bx, by, bw, bh)) = close_button_rect(win, border, scale) {
                    if x >= bx as i32 && x < (bx + bw) as i32
                        && y >= by as i32 && y < (by + bh) as i32
                    {
                        return Some(wid);
                    }
                }
            }
        }
        None
    }

    /// Geometry for a scrollbar drag at `(x, y)`: the window under the
    /// cursor plus its terminal text rect (for terminals) and close-button
    /// rect (to exclude from the bar). Widget windows fill the viewport from
    /// their scene; here we just identify the window + kind. `None` for
    /// panels or empty space.
    pub fn scroll_hit_at(&self, x: i32, y: i32) -> Option<ScrollHit> {
        let wid = self.window_at(x, y)?;
        let win = self.windows.iter().find(|w| w.id == wid)?;
        if win.is_dock || win.is_bar { return None; }
        let border = self.border;
        let pad = 6 * self.scale;
        let text_rect = (
            (win.content_x(border) + pad) as i32,
            (win.content_y(border) + pad) as i32,
            win.content_w(border).saturating_sub(pad * 2),
            win.content_h(border).saturating_sub(pad * 2),
        );
        Some(ScrollHit {
            window: wid.0,
            is_terminal: win.kind == crate::shade::window::WindowKind::Terminal,
            term_idx: win.terminal_idx,
            text_rect,
            char_h: crate::gui::font::char_size(self.scale).1,
            close_rect: close_button_rect(win, border, self.scale),
        })
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

        // The bar (bar.wasm) renders itself as a window into the chrome
        // layer — no native bar render.

        for win in &mut self.windows {
            win.dirty = false;
        }
        self.needs_full_redraw = false;
    }

    /// Render only dirty windows to layer buffers.
    pub fn render_damaged_to_layers(&mut self, info: &FbInfo) {
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

