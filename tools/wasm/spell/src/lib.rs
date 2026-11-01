//! spell — text editor with markdown preview, in loft's visual language.
//!
//! Layout (top → bottom):
//!   menu_bar   — File / View / Help
//!   tab bar    — one tab per open document
//!   body       — TextArea (edit) or rendered markdown preview (read-only)
//!
//! Editing uses `Widget::TextArea`: the compositor owns the 2-D caret
//! (arrows / Enter / Home-End / PageUp-Down), the app only mirrors the
//! document via `Event::InputChange`. Syntax highlighting is sent as
//! colour spans the compositor paints live; markdown has a separate
//! rendered preview.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::history::{History, Restore};
use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Declared capabilities: read + render. No WRITE — an editor that can
// overwrite any file in the store is exactly what the file-dialog portal
// exists to avoid.
//
// Saving still works, through two narrower routes the kernel grants:
//   - the path the user picked in the dialog (`npk_pick` records it
//     against this instance — the click is the authorisation), and
//   - `sys/config/spell`, our own settings file, whose name the kernel
//     derives from the module name so we can't claim someone else's.
//
// Browsing is likewise not ours: the dialog runs in its own module and
// hands back a single path.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::RENDER];

use nopeek_widgets::host;

// The calls without a shared wrapper; the rest are in
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_window_set_close_guard(on: i32) -> i32;
    fn npk_window_close_cancel() -> i32;
}

/// Tell the window manager the close it asked for is off: the next close
/// asks again, and a power-off waiting on this window stops.
fn window_close_cancel() {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_close_cancel() };
}

fn window_set_close_guard(on: bool) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_close_guard(on as i32) };
}

// ── Strings ───────────────────────────────────────────────────────────
// English is the source language; a new one is one more `const` below.
// See `nopeek_widgets::i18n`.

struct Strings {
    menu_file:     &'static str,
    menu_view:     &'static str,
    menu_help:     &'static str,
    new:           &'static str,
    open:          &'static str,
    save:          &'static str,
    save_as:       &'static str,
    close:         &'static str,
    view_source:   &'static str,
    view_preview:  &'static str,
    settings:      &'static str,
    zoom_in:       &'static str,
    zoom_out:      &'static str,
    /// "Actual size ({} px)" — shows where you'd land back.
    zoom_reset:    &'static str,
    /// Header written above the settings, so the file explains itself
    /// when someone opens it in spell.
    cfg_header:    &'static str,
    about:         &'static str,
    untitled:      &'static str,
    /// Default filename offered by the save dialog (no extension).
    untitled_file: &'static str,
    placeholder:   &'static str,
    unsaved_title: &'static str,
    unsaved_body:  &'static str,
    discard:       &'static str,
    cancel:        &'static str,
    /// "{} of {}" — position in the quit sequence.
    step_of:       &'static str,
    welcome:       &'static str,
    menu_edit:     &'static str,
    undo:          &'static str,
    redo:          &'static str,
    find:          &'static str,
    replace:       &'static str,
    goto_line:     &'static str,
    find_ph:       &'static str,
    replace_ph:    &'static str,
    replace_one:   &'static str,
    replace_all:   &'static str,
    /// "Go to line (1-{})".
    goto_ph:       &'static str,
    no_results:    &'static str,
    /// "Ln {}, Col {}".
    ln_col:        &'static str,
    /// "{} selected".
    selected:      &'static str,
    /// "Spaces: {}".
    spaces:        &'static str,
    tabs:          &'static str,
    toggle_comment: &'static str,
    delete_line:   &'static str,
    disk_changed:  &'static str,
    reload:        &'static str,
    keep_mine:     &'static str,
}

const EN: Strings = Strings {
    menu_file: "File", menu_view: "View", menu_help: "Help",
    new: "New", open: "Open…", save: "Save", save_as: "Save as…",
    close: "Close",
    view_source: "Source", view_preview: "Preview",
    settings: "Settings",
    zoom_in: "Zoom in", zoom_out: "Zoom out",
    zoom_reset: "Actual size ({} px)",
    cfg_header: "# spell settings — one key per line, # is a comment.\n# font: editor size in px (6-64), Ctrl+0 restores 13.\n# trim_trailing: yes removes spaces at line ends on save.\n# final_newline: yes ends the file with a line break on save.\n# recent / open: recently used files and the tabs open at exit.\n",
    about: "About Spell",
    untitled: "Untitled",
    untitled_file: "untitled",
    placeholder: "Start typing…",
    unsaved_title: "Unsaved changes",
    unsaved_body: "{} has unsaved changes.",
    discard: "Discard", cancel: "Cancel",
    step_of: "{} of {}",
    welcome: "Welcome to Spell\n\nStart typing, or open a file via File \u{2192} Open\u{2026} \u{2014} or double-click in loft.\n\nMultiple files open as tabs. Save it with a name and the\nextension decides how it is highlighted.\n",
    menu_edit: "Edit",
    undo: "Undo", redo: "Redo",
    find: "Find", replace: "Replace", goto_line: "Go to line…",
    find_ph: "Find", replace_ph: "Replace with",
    replace_one: "Replace", replace_all: "All",
    goto_ph: "Go to line (1-{})",
    no_results: "No results",
    ln_col: "Ln {}, Col {}",
    selected: "{} selected",
    spaces: "Spaces: {}",
    tabs: "Tabs",
    toggle_comment: "Toggle comment",
    delete_line: "Delete line",
    disk_changed: "This file changed on disk.",
    reload: "Reload",
    keep_mine: "Keep mine",
};


fn s() -> &'static Strings { &EN }

/// Substitute the single `{}` placeholder in a catalog string.
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

/// "(2 of 3)" — `step_of` carries two placeholders, one more than
/// `fill` handles, so substitute both here.
fn step_label(n: usize, total: usize) -> String {
    let mut a = String::with_capacity(8);
    push_usize(&mut a, n);
    let mut b = String::with_capacity(8);
    push_usize(&mut b, total);
    let once = fill(s().step_of, &a);
    fill(&once, &b)
}

/// "Actual size (13 px)" — naming the target makes the entry a status
/// line too: you can see how far you have zoomed without counting.
fn zoom_reset_label() -> String {
    let mut target = String::with_capacity(4);
    push_usize(&mut target, MONO_SIZE_PX as usize);
    fill(s().zoom_reset, &target)
}

fn log(msg: &str) { host::log_serial(msg); }

// ── Buffers ───────────────────────────────────────────────────────────
//
// An `InputChange` carries the whole document, so the usual buffer is sized
// for a large file; a larger event gets its own (`events::poll`).
const EVENT_BUF_SIZE: usize = 512 * 1024;

// Pre-allocate the document so ordinary edits stay within capacity.
const TEXT_CAP: usize = 256 * 1024;


enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event(buf: &mut [u8]) -> PollResult {
    match nopeek_widgets::events::poll(buf) {
        nopeek_widgets::events::Poll::Event(ev) => PollResult::Event(ev),
        nopeek_widgets::events::Poll::Empty => PollResult::Empty,
        nopeek_widgets::events::Poll::Gone => PollResult::WindowGone,
    }
}

// The SDK's growing heap: it frees, and it takes memory from the runtime as
// it is needed instead of reserving it at launch.
#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[spell] panic!"); core::arch::wasm32::unreachable() }


// ── Action / node ids ─────────────────────────────────────────────────

const ACT_MENU_FILE: u32 = 5_000;
const ACT_MENU_VIEW: u32 = 5_002;
const ACT_MENU_HELP: u32 = 5_004;
const ACT_MENU_DISMISS: u32 = 5_500;

const ACT_FILE_NEW:     u32 = 6_000;
const ACT_FILE_OPEN:    u32 = 6_001;
const ACT_FILE_SAVE:    u32 = 6_002;
const ACT_FILE_CLOSE:   u32 = 6_003;
const ACT_FILE_SAVE_AS: u32 = 6_004;
// Unsaved-changes-on-close dialog.
const ACT_CLOSE_SAVE:    u32 = 6_007;
const ACT_CLOSE_DISCARD: u32 = 6_008;
const ACT_CLOSE_CANCEL:  u32 = 6_009;
const ACT_VIEW_EDIT:    u32 = 6_100;
const ACT_VIEW_PREVIEW: u32 = 6_101;
const ACT_FILE_SETTINGS: u32 = 6_010;
const ACT_ZOOM_IN:    u32 = 6_102;
const ACT_ZOOM_OUT:   u32 = 6_103;
const ACT_ZOOM_RESET: u32 = 6_104;
const ACT_HELP_ABOUT: u32 = 6_300;
const ACT_MENU_EDIT:  u32 = 5_006;
const ACT_UNDO:       u32 = 6_200;
const ACT_REDO:       u32 = 6_201;
const ACT_FIND:       u32 = 6_202;
const ACT_REPLACE:    u32 = 6_203;
const ACT_GOTO:       u32 = 6_204;
// Find / go-to bar.
const ACT_FIND_NEXT:    u32 = 6_400;
const ACT_FIND_PREV:    u32 = 6_401;
const ACT_FIND_CASE:    u32 = 6_402;
const ACT_FIND_CLOSE:   u32 = 6_403;
const ACT_REPLACE_ONE:  u32 = 6_404;
const ACT_REPLACE_ALL:  u32 = 6_405;
const ACT_GOTO_SUBMIT:  u32 = 6_406;
const ACT_COMMENT:      u32 = 6_205;
const ACT_DELETE_LINE:  u32 = 6_206;
const ACT_DISK_RELOAD:  u32 = 6_407;
const ACT_DISK_KEEP:    u32 = 6_408;
/// File menu: reopen recent file i.
const ACT_RECENT_BASE:  u32 = 7_000;
const MAX_RECENT:       usize = 8;

// `npk_pick` modes.
const PICK_OPEN: host::PickMode = host::PickMode::Open;
const PICK_SAVE: host::PickMode = host::PickMode::Save;

// Tags handed to `npk_pick` and returned in `Event::Picked`. The save tags
// carry the tab index, so a reply lands on the document it was asked for
// rather than on whatever happens to be active when it arrives.
const TAG_OPEN:            u32 = 1;
const TAG_SAVE_BASE:       u32 = 1_000;
const TAG_SAVE_CLOSE_BASE: u32 = 2_000;

// Tab bar: select tab i / close tab i.
const ACT_TAB_BASE:       u32 = 8_000;
const ACT_TAB_CLOSE_BASE: u32 = 8_500;

const NODE_MENU_FILE: u32 = 100;
const NODE_MENU_VIEW: u32 = 102;
const NODE_MENU_HELP: u32 = 104;
const NODE_MENU_EDIT: u32 = 106;
const NODE_FIND:      u32 = 200;
const NODE_REPLACE:   u32 = 201;
const NODE_GOTO:      u32 = 202;
/// Each document's editor carries `NODE_EDITOR_BASE + uid`, so a caret or
/// change event still finds its document after the tab was switched.
const NODE_EDITOR_BASE: u32 = 10_000;

/// More hits than this are not counted or coloured.
const MAX_HITS: usize = 10_000;

// ── State ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Edit, Preview }

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenMenu { File, Edit, View, Help }

/// What the file's extension says it is. A buffer with no filename yet is
/// `Untyped`: no highlighting, no preview toggle, no assumed extension —
/// it becomes a kind when the user names it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Markdown, Code(Lang), Plain, Untyped }

/// Languages with a preview highlighter. Markup = HTML/XML (tag-based);
/// the rest share the C-like scanner parameterised by `syntax_for`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang { Rust, C, Js, Python, Shell, Json, Markup, Toml, Yaml, Css, Diff, Make }

/// One open document = one tab.
struct Doc {
    /// npkFS path of the open file, or None for an unsaved buffer.
    path:  Option<String>,
    /// Basename shown on the tab.
    title: String,
    /// The whole document (`\n`-separated). Pre-allocated to `TEXT_CAP`.
    text:  String,
    dirty: bool,
    /// Markdown raw↔rendered toggle (per document).
    mode:  Mode,
    /// Stable id for the editor's NodeId; tab indices shift on close.
    uid:   u32,
    history: History,
    /// Hash of the text as last loaded or saved: undoing back to it makes
    /// the tab clean again.
    saved: u64,
    /// Highlight spans for `text`, rebuilt when it changes.
    spans: Vec<Span>,
    spans_stale: bool,
    /// Indent width and whether it is tabs, detected from the text.
    indent: (u8, bool),
    /// The file on disk is no longer what was loaded or saved, and the
    /// tab has edits of its own: the user decides.
    disk_changed: bool,
}

impl Doc {
    fn empty() -> Self {
        Doc {
            path:  None,
            title: s().untitled.to_string(),
            text:  String::with_capacity(TEXT_CAP),
            dirty: false,
            mode:  Mode::Edit,
            uid:   0,
            history: History::default(),
            saved: text_hash(""),
            spans: Vec::new(),
            spans_stale: true,
            indent: (4, false),
            disk_changed: false,
        }
    }

    fn node(&self) -> u32 { NODE_EDITOR_BASE + self.uid }

    /// Adopt a freshly loaded text: nothing to undo, clean.
    fn load(&mut self, text: &str) {
        self.set_text(text);
        self.history.clear();
        self.saved = text_hash(&self.text);
        self.dirty = false;
        self.indent = detect_indent(&self.text);
    }

    /// The text changed by an edit, an undo or a replace.
    fn touch(&mut self) {
        self.dirty = text_hash(&self.text) != self.saved;
        self.spans_stale = true;
    }

    fn refresh_spans(&mut self) {
        if !self.spans_stale { return; }
        self.spans = match self.kind() {
            Kind::Code(lang) => code_spans(&self.text, lang),
            Kind::Markdown => markdown_spans(&self.text),
            _ => Vec::new(),
        };
        self.spans_stale = false;
    }

    fn ext(&self) -> Option<String> {
        self.path.as_deref()
            .and_then(|p| p.rsplit_once('.'))
            .map(|(_, e)| e.to_ascii_lowercase())
    }

    fn kind(&self) -> Kind {
        let base = self.path.as_deref().map(basename).unwrap_or("");
        if matches!(base, "Makefile" | "makefile" | "GNUmakefile") {
            return Kind::Code(Lang::Make);
        }
        match self.ext().as_deref() {
            Some("md") | Some("markdown")          => Kind::Markdown,
            Some("rs")                             => Kind::Code(Lang::Rust),
            Some("c") | Some("h")                  => Kind::Code(Lang::C),
            Some("js") | Some("ts")                => Kind::Code(Lang::Js),
            Some("py")                             => Kind::Code(Lang::Python),
            Some("sh") | Some("bash")              => Kind::Code(Lang::Shell),
            Some("json")                           => Kind::Code(Lang::Json),
            Some("html") | Some("htm") | Some("xml") | Some("svg") => Kind::Code(Lang::Markup),
            Some("toml")                           => Kind::Code(Lang::Toml),
            Some("yaml") | Some("yml")             => Kind::Code(Lang::Yaml),
            Some("css")                            => Kind::Code(Lang::Css),
            Some("diff") | Some("patch")           => Kind::Code(Lang::Diff),
            Some("mk")                             => Kind::Code(Lang::Make),
            None                                   => Kind::Untyped, // unnamed buffer
            _                                      => Kind::Plain,   // txt/log/toml/yaml/…
        }
    }


