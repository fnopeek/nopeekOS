//! pick 0.1 — the file dialog, as a portal.
//!
//! The kernel starts this module on `npk_pick` and hands it the request
//! as a launch argument (`"<open|save>\0<start-dir>\0<suggested-name>"`).
//! We browse npkFS, let the user choose, and report the path back with
//! `npk_pick_result`. The kernel turns that into an `Event::Picked` for
//! whoever asked.
//!
//! Two properties are the whole point of doing it this way:
//!
//!   - **We hold READ, the requester doesn't.** An app can offer Open
//!     and Save without the right to walk the filesystem itself.
//!   - **We never write.** Save mode returns a *path*; the requester
//!     does the writing. So this module needs no WRITE, and a bug here
//!     cannot damage a file.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::i18n;
use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read to list directories, render to draw. Deliberately no WRITE (we
// return paths, we don't create files) and no EXEC.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::RENDER];

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_pick_result(path_ptr: i32, path_len: i32) -> i32;
    fn npk_pick_mkdir(path_ptr: i32, path_len: i32) -> i32;
    fn npk_window_set_modal(modal: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

fn close_self() { unsafe { let _ = npk_close_widget(); } }

/// Answer the requester and go away. An empty path means cancelled.
fn answer(path: &str) {
    unsafe { let _ = npk_pick_result(path.as_ptr() as i32, path.len() as i32); }
    close_self();
}

// ── Strings ───────────────────────────────────────────────────────────

struct Strings {
    open_title:   &'static str,
    save_title:   &'static str,
    open_btn:     &'static str,
    save_btn:     &'static str,
    cancel:       &'static str,
    name_hint:    &'static str,
    empty:        &'static str,
    overwrite_t:  &'static str,
    overwrite_b:  &'static str,
    replace:      &'static str,
    hint_open:    &'static str,
    hint_save:    &'static str,
    new_folder:   &'static str,
    name_label:   &'static str,
    folder_title: &'static str,
    folder_hint:  &'static str,
    create:       &'static str,
    folder_click_hint: &'static str,
}

const EN: Strings = Strings {
    open_title:  "Open file",
    save_title:  "Save file",
    open_btn:    "Open",
    save_btn:    "Save",
    cancel:      "Cancel",
    name_hint:   "File name",
    empty:       "This folder is empty",
    overwrite_t: "Replace file?",
    overwrite_b: "{} already exists in this folder.",
    replace:     "Replace",
    hint_open:   "\u{2191}\u{2193} navigate   \u{21b5} open   esc cancel",
    hint_save:   "\u{2191}\u{2193} navigate   \u{21b5} save   esc cancel",
    new_folder:  "New folder",
    name_label:  "Name",
    folder_title: "New folder",
    folder_hint: "Folder name",
    create:      "Create",
    folder_click_hint: "Click the field, then Enter \u{00b7} Esc cancels",
};

const DE: Strings = Strings {
    open_title:  "Datei öffnen",
    save_title:  "Datei speichern",
    open_btn:    "Öffnen",
    save_btn:    "Speichern",
    cancel:      "Abbrechen",
    name_hint:   "Dateiname",
    empty:       "Dieser Ordner ist leer",
    overwrite_t: "Datei ersetzen?",
    overwrite_b: "{} gibt es in diesem Ordner schon.",
    replace:     "Ersetzen",
    hint_open:   "\u{2191}\u{2193} navigieren   \u{21b5} öffnen   esc abbrechen",
    hint_save:   "\u{2191}\u{2193} navigieren   \u{21b5} speichern   esc abbrechen",
    new_folder:  "Neuer Ordner",
    name_label:  "Name",
    folder_title: "Neuer Ordner",
    folder_hint: "Ordnername",
    create:      "Anlegen",
    folder_click_hint: "Ins Feld klicken, dann Enter \u{00b7} Esc bricht ab",
};

fn s() -> &'static Strings {
    match i18n::lang() { i18n::Lang::De => &DE, _ => &EN }
}

fn fill(template: &str, value: &str) -> String {
    match template.find("{}") {
        Some(i) => {
            let mut out = String::with_capacity(template.len() + value.len());
            out.push_str(&template[..i]);
            out.push_str(value);
            out.push_str(&template[i + 2..]);
            out
        }
        None => template.to_string(),
    }
}

// ── Buffers ───────────────────────────────────────────────────────────

const EVENT_BUF_SIZE: usize = 8 * 1024;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

// Directory listings. npkFS names are long; a deep home dir with many
// files needs room. Oversized rather than truncating a listing silently.
const LIST_BUF_SIZE: usize = 256 * 1024;
static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];

