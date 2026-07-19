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

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use beak_engine::{Engine, Layout};
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
    fn npk_theme_token(token_id: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
}

fn log(m: &str) {
    unsafe { npk_log_serial(m.as_ptr() as i32, m.len() as i32) };
}

/// Query one theme token's colour → Rgb (palette is 0xAARRGGBB, light/dark
/// aware). This is why the page background follows the theme like the chrome.
fn theme_rgb(token_id: i32) -> beak_engine::Rgb {
    let c = unsafe { npk_theme_token(token_id) } as u32;
    beak_engine::Rgb(((c >> 16) & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, (c & 0xFF) as u8)
}

/// Resolve the page palette from the active theme (Surface bg, OnSurface text,
/// Accent links …). Falls back to dark if the query returns nothing.
fn query_theme() -> beak_engine::Theme {
    let bg = theme_rgb(0); // Surface
    if bg == beak_engine::Rgb(0, 0, 0) {
        return beak_engine::Theme::DARK;
    }
    beak_engine::Theme {
        bg,
        text: theme_rgb(3),    // OnSurface
        heading: theme_rgb(3), // OnSurface
        link: theme_rgb(6),    // Accent
        muted: theme_rgb(4),   // OnSurfaceMuted
        rule: theme_rgb(8),    // Border
    }
}

const CANVAS_ID: i32 = 1;

// Toolbar
const ACT_GO: u32 = 1;
const ACT_BACK: u32 = 2;
const ACT_FORWARD: u32 = 3;
const ACT_RELOAD: u32 = 4;

// Menu-bar labels (toggle a dropdown)
const ACT_MENU_FILE: u32 = 5_000;
const ACT_MENU_EDIT: u32 = 5_001;
const ACT_MENU_VIEW: u32 = 5_002;
const ACT_MENU_HELP: u32 = 5_004;
const ACT_MENU_DISMISS: u32 = 5_500;

// Menu items
const ACT_FILE_CLOSE: u32 = 6_000;
const ACT_VIEW_RELOAD: u32 = 6_020;
const ACT_VIEW_TOGGLE_CSS: u32 = 6_021;
const ACT_HELP_ABOUT: u32 = 6_100;

// Menu-label anchor NodeIds (for the dropdown Popover)
const NODE_MENU_FILE: u32 = 100;
const NODE_MENU_EDIT: u32 = 101;
const NODE_MENU_VIEW: u32 = 102;
const NODE_MENU_HELP: u32 = 104;

// Which menu dropdown is open (0 = none, else ACT_MENU_FILE..HELP encoded 1..4).
static mut OPEN_MENU: u8 = 0;
fn open_menu() -> u8 {
    unsafe { core::ptr::addr_of!(OPEN_MENU).read() }
}
fn set_open_menu(v: u8) {
    unsafe { core::ptr::addr_of_mut!(OPEN_MENU).write(v) };
}
fn toggle_menu(which: u8) {
    let cur = open_menu();
    set_open_menu(if cur == which { 0 } else { which });
}

// ── Persistent state (static buffers — no heap growth across page loads) ───

const URL_CAP: usize = 4096;
static mut URL_BUF: [u8; URL_CAP] = [0; URL_CAP];
static mut URL_LEN: usize = 0;

const HTML_CAP: usize = 3 * 1024 * 1024;
static mut HTML_BUF: [u8; HTML_CAP] = [0; HTML_CAP];
static mut HTML_LEN: usize = 0;

// Concatenated bytes of the page's external <link rel=stylesheet> files.
const CSS_CAP: usize = 2 * 1024 * 1024;
const MAX_CSS_LINKS: usize = 16;
static mut CSS_BUF: [u8; CSS_CAP] = [0; CSS_CAP];
static mut CSS_LEN: usize = 0;

// Scratch buffer to fetch one <img>'s bytes into before decoding.
const IMG_FETCH_CAP: usize = 6 * 1024 * 1024;
const MAX_IMAGES: usize = 16;
static mut IMG_FETCH_BUF: [u8; IMG_FETCH_CAP] = [0; IMG_FETCH_CAP];
static mut IMAGES_DIRTY: bool = false;

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
fn css_str() -> &'static str {
    unsafe {
        let len = core::ptr::addr_of!(CSS_LEN).read();
        let ptr = core::ptr::addr_of!(CSS_BUF) as *const u8;
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).unwrap_or("")
    }
}

