//! vault — a small password manager over the kernel keyvault
//! (docs/plan/KEYSTORE.md). UI after the agreed mockup: setup / unlock, a
//! sidebar + list + detail, and the add/edit forms.
//!
//! Layout rules learned from `loft`: the root Column has no fixed size (the
//! window sizes it) and uses Flex to fill; a `Scroll` needs `Modifier::Flex(1)`
//! or it collapses. Lists use `prefab::list_row`, the sidebar
//! `prefab::sidebar_section` / `nav_row`.
//!
//! No heap state (the bump allocator resets each frame): fields sit in fixed
//! buffers, entries are fetched from the kernel each frame. The compositor
//! reports only the focused input's value, not which field, so forms edit ONE
//! field at a time — the active field is a live input, the others are value
//! boxes that activate on click; Enter advances, the last field saves.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use nopeek_widgets::host;
use nopeek_widgets::prefab::{self, ButtonStyle, InputKind};
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::EXEC | caps::RENDER, 0];

#[global_allocator]
static ALLOCATOR: nopeek_widgets::bump::Bump<{ 512 * 1024 }> = nopeek_widgets::bump::Bump::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    host::log_serial("[vault] panic!");
    core::arch::wasm32::unreachable()
}

const EVENT_BUF_SIZE: usize = 1024;
const WAIT_INPUT: u32 = 1;
const SIDEBAR_W: u16 = 190;

// Actions.
const A_SUBMIT: u32 = 1;
const A_ADD: u32 = 2;
const A_ADD_WIFI: u32 = 3;
const A_LOCK: u32 = 4;
const A_CANCEL: u32 = 5;
const A_REVEAL: u32 = 6;
const A_EDIT: u32 = 7;
const A_DELETE: u32 = 8;
const A_SEC_USER: u32 = 10;
const A_SEC_WIFI: u32 = 11;
const A_FIELD_BASE: u32 = 100; // + field index: make that field active
const A_SEL_BASE: u32 = 1000; // + row index: select that entry

#[derive(PartialEq, Clone, Copy)]
enum View {
    Setup,
    Unlock,
    List,
    EditSecret,
    AddWifi,
}

/// A fixed-size text field — survives the per-frame allocator reset.
struct Field {
    b: [u8; 256],
    n: usize,
}

