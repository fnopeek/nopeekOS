//! drun — Mod+D app launcher. First interactive widget app for nopeekOS.
//!
//! v0.3 layout: search bar at the top with live-typing filter, app rows
//! (icon + name), footer with keybind hints + result counter. Mouse
//! hover/click supported in addition to keyboard nav.
//!
//! Allocator note: the bump allocator is reset to `persistent_mark`
//! before every re-render, so anything stored across frames (module
//! list, query buffer, filtered indices) MUST be allocated before the
//! mark, with enough capacity that no subsequent `push` triggers a
//! realloc into the post-mark region.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use nopeek_widgets::app_meta::{self, IconRef};
use nopeek_widgets::*;

// Embed drun's own AppMeta as a WASM custom section so the installer
// caches it, same as every other app. build.rs generates the bytes.
#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// ── Host bindings ─────────────────────────────────────────────────────

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
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

// Scratch buffer for event decoding — postcard-encoded Event is tiny.
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
    let r = unsafe { npk_spawn_module(name.as_ptr() as i32, name.len() as i32) };
    r == 0
}

fn close_self() {
    unsafe { let _ = npk_close_widget(); }
}

// ── Bump allocator ────────────────────────────────────────────────────

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

fn alloc_reset(pos: usize) {
    unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(pos); }
}

fn alloc_mark() -> usize {
    unsafe { core::ptr::addr_of!(HEAP_POS).read() }
}

// ── Action-ID scheme ──────────────────────────────────────────────────
// 0..=9_999     → row click (index into `filtered`)
// 10_000..      → row hover (index = id - 10_000)

const HOVER_BASE: u32 = 10_000;
const QUERY_CAP: usize = 63;

// ── State ─────────────────────────────────────────────────────────────

/// One launchable entry, hydrated with its cached [`AppMeta`] when
/// available (otherwise sensible fallbacks derived from the module
/// name).
struct Entry {
    module_name:  String,   // npkFS key name — passed to npk_spawn_module
    display_name: String,   // e.g. "Drun" — title row, may fall back to module_name
    description:  String,   // subtitle, may be empty for metaless modules
    icon:         IconId,   // rendered to the left of the title
}

