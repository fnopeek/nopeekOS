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

use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_window_set_panel(edge: i32, behavior: i32, w: i32, h: i32) -> i32;
    fn npk_bar_state(buf_ptr: i32, max: i32) -> i32;
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
const VOL_DOWN: u32 = 90_002;
const VOL_UP: u32   = 90_003;
const MUTE: u32     = 90_004;

const ICON_SIZE: u16 = 24;

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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[bar] panic!");
    loop {}
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

// ── Segment → widgets ────────────────────────────────────────────────
// No per-widget Padding (the bar is short — the enclosing card supplies
// the inset; uniform Padding here would overflow the band vertically).
fn segment_widgets(name: &str, st: &BarState) -> Vec<Widget> {
    match name {
        "workspaces" => {
            // Each workspace is a rounded pill button (Wayland/Waybar style):
            // active = filled Accent, inactive = subtle SurfaceMuted. No
            // separators between them.
            let mut row = Vec::new();
            for i in 0..st.ws_count {
                let active = i == st.ws_active;
                let mut mods: Vec<Modifier> = alloc::vec![
                    // Spaces give the pill horizontal width (capsule); uniform
                    // Padding can only add vertical room too, which would
                    // overflow the band. Small Padding for a thin inset.
                    Modifier::Padding(Padding::Xs.as_u16()),
                    Modifier::Rounded(Radius::Lg.as_u8()),  // capsule ends
                    Modifier::OnClick(ActionId(WS_BASE + i as u32)),
                ];
                if active {
                    // Active = filled Accent pill; OnAccent text so the
                    // number reads on the accent fill (both themes).
                    mods.push(Modifier::Background(Token::Accent));
                    mods.push(Modifier::Tint(Token::OnAccent));
                } else {
                    mods.push(Modifier::Hover(alloc::vec![
                        Modifier::Background(Token::SurfaceMuted),
                        Modifier::Rounded(Radius::Lg.as_u8()),
                    ]));
                }
                row.push(Widget::Text {
                    content: alloc::format!("    {}    ", i + 1),
                    style: TextStyle::Body,
                    modifiers: mods,
                });
            }
            alloc::vec![Widget::Row {
                children: row,
                spacing: Spacing::Xs.as_u16(),
                align: Align::Center,
                modifiers: Vec::new(),
            }]
        }
        // The title only appears when something is focused/open; prefix it
        // with a single "|" separator (so the divider shows up only then,
        // after the workspaces) and give it a trailing space.
        "title" => {
            if st.title.is_empty() { Vec::new() }
            else {
                alloc::vec![
                    Widget::Text {
                        content: "|".to_string(),
                        style: TextStyle::Body,
                        modifiers: Vec::new(),
                    },
                    Widget::Text {
                        content: alloc::format!("{}  ", st.title),
                        style: TextStyle::Body,
                        modifiers: Vec::new(),
                    },
                ]
            }
        }
        // Padded with spaces for horizontal breathing room inside the pill
        // (uniform Modifier::Padding would also grow it vertically past the
        // band, so the spaces give left/right air without that).
        "clock" => alloc::vec![Widget::Text {
            content: alloc::format!("    {}    ", st.clock),
            style: TextStyle::Heading,
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
            alloc::vec![
                Widget::Icon { id: icon, size: ICON_SIZE, modifiers: icon_mods },
                Widget::Text {
                    content: alloc::format!(" {}%", percent),
                    style: TextStyle::Body,
                    modifiers: Vec::new(),
                },
            ]
        }
        "tray" => alloc::vec![Widget::Icon {
            id: IconId::Gear,
            size: ICON_SIZE,
            modifiers: Vec::new(),
        }],
        "screenshot" => alloc::vec![Widget::Icon {
            id: IconId::Camera,
            size: ICON_SIZE,
            modifiers: alloc::vec![Modifier::OnClick(ActionId(SHOT))],
        }],
        // Fixed-width empty filler — keeps the camera and power icons a
        // safe distance apart so a click can't land on the wrong one.
        "gap" => alloc::vec![Widget::Text {
            content: String::new(),
            style: TextStyle::Body,
            modifiers: alloc::vec![Modifier::MinWidth(32)],
        }],
        "power" => alloc::vec![Widget::Icon {
            id: IconId::Power,
            size: ICON_SIZE,
            modifiers: alloc::vec![
                Modifier::Tint(Token::Danger),
                Modifier::OnClick(ActionId(POWER)),
            ],
        }],
        // Volume: speaker icon (level / mute) toggles mute; minus/plus step by
        // 10. Reflects the kernel master volume (also moved by `volume`/apps).
        "volume" => {
            let v = st.vol;
            let icon = if v == 0 { IconId::SpeakerX }
                       else if v <= 50 { IconId::SpeakerLow }
                       else { IconId::SpeakerHigh };
            alloc::vec![
                Widget::Icon { id: icon, size: ICON_SIZE,
                    modifiers: alloc::vec![Modifier::OnClick(ActionId(MUTE))] },
                Widget::Icon { id: IconId::Minus, size: ICON_SIZE,
                    modifiers: alloc::vec![Modifier::OnClick(ActionId(VOL_DOWN))] },
                Widget::Text { content: alloc::format!(" {}% ", v),
                    style: TextStyle::Body, modifiers: Vec::new() },
                Widget::Icon { id: IconId::Plus, size: ICON_SIZE,
                    modifiers: alloc::vec![Modifier::OnClick(ActionId(VOL_UP))] },
            ]
        }
        _ => Vec::new(),
    }
}

/// A zone → a detached pill (Row with its own translucent SurfaceElevated
/// background + rounding). Empty zones collapse to a zero spacer so the
/// left/center/right structure (and the wallpaper gaps) stay intact.
fn card(names: &[String], st: &BarState) -> Widget {
    let mut kids = Vec::new();
    for n in names { kids.extend(segment_widgets(n, st)); }
    if kids.is_empty() {
        return Widget::Spacer { flex: 0 };
    }
    Widget::Row {
        children: kids,
        // A touch more air between the icons (gear/camera/battery) than the
        // default Sm so they don't crowd each other; the big pre-power gap is
        // the separate `gap` filler.
        spacing: Spacing::Md.as_u16(),
        align: Align::Center,
        modifiers: alloc::vec![
            Modifier::Background(Token::SurfaceElevated),
            // Rounded by the rasteriser: in the bar's transparent scene the
            // bg is filled at chrome alpha with proper AA corners, then the
            // compositor composites by per-pixel alpha → clean translucent
            // pill, no halo.
            Modifier::Rounded(Radius::Md.as_u8()),
            Modifier::Padding(Padding::Xs.as_u16()),
        ],
    }
}

fn build_tree(seg: &Segments, st: &BarState) -> Widget {
    // Two overlaid full-width layers so the clock is centred on the SCREEN,
    // not between the (asymmetric) side groups: the sides layer pins left to
    // the start + right to the end; the centre layer centres the clock with
    // equal flex spacers. Backgroundless containers stay the Surface clear →
    // wallpaper shows in the gaps; the cards are the detached pills.
    // Align::Stretch makes every pill the full bar height.
    let sides = Widget::Row {
        children: alloc::vec![
            card(&seg.left, st),
            Widget::Spacer { flex: 1 },
            card(&seg.right, st),
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Stretch,
        modifiers: Vec::new(),
    };
    let center = Widget::Row {
        children: alloc::vec![
            Widget::Spacer { flex: 1 },
            card(&seg.center, st),
            Widget::Spacer { flex: 1 },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Stretch,
        modifiers: Vec::new(),
    };
    Widget::Stack {
        children: alloc::vec![sides, center],
        modifiers: Vec::new(),
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
    if last == Some(cur) && bat_same && vol_same { return None; }
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
            } else if id == VOL_DOWN {
                let v = unsafe { npk_audio_get_volume() };
                unsafe { let _ = npk_audio_set_volume((v - 10).max(0)); }
            } else if id == VOL_UP {
                let v = unsafe { npk_audio_get_volume() };
                unsafe { let _ = npk_audio_set_volume((v + 10).min(100)); }
            } else if id == MUTE {
                let v = unsafe { npk_audio_get_volume() };
                if v > 0 {
                    unsafe { PRE_MUTE = v; let _ = npk_audio_set_volume(0); }
                } else {
                    unsafe { let _ = npk_audio_set_volume(PRE_MUTE.max(10)); }
                }
            } else if id >= WS_BASE && id < POWER {
                unsafe { let _ = npk_workspace_switch((id - WS_BASE) as i32); }
            }
        }
        // Right-click on the screenshot icon → full-screen capture.
        Event::ContextAction(ActionId(id)) => {
            if id == SHOT { launch("snap", "full"); }
        }
        _ => {}
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let seg = read_segments();
    unsafe { HEAP_MARK = HEAP_POS; }   // freeze config; tree rebuilds above this

    // Declare the top strut panel. `h` is the band height we need (room for
    // 24px tray icons + pill padding); the compositor reserves `margin + h`
    // and sizes our window to it. `w` is advisory.
    unsafe { let _ = npk_window_set_panel(EDGE_TOP, BEHAVIOR_STRUT, 1920, 34); }

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
