//! Window controls: the pill of buttons the compositor draws at the right
//! end of a window's 36 px top bar (`docs/spec/UI_REFRESH.md` §4).
//!
//! The geometry is computed here once and used by both the scene and the
//! hit-test, so what is drawn and what is clickable always coincide. The
//! pill is painted after the window's content and clicks inside it never
//! reach the app.

use alloc::string::String;
use alloc::vec::Vec;

use super::scene::Rect;
use super::widgets::abi::IconId;
use super::widgets::scale;
use super::window::{Window, WindowKind, WindowState};

/// Edge of one square button at 1x.
pub const BOX: u32 = 26;
/// Space between two buttons at 1x.
pub const GAP: u32 = 2;
/// Height of the app bar the pill sits in at 1x; the pill is centred in it.
pub const BAND: u32 = 36;
/// Glyph size at 1x.
pub const GLYPH: u32 = 16;
/// Inset from the bar's top, bottom and right edge at 1x.
pub const INSET: u32 = (BAND - BOX) / 2;
/// Width an app bar keeps free at its right end at 1x: four buttons, three
/// gaps and the right inset. The SDK carries the same number
/// (`WINDOW_CONTROLS_W`).
pub const RESERVE_W: u32 = 4 * BOX + 3 * GAP + INSET;
/// Below this content width (1x) the pill collapses to `More` and `Close`.
const NARROW_W: u32 = 3 * RESERVE_W;
/// Pointer dwell before a hint shows, in 10 ms ticks.
pub const HINT_TICKS: u64 = 40;
/// Pointer dwell on the top screen row before a fullscreen window shows its
/// pill, in 10 ms ticks.
pub const EDGE_TICKS: u64 = 30;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctl {
    Fullscreen,
    Float,
    Split,
    Close,
    More,
}

/// `shade.controls`: how much of the pill is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    /// All four buttons, always.
    Full,
    /// Only Close; the pill opens while the pointer is over the corner.
    Compact,
    /// No buttons.
    Off,
}

pub fn level() -> Level {
    match crate::config::get("shade.controls").as_deref() {
        Some("compact") => Level::Compact,
        Some("none") | Some("off") => Level::Off,
        _ => Level::Full,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Button {
    pub ctl: Ctl,
    pub rect: Rect,
    pub enabled: bool,
}

/// The buttons of one window, right to left in `buttons[..n]`, and the
/// corner they live in (the reserve, the full bar height).
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub buttons: [Option<Button>; 4],
    pub corner: Rect,
}

impl Layout {
    pub fn iter(&self) -> impl Iterator<Item = &Button> {
        self.buttons.iter().flatten()
    }

    pub fn at(&self, x: i32, y: i32) -> Option<Button> {
        self.iter().copied().find(|b| contains(b.rect, x, y))
    }

    /// Bounding box of the buttons.
    pub fn pill(&self) -> Rect {
        self.iter().fold(Rect::new(0, 0, 0, 0), |r, b| r.union(&b.rect))
    }
}

pub fn contains(r: Rect, x: i32, y: i32) -> bool {
    x >= r.x as i32 && y >= r.y as i32 && x < r.x1() as i32 && y < r.y1() as i32
}

/// True for the windows that carry a pill in their own bar: tiles and
/// floating windows of our own apps and loop. Not the panel, overlays,
/// popups, guest surfaces (they draw their own) or a fullscreen window.
pub fn hosts(win: &Window) -> bool {
    !win.is_panel
        && !win.is_overlay
        && win.popup_of.is_none()
        && win.kind != WindowKind::Surface
        && win.state != WindowState::Fullscreen
}

/// Lay the pill out against the top-right corner `(right, top)` of a bar
/// whose content is `content_w` wide. `expanded` opens a compact pill.
pub fn layout(right: u32, top: u32, content_w: u32, level: Level, expanded: bool, split_ok: bool)
    -> Option<Layout>
{
    let bx = scale::len(BOX);
    let gap = scale::len(GAP);
    let inset = scale::len(INSET);
    if level == Level::Off || content_w < bx + inset * 2 { return None; }
    let narrow = content_w < scale::len(NARROW_W);
    let set: &[Ctl] = match (level, expanded, narrow) {
        (Level::Compact, false, _) => &[Ctl::Close],
        (_, _, true) => &[Ctl::Close, Ctl::More],
        _ => &[Ctl::Close, Ctl::Split, Ctl::Float, Ctl::Fullscreen],
    };
    let mut buttons = [None; 4];
    let mut x = right.saturating_sub(inset);
    for (i, ctl) in set.iter().enumerate() {
        x = x.saturating_sub(bx);
        let enabled = *ctl != Ctl::Split || split_ok;
        buttons[i] = Some(Button { ctl: *ctl, rect: Rect::new(x, top + inset, bx, bx), enabled });
        x = x.saturating_sub(gap);
    }
    let reserve = scale::len(RESERVE_W).min(content_w);
    let corner = Rect::new(right.saturating_sub(reserve), top, reserve, scale::len(BAND));
    Some(Layout { buttons, corner })
}

/// The glyph a button shows: Fullscreen and Float show the state they lead
/// to, Split the orientation it flips to.
pub fn icon(ctl: Ctl, state: WindowState, split_beside: bool) -> IconId {
    match ctl {
        Ctl::Fullscreen if state == WindowState::Fullscreen => IconId::ArrowsIn,
        Ctl::Fullscreen => IconId::ArrowsOut,
        Ctl::Float if state == WindowState::Floating => IconId::SquaresFour,
        Ctl::Float => IconId::Browsers,
        Ctl::Split if split_beside => IconId::SquareSplitVertical,
        Ctl::Split => IconId::SquareSplitHorizontal,
        Ctl::Close => IconId::X,
        Ctl::More => IconId::DotsThree,
    }
}

fn german() -> bool {
    crate::config::get("lang").is_some_and(|l| l.starts_with("de"))
}

/// What a button does, in the UI language.
pub fn label(ctl: Ctl, state: WindowState) -> &'static str {
    let de = german();
    match (ctl, state) {
        (Ctl::Fullscreen, WindowState::Fullscreen) => if de { "Vollbild verlassen" } else { "Leave full screen" },
        (Ctl::Fullscreen, _) => if de { "Vollbild" } else { "Full screen" },
        (Ctl::Float, WindowState::Floating) => if de { "Kacheln" } else { "Tile" },
        (Ctl::Float, _) => if de { "Schweben" } else { "Float" },
        (Ctl::Split, _) => if de { "Teilung umklappen" } else { "Flip split" },
        (Ctl::Close, _) => if de { "Schliessen" } else { "Close" },
        (Ctl::More, _) => if de { "Fenster" } else { "Window" },
    }
}

