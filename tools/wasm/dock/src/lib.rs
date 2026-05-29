//! dock — bottom auto-hide app dock.
//!
//! A resident overlay launcher. Declares itself a dock via
//! `npk_window_set_dock`; the compositor owns the slide-in/out reveal
//! (cursor at the bottom edge reveals it, leaving hides it). Renders a
//! centred row of app icons plus a trailing launcher button that opens
//! `drun` for full search. Complementary to `drun`, not a replacement.
//!
//! Right-click on an icon opens a context Popover (Unpin / move left /
//! move right). Right-click on the trailing launcher opens an "Add to
//! dock" list of every catalog app not currently pinned. Mutations are
//! persisted to `sys/config/dock` and applied live.
//!
//! See DOCK.md for the architecture.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_catalog::{self, AppEntry, EntryKind};
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
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_spawn_module(ptr: i32, len: i32) -> i32;
    fn npk_run_intent(verb_ptr: i32, verb_len: i32) -> i32;
    fn npk_window_set_dock(w: i32, h: i32) -> i32;
    fn npk_window_set_modal(modal: i32) -> i32;
    fn npk_get_fb_size() -> i64;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32); }
}

fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}

fn spawn(name: &str) -> bool {
    unsafe { npk_spawn_module(name.as_ptr() as i32, name.len() as i32) == 0 }
}

fn run_intent(verb: &str) -> bool {
    unsafe { npk_run_intent(verb.as_ptr() as i32, verb.len() as i32) == 0 }
}

fn store(path: &str, data: &[u8]) -> bool {
    unsafe {
        npk_store(
            path.as_ptr() as i32, path.len() as i32,
            data.as_ptr() as i32, data.len() as i32,
        ) == 0
    }
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

// Bump allocator — same pattern as drun. Bumped to 512 KB because we
// now hold the full catalog (every installed app) for the Add-to-dock
// submenu, and may re-render multiple times per session.
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

fn alloc_mark() -> usize {
    unsafe { core::ptr::addr_of!(HEAP_POS).read() }
}

fn alloc_reset(pos: usize) {
    unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(pos); }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[dock] panic!");
    loop {}
}

// ── ActionIds + NodeIds ───────────────────────────────────────────────
// Ranges are far apart so a future reshuffle can't collide silently.
const CLICK_BASE:   u32 = 1;            // 1..LAUNCHER : launch cell idx
const LAUNCHER:     u32 = 90_000;       // open drun
const NODE_CELL:    u32 = 100_000;      // NODE_CELL+idx : anchor for cell idx
const NODE_LAUNCHER:u32 = 199_000;      // anchor for trailing launcher

const MENU_UNPIN:      u32 = 200_000;
const MENU_MOVE_LEFT:  u32 = 200_001;
const MENU_MOVE_RIGHT: u32 = 200_002;
const MENU_DISMISS:    u32 = 200_003;
const ADD_BASE:        u32 = 300_000;   // ADD_BASE+catalog_idx : add to dock

// Visual sizing (px at 1× scale). Kept low + tight for a flat, floating
// tray; the compositor draws it translucent with a gap from the edge.
const ICON_SIZE: u16 = 24;
const DOCK_HEIGHT: i32 = 48;
const CELL_FOOTPRINT: i32 = 46; // icon + padding + inter-cell gap
const SIDE_PADDING: i32 = 28;
/// Approximate compositor `DOCK_BOTTOM_GAP * scale` (kernel default is 12,
/// HiDPI scale 2× → 24). Subtracted from the expanded window height so
/// the visible bottom gap is preserved when a menu is open.
const DOCK_GAP_RESERVE: i32 = 24;

const DOCK_CFG_PATH: &str = "sys/config/dock";

// ── State ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum OpenMenu {
    /// Context menu for the cell at `idx` (Unpin / move left / move right).
    IconCtx(usize),
    /// "Add to dock" submenu showing every catalog app not yet pinned.
    AddApp,
}

struct Dock {
    /// Currently visible icon cells. Order = render order.
    entries:    Vec<AppEntry>,
    /// Every catalog entry (incl. pinned) — used to populate Add submenu.
    /// Cached at load + after each mutation so we don't refetch + re-hydrate
    /// the whole catalog on every right-click.
    catalog:    Vec<AppEntry>,
    /// Right-click context menu state.
    open:       Option<OpenMenu>,
    /// Screen height, fetched once at startup. Used to expand the dock
    /// window when a menu is open so the popover has room above the tray
    /// and click-outside lands inside the (now-large) dock window.
    screen_h:   i32,
}

