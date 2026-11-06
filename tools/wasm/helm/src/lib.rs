//! helm — the settings cockpit for nopeekOS, in loft's window pattern.
//!
//! Layout: menu bar · sidebar (pages) + scrolled content · status line.
//! Every system setting is read with `npk_setting_get` and changed with
//! `npk_setting_set` (docs/spec/SETTINGS.md); the kernel validates, so a
//! refused value shows in the status line instead of being guessed here.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use nopeek_widgets::app_meta::IconRef;
use nopeek_widgets::host;
use nopeek_widgets::i18n;
use nopeek_widgets::prefab::{self, ButtonStyle, CardKind};
use nopeek_widgets::style::{Padding, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// READ: the wallpapers folder and disk usage. RENDER: the window, the
// status list, network and setting reads. SETTINGS: setting writes.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::RENDER, caps::ext::SETTINGS];

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_fs_usage() -> i64;
    fn npk_status_list(buf_ptr: i32, max: i32) -> i32;
}

fn log(msg: &str) { host::log_serial(msg); }

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { log("[helm] panic!"); core::arch::wasm32::unreachable() }

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Strings ───────────────────────────────────────────────────────────

struct Strings {
    menu_helm: &'static str, menu_view: &'static str,
    close: &'static str, about: &'static str,
    system: &'static str, personal: &'static str,
    cockpit: &'static str, appearance: &'static str, display: &'static str,
    sound: &'static str, input: &'static str,
    cpu: &'static str, memory: &'static str, disk: &'static str,
    network: &'static str, volume: &'static str, uptime: &'static str,
    status: &'static str, offline: &'static str, free: &'static str,
    theme: &'static str, dark: &'static str, light: &'static str, auto: &'static str,
    accent: &'static str, wallpaper: &'static str, no_wallpapers: &'static str,
    scale: &'static str, brightness: &'static str, no_backlight: &'static str,
    screen: &'static str, output_level: &'static str, devices_hint: &'static str,
    keyboard: &'static str, mouse_speed: &'static str, mouse_size: &'static str,
    saved: &'static str, refused: &'static str, not_allowed: &'static str,
}

const EN: Strings = Strings {
    menu_helm: "Helm", menu_view: "View",
    close: "Close", about: "About Helm",
    system: "SYSTEM", personal: "PERSONAL",
    cockpit: "Cockpit", appearance: "Appearance", display: "Display",
    sound: "Sound", input: "Input",
    cpu: "CPU", memory: "Memory", disk: "Disk",
    network: "Network", volume: "Volume", uptime: "Uptime",
    status: "Status", offline: "offline", free: "free",
    theme: "Theme", dark: "Dark", light: "Light", auto: "Auto",
    accent: "Accent", wallpaper: "Wallpaper", no_wallpapers: "No pictures in pictures/wallpapers",
    scale: "Interface scale", brightness: "Brightness", no_backlight: "No driver sets the backlight of this display",
    screen: "Screen", output_level: "Output level", devices_hint: "Output, input and camera devices: the volume panel in the deck",
    keyboard: "Keyboard layout", mouse_speed: "Pointer speed", mouse_size: "Pointer size",
    saved: "Saved", refused: "Value refused", not_allowed: "Not allowed",
};

