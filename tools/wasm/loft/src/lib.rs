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

use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;
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
    size:    u64,
    is_dir:  bool,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenMenu {
    File,
    Edit,
    View,
    Go,
    Help,
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
            view_mode:     ViewMode::Grid,
            open_menu:     None,
        };
        lf.refresh();
        lf
    }

    fn refresh(&mut self) {
        self.entries = list_dir(&self.current);
        // Navigation invalidates any cached recursive listing — the
        // next non-empty query for this directory triggers a fresh
        // `list_dir_recursive` call.
        self.recursive.clear();
        self.recursive_dir = None;
        self.refilter();
        self.sync_sidebar_from_current();
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
        // Defensive log for the .trash crash report — surfaces the
        // exact path being entered on serial so any panic / freeze
        // can be correlated with the trigger.
        log("[loft] navigate");
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
}

// ── Render ────────────────────────────────────────────────────────────

fn render(lf: &Loft) -> Widget {
    let menu = render_menu_bar();
    let toolbar = render_toolbar(lf);
    let body = render_body(lf);
    let footer = render_footer(lf);

    // Custom outer column instead of `prefab::panel`: panel's
    // Padding-Xs + Spacing-Md kept the menu-bar bg from reaching
    // the window edges + put a 12 px gap between menu and divider.
    // Loft wants the menu strip + sidebar fill to be flush —
    // file-manager idiom (Thunar / Files / Finder all do this).
    // Spacing/padding on individual rows (toolbar / footer / body
    // content) handle their own breathing room.
    let mut children: Vec<Widget> = alloc::vec![
        menu,
        Widget::Divider,
        toolbar,
        Widget::Divider,
        body,                           // Modifier::Flex(1) — fills
        Widget::Divider,
        footer,
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
        ("Datei".to_string(),      ActionId(ACT_MENU_FILE)),
        ("Bearbeiten".to_string(), ActionId(ACT_MENU_EDIT)),
        ("Ansicht".to_string(),    ActionId(ACT_MENU_VIEW)),
        ("Gehe zu".to_string(),    ActionId(ACT_MENU_GO)),
        ("Hilfe".to_string(),      ActionId(ACT_MENU_HELP)),
    ];
    let anchors: Vec<NodeId> = alloc::vec![
        NodeId(NODE_MENU_FILE),
        NodeId(NODE_MENU_EDIT),
        NodeId(NODE_MENU_VIEW),
        NodeId(NODE_MENU_GO),
        NodeId(NODE_MENU_HELP),
    ];
    prefab::menu_bar_with_anchors(&labels, &anchors)
}

