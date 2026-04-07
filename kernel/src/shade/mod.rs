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

    // Enable terminal capture
    terminal::set_active(true);
    // Set GUI mode so kprintln doesn't draw directly to framebuffer
    framebuffer::set_gui_mode(true);

    crate::kprintln!("[npk] shade: compositor {}x{} scale:{}x", screen_w, screen_h, scale);
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
                comp.create_window("terminal", 0, 0, 800, 600);
            });
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
        ShadeAction::FocusNext => {
            with_compositor(|comp| comp.focus_next());
            render_frame();
        }
        ShadeAction::FocusPrev => {
            with_compositor(|comp| comp.focus_prev());
            render_frame();
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
        ShadeAction::Lock => {
            // Lock handled by intent loop
        }
    }
}

/// Fast re-render of just the current input line (for live typing feedback).
pub fn render_input_line() {
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
        ("shade.border", "2", "Window border width (px at 1x)"),
        ("shade.border_active", "", "Active window border color (hex, default: accent)"),
        ("shade.border_inactive", "3a2555", "Inactive window border color (hex)"),
        ("shade.bar_height", "28", "Shadebar height (px at 1x)"),
        ("shade.bar_position", "top", "Shadebar position (top/bottom)"),
        ("shade.rounding", "10", "Window corner radius (px at 1x)"),
        ("shade.opacity", "200", "Window background opacity (0-256)"),
    ]
}