const DE: Strings = Strings {
    menu_helm: "Helm", menu_view: "Ansicht",
    close: "Schliessen", about: "Über Helm",
    system: "SYSTEM", personal: "PERSÖNLICH",
    cockpit: "Cockpit", appearance: "Darstellung", display: "Anzeige",
    sound: "Ton", input: "Eingabe",
    cpu: "CPU", memory: "Speicher", disk: "Disk",
    network: "Netzwerk", volume: "Lautstärke", uptime: "Laufzeit",
    status: "Status", offline: "offline", free: "frei",
    theme: "Thema", dark: "Dunkel", light: "Hell", auto: "Auto",
    accent: "Akzent", wallpaper: "Hintergrund", no_wallpapers: "Keine Bilder in pictures/wallpapers",
    scale: "Skalierung", brightness: "Helligkeit", no_backlight: "Kein Treiber setzt die Helligkeit dieses Bildschirms",
    screen: "Bildschirm", output_level: "Ausgabe", devices_hint: "Ausgabe-, Eingabe- und Kamerageräte: das Lautstärke-Panel im deck",
    keyboard: "Tastaturbelegung", mouse_speed: "Zeigertempo", mouse_size: "Zeigergrösse",
    saved: "Gespeichert", refused: "Wert abgelehnt", not_allowed: "Nicht erlaubt",
};

fn s() -> &'static Strings {
    match i18n::lang() { i18n::Lang::De => &DE, _ => &EN }
}

// ── Choices ───────────────────────────────────────────────────────────

const THEMES: [&str; 3] = ["dark", "light", "auto"];
const ACCENTS: [&str; 5] = ["auto", "rose", "sage", "blue", "amber"];
const SCALES: [&str; 5] = ["1.0", "1.25", "1.5", "1.75", "2.0"];
const KEYBOARDS: [&str; 3] = ["de_CH", "de_DE", "us"];
const MOUSE_SPEED: (i32, i32) = (25, 600);
const MOUSE_SIZE: (i32, i32) = (50, 300);

// ── Actions ───────────────────────────────────────────────────────────

const ACT_MENU_HELM: u32 = 1;
const ACT_MENU_VIEW: u32 = 2;
const ACT_MENU_DISMISS: u32 = 3;
const ACT_CLOSE: u32 = 4;
const ACT_ABOUT: u32 = 5;
const ACT_PAGE_BASE: u32 = 10;
const ACT_THEME_BASE: u32 = 100;
const ACT_ACCENT_BASE: u32 = 110;
const ACT_SCALE_BASE: u32 = 120;
const ACT_KEYBOARD_BASE: u32 = 130;
const ACT_BRIGHTNESS: u32 = 200;
const ACT_VOLUME: u32 = 201;
const ACT_MOUSE_SPEED: u32 = 202;
const ACT_MOUSE_SIZE: u32 = 203;
const ACT_WALLPAPER_BASE: u32 = 1000;

const NODE_MENU_HELM: u32 = 100;
const NODE_MENU_VIEW: u32 = 101;

const SIDEBAR_W: u16 = 176;
const TILE_MIN_W: u16 = 150;
const VALUE_W: u16 = 52;

#[derive(Clone, Copy, PartialEq)]
enum Page { Cockpit, Appearance, Display, Sound, Input }

const PAGES: [Page; 5] = [Page::Cockpit, Page::Appearance, Page::Display, Page::Sound, Page::Input];