/// Build the dropdown for the currently-open menu. Returns
/// `(anchor_node_id, content_widget)` so the caller can wrap the
/// content in a `Widget::Popover` against the matching menu label.
fn render_dropdown(lf: &Loft, kind: OpenMenu) -> (u32, Widget) {
    match kind {
        OpenMenu::File => (
            NODE_MENU_FILE,
            prefab::popover_menu(&[
                ("Quit".to_string(), ActionId(ACT_FILE_QUIT)),
            ], None),
        ),
        OpenMenu::Edit => (
            NODE_MENU_EDIT,
            // Empty for now — kept so the dropdown surface still
            // appears (visual feedback that the click registered).
            prefab::popover_menu(&[
                ("(no actions yet)".to_string(), ActionId(ACT_MENU_DISMISS)),
            ], None),
        ),
        OpenMenu::View => (
            NODE_MENU_VIEW,
            prefab::popover_menu(&[
                ("Grid".to_string(), ActionId(ACT_VIEW_GRID)),
                ("List".to_string(), ActionId(ACT_VIEW_LIST)),
            ], Some(match lf.view_mode {
                ViewMode::Grid => 0,
                ViewMode::List => 1,
            })),
        ),
        OpenMenu::Go => (
            NODE_MENU_GO,
            prefab::popover_menu(&[
                ("Home".to_string(),        ActionId(ACT_GO_HOME)),
                ("Filesystem".to_string(),  ActionId(ACT_GO_FILESYSTEM)),
            ], None),
        ),
        OpenMenu::Help => (
            NODE_MENU_HELP,
            prefab::popover_menu(&[
                ("About loft".to_string(), ActionId(ACT_HELP_ABOUT)),
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
        placeholder: "search".to_string(),
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
        ],
        spacing:   Spacing::Sm.as_u16(),
        align:     Align::Center,
        modifiers: alloc::vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
            Modifier::MinWidth(220),
            Modifier::Focus(alloc::vec![
                Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
            ]),
        ],
    }
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
    let sidebar = prefab::sidebar_pane(alloc::vec![
        prefab::sidebar_section("PLACES",  places_rows),
        prefab::sidebar_section("DEVICES", devices_rows),
    ]);

    // Content — filtered grid OR list, plus two empty states
    // (genuinely empty directory vs. nothing matched the search).
    let content: Widget = if lf.filtered.is_empty() {
        let hint = if lf.query.is_empty() {
            "Empty directory"
        } else {
            "No matches"
        };
        prefab::empty_state(hint)
    } else {
        match lf.view_mode {
            ViewMode::Grid => render_grid(lf),
            ViewMode::List => render_list(lf),
        }
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
        prefab::grid_item(
            icon, &e.name,
            lf.grid_sel == Some(ui_idx),
            Some(ActionId(ACT_GRID_CLICK_BASE + ui_idx as u32)),
            Some(ActionId(ACT_GRID_HOVER_BASE + ui_idx as u32)),
        )
    }).collect();
    prefab::grid(grid_children, GRID_COLS)
}

/// Detail-list view: one row per entry, columns Name | Size | Type
/// | Modified. English headers (Florian's request — international
/// FS UX, less ambiguity than the German labels in the menu bar).
fn render_list(lf: &Loft) -> Widget {
    let source = lf.source();
    let mut rows: Vec<Widget> = Vec::with_capacity(lf.filtered.len() + 1);
    rows.push(list_header_row());
    rows.push(Widget::Divider);
    for (ui_idx, &entry_idx) in lf.filtered.iter().enumerate() {
        let e = &source[entry_idx];
        let selected = lf.grid_sel == Some(ui_idx);
        rows.push(list_data_row(
            e, selected,
            ActionId(ACT_GRID_CLICK_BASE + ui_idx as u32),
            ActionId(ACT_GRID_HOVER_BASE + ui_idx as u32),
        ));
    }
    Widget::Column {
        children: rows,
        spacing: 0,
        align:   Align::Stretch,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

fn list_header_row() -> Widget {
    Widget::Row {
        children: alloc::vec![
            list_cell_text("Name",     true,  COL_NAME_W),
            list_cell_text("Size",     false, COL_SIZE_W),
            list_cell_text("Type",     false, COL_TYPE_W),
            list_cell_text("Modified", false, COL_MTIME_W),
        ],
        spacing: Spacing::Md.as_u16(),
        align:   Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Sm.as_u16())],
    }
}

fn list_data_row(e: &Entry, selected: bool, on_click: ActionId, on_hover: ActionId) -> Widget {
    let icon = icon_for(e);
    // Name cell with icon + label.
    let name_cell = Widget::Row {
        children: alloc::vec![
            Widget::Icon { id: icon, size: 24, modifiers: alloc::vec![] },
            Widget::Text {
                content:   e.name.clone(),
                style:     TextStyle::Body,
                modifiers: alloc::vec![],
            },
        ],
        spacing: Spacing::Sm.as_u16(),
        align:   Align::Center,
        modifiers: alloc::vec![Modifier::MinWidth(COL_NAME_W)],
    };
    let size_str   = if e.is_dir { "—".to_string() } else { format_size(e.size) };
    let type_str   = type_for(e);
    let mtime_str  = format_mtime(e.mtime);
    let mut row_mods: Vec<Modifier> = alloc::vec![
        Modifier::Padding(Padding::Sm.as_u16()),
        Modifier::OnClick(on_click),
        Modifier::OnHover(on_hover),
        Modifier::Hover(alloc::vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Rounded(Radius::Sm.as_u8()),
        ]),
    ];
    if selected {
        row_mods.push(Modifier::Background(Token::SurfaceElevated));
        row_mods.push(Modifier::Border {
            token:  Token::Accent,
            width:  1,
            radius: Radius::Sm.as_u8(),
        });
    }
    Widget::Row {
        children: alloc::vec![
            name_cell,
            list_cell_text(&size_str,  false, COL_SIZE_W),
            list_cell_text(&type_str,  false, COL_TYPE_W),
            list_cell_text(&mtime_str, false, COL_MTIME_W),
        ],
        spacing: Spacing::Md.as_u16(),
        align:   Align::Center,
        modifiers: row_mods,
    }
}

