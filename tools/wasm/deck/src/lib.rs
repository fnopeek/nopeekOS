//! deck — bottom panel: bar, window list and launcher in one window.
//!
//! Started instead of `dock` + `bar` (`set autostart deck`). Declares a
//! bottom strut panel (`npk_window_set_panel(Bottom, Strut)`), so the tiles
//! end above it. Left to right: the launcher button, the workspaces, the
//! windows of the active workspace, the status readouts and the clock.
//!
//! The launcher opens a menu above the button: a search field over the app
//! catalog, the user, power. The window list gives way when the screen runs
//! out: narrower buttons, then one button per app, then icons, then "+N"
//! (`plan_tasks`); a group and the "+N" open a list of their windows.
//!
//! While a menu or a list is open the panel is raised (`Bottom, Raised`):
//! its window grows upward over the tiles and takes the keyboard. It lowers
//! again on Esc, on a launch, on a click outside the menu, and when the
//! compositor moves the focus elsewhere (`Event::Focus(false)`). See
//! docs/spec/PANEL.md.

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

// Read + exec + render, and the shell roles: the panel, window activation,
// power off.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::EXEC | caps::RENDER, caps::ext::SHELL];

use nopeek_widgets::host;

// The calls only the shell panels make; the shared ones are in
// `nopeek_widgets::host`.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_window_set_panel(edge: i32, behavior: i32, w: i32, h: i32) -> i32;
    fn npk_window_activate(id: i32) -> i32;
    fn npk_bar_state(buf_ptr: i32, max: i32) -> i32;
    fn npk_battery() -> i32;
    fn npk_workspace_switch(n: i32) -> i32;
    fn npk_power() -> i32;
    fn npk_launch(app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32;
}

fn window_set_panel(behavior: i32, w: i32, h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_panel(EDGE_BOTTOM, behavior, w, h) };
}

fn window_activate(id: u32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_activate(id as i32) };
}

/// Live bar state into `buf`; the raw length, `<= 0` when there is none.
fn bar_state(buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_bar_state(buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// `(status << 8) | percent`, or -1 without a battery.
fn battery() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_battery() }
}

fn workspace_switch(n: u32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_workspace_switch(n as i32) };
}

fn power() {
    // SAFETY: FFI without pointers.
    unsafe { npk_power() };
}

fn launch(app: &str, arg: &str) {
    // SAFETY: FFI; the kernel validates both ranges.
    unsafe {
        npk_launch(app.as_ptr() as i32, app.len() as i32, arg.as_ptr() as i32, arg.len() as i32)
    };
}

fn log(msg: &str) { host::log_serial(msg); }

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[deck] panic!");
    core::arch::wasm32::unreachable()
}

const EVENT_BUF_SIZE: usize = 64;

enum PollResult { Event(Event), Empty, WindowGone }

fn poll_event(buf: &mut [u8]) -> PollResult {
    match nopeek_widgets::events::poll(buf) {
        nopeek_widgets::events::Poll::Event(ev) => PollResult::Event(ev),
        nopeek_widgets::events::Poll::Empty => PollResult::Empty,
        nopeek_widgets::events::Poll::Gone => PollResult::WindowGone,
    }
}

// ── Strings ───────────────────────────────────────────────────────────