    fn set_text(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
        self.spans_stale = true;
    }

    /// `true` for a fresh, never-edited untitled tab — opening a file
    /// reuses it instead of stacking a blank tab (VS Code behaviour).
    fn is_pristine(&self) -> bool {
        self.path.is_none() && !self.dirty
    }
}

struct Spell {
    /// Open documents, one per tab.
    docs:         Vec<Doc>,
    /// Index of the active tab.
    active:       usize,
    open_menu:    Option<OpenMenu>,
    /// Tab index pending an unsaved-changes confirmation before close.
    /// `Some(i)` shows the "save changes?" dialog for tab `i`.
    confirm_close: Option<usize>,
    /// A quit is in progress: the confirm dialog is walking the dirty
    /// tabs one by one, and clearing the last one closes the app.
    quitting:      bool,
    /// Editor font size in px. Ctrl+wheel moves it; every tab shares one
    /// setting, like a view preference rather than a per-file property.
    font_px:       u16,
    /// Save options from the settings file.
    trim:          bool,
    final_nl:      bool,
    /// Recently opened or saved files, newest first.
    recent:        Vec<String>,
    /// What the settings file holds. A wheel spin fires a dozen zoom
    /// events and each npkFS write costs an encrypt + hash + flush, so the
    /// file is written when leaving, not per change.
    cfg_saved:     Config,
    /// How many dirty tabs the quit started with, and how many are done.
    /// Counted at the start because the live count shrinks as tabs close
    /// — deriving the step from it would show "1 of 3", "1 of 2", "1 of 1".
    quit_total:    usize,
    quit_done:     usize,
    next_uid:      u32,
    /// Counter shared by `Modifier::Select` and `Modifier::FocusRequest`:
    /// each new request takes the next number.
    seq:           u32,
    /// Caret placement for an editor: `(node, seq, anchor, cursor)`.
    select:        Option<(u32, u32, u32, u32)>,
    /// Which field should take the keyboard, and the request's number.
    focus:         Option<(Focus, u32)>,
    bar:           Option<Bar>,
    /// Editor node of the last commit: a different one means the tab
    /// changed, and that document's caret is put back.
    shown_node:    u32,
    /// Shift from the `Event::Mods` sent right before a key.
    shift:         bool,
    /// Ctrl from the `Event::Mods` sent right before a key.
    ctrl:          bool,
}

/// The fields that can ask for the keyboard.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus { Editor, Query, Replace, Goto }

/// The strip above the editor.
enum Bar {
    Find(FindBar),
    Goto(String),
}

struct FindBar {
    query:   String,
    /// Some while the replace row is shown.
    replace: Option<String>,
    case:    bool,
    /// Index of the current hit.
    cur:     usize,
}

impl Spell {
    fn new() -> Self {
        let cfg = read_config();
        let mut sp = Spell {
            docs:      Vec::new(),
            active:    0,
            open_menu: None,
            confirm_close: None,
            quitting:      false,
            font_px:       cfg.font,
            trim:          cfg.trim,
            final_nl:      cfg.final_nl,
            recent:        cfg.recent.clone(),
            quit_total:    0,
            quit_done:     0,
            next_uid:      0,
            seq:           0,
            select:        None,
            focus:         None,
            bar:           None,
            shown_node:    u32::MAX,
            shift:         false,
            ctrl:          false,
            cfg_saved:     cfg,
        };

        // Launched to open a specific file (loft file association)?
        let path = host::launch_arg().unwrap_or_default();
        if !path.is_empty() {
            if let Some(text) = read_file(&path) {
                let mut d = sp.fresh_doc();
                d.title = basename(&path).to_string();
                d.path = Some(path);
                d.load(&text);
                sp.docs.push(d);
                return sp;
            }
        }

        // The tabs open when spell last closed.
        let open = sp.cfg_saved.open.clone();
        for p in &open {
            if let Some(text) = read_file(p) {
                let mut d = sp.fresh_doc();
                d.title = basename(p).to_string();
                d.path = Some(p.clone());
                d.load(&text);
                sp.docs.push(d);
            }
        }
        if !sp.docs.is_empty() { return sp; }

        // No file argument → welcome tab.
        let mut d = sp.fresh_doc();
        d.text.push_str(s().welcome);
        sp.docs.push(d);
        sp
    }

    /// An empty document with its own uid.
    fn fresh_doc(&mut self) -> Doc {
        let mut d = Doc::empty();
        d.uid = self.next_uid;
        self.next_uid = self.next_uid.wrapping_add(1);
        d
    }

    fn next_seq(&mut self) -> u32 {
        self.seq = self.seq.wrapping_add(1).max(1);
        self.seq
    }

    /// Put the active editor's caret at `(anchor, cursor)`.
    fn place(&mut self, anchor: u32, cursor: u32) {
        let node = self.cur().node();
        let seq = self.next_seq();
        self.select = Some((node, seq, anchor, cursor));
        self.cur_mut().history.caret_moved(anchor, cursor);
    }

    fn focus_on(&mut self, f: Focus) {
        let seq = self.next_seq();
        self.focus = Some((f, seq));
    }

    /// The document whose editor carries `node`.
    fn doc_by_node(&mut self, node: u32) -> Option<&mut Doc> {
        self.docs.iter_mut().find(|d| d.node() == node)
    }

    fn cur(&self) -> &Doc { &self.docs[self.active] }
    fn cur_mut(&mut self) -> &mut Doc { &mut self.docs[self.active] }

    /// Open a file: focus its tab if already open (VS Code behaviour),
    /// else load it into a new tab (reusing a pristine untitled tab).
    fn open_path(&mut self, path: &str) {
        if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(path)) {
            self.active = i;
            return;
        }
        let text = match read_file(path) {
            Some(t) => t,
            None => {
                log("[spell] open: failed");
                self.recent.retain(|p| p != path);
                return;
            }
        };
        self.note_recent(path);
        let mut d = self.fresh_doc();
        d.path = Some(path.to_string());
        d.title = basename(path).to_string();
        d.load(&text);
        if self.cur().is_pristine() {
            let a = self.active;
            self.docs[a] = d;
        } else {
            self.docs.push(d);
            self.active = self.docs.len() - 1;
        }
    }

    fn new_doc(&mut self) {
        let d = self.fresh_doc();
        if self.cur().is_pristine() {
            let a = self.active;
            self.docs[a] = d;
        } else {
            self.docs.push(d);
            self.active = self.docs.len() - 1;
        }
    }

    /// Close tab `i`, but if it has unsaved changes show the confirm
    /// dialog first (focusing it) instead of discarding silently.
    fn request_close(&mut self, i: usize) {
        if i >= self.docs.len() { return; }
        self.active = i;
        if self.docs[i].dirty {
            self.confirm_close = Some(i);
        } else {
            self.close_tab(i);
        }
    }

    // ── Quitting ──────────────────────────────────────────────────────
    //
    // The window manager asks before closing us (`Event::CloseRequest`),
    // so this is where unsaved work gets its say. One dialog per dirty
    // tab, in order, with a counter — the same shape VS Code uses. Any
    // Cancel abandons the whole quit, not just that one file.

    /// Begin quitting. Returns true if we can go right now.
    fn begin_quit(&mut self) -> bool {
        self.open_menu = None;
        self.confirm_close = None;
        match self.first_dirty() {
            None => true,
            Some(i) => {
                self.quitting = true;
                self.quit_total = self.dirty_count();
                self.quit_done = 0;
                self.active = i;
                self.confirm_close = Some(i);
                false
            }
        }
    }

    fn first_dirty(&self) -> Option<usize> {
        self.docs.iter().position(|d| d.dirty)
    }

    fn dirty_count(&self) -> usize {
        self.docs.iter().filter(|d| d.dirty).count()
    }

    /// One dirty tab is settled; move to the next or finish. Returns true
    /// when nothing is left and the app should close.
    fn advance_quit(&mut self) -> bool {
        if !self.quitting { return false; }
        self.quit_done += 1;
        match self.first_dirty() {
            Some(i) => { self.active = i; self.confirm_close = Some(i); false }
            None => true,
        }
    }

    /// Step the editor font. `to = None` returns to the default — the
    /// "actual size" entry every editor and browser has, because after a
    /// few notches nobody remembers where they started.
    fn zoom(&mut self, to: Option<i32>) -> bool {
        let next = match to {
            None => MONO_SIZE_PX as i32,
            Some(d) => self.font_px as i32 + d,
        };
        let clamped = next.clamp(FONT_SIZE_MIN as i32, FONT_SIZE_MAX as i32) as u16;
        if clamped == self.font_px { return false; }
        self.font_px = clamped;
        true
    }

    /// Persist settings if they drifted from the file. Cheap no-op when
    /// nothing changed, so it can sit on every exit path.
    fn save_config(&mut self) {
        let c = self.config();
        if c == self.cfg_saved { return; }
        write_config(&c);
        self.cfg_saved = c;
    }

    fn config(&self) -> Config {
        Config {
            font:     self.font_px,
            trim:     self.trim,
            final_nl: self.final_nl,
            recent:   self.recent.clone(),
            open:     self.docs.iter().filter_map(|d| d.path.clone()).collect(),
        }
    }

    /// Put `path` at the top of the recent list.
    fn note_recent(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(MAX_RECENT);
    }

    fn cancel_quit(&mut self) {
        if self.quitting { window_close_cancel(); }
        self.quitting = false;
        self.quit_total = 0;
        self.quit_done = 0;
        self.confirm_close = None;
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.docs.len() { return; }
        self.docs.remove(i);
        if self.docs.is_empty() {
            let d = self.fresh_doc();
            self.docs.push(d);
            self.active = 0;
        } else if self.active >= self.docs.len() {
            self.active = self.docs.len() - 1;
        } else if self.active > i {
            self.active -= 1;
        }
    }

    /// Save the active doc, or ask where to put it if it has no file yet.
    fn save_or_name(&mut self) {
        if let Some(p) = self.cur().path.clone() {
            let _ = self.write_to(&p);
        } else {
            self.ask_save_target(self.active, false);
        }
    }

    /// Write the active doc to `path`. False if it did not happen: the
    /// file changed on disk meanwhile (the tab asks first), or the store
    /// refused it (this instance may not write there: the save dialog
    /// asks for the place, which grants it).
    fn write_to(&mut self, path: &str) -> bool {
        if let Some(disk) = host::fetch(path) {
            let d = self.cur_mut();
            if d.path.as_deref() == Some(path) && text_hash_bytes(&disk) != d.saved {
                d.disk_changed = true;
                return false;
            }
        }
        self.apply_save_options();
        if !host::store(path, self.cur().text.as_bytes()) {
            log("[spell] save: store failed, asking where");
            self.ask_save_target(self.active, false);
            return false;
        }
        self.note_recent(path);
        let d = self.cur_mut();
        d.saved = text_hash(&d.text);
        d.dirty = false;
        d.disk_changed = false;
        // Editing the settings file in spell itself is a stated use of
        // having one. Adopt what was just saved — otherwise our in-memory
        // value would be written back over it on the way out and the edit
        // would silently undo itself.
        if path == CONFIG_PATH {
            let c = read_config();
            self.font_px = c.font;
            self.trim = c.trim;
            self.final_nl = c.final_nl;
            self.recent = c.recent.clone();
            self.cfg_saved = c;
        }
        true
    }

    /// Trailing spaces and the final line break, as the settings ask; one
    /// undo step, caret kept on its text.
    fn apply_save_options(&mut self) {
        let (trim, final_nl) = (self.trim, self.final_nl);
        if !trim && !final_nl { return; }
        let d = self.cur_mut();
        let (a, c) = d.history.caret();
        let (mut new, na, nc) = if trim { trim_trailing(&d.text, a, c) } else { (d.text.clone(), a, c) };
        if final_nl && !new.is_empty() && !new.ends_with('\n') { new.push('\n'); }
        if new == d.text { return; }
        let old = core::mem::replace(&mut d.text, new);
        d.history.replaced(&old, &d.text, na, nc);
        d.touch();
        self.place(na, nc);
    }

    /// Ask the picker where to save tab `doc`. `then_close` remembers that
    /// this save came from the close dialog, so the tab goes away once the
    /// file is written.
    ///
    /// The dialog is modal, so the tab indices encoded in the tag can't
    /// shift while it's up.
    fn ask_save_target(&mut self, doc: usize, then_close: bool) {
        self.open_menu = None;
        let d = match self.docs.get(doc) { Some(d) => d, None => return };
        // Open where the file already lives; a fresh buffer starts wherever
        // the picker defaults to (the kernel resolves "" to the user's home).
        let start = d.path.as_deref().map(dirname).unwrap_or("");
        // Suggest a name so the dialog's Save is usable straight away.
        // No extension for a fresh buffer — the file has no type until the
        // user gives it one, and whatever they type decides it.
        let suggest = d.path.as_deref().map(basename).unwrap_or(s().untitled_file);
        let base = if then_close { TAG_SAVE_CLOSE_BASE } else { TAG_SAVE_BASE };
        pick(PICK_SAVE, start, suggest, base + doc as u32);
    }

    /// Adopt `path` as tab `doc`'s file and write it there.
    fn save_as(&mut self, doc: usize, path: &str) {
        if doc >= self.docs.len() { return; }
        let title = basename(path).to_string();
        self.docs[doc].title = title;
        self.docs[doc].path = Some(path.to_string());
        // The extension may have changed the language.
        self.docs[doc].spans_stale = true;
        let prev = self.active;
        self.active = doc;
        // The user just picked this place: no question about what is there.
        self.docs[doc].disk_changed = false;
        if let Some(disk) = host::fetch(path) { self.docs[doc].saved = text_hash_bytes(&disk); }
        let _ = self.write_to(path);
        self.active = prev.min(self.docs.len().saturating_sub(1));
    }
}

// ── Editing: undo, find, replace, go to line ─────────────────────────
//
// The compositor edits the buffer; spell sees each change as the whole
// value and each caret move as `Event::Caret`, and places the caret back
// with `Modifier::Select` when it changes the text itself.

impl Spell {
    fn undo(&mut self) {
        let d = self.cur_mut();
        let Some(r) = d.history.undo(&d.text) else { return };
        self.apply_restore(r);
    }

    fn redo(&mut self) {
        let d = self.cur_mut();
        let Some(r) = d.history.redo(&d.text) else { return };
        self.apply_restore(r);
    }

    fn apply_restore(&mut self, r: Restore) {
        let d = self.cur_mut();
        d.set_text(&r.text);
        d.touch();
        self.place(r.anchor, r.cursor);
        self.focus_on(Focus::Editor);
    }

