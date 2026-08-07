//! spell 0.1 — text editor with markdown preview, in loft's visual
//! language.
//!
//! Layout (top → bottom):
//!   menu_bar   — Datei / Ansicht / Hilfe
//!   toolbar    — file name (+ dirty marker) · mode toggle · save
//!   body       — TextArea (edit) OR rendered preview (read-only)
//!   footer     — line + byte counts · file kind · saved/modified
//!
//! Editing uses `Widget::TextArea`: the compositor owns the 2-D caret
//! (arrows / Enter / Home-End / PageUp-Down), the app only mirrors the
//! document via `Event::InputChange`. Syntax highlight + markdown live
//! in the preview pane (read-only `Text` + `Tint` spans) because an
//! editable field renders flat text by design.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
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

// Declared capabilities: read + render. **No WRITE** — an editor that can
// overwrite any file in the store is exactly what the file-dialog portal
// exists to avoid.
//
// Saving still works, through two narrower routes the kernel grants:
//   - the path the user picked in the dialog (`npk_pick` records it
//     against this instance — the click IS the authorisation), and
//   - `sys/config/spell`, our own settings file, whose name the kernel
//     derives from the module name so we can't claim someone else's.
//
// Browsing is likewise not ours: the dialog runs in its own module and
// hands back a single path.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::RENDER];

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_pick(mode: i32, start_ptr: i32, start_len: i32,
                suggest_ptr: i32, suggest_len: i32, tag: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_window_set_close_guard(on: i32) -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
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
    esc_cancels:   &'static str,
    /// "{} of {}" — position in the quit sequence.
    step_of:       &'static str,
    welcome:       &'static str,
}

const EN: Strings = Strings {
    menu_file: "File", menu_view: "View", menu_help: "Help",
    new: "New", open: "Open…", save: "Save", save_as: "Save as…",
    close: "Close",
    view_source: "Source", view_preview: "Preview",
    settings: "Settings",
    zoom_in: "Zoom in", zoom_out: "Zoom out",
    zoom_reset: "Actual size ({} px)",
    cfg_header: "# spell settings — one key per line, # is a comment.\n# font: editor size in px (6-64), Ctrl+0 restores 13.\n",
    about: "About Spell",
    untitled: "Untitled",
    untitled_file: "untitled",
    placeholder: "Start typing…",
    unsaved_title: "Unsaved changes",
    unsaved_body: "{} has unsaved changes.",
    discard: "Discard", cancel: "Cancel",
    esc_cancels: "Esc cancels",
    step_of: "{} of {}",
    welcome: "Welcome to Spell\n\nStart typing, or open a file via File \u{2192} Open\u{2026} \u{2014} or double-click in loft.\n\nMultiple files open as tabs. Save it with a name and the\nextension decides how it is highlighted.\n",
};

const DE: Strings = Strings {
    menu_file: "Datei", menu_view: "Ansicht", menu_help: "Hilfe",
    new: "Neu", open: "Öffnen…", save: "Speichern",
    save_as: "Speichern unter…", close: "Schließen",
    view_source: "Quelltext", view_preview: "Vorschau",
    settings: "Einstellungen",
    zoom_in: "Vergrößern", zoom_out: "Verkleinern",
    zoom_reset: "Originalgröße ({} px)",
    cfg_header: "# spell-Einstellungen — ein Schlüssel pro Zeile, # ist ein Kommentar.\n# font: Schriftgröße im Editor in px (6-64), Ctrl+0 stellt 13 her.\n",
    about: "Über Spell",
    untitled: "Unbenannt",
    untitled_file: "unbenannt",
    placeholder: "Tippe los…",
    unsaved_title: "Ungespeicherte Änderungen",
    unsaved_body: "{} hat ungespeicherte Änderungen.",
    discard: "Verwerfen", cancel: "Abbrechen",
    esc_cancels: "Esc bricht ab",
    step_of: "{} von {}",
    welcome: "Willkommen bei Spell\n\nTippe los, oder öffne eine Datei über Datei \u{2192} Öffnen\u{2026} \u{2014} oder Doppelklick in loft.\n\nMehrere Dateien liegen als Tabs nebeneinander. Speichere sie\nunter einem Namen, die Endung entscheidet über die Farben.\n",
};