impl Field {
    const fn new() -> Self {
        Field { b: [0u8; 256], n: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
    fn set(&mut self, s: &str) {
        let k = s.len().min(self.b.len());
        self.b[..k].copy_from_slice(&s.as_bytes()[..k]);
        self.n = k;
    }
    fn clear(&mut self) {
        self.n = 0;
    }
    fn is_empty(&self) -> bool {
        self.n == 0
    }
}

const NFIELDS: usize = 5;

struct St {
    view: View,
    f: [Field; NFIELDS],
    search: Field,
    /// 0 = secrets (user tier), 1 = Wi-Fi (system tier).
    section: u8,
    /// Which field in the current form is the editable input.
    active: usize,
    /// FocusRequest counter — bumps so the active input takes the keyboard.
    focus: u32,
    reveal: bool,
    /// Selected entry index in the current section's list, or -1.
    sel: i32,
    edit_orig: Field,
    msg: &'static str,
    heap_base: usize,
}

impl St {
    fn active_field(&mut self) -> &mut Field {
        &mut self.f[self.active.min(NFIELDS - 1)]
    }
    fn goto(&mut self, v: View) {
        self.view = v;
        self.active = 0;
        self.focus = self.focus.wrapping_add(1);
        self.reveal = false;
        self.msg = "";
    }
    fn set_active(&mut self, i: usize) {
        self.active = i.min(NFIELDS - 1);
        self.focus = self.focus.wrapping_add(1);
    }
}

// ── Entry record (user "Login" entry): url, username, password, note ───────

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn encode_login(url: &str, user: &str, pass: &str, note: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, url);
    put_str(&mut out, user);
    put_str(&mut out, pass);
    put_str(&mut out, note);
    out
}

fn decode_login(b: &[u8]) -> (String, String, String, String) {
    let mut off = 0usize;
    let mut take = || -> String {
        if off + 2 > b.len() {
            return String::new();
        }
        let len = u16::from_le_bytes([b[off], b[off + 1]]) as usize;
        off += 2;
        if off + len > b.len() {
            return String::new();
        }
        let s = core::str::from_utf8(&b[off..off + len]).unwrap_or("").into();
        off += len;
        s
    };
    (take(), take(), take(), take())
}

fn fetch_login(name: &str) -> (String, String, String, String) {
    let mut buf = [0u8; 2048];
    match host::vault_user_get(name, &mut buf) {
        Some(n) => {
            let (url, user, pass, note) = decode_login(&buf[..n]);
            // Backward compat: an entry from the old simple UI holds a plain
            // string, not a record. If nothing decoded but there are bytes,
            // show them as the password.
            if n > 0 && url.is_empty() && user.is_empty() && pass.is_empty() && note.is_empty() {
                let p = core::str::from_utf8(&buf[..n]).unwrap_or("").into();
                (String::new(), String::new(), p, String::new())
            } else {
                (url, user, pass, note)
            }
        }
        None => (String::new(), String::new(), String::new(), String::new()),
    }
}

fn split_names(buf: &[u8], n: usize) -> Vec<String> {
    core::str::from_utf8(&buf[..n])
        .unwrap_or("")
        .split('\n')
        .filter(|s| !s.is_empty())
        .map(|s| s.into())
        .collect()
}

fn user_names() -> Vec<String> {
    let mut buf = [0u8; 4096];
    match host::vault_user_list(&mut buf) {
        Some(n) => split_names(&buf, n),
        None => Vec::new(),
    }
}

fn wifi_names() -> Vec<String> {
    let mut buf = [0u8; 4096];
    let raw = match host::vault_system_list(&mut buf) {
        Some(n) => split_names(&buf, n),
        None => Vec::new(),
    };
    raw.iter().map(|s| s.strip_prefix("net:").unwrap_or(s).into()).collect()
}

// ── Rendering helpers ──────────────────────────────────────────────────────

fn label(text: &str) -> Widget {
    Widget::Text {
        content: text.into(),
        style: TextStyle::Caption,
        modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted)],
    }
}

fn btn(lbl: &str, style: ButtonStyle, action: u32) -> Widget {
    prefab::button(lbl, style, ActionId(action))
}

fn box_mods() -> Vec<Modifier> {
    vec![
        Modifier::Background(Token::SurfaceElevated),
        Modifier::Rounded(Radius::Md.as_u8()),
        Modifier::PaddingXY { x: Padding::Md.as_u16(), y: Padding::Sm.as_u16() },
    ]
}

/// A form field: the active one is a live input, the others value boxes that
/// activate on click. `secret` masks the value unless revealed.
fn field(st: &St, idx: usize, placeholder: &str, secret: bool) -> Widget {
    let val = st.f[idx].as_str();
    if idx == st.active {
        let kind = if secret && !st.reveal { InputKind::Password } else { InputKind::Text };
        let mut mods = vec![Modifier::FocusRequest(st.focus), Modifier::Flex(1)];
        if matches!(kind, InputKind::Password) {
            mods.push(Modifier::Mask);
        }
        let input = Widget::Input {
            value: val.into(),
            placeholder: placeholder.into(),
            on_submit: ActionId(A_SUBMIT),
            modifiers: mods,
        };
        let mut m = box_mods();
        m.push(Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() });
        Widget::Row { children: vec![input], spacing: 0, align: Align::Center, modifiers: m }
    } else {
        let shown: String = if val.is_empty() {
            placeholder.into()
        } else if secret && !st.reveal {
            "•".repeat(val.chars().count().min(24))
        } else {
            val.into()
        };
        let tint = if val.is_empty() { Token::OnSurfaceFaint } else { Token::OnSurface };
        let mut m = box_mods();
        m.push(Modifier::OnClick(ActionId(A_FIELD_BASE + idx as u32)));
        Widget::Row {
            children: vec![Widget::Text {
                content: shown,
                style: TextStyle::Body,
                modifiers: vec![Modifier::Tint(tint), Modifier::Flex(1)],
            }],
            spacing: 0,
            align: Align::Center,
            modifiers: m,
        }
    }
}

