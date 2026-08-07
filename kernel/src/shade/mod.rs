//! Shade — nopeekOS Compositor
//!
//! Native Rust compositor layer. Manages windows, Z-order, damage tracking,
//! shadebar (status bar), terminal rendering, and keybindings.
//!
//! Architecture:
//!   Keyboard → Input (Super keybinds) → Compositor → Framebuffer
//!   kprintln → Terminal buffer → Window content rendering

pub mod window;
pub mod compositor;
pub mod terminal;
pub mod input;
pub mod cursor;
pub mod widgets;
pub mod surface;
pub mod clipboard;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

/// Per-frame render/compositor performance logging ([render], [comp],
/// [chrome-cache], [gpu-blit], [rw-phase], [nvme-spin] …). Off for normal
/// use; flip to `true` to re-enable the breakdown for future perf work.
pub const PERF_LOG: bool = false;

use crate::framebuffer::{self};
use crate::gui::{font, render};

#[allow(unused_imports)]
pub use compositor::Compositor;
#[allow(unused_imports)]
pub use window::{WindowId, Window};

/// Global compositor instance.
pub(crate) static COMPOSITOR: Mutex<Option<Compositor>> = Mutex::new(None);

/// Lock-free active flag (avoids COMPOSITOR lock from xHCI poll context).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// True while a render task is queued/running on a worker core.
/// Prevents flooding the scheduler with duplicate render tasks.
#[allow(dead_code)]
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

/// Create a new window. Returns the window ID, or None if no terminal slots.
#[allow(dead_code)]
pub fn create_window(title: &str, x: u32, y: u32, w: u32, h: u32) -> Option<WindowId> {
    with_compositor(|comp| comp.create_window(title, x, y, w, h)).flatten()
}

/// Close and remove a window.
#[allow(dead_code)]
pub fn close_window(id: WindowId) {
    with_compositor(|comp| comp.close_window(id));
}

/// Create a Surface-kind window (microvm framebuffer tile). Returns
/// its id, or None if the compositor isn't up yet.
pub fn create_surface_window(title: &str) -> Option<WindowId> {
    with_compositor(|comp| comp.create_surface_window(title))
}

/// Set focus to a window.
#[allow(dead_code)]
pub fn focus_window(id: WindowId) {
    with_compositor(|comp| comp.focus_window(id));
}

/// Pixels a single mouse-wheel notch scrolls a widget app's
/// `Widget::Scroll` (≈3 text lines).
const WIDGET_SCROLL_STEP: i32 = 48;

/// Width (px) of the clickable scrollbar strip at a window's right edge.
const SCROLLBAR_STRIP: i32 = 16;

/// Active scrollbar drag (set on press over the bar, cleared on release).
static SCROLL_DRAG: spin::Mutex<Option<ScrollDragState>> = spin::Mutex::new(None);

#[derive(Clone, Copy)]
struct ScrollDragState {
    window: u32,
    is_terminal: bool,
    term_idx: u8,
    vx: i32, vy: i32, vw: u32, vh: u32,
    max_px: u32,            // widget: max scroll offset
    total: usize, rows: usize, // terminal: logical lines / visible rows
}

#[inline]
fn in_scroll_strip(mx: i32, my: i32, vx: i32, vy: i32, vw: u32, vh: u32) -> bool {
    mx >= vx + vw as i32 - SCROLLBAR_STRIP && mx < vx + vw as i32
        && my >= vy && my < vy + vh as i32
}

/// Map a drag cursor Y to a scroll offset and apply it.
fn apply_scroll_drag(s: &ScrollDragState, my: i32) {
    let vh = s.vh as i64;
    if vh <= 0 { return; }
    let thumb_h = if s.is_terminal {
        let total = s.total.max(1) as i64;
        let rows = s.rows.max(1) as i64;
        (vh * rows / total).clamp(24, vh)
    } else {
        let content_h = vh + s.max_px as i64;
        (vh * vh / content_h.max(1)).clamp(24, vh)
    };
    let travel = (vh - thumb_h).max(1);
    let thumb_top = ((my as i64 - s.vy as i64) - thumb_h / 2).clamp(0, travel);
    if s.is_terminal {
        // offset 0 = bottom; thumb at top = max scrollback.
        let max_lines = (s.total as i64 - s.rows as i64).max(0);
        let from_bottom = max_lines * (travel - thumb_top) / travel;
        terminal::set_scroll_offset(s.term_idx as usize, from_bottom as usize);
    } else {
        let off = (s.max_px as i64) * thumb_top / travel;
        widgets::set_scroll(s.window, off as u32);
    }
}

/// Try to grab a scrollbar under `(mx, my)`. Returns the drag state if the
/// cursor is on a scrollable window's right-edge strip (and not on the
/// close button).
fn scrollbar_grab(mx: i32, my: i32) -> Option<ScrollDragState> {
    let hit = with_compositor(|c| c.scroll_hit_at(mx, my)).flatten()?;
    if let Some((bx, by, bw, bh)) = hit.close_rect {
        if mx >= bx as i32 && mx < (bx + bw) as i32
            && my >= by as i32 && my < (by + bh) as i32 { return None; }
    }
    if hit.is_terminal {
        let (total, _) = terminal::scroll_metrics(hit.term_idx as usize)?;
        let (vx, vy, vw, vh) = hit.text_rect;
        let rows = (vh / hit.char_h.max(1)).max(1) as usize;
        if total <= rows || !in_scroll_strip(mx, my, vx, vy, vw, vh) { return None; }
        Some(ScrollDragState { window: hit.window, is_terminal: true, term_idx: hit.term_idx,
            vx, vy, vw, vh, max_px: 0, total, rows })
    } else {
        let (vp, max_px) = widgets::scroll_viewport_of(hit.window)?;
        if !in_scroll_strip(mx, my, vp.x, vp.y, vp.w, vp.h) { return None; }
        Some(ScrollDragState { window: hit.window, is_terminal: false, term_idx: 0,
            vx: vp.x, vy: vp.y, vw: vp.w, vh: vp.h, max_px, total: 0, rows: 0 })
    }
}

/// Drive a scrollbar drag from the current mouse state. Returns true if it
/// consumed the event (caller skips the normal focus/window-drag path).
fn handle_scrollbar_drag(mx: i32, my: i32, lmb: bool, was: bool) -> bool {
    let mut guard = SCROLL_DRAG.lock();
    if !lmb {
        return guard.take().is_some();
    }
    if guard.is_none() && !was {
        *guard = scrollbar_grab(mx, my);
    }
    if let Some(s) = guard.as_ref().copied() {
        drop(guard);
        apply_scroll_drag(&s, my);
        render_damaged();
        return true;
    }
    false
}

/// True if the focused window is a terminal (loop) window — used to route
/// the mouse wheel to the terminal scrollback, and by the intent loop to
/// decide whether to drive a terminal session at all.
pub fn focused_is_terminal() -> bool {
    with_compositor(|comp| {
        let Some(fid) = comp.focused else { return false };
        comp.windows.iter().any(|w|
            w.id == fid && w.kind == window::WindowKind::Terminal)
    }).unwrap_or(false)
}

/// If the currently focused window is a widget-kind window, return its id.
/// The intent loop uses this to route key events to the widget's event queue
/// instead of the terminal input path.
pub fn focused_widget_id() -> Option<u32> {
    with_compositor(|comp| {
        let fid = comp.focused?;
        let win = comp.windows.iter().find(|w| w.id == fid)?;
        if win.kind == window::WindowKind::Widget {
            Some(fid.0)
        } else {
            None
        }
    }).flatten()
}