struct Strings {
    search:     &'static str,
    apps:       &'static str,
    commands:   &'static str,
    no_matches: &'static str,
    open:       &'static str,
    days:       [&'static str; 7],
    months:     [&'static str; 12],
}

const EN: Strings = Strings {
    search:     "Search…",
    apps:       "APPS",
    commands:   "COMMANDS",
    no_matches: "no matches",
    open:       "open",
    days:       ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    months:     ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                 "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
};

const DE: Strings = Strings {
    search:     "Suchen…",
    apps:       "APPS",
    commands:   "BEFEHLE",
    no_matches: "keine Treffer",
    open:       "öffnen",
    days:       ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"],
    months:     ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun",
                 "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
};

fn s() -> &'static Strings {
    match i18n::lang() { Lang::De => &DE, _ => &EN }
}

// ── Layout constants ─────────────────────────────────────────────────

// Panel edge/behavior (docs/spec/PANEL.md).
const EDGE_BOTTOM: i32 = 0;
const BEHAVIOR_STRUT: i32 = 1;
const BEHAVIOR_RAISED: i32 = 2;

/// Panel window height: the 36 px band plus the card's 4 px padding.
const PANEL_H: i32 = 44;
/// Content band inside the card.
const BAND_H: u16 = 36;
/// Window height while the menu is open. The menu is a popover that flips
/// above its anchor, so this only has to hold it plus the panel.
const RAISED_H: i32 = 520;
/// Launcher button and workspace cells.
const BTN: u16 = 32;
const WS_W: u16 = 28;
const CELL_RADIUS: u8 = 8;
const ICON_PX: u16 = 16;
const SEP_H: u16 = 20;
const SEP_W: u16 = 16;
/// Gap between the cells of a row (`Spacing::Xs`).
const GAP: u16 = 4;
/// Distance from the card's top edge to a 32 px button in the band: the
/// card's padding plus half of what the band has over the button.
const BTN_INSET: u16 = 4 + (BAND_H - BTN) / 2;

// The window list's budget. The layout engine does not shrink children and
// the app cannot measure text, so the deck computes with fixed widths:
// whatever is left of the screen after the fixed cells, estimated on the
// generous side so the clock and the readouts never get pushed out.
/// Screen edge to card (both sides): the compositor's bar margin, rounded up.
const SCREEN_MARGIN: u16 = 8;
/// Card padding (`Padding::Xs`) and the bar row's gap (`Spacing::Sm`).
const CARD_PAD: u16 = 4;
const ROW_GAP: u16 = 8;
/// A readout: padding, icon, gap and "100%" in mono.
const READOUT_W: u16 = 76;
/// Time over date, padded.
const CLOCK_W: u16 = 100;
/// Slack for what the estimates miss.
const SAFETY_W: u16 = 16;
/// A window button with its caption: at most this wide, and no narrower
/// than the minimum — below it a caption is unreadable and the stages
/// further down take over.
const TASK_FULL_W: u16 = 200;
const TASK_MIN_W: u16 = 100;
/// A window button as an icon only, a group as icon and count.
const TASK_ICON_W: u16 = 40;
const GROUP_ICON_W: u16 = 56;
/// The "+N" button that holds what does not fit.
const OVERFLOW_W: u16 = 48;
/// Horizontal padding of a window button (`Padding::Md`) and the icon gap.
const TASK_PAD_X: u16 = 12;
/// Body text width per character, generous for Inter at 14 px.
const CHAR_W: u16 = 8;
/// The window list popover.
const WIN_LIST_W: u16 = 300;
/// Menu geometry. The gap is left between the menu and the bar card.
const MENU_W: u16 = 340;
const MENU_GAP: u16 = 8;
const MENU_ROWS: usize = 8;
const ROW_H: u16 = 32;
const QUERY_CAP: usize = 63;

// ── ActionIds + NodeIds ──────────────────────────────────────────────
const WS_BASE: u32 = 1;              // + workspace index
const ROW_BASE: u32 = 50_000;        // + row index (click)
const ROW_HOVER: u32 = 60_000;       // + row index (hover)
const LAUNCHER: u32 = 90_000;
const POWER: u32 = 90_001;
const SHOT: u32 = 90_002;
const VOL_OPEN: u32 = 90_003;
const MENU_DISMISS: u32 = 90_004;
const OVERFLOW: u32 = 90_005;
const GROUP_BASE: u32 = 70_000;      // + slot index
const NODE_BAR: u32 = 100_000;
const NODE_OVERFLOW: u32 = 100_001;
const NODE_SLOT_BASE: u32 = 110_000; // + slot index
/// Last, because a window id has no upper bound of its own.
const TASK_BASE: u32 = 1_000_000;    // + window id

// Wake masks for `host::wait` (docs/plan/CORES_AND_EVENTS.md).
const WAIT_INPUT: u32 = 1;
const WAIT_STATE: u32 = 32;

const STATE_MAX: usize = 256;
const WINS_CAP: usize = 4096;

// ── Window list ──────────────────────────────────────────────────────

/// One line of `npk_window_titles`:
/// `<flags>\t<workspace>\t<title>\t<id>\t<caption>`.
struct Win<'a> {
    focused: bool,
    workspace: u8,
    module: &'a str,
    id: u32,
    caption: &'a str,
}

fn parse_windows(text: &str) -> Vec<Win<'_>> {
    let mut out: Vec<Win<'_>> = text.lines().filter_map(|line| {
        let mut c = line.split('\t');
        let flags: u8 = c.next()?.trim().parse().ok()?;
        let workspace: u8 = c.next()?.trim().parse().ok()?;
        let module = c.next()?;
        let id: u32 = c.next()?.trim().parse().ok()?;
        let caption = c.next().unwrap_or(module);
        Some(Win { focused: flags & 1 != 0, workspace, module, id, caption })
    }).collect();
    // The kernel lists in z-order, which changes with every click; the
    // buttons keep their place by opening order instead.
    out.sort_by_key(|w| w.id);
    out
}

// ── Window list plan ─────────────────────────────────────────────────
//
// What fits, in this order: full buttons; narrower buttons with clipped
// captions; the windows of one app folded into a group button, the app
// with the most windows first; icons only; and last a "+N" button holding
// the rest. The focused window always keeps a place in the bar.

/// A button in the window list; the numbers index the window list.
enum Slot {
    Win(usize),
    Group(Vec<usize>),
}

