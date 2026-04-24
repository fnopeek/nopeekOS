//! loft — file browser for nopeekOS (Thunar-clone).
//!
//! Keys are paths in npkFS. The "filesystem" is a single flat key→bytes
//! store; directories are expressed by `<path>/.dir` markers. loft
//! navigates purely by prefix — no libc, no inodes, no mount table.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::prefab;
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
    // Reserved for future delete / stat UI — wired in the ABI already.
    #[allow(dead_code)]
    fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;
    #[allow(dead_code)]
    fn npk_fs_delete(name_ptr: i32, name_len: i32) -> i32;
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

const EVENT_BUF_SIZE: usize = 64;
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

// Bump allocator. State allocated before `persistent_mark` lives across
// frames; everything after it is widget-tree garbage reset on rerender.
const HEAP_SIZE: usize = 512 * 1024;
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

// ── Action IDs ─────────────────────────────────────────────────────────
// Bands let handle() dispatch without a giant match.

const ACT_GRID_CLICK_BASE:     u32 = 0;
const ACT_GRID_HOVER_BASE:     u32 = 1_000;
const ACT_SIDEBAR_CLICK_BASE:  u32 = 2_000;
const ACT_SIDEBAR_HOVER_BASE:  u32 = 2_500;
const ACT_BREADCRUMB_BASE:     u32 = 3_000;
const ACT_TOOLBAR_BACK:        u32 = 4_000;
const ACT_TOOLBAR_FORWARD:     u32 = 4_001;
const ACT_TOOLBAR_UP:          u32 = 4_002;
const ACT_TOOLBAR_REFRESH:     u32 = 4_003;

const GRID_COLS: usize = 4;
const LIST_BUF_SIZE: usize = 64 * 1024;
static mut LIST_BUF: [u8; LIST_BUF_SIZE] = [0; LIST_BUF_SIZE];

const NAME_FETCH_CAP: usize = 64;
static mut NAME_BUF: [u8; NAME_FETCH_CAP] = [0; NAME_FETCH_CAP];

// ── State ──────────────────────────────────────────────────────────────

struct Place {
    label: String,
    icon:  IconId,
    path:  String,
}

struct Entry {
    name:   String,
    size:   u64,
    is_dir: bool,
}

struct Loft {
    current:      String,     // current prefix, no trailing slash
    history:      Vec<String>,
    forward:      Vec<String>,
    sidebar:      Vec<Place>,
    entries:      Vec<Entry>,
    grid_sel:     Option<usize>,
    sidebar_sel:  Option<usize>,
}

impl Loft {
    fn new() -> Self {
        let home = read_home_dir();
        let sidebar = default_sidebar(&home);
        let mut lf = Loft {
            current: home,
            history: Vec::new(),
            forward: Vec::new(),
            sidebar,
            entries: Vec::new(),
            grid_sel: None,
            sidebar_sel: Some(0),  // Home is first
        };
        lf.refresh();
        lf
    }

    fn refresh(&mut self) {
        self.entries = list_dir(&self.current);
        // Reset selection when directory changes and prior index is invalid.
        if let Some(i) = self.grid_sel {
            if i >= self.entries.len() { self.grid_sel = None; }
        }
        self.sync_sidebar_from_current();
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
        self.grid_sel = None;
        self.refresh();
    }

    fn go_back(&mut self) {
        if let Some(p) = self.history.pop() {
            self.forward.push(self.current.clone());
            self.current = p;
            self.grid_sel = None;
            self.refresh();
        }
    }

    fn go_forward(&mut self) {
        if let Some(p) = self.forward.pop() {
            self.history.push(self.current.clone());
            self.current = p;
            self.grid_sel = None;
            self.refresh();
        }
    }

    fn go_up(&mut self) {
        let parent = parent_path(&self.current);
        if parent != self.current {
            self.navigate(parent);
        }
    }

    fn open_selected(&mut self) {
        let Some(i) = self.grid_sel else { return; };
        let Some(entry) = self.entries.get(i) else { return; };
        if entry.is_dir {
            let next = if self.current.is_empty() {
                entry.name.clone()
            } else {
                alloc::format!("{}/{}", self.current, entry.name)
            };
            self.navigate(next);
        }
        // Files: no default opener yet — caller taps are a no-op until
        // we have a text viewer / image viewer app. Keep the selection.
    }