/// If the focused window is a Surface-kind (microvm) window, return
/// its id. run_loop uses this to route input correctly: shade
/// keybinds (Mod+Q etc.) still work so the window is manageable;
/// other keys are swallowed for now (Phase B forwards them to the
/// guest's virtio-input). Without this a focused Surface window
/// wedges the terminal input path (terminal_idx 255, no session).
pub fn focused_surface_id() -> Option<u32> {
    with_compositor(|comp| {
        let fid = comp.focused?;
        let win = comp.windows.iter().find(|w| w.id == fid)?;
        if win.kind == window::WindowKind::Surface {
            Some(fid.0)
        } else {
            None
        }
    }).flatten()
}

/// Forward a host pointer event into the focused Surface window's
/// guest virtio-input eventq as an **absolute** pointer (qemu
/// usb-tablet model): cursor position relative to the tile content
/// rect, scaled to `0..ABS_MAX`, plus button transitions and wheel.
/// Resolution-independent — the guest scales `ABS_MAX` onto its own
/// display, so host-side tile scaling never desyncs the guest cursor.
/// Additive to `handle_mouse` (which still draws the host cursor).
pub fn forward_pointer_to_guest(evt: &crate::xhci::MouseEvent) {
    use crate::microvm::devices::virtio_input_pci::{push_input_event, ABS_MAX};
    const EV_SYN: u16 = 0x00;
    const EV_KEY: u16 = 0x01;
    const EV_REL: u16 = 0x02;
    const EV_ABS: u16 = 0x03;
    const SYN_REPORT: u16 = 0;
    const ABS_X: u16 = 0x00;
    const ABS_Y: u16 = 0x01;
    const REL_WHEEL: u16 = 0x08;
    const BTN_LEFT: u16 = 0x110;
    const BTN_RIGHT: u16 = 0x111;
    const BTN_MIDDLE: u16 = 0x112;

    // Content rect of the focused Surface window.
    let rect = with_compositor(|comp| {
        let fid = comp.focused?;
        let win = comp.windows.iter().find(|w| w.id == fid)?;
        if win.kind != window::WindowKind::Surface { return None; }
        Some((win.content_x(comp.border), win.content_y(comp.border),
              win.content_w(comp.border), win.content_h(comp.border)))
    }).flatten();
    let (cx, cy, cw, ch) = match rect {
        Some(r) if r.2 > 0 && r.3 > 0 => r,
        _ => return, // no Surface focused → no-op (called on every move)
    };

    let (mx, my) = cursor::atomic_pos();
    let rx = (mx - cx as i32).clamp(0, cw as i32 - 1) as u32;
    let ry = (my - cy as i32).clamp(0, ch as i32 - 1) as u32;
    let ax = rx * ABS_MAX / (cw - 1).max(1);
    let ay = ry * ABS_MAX / (ch - 1).max(1);

    let mut any = false;

    // Position — emit only on change. Absolute semantics mean a
    // dropped intermediate sample only coarsens tracking; the final
    // position is always exact (this also bounds INPUT_Q pressure).
    let packed = ((ax as u64) << 32) | ay as u64;
    if LAST_ABS.swap(packed, Ordering::Relaxed) != packed {
        push_input_event(EV_ABS, ABS_X, ax);
        push_input_event(EV_ABS, ABS_Y, ay);
        any = true;
    }

    // Buttons — emit only on transition (bit 0=L 1=R 2=M).
    let prev = LAST_BTN.swap(evt.buttons, Ordering::Relaxed);
    if (prev ^ evt.buttons) != 0 {
        for (bit, code) in [(0u8, BTN_LEFT), (1, BTN_RIGHT), (2, BTN_MIDDLE)] {
            let now = (evt.buttons >> bit) & 1;
            if now != ((prev >> bit) & 1) {
                push_input_event(EV_KEY, code, now as u32);
                any = true;
            }
        }
    }

    // Wheel — Linux reads `value` as __s32; pass the signed delta.
    if evt.scroll != 0 {
        push_input_event(EV_REL, REL_WHEEL, evt.scroll as i32 as u32);
        any = true;
    }

    if any {
        push_input_event(EV_SYN, SYN_REPORT, 0);
    }
}

static LAST_ABS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(u64::MAX);
static LAST_BTN: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(0);

/// Force a full redraw (e.g. after wallpaper change).
pub fn force_redraw() {
    // Invalidate input line cache — will be rebuilt by render_window
    terminal::invalidate_input_cache();

    // Drop the terminal-glass cache: its key doesn't track the wallpaper behind
    // the translucent surface, so a same-theme wallpaper swap would keep the old
    // backdrop until a theme change. A full redraw must re-composite the glass.
    compositor::clear_chrome_cache();

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
    render_frame_mode(false);
}

/// Move the cursor WITHOUT recompositing the scene. The front buffer
/// already holds the composed scene with the cursor baked in at its old
/// spot + the clean pixels under it saved. Restore those (erase old
/// cursor), save-under + bake at the new spot, and blit only the affected
/// bounding box. NO comp.render, NO bg memcpy — that per-move recomposite
/// was the bare-metal cost (+watts/stutter) whenever a window was open.
fn render_frame_cursor_only() {
    note_render(2);
    framebuffer::with_fb(|fb| {
        let info = *fb.info();
        let front = fb.front_ptr();
        let old = cursor::saved_pos();
        let had_old = cursor::save_valid();
        cursor::restore_under(front, &info);       // erase old cursor in RAM
        cursor::save_under_and_bake(front, &info);  // save clean + bake new
        blit_cursor_bbox(fb, old, had_old);         // one atomic small blit
    });
}

fn render_frame_mode(cursor_only: bool) {
    if cursor_only {
        render_frame_cursor_only();
    } else if layers_usable() {
        render_frame_layered();
    } else {
        render_frame_legacy();
    }
}

/// Blit the bounding box of the old ∪ new cursor position from the front
/// buffer to MMIO — one shot, so the move is atomic (no flicker). The old
/// position in the front buffer is clean scene again (cursor restored +
/// re-baked at the new spot), so this both erases the old and draws the new.
fn blit_cursor_bbox(fb: &framebuffer::FbConsole, old: (i32, i32), had_old: bool) {
    let (cw, ch) = cursor::cursor_size();
    let (cx, cy) = cursor::atomic_pos();
    let (x0, y0, x1, y1) = if had_old {
        (old.0.min(cx), old.1.min(cy),
         old.0.max(cx) + cw as i32, old.1.max(cy) + ch as i32)
    } else {
        (cx, cy, cx + cw as i32, cy + ch as i32)
    };
    let x0 = x0.max(0) as u32;
    let y0 = y0.max(0) as u32;
    let w = (x1.max(0) as u32).saturating_sub(x0);
    let h = (y1.max(0) as u32).saturating_sub(y0);
    if w > 0 && h > 0 { framebuffer::blit_rect(fb, x0, y0, w, h); }
}

