//! bar — top status bar, a strut panel rendered via the widget ABI.
//!
//! Declares itself a top-edge strut panel (`npk_window_set_panel`); the
//! compositor positions it into the bar band and draws it with the same
//! translucent-tray blit as the dock. Content is config-driven: segments
//! (`left`/`center`/`right`) listing built-in widgets — workspaces, title,
//! clock, tray, power — from `sys/config/bar`. Live state (clock / focused
//! title / active workspace) is polled from `npk_bar_state`; the tree is
//! only re-committed when that state changes. See docs/spec/PANEL.md.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read + exec + render, and the shell roles: the top panel and power off.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::EXEC | caps::RENDER, caps::ext::SHELL];

use nopeek_widgets::host;

// The calls only the bar makes; the shared ones are in
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_window_set_panel(edge: i32, behavior: i32, w: i32, h: i32) -> i32;
    fn npk_bar_state(buf_ptr: i32, max: i32) -> i32;
    fn npk_battery() -> i32;
    fn npk_workspace_switch(n: i32) -> i32;
    fn npk_power() -> i32;
    fn npk_launch(app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32;
}

fn window_set_panel(edge: i32, behavior: i32, w: i32, h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_panel(edge, behavior, w, h) };
}

/// Live bar state into `buf`; the raw length, `<= 0` when there is none.
fn bar_state(buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_bar_state(buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// `(status << 8) | percent`, or -1 without a battery.
fn battery() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_battery() }
}

fn workspace_switch(n: u32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_workspace_switch(n as i32) };
}

fn power() {
    // SAFETY: FFI without pointers.
    unsafe { npk_power() };
}

fn launch(app: &str, arg: &str) {
    // SAFETY: FFI; the kernel validates both ranges.
    unsafe {
        npk_launch(app.as_ptr() as i32, app.len() as i32, arg.as_ptr() as i32, arg.len() as i32)
    };
}

fn log(msg: &str) { host::log_serial(msg); }

// Panel edge/behavior (see docs/spec/PANEL.md / compositor set_panel).
const EDGE_TOP: i32 = 1;
const BEHAVIOR_STRUT: i32 = 1;

// ActionId encoding.
const WS_BASE: u32 = 1;        // workspace i → WS_BASE + i
const POWER: u32 = 90_000;
const SHOT: u32  = 90_001;     // screenshot: left-click = region, right = full
// Volume: left-click the speaker → launch the `volume` overlay slider;
// right-click → mute.
const VOL_OPEN: u32 = 90_002;

// Chrome sizing — docs/spec/UI_REFRESH.md §4 "Panel".
/// Height of the bar's inner content band. The panel height is derived
/// from it, so text and icons are capped to what fits inside: turning
/// them up must never push the bar taller.
const BAND_H: u16 = 24;
/// Text px in the bar. Bigger than `TextStyle::Body` (13), which reads
/// small at bar distance; 15 still leaves 6 px of band.
const FONT_DEFAULT: u16 = 15;
const FONT_MIN: u16 = 9;
const FONT_MAX: u16 = 18;      // line height at 18 ≈ 22 px < BAND_H
/// Icon px in the bar. The icon atlas holds 16, 24, 32, 48 and 64 px; any
/// other size is downscaled from the next larger one, and area-averaging a
/// 1.5 px Phosphor stroke to 0.75x blurs it. 16 is the only size below 24
/// with a 1:1 blit.
const ICON_DEFAULT: u16 = 16;
const ICON_MIN: u16 = 12;      // not in the atlas → downscaled
const ICON_MAX: u16 = 20;      // 20 + the readout's 4 px padding = BAND_H
/// Minimum width of a trailing icon cell. Used only in `tray_cell`, so it
/// widens the right-hand icons without touching the 40 px grid of the
/// workspaces (`WS_W` + zone spacing).
const CELL_W: u16 = 30;
/// Width of the separator cell: wide enough to keep a misclick off the
/// neighbouring button, with the hairline centred in it.
const SEP_W: u16 = 16;
/// Corner radius of those cells. Kept small: a tray icon is square, and
/// `Pill` would turn it into a circle.
const CELL_RADIUS: u8 = 6;
/// Width of a workspace cell. Larger than `CELL_W`, and that is the whole
/// difference between a pill and a circle: the rasterizer clamps `Pill` to
/// `min(w/2, h/2)`, i.e. 12 at a 24 px band. What remains as a straight
/// run makes the shape — 26 px leave 2 px (a circle), 38 px leave 14 px
/// (a pill).
const WS_W: u16 = 38;
// No margin of our own here. The compositor places the bar as a floating
// pill itself: `set_bar_panel` sets `win.x = margin` and
// `win.width = screen_w - 2*margin` with `shade.bar_margin`, the same
// margin the tiles use. Padding again here would make the bar narrower
// than the windows.

