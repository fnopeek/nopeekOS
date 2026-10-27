//! link — the network card next to the panel's network icon.
//!
//! deck opens it with `npk_launch_at`; the
//! kernel places it next to the icon and closes it on a click outside.
//! Top: the connection the traffic takes, with its addresses. Below, when
//! there is a WLAN: the networks in range. A click on one opens its
//! actions: connect (a new network asks for its password), disconnect,
//! forget, join automatically. The tool only shows and asks; `wifid`
//! decides and the kernel keeps the password.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use nopeek_widgets::host;
use nopeek_widgets::prefab::{self, ButtonStyle};
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read + exec + render, and the shell role for a placed overlay.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::EXEC | caps::RENDER, caps::ext::SHELL];

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_window_set_overlay(w: i32, h: i32) -> i32;
    fn npk_window_set_light_dismiss(on: i32) -> i32;
}

fn set_overlay(w: i32, h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_overlay(w, h) };
}

fn set_light_dismiss(on: bool) {
    // SAFETY: FFI without pointers.
    unsafe { npk_window_set_light_dismiss(on as i32) };
}

fn log(msg: &str) { host::log_serial(msg); }

#[global_allocator]
static ALLOCATOR: nopeek_widgets::bump::Bump<{ 256 * 1024 }> = nopeek_widgets::bump::Bump::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[link] panic!");
    core::arch::wasm32::unreachable()
}

const EVENT_BUF_SIZE: usize = 2048;
const W: i32 = 340;
const H: i32 = 520;
const WAIT_INPUT: u32 = 1;
const WAIT_STATE: u32 = 32;
/// Addresses come from DHCP, which sends no event: look again this often.
const REFRESH_MS: i32 = 2000;

// Actions.
const A_SCAN: u32 = 1;
const A_DISCONNECT: u32 = 2;
const A_FORGET: u32 = 3;
const A_AUTO: u32 = 4;
const A_CONNECT: u32 = 5;
const A_REVEAL: u32 = 6;
const A_COPY_IP: u32 = 7;
const A_ROW_BASE: u32 = 1000;

const NODE_PASS: u32 = 100;

// Requests.
const REQ_SCAN: u8 = 0x01;
const REQ_CONNECT: u8 = 0x02;
const REQ_DISCONNECT: u8 = 0x03;
const REQ_FORGET: u8 = 0x04;
const REQ_SET_AUTO: u8 = 0x06;

// `security` of a network.
const SEC_WPA2: u8 = 2;
const SEC_WPA2_WPA3: u8 = 4;

struct St {
    wifi: [u8; 4096],
    wifi_n: usize,
    net: [u8; 2048],
    net_n: usize,
    /// The network whose actions are open.
    sel: [u8; 32],
    sel_n: usize,
    pass: [u8; 63],
    pass_n: usize,
    reveal: bool,
    focus: u32,
    msg: &'static str,
    msg_err: bool,
    heap_base: usize,
}

impl St {
    fn sel(&self) -> Option<&[u8]> {
        (self.sel_n > 0).then(|| &self.sel[..self.sel_n])
    }
    fn set_sel(&mut self, s: Option<&[u8]>) {
        self.pass = [0; 63];
        self.pass_n = 0;
        self.reveal = false;
        match s {
            Some(s) if s.len() <= 32 => {
                self.sel[..s.len()].copy_from_slice(s);
                self.sel_n = s.len();
                self.focus = self.focus.wrapping_add(1).max(1);
            }
            _ => self.sel_n = 0,
        }
    }
    /// Read both sources again; true if anything changed.
    fn refresh(&mut self) -> bool {
        let mut w = [0u8; 4096];
        let wn = host::wifi_status(&mut w).unwrap_or(0).min(w.len());
        let mut n = [0u8; 2048];
        let nn = host::net_info(&mut n).unwrap_or(0).min(n.len());
        let changed = w[..wn] != self.wifi[..self.wifi_n] || n[..nn] != self.net[..self.net_n];
        self.wifi[..wn].copy_from_slice(&w[..wn]);
        self.wifi_n = wn;
        self.net[..nn].copy_from_slice(&n[..nn]);
        self.net_n = nn;
        changed
    }
}

// ── What the kernel and wifid report ─────────────────────────────────

