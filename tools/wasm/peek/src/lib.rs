//! peek — the security cockpit: what every app did with its token.
//!
//! Layout as helm: menu bar · sidebar (views, apps) + scrolled content ·
//! status line. The data is the kernel's audit journal, read with
//! `npk_audit_read` (format there); the kernel hands it to this module
//! only, by name and AUDIT. peek shows and filters, it never changes the
//! journal.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::host;
use nopeek_widgets::prefab::{self, ButtonStyle, CardKind};
use nopeek_widgets::style::{Padding, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// RENDER: the window. AUDIT: the journal.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::RENDER, caps::ext::AUDIT];

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_audit_read(since_ms: i64, buf_ptr: i32, max: i32) -> i32;
}

fn log(msg: &str) { host::log_serial(msg); }

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[peek] panic!"); core::arch::wasm32::unreachable() }

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Actions ───────────────────────────────────────────────────────────

const ACT_MENU_PEEK: u32 = 1;
const ACT_MENU_VIEW: u32 = 2;
const ACT_MENU_DISMISS: u32 = 3;
const ACT_CLOSE: u32 = 4;
const ACT_ABOUT: u32 = 5;
const ACT_PAUSE: u32 = 6;
const ACT_ALL_APPS: u32 = 7;
const ACT_PAGE_BASE: u32 = 10;
const ACT_APP_BASE: u32 = 1000;

const NODE_MENU_PEEK: u32 = 100;
const NODE_MENU_VIEW: u32 = 101;

const SIDEBAR_W: u16 = 176;
const TILE_MIN_W: u16 = 130;
const TIME_W: u16 = 64;
const WHO_W: u16 = 120;
const KIND_W: u16 = 64;
/// Rows the timeline shows, newest first; the journal keeps more.
const TIMELINE_ROWS: usize = 250;
/// Entries kept here; the kernel's ring is the same size.
const MAX_ENTRIES: usize = 4096;

#[derive(Clone, Copy, PartialEq)]
enum Page { Timeline, Apps, Refused }

const PAGES: [Page; 3] = [Page::Timeline, Page::Apps, Page::Refused];

impl Page {
    fn label(self) -> &'static str {
        match self {
            Page::Timeline => "Timeline",
            Page::Apps => "Apps",
            Page::Refused => "Refused",
        }
    }
    fn icon(self) -> IconId {
        match self {
            Page::Timeline => IconId::List,
            Page::Apps => IconId::SquaresFour,
            Page::Refused => IconId::Lock,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Menu { Peek, View }

// ── Journal ───────────────────────────────────────────────────────────

/// One journal entry as `npk_audit_read` reports it.
#[derive(Clone, PartialEq)]
struct Entry {
    local: u64,
    up_ms: u64,
    last_up_ms: u64,
    inst: u32,
    holder: String,
    kind: String,
    rights: String,
    count: u32,
    checks: u64,
    denied: u32,
    object: String,
}

impl Entry {
    fn is_denied(&self) -> bool { self.kind == "DENIED" }
}

#[derive(Default)]
struct Journal {
    entries: BTreeMap<u64, Entry>,
    /// Live holders: instance -> (name, checks passed so far).
    live: BTreeMap<u32, (String, u64)>,
    checks_passed: u64,
    written: u64,
    now_ms: u64,
    now_local: u64,
    /// Uptime of the last read; the next asks from there.
    read_ms: u64,
}

impl Journal {
    /// Fetch what changed since the last read. False when the kernel
    /// refused or nothing could be parsed.
    fn refresh(&mut self) -> bool {
        // A folded entry only moves `last_up_ms`; a second of overlap
        // catches one folded while the last answer was on its way.
        let since = self.read_ms.saturating_sub(1000) as i64;
        // SAFETY: FFI; the buffer is borrowed for the call.
        let Some(b) = host::read_all(|buf| unsafe { npk_audit_read(since, buf.as_mut_ptr() as i32, buf.len() as i32) })
        else { return false };
        let Ok(text) = core::str::from_utf8(&b) else { return false };
        self.live.clear();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let num = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let st = |i: usize| f.get(i).copied().unwrap_or("").to_string();
            match f.first().copied() {
                Some("S") => {
                    self.checks_passed = num(1);
                    self.written = num(2);
                    self.now_ms = num(3);
                    self.now_local = num(4);
                }
                Some("H") => { self.live.insert(num(1) as u32, (st(2), num(3))); }
                Some("E") if f.len() >= 13 => {
                    self.entries.insert(num(1), Entry {
                        local: num(2), up_ms: num(3), last_up_ms: num(4), inst: num(5) as u32,
                        holder: st(6), kind: st(7), rights: st(8),
                        count: num(9) as u32, checks: num(10), denied: num(11) as u32,
                        object: f[12..].join("\t"),
                    });
                }
                _ => {}
            }
        }
        while self.entries.len() > MAX_ENTRIES {
            let Some((&k, _)) = self.entries.iter().next() else { break };
            self.entries.remove(&k);
        }
        self.read_ms = self.now_ms;
        true
    }

    /// Local wall seconds of an entry; from uptime when the clock was unset.
    fn local_of(&self, e: &Entry) -> u64 {
        if e.local != 0 { return e.local; }
        if self.now_local == 0 { return 0; }
        self.now_local.saturating_sub(self.now_ms.saturating_sub(e.up_ms) / 1000)
    }

    fn refused(&self) -> u64 {
        self.entries.values().filter(|e| e.is_denied()).map(|e| e.count as u64).sum()
    }

    fn distinct(&self, pick: impl Fn(&Entry) -> bool) -> usize {
        let mut seen: Vec<&str> = self.entries.values().filter(|e| pick(e)).map(|e| e.object.as_str()).collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    }

    /// App names in order of first appearance, the kernel's own last.
    fn app_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for e in self.entries.values() {
            if !names.contains(&e.holder) { names.push(e.holder.clone()); }
        }
        for (name, _) in self.live.values() {
            if !names.contains(name) { names.push(name.clone()); }
        }
        names.sort_by_key(|n| is_system(n));
        names
    }
}

