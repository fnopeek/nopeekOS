//! beak — native, sandboxed web browser for nopeekOS (docs/spec/BROWSER.md).
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
use talc::TalcLock;

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
    /// Seconds since the epoch, UTC. `npk_ticks` cannot stand in: it restarts
    /// at every boot and a cookie's `Expires` is an absolute date.
    fn npk_unix_time() -> i64;
    fn npk_http_final_url(buf_ptr: i32, buf_max: i32) -> i32;
    /// Why the last request failed: `kind\tmessage`. Cleared on success.
    fn npk_http_last_error(buf_ptr: i32, buf_max: i32) -> i32;
    /// The last response's Content-Type, verbatim. -1 if the server sent none.
    fn npk_http_content_type(buf_ptr: i32, buf_max: i32) -> i32;
    /// Sagt dem Kernel, WELCHES Dokument gerade angezeigt wird. Er loest die
    /// Adresse selbst auf und merkt sich nur die Netzklasse; daran haengt,
    /// ob eine Unterressource ins private Netz darf.
    /// Siehe `docs/plan/BROWSER_FETCH_ORIGIN.md` §3.1 V2.
    fn npk_net_context(url_ptr: i32, url_len: i32) -> i32;
    /// Start a newline-separated list of URLs in one call, multiplexed over
    /// HTTP/2 where the host offers it. Same handle discipline as
    /// `npk_http_begin`.
    fn npk_http_begin_many(urls_ptr: i32, urls_len: i32, out_max: i32) -> i32;
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
    fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32) -> i32;
    fn npk_canvas_rect(canvas_id: i32, out_ptr: i32) -> i32;
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

/// The page palette: the canvas a document is painted on when it paints none
/// of its own, and the colours it inherits.
///
/// **This is deliberately NOT the desktop theme.** It used to be, and that
/// made every page built for a white canvas unreadable on a dark desktop:
/// the page sets only its text colour — near-black, because it expects white
/// behind it — and we put dark grey behind that. Google's consent page is the
/// exact case, measured 2026-08-09: 19 KB of CSS, zero `color-scheme`, zero
/// `prefers-color-scheme`, zero `light-dark()`, and a `body` rule that sets
/// no background at all. A browser paints that white. So do we now.
///
/// The two halves of "dark mode" were one value here and they are not one
/// thing: what the USER prefers (a media query) and what the CANVAS is (the
/// used `color-scheme` of the root, which is light until a page opts in).
/// Reporting a preference we cannot honour without making pages unreadable is
/// the worse half to keep, so both are light until `color-scheme` is parsed —
/// then a page that opts in gets a dark canvas AND a dark preference, which
/// is the whole rule rather than half of it.
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

const CANVAS_ID: i32 = 1;

// Toolbar
const ACT_GO: u32 = 1;
const ACT_BACK: u32 = 2;
const ACT_FORWARD: u32 = 3;
const ACT_RELOAD: u32 = 4;
/// Give up on the page being loaded. The reload button becomes this while a
/// navigation is in the air — which is only possible now that one IS in the
/// air rather than in a host call nobody can interrupt.
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
// A fetch-queue bound, NOT a memory bound — the pixel budget in the engine is
// what stops a page from eating the machine, and the heap grows now. This only
// keeps one absurd document from queueing thousands of round-trips, and it says
// so when it bites.
const MAX_IMAGES: usize = 512;
static mut IMG_FETCH_BUF: [u8; IMG_FETCH_CAP] = [0; IMG_FETCH_CAP];

/// How many images one batch asks for. Small on purpose: the batch is a
/// blocking call, so a whole page in one go would freeze the window again —
/// the very thing progressive loading fixed. Four is enough to overlap the
/// round-trips while a turn of the loop stays short.
const IMG_BATCH: usize = 4;

/// Receives the per-URL length table from `npk_http_take_many`. Sized for
/// the largest batch either caller asks for.
static mut LENS_BUF: [u8; 4 * MAX_CSS_LINKS] = [0; 4 * MAX_CSS_LINKS];

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
    unsafe { npk_http_begin_many(blob.as_ptr() as i32, blob.len() as i32, cap as i32) }
}