struct Ap {
    rssi: i8,
    channel: u8,
    security: u8,
    known: bool,
    ssid: Vec<u8>,
}

#[derive(Default)]
struct Wifi {
    present: bool,
    state: String,
    scanning: bool,
    ssid: Option<Vec<u8>>,
    channel: u8,
    signal: i8,
    security: Option<u8>,
    error: String,
    aps: Vec<Ap>,
    /// (ssid, auto)
    known: Vec<(Vec<u8>, bool)>,
}

/// `\xNN` back to the byte; everything else as is.
fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && b.get(i + 1) == Some(&b'x') && i + 3 < b.len() {
            if let Some(v) = s.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// A name for the screen: UTF-8 as it is, any broken byte as `?`.
fn shown(ssid: &[u8]) -> String {
    match core::str::from_utf8(ssid) {
        Ok(s) => s.into(),
        Err(_) => ssid.iter().map(|&b| if b.is_ascii() { b as char } else { '?' }).collect(),
    }
}

fn parse_wifi(t: &[u8]) -> Wifi {
    let mut w = Wifi::default();
    let Ok(t) = core::str::from_utf8(t) else { return w };
    w.present = !t.is_empty();
    for line in t.lines() {
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        match k {
            "state" => {
                w.scanning = v.contains("(scanning)");
                w.state = v.split(' ').next().unwrap_or("").into();
            }
            "ssid" => w.ssid = Some(unescape(v)),
            "channel" => w.channel = v.parse().unwrap_or(0),
            "signal" => w.signal = v.parse().unwrap_or(0),
            "security" => w.security = v.parse().ok(),
            "error" => w.error = v.into(),
            "ap" => {
                let mut f = v.splitn(5, ' ');
                let (Some(r), Some(c), Some(s), Some(k), Some(n)) = (f.next(), f.next(), f.next(), f.next(), f.next()) else {
                    continue;
                };
                w.aps.push(Ap {
                    rssi: r.parse().unwrap_or(-100),
                    channel: c.parse().unwrap_or(0),
                    security: s.parse().unwrap_or(5),
                    known: k == "1",
                    ssid: unescape(n),
                });
            }
            "known" => {
                let mut f = v.splitn(3, ' ');
                let (Some(a), Some(_prio), Some(n)) = (f.next(), f.next(), f.next()) else { continue };
                w.known.push((unescape(n), a == "1"));
            }
            _ => {}
        }
    }
    w
}

struct Iface {
    name: String,
    driver: String,
    mac: String,
    up: bool,
    primary: bool,
}

#[derive(Default)]
struct Net {
    ifaces: Vec<Iface>,
    ipv4: String,
    gateway: String,
    dns: String,
}

fn parse_net(t: &[u8]) -> Net {
    let mut n = Net::default();
    let Ok(t) = core::str::from_utf8(t) else { return n };
    for line in t.lines() {
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        match k {
            "iface" => {
                let f: Vec<&str> = v.split(' ').collect();
                if f.len() >= 5 {
                    n.ifaces.push(Iface {
                        name: f[0].into(),
                        driver: f[1].replace('_', " "),
                        mac: f[2].into(),
                        up: f[3] == "1",
                        primary: f[4] == "1",
                    });
                }
            }
            "ipv4" => n.ipv4 = v.into(),
            "gateway" => n.gateway = v.into(),
            "dns" => n.dns = v.into(),
            _ => {}
        }
    }
    n
}

fn bars(rssi: i8) -> IconId {
    match rssi {
        r if r >= -60 => IconId::WifiHigh,
        r if r >= -70 => IconId::WifiMedium,
        r if r >= -80 => IconId::WifiLow,
        _ => IconId::WifiNone,
    }
}

fn security_name(s: u8) -> &'static str {
    match s {
        0 => "open",
        1 => "WEP",
        2 => "WPA2",
        3 => "WPA3",
        4 => "WPA2/WPA3",
        _ => "Enterprise",
    }
}

fn supported(s: u8) -> bool {
    matches!(s, SEC_WPA2 | SEC_WPA2_WPA3)
}

fn band(channel: u8) -> &'static str {
    if channel > 14 { "5 GHz" } else { "2.4 GHz" }
}

// ── Rendering ────────────────────────────────────────────────────────

