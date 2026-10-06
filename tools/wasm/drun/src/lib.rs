//! drun — Mod+D app launcher.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use nopeek_widgets::app_catalog::{self, AppEntry, EntryKind};
use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read + exec + render, and the shell roles: an overlay that takes the
// keyboard focus, modal while open.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::EXEC | caps::RENDER, caps::ext::SHELL];

use nopeek_widgets::host;

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_window_set_overlay(w: i32, h: i32) -> i32;
}

fn window_set_overlay(w: i32, h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_overlay(w, h) };
}

fn log(msg: &str) { host::log_serial(msg); }

const EVENT_BUF_SIZE: usize = 64;

enum PollResult {
    Event(Event),
    Empty,
    WindowGone,
}

fn poll_event(buf: &mut [u8]) -> PollResult {
    match nopeek_widgets::events::poll(buf) {
        nopeek_widgets::events::Poll::Event(ev) => PollResult::Event(ev),
        nopeek_widgets::events::Poll::Empty => PollResult::Empty,
        nopeek_widgets::events::Poll::Gone => PollResult::WindowGone,
    }
}

fn spawn(name: &str) -> bool { host::spawn_module(name) }

fn run_intent(verb: &str) -> bool { host::run_intent(verb) == 0 }

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[drun] panic!");
    core::arch::wasm32::unreachable()
}

// ActionId encoding:
//   0          → reserved sentinel (prefab::NO_ACTION-style; never fired)
//   1..CLICK_BASE+N    = row click  (offset by CLICK_BASE so 0 stays free)
//   HOVER_BASE+N       = row hover
const CLICK_BASE: u32 = 1;
const HOVER_BASE: u32 = 10_000;
const QUERY_CAP: usize = 63;
const MAX_VISIBLE_ROWS: usize = 5;

struct Drun {
    entries:    Vec<AppEntry>,
    filtered:   Vec<usize>,
    selected:   usize,
    row_offset: usize,
    query:      String,
}

impl Drun {
    fn load() -> Self {
        // Installed modules + built-in intents, sorted. Exclude drun
        // itself and the dock (the dock is launcher infrastructure, not a
        // user app — listing it would let a click spawn a second dock).
        let entries = app_catalog::load(&["drun", "dock", "bar", "volume", "pick"]);

        let mut filtered: Vec<usize> = Vec::with_capacity(entries.len().max(1));
        for i in 0..entries.len() { filtered.push(i); }
        let query = String::with_capacity(QUERY_CAP + 1);

        Drun { entries, filtered, selected: 0, row_offset: 0, query }
    }

    fn refilter(&mut self) {
        self.filtered.clear();
        let q = self.query.to_ascii_lowercase();
        for (i, e) in self.entries.iter().enumerate() {
            if q.is_empty() || entry_matches(e, &q) {
                self.filtered.push(i);
            }
        }
        if self.selected >= self.filtered.len() { self.selected = 0; }
        self.row_offset = 0;
    }

    fn ensure_visible(&mut self) {
        if self.selected < self.row_offset {
            self.row_offset = self.selected;
        } else if self.selected >= self.row_offset + MAX_VISIBLE_ROWS {
            self.row_offset = self.selected + 1 - MAX_VISIBLE_ROWS;
        }
    }

    fn render(&self) -> Widget {
        let badge = prefab::badge("drun");
        let search = prefab::input_autofocus(
            &self.query,
            "Type to search apps…",
            prefab::InputKind::Search,
            prefab::NO_ACTION,
            Some(badge),
        );

        let rows: Vec<Widget> = if self.filtered.is_empty() {
            alloc::vec![prefab::empty_state("no matches")]
        } else {
            let end = (self.row_offset + MAX_VISIBLE_ROWS).min(self.filtered.len());
            (self.row_offset..end).map(|ui_idx| {
                let entry_idx = self.filtered[ui_idx];
                let entry = &self.entries[entry_idx];
                prefab::list_row(
                    entry.icon,
                    &entry.display_name,
                    &entry.description,
                    ui_idx == self.selected,
                    Some(ActionId(CLICK_BASE + ui_idx as u32)),
                    Some(ActionId(HOVER_BASE + ui_idx as u32)),
                )
            }).collect()
        };

        let result_text = format_count(self.filtered.len(), self.row_offset, MAX_VISIBLE_ROWS);
        let foot = prefab::footer("↑↓ navigate   ↵ open   esc close", &result_text);

        // Stack the rows in their own tight Column so the inter-row gap
        // doesn't follow the panel's top-level rhythm. Reads as a single
        // grouped list instead of widely-spaced standalone items.
        // Xs padding keeps the selected-row's accent border off the chrome
        // edge — dividers above/below stay near-full-bleed (panel padding
        // only), the list itself reads as an inset block.
        let list = Widget::Column {
            children:  rows,
            spacing:   Spacing::Xxs.as_u16(),
            align:     Align::Stretch,
            modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
        };

        let mut root: Vec<Widget> = Vec::with_capacity(6);
        root.push(search);
        root.push(Widget::Divider);
        root.push(list);
        root.push(Widget::Spacer { flex: 1 });
        root.push(Widget::Divider);
        root.push(foot);
        prefab::panel(root)
    }