fn s() -> &'static Strings {
    match i18n::lang() { i18n::Lang::De => &DE, _ => &EN }
}

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

/// "Originalgröße (13 px)" — naming the target makes the entry a status
/// line too: you can see how far you have zoomed without counting.
fn zoom_reset_label() -> String {
    let mut target = String::with_capacity(4);
    push_usize(&mut target, MONO_SIZE_PX as usize);
    fill(s().zoom_reset, &target)
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

fn close_self() { unsafe { let _ = npk_close_widget(); } }

// ── Buffers ───────────────────────────────────────────────────────────
//
// The event buffer must hold a full `InputChange` — its payload is the
// WHOLE document. `npk_event_poll` drops (does not truncate) an event
// that overflows, which would silently desync our mirror from the
// compositor's edit buffer, so size it well above any realistic file.
const EVENT_BUF_SIZE: usize = 512 * 1024;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

// Scratch for npk_fetch (open) — same ceiling as the edit buffer.
const FETCH_BUF_SIZE: usize = 512 * 1024;
static mut FETCH_BUF: [u8; FETCH_BUF_SIZE] = [0; FETCH_BUF_SIZE];

// Pre-allocate the document so ordinary edits stay within capacity and
// don't churn the bump allocator (mirrors loft's `query` discipline).
const TEXT_CAP: usize = 256 * 1024;

// An event's owned String (Open path / InputChange value) is allocated on
// the bump heap during poll, ABOVE persistent_mark — so `alloc_reset`
// before `handle` frees it and the first allocation in `handle` clobbers
// it (a use-after-free). We copy such payloads into this STATIC buffer
// (outside the bump heap) before the reset, and hand `handle` a &str into
// it. Sized to hold a whole-document InputChange.
const PAYLOAD_CAP: usize = 512 * 1024;
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

// ── Bump allocator (4 MB — a document plus its transient widget tree
//    and preview spans). State alloc'd before `persistent_mark`
//    survives `alloc_reset`; everything above the mark is per-frame. ──
const HEAP_SIZE: usize = 4 * 1024 * 1024;
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
fn panic(_: &core::panic::PanicInfo) -> ! { log("[spell] panic!"); loop {} }

fn alloc_reset(pos: usize) { unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(pos); } }
fn alloc_mark() -> usize { unsafe { core::ptr::addr_of!(HEAP_POS).read() } }

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

// `npk_pick` modes.
const PICK_OPEN: i32 = 0;
const PICK_SAVE: i32 = 1;

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

// ── State ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Edit, Preview }

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenMenu { File, View, Help }

/// What the file's extension says it is. A buffer with no filename yet is
/// `Untyped`: no highlighting, no preview toggle, no assumed extension —
/// it becomes a kind when the user names it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Markdown, Code(Lang), Plain, Untyped }

/// Languages with a preview highlighter. Markup = HTML/XML (tag-based);
/// the rest share the C-like tokenizer parameterised by `lang_spec`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang { Rust, C, Js, Python, Shell, Json, Markup }

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
}

impl Doc {
    fn empty() -> Self {
        Doc {
            path:  None,
            title: s().untitled.to_string(),
            text:  String::with_capacity(TEXT_CAP),
            dirty: false,
            mode:  Mode::Edit,
        }
    }

    fn ext(&self) -> Option<String> {
        self.path.as_deref()
            .and_then(|p| p.rsplit_once('.'))
            .map(|(_, e)| e.to_ascii_lowercase())
    }

    fn kind(&self) -> Kind {
        match self.ext().as_deref() {
            Some("md") | Some("markdown")          => Kind::Markdown,
            Some("rs")                             => Kind::Code(Lang::Rust),
            Some("c") | Some("h")                  => Kind::Code(Lang::C),
            Some("js") | Some("ts")                => Kind::Code(Lang::Js),
            Some("py")                             => Kind::Code(Lang::Python),
            Some("sh") | Some("bash")              => Kind::Code(Lang::Shell),
            Some("json")                           => Kind::Code(Lang::Json),
            Some("html") | Some("htm") | Some("xml") => Kind::Code(Lang::Markup),
            None                                   => Kind::Untyped, // unnamed buffer
            _                                      => Kind::Plain,   // txt/log/toml/yaml/…
        }
    }


