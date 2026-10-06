//! beak — native, sandboxed web browser for nopeekOS (docs/spec/BROWSER.md).
//!
//! The page is rendered by the portable `beak-engine` (own layout + fontdue
//! rasterisation) into a `Widget::Canvas`; the chrome (toolbar, address bar,
//! footer) is loft-styled widgets. Scroll comes via `Event::Wheel`, link
//! clicks via a Canvas hit-test against the engine's link rects. The engine
//! is host-agnostic (§10); this shell is the thin nopeek adapter (queries
//! the canvas rect, paints, forwards input).

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU8, AtomicUsize, Ordering::Relaxed};

mod neterror;
mod selftest;
use beak_engine::cookies;

use beak_engine::charset;
use beak_engine::forms::{self, ControlKind, FormState, Forms};
use beak_engine::raster::HoverChange;
use beak_engine::{Engine, Layout};
use nopeek_widgets::i18n;
use nopeek_widgets::style::{Padding, Radius, Spacing};
use nopeek_widgets::{caps, prefab};
use nopeek_widgets::*;
use nopeek_widgets::host;

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
    /// Start the general request — any method, extra headers
    /// (newline-separated `Name: value`), a body — and come straight back
    /// with a handle. Nothing here waits for a network: the kernel waits on a
    /// fiber of its own while this loop keeps painting and reading keys.
    /// A non-2xx comes back as bytes, not as an error — a 404 page is a
    /// document.
    fn npk_http_begin(
        method_ptr: i32,
        method_len: i32,
        url_ptr: i32,
        url_len: i32,
        hdrs_ptr: i32,
        hdrs_len: i32,
        body_ptr: i32,
        body_len: i32,
        buf_max: i32,
    ) -> i32;
    /// 1 = the answer is here, 0 = still running, -1 = it failed, -2 = no
    /// such handle.
    fn npk_http_poll(handle: i32) -> i32;
    /// Collect a finished request: bytes written, -1 if it failed (the reason
    /// is in `npk_http_last_error`), -2 unknown handle, -3 still running.
    /// Frees the handle and fills the four response getters below.
    fn npk_http_take(handle: i32, buf_ptr: i32, buf_max: i32) -> i32;
    /// Give up on a handle. Idempotent — a navigation cancels whatever the
    /// last one left in the air without having to know which state it caught.
    fn npk_http_cancel(handle: i32) -> i32;
    /// The last collected response's header block, minus the status
    /// line. `Set-Cookie` repeats, so it can only be handed over raw.
    fn npk_http_response_headers(buf_ptr: i32, buf_max: i32) -> i32;
    /// Status code of the last collected response. Irrelevant for a
    /// navigation (a 404 page is a document), essential for `fetch`:
    /// `response.ok` depends on it.
    fn npk_http_status() -> i32;
    /// Random bytes from the kernel CSPRNG (ChaCha20, seeded from RDRAND).
    /// Returns the number of bytes written, or -1. At most 64 KiB per call.
    fn npk_random_bytes(ptr: i32, len: i32) -> i32;
    fn npk_http_final_url(buf_ptr: i32, buf_max: i32) -> i32;
    /// Why the last request failed: `kind\tmessage`. Cleared on success.
    fn npk_http_last_error(buf_ptr: i32, buf_max: i32) -> i32;
    /// The last response's Content-Type, verbatim. -1 if the server sent none.
    fn npk_http_content_type(buf_ptr: i32, buf_max: i32) -> i32;
    /// Tell the kernel which document is shown. It resolves the address
    /// itself and keeps only the network class, which decides whether a
    /// subresource may reach the private network.
    /// See `docs/plan/BROWSER_FETCH_ORIGIN.md` §3.1 V2.
    fn npk_net_context(url_ptr: i32, url_len: i32) -> i32;
    fn npk_clipboard_set(ptr: i32, len: i32) -> i32;
    /// A TLS stream, unlike `npk_http_*`: not a request with an end but a
    /// connection that stays open. Used for `wss://`. `connect` blocks for
    /// the handshake; `recv` returns immediately — 0 means nothing yet,
    /// -1 means closed.
    fn npk_tls_connect(host_ptr: i32, host_len: i32, port: i32) -> i32;
    fn npk_tls_send(handle: i32, buf_ptr: i32, buf_len: i32) -> i32;
    fn npk_tls_recv(handle: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_tls_close(handle: i32) -> i32;

    /// Start a newline-separated list of URLs in one call, multiplexed over
    /// HTTP/2 where the host offers it. Same handle discipline as
    /// `npk_http_begin`.
    fn npk_http_begin_many(urls_ptr: i32, urls_len: i32, out_max: i32) -> i32;
    /// Like above, but with one cookie line per URL (separated by `\n`,
    /// empty lines count).
    fn npk_http_begin_many_hdr(urls_ptr: i32, urls_len: i32,
                               hdrs_ptr: i32, hdrs_len: i32, out_max: i32) -> i32;
    /// Collect a finished batch: the bodies back-to-back in `out`, one
    /// little-endian i32 per URL in `lens` (bytes written, or -1). Returns
    /// how many URLs the batch had, or -1 / -2 / -3 as above.
    fn npk_http_take_many(
        handle: i32,
        out_ptr: i32,
        out_max: i32,
        lens_ptr: i32,
        lens_max: i32,
    ) -> i32;
}

// Safe wrappers for the calls only beak makes; the shared ones are in
// `nopeek_widgets::host`. The kernel validates every range it is handed.

/// The raw handle, or a negative refusal.
fn http_begin(method: &str, url: &str, hdrs: &str, body: &[u8], cap: usize) -> i32 {
    // SAFETY: FFI; the four ranges are borrowed for the call.
    unsafe {
        npk_http_begin(
            method.as_ptr() as i32, method.len() as i32,
            url.as_ptr() as i32, url.len() as i32,
            hdrs.as_ptr() as i32, hdrs.len() as i32,
            body.as_ptr() as i32, body.len() as i32,
            cap as i32,
        )
    }
}

fn http_poll(h: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_http_poll(h) }
}

/// Bytes written into `buf`, or the kernel's negative code.
fn http_take(h: i32, buf: &mut [MaybeUninit<u8>]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call and the kernel writes
    // only initialised bytes into it.
    unsafe { npk_http_take(h, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

fn http_cancel(h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_http_cancel(h) };
}

fn http_status() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_http_status() }
}

/// One of the four getters of the last response: bytes into `buf`, or `None`
/// when it wrote nothing.
fn http_getter(f: unsafe extern "C" fn(i32, i32) -> i32, buf: &mut [u8]) -> Option<&str> {
    // SAFETY: FFI; `f` is one of the getters declared above, and the range
    // is borrowed for the call.
    let n = unsafe { f(buf.as_mut_ptr() as i32, buf.len() as i32) };
    if n <= 0 {
        return None;
    }
    core::str::from_utf8(&buf[..(n as usize).min(buf.len())]).ok()
}

fn net_context(url: &str) {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_net_context(url.as_ptr() as i32, url.len() as i32) };
}

fn clipboard_set(text: &str) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_clipboard_set(text.as_ptr() as i32, text.len() as i32) }
}

fn tls_connect(host: &str, port: u16) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_tls_connect(host.as_ptr() as i32, host.len() as i32, port as i32) }
}

fn tls_send(h: i32, data: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_tls_send(h, data.as_ptr() as i32, data.len() as i32) }
}

fn tls_recv(h: i32, buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_tls_recv(h, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

fn tls_close(h: i32) {
    // SAFETY: FFI without pointers.
    unsafe { npk_tls_close(h) };
}

/// A byte buffer as the out-range of a host call.
fn as_uninit(b: &mut [u8]) -> &mut [MaybeUninit<u8>] {
    // SAFETY: same layout; the only writer through the result is the kernel,
    // which writes initialised bytes.
    unsafe { &mut *(b as *mut [u8] as *mut [MaybeUninit<u8>]) }
}

/// Seconds since the epoch, UTC. `ticks` cannot stand in: it restarts at
/// every boot and a cookie's `Expires` is an absolute date.
fn unix_now() -> i64 {
    host::unix_time() as i64
}

/// Milliseconds since boot. Used only for the phase timings below.
fn now_ms() -> i64 {
    host::ticks_ms() as i64
}

/// Log "<label>: <ms> ms". Phase timings are permanent, not scaffolding:
/// the engine runs under a WASM interpreter, and knowing which phase a page
/// load spends its time in decides what is worth optimising.
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
    host::log_serial(m);
}

/// The page palette: the canvas a document is painted on when it paints none
/// of its own, and the colours it inherits.
///
/// Deliberately not the desktop theme: a page built for a white canvas sets
/// only a near-black text colour, and a dark canvas behind it makes it
/// unreadable. Browsers paint such a page white.
///
/// "Dark mode" is two things: what the user prefers (a media query) and what
/// the canvas is (the used `color-scheme` of the root, light until a page
/// opts in). Both stay light until `color-scheme` is parsed; then a page that
/// opts in gets a dark canvas and a dark preference together.
fn query_theme() -> beak_engine::Theme {
    beak_engine::Theme {
        bg: beak_engine::Rgb(255, 255, 255),
        text: beak_engine::Rgb(0, 0, 0),
        heading: beak_engine::Rgb(0, 0, 0),
        link: beak_engine::Rgb(0, 0, 238),
        muted: beak_engine::Rgb(96, 96, 96),
        rule: beak_engine::Rgb(128, 128, 128),
    }
}

const CANVAS_ID: u32 = 1;

// Toolbar
const ACT_GO: u32 = 1;
const ACT_BACK: u32 = 2;
const ACT_FORWARD: u32 = 3;
const ACT_RELOAD: u32 = 4;
/// Give up on the page being loaded. The reload button becomes this while a
/// navigation is in flight.
const ACT_STOP: u32 = 5;

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

// The tab strip. Two id bands instead of two numbers per tab: the strip is
// rebuilt on every change, so an index is the tab.
const ACT_TAB_NEW: u32 = 7_000;
const ACT_TAB_SEL: u32 = 7_100;     // + Index
const ACT_TAB_CLOSE: u32 = 7_200;   // + Index

// Menu-label anchor NodeIds (for the dropdown Popover)
const NODE_MENU_FILE: u32 = 100;
const NODE_MENU_EDIT: u32 = 101;
const NODE_MENU_VIEW: u32 = 102;
const NODE_MENU_HELP: u32 = 104;

// Which menu dropdown is open (0 = none, else ACT_MENU_FILE..HELP encoded 1..4).
static OPEN_MENU: AtomicU8 = AtomicU8::new(0);
fn open_menu() -> u8 {
    OPEN_MENU.load(Relaxed)
}
fn set_open_menu(v: u8) {
    OPEN_MENU.store(v, Relaxed);
}
fn toggle_menu(which: u8) {
    let cur = open_menu();
    set_open_menu(if cur == which { 0 } else { which });
}

// ── Persistent state (static buffers — no heap growth across page loads) ───

const URL_CAP: usize = 4096;

/// Everything that belongs to one document rather than to the program
/// (`docs/plan/BROWSER_TABS.md` §A2). Two fields in one struct cannot be the
/// same buffer, which keeps e.g. the document URL and the address-bar text
/// apart.
///
/// Rule: nothing that belongs to a document may be added as a global.
/// The globals that remain each belong to something there is only one of:
///
/// | Fetch buffers (`HTML_BUF`, `CSS_BUF`, `IMG_FETCH_BUF` …) | belong to the running fetch |
/// | `LAST_W`/`LAST_H`/`LAST_SY` | describe the frame buffer |
/// | `SCRIPT_DEADLINE`, `BUDGET_T0`, `BUDGET_SAID` | describe the run on the stack |
/// | `OPEN_MENU`, `USE_SITE_CSS`, `INSPECT_MODE` | window and tools, not page |
/// | `TABS`, `ACTIVE` | the documents themselves, and which one is live |
struct Doc {
    /// Where the document came from (after redirects). Base for every
    /// relative URL of the page and the network context the kernel keeps.
    url: String,
    /// What the address bar shows. Text only — no network context, no DNS.
    edit: String,
    /// Back/forward history.
    hist: Vec<String>,
    hist_pos: usize,

    // ── Loading this document ───────────────────────────────────────────
    /// The running fetch, or -1.
    nav_job: i32,
    /// Which stage of the chain is running.
    nav_stage: NavStage,
    /// The URL the running fetch asked for — not the one it ended at.
    nav_url: Option<String>,
    nav_push_hist: bool,
    /// Start of the running stage, for the timing lines in the log.
    nav_stage_ms: i64,
    /// Counts every navigation. Lets pending callbacks recognise that they
    /// belong to a page that no longer exists.
    nav_gen: u32,
    nav_start_ms: i64,
    nav_reported: bool,
    /// Counts every content change — the number the layout depends on.
    content_gen: u32,
    /// How far the page is scrolled.
    scroll_y: i32,
    /// Where a text selection started, while the button is down.
    sel_anchor: Option<beak_engine::select::TextPos>,
    /// The link under the press, until the release decides — with the point
    /// where it was pressed.
    pending_link: Option<(String, i32, i32)>,
    /// The selection on the page — not in the address bar, which belongs to
    /// the compositor.
    sel: Option<(beak_engine::select::TextPos, beak_engine::select::TextPos)>,
    /// The find bar: `None` means closed. The text is owned by beak, not by
    /// a `Widget::Input`: `Event::InputChange` carries no node id, so two
    /// inputs in one window could not be told apart.
    find: Option<String>,
    /// Matches of the running search and the one currently selected.
    found: Vec<(beak_engine::select::TextPos, beak_engine::select::TextPos)>,
    find_at: usize,
    /// Accumulates the bytes of one character typed into the find bar.
    find_pending: [u8; 4],
    find_pending_len: u8,

    // ── Subresource fetches of the load ─────────────────────────────────
    // Sheets, scripts, modules: each stage records what it requested and
    // which round it is in. Per document, so two tabs loading at once do
    // not share bookkeeping.
    nav_css_count: usize,
    nav_scripts: Option<Vec<PendingScript>>,
    /// The JS session of this page.
    js: Option<beak_engine::js::Session>,
    nav_js_count: usize,
    nav_mod_entries: Option<Vec<String>>,
    nav_mod_want: Option<Vec<String>>,
    nav_mod_rounds: usize,
    nav_css_urls: Option<Vec<String>>,
    nav_css_parts: Option<Vec<(String, Vec<u8>, bool)>>,
    nav_css_want: Option<Vec<(usize, String)>>,
    nav_css_rounds: usize,
    nav_sheet_nodes: Option<Vec<u32>>,
    nav_sheet_rounds: usize,
    nav_dynjs_nodes: Option<Vec<u32>>,
    nav_dynjs_rounds: usize,

    // ── The view of this document ───────────────────────────────────────
    // Not the same as what the frame buffer holds (`LAST_W/H/SY` below):
    // there is one buffer and many pages.
    /// The picture no longer matches what the page says.
    dirty: bool,
    /// For a reason other than scrolling — repaint everything.
    need_full: bool,
    /// An image arrived; the image list must be re-scanned.
    images_dirty: bool,
    /// The boxes of the last layout, for `getBoundingClientRect` & co.
    ///
    /// Held as `Rc` so handing them to the JS engine is free: the scroll
    /// position changes every frame, the boxes only on a new layout.
    geom: Option<alloc::rc::Rc<alloc::vec::Vec<beak_engine::layout::ElemRect>>>,
    /// The viewport the JS session of this document last heard.
    last_vp: (i32, i32),
    /// Scroll position and viewport as last reported to the page.
    ///
    /// Separate from `scroll_y`/`last_vp`: those say what is painted, these
    /// say what the page knows. Without the difference `scroll` would fire
    /// every frame or never.
    told_scroll: i32,
    told_vp: (i32, i32),
    /// Which half of the blink cycle the caret was last painted in.
    /// `None`: no caret is blinking.
    caret_phase: Option<bool>,
    /// When the cycle was last reset. The caret stays solid right after a
    /// key press, as in other browsers, so a typist can see where it is.
    caret_since: i64,

    // ── Subresources of this document ───────────────────────────────────
    // Images, backgrounds, fonts, `fetch`: everything that runs beside the
    // document and ends with it. `subresources_cancel` cancels exactly those
    // of one page.
    /// The running `<img>` batch and what it asked for, so an arriving body
    /// is stored under the source the page named it by. -1 / None when
    /// nothing is in flight.
    img_job: i32,
    img_job_srcs: Option<Vec<(String, String)>>,
    /// URLs a failure has already been logged for — once, not every frame.
    img_missed: Vec<String>,
    /// The same for backgrounds, keyed the way the layout keys them.
    cssimg_job: i32,
    cssimg_job_keys: Option<Vec<(u64, String)>>,
    /// The running font round.
    font_job: i32,
    font_want: Option<Vec<(String, u32, u16, bool)>>,
    /// Which engine `fetch` request sits on which host handle.
    fetch_jobs: Vec<(u32, i32)>,
    /// Images of this page not yet requested — the rest of the queue behind
    /// the running batch. Per document, so a tab switch does not keep
    /// fetching the old tab's images against the new base.
    pending_imgs: Vec<String>,
    pending_css_imgs: Vec<(u64, String)>,
    /// Every background this page has asked for, including failed ones. A
    /// failure must not be retried forever.
    css_asked: Vec<u64>,

    // ── The script round of this document ───────────────────────────────
    /// How many navigations in a row the page has triggered itself.
    script_nav_chain: u32,
    /// Set by `sync_nav` right before `nav_begin`, so `nav_begin` knows the
    /// chain continues rather than starts over.
    nav_from_script: bool,
    /// What the classic scripts amounted to (ran, failed, bytes). Must
    /// survive the module rounds because the report is written after them.
    script_tally: (usize, usize, usize),
    /// When the script round started. Not `nav_stage_ms`: after a module
    /// round that points at the round's start and the reported time is short.
    script_t0: i64,
    /// Is `load` still pending? It fires only once geometry exists.
    load_pending: bool,
    /// How many frames in a row an observer callback changed the tree — the
    /// guard against the "ResizeObserver loop".
    obs_rounds: u32,

    // ── What this page cost, and what is said about it once ─────────────
    // `set_url` resets all five: a new page gets its own verdict and its own
    // chance to report it.
    /// What the last full layout cost, in ms — decides whether this page can
    /// afford `:hover`.
    last_layout_ms: i64,
    hover_refused: bool,
    hover_said_fast: bool,
    hover_said_slow: bool,
    ctl_bail_said: bool,
    /// The box picked in the inspector: `(x, y, w, h)` in document space,
    /// with its label. Coordinates are valid in this document.
    sel_box: Option<(i32, i32, i32, i32, String)>,

    // ── What restores a frozen tab ──────────────────────────────────────
    // These fields and the four at the top (`url`, `edit`, `hist`,
    // `hist_pos`) are all a background tab keeps. See `tab_freeze`.
    /// The page's `<title>`, for the tab strip. Updated on layout and kept
    /// across freezing.
    title: String,
    /// Where to scroll after loading. 0 for a new page, the remembered
    /// position for a returning tab (which is refetched on switch).
    scroll_want: i32,
}

impl Doc {
    const fn new() -> Doc {
        Doc {
            url: String::new(), edit: String::new(), hist: Vec::new(), hist_pos: 0,
            nav_job: -1, nav_stage: NavStage::Doc, nav_url: None, nav_push_hist: false,
            nav_stage_ms: 0, nav_gen: 0, nav_start_ms: 0, nav_reported: true,
            content_gen: 0, scroll_y: 0, sel_anchor: None, sel: None, pending_link: None,
            find: None, found: Vec::new(), find_at: 0,
            find_pending: [0; 4], find_pending_len: 0,
            nav_css_count: 0, nav_scripts: None, js: None, nav_js_count: 0, nav_mod_entries: None, nav_mod_want: None, nav_mod_rounds: 0, nav_css_urls: None, nav_css_parts: None, nav_css_want: None, nav_css_rounds: 0, nav_sheet_nodes: None, nav_sheet_rounds: 0, nav_dynjs_nodes: None, nav_dynjs_rounds: 0,
            dirty: true, need_full: true, images_dirty: false, geom: None, last_vp: (0, 0),
            told_scroll: 0, told_vp: (0, 0), caret_phase: None, caret_since: 0,
            img_job: -1, img_job_srcs: None, img_missed: Vec::new(),
            cssimg_job: -1, cssimg_job_keys: None,
            font_job: -1, font_want: None, fetch_jobs: Vec::new(),
            pending_imgs: Vec::new(), pending_css_imgs: Vec::new(), css_asked: Vec::new(),
            script_nav_chain: 0, nav_from_script: false, script_tally: (0, 0, 0),
            script_t0: 0, load_pending: false, obs_rounds: 0,
            last_layout_ms: 0, hover_refused: false, hover_said_fast: false,
            hover_said_slow: false, ctl_bail_said: false, sel_box: None,
            title: String::new(), scroll_want: 0,
        }
    }
}

/// The tabs. `docs/plan/BROWSER_TABS.md` §A3 (b): one live engine, the rest
/// frozen.
///
/// The engine holds the tree, style sheet and images of one page, and
/// `HTML_BUF`/`CSS_BUF` are single buffers. A background tab is therefore
/// only what restores it (URL, history, scroll position, title), and the page
/// is refetched on switching back; `DOC_SLOTS` in the engine and the image
/// cache keep that cheap. Cost: script state does not survive a tab switch.
///
/// `Box` is required: `js_session()` and `fetch_jobs()` hand out
/// `&'static mut` into the document, and a `Vec<Doc>` that reallocates when
/// a tab opens would leave them pointing at freed memory.
static mut TABS: Vec<alloc::boxed::Box<Doc>> = Vec::new();
/// The tab that is painted and live. Always valid — `active` clamps.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// The layout engine, as a global rather than a variable of the frame loop:
/// `Interp::relayout` is a `fn` pointer and captures nothing, so the hook a
/// page uses to request a fresh layout mid-script cannot reach a local.
///
/// As with `tabs()`, the borrow ends in the calling statement; holding a
/// `&mut` across a script run would create two.
static mut ENGINE: Option<Engine> = None;

fn engine() -> &'static Engine {
    // SAFETY: single thread; `main` creates it before first use, and the
    // borrow ends in the calling statement.
    unsafe { (*core::ptr::addr_of_mut!(ENGINE)).get_or_insert_with(Engine::new) }
}
fn engine_mut() -> &'static mut Engine {
    // SAFETY: as in `engine()`.
    unsafe { (*core::ptr::addr_of_mut!(ENGINE)).get_or_insert_with(Engine::new) }
}

/// Maximum number of tabs.
///
/// The limit is the strip, not memory (a frozen tab is kilobytes): beyond
/// ten tabs the `+` would be pushed out of the window. A horizontally
/// scrolling strip (`Widget::Scroll`, `Axis::Horizontal`) would lift it.
const MAX_TABS: usize = 10;

fn tabs() -> &'static mut Vec<alloc::boxed::Box<Doc>> {
    // SAFETY: single thread; the borrow ends in the calling statement.
    // The empty case happens exactly once, on the very first access.
    unsafe {
        let p = core::ptr::addr_of_mut!(TABS);
        if (*p).is_empty() {
            (*p).push(alloc::boxed::Box::new(Doc::new()));
        }
        &mut *p
    }
}

/// The current tab. Clamped rather than checked: an `ACTIVE` pointing at a
/// closed tab would otherwise panic where nobody expects it.
fn active() -> usize {
    let n = tabs().len();
    let a = ACTIVE.load(Relaxed);
    if a >= n { n - 1 } else { a }
}
fn set_active(i: usize) {
    ACTIVE.store(i, Relaxed);
}

/// Rule for both: one borrow per statement.
///
/// beak is single-threaded, but single-threaded is not alias-free. These
/// references come from an `unsafe` deref and escape the borrow checker: it
/// would accept `let d = doc_mut(); … doc().x …`, which is two simultaneous
/// borrows of one object — undefined behaviour on `&mut`.
///
/// In practice: `doc().field` and `doc_mut().field = …` as a whole statement
/// are always correct; a held `let d = doc_mut()` only if nothing in between
/// calls back into these.
fn doc() -> &'static Doc {
    let a = active();
    &tabs()[a]
}
fn doc_mut() -> &'static mut Doc {
    let a = active();
    &mut tabs()[a]
}

/// Truncate to `cap` bytes, at a character boundary (a cut mid-character
/// would make the text invalid UTF-8).
fn clip(s: &str, cap: usize) -> &str {
    if s.len() <= cap { return s }
    let mut n = cap;
    while n > 0 && !s.is_char_boundary(n) { n -= 1; }
    &s[..n]
}

const HTML_CAP: usize = 3 * 1024 * 1024;
static mut HTML_BUF: [u8; HTML_CAP] = [0; HTML_CAP];
static HTML_LEN: AtomicUsize = AtomicUsize::new(0);

// Concatenated bytes of the page's external <link rel=stylesheet> files.
// Large pages link dozens of sheets totalling several MiB; the link count is
// the wrong unit to cap. A dropped stylesheet is a broken page, not a missing
// icon, so the headroom is deliberate. The buffer is `.bss`: it costs runtime
// memory only, nothing in the shipped .wasm.
const CSS_CAP: usize = 8 * 1024 * 1024;
const MAX_CSS_LINKS: usize = 64;
static mut CSS_BUF: [u8; CSS_CAP] = [0; CSS_CAP];
static CSS_LEN: AtomicUsize = AtomicUsize::new(0);

// Scratch buffer a whole batch of <img> bytes arrives in before decoding.
//
// Shared by `IMG_BATCH` images at once; a single large photograph must fit.
// The kernel bounds what all pending answers may reserve together
// (`MAX_RESERVED_BYTES`, 64 MB); 24 MB leaves room for a document (3) +
// stylesheets (8) + scripts (8) in flight beside it.
const IMG_FETCH_CAP: usize = 24 * 1024 * 1024;
/// A request backstop, not a memory bound. Memory is bounded in the engine:
/// a per-page budget of decoded BGRA plus a per-image pixel cap. This only
/// keeps one absurd document from queueing thousands of round-trips, and it
/// says so when it bites.
const MAX_IMAGES: usize = 512;
static mut IMG_FETCH_BUF: [u8; IMG_FETCH_CAP] = [0; IMG_FETCH_CAP];

