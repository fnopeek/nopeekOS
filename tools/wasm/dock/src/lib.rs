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
//! See docs/archive/DOCK.md for the architecture.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_catalog::{self, AppEntry, EntryKind};
use nopeek_widgets::i18n;
use nopeek_widgets::prefab;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Declared capabilities: read (catalog) + write (persist dock config) +
// exec (launch apps/intents) + render, and the shell roles (the dock panel,
// modal while a menu is open). The kernel grants exactly this.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::WRITE | caps::EXEC | caps::RENDER, caps::ext::SHELL];

use nopeek_widgets::host;

// The calls only the dock makes; the shared ones are in
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_window_set_dock(w: i32, h: i32) -> i32;
    fn npk_get_fb_size() -> i64;
}

fn window_set_dock(w: i32, h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_dock(w, h) };
}

/// Framebuffer height in pixels (the low half of `npk_get_fb_size`).
fn fb_height() -> i32 {
    // SAFETY: FFI without pointers.
    let packed = unsafe { npk_get_fb_size() };
    (packed & 0xFFFF_FFFF) as i32
}

// ── Strings ───────────────────────────────────────────────────────────
// English is the source language. A new language is one more `const`
// here — see `nopeek_widgets::i18n`.

struct Strings {
    unpin_named:  &'static str,
    unpin:        &'static str,
    move_entry:   &'static str,
    all_pinned:   &'static str,
}

const EN: Strings = Strings {
    unpin_named: "Remove {} from dock",
    unpin:       "Remove from dock",
    move_entry:  "Move",
    all_pinned:  "(every app is already pinned)",
};

const DE: Strings = Strings {
    unpin_named: "{} vom Dock entfernen",
    unpin:       "Vom Dock entfernen",
    move_entry:  "Verschieben",
    all_pinned:  "(alle Apps schon im Dock)",
};

fn s() -> &'static Strings {
    match i18n::lang() { Lang::De => &DE, _ => &EN }
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

fn log(msg: &str) { host::log_serial(msg); }

fn spawn(name: &str) -> bool { host::spawn_module(name) }

fn run_intent(verb: &str) -> bool { host::run_intent(verb) == 0 }

const EVENT_BUF_SIZE: usize = 64;

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
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[dock] panic!");
    core::arch::wasm32::unreachable()
}

// ── ActionIds + NodeIds ───────────────────────────────────────────────
// Ranges are far apart so a future reshuffle can't collide silently.
const CLICK_BASE:   u32 = 1;            // 1..HOVER_BASE : launch cell idx
const HOVER_BASE:   u32 = 50_000;       // HOVER_BASE+idx : OnHover for cell idx
const LAUNCHER:     u32 = 90_000;       // open drun (also: hover target N)
const NODE_CELL:    u32 = 100_000;      // NODE_CELL+idx : anchor for cell idx
const NODE_LAUNCHER:u32 = 199_000;      // anchor for trailing launcher

const MENU_UNPIN:   u32 = 200_000;
const MENU_MOVE:    u32 = 200_001;      // enter drag-to-reorder mode
const MENU_DISMISS: u32 = 200_003;
const ADD_BASE:     u32 = 300_000;      // ADD_BASE+catalog_idx : add to dock