/// The key that does the same, spelled with the configured mod key
/// (`shade.mod`).
pub fn shortcut(ctl: Ctl) -> Option<String> {
    let key = match ctl {
        Ctl::Fullscreen => 'F',
        Ctl::Float => 'V',
        Ctl::Split => 'J',
        Ctl::Close => 'Q',
        Ctl::More => return None,
    };
    let m = match crate::config::get("shade.mod").as_deref() {
        Some("ctrl") => "Ctrl",
        Some("alt") => "Alt",
        _ => "Super",
    };
    Some(alloc::format!("{m}+{key}"))
}

/// Hint text for a hovered button: `Vollbild · Super+F`.
pub fn hint(ctl: Ctl, state: WindowState) -> String {
    match shortcut(ctl) {
        Some(k) => alloc::format!("{} · {}", label(ctl, state), k),
        None => String::from(label(ctl, state)),
    }
}

/// The rows of the `More` menu.
pub const MENU: [Ctl; 3] = [Ctl::Fullscreen, Ctl::Float, Ctl::Split];

/// Geometry of the `More` menu below its button: the menu box and one rect
/// per row, clamped to the screen.
pub fn menu_rects(anchor: Rect, screen_w: u32, screen_h: u32) -> (Rect, [Rect; 3]) {
    let row_h = scale::len(30);
    let pad = scale::len(4);
    let w = scale::len(230);
    let h = row_h * MENU.len() as u32 + pad * 2;
    let x = anchor.x1().saturating_sub(w).min(screen_w.saturating_sub(w));
    let y = (anchor.y1() + scale::len(4)).min(screen_h.saturating_sub(h));
    let rows = core::array::from_fn(|i| Rect::new(x + pad, y + pad + row_h * i as u32, w - pad * 2, row_h));
    (Rect::new(x, y, w, h), rows)
}

/// One line of text rendered for the scene: alpha in the top byte, `color`
/// below, so `Item::Text` blends it like a terminal's text layer.
pub fn render_text(s: &str, color: u32) -> super::buffers::PixelBuffer {
    use crate::gui::text;
    use super::widgets::abi::TextStyle;
    let style = TextStyle::Body;
    let size = scale::len(13).clamp(6, 64) as u16;
    let w = text::measure_px(s, style, size) as u32 + 2;
    let h = text::line_height_px(style, size) as u32 + 1;
    let mut buf = super::buffers::PixelBuffer::new(w, h);
    if buf.width() == 0 { return buf; }
    let ascent = text::ascent_px(style, size);
    let baseline = ascent as i32 + (if (ascent as i32 as f32) < ascent { 1 } else { 0 });
    let color = color & 0x00FF_FFFF;
    let mut pen = 0.0f32;
    let mut prev: Option<char> = None;
    let px: &mut [u32] = &mut buf;
    for ch in s.chars() {
        if let Some(p) = prev { pen += text::kern_px(p, ch, style, size); }
        let adv = text::rasterize_cached_px(ch, style, size, |g| {
            let gx = pen as i32 + g.xmin as i32;
            let gy = baseline - (g.height as i32 + g.ymin as i32);
            for row in 0..g.height as i32 {
                for col in 0..g.width as i32 {
                    let (x, y) = (gx + col, gy + row);
                    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 { continue; }
                    let a = g.alpha[(row * g.width as i32 + col) as usize] as u32;
                    let i = (y as u32 * w + x as u32) as usize;
                    if a > px[i] >> 24 { px[i] = (a << 24) | color; }
                }
            }
            g.advance
        }).unwrap_or(0.0);
        pen += adv;
        prev = Some(ch);
    }
    buf
}

/// The pointer's view of the controls, kept by the compositor.
#[derive(Default)]
pub struct State {
    /// Button under the pointer and the tick it got there.
    pub hover: Option<(super::WindowId, Ctl, u64)>,
    /// A compact pill opened by the pointer.
    pub expanded: Option<super::WindowId>,
    /// The window whose `More` menu is open, and the row under the pointer.
    pub menu: Option<super::WindowId>,
    pub menu_row: Option<usize>,
    /// The hint of the hovered button is showing.
    pub hint_shown: bool,
    /// Tick the pointer arrived at the top screen row over a fullscreen window.
    pub edge_since: Option<u64>,
    /// The fullscreen window whose pill is shown at the top edge.
    pub revealed: Option<super::WindowId>,
    /// Rendered text by (text, colour), with the id its `Source` carries.
    pub labels: Vec<(String, u32, u32, super::buffers::PixelBuffer)>,
    pub next_label: u32,
    /// Labels the frame being built uses.
    pub used: Vec<u32>,
}

impl State {
    /// The id of the rendered `text` in `color`, rendering it once.
    pub fn label(&mut self, text: &str, color: u32) -> (u32, u32, u32) {
        if let Some(l) = self.labels.iter().find(|l| l.0 == text && l.1 == color) {
            self.used.push(l.2);
            return (l.2, l.3.width(), l.3.height());
        }
        let buf = render_text(text, color);
        let id = self.next_label;
        self.next_label = self.next_label.wrapping_add(1);
        let (w, h) = (buf.width(), buf.height());
        self.labels.push((String::from(text), color, id, buf));
        self.used.push(id);
        (id, w, h)
    }

    /// Start a frame: no label used yet.
    pub fn begin_frame(&mut self) {
        self.used.clear();
    }