// Separate scratch for the per-folder child count — the outer listing is
// still being read out of LIST_BUF while these run.
const COUNT_BUF_SIZE: usize = 64 * 1024;
static mut COUNT_BUF: [u8; COUNT_BUF_SIZE] = [0; COUNT_BUF_SIZE];

const ARG_CAP: usize = 1024;
static mut ARG_BUF: [u8; ARG_CAP] = [0; ARG_CAP];

const NAME_CAP: usize = 128;
static mut NAME_BUF: [u8; NAME_CAP] = [0; NAME_CAP];

// InputChange hands us a heap String that `alloc_reset` frees before
// `handle` runs — copy it out first (the same use-after-free that bit
// spell and loft).
const PAYLOAD_CAP: usize = 1024;
static mut PAYLOAD_BUF: [u8; PAYLOAD_CAP] = [0; PAYLOAD_CAP];

fn copy_payload(s: &str) -> usize {
    let n = s.len().min(PAYLOAD_CAP);
    let dst = core::ptr::addr_of_mut!(PAYLOAD_BUF) as *mut u8;
    unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), dst, n); }
    n
}

fn payload_str(len: usize) -> &'static str {
    let ptr = core::ptr::addr_of!(PAYLOAD_BUF) as *const u8;
    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(slice).unwrap_or("")
}

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

// ── Bump allocator ────────────────────────────────────────────────────

const HEAP_SIZE: usize = 2 * 1024 * 1024;
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
fn panic(_: &core::panic::PanicInfo) -> ! { log("[pick] panic!"); loop {} }

fn alloc_reset(pos: usize) { unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(pos); } }
fn alloc_mark() -> usize { unsafe { core::ptr::addr_of!(HEAP_POS).read() } }

// ── Action ids ────────────────────────────────────────────────────────

const ACT_CONFIRM:   u32 = 1;
const ACT_CANCEL:    u32 = 2;
const ACT_PARENT:    u32 = 3;
const ACT_NAME_SUBMIT: u32 = 4;
const ACT_OVERWRITE: u32 = 5;
const ACT_OVERWRITE_CANCEL: u32 = 6;
const ACT_BACK:       u32 = 7;
const ACT_NEW_FOLDER: u32 = 8;
const ACT_FOLDER_CREATE: u32 = 9;
const ACT_FOLDER_CANCEL: u32 = 10;

// i-th entry in the listing / i-th breadcrumb segment.
const ACT_ENTRY_BASE: u32 = 1_000;
const ACT_CRUMB_BASE: u32 = 5_000;

// ── State ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Open, Save }

struct Entry {
    name:   String,
    is_dir: bool,
    size:   u64,
    /// Folders only: how many entries they hold. None = not counted.
    items:  Option<usize>,
}

struct Pick {
    mode:     Mode,
    /// Directory currently shown (npkFS path, no trailing slash).
    dir:      String,
    entries:  Vec<Entry>,
    /// Index into `entries`, or None when nothing is picked yet.
    selected: Option<usize>,
    /// Save mode: the filename being typed, extension included.
    name:     String,
    /// Directories visited, for the Back arrow.
    history:  Vec<String>,
    /// Save mode: showing the "replace existing file?" confirmation.
    confirm_overwrite: bool,
    /// Naming a new folder. Its own buffer, not `name` — in save mode
    /// that one already holds the filename and must survive the detour.
    new_folder: bool,
    folder_name: String,
}

impl Pick {
    fn new() -> Self {
        // Request wire: "<open|save>\0<start-dir>\0<suggested-name>".
        let arg = read_launch_arg();
        let mut parts = arg.split('\0');
        let mode = match parts.next() {
            Some("save") => Mode::Save,
            _ => Mode::Open,
        };
        let start = parts.next().unwrap_or("").trim();
        let suggest = parts.next().unwrap_or("").trim();

        let dir = if start.is_empty() { read_home_dir() } else { start.to_string() };

        let mut p = Pick {
            mode,
            dir,
            entries:  Vec::new(),
            selected: None,
            // Pre-allocate so typing doesn't reallocate past the
            // persistent mark and get freed by the next alloc_reset.
            name:     String::with_capacity(NAME_CAP),
            history:  Vec::with_capacity(16),
            confirm_overwrite: false,
            new_folder: false,
            folder_name: String::with_capacity(NAME_CAP),
        };
        p.name.push_str(clamp_str(suggest, NAME_CAP));
        p.reload();
        p
    }