fn list_cell_text(text: &str, header: bool, min_w: u16) -> Widget {
    Widget::Text {
        content: text.to_string(),
        style:   if header { TextStyle::Caption } else { TextStyle::Body },
        modifiers: alloc::vec![Modifier::MinWidth(min_w)],
    }
}

const COL_NAME_W:  u16 = 320;
const COL_SIZE_W:  u16 = 100;
const COL_TYPE_W:  u16 = 120;
const COL_MTIME_W: u16 = 180;

fn render_footer(lf: &Loft) -> Widget {
    // Mockup-aligned: hints on the left, count + selection + size on
    // the right. Selection text changes verbatim with grid_sel; the
    // total directory size is summed across visible entries (the
    // filtered set, not the raw list, so users see what their search
    // narrowed to).
    let hints = "↑↓ navigate   ↵ open   esc clear/close";

    let visible = lf.filtered.len();
    let mut total_bytes: u64 = 0;
    let source = lf.source();
    for &i in &lf.filtered {
        if let Some(e) = source.get(i) {
            if !e.is_dir { total_bytes = total_bytes.saturating_add(e.size); }
        }
    }
    let mut right = String::with_capacity(48);
    push_usize(&mut right, visible);
    right.push_str(if visible == 1 { " item" } else { " items" });
    if let Some(idx) = lf.grid_sel {
        if idx < lf.filtered.len() { right.push_str(" · 1 selected"); }
    }
    if total_bytes > 0 {
        right.push_str(" · ");
        push_size(&mut right, total_bytes);
    }
    prefab::footer(hints, &right)
}

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
    match ev {
        Event::Key(KeyCode::Escape) => {
            // Two-step: first Escape clears a non-empty search,
            // second Escape closes the window. Mirrors the
            // common-cancel-then-quit pattern of macOS Finder /
            // Spotlight / many editors.
            if !lf.query.is_empty() {
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
        Event::Key(KeyCode::Backspace) => {
            // Same fall-through reasoning as Left/Right above —
            // Backspace inside a non-empty search is consumed by the
            // editor; reaching us means search was empty, treat it
            // as "go up" (Finder convention).
            lf.go_up(); Outcome::Rerender
        }
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
        Event::Action(ActionId(id)) => handle_action(lf, id),
        _ => Outcome::Idle,
    }
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

fn read_home_dir() -> String {
    let key = "sys/config/name";
    let buf_ptr = core::ptr::addr_of_mut!(NAME_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(
            key.as_ptr() as i32, key.len() as i32,
            buf_ptr as i32, NAME_FETCH_CAP as i32,
        )
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

fn list_dir(prefix: &str) -> Vec<Entry> {
    list_dir_internal(prefix, 0)
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
    Some(Entry { name, name_lc, size, is_dir, mtime })
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

    commit_tree(&loft);

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                alloc_reset(persistent_mark);
                let outcome = handle(&mut loft, ev);
                persistent_mark = alloc_mark();
                match outcome {
                    Outcome::Idle => {}
                    Outcome::Rerender => commit_tree(&loft),
                    Outcome::Exit => { close_self(); return; }
                }
            }
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            PollResult::WindowGone => return,
        }
    }
}

// Silence unused warning on app_meta::IconRef — referenced through
// the build.rs-generated AppMeta blob, not directly.
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
