//! vault — a small password manager over the kernel keyvault
//! (docs/plan/KEYSTORE.md). UI after the agreed mockup: setup / unlock, a
//! sidebar + list + detail, and the add/edit forms.
//!
//! Layout follows `loft`: the root Column has no fixed size (the window
//! sizes it) and fills with Flex; a `Scroll` needs `Modifier::Flex(1)` or it
//! collapses.
//!
//! Input model (see the compositor in kernel/src/shade/widgets/mod.rs):
//! - Every form field is a real `Widget::Input` tagged with its own NodeId,
//!   so `Event::InputChangeAt { node, value }` says which field changed.
//! - Form fields are not re-committed per keystroke: the compositor keeps
//!   the buffer and writes it back into its tree when focus leaves a field.
//!   Echoing every key would race fast typing (a stale echo resets the
//!   buffer). Only the search box re-commits per key, because the list
//!   must filter as you type.
//! - `Modifier::FocusRequest` focuses a field with its whole text selected,
//!   so it is raised only when a screen opens or a field must be fixed,
//!   never on show/hide.
//!
//! No heap state (the bump allocator resets each frame): fields sit in fixed
//! buffers, entries are fetched from the kernel each frame.

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

const EVENT_BUF_SIZE: usize = 2048;
const WAIT_INPUT: u32 = 1;
const SIDEBAR_W: u16 = 190;
/// How long a copied password stays on the clipboard.
const CLIP_CLEAR_MS: u64 = 30_000;
/// Shortest master passphrase accepted at setup.
const MIN_PASSPHRASE: usize = 12;

// Password generator: length range and the character classes. Quotes,
// backslash and space are left out of the symbols, they break too many
// login forms and config files.
const GEN_MIN: u32 = 12;
const GEN_MAX: u32 = 64;
const GEN_CLASSES: [(&str, &[u8]); 4] = [
    ("A-Z", b"ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
    ("a-z", b"abcdefghijklmnopqrstuvwxyz"),
    ("0-9", b"0123456789"),
    ("!#$", b"!#$%&()*+,-./:;<=>?@[]^_{|}~"),
];

// Actions.
const A_SAVE: u32 = 1;
const A_ADD: u32 = 2;
const A_ADD_WIFI: u32 = 3;
const A_LOCK: u32 = 4;
const A_CANCEL: u32 = 5;
const A_REVEAL: u32 = 6;
const A_EDIT: u32 = 7;
const A_DELETE: u32 = 8;
const A_SEC_USER: u32 = 10;
const A_SEC_WIFI: u32 = 11;
const A_SEARCH_ENTER: u32 = 20;
const A_COPY_PASS: u32 = 21;
const A_COPY_USER: u32 = 22;
const A_MENU_VAULT: u32 = 30;
const A_MENU_EDIT: u32 = 31;
const A_MENU_DISMISS: u32 = 32;
const A_CLOSE: u32 = 33;
const A_FIND: u32 = 34;
const A_GEN: u32 = 40;
const A_GEN_LEN: u32 = 41;
const A_GEN_CLASS_BASE: u32 = 50; // + class index: toggle that class
const A_ENTER_BASE: u32 = 300; // + field index: Enter pressed in that field
const A_SEL_BASE: u32 = 1000; // + row index: select that entry

/// NodeId of form field `i` is `NODE_FIELD + i`.
const NODE_FIELD: u32 = 100;
/// Menu-bar labels, the anchors of their dropdowns.
const NODE_MENU_VAULT: u32 = 200;
const NODE_MENU_EDIT: u32 = 201;
/// `focus_field` value that requests focus for the search box.
const FOCUS_SEARCH: usize = usize::MAX;

#[derive(PartialEq, Clone, Copy)]
enum Menu {
    Vault,
    Edit,
}

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
    b: [u8; 1024],
    n: usize,
}

impl Field {
    const fn new() -> Self {
        Field { b: [0u8; 1024], n: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
    /// Store `s`, cut at a char boundary if it does not fit (a cut through a
    /// UTF-8 sequence would make the whole field read as empty).
    fn set(&mut self, s: &str) {
        let mut k = s.len().min(self.b.len());
        while k > 0 && !s.is_char_boundary(k) {
            k -= 1;
        }
        self.b[..k].copy_from_slice(&s.as_bytes()[..k]);
        self.n = k;
    }
    fn clear(&mut self) {
        self.b.fill(0);
        self.n = 0;
    }
    fn is_empty(&self) -> bool {
        self.n == 0
    }
}

const NFIELDS: usize = 5;

struct St {
    view: View,
    /// Form fields, by screen: setup 0 passphrase / 1 confirm; unlock 0;
    /// secret 0 name / 1 url / 2 username / 3 password / 4 note;
    /// Wi-Fi 0 SSID / 1 password.
    f: [Field; NFIELDS],
    search: Field,
    /// 0 = secrets (user tier), 1 = Wi-Fi (system tier).
    section: u8,
    /// FocusRequest number; bumped to move the keyboard to `focus_field`.
    focus: u32,
    focus_field: usize,
    reveal: bool,
    /// Selected index into the current section's full name list, or -1.
    sel: i32,
    /// Name of the entry being edited; empty for a new one.
    edit_orig: Field,
    /// Delete was pressed once; the next Delete removes the entry.
    confirm_del: bool,
    msg: &'static str,
    msg_err: bool,
    /// The open menu-bar dropdown.
    menu: Option<Menu>,
    /// Generator options; the panel shows once Generate was pressed.
    gen_open: bool,
    gen_len: u32,
    /// Bit i set = class `GEN_CLASSES[i]` is used.
    gen_classes: u8,
    /// FNV hash of a password we put on the clipboard (0 = none), and when
    /// to clear it again.
    clip_hash: u64,
    clip_deadline: u64,
    heap_base: usize,
}

impl St {
    fn goto(&mut self, v: View) {
        self.view = v;
        self.reveal = false;
        self.confirm_del = false;
        self.gen_open = false;
        self.focus_on(0);
    }
    /// Move the keyboard to `field` (whole text selected).
    fn focus_on(&mut self, field: usize) {
        self.focus_field = field;
        self.focus = self.focus.wrapping_add(1).max(1);
    }
    fn info(&mut self, m: &'static str) {
        self.msg = m;
        self.msg_err = false;
    }
    fn error(&mut self, m: &'static str) {
        self.msg = m;
        self.msg_err = true;
    }
}

// ── Entry record (user "Login" entry) ──────────────────────────────────────
//
// v1: tag 0x01, then url, username, password, note — each u16 length +
// UTF-8. Two older shapes may still be on disk and are read, never written:
// the same fields without the tag (UI 0.2–0.4), and a bare password string
// (the first simple UI). A typed password never starts with 0x01: the
// compositor does not insert control bytes.

const RECORD_V1: u8 = 0x01;

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(u16::MAX as usize)];
    out.extend_from_slice(&(b.len() as u16).to_le_bytes());
    out.extend_from_slice(b);
}

