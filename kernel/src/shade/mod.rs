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
pub mod cursor;

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
    let (screen_w, screen_h, pitch) = framebuffer::with_fb(|fb| {
        let info = fb.info();
        (info.width, info.height, info.pitch)
    }).unwrap_or((1920, 1080, 1920 * 4));

    // Initialize layer compositor (3 layers: Background, Chrome, Text)
    crate::layers::init(screen_w, screen_h, pitch);

    // Render background into Layer 0
    if crate::layers::is_initialized() {
        if let Some((bg_buf, _w, _h, _p)) = crate::layers::buffer(crate::layers::LAYER_BG) {
            let info = framebuffer::get_info();
            crate::gui::background::draw_background(bg_buf, &info);
            crate::layers::mark_full_dirty(crate::layers::LAYER_BG);
        }
    }

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

/// Force a full redraw (e.g. after wallpaper change).
pub fn force_redraw() {
    // Re-render background into Layer 0 if layers are active
    if crate::layers::is_initialized() {
        if let Some((bg_buf, _w, _h, _p)) = crate::layers::buffer(crate::layers::LAYER_BG) {
            let info = framebuffer::get_info();
            crate::gui::background::draw_background(bg_buf, &info);
            crate::layers::mark_full_dirty(crate::layers::LAYER_BG);
        }
    }

    with_compositor(|comp| {
        comp.aurora_drawn = false;
        comp.needs_full_redraw = true;
    });
    render_frame();
}

/// Draw the entire compositor state to the framebuffer.
pub fn render_frame() {
    if crate::layers::is_initialized() {
        render_frame_layered();
    } else {
        render_frame_legacy();
    }
}

/// Layer-based render: write to layer buffers, then composite.
fn render_frame_layered() {
    let info = framebuffer::get_info();

    // Render chrome (borders, content rects) into Layer 1
    // Render text into Layer 2
    if let Some(ref mut comp) = *COMPOSITOR.lock() {
        comp.render_to_layers(&info);
    }

    // Composite all layers → shadow → MMIO
    framebuffer::with_fb(|fb| {
        let (shadow, _) = fb.shadow_ptr();
        let info = fb.info();
        let regions = crate::layers::composite(shadow, info.addr, info.pitch, info.width, info.height);

        // Cursor overlay on MMIO (after composite)
        if crate::xhci::mouse_available() {
            if let Some(ref mut comp) = *COMPOSITOR.lock() {
                if !regions.is_empty() {
                    comp.mouse.redraw_overlay(shadow, info);
                }
            }
        }
    });
}

/// Legacy render (fallback when layers not initialized).
fn render_frame_legacy() {
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            comp.render(shadow, info);

            let mut damage = render::DamageTracker::new(info.width, info.height);
            damage.mark_all();
            damage.flush(fb);

            if crate::xhci::mouse_available() {
                comp.mouse.redraw_overlay(shadow, info);
            }
        }
    });
}

/// Render only damaged regions (efficient partial update).
#[allow(dead_code)]
pub fn render_damaged() {
    if crate::layers::is_initialized() {
        render_damaged_layered();
    } else {
        render_damaged_legacy();
    }
}

fn render_damaged_layered() {
    let info = framebuffer::get_info();

    // Re-render only dirty windows to layers
    if let Some(ref mut comp) = *COMPOSITOR.lock() {
        comp.render_damaged_to_layers(&info);
    }

    // Composite dirty regions → shadow → MMIO
    framebuffer::with_fb(|fb| {
        let (shadow, _) = fb.shadow_ptr();
        let info = fb.info();
        let regions = crate::layers::composite(shadow, info.addr, info.pitch, info.width, info.height);

        if crate::xhci::mouse_available() {
            if let Some(ref mut comp) = *COMPOSITOR.lock() {
                if !regions.is_empty() {
                    comp.mouse.redraw_overlay(shadow, info);
                }
            }
        }
    });
}

