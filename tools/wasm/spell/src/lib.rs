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
use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Declared capabilities: read + write (save files) + render. No EXEC —
// Spell never launches intents. The kernel grants exactly this.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::WRITE | caps::RENDER];

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
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

// Directory listing scratch for the open dialog.
const LIST_BUF_SIZE: usize = 64 * 1024;
static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];

const NAME_FETCH_CAP: usize = 64;
static mut NAME_BUF: [u8; NAME_FETCH_CAP] = [0; NAME_FETCH_CAP];

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
// Name dialog: on_submit of the filename Input + the two buttons.
const ACT_NAME_SUBMIT:  u32 = 6_005;
const ACT_NAME_CANCEL:  u32 = 6_006;
// Unsaved-changes-on-close dialog.
const ACT_CLOSE_SAVE:    u32 = 6_007;
const ACT_CLOSE_DISCARD: u32 = 6_008;
const ACT_CLOSE_CANCEL:  u32 = 6_009;
const ACT_VIEW_EDIT:    u32 = 6_100;
const ACT_VIEW_PREVIEW: u32 = 6_101;
const ACT_HELP_ABOUT: u32 = 6_300;

// i-th file in the open dialog.
const ACT_OPEN_FILE_BASE: u32 = 7_000;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Markdown, Code(Lang), Plain }

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
            title: "Unbenannt".to_string(),
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
            None                                   => Kind::Markdown, // fresh buffer
            _                                      => Kind::Plain,    // txt/log/toml/yaml/…
        }
    }

    fn kind_label(&self) -> String {
        self.ext().unwrap_or_else(|| "md".to_string())
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
    /// The open dialog (a file-list popover) is showing.
    picking:      bool,
    /// The "Speichern unter…" name dialog is showing. While true the
    /// TextArea is hidden, so the name `Input` is the only editable
    /// widget and `InputChange` is unambiguous.
    naming:       bool,
    /// Filename being typed in the name dialog (pre-allocated so edits
    /// stay within capacity across `alloc_reset`).
    name_buf:     String,
    /// `<home>/documents` — the open dialog's source directory.
    docs_dir:     String,
    /// Files listed by the open dialog (full npkFS paths).
    files:        Vec<String>,
    /// Tab index pending an unsaved-changes confirmation before close.
    /// `Some(i)` shows the "save changes?" dialog for tab `i`.
    confirm_close: Option<usize>,
}