/// Full recomposite → blit. Scene is composed CLEAN into the back buffer
/// (no cursor), swapped to front, then the cursor is saved-under + baked
/// into the front and the whole frame blitted (cursor included → atomic,
/// no flicker; incl. the dock reveal animation + the microvm tile).
fn render_frame_layered() {
    note_render(0);
    let t0 = crate::interrupts::rdtsc();
    framebuffer::with_fb(|fb| {
        let screen_w = fb.info().width;
        let screen_h = fb.info().height;
        let pitch = fb.info().pitch;
        let info = *fb.info();
        let back = fb.shadow_back();

        // Render scene to BACK buffer (front stays stable for cursor)
        if let Some((bg_buf, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {
            let size = pitch as usize * screen_h as usize;
            // SAFETY: bg_buf and back are valid for size bytes
            unsafe { core::ptr::copy_nonoverlapping(bg_buf, back, size); }
        }
        let t_bg = crate::interrupts::rdtsc();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            comp.aurora_drawn = true;
            comp.render(back, fb.info());
        }
        let t_comp = crate::interrupts::rdtsc();

        fb.swap_buffers();
        fb.commit_front();

        // Bake the cursor into the (now) front buffer, saving the clean
        // pixels under it so a later pure move can erase it without a
        // recompose. Carried by the full blit below → atomic, no flicker.
        cursor::save_under_and_bake(fb.front_ptr(), &info);
        let t_cursor = crate::interrupts::rdtsc();

        let gpu_ok = if crate::gpu::supports_blit() {
            try_gpu_blit(fb, pitch, screen_w, screen_h)
        } else {
            false
        };

        if !gpu_ok {
            let mut damage = render::DamageTracker::new(screen_w, screen_h);
            damage.mark_all();
            damage.flush(fb);
        }
        record_surface_render(t0, t_bg, t_comp, t_cursor);
    });
}

/// Like `render_frame_layered` but for a Surface-only update (a guest/browser
/// FLUSH): recomposite the scene (cheap RAM work) yet blit ONLY the surface
/// tile rect(s) + the cursor to MMIO — not the whole screen. The full-screen
/// MMIO blit is the dominant bare-metal (GOP framebuffer) cost; a 60 Hz guest
/// doing it every frame saturated the framebuffer and starved cursor blits
/// → laggy mouse whenever an app was up. Only the surface pixels changed, so
/// the rest of the front buffer already matches what's on screen. See
/// project-baremetal-gfx-perf ("clip the MMIO blit = the main win").
fn render_frame_surface() {
    note_render(1);
    let t0 = crate::interrupts::rdtsc();
    framebuffer::with_fb(|fb| {
        let screen_w = fb.info().width;
        let screen_h = fb.info().height;
        let pitch = fb.info().pitch;
        let info = *fb.info();
        let back = fb.shadow_back();

        if let Some((bg_buf, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {
            let size = pitch as usize * screen_h as usize;
            // SAFETY: bg_buf and back are valid for size bytes.
            unsafe { core::ptr::copy_nonoverlapping(bg_buf, back, size); }
        }
        let t_bg = crate::interrupts::rdtsc();

        // Recomposite the whole scene into back (correct w.r.t. overlays),
        // collecting Surface tile rects while the lock is held.
        let mut surf_rects = [(0u32, 0u32, 0u32, 0u32); 4];
        let mut n = 0usize;
        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            comp.aurora_drawn = true;
            comp.render(back, fb.info());
            let border = comp.border;
            for win in comp.windows.iter() {
                if win.kind == crate::shade::window::WindowKind::Surface && n < surf_rects.len() {
                    // Blit only the guest's damage rect (content-clipped,
                    // translated to screen coords) instead of the whole
                    // tile — at 4K that's the difference between a tens-of-MB
                    // MMIO blit and just the changed region. comp.render
                    // already produced correct pixels in `back` everywhere,
                    // and nothing but this tile changed, so a partial blit is
                    // correct. Falls back to the full tile if no damage was
                    // recorded or the clamp degenerates.
                    let full = (win.x, win.y, win.width, win.height);
                    let cx = win.x + border;
                    let cy = win.y + border;
                    let cw = win.width.saturating_sub(border * 2);
                    let ch = win.height.saturating_sub(border * 2);
                    surf_rects[n] = match crate::shade::surface::take_damage(win.id.0) {
                        Some((dx, dy, dw, dh)) if dx < cw && dy < ch => {
                            let rw = dw.min(cw - dx);
                            let rh = dh.min(ch - dy);
                            if rw > 0 && rh > 0 { (cx + dx, cy + dy, rw, rh) } else { full }
                        }
                        _ => full,
                    };
                    n += 1;
                }
            }
        }

        let t_comp = crate::interrupts::rdtsc();

        fb.swap_buffers();
        fb.commit_front();

        // Capture the old cursor rect (to erase a ghost if it moved this
        // frame) BEFORE save_under_and_bake overwrites the saved position.
        let old = cursor::saved_pos();
        let had_old = cursor::save_valid();
        cursor::save_under_and_bake(fb.front_ptr(), &info);
        let t_cursor = crate::interrupts::rdtsc();

        // On a HW-blit host (native Xe / BCS) a full blit is cheap → keep it.
        // Otherwise (GOP) clip the MMIO blit to the tiles + cursor bbox.
        let gpu_ok = if crate::gpu::supports_blit() {
            try_gpu_blit(fb, pitch, screen_w, screen_h)
        } else {
            false
        };
        if !gpu_ok {
            let mut damage = render::DamageTracker::new(screen_w, screen_h);
            for i in 0..n {
                let (x, y, w, h) = surf_rects[i];
                damage.mark(x, y, w, h);
            }
            // Cursor bbox (old ∪ new) so a move during this frame ghosts not.
            let (cw, ch) = cursor::cursor_size();
            let (cx, cy) = cursor::atomic_pos();
            damage.mark(cx.max(0) as u32, cy.max(0) as u32, cw, ch);
            if had_old {
                damage.mark(old.0.max(0) as u32, old.1.max(0) as u32, cw, ch);
            }
            damage.flush(fb);
        }
        record_surface_render(t0, t_bg, t_comp, t_cursor);
    });
}

// [surf-render] rolling cost of a full surface composite+blit — the real
// per-frame Core-0 work whose saturation (at the guest's 60 Hz, full-tile)
// stutters the cursor + spikes network rxlat. Tells us the per-blit µs so
// we can size the FPS cap above.
static SURF_RENDER_COUNT: AtomicU64 = AtomicU64::new(0);
static SURF_RENDER_TSC_SUM: AtomicU64 = AtomicU64::new(0);
static SURF_RENDER_TSC_MAX: AtomicU64 = AtomicU64::new(0);
const SURF_RENDER_LOG_EVERY: u64 = 30;

// [render-mix] which render path actually fires (a slow desktop with an app
// open may be re-rendering the whole scene on mouse-move instead of the cheap
// cursor-only path). Counts: layered, surface, cursor-only, legacy.
static MIX_LAYERED: AtomicU64 = AtomicU64::new(0);
static MIX_SURFACE: AtomicU64 = AtomicU64::new(0);
static MIX_CURSOR: AtomicU64 = AtomicU64::new(0);
static MIX_LEGACY: AtomicU64 = AtomicU64::new(0);

fn note_render(kind: u8) {
    match kind {
        0 => MIX_LAYERED.fetch_add(1, Ordering::Relaxed),
        1 => MIX_SURFACE.fetch_add(1, Ordering::Relaxed),
        2 => MIX_CURSOR.fetch_add(1, Ordering::Relaxed),
        _ => MIX_LEGACY.fetch_add(1, Ordering::Relaxed),
    };
    let l = MIX_LAYERED.load(Ordering::Relaxed);
    let s = MIX_SURFACE.load(Ordering::Relaxed);
    let c = MIX_CURSOR.load(Ordering::Relaxed);
    let g = MIX_LEGACY.load(Ordering::Relaxed);
    if (l + s + c + g) % 60 == 0 {
        if PERF_LOG { crate::kprintln!("[render-mix] layered={} surface={} cursor-only={} legacy={}", l, s, c, g); }
    }
}

// [gpu-blit] does the real full-frame BCS blit succeed, and how long?
static GPU_BLIT_OK: AtomicU64 = AtomicU64::new(0);
static GPU_BLIT_FAIL: AtomicU64 = AtomicU64::new(0);
static GPU_BLIT_TSC_SUM: AtomicU64 = AtomicU64::new(0);
static GPU_BLIT_TSC_MAX: AtomicU64 = AtomicU64::new(0);

fn record_gpu_blit(t0: u64, ok: bool) {
    let dt = crate::interrupts::rdtsc().saturating_sub(t0);
    let mhz = (crate::interrupts::tsc_freq() / 1_000_000).max(1);
    GPU_BLIT_TSC_SUM.fetch_add(dt, Ordering::Relaxed);
    GPU_BLIT_TSC_MAX.fetch_max(dt, Ordering::Relaxed);
    if ok { GPU_BLIT_OK.fetch_add(1, Ordering::Relaxed); }
    else { GPU_BLIT_FAIL.fetch_add(1, Ordering::Relaxed); }
    let total = GPU_BLIT_OK.load(Ordering::Relaxed) + GPU_BLIT_FAIL.load(Ordering::Relaxed);
    if total % 120 == 0 {
        let sum = GPU_BLIT_TSC_SUM.swap(0, Ordering::Relaxed);
        let max = GPU_BLIT_TSC_MAX.swap(0, Ordering::Relaxed);
        let okc = GPU_BLIT_OK.swap(0, Ordering::Relaxed);
        let failc = GPU_BLIT_FAIL.swap(0, Ordering::Relaxed);
        if PERF_LOG {
        crate::kprintln!("[gpu-blit] {} blits: ok={} fail={} | avg {}us max {}us",
            okc + failc, okc, failc, (sum / 120) / mhz, max / mhz);
        }
    }
}

static SURF_BG_SUM: AtomicU64 = AtomicU64::new(0);
static SURF_COMP_SUM: AtomicU64 = AtomicU64::new(0);
static SURF_CUR_SUM: AtomicU64 = AtomicU64::new(0);

fn record_surface_render(t0: u64, t_bg: u64, t_comp: u64, t_cursor: u64) {
    let t_end = crate::interrupts::rdtsc();
    let dt = t_end.saturating_sub(t0);
    let mhz = (crate::interrupts::tsc_freq() / 1_000_000).max(1);
    SURF_RENDER_TSC_SUM.fetch_add(dt, Ordering::Relaxed);
    SURF_RENDER_TSC_MAX.fetch_max(dt, Ordering::Relaxed);
    // Sub-phase breakdown: bg memcpy | comp.render | swap+cursor | blit.
    SURF_BG_SUM.fetch_add(t_bg.saturating_sub(t0), Ordering::Relaxed);
    SURF_COMP_SUM.fetch_add(t_comp.saturating_sub(t_bg), Ordering::Relaxed);
    SURF_CUR_SUM.fetch_add(t_cursor.saturating_sub(t_comp), Ordering::Relaxed);
    let n = SURF_RENDER_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if n % SURF_RENDER_LOG_EVERY == 0 {
        let sum = SURF_RENDER_TSC_SUM.swap(0, Ordering::Relaxed);
        let max = SURF_RENDER_TSC_MAX.swap(0, Ordering::Relaxed);
        let bg = SURF_BG_SUM.swap(0, Ordering::Relaxed);
        let comp = SURF_COMP_SUM.swap(0, Ordering::Relaxed);
        let cur = SURF_CUR_SUM.swap(0, Ordering::Relaxed);
        let e = SURF_RENDER_LOG_EVERY;
        if PERF_LOG {
        crate::kprintln!(
            "[render] {} frames avg {}us (bg {} | comp {} | cursor {} | blit {}) max {}us",
            e, (sum / e) / mhz, (bg / e) / mhz, (comp / e) / mhz, (cur / e) / mhz,
            (sum.saturating_sub(bg + comp + cur) / e) / mhz, max / mhz,
        );
        }
    }
}

/// Legacy render with double-buffer (fallback when layers not initialized).
fn render_frame_legacy() {
    note_render(3);
    framebuffer::with_fb(|fb| {
        let screen_w = fb.info().width;
        let screen_h = fb.info().height;
        let pitch = fb.info().pitch;
        let info = *fb.info();
        let back = fb.shadow_back();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            comp.render(back, fb.info());
        }

        fb.swap_buffers();
        fb.commit_front();

        cursor::save_under_and_bake(fb.front_ptr(), &info);

        let gpu_ok = if crate::gpu::supports_blit() {
            try_gpu_blit(fb, pitch, screen_w, screen_h)
        } else {
            false
        };

        if !gpu_ok {
            let mut damage = render::DamageTracker::new(screen_w, screen_h);
            damage.mark_all();
            damage.flush(fb);
        }
    });
}

/// Try GPU BCS blit from front shadow → MMIO framebuffer.
/// Returns true if GPU blit succeeded, false = caller should CPU-blit.
/// Note: VSync via wait_vblank adds up to 16.7ms latency per render — not suitable
/// for on-demand compositing. True VSync needs PLANE_SURF double-buffer flip (TODO).
fn try_gpu_blit(fb: &framebuffer::FbConsole, pitch: u32, _w: u32, h: u32) -> bool {
    if !crate::gpu::supports_blit() { return false; }

    let src_ggtt = fb.front_ggtt();
    let dst_ggtt = crate::gpu::fb_ggtt_offset();
    // dst_ggtt == 0 is VALID: when the firmware scanout sits at GGTT offset 0
    // (Tiger Lake blit-only, fw PLANE_SURF=0), that IS the live scanout. Only
    // a missing shadow mapping (src == 0) means "not set up". Treating dst 0 as
    // invalid was silently routing the whole frame back to the 100ms CPU/UC
    // blit even though BCS is up + readback-verified.
    if src_ggtt == 0 { return false; }

    // [gpu-blit] instrument: does the real (full 4K) blit succeed, and how
    // long does submit+poll take? A small readback blit verified fine but a
    // 33MB blit may time out submit_blit's completion poll → false → CPU
    // fallback (the 100ms we still measure).
    let t = crate::interrupts::rdtsc();
    let ok = crate::gpu::gpu_blit_rect(src_ggtt, pitch, dst_ggtt, pitch, 0, 0, pitch / 4, h);
    record_gpu_blit(t, ok);
    ok
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
        let info = *fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            let regions = comp.render_damaged(shadow, &info);
            cursor::draw_cursor_on_shadow(shadow, &info);
            for (x, y, w, h) in regions {
                framebuffer::blit_rect(fb, x, y, w, h);
            }
        }
    });
}

