//! loft 0.2 — file browser, fresh rewrite against the v3 mockup.
//!
//! Layout (top → bottom):
//!   menu_bar      — Datei / Bearbeiten / Ansicht / Gehe zu / Hilfe
//!   toolbar       — back / forward / up / refresh + breadcrumb + search
//!   body          — sidebar │ grid (with empty-state)
//!   footer        — nav hints   ·   counts + selection
//!
//! Auto-focused search filters the current directory live (substring,
//! ASCII case-insensitive). Up/Down navigate the filtered grid;
//! Enter opens the selected entry; Esc clears the search if non-empty,
//! otherwise closes the window. Menu-bar clicks are intentionally
//! no-ops in v0.2 — dropdown overlays land once `Widget::Popover`
//! ships (Phase 11).

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::i18n;
use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Declared capabilities: read + write (copy/move/rename/delete files) +
// exec (npk_open launches the handler app) + render. Without this section
// loft would get the default READ|EXEC|RENDER and could not mutate the FS.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [caps::READ | caps::WRITE | caps::EXEC | caps::RENDER];

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;
    fn npk_fs_copy(old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32;
    fn npk_fs_rename(old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32;
    fn npk_open(app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_fs_usage() -> i64;
    fn npk_window_set_clipboard_sink() -> i32;
    fn npk_close_widget() -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

// ── Strings ───────────────────────────────────────────────────────────
// English is the source language; a new one is one more `const` below.
// See `nopeek_widgets::i18n`.

struct Strings {
    menu_file:      &'static str,
    menu_edit:      &'static str,
    menu_view:      &'static str,
    menu_go:        &'static str,
    menu_help:      &'static str,
    quit:           &'static str,
    view_grid:      &'static str,
    view_list:      &'static str,
    go_home:        &'static str,
    go_filesystem:  &'static str,
    about:          &'static str,
    copy:           &'static str,
    cut:            &'static str,
    paste:          &'static str,
    rename:         &'static str,
    no_action:      &'static str,
    rename_title:   &'static str,
    rename_label:   &'static str,
    rename_hint:    &'static str,
    cancel:         &'static str,
    confirm_rename: &'static str,
    search:         &'static str,
    empty_dir:      &'static str,
    no_matches:     &'static str,
    col_name:       &'static str,
    col_size:       &'static str,
    col_files:      &'static str,
    col_type:       &'static str,
    col_modified:   &'static str,
    no_handler:     &'static str,
}

const EN: Strings = Strings {
    menu_file: "File", menu_edit: "Edit", menu_view: "View",
    menu_go: "Go", menu_help: "Help",
    quit: "Quit",
    view_grid: "Grid", view_list: "List",
    go_home: "Home", go_filesystem: "Filesystem",
    about: "About loft",
    copy: "Copy", cut: "Cut", paste: "Paste", rename: "Rename…",
    no_action: "(no action)",
    rename_title: "Rename", rename_label: "New name:",
    rename_hint: "Click the field, then Enter · Esc cancels",
    cancel: "Cancel", confirm_rename: "Rename",
    search: "search",
    empty_dir: "Empty directory", no_matches: "No matches",
    col_name: "NAME", col_size: "SIZE", col_files: "FILES",
    col_type: "TYPE", col_modified: "MODIFIED",
    no_handler: "[loft] no app associated with this file type",
};

const DE: Strings = Strings {
    menu_file: "Datei", menu_edit: "Bearbeiten", menu_view: "Ansicht",
    menu_go: "Gehe zu", menu_help: "Hilfe",
    quit: "Beenden",
    view_grid: "Kacheln", view_list: "Liste",
    go_home: "Persönlicher Ordner", go_filesystem: "Dateisystem",
    about: "Über loft",
    copy: "Kopieren", cut: "Ausschneiden", paste: "Einfügen",
    rename: "Umbenennen…",
    no_action: "(keine Aktion)",
    rename_title: "Umbenennen", rename_label: "Neuer Name:",
    rename_hint: "Klicke ins Feld, dann Enter · Esc bricht ab",
    cancel: "Abbrechen", confirm_rename: "Umbenennen",
    search: "suchen",
    empty_dir: "Leerer Ordner", no_matches: "Keine Treffer",
    col_name: "NAME", col_size: "GRÖSSE", col_files: "DATEIEN",
    col_type: "TYP", col_modified: "GEÄNDERT",
    no_handler: "[loft] keine zugeordnete App für diesen Dateityp",
};

fn s() -> &'static Strings {
    match i18n::lang() { Lang::De => &DE, _ => &EN }
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

const EVENT_BUF_SIZE: usize = 256;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

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

fn close_self() { unsafe { let _ = npk_close_widget(); } }

// ── Bump allocator (1 MB — bigger than drun/loft 0.1 because the grid
//    can cover hundreds of entries in a deep directory). State alloc'd
//    before `persistent_mark` survives `alloc_reset` between commits;
//    everything after the mark is rebuilt from scratch each frame. ──
const HEAP_SIZE: usize = 1024 * 1024;
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
fn panic(_: &core::panic::PanicInfo) -> ! { log("[loft] panic!"); loop {} }

fn alloc_reset(pos: usize) { unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(pos); } }
fn alloc_mark() -> usize { unsafe { core::ptr::addr_of!(HEAP_POS).read() } }

// ── Action-id encoding ────────────────────────────────────────────────
//
// Each interaction surface gets its own base so the dispatcher can tell
// "which thing was clicked" by an integer comparison alone — no string
// keys, no payload. Bases are 1000 apart so each surface has plenty of
// room before colliding with the next. CLICK + HOVER share a surface
// but live in different bands so we can dedup hover events without
// confusing them with clicks.

// Idle iterations (~16 ms each) between auto-refresh dir re-scans.
// ~1.4 s — frequent enough that a new screenshot shows up promptly,
// rare enough to be negligible load.
const AUTO_REFRESH_TICKS: u32 = 90;

// Folders scanned per idle tick (~16 ms) for recursive size/count. Small
// enough that each tick stays snappy, enough that a typical directory
// fills in within a few hundred ms.
const STATS_PUMP_BUDGET: usize = 3;

const ACT_GRID_CLICK_BASE:    u32 = 1_000;
const ACT_GRID_HOVER_BASE:    u32 = 1_500;
const ACT_SIDEBAR_CLICK_BASE: u32 = 2_000;
const ACT_SIDEBAR_HOVER_BASE: u32 = 2_500;
const ACT_BREADCRUMB_BASE:    u32 = 3_000;
const ACT_TOOLBAR_BACK:       u32 = 4_000;
const ACT_TOOLBAR_FORWARD:    u32 = 4_001;
const ACT_TOOLBAR_UP:         u32 = 4_002;
const ACT_TOOLBAR_REFRESH:    u32 = 4_003;
// Menu-bar label clicks toggle the corresponding dropdown.
const ACT_MENU_FILE:          u32 = 5_000;
const ACT_MENU_EDIT:          u32 = 5_001;
const ACT_MENU_VIEW:          u32 = 5_002;
const ACT_MENU_GO:            u32 = 5_003;
const ACT_MENU_HELP:          u32 = 5_004;
// Click-outside-popover dismiss action.
const ACT_MENU_DISMISS:       u32 = 5_500;
// Dropdown items.
const ACT_FILE_QUIT:          u32 = 6_000;
const ACT_VIEW_GRID:          u32 = 6_100;
const ACT_VIEW_LIST:          u32 = 6_101;
const ACT_GO_HOME:            u32 = 6_200;
const ACT_GO_FILESYSTEM:      u32 = 6_201;
const ACT_HELP_ABOUT:         u32 = 6_300;
// List-view column headers — click to sort / toggle direction.
const ACT_HEADER_NAME:        u32 = 7_000;
const ACT_HEADER_SIZE:        u32 = 7_001;
const ACT_HEADER_FILES:       u32 = 7_002;
const ACT_HEADER_TYPE:        u32 = 7_003;
const ACT_HEADER_MTIME:       u32 = 7_004;

// File operations — shared by the Edit menu and the right-click context
// menu; both act on the current selection (`grid_sel`).
const ACT_EDIT_COPY:          u32 = 8_000;
const ACT_EDIT_CUT:           u32 = 8_001;
const ACT_EDIT_PASTE:         u32 = 8_002;
const ACT_EDIT_RENAME:        u32 = 8_003;
// Rename dialog buttons.
const ACT_RENAME_SUBMIT:      u32 = 8_100;
const ACT_RENAME_CANCEL:      u32 = 8_101;
// Click-outside dismiss for the right-click context menu.
const ACT_CTX_DISMISS:        u32 = 8_200;

// NodeId the context-menu Popover anchors against — placed on the
// selected item while the menu is open.
const NODE_CTX_ANCHOR: u32 = 200;

// NodeIds for menu-bar labels — used as Popover anchors.
const NODE_MENU_FILE: u32 = 100;
const NODE_MENU_EDIT: u32 = 101;
const NODE_MENU_VIEW: u32 = 102;
const NODE_MENU_GO:   u32 = 103;
const NODE_MENU_HELP: u32 = 104;

const GRID_COLS: usize = 4;
const QUERY_CAP: usize = 127;
const LIST_BUF_SIZE: usize = 128 * 1024;
static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];

const NAME_FETCH_CAP: usize = 64;
static mut NAME_BUF: [u8; NAME_FETCH_CAP] = [0; NAME_FETCH_CAP];

// ── State ─────────────────────────────────────────────────────────────

struct Place {
    label: String,
    icon:  IconId,
    path:  String,
}

struct Entry {
    name:    String,
    /// ASCII-lowercased mirror of `name`, computed once at parse
    /// time so refilter() doesn't allocate a fresh lowercase string
    /// on every keystroke. Critical for typing latency once the
    /// directory is large.
    name_lc: String,
    /// For files: the file's own byte size. For folders: the recursive
    /// sum of every descendant file's size, filled in by
    /// `annotate_folder_stats` on refresh (0 until then). Lets the list
    /// view show real folder sizes like Thunar/Finder.
    size:    u64,
    is_dir:  bool,
    /// Number of descendant files inside a folder (recursive). 0 for
    /// files. Drives the "Files" column. Filled progressively off the
    /// idle loop (see `pump_stats`).
    files:   u64,
    /// Folder whose recursive size/count hasn't been computed yet.
    /// True from refresh until `pump_stats` reaches it; always false for
    /// files. Rendered as "…" so the directory paints instantly and the
    /// numbers fill in without ever blocking the event loop.
    stats_pending: bool,
    /// UTC seconds since the Unix epoch, captured at write time by
    /// the kernel. Zero = unknown (RTC was unreadable when the entry
    /// was created). Filled in from the v3 `npk_fs_list` ABI tail.
    mtime:   u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Grid,
    List,
}

/// Which column the list view sorts by. Folders are always grouped
/// before files (Thunar/Files "folders first" idiom); the key only
/// orders entries *within* each group.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Name,
    Size,
    Files,
    Type,
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenMenu {
    File,
    Edit,
    View,
    Go,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipMode {
    /// Ctrl+C — paste keeps the source (content-addressed alias, cheap).
    Copy,
    /// Ctrl+X — paste moves the source (rename), then the clipboard clears.
    Cut,
}

/// A pending copy/cut, set by Ctrl+C/X and consumed by Ctrl+V (paste).
/// Holds the full source path + the bare name so paste can rebuild a
/// destination in the current directory.
struct Clip {
    full: String,
    name: String,
    mode: ClipMode,
}

struct Loft {
    current:        String,
    history:        Vec<String>,
    forward:        Vec<String>,
    sidebar:        Vec<Place>,
    /// Direct children of `current`. Used when the search query is
    /// empty (browse mode).
    entries:        Vec<Entry>,
    /// Recursive listing of `current` (every descendant). Loaded
    /// lazily on first non-empty query, cached until we navigate to
    /// a different directory. Search across the whole subtree —
    /// matches a Nautilus / Spotlight / VS-Code Quick Open pattern.
    recursive:      Vec<Entry>,
    /// `Some(path)` when `recursive` has been filled for that path
    /// in the current session; `None` after navigate() invalidates
    /// the cache. Lets refilter() decide "do I need to call
    /// `list_dir_recursive` again?".
    recursive_dir:  Option<String>,
    /// Indices into the active source list (entries / recursive)
    /// matching the current search query. Equal to 0..source.len()
    /// when the query is empty.
    filtered:       Vec<usize>,
    grid_sel:       Option<usize>,
    sidebar_sel:    Option<usize>,
    /// Pre-allocated (`String::with_capacity(QUERY_CAP + 1)`) so that
    /// `clear` + `push_str` stays inside the same heap block — bump
    /// allocator hands out the storage before `persistent_mark`, and
    /// `alloc_reset` between frames must not invalidate it.
    query:          String,
    /// Pre-allocated mirror used to compute `query.to_ascii_lowercase()`
    /// without an extra allocation per keystroke.
    query_lc:       String,
    /// Grid (Pictures-style icons) vs List (table with name + size +
    /// type + modified). Switched via the View menu dropdown.
    view_mode:      ViewMode,
    /// Which menu's dropdown is currently visible. None = no menu open.
    open_menu:      Option<OpenMenu>,
    /// File associations (extension → app module), loaded once from
    /// `sys/config/associations`. Checked before the built-in defaults.
    assoc:          Vec<(String, String)>,
    /// Cheap signature (count + name/size fold) of the current dir's
    /// listing. The idle loop re-lists periodically and auto-refreshes
    /// when this changes — so a new file (e.g. a fresh screenshot) shows
    /// up without a manual refresh.
    dir_sig:        u64,
    /// Active list-view sort column + direction. Clicking a header sets
    /// the key (or toggles direction if it's already active) and re-sorts
    /// `entries` in place.
    sort_key:       SortKey,
    sort_asc:       bool,
    /// Indices into `entries` of folders still awaiting a recursive
    /// size/count scan. Drained a few at a time off the idle loop
    /// (`pump_stats`) so opening a directory never blocks on a deep tree.
    stats_queue:    Vec<usize>,
    /// Reusable path buffer for `pump_stats` — pre-allocated before the
    /// persistent mark so building "current/sub" each tick allocates
    /// nothing on the hot path.
    scratch:        String,
    /// Pending copy/cut awaiting a paste. None = clipboard empty.
    clipboard:      Option<Clip>,
    /// True while the right-click context menu popover is showing.
    ctx_open:       bool,
    /// True while the rename dialog is showing. `rename_buf` holds the
    /// edited name and `rename_old` the source's full path (captured when
    /// the dialog opened, so a later selection change can't misdirect it).
    rename_open:    bool,
    rename_old:     String,
    /// Pre-allocated (like `query`) so `clear` + `push_str` on every
    /// InputChange stays inside the same heap block across `alloc_reset`.
    rename_buf:     String,
}

impl Loft {
    fn new() -> Self {
        let home = read_home_dir();
        let sidebar = filter_sidebar_to_existing(default_sidebar(&home));
        let mut lf = Loft {
            current:       home,
            history:       Vec::new(),
            forward:       Vec::new(),
            sidebar,
            entries:       Vec::new(),
            recursive:     Vec::new(),
            recursive_dir: None,
            filtered:      Vec::with_capacity(64),
            grid_sel:      None,
            sidebar_sel:   Some(0),
            query:         String::with_capacity(QUERY_CAP + 1),
            query_lc:      String::with_capacity(QUERY_CAP + 1),
            view_mode:     ViewMode::List,
            open_menu:     None,
            assoc:         load_associations(),
            dir_sig:       0,
            sort_key:      SortKey::Name,
            sort_asc:      true,
            stats_queue:   Vec::new(),
            scratch:       String::with_capacity(512),
            clipboard:     None,
            ctx_open:      false,
            rename_open:   false,
            rename_old:    String::new(),
            rename_buf:    String::with_capacity(256),
        };
        lf.refresh();
        lf
    }

    /// Resolve the handler app for a file name via its extension:
    /// `sys/config/associations` overrides first, then built-in defaults.
    /// Returns None for unknown types (loft does nothing on open).
    fn associated_app(&self, name: &str) -> Option<String> {
        let ext = match name.rsplit_once('.') {
            Some((_, e)) if !e.is_empty() => e.to_ascii_lowercase(),
            _ => return None,
        };
        if let Some((_, app)) = self.assoc.iter().find(|(k, _)| *k == ext) {
            return Some(app.clone());
        }
        match ext.as_str() {
            "md" | "markdown" | "txt" | "text" | "rs" | "toml" | "json"
            | "log" | "conf" | "ini" | "cfg" | "sh" | "csv" | "yaml" | "yml"
            | "xml" | "html" | "htm" | "c" | "h" | "py" | "js" | "ts"
                => Some("spell".to_string()),
            "png" => Some("iris".to_string()),
            _ => None,
        }
    }

    fn refresh(&mut self) {
        self.entries = list_dir(&self.current);
        // Signature is captured from the raw (un-annotated) listing so it
        // matches the idle probe's `dir_signature(&list_dir(..))` — folder
        // sizes are filled in below and must not perturb change-detection.
        self.dir_sig = dir_signature(&self.entries);
        // Mark folders "pending" and queue them for a recursive
        // size/count scan — the actual scanning happens incrementally off
        // the idle loop (`pump_stats`) so the directory paints instantly
        // even when it holds many/large subtrees. Order by the active
        // column first (pending folders all read size 0 → tie on name).
        init_folder_stats(&mut self.entries);
        sort_entries(&mut self.entries, self.sort_key, self.sort_asc);
        self.stats_queue = pending_folder_indices(&self.entries);
        // Navigation invalidates any cached recursive listing — the
        // next non-empty query for this directory triggers a fresh
        // `list_dir_recursive` call.
        self.recursive.clear();
        self.recursive_dir = None;
        self.refilter();
        self.sync_sidebar_from_current();
    }

    /// Re-sort the browse listing under a (possibly new) column. Clicking
    /// the already-active column flips direction; a different column
    /// switches to it with a sensible default direction (ascending for
    /// text, descending for the numeric/date columns where "biggest /
    /// newest first" is the usual intent).
    fn set_sort(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_key = key;
            self.sort_asc = matches!(key, SortKey::Name | SortKey::Type);
        }
        sort_entries(&mut self.entries, self.sort_key, self.sort_asc);
        self.refilter();
    }

    /// Compute the recursive size + file count for up to `budget` queued
    /// folders, one host scan per folder. Runs off the idle loop so a
    /// directory with many or deep subfolders fills in progressively
    /// instead of freezing the app on open. Returns true if anything
    /// changed (→ caller re-renders). Caller must wrap this between
    /// `alloc_reset(persistent_mark)` / re-capture like the other state
    /// mutations so `filtered`/`scratch` growth lands in the kept region.
    fn pump_stats(&mut self, budget: usize) -> bool {
        if self.stats_queue.is_empty() { return false; }
        let mut changed = false;
        for _ in 0..budget {
            let Some(idx) = self.stats_queue.pop() else { break };
            if self.entries.get(idx).map(|e| e.is_dir) != Some(true) { continue; }
            // Build "current/<name>" into the reusable scratch buffer
            // (disjoint field borrows — no per-tick allocation).
            self.scratch.clear();
            self.scratch.push_str(&self.current);
            if !self.current.is_empty() { self.scratch.push('/'); }
            self.scratch.push_str(&self.entries[idx].name);
            let (bytes, files) = scan_folder_stats(&self.scratch);
            if let Some(e) = self.entries.get_mut(idx) {
                e.size = bytes;
                e.files = files;
                e.stats_pending = false;
            }
            changed = true;
        }
        // Settle the order once every folder is in, but only when the
        // active column actually depends on the numbers we just filled.
        if self.stats_queue.is_empty()
            && matches!(self.sort_key, SortKey::Size | SortKey::Files)
        {
            sort_entries(&mut self.entries, self.sort_key, self.sort_asc);
            self.refilter();
        }
        changed
    }

    /// Pick the active source list for filtering — direct children
    /// when the query is empty (browse mode), recursive descendants
    /// otherwise (search mode). Lazy-loads the recursive listing on
    /// first non-empty query for the current directory.
    fn ensure_search_source(&mut self) -> bool {
        if self.query.is_empty() { return false; }
        if self.recursive_dir.as_deref() == Some(self.current.as_str()) {
            return true;
        }
        log("[loft] loading recursive listing");
        self.recursive = list_dir_recursive(&self.current);
        self.recursive_dir = Some(self.current.clone());
        true
    }

    fn refilter(&mut self) {
        let recursive_mode = self.ensure_search_source();
        self.filtered.clear();
        if self.query.is_empty() {
            for i in 0..self.entries.len() { self.filtered.push(i); }
        } else {
            // Reuse the pre-mark buffer for the lowercased query.
            self.query_lc.clear();
            for ch in self.query.chars() {
                self.query_lc.push(ch.to_ascii_lowercase());
            }
            let source: &Vec<Entry> = if recursive_mode { &self.recursive } else { &self.entries };
            for (i, e) in source.iter().enumerate() {
                if e.name_lc.contains(self.query_lc.as_str()) {
                    self.filtered.push(i);
                }
            }
        }
        self.grid_sel = if self.filtered.is_empty() { None } else { Some(0) };
    }

    /// Source list paired with `filtered` — entries when browsing,
    /// recursive when searching. Renderer + open_selected use this
    /// instead of always going through `entries`.
    fn source(&self) -> &Vec<Entry> {
        if self.query.is_empty() || self.recursive_dir.is_none() {
            &self.entries
        } else {
            &self.recursive
        }
    }

    fn sync_sidebar_from_current(&mut self) {
        self.sidebar_sel = None;
        for (i, p) in self.sidebar.iter().enumerate() {
            if p.path == self.current { self.sidebar_sel = Some(i); break; }
        }
    }

    fn navigate(&mut self, new_path: String) {
        if new_path == self.current { return; }
        self.history.push(self.current.clone());
        self.forward.clear();
        self.current = new_path;
        // Navigation clears the search filter — entering a fresh
        // directory should show its full contents, not an empty view.
        self.query.clear();
        self.refresh();
    }

    fn go_back(&mut self) {
        if let Some(p) = self.history.pop() {
            self.forward.push(self.current.clone());
            self.current = p;
            self.query.clear();
            self.refresh();
        }
    }

    fn go_forward(&mut self) {
        if let Some(p) = self.forward.pop() {
            self.history.push(self.current.clone());
            self.current = p;
            self.query.clear();
            self.refresh();
        }
    }

    fn go_up(&mut self) {
        let parent = parent_path(&self.current);
        if parent != self.current { self.navigate(parent); }
    }

    fn open_selected(&mut self) {
        let Some(i) = self.grid_sel else { return; };
        let Some(&entry_idx) = self.filtered.get(i) else { return; };
        // In search mode `source()` returns the recursive list, so
        // `entry.name` is a relative path like "wallpapers/aurora"
        // — the same join below gives the correct absolute target.
        let (is_dir, name) = match self.source().get(entry_idx) {
            Some(e) => (e.is_dir, e.name.clone()),
            None => return,
        };
        if is_dir {
            let next = if self.current.is_empty() {
                name
            } else {
                alloc::format!("{}/{}", self.current, name)
            };
            self.navigate(next);
        } else if let Some(app) = self.associated_app(&name) {
            // Open the file with its associated app (file association).
            let full = if self.current.is_empty() {
                name
            } else {
                alloc::format!("{}/{}", self.current, name)
            };
            unsafe {
                npk_open(app.as_ptr() as i32, app.len() as i32,
                         full.as_ptr() as i32, full.len() as i32);
            }
        } else {
            log(s().no_handler);
        }
    }

    fn select_delta_y(&mut self, dy: isize) {
        self.move_selection(dy * GRID_COLS as isize);
    }

    fn select_delta_x(&mut self, dx: isize) {
        self.move_selection(dx);
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() { self.grid_sel = None; return; }
        let cur = self.grid_sel.unwrap_or(0) as isize;
        let mut next = cur + delta;
        let max = self.filtered.len() as isize - 1;
        if next < 0 { next = 0; }
        if next > max { next = max; }
        self.grid_sel = Some(next as usize);
    }

    // ── File operations ───────────────────────────────────────────────

    /// The selected entry's name (in search mode a relative sub-path) and
    /// dir flag, or None if nothing is selected.
    fn selected(&self) -> Option<(String, bool)> {
        let i = self.grid_sel?;
        let &entry_idx = self.filtered.get(i)?;
        let e = self.source().get(entry_idx)?;
        Some((e.name.clone(), e.is_dir))
    }

    /// True if a paste is possible (clipboard holds something).
    fn can_paste(&self) -> bool { self.clipboard.is_some() }

    /// Ctrl+C — remember the selection for a keep-source paste.
    fn do_copy(&mut self) {
        if let Some((name, _)) = self.selected() {
            self.clipboard = Some(Clip {
                full: join(&self.current, &name),
                name: basename(&name).to_string(),
                mode: ClipMode::Copy,
            });
        }
    }

    /// Ctrl+X — remember the selection for a move paste.
    fn do_cut(&mut self) {
        if let Some((name, _)) = self.selected() {
            self.clipboard = Some(Clip {
                full: join(&self.current, &name),
                name: basename(&name).to_string(),
                mode: ClipMode::Cut,
            });
        }
    }

    /// Ctrl+V — copy or move the clipboard source into the current dir,
    /// picking a collision-free name. A Cut clears the clipboard on success.
    fn do_paste(&mut self) {
        let (src, name, mode) = match self.clipboard.as_ref() {
            Some(c) => (c.full.clone(), c.name.clone(), c.mode),
            None => return,
        };
        let dest = self.unique_dest(&name);
        let rc = match mode {
            ClipMode::Copy => unsafe {
                npk_fs_copy(src.as_ptr() as i32, src.len() as i32,
                            dest.as_ptr() as i32, dest.len() as i32)
            },
            ClipMode::Cut => unsafe {
                npk_fs_rename(src.as_ptr() as i32, src.len() as i32,
                              dest.as_ptr() as i32, dest.len() as i32)
            },
        };
        if rc == 0 {
            if mode == ClipMode::Cut { self.clipboard = None; }
            self.refresh();
        } else {
            log("[loft] paste failed");
        }
    }

    /// Open the rename dialog pre-filled with the selection's name.
    fn open_rename(&mut self) {
        if let Some((name, _)) = self.selected() {
            let base = basename(&name).to_string();
            self.rename_old = join(&self.current, &name);
            self.rename_buf.clear();
            // Stay within the pre-allocated capacity so the InputChange
            // mirror never reallocates (bump-heap discipline).
            let max = self.rename_buf.capacity().min(base.len());
            self.rename_buf.push_str(&base[..max]);
            self.rename_open = true;
            self.open_menu = None;
            self.ctx_open = false;
        }
    }

    /// Commit the rename dialog: move `rename_old` → current/<new name>.
    fn commit_rename(&mut self) {
        let new = self.rename_buf.trim();
        // Reject empty / path-bearing names — rename stays in-place.
        if new.is_empty() || new.contains('/') {
            self.rename_open = false;
            return;
        }
        let old = self.rename_old.clone();
        let dest = join(&self.current, new);
        self.rename_open = false;
        if old == dest { return; }
        let rc = unsafe {
            npk_fs_rename(old.as_ptr() as i32, old.len() as i32,
                          dest.as_ptr() as i32, dest.len() as i32)
        };
        if rc == 0 { self.refresh(); } else { log("[loft] rename failed"); }
    }

    /// A collision-free destination for `name` in the current directory.
    fn unique_dest(&self, name: &str) -> String {
        unique_in(&self.current, name)
    }
}

// ── Render ────────────────────────────────────────────────────────────

fn render(lf: &Loft) -> Widget {
    let menu = render_menu_bar();
    let toolbar = render_toolbar(lf);
    // The rename dialog replaces the file area while it's up (same idiom
    // as spell's "save as" dialog) so its Input is the only editable field.
    let body = if lf.rename_open { render_rename_dialog(lf) } else { render_body(lf) };

    // Custom outer column instead of `prefab::panel`: panel's
    // Padding-Xs + Spacing-Md kept the menu-bar bg from reaching
    // the window edges + put a 12 px gap between menu and divider.
    // Loft wants the menu strip + sidebar fill to be flush —
    // file-manager idiom (Thunar / Files / Finder all do this).
    // Footer removed (noise) — the body fills to the bottom edge.
    let mut children: Vec<Widget> = alloc::vec![
        menu,
        Widget::Divider,
        toolbar,
        Widget::Divider,
        body,                           // Modifier::Flex(1) — fills
    ];

    // Append the open menu's dropdown as a Popover. The compositor
    // resolves `anchor` against the matching menu-label NodeId
    // (recorded during layout) and floats the dropdown directly
    // below it. Click outside fires `on_dismiss = ACT_MENU_DISMISS`
    // which we route to clearing `open_menu`.
    if let Some(kind) = lf.open_menu {
        let (anchor_id, content) = render_dropdown(lf, kind);
        children.push(Widget::Popover {
            anchor:     NodeId(anchor_id),
            child:      alloc::boxed::Box::new(content),
            on_dismiss: ActionId(ACT_MENU_DISMISS),
            modifiers:  alloc::vec![],
        });
    } else if lf.ctx_open {
        // Right-click context menu, floated at the selected item (which
        // carries NODE_CTX_ANCHOR while the menu is open).
        children.push(Widget::Popover {
            anchor:     NodeId(NODE_CTX_ANCHOR),
            child:      alloc::boxed::Box::new(prefab::popover_menu(&file_op_items(lf), None)),
            on_dismiss: ActionId(ACT_CTX_DISMISS),
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

/// Copy / Cut / Paste / Rename items, shared by the Edit menu and the
/// right-click context menu. Entries appear only when meaningful — Copy /
/// Cut / Rename need a selection, Paste needs a filled clipboard.
fn file_op_items(lf: &Loft) -> Vec<(String, ActionId)> {
    let mut items: Vec<(String, ActionId)> = Vec::new();
    let has_sel = lf.selected().is_some();
    if has_sel {
        items.push((s().copy.to_string(), ActionId(ACT_EDIT_COPY)));
        items.push((s().cut.to_string(), ActionId(ACT_EDIT_CUT)));
    }
    if lf.can_paste() {
        items.push((s().paste.to_string(), ActionId(ACT_EDIT_PASTE)));
    }
    if has_sel {
        items.push((s().rename.to_string(), ActionId(ACT_EDIT_RENAME)));
    }
    if items.is_empty() {
        // Keep the surface non-empty so the click still reads as handled.
        items.push((s().no_action.to_string(), ActionId(ACT_MENU_DISMISS)));
    }
    items
}

/// Wrap a grid/list item so the context-menu Popover has an anchor rect.
/// Applied only to the selected item while the menu is open, so normal
/// rendering is untouched.
fn ctx_anchor_wrap(child: Widget) -> Widget {
    Widget::Column {
        children:  alloc::vec![child],
        spacing:   0,
        align:     Align::Stretch,
        modifiers: alloc::vec![Modifier::NodeId(NodeId(NODE_CTX_ANCHOR))],
    }
}

/// Modal rename dialog — mirrors spell's name dialog. Focus doesn't
/// auto-jump on a re-commit, so the field must be clicked before typing
/// (the footer hint says so); Enter commits, Esc cancels.
fn render_rename_dialog(lf: &Loft) -> Widget {
    let card = prefab::dialog(
        s().rename_title,
        Widget::Column {
            children: alloc::vec![
                Widget::Text {
                    content:   s().rename_label.to_string(),
                    style:     TextStyle::Muted,
                    modifiers: alloc::vec![],
                },
                prefab::input(&lf.rename_buf, "name", prefab::InputKind::Text,
                              ActionId(ACT_RENAME_SUBMIT), None),
                Widget::Row {
                    children: alloc::vec![
                        Widget::Spacer { flex: 1 },
                        prefab::button(s().cancel, prefab::ButtonStyle::Ghost, ActionId(ACT_RENAME_CANCEL)),
                        prefab::button(s().confirm_rename, prefab::ButtonStyle::Primary, ActionId(ACT_RENAME_SUBMIT)),
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
        Some(s().rename_hint),
        360,
    );
    Widget::Column {
        children:  alloc::vec![Widget::Spacer { flex: 1 }, card, Widget::Spacer { flex: 1 }],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Flex(1), Modifier::Padding(Padding::Lg.as_u16())],
    }
}

fn render_menu_bar() -> Widget {
    let labels: Vec<(String, ActionId)> = alloc::vec![
        (s().menu_file.to_string(), ActionId(ACT_MENU_FILE)),
        (s().menu_edit.to_string(), ActionId(ACT_MENU_EDIT)),
        (s().menu_view.to_string(), ActionId(ACT_MENU_VIEW)),
        (s().menu_go.to_string(), ActionId(ACT_MENU_GO)),
        (s().menu_help.to_string(), ActionId(ACT_MENU_HELP)),
    ];
    let anchors: Vec<NodeId> = alloc::vec![
        NodeId(NODE_MENU_FILE),
        NodeId(NODE_MENU_EDIT),
        NodeId(NODE_MENU_VIEW),
        NodeId(NODE_MENU_GO),
        NodeId(NODE_MENU_HELP),
    ];
    prefab::menu_bar_with_icon(IconId::Folders, &labels, &anchors)
}

/// Build the dropdown for the currently-open menu. Returns
/// `(anchor_node_id, content_widget)` so the caller can wrap the
/// content in a `Widget::Popover` against the matching menu label.
fn render_dropdown(lf: &Loft, kind: OpenMenu) -> (u32, Widget) {
    match kind {
        OpenMenu::File => (
            NODE_MENU_FILE,
            prefab::popover_menu(&[
                (s().quit.to_string(), ActionId(ACT_FILE_QUIT)),
            ], None),
        ),
        OpenMenu::Edit => (
            NODE_MENU_EDIT,
            prefab::popover_menu(&file_op_items(lf), None),
        ),
        OpenMenu::View => (
            NODE_MENU_VIEW,
            prefab::popover_menu(&[
                (s().view_grid.to_string(), ActionId(ACT_VIEW_GRID)),
                (s().view_list.to_string(), ActionId(ACT_VIEW_LIST)),
            ], Some(match lf.view_mode {
                ViewMode::Grid => 0,
                ViewMode::List => 1,
            })),
        ),
        OpenMenu::Go => (
            NODE_MENU_GO,
            prefab::popover_menu(&[
                (s().go_home.to_string(), ActionId(ACT_GO_HOME)),
                (s().go_filesystem.to_string(), ActionId(ACT_GO_FILESYSTEM)),
            ], None),
        ),
        OpenMenu::Help => (
            NODE_MENU_HELP,
            prefab::popover_menu(&[
                (s().about.to_string(), ActionId(ACT_HELP_ABOUT)),
            ], None),
        ),
    }
}

fn render_toolbar(lf: &Loft) -> Widget {
    let crumbs = breadcrumb_for(&lf.current);
    let search = search_input(&lf.query);
    Widget::Row {
        children: alloc::vec![
            prefab::icon_button(IconId::ArrowLeft,      24, Some(ActionId(ACT_TOOLBAR_BACK)),    None),
            prefab::icon_button(IconId::ArrowRight,     24, Some(ActionId(ACT_TOOLBAR_FORWARD)), None),
            prefab::icon_button(IconId::ArrowUp,        24, Some(ActionId(ACT_TOOLBAR_UP)),      None),
            prefab::icon_button(IconId::ArrowClockwise, 24, Some(ActionId(ACT_TOOLBAR_REFRESH)), None),
            crumbs,
            Widget::Spacer { flex: 1 },
            search,
        ],
        spacing: Spacing::Sm.as_u16(),
        align:   Align::Center,
        // Own padding now that the outer Column is flush — keeps
        // back/forward/breadcrumbs + search bar off the chrome.
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

/// Hand-rolled search input with always-visible chrome — `prefab::input`
/// blends with the panel by design (drun's launcher look), but loft's
/// toolbar wants the search bar to read as a discrete, framed widget
/// matching the v3 mockup. Same magnifier prefix + Heading text +
/// focus-accent border, plus a baseline `SurfaceMuted` fill and a
/// `Border` stroke that's visible without focus too.
fn search_input(query: &str) -> Widget {
    let raw = Widget::Input {
        value:       query.to_string(),
        placeholder: s().search.to_string(),
        on_submit:   prefab::NO_ACTION,
        modifiers:   alloc::vec![],
    };
    Widget::Row {
        children: alloc::vec![
            Widget::Icon {
                id:        IconId::MagnifyingGlass,
                // 24 = atlas-native size; 18 scaled down from the 24 px
                // slot looked visibly fuzzy. Same fix as `prefab::input`.
                size:      24,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
            },
            raw,
            Widget::Spacer { flex: 1 },
            kbd("⌘F"),
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
            Modifier::MinWidth(230),
            // 1 px accent border plus a 3 px ring, per the design's
            // `text_field` focus state (UI_REFRESH.md §3).
            Modifier::Focus(alloc::vec![
                Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
                Modifier::Ring { token: Token::AccentRing, width: 3 },
            ]),
        ],
    }
}

/// Keyboard-shortcut badge — a small mono chip on `SurfaceHover`.
fn kbd(keys: &str) -> Widget {
    Widget::Text {
        content:   keys.to_string(),
        style:     TextStyle::Mono,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Xxs.as_u16()),
            Modifier::Background(Token::SurfaceHover),
            Modifier::Rounded(Radius::Sm.as_u8()),
            Modifier::Tint(Token::OnSurfaceFaint),
        ],
    }
}

/// Disk fill level at the foot of the sidebar: a slim accent-filled
/// track plus the percentage. Hidden when the kernel reports no
/// mounted filesystem.
fn storage_meter() -> Option<Widget> {
    let packed = unsafe { npk_fs_usage() };
    if packed < 0 { return None; }
    let used  = ((packed as u64) >> 32) as u64;
    let total = (packed as u64) & 0xFFFF_FFFF;
    if total == 0 { return None; }
    let pct = ((used.saturating_mul(100)) / total).min(100) as u16;

    // The track is a fixed-width bar; the fill is the same bar clipped
    // to `pct` of that width, laid over it in a Stack.
    const TRACK_W: u16 = 108;
    let fill_w = ((TRACK_W as u32 * pct as u32) / 100).max(1) as u16;
    let mut pct_str = String::with_capacity(5);
    push_usize(&mut pct_str, pct as usize);
    pct_str.push('%');

    Some(Widget::Row {
        children: alloc::vec![
            Widget::Stack {
                children: alloc::vec![
                    prefab::mark(TRACK_W, 4, Some(Token::Border)),
                    prefab::mark(fill_w,  4, Some(Token::Accent)),
                ],
                modifiers: Vec::new(),
            },
            Widget::Spacer { flex: 1 },
            Widget::Text {
                content:   pct_str,
                style:     TextStyle::Mono,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceFaint)],
            },
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::Background(Token::SurfaceHover),
            Modifier::Rounded(Radius::Sm.as_u8()),
        ],
    })
}

fn render_body(lf: &Loft) -> Widget {
    // Sidebar — PLACES (Home/Documents/Downloads/Pictures/Projects)
    // + DEVICES (Filesystem/Trash) per the mockup. `nav_row`
    // selected-state lights up when the current dir matches a
    // sidebar path verbatim.
    let mut places_rows: Vec<Widget> = Vec::new();
    let mut devices_rows: Vec<Widget> = Vec::new();
    for (i, p) in lf.sidebar.iter().enumerate() {
        let selected = lf.sidebar_sel == Some(i);
        let row = prefab::nav_row(
            p.icon, &p.label, selected,
            Some(ActionId(ACT_SIDEBAR_CLICK_BASE + i as u32)),
            Some(ActionId(ACT_SIDEBAR_HOVER_BASE + i as u32)),
        );
        if is_device(&p.label) { devices_rows.push(row); }
        else { places_rows.push(row); }
    }
    let mut pane: Vec<Widget> = alloc::vec![
        prefab::sidebar_section("PLACES",  places_rows),
        prefab::sidebar_section("DEVICES", devices_rows),
    ];
    // Capacity meter pinned to the foot of the pane.
    if let Some(meter) = storage_meter() {
        pane.push(Widget::Spacer { flex: 1 });
        pane.push(meter);
    }
    let sidebar = prefab::sidebar_pane(pane);
    // Also scroll the sidebar so a long PLACES/DEVICES list can never push
    // the footer off-screen either; its overlay bar only appears if it
    // actually overflows (usually it doesn't).
    let sidebar = Widget::Scroll {
        child:     alloc::boxed::Box::new(sidebar),
        axis:      Axis::Vertical,
        modifiers: alloc::vec![],
    };

    // Content — filtered grid OR list, plus two empty states
    // (genuinely empty directory vs. nothing matched the search).
    let content: Widget = if lf.filtered.is_empty() {
        let hint = if lf.query.is_empty() {
            s().empty_dir
        } else {
            s().no_matches
        };
        prefab::empty_state(hint)
    } else {
        match lf.view_mode {
            ViewMode::Grid => render_grid(lf),
            ViewMode::List => render_list(lf),
        }
    };

    // Wrap the file area in a vertical Scroll so a long listing scrolls
    // (mouse wheel) and is clipped to the body instead of overflowing and
    // pushing the footer off-screen in a small (¼-screen) window. The
    // overlay scrollbar only shows when the content actually overflows.
    let content = Widget::Scroll {
        child:     alloc::boxed::Box::new(content),
        axis:      Axis::Vertical,
        modifiers: alloc::vec![Modifier::Flex(1)],
    };

    Widget::Row {
        children: alloc::vec![sidebar, content],
        spacing: 0,
        align:   Align::Stretch,
        // Flex(1) makes the body absorb all leftover vertical space in
        // the parent Column. Sidebar inherits via Stretch align so its
        // SurfaceMuted bg now reaches the footer divider regardless of
        // grid content height. Without this the body is intrinsic-sized
        // and the bg ends where its tallest child does.
        modifiers: alloc::vec![Modifier::Flex(1)],
    }
}

fn render_grid(lf: &Loft) -> Widget {
    // `source()` gives us either direct children (browse) or
    // recursive descendants (search) — `filtered` indexes into
    // whichever is active. Recursive entries already carry their
    // sub-path in `name` so the grid label reads "wallpapers/aurora"
    // for a search hit, which is the desired "show me where the
    // match lives" UX.
    let source = lf.source();
    let grid_children: Vec<Widget> = lf.filtered.iter().enumerate().map(|(ui_idx, &entry_idx)| {
        let e = &source[entry_idx];
        let icon = icon_for(e);
        let selected = lf.grid_sel == Some(ui_idx);
        let item = prefab::grid_item(
            icon, &e.name,
            selected,
            Some(ActionId(ACT_GRID_CLICK_BASE + ui_idx as u32)),
            Some(ActionId(ACT_GRID_HOVER_BASE + ui_idx as u32)),
        );
        if lf.ctx_open && selected { ctx_anchor_wrap(item) } else { item }
    }).collect();
    prefab::grid(grid_children, GRID_COLS)
}

/// Detail-list view: one row per entry, columns Name | Size | Files |
/// Type | Modified, spanning the full window width (Name flexes to fill
/// the slack). Headers are clickable — a click sorts by that column,
/// clicking the active column flips direction (▲/▼ marker). English
/// headers (Florian's request — international FS UX).
fn render_list(lf: &Loft) -> Widget {
    let source = lf.source();
    // Folder size/count is only computed for the browse listing; in
    // search mode (recursive source) folders show "—" rather than a
    // misleading zero.
    let browsing = lf.query.is_empty();
    let mut rows: Vec<Widget> = Vec::with_capacity(lf.filtered.len() + 1);
    rows.push(list_header_row(lf));
    rows.push(Widget::Divider);
    for (ui_idx, &entry_idx) in lf.filtered.iter().enumerate() {
        let e = &source[entry_idx];
        let selected = lf.grid_sel == Some(ui_idx);
        let row = list_data_row(
            e, selected, browsing,
            ActionId(ACT_GRID_CLICK_BASE + ui_idx as u32),
            ActionId(ACT_GRID_HOVER_BASE + ui_idx as u32),
        );
        rows.push(if lf.ctx_open && selected { ctx_anchor_wrap(row) } else { row });
    }
    Widget::Column {
        children: rows,
        spacing: 0,
        align:   Align::Stretch,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

fn list_header_row(lf: &Loft) -> Widget {
    Widget::Row {
        children: alloc::vec![
            header_cell(lf, s().col_name,     SortKey::Name,     ACT_HEADER_NAME,  COL_NAME_W,  true),
            header_cell(lf, s().col_size,     SortKey::Size,     ACT_HEADER_SIZE,  COL_SIZE_W,  false),
            header_cell(lf, s().col_files,    SortKey::Files,    ACT_HEADER_FILES, COL_FILES_W, false),
            header_cell(lf, s().col_type,     SortKey::Type,     ACT_HEADER_TYPE,  COL_TYPE_W,  false),
            header_cell(lf, s().col_modified, SortKey::Modified, ACT_HEADER_MTIME, COL_MTIME_W, false),
        ],
        spacing: Spacing::Md.as_u16(),
        align:   Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::MinHeight(HEADER_ROW_H),
        ],
    }
}

/// One clickable column header. Mono + faint, so the header band reads
/// as structure rather than as another row of data (UI_REFRESH.md §5).
/// Appends a ↑/↓ marker on the active sort column; `flex` lets the Name
/// header grow to fill the row.
fn header_cell(lf: &Loft, label: &str, key: SortKey, action: u32,
               min_w: u16, flex: bool) -> Widget {
    let mut content = String::from(label);
    let active = lf.sort_key == key;
    if active {
        content.push(' ');
        content.push(if lf.sort_asc { '↑' } else { '↓' });
    }
    let mut mods: Vec<Modifier> = alloc::vec![
        Modifier::MinWidth(min_w),
        Modifier::Tint(if active { Token::OnSurfaceMuted } else { Token::OnSurfaceFaint }),
        Modifier::OnClick(ActionId(action)),
        Modifier::Hover(alloc::vec![
            Modifier::Tint(Token::OnSurface),
        ]),
    ];
    if flex { mods.push(Modifier::Flex(1)); }
    Widget::Text { content, style: TextStyle::Mono, modifiers: mods }
}

fn list_data_row(e: &Entry, selected: bool, browsing: bool,
                 on_click: ActionId, on_hover: ActionId) -> Widget {
    let icon = icon_for(e);
    // Name cell with icon + label. Flex(1) so the column absorbs the
    // row's slack and the fixed columns sit flush against the right edge.
    let name_cell = Widget::Row {
        children: alloc::vec![
            Widget::Icon {
                id: icon,
                size: 24,
                modifiers: alloc::vec![Modifier::Tint(
                    if selected { Token::Accent } else { Token::OnSurfaceMuted },
                )],
            },
            Widget::Text {
                content:   e.name.clone(),
                style:     TextStyle::Body,
                modifiers: alloc::vec![],
            },
        ],
        spacing: Spacing::Sm.as_u16(),
        align:   Align::Center,
        modifiers: alloc::vec![Modifier::MinWidth(COL_NAME_W), Modifier::Flex(1)],
    };
    // Folders show their recursive byte sum + file count once scanned
    // ("…" while pending, "—" in search mode where we don't compute it);
    // files show their own size and "—" in the Files column.
    let size_str = if e.is_dir {
        if !browsing { "—".to_string() }
        else if e.stats_pending { "…".to_string() }
        else { format_size(e.size) }
    } else {
        format_size(e.size)
    };
    let files_str = if e.is_dir {
        if !browsing { "—".to_string() }
        else if e.stats_pending { "…".to_string() }
        else {
            let mut s = String::with_capacity(8);
            push_usize(&mut s, e.files as usize);
            s
        }
    } else {
        "—".to_string()
    };
    let type_str   = type_for(e);
    let mtime_str  = format_mtime(e.mtime);
    // Selection reads as a tint plus a 2 px accent edge on the leading
    // side — never a boxed-in row (UI_REFRESH.md §3 `list_row`). The
    // edge occupies its space on every row so nothing shifts sideways
    // when the selection moves.
    let mut row_mods: Vec<Modifier> = alloc::vec![
        Modifier::Padding(Padding::Xs.as_u16()),
        Modifier::MinHeight(DATA_ROW_H),
        Modifier::OnClick(on_click),
        Modifier::OnHover(on_hover),
        Modifier::Hover(alloc::vec![
            Modifier::Background(Token::SurfaceHover),
        ]),
    ];
    if selected {
        row_mods.push(Modifier::Background(Token::AccentMuted));
    }
    let edge = prefab::mark(2, DATA_ROW_H,
        if selected { Some(Token::Accent) } else { None });

    Widget::Row {
        children: alloc::vec![
            edge,
            name_cell,
            list_cell_text(&size_str,  COL_SIZE_W),
            list_cell_text(&files_str, COL_FILES_W),
            list_cell_text(&type_str,  COL_TYPE_W),
            list_cell_text(&mtime_str, COL_MTIME_W),
        ],
        spacing: Spacing::Md.as_u16(),
        align:   Align::Center,
        modifiers: row_mods,
    }
}

/// A metadata column. Mono + faint so the eye runs down the file names
/// and only lands on the numbers when it goes looking for them.
fn list_cell_text(text: &str, min_w: u16) -> Widget {
    Widget::Text {
        content: text.to_string(),
        style:   TextStyle::Mono,
        modifiers: alloc::vec![
            Modifier::MinWidth(min_w),
            Modifier::Tint(Token::OnSurfaceMuted),
        ],
    }
}

const HEADER_ROW_H: u16 = 30;
const DATA_ROW_H:   u16 = 34;

const COL_NAME_W:  u16 = 240;   // min — flexes to fill the row
const COL_SIZE_W:  u16 = 110;
const COL_FILES_W: u16 = 90;
const COL_TYPE_W:  u16 = 120;
const COL_MTIME_W: u16 = 170;

fn breadcrumb_for(path: &str) -> Widget {
    let mut segs: Vec<(String, ActionId)> = Vec::new();
    let mut acc = String::new();
    if path.is_empty() {
        segs.push(("/".to_string(), ActionId(ACT_BREADCRUMB_BASE)));
    } else {
        for (i, part) in path.split('/').enumerate() {
            if part.is_empty() { continue; }
            if !acc.is_empty() { acc.push('/'); }
            acc.push_str(part);
            let _ = i;
            // Each segment fires the same action base + segment count
            // so the dispatcher can rebuild the prefix from the path.
            // Simpler than embedding the path bytes in the ActionId.
            segs.push((part.to_string(),
                       ActionId(ACT_BREADCRUMB_BASE + segs.len() as u32 + 1)));
        }
    }
    prefab::breadcrumb(&segs)
}

// ── Event dispatch ────────────────────────────────────────────────────

enum Outcome { Idle, Rerender, Exit }

fn handle(lf: &mut Loft, ev: Event) -> Outcome {
    // The rename dialog is modal: while it's up it owns Enter/Esc + its
    // buttons, and InputChange feeds the name buffer. Every other event is
    // swallowed so grid navigation doesn't run underneath the dialog.
    if lf.rename_open {
        return match ev {
            Event::Key(KeyCode::Escape) => { lf.rename_open = false; Outcome::Rerender }
            Event::Key(KeyCode::Enter)  => { lf.commit_rename(); Outcome::Rerender }
            Event::InputChange { value } => {
                lf.rename_buf.clear();
                let max = lf.rename_buf.capacity().min(value.len());
                lf.rename_buf.push_str(&value[..max]);
                Outcome::Rerender
            }
            Event::Action(ActionId(id)) => handle_action(lf, id),
            _ => Outcome::Idle,
        };
    }

    match ev {
        Event::Key(KeyCode::Escape) => {
            // Cancel the most-specific overlay first, then clear a search,
            // then quit — the common cancel-then-quit ladder (Finder /
            // Spotlight / editors).
            if lf.ctx_open {
                lf.ctx_open = false;
                Outcome::Rerender
            } else if lf.open_menu.is_some() {
                lf.open_menu = None;
                Outcome::Rerender
            } else if !lf.query.is_empty() {
                lf.query.clear();
                lf.refilter();
                Outcome::Rerender
            } else {
                Outcome::Exit
            }
        }
        Event::Key(KeyCode::Up)        => { lf.select_delta_y(-1); Outcome::Rerender }
        Event::Key(KeyCode::Down)      => { lf.select_delta_y( 1); Outcome::Rerender }
        Event::Key(KeyCode::Left)      => {
            // Compositor consumes Left/Right when the search Input
            // is focused; if we get this event it means search is
            // empty AND focus is somewhere non-editing — fall back
            // to grid horizontal nav.
            lf.select_delta_x(-1); Outcome::Rerender
        }
        Event::Key(KeyCode::Right)     => { lf.select_delta_x( 1); Outcome::Rerender }
        Event::Key(KeyCode::Enter)     => { lf.open_selected(); Outcome::Rerender }
        // F2 renames the selection — the familiar file-manager shortcut.
        Event::Key(KeyCode::F(2))      => { lf.open_rename(); Outcome::Rerender }
        Event::Key(KeyCode::Backspace) => {
            // Same fall-through reasoning as Left/Right above —
            // Backspace inside a non-empty search is consumed by the
            // editor; reaching us means search was empty, treat it
            // as "go up" (Finder convention).
            lf.go_up(); Outcome::Rerender
        }
        // Ctrl+C / X / V, delivered by the compositor because loft's grid
        // isn't a text widget. Copy/cut arm the clipboard; paste applies it.
        Event::Clipboard(ClipKind::Copy)  => { lf.do_copy();  Outcome::Rerender }
        Event::Clipboard(ClipKind::Cut)   => { lf.do_cut();   Outcome::Rerender }
        Event::Clipboard(ClipKind::Paste) => { lf.do_paste(); Outcome::Rerender }
        Event::InputChange { value } => {
            // Mirror the new buffer into our pre-mark `query` slot
            // (clear + push_str within capacity) so it survives the
            // upcoming `alloc_reset`. Past QUERY_CAP we hard-cap;
            // the compositor reconciles on the next round-trip.
            lf.query.clear();
            let max = QUERY_CAP.min(value.len());
            lf.query.push_str(&value[..max]);
            lf.refilter();
            Outcome::Rerender
        }
        // Right-click a file/folder → select it and open the context menu.
        Event::ContextAction(ActionId(id)) => handle_context(lf, id),
        Event::Action(ActionId(id)) => handle_action(lf, id),
        _ => Outcome::Idle,
    }
}

/// Right-click dispatch: select the clicked grid/list item and raise the
/// context menu popover anchored to it.
fn handle_context(lf: &mut Loft, id: u32) -> Outcome {
    if id >= ACT_GRID_CLICK_BASE && id < ACT_GRID_HOVER_BASE {
        let ui_idx = (id - ACT_GRID_CLICK_BASE) as usize;
        if ui_idx < lf.filtered.len() {
            lf.grid_sel = Some(ui_idx);
            lf.open_menu = None;
            lf.ctx_open = true;
            return Outcome::Rerender;
        }
    }
    Outcome::Idle
}

fn handle_action(lf: &mut Loft, id: u32) -> Outcome {
    match id {
        ACT_TOOLBAR_BACK    => { lf.go_back();    Outcome::Rerender }
        ACT_TOOLBAR_FORWARD => { lf.go_forward(); Outcome::Rerender }
        ACT_TOOLBAR_UP      => { lf.go_up();      Outcome::Rerender }
        ACT_TOOLBAR_REFRESH => { lf.refresh();    Outcome::Rerender }
        // Menu-bar labels: toggle the matching dropdown. Clicking the
        // already-open menu's label re-fires this and closes it
        // (matches macOS / Files behavior). Clicking a different menu
        // switches dropdowns directly.
        ACT_MENU_FILE => { lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::File); Outcome::Rerender }
        ACT_MENU_EDIT => { lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::Edit); Outcome::Rerender }
        ACT_MENU_VIEW => { lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::View); Outcome::Rerender }
        ACT_MENU_GO   => { lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::Go);   Outcome::Rerender }
        ACT_MENU_HELP => { lf.open_menu = toggle_menu(lf.open_menu, OpenMenu::Help); Outcome::Rerender }
        // Click-outside-popover dismiss — close the open menu.
        ACT_MENU_DISMISS => {
            if lf.open_menu.is_some() {
                lf.open_menu = None;
                Outcome::Rerender
            } else {
                Outcome::Idle
            }
        }
        // Dropdown items.
        ACT_FILE_QUIT => Outcome::Exit,
        ACT_VIEW_GRID => {
            lf.view_mode = ViewMode::Grid;
            lf.open_menu = None;
            Outcome::Rerender
        }
        ACT_VIEW_LIST => {
            lf.view_mode = ViewMode::List;
            lf.open_menu = None;
            Outcome::Rerender
        }
        ACT_GO_HOME => {
            lf.open_menu = None;
            let home = read_home_dir();
            lf.navigate(home);
            Outcome::Rerender
        }
        ACT_GO_FILESYSTEM => {
            lf.open_menu = None;
            lf.navigate(String::new());
            Outcome::Rerender
        }
        ACT_HELP_ABOUT => {
            log("[loft] About: nopeekOS file browser, v0.2.x");
            lf.open_menu = None;
            Outcome::Rerender
        }
        // File operations — from the Edit menu or the right-click context
        // menu. Both close whichever menu raised them and act on the
        // current selection / clipboard.
        ACT_EDIT_COPY  => { lf.do_copy();  lf.open_menu = None; lf.ctx_open = false; Outcome::Rerender }
        ACT_EDIT_CUT   => { lf.do_cut();   lf.open_menu = None; lf.ctx_open = false; Outcome::Rerender }
        ACT_EDIT_PASTE => { lf.do_paste(); lf.open_menu = None; lf.ctx_open = false; Outcome::Rerender }
        ACT_EDIT_RENAME => { lf.open_rename(); Outcome::Rerender }
        ACT_RENAME_SUBMIT => { lf.commit_rename(); Outcome::Rerender }
        ACT_RENAME_CANCEL => { lf.rename_open = false; Outcome::Rerender }
        ACT_CTX_DISMISS => {
            if lf.ctx_open { lf.ctx_open = false; Outcome::Rerender } else { Outcome::Idle }
        }
        // Column-header clicks → sort / toggle direction.
        ACT_HEADER_NAME  => { lf.set_sort(SortKey::Name);     Outcome::Rerender }
        ACT_HEADER_SIZE  => { lf.set_sort(SortKey::Size);     Outcome::Rerender }
        ACT_HEADER_FILES => { lf.set_sort(SortKey::Files);    Outcome::Rerender }
        ACT_HEADER_TYPE  => { lf.set_sort(SortKey::Type);     Outcome::Rerender }
        ACT_HEADER_MTIME => { lf.set_sort(SortKey::Modified); Outcome::Rerender }
        _ => {
            if id >= ACT_BREADCRUMB_BASE && id < ACT_TOOLBAR_BACK {
                let n = (id - ACT_BREADCRUMB_BASE) as usize;
                let target = take_first_segments(&lf.current, n);
                if target != lf.current { lf.navigate(target); return Outcome::Rerender; }
                return Outcome::Idle;
            }
            if id >= ACT_SIDEBAR_HOVER_BASE && id < ACT_BREADCRUMB_BASE {
                let i = (id - ACT_SIDEBAR_HOVER_BASE) as usize;
                if i < lf.sidebar.len() && lf.sidebar_sel != Some(i) {
                    lf.sidebar_sel = Some(i);
                    return Outcome::Rerender;
                }
                return Outcome::Idle;
            }
            if id >= ACT_SIDEBAR_CLICK_BASE && id < ACT_SIDEBAR_HOVER_BASE {
                let i = (id - ACT_SIDEBAR_CLICK_BASE) as usize;
                if let Some(p) = lf.sidebar.get(i) {
                    let path = p.path.clone();
                    lf.navigate(path);
                    return Outcome::Rerender;
                }
                return Outcome::Idle;
            }
            if id >= ACT_GRID_HOVER_BASE && id < ACT_SIDEBAR_CLICK_BASE {
                let ui_idx = (id - ACT_GRID_HOVER_BASE) as usize;
                if ui_idx < lf.filtered.len() && lf.grid_sel != Some(ui_idx) {
                    lf.grid_sel = Some(ui_idx);
                    return Outcome::Rerender;
                }
                return Outcome::Idle;
            }
            if id >= ACT_GRID_CLICK_BASE && id < ACT_GRID_HOVER_BASE {
                let ui_idx = (id - ACT_GRID_CLICK_BASE) as usize;
                if ui_idx < lf.filtered.len() {
                    lf.grid_sel = Some(ui_idx);
                    lf.open_selected();
                    return Outcome::Rerender;
                }
            }
            Outcome::Idle
        }
    }
}

// ── Sidebar helpers ───────────────────────────────────────────────────

fn default_sidebar(home: &str) -> Vec<Place> {
    alloc::vec![
        Place { label: "Home".into(),       icon: IconId::Home,       path: home.into() },
        Place { label: "Documents".into(),  icon: IconId::FileText,   path: alloc::format!("{}/documents",  home) },
        Place { label: "Downloads".into(),  icon: IconId::Download,   path: alloc::format!("{}/downloads",  home) },
        Place { label: "Pictures".into(),   icon: IconId::Image,      path: alloc::format!("{}/pictures",   home) },
        Place { label: "Projects".into(),   icon: IconId::Folders,    path: alloc::format!("{}/projects",   home) },
        Place { label: "Filesystem".into(), icon: IconId::HardDrives, path: String::new() },
        Place { label: "Trash".into(),      icon: IconId::Trash,      path: alloc::format!("{}/.trash",     home) },
    ]
}

fn is_device(label: &str) -> bool { label == "Filesystem" || label == "Trash" }

/// Click on a menu-bar label: open it if no menu was open or a
/// different one was, close it if the same one was already open.
fn toggle_menu(current: Option<OpenMenu>, target: OpenMenu) -> Option<OpenMenu> {
    match current {
        Some(c) if c == target => None,
        _                       => Some(target),
    }
}

// ── Kernel-side calls ─────────────────────────────────────────────────

/// Parse `sys/config/associations` (optional) into (ext, app) pairs.
/// One mapping per line: `ext=app` (`#` comments + blanks skipped).
/// Absent file → empty (loft falls back to built-in defaults).
fn load_associations() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let key = "sys/config/associations";
    let mut buf = [0u8; 2048];
    let n = unsafe {
        npk_fetch(key.as_ptr() as i32, key.len() as i32,
                  buf.as_mut_ptr() as i32, buf.len() as i32)
    };
    if n <= 0 { return out; }
    if let Ok(s) = core::str::from_utf8(&buf[..n as usize]) {
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            if let Some((ext, app)) = line.split_once('=') {
                let ext = ext.trim().to_ascii_lowercase();
                let app = app.trim().to_string();
                if !ext.is_empty() && !app.is_empty() { out.push((ext, app)); }
            }
        }
    }
    out
}

fn read_home_dir() -> String {
    // The username lives in the single encrypted `.system/config` blob,
    // not a fetchable `sys/config/name` object — ask the kernel for the
    // resolved home dir directly.
    let buf_ptr = core::ptr::addr_of_mut!(NAME_BUF) as *mut u8;
    let n = unsafe { npk_home_dir(buf_ptr as i32, NAME_FETCH_CAP as i32) };
    if n <= 0 { return String::from("home"); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    match core::str::from_utf8(slice) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => String::from("home"),
    }
}

fn list_dir(prefix: &str) -> Vec<Entry> {
    list_dir_internal(prefix, 0)
}

/// Cheap order-sensitive signature of a directory listing — folds count,
/// names, sizes and is_dir of every entry (FNV-1a). Changes when a file
/// is added, removed, renamed or resized. Used for auto-refresh.
fn dir_signature(entries: &[Entry]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    h = (h ^ entries.len() as u64).wrapping_mul(0x0000_0100_0000_01b3);
    for e in entries {
        for &b in e.name.as_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h = (h ^ e.size).wrapping_mul(0x0000_0100_0000_01b3);
        h = (h ^ e.is_dir as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Recursive listing — `recursive=1` to the host fn — for search
/// mode. Each entry's `name` is the full sub-path under `prefix`
/// (e.g. "wallpapers/aurora" when listing under
/// "home/florian/pictures"), so a search hit visually points at the
/// match's location. Skips synthetic `.dir` markers.
fn list_dir_recursive(prefix: &str) -> Vec<Entry> {
    list_dir_internal(prefix, 1)
}

fn list_dir_internal(prefix: &str, recursive: i32) -> Vec<Entry> {
    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    let n = unsafe {
        npk_fs_list(
            prefix.as_ptr() as i32, prefix.len() as i32,
            buf_ptr as i32, LIST_BUF_SIZE as i32,
            recursive,
        )
    };
    if n <= 0 { return Vec::new(); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    let mut out: Vec<Entry> = Vec::new();
    for line in slice.split(|&b| b == b'\n') {
        if let Some(e) = parse_entry(line) { out.push(e); }
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    out
}

// ── Folder size/count + sorting ───────────────────────────────────────

/// Reset every folder row to "pending" so `pump_stats` will scan it.
/// Order-independent (just flips flags), so it's safe to call before the
/// sort. Files are left untouched (they already carry their own size).
fn init_folder_stats(entries: &mut [Entry]) {
    for e in entries.iter_mut() {
        if e.is_dir {
            e.size = 0;
            e.files = 0;
            e.stats_pending = true;
        }
    }
}

/// Indices of folders still flagged pending, in current (sorted) order.
fn pending_folder_indices(entries: &[Entry]) -> Vec<usize> {
    entries.iter().enumerate()
        .filter(|(_, e)| e.is_dir && e.stats_pending)
        .map(|(i, _)| i)
        .collect()
}

/// Recursive (size, file_count) of a single folder. One
/// `npk_fs_list(recursive=1)` scan, summed inline without allocating an
/// `Entry` per descendant — cheap even for large subtrees, and called one
/// folder at a time off the idle loop so no single scan stalls the app.
fn scan_folder_stats(path: &str) -> (u64, u64) {
    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    let n = unsafe {
        npk_fs_list(
            path.as_ptr() as i32, path.len() as i32,
            buf_ptr as i32, LIST_BUF_SIZE as i32,
            1,
        )
    };
    if n <= 0 { return (0, 0); }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    let (mut bytes, mut files) = (0u64, 0u64);
    for line in slice.split(|&b| b == b'\n') {
        let Some(nul) = line.iter().position(|&b| b == 0) else { continue };
        let rest = &line[nul + 1..];
        if rest.len() < 10 { continue; }
        if rest[9] != 0 { continue; }            // directory → count files only
        let Ok(b8) = rest[..8].try_into() else { continue };
        bytes = bytes.saturating_add(u64::from_le_bytes(b8));
        files += 1;
    }
    (bytes, files)
}

/// Sort the browse listing: folders always grouped before files
/// (Thunar/Files idiom), then ordered within each group by `key`,
/// reversed for descending.
fn sort_entries(v: &mut [Entry], key: SortKey, asc: bool) {
    v.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => return core::cmp::Ordering::Less,
            (false, true) => return core::cmp::Ordering::Greater,
            _ => {}
        }
        let o = cmp_entries(a, b, key);
        if asc { o } else { o.reverse() }
    });
}

fn cmp_entries(a: &Entry, b: &Entry, key: SortKey) -> core::cmp::Ordering {
    let tie = a.name_lc.cmp(&b.name_lc);
    match key {
        SortKey::Name     => tie,
        SortKey::Size     => a.size.cmp(&b.size).then(tie),
        SortKey::Files    => a.files.cmp(&b.files).then(tie),
        SortKey::Type     => type_for(a).cmp(&type_for(b)).then(tie),
        SortKey::Modified => a.mtime.cmp(&b.mtime).then(tie),
    }
}

/// Drop sidebar entries whose path is not currently backed by a
/// `.dir` marker. Keeps "Filesystem" (empty path = npkFS root) — it
/// always exists by definition. Honest UI: if you can see it, you
/// can navigate into it without hitting an empty phantom.
fn filter_sidebar_to_existing(places: Vec<Place>) -> Vec<Place> {
    places.into_iter().filter(|p| {
        if p.path.is_empty() { return true; } // Filesystem root
        dir_exists(&p.path)
    }).collect()
}

fn dir_exists(path: &str) -> bool {
    // npk_fs_stat returns 17 bytes since kernel v0.146 (size + is_dir
    // + mtime). Kept buffer-sized to the wider shape; the is_dir byte
    // sits at offset 8 in both v2 and v3 ABI so the check stays
    // forward-compat against future appends. `n > 0` distinguishes a
    // valid stat from "not found" (0) or "error" (-1).
    let mut out = [0u8; 17];
    let n = unsafe {
        npk_fs_stat(
            path.as_ptr() as i32, path.len() as i32,
            out.as_mut_ptr() as i32,
        )
    };
    n > 0 && out[8] != 0
}

// ── Path helpers for file operations ──────────────────────────────────

/// Join a directory path with a child name. npkFS uses slash paths and
/// the filesystem root is the empty string, so a join off root omits the
/// leading slash.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { alloc::format!("{}/{}", dir, name) }
}

/// Final path component of a (possibly relative, search-mode) name.
fn basename(name: &str) -> &str {
    match name.rsplit_once('/') {
        Some((_, b)) => b,
        None => name,
    }
}

/// True if any object (file or directory) exists at `path`.
fn path_exists(path: &str) -> bool {
    let mut out = [0u8; 17];
    let n = unsafe {
        npk_fs_stat(path.as_ptr() as i32, path.len() as i32, out.as_mut_ptr() as i32)
    };
    n > 0
}

/// Split a file name into (stem, extension-with-dot): "a.txt" → ("a",
/// ".txt"); "README" → ("README", ""). A leading dot (dotfile) stays in
/// the stem so the copy suffix lands before any real extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// A collision-free full path for `name` inside `dir`. Appends " copy",
/// then " copy 2", " copy 3"… before the extension until the path is free
/// (Finder/Files idiom), capped so a pathological directory can't spin.
fn unique_in(dir: &str, name: &str) -> String {
    let base = join(dir, name);
    if !path_exists(&base) { return base; }
    let (stem, ext) = split_ext(name);
    let mut n = 1u32;
    loop {
        let cand_name = if n == 1 {
            alloc::format!("{} copy{}", stem, ext)
        } else {
            alloc::format!("{} copy {}{}", stem, n, ext)
        };
        let cand = join(dir, &cand_name);
        if !path_exists(&cand) || n >= 999 { return cand; }
        n += 1;
    }
}

// Wire: name\0size_le_u64(8)\0is_dir_u8(1)\0mtime_le_u64(8) on
// kernel ≥ v0.146; older kernels stop after is_dir (10 trailing
// bytes). Parse defensively — accept either shape so the loft
// .wasm boots on a stale-kernel disk during dev cycles.
fn parse_entry(line: &[u8]) -> Option<Entry> {
    let nul = line.iter().position(|&b| b == 0)?;
    let name = core::str::from_utf8(&line[..nul]).ok()?.to_string();
    let rest = &line[nul + 1..];
    if rest.len() < 10 { return None; }
    let size = u64::from_le_bytes(rest[..8].try_into().ok()?);
    let is_dir = rest[9] != 0;
    // mtime tail (offset 10..19): 1 sep byte + 8 LE bytes. Absent on
    // pre-v3 kernels → mtime stays 0 ("unknown").
    let mtime = if rest.len() >= 19 {
        u64::from_le_bytes(rest[11..19].try_into().ok()?)
    } else {
        0
    };
    let name_lc = name.to_ascii_lowercase();
    Some(Entry { name, name_lc, size, is_dir, files: 0, stats_pending: false, mtime })
}

// ── Path helpers ──────────────────────────────────────────────────────

fn parent_path(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn take_first_segments(path: &str, n: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for part in path.split('/') {
        if part.is_empty() { continue; }
        if count >= n { break; }
        if !out.is_empty() { out.push('/'); }
        out.push_str(part);
        count += 1;
    }
    out
}

// ── Icon + type label ─────────────────────────────────────────────────

/// Human-readable type column for the list view. Mirrors the
/// `icon_for` taxonomy so the icon and the label always agree.
fn type_for(e: &Entry) -> String {
    if e.is_dir { return "Folder".to_string(); }
    let ext = e.name.rsplit('.').next().unwrap_or("");
    match ext {
        "md" | "txt" | "log" | "cfg" | "toml" | "json" | "yaml" | "yml" => "Text".to_string(),
        "rs" | "wasm" | "sh" | "py" | "c" | "h" | "hpp" | "cpp" | "go"  => "Code".to_string(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg"          => "Image".to_string(),
        ""    => "File".to_string(),
        other => alloc::format!("{} File", other.to_uppercase()),
    }
}

/// Render a Unix-second timestamp as "YYYY-MM-DD HH:MM" UTC. Zero
/// → "—" (mtime unknown — RTC was unreadable when the entry was
/// created, or the entry was written by a pre-v3 kernel that
/// didn't have the field). No std::time, no chrono — pure integer
/// math against the proleptic Gregorian calendar, matching what
/// `kernel/src/drivers/rtc.rs::datetime_to_unix` reverses.
fn format_mtime(secs: u64) -> String {
    if secs == 0 { return "—".to_string(); }
    let (y, mo, d, h, mi, _s) = unix_to_civil(secs);
    let mut s = String::with_capacity(16);
    push_zpad(&mut s, y as u64, 4); s.push('-');
    push_zpad(&mut s, mo as u64, 2); s.push('-');
    push_zpad(&mut s, d as u64, 2); s.push(' ');
    push_zpad(&mut s, h as u64, 2); s.push(':');
    push_zpad(&mut s, mi as u64, 2);
    s
}

/// `Howard Hinnant`-style civil_from_days. Converts Unix seconds to
/// (year, month [1..=12], day [1..=31], hour, minute, second) in UTC
/// without leap-second awareness (good enough for "modified" UI).
fn unix_to_civil(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem  = (secs % 86_400) as u32;
    let h    = rem / 3600;
    let mi   = (rem % 3600) / 60;
    let s    = rem % 60;

    // Shift epoch to 0000-03-01 to make leap math simple.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y_int = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp  = (5 * doy + 2) / 153;
    let d   = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo  = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y   = (y_int + if mo <= 2 { 1 } else { 0 }) as i32;
    (y, mo, d, h, mi, s)
}

fn push_zpad(s: &mut String, mut n: u64, width: usize) {
    let mut buf = [0u8; 20];
    let mut i = 0;
    if n == 0 { buf[0] = b'0'; i = 1; }
    while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    while i < width { s.push('0'); i += 1; }
    let written: Vec<u8> = buf.iter().take_while(|&&b| b != 0).copied().collect();
    for &b in written.iter().rev() { s.push(b as char); }
}

/// Wrapper around the in-place `push_size` helper used by the
/// footer — returns an owned String for the list view's Size cell.
fn format_size(n: u64) -> String {
    let mut s = String::with_capacity(12);
    push_size(&mut s, n);
    s
}

fn icon_for(e: &Entry) -> IconId {
    if e.is_dir { return IconId::Folder; }
    let ext = e.name.rsplit('.').next().unwrap_or("");
    match ext {
        "md" | "txt" | "log" | "cfg" | "toml" | "json" | "yaml" | "yml" => IconId::FileText,
        "rs" | "wasm" | "sh" | "py" | "c" | "h" | "hpp" | "cpp" | "go" => IconId::Code,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" => IconId::Image,
        _ => IconId::File,
    }
}

// ── Number formatters (no_std friendly) ───────────────────────────────

fn push_usize(s: &mut String, mut n: usize) {
    if n == 0 { s.push('0'); return; }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    while i > 0 { i -= 1; s.push(buf[i] as char); }
}

fn push_size(s: &mut String, bytes: u64) {
    // Powers of 1024 — KB / MB / GB. Two decimals once we leave bytes,
    // mockup-aligned ("2.4 GB" rather than "2456 MB"). Pure integer
    // math (no f64 in no_std without messing with the linker).
    const K: u64 = 1024;
    const M: u64 = K * 1024;
    const G: u64 = M * 1024;

    if bytes < K {
        push_usize(s, bytes as usize);
        s.push_str(" B");
    } else if bytes < M {
        push_decimal(s, bytes, K);
        s.push_str(" KB");
    } else if bytes < G {
        push_decimal(s, bytes, M);
        s.push_str(" MB");
    } else {
        push_decimal(s, bytes, G);
        s.push_str(" GB");
    }
}

fn push_decimal(s: &mut String, n: u64, unit: u64) {
    let whole = n / unit;
    let tenths = ((n % unit) * 10) / unit;
    push_usize(s, whole as usize);
    s.push('.');
    s.push((b'0' + tenths as u8) as char);
}

// ── Entry point ───────────────────────────────────────────────────────

fn commit_tree(lf: &Loft) {
    let tree = render(lf);
    match wire::encode(&tree) {
        Ok(bytes) => { if commit(&bytes) < 0 { log("[loft] commit failed"); } }
        Err(_) => log("[loft] encode failed"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // No `npk_window_set_overlay` — loft is a regular tiled app, the
    // first commit creates its window via shade::create_widget_window.
    //
    // Bump-allocator lifecycle:
    //   * `persistent_mark` is the heap top *after* the last state
    //     mutation. Anything below it is live `Loft` state (entries,
    //     history, sidebar Strings, …) that next frame still needs.
    //     Anything above it is the previous frame's Widget tree —
    //     transient, safe to wipe.
    //   * Reset goes *before* `handle()`, not before render.
    //     Otherwise `navigate()`'s freshly-loaded entries land above
    //     the old mark and get clobbered by the very Widget allocs
    //     that follow — the Vec metadata in `loft.entries` survives
    //     but its String contents are overwritten mid-render →
    //     UTF-8 / bounds panic on the next navigate.
    //   * `persistent_mark` is re-captured after `handle()` so the
    //     new state allocs (if any) become part of the persistent
    //     region for next frame.
    let mut loft = Loft::new();
    let mut persistent_mark = alloc_mark();
    let mut idle_ticks: u32 = 0;

    commit_tree(&loft);
    // The widget window exists after the first commit — opt into receiving
    // Ctrl+C/X/V as Event::Clipboard so the shortcuts drive file operations.
    unsafe { let _ = npk_window_set_clipboard_sink(); }

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                idle_ticks = 0;
                alloc_reset(persistent_mark);
                let outcome = handle(&mut loft, ev);
                persistent_mark = alloc_mark();
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Rerender => commit_tree(&loft),
                    Outcome::Exit => { close_self(); return; }
                }
            }
            PollResult::Empty => {
                unsafe { let _ = npk_sleep(16); }

                // Progressively fill folder sizes/counts a few per tick.
                // Wrapped in the alloc_reset/recapture discipline so the
                // scratch/filtered growth lands in the persistent region.
                // Skipped while searching (folder stats aren't shown then).
                if loft.query.is_empty() && !loft.stats_queue.is_empty() {
                    alloc_reset(persistent_mark);
                    let changed = loft.pump_stats(STATS_PUMP_BUDGET);
                    persistent_mark = alloc_mark();
                    if changed { commit_tree(&loft); }
                }

                idle_ticks += 1;
                if idle_ticks >= AUTO_REFRESH_TICKS {
                    idle_ticks = 0;
                    // Auto-refresh the browse view when the folder changed
                    // on disk (new screenshot, download, …). Skip while a
                    // search or menu is active so we don't disturb the user.
                    if loft.open_menu.is_none() && loft.query.is_empty() {
                        // Probe via a THROWAWAY listing + reset so an
                        // unchanged folder leaks nothing in the bump heap
                        // (this runs ~every 1.4 s). Only a real change does
                        // the persistent re-list + re-render.
                        alloc_reset(persistent_mark);
                        let new_sig = dir_signature(&list_dir(&loft.current));
                        alloc_reset(persistent_mark);
                        if new_sig != loft.dir_sig {
                            loft.refresh();
                            persistent_mark = alloc_mark();
                            commit_tree(&loft);
                        }
                    }
                }
            }
            PollResult::WindowGone => return,
        }
    }
}

// Silence unused warning on app_meta::IconRef — referenced through
// the build.rs-generated AppMeta blob, not directly.
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