// The three fetch buffers stay in `.bss`: they are tens of MiB and are filled
// by the kernel without being zeroed first. Every access goes through these
// accessors; the borrow they hand out must end before the next call to the
// same accessor (single thread, same rule as `doc_mut`).
fn html_buf() -> &'static mut [u8; HTML_CAP] {
    // SAFETY: single thread; see above.
    unsafe { &mut *core::ptr::addr_of_mut!(HTML_BUF) }
}
fn css_buf() -> &'static mut [u8; CSS_CAP] {
    // SAFETY: single thread; see above.
    unsafe { &mut *core::ptr::addr_of_mut!(CSS_BUF) }
}
fn img_fetch_buf() -> &'static mut [u8; IMG_FETCH_CAP] {
    // SAFETY: single thread; see above.
    unsafe { &mut *core::ptr::addr_of_mut!(IMG_FETCH_BUF) }
}

/// How many images one batch asks for. Small on purpose: a large batch
/// would make a turn of the loop long; four is enough to overlap the
/// round-trips.
const IMG_BATCH: usize = 4;

/// Room for the per-URL length table of `npk_http_take_many`. Sized for the
/// largest batch either caller asks for.
const LENS_CAP: usize = 4 * MAX_CSS_LINKS;

/// The URL list a batch call wants: one per line.
fn url_lines(urls: &[String]) -> String {
    let mut blob = String::new();
    for (i, u) in urls.iter().enumerate() {
        if i > 0 {
            blob.push('\n');
        }
        blob.push_str(u);
    }
    blob
}

/// Start a batch and return its handle, or -1. `cap` is the room the bodies
/// may take together.
fn begin_batch(urls: &[String], cap: usize) -> i32 {
    let blob = url_lines(urls);
    // Every subresource gets its cookies, not only the document request.
    //
    // One line per URL, in the same order, empty lines included: the mapping
    // is by position, and dropping empty lines would send one URL's cookies
    // to another.
    let now = unix_now();
    let mut ck = String::new();
    let mut any = false;
    for (i, u) in urls.iter().enumerate() {
        if i > 0 { ck.push('\n'); }
        let v = cookies::header_for(u, now);
        if !v.is_empty() { any = true; ck.push_str(&v); }
    }
    // No cookies involved: use the plain call; nothing for the kernel to
    // check.
    if !any {
        // SAFETY: FFI; the range is borrowed for the call.
        return unsafe { npk_http_begin_many(blob.as_ptr() as i32, blob.len() as i32, cap as i32) };
    }
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe {
        npk_http_begin_many_hdr(blob.as_ptr() as i32, blob.len() as i32,
                                ck.as_ptr() as i32, ck.len() as i32, cap as i32)
    }
}

/// Collect a finished batch into `dst`, returning each body as a
/// `(offset, len)` span.
///
/// Returns an empty vec if the batch failed, which the callers treat the
/// same as "none of them loaded" — every one of them degrades to a
/// placeholder or to unstyled content rather than to a blank page.
fn take_batch(handle: i32, dst: &mut [MaybeUninit<u8>], want: usize) -> Vec<(usize, usize)> {
    let mut lens = [0u8; LENS_CAP];
    // SAFETY: FFI; both ranges are borrowed for the call and the kernel
    // writes only initialised bytes into them.
    let n = unsafe {
        npk_http_take_many(
            handle,
            dst.as_mut_ptr() as i32,
            dst.len() as i32,
            lens.as_mut_ptr() as i32,
            LENS_CAP as i32,
        )
    };
    let mut spans = Vec::new();
    if n <= 0 {
        return spans;
    }
    let mut off = 0usize;
    for i in 0..(n as usize).min(want).min(LENS_CAP / 4) {
        let len = i32::from_le_bytes([lens[i * 4], lens[i * 4 + 1], lens[i * 4 + 2], lens[i * 4 + 3]]);
        if len < 0 {
            spans.push((0, 0)); // this one failed; keep positions aligned
        } else {
            spans.push((off, len as usize));
            off += len as usize;
        }
    }
    spans
}
const PAYLOAD_CAP: usize = URL_CAP;

const EVENT_BUF_SIZE: usize = 16 * 1024;

/// What the frame buffer holds — not what the page says.
///
/// There is one buffer, so these three numbers belong to the window. Whether
/// a repaint is needed belongs to the document (`Doc::dirty`,
/// `Doc::need_full`): an image arriving in the background makes its own page
/// stale, not the picture on screen.
static LAST_W: AtomicI32 = AtomicI32::new(-1);
static LAST_H: AtomicI32 = AtomicI32::new(-1);
/// The scroll offset the buffer currently holds, so the next frame knows how
/// far the picture has to move.
static LAST_SY: AtomicI32 = AtomicI32::new(0);

/// Tell the kernel which document the next requests come from.
///
/// The kernel trusts the URL but not the class — it resolves it itself.
/// Page code can never widen the context, because it has no path to a host
/// function. beak itself can still get it wrong; this function and
/// `set_url` are the only callers.
fn tell_net_context(url: &str) {
    net_context(url);
}

fn set_url(s: &str) {
    // Again after loading, with the URL the document really came from
    // (after redirects); `nav_begin` already reported the target's.
    tell_net_context(s);
    let d = doc_mut();
    d.url.clear();
    d.url.push_str(clip(s, URL_CAP));
    // A selection belongs to the text it marks; the new page has other text
    // ops at the same places.
    d.sel = None;
    d.sel_anchor = None;
    // A link whose release never came must not redirect the next page.
    d.pending_link = None;
    // The bar shows where we are — until someone types into it.
    set_edit(s);
    // A new page gets its own verdict on whether it can afford `:hover`, and
    // its own chance to say so once.
    let d = doc_mut();
    d.hover_refused = false;
    d.hover_said_fast = false;
    d.hover_said_slow = false;
    d.ctl_bail_said = false;
    d.last_layout_ms = 0;
}
fn url_str() -> &'static str { &doc().url }

/// The text field only — no network context, no new base, no DNS.
///
/// A key press is not a navigation: reporting every prefix of what the user
/// types as network context would send each one to the resolver.
fn set_edit(s: &str) {
    let d = doc_mut();
    d.edit.clear();
    d.edit.push_str(clip(s, URL_CAP));
}

fn edit_str() -> &'static str { &doc().edit }
fn html_str() -> &'static str {
    let len = HTML_LEN.load(Relaxed);
    core::str::from_utf8(&html_buf()[..len]).unwrap_or("")
}
fn css_str() -> &'static str {
    let len = CSS_LEN.load(Relaxed);
    core::str::from_utf8(&css_buf()[..len]).unwrap_or("")
}

/// The current page's forms + the user's live edits to them. Rebuilt on every
/// navigation (keyed on `Doc::nav_gen`, not the layout's content generation — a theme
/// switch or an image arriving must not wipe what the user has typed).
struct Page {
    forms: Forms,
    state: FormState,
    nav: u32,
    /// Tree generation `forms` was built from.
    scripted: u64,
    /// The focused field's value when it gained focus.
    ///
    /// `change` on a text field fires on leaving, not per character, and only
    /// if something actually changed (HTML §4.10.5.5).
    focus_value: Option<String>,
    /// Fingerprint of the last reported inventory.
    ///
    /// `sync` runs after every script run and observer callback, not only
    /// after a navigation; an unchanged inventory is not worth a log line.
    logged: u64,
}

impl Page {
    fn new() -> Page {
        Page { forms: forms::Forms { forms: Vec::new(), controls: Vec::new() },
               state: FormState::default(), nav: 0, scripted: 0, logged: 0,
               focus_value: None }
    }
    /// Refresh the form model — after a navigation or after scripts replaced
    /// the tree.
    ///
    /// Collected from the live tree, not the source: a page that builds its
    /// form by script would otherwise have no controls here.
    ///
    /// Returns true if it really re-collected — only then may the caller
    /// take the values from the tree.
    fn sync(&mut self, engine: &Engine) -> bool {
        let g = nav_gen();
        let sg = engine.scripted_gen();
        if self.nav == g && self.scripted == sg {
            return false;
        }
        let navigated = self.nav != g;
        self.nav = g;
        self.scripted = sg;
        self.forms = match engine.with_scripted(forms::collect) {
            Some(f) => f,
            None => forms::collect(&beak_engine::parse(html_str())),
        };
        // A navigation discards the input, a script run does not: what the
        // user typed is theirs, even if the page rebuilds around it.
        //
        // It must also survive renumbering: `Doc::to_dom` reassigns `seq` on
        // every write-back, in document order, so one extra node near the
        // top renames every control after it, and `FormState` is keyed by
        // that number. The tree carries the value (every key press writes it
        // there), so it is pulled back from there under the new numbers.
        if navigated {
            self.state.reset();
        } else if let Some(s) = js_session() {
            pull_control_values(self, s);
        }
        self.log_forms();
        true
    }

    /// Report the forms this page offers, once per navigation.
    ///
    /// "The button does nothing" has several very different causes — a button
    /// we never saw, one whose form we could not resolve, a form whose action
    /// we misread — and from the outside they look identical. Three lines on
    /// the serial tell them apart, which a screenshot cannot: a picture shows
    /// the pixels, not who owns which control.
    fn log_forms(&mut self) {
        if self.forms.controls.is_empty() {
            return;
        }
        let mut fp = ((self.forms.forms.len() as u64) << 32) | self.forms.controls.len() as u64;
        for c in self.forms.controls.iter().filter(|c| c.kind.is_submit()) {
            fp = fp.rotate_left(7) ^ (c.seq as u64) ^ ((c.form.map_or(0, |f| f as u64 + 1)) << 20);
        }
        if fp == self.logged {
            return;
        }
        self.logged = fp;
        let mut s = String::from("[beak] forms: ");
        push_i64(&mut s, self.forms.forms.len() as i64);
        s.push_str(", controls: ");
        push_i64(&mut s, self.forms.controls.len() as i64);
        log(&s);
        for (i, f) in self.forms.forms.iter().enumerate().take(6) {
            let mut s = String::from("[beak]   form#");
            push_i64(&mut s, i as i64);
            s.push(' ');
            s.push_str(if f.method_get { "GET " } else { "POST " });
            s.push_str(if f.action.is_empty() { "(dieses Dokument)" } else { &f.action });
            log(&s);
        }
        // Only the controls that are supposed to DO something — a page can
        // carry dozens of hidden fields and they are not the question.
        for c in self.forms.controls.iter().filter(|c| c.kind.is_submit()).take(8) {
            let mut s = String::from("[beak]   submit seq=");
            push_i64(&mut s, c.seq as i64);
            s.push_str(" form=");
            match c.form {
                Some(f) => push_i64(&mut s, f as i64),
                None => s.push_str("KEINS"),
            }
            s.push_str(" name=");
            s.push_str(if c.name.is_empty() { "-" } else { &c.name });
            // The label on the button: several forms may post to the same
            // URL, and only the label tells e.g. "reject all" from "accept
            // all" in the log.
            if !c.label.is_empty() {
                s.push_str("  \"");
                s.push_str(&c.label);
                s.push('"');
            }
            log(&s);
        }
    }
    /// The focused control's current text + its kind.
    fn focused(&self) -> Option<(&forms::Control, &str)> {
        let seq = self.state.focus?;
        let c = self.forms.get(seq)?;
        Some((c, self.state.value(c)))
    }
}

// Navigation generation — bumped only by a real page load.
fn nav_gen() -> u32 {
    doc().nav_gen
}
fn bump_nav_gen() {
    let d = doc_mut();
    d.nav_gen = d.nav_gen.wrapping_add(1);
}

// Reader-mode toggle: apply the site's own (external + <style>) CSS, or render
// with just our UA sheet (docs/spec/BROWSER.md §9.7 — never worse than clean content).
static USE_SITE_CSS: AtomicBool = AtomicBool::new(true);
fn use_site_css() -> bool {
    USE_SITE_CSS.load(Relaxed)
}
fn toggle_site_css() {
    USE_SITE_CSS.store(!use_site_css(), Relaxed);
}

// Inspect dev tool: when on, the engine records an element box per node and a
// canvas click selects the deepest box under the cursor (outline + a label in
// the status bar) instead of following a link, so a mis-rendered element can
// be named.
static INSPECT_MODE: AtomicBool = AtomicBool::new(false);
fn inspect_mode() -> bool {
    INSPECT_MODE.load(Relaxed)
}
fn toggle_inspect() {
    INSPECT_MODE.store(!inspect_mode(), Relaxed);
}
fn set_selected(b: Option<(i32, i32, i32, i32, String)>) {
    doc_mut().sel_box = b;
}
fn selected_rect() -> Option<(i32, i32, i32, i32)> {
    doc().sel_box.as_ref().map(|(x, y, w, h, _)| (*x, *y, *w, *h))
}
fn selected_label() -> Option<String> {
    doc().sel_box.as_ref().map(|(_, _, _, _, l)| l.clone())
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
    let ms = now_ms() - t0;
    // The width belongs in the logged number: a timing without it cannot be
    // compared, and reading it off the screen means opening a window, which
    // changes the width being measured.
    let mut label = String::from("layout @");
    push_i64(&mut label, w as i64);
    label.push_str("px (parse+cascade+layout)");
    log_ms(&label, ms);
    doc_mut().last_layout_ms = ms;
    // ...and which of the three phases it was. Under a WASM interpreter the
    // phases do not scale like on the host, so one total cannot say why.
    let p = lay.phase;
    if p[0] + p[1] + p[2] > 0 {
        log_ms("  dom::parse", p[0] as i64);
        log_ms("  css::cascade", p[1] as i64);
        log_ms("  box layout", p[2] as i64);
    }
    lay
}
fn scroll_y() -> i32 {
    doc().scroll_y
}
/// Where the page scrolls after loading.
///
/// Zero for a new page. For a returning tab, the remembered position: the
/// page is refetched on switch (`docs/plan/BROWSER_TABS.md` §A3 b), and
/// without it the reader would land at the top.
fn scroll_after_load() {
    let y = doc().scroll_want;
    doc_mut().scroll_want = 0;
    set_scroll(y);
}
fn set_scroll(y: i32) {
    doc_mut().scroll_y = y;
}
/// Something other than scrolling wants a repaint.
///
/// Scrolling does not change the page, it moves it — so a frame that is dirty
/// for scrolling alone can be blitted and have one band redrawn. Anything else
/// (a hover, a form key, a new layout) sets `need_full` and gets the whole
/// viewport. It is set, never cleared, until a frame is actually painted: a
/// hover followed by a scroll must still repaint everything.
fn mark_dirty() {
    let d = doc_mut();
    d.dirty = true;
    d.need_full = true;
}

/// Dirty because the viewport moved — the display list is untouched.
fn mark_dirty_scrolled() {
    doc_mut().dirty = true;
}

// Content generation — bumped on every fetch so the layout cache knows to
// re-lay-out (vs. reusing it for scroll, which keeps scrolling smooth).

/// Invalidate the layout cache. `why` is logged because a full re-layout is
/// the single most expensive thing this app does, so an unexpected one has
/// to be attributable at a glance.
fn bump_content_gen(why: &str) {
    let d = doc_mut();
    d.content_gen = d.content_gen.wrapping_add(1);
    let mut b = String::new();
    b.push_str("[beak] relayout: ");
    b.push_str(why);
    log(&b);
}
fn content_gen() -> u32 {
    doc().content_gen
}

/// A pointer that costs more than this to follow makes the page feel broken —
/// the window stops answering while it re-lays-out. Below it, hover is free
/// enough to be worth having. Only the fallback is measured against it: a
/// pointer change the engine can answer by repainting costs a fraction of a
/// millisecond and is never refused.
const HOVER_BUDGET_MS: i64 = 250;
/// Say once per page how the pointer is being answered here.
///
/// Without these lines a pointer answered by repainting is invisible in the
/// log, so a run cannot tell "it works" from "it never happened".
fn say_hover_once(fast: bool, ms: i64, why: &str) {
    let said = if fast { doc().hover_said_fast } else { doc().hover_said_slow };
    if said {
        return;
    }
    if fast { doc_mut().hover_said_fast = true } else { doc_mut().hover_said_slow = true }
    let mut b = String::new();
    if fast {
        b.push_str("[beak] :hover repainted in ");
        b.push_str(&alloc::format!("{ms} ms (a layout here costs {})", doc().last_layout_ms));
    } else {
        b.push_str("[beak] :hover needs a layout: ");
        b.push_str(why);
    }
    log(&b);
}

/// Can this page afford to restyle on pointer movement?
fn hover_affordable() -> bool {
    let ms = doc().last_layout_ms;
    if ms <= HOVER_BUDGET_MS {
        return true;
    }
    if !doc().hover_refused {
        doc_mut().hover_refused = true;
        let mut b = String::new();
        b.push_str("[beak] :hover needs a layout here, and one costs ");
        b.push_str(&alloc::format!("{ms} ms"));
        log(&b);
    }
    false
}

/// Why the last fetch failed, as `(kind, message)`. `None` if the kernel
/// reported nothing, so the caller must have a fallback.
fn last_error() -> Option<(String, String)> {
    let mut buf = [0u8; 512];
    let s = http_getter(npk_http_last_error, &mut buf)?;
    let (kind, msg) = s.split_once('\t')?;
    Some((kind.to_string(), msg.to_string()))
}

/// Replace the document with a diagnostic page. Sets HTML_BUF/HTML_LEN
/// exactly as a successful fetch would, so everything downstream — layout,
/// paint, scrolling — treats it as an ordinary page.
fn show_error_page(url: &str) {
    let (kind, message) = last_error()
        // A -1 with no reason attached still has to say something. Silence
        // here is the blank page this whole path exists to remove.
        .unwrap_or_else(|| (String::from("unknown"), String::from("request failed")));
    log(&alloc::format!("[beak] fetch failed: {} ({})", message, kind));

    let doc = neterror::document(url, &kind, &message);
    let len = doc.len().min(HTML_CAP);
    html_buf()[..len].copy_from_slice(&doc.as_bytes()[..len]);
    HTML_LEN.store(len, Relaxed);
    // The page carries its own inline <style> and links nothing, so any
    // leftover author CSS from the previous page must go — otherwise the
    // last site's rules would style this one.
    CSS_LEN.store(0, Relaxed);
}

/// The last response's Content-Type. `None` if the server sent none, which
/// is why every caller has to cope with not knowing rather than assume UTF-8.
fn content_type() -> Option<String> {
    let mut buf = [0u8; 256];
    http_getter(npk_http_content_type, &mut buf).map(|s| s.to_string())
}

/// Bring the freshly fetched document to valid UTF-8, in place.
///
/// Must run before anything reads `html_str()` — the stylesheet scan does,
/// and a document still holding raw Latin-1 reads back as the empty string.
fn decode_document() {
    let len = HTML_LEN.load(Relaxed);
    if len == 0 {
        return;
    }
    let ct = content_type();
    let (n, how) = charset::to_utf8_in_place(html_buf(), len, ct.as_deref());
    HTML_LEN.store(n, Relaxed);
    if how != charset::KEPT {
        log(&alloc::format!("[beak] document charset: {} ({} -> {} B)", how, len, n));
    }
}

/// Same for the concatenated stylesheets. No Content-Type here — they arrive
/// through the batch fetch, which reports one status per URL and no headers —
/// so this is sniff-only; one bad byte must not cost the page all its CSS.
fn decode_css() {
    let len = CSS_LEN.load(Relaxed);
    if len == 0 {
        return;
    }
    let (n, how) = charset::to_utf8_in_place(css_buf(), len, None);
    CSS_LEN.store(n, Relaxed);
    if how != charset::KEPT {
        log(&alloc::format!("[beak] css charset: {} ({} -> {} B)", how, len, n));
    }
}

/// The URL the last fetch's body actually came from, after redirects.
/// `None` if the kernel reported none (e.g. the request failed).
fn fetched_from() -> Option<String> {
    let mut buf = [0u8; URL_CAP];
    http_getter(npk_http_final_url, &mut buf).map(|s| s.to_string())
}

/// Room for one response's header block — the kernel caps what it hands back
/// at 8 KiB, and a page that sets a dozen cookies still fits.
const HDR_CAP: usize = 8 * 1024;

// ── A navigation that runs while the window stays alive ───────────────────
//
// A page load is two round trips — the document, then its stylesheets — and
// neither may be waited for. Both are started here and collected by
// `nav_pump` on a later turn of the loop, so between them beak paints,
// scrolls and answers keys exactly as it does when idle.
//
// The two stages stay strictly ordered, and nothing is painted between them:
// stylesheets are render-blocking, and drawing the bare document first would
// cost a full layout that the arriving CSS throws away on the next turn.

#[derive(Clone, Copy, PartialEq)]
enum NavStage {
    Doc,
    Css,
    /// The page's external scripts. A third round trip, after the
    /// stylesheets: a script reads classes and sizes.
    Js,
    /// The module graph. Not a single round trip: a module names its
    /// dependencies only once it has arrived, so it goes round by round
    /// until the graph is closed.
    Mod,
    /// Stylesheets inserted by a script. Also round-based: an arriving sheet
    /// lets a component finish building, which may insert another.
    Sheet,
    /// The `@import` sheets of the linked sheets. Round-based like `Mod`: a
    /// sheet names its own imports only once it has arrived.
    CssImport,
    /// `<script src=…>` inserted by a script. Round-based like `Sheet`: an
    /// arriving chunk inserts the next, which is how a split bundle loads.
    DynJs,
}

/// Handle of the navigation in flight, or -1.

fn nav_job() -> i32 {
    doc().nav_job
}

/// Is a page load in the air? The toolbar asks (its reload button becomes a
/// stop button), and so does the idle nap.
fn nav_busy() -> bool {
    nav_job() >= 0
}

fn nav_clear() {
    {
        doc_mut().nav_scripts = None;
        doc_mut().nav_job = -1;
        doc_mut().nav_url = None;
        doc_mut().nav_push_hist = false;
    }
}

/// Drop a navigation still in the air — a second click, or Stop. The kernel
/// throws its answer away; nothing on screen changes, so the page that is
/// already there stays readable.
fn nav_cancel() {
    let h = nav_job();
    if h >= 0 {
        http_cancel(h);
    }
    nav_clear();
}

/// The address the navigation in flight asked for.
fn nav_asked() -> String {
    doc().nav_url.clone().unwrap_or_default()
}

/// Start a navigation and return at once. `push_hist` records the address we
/// land on once the document is here.
fn nav_begin(engine: &Engine, method: &str, url: &str, body: &[u8], extra: &str, push_hist: bool) {
    // A navigation that did not come from a script (a click, the address
    // bar, history) breaks the script chain; otherwise the cap would keep
    // counting across pages and eventually stop a harmless redirect.
    if doc().nav_from_script {
        doc_mut().nav_from_script = false;
    } else {
        doc_mut().script_nav_chain = 0;
    }
    // Before the first byte. A navigation may go anywhere, including the
    // local router, because the new document is another origin the old page
    // cannot read. So the target's class is reported, not the class of the
    // page being left.
    tell_net_context(url);
    // A new navigation replaces the old one, and takes the page it was
    // loading for with it — a browser that keeps fetching the pictures of the
    // page you just left is spending the network on nothing.
    nav_cancel();
    subresources_cancel();
    // And the script-modified tree, or the next page would show the previous
    // one's — the cache is keyed by the HTML, not the tree.
    engine.set_scripted_dom(None);
    engine.set_hit_all(false);
    // The session belongs to the page being left: its handlers point at
    // nodes that are about to disappear.
    { doc_mut().js = None };

    // The built-in test page comes from the binary, not the network. From
    // here it takes the same path as a fetched document, minus the first
    // round trip.
    if selftest::matches(url) {
        deliver_builtin(engine, selftest::URL, selftest::HTML, push_hist);
        return;
    }

    let now = unix_now();
    let mut hdrs = String::new();
    let jar = cookies::header_for(url, now);
    if !jar.is_empty() {
        hdrs.push_str("Cookie: ");
        hdrs.push_str(&jar);
    }
    // Log which cookies are sent, by name. The value is a secret, the name
    // is not, and without it "5 held" says nothing.
    {
        let mit = cookies::names_for(url, now);
        let da = cookies::names_held(url);
        if !da.is_empty() || !mit.is_empty() {
            let mut m = String::from("[beak] cookies -> ");
            m.push_str(if mit.is_empty() { "(keine)" } else { &mit });
            if da != mit {
                m.push_str("   | fuer diesen Host da: ");
                m.push_str(if da.is_empty() { "(keine)" } else { &da });
            }
            log(&m);
        }
    }
    if !extra.is_empty() {
        if !hdrs.is_empty() {
            hdrs.push('\n');
        }
        hdrs.push_str(extra);
    }
    let t_nav = now_ms();
    {
        let d = doc_mut();
        d.nav_start_ms = t_nav;
        d.nav_reported = false;
        d.nav_stage_ms = t_nav;
    }
    let h = http_begin(method, url, &hdrs, body, HTML_CAP);
    {
        let d = doc_mut();
        d.nav_url = Some(url.to_string());
        d.nav_push_hist = push_hist;
        d.nav_stage = NavStage::Doc;
        d.nav_job = h;
    }
    if h < 0 {
        // Refused at the door — a malformed address, or the kernel's fetch
        // table full. That is as much a failed navigation as a refused
        // certificate, and it names itself through the same getter.
        nav_fail(url);
    }
}

/// The document did not arrive. Put a diagnostic page where it should have
/// been: a blank canvas is indistinguishable from a hung browser, and the
/// address bar keeps the URL that was asked for rather than one derived from
/// a response we never got.
fn nav_fail(url: &str) {
    // A remembered scroll position belongs to the page that did not come,
    // not to the error page in its place.
    doc_mut().scroll_want = 0;
    set_scroll(0);
    bump_content_gen("navigation");
    bump_nav_gen();
    show_error_page(url);
    mark_dirty();
    nav_clear();
}

/// Where the persistent cookies live.
///
/// In the private area, not `sys/config/beak`: a cookie is not a setting, it
/// is the login, and any app with READ could read config paths.
/// `priv/<module>/…` is the one place no capability opens — the name decides,
/// and the kernel assigns it. So beak keeps `RENDER | CANVAS | NET` and needs
/// no right to system-wide storage for its own state.
const COOKIE_FILE: &str = "priv/beak/cookies";
const COOKIE_CAP: usize = 96 * 1024;   // 256 cookies fit well below

