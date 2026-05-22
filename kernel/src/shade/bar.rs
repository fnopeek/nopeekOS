//! ShadeBar — status bar for shade compositor.
//!
//! Waybar-inspired floating bar: three rounded, detached pill segments
//! (left = workspaces, center = window title, right = clock + power)
//! with the wallpaper showing through the gaps. Rendered natively in
//! the kernel.
//!
//! Pill backgrounds are a fixed dark tone (theme-independent) so the
//! light text on top stays readable regardless of wallpaper/palette.

use alloc::format;
use alloc::string::String;
use crate::framebuffer::FbInfo;
use crate::gui::{background, font, render};
use crate::shade::widgets::abi::IconId;

// ── Fixed palette (contrast-guaranteed, theme-independent) ────────────
/// Pill background — dark, opaque. Readable text on any wallpaper.
const PILL_BG: u32 = 0x0014_141C;
const WS_INACTIVE_BG: u32 = 0x002A_2A38;
const WS_INACTIVE_FG: u32 = 0x00C2_C7D2;
const WS_ACTIVE_FG: u32 = 0x00FF_FFFF;
const TITLE_FG: u32 = 0x00DC_E0EA;
const CLOCK_FG: u32 = 0x00FF_FFFF;
const POWER_FG: u32 = 0x00E2_6A72;
/// Band fallback fill when the BG layer is unavailable (legacy path).
const BAND_FALLBACK: u32 = 0x000A_0A0E;

/// Bar position on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarPosition {
    Top,
    Bottom,
}

/// ShadeBar state.
#[allow(dead_code)]
pub struct ShadeBar {
    pub position: BarPosition,
    /// Total reserved band height in px (margin + pill height).
    pub height: u32,
    /// Visible pill height in px (scaled).
    pub pill_h: u32,
    /// Gap between the pills and the screen edge (scaled).
    pub margin: u32,
    /// Scale factor (1x or 2x).
    pub scale: u32,
    /// Number of workspaces.
    pub workspace_count: u8,
    /// Currently active workspace (0-based).
    pub active_workspace: u8,
    /// Title of the focused window.
    pub focused_title: String,
    /// Whether bar needs redraw.
    pub dirty: bool,
}

#[allow(dead_code)]
impl ShadeBar {
    pub fn new(scale: u32) -> Self {
        let scale = scale.max(1);
        // Pill height: floor at 36px (1x) so the larger 16x32 bar font
        // fits with breathing room — a stale small config can't shrink
        // the redesigned bar below readable.
        let base = crate::config::get("shade.bar_height")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(36)
            .max(36);
        let margin = crate::config::get("shade.bar_margin")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(8);
        let position = match crate::config::get("shade.bar_position").as_deref() {
            Some("bottom") => BarPosition::Bottom,
            _ => BarPosition::Top,
        };

        let pill_h = base * scale;
        let margin = margin * scale;

        ShadeBar {
            position,
            height: margin + pill_h,
            pill_h,
            margin,
            scale,
            workspace_count: 4,
            active_workspace: 0,
            focused_title: String::new(),
            dirty: true,
        }
    }

    /// Bar text font scale — one step above the UI scale so FullHD
    /// (scale 1) uses the 16x32 font instead of the tiny 8x16.
    fn text_scale(&self) -> u32 {
        self.scale + 1
    }

    /// Y of the reserved band on screen.
    pub fn y(&self, screen_h: u32) -> u32 {
        match self.position {
            BarPosition::Top => 0,
            BarPosition::Bottom => screen_h.saturating_sub(self.height),
        }
    }

    /// Y of the visible pills (band inset by the edge margin).
    fn pill_top(&self, screen_h: u32) -> u32 {
        match self.position {
            BarPosition::Top => self.margin,
            BarPosition::Bottom => screen_h.saturating_sub(self.margin + self.pill_h),
        }
    }

    /// Usable area start Y (below/above the reserved band).
    pub fn workspace_y(&self) -> u32 {
        match self.position {
            BarPosition::Top => self.height,
            BarPosition::Bottom => 0,
        }
    }

