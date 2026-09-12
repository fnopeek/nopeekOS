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
    /// Der Statuscode der zuletzt eingesammelten Antwort. Fuer eine
    /// Navigation ist er gleichgueltig — eine 404-Seite ist ein Dokument —,
    /// fuer `fetch` ist er die halbe Auskunft: `response.ok` haengt daran.
    fn npk_http_status() -> i32;
    /// Seconds since the epoch, UTC. `npk_ticks` cannot stand in: it restarts
    /// at every boot and a cookie's `Expires` is an absolute date.
    fn npk_unix_time() -> i64;
    /// Zufall aus dem CSPRNG des Kernels (ChaCha20, aus RDRAND geseedet).
    /// Liefert die geschriebenen Bytes, oder -1. Hoechstens 64 KiB je Aufruf.
    fn npk_random_bytes(ptr: i32, len: i32) -> i32;
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
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_clipboard_set(ptr: i32, len: i32) -> i32;
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_http_begin_many(urls_ptr: i32, urls_len: i32, out_max: i32) -> i32;
    /// Wie oben, aber mit einer Keks-Zeile JE ADRESSE (durch `\n` getrennt,
    /// leere Zeilen zaehlen mit). Seit Kernel 0.333.0.
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

// Der Tabstreifen. Zwei Baender statt zwei Zahlen je Tab: der Streifen wird
// bei jeder Aenderung neu gebaut, und ein Index IST der Tab.
const ACT_TAB_NEW: u32 = 7_000;
const ACT_TAB_SEL: u32 = 7_100;     // + Index
const ACT_TAB_CLOSE: u32 = 7_200;   // + Index

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

/// **Was DIESEM Dokument gehoert — und nicht dem Programm.**
///
/// Der erste Schritt zu Tabs, und er zahlt sich schon bei einem aus
/// (`docs/plan/BROWSER_TABS.md` §A2). beak hielt seinen Zustand in
/// siebenundsiebzig `static mut`, und die grosse Mehrheit davon beschreibt
/// das eine geladene Dokument. Solange das so ist, gibt es keinen zweiten
/// Ort, an dem eine zweite Seite stehen koennte — ein Tabstrip waere nur
/// Fassade.
///
/// Der Gewinn ist aber nicht erst der zweite Tab: **0.147.0 war genau diese
/// Klasse Fehler.** `URL_BUF` war die Adresse des DOKUMENTS *und* der Inhalt
/// des Textfelds, und daraus wurde ein Datenschutzfehler (jedes Praefix
/// einer Eingabe ging an den DNS) plus ein Loch in der Reichweiten-Grenze.
/// Zwei Felder in einer Struktur koennen nicht derselbe Puffer sein.
///
/// Gewandert wird gruppenweise, und nach jeder Gruppe muessen
/// `beak:selftest` und WPT unveraendert sein.
///
/// **Der Zaehler:**
///
///     grep -c '^static mut ' tools/wasm/beak/src/lib.rs
///     0.151.0:  77          <- vorher
///     0.152.0:  46          <- Adresse, Verlauf, Navigation, Ladevorgang
///     0.153.0:  47          <- +COOKIE_BUF, ein ABHOLpuffer
///     0.163.0:  23          <- Ansicht, Nebenabrufe, Skriptrunde
///     0.163.0:  24          <- und die Tabs legen EINE zurueck:
///                              aus `DOC` wurden `TABS` + `ACTIVE`
///
/// **Die Regel ist nicht „die Zahl faellt", sondern „nichts, was dem
/// DOKUMENT gehoert, kommt dazu".** 0.153.0 hat einen Puffer bekommen, in
/// den die gespeicherten Kekse geholt werden — der gehoert dem Abruf, nicht
/// der Seite, und vervielfacht sich mit Tabs nicht. Wer die Zahl allein
/// bewacht, verbietet das Richtige und uebersieht das Falsche.
///
/// **Die 23, die stehen bleiben, sind keine Reste — sie gehoeren woanders
/// hin**, und jede Gruppe hat einen Grund, der eine zweite Seite ueberlebt:
///
/// | 13 | Abholpuffer (`HTML_BUF`, `CSS_BUF`, `IMG_FETCH_BUF` …, ~47 MB `.bss`) | gehoeren dem Abruf, der gerade laeuft — es laeuft einer |
/// | 3 | `LAST_W`/`LAST_H`/`LAST_SY` | beschreiben den BILDPUFFER, und der ist einer |
/// | 3 | `SCRIPT_DEADLINE`, `BUDGET_T0`, `BUDGET_SAID` | beschreiben den LAUF auf dem Stapel, und der ist einer |
/// | 3 | `OPEN_MENU`, `USE_SITE_CSS`, `INSPECT_MODE` | Fenster und Werkzeug, nicht Seite |
/// | 2 | `TABS`, `ACTIVE` | die Dokumente selbst, und welches lebt |
struct Doc {
    /// Woher das Dokument KAM (nach Weiterleitungen). Basis fuer jede
    /// relative Adresse der Seite und der Netzkontext, den der Kernel fuehrt.
    url: String,
    /// Was in der Adresszeile STEHT. Nur Text — kein Netzkontext, kein DNS.
    edit: String,
    /// Zurueck/Vorwaerts. War ein festes Feld aus 64 × 4 KB (256 KB `.bss`),
    /// das je Tab noch einmal dagestanden haette.
    hist: Vec<String>,
    hist_pos: usize,

    // ── Der Ladevorgang dieses Dokuments ────────────────────────────────
    /// Der laufende Abruf, oder -1.
    nav_job: i32,
    /// Welche Stufe der Kette gerade laeuft.
    nav_stage: NavStage,
    /// Die Adresse, die der laufende Abruf VERLANGT hat — nicht die, aus der
    /// er am Ende kam.
    nav_url: Option<String>,
    nav_push_hist: bool,
    /// Beginn der laufenden Stufe, fuer die Zeitzeilen im Log.
    nav_stage_ms: i64,
    /// Zaehlt jede Navigation. Woran haengende Rueckrufe erkennen, dass sie
    /// zu einer Seite gehoeren, die es nicht mehr gibt.
    nav_gen: u32,
    nav_start_ms: i64,
    nav_reported: bool,
    /// Zaehlt jede Aenderung am Inhalt — die Zahl, an der das Layout haengt.
    content_gen: u32,
    /// Wie weit die Seite gerollt ist.
    scroll_y: i32,
    /// Wo eine Textmarkierung begonnen hat, solange die Taste unten ist.
    sel_anchor: Option<beak_engine::select::TextPos>,
    /// Der Link unter dem Druck, bis das Loslassen entscheidet — mit dem
    /// Punkt, an dem gedrueckt wurde.
    pending_link: Option<(String, i32, i32)>,
    /// Die Markierung auf der SEITE — nicht in der Adresszeile, die gehoert
    /// dem Compositor.
    sel: Option<(beak_engine::select::TextPos, beak_engine::select::TextPos)>,
    /// Die Suchleiste: `None` heisst zu. Der Text gehoert BEAK, nicht einem
    /// `Widget::Input` — `Event::InputChange` traegt keine Knotenkennung,
    /// zwei Eingabefelder im Fenster waeren also nicht auseinanderzuhalten.
    /// Ein selbst gefuehrter Puffer ist die kleinere Antwort als eine
    /// ABI-Erweiterung.
    find: Option<String>,
    /// Fundstellen der laufenden Suche und die, auf der man gerade steht.
    found: Vec<(beak_engine::select::TextPos, beak_engine::select::TextPos)>,
    find_at: usize,
    /// Sammelpuffer fuer ein Zeichen in der Suchleiste.
    find_pending: [u8; 4],
    find_pending_len: u8,

    // ── Die Teilabrufe des Ladevorgangs ─────────────────────────────────
    // Blaetter, Skripte, Module: jede Stufe fuehrt Buch darueber, was sie
    // verlangt hat und in welcher Runde sie steht. Das gehoert zum LADEN
    // EINES Dokuments — zwei Tabs, die gleichzeitig laden, brauchen zwei
    // davon, und mit einer Static waeren es zwei Seiten auf einem Zettel.
    nav_css_count: usize,
    nav_scripts: Option<Vec<PendingScript>>,
    /// Die JS-Sitzung DIESER Seite.
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

    // ── Die Ansicht DIESES Dokuments ────────────────────────────────────
    // Nicht zu verwechseln mit dem, was der BILDPUFFER haelt (`LAST_W/H/SY`
    // unten): der Puffer ist einer, die Seiten sind viele.
    /// Das Bild ist nicht mehr das, was die Seite sagt.
    dirty: bool,
    /// Und zwar aus einem anderen Grund als Rollen — also ganz neu malen.
    need_full: bool,
    /// Ein Bild ist angekommen, die Bildliste muss neu durchgesehen werden.
    images_dirty: bool,
    /// Die Kaesten des letzten Layouts, fuer `getBoundingClientRect` & Co.
    ///
    /// Als `Rc` gehalten, damit das Weiterreichen an die JS-Maschine nichts
    /// kostet: der Rollstand aendert sich bei JEDEM Bild, die Kaesten nur bei
    /// einem neuen Layout — ohne das waere jede Rollbewegung eine Kopie von
    /// ~180 KB.
    geom: Option<alloc::rc::Rc<alloc::vec::Vec<beak_engine::layout::ElemRect>>>,
    /// Das Sichtfeld, das die JS-Sitzung dieses Dokuments zuletzt gehoert hat.
    last_vp: (i32, i32),

    // ── Die Nebenabrufe DIESES Dokuments ────────────────────────────────
    // Bilder, Hintergruende, Schriften, `fetch`: alles, was NEBEN dem
    // Dokument laeuft und mit ihm endet. Eine zweite Seite hat ihre eigenen —
    // und `subresources_cancel` bricht genau die einer Seite ab, nicht die
    // aller.
    /// Der laufende `<img>`-Stapel und wonach er gefragt hat, damit ein
    /// ankommender Rumpf unter der Quelle abgelegt wird, unter der die Seite
    /// ihn genannt hat. -1 / None, wenn nichts unterwegs ist.
    img_job: i32,
    img_job_srcs: Option<Vec<(String, String)>>,
    /// Dasselbe fuer Hintergruende, benannt wie das Layout sie fuehrt.
    cssimg_job: i32,
    cssimg_job_keys: Option<Vec<(u64, String)>>,
    /// Die laufende Schriftrunde.
    font_job: i32,
    font_want: Option<Vec<(String, u32, u16, bool)>>,
    /// Welche `fetch`-Anfrage der Engine auf welchem Griff des Wirts liegt.
    fetch_jobs: Vec<(u32, i32)>,
    /// Die Bilder DIESER Seite, die noch nicht gefragt wurden — der Rest der
    /// Schlange hinter dem Stapel, der gerade laeuft.
    ///
    /// Standen bis 0.163.0 als Locals in der Schleife, und damit gehoerten
    /// sie niemandem: bei einem Tabwechsel haette der neue Tab die Bilder des
    /// alten weitergeholt, mit `resolve` gegen die NEUE Basis.
    pending_imgs: Vec<String>,
    pending_css_imgs: Vec<(u64, String)>,
    /// Jeder Hintergrund, nach dem diese Seite schon gefragt hat — auch die
    /// gescheiterten. Ein Fehlschlag darf nicht ewig wiederholt werden.
    css_asked: Vec<u64>,

    // ── Die Skriptrunde DIESES Dokuments ────────────────────────────────
    /// Wie viele Navigationen die Seite HINTEREINANDER selbst ausgeloest hat.
    script_nav_chain: u32,
    /// Setzt `sync_nav` unmittelbar vor `nav_begin` — daran erkennt
    /// `nav_begin`, dass die Kette WEITERgeht statt neu anzufangen.
    nav_from_script: bool,
    /// Was die gewoehnlichen Skripte ergeben haben (gelaufen, gescheitert,
    /// Bytes) — muss die Modulrunden ueberleben, weil der Bericht erst danach
    /// geschrieben wird.
    script_tally: (usize, usize, usize),
    /// Wann die Skriptrunde begann. NICHT `nav_stage_ms`: das steht nach einer
    /// Modulrunde auf deren Beginn, und die gemeldete Zeit waere zu klein.
    script_t0: i64,
    /// Steht `load` noch aus? Es faellt erst, wenn die Geometrie steht.
    load_pending: bool,
    /// Wie viele Bilder hintereinander ein Beobachter-Rueckruf schon den Baum
    /// geaendert hat — der Riegel gegen die „ResizeObserver loop".
    obs_rounds: u32,

    // ── Was diese Seite gekostet hat, und was darueber EINMAL gesagt wird ─
    // Alle fuenf setzt `set_url` zurueck: eine neue Seite bekommt ihr eigenes
    // Urteil und ihre eigene Gelegenheit, es zu sagen. Ohne das brachte eine
    // schwere Seite den Zeiger fuer jede spaetere zum Schweigen.
    /// Was das letzte volle Layout gekostet hat, ms — die Zahl, die
    /// entscheidet, ob diese Seite sich `:hover` leisten kann.
    last_layout_ms: i64,
    hover_refused: bool,
    hover_said_fast: bool,
    hover_said_slow: bool,
    ctl_bail_said: bool,
    /// Der im Inspektor gewaehlte Kasten: `(x, y, w, h)` im Dokumentraum, mit
    /// seiner Beschriftung. Die Koordinaten gelten in DIESEM Dokument.
    sel_box: Option<(i32, i32, i32, i32, String)>,

