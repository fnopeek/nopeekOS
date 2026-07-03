//! beak — native, sandboxed web browser for nopeekOS (BROWSER.md).
//!
//! Stage 0.1: the page is rendered by the portable `beak-engine` (own block
//! layout + fontdue rasterisation) into a `Widget::Canvas`; the chrome
//! (toolbar, address bar, footer) is loft-styled widgets. Scroll comes via
//! `Event::Wheel`, link clicks via a Canvas hit-test against the engine's
//! link rects. The engine is host-agnostic (§10); this shell is the thin
//! nopeek adapter (queries the canvas rect, paints, forwards input).

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;

use beak_engine::Engine;
use linked_list_allocator::LockedHeap;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::{caps, prefab};
use nopeek_widgets::*;

// ── App metadata + capabilities ───────────────────────────────────────────

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// RENDER (scene / event / canvas_rect) + CANVAS (canvas_commit) + NET (fetch).
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 2] = [caps::RENDER | caps::CANVAS, caps::ext::NET];

// ── Host functions ────────────────────────────────────────────────────────

unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_http_request(url_ptr: i32, url_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32) -> i32;
    fn npk_canvas_rect(canvas_id: i32, out_ptr: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(m: &str) {
    unsafe { npk_log_serial(m.as_ptr() as i32, m.len() as i32) };
}

const CANVAS_ID: i32 = 1;
const ACT_GO: u32 = 1;
const ACT_RELOAD: u32 = 2;

// ── Persistent state (static buffers — no heap growth across page loads) ───

const URL_CAP: usize = 4096;
static mut URL_BUF: [u8; URL_CAP] = [0; URL_CAP];
static mut URL_LEN: usize = 0;

const HTML_CAP: usize = 3 * 1024 * 1024;
static mut HTML_BUF: [u8; HTML_CAP] = [0; HTML_CAP];
static mut HTML_LEN: usize = 0;

const PAYLOAD_CAP: usize = URL_CAP;
static mut PAYLOAD_BUF: [u8; PAYLOAD_CAP] = [0; PAYLOAD_CAP];

const EVENT_BUF_SIZE: usize = 16 * 1024;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

static mut RECT_BUF: [u8; 16] = [0; 16];

static mut SCROLL_Y: i32 = 0;
static mut DIRTY: bool = true; // page content needs a repaint
static mut LAST_W: i32 = -1;
static mut LAST_H: i32 = -1;

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
fn payload_str(len: usize) -> &'static str {
    unsafe {
        let ptr = core::ptr::addr_of!(PAYLOAD_BUF) as *const u8;
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).unwrap_or("")
    }
}
fn scroll_y() -> i32 {
    unsafe { core::ptr::addr_of!(SCROLL_Y).read() }
}
fn set_scroll(y: i32) {
    unsafe { core::ptr::addr_of_mut!(SCROLL_Y).write(y) };
}
fn mark_dirty() {
    unsafe { core::ptr::addr_of_mut!(DIRTY).write(true) };
}

/// Fetch `url` into HTML_BUF; resets scroll + marks dirty.
fn fetch(url: &str) -> bool {
    let dst = core::ptr::addr_of_mut!(HTML_BUF) as *mut u8;
    let n = unsafe { npk_http_request(url.as_ptr() as i32, url.len() as i32, dst as i32, HTML_CAP as i32) };
    let len = if n < 0 { 0 } else { n as usize };
    unsafe { core::ptr::addr_of_mut!(HTML_LEN).write(len) };
    set_scroll(0);
    mark_dirty();
    n >= 0
}

/// Navigate the address bar's typed text (normalise scheme, then fetch).
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

/// Follow a link href relative to the current page.
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
        alloc::format!("https://{}", url.split('/').next().unwrap_or(""))
    }
}
fn resolve(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("https://") || href.starts_with("http://") {
        return href.to_string();
    }
    if let Some(rest) = href.strip_prefix("//") {
        return alloc::format!("https://{}", rest);
    }
    if href.is_empty() || href.starts_with('#') {
        return base.to_string();
    }
    let origin = origin_of(base);
    if href.starts_with('/') {
        return alloc::format!("{}{}", origin, href);
    }
    let path = base.split(['?', '#']).next().unwrap_or(base);
    match path.rfind('/').filter(|&i| i > origin.len().saturating_sub(1)) {
        Some(i) => alloc::format!("{}{}", &path[..=i], href),
        None => alloc::format!("{}/{}", origin, href),
    }
}

