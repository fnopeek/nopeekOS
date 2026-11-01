//! pick — the file dialog, as a portal.
//!
//! The kernel starts this module on `npk_pick` and hands it the request
//! as a launch argument (`"<open|save|folder>\0<start-dir>\0<suggested-name>"`).
//! We browse npkFS, let the user choose, and report the path back with
//! `npk_pick_result`. The kernel turns that into an `Event::Picked` for
//! whoever asked.
//!
//! Two properties are the whole point of doing it this way:
//!
//!   - We hold READ, the requester doesn't. An app can offer Open
//!     and Save without the right to walk the filesystem itself.
//!   - We never write. Save mode returns a path; the requester
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

// Read to list directories, render to draw, the shell role to stay modal
// over the asking app. Deliberately no WRITE (we return paths, we don't
// create files) and no EXEC.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::RENDER, caps::ext::SHELL | caps::ext::EXTERNAL];

use nopeek_widgets::host;

// The two calls only the file dialog makes; the shared ones are in
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_pick_result(path_ptr: i32, path_len: i32) -> i32;
    fn npk_pick_mkdir(path_ptr: i32, path_len: i32) -> i32;
}

fn pick_result(path: &str) {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_pick_result(path.as_ptr() as i32, path.len() as i32) };
}

fn pick_mkdir(path: &str) -> bool {
    // SAFETY: FFI; the kernel validates the range.
    unsafe { npk_pick_mkdir(path.as_ptr() as i32, path.len() as i32) == 0 }
}

fn log(msg: &str) { host::log_serial(msg); }

/// Answer the requester and go away. An empty path means cancelled.
fn answer(path: &str) {
    pick_result(path);
    host::close_widget();
}

// ── Strings ───────────────────────────────────────────────────────────

struct Strings {
    open_title:   &'static str,
    save_title:   &'static str,
    open_btn:     &'static str,
    folder_open_title: &'static str,
    select_btn:   &'static str,
    save_btn:     &'static str,
    cancel:       &'static str,
    name_hint:    &'static str,
    empty:        &'static str,
    overwrite_t:  &'static str,
    overwrite_b:  &'static str,
    replace:      &'static str,
    new_folder:   &'static str,
    folder_title: &'static str,
    folder_hint:  &'static str,
    create:       &'static str,
}

const EN: Strings = Strings {
    open_title:  "Open file",
    save_title:  "Save file",
    open_btn:    "Open",
    folder_open_title: "Open folder",
    select_btn:  "Select folder",
    save_btn:    "Save",
    cancel:      "Cancel",
    name_hint:   "File name",
    empty:       "This folder is empty",
    overwrite_t: "Replace file?",
    overwrite_b: "{} already exists in this folder.",
    replace:     "Replace",
    new_folder:  "New folder",
    folder_title: "New folder",
    folder_hint: "Folder name",
    create:      "Create",
};

const DE: Strings = Strings {
    open_title:  "Datei öffnen",
    save_title:  "Datei speichern",
    open_btn:    "Öffnen",
    folder_open_title: "Ordner öffnen",
    select_btn:  "Ordner wählen",
    save_btn:    "Speichern",
    cancel:      "Abbrechen",
    name_hint:   "Dateiname",
    empty:       "Dieser Ordner ist leer",
    overwrite_t: "Datei ersetzen?",
    overwrite_b: "{} gibt es in diesem Ordner schon.",
    replace:     "Ersetzen",
    new_folder:  "Neuer Ordner",
    folder_title: "Neuer Ordner",
    folder_hint: "Ordnername",
    create:      "Anlegen",
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
const NAME_CAP: usize = 128;

enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event(buf: &mut [u8]) -> PollResult {
    match nopeek_widgets::events::poll(buf) {
        nopeek_widgets::events::Poll::Event(ev) => PollResult::Event(ev),
        nopeek_widgets::events::Poll::Empty => PollResult::Empty,
        nopeek_widgets::events::Poll::Gone => PollResult::WindowGone,
    }
}

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[pick] panic!"); core::arch::wasm32::unreachable() }


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
enum Mode { Open, Save, Folder }

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
    /// Raised when a sheet opens or closes, so the field now on screen
    /// takes the keyboard.
    focus_seq: u32,
}