    /// Open the find bar, with the replace row if `replace`. A selection
    /// on one line becomes the query.
    fn open_find(&mut self, replace: bool) {
        let d = self.cur();
        let (a, c) = d.history.caret();
        let (lo, hi) = (a.min(c) as usize, a.max(c) as usize);
        let picked = d.text.get(lo..hi).filter(|t| !t.is_empty() && !t.contains('\n'))
            .map(|t| t.to_string());
        match &mut self.bar {
            Some(Bar::Find(f)) => {
                if let Some(q) = picked { f.query = q; }
                if replace && f.replace.is_none() { f.replace = Some(String::new()); }
            }
            _ => {
                self.bar = Some(Bar::Find(FindBar {
                    query:   picked.unwrap_or_default(),
                    replace: if replace { Some(String::new()) } else { None },
                    case:    false,
                    cur:     0,
                }));
            }
        }
        self.focus_on(if replace { Focus::Replace } else { Focus::Query });
        self.seek_from_caret();
    }

    /// Close the bar; the editor takes the keyboard at the current hit.
    fn close_bar(&mut self) {
        self.bar = None;
        self.focus_on(Focus::Editor);
        let (a, c) = self.cur().history.caret();
        self.place(a, c);
    }

    fn hits(&self) -> Vec<(usize, usize)> {
        match &self.bar {
            Some(Bar::Find(f)) => find_all(&self.cur().text, &f.query, f.case),
            _ => Vec::new(),
        }
    }

    /// Make the first hit at or after the caret current and select it.
    fn seek_from_caret(&mut self) {
        let hits = self.hits();
        let (a, c) = self.cur().history.caret();
        let from = a.min(c) as usize;
        let i = hits.iter().position(|h| h.0 >= from).unwrap_or(0);
        self.select_hit(&hits, i);
    }

    fn select_hit(&mut self, hits: &[(usize, usize)], i: usize) {
        if let Some(Bar::Find(f)) = &mut self.bar { f.cur = i; }
        if let Some(&(s, e)) = hits.get(i) {
            self.place(s as u32, e as u32);
        }
    }

    /// Step to the next (`dir` 1) or previous (`dir` -1) hit.
    fn step_hit(&mut self, dir: isize) {
        let hits = self.hits();
        if hits.is_empty() { return; }
        let cur = match &self.bar { Some(Bar::Find(f)) => f.cur, _ => 0 };
        let n = hits.len() as isize;
        let i = ((cur.min(hits.len() - 1) as isize + dir).rem_euclid(n)) as usize;
        self.select_hit(&hits, i);
    }

    /// Replace the current hit and move on to the next one.
    fn replace_one(&mut self) {
        let hits = self.hits();
        let (cur, with) = match &self.bar {
            Some(Bar::Find(FindBar { cur, replace: Some(r), .. })) => (*cur, r.clone()),
            _ => return,
        };
        let Some(&(s, e)) = hits.get(cur) else { return };
        let d = self.cur_mut();
        let old = d.text.clone();
        d.text.replace_range(s..e, &with);
        let end = (s + with.len()) as u32;
        d.history.replaced(&old, &d.text, end, end);
        d.touch();
        let hits = self.hits();
        let i = hits.iter().position(|h| h.0 >= end as usize).unwrap_or(0);
        if hits.is_empty() { self.place(end, end); } else { self.select_hit(&hits, i); }
    }

    /// Replace every hit, as one undo step.
    fn replace_all(&mut self) {
        let hits = self.hits();
        let with = match &self.bar {
            Some(Bar::Find(FindBar { replace: Some(r), .. })) => r.clone(),
            _ => return,
        };
        if hits.is_empty() { return; }
        let d = self.cur_mut();
        let old = d.text.clone();
        let mut out = String::with_capacity(old.len());
        let (mut last, mut end) = (0, 0);
        for &(s, e) in &hits {
            out.push_str(&old[last..s]);
            out.push_str(&with);
            end = out.len() as u32;
            last = e;
        }
        out.push_str(&old[last..]);
        d.set_text(&out);
        d.history.replaced(&old, &d.text, end, end);
        d.touch();
        self.place(end, end);
    }

    /// Ctrl+/: comment the covered lines out, or back in if all are.
    fn toggle_comment(&mut self) {
        let Some((open, close)) = comment_markers(self.cur().kind()) else { return };
        let d = self.cur_mut();
        let (a, c) = d.history.caret();
        let (ls, le) = covered(&d.text, a, c);
        let block = d.text[ls..le].to_string();
        let new_block = if close.is_empty() {
            let lines: Vec<&str> = block.split('\n').collect();
            let filled = || lines.iter().filter(|l| !l.trim().is_empty());
            let all = filled().all(|l| l.trim_start().starts_with(open));
            let col = filled().map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
            let mut out = String::with_capacity(block.len() + lines.len() * 3);
            for (n, l) in lines.iter().enumerate() {
                if n > 0 { out.push('\n'); }
                if l.trim().is_empty() { out.push_str(l); continue; }
                if all {
                    let ind = l.len() - l.trim_start().len();
                    let rest = &l[ind + open.len()..];
                    out.push_str(&l[..ind]);
                    out.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                } else {
                    out.push_str(&l[..col]);
                    out.push_str(open);
                    out.push(' ');
                    out.push_str(&l[col..]);
                }
            }
            out
        } else {
            let t = block.trim();
            let lead = &block[..block.len() - block.trim_start().len()];
            if t.starts_with(open) && t.ends_with(close) && t.len() >= open.len() + close.len() {
                let inner = &t[open.len()..t.len() - close.len()];
                let mut out = String::from(lead);
                let inner = inner.strip_prefix(' ').unwrap_or(inner);
                out.push_str(inner.strip_suffix(' ').unwrap_or(inner));
                out
            } else {
                let mut out = String::from(lead);
                out.push_str(open);
                out.push(' ');
                out.push_str(block.trim_start());
                out.push(' ');
                out.push_str(close);
                out
            }
        };
        if new_block == block { return; }
        let old = d.text.clone();
        d.text.replace_range(ls..le, &new_block);
        let (na, nc) = if a != c {
            (ls as u32, (ls + new_block.len()) as u32)
        } else {
            let delta = new_block.len() as isize - block.len() as isize;
            let p = (c as isize + delta).clamp(ls as isize, (ls + new_block.len()) as isize) as u32;
            (p, p)
        };
        d.history.replaced(&old, &d.text, na, nc);
        d.touch();
        self.place(na, nc);
    }

    /// Ctrl+Shift+K: remove the covered lines.
    fn delete_lines(&mut self) {
        let d = self.cur_mut();
        let (a, c) = d.history.caret();
        let (ls, le) = covered(&d.text, a, c);
        let (from, to) = if le < d.text.len() { (ls, le + 1) }
                         else if ls > 0 { (ls - 1, le) } else { (ls, le) };
        if from == to { return; }
        let old = d.text.clone();
        d.text.replace_range(from..to, "");
        let p = from.min(d.text.len());
        let p = if from < ls { d.text[..p].rfind('\n').map(|i| i + 1).unwrap_or(0) } else { p };
        d.history.replaced(&old, &d.text, p as u32, p as u32);
        d.touch();
        self.place(p as u32, p as u32);
    }

    /// The window got the keyboard back: what changed on disk meanwhile?
    /// A clean tab follows the file (as one undo step), a tab with its own
    /// edits asks.
    fn check_disk(&mut self) -> bool {
        let mut any = false;
        let active = self.active;
        for i in 0..self.docs.len() {
            let Some(path) = self.docs[i].path.clone() else { continue };
            let Some(disk) = host::fetch(&path) else { continue };
            let h = text_hash_bytes(&disk);
            let d = &mut self.docs[i];
            if h == d.saved || d.disk_changed { continue; }
            any = true;
            if d.dirty {
                d.disk_changed = true;
            } else if let Ok(t) = String::from_utf8(disk) {
                let (a, c) = d.history.caret();
                let (a, c) = (a.min(t.len() as u32), c.min(t.len() as u32));
                let old = core::mem::replace(&mut d.text, t);
                d.history.replaced(&old, &d.text, a, c);
                d.saved = h;
                d.touch();
                if i == active { self.place(a, c); }
            }
        }
        any
    }

    fn disk_reload(&mut self) {
        let Some(path) = self.cur().path.clone() else { return };
        let Some(t) = read_file(&path) else { return };
        let d = self.cur_mut();
        let (a, c) = d.history.caret();
        let (a, c) = (a.min(t.len() as u32), c.min(t.len() as u32));
        let old = core::mem::replace(&mut d.text, t);
        d.history.replaced(&old, &d.text, a, c);
        d.saved = text_hash(&d.text);
        d.disk_changed = false;
        d.touch();
        self.place(a, c);
    }

    /// Keep the tab's text; the next save overwrites the file.
    fn disk_keep(&mut self) {
        let Some(path) = self.cur().path.clone() else { return };
        let disk = host::fetch(&path).map(|b| text_hash_bytes(&b));
        let d = self.cur_mut();
        if let Some(h) = disk { d.saved = h; }
        d.disk_changed = false;
        d.touch();
    }

    /// Ctrl+PageUp/Down: the tab before or after, wrapping.
    fn cycle_tab(&mut self, dir: isize) {
        let n = self.docs.len() as isize;
        self.active = ((self.active as isize + dir).rem_euclid(n)) as usize;
    }

    fn open_goto(&mut self) {
        self.bar = Some(Bar::Goto(String::new()));
        self.focus_on(Focus::Goto);
    }

    /// Jump to the line typed into the go-to bar (1-based).
    fn goto_submit(&mut self) {
        let n = match &self.bar {
            Some(Bar::Goto(t)) => t.trim().parse::<usize>().unwrap_or(0),
            _ => return,
        };
        if n == 0 { return; }
        let off = line_offset(&self.cur().text, n - 1) as u32;
        self.bar = None;
        self.focus_on(Focus::Editor);
        self.place(off, off);
    }
}

/// FNV-1a over a text, for "is this what is on disk".
fn text_hash(t: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in t.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn text_hash_bytes(b: &[u8]) -> u64 {
    match core::str::from_utf8(b) { Ok(t) => text_hash(t), Err(_) => 0 }
}

/// `t` without spaces and tabs at line ends, with `(anchor, cursor)`
/// moved along: by what went before them, onto the line's text when they
/// stood in what went.
fn trim_trailing(t: &str, anchor: u32, cursor: u32) -> (String, u32, u32) {
    let mut out = String::with_capacity(t.len());
    let (mut a, mut c) = (anchor as usize, cursor as usize);
    let (oa, oc) = (a, c);
    let mut removed = 0usize;
    let mut ls = 0usize;
    for line in t.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let kept = body.trim_end_matches([' ', '\t']);
        let cut_from = ls + kept.len();
        let cut_to = ls + body.len();
        for (orig, new) in [(oa, &mut a), (oc, &mut c)] {
            if orig > cut_from && orig <= cut_to { *new = cut_from - removed; }
            else if orig > cut_to { *new = orig - removed - (cut_to - cut_from); }
            else if orig <= cut_from && orig >= ls { *new = orig - removed; }
        }
        removed += cut_to - cut_from;
        out.push_str(kept);
        if line.ends_with('\n') { out.push('\n'); }
        ls += line.len();
    }
    (out, a as u32, c as u32)
}

/// Line comment marker, or block markers, for a kind.
fn comment_markers(k: Kind) -> Option<(&'static str, &'static str)> {
    match k {
        Kind::Code(Lang::Rust | Lang::C | Lang::Js) => Some(("//", "")),
        Kind::Code(Lang::Python | Lang::Shell | Lang::Toml | Lang::Yaml | Lang::Make) => Some(("#", "")),
        Kind::Code(Lang::Css) => Some(("/*", "*/")),
        Kind::Code(Lang::Markup) | Kind::Markdown => Some(("<!--", "-->")),
        _ => None,
    }
}

/// AutoClose bits for a kind: brackets, then the quotes the language uses.
fn pair_bits(k: Kind) -> u8 {
    match k {
        Kind::Code(Lang::Rust) => 1 | 2,
        Kind::Code(Lang::Js) => 1 | 2 | 4 | 8,
        Kind::Code(Lang::C | Lang::Python | Lang::Shell | Lang::Toml | Lang::Yaml | Lang::Make) => 1 | 2 | 4,
        Kind::Code(Lang::Json | Lang::Css) => 1 | 2,
        Kind::Code(Lang::Markup) => 2,
        _ => 0,
    }
}

/// The line range `[start, end)` the caret or selection covers; a
/// selection ending at a line start leaves that line out.
fn covered(t: &str, anchor: u32, cursor: u32) -> (usize, usize) {
    let (a, mut b) = (anchor.min(cursor) as usize, anchor.max(cursor) as usize);
    let a = a.min(t.len());
    b = b.min(t.len());
    if b > a && t[..b].ends_with('\n') { b -= 1; }
    let ls = t[..a].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let le = t[b..].find('\n').map(|i| b + i).unwrap_or(t.len());
    (ls, le)
}

/// The bracket matching the one at or just before `at`, as both offsets.
fn bracket_pair(t: &str, at: usize) -> Option<(usize, usize)> {
    let b = t.as_bytes();
    let pick = |i: usize| -> Option<(usize, u8, u8, bool)> {
        match *b.get(i)? {
            b'(' => Some((i, b'(', b')', true)), b'[' => Some((i, b'[', b']', true)),
            b'{' => Some((i, b'{', b'}', true)), b')' => Some((i, b'(', b')', false)),
            b']' => Some((i, b'[', b']', false)), b'}' => Some((i, b'{', b'}', false)),
            _ => None,
        }
    };
    let (i, open, close, fwd) = pick(at).or_else(|| at.checked_sub(1).and_then(pick))?;
    const SCAN: usize = 200_000;
    let mut depth = 0i32;
    if fwd {
        for (k, &c) in b.iter().enumerate().skip(i).take(SCAN) {
            if c == open { depth += 1; }
            if c == close { depth -= 1; if depth == 0 { return Some((i, k)); } }
        }
    } else {
        let lo = i.saturating_sub(SCAN);
        for k in (lo..=i).rev() {
            if b[k] == close { depth += 1; }
            if b[k] == open { depth -= 1; if depth == 0 { return Some((k, i)); } }
        }
    }
    None
}

/// Indent of a file: tabs if more lines start with a tab than with
/// spaces, else the smallest space indent of 2 or more (4 if none).
fn detect_indent(t: &str) -> (u8, bool) {
    let (mut tabs, mut spaced, mut min) = (0usize, 0usize, usize::MAX);
    for line in t.lines().take(2_000) {
        if line.starts_with('\t') { tabs += 1; continue; }
        let n = line.bytes().take_while(|b| *b == b' ').count();
        if n >= 2 && n < line.len() {
            spaced += 1;
            min = min.min(n);
        }
    }
    if tabs > spaced { return (4, true); }
    match min {
        2 | 3 => (min as u8, false),
        usize::MAX => (4, false),
        _ => (4, false),
    }
}