/// Fade the focus flash down to the steady halo, repainting only its band —
/// nothing inside the window changes, so a full frame per tick would be pure
/// waste. Every loop that idles while shade is up has to call this, or the
/// flash stands still at full strength until something else repaints.
pub fn tick_focus_glow() {
    if !is_active() { return; }
    if crate::smp::per_core::current_core_id() != 0 { return; }
    if with_compositor(|comp| comp.tick_focus_glow()).unwrap_or(false) {
        render_focus_glow();
    }
}

/// Repaint just the focus halo band (see `Compositor::render_focus_glow`).
fn render_focus_glow() {
    framebuffer::with_fb(|fb| {
        let info = *fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref comp) = *COMPOSITOR.lock() {
            // The cursor is BAKED into the front buffer. Baking another one
            // per tick — which is what a plain `draw_cursor_on_shadow` does —
            // left a copy at every position the mouse passed through: this
            // pass repaints only the halo band, so nothing overwrites them
            // (they vanished later, when a full render came along). Erase,
            // repaint, bake at the new spot: the same save/restore dance
            // `render_frame_cursor_only` does for every partial repaint.
            let old = cursor::saved_pos();
            let had_old = cursor::save_valid();
            cursor::restore_under(shadow, &info);
            let regions = comp.render_focus_glow(shadow, &info);
            cursor::save_under_and_bake(shadow, &info);
            for (x, y, w, h) in regions {
                framebuffer::blit_rect(fb, x, y, w, h);
            }
            blit_cursor_bbox(fb, old, had_old);
        }
    });
}

