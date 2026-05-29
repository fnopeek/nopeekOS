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
use nopeek_widgets::style::{Padding, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
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

const ACT_TOOLBAR_SAVE: u32 = 4_000;
const ACT_TOOLBAR_MODE: u32 = 4_001;

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
const ACT_VIEW_EDIT:    u32 = 6_100;
const ACT_VIEW_PREVIEW: u32 = 6_101;
const ACT_HELP_ABOUT: u32 = 6_300;

// i-th file in the open dialog.
const ACT_OPEN_FILE_BASE: u32 = 7_000;

const NODE_MENU_FILE: u32 = 100;
const NODE_MENU_VIEW: u32 = 102;
const NODE_MENU_HELP: u32 = 104;

// ── State ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Edit, Preview }

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenMenu { File, View, Help }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Markdown, Rust, Plain }

struct Spell {
    /// The whole document. Pre-allocated to `TEXT_CAP`; mutated via
    /// `clear` + `push_str` so the backing buffer stays put across
    /// `alloc_reset` between frames.
    text:         String,
    /// npkFS path of the open file, or None for an unsaved buffer.
    path:         Option<String>,
    /// Basename shown in the toolbar.
    title:        String,
    dirty:        bool,
    mode:         Mode,
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
}

impl Spell {
    fn new() -> Self {
        let home = read_home_dir();
        let docs_dir = alloc::format!("{}/documents", home);
        let mut text = String::with_capacity(TEXT_CAP);
        text.push_str("# Willkommen bei Spell\n\nTippe los, oder öffne eine Datei über **Datei → Öffnen…**.\n\n- Markdown-Vorschau über *Ansicht → Vorschau*\n- Speichern über das Disketten-Icon\n");
        Spell {
            text,
            path:      None,
            title:     "Unbenannt".to_string(),
            dirty:     false,
            mode:      Mode::Edit,
            open_menu: None,
            picking:   false,
            naming:    false,
            name_buf:  String::with_capacity(256),
            docs_dir,
            files:     Vec::new(),
        }
    }

    fn kind(&self) -> Kind {
        match self.path.as_deref() {
            Some(p) if p.ends_with(".md") || p.ends_with(".markdown") => Kind::Markdown,
            Some(p) if p.ends_with(".rs") => Kind::Rust,
            None => Kind::Markdown, // fresh buffers default to markdown
            _ => Kind::Plain,
        }
    }

    fn kind_label(&self) -> &'static str {
        match self.kind() {
            Kind::Markdown => "md",
            Kind::Rust     => "rs",
            Kind::Plain    => "txt",
        }
    }

    fn set_text(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
    }

    fn open(&mut self, path: &str) {
        let buf_ptr = core::ptr::addr_of_mut!(FETCH_BUF) as *mut u8;
        let n = unsafe {
            npk_fetch(path.as_ptr() as i32, path.len() as i32,
                      buf_ptr as i32, FETCH_BUF_SIZE as i32)
        };
        if n <= 0 {
            log("[spell] open: fetch failed");
            return;
        }
        let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
        match core::str::from_utf8(slice) {
            Ok(s) => self.set_text(s),
            Err(_) => { log("[spell] open: not UTF-8"); return; }
        }
        self.path  = Some(path.to_string());
        self.title = basename(path).to_string();
        self.dirty = false;
        self.mode  = Mode::Edit;
    }

    /// Save to the current file, or open the name dialog if the buffer
    /// has no filename yet (fresh "Neu" document).
    fn save_or_name(&mut self) {
        match self.path.clone() {
            Some(p) => self.write_to(&p),
            None => self.start_naming(""),
        }
    }

    fn write_to(&mut self, path: &str) {
        let r = unsafe {
            npk_store(path.as_ptr() as i32, path.len() as i32,
                      self.text.as_ptr() as i32, self.text.len() as i32)
        };
        if r < 0 { log("[spell] save: store failed"); return; }
        self.dirty = false;
    }

    /// Open the "Speichern unter…" dialog. `default` pre-fills the name
    /// field (current basename, or a sensible suggestion when empty).
    fn start_naming(&mut self, default: &str) {
        self.open_menu = None;
        self.picking = false;
        self.name_buf.clear();
        if default.is_empty() {
            self.name_buf.push_str(match self.kind() {
                Kind::Rust => "unbenannt.rs",
                _ => "unbenannt.md",
            });
        } else {
            self.name_buf.push_str(default);
        }
        self.naming = true;
    }

    /// Commit the name dialog: write the buffer to `documents/<name>`
    /// and adopt it as the current file.
    fn commit_name(&mut self) {
        let name = self.name_buf.trim();
        if name.is_empty() { return; }
        let path = alloc::format!("{}/{}", self.docs_dir, name);
        self.write_to(&path);
        self.title = basename(&path).to_string();
        self.path = Some(path);
        self.naming = false;
    }

    fn refresh_files(&mut self) {
        self.files = list_files(&self.docs_dir);
    }
}