impl Page {
    fn label(self) -> &'static str {
        match self {
            Page::Cockpit => s().cockpit,
            Page::Appearance => s().appearance,
            Page::Display => s().display,
            Page::Sound => s().sound,
            Page::Input => s().input,
        }
    }
    fn icon(self) -> IconId {
        match self {
            Page::Cockpit => IconId::Gauge,
            Page::Appearance => IconId::Palette,
            Page::Display => IconId::Monitor,
            Page::Sound => IconId::SpeakerHigh,
            Page::Input => IconId::Keyboard,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Menu { Helm, View }

// ── State ─────────────────────────────────────────────────────────────

/// The system settings as the kernel reported them last.
#[derive(Default, PartialEq)]
struct Settings {
    theme: String,
    accent: String,
    wallpaper: String,
    scale: String,
    brightness: Option<u32>,
    keyboard: String,
    mouse_speed: i32,
    mouse_size: i32,
}

impl Settings {
    fn read() -> Self {
        let get = |k: &str| host::setting_get(k).unwrap_or_default();
        let num = |k: &str, d: i32| host::setting_get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        Settings {
            theme: get("theme"),
            accent: get("accent"),
            wallpaper: get("wallpaper"),
            scale: get("shade.scale"),
            brightness: host::setting_get("brightness").and_then(|v| v.parse().ok()),
            keyboard: get("keyboard"),
            mouse_speed: num("mouse_speed", 100),
            mouse_size: num("mouse_size", 100),
        }
    }
}

/// Live system values for the cockpit.
#[derive(Default, PartialEq)]
struct Live {
    cpu_pct: i64,
    cores: i64,
    free_mb: i64,
    disk: Option<(u64, u64)>,
    network: Option<String>,
    volume: Option<u32>,
    uptime: i64,
    items: Vec<(IconId, String)>,
}

impl Live {
    fn read() -> Self {
        let cores = npk_sys::sys_info(0).max(0);
        let mut sum = 0i64;
        for c in 0..cores {
            sum += npk_sys::sys_info(15 | ((c as i32) << 8)).max(0);
        }
        // SAFETY: FFI without pointers.
        let usage = unsafe { npk_fs_usage() };
        let disk = (usage >= 0).then(|| ((usage as u64) >> 32, usage as u64 & 0xFFFF_FFFF));
        Live {
            cpu_pct: if cores > 0 { sum / cores } else { 0 },
            cores,
            free_mb: npk_sys::sys_info(2),
            disk,
            network: network_summary(),
            volume: host::audio_volume(),
            uptime: npk_sys::sys_info(1),
            items: status_items(),
        }
    }
}

struct Helm {
    page: Page,
    menu: Option<Menu>,
    set: Settings,
    live: Live,
    wallpapers: Vec<String>,
    /// The status line: the last change and how the kernel answered.
    note: String,
    /// While a pointer slider drags: its value, not yet sent.
    mouse_drag: Option<(u32, i32)>,
    last_tick: u64,
}

impl Helm {
    fn new() -> Self {
        let mut h = Helm {
            page: Page::Cockpit, menu: None,
            set: Settings::read(), live: Live::read(),
            wallpapers: Vec::new(), note: String::new(),
            mouse_drag: None, last_tick: host::ticks_ms(),
        };
        h.wallpapers = list_wallpapers();
        h
    }

    /// Change `key` and say how it went; re-read so the view shows what
    /// the kernel stored.
    fn change(&mut self, key: &str, value: &str) {
        let r = host::setting_set(key, value);
        self.note = match r {
            0 => alloc::format!("{}: {} = {}", s().saved, key, value),
            -2 => alloc::format!("{}: {} = {}", s().refused, key, value),
            _ => alloc::format!("{}: {}", s().not_allowed, key),
        };
        if r != 0 { log(&alloc::format!("[helm] {} -> {}", key, r)); }
        self.set = Settings::read();
    }
}

fn wallpaper_dir() -> String {
    let home = host::home_dir().filter(|h| !h.is_empty()).unwrap_or_else(|| "home".to_string());
    alloc::format!("{}/pictures/wallpapers", home)
}

fn list_wallpapers() -> Vec<String> {
    let listing = host::fs_list(&wallpaper_dir(), false).unwrap_or_default();
    let mut out: Vec<String> = nopeek_widgets::fs::list_entries(&listing)
        .filter(|e| !e.is_dir && !e.name.starts_with('.'))
        .map(|e| e.name.to_string())
        .collect();
    out.sort();
    out
}

/// `ipv4 a.b.c.d/p` and the primary interface out of `npk_net_info`.
fn network_summary() -> Option<String> {
    let info = host::net_info()?;
    let text = core::str::from_utf8(&info).ok()?;
    let mut iface = None;
    let mut addr = None;
    for line in text.lines() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("iface") => {
                let name = w.next();
                let f: Vec<&str> = w.collect();
                // <driver> <mac> <up> <primary>
                if f.get(2) == Some(&"1") && (iface.is_none() || f.get(3) == Some(&"1")) {
                    iface = name;
                }
            }
            Some("ipv4") if addr.is_none() => addr = w.next(),
            _ => {}
        }
    }
    match (iface, addr) {
        (Some(i), Some(a)) => Some(alloc::format!("{} · {}", a, i)),
        (None, Some(a)) => Some(a.to_string()),
        _ => None,
    }
}