    /// Usable area height (screen minus reserved band).
    pub fn workspace_height(&self, screen_h: u32) -> u32 {
        screen_h.saturating_sub(self.height)
    }

    /// Set active workspace.
    pub fn set_workspace(&mut self, ws: u8) {
        if ws != self.active_workspace {
            self.active_workspace = ws;
            self.dirty = true;
        }
    }

    /// Set focused window title.
    pub fn set_title(&mut self, title: &str) {
        if self.focused_title != title {
            self.focused_title = String::from(title);
            self.dirty = true;
        }
    }

    /// Render the bar onto the shadow buffer.
    pub fn render(&mut self, shadow: *mut u8, info: &FbInfo, screen_w: u32, screen_h: u32) {
        // Restore the wallpaper across the whole band first, so the gaps
        // around the floating pills show through and stale text clears
        // (the partial-render path doesn't pre-restore this region).
        self.restore_band(shadow, info, screen_w, screen_h);

        let pill_top = self.pill_top(screen_h);
        let radius = 14 * self.scale;
        let hpad = 10 * self.scale;
        let accent = background::accent_color();
        let tf = self.text_scale();
        let (cw, ch) = font::char_size(tf);
        let text_y = pill_top + (self.pill_h.saturating_sub(ch)) / 2;

        // ── Left pill: workspace indicators ──────────────────────────
        let btn_h = self.pill_h.saturating_sub(8 * self.scale);
        let btn_w = btn_h.max(cw + 14 * self.scale);
        let ws_gap = 6 * self.scale;
        let n = self.workspace_count as u32;
        let left_w = 2 * hpad
            + n * btn_w
            + n.saturating_sub(1) * ws_gap;
        let left_x = self.margin;
        render::fill_rounded_rect_aa(shadow, info, left_x, pill_top, left_w, self.pill_h, PILL_BG, radius);

        let btn_y = pill_top + (self.pill_h.saturating_sub(btn_h)) / 2;
        let mut bx = left_x + hpad;
        for i in 0..self.workspace_count {
            let active = i == self.active_workspace;
            let (bg, fg) = if active {
                (accent, WS_ACTIVE_FG)
            } else {
                (WS_INACTIVE_BG, WS_INACTIVE_FG)
            };
            render::fill_rounded_rect_aa(shadow, info, bx, btn_y, btn_w, btn_h, bg, 8 * self.scale);
            let num = format!("{}", i + 1);
            let tx = bx + (btn_w.saturating_sub(cw)) / 2;
            font::draw_str(shadow, info, &num, tx, text_y, fg, None, tf);
            bx += btn_w + ws_gap;
        }

        // ── Right pill: clock + power button ─────────────────────────
        let time_str = self.format_time();
        let clock_w = font::measure_str(&time_str, tf);
        let icon_sz = (self.pill_h * 11 / 20).max(16); // ~55% of pill height
        let cp_gap = 12 * self.scale;
        let right_w = 2 * hpad + clock_w + cp_gap + icon_sz;
        let right_x = screen_w.saturating_sub(self.margin + right_w);
        render::fill_rounded_rect_aa(shadow, info, right_x, pill_top, right_w, self.pill_h, PILL_BG, radius);

        let clock_x = right_x + hpad;
        font::draw_str(shadow, info, &time_str, clock_x, text_y, CLOCK_FG, None, tf);
        let icon_x = clock_x + clock_w + cp_gap;
        let icon_y = pill_top + (self.pill_h.saturating_sub(icon_sz)) / 2;
        self.draw_icon(shadow, info, IconId::Power, icon_sz, icon_x, icon_y, POWER_FG);

        // ── Center pill: focused window title (clamped, truncated) ────
        if !self.focused_title.is_empty() {
            let left_edge = left_x + left_w + 2 * self.margin;
            let right_edge = right_x.saturating_sub(2 * self.margin);
            let avail = right_edge.saturating_sub(left_edge);
            let inner_max = avail.saturating_sub(2 * hpad);
            if inner_max >= cw * 4 {
                let max_chars = (inner_max / cw.max(1)) as usize;
                let title = truncate_to(&self.focused_title, max_chars);
                let title_w = font::measure_str(&title, tf);
                let center_w = title_w + 2 * hpad;
                // Centre on screen, then clamp inside the available band.
                let mut center_x = (screen_w.saturating_sub(center_w)) / 2;
                if center_x < left_edge { center_x = left_edge; }
                if center_x + center_w > right_edge {
                    center_x = right_edge.saturating_sub(center_w);
                }
                render::fill_rounded_rect_aa(shadow, info, center_x, pill_top, center_w, self.pill_h, PILL_BG, radius);
                font::draw_str(shadow, info, &title, center_x + hpad, text_y, TITLE_FG, None, tf);
            }
        }

        self.dirty = false;
    }

