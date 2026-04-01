//! Graphical login screen.
//!
//! Centered card with passphrase input, cursor blink, error/backoff states.
//! Replaces the text-mode auth prompt in main.rs.

use alloc::format;
use super::{background, color::Theme, font, render};
use crate::framebuffer::{self, FbInfo};

/// Layout computed from screen dimensions.
struct Layout {
    scale: u32,
    card_x: u32,
    card_y: u32,
    card_w: u32,
    card_h: u32,
    title_y: u32,
    subtitle_y: u32,
    input_x: u32,
    input_y: u32,
    input_w: u32,
    input_h: u32,
    status_y: u32,
    padding: u32,
}

impl Layout {
    fn compute(sw: u32, sh: u32, scale: u32) -> Self {
        let padding = 24 * scale;
        let card_w = (sw * 40 / 100).max(400 * scale).min(600 * scale);
        let card_h = 260 * scale;
        let card_x = (sw.saturating_sub(card_w)) / 2;
        let card_y = (sh.saturating_sub(card_h)) / 2;

        let title_y = card_y + padding;
        let subtitle_y = title_y + 32 * scale;
        let input_y = subtitle_y + 48 * scale;
        let input_x = card_x + padding;
        let input_w = card_w - 2 * padding;
        let input_h = 36 * scale;
        let status_y = input_y + input_h + 16 * scale;

        Layout {
            scale, card_x, card_y, card_w, card_h,
            title_y, subtitle_y,
            input_x, input_y, input_w, input_h,
            status_y, padding,
        }
    }
}

/// Draw the procedural aurora background.
fn draw_background(shadow: *mut u8, info: &FbInfo) {
    background::draw_aurora(shadow, info);
}

/// Draw the card (semi-transparent rounded rectangle with rounded border).
fn draw_card(shadow: *mut u8, info: &FbInfo, l: &Layout) {
    let radius = 16 * l.scale;
    let border = 1 * l.scale;
    // Border: slightly larger rounded rect, ~60% opaque
    render::fill_rounded_rect_blend(shadow, info,
        l.card_x, l.card_y, l.card_w, l.card_h,
        Theme::BORDER_CARD, radius, 160);
    // Fill: inset, ~70% opaque (background shines through)
    render::fill_rounded_rect_blend(shadow, info,
        l.card_x + border, l.card_y + border,
        l.card_w - 2 * border, l.card_h - 2 * border,
        Theme::BG_CARD, radius - border, 180);
}

/// Draw title and subtitle text.
fn draw_title(shadow: *mut u8, info: &FbInfo, l: &Layout) {
    font::draw_str_centered(shadow, info,
        "nopeekOS", l.card_x, l.card_w, l.title_y,
        Theme::FG_PRIMARY, None, l.scale);
    font::draw_str_centered(shadow, info,
        "Identity required. Your passphrase IS your identity.",
        l.card_x, l.card_w, l.subtitle_y,
        Theme::FG_SECONDARY, None, l.scale);
}

/// Draw the input field background and border.
fn draw_input_box(shadow: *mut u8, info: &FbInfo, l: &Layout, focused: bool) {
    render::fill_rect(shadow, info,
        l.input_x, l.input_y, l.input_w, l.input_h,
        Theme::BG_INPUT);
    let border_color = if focused { Theme::BORDER_FOCUS } else { Theme::BORDER_INPUT };
    render::draw_border(shadow, info,
        l.input_x, l.input_y, l.input_w, l.input_h,
        border_color, 1);
}

/// Draw passphrase dots inside the input field.
fn draw_dots(shadow: *mut u8, info: &FbInfo, l: &Layout, count: usize) {
    let dot_r = 3 * l.scale;
    let dot_spacing = 10 * l.scale;
    let start_x = l.input_x + 8 * l.scale;
    let center_y = l.input_y + l.input_h / 2;

    // Clear the input interior first
    render::fill_rect(shadow, info,
        l.input_x + 1, l.input_y + 1,
        l.input_w - 2, l.input_h - 2,
        Theme::BG_INPUT);

    for i in 0..count {
        let cx = start_x + i as u32 * dot_spacing + dot_r;
        if cx + dot_r >= l.input_x + l.input_w { break; }
        // Draw filled circle (dot)
        let r2 = (dot_r * dot_r) as i32;
        for dy in 0..dot_r * 2 {
            for dx in 0..dot_r * 2 {
                let ddx = dx as i32 - dot_r as i32;
                let ddy = dy as i32 - dot_r as i32;
                if ddx * ddx + ddy * ddy <= r2 {
                    render::put_pixel(shadow, info,
                        cx - dot_r + dx, center_y - dot_r + dy,
                        Theme::FG_DOT);
                }
            }
        }
    }
}