/// Query the canvas widget's actual laid-out rect (x, y, w, h) in the app's
/// window space. `None` until the compositor has laid it out at least once.
fn canvas_rect() -> Option<(i32, i32, i32, i32)> {
    let out = core::ptr::addr_of_mut!(RECT_BUF) as *mut u8;
    if unsafe { npk_canvas_rect(CANVAS_ID, out as i32) } != 0 {
        return None;
    }
    let b = unsafe { core::slice::from_raw_parts(out as *const u8, 16) };
    let rd = |i: usize| i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    Some((rd(0), rd(4), rd(8), rd(12)))
}

/// Re-layout + paint the visible slice into the canvas if it's dirty or the
/// viewport resized. Paints only the viewport (bounded memory, any page
/// length — long one-pagers just scroll).
fn maybe_repaint(engine: &Engine) {
    let (_x, _y, w, h) = match canvas_rect() {
        Some(r) => r,
        None => return,
    };
    if w <= 0 || h <= 0 {
        return;
    }
    let dirty = unsafe { core::ptr::addr_of!(DIRTY).read() };
    let lw = unsafe { core::ptr::addr_of!(LAST_W).read() };
    let lh = unsafe { core::ptr::addr_of!(LAST_H).read() };
    if !dirty && w == lw && h == lh {
        return;
    }

    if lw < 0 {
        log("[beak] first paint w/h:");
        log(u32_str(w as u32));
        log(u32_str(h as u32));
    }
    let layout = engine.layout(html_str(), w as u32);
    // clamp scroll to the document
    let max_scroll = (layout.height as i32 - h).max(0);
    let sy = scroll_y().clamp(0, max_scroll);
    set_scroll(sy);

    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    engine.paint(&layout, w as u32, h as u32, sy, &mut buf);
    unsafe { npk_canvas_commit(CANVAS_ID, buf.as_ptr() as i32, buf.len() as i32, w, h) };

    unsafe {
        core::ptr::addr_of_mut!(LAST_W).write(w);
        core::ptr::addr_of_mut!(LAST_H).write(h);
        core::ptr::addr_of_mut!(DIRTY).write(false);
    }
}

