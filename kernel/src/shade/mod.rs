//! Shade — nopeekOS Compositor
//!
//! Native Rust compositor layer. Manages windows, Z-order, damage tracking,
//! shadebar (status bar), terminal rendering, and keybindings.
//!
//! Architecture:
//!   Keyboard → Input (Super keybinds) → Compositor → Framebuffer
//!   kprintln → Terminal buffer → Window content rendering

pub mod window;
pub mod bar;
pub mod compositor;
pub mod terminal;
pub mod input;

use spin::Mutex;

use crate::framebuffer::{self};
use crate::gui::{font, render};

#[allow(unused_imports)]
pub use compositor::Compositor;
#[allow(unused_imports)]
pub use window::{WindowId, Window};
#[allow(unused_imports)]
pub use bar::ShadeBar;

/// Global compositor instance.
pub(crate) static COMPOSITOR: Mutex<Option<Compositor>> = Mutex::new(None);

/// Initialize shade compositor. Call after login + GPU setup.
pub fn init() {
    let (screen_w, screen_h) = framebuffer::with_fb(|fb| {
        let info = fb.info();
        (info.width, info.height)
    }).unwrap_or((1920, 1080));

    let scale = font::scale_for(screen_w);
    let comp = Compositor::new(screen_w, screen_h, scale);

    *COMPOSITOR.lock() = Some(comp);

    // Enable terminal capture + GUI mode (no window yet — Mod+Enter opens first loop)
    terminal::set_active(true);
    framebuffer::set_gui_mode(true);
}

/// Execute a closure with exclusive access to the compositor.
pub fn with_compositor<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Compositor) -> R,
{
    COMPOSITOR.lock().as_mut().map(f)
}

/// Create a new window. Returns the window ID.
pub fn create_window(title: &str, x: u32, y: u32, w: u32, h: u32) -> Option<WindowId> {
    with_compositor(|comp| comp.create_window(title, x, y, w, h))
}

/// Close and remove a window.
pub fn close_window(id: WindowId) {
    with_compositor(|comp| comp.close_window(id));
}

/// Set focus to a window.
#[allow(dead_code)]
pub fn focus_window(id: WindowId) {
    with_compositor(|comp| comp.focus_window(id));
}

/// Draw the entire compositor state to the framebuffer.
pub fn render_frame() {
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            comp.render(shadow, info);
        }

        // Full blit
        let mut damage = render::DamageTracker::new(info.width, info.height);
        damage.mark_all();
        damage.flush(fb);
    });
}

/// Render only damaged regions (efficient partial update).
#[allow(dead_code)]
pub fn render_damaged() {
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            let regions = comp.render_damaged(shadow, info);
            for (x, y, w, h) in regions {
                framebuffer::blit_rect(fb, x, y, w, h);
            }
        }
    });
}

/// Check if shade compositor is active.
pub fn is_active() -> bool {
    COMPOSITOR.lock().is_some()
}