    // ── Was einen eingefrorenen Tab wiederherstellt ─────────────────────
    // Diese Felder und die vier ganz oben (`url`, `edit`, `hist`, `hist_pos`)
    // sind ALLES, was ein Tab im Hintergrund behaelt — Kilobytes statt der
    // 44 MiB, die eine lebende Seite haelt. Siehe `tab_freeze`.
    /// Der `<title>` der Seite, fuer den Streifen. Wird beim Auslegen
    /// nachgezogen (frueher weiss es niemand) und ueberlebt das Einfrieren.
    title: String,
    /// Wohin nach dem Laden gerollt werden soll. 0 fuer eine neue Seite, der
    /// gemerkte Stand fuer einen Tab, der zurueckkommt — die Seite wird beim
    /// Zurueckwechseln neu geholt, und ohne diese Zahl stuende sie oben.
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
            nav_css_count: 0, nav_scripts: None, js: None, nav_js_count: 0, nav_mod_entries: None, nav_mod_want: None, nav_mod_rounds: 0, nav_css_urls: None, nav_css_parts: None, nav_css_want: None, nav_css_rounds: 0, nav_sheet_nodes: None, nav_sheet_rounds: 0,
            dirty: true, need_full: true, images_dirty: false, geom: None, last_vp: (0, 0),
            img_job: -1, img_job_srcs: None, cssimg_job: -1, cssimg_job_keys: None,
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

/// **Die Tabs.** `docs/plan/BROWSER_TABS.md` §A3 (b): EIN lebendiger Motor,
/// der Rest eingefroren.
///
/// Der Motor haelt einen Baum, ein Stilblatt und die Bilder EINER Seite;
/// `HTML_BUF`/`CSS_BUF` sind je ein Puffer. Zwei lebende Seiten waeren also
/// zwei Motoren — 44 MiB gehaltene Halde je Stueck, gemessen auf srf.ch. Ein
/// Tab im Hintergrund ist deshalb nur das, was ihn wiederherstellt (Adresse,
/// Verlauf, Rollstand, Titel), und beim Zurueckwechseln wird die Seite neu
/// geholt. Was das billig macht, ist schon gebaut: `DOC_SLOTS = 3` im Motor
/// haelt die letzten drei geparsten Baeume, und der Bildspeicher ueberlebt
/// die Navigation.
///
/// Der Preis, und er gehoert benannt: **Skriptzustand geht verloren.** Ein
/// halb ausgefuelltes Formular, ein offenes Menue, ein Warenkorb per Skript
/// sind nach einem Tabwechsel weg. Das ist der Eintausch fuer „zwanzig Tabs
/// kosten wie einer"; die Gegenrichtung (2-3 lebendige nach LRU) steht in
/// §A3 und braucht mehrere Motoren.
///
/// **`Box`, und das ist keine Zierde.** `js_session()` und `fetch_jobs()`
/// geben `&'static mut` INS Dokument heraus. Ein `Vec<Doc>`, der beim
/// Oeffnen eines Tabs umzieht, liesse sie auf den alten Speicher zeigen —
/// ein Fehler, den man erst Wochen spaeter als Datenmuell sieht. Ein `Box`
/// steht fest, was der Vec auch tut.
static mut TABS: Vec<alloc::boxed::Box<Doc>> = Vec::new();
/// Welcher Tab gemalt wird und lebt. Immer gueltig — `active` klemmt.
static mut ACTIVE: usize = 0;

/// Wieviele Tabs.
///
/// **Der Speicher ist es nicht** — ein eingefrorener Tab ist Kilobytes. Es
/// ist der Streifen: 160 px je Tab, zehn davon sind 1600 px, und darueber
/// schoebe der elfte das `+` aus dem Fenster. Was diesen Deckel hebt, ist
/// ein rollender Streifen (`Widget::Scroll`, `Axis::Horizontal`) — der
/// einzige Kasten, der im Compositor wirklich abschneidet.
const MAX_TABS: usize = 10;

fn tabs() -> &'static mut Vec<alloc::boxed::Box<Doc>> {
    // SAFETY: ein Faden; die Entleihung endet in der rufenden Anweisung.
    // Der leere Fall trifft genau einmal, beim allerersten Zugriff.
    unsafe {
        let p = core::ptr::addr_of_mut!(TABS);
        if (*p).is_empty() {
            (*p).push(alloc::boxed::Box::new(Doc::new()));
        }
        &mut *p
    }
}

/// Der laufende Tab. **Geklemmt, nicht geprueft:** ein `ACTIVE`, das auf
/// einen geschlossenen Tab zeigt, waere sonst eine Panik an einer Stelle, an
/// der niemand sie erwartet.
fn active() -> usize {
    let n = tabs().len();
    let a = unsafe { core::ptr::addr_of!(ACTIVE).read() };
    if a >= n { n - 1 } else { a }
}
fn set_active(i: usize) {
    unsafe { core::ptr::addr_of_mut!(ACTIVE).write(i) };
}

/// **Die Regel fuer beide: eine Entleihung je Anweisung.**
///
/// beak ist ein einziger Faden, es gibt also keinen zweiten Zugriff — aber
/// „einfaedig" ist nicht dasselbe wie „aliasfrei". Diese Referenzen kommen
/// aus einem `unsafe`-Deref und fallen damit aus der Buchhaltung des
/// Rechners: er wuerde `let d = doc_mut(); … doc().x …` durchgehen lassen,
/// und das sind zwei gleichzeitige Entleihungen auf dasselbe Objekt. Auf
/// `&mut` ist das undefiniert, nicht bloss unschoen.
///
/// Praktisch heisst das: `doc().feld` und `doc_mut().feld = …` als GANZE
/// Anweisung sind immer richtig; ein festgehaltenes `let d = doc_mut()` nur
/// dann, wenn darin kein weiterer Aufruf steckt.
fn doc() -> &'static Doc {
    let a = active();
    &tabs()[a]
}
fn doc_mut() -> &'static mut Doc {
    let a = active();
    &mut tabs()[a]
}

/// Auf `cap` Bytes kuerzen, aber an einer ZEICHENgrenze.
///
/// Der Vorgaenger kopierte `min(len, cap)` rohe Bytes in einen festen Puffer;
/// traf das mitten in ein Zeichen, war der Inhalt kein gueltiges UTF-8 mehr
/// und `from_utf8(...).unwrap_or("")` machte daraus eine LEERE Adresse.
fn clip(s: &str, cap: usize) -> &str {
    if s.len() <= cap { return s }
    let mut n = cap;
    while n > 0 && !s.is_char_boundary(n) { n -= 1; }
    &s[..n]
}

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

// Scratch buffer a whole BATCH of <img> bytes arrives in before decoding.
//
// It is shared by `IMG_BATCH` images at once, so it was never "6 MB per
// picture" — it was 1.5. A single press photograph is bigger than that, and it
// would have failed with `n == 0`, which used to say nothing at all. The
// kernel bounds what all pending answers may reserve together
// (`MAX_RESERVED_BYTES`, 64 MB); 24 MB here leaves room for a document (3) +
// stylesheets (8) + scripts (8) in flight beside it.
const IMG_FETCH_CAP: usize = 24 * 1024 * 1024;
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
    // **Jede Unterressource bekommt ihren Keks.** Vorher trug nur die
    // Anfrage nach dem DOKUMENT eine `Cookie`-Zeile; Bilder, Blaetter und
    // Skripte gingen anonym raus. Hinter einer Anmeldung kam so der Text an
    // und die Bilder nicht, und es sah aus wie ein Bildfehler.
    //
    // Eine Zeile je Adresse, in derselben Reihenfolge, LEERE ZEILEN
    // EINGESCHLOSSEN — die Zuordnung ist die Position, und wer leere Zeilen
    // wegwirft, schickt den Keks der einen Adresse an eine andere.
    let now = unsafe { npk_unix_time() };
    let mut ck = String::new();
    let mut any = false;
    for (i, u) in urls.iter().enumerate() {
        if i > 0 { ck.push('\n'); }
        let v = cookies::header_for(u, now);
        if !v.is_empty() { any = true; ck.push_str(&v); }
    }
    // Kein Keks im Spiel? Dann der alte Weg — eine Zeile weniger ueber die
    // Grenze, und der Kernel muss nichts pruefen.
    if !any {
        return unsafe { npk_http_begin_many(blob.as_ptr() as i32, blob.len() as i32, cap as i32) };
    }
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
// Scratch for the kernel to write back the post-redirect URL of a fetch.
static mut FINAL_URL_BUF: [u8; URL_CAP] = [0; URL_CAP];

const PAYLOAD_CAP: usize = URL_CAP;
static mut PAYLOAD_BUF: [u8; PAYLOAD_CAP] = [0; PAYLOAD_CAP];

const EVENT_BUF_SIZE: usize = 16 * 1024;
static mut EVENT_BUF: [u8; EVENT_BUF_SIZE] = [0; EVENT_BUF_SIZE];

static mut RECT_BUF: [u8; 16] = [0; 16];

/// **Was der BILDPUFFER haelt — nicht, was die Seite sagt.**
///
/// Der Puffer ist einer, auch wenn es spaeter mehrere Dokumente gibt: diese
/// drei Zahlen beschreiben das Bild, das gerade im Puffer steht, und gehoeren
/// deshalb dem Fenster. Die Frage „muss neu gemalt werden?" gehoert dagegen
/// dem Dokument (`Doc::dirty`, `Doc::need_full`) — ein Bild, das im
/// Hintergrund ankommt, macht SEINE Seite alt, nicht das Bild auf dem Schirm.
static mut LAST_W: i32 = -1;
static mut LAST_H: i32 = -1;
/// The scroll offset the buffer currently HOLDS, so the next frame knows how
/// far the picture has to move.
static mut LAST_SY: i32 = 0;

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
    let d = doc_mut();
    d.url.clear();
    d.url.push_str(clip(s, URL_CAP));
    // Eine Markierung gehoert dem Text, den sie markiert. Die neue Seite hat
    // andere Textbefehle an denselben Stellen — die alten Orte zeigten dort
    // auf irgendetwas.
    d.sel = None;
    d.sel_anchor = None;
    // Ein Link, dessen Loslassen nie kam, darf die naechste Seite nicht
    // umleiten.
    d.pending_link = None;
    // Die Zeile zeigt, wo man IST — bis jemand hineintippt.
    set_edit(s);
    // A new page gets its own verdict on whether it can afford `:hover` —
    // and its own chance to say so once. Without this, one heavy page
    // silences the pointer for every page after it.
    let d = doc_mut();
    d.hover_refused = false;
    d.hover_said_fast = false;
    d.hover_said_slow = false;
    d.ctl_bail_said = false;
    d.last_layout_ms = 0;
}
fn url_str() -> &'static str { &doc().url }

/// Nur das Textfeld — kein Netzkontext, keine neue Basis, kein DNS.
///
/// **Ein Tastendruck ist keine Navigation.** Bis 0.146.0 rief jeder
/// Tastendruck `set_url`, und der meldet dem Kernel den Netzkontext; der
/// loest dafuer AUF. Am Geraet stand das als sechzehn DNS-Abfragen im Log —
/// `sandbox.nopeek.c`, `sandbox.nopeek.`, `sandbox.nopeek`, … bis zur leeren
/// Zeichenkette —, weil der Benutzer die Adresse rueckwaerts geloescht hat.
/// Jedes Praefix dessen, was jemand tippt, ging an den Aufloeser.
fn set_edit(s: &str) {
    let d = doc_mut();
    d.edit.clear();
    d.edit.push_str(clip(s, URL_CAP));
}

fn edit_str() -> &'static str { &doc().edit }
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
/// navigation (keyed on `Doc::nav_gen`, NOT the layout's content generation — a theme
/// switch or an image arriving must not wipe what the user has typed).
struct Page {
    forms: Forms,
    state: FormState,
    nav: u32,
    /// Stand des Baums, aus dem `forms` gebaut wurde.
    scripted: u64,
    /// Fingerabdruck des zuletzt GEMELDETEN Bestands.
    ///
    /// `sync` laeuft nach jedem Skriptlauf und nach jedem Beobachter-Rueckruf,
    /// nicht nur nach einer Navigation — der Kommentar unten sagte „once per
    /// navigation", und am Geraet standen dieselben acht Zeilen achtmal
    /// untereinander. Ein Bestand, der sich nicht geaendert hat, ist keine
    /// Nachricht.
    logged: u64,
}