fn encode_login(url: &str, user: &str, pass: &str, note: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + url.len() + user.len() + pass.len() + note.len());
    out.push(RECORD_V1);
    put_str(&mut out, url);
    put_str(&mut out, user);
    put_str(&mut out, pass);
    put_str(&mut out, note);
    out
}

/// Exactly four length-prefixed fields filling `b`, or None.
fn decode_fields(b: &[u8]) -> Option<[String; 4]> {
    let mut off = 0usize;
    let mut out: [String; 4] = [String::new(), String::new(), String::new(), String::new()];
    for slot in out.iter_mut() {
        if off + 2 > b.len() {
            return None;
        }
        let len = u16::from_le_bytes([b[off], b[off + 1]]) as usize;
        off += 2;
        if off + len > b.len() {
            return None;
        }
        *slot = core::str::from_utf8(&b[off..off + len]).ok()?.into();
        off += len;
    }
    (off == b.len()).then_some(out)
}

struct Login {
    url: String,
    user: String,
    pass: String,
    note: String,
}

fn decode_login(b: &[u8]) -> Login {
    let fields = if b.first() == Some(&RECORD_V1) {
        decode_fields(&b[1..])
    } else {
        decode_fields(b)
    };
    match fields {
        Some([url, user, pass, note]) => Login { url, user, pass, note },
        None => Login {
            url: String::new(),
            user: String::new(),
            pass: core::str::from_utf8(b).unwrap_or("").into(),
            note: String::new(),
        },
    }
}

fn fetch_login(name: &str) -> Login {
    let mut buf = [0u8; 8192];
    match host::vault_user_get(name, &mut buf) {
        Some(n) if n <= buf.len() => decode_login(&buf[..n]),
        _ => Login { url: String::new(), user: String::new(), pass: String::new(), note: String::new() },
    }
}

fn split_names(buf: &[u8], n: usize) -> Vec<String> {
    core::str::from_utf8(&buf[..n.min(buf.len())])
        .unwrap_or("")
        .split('\n')
        .filter(|s| !s.is_empty())
        .map(|s| s.into())
        .collect()
}

fn user_names() -> Vec<String> {
    let mut buf = [0u8; 8192];
    match host::vault_user_list(&mut buf) {
        Some(n) => split_names(&buf, n),
        None => Vec::new(),
    }
}

fn wifi_names() -> Vec<String> {
    let mut buf = [0u8; 8192];
    let raw = match host::vault_system_list(&mut buf) {
        Some(n) => split_names(&buf, n),
        None => Vec::new(),
    };
    raw.iter().filter_map(|s| s.strip_prefix("net:")).map(|s| s.into()).collect()
}

fn section_names(st: &St) -> Vec<String> {
    if st.section == 0 { user_names() } else { wifi_names() }
}

/// Indices into `names` that match the search box.
fn visible(st: &St, names: &[String]) -> Vec<usize> {
    let filter = st.search.as_str().to_lowercase();
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| filter.is_empty() || n.to_lowercase().contains(&filter))
        .map(|(i, _)| i)
        .collect()
}

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h | 1
}

// ── Password generator ─────────────────────────────────────────────────────

/// A uniform index below `n` from the CSPRNG (rejection sampling, so no
/// character is more likely than another).
fn random_below(n: usize, pool: &mut [u8; 64], at: &mut usize) -> Option<usize> {
    let limit = 256 - 256 % n;
    loop {
        if *at == pool.len() {
            if !host::random_bytes(pool) {
                return None;
            }
            *at = 0;
        }
        let b = pool[*at] as usize;
        *at += 1;
        if b < limit {
            return Some(b % n);
        }
    }
}

/// A password of `len` characters from the selected classes, containing
/// at least one of each. None without a real entropy source.
fn generate(len: u32, classes: u8) -> Option<String> {
    let mut set: Vec<u8> = Vec::new();
    for (i, (_, chars)) in GEN_CLASSES.iter().enumerate() {
        if classes & (1 << i) != 0 {
            set.extend_from_slice(chars);
        }
    }
    if set.is_empty() {
        return None;
    }
    let mut pool = [0u8; 64];
    let mut at = pool.len();
    // Redraw until every class is present; at 8+ characters a redraw is
    // rare, and redrawing keeps the result uniform over the valid ones.
    loop {
        let mut out = String::with_capacity(len as usize);
        for _ in 0..len {
            out.push(set[random_below(set.len(), &mut pool, &mut at)?] as char);
        }
        let complete = GEN_CLASSES
            .iter()
            .enumerate()
            .all(|(i, (_, chars))| classes & (1 << i) == 0 || out.bytes().any(|b| chars.contains(&b)));
        if complete {
            return Some(out);
        }
    }
}