    fn commit_tree(&self) {
        let tree = self.render();
        match wire::encode(&tree) {
            Ok(bytes) => { if !host::scene_commit(&bytes) { log("[drun] commit failed"); } }
            Err(_) => log("[drun] encode failed"),
        }
    }

    fn spawn_selected(&self) {
        if let Some(&entry_idx) = self.filtered.get(self.selected) {
            if let Some(entry) = self.entries.get(entry_idx) {
                let ok = match entry.kind {
                    EntryKind::Module => spawn(&entry.launch_name),
                    EntryKind::Intent => run_intent(&entry.launch_name),
                };
                if !ok { log("[drun] launch failed"); }
            }
        }
    }

    fn handle(&mut self, ev: Event) -> Outcome {
        match ev {
            Event::Key(KeyCode::Up) => {
                if self.selected > 0 { self.selected -= 1; }
                self.ensure_visible();
                Outcome::Rerender
            }
            Event::Key(KeyCode::Down) => {
                if self.selected + 1 < self.filtered.len() { self.selected += 1; }
                self.ensure_visible();
                Outcome::Rerender
            }
            Event::Key(KeyCode::Enter) => { self.spawn_selected(); Outcome::Exit }
            Event::Key(KeyCode::Escape) => Outcome::Exit,
            // The compositor owns the search buffer; mirror its value,
            // capped at QUERY_CAP.
            Event::InputChange { value } => {
                self.query.clear();
                // At a character boundary: typed text is UTF-8.
                self.query.push_str(nopeek_widgets::rt::str_clamp(&value, QUERY_CAP));
                self.refilter();
                Outcome::Rerender
            }
            Event::Action(ActionId(id)) => {
                if id >= HOVER_BASE {
                    let ui_idx = (id - HOVER_BASE) as usize;
                    if ui_idx < self.filtered.len() && ui_idx != self.selected {
                        self.selected = ui_idx;
                        self.ensure_visible();
                        return Outcome::Rerender;
                    }
                    Outcome::Idle
                } else if id >= CLICK_BASE {
                    let ui_idx = (id - CLICK_BASE) as usize;
                    if ui_idx < self.filtered.len() {
                        self.selected = ui_idx;
                        self.spawn_selected();
                        Outcome::Exit
                    } else { Outcome::Idle }
                } else {
                    // ActionId(0) — input on_submit sentinel; ignored.
                    Outcome::Idle
                }
            }
            _ => Outcome::Idle,
        }
    }
}

enum Outcome { Idle, Rerender, Exit }

// Case-insensitive substring match over display + launch name. Lives in
// drun (not the SDK) because filtering is launcher-specific; the dock has
// no search box.
fn entry_matches(e: &AppEntry, query_lower: &str) -> bool {
    e.display_name.to_ascii_lowercase().contains(query_lower)
        || e.launch_name.to_ascii_lowercase().contains(query_lower)
}

fn format_count(total: usize, offset: usize, window: usize) -> String {
    let mut s = String::with_capacity(24);
    if total == 0 {
        s.push_str("no results");
        return s;
    }
    if total <= window {
        push_usize(&mut s, total);
        s.push_str(if total == 1 { " result" } else { " results" });
    } else {
        let end = (offset + window).min(total);
        push_usize(&mut s, offset + 1);
        s.push('–');
        push_usize(&mut s, end);
        s.push_str(" of ");
        push_usize(&mut s, total);
    }
    s
}

fn push_usize(s: &mut String, mut n: usize) {
    if n == 0 { s.push('0'); return; }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        s.push(buf[i] as char);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    window_set_overlay(600, 540);
    host::window_set_modal(true);

    let mut drun = Drun::load();
    let mut event_buf = [0u8; EVENT_BUF_SIZE];

    drun.commit_tree();

    loop {
        match poll_event(&mut event_buf) {
            PollResult::Event(ev) => match drun.handle(ev) {
                Outcome::Idle => {}
                Outcome::Rerender => drun.commit_tree(),
                Outcome::Exit => {
                    host::close_widget();
                    return;
                }
            },
            PollResult::Empty => host::sleep_ms(16),
            PollResult::WindowGone => return,
        }
    }
}
