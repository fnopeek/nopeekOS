//! bar — top status status bar, a strut panel rendered via the widget ABI.
//!
//! Declares itself a top-edge strut panel (`npk_window_set_panel`); the
//! compositor positions it into the bar band and draws it with the same
//! translucent-tray blit as the dock. Content is config-driven: segments
//! (`left`/`center`/`right`) listing built-in widgets — workspaces, title,
//! clock, tray, power — from `sys/config/bar`. Live state (clock / focused
//! title / active workspace) is polled from `npk_bar_state`; the tree is
//! only re-committed when that state changes. See PANEL.md.

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

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_window_set_panel(edge: i32, behavior: i32, w: i32, h: i32) -> i32;
    fn npk_bar_state(buf_ptr: i32, max: i32) -> i32;
    fn npk_window_titles(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_battery() -> i32;
    fn npk_workspace_switch(n: i32) -> i32;
    fn npk_power() -> i32;
    fn npk_audio_get_volume() -> i32;
    fn npk_audio_set_volume(pct: i32) -> i32;
    fn npk_launch(app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

// Panel edge/behavior (see PANEL.md / compositor set_panel).
const EDGE_TOP: i32 = 1;
const BEHAVIOR_STRUT: i32 = 1;

// ActionId encoding.
const WS_BASE: u32 = 1;        // workspace i → WS_BASE + i
const POWER: u32 = 90_000;
const SHOT: u32  = 90_001;     // screenshot: left-click = region, right = full
// Volume: left-click the speaker → launch the `volume` overlay slider;
// right-click → mute.
const VOL_OPEN: u32 = 90_002;

// Chrome sizing — UI_REFRESH.md §4 "Panel".
const ICON_SIZE: u16 = 16;
/// Height of the bar's inner content band.
const BAND_H: u16 = 24;
/// Minimum width of a workspace pill / a trailing icon cell.
const CELL_W: u16 = 26;
/// Corner radius of those cells.
const CELL_RADIUS: u8 = 6;

// ── Bump allocator with a reset mark ─────────────────────────────────
// Config is parsed once (below MARK and kept); the per-frame widget tree
// is rebuilt above MARK and reclaimed each commit by resetting to MARK,
// so re-committing every clock tick never leaks.
const HEAP_SIZE: usize = 256 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;
static mut HEAP_MARK: usize = 0;

struct BumpAllocator;
unsafe impl core::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let pos_ptr = core::ptr::addr_of_mut!(HEAP_POS);
        let current = unsafe { pos_ptr.read() };
        let aligned = (current + layout.align() - 1) & !(layout.align() - 1);
        if aligned + layout.size() > HEAP_SIZE { return core::ptr::null_mut(); }
        unsafe { pos_ptr.write(aligned + layout.size()); }
        unsafe { (core::ptr::addr_of_mut!(HEAP) as *mut u8).add(aligned) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

// u32 → decimal &str in a static buffer (no alloc — safe in the panic handler
// even when the panic IS an allocation failure).
static mut NUMBUF: [u8; 12] = [0; 12];
fn u32_str(mut n: u32) -> &'static str {
    let b = core::ptr::addr_of_mut!(NUMBUF) as *mut u8;
    let buf = unsafe { core::slice::from_raw_parts_mut(b, 12) };
    let mut i = 12;
    if n == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i..]) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log("[bar] panic!");
    if let Some(loc) = info.location() {
        log(loc.file());
        log(u32_str(loc.line()));
    }
    // Trap — do NOT `loop {}`. A wasm `unreachable` makes `_start`'s host call
    // return Err, so the kernel tears this instance down and frees its worker
    // core. A busy loop pins the cooperative fiber forever → the whole core
    // pegs at 100% (the "core spins, never halts" bug). Clean death > spin.
    core::arch::wasm32::unreachable()
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

const EVENT_BUF_SIZE: usize = 64;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event() -> PollResult {
    let buf_ptr = core::ptr::addr_of_mut!(EVENT_BUF) as *mut u8;
    let n = unsafe { npk_event_poll(buf_ptr as i32, EVENT_BUF_SIZE as i32) };
    if n < 0 { return PollResult::WindowGone; }
    if n == 0 { return PollResult::Empty; }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    match postcard::from_bytes::<Event>(slice) {
        Ok(ev) => PollResult::Event(ev),
        Err(_) => PollResult::Empty,
    }
}

// ── Config: segments ─────────────────────────────────────────────────
// `sys/config/bar` lines: "left: a b c", "center: x", "right: y z".
// Missing / empty → built-in default. Unknown widget names are skipped
// at render time.
struct Segments { left: Vec<String>, center: Vec<String>, right: Vec<String> }

fn default_segments() -> Segments {
    Segments {
        left:   ["workspaces", "title"].iter().map(|s| s.to_string()).collect(),
        center: ["clock"].iter().map(|s| s.to_string()).collect(),
        right:  ["volume", "battery", "tray", "screenshot", "gap", "power"].iter().map(|s| s.to_string()).collect(),
    }
}

fn read_segments() -> Segments {
    const CFG: usize = 2048;
    static mut BUF: [u8; CFG] = [0; CFG];
    let path = "sys/config/bar";
    let p = core::ptr::addr_of_mut!(BUF) as *mut u8;
    let n = unsafe { npk_fetch(path.as_ptr() as i32, path.len() as i32, p as i32, CFG as i32) };
    if n <= 0 { return default_segments(); }
    let bytes = unsafe { core::slice::from_raw_parts(p as *const u8, n as usize) };
    let Ok(text) = core::str::from_utf8(bytes) else { return default_segments() };
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

// ── Live state ───────────────────────────────────────────────────────
// `npk_bar_state` → "HH:MM\n<ws_count>\n<ws_active>\n<title>".
const STATE_MAX: usize = 256;
static mut STATE_BUF: [u8; STATE_MAX] = [0; STATE_MAX];
static mut LAST_BUF: [u8; STATE_MAX] = [0; STATE_MAX];
static mut LAST_LEN: usize = usize::MAX;
// Battery, polled alongside bar_state. -1 = no battery (segment hidden);
// else (status<<8)|percent. Sentinel i32::MIN = never read yet.
static mut BAT: i32 = -1;
static mut LAST_BAT: i32 = i32::MIN;
static mut BAT_TICK: u32 = 0;
// Master volume (0..=100), polled each tick (cheap atomic). i32::MIN = unread.
static mut VOL: i32 = 80;
static mut LAST_VOL: i32 = i32::MIN;
static mut PRE_MUTE: i32 = 50; // level restored on un-mute

struct BarState<'a> { clock: &'a str, ws_count: u8, ws_active: u8, title: &'a str, bat: i32, vol: u8 }

fn parse_state(s: &str) -> BarState<'_> {
    let mut it = s.splitn(4, '\n');
    let clock = it.next().unwrap_or("");
    let ws_count = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let ws_active = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let title = it.next().unwrap_or("");
    BarState { clock, ws_count, ws_active, title, bat: -1, vol: 0 }
}

// ── Window list ──────────────────────────────────────────────────────
// `npk_window_titles` → "<flags>\t<workspace>\t<title>" per open window.
// Kept in a static buffer: the render loop resets the bump allocator
// before every commit, so heap-derived state would dangle a frame later.
const WINS_CAP: usize = 2048;
static mut WINS: [u8; WINS_CAP] = [0; WINS_CAP];
static mut WINS_LEN: usize = 0;

/// Re-read the window list; true when it changed since the last read.
fn refresh_windows() -> bool {
    let mut scratch = [0u8; WINS_CAP];
    let n = unsafe { npk_window_titles(scratch.as_mut_ptr() as i32, WINS_CAP as i32) };
    let len = if n > 0 { n as usize } else { 0 };
    // SAFETY: single-threaded WASM app.
    unsafe {
        if *(&raw const WINS_LEN) == len && (&*(&raw const WINS))[..len] == scratch[..len] {
            return false;
        }
        (&mut *(&raw mut WINS))[..len].copy_from_slice(&scratch[..len]);
        *(&raw mut WINS_LEN) = len;
    }
    true
}

fn windows_text() -> &'static str {
    // SAFETY: single-threaded; only refresh_windows writes the buffer.
    unsafe {
        let len = *(&raw const WINS_LEN);
        core::str::from_utf8(&(&*(&raw const WINS))[..len]).unwrap_or("")
    }
}