// Visual sizing (px at 1× scale) — docs/spec/UI_REFRESH.md §3 "dock".
/// Glyph inside a cell tile.
const ICON_SIZE: u16 = 24;
/// Hover zoom, Q8.8. 24 × 342 / 256 = 32 px: an atlas size, so the
/// enlarged glyph is blitted 1:1 instead of resampled, and it still fits
/// the 34 px tile.
const HOVER_SCALE: u16 = 342;
/// Square tile the glyph sits in.
const CELL_BOX: u16 = 34;
/// Corner radius of that tile.
const CELL_RADIUS: u8 = 9;
/// Running-indicator dash: full width when the app is focused, a stub
/// when it merely runs, invisible (but space-holding) when it doesn't.
const DASH_W_ACTIVE: u16 = 12;
const DASH_W_RUNNING: u16 = 3;
const DASH_H: u16 = 2;
/// Balancing mark above the tile — see `icon_cell`. Invisible but
/// space-holding, like the dash of an app that is not running.
const CELL_LIFT: u16 = 2;
/// Gap between the three parts of the cell. It appears twice in the column
/// (mark|tile and tile|dash), hence 1 rather than `Spacing::Xs`.
const CELL_GAP: u16 = 1;
/// Tile + gap + dash → the cell column's height.
const DOCK_HEIGHT: i32 = 50;
const CELL_FOOTPRINT: i32 = 36; // tile + inter-cell gap
/// Horizontal padding inside the tray — room for the pill's corner arc.
/// Computed: the tightest spot needs 4.8 px (see `render`), 12 leaves
/// 7.2 px to spare.
const TRAY_PAD_X: u16 = 12;
/// Vertical padding. Sets the tray height (40 + 2x4 = 48) and with it the
/// pill radius (24). Redo the arithmetic in `render` before changing it.
const TRAY_PAD_Y: u16 = 4;
/// Window width = cells + this. Includes twice `TRAY_PAD_X`, otherwise the
/// arc takes space from the cells.
const SIDE_PADDING: i32 = 24 + 2 * TRAY_PAD_X as i32;
/// Approximate compositor `DOCK_BOTTOM_GAP * scale` (kernel default is 12,
/// HiDPI scale 2× → 24). Subtracted from the expanded window height so
/// the visible bottom gap is preserved when a menu is open.
const DOCK_GAP_RESERVE: i32 = 24;

/// Room for the compositor's window list.
const TITLES_CAP: usize = 2048;

const DOCK_CFG_PATH: &str = "sys/config/dock";
/// Header line written by `persist`. Its presence (returned by
/// `read_pins`) tells the next boot the user has touched the config —
/// an otherwise-empty body then means "user wants an empty dock" and
/// the catalog fallback is suppressed.
const DOCK_CFG_MARKER: &str = "# nopeekos dock v1";

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
    /// While `Some(launch_name)`, the dock is in drag-to-reorder mode:
    /// hovering another cell live-shuffles the moving entry to that slot,
    /// and the next click of any kind exits + persists. Tracked by name
    /// (stable across remove+insert) instead of index.
    moving:     Option<String>,
    /// True for one MouseButton{Left, down} after entering drag-reorder
    /// mode — the compositor pushes Action(MENU_MOVE) and the raw left-down
    /// for the same physical click, so without this flag the down would
    /// immediately re-exit the drag we just entered.
    suppress_next_press: bool,
    /// Screen height, fetched once at startup. Used to expand the dock
    /// window when a menu is open so the popover has room above the tray
    /// and click-outside lands inside the (now-large) dock window.
    screen_h:   i32,
    /// The compositor's window list, as last read by `refresh_titles`.
    titles:     Vec<u8>,
}

impl Dock {
    fn load() -> Self {
        // Exclude the dock itself and drun — the trailing launcher
        // button already opens drun, so a separate drun tile is redundant.
        let catalog = app_catalog::load(&["dock", "drun", "bar", "pick"]);
        let initial: Vec<AppEntry> = match read_pins() {
            // File exists (incl. empty) → honour the user's choice,
            // even if it means an empty dock.
            Some(pins) => order_by_pins(&catalog, &pins),
            // File never written → first boot, seed from the catalog.
            None => catalog.clone(),
        };
        let mut entries: Vec<AppEntry> = Vec::with_capacity(catalog.len() + 1);
        entries.extend(initial);
        Dock {
            entries, catalog,
            open: None, moving: None, suppress_next_press: false,
            screen_h: fb_height(),
            titles: Vec::new(),
        }
    }