impl Dock {
    fn load() -> Self {
        // Exclude the dock itself and drun — the trailing launcher
        // button already opens drun, so a separate drun tile is redundant.
        let catalog = app_catalog::load(&["dock", "drun", "bar"]);
        let initial: Vec<AppEntry> = match read_pins() {
            Some(pins) if !pins.is_empty() => order_by_pins(&catalog, &pins),
            _ => catalog.clone(),
        };
        // Pre-allocate `entries` to the full catalog size so subsequent
        // Add/Remove mutations never re-allocate behind the persistent
        // bump mark (which would leak the Vec buffer on every alloc_reset).
        let mut entries: Vec<AppEntry> = Vec::with_capacity(catalog.len() + 1);
        entries.extend(initial);
        let packed = unsafe { npk_get_fb_size() };
        let screen_h = (packed & 0xFFFF_FFFF) as i32;
        Dock { entries, catalog, open: None, screen_h }
    }

    /// Total dock width to request from the compositor (it clamps).
    fn width(&self) -> i32 {
        let cells = self.entries.len() as i32 + 1; // + launcher button
        cells * CELL_FOOTPRINT + SIDE_PADDING
    }

    fn render(&self) -> Widget {
        let mut cells: Vec<Widget> = Vec::with_capacity(self.entries.len() + 3);
        // Leading flex spacer centres the icon group on the main axis
        // (the row fills the full tray width, icons don't left-pack).
        cells.push(Widget::Spacer { flex: 1 });
        for (i, e) in self.entries.iter().enumerate() {
            cells.push(icon_cell(
                e.icon,
                ICON_SIZE,
                ActionId(CLICK_BASE + i as u32),
                NodeId(NODE_CELL + i as u32),
            ));
        }
        // Trailing launcher button → drun (full search). Right-click on
        // it opens the Add-to-dock submenu.
        cells.push(icon_cell(
            IconId::MagnifyingGlass,
            ICON_SIZE,
            ActionId(LAUNCHER),
            NodeId(NODE_LAUNCHER),
        ));
        cells.push(Widget::Spacer { flex: 1 });

        // The tray: a rounded SurfaceElevated pill holding the icons. No row
        // Padding — icons carry their own; a padded root would be double-
        // counted by the layout pass and collapse the vertical centring on a
        // short tray. Spacers centre the icons horizontally.
        let tray = Widget::Row {
            children:  cells,
            spacing:   Spacing::Sm.as_u16(),
            align:     Align::Center,
            modifiers: alloc::vec![
                Modifier::Background(Token::SurfaceElevated),
                Modifier::Rounded(Radius::Lg.as_u8()),
            ],
        };

        // When a menu is open the window is grown to the full screen height
        // (see `apply_window_size`) so the popover has room above and clicks
        // anywhere outside land inside the dock window → on_dismiss fires.
        // Push the tray to the bottom of that tall window via a Column with
        // a flex spacer; when no menu is open the column collapses to the
        // tray alone in the small (DOCK_HEIGHT) window.
        let tray_col = Widget::Column {
            children:  alloc::vec![Widget::Spacer { flex: 1 }, tray],
            spacing:   Spacing::None.as_u16(),
            align:     Align::Stretch,
            modifiers: alloc::vec![],
        };
        let mut stack_children: Vec<Widget> = alloc::vec![tray_col];
        if let Some(menu) = self.open {
            stack_children.push(self.render_popover(menu));
        }

        // Wrap in a Stack so the compositor renders the dock as a translucent
        // panel scene (transparent clear + chrome-opacity background, crisp
        // icons composited by alpha — no halo). Same path as the bar.
        Widget::Stack {
            children:  stack_children,
            modifiers: Vec::new(),
        }
    }

    /// Tell the compositor what size the dock window should be. Called
    /// after every state change. When a menu is open we expand toward
    /// the full screen height so the popover has room and click-outside
    /// lands inside the dock window; auto-hide naturally pauses because
    /// the reveal hot-zone becomes the entire screen. We undershoot by
    /// `DOCK_GAP_RESERVE` so the visible bottom gap from the screen edge
    /// is preserved when the window is `shown` (win.y = baseline - dh -
    /// gap, the dock tray then floats above the bottom edge).
    fn apply_window_size(&self) {
        let h = if self.open.is_some() {
            (self.screen_h - DOCK_GAP_RESERVE).max(DOCK_HEIGHT)
        } else {
            DOCK_HEIGHT
        };
        unsafe { let _ = npk_window_set_dock(self.width(), h); }
    }