/// Byte offset of the start of line `n` (0-based), or the end of the text.
fn line_offset(t: &str, n: usize) -> usize {
    if n == 0 { return 0; }
    t.match_indices('\n').nth(n - 1).map(|(i, _)| i + 1).unwrap_or(t.len())
}

/// 1-based line and column (in characters) of byte offset `at`.
fn line_col(t: &str, at: usize) -> (usize, usize) {
    let at = at.min(t.len());
    let mut at_b = at;
    while at_b > 0 && !t.is_char_boundary(at_b) { at_b -= 1; }
    let before = &t[..at_b];
    let line = before.matches('\n').count() + 1;
    let ls = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    (line, before[ls..].chars().count() + 1)
}

/// Every non-overlapping occurrence of `q` in `t`, as byte ranges.
/// Without `case`, ASCII letters match either case.
fn find_all(t: &str, q: &str, case: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if q.is_empty() { return out; }
    let (tb, qb) = (t.as_bytes(), q.as_bytes());
    let mut i = 0;
    while i + qb.len() <= tb.len() && out.len() < MAX_HITS {
        let hit = if case { &tb[i..i + qb.len()] == qb }
                  else { tb[i..i + qb.len()].eq_ignore_ascii_case(qb) };
        if hit && t.is_char_boundary(i) && t.is_char_boundary(i + qb.len()) {
            out.push((i, i + qb.len()));
            i += qb.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Ask the kernel for a file dialog. `tag` comes back in `Event::Picked`.
fn pick(mode: host::PickMode, start: &str, suggest: &str, tag: u32) {
    host::pick(mode, start, suggest, tag);
}

/// Fetch a file's contents as a String, or None on error, an empty file or
/// non-UTF-8. The file comes whole; a cut document must never be opened,
/// or Ctrl+S writes it back cut.
fn read_file(path: &str) -> Option<String> {
    let buf = host::fetch(path)?;
    if buf.is_empty() { return None; }
    String::from_utf8(buf).ok()
}

// ── Config ────────────────────────────────────────────────────────────
//
// `sys/config/spell`, same shape as `sys/config/bar`: one `key: value`
// per line, `#` starts a comment. Meant to be opened and edited in spell
// itself, so it carries a header explaining what is in it and unknown
// keys are ignored rather than rejected — a newer spell's settings must
// not make an older one refuse the file.

const CONFIG_PATH: &str = "sys/config/spell";

/// The settings file, and the state spell keeps there across runs.
#[derive(Clone, PartialEq, Eq)]
struct Config {
    font:     u16,
    trim:     bool,
    final_nl: bool,
    recent:   Vec<String>,
    open:     Vec<String>,
}

/// Read the settings. Anything missing or unparseable falls back to the
/// default — a broken config should never keep the editor shut.
fn read_config() -> Config {
    let mut c = Config { font: MONO_SIZE_PX, trim: false, final_nl: false,
                         recent: Vec::new(), open: Vec::new() };
    let Some(buf) = host::fetch(CONFIG_PATH) else { return c };
    let Ok(text) = core::str::from_utf8(&buf) else { return c };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((key, val)) = line.split_once(':') else { continue };
        let val = val.trim();
        let yes = matches!(val, "yes" | "true" | "on" | "1");
        match key.trim() {
            "font" => if let Ok(v) = val.parse::<u16>() {
                c.font = v.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
            },
            "trim_trailing" => c.trim = yes,
            "final_newline" => c.final_nl = yes,
            "recent" if !val.is_empty() && c.recent.len() < MAX_RECENT => c.recent.push(val.to_string()),
            "open" if !val.is_empty() => c.open.push(val.to_string()),
            _ => {}
        }
    }
    c
}

/// Write the settings back, header and all.
fn write_config(c: &Config) {
    let mut out = String::with_capacity(512);
    out.push_str(s().cfg_header);
    out.push_str("font: ");
    push_usize(&mut out, c.font as usize);
    out.push_str("\ntrim_trailing: ");
    out.push_str(if c.trim { "yes" } else { "no" });
    out.push_str("\nfinal_newline: ");
    out.push_str(if c.final_nl { "yes" } else { "no" });
    out.push('\n');
    for p in &c.recent { out.push_str("recent: "); out.push_str(p); out.push('\n'); }
    for p in &c.open { out.push_str("open: "); out.push_str(p); out.push('\n'); }
    if !host::store(CONFIG_PATH, out.as_bytes()) { log("[spell] config: store failed"); }
}

// ── Path helpers ──────────────────────────────────────────────────────

fn basename(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

// ── Render ────────────────────────────────────────────────────────────

fn render(sp: &Spell) -> Widget {
    let mut children: Vec<Widget> = alloc::vec![
        render_menu_bar(),
        Widget::Divider,
        render_tabbar(sp),
        Widget::Divider,
    ];
    let editing = sp.confirm_close.is_none();
    if editing && sp.cur().disk_changed {
        children.push(render_disk_strip());
        children.push(Widget::Divider);
    }
    if editing {
        if let Some(bar) = render_bar(sp) {
            children.push(bar);
            children.push(Widget::Divider);
        }
    }
    children.push(render_body(sp));   // Flex(1) — fills
    if editing {
        children.push(Widget::Divider);
        children.push(render_status(sp));
    }

    if let Some(kind) = sp.open_menu {
        let (anchor, content) = render_dropdown(sp, kind);
        children.push(Widget::Popover {
            anchor:     NodeId(anchor),
            child:      Box::new(content),
            on_dismiss: ActionId(ACT_MENU_DISMISS),
            modifiers:  alloc::vec![],
        });
    }

    Widget::Column {
        children,
        spacing:   Spacing::None.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![],
    }
}

fn render_menu_bar() -> Widget {
    let labels: Vec<(String, ActionId)> = alloc::vec![
        (s().menu_file.to_string(), ActionId(ACT_MENU_FILE)),
        (s().menu_edit.to_string(), ActionId(ACT_MENU_EDIT)),
        (s().menu_view.to_string(), ActionId(ACT_MENU_VIEW)),
        (s().menu_help.to_string(), ActionId(ACT_MENU_HELP)),
    ];
    let anchors: Vec<NodeId> = alloc::vec![
        NodeId(NODE_MENU_FILE),
        NodeId(NODE_MENU_EDIT),
        NodeId(NODE_MENU_VIEW),
        NodeId(NODE_MENU_HELP),
    ];
    prefab::menu_bar_with_icon(IconId::FileText, &labels, &anchors)
}

fn render_dropdown(sp: &Spell, kind: OpenMenu) -> (u32, Widget) {
    match kind {
        // The menu is where the shortcuts are learned, so they're listed
        // next to the entry that binds them.
        OpenMenu::File => {
            let mut items: Vec<(String, ActionId)> = alloc::vec![
                (s().new.to_string(), ActionId(ACT_FILE_NEW)),
                (s().open.to_string(), ActionId(ACT_FILE_OPEN)),
                (s().save.to_string(), ActionId(ACT_FILE_SAVE)),
                (s().save_as.to_string(), ActionId(ACT_FILE_SAVE_AS)),
                (s().close.to_string(), ActionId(ACT_FILE_CLOSE)),
                (s().settings.to_string(), ActionId(ACT_FILE_SETTINGS)),
            ];
            let mut keys: Vec<&str> = alloc::vec!["Ctrl+N", "Ctrl+O", "Ctrl+S", "Ctrl+Shift+S", "Ctrl+W", ""];
            // Recent files, other than the ones open, under the name and
            // with the folder as the hint.
            let open: Vec<&str> = sp.docs.iter().filter_map(|d| d.path.as_deref()).collect();
            for (i, p) in sp.recent.iter().enumerate() {
                if open.contains(&p.as_str()) { continue; }
                items.push((basename(p).to_string(), ActionId(ACT_RECENT_BASE + i as u32)));
                keys.push(dirname(p));
            }
            (NODE_MENU_FILE, prefab::popover_menu_shortcuts(&items, &keys, None))
        }
        OpenMenu::Edit => (
            NODE_MENU_EDIT,
            prefab::popover_menu_shortcuts(&[
                (s().undo.to_string(), ActionId(ACT_UNDO)),
                (s().redo.to_string(), ActionId(ACT_REDO)),
                (s().find.to_string(), ActionId(ACT_FIND)),
                (s().replace.to_string(), ActionId(ACT_REPLACE)),
                (s().goto_line.to_string(), ActionId(ACT_GOTO)),
                (s().toggle_comment.to_string(), ActionId(ACT_COMMENT)),
                (s().delete_line.to_string(), ActionId(ACT_DELETE_LINE)),
            ], &["Ctrl+Z", "Ctrl+Y", "Ctrl+F", "Ctrl+H", "Ctrl+G", "Ctrl+/", "Ctrl+Shift+K"], None),
        ),
        OpenMenu::View => (
            NODE_MENU_VIEW,
            prefab::popover_menu_shortcuts(&[
                (s().view_source.to_string(), ActionId(ACT_VIEW_EDIT)),
                (s().view_preview.to_string(), ActionId(ACT_VIEW_PREVIEW)),
                (s().zoom_in.to_string(), ActionId(ACT_ZOOM_IN)),
                (s().zoom_out.to_string(), ActionId(ACT_ZOOM_OUT)),
                (zoom_reset_label(), ActionId(ACT_ZOOM_RESET)),
            ], &["", "", "Ctrl++", "Ctrl+-", "Ctrl+0"],
               Some(match sp.cur().mode { Mode::Edit => 0, Mode::Preview => 1 })),
        ),
        OpenMenu::Help => (
            NODE_MENU_HELP,
            prefab::popover_menu(&[
                (s().about.to_string(), ActionId(ACT_HELP_ABOUT)),
            ], None),
        ),
    }
}

/// Tab bar — one tab per open document, active tab highlighted, each
/// with a dirty dot and a close (×). Trailing "+" opens a new tab.
/// Save lives in the File menu, the markdown view toggle in View.
/// Fixed tab width — names ellipsize rather than letting the strip
/// reflow as you open files (docs/spec/UI_REFRESH.md §3 `tab`).
const TAB_W: u16 = 200;
const TAB_H: u16 = 30;
/// Rounded on top only would need per-corner radii; the strip clips the
/// bottom edge visually because the active tab shares the body's colour.
const TAB_RADIUS: u8 = 8;

fn render_tabbar(sp: &Spell) -> Widget {
    let mut tabs: Vec<Widget> = Vec::with_capacity(sp.docs.len() + 1);
    for (i, d) in sp.docs.iter().enumerate() {
        let active = i == sp.active;
        let mut mods: Vec<Modifier> = alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::MinWidth(TAB_W),
            Modifier::MaxWidth(TAB_W),
            Modifier::MinHeight(TAB_H),
            Modifier::OnClick(ActionId(ACT_TAB_BASE + i as u32)),
            Modifier::Rounded(TAB_RADIUS),
        ];
        if active {
            // The active tab carries the colour of the content below it,
            // so the two read as one surface.
            mods.push(Modifier::Background(Token::Page));
        } else {
            mods.push(Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Rounded(TAB_RADIUS),
            ]));
        }

        // The active tab offers the ×, in the accent colour when its work
        // is unsaved. An inactive unsaved tab shows an accent dot that
        // closes it too; either way the unsaved-changes dialog asks first.
        // A saved, inactive tab shows neither.
        let close = ActionId(ACT_TAB_CLOSE_BASE + i as u32);
        let trailing = if active {
            Widget::Icon {
                id:   IconId::X,
                size: 16,
                modifiers: alloc::vec![
                    Modifier::OnClick(close),
                    Modifier::Tint(if d.dirty { Token::Accent } else { Token::OnSurfaceMuted }),
                ],
            }
        } else if d.dirty {
            Widget::Row {
                children:  alloc::vec![prefab::mark(8, 8, Some(Token::Accent))],
                spacing:   0,
                align:     Align::Center,
                modifiers: alloc::vec![Modifier::Rounded(4), Modifier::OnClick(close)],
            }
        } else {
            prefab::mark(8, 8, None)
        };

        tabs.push(Widget::Row {
            children: alloc::vec![
                Widget::Icon {
                    id:   tab_icon(d),
                    size: 16,
                    modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceFaint)],
                },
                Widget::Text {
                    content:   d.title.clone(),
                    style:     TextStyle::Body,
                    modifiers: if active { alloc::vec![] }
                               else { alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                },
                Widget::Spacer { flex: 1 },
                trailing,
            ],
            spacing:   Spacing::Sm.as_u16(),
            align:     Align::Center,
            modifiers: mods,
        });
    }
    // New-tab button.
    tabs.push(Widget::Text {
        content:   "+".to_string(),
        style:     TextStyle::Body,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::MinHeight(TAB_H),
            Modifier::Tint(Token::OnSurfaceFaint),
            Modifier::OnClick(ActionId(ACT_FILE_NEW)),
            Modifier::Rounded(TAB_RADIUS),
            Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Rounded(TAB_RADIUS),
            ]),
        ],
    });
    Widget::Scroll {
        child: Box::new(Widget::Row {
            children:  tabs,
            spacing:   Spacing::Xxs.as_u16(),
            align:     Align::End,
            modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
        }),
        axis:      Axis::Horizontal,
        modifiers: alloc::vec![Modifier::Background(Token::SurfaceElevated)],
    }
}

/// Tab glyph by file kind — plain text vs. source.
fn tab_icon(d: &Doc) -> IconId {
    match d.kind() {
        Kind::Code(_) => IconId::Code,
        _             => IconId::FileText,
    }
}

fn render_body(sp: &Spell) -> Widget {
    if sp.confirm_close.is_some() {
        return render_confirm_dialog(sp);
    }
    // Markdown is the special case: a rendered preview you toggle to.
    // Everything else is the live-highlighted editor — code gets colour
    // spans the compositor paints while you type, no toggle needed.
    let doc = sp.cur();
    if matches!(doc.kind(), Kind::Markdown) && doc.mode == Mode::Preview {
        return Widget::Scroll {
            child:     Box::new(markdown_preview(&doc.text)),
            axis:      Axis::Vertical,
            modifiers: alloc::vec![
                Modifier::Flex(1),
                Modifier::Background(Token::Page),
                Modifier::Padding(Padding::Md.as_u16()),
            ],
        };
    }
    // Overlays in priority order: find hits, then the bracket next to
    // the caret and its partner.
    let mut over: Vec<Span> = Vec::new();
    if let Some(Bar::Find(f)) = &sp.bar {
        for (i, (a, b)) in sp.hits().into_iter().enumerate() {
            let token = if i == f.cur { Token::Accent } else { Token::Warning };
            over.push(Span { start: a as u32, len: (b - a) as u32, token });
        }
    }
    let (ca, cc) = doc.history.caret();
    if ca == cc {
        if let Some((x, y)) = bracket_pair(&doc.text, cc as usize) {
            over.push(Span { start: x as u32, len: 1, token: Token::Accent });
            over.push(Span { start: y as u32, len: 1, token: Token::Accent });
        }
    }
    let spans = overlay_spans(&doc.spans, over);
    let node = doc.node();
    let mut modifiers = alloc::vec![
        Modifier::Flex(1),
        Modifier::Background(Token::Page),
        Modifier::Padding(Padding::Sm.as_u16()),
        Modifier::LineNumbers(true),
        Modifier::FontSize(sp.font_px),
        // Typing works without a click: on open, and whenever focus
        // has nowhere else to be (new tab, closed dialog).
        Modifier::Autofocus,
        Modifier::NodeId(NodeId(node)),
        Modifier::Indent {
            width: doc.indent.0,
            tabs:  doc.indent.1,
            colon: matches!(doc.kind(), Kind::Code(Lang::Python)),
        },
        Modifier::AutoClose(pair_bits(doc.kind())),
    ];
    if let Some((n, seq, a, c)) = sp.select {
        if n == node {
            modifiers.push(Modifier::Select { seq, anchor: a, cursor: c });
        }
    }
    if let Some((Focus::Editor, seq)) = sp.focus {
        modifiers.push(Modifier::FocusRequest(seq));
    }
    Widget::TextArea {
        value:       doc.text.clone(),
        placeholder: s().placeholder.to_string(),
        spans,
        modifiers,
    }
}