/// Load the stored cookies into the jar — once at startup.
fn cookies_restore() {
    let now = unix_now();
    let mut buf = vec![0u8; COOKIE_CAP];
    let n = host::fetch(COOKIE_FILE, &mut buf).unwrap_or(0);
    // No file is the normal case on first start, not an error.
    if n == 0 { return }
    let Ok(text) = core::str::from_utf8(&buf[..n.min(COOKIE_CAP)]) else {
        log("[beak] cookies: gespeicherte Datei ist kein UTF-8 — uebergangen");
        return;
    };
    let got = cookies::load(text, now);
    if got > 0 {
        log(&alloc::format!("[beak] cookies: {got} aus dem Speicher zurueck"));
    }
}

/// Write the persistent cookies to disk. Session cookies stay out —
/// `Jar::serialize` decides that, not this function.
fn cookies_persist() {
    let now = unix_now();
    let text = cookies::serialize(now);
    if !host::store(COOKIE_FILE, text.as_bytes()) {
        log("[beak] cookies: konnten nicht gespeichert werden");
    }
}

/// File whatever `Set-Cookie` the response carried.
///
/// Cookies are scoped to where the response came from, after redirects —
/// filing them against the URL we asked for would scope a login cookie to the
/// wrong host.
fn file_cookies(asked: &str) {
    let now = unix_now();
    let mut buf = [0u8; HDR_CAP];
    let Some(h) = http_getter(npk_http_response_headers, &mut buf) else { return };
    let from = fetched_from().unwrap_or_else(|| asked.to_string());
    let before = cookies::count();
    cookies::store(&from, h, now);
    let after = cookies::count();
    if after != before || h.to_ascii_lowercase().contains("set-cookie") {
        // Only when something really changed — otherwise every response
        // would rewrite the same file.
        cookies_persist();
        let mut m = String::from("[beak] cookies: ");
        push_i64(&mut m, after as i64);
        m.push_str(" held");
        // And which ones the host now has: a count does not say whether the
        // cookie the session depends on is among them.
        let da = cookies::names_held(&from);
        if !da.is_empty() {
            m.push_str(" — hier: ");
            m.push_str(&da);
        }
        log(&m);
    }
}

/// Put a document from our own binary where the server's answer would go,
/// and from then on do nothing differently.
///
/// The rest of the chain (scripts, painting, history) must not know where
/// the bytes came from; otherwise the test page would test its own path
/// instead of the real one.
fn deliver_builtin(engine: &Engine, url: &str, html: &str, push_hist: bool) {
    let len = html.len().min(HTML_CAP);
    html_buf()[..len].copy_from_slice(&html.as_bytes()[..len]);
    HTML_LEN.store(len, Relaxed);
    // The page brings its own `<style>` and links nothing; the previous
    // page's CSS must go or it would style this one.
    CSS_LEN.store(0, Relaxed);
    doc_mut().nav_start_ms = now_ms();
    doc_mut().nav_reported = false;
    scroll_after_load();
    bump_content_gen("navigation");
    bump_nav_gen();
    set_url(url);
    if push_hist {
        hist_push(url);
    }
    nav_finish(engine);
}

/// Collect whichever half of the navigation has finished. Returns true if the
/// chrome needs redrawing — the address changed, or the stop button goes back
/// to being a reload button.
fn nav_pump(engine: &Engine) -> bool {
    let h = nav_job();
    if h < 0 {
        return false;
    }
    // 0 = still running. Everything else (done, failed, or a handle the
    // kernel no longer knows) is answered by collecting it.
    if http_poll(h) == 0 {
        return false;
    }
    match doc().nav_stage {
        NavStage::Doc => nav_document_arrived(engine),
        NavStage::Css => nav_stylesheets_arrived(engine),
        NavStage::Js => nav_scripts_arrived(engine),
        NavStage::Mod => nav_modules_arrived(engine),
        NavStage::Sheet => nav_sheets_arrived(engine),
        NavStage::CssImport => nav_css_imports_arrived(engine),
        NavStage::DynJs => nav_dynjs_arrived(engine),
    }
    true
}

fn nav_document_arrived(engine: &Engine) {
    let h = nav_job();
    let asked = nav_asked();
    let n = http_take(h, as_uninit(html_buf()));
    log_ms("fetch document", now_ms() - doc().nav_stage_ms);
    if n < 0 {
        nav_fail(&asked);
        return;
    }
    // Before anything downstream: the cookies belong to this response, and
    // the getters that carry them are overwritten by the next `take`.
    file_cookies(&asked);
    HTML_LEN.store((n as usize).min(HTML_CAP), Relaxed);
    // The bytes are not UTF-8 just because we would like them to be, and the
    // stylesheet scan below reads `html_str()`.
    decode_document();
    let len = HTML_LEN.load(Relaxed);
    if len == 0 {
        // Succeeded with nothing in it. The reader gets told, same as for a
        // refusal, and `nav_fail` does the bookkeeping below itself.
        nav_fail(&asked);
        return;
    }
    scroll_after_load();
    bump_content_gen("navigation");
    bump_nav_gen();
    // Relative sub-resources resolve against the URL the document came from,
    // not the one we asked for (RFC 3986 §5.1.3); otherwise every stylesheet
    // and image would repeat the document's own redirect.
    let base = fetched_from().unwrap_or(asked);
    set_url(&base);
    if doc().nav_push_hist {
        hist_push(url_str());
    }
    nav_begin_stylesheets(engine, &base);
}

/// Start the second round trip: every `<link rel=stylesheet>` of the document
/// that just landed, in one batch. They are render-blocking, so this is where
/// overlapping the round trips is worth the most. Bounded by CSS_CAP +
/// MAX_CSS_LINKS.
fn nav_begin_stylesheets(engine: &Engine, base: &str) {
    // An abandoned navigation must not leave sheets to the next one.
    {
        doc_mut().nav_css_parts = None;
        doc_mut().nav_css_want = None;
        doc_mut().nav_css_urls = None;
        doc_mut().nav_css_rounds = 0;
    }
    let links = beak_engine::stylesheet_links(html_str());
    let mut urls: Vec<String> = Vec::new();
    for href in links.iter() {
        if urls.len() >= MAX_CSS_LINKS {
            // Say so. A silently dropped stylesheet looks like a layout bug
            // and sends the next session hunting in the engine.
            log(&alloc::format!("[beak] stylesheet cap hit: {} of {} linked sheets used",
                MAX_CSS_LINKS, links.len()));
            break;
        }
        let abs = resolve(base, href);
        // The same sheet linked twice fetches identical bytes; dedupe on the
        // resolved URL, since two different hrefs can resolve to one file.
        if !urls.contains(&abs) {
            urls.push(abs);
        }
    }
    if urls.is_empty() {
        CSS_LEN.store(0, Relaxed);
        nav_finish(engine);
        return;
    }
    let h = begin_batch(&urls, CSS_CAP);
    if h < 0 {
        // No stylesheets is not a failed page — it renders against our UA
        // sheet — so this ends the navigation rather than diagnosing it.
        log("[beak] stylesheet fetch could not start — rendering unstyled");
        CSS_LEN.store(0, Relaxed);
        nav_finish(engine);
        return;
    }
    {
        doc_mut().nav_stage = NavStage::Css;
        doc_mut().nav_job = h;
        doc_mut().nav_css_count = urls.len();
        doc_mut().nav_stage_ms = now_ms();
        // An `@import` resolves against its own sheet's address, not the
        // document's, so the addresses have to survive the round trip.
        doc_mut().nav_css_urls = Some(urls);
        doc_mut().nav_css_rounds = 0;
    }
}

/// The JS session of this page.
///
/// Survives the script round: the handlers a script registers live in it.
/// A navigation drops it.
fn js_session() -> Option<&'static mut beak_engine::js::Session> {
    doc_mut().js.as_mut()
}
/// An `@import` chain may nest, but not endlessly, and a cycle must not stall
/// the navigation.
const MAX_IMPORT_ROUNDS: usize = 4;
const MAX_IMPORT_SHEETS: usize = 64;


/// A script waiting for its text — or already holding it.
enum PendingScript {
    /// Source, id (for a module: its URL), whether it is a module, and the
    /// node of the `<script>` — it is `document.currentScript`.
    Ready(String, String, bool, u32),
    /// Index in the batch, in request order, plus the URL — an error
    /// without an id says nothing.
    Fetching(usize, String, bool, u32),
}

/// How many external scripts one page may fetch.
///
/// Capped by count and by bytes (`SCRIPT_CAP`): 200 bundles must not cause
/// 200 round trips, and a 50 MB one must not overflow the buffer.
const MAX_SCRIPT_URLS: usize = 32;
const SCRIPT_CAP: usize = 8 * 1024 * 1024;
/// How large a module graph may grow, and in how many rounds.
///
/// Both ends can run away: a thousand small modules, and a chain that
/// fetches one more link every round.
const MAX_MODULE_URLS: usize = 256;
const MAX_MODULE_ROUNDS: usize = 24;
/// How often a page may add script-inserted stylesheets. Each round is a
/// round trip; a page that adds one every round would keep the load open.
const MAX_SHEET_ROUNDS: usize = 8;
/// How often a page may insert scripts from script. A split bundle loads its
/// tree in chains; without a cap, a page that adds one every round keeps the
/// load open.
const MAX_DYNJS_ROUNDS: usize = 12;

fn nav_stylesheets_arrived(engine: &Engine) {
    let h = nav_job();
    let want = doc().nav_css_count;
    // Fetched into a scratch buffer first because the bodies come back
    // concatenated, and they need a separator between them: without one, a
    // sheet not ending in `}` would merge into the next sheet's first rule.
    let mut scratch: Vec<u8> = Vec::with_capacity(CSS_CAP);
    let spans = take_batch(h, &mut scratch.spare_capacity_mut()[..CSS_CAP], want);
    let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
    // SAFETY: the kernel initialised the bytes up to the end of the last
    // span, and the capacity is `CSS_CAP`.
    unsafe { scratch.set_len(total.min(CSS_CAP)) };

    // The bodies are kept as parts rather than written straight into
    // `CSS_BUF`: an `@import` cascades ahead of the sheet that imported it, so
    // the buffer can only be assembled once every round of imports is in.
    let urls = doc_mut().nav_css_urls.take().unwrap_or_default();
    let mut parts: Vec<(String, Vec<u8>, bool)> = Vec::with_capacity(spans.len());
    for (k, (off, n)) in spans.iter().copied().enumerate() {
        if n == 0 || off + n > scratch.len() {
            continue;
        }
        let url = urls.get(k).cloned().unwrap_or_default();
        parts.push((url, scratch[off..off + n].to_vec(), false));
    }
    log_ms("fetch stylesheets", now_ms() - doc().nav_stage_ms);
    { doc_mut().nav_css_parts = Some(parts) };
    if !css_import_pump() {
        css_assemble();
        nav_finish(engine);
    }
}

/// Start one round of `@import` fetches, if any sheet still has unexamined
/// bytes. `true` when a round is in flight.
///
/// Round-based like the module graph: a sheet only names its own imports
/// once it has arrived. A `main.css` holding nothing but `@import`s is the
/// shape this exists for.
fn css_import_pump() -> bool {
    let rounds = doc().nav_css_rounds;
    let parts = doc_mut().nav_css_parts.as_mut();
    let Some(parts) = parts else { return false };
    if rounds >= MAX_IMPORT_ROUNDS {
        log(&alloc::format!("[beak] @import: bei {MAX_IMPORT_ROUNDS} Runden gekappt"));
        return false;
    }
    let mut want: Vec<(usize, String)> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for i in 0..parts.len() {
        if parts[i].2 {
            continue;
        }
        parts[i].2 = true;
        let (base, body) = (parts[i].0.clone(), parts[i].1.clone());
        let Ok(text) = core::str::from_utf8(&body) else { continue };
        for href in beak_engine::import_urls(text) {
            if parts.len() + want.len() >= MAX_IMPORT_SHEETS {
                log(&alloc::format!("[beak] @import: bei {MAX_IMPORT_SHEETS} Blaettern gekappt"));
                break;
            }
            let abs = resolve(&base, &href);
            // A sheet already in the list is a cycle or a repeat; either way
            // its bytes are here once and that is enough.
            if parts.iter().any(|(u, _, _)| *u == abs) || want.iter().any(|(_, u)| *u == abs) {
                continue;
            }
            want.push((i, abs.clone()));
            urls.push(abs);
        }
    }
    if urls.is_empty() {
        return false;
    }
    let h = begin_batch(&urls, CSS_CAP);
    if h < 0 {
        log("[beak] @import: Holen konnte nicht starten");
        return false;
    }
    log(&alloc::format!("[beak] @import: {} Blaetter, Runde {}", urls.len(), rounds + 1));
    {
        doc_mut().nav_stage = NavStage::CssImport;
        doc_mut().nav_job = h;
        doc_mut().nav_css_want = Some(want);
        doc_mut().nav_css_rounds = rounds + 1;
        doc_mut().nav_stage_ms = now_ms();
    }
    true
}

fn nav_css_imports_arrived(engine: &Engine) {
    let h = nav_job();
    let want = doc_mut().nav_css_want.take().unwrap_or_default();
    let mut scratch: Vec<u8> = Vec::with_capacity(CSS_CAP);
    let spans = take_batch(h, &mut scratch.spare_capacity_mut()[..CSS_CAP], want.len());
    let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
    // SAFETY: as in `nav_stylesheets_arrived`.
    unsafe { scratch.set_len(total.min(CSS_CAP)) };
    let (mut ok, mut bad) = (0usize, 0usize);
    if let Some(parts) = doc_mut().nav_css_parts.as_mut() {
        // Insert from the back: each insertion shifts everything after it;
        // in descending order the remaining, smaller positions stay valid.
        for k in (0..want.len()).rev() {
            let (owner, url) = &want[k];
            let (off, n) = spans.get(k).copied().unwrap_or((0, 0));
            if n == 0 || off + n > scratch.len() {
                bad += 1;
                log(&alloc::format!("[beak] @import gescheitert: {url}"));
                continue;
            }
            ok += 1;
            let at = (*owner).min(parts.len());
            parts.insert(at, (url.clone(), scratch[off..off + n].to_vec(), false));
        }
    }
    log(&alloc::format!("[beak] @import: {ok} geholt, {bad} gescheitert, {} ms",
                        now_ms() - doc().nav_stage_ms));
    if !css_import_pump() {
        css_assemble();
        nav_finish(engine);
    }
}

/// Write the assembled sheets into `CSS_BUF`, in the order the list holds —
/// which is cascade order, imports ahead of their importer.
fn css_assemble() {
    let parts = doc_mut().nav_css_parts.take().unwrap_or_default();
    CSS_LEN.store(0, Relaxed);
    for (_, body, _) in &parts {
        if !css_append(body) {
            break;
        }
    }
}

/// Document and stylesheets are both in: the page may be drawn, and its
/// images may start arriving.
fn nav_finish(engine: &Engine) {
    decode_css();
    log_font_faces(css_str());
    // Collect the scripts first. If external ones exist, the navigation goes
    // into a third stage and ends only after it — otherwise the page would
    // be finished before its scripts built it.
    if nav_begin_scripts(engine) { return; }
    nav_done();
}

/// The page is finished: paint and fetch images.
fn nav_done() {
    doc_mut().images_dirty = true;
    mark_dirty();
    nav_clear();
}

/// Collect the page's scripts and request the external ones.
///
/// Returns true if a round trip is running — it continues in `nav_pump`.
/// Otherwise the scripts have already run.
fn nav_begin_scripts(engine: &Engine) -> bool {
    use beak_engine::js::dombind::ScriptRef;
    let dom = beak_engine::parse(html_str());
    let doc = beak_engine::js::dombind::Doc::from_dom(&dom);
    let refs = beak_engine::js::dombind::page_scripts(&doc);
    if refs.is_empty() {
        engine.set_scripted_dom(None);
        return false;
    }
    let base = url_str().to_string();
    let mut list: Vec<PendingScript> = Vec::with_capacity(refs.len());
    let mut urls: Vec<String> = Vec::new();
    let mut inline_n = 0usize;
    for r in refs {
        match r {
            ScriptRef::Inline(t, m, node) => {
                inline_n += 1;
                // An inline module gets its own URL: it is the loader key
                // and what `import.meta.url` says, and relative specifiers
                // resolve against it.
                let label = if m { alloc::format!("{base}#inline{inline_n}") }
                            else { alloc::format!("inline #{inline_n}") };
                list.push(PendingScript::Ready(t, label, m, node));
            }
            ScriptRef::External(src, m, node) => {
                if urls.len() >= MAX_SCRIPT_URLS {
                    log(&alloc::format!("[beak] script cap hit: {} external scripts used", MAX_SCRIPT_URLS));
                    continue;
                }
                let u = resolve(&base, &src);
                list.push(PendingScript::Fetching(urls.len(), u.clone(), m, node));
                urls.push(u);
            }
        }
    }
    if urls.is_empty() {
        return run_scripts(engine, list);
    }
    let h = begin_batch(&urls, SCRIPT_CAP);
    if h < 0 {
        // Not fetchable: the inline ones still run. A page without its
        // bundles is less than whole, but more than nothing.
        log("[beak] external scripts could not be fetched — running inline only");
        return run_scripts(engine, list);
    }
    {
        doc_mut().nav_scripts = Some(list);
        doc_mut().nav_js_count = urls.len();
        doc_mut().nav_stage = NavStage::Js;
        doc_mut().nav_job = h;
        doc_mut().nav_stage_ms = now_ms();
    }
    true
}

fn nav_scripts_arrived(engine: &Engine) {
    let h = nav_job();
    let want = doc().nav_js_count;
    let mut list = doc_mut().nav_scripts.take().unwrap_or_default();
    let dst = img_fetch_buf();
    let spans = take_batch(h, as_uninit(&mut dst[..SCRIPT_CAP.min(IMG_FETCH_CAP)]), want);
    for p in list.iter_mut() {
        let (k, label, is_mod, node) = match p {
            PendingScript::Fetching(k, l, m, n) => (*k, core::mem::take(l), *m, *n),
            _ => continue,
        };
        let (off, n) = spans.get(k).copied().unwrap_or((0, 0));
        let text = if n == 0 { String::new() } else {
            let bytes = &dst[off..off + n];
            // Not decodable means not executed. Running half a script is
            // worse than skipping it.
            match core::str::from_utf8(bytes) {
                Ok(t) => String::from(t),
                Err(e) => {
                    log(&alloc::format!(
                        "[beak]   script FAIL {label}: {n} B, aber kein UTF-8 (@{})",
                        e.valid_up_to()));
                    String::new()
                }
            }
        };
        *p = PendingScript::Ready(text, label, is_mod, node);
    }
    log_ms("fetch scripts", now_ms() - doc().nav_stage_ms);
    if !run_scripts(engine, list) { nav_done(); }
}

/// A byte offset as `line:column` plus the surrounding source.
///
/// An error that only says `@41822` is useless on minified code. The line
/// itself is not shown whole — minified code has very long lines.
fn src_pos(src: &str, at: usize) -> String {
    let mut at = at.min(src.len());
    while at > 0 && !src.is_char_boundary(at) { at -= 1; }
    let before = &src[..at];
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let ls = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = at - ls + 1;
    let le = src[at..].find('\n').map(|i| at + i).unwrap_or(src.len());
    let mut from = at.saturating_sub(48).max(ls);
    while from < at && !src.is_char_boundary(from) { from += 1; }
    let mut to = (at + 48).min(le);
    while to > at && !src.is_char_boundary(to) { to -= 1; }
    alloc::format!("{line}:{col} ...{}<<HIER>>{}...", &src[from..at], &src[at..to])
}

/// Copy what the page wrote to `console` to the serial log.
///
/// Prefixed, so the log shows who spoke: these are foreign bytes, not
/// beak's voice.
fn drain_console(sess: &mut beak_engine::js::Session) {
    for line in sess.interp.take_console() {
        let mut m = String::from("[seite] ");
        m.push_str(&line);
        log(&m);
    }
}

/// Move what the page set with `document.cookie = …` into the jar, and hand
/// the resulting view back.
///
/// After every entry point into the engine, not only after loading: a click
/// sets cookies just like a startup script. The engine holds no jar — what
/// applies is decided by `cookies`, including domain, path and `HttpOnly`.
fn sync_cookies(sess: &mut beak_engine::js::Session) {
    let url = url_str();
    if url.is_empty() {
        return;
    }
    let now = unix_now();
    let sets = sess.interp.take_cookie_sets();
    for decl in &sets {
        cookies::store_from_script(url, decl, now);
    }
    if !sets.is_empty() {
        // A cookie set by script is as persistent as one set by header —
        // `document.cookie = "…; expires=…"` is the same contract.
        cookies_persist();
        let mut m = String::from("[beak] cookies: Seite setzte ");
        push_i64(&mut m, sets.len() as i64);
        m.push_str(", ");
        push_i64(&mut m, cookies::count() as i64);
        m.push_str(" held");
        log(&m);
    }
    sess.interp.set_cookies(cookies::script_header_for(url, now));
}

/// Perform the scroll the page requested.
///
/// The engine has no window, so it only records the wish; the scroll
/// position lives here. beak does not scroll horizontally, so the x request
/// is dropped deliberately.
fn sync_scroll(sess: &mut beak_engine::js::Session) {
    let Some((_x, y)) = sess.interp.take_scroll() else { return };
    let Some(y) = y else { return };
    if !y.is_finite() { return }
    let want = y.max(0.0) as i32;
    if want == scroll_y() { return }
    set_scroll(want);
    mark_dirty();
}

/// Collect and perform what the page asked of the history.
///
/// The engine only records intents, because it has no history and must not
/// invent one. This is where they take effect, and where the engine learns
/// the current length so `history.length` is not stuck at 1.
///
/// Called at the same places as `sync_cookies`: after every entry point.
fn sync_history(engine: &Engine, sess: &mut beak_engine::js::Session) {
    use beak_engine::js::interp::HistoryOp;
    for op in sess.interp.take_history_ops() {
        match op {
            // `pushState`/`replaceState` do not navigate — they only rewrite
            // the URL, so an application can switch views without fetching.
            HistoryOp::Push { ref url } | HistoryOp::Replace { ref url } => {
                let replace = matches!(op, HistoryOp::Replace { .. });
                if url.is_empty() {
                    continue;
                }
                let abs = resolve(url_str(), url);
                // Same origin only. A page may rewrite its address bar, but
                // not to a foreign origin — that would be an invisible forgery.
                if origin_of(&abs) != origin_of(url_str()) {
                    log("[beak] history: Adresse fremder Herkunft abgelehnt");
                    continue;
                }
                set_url(&abs);
                if !replace {
                    hist_push(&abs);
                }
            }
            // `go(n)` really navigates; recording the intent and doing
            // nothing would let the page believe it went back.
            //
            // beak's history moves one step at a time, so step as often as
            // requested and stop at the end.
            HistoryOp::Go(n) => {
                let mut target: Option<String> = None;
                for _ in 0..n.unsigned_abs().min(HIST_MAX as u32) {
                    match if n < 0 { hist_back() } else { hist_forward() } {
                        Some(u) => target = Some(String::from(u)),
                        None => break,
                    }
                }
                if let Some(u) = target {
                    // `push_hist` must be false here, or going back would
                    // grow the history and never get out.
                    nav_begin(engine, "GET", &u, &[], "", false);
                    return;
                }
            }
        }
    }
    let count = doc().hist.len();
    sess.interp.set_history(count.max(1) as f64, beak_engine::js::value::Value::Null);
}

/// How many navigations a page may trigger itself in a row.
///
/// Not against slow pages but against `location.href = "/a"` on `/a`: an
/// endless loop costing a round trip per turn that looks like a hung
/// browser. Short chains are normal.
const SCRIPT_NAV_MAX: u32 = 8;

/// Collect what the page requested via `location` and actually navigate.
///
/// Unlike `sync_history`: `pushState` rewrites the URL and keeps the
/// document, `location.replace` discards it and fetches a new one.
///
/// Returns true if it navigated — the current document is gone and the
/// caller must stop working on it.
fn sync_nav(engine: &Engine) -> bool {
    let Some(sess) = js_session() else { return false };
    let Some(n) = sess.interp.take_nav() else { return false };
    // Only what can yield a document. The engine already rejected
    // `javascript:` and `data:`; the rest (`mailto:`, `file:`, `blob:`) is
    // dropped here because `nav_begin` would send it to the network and the
    // answer would be a blank page that looks like a server error.
    let ok = n.url.starts_with("https://") || n.url.starts_with("http://")
             || selftest::matches(&n.url);
    if !ok {
        log(&alloc::format!("[beak] Navigation abgelehnt (Schema): {}", n.url));
        return false;
    }
    let chain = doc().script_nav_chain + 1;
    if chain > SCRIPT_NAV_MAX {
        log(&alloc::format!("[beak] Navigation abgebrochen: {SCRIPT_NAV_MAX} Sprünge in Folge, zuletzt {}", n.url));
        return false;
    }
    doc_mut().script_nav_chain = chain;
    let mut m = String::from("[beak] location.");
    m.push_str(if n.reload { "reload()" } else if n.replace { "replace()" } else { "assign()" });
    m.push_str(" -> ");
    m.push_str(&n.url);
    if chain > 1 { m.push_str(" ("); push_i64(&mut m, chain as i64); m.push_str(". in Folge)"); }
    log(&m);
    doc_mut().nav_from_script = true;
    // `replace` and `reload` add no entry: otherwise "back" could never
    // leave a page that replaces itself.
    nav_begin(engine, "GET", &n.url, &[], "", !n.replace && !n.reload);
    true
}

