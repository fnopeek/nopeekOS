//! Graphical login screen (Hyprlock-inspired).
//!
//! Aurora background, large centered clock, greeting, light input field
//! with centered dots. No card container — clean, minimal.

use alloc::format;
use super::{background, color::Theme, font, render};
use crate::framebuffer::{self, FbInfo};

/// Layout computed from screen dimensions.
struct Layout {
    scale: u32,
    screen_w: u32,
    screen_h: u32,
    // Vertical positions (all centered horizontally)
    clock_y: u32,
    greeting_y: u32,
    input_x: u32,
    input_y: u32,
    input_w: u32,
    input_h: u32,
    status_y: u32,
}

impl Layout {
    fn compute(sw: u32, sh: u32, scale: u32) -> Self {
        let input_w = 300 * scale;
        let input_h = 40 * scale;
        let input_x = (sw.saturating_sub(input_w)) / 2;

        // Center vertically with clock above, input in middle
        let center_y = sh / 2;
        let input_y = center_y - input_h / 2 - 10 * scale;
        let greeting_y = input_y - 40 * scale;
        let clock_y = greeting_y - 70 * scale;
        let status_y = input_y + input_h + 16 * scale;

        Layout {
            scale, screen_w: sw, screen_h: sh,
            clock_y, greeting_y,
            input_x, input_y, input_w, input_h,
            status_y,
        }
    }
}

/// Draw the procedural aurora background.
fn draw_background(shadow: *mut u8, info: &FbInfo) {
    background::draw_aurora(shadow, info);
}

/// Draw the large clock display.
fn draw_clock(shadow: *mut u8, info: &FbInfo, l: &Layout) {
    let unix = crate::rtc::read_unix_time().unwrap_or(0);
    let tz_offset: i64 = crate::config::get("timezone")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let local = unix as i64 + tz_offset * 3600;
    let secs_today = ((local % 86400) + 86400) % 86400;
    let hour = secs_today / 3600;
    let min = (secs_today % 3600) / 60;
    let time_str = format!("{:02}:{:02}", hour, min);

    // Large clock using dedicated clock font (32x64 or 16x32)
    font::draw_clock_str_centered(shadow, info,
        &time_str, 0, l.screen_w, l.clock_y,
        Theme::FG_PRIMARY, None, l.scale);
}

/// Draw the greeting text.
fn draw_greeting(shadow: *mut u8, info: &FbInfo, l: &Layout) {
    // Before auth we don't have the name yet, show generic greeting
    font::draw_str_centered(shadow, info,
        "Enter Password...", 0, l.screen_w, l.greeting_y,
        Theme::FG_SECONDARY, None, l.scale);
}

/// Draw greeting with name (after config is loaded on success).
fn draw_greeting_name(shadow: *mut u8, info: &FbInfo, l: &Layout, name: &str) {
    let msg = format!("Hi {}", name);
    // Clear greeting area
    clear_greeting_area(shadow, info, l);
    font::draw_str_centered(shadow, info,
        &msg, 0, l.screen_w, l.greeting_y,
        Theme::FG_PRIMARY, None, l.scale);
}

fn clear_greeting_area(shadow: *mut u8, info: &FbInfo, l: &Layout) {
    let (_, ch) = font::char_size(l.scale);
    let h = ch + 4 * l.scale;
    background::draw_aurora_region(shadow, info, 0, l.greeting_y, info.width, h);
}

/// Draw the input field (light background, dark outline, rounded).
fn draw_input_box(shadow: *mut u8, info: &FbInfo, l: &Layout, focused: bool) {
    let radius = 12 * l.scale; // Moderate rounding (not full pill)
    let outline = 2 * l.scale;

    // Outer border
    let border_color = if focused { Theme::BORDER_FOCUS } else { Theme::INPUT_OUTER };
    render::fill_rounded_rect_aa(shadow, info,
        l.input_x, l.input_y, l.input_w, l.input_h,
        border_color, radius);

    // Inner fill (light)
    render::fill_rounded_rect_aa(shadow, info,
        l.input_x + outline, l.input_y + outline,
        l.input_w - 2 * outline, l.input_h - 2 * outline,
        Theme::INPUT_INNER, radius.saturating_sub(outline));
}