    fn set_text(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
    }

    /// `true` for a fresh, never-edited Unbenannt tab — opening a file
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
    /// What is actually on disk. A wheel spin fires a dozen zoom events
    /// and each npkFS write costs an encrypt + hash + flush, so the file
    /// is written when leaving, not per notch — this says whether that
    /// write is still owed.
    font_px_saved: u16,
    /// How many dirty tabs the quit started with, and how many are done.
    /// Counted at the start because the live count shrinks as tabs close
    /// — deriving the step from it would show "1 of 3", "1 of 2", "1 of 1".
    quit_total:    usize,
    quit_done:     usize,
}

impl Spell {
    fn new() -> Self {
        let cfg_font = read_config_font();
        let mut sp = Spell {
            docs:      Vec::new(),
            active:    0,
            open_menu: None,
            confirm_close: None,
            quitting:      false,
            font_px:       cfg_font,
            font_px_saved: cfg_font,
            quit_total:    0,
            quit_done:     0,
        };

        // Launched to open a specific file (loft file association)?
        let mut argbuf = [0u8; 512];
        let n = unsafe { npk_launch_arg(argbuf.as_mut_ptr() as i32, argbuf.len() as i32) };
        if n > 0 {
            if let Ok(path) = core::str::from_utf8(&argbuf[..n as usize]) {
                if let Some(text) = read_file(path) {
                    let mut d = Doc::empty();
                    d.set_text(&text);
                    d.path = Some(path.to_string());
                    d.title = basename(path).to_string();
                    sp.docs.push(d);
                    return sp;
                }
            }
        }

        // No file argument → welcome tab.
        let mut d = Doc::empty();
        d.text.push_str(s().welcome);
        sp.docs.push(d);
        sp
    }

    fn cur(&self) -> &Doc { &self.docs[self.active] }
    fn cur_mut(&mut self) -> &mut Doc { &mut self.docs[self.active] }

    /// Open a file: focus its tab if already open (VS Code behaviour),
    /// else load it into a new tab (reusing a pristine Unbenannt tab).
    fn open_path(&mut self, path: &str) {
        if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(path)) {
            self.active = i;
            return;
        }
        let text = match read_file(path) {
            Some(t) => t,
            None => { log("[spell] open: failed"); return; }
        };
        let mut d = Doc::empty();
        d.set_text(&text);
        d.path = Some(path.to_string());
        d.title = basename(path).to_string();
        if self.cur().is_pristine() {
            let a = self.active;
            self.docs[a] = d;
        } else {
            self.docs.push(d);
            self.active = self.docs.len() - 1;
        }
    }

    fn new_doc(&mut self) {
        if self.cur().is_pristine() {
            let a = self.active;
            self.docs[a] = Doc::empty();
        } else {
            self.docs.push(Doc::empty());
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
        if self.font_px == self.font_px_saved { return; }
        write_config(self.font_px);
        self.font_px_saved = self.font_px;
    }

    fn cancel_quit(&mut self) {
        self.quitting = false;
        self.quit_total = 0;
        self.quit_done = 0;
        self.confirm_close = None;
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.docs.len() { return; }
        self.docs.remove(i);
        if self.docs.is_empty() {
            self.docs.push(Doc::empty());
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
            self.write_to(&p);
        } else {
            self.ask_save_target(self.active, false);
        }
    }

    fn write_to(&mut self, path: &str) {
        let r = unsafe {
            npk_store(path.as_ptr() as i32, path.len() as i32,
                      self.cur().text.as_ptr() as i32, self.cur().text.len() as i32)
        };
        if r < 0 { log("[spell] save: store failed"); return; }
        self.cur_mut().dirty = false;
        // Editing the settings file in spell itself is a stated use of
        // having one. Adopt what was just saved — otherwise our in-memory
        // value would be written back over it on the way out and the edit
        // would silently undo itself.
        if path == CONFIG_PATH {
            let f = read_config_font();
            self.font_px = f;
            self.font_px_saved = f;
        }
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
        let prev = self.active;
        self.active = doc;
        self.write_to(path);
        self.active = prev.min(self.docs.len().saturating_sub(1));
    }
}