impl Slot {
    fn windows(&self) -> &[usize] {
        match self {
            Slot::Win(i) => core::slice::from_ref(i),
            Slot::Group(v) => v,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Icon and caption, each button this wide.
    Text(u16),
    Icons,
}

struct Plan {
    slots: Vec<Slot>,
    mode: Mode,
    /// Windows that did not fit, behind the "+N" button.
    overflow: Vec<usize>,
}

fn plan_tasks(wins: &[Win], ws_active: u8, avail: u16) -> Plan {
    let mut slots: Vec<Slot> = wins.iter().enumerate()
        .filter(|(_, w)| w.workspace == ws_active)
        .map(|(i, _)| Slot::Win(i))
        .collect();
    let per_slot = |n: usize| (avail / n.max(1) as u16).saturating_sub(GAP);

    loop {
        if slots.is_empty() || per_slot(slots.len()) >= TASK_MIN_W {
            let w = per_slot(slots.len()).min(TASK_FULL_W);
            return Plan { slots, mode: Mode::Text(w), overflow: Vec::new() };
        }
        // Fold the app with the most ungrouped windows, if one has two.
        let mut best: Option<(&str, usize)> = None;
        for slot in &slots {
            let Slot::Win(i) = slot else { continue };
            let m = wins[*i].module;
            let n = slots.iter()
                .filter(|s| matches!(s, Slot::Win(j) if wins[*j].module == m))
                .count();
            if n >= 2 && best.is_none_or(|(_, b)| n > b) { best = Some((m, n)); }
        }
        let Some((module, _)) = best else { break };
        let members: Vec<usize> = slots.iter()
            .filter_map(|s| match s { Slot::Win(j) if wins[*j].module == module => Some(*j), _ => None })
            .collect();
        let at = slots.iter()
            .position(|s| matches!(s, Slot::Win(j) if wins[*j].module == module))
            .unwrap_or(0);
        slots.retain(|s| !matches!(s, Slot::Win(j) if wins[*j].module == module));
        slots.insert(at.min(slots.len()), Slot::Group(members));
    }

    let icon_w = |s: &Slot| match s { Slot::Win(_) => TASK_ICON_W, Slot::Group(_) => GROUP_ICON_W } + GAP;
    let total: u16 = slots.iter().map(icon_w).sum();
    if total <= avail {
        return Plan { slots, mode: Mode::Icons, overflow: Vec::new() };
    }

    // Overflow: as many as fit beside the "+N" button.
    let room = avail.saturating_sub(OVERFLOW_W + GAP);
    let mut used = 0u16;
    let mut keep = 0usize;
    for slot in &slots {
        if used + icon_w(slot) > room { break; }
        used += icon_w(slot);
        keep += 1;
    }
    let focused = slots.iter().position(|s| s.windows().iter().any(|&i| wins[i].focused));
    if let Some(f) = focused {
        if f >= keep && keep > 0 {
            let slot = slots.remove(f);
            slots.insert(keep - 1, slot);
        }
    }
    let rest = slots.split_off(keep);
    let overflow = rest.iter().flat_map(|s| s.windows().iter().copied()).collect();
    Plan { slots, mode: Mode::Icons, overflow }
}

// ── State ────────────────────────────────────────────────────────────

enum Menu {
    Launcher,
    /// A list of windows above the button `anchor`: a group's, or the
    /// overflow's.
    Windows { anchor: u32, ids: Vec<u32> },
}

struct Deck {
    screen_w: i32,
    /// Bar state of the last read: "HH:MM\n<count>\n<active>\n<title>".
    state: Vec<u8>,
    bat: i32,
    vol: i32,
    pre_mute: u32,
    wins: Vec<u8>,
    catalog: Vec<AppEntry>,
    user: String,
    menu: Option<Menu>,
    /// Raised on every open: the search field's `FocusRequest`. The click
    /// on the launcher has just put the focus on the button, so
    /// `Autofocus` would not take it.
    focus_n: u32,
    query: String,
    filtered: Vec<usize>,
    selected: usize,
    offset: usize,
}

impl Deck {
    fn new() -> Self {
        let (w, _) = host::screen_size();
        let catalog = app_catalog::load(&["deck", "dock", "bar", "drun", "pick", "volume"]);
        let filtered = (0..catalog.len()).collect();
        Deck {
            screen_w: if w > 0 { w as i32 } else { 1920 },
            state: Vec::new(),
            bat: i32::MIN,
            vol: i32::MIN,
            pre_mute: 50,
            wins: Vec::new(),
            catalog,
            user: user_name(),
            menu: None,
            focus_n: 0,
            query: String::with_capacity(QUERY_CAP + 1),
            filtered,
            selected: 0,
            offset: 0,
        }
    }

    /// Re-read everything the panel shows; true when any of it changed.
    fn refresh(&mut self) -> bool {
        let mut buf = [0u8; STATE_MAX];
        let n = bar_state(&mut buf);
        let n = if n > 0 { (n as usize).min(STATE_MAX) } else { 0 };
        let mut wins = [0u8; WINS_CAP];
        let wn = host::window_titles(&mut wins).unwrap_or(0).min(WINS_CAP);
        let bat = battery();
        let vol = host::audio_volume().map_or(-1, |v| v as i32);
        if self.state[..] == buf[..n] && self.wins[..] == wins[..wn]
            && bat == self.bat && vol == self.vol
        {
            return false;
        }
        self.state.clear();
        self.state.extend_from_slice(&buf[..n]);
        self.wins.clear();
        self.wins.extend_from_slice(&wins[..wn]);
        self.bat = bat;
        self.vol = vol;
        true
    }

    fn commit(&self) {
        match wire::encode(&self.render()) {
            Ok(bytes) => { if !host::scene_commit(&bytes) { log("[deck] commit failed"); } }
            Err(_) => log("[deck] encode failed"),
        }
    }

    fn open_menu(&mut self) {
        self.menu = Some(Menu::Launcher);
        self.focus_n = self.focus_n.wrapping_add(1).max(1);
        self.query.clear();
        self.refilter();
        window_set_panel(BEHAVIOR_RAISED, self.screen_w, RAISED_H);
    }

    fn close_menu(&mut self) {
        if self.menu.take().is_none() { return; }
        window_set_panel(BEHAVIOR_STRUT, self.screen_w, PANEL_H);
    }

    /// Open the window list above `anchor`; a second click on the same
    /// button closes it.
    fn toggle_windows(&mut self, anchor: u32, ids: Vec<u32>) {
        if matches!(&self.menu, Some(Menu::Windows { anchor: a, .. }) if *a == anchor) {
            self.close_menu();
            return;
        }
        let was_open = self.menu.is_some();
        self.menu = Some(Menu::Windows { anchor, ids });
        if !was_open { window_set_panel(BEHAVIOR_RAISED, self.screen_w, RAISED_H); }
    }

    /// Close a window list whose windows are all gone.
    fn prune_menu(&mut self) {
        let Some(Menu::Windows { ids, .. }) = &self.menu else { return };
        let text = core::str::from_utf8(&self.wins).unwrap_or("");
        let wins = parse_windows(text);
        if !ids.iter().any(|id| wins.iter().any(|w| w.id == *id)) {
            self.close_menu();
        }
    }

    fn launcher_open(&self) -> bool { matches!(self.menu, Some(Menu::Launcher)) }

    /// Width the window list may take: the screen minus every fixed cell.
    fn task_budget(&self, ws_count: u8) -> u16 {
        let ws = ws_count as u16;
        let left = BTN + SEP_W + ws * WS_W + SEP_W + (3 + ws) * GAP;
        let mut right = BTN + SEP_W + CLOCK_W + 3 * GAP;
        if self.vol >= 0 { right += READOUT_W + GAP; }
        if self.bat >= 0 { right += READOUT_W + GAP; }
        (self.screen_w.clamp(0, u16::MAX as i32) as u16)
            .saturating_sub(2 * SCREEN_MARGIN + 2 * CARD_PAD + 2 * ROW_GAP)
            .saturating_sub(left + right + SAFETY_W)
    }

    /// The window list and its plan, as the bar shows it now.
    fn tasks(&self) -> (Vec<Win<'_>>, Plan) {
        let state = core::str::from_utf8(&self.state).unwrap_or("");
        let mut it = state.splitn(4, '\n');
        it.next();
        let ws_count: u8 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let ws_active: u8 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let wins = parse_windows(core::str::from_utf8(&self.wins).unwrap_or(""));
        let plan = plan_tasks(&wins, ws_active, self.task_budget(ws_count));
        (wins, plan)
    }

    fn refilter(&mut self) {
        let q = self.query.to_ascii_lowercase();
        self.filtered.clear();
        // Apps first, then commands — the order the menu draws them in, so
        // the arrow keys walk down the screen.
        for kind in [EntryKind::Module, EntryKind::Intent] {
            for (i, e) in self.catalog.iter().enumerate() {
                if e.kind != kind { continue; }
                if q.is_empty()
                    || e.display_name.to_ascii_lowercase().contains(&q)
                    || e.launch_name.to_ascii_lowercase().contains(&q)
                {
                    self.filtered.push(i);
                }
            }
        }
        self.selected = 0;
        self.offset = 0;
    }

    fn ensure_visible(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + MENU_ROWS {
            self.offset = self.selected + 1 - MENU_ROWS;
        }
    }

    fn launch_selected(&self) {
        let Some(e) = self.filtered.get(self.selected).and_then(|&i| self.catalog.get(i))
            else { return };
        let ok = match e.kind {
            EntryKind::Module => host::spawn_module(&e.launch_name),
            EntryKind::Intent => host::run_intent(&e.launch_name) == 0,
        };
        if !ok { log("[deck] launch failed"); }
    }

    /// True when the tree must be committed again.
    fn handle(&mut self, ev: Event) -> bool {
        match ev {
            Event::Action(ActionId(id)) => self.on_action(id),
            Event::ContextAction(ActionId(id)) => {
                if id == SHOT {
                    launch("snap", "full");
                } else if id == VOL_OPEN {
                    let v = host::audio_volume().unwrap_or(0);
                    if v > 0 {
                        self.pre_mute = v;
                        host::audio_set_volume(0);
                    } else {
                        host::audio_set_volume(self.pre_mute.max(10));
                    }
                }
                false
            }
            Event::Focus(false) => { self.close_menu(); true }
            Event::Key(KeyCode::Escape) if self.menu.is_some() => { self.close_menu(); true }
            Event::Key(k) if self.launcher_open() => match k {
                KeyCode::Enter => { self.launch_selected(); self.close_menu(); true }
                KeyCode::Up => {
                    if self.selected > 0 { self.selected -= 1; }
                    self.ensure_visible();
                    true
                }
                KeyCode::Down => {
                    if self.selected + 1 < self.filtered.len() { self.selected += 1; }
                    self.ensure_visible();
                    true
                }
                _ => false,
            },
            Event::InputChange { value } if self.launcher_open() => {
                self.query.clear();
                self.query.push_str(nopeek_widgets::rt::str_clamp(&value, QUERY_CAP));
                self.refilter();
                true
            }
            _ => false,
        }
    }

    fn on_action(&mut self, id: u32) -> bool {
        match id {
            LAUNCHER => {
                // From a window list straight to the launcher: the panel is
                // raised already and stays so.
                if self.launcher_open() { self.close_menu(); } else { self.open_menu(); }
                true
            }
            OVERFLOW => {
                let (wins, plan) = self.tasks();
                let ids = plan.overflow.iter().map(|&i| wins[i].id).collect();
                self.toggle_windows(NODE_OVERFLOW, ids);
                true
            }
            _ if (GROUP_BASE..LAUNCHER).contains(&id) => {
                let slot = (id - GROUP_BASE) as usize;
                let (wins, plan) = self.tasks();
                let Some(Slot::Group(members)) = plan.slots.get(slot) else { return false };
                let ids = members.iter().map(|&i| wins[i].id).collect();
                self.toggle_windows(NODE_SLOT_BASE + slot as u32, ids);
                true
            }
            MENU_DISMISS => { self.close_menu(); true }
            POWER => { power(); false }
            SHOT => { launch("snap", "region"); false }
            VOL_OPEN => { launch("volume", ""); false }
            _ if (ROW_HOVER..ROW_HOVER + MENU_ROWS as u32).contains(&id) => {
                let i = self.offset + (id - ROW_HOVER) as usize;
                if i < self.filtered.len() && i != self.selected {
                    self.selected = i;
                    return true;
                }
                false
            }
            _ if (ROW_BASE..ROW_BASE + MENU_ROWS as u32).contains(&id) => {
                let i = self.offset + (id - ROW_BASE) as usize;
                if i < self.filtered.len() {
                    self.selected = i;
                    self.launch_selected();
                    self.close_menu();
                    return true;
                }
                false
            }
            _ if id >= TASK_BASE => {
                self.close_menu();
                window_activate(id - TASK_BASE);
                true
            }
            _ if id >= WS_BASE && id < ROW_BASE => {
                self.close_menu();
                workspace_switch(id - WS_BASE);
                true
            }
            _ => false,
        }
    }

    // ── Render ───────────────────────────────────────────────────────

    fn render(&self) -> Widget {
        let state = core::str::from_utf8(&self.state).unwrap_or("");
        let mut it = state.splitn(4, '\n');
        let clock = it.next().unwrap_or("");
        let ws_count: u8 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let ws_active: u8 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let (wins, plan) = self.tasks();

        let mut left: Vec<Widget> = Vec::new();
        left.push(launcher_button(self.launcher_open()));
        left.push(separator());
        for i in 0..ws_count {
            let occupied = wins.iter().any(|w| w.workspace == i);
            left.push(workspace_cell(i, i == ws_active, occupied));
        }
        if !plan.slots.is_empty() || !plan.overflow.is_empty() {
            left.push(separator());
            for (n, slot) in plan.slots.iter().enumerate() {
                left.push(self.slot_button(&wins, slot, n as u32, plan.mode));
            }
            if !plan.overflow.is_empty() {
                let focused = plan.overflow.iter().any(|&i| wins[i].focused);
                left.push(overflow_button(plan.overflow.len(), focused));
            }
        }

        let mut right: Vec<Widget> = Vec::new();
        if self.vol >= 0 { right.push(volume_readout(self.vol as u8)); }
        if self.bat >= 0 { right.push(battery_readout(self.bat)); }
        right.push(tray_cell(IconId::Camera, SHOT, Token::OnSurfaceMuted));
        right.push(separator());
        right.push(clock_block(clock));

        let bar = Widget::Row {
            children: alloc::vec![
                band_row(left),
                Widget::Spacer { flex: 1 },
                band_row(right),
            ],
            spacing: Spacing::Sm.as_u16(),
            align: Align::Center,
            modifiers: alloc::vec![
                Modifier::Background(Token::SurfaceElevated),
                Modifier::Border { token: Token::Border, width: 1, radius: Radius::Lg.as_u8() },
                Modifier::Rounded(Radius::Lg.as_u8()),
                Modifier::PaddingXY { x: Padding::Xs.as_u16(), y: Padding::Xs.as_u16() },
                Modifier::MinHeight(PANEL_H as u16),
                Modifier::MaxHeight(PANEL_H as u16),
                Modifier::NodeId(NodeId(NODE_BAR)),
            ],
        };

        // The bar sits at the bottom of the window whatever its height: the
        // compositor lays the last tree out again at the raised size before
        // the next commit arrives, and the bar must not move meanwhile.
        let column = Widget::Column {
            children: alloc::vec![Widget::Spacer { flex: 1 }, bar],
            spacing: 0,
            align: Align::Stretch,
            modifiers: Vec::new(),
        };
        // A popover flips to end at its anchor's top. The launcher menu is
        // anchored at the card, flush with its left edge; a window list at
        // its button, which sits `BTN_INSET` inside the card — the gap
        // below each covers that, so neither overlaps the card.
        let (anchor, content, gap) = match &self.menu {
            None => return Widget::Stack { children: alloc::vec![column], modifiers: Vec::new() },
            Some(Menu::Launcher) => (NODE_BAR, self.render_menu(), MENU_GAP),
            Some(Menu::Windows { anchor, ids }) => {
                (*anchor, self.render_windows(&wins, ids), MENU_GAP + BTN_INSET)
            }
        };
        let menu = Widget::Column {
            children: alloc::vec![content, prefab::mark(1, gap, None)],
            spacing: 0,
            align: Align::Start,
            modifiers: Vec::new(),
        };
        let popover = Widget::Popover {
            anchor: NodeId(anchor),
            child: alloc::boxed::Box::new(menu),
            on_dismiss: ActionId(MENU_DISMISS),
            modifiers: Vec::new(),
        };
        Widget::Stack { children: alloc::vec![column, popover], modifiers: Vec::new() }
    }

    /// One button of the window list.
    fn slot_button(&self, wins: &[Win], slot: &Slot, n: u32, mode: Mode) -> Widget {
        match slot {
            Slot::Win(i) => {
                let w = &wins[*i];
                let icon = icon_for(&self.catalog, w.module);
                let label = match mode {
                    Mode::Text(width) => Some((clip(w.caption, caption_chars(width, 0)), width)),
                    Mode::Icons => None,
                };
                task_button(icon, label, None, w.focused,
                    ActionId(TASK_BASE + w.id), NodeId(NODE_SLOT_BASE + n))
            }
            Slot::Group(members) => {
                let first = &wins[members[0]];
                let entry = self.catalog.iter().find(|e| e.launch_name == first.module);
                let icon = entry.map(|e| e.icon).unwrap_or(IconId::Monitor);
                let name = entry.map(|e| e.display_name.as_str()).unwrap_or(first.module);
                let count = alloc::format!("{}", members.len());
                let label = match mode {
                    Mode::Text(width) =>
                        Some((clip(name, caption_chars(width, count.len() + 1)), width)),
                    Mode::Icons => None,
                };
                let focused = members.iter().any(|&i| wins[i].focused);
                task_button(icon, label, Some(count), focused,
                    ActionId(GROUP_BASE + n), NodeId(NODE_SLOT_BASE + n))
            }
        }
    }

    /// The windows `ids` as a list; a click brings one to the front.
    fn render_windows(&self, wins: &[Win], ids: &[u32]) -> Widget {
        let rows: Vec<Widget> = ids.iter()
            .filter_map(|id| wins.iter().find(|w| w.id == *id))
            .map(|w| {
                let mut mods = alloc::vec![
                    Modifier::MinHeight(ROW_H),
                    Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: 0 },
                    Modifier::Rounded(CELL_RADIUS),
                    Modifier::OnClick(ActionId(TASK_BASE + w.id)),
                ];
                if w.focused {
                    mods.push(Modifier::Background(Token::SurfaceHover));
                } else {
                    mods.push(hover(CELL_RADIUS));
                }
                Widget::Row {
                    children: alloc::vec![
                        Widget::Icon { id: icon_for(&self.catalog, w.module), size: ICON_PX,
                            modifiers: Vec::new() },
                        Widget::Text {
                            content: clip(w.caption, caption_chars(WIN_LIST_W, 0)),
                            style: TextStyle::Body,
                            modifiers: Vec::new(),
                        },
                    ],
                    spacing: Spacing::Sm.as_u16(),
                    align: Align::Center,
                    modifiers: mods,
                }
            })
            .collect();
        Widget::Column {
            children: rows,
            spacing: 0,
            align: Align::Stretch,
            modifiers: alloc::vec![
                Modifier::Background(Token::SurfaceElevated),
                Modifier::Border { token: Token::Border, width: 1, radius: Radius::Lg.as_u8() },
                Modifier::Rounded(Radius::Lg.as_u8()),
                Modifier::Padding(Padding::Xs.as_u16()),
                Modifier::MinWidth(WIN_LIST_W),
                Modifier::MaxWidth(WIN_LIST_W),
            ],
        }
    }

    fn render_menu(&self) -> Widget {
        let search = search_field(&self.query, self.focus_n);

        let mut list: Vec<Widget> = Vec::new();
        if self.filtered.is_empty() {
            list.push(prefab::empty_state(s().no_matches));
        }
        let end = (self.offset + MENU_ROWS).min(self.filtered.len());
        let mut last_kind: Option<EntryKind> = None;
        for i in self.offset..end {
            let e = &self.catalog[self.filtered[i]];
            if last_kind != Some(e.kind) {
                let label = if e.kind == EntryKind::Module { s().apps } else { s().commands };
                list.push(section_label(label));
                last_kind = Some(e.kind);
            }
            list.push(menu_row(e, i == self.selected, (i - self.offset) as u32));
        }

        let user = Widget::Row {
            children: alloc::vec![
                Widget::Icon { id: IconId::Home, size: ICON_PX,
                    modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                Widget::Text { content: self.user.clone(), style: TextStyle::Body,
                    modifiers: Vec::new() },
                Widget::Spacer { flex: 1 },
                // No settings app yet: shown, not wired.
                prefab::center_box(
                    Widget::Icon { id: IconId::Gear, size: ICON_PX, modifiers: Vec::new() },
                    alloc::vec![
                        Modifier::MinWidth(BTN), Modifier::MinHeight(BTN),
                        Modifier::Tint(Token::OnSurfaceFaint),
                    ]),
                tray_cell(IconId::Power, POWER, Token::Danger),
            ],
            spacing: Spacing::Sm.as_u16(),
            align: Align::Center,
            modifiers: alloc::vec![Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: 0 }],
        };

        Widget::Column {
            children: alloc::vec![
                search,
                Widget::Column {
                    children: list,
                    spacing: 0,
                    align: Align::Stretch,
                    modifiers: Vec::new(),
                },
                Widget::Divider,
                user,
            ],
            spacing: Spacing::Sm.as_u16(),
            align: Align::Stretch,
            modifiers: alloc::vec![
                Modifier::Background(Token::SurfaceElevated),
                Modifier::Border { token: Token::Border, width: 1, radius: Radius::Lg.as_u8() },
                Modifier::Rounded(Radius::Lg.as_u8()),
                Modifier::Padding(Padding::Sm.as_u16()),
                Modifier::MinWidth(MENU_W),
                Modifier::MaxWidth(MENU_W),
            ],
        }
    }
}

// ── Pieces ───────────────────────────────────────────────────────────

/// A row of the panel, pinned to the band so an over-tall piece cannot
/// push the rest out of the card.
fn band_row(children: Vec<Widget>) -> Widget {
    Widget::Row {
        children,
        spacing: Spacing::Xs.as_u16(),
        align: Align::Center,
        modifiers: alloc::vec![Modifier::MinHeight(BAND_H), Modifier::MaxHeight(BAND_H)],
    }
}

fn hover(radius: u8) -> Modifier {
    Modifier::Hover(alloc::vec![
        Modifier::Background(Token::SurfaceHover),
        Modifier::Rounded(radius),
    ])
}

fn launcher_button(open: bool) -> Widget {
    let mut mods = alloc::vec![
        Modifier::MinWidth(BTN),
        Modifier::MinHeight(BTN),
        Modifier::Rounded(CELL_RADIUS),
        Modifier::OnClick(ActionId(LAUNCHER)),
    ];
    if open {
        mods.push(Modifier::Background(Token::Accent));
        mods.push(Modifier::Tint(Token::OnAccent));
    } else {
        mods.push(Modifier::Background(Token::SurfaceHover));
        mods.push(Modifier::Tint(Token::Accent));
        mods.push(Modifier::Hover(alloc::vec![
            Modifier::Background(Token::AccentMuted),
            Modifier::Rounded(CELL_RADIUS),
        ]));
    }
    prefab::center_box(Widget::Icon { id: IconId::List, size: ICON_PX, modifiers: Vec::new() }, mods)
}

fn separator() -> Widget {
    Widget::Row {
        children: alloc::vec![
            Widget::Spacer { flex: 1 },
            prefab::mark(1, SEP_H, Some(Token::Border)),
            Widget::Spacer { flex: 1 },
        ],
        spacing: 0,
        align: Align::Center,
        modifiers: alloc::vec![Modifier::MinWidth(SEP_W), Modifier::MaxWidth(SEP_W)],
    }
}

/// Active = filled accent; occupied = full-strength digit; empty = faint.
fn workspace_cell(i: u8, active: bool, occupied: bool) -> Widget {
    let mut mods = alloc::vec![
        Modifier::MinWidth(WS_W),
        Modifier::MinHeight(WS_W),
        Modifier::Rounded(CELL_RADIUS),
        Modifier::OnClick(ActionId(WS_BASE + i as u32)),
    ];
    if active {
        mods.push(Modifier::Background(Token::Accent));
        mods.push(Modifier::Tint(Token::OnAccent));
    } else {
        if !occupied { mods.push(Modifier::Tint(Token::OnSurfaceFaint)); }
        mods.push(hover(CELL_RADIUS));
    }
    prefab::center_box(
        Widget::Text { content: alloc::format!("{}", i + 1), style: TextStyle::Mono,
            modifiers: Vec::new() },
        mods)
}

/// A button of the window list: the app icon, a caption at `width` when
/// there is room for one, and a count for a group. The focused window (or
/// a group holding it) carries an accent line underneath, the way the dock
/// marks it.
fn task_button(icon: IconId, label: Option<(String, u16)>, count: Option<String>,
               focused: bool, action: ActionId, node: NodeId) -> Widget {
    let mut row = alloc::vec![Widget::Icon { id: icon, size: ICON_PX, modifiers: Vec::new() }];
    let width = match label {
        Some((text, width)) => {
            row.push(Widget::Text { content: text, style: TextStyle::Body, modifiers: Vec::new() });
            width
        }
        None => if count.is_some() { GROUP_ICON_W } else { TASK_ICON_W },
    };
    if let Some(n) = count {
        row.push(Widget::Spacer { flex: 1 });
        row.push(Widget::Text { content: n, style: TextStyle::Mono,
            modifiers: alloc::vec![Modifier::Tint(Token::Accent)] });
    }
    let label = Widget::Row {
        children: row,
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: Vec::new(),
    };
    let line = prefab::mark(24, 2, if focused { Some(Token::Accent) } else { None });
    let mut mods = alloc::vec![
        Modifier::MinHeight(BTN),
        Modifier::MinWidth(width),
        Modifier::MaxWidth(width),
        Modifier::PaddingXY { x: TASK_PAD_X, y: 0 },
        Modifier::Rounded(CELL_RADIUS),
        Modifier::OnClick(action),
        Modifier::NodeId(node),
    ];
    if focused {
        mods.push(Modifier::Background(Token::SurfaceHover));
    } else {
        mods.push(Modifier::Tint(Token::OnSurfaceMuted));
        mods.push(hover(CELL_RADIUS));
    }
    Widget::Column {
        children: alloc::vec![Widget::Spacer { flex: 1 }, label, Widget::Spacer { flex: 1 }, line],
        spacing: 0,
        align: Align::Center,
        modifiers: mods,
    }
}

/// "+N": the windows that did not fit.
fn overflow_button(n: usize, focused: bool) -> Widget {
    let mut mods = alloc::vec![
        Modifier::MinWidth(OVERFLOW_W),
        Modifier::MinHeight(BTN),
        Modifier::Rounded(CELL_RADIUS),
        Modifier::OnClick(ActionId(OVERFLOW)),
        Modifier::NodeId(NodeId(NODE_OVERFLOW)),
        hover(CELL_RADIUS),
    ];
    mods.push(Modifier::Tint(if focused { Token::Accent } else { Token::OnSurfaceMuted }));
    prefab::center_box(
        Widget::Text { content: alloc::format!("+{}", n), style: TextStyle::Mono,
            modifiers: Vec::new() },
        mods)
}

/// Characters of caption a button of `width` holds beside its icon, with
/// `reserve` characters kept for a count.
fn caption_chars(width: u16, reserve: usize) -> usize {
    let text_w = width.saturating_sub(2 * TASK_PAD_X + ICON_PX + 8);
    ((text_w / CHAR_W) as usize).saturating_sub(reserve).max(1)
}

fn tray_cell(icon: IconId, action: u32, tint: Token) -> Widget {
    prefab::center_box(
        Widget::Icon { id: icon, size: ICON_PX, modifiers: Vec::new() },
        alloc::vec![
            Modifier::MinWidth(BTN),
            Modifier::MinHeight(BTN),
            Modifier::Rounded(CELL_RADIUS),
            Modifier::Tint(tint),
            Modifier::OnClick(ActionId(action)),
            hover(CELL_RADIUS),
        ])
}

fn readout(icon: IconId, icon_tint: Token, value: String, click: Option<u32>) -> Widget {
    let mut mods = alloc::vec![
        Modifier::MinHeight(BTN),
        Modifier::Rounded(CELL_RADIUS),
        Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: 0 },
        Modifier::Tint(Token::OnSurfaceMuted),
    ];
    if let Some(a) = click {
        mods.push(Modifier::OnClick(ActionId(a)));
        mods.push(hover(CELL_RADIUS));
    }
    Widget::Row {
        children: alloc::vec![
            Widget::Icon { id: icon, size: ICON_PX, modifiers: alloc::vec![Modifier::Tint(icon_tint)] },
            Widget::Text { content: value, style: TextStyle::Mono, modifiers: Vec::new() },
        ],
        spacing: Spacing::Xs.as_u16(),
        align: Align::Center,
        modifiers: mods,
    }
}