fn caption(text: &str, tint: Token) -> Widget {
    Widget::Text { content: text.into(), style: TextStyle::Caption, modifiers: vec![Modifier::Tint(tint)] }
}

fn icon(id: IconId, size: u16, tint: Token) -> Widget {
    Widget::Icon { id, size, modifiers: vec![Modifier::Tint(tint)] }
}

fn row(children: Vec<Widget>, spacing: Spacing) -> Widget {
    Widget::Row { children, spacing: spacing.as_u16(), align: Align::Center, modifiers: vec![] }
}

fn btn(label: &str, style: ButtonStyle, action: u32) -> Widget {
    prefab::button(label, style, ActionId(action))
}

/// `LABEL   value`, the value in mono, selectable by eye.
fn detail(label: &str, value: &str, trailing: Option<Widget>) -> Widget {
    let mut children = vec![
        Widget::Text {
            content: label.into(),
            style: TextStyle::Caption,
            modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted), Modifier::MinWidth(72)],
        },
        Widget::Text {
            content: value.into(),
            style: TextStyle::Mono,
            modifiers: vec![Modifier::Tint(Token::OnSurface), Modifier::Flex(1)],
        },
    ];
    children.extend(trailing);
    row(children, Spacing::Sm)
}

/// The connection the traffic takes.
fn connection(w: &Wifi, n: &Net) -> Vec<Widget> {
    let primary = n.ifaces.iter().find(|i| i.primary);
    let wifi_up = w.state == "connected" && primary.is_none_or(|i| i.name == "wlan");
    let mut out = Vec::new();

    let (ic, title, sub): (IconId, String, String) = if wifi_up {
        let name = w.ssid.as_deref().map(shown).unwrap_or_default();
        let mut sub = String::new();
        if w.signal != 0 {
            sub.push_str(&alloc::format!("{} dBm · ", w.signal));
        }
        sub.push_str(&alloc::format!("{} · channel {}", band(w.channel), w.channel));
        if let Some(s) = w.security {
            sub.push_str(" · ");
            sub.push_str(security_name(s));
        }
        (if w.signal != 0 { bars(w.signal) } else { IconId::WifiHigh }, name, sub)
    } else if let Some(i) = primary.filter(|i| i.name != "wlan") {
        (IconId::Network, "Ethernet".into(),
         alloc::format!("{} · {}", i.driver, if i.up { "connected" } else { "cable unplugged" }))
    } else if matches!(w.state.as_str(), "connecting" | "reconnecting") {
        (IconId::WifiNone, w.ssid.as_deref().map(shown).unwrap_or_else(|| "Wi-Fi".into()), "Connecting…".into())
    } else {
        (IconId::WifiSlash, "Not connected".into(),
         if w.present { "Pick a network below.".into() } else { "No network device.".into() })
    };

    out.push(row(vec![
        icon(ic, 24, Token::OnSurface),
        Widget::Column {
            children: vec![
                Widget::Text { content: title, style: TextStyle::Heading, modifiers: vec![] },
                caption(&sub, Token::OnSurfaceMuted),
            ],
            spacing: 2,
            align: Align::Start,
            modifiers: vec![Modifier::Flex(1)],
        },
    ], Spacing::Md));

    if !n.ipv4.is_empty() && (wifi_up || primary.is_some_and(|i| i.name != "wlan" && i.up)) {
        out.push(detail("IP", &n.ipv4, Some(btn("Copy", ButtonStyle::Ghost, A_COPY_IP))));
        out.push(detail("Gateway", &n.gateway, None));
        out.push(detail("DNS", &n.dns, None));
    }
    if let Some(i) = primary.or(n.ifaces.first()) {
        out.push(detail("MAC", &i.mac, None));
    }
    if !w.error.is_empty() {
        out.push(caption(&w.error, Token::Danger));
    }

    // The joined WLAN's own actions.
    if wifi_up || matches!(w.state.as_str(), "connecting" | "reconnecting") {
        if let Some(ssid) = w.ssid.as_deref() {
            let auto = w.known.iter().find(|(s, _)| s == ssid).map(|(_, a)| *a);
            let mut buttons = vec![btn("Disconnect", ButtonStyle::Secondary, A_DISCONNECT)];
            if let Some(a) = auto {
                buttons.push(btn("Forget", ButtonStyle::Ghost, A_FORGET));
                buttons.push(Widget::Spacer { flex: 1 });
                buttons.push(chip("Auto-join", a, A_AUTO));
            }
            out.push(row(buttons, Spacing::Sm));
        }
    }
    out
}