/// log2(n) in 1/256 bits.
fn log2_q8(n: u32) -> u32 {
    if n < 2 {
        return 0;
    }
    let int = 31 - n.leading_zeros();
    // Mantissa in [1, 2) as Q16; each squaring yields one fraction bit.
    let mut x: u64 = ((n as u64) << 16) >> int;
    let mut frac = 0u32;
    for bit in (0..8).rev() {
        x = (x * x) >> 16;
        if x >= 2 << 16 {
            x >>= 1;
            frac |= 1 << bit;
        }
    }
    (int << 8) | frac
}

fn gen_bits(st: &St) -> u32 {
    let n: usize = GEN_CLASSES
        .iter()
        .enumerate()
        .filter(|(i, _)| st.gen_classes & (1 << i) != 0)
        .map(|(_, (_, c))| c.len())
        .sum();
    st.gen_len * log2_q8(n as u32) / 256
}

fn fill_generated(st: &mut St) {
    match generate(st.gen_len, st.gen_classes) {
        Some(p) => {
            st.f[3].set(&p);
            st.reveal = true;
            st.info("");
        }
        None if st.gen_classes == 0 => st.error("Pick at least one character class."),
        None => st.error("No random source available."),
    }
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

fn field_chrome() -> Vec<Modifier> {
    vec![
        Modifier::Background(Token::Surface),
        Modifier::Rounded(Radius::Md.as_u8()),
        Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
        Modifier::PaddingXY { x: Padding::Md.as_u16(), y: Padding::Sm.as_u16() },
        Modifier::Focus(vec![Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() }]),
    ]
}

/// A real form input, tagged with its NodeId so `InputChangeAt` names it.
fn input_field(st: &St, idx: usize, placeholder: &str, secret: bool, extra: Vec<Widget>) -> Widget {
    let mut mods = vec![Modifier::NodeId(NodeId(NODE_FIELD + idx as u32)), Modifier::Flex(1)];
    if st.focus_field == idx {
        mods.push(Modifier::FocusRequest(st.focus));
    }
    if secret && !st.reveal {
        mods.push(Modifier::Mask);
    }
    let mut children = vec![Widget::Input {
        value: st.f[idx].as_str().into(),
        placeholder: placeholder.into(),
        on_submit: ActionId(A_ENTER_BASE + idx as u32),
        modifiers: mods,
    }];
    if secret {
        children.push(btn(if st.reveal { "Hide" } else { "Show" }, ButtonStyle::Ghost, A_REVEAL));
    }
    children.extend(extra);
    Widget::Row { children, spacing: Spacing::Sm.as_u16(), align: Align::Center, modifiers: field_chrome() }
}

fn labelled(st: &St, idx: usize, lab: &str, placeholder: &str, secret: bool) -> Widget {
    Widget::Column {
        children: vec![label(lab), input_field(st, idx, placeholder, secret, vec![])],
        spacing: 4,
        align: Align::Stretch,
        modifiers: vec![],
    }
}

/// An on/off chip: accent-tinted when on.
fn chip(text: &str, on: bool, action: u32) -> Widget {
    Widget::Text {
        content: text.into(),
        style: TextStyle::Mono,
        modifiers: vec![
            Modifier::PaddingXY { x: Padding::Md.as_u16(), y: Padding::Xs.as_u16() },
            Modifier::Rounded(Radius::Md.as_u8()),
            Modifier::Background(if on { Token::AccentMuted } else { Token::SurfaceMuted }),
            Modifier::Tint(if on { Token::OnSurface } else { Token::OnSurfaceFaint }),
            Modifier::OnClick(ActionId(action)),
            Modifier::Hover(vec![Modifier::Background(Token::SurfaceHover)]),
        ],
    }
}

fn generator_panel(st: &St) -> Widget {
    let slider = Widget::Slider {
        value: ((st.gen_len - GEN_MIN) * SLIDER_MAX as u32 / (GEN_MAX - GEN_MIN)) as u16,
        on_change: ActionId(A_GEN_LEN),
        modifiers: vec![Modifier::Flex(1)],
    };
    let mut chips: Vec<Widget> = GEN_CLASSES
        .iter()
        .enumerate()
        .map(|(i, (name, _))| chip(name, st.gen_classes & (1 << i) != 0, A_GEN_CLASS_BASE + i as u32))
        .collect();
    chips.push(Widget::Spacer { flex: 1 });
    chips.push(label(&alloc::format!("≈ {} bits", gen_bits(st))));
    Widget::Column {
        children: vec![
            Widget::Row {
                children: vec![
                    Widget::Text {
                        content: alloc::format!("Length {}", st.gen_len),
                        style: TextStyle::Caption,
                        modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted), Modifier::MinWidth(72)],
                    },
                    slider,
                ],
                spacing: Spacing::Sm.as_u16(),
                align: Align::Center,
                modifiers: vec![],
            },
            Widget::Row { children: chips, spacing: Spacing::Xs.as_u16(), align: Align::Center, modifiers: vec![] },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Stretch,
        modifiers: vec![
            Modifier::Background(Token::Surface),
            Modifier::Rounded(Radius::Md.as_u8()),
            Modifier::Padding(Padding::Sm.as_u16()),
        ],
    }
}

fn button_row(buttons: Vec<Widget>) -> Widget {
    Widget::Row { children: buttons, spacing: Spacing::Sm.as_u16(), align: Align::Center, modifiers: vec![] }
}

fn heading(text: &str) -> Widget {
    Widget::Text { content: text.into(), style: TextStyle::Heading, modifiers: vec![] }
}

fn message(st: &St) -> Option<Widget> {
    if st.msg.is_empty() {
        return None;
    }
    Some(Widget::Text {
        content: st.msg.into(),
        style: TextStyle::Caption,
        modifiers: vec![Modifier::Tint(if st.msg_err { Token::Danger } else { Token::OnSurfaceMuted })],
    })
}

