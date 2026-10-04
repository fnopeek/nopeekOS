# `tools/wasm/beak/src/lib.rs` @ 5e0102684

## L1-8 · `#![no_std]`

```
//! beak — native, sandboxed web browser for nopeekOS (docs/spec/BROWSER.md).
//!
//! Stage 0.1: the page is rendered by the portable `beak-engine` (own block
//! layout + fontdue rasterisation) into a `Widget::Canvas`; the chrome
//! (toolbar, address bar, footer) is loft-styled widgets. Scroll comes via
//! `Event::Wheel`, link clicks via a Canvas hit-test against the engine's
//! link rects. The engine is host-agnostic (§10); this shell is the thin
//! nopeek adapter (queries the canvas rect, paints, forwards input).
```

## L32-34 · `struct Strings {`

```
// ── Strings ───────────────────────────────────────────────────────────
// English is the source language; a new one is one more `const` below.
// See `nopeek_widgets::i18n`.
```

## L82 · `#[unsafe(link_section = ".npk.app_meta")]`

```
// ── App metadata + capabilities ───────────────────────────────────────────
```

## L89 · `#[unsafe(link_section = ".npk.caps")]`

```
// RENDER (scene / event / canvas_rect) + CANVAS (canvas_commit) + NET (fetch).
```

## L94 · `#[link(wasm_import_module = "env")]`

```
// ── Host functions ────────────────────────────────────────────────────────
```

## L96-98 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L103-108 · `fn npk_http_begin(`

```
/// Start the general request — any method, extra headers
/// (newline-separated `Name: value`), a body — and come straight back
/// with a handle. Nothing here waits for a network: the kernel waits on a
/// fiber of its own while this loop keeps painting and reading keys.
/// A non-2xx comes back as bytes, not as an error — a 404 page is a
/// document.
```

## L120-121 · `fn npk_http_poll(handle: i32) -> i32;`

```
/// 1 = the answer is here, 0 = still running, -1 = it failed, -2 = no
/// such handle.
```

## L123-125 · `fn npk_http_take(handle: i32, buf_ptr: i32, buf_max: i32) -> i32;`

```
/// Collect a finished request: bytes written, -1 if it failed (the reason
/// is in `npk_http_last_error`), -2 unknown handle, -3 still running.
/// Frees the handle and fills the four response getters below.
```

## L127-128 · `fn npk_http_cancel(handle: i32) -> i32;`

```
/// Give up on a handle. Idempotent — a navigation cancels whatever the
/// last one left in the air without having to know which state it caught.
```

## L130-131 · `fn npk_http_response_headers(buf_ptr: i32, buf_max: i32) -> i32;`

```
/// The last collected response's header block, minus the status
/// line. `Set-Cookie` repeats, so it can only be handed over raw.
```

## L133-135 · `fn npk_http_status() -> i32;`

```
/// Der Statuscode der zuletzt eingesammelten Antwort. Fuer eine
/// Navigation ist er gleichgueltig — eine 404-Seite ist ein Dokument —,
/// fuer `fetch` ist er die halbe Auskunft: `response.ok` haengt daran.
```

## L137-138 · `fn npk_unix_time() -> i64;`

```
/// Seconds since the epoch, UTC. `npk_ticks` cannot stand in: it restarts
/// at every boot and a cookie's `Expires` is an absolute date.
```

## L140-141 · `fn npk_random_bytes(ptr: i32, len: i32) -> i32;`

```
/// Zufall aus dem CSPRNG des Kernels (ChaCha20, aus RDRAND geseedet).
/// Liefert die geschriebenen Bytes, oder -1. Hoechstens 64 KiB je Aufruf.
```

## L144 · `fn npk_http_last_error(buf_ptr: i32, buf_max: i32) -> i32;`

```
/// Why the last request failed: `kind\tmessage`. Cleared on success.
```

## L146 · `fn npk_http_content_type(buf_ptr: i32, buf_max: i32) -> i32;`

```
/// The last response's Content-Type, verbatim. -1 if the server sent none.
```

## L148-151 · `fn npk_net_context(url_ptr: i32, url_len: i32) -> i32;`

```
/// Sagt dem Kernel, WELCHES Dokument gerade angezeigt wird. Er loest die
/// Adresse selbst auf und merkt sich nur die Netzklasse; daran haengt,
/// ob eine Unterressource ins private Netz darf.
/// Siehe `docs/plan/BROWSER_FETCH_ORIGIN.md` §3.1 V2.
```

## L153-155 · `fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;`

```
/// Start a newline-separated list of URLs in one call, multiplexed over
/// HTTP/2 where the host offers it. Same handle discipline as
/// `npk_http_begin`.
```

## L159-163 · `fn npk_tls_connect(host_ptr: i32, host_len: i32, port: i32) -> i32;`