fn render_damaged_legacy() {
    framebuffer::with_fb(|fb| {
        let info = *fb.info();
        let (shadow, _) = fb.shadow_ptr();

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            let regions = comp.render_damaged(shadow, &info);
            cursor::draw_cursor_on_shadow(shadow, &info);
            for (x, y, w, h) in regions {
                framebuffer::blit_rect(fb, x, y, w, h);
            }
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

/// Request a full shade render on the next Core-0 poll cycle. Safe to
/// call from any core (worker cores executing WASM etc.) — actual
/// MMIO work still runs on Core 0.
pub fn request_render() {
    DEFERRED_RENDER.store(true, Ordering::Relaxed);
}

/// Process a shade action (called from intent loop).
pub fn handle_action(action: input::ShadeAction) {
    use input::ShadeAction;

    // Modal gate: while ANY window is marked modal (set via
    // `npk_window_set_modal` from the app), suppress every action that
    // would steal focus or reshape the grid underneath. CloseWindow,
    // SpawnLauncher, and Lock stay allowed so the user can still
    // dismiss / relaunch the overlay. Kernel knows nothing about which
    // app set itself modal — the flag is per-window.
    let modal_open = with_compositor(|c| c.has_modal_window()).unwrap_or(false);
    if modal_open {
        match action {
            ShadeAction::CloseWindow
            | ShadeAction::SpawnLauncher
            | ShadeAction::Lock
            | ShadeAction::Copy
            | ShadeAction::Paste => {}
            _ => return,
        }
    }

    match action {
        ShadeAction::NewWindow => {
            let created = with_compositor(|comp| {
                comp.create_window("loop", 0, 0, 800, 600).is_some()
            }).unwrap_or(false);
            if created {
                // Terminal was freshly allocated (cleared) — reset session prompt
                crate::intent::reset_session_prompt(terminal::active_idx());
                render_frame();
            }
            // If not created: no free terminals, silently ignore
        }
        ShadeAction::CloseWindow => {
            // compositor::close_window handles widget-scene cleanup
            // via remove_scene when the closed window's kind is Widget.
            with_compositor(|comp| {
                if let Some(id) = comp.focused {
                    // Never close a panel (dock/bar) via Mod+Q — they are
                    // managed chrome. Focus shouldn't land on one, but guard
                    // here too in case some path focuses a panel.
                    if !comp.is_panel(id) {
                        comp.request_close_window(id);
                    }
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
        ShadeAction::ToggleSplit => {
            with_compositor(|comp| comp.toggle_split());
            render_frame();
        }
        ShadeAction::ScrollUp => {
            // Focused widget app with scrollable content → scroll its
            // Widget::Scroll; otherwise the terminal scrollback. (scroll_by
            // locks the compositor internally, so resolve the target first
            // and release the lock before calling it — no re-entrancy.)
            if let Some(wid) = focused_widget_id() {
                widgets::scroll_by(wid, -WIDGET_SCROLL_STEP);
                render_damaged();
            } else {
                terminal::scroll_up(10);
                with_compositor(|comp| {
                    if let Some(fid) = comp.focused {
                        if let Some(win) = comp.window_mut(fid) { win.dirty = true; }
                    }
                });
                render_damaged();
            }
        }
        ShadeAction::ScrollDown => {
            if let Some(wid) = focused_widget_id() {
                widgets::scroll_by(wid, WIDGET_SCROLL_STEP);
                render_damaged();
            } else {
                terminal::scroll_down(10);
                with_compositor(|comp| {
                    if let Some(fid) = comp.focused {
                        if let Some(win) = comp.window_mut(fid) { win.dirty = true; }
                    }
                });
                render_damaged();
            }
        }
        ShadeAction::Lock => {
            // Lock handled by intent loop
        }
        ShadeAction::SpawnLauncher => {
            spawn_launcher();
        }
        // Ctrl+Shift+C / V. In a terminal (where Ctrl+C stays SIGINT) copy
        // the mouse selection / paste into the input line; in a widget app
        // copy or paste the focused Input/TextArea selection.
        ShadeAction::Copy => {
            if focused_is_terminal() {
                terminal::copy_selection();
            } else if let Some(wid) = focused_widget_id() {
                widgets::clipboard_copy(wid);
            }
        }
        ShadeAction::Paste => {
            if focused_is_terminal() {
                terminal::paste_clipboard();
            } else if let Some(wid) = focused_widget_id() {
                widgets::clipboard_paste(wid);
            }
        }
    }
}

/// Spawn the configured launcher module as a widget app.
///
/// Module name is read from `sys/config/launcher` (single line, e.g.
/// `drun`). When the file is missing or unreadable we fall back to
/// `drun` so a fresh install still has a working Mod+D out of the box.
///
/// The kernel knows nothing else about the module — overlay size,
/// modal behaviour, and UI are entirely the module's responsibility
/// (see `npk_window_set_overlay` / `npk_window_set_modal`).
fn spawn_launcher() {
    const DEFAULT_LAUNCHER: &str = "drun";

    // Resolve the configured name. Limit to 64 bytes to prevent
    // surprises from a giant config file.
    let name: alloc::string::String = match crate::npkfs::fetch("sys/config/launcher") {
        Ok((bytes, _)) => {
            let raw = core::str::from_utf8(&bytes).unwrap_or("").trim();
            let cleaned: alloc::string::String = raw
                .chars()
                .take_while(|c| !c.is_control())
                .take(64)
                .collect();
            if cleaned.is_empty() || cleaned.contains('/') || cleaned.contains("..") {
                alloc::string::String::from(DEFAULT_LAUNCHER)
            } else {
                cleaned
            }
        }
        Err(_) => alloc::string::String::from(DEFAULT_LAUNCHER),
    };

    // Already running? Re-focus instead of spawning a second instance.
    // Window title matches the module name, set by the spawn path.
    let already_open = with_compositor(|comp| {
        comp.windows.iter()
            .find(|w| w.kind == window::WindowKind::Widget && w.title == name)
            .map(|w| w.id)
    }).flatten();
    if let Some(id) = already_open {
        with_compositor(|comp| comp.focus_window(id));
        render_frame();
        return;
    }

    let path = alloc::format!("sys/wasm/{}", name);
    let (bytes, _hash) = match crate::npkfs::fetch(&path) {
        Ok(v) => v,
        Err(_) => {
            crate::kprintln!("[npk] launcher: '{}' not installed", name);
            return;
        }
    };

    // Grant exactly the rights the app declares in its `.npk.caps`
    // section (per-app, no blanket WRITE); absent → safe default.
    let rights = crate::capability::widget_rights_from_wasm(&bytes);
    let module_cap = match crate::capability::create_module_cap(rights, Some(600_000)) {
        Ok(id) => id,
        Err(e) => { crate::kprintln!("[npk] launcher: cap delegation failed: {}", e); return; }
    };

    // Spawn with widget_wid = 0 — the module itself decides whether
    // to be an overlay and modal. Window is created on its first
    // scene_commit (or npk_window_set_overlay call).
    if !crate::wasm::spawn_widget_app(bytes.to_vec(), module_cap, &name, 0) {
        crate::kprintln!("[npk] launcher: spawn failed for '{}'", name);
    }
}

/// Launch an installed widget module by name (e.g. "loft") from kernel
/// code — the spawn half of `spawn_launcher`, exposed for cross-boundary
/// actions (the 9p "open in loft" trigger from the microvm browser).
/// MUST run on Core 0 (touches the compositor). Re-focuses an existing
/// window of that name instead of spawning a duplicate.
/// Launch every app named in the `autostart` config (comma/space-
/// separated) via the widget-spawn path — a fresh window, no terminal,
/// no `APP_RUNNING` capture (so background overlays like the dock don't
/// hijack keyboard focus the way `run <module>` would). Called once
/// after `shade::init()`. The kernel stays generic: the app names live
/// in config, not here. Set with e.g. `set autostart dock`.
pub fn start_autostart() {
    let list = match crate::config::get("autostart") {
        Some(v) => v,
        None => return,
    };
    for name in list
        .split(|c| c == ',' || c == ' ')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        launch_app(name);
    }
}

pub fn launch_app(name: &str) {
    let already_open = with_compositor(|comp| {
        comp.windows.iter()
            .find(|w| w.kind == window::WindowKind::Widget && w.title == name)
            .map(|w| w.id)
    }).flatten();
    if let Some(id) = already_open {
        with_compositor(|comp| comp.focus_window(id));
        render_frame();
        return;
    }
    let path = alloc::format!("sys/wasm/{}", name);
    let (bytes, _hash) = match crate::npkfs::fetch(&path) {
        Ok(v) => v,
        Err(_) => { crate::kprintln!("[npk] launch_app: '{}' not installed", name); return; }
    };
    // Per-app rights from the module's `.npk.caps` section (no blanket
    // WRITE); absent → safe default.
    let rights = crate::capability::widget_rights_from_wasm(&bytes);
    let module_cap = match crate::capability::create_module_cap(rights, Some(600_000)) {
        Ok(id) => id,
        Err(e) => { crate::kprintln!("[npk] launch_app: cap failed: {}", e); return; }
    };
    if !crate::wasm::spawn_widget_app(bytes.to_vec(), module_cap, name, 0) {
        crate::kprintln!("[npk] launch_app: spawn failed for '{}'", name);
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
                        cursor::draw_cursor_on_shadow(shadow, &info);
                        framebuffer::blit_rect(fb, x, y, w, h);
                    }
                }
            }
        }
    });
}

/// Progressive render: if terminal has new output, re-render the focused window's text.
/// Fast path: only redraws text layer. Call from net::poll().
/// Only Core 0 may call this — compositor + framebuffer are not thread-safe.
pub fn poll_render() {
    if !is_active() { return; }
    if crate::smp::per_core::current_core_id() != 0 { return; }

    // Tick swap animation (smooth window transition)
    let animating = with_compositor(|comp| comp.tick_animation()).unwrap_or(false);
    if animating {
        render_frame();
    }

    // Screenshot flash — its own tick so it also runs while nothing else
    // is moving (a capture usually happens on a still desktop).
    if with_compositor(|comp| comp.flash_tick()).unwrap_or(false) {
        render_frame();
    }

    if !animating { tick_focus_glow(); }

    // Reap close requests a guarded app never answered.
    if with_compositor(|comp| comp.tick_close_requests()).unwrap_or(false) {
        render_frame();
    }

    // Drive the auto-hide dock. Feed it the current cursor Y so the reveal
    // dwell / hide debounce advance even while the cursor is parked, then
    // advance the slide. Re-render the frame while it's moving.
    let (_, cy) = cursor::atomic_pos();
    let dock_moving = with_compositor(|comp| {
        comp.dock_update_reveal(cy);
        comp.dock_tick()
    }).unwrap_or(false);
    if dock_moving {
        render_frame();
    }

    // Process each mouse event with clean cursor restore + redraw
    while let Some(evt) = crate::xhci::poll_mouse() {
        handle_mouse(&evt);
    }

    // Deferred scene redraw (drag resize/swap sets this to avoid blocking event loop)
    if DEFERRED_RENDER.swap(false, Ordering::Relaxed) {
        SURFACE_DIRTY.store(false, Ordering::Relaxed); // a full render covers the tile too
        render_frame();
        CURSOR_MOVED.store(false, Ordering::Relaxed);
    } else if !terminal::is_dirty() && SURFACE_DIRTY.load(Ordering::Relaxed) {
        // Guest produced a new frame. On HW the guest reports 100% damage
        // every frame, so this is always a full-tile blit — ~16 MB of MMIO
        // at 4K. Run unthrottled at the guest's 60 Hz it saturates Core 0,
        // which ALSO drains the NIC RX ring, so saturation spiked network
        // rxlat into the 100 ms range and the host cursor stuttered across
        // ALL windows. So FPS-cap the heavy blit: at most once per
        // SURFACE_MIN_TICKS normally, and stretch the cap to
        // SURFACE_MAX_STALE_TICKS while the mouse is actively moving (the
        // browser briefly drops fps) so the cursor stays smooth — the gaps
        // are filled with the cheap cursor-only render. Coalescing is free:
        // SURFACE_DIRTY stays set and we render the latest frame when due.
        let now = crate::interrupts::ticks();
        let since = now.wrapping_sub(LAST_SURFACE_TICK.load(Ordering::Relaxed));
        let cap = if CURSOR_MOVED.load(Ordering::Relaxed) {
            SURFACE_MAX_STALE_TICKS
        } else {
            SURFACE_MIN_TICKS
        };
        if since >= cap {
            SURFACE_DIRTY.store(false, Ordering::Relaxed);
            LAST_SURFACE_TICK.store(now, Ordering::Relaxed);
            render_frame_surface();
            CURSOR_MOVED.store(false, Ordering::Relaxed);
        } else if CURSOR_MOVED.swap(false, Ordering::Relaxed) {
            render_frame_cursor_only();
        }
    } else if !terminal::is_dirty() && CURSOR_MOVED.swap(false, Ordering::Relaxed) {
        // Pure mouse move, nothing else changed → save-under cursor move.
        render_frame_cursor_only();
    }

    if !terminal::is_dirty() { return; }
    terminal::clear_dirty();
    CURSOR_MOVED.store(false, Ordering::Relaxed);

    // Partial render: only re-render dirty or focused windows (not all).
    // Focus change marks old+new windows dirty. Terminal output marks focused dirty.
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();

        // Erase the baked cursor before repainting windows; it's re-baked
        // + blitted at the end so the partial update stays flicker-free.
        let cur_old = cursor::saved_pos();
        let cur_had = cursor::save_valid();
        cursor::restore_under(shadow, info);

        if let Some(ref mut comp) = *COMPOSITOR.lock() {
            // Propagate per-terminal dirty flags to window dirty flags
            for win in &mut comp.windows {
                if terminal::is_term_dirty(win.terminal_idx as usize) {
                    win.dirty = true;
                    terminal::clear_term_dirty(win.terminal_idx as usize);
                }
            }

            // Any overlay that sits on top of a dirty non-overlay must
            // repaint too — otherwise the window below paints over it
            // (classic top-refresh-punches-through-drun bug).
            let dirty_rects: alloc::vec::Vec<(u32, u32, u32, u32)> = comp.windows.iter()
                .filter(|w| w.dirty && !w.is_overlay
                    && w.workspace == comp.active_workspace && w.visible)
                .map(|w| (w.x, w.y, w.width, w.height))
                .collect();
            for win in &mut comp.windows {
                if !win.is_overlay || win.dirty { continue; }
                for (rx, ry, rw, rh) in &dirty_rects {
                    if win.x < rx + rw && *rx < win.x + win.width
                        && win.y < ry + rh && *ry < win.y + win.height
                    {
                        win.dirty = true;
                        break;
                    }
                }
            }

            let focused_id = comp.focused;
            // Render back-to-front so overlays paint last (otherwise a
            // terminal behind drun would overwrite drun's pixels).
            let render_order: alloc::vec::Vec<crate::shade::window::WindowId> =
                comp.z_order.iter().rev().copied().collect();
            // One source of truth for the tile borders: the palette. It
            // already folds in the wallpaper accent (`accent = auto`) and
            // any `set accent` override, so there is no second theme path.
            let active_border = comp.border_active();
            let inactive_border = comp.border_inactive();
            for wid in render_order {
                // Skip checks and halo up front: the paint below needs a &mut
                // to clear `dirty`, while the halo has to be resolved against
                // the whole compositor (gaps, flash state).
                let glow = match comp.windows.iter().find(|w| w.id == wid) {
                    Some(w) if w.workspace == comp.active_workspace
                        && w.visible
                        && (w.dirty || Some(w.id) == focused_id) => comp.glow_for(w),
                    _ => continue,
                };
                let win = match comp.windows.iter_mut().find(|w| w.id == wid) {
                    Some(w) => w,
                    None => continue,
                };
                win.dirty = false;

                // Restore window region from BG layer. Overlays skip
                // this — we want their rounded-out corners to keep
                // whatever scene (terminal, another app, wallpaper)
                // sits below in the current shadow state.
                if !win.is_overlay {
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
                }

                let border_color = if win.focused { active_border } else { inactive_border };
                compositor::Compositor::render_window(shadow, info, win,
                    comp.border, comp.rounding, comp.opacity, comp.scale, border_color,
                    glow);
                framebuffer::blit_rect(fb, win.x, win.y, win.width, win.height);
            }
        }

        // Re-bake the cursor on top of the repainted windows + blit its
        // region (atomic). Covers a cursor over a just-redrawn window and
        // erases it at the old spot.
        cursor::save_under_and_bake(shadow, info);
        blit_cursor_bbox(fb, cur_old, cur_had);
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
                    let border_color = if win.focused { comp.border_active() } else { comp.border_inactive() };
                    compositor::Compositor::render_window(shadow, info, win,
                        comp.border, comp.rounding, comp.opacity, comp.scale, border_color,
                        comp.glow_for(win));
                    cursor::draw_cursor_on_shadow(shadow, info);
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
                    let border_color = if win.focused { comp.border_active() } else { comp.border_inactive() };
                    compositor::Compositor::render_window(shadow, info, win,
                        comp.border, comp.rounding, comp.opacity, comp.scale, border_color,
                        comp.glow_for(win));
                    cursor::draw_cursor_on_shadow(shadow, info);
                    framebuffer::blit_rect(fb, win.x, win.y, win.width, win.height);
                }
            }
        }
    });
}