/// Indent before the first workspace cell, as an invisible mark.
///
/// Computed, not chosen: the card pads 4 and the row adds its own
/// `Spacing::Xxs` = 2 between mark and first cell, so 4 + 6 + 2 = 12 px
/// to the "1" — the same 12 the dock's shelf leaves inside, so the two
/// pills look alike. A mark rather than `PaddingXY`, which would pad both
/// sides.
const WS_INDENT: u16 = 6;
/// Height of the separator. Its width is not a number of its own: it sits
/// in a cell exactly as wide as a workspace — see the "title" segment.
const SEP_H: u16 = 14;
/// Corner radius of the workspace cells: fully round.
///
/// They sit inside the card, itself a pill (36 px tall → radius 18), with
/// 4 px padding between. Concentric would be 18 − 4 = 14; a 38x24 cell gets
/// 12, two pixels tighter than the arc above it. That is the safe direction
/// — too wide is the case where the corner cuts — so the bar needs no
/// horizontal allowance, unlike the dock's shelf.
const WS_RADIUS: u8 = Radius::Pill as u8;

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log("[bar] panic!");
    if let Some(loc) = info.location() {
        log(loc.file());
        log(nopeek_widgets::heap::u32_str(loc.line()));
    }
    // Trap — do not `loop {}`. A wasm `unreachable` makes `_start`'s host call
    // return Err, so the kernel tears this instance down and frees its worker
    // core. A busy loop pins the cooperative fiber forever and pegs the whole
    // core at 100%. Clean death beats a spin.
    core::arch::wasm32::unreachable()
}

const EVENT_BUF_SIZE: usize = 64;

enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event(buf: &mut [u8]) -> PollResult {
    match nopeek_widgets::events::poll(buf) {
        nopeek_widgets::events::Poll::Event(ev) => PollResult::Event(ev),
        nopeek_widgets::events::Poll::Empty => PollResult::Empty,
        nopeek_widgets::events::Poll::Gone => PollResult::WindowGone,
    }
}

// ── Config: segments ─────────────────────────────────────────────────
// `sys/config/bar` lines: "left: a b c", "center: x", "right: y z",
// plus "font: <px>" / "icon: <px>" (see read_sizes).
// Missing / empty → built-in default. Unknown widget names are skipped
// at render time.
const CFG_PATH: &str = "sys/config/bar";
const CFG_MAX: usize = 2048;

struct Segments { left: Vec<String>, center: Vec<String>, right: Vec<String> }

fn default_segments() -> Segments {
    Segments {
        left:   ["workspaces", "title"].iter().map(|s| s.to_string()).collect(),
        center: ["clock"].iter().map(|s| s.to_string()).collect(),
        right:  ["volume", "battery", "screenshot", "sep", "power"].iter().map(|s| s.to_string()).collect(),
    }
}

fn read_segments() -> Segments {
    let mut buf = [0u8; CFG_MAX];
    let n = host::fetch(CFG_PATH, &mut buf).unwrap_or(0);
    if n == 0 { return default_segments(); }
    let Ok(text) = core::str::from_utf8(&buf[..n]) else { return default_segments() };
    let mut seg = Segments { left: Vec::new(), center: Vec::new(), right: Vec::new() };
    let mut any = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((zone, items)) = line.split_once(':') else { continue };
        let list: Vec<String> = items.split_whitespace().map(|s| s.to_string()).collect();
        match zone.trim() {
            "left" => { seg.left = list; any = true; }
            "center" => { seg.center = list; any = true; }
            "right" => { seg.right = list; any = true; }
            _ => {}
        }
    }
    if any { seg } else { default_segments() }
}

// ── Config: sizing ───────────────────────────────────────────────────
// Same file, two more lines: "font: 15", "icon: 18". Re-read on config
// change, so tuning them is edit-and-look.

