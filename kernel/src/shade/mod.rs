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

/// Check if layer system is usable (initialized AND matches current framebuffer).
fn layers_usable() -> bool {
    if !crate::layers::is_initialized() { return false; }
    let info = framebuffer::get_info();
    crate::layers::matches_resolution(info.width, info.height, info.pitch)
}

/// Initialize shade compositor. Call after login + GPU setup.
pub fn init() {
    let (screen_w, screen_h, pitch) = framebuffer::with_fb(|fb| {
        let info = fb.info();
        (info.width, info.height, info.pitch)
    }).unwrap_or((1920, 1080, 1920 * 4));

    // Initialize layer compositor (3 layers: Background, Chrome, Text)
    crate::kprintln!("[npk] shade::init: {}x{} pitch={}", screen_w, screen_h, pitch);
    crate::layers::init(screen_w, screen_h, pitch);
    crate::kprintln!("[npk] shade::init: layers_initialized={}", crate::layers::is_initialized());

    // Render background into Layer 0
    if crate::layers::is_initialized() {
        if let Some((bg_buf, _w, _h, _p)) = crate::layers::buffer(crate::layers::LAYER_BG) {
            let info = framebuffer::get_info();
            crate::kprintln!("[npk] shade::init: drawing bg into layer ({}x{})", info.width, info.height);
            crate::gui::background::draw_background(bg_buf, &info);
            crate::layers::mark_full_dirty(crate::layers::LAYER_BG);
        } else {
            crate::kprintln!("[npk] shade::init: BG layer buffer is None!");
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
    if layers_usable() {
        render_frame_layered();
    } else {
        render_frame_legacy();
    }
}

/// Layer-based render: use BG layer as background cache, render chrome+text to shadow.
fn render_frame_layered() {
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        // Copy BG layer → shadow as clean background
        if let Some((bg_buf, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {
            let size = info.pitch as usize * info.height as usize;
            unsafe { core::ptr::copy_nonoverlapping(bg_buf, shadow, size); }
        }

        // Render chrome + text directly to shadow (proven approach)
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
    if layers_usable() {
        render_damaged_layered();
    } else {
        render_damaged_legacy();
    }
}

fn render_damaged_layered() {
    // Use BG layer for background restore, then legacy render_damaged for chrome+text
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
/// Single function — no cache, no legacy fallback, no deadlock.
pub fn render_input_line() {
    terminal::clear_dirty();

    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref comp) = *COMPOSITOR.lock() {
            if let Some(fid) = comp.focused {
                if let Some(win) = comp.windows.iter().find(|w| w.id == fid && w.workspace == comp.active_workspace) {
                    let pad = 6 * comp.scale;
                    let cx = win.content_x(comp.border) + pad;
                    let cy = win.content_y(comp.border) + pad;
                    let cw = win.content_w(comp.border).saturating_sub(pad * 2);
                    let ch = win.content_h(comp.border).saturating_sub(pad * 2);

                    let (char_w, char_h) = crate::gui::font::char_size(1);
                    let cols = cw / char_w;
                    let rows = ch / char_h;
                    if cols == 0 || rows == 0 { return; }

                    let total = terminal::line_count();
                    let visible = (rows as usize).min(total + 1);
                    let last_y = cy + (visible as u32).saturating_sub(1) * char_h;

                    // 1. Restore background — try BG layer (preserves wallpaper)
                    //    Check resolution inline (no layers_usable() → no CONSOLE deadlock)
                    let mut restored_from_layer = false;
                    let layers_init = crate::layers::is_initialized();
                    let layers_match = if layers_init {
                        crate::layers::matches_resolution(info.width, info.height, info.pitch)
                    } else { false };

                    // DEBUG: print once on first keystroke
                    static ONCE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
                    if !ONCE.swap(true, core::sync::atomic::Ordering::Relaxed) {
                        crate::kprintln!("[dbg] input_line: fb={}x{} p={} init={} match={} cx={} ly={} cw={} ch={}",
                            info.width, info.height, info.pitch, layers_init, layers_match, cx, last_y, cw, char_h);
                    }

                    if layers_init && layers_match {
                        if let Some((bg_buf, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {
                            let pitch = info.pitch as usize;
                            let x1 = (cx + cw).min(info.width);
                            let bytes = x1.saturating_sub(cx) as usize * 4;

                            if bytes > 0 {
                                for row in 0..char_h {
                                    let py = last_y + row;
                                    if py < info.height {
                                        let off = py as usize * pitch + cx as usize * 4;
                                        unsafe {
                                            core::ptr::copy_nonoverlapping(
                                                bg_buf.add(off), shadow.add(off), bytes);
                                        }
                                    }
                                }
                                restored_from_layer = true;
                            }
                        }
                    }

                    // 2. Fallback: resolution changed (gpu 4k60) → recalculate background
                    if !restored_from_layer {
                        crate::gui::background::draw_background_region(shadow, info,
                            cx, last_y, cw, char_h);
                    }

                    // 3. Blend content bg (dark tint at opacity)
                    render::fill_rounded_rect_blend(shadow, info,
                        cx, last_y, cw, char_h,
                        win.bg_color, 0, comp.opacity);

                    // 4. Draw text
                    let terms = terminal::current_line_data();
                    let visible_len = terms.1.min(cols as usize);
                    if visible_len > 0 {
                        let prompt_color = crate::gui::background::accent_color();
                        let fg = 0x00E8E8E8u32;
                        if let Ok(text) = core::str::from_utf8(&terms.0[..visible_len]) {
                            if text.contains("@npk") {
                                crate::gui::font::draw_str(shadow, info, text, cx, last_y, prompt_color, None, 1);
                            } else {
                                crate::gui::font::draw_str(shadow, info, text, cx, last_y, fg, None, 1);
                            }
                        }
                    }

                    // 5. Draw cursor
                    let cur = terminal::cursor_pos();
                    let cursor_x = cx + cur as u32 * char_w;
                    if cursor_x + 2 <= cx + cw {
                        render::fill_rect(shadow, info, cursor_x, last_y, 2, char_h, 0x00E8E8E8);
                    }

                    // 6. Blit to MMIO
                    framebuffer::blit_rect(fb, cx, last_y, cw, char_h);
                }
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

    if layers_usable() {
        poll_render_layered();
    } else {
        poll_render_legacy();
    }
}

fn poll_render_layered() {
    // Focused window text changed — use BG layer to restore background, then re-render window
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref comp) = *COMPOSITOR.lock() {
            if let Some(fid) = comp.focused {
                if let Some(win) = comp.windows.iter().find(|w| w.id == fid && w.workspace == comp.active_workspace) {
                    // Restore window region from BG layer
                    if let Some((bg_buf, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {
                        let pitch = info.pitch as usize;
                        let y1 = (win.y + win.height).min(info.height);
                        let x1 = (win.x + win.width).min(info.width);
                        let bytes = x1.saturating_sub(win.x) as usize * 4;
                        if bytes > 0 {
                            for row in win.y..y1 {
                                let off = row as usize * pitch + win.x as usize * 4;
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        bg_buf.add(off), shadow.add(off), bytes);
                                }
                            }
                        }
                    }

                    // Re-render window chrome + text on clean background
                    let border_color = if win.focused { comp.border_active } else { comp.border_inactive };
                    compositor::Compositor::render_window(shadow, info, win,
                        comp.border, comp.rounding, comp.opacity, comp.scale, border_color);
                    framebuffer::blit_rect(fb, win.x, win.y, win.width, win.height);
                }
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