fn is_system(name: &str) -> bool { matches!(name, "system" | "kernel" | "loop") }

fn is_file(kind: &str) -> bool { matches!(kind, "read" | "write" | "create" | "delete" | "rename" | "mkdir") }

/// Per app: everything the journal says about all its instances.
#[derive(Default)]
struct AppSummary {
    launches: u32,
    running: u32,
    checks: u64,
    refused: u64,
    files: Vec<String>,
    hosts: Vec<String>,
    devices: Vec<String>,
    rights: String,
}

fn summarize(j: &Journal, name: &str) -> AppSummary {
    let mut a = AppSummary::default();
    for e in j.entries.values().filter(|e| e.holder == name) {
        let push = |v: &mut Vec<String>, s: &str| if !v.iter().any(|x| x == s) { v.push(s.to_string()) };
        match e.kind.as_str() {
            "launch" => { a.launches += 1; a.rights = e.rights.clone(); }
            "exit" => a.checks += e.checks,
            "DENIED" => a.refused += e.count as u64,
            "net" => push(&mut a.hosts, &e.object),
            "on" => push(&mut a.devices, &e.object),
            k if is_file(k) => push(&mut a.files, &e.object),
            _ => {}
        }
    }
    for (n, checks) in j.live.values() {
        if n == name { a.running += 1; a.checks += checks; }
    }
    a
}

// ── State ─────────────────────────────────────────────────────────────

struct Peek {
    page: Page,
    menu: Option<Menu>,
    journal: Journal,
    /// The app the views are narrowed to.
    filter: Option<String>,
    paused: bool,
    note: String,
    last_tick: u64,
}

impl Peek {
    fn new() -> Self {
        let mut p = Peek {
            page: Page::Timeline, menu: None, journal: Journal::default(),
            filter: None, paused: false, note: String::new(), last_tick: host::ticks_ms(),
        };
        if !p.journal.refresh() {
            p.note = "The kernel did not hand over the journal".to_string();
        }
        p
    }
}

// ── Events ────────────────────────────────────────────────────────────

enum Outcome { Idle, Render, Exit }

fn handle(p: &mut Peek, ev: Event) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => {
            if p.menu.is_some() { p.menu = None; Outcome::Render } else { Outcome::Exit }
        }
        Event::Action(ActionId(id)) => handle_action(p, id),
        _ => Outcome::Idle,
    }
}