/// The two size lines, clamped; the defaults where a line is missing.
fn read_sizes() -> (u16, u16) {
    let mut buf = [0u8; CFG_MAX];
    let n = host::fetch(CFG_PATH, &mut buf).unwrap_or(0);
    let mut font = FONT_DEFAULT;
    let mut icon = ICON_DEFAULT;
    if n > 0 {
        if let Ok(text) = core::str::from_utf8(&buf[..n]) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                let Some((key, val)) = line.split_once(':') else { continue };
                let Ok(v) = val.trim().parse::<u16>() else { continue };
                match key.trim() {
                    "font" => font = v.clamp(FONT_MIN, FONT_MAX),
                    "icon" => icon = v.clamp(ICON_MIN, ICON_MAX),
                    _ => {}
                }
            }
        }
    }
    (font, icon)
}

// ── Live state ───────────────────────────────────────────────────────
// `npk_bar_state` → "HH:MM\n<ws_count>\n<ws_active>\n<title>".
const STATE_MAX: usize = 256;

// `npk_window_titles` → "<flags>\t<workspace>\t<title>" per open window.
const WINS_CAP: usize = 2048;

struct Bar {
    seg: Segments,
    font_px: u16,
    icon_px: u16,
    /// The bar state of the last commit; `None` before the first.
    last: Option<([u8; STATE_MAX], usize)>,
    /// Battery, polled alongside bar_state. -1 = no battery (segment
    /// hidden); else (status<<8)|percent. `i32::MIN` = never read yet.
    bat: i32,
    /// Master volume (0..=100), polled each tick. `i32::MIN` = unread.
    vol: i32,
    /// Level restored on un-mute.
    pre_mute: i32,
    /// The window list as last read.
    wins: Vec<u8>,
    /// Window titles carry the module name; the catalog maps that to the
    /// app's declared icon. Loaded once at startup.
    catalog: Vec<app_catalog::AppEntry>,
}

/// What one frame renders from.
struct BarState<'a> {
    clock: &'a str,
    ws_count: u8,
    ws_active: u8,
    title: &'a str,
    bat: i32,
    vol: u8,
    font_px: u16,
    icon_px: u16,
    wins: &'a str,
    catalog: &'a [app_catalog::AppEntry],
}

impl Bar {
    fn new() -> Self {
        let (font_px, icon_px) = read_sizes();
        Bar {
            seg: read_segments(),
            font_px,
            icon_px,
            last: None,
            bat: i32::MIN,
            vol: i32::MIN,
            pre_mute: 50,
            wins: Vec::new(),
            catalog: app_catalog::load(&[]),
        }
    }

    /// Re-read the two size lines. True when either changed.
    fn reload_sizes(&mut self) -> bool {
        let (font, icon) = read_sizes();
        let changed = self.font_px != font || self.icon_px != icon;
        self.font_px = font;
        self.icon_px = icon;
        changed
    }

    /// Re-read the window list; true when it changed since the last read.
    fn refresh_windows(&mut self) -> bool {
        let mut scratch = [0u8; WINS_CAP];
        let len = host::window_titles(&mut scratch).unwrap_or(0).min(WINS_CAP);
        if self.wins[..] == scratch[..len] {
            return false;
        }
        self.wins.clear();
        self.wins.extend_from_slice(&scratch[..len]);
        true
    }

    /// Read live state; true if it changed since the last commit (and
    /// remember it as the new last state).
    fn state_changed(&mut self) -> bool {
        let mut cur = [0u8; STATE_MAX];
        let n = bar_state(&mut cur);
        if n <= 0 { return false; }
        let n = (n as usize).min(STATE_MAX);
        // Battery and volume fold into the same change gate. The bar only
        // wakes when something changed (or the minute turned), so no
        // throttle: with the AML driver the battery is a cached value, and
        // the kernel wakes us when it moves.
        let bat = battery();
        let vol = host::audio_volume().map_or(-1, |v| v as i32);
        let same = matches!(&self.last, Some((buf, len)) if buf[..*len] == cur[..n]);
        // The window list feeds the occupied-workspace hints, and a window
        // opening on another workspace leaves bar_state untouched — so it
        // needs its own vote in the change gate.
        let wins_changed = self.refresh_windows();
        if same && bat == self.bat && vol == self.vol && !wins_changed { return false; }
        self.last = Some((cur, n));
        self.bat = bat;
        self.vol = vol;
        true
    }