fn render_damaged_legacy() {
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            let regions = comp.render_damaged(shadow, info);
            for (x, y, w, h) in regions {
                framebuffer::blit_rect(fb, x, y, w, h);
            }
            if crate::xhci::mouse_available() {
                comp.mouse.redraw_overlay(shadow, info);
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
            terminal::set_cursor_pos(terminal::current_line_len());
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
        ShadeAction::SwapLeft => {
            with_compositor(|comp| comp.swap_direction(-1, 0));
            render_frame();
        }
        ShadeAction::SwapRight => {
            with_compositor(|comp| comp.swap_direction(1, 0));
            render_frame();
        }
        ShadeAction::SwapUp => {
            with_compositor(|comp| comp.swap_direction(0, -1));
            render_frame();
        }
        ShadeAction::SwapDown => {
            with_compositor(|comp| comp.swap_direction(0, 1));
            render_frame();
        }
        ShadeAction::ResizeLeft => {
            with_compositor(|comp| comp.resize_focused(-40, 0));
            render_frame();
        }
        ShadeAction::ResizeRight => {
            with_compositor(|comp| comp.resize_focused(40, 0));
            render_frame();
        }
        ShadeAction::ResizeUp => {
            with_compositor(|comp| comp.resize_focused(0, -40));
            render_frame();
        }
        ShadeAction::ResizeDown => {
            with_compositor(|comp| comp.resize_focused(0, 40));
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
    terminal::clear_dirty();

    if crate::layers::is_initialized() {
        render_input_line_layered();
    } else {
        render_input_line_legacy();
    }
}

fn render_input_line_layered() {
    // Use full text re-render (same path as poll_render_layered).
    // Re-renders all text lines for the focused window into Layer 2.
    // Slightly less efficient than partial update, but guarantees correct compositing.
    let info = framebuffer::get_info();

    if let Some(ref comp) = *COMPOSITOR.lock() {
        if let Some(fid) = comp.focused {
            if let Some(win) = comp.windows.iter().find(|w| w.id == fid && w.workspace == comp.active_workspace) {
                comp.render_text_to_layer(&info, win);
            }
        }
    }

    // Composite dirty text region
    framebuffer::with_fb(|fb| {
        let (shadow, _) = fb.shadow_ptr();
        let info = fb.info();
        let regions = crate::layers::composite(shadow, info.addr, info.pitch, info.width, info.height);

        if crate::xhci::mouse_available() && !regions.is_empty() {
            if let Some(ref mut comp) = *COMPOSITOR.lock() {
                comp.mouse.redraw_overlay(shadow, info);
            }
        }
    });
}

fn render_input_line_legacy() {
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
/// Fast path: only redraws text layer. Call from net::poll().
pub fn poll_render() {
    if !is_active() { return; }

    // Tick swap animation (smooth window transition)
    let animating = with_compositor(|comp| comp.tick_animation()).unwrap_or(false);
    if animating {
        render_frame();
    }

    // Poll mouse events (batch: update state for all, render only last)
    if let Some(evt) = crate::xhci::poll_mouse() {
        let mut last = evt;
        while let Some(next) = crate::xhci::poll_mouse() {
            with_compositor(|comp| comp.handle_mouse(&last));
            last = next;
        }
        handle_mouse(&last);
    }

    if !terminal::is_dirty() { return; }
    terminal::clear_dirty();

    if crate::layers::is_initialized() {
        poll_render_layered();
    } else {
        poll_render_legacy();
    }
}

fn poll_render_layered() {
    let info = framebuffer::get_info();

    // Re-render text for focused window into Layer 2
    if let Some(ref comp) = *COMPOSITOR.lock() {
        if let Some(fid) = comp.focused {
            if let Some(win) = comp.windows.iter().find(|w| w.id == fid && w.workspace == comp.active_workspace) {
                comp.render_text_to_layer(&info, win);
            }
        }
    }

    // Composite dirty text region
    framebuffer::with_fb(|fb| {
        let (shadow, _) = fb.shadow_ptr();
        let info = fb.info();
        let regions = crate::layers::composite(shadow, info.addr, info.pitch, info.width, info.height);

        if crate::xhci::mouse_available() && !regions.is_empty() {
            if let Some(ref mut comp) = *COMPOSITOR.lock() {
                comp.mouse.redraw_overlay(shadow, info);
            }
        }
    });
}

fn poll_render_legacy() {
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref comp) = *COMPOSITOR.lock() {
            if let Some(fid) = comp.focused {
                if let Some(win) = comp.windows.iter().find(|w| w.id == fid && w.workspace == comp.active_workspace) {
                    let border_color = if win.focused { comp.border_active } else { comp.border_inactive };
                    compositor::Compositor::render_window(shadow, info, win,
                        comp.border, comp.rounding, comp.opacity, comp.scale, border_color);
                    framebuffer::blit_rect(fb, win.x, win.y, win.width, win.height);
                }
            }
        }
    });
}

/// Process a mouse event: update compositor state, redraw cursor overlay.
pub fn handle_mouse(evt: &crate::xhci::MouseEvent) {
    let needs_scene_redraw = with_compositor(|comp| comp.handle_mouse(evt)).unwrap_or(false);

    if crate::layers::is_initialized() {
        if needs_scene_redraw {
            let info = framebuffer::get_info();
            if let Some(ref mut comp) = *COMPOSITOR.lock() {
                comp.render_damaged_to_layers(&info);
            }
        }

        framebuffer::with_fb(|fb| {
            let (shadow, _) = fb.shadow_ptr();
            let info = fb.info();

            if needs_scene_redraw {
                crate::layers::composite(shadow, info.addr, info.pitch, info.width, info.height);
            }

            if let Some(ref mut comp) = *COMPOSITOR.lock() {
                comp.mouse.redraw_overlay(shadow, info);
            }
        });
    } else {
        framebuffer::with_fb(|fb| {
            let info = fb.info();
            let (shadow, _) = fb.shadow_ptr();

            if let Some(ref mut comp) = *COMPOSITOR.lock() {
                if needs_scene_redraw {
                    let regions = comp.render_damaged(shadow, info);
                    for (x, y, w, h) in regions {
                        framebuffer::blit_rect(fb, x, y, w, h);
                    }
                }
                comp.mouse.redraw_overlay(shadow, info);
            }
        });
    }
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