fn handle_action(p: &mut Peek, id: u32) -> Outcome {
    if id >= ACT_APP_BASE {
        let names = p.journal.app_names();
        if let Some(n) = names.get((id - ACT_APP_BASE) as usize) {
            p.filter = Some(n.clone());
            if p.page == Page::Apps { p.page = Page::Timeline; }
        }
        p.menu = None;
        return Outcome::Render;
    }
    if let Some(i) = id.checked_sub(ACT_PAGE_BASE).map(|i| i as usize).filter(|&i| i < PAGES.len()) {
        p.menu = None;
        p.page = PAGES[i];
        return Outcome::Render;
    }
    match id {
        ACT_MENU_PEEK => { p.menu = toggle(p.menu, Menu::Peek); Outcome::Render }
        ACT_MENU_VIEW => { p.menu = toggle(p.menu, Menu::View); Outcome::Render }
        ACT_MENU_DISMISS => { p.menu = None; Outcome::Render }
        ACT_CLOSE => Outcome::Exit,
        ACT_ABOUT => { p.menu = None; p.note = alloc::format!("peek {}", VERSION); Outcome::Render }
        ACT_PAUSE => {
            p.paused = !p.paused;
            p.note = if p.paused { "Paused".to_string() } else { "Live".to_string() };
            if !p.paused { p.journal.refresh(); }
            Outcome::Render
        }
        ACT_ALL_APPS => { p.filter = None; p.menu = None; Outcome::Render }
        _ => Outcome::Idle,
    }
}

fn toggle(cur: Option<Menu>, want: Menu) -> Option<Menu> {
    if cur == Some(want) { None } else { Some(want) }
}

// ── Scene ─────────────────────────────────────────────────────────────

fn text(content: String, style: TextStyle) -> Widget {
    Widget::Text { content, style, modifiers: Vec::new() }
}

fn styled(content: String, style: TextStyle, mods: Vec<Modifier>) -> Widget {
    Widget::Text { content, style, modifiers: mods }
}

fn muted(content: &str) -> Widget {
    styled(content.to_string(), TextStyle::Caption, alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)])
}

fn heading(content: &str) -> Widget { text(content.to_string(), TextStyle::Heading) }

fn column(children: Vec<Widget>, spacing: Spacing) -> Widget {
    Widget::Column { children, spacing: spacing.as_u16(), align: Align::Stretch, modifiers: Vec::new() }
}

fn row(children: Vec<Widget>, spacing: Spacing) -> Widget {
    Widget::Row { children, spacing: spacing.as_u16(), align: Align::Center, modifiers: Vec::new() }
}

/// 1234567 as `1'234'567`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 { out.push('\''); }
        out.push(c);
    }
    out
}