/// A centered card screen (setup / unlock / forms).
fn screen_card(st: &St, mut children: Vec<Widget>) -> Widget {
    if let Some(m) = message(st) {
        children.push(m);
    }
    let card = Widget::Column {
        children,
        spacing: Spacing::Lg.as_u16(),
        align: Align::Stretch,
        modifiers: vec![
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Rounded(Radius::Lg.as_u8()),
            Modifier::Padding(Padding::Lg.as_u16()),
            Modifier::MinWidth(400),
            Modifier::MaxWidth(460),
        ],
    };
    Widget::Column {
        children: vec![Widget::Spacer { flex: 1 }, card, Widget::Spacer { flex: 1 }],
        spacing: 0,
        align: Align::Center,
        modifiers: vec![Modifier::Flex(1), Modifier::Padding(Padding::Lg.as_u16())],
    }
}

// ── Screens ────────────────────────────────────────────────────────────────

fn setup_screen(st: &St) -> Widget {
    screen_card(
        st,
        vec![
            heading("Create your vault"),
            prefab::muted("One passphrase unlocks everything in here. There is no reset and no recovery."),
            labelled(st, 0, "MASTER PASSPHRASE", "at least 12 characters", true),
            labelled(st, 1, "CONFIRM", "", true),
            button_row(vec![btn("Create vault", ButtonStyle::Primary, A_SAVE)]),
        ],
    )
}

fn unlock_screen(st: &St) -> Widget {
    let wifis = wifi_names().len();
    let sub = if wifis > 0 {
        alloc::format!("Enter your master passphrase · {} Wi-Fi network(s) stored", wifis)
    } else {
        "Enter your master passphrase.".into()
    };
    screen_card(
        st,
        vec![
            Widget::Row {
                children: vec![
                    Widget::Icon { id: IconId::Lock, size: 24, modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                    heading("Vault is locked"),
                ],
                spacing: Spacing::Sm.as_u16(),
                align: Align::Center,
                modifiers: vec![],
            },
            prefab::muted(&sub),
            labelled(st, 0, "MASTER PASSPHRASE", "", true),
            button_row(vec![btn("Unlock", ButtonStyle::Primary, A_SAVE)]),
        ],
    )
}

fn edit_screen(st: &St) -> Widget {
    let title = if st.edit_orig.is_empty() { "New secret" } else { "Edit secret" };
    screen_card(
        st,
        vec![
            heading(title),
            labelled(st, 0, "NAME", "e.g. github.com", false),
            labelled(st, 1, "URL", "https://…", false),
            labelled(st, 2, "USERNAME", "", false),
            password_with_generator(st),
            labelled(st, 4, "NOTE", "", false),
            button_row(vec![
                btn("Cancel", ButtonStyle::Ghost, A_CANCEL),
                btn("Save", ButtonStyle::Primary, A_SAVE),
            ]),
        ],
    )
}

fn password_with_generator(st: &St) -> Widget {
    let generate = btn(if st.gen_open { "New" } else { "Generate" }, ButtonStyle::Ghost, A_GEN);
    let mut children = vec![label("PASSWORD"), input_field(st, 3, "", true, vec![generate])];
    if st.gen_open {
        children.push(generator_panel(st));
    }
    Widget::Column { children, spacing: 4, align: Align::Stretch, modifiers: vec![] }
}

fn wifi_screen(st: &St) -> Widget {
    screen_card(
        st,
        vec![
            heading("New Wi-Fi network"),
            labelled(st, 0, "SSID", "network name", false),
            labelled(st, 1, "PASSWORD", "", true),
            button_row(vec![
                btn("Cancel", ButtonStyle::Ghost, A_CANCEL),
                btn("Save", ButtonStyle::Primary, A_SAVE),
            ]),
        ],
    )
}

fn value_box(content: String, mono: bool, trailing: Vec<Widget>) -> Widget {
    let mut children = vec![Widget::Text {
        content,
        style: if mono { TextStyle::Mono } else { TextStyle::Body },
        modifiers: vec![Modifier::Tint(Token::OnSurface), Modifier::Flex(1)],
    }];
    children.extend(trailing);
    Widget::Row {
        children,
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: vec![
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Rounded(Radius::Md.as_u8()),
            Modifier::PaddingXY { x: Padding::Md.as_u16(), y: Padding::Sm.as_u16() },
        ],
    }
}

/// Characters per line of a shown password: long ones wrap in mono
/// columns instead of being cut, so every character stays readable.
const PASS_LINE: usize = 24;

fn password_box(shown: &str, trailing: Vec<Widget>) -> Widget {
    let chars: Vec<char> = shown.chars().collect();
    let lines: Vec<Widget> = chars
        .chunks(PASS_LINE)
        .map(|c| Widget::Text {
            content: c.iter().collect(),
            style: TextStyle::Mono,
            modifiers: vec![Modifier::Tint(Token::OnSurface)],
        })
        .collect();
    let mut children = vec![Widget::Column {
        children: lines,
        spacing: 2,
        align: Align::Start,
        modifiers: vec![Modifier::Flex(1)],
    }];
    children.extend(trailing);
    Widget::Row {
        children,
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: vec![
            Modifier::Background(Token::SurfaceElevated),
            Modifier::Rounded(Radius::Md.as_u8()),
            Modifier::PaddingXY { x: Padding::Md.as_u16(), y: Padding::Sm.as_u16() },
        ],
    }
}

fn detail_row(lab: &str, value: &str, trailing: Vec<Widget>) -> Widget {
    let shown = if value.is_empty() { "—".into() } else { value.into() };
    Widget::Column {
        children: vec![label(lab), value_box(shown, false, trailing)],
        spacing: 4,
        align: Align::Stretch,
        modifiers: vec![],
    }
}

fn delete_button(st: &St) -> Widget {
    btn(if st.confirm_del { "Confirm delete" } else { "Delete" }, ButtonStyle::Destructive, A_DELETE)
}

fn detail_pane(st: &St, names: &[String], shown: &[usize]) -> Widget {
    let pane_mods = vec![Modifier::Flex(1), Modifier::MinWidth(280), Modifier::Padding(Padding::Md.as_u16())];
    let sel = st.sel;
    if sel < 0 || !shown.contains(&(sel as usize)) {
        let mut children = vec![prefab::muted("Select an entry")];
        children.extend(message(st));
        return Widget::Column {
            children,
            spacing: 0,
            align: Align::Start,
            modifiers: pane_mods,
        };
    }
    let name = &names[sel as usize];
    let mut children: Vec<Widget> = if st.section == 0 {
        let l = fetch_login(name);
        let pass_shown = if l.pass.is_empty() {
            "—".into()
        } else if st.reveal {
            l.pass.clone()
        } else {
            "•".repeat(l.pass.chars().count().min(24))
        };
        let mut user_trail = Vec::new();
        if !l.user.is_empty() {
            user_trail.push(btn("Copy", ButtonStyle::Ghost, A_COPY_USER));
        }
        let mut pass_trail = Vec::new();
        if !l.pass.is_empty() {
            pass_trail.push(btn(if st.reveal { "Hide" } else { "Show" }, ButtonStyle::Ghost, A_REVEAL));
            pass_trail.push(btn("Copy", ButtonStyle::Ghost, A_COPY_PASS));
        }
        vec![
            heading(name),
            detail_row("URL", &l.url, vec![]),
            detail_row("USERNAME", &l.user, user_trail),
            Widget::Column {
                children: vec![label("PASSWORD"), password_box(&pass_shown, pass_trail)],
                spacing: 4,
                align: Align::Stretch,
                modifiers: vec![],
            },
            detail_row("NOTE", &l.note, vec![]),
            button_row(vec![btn("Edit", ButtonStyle::Secondary, A_EDIT), delete_button(st)]),
        ]
    } else {
        vec![
            Widget::Row {
                children: vec![
                    Widget::Icon { id: IconId::WifiHigh, size: 20, modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted)] },
                    heading(name),
                ],
                spacing: Spacing::Sm.as_u16(),
                align: Align::Center,
                modifiers: vec![],
            },
            prefab::muted("Password stored. Showing it comes with the Wi-Fi integration."),
            button_row(vec![delete_button(st)]),
        ]
    };
    children.extend(message(st));
    Widget::Column { children, spacing: Spacing::Md.as_u16(), align: Align::Stretch, modifiers: pane_mods }
}