/// Run the page's scripts and hand the modified tree to the layout.
///
/// Only here, after the stylesheets: a script reads classes and sizes, and a
/// half-built document would have both wrong. Runs once per navigation, not
/// per arriving image.
///
/// What a script does stays in the sandbox: the engine has a step cap, a
/// call depth limit and no access to host functions.
///
/// Returns true if a module round is running — then the navigation is not
/// finished yet.
fn run_scripts(engine: &Engine, list: Vec<PendingScript>) -> bool {
    doc_mut().script_t0 = now_ms();
    let dom = beak_engine::parse(html_str());
    let doc = beak_engine::js::dombind::Doc::from_dom(&dom);
    let mut sess = beak_engine::js::Session::new(SCRIPT_STEPS);
    sess.interp.deadline = Some(script_time_left);
    // Lay out on demand. Without it every box query answers from the last
    // frame, and an element a script just inserted reports 0 — forever, on a
    // page that measures only once. See `docs/plan/BROWSER_RELAYOUT_ON_DEMAND.md`.
    sess.interp.relayout = Some(host_relayout);
    arm_script_budget();
    sess.interp.set_document(doc);
    // The location belongs to the host — the URL the document came from,
    // not the requested one: a redirect changes the origin and with it the
    // cookies.
    sess.interp.set_location(url_str());
    // The cookies this document may see. `HttpOnly` stays out: that flag is
    // the defence against foreign code on the page.
    if !url_str().is_empty() {
        let now = unix_now();
        sess.interp.set_cookies(cookies::script_header_for(url_str(), now));
    }
    // The cascade context for `getComputedStyle`. Without it the answer
    // comes from the inline style only; with it the engine computes the same
    // cascade the layout does, on the same tree and sheet.
    if let Some((_, _, w, _)) = canvas_rect() {
        let media = beak_engine::css::Media::new(w as f32, query_theme().is_dark());
        let sheet = beak_engine::css::collect_all(&dom, css_str(), media);
        sess.interp.set_style_context(beak_engine::js::interp::StyleCtx {
            sheet: alloc::rc::Rc::new(sheet),
            theme: engine.theme(),
            viewport_w: w as f32,
        });
    }
    // The window size belongs to the host. Without it there is no
    // `innerWidth`, and a page choosing its narrow variant by it fails.
    if let Some((_, _, w, h)) = canvas_rect() {
        // Pass the colour scheme too: `matchMedia` must agree with the
        // cascade, or the script picks a variant the layout does not paint.
        sess.interp.set_media(w as f64, h as f64, query_theme().is_dark());
    }
    // `Math.random` gets a real seed. Without it every page gets the same
    // sequence, and the engine deliberately invents none.
    sess.interp.seed_random(now_ms() as u64 ^ 0x9E37_79B9_7F4A_7C15);
    // And a real clock. The engine has none; without this `Date.now()` is in
    // 1970.
    sess.interp.epoch_ms = unix_now() as f64 * 1000.0;
    let (mut ran, mut failed, mut bytes) = (0usize, 0usize, 0usize);
    // Module entries, in document order. They run after all classic scripts:
    // `type="module"` is deferred by spec.
    let mut entries: Vec<String> = Vec::new();
    for p in &list {
        let (src, label, is_mod, node) = match p {
            PendingScript::Ready(s, l, m, n) => (s, l.as_str(), *m, *n),
            PendingScript::Fetching(_, l, _, _) => {
                failed += 1;
                log(&alloc::format!("[beak]   script FAIL {l}: nie angekommen"));
                continue;
            }
        };
        if src.is_empty() { failed += 1; continue; }
        bytes += src.len();
        if is_mod {
            match beak_engine::js::parse(src, true) {
                Ok(p) => {
                    sess.interp.add_module(label, alloc::rc::Rc::new(p));
                    entries.push(label.to_string());
                }
                Err(e) => {
                    failed += 1;
                    log(&alloc::format!("[beak]   script FAIL {label}: SyntaxError: {} @{}",
                                        e.msg, src_pos(src, e.at)));
                }
            }
            continue;
        }
        let prog = match beak_engine::js::parse(src, false) {
            Ok(p) => p,
            // A module is also a script — the file does not say which, so try
            // both.
            Err(e) => match beak_engine::js::parse(src, true) {
                Ok(p) => p,
                Err(em) => {
                    failed += 1;
                    let mut w = alloc::format!("SyntaxError: {} @{}", e.msg, src_pos(src, e.at));
                    if em.msg != e.msg {
                        w.push_str(&alloc::format!(" | als Modul: {} @{}", em.msg, src_pos(src, em.at)));
                    }
                    log(&alloc::format!("[beak]   script FAIL {label}: {w}"));
                    continue;
                }
            },
        };
        // A failing script must not take the next ones down, as in other
        // browsers.
        // `document.currentScript` points at this node while it runs. A
        // module never gets here; for it the answer is `null` (HTML §4.12.1).
        sess.interp.current_script = Some(node);
        let r = sess.run(&prog);
        sess.interp.current_script = None;
        match r {
            Ok(()) => ran += 1,
            Err(e) => {
                failed += 1;
                log(&alloc::format!("[beak]   script FAIL {label}: {e}"));
            }
        }
    }
    doc_mut().js = Some(sess);
    doc_mut().script_tally = (ran, failed, bytes);
    doc_mut().nav_mod_entries = if entries.is_empty() { None } else { Some(entries) };
    doc_mut().nav_mod_rounds = 0;
    module_pump(engine)
}


/// How many fonts a page may fetch in one round, and how many bytes
/// together. The font round is its own job, not the navigation's: which
/// fonts a page needs is known only after the first layout, as with images.
const MAX_FONT_URLS: usize = 12;
const FONT_CAP: usize = 4 * 1024 * 1024;

/// How many `fetch` requests may be in flight at once.
///
/// The host holds a response buffer per request; what does not fit waits
/// for the next round.
const MAX_FETCH_INFLIGHT: usize = 6;
const FETCH_CAP: usize = 2 * 1024 * 1024;

fn fetch_jobs() -> &'static mut Vec<(u32, i32)> {
    &mut doc_mut().fetch_jobs
}

/// The header block of the last collected response.
fn response_headers() -> String {
    let mut buf = [0u8; HDR_CAP];
    http_getter(npk_http_response_headers, &mut buf).unwrap_or("").to_string()
}

/// A global for state that has no context to be passed through.
struct Single<T>(RefCell<T>);
// SAFETY: wasm is single-threaded; there is no second thread to share with.
unsafe impl<T> Sync for Single<T> {}

/// The WebSockets: one read buffer for all, used in turn, and the open
/// connections as `(engine id, TLS handle)`.
struct Sockets {
    buf: Vec<u8>,
    jobs: Vec<(u32, i32)>,
}
static SOCKETS: Single<Sockets> = Single(RefCell::new(Sockets { buf: Vec::new(), jobs: Vec::new() }));
const WS_BUF: usize = 16 * 1024;

/// Drive the connections: open what is pending, collect what arrived, send
/// what the engine queued.
///
/// `true` if something happened — then a round of microtasks is worthwhile.
fn pump_websockets(sess: &mut beak_engine::js::Session) -> bool {
    let mut moved = false;
    let mut sockets = SOCKETS.0.borrow_mut();
    let Sockets { buf, jobs } = &mut *sockets;
    // 1. New connections. The handshake blocks; it is the same price the
    //    HTTP path pays in `open_tls`, once per connection.
    let want: Vec<_> = core::mem::take(&mut sess.interp.pending_sockets);
    for w in want {
        if !w.secure {
            // `ws://` without TLS is not driven: the engine allows it only
            // from an unencrypted page, and there is no plaintext stream.
            // Said in the log, not silently.
            log("[beak] WebSocket: ws:// ohne TLS wird nicht gefahren");
            beak_engine::js::websocket::host_bytes(&mut sess.interp, w.id, None);
            continue;
        }
        let h = tls_connect(&w.host, w.port);
        if h < 0 {
            let mut m = String::from("[beak] WebSocket: Verbindung zu ");
            m.push_str(&w.host);
            m.push_str(" gescheitert");
            log(&m);
            beak_engine::js::websocket::host_bytes(&mut sess.interp, w.id, None);
            moved = true;
            continue;
        }
        if tls_send(h, &w.hello) < 0 {
            tls_close(h);
            beak_engine::js::websocket::host_bytes(&mut sess.interp, w.id, None);
            moved = true;
            continue;
        }
        // A successful open must log itself; otherwise a log looks the same
        // as one where nothing was attempted.
        log(&alloc::format!("[beak] WebSocket: {}:{} verbunden, Handschlag raus ({} B)",
                            w.host, w.port, w.hello.len()));
        jobs.push((w.id, h));
        moved = true;
    }
    // 2. Read and write. One buffer per round suffices: what does not fit
    //    stays in the kernel and arrives next frame.
    // Not on the stack: 16 KB zeroed per call, every frame, right under the
    // recursive layout.
    if buf.is_empty() {
        buf.resize(WS_BUF, 0);
    }
    let mut tot: Vec<(u32, i32)> = Vec::new();
    for (id, h) in jobs.clone() {
        // Drain, not peek. `npk_tls_recv` returns 0 exactly when nothing
        // complete is left; stopping after the first record would leave the
        // rest in the kernel until its buffer fills. The cap guards the other
        // direction: a peer that never stops must not eat the whole frame.
        for _ in 0..64 {
            let n = tls_recv(h, buf);
            if n < 0 {
                // Closed or broken — the engine turns this into 1006.
                log("[beak] WebSocket: Leitung zu");
                beak_engine::js::websocket::host_bytes(&mut sess.interp, id, None);
                tls_close(h);
                tot.push((id, h));
                moved = true;
                break;
            }
            if n == 0 { break }
            beak_engine::js::websocket::host_bytes(&mut sess.interp, id, Some(&buf[..(n as usize).min(WS_BUF)]));
            moved = true;
        }
        if tot.iter().any(|(x, _)| *x == id) { continue }
        if let Some(out) = beak_engine::js::websocket::take_out_for(&mut sess.interp, id) {
            if tls_send(h, &out) < 0 {
                beak_engine::js::websocket::host_bytes(&mut sess.interp, id, None);
                tls_close(h);
                tot.push((id, h));
            }
            moved = true;
        }
    }
    jobs.retain(|(id, _)| !tot.iter().any(|(x, _)| x == id));
    moved
}

/// One round over the page's `fetch` requests.
///
/// The engine fetches nothing — it queues the request and this fetches it.
/// Same path as `pending_sheets`/`sheet_done`, but with one handle per
/// request instead of per round: `fetch` is concurrent.
///
/// An abort is real: `controller.abort()` records the id and the connection
/// is cancelled, so the page does not keep loading what nobody reads.
///
/// Returns true if something arrived — page code ran and the tree may have
/// changed.
fn pump_fetches() -> bool {
    let Some(sess) = js_session() else { return false };
    let mut landed = false;

    for id in sess.interp.take_aborted_fetches() {
        let jobs = fetch_jobs();
        if let Some(k) = jobs.iter().position(|(n, _)| *n == id) {
            http_cancel(jobs[k].1);
            jobs.remove(k);
        }
    }

    let mut k = 0;
    while k < fetch_jobs().len() {
        let (id, h) = fetch_jobs()[k];
        if http_poll(h) == 0 { k += 1; continue }
        fetch_jobs().remove(k);
        let mut buf: Vec<u8> = Vec::with_capacity(FETCH_CAP);
        let n = http_take(h, &mut buf.spare_capacity_mut()[..FETCH_CAP]);
        landed = true;
        if n < 0 {
            let why = last_error().map(|(a, _)| a).unwrap_or_else(|| String::from("request failed"));
            beak_engine::js::fetch::fetch_failed(&mut sess.interp, id, &why);
            continue;
        }
        // SAFETY: the kernel initialised the first `n` bytes, and the
        // capacity is `FETCH_CAP`.
        unsafe { buf.set_len((n as usize).min(FETCH_CAP)) };
        // The cookies belong to this response, and the getters carrying them
        // are overwritten by the next `take`.
        let from = fetched_from().unwrap_or_default();
        file_cookies(&from);
        let status = http_status().max(0) as u16;
        let hdrs = response_headers();
        let body = alloc::string::String::from_utf8_lossy(&buf).into_owned();
        beak_engine::js::fetch::fetch_done(&mut sess.interp, id, status, &from, &hdrs, body);
    }

    let mut queued = sess.interp.take_pending_fetches();
    let base = url_str().to_string();
    let mut rest: Vec<beak_engine::js::fetch::PendingFetch> = Vec::new();
    for f in queued.drain(..) {
        if fetch_jobs().len() >= MAX_FETCH_INFLIGHT { rest.push(f); continue }
        let url = resolve(&base, &f.url);
        // The host separates headers with `\n`; the engine holds the raw
        // block as it goes over the wire, with `\r\n`.
        let mut hdrs = f.headers.replace("\r\n", "\n").trim_end_matches('\n').to_string();
        // Cookies go along for the same origin only (rule K3), and the
        // engine lets only same-origin requests through. Across an origin
        // boundary they would be the ambient authority
        // `BROWSER_FETCH_ORIGIN.md` §3.1 warns about.
        let jar = cookies::header_for(&url, unix_now());
        if !jar.is_empty() {
            if !hdrs.is_empty() { hdrs.push('\n'); }
            hdrs.push_str("Cookie: ");
            hdrs.push_str(&jar);
        }
        let body = f.body.clone().unwrap_or_default();
        let h = http_begin(&f.method, &url, &hdrs, body.as_bytes(), FETCH_CAP);
        if h < 0 {
            beak_engine::js::fetch::fetch_failed(&mut sess.interp, f.id, "no handle");
            landed = true;
        } else {
            fetch_jobs().push((f.id, h));
        }
    }
    for f in rest { sess.interp.pending_fetches.push(f); }

    landed
}

/// One round over the page's fonts. Returns true if something arrived — then
/// a re-layout is needed, because every width changes.
fn pump_fonts(engine: &Engine) -> bool {
    let h = doc().font_job;
    let mut loaded = false;
    if h >= 0 {
        if http_poll(h) == 0 { return false }
        doc_mut().font_job = -1;
        let want = doc_mut().font_want.take().unwrap_or_default();
        let mut buf: Vec<u8> = Vec::with_capacity(FONT_CAP);
        let spans = take_batch(h, &mut buf.spare_capacity_mut()[..FONT_CAP], want.len());
        let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
        // SAFETY: the kernel initialised the bytes up to the end of the last
        // span, and the capacity is `FONT_CAP`.
        unsafe { buf.set_len(total.min(FONT_CAP)) };
        let (mut ok, mut bad) = (0usize, 0usize);
        for (k, (url, family, weight, italic)) in want.iter().enumerate() {
            let (off, n) = spans.get(k).copied().unwrap_or((0, 0));
            if n == 0 || off + n > buf.len() {
                log(&alloc::format!("[beak]   Schrift FEHLT {url}"));
                bad += 1;
                continue;
            }
            if engine.add_font(*family, *weight, *italic, &buf[off..off + n]) {
                ok += 1;
                loaded = true;
            } else {
                // Not silent: a font beak cannot read is why the page looks
                // different.
                log(&alloc::format!("[beak]   Schrift NICHT LESBAR {url} ({n} B)"));
                bad += 1;
            }
        }
        log(&alloc::format!("[beak] Schriften: {ok} geladen, {bad} gescheitert, {} ms",
                            now_ms() - doc().nav_stage_ms));
    }
    if doc().font_job >= 0 { return loaded }
    // Inline faces first: their bytes are already in the sheet, so they cost
    // no round trip.
    if engine.load_inline_fonts() { loaded = true; }
    let mut want = engine.take_pending_fonts();
    if want.is_empty() { return loaded }
    want.truncate(MAX_FONT_URLS);
    let base = url_str().to_string();
    let urls: Vec<String> = want.iter().map(|(u, ..)| resolve(&base, u)).collect();
    let h = begin_batch(&urls, FONT_CAP);
    if h < 0 {
        log("[beak] Schriften konnten nicht angefordert werden");
        return loaded;
    }
    doc_mut().font_job = h;
    doc_mut().font_want = Some(want);
    doc_mut().nav_stage_ms = now_ms();
    loaded
}

/// Dispatch `load` — after the first paint, when the boxes exist.
///
/// Returns true if the tree changed.
fn fire_load(engine: &Engine, page: &Page) -> bool {
    if !doc().load_pending { return false }
    doc_mut().load_pending = false;
    // The handler must see what the user already typed; otherwise it reads
    // the default value and may write it back.
    push_control_values(page);
    let Some(sess) = js_session() else { return false };
    let Some(dn) = sess.interp.doc.as_ref().map(|d| d.doc) else { return false };
    arm_script_budget();
    let t0 = now_ms();
    let _ = beak_engine::js::dombind::dispatch(&mut sess.interp, "load", &[dn]);
    let timers = sess.interp.run_timers();
    sync_cookies(sess);
    sync_history(engine, sess);
    sync_scroll(sess);
    drain_console(sess);
    let changed = sess.interp.doc.as_ref().is_some_and(|d| d.dirty);
    if changed {
        if let Some(d) = sess.interp.doc.as_mut() {
            engine.set_scripted_dom(Some(d.to_dom()));
        }
        bump_content_gen("load");
        mark_dirty();
    }
    log(&alloc::format!("[beak] load: {}, {timers} Zeitgeber, {} ms",
                        if changed { "Baum geaendert" } else { "unveraendert" },
                        now_ms() - t0));
    changed
}

/// Deliver what `ResizeObserver`/`IntersectionObserver` measured.
///
/// Only when something is queued: on a page without observers this is two
/// empty lists and nothing else.
fn pump_box_observers(engine: &Engine) {
    let Some(sess) = js_session() else { return };
    if !sess.interp.box_observations_pending() { return }
    arm_script_budget();
    // `run_timers` starts with the microtasks, and delivery sits there — a
    // callback may itself schedule a `setTimeout` and is served in the same
    // round.
    let timers = sess.interp.run_timers();
    let _ = timers;
    sync_cookies(sess);
    sync_history(engine, sess);
    sync_scroll(sess);
    drain_console(sess);
    if sess.interp.doc.as_ref().is_some_and(|d| d.dirty) {
        if let Some(d) = sess.interp.doc.as_mut() {
            engine.set_scripted_dom(Some(d.to_dom()));
        }
        bump_content_gen("observer");
        // Guard against the loop: a callback that changes the size of its
        // own target measures something new next frame ("ResizeObserver
        // loop"). Without the guard that would be a layout every frame.
        //
        // The tree is still taken; only the immediate repaint is skipped, so
        // the cycle settles and the next input shows the state.
        let n = doc().obs_rounds + 1;
        doc_mut().obs_rounds = n;
        if n <= OBS_ROUNDS_MAX {
            mark_dirty();
        } else if n == OBS_ROUNDS_MAX + 1 {
            log("[beak] Beobachter: der Rueckruf aendert, was er misst —                  Neumalen ausgesetzt");
        }
    } else {
        doc_mut().obs_rounds = 0;
    }
}

/// How many frames in a row an observer callback may change the tree before
/// repainting is suspended.
const OBS_ROUNDS_MAX: u32 = 8;

/// One round over the module graph: what is still missing?
///
/// Returns true if a round trip is running — it continues in `nav_pump`.
/// Otherwise the graph is closed and everything is evaluated.
fn module_pump(engine: &Engine) -> bool {
    let entries = match doc().nav_mod_entries.clone() {
        Some(e) => e,
        None => { finish_scripts(engine); return false }
    };
    let Some(sess) = js_session() else { finish_scripts(engine); return false };
    // Walk from the entries and resolve every specifier — the loader knows
    // only absolute URLs, resolving belongs to the host.
    let mut seen: Vec<String> = Vec::new();
    let mut queue = entries;
    let mut missing: Vec<String> = Vec::new();
    while let Some(u) = queue.pop() {
        if seen.iter().any(|x| x == &u) { continue }
        seen.push(u.clone());
        if !sess.interp.has_module(&u) {
            if !missing.iter().any(|x| x == &u) { missing.push(u); }
            continue;
        }
        for spec in sess.interp.module_requests(&u) {
            let r = resolve(&u, &spec);
            sess.interp.map_module_dep(&u, &spec, &r);
            queue.push(r);
        }
    }
    let rounds = doc().nav_mod_rounds;
    // Which cap was hit belongs in the message; the round cap and the URL cap
    // are different diagnoses.
    let cap = if rounds >= MAX_MODULE_ROUNDS { Some("Runden") }
              else if seen.len() > MAX_MODULE_URLS { Some("Adressen") }
              else { None };
    if !missing.is_empty() && cap.is_none() {
        missing.truncate(MAX_SCRIPT_URLS);
        let h = begin_batch(&missing, SCRIPT_CAP);
        if h >= 0 {
            {
                doc_mut().nav_mod_want = Some(missing);
                doc_mut().nav_mod_rounds = rounds + 1;
                doc_mut().nav_stage = NavStage::Mod;
                doc_mut().nav_job = h;
                doc_mut().nav_stage_ms = now_ms();
            }
            return true;
        }
        log("[beak] module graph could not be fetched");
    }
    // Too large, too deep or done: evaluate what is there. A missing module
    // reports itself by name when linking.
    if !missing.is_empty() {
        let why = cap.unwrap_or("nicht holbar");
        log(&alloc::format!(
            "[beak] module graph unresolved: {} offen, {} im Graphen, {rounds} Runden — Deckel: {why}",
            missing.len(), seen.len()));
        if let Some(u) = missing.first() {
            log(&alloc::format!("[beak]   erste offene Adresse: {u}"));
        }
    }
    eval_modules();
    sheet_pump(engine)
}

/// One round over stylesheets a script inserted.
///
/// Returns true if a round trip is running. Round-based like the module
/// graph: an arriving sheet lets a component finish, which may insert
/// another.
fn sheet_pump(engine: &Engine) -> bool {
    let Some(sess) = js_session() else { finish_scripts(engine); return false };
    // Run microtasks and timers first: whatever just finished registers its
    // sheets now.
    for _ in 0..8 { if sess.interp.run_timers() == 0 { break } }
    let want = sess.interp.take_pending_sheets();
    // No more sheets does not mean done: script-inserted scripts come next.
    if want.is_empty() { return script_pump(engine) }
    let rounds = doc().nav_sheet_rounds;
    if rounds >= MAX_SHEET_ROUNDS {
        log(&alloc::format!("[beak] sheet rounds capped at {MAX_SHEET_ROUNDS}, {} offen", want.len()));
        for (id, _) in want { beak_engine::js::dombind::sheet_done(&mut sess.interp, id, false); }
        finish_scripts(engine);
        return false;
    }
    let base = url_str().to_string();
    let mut nodes: Vec<u32> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for (id, href) in want.into_iter().take(MAX_SCRIPT_URLS) {
        nodes.push(id);
        urls.push(resolve(&base, &href));
    }
    let h = begin_batch(&urls, CSS_CAP);
    if h < 0 {
        log("[beak] script stylesheets could not be fetched");
        for id in nodes { beak_engine::js::dombind::sheet_done(&mut sess.interp, id, false); }
        finish_scripts(engine);
        return false;
    }
    {
        doc_mut().nav_sheet_nodes = Some(nodes);
        doc_mut().nav_sheet_rounds = rounds + 1;
        doc_mut().nav_stage = NavStage::Sheet;
        doc_mut().nav_job = h;
        doc_mut().nav_stage_ms = now_ms();
    }
    true
}

fn nav_sheets_arrived(engine: &Engine) {
    let h = nav_job();
    let nodes = doc_mut().nav_sheet_nodes.take().unwrap_or_default();
    let mut scratch: Vec<u8> = Vec::with_capacity(CSS_CAP);
    let spans = take_batch(h, &mut scratch.spare_capacity_mut()[..CSS_CAP], nodes.len());
    let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
    // SAFETY: as in `nav_stylesheets_arrived`.
    unsafe { scratch.set_len(total.min(CSS_CAP)) };
    let (mut ok, mut bad) = (0usize, 0usize);
    if let Some(sess) = js_session() {
        for (k, id) in nodes.iter().enumerate() {
            let (off, n) = spans.get(k).copied().unwrap_or((0, 0));
            let got = n > 0 && off + n <= scratch.len() && css_append(&scratch[off..off + n]);
            if got { ok += 1 } else { bad += 1 }
            beak_engine::js::dombind::sheet_done(&mut sess.interp, *id, got);
        }
    }
    log(&alloc::format!("[beak] script stylesheets: {ok} geholt, {bad} gescheitert, {} ms",
                        now_ms() - doc().nav_stage_ms));
    if ok > 0 {
        decode_css();
        // The cascade must run again, or the sheet sits in the buffer without
        // effect.
        bump_content_gen("sheet");
        mark_dirty();
    }
    if !sheet_pump(engine) { nav_done(); }
}

/// One round over scripts that a script inserted.
///
/// This is how split bundles load their chunks: webpack creates a
/// `<script>`, appends it to the head and waits for its `onload`. Without an
/// answer the promise stays pending and the app never renders.
///
/// Round-based like the sheets: an arriving chunk inserts the next.
fn script_pump(engine: &Engine) -> bool {
    let Some(sess) = js_session() else { finish_scripts(engine); return false };
    for _ in 0..8 { if sess.interp.run_timers() == 0 { break } }
    let want = sess.interp.take_pending_scripts();
    if want.is_empty() { finish_scripts(engine); return false }
    let rounds = doc().nav_dynjs_rounds;
    if rounds >= MAX_DYNJS_ROUNDS {
        log(&alloc::format!("[beak] dyn-script rounds capped at {MAX_DYNJS_ROUNDS}, {} offen",
                            want.len()));
        for (id, _) in want { beak_engine::js::dombind::script_done(&mut sess.interp, id, None); }
        finish_scripts(engine);
        return false;
    }
    let base = url_str().to_string();
    let mut nodes: Vec<u32> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for (id, src) in want.into_iter().take(MAX_SCRIPT_URLS) {
        nodes.push(id);
        urls.push(resolve(&base, &src));
    }
    let h = begin_batch(&urls, SCRIPT_CAP);
    if h < 0 {
        log("[beak] dyn scripts could not be fetched");
        for id in nodes { beak_engine::js::dombind::script_done(&mut sess.interp, id, None); }
        finish_scripts(engine);
        return false;
    }
    {
        doc_mut().nav_dynjs_nodes = Some(nodes);
        doc_mut().nav_dynjs_rounds = rounds + 1;
        doc_mut().nav_stage = NavStage::DynJs;
        doc_mut().nav_job = h;
        doc_mut().nav_stage_ms = now_ms();
    }
    true
}