fn clock(local: u64) -> String {
    if local == 0 { return "–".to_string(); }
    let s = local % 86_400;
    alloc::format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn tile(icon: IconId, tint: Token, label: &str, value: String, sub: &str) -> Widget {
    let head = row(alloc::vec![
        Widget::Icon { id: icon, size: 16, modifiers: alloc::vec![Modifier::Tint(tint)] },
        muted(label),
    ], Spacing::Xs);
    let mut body = alloc::vec![head, text(value, TextStyle::Title)];
    if !sub.is_empty() { body.push(muted(sub)); }
    let mut card = prefab::card(column(body, Spacing::Xs), CardKind::Panel);
    if let Widget::Column { modifiers, .. } = &mut card {
        modifiers.push(Modifier::Flex(1));
        modifiers.push(Modifier::MinWidth(TILE_MIN_W));
    }
    card
}

fn kind_look(kind: &str, object: &str) -> (IconId, Token) {
    match kind {
        "launch" => (IconId::Play, Token::Success),
        "exit" => (IconId::Power, Token::OnSurfaceMuted),
        "DENIED" => (IconId::Lock, Token::Danger),
        "grant" => (IconId::Check, Token::Accent),
        "read" if object == "screen" => (IconId::Monitor, Token::Warning),
        "read" => (IconId::FileText, Token::OnSurfaceMuted),
        "write" | "create" => (IconId::File, Token::Accent),
        "delete" => (IconId::Trash, Token::Warning),
        "rename" => (IconId::ArrowRight, Token::OnSurfaceMuted),
        "mkdir" => (IconId::Folder, Token::OnSurfaceMuted),
        "net" => (IconId::Globe, Token::Accent),
        "on" | "off" => {
            let icon = if object.contains("camera") { IconId::Camera }
                else if object.contains("micro") { IconId::Microphone }
                else { IconId::Monitor };
            (icon, if kind == "on" { Token::Warning } else { Token::OnSurfaceMuted })
        }
        "setting" => (IconId::SlidersHorizontal, Token::Accent),
        _ => (IconId::DotsThree, Token::OnSurfaceMuted),
    }
}

/// What a row says after the kind.
fn detail(e: &Entry) -> String {
    match e.kind.as_str() {
        "launch" => alloc::format!("token {}", e.rights),
        "exit" => alloc::format!("{} checks passed, {} refused", grouped(e.checks), e.denied),
        "DENIED" => alloc::format!("{}  needs {}", e.object, e.rights),
        "grant" => alloc::format!("{}  {}", e.object, e.rights),
        _ => e.object.clone(),
    }
}

fn timeline_row(j: &Journal, e: &Entry) -> Widget {
    let (icon, tint) = kind_look(&e.kind, &e.object);
    let denied = e.is_denied();
    let main_tint = if denied { Token::Danger } else { Token::OnSurface };
    let mut children = alloc::vec![
        styled(clock(j.local_of(e)), TextStyle::Mono,
            alloc::vec![Modifier::MinWidth(TIME_W), Modifier::Tint(Token::OnSurfaceMuted)]),
        Widget::Icon { id: icon, size: 16, modifiers: alloc::vec![Modifier::Tint(tint)] },
        styled(alloc::format!("{}#{}", e.holder, e.inst), TextStyle::Body,
            alloc::vec![Modifier::MinWidth(WHO_W), Modifier::MaxWidth(WHO_W)]),
        styled(e.kind.clone(), TextStyle::Caption,
            alloc::vec![Modifier::MinWidth(KIND_W), Modifier::Tint(if denied { Token::Danger } else { Token::OnSurfaceMuted })]),
        styled(detail(e), TextStyle::Body, alloc::vec![Modifier::Flex(1), Modifier::Tint(main_tint)]),
    ];
    if e.count > 1 {
        children.push(styled(alloc::format!("×{}", grouped(e.count as u64)), TextStyle::Mono,
            alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)]));
    }
    let mut mods = alloc::vec![Modifier::Padding(Padding::Xs.as_u16())];
    if denied { mods.push(Modifier::Background(Token::SurfaceMuted)); }
    Widget::Row { children, spacing: Spacing::Sm.as_u16(), align: Align::Center, modifiers: mods }
}

fn summary_tiles(j: &Journal) -> Widget {
    let running = j.live.values().filter(|(n, _)| !is_system(n)).count();
    let refused = j.refused();
    let files = j.distinct(|e| is_file(&e.kind));
    let hosts = j.distinct(|e| e.kind == "net");
    row(alloc::vec![
        tile(IconId::Lock, Token::Accent, "Token checks", grouped(j.checks_passed), "passed since boot"),
        tile(IconId::SquaresFour, Token::Success, "Apps running", running.to_string(), ""),
        tile(IconId::Lock, if refused > 0 { Token::Danger } else { Token::OnSurfaceMuted },
            "Refused", grouped(refused), ""),
        tile(IconId::FileText, Token::OnSurfaceMuted, "Files touched", files.to_string(), ""),
        tile(IconId::Globe, Token::OnSurfaceMuted, "Hosts reached", hosts.to_string(), ""),
    ], Spacing::Md)
}

fn render_timeline(p: &Peek, only_refused: bool) -> Vec<Widget> {
    let j = &p.journal;
    let title = match (&p.filter, only_refused) {
        (Some(f), false) => alloc::format!("Timeline · {}", f),
        (Some(f), true) => alloc::format!("Refused · {}", f),
        (None, false) => "Timeline".to_string(),
        (None, true) => "Refused".to_string(),
    };
    let rows: Vec<Widget> = j.entries.values().rev()
        .filter(|e| p.filter.as_ref().is_none_or(|f| &e.holder == f))
        .filter(|e| !only_refused || e.is_denied())
        .take(TIMELINE_ROWS)
        .map(|e| timeline_row(j, e))
        .collect();
    let mut out = alloc::vec![heading(&title), summary_tiles(j)];
    if rows.is_empty() {
        out.push(muted(if only_refused { "Nothing was refused." } else { "Nothing recorded yet." }));
    } else {
        out.push(prefab::card(column(rows, Spacing::None), CardKind::Inset));
    }
    out
}