/// An on/off chip, accent-tinted when on.
fn chip(text: &str, on: bool, action: u32) -> Widget {
    row(vec![
        icon(if on { IconId::Check } else { IconId::None }, 16, Token::Accent),
        caption(text, if on { Token::OnSurface } else { Token::OnSurfaceMuted }),
    ], Spacing::Xs)
    .with_mods(vec![
        Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Xs.as_u16() },
        Modifier::Rounded(Radius::Md.as_u8()),
        Modifier::Background(if on { Token::AccentMuted } else { Token::SurfaceMuted }),
        Modifier::OnClick(ActionId(action)),
        Modifier::Hover(vec![Modifier::Background(Token::SurfaceHover)]),
    ])
}

trait WithMods {
    fn with_mods(self, m: Vec<Modifier>) -> Widget;
}

impl WithMods for Widget {
    fn with_mods(self, m: Vec<Modifier>) -> Widget {
        match self {
            Widget::Row { children, spacing, align, .. } => Widget::Row { children, spacing, align, modifiers: m },
            other => other,
        }
    }
}

fn network_row(st: &St, w: &Wifi, i: usize, ap: &Ap) -> Widget {
    let current = w.ssid.as_deref() == Some(&ap.ssid[..]) && w.state == "connected";
    let selected = st.sel() == Some(&ap.ssid[..]);
    let mut children = vec![
        icon(bars(ap.rssi), 16, Token::OnSurface),
        Widget::Text {
            content: shown(&ap.ssid),
            style: TextStyle::Body,
            modifiers: vec![Modifier::Flex(1)],
        },
    ];
    if current {
        children.push(caption("Connected", Token::Accent));
    } else if ap.known {
        children.push(caption("Saved", Token::OnSurfaceMuted));
    }
    if ap.security != 0 {
        children.push(icon(IconId::Lock, 16, Token::OnSurfaceMuted));
    }
    let mut mods = vec![
        Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Sm.as_u16() },
        Modifier::Rounded(Radius::Md.as_u8()),
        Modifier::OnClick(ActionId(A_ROW_BASE + i as u32)),
        Modifier::Hover(vec![Modifier::Background(Token::SurfaceHover)]),
    ];
    if selected {
        mods.push(Modifier::Background(Token::SurfaceMuted));
    }
    let head = Widget::Row { children, spacing: Spacing::Sm.as_u16(), align: Align::Center, modifiers: mods };
    if !selected || current {
        return head;
    }

    // The actions of the open row.
    let body: Vec<Widget> = if !supported(ap.security) {
        vec![caption(
            alloc::format!("{} is not supported yet (WPA2 only).", security_name(ap.security)).as_str(),
            Token::OnSurfaceMuted,
        )]
    } else if ap.known {
        vec![row(vec![
            caption(&alloc::format!("{} · channel {}", band(ap.channel), ap.channel), Token::OnSurfaceMuted),
            Widget::Spacer { flex: 1 },
            btn("Connect", ButtonStyle::Primary, A_CONNECT),
        ], Spacing::Sm)]
    } else {
        let mut input_mods = vec![
            Modifier::NodeId(NodeId(NODE_PASS)),
            Modifier::Flex(1),
            Modifier::FocusRequest(st.focus),
        ];
        if !st.reveal {
            input_mods.push(Modifier::Mask);
        }
        vec![
            Widget::Row {
                children: vec![
                    Widget::Input {
                        value: core::str::from_utf8(&st.pass[..st.pass_n]).unwrap_or("").into(),
                        placeholder: "Password".into(),
                        on_submit: ActionId(A_CONNECT),
                        modifiers: input_mods,
                    },
                    btn(if st.reveal { "Hide" } else { "Show" }, ButtonStyle::Ghost, A_REVEAL),
                ],
                spacing: Spacing::Sm.as_u16(),
                align: Align::Center,
                modifiers: vec![
                    Modifier::Background(Token::Surface),
                    Modifier::Rounded(Radius::Md.as_u8()),
                    Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
                    Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Xs.as_u16() },
                    Modifier::Focus(vec![Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() }]),
                ],
            },
            row(vec![Widget::Spacer { flex: 1 }, btn("Connect", ButtonStyle::Primary, A_CONNECT)], Spacing::Sm),
        ]
    };
    Widget::Column {
        children: vec![head, Widget::Column {
            children: body,
            spacing: Spacing::Sm.as_u16(),
            align: Align::Stretch,
            modifiers: vec![Modifier::PaddingXY { x: Padding::Sm.as_u16(), y: Padding::Xs.as_u16() }],
        }],
        spacing: 0,
        align: Align::Stretch,
        modifiers: vec![Modifier::Background(Token::SurfaceMuted), Modifier::Rounded(Radius::Md.as_u8())],
    }
}