    /// Drop rendered text the frame just built does not use.
    pub fn end_frame(&mut self) {
        let used = core::mem::take(&mut self.used);
        self.labels.retain(|l| used.contains(&l.2));
        self.used = used;
    }
}

use super::compositor::Compositor;
use super::scene::Item;
use super::widgets::abi::Token;
use super::WindowId;

/// Glyph opacity of an unfocused window's pill and of a disabled button.
const DIM_ALPHA: u32 = 129;
const DISABLED_ALPHA: u32 = 80;

fn token(win_kind: WindowKind, t: Token) -> u32 {
    let c = if win_kind == WindowKind::Terminal {
        super::widgets::palette::resolve_glass(t)
    } else {
        super::widgets::palette::resolve(t)
    };
    c & 0x00FF_FFFF
}

impl Compositor {
    /// The pill of `win` as it stands now.
    pub(super) fn controls_layout(&self, win: &Window) -> Option<Layout> {
        if !hosts(win) { return None; }
        let b = self.border;
        let expanded = self.controls.expanded == Some(win.id);
        let split_ok = win.state == WindowState::Tiled && self.split_line_owner(win.id).is_some();
        layout(win.x + win.width.saturating_sub(b), win.y + b, win.content_w(b), level(), expanded, split_ok)
    }

    /// The pill a fullscreen window shows at the top edge of the screen.
    fn revealed_layout(&self) -> Option<(WindowId, Layout)> {
        let id = self.controls.revealed?;
        let win = self.window(id)?;
        if win.state != WindowState::Fullscreen { return None; }
        layout(self.screen_w, 0, self.screen_w, Level::Full, true, false).map(|l| (id, l))
    }

    /// Orientation the split of `win` has now (true = side by side).
    fn split_beside_of(&self, win: &Window) -> bool {
        self.split_line_owner(win.id)
            .and_then(|o| self.window(o))
            .map(|o| o.split_beside)
            .unwrap_or(true)
    }

    /// Topmost layout under `(x, y)` (its corner, not only its buttons).
    fn controls_layout_at(&self, x: i32, y: i32) -> Option<(WindowId, Layout)> {
        if let Some((id, l)) = self.revealed_layout() {
            if contains(l.pill(), x, y) { return Some((id, l)); }
        }
        let top = self.window_at(x, y)?;
        let win = self.window(top)?;
        let l = self.controls_layout(win)?;
        if contains(l.corner, x, y) { Some((top, l)) } else { None }
    }

    /// The button under `(x, y)`, on the topmost window there.
    pub fn controls_at(&self, x: i32, y: i32) -> Option<(WindowId, Button)> {
        let (id, l) = self.controls_layout_at(x, y)?;
        l.at(x, y).map(|b| (id, b))
    }

    /// The corner of `win`'s pill, for the hit-tests that must leave it alone.
    pub fn controls_corner(&self, win: &Window) -> Option<Rect> {
        self.controls_layout(win).map(|l| l.corner)
    }

    /// The area a change to `id`'s controls repaints: its corner and the
    /// space below it where a hint or the menu opens.
    fn controls_zone(&self, id: WindowId) -> Option<Rect> {
        let corner = match self.revealed_layout() {
            Some((rid, l)) if rid == id => l.corner,
            _ => {
                let win = self.window(id)?;
                match self.controls_layout(win) {
                    Some(l) => l.corner,
                    None if win.state == WindowState::Fullscreen => {
                        layout(self.screen_w, 0, self.screen_w, Level::Full, true, false)?.corner
                    }
                    None => return None,
                }
            }
        };
        let w = scale::len(340);
        let x = corner.x1().saturating_sub(w);
        Some(Rect::new(x, corner.y, corner.x1() - x, corner.h + scale::len(130))
            .intersect(&Rect::new(0, 0, self.screen_w, self.screen_h)))
    }

    fn damage_controls(&mut self, ids: &[Option<WindowId>]) {
        for id in ids.iter().flatten() {
            if let Some(z) = self.controls_zone(*id) {
                if !z.is_empty() { self.extra_damage.push(z); }
            }
        }
    }

    /// The pointer moved to `(x, y)` at tick `now`. True when the controls
    /// changed and `extra_damage` holds what to repaint.
    pub fn controls_pointer(&mut self, x: i32, y: i32, now: u64) -> bool {
        let before = (self.controls.hover.map(|(i, c, _)| (i, c)), self.controls.expanded,
            self.controls.menu_row, self.controls.revealed);
        let mut touched: Vec<Option<WindowId>> = Vec::new();

        // A fullscreen window's pill comes in from the top screen row.
        let fs = self.active_fullscreen()
            .filter(|id| self.window(*id).is_some_and(|w| w.kind != WindowKind::Surface))
            .filter(|_| level() != Level::Off);
        match fs {
            Some(id) => {
                if y <= 0 {
                    self.controls.edge_since.get_or_insert(now);
                } else {
                    self.controls.edge_since = None;
                }
                if self.controls.revealed == Some(id) && self.controls.menu != Some(id) {
                    let keep = self.revealed_layout()
                        .is_some_and(|(_, l)| contains(l.pill().grow(scale::len(BAND)), x, y));
                    if !keep { self.controls.revealed = None; }
                }
            }
            None => {
                self.controls.edge_since = None;
                self.controls.revealed = None;
            }
        }

        let hit = self.controls_at(x, y).map(|(id, b)| (id, b.ctl));
        if hit != before.0 {
            self.controls.hover = hit.map(|(id, c)| (id, c, now));
            self.controls.hint_shown = false;
        }
        self.controls.expanded = if level() == Level::Compact {
            self.window_at(x, y)
                .and_then(|id| self.window(id).map(|w| (id, w)))
                .and_then(|(id, w)| {
                    let b = self.border;
                    let l = layout(w.x + w.width.saturating_sub(b), w.y + b, w.content_w(b), Level::Full,
                        true, false)?;
                    (hosts(w) && contains(l.corner, x, y)).then_some(id)
                })
        } else {
            None
        };
        if let Some(mid) = self.controls.menu {
            self.controls.menu_row = self.menu_geometry(mid)
                .and_then(|(_, rows)| rows.iter().position(|r| contains(*r, x, y)));
        }

        let after = (self.controls.hover.map(|(i, c, _)| (i, c)), self.controls.expanded,
            self.controls.menu_row, self.controls.revealed);
        if before == after { return false; }
        touched.extend([before.0.map(|h| h.0), after.0.map(|h| h.0), before.1, after.1, self.controls.menu,
            before.3, after.3]);
        self.damage_controls(&touched);
        true
    }