fn list_line(label: &str, items: &[String]) -> Widget {
    let value = if items.is_empty() {
        "–".to_string()
    } else {
        let shown: Vec<&str> = items.iter().take(4).map(|s| s.as_str()).collect();
        let more = if items.len() > 4 { alloc::format!("  +{}", items.len() - 4) } else { String::new() };
        alloc::format!("{}{}", shown.join(", "), more)
    };
    row(alloc::vec![
        styled(label.to_string(), TextStyle::Caption,
            alloc::vec![Modifier::MinWidth(KIND_W), Modifier::Tint(Token::OnSurfaceMuted)]),
        styled(value, TextStyle::Body, alloc::vec![Modifier::Flex(1)]),
    ], Spacing::Sm)
}

fn app_card(j: &Journal, name: &str, index: usize) -> Widget {
    let a = summarize(j, name);
    let state = if a.running > 0 { ("running", Token::Success) } else { ("ended", Token::OnSurfaceMuted) };
    let head = row(alloc::vec![
        styled(name.to_string(), TextStyle::Title, alloc::vec![Modifier::Flex(1)]),
        styled(state.0.to_string(), TextStyle::Caption, alloc::vec![Modifier::Tint(state.1)]),
        prefab::button("Timeline", ButtonStyle::Secondary, ActionId(ACT_APP_BASE + index as u32)),
    ], Spacing::Sm);
    let counts = alloc::format!("{} start(s) · {} checks passed · {} refused",
        a.launches, grouped(a.checks), grouped(a.refused));
    let mut body = alloc::vec![
        head,
        styled(counts, TextStyle::Caption,
            alloc::vec![Modifier::Tint(if a.refused > 0 { Token::Danger } else { Token::OnSurfaceMuted })]),
    ];
    if !a.rights.is_empty() { body.push(list_line("token", core::slice::from_ref(&a.rights))); }
    body.push(list_line("files", &a.files));
    body.push(list_line("hosts", &a.hosts));
    if !a.devices.is_empty() { body.push(list_line("devices", &a.devices)); }
    prefab::card(column(body, Spacing::Xs), CardKind::Panel)
}

fn render_apps(p: &Peek) -> Vec<Widget> {
    let j = &p.journal;
    let names = j.app_names();
    let mut out = alloc::vec![heading("Apps"), summary_tiles(j)];
    for (i, n) in names.iter().enumerate() {
        if p.filter.as_ref().is_some_and(|f| f != n) { continue; }
        out.push(app_card(j, n, i));
    }
    out
}

fn render_sidebar(p: &Peek) -> Widget {
    let nav = |pg: Page| {
        let i = PAGES.iter().position(|x| *x == pg).unwrap_or(0) as u32;
        prefab::nav_row(pg.icon(), pg.label(), p.page == pg, Some(ActionId(ACT_PAGE_BASE + i)), None)
    };
    let names = p.journal.app_names();
    let mut apps = alloc::vec![prefab::nav_row(IconId::SquaresFour, "All apps", p.filter.is_none(),
        Some(ActionId(ACT_ALL_APPS)), None)];
    for (i, n) in names.iter().enumerate() {
        let live = p.journal.live.values().any(|(name, _)| name == n);
        let icon = if live { IconId::PlayCircle } else { IconId::DotsThree };
        apps.push(prefab::nav_row(icon, n, p.filter.as_deref() == Some(n.as_str()),
            Some(ActionId(ACT_APP_BASE + i as u32)), None));
    }
    Widget::Column {
        children: alloc::vec![
            prefab::sidebar_section("VIEWS", PAGES.iter().map(|pg| nav(*pg)).collect()),
            prefab::sidebar_section("APPS", apps),
            Widget::Spacer { flex: 1 },
        ],
        spacing: Spacing::Xs.as_u16(),
        align: Align::Stretch,
        modifiers: alloc::vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Padding(Padding::Xs.as_u16()),
            Modifier::MinWidth(SIDEBAR_W),
            Modifier::MaxWidth(SIDEBAR_W),
        ],
    }
}