/// Ask the kernel for a file dialog. `tag` comes back in `Event::Picked`.
fn pick(mode: i32, start: &str, suggest: &str, tag: u32) {
    unsafe {
        let _ = npk_pick(
            mode,
            start.as_ptr() as i32, start.len() as i32,
            suggest.as_ptr() as i32, suggest.len() as i32,
            tag as i32,
        );
    }
}

/// Fetch a file's contents as a String, or None on error / non-UTF-8.
fn read_file(path: &str) -> Option<String> {
    let buf_ptr = core::ptr::addr_of_mut!(FETCH_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(path.as_ptr() as i32, path.len() as i32,
                  buf_ptr as i32, FETCH_BUF_SIZE as i32)
    };
    if n <= 0 { return None; }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    core::str::from_utf8(slice).ok().map(|s| s.to_string())
}

// ── Config ────────────────────────────────────────────────────────────
//
// `sys/config/spell`, same shape as `sys/config/bar`: one `key: value`
// per line, `#` starts a comment. Meant to be opened and edited in spell
// itself, so it carries a header explaining what is in it and unknown
// keys are ignored rather than rejected — a newer spell's settings must
// not make an older one refuse the file.

const CONFIG_PATH: &str = "sys/config/spell";
const CONFIG_CAP: usize = 2048;
static mut CONFIG_BUF: [u8; CONFIG_CAP] = [0; CONFIG_CAP];

/// Read the saved font size. Anything missing or unparseable falls back
/// to the default — a broken config should never keep the editor shut.
fn read_config_font() -> u16 {
    let p = core::ptr::addr_of_mut!(CONFIG_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(CONFIG_PATH.as_ptr() as i32, CONFIG_PATH.len() as i32,
                  p as i32, CONFIG_CAP as i32)
    };
    if n <= 0 { return MONO_SIZE_PX; }
    let bytes = unsafe { core::slice::from_raw_parts(p as *const u8, n as usize) };
    let text = match core::str::from_utf8(bytes) { Ok(t) => t, Err(_) => return MONO_SIZE_PX };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((key, val)) = line.split_once(':') else { continue };
        if key.trim() != "font" { continue; }
        if let Ok(v) = val.trim().parse::<u16>() {
            return v.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        }
    }
    MONO_SIZE_PX
}

/// Write the config back, header and all. Called when settings change,
/// not on every wheel notch — see `Event::Zoom`.
fn write_config(font_px: u16) {
    let mut out = String::with_capacity(256);
    out.push_str(s().cfg_header);
    out.push_str("font: ");
    push_usize(&mut out, font_px as usize);
    out.push('\n');
    let r = unsafe {
        npk_store(CONFIG_PATH.as_ptr() as i32, CONFIG_PATH.len() as i32,
                  out.as_ptr() as i32, out.len() as i32)
    };
    if r < 0 { log("[spell] config: store failed"); }
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
        render_body(sp),       // Flex(1) — fills (no footer; removed as noise)
    ];

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
        (s().menu_view.to_string(), ActionId(ACT_MENU_VIEW)),
        (s().menu_help.to_string(), ActionId(ACT_MENU_HELP)),
    ];
    let anchors: Vec<NodeId> = alloc::vec![
        NodeId(NODE_MENU_FILE),
        NodeId(NODE_MENU_VIEW),
        NodeId(NODE_MENU_HELP),
    ];
    prefab::menu_bar_with_icon(IconId::FileText, &labels, &anchors)
}