/// True while a Mod+LMB/RMB drag is active (swap or resize).
/// When set, handle_mouse enters slow path on EVERY event (not just button changes).
static DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set by handle_mouse when a full scene redraw is needed but deferred.
/// poll_render picks this up AFTER processing all mouse events.
static DEFERRED_RENDER: AtomicBool = AtomicBool::new(false);
/// A pure mouse move occurred (no scene change). poll_render moves the
/// cursor via save-under (no recomposite) for it — cheap, flicker-free.
static CURSOR_MOVED: AtomicBool = AtomicBool::new(false);

/// A guest Surface produced a new frame (browser/microvm FLUSH). Distinct
/// from DEFERRED_RENDER: only the surface tile pixels changed, so the final
/// MMIO blit can be clipped to the tile rect instead of the whole screen.
/// The full-screen blit per 60 Hz guest frame is the dominant bare-metal
/// (GOP framebuffer) cost and starved the cursor → lag when an app is up.
static SURFACE_DIRTY: AtomicBool = AtomicBool::new(false);

/// Last tick (100 Hz) at which a Surface (guest) frame was actually
/// composited+blit. poll_render throttles the expensive 4K-tile surface
/// render in favour of the cheap cursor-only path while the host mouse is
/// moving, so a busy guest can't starve the cursor across all windows.
static LAST_SURFACE_TICK: AtomicU64 = AtomicU64::new(0);