/// The status items (format in kernel `status.rs`), icon and text.
fn status_items() -> Vec<(IconId, String)> {
    // SAFETY: FFI; the buffer is borrowed for the call.
    let Some(b) = host::read_all(|buf| unsafe { npk_status_list(buf.as_mut_ptr() as i32, buf.len() as i32) })
    else { return Vec::new() };
    let mut v = Vec::new();
    let Some(count) = b.get(..2).map(|c| u16::from_le_bytes([c[0], c[1]])) else { return v };
    let mut p = 2usize;
    let st = |p: &mut usize| -> Option<String> {
        let n = *b.get(*p)? as usize;
        let t = core::str::from_utf8(b.get(*p + 1..*p + 1 + n)?).ok()?;
        *p += 1 + n;
        Some(t.to_string())
    };
    for _ in 0..count {
        if st(&mut p).is_none() || st(&mut p).is_none() { break; }
        let Some(f) = b.get(p..p + 8) else { break };
        let icon = IconId(u16::from_le_bytes([f[0], f[1]]));
        p += 8;
        let Some(text) = st(&mut p) else { break };
        if st(&mut p).is_none() { break; }
        if !text.is_empty() { v.push((icon, text)); }
    }
    v
}

// ── Events ────────────────────────────────────────────────────────────

enum Outcome { Idle, Render, Exit }

fn handle(h: &mut Helm, ev: Event) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => {
            if h.menu.is_some() { h.menu = None; Outcome::Render } else { Outcome::Exit }
        }
        Event::Focus(true) => {
            h.set = Settings::read();
            h.wallpapers = list_wallpapers();
            Outcome::Render
        }
        Event::Action(ActionId(id)) => handle_action(h, id),
        Event::Slide { action: ActionId(id), value, done } => handle_slide(h, id, value as u32, done),
        _ => Outcome::Idle,
    }
}

fn handle_action(h: &mut Helm, id: u32) -> Outcome {
    let pick = |base: u32, n: usize| id.checked_sub(base).map(|i| i as usize).filter(|&i| i < n);
    if id >= ACT_WALLPAPER_BASE {
        if let Some(name) = h.wallpapers.get((id - ACT_WALLPAPER_BASE) as usize).cloned() {
            h.change("wallpaper", &name);
        }
        return Outcome::Render;
    }
    if let Some(i) = pick(ACT_PAGE_BASE, PAGES.len()) {
        h.menu = None;
        h.page = PAGES[i];
        h.set = Settings::read();
        if h.page == Page::Cockpit { h.live = Live::read(); }
        if h.page == Page::Appearance { h.wallpapers = list_wallpapers(); }
        if h.page == Page::Sound { h.live.volume = host::audio_volume(); }
        return Outcome::Render;
    }
    if let Some(i) = pick(ACT_THEME_BASE, THEMES.len()) { h.change("theme", THEMES[i]); return Outcome::Render; }
    if let Some(i) = pick(ACT_ACCENT_BASE, ACCENTS.len()) { h.change("accent", ACCENTS[i]); return Outcome::Render; }
    if let Some(i) = pick(ACT_SCALE_BASE, SCALES.len()) { h.change("shade.scale", SCALES[i]); return Outcome::Render; }
    if let Some(i) = pick(ACT_KEYBOARD_BASE, KEYBOARDS.len()) { h.change("keyboard", KEYBOARDS[i]); return Outcome::Render; }
    match id {
        ACT_MENU_HELM => { h.menu = toggle(h.menu, Menu::Helm); Outcome::Render }
        ACT_MENU_VIEW => { h.menu = toggle(h.menu, Menu::View); Outcome::Render }
        ACT_MENU_DISMISS => { h.menu = None; Outcome::Render }
        ACT_CLOSE => Outcome::Exit,
        ACT_ABOUT => {
            h.menu = None;
            h.note = alloc::format!("helm {}", VERSION);
            Outcome::Render
        }
        _ => Outcome::Idle,
    }
}