fn render_dropdown(sp: &Spell, kind: OpenMenu) -> (u32, Widget) {
    match kind {
        // The menu is where the shortcuts are learned, so they're listed
        // next to the entry that binds them.
        OpenMenu::File => (
            NODE_MENU_FILE,
            prefab::popover_menu_shortcuts(&[
                (s().new.to_string(), ActionId(ACT_FILE_NEW)),
                (s().open.to_string(), ActionId(ACT_FILE_OPEN)),
                (s().save.to_string(), ActionId(ACT_FILE_SAVE)),
                (s().save_as.to_string(), ActionId(ACT_FILE_SAVE_AS)),
                (s().close.to_string(), ActionId(ACT_FILE_CLOSE)),
                (s().settings.to_string(), ActionId(ACT_FILE_SETTINGS)),
            ], &["Ctrl+N", "Ctrl+O", "Ctrl+S", "Ctrl+Shift+S", "Ctrl+W", ""], None),
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
/// Replaces the old filename+icons toolbar (save lives in the Datei
/// menu, the markdown view toggle in the Ansicht menu).
/// Fixed tab width — names ellipsize rather than letting the strip
/// reflow as you open files (UI_REFRESH.md §3 `tab`).
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

        // Unsaved work shows as an accent dot; only a tab you can act on
        // offers the ×. A saved, inactive tab shows neither.
        let trailing = if d.dirty {
            Widget::Row {
                children:  alloc::vec![prefab::mark(8, 8, Some(Token::Accent))],
                spacing:   0,
                align:     Align::Center,
                modifiers: alloc::vec![Modifier::Rounded(4)],
            }
        } else if active {
            Widget::Icon {
                id:   IconId::X,
                size: 16,
                modifiers: alloc::vec![
                    Modifier::OnClick(ActionId(ACT_TAB_CLOSE_BASE + i as u32)),
                    Modifier::Tint(Token::OnSurfaceMuted),
                ],
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
    let spans = match doc.kind() {
        Kind::Code(lang) => code_spans(&doc.text, lang),
        _ => Vec::new(),
    };
    Widget::TextArea {
        value:       doc.text.clone(),
        placeholder: s().placeholder.to_string(),
        spans,
        modifiers:   alloc::vec![
            Modifier::Flex(1),
            Modifier::Background(Token::Page),
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::LineNumbers(true),
            Modifier::FontSize(sp.font_px),
        ],
    }
}

/// Unsaved-changes confirmation shown when closing a dirty tab. Buttons
/// only (no text input), so no focus dance needed.
///
/// While quitting this is one step of a sequence, so it carries a
/// "(2 of 3)" counter — otherwise a second dialog appearing right after
/// the first reads like the button didn't take.
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
                        prefab::button(s().discard, prefab::ButtonStyle::Destructive, ActionId(ACT_CLOSE_DISCARD)),
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
        Some(s().esc_cancels),
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
/// Other inline markup (**bold**, *italic*) is left as literal text in
/// v1 — there is no bold weight in the text vocab yet.
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
// The tokenizers walk the whole buffer and emit `Span { start, len,
// token }` byte ranges for non-default tokens (keywords → Accent,
// strings → Warning, comments → Muted). Uncovered bytes render in the
// default colour. Spans come out sorted by `start` (left-to-right scan),
// which the compositor's renderer relies on.

fn push_span(out: &mut Vec<Span>, start: usize, len: usize, token: Token) {
    if len == 0 { return; }
    out.push(Span { start: start as u32, len: len as u32, token });
}

/// Tokenise the whole document into colour spans for the given language.
fn code_spans(text: &str, lang: Lang) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut base = 0usize;
    let markup = matches!(lang, Lang::Markup);
    let (kw, comment, squote) = lang_spec(lang);
    for line in text.split('\n') {
        if markup {
            markup_line_spans(line, base, &mut out);
        } else {
            clike_line_spans(line, base, kw, comment, squote, &mut out);
        }
        base += line.len() + 1; // include the '\n'
    }
    out
}

// Keyword sets — not full lexers, just enough for the visual signal.
const KW_RUST: &[&str] = &[
    "fn","let","mut","const","static","pub","use","mod","crate","struct","enum",
    "impl","trait","type","for","while","loop","if","else","match","return","self",
    "Self","as","in","ref","move","where","async","await","dyn","unsafe","extern",
    "break","continue","true","false","Some","None","Ok","Err","super",
];
const KW_C: &[&str] = &[
    "auto","break","case","char","const","continue","default","do","double","else",
    "enum","extern","float","for","goto","if","inline","int","long","register",
    "return","short","signed","sizeof","static","struct","switch","typedef","union",
    "unsigned","void","volatile","while","include","define","ifdef","ifndef","endif",
    "pragma","sizeof","NULL","true","false",
];
const KW_JS: &[&str] = &[
    "var","let","const","function","return","if","else","for","while","do","switch",
    "case","default","break","continue","new","delete","typeof","instanceof","this",
    "class","extends","super","import","export","from","async","await","yield","try",
    "catch","finally","throw","null","undefined","true","false","in","of","void","static",
];
const KW_PY: &[&str] = &[
    "def","class","return","if","elif","else","for","while","break","continue","import",
    "from","as","pass","with","try","except","finally","raise","yield","lambda","global",
    "nonlocal","and","or","not","in","is","None","True","False","async","await","del","assert",
];
const KW_SH: &[&str] = &[
    "if","then","else","elif","fi","for","while","until","do","done","case","esac",
    "function","in","select","echo","export","local","readonly","return","exit",
    "source","alias","unset","set","cd","then",
];
const KW_JSON: &[&str] = &["true","false","null"];

/// `(keywords, line-comment prefix or None, treat single-quotes as strings)`.
fn lang_spec(lang: Lang) -> (&'static [&'static str], Option<&'static str>, bool) {
    match lang {
        Lang::Rust   => (KW_RUST, Some("//"), false), // ' is a lifetime, not a string
        Lang::C      => (KW_C,    Some("//"), true),
        Lang::Js     => (KW_JS,   Some("//"), true),
        Lang::Python => (KW_PY,   Some("#"),  true),
        Lang::Shell  => (KW_SH,   Some("#"),  true),
        Lang::Json   => (KW_JSON, None,       false),
        Lang::Markup => (&[],     None,       false), // handled separately
    }
}

fn char_len(s: &str, i: usize) -> usize {
    s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
}

/// Tokenise one C-like line into spans (absolute byte base). Keywords →
/// Accent, `"…"`/`'…'` strings → Warning, line comments → Muted; the
/// rest is left uncovered (default colour). UTF-8-safe.
fn clike_line_spans(
    line: &str, base: usize, keywords: &[&str], comment: Option<&str>,
    squote: bool, out: &mut Vec<Span>,
) {
    // Whole-line comment.
    if let Some(c) = comment {
        if line.trim_start().starts_with(c) {
            push_span(out, base, line.len(), Token::OnSurfaceMuted);
            return;
        }
    }
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut word_start: Option<usize> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'_' {
            if word_start.is_none() { word_start = Some(i); }
            i += 1;
            continue;
        }
        if let Some(ws) = word_start.take() {
            let w = &line[ws..i];
            if keywords.contains(&w) { push_span(out, base + ws, i - ws, Token::Accent); }
        }
        // Mid-line comment to end of line.
        if let Some(c) = comment {
            if line[i..].starts_with(c) {
                push_span(out, base + i, line.len() - i, Token::OnSurfaceMuted);
                return;
            }
        }
        // String / char literal (quote is ASCII → boundaries are safe).
        if b == b'"' || (squote && b == b'\'') {
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 1;
                    if i < bytes.len() { i += char_len(line, i); }
                    continue;
                }
                if bytes[i] == quote { i += 1; break; }
                i += char_len(line, i);
            }
            push_span(out, base + start, i - start, Token::Warning);
            continue;
        }
        // Default char — no span.
        i += char_len(line, i);
    }
    if let Some(ws) = word_start.take() {
        let w = &line[ws..];
        if keywords.contains(&w) { push_span(out, base + ws, line.len() - ws, Token::Accent); }
    }
}

