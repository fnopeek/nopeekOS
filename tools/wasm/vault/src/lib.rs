//! vault — a small password manager over the kernel keyvault
//! (docs/plan/KEYSTORE.md).
//!
//! Three screens: set-up / unlock (masked passphrase), the list of user
//! secrets and system credentials, and the small add-forms. Only the kernel
//! holds the DEKs; this app reaches them through the `npk_vault_*` host
//! functions, which are usable only by this (signed) module.
//!
//! The bump allocator is reset every frame, so no state lives on the heap:
//! typed fields sit in fixed buffers and the entry lists are fetched fresh
//! from the kernel each frame.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use nopeek_widgets::host;
use nopeek_widgets::prefab::{self, ButtonStyle, InputKind};
use nopeek_widgets::style::{Padding, Spacing};
use nopeek_widgets::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// Read + exec + render — a normal windowed app. No WRITE: secrets go through
// the keyvault host calls, never the generic FS path. No shell role.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::READ | caps::EXEC | caps::RENDER, 0];

#[global_allocator]
static ALLOCATOR: nopeek_widgets::bump::Bump<{ 256 * 1024 }> = nopeek_widgets::bump::Bump::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    host::log_serial("[vault] panic!");
    core::arch::wasm32::unreachable()
}

const EVENT_BUF_SIZE: usize = 512;
const WAIT_INPUT: u32 = 1;

// Action ids.
const A_SUBMIT: u32 = 1;
const A_ADD_USER: u32 = 2;
const A_ADD_NET: u32 = 3;
const A_LOCK: u32 = 4;
const A_BACK: u32 = 5;

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Setup,
    Unlock,
    List,
    AddUserName,
    AddUserValue,
    AddNetSsid,
    AddNetPsk,
}

/// A fixed-size text field — no heap, so it survives the per-frame allocator
/// reset. 128 bytes covers a passphrase, an entry name, or a value.
struct Field {
    b: [u8; 128],
    n: usize,
}