    fn select_delta_x(&mut self, dx: isize) {
        self.move_selection(dx);
    }

    fn select_delta_y(&mut self, dy: isize) {
        self.move_selection(dy * GRID_COLS as isize);
    }

    fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() { self.grid_sel = None; return; }
        let cur = self.grid_sel.unwrap_or(0) as isize;
        let mut next = cur + delta;
        let max = self.entries.len() as isize - 1;
        if next < 0 { next = 0; }
        if next > max { next = max; }
        self.grid_sel = Some(next as usize);
    }
}

// ── Widget tree ────────────────────────────────────────────────────────

fn render(lf: &Loft) -> Widget {
    // Sidebar: PLACES + DEVICES, grouped by the last 2 synthetic entries.
    let mut places_rows: Vec<Widget> = Vec::new();
    let mut devices_rows: Vec<Widget> = Vec::new();

    for (i, p) in lf.sidebar.iter().enumerate() {
        let selected = lf.sidebar_sel == Some(i);
        let row = prefab::nav_row(
            p.icon,
            &p.label,
            selected,
            Some(ActionId(ACT_SIDEBAR_CLICK_BASE + i as u32)),
            Some(ActionId(ACT_SIDEBAR_HOVER_BASE + i as u32)),
        );
        if is_device(&p.label) { devices_rows.push(row); }
        else { places_rows.push(row); }
    }

    let sidebar = Widget::Column {
        children: alloc::vec![
            prefab::sidebar_section("PLACES",  places_rows),
            prefab::sidebar_section("DEVICES", devices_rows),
            Widget::Spacer { flex: 1 },
        ],
        spacing: 0,
        align:   Align::Stretch,
        modifiers: alloc::vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Padding(8),
        ],
    };

    // Toolbar: back / forward / up + breadcrumb.
    let toolbar = prefab::toolbar(alloc::vec![
        prefab::icon_button(IconId::ArrowLeft,  18, Some(ActionId(ACT_TOOLBAR_BACK)),    None),
        prefab::icon_button(IconId::ArrowRight, 18, Some(ActionId(ACT_TOOLBAR_FORWARD)), None),
        prefab::icon_button(IconId::ArrowUp,    18, Some(ActionId(ACT_TOOLBAR_UP)),      None),
        breadcrumb_for(&lf.current),
        Widget::Spacer { flex: 1 },
        prefab::icon_button(IconId::ArrowClockwise, 16, Some(ActionId(ACT_TOOLBAR_REFRESH)), None),
    ]);

    // Grid content area.
    let grid_children: Vec<Widget> = lf.entries.iter().enumerate().map(|(i, e)| {
        let icon = icon_for(e);
        prefab::grid_item(
            icon,
            &e.name,
            lf.grid_sel == Some(i),
            Some(ActionId(ACT_GRID_CLICK_BASE + i as u32)),
            Some(ActionId(ACT_GRID_HOVER_BASE + i as u32)),
        )
    }).collect();

    let content: Widget = if grid_children.is_empty() {
        prefab::empty_state("Empty directory")
    } else {
        prefab::grid(grid_children, GRID_COLS)
    };

    // Footer: count on the left, selected name on the right.
    let footer_left = alloc::format!(
        "{} item{}",
        lf.entries.len(),
        if lf.entries.len() == 1 { "" } else { "s" },
    );
    let footer_right = match lf.grid_sel.and_then(|i| lf.entries.get(i)) {
        Some(e) if e.is_dir => alloc::format!("selected: {} (dir)", e.name),
        Some(e) => alloc::format!("selected: {} ({} B)", e.name, e.size),
        None => String::from("↑↓←→ navigate   ↵ open   esc close"),
    };
    let footer = prefab::footer(&footer_right, &footer_left);

    // Compose: toolbar row, then split sidebar + content, then footer.
    let body = Widget::Row {
        children: alloc::vec![sidebar, content],
        spacing: 0,
        align:   Align::Stretch,
        modifiers: alloc::vec![],
    };

    prefab::panel(alloc::vec![
        toolbar,
        Widget::Divider,
        body,
        Widget::Spacer { flex: 1 },
        Widget::Divider,
        footer,
    ])
}

