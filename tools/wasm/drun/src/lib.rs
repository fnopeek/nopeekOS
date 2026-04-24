//! drun — Mod+D app launcher.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::{self, IconRef};
use nopeek_widgets::prefab;
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_list_modules(ptr: i32, max: i32) -> i32;
    fn npk_app_meta(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_spawn_module(ptr: i32, len: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_window_set_overlay(w: i32, h: i32) -> i32;
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

const EVENT_BUF_SIZE: usize = 64;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

enum PollResult {
    Event(Event),
    Empty,
    WindowGone,
}

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

fn spawn(name: &str) -> bool {
    unsafe { npk_spawn_module(name.as_ptr() as i32, name.len() as i32) == 0 }
}

fn close_self() {
    unsafe { let _ = npk_close_widget(); }
}

// Bump allocator reset every rerender; state kept across frames must be
// allocated before `persistent_mark` with enough capacity that push()
// never reallocates past the mark.
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
    log("[drun] panic!");
    loop {}
}

fn alloc_reset(pos: usize) {
    unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(pos); }
}

fn alloc_mark() -> usize {
    unsafe { core::ptr::addr_of!(HEAP_POS).read() }
}

// ActionId encoding: 0..HOVER_BASE = row click, HOVER_BASE.. = row hover.
const HOVER_BASE: u32 = 10_000;
const QUERY_CAP: usize = 63;
const MAX_VISIBLE_ROWS: usize = 6;

struct Entry {
    module_name:  String,
    display_name: String,
    description:  String,
    icon:         IconId,
}

struct Drun {
    entries:    Vec<Entry>,
    filtered:   Vec<usize>,
    selected:   usize,
    row_offset: usize,
    query:      String,
}

impl Drun {
    fn load() -> Self {
        const LIST_BUF_SIZE: usize = 4096;
        static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];
        let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
        let n = unsafe { npk_list_modules(buf_ptr as i32, LIST_BUF_SIZE as i32) };

        let mut entries: Vec<Entry> = Vec::new();
        if n > 0 {
            let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
            for chunk in slice.split(|&b| b == 0) {
                if chunk.is_empty() { continue; }
                if let Ok(s) = core::str::from_utf8(chunk) {
                    if s == "drun" { continue; }
                    entries.push(Entry::hydrate(s));
                }
            }
            entries.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        }

        let mut filtered: Vec<usize> = Vec::with_capacity(entries.len().max(1));
        for i in 0..entries.len() { filtered.push(i); }
        let query = String::with_capacity(QUERY_CAP + 1);

        Drun { entries, filtered, selected: 0, row_offset: 0, query }
    }

    fn refilter(&mut self) {
        self.filtered.clear();
        let q = self.query.to_ascii_lowercase();
        for (i, e) in self.entries.iter().enumerate() {
            if q.is_empty() || e.matches(&q) {
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
        let search = prefab::searchbar(&self.query, "Type to search apps…", Some(badge));

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
                    Some(ActionId(ui_idx as u32)),
                    Some(ActionId(HOVER_BASE + ui_idx as u32)),
                )
            }).collect()
        };

        let result_text = format_count(self.filtered.len(), self.row_offset, MAX_VISIBLE_ROWS);
        let foot = prefab::footer("↑↓ navigate   ↵ open   esc close", &result_text);

        let mut root: Vec<Widget> = Vec::with_capacity(rows.len() + 6);
        root.push(search);
        root.push(Widget::Divider);
        root.extend(rows);
        root.push(Widget::Spacer { flex: 1 });
        root.push(Widget::Divider);
        root.push(foot);
        prefab::panel(root)
    }

    fn commit_tree(&self) {
        let tree = self.render();
        match wire::encode(&tree) {
            Ok(bytes) => { if commit(&bytes) < 0 { log("[drun] commit failed"); } }
            Err(_) => log("[drun] encode failed"),
        }
    }

    fn spawn_selected(&self) {
        if let Some(&entry_idx) = self.filtered.get(self.selected) {
            if let Some(entry) = self.entries.get(entry_idx) {
                if !spawn(&entry.module_name) { log("[drun] spawn failed"); }
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
            Event::Key(KeyCode::Backspace) => {
                if self.query.pop().is_some() {
                    self.refilter();
                    Outcome::Rerender
                } else { Outcome::Idle }
            }
            Event::Key(KeyCode::Char(b)) => {
                if b >= 0x20 && b < 0x7F && self.query.len() < QUERY_CAP {
                    self.query.push(b as char);
                    self.refilter();
                    Outcome::Rerender
                } else { Outcome::Idle }
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
                } else {
                    let ui_idx = id as usize;
                    if ui_idx < self.filtered.len() {
                        self.selected = ui_idx;
                        self.spawn_selected();
                        Outcome::Exit
                    } else { Outcome::Idle }
                }
            }
            _ => Outcome::Idle,
        }
    }
}

enum Outcome { Idle, Rerender, Exit }

impl Entry {
    fn hydrate(module_name: &str) -> Self {
        const META_BUF_SIZE: usize = 512;
        static mut META_BUF: [u8; META_BUF_SIZE] = [0; META_BUF_SIZE];
        let buf_ptr = core::ptr::addr_of_mut!(META_BUF) as *mut u8;

        let n = unsafe {
            npk_app_meta(
                module_name.as_ptr() as i32,
                module_name.len() as i32,
                buf_ptr as i32,
                META_BUF_SIZE as i32,
            )
        };

        if n > 0 {
            let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
            if let Ok(meta) = app_meta::decode(slice) {
                return Entry {
                    module_name:  module_name.to_string(),
                    display_name: meta.display_name,
                    description:  meta.description,
                    icon:         icon_ref_to_id(&meta.icon),
                };
            }
        }

        Entry {
            module_name:  module_name.to_string(),
            display_name: module_name.to_string(),
            description:  String::new(),
            icon:         IconId::List,
        }
    }

    fn matches(&self, query_lower: &str) -> bool {
        self.display_name.to_ascii_lowercase().contains(query_lower)
            || self.module_name.to_ascii_lowercase().contains(query_lower)
    }
}

fn icon_ref_to_id(r: &IconRef) -> IconId {
    match r {
        IconRef::Builtin(id) => *id,
        _ => IconId::List,
    }
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
    unsafe {
        let _ = npk_window_set_overlay(680, 540);
        let _ = npk_window_set_modal(1);
    }

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
                unsafe { let _ = npk_sleep(16); }
            }
            PollResult::WindowGone => return,
        }
    }
}