    fn render_popover(&self, menu: OpenMenu) -> Widget {
        let (anchor, content) = match menu {
            OpenMenu::IconCtx(idx) => {
                // Include the app's display name in the unpin label so users
                // don't have to recognise the icon to know which app they're
                // about to remove. Falls back to the launch name if the
                // catalog never hydrated a display name (rare).
                let name = self.entries.get(idx)
                    .map(|e| if e.display_name.is_empty() {
                        e.launch_name.clone()
                    } else {
                        e.display_name.clone()
                    })
                    .unwrap_or_default();
                let unpin_label = if name.is_empty() {
                    "Vom Dock entfernen".to_string()
                } else {
                    alloc::format!("{} vom Dock entfernen", name)
                };
                let mut items: Vec<(String, ActionId)> = Vec::with_capacity(3);
                items.push((unpin_label, ActionId(MENU_UNPIN)));
                if idx > 0 {
                    items.push(("Nach links".to_string(),
                                ActionId(MENU_MOVE_LEFT)));
                }
                if idx + 1 < self.entries.len() {
                    items.push(("Nach rechts".to_string(),
                                ActionId(MENU_MOVE_RIGHT)));
                }
                (NodeId(NODE_CELL + idx as u32),
                 prefab::popover_menu(&items, None))
            }
            OpenMenu::AddApp => {
                // Every catalog entry not currently pinned.
                let pinned: alloc::collections::BTreeSet<&str> = self.entries
                    .iter().map(|e| e.launch_name.as_str()).collect();
                let mut items: Vec<(String, ActionId)> = Vec::new();
                for (i, e) in self.catalog.iter().enumerate() {
                    if pinned.contains(e.launch_name.as_str()) { continue; }
                    items.push((e.display_name.clone(),
                                ActionId(ADD_BASE + i as u32)));
                }
                if items.is_empty() {
                    items.push(("(alle Apps schon im Dock)".to_string(),
                                ActionId(MENU_DISMISS)));
                }
                (NodeId(NODE_LAUNCHER),
                 prefab::popover_menu(&items, None))
            }
        };
        Widget::Popover {
            anchor,
            child:      alloc::boxed::Box::new(content),
            on_dismiss: ActionId(MENU_DISMISS),
            modifiers:  alloc::vec![],
        }
    }

    fn commit_tree(&self) {
        match wire::encode(&self.render()) {
            Ok(bytes) => { if commit(&bytes) < 0 { log("[dock] commit failed"); } }
            Err(_) => log("[dock] encode failed"),
        }
    }

    fn launch(&self, idx: usize) {
        if let Some(e) = self.entries.get(idx) {
            let ok = match e.kind {
                EntryKind::Module => spawn(&e.launch_name),
                EntryKind::Intent => run_intent(&e.launch_name),
            };
            if !ok { log("[dock] launch failed"); }
        }
    }

    fn handle(&mut self, ev: Event) -> bool {
        match ev {
            Event::Action(ActionId(id)) => self.on_action(id),
            Event::ContextAction(ActionId(id)) => self.on_context(id),
            _ => false,
        }
    }

    fn on_action(&mut self, id: u32) -> bool {
        // Menu actions take priority — they only fire while a popover is
        // open and route to a state mutation + close.
        match id {
            MENU_DISMISS => {
                self.open = None;
                return true;
            }
            MENU_UNPIN => {
                if let Some(OpenMenu::IconCtx(idx)) = self.open {
                    if idx < self.entries.len() {
                        self.entries.remove(idx);
                        self.persist();
                    }
                }
                self.open = None;
                return true;
            }
            MENU_MOVE_LEFT => {
                if let Some(OpenMenu::IconCtx(idx)) = self.open {
                    if idx > 0 && idx < self.entries.len() {
                        self.entries.swap(idx - 1, idx);
                        self.persist();
                    }
                }
                self.open = None;
                return true;
            }
            MENU_MOVE_RIGHT => {
                if let Some(OpenMenu::IconCtx(idx)) = self.open {
                    if idx + 1 < self.entries.len() {
                        self.entries.swap(idx, idx + 1);
                        self.persist();
                    }
                }
                self.open = None;
                return true;
            }
            _ if id >= ADD_BASE && (id - ADD_BASE) < self.catalog.len() as u32 => {
                let cidx = (id - ADD_BASE) as usize;
                let entry = self.catalog[cidx].clone();
                // Guard against double-add.
                let already = self.entries.iter()
                    .any(|e| e.launch_name == entry.launch_name);
                if !already {
                    self.entries.push(entry);
                    self.persist();
                }
                self.open = None;
                return true;
            }
            _ => {}
        }
        // Regular launch click — but close any open menu first.
        if self.open.is_some() {
            self.open = None;
            return true;
        }
        if id == LAUNCHER {
            let _ = spawn("drun");
        } else if id >= CLICK_BASE && id < LAUNCHER {
            self.launch((id - CLICK_BASE) as usize);
        }
        false
    }