fn breadcrumb_for(path: &str) -> Widget {
    let mut segs: Vec<(String, ActionId)> = Vec::new();
    let mut acc = String::new();
    if path.is_empty() {
        segs.push(("/".into(), ActionId(ACT_BREADCRUMB_BASE)));
    } else {
        segs.push(("/".into(), ActionId(ACT_BREADCRUMB_BASE)));
        for (i, part) in path.split('/').enumerate() {
            if part.is_empty() { continue; }
            if !acc.is_empty() { acc.push('/'); }
            acc.push_str(part);
            segs.push((part.to_string(), ActionId(ACT_BREADCRUMB_BASE + i as u32 + 1)));
        }
    }
    prefab::breadcrumb(&segs)
}

// ── Event handling ─────────────────────────────────────────────────────

enum Outcome { Idle, Rerender, Exit }

fn handle(lf: &mut Loft, ev: Event) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => Outcome::Exit,
        Event::Key(KeyCode::Up)     => { lf.select_delta_y(-1); Outcome::Rerender }
        Event::Key(KeyCode::Down)   => { lf.select_delta_y( 1); Outcome::Rerender }
        Event::Key(KeyCode::Left)   => { lf.select_delta_x(-1); Outcome::Rerender }
        Event::Key(KeyCode::Right)  => { lf.select_delta_x( 1); Outcome::Rerender }
        Event::Key(KeyCode::Enter)  => { lf.open_selected(); Outcome::Rerender }
        Event::Key(KeyCode::Backspace) => { lf.go_up(); Outcome::Rerender }

        Event::Action(ActionId(id)) => {
            if id == ACT_TOOLBAR_BACK    { lf.go_back();    return Outcome::Rerender; }
            if id == ACT_TOOLBAR_FORWARD { lf.go_forward(); return Outcome::Rerender; }
            if id == ACT_TOOLBAR_UP      { lf.go_up();      return Outcome::Rerender; }
            if id == ACT_TOOLBAR_REFRESH { lf.refresh();    return Outcome::Rerender; }

            if (ACT_BREADCRUMB_BASE..ACT_TOOLBAR_BACK).contains(&id) {
                let depth = id - ACT_BREADCRUMB_BASE;
                let new_path = if depth == 0 { String::new() } else {
                    take_first_segments(&lf.current, depth as usize)
                };
                lf.navigate(new_path);
                return Outcome::Rerender;
            }

            if (ACT_SIDEBAR_CLICK_BASE..ACT_SIDEBAR_HOVER_BASE).contains(&id) {
                let i = (id - ACT_SIDEBAR_CLICK_BASE) as usize;
                if let Some(p) = lf.sidebar.get(i) {
                    let path = p.path.clone();
                    lf.navigate(path);
                }
                return Outcome::Rerender;
            }

            if (ACT_SIDEBAR_HOVER_BASE..ACT_BREADCRUMB_BASE).contains(&id) {
                // No-op: we only highlight the active sidebar entry,
                // hover doesn't move selection (different from grid).
                return Outcome::Idle;
            }

            if (ACT_GRID_CLICK_BASE..ACT_GRID_HOVER_BASE).contains(&id) {
                let i = (id - ACT_GRID_CLICK_BASE) as usize;
                if i >= lf.entries.len() { return Outcome::Idle; }
                if lf.grid_sel == Some(i) {
                    lf.open_selected();
                } else {
                    lf.grid_sel = Some(i);
                }
                return Outcome::Rerender;
            }

            if (ACT_GRID_HOVER_BASE..ACT_SIDEBAR_CLICK_BASE).contains(&id) {
                let i = (id - ACT_GRID_HOVER_BASE) as usize;
                if i < lf.entries.len() && lf.grid_sel != Some(i) {
                    lf.grid_sel = Some(i);
                    return Outcome::Rerender;
                }
                return Outcome::Idle;
            }

            Outcome::Idle
        }
        _ => Outcome::Idle,
    }
}

// ── Sidebar helpers ────────────────────────────────────────────────────