/// Draw passphrase dots centered inside the input field.
fn draw_dots(shadow: *mut u8, info: &FbInfo, l: &Layout, count: usize) {
    let outline = 2 * l.scale;
    let radius = 12 * l.scale;

    // Clear input interior (redraw inner fill)
    render::fill_rounded_rect_aa(shadow, info,
        l.input_x + outline, l.input_y + outline,
        l.input_w - 2 * outline, l.input_h - 2 * outline,
        Theme::INPUT_INNER, radius.saturating_sub(outline));

    if count == 0 { return; }

    // Small dots: radius = 3px at 1080p, 6px at 4K
    let dot_r = 3 * l.scale;
    let dot_gap = 6 * l.scale; // gap between dots
    let dot_diameter = dot_r * 2;
    let total_w = count as u32 * dot_diameter + count.saturating_sub(1) as u32 * dot_gap;

    // Clamp: don't let dots exceed field width
    let usable_w = l.input_w - 2 * (outline + radius / 2);
    let max_visible = (usable_w + dot_gap) / (dot_diameter + dot_gap);
    let visible = (count as u32).min(max_visible);

    // Center dots horizontally in input field
    let vis_w = visible * dot_diameter + visible.saturating_sub(1) * dot_gap;
    let start_x = l.input_x + (l.input_w.saturating_sub(vis_w)) / 2;
    let center_y = l.input_y + l.input_h / 2;

    for i in 0..visible as usize {
        let cx = start_x + i as u32 * (dot_diameter + dot_gap) + dot_r;
        // Draw filled circle
        let r2 = (dot_r * dot_r) as i32;
        for dy in 0..dot_r * 2 {
            for dx in 0..dot_r * 2 {
                let ddx = dx as i32 - dot_r as i32;
                let ddy = dy as i32 - dot_r as i32;
                if ddx * ddx + ddy * ddy <= r2 {
                    render::put_pixel(shadow, info,
                        cx - dot_r + dx, center_y - dot_r + dy,
                        Theme::INPUT_DOT);
                }
            }
        }
    }
}

/// Draw or hide the blinking cursor.
fn draw_cursor(shadow: *mut u8, info: &FbInfo, l: &Layout, pos: usize, visible: bool) {
    let dot_r = 3 * l.scale;
    let dot_gap = 6 * l.scale;
    let dot_diameter = dot_r * 2;

    // Cursor position: after last dot (or center if empty)
    let cursor_x = if pos == 0 {
        l.input_x + l.input_w / 2
    } else {
        let vis_w = pos as u32 * dot_diameter + pos.saturating_sub(1) as u32 * dot_gap;
        let start_x = l.input_x + (l.input_w.saturating_sub(vis_w)) / 2;
        start_x + pos as u32 * (dot_diameter + dot_gap) + l.scale
    };
    let cursor_y = l.input_y + l.input_h / 4;
    let cursor_w = 2 * l.scale;
    let cursor_h = l.input_h / 2;
    let color = if visible { Theme::INPUT_DOT } else { Theme::INPUT_INNER };
    render::fill_rect(shadow, info, cursor_x, cursor_y, cursor_w, cursor_h, color);
}

