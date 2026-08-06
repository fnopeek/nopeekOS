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

use beak_engine::forms::{self, ControlKind, FormState, Forms};
use beak_engine::{Engine, Layout};
use nopeek_widgets::i18n;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::{caps, prefab};
use nopeek_widgets::*;

// ── Strings ───────────────────────────────────────────────────────────
// English is the source language; a new one is one more `const` below.
// See `nopeek_widgets::i18n`.

struct Strings {
    menu_file:           &'static str,
    menu_edit:           &'static str,
    menu_view:           &'static str,
    menu_help:           &'static str,
    close:               &'static str,
    nothing_yet:         &'static str,
    reload:              &'static str,
    css_on:              &'static str,
    css_off:             &'static str,
    inspect_on:          &'static str,
    inspect_off:         &'static str,
    inspect_hint:        &'static str,
    about:               &'static str,
    address_placeholder: &'static str,
}

const EN: Strings = Strings {
    menu_file: "File", menu_edit: "Edit", menu_view: "View", menu_help: "Help",
    close: "Close",
    nothing_yet: "(nothing yet)",
    reload: "Reload",
    css_on: "Site CSS: on", css_off: "Site CSS: off",
    inspect_on: "Inspect: on", inspect_off: "Inspect: off",
    inspect_hint: "Inspect: click an element in the page",
    about: "About beak",
    address_placeholder: "Enter address …",
};

const DE: Strings = Strings {
    menu_file: "Datei", menu_edit: "Bearbeiten", menu_view: "Ansicht",
    menu_help: "Hilfe",
    close: "Schließen",
    nothing_yet: "(noch nichts)",
    reload: "Neu laden",
    css_on: "Site-CSS: an", css_off: "Site-CSS: aus",
    inspect_on: "Inspizieren: an", inspect_off: "Inspizieren: aus",
    inspect_hint: "Inspizieren: klicke ein Element im Seiteninhalt",
    about: "Über beak",
    address_placeholder: "Adresse eingeben …",
};

fn s() -> &'static Strings {
    match i18n::lang() { i18n::Lang::De => &DE, _ => &EN }
}
use talc::{ClaimOnOom, Span, Talc, Talck};

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

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
    fn npk_http_request(url_ptr: i32, url_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_http_final_url(buf_ptr: i32, buf_max: i32) -> i32;
    /// Fetch a newline-separated list of URLs in one call, multiplexed over
    /// HTTP/2 where the host offers it. Bodies land back-to-back in `out`;
    /// `lens` receives one little-endian i32 per URL (bytes written, or -1).
    fn npk_http_request_many(
        urls_ptr: i32,
        urls_len: i32,
        out_ptr: i32,
        out_max: i32,
        lens_ptr: i32,
        lens_max: i32,
    ) -> i32;
    fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32) -> i32;
    fn npk_canvas_rect(canvas_id: i32, out_ptr: i32) -> i32;
    fn npk_theme_token(token_id: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
    /// Milliseconds since boot, 10 ms resolution (the 100 Hz timer).
    fn npk_ticks() -> i64;
}

/// Milliseconds since boot. Used only for the phase timings below.
fn now_ms() -> i64 {
    unsafe { npk_ticks() }
}

/// Log "<label>: <ms> ms". Phase timings are permanent, not scaffolding:
/// on this hardware the engine runs under a WASM interpreter, so knowing
/// which phase a page load actually spends its time in is the difference
/// between fixing the slow thing and rewriting the fast one.
fn log_ms(label: &str, ms: i64) {
    let mut b = String::new();
    b.push_str("[beak] ");
    b.push_str(label);
    b.push_str(": ");
    push_i64(&mut b, ms);
    b.push_str(" ms");
    log(&b);
}