fn labelled(st: &St, idx: usize, lab: &str, placeholder: &str, secret: bool) -> Widget {
    Widget::Column {
        children: vec![label(lab), field(st, idx, placeholder, secret)],
        spacing: 2,
        align: Align::Stretch,
        modifiers: vec![],
    }
}

fn button_row(buttons: Vec<Widget>) -> Widget {
    Widget::Row { children: buttons, spacing: Spacing::Sm.as_u16(), align: Align::Center, modifiers: vec![] }
}

/// A centered card screen (setup / unlock / forms) on a full-window surface.
fn screen_card(children: Vec<Widget>) -> Widget {
    let card = Widget::Column {
        children,
        spacing: Spacing::Md.as_u16(),
        align: Align::Stretch,
        modifiers: vec![
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Rounded(Radius::Lg.as_u8()),
            Modifier::Padding(Padding::Lg.as_u16()),
            Modifier::MinWidth(380),
            Modifier::MaxWidth(440),
        ],
    };
    Widget::Column {
        children: vec![Widget::Spacer { flex: 1 }, card, Widget::Spacer { flex: 1 }],
        spacing: 0,
        align: Align::Center,
        modifiers: vec![Modifier::Background(Token::Surface), Modifier::Padding(Padding::Lg.as_u16())],
    }
}

// ── Screens ────────────────────────────────────────────────────────────────

fn setup_screen(st: &St) -> Widget {
    screen_card(vec![
        Widget::Text { content: "Create your vault".into(), style: TextStyle::Heading, modifiers: vec![] },
        prefab::muted("One passphrase unlocks everything in here. There is no reset and no recovery."),
        labelled(st, 0, "MASTER PASSPHRASE", "", true),
        labelled(st, 1, "CONFIRM", "", true),
        button_row(vec![btn("Create vault", ButtonStyle::Primary, A_SUBMIT)]),
    ])
}