fn nav_dynjs_arrived(engine: &Engine) {
    let h = nav_job();
    let nodes = doc_mut().nav_dynjs_nodes.take().unwrap_or_default();
    let dst = img_fetch_buf();
    let spans = take_batch(h, as_uninit(&mut dst[..SCRIPT_CAP.min(IMG_FETCH_CAP)]), nodes.len());
    // Copy everything out first, then run: a running script may insert the
    // next one, which reuses the same buffer.
    let mut texts: Vec<Option<String>> = Vec::with_capacity(nodes.len());
    for k in 0..nodes.len() {
        let (off, n) = spans.get(k).copied().unwrap_or((0, 0));
        if n == 0 { texts.push(None); continue }
        let bytes = &dst[off..off + n];
        texts.push(core::str::from_utf8(bytes).ok().map(String::from));
    }
    let (mut ok, mut bad) = (0usize, 0usize);
    if let Some(sess) = js_session() {
        for (k, id) in nodes.iter().enumerate() {
            let t = texts.get(k).and_then(|t| t.as_deref());
            if t.is_some() { ok += 1 } else { bad += 1 }
            beak_engine::js::dombind::script_done(&mut sess.interp, *id, t);
        }
    }
    log(&alloc::format!("[beak] dyn scripts: {ok} geholt, {bad} gescheitert, {} ms",
                        now_ms() - doc().nav_stage_ms));
    if !sheet_pump(engine) { nav_done(); }
}

/// Log which fonts the page brings via `@font-face`.
///
/// Runs before fetching, so it can only say what the page requests; the
/// verdict comes from `pump_fonts`, which logs `FEHLT` or `NICHT LESBAR` per
/// file and the total.
fn log_font_faces(css: &str) {
    let mut names: Vec<&str> = Vec::new();
    let mut rest = css;
    while let Some(i) = rest.find("@font-face") {
        rest = &rest[i + 10..];
        let Some(open) = rest.find('{') else { break };
        let Some(close) = rest[open..].find('}') else { break };
        let block = &rest[open..open + close];
        rest = &rest[open + close..];
        let Some(f) = block.find("font-family") else { continue };
        let after = &block[f + 11..];
        let Some(c) = after.find(':') else { continue };
        let val = after[c + 1..].split(';').next().unwrap_or("").trim()
            .trim_matches(['"', '\'']).trim();
        if !val.is_empty() && !names.contains(&val) && names.len() < 12 { names.push(val); }
    }
    if names.is_empty() { return }
    log(&alloc::format!("[beak] @font-face: {} Familien verlangt ({}) — werden geholt",
                        names.len(), names.join(", ")));
}

/// Append a stylesheet. Fetched later means later in the cascade, which is
/// the order a browser applies it in.
fn css_append(bytes: &[u8]) -> bool {
    let len = CSS_LEN.load(Relaxed);
    if len + bytes.len() + 1 >= CSS_CAP {
        log(&alloc::format!("[beak] CSS buffer full at {len} B — dropped a {} B sheet", bytes.len()));
        return false;
    }
    let dst = css_buf();
    dst[len..len + bytes.len()].copy_from_slice(bytes);
    dst[len + bytes.len()] = b'\n';
    CSS_LEN.store(len + bytes.len() + 1, Relaxed);
    true
}

fn nav_modules_arrived(engine: &Engine) {
    let h = nav_job();
    let want = doc_mut().nav_mod_want.take().unwrap_or_default();
    let dst = img_fetch_buf();
    let spans = take_batch(h, as_uninit(&mut dst[..SCRIPT_CAP.min(IMG_FETCH_CAP)]), want.len());
    if let Some(sess) = js_session() {
        for (k, url) in want.iter().enumerate() {
            let (off, n) = spans.get(k).copied().unwrap_or((0, 0));
            if n == 0 { log(&alloc::format!("[beak]   module FAIL {url}: leer")); continue }
            let bytes = &dst[off..off + n];
            let Ok(text) = core::str::from_utf8(bytes) else {
                log(&alloc::format!("[beak]   module FAIL {url}: kein UTF-8"));
                continue;
            };
            match beak_engine::js::parse(text, true) {
                Ok(p) => sess.interp.add_module(url, alloc::rc::Rc::new(p)),
                Err(e) => log(&alloc::format!("[beak]   module FAIL {url}: SyntaxError: {} @{}",
                                              e.msg, src_pos(text, e.at))),
            }
        }
    }
    if !module_pump(engine) { nav_done(); }
}

/// Evaluate the entries — in document order, each exactly once.
fn eval_modules() {
    let entries = match doc().nav_mod_entries.clone() {
        Some(e) => e, None => return,
    };
    let Some(sess) = js_session() else { return };
    let t0 = now_ms();
    let (mut ok, mut bad) = (0usize, 0usize);
    for u in &entries {
        match sess.interp.eval_module(u) {
            Ok(()) => ok += 1,
            Err(e) => {
                bad += 1;
                let msg = beak_engine::js::modules::describe(&mut sess.interp, e);
                log(&alloc::format!("[beak]   module FAIL {u}: {msg}"));
            }
        }
    }
    let mut m = String::from("[beak] modules: ");
    push_i64(&mut m, ok as i64);
    m.push_str(" gelaufen, ");
    push_i64(&mut m, bad as i64);
    m.push_str(" gescheitert, ");
    push_i64(&mut m, sess.interp.modules.len() as i64);
    m.push_str(" im Graphen, ");
    push_i64(&mut m, (now_ms() - t0) as i64);
    m.push_str(" ms");
    log(&m);
    let (r, f, b) = doc().script_tally;
    doc_mut().script_tally = (r + ok, f + bad, b);
}

/// Timers, cookies, tree and the report — after all of the page's code,
/// classic scripts and modules alike.
fn finish_scripts(engine: &Engine) {
    let (ran, failed, bytes) = doc().script_tally;
    let t0 = doc().script_t0;
    let Some(sess) = js_session() else { return };
    // `DOMContentLoaded`, then `load`, in spec order: first the document
    // (deferred scripts and modules have run), then `load` (late sheets are
    // in too). Both target the document node — `window.addEventListener`
    // lands there (`target_node`), and a handler on `document` gets them by
    // bubbling.
    // `load` itself fires after the first paint, not here: the host passes
    // box geometry only then (`Interp::set_geometry`), and a measuring
    // handler would otherwise read zeros that look like measurements.
    let doc_node = sess.interp.doc.as_ref().map(|d| d.doc);
    if let Some(dn) = doc_node {
        let _ = beak_engine::js::dombind::dispatch(&mut sess.interp, "DOMContentLoaded", &[dn]);
    }
    doc_mut().load_pending = true;
    // Run timers registered during loading once — many pages finish their
    // UI in a `setTimeout(…, 0)`.
    let timers = sess.interp.run_timers();
    sync_cookies(sess);
    sync_history(engine, sess);
    sync_scroll(sess);
    drain_console(sess);
    let mut listeners = false;
    if let Some(d) = sess.interp.doc.as_mut() {
        listeners = d.has_listeners;
        // The boxes are needed for `getBoundingClientRect` too, not only for
        // clicks; a page that runs scripts may ask even without handlers.
        engine.set_hit_all(listeners || ran > 0);
        engine.set_scripted_dom(Some(d.to_dom()));
    }
    { doc_mut().nav_mod_entries = None };
    let mut m = String::from("[beak] scripts: ");
    push_i64(&mut m, ran as i64);
    m.push_str(" gelaufen, ");
    push_i64(&mut m, failed as i64);
    m.push_str(" gescheitert, ");
    push_i64(&mut m, (bytes / 1024) as i64);
    m.push_str(" KB, ");
    push_i64(&mut m, (now_ms() - t0) as i64);
    m.push_str(" ms");
    if timers > 0 { m.push_str(", "); push_i64(&mut m, timers as i64); m.push_str(" Zeitgeber"); }
    if now_ms() - t0 > SCRIPT_SLOW_MS { m.push_str(", RECHNET LANGE"); }
    // Whether dispatch is armed belongs in the log once per page; otherwise
    // "no click arrived" cannot be told from "the page has no handlers".
    m.push_str(if listeners { ", Ereignisse SCHARF" } else { ", keine Behandler" });
    log(&m);
}

/// The two directions of the form bridge. The rule lives in the engine
/// (`dombind::push_control_values` / `pull_control_values`); here are only
/// the borrows, so the rule exists once.
fn push_control_values(page: &Page) {
    let Some(sess) = js_session() else { return };
    let Some(doc) = sess.interp.doc.as_mut() else { return };
    beak_engine::js::dombind::push_control_values(doc, &page.forms, &page.state);
}

fn pull_control_values(page: &mut Page, sess: &beak_engine::js::Session) {
    let Some(doc) = sess.interp.doc.as_ref() else { return };
    beak_engine::js::dombind::pull_control_values(doc, &page.forms, &mut page.state);
}

/// Dispatch a click to the page. Returns true if a handler called
/// `preventDefault` — then what beak would otherwise do (follow a link,
/// operate a control) is skipped.
fn dispatch_click(engine: &Engine, page: &mut Page, lay: &Layout, cx: i32, cy: i32) -> bool {
    // Each handler gets its own time budget, so the twentieth click does not
    // pay for the nineteen before.
    arm_script_budget();
    // The handler must see what the user typed.
    push_control_values(page);
    let Some(sess) = js_session() else { return false };
    // The seq chain under the pointer, outermost to innermost — the same
    // list `:hover` uses.
    let chain = lay.element_chain(cx, cy);
    if chain.is_empty() { return false; }
    let Some(doc) = sess.interp.doc.as_ref() else { return false };
    let nodes: Vec<u32> = chain.iter().filter_map(|s| doc.by_seq(*s)).collect();
    if nodes.is_empty() { return false; }
    // A label activates its control even without script. The fast path
    // returns when the page has no handlers, which is right for events but
    // wrong for built-in behaviour.
    let on_label = nodes.last().is_some_and(|n| {
        beak_engine::js::dombind::label_target(&sess.interp, *n).is_some()
    });
    if !doc.has_listeners && !on_label { return false; }
    let t0 = now_ms();
    // With the position. `cx`/`cy` arrive as window x and document y (`cy`
    // includes the scroll offset); `client*` wants both window-relative,
    // `page*` both document-relative. beak does not scroll horizontally, so
    // the two x are equal.
    let sy = scroll_y() as f64;
    let prevented = matches!(
        beak_engine::js::dombind::dispatch_at(&mut sess.interp, "click", &nodes,
            Some((cx as f64, cy as f64 - sy, cx as f64, cy as f64))), Ok(true));
    let timers = sess.interp.run_timers();
    sync_cookies(sess);
    sync_history(engine, sess);
    sync_scroll(sess);
    // Only if something changed: a handler that merely counts must not cost
    // a layout.
    let changed = sess.interp.doc.as_ref().is_some_and(|d| d.dirty);
    if changed {
        if let Some(d) = sess.interp.doc.as_mut() {
            engine.set_scripted_dom(Some(d.to_dom()));
        }
        bump_content_gen("script");
        mark_dirty();
    }
    // Refresh the form model and values before running a submit request,
    // or it sends the previous state. After a handler the tree knows more
    // than the host.
    page.sync(engine);
    pull_control_values(page, sess);
    let submits = sess.interp.take_submits();
    drain_console(sess);
    for seq in submits {
        log(&alloc::format!("[beak] script submit: form seq={seq}"));
        if submit_form_seq(engine, page, seq) { return true }
    }
    // A handler that sets `location.href` has said where to go. Following
    // the clicked link too would make two navigations from one click, so the
    // click counts as handled.
    if sync_nav(engine) { return true }
    if changed || prevented || timers > 0 {
        let mut m = String::from("[beak] click -> js: ");
        push_i64(&mut m, nodes.len() as i64);
        m.push_str(" Knoten, ");
        m.push_str(if changed { "Baum geaendert" } else { "unveraendert" });
        if prevented { m.push_str(", preventDefault"); }
        if timers > 0 { m.push_str(", "); push_i64(&mut m, timers as i64); m.push_str(" Zeitgeber"); }
        m.push_str(", ");
        push_i64(&mut m, (now_ms() - t0) as i64);
        m.push_str(" ms");
        log(&m);
    }
    prevented
}

/// Kernel randomness into a buffer. `false` if the kernel refused — the
/// caller then throws rather than return weak randomness.
///
/// Chunked because the kernel caps a call at 64 KiB (it holds the RNG mutex
/// meanwhile), so a larger buffer is never left half unfilled.
fn random_bytes(out: &mut [u8]) -> bool {
    for teil in out.chunks_mut(64 * 1024) {
        // SAFETY: the kernel writes at most `len` bytes from `ptr`, and both
        // describe exactly this chunk.
        let n = unsafe { npk_random_bytes(teil.as_mut_ptr() as i32, teil.len() as i32) };
        if n < 0 || n as usize != teil.len() { return false }
    }
    true
}

/// Step budget for a page script.
///
/// Only a safety net against a run that makes no progress. What really
/// bounds a page is time (`SCRIPT_BUDGET_MS`); a step cap would equally hit
/// a page that just computes a lot, which is not an error.
const SCRIPT_STEPS: u64 = 20_000_000_000;

/// How long a script run or handler may compute.
///
/// Generous on purpose: a login page that hashes its password itself
/// (PBKDF2, tens of thousands of rounds) takes minutes under an interpreter.
/// That is not an endless loop, and aborting it would report a broken page
/// where there is none. The cap is against `while(true)`; meanwhile the
/// heartbeat reports every few seconds that the script is still running.
const SCRIPT_BUDGET_MS: i64 = 900_000;

/// When a run gets noticed in the log. A script computing for minutes is not
/// an error, but it is why nothing happens, and that should be said.
const SCRIPT_SLOW_MS: i64 = 3_000;

/// How often a long run reports in.
const SCRIPT_HEARTBEAT_MS: i64 = 5_000;

/// The budget belongs to the run, not the document.
///
/// Exactly one piece of page code runs at a time, tabs or not; these three
/// describe the run on the stack, and `arm_script_budget` resets them before
/// each one.
///
/// Also: the engine calls `script_time_left` from inside the running
/// interpreter, which already holds a `&mut` borrow from the document via
/// `js_session()`. A `doc()` there would be the aliasing `doc`/`doc_mut`
/// warns about.
static SCRIPT_DEADLINE: AtomicI64 = AtomicI64::new(0);
/// When the running handler started, for the heartbeat. Distinct from
/// `Doc::script_t0`, which is the start of the page's script round.
static BUDGET_T0: AtomicI64 = AtomicI64::new(0);
static BUDGET_SAID: AtomicI64 = AtomicI64::new(0);

/// The clock the engine asks every 65 536 steps — and the heartbeat.
///
/// A screen frozen for minutes with nothing in the log is an error even if
/// the computation is not, so a long run reports itself while running, not
/// afterwards. The engine asks here anyway, often enough for once a second.
fn script_time_left() -> bool {
    let now = now_ms();
    let t0 = BUDGET_T0.load(Relaxed);
    let said = BUDGET_SAID.load(Relaxed);
    let el = now - t0;
    if el >= SCRIPT_SLOW_MS && el - said >= SCRIPT_HEARTBEAT_MS {
        BUDGET_SAID.store(el, Relaxed);
        let mut m = String::from("[beak] Skript rechnet noch: ");
        push_i64(&mut m, el / 1000);
        m.push_str(" s von hoechstens ");
        push_i64(&mut m, SCRIPT_BUDGET_MS / 1000);
        m.push_str(" s");
        log(&m);
    }
    now < SCRIPT_DEADLINE.load(Relaxed)
}

/// Reset the clock — before every run of page code.
fn arm_script_budget() {
    let now = now_ms();
    SCRIPT_DEADLINE.store(now + SCRIPT_BUDGET_MS, Relaxed);
    BUDGET_T0.store(now, Relaxed);
    BUDGET_SAID.store(0, Relaxed);
}

/// Start a page's image load: drop the old pixels and return the list of
/// sources still to fetch. Touches the network not at all, so the first paint
/// can happen right after it.
///
/// The same src repeats all over a real page (icons, bullets, a logo in header
/// and footer). The engine keys decoded images by src, so a repeat only
/// re-fetched and re-decoded identical bytes — wasted requests against the
/// server's rate limit, and wasted MAX_IMAGES slots that real images needed.
fn begin_images(engine: &mut Engine) -> Vec<String> {
    doc_mut().images_dirty = false;
    // The engine holds the highlights; a new page has none. `set_url` clears
    // them in the document, this drops the paint.
    engine.set_marks(None, Vec::new());
    engine.images_begin();
    // A built-in page fetches nothing. Its images are relative to
    // `beak:selftest`, which would resolve to a host name `beak` — and an
    // invented name sent to the resolver leaves the machine.
    if selftest::matches(url_str()) {
        return Vec::new();
    }
    let mut pending: Vec<String> = Vec::new();
    // The same viewport width layout uses: `<picture>`/`srcset` picks its
    // candidate per media query, so fetching at a different width would fetch
    // a URL the page never asks for and leave the real one blank.
    let vw = canvas_rect().map(|(_, _, w, _)| w as u32).unwrap_or(1280);
    // From the tree the layout uses, not the original HTML: a page that
    // builds its content by script has no images in its source.
    let all = engine.image_srcs_now(html_str(), vw);
    // Log what was collected, so "no images on the page" can be told from
    // "looked in the wrong tree".
    if !all.is_empty() {
        log(&alloc::format!("[beak] Bilder gesammelt: {} (z.B. {})",
            all.len(), all.first().map(|s| &s[..s.len().min(72)]).unwrap_or("")));
    }
    for src in all.iter() {
        if pending.len() >= MAX_IMAGES {
            log(&alloc::format!("[beak] image cap hit: {} of {} sources fetched", MAX_IMAGES, all.len()));
            break;
        }
        if !pending.iter().any(|s| s == src) {
            pending.push(src.clone());
        }
    }
    // Serve what the last pages already decoded, before the first layout.
    // That pays twice: no request, no decode — and the box is definite on the
    // very first layout instead of being guessed and moving the page later.
    //
    // Keyed by the resolved url, because the `src` attribute alone is
    // ambiguous across sites (`/logo.png`).
    let pairs: Vec<(String, String)> =
        pending.iter().map(|s| (s.clone(), resolve(url_str(), s))).collect();
    let served = engine.adopt_cached(&pairs);
    if !served.is_empty() {
        pending.retain(|s| !served.iter().any(|d| d == s));
        let (n, bytes) = engine.img_cache_stats();
        log(&alloc::format!("[beak] images: {} from cache, {} to fetch (cache {} imgs, {} KiB)",
            served.len(), pending.len(), n, bytes / 1024));
    }
    bump_content_gen("images-begin"); // lay out with placeholders
    mark_dirty();
    pending
}

/// Ask for the next few images, and take delivery of the last few.
///
/// One batch in flight at a time, not a whole page: a batch is answered in one
/// go, so asking for everything at once would put the page's whole image
/// traffic between two repaints. Small batches let the reader scroll through a
/// loading page.
///
/// The last layout's `guessed_image_srcs` lists the `src`s whose box it had to
/// guess. Only if one of those arrives does the page move and a re-layout pay
/// for itself; everything else is a repaint, which is far cheaper.
///
/// `band` is the visible document band `(scroll_y, scroll_y + viewport_h)`.
/// A repaint is the whole viewport, so an image below the fold is paid for in
/// full and shows nothing — see `Layout::images_in_band`.
fn pump_images(
    engine: &mut Engine,
    pending: &mut Vec<String>,
    layout: Option<&Layout>,
    band: (i32, i32),
) {
    let h = img_job();
    if h >= 0 {
        if http_poll(h) == 0 {
            return; // still on the wire — come back next turn
        }
        images_arrived(engine, h, layout, band);
    }
    images_start(pending, layout);
}

fn images_start(pending: &mut Vec<String>, layout: Option<&Layout>) {
    if pending.is_empty() {
        return;
    }
    // Layout-affecting first. An image whose box was guessed moves the page
    // when it lands, and that costs a full re-layout wherever it sits.
    // Fetching it in the first batch pays that once, immediately, instead of
    // after repaints the re-layout then throws away.
    if let Some(l) = layout {
        if !l.guessed_image_srcs.is_empty() {
            // Stable, so document order survives inside each group.
            pending.sort_by_key(|s| !l.guessed_image_srcs.iter().any(|g| g == s));
        }
    }
    let take = pending.len().min(IMG_BATCH);
    let srcs: Vec<String> = pending.drain(..take).collect();
    let urls: Vec<String> = srcs.iter().map(|s| resolve(url_str(), s)).collect();
    let h = begin_batch(&urls, IMG_FETCH_CAP);
    if h < 0 {
        // Could not even be asked for. These keep their placeholders rather
        // than being retried every turn for as long as the page stays open.
        log(&alloc::format!("[beak] image batch of {} could not start", urls.len()));
        return;
    }
    doc_mut().img_job_srcs = Some(srcs.into_iter().zip(urls).collect());
    doc_mut().img_job = h;
}

fn images_arrived(
    engine: &mut Engine,
    handle: i32,
    layout: Option<&Layout>,
    band: (i32, i32),
) {
    let want: Vec<(String, String)> = doc_mut().img_job_srcs.take().unwrap_or_default();
    doc_mut().img_job = -1;
    let dst = img_fetch_buf();
    let spans = take_batch(handle, as_uninit(dst), want.len());
    let mut arrived: Vec<&str> = Vec::new();
    let mut moved = false;
    for ((src, url), (off, n)) in want.iter().zip(spans) {
        if n == 0 {
            // Not silent: a failed request and an answer too large for the
            // buffer look the same here, and the second is our own cap.
            log(&alloc::format!("[beak] image not delivered (request failed or over {} KiB buffer) — {}",
                IMG_FETCH_CAP / 1024, src));
            continue;
        }
        let bytes = &dst[off..off + n];
        // Decode now, drop the compressed bytes — and keep the pixels under
        // their url so the next navigation to this page needs neither.
        if let Err(why) = engine.add_image_cached(src, url, bytes) {
            // The reason belongs in the message: a decode failure and the
            // budget refusing the image are different problems.
            log(&alloc::format!("[beak] image dropped ({n} B): {why} — {src}"));
            continue;
        }
        arrived.push(src.as_str());
        if layout.is_some_and(|l| l.guessed_image_srcs.iter().any(|g| g == src)) {
            moved = true;
        }
    }
    // Log a successful round too; otherwise a round where seven images
    // arrived looks the same as one where none was requested.
    log(&alloc::format!("[beak] Bilder: {} von {} dekodiert{}",
        arrived.len(), want.len(),
        if moved { ", ein geratener Kasten wurde bestimmt" } else { "" }));
    if arrived.is_empty() {
        return;
    }
    if moved {
        // A guessed box moves once the real size lands: the page below it
        // shifts and the scroll extent changes, so this one must re-lay-out
        // wherever it sits.
        bump_content_gen("image-arrived");
        mark_dirty();
        return;
    }
    // Pure repaint. Ask first whether it would show anything: images below
    // the fold cannot change a pixel. Scrolling marks the page dirty on its
    // own, so nothing is lost; it is drawn the moment it can be seen.
    match layout {
        Some(l) if !l.images_in_band(&arrived, band.0, band.1) => {}
        _ => mark_dirty(),
    }
}

/// Log what the last paint looked for in vain — once per URL.
///
/// The placeholder is silent, so "never requested" looks the same as
/// "fetched, decoded, and looked up under another key". The line names both
/// halves: the key searched for and how many images the cache holds.
fn log_image_miss(engine: &Engine) {
    let Some((src, held)) = engine.image_miss() else { return };
    if doc().img_missed.iter().any(|s| *s == src) { return }
    doc_mut().img_missed.push(src.clone());
    log(&alloc::format!("[beak] Bild nicht im Speicher ({held} da): {}",
        &src[..src.len().min(96)]));
}

fn images_dirty() -> bool {
    doc().images_dirty
}

/// Ask for the CSS images (`background-image`/`mask-image`) the last layout
/// wanted, and take delivery of the last batch — one batch in flight, like
/// `<img>`.
///
/// Kept apart from `pump_images` for one reason that matters: a CSS image can
/// never move a box, so an arriving one is always just a repaint — there is no
/// `guessed` case and no `bump_content_gen`. The engine already resolved every
/// `data:` URI itself, so this list is only what genuinely needs the network.
///
/// The URL is resolved against the document, not the stylesheet that declared
/// it. Those differ only for a relative url() in a linked sheet; the shell
/// concatenates the sheets into one buffer, so the per-sheet base is gone by
/// here. Absolute and root-relative urls — which is what real sheets ship —
/// resolve identically either way.
fn pump_css_images(
    engine: &Engine,
    pending: &mut Vec<(u64, String)>,
    layout: Option<&Layout>,
    band: (i32, i32),
) {
    let h = cssimg_job();
    if h >= 0 {
        if http_poll(h) == 0 {
            return;
        }
        css_images_arrived(engine, h, layout, band);
    }
    css_images_start(pending);
}

fn css_images_start(pending: &mut Vec<(u64, String)>) {
    if pending.is_empty() {
        return;
    }
    let take = pending.len().min(IMG_BATCH);
    let want: Vec<(u64, String)> = pending.drain(..take).collect();
    let urls: Vec<String> = want.iter().map(|(_, u)| resolve(url_str(), u)).collect();
    let h = begin_batch(&urls, IMG_FETCH_CAP);
    if h < 0 {
        log(&alloc::format!("[beak] background batch of {} could not start", urls.len()));
        return;
    }
    let keys: Vec<(u64, String)> =
        want.into_iter().map(|(k, _)| k).zip(urls).collect();
    doc_mut().cssimg_job_keys = Some(keys);
    doc_mut().cssimg_job = h;
}

fn css_images_arrived(
    engine: &Engine,
    handle: i32,
    layout: Option<&Layout>,
    band: (i32, i32),
) {
    let want: Vec<(u64, String)> = doc_mut().cssimg_job_keys.take().unwrap_or_default();
    doc_mut().cssimg_job = -1;
    let dst = img_fetch_buf();
    let spans = take_batch(handle, as_uninit(dst), want.len());
    let mut arrived: Vec<u64> = Vec::new();
    for ((key, url), (off, n)) in want.iter().zip(spans) {
        if n == 0 {
            log(&alloc::format!("[beak] background not delivered (request failed or over {} KiB buffer) — {}",
                IMG_FETCH_CAP / 1024, url));
            continue; // the box stays undecorated
        }
        let bytes = &dst[off..off + n];
        match engine.add_css_image_cached(*key, url, bytes) {
            Ok(()) => arrived.push(*key),
            Err(why) => log(&alloc::format!("[beak] background dropped ({n} B): {why} — {url}")),
        }
    }
    if arrived.is_empty() {
        return;
    }
    // No `moved` case here at all — a background can never change geometry —
    // so the visibility question is the only one.
    match layout {
        Some(l) if !l.css_images_in_band(&arrived, band.0, band.1) => {}
        _ => mark_dirty(),
    }
}

// ── Sub-resource batches in flight ────────────────────────────────────────