impl Pick {
    fn new() -> Self {
        // Request wire: "<open|save|folder>\0<start-dir>\0<suggested-name>".
        let arg = read_launch_arg();
        let mut parts = arg.split('\0');
        let mode = match parts.next() {
            Some("save") => Mode::Save,
            Some("folder") => Mode::Folder,
            _ => Mode::Open,
        };
        let start = parts.next().unwrap_or("").trim();
        let suggest = parts.next().unwrap_or("").trim();

        // The start dir is the requester's hint: one that is gone, or
        // that folder mode could not grant, opens at home instead, so
        // Enter never lands on a folder the user did not navigate to.
        let usable = !start.is_empty() && dir_exists(start)
            && (mode != Mode::Folder || folder_ok(start));
        let dir = if usable { start.to_string() } else { read_home_dir() };

        let mut p = Pick {
            mode,
            dir,
            entries:  Vec::new(),
            selected: None,
            name:     String::with_capacity(NAME_CAP),
            history:  Vec::with_capacity(16),
            confirm_overwrite: false,
            new_folder: false,
            folder_name: String::with_capacity(NAME_CAP),
            focus_seq: 0,
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
            Mode::Save | Mode::Folder => false,
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
            Mode::Folder => {
                let dir = self.folder_target();
                if !folder_ok(&dir) { return false; }
                answer(&dir);
                true
            }
            Mode::Save => {
                if !can_save_in(&self.dir) || !can_commit_name(&self.name) { return false; }
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

    /// Folder mode: the highlighted folder, else the one being shown.
    fn folder_target(&self) -> String {
        match self.selected_entry() {
            Some(e) if e.is_dir => self.join(&e.name),
            _ => self.dir.clone(),
        }
    }

    /// Create the folder the user just named and step into it — that is
    /// almost always why they made it. Returns false so the dialog stays
    /// open (a new folder is a step towards picking, not the answer).
    fn create_folder(&mut self) -> bool {
        let name = self.folder_name.trim().to_string();
        if !can_commit_name(&name) { return false; }
        let path = self.join(&name);
        let ok = pick_mkdir(&path);
        self.new_folder = false;
        self.focus_seq += 1;
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
    host::launch_arg().unwrap_or_default()
}

fn read_home_dir() -> String {
    match host::home_dir() {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => String::from("home"),
    }
}

/// Immediate children of `dir`, folders first then files, each group
/// sorted by name. Wire per line: `name\0size(8)\0is_dir(1)\0mtime(8)`.
/// Where external drives appear. npkFS reserves the name, so the root
/// listing never carries it: the dialog adds it while a drive is mounted.
const EXT: &str = "ext";

fn list_dir(dir: &str) -> Vec<Entry> {
    let Some(listing) = host::fs_list(dir, false) else { return Vec::new() };

    let mut out: Vec<Entry> = Vec::new();
    if dir.is_empty() {
        let drives = count_children(EXT);
        if drives > 0 {
            out.push(Entry { name: String::from(EXT), is_dir: true, size: 0, items: Some(drives) });
        }
    }
    for e in nopeek_widgets::fs::list_entries(&listing) {
        if e.name == ".dir" { continue; }
        let full = if dir.is_empty() { e.name.to_string() }
                   else { alloc::format!("{}/{}", dir, e.name) };
        // Count a folder's children so the right-hand column carries real
        // information. One extra listing per folder — fine for a dialog
        // showing one directory, and it is the number the user wants.
        let items = if e.is_dir { Some(count_children(&full)) } else { None };
        out.push(Entry {
            name:   e.name.to_string(),
            is_dir: e.is_dir,
            size:   e.size,
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

/// Number of entries directly inside `dir`.
fn count_children(dir: &str) -> usize {
    let listing = host::fs_list(dir, false).unwrap_or_default();
    nopeek_widgets::fs::list_entries(&listing)
        .filter(|e| e.name != ".dir")
        .count()
}

/// Truncate to at most `max` bytes without splitting a character. Slicing
/// a `&str` mid-codepoint panics, and a panicking widget app freezes the
/// machine — so every cap on a user-supplied name goes through here.
fn clamp_str(s: &str, max: usize) -> &str {
    if s.len() <= max { return s; }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) { end -= 1; }
    &s[..end]
}

fn dir_exists(dir: &str) -> bool {
    matches!(host::fs_stat(dir), Some((_, true, _)))
}

fn path_exists(path: &str) -> bool {
    host::fs_stat(path).is_some()
}

// ── Render ────────────────────────────────────────────────────────────
//
// Follows loft's list language (docs/spec/UI_REFRESH.md §3/§5): 34 px rows, icon +
// name on the left, one mono column on the right carrying real
// information, and a selection that reads as an accent tint plus a 2 px
// leading edge — never a floating outline.

/// Row metrics, matched to loft's list view so the two read as one system.
const ROW_H:   u16 = 34;
const ROW_PAD: u16 = 4;
/// Right-hand column: "4 items" / "12 KB" / "—".
const COL_META_W: u16 = 96;
const FIELD_H: u16 = 30;
const HEADER_H: u16 = 36;

const NODE_NAME:   u32 = 1;
const NODE_FOLDER: u32 = 2;

fn render(p: &Pick) -> Widget {
    let title = match p.mode {
        Mode::Save => s().save_title,
        Mode::Open => s().open_title,
        Mode::Folder => s().folder_open_title,
    };

    // Flush column, as in loft: the bands reach the window edges and the
    // dividers sit directly between them.
    let mut children: Vec<Widget> = Vec::with_capacity(7);
    children.push(render_header(p, title));
    children.push(Widget::Divider);

    // A sheet replaces toolbar, list and footer, so its field is the only
    // editable widget and `InputChange` stays unambiguous.
    let sheet = if p.confirm_overwrite {
        Some(render_overwrite(p))
    } else if p.new_folder {
        Some(render_new_folder(p))
    } else {
        None
    };
    match sheet {
        Some(card) => children.push(Widget::Column {
            children:  alloc::vec![Widget::Spacer { flex: 1 }, card, Widget::Spacer { flex: 1 }],
            spacing:   0,
            align:     Align::Center,
            modifiers: alloc::vec![Modifier::Flex(1), Modifier::Padding(Padding::Lg.as_u16())],
        }),
        None => {
            children.push(render_toolbar(p));
            children.push(Widget::Divider);
            children.push(render_list(p));
            children.push(Widget::Divider);
            children.push(render_footer(p));
        }
    }

    Widget::Column {
        children,
        spacing:   0,
        align:     Align::Stretch,
        modifiers: alloc::vec![],
    }
}

/// Top band: the 36 px menu-bar band of the other apps (UI_REFRESH §5),
/// with the title in place of the menus. An overlay gets no window
/// controls from the compositor, so the close button is ours.
fn render_header(p: &Pick, title: &str) -> Widget {
    let icon = if p.mode == Mode::Save { IconId::Folder } else { IconId::FolderOpen };
    Widget::Row {
        children: alloc::vec![
            Widget::Icon {
                id:        icon,
                size:      16,
                modifiers: alloc::vec![
                    Modifier::Tint(Token::Accent),
                    Modifier::Padding(Padding::Xs.as_u16()),
                ],
            },
            Widget::Text {
                content:   title.to_string(),
                style:     TextStyle::Body,
                modifiers: alloc::vec![],
            },
            Widget::Spacer { flex: 1 },
            prefab::icon_button(IconId::X, 16, Some(ActionId(ACT_CANCEL)), None),
        ],
        spacing:   Spacing::Xs.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::MinHeight(HEADER_H),
            Modifier::Background(Token::SurfaceElevated),
        ],
    }
}

/// Back / up / breadcrumb; in save mode a new-folder button on the right.
fn render_toolbar(p: &Pick) -> Widget {
    let mut children = alloc::vec![
        nav_icon(IconId::ArrowLeft, ACT_BACK, !p.history.is_empty()),
        nav_icon(IconId::ArrowUp,   ACT_PARENT, !p.dir.is_empty()),
        prefab::breadcrumb(&crumbs(&p.dir)),
        Widget::Spacer { flex: 1 },
    ];
    // Opening has no use for a new directory, and offering it there
    // invites a stray write on a dialog that is meant to be read-only.
    if p.mode != Mode::Open && can_save_in(&p.dir) {
        let mut add = prefab::button(s().new_folder, prefab::ButtonStyle::Ghost,
                                     ActionId(ACT_NEW_FOLDER));
        if let Widget::Button { icon, .. } = &mut add { *icon = IconId::Plus; }
        children.push(add);
    }
    Widget::Row {
        children,
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
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
/// In save mode files are shown but not selectable — you can see what
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
            Modifier::Tint(Token::OnSurfaceMuted),
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

/// The framed text field of loft's toolbar (UI_REFRESH §3 `text_field`).
/// `focus` raises a focus request with that number; 0 asks for nothing.
fn text_field(value: &str, hint: &str, on_submit: u32, node: u32, autofocus: bool,
              focus: u32) -> Widget {
    let mut mods = alloc::vec![Modifier::NodeId(NodeId(node)), Modifier::Flex(1)];
    if autofocus { mods.push(Modifier::Autofocus); }
    if focus != 0 { mods.push(Modifier::FocusRequest(focus)); }
    Widget::Row {
        children: alloc::vec![Widget::Input {
            value:       value.to_string(),
            placeholder: hint.to_string(),
            on_submit:   ActionId(on_submit),
            modifiers:   mods,
        }],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Flex(1),
            Modifier::PaddingXY { x: 8, y: 0 },
            Modifier::MinHeight(FIELD_H),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
            Modifier::Focus(alloc::vec![
                Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
                Modifier::Ring { token: Token::AccentRing, width: 3 },
            ]),
        ],
    }
}

/// Cancel and the main action. The main action is always the filled
/// button; until it can act it is disabled, not restyled.
fn buttons(cancel_act: u32, label: &str, style: prefab::ButtonStyle, act: u32,
           ready: bool) -> [Widget; 2] {
    let mut main = prefab::button(label, style, ActionId(act));
    if !ready {
        if let Widget::Button { modifiers, .. } = &mut main {
            modifiers.push(Modifier::Disabled(alloc::vec![Modifier::Opacity(110)]));
        }
    }
    [
        prefab::button(s().cancel, prefab::ButtonStyle::Ghost, ActionId(cancel_act)),
        main,
    ]
}

/// One band at the bottom: the file name (save) or the picked file
/// (open), then the buttons.
fn render_footer(p: &Pick) -> Widget {
    let (lead, label, ready) = match p.mode {
        Mode::Save => (
            text_field(&p.name, s().name_hint, ACT_NAME_SUBMIT, NODE_NAME, true, p.focus_seq),
            s().save_btn,
            can_save_in(&p.dir) && can_commit_name(&p.name),
        ),
        Mode::Open => {
            let file = p.selected_entry().filter(|e| !e.is_dir);
            let lead = Widget::Text {
                content:   file.map(|e| e.name.clone()).unwrap_or_default(),
                style:     TextStyle::Body,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted), Modifier::Flex(1)],
            };
            (lead, s().open_btn, file.is_some())
        }
        Mode::Folder => {
            let dir = p.folder_target();
            let lead = Widget::Text {
                content:   if dir.is_empty() { String::from("/") } else { dir.clone() },
                style:     TextStyle::Mono,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted), Modifier::Flex(1)],
            };
            (lead, s().select_btn, folder_ok(&dir))
        }
    };
    let [cancel, main] = buttons(ACT_CANCEL, label, prefab::ButtonStyle::Primary, ACT_CONFIRM, ready);
    Widget::Row {
        children:  alloc::vec![lead, cancel, main],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::Background(Token::SurfaceElevated),
        ],
    }
}

/// A folder an app may be handed for editing: not the root, and nothing in
/// `sys`, `.system` or `priv`. The kernel refuses those grants anyway; the button
/// says so before the click.
fn folder_ok(dir: &str) -> bool {
    let top = dir.split('/').next().unwrap_or("");
    !dir.is_empty() && dir != EXT && !matches!(top, "sys" | ".system" | "priv")
}

/// `ext` itself only lists the drives; a file or folder goes on one of
/// them (docs/spec/EXTERNAL_STORAGE.md).
fn can_save_in(dir: &str) -> bool {
    dir != EXT
}

/// A name is committable when it's non-empty and names a file, not a path.
fn can_commit_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.contains('/') && n != "." && n != ".."
}

/// Card body as in loft's dialogs: text, then the buttons right-aligned.
fn sheet_body(top: Widget, buttons: [Widget; 2]) -> Widget {
    let [cancel, main] = buttons;
    Widget::Column {
        children: alloc::vec![
            top,
            Widget::Row {
                children:  alloc::vec![Widget::Spacer { flex: 1 }, cancel, main],
                spacing:   Spacing::Sm.as_u16(),
                align:     Align::Center,
                modifiers: alloc::vec![],
            },
        ],
        spacing:   Spacing::Md.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![],
    }
}

fn render_new_folder(p: &Pick) -> Widget {
    let field = text_field(&p.folder_name, s().folder_hint, ACT_FOLDER_CREATE, NODE_FOLDER,
                           false, p.focus_seq);
    let ready = can_commit_name(&p.folder_name);
    prefab::dialog(
        s().folder_title,
        sheet_body(field, buttons(ACT_FOLDER_CANCEL, s().create, prefab::ButtonStyle::Primary,
                                  ACT_FOLDER_CREATE, ready)),
        None,
        360,
    )
}

fn render_overwrite(p: &Pick) -> Widget {
    prefab::dialog(
        s().overwrite_t,
        sheet_body(prefab::body(&fill(s().overwrite_b, &p.full_name())),
                   buttons(ACT_OVERWRITE_CANCEL, s().replace, prefab::ButtonStyle::Destructive,
                           ACT_OVERWRITE, true)),
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
            if p.confirm_overwrite { p.confirm_overwrite = false; p.focus_seq += 1; return Outcome::Rerender; }
            if p.new_folder { p.new_folder = false; p.focus_seq += 1; return Outcome::Rerender; }
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
        Event::Key(KeyCode::Backspace) if p.mode != Mode::Save && !p.new_folder => {
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
        ACT_NEW_FOLDER if can_save_in(&p.dir) => {
            p.folder_name.clear();
            p.new_folder = true;
            p.focus_seq += 1;
            Outcome::Rerender
        }
        ACT_FOLDER_CREATE => finish(p.create_folder()),
        ACT_FOLDER_CANCEL => { p.new_folder = false; p.focus_seq += 1; Outcome::Rerender }
        ACT_OVERWRITE => {
            // Already asked — commit straight through.
            let path = p.join(&p.full_name());
            answer(&path);
            Outcome::Done
        }
        ACT_OVERWRITE_CANCEL => { p.confirm_overwrite = false; p.focus_seq += 1; Outcome::Rerender }
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
        Ok(bytes) => { if !host::scene_commit(&bytes) { log("[pick] commit failed"); } }
        Err(_) => log("[pick] encode failed"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // The kernel already made this window a centred overlay; modal keeps
    // stray keystrokes out of the app behind us while a dialog is up.
    host::window_set_modal(true);

    let mut p = Pick::new();
    let mut event_buf = alloc::vec![0u8; EVENT_BUF_SIZE];

    commit_tree(&p);

    loop {
        match poll_event(&mut event_buf) {
            PollResult::Event(ev) => {
                let payload = match &ev {
                    Event::InputChange { value } => value.clone(),
                    _ => String::new(),
                };
                let outcome = handle(&mut p, ev, &payload);
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Rerender => commit_tree(&p),
                    // `answer` already reported and closed the window.
                    Outcome::Done => return,
                }
            }
            PollResult::Empty => host::sleep_ms(16),
            PollResult::WindowGone => return,
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_icon_ref(_: IconRef) {}