fn search_field(st: &St) -> Widget {
    let mut mods = vec![Modifier::Flex(1)];
    if st.focus_field == FOCUS_SEARCH {
        mods.push(Modifier::FocusRequest(st.focus));
    }
    let mut chrome = field_chrome();
    chrome.push(Modifier::Flex(1));
    Widget::Row {
        children: vec![
            Widget::Icon { id: IconId::MagnifyingGlass, size: 16, modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted)] },
            Widget::Input {
                value: st.search.as_str().into(),
                placeholder: "search".into(),
                on_submit: ActionId(A_SEARCH_ENTER),
                modifiers: mods,
            },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: chrome,
    }
}

fn list_screen(st: &St) -> Widget {
    let users = user_names();
    let wifis = wifi_names();
    let on_user = st.section == 0;
    let names: &[String] = if on_user { &users } else { &wifis };
    let shown = visible(st, names);

    let toolbar = prefab::toolbar(vec![
        search_field(st),
        btn("+ Add", ButtonStyle::Primary, A_ADD),
        btn("+ Wi-Fi", ButtonStyle::Secondary, A_ADD_WIFI),
        btn("Lock", ButtonStyle::Ghost, A_LOCK),
    ]);

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

    let mut rows: Vec<Widget> = Vec::new();
    for &i in &shown {
        let name = &names[i];
        let subtitle = if on_user { fetch_login(name).user } else { String::new() };
        rows.push(prefab::list_row(
            if on_user { IconId::Lock } else { IconId::WifiHigh },
            name,
            &subtitle,
            st.sel == i as i32,
            Some(ActionId(A_SEL_BASE + i as u32)),
            None,
        ));
    }
    if rows.is_empty() {
        rows.push(prefab::empty_state(if names.is_empty() { "No entries yet" } else { "No match" }));
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
        children: vec![sidebar, list, Widget::Divider, detail_pane(st, names, &shown)],
        spacing: 0,
        align: Align::Stretch,
        modifiers: vec![Modifier::Flex(1)],
    };

    Widget::Column {
        children: vec![toolbar, Widget::Divider, body],
        spacing: Spacing::None.as_u16(),
        align: Align::Stretch,
        modifiers: vec![Modifier::Flex(1)],
    }
}

fn menu_items(items: &[(&str, u32, &str)]) -> Widget {
    let entries: Vec<(String, ActionId)> = items.iter().map(|(l, a, _)| ((*l).into(), ActionId(*a))).collect();
    let hints: Vec<&str> = items.iter().map(|(_, _, h)| *h).collect();
    prefab::popover_menu_shortcuts(&entries, &hints, None)
}