fn render(st: &St) -> Widget {
    let w = parse_wifi(&st.wifi[..st.wifi_n]);
    let n = parse_net(&st.net[..st.net_n]);
    let mut children = connection(&w, &n);

    if w.present && w.state != "no-driver" {
        children.push(Widget::Divider);
        children.push(row(vec![
            caption("WI-FI NETWORKS", Token::OnSurfaceMuted),
            Widget::Spacer { flex: 1 },
            caption(if w.scanning { "Searching…" } else { "" }, Token::OnSurfaceMuted),
            prefab::icon_button(IconId::ArrowClockwise, 16, Some(ActionId(A_SCAN)), None),
        ], Spacing::Sm));
        let mut rows: Vec<Widget> = w.aps.iter().enumerate().map(|(i, ap)| network_row(st, &w, i, ap)).collect();
        if rows.is_empty() {
            rows.push(caption(if w.scanning { "Searching…" } else { "No networks found." }, Token::OnSurfaceMuted));
        }
        children.push(Widget::Scroll {
            child: alloc::boxed::Box::new(Widget::Column {
                children: rows,
                spacing: 2,
                align: Align::Stretch,
                modifiers: vec![],
            }),
            axis: Axis::Vertical,
            modifiers: vec![Modifier::Flex(1)],
        });
    } else {
        children.push(Widget::Spacer { flex: 1 });
    }
    if !st.msg.is_empty() {
        children.push(caption(st.msg, if st.msg_err { Token::Danger } else { Token::OnSurfaceMuted }));
    }

    Widget::Column {
        children,
        spacing: Spacing::Md.as_u16(),
        align: Align::Stretch,
        modifiers: vec![
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Lg.as_u8() },
            Modifier::Rounded(Radius::Lg.as_u8()),
            Modifier::Padding(Padding::Md.as_u16()),
            Modifier::MinWidth(W as u16),
            Modifier::MaxWidth(W as u16),
            Modifier::MinHeight(H as u16),
            Modifier::MaxHeight(H as u16),
        ],
    }
}

fn commit_tree(st: &St) {
    ALLOCATOR.reset(st.heap_base);
    let tree = render(st);
    match wire::encode(&tree) {
        Ok(bytes) => { if !host::scene_commit(&bytes) { log("[link] commit failed"); } }
        Err(_) => log("[link] encode failed"),
    }
}

// ── Actions ──────────────────────────────────────────────────────────

fn ssid_msg(op: u8, ssid: &[u8]) -> Vec<u8> {
    let mut m = vec![op, ssid.len() as u8];
    m.extend_from_slice(ssid);
    m
}

fn ask(st: &mut St, msg: &[u8], ok: &'static str) {
    if host::wifi_request(msg) {
        st.msg = ok;
        st.msg_err = false;
    } else {
        st.msg = "The network service did not take the request.";
        st.msg_err = true;
    }
}

fn connect(st: &mut St, w: &Wifi) {
    let Some(sel) = st.sel().map(Vec::from) else { return };
    let known = w.aps.iter().any(|a| a.ssid == sel && a.known)
        || w.known.iter().any(|(s, _)| *s == sel);
    let mut m = ssid_msg(REQ_CONNECT, &sel);
    if !known {
        let p = &st.pass[..st.pass_n];
        if !(8..=63).contains(&p.len()) {
            st.msg = "A Wi-Fi password is 8 to 63 characters.";
            st.msg_err = true;
            st.focus = st.focus.wrapping_add(1).max(1);
            return;
        }
        m.push(p.len() as u8);
        m.extend_from_slice(p);
    }
    ask(st, &m, "Connecting…");
    m.fill(0);
    st.set_sel(None);
}