/// A text field of the bar: its node, and the focus request when it is
/// the one that should have the keyboard.
fn bar_input(sp: &Spell, value: &str, placeholder: &str, node: u32, me: Focus,
             submit: u32, width: u16) -> Widget {
    let mut mods = alloc::vec![
        Modifier::NodeId(NodeId(node)),
        Modifier::MinWidth(width),
    ];
    if let Some((f, seq)) = sp.focus {
        if f == me { mods.push(Modifier::FocusRequest(seq)); }
    }
    let input = Widget::Input {
        value:       value.to_string(),
        placeholder: placeholder.to_string(),
        on_submit:   ActionId(submit),
        modifiers:   mods,
    };
    Widget::Row {
        children:  alloc::vec![input],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Background(Token::Page),
            Modifier::Rounded(6),
            Modifier::Focus(alloc::vec![
                Modifier::Border { token: Token::Accent, width: 1, radius: 6 },
            ]),
        ],
    }
}

fn bar_button(label: &str, action: u32, on: bool) -> Widget {
    Widget::Text {
        content:   label.to_string(),
        style:     TextStyle::Body,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Rounded(6),
            Modifier::OnClick(ActionId(action)),
            Modifier::Tint(if on { Token::Accent } else { Token::OnSurfaceMuted }),
            Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceHover),
                Modifier::Rounded(6),
            ]),
        ],
    }
}

fn bar_icon(id: IconId, action: u32) -> Widget {
    Widget::Icon {
        id,
        size: 16,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::OnClick(ActionId(action)),
            Modifier::Tint(Token::OnSurfaceMuted),
            Modifier::Rounded(6),
            Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceHover),
                Modifier::Rounded(6),
            ]),
        ],
    }
}

fn bar_row(children: Vec<Widget>) -> Widget {
    Widget::Row {
        children,
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![],
    }
}

/// Find / replace / go-to strip above the editor.
fn render_bar(sp: &Spell) -> Option<Widget> {
    let rows = match sp.bar.as_ref()? {
        Bar::Find(f) => {
            let hits = sp.hits();
            let count = if f.query.is_empty() {
                String::new()
            } else if hits.is_empty() {
                s().no_results.to_string()
            } else {
                step_label(f.cur.min(hits.len() - 1) + 1, hits.len())
            };
            let mut rows = alloc::vec![bar_row(alloc::vec![
                bar_input(sp, &f.query, s().find_ph, NODE_FIND, Focus::Query, ACT_FIND_NEXT, 240),
                bar_button("Aa", ACT_FIND_CASE, f.case),
                Widget::Text {
                    content:   count,
                    style:     TextStyle::Caption,
                    modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceFaint), Modifier::MinWidth(80)],
                },
                bar_icon(IconId::ArrowUp, ACT_FIND_PREV),
                bar_icon(IconId::ArrowDown, ACT_FIND_NEXT),
                Widget::Spacer { flex: 1 },
                bar_icon(IconId::X, ACT_FIND_CLOSE),
            ])];
            if let Some(r) = &f.replace {
                rows.push(bar_row(alloc::vec![
                    bar_input(sp, r, s().replace_ph, NODE_REPLACE, Focus::Replace, ACT_REPLACE_ONE, 240),
                    bar_button(s().replace_one, ACT_REPLACE_ONE, false),
                    bar_button(s().replace_all, ACT_REPLACE_ALL, false),
                ]));
            }
            rows
        }
        Bar::Goto(t) => {
            let mut n = String::with_capacity(8);
            push_usize(&mut n, sp.cur().text.matches('\n').count() + 1);
            alloc::vec![bar_row(alloc::vec![
                bar_input(sp, t, &fill(s().goto_ph, &n), NODE_GOTO, Focus::Goto, ACT_GOTO_SUBMIT, 200),
                Widget::Spacer { flex: 1 },
                bar_icon(IconId::X, ACT_FIND_CLOSE),
            ])]
        }
    };
    Some(Widget::Column {
        children:  rows,
        spacing:   Spacing::Xs.as_u16(),
        align:     Align::Stretch,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::Background(Token::SurfaceElevated),
        ],
    })
}

/// The file changed on disk under a tab with edits of its own.
fn render_disk_strip() -> Widget {
    Widget::Row {
        children: alloc::vec![
            Widget::Icon { id: IconId::FileText, size: 16, modifiers: alloc::vec![Modifier::Tint(Token::Warning)] },
            Widget::Text { content: s().disk_changed.to_string(), style: TextStyle::Body, modifiers: alloc::vec![] },
            Widget::Spacer { flex: 1 },
            bar_button(s().reload, ACT_DISK_RELOAD, true),
            bar_button(s().keep_mine, ACT_DISK_KEEP, false),
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Xs.as_u16() },
            Modifier::Background(Token::SurfaceElevated),
        ],
    }
}

/// Status line: caret position on the left, the file's facts on the right.
fn render_status(sp: &Spell) -> Widget {
    let d = sp.cur();
    let (a, c) = d.history.caret();
    let (line, col) = line_col(&d.text, c as usize);
    let mut l = String::with_capacity(8);
    push_usize(&mut l, line);
    let mut k = String::with_capacity(8);
    push_usize(&mut k, col);
    let mut left = fill(&fill(s().ln_col, &l), &k);
    if a != c {
        let (lo, hi) = (a.min(c) as usize, a.max(c) as usize);
        let n = d.text.get(lo..hi).map(|t| t.chars().count()).unwrap_or(0);
        let mut m = String::with_capacity(8);
        push_usize(&mut m, n);
        left.push_str("   ");
        left.push_str(&fill(s().selected, &m));
    }
    let mut right = String::with_capacity(48);
    right.push_str(lang_name(d.kind()));
    right.push_str("   ");
    if d.indent.1 {
        right.push_str(s().tabs);
    } else {
        let mut w = String::with_capacity(2);
        push_usize(&mut w, d.indent.0 as usize);
        right.push_str(&fill(s().spaces, &w));
    }
    right.push_str("   ");
    right.push_str(if d.text.contains("\r\n") { "CRLF" } else { "LF" });
    right.push_str("   UTF-8");
    let text = |t: String| Widget::Text {
        content:   t,
        style:     TextStyle::Caption,
        modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
    };
    Widget::Row {
        children:  alloc::vec![text(left), Widget::Spacer { flex: 1 }, text(right)],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Xxs.as_u16() },
            Modifier::Background(Token::SurfaceElevated),
        ],
    }
}

fn lang_name(k: Kind) -> &'static str {
    match k {
        Kind::Markdown        => "Markdown",
        Kind::Code(Lang::Rust)   => "Rust",
        Kind::Code(Lang::C)      => "C",
        Kind::Code(Lang::Js)     => "JavaScript",
        Kind::Code(Lang::Python) => "Python",
        Kind::Code(Lang::Shell)  => "Shell",
        Kind::Code(Lang::Json)   => "JSON",
        Kind::Code(Lang::Markup) => "HTML/XML",
        Kind::Code(Lang::Toml)   => "TOML",
        Kind::Code(Lang::Yaml)   => "YAML",
        Kind::Code(Lang::Css)    => "CSS",
        Kind::Code(Lang::Diff)   => "Diff",
        Kind::Code(Lang::Make)   => "Makefile",
        Kind::Plain | Kind::Untyped => "Plain text",
    }
}

/// Unsaved-changes confirmation shown when closing a dirty tab. Buttons
/// only (no text input), so no focus dance needed.
///
/// While quitting this is one step of a sequence, so it carries a
/// "(2 of 3)" counter — otherwise a second dialog appearing right after
/// the first reads like the button didn't take.
/// Discard throws work away, so it says so in red, but as a quiet
/// button: Save is the one filled action in the card.
fn discard_button() -> Widget {
    let mut b = prefab::button(s().discard, prefab::ButtonStyle::Ghost, ActionId(ACT_CLOSE_DISCARD));
    if let Widget::Button { modifiers, .. } = &mut b {
        modifiers.push(Modifier::Tint(Token::Danger));
    }
    b
}

fn render_confirm_dialog(sp: &Spell) -> Widget {
    let idx = sp.confirm_close.unwrap_or(0);
    let title = match sp.docs.get(idx) {
        Some(d) => d.title.clone(),
        None => s().menu_file.to_string(),
    };
    let total = sp.quit_total;
    let mut body: Vec<Widget> = Vec::with_capacity(3);
    body.push(Widget::Text {
        content:   fill(s().unsaved_body, &title),
        style:     TextStyle::Body,
        modifiers: alloc::vec![],
    });
    if sp.quitting && total > 1 {
        body.push(Widget::Text {
            content:   step_label(sp.quit_done + 1, total),
            style:     TextStyle::Caption,
            modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceFaint)],
        });
    }
    let card = prefab::dialog(
        s().unsaved_title,
        Widget::Column {
            children: alloc::vec![
                Widget::Column {
                    children:  body,
                    spacing:   Spacing::Xs.as_u16(),
                    align:     Align::Stretch,
                    modifiers: alloc::vec![],
                },
                Widget::Row {
                    children: alloc::vec![
                        discard_button(),
                        Widget::Spacer { flex: 1 },
                        prefab::button(s().cancel, prefab::ButtonStyle::Ghost, ActionId(ACT_CLOSE_CANCEL)),
                        prefab::button(s().save, prefab::ButtonStyle::Primary, ActionId(ACT_CLOSE_SAVE)),
                    ],
                    spacing:   Spacing::Sm.as_u16(),
                    align:     Align::Center,
                    modifiers: alloc::vec![],
                },
            ],
            spacing:   Spacing::Md.as_u16(),
            align:     Align::Stretch,
            modifiers: alloc::vec![],
        },
        None,
        380,
    );
    Widget::Column {
        children:  alloc::vec![Widget::Spacer { flex: 1 }, card, Widget::Spacer { flex: 1 }],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Flex(1), Modifier::Padding(Padding::Lg.as_u16())],
    }
}

// ── Markdown preview ──────────────────────────────────────────────────

fn markdown_preview(text: &str) -> Widget {
    let mut blocks: Vec<Widget> = Vec::new();
    let mut fence = false;
    let mut code: Vec<String> = Vec::new();

    for line in text.split('\n') {
        if fence {
            if line.trim_start().starts_with("```") {
                blocks.push(code_block(&code));
                code.clear();
                fence = false;
            } else {
                code.push(line.to_string());
            }
            continue;
        }
        let t = line.trim_start();
        if t.starts_with("```") {
            fence = true;
            code.clear();
        } else if let Some(rest) = t.strip_prefix("### ") {
            blocks.push(heading(rest, TextStyle::Heading));
        } else if let Some(rest) = t.strip_prefix("## ") {
            blocks.push(heading(rest, TextStyle::Title));
        } else if let Some(rest) = t.strip_prefix("# ") {
            blocks.push(heading(rest, TextStyle::Title));
        } else if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            blocks.push(bullet(rest));
        } else if t.is_empty() {
            blocks.push(Widget::Spacer { flex: 0 });
        } else {
            blocks.push(paragraph(line));
        }
    }
    if fence { blocks.push(code_block(&code)); }

    Widget::Column {
        children:  blocks,
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Start,
        modifiers: alloc::vec![],
    }
}

fn heading(text: &str, style: TextStyle) -> Widget {
    Widget::Text {
        content:   text.to_string(),
        style,
        modifiers: alloc::vec![Modifier::Tint(Token::Accent)],
    }
}