fn toggle(cur: Option<Menu>, want: Menu) -> Option<Menu> {
    if cur == Some(want) { None } else { Some(want) }
}

fn from_slider(v: u32, (lo, hi): (i32, i32)) -> i32 {
    lo + ((hi - lo) as u32 * v.min(SLIDER_MAX as u32) / SLIDER_MAX as u32) as i32
}

fn to_slider(v: i32, (lo, hi): (i32, i32)) -> u16 {
    ((v.clamp(lo, hi) - lo) as u32 * SLIDER_MAX as u32 / (hi - lo) as u32) as u16
}

fn handle_slide(h: &mut Helm, id: u32, value: u32, done: bool) -> Outcome {
    match id {
        // The backlight is not stored, and the kernel keeps only the newest
        // queued value: every step goes out.
        ACT_BRIGHTNESS => {
            let pct = value * 100 / SLIDER_MAX as u32;
            h.change("brightness", &pct.to_string());
            Outcome::Render
        }
        ACT_VOLUME => {
            let pct = value * 100 / SLIDER_MAX as u32;
            host::audio_set_volume(pct);
            h.live.volume = host::audio_volume();
            Outcome::Render
        }
        // Pointer settings are stored on every write: sent once on release.
        ACT_MOUSE_SPEED | ACT_MOUSE_SIZE => {
            let (key, range) = if id == ACT_MOUSE_SPEED {
                ("mouse_speed", MOUSE_SPEED)
            } else {
                ("mouse_size", MOUSE_SIZE)
            };
            let n = from_slider(value, range);
            if done {
                h.mouse_drag = None;
                h.change(key, &n.to_string());
            } else {
                h.mouse_drag = Some((id, n));
            }
            Outcome::Render
        }
        _ => Outcome::Idle,
    }
}

// ── Scene ─────────────────────────────────────────────────────────────

fn text(content: String, style: TextStyle) -> Widget {
    Widget::Text { content, style, modifiers: Vec::new() }
}

fn muted(content: &str) -> Widget {
    Widget::Text {
        content: content.to_string(),
        style: TextStyle::Caption,
        modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)],
    }
}

fn heading(content: &str) -> Widget {
    text(content.to_string(), TextStyle::Heading)
}

fn column(children: Vec<Widget>, spacing: Spacing) -> Widget {
    Widget::Column { children, spacing: spacing.as_u16(), align: Align::Stretch, modifiers: Vec::new() }
}

fn row(children: Vec<Widget>) -> Widget {
    Widget::Row { children, spacing: Spacing::Sm.as_u16(), align: Align::Center, modifiers: Vec::new() }
}

/// A row of buttons, the chosen one filled: a segmented control.
fn choices(labels: &[&str], chosen: Option<usize>, base: u32) -> Widget {
    let buttons = labels.iter().enumerate().map(|(i, l)| {
        let style = if chosen == Some(i) { ButtonStyle::Primary } else { ButtonStyle::Secondary };
        prefab::button(l, style, ActionId(base + i as u32))
    }).collect();
    Widget::Row { children: buttons, spacing: Spacing::Xs.as_u16(), align: Align::Center, modifiers: Vec::new() }
}

/// A labelled setting inside a card.
fn setting(label: &str, control: Widget) -> Widget {
    prefab::card(column(alloc::vec![muted(label), control], Spacing::Sm), CardKind::Inset)
}