/// The `<img>` batch on the wire, or -1.
fn img_job() -> i32 {
    doc().img_job
}
fn cssimg_job() -> i32 {
    doc().cssimg_job
}
fn font_job() -> i32 {
    doc().font_job
}

/// Drop the sub-resource batches of a page that is being replaced. A browser
/// that keeps fetching the pictures of the page you just left spends the
/// network on nothing — and with one kernel fetch queue behind them, it also
/// makes the new document wait its turn.
fn subresources_cancel() {
    if doc().img_job >= 0 {
        http_cancel(doc().img_job);
        doc_mut().img_job = -1;
    }
    doc_mut().img_job_srcs = None;
    if doc().cssimg_job >= 0 {
        http_cancel(doc().cssimg_job);
        doc_mut().cssimg_job = -1;
    }
    doc_mut().cssimg_job_keys = None;
    // And what was not yet requested. The queue belongs to the page being
    // replaced: keeping it would share the kernel queue with the old page's
    // images and resolve their relative URLs against the new base.
    let d = doc_mut();
    d.pending_imgs.clear();
    d.pending_css_imgs.clear();
    d.css_asked.clear();
}

/// Set the address + start fetching, without touching history (reload,
/// back/forward — those addresses are already in it).
///
/// A failure is not silent: `nav_fail` puts a diagnostic page in the document
/// and logs the reason, so there is nothing to add here.
fn fetch_url(engine: &Engine, url: &str) {
    set_url(url);
    nav_begin(engine, "GET", url, &[], "", false);
}

/// The same, but the address we land on joins the history — a click, a typed
/// address, a form. Recorded when the document arrives, not now: recording
/// where we aimed would make every trip back replay the redirect.
fn nav_goto(engine: &Engine, url: &str) {
    set_url(url);
    nav_begin(engine, "GET", url, &[], "", true);
}

/// Navigate by POSTing `body` to `url` (a form with `method=post`).
fn post_url(engine: &Engine, url: &str, body: &[u8]) {
    set_url(url);
    nav_begin(
        engine, "POST", url, body,
        "Content-Type: application/x-www-form-urlencoded",
        true,
    );
}

/// Navigate the address bar's typed text (normalise scheme) — new entry.
fn go(engine: &Engine, typed: &str) {
    if let Some(abs) = typed_to_url(typed) {
        nav_goto(engine, &abs);
    }
}

/// What the address bar means: a URL, or a search for the text.
/// `None` means nothing was entered.
fn typed_to_url(typed: &str) -> Option<String> {
    let t = typed.trim();
    if t.is_empty() {
        return None;
    }
    let abs = if selftest::matches(t) {
        selftest::URL.to_string()
    } else if t.starts_with("http://") || t.starts_with("https://") {
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
    Some(abs)
}

// ── Tabs ───────────────────────────────────────────────────────────────

/// A tab's label: the title, else the host of the URL, else "Neuer Tab".
///
/// The host, not the whole URL: a label is cut at the end, and URLs all
/// start alike.
fn tab_label(d: &Doc) -> String {
    let full: &str = if !d.title.is_empty() {
        &d.title
    } else if d.url.is_empty() {
        "Neuer Tab"
    } else {
        let after = d.url.split_once("://").map(|(_, r)| r).unwrap_or(&d.url);
        after.split('/').next().unwrap_or(after)
    };
    // Truncate here, because nobody else does: the compositor draws text
    // from the left edge of its box and does not clip it (`MaxWidth` bounds
    // the box, not the glyphs). Overflow would land in the neighbouring tab.
    if full.chars().count() <= TAB_CHARS {
        return full.to_string();
    }
    let mut out: String = full.chars().take(TAB_CHARS - 1).collect();
    out.push('\u{2026}');
    out
}

/// Freeze a tab: release the handles, drop the rest.
///
/// A frozen tab is its restore description. Instead of listing everything
/// that must go — and forgetting the next new field — the document is reset
/// to a fresh one and only what restores it is written back. The large item
/// is the JS realm and its tree; what remains is kilobytes.
fn tab_freeze() {
    // Handles first: they belong to the host, not to the memory about to be
    // dropped. A batch that keeps running would take the one fetch queue
    // from the tab being switched to.
    nav_cancel();
    subresources_cancel();
    {
        let d = doc_mut();
        if d.font_job >= 0 {
            http_cancel(d.font_job);
        }
        for (_, h) in core::mem::take(&mut d.fetch_jobs) {
            http_cancel(h);
        }
    }
    let (url, hist, hist_pos, y, title) = {
        let d = doc();
        (d.url.clone(), d.hist.clone(), d.hist_pos, d.scroll_y, d.title.clone())
    };
    let d = doc_mut();
    *d = Doc::new();
    d.edit.push_str(&url);
    d.url = url;
    d.hist = hist;
    d.hist_pos = hist_pos;
    d.title = title;
    // Where the reader was. Not `scroll_y`: loading resets it to 0, rightly,
    // since a new page starts at the top.
    d.scroll_want = y;
}

/// An empty tab.
///
/// The buffers must really be emptied: `HTML_BUF`/`CSS_BUF` belong to the
/// program, not the page, and a new tab showing the old one's text would be
/// exactly the mix-up `Doc` exists to prevent.
fn blank_document(engine: &Engine) {
    HTML_LEN.store(0, Relaxed);
    CSS_LEN.store(0, Relaxed);
    engine.set_scripted_dom(None);
    engine.set_hit_all(false);
    bump_content_gen("tab-blank");
    mark_dirty();
}

/// Load the current tab's page — the way back from freezing.
///
/// A tab without history creates its first entry; a returning one does not,
/// since it is already there.
fn tab_load(engine: &Engine, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) {
    // The cache holds the previous page's layout, the form model its
    // controls. Both depend on per-document counters, so after a switch a
    // comparison means nothing.
    *cache = None;
    *page = Page::new();
    let u = doc().url.to_string();
    if u.is_empty() {
        blank_document(engine);
        return;
    }
    if doc().hist.is_empty() {
        nav_goto(engine, &u);
    } else {
        fetch_url(engine, &u);
    }
}

/// Switch to another tab.
fn tab_activate(engine: &Engine, i: usize, cache: &mut Option<(Layout, i32, i32, u32)>,
                page: &mut Page) {
    if i >= tabs().len() || i == active() {
        return;
    }
    tab_freeze();
    set_active(i);
    tab_load(engine, cache, page);
    mark_dirty();
}

/// Open a tab. `background` only creates it.
///
/// A background tab fetches nothing until someone looks at it: the kernel
/// runs one fetch at a time, and an unseen tab would take the connection
/// from the page in front of the reader.
fn tab_open(engine: &Engine, url: &str, background: bool,
            cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) {
    if tabs().len() >= MAX_TABS {
        log("[beak] der Streifen ist voll — mehr als 10 Tabs passen nicht nebeneinander");
        return;
    }
    let mut d = Box::new(Doc::new());
    d.url.push_str(url);
    d.edit.push_str(url);
    tabs().push(d);
    if background {
        return;
    }
    let i = tabs().len() - 1;
    tab_freeze();
    set_active(i);
    tab_load(engine, cache, page);
    mark_dirty();
}

/// Close a tab.
fn tab_close(engine: &Engine, i: usize, cache: &mut Option<(Layout, i32, i32, u32)>,
             page: &mut Page) {
    let n = tabs().len();
    if i >= n {
        return;
    }
    // The last tab is the window, as in other browsers; otherwise Ctrl+W
    // would do nothing and the window could not be closed by keyboard.
    if n == 1 {
        host::close_widget();
        return;
    }
    let a = active();
    if i == a {
        // Release the handles before the document goes.
        tab_freeze();
    }
    tabs().remove(i);
    let last = tabs().len() - 1;
    set_active(if i < a { a - 1 } else if i > a { a } else { i.min(last) });
    if i == a {
        tab_load(engine, cache, page);
        mark_dirty();
    }
}

/// A typed address that is not a URL becomes a web search. Marginalia serves
/// real results to a no-JS client; other engines gate on browser
/// fingerprinting.
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

/// A form the page itself submits (`form.submit()`).
fn submit_form_seq(engine: &Engine, page: &Page, form_seq: u32) -> bool {
    match forms::submit_form(&page.forms, &page.state, form_seq) {
        Some(s) => { send_submission(engine, s); true }
        None => {
            log("[beak] script submit: dieses Formular kennt beak nicht");
            false
        }
    }
}

/// Submit a form. GET puts the data in the query string, POST in the request
/// body — the same encoding either way (HTML §4.10.21.3).
fn submit_form(engine: &Engine, page: &mut Page, activated: Option<u32>) -> bool {
    // The `submit` event first. A page computes in its handler what it sends
    // (a password hash, a timestamp, a token) and may cancel; without it the
    // computed fields would go out empty.
    //
    // Only on the user path: `form.submit()` from script fires no `submit`
    // event per spec, or the page's handler would run twice.
    let form_seq = activated.or(page.state.focus)
        .and_then(|s| page.forms.get(s)?.form)
        .and_then(|f| page.forms.forms.get(f).map(|d| d.seq));
    // Hold the relevant controls by their tree nodes: the page's handler may
    // rebuild, and `to_dom` renumbers. Otherwise `activated` could point at
    // another element after the handler — or at none.
    let node_of = |s: Option<u32>| -> Option<u32> {
        let s = s?;
        js_session().and_then(|x| x.interp.doc.as_ref()).and_then(|d| d.by_seq(s))
    };
    let act_node = node_of(activated);
    let focus_node = node_of(page.state.focus);
    if let Some(fs) = form_seq {
        arm_script_budget();
        // Before the handler. It may compute for minutes (a login hashing
        // itself), and until then the user would see nothing.
        let ts0 = now_ms();
        log("[beak] submit: Behandler der Seite laeuft…");
        if let Some(sess) = js_session() {
            if beak_engine::js::dombind::dispatch_seq(&mut sess.interp, "submit", fs) {
                // Cancelled. The handler often still has work queued (a
                // `setTimeout`, a promise), so run it and refresh the tree.
                let n = sess.interp.run_timers();
                drain_console(sess);
                if sess.interp.doc.as_ref().is_some_and(|d| d.dirty) {
                    if let Some(d) = sess.interp.doc.as_mut() {
                        engine.set_scripted_dom(Some(d.to_dom()));
                    }
                    bump_content_gen("submit");
                    mark_dirty();
                }
                log(&alloc::format!("[beak] submit: von der Seite abgefangen ({n} Zeitgeber)"));
                return true;
            }
            // Not cancelled — but the handler may have filled fields, and
            // those belong in the submission.
            let n = sess.interp.run_timers();
            let _ = n;
            drain_console(sess);
            if sess.interp.doc.as_ref().is_some_and(|d| d.dirty) {
                if let Some(d) = sess.interp.doc.as_mut() {
                    engine.set_scripted_dom(Some(d.to_dom()));
                }
                bump_content_gen("submit");
            }
        }
        page.sync(engine);
        if let Some(s) = js_session() { pull_control_values(page, s); }
        let mut m = String::from("[beak] submit: Behandler fertig nach ");
        push_i64(&mut m, now_ms() - ts0);
        m.push_str(" ms");
        log(&m);
    }
    // And back: the same node, its current number.
    let seq_of = |n: Option<u32>| -> Option<u32> {
        let n = n?;
        js_session().and_then(|x| x.interp.doc.as_ref())
            .and_then(|d| d.nodes.get(n as usize).map(|x| x.seq))
            .filter(|s| *s != 0)
    };
    let activated = seq_of(act_node).or(activated);
    if let Some(f) = seq_of(focus_node) { page.state.focus = Some(f); }
    let sub = match forms::submit(&page.forms, &page.state, activated) {
        Some(s) => s,
        // Silence here reads as "the button is dead". It is not the same
        // thing as a failed request, and the difference is the whole
        // diagnosis: a button whose form we never resolved (nested outside
        // it, or owned by a `form=` attribute we do not read) versus a form
        // that submitted and came back wrong.
        //
        // A submit button without a form does nothing per HTML §4.10.6 —
        // that is the rule, not an error, so it is not logged.
        None => {
            let owned = activated
                .and_then(|s| page.forms.get(s))
                .is_some_and(|c| c.form.is_some());
            if owned {
                log("[beak] submit: Formular gefunden, aber keine Eingabe daraus");
            }
            return false;
        }
    };
    send_submission(engine, sub);
    true
}

/// Send a finished submission — GET appends it to the URL, POST puts it in
/// the body. One version for both ways there (button and `submit()`).
fn send_submission(engine: &Engine, sub: forms::Submission) {
    // An empty action targets the current document; either way the form data
    // replaces the action's query string (HTML §4.10.21.3 "mutate action URL").
    let base = url_str().to_string();
    let action = if sub.action.is_empty() { base.clone() } else { resolve(&base, &sub.action) };
    let mut url = action.split(['?', '#']).next().unwrap_or(&action).to_string();
    if sub.method_get {
        if !sub.query.is_empty() {
            url.push('?');
            url.push_str(&sub.query);
        }
        nav_goto(engine, &url);
    } else {
        // A POST keeps the action's own query string — only a GET replaces it.
        let target = if action.contains('?') { action.clone() } else { url.clone() };
        post_url(engine, &target, sub.query.as_bytes());
    }
}

/// Follow a link href relative to the current page — new history entry.
fn follow(engine: &Engine, href: &str) {
    let base = url_str().to_string();
    let abs = resolve(&base, href);
    nav_goto(engine, &abs);
}

// ── Back/forward history (bounded list of URLs) ─────────────────────────────

const HIST_MAX: usize = 64;

fn hist_get(i: usize) -> &'static str {
    doc().hist.get(i).map(|s| s.as_str()).unwrap_or("")
}
/// Record a new navigation: truncate forward entries, append (caps at HIST_MAX).
///
/// At the cap the last entry is overwritten and the position stays.
fn hist_push(url: &str) {
    let d = doc_mut();
    if !d.hist.is_empty() && d.hist.get(d.hist_pos).map(|s| s.as_str()) == Some(url) {
        return;
    }
    let new_pos = if d.hist.is_empty() { 0 } else { d.hist_pos + 1 };
    if new_pos >= HIST_MAX {
        if let Some(last) = d.hist.get_mut(HIST_MAX - 1) {
            last.clear();
            last.push_str(clip(url, URL_CAP));
        }
        return;
    }
    // Forward entries are dropped.
    d.hist.truncate(new_pos);
    d.hist.push(String::from(clip(url, URL_CAP)));
    d.hist_pos = new_pos;
}
// Each borrow ends in its own statement: read `doc().hist_pos`, then write,
// then call `hist_get`, rather than holding one reference across all three
// (see `doc`/`doc_mut`).
fn hist_back() -> Option<&'static str> {
    let pos = doc().hist_pos;
    if pos == 0 { return None }
    doc_mut().hist_pos = pos - 1;
    Some(hist_get(pos - 1))
}
fn hist_forward() -> Option<&'static str> {
    let pos = doc().hist_pos;
    if pos + 1 >= doc().hist.len() { return None }
    doc_mut().hist_pos = pos + 1;
    Some(hist_get(pos + 1))
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
/// Resolve a URL against the page's.
///
/// Uses the engine's resolution (`js::url::resolve`, including RFC 3986
/// §5.2.4 dot-segment removal) rather than a second implementation. For the
/// module graph a URL is a key, so unnormalised `./` and `../` would load the
/// same module under several names.
fn resolve(base: &str, href: &str) -> String {
    use beak_engine::js::url;
    let href = href.trim();
    let Some(b) = url::parse_abs(base) else {
        // Without a usable base only the reference itself remains.
        return href.to_string();
    };
    url::resolve(href, &b).href()
}

/// Query the canvas widget's actual laid-out rect (x, y, w, h) in the app's
/// window space. `None` until the compositor has laid it out at least once.
fn canvas_rect() -> Option<(i32, i32, i32, i32)> {
    host::canvas_rect(CANVAS_ID)
}

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

/// A fresh layout, in the middle of a script.
///
/// Called from `Interp` when a page reads the box of an element it inserted
/// in the same step. The same path the frame loop runs per frame — write
/// back the tree, lay out, hand in the boxes — only on demand.
///
/// It does not fill the loop's layout cache (it lives in `main`, out of reach
/// of a `fn` pointer). The next frame lays out again, but it would anyway,
/// since the tree changed and `content_gen` moved on.
///
/// The engine holds the cap against layout thrashing (`FORCED_LAYOUT_CAP`);
/// this is only the work.
fn host_relayout(ip: &mut beak_engine::js::interp::Interp) {
    let Some((_x, _y, w, h)) = canvas_rect() else { return };
    if w <= 0 || h <= 0 { return }
    if let Some(d) = ip.doc.as_mut() {
        engine().set_scripted_dom(Some(d.to_dom()));
    }
    // Tell the loop its cache is stale. Otherwise it would consider it valid,
    // because `to_dom` just cleared `dirty`.
    bump_content_gen("relayout");
    // The user's typed values, as in the last layout. With
    // `FormState::default()` a field with text would be narrower, and that
    // wrong number would go back to the page.
    let forms = engine().last_forms();
    let lay = do_layout(engine(), w as u32, &forms);
    let boxes = alloc::rc::Rc::new(lay.element_rects());
    doc_mut().geom = Some(boxes.clone());
    let max_scroll = (lay.height as i32 - h).max(0);
    let sy = scroll_y().clamp(0, max_scroll);
    ip.set_geometry(beak_engine::js::interp::Geometry {
        boxes, scroll: (0, sy), content: (w, lay.height as i32),
    });
}

fn maybe_repaint(engine: &Engine, cache: &mut Option<(Layout, i32, i32, u32)>, buf: &mut Vec<u8>, state: &FormState) {
    let (_x, _y, w, h) = match canvas_rect() {
        Some(r) => r,
        None => return,
    };
    if w <= 0 || h <= 0 {
        return;
    }
    let dirty = doc().dirty;
    let lw = LAST_W.load(Relaxed);
    let lh = LAST_H.load(Relaxed);
    if !dirty && w == lw && h == lh {
        return;
    }

    // (Re)lay out only when the content or the viewport changed — not on every
    // scroll. Reusing the cached layout for scroll is what keeps scrolling
    // smooth. The height counts only when the page's geometry actually depends
    // on it — a `vh` length that won the cascade, a cap that really clamps, an
    // out-of-flow box anchored to the viewport's bottom edge. `html, body
    // { height: 100% }` is on nearly every site and moves nothing, so it must
    // not count: a window resized by a few pixels would otherwise cost a full
    // re-layout for a picture that cannot change.
    let cur_gen = content_gen();
    let need_layout = match cache.as_ref() {
        None => true,
        Some((lay, cw, ch, cg)) => {
            *cw != w || *cg != cur_gen || (*ch != h && lay.viewport_h_used)
        }
    };
    if need_layout {
        *cache = Some((do_layout(engine, w as u32, state), w, h, cur_gen));
        // The title for the tab strip. Here, not when the document arrives:
        // the engine parses at layout, so until now it still holds the
        // previous page's tree.
        let t = engine.title().unwrap_or_default();
        if t != doc().title {
            doc_mut().title = t;
            render_chrome();
        }
        // Collect the boxes again — only here, not per frame.
        let boxes = cache.as_ref().unwrap().0.element_rects();
        // Assignment, not `ptr::write`: that overwrites without dropping the
        // old value and would leak the previous boxes.
        doc_mut().geom = Some(alloc::rc::Rc::new(boxes));

        // A selection points at op indices, which shift on re-layout (a late
        // image is enough), so the selection is dropped rather than
        // highlighting the wrong text.
        //
        // Find is recomputed instead: its query (the string) still holds,
        // while a selection only had a position. Without jumping, or a late
        // image would yank the page from under the reader.
        if doc().sel.is_some() {
            doc_mut().sel = None;
            doc_mut().sel_anchor = None;
            engine.set_marks(None, Vec::new());
        }
        if doc().find.is_some() {
            find_run(engine, cache, false);
        }
    }
    let layout = &cache.as_ref().unwrap().0;

    let max_scroll = (layout.height as i32 - h).max(0);
    let sy = scroll_y().clamp(0, max_scroll);
    set_scroll(sy);
    // Hand geometry and scroll position to the engine. The scroll position
    // goes every frame because it changes without a layout; the boxes are an
    // `Rc` and cost nothing.
    let geom = doc().geom.clone();
    if let (Some(sess), Some(g)) = (js_session(), geom) {
        // Update the viewport when it moved. The root of an
        // `IntersectionObserver` without its own root is the viewport, and
        // `set_media` runs only once at script start. Only on change, because
        // `set_viewport` builds a fresh `screen`.
        let vp = (w, h);
        if doc().last_vp != vp {
            doc_mut().last_vp = vp;
            sess.interp.set_viewport(w as f64, h as f64);
        }
        sess.interp.set_geometry(beak_engine::js::interp::Geometry {
            boxes: g, scroll: (0, sy),
            // The scroll area as the layout computed it — the same number
            // `max_scroll` is clamped against above.
            // `document.documentElement.scrollHeight` must agree, or a page
            // computes with a height it can never scroll to.
            content: (w, layout.height as i32),
        });
    }

    // Reuse a persistent paint buffer across frames — `engine.paint` fills every
    // pixel (background first), so no re-zeroing is needed, and a scroll repaint
    // does not allocate and zero a full frame.
    let need = (w as usize) * (h as usize) * 4;
    let resized = buf.len() != need;
    if resized {
        buf.resize(need, 0);
    }

    // Scrolling does not change the page; it moves it. When nothing else asked
    // for a repaint, shift the pixels that merely moved and draw only the band
    // that came into view; a full fill is far more expensive than a scroll's
    // few dozen new rows.
    //
    // The inspect overlay is drawn over the frame rather than being part of the
    // display list, so a blit would smear it; that mode takes the full path.
    let dy = sy - LAST_SY.load(Relaxed);
    let full = doc().need_full
        || need_layout
        || resized
        || inspect_mode()
        || dy.abs() >= h;
    // A scroll that the clamp swallowed — at the top or the bottom of the page
    // the offset does not move, so the buffer already holds this exact frame.
    if dy == 0 && !full {
        doc_mut().dirty = false;
        return;
    }
    let t_paint = now_ms();
    if !full {
        let stride = w as usize * 4;
        let rows = h as usize;
        let moved = dy.unsigned_abs() as usize;
        if dy > 0 {
            // Scrolled down: the picture moves up, the new band is at the foot.
            buf.copy_within(moved * stride..rows * stride, 0);
            engine.paint_band(layout, w as u32, h as u32, sy, buf,
                              (rows - moved) as u32, rows as u32);
        } else {
            // Scrolled up: the picture moves down, the new band is at the head.
            buf.copy_within(0..(rows - moved) * stride, moved * stride);
            engine.paint_band(layout, w as u32, h as u32, sy, buf, 0, moved as u32);
        }
    } else {
        engine.paint(layout, w as u32, h as u32, sy, buf);
    }
    // What painting did not find — once per URL.
    log_image_miss(engine);
    // Inspect overlay: outline the selected element box (document → screen).
    if inspect_mode() {
        if let Some((bx, by, bw, bh)) = selected_rect() {
            stroke_rect_bgra(buf, w, h, bx, by - sy, bw, bh, [0, 0, 255]);
        }
    }
    let t_commit = now_ms();
    host::canvas_commit(CANVAS_ID, buf, w as u32, h as u32);
    // Say which path ran. A fast path that never says so looks exactly like one
    // that never happened, and the whole point of this one is a number.
    if full {
        log_ms("paint", t_commit - t_paint);
    } else {
        log(&alloc::format!("[beak] paint band {}px: {} ms",
            dy.unsigned_abs(), t_commit - t_paint));
    }
    log_ms("canvas commit", now_ms() - t_commit);
    // The number that matters: navigation → first pixels.
    //
    // Not while one is still in flight: the old page keeps repainting for
    // scrolls and hovers during a load, and reporting one of those would
    // credit the new navigation with a picture of the previous page.
    {
        if !doc().nav_reported && !nav_busy() {
            doc_mut().nav_reported = true;
            log_ms("=== navigation -> first paint", now_ms() - doc().nav_start_ms);
            // How many of the six built-in faces this page actually needed.
            // They load lazily; without this number "lazy" is only a claim.
            let mut m = String::from("[beak] Schriften: ");
            push_i64(&mut m, engine.loaded_faces() as i64);
            m.push_str(" von 6 Gesichtern geparst");
            log(&m);
        }
    }

    LAST_W.store(w, Relaxed);
    LAST_H.store(h, Relaxed);
    LAST_SY.store(sy, Relaxed);
    doc_mut().need_full = false;
    doc_mut().dirty = false;
}