fn bullet(text: &str) -> Widget {
    Widget::Row {
        children: alloc::vec![
            Widget::Text { content: "•".to_string(), style: TextStyle::Body, modifiers: alloc::vec![Modifier::Tint(Token::Accent)] },
            paragraph(text),
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Start,
        modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

/// Paragraph with inline `code` spans rendered as Mono + muted tint.
/// Other inline markup (bold, italic) is left as literal text — there is
/// no bold weight in the text vocab.
fn paragraph(text: &str) -> Widget {
    // Odd split segments sit between backticks → inline code.
    let mut spans: Vec<Widget> = Vec::new();
    for (i, part) in text.split('`').enumerate() {
        if part.is_empty() { continue; }
        let (style, mods) = if i % 2 == 1 {
            (TextStyle::Mono, alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)])
        } else {
            (TextStyle::Body, alloc::vec![])
        };
        spans.push(Widget::Text { content: part.to_string(), style, modifiers: mods });
    }
    if spans.len() == 1 {
        return spans.pop().unwrap();
    }
    Widget::Row {
        children:  spans,
        spacing:   0,
        align:     Align::Start,
        modifiers: alloc::vec![],
    }
}

fn code_block(lines: &[String]) -> Widget {
    let rows: Vec<Widget> = lines.iter().map(|l| Widget::Text {
        content:   l.clone(),
        style:     TextStyle::Mono,
        modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
    }).collect();
    let col = Widget::Column {
        children:  rows,
        spacing:   0,
        align:     Align::Start,
        modifiers: alloc::vec![],
    };
    prefab::card(col, prefab::CardKind::Inset)
}

// ── Syntax highlighting → colour spans (live in the TextArea) ─────────
//
// One pass over the whole buffer, not line by line. Block comments,
// triple-quoted strings and JS templates all run across newlines, and a
// per-line scanner has to guess at every line start which construct it
// is standing inside — it guesses wrong on exactly the files people
// write.
//
// Spans cover only what is not default-coloured; uncovered bytes render
// in `OnSurface`, which is most of a file. They come out sorted by
// `start` (the scan only ever moves forward), which the compositor's
// renderer relies on.

/// `base` (sorted, non-overlapping) with `over` laid on top, as the
/// TextArea contract wants it: sorted by `start`, non-overlapping. An
/// overlay hides what is under it; among overlays the earlier one wins.
fn overlay_spans(base: &[Span], over: Vec<Span>) -> Vec<Span> {
    let mut top: Vec<Span> = Vec::with_capacity(over.len());
    for o in over {
        if o.len == 0 { continue; }
        let (a, b) = (o.start, o.start + o.len);
        if top.iter().all(|t| b <= t.start || t.start + t.len <= a) {
            top.push(o);
        }
    }
    if top.is_empty() { return base.to_vec(); }
    top.sort_unstable_by_key(|t| t.start);
    let mut out = Vec::with_capacity(base.len() + 2 * top.len());
    let mut ti = 0;
    for s in base {
        let mut a = s.start;
        let b = s.start + s.len;
        while ti < top.len() && top[ti].start < b {
            let t = top[ti];
            let te = t.start + t.len;
            if t.start > a {
                out.push(Span { start: a, len: t.start - a, token: s.token });
            }
            if te <= b || te <= a {
                out.push(t);
                ti += 1;
                a = a.max(te);
            } else {
                // Runs past this span: emit it once the base catches up.
                a = b;
                break;
            }
        }
        if a < b && (ti >= top.len() || top[ti].start >= b) {
            out.push(Span { start: a, len: b - a, token: s.token });
        }
    }
    out.extend_from_slice(&top[ti..]);
    out
}

fn push_span(out: &mut Vec<Span>, start: usize, len: usize, token: Token) {
    if len == 0 { return; }
    out.push(Span { start: start as u32, len: len as u32, token });
}

/// Per-language lexer knobs.
///
/// `decl` and `ctrl` are split the way the themes split them —
/// storage/declaration against control flow and imports, which common
/// editor themes paint in two different colours.
struct Syntax {
    decl:      &'static [&'static str],
    ctrl:      &'static [&'static str],
    consts:    &'static [&'static str],
    /// Built-in type names that are not keywords: `u32`, `str`, `bytes`.
    types:     &'static [&'static str],
    /// Names that are callable without parentheses (shell builtins).
    builtins:  &'static [&'static str],
    /// Line-comment marker, or "" for none.
    line:      &'static str,
    block:     Option<(&'static str, &'static str)>,
    /// Rust nests `/* /* */ */`; C and JS do not.
    nest_block: bool,
    /// `'x'` is a string. False for Rust, where `'a` is a lifetime.
    squote:    bool,
    /// `'x'` is a char literal — only when a closing quote really follows.
    char_lit:  bool,
    /// `` `…` `` template literal, newlines allowed.
    backtick:  bool,
    /// `"""…"""` / `'''…'''`.
    triple:    bool,
    /// `r"…"`, `r#"…"#`, `b"…"`, `br#"…"#`.
    raw_str:   bool,
    /// `$VAR`, `${VAR}`, `$1`, `$?`.
    dollar:    bool,
    /// `@name` decorator.
    decorator: bool,
    /// `#directive`, and `<stdio.h>` after `#include`.
    preproc:   bool,
    /// Literal prefixes that belong to the string they precede:
    /// `f"…"`, `b'…'`, `r#"…"#`.
    str_prefix: &'static [&'static str],
    /// `$(…)` is a command substitution — inside a double-quoted shell
    /// string it opens a fresh quoting context, so the string does not
    /// end at the next quote it contains.
    subst:     bool,
}

const NONE: &[&str] = &[];

const RUST: Syntax = Syntax {
    decl: &["fn","let","mut","const","static","pub","use","mod","crate","struct",
            "enum","impl","trait","type","where","self","Self","as","ref","move",
            "dyn","unsafe","extern","super","union","macro_rules"],
    ctrl: &["if","else","match","for","while","loop","return","break","continue",
            "in","async","await","yield","try"],
    consts: &["true","false"],
    types: &["u8","u16","u32","u64","u128","usize","i8","i16","i32","i64",
             "i128","isize","f32","f64","bool","char","str"],
    builtins: NONE,
    line: "//", block: Some(("/*","*/")), nest_block: true,
    squote: false, char_lit: true, backtick: false, triple: false,
    raw_str: true, dollar: false, decorator: false, preproc: false,
    str_prefix: &["r","b","br"], subst: false,
};

const CLANG: Syntax = Syntax {
    decl: &["auto","char","const","double","enum","extern","float","inline","int",
            "long","register","restrict","short","signed","static","struct",
            "typedef","union","unsigned","void","volatile","_Bool","size_t",
            "ssize_t","uint8_t","uint16_t","uint32_t","uint64_t",
            "int8_t","int16_t","int32_t","int64_t"],
    ctrl: &["break","case","continue","default","do","else","for","goto","if",
            "return","switch","while","sizeof",
            // preprocessor directives, reached through the '#' branch
            "include","define","undef","ifdef","ifndef","endif","pragma","elif","error"],
    consts: &["NULL","true","false"],
    types: NONE,
    builtins: NONE,
    line: "//", block: Some(("/*","*/")), nest_block: false,
    squote: false, char_lit: true, backtick: false, triple: false,
    raw_str: false, dollar: false, decorator: false, preproc: true,
    str_prefix: NONE, subst: false,
};

const JS: Syntax = Syntax {
    decl: &["var","let","const","function","class","extends","static","get","set",
            "interface","implements","declare","namespace","abstract","readonly",
            "public","private","protected","enum"],
    ctrl: &["return","if","else","for","while","do","switch","case","default",
            "break","continue","new","delete","typeof","instanceof","in","of",
            "throw","try","catch","finally","yield","await","async","import",
            "export","from","as"],
    consts: &["true","false","null","undefined","NaN","Infinity","this","super"],
    types: &["string","number","boolean","any","void","never","unknown","symbol",
             "bigint","object"],
    builtins: NONE,
    line: "//", block: Some(("/*","*/")), nest_block: false,
    squote: true, char_lit: false, backtick: true, triple: false,
    raw_str: false, dollar: false, decorator: true, preproc: false,
    str_prefix: NONE, subst: false,
};

const PY: Syntax = Syntax {
    decl: &["def","class","lambda","global","nonlocal","import","from","as","with"],
    ctrl: &["return","if","elif","else","for","while","break","continue","pass",
            "try","except","finally","raise","yield","await","async","del",
            "assert","in","is","not","and","or"],
    consts: &["None","True","False","self","cls","__name__","__main__"],
    types: &["int","float","complex","str","bytes","bytearray","bool","list",
             "dict","set","frozenset","tuple","object","type"],
    builtins: NONE,
    line: "#", block: None, nest_block: false,
    squote: true, char_lit: false, backtick: false, triple: true,
    raw_str: false, dollar: false, decorator: true, preproc: false,
    str_prefix: &["f","F","r","R","b","B","u","U","rb","rf","br","fr"],
    subst: false,
};

const SH: Syntax = Syntax {
    decl: &["function","local","readonly","declare","export","typeset","alias"],
    ctrl: &["if","then","else","elif","fi","for","while","until","do","done",
            "case","esac","in","select","return","exit","break","continue",
            "source","trap"],
    consts: &["true","false"],
    types: NONE,
    builtins: &["echo","printf","read","cd","set","unset","shift","test","eval",
                "exec","pwd","kill","wait","umask","getopts"],
    line: "#", block: None, nest_block: false,
    squote: true, char_lit: false, backtick: false, triple: false,
    raw_str: false, dollar: true, decorator: false, preproc: false,
    str_prefix: NONE, subst: true,
};

fn syntax_for(lang: Lang) -> &'static Syntax {
    match lang {
        Lang::Rust   => &RUST,
        Lang::C      => &CLANG,
        Lang::Js     => &JS,
        Lang::Python => &PY,
        Lang::Shell | Lang::Make => &SH,
        // Handled by their own scanners — never reached.
        Lang::Json | Lang::Markup | Lang::Toml | Lang::Yaml | Lang::Css | Lang::Diff => &RUST,
    }
}

/// Tokenise the whole document into colour spans for the given language.
fn code_spans(text: &str, lang: Lang) -> Vec<Span> {
    match lang {
        Lang::Markup => markup_spans(text),
        Lang::Json   => json_spans(text),
        Lang::Toml   => conf_spans(text, false),
        Lang::Yaml   => conf_spans(text, true),
        Lang::Css    => css_spans(text),
        Lang::Diff   => diff_spans(text),
        other        => clike_spans(text, syntax_for(other)),
    }
}

// ── Scanning primitives ───────────────────────────────────────────────

fn char_len(s: &str, i: usize) -> usize {
    s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
}

fn is_ident(b: u8) -> bool { b.is_ascii_alphanumeric() || b == b'_' }
fn is_ident_start(b: u8) -> bool { b.is_ascii_alphabetic() || b == b'_' }

fn line_end(b: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < b.len() && b[i] != b'\n' { i += 1; }
    i
}

/// End offset of a block comment starting at `start`. Unterminated runs
/// to EOF — that is what an editor should show while you are still
/// typing the closing marker.
fn scan_block(text: &str, start: usize, open: &str, close: &str, nest: bool) -> usize {
    let mut i = start + open.len();
    let mut depth = 1usize;
    while i < text.len() {
        if nest && text[i..].starts_with(open) { depth += 1; i += open.len(); continue; }
        if text[i..].starts_with(close) {
            i += close.len();
            depth -= 1;
            if depth == 0 { return i; }
            continue;
        }
        i += char_len(text, i);
    }
    text.len()
}

/// End offset of a string starting at `start`. A plain quoted string
/// stops at the newline if it is unterminated, so one stray `"` cannot
/// paint the rest of the file.
fn scan_string(text: &str, start: usize, quote: u8, triple: bool, subst: bool) -> usize {
    let b = text.as_bytes();
    if triple && b.len() >= start + 3 && b[start + 1] == quote && b[start + 2] == quote {
        let mut i = start + 3;
        while i + 2 < b.len() {
            if b[i] == b'\\' { i += 2; continue; }
            if b[i] == quote && b[i + 1] == quote && b[i + 2] == quote { return i + 3; }
            i += 1;
        }
        return b.len();
    }
    let mut i = start + 1;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 1;
            if i < b.len() { i += char_len(text, i); }
            continue;
        }
        // `"$(cd "$dir" && pwd)"` is one string — the quotes inside the
        // substitution belong to it, not to us. Skip to the balanced `)`.
        if subst && b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'(' {
            let mut depth = 1usize;
            let mut k = i + 2;
            while k < b.len() && depth > 0 {
                match b[k] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                k += 1;
            }
            i = k;
            continue;
        }
        if b[i] == quote { return i + 1; }
        if b[i] == b'\n' && quote != b'`' { return i; }
        i += char_len(text, i);
    }
    b.len()
}

/// Rust `r"…"` / `r#"…"#`. `at` points just past the `r`/`b`/`br` prefix.
/// Returns `None` if no raw string actually follows.
fn scan_raw_string(text: &str, at: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = at;
    let mut hashes = 0usize;
    while i < b.len() && b[i] == b'#' { hashes += 1; i += 1; }
    if i >= b.len() || b[i] != b'"' { return None; }
    i += 1;
    while i < b.len() {
        if b[i] == b'"' {
            let mut k = i + 1;
            let mut n = 0usize;
            while k < b.len() && n < hashes && b[k] == b'#' { k += 1; n += 1; }
            if n == hashes { return Some(k); }
        }
        i += char_len(text, i);
    }
    Some(b.len())
}

/// `'x'` / `'\n'` / `'\u{1F600}'` — but not `'a`, which is a lifetime.
/// `None` unless a closing quote really follows.
fn scan_char_lit(text: &str, start: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = start + 1;
    if i >= b.len() { return None; }
    if b[i] == b'\\' {
        i += 1;
        if i >= b.len() { return None; }
        if b[i] == b'u' {
            i += 1;
            if i < b.len() && b[i] == b'{' {
                i = text[i..].find('}').map(|p| i + p + 1)?;
            }
        } else {
            i += char_len(text, i);
        }
    } else {
        i += char_len(text, i);
    }
    if i < b.len() && b[i] == b'\'' { Some(i + 1) } else { None }
}

fn scan_number(text: &str, start: usize) -> usize {
    let b = text.as_bytes();
    let mut i = start;
    // 0x… / 0b… / 0o… — one run, digits and separators alike.
    if b[i] == b'0' && i + 1 < b.len() && matches!(b[i + 1] | 0x20, b'x' | b'b' | b'o') {
        i += 2;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
        return i;
    }
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') { i += 1; }
    // A fraction, but `1..2` is a range — the digit after the dot decides.
    if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
        i += 1;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') { i += 1; }
    }
    if i < b.len() && (b[i] | 0x20) == b'e' {
        let mut k = i + 1;
        if k < b.len() && (b[k] == b'+' || b[k] == b'-') { k += 1; }
        if k < b.len() && b[k].is_ascii_digit() {
            i = k;
            while i < b.len() && b[i].is_ascii_digit() { i += 1; }
        }
    }
    // Type suffix: 1u32, 3.14f64, 10UL.
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
    i
}

/// Which colour an identifier gets. There is no symbol table here, so
/// after the keyword lists it is shape that carries the signal: a name
/// with a `(` behind it is a call, `HttpRequest` is a type, `MAX_LEN` is
/// a constant, and `count` is none of the three and stays default.
fn classify(w: &str, b: &[u8], after: usize, sx: &Syntax) -> Option<Token> {
    if sx.ctrl.contains(&w)     { return Some(Token::CodeControl); }
    if sx.decl.contains(&w)     { return Some(Token::CodeKeyword); }
    if sx.consts.contains(&w)   { return Some(Token::CodeConstant); }
    if sx.types.contains(&w)    { return Some(Token::CodeType); }
    if sx.builtins.contains(&w) { return Some(Token::CodeFunction); }
    // Shape is checked before the call site: `new Error(…)` and
    // `Downloader(url)` are a type being used, not a free function.
    let first = w.as_bytes()[0];
    if first.is_ascii_uppercase() {
        if w.bytes().any(|c| c.is_ascii_lowercase()) { return Some(Token::CodeType); }
        if w.len() > 1 { return Some(Token::CodeConstant); }
    }
    let mut k = after;
    while k < b.len() && (b[k] == b' ' || b[k] == b'\t') { k += 1; }
    if k < b.len() && b[k] == b'(' { return Some(Token::CodeFunction); }
    None
}

// ── The C-like scanner ────────────────────────────────────────────────