fn unlock_screen(st: &St) -> Widget {
    let users = user_names().len();
    let wifis = wifi_names().len();
    screen_card(vec![
        Widget::Row {
            children: vec![
                Widget::Icon { id: IconId::Lock, size: 24, modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                Widget::Text { content: "Vault is locked".into(), style: TextStyle::Heading, modifiers: vec![] },
            ],
            spacing: Spacing::Sm.as_u16(),
            align: Align::Center,
            modifiers: vec![],
        },
        prefab::muted(&alloc::format!("{} secrets · {} Wi-Fi", users, wifis)),
        labelled(st, 0, "MASTER PASSPHRASE", "", true),
        button_row(vec![
            btn(if st.reveal { "Hide" } else { "Show" }, ButtonStyle::Ghost, A_REVEAL),
            btn("Unlock", ButtonStyle::Primary, A_SUBMIT),
        ]),
    ])
}

fn detail_field(lab: &str, value: &str) -> Widget {
    Widget::Column {
        children: vec![
            label(lab),
            Widget::Row {
                children: vec![Widget::Text {
                    content: if value.is_empty() { "—".into() } else { value.into() },
                    style: TextStyle::Body,
                    modifiers: vec![Modifier::Tint(Token::OnSurface), Modifier::Flex(1)],
                }],
                spacing: 0,
                align: Align::Center,
                modifiers: box_mods(),
            },
        ],
        spacing: 2,
        align: Align::Stretch,
        modifiers: vec![],
    }
}

fn detail_pane(st: &St, users: &[String], wifis: &[String]) -> Widget {
    let on_user = st.section == 0;
    let names = if on_user { users } else { wifis };
    if st.sel < 0 || st.sel as usize >= names.len() {
        return Widget::Column {
            children: vec![prefab::muted("Select an entry")],
            spacing: 0,
            align: Align::Start,
            modifiers: vec![Modifier::Flex(1), Modifier::MinWidth(260), Modifier::Padding(Padding::Md.as_u16())],
        };
    }
    let name = &names[st.sel as usize];
    let children: Vec<Widget> = if on_user {
        let (url, user, pass, note) = fetch_login(name);
        let pass_shown = if st.reveal { pass.clone() } else { "•".repeat(pass.chars().count().min(24)) };
        vec![
            Widget::Text { content: name.clone(), style: TextStyle::Heading, modifiers: vec![] },
            detail_field("URL", &url),
            detail_field("USERNAME", &user),
            Widget::Column {
                children: vec![
                    label("PASSWORD"),
                    Widget::Row {
                        children: vec![
                            Widget::Text {
                                content: if pass.is_empty() { "—".into() } else { pass_shown },
                                style: TextStyle::Mono,
                                modifiers: vec![Modifier::Tint(Token::OnSurface), Modifier::Flex(1)],
                            },
                            btn(if st.reveal { "Hide" } else { "Show" }, ButtonStyle::Ghost, A_REVEAL),
                        ],
                        spacing: Spacing::Sm.as_u16(),
                        align: Align::Center,
                        modifiers: box_mods(),
                    },
                ],
                spacing: 2,
                align: Align::Stretch,
                modifiers: vec![],
            },
            detail_field("NOTE", &note),
            button_row(vec![
                btn("Edit", ButtonStyle::Secondary, A_EDIT),
                btn("Delete", ButtonStyle::Destructive, A_DELETE),
            ]),
        ]
    } else {
        vec![
            Widget::Row {
                children: vec![
                    Widget::Icon { id: IconId::WifiHigh, size: 20, modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                    Widget::Text { content: name.clone(), style: TextStyle::Heading, modifiers: vec![] },
                ],
                spacing: Spacing::Sm.as_u16(),
                align: Align::Center,
                modifiers: vec![],
            },
            prefab::muted("Wi-Fi password stored. Reveal comes with the WLAN integration."),
            button_row(vec![btn("Delete", ButtonStyle::Destructive, A_DELETE)]),
        ]
    };
    Widget::Column {
        children,
        spacing: Spacing::Md.as_u16(),
        align: Align::Stretch,
        modifiers: vec![Modifier::Flex(1), Modifier::MinWidth(260), Modifier::Padding(Padding::Md.as_u16())],
    }
}

fn list_screen(st: &St) -> Widget {
    let users = user_names();
    let wifis = wifi_names();
    let on_user = st.section == 0;

    // Toolbar: search + actions.
    let search = prefab::input(st.search.as_str(), "search", InputKind::Search, ActionId(A_SUBMIT), None);
    let toolbar = prefab::toolbar(vec![
        Widget::Row { children: vec![search], spacing: 0, align: Align::Center, modifiers: vec![Modifier::Flex(1)] },
        btn("+ Add", ButtonStyle::Primary, A_ADD),
        btn("+ Wi-Fi", ButtonStyle::Secondary, A_ADD_WIFI),
        btn("Lock", ButtonStyle::Ghost, A_LOCK),
    ]);

    // Sidebar.
    let sidebar = Widget::Column {
        children: vec![
            prefab::sidebar_section(
                "VAULT",
                vec![prefab::nav_row(
                    IconId::Lock,
                    &alloc::format!("Secrets  ({})", users.len()),
                    on_user,
                    Some(ActionId(A_SEC_USER)),
                    None,
                )],
            ),
            prefab::sidebar_section(
                "SYSTEM",
                vec![prefab::nav_row(
                    IconId::WifiHigh,
                    &alloc::format!("Wi-Fi  ({})", wifis.len()),
                    !on_user,
                    Some(ActionId(A_SEC_WIFI)),
                    None,
                )],
            ),
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Stretch,
        modifiers: vec![
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::MinWidth(SIDEBAR_W),
            Modifier::MaxWidth(SIDEBAR_W),
        ],
    };

    // Middle: the entry list.
    let filter = st.search.as_str().to_lowercase();
    let names: &[String] = if on_user { &users } else { &wifis };
    let mut rows: Vec<Widget> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
            continue;
        }
        let selected = st.sel == i as i32;
        let subtitle = if on_user { fetch_login(name).1 } else { String::new() };
        rows.push(prefab::list_row(
            if on_user { IconId::Lock } else { IconId::WifiHigh },
            name,
            &subtitle,
            selected,
            Some(ActionId(A_SEL_BASE + i as u32)),
            None,
        ));
    }
    if rows.is_empty() {
        rows.push(prefab::empty_state("No entries yet"));
    }
    let list = Widget::Column {
        children: vec![Widget::Scroll {
            child: alloc::boxed::Box::new(Widget::Column {
                children: rows,
                spacing: 2,
                align: Align::Stretch,
                modifiers: vec![],
            }),
            axis: Axis::Vertical,
            modifiers: vec![Modifier::Flex(1)],
        }],
        spacing: 0,
        align: Align::Stretch,
        modifiers: vec![Modifier::Flex(1), Modifier::MinWidth(240), Modifier::Padding(Padding::Sm.as_u16())],
    };

    let body = Widget::Row {
        children: vec![sidebar, list, Widget::Divider, detail_pane(st, &users, &wifis)],
        spacing: 0,
        align: Align::Stretch,
        modifiers: vec![Modifier::Flex(1)],
    };

    let count = if on_user { users.len() } else { wifis.len() };
    Widget::Column {
        children: vec![
            toolbar,
            Widget::Divider,
            body,
            Widget::Divider,
            prefab::footer(&alloc::format!("{} items · unlocked", count), "Enter show · Ctrl L lock"),
        ],
        spacing: Spacing::None.as_u16(),
        align: Align::Stretch,
        modifiers: vec![],
    }
}

fn edit_screen(st: &St) -> Widget {
    let title = if st.edit_orig.is_empty() { "New secret" } else { "Edit secret" };
    screen_card(vec![
        Widget::Text { content: title.into(), style: TextStyle::Heading, modifiers: vec![] },
        labelled(st, 0, "NAME", "github.com", false),
        labelled(st, 1, "URL", "https://…", false),
        labelled(st, 2, "USERNAME", "", false),
        labelled(st, 3, "PASSWORD", "", true),
        labelled(st, 4, "NOTE", "", false),
        button_row(vec![
            btn(if st.reveal { "Hide pw" } else { "Show pw" }, ButtonStyle::Ghost, A_REVEAL),
            btn("Cancel", ButtonStyle::Ghost, A_CANCEL),
            btn("Save", ButtonStyle::Primary, A_SUBMIT),
        ]),
    ])
}

fn wifi_screen(st: &St) -> Widget {
    screen_card(vec![
        Widget::Text { content: "New Wi-Fi network".into(), style: TextStyle::Heading, modifiers: vec![] },
        labelled(st, 0, "SSID", "", false),
        labelled(st, 1, "PASSWORD", "", true),
        button_row(vec![
            btn(if st.reveal { "Hide" } else { "Show" }, ButtonStyle::Ghost, A_REVEAL),
            btn("Cancel", ButtonStyle::Ghost, A_CANCEL),
            btn("Save", ButtonStyle::Primary, A_SUBMIT),
        ]),
    ])
}

fn render(st: &St) -> Widget {
    let body = match st.view {
        View::Setup => setup_screen(st),
        View::Unlock => unlock_screen(st),
        View::List => list_screen(st),
        View::EditSecret => edit_screen(st),
        View::AddWifi => wifi_screen(st),
    };
    if st.msg.is_empty() {
        body
    } else {
        Widget::Column {
            children: vec![body, prefab::muted(st.msg)],
            spacing: Spacing::Sm.as_u16(),
            align: Align::Stretch,
            modifiers: vec![Modifier::Background(Token::Surface)],
        }
    }
}

fn commit_tree(st: &St) {
    ALLOCATOR.reset(st.heap_base);
    let tree = render(st);
    match wire::encode(&tree) {
        Ok(bytes) => {
            if !host::scene_commit(&bytes) {
                host::log_serial("[vault] commit failed");
            }
        }
        Err(_) => host::log_serial("[vault] encode failed"),
    }
}

// ── Events ───────────────────────────────────────────────────────────────

fn start_view() -> View {
    if host::vault_user_present() {
        View::Unlock
    } else {
        View::Setup
    }
}

fn clear_form(st: &mut St) {
    for f in st.f.iter_mut() {
        f.clear();
    }
    st.edit_orig.clear();
}

fn open_edit(st: &mut St, name: &str) {
    clear_form(st);
    let (url, user, pass, note) = fetch_login(name);
    st.f[0].set(name);
    st.f[1].set(&url);
    st.f[2].set(&user);
    st.f[3].set(&pass);
    st.f[4].set(&note);
    st.edit_orig.set(name);
    st.goto(View::EditSecret);
}

fn edit_selected(st: &mut St) {
    if st.section != 0 {
        return;
    }
    let users = user_names();
    if st.sel >= 0 && (st.sel as usize) < users.len() {
        let name = users[st.sel as usize].clone();
        open_edit(st, &name);
    }
}

fn delete_selected(st: &mut St) {
    if st.section == 0 {
        let users = user_names();
        if st.sel >= 0 && (st.sel as usize) < users.len() {
            host::vault_user_delete(&users[st.sel as usize]);
        }
    } else {
        let wifis = wifi_names();
        if st.sel >= 0 && (st.sel as usize) < wifis.len() {
            let mut key = String::from("net:");
            key.push_str(&wifis[st.sel as usize]);
            host::vault_system_delete(&key);
        }
    }
    st.sel = -1;
    st.reveal = false;
}

fn save_secret(st: &mut St) {
    let name = st.f[0].as_str();
    if name.is_empty() {
        st.msg = "Name is required.";
        return;
    }
    let rec = encode_login(st.f[1].as_str(), st.f[2].as_str(), st.f[3].as_str(), st.f[4].as_str());
    let orig = st.edit_orig.as_str();
    if !orig.is_empty() && orig != name {
        host::vault_user_delete(orig);
    }
    host::vault_user_set(name, &rec);
    clear_form(st);
    st.sel = -1;
    st.section = 0;
    st.goto(View::List);
}

fn save_wifi(st: &mut St) {
    let ssid = st.f[0].as_str();
    if ssid.is_empty() {
        st.msg = "SSID is required.";
        return;
    }
    let mut key = String::from("net:");
    key.push_str(ssid);
    host::vault_system_set(&key, st.f[1].as_str().as_bytes());
    clear_form(st);
    st.sel = -1;
    st.section = 1;
    st.goto(View::List);
}

fn on_submit(st: &mut St) -> Outcome {
    match st.view {
        View::Setup => {
            if st.f[0].as_str().is_empty() {
                st.msg = "Choose a passphrase.";
            } else if st.f[0].as_str() != st.f[1].as_str() {
                st.msg = "Passphrases do not match.";
                st.f[1].clear();
                st.set_active(1);
            } else if host::vault_set_passphrase("", st.f[0].as_str()) {
                clear_form(st);
                st.sel = -1;
                st.goto(View::List);
            } else {
                st.msg = "Could not create the vault.";
            }
        }
        View::Unlock => {
            if host::vault_unlock(st.f[0].as_str()) {
                clear_form(st);
                st.sel = -1;
                st.goto(View::List);
            } else {
                st.f[0].clear();
                st.msg = "Wrong passphrase.";
                st.set_active(0);
            }
        }
        View::EditSecret => {
            if st.active < 4 {
                st.set_active(st.active + 1);
            } else {
                save_secret(st);
            }
        }
        View::AddWifi => {
            if st.active < 1 {
                st.set_active(st.active + 1);
            } else {
                save_wifi(st);
            }
        }
        View::List => {
            // Enter on the list reveals the selected password (footer: "Enter show").
            st.reveal = !st.reveal;
            st.focus = st.focus.wrapping_add(1);
        }
    }
    Outcome::Rerender
}

fn handle(st: &mut St, ev: Event) -> Outcome {
    match ev {
        Event::InputChange { value } => {
            if st.view == View::List {
                st.search.set(&value);
            } else {
                st.active_field().set(&value);
            }
            Outcome::Idle
        }
        Event::Action(ActionId(id)) => {
            match id {
                A_SUBMIT => return on_submit(st),
                A_REVEAL => {
                    st.reveal = !st.reveal;
                    st.focus = st.focus.wrapping_add(1);
                }
                A_ADD => {
                    clear_form(st);
                    st.goto(View::EditSecret);
                }
                A_ADD_WIFI => {
                    clear_form(st);
                    st.goto(View::AddWifi);
                }
                A_LOCK => {
                    host::vault_lock();
                    st.sel = -1;
                    st.goto(View::Unlock);
                }
                A_CANCEL => {
                    clear_form(st);
                    st.goto(View::List);
                }
                A_SEC_USER => {
                    st.section = 0;
                    st.sel = -1;
                    st.reveal = false;
                }
                A_SEC_WIFI => {
                    st.section = 1;
                    st.sel = -1;
                    st.reveal = false;
                }
                A_EDIT => edit_selected(st),
                A_DELETE => delete_selected(st),
                id if id >= A_SEL_BASE => {
                    st.sel = (id - A_SEL_BASE) as i32;
                    st.reveal = false;
                }
                id if id >= A_FIELD_BASE => {
                    st.set_active((id - A_FIELD_BASE) as usize);
                }
                _ => {}
            }
            Outcome::Rerender
        }
        // Ctrl chords (Ctrl is implied) that no focused text widget consumed.
        Event::Chord { letter, .. } => match letter {
            b'l' => {
                host::vault_lock();
                st.sel = -1;
                st.goto(View::Unlock);
                Outcome::Rerender
            }
            b'n' if st.view == View::List => {
                clear_form(st);
                st.goto(View::EditSecret);
                Outcome::Rerender
            }
            _ => Outcome::Idle,
        },
        Event::Key(KeyCode::Escape) => match st.view {
            View::List | View::Unlock | View::Setup => Outcome::Exit,
            _ => {
                clear_form(st);
                st.goto(View::List);
                Outcome::Rerender
            }
        },
        // List navigation and actions (when no text field has the keyboard).
        Event::Key(k) if st.view == View::List => {
            let n = if st.section == 0 { user_names().len() } else { wifi_names().len() } as i32;
            match k {
                KeyCode::Enter => {
                    st.reveal = !st.reveal;
                    st.focus = st.focus.wrapping_add(1);
                }
                KeyCode::Down if n > 0 => {
                    st.sel = (st.sel + 1).clamp(0, n - 1);
                    st.reveal = false;
                }
                KeyCode::Up if st.sel > 0 => {
                    st.sel -= 1;
                    st.reveal = false;
                }
                KeyCode::Delete => delete_selected(st),
                KeyCode::F(2) => edit_selected(st),
                _ => return Outcome::Idle,
            }
            Outcome::Rerender
        }
        _ => Outcome::Idle,
    }
}

enum Outcome {
    Idle,
    Rerender,
    Exit,
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut st = St {
        view: start_view(),
        f: [Field::new(), Field::new(), Field::new(), Field::new(), Field::new()],
        search: Field::new(),
        section: 0,
        active: 0,
        focus: 1,
        reveal: false,
        sel: -1,
        edit_orig: Field::new(),
        msg: "",
        heap_base: ALLOCATOR.mark(),
    };
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
        if redraw {
            commit_tree(&st);
        }
        host::wait(WAIT_INPUT, -1);
    }
}