fn volume_readout(v: u8) -> Widget {
    let icon = if v == 0 { IconId::SpeakerX }
               else if v <= 50 { IconId::SpeakerLow }
               else { IconId::SpeakerHigh };
    readout(icon, Token::OnSurfaceMuted, alloc::format!("{}%", v), Some(VOL_OPEN))
}

fn battery_readout(bat: i32) -> Widget {
    let percent = (bat & 0xFF) as u8;
    let status = (bat >> 8) & 0xFF; // 0=discharge 1=charge 2=full 3=plugged-idle
    let icon = match status {
        1 => IconId::BatteryCharging,
        2 => IconId::BatteryFull,
        3 => IconId::Plug,
        _ => match percent {
            0..=10  => IconId::BatteryWarning,
            11..=30 => IconId::BatteryLow,
            31..=55 => IconId::BatteryMedium,
            56..=85 => IconId::BatteryHigh,
            _       => IconId::BatteryFull,
        },
    };
    let tint = if status == 0 && percent <= 10 { Token::Danger } else { Token::OnSurfaceMuted };
    readout(icon, tint, alloc::format!("{}%", percent), None)
}

/// Time over date, right-aligned.
fn clock_block(clock: &str) -> Widget {
    Widget::Column {
        children: alloc::vec![
            Widget::Text { content: clock.to_string(), style: TextStyle::Mono,
                modifiers: Vec::new() },
            Widget::Text { content: date_line(clock), style: TextStyle::Caption,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)] },
        ],
        spacing: 0,
        align: Align::End,
        modifiers: alloc::vec![Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: 0 }],
    }
}