/// Does workspace `ws` hold at least one window?
fn workspace_occupied(ws: u8) -> bool {
    windows_text().lines().any(|line| {
        let mut cols = line.split('\t');
        cols.next();
        cols.next().and_then(|w| w.trim().parse::<u8>().ok()) == Some(ws)
    })
}

// ── App icons ────────────────────────────────────────────────────────
// Window titles carry the module name; the catalog maps that to the
// app's declared icon. Loaded once at startup (below the heap mark).
static mut CATALOG: Option<Vec<app_catalog::AppEntry>> = None;

fn load_catalog() {
    // SAFETY: single-threaded; called once before the render loop.
    unsafe { *(&raw mut CATALOG) = Some(app_catalog::load(&[])); }
}

fn icon_for_app(title: &str) -> IconId {
    // SAFETY: single-threaded; written once by load_catalog.
    let cat = unsafe { (*(&raw const CATALOG)).as_ref() };
    cat.and_then(|c| c.iter().find(|e| e.launch_name == title))
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

/// Icon plus a mono value (volume, battery) as one hoverable unit.
fn readout(icon: IconId, icon_mods: Vec<Modifier>, value: String,
           click: Option<ActionId>) -> Widget {
    let mut mods: Vec<Modifier> = alloc::vec![
        Modifier::MinHeight(BAND_H),
        Modifier::Rounded(CELL_RADIUS),
        Modifier::Padding(Padding::Sm.as_u16()),
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
            Widget::Icon { id: icon, size: ICON_SIZE, modifiers: icon_mods },
            Widget::Text { content: value, style: TextStyle::Mono, modifiers: Vec::new() },
        ],
        spacing: Spacing::Xs.as_u16(),
        align: Align::Center,
        modifiers: mods,
    }
}