    /// Timers of the controls: the hint after a dwell, the fullscreen pill
    /// after the pointer rests on the top row. True when something changed.
    pub fn tick_controls(&mut self, now: u64) -> bool {
        let mut changed = Vec::new();
        if let Some((id, _, since)) = self.controls.hover {
            if !self.controls.hint_shown && now.saturating_sub(since) >= HINT_TICKS {
                self.controls.hint_shown = true;
                changed.push(Some(id));
            }
        }
        if let (Some(since), None) = (self.controls.edge_since, self.controls.revealed) {
            if now.saturating_sub(since) >= EDGE_TICKS {
                self.controls.revealed = self.active_fullscreen();
                changed.push(self.controls.revealed);
            }
        }
        if changed.is_empty() { return false; }
        self.damage_controls(&changed);
        true
    }

    /// A controls timer is running.
    pub fn controls_need_tick(&self) -> bool {
        (self.controls.hover.is_some() && !self.controls.hint_shown)
            || (self.controls.edge_since.is_some() && self.controls.revealed.is_none())
    }

    /// The `More` menu of `id`: its box and rows.
    fn menu_geometry(&self, id: WindowId) -> Option<(Rect, [Rect; 3])> {
        let anchor = match self.revealed_layout() {
            Some((rid, l)) if rid == id => l,
            _ => self.controls_layout(self.window(id)?)?,
        }.iter().find(|b| b.ctl == Ctl::More)?.rect;
        Some(menu_rects(anchor, self.screen_w, self.screen_h))
    }

    /// A left press at `(x, y)` on the controls or their menu. True when it
    /// was theirs; the app never sees it.
    pub fn controls_click(&mut self, x: i32, y: i32) -> bool {
        if let Some(mid) = self.controls.menu.take() {
            self.controls.menu_row = None;
            self.damage_controls(&[Some(mid)]);
            if let Some((_, rows)) = self.menu_geometry(mid) {
                if let Some(i) = rows.iter().position(|r| contains(*r, x, y)) {
                    let ctl = MENU[i];
                    let enabled = ctl != Ctl::Split || self.window(mid)
                        .is_some_and(|w| w.state == WindowState::Tiled && self.split_line_owner(mid).is_some());
                    if enabled { self.controls_act(mid, ctl); }
                    return true;
                }
            }
            if self.controls_at(x, y).is_some_and(|(id, b)| id == mid && b.ctl == Ctl::More) {
                return true;
            }
        }
        let Some((id, b)) = self.controls_at(x, y) else { return false };
        if b.enabled { self.controls_act(id, b.ctl); }
        true
    }

    fn controls_act(&mut self, id: WindowId, ctl: Ctl) {
        let zone = self.controls_zone(id);
        match ctl {
            Ctl::Close => {
                self.request_close_window(id);
            }
            Ctl::More => {
                self.controls.menu = Some(id);
                self.controls.menu_row = None;
            }
            Ctl::Fullscreen => {
                self.focus_window(id);
                self.toggle_fullscreen(id);
                self.controls.revealed = None;
                self.controls.edge_since = None;
            }
            Ctl::Float => {
                self.focus_window(id);
                self.toggle_floating(id);
            }
            Ctl::Split => {
                self.focus_window(id);
                self.toggle_split();
            }
        }
        self.controls.hover = None;
        if let Some(z) = zone { self.extra_damage.push(z); }
        self.damage_controls(&[Some(id)]);
        self.needs_full_redraw = true;
    }

    /// Mod+F and the Fullscreen button: fullscreen on or back to a tile.
    pub fn toggle_fullscreen(&mut self, id: WindowId) {
        if let Some(win) = self.window_mut(id) {
            win.state = match win.state {
                WindowState::Fullscreen => WindowState::Tiled,
                _ => WindowState::Fullscreen,
            };
        }
        self.retile();
    }

    /// Mod+V and the Float button: lifted off the tiling, or back into it.
    pub fn toggle_floating(&mut self, id: WindowId) {
        let lift = self.window(id).is_some_and(|w| w.state != WindowState::Floating);
        if let Some(win) = self.window_mut(id) {
            win.state = if lift { WindowState::Floating } else { WindowState::Tiled };
            win.restore = None;
        }
        if lift {
            self.detach_geometry(id);
            self.focus_window(id);
        }
        self.retile();
        self.needs_full_redraw = true;
    }

    /// Scene items of `win`'s pill, painted right after its content.
    pub(super) fn controls_items(&self, win: &Window, items: &mut Vec<Item>) {
        let Some(l) = self.controls_layout(win) else { return };
        self.pill_items(win, &l, items);
    }

    fn pill_items(&self, win: &Window, l: &Layout, items: &mut Vec<Item>) {
        let size = scale::len(GLYPH);
        let radius = scale::len(4);
        let muted = token(win.kind, Token::OnSurfaceMuted);
        let beside = self.split_beside_of(win);
        let over_window = self.controls.hover.is_some_and(|(id, _, _)| id == win.id)
            || self.controls.menu == Some(win.id);
        for b in l.iter() {
            let hovered = b.enabled && (self.controls.hover.is_some_and(|(id, c, _)| id == win.id && c == b.ctl)
                || (b.ctl == Ctl::More && self.controls.menu == Some(win.id)));
            let danger = hovered && b.ctl == Ctl::Close;
            if hovered {
                let (color, alpha) = if danger {
                    (token(win.kind, Token::Danger), 56)
                } else {
                    (token(win.kind, Token::SurfaceHover), 255)
                };
                items.push(Item::Fill { rect: b.rect, color, alpha, radius });
            }
            let alpha = if !b.enabled { DISABLED_ALPHA }
                else if win.focused || over_window || win.state == WindowState::Fullscreen { 255 }
                else { DIM_ALPHA };
            let color = if danger { token(win.kind, Token::Danger) } else { muted };
            items.push(Item::Glyph { rect: b.rect, icon: icon(b.ctl, win.state, beside), size, color, alpha });
        }
    }