impl Page {
    fn new() -> Page {
        Page { forms: forms::Forms { forms: Vec::new(), controls: Vec::new() },
               state: FormState::default(), nav: 0, scripted: 0, logged: 0 }
    }
    /// Das Formularmodell nachziehen — nach einer Navigation ODER nachdem
    /// Skripte den Baum ersetzt haben.
    ///
    /// **Aus dem LEBENDEN Baum, nicht aus dem Quelltext.** Eine Seite, die
    /// ihre Maske erst per Skript baut, hatte hier vorher gar keine
    /// Steuerelemente: das Bild zeigte ein Anmeldeformular, `submit` sagte
    /// „kein zugehoeriges Formular". Das ist kein Sonderfall einer Seite,
    /// sondern die Regel bei allem, was seine Oberflaeche zur Laufzeit baut.
    ///
    /// Liefert true, wenn wirklich neu eingesammelt wurde — nur dann darf der
    /// Rufer die Werte aus dem Baum uebernehmen.
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
        // Eine NAVIGATION wirft die Eingaben weg, ein Skriptlauf nicht — was
        // der Benutzer getippt hat, gehoert ihm, auch wenn die Seite daneben
        // etwas umbaut.
        if navigated { self.state.reset(); }
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
            // **Was auf dem Knopf STEHT.** Googles Einwilligungsseite hat vier
            // Formulare auf dieselbe Adresse; ohne die Beschriftung sind
            // „Alle ablehnen" und „Alle akzeptieren" im Log nicht zu
            // unterscheiden, und wer hier blind das erste nimmt, wirft eine
            // Muenze ueber eine Entscheidung des Benutzers.
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

// Navigation generation — bumped ONLY by a real page load.
fn nav_gen() -> u32 {
    doc().nav_gen
}
fn bump_nav_gen() {
    let d = doc_mut();
    d.nav_gen = d.nav_gen.wrapping_add(1);
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
    // The width belongs IN the number. A device timing without it cannot be
    // compared with anything -- 1000 px against 1880 px is a factor of 1.75 --
    // and reading it off the screen means opening a window, which changes the
    // very width being measured.
    let mut label = String::from("layout @");
    push_i64(&mut label, w as i64);
    label.push_str("px (parse+cascade+layout)");
    log_ms(&label, ms);
    doc_mut().last_layout_ms = ms;
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
    doc().scroll_y
}
/// Wohin die Seite nach dem Laden rollt.
///
/// **Null fuer eine neue Seite** — sie faengt oben an. Der gemerkte Stand
/// fuer einen Tab, der zurueckkommt: er wird beim Wechsel neu GEHOLT
/// (`docs/plan/BROWSER_TABS.md` §A3 b), und ohne diese Zahl staende der Leser
/// nach jedem Wechsel wieder ganz oben.
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
/// for scrolling ALONE can be blitted and have one band redrawn. Anything else
/// (a hover, a form key, a new layout) sets `need_full` and gets the whole
/// viewport. It is set, never cleared, until a frame is actually painted: a
/// hover followed by a scroll must still repaint everything.
fn mark_dirty() {
    let d = doc_mut();
    d.dirty = true;
    d.need_full = true;
}

/// Dirty because the viewport MOVED — the display list is untouched.
fn mark_dirty_scrolled() {
    doc_mut().dirty = true;
}

// Content generation — bumped on every fetch so the layout cache knows to
// re-lay-out (vs. reusing it for scroll, which keeps scrolling smooth).
/// When the current navigation started, so the first paint after it can
/// report the ONE number a user actually feels: click → something on screen.
/// Cleared once that number has been reported for this navigation.

/// Invalidate the layout cache. `why` is logged because a full re-layout is
/// the single most expensive thing this app does (~4.7 s on device), so an
/// unexpected one has to be attributable at a glance.
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
/// enough to be worth having. Only the FALLBACK is measured against it: a
/// pointer change the engine can answer by repainting costs a fraction of a
/// millisecond and is never refused.
const HOVER_BUDGET_MS: i64 = 250;
/// Say ONCE per page how the pointer is being answered here.
///
/// Once each, for the reason in `Doc`: without them a pointer answered by
/// repainting is INVISIBLE in the log, so a device run cannot tell "it works"
/// from "it never happened" — which is exactly what the first 0.28.0 log could
/// not say ([[feedback-log-the-version-in-the-trace]]).
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
    /// Die `@import`-Blaetter der verlinkten Blaetter. Rundenweise wie `Mod`:
    /// ein Blatt nennt seine eigenen Importe erst, wenn es da ist.
    CssImport,
}

/// Handle of the navigation in flight, or -1.
/// The address that was ASKED for. The diagnostic page names it, and it
/// stands in for the base URL if the response never said where it came from.
/// Record the landing address in the history once the document is here.
/// Where we LANDED, not where we aimed — otherwise every trip back through
/// history replays the redirect.
/// When the stage in flight started, so each round trip reports its own span
/// instead of the navigation's total.

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
        unsafe { npk_http_cancel(h) };
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
    // Eine Navigation, die nicht aus einem Skript kam — ein Klick, die
    // Adresszeile, der Verlauf — bricht die Skriptkette. Sonst zaehlte der
    // Deckel ueber Seiten hinweg weiter und wuerde irgendwann eine
    // vollkommen harmlose Weiterleitung abwuergen.
    if doc().nav_from_script {
        doc_mut().nav_from_script = false;
    } else {
        doc_mut().script_nav_chain = 0;
    }
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
    { doc_mut().js = None };

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
    // **Welche Kekse mitgehen, nach NAMEN.** Der Wert ist ein Geheimnis, der
    // Name nicht — und ohne ihn ist „5 held" keine Auskunft. Googles
    // Einwilligung schickte im Kreis, und aus dem Log war nicht zu sehen, ob
    // `SOCS` ueberhaupt dabei war.
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
    let h = unsafe {
        npk_http_begin(
            method.as_ptr() as i32, method.len() as i32,
            url.as_ptr() as i32, url.len() as i32,
            hdrs.as_ptr() as i32, hdrs.len() as i32,
            body.as_ptr() as i32, body.len() as i32,
            HTML_CAP as i32,
        )
    };
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
/// address bar keeps the URL that was ASKED for rather than one derived from
/// a response we never got.
fn nav_fail(url: &str) {
    // Ein gemerkter Rollstand gehoert der Seite, die nicht kam — nicht der
    // Fehlermeldung, die an ihrer Stelle steht.
    doc_mut().scroll_want = 0;
    set_scroll(0);
    bump_content_gen("navigation");
    bump_nav_gen();
    show_error_page(url);
    mark_dirty();
    nav_clear();
}

/// File whatever `Set-Cookie` the response carried.
///
/// Wo die dauerhaften Kekse liegen.
///
/// **Im PRIVATEN Bereich, nicht in `sys/config/beak`.** Ein Keks ist keine
/// Einstellung, er IST die Anmeldung — und `npk_fetch` prueft die
/// Kapabilitaet und danach jeden Pfad, also haette jede App mit READ alle
/// Sitzungen der Maschine lesen koennen. `priv/<modul>/…` ist der einzige
/// Ort, den eine Kapabilitaet nicht aufmacht: der Name entscheidet, und den
/// vergibt der Kernel. beak behaelt dadurch `RENDER | CANVAS | NET` und
/// braucht fuer den eigenen Zustand kein Recht am Speicher der Maschine.
const COOKIE_FILE: &str = "priv/beak/cookies";
const COOKIE_CAP: usize = 96 * 1024;   // 256 Kekse passen weit darunter
static mut COOKIE_BUF: [u8; COOKIE_CAP] = [0; COOKIE_CAP];

/// Die gespeicherten Kekse ins Glas — einmal beim Start.
fn cookies_restore() {
    let now = unsafe { npk_unix_time() };
    let p = core::ptr::addr_of_mut!(COOKIE_BUF) as *mut u8;
    let n = unsafe {
        npk_fetch(COOKIE_FILE.as_ptr() as i32, COOKIE_FILE.len() as i32,
                  p as i32, COOKIE_CAP as i32)
    };
    // Keine Datei ist der Normalfall beim ersten Start, kein Fehler.
    if n <= 0 { return }
    let bytes = unsafe { core::slice::from_raw_parts(p as *const u8, n as usize) };
    let Ok(text) = core::str::from_utf8(bytes) else {
        log("[beak] cookies: gespeicherte Datei ist kein UTF-8 — uebergangen");
        return;
    };
    let got = cookies::load(text, now);
    if got > 0 {
        log(&alloc::format!("[beak] cookies: {got} aus dem Speicher zurueck"));
    }
}

/// Die dauerhaften Kekse auf die Platte. Sitzungskekse bleiben draussen —
/// `Jar::serialize` entscheidet das, nicht diese Stelle.
fn cookies_persist() {
    let now = unsafe { npk_unix_time() };
    let text = cookies::serialize(now);
    let r = unsafe {
        npk_store(COOKIE_FILE.as_ptr() as i32, COOKIE_FILE.len() as i32,
                  text.as_ptr() as i32, text.len() as i32)
    };
    if r < 0 {
        log("[beak] cookies: konnten nicht gespeichert werden");
    }
}

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
        // Nur wenn sich wirklich etwas geaendert hat — sonst schriebe jede
        // Antwort dieselbe Datei neu.
        cookies_persist();
        let mut m = String::from("[beak] cookies: ");
        push_i64(&mut m, after as i64);
        m.push_str(" held");
        // Und WELCHE der Host jetzt hat. Eine Zahl sagt nicht, ob der eine
        // Keks dabei ist, an dem die Sitzung haengt.
        let da = cookies::names_held(&from);
        if !da.is_empty() {
            m.push_str(" — hier: ");
            m.push_str(&da);
        }
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
        doc_mut().nav_start_ms = now_ms();
        doc_mut().nav_reported = false;
    }
    scroll_after_load();
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
    match doc().nav_stage {
        NavStage::Doc => nav_document_arrived(engine),
        NavStage::Css => nav_stylesheets_arrived(engine),
        NavStage::Js => nav_scripts_arrived(engine),
        NavStage::Mod => nav_modules_arrived(engine),
        NavStage::Sheet => nav_sheets_arrived(engine),
        NavStage::CssImport => nav_css_imports_arrived(engine),
    }
    true
}

fn nav_document_arrived(engine: &Engine) {
    let h = nav_job();
    let asked = nav_asked();
    let dst = core::ptr::addr_of_mut!(HTML_BUF) as *mut u8;
    let n = unsafe { npk_http_take(h, dst as i32, HTML_CAP as i32) };
    log_ms("fetch document", now_ms() - doc().nav_stage_ms);
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
    scroll_after_load();
    bump_content_gen("navigation");
    bump_nav_gen();
    // Relative sub-resources resolve against the URL the document came FROM,
    // not the one we asked for (RFC 3986 §5.1.3). Getting this wrong made
    // every stylesheet and image repeat the document's own redirect.
    let base = fetched_from().unwrap_or(asked);
    set_url(&base);
    if doc().nav_push_hist {
        hist_push(url_str());
    }
    nav_begin_stylesheets(engine, &base);
}

/// Start the second round trip: every `<link rel=stylesheet>` of the document
/// that just landed, in ONE batch. They are render-blocking, so this is where
/// overlapping the round trips is worth the most. Bounded by CSS_CAP +
/// MAX_CSS_LINKS.
fn nav_begin_stylesheets(engine: &Engine, base: &str) {
    // Eine abgebrochene Navigation darf der naechsten keine Blaetter
    // hinterlassen ([[feedback_a_copy_is_a_second_semantics_waiting]]).
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
    {
        doc_mut().nav_stage = NavStage::Css;
        doc_mut().nav_job = h;
        doc_mut().nav_css_count = urls.len();
        doc_mut().nav_stage_ms = now_ms();
        // An `@import` resolves against ITS OWN sheet's address, not the
        // document's, so the addresses have to survive the round trip.
        doc_mut().nav_css_urls = Some(urls);
        doc_mut().nav_css_rounds = 0;
    }
}

/// How many sheets the batch in flight asked for.
/// Die Skripte der Seite in Dokumentreihenfolge, waehrend die externen noch
/// unterwegs sind. `None` heisst: keine offene Skriptrunde.
/// Die JS-Sitzung DIESER Seite.
///
/// Sie muss die Skriptrunde ueberleben: die Behandler, die ein Skript
/// anmeldet, leben in ihr, und ohne sie waere jeder `addEventListener` beim
/// Verlassen der Funktion wieder weg. Eine Navigation wirft sie weg.

fn js_session() -> Option<&'static mut beak_engine::js::Session> {
    doc_mut().js.as_mut()
}
/// Wie viele externe Adressen das Buendel angefordert hat.
/// Die Modul-Einstiege der Seite, in Dokumentreihenfolge — das sind die
/// Adressen, die am Ende ausgewertet werden.
/// Die Adressen, die in DIESER Runde unterwegs sind, in Bestellreihenfolge.
/// Wie viele Runden schon liefen — der Deckel gegen einen Graphen, der sich
/// selbst nachlaedt.
/// Die Knoten der Stilblaetter, die in DIESER Runde unterwegs sind — in
/// Bestellreihenfolge, damit die Antwort dem `<link>` zugeordnet werden kann.
/// Die Blaetter der Seite in KASKADENREIHENFOLGE, waehrend die `@import`-Runden
/// laufen: `(Adresse, Rumpf, schon nach Importen durchsucht)`. Ein Import wird
/// VOR seinem Blatt eingefuegt — dort steht er in der Kaskade, weil ein
/// `@import` wirkt, als staende sein Inhalt an seiner Stelle, und das ist vor
/// allem, was danach im Blatt folgt.
/// Welches Blatt jede Adresse der Runde in Arbeit angefordert hat.
/// Ein Blatt darf importieren, was importiert, was importiert — aber nicht
/// endlos, und ein Ring darf die Navigation nicht anhalten.
const MAX_IMPORT_ROUNDS: usize = 4;
const MAX_IMPORT_SHEETS: usize = 64;


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
    let want = doc().nav_css_count;
    // Fetched into a scratch buffer first because the bodies come back
    // concatenated, and they need a separator between them: without one, a
    // sheet not ending in `}` would merge into the next sheet's first rule.
    let mut scratch: Vec<u8> = Vec::with_capacity(CSS_CAP);
    let spans = take_batch(h, scratch.as_mut_ptr(), CSS_CAP, want);
    let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
    unsafe { scratch.set_len(total.min(CSS_CAP)) };

    // The bodies are kept as PARTS rather than written straight into
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
/// Round-based like the module graph: a sheet only names its own imports once
/// it has arrived. `sandbox.nopeek.ch` is the shape this exists for — one
/// `<link>` to a `main.css` that holds nothing but fifteen `@import`s, and
/// every one of them is the actual design.
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
    let spans = take_batch(h, scratch.as_mut_ptr(), CSS_CAP, want.len());
    let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
    unsafe { scratch.set_len(total.min(CSS_CAP)) };
    let (mut ok, mut bad) = (0usize, 0usize);
    if let Some(parts) = doc_mut().nav_css_parts.as_mut() {
        // **Von hinten einfuegen.** Jedes Einfuegen verschiebt alles dahinter;
        // absteigend bleiben die noch offenen, kleineren Stellen gueltig.
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
    unsafe { core::ptr::addr_of_mut!(CSS_LEN).write(0) };
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
    // Erst die Skripte einsammeln. Sind externe dabei, geht die Navigation in
    // eine dritte Stufe und endet erst danach — sonst waere die Seite fertig,
    // bevor ihre Skripte sie gebaut haben.
    if nav_begin_scripts(engine) { return; }
    nav_done();
}