    /// Restore the bar band from the background (wallpaper) layer so the
    /// gaps around the floating pills are transparent to the wallpaper.
    fn restore_band(&self, shadow: *mut u8, info: &FbInfo, screen_w: u32, screen_h: u32) {
        let bar_y = self.y(screen_h);
        if let Some((bg, _, _, _)) = crate::layers::buffer(crate::layers::LAYER_BG) {
            let pitch = info.pitch as usize;
            let y1 = (bar_y + self.height).min(info.height);
            let bytes = (screen_w.min(info.width) as usize) * 4;
            for row in bar_y..y1 {
                let off = row as usize * pitch;
                // SAFETY: bg and shadow are full-screen buffers of `pitch`
                // bytes per row; `off + bytes` stays within one row.
                unsafe {
                    core::ptr::copy_nonoverlapping(bg.add(off), shadow.add(off), bytes);
                }
            }
        } else {
            render::fill_rect(shadow, info, 0, bar_y, screen_w, self.height, BAND_FALLBACK);
        }
    }

    /// Blit an alpha-only atlas icon onto the shadow, tinted `color`.
    /// Nearest-neighbour scales the atlas glyph to `size` when needed.
    fn draw_icon(&self, shadow: *mut u8, info: &FbInfo, id: IconId,
                 size: u32, x: u32, y: u32, color: u32) {
        let (asz, alpha) = match crate::gui::icons::alpha_for(id, size as u16) {
            Some(v) => v,
            None => return,
        };
        let asz = asz as u32;
        if asz == 0 { return; }
        for row in 0..size {
            let sy = (row * asz / size).min(asz - 1);
            for col in 0..size {
                let sx = (col * asz / size).min(asz - 1);
                let a = alpha[(sy * asz + sx) as usize] as u32;
                if a > 0 {
                    // 255 → 256 so a fully-opaque texel writes the pure color.
                    render::blend_pixel(shadow, info, x + col, y + row, color, a + (a >> 7));
                }
            }
        }
    }

    /// Format current time for display.
    fn format_time(&self) -> String {
        let unix = crate::rtc::read_unix_time().unwrap_or(0);
        let tz_minutes = crate::config::timezone_offset_minutes();
        let local = unix as i64 + tz_minutes as i64 * 60;
        let secs_today = ((local % 86400) + 86400) % 86400;
        let hour = secs_today / 3600;
        let min = (secs_today % 3600) / 60;
        format!("{:02}:{:02}", hour, min)
    }
}

/// Truncate `s` to at most `max_chars` characters, appending an ASCII
/// ellipsis ("…" is outside the bitmap font's ASCII range) when cut.
fn truncate_to(s: &str, max_chars: usize) -> String {
    let len = s.chars().count();
    if len <= max_chars {
        return String::from(s);
    }
    if max_chars <= 3 {
        return s.chars().take(max_chars).collect();
    }
    let keep = max_chars - 3;
    let mut out: String = s.chars().take(keep).collect();
    out.push_str("...");
    out
}
