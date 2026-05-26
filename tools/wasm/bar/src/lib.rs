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
    fn npk_workspace_switch(n: i32) -> i32;
    fn npk_power() -> i32;
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
        right:  ["tray", "power"].iter().map(|s| s.to_string()).collect(),
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

struct BarState<'a> { clock: &'a str, ws_count: u8, ws_active: u8, title: &'a str }

fn parse_state(s: &str) -> BarState<'_> {
    let mut it = s.splitn(4, '\n');
    let clock = it.next().unwrap_or("");
    let ws_count = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let ws_active = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let title = it.next().unwrap_or("");
    BarState { clock, ws_count, ws_active, title }
}

// ── Segment → widgets ────────────────────────────────────────────────
// No per-widget Padding (the bar is short — the enclosing card supplies
// the inset; uniform Padding here would overflow the band vertically).
fn segment_widgets(name: &str, st: &BarState) -> Vec<Widget> {
    match name {
        "workspaces" => {
            let mut row = Vec::new();
            for i in 0..st.ws_count {
                if i > 0 {
                    // Thin separator between workspace numbers.
                    row.push(Widget::Text {
                        content: "|".to_string(),
                        style: TextStyle::Body,
                        modifiers: Vec::new(),
                    });
                }
                let active = i == st.ws_active;
                let mut mods: Vec<Modifier> = Vec::new();
                mods.push(Modifier::Padding(Padding::Xs.as_u16()));
                mods.push(Modifier::OnClick(ActionId(WS_BASE + i as u32)));
                if active {
                    // Clear filled chip for the active workspace.
                    mods.push(Modifier::Background(Token::Accent));
                    mods.push(Modifier::Rounded(Radius::Sm.as_u8()));
                } else {
                    mods.push(Modifier::Hover(alloc::vec![
                        Modifier::Background(Token::SurfaceMuted),
                        Modifier::Rounded(Radius::Sm.as_u8()),
                    ]));
                }
                row.push(Widget::Text {
                    content: alloc::format!("{}", i + 1),
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
        "title" => {
            if st.title.is_empty() { Vec::new() }
            else {
                alloc::vec![Widget::Text {
                    content: st.title.to_string(),
                    style: TextStyle::Body,
                    modifiers: Vec::new(),
                }]
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
        "tray" => alloc::vec![Widget::Icon {
            id: IconId::Gear,
            size: ICON_SIZE,
            modifiers: Vec::new(),
        }],
        "power" => alloc::vec![Widget::Icon {
            id: IconId::Power,
            size: ICON_SIZE,
            modifiers: alloc::vec![
                Modifier::Tint(Token::Danger),
                Modifier::OnClick(ActionId(POWER)),
            ],
        }],
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
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: alloc::vec![
            Modifier::Background(Token::SurfaceElevated),
            // Square fill in the scene; the compositor rounds each pill
            // cleanly with its SDF (like the dock window) — rasteriser
            // rounding here would double up + reintroduce corner-AA cruft.
            Modifier::Padding(Padding::Xs.as_u16()),
        ],
    }
}

fn build_tree(seg: &Segments, st: &BarState) -> Widget {
    // Root has NO background/padding: the gaps between the three pills stay
    // the scene's Surface clear, which the compositor's is_bar blit treats
    // as transparent so the wallpaper shows through. Flex spacers detach
    // the pills (left / centre / right).
    Widget::Row {
        children: alloc::vec![
            card(&seg.left, st),
            Widget::Spacer { flex: 1 },
            card(&seg.center, st),
            Widget::Spacer { flex: 1 },
            card(&seg.right, st),
        ],
        spacing: Spacing::Sm.as_u16(),
        // Stretch: every pill takes the full bar height so they're all the
        // same size (the clock pill was shorter than the others).
        align: Align::Stretch,
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
    let cur = unsafe { core::slice::from_raw_parts(p as *const u8, n) };
    let last = unsafe {
        let lp = core::ptr::addr_of!(LAST_BUF) as *const u8;
        let ll = LAST_LEN;
        if ll == usize::MAX { None } else { Some(core::slice::from_raw_parts(lp, ll)) }
    };
    if last == Some(cur) { return None; }
    unsafe {
        let lp = core::ptr::addr_of_mut!(LAST_BUF) as *mut u8;
        core::ptr::copy_nonoverlapping(p, lp, n);
        LAST_LEN = n;
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
    let st = parse_state(s);
    let tree = build_tree(seg, &st);
    match wire::encode(&tree) {
        Ok(bytes) => { if commit(&bytes) < 0 { log("[bar] commit failed"); } }
        Err(_) => log("[bar] encode failed"),
    }
}

fn handle(ev: Event) {
    if let Event::Action(ActionId(id)) = ev {
        if id == POWER {
            unsafe { let _ = npk_power(); }
        } else if id >= WS_BASE && id < POWER {
            unsafe { let _ = npk_workspace_switch((id - WS_BASE) as i32); }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let seg = read_segments();
    unsafe { HEAP_MARK = HEAP_POS; }   // freeze config; tree rebuilds above this

    // Declare the top strut panel. w/h are advisory — the compositor sizes
    // the bar to its band.
    unsafe { let _ = npk_window_set_panel(EDGE_TOP, BEHAVIOR_STRUT, 1920, 28); }

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
