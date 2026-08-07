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
    parent:       &'static str,
    overwrite_t:  &'static str,
    overwrite_b:  &'static str,
    replace:      &'static str,
    folder:       &'static str,
    hint_open:    &'static str,
    hint_save:    &'static str,
}

const EN: Strings = Strings {
    open_title:  "Open file",
    save_title:  "Save file",
    open_btn:    "Open",
    save_btn:    "Save",
    cancel:      "Cancel",
    name_hint:   "File name",
    empty:       "This folder is empty",
    parent:      "Parent folder",
    overwrite_t: "Replace file?",
    overwrite_b: "{} already exists in this folder.",
    replace:     "Replace",
    folder:      "Folder",
    hint_open:   "\u{2191}\u{2193} navigate   \u{21b5} open   esc cancel",
    hint_save:   "\u{2191}\u{2193} navigate   \u{21b5} save   esc cancel",
};

const DE: Strings = Strings {
    open_title:  "Datei öffnen",
    save_title:  "Datei speichern",
    open_btn:    "Öffnen",
    save_btn:    "Speichern",
    cancel:      "Abbrechen",
    name_hint:   "Dateiname",
    empty:       "Dieser Ordner ist leer",
    parent:      "Übergeordneter Ordner",
    overwrite_t: "Datei ersetzen?",
    overwrite_b: "{} gibt es in diesem Ordner schon.",
    replace:     "Ersetzen",
    folder:      "Ordner",
    hint_open:   "\u{2191}\u{2193} navigieren   \u{21b5} öffnen   esc abbrechen",
    hint_save:   "\u{2191}\u{2193} navigieren   \u{21b5} speichern   esc abbrechen",
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
}