    /// The controls drawn over everything: a fullscreen window's pill, the
    /// open menu and the hint. Labels the frame does not use are dropped.
    pub(super) fn controls_overlay(&mut self, items: &mut Vec<Item>) {
        use super::scene::Source;
        let screen = Rect::new(0, 0, self.screen_w, self.screen_h);

        if let Some((id, l)) = self.revealed_layout() {
            if let Some(win) = self.window(id) {
                let back = l.pill().grow(scale::len(4));
                items.push(Item::Fill { rect: back, color: token(WindowKind::Widget, Token::SurfaceElevated),
                    alpha: 240, radius: back.h / 2 });
                let mut pill = Vec::new();
                self.pill_items(win, &l, &mut pill);
                items.extend(pill);
            }
        }

        if let Some(mid) = self.controls.menu {
            if let (Some((menu, rows)), Some(win)) = (self.menu_geometry(mid), self.window(mid)) {
                let state = win.state;
                let split_ok = state == WindowState::Tiled && self.split_line_owner(mid).is_some();
                let beside = self.split_beside_of(win);
                let edge = token(WindowKind::Widget, Token::Border);
                let bg = token(WindowKind::Widget, Token::SurfaceElevated);
                let text = token(WindowKind::Widget, Token::OnSurface);
                let faint = token(WindowKind::Widget, Token::OnSurfaceFaint);
                let muted = token(WindowKind::Widget, Token::OnSurfaceMuted);
                let hover = token(WindowKind::Widget, Token::SurfaceHover);
                let r = scale::len(8);
                let one = scale::len(1);
                items.push(Item::Fill { rect: menu, color: edge, alpha: 255, radius: r });
                items.push(Item::Fill { rect: Rect::new(menu.x + one, menu.y + one, menu.w - 2 * one,
                    menu.h - 2 * one), color: bg, alpha: 255, radius: r.saturating_sub(one) });
                let pad = scale::len(8);
                let gsz = scale::len(GLYPH);
                for (i, row) in rows.iter().enumerate() {
                    let ctl = MENU[i];
                    let enabled = ctl != Ctl::Split || split_ok;
                    if enabled && self.controls.menu_row == Some(i) {
                        items.push(Item::Fill { rect: *row, color: hover, alpha: 255, radius: scale::len(4) });
                    }
                    let gr = Rect::new(row.x + pad, row.y + (row.h - gsz) / 2, gsz, gsz);
                    items.push(Item::Glyph { rect: gr, icon: icon(ctl, state, beside), size: gsz,
                        color: muted, alpha: if enabled { 255 } else { DISABLED_ALPHA } });
                    let (lid, lw, lh) = self.controls.label(label(ctl, state), if enabled { text } else { faint });
                    let lx = gr.x1() + scale::len(10);
                    let ly = row.y + row.h.saturating_sub(lh) / 2;
                    items.push(Item::Text { src: Source::Label(lid),
                        dst: Rect::new(lx, ly, lw, lh).intersect(row) });
                    if let Some(k) = shortcut(ctl) {
                        let (kid, kw, kh) = self.controls.label(&k, faint);
                        let kx = row.x1().saturating_sub(pad + kw).max(lx + lw);
                        items.push(Item::Text { src: Source::Label(kid),
                            dst: Rect::new(kx, row.y + row.h.saturating_sub(kh) / 2, kw, kh).intersect(row) });
                    }
                }
            }
        }

        let hinted = self.controls.hover.filter(|_| self.controls.hint_shown)
            .filter(|(id, _, _)| self.controls.menu != Some(*id));
        if let Some((id, ctl, _)) = hinted {
            let button = match self.revealed_layout() {
                Some((rid, l)) if rid == id => l.iter().find(|b| b.ctl == ctl).map(|b| b.rect),
                _ => self.window(id).and_then(|w| self.controls_layout(w))
                    .and_then(|l| l.iter().find(|b| b.ctl == ctl).map(|b| b.rect)),
            };
            if let (Some(br), Some(win)) = (button, self.window(id)) {
                let text = hint(ctl, win.state);
                let fg = token(WindowKind::Widget, Token::Surface);
                let bg = token(WindowKind::Widget, Token::OnSurface);
                let (lid, lw, lh) = self.controls.label(&text, fg);
                let (px, py) = (scale::len(8), scale::len(4));
                let w = lw + 2 * px;
                let h = lh + 2 * py;
                let x = br.x1().saturating_sub(w).min(self.screen_w.saturating_sub(w));
                let y = (br.y1() + scale::len(6)).min(self.screen_h.saturating_sub(h));
                let back = Rect::new(x, y, w, h).intersect(&screen);
                items.push(Item::Fill { rect: back, color: bg, alpha: 235, radius: scale::len(6) });
                items.push(Item::Text { src: Source::Label(lid),
                    dst: Rect::new(x + px, y + py, lw, lh).intersect(&screen) });
            }
        }
    }

    /// Pixels of a rendered label.
    pub(super) fn label_pixels(&self, id: u32) -> Option<&super::buffers::PixelBuffer> {
        self.controls.labels.iter().find(|l| l.2 == id).map(|l| &l.3)
    }