/// Collect a finished batch into `dst`, returning each body as a
/// `(offset, len)` span.
///
/// Returns an empty vec if the batch failed, which the callers treat the
/// same as "none of them loaded" — every one of them degrades to a
/// placeholder or to unstyled content rather than to a blank page.
fn take_batch(handle: i32, dst: *mut u8, cap: usize, want: usize) -> Vec<(usize, usize)> {
    let lens = core::ptr::addr_of_mut!(LENS_BUF) as *mut u8;
    let n = unsafe {
        npk_http_take_many(
            handle,
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
    for i in 0..(n as usize).min(want) {
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
/// Die Kaesten des letzten Layouts, fuer `getBoundingClientRect` & Co.
///
/// Als `Rc` gehalten, damit das Weiterreichen an die JS-Maschine nichts
/// kostet: der Rollstand aendert sich bei JEDEM Bild, die Kaesten nur bei
/// einem neuen Layout — ohne das waere jede Rollbewegung eine Kopie von
/// ~180 KB.
static mut GEOM: Option<alloc::rc::Rc<alloc::vec::Vec<beak_engine::layout::ElemRect>>> = None;
static mut LAST_W: i32 = -1;
static mut LAST_H: i32 = -1;
/// The scroll offset the buffer currently HOLDS, so the next frame knows how
/// far the picture has to move.
static mut LAST_SY: i32 = 0;
/// Something other than scrolling wants a repaint.
///
/// Scrolling does not change the page, it moves it — so a frame that is dirty
/// for scrolling ALONE can be blitted and have one band redrawn. Anything else
/// (a hover, a form key, a new layout) sets this and gets the whole viewport.
/// It is set, never cleared, until a frame is actually painted: a hover
/// followed by a scroll must still repaint everything.
static mut NEED_FULL: bool = true;

/// Dem Kernel sagen, aus welchem Dokument die naechsten Anfragen kommen.
///
/// **Der Kernel glaubt uns die Adresse, aber nicht die Klasse** — er loest
/// selbst auf. Was das garantiert: Seitencode kann den Kontext nie
/// erweitern, weil Seitencode keinen Weg zu einer Host-Funktion hat. Was es
/// NICHT garantiert: dass beak selbst sich nicht vertut. Dafuer gibt es nur
/// diese eine Stelle und `set_url` — beide unten.
fn tell_net_context(url: &str) {
    unsafe { npk_net_context(url.as_ptr() as i32, url.len() as i32) };
}

fn set_url(s: &str) {
    // Nach dem Laden noch einmal, mit der Adresse, aus der das Dokument
    // WIRKLICH kam (nach Weiterleitungen). `nav_begin` hat vorher schon die
    // des Ziels gemeldet; hier wird sie richtiggestellt.
    tell_net_context(s);
    let n = s.len().min(URL_CAP);
    unsafe {
        core::ptr::copy_nonoverlapping(s.as_ptr(), core::ptr::addr_of_mut!(URL_BUF) as *mut u8, n);
        core::ptr::addr_of_mut!(URL_LEN).write(n);
        // A new page gets its own verdict on whether it can afford `:hover` —
        // and its own chance to say so once. Without this, one heavy page
        // silences the pointer for every page after it.
        core::ptr::addr_of_mut!(HOVER_REFUSED).write(false);
        core::ptr::addr_of_mut!(HOVER_SAID_FAST).write(false);
        core::ptr::addr_of_mut!(HOVER_SAID_SLOW).write(false);
        core::ptr::addr_of_mut!(CTL_BAIL_SAID).write(false);
        core::ptr::addr_of_mut!(LAST_LAYOUT_MS).write(0);
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
    /// Stand des Baums, aus dem `forms` gebaut wurde.
    scripted: u64,
}

impl Page {
    fn new() -> Page {
        Page { forms: forms::Forms { forms: Vec::new(), controls: Vec::new() },
               state: FormState::default(), nav: 0, scripted: 0 }
    }
    /// Das Formularmodell nachziehen — nach einer Navigation ODER nachdem
    /// Skripte den Baum ersetzt haben.
    ///
    /// **Aus dem LEBENDEN Baum, nicht aus dem Quelltext.** Eine Seite, die
    /// ihre Maske erst per Skript baut, hatte hier vorher gar keine
    /// Steuerelemente: das Bild zeigte ein Anmeldeformular, `submit` sagte
    /// „kein zugehoeriges Formular". Das ist kein Sonderfall einer Seite,
    /// sondern die Regel bei allem, was seine Oberflaeche zur Laufzeit baut.
    fn sync(&mut self, engine: &Engine) {
        let g = nav_gen();
        let sg = engine.scripted_gen();
        if self.nav == g && self.scripted == sg {
            return;
        }
        let navigated = self.nav != g;
        self.nav = g;
        self.scripted = sg;
        self.forms = match engine.with_scripted(forms::collect) {
            Some(f) => f,
            None => forms::collect(&beak_engine::parse(html_str())),
        };
        // Eine NAVIGATION wirft die Eingaben weg, ein Skriptlauf nicht — was
        // der Benutzer getippt hat, gehoert ihm, auch wenn die Seite daneben
        // etwas umbaut.
        if navigated { self.state.reset(); }
        self.log_forms();
    }

    /// Report the forms this page offers, once per navigation.
    ///
    /// "The button does nothing" has several very different causes — a button
    /// we never saw, one whose form we could not resolve, a form whose action
    /// we misread — and from the outside they look identical. Three lines on
    /// the serial tell them apart, which a screenshot cannot: a picture shows
    /// the pixels, not who owns which control.
    fn log_forms(&self) {
        if self.forms.controls.is_empty() {
            return;
        }
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
// with just our UA sheet (docs/spec/BROWSER.md §9.7 — never worse than clean content).
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
    let ms = now_ms() - t0;
    // The width belongs IN the number. A device timing without it cannot be
    // compared with anything -- 1000 px against 1880 px is a factor of 1.75 --
    // and reading it off the screen means opening a window, which changes the
    // very width being measured.
    let mut label = String::from("layout @");
    push_i64(&mut label, w as i64);
    label.push_str("px (parse+cascade+layout)");
    log_ms(&label, ms);
    unsafe { core::ptr::addr_of_mut!(LAST_LAYOUT_MS).write(ms) };
    // ...and WHICH of the three it was. The host profile says the box layout
    // dominates, but the host is not a WASM interpreter and the phases do not
    // scale alike under one: beak 0.18.0 halved the box layout on the host and
    // moved the device number by nothing at all. One number cannot say why.
    let p = lay.phase;
    if p[0] + p[1] + p[2] > 0 {
        log_ms("  dom::parse", p[0] as i64);
        log_ms("  css::cascade", p[1] as i64);
        log_ms("  box layout", p[2] as i64);
    }
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
    unsafe {
        core::ptr::addr_of_mut!(DIRTY).write(true);
        core::ptr::addr_of_mut!(NEED_FULL).write(true);
    }
}

/// Dirty because the viewport MOVED — the display list is untouched.
fn mark_dirty_scrolled() {
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

/// What the last full layout cost, ms. `:hover` needs one per element the
/// pointer enters, so this is what decides whether the page can afford to
/// react to the pointer at all.
static mut LAST_LAYOUT_MS: i64 = 0;
/// A pointer that costs more than this to follow makes the page feel broken —
/// the window stops answering while it re-lays-out. Below it, hover is free
/// enough to be worth having. Only the FALLBACK is measured against it: a
/// pointer change the engine can answer by repainting costs a fraction of a
/// millisecond and is never refused.
const HOVER_BUDGET_MS: i64 = 250;
/// Did we already say that this page is too heavy to hover? Said once per
/// page, not once per mouse move ([[feedback-log-the-exception-not-the-rule]]).
static mut HOVER_REFUSED: bool = false;
/// Has the first pointer answer on this page been reported yet — the repaint
/// with what it cost, and separately the first fallback with its reason?
///
/// Once each per page, for the same reason. Without them a pointer answered by
/// repainting is INVISIBLE in the log, so a device run cannot tell "it works"
/// from "it never happened" — which is exactly what the first 0.28.0 log could
/// not say ([[feedback-log-the-version-in-the-trace]]).
static mut HOVER_SAID_FAST: bool = false;
static mut HOVER_SAID_SLOW: bool = false;

/// Say ONCE per page how the pointer is being answered here.
fn say_hover_once(fast: bool, ms: i64, why: &str) {
    unsafe {
        let p = if fast {
            core::ptr::addr_of_mut!(HOVER_SAID_FAST)
        } else {
            core::ptr::addr_of_mut!(HOVER_SAID_SLOW)
        };
        if p.read() {
            return;
        }
        p.write(true);
    }
    let mut b = String::new();
    if fast {
        b.push_str("[beak] :hover repainted in ");
        b.push_str(&alloc::format!("{ms} ms (a layout here costs {})", unsafe {
            core::ptr::addr_of!(LAST_LAYOUT_MS).read()
        }));
    } else {
        b.push_str("[beak] :hover needs a layout: ");
        b.push_str(why);
    }
    log(&b);
}

/// Can this page afford to restyle on pointer movement?
fn hover_affordable() -> bool {
    let ms = unsafe { core::ptr::addr_of!(LAST_LAYOUT_MS).read() };
    if ms <= HOVER_BUDGET_MS {
        return true;
    }
    unsafe {
        let p = core::ptr::addr_of_mut!(HOVER_REFUSED);
        if !p.read() {
            p.write(true);
            let mut b = String::new();
            b.push_str("[beak] :hover needs a layout here, and one costs ");
            b.push_str(&alloc::format!("{ms} ms"));
            log(&b);
        }
    }
    false
}

/// Why the last fetch failed, as `(kind, message)`. `None` if the kernel
/// reported nothing — which includes an older kernel without the host fn,
/// so the caller must have a fallback rather than assume this is present.
fn last_error() -> Option<(String, String)> {
    const ERR_CAP: usize = 512;
    static mut ERR_BUF: [u8; ERR_CAP] = [0; ERR_CAP];
    let dst = core::ptr::addr_of_mut!(ERR_BUF) as *mut u8;
    let n = unsafe { npk_http_last_error(dst as i32, ERR_CAP as i32) };
    if n <= 0 {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(dst as *const u8, n as usize) };
    let s = core::str::from_utf8(bytes).ok()?;
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
    unsafe {
        let dst = core::ptr::addr_of_mut!(HTML_BUF) as *mut u8;
        core::ptr::copy_nonoverlapping(doc.as_ptr(), dst, len);
        core::ptr::addr_of_mut!(HTML_LEN).write(len);
        // The page carries its own inline <style> and links nothing, so any
        // leftover author CSS from the previous page must go — otherwise the
        // last site's rules would style this one.
        core::ptr::addr_of_mut!(CSS_LEN).write(0);
    }
}

/// The last response's Content-Type. `None` if the server sent none — or if
/// the kernel is older than the host fn, which is why every caller has to
/// cope with not knowing rather than assume UTF-8.
fn content_type() -> Option<String> {
    const CT_CAP: usize = 256;
    static mut CT_BUF: [u8; CT_CAP] = [0; CT_CAP];
    let dst = core::ptr::addr_of_mut!(CT_BUF) as *mut u8;
    let n = unsafe { npk_http_content_type(dst as i32, CT_CAP as i32) };
    if n <= 0 {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(dst as *const u8, n as usize) };
    core::str::from_utf8(bytes).ok().map(|s| s.to_string())
}

/// Bring the freshly fetched document to valid UTF-8, in place.
///
/// Must run before ANYTHING reads `html_str()` — the stylesheet scan does,
/// and a document still holding raw Latin-1 reads back as the empty string.
fn decode_document() {
    let len = unsafe { core::ptr::addr_of!(HTML_LEN).read() };
    if len == 0 {
        return;
    }
    let ct = content_type();
    let buf = unsafe {
        core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(HTML_BUF) as *mut u8, HTML_CAP)
    };
    let (n, how) = charset::to_utf8_in_place(buf, len, ct.as_deref());
    unsafe { core::ptr::addr_of_mut!(HTML_LEN).write(n) };
    if how != charset::KEPT {
        log(&alloc::format!("[beak] document charset: {} ({} -> {} B)", how, len, n));
    }
}

/// Same for the concatenated stylesheets. No Content-Type here — they arrive
/// through the batch fetch, which reports one status per URL and no headers —
/// so this is sniff-only. One bad byte used to cost the page ALL its CSS.
fn decode_css() {
    let len = unsafe { core::ptr::addr_of!(CSS_LEN).read() };
    if len == 0 {
        return;
    }
    let buf = unsafe {
        core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(CSS_BUF) as *mut u8, CSS_CAP)
    };
    let (n, how) = charset::to_utf8_in_place(buf, len, None);
    unsafe { core::ptr::addr_of_mut!(CSS_LEN).write(n) };
    if how != charset::KEPT {
        log(&alloc::format!("[beak] css charset: {} ({} -> {} B)", how, len, n));
    }
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

/// Room for one response's header block — the kernel caps what it hands back
/// at 8 KiB, and a page that sets a dozen cookies still fits.
const HDR_CAP: usize = 8 * 1024;
static mut HDR_BUF: [u8; HDR_CAP] = [0; HDR_CAP];

// ── A navigation that runs while the window stays alive ───────────────────
//
// A page load is two round trips — the document, then its stylesheets — and
// neither may be waited for. Both are started here and collected by
// `nav_pump` on a later turn of the loop, so between them beak paints,
// scrolls and answers keys exactly as it does when idle.
//
// The two stages stay strictly ordered, and NOTHING is painted between them:
// stylesheets are render-blocking, and drawing the bare document first would
// cost a full layout (1,7 s on the device) that the arriving CSS throws away
// on the very next turn.

#[derive(Clone, Copy, PartialEq)]
enum NavStage {
    Doc,
    Css,
    /// Die externen Skripte der Seite. Eine dritte Rundreise, und sie kommt
    /// NACH den Stilblaettern: ein Skript liest Klassen und Groessen.
    Js,
    /// Der Modulgraph. Anders als die drei davor ist das KEINE einzelne
    /// Rundreise: ein Modul nennt seine Abhaengigkeiten erst, wenn es da ist,
    /// also geht es rundenweise, bis der Graph geschlossen ist.
    Mod,
    /// Stilblaetter, die ein SKRIPT eingehaengt hat. Auch rundenweise: ein
    /// Blatt, das ankommt, laesst eine Komponente fertig bauen, und die haengt
    /// ihrerseits eins ein.
    Sheet,
}

/// Handle of the navigation in flight, or -1.
static mut NAV_JOB: i32 = -1;
static mut NAV_STAGE: NavStage = NavStage::Doc;
/// The address that was ASKED for. The diagnostic page names it, and it
/// stands in for the base URL if the response never said where it came from.
static mut NAV_URL: Option<String> = None;
/// Record the landing address in the history once the document is here.
/// Where we LANDED, not where we aimed — otherwise every trip back through
/// history replays the redirect.
static mut NAV_PUSH_HIST: bool = false;
/// When the stage in flight started, so each round trip reports its own span
/// instead of the navigation's total.
static mut NAV_STAGE_MS: i64 = 0;

fn nav_job() -> i32 {
    unsafe { core::ptr::addr_of!(NAV_JOB).read() }
}

/// Is a page load in the air? The toolbar asks (its reload button becomes a
/// stop button), and so does the idle nap.
fn nav_busy() -> bool {
    nav_job() >= 0
}

fn nav_clear() {
    unsafe {
        core::ptr::addr_of_mut!(NAV_SCRIPTS).write(None);
        core::ptr::addr_of_mut!(NAV_JOB).write(-1);
        core::ptr::addr_of_mut!(NAV_URL).write(None);
        core::ptr::addr_of_mut!(NAV_PUSH_HIST).write(false);
    }
}

/// Drop a navigation still in the air — a second click, or Stop. The kernel
/// throws its answer away; nothing on screen changes, so the page that is
/// already there stays readable.
fn nav_cancel() {
    let h = nav_job();
    if h >= 0 {
        unsafe { npk_http_cancel(h) };
    }
    nav_clear();
}

/// The address the navigation in flight asked for.
fn nav_asked() -> String {
    unsafe { (*core::ptr::addr_of!(NAV_URL)).clone() }.unwrap_or_default()
}

/// Start a navigation and return at once. `push_hist` records the address we
/// land on once the document is here.
fn nav_begin(engine: &Engine, method: &str, url: &str, body: &[u8], extra: &str, push_hist: bool) {
    // **Vor dem ersten Byte.** Eine Navigation darf ueberallhin — auch auf
    // den eigenen Router —, denn das neue Dokument ist eine andere Herkunft
    // und die alte Seite kann es nicht lesen. Gemeldet wird deshalb die
    // Klasse des ZIELS, nicht die der Seite, die wir verlassen; sonst waere
    // `https://192.168.1.1` von einer oeffentlichen Seite aus gesperrt.
    tell_net_context(url);
    // A new navigation replaces the old one, and takes the page it was
    // loading for with it — a browser that keeps fetching the pictures of the
    // page you just left is spending the network on nothing.
    nav_cancel();
    subresources_cancel();
    // Und den vom Skript veraenderten Baum, sonst zeigt die naechste Seite den
    // der vorigen — der Zwischenspeicher haengt am HTML, nicht am Baum.
    engine.set_scripted_dom(None);
    engine.set_hit_all(false);
    // Die Sitzung gehoert der Seite, die gerade verlassen wird: ihre
    // Behandler zeigen auf Knoten, die es gleich nicht mehr gibt.
    unsafe { core::ptr::addr_of_mut!(JS).write(None) };

    // Die eingebaute Pruefseite kommt aus dem Binaerbild, nicht aus dem Netz.
    // Sie durchlaeuft ab hier denselben Weg wie ein geholtes Dokument — nur
    // ohne die erste Rundreise.
    if selftest::matches(url) {
        deliver_builtin(engine, selftest::URL, selftest::HTML, push_hist);
        return;
    }

    let now = unsafe { npk_unix_time() };
    let mut hdrs = String::new();
    let jar = cookies::header_for(url, now);
    if !jar.is_empty() {
        hdrs.push_str("Cookie: ");
        hdrs.push_str(&jar);
    }
    if !extra.is_empty() {
        if !hdrs.is_empty() {
            hdrs.push('\n');
        }
        hdrs.push_str(extra);
    }
    let t_nav = now_ms();
    unsafe {
        core::ptr::addr_of_mut!(NAV_START_MS).write(t_nav);
        core::ptr::addr_of_mut!(NAV_REPORTED).write(false);
        core::ptr::addr_of_mut!(NAV_STAGE_MS).write(t_nav);
    }
    let h = unsafe {
        npk_http_begin(
            method.as_ptr() as i32, method.len() as i32,
            url.as_ptr() as i32, url.len() as i32,
            hdrs.as_ptr() as i32, hdrs.len() as i32,
            body.as_ptr() as i32, body.len() as i32,
            HTML_CAP as i32,
        )
    };
    unsafe {
        core::ptr::addr_of_mut!(NAV_URL).write(Some(url.to_string()));
        core::ptr::addr_of_mut!(NAV_PUSH_HIST).write(push_hist);
        core::ptr::addr_of_mut!(NAV_STAGE).write(NavStage::Doc);
        core::ptr::addr_of_mut!(NAV_JOB).write(h);
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
/// address bar keeps the URL that was ASKED for rather than one derived from
/// a response we never got.
fn nav_fail(url: &str) {
    set_scroll(0);
    bump_content_gen("navigation");
    bump_nav_gen();
    show_error_page(url);
    mark_dirty();
    nav_clear();
}

/// File whatever `Set-Cookie` the response carried.
///
/// Cookies are scoped to where the response CAME from, after redirects —
/// filing them against the URL we asked for would scope a login cookie to the
/// wrong host.
fn file_cookies(asked: &str) {
    let now = unsafe { npk_unix_time() };
    let hp = core::ptr::addr_of_mut!(HDR_BUF) as *mut u8;
    let hn = unsafe { npk_http_response_headers(hp as i32, HDR_CAP as i32) };
    if hn <= 0 {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(hp as *const u8, hn as usize) };
    let Ok(h) = core::str::from_utf8(bytes) else { return };
    let from = fetched_from().unwrap_or_else(|| asked.to_string());
    let before = cookies::count();
    cookies::store(&from, h, now);
    let after = cookies::count();
    if after != before || h.to_ascii_lowercase().contains("set-cookie") {
        let mut m = String::from("[beak] cookies: ");
        push_i64(&mut m, after as i64);
        m.push_str(" held");
        log(&m);
    }
}

/// Collect whichever half of the navigation has finished. Returns true if the
/// chrome needs redrawing — the address changed, or the stop button goes back
/// to being a reload button.
/// Ein Dokument aus dem eigenen Binaerbild an die Stelle setzen, an der sonst
/// die Antwort des Servers steht — und ab da nichts anders machen.
///
/// Der Rest der Kette (Skripte, Zeichnen, Verlauf) darf nicht wissen, woher
/// die Bytes kamen; sonst haette die Pruefseite einen eigenen Pfad und
/// pruefte am Ende diesen statt den echten.
fn deliver_builtin(engine: &Engine, url: &str, html: &str, push_hist: bool) {
    let len = html.len().min(HTML_CAP);
    unsafe {
        let dst = core::ptr::addr_of_mut!(HTML_BUF) as *mut u8;
        core::ptr::copy_nonoverlapping(html.as_ptr(), dst, len);
        core::ptr::addr_of_mut!(HTML_LEN).write(len);
        // Die Seite bringt ihr eigenes `<style>` mit und verlinkt nichts —
        // das CSS der vorigen Seite muss weg, sonst stylt es diese hier.
        core::ptr::addr_of_mut!(CSS_LEN).write(0);
        core::ptr::addr_of_mut!(NAV_START_MS).write(now_ms());
        core::ptr::addr_of_mut!(NAV_REPORTED).write(false);
    }
    set_scroll(0);
    bump_content_gen("navigation");
    bump_nav_gen();
    set_url(url);
    if push_hist {
        hist_push(url);
    }
    nav_finish(engine);
}

fn nav_pump(engine: &Engine) -> bool {
    let h = nav_job();
    if h < 0 {
        return false;
    }
    // 0 = still running. Everything else (done, failed, or a handle the
    // kernel no longer knows) is answered by collecting it.
    if unsafe { npk_http_poll(h) } == 0 {
        return false;
    }
    match unsafe { core::ptr::addr_of!(NAV_STAGE).read() } {
        NavStage::Doc => nav_document_arrived(engine),
        NavStage::Css => nav_stylesheets_arrived(engine),
        NavStage::Js => nav_scripts_arrived(engine),
        NavStage::Mod => nav_modules_arrived(engine),
        NavStage::Sheet => nav_sheets_arrived(engine),
    }
    true
}

fn nav_document_arrived(engine: &Engine) {
    let h = nav_job();
    let asked = nav_asked();
    let dst = core::ptr::addr_of_mut!(HTML_BUF) as *mut u8;
    let n = unsafe { npk_http_take(h, dst as i32, HTML_CAP as i32) };
    log_ms("fetch document", now_ms() - unsafe { core::ptr::addr_of!(NAV_STAGE_MS).read() });
    if n < 0 {
        nav_fail(&asked);
        return;
    }
    // Before anything downstream: the cookies belong to THIS response, and
    // the getters that carry them are overwritten by the next `take`.
    file_cookies(&asked);
    unsafe { core::ptr::addr_of_mut!(HTML_LEN).write(n as usize) };
    // The bytes are not UTF-8 just because we would like them to be, and the
    // stylesheet scan below reads `html_str()`.
    decode_document();
    let len = unsafe { core::ptr::addr_of!(HTML_LEN).read() };
    if len == 0 {
        // Succeeded with nothing in it. The reader gets told, same as for a
        // refusal, and `nav_fail` does the bookkeeping below itself.
        nav_fail(&asked);
        return;
    }
    set_scroll(0);
    bump_content_gen("navigation");
    bump_nav_gen();
    // Relative sub-resources resolve against the URL the document came FROM,
    // not the one we asked for (RFC 3986 §5.1.3). Getting this wrong made
    // every stylesheet and image repeat the document's own redirect.
    let base = fetched_from().unwrap_or(asked);
    set_url(&base);
    if unsafe { core::ptr::addr_of!(NAV_PUSH_HIST).read() } {
        hist_push(url_str());
    }
    nav_begin_stylesheets(engine, &base);
}

/// Start the second round trip: every `<link rel=stylesheet>` of the document
/// that just landed, in ONE batch. They are render-blocking, so this is where
/// overlapping the round trips is worth the most. Bounded by CSS_CAP +
/// MAX_CSS_LINKS.
fn nav_begin_stylesheets(engine: &Engine, base: &str) {
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
        unsafe { core::ptr::addr_of_mut!(CSS_LEN).write(0) };
        nav_finish(engine);
        return;
    }
    let h = begin_batch(&urls, CSS_CAP);
    if h < 0 {
        // No stylesheets is not a failed page — it renders against our UA
        // sheet — so this ends the navigation rather than diagnosing it.
        log("[beak] stylesheet fetch could not start — rendering unstyled");
        unsafe { core::ptr::addr_of_mut!(CSS_LEN).write(0) };
        nav_finish(engine);
        return;
    }
    unsafe {
        core::ptr::addr_of_mut!(NAV_STAGE).write(NavStage::Css);
        core::ptr::addr_of_mut!(NAV_JOB).write(h);
        core::ptr::addr_of_mut!(NAV_CSS_COUNT).write(urls.len());
        core::ptr::addr_of_mut!(NAV_STAGE_MS).write(now_ms());
    }
}

/// How many sheets the batch in flight asked for.
static mut NAV_CSS_COUNT: usize = 0;
/// Die Skripte der Seite in Dokumentreihenfolge, waehrend die externen noch
/// unterwegs sind. `None` heisst: keine offene Skriptrunde.
static mut NAV_SCRIPTS: Option<Vec<PendingScript>> = None;
/// Die JS-Sitzung DIESER Seite.
///
/// Sie muss die Skriptrunde ueberleben: die Behandler, die ein Skript
/// anmeldet, leben in ihr, und ohne sie waere jeder `addEventListener` beim
/// Verlassen der Funktion wieder weg. Eine Navigation wirft sie weg.
static mut JS: Option<beak_engine::js::Session> = None;

fn js_session() -> Option<&'static mut beak_engine::js::Session> {
    unsafe { (*core::ptr::addr_of_mut!(JS)).as_mut() }
}
/// Wie viele externe Adressen das Buendel angefordert hat.
static mut NAV_JS_COUNT: usize = 0;
/// Die Modul-Einstiege der Seite, in Dokumentreihenfolge — das sind die
/// Adressen, die am Ende ausgewertet werden.
static mut NAV_MOD_ENTRIES: Option<Vec<String>> = None;
/// Die Adressen, die in DIESER Runde unterwegs sind, in Bestellreihenfolge.
static mut NAV_MOD_WANT: Option<Vec<String>> = None;
/// Wie viele Runden schon liefen — der Deckel gegen einen Graphen, der sich
/// selbst nachlaedt.
static mut NAV_MOD_ROUNDS: usize = 0;
/// Die Knoten der Stilblaetter, die in DIESER Runde unterwegs sind — in
/// Bestellreihenfolge, damit die Antwort dem `<link>` zugeordnet werden kann.
static mut NAV_SHEET_NODES: Option<Vec<u32>> = None;
static mut NAV_SHEET_ROUNDS: usize = 0;

/// Ein Skript, das auf seinen Text wartet — oder ihn schon hat.
enum PendingScript {
    /// Quelltext, die Kennung (fuer ein Modul: seine Adresse) und ob es ein
    /// Modul ist.
    Ready(String, String, bool),
    /// Der Index in der Bestellung, in der Reihenfolge der Anforderung,
    /// plus die Adresse — ein Fehler ohne Kennung ist keine Auskunft.
    Fetching(usize, String, bool),
}

/// Wie viele externe Skripte eine Seite holen darf.
///
/// Nach Anzahl gedeckelt UND nach Bytes (`SCRIPT_CAP`): eine Seite mit 200
/// Bundles soll nicht 200 Rundreisen ausloesen, und eines mit 50 MB soll den
/// Puffer nicht sprengen. Der Zensus sagt, echte Seiten laden 1 bis 11
/// externe (github am meisten).
const MAX_SCRIPT_URLS: usize = 32;
const SCRIPT_CAP: usize = 8 * 1024 * 1024;
/// Wie gross ein Modulgraph werden darf, und in wie vielen Runden.
///
/// Nach Anzahl UND Runden, weil beide Enden ausufern koennen: eine Seite mit
/// tausend kleinen Modulen und eine Kette, die sich in jeder Runde ein
/// weiteres Glied holt. Gemessen: die Fritzbox-Anmeldeseite braucht 56
/// Adressen in 6 Runden.
const MAX_MODULE_URLS: usize = 256;
const MAX_MODULE_ROUNDS: usize = 24;
/// Wie oft eine Seite nachgeladene Stilblaetter nachlegen darf. Jede Runde
/// ist eine Rundreise; eine Seite, die in jeder Runde ein weiteres anmeldet,
/// haelt den Aufbau sonst offen.
const MAX_SHEET_ROUNDS: usize = 8;

fn nav_stylesheets_arrived(engine: &Engine) {
    let h = nav_job();
    let want = unsafe { core::ptr::addr_of!(NAV_CSS_COUNT).read() };
    // Fetched into a scratch buffer first because the bodies come back
    // concatenated, and they need a separator between them: without one, a
    // sheet not ending in `}` would merge into the next sheet's first rule.
    let mut scratch: Vec<u8> = Vec::with_capacity(CSS_CAP);
    let spans = take_batch(h, scratch.as_mut_ptr(), CSS_CAP, want);
    let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
    unsafe { scratch.set_len(total.min(CSS_CAP)) };

    let dst = core::ptr::addr_of_mut!(CSS_BUF) as *mut u8;
    let mut len = 0usize;
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
    unsafe { core::ptr::addr_of_mut!(CSS_LEN).write(len) };
    log_ms("fetch stylesheets", now_ms() - unsafe { core::ptr::addr_of!(NAV_STAGE_MS).read() });
    nav_finish(engine);
}

/// Document and stylesheets are both in: the page may be drawn, and its
/// images may start arriving.
fn nav_finish(engine: &Engine) {
    decode_css();
    // Erst die Skripte einsammeln. Sind externe dabei, geht die Navigation in
    // eine dritte Stufe und endet erst danach — sonst waere die Seite fertig,
    // bevor ihre Skripte sie gebaut haben.
    if nav_begin_scripts(engine) { return; }
    nav_done();
}

/// Die Seite ist fertig: zeichnen und Bilder holen.
fn nav_done() {
    unsafe { core::ptr::addr_of_mut!(IMAGES_DIRTY).write(true) };
    mark_dirty();
    nav_clear();
}

/// Die Skripte der Seite einsammeln und die externen anfordern.
///
/// Liefert true, wenn eine Rundreise laeuft — dann geht es in `nav_pump`
/// weiter. Sonst sind die Skripte schon gelaufen.
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
            ScriptRef::Inline(t, m) => {
                inline_n += 1;
                // Ein eingebettetes Modul bekommt eine eigene Adresse: sie
                // ist der Schluessel im Lader UND was `import.meta.url`
                // sagt, und relative Angaben loesen sich dagegen auf.
                let label = if m { alloc::format!("{base}#inline{inline_n}") }
                            else { alloc::format!("inline #{inline_n}") };
                list.push(PendingScript::Ready(t, label, m));
            }
            ScriptRef::External(src, m) => {
                if urls.len() >= MAX_SCRIPT_URLS {
                    log(&alloc::format!("[beak] script cap hit: {} external scripts used", MAX_SCRIPT_URLS));
                    continue;
                }
                let u = resolve(&base, &src);
                list.push(PendingScript::Fetching(urls.len(), u.clone(), m));
                urls.push(u);
            }
        }
    }
    if urls.is_empty() {
        return run_scripts(engine, list);
    }
    let h = begin_batch(&urls, SCRIPT_CAP);
    if h < 0 {
        // Nicht anforderbar: die eingebetteten laufen trotzdem. Eine Seite
        // ohne ihre Bundles ist weniger als eine ganze, aber mehr als keine.
        log("[beak] external scripts could not be fetched — running inline only");
        return run_scripts(engine, list);
    }
    unsafe {
        core::ptr::addr_of_mut!(NAV_SCRIPTS).write(Some(list));
        core::ptr::addr_of_mut!(NAV_JS_COUNT).write(urls.len());
        core::ptr::addr_of_mut!(NAV_STAGE).write(NavStage::Js);
        core::ptr::addr_of_mut!(NAV_JOB).write(h);
        core::ptr::addr_of_mut!(NAV_STAGE_MS).write(now_ms());
    }
    true
}

fn nav_scripts_arrived(engine: &Engine) {
    let h = nav_job();
    let want = unsafe { core::ptr::addr_of!(NAV_JS_COUNT).read() };
    let mut list = unsafe { (*core::ptr::addr_of_mut!(NAV_SCRIPTS)).take() }.unwrap_or_default();
    let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
    let spans = take_batch(h, dst, SCRIPT_CAP.min(IMG_FETCH_CAP), want);
    for p in list.iter_mut() {
        let (k, label, is_mod) = match p {
            PendingScript::Fetching(k, l, m) => (*k, core::mem::take(l), *m),
            _ => continue,
        };
        let (off, n) = spans.get(k).copied().unwrap_or((0, 0));
        let text = if n == 0 { String::new() } else {
            let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };
            // Nicht dekodierbar heisst hier: nicht ausfuehren. Ein Skript
            // halb zu lesen ist schlimmer als es zu lassen.
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
        *p = PendingScript::Ready(text, label, is_mod);
    }
    log_ms("fetch scripts", now_ms() - unsafe { core::ptr::addr_of!(NAV_STAGE_MS).read() });
    if !run_scripts(engine, list) { nav_done(); }
}

/// Ein Byte-Offset als `Zeile:Spalte` plus die Umgebung im Quelltext.
///
/// Ein Fehler, der nur `@41822` sagt, kostet auf minifiziertem Fremdcode eine
/// Stunde. Die Zeile selbst wird NICHT ganz gezeigt — minifizierter Code hat
/// Zeilen von 200 KB.
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

/// Die eingebetteten Skripte der Seite ausfuehren und den veraenderten Baum
/// ans Layout weiterreichen.
///
/// Erst HIER, nach den Stilblaettern: ein Skript liest Klassen und Groessen,
/// und ein halb aufgebautes Dokument haette beides falsch. Es laeuft EINMAL je
/// Navigation — nicht bei jedem Bild, das nachkommt.
///
/// Was ein Skript anstellt, bleibt im Sandkasten: die Maschine hat einen
/// Schrittdeckel, eine Aufruftiefe und keinen Zugang zu Host-Funktionen. Sie
/// kann diese Seite verunstalten und sonst nichts.
/// Was die Seite auf `console` geschrieben hat, auf die Serienleitung geben.
///
/// Eine Seite, deren eigene Diagnose ins Leere laeuft, kann man aus der Ferne
/// nicht befragen — und ein Geraetelauf ist immer eine Ferndiagnose. Mit
/// Praefix, damit im Log sichtbar bleibt, wer geredet hat: das sind fremde
/// Bytes, nicht beaks Stimme.
fn drain_console(sess: &mut beak_engine::js::Session) {
    for line in sess.interp.take_console() {
        let mut m = String::from("[seite] ");
        m.push_str(&line);
        log(&m);
    }
}

/// Was die Seite mit `document.cookie = …` gesetzt hat, in den Behaelter —
/// und die Sicht danach neu einreichen.
///
/// Nach JEDEM Einstiegspunkt der Maschine, nicht nur nach dem Laden: ein
/// Klick setzt Kekse genauso wie ein Startskript, und wer nur einmal
/// abholt, verliert alles danach. Die Engine haelt keinen Behaelter — was
/// gilt, entscheidet `cookies`, samt Domain, Pfad und `HttpOnly`.
fn sync_cookies(sess: &mut beak_engine::js::Session) {
    let url = url_str();
    if url.is_empty() {
        return;
    }
    let now = unsafe { npk_unix_time() };
    let sets = sess.interp.take_cookie_sets();
    for decl in &sets {
        cookies::store_from_script(url, decl, now);
    }
    if !sets.is_empty() {
        let mut m = String::from("[beak] cookies: Seite setzte ");
        push_i64(&mut m, sets.len() as i64);
        m.push_str(", ");
        push_i64(&mut m, cookies::count() as i64);
        m.push_str(" held");
        log(&m);
    }
    sess.interp.set_cookies(cookies::script_header_for(url, now));
}

/// Was die Seite am Verlauf verlangt hat — abholen und tun.
///
/// **Die Engine sammelt nur Absichten**, weil sie keinen Verlauf hat und
/// keinen erfinden soll. Hier ist der Ort, an dem daraus etwas wird; und
/// hier wird ihr auch gesagt, wie lang der Verlauf inzwischen ist, damit
/// `history.length` nicht ewig 1 behauptet.
///
/// Gerufen an denselben Stellen wie `sync_cookies` — nach JEDEM
/// Einstiegspunkt, nicht nur nach dem Laden. Ein Klick schreibt Verlauf
/// genauso wie ein Skript beim Start.
fn sync_history(engine: &Engine, sess: &mut beak_engine::js::Session) {
    use beak_engine::js::interp::HistoryOp;
    for op in sess.interp.take_history_ops() {
        match op {
            // `pushState`/`replaceState` NAVIGIEREN NICHT — sie schreiben nur
            // die Adresse um. Genau das ist ihr Sinn: eine Anwendung, die
            // ihre Ansicht wechselt, ohne ein Dokument zu holen.
            HistoryOp::Push { ref url } | HistoryOp::Replace { ref url } => {
                let replace = matches!(op, HistoryOp::Replace { .. });
                if url.is_empty() {
                    continue;
                }
                let abs = resolve(url_str(), url);
                // Nur die eigene Herkunft. Eine Seite darf ihre Adresszeile
                // umschreiben, aber nicht auf eine fremde Herkunft — das
                // waere eine Faelschung, die der Nutzer nicht sieht.
                if origin_of(&abs) != origin_of(url_str()) {
                    log("[beak] history: Adresse fremder Herkunft abgelehnt");
                    continue;
                }
                set_url(&abs);
                if !replace {
                    hist_push(&abs);
                }
            }
            // `go(n)` springt WIRKLICH. Ein Zaehler, der die Absicht
            // notiert und nichts tut, waere schlimmer als kein `history`:
            // die Seite glaubt dann, sie sei zurueckgegangen.
            //
            // Mehr als einen Schritt kann beaks Verlauf nicht am Stueck —
            // also so oft, wie verlangt, und wer am Ende ist, hoert auf.
            HistoryOp::Go(n) => {
                let mut target: Option<String> = None;
                for _ in 0..n.unsigned_abs().min(HIST_MAX as u32) {
                    match if n < 0 { hist_back() } else { hist_forward() } {
                        Some(u) => target = Some(String::from(u)),
                        None => break,
                    }
                }
                if let Some(u) = target {
                    // `push_hist` ist hier FALSCH: sonst waechst der Verlauf
                    // beim Zurueckgehen, und man kaeme nie heraus.
                    nav_begin(engine, "GET", &u, &[], "", false);
                    return;
                }
            }
        }
    }
    let count = unsafe { core::ptr::addr_of!(HIST_COUNT).read() };
    sess.interp.set_history(count.max(1) as f64, beak_engine::js::value::Value::Null);
}

/// Liefert true, wenn eine Modulrunde laeuft — dann ist die Navigation
/// NOCH nicht fertig.
fn run_scripts(engine: &Engine, list: Vec<PendingScript>) -> bool {
    unsafe { core::ptr::addr_of_mut!(SCRIPT_T0).write(now_ms()) };
    let dom = beak_engine::parse(html_str());
    let doc = beak_engine::js::dombind::Doc::from_dom(&dom);
    let mut sess = beak_engine::js::Session::new(SCRIPT_STEPS);
    sess.interp.deadline = Some(script_time_left);
    arm_script_budget();
    sess.interp.set_document(doc);
    // Die Adresse gehoert dem Wirt — und zwar die, aus der das Dokument KAM,
    // nicht die erfragte: eine Weiterleitung aendert Herkunft und damit die
    // Kekse. Ohne diese Zeile stand in `location` `about:blank`, und ein
    // Skript, das seinen Pfad prueft, nahm still den falschen Zweig.
    sess.interp.set_location(url_str());
    // Die Kekse, die dieses Dokument sehen DARF. `HttpOnly` bleibt draussen:
    // die Fahne ist die Gegenmassnahme gegen fremden Code auf der Seite, und
    // seit die Maschine Seitenskripte faehrt, gibt es fremden Code.
    if !url_str().is_empty() {
        let now = unsafe { npk_unix_time() };
        sess.interp.set_cookies(cookies::script_header_for(url_str(), now));
    }
    // Der Kaskadenkontext fuer `getComputedStyle`. Ohne ihn antwortet es aus
    // dem Inline-Stil — eine Teilantwort, die eine Seite laufen laesst, aber
    // die falsche Auskunft gibt. Mit ihm rechnet die Maschine dieselbe
    // Kaskade, die das Layout rechnet, auf demselben Baum und demselben
    // Blatt.
    if let Some((_, _, w, _)) = canvas_rect() {
        let media = beak_engine::css::Media::new(w as f32, query_theme().is_dark());
        let sheet = beak_engine::css::collect_all(&dom, css_str(), media);
        sess.interp.set_style_context(beak_engine::js::interp::StyleCtx {
            sheet: alloc::rc::Rc::new(sheet),
            theme: engine.theme(),
            viewport_w: w as f32,
        });
    }
    // Die Fenstergroesse gehoert dem Wirt. Ohne sie gibt es `innerWidth`
    // nicht, und eine Seite, die ihre schmale Fassung danach waehlt, faellt
    // mit ReferenceError aus, statt sie zu nehmen.
    if let Some((_, _, w, h)) = canvas_rect() {
        // Farbschema MIT einreichen, nicht nur die Groesse: `matchMedia`
        // muss dieselbe Antwort geben wie der Kaskadenlauf, sonst waehlt das
        // Skript eine Fassung, die das Layout nicht malt.
        sess.interp.set_media(w as f64, h as f64, query_theme().is_dark());
    }
    // `Math.random` bekommt eine echte Saat. Ohne sie liefert jede Seite
    // dieselbe Folge — und die Engine erfindet sich absichtlich keine.
    sess.interp.seed_random(now_ms() as u64 ^ 0x9E37_79B9_7F4A_7C15);
    // Und eine echte Uhr. Die Engine hat keine — ohne diese Zeile steht
    // `Date.now()` bei 1970, und jede Seite, die ein Datum ausrechnet,
    // rechnet falsch.
    sess.interp.epoch_ms = unsafe { npk_unix_time() } as f64 * 1000.0;
    let (mut ran, mut failed, mut bytes) = (0usize, 0usize, 0usize);
    // Die Modul-Einstiege, in Dokumentreihenfolge. Sie laufen NACH allen
    // gewoehnlichen Skripten — `type="module"` ist per Spezifikation
    // aufgeschoben, und die Fritzbox verlaesst sich darauf: ihr Modulcode
    // liest `gNbc`, das ein eingebettetes Skript davor setzt.
    let mut entries: Vec<String> = Vec::new();
    for p in &list {
        let (src, label, is_mod) = match p {
            PendingScript::Ready(s, l, m) => (s, l.as_str(), *m),
            PendingScript::Fetching(_, l, _) => {
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
            // Ein Modul ist auch ein Skript — die Datei sagt es nicht, also
            // beides versuchen.
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
        // Ein Skript, das scheitert, darf die naechsten nicht mitnehmen — so
        // macht es ein Browser auch.
        match sess.run(&prog) {
            Ok(()) => ran += 1,
            Err(e) => {
                failed += 1;
                log(&alloc::format!("[beak]   script FAIL {label}: {e}"));
            }
        }
    }
    unsafe {
        core::ptr::addr_of_mut!(JS).write(Some(sess));
        core::ptr::addr_of_mut!(SCRIPT_TALLY).write((ran, failed, bytes));
        core::ptr::addr_of_mut!(NAV_MOD_ENTRIES).write(if entries.is_empty() { None } else { Some(entries) });
        core::ptr::addr_of_mut!(NAV_MOD_ROUNDS).write(0);
    }
    module_pump(engine)
}

/// Was die gewoehnlichen Skripte ergeben haben — muss die Modulrunden
/// ueberleben, weil der Bericht erst danach geschrieben wird.
static mut SCRIPT_TALLY: (usize, usize, usize) = (0, 0, 0);
/// Wann die Skriptrunde begann. NICHT `NAV_STAGE_MS`: das steht nach einer
/// Modulrunde auf deren Beginn, und die gemeldete Zeit waere zu klein.
static mut SCRIPT_T0: i64 = 0;

/// Eine Runde am Modulgraphen: was fehlt noch?
///
/// Liefert true, wenn eine Rundreise laeuft — dann geht es in `nav_pump`
/// weiter. Sonst ist der Graph geschlossen und alles ist ausgewertet.
fn module_pump(engine: &Engine) -> bool {
    let entries = match unsafe { (*core::ptr::addr_of!(NAV_MOD_ENTRIES)).clone() } {
        Some(e) => e,
        None => { finish_scripts(engine); return false }
    };
    let Some(sess) = js_session() else { finish_scripts(engine); return false };
    // Vom Einstieg aus laufen und dabei JEDE Angabe aufloesen — der Lader
    // kennt nur absolute Adressen, das Aufloesen gehoert dem Wirt.
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
    let rounds = unsafe { core::ptr::addr_of!(NAV_MOD_ROUNDS).read() };
    // WELCHER Deckel gerissen ist, gehoert in die Meldung: „nach 5 Runden"
    // klang nach der Rundengrenze, obwohl die bei 24 liegt — gerissen war die
    // Adressgrenze, und das ist eine ganz andere Diagnose.
    let cap = if rounds >= MAX_MODULE_ROUNDS { Some("Runden") }
              else if seen.len() > MAX_MODULE_URLS { Some("Adressen") }
              else { None };
    if !missing.is_empty() && cap.is_none() {
        missing.truncate(MAX_SCRIPT_URLS);
        let h = begin_batch(&missing, SCRIPT_CAP);
        if h >= 0 {
            unsafe {
                core::ptr::addr_of_mut!(NAV_MOD_WANT).write(Some(missing));
                core::ptr::addr_of_mut!(NAV_MOD_ROUNDS).write(rounds + 1);
                core::ptr::addr_of_mut!(NAV_STAGE).write(NavStage::Mod);
                core::ptr::addr_of_mut!(NAV_JOB).write(h);
                core::ptr::addr_of_mut!(NAV_STAGE_MS).write(now_ms());
            }
            return true;
        }
        log("[beak] module graph could not be fetched");
    }
    // Zu gross, zu tief oder fertig: auswerten, was da ist. Ein Modul, das
    // fehlt, meldet sich beim Verknuepfen mit Namen.
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

/// Eine Runde an den Stilblaettern, die ein Skript eingehaengt hat.
///
/// Liefert true, wenn eine Rundreise laeuft. Wie beim Modulgraphen
/// rundenweise: ein Blatt, das ankommt, laesst eine Komponente fertig bauen,
/// und die haengt ihrerseits eins ein.
fn sheet_pump(engine: &Engine) -> bool {
    let Some(sess) = js_session() else { finish_scripts(engine); return false };
    // Erst die Microtasks und Zeitgeber laufen lassen: was gerade fertig
    // geworden ist, meldet seine Blaetter JETZT an.
    for _ in 0..8 { if sess.interp.run_timers() == 0 { break } }
    let want = sess.interp.take_pending_sheets();
    if want.is_empty() { finish_scripts(engine); return false }
    let rounds = unsafe { core::ptr::addr_of!(NAV_SHEET_ROUNDS).read() };
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
    unsafe {
        core::ptr::addr_of_mut!(NAV_SHEET_NODES).write(Some(nodes));
        core::ptr::addr_of_mut!(NAV_SHEET_ROUNDS).write(rounds + 1);
        core::ptr::addr_of_mut!(NAV_STAGE).write(NavStage::Sheet);
        core::ptr::addr_of_mut!(NAV_JOB).write(h);
        core::ptr::addr_of_mut!(NAV_STAGE_MS).write(now_ms());
    }
    true
}

fn nav_sheets_arrived(engine: &Engine) {
    let h = nav_job();
    let nodes = unsafe { (*core::ptr::addr_of_mut!(NAV_SHEET_NODES)).take() }.unwrap_or_default();
    let mut scratch: Vec<u8> = Vec::with_capacity(CSS_CAP);
    let spans = take_batch(h, scratch.as_mut_ptr(), CSS_CAP, nodes.len());
    let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
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
                        now_ms() - unsafe { core::ptr::addr_of!(NAV_STAGE_MS).read() }));
    if ok > 0 {
        decode_css();
        // Die Kaskade muss neu laufen — sonst haengt das Blatt im Puffer und
        // wirkt nicht.
        bump_content_gen("sheet");
        mark_dirty();
    }
    if !sheet_pump(engine) { nav_done(); }
}

/// Ein Stilblatt ANHAENGEN. Spaeter geholt heisst spaeter in der Kaskade, und
/// das ist genau die Reihenfolge, in der es der Browser auch anwendet.
fn css_append(bytes: &[u8]) -> bool {
    let len = unsafe { core::ptr::addr_of!(CSS_LEN).read() };
    if len + bytes.len() + 1 >= CSS_CAP {
        log(&alloc::format!("[beak] CSS buffer full at {len} B — dropped a {} B sheet", bytes.len()));
        return false;
    }
    let dst = core::ptr::addr_of_mut!(CSS_BUF) as *mut u8;
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst.add(len), bytes.len());
        *dst.add(len + bytes.len()) = b'\n';
        core::ptr::addr_of_mut!(CSS_LEN).write(len + bytes.len() + 1);
    }
    true
}

fn nav_modules_arrived(engine: &Engine) {
    let h = nav_job();
    let want = unsafe { (*core::ptr::addr_of_mut!(NAV_MOD_WANT)).take() }.unwrap_or_default();
    let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
    let spans = take_batch(h, dst, SCRIPT_CAP.min(IMG_FETCH_CAP), want.len());
    if let Some(sess) = js_session() {
        for (k, url) in want.iter().enumerate() {
            let (off, n) = spans.get(k).copied().unwrap_or((0, 0));
            if n == 0 { log(&alloc::format!("[beak]   module FAIL {url}: leer")); continue }
            let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };
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

/// Die Einstiege auswerten — in Dokumentreihenfolge, jeder genau einmal.
fn eval_modules() {
    let entries = match unsafe { (*core::ptr::addr_of!(NAV_MOD_ENTRIES)).clone() } {
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
    unsafe {
        let (r, f, b) = core::ptr::addr_of!(SCRIPT_TALLY).read();
        core::ptr::addr_of_mut!(SCRIPT_TALLY).write((r + ok, f + bad, b));
    }
}

/// Zeitgeber, Kekse, Baum und der Bericht — nach ALLEM, was die Seite an
/// Code hat: gewoehnliche Skripte wie Module.
fn finish_scripts(engine: &Engine) {
    let (ran, failed, bytes) = unsafe { core::ptr::addr_of!(SCRIPT_TALLY).read() };
    let t0 = unsafe { core::ptr::addr_of!(SCRIPT_T0).read() };
    let Some(sess) = js_session() else { return };
    // Die Zeitgeber, die waehrend des Ladens angemeldet wurden, einmal
    // laufen lassen — viele Seiten stellen ihre Oberflaeche in einem
    // `setTimeout(…, 0)` fertig.
    let timers = sess.interp.run_timers();
    sync_cookies(sess);
    sync_history(engine, sess);
    drain_console(sess);
    let mut listeners = false;
    if let Some(d) = sess.interp.doc.as_mut() {
        listeners = d.has_listeners;
        // Die Kaesten werden nicht nur fuer Klicks gebraucht, sondern auch
        // fuer `getBoundingClientRect`. Eine Seite, die Skripte FAEHRT, kann
        // danach fragen, auch wenn sie keinen Behandler angemeldet hat.
        engine.set_hit_all(listeners || ran > 0);
        engine.set_scripted_dom(Some(d.to_dom()));
    }
    unsafe { core::ptr::addr_of_mut!(NAV_MOD_ENTRIES).write(None) };
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
    // Ob die Zustellung ueberhaupt scharf ist, gehoert EINMAL je Seite ins
    // Log. Ohne diese Zeile ist "kein Klick kam an" nicht von "die Seite hat
    // keine Behandler" zu unterscheiden — und das ist genau der Unterschied,
    // den man sucht ([[feedback_the_fast_path_must_say_it_ran]]).
    m.push_str(if listeners { ", Ereignisse SCHARF" } else { ", keine Behandler" });
    log(&m);
}

/// Die zwei Richtungen der Formular-Bruecke. Die REGEL steht in der Engine
/// (`dombind::push_control_values` / `pull_control_values`) — hier stehen nur
/// die Ausleihen, damit es die Regel nicht zweimal gibt.
fn push_control_values(page: &Page) {
    let Some(sess) = js_session() else { return };
    let Some(doc) = sess.interp.doc.as_mut() else { return };
    beak_engine::js::dombind::push_control_values(doc, &page.forms, &page.state);
}

fn pull_control_values(page: &mut Page, sess: &beak_engine::js::Session) {
    let Some(doc) = sess.interp.doc.as_ref() else { return };
    beak_engine::js::dombind::pull_control_values(doc, &page.forms, &mut page.state);
}

/// Einen Klick an die Seite zustellen. Liefert true, wenn ein Behandler
/// `preventDefault` gerufen hat — dann unterbleibt, was beak sonst getan
/// haette (einem Link folgen, ein Steuerelement bedienen).
fn dispatch_click(engine: &Engine, page: &mut Page, lay: &Layout, cx: i32, cy: i32) -> bool {
    // Jeder Behandler bekommt sein eigenes Zeitbudget — sonst zahlt der
    // zwanzigste Klick fuer die neunzehn davor.
    arm_script_budget();
    // Was der Benutzer getippt hat, muss der Behandler sehen.
    push_control_values(page);
    let Some(sess) = js_session() else { return false };
    // Die seq-Kette unter dem Zeiger, vom aeussersten zum innersten. Das
    // Layout gibt sie schon aus — dieselbe Liste, aus der `:hover` lebt.
    let chain = lay.element_chain(cx, cy);
    if chain.is_empty() { return false; }
    let Some(doc) = sess.interp.doc.as_ref() else { return false };
    if !doc.has_listeners { return false; }
    let nodes: Vec<u32> = chain.iter().filter_map(|s| doc.by_seq(*s)).collect();
    if nodes.is_empty() { return false; }
    let t0 = now_ms();
    let prevented = matches!(
        beak_engine::js::dombind::dispatch(&mut sess.interp, "click", &nodes), Ok(true));
    let timers = sess.interp.run_timers();
    sync_cookies(sess);
    sync_history(engine, sess);
    // NUR wenn sich etwas geaendert hat. Ein Behandler, der bloss zaehlt,
    // darf keine 130 ms Layout kosten.
    let changed = sess.interp.doc.as_ref().is_some_and(|d| d.dirty);
    if changed {
        if let Some(d) = sess.interp.doc.as_mut() {
            engine.set_scripted_dom(Some(d.to_dom()));
        }
        bump_content_gen("script");
        mark_dirty();
    }
    // Das Formularmodell und die Werte nachziehen, BEVOR ein Absende-Auftrag
    // ausgefuehrt wird — sonst schickt er den Stand von vorher.
    page.sync(engine);
    pull_control_values(page, sess);
    let submits = sess.interp.take_submits();
    drain_console(sess);
    for seq in submits {
        log(&alloc::format!("[beak] script submit: form seq={seq}"));
        if submit_form_seq(engine, page, seq) { return true }
    }
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

/// Was ein Seitenskript an Schritten bekommt.
///
/// Es laeuft im Fenster des Anwenders, nicht in einem Testlaeufer: reisst der
/// Deckel, steht die Seite so da, wie das Skript sie bis dahin gebaut hat —
/// und beak antwortet weiter. Grosszuegiger als im Test (200 000), weil eine
/// echte Startroutine mehr tut als ein Einzeltest.
/// Der Schrittdeckel ist nur noch das Sicherungsnetz gegen einen Lauf, der
/// gar nichts mehr tut. Was eine Seite wirklich begrenzt, ist die ZEIT
/// (`SCRIPT_BUDGET_MS`) — ein Schrittdeckel trifft sonst genauso eine Seite,
/// die viel rechnet, und „viel rechnen" ist kein Fehler.
const SCRIPT_STEPS: u64 = 20_000_000_000;

/// Wie lange ein Skriptlauf oder ein Behandler rechnen darf.
///
/// **Grosszuegig, und mit Grund.** Eine Anmeldung, die ihren Kennwort-Hash
/// selbst rechnet (PBKDF2, zehntausende Runden), braucht in beak Minuten. Das
/// ist keine Endlosschleife, das ist der Preis eines Interpreters, und ihn
/// abzuwuergen hiesse „die Seite ist kaputt" zu melden, wo sie es nicht ist.
/// Der Deckel ist gegen das ANDERE da: `while(true)`.
///
/// **Die Zahl kommt vom GERAET, nicht vom Host.** Host-seitig gemessen
/// kostet die Fritzbox-Anmeldung 65 s — aber das ist NATIVER Code. Am Geraet
/// laeuft beak durch forge, und `tools/beaknative` hat das Verhaeltnis
/// ausgezaehlt: nativ 17,0 ms, forge 66,6 ms, wasmi 350,4 ms, also **3,9x**.
/// Mit einem 120-s-Deckel haette dieselbe Anmeldung, die host-seitig
/// durchlaeuft, am Geraet abgebrochen — und der Bericht haette „script ran
/// too long" gesagt, wo in Wahrheit die Messung am falschen Ziel stand
/// ([[feedback_host_profile_is_not_the_device]]).
const SCRIPT_BUDGET_MS: i64 = 360_000;

/// Ab wann ein Lauf im Log auffaellt. Ein Skript, das Minuten rechnet, ist
/// kein Fehler — aber es ist der Grund, warum nichts passiert, und das
/// gehoert gesagt, statt es aus einem Zeitstempel raten zu lassen.
const SCRIPT_SLOW_MS: i64 = 3_000;

static mut SCRIPT_DEADLINE: i64 = 0;

/// Die Uhr, die die Engine alle 65 536 Schritte fragt.
fn script_time_left() -> bool {
    now_ms() < unsafe { core::ptr::addr_of!(SCRIPT_DEADLINE).read() }
}

/// Die Uhr neu stellen — vor jedem Lauf von Seitencode.
fn arm_script_budget() {
    unsafe { core::ptr::addr_of_mut!(SCRIPT_DEADLINE).write(now_ms() + SCRIPT_BUDGET_MS) };
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
    // The SAME viewport width layout uses: `<picture>`/`srcset` picks its
    // candidate per media query, so fetching at a different width would fetch
    // a URL the page never asks for and leave the real one blank.
    let vw = canvas_rect().map(|(_, _, w, _)| w as u32).unwrap_or(1280);
    let all = beak_engine::image_srcs(html_str(), vw);
    for src in all.iter() {
        if pending.len() >= MAX_IMAGES {
            log(&alloc::format!("[beak] image cap hit: {} of {} sources fetched", MAX_IMAGES, all.len()));
            break;
        }
        if !pending.iter().any(|s| s == src) {
            pending.push(src.clone());
        }
    }
    // Serve what the last pages already decoded, BEFORE the first layout.
    // That is where it pays twice: no request, no decode — and the box is
    // DEFINITE on the very first layout instead of being guessed and moving
    // the page a second later.
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
/// One batch in flight at a time, NOT a whole page: a batch is answered in one
/// go, so asking for everything at once would put the page's whole image
/// traffic between two repaints. Small batches let the reader scroll through a
/// loading page.
///
/// The last layout's `guessed_image_srcs` lists the `src`s whose box it had to
/// guess. Only if one of THOSE arrives does the page move and a re-layout pay
/// for itself; everything else is a repaint. That is ~15 ms instead of ~145 ms
/// of engine work per batch on a real article — and on the device, the
/// difference between a page that scrolls while it loads and one that freezes
/// for seconds at a time.
///
/// `band` is the visible document band `(scroll_y, scroll_y + viewport_h)`.
/// A repaint is the WHOLE viewport, so an image below the fold is paid for in
/// full and shows nothing — see `Layout::images_in_band`.
fn pump_images(
    engine: &mut Engine,
    pending: &mut Vec<String>,
    layout: Option<&Layout>,
    band: (i32, i32),
) {
    let h = img_job();
    if h >= 0 {
        if unsafe { npk_http_poll(h) } == 0 {
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
    // Layout-affecting first. An image whose box was GUESSED moves the page
    // when it lands, and that costs a FULL re-layout wherever it sits —
    // measured on the device: 1110-1710 ms on an article, against ~540 ms for
    // the whole page's image traffic. Fetching it in the FIRST batch pays that
    // once, immediately, instead of after two repaints the re-layout then
    // throws away. On de.wikipedia/Stansstad exactly ONE `<img>` of 17 is such
    // a box (a MediaWiki timeline, no width/height); the Hauptseite has none,
    // which is why only the article ever showed the jump.
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
    unsafe {
        core::ptr::addr_of_mut!(IMG_JOB_SRCS)
            .write(Some(srcs.into_iter().zip(urls).collect()));
        core::ptr::addr_of_mut!(IMG_JOB).write(h);
    }
}

fn images_arrived(
    engine: &mut Engine,
    handle: i32,
    layout: Option<&Layout>,
    band: (i32, i32),
) {
    let want: Vec<(String, String)> =
        unsafe { (*core::ptr::addr_of_mut!(IMG_JOB_SRCS)).take() }.unwrap_or_default();
    unsafe { core::ptr::addr_of_mut!(IMG_JOB).write(-1) };
    let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
    let spans = take_batch(handle, dst, IMG_FETCH_CAP, want.len());
    let mut arrived: Vec<&str> = Vec::new();
    let mut moved = false;
    for ((src, url), (off, n)) in want.iter().zip(spans) {
        if n == 0 {
            continue; // failed or did not fit → keeps its placeholder
        }
        let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };
        // Decode now, drop the compressed bytes — and keep the pixels under
        // their url so the next navigation to this page needs neither.
        if !engine.add_image_cached(src, url, bytes) {
            // Undecodable, or past the page's pixel budget. Either way the
            // box keeps its placeholder, and a picture that silently does not
            // appear is the kind of bug that gets blamed on layout for weeks.
            log(&alloc::format!("[beak] image dropped ({} B): undecodable or over budget — {}",
                n, src));
            continue;
        }
        arrived.push(src.as_str());
        if layout.is_some_and(|l| l.guessed_image_srcs.iter().any(|g| g == src)) {
            moved = true;
        }
    }
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
    // Pure repaint. Ask first whether it would show anything: measured on the
    // Hauptseite, ONE navigation paid eight full-viewport repaints (~50 ms
    // each) for image batches, and the page is 3421 px tall against a ~1000 px
    // viewport — most of those pictures were below the fold and could not
    // change a pixel. Scrolling marks the page dirty on its own, so nothing
    // is lost; it is drawn the moment it can be seen.
    match layout {
        Some(l) if !l.images_in_band(&arrived, band.0, band.1) => {}
        _ => mark_dirty(),
    }
}

fn images_dirty() -> bool {
    unsafe { core::ptr::addr_of!(IMAGES_DIRTY).read() }
}

/// Ask for the CSS images (`background-image`/`mask-image`) the last layout
/// wanted, and take delivery of the last batch — one batch in flight, like
/// `<img>`.
///
/// Kept apart from `pump_images` for one reason that matters: a CSS image can
/// never move a box, so an arriving one is ALWAYS just a repaint — there is no
/// `guessed` case and no `bump_content_gen`. The engine already resolved every
/// `data:` URI itself, so this list is only what genuinely needs the network.
///
/// The URL is resolved against the DOCUMENT, not the stylesheet that declared
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
        if unsafe { npk_http_poll(h) } == 0 {
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
    unsafe {
        core::ptr::addr_of_mut!(CSSIMG_JOB_KEYS).write(Some(keys));
        core::ptr::addr_of_mut!(CSSIMG_JOB).write(h);
    }
}

fn css_images_arrived(
    engine: &Engine,
    handle: i32,
    layout: Option<&Layout>,
    band: (i32, i32),
) {
    let want: Vec<(u64, String)> =
        unsafe { (*core::ptr::addr_of_mut!(CSSIMG_JOB_KEYS)).take() }.unwrap_or_default();
    unsafe { core::ptr::addr_of_mut!(CSSIMG_JOB).write(-1) };
    let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
    let spans = take_batch(handle, dst, IMG_FETCH_CAP, want.len());
    let mut arrived: Vec<u64> = Vec::new();
    for ((key, url), (off, n)) in want.iter().zip(spans) {
        if n == 0 {
            continue; // failed or did not fit → the box stays undecorated
        }
        let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };
        if engine.add_css_image_cached(*key, url, bytes) {
            arrived.push(*key);
        } else {
            log(&alloc::format!("[beak] background dropped ({} B): undecodable or over budget — {}",
                n, url));
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

/// The `<img>` batch on the wire and the `(src, resolved url)` pairs it was
/// asked for, so an arriving body can be filed under the src the page named
/// it by. -1 / None when nothing is in flight.
static mut IMG_JOB: i32 = -1;
static mut IMG_JOB_SRCS: Option<Vec<(String, String)>> = None;
/// The same for backgrounds, keyed the way the layout names them.
static mut CSSIMG_JOB: i32 = -1;
static mut CSSIMG_JOB_KEYS: Option<Vec<(u64, String)>> = None;

fn img_job() -> i32 {
    unsafe { core::ptr::addr_of!(IMG_JOB).read() }
}
fn cssimg_job() -> i32 {
    unsafe { core::ptr::addr_of!(CSSIMG_JOB).read() }
}

/// Drop the sub-resource batches of a page that is being replaced. A browser
/// that keeps fetching the pictures of the page you just left spends the
/// network on nothing — and with one kernel fetch queue behind them, it also
/// makes the new document wait its turn.
fn subresources_cancel() {
    unsafe {
        let p = core::ptr::addr_of_mut!(IMG_JOB);
        if p.read() >= 0 {
            npk_http_cancel(p.read());
            p.write(-1);
        }
        core::ptr::addr_of_mut!(IMG_JOB_SRCS).write(None);
        let p = core::ptr::addr_of_mut!(CSSIMG_JOB);
        if p.read() >= 0 {
            npk_http_cancel(p.read());
            p.write(-1);
        }
        core::ptr::addr_of_mut!(CSSIMG_JOB_KEYS).write(None);
    }
}

/// Set the address + start fetching, WITHOUT touching history (reload,
/// back/forward — those addresses are already in it).
///
/// A failure is not silent: `nav_fail` puts a diagnostic page in the document
/// and logs the reason, so there is nothing to add here.
fn fetch_url(engine: &Engine, url: &str) {
    set_url(url);
    nav_begin(engine, "GET", url, &[], "", false);
}

/// The same, but the address we LAND on joins the history — a click, a typed
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
    let t = typed.trim();
    if t.is_empty() {
        return;
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
    nav_goto(engine, &abs);
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

/// Submit a form. GET puts the data in the query string, POST in the request
/// body — the same encoding either way (HTML §4.10.21.3).
/// Ein Formular, das die SEITE abschicken will (`form.submit()`).
fn submit_form_seq(engine: &Engine, page: &Page, form_seq: u32) -> bool {
    match forms::submit_form(&page.forms, &page.state, form_seq) {
        Some(s) => { send_submission(engine, s); true }
        None => {
            log("[beak] script submit: dieses Formular kennt beak nicht");
            false
        }
    }
}

fn submit_form(engine: &Engine, page: &mut Page, activated: Option<u32>) -> bool {
    // **Erst das `submit`-Ereignis.** Eine Seite rechnet in ihrem Behandler
    // aus, was sie mitschickt — ein Kennwort-Hash, ein Zeitstempel, ein
    // Token — und darf abbrechen. Ohne diesen Schritt schickte beak das
    // Formular so ab, wie es im Baum stand: die berechneten Felder leer, und
    // die Gegenseite antwortet mit „falsches Kennwort".
    //
    // Nur auf dem BENUTZERweg. `form.submit()` aus einem Skript feuert laut
    // Spezifikation kein `submit` — sonst liefe der Behandler der Seite ein
    // zweites Mal.
    let form_seq = activated.or(page.state.focus)
        .and_then(|s| page.forms.get(s)?.form)
        .and_then(|f| page.forms.forms.get(f).map(|d| d.seq));
    if let Some(fs) = form_seq {
        arm_script_budget();
        if let Some(sess) = js_session() {
            if beak_engine::js::dombind::dispatch_seq(&mut sess.interp, "submit", fs) {
                // Abgebrochen. Der Behandler hat oft trotzdem etwas vor —
                // ein `setTimeout`, ein Versprechen — also laufen lassen und
                // den Baum nachziehen.
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
            // Nicht abgefangen — aber der Behandler kann Felder gefuellt
            // haben, und die gehoeren in die Eingabe.
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
    }
    let sub = match forms::submit(&page.forms, &page.state, activated) {
        Some(s) => s,
        // Silence here reads as "the button is dead". It is not the same
        // thing as a failed request, and the difference is the whole
        // diagnosis: a button whose form we never resolved (nested outside
        // it, or owned by a `form=` attribute we do not read) versus a form
        // that submitted and came back wrong.
        None => {
            log("[beak] submit: kein zugehoeriges Formular");
            return false;
        }
    };
    send_submission(engine, sub);
    true
}

/// Eine fertige Eingabe abschicken — GET haengt sie an die Adresse, POST in
/// den Rumpf. Eine Fassung fuer beide Wege dorthin (Knopf und `submit()`).
fn send_submission(engine: &Engine, sub: forms::Submission) {
    // An empty action targets the current document; either way the form data
    // REPLACES the action's query string (HTML §4.10.21.3 "mutate action URL").
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
/// Eine Adresse gegen die der Seite aufloesen.
///
/// **Die Aufloesung der ENGINE, nicht eine zweite.** Der Wirt hatte seine
/// eigene, und sie hat `.` und `..` stehen lassen (RFC 3986 §5.2.4 fehlte
/// ganz). Solange nur Bilder und Links daran hingen, fiel das nicht auf: die
/// Server liefern `/js/../js/x.css` klaglos aus. Beim Modulgraphen ist eine
/// Adresse aber ein SCHLUESSEL — `./jsl.js` aus `/js/./jsl.js` wurde
/// `/js/././jsl.js`, jede Ebene hing ein weiteres Segment an, und derselbe
/// Modul lag am Ende unter sechs Namen im Graphen. Am Geraet: 106 geladen,
/// 179 offen, Deckel gerissen, nichts gelaufen.
///
/// `js::url::resolve` konnte das die ganze Zeit ([[url::norm]]). Zwei
/// Umsetzungen derselben Regel sind eine wartende zweite Semantik, und diese
/// hier ist die falsche gewesen — also gibt es sie nicht mehr.
fn resolve(base: &str, href: &str) -> String {
    use beak_engine::js::url;
    let href = href.trim();
    let Some(b) = url::parse_abs(base) else {
        // Ohne brauchbare Grundlage bleibt nur die Angabe selbst.
        return href.to_string();
    };
    url::resolve(href, &b).href()
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

fn maybe_repaint(engine: &Engine, cache: &mut Option<(Layout, i32, i32, u32)>, buf: &mut Vec<u8>, state: &FormState) {
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

    // (Re)lay out only when the content or the viewport changed — NOT on every
    // scroll. Reusing the cached layout for scroll is what keeps scrolling
    // smooth. The HEIGHT counts only when the page's geometry actually depends
    // on it — a `vh` length that won the cascade, a cap that really clamps, an
    // out-of-flow box anchored to the viewport's bottom edge. `html, body
    // { height: 100% }` is on nearly every site and moves nothing, so it must
    // not count: the dock sliding this window up a few pixels was costing a
    // full re-layout (~6.4 s on a big article) for a picture that could not
    // change.
    let cur_gen = content_gen();
    let need_layout = match cache.as_ref() {
        None => true,
        Some((lay, cw, ch, cg)) => {
            *cw != w || *cg != cur_gen || (*ch != h && lay.viewport_h_used)
        }
    };
    if need_layout {
        *cache = Some((do_layout(engine, w as u32, state), w, h, cur_gen));
        // Die Kaesten neu einsammeln — nur hier, nicht je Bild.
        let boxes = cache.as_ref().unwrap().0.element_rects();
        unsafe { core::ptr::addr_of_mut!(GEOM).write(Some(alloc::rc::Rc::new(boxes))) };
    }
    let layout = &cache.as_ref().unwrap().0;

    let max_scroll = (layout.height as i32 - h).max(0);
    let sy = scroll_y().clamp(0, max_scroll);
    set_scroll(sy);
    // Geometrie und Rollstand an die Maschine reichen. Der Rollstand geht bei
    // jedem Bild mit, weil er sich ohne Layout aendert; die Kaesten sind ein
    // `Rc` und kosten dabei nichts.
    // `ptr::read` waere hier ein Fehler: es kopiert das `Rc` BITWEISE, ohne
    // den Zaehler hochzusetzen — beim naechsten Fallenlassen ein Double-Free.
    // `clone()` ist das, was gemeint ist.
    let geom = unsafe { (*core::ptr::addr_of!(GEOM)).clone() };
    if let (Some(sess), Some(g)) = (js_session(), geom) {
        sess.interp.set_geometry(beak_engine::js::interp::Geometry {
            boxes: g, scroll: (0, sy),
        });
    }

    // Reuse a persistent paint buffer across frames — `engine.paint` fills every
    // pixel (background first), so no re-zeroing is needed. A fresh
    // `vec![0; w*h*4]` per frame was a ~5 MB alloc+zero+free on EVERY scroll
    // repaint (heap churn + latency).
    let need = (w as usize) * (h as usize) * 4;
    let resized = buf.len() != need;
    if resized {
        buf.resize(need, 0);
    }

    // Scrolling does not change the page; it moves it. When nothing else asked
    // for a repaint, shift the pixels that merely moved and draw only the band
    // that came into view — 1902x1000 is 7,6 MB of fill, ~60-80 ms on the
    // device, and a scroll exposes a few dozen rows of it.
    //
    // The inspect overlay is drawn OVER the frame rather than being part of the
    // display list, so a blit would smear it; that mode takes the full path.
    let dy = sy - unsafe { core::ptr::addr_of!(LAST_SY).read() };
    let full = unsafe { core::ptr::addr_of!(NEED_FULL).read() }
        || need_layout
        || resized
        || inspect_mode()
        || dy.abs() >= h;
    // A scroll that the clamp swallowed — at the top or the bottom of the page
    // the offset does not move, so the buffer already holds this exact frame.
    // Repainting it was 60-80 ms for a picture that cannot differ, and holding
    // the wheel at the foot of an article does it every turn.
    if dy == 0 && !full {
        unsafe {
            core::ptr::addr_of_mut!(DIRTY).write(false);
        }
        return;
    }
    let t_paint = now_ms();
    if !full {
        let stride = w as usize * 4;
        let rows = h as usize;
        let moved = dy.unsigned_abs() as usize;
        if dy > 0 {
            // Scrolled down: the picture moves UP, the new band is at the foot.
            buf.copy_within(moved * stride..rows * stride, 0);
            engine.paint_band(layout, w as u32, h as u32, sy, buf,
                              (rows - moved) as u32, rows as u32);
        } else {
            // Scrolled up: the picture moves DOWN, the new band is at the head.
            buf.copy_within(0..(rows - moved) * stride, moved * stride);
            engine.paint_band(layout, w as u32, h as u32, sy, buf, 0, moved as u32);
        }
    } else {
        engine.paint(layout, w as u32, h as u32, sy, buf);
    }
    // Inspect overlay: outline the selected element box (document → screen).
    if inspect_mode() {
        if let Some((bx, by, bw, bh)) = selected_rect() {
            stroke_rect_bgra(buf, w, h, bx, by - sy, bw, bh, [0, 0, 255]);
        }
    }
    let t_commit = now_ms();
    unsafe { npk_canvas_commit(CANVAS_ID, buf.as_ptr() as i32, buf.len() as i32, w, h) };
    // Say WHICH path ran. A fast path that never says so looks exactly like one
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
    // Not while one is still in the air: the OLD page keeps repainting for
    // scrolls and hovers during a load now, and reporting one of those would
    // credit the new navigation with a picture of the previous page.
    unsafe {
        if !core::ptr::addr_of!(NAV_REPORTED).read() && !nav_busy() {
            core::ptr::addr_of_mut!(NAV_REPORTED).write(true);
            log_ms("=== navigation -> first paint", now_ms() - core::ptr::addr_of!(NAV_START_MS).read());
        }
    }

    unsafe {
        core::ptr::addr_of_mut!(LAST_W).write(w);
        core::ptr::addr_of_mut!(LAST_H).write(h);
        core::ptr::addr_of_mut!(LAST_SY).write(sy);
        core::ptr::addr_of_mut!(NEED_FULL).write(false);
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
    // scheme belongs in the field, not in the URL text (docs/spec/UI_REFRESH.md §5).
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

/// Toolbar chrome sizing (docs/spec/UI_REFRESH.md §5).
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
fn edit_key(engine: &Engine, page: &mut Page, key: KeyCode) -> bool {
    let (seq, kind, mut value) = match page.focused() {
        Some((c, v)) => (c.seq, c.kind, v.to_string()),
        None => return false,
    };
    if !kind.is_text() {
        // Space / Enter activate a button or toggle a box, like a browser.
        return match key {
            KeyCode::Enter | KeyCode::Char(b' ') => {
                activate(engine, page, seq);
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
            submit_form(engine, page, activated);
            return true;
        }
        _ => return false,
    }
    page.state.set_value(seq, value);
    page.state.caret = caret;
    true
}

/// Click / keyboard activation of a control: submit, toggle, or take focus.
fn activate(engine: &Engine, page: &mut Page, seq: u32) {
    let kind = match page.forms.get(seq) {
        Some(c) => c.kind,
        None => return,
    };
    match kind {
        ControlKind::Submit => {
            page.state.focus = Some(seq);
            submit_form(engine, page, Some(seq));
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
///
/// A navigation started INSIDE the page — submitting a form, following a
/// link — changes the address without anyone touching the address bar, and
/// every one of those paths used to return `false` here. The result was a
/// browser that had loaded the new page but still displayed the old URL.
/// Rather than remember to flag each path, compare the navigation counter
/// that a real page load bumps: a path added later cannot forget it.
///
/// A navigation no longer COMPLETES in here — it is started and picked up by
/// `nav_pump`, which reports its own redraw — so this guard now only catches
/// a path that bumps the counter without waiting for a network.
/// Ein Ereignis, das nur den Zustand EINES Steuerelements aendert: erst neu
/// malen versuchen, und nur wenn das nicht geht, die Seite auslegen.
///
/// Der Unterschied ist nicht klein. Ein volles Auslegen kostet auf Wikipedia
/// 280 ms, ein Neumalen einen Bruchteil einer Millisekunde — und bis 0.71.0
/// ging JEDER Tastendruck in einem Feld den teuren Weg. Wer hier eine neue
/// Ursache einhaengt, prueft zuerst, ob sie wirklich nur einen Kasten
/// betrifft; `repaint_controls` sagt selbst nein, wenn nicht.
fn restate_control(engine: &Engine, cache: &mut Option<(Layout, i32, i32, u32)>,
                   state: &FormState, why: &str) {
    let done = cache.as_mut().is_some_and(|(lay, ..)| engine.repaint_controls(lay, state));
    if !done {
        // EINMAL je Seite sagen, WARUM ausgelegt wird. Ohne diese Zeile sieht
        // ein Schnellweg, der nie laeuft, genauso aus wie einer, der nie
        // gebraucht wurde — und genau so ist er drei Versionen lang tot
        // gewesen ([[feedback_the_fast_path_must_say_it_ran]]).
        say_ctl_bail_once(engine.repaint_bail());
        bump_content_gen(why);
    }
    mark_dirty();
}

static mut CTL_BAIL_SAID: bool = false;
fn say_ctl_bail_once(why: &str) {
    if unsafe { core::ptr::addr_of!(CTL_BAIL_SAID).read() } || why.is_empty() {
        return;
    }
    unsafe { core::ptr::addr_of_mut!(CTL_BAIL_SAID).write(true) };
    log(&alloc::format!("[beak] Steuerelement neu malen geht nicht: {why}"));
}

fn handle(engine: &Engine, ev: Event, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) -> bool {
    let nav = nav_gen();
    let chrome = handle_event(engine, ev, cache, page);
    chrome || nav_gen() != nav
}

fn handle_event(engine: &Engine, ev: Event, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) -> bool {
    match ev {
        // Keep URL_BUF synced with the address-bar edit buffer.
        Event::InputChange { value } => {
            set_url(&value);
            // Typing in the address bar means the compositor moved keyboard
            // focus there — drop the page control's focus so only one caret
            // blinks and Enter goes to the right place.
            if page.state.focus.take().is_some() {
                restate_control(engine, cache, &page.state, "addressbar-focus");
            }
            false
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
                let t = url_str().to_string();
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
                    // Der Zwischenspeicher muss zum aktuellen Stand passen —
                    // und wenn nicht, wird das Layout EINGELEGT statt
                    // weggeworfen.
                    //
                    // Vorher stand hier eine Rechnung fuer genau einen
                    // Treffertest, die der naechste Anstrich sofort noch einmal
                    // machte. Der Kommentar nannte das „rare — only if a click
                    // races a resize", aber die Bedingung ist `content_gen`,
                    // und das steigt bei JEDEM Hover. Wer den Zeiger auf etwas
                    // bewegt, um es anzuklicken, loest also erst ein
                    // Neuauslegen aus und klickt dann in den veralteten Stand:
                    // im Geraetelog zwei volle Layouts hintereinander, ohne
                    // eine Zeile dazwischen.
                    let stale = !matches!(cache.as_ref(),
                        Some((_, cw, ch, cg)) if *cw == w && *ch == h && *cg == content_gen());
                    if stale {
                        *cache = Some((do_layout(engine, w as u32, &page.state), w, h, content_gen()));
                    }
                    // Alles, was das Layout beantworten kann, VOR der ersten
                    // Aenderung am Zwischenspeicher holen — danach ist er
                    // veraenderlich geliehen und `lay` gaebe es nicht mehr.
                    let (dispatched, inspect_sel, ctl_seq, href, toggle) = {
                        let lay = &cache.as_ref().unwrap().0;
                        // Die Seite bekommt den Klick ZUERST. Ruft ein Behandler
                        // `preventDefault`, ist der Klick verbraucht — sonst
                        // wuerde beak zusaetzlich dem Link folgen, den die Seite
                        // gerade abgefangen hat.
                        let dispatched = dispatch_click(engine, page, lay, cx, cy);
                        (
                            dispatched,
                            // Nur im Inspect-Modus: der Test laeuft ueber ALLE
                            // Kaesten, und ein Klick auf einer grossen Seite
                            // soll dafuer nicht zahlen, wenn niemand hinschaut.
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
                    if let Some(href) = href {
                        follow(engine, &href);
                        return true;
                    }
                    // A `<summary>` opens/closes its section. It comes AFTER
                    // the control and the link: a link inside a summary
                    // navigates, which is what a browser does too.
                    if let Some(seq) = toggle {
                        if engine.toggle_details(seq) {
                            bump_content_gen("details-toggle");
                            mark_dirty();
                        }
                        return true;
                    }
                }
            }
            false
        }
        // `:hover`. A series of ever-cheaper ways to answer "nothing to do":
        // no hover rules on the page at all, then no usable cached layout,
        // then the same element as last time. What is left is answered by
        // repainting the display list in place where that is provably enough
        // — measured on Wikipedia at 0.16 ms against 24 ms for a layout — and
        // only otherwise by laying the page out again.
        Event::MouseMove { x, y } => {
            if !engine.page_has_hover() {
                return false;
            }
            let Some((rx, ry, w, h)) = canvas_rect() else {
                return false;
            };
            // Leaving the canvas has to CLEAR the hover, or whatever the pointer
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
        Event::Open(s) => {
            go(engine, &s);
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

// ── The heap GROWS; it is not guessed ───────────────────────────────────────

/// Ported from talc's own `WasmGrowAndExtend` (talc 5.0.4, `src/wasm.rs`) with
/// exactly one change: the growth STEP.
///
/// Talc grows by just enough for the allocation that failed. That is right
/// where `memory.grow` is cheap. Under wasmi it is not — linear memory is one
/// contiguous buffer, so every grow copies all of it, and growing a page at a
/// time up to a 60 MB working set would copy tens of gigabytes. Doubling makes
/// the number of grows logarithmic and the total copying linear in the final
/// size, at the cost of holding up to twice the peak.
///
/// What it replaces: `static mut HEAP: [u8; 128 MB]` — a ceiling nobody
/// measured. And it was not free headroom: wasmi allocates the whole linear
/// memory eagerly, so that array cost 128 MB on every start, while a page that
/// wanted more died against it anyway.
#[derive(Debug)]
struct GrowingHeap {
    /// End of the arena talc last received, so a contiguous grow can EXTEND it
    /// instead of starting a second heap. Zero means "nothing handed over yet".
    ///
    /// An address and not a `NonNull`, which is what talc's own source stores:
    /// `NonNull` is not `Send`, and this allocator sits behind a mutex on
    /// purpose (see the note where it is declared) rather than behind talc's
    /// single-threaded cell.
    end: usize,
}

impl GrowingHeap {
    const fn new() -> Self {
        GrowingHeap { end: 0 }
    }
}

const WASM_PAGE: usize = 64 * 1024;

// SAFETY: `acquire` hands talc only memory that `memory.grow` just returned —
// freshly mapped pages past the previous end of linear memory, which nothing
// else can reach. It allocates nothing itself. That is talc's own
// `WasmGrowAndExtend` contract, kept.
unsafe impl talc::source::Source for GrowingHeap {
    fn acquire<B: talc::base::binning::Binning>(
        talc: &mut talc::base::Talc<Self, B>,
        layout: core::alloc::Layout,
    ) -> Result<(), ()> {
        // Over-estimate deliberately: talc warns that UNDER-sizing here loops
        // forever, handing over heaps that can never fit the allocation.
        let need = layout.size() + layout.align() + 4 * WASM_PAGE;
        let need_pages = need.div_ceil(WASM_PAGE);
        let have_pages = core::arch::wasm32::memory_size::<0>();
        let delta = need_pages.max(have_pages.max(1));

        let prev_end = core::arch::wasm32::memory_grow::<0>(delta);
        if prev_end == usize::MAX {
            return Err(()); // the host said no — the machine really is full
        }
        let base = (prev_end * WASM_PAGE) as *mut u8;
        let size = delta * WASM_PAGE;

        let old_end = core::mem::replace(&mut talc.source.end, 0);
        if old_end == base as usize {
            // SAFETY: contiguous with the arena we handed over last time, and
            // `old_end` came from talc itself, so it is non-null.
            let new_end = unsafe {
                talc.extend(
                    core::ptr::NonNull::new_unchecked(base),
                    base.wrapping_add(size),
                )
            };
            talc.source.end = new_end.as_ptr() as usize;
            return Ok(());
        }
        // SAFETY: fresh pages, owned by nothing else.
        talc.source.end = unsafe { talc.claim(base, size) }.map_or(0, |e| e.as_ptr() as usize);
        Ok(())
    }
}

// `TalcLock` (mutex-guarded), NOT talc's `WasmArenaTalc`/`TalcSyncCell`. The
// cell variants are only sound on single-threaded WebAssembly and enforce that
// with a target check, not the type system — so the day beak gets workers, or
// wasmi turns on the threads proposal, they would go quietly unsound. The
// uncontended spin lock costs a few instructions; that is the cheaper mistake.
#[global_allocator]
static ALLOCATOR: TalcLock<spin::Mutex<()>, GrowingHeap> = TalcLock::new(GrowingHeap::new());

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

    // Which build is actually running. Without this a serial trace cannot say
    // whether a measurement belongs to the version that was just installed —
    // and a perf number from the wrong build is worse than no number.
    log(concat!("[beak] version ", env!("CARGO_PKG_VERSION")));

    log("[beak] parsing font…");
    let mut engine = Engine::new();
    // Lend the engine our tick source so it can report the per-phase split.
    engine.set_clock(|| unsafe { npk_ticks() } as u64);
    engine.set_theme(query_theme());
    log("[beak] engine ready");

    // Engine is up — fetch the launch URL now (if we were opened with one).
    if arg_len > 0 {
        let u = url_str().to_string();
        go(&engine, &u);
    }

    // Cached layout: (Layout, width it was laid out at, content generation).
    let mut cache: Option<(Layout, i32, i32, u32)> = None;
    // The page's forms + the user's edits, rebuilt on every navigation.
    let mut page = Page::new();
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
                    if handle(&engine, ev, &mut cache, &mut page) {
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
        // Take delivery of whatever the kernel finished while we were
        // painting: the document, or its stylesheets. THIS is where a
        // navigation completes now — no path through `handle` waits for one.
        if nav_pump(&engine) {
            chrome = true;
        }
        // …and only then re-parse the document's forms, so the page that just
        // arrived is laid out against its OWN controls rather than the
        // previous page's. Cheap when nothing navigated.
        page.sync(&engine);
        // Und was die Seite selbst gesetzt hat, uebernehmen: nach einem Umbau
        // traegt der Baum die Wahrheit, und die `seq` sind neu vergeben. Die
        // Sitzung wird DURCHGEREICHT, nicht neu geholt — zweimal `js_session`
        // waeren zwei veraenderliche Ausleihen auf dasselbe Feld.
        if let Some(s) = js_session() { pull_control_values(&mut page, s); }
        // Ein Formular, das die Seite schon beim Laden abschicken will, darf
        // nicht bis zum naechsten Klick liegenbleiben.
        let pending = js_session().map(|s| s.interp.take_submits()).unwrap_or_default();
        for seq in pending {
            log(&alloc::format!("[beak] script submit: form seq={seq}"));
            if submit_form_seq(&engine, &page, seq) { break }
        }
        if chrome {
            render_chrome();
        }
        if !had_event {
            // No theme watch here any more: the PAGE palette no longer
            // follows the desktop (see `query_theme`), so a light/dark switch
            // changes the chrome and nothing about the document. Re-laying the
            // page out for it would cost a full layout — over five seconds on
            // the device — for a picture that cannot change.
        }
        // A fresh page: drop the old page's decoded images and note which ones
        // it wants (fetching them happens after the repaint, one batch a turn).
        //
        // This has to sit AFTER `nav_pump` and BEFORE the repaint. A page is
        // completed by `nav_pump`, so from the top of the loop this would
        // always be one turn late: the page was laid out once against the
        // PREVIOUS page's images, and clearing them a turn later invalidated
        // that layout and laid it out again. Two full layouts per navigation,
        // and on the device a layout is over five seconds.
        if images_dirty() {
            pending_imgs = begin_images(&mut engine);
            engine.css_images_begin();
            pending_css_imgs.clear();
            css_asked.clear();
        }
        maybe_repaint(&engine, &mut cache, &mut paint_buf, &page.state);
        // The visible document band, read AFTER the repaint clamped the scroll
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
        // between, because a batch is small. The layout goes in whole rather
        // than a cloned `guessed_image_srcs`: it answers both questions this
        // needs (did a guessed box land, and is the picture even on screen),
        // and the clone happened every turn of the loop.
        let layout = cache.as_ref().map(|(l, _, _, _): &(Layout, i32, i32, u32)| l);
        pump_images(&mut engine, &mut pending_imgs, layout, band);
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
        let mut css_adopted: Vec<u64> = Vec::new();
        if let Some((l, _, _, _)) = cache.as_ref() {
            for (k, u) in &l.css_image_srcs {
                if css_asked.contains(k) {
                    continue;
                }
                css_asked.push(*k);
                // Same question as for `<img>`, one layer later: the layout
                // only names its background images AFTER it has run, so this
                // cannot happen in `begin_images`. The url is resolved exactly
                // as `pump_css_images` resolves it, or put and get would
                // use different keys for one picture.
                if engine.adopt_css_cached(*k, &resolve(url_str(), u)) {
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
        pump_css_images(&engine, &mut pending_css_imgs, layout, band);
        // ALWAYS yield so this worker core can halt — a cooperative fiber that
        // never sleeps pins its core at 100%. A short nap while interacting
        // stays responsive; a longer one when idle keeps the core asleep.
        unsafe {
            // Anything on the wire keeps the short nap: that is how often we
            // ask the kernel whether the answer is here, and it is the whole
            // latency the split costs. 4 ms against a round trip is nothing.
            let waiting = nav_busy() || img_job() >= 0 || cssimg_job() >= 0;
            let busy = had_event
                || waiting
                || !pending_imgs.is_empty()
                || !pending_css_imgs.is_empty();
            let nap = if busy { 4 } else { 16 };
            let _ = npk_sleep(nap);
        }
    }
}

// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
#[allow(dead_code)]
fn _keep_iconref_alive() -> Option<app_meta::IconRef> {
    None
}