fn clike_spans(text: &str, sx: &Syntax) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    let mut i = 0usize;
    // Set while inside a `#include` line, so `<stdio.h>` reads as a path
    // and not as two comparisons.
    let mut include_line = false;

    while i < b.len() {
        let c = b[i];
        if c == b'\n' { include_line = false; i += 1; continue; }
        if c == b' ' || c == b'\t' || c == b'\r' { i += 1; continue; }

        if !sx.line.is_empty() && text[i..].starts_with(sx.line) {
            let end = line_end(b, i);
            push_span(&mut out, i, end - i, Token::CodeComment);
            i = end;
            continue;
        }
        if let Some((open, close)) = sx.block {
            if text[i..].starts_with(open) {
                let end = scan_block(text, i, open, close, sx.nest_block);
                push_span(&mut out, i, end - i, Token::CodeComment);
                i = end;
                continue;
            }
        }

        if sx.preproc && c == b'#' {
            let mut j = i + 1;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') { j += 1; }
            let ns = j;
            while j < b.len() && is_ident(b[j]) { j += 1; }
            push_span(&mut out, i, 1, Token::CodeControl);
            if j > ns {
                push_span(&mut out, ns, j - ns, Token::CodeControl);
                if &text[ns..j] == "include" { include_line = true; }
            }
            i = j;
            continue;
        }
        if include_line && c == b'<' {
            let end = text[i..].find('>').map(|p| i + p + 1).unwrap_or_else(|| line_end(b, i));
            push_span(&mut out, i, end - i, Token::CodeString);
            i = end;
            continue;
        }
        if sx.decorator && c == b'@' && i + 1 < b.len() && is_ident_start(b[i + 1]) {
            let mut j = i + 1;
            while j < b.len() && (is_ident(b[j]) || b[j] == b'.') { j += 1; }
            push_span(&mut out, i, j - i, Token::CodeFunction);
            i = j;
            continue;
        }
        if sx.dollar && c == b'$' {
            // `$(…)` is a command substitution. Stepping over just the
            // `$(` lets the command inside keep its own colours.
            if i + 1 < b.len() && b[i + 1] == b'(' { i += 2; continue; }
            let mut j = i + 1;
            if j < b.len() && b[j] == b'{' {
                j = text[i..].find('}').map(|p| i + p + 1).unwrap_or(b.len());
            } else {
                while j < b.len() && is_ident(b[j]) { j += 1; }
                // $? $@ $# $* — one punctuation byte is still a variable.
                if j == i + 1 && j < b.len() { j += 1; }
            }
            push_span(&mut out, i, j - i, Token::CodeVariable);
            i = j;
            continue;
        }

        if c == b'"' || (sx.squote && c == b'\'') || (sx.backtick && c == b'`') {
            let end = scan_string(text, i, c, sx.triple, sx.subst);
            push_span(&mut out, i, end - i, Token::CodeString);
            i = end;
            continue;
        }
        if sx.char_lit && c == b'\'' {
            if let Some(end) = scan_char_lit(text, i) {
                push_span(&mut out, i, end - i, Token::CodeString);
                i = end;
                continue;
            }
            // A lifetime. Step over the quote and read the name normally.
            i += 1;
            continue;
        }

        if c.is_ascii_digit() {
            let end = scan_number(text, i);
            push_span(&mut out, i, end - i, Token::CodeNumber);
            i = end;
            continue;
        }

        if is_ident_start(c) {
            let start = i;
            let mut j = i;
            while j < b.len() && is_ident(b[j]) { j += 1; }
            let w = &text[start..j];
            // `f"…"`, `b'…'`, `r#"…"#` — the prefix is part of the literal.
            if sx.str_prefix.contains(&w) && j < b.len() {
                if sx.raw_str && (b[j] == b'"' || b[j] == b'#') {
                    if let Some(end) = scan_raw_string(text, j) {
                        push_span(&mut out, start, end - start, Token::CodeString);
                        i = end;
                        continue;
                    }
                }
                if b[j] == b'"' || (sx.squote && b[j] == b'\'') {
                    let end = scan_string(text, j, b[j], sx.triple, sx.subst);
                    push_span(&mut out, start, end - start, Token::CodeString);
                    i = end;
                    continue;
                }
            }
            if let Some(t) = classify(w, b, j, sx) {
                push_span(&mut out, start, j - start, t);
            }
            i = j;
            continue;
        }

        i += char_len(text, i);
    }
    out
}

// ── JSON ──────────────────────────────────────────────────────────────

/// Keys and values are both quoted strings, so the next non-space byte
/// after a string is what tells them apart.
fn json_spans(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            let end = scan_string(text, i, b'"', false, false);
            let mut k = end;
            while k < b.len() && (b[k] == b' ' || b[k] == b'\t' || b[k] == b'\n' || b[k] == b'\r') {
                k += 1;
            }
            let tok = if k < b.len() && b[k] == b':' { Token::CodeVariable } else { Token::CodeString };
            push_span(&mut out, i, end - i, tok);
            i = end;
            continue;
        }
        if c.is_ascii_digit() || (c == b'-' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let start = i;
            if c == b'-' { i += 1; }
            let end = scan_number(text, i);
            push_span(&mut out, start, end - start, Token::CodeNumber);
            i = end;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let start = i;
            while i < b.len() && b[i].is_ascii_alphabetic() { i += 1; }
            if matches!(&text[start..i], "true" | "false" | "null") {
                push_span(&mut out, start, i - start, Token::CodeConstant);
            }
            continue;
        }
        i += char_len(text, i);
    }
    out
}

// ── TOML / YAML ───────────────────────────────────────────────────────

/// Settings files: `# comment`, `[section]` (TOML) as a type, the key in
/// front of `=` or `:` as a variable, strings, numbers and the literal
/// constants.
fn conf_spans(text: &str, yaml: bool) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    let mut ls = 0usize;
    while ls < b.len() {
        let le = line_end(b, ls);
        let mut i = ls;
        while i < le && (b[i] == b' ' || b[i] == b'\t') { i += 1; }
        if yaml && i + 1 < le && b[i] == b'-' && b[i + 1] == b' ' {
            push_span(&mut out, i, 1, Token::CodeControl);
            i += 2;
        }
        if i < le && b[i] == b'[' && !yaml {
            push_span(&mut out, i, le - i, Token::CodeType);
            ls = le + 1;
            continue;
        }
        // The key: up to the first `=` (TOML) or `: ` (YAML) outside quotes.
        let sep = if yaml { b':' } else { b'=' };
        let mut k = i;
        while k < le && b[k] != sep && b[k] != b'#' && b[k] != b'"' && b[k] != b'\'' { k += 1; }
        if k < le && b[k] == sep && k > i && (!yaml || k + 1 == le || b[k + 1] == b' ') {
            push_span(&mut out, i, k - i, Token::CodeVariable);
            i = k + 1;
        }
        while i < le {
            let c = b[i];
            if c == b'#' && (i == ls || b[i - 1] == b' ' || b[i - 1] == b'\t') {
                push_span(&mut out, i, le - i, Token::CodeComment);
                break;
            }
            if c == b'"' || c == b'\'' {
                let end = scan_string(text, i, c, !yaml, false).min(le.max(i + 1));
                push_span(&mut out, i, end - i, Token::CodeString);
                i = end;
                continue;
            }
            let word_start = i == ls || !is_ident(b[i - 1]);
            if word_start && (c.is_ascii_digit() || (c == b'-' && i + 1 < le && b[i + 1].is_ascii_digit())) {
                let s0 = i;
                if c == b'-' { i += 1; }
                let end = scan_number(text, i).min(le);
                push_span(&mut out, s0, end - s0, Token::CodeNumber);
                i = end.max(i + 1);
                continue;
            }
            if word_start && is_ident_start(c) {
                let s0 = i;
                while i < le && is_ident(b[i]) { i += 1; }
                if matches!(&text[s0..i], "true" | "false" | "null" | "yes" | "no" | "on" | "off") {
                    push_span(&mut out, s0, i - s0, Token::CodeConstant);
                }
                continue;
            }
            i += char_len(text, i);
        }
        ls = le + 1;
    }
    out
}

// ── CSS ───────────────────────────────────────────────────────────────

/// Selectors as types, `@rules` as keywords, properties as variables,
/// strings, numbers with their unit, `#rrggbb` as constants, comments.
fn css_spans(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if b[i..].starts_with(b"/*") {
            let end = scan_block(text, i, "/*", "*/", false);
            push_span(&mut out, i, end - i, Token::CodeComment);
            i = end;
            continue;
        }
        match c {
            b'{' => { depth += 1; i += 1; continue; }
            b'}' => { depth = depth.saturating_sub(1); i += 1; continue; }
            b'"' | b'\'' => {
                let end = scan_string(text, i, c, false, false);
                push_span(&mut out, i, end - i, Token::CodeString);
                i = end;
                continue;
            }
            b'@' => {
                let s0 = i;
                i += 1;
                while i < b.len() && (is_ident(b[i]) || b[i] == b'-') { i += 1; }
                push_span(&mut out, s0, i - s0, Token::CodeKeyword);
                continue;
            }
            _ => {}
        }
        if depth == 0 {
            // A selector runs to the `{`.
            if !c.is_ascii_whitespace() && c != b',' {
                let s0 = i;
                while i < b.len() && !matches!(b[i], b'{' | b',' | b'\n') && !b[i..].starts_with(b"/*") { i += 1; }
                push_span(&mut out, s0, i - s0, Token::CodeType);
                continue;
            }
        } else {
            if c == b'#' {
                let s0 = i;
                i += 1;
                while i < b.len() && b[i].is_ascii_hexdigit() { i += 1; }
                push_span(&mut out, s0, i - s0, Token::CodeConstant);
                continue;
            }
            if c.is_ascii_digit() || (c == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
                let s0 = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') { i += 1; }
                while i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'%') { i += 1; }
                push_span(&mut out, s0, i - s0, Token::CodeNumber);
                continue;
            }
            if is_ident_start(c) || c == b'-' {
                let s0 = i;
                while i < b.len() && (is_ident(b[i]) || b[i] == b'-') { i += 1; }
                let mut k = i;
                while k < b.len() && b[k] == b' ' { k += 1; }
                if k < b.len() && b[k] == b':' {
                    push_span(&mut out, s0, i - s0, Token::CodeVariable);
                } else if i < b.len() && b[i] == b'(' {
                    push_span(&mut out, s0, i - s0, Token::CodeFunction);
                }
                continue;
            }
        }
        i += char_len(text, i);
    }
    out
}

// ── diff / patch ──────────────────────────────────────────────────────

/// Added lines green, removed red, hunk headers as keywords, file
/// headers as comments.
fn diff_spans(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    let mut ls = 0usize;
    while ls < b.len() {
        let le = line_end(b, ls);
        let line = &text[ls..le];
        let tok = if line.starts_with("+++") || line.starts_with("---")
            || line.starts_with("diff ") || line.starts_with("index ") {
            Some(Token::CodeComment)
        } else if line.starts_with("@@") {
            Some(Token::CodeKeyword)
        } else if line.starts_with('+') {
            Some(Token::Success)
        } else if line.starts_with('-') {
            Some(Token::Danger)
        } else {
            None
        };
        if let Some(t) = tok { push_span(&mut out, ls, le - ls, t); }
        ls = le + 1;
    }
    out
}

// ── Markdown source ───────────────────────────────────────────────────

/// Headings, list markers, quotes, fenced and inline code, link targets,
/// emphasis markers: enough colour to find the structure while writing.
fn markdown_spans(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    let mut fence = false;
    let mut ls = 0usize;
    while ls < b.len() {
        let le = line_end(b, ls);
        let line = &text[ls..le];
        let trimmed = line.trim_start();
        let ind = line.len() - trimmed.len();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
            push_span(&mut out, ls, le - ls, Token::CodeString);
            ls = le + 1;
            continue;
        }
        if fence {
            push_span(&mut out, ls, le - ls, Token::CodeString);
            ls = le + 1;
            continue;
        }
        if trimmed.starts_with('#') {
            push_span(&mut out, ls, le - ls, Token::CodeKeyword);
            ls = le + 1;
            continue;
        }
        if trimmed.starts_with('>') {
            push_span(&mut out, ls, le - ls, Token::CodeComment);
            ls = le + 1;
            continue;
        }
        let tb = trimmed.as_bytes();
        if tb.len() >= 2 && matches!(tb[0], b'-' | b'*' | b'+') && tb[1] == b' ' {
            push_span(&mut out, ls + ind, 1, Token::CodeControl);
        } else {
            let digits = tb.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && tb.get(digits) == Some(&b'.') {
                push_span(&mut out, ls + ind, digits + 1, Token::CodeControl);
            }
        }
        let mut i = ls + ind;
        while i < le {
            match b[i] {
                b'`' => {
                    let end = text[i + 1..le].find('`').map(|k| i + 1 + k + 1).unwrap_or(le);
                    push_span(&mut out, i, end - i, Token::CodeString);
                    i = end;
                    continue;
                }
                b']' if i + 1 < le && b[i + 1] == b'(' => {
                    let end = text[i + 1..le].find(')').map(|k| i + 1 + k + 1).unwrap_or(le);
                    push_span(&mut out, i + 1, end - i - 1, Token::CodeFunction);
                    i = end;
                    continue;
                }
                b'*' | b'_' => {
                    let s0 = i;
                    while i < le && b[i] == b[s0] { i += 1; }
                    push_span(&mut out, s0, i - s0, Token::CodeControl);
                    continue;
                }
                _ => {}
            }
            i += char_len(text, i);
        }
        ls = le + 1;
    }
    out
}

// ── HTML / XML ────────────────────────────────────────────────────────

/// Tag names → type colour, attribute names → variable, attribute values
/// → string, `<!-- -->` → comment, `&entity;` → constant. The angle
/// brackets themselves stay default: colouring the punctuation too turns
/// a page of markup into one solid block.
fn markup_spans(text: &str) -> Vec<Span> {
    let b = text.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        if text[i..].starts_with("<!--") {
            let end = text[i..].find("-->").map(|p| i + p + 3).unwrap_or(text.len());
            push_span(&mut out, i, end - i, Token::CodeComment);
            i = end;
            continue;
        }
        if b[i] == b'&' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'#') { j += 1; }
            if j > i + 1 && j < b.len() && b[j] == b';' {
                push_span(&mut out, i, j + 1 - i, Token::CodeConstant);
                i = j + 1;
            } else {
                i += 1;
            }
            continue;
        }
        if b[i] != b'<' { i += char_len(text, i); continue; }

        i += 1;
        if i < b.len() && matches!(b[i], b'/' | b'!' | b'?') { i += 1; }
        let ns = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'-' | b'_' | b':')) {
            i += 1;
        }
        push_span(&mut out, ns, i - ns, Token::CodeType);

        while i < b.len() && b[i] != b'>' {
            let c = b[i];
            if c == b'"' || c == b'\'' {
                let end = scan_string(text, i, c, false, false);
                push_span(&mut out, i, end - i, Token::CodeString);
                i = end;
                continue;
            }
            if is_ident_start(c) {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'-' | b'_' | b':')) {
                    i += 1;
                }
                push_span(&mut out, start, i - start, Token::CodeVariable);
                continue;
            }
            i += char_len(text, i);
        }
        if i < b.len() { i += 1; }
    }
    out
}

// ── Events ────────────────────────────────────────────────────────────

enum Outcome { Idle, Rerender, Exit }