fn search_field(value: &str, focus_n: u32) -> Widget {
    Widget::Row {
        children: alloc::vec![
            Widget::Icon { id: IconId::MagnifyingGlass, size: ICON_PX,
                modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)] },
            Widget::Input {
                value: value.to_string(),
                placeholder: s().search.to_string(),
                on_submit: prefab::NO_ACTION,
                modifiers: alloc::vec![Modifier::FocusRequest(focus_n)],
            },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: alloc::vec![
            Modifier::PaddingXY { x: Padding::Md.as_u16(), y: Padding::Sm.as_u16() },
            Modifier::Border { token: Token::AccentRing, width: 1, radius: Radius::Md.as_u8() },
            Modifier::Rounded(Radius::Md.as_u8()),
        ],
    }
}

fn section_label(label: &str) -> Widget {
    Widget::Text {
        content: label.to_string(),
        style: TextStyle::Caption,
        modifiers: alloc::vec![
            Modifier::Tint(Token::OnSurfaceFaint),
            Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Xs.as_u16() },
        ],
    }
}

fn menu_row(e: &AppEntry, selected: bool, slot: u32) -> Widget {
    let mut children = alloc::vec![
        Widget::Icon { id: e.icon, size: ICON_PX, modifiers: Vec::new() },
        Widget::Text { content: e.display_name.clone(), style: TextStyle::Body,
            modifiers: Vec::new() },
        Widget::Spacer { flex: 1 },
    ];
    let mut mods = alloc::vec![
        Modifier::MinHeight(ROW_H),
        Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: 0 },
        Modifier::Rounded(CELL_RADIUS),
        Modifier::OnClick(ActionId(ROW_BASE + slot)),
        Modifier::OnHover(ActionId(ROW_HOVER + slot)),
    ];
    if selected {
        children.push(Widget::Text {
            content: alloc::format!("↵ {}", s().open),
            style: TextStyle::Caption,
            modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceFaint)],
        });
        mods.push(Modifier::Background(Token::SurfaceHover));
    }
    Widget::Row {
        children,
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: mods,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn icon_for(catalog: &[AppEntry], module: &str) -> IconId {
    catalog.iter().find(|e| e.launch_name == module).map(|e| e.icon).unwrap_or(IconId::Monitor)
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { return text.to_string(); }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The last component of the home directory: the user's name.
fn user_name() -> String {
    let mut buf = [0u8; 128];
    let n = host::home_dir(&mut buf).unwrap_or(0).min(buf.len());
    let home = core::str::from_utf8(&buf[..n]).unwrap_or("");
    home.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

/// "Sun 18 Oct" for today. The kernel gives the local time only as HH:MM;
/// the UTC offset falls out of it against the unix clock, rounded to a
/// quarter hour so a minute turning between the two reads cannot skew it.
fn date_line(clock: &str) -> String {
    let Some((h, m)) = clock.split_once(':') else { return String::new() };
    let (Ok(h), Ok(m)) = (h.parse::<i64>(), m.parse::<i64>()) else { return String::new() };
    let unix = host::unix_time() as i64;
    if unix == 0 { return String::new(); }
    let utc_min = (unix / 60).rem_euclid(1440);
    let mut off = (h * 60 + m - utc_min).rem_euclid(1440);
    if off > 720 { off -= 1440; }
    let off = (off + if off >= 0 { 7 } else { -7 }) / 15 * 15;
    let days = (unix + off * 60).div_euclid(86_400);
    let weekday = (days + 4).rem_euclid(7) as usize; // 1970-01-01 was a Thursday
    let (_, month, day) = civil_from_days(days);
    alloc::format!("{} {} {}", s().days[weekday], day, s().months[(month - 1) as usize])
}

/// Days since 1970-01-01 → (year, month 1..=12, day 1..=31). Howard
/// Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut deck = Deck::new();
    window_set_panel(BEHAVIOR_STRUT, deck.screen_w, PANEL_H);
    deck.refresh();
    deck.commit();

    let mut event_buf = [0u8; EVENT_BUF_SIZE];
    loop {
        let mut dirty = false;
        loop {
            match poll_event(&mut event_buf) {
                PollResult::Event(ev) => dirty |= deck.handle(ev),
                PollResult::Empty => break,
                PollResult::WindowGone => return,
            }
        }
        if deck.refresh() {
            deck.prune_menu();
            dirty = true;
        }
        if dirty { deck.commit(); }
        // Woken by a click, a key, a watched topic (windows, battery,
        // volume), or the next full minute for the clock.
        let now = host::unix_time();
        let to_minute_ms = (60 - now % 60) * 1000 + 50;
        host::wait(WAIT_INPUT | WAIT_STATE, to_minute_ms as i32);
    }
}