/// Die Seite ist fertig: zeichnen und Bilder holen.
fn nav_done() {
    doc_mut().images_dirty = true;
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
    log_ms("fetch scripts", now_ms() - doc().nav_stage_ms);
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
        // Ein Keks per Skript ist so dauerhaft wie einer per Kopfzeile —
        // `document.cookie = "…; expires=…"` ist genau derselbe Vertrag.
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
/// Was die Seite an Rollen verlangt hat, ausfuehren.
///
/// **Die Engine rollt nicht** — sie hat kein Fenster; sie merkt sich den
/// Wunsch, und hier steht der Rollstand. Waagerecht rollt beak nicht, also
/// wird der x-Wunsch bewusst verworfen statt so getan, als waere er
/// angekommen.
fn sync_scroll(sess: &mut beak_engine::js::Session) {
    let Some((_x, y)) = sess.interp.take_scroll() else { return };
    let Some(y) = y else { return };
    if !y.is_finite() { return }
    let want = y.max(0.0) as i32;
    if want == scroll_y() { return }
    set_scroll(want);
    mark_dirty();
}

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
    let count = doc().hist.len();
    sess.interp.set_history(count.max(1) as f64, beak_engine::js::value::Value::Null);
}

/// Wie viele Navigationen eine Seite HINTEREINANDER selbst ausloesen darf.
///
/// Nicht gegen langsame Seiten, sondern gegen `location.href = "/a"` AUF
/// `/a` — das ist eine Endlosschleife, die je Runde eine Rundreise kostet
/// und von aussen wie ein haengender Browser aussieht. Eine Kette von
/// wenigen ist dagegen normal: Googles Sperrseite braucht zwei.
const SCRIPT_NAV_MAX: u32 = 8;

/// Was die Seite per `location` verlangt hat — abholen und wirklich fahren.
///
/// **Der Unterschied zu `sync_history` ist der ganze Punkt.**
/// `pushState` schreibt die Adresse um und laesst das Dokument stehen;
/// `location.replace` wirft es weg und holt ein neues. Bis hierher war der
/// zweite Fall gar nicht da: `location` war ein Datenobjekt, `replace` ein
/// `TypeError`, und `location.href = u` schrieb still eine Eigenschaft um.
/// Eine Seite, die sich selbst weiterschickt, kam nie an.
///
/// Liefert true, wenn navigiert wurde — dann ist das Dokument von eben weg
/// und der Rufer muss aufhoeren, daran zu arbeiten.
fn sync_nav(engine: &Engine) -> bool {
    let Some(sess) = js_session() else { return false };
    let Some(n) = sess.interp.take_nav() else { return false };
    // **Nur, was ein Dokument liefern kann.** Die Engine hat `javascript:`
    // und `data:` schon abgelehnt; hier faellt der Rest (`mailto:`, `file:`,
    // `blob:`) — nicht weil er gefaehrlich waere, sondern weil `nav_begin`
    // ihn ins Netz reichen wuerde und die Antwort eine leere Seite ist, die
    // aussieht wie ein Fehler der Gegenstelle.
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
    // `replace` und `reload` haengen KEINEN Eintrag an: sonst kaeme man mit
    // „zurueck" nie aus einer Seite heraus, die sich selbst ersetzt.
    nav_begin(engine, "GET", &n.url, &[], "", !n.replace && !n.reload);
    true
}

/// Liefert true, wenn eine Modulrunde laeuft — dann ist die Navigation
/// NOCH nicht fertig.
fn run_scripts(engine: &Engine, list: Vec<PendingScript>) -> bool {
    doc_mut().script_t0 = now_ms();
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
    doc_mut().js = Some(sess);
    doc_mut().script_tally = (ran, failed, bytes);
    doc_mut().nav_mod_entries = if entries.is_empty() { None } else { Some(entries) };
    doc_mut().nav_mod_rounds = 0;
    module_pump(engine)
}


/// Die Schriftrunde. Eigener Auftrag, nicht der der Navigation: welche
/// Schriften eine Seite braucht, weiss die Engine erst nach dem ersten
/// Auslegen — genau wie bei Bildern.
/// Wie viele Schriften eine Seite in einer Runde holen darf, und wie viele
/// Bytes zusammen. Gemessen: eine Seite bringt 3 bis 6 mit, je 30-90 KB.
const MAX_FONT_URLS: usize = 12;
const FONT_CAP: usize = 4 * 1024 * 1024;

/// Wieviele `fetch`-Anfragen gleichzeitig unterwegs sein duerfen.
///
/// Nicht erfunden, sondern die Grenze des Wirts: er haelt je Anfrage einen
/// Antwortpuffer, und eine Seite, die zwanzig auf einmal stellt, band damit
/// vierzig Megabyte. Was nicht drankommt, wartet auf die naechste Runde.
const MAX_FETCH_INFLIGHT: usize = 6;
const FETCH_CAP: usize = 2 * 1024 * 1024;

fn fetch_jobs() -> &'static mut Vec<(u32, i32)> {
    &mut doc_mut().fetch_jobs
}

/// Der Kopfblock der zuletzt eingesammelten Antwort.
fn response_headers() -> String {
    let hp = core::ptr::addr_of_mut!(HDR_BUF) as *mut u8;
    let hn = unsafe { npk_http_response_headers(hp as i32, HDR_CAP as i32) };
    if hn <= 0 { return String::new() }
    let bytes = unsafe { core::slice::from_raw_parts(hp as *const u8, hn as usize) };
    core::str::from_utf8(bytes).unwrap_or("").to_string()
}