fn handle(sp: &mut Spell, ev: Event, payload: &str) -> Outcome {
    // `Event::Mods` describes only the event right behind it.
    let shift = core::mem::take(&mut sp.shift);
    let ctrl = core::mem::take(&mut sp.ctrl);
    match ev {
        Event::Mods { shift, ctrl } => { sp.shift = shift; sp.ctrl = ctrl; Outcome::Idle }
        Event::Key(KeyCode::PageUp) if ctrl => { sp.cycle_tab(-1); Outcome::Rerender }
        Event::Key(KeyCode::PageDown) if ctrl => { sp.cycle_tab(1); Outcome::Rerender }
        Event::Focus(true) => if sp.check_disk() { Outcome::Rerender } else { Outcome::Idle },
        // Esc backs out of whatever is open. It does not quit: in an
        // editor Esc is the cancel key, and a stray press should never
        // end the session. Closing is Mod+Q, the window's ×, or
        // File → Close.
        // Enter in the unsaved-changes card saves, as its filled button.
        Event::Key(KeyCode::Enter) if sp.confirm_close.is_some() => handle_action(sp, ACT_CLOSE_SAVE),
        Event::Key(KeyCode::Escape) => {
            if sp.confirm_close.is_some() { sp.cancel_quit(); Outcome::Rerender }
            else if sp.open_menu.is_some() { sp.open_menu = None; Outcome::Rerender }
            else if sp.bar.is_some() { sp.close_bar(); Outcome::Rerender }
            else { Outcome::Idle }
        }
        // F3 next hit, Shift+F3 the previous one.
        Event::Key(KeyCode::F(3)) => {
            if matches!(sp.bar, Some(Bar::Find(_))) {
                sp.step_hit(if shift { -1 } else { 1 });
            } else {
                sp.open_find(false);
            }
            Outcome::Rerender
        }
        // The window manager is asking whether it may close us.
        Event::CloseRequest => {
            if sp.begin_quit() { Outcome::Exit } else { Outcome::Rerender }
        }
        // Every field carries a NodeId, so the change comes with the node as
        // well; the plain `InputChange` beside it is ignored.
        Event::InputChangeAt { node, .. } => match node {
            NODE_FIND => {
                if let Some(Bar::Find(f)) = &mut sp.bar { f.query = payload.to_string(); }
                sp.seek_from_caret();
                Outcome::Rerender
            }
            NODE_REPLACE => {
                if let Some(Bar::Find(FindBar { replace: Some(r), .. })) = &mut sp.bar {
                    *r = payload.to_string();
                }
                Outcome::Idle
            }
            NODE_GOTO => {
                if let Some(Bar::Goto(t)) = &mut sp.bar { *t = payload.to_string(); }
                Outcome::Idle
            }
            _ => match sp.doc_by_node(node) {
                Some(d) => {
                    d.history.changed(&d.text, payload);
                    d.set_text(payload);
                    d.touch();
                    Outcome::Rerender
                }
                None => Outcome::Idle,
            },
        },
        Event::Caret { node, cursor, anchor } => {
            let active = sp.cur().node() == node;
            match sp.doc_by_node(node) {
                Some(d) => {
                    d.history.caret_moved(anchor, cursor);
                    if active { Outcome::Rerender } else { Outcome::Idle }
                }
                None => Outcome::Idle,
            }
        }
        // The file dialog came back. `payload` is the chosen path; empty
        // means the user cancelled.
        Event::Picked { tag, .. } => {
            if payload.is_empty() { return Outcome::Idle; }
            if tag == TAG_OPEN {
                sp.open_path(payload);
            } else if tag >= TAG_SAVE_CLOSE_BASE {
                let doc = (tag - TAG_SAVE_CLOSE_BASE) as usize;
                sp.save_as(doc, payload);
                sp.close_tab(doc);
                if sp.advance_quit() { return Outcome::Exit; }
            } else if tag >= TAG_SAVE_BASE {
                sp.save_as((tag - TAG_SAVE_BASE) as usize, payload);
            }
            Outcome::Rerender
        }
        // Another file was opened while we're running (loft association,
        // singleton routing) → open it as a tab. `payload` = the path.
        Event::Open(_) => {
            sp.open_path(payload);
            Outcome::Rerender
        }
        // Ctrl chords. The editor keeps Ctrl+A/C/X/V for text, so those
        // never arrive here.
        Event::Chord { letter, shift, .. } => match letter {
            b's' if shift => { sp.ask_save_target(sp.active, false); Outcome::Rerender }
            b's' => { sp.save_or_name(); Outcome::Rerender }
            b'o' => {
                let start = sp.cur().path.as_deref().map(dirname).unwrap_or("").to_string();
                pick(PICK_OPEN, &start, "", TAG_OPEN);
                Outcome::Rerender
            }
            // New tab: Ctrl+N as in editors, Ctrl+T as in browsers.
            b'n' | b't' => { sp.new_doc(); Outcome::Rerender }
            b'w' => { sp.request_close(sp.active); Outcome::Rerender }
            // Zoom. '=' comes along because on a US layout the '+' key is
            // Shift+'=', and people press Ctrl+'=' without the Shift.
            b'+' | b'=' => { sp.zoom(Some(1)); Outcome::Rerender }
            b'-' => { sp.zoom(Some(-1)); Outcome::Rerender }
            b'0' => { sp.zoom(None); Outcome::Rerender }
            b'z' if shift => { sp.redo(); Outcome::Rerender }
            b'z' => { sp.undo(); Outcome::Rerender }
            b'y' => { sp.redo(); Outcome::Rerender }
            b'f' => { sp.open_find(false); Outcome::Rerender }
            b'h' => { sp.open_find(true); Outcome::Rerender }
            b'g' => { sp.open_goto(); Outcome::Rerender }
            // `/` on US layouts, `#` where it sits on its own key (German).
            b'/' | b'#' => { sp.toggle_comment(); Outcome::Rerender }
            b'k' if shift => { sp.delete_lines(); Outcome::Rerender }
            _ => Outcome::Idle,
        },
        // Ctrl+wheel. One px per notch: small enough to land on the size
        // you want, and the compositor clamps the ends anyway.
        Event::Zoom { delta } => {
            if sp.zoom(Some(delta)) { Outcome::Rerender } else { Outcome::Idle }
        }
        Event::Action(ActionId(id)) => handle_action(sp, id),
        _ => Outcome::Idle,
    }
}

fn handle_action(sp: &mut Spell, id: u32) -> Outcome {
    match id {
        ACT_MENU_FILE => { sp.open_menu = toggle(sp.open_menu, OpenMenu::File); Outcome::Rerender }
        ACT_MENU_VIEW => { sp.open_menu = toggle(sp.open_menu, OpenMenu::View); Outcome::Rerender }
        ACT_MENU_HELP => { sp.open_menu = toggle(sp.open_menu, OpenMenu::Help); Outcome::Rerender }
        ACT_MENU_EDIT => { sp.open_menu = toggle(sp.open_menu, OpenMenu::Edit); Outcome::Rerender }
        ACT_UNDO    => { sp.open_menu = None; sp.undo(); Outcome::Rerender }
        ACT_REDO    => { sp.open_menu = None; sp.redo(); Outcome::Rerender }
        ACT_FIND    => { sp.open_menu = None; sp.open_find(false); Outcome::Rerender }
        ACT_REPLACE => { sp.open_menu = None; sp.open_find(true); Outcome::Rerender }
        ACT_GOTO    => { sp.open_menu = None; sp.open_goto(); Outcome::Rerender }
        ACT_FIND_NEXT => { sp.step_hit(1); Outcome::Rerender }
        ACT_FIND_PREV => { sp.step_hit(-1); Outcome::Rerender }
        ACT_FIND_CASE => {
            if let Some(Bar::Find(f)) = &mut sp.bar { f.case = !f.case; }
            sp.seek_from_caret();
            Outcome::Rerender
        }
        ACT_FIND_CLOSE  => { sp.close_bar(); Outcome::Rerender }
        ACT_REPLACE_ONE => { sp.replace_one(); Outcome::Rerender }
        ACT_REPLACE_ALL => { sp.replace_all(); Outcome::Rerender }
        ACT_GOTO_SUBMIT => { sp.goto_submit(); Outcome::Rerender }
        ACT_COMMENT     => { sp.open_menu = None; sp.toggle_comment(); Outcome::Rerender }
        ACT_DELETE_LINE => { sp.open_menu = None; sp.delete_lines(); Outcome::Rerender }
        ACT_DISK_RELOAD => { sp.disk_reload(); Outcome::Rerender }
        ACT_DISK_KEEP   => { sp.disk_keep(); Outcome::Rerender }
        ACT_MENU_DISMISS => {
            if sp.open_menu.is_some() { sp.open_menu = None; Outcome::Rerender }
            else { Outcome::Idle }
        }
        ACT_FILE_NEW => {
            sp.open_menu = None;
            sp.new_doc();
            Outcome::Rerender
        }
        ACT_FILE_OPEN => {
            sp.open_menu = None;
            // Start where the current file lives; the picker falls back to
            // the user's home for an unnamed buffer.
            let start = sp.cur().path.as_deref().map(dirname).unwrap_or("").to_string();
            pick(PICK_OPEN, &start, "", TAG_OPEN);
            Outcome::Rerender
        }
        ACT_FILE_SAVE => { sp.open_menu = None; sp.save_or_name(); Outcome::Rerender }
        ACT_FILE_SAVE_AS => {
            sp.ask_save_target(sp.active, false);
            Outcome::Rerender
        }
        ACT_FILE_CLOSE => { sp.open_menu = None; sp.request_close(sp.active); Outcome::Rerender }
        // Unsaved-changes dialog buttons.
        ACT_CLOSE_DISCARD => {
            if let Some(i) = sp.confirm_close.take() {
                sp.close_tab(i);
                if sp.advance_quit() { return Outcome::Exit; }
            }
            Outcome::Rerender
        }
        // Cancel abandons the whole quit, not just this one file — the
        // user said "no" to closing, and losing the other tabs' prompts
        // would be a surprise.
        ACT_CLOSE_CANCEL => { sp.cancel_quit(); Outcome::Rerender }
        ACT_CLOSE_SAVE => {
            if let Some(i) = sp.confirm_close.take() {
                sp.active = i;
                if let Some(p) = sp.cur().path.clone() {
                    // A save that did not happen keeps the tab open and ends
                    // the quit: its strip or dialog now asks.
                    if sp.write_to(&p) {
                        sp.close_tab(i);
                        if sp.advance_quit() { return Outcome::Exit; }
                    } else {
                        sp.cancel_quit();
                    }
                } else {
                    // No filename yet → ask where to put it. The chain
                    // resumes when the picker replies.
                    sp.ask_save_target(i, true);
                }
            }
            Outcome::Rerender
        }
        ACT_VIEW_EDIT => { sp.open_menu = None; sp.cur_mut().mode = Mode::Edit; Outcome::Rerender }
        ACT_VIEW_PREVIEW => {
            sp.open_menu = None;
            // Only markdown has a preview. Leaving the mode on Edit for
            // everything else keeps the menu's radio dot honest instead of
            // marking a view that never renders.
            if matches!(sp.cur().kind(), Kind::Markdown) {
                sp.cur_mut().mode = Mode::Preview;
            }
            Outcome::Rerender
        }
        ACT_FILE_SETTINGS => {
            sp.open_menu = None;
            // Write it first when it doesn't exist yet, so "Settings"
            // always opens something — an empty editor with no file would
            // just raise the question of where to put it.
            if read_file(CONFIG_PATH).is_none() {
                let c = sp.config();
                write_config(&c);
                sp.cfg_saved = c;
            }
            sp.open_path(CONFIG_PATH);
            Outcome::Rerender
        }
        ACT_ZOOM_IN  => { sp.open_menu = None; sp.zoom(Some(1));  Outcome::Rerender }
        ACT_ZOOM_OUT => { sp.open_menu = None; sp.zoom(Some(-1)); Outcome::Rerender }
        ACT_ZOOM_RESET => { sp.open_menu = None; sp.zoom(None);   Outcome::Rerender }
        ACT_HELP_ABOUT => {
            log("[spell] Spell 0.1 — nopeekOS text editor");
            sp.open_menu = None;
            Outcome::Rerender
        }
        _ => {
            // Ranges, highest base first.
            if id >= ACT_TAB_CLOSE_BASE {
                sp.request_close((id - ACT_TAB_CLOSE_BASE) as usize);
                return Outcome::Rerender;
            }
            if id >= ACT_TAB_BASE {
                let i = (id - ACT_TAB_BASE) as usize;
                if i < sp.docs.len() { sp.active = i; }
                return Outcome::Rerender;
            }
            if id >= ACT_RECENT_BASE && id < ACT_RECENT_BASE + MAX_RECENT as u32 {
                sp.open_menu = None;
                if let Some(p) = sp.recent.get((id - ACT_RECENT_BASE) as usize).cloned() {
                    sp.open_path(&p);
                }
                return Outcome::Rerender;
            }
            Outcome::Idle
        }
    }
}

fn toggle(cur: Option<OpenMenu>, want: OpenMenu) -> Option<OpenMenu> {
    if cur == Some(want) { None } else { Some(want) }
}

// ── Commit + main loop ────────────────────────────────────────────────

fn commit_tree(sp: &mut Spell) {
    // A tab switch: put that document's caret back where it was.
    let node = sp.cur().node();
    if node != sp.shown_node {
        sp.shown_node = node;
        let (a, c) = sp.cur().history.caret();
        sp.place(a, c);
    }
    sp.cur_mut().refresh_spans();
    let tree = render(sp);
    // `wire::encode` prepends the WIRE_VERSION byte that the compositor's
    // `scene_commit` checks — a raw postcard payload is rejected (-1).
    match wire::encode(&tree) {
        Ok(bytes) => { if !host::scene_commit(&bytes) { log("[spell] commit failed"); } }
        Err(_) => log("[spell] encode failed"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Tiled app — the first commit creates the window, and the TextArea's
    // `Autofocus` gives it the keyboard.
    let mut sp = Spell::new();

    commit_tree(&mut sp);
    // After the first commit — the window has to exist before it can be
    // guarded. From here Mod+Q and the × ask us first.
    window_set_close_guard(true);

    let mut event_buf = alloc::vec![0u8; EVENT_BUF_SIZE];
    loop {
        match poll_event(&mut event_buf) {
            PollResult::Event(ev) => {
                // The event's text (Open path, InputChange value, picked
                // path), whole: a document has no size limit here.
                let payload = match &ev {
                    Event::Open(s) => s.clone(),
                    Event::InputChangeAt { value, .. } => value.clone(),
                    Event::Picked { path, .. } => path.clone(),
                    _ => alloc::string::String::new(),
                };
                let outcome = handle(&mut sp, ev, &payload);
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Rerender => commit_tree(&mut sp),
                    Outcome::Exit => { sp.save_config(); host::close_widget(); return; }
                }
            }
            PollResult::Empty => host::sleep_ms(16),
            // Window pulled out from under us (hard close): the settings
            // write still goes through, npkFS doesn't need the window.
            PollResult::WindowGone => { sp.save_config(); return; }
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
