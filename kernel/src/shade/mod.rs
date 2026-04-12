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

use core::sync::atomic::{AtomicBool, Ordering};
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

/// Lock-free active flag (avoids COMPOSITOR lock from xHCI poll context).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// True while a render task is queued/running on a worker core.
/// Prevents flooding the scheduler with duplicate render tasks.
static RENDER_PENDING: AtomicBool = AtomicBool::new(false);

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

    // Initialize lock-free cursor position (centered, screen bounds for clamping)
    cursor::init_atomic(screen_w, screen_h);

    // Cache framebuffer MMIO address for IRQ-safe cursor draw
    let fb_info = framebuffer::get_info();
    cursor::cache_fb_info(fb_info.addr, fb_info.pitch);

    *COMPOSITOR.lock() = Some(comp);
    ACTIVE.store(true, Ordering::Release);

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
    // Invalidate input line cache — will be rebuilt by render_window
    terminal::invalidate_input_cache();

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

/// Non-blocking render: dispatches to a worker core via SMP scheduler.
/// Core 0 returns immediately — no waiting for 4K framebuffer blit.
/// Duplicate calls while a render is in-flight are silently skipped.
pub fn render_frame_async() {
    if !is_active() { return; }
    if RENDER_PENDING.swap(true, Ordering::AcqRel) {
        return; // Already queued — skip
    }
    crate::smp::scheduler::spawn(
        crate::smp::scheduler::Priority::Interactive,
        render_frame_task,
        0,
    );
}

fn render_frame_task(_arg: u64) {
    render_frame();
    RENDER_PENDING.store(false, Ordering::Release);
}

/// Layer-based render with double-buffer: render to back, swap, blit from front.
/// Cursor reads from front via cached atomic — never blocked by this render.
fn render_frame_layered() {
    framebuffer::with_fb(|fb| {
        let screen_w = fb.info().width;
        let screen_h = fb.info().height;
        let pitch = fb.info().pitch;
        let back = fb.shadow_back();

        // Render scene to BACK buffer (front stays stable for cursor)
        if let Some((bg_buf, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {
            let size = pitch as usize * screen_h as usize;
            // SAFETY: bg_buf and back are valid for size bytes
            unsafe { core::ptr::copy_nonoverlapping(bg_buf, back, size); }
        }

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            comp.aurora_drawn = true;
            comp.render(back, fb.info());
        }

        // Swap front index (blit_rect now reads from new frame)
        // Cached pointer stays on OLD front — cursor matches MMIO during blit.
        fb.swap_buffers();

        // Blit new front → MMIO
        let mut damage = render::DamageTracker::new(screen_w, screen_h);
        damage.mark_all();
        damage.flush(fb);

        // NOW update cached pointer — MMIO matches new front
        fb.commit_front();

        // Cursor overlay on MMIO
        if crate::xhci::mouse_available() {
            cursor::redraw_overlay_lockfree_inner(fb);
        }
    });
}

/// Legacy render with double-buffer (fallback when layers not initialized).
fn render_frame_legacy() {
    framebuffer::with_fb(|fb| {
        let screen_w = fb.info().width;
        let screen_h = fb.info().height;
        let back = fb.shadow_back();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            comp.render(back, fb.info());
        }

        fb.swap_buffers();

        let mut damage = render::DamageTracker::new(screen_w, screen_h);
        damage.mark_all();
        damage.flush(fb);

        fb.commit_front();

        if crate::xhci::mouse_available() {
            cursor::redraw_overlay_lockfree_inner(fb);
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
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            let regions = comp.render_damaged(shadow, info);
            for (x, y, w, h) in regions {
                framebuffer::blit_rect(fb, x, y, w, h);
            }
        }
        if crate::xhci::mouse_available() {
            cursor::redraw_overlay_lockfree_inner(fb);
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
        }
        if crate::xhci::mouse_available() {
            cursor::redraw_overlay_lockfree_inner(fb);
        }
    });
}

/// Check if shade compositor is active (lock-free, safe from any context).
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Check and clear deferred render flag (for callers with their own mouse loop).
pub fn take_deferred_render() -> bool {
    DEFERRED_RENDER.swap(false, Ordering::Relaxed)
}