    /// Name as it will land on disk — exactly what the field shows.
    fn full_name(&self) -> String {
        self.name.trim().to_string()
    }

    fn reload(&mut self) {
        self.entries = list_dir(&self.dir);
        self.selected = None;
    }

    /// Navigate to `path`, remembering where we came from for Back.
    fn enter_dir(&mut self, path: String) {
        if path != self.dir {
            if self.history.len() < 64 {
                self.history.push(self.dir.clone());
            }
        }
        self.dir = path;
        self.reload();
    }

    fn go_back(&mut self) {
        if let Some(prev) = self.history.pop() {
            self.dir = prev;
            self.reload();
        }
    }

    /// Go up one level. From a top-level directory ("home") that means the
    /// npkFS root, which lists as the empty prefix — not a no-op, or the
    /// user could never leave the branch they started in.
    fn go_parent(&mut self) {
        if self.dir.is_empty() { return; }
        let up = match self.dir.rfind('/') {
            Some(i) => self.dir[..i].to_string(),
            None => String::new(),
        };
        self.enter_dir(up);
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.selected.and_then(|i| self.entries.get(i))
    }

    fn join(&self, name: &str) -> String {
        if self.dir.is_empty() { name.to_string() }
        else { alloc::format!("{}/{}", self.dir, name) }
    }

    /// Act on the current selection: descend into a folder, or return a
    /// file. Files are only selectable in open mode (in save mode they are
    /// shown dimmed, so you can see what you would overwrite).
    fn activate(&mut self) -> bool {
        let (is_dir, name) = match self.selected_entry() {
            Some(e) => (e.is_dir, e.name.clone()),
            None => return false,
        };
        if is_dir {
            let path = self.join(&name);
            self.enter_dir(path);
            return false;
        }
        match self.mode {
            Mode::Open => { answer(&self.join(&name)); true }
            Mode::Save => false,
        }
    }

    /// The confirm button: open the selected file, or commit the typed
    /// name. Returns true when the dialog is done.
    fn confirm(&mut self) -> bool {
        match self.mode {
            Mode::Open => {
                let picked = self.selected_entry().map(|e| (e.is_dir, e.name.clone()));
                match picked {
                    Some((false, name)) => { answer(&self.join(&name)); true }
                    Some((true, _)) => self.activate(),
                    None => false,
                }
            }
            Mode::Save => {
                if !can_commit_name(&self.name) { return false; }
                let name = self.full_name();
                // Warn before clobbering — the requester writes blind, so
                // this is the only place the user can be asked.
                if !self.confirm_overwrite && path_exists(&self.join(&name)) {
                    self.confirm_overwrite = true;
                    return false;
                }
                answer(&self.join(&name));
                true
            }
        }
    }

    /// Create the folder the user just named and step into it — that is
    /// almost always why they made it. Returns false so the dialog stays
    /// open (a new folder is a step towards picking, not the answer).
    fn create_folder(&mut self) -> bool {
        let name = self.folder_name.trim().to_string();
        if !can_commit_name(&name) { return false; }
        let path = self.join(&name);
        let ok = unsafe {
            npk_pick_mkdir(path.as_ptr() as i32, path.len() as i32) == 0
        };
        self.new_folder = false;
        if ok {
            self.enter_dir(path);
        } else {
            log("[pick] mkdir failed");
        }
        false
    }

