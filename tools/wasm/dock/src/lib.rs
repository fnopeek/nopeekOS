//! dock — bottom auto-hide app dock.
//!
//! A resident overlay launcher. Declares itself a dock via
//! `npk_window_set_dock`; the compositor owns the slide-in/out reveal
//! (cursor at the bottom edge reveals it, leaving hides it). Renders a
//! centred row of app icons plus a trailing launcher button that opens
//! `drun` for full search. Complementary to `drun`, not a replacement.
//!
//! See DOCK.md for the architecture.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_catalog::{self, AppEntry, EntryKind};
use nopeek_widgets::prefab;
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
    fn npk_spawn_module(ptr: i32, len: i32) -> i32;
    fn npk_run_intent(verb_ptr: i32, verb_len: i32) -> i32;
    fn npk_window_set_dock(w: i32, h: i32) -> i32;
    fn npk_window_set_modal(modal: i32) -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

fn spawn(name: &str) -> bool {
    unsafe { npk_spawn_module(name.as_ptr() as i32, name.len() as i32) == 0 }
}

fn run_intent(verb: &str) -> bool {
    unsafe { npk_run_intent(verb.as_ptr() as i32, verb.len() as i32) == 0 }
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

// Bump allocator — same pattern as drun. The tree is committed once at
// startup and only re-committed if the catalog ever changes, so there's
// no per-frame churn.
const HEAP_SIZE: usize = 256 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct BumpAllocator;

unsafe impl core::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let align = layout.align();
        let size = layout.size();
        let pos_ptr = core::ptr::addr_of_mut!(HEAP_POS);
        let current = unsafe { pos_ptr.read() };
        let aligned = (current + align - 1) & !(align - 1);
        if aligned + size > HEAP_SIZE { return core::ptr::null_mut(); }
        unsafe { pos_ptr.write(aligned + size); }
        let heap_ptr = core::ptr::addr_of_mut!(HEAP) as *mut u8;
        unsafe { heap_ptr.add(aligned) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[dock] panic!");
    loop {}
}

// ActionId encoding:
//   1..LAUNCHER     = app cell click (CLICK_BASE + index)
//   LAUNCHER        = the trailing launcher button → opens drun
const CLICK_BASE: u32 = 1;
const LAUNCHER: u32 = 90_000;

// Visual sizing (px at 1× scale).
const ICON_SIZE: u16 = 28;
const DOCK_HEIGHT: i32 = 72;
const CELL_FOOTPRINT: i32 = 56; // icon + padding + inter-cell gap
const SIDE_PADDING: i32 = 48;

struct Dock {
    entries: Vec<AppEntry>,
}

impl Dock {
    fn load() -> Self {
        // Exclude the dock itself, and drun — the trailing launcher
        // button already opens drun, so a separate drun tile is redundant.
        let catalog = app_catalog::load(&["dock", "drun"]);
        let entries = match read_pins() {
            Some(pins) if !pins.is_empty() => order_by_pins(catalog, &pins),
            _ => catalog,
        };
        Dock { entries }
    }

    /// Total dock width to request from the compositor (it clamps).
    fn width(&self) -> i32 {
        let cells = self.entries.len() as i32 + 1; // + launcher button
        cells * CELL_FOOTPRINT + SIDE_PADDING
    }

    fn render(&self) -> Widget {
        let mut cells: Vec<Widget> = Vec::with_capacity(self.entries.len() + 1);
        for (i, e) in self.entries.iter().enumerate() {
            cells.push(prefab::icon_button(
                e.icon,
                ICON_SIZE,
                Some(ActionId(CLICK_BASE + i as u32)),
                None,
            ));
        }
        // Trailing launcher button → drun (full search).
        cells.push(prefab::icon_button(
            IconId::MagnifyingGlass,
            ICON_SIZE,
            Some(ActionId(LAUNCHER)),
            None,
        ));

        // Flat look: no hard bordered box (the compositor renders the
        // dock chrome-less). The icons sit on a soft, rounded translucent
        // tray that the dock supplies itself.
        Widget::Row {
            children:  cells,
            spacing:   Spacing::Sm.as_u16(),
            align:     Align::Center,
            modifiers: alloc::vec![
                Modifier::Padding(Padding::Sm.as_u16()),
                Modifier::Background(Token::Surface),
                Modifier::Rounded(Radius::Xl.as_u8()),
            ],
        }
    }

    fn commit_tree(&self) {
        match wire::encode(&self.render()) {
            Ok(bytes) => { if commit(&bytes) < 0 { log("[dock] commit failed"); } }
            Err(_) => log("[dock] encode failed"),
        }
    }

    fn launch(&self, idx: usize) {
        if let Some(e) = self.entries.get(idx) {
            let ok = match e.kind {
                EntryKind::Module => spawn(&e.launch_name),
                EntryKind::Intent => run_intent(&e.launch_name),
            };
            if !ok { log("[dock] launch failed"); }
        }
    }

    fn handle(&self, ev: Event) {
        if let Event::Action(ActionId(id)) = ev {
            if id == LAUNCHER {
                let _ = spawn("drun");
            } else if id >= CLICK_BASE {
                self.launch((id - CLICK_BASE) as usize);
            }
        }
    }
}

/// Read `sys/config/dock` — one app name (module or intent) per line.
/// Missing / empty → None (caller falls back to the full catalog).
fn read_pins() -> Option<Vec<String>> {
    const CFG_BUF_SIZE: usize = 4096;
    static mut CFG_BUF: [u8; CFG_BUF_SIZE] = [0; CFG_BUF_SIZE];
    let path = "sys/config/dock";
    let buf_ptr = core::ptr::addr_of_mut!(CFG_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(path.as_ptr() as i32, path.len() as i32, buf_ptr as i32, CFG_BUF_SIZE as i32)
    };
    if n <= 0 { return None; }
    let bytes = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    let text = core::str::from_utf8(bytes).ok()?;
    let mut pins: Vec<String> = Vec::new();
    for line in text.lines() {
        let name = line.trim();
        if !name.is_empty() && !name.starts_with('#') {
            pins.push(name.to_string());
        }
    }
    Some(pins)
}

/// Keep only pinned entries, in pin order. Unmatched pins are skipped.
fn order_by_pins(catalog: Vec<AppEntry>, pins: &[String]) -> Vec<AppEntry> {
    let mut out: Vec<AppEntry> = Vec::with_capacity(pins.len());
    for pin in pins {
        if let Some(e) = catalog.iter().find(|e| &e.launch_name == pin) {
            out.push(e.clone());
        }
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let dock = Dock::load();

    unsafe {
        let _ = npk_window_set_dock(dock.width(), DOCK_HEIGHT);
        let _ = npk_window_set_modal(0);
    }

    dock.commit_tree();

    loop {
        match poll_event() {
            PollResult::Event(ev) => dock.handle(ev),
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            PollResult::WindowGone => return,
        }
    }
}