fn render_dropdown(p: &Peek, menu: Menu) -> (u32, Widget) {
    match menu {
        Menu::Peek => (NODE_MENU_PEEK, prefab::popover_menu(&[
            ((if p.paused { "Resume live" } else { "Pause" }).to_string(), ActionId(ACT_PAUSE)),
            ("About Peek".to_string(), ActionId(ACT_ABOUT)),
            ("Close".to_string(), ActionId(ACT_CLOSE)),
        ], None)),
        Menu::View => {
            let mut items: Vec<(String, ActionId)> = PAGES.iter().enumerate()
                .map(|(i, pg)| (pg.label().to_string(), ActionId(ACT_PAGE_BASE + i as u32)))
                .collect();
            items.push(("All apps".to_string(), ActionId(ACT_ALL_APPS)));
            (NODE_MENU_VIEW, prefab::popover_menu(&items, None))
        }
    }
}

fn render(p: &Peek) -> Widget {
    let page = match p.page {
        Page::Timeline => render_timeline(p, false),
        Page::Refused => render_timeline(p, true),
        Page::Apps => render_apps(p),
    };
    let content = Widget::Scroll {
        child: Box::new(Widget::Column {
            children: page,
            spacing: Spacing::Md.as_u16(),
            align: Align::Stretch,
            modifiers: alloc::vec![Modifier::Padding(Padding::Lg.as_u16())],
        }),
        axis: Axis::Vertical,
        modifiers: alloc::vec![Modifier::Flex(1), Modifier::Background(Token::Surface)],
    };
    let body = Widget::Row {
        children: alloc::vec![render_sidebar(p), content],
        spacing: Spacing::None.as_u16(),
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::Flex(1)],
    };
    let right = alloc::format!("{} · {} entries", if p.paused { "paused" } else { "live" }, grouped(p.journal.written));
    let mut children = alloc::vec![
        prefab::menu_bar_with_icon(
            IconId::MagnifyingGlass,
            &[
                ("Peek".to_string(), ActionId(ACT_MENU_PEEK)),
                ("View".to_string(), ActionId(ACT_MENU_VIEW)),
            ],
            &[NodeId(NODE_MENU_PEEK), NodeId(NODE_MENU_VIEW)],
        ),
        Widget::Divider,
        body,
        Widget::Divider,
        prefab::footer(&p.note, &right),
    ];
    if let Some(m) = p.menu {
        let (anchor, content) = render_dropdown(p, m);
        children.push(Widget::Popover {
            anchor: NodeId(anchor),
            child: Box::new(content),
            on_dismiss: ActionId(ACT_MENU_DISMISS),
            modifiers: Vec::new(),
        });
    }
    Widget::Column {
        children,
        spacing: Spacing::None.as_u16(),
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::Background(Token::Surface)],
    }
}

fn commit_scene(p: &Peek) {
    match wire::encode(&render(p)) {
        Ok(bytes) => { if !host::scene_commit(&bytes) { log("[peek] commit failed"); } }
        Err(_) => log("[peek] encode failed"),
    }
}

// ── Entry ─────────────────────────────────────────────────────────────

const EVENT_BUF_SIZE: usize = 4 * 1024;
/// How often the live view asks the kernel for news.
const TICK_MS: u64 = 1000;

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut p = Peek::new();
    let mut buf = alloc::vec![0u8; EVENT_BUF_SIZE];
    commit_scene(&p);
    loop {
        match nopeek_widgets::events::poll(&mut buf) {
            nopeek_widgets::events::Poll::Event(ev) => match handle(&mut p, ev) {
                Outcome::Idle => {}
                Outcome::Render => commit_scene(&p),
                Outcome::Exit => { host::close_widget(); return; }
            },
            nopeek_widgets::events::Poll::Empty => {
                let now = host::ticks_ms();
                if !p.paused && now.saturating_sub(p.last_tick) >= TICK_MS {
                    p.last_tick = now;
                    let before = (p.journal.written, p.journal.checks_passed, p.journal.live.len());
                    if p.journal.refresh()
                        && before != (p.journal.written, p.journal.checks_passed, p.journal.live.len())
                    {
                        commit_scene(&p);
                    }
                }
                host::sleep_ms(50);
            }
            nopeek_widgets::events::Poll::Gone => return,
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<IconRef> { None }