fn slider(label_value: String, value: u16, action: u32) -> Widget {
    row(alloc::vec![
        Widget::Slider { value, on_change: ActionId(action), modifiers: alloc::vec![Modifier::Flex(1)] },
        Widget::Text {
            content: label_value,
            style: TextStyle::Mono,
            modifiers: alloc::vec![Modifier::MinWidth(VALUE_W), Modifier::Tint(Token::OnSurfaceMuted)],
        },
    ])
}

fn tile(icon: IconId, label: &str, value: String, sub: String) -> Widget {
    let head = Widget::Row {
        children: alloc::vec![
            Widget::Icon { id: icon, size: 16, modifiers: alloc::vec![Modifier::Tint(Token::Accent)] },
            muted(label),
        ],
        spacing: Spacing::Xs.as_u16(),
        align: Align::Center,
        modifiers: Vec::new(),
    };
    let mut body = alloc::vec![head, text(value, TextStyle::Title)];
    if !sub.is_empty() { body.push(muted(&sub)); }
    let mut card = prefab::card(column(body, Spacing::Xs), CardKind::Panel);
    if let Widget::Column { modifiers, .. } = &mut card {
        modifiers.push(Modifier::Flex(1));
        modifiers.push(Modifier::MinWidth(TILE_MIN_W));
    }
    card
}

fn tiles(items: Vec<Widget>) -> Widget {
    let mut rows = Vec::new();
    let mut it = items.into_iter().peekable();
    while it.peek().is_some() {
        let chunk: Vec<Widget> = it.by_ref().take(3).collect();
        rows.push(Widget::Row { children: chunk, spacing: Spacing::Md.as_u16(), align: Align::Stretch, modifiers: Vec::new() });
    }
    column(rows, Spacing::Md)
}

fn uptime_text(secs: i64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 { alloc::format!("{}d {}h", d, h) } else { alloc::format!("{}h {}m", h, m) }
}

