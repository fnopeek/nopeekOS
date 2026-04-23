//! drun — Mod+D app launcher. First interactive widget app for nopeekOS.
//!
//! Flow:
//!   1. Enumerate installed modules via `npk_list_modules`.
//!   2. Commit a widget tree: a centred panel with a Title + one Row
//!      per module (accent background on the selected one).
//!   3. Loop on `npk_event_poll` for `Event::Key`:
//!        Up/Down   → change selection, re-commit.
//!        Enter     → `npk_spawn_module(selected)` + close self.
//!        Escape    → close self.
//!   4. On close: `npk_close_widget()` tears down the window, then we
//!      drop out of the poll loop and `_start` returns.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use nopeek_widgets::*;

// ── Host bindings ─────────────────────────────────────────────────────

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_list_modules(ptr: i32, max: i32) -> i32;
    fn npk_spawn_module(ptr: i32, len: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_log(ptr: i32, len: i32);
}

fn log(msg: &str) {
    unsafe { npk_log(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

// Scratch buffer for event decoding — postcard-encoded Event is tiny,
// 64 bytes covers every variant with headroom.
const EVENT_BUF_SIZE: usize = 64;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

enum PollResult {
    Event(Event),
    Empty,
    WindowGone,
}

fn poll_event() -> PollResult {
    let buf_ptr = core::ptr::addr_of_mut!(EVENT_BUF) as *mut u8;
    let buf_len = EVENT_BUF_SIZE;
    let n = unsafe { npk_event_poll(buf_ptr as i32, buf_len as i32) };
    if n < 0 { return PollResult::WindowGone; }
    if n == 0 { return PollResult::Empty; }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    match postcard::from_bytes::<Event>(slice) {
        Ok(ev) => PollResult::Event(ev),
        Err(_) => PollResult::Empty,
    }
}

fn spawn(name: &str) -> bool {
    let r = unsafe {
        npk_spawn_module(name.as_ptr() as i32, name.len() as i32)
    };
    r == 0
}

fn close_self() {
    unsafe { let _ = npk_close_widget(); }
}

// ── Bump allocator (same pattern as files-stub) ────────────────────────

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
        if aligned + size > HEAP_SIZE {
            return core::ptr::null_mut();
        }
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
    log("[drun] panic!");
    loop {}
}

/// Snapshot drun's own bump-allocator position + heap pointer. We use
/// this to free every allocation made while building the previous
/// frame's widget tree — rebuilding on every keystroke would otherwise
/// exhaust the 256 KB heap within a few presses.
fn alloc_reset(pos: usize) {
    unsafe {
        core::ptr::addr_of_mut!(HEAP_POS).write(pos);
    }
}

fn alloc_mark() -> usize {
    unsafe { core::ptr::addr_of!(HEAP_POS).read() }
}

// ── State ─────────────────────────────────────────────────────────────

struct Drun {
    modules: Vec<String>,
    selected: usize,
}

impl Drun {
    fn load() -> Self {
        // NUL-separated list; module names are printable ASCII.
        const LIST_BUF_SIZE: usize = 4096;
        static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];
        let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
        let n = unsafe { npk_list_modules(buf_ptr as i32, LIST_BUF_SIZE as i32) };
        let mut modules: Vec<String> = Vec::new();
        if n > 0 {
            let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
            for chunk in slice.split(|&b| b == 0) {
                if chunk.is_empty() { continue; }
                if let Ok(s) = core::str::from_utf8(chunk) {
                    // Don't list drun itself in its own launcher.
                    if s == "drun" { continue; }
                    modules.push(s.to_string());
                }
            }
            modules.sort();
        }
        Drun { modules, selected: 0 }
    }

    fn render(&self) -> Widget {
        let mut rows: Vec<Widget> = Vec::with_capacity(self.modules.len() + 1);

        // Title stays at the top.
        rows.push(Widget::Text {
            content: "Launch…".to_string(),
            style: TextStyle::Title,
            modifiers: vec![Modifier::Padding(6)],
        });

        if self.modules.is_empty() {
            rows.push(Widget::Text {
                content: "no modules installed".to_string(),
                style: TextStyle::Muted,
                modifiers: vec![Modifier::Padding(4)],
            });
        } else {
            for (i, name) in self.modules.iter().enumerate() {
                let is_sel = i == self.selected;
                let mut row_mods: Vec<Modifier> = vec![Modifier::Padding(6)];
                if is_sel {
                    row_mods.push(Modifier::Background(Token::Accent));
                }
                rows.push(Widget::Row {
                    children: vec![
                        Widget::Text {
                            content: name.clone(),
                            style: TextStyle::Body,
                            modifiers: vec![],
                        },
                    ],
                    spacing: 0,
                    align: Align::Center,
                    modifiers: row_mods,
                });
            }
        }

        // Footer hint.
        rows.push(Widget::Text {
            content: "↑↓  Enter  Esc".to_string(),
            style: TextStyle::Caption,
            modifiers: vec![Modifier::Padding(6), Modifier::Opacity(160)],
        });

        Widget::Column {
            children: rows,
            spacing: 2,
            align: Align::Stretch,
            modifiers: vec![
                Modifier::Background(Token::SurfaceElevated),
                Modifier::Padding(8),
            ],
        }
    }

    fn commit_tree(&self) {
        let tree = self.render();
        match wire::encode(&tree) {
            Ok(bytes) => {
                let r = commit(&bytes);
                if r < 0 { log("[drun] commit failed"); }
            }
            Err(_) => log("[drun] encode failed"),
        }
    }

    /// True if the event implies we should exit; the caller closes the
    /// widget + returns from `_start`.
    fn handle(&mut self, ev: Event) -> Outcome {
        match ev {
            Event::Key(KeyCode::Up) => {
                if self.selected > 0 { self.selected -= 1; }
                Outcome::Rerender
            }
            Event::Key(KeyCode::Down) => {
                if self.selected + 1 < self.modules.len() {
                    self.selected += 1;
                }
                Outcome::Rerender
            }
            Event::Key(KeyCode::Enter) => {
                if let Some(name) = self.modules.get(self.selected) {
                    // Clone out of `self` before we drop the borrow —
                    // `spawn` is a host call, doesn't touch our tree.
                    let name = name.clone();
                    if !spawn(&name) {
                        log("[drun] spawn failed");
                        return Outcome::Idle;
                    }
                }
                Outcome::Exit
            }
            Event::Key(KeyCode::Escape) => Outcome::Exit,
            _ => Outcome::Idle,
        }
    }
}

enum Outcome {
    Idle,
    Rerender,
    Exit,
}

// ── Entry point ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Reserve the first 128 KB for persistent state (module list, etc.)
    // and rewind to this mark before every re-render so we don't leak.
    let mut drun = Drun::load();
    let persistent_mark = alloc_mark();

    drun.commit_tree();

    loop {
        match poll_event() {
            PollResult::Event(ev) => match drun.handle(ev) {
                Outcome::Idle => {}
                Outcome::Rerender => {
                    alloc_reset(persistent_mark);
                    drun.commit_tree();
                }
                Outcome::Exit => {
                    close_self();
                    return;
                }
            },
            PollResult::Empty => {
                // No event — yield. We don't have npk_event_wait yet, so
                // a tight loop would melt a core. Do a bounded spin that
                // matches the intent loop's cadence (~5000 pauses).
                for _ in 0..5_000 {
                    core::hint::spin_loop();
                }
            }
            PollResult::WindowGone => {
                // Shade tore down our window (e.g. Mod+Shift+Q). Exit
                // cleanly so the worker core returns from _start.
                return;
            }
        }
    }
}