    fn on_context(&mut self, id: u32) -> bool {
        if id == LAUNCHER {
            self.open = Some(OpenMenu::AddApp);
            return true;
        }
        if id >= CLICK_BASE && id < LAUNCHER {
            let idx = (id - CLICK_BASE) as usize;
            if idx < self.entries.len() {
                self.open = Some(OpenMenu::IconCtx(idx));
                return true;
            }
        }
        false
    }

    /// Write the current pin order to `sys/config/dock` — one launch_name
    /// per line. Errors are logged but non-fatal: the in-memory ordering
    /// remains; next boot just falls back to the previous file.
    fn persist(&self) {
        let mut buf = String::with_capacity(self.entries.len() * 16);
        for e in &self.entries {
            buf.push_str(&e.launch_name);
            buf.push('\n');
        }
        if !store(DOCK_CFG_PATH, buf.as_bytes()) {
            log("[dock] persist failed");
        }
    }
}

/// Single dock cell — `Widget::Icon` with hover + click + a NodeId
/// anchor so the right-click popover can attach to it.
fn icon_cell(icon: IconId, size: u16, click: ActionId, anchor: NodeId) -> Widget {
    let mods: Vec<Modifier> = alloc::vec![
        Modifier::Padding(Padding::Sm.as_u16()),
        Modifier::OnClick(click),
        Modifier::NodeId(anchor),
        Modifier::Hover(alloc::vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Rounded(Radius::Sm.as_u8()),
        ]),
    ];
    Widget::Icon { id: icon, size, modifiers: mods }
}

/// Read `sys/config/dock` — one app name (module or intent) per line.
/// Missing / empty → None (caller falls back to the full catalog).
fn read_pins() -> Option<Vec<String>> {
    const CFG_BUF_SIZE: usize = 4096;
    static mut CFG_BUF: [u8; CFG_BUF_SIZE] = [0; CFG_BUF_SIZE];
    let buf_ptr = core::ptr::addr_of_mut!(CFG_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(DOCK_CFG_PATH.as_ptr() as i32, DOCK_CFG_PATH.len() as i32,
                  buf_ptr as i32, CFG_BUF_SIZE as i32)
    };
    if n <= 0 { return None; }
    let bytes = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    let text = core::str::from_utf8(bytes).ok()?;
    let mut pins: Vec<String> = Vec::new();
    for line in text.lines() {
        let name = line.trim();
        if !name.is_empty() && !name.starts_with('#') {
            pins.push(name.to_string());
        }
    }
    Some(pins)
}

/// Keep only pinned entries, in pin order. Unmatched pins are skipped.
fn order_by_pins(catalog: &[AppEntry], pins: &[String]) -> Vec<AppEntry> {
    let mut out: Vec<AppEntry> = Vec::with_capacity(pins.len());
    for pin in pins {
        if let Some(e) = catalog.iter().find(|e| &e.launch_name == pin) {
            out.push(e.clone());
        }
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut dock = Dock::load();

    unsafe { let _ = npk_window_set_modal(0); }
    dock.apply_window_size();

    let persistent_mark = alloc_mark();
    dock.commit_tree();

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                let dirty = dock.handle(ev);
                if dirty {
                    // Window may need to grow/shrink as the menu opens or
                    // closes. set_dock is idempotent at the same size, so
                    // unconditionally calling it here costs nothing extra.
                    dock.apply_window_size();
                    // Per-frame Vecs from render() live past the mark —
                    // reset and rebuild so the heap doesn't grow on every
                    // mutation. entries/catalog were allocated before mark
                    // with capacity headroom, so they survive.
                    alloc_reset(persistent_mark);
                    dock.commit_tree();
                }
            }
            PollResult::Empty => { unsafe { let _ = npk_sleep(16); } }
            PollResult::WindowGone => return,
        }
    }
}