fn render_cockpit(h: &Helm) -> Vec<Widget> {
    let l = &h.live;
    let disk = match l.disk {
        Some((used, total)) if total > 0 => (
            alloc::format!("{} %", used * 100 / total),
            alloc::format!("{:.1} / {:.1} GB", used as f32 / 1024.0, total as f32 / 1024.0),
        ),
        _ => ("–".to_string(), String::new()),
    };
    let net = l.network.clone().unwrap_or_else(|| s().offline.to_string());
    let vol = l.volume.map(|v| alloc::format!("{} %", v)).unwrap_or_else(|| "–".to_string());
    let mut out = alloc::vec![
        heading(s().cockpit),
        tiles(alloc::vec![
            tile(IconId::Gauge, s().cpu, alloc::format!("{} %", l.cpu_pct), alloc::format!("{} cores", l.cores)),
            tile(IconId::HardDrives, s().memory, alloc::format!("{} MB", l.free_mb.max(0)), s().free.to_string()),
            tile(IconId::HardDrives, s().disk, disk.0, disk.1),
            tile(IconId::Network, s().network, net, String::new()),
            tile(IconId::SpeakerHigh, s().volume, vol, String::new()),
            tile(IconId::ArrowClockwise, s().uptime, uptime_text(l.uptime), alloc::format!("helm {}", VERSION)),
        ]),
    ];
    if !l.items.is_empty() {
        let rows = l.items.iter().map(|(icon, t)| Widget::Row {
            children: alloc::vec![
                Widget::Icon { id: *icon, size: 16, modifiers: alloc::vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                text(t.clone(), TextStyle::Body),
            ],
            spacing: Spacing::Sm.as_u16(),
            align: Align::Center,
            modifiers: Vec::new(),
        }).collect();
        out.push(setting(s().status, column(rows, Spacing::Xs)));
    }
    out
}

fn render_appearance(h: &Helm) -> Vec<Widget> {
    let st = &h.set;
    let theme = THEMES.iter().position(|t| *t == st.theme).or(Some(2));
    let accent = ACCENTS.iter().position(|a| a.eq_ignore_ascii_case(&st.accent)).or(Some(0));
    let theme_labels = [s().dark, s().light, s().auto];
    let accent_labels = ["Auto", "Rose", "Sage", "Blue", "Amber"];
    let current = st.wallpaper.rsplit('/').next().unwrap_or("");
    let walls: Widget = if h.wallpapers.is_empty() {
        muted(s().no_wallpapers)
    } else {
        let mut rows = Vec::new();
        for (r, chunk) in h.wallpapers.chunks(3).enumerate() {
            let buttons = chunk.iter().enumerate().map(|(c, name)| {
                let i = r * 3 + c;
                let style = if name == current { ButtonStyle::Primary } else { ButtonStyle::Secondary };
                prefab::button(name, style, ActionId(ACT_WALLPAPER_BASE + i as u32))
            }).collect();
            rows.push(Widget::Row { children: buttons, spacing: Spacing::Xs.as_u16(), align: Align::Center, modifiers: Vec::new() });
        }
        column(rows, Spacing::Xs)
    };
    alloc::vec![
        heading(s().appearance),
        setting(s().theme, choices(&theme_labels, theme, ACT_THEME_BASE)),
        setting(s().accent, choices(&accent_labels, accent, ACT_ACCENT_BASE)),
        setting(s().wallpaper, walls),
    ]
}

fn scale_index(v: &str) -> Option<usize> {
    let f: f32 = v.trim().parse().ok()?;
    SCALES.iter().position(|s| s.parse::<f32>().is_ok_and(|x| (x - f).abs() < 0.01))
}

fn render_display(h: &Helm) -> Vec<Widget> {
    let bright = match h.set.brightness {
        Some(p) => slider(alloc::format!("{} %", p), (p.min(100) * SLIDER_MAX as u32 / 100) as u16, ACT_BRIGHTNESS),
        None => muted(s().no_backlight),
    };
    let (w, ht) = host::screen_size();
    alloc::vec![
        heading(s().display),
        setting(s().brightness, bright),
        setting(s().scale, choices(&SCALES, scale_index(&h.set.scale), ACT_SCALE_BASE)),
        setting(s().screen, text(alloc::format!("{} × {}", w, ht), TextStyle::Mono)),
    ]
}

fn render_sound(h: &Helm) -> Vec<Widget> {
    let level = match h.live.volume {
        Some(v) => slider(alloc::format!("{} %", v), (v.min(100) * SLIDER_MAX as u32 / 100) as u16, ACT_VOLUME),
        None => muted("–"),
    };
    alloc::vec![
        heading(s().sound),
        setting(s().output_level, level),
        muted(s().devices_hint),
    ]
}

fn render_input(h: &Helm) -> Vec<Widget> {
    let kb = KEYBOARDS.iter().position(|k| *k == h.set.keyboard).or(Some(0));
    let value = |id: u32, stored: i32| match h.mouse_drag {
        Some((d, n)) if d == id => n,
        _ => stored,
    };
    let speed = value(ACT_MOUSE_SPEED, h.set.mouse_speed);
    let size = value(ACT_MOUSE_SIZE, h.set.mouse_size);
    alloc::vec![
        heading(s().input),
        setting(s().keyboard, choices(&KEYBOARDS, kb, ACT_KEYBOARD_BASE)),
        setting(s().mouse_speed, slider(alloc::format!("{} %", speed), to_slider(speed, MOUSE_SPEED), ACT_MOUSE_SPEED)),
        setting(s().mouse_size, slider(alloc::format!("{} %", size), to_slider(size, MOUSE_SIZE), ACT_MOUSE_SIZE)),
    ]
}

fn render_sidebar(h: &Helm) -> Widget {
    let nav = |p: Page| {
        let i = PAGES.iter().position(|x| *x == p).unwrap_or(0) as u32;
        prefab::nav_row(p.icon(), p.label(), h.page == p, Some(ActionId(ACT_PAGE_BASE + i)), None)
    };
    Widget::Column {
        children: alloc::vec![
            prefab::sidebar_section(s().system, alloc::vec![nav(Page::Cockpit)]),
            prefab::sidebar_section(s().personal, alloc::vec![
                nav(Page::Appearance), nav(Page::Display), nav(Page::Sound), nav(Page::Input),
            ]),
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

fn render_dropdown(menu: Menu) -> (u32, Widget) {
    match menu {
        Menu::Helm => (NODE_MENU_HELM, prefab::popover_menu(&[
            (s().about.to_string(), ActionId(ACT_ABOUT)),
            (s().close.to_string(), ActionId(ACT_CLOSE)),
        ], None)),
        Menu::View => {
            let items: Vec<(String, ActionId)> = PAGES.iter().enumerate()
                .map(|(i, p)| (p.label().to_string(), ActionId(ACT_PAGE_BASE + i as u32)))
                .collect();
            (NODE_MENU_VIEW, prefab::popover_menu(&items, None))
        }
    }
}

fn render(h: &Helm) -> Widget {
    let page = match h.page {
        Page::Cockpit => render_cockpit(h),
        Page::Appearance => render_appearance(h),
        Page::Display => render_display(h),
        Page::Sound => render_sound(h),
        Page::Input => render_input(h),
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
        children: alloc::vec![render_sidebar(h), content],
        spacing: Spacing::None.as_u16(),
        align: Align::Stretch,
        modifiers: alloc::vec![Modifier::Flex(1)],
    };
    let mut children = alloc::vec![
        prefab::menu_bar_with_icon(
            IconId::Gear,
            &[
                (s().menu_helm.to_string(), ActionId(ACT_MENU_HELM)),
                (s().menu_view.to_string(), ActionId(ACT_MENU_VIEW)),
            ],
            &[NodeId(NODE_MENU_HELM), NodeId(NODE_MENU_VIEW)],
        ),
        Widget::Divider,
        body,
        Widget::Divider,
        prefab::footer(&h.note, h.page.label()),
    ];
    if let Some(m) = h.menu {
        let (anchor, content) = render_dropdown(m);
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

fn commit_scene(h: &Helm) {
    match wire::encode(&render(h)) {
        Ok(bytes) => { if !host::scene_commit(&bytes) { log("[helm] commit failed"); } }
        Err(_) => log("[helm] encode failed"),
    }
}

// ── Entry ─────────────────────────────────────────────────────────────

const EVENT_BUF_SIZE: usize = 4 * 1024;
/// How often the cockpit and the display page refresh their live values.
const TICK_MS: u64 = 1000;

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut h = Helm::new();
    let mut buf = alloc::vec![0u8; EVENT_BUF_SIZE];
    commit_scene(&h);
    loop {
        match nopeek_widgets::events::poll(&mut buf) {
            nopeek_widgets::events::Poll::Event(ev) => match handle(&mut h, ev) {
                Outcome::Idle => {}
                Outcome::Render => commit_scene(&h),
                Outcome::Exit => { host::close_widget(); return; }
            },
            nopeek_widgets::events::Poll::Empty => {
                let now = host::ticks_ms();
                if now.saturating_sub(h.last_tick) >= TICK_MS {
                    h.last_tick = now;
                    let changed = match h.page {
                        Page::Cockpit => {
                            let live = Live::read();
                            let c = live != h.live;
                            h.live = live;
                            c
                        }
                        // The backlight level arrives after the first read.
                        Page::Display if h.set.brightness.is_none() => {
                            let set = Settings::read();
                            let c = set != h.set;
                            h.set = set;
                            c
                        }
                        _ => false,
                    };
                    if changed { commit_scene(&h); }
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