/// Process a shade action (called from intent loop).
pub fn handle_action(action: input::ShadeAction) {
    use input::ShadeAction;
    match action {
        ShadeAction::NewWindow => {
            with_compositor(|comp| {
                comp.create_window("loop", 0, 0, 800, 600);
            });
            // run_loop prints the prompt via IntentSession
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
/// Uses INPUT_LINE_CACHE from render_window for pixel-perfect background match.
pub fn render_input_line() {
    terminal::clear_dirty();

    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            if let Some(fid) = comp.focused {
                if let Some(win) = comp.windows.iter().find(|w| w.id == fid && w.workspace == comp.active_workspace) {
                    let pad = 6 * comp.scale;
                    let cx = win.content_x(comp.border) + pad;
                    let cy = win.content_y(comp.border) + pad;
                    let cw = win.content_w(comp.border).saturating_sub(pad * 2);
                    let ch = win.content_h(comp.border).saturating_sub(pad * 2);

                    if let Some((x, y, w, h)) = terminal::render_input_line(
                        shadow, &info, cx, cy, cw, ch, win.terminal_idx,
                    ) {
                        framebuffer::blit_rect(fb, x, y, w, h);
                    }
                }
            }
        }
        // Redraw cursor overlay after blit (lock-free, reads atomic position)
        if crate::xhci::mouse_available() {
            cursor::redraw_overlay_lockfree_inner(fb);
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

    // Process each mouse event with clean cursor restore + redraw
    while let Some(evt) = crate::xhci::poll_mouse() {
        handle_mouse(&evt);
    }

    // Deferred scene redraw (drag resize/swap sets this to avoid blocking event loop)
    if DEFERRED_RENDER.swap(false, Ordering::Relaxed) {
        render_frame();
    }

    if !terminal::is_dirty() { return; }
    terminal::clear_dirty();

    // Partial render: only re-render dirty or focused windows (not all).
    // Focus change marks old+new windows dirty. Terminal output marks focused dirty.
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            // Propagate per-terminal dirty flags to window dirty flags
            for win in &mut comp.windows {
                if terminal::is_term_dirty(win.terminal_idx as usize) {
                    win.dirty = true;
                    terminal::clear_term_dirty(win.terminal_idx as usize);
                }
            }

            let focused_id = comp.focused;
            for win in &mut comp.windows {
                if win.workspace != comp.active_workspace { continue; }
                if !win.visible { continue; }
                // Only render windows that are dirty OR focused (active terminal output)
                if !win.dirty && Some(win.id) != focused_id { continue; }
                win.dirty = false;

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

                // Re-render window chrome + text (use theme colors if active)
                let active_border = if crate::theme::is_active() {
                    crate::gui::background::accent_color()
                } else {
                    comp.border_active
                };
                let inactive_border = if crate::theme::is_active() {
                    crate::theme::inactive_border()
                } else {
                    comp.border_inactive
                };
                let border_color = if win.focused { active_border } else { inactive_border };
                compositor::Compositor::render_window(shadow, info, win,
                    comp.border, comp.rounding, comp.opacity, comp.scale, border_color);
                framebuffer::blit_rect(fb, win.x, win.y, win.width, win.height);
            }
        }

        // Redraw cursor overlay (erase old + draw new)
        if crate::xhci::mouse_available() {
            cursor::redraw_overlay_lockfree_inner(fb);
        }
    });
    return;

    // Legacy partial render path (kept for reference)
    #[allow(unreachable_code)]
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
        // Redraw cursor overlay after blit (was overwritten by window render)
        if crate::xhci::mouse_available() {
            cursor::redraw_overlay_lockfree_inner(fb);
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
        if crate::xhci::mouse_available() {
            cursor::redraw_overlay_lockfree_inner(fb);
        }
    });
}

/// True while a Mod+LMB/RMB drag is active (swap or resize).
/// When set, handle_mouse enters slow path on EVERY event (not just button changes).
static DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set by handle_mouse when a full scene redraw is needed but deferred.
/// poll_render picks this up AFTER processing all mouse events.
static DEFERRED_RENDER: AtomicBool = AtomicBool::new(false);

/// Process a mouse event: handle buttons/drag, redraw cursor.
/// Position is already updated by timer IRQ (process_mouse_report).
pub fn handle_mouse(evt: &crate::xhci::MouseEvent) {
    // Position + cursor draw already done by timer IRQ (immediate, lock-free).
    // Main loop only needs to: redraw cursor (cleanup after blit_rect) + handle buttons.
    cursor::redraw_overlay_lockfree();

    // SLOW PATH: take COMPOSITOR lock for focus/drag.
    // Enter on button state change (click/release) OR during active drag (every move).
    let is_button_change = cursor::has_button_event();
    let is_drag = DRAG_ACTIVE.load(Ordering::Relaxed);

    if is_button_change || is_drag {
        let (x, y) = cursor::atomic_pos();
        let (btn, prev) = cursor::atomic_buttons();
        let (needs_redraw, needs_full, dragging) = with_compositor(|comp| {
            comp.mouse.x = x;
            comp.mouse.y = y;
            comp.mouse.buttons = btn;
            comp.mouse.prev_buttons = prev;
            let redraw = comp.handle_mouse_buttons();
            let full = comp.needs_full_redraw;
            comp.needs_full_redraw = false;
            (redraw, full, comp.drag.is_some())
        }).unwrap_or((false, false, false));

        DRAG_ACTIVE.store(dragging, Ordering::Relaxed);

        if needs_redraw {
            if needs_full {
                if dragging {
                    // DEFER: don't render inside the event loop — blocks cursor.
                    // poll_render renders AFTER all events are drained.
                    DEFERRED_RENDER.store(true, Ordering::Relaxed);
                } else {
                    render_frame();
                }
            } else {
                terminal::mark_dirty();
            }
        }
    }
}

/// Stop shade compositor.
pub fn stop() {
    ACTIVE.store(false, Ordering::Release);
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