/// The open dropdown. What it offers depends on the screen: locked, only
/// Close; in a form, Lock and Close; on the list, everything.
fn dropdown(st: &St, menu: Menu) -> Widget {
    let on_list = st.view == View::List;
    let unlocked = matches!(st.view, View::List | View::EditSecret | View::AddWifi);
    let (anchor, content) = match menu {
        Menu::Vault => {
            let mut items: Vec<(&str, u32, &str)> = Vec::new();
            if on_list {
                items.push(("New secret", A_ADD, "Ctrl+N"));
                items.push(("New Wi-Fi network", A_ADD_WIFI, ""));
            }
            if st.view == View::EditSecret {
                items.push(("Generate password", A_GEN, "Ctrl+G"));
                items.push(("Save", A_SAVE, "Ctrl+S"));
            }
            if unlocked {
                items.push(("Lock", A_LOCK, "Ctrl+L"));
            }
            items.push(("Close", A_CLOSE, "Esc"));
            (NODE_MENU_VAULT, menu_items(&items))
        }
        Menu::Edit => {
            let user = st.section == 0;
            let mut items: Vec<(&str, u32, &str)> = vec![("Find", A_FIND, "Ctrl+F")];
            if st.sel >= 0 {
                if user {
                    items.push(("Edit", A_EDIT, "F2"));
                    items.push(("Copy username", A_COPY_USER, "Ctrl+B"));
                    items.push(("Copy password", A_COPY_PASS, "Ctrl+C"));
                }
                items.push((if st.reveal { "Hide password" } else { "Show password" }, A_REVEAL, "Enter"));
                items.push(("Delete", A_DELETE, "Del"));
            }
            (NODE_MENU_EDIT, menu_items(&items))
        }
    };
    Widget::Popover {
        anchor: NodeId(anchor),
        child: alloc::boxed::Box::new(content),
        on_dismiss: ActionId(A_MENU_DISMISS),
        modifiers: vec![],
    }
}