struct Drun {
    entries:  Vec<Entry>,  // all installable modules, sorted by display_name
    filtered: Vec<usize>,  // indices into `entries` matching `query`
    selected: usize,       // index into `filtered`
    query:    String,      // pre-allocated capacity QUERY_CAP
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
                    // drun itself never appears in its own launcher.
                    if s == "drun" { continue; }
                    entries.push(Entry::hydrate(s));
                }
            }
            entries.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        }

        // Pre-size filtered to the full entry count — every subsequent
        // `refilter()` only clear+push within that capacity, never
        // reallocates (which would land past persistent_mark).
        let mut filtered: Vec<usize> = Vec::with_capacity(entries.len().max(1));
        for i in 0..entries.len() { filtered.push(i); }

        // Same principle for the query buffer.
        let query = String::with_capacity(QUERY_CAP + 1);

        Drun { entries, filtered, selected: 0, query }
    }

    fn refilter(&mut self) {
        self.filtered.clear();
        let q = self.query.to_ascii_lowercase();
        for (i, e) in self.entries.iter().enumerate() {
            if q.is_empty() || e.matches(&q) {
                self.filtered.push(i);
            }
        }
        if self.selected >= self.filtered.len() {
            self.selected = 0;
        }
    }

    fn render(&self) -> Widget {
        let mut root: Vec<Widget> = Vec::with_capacity(self.filtered.len() + 3);

        // ── Search bar ────────────────────────────────────────────────
        let (query_text, query_style) = if self.query.is_empty() {
            ("Type to search apps…".to_string(), TextStyle::Muted)
        } else {
            (self.query.clone(), TextStyle::Body)
        };
        root.push(Widget::Row {
            children: vec![
                Widget::Icon { id: IconId::MagnifyingGlass, size: 20, modifiers: vec![] },
                Widget::Text {
                    content: query_text,
                    style:   query_style,
                    modifiers: vec![],
                },
                Widget::Spacer { flex: 1 },
                // "drun" badge — hints which launcher is active.
                Widget::Text {
                    content: "drun".to_string(),
                    style:   TextStyle::Muted,
                    modifiers: vec![
                        Modifier::Padding(4),
                        Modifier::Background(Token::SurfaceMuted),
                        Modifier::Border { token: Token::Border, width: 1, radius: 4 },
                    ],
                },
            ],
            spacing: 10,
            align:   Align::Center,
            modifiers: vec![
                Modifier::Padding(10),
                Modifier::Background(Token::SurfaceElevated),
                Modifier::Border { token: Token::Border, width: 1, radius: 8 },
            ],
        });

        // ── Row list ──────────────────────────────────────────────────
        if self.filtered.is_empty() {
            root.push(Widget::Text {
                content: "no matches".to_string(),
                style:   TextStyle::Muted,
                modifiers: vec![Modifier::Padding(14)],
            });
        } else {
            for (ui_idx, &entry_idx) in self.filtered.iter().enumerate() {
                let is_sel = ui_idx == self.selected;
                root.push(self.render_row(ui_idx, &self.entries[entry_idx], is_sel));
            }
        }

        // ── Footer ────────────────────────────────────────────────────
        let result_text = if self.filtered.len() == 1 {
            "1 result".to_string()
        } else {
            let mut s = String::with_capacity(12);
            push_u32(&mut s, self.filtered.len() as u32);
            s.push_str(" results");
            s
        };
        root.push(Widget::Row {
            children: vec![
                Widget::Text {
                    content: "↑↓ navigate   ↵ open   esc close".to_string(),
                    style:   TextStyle::Muted,
                    modifiers: vec![],
                },
                Widget::Spacer { flex: 1 },
                Widget::Text {
                    content: result_text,
                    style:   TextStyle::Muted,
                    modifiers: vec![],
                },
            ],
            spacing: 0,
            align:   Align::Center,
            modifiers: vec![Modifier::Padding(8)],
        });

        Widget::Column {
            children: root,
            spacing: 4,
            align:   Align::Stretch,
            modifiers: vec![
                Modifier::Background(Token::Surface),
                Modifier::Padding(12),
                Modifier::Border { token: Token::Border, width: 1, radius: 12 },
            ],
        }
    }

    fn render_row(&self, ui_idx: usize, entry: &Entry, is_sel: bool) -> Widget {
        let mut row_mods: Vec<Modifier> = Vec::with_capacity(4);
        row_mods.push(Modifier::Padding(10));
        row_mods.push(Modifier::OnClick(ActionId(ui_idx as u32)));
        row_mods.push(Modifier::OnHover(ActionId(HOVER_BASE + ui_idx as u32)));
        if is_sel {
            row_mods.push(Modifier::Background(Token::AccentMuted));
            row_mods.push(Modifier::Border {
                token:  Token::Accent,
                width:  1,
                radius: 6,
            });
        }

        // Title + (optional) subtitle in a vertical column.
        let mut title_col: Vec<Widget> = Vec::with_capacity(2);
        title_col.push(Widget::Text {
            content:   entry.display_name.clone(),
            style:     TextStyle::Body,
            modifiers: vec![],
        });
        if !entry.description.is_empty() {
            title_col.push(Widget::Text {
                content:   entry.description.clone(),
                style:     TextStyle::Muted,
                modifiers: vec![],
            });
        }

        let mut children: Vec<Widget> = Vec::with_capacity(4);
        children.push(Widget::Icon {
            id:        entry.icon,
            size:      22,
            modifiers: vec![],
        });
        children.push(Widget::Column {
            children:  title_col,
            spacing:   2,
            align:     Align::Start,
            modifiers: vec![],
        });
        children.push(Widget::Spacer { flex: 1 });
        if is_sel {
            children.push(Widget::Icon {
                id:        IconId::ArrowRight,
                size:      14,
                modifiers: vec![],
            });
        }

        Widget::Row {
            children,
            spacing: 12,
            align:   Align::Center,
            modifiers: row_mods,
        }
    }

    fn commit_tree(&self) {
        let tree = self.render();
        match wire::encode(&tree) {
            Ok(bytes) => {
                if commit(&bytes) < 0 { log("[drun] commit failed"); }
            }
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
                Outcome::Rerender
            }
            Event::Key(KeyCode::Down) => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
                Outcome::Rerender
            }
            Event::Key(KeyCode::Enter) => {
                self.spawn_selected();
                Outcome::Exit
            }
            Event::Key(KeyCode::Escape) => Outcome::Exit,
            Event::Key(KeyCode::Backspace) => {
                if self.query.pop().is_some() {
                    self.refilter();
                    Outcome::Rerender
                } else {
                    Outcome::Idle
                }
            }
            Event::Key(KeyCode::Char(b)) => {
                if b >= 0x20 && b < 0x7F && self.query.len() < QUERY_CAP {
                    self.query.push(b as char);
                    self.refilter();
                    Outcome::Rerender
                } else {
                    Outcome::Idle
                }
            }
            Event::Action(ActionId(id)) => {
                if id >= HOVER_BASE {
                    let ui_idx = (id - HOVER_BASE) as usize;
                    if ui_idx < self.filtered.len() && ui_idx != self.selected {
                        self.selected = ui_idx;
                        return Outcome::Rerender;
                    }
                    Outcome::Idle
                } else {
                    let ui_idx = id as usize;
                    if ui_idx < self.filtered.len() {
                        self.selected = ui_idx;
                        self.spawn_selected();
                        Outcome::Exit
                    } else {
                        Outcome::Idle
                    }
                }
            }
            _ => Outcome::Idle,
        }
    }
}

enum Outcome {
    Idle,
    Rerender,
    Exit,
}

impl Entry {
    /// Build an entry for a given module name, pulling cached metadata
    /// from the kernel's `sys/meta/<name>` store via `npk_app_meta`.
    /// Falls back to sensible defaults (module name + List icon + no
    /// description) when the module was built without an `.npk.app_meta`
    /// section.
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

        // No meta (either n == 0, n < 0, or decode failed) → fallback.
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
        // Non-exhaustive → future variants fall through to a safe default.
        _ => IconId::List,
    }
}

fn push_u32(s: &mut String, mut n: u32) {
    if n == 0 { s.push('0'); return; }
    let mut buf = [0u8; 10];
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

// ── Entry point ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    unsafe {
        let _ = npk_window_set_overlay(560, 500);
        let _ = npk_window_set_modal(1);
    }
    log("[drun] overlay+modal configured");

    let mut drun = Drun::load();
    let persistent_mark = alloc_mark();

    drun.commit_tree();
    log("[drun] first tree committed, entering poll loop");

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
                for _ in 0..5_000 {
                    core::hint::spin_loop();
                }
            }
            PollResult::WindowGone => return,
        }
    }
}