/// The colour runs of the address bar: the registrable domain at full
/// strength, everything else muted.
///
/// This is the anti-phishing display: in `https://paypal.com.example.ru/login`
/// the domain is `example.ru`, and the eye reads the first thing that looks
/// like a name.
///
/// Computed with `site::registrable_domain`, i.e. the real Public Suffix
/// List — not "the last two labels": for `a.github.io` that would be
/// `github.io`, showing two unrelated user sites as the same.
///
/// Only when the field shows the loaded URL. If it differs, someone is
/// typing, and highlighting half a host would be wrong a keystroke later.
/// While typing the text stays one colour.
fn address_spans(field: &str, url: &str) -> Vec<Span> {
    if field.is_empty() || field != url {
        return Vec::new();
    }
    // The host: after `scheme://`, up to the first `/?#`, without `user@`
    // and without `:port`.
    let after_scheme = match field.find("://") { Some(i) => i + 3, None => 0 };
    let rest = &field[after_scheme..];
    let host_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..host_len];
    let host_at = after_scheme + authority.rfind('@').map(|i| i + 1).unwrap_or(0);
    let host_raw = &field[host_at..after_scheme + host_len];
    // A colon separates the port — but in `[::1]` it belongs to the address,
    // and an IP has no registrable domain anyway.
    let host = match host_raw.rfind(':') {
        Some(i) if !host_raw.contains(']') => &host_raw[..i],
        _ => host_raw,
    };
    let Some(dom) = beak_engine::site::registrable_domain(host) else { return Vec::new() };
    // `registrable_domain` returns lowercase; we search in the original, and
    // it is always a suffix of the host.
    if host.len() < dom.len() { return Vec::new() }
    let start = host_at + (host.len() - dom.len());
    let end = start + dom.len();
    let muted = |a: usize, b: usize| Span {
        start: a as u32, len: (b - a) as u32, token: Token::OnSurfaceMuted,
    };
    let mut out = Vec::new();
    if start > 0 { out.push(muted(0, start)); }
    out.push(Span { start: start as u32, len: dom.len() as u32, token: Token::OnSurface });
    if end < field.len() { out.push(muted(end, field.len())); }
    out
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
    // scheme belongs in the field, not in the URL text (docs/spec/UI_REFRESH.md §5).
    // The lock belongs to the loaded document, the text to the field: while
    // someone types, the lock still tells the truth about the page shown.
    let url = url_str();
    let field = edit_str();
    let spans = address_spans(field, url);
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
                value: field.to_string(),
                placeholder: s().address_placeholder.to_string(),
                on_submit: ActionId(ACT_GO),
                modifiers: {
                    let mut m = vec![Modifier::Flex(1)];
                    if !spans.is_empty() { m.push(Modifier::Spans(spans)); }
                    m
                },
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
            // One button, two jobs: reload when the page is settled, stop
            // while it is loading. It is also the only thing on screen that
            // says a fetch is running at all.
            if nav_busy() {
                nav_button(IconId::X, ActionId(ACT_STOP))
            } else {
                nav_button(IconId::ArrowClockwise, ActionId(ACT_RELOAD))
            },
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
        tab_strip(),
        toolbar,
        Widget::Divider,
        Widget::Canvas {
            id: CanvasId(CANVAS_ID),
            width: 800,
            height: 600,
            modifiers: vec![Modifier::Flex(1), Modifier::Background(Token::Page)],
        },
    ];

    // The find bar. Not a `Widget::Input` but text — the buffer belongs to
    // beak, see `Doc::find`. It sits below the canvas as in other browsers.
    if let Some(q) = doc().find.clone() {
        let (at, n) = (doc().find_at, doc().found.len());
        let mut label = String::from("Suchen: ");
        label.push_str(&q);
        label.push('\u{2502}');           // a vertical bar as caret
        if !q.is_empty() {
            label.push_str("    ");
            if n == 0 {
                label.push_str("nichts gefunden");
            } else {
                push_i64(&mut label, at as i64 + 1);
                label.push_str(" von ");
                push_i64(&mut label, n as i64);
            }
        }
        label.push_str("      Enter = weiter · Esc = zu");
        children.push(Widget::Divider);
        children.push(Widget::Text {
            content: label,
            style: TextStyle::Body,
            modifiers: vec![
                Modifier::Padding(Padding::Sm.as_u16()),
                Modifier::Background(Token::SurfaceElevated),
                Modifier::Tint(if n == 0 && !q.is_empty() {
                    Token::OnSurfaceMuted
                } else {
                    Token::OnSurface
                }),
            ],
        });
    }

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
            if !host::scene_commit(&b) {
                log("[beak] commit failed");
            }
        }
        Err(_) => log("[beak] encode failed"),
    }
}

/// Toolbar chrome sizing (docs/spec/UI_REFRESH.md §5).
const TOOLBAR_H: u16 = 44;
const FIELD_H: u16 = 30;
const NAV_BTN: u16 = 28;
const NAV_BTN_RADIUS: u8 = 7;

/// The tab strip: 36 px, `SurfaceElevated`, tabs bottom-aligned
/// (`docs/spec/UI_REFRESH.md` §5.2 and §3 `tab`).
const TABSTRIP_H: u16 = 36;
/// The accent bar on top of the active tab.
const TAB_ACCENT: u16 = 2;
const TAB_H: u16 = 30;
/// Fixed width, not adaptive (§3 `tab`). An adaptive tab needs a label that
/// adapts with it, and the font belongs to the compositor — an app cannot
/// measure it.
const TAB_W: u16 = 160;
/// 22, so a 16 px icon fits. The atlas holds 16, 24, 32, 48 and 64; any
/// other size is downscaled with area averaging, which blurs thin strokes.
/// Requesting an atlas size is a 1:1 blit and stays sharp.
const TAB_BTN: u16 = 22;
/// How much text fits into a tab, in characters.
///
/// Assumes 7 px per character for `TextStyle::Body` (13 px) — an estimate,
/// deliberately high: a short label is still a label, a long one is a bug,
/// because the compositor does not clip text but paints it over the
/// neighbour.
const TAB_CHARS: usize = ((TAB_W - 16 - TAB_BTN - 4) / 7) as usize;

/// A small button in the strip: a tab's `\u{d7}`, the `+` after them.
fn tab_btn(icon: IconId, action: ActionId) -> Widget {
    prefab::center_box(
        Widget::Icon { id: icon, size: 16, modifiers: vec![Modifier::Tint(Token::OnSurfaceMuted)] },
        vec![
            Modifier::MinWidth(TAB_BTN),
            Modifier::MaxWidth(TAB_BTN),
            Modifier::MinHeight(TAB_BTN),
            Modifier::Rounded(Radius::Sm.as_u8()),
            Modifier::OnClick(action),
            Modifier::Hover(vec![
                Modifier::Background(Token::SurfaceHover),
                Modifier::Rounded(Radius::Sm.as_u8()),
            ]),
        ],
    )
}

/// The tab strip.
///
/// Always shown, even for a single tab — the `+` is the only place a second
/// one can be opened without knowing the shortcut.
fn tab_strip() -> Widget {
    let a = active();
    let n = tabs().len();
    let mut kids: Vec<Widget> = Vec::with_capacity(n + 1);
    for i in 0..n {
        let sel = i == a;
        let label = tab_label(&tabs()[i]);
        let mut m = vec![
            Modifier::OnClick(ActionId(ACT_TAB_SEL + i as u32)),
            Modifier::MinWidth(TAB_W),
            Modifier::MaxWidth(TAB_W),
            Modifier::MinHeight(TAB_H),
            Modifier::Rounded(Radius::Md.as_u8()),
        ];
        // The active tab carries the colour of the content below (§3
        // `tab`): it is the page, the strip is the frame.
        if sel {
            m.push(Modifier::Background(Token::Surface));
        } else {
            m.push(Modifier::Hover(vec![
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Rounded(Radius::Md.as_u8()),
            ]));
        }
        // An accent bar on top of the active tab, the usual marker. It sits
        // inside the tab, not above it, or it would shift the active tab's
        // label against the others.
        let bar = Widget::Row {
            children: Vec::new(),
            spacing: 0,
            align: Align::Center,
            modifiers: vec![
                Modifier::MinHeight(TAB_ACCENT),
                Modifier::MaxHeight(TAB_ACCENT),
                Modifier::Background(if sel { Token::Accent } else { Token::SurfaceElevated }),
            ],
        };
        let inner = Widget::Row {
            children: vec![
                Widget::Text {
                    content: label,
                    style: TextStyle::Body,
                    modifiers: vec![
                        Modifier::Flex(1),
                        Modifier::Tint(if sel { Token::OnSurface } else { Token::OnSurfaceMuted }),
                    ],
                },
                tab_btn(IconId::X, ActionId(ACT_TAB_CLOSE + i as u32)),
            ],
            spacing: Spacing::Xs.as_u16(),
            align: Align::Center,
            modifiers: vec![
                Modifier::Flex(1),
                Modifier::PaddingXY { x: 8, y: 0 },
            ],
        };
        // `Stretch`, not `Start`: in a column `align` is the cross axis, the
        // width. With `Start` the accent bar would get its natural width,
        // which for a row without children is zero.
        kids.push(Widget::Column {
            children: vec![bar, inner],
            spacing: 0,
            align: Align::Stretch,
            modifiers: m,
        });
    }
    kids.push(tab_btn(IconId::Plus, ActionId(ACT_TAB_NEW)));
    kids.push(Widget::Spacer { flex: 1 });
    Widget::Row {
        children: kids,
        spacing: Spacing::Xxs.as_u16(),
        align: Align::End,          // bottom-aligned
        modifiers: vec![
            Modifier::MinHeight(TABSTRIP_H),
            Modifier::Background(Token::SurfaceElevated),
            Modifier::PaddingXY { x: 6, y: 0 },
        ],
    }
}

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

// ── Find in page ──────────────────────────────────────────────────────────

/// Recompute the search and highlight the matches.
///
/// `jump` scrolls to the current match. Wanted while typing (to see whether
/// it exists), not on a mere re-layout — or a late image would yank the page
/// from under the reader.
fn find_run(engine: &Engine, cache: &Option<(Layout, i32, i32, u32)>, jump: bool) {
    let Some(q) = doc().find.clone() else { return };
    let Some((lay, _, _, _)) = cache.as_ref() else { return };
    let hits = if q.is_empty() { Vec::new() } else { engine.find_all(lay, &q) };
    let d = doc_mut();
    if d.find_at >= hits.len() { d.find_at = 0 }
    d.found = hits;
    let (at, n) = (d.find_at, d.found.len());
    let cur = d.found.get(at).copied();
    engine.set_marks(d.sel, d.found.clone());
    // Scroll the match into view — a third from the top, not at the edge,
    // where a match on the last line is hard to read.
    if jump && n > 0 {
        if let Some((a, b)) = cur {
            if let Some((_, _, _, ch)) = canvas_rect() {
                let rects = engine.selection_rects(lay, a, b);
                if let Some((_, ry, _, _)) = rects.first() {
                    let want = (*ry - ch / 3).max(0);
                    if (want - scroll_y()).abs() > ch / 8 { set_scroll(want); }
                }
            }
        }
    }
    mark_dirty();
}

/// One match forward (or back), wrapping around.
fn find_step(engine: &Engine, cache: &Option<(Layout, i32, i32, u32)>, back: bool) {
    let n = doc().found.len();
    if n == 0 { return }
    let d = doc_mut();
    d.find_at = if back { (d.find_at + n - 1) % n } else { (d.find_at + 1) % n };
    find_run(engine, cache, true);
}

/// Close the bar and clean up.
fn find_close(engine: &Engine) {
    let d = doc_mut();
    d.find = None;
    d.found.clear();
    d.find_at = 0;
    d.find_pending_len = 0;
    engine.set_marks(d.sel, Vec::new());
    mark_dirty();
    render_chrome();
}

/// Accumulate a key byte into a character.
///
/// A key press carries one byte, and U+00E4 is two. Everything from 0x80 is
/// part of a UTF-8 sequence; `b as char` would read it as Latin-1 and turn
/// the lead byte 0xC3 into U+00C3. `None` means the sequence is not complete yet — then
/// nothing happens, not even a repaint.
///
/// One place for both of beak's own inputs (page forms and the find bar).
fn utf8_feed(pending: &mut [u8; 4], len: &mut u8, b: u8) -> Option<char> {
    if b < 0x80 {
        *len = 0;
        return Some(b as char);
    }
    if b >= 0xC0 { *len = 0; }              // new sequence
    let n = *len as usize;
    if n < 4 { pending[n] = b; *len = n as u8 + 1; } else { *len = 0; }
    let take = *len as usize;
    match core::str::from_utf8(&pending[..take]).ok().and_then(|t| t.chars().next()) {
        Some(ch) => { *len = 0; Some(ch) }
        None => None,
    }
}

/// What a key is called in the page's language: `key` (the value), `code`
/// (the position on the keyboard) and the legacy `keyCode`.
///
/// Three names, because pages read all three: modern JS reads `key`,
/// shortcuts read `code` (it survives another layout), and legacy code reads
/// `keyCode`/`which`.
fn key_names(key: KeyCode, pending: bool) -> Option<(String, String, u32, bool)> {
    let s = |a: &str| String::from(a);
    Some(match key {
        // Half a UTF-8 character is not a key yet — it is reported once
        // complete.
        KeyCode::Char(_) if pending => return None,
        KeyCode::Char(b' ') => (s(" "), s("Space"), 32, false),
        KeyCode::Char(b) if b >= 0x20 && b != 0x7F && b < 0x80 => {
            let c = b as char;
            let code = if c.is_ascii_alphabetic() {
                alloc::format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                alloc::format!("Digit{c}")
            } else { s("") };
            // `keyCode` is the upper-case letter even when typed lower case —
            // the legacy value knows only the key, not the case.
            (alloc::format!("{c}"), code, c.to_ascii_uppercase() as u32,
             c.is_ascii_uppercase())
        }
        // Everything from 0x80 is a complete character from `utf8_feed`; its
        // `code` depends on a layout we do not know, so it stays empty.
        KeyCode::Char(_) => (s(""), s(""), 0, false),
        KeyCode::Backspace => (s("Backspace"), s("Backspace"), 8, false),
        KeyCode::Delete => (s("Delete"), s("Delete"), 46, false),
        KeyCode::Enter => (s("Enter"), s("Enter"), 13, false),
        KeyCode::Escape => (s("Escape"), s("Escape"), 27, false),
        KeyCode::Left => (s("ArrowLeft"), s("ArrowLeft"), 37, false),
        KeyCode::Right => (s("ArrowRight"), s("ArrowRight"), 39, false),
        KeyCode::Up => (s("ArrowUp"), s("ArrowUp"), 38, false),
        KeyCode::Down => (s("ArrowDown"), s("ArrowDown"), 40, false),
        KeyCode::Home => (s("Home"), s("Home"), 36, false),
        KeyCode::End => (s("End"), s("End"), 35, false),
        KeyCode::PageUp => (s("PageUp"), s("PageUp"), 33, false),
        KeyCode::PageDown => (s("PageDown"), s("PageDown"), 34, false),
        KeyCode::Tab => (s("Tab"), s("Tab"), 9, false),
        _ => return None,
    })
}

/// Dispatch a UI event to the page and finish the round.
///
/// The round is part of it: a handler that starts a `fetch` or sets a
/// `setTimeout` (every suggestion list does) needs the microtasks and timers,
/// or its work sits idle until the next event. Returns whether the page
/// cancelled.
fn fire_ui(engine: &Engine, f: impl FnOnce(&mut beak_engine::js::interp::Interp) -> bool) -> bool {
    let Some(sess) = js_session() else { return false };
    arm_script_budget();
    let prevented = f(&mut sess.interp);
    let _ = sess.interp.run_timers();
    sync_cookies(sess);
    sync_history(engine, sess);
    sync_scroll(sess);
    drain_console(sess);
    if sess.interp.doc.as_ref().is_some_and(|d| d.dirty) {
        if let Some(d) = sess.interp.doc.as_mut() {
            engine.set_scripted_dom(Some(d.to_dom()));
        }
        bump_content_gen("ui-event");
        mark_dirty();
    }
    prevented
}

/// Apply one key to the focused control. Returns true if the page must be
/// re-laid-out (the control's painted text or caret changed).
fn edit_key(engine: &Engine, page: &mut Page, key: KeyCode) -> bool {
    let (seq, kind, mut value) = match page.focused() {
        Some((c, v)) => (c.seq, c.kind, v.to_string()),
        None => return false,
    };
    // `keydown` first, and its cancellation counts: that is how input fields
    // filter characters (UI Events §5.4). Otherwise a page would believe it
    // prevented a key that still arrives.
    if let Some((k, code, kc, shift)) = key_names(key, page.state.pending_len > 0) {
        if fire_ui(engine, |ip| {
            beak_engine::js::dombind::dispatch_key(ip, "keydown", seq, &k, &code, kc, shift)
        }) {
            return true;
        }
    }
    if !kind.is_text() {
        // Space / Enter activate a button or toggle a box, like a browser.
        return match key {
            KeyCode::Enter | KeyCode::Char(b' ') => {
                activate(engine, page, seq);
                true
            }
            KeyCode::Escape => {
                set_focus(engine, page, None);
                true
            }
            _ => false,
        };
    }
    let mut caret = page.state.caret.min(value.len());
    // What was inserted — `InputEvent.data`. `None` on deletion, which is
    // the spec's `null`, not a placeholder.
    let mut typed: Option<String> = None;
    match key {
        // Not only ASCII: bytes from 0x80 are parts of a UTF-8 sequence,
        // collected by `utf8_feed`.
        KeyCode::Char(b) if b >= 0x20 && b != 0x7F => {
            match utf8_feed(&mut page.state.pending, &mut page.state.pending_len, b) {
                Some(ch) => {
                    value.insert(caret, ch);
                    caret += ch.len_utf8();
                    typed = Some(alloc::format!("{ch}"));
                }
                // Sequence not complete yet: do nothing, paint nothing.
                None => return false,
            }
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
            set_focus(engine, page, None);
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
            push_control_values(page);
            submit_form(engine, page, activated);
            return true;
        }
        _ => return false,
    }
    page.state.set_value(seq, value);
    page.state.caret = caret;
    // Restart the blink cycle: right after a key press the caret is solid.
    doc_mut().caret_since = now_ms();
    doc_mut().caret_phase = None;
    // Into the tree, on every key press. The `seq` the value is stored under
    // in `FormState` is valid only until the next `to_dom`; the tree node
    // stays valid. It is the bridge `sync` pulls the value back over, and
    // what a page handler reads.
    push_control_values(page);
    // Now announce the change. The value is already in the node, so a
    // handler reading `e.target.value` gets it. `input` bubbles and is not
    // cancelable; suggestion lists, live filters and counters depend on it.
    let (itype, data) = match key {
        KeyCode::Backspace => ("deleteContentBackward", None),
        KeyCode::Delete => ("deleteContentForward", None),
        _ => ("insertText", typed.clone()),
    };
    fire_ui(engine, |ip| {
        beak_engine::js::dombind::dispatch_input_event(ip, "input", seq, itype, data.as_deref())
    });
    if let Some((k, code, kc, shift)) = key_names(key, false) {
        fire_ui(engine, |ip| {
            beak_engine::js::dombind::dispatch_key(ip, "keyup", seq, &k, &code, kc, shift)
        });
    }
    true
}

/// Change focus — and tell the page.
///
/// One path for all callers. `focus`/`blur` do not bubble,
/// `focusin`/`focusout` do (UI Events §5.2), and pages use both: a listener
/// on the form instead of the field hears only the latter. `change` fires
/// here too — for a text field on leaving, and only if the value changed
/// since it gained focus (HTML §4.10.5.5), not per character.
fn set_focus(engine: &Engine, page: &mut Page, next: Option<u32>) {
    let prev = page.state.focus;
    if prev == next { return }
    if let Some(old) = prev {
        let now = page.forms.get(old).map(|c| page.state.value(c).to_string());
        let changed = page.forms.get(old).is_some_and(|c| c.kind.is_text())
            && now != page.focus_value;
        if changed {
            fire_ui(engine, |ip| {
                beak_engine::js::dombind::dispatch_seq(ip, "change", old)
            });
        }
        fire_ui(engine, |ip| {
            beak_engine::js::dombind::dispatch_focus(ip, "blur", old, next)
        });
        fire_ui(engine, |ip| {
            beak_engine::js::dombind::dispatch_focus(ip, "focusout", old, next)
        });
    }
    page.state.focus = next;
    doc_mut().caret_since = now_ms();
    doc_mut().caret_phase = None;
    page.focus_value = next
        .and_then(|n| page.forms.get(n).map(|c| page.state.value(c).to_string()));
    if let Some(new) = next {
        fire_ui(engine, |ip| {
            beak_engine::js::dombind::dispatch_focus(ip, "focus", new, prev)
        });
        fire_ui(engine, |ip| {
            beak_engine::js::dombind::dispatch_focus(ip, "focusin", new, prev)
        });
    }
}

/// `mousedown`/`mouseup` at the pointer position.
///
/// A `click` alone is not enough for pages that open a menu on press and
/// select on release.
fn dispatch_mouse_edge(engine: &Engine, lay: &Layout, kind: &'static str, cx: i32, cy: i32) {
    let Some(sess) = js_session() else { return };
    if !sess.interp.doc.as_ref().is_some_and(|d| d.has_listeners) { return }
    let chain = lay.element_chain(cx, cy);
    if chain.is_empty() { return }
    let Some(doc) = sess.interp.doc.as_ref() else { return };
    let nodes: Vec<u32> = chain.iter().filter_map(|s| doc.by_seq(*s)).collect();
    if nodes.is_empty() { return }
    let sy = scroll_y() as f64;
    fire_ui(engine, move |ip| {
        matches!(beak_engine::js::dombind::dispatch_at(ip, kind, &nodes,
            Some((cx as f64, cy as f64 - sy, cx as f64, cy as f64))), Ok(true))
    });
}

/// The text caret blinks, and only the band it sits in is repainted.
///
/// 530 ms is the half period browsers use. After a key press the cycle
/// restarts and the caret stays solid, so a typist can see where it is.
///
/// Costs something only while a text field has focus; otherwise the first
/// check returns and it is one comparison per round.
fn blink_caret(engine: &Engine, cache: &Option<(Layout, i32, i32, u32)>,
               buf: &mut [u8], page: &Page) {
    let focused_text = page.state.focus
        .and_then(|f| page.forms.get(f))
        .is_some_and(|c| c.kind.is_text());
    if !focused_text {
        doc_mut().caret_phase = None;
        return;
    }
    let Some((lay, _, _, _)) = cache.as_ref() else { return };
    let Some((_, cy, _, chh)) = lay.caret_rect() else { return };
    let Some((_, _, w, h)) = canvas_rect() else { return };
    if w <= 0 || h <= 0 || buf.len() < (w * h * 4) as usize { return }
    let on = ((now_ms() - doc().caret_since) / 530) % 2 == 0;
    if doc().caret_phase == Some(on) { return }
    doc_mut().caret_phase = Some(on);
    // Only the caret's rows, in window coordinates and clamped.
    let sy = scroll_y();
    let (y0, y1) = ((cy - sy).max(0), (cy - sy + chh).min(h));
    if y1 <= y0 { return }
    engine.set_caret_on(on);
    engine.paint_band(lay, w as u32, h as u32, sy, buf, y0 as u32, y1 as u32);
    engine.set_caret_on(true);
    host::canvas_commit(CANVAS_ID, buf, w as u32, h as u32);
}

/// Report `scroll` and `resize` to the page — after the frame, not during.
///
/// Only on change, hence its own memory: `scroll_y` says where we paint,
/// `told_scroll` what the page last heard. Without the difference the event
/// would fire every frame or never.
fn fire_viewport_events(engine: &Engine) {
    let sy = scroll_y();
    let vp = canvas_rect().map(|(_, _, w, h)| (w, h)).unwrap_or((0, 0));
    let scrolled = sy != doc().told_scroll;
    let resized = vp != doc().told_vp && doc().told_vp != (0, 0);
    if !scrolled && !resized { doc_mut().told_vp = vp; return }
    doc_mut().told_scroll = sy;
    doc_mut().told_vp = vp;
    // Only if anyone listens — otherwise every scroll costs a pass through
    // the engine for nothing.
    if !js_session().is_some_and(|s| s.interp.doc.as_ref().is_some_and(|d| d.has_listeners)) {
        return;
    }
    if scrolled {
        fire_ui(engine, |ip| {
            let Some(d) = ip.doc.as_ref() else { return false };
            let doc_node = d.doc;
            matches!(beak_engine::js::dombind::dispatch(ip, "scroll", &[doc_node]), Ok(true))
        });
    }
    if resized {
        fire_ui(engine, |ip| {
            let Some(d) = ip.doc.as_ref() else { return false };
            let doc_node = d.doc;
            matches!(beak_engine::js::dombind::dispatch(ip, "resize", &[doc_node]), Ok(true))
        });
    }
}

/// Click / keyboard activation of a control: submit, toggle, or take focus.
fn activate(engine: &Engine, page: &mut Page, seq: u32) {
    let kind = match page.forms.get(seq) {
        Some(c) => c.kind,
        None => return,
    };
    match kind {
        ControlKind::Submit => {
            set_focus(engine, page, Some(seq));
            submit_form(engine, page, Some(seq));
        }
        ControlKind::Reset => {
            page.state.reset();
        }
        ControlKind::Checkbox | ControlKind::Radio => {
            set_focus(engine, page, Some(seq));
            let f = &page.forms;
            page.state.toggle(f, seq);
            // Same reason as for typing: the check belongs in the tree, or it
            // does not survive the next `to_dom`.
            push_control_values(page);
        }
        ControlKind::Select => {
            set_focus(engine, page, Some(seq));
            let f = &page.forms;
            page.state.cycle_select(f, seq);
            push_control_values(page);
        }
        _ => {
            // A text field takes focus with the caret at the end.
            set_focus(engine, page, Some(seq));
            page.state.caret = page.forms.get(seq).map(|c| page.state.value(c).len()).unwrap_or(0);
        }
    }
}

/// An event that changes only the state of one control: try a repaint first,
/// and lay out the page only if that is not possible.
///
/// The difference is large: a full layout costs orders of magnitude more than
/// a repaint. Before adding a new cause here, check that it really affects
/// only one box; `repaint_controls` itself says no if not.
fn restate_control(engine: &Engine, cache: &mut Option<(Layout, i32, i32, u32)>,
                   state: &FormState, why: &str) {
    let done = cache.as_mut().is_some_and(|(lay, ..)| engine.repaint_controls(lay, state));
    if !done {
        // Say once per page why a layout is needed. Without it a fast path
        // that never runs looks the same as one never needed.
        say_ctl_bail_once(engine.repaint_bail());
        bump_content_gen(why);
    }
    mark_dirty();
}

fn say_ctl_bail_once(why: &str) {
    if doc().ctl_bail_said || why.is_empty() {
        return;
    }
    doc_mut().ctl_bail_said = true;
    log(&alloc::format!("[beak] Steuerelement neu malen geht nicht: {why}"));
}

/// Handle one event. Returns true if the chrome (address bar / title) should
/// be re-committed.
///
/// A navigation started inside the page — submitting a form, following a
/// link — changes the address without anyone touching the address bar.
/// Rather than flag each path, compare the navigation counter that a real
/// page load bumps: a path added later cannot forget it. Navigations
/// themselves complete in `nav_pump`, which reports its own redraw.
fn handle(engine: &Engine, ev: Event, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) -> bool {
    let nav = nav_gen();
    let chrome = handle_event(engine, ev, cache, page);
    chrome || nav_gen() != nav
}