/// Min ticks (100 Hz) between full-tile surface blits when the mouse is
/// idle — FPS-caps the ~16 MB 4K MMIO blit at ~50 fps so a 60 Hz guest
/// can't peg Core 0 (which also drains the NIC RX ring).
const SURFACE_MIN_TICKS: u64 = 2;
/// Min ticks between surface blits while the mouse is actively moving —
/// the browser drops to ~20 fps so the cheap cursor-only path can keep the
/// cursor smooth in the gaps.
const SURFACE_MAX_STALE_TICKS: u64 = 5;

/// Clip the blit on a Surface flush to the tile rect (vs full-screen). Flag-
/// gated because the cursor/blit path has a flicker history (see
/// project-baremetal-gfx-perf) — flip to `false` to fall back to a full
/// `request_render` per guest frame (the old behavior).
pub const SURFACE_CLIP_BLIT: bool = true;

/// A guest Surface produced a new frame → clipped recomposite+blit. Set from
/// `surface::write_frame`; consumed by `poll_render`. No-op (caller uses the
/// full `request_render`) when `SURFACE_CLIP_BLIT` is off.
pub fn request_surface_render() {
    SURFACE_DIRTY.store(true, Ordering::Relaxed);
}

/// Signal a bare pointer move from the input IRQ. Picks the cheap cursor-
/// only render as the default; `handle_mouse` (run by the Core-0 loop /
/// poll_render before this flag is consumed) upgrades to a full
/// DEFERRED_RENDER if the event turns out to be a drag/click/scroll. The
/// IRQ previously called the full `request_render`, which forced a whole-
/// scene recomposite on *every* pointer delta and defeated `handle_mouse`'s
/// own CURSOR_MOVED logic (DEFERRED_RENDER is never cleared by it) — the
/// dominant framebuffer cost while a window/tile was open.
pub fn request_cursor_move() {
    CURSOR_MOVED.store(true, Ordering::Relaxed);
}