/// Tray icon cell: rest is `OnSurfaceMuted` on no background, hover
/// fills `SurfaceHover` (UI_REFRESH.md §3 `toolbar_button`).
fn tray_cell(icon: IconId, click: Option<ActionId>, tint: Token) -> Widget {
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
    cell(Widget::Icon { id: icon, size: ICON_SIZE, modifiers: Vec::new() }, mods)
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
            for i in 0..st.ws_count {
                let active = i == st.ws_active;
                let mut mods: Vec<Modifier> = alloc::vec![
                    Modifier::MinWidth(CELL_W),
                    Modifier::MinHeight(BAND_H),
                    Modifier::Rounded(CELL_RADIUS),
                    Modifier::OnClick(ActionId(WS_BASE + i as u32)),
                ];
                if active {
                    mods.push(Modifier::Background(Token::Accent));
                    mods.push(Modifier::Tint(Token::OnAccent));
                } else {
                    if !workspace_occupied(i) {
                        mods.push(Modifier::Tint(Token::OnSurfaceFaint));
                    }
                    mods.push(Modifier::Hover(alloc::vec![
                        Modifier::Background(Token::SurfaceHover),
                        Modifier::Rounded(CELL_RADIUS),
                    ]));
                }
                row.push(cell(Widget::Text {
                    content: alloc::format!("{}", i + 1),
                    style: TextStyle::Mono,
                    modifiers: Vec::new(),
                }, mods));
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
                alloc::vec![
                    Widget::Row {
                        children: alloc::vec![prefab::mark(1, 14, Some(Token::Border))],
                        spacing: 0,
                        align: Align::Center,
                        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
                    },
                    Widget::Row {
                        children: alloc::vec![
                            Widget::Icon {
                                id: icon_for_app(st.title),
                                size: 16,
                                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
                            },
                            Widget::Text {
                                content: st.title.to_string(),
                                style: TextStyle::Body,
                                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
                            },
                        ],
                        spacing: Spacing::Sm.as_u16(),
                        align: Align::Center,
                        modifiers: Vec::new(),
                    },
                ]
            }
        }
        "clock" => alloc::vec![Widget::Text {
            content: st.clock.to_string(),
            style: TextStyle::Mono,
            modifiers: Vec::new(),
        }],
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
            alloc::vec![readout(icon, icon_mods,
                alloc::format!("{}%", percent), None)]
        }
        "tray" => alloc::vec![tray_cell(IconId::Gear, None, Token::OnSurfaceMuted)],
        "screenshot" => alloc::vec![
            tray_cell(IconId::Camera, Some(ActionId(SHOT)), Token::OnSurfaceMuted)
        ],
        // Fixed-width empty filler — keeps the camera and power icons a
        // safe distance apart so a click can't land on the wrong one.
        "gap" => alloc::vec![Widget::Text {
            content: String::new(),
            style: TextStyle::Body,
            modifiers: alloc::vec![Modifier::MinWidth(16)],
        }],
        "power" => alloc::vec![
            tray_cell(IconId::Power, Some(ActionId(POWER)), Token::Danger)
        ],
        // Volume: speaker icon + level. Left-click either → launch the
        // `volume` overlay slider; right-click → mute. Reflects the kernel
        // master volume (also moved by the slider / apps).
        "volume" => {
            let v = st.vol;
            let icon = if v == 0 { IconId::SpeakerX }
                       else if v <= 50 { IconId::SpeakerLow }
                       else { IconId::SpeakerHigh };
            alloc::vec![readout(icon, Vec::new(),
                alloc::format!("{}%", v), Some(ActionId(VOL_OPEN)))]
        }
        _ => Vec::new(),
    }
}