/// Draw status message below input field.
fn draw_status(shadow: *mut u8, info: &FbInfo, l: &Layout, msg: &str, color: u32) {
    // Clear status area (redraw aurora background strip)
    let (_, ch) = font::char_size(l.scale);
    background::draw_aurora_region(shadow, info, 0, l.status_y, l.screen_w, ch + 4 * l.scale);
    font::draw_str_centered(shadow, info,
        msg, 0, l.screen_w, l.status_y,
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
        draw_clock(shadow, info, &layout);
        draw_greeting(shadow, info, &layout);
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
                            0, layout.status_y,
                            layout.screen_w, 24 * layout.scale);
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
                            let welcome = match &name {
                                Some(n) => format!("Welcome back, {}.", n),
                                None => alloc::string::String::from("Identity verified."),
                            };

                            framebuffer::with_fb(|fb| {
                                let info = fb.info();
                                let (shadow, _) = fb.shadow_ptr();
                                // Show personalized greeting
                                if let Some(n) = &name {
                                    draw_greeting_name(shadow, info, &layout, n);
                                }
                                draw_status(shadow, info, &layout, &welcome, Theme::ACCENT_GREEN);
                                // Blit greeting + status area
                                framebuffer::blit_rect(fb,
                                    0, layout.greeting_y,
                                    layout.screen_w,
                                    layout.status_y + 24 * layout.scale - layout.greeting_y);
                            });

                            // Brief pause to show welcome
                            let start = crate::interrupts::ticks();
                            while crate::interrupts::ticks().wrapping_sub(start) < 150 {
                                core::hint::spin_loop();
                            }

                            // Exit GUI mode, clear screen for loop
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
                            let fail_msg = format!("Wrong passphrase ({}/10). Wait {}s...", attempts, delay_secs);

                            // Change input border to fail color
                            framebuffer::with_fb(|fb| {
                                let info = fb.info();
                                let (shadow, _) = fb.shadow_ptr();
                                // Redraw input with fail color border
                                let radius = layout.input_h / 2;
                                render::fill_rounded_rect_aa(shadow, info,
                                    layout.input_x, layout.input_y,
                                    layout.input_w, layout.input_h,
                                    Theme::FAIL_COLOR, radius);
                                let outline = 3 * layout.scale;
                                render::fill_rounded_rect_aa(shadow, info,
                                    layout.input_x + outline, layout.input_y + outline,
                                    layout.input_w - 2 * outline, layout.input_h - 2 * outline,
                                    Theme::INPUT_INNER, radius - outline);
                                draw_status(shadow, info, &layout, &fail_msg, Theme::FAIL_COLOR);
                                framebuffer::blit_rect(fb,
                                    0, layout.input_y,
                                    layout.screen_w,
                                    layout.status_y + 24 * layout.scale - layout.input_y);
                            });

                            // Wait for backoff
                            let start = crate::interrupts::ticks();
                            let delay_ticks = delay_secs * 100;
                            while crate::interrupts::ticks().wrapping_sub(start) < delay_ticks {
                                let elapsed = crate::interrupts::ticks().wrapping_sub(start);
                                let remaining = (delay_ticks.saturating_sub(elapsed) + 99) / 100;
                                let cd_msg = format!("Wrong passphrase ({}/10). Wait {}s...", attempts, remaining);
                                framebuffer::with_fb(|fb| {
                                    let info = fb.info();
                                    let (shadow, _) = fb.shadow_ptr();
                                    draw_status(shadow, info, &layout, &cd_msg, Theme::FAIL_COLOR);
                                    framebuffer::blit_rect(fb,
                                        0, layout.status_y,
                                        layout.screen_w, 24 * layout.scale);
                                });
                                core::hint::spin_loop();
                            }

                            // Reset input field
                            framebuffer::with_fb(|fb| {
                                let info = fb.info();
                                let (shadow, _) = fb.shadow_ptr();
                                draw_input_box(shadow, info, &layout, true);
                                draw_cursor(shadow, info, &layout, 0, true);
                                draw_status(shadow, info, &layout, "", Theme::FG_SECONDARY);
                                framebuffer::blit_rect(fb,
                                    0, layout.input_y,
                                    layout.screen_w,
                                    layout.status_y + 24 * layout.scale - layout.input_y);
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