```
/// Ein TLS-STROM, im Gegensatz zu `npk_http_*`: keine Anfrage mit Ende,
/// sondern eine Leitung, die offen bleibt. Gebraucht fuer `wss://`.
/// `connect` blockiert fuer den Handschlag (~60 ms, einmal je
/// Verbindung); `recv` kommt SOFORT zurueck — 0 heisst „noch nichts",
/// -1 heisst zu.
```

## L170-171 · `fn npk_http_begin_many_hdr(urls_ptr: i32, urls_len: i32,`

```
/// Wie oben, aber mit einer Keks-Zeile JE ADRESSE (durch `\n` getrennt,
/// leere Zeilen zaehlen mit). Seit Kernel 0.333.0.
```

## L174-176 · `fn npk_http_take_many(`

```
/// Collect a finished batch: the bodies back-to-back in `out`, one
/// little-endian i32 per URL in `lens` (bytes written, or -1). Returns
/// how many URLs the batch had, or -1 / -2 / -3 as above.
```

## L190 · `fn npk_ticks() -> i64;`

```
/// Milliseconds since boot, 10 ms resolution (the 100 Hz timer).
```

## L194 · `fn now_ms() -> i64 {`

```
/// Milliseconds since boot. Used only for the phase timings below.
```

## L199-202 · `fn log_ms(label: &str, ms: i64) {`

```
/// Log "<label>: <ms> ms". Phase timings are permanent, not scaffolding:
/// on this hardware the engine runs under a WASM interpreter, so knowing
/// which phase a page load actually spends its time in is the difference
/// between fixing the slow thing and rewriting the fast one.
```

## L230-247 · `fn query_theme() -> beak_engine::Theme {`

```
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
```

## L261 · `const ACT_GO: u32 = 1;`

```
// Toolbar
```

## L266-268 · `const ACT_STOP: u32 = 5;`

```
/// Give up on the page being loaded. The reload button becomes this while a
/// navigation is in the air — which is only possible now that one IS in the
/// air rather than in a host call nobody can interrupt.
```

## L271 · `const ACT_MENU_FILE: u32 = 5_000;`

```
// Menu-bar labels (toggle a dropdown)
```

## L278 · `const ACT_FILE_CLOSE: u32 = 6_000;`

```
// Menu items
```

## L285-286 · `const ACT_TAB_NEW: u32 = 7_000;`

```
// Der Tabstreifen. Zwei Baender statt zwei Zahlen je Tab: der Streifen wird
// bei jeder Aenderung neu gebaut, und ein Index IST der Tab.
```

## L288 · `const ACT_TAB_SEL: u32 = 7_100;     // + Index`

```
// + Index
```

## L289 · `const ACT_TAB_CLOSE: u32 = 7_200;   // + Index`

```
// + Index
```

## L291 · `const NODE_MENU_FILE: u32 = 100;`

```
// Menu-label anchor NodeIds (for the dropdown Popover)
```

## L297 · `static mut OPEN_MENU: u8 = 0;`

```
// Which menu dropdown is open (0 = none, else ACT_MENU_FILE..HELP encoded 1..4).
```

## L310 · `const URL_CAP: usize = 4096;`

```
// ── Persistent state (static buffers — no heap growth across page loads) ───
```

## L314-355 · `struct Doc {`

```
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
```

## L357-358 · `url: String,`

```
/// Woher das Dokument KAM (nach Weiterleitungen). Basis fuer jede
/// relative Adresse der Seite und der Netzkontext, den der Kernel fuehrt.
```

## L360 · `edit: String,`

```
/// Was in der Adresszeile STEHT. Nur Text — kein Netzkontext, kein DNS.
```

## L362-363 · `hist: Vec<String>,`

```
/// Zurueck/Vorwaerts. War ein festes Feld aus 64 × 4 KB (256 KB `.bss`),
/// das je Tab noch einmal dagestanden haette.
```

## L367-368 · `nav_job: i32,`

```
// ── Der Ladevorgang dieses Dokuments ────────────────────────────────
/// Der laufende Abruf, oder -1.
```

## L370 · `nav_stage: NavStage,`

```
/// Welche Stufe der Kette gerade laeuft.
```

## L372-373 · `nav_url: Option<String>,`

```
/// Die Adresse, die der laufende Abruf VERLANGT hat — nicht die, aus der
/// er am Ende kam.
```

## L376 · `nav_stage_ms: i64,`

```
/// Beginn der laufenden Stufe, fuer die Zeitzeilen im Log.
```

## L378-379 · `nav_gen: u32,`

```
/// Zaehlt jede Navigation. Woran haengende Rueckrufe erkennen, dass sie
/// zu einer Seite gehoeren, die es nicht mehr gibt.
```

## L383 · `content_gen: u32,`

```
/// Zaehlt jede Aenderung am Inhalt — die Zahl, an der das Layout haengt.
```

## L385 · `scroll_y: i32,`

```
/// Wie weit die Seite gerollt ist.
```

## L387 · `sel_anchor: Option<beak_engine::select::TextPos>,`

```
/// Wo eine Textmarkierung begonnen hat, solange die Taste unten ist.
```

## L389-390 · `pending_link: Option<(String, i32, i32)>,`

```
/// Der Link unter dem Druck, bis das Loslassen entscheidet — mit dem
/// Punkt, an dem gedrueckt wurde.
```

## L392-393 · `sel: Option<(beak_engine::select::TextPos, beak_engine::select::TextPos)>,`

```
/// Die Markierung auf der SEITE — nicht in der Adresszeile, die gehoert
/// dem Compositor.
```

## L395-399 · `find: Option<String>,`

```
/// Die Suchleiste: `None` heisst zu. Der Text gehoert BEAK, nicht einem
/// `Widget::Input` — `Event::InputChange` traegt keine Knotenkennung,
/// zwei Eingabefelder im Fenster waeren also nicht auseinanderzuhalten.
/// Ein selbst gefuehrter Puffer ist die kleinere Antwort als eine
/// ABI-Erweiterung.
```

## L401 · `found: Vec<(beak_engine::select::TextPos, beak_engine::select::TextPos)>,`

```
/// Fundstellen der laufenden Suche und die, auf der man gerade steht.
```

## L404 · `find_pending: [u8; 4],`

```
/// Sammelpuffer fuer ein Zeichen in der Suchleiste.
```

## L408-412 · `nav_css_count: usize,`

```
// ── Die Teilabrufe des Ladevorgangs ─────────────────────────────────
// Blaetter, Skripte, Module: jede Stufe fuehrt Buch darueber, was sie
// verlangt hat und in welcher Runde sie steht. Das gehoert zum LADEN
// EINES Dokuments — zwei Tabs, die gleichzeitig laden, brauchen zwei
// davon, und mit einer Static waeren es zwei Seiten auf einem Zettel.
```

## L415 · `js: Option<beak_engine::js::Session>,`

```
/// Die JS-Sitzung DIESER Seite.
```

## L430-433 · `dirty: bool,`

```
// ── Die Ansicht DIESES Dokuments ────────────────────────────────────
// Nicht zu verwechseln mit dem, was der BILDPUFFER haelt (`LAST_W/H/SY`
// unten): der Puffer ist einer, die Seiten sind viele.
/// Das Bild ist nicht mehr das, was die Seite sagt.
```

## L435 · `need_full: bool,`

```
/// Und zwar aus einem anderen Grund als Rollen — also ganz neu malen.
```

## L437 · `images_dirty: bool,`

```
/// Ein Bild ist angekommen, die Bildliste muss neu durchgesehen werden.
```

## L439-444 · `geom: Option<alloc::rc::Rc<alloc::vec::Vec<beak_engine::layout::ElemRect>>>,`

```
/// Die Kaesten des letzten Layouts, fuer `getBoundingClientRect` & Co.
///
/// Als `Rc` gehalten, damit das Weiterreichen an die JS-Maschine nichts
/// kostet: der Rollstand aendert sich bei JEDEM Bild, die Kaesten nur bei
/// einem neuen Layout — ohne das waere jede Rollbewegung eine Kopie von
/// ~180 KB.
```

## L446 · `last_vp: (i32, i32),`

```
/// Das Sichtfeld, das die JS-Sitzung dieses Dokuments zuletzt gehoert hat.
```

## L448-452 · `told_scroll: i32,`

```
/// Rollstand und Fenstermass, wie sie der SEITE zuletzt gemeldet wurden.
///
/// Getrennt von `scroll_y`/`last_vp`: die sagen, was gemalt wird, diese
/// sagen, was die Seite WEISS. Ohne den Unterschied faellt `scroll` bei
/// jedem Bild oder gar nicht.
```

## L455-456 · `caret_phase: Option<bool>,`

```
/// In welcher Haelfte des Blinktakts der Zeiger zuletzt gemalt wurde.
/// `None` heisst: es blinkt gerade keiner.
```

## L458-460 · `caret_since: i64,`

```
/// Wann der Takt zuletzt zurueckgesetzt wurde. Ein Browser zeigt den
/// Zeiger direkt nach einem Tastendruck SOLIDE — wer tippt, will sehen,
/// wo er steht, und nicht auf die naechste Halbsekunde warten.
```

## L463-470 · `img_job: i32,`

```
// ── Die Nebenabrufe DIESES Dokuments ────────────────────────────────
// Bilder, Hintergruende, Schriften, `fetch`: alles, was NEBEN dem
// Dokument laeuft und mit ihm endet. Eine zweite Seite hat ihre eigenen —
// und `subresources_cancel` bricht genau die einer Seite ab, nicht die
// aller.
/// Der laufende `<img>`-Stapel und wonach er gefragt hat, damit ein
/// ankommender Rumpf unter der Quelle abgelegt wird, unter der die Seite
/// ihn genannt hat. -1 / None, wenn nichts unterwegs ist.
```

## L473-474 · `img_missed: Vec<String>,`

```
/// Adressen, zu denen schon eine Fehlanzeige im Log steht — die Meldung
/// gehoert EINMAL hin, nicht in jedes Bild.
```

## L476 · `cssimg_job: i32,`

```
/// Dasselbe fuer Hintergruende, benannt wie das Layout sie fuehrt.
```

## L479 · `font_job: i32,`

```
/// Die laufende Schriftrunde.
```

## L482 · `fetch_jobs: Vec<(u32, i32)>,`

```
/// Welche `fetch`-Anfrage der Engine auf welchem Griff des Wirts liegt.
```

## L484-489 · `pending_imgs: Vec<String>,`

```
/// Die Bilder DIESER Seite, die noch nicht gefragt wurden — der Rest der
/// Schlange hinter dem Stapel, der gerade laeuft.
///
/// Standen bis 0.163.0 als Locals in der Schleife, und damit gehoerten
/// sie niemandem: bei einem Tabwechsel haette der neue Tab die Bilder des
/// alten weitergeholt, mit `resolve` gegen die NEUE Basis.
```

## L492-493 · `css_asked: Vec<u64>,`

```
/// Jeder Hintergrund, nach dem diese Seite schon gefragt hat — auch die
/// gescheiterten. Ein Fehlschlag darf nicht ewig wiederholt werden.
```

## L496-497 · `script_nav_chain: u32,`

```
// ── Die Skriptrunde DIESES Dokuments ────────────────────────────────
/// Wie viele Navigationen die Seite HINTEREINANDER selbst ausgeloest hat.
```

## L499-500 · `nav_from_script: bool,`

```
/// Setzt `sync_nav` unmittelbar vor `nav_begin` — daran erkennt
/// `nav_begin`, dass die Kette WEITERgeht statt neu anzufangen.
```

## L502-504 · `script_tally: (usize, usize, usize),`

```
/// Was die gewoehnlichen Skripte ergeben haben (gelaufen, gescheitert,
/// Bytes) — muss die Modulrunden ueberleben, weil der Bericht erst danach
/// geschrieben wird.
```

## L506-507 · `script_t0: i64,`

```
/// Wann die Skriptrunde begann. NICHT `nav_stage_ms`: das steht nach einer
/// Modulrunde auf deren Beginn, und die gemeldete Zeit waere zu klein.
```

## L509 · `load_pending: bool,`

```
/// Steht `load` noch aus? Es faellt erst, wenn die Geometrie steht.
```

## L511-512 · `obs_rounds: u32,`

```
/// Wie viele Bilder hintereinander ein Beobachter-Rueckruf schon den Baum
/// geaendert hat — der Riegel gegen die „ResizeObserver loop".
```

## L515-520 · `last_layout_ms: i64,`

```
// ── Was diese Seite gekostet hat, und was darueber EINMAL gesagt wird ─
// Alle fuenf setzt `set_url` zurueck: eine neue Seite bekommt ihr eigenes
// Urteil und ihre eigene Gelegenheit, es zu sagen. Ohne das brachte eine
// schwere Seite den Zeiger fuer jede spaetere zum Schweigen.
/// Was das letzte volle Layout gekostet hat, ms — die Zahl, die
/// entscheidet, ob diese Seite sich `:hover` leisten kann.
```

## L526-527 · `sel_box: Option<(i32, i32, i32, i32, String)>,`

```
/// Der im Inspektor gewaehlte Kasten: `(x, y, w, h)` im Dokumentraum, mit
/// seiner Beschriftung. Die Koordinaten gelten in DIESEM Dokument.
```

## L530-535 · `title: String,`

```
// ── Was einen eingefrorenen Tab wiederherstellt ─────────────────────
// Diese Felder und die vier ganz oben (`url`, `edit`, `hist`, `hist_pos`)
// sind ALLES, was ein Tab im Hintergrund behaelt — Kilobytes statt der
// 44 MiB, die eine lebende Seite haelt. Siehe `tab_freeze`.
/// Der `<title>` der Seite, fuer den Streifen. Wird beim Auslegen
/// nachgezogen (frueher weiss es niemand) und ueberlebt das Einfrieren.
```

## L537-539 · `scroll_want: i32,`

```
/// Wohin nach dem Laden gerollt werden soll. 0 fuer eine neue Seite, der
/// gemerkte Stand fuer einen Tab, der zurueckkommt — die Seite wird beim
/// Zurueckwechseln neu geholt, und ohne diese Zahl stuende sie oben.
```

## L568-590 · `static mut TABS: Vec<alloc::boxed::Box<Doc>> = Vec::new();`

```
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
```

## L592 · `static mut ACTIVE: usize = 0;`

```
/// Welcher Tab gemalt wird und lebt. Immer gueltig — `active` klemmt.
```

## L595-603 · `static mut ENGINE: Option<Engine> = None;`

```
/// **Die Layout-Engine als Globale, nicht als Variable der Bildschleife.**
///
/// Sie lag bis 0.184.0 in `main`, und das ging, solange nur die Schleife sie
/// brauchte. `Interp::relayout` ist aber ein `fn`-Zeiger und faengt nichts
/// ein: der Haken, mit dem eine Seite mitten im Skript ein frisches Layout
/// verlangt, kommt an eine lokale Variable nicht heran.
///
/// Die Entleihung endet wie bei `tabs()` in der rufenden Anweisung — wer ein
/// `&mut` ueber einen Skriptlauf festhielte, haette zwei davon.
```

## L607-608 · `unsafe { (*core::ptr::addr_of_mut!(ENGINE)).get_or_insert_with(Engine::new) }`

```
// SAFETY: ein Faden; `main` legt sie vor dem ersten Gebrauch an, und die
// Entleihung endet in der rufenden Anweisung.
```

## L612 · `unsafe { (*core::ptr::addr_of_mut!(ENGINE)).get_or_insert_with(Engine::new) }`

```
// SAFETY: wie `engine()`.
```

## L616-622 · `const MAX_TABS: usize = 10;`

```
/// Wieviele Tabs.
///
/// **Der Speicher ist es nicht** — ein eingefrorener Tab ist Kilobytes. Es
/// ist der Streifen: 160 px je Tab, zehn davon sind 1600 px, und darueber
/// schoebe der elfte das `+` aus dem Fenster. Was diesen Deckel hebt, ist
/// ein rollender Streifen (`Widget::Scroll`, `Axis::Horizontal`) — der
/// einzige Kasten, der im Compositor wirklich abschneidet.
```

## L626-627 · `unsafe {`

```
// SAFETY: ein Faden; die Entleihung endet in der rufenden Anweisung.
// Der leere Fall trifft genau einmal, beim allerersten Zugriff.
```

## L637-639 · `fn active() -> usize {`

```
/// Der laufende Tab. **Geklemmt, nicht geprueft:** ein `ACTIVE`, das auf
/// einen geschlossenen Tab zeigt, waere sonst eine Panik an einer Stelle, an
/// der niemand sie erwartet.
```

## L649-660 · `fn doc() -> &'static Doc {`

```
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
```

## L670-674 · `fn clip(s: &str, cap: usize) -> &str {`

```
/// Auf `cap` Bytes kuerzen, aber an einer ZEICHENgrenze.
///
/// Der Vorgaenger kopierte `min(len, cap)` rohe Bytes in einen festen Puffer;
/// traf das mitten in ein Zeichen, war der Inhalt kein gueltiges UTF-8 mehr
/// und `from_utf8(...).unwrap_or("")` machte daraus eine LEERE Adresse.
```

## L686-693 · `const CSS_CAP: usize = 8 * 1024 * 1024;`

```
// Concatenated bytes of the page's external <link rel=stylesheet> files.
// Both numbers are measured against real pages, not guessed: GitHub links 37
// sheets totalling 4.4 MiB, MDN links 17 that come to 71 KiB, SRF 5 / 367 KiB,
// Wikipedia 3 / 273 KiB. The old 16-link cap therefore broke MDN at 3 % of the
// byte budget — the count was the wrong unit, the same mistake MAX_IMAGES made.
// A dropped stylesheet is not a missing icon, it is a broken page, so the
// headroom is deliberate. The buffer is `.bss`: it costs runtime memory only,
// nothing in the shipped .wasm.
```

## L699-706 · `const IMG_FETCH_CAP: usize = 24 * 1024 * 1024;`

```
// Scratch buffer a whole BATCH of <img> bytes arrives in before decoding.
//
// It is shared by `IMG_BATCH` images at once, so it was never "6 MB per
// picture" — it was 1.5. A single press photograph is bigger than that, and it
// would have failed with `n == 0`, which used to say nothing at all. The
// kernel bounds what all pending answers may reserve together
// (`MAX_RESERVED_BYTES`, 64 MB); 24 MB here leaves room for a document (3) +
// stylesheets (8) + scripts (8) in flight beside it.
```

## L708-718 · `const MAX_IMAGES: usize = 512;`

```
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
```

## L722-725 · `const IMG_BATCH: usize = 4;`

```
/// How many images one batch asks for. Small on purpose: the batch is a
/// blocking call, so a whole page in one go would freeze the window again —
/// the very thing progressive loading fixed. Four is enough to overlap the
/// round-trips while a turn of the loop stays short.
```

## L728-729 · `static mut LENS_BUF: [u8; 4 * MAX_CSS_LINKS] = [0; 4 * MAX_CSS_LINKS];`

```
/// Receives the per-URL length table from `npk_http_take_many`. Sized for
/// the largest batch either caller asks for.
```

## L732 · `fn url_lines(urls: &[String]) -> String {`

```
/// The URL list a batch call wants: one per line.
```

## L744-745 · `fn begin_batch(urls: &[String], cap: usize) -> i32 {`

```
/// Start a batch and return its handle, or -1. `cap` is the room the bodies
/// may take together.
```

## L748-755 · `let now = unsafe { npk_unix_time() };`

```
// **Jede Unterressource bekommt ihren Keks.** Vorher trug nur die
// Anfrage nach dem DOKUMENT eine `Cookie`-Zeile; Bilder, Blaetter und
// Skripte gingen anonym raus. Hinter einer Anmeldung kam so der Text an
// und die Bilder nicht, und es sah aus wie ein Bildfehler.
//
// Eine Zeile je Adresse, in derselben Reihenfolge, LEERE ZEILEN
// EINGESCHLOSSEN — die Zuordnung ist die Position, und wer leere Zeilen
// wegwirft, schickt den Keks der einen Adresse an eine andere.
```

## L764-765 · `if !any {`

```
// Kein Keks im Spiel? Dann der alte Weg — eine Zeile weniger ueber die
// Grenze, und der Kernel muss nichts pruefen.
```

## L775-780 · `fn take_batch(handle: i32, dst: *mut u8, cap: usize, want: usize) -> Vec<(usize, usize)> {`

```
/// Collect a finished batch into `dst`, returning each body as a
/// `(offset, len)` span.
///
/// Returns an empty vec if the batch failed, which the callers treat the
/// same as "none of them loaded" — every one of them degrades to a
/// placeholder or to unstyled content rather than to a blank page.
```

## L802 · `spans.push((0, 0)); // this one failed; keep positions aligned`

```
// this one failed; keep positions aligned
```

## L810 · `static mut FINAL_URL_BUF: [u8; URL_CAP] = [0; URL_CAP];`

```
// Scratch for the kernel to write back the post-redirect URL of a fetch.
```

## L821-827 · `static mut LAST_W: i32 = -1;`

```
/// **Was der BILDPUFFER haelt — nicht, was die Seite sagt.**
///
/// Der Puffer ist einer, auch wenn es spaeter mehrere Dokumente gibt: diese
/// drei Zahlen beschreiben das Bild, das gerade im Puffer steht, und gehoeren
/// deshalb dem Fenster. Die Frage „muss neu gemalt werden?" gehoert dagegen
/// dem Dokument (`Doc::dirty`, `Doc::need_full`) — ein Bild, das im
/// Hintergrund ankommt, macht SEINE Seite alt, nicht das Bild auf dem Schirm.
```

## L830-831 · `static mut LAST_SY: i32 = 0;`

```
/// The scroll offset the buffer currently HOLDS, so the next frame knows how
/// far the picture has to move.
```

## L834-840 · `fn tell_net_context(url: &str) {`

```
/// Dem Kernel sagen, aus welchem Dokument die naechsten Anfragen kommen.
///
/// **Der Kernel glaubt uns die Adresse, aber nicht die Klasse** — er loest
/// selbst auf. Was das garantiert: Seitencode kann den Kontext nie
/// erweitern, weil Seitencode keinen Weg zu einer Host-Funktion hat. Was es
/// NICHT garantiert: dass beak selbst sich nicht vertut. Dafuer gibt es nur
/// diese eine Stelle und `set_url` — beide unten.
```

## L846-848 · `tell_net_context(s);`

```
// Nach dem Laden noch einmal, mit der Adresse, aus der das Dokument
// WIRKLICH kam (nach Weiterleitungen). `nav_begin` hat vorher schon die
// des Ziels gemeldet; hier wird sie richtiggestellt.
```

## L853-855 · `d.sel = None;`

```
// Eine Markierung gehoert dem Text, den sie markiert. Die neue Seite hat
// andere Textbefehle an denselben Stellen — die alten Orte zeigten dort
// auf irgendetwas.
```

## L858-859 · `d.pending_link = None;`

```
// Ein Link, dessen Loslassen nie kam, darf die naechste Seite nicht
// umleiten.
```

## L861 · `set_edit(s);`

```
// Die Zeile zeigt, wo man IST — bis jemand hineintippt.
```

## L863-865 · `let d = doc_mut();`

```
// A new page gets its own verdict on whether it can afford `:hover` —
// and its own chance to say so once. Without this, one heavy page
// silences the pointer for every page after it.
```

## L875-882 · `fn set_edit(s: &str) {`

```
/// Nur das Textfeld — kein Netzkontext, keine neue Basis, kein DNS.
///
/// **Ein Tastendruck ist keine Navigation.** Bis 0.146.0 rief jeder
/// Tastendruck `set_url`, und der meldet dem Kernel den Netzkontext; der
/// loest dafuer AUF. Am Geraet stand das als sechzehn DNS-Abfragen im Log —
/// `sandbox.nopeek.c`, `sandbox.nopeek.`, `sandbox.nopeek`, … bis zur leeren
/// Zeichenkette —, weil der Benutzer die Adresse rueckwaerts geloescht hat.
/// Jedes Praefix dessen, was jemand tippt, ging an den Aufloeser.
```

## L905-907 · `struct Page {`

```
/// The current page's forms + the user's live edits to them. Rebuilt on every
/// navigation (keyed on `Doc::nav_gen`, NOT the layout's content generation — a theme
/// switch or an image arriving must not wipe what the user has typed).
```

## L912 · `scripted: u64,`

```
/// Stand des Baums, aus dem `forms` gebaut wurde.
```

## L914-918 · `focus_value: Option<String>,`

```
/// Der Wert des fokussierten Feldes, als es den Fokus BEKAM.
///
/// `change` faellt bei einem Textfeld nicht je Zeichen, sondern beim
/// Verlassen — und nur, wenn sich wirklich etwas geaendert hat (HTML
/// §4.10.5.5). Ohne diesen Wert waere „geaendert" nicht zu beantworten.
```

## L920-926 · `logged: u64,`

```
/// Fingerabdruck des zuletzt GEMELDETEN Bestands.
///
/// `sync` laeuft nach jedem Skriptlauf und nach jedem Beobachter-Rueckruf,
/// nicht nur nach einer Navigation — der Kommentar unten sagte „once per
/// navigation", und am Geraet standen dieselben acht Zeilen achtmal
/// untereinander. Ein Bestand, der sich nicht geaendert hat, ist keine
/// Nachricht.
```

## L936-946 · `fn sync(&mut self, engine: &Engine) -> bool {`

```
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
```

## L960-973 · `if navigated {`

```
// Eine NAVIGATION wirft die Eingaben weg, ein Skriptlauf nicht — was
// der Benutzer getippt hat, gehoert ihm, auch wenn die Seite daneben
// etwas umbaut.
//
// **Und „gehoert ihm" heisst: es muss die Nummerierung ueberleben.**
// `Doc::to_dom` vergibt die `seq` bei JEDEM Zurueckschreiben neu, von
// eins an in Dokumentreihenfolge — ein einziger Knoten mehr am
// Anfang, und jedes Steuerelement dahinter heisst anders. `FormState`
// ist nach genau dieser Zahl geschluesselt. Auf DuckDuckGos
// Startseite wanderten die Knoepfe zwischen 156 und 157 hin und her,
// und die Sucheingabe war nach jedem Lauf verwaist: abgeschickt wurde
// ein leeres `q`. Der BAUM traegt den Wert (dorthin schreibt jeder
// Tastendruck), also wird er von dort zurueckgeholt — unter den neuen
// Nummern.
```

## L983-989 · `fn log_forms(&mut self) {`

```
/// Report the forms this page offers, once per navigation.
///
/// "The button does nothing" has several very different causes — a button
/// we never saw, one whose form we could not resolve, a form whose action
/// we misread — and from the outside they look identical. Three lines on
/// the serial tell them apart, which a screenshot cannot: a picture shows
/// the pixels, not who owns which control.
```

## L1015-1016 · `for c in self.forms.controls.iter().filter(|c| c.kind.is_submit()).take(8) {`

```
// Only the controls that are supposed to DO something — a page can
// carry dozens of hidden fields and they are not the question.
```

## L1027-1031 · `if !c.label.is_empty() {`

```
// **Was auf dem Knopf STEHT.** Googles Einwilligungsseite hat vier
// Formulare auf dieselbe Adresse; ohne die Beschriftung sind
// „Alle ablehnen" und „Alle akzeptieren" im Log nicht zu
// unterscheiden, und wer hier blind das erste nimmt, wirft eine
// Muenze ueber eine Entscheidung des Benutzers.
```

## L1040 · `fn focused(&self) -> Option<(&forms::Control, &str)> {`

```
/// The focused control's current text + its kind.
```

## L1048 · `fn nav_gen() -> u32 {`

```
// Navigation generation — bumped ONLY by a real page load.
```

## L1057-1058 · `static mut USE_SITE_CSS: bool = true;`

```
// Reader-mode toggle: apply the site's own (external + <style>) CSS, or render
// with just our UA sheet (docs/spec/BROWSER.md §9.7 — never worse than clean content).
```

## L1067-1070 · `static mut INSPECT_MODE: bool = false;`

```
// Inspect dev tool: when on, the engine records an element box per node and a
// canvas click selects the deepest box under the cursor (outline + a label in
// the status bar) instead of following a link — so a mis-rendered element can
// be named on the device.
```

## L1090-1091 · `fn do_layout(engine: &Engine, w: u32, state: &FormState) -> Layout {`

```
/// Lay out the current page honoring the reader-mode toggle: full site CSS
/// (external `<link>` + inline `<style>`) when on, UA-only when off.
```

## L1093-1094 · `if let Some((_, _, _, h)) = canvas_rect() {`

```
// The viewport height is the initial containing block's height — what
// `top:0; bottom:0` on a root-level abspos box stretches to.
```

## L1106-1109 · `let mut label = String::from("layout @");`

```
// The width belongs IN the number. A device timing without it cannot be
// compared with anything -- 1000 px against 1880 px is a factor of 1.75 --
// and reading it off the screen means opening a window, which changes the
// very width being measured.
```

## L1115-1118 · `let p = lay.phase;`

```
// ...and WHICH of the three it was. The host profile says the box layout
// dominates, but the host is not a WASM interpreter and the phases do not
// scale alike under one: beak 0.18.0 halved the box layout on the host and
// moved the device number by nothing at all. One number cannot say why.
```

## L1136-1141 · `fn scroll_after_load() {`

```
/// Wohin die Seite nach dem Laden rollt.
///
/// **Null fuer eine neue Seite** — sie faengt oben an. Der gemerkte Stand
/// fuer einen Tab, der zurueckkommt: er wird beim Wechsel neu GEHOLT
/// (`docs/plan/BROWSER_TABS.md` §A3 b), und ohne diese Zahl staende der Leser
/// nach jedem Wechsel wieder ganz oben.
```

## L1150-1156 · `fn mark_dirty() {`

```
/// Something other than scrolling wants a repaint.
///
/// Scrolling does not change the page, it moves it — so a frame that is dirty
/// for scrolling ALONE can be blitted and have one band redrawn. Anything else
/// (a hover, a form key, a new layout) sets `need_full` and gets the whole
/// viewport. It is set, never cleared, until a frame is actually painted: a
/// hover followed by a scroll must still repaint everything.
```

## L1163 · `fn mark_dirty_scrolled() {`

```
/// Dirty because the viewport MOVED — the display list is untouched.
```

## L1168-1172 · `fn bump_content_gen(why: &str) {`

```
// Content generation — bumped on every fetch so the layout cache knows to
// re-lay-out (vs. reusing it for scroll, which keeps scrolling smooth).
/// When the current navigation started, so the first paint after it can
/// report the ONE number a user actually feels: click → something on screen.
/// Cleared once that number has been reported for this navigation.
```

## L1174-1176 · `fn bump_content_gen(why: &str) {`

```
/// Invalidate the layout cache. `why` is logged because a full re-layout is
/// the single most expensive thing this app does (~4.7 s on device), so an
/// unexpected one has to be attributable at a glance.
```

## L1189-1193 · `const HOVER_BUDGET_MS: i64 = 250;`

```
/// A pointer that costs more than this to follow makes the page feel broken —
/// the window stops answering while it re-lays-out. Below it, hover is free
/// enough to be worth having. Only the FALLBACK is measured against it: a
/// pointer change the engine can answer by repainting costs a fraction of a
/// millisecond and is never refused.
```

## L1195-1200 · `fn say_hover_once(fast: bool, ms: i64, why: &str) {`

```
/// Say ONCE per page how the pointer is being answered here.
///
/// Once each, for the reason in `Doc`: without them a pointer answered by
/// repainting is INVISIBLE in the log, so a device run cannot tell "it works"
/// from "it never happened" — which is exactly what the first 0.28.0 log could
/// not say ([[feedback-log-the-version-in-the-trace]]).
```

## L1218 · `fn hover_affordable() -> bool {`

```
/// Can this page afford to restyle on pointer movement?
```

## L1234-1236 · `fn last_error() -> Option<(String, String)> {`

```
/// Why the last fetch failed, as `(kind, message)`. `None` if the kernel
/// reported nothing — which includes an older kernel without the host fn,
/// so the caller must have a fallback rather than assume this is present.
```

## L1251-1253 · `fn show_error_page(url: &str) {`

```
/// Replace the document with a diagnostic page. Sets HTML_BUF/HTML_LEN
/// exactly as a successful fetch would, so everything downstream — layout,
/// paint, scrolling — treats it as an ordinary page.
```

## L1256-1257 · `.unwrap_or_else(|| (String::from("unknown"), String::from("request failed")));`

```
// A -1 with no reason attached still has to say something. Silence
// here is the blank page this whole path exists to remove.
```

## L1267-1269 · `core::ptr::addr_of_mut!(CSS_LEN).write(0);`

```
// The page carries its own inline <style> and links nothing, so any
// leftover author CSS from the previous page must go — otherwise the
// last site's rules would style this one.
```

## L1274-1276 · `fn content_type() -> Option<String> {`

```
/// The last response's Content-Type. `None` if the server sent none — or if
/// the kernel is older than the host fn, which is why every caller has to
/// cope with not knowing rather than assume UTF-8.
```

## L1289-1292 · `fn decode_document() {`

```
/// Bring the freshly fetched document to valid UTF-8, in place.
///
/// Must run before ANYTHING reads `html_str()` — the stylesheet scan does,
/// and a document still holding raw Latin-1 reads back as the empty string.
```

## L1309-1311 · `fn decode_css() {`

```
/// Same for the concatenated stylesheets. No Content-Type here — they arrive
/// through the batch fetch, which reports one status per URL and no headers —
/// so this is sniff-only. One bad byte used to cost the page ALL its CSS.
```

## L1327-1328 · `fn fetched_from() -> Option<String> {`

```
/// The URL the last fetch's body actually came from, after redirects.
/// `None` if the kernel reported none (request failed, or an older kernel).
```

## L1339-1340 · `const HDR_CAP: usize = 8 * 1024;`

```
/// Room for one response's header block — the kernel caps what it hands back
/// at 8 KiB, and a page that sets a dozen cookies still fits.
```

## L1344-1354 · `#[derive(Clone, Copy, PartialEq)]`

```
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
```

## L1360-1361 · `Js,`

```
/// Die externen Skripte der Seite. Eine dritte Rundreise, und sie kommt
/// NACH den Stilblaettern: ein Skript liest Klassen und Groessen.
```

## L1363-1365 · `Mod,`

```
/// Der Modulgraph. Anders als die drei davor ist das KEINE einzelne
/// Rundreise: ein Modul nennt seine Abhaengigkeiten erst, wenn es da ist,
/// also geht es rundenweise, bis der Graph geschlossen ist.
```

## L1367-1369 · `Sheet,`

```
/// Stilblaetter, die ein SKRIPT eingehaengt hat. Auch rundenweise: ein
/// Blatt, das ankommt, laesst eine Komponente fertig bauen, und die haengt
/// ihrerseits eins ein.
```

## L1371-1372 · `CssImport,`

```
/// Die `@import`-Blaetter der verlinkten Blaetter. Rundenweise wie `Mod`:
/// ein Blatt nennt seine eigenen Importe erst, wenn es da ist.
```

## L1374-1376 · `DynJs,`

```
/// `<script src=…>`, die ein SKRIPT eingehaengt hat. Rundenweise wie
/// `Sheet`: ein Stueck, das ankommt, haengt das naechste ein — genau so
/// laedt ein geteiltes Buendel seinen Baum nach.
```

## L1380-1387 · `fn nav_job() -> i32 {`

```
/// Handle of the navigation in flight, or -1.
/// The address that was ASKED for. The diagnostic page names it, and it
/// stands in for the base URL if the response never said where it came from.
/// Record the landing address in the history once the document is here.
/// Where we LANDED, not where we aimed — otherwise every trip back through
/// history replays the redirect.
/// When the stage in flight started, so each round trip reports its own span
/// instead of the navigation's total.
```

## L1393-1394 · `fn nav_busy() -> bool {`

```
/// Is a page load in the air? The toolbar asks (its reload button becomes a
/// stop button), and so does the idle nap.
```

## L1408-1410 · `fn nav_cancel() {`

```
/// Drop a navigation still in the air — a second click, or Stop. The kernel
/// throws its answer away; nothing on screen changes, so the page that is
/// already there stays readable.
```

## L1419 · `fn nav_asked() -> String {`

```
/// The address the navigation in flight asked for.
```

## L1424-1425 · `fn nav_begin(engine: &Engine, method: &str, url: &str, body: &[u8], extra: &str, push_hist: bool) {`

```
/// Start a navigation and return at once. `push_hist` records the address we
/// land on once the document is here.
```

## L1427-1430 · `if doc().nav_from_script {`

```
// Eine Navigation, die nicht aus einem Skript kam — ein Klick, die
// Adresszeile, der Verlauf — bricht die Skriptkette. Sonst zaehlte der
// Deckel ueber Seiten hinweg weiter und wuerde irgendwann eine
// vollkommen harmlose Weiterleitung abwuergen.
```

## L1436-1440 · `tell_net_context(url);`

```
// **Vor dem ersten Byte.** Eine Navigation darf ueberallhin — auch auf
// den eigenen Router —, denn das neue Dokument ist eine andere Herkunft
// und die alte Seite kann es nicht lesen. Gemeldet wird deshalb die
// Klasse des ZIELS, nicht die der Seite, die wir verlassen; sonst waere
// `https://192.168.1.1` von einer oeffentlichen Seite aus gesperrt.
```

## L1442-1444 · `nav_cancel();`

```
// A new navigation replaces the old one, and takes the page it was
// loading for with it — a browser that keeps fetching the pictures of the
// page you just left is spending the network on nothing.
```

## L1447-1448 · `engine.set_scripted_dom(None);`

```
// Und den vom Skript veraenderten Baum, sonst zeigt die naechste Seite den
// der vorigen — der Zwischenspeicher haengt am HTML, nicht am Baum.
```

## L1451-1452 · `{ doc_mut().js = None };`

```
// Die Sitzung gehoert der Seite, die gerade verlassen wird: ihre
// Behandler zeigen auf Knoten, die es gleich nicht mehr gibt.
```

## L1455-1457 · `if selftest::matches(url) {`

```
// Die eingebaute Pruefseite kommt aus dem Binaerbild, nicht aus dem Netz.
// Sie durchlaeuft ab hier denselben Weg wie ein geholtes Dokument — nur
// ohne die erste Rundreise.
```

## L1470-1473 · `{`

```
// **Welche Kekse mitgehen, nach NAMEN.** Der Wert ist ein Geheimnis, der
// Name nicht — und ohne ihn ist „5 held" keine Auskunft. Googles
// Einwilligung schickte im Kreis, und aus dem Log war nicht zu sehen, ob
// `SOCS` ueberhaupt dabei war.
```

## L1517-1519 · `nav_fail(url);`

```
// Refused at the door — a malformed address, or the kernel's fetch
// table full. That is as much a failed navigation as a refused
// certificate, and it names itself through the same getter.
```

## L1524-1527 · `fn nav_fail(url: &str) {`

```
/// The document did not arrive. Put a diagnostic page where it should have
/// been: a blank canvas is indistinguishable from a hung browser, and the
/// address bar keeps the URL that was ASKED for rather than one derived from
/// a response we never got.
```

## L1529-1530 · `doc_mut().scroll_want = 0;`

```
// Ein gemerkter Rollstand gehoert der Seite, die nicht kam — nicht der
// Fehlermeldung, die an ihrer Stelle steht.
```

## L1540-1550 · `const COOKIE_FILE: &str = "priv/beak/cookies";`

```
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
```

## L1552 · `const COOKIE_CAP: usize = 96 * 1024;   // 256 Kekse passen weit darunter`

```
// 256 Kekse passen weit darunter
```

## L1555 · `fn cookies_restore() {`

```
/// Die gespeicherten Kekse ins Glas — einmal beim Start.
```

## L1563 · `if n <= 0 { return }`

```
// Keine Datei ist der Normalfall beim ersten Start, kein Fehler.
```

## L1576-1577 · `fn cookies_persist() {`

```
/// Die dauerhaften Kekse auf die Platte. Sitzungskekse bleiben draussen —
/// `Jar::serialize` entscheidet das, nicht diese Stelle.
```

## L1590-1592 · `fn file_cookies(asked: &str) {`

```
/// Cookies are scoped to where the response CAME from, after redirects —
/// filing them against the URL we asked for would scope a login cookie to the
/// wrong host.
```

## L1607-1608 · `cookies_persist();`

```
// Nur wenn sich wirklich etwas geaendert hat — sonst schriebe jede
// Antwort dieselbe Datei neu.
```

## L1613-1614 · `let da = cookies::names_held(&from);`

```
// Und WELCHE der Host jetzt hat. Eine Zahl sagt nicht, ob der eine
// Keks dabei ist, an dem die Sitzung haengt.
```

## L1624-1632 · `fn deliver_builtin(engine: &Engine, url: &str, html: &str, push_hist: bool) {`

```
/// Collect whichever half of the navigation has finished. Returns true if the
/// chrome needs redrawing — the address changed, or the stop button goes back
/// to being a reload button.
/// Ein Dokument aus dem eigenen Binaerbild an die Stelle setzen, an der sonst
/// die Antwort des Servers steht — und ab da nichts anders machen.
///
/// Der Rest der Kette (Skripte, Zeichnen, Verlauf) darf nicht wissen, woher
/// die Bytes kamen; sonst haette die Pruefseite einen eigenen Pfad und
/// pruefte am Ende diesen statt den echten.
```

## L1639-1640 · `core::ptr::addr_of_mut!(CSS_LEN).write(0);`

```
// Die Seite bringt ihr eigenes `<style>` mit und verlinkt nichts —
// das CSS der vorigen Seite muss weg, sonst stylt es diese hier.
```

## L1660-1661 · `if unsafe { npk_http_poll(h) } == 0 {`

```
// 0 = still running. Everything else (done, failed, or a handle the
// kernel no longer knows) is answered by collecting it.
```

## L1687-1688 · `file_cookies(&asked);`

```
// Before anything downstream: the cookies belong to THIS response, and
// the getters that carry them are overwritten by the next `take`.
```

## L1691-1692 · `decode_document();`

```
// The bytes are not UTF-8 just because we would like them to be, and the
// stylesheet scan below reads `html_str()`.
```

## L1696-1697 · `nav_fail(&asked);`

```
// Succeeded with nothing in it. The reader gets told, same as for a
// refusal, and `nav_fail` does the bookkeeping below itself.
```

## L1704-1706 · `let base = fetched_from().unwrap_or(asked);`

```
// Relative sub-resources resolve against the URL the document came FROM,
// not the one we asked for (RFC 3986 §5.1.3). Getting this wrong made
// every stylesheet and image repeat the document's own redirect.
```

## L1715-1718 · `fn nav_begin_stylesheets(engine: &Engine, base: &str) {`

```
/// Start the second round trip: every `<link rel=stylesheet>` of the document
/// that just landed, in ONE batch. They are render-blocking, so this is where
/// overlapping the round trips is worth the most. Bounded by CSS_CAP +
/// MAX_CSS_LINKS.
```

## L1720-1721 · `{`

```
// Eine abgebrochene Navigation darf der naechsten keine Blaetter
// hinterlassen ([[feedback_a_copy_is_a_second_semantics_waiting]]).
```

## L1732-1733 · `log(&alloc::format!("[beak] stylesheet cap hit: {} of {} linked sheets used",`

```
// Say so. A silently dropped stylesheet looks like a layout bug
// and sends the next session hunting in the engine.
```

## L1739-1740 · `if !urls.contains(&abs) {`

```
// The same sheet linked twice fetches identical bytes; dedupe on the
// resolved URL, since two different hrefs can resolve to one file.
```

## L1752-1753 · `log("[beak] stylesheet fetch could not start — rendering unstyled");`

```
// No stylesheets is not a failed page — it renders against our UA
// sheet — so this ends the navigation rather than diagnosing it.
```

## L1764-1765 · `doc_mut().nav_css_urls = Some(urls);`

```
// An `@import` resolves against ITS OWN sheet's address, not the
// document's, so the addresses have to survive the round trip.
```

## L1771-1778 · `fn js_session() -> Option<&'static mut beak_engine::js::Session> {`

```
/// How many sheets the batch in flight asked for.
/// Die Skripte der Seite in Dokumentreihenfolge, waehrend die externen noch
/// unterwegs sind. `None` heisst: keine offene Skriptrunde.
/// Die JS-Sitzung DIESER Seite.
///
/// Sie muss die Skriptrunde ueberleben: die Behandler, die ein Skript
/// anmeldet, leben in ihr, und ohne sie waere jeder `addEventListener` beim
/// Verlassen der Funktion wieder weg. Eine Navigation wirft sie weg.
```

## L1783-1798 · `const MAX_IMPORT_ROUNDS: usize = 4;`

```
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
```

## L1803 · `enum PendingScript {`

```
/// Ein Skript, das auf seinen Text wartet — oder ihn schon hat.
```

## L1805-1807 · `Ready(String, String, bool, u32),`

```
/// Quelltext, die Kennung (fuer ein Modul: seine Adresse), ob es ein
/// Modul ist, und der KNOTEN des `<script>` — er ist
/// `document.currentScript`.
```

## L1809-1810 · `Fetching(usize, String, bool, u32),`

```
/// Der Index in der Bestellung, in der Reihenfolge der Anforderung,
/// plus die Adresse — ein Fehler ohne Kennung ist keine Auskunft.
```

## L1814-1819 · `const MAX_SCRIPT_URLS: usize = 32;`

```
/// Wie viele externe Skripte eine Seite holen darf.
///
/// Nach Anzahl gedeckelt UND nach Bytes (`SCRIPT_CAP`): eine Seite mit 200
/// Bundles soll nicht 200 Rundreisen ausloesen, und eines mit 50 MB soll den
/// Puffer nicht sprengen. Der Zensus sagt, echte Seiten laden 1 bis 11
/// externe (github am meisten).
```

## L1822-1827 · `const MAX_MODULE_URLS: usize = 256;`

```
/// Wie gross ein Modulgraph werden darf, und in wie vielen Runden.
///
/// Nach Anzahl UND Runden, weil beide Enden ausufern koennen: eine Seite mit
/// tausend kleinen Modulen und eine Kette, die sich in jeder Runde ein
/// weiteres Glied holt. Gemessen: die Fritzbox-Anmeldeseite braucht 56
/// Adressen in 6 Runden.
```

## L1830-1832 · `const MAX_SHEET_ROUNDS: usize = 8;`

```
/// Wie oft eine Seite nachgeladene Stilblaetter nachlegen darf. Jede Runde
/// ist eine Rundreise; eine Seite, die in jeder Runde ein weiteres anmeldet,
/// haelt den Aufbau sonst offen.
```

## L1834-1837 · `const MAX_DYNJS_ROUNDS: usize = 12;`

```
/// Wie oft eine Seite per Skript Skripte nachlegen darf. Ein geteiltes
/// Buendel laedt seinen Baum in Ketten nach — gemessen an DuckDuckGos
/// Startseite sind es sechs Glieder —, und ohne Deckel haelt eine Seite, die
/// in jeder Runde ein weiteres anmeldet, den Aufbau offen.
```

## L1843-1845 · `let mut scratch: Vec<u8> = Vec::with_capacity(CSS_CAP);`

```
// Fetched into a scratch buffer first because the bodies come back
// concatenated, and they need a separator between them: without one, a
// sheet not ending in `}` would merge into the next sheet's first rule.
```

## L1851-1853 · `let urls = doc_mut().nav_css_urls.take().unwrap_or_default();`

```
// The bodies are kept as PARTS rather than written straight into
// `CSS_BUF`: an `@import` cascades ahead of the sheet that imported it, so
// the buffer can only be assembled once every round of imports is in.
```

## L1871-1877 · `fn css_import_pump() -> bool {`

```
/// Start one round of `@import` fetches, if any sheet still has unexamined
/// bytes. `true` when a round is in flight.
///
/// Round-based like the module graph: a sheet only names its own imports once
/// it has arrived. `sandbox.nopeek.ch` is the shape this exists for — one
/// `<link>` to a `main.css` that holds nothing but fifteen `@import`s, and
/// every one of them is the actual design.
```

## L1901-1902 · `if parts.iter().any(|(u, _, _)| *u == abs) || want.iter().any(|(_, u)| *u == abs) {`

```
// A sheet already in the list is a cycle or a repeat; either way
// its bytes are here once and that is enough.
```

## L1938-1939 · `for k in (0..want.len()).rev() {`

```
// **Von hinten einfuegen.** Jedes Einfuegen verschiebt alles dahinter;
// absteigend bleiben die noch offenen, kleineren Stellen gueltig.
```

## L1961-1962 · `fn css_assemble() {`

```
/// Write the assembled sheets into `CSS_BUF`, in the order the list holds —
/// which is cascade order, imports ahead of their importer.
```

## L1973-1974 · `fn nav_finish(engine: &Engine) {`

```
/// Document and stylesheets are both in: the page may be drawn, and its
/// images may start arriving.
```

## L1978-1980 · `if nav_begin_scripts(engine) { return; }`

```
// Erst die Skripte einsammeln. Sind externe dabei, geht die Navigation in
// eine dritte Stufe und endet erst danach — sonst waere die Seite fertig,
// bevor ihre Skripte sie gebaut haben.
```

## L1985 · `fn nav_done() {`

```
/// Die Seite ist fertig: zeichnen und Bilder holen.
```

## L1992-1995 · `fn nav_begin_scripts(engine: &Engine) -> bool {`

```
/// Die Skripte der Seite einsammeln und die externen anfordern.
///
/// Liefert true, wenn eine Rundreise laeuft — dann geht es in `nav_pump`
/// weiter. Sonst sind die Skripte schon gelaufen.
```

## L2013-2015 · `let label = if m { alloc::format!("{base}#inline{inline_n}") }`

```
// Ein eingebettetes Modul bekommt eine eigene Adresse: sie
// ist der Schluessel im Lader UND was `import.meta.url`
// sagt, und relative Angaben loesen sich dagegen auf.
```

## L2036-2037 · `log("[beak] external scripts could not be fetched — running inline only");`

```
// Nicht anforderbar: die eingebetteten laufen trotzdem. Eine Seite
// ohne ihre Bundles ist weniger als eine ganze, aber mehr als keine.
```

## L2065-2066 · `match core::str::from_utf8(bytes) {`

```
// Nicht dekodierbar heisst hier: nicht ausfuehren. Ein Skript
// halb zu lesen ist schlimmer als es zu lassen.
```

## L2083-2087 · `fn src_pos(src: &str, at: usize) -> String {`

```
/// Ein Byte-Offset als `Zeile:Spalte` plus die Umgebung im Quelltext.
///
/// Ein Fehler, der nur `@41822` sagt, kostet auf minifiziertem Fremdcode eine
/// Stunde. Die Zeile selbst wird NICHT ganz gezeigt — minifizierter Code hat
/// Zeilen von 200 KB.
```

## L2103-2118 · `fn drain_console(sess: &mut beak_engine::js::Session) {`

```
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
```

## L2127-2133 · `fn sync_cookies(sess: &mut beak_engine::js::Session) {`

```
/// Was die Seite mit `document.cookie = …` gesetzt hat, in den Behaelter —
/// und die Sicht danach neu einreichen.
///
/// Nach JEDEM Einstiegspunkt der Maschine, nicht nur nach dem Laden: ein
/// Klick setzt Kekse genauso wie ein Startskript, und wer nur einmal
/// abholt, verliert alles danach. Die Engine haelt keinen Behaelter — was
/// gilt, entscheidet `cookies`, samt Domain, Pfad und `HttpOnly`.
```

## L2145-2146 · `cookies_persist();`

```
// Ein Keks per Skript ist so dauerhaft wie einer per Kopfzeile —
// `document.cookie = "…; expires=…"` ist genau derselbe Vertrag.
```

## L2158-2173 · `fn sync_scroll(sess: &mut beak_engine::js::Session) {`

```
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
```

## L2188-2190 · `HistoryOp::Push { ref url } | HistoryOp::Replace { ref url } => {`

```
// `pushState`/`replaceState` NAVIGIEREN NICHT — sie schreiben nur
// die Adresse um. Genau das ist ihr Sinn: eine Anwendung, die
// ihre Ansicht wechselt, ohne ein Dokument zu holen.
```

## L2197-2199 · `if origin_of(&abs) != origin_of(url_str()) {`

```
// Nur die eigene Herkunft. Eine Seite darf ihre Adresszeile
// umschreiben, aber nicht auf eine fremde Herkunft — das
// waere eine Faelschung, die der Nutzer nicht sieht.
```

## L2209-2214 · `HistoryOp::Go(n) => {`

```
// `go(n)` springt WIRKLICH. Ein Zaehler, der die Absicht
// notiert und nichts tut, waere schlimmer als kein `history`:
// die Seite glaubt dann, sie sei zurueckgegangen.
//
// Mehr als einen Schritt kann beaks Verlauf nicht am Stueck —
// also so oft, wie verlangt, und wer am Ende ist, hoert auf.
```

## L2224-2225 · `nav_begin(engine, "GET", &u, &[], "", false);`

```
// `push_hist` ist hier FALSCH: sonst waechst der Verlauf
// beim Zurueckgehen, und man kaeme nie heraus.
```

## L2236-2241 · `const SCRIPT_NAV_MAX: u32 = 8;`

```
/// Wie viele Navigationen eine Seite HINTEREINANDER selbst ausloesen darf.
///
/// Nicht gegen langsame Seiten, sondern gegen `location.href = "/a"` AUF
/// `/a` — das ist eine Endlosschleife, die je Runde eine Rundreise kostet
/// und von aussen wie ein haengender Browser aussieht. Eine Kette von
/// wenigen ist dagegen normal: Googles Sperrseite braucht zwei.
```

## L2244-2254 · `fn sync_nav(engine: &Engine) -> bool {`

```
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
```

## L2258-2262 · `let ok = n.url.starts_with("https://") || n.url.starts_with("http://")`

```
// **Nur, was ein Dokument liefern kann.** Die Engine hat `javascript:`
// und `data:` schon abgelehnt; hier faellt der Rest (`mailto:`, `file:`,
// `blob:`) — nicht weil er gefaehrlich waere, sondern weil `nav_begin`
// ihn ins Netz reichen wuerde und die Antwort eine leere Seite ist, die
// aussieht wie ein Fehler der Gegenstelle.
```

## L2282-2283 · `nav_begin(engine, "GET", &n.url, &[], "", !n.replace && !n.reload);`

```
// `replace` und `reload` haengen KEINEN Eintrag an: sonst kaeme man mit
// „zurueck" nie aus einer Seite heraus, die sich selbst ersetzt.
```

## L2288-2289 · `fn run_scripts(engine: &Engine, list: Vec<PendingScript>) -> bool {`

```
/// Liefert true, wenn eine Modulrunde laeuft — dann ist die Navigation
/// NOCH nicht fertig.
```

## L2296-2299 · `sess.interp.relayout = Some(host_relayout);`

```
// **Neu auslegen auf Verlangen.** Ohne diese Zeile antwortet jede
// Kastenfrage aus dem letzten BILD, und ein Element, das ein Skript eben
// eingehaengt hat, meldet 0 — bei einer Seite, die nur EINMAL misst, fuer
// immer. Siehe `docs/plan/BROWSER_RELAYOUT_ON_DEMAND.md`.
```

## L2303-2306 · `sess.interp.set_location(url_str());`

```
// Die Adresse gehoert dem Wirt — und zwar die, aus der das Dokument KAM,
// nicht die erfragte: eine Weiterleitung aendert Herkunft und damit die
// Kekse. Ohne diese Zeile stand in `location` `about:blank`, und ein
// Skript, das seinen Pfad prueft, nahm still den falschen Zweig.
```

## L2308-2310 · `if !url_str().is_empty() {`

```
// Die Kekse, die dieses Dokument sehen DARF. `HttpOnly` bleibt draussen:
// die Fahne ist die Gegenmassnahme gegen fremden Code auf der Seite, und
// seit die Maschine Seitenskripte faehrt, gibt es fremden Code.
```

## L2315-2319 · `if let Some((_, _, w, _)) = canvas_rect() {`

```
// Der Kaskadenkontext fuer `getComputedStyle`. Ohne ihn antwortet es aus
// dem Inline-Stil — eine Teilantwort, die eine Seite laufen laesst, aber
// die falsche Auskunft gibt. Mit ihm rechnet die Maschine dieselbe
// Kaskade, die das Layout rechnet, auf demselben Baum und demselben
// Blatt.
```

## L2329-2331 · `if let Some((_, _, w, h)) = canvas_rect() {`

```
// Die Fenstergroesse gehoert dem Wirt. Ohne sie gibt es `innerWidth`
// nicht, und eine Seite, die ihre schmale Fassung danach waehlt, faellt
// mit ReferenceError aus, statt sie zu nehmen.
```

## L2333-2335 · `sess.interp.set_media(w as f64, h as f64, query_theme().is_dark());`

```
// Farbschema MIT einreichen, nicht nur die Groesse: `matchMedia`
// muss dieselbe Antwort geben wie der Kaskadenlauf, sonst waehlt das
// Skript eine Fassung, die das Layout nicht malt.
```

## L2338-2339 · `sess.interp.seed_random(now_ms() as u64 ^ 0x9E37_79B9_7F4A_7C15);`

```
// `Math.random` bekommt eine echte Saat. Ohne sie liefert jede Seite
// dieselbe Folge — und die Engine erfindet sich absichtlich keine.
```

## L2341-2343 · `sess.interp.epoch_ms = unsafe { npk_unix_time() } as f64 * 1000.0;`

```
// Und eine echte Uhr. Die Engine hat keine — ohne diese Zeile steht
// `Date.now()` bei 1970, und jede Seite, die ein Datum ausrechnet,
// rechnet falsch.
```

## L2346-2349 · `let mut entries: Vec<String> = Vec::new();`

```
// Die Modul-Einstiege, in Dokumentreihenfolge. Sie laufen NACH allen
// gewoehnlichen Skripten — `type="module"` ist per Spezifikation
// aufgeschoben, und die Fritzbox verlaesst sich darauf: ihr Modulcode
// liest `gNbc`, das ein eingebettetes Skript davor setzt.
```

## L2378-2379 · `Err(e) => match beak_engine::js::parse(src, true) {`

```
// Ein Modul ist auch ein Skript — die Datei sagt es nicht, also
// beides versuchen.
```

## L2393-2397 · `sess.interp.current_script = Some(node);`

```
// Ein Skript, das scheitert, darf die naechsten nicht mitnehmen — so
// macht es ein Browser auch.
// `document.currentScript` zeigt auf DIESEN Knoten, solange er
// laeuft. Ein Modul kommt hier nicht vorbei — dort ist die Antwort
// laut HTML §4.12.1 `null`.
```

## L2417-2421 · `const MAX_FONT_URLS: usize = 12;`

```
/// Die Schriftrunde. Eigener Auftrag, nicht der der Navigation: welche
/// Schriften eine Seite braucht, weiss die Engine erst nach dem ersten
/// Auslegen — genau wie bei Bildern.
/// Wie viele Schriften eine Seite in einer Runde holen darf, und wie viele
/// Bytes zusammen. Gemessen: eine Seite bringt 3 bis 6 mit, je 30-90 KB.
```

## L2425-2429 · `const MAX_FETCH_INFLIGHT: usize = 6;`

```
/// Wieviele `fetch`-Anfragen gleichzeitig unterwegs sein duerfen.
///
/// Nicht erfunden, sondern die Grenze des Wirts: er haelt je Anfrage einen
/// Antwortpuffer, und eine Seite, die zwanzig auf einmal stellt, band damit
/// vierzig Megabyte. Was nicht drankommt, wartet auf die naechste Runde.
```

## L2437 · `fn response_headers() -> String {`

```
/// Der Kopfblock der zuletzt eingesammelten Antwort.
```

## L2446-2459 · `fn ws_buf() -> &'static mut [u8; 16 * 1024] {`

```
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
/// Der Lesepuffer der WebSockets — einer fuer alle, nacheinander benutzt.
```

## L2462 · `unsafe { &mut *core::ptr::addr_of_mut!(BUF) }`

```
// SAFETY: beak ist einfaedig; dieselbe Regel wie bei `fetch_jobs`.
```

## L2466 · `fn ws_jobs() -> &'static mut Vec<(u32, i32)> {`

```
/// Die offenen WebSockets: `(Engine-Id, TLS-Griff)`.
```

## L2469 · `unsafe { &mut *core::ptr::addr_of_mut!(JOBS) }`

```
// SAFETY: beak ist einfaedig; dieselbe Regel wie bei `fetch_jobs`.
```

## L2473-2476 · `fn pump_websockets(sess: &mut beak_engine::js::Session) -> bool {`

```
/// **Die Leitung fahren.** Aufbauen, was ansteht; abholen, was ankam;
/// wegschicken, was die Engine hingelegt hat.
///
/// `true`, wenn etwas passiert ist — dann lohnt eine Runde Microtasks.
```

## L2479-2481 · `let want: Vec<_> = core::mem::take(&mut sess.interp.pending_sockets);`

```
// 1. Neue Verbindungen. Der Handschlag kostet ~60 ms und blockiert; das
//    ist derselbe Preis, den `open_tls` im HTTP-Weg ohnehin zahlt, und er
//    faellt einmal je Verbindung an.
```

## L2485-2487 · `log("[beak] WebSocket: ws:// ohne TLS wird nicht gefahren");`

```
// `ws://` ohne TLS gibt es hier nicht: die Engine laesst es nur
// aus einer unverschluesselten Seite zu, und dafuer fehlt der
// Klartext-Strom. Benannt statt still.
```

## L2510-2512 · `log(&alloc::format!("[beak] WebSocket: {}:{} verbunden, Handschlag raus ({} B)",`

```
// **Ein gelungener Aufbau muss sich melden.** Schweigt er, sieht ein
// Log genauso aus wie eines, in dem gar nichts versucht wurde — und
// dann ist die erste Frage nach einem Fehlschlag unbeantwortbar.
```

## L2518-2523 · `let buf = ws_buf();`

```
// 2. Lesen und schreiben. Ein Puffer je Runde reicht: was nicht
//    hineinpasst, liegt im Kernel und kommt beim naechsten Bild.
// **Nicht auf dem Stapel.** 16 KB je Aufruf hiessen 16 KB nullen, 62-mal
// in der Sekunde, fuer eine Leitung, die meistens schweigt — und es waere
// der einzige Puffer dieser Groesse auf beaks Stapel, direkt unter dem
// rekursiven Auslegen.
```

## L2527-2532 · `for _ in 0..64 {`

```
// **Leerholen, nicht anlesen.** `npk_tls_recv` gibt 0 genau dann, wenn
// nichts Vollstaendiges mehr dasteht; wer nach dem ersten Satz
// aufhoert, holt einen je Bild ab (60/s) und laesst den Rest im
// Kernel liegen, bis dessen Puffer zumacht. Der Deckel ist gegen die
// andere Richtung: eine Gegenstelle, die nicht aufhoert, darf dieses
// Bild nicht ganz auffressen.
```

## L2536 · `log("[beak] WebSocket: Leitung zu");`

```
// Zu oder kaputt — die Engine macht daraus 1006.
```

## L2588-2589 · `let from = fetched_from().unwrap_or_default();`

```
// Die Kekse gehoeren zu DIESER Antwort, und die Getter, die sie
// tragen, ueberschreibt das naechste `take`.
```

## L2604-2605 · `let mut hdrs = f.headers.replace("\r\n", "\n").trim_end_matches('\n').to_string();`

```
// Der Wirt trennt Kopfzeilen mit `\n`; die Engine haelt den rohen
// Block, wie er ueber die Leitung geht, also mit `\r\n`.
```

## L2607-2611 · `let jar = cookies::header_for(&url, unsafe { npk_unix_time() });`

```
// **Kekse fahren mit — bei GLEICHER Herkunft.** Regel K3 des Papiers,
// und die Engine laesst nur gleiche Herkunft ueberhaupt durch. Ohne
// sie kann keine angemeldete API-Schicht antworten; mit ihnen ueber
// eine Herkunftsgrenze waere es die mitreisende Vollmacht, vor der
// `BROWSER_FETCH_ORIGIN.md` §3.1 warnt.
```

## L2639-2640 · `fn pump_fonts(engine: &Engine) -> bool {`

```
/// Eine Runde an den Schriften der Seite. Liefert true, wenn etwas ankam —
/// dann muss neu ausgelegt werden, denn jede Breite aendert sich.
```

## L2664-2665 · `log(&alloc::format!("[beak]   Schrift NICHT LESBAR {url} ({n} B)"));`

```
// Kein stilles Weiterlaufen: eine Schrift, die beak nicht
// lesen kann, ist der Grund, warum die Seite anders aussieht.
```

## L2674-2675 · `if engine.load_inline_fonts() { loaded = true; }`

```
// Erst die Gesichter, deren Bytes schon im Blatt stehen — sie gehen nicht
// ins Netz und kosten keine Runde.
```

## L2693-2695 · `fn fire_load(engine: &Engine, page: &Page) -> bool {`

```
/// `load` zustellen — nach dem ersten Malen, wenn die Kaesten stehen.
///
/// Liefert true, wenn dabei etwas am Baum passiert ist.
```

## L2699-2700 · `push_control_values(page);`

```
// Was der Benutzer schon getippt hat, muss der Behandler sehen — sonst
// liest er den Vorgabewert und schreibt ihn womoeglich zurueck.
```

## L2726-2729 · `fn pump_box_observers(engine: &Engine) {`

```
/// Was `ResizeObserver`/`IntersectionObserver` gemessen haben, zustellen.
///
/// Nur wenn wirklich etwas in der Schlange liegt: auf einer Seite ohne
/// Beobachter sind das zwei leere Listen und sonst nichts.
```

## L2734-2736 · `let timers = sess.interp.run_timers();`

```
// `run_timers` faengt mit den Microtasks an, und die Zustellung sitzt
// dort — ein Rueckruf darf also selbst ein `setTimeout` anlegen und wird
// in derselben Runde bedient.
```

## L2748-2757 · `let n = doc().obs_rounds + 1;`

```
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
```

## L2770-2771 · `const OBS_ROUNDS_MAX: u32 = 8;`

```
/// Wieviele Bilder hintereinander ein Beobachter-Rueckruf den Baum aendern
/// darf, bevor das Neumalen aussetzt.
```

## L2774-2777 · `fn module_pump(engine: &Engine) -> bool {`

```
/// Eine Runde am Modulgraphen: was fehlt noch?
///
/// Liefert true, wenn eine Rundreise laeuft — dann geht es in `nav_pump`
/// weiter. Sonst ist der Graph geschlossen und alles ist ausgewertet.
```

## L2784-2785 · `let mut seen: Vec<String> = Vec::new();`

```
// Vom Einstieg aus laufen und dabei JEDE Angabe aufloesen — der Lader
// kennt nur absolute Adressen, das Aufloesen gehoert dem Wirt.
```

## L2803-2805 · `let cap = if rounds >= MAX_MODULE_ROUNDS { Some("Runden") }`

```
// WELCHER Deckel gerissen ist, gehoert in die Meldung: „nach 5 Runden"
// klang nach der Rundengrenze, obwohl die bei 24 liegt — gerissen war die
// Adressgrenze, und das ist eine ganz andere Diagnose.
```

## L2824-2825 · `if !missing.is_empty() {`

```
// Zu gross, zu tief oder fertig: auswerten, was da ist. Ein Modul, das
// fehlt, meldet sich beim Verknuepfen mit Namen.
```

## L2839-2843 · `fn sheet_pump(engine: &Engine) -> bool {`

```
/// Eine Runde an den Stilblaettern, die ein Skript eingehaengt hat.
///
/// Liefert true, wenn eine Rundreise laeuft. Wie beim Modulgraphen
/// rundenweise: ein Blatt, das ankommt, laesst eine Komponente fertig bauen,
/// und die haengt ihrerseits eins ein.
```

## L2846-2847 · `for _ in 0..8 { if sess.interp.run_timers() == 0 { break } }`

```
// Erst die Microtasks und Zeitgeber laufen lassen: was gerade fertig
// geworden ist, meldet seine Blaetter JETZT an.
```

## L2850-2851 · `if want.is_empty() { return script_pump(engine) }`

```
// Nichts mehr an Blaettern heisst nicht „fertig": danach kommen die
// Skripte, die ein Skript eingehaengt hat.
```

## L2904-2905 · `bump_content_gen("sheet");`

```
// Die Kaskade muss neu laufen — sonst haengt das Blatt im Puffer und
// wirkt nicht.
```

## L2912-2922 · `fn script_pump(engine: &Engine) -> bool {`

```
/// Eine Runde an den Skripten, die ein SKRIPT eingehaengt hat.
///
/// **Der Weg, auf dem jedes geteilte Buendel seine Stuecke nachlaedt.**
/// webpack baut ein `<script>`, haengt es an den Kopf und wartet auf dessen
/// `onload`; Next.js gibt React erst dann etwas zu rendern. Ohne Antwort
/// blieb das Versprechen offen — DuckDuckGos Startseite raeumte ihr
/// servergerendertes HTML weg und stand danach leer da, ohne eine einzige
/// Fehlerzeile.
///
/// Rundenweise wie die Blaetter: ein Stueck, das ankommt, haengt das
/// naechste ein.
```

## L2965-2966 · `let mut texts: Vec<Option<String>> = Vec::with_capacity(nodes.len());`

```
// **Erst ALLES abschreiben, dann laufen lassen.** Ein Skript, das laeuft,
// kann das naechste einhaengen — und das holt sich denselben Puffer.
```

## L2971-2972 · `let bytes = unsafe { core::slice::from_raw_parts(dst.add(off) as *const u8, n) };`

```
// SAFETY: `take_batch` hat genau diese Spanne in `IMG_FETCH_BUF`
// gefuellt und `off + n` liegt im Deckel, den wir ihm gegeben haben.
```

## L2989-3001 · `fn log_font_faces(css: &str) {`

```
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
```

## L3023-3024 · `fn css_append(bytes: &[u8]) -> bool {`

```
/// Ein Stilblatt ANHAENGEN. Spaeter geholt heisst spaeter in der Kaskade, und
/// das ist genau die Reihenfolge, in der es der Browser auch anwendet.
```

## L3064 · `fn eval_modules() {`

```
/// Die Einstiege auswerten — in Dokumentreihenfolge, jeder genau einmal.
```

## L3096-3097 · `fn finish_scripts(engine: &Engine) {`

```
/// Zeitgeber, Kekse, Baum und der Bericht — nach ALLEM, was die Seite an
/// Code hat: gewoehnliche Skripte wie Module.
```

## L3102-3121 · `let doc_node = sess.interp.doc.as_ref().map(|d| d.doc);`

```
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
```

## L3127-3129 · `let timers = sess.interp.run_timers();`

```
// Die Zeitgeber, die waehrend des Ladens angemeldet wurden, einmal
// laufen lassen — viele Seiten stellen ihre Oberflaeche in einem
// `setTimeout(…, 0)` fertig.
```

## L3138-3140 · `engine.set_hit_all(listeners || ran > 0);`

```
// Die Kaesten werden nicht nur fuer Klicks gebraucht, sondern auch
// fuer `getBoundingClientRect`. Eine Seite, die Skripte FAEHRT, kann
// danach fragen, auch wenn sie keinen Behandler angemeldet hat.
```

## L3156-3159 · `m.push_str(if listeners { ", Ereignisse SCHARF" } else { ", keine Behandler" });`

```
// Ob die Zustellung ueberhaupt scharf ist, gehoert EINMAL je Seite ins
// Log. Ohne diese Zeile ist "kein Klick kam an" nicht von "die Seite hat
// keine Behandler" zu unterscheiden — und das ist genau der Unterschied,
// den man sucht ([[feedback_the_fast_path_must_say_it_ran]]).
```

## L3164-3166 · `fn push_control_values(page: &Page) {`

```
/// Die zwei Richtungen der Formular-Bruecke. Die REGEL steht in der Engine
/// (`dombind::push_control_values` / `pull_control_values`) — hier stehen nur
/// die Ausleihen, damit es die Regel nicht zweimal gibt.
```

## L3178-3180 · `fn dispatch_click(engine: &Engine, page: &mut Page, lay: &Layout, cx: i32, cy: i32) -> bool {`

```
/// Einen Klick an die Seite zustellen. Liefert true, wenn ein Behandler
/// `preventDefault` gerufen hat — dann unterbleibt, was beak sonst getan
/// haette (einem Link folgen, ein Steuerelement bedienen).
```

## L3182-3183 · `arm_script_budget();`

```
// Jeder Behandler bekommt sein eigenes Zeitbudget — sonst zahlt der
// zwanzigste Klick fuer die neunzehn davor.
```

## L3185 · `push_control_values(page);`

```
// Was der Benutzer getippt hat, muss der Behandler sehen.
```

## L3188-3189 · `let chain = lay.element_chain(cx, cy);`

```
// Die seq-Kette unter dem Zeiger, vom aeussersten zum innersten. Das
// Layout gibt sie schon aus — dieselbe Liste, aus der `:hover` lebt.
```

## L3195-3199 · `let on_label = nodes.last().is_some_and(|n| {`

```
// **Ein Schild aktiviert sein Kaestchen auch OHNE Skript.** Der Schnellweg
// hier springt ab, wenn die Seite keinen Behandler hat — richtig fuer
// Ereignisse, falsch fuer eingebautes Verhalten: die Klickflaeche eines
// Kaestchens ist der Text daneben, und eine Seite ganz ohne JS hat ihn
// genauso.
```

## L3205-3208 · `let sy = scroll_y() as f64;`

```
// **Mit dem ORT.** `cx`/`cy` kommen als Fenster-x und Dokument-y herein
// (`cy` hat den Rollstand schon drin); `client*` will beides
// fensterbezogen, `page*` beides dokumentbezogen. Waagrecht rollt beak
// nicht, also sind die zwei x gleich.
```

## L3217-3218 · `let changed = sess.interp.doc.as_ref().is_some_and(|d| d.dirty);`

```
// NUR wenn sich etwas geaendert hat. Ein Behandler, der bloss zaehlt,
// darf keine 130 ms Layout kosten.
```

## L3227-3229 · `page.sync(engine);`

```
// Das Formularmodell und die Werte nachziehen, BEVOR ein Absende-Auftrag
// ausgefuehrt wird — sonst schickt er den Stand von vorher.
// Nach einem Behandler: der Baum weiss jetzt mehr als der Wirt.
```

## L3238-3241 · `if sync_nav(engine) { return true }`

```
// Ein Behandler, der `location.href` setzt, hat damit gesagt, wohin es
// geht. Danach auch noch dem angeklickten Link zu folgen hiesse, zwei
// Navigationen aus einem Klick zu machen — also gilt der Klick als
// behandelt.
```

## L3258-3264 · `fn random_bytes(out: &mut [u8]) -> bool {`

```
/// Zufall aus dem Kernel in einen Puffer. `false`, wenn der Kernel abgelehnt
/// hat — der Rufer wirft dann, statt schwachen Zufall zu liefern.
///
/// In Stuecken, weil der Kernel 64 KiB je Aufruf deckelt (er haelt dabei den
/// RNG-Mutex). Der Motor deckelt ohnehin bei derselben Zahl; die Schleife
/// steht hier, damit dieselbe Funktion auch einen groesseren Puffer bedienen
/// koennte, ohne still die Haelfte ungefuellt zu lassen.
```

## L3267-3268 · `let n = unsafe { npk_random_bytes(teil.as_mut_ptr() as i32, teil.len() as i32) };`

```
// SAFETY: der Kernel schreibt hoechstens `len` Bytes ab `ptr`, und
// beides beschreibt genau dieses Stueck.
```

## L3275-3284 · `const SCRIPT_STEPS: u64 = 20_000_000_000;`

```
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
```

## L3287-3308 · `const SCRIPT_BUDGET_MS: i64 = 900_000;`

```
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
```

## L3311-3313 · `const SCRIPT_SLOW_MS: i64 = 3_000;`

```
/// Ab wann ein Lauf im Log auffaellt. Ein Skript, das Minuten rechnet, ist
/// kein Fehler — aber es ist der Grund, warum nichts passiert, und das
/// gehoert gesagt, statt es aus einem Zeitstempel raten zu lassen.
```

## L3316 · `const SCRIPT_HEARTBEAT_MS: i64 = 5_000;`

```
/// Wie oft ein langer Lauf von sich hoeren laesst.
```

## L3319-3329 · `static mut SCRIPT_DEADLINE: i64 = 0;`

```
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
```

## L3331-3332 · `static mut BUDGET_T0: i64 = 0;`

```
/// Wann der laufende Behandler begann — fuer den Herzschlag. Eigener Name
/// neben `Doc::script_t0`: das ist der Beginn der SEITENrunde, nicht des Laufs.
```

## L3336-3345 · `fn script_time_left() -> bool {`

```
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
```

## L3363 · `fn arm_script_budget() {`

```
/// Die Uhr neu stellen — vor jedem Lauf von Seitencode.
```

## L3373-3380 · `fn begin_images(engine: &mut Engine) -> Vec<String> {`

```
/// Start a page's image load: drop the old pixels and return the list of
/// sources still to fetch. Touches the network NOT AT ALL, so the first paint
/// can happen right after it.
///
/// The same src repeats all over a real page (icons, bullets, a logo in header
/// and footer). The engine keys decoded images by src, so a repeat only
/// re-fetched and re-decoded identical bytes — wasted requests against the
/// server's rate limit, and wasted MAX_IMAGES slots that real images needed.
```

## L3383-3384 · `engine.set_marks(None, Vec::new());`

```
// Der Motor haelt die Hervorhebungen; eine neue Seite hat keine.
// `set_url` raeumt sie im Dokument weg, hier faellt der Anstrich nach.
```

## L3387-3391 · `if selftest::matches(url_str()) {`

```
// **Eine eingebaute Seite holt nichts aus dem Netz.** Ihre Bilder stehen
// relativ zu `beak:selftest`, und daraus wird beim Aufloesen ein
// RECHNERname `beak` — im Geraetelauf vom 2026-09-17 stand woertlich
// `dns: beak: gibt es nicht`. Ein erfundener Name, der an den Aufloeser
// geht, ist kein Schoenheitsfehler: er verlaesst das Geraet.
```

## L3396-3398 · `let vw = canvas_rect().map(|(_, _, w, _)| w as u32).unwrap_or(1280);`

```
// The SAME viewport width layout uses: `<picture>`/`srcset` picks its
// candidate per media query, so fetching at a different width would fetch
// a URL the page never asks for and leave the real one blank.
```

## L3400-3403 · `let all = engine.image_srcs_now(html_str(), vw);`

```
// **Aus dem Baum, den das LAYOUT benutzt**, nicht aus dem urspruenglichen
// HTML. Eine Seite, die ihren Inhalt per Skript baut, hat dort keine
// Bilder stehen — DDGs Ergebnisseite ist eine Huelle, und ihre Karte wie
// ihre Seitensymbole kamen deshalb nie zur Anfrage.
```

## L3405-3407 · `if !all.is_empty() {`

```
// **Die Sammelstelle sagte bis hierher nicht, was sie gesammelt hat.**
// Damit sah „auf der Seite steht kein Bild" genauso aus wie „ich habe im
// falschen Baum nachgesehen" — und genau das war es einmal.
```

## L3421-3427 · `let pairs: Vec<(String, String)> =`

```
// Serve what the last pages already decoded, BEFORE the first layout.
// That is where it pays twice: no request, no decode — and the box is
// DEFINITE on the very first layout instead of being guessed and moving
// the page a second later.
//
// Keyed by the resolved url, because the `src` attribute alone is
// ambiguous across sites (`/logo.png`).
```

## L3437 · `bump_content_gen("images-begin"); // lay out with placeholders`

```
// lay out with placeholders
```

## L3442-3458 · `fn pump_images(`

```
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
```

## L3468 · `return; // still on the wire — come back next turn`

```
// still on the wire — come back next turn
```

## L3479-3486 · `if let Some(l) = layout {`

```
// Layout-affecting first. An image whose box was GUESSED moves the page
// when it lands, and that costs a FULL re-layout wherever it sits —
// measured on the device: 1110-1710 ms on an article, against ~540 ms for
// the whole page's image traffic. Fetching it in the FIRST batch pays that
// once, immediately, instead of after two repaints the re-layout then
// throws away. On de.wikipedia/Stansstad exactly ONE `<img>` of 17 is such
// a box (a MediaWiki timeline, no width/height); the Hauptseite has none,
// which is why only the article ever showed the jump.
```

## L3489 · `pending.sort_by_key(|s| !l.guessed_image_srcs.iter().any(|g| g == s));`

```
// Stable, so document order survives inside each group.
```

## L3498-3499 · `log(&alloc::format!("[beak] image batch of {} could not start", urls.len()));`

```
// Could not even be asked for. These keep their placeholders rather
// than being retried every turn for as long as the page stays open.
```

## L3521-3523 · `log(&alloc::format!("[beak] image not delivered (request failed or over {} KiB buffer) — {}",`

```
// Nicht stillschweigend: eine Anfrage, die scheiterte, und eine,
// deren Antwort nicht in den Puffer passte, sehen hier gleich aus
// — und die zweite ist ein Deckel von UNS.
```

## L3529-3530 · `if let Err(why) = engine.add_image_cached(src, url, bytes) {`

```
// Decode now, drop the compressed bytes — and keep the pixels under
// their url so the next navigation to this page needs neither.
```

## L3532-3534 · `log(&alloc::format!("[beak] image dropped ({n} B): {why} — {src}"));`

```
// Der Grund gehoert IN die Meldung. „nicht dekodierbar oder ueber
// dem Budget" schickte eine ganze Sitzung hinter einen
// JPEG-Dekoder her, der nie schuld war — es war das Budget.
```

## L3543-3548 · `log(&alloc::format!("[beak] Bilder: {} von {} dekodiert{}",`

```
// **Ein geglueckter Lauf war stumm**, und damit sah eine Runde, in der
// sieben Bilder ankamen, im Log genauso aus wie eine, in der keines
// angefragt wurde. Genau das hat einen ganzen Geraetelauf gekostet: die
// Frage „werden sie geholt?" liess sich nur an den `h2`-Zeilen des Wirts
// ablesen, und ob sie DEKODIERT wurden, gar nicht
// ([[feedback_the_fast_path_must_say_it_ran]]).
```

## L3556-3558 · `bump_content_gen("image-arrived");`

```
// A guessed box moves once the real size lands: the page below it
// shifts and the scroll extent changes, so this one must re-lay-out
// wherever it sits.
```

## L3563-3568 · `match layout {`

```
// Pure repaint. Ask first whether it would show anything: measured on the
// Hauptseite, ONE navigation paid eight full-viewport repaints (~50 ms
// each) for image batches, and the page is 3421 px tall against a ~1000 px
// viewport — most of those pictures were below the fold and could not
// change a pixel. Scrolling marks the page dirty on its own, so nothing
// is lost; it is drawn the moment it can be seen.
```

## L3575-3580 · `fn log_image_miss(engine: &Engine) {`

```
/// Wonach das letzte Malen vergeblich suchte — EINMAL je Adresse.
///
/// Der Platzhalter ist stumm, und damit sieht „nie angefragt" genauso aus wie
/// „geholt, dekodiert, und beim Malen unter einem anderen Schluessel gesucht".
/// Die Zeile nennt beide Haelften: die gesuchte Zeichenkette und wie viele
/// Bilder der Speicher ueberhaupt haelt.
```

## L3593-3606 · `fn pump_css_images(`

```
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
```

## L3656 · `continue; // the box stays undecorated`

```
// the box stays undecorated
```

## L3667-3668 · `match layout {`

```
// No `moved` case here at all — a background can never change geometry —
// so the visibility question is the only one.
```

## L3675 · `fn img_job() -> i32 {`

```
// ── Sub-resource batches in flight ────────────────────────────────────────
```

## L3677 · `fn img_job() -> i32 {`

```
/// The `<img>` batch on the wire and the `(src, resolved url)` pairs it was
```

## L3688-3691 · `fn subresources_cancel() {`

```
/// Drop the sub-resource batches of a page that is being replaced. A browser
/// that keeps fetching the pictures of the page you just left spends the
/// network on nothing — and with one kernel fetch queue behind them, it also
/// makes the new document wait its turn.
```

## L3703-3706 · `let d = doc_mut();`

```
// Und was noch gar nicht gefragt wurde. **Die Schlange gehoert der Seite,
// die ersetzt wird**: sie stehen zu lassen hiesse, dass der neue Abruf
// sich die eine Kernel-Schlange mit den Bildern der alten Seite teilt —
// und ihre relativen Adressen wuerden gegen die NEUE Basis aufgeloest.
```

## L3713-3717 · `fn fetch_url(engine: &Engine, url: &str) {`

```
/// Set the address + start fetching, WITHOUT touching history (reload,
/// back/forward — those addresses are already in it).
///
/// A failure is not silent: `nav_fail` puts a diagnostic page in the document
/// and logs the reason, so there is nothing to add here.
```

## L3723-3725 · `fn nav_goto(engine: &Engine, url: &str) {`

```
/// The same, but the address we LAND on joins the history — a click, a typed
/// address, a form. Recorded when the document arrives, not now: recording
/// where we aimed would make every trip back replay the redirect.
```

## L3731 · `fn post_url(engine: &Engine, url: &str, body: &[u8]) {`

```
/// Navigate by POSTing `body` to `url` (a form with `method=post`).
```

## L3741 · `fn go(engine: &Engine, typed: &str) {`

```
/// Navigate the address bar's typed text (normalise scheme) — new entry.
```

## L3748-3749 · `fn typed_to_url(typed: &str) -> Option<String> {`

```
/// Was die Adresszeile MEINT: eine Adresse, oder eine Suche daraus.
/// `None` heisst „nichts eingegeben".
```

## L3764 · `let mut s = String::from(SEARCH_URL);`

```
// Not an address → search the web for it (omnibox).
```

## L3774 · `fn tab_label(d: &Doc) -> String {`

```
// ── Tabs ───────────────────────────────────────────────────────────────
```

## L3776-3781 · `fn tab_label(d: &Doc) -> String {`

```
/// Womit ein Tab beschriftet wird: der Titel, sonst der WIRT der Adresse,
/// sonst „Neuer Tab".
///
/// Der Wirt und nicht die ganze Adresse. Ein Etikett wird hinten
/// abgeschnitten, und bei Adressen ist vorne alles gleich — fuenf Tabs auf
/// Wikipedia waeren fuenfmal `https://de.wikipedia.org/wi…`.
```

## L3791-3794 · `if full.chars().count() <= TAB_CHARS {`

```
// **Selbst kuerzen, denn sonst tut es niemand.** Der Compositor malt
// Text vom linken Rand seines Kastens aus und klemmt ihn nicht:
// `MaxWidth` begrenzt die Kiste, nicht die Glyphen. Was nicht
// hineinpasst, steht im NACHBARtab.
```

## L3803-3813 · `fn tab_freeze() {`

```
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
```

## L3815-3817 · `nav_cancel();`

```
// Zuerst die Griffe: sie gehoeren dem Wirt, nicht dem Speicher, den wir
// gleich fallen lassen. Ein Stapel, der weiterlaeuft, nimmt der Seite,
// auf die gewechselt wird, die eine Abrufschlange weg.
```

## L3840-3841 · `d.scroll_want = y;`

```
// Wo der Leser stand. `scroll_y` waere die falsche Stelle: das Laden
// setzt sie auf 0, und zwar zu Recht — eine neue Seite faengt oben an.
```

## L3845-3849 · `fn blank_document(engine: &Engine) {`

```
/// Ein leerer Tab.
///
/// **Die Puffer muessen wirklich leer werden.** `HTML_BUF`/`CSS_BUF` gehoeren
/// dem Programm, nicht der Seite; ein neuer Tab, der den Text des alten
/// zeigt, waere genau die Verwechslung, gegen die `Doc` gebaut wurde.
```

## L3861-3864 · `fn tab_load(engine: &Engine, cache: &mut Option<(Layout, i32, i32, u32)>, page: &mut Page) {`

```
/// Die Seite des laufenden Tabs holen — der Weg zurueck aus dem Einfrieren.
///
/// Ein Tab ohne Verlauf legt seinen ersten Eintrag an; ein zurueckkehrender
/// nicht, denn er steht schon darin.
```

## L3866-3868 · `let u = doc().url.to_string();`

```
// Der Zwischenspeicher haelt das Layout der vorigen Seite, das
// Formularmodell ihre Steuerelemente. Beide haengen an Zaehlern, die JE
// DOKUMENT laufen — nach einem Wechsel sagt ein Vergleich nichts mehr.
```

## L3883 · `fn tab_activate(engine: &Engine, i: usize, cache: &mut Option<(Layout, i32, i32, u32)>,`

```
/// Auf einen anderen Tab umschalten.
```

## L3895-3900 · `fn tab_open(engine: &Engine, url: &str, background: bool,`

```
/// Einen Tab oeffnen. `background` legt ihn nur AN.
///
/// Ein Hintergrundtab holt nichts: geholt wird, wenn ihn jemand ansieht. Der
/// Grund steht im Kernel — `WORKER_COUNT = 1`, es laeuft ein Abruf zur Zeit,
/// und ein Tab, den niemand liest, nimmt der Seite vor den Augen des Lesers
/// die Leitung weg.
```

## L3921 · `fn tab_close(engine: &Engine, i: usize, cache: &mut Option<(Layout, i32, i32, u32)>,`

```
/// Einen Tab schliessen.
```

## L3928-3930 · `if n == 1 {`

```
// **Der letzte Tab IST das Fenster.** So macht es jeder Browser, und die
// Gegenrichtung waere ein Strg+W, das nichts tut — also ein Fenster, das
// sich mit der Tastatur nicht schliessen laesst.
```

## L3939 · `tab_freeze();`

```
// Die Griffe zurueck, bevor das Dokument faellt.
```

## L3951-3953 · `const SEARCH_URL: &str = "https://marginalia-search.com/search?query=";`

```
/// A typed address that is not a URL becomes a web search. Marginalia is the
/// one engine that serves real results to a no-JS client (the others gate on
/// browser fingerprinting — see the 2026-07-20 recon).
```

## L3956 · `fn looks_like_url(t: &str) -> bool {`

```
/// Does this look like an address rather than a search phrase?
```

## L3964 · `let host = t.split(['/', '?', '#']).next().unwrap_or(t);`

```
// A dotted host (example.com, de.wikipedia.org/wiki/X) or localhost.
```

## L3969-3971 · `fn submit_form_seq(engine: &Engine, page: &Page, form_seq: u32) -> bool {`

```
/// Submit a form. GET puts the data in the query string, POST in the request
/// body — the same encoding either way (HTML §4.10.21.3).
/// Ein Formular, das die SEITE abschicken will (`form.submit()`).
```

## L3983-3991 · `let form_seq = activated.or(page.state.focus)`

```
// **Erst das `submit`-Ereignis.** Eine Seite rechnet in ihrem Behandler
// aus, was sie mitschickt — ein Kennwort-Hash, ein Zeitstempel, ein
// Token — und darf abbrechen. Ohne diesen Schritt schickte beak das
// Formular so ab, wie es im Baum stand: die berechneten Felder leer, und
// die Gegenseite antwortet mit „falsches Kennwort".
//
// Nur auf dem BENUTZERweg. `form.submit()` aus einem Skript feuert laut
// Spezifikation kein `submit` — sonst liefe der Behandler der Seite ein
// zweites Mal.
```

## L3995-3999 · `let node_of = |s: Option<u32>| -> Option<u32> {`

```
// Die Steuerelemente, um die es geht, ueber ihre BAUMknoten festhalten:
// der Behandler der Seite darf umbauen, und dann heissen sie anders
// (`to_dom` nummeriert neu). Ohne das zeigte `activated` nach dem
// Behandler auf ein anderes Element — oder auf keines, und dann wurde
// gar nichts abgeschickt.
```

## L4008-4010 · `let ts0 = now_ms();`

```
// VOR dem Behandler. Er kann Minuten rechnen (eine Anmeldung, die
// ihren Hash selbst macht), und bis dahin sah der Benutzer nichts —
// weder dass die Eingabe angekommen ist noch dass etwas laeuft.
```

## L4015-4017 · `let n = sess.interp.run_timers();`

```
// Abgebrochen. Der Behandler hat oft trotzdem etwas vor —
// ein `setTimeout`, ein Versprechen — also laufen lassen und
// den Baum nachziehen.
```

## L4030-4031 · `let n = sess.interp.run_timers();`

```
// Nicht abgefangen — aber der Behandler kann Felder gefuellt
// haben, und die gehoeren in die Eingabe.
```

## L4049 · `let seq_of = |n: Option<u32>| -> Option<u32> {`

```
// Und zurueck: derselbe Knoten, seine JETZIGE Nummer.
```

## L4060-4069 · `None => {`

```
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
```

## L4084-4085 · `fn send_submission(engine: &Engine, sub: forms::Submission) {`

```
/// Eine fertige Eingabe abschicken — GET haengt sie an die Adresse, POST in
/// den Rumpf. Eine Fassung fuer beide Wege dorthin (Knopf und `submit()`).
```

## L4087-4088 · `let base = url_str().to_string();`

```
// An empty action targets the current document; either way the form data
// REPLACES the action's query string (HTML §4.10.21.3 "mutate action URL").
```

## L4099 · `let target = if action.contains('?') { action.clone() } else { url.clone() };`

```
// A POST keeps the action's own query string — only a GET replaces it.
```

## L4105 · `fn follow(engine: &Engine, href: &str) {`

```
/// Follow a link href relative to the current page — new history entry.
```

## L4112 · `const HIST_MAX: usize = 64;`

```
// ── Back/forward history (fixed-size static ring of URLs) ──────────────────
```

## L4119-4125 · `fn hist_push(url: &str) {`

```
/// Record a new navigation: truncate forward entries, append (caps at HIST_MAX).
///
/// **Verhalten unveraendert uebernommen**, auch die Ecke am Deckel: ist die
/// Liste voll, wird der LETZTE Eintrag ueberschrieben und die Stelle bleibt
/// stehen. Ein Umbau ist der falsche Ort, um nebenbei eine Regel zu aendern
/// — was hier steht, muss sich genauso verhalten wie vorher, sonst misst
/// kein Test mehr den Umbau.
```

## L4139-4140 · `d.hist.truncate(new_pos);`

```
// Vorwaerts-Eintraege fallen weg — dasselbe, was das feste Feld tat,
// indem es `HIST_COUNT` auf `new_pos + 1` zurueckschrieb.
```

## L4145-4151 · `fn hist_back() -> Option<&'static str> {`

```
// **Jede Entleihung endet in ihrer eigenen Anweisung.** `doc_mut` gibt ein
// `&'static mut` heraus; zwei davon gleichzeitig — oder eines neben einem
// `doc()` — sind Aliasing, und Aliasing auf `&mut` ist kein Stilfehler,
// sondern undefiniert. Deshalb steht hier `doc().hist_pos` lesen, DANN
// schreiben, DANN `hist_get` rufen, statt eine Referenz ueber alles drei zu
// halten. Der Rechner merkt es nicht — die Referenz kommt aus einem
// `unsafe`-Deref und faellt aus seiner Buchhaltung.
```

## L4174-4187 · `fn resolve(base: &str, href: &str) -> String {`

```
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
```

## L4192 · `return href.to_string();`

```
// Ohne brauchbare Grundlage bleibt nur die Angabe selbst.
```

## L4198-4199 · `fn canvas_rect() -> Option<(i32, i32, i32, i32)> {`

```
/// Query the canvas widget's actual laid-out rect (x, y, w, h) in the app's
/// window space. `None` until the compositor has laid it out at least once.
```

## L4210-4213 · `fn px_set(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, bgr: [u8; 3]) {`

```
/// Re-layout + paint the visible slice into the canvas if it's dirty or the
/// viewport resized. Paints only the viewport (bounded memory, any page
/// length — long one-pagers just scroll).
/// Set one BGRA pixel (bounds-checked).
```

## L4227 · `fn stroke_rect_bgra(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, bgr: [u8; 3]) {`

```
/// Stroke a 2px rectangle border into a BGRA buffer — the inspect highlight.
```

## L4247-4261 · `fn host_relayout(ip: &mut beak_engine::js::interp::Interp) {`

```
/// **Ein frisches Layout, mitten im Skript.**
///
/// Gerufen aus `Interp`, wenn eine Seite den Kasten eines Elements liest, das
/// sie im selben Schritt eingehaengt hat. Derselbe Weg, den die Bildschleife
/// je Bild faehrt — Baum zurueckschreiben, auslegen, Kaesten einreichen —
/// nur jetzt auf Verlangen.
///
/// **Was er NICHT tut: den Zwischenspeicher der Schleife fuellen.** Der liegt
/// in `main`, und ein `fn`-Zeiger kommt nicht daran. Das naechste Bild legt
/// also noch einmal aus — aber das haette es ohnehin getan, denn der Baum hat
/// sich geaendert und `content_gen` steht schon weiter. Der Preis ist genau
/// dieses eine erzwungene Layout, nicht zwei.
///
/// Den Deckel gegen Layout-Thrashing haelt die Maschine (`FORCED_LAYOUT_CAP`);
/// hier steht nur die Arbeit.
```

## L4268-4270 · `bump_content_gen("relayout");`

```
// Der Schleife sagen, dass ihr Zwischenspeicher alt ist. Ohne das haelt
// sie ihn fuer gueltig, weil `to_dom` eine Zeile weiter oben `dirty`
// geloescht hat.
```

## L4272-4274 · `let forms = engine().last_forms();`

```
// Die getippten Werte des Benutzers, wie beim letzten Auslegen. Mit
// `FormState::default()` waere ein Feld mit Text schmaler, und genau
// diese falsche Zahl ginge an die Seite zurueck.
```

## L4301-4309 · `let cur_gen = content_gen();`

```
// (Re)lay out only when the content or the viewport changed — NOT on every
// scroll. Reusing the cached layout for scroll is what keeps scrolling
// smooth. The HEIGHT counts only when the page's geometry actually depends
// on it — a `vh` length that won the cascade, a cap that really clamps, an
// out-of-flow box anchored to the viewport's bottom edge. `html, body
// { height: 100% }` is on nearly every site and moves nothing, so it must
// not count: the dock sliding this window up a few pixels was costing a
// full re-layout (~6.4 s on a big article) for a picture that could not
// change.
```

## L4319-4322 · `let t = engine.title().unwrap_or_default();`

```
// Der Titel fuer den Streifen. **Hier und nicht beim Ankommen des
// Dokuments:** der Motor parst beim AUSLEGEN, also haelt er bis
// hierher noch den Baum der vorigen Seite — ein Tab haette den Namen
// der Seite getragen, von der man kam.
```

## L4328 · `let boxes = cache.as_ref().unwrap().0.element_rects();`

```
// Die Kaesten neu einsammeln — nur hier, nicht je Bild.
```

## L4330-4332 · `doc_mut().geom = Some(alloc::rc::Rc::new(boxes));`

```
// Zuweisung, nicht `ptr::write`: die ueberschreibt OHNE den alten Wert
// fallen zu lassen, und das waren ~180 KB Kaesten je Neuauslegung, die
// nie zurueckkamen.
```

## L4335-4344 · `if doc().sel.is_some() {`

```
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
```

## L4359-4361 · `let geom = doc().geom.clone();`

```
// Geometrie und Rollstand an die Maschine reichen. Der Rollstand geht bei
// jedem Bild mit, weil er sich ohne Layout aendert; die Kaesten sind ein
// `Rc` und kosten dabei nichts.
```

## L4364-4369 · `let vp = (w, h);`

```
// Das Sichtfeld nachziehen, wenn es sich bewegt hat. Der Ausschnitt
// eines `IntersectionObserver` ohne eigene Wurzel IST das Sichtfeld —
// und `set_media` laeuft nur einmal beim Skriptstart, also stand hier
// nach jeder Fensteraenderung die Zahl von damals. Nur bei
// Aenderung, weil `set_viewport` ein frisches `screen` baut und das
// je Bild Muell waere.
```

## L4377-4381 · `content: (w, layout.height as i32),`

```
// Die Rollflaeche, wie das Layout sie ausgerechnet hat — dieselbe
// Zahl, gegen die der Wirt zwei Zeilen weiter oben `max_scroll`
// klemmt. `document.documentElement.scrollHeight` MUSS dieselbe
// sagen, sonst rechnet eine Seite mit einer Hoehe, an die sie nie
// rollen kann.
```

## L4386-4389 · `let need = (w as usize) * (h as usize) * 4;`

```
// Reuse a persistent paint buffer across frames — `engine.paint` fills every
// pixel (background first), so no re-zeroing is needed. A fresh
// `vec![0; w*h*4]` per frame was a ~5 MB alloc+zero+free on EVERY scroll
// repaint (heap churn + latency).
```

## L4396-4402 · `let dy = sy - unsafe { core::ptr::addr_of!(LAST_SY).read() };`

```
// Scrolling does not change the page; it moves it. When nothing else asked
// for a repaint, shift the pixels that merely moved and draw only the band
// that came into view — 1902x1000 is 7,6 MB of fill, ~60-80 ms on the
// device, and a scroll exposes a few dozen rows of it.
//
// The inspect overlay is drawn OVER the frame rather than being part of the
// display list, so a blit would smear it; that mode takes the full path.
```

## L4409-4412 · `if dy == 0 && !full {`

```
// A scroll that the clamp swallowed — at the top or the bottom of the page
// the offset does not move, so the buffer already holds this exact frame.
// Repainting it was 60-80 ms for a picture that cannot differ, and holding
// the wheel at the foot of an article does it every turn.
```

## L4423 · `buf.copy_within(moved * stride..rows * stride, 0);`

```
// Scrolled down: the picture moves UP, the new band is at the foot.
```

## L4428 · `buf.copy_within(0..(rows - moved) * stride, moved * stride);`

```
// Scrolled up: the picture moves DOWN, the new band is at the head.
```

## L4435 · `log_image_miss(engine);`

```
// Was das Malen nicht gefunden hat — einmal je Adresse.
```

## L4437 · `if inspect_mode() {`

```
// Inspect overlay: outline the selected element box (document → screen).
```

## L4445-4446 · `if full {`

```
// Say WHICH path ran. A fast path that never says so looks exactly like one
// that never happened, and the whole point of this one is a number.
```

## L4454-4458 · `{`

```
// The number that matters: navigation → first pixels.
//
// Not while one is still in the air: the OLD page keeps repainting for
// scrolls and hovers during a load now, and reporting one of those would
// credit the new navigation with a picture of the previous page.
```

## L4463-4466 · `let mut m = String::from("[beak] Schriften: ");`

```
// Wieviele der sechs eingebauten Gesichter diese Seite wirklich
// gebraucht hat. Sie werden faul geladen, und ohne diese Zahl ist
// „faul" eine Behauptung: eine Seite, die doch alle sechs
// anfasst, spart nichts, und man saehe es nicht.
```

## L4483-4500 · `fn address_spans(field: &str, url: &str) -> Vec<Span> {`

```
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
```

## L4505-4506 · `let after_scheme = match field.find("://") { Some(i) => i + 3, None => 0 };`

```
// Der Host: hinter `schema://`, bis zum ersten `/?#`, ohne `benutzer@`
// und ohne `:port`.
```

## L4513-4514 · `let host = match host_raw.rfind(':') {`

```
// Ein Doppelpunkt trennt den Port — aber in `[::1]` gehoert er zur
// Adresse, und eine IP hat ohnehin keine registrierbare Domain.
```

## L4520-4521 · `if host.len() < dom.len() { return Vec::new() }`

```
// `registrable_domain` gibt kleingeschrieben zurueck; gesucht wird im
// ORIGINAL, und sie ist immer ein Endstueck des Hosts.
```

## L4535 · `fn render_chrome() {`

```
/// framed address bar) · canvas body · the open dropdown as a Popover.
```

## L4553-4557 · `let url = url_str();`

```
// A lock in Success for https, the bird for anything else — the
// scheme belongs in the field, not in the URL text (docs/spec/UI_REFRESH.md §5).
// Das Schloss gehoert dem GELADENEN Dokument, der Text dem Feld: waehrend
// jemand tippt, sagt das Schloss weiter die Wahrheit ueber die Seite, die
// dasteht.
```

## L4594-4595 · `Modifier::Focus(vec![`

```
// 1 px accent border plus a 3 px ring — the design's
// `text_field` focus state.
```

## L4607-4609 · `if nav_busy() {`

```
// One button, two jobs: reload when the page is settled, stop
// while it is loading. It is also the only thing on screen that
// says a fetch is running at all.
```

## L4639-4642 · `if let Some(q) = doc().find.clone() {`

```
// Die Suchleiste. **Kein `Widget::Input`, sondern Text** — der Puffer
// gehoert beak, siehe `Doc::find`. Sie steht UNTER der Leinwand wie in
// jedem Browser: oben waere sie ein Werkzeug, unten ist sie eine
// Randnotiz, und genau das ist sie.
```

## L4647 · `label.push('\u{2502}');           // ein stehender Strich als Schreibmarke`

```
// ein stehender Strich als Schreibmarke
```

## L4675 · `if inspect_mode() {`

```
// Inspect status bar: the selected element's label, or a hint.
```

## L4716 · `const TOOLBAR_H: u16 = 44;`

```
/// Toolbar chrome sizing (docs/spec/UI_REFRESH.md §5).
```

## L4722-4723 · `const TABSTRIP_H: u16 = 36;`

```
/// Der Streifen: 36 px, `SurfaceElevated`, Tabs unten buendig
/// (`docs/spec/UI_REFRESH.md` §5.2 und §3 `tab`).
```

## L4725 · `const TAB_ACCENT: u16 = 2;`

```
/// Der Akzentstreifen oben auf dem aktiven Tab.
```

## L4728-4730 · `const TAB_W: u16 = 160;`

```
/// **Feste Breite, nicht mitwachsend** (§3 `tab`). Ein Tab, der sich der
/// Anzahl anpasst, braucht ein Etikett, das mitgeht — und die Schrift gehoert
/// dem Compositor, eine App kann sie nicht messen.
```

## L4732-4737 · `const TAB_BTN: u16 = 22;`

```
/// **22, damit ein 16-px-Zeichen hineinpasst.** Der Atlas fuehrt 16, 24, 32,
/// 48 und 64 — eine Anfrage auf 12 bekam das 16er und wurde auf 4:3
/// verkleinert. Das Verkleinern mittelt korrekt ueber Flaechen, und genau
/// deshalb wird ein 1,5 px breiter Strich dabei weich: Florian am Geraet,
/// „das x symbol malt bisschen unscharf". Eine Atlasgroesse zu verlangen ist
/// ein 1:1-Blit und damit scharf.
```

## L4739-4744 · `const TAB_CHARS: usize = ((TAB_W - 16 - TAB_BTN - 4) / 7) as usize;`

```
/// Wieviel Text in einen Tab passt, in Zeichen.
///
/// Gerechnet mit 7 px je Zeichen gegen `TextStyle::Body` (13 px). Das ist
/// eine SCHAETZUNG und absichtlich zu hoch: ein zu kurzes Etikett ist ein
/// Etikett, ein zu langes ist ein Fehler — der Compositor schneidet Text
/// nicht ab, er malt ihn ueber den Nachbarn.
```

## L4747 · `fn tab_btn(icon: IconId, action: ActionId) -> Widget {`

```
/// Ein kleiner Knopf im Streifen: das `\u{d7}` eines Tabs, das `+` dahinter.
```

## L4765-4769 · `fn tab_strip() -> Widget {`

```
/// Der Tabstreifen.
///
/// **Er steht immer da, auch bei einem Tab** — das `+` ist der einzige Ort,
/// an dem ein zweiter entsteht, wenn man die Tastenfolge nicht kennt. 36 px
/// dafuer sind der Preis, den jeder Browser zahlt.
```

## L4784-4785 · `if sel {`

```
// **Der aktive Tab traegt die Farbe des Inhalts darunter** (§3
// `tab`): er IST die Seite, der Streifen ist der Rahmen.
```

## L4794-4800 · `let bar = Widget::Row {`

```
// **Ein Akzentstreifen OBEN auf dem aktiven Tab.** Bis hierher war der
// Unterschied zwischen aktiv und ruhend `Surface` gegen
// `SurfaceElevated` plus eine Textfarbe — richtig, aber am Geraet zu
// leise (Florian: „tabs optisch noch bisschen mehr hervorheben").
// Der Streifen ist das uebliche Zeichen und das einzige, das auch aus
// zwei Metern liest. Er liegt IM Tab, nicht darueber: sonst
// verschoebe er die Beschriftung des aktiven gegen die der anderen.
```

## L4830-4833 · `kids.push(Widget::Column {`

```
// **`Stretch`, nicht `Start`.** In einer Spalte ist `align` die
// QUERachse, also die Breite: mit `Start` haette der Akzentstreifen
// seine natuerliche Breite bekommen — und die ist bei einer Zeile
// ohne Kinder null.
```

## L4846 · `align: Align::End,          // unten buendig`

```
// unten buendig
```

## L4855-4856 · `fn nav_button(icon: IconId, action: ActionId) -> Widget {`

```
/// Navigation button — the design's `toolbar_button`: bare at rest,
/// `SurfaceHover` fill under the cursor, accent tint while pressed.
```

## L4878 · `fn dropdown_for(which: u8) -> Option<(u32, Widget)> {`

```
/// Dropdown content for the open menu code (1=File .. 4=Help) → (anchor, menu).
```

## L4914-4915 · `fn prev_boundary(s: &str, i: usize) -> usize {`

```
// ── in-page text editing (the compositor edits its own Input widgets; a
//    control painted into our canvas is ours to edit) ──────────────────────
```

## L4925 · `fn find_run(engine: &Engine, cache: &Option<(Layout, i32, i32, u32)>, jump: bool) {`

```
// ── Suchen in der Seite ───────────────────────────────────────────────────
```

## L4927-4931 · `fn find_run(engine: &Engine, cache: &Option<(Layout, i32, i32, u32)>, jump: bool) {`

```
/// Die Suche neu rechnen und die Fundstellen hervorheben.
///
/// `jump` springt zur aktuellen Fundstelle. Beim Tippen ist das erwuenscht
/// (man will sehen, ob es sie gibt), beim blossen Neuauslegen nicht — sonst
/// reisst ein nachgeladenes Bild die Seite unter dem Leser weg.
```

## L4942-4943 · `if jump && n > 0 {`

```
// Hinscrollen, damit die Fundstelle im Blick ist — ein Drittel von oben,
// nicht am Rand: eine Fundstelle in der letzten Zeile liest sich nicht.
```

## L4958 · `fn find_step(engine: &Engine, cache: &Option<(Layout, i32, i32, u32)>, back: bool) {`

```
/// Eine Fundstelle weiter (oder zurueck), rundherum.
```

## L4967 · `fn find_close(engine: &Engine) {`

```
/// Die Leiste schliessen und alles aufraeumen.
```

## L4979-4989 · `fn utf8_feed(pending: &mut [u8; 4], len: &mut u8, b: u8) -> Option<char> {`

```
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
```

## L4995 · `if b >= 0xC0 { *len = 0; }              // neue Folge`

```
// neue Folge
```

## L5005-5006 · `fn key_names(key: KeyCode, pending: bool) -> Option<(String, String, u32, bool)> {`

```
/// Apply one key to the focused control. Returns true if the page must be
/// re-laid-out (the control's painted text or caret changed).
```

## L5008-5014 · `fn key_names(key: KeyCode, pending: bool) -> Option<(String, String, u32, bool)> {`

```
/// Wie eine Taste in der Sprache der Seite heisst: `key` (der WERT), `code`
/// (der ORT auf der Tastatur) und die Altlast `keyCode`.
///
/// Drei Namen und nicht einer, weil Seiten alle drei lesen: React und
/// modernes JS `key`, Tastaturkuerzel `code` (der ueberlebt ein anderes
/// Layout), und der ganze Altbestand `keyCode`/`which`. Wer nur `key`
/// liefert, laesst die Haelfte der Behandler ins Leere greifen.
```

## L5018-5019 · `KeyCode::Char(_) if pending => return None,`

```
// Ein halbes UTF-8-Zeichen ist noch keine Taste — es wird gemeldet,
// wenn es vollstaendig ist.
```

## L5029-5030 · `(alloc::format!("{c}"), code, c.to_ascii_uppercase() as u32,`

```
// `keyCode` ist der GROSSBUCHSTABE, auch wenn klein getippt wird —
// die Altlast kennt keine Schreibweise, nur die Taste.
```

## L5034-5035 · `KeyCode::Char(_) => (s(""), s(""), 0, false),`

```
// Alles ab 0x80 ist ein fertiges Zeichen aus `utf8_feed`; sein `code`
// haengt am Layout, das wir nicht kennen, also bleibt er leer.
```

## L5054-5060 · `fn fire_ui(engine: &Engine, f: impl FnOnce(&mut beak_engine::js::interp::Interp) -> bool) -> bool {`

```
/// Ein Ereignis der Bedienung an die Seite geben und die Runde zu Ende
/// fahren.
///
/// **Die Runde gehoert dazu.** Ein Behandler, der `fetch` anstoesst oder ein
/// `setTimeout` legt — und das tut jede Vorschlagsliste —, braucht die
/// Microtasks und die Zeitgeber, sonst liegt seine Arbeit bis zum naechsten
/// Ereignis still. Der Rueckgabewert sagt, ob die Seite ABGEBROCHEN hat.
```

## L5085-5088 · `if let Some((k, code, kc, shift)) = key_names(key, page.state.pending_len > 0) {`

```
// **`keydown` ZUERST, und sein Abbruch gilt.** So filtert jedes
// Eingabefeld der Welt Zeichen (UI Events §5.4). Ohne diese Zeile glaubt
// eine Seite, sie haette verhindert, und der Tastendruck kommt trotzdem
// an — das waere schlimmer als gar keine Zustellung.
```

## L5097 · `return match key {`

```
// Space / Enter activate a button or toggle a box, like a browser.
```

## L5111-5112 · `let mut typed: Option<String> = None;`

```
// Was eingefuegt wurde — `InputEvent.data`. Bei einer Loeschung `None`,
// und das ist kein Platzhalter: die Spezifikation sagt dort `null`.
```

## L5115-5119 · `KeyCode::Char(b) if b >= 0x20 && b != 0x7F => {`

```
// **Nicht mehr nur ASCII.** `0x20..0x7F` hiess woertlich: auf einer
// Deutschschweizer Tastatur laesst sich kein `ä` in ein Formular
// tippen — nicht in ein Suchfeld, nicht in ein Anmeldefeld. Alles ab
// 0x80 ist ein Stueck einer UTF-8-Folge; `b as char` waere dort
// LATIN-1 und machte aus dem Fuehrungsbyte 0xC3 ein `Ã`.
```

## L5127 · `None => return false,`

```
// Folge noch nicht vollstaendig: nichts tun, nichts malen.
```

## L5155-5156 · `let activated = page`

```
// Implicit submission (HTML §4.10.21.2): Enter in a text field
// activates the form's default button.
```

## L5173-5174 · `doc_mut().caret_since = now_ms();`

```
// Der Blinktakt faengt von vorn an: direkt nach einem Tastendruck steht
// der Zeiger solide, sonst blinkt er einem beim Tippen weg.
```

## L5177-5180 · `push_control_values(page);`

```
// **In den BAUM, bei jedem Tastendruck.** Die `seq`, unter der der Wert
// in `FormState` liegt, gilt nur bis zum naechsten `to_dom` — der
// Baumknoten gilt weiter. Er ist die Bruecke, ueber die `sync` den Wert
// zurueckholt, und nebenbei das, was ein Behandler der Seite liest.
```

## L5182-5185 · `let (itype, data) = match key {`

```
// **Und jetzt sagen, dass sich etwas geaendert hat.** Der Wert steht
// schon im Knoten — ein Behandler, der `e.target.value` liest, bekommt
// ihn also. `input` blast und ist NICHT abbrechbar; daran haengt jede
// Vorschlagsliste, jeder Live-Filter, jeder Zeichenzaehler des Webs.
```

## L5202-5208 · `fn set_focus(engine: &Engine, page: &mut Page, next: Option<u32>) {`

```
/// Den Fokus wechseln — und es der Seite sagen.
///
/// **Ein Weg, nicht sieben Zuweisungen.** `focus`/`blur` blasen NICHT,
/// `focusin`/`focusout` schon (UI Events §5.2), und Seiten benutzen beide:
/// wer am Formular lauscht statt am Feld, hoert nur das zweite. Dazu faellt
/// hier `change` — bei einem Textfeld beim VERLASSEN und nur, wenn sich der
/// Wert seit dem Fokussieren geaendert hat (HTML §4.10.5.5), nicht je Zeichen.
```

## L5243-5247 · `fn dispatch_mouse_edge(engine: &Engine, lay: &Layout, kind: &'static str, cx: i32, cy: i32) {`

```
/// `mousedown`/`mouseup` am Ort des Zeigers.
///
/// Gemessen melden acht von zwoelf Korpusseiten `mousedown` an — mehr als
/// `input`. Ein `click` allein reicht ihnen nicht: wer ein Menue beim
/// Druecken oeffnet und beim Loslassen waehlt, sieht sonst nur die Mitte.
```

## L5263-5271 · `fn blink_caret(engine: &Engine, cache: &Option<(Layout, i32, i32, u32)>,`

```
/// **Der Schreibzeiger blinkt.** Etwa zweimal je Sekunde, und nur der
/// Streifen, in dem er steht, wird neu gemalt.
///
/// Die 530 ms sind die Halbperiode, die Browser benutzen. Nach einem
/// Tastendruck faengt der Takt von vorn an und der Zeiger steht SOLIDE — wer
/// tippt, will sehen, wo er ist.
///
/// Kostet nur etwas, solange ein Textfeld den Fokus hat: sonst faellt die
/// erste Zeile heraus und es bleibt bei einem Vergleich je Runde.
```

## L5288 · `let sy = scroll_y();`

```
// Nur die Zeilen des Zeigers, in FENSTERkoordinaten und geklemmt.
```

## L5298-5306 · `fn fire_viewport_events(engine: &Engine) {`

```
/// `scroll` und `resize` an die Seite melden — nach dem Bild, nicht waehrend.
///
/// **Nur bei Aenderung, und deshalb mit eigenem Gedaechtnis.** `scroll_y`
/// sagt, wohin gemalt wird; `told_scroll`, was die Seite zuletzt gehoert hat.
/// Ohne den Unterschied faellt das Ereignis je Bild (60/s, und jede Seite mit
/// einem Sticky-Kopf rechnet dann dauernd) oder nie.
///
/// Gemessen ueber die zwoelf Korpusseiten: `resize` meldet auf 10 von 12 an,
/// `scroll` auf 9 (`docs/plan/BROWSER_INPUT_EVENTS.md`).
```

## L5315-5316 · `if !js_session().is_some_and(|s| s.interp.doc.as_ref().is_some_and(|d| d.has_listeners)) {`

```
// Nur wenn ueberhaupt jemand zuhoert — sonst kostet jedes Rollen einen
// Durchlauf durch die Maschine, fuer nichts.
```

## L5336 · `fn activate(engine: &Engine, page: &mut Page, seq: u32) {`

```
/// Click / keyboard activation of a control: submit, toggle, or take focus.
```

## L5354-5355 · `push_control_values(page);`

```
// Derselbe Grund wie beim Tippen: der Haken gehoert in den Baum,
// sonst ueberlebt er das naechste `to_dom` nicht.
```

## L5365 · `set_focus(engine, page, Some(seq));`

```
// A text field takes focus with the caret at the end.
```

## L5372-5392 · `fn restate_control(engine: &Engine, cache: &mut Option<(Layout, i32, i32, u32)>,`

```
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
```

## L5397-5400 · `say_ctl_bail_once(engine.repaint_bail());`

```
// EINMAL je Seite sagen, WARUM ausgelegt wird. Ohne diese Zeile sieht
// ein Schnellweg, der nie laeuft, genauso aus wie einer, der nie
// gebraucht wurde — und genau so ist er drei Versionen lang tot
// gewesen ([[feedback_the_fast_path_must_say_it_ran]]).
```

## L5423-5425 · `Event::InputChange { value } => {`

```
// Nur das Textfeld. NICHT `set_url`: das meldet dem Kernel den
// Netzkontext, und der loest dafuer auf — ein Tastendruck ist keine
// Navigation ([[feedback_a_keystroke_is_not_a_navigation]]).
```

## L5428-5430 · `if page.state.focus.take().is_some() {`

```
// Typing in the address bar means the compositor moved keyboard
// focus there — drop the page control's focus so only one caret
// blinks and Enter goes to the right place.
```

## L5436-5438 · `Event::Chord { letter: b'f', .. } => {`

```
// **Strg+F.** Kommt als Chord, weil `Event::Key` keine Umschalter
// traegt — sonst waere Strg+F von einem getippten „f" nicht zu
// unterscheiden.
```

## L5446-5450 · `Event::Chord { letter: b't', .. } => {`

```
// **Strg+T / Strg+W / Strg+1..9.** Strg+Tab gibt es NICHT, und das ist
// keine Nachlaessigkeit: `Event::Chord` traegt einen Buchstaben, und
// der Kernel baut ihn aus `KeyCode::Char` — `KeyCode::Tab` ist kein
// `Char` und kommt gar nicht erst an. Ziffern schon (jede druckbare
// Taste), also sind die Zahlen der Weg zu einem bestimmten Tab.
```

## L5459-5461 · `Event::Chord { letter: b'9', .. } => {`

```
// `9` ist der LETZTE, nicht der neunte — so macht es jeder Browser,
// und bei zwanzig Tabs ist „der letzte" die einzige Zahl, die man
// ohne Zaehlen trifft.
```

## L5470-5472 · `Event::Key(k) if doc().find.is_some() => {`

```
// Die Suchleiste ist offen: die Tasten gehoeren IHR. Sie kommen nur
// hierher, wenn kein Textfeld des Compositors den Fokus hat — wer in
// die Adresszeile klickt, tippt dort weiter, und das ist richtig so.
```

## L5490 · `let Some(ch) = ch else { return true };   // Folge unvollstaendig`

```
// Folge unvollstaendig
```

## L5500-5501 · `Event::Key(k) if page.state.focus.is_some() => {`

```
// A page control has focus → the key is ours (the compositor only
// routes keys here when no chrome Input/TextArea consumed them).
```

## L5508 · `Event::Key(k) => {`

```
// No control focused: the keys that reach us drive the viewport.
```

## L5525 · `let t = edit_str().to_string();`

```
// Was in der Zeile STEHT, nicht wo wir sind.
```

## L5547 · `bump_content_gen("site-css-toggle"); // force re-layout with/without site CSS`

```
// force re-layout with/without site CSS
```

## L5557 · `bump_content_gen("inspect-toggle"); // re-layout with/without inspect boxes`

```
// re-layout with/without inspect boxes
```

## L5607-5608 · `id if (ACT_TAB_SEL..ACT_TAB_SEL + MAX_TABS as u32).contains(&id) => {`

```
// Zwei Baender, kein Feld je Tab: der Streifen wird bei jeder
// Aenderung neu gebaut, also IST der Index der Tab.
```

## L5621-5626 · `Event::MouseButton { button: MouseButton::Left, down: true, x, y } => {`

```
// Link clicks land in the canvas → hit-test the engine's link rects.
// IMPORTANT: only clicks INSIDE the canvas are ours. Menu-bar/toolbar
// clicks are delivered here too (as a MouseButton alongside their
// Action); touching the open menu on those would close the dropdown the
// very same click just opened (it "flashed open then shut"). Those are
// handled entirely by their Action / the Popover's on_dismiss.
```

## L5630 · `if open_menu() != 0 {`

```
// A page click with a menu open just dismisses it (no nav).
```

## L5637-5649 · `let stale = !matches!(cache.as_ref(),`

```
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
```

## L5655-5657 · `let (dispatched, inspect_sel, ctl_seq, href, toggle) = {`

```
// Alles, was das Layout beantworten kann, VOR der ersten
// Aenderung am Zwischenspeicher holen — danach ist er
// veraenderlich geliehen und `lay` gaebe es nicht mehr.
```

## L5660-5663 · `dispatch_mouse_edge(engine, lay, "mousedown", cx, cy);`

```
// Die Seite bekommt den Klick ZUERST. Ruft ein Behandler
// `preventDefault`, ist der Klick verbraucht — sonst
// wuerde beak zusaetzlich dem Link folgen, den die Seite
// gerade abgefangen hat.
```

## L5668-5670 · `inspect_mode()`

```
// Nur im Inspect-Modus: der Test laeuft ueber ALLE
// Kaesten, und ein Klick auf einer grossen Seite
// soll dafuer nicht zahlen, wenn niemand hinschaut.
```

## L5680-5682 · `if inspect_mode() {`

```
// Inspect mode intercepts the click: select the deepest
// element box under the cursor (shown as an outline + a
// status-bar label) instead of following a link.
```

## L5684-5685 · `if let Some((bx, by, _, _, ref label)) = inspect_sel {`

```
// Also echo to the serial console so it can be copied
// without transcribing from the screen.
```

## L5695-5696 · `if let Some(seq) = ctl_seq {`

```
// A control wins over a link: a submit button inside an
// <a>, or a field overlapping a link rect, is the target.
```

## L5702 · `if page.state.focus.take().is_some() {`

```
// Clicking the page elsewhere blurs a focused control.
```

## L5706-5716 · `if let Some(href) = href {`

```
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
```

## L5719-5720 · `let lay = &cache.as_ref().unwrap().0;`

```
// Ein Anker fuer die Markierung wird trotzdem gesetzt
// — genau darum geht es.
```

## L5730-5732 · `if let Some(seq) = toggle {`

```
// A `<summary>` opens/closes its section. It comes AFTER
// the control and the link: a link inside a summary
// navigates, which is what a browser does too.
```

## L5740-5750 · `let lay = &cache.as_ref().unwrap().0;`

```
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
```

## L5762-5764 · `Event::MouseButton { button: MouseButton::Left, down: false, x, y } => {`

```
// Loslassen: die Markierung steht, das Ziehen ist vorbei — und HIER
// entscheidet sich, ob der Druck ein Klick war oder der Anfang einer
// Markierung.
```

## L5767-5769 · `if let Some((rx, ry, _, _)) = canvas_rect() {`

```
// **VOR der Linklogik, und unabhaengig davon.** Ein `mouseup`
// faellt auch dort, wo kein Link haengt — ein Schieberegler, der
// beim Loslassen einrastet, liegt auf keinem `<a>`.
```

## L5776-5777 · `if doc().sel.is_some() || (x - px).abs() > 4 || (y - py).abs() > 4 {`

```
// Gezogen? Dann war es keine Navigation. Vier Pixel Toleranz,
// damit eine zitternde Hand noch klickt.
```

## L5784-5786 · `Event::Clipboard(ClipKind::Copy) => {`

```
// **Strg+C auf der Seite.** Der Compositor faengt die Tastenfolge nur
// ab, wenn eines SEINER Textfelder den Fokus hat (`handle_input_key`
// steigt sonst sofort aus) — auf der Leinwand kommt sie hier an.
```

## L5796-5801 · `Event::MouseMove { x, y } => {`

```
// `:hover`. A series of ever-cheaper ways to answer "nothing to do":
// no hover rules on the page at all, then no usable cached layout,
// then the same element as last time. What is left is answered by
// repainting the display list in place where that is provably enough
// — measured on Wikipedia at 0.16 ms against 24 ms for a layout — and
// only otherwise by laying the page out again.
```

## L5803-5806 · `if doc().sel_anchor.is_some() {`

```
// **Ziehen kommt VOR dem Hover.** Wer markiert, will keine
// `:hover`-Rechnung dazwischen — und die frueheste Absage dieses
// Zweigs („die Seite hat gar keine Hover-Regeln") wuerde die
// Markierung sonst auf jeder gewoehnlichen Seite verschlucken.
```

## L5816-5817 · `let sel = (a != b).then_some((a, b));`

```
// Ein Punkt ist keine Markierung — sonst blinkt
// bei jedem Klick ein Schleier von einem Pixel auf.
```

## L5835-5836 · `let inside = x >= rx && x < rx + w && y >= ry && y < ry + h;`

```
// Leaving the canvas has to CLEAR the hover, or whatever the pointer
// left behind stays lit for good.
```

## L5843-5845 · `_ => return false,`

```
// No layout to hit-test against. Laying one out just to
// answer where the pointer is would cost the very thing
// this arm is trying to avoid.
```

## L5869-5872 · `engine.revert_hover();`

```
// Neither cheap enough to repaint nor affordable to lay
// out: put the state back, or the next layout that runs
// for some other reason lights up a pointer that has
// long moved on.
```

## L5884-5889 · `Event::MouseButton { button: MouseButton::Middle, down: true, x, y } => {`

```
// **Mittelklick auf einen Link: neuer Tab im Hintergrund.**
//
// Die Seite bekommt ihn NICHT. Ein Mittelklick ist `auxclick`, nicht
// `click`; ihn als `click` zuzustellen waere falscher, als ihn
// wegzulassen, denn ein Behandler, der `preventDefault` ruft, meint
// damit die linke Taste.
```

## L5905-5907 · `Event::Open(s) => {`

```
// **`npk_open` auf eine laufende Instanz.** Der Kernel nennt es
// „Singleton + tabs" — und genau das ist es jetzt: ein zweites
// Oeffnen ist ein Tab, kein Ersetzen dessen, was gerade dasteht.
```

## L5939-5946 · `#[global_allocator]`

```
// ── Heap: a real free-list allocator. The six font faces (persistent) + each
//    frame's layout + paint buffer are freed on drop, unlike a bump heap. ───
//
// Die wachsende Halde selbst steht seit widgets 0.28.0 in der SDK
// (`nopeek_widgets::heap`), weil tune sie fuer dekodierte Videobilder
// genauso braucht. Sie stand bis beak 0.188.0 hier; die zwei Zahlen darin
// — der Verdopplungsschritt und sein Deckel — sind je einmal bezahlt
// worden, und eine zweite Kopie haette sie nochmal bezahlen muessen.
```

## L5961-5965 · `core::arch::wasm32::unreachable()`

```
// Trap — do NOT `loop {}`. A wasm `unreachable` makes `_start`'s host call
// return Err, so the kernel tears this instance down and frees its worker
// core. A busy loop would instead pin the core forever (fibers are
// cooperative → a spinning fiber never yields) = the "app panic freezes the
// machine" bug. Cleanly dying is the whole point of the per-tab sandbox.
```

## L5971 · `let arg_len = {`

```
// No heap init here — talc claims `HEAP` lazily on the first allocation.
```

## L5973-5974 · `let arg_len = {`

```
// Launch argument: `npk_open("beak", "https://…")` → prime the address bar
// now; the actual fetch waits until the font is parsed (below).
```

## L5984-5987 · `render_chrome();`

```
// Commit the chrome IMMEDIATELY so the window is an opaque browser from the
// first frame. Parsing the 880 KB font (below) is slow enough to otherwise
// leave the window empty/transparent for a beat (the "loop window" look)
// and makes beak feel slower to start than font-free apps (spell/loft).
```

## L5990-5992 · `log(concat!("[beak] version ", env!("CARGO_PKG_VERSION")));`

```
// Which build is actually running. Without this a serial trace cannot say
// whether a measurement belongs to the version that was just installed —
// and a perf number from the wrong build is worse than no number.
```

## L5995-5999 · `beak_engine::js::random::set_source(random_bytes);`

```
// **Dem Motor den Zufall des Kernels leihen.** Die Engine hat keine
// Hostfunktionen; sie bekommt eine gereicht, genau wie die Uhr. Ohne
// diese Zeile gibt es in einer Seite kein `crypto` — und das ist die
// richtige Antwort, solange keine echte Quelle da ist, statt `Math.random`
// als sichere auszugeben.
```

## L6002-6006 · `engine_mut().set_theme(query_theme());`

```
// **Hier wurde frueher die Schrift geparst — alle sechs Gesichter, 435 ms
// und 40 MB Halde, gemessen mit `beakbench`.** Jetzt wird ein Gesicht
// gebaut, wenn es zum ersten Mal gebraucht wird; eine gewoehnliche Seite
// fasst zwei bis vier an. Wieviele es wirklich waren, sagt die Zeile nach
// dem ersten Malen.
```

## L6008 · `engine().set_clock(|| unsafe { npk_ticks() } as u64);`

```
// Lend the engine our tick source so it can report the per-phase split.
```

## L6010-6012 · `cookies_restore();`

```
// Die Kekse der letzten Sitzungen zurueck ins Glas, BEVOR die erste
// Anfrage rausgeht — sonst laedt die Startseite abgemeldet und meldet
// sich erst beim zweiten Klick wieder an.
```

## L6016 · `if arg_len > 0 {`

```
// Engine is up — fetch the launch URL now (if we were opened with one).
```

## L6022 · `let mut cache: Option<(Layout, i32, i32, u32)> = None;`

```
// Cached layout: (Layout, width it was laid out at, content generation).
```

## L6024 · `let mut page = Page::new();`

```
// The page's forms + the user's edits, rebuilt on every navigation.
```

## L6026 · `let mut paint_buf: Vec<u8> = Vec::new();`

```
// Persistent paint buffer, reused across frames (see maybe_repaint).
```

## L6028 · `loop {`

```
// Image sources of the current page still to fetch, one per loop turn.
```

## L6030 · `loop {`

```
// CSS images still to fetch, as (url_key, url). Filled from the layout.
```

## L6032-6034 · `loop {`

```
// Every CSS image this page has already been asked for — including the
// ones that failed. A miss must not be retried forever; the box simply
// stays undecorated until the next navigation.
```

## L6037-6043 · `let mut chrome = false;`

```
// Drain the ENTIRE event queue this tick, THEN repaint once. Wheel
// events used to be handled one-per-loop with a full repaint (and a
// ~5 MB buffer alloc) each — a burst of scroll notches backed up so the
// page scrolled slowly and "kept going" after the wheel stopped, AND
// the loop never reached the idle sleep, so the worker core span at
// 100% (never halting). Coalescing collapses the burst into one scroll
// step + one repaint.
```

## L6063-6065 · `if nav_pump(engine()) {`

```
// Take delivery of whatever the kernel finished while we were
// painting: the document, or its stylesheets. THIS is where a
// navigation completes now — no path through `handle` waits for one.
```

## L6069-6081 · `if page.sync(engine()) {`

```
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
```

## L6085-6086 · `let pending = js_session().map(|s| s.interp.take_submits()).unwrap_or_default();`

```
// Ein Formular, das die Seite schon beim Laden abschicken will, darf
// nicht bis zum naechsten Klick liegenbleiben.
```

## L6092-6096 · `sync_nav(engine());`

```
// Und eine Navigation, die die Seite selbst verlangt hat. **Hier
// zentral und nicht an jedem Einstiegspunkt:** ein Skript beim
// Laden, ein Zeitgeber, ein `load`-Behandler und ein Klick landen
// alle in derselben Runde — sechs Aufrufstellen waeren sechs
// Gelegenheiten, eine zu vergessen.
```

## L6102-6106 · `}`

```
// No theme watch here any more: the PAGE palette no longer
// follows the desktop (see `query_theme`), so a light/dark switch
// changes the chrome and nothing about the document. Re-laying the
// page out for it would cost a full layout — over five seconds on
// the device — for a picture that cannot change.
```

## L6108-6116 · `if images_dirty() {`

```
// A fresh page: drop the old page's decoded images and note which ones
// it wants (fetching them happens after the repaint, one batch a turn).
//
// This has to sit AFTER `nav_pump` and BEFORE the repaint. A page is
// completed by `nav_pump`, so from the top of the loop this would
// always be one turn late: the page was laid out once against the
// PREVIOUS page's images, and clearing them a turn later invalidated
// that layout and laid it out again. Two full layouts per navigation,
// and on the device a layout is over five seconds.
```

## L6126-6129 · `fire_viewport_events(engine());`

```
// NACH dem Bild: die Seite hoert, was sich bewegt hat. Waehrend
// `maybe_repaint` ginge es nicht — dort ist der Zwischenspeicher
// veraenderlich geliehen, und ein Behandler, der den Baum aendert,
// haette ihn unter dem laufenden Bild weg.
```

## L6132-6133 · `if pump_fonts(engine()) {`

```
// Die Schriften, die die Seite mitbringt. NACH dem ersten Auslegen:
// vorher weiss niemand, welche sie ueberhaupt verlangt.
```

## L6138-6143 · `let ws_moved = match js_session() {`

```
// Die offenen WebSockets. Sie laufen VOR den Antworten von `fetch`,
// weil eine Nachricht ohne Anlass kommt: niemand hat sie bestellt,
// und wer nicht in jedem Bild nachsieht, sieht sie gar nicht.
// Ereignisse daraus sind Einstiegspunkte wie jeder andere, also
// laeuft danach dieselbe Nacharbeit — deshalb steht die Zeile in
// DERSELBEN Bedingung.
```

## L6148-6149 · `if pump_fetches() | ws_moved {`

```
// Was `fetch()` bestellt hat. Kommt eine Antwort an, lief danach
// Seitencode — und der darf den Baum umgebaut haben.
```

## L6154-6159 · `sync_cookies(s);`

```
// Ein `fetch`-Rueckruf ist ein Einstiegspunkt wie jeder
// andere: er darf einen Keks setzen und die Adresse
// umschreiben. Ohne diese zwei Zeilen fiel beides still
// unter den Tisch — und „still" heisst hier: die naechste
// Anfrage geht ohne den Keks hinaus, den die Seite gerade
// gesetzt hat.
```

## L6173-6180 · `pump_box_observers(engine());`

```
// Die Kasten-Beobachter. `set_geometry` hat waehrend des Malens
// GEMESSEN; zugestellt wird hier, weil ein Rueckruf ein
// Einstiegspunkt ist und mitten im Malen nichts zu suchen hat.
//
// **Und es MUSS hier stehen.** Ohne diese Zeilen haette eine Seite
// ohne Zeitgeber und ohne Ereignisse ihre Beobachter angemeldet und
// nie einen Rueckruf gesehen — die Meldung laege in der Schlange und
// wartete auf einen Einstiegspunkt, den es nicht gibt.
```

## L6182-6186 · `if fire_load(engine(), &page) {`

```
// JETZT steht die Geometrie — `load` darf fallen.
// `pageshow` faellt NACH `load`, einmal je Navigation (HTML §7.11.4).
// Eine Seite, die ihren Zustand beim Zurueckkommen aus dem Verlauf
// wiederherstellt, haengt daran — und `persisted` ist bei uns immer
// `false`, weil beak keinen Seitenzwischenspeicher hat.
```

## L6196-6198 · `let band = match canvas_rect() {`

```
// The visible document band, read AFTER the repaint clamped the scroll
// offset. Without a canvas the band is everything, so an arriving
// image always repaints — the conservative direction.
```

## L6206-6211 · `let layout = cache.as_ref().map(|(l, _, _, _): &(Layout, i32, i32, u32)| l);`

```
// Text and layout are on screen now — pull in the next few images,
// then come back round and paint them. Scrolling keeps working in
// between, because a batch is small. The layout goes in whole rather
// than a cloned `guessed_image_srcs`: it answers both questions this
// needs (did a guessed box land, and is the picture even on screen),
// and the clone happened every turn of the loop.
```

## L6213-6216 · `let mut q = core::mem::take(&mut doc_mut().pending_imgs);`

```
// Die Schlange wird HERAUSgenommen und zurueckgelegt, statt sie
// liegend zu leihen: `pump_images` ruft selbst `doc()`, und eine
// gehaltene Referenz daneben waere die zweite Entleihung, vor der der
// Kommentar an `doc`/`doc_mut` warnt.
```

## L6220-6229 · `let mut css_adopted: Vec<u64> = Vec::new();`

```
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
```

## L6239-6243 · `if engine().adopt_css_cached(*k, &resolve(url_str(), u)) {`

```
// Same question as for `<img>`, one layer later: the layout
// only names its background images AFTER it has run, so this
// cannot happen in `begin_images`. The url is resolved exactly
// as `pump_css_images` resolves it, or put and get would
// use different keys for one picture.
```

## L6251-6252 · `if !css_adopted.is_empty() {`

```
// An adopted layer arrives after this turn's paint, so it needs the
// next one — but only if it would show. Same test the fetched ones get.
```

## L6263-6265 · `unsafe {`

```
// ALWAYS yield so this worker core can halt — a cooperative fiber that
// never sleeps pins its core at 100%. A short nap while interacting
// stays responsive; a longer one when idle keeps the core asleep.
```

## L6267-6272 · `let waiting = nav_busy() || img_job() >= 0 || cssimg_job() >= 0`

```
// Anything on the wire keeps the short nap: that is how often we
// ask the kernel whether the answer is here, and it is the whole
// latency the split costs. 4 ms against a round trip is nothing.
// Eine laufende Schriftrunde gehoert dazu: sonst schlaeft die
// Schleife 16 ms je Frage, und eine Seite steht eine Sekunde
// laenger ungestylt da.
```

## L6285 · `#[allow(dead_code)]`

```
// Keep IconRef referenced (used via the build.rs-generated AppMeta blob).
```