fn default_sidebar(home: &str) -> Vec<Place> {
    alloc::vec![
        Place { label: "Home".into(),      icon: IconId::Home,       path: home.into() },
        Place { label: "Documents".into(), icon: IconId::FileText,   path: alloc::format!("{}/documents", home) },
        Place { label: "Downloads".into(), icon: IconId::Download,   path: alloc::format!("{}/downloads", home) },
        Place { label: "Pictures".into(),  icon: IconId::Image,      path: alloc::format!("{}/pictures",  home) },
        Place { label: "Projects".into(),  icon: IconId::Folders,    path: alloc::format!("{}/projects",  home) },
        Place { label: "Filesystem".into(), icon: IconId::HardDrives, path: String::new() },
        Place { label: "Trash".into(),     icon: IconId::Trash,      path: alloc::format!("{}/.trash",    home) },
    ]
}

fn is_device(label: &str) -> bool {
    label == "Filesystem" || label == "Trash"
}

// ── Kernel calls ───────────────────────────────────────────────────────

fn read_home_dir() -> String {
    let key = "sys/config/name";
    let buf_ptr = core::ptr::addr_of_mut!(NAME_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(
            key.as_ptr() as i32,
            key.len() as i32,
            buf_ptr as i32,
            NAME_FETCH_CAP as i32,
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
    let buf_ptr = core::ptr::addr_of_mut!(LIST_BUF) as *mut u8;
    let n = unsafe {
        npk_fs_list(
            prefix.as_ptr() as i32,
            prefix.len() as i32,
            buf_ptr as i32,
            LIST_BUF_SIZE as i32,
            0,
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

// Wire: name\0size_le_u64\0is_dir_u8
fn parse_entry(line: &[u8]) -> Option<Entry> {
    let nul = line.iter().position(|&b| b == 0)?;
    let name = core::str::from_utf8(&line[..nul]).ok()?.to_string();
    let rest = &line[nul + 1..];
    if rest.len() < 10 { return None; }
    let size = u64::from_le_bytes(rest[..8].try_into().ok()?);
    // rest[8] is \0 separator, rest[9] is is_dir byte
    let is_dir = rest[9] != 0;
    Some(Entry { name, size, is_dir })
}

// ── Path helpers ───────────────────────────────────────────────────────

fn parent_path(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn take_first_segments(path: &str, n: usize) -> String {
    let mut out = String::new();
    for (i, part) in path.split('/').enumerate() {
        if part.is_empty() { continue; }
        if i >= n { break; }
        if !out.is_empty() { out.push('/'); }
        out.push_str(part);
        if out.split('/').filter(|s| !s.is_empty()).count() == n { break; }
    }
    out
}

// ── Icon heuristic ─────────────────────────────────────────────────────

fn icon_for(e: &Entry) -> IconId {
    if e.is_dir { return IconId::Folder; }
    let name = e.name.as_str();
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "md" | "txt" | "log" | "cfg" | "toml" | "json" | "yaml" | "yml" => IconId::FileText,
        "rs" | "wasm" | "sh" | "py" | "c" | "h" | "hpp" | "cpp" | "go" => IconId::Code,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" => IconId::Image,
        _ => IconId::File,
    }
}

// ── Entry point ────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Deliberately no `npk_window_set_overlay` — that's drun's modal
    // behaviour. Regular apps let the compositor tile them: the first
    // `scene_commit` creates the widget window and retile() places it.
    let mut loft = Loft::new();
    let persistent_mark = alloc_mark();

    commit_tree(&loft);

    loop {
        match poll_event() {
            PollResult::Event(ev) => match handle(&mut loft, ev) {
                Outcome::Idle => {}
                Outcome::Rerender => {
                    alloc_reset(persistent_mark);
                    commit_tree(&loft);
                }
                Outcome::Exit => { close_self(); return; }
            },
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            PollResult::WindowGone => return,
        }
    }
}

fn commit_tree(lf: &Loft) {
    let tree = render(lf);
    match wire::encode(&tree) {
        Ok(bytes) => { if commit(&bytes) < 0 { log("[loft] commit failed"); } }
        Err(_) => log("[loft] encode failed"),
    }
}