/// Process a shade action (called from intent loop).
pub fn handle_action(action: input::ShadeAction) {
    use input::ShadeAction;
    match action {
        ShadeAction::NewWindow => {
            with_compositor(|comp| {
                comp.create_window("loop", 0, 0, 800, 600);
            });
            // Write prompt to the new terminal
            terminal::write_prompt();
            render_frame();
        }
        ShadeAction::CloseWindow => {
            with_compositor(|comp| {
                if let Some(id) = comp.focused {
                    comp.close_window(id);
                }
            });
            render_frame();
        }
        ShadeAction::Workspace(ws) => {
            with_compositor(|comp| {
                comp.switch_workspace(ws);
            });
            render_frame();
        }
        ShadeAction::MoveToWorkspace(ws) => {
            with_compositor(|comp| {
                comp.move_to_workspace(ws);
            });
            render_frame();
        }
        ShadeAction::FocusLeft => {
            with_compositor(|comp| comp.focus_direction(-1, 0));
            render_damaged();
        }
        ShadeAction::FocusRight => {
            with_compositor(|comp| comp.focus_direction(1, 0));
            render_damaged();
        }
        ShadeAction::FocusUp => {
            with_compositor(|comp| comp.focus_direction(0, -1));
            render_damaged();
        }
        ShadeAction::FocusDown => {
            with_compositor(|comp| comp.focus_direction(0, 1));
            render_damaged();
        }
        ShadeAction::ToggleFullscreen => {
            with_compositor(|comp| {
                if let Some(id) = comp.focused {
                    if let Some(win) = comp.window_mut(id) {
                        win.state = match win.state {
                            window::WindowState::Fullscreen => window::WindowState::Tiled,
                            _ => window::WindowState::Fullscreen,
                        };
                    }
                    comp.retile();
                }
            });
            render_frame();
        }
        ShadeAction::ToggleFloating => {
            with_compositor(|comp| {
                if let Some(id) = comp.focused {
                    if let Some(win) = comp.window_mut(id) {
                        win.state = match win.state {
                            window::WindowState::Floating => window::WindowState::Tiled,
                            _ => window::WindowState::Floating,
                        };
                    }
                    comp.retile();
                }
            });
            render_frame();
        }
        ShadeAction::ScrollUp => {
            terminal::scroll_up(10);
            // Mark focused window dirty for re-render
            with_compositor(|comp| {
                if let Some(fid) = comp.focused {
                    if let Some(win) = comp.window_mut(fid) { win.dirty = true; }
                }
            });
            render_damaged();
        }
        ShadeAction::ScrollDown => {
            terminal::scroll_down(10);
            with_compositor(|comp| {
                if let Some(fid) = comp.focused {
                    if let Some(win) = comp.window_mut(fid) { win.dirty = true; }
                }
            });
            render_damaged();
        }
        ShadeAction::Lock => {
            // Lock handled by intent loop
        }
    }
}

/// Fast re-render of just the current input line (for live typing feedback).
pub fn render_input_line() {
    terminal::clear_dirty(); // Prevent poll_render from doing full window render
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref comp) = *COMPOSITOR.lock() {
            if let Some((x, y, w, h)) = comp.render_input_line(shadow, info) {
                framebuffer::blit_rect(fb, x, y, w, h);
            }
        }
    });
}

/// Progressive render: if terminal has new output, re-render the focused window's text.
/// Fast path: only clears + redraws text (no aurora, no blend). Call from net::poll().
pub fn poll_render() {
    if !is_active() { return; }
    if !terminal::is_dirty() { return; }
    terminal::clear_dirty();

    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref comp) = *COMPOSITOR.lock() {
            if let Some(fid) = comp.focused {
                if let Some(win) = comp.windows.iter().find(|w| w.id == fid && w.workspace == comp.active_workspace) {
                    let border = comp.border;
                    let scale = comp.scale;
                    let pad = 6 * scale;
                    let cx = win.content_x(border) + pad;
                    let cy = win.content_y(border) + pad;
                    let cw = win.content_w(border).saturating_sub(pad * 2);
                    let ch = win.content_h(border).saturating_sub(pad * 2);

                    // Full window re-render (aurora+blend+text, preserves transparency)
                    let border_color = if win.focused { comp.border_active } else { comp.border_inactive };
                    compositor::Compositor::render_window(shadow, info, win,
                        comp.border, comp.rounding, comp.opacity, comp.scale, border_color);
                    framebuffer::blit_rect(fb, win.x, win.y, win.width, win.height);
                }
            }
        }
    });
}

/// Stop shade compositor.
pub fn stop() {
    terminal::set_active(false);
    framebuffer::set_gui_mode(false);
    *COMPOSITOR.lock() = None;
    framebuffer::clear();
}

/// Get shade config defaults.
pub fn default_config() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("shade.gaps", "8", "Gap between tiled windows (px at 1x)"),
        ("shade.border", "1", "Window border width (px at 1x)"),
        ("shade.border_active", "", "Active window border color (hex, default: accent)"),
        ("shade.border_inactive", "3a2555", "Inactive window border color (hex)"),
        ("shade.bar_height", "28", "Shadebar height (px at 1x)"),
        ("shade.bar_position", "top", "Shadebar position (top/bottom)"),
        ("shade.rounding", "10", "Window corner radius (px at 1x)"),
        ("shade.opacity", "160", "Window background opacity (0-256, lower=more transparent)"),
    ]
}