// Reader-mode toggle: apply the site's own (external + <style>) CSS, or render
// with just our UA sheet (BROWSER.md §9.7 — never worse than clean content).
static mut USE_SITE_CSS: bool = true;
fn use_site_css() -> bool {
    unsafe { core::ptr::addr_of!(USE_SITE_CSS).read() }
}
fn toggle_site_css() {
    unsafe { core::ptr::addr_of_mut!(USE_SITE_CSS).write(!use_site_css()) };
}
/// Lay out the current page honoring the reader-mode toggle: full site CSS
/// (external `<link>` + inline `<style>`) when on, UA-only when off.
fn do_layout(engine: &Engine, w: u32) -> Layout {
    if use_site_css() {
        engine.layout_ext(html_str(), css_str(), w)
    } else {
        engine.layout_ua(html_str(), w)
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

// Content generation — bumped on every fetch so the layout cache knows to
// re-lay-out (vs. reusing it for scroll, which keeps scrolling smooth).
static mut CONTENT_GEN: u32 = 0;
fn bump_content_gen() {
    unsafe {
        let p = core::ptr::addr_of_mut!(CONTENT_GEN);
        p.write(p.read().wrapping_add(1));
    }
}
fn content_gen() -> u32 {
    unsafe { core::ptr::addr_of!(CONTENT_GEN).read() }
}

/// Fetch `url` into HTML_BUF, then its linked stylesheets; resets scroll +
/// marks dirty.
fn fetch(url: &str) -> bool {
    let dst = core::ptr::addr_of_mut!(HTML_BUF) as *mut u8;
    let n = unsafe { npk_http_request(url.as_ptr() as i32, url.len() as i32, dst as i32, HTML_CAP as i32) };
    let len = if n < 0 { 0 } else { n as usize };
    unsafe { core::ptr::addr_of_mut!(HTML_LEN).write(len) };
    set_scroll(0);
    mark_dirty();
    bump_content_gen();
    if len > 0 {
        fetch_stylesheets(url);
        unsafe { core::ptr::addr_of_mut!(IMAGES_DIRTY).write(true) };
    } else {
        unsafe { core::ptr::addr_of_mut!(CSS_LEN).write(0) };
    }
    n >= 0
}

/// Fetch the page's `<img>` sources and hand them to the engine to decode.
/// Runs in the main loop (needs `&mut engine`); bounded by MAX_IMAGES.
///
/// STREAMING: fetch one image → decode it now → keep only its pixels → reuse
/// the same scratch buffer for the next. We never build a Vec of all the
/// compressed image bytes (the old `pairs` approach peaked at ~16 blobs at
/// once → the heap-OOM the fast keep-alive pool exposed).
fn refresh_images(engine: &mut Engine) {
    unsafe { core::ptr::addr_of_mut!(IMAGES_DIRTY).write(false) };
    let srcs = beak_engine::image_srcs(html_str());
    engine.images_begin();
    if !srcs.is_empty() {
        let base = url_str().to_string();
        let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
        for src in srcs.iter().take(MAX_IMAGES) {
            let abs = resolve(&base, src);
            let n = unsafe {
                npk_http_request(abs.as_ptr() as i32, abs.len() as i32, dst as i32, IMG_FETCH_CAP as i32)
            };
            if n > 0 {
                let bytes = unsafe { core::slice::from_raw_parts(dst as *const u8, n as usize) };
                engine.add_image(src, bytes); // decode now, drop compressed
            }
        }
    }
    bump_content_gen(); // re-layout so decoded images replace placeholders
    mark_dirty();
}

fn images_dirty() -> bool {
    unsafe { core::ptr::addr_of!(IMAGES_DIRTY).read() }
}

/// Fetch every `<link rel=stylesheet>` of the just-loaded page into CSS_BUF
/// (concatenated), resolving hrefs against `base`. Bounded by CSS_CAP +
/// MAX_CSS_LINKS. Each is a blocking sub-resource request (adds latency).
fn fetch_stylesheets(base: &str) {
    let links = beak_engine::stylesheet_links(html_str());
    let base = base.to_string();
    let dst = core::ptr::addr_of_mut!(CSS_BUF) as *mut u8;
    let mut len = 0usize;
    for (i, href) in links.iter().enumerate() {
        if i >= MAX_CSS_LINKS || len + 8192 >= CSS_CAP {
            break;
        }
        let abs = resolve(&base, href);
        let remaining = (CSS_CAP - len - 1) as i32;
        let n = unsafe {
            npk_http_request(abs.as_ptr() as i32, abs.len() as i32, dst.add(len) as i32, remaining)
        };
        if n > 0 {
            len += n as usize;
            unsafe { *dst.add(len) = b'\n' };
            len += 1;
        }
    }
    unsafe { core::ptr::addr_of_mut!(CSS_LEN).write(len) };
}

/// Set the address + fetch, WITHOUT touching history (used by back/forward).
fn fetch_url(url: &str) {
    set_url(url);
    if !fetch(url) {
        log("[beak] fetch failed");
    }
}

/// Navigate the address bar's typed text (normalise scheme) — new entry.
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
    fetch_url(&abs);
    hist_push(&abs);
}

/// Follow a link href relative to the current page — new history entry.
fn follow(href: &str) {
    let base = url_str().to_string();
    let abs = resolve(&base, href);
    fetch_url(&abs);
    hist_push(&abs);
}

// ── Back/forward history (fixed-size static ring of URLs) ──────────────────

const HIST_MAX: usize = 64;
static mut HIST: [[u8; URL_CAP]; HIST_MAX] = [[0; URL_CAP]; HIST_MAX];
static mut HIST_LEN: [usize; HIST_MAX] = [0; HIST_MAX];
static mut HIST_COUNT: usize = 0;
static mut HIST_POS: usize = 0;

fn hist_get(i: usize) -> &'static str {
    unsafe {
        let slot = (core::ptr::addr_of!(HIST) as *const [u8; URL_CAP]).add(i) as *const u8;
        let len = (core::ptr::addr_of!(HIST_LEN) as *const usize).add(i).read();
        core::str::from_utf8(core::slice::from_raw_parts(slot, len)).unwrap_or("")
    }
}
fn hist_set(i: usize, url: &str) {
    let n = url.len().min(URL_CAP);
    unsafe {
        let slot = (core::ptr::addr_of_mut!(HIST) as *mut [u8; URL_CAP]).add(i) as *mut u8;
        core::ptr::copy_nonoverlapping(url.as_ptr(), slot, n);
        (core::ptr::addr_of_mut!(HIST_LEN) as *mut usize).add(i).write(n);
    }
}
/// Record a new navigation: truncate forward entries, append (caps at HIST_MAX).
fn hist_push(url: &str) {
    unsafe {
        let count = core::ptr::addr_of!(HIST_COUNT).read();
        let pos = core::ptr::addr_of!(HIST_POS).read();
        if count > 0 && hist_get(pos) == url {
            return;
        }
        let new_pos = if count == 0 { 0 } else { pos + 1 };
        if new_pos >= HIST_MAX {
            hist_set(HIST_MAX - 1, url);
            return;
        }
        hist_set(new_pos, url);
        core::ptr::addr_of_mut!(HIST_POS).write(new_pos);
        core::ptr::addr_of_mut!(HIST_COUNT).write(new_pos + 1);
    }
}
fn hist_back() -> Option<&'static str> {
    unsafe {
        let pos = core::ptr::addr_of!(HIST_POS).read();
        if pos > 0 {
            core::ptr::addr_of_mut!(HIST_POS).write(pos - 1);
            Some(hist_get(pos - 1))
        } else {
            None
        }
    }
}
fn hist_forward() -> Option<&'static str> {
    unsafe {
        let pos = core::ptr::addr_of!(HIST_POS).read();
        let count = core::ptr::addr_of!(HIST_COUNT).read();
        if pos + 1 < count {
            core::ptr::addr_of_mut!(HIST_POS).write(pos + 1);
            Some(hist_get(pos + 1))
        } else {
            None
        }
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
fn maybe_repaint(engine: &Engine, cache: &mut Option<(Layout, i32, u32)>, buf: &mut Vec<u8>) {
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

    // (Re)lay out only when the content or width changed — NOT on every scroll.
    // Reusing the cached layout for scroll is what keeps scrolling smooth.
    let cur_gen = content_gen();
    let need_layout = match cache.as_ref() {
        None => true,
        Some((_, cw, cg)) => *cw != w || *cg != cur_gen,
    };
    if need_layout {
        *cache = Some((do_layout(engine, w as u32), w, cur_gen));
    }
    let layout = &cache.as_ref().unwrap().0;

    let max_scroll = (layout.height as i32 - h).max(0);
    let sy = scroll_y().clamp(0, max_scroll);
    set_scroll(sy);

    // Reuse a persistent paint buffer across frames — `engine.paint` fills every
    // pixel (background first), so no re-zeroing is needed. A fresh
    // `vec![0; w*h*4]` per frame was a ~5 MB alloc+zero+free on EVERY scroll
    // repaint (heap churn + latency).
    let need = (w as usize) * (h as usize) * 4;
    if buf.len() != need {
        buf.resize(need, 0);
    }
    engine.paint(layout, w as u32, h as u32, sy, buf);
    unsafe { npk_canvas_commit(CANVAS_ID, buf.as_ptr() as i32, buf.len() as i32, w, h) };

    unsafe {
        core::ptr::addr_of_mut!(LAST_W).write(w);
        core::ptr::addr_of_mut!(LAST_H).write(h);
        core::ptr::addr_of_mut!(DIRTY).write(false);
    }
}

/// Commit the loft-styled chrome: menu bar · toolbar (back/forward/reload +
/// framed address bar) · canvas body · the open dropdown as a Popover.
fn render_chrome() {
    let menu = prefab::menu_bar_with_anchors(
        &[
            ("Datei".to_string(), ActionId(ACT_MENU_FILE)),
            ("Bearbeiten".to_string(), ActionId(ACT_MENU_EDIT)),
            ("Ansicht".to_string(), ActionId(ACT_MENU_VIEW)),
            ("Hilfe".to_string(), ActionId(ACT_MENU_HELP)),
        ],
        &[
            NodeId(NODE_MENU_FILE),
            NodeId(NODE_MENU_EDIT),
            NodeId(NODE_MENU_VIEW),
            NodeId(NODE_MENU_HELP),
        ],
    );

    // loft's framed-input idiom: icon prefix + Input, SurfaceMuted fill, a
    // visible Border stroke, Focus→Accent — grown to fill the toolbar.
    let address = Widget::Row {
        children: vec![
            Widget::Icon {
                id: IconId::Bird,
                size: 20,
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
            prefab::icon_button(IconId::ArrowLeft, 22, Some(ActionId(ACT_BACK)), None),
            prefab::icon_button(IconId::ArrowRight, 22, Some(ActionId(ACT_FORWARD)), None),
            prefab::icon_button(IconId::ArrowClockwise, 22, Some(ActionId(ACT_RELOAD)), None),
            address,
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: vec![Modifier::Padding(Padding::Sm.as_u16())],
    };

    let mut children = vec![
        menu,
        Widget::Divider,
        toolbar,
        Widget::Divider,
        Widget::Canvas {
            id: CanvasId(CANVAS_ID as u32),
            width: 800,
            height: 600,
            modifiers: vec![Modifier::Flex(1), Modifier::Background(Token::Surface)],
        },
    ];

    if let Some((anchor, content)) = dropdown_for(open_menu()) {
        children.push(Widget::Popover {
            anchor: NodeId(anchor),
            child: Box::new(content),
            on_dismiss: ActionId(ACT_MENU_DISMISS),
            modifiers: vec![],
        });
    }

    let tree = Widget::Column {
        children,
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

/// Dropdown content for the open menu code (1=File .. 4=Help) → (anchor, menu).
fn dropdown_for(which: u8) -> Option<(u32, Widget)> {
    match which {
        1 => Some((
            NODE_MENU_FILE,
            prefab::popover_menu(&[("Schließen".to_string(), ActionId(ACT_FILE_CLOSE))], None),
        )),
        2 => Some((
            NODE_MENU_EDIT,
            prefab::popover_menu(&[("(noch nichts)".to_string(), ActionId(ACT_MENU_DISMISS))], None),
        )),
        3 => Some((
            NODE_MENU_VIEW,
            prefab::popover_menu(
                &[
                    ("Neu laden".to_string(), ActionId(ACT_VIEW_RELOAD)),
                    (
                        if use_site_css() { "Site-CSS: an".to_string() } else { "Site-CSS: aus".to_string() },
                        ActionId(ACT_VIEW_TOGGLE_CSS),
                    ),
                ],
                None,
            ),
        )),
        4 => Some((
            NODE_MENU_HELP,
            prefab::popover_menu(&[("Über beak".to_string(), ActionId(ACT_HELP_ABOUT))], None),
        )),
        _ => None,
    }
}

/// Handle one event. Returns true if the chrome (address bar / title) should
/// be re-committed.
fn handle(engine: &Engine, ev: Event, cache: &Option<(Layout, i32, u32)>) -> bool {
    match ev {
        // Keep URL_BUF synced with the address-bar edit buffer.
        Event::InputChange { value } => {
            set_url(&value);
            false
        }
        Event::Action(ActionId(id)) => match id {
            ACT_GO => {
                let t = url_str().to_string();
                go(&t);
                set_open_menu(0);
                true
            }
            ACT_RELOAD | ACT_VIEW_RELOAD => {
                let t = url_str().to_string();
                if !t.is_empty() {
                    fetch_url(&t);
                }
                set_open_menu(0);
                true
            }
            ACT_VIEW_TOGGLE_CSS => {
                toggle_site_css();
                bump_content_gen(); // force re-layout with/without site CSS
                mark_dirty();
                set_open_menu(0);
                true
            }
            ACT_BACK => {
                if let Some(u) = hist_back() {
                    fetch_url(u);
                }
                set_open_menu(0);
                true
            }
            ACT_FORWARD => {
                if let Some(u) = hist_forward() {
                    fetch_url(u);
                }
                set_open_menu(0);
                true
            }
            ACT_MENU_FILE => {
                toggle_menu(1);
                true
            }
            ACT_MENU_EDIT => {
                toggle_menu(2);
                true
            }
            ACT_MENU_VIEW => {
                toggle_menu(3);
                true
            }
            ACT_MENU_HELP => {
                toggle_menu(4);
                true
            }
            ACT_MENU_DISMISS | ACT_HELP_ABOUT => {
                set_open_menu(0);
                true
            }
            ACT_FILE_CLOSE => {
                unsafe {
                    let _ = npk_close_widget();
                }
                true
            }
            _ => false,
        },
        // Link clicks land in the canvas → hit-test the engine's link rects.
        // IMPORTANT: only clicks INSIDE the canvas are ours. Menu-bar/toolbar
        // clicks are delivered here too (as a MouseButton alongside their
        // Action); touching the open menu on those would close the dropdown the
        // very same click just opened (it "flashed open then shut"). Those are
        // handled entirely by their Action / the Popover's on_dismiss.
        Event::MouseButton { button: MouseButton::Left, down: true, x, y } => {
            if let Some((rx, ry, w, h)) = canvas_rect() {
                if x >= rx && x < rx + w && y >= ry && y < ry + h {
                    // A page click with a menu open just dismisses it (no nav).
                    if open_menu() != 0 {
                        set_open_menu(0);
                        return true;
                    }
                    let cx = x - rx;
                    let cy = y - ry + scroll_y();
                    // Hit-test the cached layout if it still matches; else lay
                    // out on the fly (rare — only if a click races a resize).
                    let href = match cache.as_ref() {
                        Some((lay, cw, cg)) if *cw == w && *cg == content_gen() => {
                            lay.hit_test(cx, cy).map(|s| s.to_string())
                        }
                        _ => do_layout(engine, w as u32).hit_test(cx, cy).map(|s| s.to_string()),
                    };
                    if let Some(href) = href {
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

// ── Heap: a real free-list allocator. The six font faces (persistent) + each
//    frame's layout + paint buffer are freed on drop, unlike a bump heap. ───

// 128 MB: 64 MB was too tight for one heavy page — the image budget alone is
// 24 MB, the six faces parse to ~6 MB, plus paint buffer + layout + glyph cache
// + transient DOM/CSS peaked over the cap (OOM on real sites). wasmi backs the
// linear memory with one contiguous alloc, so 128 MB is the safe headroom step;
// go higher only if a specific page still trips it.
const HEAP_SIZE: usize = 128 * 1024 * 1024;
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
    // Trap — do NOT `loop {}`. A wasm `unreachable` makes `_start`'s host call
    // return Err, so the kernel tears this instance down and frees its worker
    // core. A busy loop would instead pin the core forever (fibers are
    // cooperative → a spinning fiber never yields) = the "app panic freezes the
    // machine" bug. Cleanly dying is the whole point of the per-tab sandbox.
    core::arch::wasm32::unreachable()
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Init the heap FIRST — before any allocation.
    unsafe {
        ALLOCATOR.lock().init(core::ptr::addr_of_mut!(HEAP) as *mut u8, HEAP_SIZE);
    }

    // Launch argument: `npk_open("beak", "https://…")` → prime the address bar
    // now; the actual fetch waits until the font is parsed (below).
    let arg_len = {
        let p = core::ptr::addr_of_mut!(PAYLOAD_BUF) as *mut u8;
        let n = unsafe { npk_launch_arg(p as i32, PAYLOAD_CAP as i32) };
        if n > 0 { n as usize } else { 0 }
    };
    if arg_len > 0 {
        set_url(payload_str(arg_len));
    }

    // Commit the chrome IMMEDIATELY so the window is an opaque browser from the
    // first frame. Parsing the 880 KB font (below) is slow enough to otherwise
    // leave the window empty/transparent for a beat (the "loop window" look)
    // and makes beak feel slower to start than font-free apps (spell/loft).
    render_chrome();

    log("[beak] parsing font…");
    let mut engine = Engine::new();
    engine.set_theme(query_theme());
    log("[beak] engine ready");

    // Engine is up — fetch the launch URL now (if we were opened with one).
    if arg_len > 0 {
        let u = url_str().to_string();
        go(&u);
    }

    // Cached layout: (Layout, width it was laid out at, content generation).
    let mut cache: Option<(Layout, i32, u32)> = None;
    let mut last_bg = unsafe { npk_theme_token(0) };
    // Persistent paint buffer, reused across frames (see maybe_repaint).
    let mut paint_buf: Vec<u8> = Vec::new();
    loop {
        // A fresh page: fetch its images (blocking) and hand them to the engine
        // before painting, so decoded images replace placeholders in one pass.
        if images_dirty() {
            refresh_images(&mut engine);
        }
        // Drain the ENTIRE event queue this tick, THEN repaint once. Wheel
        // events used to be handled one-per-loop with a full repaint (and a
        // ~5 MB buffer alloc) each — a burst of scroll notches backed up so the
        // page scrolled slowly and "kept going" after the wheel stopped, AND
        // the loop never reached the idle sleep, so the worker core span at
        // 100% (never halting). Coalescing collapses the burst into one scroll
        // step + one repaint.
        let mut chrome = false;
        let mut had_event = false;
        loop {
            match poll_event() {
                PollResult::Event(ev) => {
                    had_event = true;
                    if handle(&engine, ev, &cache) {
                        chrome = true;
                    }
                }
                PollResult::Empty => break,
                PollResult::WindowGone => {
                    unsafe {
                        let _ = npk_close_widget();
                    }
                    return;
                }
            }
        }
        if chrome {
            render_chrome();
        }
        if !had_event {
            // Idle: pick up a runtime theme switch (light ↔ dark) cheaply — one
            // token read per idle frame; on change, re-resolve the palette and
            // invalidate the layout so text colours update too.
            let bg = unsafe { npk_theme_token(0) };
            if bg != last_bg {
                engine.set_theme(query_theme());
                last_bg = bg;
                bump_content_gen();
                mark_dirty();
            }
        }
        maybe_repaint(&engine, &mut cache, &mut paint_buf);
        // ALWAYS yield so this worker core can halt — a cooperative fiber that
        // never sleeps pins its core at 100%. A short nap while interacting
        // stays responsive; a longer one when idle keeps the core asleep.
        unsafe {
            let _ = npk_sleep(if had_event { 4 } else { 16 });
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<app_meta::IconRef> {
    None
}
