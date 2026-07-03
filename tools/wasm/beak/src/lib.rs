//! beak — native, sandboxed web browser for nopeekOS (BROWSER.md).
//!
//! Slice 0 (the reader): address bar → `npk_http_request` fetch → the
//! portable `beak_engine` lowers HTML to platform-neutral `Block`s → this
//! nopeekOS adapter maps them to a scrollable widget tree with clickable
//! links. The real layout/paint engine (own box/flex/grid + Canvas) grows
//! inside `beak-engine` next; this shell stays the thin nopeek port (§10).

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};

use beak_engine::Block;
use nopeek_widgets::caps;
use nopeek_widgets::*;

// ── App metadata + capabilities ───────────────────────────────────────────

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// RENDER (scene_commit / event_poll) + NET (npk_http_request). No WRITE, no
// filesystem — page content never touches npkFS. NET lives in the 2nd caps
// byte (the 1st byte's 8 bits are full — see nopeek_widgets::caps::ext).
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::RENDER, caps::ext::NET];

// ── Host functions ────────────────────────────────────────────────────────

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_http_request(url_ptr: i32, url_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(msg: &str) {
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32) };
}
fn commit(bytes: &[u8]) -> i32 {
    unsafe { npk_scene_commit(bytes.as_ptr() as i32, bytes.len() as i32) }
}
fn close_self() {
    unsafe {
        let _ = npk_close_widget();
    }
}

// ── Actions ───────────────────────────────────────────────────────────────

const ACT_GO: u32 = 1;
const ACT_LINK_BASE: u32 = 100_000;

// ── Persistent state — all in static buffers (no heap growth across page
//    loads; the heap holds only the per-frame widget tree + parse output,
//    reset every iteration). ──────────────────────────────────────────────

const URL_CAP: usize = 4096;
static mut URL_BUF: [u8; URL_CAP] = [0; URL_CAP];
static mut URL_LEN: usize = 0;

const HTML_CAP: usize = 2 * 1024 * 1024;
static mut HTML_BUF: [u8; HTML_CAP] = [0; HTML_CAP];
static mut HTML_LEN: usize = 0;

const PAYLOAD_CAP: usize = URL_CAP;
static mut PAYLOAD_BUF: [u8; PAYLOAD_CAP] = [0; PAYLOAD_CAP];

const EVENT_BUF_SIZE: usize = 8 * 1024;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

fn set_url(s: &str) {
    let n = s.len().min(URL_CAP);
    unsafe {
        core::ptr::copy_nonoverlapping(s.as_ptr(), core::ptr::addr_of_mut!(URL_BUF) as *mut u8, n);
        core::ptr::addr_of_mut!(URL_LEN).write(n);
    }
}
fn url_str() -> &'static str {
    unsafe {
        let len = core::ptr::addr_of!(URL_LEN).read();
        let ptr = core::ptr::addr_of!(URL_BUF) as *const u8;
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).unwrap_or("")
    }
}
fn html_str() -> &'static str {
    unsafe {
        let len = core::ptr::addr_of!(HTML_LEN).read();
        let ptr = core::ptr::addr_of!(HTML_BUF) as *const u8;
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).unwrap_or("")
    }
}
fn copy_payload(s: &str) -> usize {
    let n = s.len().min(PAYLOAD_CAP);
    unsafe {
        core::ptr::copy_nonoverlapping(
            s.as_ptr(),
            core::ptr::addr_of_mut!(PAYLOAD_BUF) as *mut u8,
            n,
        );
    }
    n
}
fn payload_str(len: usize) -> &'static str {
    unsafe {
        let ptr = core::ptr::addr_of!(PAYLOAD_BUF) as *const u8;
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).unwrap_or("")
    }
}

/// Fetch `url` into HTML_BUF via the host. Returns true on success.
fn fetch(url: &str) -> bool {
    let dst = core::ptr::addr_of_mut!(HTML_BUF) as *mut u8;
    let n = unsafe { npk_http_request(url.as_ptr() as i32, url.len() as i32, dst as i32, HTML_CAP as i32) };
    let len = if n < 0 { 0 } else { n as usize };
    unsafe { core::ptr::addr_of_mut!(HTML_LEN).write(len) };
    n >= 0
}

/// Navigate the address bar's typed text (normalize scheme, then fetch).
fn go(typed: &str) {
    let t = typed.trim();
    if t.is_empty() {
        return;
    }
    let abs = if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        let mut s = String::from("https://");
        s.push_str(t);
        s
    };
    set_url(&abs);
    if !fetch(&abs) {
        log("[beak] fetch failed");
    }
}