fn push_i64(out: &mut String, mut v: i64) {
    if v < 0 { out.push('-'); v = -v; }
    let mut d = [0u8; 20];
    let mut n = 0;
    loop {
        d[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
        if v == 0 { break; }
    }
    while n > 0 { n -= 1; out.push(d[n] as char); }
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
const ACT_VIEW_INSPECT: u32 = 6_022;
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
// Both numbers are measured against real pages, not guessed: GitHub links 37
// sheets totalling 4.4 MiB, MDN links 17 that come to 71 KiB, SRF 5 / 367 KiB,
// Wikipedia 3 / 273 KiB. The old 16-link cap therefore broke MDN at 3 % of the
// byte budget — the count was the wrong unit, the same mistake MAX_IMAGES made.
// A dropped stylesheet is not a missing icon, it is a broken page, so the
// headroom is deliberate. The buffer is `.bss`: it costs runtime memory only,
// nothing in the shipped .wasm.
const CSS_CAP: usize = 8 * 1024 * 1024;
const MAX_CSS_LINKS: usize = 64;
static mut CSS_BUF: [u8; CSS_CAP] = [0; CSS_CAP];
static mut CSS_LEN: usize = 0;

// Scratch buffer to fetch one <img>'s bytes into before decoding.
const IMG_FETCH_CAP: usize = 6 * 1024 * 1024;
/// A REQUEST backstop, not a memory bound. Memory is bounded one layer down,
/// where it can be measured: the engine keeps a per-page budget of decoded
/// BGRA and refuses anything over it, plus a per-image pixel cap. Counting
/// images here as well was the cruder of the two caps and the one that bit —
/// de.wikipedia/Stansstad has 20 distinct sources whose pixels come to a
/// couple of MB, so the byte budget never came near, and #17/#19/#20 (a navbox
/// coat of arms and both footer icons) silently kept their placeholders.
const MAX_IMAGES: usize = 64;
static mut IMG_FETCH_BUF: [u8; IMG_FETCH_CAP] = [0; IMG_FETCH_CAP];

/// How many images one batch asks for. Small on purpose: the batch is a
/// blocking call, so a whole page in one go would freeze the window again —
/// the very thing progressive loading fixed. Four is enough to overlap the
/// round-trips while a turn of the loop stays short.
const IMG_BATCH: usize = 4;

/// Receives the per-URL length table from `npk_http_request_many`. Sized for
/// the largest batch either caller asks for.
static mut LENS_BUF: [u8; 4 * MAX_CSS_LINKS] = [0; 4 * MAX_CSS_LINKS];

/// Fetch a batch of URLs into `dst`, returning each body as a slice.
///
/// Returns an empty vec if the host call fails, which the callers treat the
/// same as "none of them loaded" — every one of them degrades to a
/// placeholder or to unstyled content rather than to a blank page.
fn fetch_batch(urls: &[String], dst: *mut u8, cap: usize) -> Vec<(usize, usize)> {
    let mut blob = String::new();
    for (i, u) in urls.iter().enumerate() {
        if i > 0 {
            blob.push('\n');
        }
        blob.push_str(u);
    }
    let lens = core::ptr::addr_of_mut!(LENS_BUF) as *mut u8;
    let n = unsafe {
        npk_http_request_many(
            blob.as_ptr() as i32,
            blob.len() as i32,
            dst as i32,
            cap as i32,
            lens as i32,
            (4 * MAX_CSS_LINKS) as i32,
        )
    };
    let mut spans = Vec::new();
    if n <= 0 {
        return spans;
    }
    let mut off = 0usize;
    for i in 0..(n as usize).min(urls.len()) {
        let mut raw = [0u8; 4];
        unsafe { core::ptr::copy_nonoverlapping(lens.add(i * 4), raw.as_mut_ptr(), 4) };
        let len = i32::from_le_bytes(raw);
        if len < 0 {
            spans.push((0, 0)); // this one failed; keep positions aligned
        } else {
            spans.push((off, len as usize));
            off += len as usize;
        }
    }
    spans
}
static mut IMAGES_DIRTY: bool = false;

// Scratch for the kernel to write back the post-redirect URL of a fetch.
static mut FINAL_URL_BUF: [u8; URL_CAP] = [0; URL_CAP];

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

/// The current page's forms + the user's live edits to them. Rebuilt on every
/// navigation (keyed on NAV_GEN, NOT the layout's content generation — a theme
/// switch or an image arriving must not wipe what the user has typed).
struct Page {
    forms: Forms,
    state: FormState,
    nav: u32,
}

impl Page {
    fn new() -> Page {
        Page { forms: forms::Forms { forms: Vec::new(), controls: Vec::new() }, state: FormState::default(), nav: 0 }
    }
    /// Re-parse the document's forms if we have navigated since the last call.
    fn sync(&mut self) {
        let g = nav_gen();
        if self.nav == g {
            return;
        }
        self.nav = g;
        self.forms = forms::collect(&beak_engine::parse(html_str()));
        self.state.reset();
    }
    /// The focused control's current text + its kind.
    fn focused(&self) -> Option<(&forms::Control, &str)> {
        let seq = self.state.focus?;
        let c = self.forms.get(seq)?;
        Some((c, self.state.value(c)))
    }
}

// Navigation generation — bumped ONLY by a real page load.
static mut NAV_GEN: u32 = 0;
fn nav_gen() -> u32 {
    unsafe { core::ptr::addr_of!(NAV_GEN).read() }
}
fn bump_nav_gen() {
    unsafe {
        let p = core::ptr::addr_of_mut!(NAV_GEN);
        p.write(p.read().wrapping_add(1));
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

// Inspect dev tool: when on, the engine records an element box per node and a
// canvas click selects the deepest box under the cursor (outline + a label in
// the status bar) instead of following a link — so a mis-rendered element can
// be named on the device.
static mut INSPECT_MODE: bool = false;
fn inspect_mode() -> bool {
    unsafe { core::ptr::addr_of!(INSPECT_MODE).read() }
}
fn toggle_inspect() {
    unsafe {
        let p = core::ptr::addr_of_mut!(INSPECT_MODE);
        p.write(!p.read());
    }
}
/// The selected element: document-space `(x, y, w, h)` + its label.
static mut SEL_BOX: Option<(i32, i32, i32, i32, String)> = None;
fn set_selected(b: Option<(i32, i32, i32, i32, String)>) {
    unsafe { core::ptr::addr_of_mut!(SEL_BOX).write(b) };
}
fn selected_rect() -> Option<(i32, i32, i32, i32)> {
    unsafe { (*core::ptr::addr_of!(SEL_BOX)).as_ref().map(|(x, y, w, h, _)| (*x, *y, *w, *h)) }
}
fn selected_label() -> Option<String> {
    unsafe { (*core::ptr::addr_of!(SEL_BOX)).as_ref().map(|(_, _, _, _, l)| l.clone()) }
}
/// Lay out the current page honoring the reader-mode toggle: full site CSS
/// (external `<link>` + inline `<style>`) when on, UA-only when off.
fn do_layout(engine: &Engine, w: u32, state: &FormState) -> Layout {
    // The viewport height is the initial containing block's height — what
    // `top:0; bottom:0` on a root-level abspos box stretches to.
    if let Some((_, _, _, h)) = canvas_rect() {
        engine.set_viewport_h(h as u32);
    }
    engine.set_inspect(inspect_mode());
    let t0 = now_ms();
    let lay = if use_site_css() {
        engine.layout_forms(html_str(), css_str(), w, state)
    } else {
        engine.layout_ua_forms(html_str(), w, state)
    };
    log_ms("layout (parse+cascade+layout)", now_ms() - t0);
    lay
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
/// When the current navigation started, so the first paint after it can
/// report the ONE number a user actually feels: click → something on screen.
static mut NAV_START_MS: i64 = 0;
/// Cleared once that number has been reported for this navigation.
static mut NAV_REPORTED: bool = true;

static mut CONTENT_GEN: u32 = 0;
/// Invalidate the layout cache. `why` is logged because a full re-layout is
/// the single most expensive thing this app does (~4.7 s on device), so an
/// unexpected one has to be attributable at a glance.
fn bump_content_gen(why: &str) {
    unsafe {
        let p = core::ptr::addr_of_mut!(CONTENT_GEN);
        p.write(p.read().wrapping_add(1));
    }
    let mut b = String::new();
    b.push_str("[beak] relayout: ");
    b.push_str(why);
    log(&b);
}
fn content_gen() -> u32 {
    unsafe { core::ptr::addr_of!(CONTENT_GEN).read() }
}

/// The URL the last fetch's body actually came from, after redirects.
/// `None` if the kernel reported none (request failed, or an older kernel).
fn fetched_from() -> Option<String> {
    let dst = core::ptr::addr_of_mut!(FINAL_URL_BUF) as *mut u8;
    let n = unsafe { npk_http_final_url(dst as i32, URL_CAP as i32) };
    if n <= 0 {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(dst as *const u8, n as usize) };
    core::str::from_utf8(bytes).ok().map(|s| s.to_string())
}

/// Fetch `url` into HTML_BUF, then its linked stylesheets; resets scroll +
/// marks dirty.
fn fetch(url: &str) -> bool {
    let t_nav = now_ms();
    unsafe {
        core::ptr::addr_of_mut!(NAV_START_MS).write(t_nav);
        core::ptr::addr_of_mut!(NAV_REPORTED).write(false);
    }
    let dst = core::ptr::addr_of_mut!(HTML_BUF) as *mut u8;
    let n = unsafe { npk_http_request(url.as_ptr() as i32, url.len() as i32, dst as i32, HTML_CAP as i32) };
    log_ms("fetch document", now_ms() - t_nav);
    let len = if n < 0 { 0 } else { n as usize };
    unsafe { core::ptr::addr_of_mut!(HTML_LEN).write(len) };
    set_scroll(0);
    mark_dirty();
    bump_content_gen("navigation");
    bump_nav_gen();
    if len > 0 {
        // Relative sub-resources resolve against the URL the document came
        // FROM, not the one we asked for (RFC 3986 §5.1.3). Getting this
        // wrong made every stylesheet and image repeat the document's own
        // redirect — two requests each, which is what walked us into
        // Wikimedia's rate limit (a wall of HTTP 429).
        let base = fetched_from().unwrap_or_else(|| url.to_string());
        set_url(&base);
        let t_css = now_ms();
        fetch_stylesheets(&base);
        log_ms("fetch stylesheets", now_ms() - t_css);
        unsafe { core::ptr::addr_of_mut!(IMAGES_DIRTY).write(true) };
    } else {
        unsafe { core::ptr::addr_of_mut!(CSS_LEN).write(0) };
    }
    n >= 0
}

/// Start a page's image load: drop the old pixels and return the list of
/// sources still to fetch. Touches the network NOT AT ALL, so the first paint
/// can happen right after it.
///
/// The same src repeats all over a real page (icons, bullets, a logo in header
/// and footer). The engine keys decoded images by src, so a repeat only
/// re-fetched and re-decoded identical bytes — wasted requests against the
/// server's rate limit, and wasted MAX_IMAGES slots that real images needed.
fn begin_images(engine: &mut Engine) -> Vec<String> {
    unsafe { core::ptr::addr_of_mut!(IMAGES_DIRTY).write(false) };
    engine.images_begin();
    let mut pending: Vec<String> = Vec::new();
    let all = beak_engine::image_srcs(html_str());
    for src in all.iter() {
        if pending.len() >= MAX_IMAGES {
            log(&alloc::format!("[beak] image cap hit: {} of {} sources fetched", MAX_IMAGES, all.len()));
            break;
        }
        if !pending.iter().any(|s| s == src) {
            pending.push(src.clone());
        }
    }
    bump_content_gen("images-begin"); // lay out with placeholders
    mark_dirty();
    pending
}

/// Fetch exactly ONE pending image and hand it to the engine to decode.
///
/// One per main-loop turn, NOT a batch: images are not render-blocking (only
/// stylesheets are), so the page must already be on screen and scrollable
/// while they trickle in. Fetches are blocking, and a rate-limited host can
/// stall one for seconds — batching them froze the whole app until the last
/// one landed.
///
/// STREAMING: fetch → decode now → keep only its pixels → reuse the same
/// scratch buffer for the next. We never hold all the compressed image bytes
/// at once (the old `pairs` approach peaked at ~16 blobs → the heap-OOM the
/// fast keep-alive pool exposed).
/// `guessed` lists the `src`s whose box the last layout had to guess. Only if
/// one of THOSE arrives does the page move and a re-layout pay for itself;
/// everything else is a repaint. That is ~15 ms instead of ~145 ms of engine
/// work per batch on a real article — and under the wasmi interpreter on the
/// device, the difference between a page that scrolls while it loads and one
/// that freezes for seconds at a time.
fn fetch_next_images(engine: &mut Engine, pending: &mut Vec<String>, guessed: &[String]) {
    if pending.is_empty() {
        return;
    }
    let take = pending.len().min(IMG_BATCH);
    let srcs: Vec<String> = pending.drain(..take).collect();
    let urls: Vec<String> = srcs.iter().map(|s| resolve(url_str(), s)).collect();
    let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
    let spans = fetch_batch(&urls, dst, IMG_FETCH_CAP);
    let mut any = false;
    let mut moved = false;
    for (src, (off, n)) in srcs.iter().zip(spans) {
        if n == 0 {
            continue; // failed or did not fit → keeps its placeholder
        }
        let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };
        engine.add_image(src, bytes); // decode now, drop compressed
        any = true;
        if guessed.iter().any(|g| g == src) {
            moved = true;
        }
    }
    if any {
        if moved {
            bump_content_gen("image-arrived"); // a guessed box moves once the real size lands
        }
        mark_dirty(); // otherwise just paint: the display list is unchanged
    }
}

fn images_dirty() -> bool {
    unsafe { core::ptr::addr_of!(IMAGES_DIRTY).read() }
}

/// Fetch the CSS images (`background-image`/`mask-image`) the last layout
/// asked for, one batch a turn.
///
/// Kept apart from `fetch_next_images` for one reason that matters: a CSS
/// image can never move a box, so an arriving one is ALWAYS just a repaint —
/// there is no `guessed` case and no `bump_content_gen`. The engine already
/// resolved every `data:` URI itself, so this list is only what genuinely
/// needs the network.
///
/// The URL is resolved against the DOCUMENT, not the stylesheet that declared
/// it. Those differ only for a relative url() in a linked sheet; the shell
/// concatenates the sheets into one buffer, so the per-sheet base is gone by
/// here. Absolute and root-relative urls — which is what real sheets ship —
/// resolve identically either way.
fn fetch_next_css_images(engine: &Engine, pending: &mut Vec<(u64, String)>) {
    if pending.is_empty() {
        return;
    }
    let take = pending.len().min(IMG_BATCH);
    let want: Vec<(u64, String)> = pending.drain(..take).collect();
    let urls: Vec<String> = want.iter().map(|(_, u)| resolve(url_str(), u)).collect();
    let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
    let spans = fetch_batch(&urls, dst, IMG_FETCH_CAP);
    let mut any = false;
    for ((key, _), (off, n)) in want.iter().zip(spans) {
        if n == 0 {
            continue; // failed or did not fit → the box stays undecorated
        }
        let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };
        any |= engine.add_css_image(*key, bytes);
    }
    if any {
        mark_dirty();
    }
}

/// Fetch every `<link rel=stylesheet>` of the just-loaded page into CSS_BUF
/// (concatenated), resolving hrefs against `base`. Bounded by CSS_CAP +
/// MAX_CSS_LINKS. Each is a blocking sub-resource request (adds latency).
fn fetch_stylesheets(base: &str) {
    let links = beak_engine::stylesheet_links(html_str());
    let base = base.to_string();
    let mut urls: Vec<String> = Vec::new();
    for href in links.iter() {
        if urls.len() >= MAX_CSS_LINKS {
            // Say so. A silently dropped stylesheet looks like a layout bug
            // and sends the next session hunting in the engine.
            log(&alloc::format!("[beak] stylesheet cap hit: {} of {} linked sheets used", MAX_CSS_LINKS, links.len()));
            break;
        }
        let abs = resolve(&base, href);
        // The same sheet linked twice fetches identical bytes; dedupe on the
        // resolved URL, since two different hrefs can resolve to one file.
        if !urls.contains(&abs) {
            urls.push(abs);
        }
    }
    let mut len = 0usize;
    if !urls.is_empty() {
        // One batch, not one request per sheet. Stylesheets are
        // render-blocking — nothing paints until the last one arrives — so
        // this is where overlapping the round-trips is worth the most.
        // Fetched into a scratch buffer first because the bodies come back
        // concatenated, and they need a separator between them: without one,
        // a sheet not ending in `}` would merge into the next sheet's first
        // rule.
        let mut scratch: Vec<u8> = Vec::with_capacity(CSS_CAP);
        let spans = fetch_batch(&urls, scratch.as_mut_ptr(), CSS_CAP);
        let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
        unsafe { scratch.set_len(total.min(CSS_CAP)) };

        let dst = core::ptr::addr_of_mut!(CSS_BUF) as *mut u8;
        for (off, n) in spans {
            if n == 0 || off + n > scratch.len() || len + n + 1 >= CSS_CAP {
                if n > 0 && len + n + 1 >= CSS_CAP {
                    log(&alloc::format!("[beak] CSS buffer full at {len} B — dropped a {n} B sheet"));
                }
                continue;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(scratch.as_ptr().add(off), dst.add(len), n);
                len += n;
                *dst.add(len) = b'\n';
            }
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
    } else if looks_like_url(t) {
        let mut s = String::from("https://");
        s.push_str(t);
        s
    } else {
        // Not an address → search the web for it (omnibox).
        let mut s = String::from(SEARCH_URL);
        let mut q = String::new();
        forms::encode_query_value(t, &mut q);
        s.push_str(&q);
        s
    };
    fetch_url(&abs);
    // Record where we LANDED, not where we aimed — otherwise every trip back
    // through history replays the redirect.
    hist_push(url_str());
}

/// A typed address that is not a URL becomes a web search. Marginalia is the
/// one engine that serves real results to a no-JS client (the others gate on
/// browser fingerprinting — see the 2026-07-20 recon).
const SEARCH_URL: &str = "https://marginalia-search.com/search?query=";

/// Does this look like an address rather than a search phrase?
fn looks_like_url(t: &str) -> bool {
    if t.starts_with("http://") || t.starts_with("https://") {
        return true;
    }
    if t.contains(' ') {
        return false;
    }
    // A dotted host (example.com, de.wikipedia.org/wiki/X) or localhost.
    let host = t.split(['/', '?', '#']).next().unwrap_or(t);
    host == "localhost" || (host.contains('.') && !host.ends_with('.'))
}

/// Submit a form: build `action?name=value&…` and navigate there. GET only —
/// a POST needs a request body, which `npk_http_request` cannot send yet.
fn submit_form(page: &Page, activated: Option<u32>) -> bool {
    let sub = match forms::submit(&page.forms, &page.state, activated) {
        Some(s) => s,
        None => return false,
    };
    if !sub.method_get {
        log("[beak] POST-Formulare noch nicht unterstützt");
        return false;
    }
    // An empty action targets the current document; either way the form data
    // REPLACES the action's query string (HTML §4.10.21.3 "mutate action URL").
    let base = url_str().to_string();
    let action = if sub.action.is_empty() { base.clone() } else { resolve(&base, &sub.action) };
    let mut url = action.split(['?', '#']).next().unwrap_or(&action).to_string();
    if !sub.query.is_empty() {
        url.push('?');
        url.push_str(&sub.query);
    }
    fetch_url(&url);
    hist_push(url_str());
    true
}

/// Follow a link href relative to the current page — new history entry.
fn follow(href: &str) {
    let base = url_str().to_string();
    let abs = resolve(&base, href);
    fetch_url(&abs);
    // Record where we LANDED, not where we aimed — otherwise every trip back
    // through history replays the redirect.
    hist_push(url_str());
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
/// Set one BGRA pixel (bounds-checked).
fn px_set(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, bgr: [u8; 3]) {
    if x < 0 || x >= w || y < 0 || y >= h {
        return;
    }
    let o = ((y * w + x) * 4) as usize;
    if o + 3 < buf.len() {
        buf[o] = bgr[0];
        buf[o + 1] = bgr[1];
        buf[o + 2] = bgr[2];
        buf[o + 3] = 255;
    }
}

/// Stroke a 2px rectangle border into a BGRA buffer — the inspect highlight.
fn stroke_rect_bgra(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, bgr: [u8; 3]) {
    if rw <= 0 || rh <= 0 {
        return;
    }
    let t = 2i32;
    for dy in 0..rh {
        for k in 0..t {
            px_set(buf, w, h, x + k, y + dy, bgr);
            px_set(buf, w, h, x + rw - 1 - k, y + dy, bgr);
        }
    }
    for dx in 0..rw {
        for k in 0..t {
            px_set(buf, w, h, x + dx, y + k, bgr);
            px_set(buf, w, h, x + dx, y + rh - 1 - k, bgr);
        }
    }
}

fn maybe_repaint(engine: &Engine, cache: &mut Option<(Layout, i32, u32)>, buf: &mut Vec<u8>, state: &FormState) {
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
        *cache = Some((do_layout(engine, w as u32, state), w, cur_gen));
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
    let t_paint = now_ms();
    engine.paint(layout, w as u32, h as u32, sy, buf);
    // Inspect overlay: outline the selected element box (document → screen).
    if inspect_mode() {
        if let Some((bx, by, bw, bh)) = selected_rect() {
            stroke_rect_bgra(buf, w, h, bx, by - sy, bw, bh, [0, 0, 255]);
        }
    }
    let t_commit = now_ms();
    unsafe { npk_canvas_commit(CANVAS_ID, buf.as_ptr() as i32, buf.len() as i32, w, h) };
    log_ms("paint", t_commit - t_paint);
    log_ms("canvas commit", now_ms() - t_commit);
    // The number that matters: navigation → first pixels.
    unsafe {
        if !core::ptr::addr_of!(NAV_REPORTED).read() {
            core::ptr::addr_of_mut!(NAV_REPORTED).write(true);
            log_ms("=== navigation -> first paint", now_ms() - core::ptr::addr_of!(NAV_START_MS).read());
        }
    }

    unsafe {
        core::ptr::addr_of_mut!(LAST_W).write(w);
        core::ptr::addr_of_mut!(LAST_H).write(h);
        core::ptr::addr_of_mut!(DIRTY).write(false);
    }
}

/// Commit the loft-styled chrome: menu bar · toolbar (back/forward/reload +
/// framed address bar) · canvas body · the open dropdown as a Popover.
fn render_chrome() {
    let menu = prefab::menu_bar_with_icon(
        IconId::Bird,
        &[
            (s().menu_file.to_string(), ActionId(ACT_MENU_FILE)),
            (s().menu_edit.to_string(), ActionId(ACT_MENU_EDIT)),
            (s().menu_view.to_string(), ActionId(ACT_MENU_VIEW)),
            (s().menu_help.to_string(), ActionId(ACT_MENU_HELP)),
        ],
        &[
            NodeId(NODE_MENU_FILE),
            NodeId(NODE_MENU_EDIT),
            NodeId(NODE_MENU_VIEW),
            NodeId(NODE_MENU_HELP),
        ],
    );

    // A lock in Success for https, the bird for anything else — the
    // scheme belongs in the field, not in the URL text (UI_REFRESH.md §5).
    let url = url_str();
    let (lead_icon, lead_tint) = if url.starts_with("https://") {
        (IconId::Lock, Token::Success)
    } else {
        (IconId::Bird, Token::Accent)
    };

    let address = Widget::Row {
        children: vec![
            Widget::Icon {
                id: lead_icon,
                size: 16,
                modifiers: vec![Modifier::Tint(lead_tint)],
            },
            Widget::Input {
                value: url.to_string(),
                placeholder: s().address_placeholder.to_string(),
                on_submit: ActionId(ACT_GO),
                modifiers: vec![Modifier::Flex(1)],
            },
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: vec![
            Modifier::Flex(1),
            Modifier::PaddingXY { x: 8, y: 0 },
            Modifier::MinHeight(FIELD_H),
            Modifier::Background(Token::SurfaceMuted),
            Modifier::Border { token: Token::Border, width: 1, radius: Radius::Md.as_u8() },
            Modifier::MinWidth(220),
            // 1 px accent border plus a 3 px ring — the design's
            // `text_field` focus state.
            Modifier::Focus(vec![
                Modifier::Border { token: Token::Accent, width: 1, radius: Radius::Md.as_u8() },
                Modifier::Ring { token: Token::AccentRing, width: 3 },
            ]),
        ],
    };

    let toolbar = Widget::Row {
        children: vec![
            nav_button(IconId::ArrowLeft, ActionId(ACT_BACK)),
            nav_button(IconId::ArrowRight, ActionId(ACT_FORWARD)),
            nav_button(IconId::ArrowClockwise, ActionId(ACT_RELOAD)),
            address,
        ],
        spacing: Spacing::Sm.as_u16(),
        align: Align::Center,
        modifiers: vec![
            Modifier::Padding(Padding::Sm.as_u16()),
            Modifier::MinHeight(TOOLBAR_H),
        ],
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
            modifiers: vec![Modifier::Flex(1), Modifier::Background(Token::Page)],
        },
    ];

    // Inspect status bar: the selected element's label, or a hint.
    if inspect_mode() {
        children.push(Widget::Divider);
        let label = selected_label().unwrap_or_else(|| s().inspect_hint.to_string());
        children.push(Widget::Text {
            content: label,
            style: TextStyle::Mono,
            modifiers: vec![
                Modifier::Padding(Padding::Sm.as_u16()),
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Tint(Token::OnSurface),
            ],
        });
    }

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

/// Toolbar chrome sizing (UI_REFRESH.md §5).
const TOOLBAR_H: u16 = 44;
const FIELD_H: u16 = 30;
const NAV_BTN: u16 = 28;
const NAV_BTN_RADIUS: u8 = 7;

/// Navigation button — the design's `toolbar_button`: bare at rest,
/// `SurfaceHover` fill under the cursor, accent tint while pressed.
fn nav_button(icon: IconId, action: ActionId) -> Widget {
    prefab::center_box(
        Widget::Icon { id: icon, size: 16, modifiers: vec![] },
        vec![
            Modifier::MinWidth(NAV_BTN),
            Modifier::MinHeight(NAV_BTN),
            Modifier::Rounded(NAV_BTN_RADIUS),
            Modifier::OnClick(action),
            Modifier::Hover(vec![
                Modifier::Background(Token::SurfaceHover),
                Modifier::Rounded(NAV_BTN_RADIUS),
            ]),
            Modifier::Active(vec![
                Modifier::Background(Token::AccentMuted),
                Modifier::Tint(Token::Accent),
                Modifier::Rounded(NAV_BTN_RADIUS),
            ]),
        ],
    )
}

/// Dropdown content for the open menu code (1=File .. 4=Help) → (anchor, menu).
fn dropdown_for(which: u8) -> Option<(u32, Widget)> {
    match which {
        1 => Some((
            NODE_MENU_FILE,
            prefab::popover_menu(&[(s().close.to_string(), ActionId(ACT_FILE_CLOSE))], None),
        )),
        2 => Some((
            NODE_MENU_EDIT,
            prefab::popover_menu(&[(s().nothing_yet.to_string(), ActionId(ACT_MENU_DISMISS))], None),
        )),
        3 => Some((
            NODE_MENU_VIEW,
            prefab::popover_menu(
                &[
                    (s().reload.to_string(), ActionId(ACT_VIEW_RELOAD)),
                    (
                        if use_site_css() { s().css_on.to_string() } else { s().css_off.to_string() },
                        ActionId(ACT_VIEW_TOGGLE_CSS),
                    ),
                    (
                        if inspect_mode() { s().inspect_on.to_string() } else { s().inspect_off.to_string() },
                        ActionId(ACT_VIEW_INSPECT),
                    ),
                ],
                None,
            ),
        )),
        4 => Some((
            NODE_MENU_HELP,
            prefab::popover_menu(&[(s().about.to_string(), ActionId(ACT_HELP_ABOUT))], None),
        )),
        _ => None,
    }
}

// ── in-page text editing (the compositor edits its own Input widgets; a
//    control painted into our canvas is ours to edit) ──────────────────────

fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i.min(s.len())].char_indices().next_back().map(|(j, _)| j).unwrap_or(0)
}
fn next_boundary(s: &str, i: usize) -> usize {
    let i = i.min(s.len());
    s[i..].chars().next().map(|c| i + c.len_utf8()).unwrap_or(i)
}

/// Apply one key to the focused control. Returns true if the page must be
/// re-laid-out (the control's painted text or caret changed).
fn edit_key(page: &mut Page, key: KeyCode) -> bool {
    let (seq, kind, mut value) = match page.focused() {
        Some((c, v)) => (c.seq, c.kind, v.to_string()),
        None => return false,
    };
    if !kind.is_text() {
        // Space / Enter activate a button or toggle a box, like a browser.
        return match key {
            KeyCode::Enter | KeyCode::Char(b' ') => {
                activate(page, seq);
                true
            }
            KeyCode::Escape => {
                page.state.focus = None;
                true
            }
            _ => false,
        };
    }
    let mut caret = page.state.caret.min(value.len());
    match key {
        KeyCode::Char(b) if (0x20..0x7F).contains(&b) => {
            value.insert(caret, b as char);
            caret += 1;
        }
        KeyCode::Backspace => {
            if caret == 0 {
                return false;
            }
            let p = prev_boundary(&value, caret);
            value.replace_range(p..caret, "");
            caret = p;
        }
        KeyCode::Delete => {
            if caret >= value.len() {
                return false;
            }
            let n = next_boundary(&value, caret);
            value.replace_range(caret..n, "");
        }
        KeyCode::Left => caret = prev_boundary(&value, caret),
        KeyCode::Right => caret = next_boundary(&value, caret),
        KeyCode::Home => caret = 0,
        KeyCode::End => caret = value.len(),
        KeyCode::Escape => {
            page.state.focus = None;
            return true;
        }
        KeyCode::Enter => {
            // Implicit submission (HTML §4.10.21.2): Enter in a text field
            // activates the form's default button.
            let activated = page
                .forms
                .get(seq)
                .and_then(|c| c.form)
                .and_then(|f| page.forms.default_button(f))
                .map(|b| b.seq);
            page.state.set_value(seq, value);
            page.state.caret = caret;
            submit_form(page, activated);
            return true;
        }
        _ => return false,
    }
    page.state.set_value(seq, value);
    page.state.caret = caret;
    true
}

/// Click / keyboard activation of a control: submit, toggle, or take focus.
fn activate(page: &mut Page, seq: u32) {
    let kind = match page.forms.get(seq) {
        Some(c) => c.kind,
        None => return,
    };
    match kind {
        ControlKind::Submit => {
            page.state.focus = Some(seq);
            submit_form(page, Some(seq));
        }
        ControlKind::Reset => {
            page.state.reset();
        }
        ControlKind::Checkbox | ControlKind::Radio => {
            page.state.focus = Some(seq);
            let f = &page.forms;
            page.state.toggle(f, seq);
        }
        ControlKind::Select => {
            page.state.focus = Some(seq);
            let f = &page.forms;
            page.state.cycle_select(f, seq);
        }
        _ => {
            // A text field takes focus with the caret at the end.
            page.state.focus = Some(seq);
            page.state.caret = page.forms.get(seq).map(|c| page.state.value(c).len()).unwrap_or(0);
        }
    }
}

/// Handle one event. Returns true if the chrome (address bar / title) should
/// be re-committed.
fn handle(engine: &Engine, ev: Event, cache: &Option<(Layout, i32, u32)>, page: &mut Page) -> bool {
    match ev {
        // Keep URL_BUF synced with the address-bar edit buffer.
        Event::InputChange { value } => {
            set_url(&value);
            // Typing in the address bar means the compositor moved keyboard
            // focus there — drop the page control's focus so only one caret
            // blinks and Enter goes to the right place.
            if page.state.focus.take().is_some() {
                bump_content_gen("addressbar-focus");
                mark_dirty();
            }
            false
        }
        // A page control has focus → the key is ours (the compositor only
        // routes keys here when no chrome Input/TextArea consumed them).
        Event::Key(k) if page.state.focus.is_some() => {
            if edit_key(page, k) {
                bump_content_gen("form-key");
                mark_dirty();
            }
            false
        }
        // No control focused: the keys that reach us drive the viewport.
        Event::Key(k) => {
            let step = match k {
                KeyCode::Down => 60,
                KeyCode::Up => -60,
                KeyCode::PageDown | KeyCode::Char(b' ') => 400,
                KeyCode::PageUp => -400,
                KeyCode::Home => i32::MIN / 2,
                KeyCode::End => i32::MAX / 2,
                _ => return false,
            };
            set_scroll(scroll_y().saturating_add(step));
            mark_dirty();
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
                bump_content_gen("site-css-toggle"); // force re-layout with/without site CSS
                mark_dirty();
                set_open_menu(0);
                true
            }
            ACT_VIEW_INSPECT => {
                toggle_inspect();
                if !inspect_mode() {
                    set_selected(None);
                }
                bump_content_gen("inspect-toggle"); // re-layout with/without inspect boxes
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
                    let fresh;
                    let lay = match cache.as_ref() {
                        Some((lay, cw, cg)) if *cw == w && *cg == content_gen() => lay,
                        _ => {
                            fresh = do_layout(engine, w as u32, &page.state);
                            &fresh
                        }
                    };
                    // Inspect mode intercepts the click: select the deepest
                    // element box under the cursor (shown as an outline + a
                    // status-bar label) instead of following a link.
                    if inspect_mode() {
                        let sel = lay.hit_inspect(cx, cy).map(|b| (b.x, b.y, b.w, b.h, b.label.clone()));
                        // Also echo to the serial console so it can be copied
                        // without transcribing from the screen.
                        if let Some((bx, by, _, _, ref label)) = sel {
                            log(&alloc::format!("[inspect] @({bx},{by}) {label}"));
                        } else {
                            log("[inspect] (no element here)");
                        }
                        set_selected(sel);
                        mark_dirty();
                        return true;
                    }
                    // A control wins over a link: a submit button inside an
                    // <a>, or a field overlapping a link rect, is the target.
                    if let Some(ctl) = lay.hit_control(cx, cy) {
                        let seq = ctl.seq;
                        activate(page, seq);
                        bump_content_gen("control-activate"); // repaint the focus ring / new value
                        mark_dirty();
                        return true;
                    }
                    let href = lay.hit_test(cx, cy).map(|s| s.to_string());
                    // Clicking the page elsewhere blurs a focused control.
                    if page.state.focus.take().is_some() {
                        bump_content_gen("control-blur");
                        mark_dirty();
                    }
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

// SAFETY: `HEAP` is a `static mut` we hand to the allocator exactly once, here,
// before any allocation can happen (this is a `const` initialiser). Nothing else
// ever reads or writes it, so the allocator holds the only reference.
#[global_allocator]
static ALLOCATOR: Talck<spin::Mutex<()>, ClaimOnOom> = Talc::new(unsafe {
    ClaimOnOom::new(Span::from_array(core::ptr::addr_of!(HEAP) as *mut [u8; HEAP_SIZE]))
})
.lock();

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
    // No heap init here — talc claims `HEAP` lazily on the first allocation.

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
    // The page's forms + the user's edits, rebuilt on every navigation.
    let mut page = Page::new();
    let mut last_bg = unsafe { npk_theme_token(0) };
    // Persistent paint buffer, reused across frames (see maybe_repaint).
    let mut paint_buf: Vec<u8> = Vec::new();
    // Image sources of the current page still to fetch, one per loop turn.
    let mut pending_imgs: Vec<String> = Vec::new();
    // CSS images still to fetch, as (url_key, url). Filled from the layout.
    let mut pending_css_imgs: Vec<(u64, String)> = Vec::new();
    // Every CSS image this page has already been asked for — including the
    // ones that failed. A miss must not be retried forever; the box simply
    // stays undecorated until the next navigation.
    let mut css_asked: Vec<u64> = Vec::new();
    loop {
        // Pick up a navigation: re-parse the document's forms, drop old edits.
        page.sync();
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
                    if handle(&engine, ev, &cache, &mut page) {
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
                bump_content_gen("theme-change");
                mark_dirty();
            }
        }
        // A fresh page: drop the old page's decoded images and note which ones
        // it wants (fetching them happens after the repaint, one batch a turn).
        //
        // This has to sit RIGHT BEFORE the repaint, not at the top of the loop.
        // A navigation happens while the event queue above is being drained, so
        // from the top of the loop it is always one turn late: the page was laid
        // out once against the PREVIOUS page's images, and clearing them a turn
        // later invalidated that layout and laid it out again. Two full layouts
        // per navigation, and on the device a layout is over five seconds.
        if images_dirty() {
            pending_imgs = begin_images(&mut engine);
            engine.css_images_begin();
            pending_css_imgs.clear();
            css_asked.clear();
        }
        maybe_repaint(&engine, &mut cache, &mut paint_buf, &page.state);
        // Text and layout are on screen now — pull in the next few images,
        // then come back round and paint them. Scrolling keeps working in
        // between, because a batch is small.
        let guessed: Vec<String> = cache
            .as_ref()
            .map(|(l, _, _): &(Layout, i32, u32)| l.guessed_image_srcs.clone())
            .unwrap_or_default();
        fetch_next_images(&mut engine, &mut pending_imgs, &guessed);
        // The layout reports which CSS images it needs, so this queue can only
        // be filled AFTER a layout — unlike `<img>`, whose srcs are in the HTML
        // and are queued once by `begin_images`.
        //
        // That difference is a trap: the cached layout keeps listing the SAME
        // srcs every turn (nothing re-lays-out when a background arrives — it
        // is a repaint), so the guard has to be "already asked for this page",
        // NOT "already in the queue". The queue empties on every fetch, so
        // checking it re-requested all of them once a turn, for as long as the
        // page stayed open. Cleared on navigation, with the engine's cache.
        if let Some((l, _, _)) = cache.as_ref() {
            for (k, u) in &l.css_image_srcs {
                if !css_asked.contains(k) {
                    css_asked.push(*k);
                    pending_css_imgs.push((*k, u.clone()));
                }
            }
        }
        fetch_next_css_images(&engine, &mut pending_css_imgs);
        // ALWAYS yield so this worker core can halt — a cooperative fiber that
        // never sleeps pins its core at 100%. A short nap while interacting
        // stays responsive; a longer one when idle keeps the core asleep.
        unsafe {
            let nap = if had_event || !pending_imgs.is_empty() || !pending_css_imgs.is_empty() { 4 } else { 16 };
            let _ = npk_sleep(nap);
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<app_meta::IconRef> {
    None
}