impl Spell {
    fn new() -> Self {
        let home = read_home_dir();
        let docs_dir = alloc::format!("{}/documents", home);
        let mut sp = Spell {
            docs:      Vec::new(),
            active:    0,
            open_menu: None,
            picking:   false,
            naming:    false,
            name_buf:  String::with_capacity(256),
            docs_dir,
            files:     Vec::new(),
            confirm_close: None,
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
        d.text.push_str("# Willkommen bei Spell\n\nTippe los, oder öffne eine Datei über **Datei → Öffnen…** — oder Doppelklick in loft.\n\n- Mehrere Dateien als Tabs\n- Code wird live farbig dargestellt\n- Tab rückt 4 Leerzeichen ein\n");
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

    /// Save the active doc, or open the name dialog if it has no file yet.
    fn save_or_name(&mut self) {
        if let Some(p) = self.cur().path.clone() {
            self.write_to(&p);
        } else {
            self.start_naming("");
        }
    }

    fn write_to(&mut self, path: &str) {
        let r = unsafe {
            npk_store(path.as_ptr() as i32, path.len() as i32,
                      self.cur().text.as_ptr() as i32, self.cur().text.len() as i32)
        };
        if r < 0 { log("[spell] save: store failed"); return; }
        self.cur_mut().dirty = false;
    }

    /// Open the "Speichern unter…" dialog. `default` pre-fills the name
    /// field (current basename, or a sensible suggestion when empty).
    fn start_naming(&mut self, default: &str) {
        self.open_menu = None;
        self.picking = false;
        self.name_buf.clear();
        if default.is_empty() {
            self.name_buf.push_str(match self.cur().kind() {
                Kind::Code(Lang::Rust) => "unbenannt.rs",
                _ => "unbenannt.md",
            });
        } else {
            self.name_buf.push_str(default);
        }
        self.naming = true;
    }

    /// Commit the name dialog: write the active doc to `documents/<name>`
    /// and adopt it as that doc's file.
    fn commit_name(&mut self) {
        let name = self.name_buf.trim();
        if name.is_empty() { return; }
        let path = alloc::format!("{}/{}", self.docs_dir, name);
        self.write_to(&path);
        let title = basename(&path).to_string();
        let d = self.cur_mut();
        d.title = title;
        d.path = Some(path);
        self.naming = false;
    }

    fn refresh_files(&mut self) {
        self.files = list_files(&self.docs_dir);
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

// ── Filesystem helpers ────────────────────────────────────────────────

fn read_home_dir() -> String {
    // The username lives in the single encrypted `.system/config` blob,
    // not a fetchable `sys/config/name` object, so ask the kernel for the
    // resolved home dir ("home/<name>") directly.
    let buf_ptr = core::ptr::addr_of_mut!(NAME_BUF) as *mut u8;
    let n = unsafe { npk_home_dir(buf_ptr as i32, NAME_FETCH_CAP as i32) };
    if n <= 0 { return String::from("home"); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    match core::str::from_utf8(slice) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => String::from("home"),
    }
}

/// List regular files directly under `dir` (non-recursive), returning
/// full npkFS paths. Directories and `.dir` markers are skipped.
fn list_files(dir: &str) -> Vec<String> {
    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    let n = unsafe {
        npk_fs_list(dir.as_ptr() as i32, dir.len() as i32,
                    buf_ptr as i32, LIST_BUF_SIZE as i32, 0)
    };
    if n <= 0 { return Vec::new(); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    let mut out: Vec<String> = Vec::new();
    for line in slice.split(|&b| b == b'\n') {
        // Wire: name\0size(8)\0is_dir(1)[\0mtime(8)]
        let nul = match line.iter().position(|&b| b == 0) { Some(i) => i, None => continue };
        let name = match core::str::from_utf8(&line[..nul]) { Ok(s) => s, Err(_) => continue };
        if name.is_empty() || name == ".dir" { continue; }
        let rest = &line[nul + 1..];
        if rest.len() < 10 { continue; }
        if rest[9] != 0 { continue; } // is_dir
        out.push(alloc::format!("{}/{}", dir, name));
    }
    out.sort();
    out
}

fn basename(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
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

    // Open dialog takes precedence over the Datei dropdown (it replaces
    // it, anchored to the same menu label).
    if sp.picking {
        children.push(render_open_dialog(sp));
    } else if let Some(kind) = sp.open_menu {
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
        ("Datei".to_string(),   ActionId(ACT_MENU_FILE)),
        ("Ansicht".to_string(), ActionId(ACT_MENU_VIEW)),
        ("Hilfe".to_string(),   ActionId(ACT_MENU_HELP)),
    ];
    let anchors: Vec<NodeId> = alloc::vec![
        NodeId(NODE_MENU_FILE),
        NodeId(NODE_MENU_VIEW),
        NodeId(NODE_MENU_HELP),
    ];
    prefab::menu_bar_with_anchors(&labels, &anchors)
}

fn render_dropdown(sp: &Spell, kind: OpenMenu) -> (u32, Widget) {
    match kind {
        OpenMenu::File => (
            NODE_MENU_FILE,
            prefab::popover_menu(&[
                ("Neu".to_string(),            ActionId(ACT_FILE_NEW)),
                ("Öffnen…".to_string(),        ActionId(ACT_FILE_OPEN)),
                ("Speichern".to_string(),      ActionId(ACT_FILE_SAVE)),
                ("Speichern unter…".to_string(), ActionId(ACT_FILE_SAVE_AS)),
                ("Schließen".to_string(),      ActionId(ACT_FILE_CLOSE)),
            ], None),
        ),
        OpenMenu::View => (
            NODE_MENU_VIEW,
            prefab::popover_menu(&[
                ("Quelltext".to_string(), ActionId(ACT_VIEW_EDIT)),
                ("Vorschau".to_string(),  ActionId(ACT_VIEW_PREVIEW)),
            ], Some(match sp.cur().mode { Mode::Edit => 0, Mode::Preview => 1 })),
        ),
        OpenMenu::Help => (
            NODE_MENU_HELP,
            prefab::popover_menu(&[
                ("Über Spell".to_string(), ActionId(ACT_HELP_ABOUT)),
            ], None),
        ),
    }
}

fn render_open_dialog(sp: &Spell) -> Widget {
    let content = if sp.files.is_empty() {
        prefab::popover_menu(&[
            ("(keine Dateien in documents)".to_string(), ActionId(ACT_MENU_DISMISS)),
        ], None)
    } else {
        let items: Vec<(String, ActionId)> = sp.files.iter().enumerate()
            .map(|(i, p)| (basename(p).to_string(), ActionId(ACT_OPEN_FILE_BASE + i as u32)))
            .collect();
        prefab::popover_menu(&items, None)
    };
    Widget::Popover {
        anchor:     NodeId(NODE_MENU_FILE),
        child:      Box::new(content),
        on_dismiss: ActionId(ACT_MENU_DISMISS),
        modifiers:  alloc::vec![],
    }
}

/// Tab bar — one tab per open document, active tab highlighted, each
/// with a dirty dot and a close (×). Trailing "+" opens a new tab.
/// Replaces the old filename+icons toolbar (save lives in the Datei
/// menu, the markdown view toggle in the Ansicht menu).
fn render_tabbar(sp: &Spell) -> Widget {
    let mut tabs: Vec<Widget> = Vec::with_capacity(sp.docs.len() + 1);
    for (i, d) in sp.docs.iter().enumerate() {
        let active = i == sp.active;
        let label = if d.dirty { alloc::format!("{} ●", d.title) } else { d.title.clone() };
        let mut mods: Vec<Modifier> = alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::OnClick(ActionId(ACT_TAB_BASE + i as u32)),
            Modifier::Rounded(Radius::Sm.as_u8()),
        ];
        if active {
            mods.push(Modifier::Background(Token::SurfaceElevated));
            mods.push(Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Sm.as_u8() });
        } else {
            mods.push(Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Rounded(Radius::Sm.as_u8()),
            ]));
        }
        tabs.push(Widget::Row {
            children: alloc::vec![
                Widget::Text {
                    content:   label,
                    style:     TextStyle::Body,
                    modifiers: if active { alloc::vec![] } else { alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                },
                Widget::Icon {
                    id:   IconId::X,
                    size: 14,
                    modifiers: alloc::vec![
                        Modifier::OnClick(ActionId(ACT_TAB_CLOSE_BASE + i as u32)),
                        Modifier::Tint(Token::OnSurfaceMuted),
                    ],
                },
            ],
            spacing:   Spacing::Xs.as_u16(),
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
            Modifier::OnClick(ActionId(ACT_FILE_NEW)),
            Modifier::Hover(alloc::vec![
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Rounded(Radius::Sm.as_u8()),
            ]),
        ],
    });
    Widget::Scroll {
        child: Box::new(Widget::Row {
            children:  tabs,
            spacing:   Spacing::Xs.as_u16(),
            align:     Align::Center,
            // Taller padding so the tab row matches loft's toolbar height
            // (its search box makes it tall) → the divider below the tab
            // row lines up with loft's across a split view.
            modifiers: alloc::vec![Modifier::Padding(Padding::Md.as_u16())],
        }),
        axis:      Axis::Horizontal,
        modifiers: alloc::vec![],
    }
}

fn render_body(sp: &Spell) -> Widget {
    if sp.confirm_close.is_some() {
        return render_confirm_dialog(sp);
    }
    if sp.naming {
        return render_name_dialog(sp);
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
                Modifier::Background(Token::Surface),
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
        placeholder: "Tippe los…".to_string(),
        spans,
        modifiers:   alloc::vec![
            Modifier::Flex(1),
            Modifier::Background(Token::Surface),
            Modifier::Padding(Padding::Sm.as_u16()),
        ],
    }
}

/// Unsaved-changes confirmation shown when closing a dirty tab. Buttons
/// only (no text input), so no focus dance needed.
fn render_confirm_dialog(sp: &Spell) -> Widget {
    let title = match sp.confirm_close.and_then(|i| sp.docs.get(i)) {
        Some(d) => d.title.clone(),
        None => "Datei".to_string(),
    };
    let card = prefab::dialog(
        "Ungespeicherte Änderungen",
        Widget::Column {
            children: alloc::vec![
                Widget::Text {
                    content:   alloc::format!("„{}“ hat ungespeicherte Änderungen.", title),
                    style:     TextStyle::Body,
                    modifiers: alloc::vec![],
                },
                Widget::Row {
                    children: alloc::vec![
                        prefab::button("Verwerfen", prefab::ButtonStyle::Destructive, ActionId(ACT_CLOSE_DISCARD)),
                        Widget::Spacer { flex: 1 },
                        prefab::button("Abbrechen", prefab::ButtonStyle::Ghost,   ActionId(ACT_CLOSE_CANCEL)),
                        prefab::button("Speichern",  prefab::ButtonStyle::Primary, ActionId(ACT_CLOSE_SAVE)),
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
        Some("Esc bricht ab"),
        380,
    );
    Widget::Column {
        children:  alloc::vec![Widget::Spacer { flex: 1 }, card, Widget::Spacer { flex: 1 }],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Flex(1), Modifier::Padding(Padding::Lg.as_u16())],
    }
}

/// "Speichern unter…" dialog — fills the body (TextArea hidden) so the
/// name `Input` is the only editable widget. Click the field to focus it
/// (a re-commit doesn't auto-focus), type the name, Enter or Speichern.
fn render_name_dialog(sp: &Spell) -> Widget {
    let card = prefab::dialog(
        "Speichern unter",
        Widget::Column {
            children: alloc::vec![
                Widget::Text {
                    content:   alloc::format!("Dateiname (in {}/):", basename(&sp.docs_dir)),
                    style:     TextStyle::Muted,
                    modifiers: alloc::vec![],
                },
                prefab::input(&sp.name_buf, "name.md", prefab::InputKind::Text,
                              ActionId(ACT_NAME_SUBMIT), None),
                Widget::Row {
                    children: alloc::vec![
                        Widget::Spacer { flex: 1 },
                        prefab::button("Abbrechen", prefab::ButtonStyle::Ghost,   ActionId(ACT_NAME_CANCEL)),
                        prefab::button("Speichern",  prefab::ButtonStyle::Primary, ActionId(ACT_NAME_SUBMIT)),
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
        Some("Klicke ins Feld, dann Enter zum Speichern · Esc bricht ab"),
        360,
    );
    // Centre the dialog in the body area.
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
        Event::Key(KeyCode::Escape) => {
            if sp.confirm_close.is_some() { sp.confirm_close = None; Outcome::Rerender }
            else if sp.naming { sp.naming = false; Outcome::Rerender }
            else if sp.picking { sp.picking = false; Outcome::Rerender }
            else if sp.open_menu.is_some() { sp.open_menu = None; Outcome::Rerender }
            // Quit only when nothing has unsaved changes; otherwise ask
            // about the active tab first.
            else if sp.cur().dirty { sp.request_close(sp.active); Outcome::Rerender }
            else if sp.docs.iter().any(|d| d.dirty) {
                if let Some(i) = sp.docs.iter().position(|d| d.dirty) { sp.request_close(i); }
                Outcome::Rerender
            }
            else { Outcome::Exit }
        }
        Event::InputChange { .. } => {
            // `payload` is the stabilized buffer value (the event's own
            // String was freed by alloc_reset).
            if sp.naming {
                // Only the name Input is editable while naming.
                sp.name_buf.clear();
                sp.name_buf.push_str(payload);
            } else {
                // Mirror the compositor's edit buffer into the active doc.
                let d = sp.cur_mut();
                d.set_text(payload);
                d.dirty = true;
            }
            Outcome::Rerender
        }
        // Another file was opened while we're running (loft association,
        // singleton routing) → open it as a tab. `payload` = the path.
        Event::Open(_) => {
            sp.open_path(payload);
            Outcome::Rerender
        }
        Event::Action(ActionId(id)) => handle_action(sp, id),
        _ => Outcome::Idle,
    }
}

fn handle_action(sp: &mut Spell, id: u32) -> Outcome {
    match id {
        ACT_MENU_FILE => { sp.picking = false; sp.open_menu = toggle(sp.open_menu, OpenMenu::File); Outcome::Rerender }
        ACT_MENU_VIEW => { sp.picking = false; sp.open_menu = toggle(sp.open_menu, OpenMenu::View); Outcome::Rerender }
        ACT_MENU_HELP => { sp.picking = false; sp.open_menu = toggle(sp.open_menu, OpenMenu::Help); Outcome::Rerender }
        ACT_MENU_DISMISS => {
            if sp.picking || sp.open_menu.is_some() {
                sp.picking = false;
                sp.open_menu = None;
                Outcome::Rerender
            } else { Outcome::Idle }
        }
        ACT_FILE_NEW => {
            sp.open_menu = None;
            sp.new_doc();
            Outcome::Rerender
        }
        ACT_FILE_OPEN => {
            sp.open_menu = None;
            sp.refresh_files();
            sp.picking = true;
            Outcome::Rerender
        }
        ACT_FILE_SAVE => { sp.open_menu = None; sp.save_or_name(); Outcome::Rerender }
        ACT_FILE_SAVE_AS => {
            let d = if sp.cur().path.is_some() { sp.cur().title.clone() } else { String::new() };
            sp.start_naming(&d);
            Outcome::Rerender
        }
        ACT_NAME_SUBMIT => { sp.commit_name(); Outcome::Rerender }
        ACT_NAME_CANCEL => { sp.naming = false; Outcome::Rerender }
        ACT_FILE_CLOSE => { sp.open_menu = None; sp.request_close(sp.active); Outcome::Rerender }
        // Unsaved-changes dialog buttons.
        ACT_CLOSE_DISCARD => {
            if let Some(i) = sp.confirm_close.take() { sp.close_tab(i); }
            Outcome::Rerender
        }
        ACT_CLOSE_CANCEL => { sp.confirm_close = None; Outcome::Rerender }
        ACT_CLOSE_SAVE => {
            if let Some(i) = sp.confirm_close.take() {
                sp.active = i;
                if let Some(p) = sp.cur().path.clone() {
                    sp.write_to(&p);
                    sp.close_tab(i);
                } else {
                    // No filename yet → name it first; close afterwards
                    // (the tab is no longer dirty once saved).
                    sp.start_naming("");
                }
            }
            Outcome::Rerender
        }
        ACT_VIEW_EDIT => { sp.open_menu = None; sp.cur_mut().mode = Mode::Edit; Outcome::Rerender }
        ACT_VIEW_PREVIEW => { sp.open_menu = None; sp.cur_mut().mode = Mode::Preview; Outcome::Rerender }
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
            if id >= ACT_OPEN_FILE_BASE {
                let i = (id - ACT_OPEN_FILE_BASE) as usize;
                if let Some(path) = sp.files.get(i).cloned() {
                    sp.open_path(&path);
                    sp.picking = false;
                    return Outcome::Rerender;
                }
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
                    _ => 0,
                };
                alloc_reset(persistent_mark);
                let outcome = handle(&mut sp, ev, payload_str(plen));
                persistent_mark = alloc_mark();
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Rerender => commit_tree(&sp),
                    Outcome::Exit => { close_self(); return; }
                }
            }
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            PollResult::WindowGone => return,
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