/// Follow a link `href` relative to the current page.
fn follow(href: &str) {
    let base = url_str().to_string();
    let abs = resolve(&base, href);
    set_url(&abs);
    if !fetch(&abs) {
        log("[beak] fetch failed");
    }
}

fn origin_of(url: &str) -> String {
    if let Some(pos) = url.find("://") {
        let host_start = pos + 3;
        let host_end = url[host_start..].find('/').map(|i| host_start + i).unwrap_or(url.len());
        url[..host_end].to_string()
    } else {
        format!("https://{}", url.split('/').next().unwrap_or(""))
    }
}
fn resolve(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("https://") || href.starts_with("http://") {
        return href.to_string();
    }
    if let Some(rest) = href.strip_prefix("//") {
        return format!("https://{}", rest);
    }
    if href.is_empty() || href.starts_with('#') {
        return base.to_string();
    }
    let origin = origin_of(base);
    if href.starts_with('/') {
        return format!("{}{}", origin, href);
    }
    // Relative to the current directory.
    let path = base.split(['?', '#']).next().unwrap_or(base);
    let dir_end = path.rfind('/').filter(|&i| i > origin.len().saturating_sub(1));
    match dir_end {
        Some(i) => format!("{}{}", &path[..=i], href),
        None => format!("{}/{}", origin, href),
    }
}

fn nth_link_href(idx: usize) -> Option<String> {
    beak_engine::parse(html_str())
        .into_iter()
        .filter_map(|b| match b {
            Block::Link { href, .. } => Some(href),
            _ => None,
        })
        .nth(idx)
}

// ── Rendering ─────────────────────────────────────────────────────────────

fn text(content: &str, style: TextStyle, modifiers: Vec<Modifier>) -> Widget {
    Widget::Text { content: content.to_string(), style, modifiers }
}

fn render() {
    let blocks = beak_engine::parse(html_str());
    let mut body: Vec<Widget> = Vec::new();
    let mut link_idx: u32 = 0;
    for b in &blocks {
        match b {
            Block::Heading { level, text: t } => {
                let style = if *level <= 2 { TextStyle::Title } else { TextStyle::Heading };
                body.push(text(t, style, vec![]));
            }
            Block::Para(t) => body.push(text(t, TextStyle::Body, vec![])),
            Block::ListItem(t) => {
                let s = format!("\u{2022}  {}", t);
                body.push(text(&s, TextStyle::Body, vec![Modifier::Padding(2)]));
            }
            Block::Link { text: t, .. } => {
                let id = ACT_LINK_BASE + link_idx;
                link_idx += 1;
                body.push(text(
                    t,
                    TextStyle::Body,
                    vec![
                        Modifier::Tint(Token::Accent),
                        Modifier::OnClick(ActionId(id)),
                        Modifier::RoleOverride(Role::Link),
                    ],
                ));
            }
            Block::Rule => body.push(Widget::Divider),
        }
    }
    if body.is_empty() {
        body.push(text(
            "Gib oben eine URL ein und drücke Enter.",
            TextStyle::Muted,
            vec![],
        ));
    }

    let status = if html_str().is_empty() {
        "beak — bereit".to_string()
    } else {
        let title = beak_engine::title(html_str()).unwrap_or_default();
        if title.is_empty() {
            format!("{}  ·  {} bytes", url_str(), html_str().len())
        } else {
            format!("{}  ·  {}", title, url_str())
        }
    };

    let address_bar = Widget::Row {
        children: vec![
            Widget::Input {
                value: url_str().to_string(),
                placeholder: "https://example.com".to_string(),
                on_submit: ActionId(ACT_GO),
                modifiers: vec![Modifier::Flex(1)],
            },
            Widget::Button {
                label: "Go".to_string(),
                icon: IconId::ArrowRight,
                on_click: ActionId(ACT_GO),
                modifiers: vec![],
            },
        ],
        spacing: 8,
        align: Align::Center,
        modifiers: vec![Modifier::Padding(8)],
    };

    let tree = Widget::Column {
        children: vec![
            address_bar,
            Widget::Divider,
            text(&status, TextStyle::Caption, vec![Modifier::Padding(6)]),
            Widget::Divider,
            Widget::Scroll {
                child: Box::new(Widget::Column {
                    children: body,
                    spacing: 10,
                    align: Align::Start,
                    modifiers: vec![Modifier::Padding(12)],
                }),
                axis: Axis::Vertical,
                modifiers: vec![Modifier::Flex(1)],
            },
        ],
        spacing: 0,
        align: Align::Stretch,
        modifiers: vec![],
    };

    match wire::encode(&tree) {
        Ok(bytes) => {
            if commit(&bytes) < 0 {
                log("[beak] commit failed");
            }
        }
        Err(_) => log("[beak] encode failed"),
    }
}