fn render(st: &St) -> Widget {
    let mut labels: Vec<(String, ActionId)> = vec![("Vault".into(), ActionId(A_MENU_VAULT))];
    let mut anchors = vec![NodeId(NODE_MENU_VAULT)];
    if st.view == View::List {
        labels.push(("Edit".into(), ActionId(A_MENU_EDIT)));
        anchors.push(NodeId(NODE_MENU_EDIT));
    }
    let content = match st.view {
        View::Setup => setup_screen(st),
        View::Unlock => unlock_screen(st),
        View::List => list_screen(st),
        View::EditSecret => edit_screen(st),
        View::AddWifi => wifi_screen(st),
    };
    let mut children = vec![prefab::menu_bar_with_icon(IconId::Lock, &labels, &anchors), Widget::Divider, content];
    if let Some(m) = st.menu {
        children.push(dropdown(st, m));
    }
    Widget::Column {
        children,
        spacing: Spacing::None.as_u16(),
        align: Align::Stretch,
        modifiers: vec![Modifier::Background(Token::Surface)],
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

// ── Actions ────────────────────────────────────────────────────────────────

fn start_view() -> View {
    if !host::vault_user_present() {
        View::Setup
    } else if host::vault_unlocked() {
        View::List
    } else {
        View::Unlock
    }
}

fn clear_form(st: &mut St) {
    for f in st.f.iter_mut() {
        f.clear();
    }
    st.edit_orig.clear();
}

/// Views that need the user tier open. If something else locked the vault
/// (the shell, another instance), fall back to the unlock screen.
fn ensure_unlocked(st: &mut St) -> bool {
    if matches!(st.view, View::List | View::EditSecret | View::AddWifi) && !host::vault_unlocked() {
        clear_form(st);
        st.sel = -1;
        st.goto(View::Unlock);
        st.error("The vault was locked.");
        return false;
    }
    true
}

fn create_vault(st: &mut St) {
    let pass = st.f[0].as_str();
    if pass.chars().count() < MIN_PASSPHRASE {
        st.error("Use at least 12 characters.");
        st.focus_on(0);
    } else if pass != st.f[1].as_str() {
        st.f[1].clear();
        st.error("Passphrases do not match.");
        st.focus_on(1);
    } else if host::vault_set_passphrase("", pass) {
        clear_form(st);
        st.sel = -1;
        st.goto(View::List);
        st.info("Vault created.");
    } else {
        st.error("Could not create the vault.");
    }
}

fn unlock(st: &mut St) {
    if st.f[0].is_empty() {
        st.error("Enter your passphrase.");
        st.focus_on(0);
    } else if host::vault_unlock(st.f[0].as_str()) {
        clear_form(st);
        st.sel = -1;
        st.goto(View::List);
    } else {
        st.f[0].clear();
        st.error("Wrong passphrase.");
        st.focus_on(0);
    }
}

fn open_edit(st: &mut St, name: &str) {
    clear_form(st);
    let l = fetch_login(name);
    st.f[0].set(name);
    st.f[1].set(&l.url);
    st.f[2].set(&l.user);
    st.f[3].set(&l.pass);
    st.f[4].set(&l.note);
    st.edit_orig.set(name);
    st.goto(View::EditSecret);
}

fn select_name(st: &mut St, name: &str) {
    st.sel = section_names(st).iter().position(|n| n == name).map_or(-1, |i| i as i32);
}

fn save_secret(st: &mut St) {
    let name = st.f[0].as_str().trim();
    if name.is_empty() {
        st.error("Name is required.");
        st.focus_on(0);
        return;
    }
    let orig = st.edit_orig.as_str();
    if name != orig && user_names().iter().any(|n| n == name) {
        st.error("An entry with this name already exists.");
        st.focus_on(0);
        return;
    }
    let rec = encode_login(st.f[1].as_str(), st.f[2].as_str(), st.f[3].as_str(), st.f[4].as_str());
    // Write the new entry first; only then drop the old name of a rename,
    // so a failed write never loses the entry.
    if !host::vault_user_set(name, &rec) {
        st.error("Could not save — is the vault still unlocked?");
        return;
    }
    if !orig.is_empty() && orig != name {
        host::vault_user_delete(orig);
    }
    let saved = String::from(name);
    clear_form(st);
    st.section = 0;
    st.goto(View::List);
    select_name(st, &saved);
    st.info("Saved.");
}

fn save_wifi(st: &mut St) {
    let ssid = st.f[0].as_str().trim();
    if ssid.is_empty() {
        st.error("SSID is required.");
        st.focus_on(0);
        return;
    }
    let mut key = String::from("net:");
    key.push_str(ssid);
    if !host::vault_system_set(&key, st.f[1].as_str().as_bytes()) {
        st.error("Could not save the network.");
        return;
    }
    let saved = String::from(ssid);
    clear_form(st);
    st.section = 1;
    st.goto(View::List);
    select_name(st, &saved);
    st.info("Saved.");
}

fn save_form(st: &mut St) {
    match st.view {
        View::Setup => create_vault(st),
        View::Unlock => unlock(st),
        View::EditSecret => save_secret(st),
        View::AddWifi => save_wifi(st),
        View::List => {}
    }
}

/// Number of fields on the current form screen.
fn form_fields(view: View) -> usize {
    match view {
        View::Setup => 2,
        View::Unlock => 1,
        View::EditSecret => 5,
        View::AddWifi => 2,
        View::List => 0,
    }
}

/// Enter in field `i`: the next field, or submit on the last one.
fn on_enter(st: &mut St, i: usize) {
    if i + 1 < form_fields(st.view) {
        st.focus_on(i + 1);
    } else {
        save_form(st);
    }
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

/// First press arms, second press deletes.
fn delete_selected(st: &mut St) {
    let names = section_names(st);
    if st.sel < 0 || st.sel as usize >= names.len() {
        return;
    }
    if !st.confirm_del {
        st.confirm_del = true;
        st.info("Press Delete again to remove this entry.");
        return;
    }
    let name = &names[st.sel as usize];
    let ok = if st.section == 0 {
        host::vault_user_delete(name)
    } else {
        let mut key = String::from("net:");
        key.push_str(name);
        host::vault_system_delete(&key)
    };
    st.confirm_del = false;
    st.sel = -1;
    st.reveal = false;
    if ok {
        st.info("Deleted.");
    } else {
        st.error("Could not delete.");
    }
}

fn move_selection(st: &mut St, down: bool) {
    let names = section_names(st);
    let shown = visible(st, &names);
    if shown.is_empty() {
        return;
    }
    let pos = shown.iter().position(|&i| i as i32 == st.sel);
    let next = match (pos, down) {
        (None, _) => 0,
        (Some(p), true) => (p + 1).min(shown.len() - 1),
        (Some(p), false) => p.saturating_sub(1),
    };
    st.sel = shown[next] as i32;
    st.reveal = false;
}

fn copy_selected(st: &mut St, password: bool) {
    if st.section != 0 {
        return;
    }
    let users = user_names();
    if st.sel < 0 || st.sel as usize >= users.len() {
        return;
    }
    let l = fetch_login(&users[st.sel as usize]);
    let text = if password { &l.pass } else { &l.user };
    if text.is_empty() {
        st.info("Nothing to copy.");
    } else if host::clipboard_set(text) {
        if password {
            st.clip_hash = fnv(text.as_bytes());
            st.clip_deadline = host::ticks_ms() + CLIP_CLEAR_MS;
            st.info("Password copied — the clipboard clears in 30 s.");
        } else {
            st.info("Username copied.");
        }
    } else {
        st.error("Could not copy.");
    }
}

/// Clear the clipboard if it still holds the password we put there. False
/// while it cannot be checked (the clipboard is focus-gated).
fn clear_clipboard(st: &mut St) -> bool {
    if st.clip_hash == 0 {
        return true;
    }
    let mut buf = [0u8; 2048];
    match host::clipboard_get(&mut buf) {
        Some(n) => {
            if n <= buf.len() && fnv(&buf[..n]) == st.clip_hash {
                host::clipboard_set("");
            }
            st.clip_hash = 0;
            st.clip_deadline = 0;
            true
        }
        None => false,
    }
}

fn lock(st: &mut St) {
    clear_clipboard(st);
    host::vault_lock();
    clear_form(st);
    st.search.clear();
    st.sel = -1;
    st.goto(View::Unlock);
}

fn on_action(st: &mut St, id: u32) {
    if id != A_DELETE {
        st.confirm_del = false;
    }
    // A menu label toggles its own dropdown; anything else closes it.
    let open = st.menu;
    st.menu = None;
    match id {
        A_MENU_VAULT => st.menu = if open == Some(Menu::Vault) { None } else { Some(Menu::Vault) },
        A_MENU_EDIT => st.menu = if open == Some(Menu::Edit) { None } else { Some(Menu::Edit) },
        A_MENU_DISMISS => {}
        A_FIND => st.focus_on(FOCUS_SEARCH),
        A_GEN if st.view == View::EditSecret => {
            st.gen_open = true;
            fill_generated(st);
        }
        id if (A_GEN_CLASS_BASE..A_GEN_CLASS_BASE + GEN_CLASSES.len() as u32).contains(&id) => {
            st.gen_classes ^= 1 << (id - A_GEN_CLASS_BASE);
            fill_generated(st);
        }
        A_SAVE => save_form(st),
        A_REVEAL => st.reveal = !st.reveal,
        A_ADD => {
            clear_form(st);
            st.goto(View::EditSecret);
        }
        A_ADD_WIFI => {
            clear_form(st);
            st.goto(View::AddWifi);
        }
        A_LOCK => lock(st),
        A_CANCEL => {
            clear_form(st);
            st.goto(View::List);
        }
        A_SEC_USER | A_SEC_WIFI => {
            st.section = if id == A_SEC_USER { 0 } else { 1 };
            st.sel = -1;
            st.reveal = false;
            st.info("");
        }
        A_EDIT => edit_selected(st),
        A_DELETE => delete_selected(st),
        A_COPY_PASS => copy_selected(st, true),
        A_COPY_USER => copy_selected(st, false),
        A_SEARCH_ENTER => {
            let names = section_names(st);
            if let Some(&first) = visible(st, &names).first() {
                st.sel = first as i32;
                st.reveal = false;
            }
        }
        id if id >= A_SEL_BASE => {
            st.sel = (id - A_SEL_BASE) as i32;
            st.reveal = false;
            st.info("");
        }
        id if id >= A_ENTER_BASE => on_enter(st, (id - A_ENTER_BASE) as usize),
        _ => {}
    }
}

fn handle(st: &mut St, ev: Event) -> Outcome {
    match ev {
        // A form field changed: store it, no re-commit (see the module doc).
        Event::InputChangeAt { node, value } => {
            if let Some(i) = node.checked_sub(NODE_FIELD).map(|i| i as usize).filter(|&i| i < NFIELDS) {
                st.f[i].set(&value);
            }
            Outcome::Idle
        }
        // Untagged change: only the search box sends these. It re-commits so
        // the list filters as you type.
        Event::InputChange { value } => {
            if st.view == View::List {
                st.search.set(&value);
                st.info("");
                Outcome::Rerender
            } else {
                Outcome::Idle
            }
        }
        Event::Focus(true) => {
            ensure_unlocked(st);
            Outcome::Rerender
        }
        Event::Action(ActionId(A_CLOSE)) => Outcome::Exit,
        // The length slider regenerates as it moves.
        Event::Slide { action: ActionId(A_GEN_LEN), value, .. } if st.view == View::EditSecret => {
            let len = GEN_MIN + (value.min(SLIDER_MAX) as u32 * (GEN_MAX - GEN_MIN) + SLIDER_MAX as u32 / 2) / SLIDER_MAX as u32;
            if len == st.gen_len {
                return Outcome::Idle;
            }
            st.gen_len = len;
            fill_generated(st);
            Outcome::Rerender
        }
        Event::Action(ActionId(id)) => {
            if ensure_unlocked(st) {
                on_action(st, id);
            }
            Outcome::Rerender
        }
        // Ctrl chords (Ctrl implied). They reach the app even while a field
        // has the keyboard, except A/C/X/V, which stay with the editor.
        Event::Chord { letter, .. } => {
            if !ensure_unlocked(st) {
                return Outcome::Rerender;
            }
            match (letter, st.view) {
                (b'l', View::List | View::EditSecret | View::AddWifi) => lock(st),
                (b's', View::EditSecret | View::AddWifi | View::Setup) => save_form(st),
                (b'g', View::EditSecret) => {
                    st.gen_open = true;
                    fill_generated(st);
                }
                (b'n', View::List) => {
                    clear_form(st);
                    st.goto(View::EditSecret);
                }
                (b'b', View::List) => copy_selected(st, false),
                (b'f', View::List) => st.focus_on(FOCUS_SEARCH),
                _ => return Outcome::Idle,
            }
            Outcome::Rerender
        }
        Event::Key(KeyCode::Escape) if st.menu.is_some() => {
            st.menu = None;
            Outcome::Rerender
        }
        Event::Key(KeyCode::Escape) => match st.view {
            View::List if !st.search.is_empty() => {
                st.search.clear();
                Outcome::Rerender
            }
            View::List | View::Unlock | View::Setup => Outcome::Exit,
            View::EditSecret | View::AddWifi => {
                clear_form(st);
                st.goto(View::List);
                Outcome::Rerender
            }
        },
        // List keys arrive only while no text field has the keyboard
        // (Up/Down also pass through a focused search box).
        Event::Key(k) if st.view == View::List => {
            if !ensure_unlocked(st) {
                return Outcome::Rerender;
            }
            if k != KeyCode::Delete {
                st.confirm_del = false;
            }
            match k {
                KeyCode::Enter if st.sel >= 0 => st.reveal = !st.reveal,
                KeyCode::Down => move_selection(st, true),
                KeyCode::Up => move_selection(st, false),
                KeyCode::Delete => delete_selected(st),
                KeyCode::F(2) => edit_selected(st),
                // Ctrl+C without a focused text field arrives as the ETX byte.
                KeyCode::Char(0x03) => copy_selected(st, true),
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

fn shutdown(st: &mut St) {
    clear_clipboard(st);
    // Closing the window locks the user tier: no idle lock exists yet, and
    // an unlocked vault should not outlive the window that opened it.
    host::vault_lock();
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut st = St {
        view: start_view(),
        f: [Field::new(), Field::new(), Field::new(), Field::new(), Field::new()],
        search: Field::new(),
        section: 0,
        focus: 1,
        focus_field: 0,
        reveal: false,
        sel: -1,
        edit_orig: Field::new(),
        confirm_del: false,
        msg: "",
        msg_err: false,
        menu: None,
        gen_open: false,
        gen_len: 20,
        gen_classes: 0b1111,
        clip_hash: 0,
        clip_deadline: 0,
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
                        shutdown(&mut st);
                        host::close_widget();
                        return;
                    }
                },
                nopeek_widgets::events::Poll::Empty => break,
                nopeek_widgets::events::Poll::Gone => {
                    shutdown(&mut st);
                    return;
                }
            }
        }
        // A copied password is cleared once its time is up. The clipboard is
        // focus-gated: if we are not focused, retry on the next event.
        let now = host::ticks_ms();
        if st.clip_hash != 0 && now >= st.clip_deadline && clear_clipboard(&mut st) {
            st.info("Clipboard cleared.");
            redraw = true;
        }
        if redraw {
            commit_tree(&st);
        }
        let timeout = if st.clip_hash != 0 && now < st.clip_deadline {
            (st.clip_deadline - now).min(i32::MAX as u64) as i32
        } else {
            -1
        };
        host::wait(WAIT_INPUT, timeout);
    }
}