/// Eine Runde an den `fetch`-Anfragen der Seite.
///
/// **Die Engine holt nichts** — sie legt die Anfrage hin, das hier holt sie.
/// Derselbe Weg wie `pending_sheets`/`sheet_done`, nur mit einem Griff JE
/// ANFRAGE statt einem je Runde: `fetch` ist nebenlaeufig, eine Seite stellt
/// mehrere und wartet auf alle.
///
/// Ein Abbruch ist hier ECHT: `controller.abort()` legt die id ab, und die
/// Verbindung wird beendet. Nur die Fahne zu setzen hiesse, dass die Seite
/// weiterlaedt, was niemand mehr liest.
///
/// Liefert true, wenn etwas ankam — dann lief Seitencode, und der Baum kann
/// sich geaendert haben.
fn pump_fetches() -> bool {
    let Some(sess) = js_session() else { return false };
    let mut landed = false;

    for id in sess.interp.take_aborted_fetches() {
        let jobs = fetch_jobs();
        if let Some(k) = jobs.iter().position(|(n, _)| *n == id) {
            unsafe { npk_http_cancel(jobs[k].1) };
            jobs.remove(k);
        }
    }

    let mut k = 0;
    while k < fetch_jobs().len() {
        let (id, h) = fetch_jobs()[k];
        if unsafe { npk_http_poll(h) } == 0 { k += 1; continue }
        fetch_jobs().remove(k);
        let mut buf: Vec<u8> = Vec::with_capacity(FETCH_CAP);
        let n = unsafe { npk_http_take(h, buf.as_mut_ptr() as i32, FETCH_CAP as i32) };
        landed = true;
        if n < 0 {
            let why = last_error().map(|(a, _)| a).unwrap_or_else(|| String::from("request failed"));
            beak_engine::js::fetch::fetch_failed(&mut sess.interp, id, &why);
            continue;
        }
        unsafe { buf.set_len((n as usize).min(FETCH_CAP)) };
        // Die Kekse gehoeren zu DIESER Antwort, und die Getter, die sie
        // tragen, ueberschreibt das naechste `take`.
        let from = fetched_from().unwrap_or_default();
        file_cookies(&from);
        let status = unsafe { npk_http_status() }.max(0) as u16;
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
        // Der Wirt trennt Kopfzeilen mit `\n`; die Engine haelt den rohen
        // Block, wie er ueber die Leitung geht, also mit `\r\n`.
        let mut hdrs = f.headers.replace("\r\n", "\n").trim_end_matches('\n').to_string();
        // **Kekse fahren mit — bei GLEICHER Herkunft.** Regel K3 des Papiers,
        // und die Engine laesst nur gleiche Herkunft ueberhaupt durch. Ohne
        // sie kann keine angemeldete API-Schicht antworten; mit ihnen ueber
        // eine Herkunftsgrenze waere es die mitreisende Vollmacht, vor der
        // `BROWSER_FETCH_ORIGIN.md` §3.1 warnt.
        let jar = cookies::header_for(&url, unsafe { npk_unix_time() });
        if !jar.is_empty() {
            if !hdrs.is_empty() { hdrs.push('\n'); }
            hdrs.push_str("Cookie: ");
            hdrs.push_str(&jar);
        }
        let body = f.body.clone().unwrap_or_default();
        let h = unsafe {
            npk_http_begin(
                f.method.as_ptr() as i32, f.method.len() as i32,
                url.as_ptr() as i32, url.len() as i32,
                hdrs.as_ptr() as i32, hdrs.len() as i32,
                body.as_ptr() as i32, body.len() as i32,
                FETCH_CAP as i32)
        };
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

/// Eine Runde an den Schriften der Seite. Liefert true, wenn etwas ankam —
/// dann muss neu ausgelegt werden, denn jede Breite aendert sich.
fn pump_fonts(engine: &Engine) -> bool {
    let h = doc().font_job;
    let mut loaded = false;
    if h >= 0 {
        if unsafe { npk_http_poll(h) } == 0 { return false }
        doc_mut().font_job = -1;
        let want = doc_mut().font_want.take().unwrap_or_default();
        let mut buf: Vec<u8> = Vec::with_capacity(FONT_CAP);
        let spans = take_batch(h, buf.as_mut_ptr(), FONT_CAP, want.len());
        let total = spans.iter().map(|(o, l)| o + l).max().unwrap_or(0);
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
                // Kein stilles Weiterlaufen: eine Schrift, die beak nicht
                // lesen kann, ist der Grund, warum die Seite anders aussieht.
                log(&alloc::format!("[beak]   Schrift NICHT LESBAR {url} ({n} B)"));
                bad += 1;
            }
        }
        log(&alloc::format!("[beak] Schriften: {ok} geladen, {bad} gescheitert, {} ms",
                            now_ms() - doc().nav_stage_ms));
    }
    if doc().font_job >= 0 { return loaded }
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

/// `load` zustellen — nach dem ersten Malen, wenn die Kaesten stehen.
///
/// Liefert true, wenn dabei etwas am Baum passiert ist.
fn fire_load(engine: &Engine, page: &Page) -> bool {
    if !doc().load_pending { return false }
    doc_mut().load_pending = false;
    // Was der Benutzer schon getippt hat, muss der Behandler sehen — sonst
    // liest er den Vorgabewert und schreibt ihn womoeglich zurueck.
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

/// Was `ResizeObserver`/`IntersectionObserver` gemessen haben, zustellen.
///
/// Nur wenn wirklich etwas in der Schlange liegt: auf einer Seite ohne
/// Beobachter sind das zwei leere Listen und sonst nichts.
fn pump_box_observers(engine: &Engine) {
    let Some(sess) = js_session() else { return };
    if !sess.interp.box_observations_pending() { return }
    arm_script_budget();
    // `run_timers` faengt mit den Microtasks an, und die Zustellung sitzt
    // dort — ein Rueckruf darf also selbst ein `setTimeout` anlegen und wird
    // in derselben Runde bedient.
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
        // **Der Riegel gegen die Schleife.** Ein Rueckruf, der die Groesse
        // seines eigenen Ziels aendert, misst beim naechsten Bild wieder
        // etwas Neues — das ist der haeufigste Konsolenfehler des Webs
        // ueberhaupt („ResizeObserver loop"). Ohne Riegel liefe hier ein
        // Layout je Bild, und auf dem Geraet sind das fuenf Sekunden je
        // Runde: eine Seite, die haengt.
        //
        // Der Baum wird trotzdem uebernommen — nur das sofortige Neumalen
        // faellt weg. Damit kommt der Kreis zur Ruhe, und die naechste
        // Eingabe zeigt den Stand.
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

/// Wieviele Bilder hintereinander ein Beobachter-Rueckruf den Baum aendern
/// darf, bevor das Neumalen aussetzt.
const OBS_ROUNDS_MAX: u32 = 8;

/// Eine Runde am Modulgraphen: was fehlt noch?
///
/// Liefert true, wenn eine Rundreise laeuft — dann geht es in `nav_pump`
/// weiter. Sonst ist der Graph geschlossen und alles ist ausgewertet.
fn module_pump(engine: &Engine) -> bool {
    let entries = match doc().nav_mod_entries.clone() {
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
    let rounds = doc().nav_mod_rounds;
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
                        now_ms() - doc().nav_stage_ms));
    if ok > 0 {
        decode_css();
        // Die Kaskade muss neu laufen — sonst haengt das Blatt im Puffer und
        // wirkt nicht.
        bump_content_gen("sheet");
        mark_dirty();
    }
    if !sheet_pump(engine) { nav_done(); }
}

/// Welche Schriften die Seite ueber `@font-face` mitbringt.
///
/// **Die Zeile stand hier aus der Zeit VOR `@font-face`** und sagte
/// unbedingt „nicht geladen" — auch nachdem 0.104.0 WOFF2 samt Brotli
/// gebaut hatte. Am Geraet las sich das als Fehler, waehrend zehn Zeilen
/// spaeter `Schriften: 5 geladen, 0 gescheitert` stand: dieselben drei
/// Familien, alle da. Eine Diagnose von damals ist keine Messung von heute
/// ([[feedback_the_named_gap_may_not_be_the_gap]]).
///
/// Sie laeuft VOR dem Holen, also kann sie ueber Erfolg gar nichts wissen.
/// Was sie sagen darf, ist was die Seite VERLANGT; das Urteil kommt aus
/// `pump_fonts` — dort steht je Datei `FEHLT` oder `NICHT LESBAR`, und
/// darunter die Summe.
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
    let want = doc_mut().nav_mod_want.take().unwrap_or_default();
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

/// Zeitgeber, Kekse, Baum und der Bericht — nach ALLEM, was die Seite an
/// Code hat: gewoehnliche Skripte wie Module.
fn finish_scripts(engine: &Engine) {
    let (ran, failed, bytes) = doc().script_tally;
    let t0 = doc().script_t0;
    let Some(sess) = js_session() else { return };
    // **`DOMContentLoaded` und `load`.**
    //
    // Beide wurden NIE zugestellt. Die Eigenschaften gab es seit langem
    // (`window.onload`, `document.ondomcontentloaded`), das Ereignis nicht —
    // und `document.readyState` sagte trotzdem „complete". Eine Seite, die
    // ihre Oberflaeche in `window.addEventListener("load", …)` baut, wartete
    // damit fuer immer, ohne Fehler und ohne Meldung. Das ist kein
    // Randmerkmal: es ist eine der beiden ueblichen Arten, ueberhaupt
    // anzufangen.
    //
    // Die Reihenfolge ist die der Spezifikation: erst `DOMContentLoaded`
    // (Dokument steht, aufgeschobene Skripte und Module sind gelaufen), dann
    // `load` (auch die nachgeladenen Blaetter sind da). Beide gehen an den
    // DOKUMENTknoten — `window.addEventListener` landet dort
    // (`target_node`), und ein Behandler am `document` bekommt sie durch das
    // Blubbern ebenfalls.
    // `load` selbst faellt aber NICHT hier, sondern nach dem ersten Malen:
    // die Kaestchengeometrie reicht der Wirt erst dort ein
    // (`Interp::set_geometry`), und ein Behandler, der misst, bekaeme sonst
    // ueberall Nullen — eine Zahl, die aussieht wie eine Messung.
    let doc_node = sess.interp.doc.as_ref().map(|d| d.doc);
    if let Some(dn) = doc_node {
        let _ = beak_engine::js::dombind::dispatch(&mut sess.interp, "DOMContentLoaded", &[dn]);
    }
    doc_mut().load_pending = true;
    // Die Zeitgeber, die waehrend des Ladens angemeldet wurden, einmal
    // laufen lassen — viele Seiten stellen ihre Oberflaeche in einem
    // `setTimeout(…, 0)` fertig.
    let timers = sess.interp.run_timers();
    sync_cookies(sess);
    sync_history(engine, sess);
    sync_scroll(sess);
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
    let nodes: Vec<u32> = chain.iter().filter_map(|s| doc.by_seq(*s)).collect();
    if nodes.is_empty() { return false; }
    // **Ein Schild aktiviert sein Kaestchen auch OHNE Skript.** Der Schnellweg
    // hier springt ab, wenn die Seite keinen Behandler hat — richtig fuer
    // Ereignisse, falsch fuer eingebautes Verhalten: die Klickflaeche eines
    // Kaestchens ist der Text daneben, und eine Seite ganz ohne JS hat ihn
    // genauso.
    let on_label = nodes.last().is_some_and(|n| {
        beak_engine::js::dombind::label_target(&sess.interp, *n).is_some()
    });
    if !doc.has_listeners && !on_label { return false; }
    let t0 = now_ms();
    let prevented = matches!(
        beak_engine::js::dombind::dispatch(&mut sess.interp, "click", &nodes), Ok(true));
    let timers = sess.interp.run_timers();
    sync_cookies(sess);
    sync_history(engine, sess);
    sync_scroll(sess);
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
    // Nach einem Behandler: der Baum weiss jetzt mehr als der Wirt.
    page.sync(engine);
    pull_control_values(page, sess);
    let submits = sess.interp.take_submits();
    drain_console(sess);
    for seq in submits {
        log(&alloc::format!("[beak] script submit: form seq={seq}"));
        if submit_form_seq(engine, page, seq) { return true }
    }
    // Ein Behandler, der `location.href` setzt, hat damit gesagt, wohin es
    // geht. Danach auch noch dem angeklickten Link zu folgen hiesse, zwei
    // Navigationen aus einem Klick zu machen — also gilt der Klick als
    // behandelt.
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

/// Zufall aus dem Kernel in einen Puffer. `false`, wenn der Kernel abgelehnt
/// hat — der Rufer wirft dann, statt schwachen Zufall zu liefern.
///
/// In Stuecken, weil der Kernel 64 KiB je Aufruf deckelt (er haelt dabei den
/// RNG-Mutex). Der Motor deckelt ohnehin bei derselben Zahl; die Schleife
/// steht hier, damit dieselbe Funktion auch einen groesseren Puffer bedienen
/// koennte, ohne still die Haelfte ungefuellt zu lassen.
fn random_bytes(out: &mut [u8]) -> bool {
    for teil in out.chunks_mut(64 * 1024) {
        // SAFETY: der Kernel schreibt hoechstens `len` Bytes ab `ptr`, und
        // beides beschreibt genau dieses Stueck.
        let n = unsafe { npk_random_bytes(teil.as_mut_ptr() as i32, teil.len() as i32) };
        if n < 0 || n as usize != teil.len() { return false }
    }
    true
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
/// **Erhoeht von 360 auf 900 s.** Die Fritzbox-Anmeldung misst am Geraet
/// ~270 s — bei 360 s waren das 33 % Luft, und eine Maschine unter Last oder
/// mit kaltem Zwischenspeicher fiel darueber. Ein Abbruch sieht dann aus wie
/// „falsches Kennwort", obwohl nur die Uhr abgelaufen war. Gegen eine echte
/// Endlosschleife hilft jetzt der HERZSCHLAG: er sagt jede Sekunde, dass noch
/// gerechnet wird, statt den Benutzer raten zu lassen.
const SCRIPT_BUDGET_MS: i64 = 900_000;

/// Ab wann ein Lauf im Log auffaellt. Ein Skript, das Minuten rechnet, ist
/// kein Fehler — aber es ist der Grund, warum nichts passiert, und das
/// gehoert gesagt, statt es aus einem Zeitstempel raten zu lassen.
const SCRIPT_SLOW_MS: i64 = 3_000;

/// Wie oft ein langer Lauf von sich hoeren laesst.
const SCRIPT_HEARTBEAT_MS: i64 = 5_000;

/// **Der Deckel gehoert dem LAUF, nicht dem Dokument.**
///
/// Es laeuft immer genau ein Stueck Seitencode — die Pumpe ist einfaedig, und
/// daran aendern auch mehrere Tabs nichts. Diese drei beschreiben den Lauf,
/// der gerade auf dem Stapel liegt, und `arm_script_budget` stellt sie vor
/// jedem neu. In `Doc` waeren sie ein Feld, das je Seite dasselbe sagt.
///
/// Der zweite Grund ist handfester: `script_time_left` ruft die Engine aus
/// dem laufenden Interpreter heraus, und der ist ueber `js_session()` bereits
/// eine `&mut`-Entleihung aus dem Dokument. Ein `doc()` daneben waere genau
/// das Aliasing, vor dem der Kommentar an `doc`/`doc_mut` warnt.
static mut SCRIPT_DEADLINE: i64 = 0;
/// Wann der laufende Behandler begann — fuer den Herzschlag. Eigener Name
/// neben `Doc::script_t0`: das ist der Beginn der SEITENrunde, nicht des Laufs.
static mut BUDGET_T0: i64 = 0;
static mut BUDGET_SAID: i64 = 0;

/// Die Uhr, die die Engine alle 65 536 Schritte fragt — und der HERZSCHLAG.
///
/// **Ein Bildschirm, der Minuten stillsteht, ohne dass irgendwo etwas steht,
/// ist ein Fehler, auch wenn das Rechnen keiner ist.** Florians Befund:
/// „beim Absenden wird nichts geloggt, es bleibt einfach stehen." Die
/// Meldung kam erst NACH dem Behandler, also nach vier Minuten — blind fuer
/// genau das, was lange dauert ([[feedback_report_before_not_after]]).
///
/// Die Stelle ist die richtige: die Engine fragt hier ohnehin schon, und
/// oefter als jede Sekunde kommt sie nicht vorbei.
fn script_time_left() -> bool {
    let now = now_ms();
    let t0 = unsafe { core::ptr::addr_of!(BUDGET_T0).read() };
    let said = unsafe { core::ptr::addr_of!(BUDGET_SAID).read() };
    let el = now - t0;
    if el >= SCRIPT_SLOW_MS && el - said >= SCRIPT_HEARTBEAT_MS {
        unsafe { core::ptr::addr_of_mut!(BUDGET_SAID).write(el) };
        let mut m = String::from("[beak] Skript rechnet noch: ");
        push_i64(&mut m, el / 1000);
        m.push_str(" s von hoechstens ");
        push_i64(&mut m, SCRIPT_BUDGET_MS / 1000);
        m.push_str(" s");
        log(&m);
    }
    now < unsafe { core::ptr::addr_of!(SCRIPT_DEADLINE).read() }
}

/// Die Uhr neu stellen — vor jedem Lauf von Seitencode.
fn arm_script_budget() {
    let now = now_ms();
    unsafe {
        core::ptr::addr_of_mut!(SCRIPT_DEADLINE).write(now + SCRIPT_BUDGET_MS);
        core::ptr::addr_of_mut!(BUDGET_T0).write(now);
        core::ptr::addr_of_mut!(BUDGET_SAID).write(0);
    }
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
    doc_mut().images_dirty = false;
    // Der Motor haelt die Hervorhebungen; eine neue Seite hat keine.
    // `set_url` raeumt sie im Dokument weg, hier faellt der Anstrich nach.
    engine.set_marks(None, Vec::new());
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
    let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
    let spans = take_batch(handle, dst, IMG_FETCH_CAP, want.len());
    let mut arrived: Vec<&str> = Vec::new();
    let mut moved = false;
    for ((src, url), (off, n)) in want.iter().zip(spans) {
        if n == 0 {
            // Nicht stillschweigend: eine Anfrage, die scheiterte, und eine,
            // deren Antwort nicht in den Puffer passte, sehen hier gleich aus
            // — und die zweite ist ein Deckel von UNS.
            log(&alloc::format!("[beak] image not delivered (request failed or over {} KiB buffer) — {}",
                IMG_FETCH_CAP / 1024, src));
            continue;
        }
        let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };
        // Decode now, drop the compressed bytes — and keep the pixels under
        // their url so the next navigation to this page needs neither.
        if let Err(why) = engine.add_image_cached(src, url, bytes) {
            // Der Grund gehoert IN die Meldung. „nicht dekodierbar oder ueber
            // dem Budget" schickte eine ganze Sitzung hinter einen
            // JPEG-Dekoder her, der nie schuld war — es war das Budget.
            log(&alloc::format!("[beak] image dropped ({n} B): {why} — {src}"));
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
    doc().images_dirty
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
    let dst = core::ptr::addr_of_mut!(IMG_FETCH_BUF) as *mut u8;
    let spans = take_batch(handle, dst, IMG_FETCH_CAP, want.len());
    let mut arrived: Vec<u64> = Vec::new();
    for ((key, url), (off, n)) in want.iter().zip(spans) {
        if n == 0 {
            log(&alloc::format!("[beak] background not delivered (request failed or over {} KiB buffer) — {}",
                IMG_FETCH_CAP / 1024, url));
            continue; // the box stays undecorated
        }
        let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };
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

/// The `<img>` batch on the wire and the `(src, resolved url)` pairs it was
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
        unsafe { npk_http_cancel(doc().img_job) };
        doc_mut().img_job = -1;
    }
    doc_mut().img_job_srcs = None;
    if doc().cssimg_job >= 0 {
        unsafe { npk_http_cancel(doc().cssimg_job) };
        doc_mut().cssimg_job = -1;
    }
    doc_mut().cssimg_job_keys = None;
    // Und was noch gar nicht gefragt wurde. **Die Schlange gehoert der Seite,
    // die ersetzt wird**: sie stehen zu lassen hiesse, dass der neue Abruf
    // sich die eine Kernel-Schlange mit den Bildern der alten Seite teilt —
    // und ihre relativen Adressen wuerden gegen die NEUE Basis aufgeloest.
    let d = doc_mut();
    d.pending_imgs.clear();
    d.pending_css_imgs.clear();
    d.css_asked.clear();
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
    if let Some(abs) = typed_to_url(typed) {
        nav_goto(engine, &abs);
    }
}

/// Was die Adresszeile MEINT: eine Adresse, oder eine Suche daraus.
/// `None` heisst „nichts eingegeben".
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

/// Womit ein Tab beschriftet wird: der Titel, sonst der WIRT der Adresse,
/// sonst „Neuer Tab".
///
/// Der Wirt und nicht die ganze Adresse. Ein Etikett wird hinten
/// abgeschnitten, und bei Adressen ist vorne alles gleich — fuenf Tabs auf
/// Wikipedia waeren fuenfmal `https://de.wikipedia.org/wi…`.
fn tab_label(d: &Doc) -> String {
    let full: &str = if !d.title.is_empty() {
        &d.title
    } else if d.url.is_empty() {
        "Neuer Tab"
    } else {
        let after = d.url.split_once("://").map(|(_, r)| r).unwrap_or(&d.url);
        after.split('/').next().unwrap_or(after)
    };
    // **Selbst kuerzen, denn sonst tut es niemand.** Der Compositor malt
    // Text vom linken Rand seines Kastens aus und klemmt ihn nicht:
    // `MaxWidth` begrenzt die Kiste, nicht die Glyphen. Was nicht
    // hineinpasst, steht im NACHBARtab.
    if full.chars().count() <= TAB_CHARS {
        return full.to_string();
    }
    let mut out: String = full.chars().take(TAB_CHARS - 1).collect();
    out.push('\u{2026}');
    out
}

/// Einen Tab einfrieren: die Griffe zurueckgeben, den Rest fallen lassen.
///
/// **Ein eingefrorener Tab IST seine Wiederherstellungsbeschreibung.** Statt
/// aufzuzaehlen, was alles weg muss — und beim naechsten neuen Feld eines zu
/// vergessen —, wird das Dokument auf ein frisches gesetzt und nur
/// zurueckgeschrieben, was ihn wiederherstellt. Genau die Klasse Fehler, die
/// `Doc` abgeschafft hat („navigate() muss an zwanzig Statics denken"), waere
/// hier sonst zurueck.
///
/// Der grosse Posten dabei ist der JS-Realm: ~973 KB je Sitzung, plus ihr
/// Baum. Was bleibt, sind Kilobytes.
fn tab_freeze() {
    // Zuerst die Griffe: sie gehoeren dem Wirt, nicht dem Speicher, den wir
    // gleich fallen lassen. Ein Stapel, der weiterlaeuft, nimmt der Seite,
    // auf die gewechselt wird, die eine Abrufschlange weg.
    nav_cancel();
    subresources_cancel();
    {
        let d = doc_mut();
        if d.font_job >= 0 {
            unsafe { npk_http_cancel(d.font_job) };
        }
        for (_, h) in core::mem::take(&mut d.fetch_jobs) {
            unsafe { npk_http_cancel(h) };
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
    // Wo der Leser stand. `scroll_y` waere die falsche Stelle: das Laden
    // setzt sie auf 0, und zwar zu Recht — eine neue Seite faengt oben an.
    d.scroll_want = y;
}

/// Ein leerer Tab.
///
/// **Die Puffer muessen wirklich leer werden.** `HTML_BUF`/`CSS_BUF` gehoeren
/// dem Programm, nicht der Seite; ein neuer Tab, der den Text des alten
/// zeigt, waere genau die Verwechslung, gegen die `Doc` gebaut wurde.
fn blank_document(engine: &Engine) {
    unsafe {
        core::ptr::addr_of_mut!(HTML_LEN).write(0);
        core::ptr::addr_of_mut!(CSS_LEN).write(0);
    }
    engine.set_scripted_dom(None);
    engine.set_hit_all(false);
    bump_content_gen("tab-blank");
    mark_dirty();
}

/// Die Seite des laufenden Tabs holen — der Weg zurueck aus dem Einfrieren.
///
/// Ein Tab ohne Verlauf legt seinen ersten Eintrag an; ein zurueckkehrender
/// nicht, denn er steht schon darin.
fn tab_load(engine: &Engine, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) {
    // Der Zwischenspeicher haelt das Layout der vorigen Seite, das
    // Formularmodell ihre Steuerelemente. Beide haengen an Zaehlern, die JE
    // DOKUMENT laufen — nach einem Wechsel sagt ein Vergleich nichts mehr.
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

/// Auf einen anderen Tab umschalten.
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

/// Einen Tab oeffnen. `background` legt ihn nur AN.
///
/// Ein Hintergrundtab holt nichts: geholt wird, wenn ihn jemand ansieht. Der
/// Grund steht im Kernel — `WORKER_COUNT = 1`, es laeuft ein Abruf zur Zeit,
/// und ein Tab, den niemand liest, nimmt der Seite vor den Augen des Lesers
/// die Leitung weg.
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

/// Einen Tab schliessen.
fn tab_close(engine: &Engine, i: usize, cache: &mut Option<(Layout, i32, i32, u32)>,
             page: &mut Page) {
    let n = tabs().len();
    if i >= n {
        return;
    }
    // **Der letzte Tab IST das Fenster.** So macht es jeder Browser, und die
    // Gegenrichtung waere ein Strg+W, das nichts tut — also ein Fenster, das
    // sich mit der Tastatur nicht schliessen laesst.
    if n == 1 {
        unsafe {
            let _ = npk_close_widget();
        }
        return;
    }
    let a = active();
    if i == a {
        // Die Griffe zurueck, bevor das Dokument faellt.
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
        // VOR dem Behandler. Er kann Minuten rechnen (eine Anmeldung, die
        // ihren Hash selbst macht), und bis dahin sah der Benutzer nichts —
        // weder dass die Eingabe angekommen ist noch dass etwas laeuft.
        let ts0 = now_ms();
        log("[beak] submit: Behandler der Seite laeuft…");
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
        let mut m = String::from("[beak] submit: Behandler fertig nach ");
        push_i64(&mut m, now_ms() - ts0);
        m.push_str(" ms");
        log(&m);
    }
    let sub = match forms::submit(&page.forms, &page.state, activated) {
        Some(s) => s,
        // Silence here reads as "the button is dead". It is not the same
        // thing as a failed request, and the difference is the whole
        // diagnosis: a button whose form we never resolved (nested outside
        // it, or owned by a `form=` attribute we do not read) versus a form
        // that submitted and came back wrong.
        //
        // **Ein Absendeknopf OHNE Formular tut laut HTML §4.10.6 nichts** —
        // das ist die Regel, kein Fehler. Auf einer Anwendungsseite ist jeder
        // `<button>` ohne `type` ein solcher Knopf, und die Meldung stand dort
        // bei JEDEM Klick, als waere etwas kaputt.
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

fn hist_get(i: usize) -> &'static str {
    doc().hist.get(i).map(|s| s.as_str()).unwrap_or("")
}
/// Record a new navigation: truncate forward entries, append (caps at HIST_MAX).
///
/// **Verhalten unveraendert uebernommen**, auch die Ecke am Deckel: ist die
/// Liste voll, wird der LETZTE Eintrag ueberschrieben und die Stelle bleibt
/// stehen. Ein Umbau ist der falsche Ort, um nebenbei eine Regel zu aendern
/// — was hier steht, muss sich genauso verhalten wie vorher, sonst misst
/// kein Test mehr den Umbau.
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
    // Vorwaerts-Eintraege fallen weg — dasselbe, was das feste Feld tat,
    // indem es `HIST_COUNT` auf `new_pos + 1` zurueckschrieb.
    d.hist.truncate(new_pos);
    d.hist.push(String::from(clip(url, URL_CAP)));
    d.hist_pos = new_pos;
}
// **Jede Entleihung endet in ihrer eigenen Anweisung.** `doc_mut` gibt ein
// `&'static mut` heraus; zwei davon gleichzeitig — oder eines neben einem
// `doc()` — sind Aliasing, und Aliasing auf `&mut` ist kein Stilfehler,
// sondern undefiniert. Deshalb steht hier `doc().hist_pos` lesen, DANN
// schreiben, DANN `hist_get` rufen, statt eine Referenz ueber alles drei zu
// halten. Der Rechner merkt es nicht — die Referenz kommt aus einem
// `unsafe`-Deref und faellt aus seiner Buchhaltung.
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
    let dirty = doc().dirty;
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
        // Der Titel fuer den Streifen. **Hier und nicht beim Ankommen des
        // Dokuments:** der Motor parst beim AUSLEGEN, also haelt er bis
        // hierher noch den Baum der vorigen Seite — ein Tab haette den Namen
        // der Seite getragen, von der man kam.
        let t = engine.title().unwrap_or_default();
        if t != doc().title {
            doc_mut().title = t;
            render_chrome();
        }
        // Die Kaesten neu einsammeln — nur hier, nicht je Bild.
        let boxes = cache.as_ref().unwrap().0.element_rects();
        // Zuweisung, nicht `ptr::write`: die ueberschreibt OHNE den alten Wert
        // fallen zu lassen, und das waren ~180 KB Kaesten je Neuauslegung, die
        // nie zurueckkamen.
        doc_mut().geom = Some(alloc::rc::Rc::new(boxes));

        // **Eine Markierung zeigt auf BEFEHLSINDIZES, und die verschieben
        // sich beim Neuauslegen.** Ein nachgeladenes Bild reicht: aus der
        // markierten Zeile wird eine andere, und der Schleier liegt ueber
        // fremdem Text. Also faellt die Auswahl weg — das ist ehrlicher, als
        // etwas Falsches hervorzuheben.
        //
        // Die SUCHE dagegen wird neu gerechnet statt weggeworfen: sie hat
        // eine Frage, die noch gilt (die Zeichenkette), waehrend eine
        // Auswahl nur einen Ort hatte. Ohne Sprung — sonst reisst ein
        // nachgeladenes Bild die Seite unter dem Leser weg.
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
    // Geometrie und Rollstand an die Maschine reichen. Der Rollstand geht bei
    // jedem Bild mit, weil er sich ohne Layout aendert; die Kaesten sind ein
    // `Rc` und kosten dabei nichts.
    let geom = doc().geom.clone();
    if let (Some(sess), Some(g)) = (js_session(), geom) {
        // Das Sichtfeld nachziehen, wenn es sich bewegt hat. Der Ausschnitt
        // eines `IntersectionObserver` ohne eigene Wurzel IST das Sichtfeld —
        // und `set_media` laeuft nur einmal beim Skriptstart, also stand hier
        // nach jeder Fensteraenderung die Zahl von damals. Nur bei
        // Aenderung, weil `set_viewport` ein frisches `screen` baut und das
        // je Bild Muell waere.
        let vp = (w, h);
        if doc().last_vp != vp {
            doc_mut().last_vp = vp;
            sess.interp.set_viewport(w as f64, h as f64);
        }
        sess.interp.set_geometry(beak_engine::js::interp::Geometry {
            boxes: g, scroll: (0, sy),
            // Die Rollflaeche, wie das Layout sie ausgerechnet hat — dieselbe
            // Zahl, gegen die der Wirt zwei Zeilen weiter oben `max_scroll`
            // klemmt. `document.documentElement.scrollHeight` MUSS dieselbe
            // sagen, sonst rechnet eine Seite mit einer Hoehe, an die sie nie
            // rollen kann.
            content: (w, layout.height as i32),
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
    let full = doc().need_full
        || need_layout
        || resized
        || inspect_mode()
        || dy.abs() >= h;
    // A scroll that the clamp swallowed — at the top or the bottom of the page
    // the offset does not move, so the buffer already holds this exact frame.
    // Repainting it was 60-80 ms for a picture that cannot differ, and holding
    // the wheel at the foot of an article does it every turn.
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
    {
        if !doc().nav_reported && !nav_busy() {
            doc_mut().nav_reported = true;
            log_ms("=== navigation -> first paint", now_ms() - doc().nav_start_ms);
            // Wieviele der sechs eingebauten Gesichter diese Seite wirklich
            // gebraucht hat. Sie werden faul geladen, und ohne diese Zahl ist
            // „faul" eine Behauptung: eine Seite, die doch alle sechs
            // anfasst, spart nichts, und man saehe es nicht.
            let mut m = String::from("[beak] Schriften: ");
            push_i64(&mut m, engine.loaded_faces() as i64);
            m.push_str(" von 6 Gesichtern geparst");
            log(&m);
        }
    }

    unsafe {
        core::ptr::addr_of_mut!(LAST_W).write(w);
        core::ptr::addr_of_mut!(LAST_H).write(h);
        core::ptr::addr_of_mut!(LAST_SY).write(sy);
    }
    doc_mut().need_full = false;
    doc_mut().dirty = false;
}

/// Commit the loft-styled chrome: menu bar · toolbar (back/forward/reload +
/// Die Farb-Laeufe der Adresszeile: die registrierbare Domain in voller
/// Staerke, alles andere abgeblendet.
///
/// **Das ist keine Zierde, sondern die Anti-Phishing-Anzeige.** In
/// `https://paypal.com.betrug.ru/login` heisst die Domain `betrug.ru`, und
/// das Auge liest das erste, was wie ein Name aussieht. Jeder Browser hebt
/// deshalb genau diesen Teil hervor.
///
/// Gerechnet wird mit `site::registrable_domain`, also mit der echten Public
/// Suffix List — NICHT mit „die letzten zwei Bestandteile". Der Unterschied
/// ist der ganze Punkt: bei `a.github.io` waeren das `github.io`, und dann
/// haette die Anzeige zwei fremde Nutzerseiten als dieselbe ausgewiesen.
///
/// **Nur wenn das Feld die geladene Adresse ZEIGT.** Weicht es ab, tippt
/// gerade jemand, und dann ist jede Hervorhebung eine Aussage ueber einen
/// halben Satz: `arcade.c` waere `arcade.c`, eine Zehntelsekunde spaeter
/// `arcade.ch`. Waehrend des Tippens bleibt der Text einfarbig.
fn address_spans(field: &str, url: &str) -> Vec<Span> {
    if field.is_empty() || field != url {
        return Vec::new();
    }
    // Der Host: hinter `schema://`, bis zum ersten `/?#`, ohne `benutzer@`
    // und ohne `:port`.
    let after_scheme = match field.find("://") { Some(i) => i + 3, None => 0 };
    let rest = &field[after_scheme..];
    let host_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..host_len];
    let host_at = after_scheme + authority.rfind('@').map(|i| i + 1).unwrap_or(0);
    let host_raw = &field[host_at..after_scheme + host_len];
    // Ein Doppelpunkt trennt den Port — aber in `[::1]` gehoert er zur
    // Adresse, und eine IP hat ohnehin keine registrierbare Domain.
    let host = match host_raw.rfind(':') {
        Some(i) if !host_raw.contains(']') => &host_raw[..i],
        _ => host_raw,
    };
    let Some(dom) = beak_engine::site::registrable_domain(host) else { return Vec::new() };
    // `registrable_domain` gibt kleingeschrieben zurueck; gesucht wird im
    // ORIGINAL, und sie ist immer ein Endstueck des Hosts.
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
    // Das Schloss gehoert dem GELADENEN Dokument, der Text dem Feld: waehrend
    // jemand tippt, sagt das Schloss weiter die Wahrheit ueber die Seite, die
    // dasteht.
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
            id: CanvasId(CANVAS_ID as u32),
            width: 800,
            height: 600,
            modifiers: vec![Modifier::Flex(1), Modifier::Background(Token::Page)],
        },
    ];

    // Die Suchleiste. **Kein `Widget::Input`, sondern Text** — der Puffer
    // gehoert beak, siehe `Doc::find`. Sie steht UNTER der Leinwand wie in
    // jedem Browser: oben waere sie ein Werkzeug, unten ist sie eine
    // Randnotiz, und genau das ist sie.
    if let Some(q) = doc().find.clone() {
        let (at, n) = (doc().find_at, doc().found.len());
        let mut label = String::from("Suchen: ");
        label.push_str(&q);
        label.push('\u{2502}');           // ein stehender Strich als Schreibmarke
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

/// Der Streifen: 36 px, `SurfaceElevated`, Tabs unten buendig
/// (`docs/spec/UI_REFRESH.md` §5.2 und §3 `tab`).
const TABSTRIP_H: u16 = 36;
/// Der Akzentstreifen oben auf dem aktiven Tab.
const TAB_ACCENT: u16 = 2;
const TAB_H: u16 = 30;
/// **Feste Breite, nicht mitwachsend** (§3 `tab`). Ein Tab, der sich der
/// Anzahl anpasst, braucht ein Etikett, das mitgeht — und die Schrift gehoert
/// dem Compositor, eine App kann sie nicht messen.
const TAB_W: u16 = 160;
/// **22, damit ein 16-px-Zeichen hineinpasst.** Der Atlas fuehrt 16, 24, 32,
/// 48 und 64 — eine Anfrage auf 12 bekam das 16er und wurde auf 4:3
/// verkleinert. Das Verkleinern mittelt korrekt ueber Flaechen, und genau
/// deshalb wird ein 1,5 px breiter Strich dabei weich: Florian am Geraet,
/// „das x symbol malt bisschen unscharf". Eine Atlasgroesse zu verlangen ist
/// ein 1:1-Blit und damit scharf.
const TAB_BTN: u16 = 22;
/// Wieviel Text in einen Tab passt, in Zeichen.
///
/// Gerechnet mit 7 px je Zeichen gegen `TextStyle::Body` (13 px). Das ist
/// eine SCHAETZUNG und absichtlich zu hoch: ein zu kurzes Etikett ist ein
/// Etikett, ein zu langes ist ein Fehler — der Compositor schneidet Text
/// nicht ab, er malt ihn ueber den Nachbarn.
const TAB_CHARS: usize = ((TAB_W - 16 - TAB_BTN - 4) / 7) as usize;

/// Ein kleiner Knopf im Streifen: das `\u{d7}` eines Tabs, das `+` dahinter.
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

/// Der Tabstreifen.
///
/// **Er steht immer da, auch bei einem Tab** — das `+` ist der einzige Ort,
/// an dem ein zweiter entsteht, wenn man die Tastenfolge nicht kennt. 36 px
/// dafuer sind der Preis, den jeder Browser zahlt.
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
        // **Der aktive Tab traegt die Farbe des Inhalts darunter** (§3
        // `tab`): er IST die Seite, der Streifen ist der Rahmen.
        if sel {
            m.push(Modifier::Background(Token::Surface));
        } else {
            m.push(Modifier::Hover(vec![
                Modifier::Background(Token::SurfaceMuted),
                Modifier::Rounded(Radius::Md.as_u8()),
            ]));
        }
        // **Ein Akzentstreifen OBEN auf dem aktiven Tab.** Bis hierher war der
        // Unterschied zwischen aktiv und ruhend `Surface` gegen
        // `SurfaceElevated` plus eine Textfarbe — richtig, aber am Geraet zu
        // leise (Florian: „tabs optisch noch bisschen mehr hervorheben").
        // Der Streifen ist das uebliche Zeichen und das einzige, das auch aus
        // zwei Metern liest. Er liegt IM Tab, nicht darueber: sonst
        // verschoebe er die Beschriftung des aktiven gegen die der anderen.
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
        // **`Stretch`, nicht `Start`.** In einer Spalte ist `align` die
        // QUERachse, also die Breite: mit `Start` haette der Akzentstreifen
        // seine natuerliche Breite bekommen — und die ist bei einer Zeile
        // ohne Kinder null.
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
        align: Align::End,          // unten buendig
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

// ── Suchen in der Seite ───────────────────────────────────────────────────

/// Die Suche neu rechnen und die Fundstellen hervorheben.
///
/// `jump` springt zur aktuellen Fundstelle. Beim Tippen ist das erwuenscht
/// (man will sehen, ob es sie gibt), beim blossen Neuauslegen nicht — sonst
/// reisst ein nachgeladenes Bild die Seite unter dem Leser weg.
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
    // Hinscrollen, damit die Fundstelle im Blick ist — ein Drittel von oben,
    // nicht am Rand: eine Fundstelle in der letzten Zeile liest sich nicht.
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

/// Eine Fundstelle weiter (oder zurueck), rundherum.
fn find_step(engine: &Engine, cache: &Option<(Layout, i32, i32, u32)>, back: bool) {
    let n = doc().found.len();
    if n == 0 { return }
    let d = doc_mut();
    d.find_at = if back { (d.find_at + n - 1) % n } else { (d.find_at + 1) % n };
    find_run(engine, cache, true);
}

/// Die Leiste schliessen und alles aufraeumen.
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

/// Ein Tastenbyte zu einem ZEICHEN sammeln.
///
/// **Ein Tastendruck traegt ein Byte, und `ä` sind zwei.** Alles ab 0x80 ist
/// ein Stueck einer UTF-8-Folge; `b as char` waere dort LATIN-1 und machte
/// aus dem Fuehrungsbyte 0xC3 ein `Ã`. `None` heisst „die Folge ist noch
/// nicht vollstaendig" — dann passiert nichts, auch kein Neumalen.
///
/// Eine Stelle, nicht drei: die Adresszeile bedient der Compositor, aber
/// beak hat zwei eigene Eingaben (Seitenformulare und die Suchleiste), und
/// zwei Kopien derselben Rechnung sind eine wartende zweite Semantik
/// ([[feedback_a_copy_is_a_second_semantics_waiting]]).
fn utf8_feed(pending: &mut [u8; 4], len: &mut u8, b: u8) -> Option<char> {
    if b < 0x80 {
        *len = 0;
        return Some(b as char);
    }
    if b >= 0xC0 { *len = 0; }              // neue Folge
    let n = *len as usize;
    if n < 4 { pending[n] = b; *len = n as u8 + 1; } else { *len = 0; }
    let take = *len as usize;
    match core::str::from_utf8(&pending[..take]).ok().and_then(|t| t.chars().next()) {
        Some(ch) => { *len = 0; Some(ch) }
        None => None,
    }
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
        // **Nicht mehr nur ASCII.** `0x20..0x7F` hiess woertlich: auf einer
        // Deutschschweizer Tastatur laesst sich kein `ä` in ein Formular
        // tippen — nicht in ein Suchfeld, nicht in ein Anmeldefeld. Alles ab
        // 0x80 ist ein Stueck einer UTF-8-Folge; `b as char` waere dort
        // LATIN-1 und machte aus dem Fuehrungsbyte 0xC3 ein `Ã`.
        KeyCode::Char(b) if b >= 0x20 && b != 0x7F => {
            match utf8_feed(&mut page.state.pending, &mut page.state.pending_len, b) {
                Some(ch) => { value.insert(caret, ch); caret += ch.len_utf8(); }
                // Folge noch nicht vollstaendig: nichts tun, nichts malen.
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

fn say_ctl_bail_once(why: &str) {
    if doc().ctl_bail_said || why.is_empty() {
        return;
    }
    doc_mut().ctl_bail_said = true;
    log(&alloc::format!("[beak] Steuerelement neu malen geht nicht: {why}"));
}

fn handle(engine: &Engine, ev: Event, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) -> bool {
    let nav = nav_gen();
    let chrome = handle_event(engine, ev, cache, page);
    chrome || nav_gen() != nav
}

fn handle_event(engine: &Engine, ev: Event, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) -> bool {
    match ev {
        // Nur das Textfeld. NICHT `set_url`: das meldet dem Kernel den
        // Netzkontext, und der loest dafuer auf — ein Tastendruck ist keine
        // Navigation ([[feedback_a_keystroke_is_not_a_navigation]]).
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
        // **Strg+F.** Kommt als Chord, weil `Event::Key` keine Umschalter
        // traegt — sonst waere Strg+F von einem getippten „f" nicht zu
        // unterscheiden.
        Event::Chord { letter: b'f', .. } => {
            if doc().find.is_none() { doc_mut().find = Some(String::new()); }
            doc_mut().find_pending_len = 0;
            render_chrome();
            mark_dirty();
            true
        }
        // **Strg+T / Strg+W / Strg+1..9.** Strg+Tab gibt es NICHT, und das ist
        // keine Nachlaessigkeit: `Event::Chord` traegt einen Buchstaben, und
        // der Kernel baut ihn aus `KeyCode::Char` — `KeyCode::Tab` ist kein
        // `Char` und kommt gar nicht erst an. Ziffern schon (jede druckbare
        // Taste), also sind die Zahlen der Weg zu einem bestimmten Tab.
        Event::Chord { letter: b't', .. } => {
            tab_open(engine, "", false, cache, page);
            true
        }
        Event::Chord { letter: b'w', .. } => {
            tab_close(engine, active(), cache, page);
            true
        }
        // `9` ist der LETZTE, nicht der neunte — so macht es jeder Browser,
        // und bei zwanzig Tabs ist „der letzte" die einzige Zahl, die man
        // ohne Zaehlen trifft.
        Event::Chord { letter: b'9', .. } => {
            tab_activate(engine, tabs().len() - 1, cache, page);
            true
        }
        Event::Chord { letter: d @ b'1'..=b'8', .. } => {
            tab_activate(engine, (d - b'1') as usize, cache, page);
            true
        }
        // Die Suchleiste ist offen: die Tasten gehoeren IHR. Sie kommen nur
        // hierher, wenn kein Textfeld des Compositors den Fokus hat — wer in
        // die Adresszeile klickt, tippt dort weiter, und das ist richtig so.
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
                    let Some(ch) = ch else { return true };   // Folge unvollstaendig
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
                // Was in der Zeile STEHT, nicht wo wir sind.
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
                unsafe {
                    let _ = npk_close_widget();
                }
                true
            }
            ACT_TAB_NEW => {
                tab_open(engine, "", false, cache, page);
                set_open_menu(0);
                true
            }
            // Zwei Baender, kein Feld je Tab: der Streifen wird bei jeder
            // Aenderung neu gebaut, also IST der Index der Tab.
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
                    // **Ein Link wird beim LOSLASSEN gefolgt, nicht beim
                    // Druecken.** So macht es jeder Browser, und es ist die
                    // Bedingung dafuer, dass sich Text markieren laesst, der
                    // auf einem Link ANFAENGT — bis hierher navigierte der
                    // Druck, bevor das Ziehen ueberhaupt begann.
                    //
                    // Nur der Link wandert; Skript, Steuerelemente und
                    // `<summary>` bleiben beim Druecken. Den ganzen Klickweg
                    // umzustellen ist ein eigener Schritt, und dieser hier
                    // soll das Markieren fertig machen, nicht die
                    // Ereignisreihenfolge neu erfinden.
                    if let Some(href) = href {
                        doc_mut().pending_link = Some((href, x, y));
                        // Ein Anker fuer die Markierung wird trotzdem gesetzt
                        // — genau darum geht es.
                        let lay = &cache.as_ref().unwrap().0;
                        let had = doc_mut().sel.take();
                        doc_mut().sel_anchor = engine.text_pos_at(lay, cx, cy);
                        if had.is_some() {
                            engine.set_marks(None, Vec::new());
                            mark_dirty();
                        }
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
                    // **Nichts anderes wollte diesen Klick — also faengt hier
                    // eine Markierung an.** Sie kommt ZULETZT, damit sie
                    // keinem Link, keinem Steuerelement und keinem
                    // Seitenskript in die Quere kommt.
                    //
                    // Der Preis, und er ist benannt: eine Markierung, die AUF
                    // einem Link beginnt, gibt es nicht — der Klick
                    // navigiert vorher. Ein Browser folgt dem Link erst beim
                    // LOSLASSEN und kann deshalb beides; das umzustellen ist
                    // ein Eingriff in den Klickweg und gehoert nicht in
                    // denselben Schritt wie das Markieren selbst.
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
        // Loslassen: die Markierung steht, das Ziehen ist vorbei — und HIER
        // entscheidet sich, ob der Druck ein Klick war oder der Anfang einer
        // Markierung.
        Event::MouseButton { button: MouseButton::Left, down: false, x, y } => {
            doc_mut().sel_anchor = None;
            let Some((href, px, py)) = doc_mut().pending_link.take() else { return false };
            // Gezogen? Dann war es keine Navigation. Vier Pixel Toleranz,
            // damit eine zitternde Hand noch klickt.
            if doc().sel.is_some() || (x - px).abs() > 4 || (y - py).abs() > 4 {
                return false;
            }
            follow(engine, &href);
            true
        }
        // **Strg+C auf der Seite.** Der Compositor faengt die Tastenfolge nur
        // ab, wenn eines SEINER Textfelder den Fokus hat (`handle_input_key`
        // steigt sonst sofort aus) — auf der Leinwand kommt sie hier an.
        Event::Clipboard(ClipKind::Copy) => {
            let Some((a, b)) = doc().sel else { return false };
            let Some((lay, _, _, _)) = cache.as_ref() else { return false };
            let text = engine.selected_text(lay, a, b);
            if text.is_empty() { return false }
            let n = unsafe { npk_clipboard_set(text.as_ptr() as i32, text.len() as i32) };
            log(&alloc::format!("[beak] kopiert: {} Zeichen", if n < 0 { 0 } else { n }));
            true
        }
        // `:hover`. A series of ever-cheaper ways to answer "nothing to do":
        // no hover rules on the page at all, then no usable cached layout,
        // then the same element as last time. What is left is answered by
        // repainting the display list in place where that is provably enough
        // — measured on Wikipedia at 0.16 ms against 24 ms for a layout — and
        // only otherwise by laying the page out again.
        Event::MouseMove { x, y } => {
            // **Ziehen kommt VOR dem Hover.** Wer markiert, will keine
            // `:hover`-Rechnung dazwischen — und die frueheste Absage dieses
            // Zweigs („die Seite hat gar keine Hover-Regeln") wuerde die
            // Markierung sonst auf jeder gewoehnlichen Seite verschlucken.
            if doc().sel_anchor.is_some() {
                if let Some((rx, ry, w, h)) = canvas_rect() {
                    let (cx, cy) = (x - rx, y - ry + scroll_y());
                    let fresh = matches!(cache.as_ref(),
                        Some((_, cw, ch, cg)) if *cw == w && *ch == h && *cg == content_gen());
                    if fresh {
                        let lay = &cache.as_ref().unwrap().0;
                        let a = doc().sel_anchor.unwrap();
                        if let Some(b) = engine.text_pos_at(lay, cx, cy) {
                            // Ein Punkt ist keine Markierung — sonst blinkt
                            // bei jedem Klick ein Schleier von einem Pixel auf.
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
        // **Mittelklick auf einen Link: neuer Tab im Hintergrund.**
        //
        // Die Seite bekommt ihn NICHT. Ein Mittelklick ist `auxclick`, nicht
        // `click`; ihn als `click` zuzustellen waere falscher, als ihn
        // wegzulassen, denn ein Behandler, der `preventDefault` ruft, meint
        // damit die linke Taste.
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
        // **`npk_open` auf eine laufende Instanz.** Der Kernel nennt es
        // „Singleton + tabs" — und genau das ist es jetzt: ein zweites
        // Oeffnen ist ein Tab, kein Ersetzen dessen, was gerade dasteht.
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

/// Groesster Wachstumsschritt der Halde: 64 MB. Siehe `acquire` — Verdoppeln
/// ohne Deckel wird ab einigen hundert MB zu einer Forderung, die das Geraet
/// nicht erfuellen kann.
const GROW_CAP_PAGES: usize = 1024;

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
        // **Verdoppeln, aber nicht endlos.** Bis 64 MB ist Verdoppeln richtig:
        // wenige Anfragen, wenig Verschnitt. Darueber wird daraus eine
        // Forderung, die das Geraet nicht erfuellen KANN — bei 512 MB
        // Halde fragte beak nach weiteren 512 MB, der Kernel konnte sie nicht
        // abbilden, und der Absturz sah aus wie „Speicher voll", obwohl vier
        // Seiten gereicht haetten.
        let step = have_pages.max(1).min(GROW_CAP_PAGES);
        let delta = need_pages.max(step);

        // Nacheinander kleiner fragen, statt beim ersten Nein aufzugeben. Das
        // Geraet sagt zu einem halben Gigabyte nein und zu vier Seiten ja —
        // und die vier Seiten sind das, was der Aufrufer wirklich braucht.
        let mut delta = delta;
        let prev_end = loop {
            let got = core::arch::wasm32::memory_grow::<0>(delta);
            if got != usize::MAX { break got }
            if delta <= need_pages {
                // Jetzt ist die Maschine wirklich voll — und das gehoert mit
                // ZAHLEN ins Log, nicht als nackte Panikzeile. Ohne
                // Allokation: wir stecken gerade IM Allokator.
                log("[beak] Halde erschoepft — memory.grow hat nein gesagt");
                log("[beak]   Seiten bisher:");
                log(u32_str(have_pages as u32));
                log("[beak]   noch gebraucht (Seiten):");
                log(u32_str(need_pages as u32));
                return Err(());
            }
            delta = (delta / 2).max(need_pages);
        };
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

    // **Dem Motor den Zufall des Kernels leihen.** Die Engine hat keine
    // Hostfunktionen; sie bekommt eine gereicht, genau wie die Uhr. Ohne
    // diese Zeile gibt es in einer Seite kein `crypto` — und das ist die
    // richtige Antwort, solange keine echte Quelle da ist, statt `Math.random`
    // als sichere auszugeben.
    beak_engine::js::random::set_source(random_bytes);

    // **Hier wurde frueher die Schrift geparst — alle sechs Gesichter, 435 ms
    // und 40 MB Halde, gemessen mit `beakbench`.** Jetzt wird ein Gesicht
    // gebaut, wenn es zum ersten Mal gebraucht wird; eine gewoehnliche Seite
    // fasst zwei bis vier an. Wieviele es wirklich waren, sagt die Zeile nach
    // dem ersten Malen.
    let mut engine = Engine::new();
    // Lend the engine our tick source so it can report the per-phase split.
    engine.set_clock(|| unsafe { npk_ticks() } as u64);
    engine.set_theme(query_theme());
    // Die Kekse der letzten Sitzungen zurueck ins Glas, BEVOR die erste
    // Anfrage rausgeht — sonst laedt die Startseite abgemeldet und meldet
    // sich erst beim zweiten Klick wieder an.
    cookies_restore();
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

    // CSS images still to fetch, as (url_key, url). Filled from the layout.

    // Every CSS image this page has already been asked for — including the
    // ones that failed. A miss must not be retried forever; the box simply
    // stays undecorated until the next navigation.

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
        // **NUR wenn wirklich neu eingesammelt wurde.**
        //
        // Der Wert im Baum ist der, den SEITENCODE gesetzt hat. Ihn in jeder
        // Runde zu uebernehmen hiess: was der Benutzer tippt, wird im
        // naechsten Durchlauf wieder ueberschrieben — das Feld zeigte erst
        // nach dem Abschicken, was darin steht. Ein Umbau des Baums ist das
        // einzige Ereignis, nach dem der Baum mehr weiss als der Wirt.
        //
        // Die Sitzung wird DURCHGEREICHT, nicht neu geholt — zweimal
        // `js_session` waeren zwei veraenderliche Ausleihen auf dasselbe Feld.
        if page.sync(&engine) {
            if let Some(s) = js_session() { pull_control_values(&mut page, s); }
        }
        // Ein Formular, das die Seite schon beim Laden abschicken will, darf
        // nicht bis zum naechsten Klick liegenbleiben.
        let pending = js_session().map(|s| s.interp.take_submits()).unwrap_or_default();
        for seq in pending {
            log(&alloc::format!("[beak] script submit: form seq={seq}"));
            if submit_form_seq(&engine, &page, seq) { break }
        }
        // Und eine Navigation, die die Seite selbst verlangt hat. **Hier
        // zentral und nicht an jedem Einstiegspunkt:** ein Skript beim
        // Laden, ein Zeitgeber, ein `load`-Behandler und ein Klick landen
        // alle in derselben Runde — sechs Aufrufstellen waeren sechs
        // Gelegenheiten, eine zu vergessen.
        sync_nav(&engine);
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
            let q = begin_images(&mut engine);
            engine.css_images_begin();
            let d = doc_mut();
            d.pending_imgs = q;
            d.pending_css_imgs.clear();
            d.css_asked.clear();
        }
        maybe_repaint(&engine, &mut cache, &mut paint_buf, &page.state);
        // Die Schriften, die die Seite mitbringt. NACH dem ersten Auslegen:
        // vorher weiss niemand, welche sie ueberhaupt verlangt.
        if pump_fonts(&engine) {
            bump_content_gen("font");
            mark_dirty();
        }
        // Was `fetch()` bestellt hat. Kommt eine Antwort an, lief danach
        // Seitencode — und der darf den Baum umgebaut haben.
        if pump_fetches() {
            if let Some(s) = js_session() {
                let n = s.interp.run_timers();
                let _ = n;
                // Ein `fetch`-Rueckruf ist ein Einstiegspunkt wie jeder
                // andere: er darf einen Keks setzen und die Adresse
                // umschreiben. Ohne diese zwei Zeilen fiel beides still
                // unter den Tisch — und „still" heisst hier: die naechste
                // Anfrage geht ohne den Keks hinaus, den die Seite gerade
                // gesetzt hat.
                sync_cookies(s);
                sync_history(&engine, s);
                sync_scroll(s);
                drain_console(s);
                if s.interp.doc.as_ref().is_some_and(|d| d.dirty) {
                    if let Some(d) = s.interp.doc.as_mut() {
                        engine.set_scripted_dom(Some(d.to_dom()));
                    }
                    bump_content_gen("fetch");
                    mark_dirty();
                }
            }
        }
        // Die Kasten-Beobachter. `set_geometry` hat waehrend des Malens
        // GEMESSEN; zugestellt wird hier, weil ein Rueckruf ein
        // Einstiegspunkt ist und mitten im Malen nichts zu suchen hat.
        //
        // **Und es MUSS hier stehen.** Ohne diese Zeilen haette eine Seite
        // ohne Zeitgeber und ohne Ereignisse ihre Beobachter angemeldet und
        // nie einen Rueckruf gesehen — die Meldung laege in der Schlange und
        // wartete auf einen Einstiegspunkt, den es nicht gibt.
        pump_box_observers(&engine);
        // JETZT steht die Geometrie — `load` darf fallen.
        if fire_load(&engine, &page) {
            page.sync(&engine);
            if let Some(s) = js_session() { pull_control_values(&mut page, s); }
        }
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
        // Die Schlange wird HERAUSgenommen und zurueckgelegt, statt sie
        // liegend zu leihen: `pump_images` ruft selbst `doc()`, und eine
        // gehaltene Referenz daneben waere die zweite Entleihung, vor der der
        // Kommentar an `doc`/`doc_mut` warnt.
        let mut q = core::mem::take(&mut doc_mut().pending_imgs);
        pump_images(&mut engine, &mut q, layout, band);
        doc_mut().pending_imgs = q;
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
        let mut css_asked = core::mem::take(&mut doc_mut().css_asked);
        let mut pending_css_imgs = core::mem::take(&mut doc_mut().pending_css_imgs);
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
        doc_mut().css_asked = css_asked;
        doc_mut().pending_css_imgs = pending_css_imgs;
        // ALWAYS yield so this worker core can halt — a cooperative fiber that
        // never sleeps pins its core at 100%. A short nap while interacting
        // stays responsive; a longer one when idle keeps the core asleep.
        unsafe {
            // Anything on the wire keeps the short nap: that is how often we
            // ask the kernel whether the answer is here, and it is the whole
            // latency the split costs. 4 ms against a round trip is nothing.
            // Eine laufende Schriftrunde gehoert dazu: sonst schlaeft die
            // Schleife 16 ms je Frage, und eine Seite steht eine Sekunde
            // laenger ungestylt da.
            let waiting = nav_busy() || img_job() >= 0 || cssimg_job() >= 0
                || font_job() >= 0;
            let busy = had_event
                || waiting
                || !doc().pending_imgs.is_empty()
                || !doc().pending_css_imgs.is_empty();
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