/// Commit the loft-styled chrome: a flush toolbar (nav icon-buttons + a
/// framed address bar in loft's `search_input` idiom) over the canvas body.
/// No footer — the body fills to the bottom edge (file-manager idiom).
fn render_chrome() {
    // loft's framed-input idiom: icon prefix + Input, SurfaceMuted fill, a
    // visible Border stroke (not just on focus), Focus→Accent — grown to fill.
    let address = Widget::Row {
        children: vec![
            Widget::Icon {
                id: IconId::Bird,
                size: 24,
                modifiers: vec![Modifier::Tint(Token::Accent)],
            },
            Widget::Input {
                value: url_str().to_string(),
                placeholder: "Adresse eingeben …".to_string(),
                on_submit: ActionId(ACT_GO),
                modifiers: vec![Modifier::Flex(1)],
            },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: vec![
            Modifier::Flex(1),
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
            Modifier::MinWidth(220),
            Modifier::Focus(vec![Modifier::Border {
                token: Token::Accent,
                width: 1,
                radius: Radius::Md.as_u8(),
            }]),
        ],
    };

    let toolbar = Widget::Row {
        children: vec![
            prefab::icon_button(IconId::ArrowClockwise, 24, Some(ActionId(ACT_RELOAD)), None),
            address,
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: vec![Modifier::Padding(Padding::Sm.as_u16())],
    };

    let tree = Widget::Column {
        children: vec![
            toolbar,
            Widget::Divider,
            Widget::Canvas {
                id: CanvasId(CANVAS_ID as u32),
                width: 800,
                height: 600,
                modifiers: vec![Modifier::Flex(1), Modifier::Background(Token::Surface)],
            },
        ],
        spacing: Spacing::None.as_u16(),
        align: Align::Stretch,
        modifiers: vec![],
    };

    match wire::encode(&tree) {
        Ok(b) => {
            if unsafe { npk_scene_commit(b.as_ptr() as i32, b.len() as i32) } < 0 {
                log("[beak] commit failed");
            }
        }
        Err(_) => log("[beak] encode failed"),
    }
}

/// Handle one event. Returns true if the chrome (address bar / title) should
/// be re-committed.
fn handle(engine: &Engine, ev: Event) -> bool {
    match ev {
        // Keep URL_BUF synced with the address-bar edit buffer.
        Event::InputChange { value } => {
            set_url(&value);
            false
        }
        Event::Action(ActionId(id)) => {
            if id == ACT_GO {
                let t = url_str().to_string();
                go(&t);
                true
            } else if id == ACT_RELOAD {
                let t = url_str().to_string();
                if !t.is_empty() {
                    go(&t);
                }
                true
            } else {
                false
            }
        }
        // Link clicks land in the canvas → hit-test the engine's link rects.
        Event::MouseButton { button: MouseButton::Left, down: true, x, y } => {
            if let Some((rx, ry, w, h)) = canvas_rect() {
                if x >= rx && x < rx + w && y >= ry && y < ry + h {
                    let cx = x - rx;
                    let cy = y - ry + scroll_y();
                    let layout = engine.layout(html_str(), w as u32);
                    if let Some(href) = layout.hit_test(cx, cy) {
                        let href = href.to_string();
                        follow(&href);
                        return true;
                    }
                }
            }
            false
        }
        Event::Wheel { dy } => {
            set_scroll(scroll_y() + dy);
            mark_dirty();
            false
        }
        Event::Open(s) => {
            go(&s);
            true
        }
        _ => false,
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

// ── Heap: a real free-list allocator (32 MB). The font (persistent) + each
//    frame's layout + paint buffer are freed on drop, unlike a bump heap. ───

const HEAP_SIZE: usize = 32 * 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// u32 → decimal &str in a static buffer (no alloc — safe in the panic handler
// even when the panic is an allocation failure).
static mut NUMBUF: [u8; 12] = [0; 12];
fn u32_str(mut n: u32) -> &'static str {
    let b = core::ptr::addr_of_mut!(NUMBUF) as *mut u8;
    let buf = unsafe { core::slice::from_raw_parts_mut(b, 12) };
    let mut i = 12;
    if n == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i..]) }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log("[beak] PANIC");
    if let Some(loc) = info.location() {
        log(loc.file());
        log(u32_str(loc.line()));
    } else {
        log("[beak] no location (likely alloc failure)");
    }
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Init the heap FIRST — before any allocation.
    unsafe {
        ALLOCATOR.lock().init(core::ptr::addr_of_mut!(HEAP) as *mut u8, HEAP_SIZE);
    }

    // Launch argument: `npk_open("beak", "https://…")` opens straight to a URL.
    let arg_len = {
        let p = core::ptr::addr_of_mut!(PAYLOAD_BUF) as *mut u8;
        let n = unsafe { npk_launch_arg(p as i32, PAYLOAD_CAP as i32) };
        if n > 0 { n as usize } else { 0 }
    };
    if arg_len > 0 {
        let arg = payload_str(arg_len).to_string();
        go(&arg);
    }

    log("[beak] parsing font…");
    let engine = Engine::new();
    log("[beak] engine ready");

    render_chrome();

    loop {
        match poll_event() {
            PollResult::Event(ev) => {
                if handle(&engine, ev) {
                    render_chrome();
                }
                maybe_repaint(&engine);
            }
            PollResult::Empty => {
                maybe_repaint(&engine);
                unsafe {
                    let _ = npk_sleep(16);
                }
            }
            PollResult::WindowGone => {
                unsafe {
                    let _ = npk_close_widget();
                }
                return;
            }
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<app_meta::IconRef> {
    None
}