    /// Scene items of a `loop` window's top bar: what runs in it at the
    /// left, a hairline below. The pill sits at its right end.
    pub(super) fn loop_bar_items(&mut self, content: Rect, terminal_idx: u8, items: &mut Vec<Item>) {
        use super::scene::Source;
        let band = scale::len(BAND).min(content.h);
        if band == 0 || content.w == 0 { return; }
        let name = loop_bar_title(terminal_idx);
        let pad = scale::len(8);
        let gsz = scale::len(GLYPH);
        let gr = Rect::new(content.x + pad, content.y + (band - gsz.min(band)) / 2, gsz, gsz);
        items.push(Item::Glyph { rect: gr, icon: IconId::Terminal, size: gsz,
            color: token(WindowKind::Terminal, Token::Accent), alpha: 255 });
        let (lid, lw, lh) = self.controls.label(&name, token(WindowKind::Terminal, Token::OnSurfaceMuted));
        let lx = gr.x1() + pad;
        let room = content.x1().saturating_sub(lx + scale::len(RESERVE_W));
        items.push(Item::Text { src: Source::Label(lid),
            dst: Rect::new(lx, content.y + band.saturating_sub(lh) / 2, lw.min(room), lh) });
        items.push(Item::Fill { rect: Rect::new(content.x, content.y + band, content.w, scale::len(1)),
            color: token(WindowKind::Terminal, Token::Border), alpha: 160, radius: 0 });
    }
}

/// Height of a `loop` window's top bar, physical px.
pub fn loop_bar_h() -> u32 {
    scale::len(BAND)
}

/// The title of a `loop` bar: the module or intent running in it, else `loop`.
fn loop_bar_title(terminal_idx: u8) -> String {
    use crate::process::{newest_on_terminal, KIND_INTENT, KIND_WASM};
    let running = if crate::wasm::has_wasm_app(terminal_idx) {
        newest_on_terminal(terminal_idx, KIND_WASM)
    } else if crate::intent::has_running_intent(terminal_idx) {
        newest_on_terminal(terminal_idx, KIND_INTENT)
    } else {
        None
    };
    running.unwrap_or_else(|| String::from("loop"))
}

// Floating windows: lifted off the tiling, moved by their bar, resized at
// their edges.

/// Edge bits of a `DragMode::Edge`.
pub const EDGE_L: u8 = 1;
pub const EDGE_R: u8 = 2;
pub const EDGE_T: u8 = 4;
pub const EDGE_B: u8 = 8;

/// Two presses this close in time (10 ms ticks) are a double click.
const DOUBLE_CLICK_TICKS: u64 = 45;

/// Where a floating window lands when it is let go at a screen edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Snap {
    /// Top edge: the whole work area.
    Max,
    /// Left or right edge: that half, as a tile.
    Left,
    Right,
}

/// Pointer travel before a press on a bar becomes a move, physical px.
pub fn drag_threshold() -> i32 {
    scale::len(6) as i32
}

/// Grab band of an edge: outside the window, and inside it, physical px.
fn edge_out() -> i32 { scale::len(5) as i32 }
fn edge_in() -> i32 { scale::len(4) as i32 }
/// Length along an edge that counts as its corner, physical px.
fn corner_len() -> i32 { scale::len(16) as i32 }

/// Smallest floating window, logical px.
const FLOAT_MIN_W: u32 = 240;
const FLOAT_MIN_H: u32 = 160;

use super::compositor::{DragMode, DragState};
use super::cursor::Shape;

impl Compositor {
    /// A visible floating window of ours on this workspace.
    pub(super) fn is_floating_window(&self, id: WindowId) -> bool {
        self.window(id).is_some_and(|w| w.state == WindowState::Floating
            && w.workspace == self.active_workspace && w.visible
            && !w.is_overlay && !w.is_panel && w.popup_of.is_none())
    }

    /// Floating windows lie above the tiles: a stable reorder of `z_order`
    /// that keeps every other relation.
    pub(super) fn floats_above_tiles(&mut self) {
        let floating: Vec<WindowId> = self.z_order.iter().copied()
            .filter(|id| self.window(*id).is_some_and(|w| w.state == WindowState::Floating
                && !w.is_overlay && !w.is_panel && w.popup_of.is_none()))
            .collect();
        if floating.is_empty() { return; }
        let first_tile = self.z_order.iter().position(|id| self.window(*id)
            .is_some_and(|w| w.state == WindowState::Tiled && !w.is_overlay));
        let Some(first_tile) = first_tile else { return };
        let late: Vec<WindowId> = floating.iter().copied()
            .filter(|id| self.z_order.iter().position(|z| z == id).is_some_and(|p| p > first_tile))
            .collect();
        if late.is_empty() { return; }
        self.z_order.retain(|id| !late.contains(id));
        let at = self.z_order.iter().position(|id| self.window(*id)
            .is_some_and(|w| w.state == WindowState::Tiled && !w.is_overlay))
            .unwrap_or(self.z_order.len());
        for (i, id) in late.into_iter().enumerate() {
            self.z_order.insert(at + i, id);
        }
    }

    /// Lift `win` off its tile: smaller, centred over where it was, inside
    /// the work area, so it is plain that it floats.
    pub(super) fn detach_geometry(&mut self, id: WindowId) {
        let (ax, ay, aw, ah) = self.workspace_area_pub();
        let Some(win) = self.window_mut(id) else { return };
        let w = (win.width * 3 / 5).max(scale::len(FLOAT_MIN_W)).min(aw * 4 / 5).max(1);
        let h = (win.height * 3 / 5).max(scale::len(FLOAT_MIN_H)).min(ah * 4 / 5).max(1);
        let x = (win.x + win.width / 2).saturating_sub(w / 2).clamp(ax, (ax + aw).saturating_sub(w).max(ax));
        let y = (win.y + win.height / 2).saturating_sub(h / 2).clamp(ay, (ay + ah).saturating_sub(h).max(ay));
        (win.x, win.y, win.width, win.height) = (x, y, w, h);
        win.dirty = true;
    }