/// HTML/XML line → spans: tags `<…>` → Accent, `<!-- … -->` → Muted,
/// text left uncovered. Split points are ASCII, so boundaries are safe.
fn markup_line_spans(line: &str, base: usize, out: &mut Vec<Span>) {
    let mut i = 0;
    while i < line.len() {
        if line[i..].starts_with("<!--") {
            let end = line[i..].find("-->").map(|p| i + p + 3).unwrap_or(line.len());
            push_span(out, base + i, end - i, Token::OnSurfaceMuted);
            i = end;
            continue;
        }
        if line.as_bytes()[i] == b'<' {
            let end = line[i..].find('>').map(|p| i + p + 1).unwrap_or(line.len());
            push_span(out, base + i, end - i, Token::Accent);
            i = end;
            continue;
        }
        // Plain text up to the next tag — no span.
        let mut end = line[i..].find('<').map(|p| i + p).unwrap_or(line.len());
        if end <= i { end = line.len(); }
        i = end;
    }
}

// ── Events ────────────────────────────────────────────────────────────

enum Outcome { Idle, Rerender, Exit }

fn handle(sp: &mut Spell, ev: Event, payload: &str) -> Outcome {
    match ev {
        // Esc backs out of whatever is open. It does NOT quit: in an
        // editor Esc is the cancel key, and a stray press should never
        // end the session. Closing is Mod+Q, the window's ×, or
        // File → Close.
        Event::Key(KeyCode::Escape) => {
            if sp.confirm_close.is_some() { sp.cancel_quit(); Outcome::Rerender }
            else if sp.open_menu.is_some() { sp.open_menu = None; Outcome::Rerender }
            else { Outcome::Idle }
        }
        // The window manager is asking whether it may close us.
        Event::CloseRequest => {
            if sp.begin_quit() { Outcome::Exit } else { Outcome::Rerender }
        }
        Event::InputChange { .. } => {
            // `payload` is the stabilized buffer value (the event's own
            // String was freed by alloc_reset). The TextArea is the only
            // editable widget we render — the file dialogs live in their
            // own window now.
            let d = sp.cur_mut();
            d.set_text(payload);
            d.dirty = true;
            Outcome::Rerender
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
            b'n' => { sp.new_doc(); Outcome::Rerender }
            b'w' => { sp.request_close(sp.active); Outcome::Rerender }
            // Zoom. '=' comes along because on a US layout the '+' key is
            // Shift+'=', and people press Ctrl+'=' without the Shift.
            b'+' | b'=' => { sp.zoom(Some(1)); Outcome::Rerender }
            b'-' => { sp.zoom(Some(-1)); Outcome::Rerender }
            b'0' => { sp.zoom(None); Outcome::Rerender }
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
                    sp.write_to(&p);
                    sp.close_tab(i);
                    if sp.advance_quit() { return Outcome::Exit; }
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
            if read_file(CONFIG_PATH).is_none() { write_config(sp.font_px); }
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
            Outcome::Idle
        }
    }
}