    /// True if row `i` can hold the selection. In save mode files are on
    /// display but not choosable, so the arrows skip over them rather
    /// than parking on a row that does nothing.
    fn selectable(&self, i: usize) -> bool {
        match self.entries.get(i) {
            Some(e) => e.is_dir || self.mode == Mode::Open,
            None => false,
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() || delta == 0 { return; }
        let last = self.entries.len() as i32 - 1;
        let mut i = match self.selected {
            None => if delta < 0 { last } else { 0 },
            Some(cur) => cur as i32 + delta,
        };
        // Walk in the requested direction until a selectable row turns up;
        // stop at the edge rather than wrapping.
        while i >= 0 && i <= last {
            if self.selectable(i as usize) {
                self.selected = Some(i as usize);
                return;
            }
            i += if delta < 0 { -1 } else { 1 };
        }
    }
}

// ── Filesystem ────────────────────────────────────────────────────────

fn read_launch_arg() -> String {
    let buf_ptr = core::ptr::addr_of_mut!(ARG_BUF) as *mut u8;
    let n = unsafe { npk_launch_arg(buf_ptr as i32, ARG_CAP as i32) };
    if n <= 0 { return String::new(); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    core::str::from_utf8(slice).unwrap_or("").to_string()
}

fn read_home_dir() -> String {
    let buf_ptr = core::ptr::addr_of_mut!(NAME_BUF) as *mut u8;
    let n = unsafe { npk_home_dir(buf_ptr as i32, NAME_CAP as i32) };
    if n <= 0 { return String::from("home"); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    match core::str::from_utf8(slice) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => String::from("home"),
    }
}

/// Immediate children of `dir`, folders first then files, each group
/// sorted by name. Wire per line: `name\0size(8)\0is_dir(1)\0mtime(8)`.
fn list_dir(dir: &str) -> Vec<Entry> {
    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    let n = unsafe {
        npk_fs_list(dir.as_ptr() as i32, dir.len() as i32,
                    buf_ptr as i32, LIST_BUF_SIZE as i32, 0)
    };
    if n <= 0 { return Vec::new(); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };

    let mut out: Vec<Entry> = Vec::new();
    for line in slice.split(|&b| b == b'\n') {
        let nul = match line.iter().position(|&b| b == 0) { Some(i) => i, None => continue };
        let name = match core::str::from_utf8(&line[..nul]) { Ok(s) => s, Err(_) => continue };
        if name.is_empty() || name == ".dir" { continue; }
        let rest = &line[nul + 1..];
        if rest.len() < 10 { continue; }
        let mut size_bytes = [0u8; 8];
        size_bytes.copy_from_slice(&rest[0..8]);
        let is_dir = rest[9] != 0;
        let full = if dir.is_empty() { name.to_string() }
                   else { alloc::format!("{}/{}", dir, name) };
        // Count a folder's children so the right-hand column carries real
        // information. One extra listing per folder — fine for a dialog
        // showing one directory, and it is the number the user wants.
        let items = if is_dir { Some(count_children(&full)) } else { None };
        out.push(Entry {
            name:   name.to_string(),
            is_dir,
            size:   u64::from_le_bytes(size_bytes),
            items,
        });
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    out
}

/// Number of entries directly inside `dir`. Uses its own buffer so it can
/// run while the outer listing is still being parsed out of `LIST_BUF`.
fn count_children(dir: &str) -> usize {
    let buf_ptr = core::ptr::addr_of_mut!(COUNT_BUF) as *mut u8;
    let n = unsafe {
        npk_fs_list(dir.as_ptr() as i32, dir.len() as i32,
                    buf_ptr as i32, COUNT_BUF_SIZE as i32, 0)
    };
    if n <= 0 { return 0; }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    slice.split(|&b| b == b'\n')
        .filter(|line| {
            match line.iter().position(|&b| b == 0) {
                Some(nul) => {
                    let name = &line[..nul];
                    !name.is_empty() && name != b".dir"
                }
                None => false,
            }
        })
        .count()
}

/// Truncate to at most `max` BYTES without splitting a character. Slicing
/// a `&str` mid-codepoint panics, and a panicking widget app freezes the
/// machine — so every cap on a user-supplied name goes through here.
fn clamp_str(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) { end -= 1; }
    &s[..end]
}

fn path_exists(path: &str) -> bool {
    let mut out = [0u8; 17];
    let r = unsafe {
        npk_fs_stat(path.as_ptr() as i32, path.len() as i32, out.as_mut_ptr() as i32)
    };
    r > 0
}

// ── Render ────────────────────────────────────────────────────────────
//
// Follows loft's list language (UI_REFRESH.md §3/§5): 34 px rows, icon +
// name on the left, one mono column on the right carrying real
// information, and a selection that reads as an accent tint plus a 2 px
// leading edge — never a floating outline.

/// Row metrics, matched to loft's list view so the two read as one system.
const ROW_H:   u16 = 34;
const ROW_PAD: u16 = 4;
/// Right-hand column: "4 items" / "12 KB" / "—".
const COL_META_W: u16 = 96;
const FIELD_H: u16 = 30;

fn render(p: &Pick) -> Widget {
    if p.confirm_overwrite {
        return render_overwrite(p);
    }
    if p.new_folder {
        return render_new_folder(p);
    }

    let title = if p.mode == Mode::Save { s().save_title } else { s().open_title };
    let hint = if p.mode == Mode::Save { s().hint_save } else { s().hint_open };

    // Flat in the panel's own Column, with Flex(1) on the Scroll ITSELF.
    // `measure` reports only a 24 px floor for a scroll container on its
    // axis (that's what lets a flex parent size it), so a Flex wrapper
    // around an unflexed Scroll hands the list 24 px and squashes the
    // rows. loft puts the Flex on the Scroll for the same reason.
    let mut children: Vec<Widget> = Vec::with_capacity(9);
    children.push(render_title_bar(title));
    children.push(Widget::Divider);
    children.push(render_toolbar(p));
    children.push(Widget::Divider);
    children.push(render_list(p));

    if p.mode == Mode::Save {
        children.push(Widget::Divider);
        children.push(render_name_field(p));
    }

    children.push(Widget::Divider);
    children.push(render_footer(p, hint));

    prefab::panel(children)
}

/// Title row — icon + title, close button on the right.
fn render_title_bar(title: &str) -> Widget {
    Widget::Row {
        children: alloc::vec![
            Widget::Icon {
                id:        IconId::Folder,
                size:      16,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
            },
            // Body, not Title: Title is 24 px bold and shouted over a
            // dialog whose every other line is 14 px.
            Widget::Text {
                content:   title.to_string(),
                style:     TextStyle::Body,
                modifiers: alloc::vec![],
            },
            Widget::Spacer { flex: 1 },
            prefab::icon_button(IconId::X, 16, Some(ActionId(ACT_CANCEL)), None),
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

/// Back / up / breadcrumb, with "New folder" pinned right.
fn render_toolbar(p: &Pick) -> Widget {
    let can_go_up = !p.dir.is_empty();
    Widget::Row {
        children: alloc::vec![
            nav_icon(IconId::ArrowLeft, ACT_BACK, !p.history.is_empty()),
            nav_icon(IconId::ArrowUp,   ACT_PARENT, can_go_up),
            prefab::breadcrumb(&crumbs(&p.dir)),
            Widget::Spacer { flex: 1 },
            // Only when saving. Opening an existing file has no use for a
            // new directory, and offering it there just invites a stray
            // write on a dialog that is meant to be read-only.
            if p.mode == Mode::Save { new_folder_button() } else { Widget::Spacer { flex: 0 } },
        ],
        spacing:   Spacing::Xs.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

fn new_folder_button() -> Widget {
    Widget::Row {
        children: alloc::vec![
            Widget::Icon {
                id:        IconId::Folders,
                size:      16,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
            },
            Widget::Text {
                content:   s().new_folder.to_string(),
                style:     TextStyle::Body,
                modifiers: alloc::vec![],
            },
        ],
        spacing:   Spacing::Xs.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Rounded(Radius::Sm.as_u8()),
            Modifier::OnClick(ActionId(ACT_NEW_FOLDER)),
            Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceHover),
                Modifier::Rounded(Radius::Sm.as_u8()),
            ]),
        ],
    }
}

/// A toolbar arrow. Disabled ones stay in place (no layout shift) but
/// lose their click target and fade.
fn nav_icon(icon: IconId, action: u32, enabled: bool) -> Widget {
    if enabled {
        prefab::icon_button(icon, 16, Some(ActionId(action)), None)
    } else {
        Widget::Icon {
            id:        icon,
            size:      16,
            modifiers: alloc::vec![
                Modifier::Padding(Padding::Xs.as_u16()),
                Modifier::Tint(Token::OnSurfaceFaint),
            ],
        }
    }
}

/// Path split into clickable segments, so the user can jump back up
/// several levels at once instead of pressing "up" repeatedly.
fn crumbs(dir: &str) -> Vec<(String, ActionId)> {
    let mut out: Vec<(String, ActionId)> = Vec::new();
    for (i, seg) in dir.split('/').filter(|s| !s.is_empty()).enumerate() {
        out.push((seg.to_string(), ActionId(ACT_CRUMB_BASE + i as u32)));
    }
    if out.is_empty() {
        out.push((String::from("/"), ActionId(ACT_CRUMB_BASE)));
    }
    out
}

fn render_list(p: &Pick) -> Widget {
    let mut rows: Vec<Widget> = Vec::with_capacity(p.entries.len() + 1);

    for (i, e) in p.entries.iter().enumerate() {
        rows.push(entry_row(p, e, i));
    }
    if p.entries.is_empty() {
        rows.push(prefab::empty_state(s().empty));
    }

    Widget::Scroll {
        child: alloc::boxed::Box::new(Widget::Column {
            children:  rows,
            spacing:   0,
            align:     Align::Stretch,
            modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
        }),
        axis:      Axis::Vertical,
        // Flex on the Scroll: it swallows the leftover height, which is
        // what pins the name field and buttons to the bottom.
        modifiers: alloc::vec![Modifier::Flex(1)],
    }
}

/// One listing row: icon + name, then a mono column with the item count
/// (folders) or size (files). A chevron marks the selected row.
///
/// In save mode files are shown but NOT selectable — you can see what
/// you would overwrite without the list fighting the name field over
/// what "chosen" means.
fn entry_row(p: &Pick, e: &Entry, i: usize) -> Widget {
    let selectable = e.is_dir || p.mode == Mode::Open;
    let selected = p.selected == Some(i);

    let tint = if !selectable {
        Token::OnSurfaceFaint
    } else if selected {
        Token::Accent
    } else {
        Token::OnSurfaceMuted
    };

    let name_cell = Widget::Row {
        children: alloc::vec![
            Widget::Icon {
                id:        if e.is_dir { IconId::Folder } else { IconId::FileText },
                size:      16,
                modifiers: alloc::vec![Modifier::Tint(tint)],
            },
            Widget::Text {
                content:   e.name.clone(),
                style:     TextStyle::Body,
                modifiers: if selectable {
                    alloc::vec![]
                } else {
                    alloc::vec![Modifier::Tint(Token::OnSurfaceFaint)]
                },
            },
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Flex(1)],
    };

    let mut mods: Vec<Modifier> = alloc::vec![
        Modifier::Padding(Padding::Xs.as_u16()),
        Modifier::MinHeight(ROW_H),
    ];
    if selectable {
        mods.push(Modifier::OnClick(ActionId(ACT_ENTRY_BASE + i as u32)));
        mods.push(Modifier::Hover(alloc::vec![
            Modifier::Background(Token::SurfaceHover),
        ]));
    }
    if selected {
        mods.push(Modifier::Background(Token::AccentMuted));
    }

    // The 2 px edge occupies its space on every row, so nothing shifts
    // sideways when the selection moves (loft's rule).
    let edge = prefab::mark(2, ROW_H - 2 * ROW_PAD,
                            if selected { Some(Token::Accent) } else { None });

    Widget::Row {
        children: alloc::vec![
            edge,
            name_cell,
            meta_cell(e),
            chevron(selected && e.is_dir),
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: mods,
    }
}

/// Right column — item count for folders, size for files. Mono, so the
/// numbers line up down the list.
fn meta_cell(e: &Entry) -> Widget {
    let text = if e.is_dir {
        match e.items {
            Some(n) => {
                let mut s2 = String::with_capacity(12);
                push_usize(&mut s2, n);
                s2.push_str(if n == 1 { " item" } else { " items" });
                s2
            }
            None => String::from("—"),
        }
    } else {
        size_label(e.size)
    };
    Widget::Text {
        content:   text,
        style:     TextStyle::Mono,
        modifiers: alloc::vec![
            Modifier::MinWidth(COL_META_W),
            Modifier::Tint(Token::OnSurfaceFaint),
        ],
    }
}

fn chevron(show: bool) -> Widget {
    Widget::Icon {
        id:        if show { IconId::CaretRight } else { IconId::None },
        size:      16,
        modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
    }
}

/// "Name" label + the field, separated from the list so it reads as the
/// thing you are creating rather than another list entry. A known
/// extension trails the caret dimmed, so it reads as a suffix, not as
/// part of the name you are typing.
fn render_name_field(p: &Pick) -> Widget {
    // One field, whole filename. The extension used to sit beside the
    // Input as its own dimmed Text, but `measure` floors an Input at
    // 120 px so empty fields don't collapse — so on a short name the
    // suffix drifted off to the right of that floor ("test|      .py")
    // and crept back as you typed. Spans on an Input would fix it
    // properly; that needs ABI the widget doesn't have. A single field
    // also lets the user change the extension, which "save as" wants.
    let field: Vec<Widget> = alloc::vec![
        Widget::Input {
            value:       p.name.clone(),
            placeholder: s().name_hint.to_string(),
            on_submit:   ActionId(ACT_NAME_SUBMIT),
            modifiers:   alloc::vec![Modifier::Autofocus, Modifier::Flex(1)],
        },
    ];

    Widget::Row {
        children: alloc::vec![
            Widget::Text {
                content:   s().name_label.to_string(),
                style:     TextStyle::Body,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
            },
            Widget::Row {
                children:  field,
                spacing:   0,
                align:     Align::Center,
                modifiers: alloc::vec![
                    Modifier::Flex(1),
                    Modifier::MinHeight(FIELD_H),
                    Modifier::Padding(Padding::Xs.as_u16()),
                    Modifier::Rounded(Radius::Md.as_u8()),
                    Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
                    Modifier::Focus(alloc::vec![
                        Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
                    ]),
                ],
            },
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

/// Key hints on the left, the two buttons on the right — one row, so the
/// dialog ends on a single line instead of two stacked bands.
fn render_footer(p: &Pick, hint: &str) -> Widget {
    let confirm = if p.mode == Mode::Save { s().save_btn } else { s().open_btn };
    // A button that looks live but does nothing is worse than one that
    // says it can't act yet: save needs a name, open needs a file picked.
    let ready = match p.mode {
        Mode::Save => can_commit_name(&p.name),
        Mode::Open => p.selected_entry().map(|e| !e.is_dir).unwrap_or(false),
    };
    let confirm_btn = if ready {
        prefab::button(confirm, prefab::ButtonStyle::Primary, ActionId(ACT_CONFIRM))
    } else {
        prefab::button(confirm, prefab::ButtonStyle::Ghost, prefab::NO_ACTION)
    };

    Widget::Row {
        children: alloc::vec![
            Widget::Text {
                content:   hint.to_string(),
                style:     TextStyle::Caption,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceFaint)],
            },
            Widget::Spacer { flex: 1 },
            prefab::button(s().cancel, prefab::ButtonStyle::Ghost, ActionId(ACT_CANCEL)),
            confirm_btn,
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

/// A name is committable when it's non-empty and names a file, not a path.
fn can_commit_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.contains('/') && n != "." && n != ".."
}

/// Name-the-new-folder sheet. The list and the save field are hidden
/// while this is up, so there is still exactly one editable widget and
/// `InputChange` stays unambiguous.
fn render_new_folder(p: &Pick) -> Widget {
    prefab::dialog(
        s().folder_title,
        Widget::Column {
            children: alloc::vec![
                // Plain `input`, not the autofocus variant: the compositor
                // only auto-focuses on a window's FIRST commit, and this
                // sheet appears later. Claiming autofocus here would just
                // be a lie in the tree — hence the hint below.
                prefab::input(&p.folder_name, s().folder_hint, prefab::InputKind::Text,
                              ActionId(ACT_FOLDER_CREATE), None),
                Widget::Row {
                    children: alloc::vec![
                        Widget::Spacer { flex: 1 },
                        prefab::button(s().cancel, prefab::ButtonStyle::Ghost,
                                       ActionId(ACT_FOLDER_CANCEL)),
                        prefab::button(s().create, prefab::ButtonStyle::Primary,
                                       ActionId(ACT_FOLDER_CREATE)),
                    ],
                    spacing:   Spacing::Sm.as_u16(),
                    align:     Align::Center,
                    modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
                },
            ],
            spacing:   Spacing::Md.as_u16(),
            align:     Align::Stretch,
            modifiers: alloc::vec![],
        },
        Some(s().folder_click_hint),
        360,
    )
}

fn render_overwrite(p: &Pick) -> Widget {
    prefab::dialog(
        s().overwrite_t,
        Widget::Column {
            children: alloc::vec![
                prefab::body(&fill(s().overwrite_b, &p.full_name())),
                Widget::Row {
                    children: alloc::vec![
                        Widget::Spacer { flex: 1 },
                        prefab::button(s().cancel, prefab::ButtonStyle::Ghost,
                                       ActionId(ACT_OVERWRITE_CANCEL)),
                        prefab::button(s().replace, prefab::ButtonStyle::Destructive,
                                       ActionId(ACT_OVERWRITE)),
                    ],
                    spacing:   Spacing::Sm.as_u16(),
                    align:     Align::Center,
                    modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
                },
            ],
            spacing:   Spacing::Md.as_u16(),
            align:     Align::Stretch,
            modifiers: alloc::vec![],
        },
        None,
        360,
    )
}

fn size_label(bytes: u64) -> String {
    let mut s2 = String::with_capacity(16);
    if bytes < 1024 {
        push_usize(&mut s2, bytes as usize);
        s2.push_str(" B");
    } else if bytes < 1024 * 1024 {
        push_usize(&mut s2, (bytes / 1024) as usize);
        s2.push_str(" KB");
    } else {
        push_usize(&mut s2, (bytes / (1024 * 1024)) as usize);
        s2.push_str(" MB");
    }
    s2
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
    while i > 0 { i -= 1; s.push(buf[i] as char); }
}

// ── Events ────────────────────────────────────────────────────────────

enum Outcome { Idle, Rerender, Done }

fn handle(p: &mut Pick, ev: Event, payload: &str) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => {
            // Back out of a sheet first; only a bare Esc cancels the dialog.
            if p.confirm_overwrite { p.confirm_overwrite = false; return Outcome::Rerender; }
            if p.new_folder { p.new_folder = false; return Outcome::Rerender; }
            answer("");
            Outcome::Done
        }
        Event::Key(KeyCode::Up)   => { p.move_selection(-1); Outcome::Rerender }
        Event::Key(KeyCode::Down) => { p.move_selection(1);  Outcome::Rerender }
        Event::Key(KeyCode::Enter) => {
            if p.new_folder { return finish(p.create_folder()); }
            if p.confirm_overwrite { return finish(p.confirm()); }
            // A folder is always "descend"; anything else confirms.
            match p.selected_entry() {
                Some(e) if e.is_dir => { p.activate(); Outcome::Rerender }
                _ => finish(p.confirm()),
            }
        }
        Event::Key(KeyCode::Backspace) if p.mode == Mode::Open && !p.new_folder => {
            p.go_parent();
            Outcome::Rerender
        }
        Event::InputChange { .. } => {
            // Exactly one editable widget is on screen at a time: the
            // folder sheet hides the save field, so there is no ambiguity
            // about which buffer this belongs to.
            let buf = if p.new_folder { &mut p.folder_name } else { &mut p.name };
            buf.clear();
            buf.push_str(clamp_str(payload, NAME_CAP));
            Outcome::Rerender
        }
        Event::Action(ActionId(id)) => handle_action(p, id),
        _ => Outcome::Idle,
    }
}

fn finish(done: bool) -> Outcome {
    if done { Outcome::Done } else { Outcome::Rerender }
}

fn handle_action(p: &mut Pick, id: u32) -> Outcome {
    match id {
        ACT_CANCEL => { answer(""); Outcome::Done }
        ACT_CONFIRM | ACT_NAME_SUBMIT => finish(p.confirm()),
        ACT_PARENT => { p.go_parent(); Outcome::Rerender }
        ACT_BACK   => { p.go_back();   Outcome::Rerender }
        ACT_NEW_FOLDER => {
            p.folder_name.clear();
            p.new_folder = true;
            Outcome::Rerender
        }
        ACT_FOLDER_CREATE => finish(p.create_folder()),
        ACT_FOLDER_CANCEL => { p.new_folder = false; Outcome::Rerender }
        ACT_OVERWRITE => {
            // Already asked — commit straight through.
            let path = p.join(&p.full_name());
            answer(&path);
            Outcome::Done
        }
        ACT_OVERWRITE_CANCEL => { p.confirm_overwrite = false; Outcome::Rerender }
        _ => {
            if id >= ACT_CRUMB_BASE {
                let want = (id - ACT_CRUMB_BASE) as usize;
                let mut path = String::with_capacity(p.dir.len());
                for (i, seg) in p.dir.split('/').filter(|s| !s.is_empty()).enumerate() {
                    if i > want { break; }
                    if i > 0 { path.push('/'); }
                    path.push_str(seg);
                }
                if path != p.dir { p.enter_dir(path); }
                return Outcome::Rerender;
            }
            if id >= ACT_ENTRY_BASE {
                let i = (id - ACT_ENTRY_BASE) as usize;
                if i < p.entries.len() {
                    // First click selects; clicking the selected row again
                    // activates it (descend / choose) — no double-click
                    // event exists, and this keeps one click reversible.
                    if p.selected == Some(i) {
                        if p.activate() { return Outcome::Done; }
                    } else {
                        p.selected = Some(i);
                    }
                    return Outcome::Rerender;
                }
            }
            Outcome::Idle
        }
    }
}

// ── Main loop ─────────────────────────────────────────────────────────

fn commit_tree(p: &Pick) {
    let tree = render(p);
    // Always through `wire::encode` — a bare postcard payload is rejected
    // by the compositor and the window stays blank.
    match wire::encode(&tree) {
        Ok(bytes) => { if commit(&bytes) < 0 { log("[pick] commit failed"); } }
        Err(_) => log("[pick] encode failed"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // The kernel already made this window a centred overlay; modal keeps
    // stray keystrokes out of the app behind us while a dialog is up.
    unsafe { let _ = npk_window_set_modal(1); }

    let mut p = Pick::new();
    let mut persistent_mark = alloc_mark();

    commit_tree(&p);

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                let plen = match &ev {
                    Event::InputChange { value } => copy_payload(value),
                    _ => 0,
                };
                alloc_reset(persistent_mark);
                let outcome = handle(&mut p, ev, payload_str(plen));
                persistent_mark = alloc_mark();
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Rerender => commit_tree(&p),
                    // `answer` already reported and closed the window.
                    Outcome::Done => return,
                }
            }
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            PollResult::WindowGone => return,
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_icon_ref(_: IconRef) {}