impl Field {
    const fn new() -> Self {
        Field { b: [0u8; 128], n: 0 }
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
}

struct St {
    screen: Screen,
    input: Field,
    pending: Field,
    msg: &'static str,
    heap_base: usize,
}

enum Outcome {
    Idle,
    Rerender,
    Exit,
}

// ── Rendering ──────────────────────────────────────────────────────────

fn field(st: &St, placeholder: &str, kind: InputKind) -> Widget {
    prefab::input_autofocus(st.input.as_str(), placeholder, kind, ActionId(A_SUBMIT), None)
}

fn button(label: &str, style: ButtonStyle, action: u32) -> Widget {
    prefab::button(label, style, ActionId(action))
}

fn button_row(buttons: Vec<Widget>) -> Widget {
    Widget::Row {
        children: buttons,
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: vec![],
    }
}

fn parse_names(buf: &[u8], n: usize) -> Vec<String> {
    core::str::from_utf8(&buf[..n])
        .unwrap_or("")
        .split('\n')
        .filter(|s| !s.is_empty())
        .map(|s| s.into())
        .collect()
}

fn name_rows(names: &[String]) -> Vec<Widget> {
    names.iter().map(|n| prefab::body(n)).collect()
}

fn list_body() -> Vec<Widget> {
    let mut buf = [0u8; 4096];
    let users = match host::vault_user_list(&mut buf) {
        Some(n) => parse_names(&buf, n),
        None => Vec::new(),
    };
    let systems = match host::vault_system_list(&mut buf) {
        Some(n) => parse_names(&buf, n),
        None => Vec::new(),
    };

    let mut out: Vec<Widget> = Vec::new();
    out.push(prefab::muted("Your secrets"));
    if users.is_empty() {
        out.push(prefab::body("(no entries yet)"));
    } else {
        out.extend(name_rows(&users));
    }
    out.push(button_row(vec![
        button("+ Password", ButtonStyle::Primary, A_ADD_USER),
        button("+ Wi-Fi", ButtonStyle::Secondary, A_ADD_NET),
        button("Lock", ButtonStyle::Ghost, A_LOCK),
    ]));
    out.push(Widget::Divider);
    out.push(prefab::muted("Wi-Fi credentials"));
    if systems.is_empty() {
        out.push(prefab::body("(none)"));
    } else {
        out.extend(name_rows(&systems));
    }
    out
}

fn screen_body(st: &St) -> Vec<Widget> {
    match st.screen {
        Screen::Setup => vec![
            prefab::body("Set a vault password to protect your secrets."),
            field(st, "New vault password", InputKind::Password),
            button_row(vec![button("Set password", ButtonStyle::Primary, A_SUBMIT)]),
        ],
        Screen::Unlock => vec![
            prefab::body("Enter your vault password."),
            field(st, "Vault password", InputKind::Password),
            button_row(vec![button("Unlock", ButtonStyle::Primary, A_SUBMIT)]),
        ],
        Screen::List => list_body(),
        Screen::AddUserName => vec![
            prefab::body("New entry name (e.g. mail@example.com)"),
            field(st, "Name", InputKind::Text),
            button_row(vec![
                button("Next", ButtonStyle::Primary, A_SUBMIT),
                button("Cancel", ButtonStyle::Ghost, A_BACK),
            ]),
        ],
        Screen::AddUserValue => vec![
            prefab::body("Password / secret value"),
            field(st, "Value", InputKind::Text),
            button_row(vec![
                button("Save", ButtonStyle::Primary, A_SUBMIT),
                button("Cancel", ButtonStyle::Ghost, A_BACK),
            ]),
        ],
        Screen::AddNetSsid => vec![
            prefab::body("Wi-Fi network name (SSID)"),
            field(st, "SSID", InputKind::Text),
            button_row(vec![
                button("Next", ButtonStyle::Primary, A_SUBMIT),
                button("Cancel", ButtonStyle::Ghost, A_BACK),
            ]),
        ],
        Screen::AddNetPsk => vec![
            prefab::body("Wi-Fi password"),
            field(st, "Password", InputKind::Password),
            button_row(vec![
                button("Save", ButtonStyle::Primary, A_SUBMIT),
                button("Cancel", ButtonStyle::Ghost, A_BACK),
            ]),
        ],
    }
}

fn render(st: &St) -> Widget {
    let mut children = vec![prefab::title_bar("Vault")];
    children.extend(screen_body(st));
    if !st.msg.is_empty() {
        children.push(prefab::muted(st.msg));
    }
    Widget::Column {
        children,
        spacing: Spacing::Sm.as_u16(),
        align: Align::Start,
        modifiers: vec![
            Modifier::Background(Token::Surface),
            Modifier::PaddingXY { x: Padding::Md.as_u16(), y: Padding::Md.as_u16() },
            Modifier::MinWidth(400),
            Modifier::MinHeight(340),
        ],
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

fn start_screen() -> Screen {
    // A user tier exists → unlock; otherwise (no vault, or device-only) set up.
    if host::vault_user_present() {
        Screen::Unlock
    } else {
        Screen::Setup
    }
}

fn on_submit(st: &mut St) -> Outcome {
    match st.screen {
        Screen::Setup => {
            if host::vault_set_passphrase("", st.input.as_str()) {
                st.input.clear();
                st.msg = "";
                st.screen = Screen::List;
            } else {
                st.msg = "Could not set the password.";
            }
        }
        Screen::Unlock => {
            if host::vault_unlock(st.input.as_str()) {
                st.input.clear();
                st.msg = "";
                st.screen = Screen::List;
            } else {
                st.input.clear();
                st.msg = "Wrong password.";
            }
        }
        Screen::AddUserName => {
            if !st.input.as_str().is_empty() {
                st.pending.set(st.input.as_str());
                st.input.clear();
                st.screen = Screen::AddUserValue;
            }
        }
        Screen::AddUserValue => {
            host::vault_user_set(st.pending.as_str(), st.input.as_str());
            st.input.clear();
            st.pending.clear();
            st.screen = Screen::List;
        }
        Screen::AddNetSsid => {
            if !st.input.as_str().is_empty() {
                st.pending.set(st.input.as_str());
                st.input.clear();
                st.screen = Screen::AddNetPsk;
            }
        }
        Screen::AddNetPsk => {
            let mut key = String::from("net:");
            key.push_str(st.pending.as_str());
            host::vault_system_set(&key, st.input.as_str());
            st.input.clear();
            st.pending.clear();
            st.screen = Screen::List;
        }
        Screen::List => {}
    }
    Outcome::Rerender
}

fn handle(st: &mut St, ev: Event) -> Outcome {
    match ev {
        // The compositor owns the edit buffer and shows it live; mirror it so
        // submit reads the right value. No re-commit per keystroke.
        Event::InputChange { value } => {
            st.input.set(&value);
            Outcome::Idle
        }
        Event::Action(ActionId(id)) => match id {
            A_SUBMIT => on_submit(st),
            A_ADD_USER => {
                st.input.clear();
                st.msg = "";
                st.screen = Screen::AddUserName;
                Outcome::Rerender
            }
            A_ADD_NET => {
                st.input.clear();
                st.msg = "";
                st.screen = Screen::AddNetSsid;
                Outcome::Rerender
            }
            A_LOCK => {
                host::vault_lock();
                st.input.clear();
                st.screen = Screen::Unlock;
                Outcome::Rerender
            }
            A_BACK => {
                st.input.clear();
                st.pending.clear();
                st.screen = Screen::List;
                Outcome::Rerender
            }
            _ => Outcome::Idle,
        },
        Event::Key(KeyCode::Escape) => match st.screen {
            Screen::List | Screen::Unlock | Screen::Setup => Outcome::Exit,
            _ => {
                st.input.clear();
                st.pending.clear();
                st.screen = Screen::List;
                Outcome::Rerender
            }
        },
        _ => Outcome::Idle,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mut st = St {
        screen: start_screen(),
        input: Field::new(),
        pending: Field::new(),
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