    /// Render and commit the last state, if there is one.
    fn rebuild_and_commit(&self) {
        let Some((buf, len)) = &self.last else { return };
        let s = core::str::from_utf8(&buf[..*len]).unwrap_or("");
        let mut it = s.splitn(4, '\n');
        let st = BarState {
            clock: it.next().unwrap_or(""),
            ws_count: it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            ws_active: it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            title: it.next().unwrap_or(""),
            bat: self.bat,
            vol: self.vol as u8,
            font_px: self.font_px,
            icon_px: self.icon_px,
            wins: core::str::from_utf8(&self.wins).unwrap_or(""),
            catalog: &self.catalog,
        };
        let tree = build_tree(&self.seg, &st);
        match wire::encode(&tree) {
            Ok(bytes) => { if !host::scene_commit(&bytes) { log("[bar] commit failed"); } }
            Err(_) => log("[bar] encode failed"),
        }
    }

    fn handle(&mut self, ev: Event) {
        match ev {
            // Left-click. Screenshot icon → region select. Power → off.
            // Otherwise a workspace pill.
            Event::Action(ActionId(id)) => {
                if id == SHOT {
                    launch("snap", "region");
                } else if id == POWER {
                    power();
                } else if id == VOL_OPEN {
                    // Open the slider as its own centred overlay (drun-style).
                    launch("volume", "");
                } else if id >= WS_BASE && id < POWER {
                    workspace_switch(id - WS_BASE);
                }
            }
            // Right-click: screenshot icon → full-screen capture; speaker → mute.
            Event::ContextAction(ActionId(id)) => {
                if id == SHOT {
                    launch("snap", "full");
                } else if id == VOL_OPEN {
                    let v = host::audio_volume().unwrap_or(0);
                    if v > 0 {
                        self.pre_mute = v as i32;
                        host::audio_set_volume(0);
                    } else {
                        host::audio_set_volume(self.pre_mute.max(10) as u32);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Does workspace `ws` hold at least one window?
fn workspace_occupied(wins: &str, ws: u8) -> bool {
    wins.lines().any(|line| {
        let mut cols = line.split('\t');
        cols.next();
        cols.next().and_then(|w| w.trim().parse::<u8>().ok()) == Some(ws)
    })
}

fn icon_for_app(catalog: &[app_catalog::AppEntry], title: &str) -> IconId {
    catalog.iter().find(|e| e.launch_name == title)
        .map(|e| e.icon)
        .unwrap_or(IconId::Monitor)
}

// ── Segment → widgets ────────────────────────────────────────────────

/// A fixed-size, centred chrome cell — a workspace pill or a tray icon.
/// `prefab::center_box` supplies the flex spacers: `MinWidth` alone
/// widens the box and leaves the glyph pinned to its left edge.
fn cell(child: Widget, modifiers: Vec<Modifier>) -> Widget {
    prefab::center_box(child, modifiers)
}

/// Bar text: the style picks the face, `FontSize` the size. Every string
/// in the bar goes through here so one config line moves all of them.
fn bar_text(st: &BarState, content: String, style: TextStyle, mut modifiers: Vec<Modifier>) -> Widget {
    modifiers.push(Modifier::FontSize(st.font_px));
    Widget::Text { content, style, modifiers }
}

/// Icon plus a mono value (volume, battery) as one hoverable unit.
fn readout(st: &BarState, icon: IconId, icon_mods: Vec<Modifier>, value: String,
           click: Option<ActionId>) -> Widget {
    let mut mods: Vec<Modifier> = alloc::vec![
        Modifier::MinHeight(BAND_H),
        Modifier::MaxHeight(BAND_H),
        Modifier::Rounded(CELL_RADIUS),
        Modifier::Padding(Padding::Xxs.as_u16()),
        Modifier::Tint(Token::OnSurfaceMuted),
    ];
    if let Some(a) = click {
        mods.push(Modifier::OnClick(a));
        mods.push(Modifier::Hover(alloc::vec![
            Modifier::Background(Token::SurfaceHover),
            Modifier::Rounded(CELL_RADIUS),
        ]));
    }
    Widget::Row {
        children: alloc::vec![
            Widget::Icon { id: icon, size: st.icon_px, modifiers: icon_mods },
            bar_text(st, value, TextStyle::Mono, Vec::new()),
        ],
        spacing: Spacing::Xs.as_u16(),
        align: Align::Center,
        modifiers: mods,
    }
}

/// Tray icon cell: rest is `OnSurfaceMuted` on no background, hover
/// fills `SurfaceHover` (docs/spec/UI_REFRESH.md §3 `toolbar_button`).
fn tray_cell(st: &BarState, icon: IconId, click: Option<ActionId>, tint: Token) -> Widget {
    let mut mods: Vec<Modifier> = alloc::vec![
        Modifier::MinWidth(CELL_W),
        Modifier::MinHeight(BAND_H),
        Modifier::Rounded(CELL_RADIUS),
        Modifier::Tint(tint),
        Modifier::Hover(alloc::vec![
            Modifier::Background(Token::SurfaceHover),
            Modifier::Rounded(CELL_RADIUS),
        ]),
    ];
    if let Some(a) = click { mods.push(Modifier::OnClick(a)); }
    cell(Widget::Icon { id: icon, size: st.icon_px, modifiers: Vec::new() }, mods)
}

// No per-widget Padding (the bar is short — the enclosing card supplies
// the inset; uniform Padding here would overflow the band vertically).
fn segment_widgets(name: &str, st: &BarState) -> Vec<Widget> {
    match name {
        "workspaces" => {
            // A rounded cell per workspace. Active = filled Accent; an
            // occupied-but-inactive one keeps full-strength text; an empty
            // one recedes to OnSurfaceFaint, so the row doubles as an
            // at-a-glance map of where your windows are.
            let mut row = Vec::new();
            row.push(prefab::mark(WS_INDENT, 1, None));
            for i in 0..st.ws_count {
                let active = i == st.ws_active;
                let mut mods: Vec<Modifier> = alloc::vec![
                    Modifier::MinWidth(WS_W),
                    Modifier::MinHeight(BAND_H),
                    Modifier::Rounded(WS_RADIUS),
                    Modifier::OnClick(ActionId(WS_BASE + i as u32)),
                ];
                if active {
                    mods.push(Modifier::Background(Token::Accent));
                    mods.push(Modifier::Tint(Token::OnAccent));
                } else {
                    if !workspace_occupied(st.wins, i) {
                        mods.push(Modifier::Tint(Token::OnSurfaceFaint));
                    }
                    // Same shape as the cell: a hover rectangle with a different radius
                    // is the kind of thing that stands out.
                    mods.push(Modifier::Hover(alloc::vec![
                        Modifier::Background(Token::SurfaceHover),
                        Modifier::Rounded(WS_RADIUS),
                    ]));
                }
                row.push(cell(
                    bar_text(st, alloc::format!("{}", i + 1), TextStyle::Mono, Vec::new()),
                    mods));
            }
            alloc::vec![Widget::Row {
                children: row,
                spacing: Spacing::Xxs.as_u16(),
                align: Align::Center,
                modifiers: Vec::new(),
            }]
        }
        // The focused app: its catalog icon plus its name. Absent when
        // nothing is open, and preceded by a hairline so the divider only
        // shows up together with it.
        "title" => {
            if st.title.is_empty() { Vec::new() }
            else {
                // `1 2 3 4 (5) | (6) Appname` — the grid does the spacing, not a
                // balanced pair of gaps.
                //
                // The separator gets a real cell: a box of `WS_W` with the
                // hairline between two spacers, so it is centred. Everything then
                // sits on the same 40 px grid as the digits (`WS_W` + zone
                // spacing), and because the hairline is centred in its cell the
                // gap to either side is equal by construction, not by tuning.
                alloc::vec![
                    Widget::Row {
                        children: alloc::vec![
                            Widget::Spacer { flex: 1 },
                            prefab::mark(1, SEP_H, Some(Token::Border)),
                            Widget::Spacer { flex: 1 },
                        ],
                        spacing: 0,
                        align: Align::Center,
                        modifiers: alloc::vec![
                            Modifier::MinWidth(WS_W),
                            Modifier::MaxWidth(WS_W),
                            Modifier::MaxHeight(BAND_H),
                        ],
                    },
                    Widget::Row {
                        children: alloc::vec![
                            // Centre the icon in its cell, like a digit in its own.
                            //
                            // Without this it sticks to the separator: a digit is a narrow
                            // glyph in the middle of a 38 px cell, while the icon starts at
                            // its box edge. With the lead-in its centre lands on the centre
                            // of cell 6, keeping everything on the same 40 px rhythm.
                            //
                            // The width is computed: half the remaining cell minus the gap
                            // the row already sets between mark and icon. It follows
                            // `icon_px`, which comes from the config.
                            prefab::mark(
                                (WS_W.saturating_sub(st.icon_px) / 2)
                                    .saturating_sub(Spacing::Sm.as_u16()),
                                1, None),
                            Widget::Icon {
                                id: icon_for_app(st.catalog, st.title),
                                size: st.icon_px,
                                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
                            },
                            bar_text(st, st.title.to_string(), TextStyle::Body,
                                alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)]),
                        ],
                        spacing: Spacing::Sm.as_u16(),
                        align: Align::Center,
                        modifiers: Vec::new(),
                    },
                ]
            }
        }
        "clock" => alloc::vec![bar_text(st, st.clock.to_string(), TextStyle::Mono, Vec::new())],
        // Battery: hidden entirely when no smart battery responds (bat < 0,
        // e.g. desktops/QEMU). Icon picked by charge level; charging shows
        // the bolt; a near-empty pack tints Danger.
        "battery" => {
            if st.bat < 0 { return Vec::new(); }
            let percent = (st.bat & 0xFF) as u8;
            let status = (st.bat >> 8) & 0xFF; // 0=discharge 1=charge 2=full 3=plugged-idle
            let icon = match status {
                1 => IconId::BatteryCharging, // actively charging → bolt
                2 => IconId::BatteryFull,
                3 => IconId::Plug,            // on AC, held (not charging) → plug
                _ => match percent {
                    0..=10  => IconId::BatteryWarning,
                    11..=30 => IconId::BatteryLow,
                    31..=55 => IconId::BatteryMedium,
                    56..=85 => IconId::BatteryHigh,
                    _       => IconId::BatteryFull,
                },
            };
            let mut icon_mods: Vec<Modifier> = Vec::new();
            if status == 0 && percent <= 10 {
                icon_mods.push(Modifier::Tint(Token::Danger));
            }
            alloc::vec![readout(st, icon, icon_mods,
                alloc::format!("{}%", percent), None)]
        }
        "screenshot" => alloc::vec![
            tray_cell(st, IconId::Camera, Some(ActionId(SHOT)), Token::OnSurfaceMuted)
        ],
        // Separator before the power button, in its own cell — built like
        // the one between workspaces and app. Centred in the cell, so the
        // gap to the camera and to the power button is equal by
        // construction. The cell also keeps a click on the camera from
        // landing on the power button.
        "sep" => alloc::vec![Widget::Row {
            children: alloc::vec![
                Widget::Spacer { flex: 1 },
                prefab::mark(1, SEP_H, Some(Token::Border)),
                Widget::Spacer { flex: 1 },
            ],
            spacing: 0,
            align: Align::Center,
            modifiers: alloc::vec![
                Modifier::MinWidth(SEP_W),
                Modifier::MaxWidth(SEP_W),
                Modifier::MaxHeight(BAND_H),
            ],
        }],
        // Fixed-width empty space. No longer in the default ("sep"
        // replaced it), kept so an existing `sys/config/bar` may still
        // name it.
        "gap" => alloc::vec![Widget::Text {
            content: String::new(),
            style: TextStyle::Body,
            modifiers: alloc::vec![Modifier::MinWidth(16)],
        }],
        "power" => alloc::vec![
            tray_cell(st, IconId::Power, Some(ActionId(POWER)), Token::Danger)
        ],
        // Volume: speaker icon + level. Left-click either → launch the
        // `volume` overlay slider; right-click → mute. Reflects the kernel
        // master volume (also moved by the slider / apps).
        "volume" => {
            let v = st.vol;
            let icon = if v == 0 { IconId::SpeakerX }
                       else if v <= 50 { IconId::SpeakerLow }
                       else { IconId::SpeakerHigh };
            alloc::vec![readout(st, icon, Vec::new(),
                alloc::format!("{}%", v), Some(ActionId(VOL_OPEN)))]
        }
        _ => Vec::new(),
    }
}

/// A zone → a plain Row of its segments. The chrome is not per zone:
/// one bar card holds all three (docs/spec/UI_REFRESH.md §4). Empty
/// zones collapse to a zero spacer so the left/center/right structure
/// stays intact.
fn zone(names: &[String], st: &BarState) -> Widget {
    let mut kids = Vec::new();
    for n in names { kids.extend(segment_widgets(n, st)); }
    if kids.is_empty() {
        return Widget::Spacer { flex: 0 };
    }
    Widget::Row {
        children: kids,
        spacing: Spacing::Xxs.as_u16(),
        align: Align::Center,
        // Pin the zone to the band. Without this a single over-tall
        // segment makes the zone taller than the bar's content box, the
        // centring offset saturates to zero, and everything in that zone
        // rides out of the bottom edge.
        modifiers: alloc::vec![
            Modifier::MinHeight(BAND_H),
            Modifier::MaxHeight(BAND_H),
        ],
    }
}

fn build_tree(seg: &Segments, st: &BarState) -> Widget {
    // Two overlaid full-width layers so the clock is centred on the screen,
    // not between the (asymmetric) side groups: the sides layer pins left to
    // the start + right to the end; the centre layer centres the clock with
    // equal flex spacers.
    let sides = Widget::Row {
        children: alloc::vec![
            zone(&seg.left, st),
            Widget::Spacer { flex: 1 },
            zone(&seg.right, st),
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: Vec::new(),
    };
    let center = Widget::Row {
        children: alloc::vec![
            Widget::Spacer { flex: 1 },
            zone(&seg.center, st),
            Widget::Spacer { flex: 1 },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: Vec::new(),
    };
    // One continuous card. In the bar's transparent scene the background
    // is filled at chrome alpha with anti-aliased corners and the
    // compositor composites it by per-pixel alpha — translucent panel,
    // crisp glyphs, no halo.
    Widget::Stack {
        children: alloc::vec![sides, center],
        modifiers: alloc::vec![
            Modifier::Background(Token::SurfaceElevated),
            // Fully round. The rasterizer clamps `Pill` to `min(w/2, h/2)`.
            // The card fills the window, whose height the compositor sets to
            // `pill_h` = the `h` from `set_panel`, i.e. 36 → radius 18, true
            // semicircles at both ends.
            //
            // Border and fill share the same value; two values would leave
            // the 1 px stroke beside its own area.
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Pill.as_u8() },
            Modifier::Rounded(Radius::Pill.as_u8()),
            Modifier::Padding(Padding::Xs.as_u16()),
        ],
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut bar = Bar::new();

    // Declare the top strut panel. `h` is the band height we need (room for
    // 24px tray icons + pill padding); the compositor reserves `margin + h`
    // and sizes our window to it. `w` is advisory.
    window_set_panel(EDGE_TOP, BEHAVIOR_STRUT, 1920, 36);

    // First commit.
    if bar.state_changed() {
        bar.rebuild_and_commit();
    }

    // Wait for a change, don't poll for one. The kernel wakes the bar:
    // WAIT_INPUT for a click, WAIT_STATE when a watched topic changes
    // (windows, battery, volume, a config file), and the deadline is the
    // next full minute for the clock. docs/plan/CORES_AND_EVENTS.md.
    const WAIT_INPUT: u32 = 1;
    const WAIT_STATE: u32 = 32;
    let mut fired = 0u32;
    let mut event_buf = [0u8; EVENT_BUF_SIZE];

    loop {
        // Drain any pending click events first.
        loop {
            match poll_event(&mut event_buf) {
                PollResult::Event(ev) => bar.handle(ev),
                PollResult::Empty => break,
                PollResult::WindowGone => return,
            }
        }
        // Sizes come from `sys/config/bar` and are tuned by eye, so an edit
        // must show without a restart — a config write is a watched topic.
        let resized = fired & WAIT_STATE != 0 && bar.reload_sizes();
        // Re-render only when the live state changed (clock minute / title
        // / active workspace) or the sizes moved, so the tree isn't rebuilt
        // every wake.
        if bar.state_changed() || resized {
            bar.rebuild_and_commit();
        }
        // Until the next full minute, plus a little so the minute has
        // surely turned when we read the clock.
        let now = host::unix_time();
        let to_minute_ms = (60 - now % 60) * 1000 + 50;
        fired = host::wait(WAIT_INPUT | WAIT_STATE, to_minute_ms as i32) as u32;
    }
}
