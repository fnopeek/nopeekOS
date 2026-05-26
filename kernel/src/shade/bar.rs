//! ShadeBar — status bar for shade compositor.
//!
//! Waybar-inspired floating bar: three rounded, detached pill segments
//! with the wallpaper showing through the gaps. Rendered natively.
//!
//!   left   = workspaces │ focused window title
//!   center = clock
//!   right  = status widgets (placeholders for now) + power button
//!
//! Pills + foregrounds follow the theme palette (same SurfaceElevated
//! tray + translucency as the dock), so light/dark mode applies here too.

use alloc::format;
use alloc::string::String;
use crate::framebuffer::FbInfo;
use crate::gui::{background, font, render};
use crate::shade::widgets::abi::{IconId, Token};
use crate::shade::widgets::palette;

// Power button stays a fixed red regardless of theme; band fallback only
// shows when there's no wallpaper layer to restore from.
const POWER_FG: u32 = 0x00E2_6A72;
const BAND_FALLBACK: u32 = 0x000A_0A0E;

/// Right-side status widgets. Placeholders until wired to real data /
/// dedicated atlas icons (LAN, volume, screenshot don't exist yet —
/// these reuse generic glyphs to show the layout).
const WIDGETS: &[IconId] = &[IconId::HardDrives, IconId::Monitor, IconId::Image];

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
        let base = crate::config::get("shade.bar_height")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(26)
            .max(22); // floor so the 8x16 bar font fits with padding
        let margin = crate::config::get("shade.bar_margin")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(6);
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

    /// Bar text font scale. 1x → 8x16, 2x → 16x32 (font caps there).
    fn text_scale(&self) -> u32 {
        self.scale
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

    pub fn set_workspace(&mut self, ws: u8) {
        if ws != self.active_workspace {
            self.active_workspace = ws;
            self.dirty = true;
        }
    }

    pub fn set_title(&mut self, title: &str) {
        if self.focused_title != title {
            self.focused_title = String::from(title);
            self.dirty = true;
        }
    }

    /// Render the bar onto the shadow buffer.
    pub fn render(&mut self, shadow: *mut u8, info: &FbInfo, screen_w: u32, screen_h: u32) {
        // Wallpaper shows through the gaps around the floating pills.
        self.restore_band(shadow, info, screen_w, screen_h);

        let pill_top = self.pill_top(screen_h);
        let radius = 10 * self.scale;
        let hpad = 8 * self.scale;
        let accent = background::accent_color();

        // Theme palette — same tokens + translucency as the dock tray.
        // Mask to RGB (resolve returns opaque 0xFF.. ARGB; the bar's
        // draw helpers use the convention of a 0 top byte).
        let pill_bg  = palette::resolve(Token::SurfaceElevated) & 0x00FF_FFFF;
        let pill_op  = palette::chrome_opacity();
        let fg       = palette::resolve(Token::OnSurface)       & 0x00FF_FFFF;
        let fg_muted = palette::resolve(Token::OnSurfaceMuted)  & 0x00FF_FFFF;
        let ws_bg    = palette::resolve(Token::SurfaceMuted)    & 0x00FF_FFFF;
        let on_accent = palette::resolve(Token::OnAccent)       & 0x00FF_FFFF;
        let sep      = palette::resolve(Token::Border)          & 0x00FF_FFFF;
        let tf = self.text_scale();
        let (cw, ch) = font::char_size(tf);
        let text_y = pill_top + (self.pill_h.saturating_sub(ch)) / 2;
        let icon_box = self.pill_h.saturating_sub(8 * self.scale);

        // ── Center pill: clock (anchored first — left/right clamp to it)
        let time_str = self.format_time();
        let clock_w = font::measure_str(&time_str, tf);
        let center_w = clock_w + 2 * hpad;
        let center_x = (screen_w.saturating_sub(center_w)) / 2;
        render::fill_rounded_rect_alpha(shadow, info, center_x, pill_top, center_w, self.pill_h, pill_bg, radius, pill_op);
        font::draw_str(shadow, info, &time_str, center_x + hpad, text_y, fg, None, tf);

        // ── Left pill: workspaces │ title ────────────────────────────
        let btn_h = self.pill_h.saturating_sub(8 * self.scale);
        let btn_w = btn_h.max(cw + 10 * self.scale);
        let ws_gap = 4 * self.scale;
        let n = self.workspace_count as u32;
        let ws_total = n * btn_w + n.saturating_sub(1) * ws_gap;
        let sep_pad = 8 * self.scale;
        let sep_w = self.scale.max(1);
        let left_x = self.margin;

        // Title space = up to the center pill's left edge.
        let has_title = !self.focused_title.is_empty();
        let left_fixed = hpad + ws_total
            + if has_title { sep_pad + sep_w + sep_pad } else { 0 };
        let title_avail = center_x
            .saturating_sub(self.margin)
            .saturating_sub(left_x + left_fixed + hpad);
        let title = if has_title && title_avail >= cw * 3 {
            truncate_to(&self.focused_title, (title_avail / cw.max(1)) as usize)
        } else {
            String::new()
        };
        let title_w = if title.is_empty() { 0 } else { font::measure_str(&title, tf) };
        let left_w = if title.is_empty() {
            hpad + ws_total + hpad
        } else {
            left_fixed + title_w + hpad
        };

        render::fill_rounded_rect_alpha(shadow, info, left_x, pill_top, left_w, self.pill_h, pill_bg, radius, pill_op);

        let btn_y = pill_top + (self.pill_h.saturating_sub(btn_h)) / 2;
        let mut x = left_x + hpad;
        for i in 0..self.workspace_count {
            let active = i == self.active_workspace;
            let (btn_bg, btn_fg) = if active { (accent, on_accent) } else { (ws_bg, fg_muted) };
            render::fill_rounded_rect_aa(shadow, info, x, btn_y, btn_w, btn_h, btn_bg, 6 * self.scale);
            let num = format!("{}", i + 1);
            font::draw_str(shadow, info, &num, x + (btn_w.saturating_sub(cw)) / 2, text_y, btn_fg, None, tf);
            x += btn_w + ws_gap;
        }
        if !title.is_empty() {
            x = x - ws_gap + sep_pad;
            render::fill_rect(shadow, info, x, btn_y, sep_w, btn_h, sep);
            x += sep_w + sep_pad;
            font::draw_str(shadow, info, &title, x, text_y, fg, None, tf);
        }

        // ── Right pill: status widgets (placeholder) + power ─────────
        let mod_gap = 8 * self.scale;
        let slots = WIDGETS.len() as u32 + 1; // + power
        let right_w = 2 * hpad + slots * icon_box + (slots - 1) * mod_gap;
        let right_x = screen_w.saturating_sub(self.margin + right_w);
        render::fill_rounded_rect_alpha(shadow, info, right_x, pill_top, right_w, self.pill_h, pill_bg, radius, pill_op);

        let icon_y = pill_top + (self.pill_h.saturating_sub(icon_box)) / 2;
        let mut ix = right_x + hpad;
        for &id in WIDGETS {
            self.draw_icon(shadow, info, id, ix, icon_y, icon_box, fg_muted);
            ix += icon_box + mod_gap;
        }
        self.draw_icon(shadow, info, IconId::Power, ix, icon_y, icon_box, POWER_FG);

        self.dirty = false;
    }

    /// Restore the bar band from the background (wallpaper) layer.
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

    /// Blit an alpha-only atlas icon, centered in a `box`×`box` cell,
    /// tinted `color`. Renders at the icon's native atlas size (no
    /// rescale — nearest-neighbour mangles thin strokes), so the cell
    /// must be at least the requested 16px·scale.
    fn draw_icon(&self, shadow: *mut u8, info: &FbInfo, id: IconId,
                 x: u32, y: u32, cell: u32, color: u32) {
        let req = 16 * self.scale;
        let (asz, alpha) = match crate::gui::icons::alpha_for(id, req as u16) {
            Some(v) => v,
            None => return,
        };
        let asz = asz as u32;
        if asz == 0 || alpha.len() < (asz * asz) as usize { return; }
        let ox = x + cell.saturating_sub(asz) / 2;
        let oy = y + cell.saturating_sub(asz) / 2;
        for row in 0..asz {
            for col in 0..asz {
                let a = alpha[(row * asz + col) as usize] as u32;
                if a > 0 {
                    // 255 → 256 so a fully-opaque texel writes pure color.
                    render::blend_pixel(shadow, info, ox + col, oy + row, color, a + (a >> 7));
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