struct Pick {
    mode:     Mode,
    /// Directory currently shown (npkFS path, no trailing slash).
    dir:      String,
    entries:  Vec<Entry>,
    /// Index into `entries`, or None when nothing is picked yet.
    selected: Option<usize>,
    /// Save mode: the filename being typed.
    name:     String,
    /// Save mode: showing the "replace existing file?" confirmation.
    confirm_overwrite: bool,
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
            confirm_overwrite: false,
        };
        p.name.push_str(clamp_str(suggest, NAME_CAP));
        p.reload();
        p
    }

    fn reload(&mut self) {
        self.entries = list_dir(&self.dir);
        self.selected = None;
    }

    fn enter_dir(&mut self, path: String) {
        self.dir = path;
        self.reload();
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
    /// file. In save mode a click on a file adopts its name rather than
    /// returning it, so the user can overwrite by pointing at it.
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
            Mode::Save => {
                self.name.clear();
                self.name.push_str(clamp_str(&name, NAME_CAP));
                false
            }
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
                let name = self.name.trim().to_string();
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

    fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() { return; }
        let last = self.entries.len() - 1;
        self.selected = Some(match self.selected {
            None => if delta < 0 { last } else { 0 },
            Some(i) => {
                let i = i as i32 + delta;
                if i < 0 { 0 } else if i as usize > last { last } else { i as usize }
            }
        });
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
        out.push(Entry {
            name:   name.to_string(),
            is_dir: rest[9] != 0,
            size:   u64::from_le_bytes(size_bytes),
        });
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    out
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

fn render(p: &Pick) -> Widget {
    if p.confirm_overwrite {
        return render_overwrite(p);
    }

    let title = if p.mode == Mode::Save { s().save_title } else { s().open_title };
    let hint = if p.mode == Mode::Save { s().hint_save } else { s().hint_open };

    // Flat in the panel's own Column, with Flex(1) on the Scroll ITSELF.
    // `measure` reports only a 24 px floor for a scroll container on its
    // axis (that's what lets a flex parent size it), so a Flex wrapper
    // around an unflexed Scroll hands the list 24 px and squashes the
    // rows. loft puts the Flex on the Scroll for the same reason.
    let mut children: Vec<Widget> = Vec::with_capacity(8);
    children.push(prefab::title_bar(title));
    children.push(Widget::Divider);
    children.push(prefab::breadcrumb(&crumbs(&p.dir)));
    children.push(render_list(p));

    if p.mode == Mode::Save {
        children.push(Widget::Divider);
        children.push(prefab::input_autofocus(
            &p.name,
            s().name_hint,
            prefab::InputKind::Text,
            ActionId(ACT_NAME_SUBMIT),
            None,
        ));
    }

    children.push(Widget::Divider);
    children.push(render_buttons(p));
    children.push(prefab::footer(hint, &count_label(p)));

    prefab::panel(children)
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

    // "Up" is a row rather than a toolbar button so keyboard and mouse
    // navigate the same single list.
    if !p.dir.is_empty() {
        rows.push(prefab::list_row(
            IconId::ArrowUp, "..", s().parent, false,
            Some(ActionId(ACT_PARENT)), None,
        ));
    }

    for (i, e) in p.entries.iter().enumerate() {
        let subtitle = if e.is_dir { s().folder.to_string() } else { size_label(e.size) };
        rows.push(prefab::list_row(
            if e.is_dir { IconId::Folder } else { IconId::FileText },
            &e.name,
            &subtitle,
            p.selected == Some(i),
            Some(ActionId(ACT_ENTRY_BASE + i as u32)),
            None,
        ));
    }

    if p.entries.is_empty() {
        rows.push(prefab::empty_state(s().empty));
    }

    Widget::Scroll {
        child: alloc::boxed::Box::new(Widget::Column {
            children:  rows,
            spacing:   Spacing::Xxs.as_u16(),
            align:     Align::Stretch,
            modifiers: alloc::vec![],
        }),
        axis:      Axis::Vertical,
        // Flex on the Scroll: it swallows the leftover height, which is
        // what pins the name field and buttons to the bottom.
        modifiers: alloc::vec![Modifier::Flex(1)],
    }
}

fn render_buttons(p: &Pick) -> Widget {
    let confirm = if p.mode == Mode::Save { s().save_btn } else { s().open_btn };
    // A button that looks live but does nothing is worse than one that
    // says it can't act yet: save needs a name, open needs a file picked.
    let ready = match p.mode {
        Mode::Save => can_commit_name(&p.name),
        Mode::Open => p.selected_entry().is_some(),
    };
    let confirm_btn = if ready {
        prefab::button(confirm, prefab::ButtonStyle::Primary, ActionId(ACT_CONFIRM))
    } else {
        prefab::button(confirm, prefab::ButtonStyle::Ghost, prefab::NO_ACTION)
    };
    Widget::Row {
        children: alloc::vec![
            Widget::Spacer { flex: 1 },
            prefab::button(s().cancel, prefab::ButtonStyle::Secondary, ActionId(ACT_CANCEL)),
            confirm_btn,
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

/// A name is committable when it's non-empty and names a file, not a path.
fn can_commit_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.contains('/') && n != "." && n != ".."
}

fn render_overwrite(p: &Pick) -> Widget {
    let name = p.name.trim().to_string();
    prefab::dialog(
        s().overwrite_t,
        Widget::Column {
            children: alloc::vec![
                prefab::body(&fill(s().overwrite_b, &name)),
                Widget::Row {
                    children: alloc::vec![
                        Widget::Spacer { flex: 1 },
                        prefab::button(s().cancel, prefab::ButtonStyle::Secondary,
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

fn count_label(p: &Pick) -> String {
    let mut s2 = String::with_capacity(24);
    push_usize(&mut s2, p.entries.len());
    s2.push_str(if p.entries.len() == 1 { " item" } else { " items" });
    s2
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
            if p.confirm_overwrite { p.confirm_overwrite = false; return Outcome::Rerender; }
            answer("");
            Outcome::Done
        }
        Event::Key(KeyCode::Up)   => { p.move_selection(-1); Outcome::Rerender }
        Event::Key(KeyCode::Down) => { p.move_selection(1);  Outcome::Rerender }
        Event::Key(KeyCode::Enter) => {
            if p.confirm_overwrite { return finish(p.confirm()); }
            // A folder is always "descend"; anything else confirms.
            match p.selected_entry() {
                Some(e) if e.is_dir => { p.activate(); Outcome::Rerender }
                _ => finish(p.confirm()),
            }
        }
        Event::Key(KeyCode::Backspace) if p.mode == Mode::Open => {
            p.go_parent();
            Outcome::Rerender
        }
        Event::InputChange { .. } => {
            // Save mode's only editable widget is the name field.
            p.name.clear();
            p.name.push_str(clamp_str(payload, NAME_CAP));
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
        ACT_OVERWRITE => {
            // Already asked — commit straight through.
            let path = p.join(&p.name.trim().to_string());
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