enum Outcome { Idle, Rerender, Exit }

fn handle(st: &mut St, ev: Event) -> Outcome {
    match ev {
        Event::Key(KeyCode::Escape) => Outcome::Exit,
        // The password field: kept here, not echoed per key (vault's rule).
        Event::InputChangeAt { node: NODE_PASS, value } => {
            let b = value.as_bytes();
            let n = b.len().min(st.pass.len());
            st.pass[..n].copy_from_slice(&b[..n]);
            st.pass_n = n;
            Outcome::Idle
        }
        Event::Action(ActionId(id)) => {
            let w = parse_wifi(&st.wifi[..st.wifi_n]);
            st.msg = "";
            match id {
                A_SCAN => ask(st, &[REQ_SCAN], "Searching…"),
                A_DISCONNECT => ask(st, &[REQ_DISCONNECT], "Disconnected — no automatic reconnect."),
                A_FORGET => {
                    if let Some(s) = w.ssid.as_deref() {
                        ask(st, &ssid_msg(REQ_FORGET, s), "Forgotten.");
                    }
                }
                A_AUTO => {
                    if let Some(s) = w.ssid.as_deref() {
                        let on = w.known.iter().find(|(k, _)| k == s).is_some_and(|(_, a)| !*a);
                        let mut m = ssid_msg(REQ_SET_AUTO, s);
                        m.push(on as u8);
                        ask(st, &m, if on { "Joins automatically." } else { "Joins only when asked." });
                    }
                }
                A_CONNECT => connect(st, &w),
                A_REVEAL => st.reveal = !st.reveal,
                A_COPY_IP => {
                    let n = parse_net(&st.net[..st.net_n]);
                    let ip = n.ipv4.split('/').next().unwrap_or("");
                    if !ip.is_empty() && host::clipboard_set(ip) {
                        st.msg = "IP address copied.";
                        st.msg_err = false;
                    }
                }
                id if id >= A_ROW_BASE => {
                    if let Some(ap) = w.aps.get((id - A_ROW_BASE) as usize) {
                        let again = st.sel() == Some(&ap.ssid[..]);
                        let s = ap.ssid.clone();
                        st.set_sel(if again { None } else { Some(&s) });
                    }
                }
                _ => {}
            }
            Outcome::Rerender
        }
        _ => Outcome::Idle,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut st = St {
        wifi: [0; 4096],
        wifi_n: 0,
        net: [0; 2048],
        net_n: 0,
        sel: [0; 32],
        sel_n: 0,
        pass: [0; 63],
        pass_n: 0,
        reveal: false,
        focus: 1,
        msg: "",
        msg_err: false,
        heap_base: ALLOCATOR.mark(),
    };
    st.refresh();
    // A list to show. While linked a scan runs in slices beside the link
    // (~10 s), so only when there is next to nothing; the button forces one.
    if parse_wifi(&st.wifi[..st.wifi_n]).aps.len() <= 1 {
        host::wifi_request(&[REQ_SCAN]);
    }
    // Placed by the kernel next to the icon deck was clicked on.
    set_overlay(W, H);
    set_light_dismiss(true);
    commit_tree(&st);

    let mut event_buf = [0u8; EVENT_BUF_SIZE];
    loop {
        let mut redraw = false;
        loop {
            match nopeek_widgets::events::poll(&mut event_buf) {
                nopeek_widgets::events::Poll::Event(ev) => match handle(&mut st, ev) {
                    Outcome::Idle => {}
                    Outcome::Rerender => redraw = true,
                    Outcome::Exit => {
                        host::close_widget();
                        return;
                    }
                },
                nopeek_widgets::events::Poll::Empty => break,
                nopeek_widgets::events::Poll::Gone => return,
            }
        }
        if st.refresh() {
            redraw = true;
        }
        if redraw {
            commit_tree(&st);
        }
        host::wait(WAIT_INPUT | WAIT_STATE, REFRESH_MS);
    }
}
