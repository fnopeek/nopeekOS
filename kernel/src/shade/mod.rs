//! Shade — nopeekOS Compositor
//!
//! Native Rust compositor layer. Manages windows, Z-order, damage tracking,
//! and the shadebar (status bar). WASM modules control layout logic via
//! host functions; pixel rendering stays in native kernel code.
//!
//! Architecture:
//!   WASM WM (layout, focus, keybinds) → Host Functions → Compositor → Framebuffer

pub mod window;
pub mod bar;
pub mod compositor;

use alloc::string::String;
use spin::Mutex;

use crate::framebuffer::{self, FbInfo};
use crate::gui::{background, color::Theme, font, render};

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

/// Get shade config defaults.
pub fn default_config() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("shade.gaps", "8", "Gap between tiled windows (px at 1x)"),
        ("shade.border", "2", "Window border width (px at 1x)"),
        ("shade.border_active", "", "Active window border color (hex, default: accent)"),
        ("shade.border_inactive", "3a2555", "Inactive window border color (hex)"),
        ("shade.bar_height", "28", "Shadebar height (px at 1x)"),
        ("shade.bar_position", "top", "Shadebar position (top/bottom)"),
        ("shade.font_size", "1", "Font scale (1=8x16, 2=16x32)"),
        ("shade.animation", "true", "Enable window animations"),
    ]
}