/// A zone → a plain Row of its segments. The chrome is no longer per
/// zone: the design has ONE bar card holding all three (UI_REFRESH.md
/// §4). Empty zones collapse to a zero spacer so the left/center/right
/// structure stays intact.
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
        modifiers: Vec::new(),
    }
}

fn build_tree(seg: &Segments, st: &BarState) -> Widget {
    // Two overlaid full-width layers so the clock is centred on the SCREEN,
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
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
            Modifier::Rounded(Radius::Md.as_u8()),
            Modifier::Padding(Padding::Xs.as_u16()),
        ],
    }
}

/// Read live state into STATE_BUF; return Some(len) if it changed since
/// the last commit (and update LAST_BUF), else None.
fn state_changed() -> Option<usize> {
    let p = core::ptr::addr_of_mut!(STATE_BUF) as *mut u8;
    let n = unsafe { npk_bar_state(p as i32, STATE_MAX as i32) };
    if n <= 0 { return None; }
    let n = n as usize;
    // Poll battery too — it changes slowly, so throttle the SMBus reads to
    // roughly every 5 s (loop tick is 300 ms) and fold the result into the
    // same change-gate (a % or charge-state flip forces a re-commit).
    let bat = unsafe {
        if BAT_TICK == 0 { BAT_TICK = 16; npk_battery() } else { BAT_TICK -= 1; LAST_BAT }
    };
    let vol = unsafe { npk_audio_get_volume() };
    let cur = unsafe { core::slice::from_raw_parts(p as *const u8, n) };
    let last = unsafe {
        let lp = core::ptr::addr_of!(LAST_BUF) as *const u8;
        let ll = LAST_LEN;
        if ll == usize::MAX { None } else { Some(core::slice::from_raw_parts(lp, ll)) }
    };
    let bat_same = bat == unsafe { LAST_BAT };
    let vol_same = vol == unsafe { LAST_VOL };
    // The window list feeds the occupied-workspace hints, and a window
    // opening on ANOTHER workspace leaves bar_state untouched — so it
    // needs its own vote in the change gate.
    let wins_changed = refresh_windows();
    if last == Some(cur) && bat_same && vol_same && !wins_changed { return None; }
    unsafe {
        let lp = core::ptr::addr_of_mut!(LAST_BUF) as *mut u8;
        core::ptr::copy_nonoverlapping(p, lp, n);
        LAST_LEN = n;
        LAST_BAT = bat;
        BAT = bat;
        LAST_VOL = vol;
        VOL = vol;
    }
    Some(n)
}