/// Draw or hide the blinking cursor.
fn draw_cursor(shadow: *mut u8, info: &FbInfo, l: &Layout, pos: usize, visible: bool) {
    let dot_spacing = 10 * l.scale;
    let cursor_x = l.input_x + 8 * l.scale + pos as u32 * dot_spacing;
    let cursor_y = l.input_y + 4 * l.scale;
    let cursor_w = 2 * l.scale;
    let cursor_h = l.input_h - 8 * l.scale;
    let color = if visible { Theme::CURSOR } else { Theme::BG_INPUT };
    render::fill_rect(shadow, info, cursor_x, cursor_y, cursor_w, cursor_h, color);
}

/// Draw status message below input field.
fn draw_status(shadow: *mut u8, info: &FbInfo, l: &Layout, msg: &str, color: u32) {
    // Clear status area
    render::fill_rect(shadow, info,
        l.card_x + l.padding, l.status_y,
        l.card_w - 2 * l.padding, 20 * l.scale,
        Theme::BG_CARD);
    font::draw_str_centered(shadow, info,
        msg, l.card_x, l.card_w, l.status_y,
        color, None, l.scale);
}

/// Run the graphical login screen.
/// Returns the 256-bit master key on success, or halts on lockout.
pub fn run(salt: &[u8; 16]) -> [u8; 32] {
    // Enable GUI mode (kprintln skips framebuffer, only serial)
    framebuffer::set_gui_mode(true);

    // Read screen dimensions
    let (screen_w, screen_h) = framebuffer::with_fb(|fb| {
        let info = fb.info();
        (info.width, info.height)
    }).unwrap_or((1024, 768));

    let scale = font::scale_for(screen_w);
    let layout = Layout::compute(screen_w, screen_h, scale);

    // Initial full draw
    framebuffer::with_fb(|fb| {
        let info = fb.info();
        let (shadow, _) = fb.shadow_ptr();
        draw_background(shadow, info);
        draw_card(shadow, info, &layout);
        draw_title(shadow, info, &layout);
        draw_input_box(shadow, info, &layout, true);
        draw_cursor(shadow, info, &layout, 0, true);
        // Full blit
        let mut damage = render::DamageTracker::new(info.width, info.height);
        damage.mark_all();
        damage.flush(fb);
    });

    // Input loop
    let mut passphrase = [0u8; 128];
    let mut pos: usize = 0;
    let mut attempts: u32 = 0;
    let mut cursor_visible = true;
    let mut last_cursor_toggle = crate::interrupts::ticks();

    loop {
        // Cursor blink (toggle every 50 ticks = 500ms at 100Hz)
        let now = crate::interrupts::ticks();
        if now.wrapping_sub(last_cursor_toggle) >= 50 {
            cursor_visible = !cursor_visible;
            last_cursor_toggle = now;
            framebuffer::with_fb(|fb| {
                let info = fb.info();
                let (shadow, _) = fb.shadow_ptr();
                draw_cursor(shadow, info, &layout, pos, cursor_visible);
                framebuffer::blit_rect(fb,
                    layout.input_x, layout.input_y,
                    layout.input_w, layout.input_h);
            });
        }

        // Poll network while waiting
        crate::net::poll();

        // Poll keyboard (non-blocking)
        if let Some(key) = crate::keyboard::read_key() {
            match key {
                b'\r' | b'\n' => {
                    if pos == 0 { continue; }

                    // Show verifying state
                    framebuffer::with_fb(|fb| {
                        let info = fb.info();
                        let (shadow, _) = fb.shadow_ptr();
                        draw_status(shadow, info, &layout, "Verifying...", Theme::FG_SECONDARY);
                        framebuffer::blit_rect(fb,
                            layout.card_x, layout.status_y,
                            layout.card_w, 20 * layout.scale);
                    });

                    // Derive key (OUTSIDE fb lock)
                    let key = crate::crypto::derive_master_key(&passphrase[..pos], salt);
                    for b in passphrase.iter_mut() { *b = 0; }
                    pos = 0;

                    crate::crypto::set_master_key(key);

                    match crate::npkfs::fetch(".npk-keycheck") {
                        Ok((data, _)) if &data[..] == b"nopeekOS.keycheck.v1.valid" => {
                            // Success!
                            crate::config::load();
                            let name = crate::config::get("name");
                            let msg = match &name {
                                Some(n) => format!("Welcome back, {}.", n),
                                None => alloc::string::String::from("Identity verified."),
                            };

                            framebuffer::with_fb(|fb| {
                                let info = fb.info();
                                let (shadow, _) = fb.shadow_ptr();
                                draw_status(shadow, info, &layout, &msg, Theme::ACCENT_GREEN);
                                framebuffer::blit_rect(fb,
                                    layout.card_x, layout.status_y,
                                    layout.card_w, 20 * layout.scale);
                            });

                            // Brief pause to show welcome
                            let start = crate::interrupts::ticks();
                            while crate::interrupts::ticks().wrapping_sub(start) < 150 {
                                core::hint::spin_loop();
                            }

                            // Exit GUI mode, clear screen for intent loop
                            framebuffer::set_gui_mode(false);
                            framebuffer::clear();
                            return key;
                        }
                        _ => {
                            // Wrong passphrase
                            crate::crypto::clear_master_key();
                            attempts += 1;

                            if attempts >= 10 {
                                framebuffer::with_fb(|fb| {
                                    let info = fb.info();
                                    let (shadow, _) = fb.shadow_ptr();
                                    draw_status(shadow, info, &layout,
                                        "Too many attempts. System halted.",
                                        Theme::ACCENT_RED);
                                    let mut damage = render::DamageTracker::new(info.width, info.height);
                                    damage.mark_all();
                                    damage.flush(fb);
                                });
                                loop { unsafe { core::arch::asm!("cli; hlt"); } }
                            }

                            let delay_secs = 1u64 << attempts.min(5);
                            let delay_msg = format!("Wrong passphrase. Wait {}s...", delay_secs);

                            framebuffer::with_fb(|fb| {
                                let info = fb.info();
                                let (shadow, _) = fb.shadow_ptr();
                                draw_input_box(shadow, info, &layout, true);
                                draw_cursor(shadow, info, &layout, 0, true);
                                draw_status(shadow, info, &layout, &delay_msg, Theme::ACCENT_RED);
                                framebuffer::blit_rect(fb,
                                    layout.card_x, layout.input_y,
                                    layout.card_w, layout.card_h - (layout.input_y - layout.card_y));
                            });

                            // Wait for backoff
                            let start = crate::interrupts::ticks();
                            let delay_ticks = delay_secs * 100;
                            while crate::interrupts::ticks().wrapping_sub(start) < delay_ticks {
                                // Update countdown
                                let elapsed = crate::interrupts::ticks().wrapping_sub(start);
                                let remaining = (delay_ticks.saturating_sub(elapsed) + 99) / 100;
                                let cd_msg = format!("Wrong passphrase. Wait {}s...", remaining);
                                framebuffer::with_fb(|fb| {
                                    let info = fb.info();
                                    let (shadow, _) = fb.shadow_ptr();
                                    draw_status(shadow, info, &layout, &cd_msg, Theme::ACCENT_RED);
                                    framebuffer::blit_rect(fb,
                                        layout.card_x, layout.status_y,
                                        layout.card_w, 20 * layout.scale);
                                });
                                core::hint::spin_loop();
                            }

                            // Clear error, reset input
                            framebuffer::with_fb(|fb| {
                                let info = fb.info();
                                let (shadow, _) = fb.shadow_ptr();
                                draw_input_box(shadow, info, &layout, true);
                                draw_cursor(shadow, info, &layout, 0, true);
                                draw_status(shadow, info, &layout, "", Theme::BG_CARD);
                                framebuffer::blit_rect(fb,
                                    layout.card_x, layout.input_y,
                                    layout.card_w, layout.card_h - (layout.input_y - layout.card_y));
                            });

                            cursor_visible = true;
                            last_cursor_toggle = crate::interrupts::ticks();
                        }
                    }
                }

                0x08 | 0x7F => {
                    // Backspace
                    if pos > 0 {
                        pos -= 1;
                        passphrase[pos] = 0;
                        framebuffer::with_fb(|fb| {
                            let info = fb.info();
                            let (shadow, _) = fb.shadow_ptr();
                            draw_dots(shadow, info, &layout, pos);
                            draw_cursor(shadow, info, &layout, pos, true);
                            framebuffer::blit_rect(fb,
                                layout.input_x, layout.input_y,
                                layout.input_w, layout.input_h);
                        });
                        cursor_visible = true;
                        last_cursor_toggle = crate::interrupts::ticks();
                    }
                }

                b if b >= 0x20 && b < 0x7F => {
                    // Printable character
                    if pos < passphrase.len() {
                        passphrase[pos] = b;
                        pos += 1;
                        framebuffer::with_fb(|fb| {
                            let info = fb.info();
                            let (shadow, _) = fb.shadow_ptr();
                            draw_dots(shadow, info, &layout, pos);
                            draw_cursor(shadow, info, &layout, pos, true);
                            framebuffer::blit_rect(fb,
                                layout.input_x, layout.input_y,
                                layout.input_w, layout.input_h);
                        });
                        cursor_visible = true;
                        last_cursor_toggle = crate::interrupts::ticks();
                    }
                }

                _ => {}
            }
        } else {
            // No key — spin (xHCI needs active polling, no IRQ on UEFI-only systems)
            core::hint::spin_loop();
        }
    }
}