// ── Event loop ────────────────────────────────────────────────────────────

enum Outcome {
    Idle,
    Rerender,
    Exit,
}

fn handle(ev: Event, payload: &str) -> Outcome {
    match ev {
        // Keep URL_BUF synced with the address-bar edit buffer (the
        // compositor owns the live field, so no rerender needed per key).
        Event::InputChange { .. } => {
            set_url(payload);
            Outcome::Idle
        }
        Event::Action(ActionId(id)) => {
            if id == ACT_GO {
                let typed = url_str().to_string();
                go(&typed);
                Outcome::Rerender
            } else if id >= ACT_LINK_BASE {
                if let Some(href) = nth_link_href((id - ACT_LINK_BASE) as usize) {
                    follow(&href);
                }
                Outcome::Rerender
            } else {
                Outcome::Idle
            }
        }
        Event::Open(_) => {
            go(payload);
            Outcome::Rerender
        }
        _ => Outcome::Idle,
    }
}

enum PollResult {
    Event(Event),
    Empty,
    WindowGone,
}

fn poll_event() -> PollResult {
    let buf_ptr = core::ptr::addr_of_mut!(EVENT_BUF) as *mut u8;
    let n = unsafe { npk_event_poll(buf_ptr as i32, EVENT_BUF_SIZE as i32) };
    if n < 0 {
        return PollResult::WindowGone;
    }
    if n == 0 {
        return PollResult::Empty;
    }
    let slice = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, n as usize) };
    match postcard::from_bytes::<Event>(slice) {
        Ok(ev) => PollResult::Event(ev),
        Err(_) => PollResult::Empty,
    }
}

// ── Bump allocator ────────────────────────────────────────────────────────
//
// Only the per-frame widget tree + engine parse output live on the heap;
// everything persistent is in static buffers above. We reset to a fixed base
// mark before handling each event so the heap never grows across page loads.

const HEAP_SIZE: usize = 8 * 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct BumpAllocator;
unsafe impl core::alloc::GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let pos_ptr = core::ptr::addr_of_mut!(HEAP_POS);
        let current = unsafe { pos_ptr.read() };
        let aligned = (current + layout.align() - 1) & !(layout.align() - 1);
        if aligned + layout.size() > HEAP_SIZE {
            return core::ptr::null_mut();
        }
        unsafe { pos_ptr.write(aligned + layout.size()) };
        unsafe { (core::ptr::addr_of_mut!(HEAP) as *mut u8).add(aligned) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}
#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

fn alloc_reset(pos: usize) {
    unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(pos) };
}
fn alloc_mark() -> usize {
    unsafe { core::ptr::addr_of!(HEAP_POS).read() }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[beak] panic!");
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Launch argument: `npk_open("beak", "https://…")` opens straight to a URL.
    let arg_len = {
        let ptr = core::ptr::addr_of_mut!(PAYLOAD_BUF) as *mut u8;
        let n = unsafe { npk_launch_arg(ptr as i32, PAYLOAD_CAP as i32) };
        if n > 0 { n as usize } else { 0 }
    };
    if arg_len > 0 {
        let arg = payload_str(arg_len).to_string();
        go(&arg);
    }

    let base = alloc_mark();
    render();

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                // Stabilize heap-backed payloads (InputChange value / Open arg)
                // into the static buffer before the reset frees the event.
                let plen = match &ev {
                    Event::InputChange { value } => copy_payload(value),
                    Event::Open(s) => copy_payload(s),
                    _ => 0,
                };
                alloc_reset(base);
                match handle(ev, payload_str(plen)) {
                    Outcome::Idle => {}
                    Outcome::Rerender => render(),
                    Outcome::Exit => {
                        close_self();
                        return;
                    }
                }
            }
            PollResult::Empty => {
                unsafe { let _ = npk_sleep(16); }
            }
            PollResult::WindowGone => return,
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<app_meta::IconRef> {
    None
}