    /// Total dock width to request from the compositor (it clamps).
    fn width(&self) -> i32 {
        let cells = self.entries.len() as i32 + 1; // + launcher button
        cells * CELL_FOOTPRINT + SIDE_PADDING
    }

    fn render(&self) -> Widget {
        let mut cells: Vec<Widget> = Vec::with_capacity(self.entries.len() + 4);
        // Leading flex spacer centres the icon group on the main axis
        // (the row fills the full tray width, icons don't left-pack).
        cells.push(Widget::Spacer { flex: 1 });
        for (i, e) in self.entries.iter().enumerate() {
            cells.push(icon_cell(
                e.icon,
                ActionId(CLICK_BASE + i as u32),
                ActionId(HOVER_BASE + i as u32),
                NodeId(NODE_CELL + i as u32),
                run_state_of(self.titles_text(), &e.launch_name),
            ));
        }
        cells.push(separator());
        // Trailing launcher button → drun (full search). Right-click on
        // it opens the Add-to-dock submenu. Hover with HOVER_BASE+N (where
        // N == entries.len()) so a drag-reorder can move past the last
        // pinned slot, dropping the moving entry at the end of the list.
        cells.push(icon_cell(
            IconId::MagnifyingGlass,
            ActionId(LAUNCHER),
            ActionId(HOVER_BASE + self.entries.len() as u32),
            NodeId(NODE_LAUNCHER),
            RunState::Idle,
        ));
        cells.push(Widget::Spacer { flex: 1 });

        // The tray: a SurfaceElevated pill holding the icons.
        // TRAY_PAD_Y (4 px) top and bottom bumps the Row's intrinsic height
        // from (icon+OnHover-pad) = 40 to a full 48 px → matches DOCK_HEIGHT
        // in the idle window and keeps the visible tray the same size when
        // the menu-expand wraps it in a bottom-anchored Column (whose Spacer
        // would otherwise let the tray collapse to its intrinsic 40 px and
        // make the pill look shorter on right-click).
        let tray = Widget::Row {
            children:  cells,
            spacing:   Spacing::Xxs.as_u16(),
            align:     Align::Center,
            modifiers: alloc::vec![
                Modifier::Background(Token::SurfaceElevated),
                // Pill = fully round: the rasterizer clamps the radius to
                // `min(w/2, h/2)`, so at 48 px height true semicircles.
                Modifier::Rounded(Radius::Pill.as_u8()),
                // More padding horizontally than vertically, on purpose.
                //
                // The corner arc eats horizontal space, most of all not at the
                // very top but where the pill's arc and the tile's arc run into
                // each other. For a 48 px tray and a 34 px tile with radius 9 the
                // tightest spot is at y = 6.4 px and needs 4.8 px; less and the
                // pill cuts into the first and last tile, visible on hover.
                //
                // The centring spacers usually leave more slack, but pinning
                // another icon consumes it — so 12 px explicitly, independent of
                // the centring. Vertically it stays 4: the height sets the pill
                // radius, and 48 px is also DOCK_HEIGHT.
                Modifier::PaddingXY { x: TRAY_PAD_X, y: TRAY_PAD_Y },
            ],
        };

        // When the window is expanded (menu open or drag-reorder active),
        // the tray must be pushed to the bottom — wrap it in a Column with
        // a flex spacer above. In the normal small window, leave the tray
        // as the direct Stack child so it fills the full DOCK_HEIGHT pill
        // (a Column with a Spacer would otherwise leave the tray at its
        // intrinsic ~40 px and add transparent slack on top, making the
        // visible pill look shorter).
        let expanded = self.open.is_some() || self.moving.is_some();
        let bottom_anchored: Widget = if expanded {
            Widget::Column {
                children:  alloc::vec![Widget::Spacer { flex: 1 }, tray],
                spacing:   Spacing::None.as_u16(),
                align:     Align::Stretch,
                modifiers: alloc::vec![],
            }
        } else {
            tray
        };
        let mut stack_children: Vec<Widget> = alloc::vec![bottom_anchored];
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
        // Stay expanded while the user is mid-drag too, so any click
        // (incl. on empty area) lands inside the dock window and exits
        // the move cleanly.
        let h = if self.open.is_some() || self.moving.is_some() {
            (self.screen_h - DOCK_GAP_RESERVE).max(DOCK_HEIGHT)
        } else {
            DOCK_HEIGHT
        };
        window_set_dock(self.width(), h);
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
                    s().unpin.to_string()
                } else {
                    fill(s().unpin_named, &name)
                };
                let mut items: Vec<(String, ActionId)> = Vec::with_capacity(2);
                items.push((unpin_label, ActionId(MENU_UNPIN)));
                // Moving only makes sense when there's somewhere to move
                // TO — at least two pinned entries.
                if self.entries.len() > 1 {
                    items.push((s().move_entry.to_string(), ActionId(MENU_MOVE)));
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
                    items.push((s().all_pinned.to_string(), ActionId(MENU_DISMISS)));
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
            Ok(bytes) => { if !host::scene_commit(&bytes) { log("[dock] commit failed"); } }
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
            // A press anywhere — even on the empty area of an expanded
            // window — exits an active drag-reorder. The hit-tested
            // Action(...) above already handles clicks that land on a
            // cell; this catches the in-between case (clicks on the
            // transparent expand-region with no hit-tested target).
            Event::MouseButton { button: MouseButton::Left, down: true, .. } => {
                if self.suppress_next_press {
                    // This is the down half of the click that just
                    // entered drag mode via Action(MENU_MOVE) — swallow
                    // it so we don't immediately exit. The NEXT press
                    // commits.
                    self.suppress_next_press = false;
                    false
                } else if self.moving.is_some() {
                    self.exit_move()
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn on_action(&mut self, id: u32) -> bool {
        // While dragging: a hover over another cell shuffles the moving
        // entry into that slot. Any non-hover Action ends the drag.
        if self.moving.is_some() {
            if id >= HOVER_BASE && id < LAUNCHER {
                let target = (id - HOVER_BASE) as usize;
                self.reorder_moving_to(target);
                return true;
            }
            // Any other Action (a click on a cell, MENU_DISMISS, …) means
            // the user is done dragging.
            return self.exit_move();
        }

        // Menu actions — only fire while a popover is open.
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
            MENU_MOVE => {
                if let Some(OpenMenu::IconCtx(idx)) = self.open {
                    if let Some(e) = self.entries.get(idx) {
                        self.moving = Some(e.launch_name.clone());
                        // The MouseButton{down:true} paired with this
                        // very click is still queued — tag it to be
                        // ignored so the drag we just armed survives.
                        self.suppress_next_press = true;
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
        // Hover events while no popover is open and no drag is active —
        // ignore (the cells re-render their own hover modifier).
        if id >= HOVER_BASE && id < LAUNCHER {
            return false;
        }
        // Regular launch click — but close any open menu first.
        if self.open.is_some() {
            self.open = None;
            return true;
        }
        if id == LAUNCHER {
            let _ = spawn("drun");
        } else if id >= CLICK_BASE && id < HOVER_BASE {
            self.launch((id - CLICK_BASE) as usize);
        }
        false
    }

    fn on_context(&mut self, id: u32) -> bool {
        // Right-click during a drag commits at the current position too —
        // no separate cancel, the user accepts wherever the entry sits now.
        if self.moving.is_some() {
            return self.exit_move();
        }
        if id == LAUNCHER {
            self.open = Some(OpenMenu::AddApp);
            return true;
        }
        if id >= CLICK_BASE && id < HOVER_BASE {
            let idx = (id - CLICK_BASE) as usize;
            if idx < self.entries.len() {
                self.open = Some(OpenMenu::IconCtx(idx));
                return true;
            }
        }
        false
    }

    /// Shuffle the moving entry to `target` (0..=entries.len() — the
    /// launcher slot maps to entries.len(), i.e. "end of list").
    fn reorder_moving_to(&mut self, target: usize) {
        let Some(name) = self.moving.clone() else { return };
        let Some(cur) = self.entries.iter().position(|e| e.launch_name == name) else {
            // The moving entry was removed somehow — drop the mode.
            self.moving = None;
            return;
        };
        let dest = target.min(self.entries.len().saturating_sub(1));
        if cur == dest { return; }
        let entry = self.entries.remove(cur);
        let insert_at = dest.min(self.entries.len());
        self.entries.insert(insert_at, entry);
    }

    /// Exit drag mode and write the new order to disk. Returns true so
    /// the main loop re-renders + shrinks the window.
    fn exit_move(&mut self) -> bool {
        if self.moving.take().is_some() {
            self.persist();
            true
        } else {
            false
        }
    }

    /// Write the current pin order to `sys/config/dock` — one launch_name
    /// per line. A leading marker line is written even when entries is
    /// empty so that "no apps pinned" is distinguishable from "file
    /// never existed" on the next boot. Errors are logged but non-fatal.
    /// Re-read the window list. Returns true when it differs from the last
    /// read — the dock re-renders only then, so polling stays cheap.
    fn refresh_titles(&mut self) -> bool {
        let mut scratch = [0u8; TITLES_CAP];
        let len = host::window_titles(&mut scratch).unwrap_or(0).min(TITLES_CAP);
        if self.titles[..] == scratch[..len] {
            return false;
        }
        self.titles.clear();
        self.titles.extend_from_slice(&scratch[..len]);
        true
    }

    fn titles_text(&self) -> &str {
        core::str::from_utf8(&self.titles).unwrap_or("")
    }

    fn persist(&self) {
        let mut buf = String::with_capacity(self.entries.len() * 16 + 32);
        buf.push_str(DOCK_CFG_MARKER);
        buf.push('\n');
        for e in &self.entries {
            buf.push_str(&e.launch_name);
            buf.push('\n');
        }
        if !host::store(DOCK_CFG_PATH, buf.as_bytes()) {
            log("[dock] persist failed");
        }
    }
}

/// How an app in the dock relates to the current window set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunState {
    /// No window open.
    Idle,
    /// Has a window somewhere.
    Running,
    /// Has the focused window.
    Active,
}

/// Single dock cell — a tile holding the glyph, with the running
/// indicator dash below it. Hover enlarges the glyph and paints it in
/// the accent; the tile background is reserved for the app that owns the
/// focus, so the two cues never mean the same thing. The `hover` ActionId also drives
/// the drag-reorder live shuffle, and the NodeId anchors the
/// right-click popover.
fn icon_cell(
    icon: IconId,
    click: ActionId,
    hover: ActionId,
    anchor: NodeId,
    run: RunState,
) -> Widget {
    let mut tile_mods: Vec<Modifier> = alloc::vec![
        Modifier::MinWidth(CELL_BOX),
        Modifier::MinHeight(CELL_BOX),
        Modifier::Rounded(CELL_RADIUS),
        Modifier::OnClick(click),
        Modifier::OnHover(hover),
        Modifier::NodeId(anchor),
        Modifier::Hover(alloc::vec![
            Modifier::Scale(HOVER_SCALE),
            Modifier::Tint(Token::Accent),
        ]),
    ];
    let mut glyph_mods: Vec<Modifier> = Vec::new();
    if run == RunState::Active {
        tile_mods.push(Modifier::Background(Token::SurfaceHover));
        glyph_mods.push(Modifier::Tint(Token::Accent));
    }

    let tile = prefab::center_box(
        Widget::Icon { id: icon, size: ICON_SIZE, modifiers: glyph_mods },
        tile_mods,
    );

    let dash = match run {
        RunState::Active  => prefab::mark(DASH_W_ACTIVE,  DASH_H, Some(Token::Accent)),
        RunState::Running => prefab::mark(DASH_W_RUNNING, DASH_H, Some(Token::OnSurfaceFaint)),
        RunState::Idle    => prefab::mark(DASH_W_RUNNING, DASH_H, None),
    };

    // Without balancing, the tile sits too high: below it are the gap
    // and the dash, above it nothing. For an app that is not running
    // the dash is invisible but space-holding, so the gap shows as
    // empty space under the icon.
    //
    // An equally invisible mark on top balances it. The column must
    // stay at 40 (otherwise the tray grows and with it the pill
    // radius), and the gap counts twice — between mark and tile as
    // well as between tile and dash —
    //
    //     M + 2*S + 34 + 2 = 40   with the icon centred
    //
    // has exactly one integer solution: M = 2, S = 1. That gives
    // 12 px above and below the icon and a centred hover tile, with
    // the dash on the same pixel.
    Widget::Column {
        children:  alloc::vec![prefab::mark(1, CELL_LIFT, None), tile, dash],
        spacing:   CELL_GAP,
        align:     Align::Center,
        modifiers: Vec::new(),
    }
}

/// Vertical hairline between the pinned apps and the trailing launcher.
fn separator() -> Widget {
    Widget::Row {
        children:  alloc::vec![prefab::mark(1, 20, Some(Token::Border))],
        spacing:   0,
        align:     Align::Center,
        modifiers: alloc::vec![Modifier::Padding(Padding::Xs.as_u16())],
    }
}

/// Window titles carry the module name, so a dock entry's `launch_name`
/// matches a window line directly.
fn run_state_of(titles: &str, launch_name: &str) -> RunState {
    let mut state = RunState::Idle;
    for line in titles.lines() {
        let mut cols = line.split('\t');
        let (Some(flags), Some(_ws), Some(title)) = (cols.next(), cols.next(), cols.next())
            else { continue };
        if title.trim() != launch_name { continue; }
        let flags: u8 = flags.trim().parse().unwrap_or(0);
        if flags & 1 != 0 { return RunState::Active; }
        state = RunState::Running;
    }
    state
}

/// Read `sys/config/dock` — one app name (module or intent) per line.
/// Missing / empty → None (caller falls back to the full catalog).
fn read_pins() -> Option<Vec<String>> {
    const CFG_BUF_SIZE: usize = 4096;
    let mut buf = [0u8; CFG_BUF_SIZE];
    let n = host::fetch(DOCK_CFG_PATH, &mut buf).unwrap_or(0);
    if n == 0 { return None; }
    let text = core::str::from_utf8(&buf[..n]).ok()?;
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

    host::window_set_modal(false);
    dock.apply_window_size();

    dock.refresh_titles();
    dock.commit_tree();

    let mut event_buf = [0u8; EVENT_BUF_SIZE];
    loop {
        match poll_event(&mut event_buf) {
            PollResult::Event(ev) => {
                let dirty = dock.handle(ev);
                if dirty {
                    // Window may need to grow/shrink as the menu opens or
                    // closes. set_dock is idempotent at the same size, so
                    // unconditionally calling it here costs nothing extra.
                    dock.apply_window_size();
                    dock.commit_tree();
                }
            }
            PollResult::Empty => {
                // Focus and window opens/closes happen elsewhere; re-read
                // the window list and re-render only when the running
                // indicators changed.
                if dock.refresh_titles() {
                    dock.commit_tree();
                }
                // Then wait to be told. The kernel wakes the dock on an event
                // (hover, click) or a window change (`WAIT_STATE`, the
                // compositor's fingerprint after each frame). No deadline: an
                // indicator that goes stale means a change nobody reported, and
                // that must show, not be papered over by a timer.
                // docs/plan/CORES_AND_EVENTS.md.
                const WAIT_INPUT: u32 = 1;
                const WAIT_STATE: u32 = 32;
                host::wait(WAIT_INPUT | WAIT_STATE, -1);
            }
            PollResult::WindowGone => return,
        }
    }
}