    /// The floating window whose edge is under `(x, y)`, and which edges.
    pub(super) fn floating_edge_at(&self, x: i32, y: i32) -> Option<(WindowId, u8)> {
        let (o, i, c) = (edge_out(), edge_in(), corner_len());
        for &id in &self.z_order {
            let Some(w) = self.window(id) else { continue };
            if !self.is_floating_window(id) { continue; }
            let (x0, y0) = (w.x as i32, w.y as i32);
            let (x1, y1) = (x0 + w.width as i32, y0 + w.height as i32);
            if x < x0 - o || x >= x1 + o || y < y0 - o || y >= y1 + o { continue; }
            let inner = x >= x0 + i && x < x1 - i && y >= y0 + i && y < y1 - i;
            if inner { return None; }
            let mut e = 0;
            if x < x0 + c.max(i) && (x < x0 + i || y < y0 + i || y >= y1 - i) { e |= EDGE_L; }
            if x >= x1 - c.max(i) && (x >= x1 - i || y < y0 + i || y >= y1 - i) { e |= EDGE_R; }
            if y < y0 + c.max(i) && (y < y0 + i || x < x0 + i || x >= x1 - i) { e |= EDGE_T; }
            if y >= y1 - c.max(i) && (y >= y1 - i || x < x0 + i || x >= x1 - i) { e |= EDGE_B; }
            if e == 0 { return None; }
            return Some((id, e));
        }
        None
    }

    /// The corner of a floating window nearest `(x, y)` (Mod+RMB).
    pub(super) fn nearest_corner(&self, id: WindowId, x: i32, y: i32) -> u8 {
        let Some(w) = self.window(id) else { return EDGE_R | EDGE_B };
        let h = if x < (w.x + w.width / 2) as i32 { EDGE_L } else { EDGE_R };
        let v = if y < (w.y + w.height / 2) as i32 { EDGE_T } else { EDGE_B };
        h | v
    }

    /// `(x, y)` lies on a free spot of a window's top bar: in the bar, off
    /// the controls, on nothing the app reacts to.
    pub(super) fn bar_free_at(&self, id: WindowId, x: i32, y: i32) -> bool {
        let Some(win) = self.window(id) else { return false };
        if !hosts(win) || win.workspace != self.active_workspace { return false; }
        let b = self.border;
        let bar = Rect::new(win.x + b, win.y + b, win.content_w(b), scale::len(BAND));
        if !contains(bar, x, y) { return false; }
        if self.controls_corner(win).is_some_and(|c| contains(c, x, y)) { return false; }
        match win.kind {
            WindowKind::Terminal => true,
            WindowKind::Widget => super::widgets::hit_test(id.0, x, y).is_none(),
            WindowKind::Surface => false,
        }
    }

    pub(super) fn start_float_drag(&mut self, id: WindowId, mode: DragMode, mx: i32, my: i32) {
        // A maximized window pulled away takes its own size back, under the
        // pointer at the same share of its width; a resized one forgets it.
        let restore = self.window(id).and_then(|w| w.restore);
        if let (Some((_, _, rw, rh)), Some(w)) = (restore, self.window(id)) {
            if mode == DragMode::Move {
                let share = (mx - w.x as i32).max(0) as u64 * rw as u64 / w.width.max(1) as u64;
                let nx = (mx - share as i32).max(0) as u32;
                let ny = w.y;
                if let Some(w) = self.window_mut(id) {
                    (w.x, w.y, w.width, w.height) = (nx, ny, rw, rh);
                    w.dirty = true;
                }
            }
            if let Some(w) = self.window_mut(id) { w.restore = None; }
        }
        let Some(w) = self.window(id) else { return };
        let start_geom = (w.x, w.y, w.width, w.height);
        self.drag = Some(DragState {
            window: id, mode, last_target: None,
            start_mx: mx, start_my: my, start_rw: 0, start_rh: 0,
            w_target: None, h_target: None, start_geom,
        });
        self.focus_window(id);
        self.controls.hover = None;
    }

    /// Follow the pointer during a Move or Edge drag. The window keeps a
    /// grip on the screen: at least `keep` of it stays inside.
    pub(super) fn apply_float_drag(&mut self, drag: &DragState, mx: i32, my: i32) {
        let (sw, sh) = (self.screen_w as i32, self.screen_h as i32);
        let (dx, dy) = (mx - drag.start_mx, my - drag.start_my);
        let (gx, gy, gw, gh) = drag.start_geom;
        let (gx, gy, gw, gh) = (gx as i32, gy as i32, gw as i32, gh as i32);
        let keep = scale::len(64) as i32;
        let (min_w, min_h) = (scale::len(FLOAT_MIN_W) as i32, scale::len(FLOAT_MIN_H) as i32);
        let (x, y, w, h) = match drag.mode {
            DragMode::Move => {
                let x = (gx + dx).clamp(keep - gw, sw - keep).max(0);
                let y = (gy + dy).clamp(0, sh - scale::len(BAND) as i32);
                (x, y, gw, gh)
            }
            DragMode::Edge(e) => {
                let (mut x0, mut y0, mut x1, mut y1) = (gx, gy, gx + gw, gy + gh);
                if e & EDGE_L != 0 { x0 = (gx + dx).clamp(0, x1 - min_w); }
                if e & EDGE_R != 0 { x1 = (gx + gw + dx).clamp(x0 + min_w, sw); }
                if e & EDGE_T != 0 { y0 = (gy + dy).clamp(0, y1 - min_h); }
                if e & EDGE_B != 0 { y1 = (gy + gh + dy).clamp(y0 + min_h, sh); }
                (x0, y0, x1 - x0, y1 - y0)
            }
            _ => return,
        };
        if let Some(win) = self.window_mut(drag.window) {
            (win.x, win.y, win.width, win.height) = (x.max(0) as u32, y.max(0) as u32, w.max(1) as u32, h.max(1) as u32);
            win.dirty = true;
        }
        if drag.mode == DragMode::Move {
            let edge = scale::len(2) as i32;
            self.snap = if mx < edge { Some(Snap::Left) }
                else if mx >= sw - edge { Some(Snap::Right) }
                else if my < edge { Some(Snap::Max) }
                else { None };
        }
    }