fn handle_event(engine: &Engine, ev: Event, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) -> bool {
    match ev {
        // The text field only. Not `set_url`: that reports the network
        // context to the kernel, which resolves it — a key press is not a
        // navigation.
        Event::InputChange { value } => {
            set_edit(&value);
            // Typing in the address bar means the compositor moved keyboard
            // focus there — drop the page control's focus so only one caret
            // blinks and Enter goes to the right place.
            if page.state.focus.take().is_some() {
                restate_control(engine, cache, &page.state, "addressbar-focus");
            }
            false
        }
        // Ctrl+F. Arrives as a chord because `Event::Key` carries no
        // modifiers; otherwise Ctrl+F could not be told from a typed "f".
        Event::Chord { letter: b'f', .. } => {
            if doc().find.is_none() { doc_mut().find = Some(String::new()); }
            doc_mut().find_pending_len = 0;
            render_chrome();
            mark_dirty();
            true
        }
        // Ctrl+T / Ctrl+W / Ctrl+1..9. There is no Ctrl+Tab: `Event::Chord`
        // carries a letter the kernel builds from `KeyCode::Char`, and
        // `KeyCode::Tab` is not a `Char`. Digits are, so numbers are the way
        // to a specific tab.
        Event::Chord { letter: b't', .. } => {
            tab_open(engine, "", false, cache, page);
            true
        }
        Event::Chord { letter: b'w', .. } => {
            tab_close(engine, active(), cache, page);
            true
        }
        // `9` is the last tab, not the ninth, as in other browsers.
        Event::Chord { letter: b'9', .. } => {
            tab_activate(engine, tabs().len() - 1, cache, page);
            true
        }
        Event::Chord { letter: d @ b'1'..=b'8', .. } => {
            tab_activate(engine, (d - b'1') as usize, cache, page);
            true
        }
        // The find bar is open: the keys belong to it. They only arrive when
        // no compositor text field has focus — someone who clicks into the
        // address bar keeps typing there, which is right.
        Event::Key(k) if doc().find.is_some() => {
            match k {
                KeyCode::Escape => { find_close(engine); return true }
                KeyCode::Enter => { find_step(engine, cache, false); render_chrome(); return true }
                KeyCode::Backspace => {
                    let d = doc_mut();
                    d.find_pending_len = 0;
                    if let Some(q) = d.find.as_mut() { q.pop(); }
                    d.find_at = 0;
                }
                KeyCode::Char(b) if b >= 0x20 && b != 0x7F => {
                    let mut pend = doc().find_pending;
                    let mut len = doc().find_pending_len;
                    let ch = utf8_feed(&mut pend, &mut len, b);
                    let d = doc_mut();
                    d.find_pending = pend;
                    d.find_pending_len = len;
                    let Some(ch) = ch else { return true };   // sequence incomplete
                    if let Some(q) = d.find.as_mut() { q.push(ch); }
                    d.find_at = 0;
                }
                _ => return true,
            }
            find_run(engine, cache, true);
            render_chrome();
            true
        }
        // A page control has focus → the key is ours (the compositor only
        // routes keys here when no chrome Input/TextArea consumed them).
        Event::Key(k) if page.state.focus.is_some() => {
            if edit_key(engine, page, k) {
                restate_control(engine, cache, &page.state, "form-key");
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
            mark_dirty_scrolled();
            false
        }
        Event::Action(ActionId(id)) => match id {
            ACT_GO => {
                // What the bar shows, not where we are.
                let t = edit_str().to_string();
                go(engine, &t);
                set_open_menu(0);
                true
            }
            ACT_RELOAD | ACT_VIEW_RELOAD => {
                let t = url_str().to_string();
                if !t.is_empty() {
                    fetch_url(engine, &t);
                }
                set_open_menu(0);
                true
            }
            ACT_STOP => {
                nav_cancel();
                subresources_cancel();
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
                    fetch_url(engine, u);
                }
                set_open_menu(0);
                true
            }
            ACT_FORWARD => {
                if let Some(u) = hist_forward() {
                    fetch_url(engine, u);
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
                host::close_widget();
                true
            }
            ACT_TAB_NEW => {
                tab_open(engine, "", false, cache, page);
                set_open_menu(0);
                true
            }
            // Two id bands, no field per tab: the strip is rebuilt on every
            // change, so the index is the tab.
            id if (ACT_TAB_SEL..ACT_TAB_SEL + MAX_TABS as u32).contains(&id) => {
                tab_activate(engine, (id - ACT_TAB_SEL) as usize, cache, page);
                set_open_menu(0);
                true
            }
            id if (ACT_TAB_CLOSE..ACT_TAB_CLOSE + MAX_TABS as u32).contains(&id) => {
                tab_close(engine, (id - ACT_TAB_CLOSE) as usize, cache, page);
                set_open_menu(0);
                true
            }
            _ => false,
        },
        // Link clicks land in the canvas → hit-test the engine's link rects.
        // Only clicks inside the canvas are ours. Menu-bar/toolbar clicks
        // are delivered here too (as a MouseButton alongside their Action);
        // touching the open menu on those would close the dropdown the same
        // click just opened. Those are handled entirely by their Action / the
        // Popover's on_dismiss.
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
                    // The cache must match the current state; if it does not,
                    // the new layout is stored rather than thrown away.
                    // `content_gen` rises on every hover, so moving the pointer
                    // onto a target and clicking it would otherwise lay out
                    // twice.
                    let stale = !matches!(cache.as_ref(),
                        Some((_, cw, ch, cg)) if *cw == w && *ch == h && *cg == content_gen());
                    if stale {
                        *cache = Some((do_layout(engine, w as u32, &page.state), w, h, content_gen()));
                    }
                    // Fetch everything the layout can answer before the first
                    // change to the cache — afterwards it is mutably borrowed
                    // and `lay` is gone.
                    let (dispatched, inspect_sel, ctl_seq, href, toggle) = {
                        let lay = &cache.as_ref().unwrap().0;
                        // The page gets the click first. If a handler calls
                        // `preventDefault`, the click is consumed — otherwise
                        // beak would also follow the link the page just
                        // intercepted.
                        dispatch_mouse_edge(engine, lay, "mousedown", cx, cy);
                        let dispatched = dispatch_click(engine, page, lay, cx, cy);
                        (
                            dispatched,
                            // Inspect mode only: the test runs over all boxes,
                            // and a click on a large page should not pay for it
                            // when nobody is looking.
                            inspect_mode()
                                .then(|| lay.hit_inspect(cx, cy).map(|b| (b.x, b.y, b.w, b.h, b.label.clone())))
                                .flatten(),
                            lay.hit_control(cx, cy).map(|c| c.seq),
                            lay.hit_test(cx, cy).map(|s| s.to_string()),
                            lay.hit_toggle(cx, cy),
                        )
                    };
                    if dispatched { return true; }
                    // Inspect mode intercepts the click: select the deepest
                    // element box under the cursor (shown as an outline + a
                    // status-bar label) instead of following a link.
                    if inspect_mode() {
                        // Also echo to the serial console so it can be copied
                        // without transcribing from the screen.
                        if let Some((bx, by, _, _, ref label)) = inspect_sel {
                            log(&alloc::format!("[inspect] @({bx},{by}) {label}"));
                        } else {
                            log("[inspect] (no element here)");
                        }
                        set_selected(inspect_sel);
                        mark_dirty();
                        return true;
                    }
                    // A control wins over a link: a submit button inside an
                    // <a>, or a field overlapping a link rect, is the target.
                    if let Some(seq) = ctl_seq {
                        activate(engine, page, seq);
                        restate_control(engine, cache, &page.state, "control-activate");
                        return true;
                    }
                    // Clicking the page elsewhere blurs a focused control.
                    if page.state.focus.take().is_some() {
                        restate_control(engine, cache, &page.state, "control-blur");
                    }
                    // A link is followed on release, not on press, as in
                    // other browsers; that is what allows selecting text that
                    // starts on a link.
                    //
                    // Only links move to release; script, controls and
                    // `<summary>` still act on press.
                    if let Some(href) = href {
                        doc_mut().pending_link = Some((href, x, y));
                        // A selection anchor is still set — that is the point.
                        let lay = &cache.as_ref().unwrap().0;
                        let had = doc_mut().sel.take();
                        doc_mut().sel_anchor = engine.text_pos_at(lay, cx, cy);
                        if had.is_some() {
                            engine.set_marks(None, Vec::new());
                            mark_dirty();
                        }
                        return true;
                    }
                    // A `<summary>` opens/closes its section. It comes after
                    // the control and the link: a link inside a summary
                    // navigates, which is what a browser does too.
                    if let Some(seq) = toggle {
                        if engine.toggle_details(seq) {
                            bump_content_gen("details-toggle");
                            mark_dirty();
                        }
                        return true;
                    }
                    // Nothing else wanted this click, so a selection starts
                    // here. It comes last so it gets in the way of no link,
                    // control or page script.
                    let lay = &cache.as_ref().unwrap().0;
                    let d = doc_mut();
                    let was = d.sel.take();
                    d.sel_anchor = engine.text_pos_at(lay, cx, cy);
                    engine.set_marks(None, Vec::new());
                    if was.is_some() { mark_dirty(); }
                    return was.is_some();
                }
            }
            false
        }
        // Release: the selection is fixed, dragging is over — and here it is
        // decided whether the press was a click or the start of a selection.
        Event::MouseButton { button: MouseButton::Left, down: false, x, y } => {
            doc_mut().sel_anchor = None;
            // Before the link logic and independent of it: `mouseup` fires
            // where no link is, too (a slider that snaps on release).
            if let Some((rx, ry, _, _)) = canvas_rect() {
                if let Some((lay, _, _, _)) = cache.as_ref() {
                    dispatch_mouse_edge(engine, lay, "mouseup", x - rx, y - ry + scroll_y());
                }
            }
            let Some((href, px, py)) = doc_mut().pending_link.take() else { return false };
            // Dragged? Then it was no navigation. Four pixels of tolerance
            // so a shaky hand still clicks.
            if doc().sel.is_some() || (x - px).abs() > 4 || (y - py).abs() > 4 {
                return false;
            }
            follow(engine, &href);
            true
        }
        // Ctrl+C on the page. The compositor intercepts the chord only when
        // one of its own text fields has focus (`handle_input_key` returns
        // early otherwise); on the canvas it arrives here.
        Event::Clipboard(ClipKind::Copy) => {
            let Some((a, b)) = doc().sel else { return false };
            let Some((lay, _, _, _)) = cache.as_ref() else { return false };
            let text = engine.selected_text(lay, a, b);
            if text.is_empty() { return false }
            let n = clipboard_set(&text);
            log(&alloc::format!("[beak] kopiert: {} Zeichen", if n < 0 { 0 } else { n }));
            true
        }
        // `:hover`. A series of ever-cheaper ways to answer "nothing to do":
        // no hover rules on the page at all, then no usable cached layout,
        // then the same element as last time. What is left is answered by
        // repainting the display list in place where that is provably enough,
        // and only otherwise by laying the page out again.
        Event::MouseMove { x, y } => {
            // Dragging comes before hover. Someone selecting wants no
            // `:hover` work in between, and the earliest exit of this arm
            // ("the page has no hover rules") would swallow the selection.
            if doc().sel_anchor.is_some() {
                if let Some((rx, ry, w, h)) = canvas_rect() {
                    let (cx, cy) = (x - rx, y - ry + scroll_y());
                    let fresh = matches!(cache.as_ref(),
                        Some((_, cw, ch, cg)) if *cw == w && *ch == h && *cg == content_gen());
                    if fresh {
                        let lay = &cache.as_ref().unwrap().0;
                        let a = doc().sel_anchor.unwrap();
                        if let Some(b) = engine.text_pos_at(lay, cx, cy) {
                            // A point is not a selection — otherwise every
                            // click would flash a one-pixel highlight.
                            let sel = (a != b).then_some((a, b));
                            if doc().sel != sel {
                                doc_mut().sel = sel;
                                engine.set_marks(sel, Vec::new());
                                mark_dirty();
                            }
                        }
                    }
                }
                return true;
            }
            if !engine.page_has_hover() {
                return false;
            }
            let Some((rx, ry, w, h)) = canvas_rect() else {
                return false;
            };
            // Leaving the canvas has to clear the hover, or whatever the pointer
            // left behind stays lit for good.
            let inside = x >= rx && x < rx + w && y >= ry && y < ry + h;
            let hovered = if inside {
                match cache.as_ref() {
                    Some((lay, cw, ch, cg)) if *cw == w && *ch == h && *cg == content_gen() => {
                        lay.hover_at(x - rx, y - ry + scroll_y())
                    }
                    // No layout to hit-test against. Laying one out just to
                    // answer where the pointer is would cost the very thing
                    // this arm is trying to avoid.
                    _ => return false,
                }
            } else {
                Vec::new()
            };
            match engine.set_hover(hovered) {
                HoverChange::Unchanged => {}
                HoverChange::Changed { paint_only } => {
                    let t0 = now_ms();
                    let repainted = paint_only
                        && cache.as_mut().is_some_and(|(lay, ..)| engine.repaint_hover(lay));
                    if repainted {
                        say_hover_once(true, now_ms() - t0, "");
                        mark_dirty();
                    } else if hover_affordable() {
                        say_hover_once(
                            false,
                            0,
                            if paint_only { engine.repaint_bail() } else { "a rule moves a box" },
                        );
                        bump_content_gen("hover");
                        mark_dirty();
                    } else {
                        // Neither cheap enough to repaint nor affordable to lay
                        // out: put the state back, or the next layout that runs
                        // for some other reason lights up a pointer that has
                        // long moved on.
                        engine.revert_hover();
                    }
                }
            }
            false
        }
        Event::Wheel { dy } => {
            set_scroll(scroll_y() + dy);
            mark_dirty_scrolled();
            false
        }
        // Middle click on a link: new tab in the background.
        //
        // The page does not get it. A middle click is `auxclick`, not
        // `click`; dispatching it as `click` would be worse than omitting it,
        // since a handler calling `preventDefault` means the left button.
        Event::MouseButton { button: MouseButton::Middle, down: true, x, y } => {
            let Some((rx, ry, w, h)) = canvas_rect() else { return false };
            if x < rx || x >= rx + w || y < ry || y >= ry + h {
                return false;
            }
            let (cx, cy) = (x - rx, y - ry + scroll_y());
            let href = match cache.as_ref() {
                Some((lay, ..)) => lay.hit_test(cx, cy).map(|s| s.to_string()),
                None => None,
            };
            let Some(href) = href else { return false };
            let abs = resolve(url_str(), &href);
            tab_open(engine, &abs, true, cache, page);
            true
        }
        // `npk_open` on a running instance: a second open is a new tab, not
        // a replacement of what is shown.
        Event::Open(s) => {
            let Some(abs) = typed_to_url(&s) else { return false };
            tab_open(engine, &abs, false, cache, page);
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

fn poll_event(buf: &mut [u8]) -> PollResult {
    match nopeek_widgets::events::poll(buf) {
        nopeek_widgets::events::Poll::Event(ev) => PollResult::Event(ev),
        nopeek_widgets::events::Poll::Empty => PollResult::Empty,
        nopeek_widgets::events::Poll::Gone => PollResult::WindowGone,
    }
}

// ── Heap: a real free-list allocator. The six font faces (persistent) + each
//    frame's layout + paint buffer are freed on drop, unlike a bump heap. ───
//
// The growing heap itself lives in the SDK (`nopeek_widgets::heap`), shared
// with tune.
#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

use nopeek_widgets::heap::u32_str;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log("[beak] PANIC");
    if let Some(loc) = info.location() {
        log(loc.file());
        log(u32_str(loc.line()));
    } else {
        log("[beak] no location (likely alloc failure)");
    }
    // Trap — do not `loop {}`. A wasm `unreachable` makes `_start`'s host call
    // return Err, so the kernel tears this instance down and frees its worker
    // core. A busy loop would instead pin the core forever (fibers are
    // cooperative → a spinning fiber never yields) and freeze the machine.
    // Cleanly dying is the whole point of the per-tab sandbox.
    core::arch::wasm32::unreachable()
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // No heap init here — the allocator claims the heap lazily on first use.

    // Launch argument: `npk_open("beak", "https://…")` → prime the address bar
    // now; the actual fetch waits until the engine is set up (below).
    let mut arg = [0u8; PAYLOAD_CAP];
    let arg_len = host::launch_arg(&mut arg).unwrap_or(0).min(PAYLOAD_CAP);
    if arg_len > 0 {
        set_url(core::str::from_utf8(&arg[..arg_len]).unwrap_or(""));
    }

    // Commit the chrome immediately so the window is an opaque browser from
    // the first frame, before any slow startup work.
    render_chrome();

    // Which build is actually running. Without this a serial trace cannot say
    // whether a measurement belongs to the version that was just installed —
    // and a perf number from the wrong build is worse than no number.
    log(concat!("[beak] version ", env!("CARGO_PKG_VERSION")));

    // Lend the engine the kernel's randomness. The engine has no host
    // functions; it is handed one, like the clock. Without it a page has no
    // `crypto` — the right answer when there is no real source, rather than
    // passing off `Math.random` as secure.
    beak_engine::js::random::set_source(random_bytes);

    // Font faces are built lazily on first use; the log line after the first
    // paint says how many a page needed.
    engine_mut().set_theme(query_theme());
    // Lend the engine our tick source so it can report the per-phase split.
    engine().set_clock(host::ticks_ms);
    // Restore stored cookies before the first request goes out, or the start
    // page would load logged out.
    cookies_restore();
    log("[beak] engine ready");

    // Engine is up — fetch the launch URL now (if we were opened with one).
    if arg_len > 0 {
        let u = url_str().to_string();
        go(engine(), &u);
    }

    // Cached layout: (Layout, width, height, content generation).
    let mut cache: Option<(Layout, i32, i32, u32)> = None;
    // The page's forms + the user's edits, rebuilt on every navigation.
    let mut page = Page::new();
    // Persistent paint buffer, reused across frames (see maybe_repaint).
    let mut paint_buf: Vec<u8> = Vec::new();
    let mut event_buf = vec![0u8; EVENT_BUF_SIZE];

    loop {
        // Drain the entire event queue this tick, then repaint once.
        // Coalescing collapses a burst of wheel notches into one scroll step
        // + one repaint, and lets the loop reach the idle sleep.
        let mut chrome = false;
        let mut had_event = false;
        loop {
            match poll_event(&mut event_buf) {
                PollResult::Event(ev) => {
                    had_event = true;
                    if handle(engine(), ev, &mut cache, &mut page) {
                        chrome = true;
                    }
                }
                PollResult::Empty => break,
                PollResult::WindowGone => {
                    host::close_widget();
                    return;
                }
            }
        }
        // Take delivery of whatever the kernel finished while we were
        // painting: the document, or its stylesheets. This is where a
        // navigation completes — no path through `handle` waits for one.
        if nav_pump(engine()) {
            chrome = true;
        }
        // …and only then re-parse the document's forms, so the page that just
        // arrived is laid out against its own controls rather than the
        // previous page's. Cheap when nothing navigated.
        //
        // Pull values from the tree only if it was really re-collected: the
        // tree holds what page code set, and taking it every round would
        // overwrite what the user types. A tree rebuild is the only event
        // after which the tree knows more than the host.
        //
        // The session is passed in, not fetched again — two `js_session`
        // calls would be two mutable borrows of the same field.
        if page.sync(engine()) {
            if let Some(s) = js_session() { pull_control_values(&mut page, s); }
        }
        // A form the page wants to submit while loading must not wait for
        // the next click.
        let pending = js_session().map(|s| s.interp.take_submits()).unwrap_or_default();
        for seq in pending {
            log(&alloc::format!("[beak] script submit: form seq={seq}"));
            if submit_form_seq(engine(), &page, seq) { break }
        }
        // And a navigation the page requested. Handled centrally here, not
        // at each entry point: load scripts, timers, `load` handlers and
        // clicks all end up in the same round.
        sync_nav(engine());
        if chrome {
            render_chrome();
        }
        if !had_event {
            // No theme watch: the page palette does not follow the desktop
            // (see `query_theme`), so a light/dark switch changes only the
            // chrome and needs no re-layout.
        }
        // A fresh page: drop the old page's decoded images and note which ones
        // it wants (fetching them happens after the repaint, one batch a turn).
        //
        // This has to sit after `nav_pump` and before the repaint. A page is
        // completed by `nav_pump`, so from the top of the loop this would
        // always be one turn late: the page would be laid out once against
        // the previous page's images and then again.
        if images_dirty() {
            let q = begin_images(engine_mut());
            engine().css_images_begin();
            let d = doc_mut();
            d.pending_imgs = q;
            d.pending_css_imgs.clear();
            d.css_asked.clear();
        }
        maybe_repaint(engine(), &mut cache, &mut paint_buf, &page.state);
        // After the frame: the page hears what moved. Not during
        // `maybe_repaint` — the cache is mutably borrowed there, and a
        // handler changing the tree would pull it from under the frame.
        fire_viewport_events(engine());
        blink_caret(engine(), &cache, &mut paint_buf, &page);
        // The page's own fonts. After the first layout: before it nobody
        // knows which ones it requests.
        if pump_fonts(engine()) {
            bump_content_gen("font");
            mark_dirty();
        }
        // The open WebSockets. A message arrives unprompted, so they are
        // polled every frame. Events from them are entry points like any
        // other, so the same follow-up work runs, under the same condition.
        let ws_moved = match js_session() {
            Some(s) => pump_websockets(s),
            None => false,
        };
        // What `fetch()` requested. When an answer arrives page code ran,
        // and it may have rebuilt the tree.
        if pump_fetches() | ws_moved {
            if let Some(s) = js_session() {
                let n = s.interp.run_timers();
                let _ = n;
                // A `fetch` callback is an entry point like any other: it may
                // set a cookie and rewrite the URL, and the next request must
                // carry that cookie.
                sync_cookies(s);
                sync_history(engine(), s);
                sync_scroll(s);
                drain_console(s);
                if s.interp.doc.as_ref().is_some_and(|d| d.dirty) {
                    if let Some(d) = s.interp.doc.as_mut() {
                        engine().set_scripted_dom(Some(d.to_dom()));
                    }
                    bump_content_gen("fetch");
                    mark_dirty();
                }
            }
        }
        // The box observers. `set_geometry` measured during painting;
        // delivery happens here, because a callback is an entry point and
        // does not belong in the middle of painting.
        //
        // It must be here: a page without timers or events would otherwise
        // register its observers and never see a callback.
        pump_box_observers(engine());
        // Now geometry exists — `load` may fire.
        // `pageshow` fires after `load`, once per navigation (HTML §7.11.4).
        // `persisted` is always `false`: beak has no page cache.
        if fire_load(engine(), &page) {
            if let Some(sess) = js_session() {
                if let Some(dn) = sess.interp.doc.as_ref().map(|d| d.doc) {
                    let _ = beak_engine::js::dombind::dispatch(&mut sess.interp, "pageshow", &[dn]);
                }
            }
            page.sync(engine());
            if let Some(s) = js_session() { pull_control_values(&mut page, s); }
        }
        // The visible document band, read after the repaint clamped the scroll
        // offset. Without a canvas the band is everything, so an arriving
        // image always repaints — the conservative direction.
        let band = match canvas_rect() {
            Some((_, _, _, h)) => {
                let sy = scroll_y();
                (sy, sy.saturating_add(h))
            }
            None => (0, i32::MAX),
        };
        // Text and layout are on screen now — pull in the next few images,
        // then come back round and paint them. Scrolling keeps working in
        // between, because a batch is small. The layout is passed whole: it
        // answers both questions needed (did a guessed box land, and is the
        // picture even on screen).
        let layout = cache.as_ref().map(|(l, _, _, _): &(Layout, i32, i32, u32)| l);
        // The queue is taken out and put back rather than borrowed in place:
        // `pump_images` calls `doc()` itself, and a reference held alongside
        // would be the second borrow `doc`/`doc_mut` warns about.
        let mut q = core::mem::take(&mut doc_mut().pending_imgs);
        pump_images(engine_mut(), &mut q, layout, band);
        doc_mut().pending_imgs = q;
        // The layout reports which CSS images it needs, so this queue can only
        // be filled after a layout — unlike `<img>`, whose srcs are in the HTML
        // and are queued once by `begin_images`.
        //
        // The cached layout keeps listing the same srcs every turn (a
        // background arriving is a repaint, not a re-layout), so the guard
        // must be "already asked for this page", not "already in the queue":
        // the queue empties on every fetch. Cleared on navigation, with the
        // engine's cache.
        let mut css_adopted: Vec<u64> = Vec::new();
        let mut css_asked = core::mem::take(&mut doc_mut().css_asked);
        let mut pending_css_imgs = core::mem::take(&mut doc_mut().pending_css_imgs);
        if let Some((l, _, _, _)) = cache.as_ref() {
            for (k, u) in &l.css_image_srcs {
                if css_asked.contains(k) {
                    continue;
                }
                css_asked.push(*k);
                // Same question as for `<img>`, one layer later: the layout
                // only names its background images after it has run, so this
                // cannot happen in `begin_images`. The url is resolved exactly
                // as `pump_css_images` resolves it, or put and get would
                // use different keys for one picture.
                if engine().adopt_css_cached(*k, &resolve(url_str(), u)) {
                    css_adopted.push(*k);
                } else {
                    pending_css_imgs.push((*k, u.clone()));
                }
            }
        }
        // An adopted layer arrives after this turn's paint, so it needs the
        // next one — but only if it would show. Same test the fetched ones get.
        if !css_adopted.is_empty() {
            match cache.as_ref() {
                Some((l, _, _, _)) if !l.css_images_in_band(&css_adopted, band.0, band.1) => {}
                _ => mark_dirty(),
            }
        }
        let layout = cache.as_ref().map(|(l, _, _, _): &(Layout, i32, i32, u32)| l);
        pump_css_images(engine(), &mut pending_css_imgs, layout, band);
        doc_mut().css_asked = css_asked;
        doc_mut().pending_css_imgs = pending_css_imgs;
        // Always yield so this worker core can halt — a cooperative fiber that
        // never sleeps pins its core at 100%. A short nap while interacting
        // stays responsive; a longer one when idle keeps the core asleep.
        // Anything on the wire keeps the short nap: that is how often we
        // ask the kernel whether the answer is here. A running font round
        // counts too, or the page would stay unstyled longer.
        let waiting = nav_busy() || img_job() >= 0 || cssimg_job() >= 0
            || font_job() >= 0;
        let busy = had_event
            || waiting
            || !doc().pending_imgs.is_empty()
            || !doc().pending_css_imgs.is_empty();
        let nap = if busy { 4 } else { 16 };
        host::sleep_ms(nap);
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<app_meta::IconRef> {
    None
}