// ── Filesystem helpers ────────────────────────────────────────────────

fn read_home_dir() -> String {
    let key = "sys/config/name";
    let buf_ptr = core::ptr::addr_of_mut!(NAME_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(key.as_ptr() as i32, key.len() as i32,
                  buf_ptr as i32, NAME_FETCH_CAP as i32)
    };
    if n <= 0 { return String::from("home"); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    match core::str::from_utf8(slice) {
        Ok(name) => {
            let name = name.trim();
            if name.is_empty() { String::from("home") }
            else { alloc::format!("home/{}", name) }
        }
        Err(_) => String::from("home"),
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
        render_toolbar(sp),
        Widget::Divider,
        render_body(sp),       // Flex(1) — fills
        Widget::Divider,
        render_footer(sp),
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
            ], Some(match sp.mode { Mode::Edit => 0, Mode::Preview => 1 })),
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

fn render_toolbar(sp: &Spell) -> Widget {
    let title = if sp.dirty {
        alloc::format!("{} *", sp.title)
    } else {
        sp.title.clone()
    };
    // Mode toggle: in Edit show the preview glyph (click → preview);
    // in Preview show the code glyph (click → edit).
    let mode_icon = match sp.mode { Mode::Edit => IconId::FileText, Mode::Preview => IconId::Code };
    Widget::Row {
        children: alloc::vec![
            Widget::Icon { id: IconId::FileText, size: 24, modifiers: alloc::vec![Modifier::Tint(Token::Accent)] },
            Widget::Text { content: title, style: TextStyle::Body, modifiers: alloc::vec![] },
            Widget::Spacer { flex: 1 },
            prefab::icon_button(mode_icon,        24, Some(ActionId(ACT_TOOLBAR_MODE)), None),
            prefab::icon_button(IconId::Download,  24, Some(ActionId(ACT_TOOLBAR_SAVE)), None),
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

fn render_body(sp: &Spell) -> Widget {
    if sp.naming {
        return render_name_dialog(sp);
    }
    match sp.mode {
        Mode::Edit => Widget::TextArea {
            value:       sp.text.clone(),
            placeholder: "Tippe los…".to_string(),
            modifiers:   alloc::vec![
                Modifier::Flex(1),
                Modifier::Background(Token::Surface),
                Modifier::Padding(Padding::Sm.as_u16()),
            ],
        },
        Mode::Preview => {
            let content = match sp.kind() {
                Kind::Markdown => markdown_preview(&sp.text),
                Kind::Rust     => code_preview(&sp.text, true),
                Kind::Plain    => code_preview(&sp.text, false),
            };
            Widget::Scroll {
                child:     Box::new(content),
                axis:      Axis::Vertical,
                modifiers: alloc::vec![
                    Modifier::Flex(1),
                    Modifier::Background(Token::Surface),
                    Modifier::Padding(Padding::Md.as_u16()),
                ],
            }
        }
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

fn render_footer(sp: &Spell) -> Widget {
    let lines = sp.text.split('\n').count();
    let left = alloc::format!("{} Zeilen · {} Zeichen", lines, sp.text.chars().count());
    let right = if sp.dirty {
        alloc::format!("{} · ● geändert", sp.kind_label())
    } else {
        alloc::format!("{} · gespeichert", sp.kind_label())
    };
    prefab::footer(&left, &right)
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

// ── Code preview (per-line tokenizer) ─────────────────────────────────

fn code_preview(text: &str, rust: bool) -> Widget {
    let rows: Vec<Widget> = text.split('\n').map(|line| {
        if rust {
            highlight_rust_line(line)
        } else {
            Widget::Text { content: line.to_string(), style: TextStyle::Mono, modifiers: alloc::vec![] }
        }
    }).collect();
    Widget::Column {
        children:  rows,
        spacing:   0,
        align:     Align::Start,
        modifiers: alloc::vec![],
    }
}

const RUST_KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "const", "static", "pub", "use", "mod", "crate", "struct",
    "enum", "impl", "trait", "type", "for", "while", "loop", "if", "else", "match",
    "return", "self", "Self", "as", "in", "ref", "move", "where", "async", "await",
    "dyn", "unsafe", "extern", "break", "continue", "true", "false", "Some", "None",
    "Ok", "Err", "super",
];

fn mono_span(s: &str, tint: Option<Token>) -> Widget {
    let mods = match tint {
        Some(t) => alloc::vec![Modifier::Tint(t)],
        None => alloc::vec![],
    };
    Widget::Text { content: s.to_string(), style: TextStyle::Mono, modifiers: mods }
}

/// Lightweight single-line Rust highlighter: line comments → muted,
/// "string literals" → warning, keywords → accent, everything else →
/// on-surface. Good enough for the visual signal; not a real lexer.
fn highlight_rust_line(line: &str) -> Widget {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") {
        return mono_span(line, Some(Token::OnSurfaceMuted));
    }

    let mut spans: Vec<Widget> = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut word_start: Option<usize> = None;

    let flush_word = |spans: &mut Vec<Widget>, line: &str, start: usize, end: usize| {
        let w = &line[start..end];
        let tint = if RUST_KEYWORDS.contains(&w) { Some(Token::Accent) } else { None };
        spans.push(mono_span(w, tint));
    };

    while i < bytes.len() {
        let b = bytes[i];
        let is_word = b.is_ascii_alphanumeric() || b == b'_';
        if is_word {
            if word_start.is_none() { word_start = Some(i); }
            i += 1;
            continue;
        }
        // Non-word boundary — flush any pending word.
        if let Some(ws) = word_start.take() {
            flush_word(&mut spans, line, ws, i);
        }
        // Line comment to end-of-line.
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            spans.push(mono_span(&line[i..], Some(Token::OnSurfaceMuted)));
            break;
        }
        // String literal.
        if b == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' { i += 2; continue; }
                if bytes[i] == b'"' { i += 1; break; }
                i += 1;
            }
            let end = i.min(line.len());
            spans.push(mono_span(&line[start..end], Some(Token::Warning)));
            continue;
        }
        // Punctuation / whitespace — emit one byte as plain.
        spans.push(mono_span(&line[i..i + 1], None));
        i += 1;
    }
    if let Some(ws) = word_start.take() {
        flush_word(&mut spans, line, ws, bytes.len());
    }
    if spans.is_empty() {
        return mono_span(line, None);
    }
    Widget::Row {
        children:  spans,
        spacing:   0,
        align:     Align::Start,
        modifiers: alloc::vec![],
    }
}

// ── Events ────────────────────────────────────────────────────────────

enum Outcome { Idle, Rerender, Exit }

fn handle(sp: &mut Spell, ev: Event) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => {
            if sp.naming { sp.naming = false; Outcome::Rerender }
            else if sp.picking { sp.picking = false; Outcome::Rerender }
            else if sp.open_menu.is_some() { sp.open_menu = None; Outcome::Rerender }
            else { Outcome::Exit }
        }
        Event::InputChange { value } => {
            if sp.naming {
                // Only the name Input is editable while naming.
                sp.name_buf.clear();
                sp.name_buf.push_str(&value);
            } else {
                // Mirror the compositor's edit buffer (the whole document).
                sp.set_text(&value);
                sp.dirty = true;
            }
            Outcome::Rerender
        }
        Event::Action(ActionId(id)) => handle_action(sp, id),
        _ => Outcome::Idle,
    }
}