fn toggle(cur: Option<OpenMenu>, want: OpenMenu) -> Option<OpenMenu> {
    if cur == Some(want) { None } else { Some(want) }
}

// ── Commit + main loop ────────────────────────────────────────────────

fn commit_tree(sp: &Spell) {
    let tree = render(sp);
    // `wire::encode` prepends the WIRE_VERSION byte that the compositor's
    // `scene_commit` checks — a raw postcard payload is rejected (-1).
    match wire::encode(&tree) {
        Ok(bytes) => { if commit(&bytes) < 0 { log("[spell] commit failed"); } }
        Err(_) => log("[spell] encode failed"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Tiled app — the first commit creates the window; the compositor
    // auto-focuses the first text widget (our TextArea) so the user can
    // type immediately. See the loft bump-allocator notes for the
    // persistent_mark / alloc_reset lifecycle.
    let mut sp = Spell::new();
    let mut persistent_mark = alloc_mark();

    commit_tree(&sp);
    // After the first commit — the window has to exist before it can be
    // guarded. From here Mod+Q and the × ask us first.
    unsafe { let _ = npk_window_set_close_guard(1); }

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                // Stabilize heap-backed payloads (Open path, InputChange
                // value) into the static buffer BEFORE alloc_reset frees
                // the event — otherwise handle's first allocation clobbers
                // them (use-after-free).
                let plen = match &ev {
                    Event::Open(s) => copy_payload(s),
                    Event::InputChange { value } => copy_payload(value),
                    Event::Picked { path, .. } => copy_payload(path),
                    _ => 0,
                };
                alloc_reset(persistent_mark);
                let outcome = handle(&mut sp, ev, payload_str(plen));
                persistent_mark = alloc_mark();
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Rerender => commit_tree(&sp),
                    Outcome::Exit => { sp.save_config(); close_self(); return; }
                }
            }
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            // Window pulled out from under us (hard close): the settings
            // write still goes through, npkFS doesn't need the window.
            PollResult::WindowGone => { sp.save_config(); return; }
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