fn rebuild_and_commit(seg: &Segments, len: usize) {
    // Reclaim the previous frame's tree; config (below the mark) survives.
    unsafe { HEAP_POS = HEAP_MARK; }
    let s = unsafe {
        core::str::from_utf8(core::slice::from_raw_parts(
            core::ptr::addr_of!(STATE_BUF) as *const u8, len)).unwrap_or("")
    };
    let mut st = parse_state(s);
    st.bat = unsafe { BAT };
    st.vol = unsafe { VOL as u8 };
    let tree = build_tree(seg, &st);
    match wire::encode(&tree) {
        Ok(bytes) => { if commit(&bytes) < 0 { log("[bar] commit failed"); } }
        Err(_) => log("[bar] encode failed"),
    }
}

fn launch(app: &str, arg: &str) {
    unsafe {
        let _ = npk_launch(app.as_ptr() as i32, app.len() as i32,
                           arg.as_ptr() as i32, arg.len() as i32);
    }
}

fn handle(ev: Event) {
    match ev {
        // Left-click. Screenshot icon → region select (slice ③; falls
        // back to full for now). Power → off. Otherwise a workspace pill.
        Event::Action(ActionId(id)) => {
            if id == SHOT {
                launch("snap", "region");
            } else if id == POWER {
                unsafe { let _ = npk_power(); }
            } else if id == VOL_OPEN {
                // Open the slider as its own centred overlay (drun-style).
                launch("volume", "");
            } else if id >= WS_BASE && id < POWER {
                unsafe { let _ = npk_workspace_switch((id - WS_BASE) as i32); }
            }
        }
        // Right-click: screenshot icon → full-screen capture; speaker → mute.
        Event::ContextAction(ActionId(id)) => {
            if id == SHOT {
                launch("snap", "full");
            } else if id == VOL_OPEN {
                let v = unsafe { npk_audio_get_volume() };
                if v > 0 {
                    unsafe { PRE_MUTE = v; let _ = npk_audio_set_volume(0); }
                } else {
                    unsafe { let _ = npk_audio_set_volume(PRE_MUTE.max(10)); }
                }
            }
        }
        _ => {}
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let seg = read_segments();
    load_catalog();                    // title → app icon, resolved once
    unsafe { HEAP_MARK = HEAP_POS; }   // freeze config; tree rebuilds above this

    // Declare the top strut panel. `h` is the band height we need (room for
    // 24px tray icons + pill padding); the compositor reserves `margin + h`
    // and sizes our window to it. `w` is advisory.
    unsafe { let _ = npk_window_set_panel(EDGE_TOP, BEHAVIOR_STRUT, 1920, 36); }

    // First commit.
    if let Some(len) = state_changed() {
        rebuild_and_commit(&seg, len);
    }

    loop {
        // Drain any pending click events first.
        loop {
            match poll_event() {
                PollResult::Event(ev) => handle(ev),
                PollResult::Empty => break,
                PollResult::WindowGone => return,
            }
        }
        // Re-render only when the live state changed (clock minute / title
        // / active workspace), so the tree isn't rebuilt every tick.
        if let Some(len) = state_changed() {
            rebuild_and_commit(&seg, len);
        }
        unsafe { let _ = npk_sleep(300); }
    }
}