/// Process a mouse event: handle buttons/drag, redraw cursor.
/// Position is already updated by timer IRQ (process_mouse_report).
pub fn handle_mouse(evt: &crate::xhci::MouseEvent) {
    // Forward to a focused microvm guest HERE, not at the call sites:
    // poll_mouse is a single consuming ring with several Core-0
    // consumers (this via poll_render:612, and the run_loop Surface
    // branch). Whichever drains first must still forward, else the
    // guest only gets events when the timing happens to favour the
    // forwarding path (was: "only with Mod held" — Mod+drag shifted
    // the race). handle_mouse is on every consumer's path, so this is
    // race-free. forward_pointer_to_guest no-ops unless a Surface
    // window is focused, so this is free in normal desktop use.
    forward_pointer_to_guest(evt);

    // Mouse wheel over a focused widget app → scroll its Widget::Scroll.
    // (Surface/microvm windows already consumed the wheel in
    // forward_pointer_to_guest above; terminal scrollback uses PageUp/Down.)
    // The raw USB wheel never went through the ShadeAction::Scroll* path —
    // that only fires for keyboard/serial — so wire it here. scroll_by locks
    // the compositor internally, so it must run outside any with_compositor.
    if evt.scroll != 0 {
        if let Some(wid) = focused_widget_id() {
            // Ctrl+wheel is a zoom request, not a scroll. Sent to the app
            // instead of moving the viewport — an app that doesn't zoom
            // just ignores it (and deliberately doesn't scroll either,
            // which is what every editor does).
            if crate::keyboard::is_ctrl_held() {
                widgets::push_event(wid, widgets::abi::Event::Zoom {
                    delta: evt.scroll as i32,
                });
                request_render();
                return;
            }
            let delta = -(evt.scroll as i32) * WIDGET_SCROLL_STEP;
            if widgets::scroll_by(wid, delta) {
                request_render();
            } else {
                // No Widget::Scroll consumed it → forward to the app so a
                // Canvas/surface app (e.g. the browser) can scroll itself.
                widgets::push_event(wid, widgets::abi::Event::Wheel { dy: delta });
            }
        } else if focused_is_terminal() {
            // loop/terminal scrollback — wheel up = older lines.
            if evt.scroll > 0 { terminal::scroll_up(3); } else { terminal::scroll_down(3); }
            with_compositor(|comp| {
                if let Some(fid) = comp.focused {
                    if let Some(win) = comp.window_mut(fid) { win.dirty = true; }
                }
            });
            render_damaged();
        }
    }

    // Scrollbar drag (loop + widget windows). Handled before the
    // compositor's focus/window-drag logic so dragging the bar never moves
    // or refocuses windows; a consumed event short-circuits the rest.
    {
        let (smx, smy) = cursor::atomic_pos();
        let (btn, prev) = cursor::atomic_buttons();
        if handle_scrollbar_drag(smx, smy, btn & 1 != 0, prev & 1 != 0) {
            return;
        }
    }

    // Terminal text selection (drag to mark; Ctrl+Shift+C copies). After the
    // scrollbar strip so a press on the bar scrolls instead of selecting.
    {
        let (mx, my) = cursor::atomic_pos();
        let (btn, prev) = cursor::atomic_buttons();
        update_terminal_selection(mx, my, btn & 1 != 0, prev & 1 != 0);
    }

    // Hover routing — deduplicated per window. Runs on every move so the
    // app can react even when no buttons changed.
    let (hx, hy) = cursor::atomic_pos();
    let hover_target = with_compositor(|comp| {
        if comp.drag.is_some() { return None; }
        let wid = comp.window_at(hx, hy)?;
        let is_widget = comp.windows.iter()
            .any(|w| w.id == wid && w.kind == crate::shade::window::WindowKind::Widget);
        if is_widget { Some(wid.0) } else { None }
    }).flatten();
    if let Some(wid) = hover_target {
        widgets::update_hover(wid, hx, hy);
    }

    // Drag motion → the focused widget app. Only while its primary button
    // is held, i.e. during an actual drag: hover is resolved above,
    // compositor-side, precisely so apps never see a firehose of moves.
    // A drag is the part only the app can interpret (panning a zoomed
    // canvas, rubber-banding), and it is bounded by the button being down.
    // Addressed to the FOCUSED window, not the one under the cursor, so a
    // drag that leaves the window keeps arriving.
    //
    // This has to live here, on the real motion path: `handle_mouse` on
    // the compositor looks like the right home but has no callers at all.
    {
        let (btn, _) = cursor::atomic_buttons();
        if btn & 1 != 0
            && !DRAG_ACTIVE.load(Ordering::Relaxed)
            && !crate::keyboard::is_super_held()
        {
            if let Some(wid) = focused_widget_id() {
                widgets::push_event(wid, widgets::abi::Event::MouseMove { x: hx, y: hy });
            }
        }
    }

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

    // Widget text selection — after the button block (press_at has focused
    // the clicked Input/TextArea) and independent of it (so pure drag-moves,
    // which raise no button change, still extend the selection).
    {
        let (mx, my) = cursor::atomic_pos();
        let (btn, prev) = cursor::atomic_buttons();
        update_widget_text_selection(mx, my, btn & 1 != 0, prev & 1 != 0);
    }

    // If nothing above scheduled a real render (no button, no drag, no
    // DEFERRED set by a hover/focus change), this was a pure positional
    // move → flag a cheap cursor-only render (recompose + blit just the
    // cursor rects). Otherwise the pending full render carries the cursor.
    if !is_button_change
        && !DRAG_ACTIVE.load(Ordering::Relaxed)
        && !DEFERRED_RENDER.load(Ordering::Relaxed)
    {
        CURSOR_MOVED.store(true, Ordering::Relaxed);
    } else {
        CURSOR_MOVED.store(false, Ordering::Relaxed);
    }
}

/// Terminal drag-selection state machine. A press inside a terminal's text
/// area begins a selection; holding + moving extends it; release ends it.
/// Geometry is captured at press so a drag can run past the window edge.
/// Doesn't consume the event — a plain LMB drag in a terminal never moves
/// the window, so the normal focus/click flow runs alongside.
/// Re-render the focused terminal so a selection change (mouse or keyboard)
/// shows. Marks the terminal + its window dirty, then repaints damage.
pub fn refresh_focused_terminal() {
    terminal::mark_dirty();
    with_compositor(|comp| {
        if let Some(fid) = comp.focused {
            if let Some(win) = comp.window_mut(fid) { win.dirty = true; }
        }
    });
    render_damaged();
}

fn update_terminal_selection(mx: i32, my: i32, lmb: bool, was: bool) {
    if terminal::selection_dragging() {
        let changed = if lmb {
            terminal::selection_extend(mx, my)
        } else {
            terminal::selection_end()
        };
        if changed {
            refresh_focused_terminal();
        }
        return;
    }
    if lmb && !was {
        if let Some(hit) = with_compositor(|c| c.scroll_hit_at(mx, my)).flatten() {
            if hit.is_terminal {
                let (rx, ry, rw, rh) = hit.text_rect;
                if mx >= rx && mx < rx + rw as i32 && my >= ry && my < ry + rh as i32 {
                    // Collapsed selection draws nothing; the click's focus
                    // redraw covers the (empty) initial state.
                    terminal::selection_begin(hit.term_idx as usize, rx, ry, rw, rh, mx, my);
                }
            }
        }
    }
}

/// Widget text drag-selection for the focused Input/TextArea. Mirrors
/// `update_terminal_selection`; the widget helpers re-render themselves.
/// Runs AFTER the compositor's button block so `press_at` has already moved
/// focus onto the clicked text widget.
fn update_widget_text_selection(mx: i32, my: i32, lmb: bool, was: bool) {
    let wid = match focused_widget_id() {
        Some(w) => w,
        None => { widgets::text_select_cancel(); return; }
    };
    if widgets::text_selecting() {
        if lmb { widgets::text_select_extend(wid, mx, my); }
        else   { widgets::text_select_end(wid); }
        return;
    }
    if lmb && !was {
        widgets::text_select_begin(wid, mx, my);
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
        ("shade.gaps", "6", "Gap between tiled windows (px at 1x)"),
        ("shade.border", "1", "Window border width (px at 1x)"),
        ("shade.border_active", "", "Active window border color (hex; empty = palette AccentLine)"),
        ("shade.border_inactive", "", "Inactive window border color (hex; empty = palette Border)"),
        ("shade.bar_margin", "6", "Bar strut gap to screen edge (px); bar.wasm reports its own height"),
        ("shade.rounding", "10", "Window corner radius (px at 1x)"),
        ("shade.opacity", "160", "Window background opacity (0-256, lower=more transparent)"),
        ("shade.light_tint", "70", "Darken glass on bright wallpapers (0-100, light mode; higher=darker)"),
        ("shade.chrome_opacity", "", "Bar/dock panel opacity (0-255; empty = 235 dark / 205 light)"),
        ("shade.bar_opacity", "", "Top-bar opacity (0-255; empty = shade.chrome_opacity)"),
        ("shade.dock_opacity", "", "Dock opacity (0-255; empty = shade.chrome_opacity)"),
        ("shade.flash_opacity", "", "Screenshot dim strength (0-255, 0 = off; empty = 110)"),
    ]
}