fn handle_action(sp: &mut Spell, id: u32) -> Outcome {
    match id {
        ACT_TOOLBAR_SAVE => { sp.save_or_name(); Outcome::Rerender }
        ACT_TOOLBAR_MODE => {
            sp.mode = match sp.mode { Mode::Edit => Mode::Preview, Mode::Preview => Mode::Edit };
            Outcome::Rerender
        }
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
            sp.set_text("");
            sp.path = None;
            sp.title = "Unbenannt".to_string();
            sp.dirty = false;
            sp.mode = Mode::Edit;
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
            let d = if sp.path.is_some() { sp.title.clone() } else { String::new() };
            sp.start_naming(&d);
            Outcome::Rerender
        }
        ACT_NAME_SUBMIT => { sp.commit_name(); Outcome::Rerender }
        ACT_NAME_CANCEL => { sp.naming = false; Outcome::Rerender }
        ACT_FILE_CLOSE => Outcome::Exit,
        ACT_VIEW_EDIT => { sp.open_menu = None; sp.mode = Mode::Edit; Outcome::Rerender }
        ACT_VIEW_PREVIEW => { sp.open_menu = None; sp.mode = Mode::Preview; Outcome::Rerender }
        ACT_HELP_ABOUT => {
            log("[spell] Spell 0.1 — nopeekOS text editor");
            sp.open_menu = None;
            Outcome::Rerender
        }
        _ => {
            if id >= ACT_OPEN_FILE_BASE {
                let i = (id - ACT_OPEN_FILE_BASE) as usize;
                if let Some(path) = sp.files.get(i).cloned() {
                    sp.open(&path);
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
                alloc_reset(persistent_mark);
                let outcome = handle(&mut sp, ev);
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