    /// A press on a free bar: true when it completes a double click, which
    /// then maximizes or restores a floating window, or toggles fullscreen
    /// on a tile.
    pub(super) fn bar_double_click(&mut self, id: WindowId, x: i32, y: i32) -> bool {
        let now = crate::interrupts::ticks();
        let near = scale::len(6) as i32;
        let double = self.controls_last_press(id, x, y, now, near);
        if !double {
            self.last_bar_press = Some((id, x, y, now));
            return false;
        }
        self.last_bar_press = None;
        self.pending_move = None;
        if self.is_floating_window(id) {
            self.focus_window(id);
            self.toggle_maximize(id);
        } else {
            self.focus_window(id);
            self.toggle_fullscreen(id);
        }
        self.controls.hover = None;
        self.needs_full_redraw = true;
        true
    }

    fn controls_last_press(&self, id: WindowId, x: i32, y: i32, now: u64, near: i32) -> bool {
        self.last_bar_press.is_some_and(|(lid, lx, ly, t)| lid == id
            && now.saturating_sub(t) <= DOUBLE_CLICK_TICKS
            && (x - lx).abs() <= near && (y - ly).abs() <= near)
    }

    /// A floating window to the whole work area, or back to where it was.
    pub(super) fn toggle_maximize(&mut self, id: WindowId) {
        let area = self.workspace_area_pub();
        let Some(w) = self.window_mut(id) else { return };
        match w.restore.take() {
            Some(g) => (w.x, w.y, w.width, w.height) = g,
            None => {
                w.restore = Some((w.x, w.y, w.width, w.height));
                (w.x, w.y, w.width, w.height) = area;
            }
        }
        w.dirty = true;
        self.sync_surface_sizes_pub();
    }

    /// Where a snap puts a window, screen rect.
    fn snap_rect(&self, snap: Snap) -> Rect {
        let (ax, ay, aw, ah) = self.workspace_area_pub();
        let half = aw.saturating_sub(self.gaps) / 2;
        match snap {
            Snap::Max => Rect::new(ax, ay, aw, ah),
            Snap::Left => Rect::new(ax, ay, half, ah),
            Snap::Right => Rect::new(ax + aw - half, ay, half, ah),
        }
    }

    /// The preview of a pending snap, painted under the window being moved.
    pub(super) fn snap_preview(&self) -> Option<(WindowId, Item)> {
        let snap = self.snap?;
        let drag = self.drag.filter(|d| d.mode == DragMode::Move)?;
        let r = self.snap_rect(snap);
        let color = super::widgets::palette::resolve(Token::Accent) & 0x00FF_FFFF;
        Some((drag.window, Item::Fill { rect: r, color, alpha: 70, radius: self.rounding }))
    }

    /// A move ended: land on the snap the pointer reached, if any.
    pub(super) fn finish_move(&mut self, id: WindowId) {
        let Some(snap) = self.snap.take() else { return };
        match snap {
            Snap::Max => {
                if self.window(id).is_some_and(|w| w.restore.is_none()) {
                    self.toggle_maximize(id);
                }
            }
            Snap::Left | Snap::Right => self.tile_to_side(id, snap == Snap::Left),
        }
    }

    /// Put a floating window back into the tiling as the whole left or right
    /// half; everything else shares the other half as it did before.
    fn tile_to_side(&mut self, id: WindowId, left: bool) {
        let ws = self.active_workspace;
        // The root of the tiling, if there is one.
        let root = self.windows.iter()
            .find(|w| w.id != id && w.workspace == ws && w.state == WindowState::Tiled && w.visible
                && !w.is_overlay && w.split_from.is_none_or(|p| !self.is_tiled(p)))
            .map(|w| w.id);
        // Its own children follow the old root, so this window is a leaf.
        // The root itself may be one of them (this window was the root
        // before it floated): it becomes the root for real, never its own
        // parent.
        for w in self.windows.iter_mut() {
            if w.split_from == Some(id) {
                w.split_from = if Some(w.id) == root { None } else { root };
            }
        }
        if let Some(w) = self.window_mut(id) {
            w.state = WindowState::Tiled;
            w.restore = None;
            w.resize_w = 0;
            w.resize_h = 0;
        }
        match (root, left) {
            (None, _) => {
                if let Some(w) = self.window_mut(id) { w.split_from = None; }
            }
            (Some(r), true) => {
                // This window becomes the root; the old root splits it and
                // takes the right half with everything under it.
                if let Some(w) = self.window_mut(id) { w.split_from = None; }
                if let Some(w) = self.window_mut(r) {
                    w.split_from = Some(id);
                    w.split_beside = true;
                    w.resize_w = 0;
                    w.resize_h = 0;
                }
            }
            (Some(r), false) => {
                // The first child of the root: it takes the right half, the
                // root's other children share the left.
                if let Some(w) = self.window_mut(id) {
                    w.split_from = Some(r);
                    w.split_beside = true;
                }
                let first_kid = self.windows.iter().position(|w| w.id != id && w.split_from == Some(r));
                let me = self.windows.iter().position(|w| w.id == id);
                if let (Some(k), Some(m)) = (first_kid, me) {
                    if m > k {
                        let w = self.windows.remove(m);
                        self.windows.insert(k, w);
                    }
                }
            }
        }
        self.focus_window(id);
        self.retile();
    }

    /// The pointer's shape at `(x, y)`: a resize arrow over a floating
    /// window's edge or during an edge drag.
    pub fn pointer_shape(&self, x: i32, y: i32) -> Shape {
        let edges = match self.drag {
            Some(DragState { mode: DragMode::Edge(e), .. }) => e,
            Some(_) => return Shape::Arrow,
            None => match self.floating_edge_at(x, y) {
                Some((_, e)) => e,
                None => return Shape::Arrow,
            },
        };
        let h = edges & (EDGE_L | EDGE_R) != 0;
        let v = edges & (EDGE_T | EDGE_B) != 0;
        match (h, v) {
            (true, true) if edges & (EDGE_L | EDGE_T) == (EDGE_L | EDGE_T)
                || edges & (EDGE_R | EDGE_B) == (EDGE_R | EDGE_B) => Shape::ResizeNwse,
            (true, true) => Shape::ResizeNesw,
            (true, false) => Shape::ResizeH,
            (false, true) => Shape::ResizeV,
            _ => Shape::Arrow,
        }
    }
}
