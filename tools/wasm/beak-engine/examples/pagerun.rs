//! Die GANZE Skriptrunde einer Seite host-seitig fahren — in EINER Sitzung,
//! in Dokumentreihenfolge, mit demselben Modul-Rueckfall wie beak.
//!
//! `jsrun` faehrt eine Datei allein, und das beantwortet die falsche Frage:
//! am Geraet teilen sich alle Skripte einen globalen Bereich, und ein Skript,
//! das allein `x is not defined` wirft, laeuft in der Kette sauber. Genau so
//! ist die Fritzbox-Anmeldeseite zuerst falsch gelesen worden.
//!
//!   cargo run --release --example pagerun -- seite.html verzeichnis/
//!
//! Die externen Skripte werden NICHT geholt — sie werden im Verzeichnis unter
//! dem letzten Pfadbestandteil ihrer `src` erwartet (`curl` legt sie so ab).
//! Fehlt eine Datei, sagt der Lauf das, statt sie still zu ueberspringen.
use beak_engine::js::dombind::ScriptRef;

/// Ein Wirt fuer `fetch`, aus dem Spiegelverzeichnis.
///
/// **Damit laesst sich die API-Schicht einer Seite host-seitig fahren.** Die
/// Fritzbox-Oberflaeche baut ihr `rest-helper.js` schon im MODULKOPF auf
/// einem `AbortController` — das Modul scheiterte daran, bevor irgendetwas
/// von seinem Inhalt lief, und `fetch` gab es daneben auch nicht.
///
/// Was der Wirt hier NICHT tut: raten. Eine Datei, die es nicht gibt, wird
/// zur Ablehnung mit `TypeError`, genau wie ein Netzfehler im Browser — nicht
/// zu einer leeren 200er-Antwort.
fn serve_fetches(sess: &mut beak_engine::js::Session, dir: &str) -> usize {
    let dropped = sess.interp.take_aborted_fetches();
    let want = sess.interp.take_pending_fetches();
    let mut n = 0;
    for f in want {
        if dropped.contains(&f.id) { continue }
        n += 1;
        let u = resolve_path(&format!("{}/", origin()), &f.url);
        match std::fs::read(local(dir, &u)) {
            Ok(bytes) => {
                let ct = if u.ends_with(".json") { "content-type: application/json\r\n" }
                         else { "content-type: text/plain\r\n" };
                beak_engine::js::fetch::fetch_done(
                    &mut sess.interp, f.id, 200, &u, ct,
                    String::from_utf8_lossy(&bytes).into_owned());
            }
            Err(e) => beak_engine::js::fetch::fetch_failed(&mut sess.interp, f.id, &e.to_string()),
        }
    }
    n
}

/// Die `<script src=…>`, die ein Skript eingehaengt hat — aus dem
/// Spiegelverzeichnis bedient, wie der Wirt sie aus dem Netz bedient.
///
/// **Ohne diese Zeilen misst die Probe eine andere Plattform als das
/// Geraet**: jeder code-geteilte Bundler laedt so nach, und ein Versprechen,
/// das nie faellt, sieht host-seitig wie eine haengende Seite aus
/// ([[feedback_the_test_path_must_be_the_real_path]]).
fn serve_dyn_scripts(sess: &mut beak_engine::js::Session, dir: &str) -> usize {
    let want = sess.interp.take_pending_scripts();
    let mut n = 0;
    for (id, src) in want {
        n += 1;
        let u = resolve_path(&format!("{}/", origin()), &src);
        match std::fs::read_to_string(local(dir, &u)) {
            Ok(t) => beak_engine::js::dombind::script_done(&mut sess.interp, id, Some(&t)),
            Err(_) => {
                println!("  dyn-Skript fehlt: {u}");
                beak_engine::js::dombind::script_done(&mut sess.interp, id, None);
            }
        }
    }
    n
}

/// Zufall fuer die HOST-Werkzeuge — aus `/dev/urandom`, nicht aus einer
/// Bequemlichkeit. Ohne sie gaebe es hier kein `crypto`, und dann misst das
/// Werkzeug eine andere Plattform als das Geraet.
fn host_random(out: &mut [u8]) -> bool {
    use std::io::Read;
    match std::fs::File::open("/dev/urandom") {
        Ok(mut f) => f.read_exact(out).is_ok(),
        Err(_) => false,
    }
}

/// Ein Allokator, der GROSSE Anforderungen meldet — mit Rueckwaertsspur.
///
/// Ein Leck sieht man am RSS, aber nicht, WER es anfordert: ein Abtastprofil
/// zeigt Rechenzeit, und eine einzige Allokation von 1,8 GB kostet keine.
struct Loud;
static LOUD_LIMIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);
/// Was gerade WIRKLICH belegt ist — Allokationen minus Freigaben.
///
/// **Der RSS ist die falsche Zahl.** Er zeigt, was der Wirtsallokator vom
/// System behalten hat, nicht was die Seite haelt; auf dem Geraet entscheidet
/// aber die zweite. Der Unterschied war auf DuckDuckGos Ergebnisseite der
/// zwischen 1,4 GB und dem, was wirklich lebt.
pub static LIVE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
pub static LIVE_PEAK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
fn note_alloc(n: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    let v = LIVE.fetch_add(n as i64, Relaxed) + n as i64;
    if v > LIVE_PEAK.load(Relaxed) { LIVE_PEAK.store(v, Relaxed); }
}

unsafe impl std::alloc::GlobalAlloc for Loud {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        note_alloc(l.size());
        if l.size() >= LOUD_LIMIT.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("\n=== ALLOC {} B ===\n{}", l.size(), std::backtrace::Backtrace::force_capture());
        }
        unsafe { std::alloc::System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        LIVE.fetch_sub(l.size() as i64, std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
        LIVE.fetch_sub(l.size() as i64, std::sync::atomic::Ordering::Relaxed);
        note_alloc(n);
        if n >= LOUD_LIMIT.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("\n=== REALLOC {} B ===\n{}", n, std::backtrace::Backtrace::force_capture());
        }
        unsafe { std::alloc::System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Loud = Loud;

static mut DEADLINE: std::time::Instant = unsafe { core::mem::zeroed() };
static mut T0: Option<std::time::Instant> = None;

/// Die ECHTE Uhr fuer die Probe. Ohne sie ist `performance.now()` ein
/// Aufrufzaehler, und dann misst das Werkzeug eine andere Plattform als das
/// Geraet ([[feedback_the_test_path_must_be_the_real_path]]).
/// Was der Prozess gerade belegt — die einzige Zahl, die einen Leck-Verdacht
/// bestaetigt oder ausraeumt.
fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:"))
            .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok())))
        .map(|kb| kb / 1024).unwrap_or(0)
}

fn host_clock() -> f64 {
    unsafe { (&raw const T0).read() }
        .map(|t| t.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn main() {
    if let Ok(v) = std::env::var("LOUD") {
        if let Ok(n) = v.parse::<usize>() {
            LOUD_LIMIT.store(n, std::sync::atomic::Ordering::Relaxed);
        }
    }
    unsafe { T0 = Some(std::time::Instant::now()) };
    beak_engine::js::random::set_source(host_random);
    let html_path = std::env::args().nth(1).unwrap_or_default();
    let dir = std::env::args().nth(2).unwrap_or_else(|| ".".into());
    let html = std::fs::read_to_string(&html_path).expect("HTML-Datei");

    let dom = beak_engine::dom::parse(&html);
    let doc = beak_engine::js::dombind::Doc::from_dom(&dom);
    let refs = beak_engine::js::dombind::page_scripts(&doc);

    // Derselbe Deckel wie im Wirt — die Probe soll nicht an einer Grenze
    // scheitern, die es am Geraet nicht gibt.
    let mut sess = beak_engine::js::Session::new(20_000_000_000);
    // `NOVM=1`: den Baumlaeufer erzwingen. Der Vergleich beantwortet die
    // Frage, die kein Zaehler beantwortet — laeuft der heisse Code der Seite
    // ueberhaupt auf der Befehlsmaschine?
    if std::env::var("NOVM").is_ok() { sess.interp.vm_off = true; }
    sess.interp.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
    let media = beak_engine::css::Media::new(1902.0, false);
    // **Die verlinkten Blaetter gehoeren in den Stilkontext.** Ohne sie
    // antwortet `getComputedStyle` nur aus den `<style>`-Bloecken der Seite,
    // und jede Bootstrap-Klasse sieht aus, als gaebe es sie nicht: `.row`
    // meldete `block` statt `flex` — ein Fehler der PROBE, der wie ein
    // Kaskadenfehler in beak aussah
    // ([[feedback_the_test_path_must_be_the_real_path]]).
    let mut linked = String::new();
    let mut nlink = 0usize;
    collect_links(dom.body(), &dir, &mut linked, &mut nlink);
    collect_links(&dom.root, &dir, &mut linked, &mut nlink);
    let sheet = beak_engine::css::collect_all(&dom, &linked, media);
    sess.interp.set_style_context(beak_engine::js::interp::StyleCtx {
        sheet: std::rc::Rc::new(sheet),
        theme: beak_engine::layout::Theme {
            bg: beak_engine::layout::Rgb(255, 255, 255),
            text: beak_engine::layout::Rgb(33, 37, 41),
            heading: beak_engine::layout::Rgb(33, 37, 41),
            link: beak_engine::layout::Rgb(13, 110, 253),
            muted: beak_engine::layout::Rgb(108, 117, 125),
            rule: beak_engine::layout::Rgb(222, 226, 230),
        },
        viewport_w: 1902.0,
    });
    // `DARK=1` faehrt die Probe im Dunkelmodus — dieselbe Lage, die der Wirt
    // aus `query_theme().is_dark()` einreicht. Ohne den Schalter misst man
    // immer hell und kann nicht sagen, ob eine Seite das Schema ueberhaupt
    // liest.
    let dark = std::env::var("DARK").is_ok();
    sess.interp.set_media(1902.0, 1000.0, dark);
    // `DEPTH=` hebt den Aufrufdeckel — die Frage „echte Endlosschleife oder
    // nur tiefer als 400?" ist sonst nicht zu beantworten.
    if let Ok(d) = std::env::var("DEPTH") {
        if let Ok(n) = d.parse() { sess.interp.max_depth = n; }
    }
    // `STEPS=` hebt den Schrittdeckel — die Frage „wie teuer ist die Rechnung
    // dieser Seite wirklich?" ist sonst nicht zu beantworten.
    if let Ok(d) = std::env::var("STEPS") {
        if let Ok(n) = d.parse() { sess.interp.max_steps = n; }
    }
    if std::env::var("NOCLOCK").is_err() { sess.interp.clock = Some(host_clock); }
    // **Neu auslegen auf Verlangen.** `NORELAYOUT=1` nimmt es heraus — fuer
    // das A/B, und damit eine Messung sagen kann, WAS sie misst.
    if std::env::var("NORELAYOUT").is_err() {
        HTML.with(|h| *h.borrow_mut() = html.clone());
        DIR.with(|d| *d.borrow_mut() = dir.clone());
        sess.interp.relayout = Some(host_relayout);
    }
    if let Ok(u) = std::env::var("URL") { sess.interp.set_location(&u); }
    // `BUDGET=<sekunden>`: dieselbe Frist, die der Wirt am Geraet stellt.
    // Ohne sie laeuft eine Seite host-seitig unbegrenzt, und „haengt" ist
    // dann keine Messung, sondern ein Abbruch durch die Uhr daneben.
    if let Ok(v) = std::env::var("BUDGET") {
        if let Ok(sec) = v.parse::<u64>() {
            unsafe { DEADLINE = std::time::Instant::now() + std::time::Duration::from_secs(sec) };
            sess.interp.deadline = Some(|| unsafe { std::time::Instant::now() < DEADLINE });
        }
    }

    let (mut ran, mut failed) = (0usize, 0usize);
    let mut inline_n = 0usize;
    for r in refs {
        let (src, label, is_mod, node) = match r {
            ScriptRef::Inline(t, m, n) => { inline_n += 1; (t, format!("inline #{inline_n}"), m, n) }
            ScriptRef::External(u, m, n) => {
                // **Denselben Weg wie die Blaetter**: `local` schneidet die
                // Abfrage ab und legt den Pfad flach, wie `mirror.py` es tut.
                // Der alte Weg nahm nur den Dateinamen — `js/main.js?v=1`
                // wurde `main.js?v=1`, und die Probe meldete „nicht im
                // Verzeichnis" fuer eine Datei, die daliegt
                // ([[feedback_the_probe_must_use_the_targets_resolver]]).
                match std::fs::read_to_string(local(&dir, &u)) {
                    Ok(t) => (t, u, m, n),
                    Err(e) => {
                        failed += 1;
                        println!("FAIL {u}: nicht im Verzeichnis ({e})");
                        continue;
                    }
                }
            }
        };
        let n0 = sess.interp.console.len();
        // Ein Modul: erst den GANZEN Graphen holen, dann verknuepfen, dann
        // auswerten — genau die Reihenfolge, die beak am Geraet faehrt.
        if is_mod || is_module(&src) {
            match run_module_graph(&mut sess, &label, &src, &dir) {
                Ok(()) => { ran += 1; println!("ok   {label} (Modul, {} B)", src.len()); }
                Err(e) => { failed += 1; println!("FAIL {label}: {e}"); }
            }
            for l in &sess.interp.console[n0..] { println!("       | {l}"); }
            continue;
        }
        let prog = match beak_engine::js::parse(&src, false) {
            Ok(p) => p,
            Err(e) => match beak_engine::js::parse(&src, true) {
                Ok(p) => p,
                Err(em) => {
                    failed += 1;
                    println!("FAIL {label}: SyntaxError: {} @{} | als Modul: {} @{}",
                             e.msg, pos(&src, e.at), em.msg, pos(&src, em.at));
                    continue;
                }
            },
        };
        // `document.currentScript` — ein Modul hat keinen (HTML §4.12.1),
        // und genau deshalb steht es NUR an diesem Zweig.
        sess.interp.current_script = Some(node);
        let r = sess.run(&prog);
        sess.interp.current_script = None;
        match r {
            Ok(()) => { ran += 1; println!("ok   {label} ({} B)", src.len()); }
            Err(e) => { failed += 1; println!("FAIL {label}: {e}"); }
        }
        for l in &sess.interp.console[n0..] { println!("       | {l}"); }
    }
    let n0 = sess.interp.console.len();
    // **Die Reihenfolge des WIRTS, und sie ist der ganze Punkt.** Erst die
    // Stilblattrunden (ein geholtes Blatt laesst eine Komponente fertig
    // bauen), dann `DOMContentLoaded`, dann EIN LAYOUT — und erst danach
    // `load`. Wer die Geometrie vor den Runden einreicht, misst einen Baum,
    // den es so nie gab: die Komponenten sind dann noch leer.
    let mut timers = 0;
    let (mut sheets_ok, mut sheets_bad) = (0usize, 0usize);
    let mut fetches = 0usize;
    let mut dynjs = 0usize;
    let stats = std::env::var("STATS").is_ok();
    let mut round = 0usize;
    for _ in 0..64 {
        round += 1;
        if stats {
            let d = sess.interp.doc.as_ref();
            println!("[stats {round:2}] Knoten={} Zeitgeber={} Jobs={} Konsole={} Module={} \
Beobachter={}/{} Kekse={} rss={} MB",
                d.map_or(0, |d| d.nodes.len()),
                sess.interp.timers.len(),
                sess.interp.jobs.len(),
                sess.interp.console.len(),
                sess.interp.modules.len(),
                sess.interp.resize_obs.len(), sess.interp.inter_obs.len(),
                sess.interp.cookies.len(),
                rss_mb());
            use std::sync::atomic::Ordering::Relaxed;
            // **Die Halde zaehlt der Allokator, den Zensus die Engine.** Nur
            // der zweite haengt an `heap-census`; ohne das Merkmal soll die
            // Probe TROTZDEM bauen. Sie tat es seit 0.175.3 nicht mehr, und
            // daran fiel der Galerie-Vergleich aus — still, weil `cargo run`
            // seinen Baufehler nach stderr schreibt und der Aufrufer stdout
            // liest ([[feedback_a_silent_failure_hides_every_bug_upstream_of_it]]).
            #[cfg(feature = "heap-census")]
            {
                let c = sess.interp.heap_census();
                println!("          Halde LEBEND {} MB, Spitze {} MB · Zensus: {} von {} Objekten \
erreichbar ({} Umgebungen, {} Eigenschaften){}",
                    LIVE.load(Relaxed) / 1048576, LIVE_PEAK.load(Relaxed) / 1048576,
                    c.reachable, c.live, c.envs, c.props,
                    if c.live > c.reachable {
                        format!(" — {} in Ringen", c.live - c.reachable)
                    } else { String::new() });
            }
            #[cfg(not(feature = "heap-census"))]
            println!("          Halde LEBEND {} MB, Spitze {} MB (Zensus: --features heap-census)",
                LIVE.load(Relaxed) / 1048576, LIVE_PEAK.load(Relaxed) / 1048576);
        }
        // **Erst bedienen, dann die Uhr laufen lassen.** Wer wartet, darf die
        // Zeitgeber nicht vorziehen — sonst faellt webpacks
        // Zeitueberschreitung vor der Zustellung des Stuecks, auf das sie
        // sich bezieht.
        fetches += serve_fetches(&mut sess, &dir);
        let want = sess.interp.take_pending_sheets();
        let js = serve_dyn_scripts(&mut sess, &dir);
        dynjs += js;
        let t = sess.interp.run_timers();
        timers += t;
        // Siehe `GEOMEVERY` weiter unten: der Wirt misst je Bild, auch
        // WAEHREND die Skripte noch laufen. Genau in dieser Runde haengt
        // React seine Bausteine ein, und wer hier nicht misst, laesst jedes
        // frisch eingehaengte Element `offsetWidth == 0` melden.
        if std::env::var("GEOMEVERY").is_ok() { feed_geometry(&mut sess.interp, &html, &dir); }
        if t == 0 && js == 0 && want.is_empty() && sess.interp.pending_fetches.is_empty() { break }
        for (id, href) in want {
            let u = resolve_path(&format!("{}/", origin()), &href);
            let ok = std::fs::read_to_string(local(&dir, &u)).is_ok();
            if ok { sheets_ok += 1 } else { sheets_bad += 1 }
            beak_engine::js::dombind::sheet_done(&mut sess.interp, id, ok);
        }
    }
    if sheets_ok + sheets_bad > 0 {
        println!("Stilblaetter per Skript: {sheets_ok} geholt, {sheets_bad} gescheitert");
    }
    if let Some(dn) = sess.interp.doc.as_ref().map(|d| d.doc) {
        let _ = beak_engine::js::dombind::dispatch(&mut sess.interp, "DOMContentLoaded", &[dn]);
    }
    feed_geometry(&mut sess.interp, &html, &dir);
    if let Some(dn) = sess.interp.doc.as_ref().map(|d| d.doc) {
        let _ = beak_engine::js::dombind::dispatch(&mut sess.interp, "load", &[dn]);
    }
    // **`GEOMEVERY=1` legt zwischen zwei Zeitgeber-Runden ein BILD ein.**
    // Der Wirt misst je Bild neu (`set_geometry` in `beak/src/lib.rs`); die
    // Probe tut es sonst genau einmal, und dann meldet jedes Element, das ein
    // Skript spaeter einhaengt, fuer immer `offsetWidth == 0`. Wer eine Seite
    // untersucht, die sich selbst vermisst, misst ohne diesen Schalter die
    // Probe statt beak.
    let geom_every = std::env::var("GEOMEVERY").is_ok();
    for _ in 0..64 {
        let f = serve_fetches(&mut sess, &dir);
        fetches += f;
        let js = serve_dyn_scripts(&mut sess, &dir);
        dynjs += js;
        let t = sess.interp.run_timers();
        timers += t;
        if geom_every { feed_geometry(&mut sess.interp, &html, &dir); }
        if t == 0 && f == 0 && js == 0 { break }
    }
    if dynjs > 0 { println!("Skripte per Skript: {dynjs} eingehaengt"); }
    if fetches > 0 { println!("fetch: {fetches} Anfragen aus dem Spiegel bedient"); }
    // Was die Seite per `location` verlangt hat. Ohne diese Zeile sieht eine
    // Seite, die sich selbst weiterschickt, genauso aus wie eine, die nichts
    // tut — und genau daran ist Googles Sperrseite eine Woche lang
    // vorbeigelaufen.
    if let Some(n) = sess.interp.take_nav() {
        println!("NAVIGATION {}{} -> {}",
                 if n.replace { "replace" } else { "assign" },
                 if n.reload { "/reload" } else { "" }, n.url);
    }
    let mut n1 = sess.interp.console.len();
    for l in &sess.interp.console[n0..] { println!("  timer| {l}"); }
    // `DUMP=1` zeigt, was am Ende im Baum steht — die Frage „laufen die
    // Skripte" ist nicht dieselbe wie „haben sie etwas gebaut".
    if std::env::var("DUMP").is_ok() {
        if let Some(d) = sess.interp.doc.as_mut() {
            let dom = d.to_dom();
            let mut out = String::new();
            dump(dom.body(), 0, &mut out);
            println!("\n── Baum nach den Skripten ──\n{out}");
        }
    }
    // `SUBMIT=<id>` faehrt den GANZEN Absendeweg: `submit`-Ereignis, was der
    // Behandler ausrechnet, die Auftraege aus `form.submit()`, und am Ende
    // die fertige Eingabe. Genau die Reihenfolge, die der Wirt faehrt.
    // `TYPE=id=wert[,id=wert]` tippt in Felder, bevor abgeschickt wird — der
    // Weg, den der Benutzer nimmt. Geschrieben wird der SCHMUTZIGE Wert, also
    // genau das, was ein Tastendruck im Wirt auch setzt.
    if let Ok(spec) = std::env::var("TYPE") {
        let dom = sess.interp.doc.as_mut().map(|d| d.to_dom());
        if let Some(dom) = dom {
            for pair in spec.split(',') {
                let Some((id, v)) = pair.split_once('=') else { continue };
                let Some(seq) = find_seq(dom.body(), id) else {
                    println!("TYPE: kein Element mit id={id}"); continue };
                // **Zeichen fuer Zeichen, mit den Ereignissen dazu** — das
                // ist der Weg des Wirts seit 0.186.0 (`edit_key`). Den Wert
                // in einem Zug hineinzuschreiben hiesse, eine Seite zu
                // messen, die nie getippt bekommt: die Vorschlagsliste haengt
                // an `input`, nicht am Wert
                // ([[feedback_the_test_path_must_be_the_real_path]]).
                let mut acc = String::new();
                for ch in v.chars() {
                    let key = format!("{ch}");
                    let code = if ch.is_ascii_alphabetic() {
                        format!("Key{}", ch.to_ascii_uppercase())
                    } else if ch.is_ascii_digit() {
                        format!("Digit{ch}")
                    } else if ch == ' ' { String::from("Space") } else { String::new() };
                    let kc = ch.to_ascii_uppercase() as u32;
                    if beak_engine::js::dombind::dispatch_key(
                        &mut sess.interp, "keydown", seq, &key, &code, kc, false) {
                        continue;
                    }
                    acc.push(ch);
                    if let Some(d) = sess.interp.doc.as_mut() {
                        if let Some(n) = d.by_seq(seq) {
                            d.nodes[n as usize].value = Some(std::rc::Rc::from(acc.as_str()));
                            d.touch();
                        }
                    }
                    beak_engine::js::dombind::dispatch_input_event(
                        &mut sess.interp, "input", seq, "insertText", Some(&key));
                    beak_engine::js::dombind::dispatch_key(
                        &mut sess.interp, "keyup", seq, &key, &code, kc, false);
                    let _ = sess.interp.run_timers();
                }
                // Was ein Behandler angestossen hat, zu Ende fahren: eine
                // Vorschlagsliste holt ihre Antwort mit `fetch`.
                for _ in 0..32 {
                    let f = serve_fetches(&mut sess, &dir);
                    let j = serve_dyn_scripts(&mut sess, &dir);
                    let t = sess.interp.run_timers();
                    if f == 0 && j == 0 && t == 0 { break }
                }
            }
        }
    }
    if let Ok(spec) = std::env::var("CLICK") { click_ids(&mut sess, &html, &dir, &spec); }
    // `TEXT=<id>` gibt den Inhalt EINES Elements ungekuerzt aus — der Weg,
    // auf dem eine eingehaengte Sonde ihr Ergebnis herausreicht, und zwar
    // derselbe, den Chromium mit `--dump-dom` nimmt.
    // **Was WAEHREND `TYPE`/`CLICK`/`SUBMIT` gesagt wurde.** Die Zeile oben
    // leert die Konsole nach der Skriptrunde; alles, was ein Behandler danach
    // schreibt, stand bis 0.186.0 nirgends — und eine Probe, die das Tippen
    // misst, sah ihre eigenen Ausgaben nicht.
    if sess.interp.console.len() > n1 {
        for l in &sess.interp.console[n1..] { println!("  nach| {l}"); }
        n1 = sess.interp.console.len();
    }
    let _ = n1;
    if let Ok(id) = std::env::var("TEXT") {
        let dom = sess.interp.doc.as_mut().map(|d| d.to_dom());
        match dom.as_ref().and_then(|d| find_el(&d.root, &id)) {
            Some(e) => { let mut t = String::new(); raw_text(e, &mut t); println!("{t}"); }
            None => println!("TEXT: kein Element mit id={id}"),
        }
    }
    // `SWEEP=1`: die unerreichbaren Ringe brechen und nachmessen. Die Zahl,
    // die sagt, was ein Sammler wirklich braechte — alles davor ist eine
    // Schaetzung ueber Objektzahlen.
    #[cfg(feature = "heap-census")]
    if std::env::var("SWEEP").is_ok() {
        use std::sync::atomic::Ordering::Relaxed;
        let before = LIVE.load(Relaxed);
        let c = sess.interp.heap_census();
        let (dead, left) = sess.interp.collect_cycles();
        let after = LIVE.load(Relaxed);
        println!("\n── Ringe gebrochen ──");
        println!("  vorher:  {} MB, {} Objekte ({} erreichbar)", before / 1048576, c.live, c.reachable);
        println!("  geleert: {dead} Objekte, danach {left} im Verzeichnis");
        println!("  nachher: {} MB — {} MB zurueck ({} %)",
                 after / 1048576, (before - after) / 1048576,
                 if before > 0 { (before - after) * 100 / before } else { 0 });
    }
    if let Ok(want) = std::env::var("SUBMIT") {
        let dom = sess.interp.doc.as_mut().map(|d| d.to_dom());
        let Some(dom) = dom else { return };
        let forms = beak_engine::forms::collect(&dom);
        let mut state = beak_engine::forms::FormState::default();
        let seq = find_seq(dom.body(), &want);
        match seq {
            None => println!("\nSUBMIT: kein Element mit id={want}"),
            Some(seq) => {
                let t0 = std::time::Instant::now();
                let s0 = sess.interp.steps;
                let prevented = beak_engine::js::dombind::dispatch_seq(&mut sess.interp, "submit", seq);
                let mut n = 0;
                for _ in 0..64 { let t = sess.interp.run_timers(); n += t; if t == 0 { break } }
                println!("  Kosten: {} Schritte, {:?}", sess.interp.steps - s0, t0.elapsed());
                println!("  Befehle: {} ({:.2} ns je Befehl)", sess.interp.vm_ops,
                         t0.elapsed().as_nanos() as f64 / sess.interp.vm_ops.max(1) as f64);
                println!("  Maschine: {} Programme gefahren, {} abgelehnt; Aufrufe {} (davon {} langsam, {} nativ)",
                         sess.interp.vm_ran, sess.interp.vm_declined,
                         sess.interp.vm_calls, sess.interp.vm_calls_slow, sess.interp.vm_calls_native);
                let mut d: Vec<(&&'static str, &u64)> = sess.interp.func_declines.iter().collect();
                d.sort_by_key(|(_, n)| core::cmp::Reverse(**n));
                let tot: u64 = d.iter().map(|(_, n)| **n).sum();
                println!("    Rumpfe abgelehnt: {tot} in {} Sorten", d.len());
                for (why, n) in d.iter().take(8) { println!("      {why} x{n}"); }
                println!("\nSUBMIT auf #{want} (seq {seq}): {}, {n} Zeitgeber",
                         if prevented { "abgefangen" } else { "durchgelassen" });
                for l in &sess.interp.console[n0..] { println!("       | {l}"); }
                // Der Baum kann sich geaendert haben — neu einsammeln.
                let dom = sess.interp.doc.as_mut().map(|d| d.to_dom()).unwrap();
                let forms2 = beak_engine::forms::collect(&dom);
                // Dieselbe Bruecke wie im Wirt — nicht eine zweite.
                if let Some(d) = sess.interp.doc.as_ref() {
                    beak_engine::js::dombind::pull_control_values(d, &forms2, &mut state);
                }
                let asked = sess.interp.take_submits();
                let target = asked.first().copied().or(if prevented { None } else { Some(seq) });
                match target.and_then(|s| beak_engine::forms::submit_form(&forms2, &state, s)) {
                    Some(sub) => println!("  -> {} {}\n     {}",
                                          if sub.method_get { "GET" } else { "POST" },
                                          sub.action, sub.query),
                    None => println!("  -> nichts abgeschickt (Auftraege: {asked:?})"),
                }
                let _ = forms;
            }
        }
    }
    // `RENDER=<datei.bmp>` malt die Seite so, wie beak sie malt: der
    // GESKRIPTETE Baum plus ALLE Stilblaetter, die im Baum stehen — die aus
    // dem Quelltext und die, die Skripte nachgelegt haben, in Baumreihenfolge.
    // Das ist der einzige Weg, „schaut falsch aus" in etwas Nachpruefbares zu
    // verwandeln ([[feedback_trust_the_pixels_not_the_tool]]).
    if let Ok(out) = std::env::var("RENDER") {
        let Some(dom) = sess.interp.doc.as_mut().map(|d| d.to_dom()) else { return };
        let mut css = String::new();
        let mut sheets = 0;
        collect_links(dom.body(), &dir, &mut css, &mut sheets);
        // `<head>` steht nicht unter `body()` — beide Seiten des Baums.
        collect_links(&dom.root, &dir, &mut css, &mut sheets);
        let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
        use beak_engine::layout::{Rgb, Theme};
        let mut eng = beak_engine::Engine::new();
        eng.set_theme(Theme { bg: Rgb(255,255,255), text: Rgb(33,37,41), heading: Rgb(33,37,41),
                              link: Rgb(13,110,253), muted: Rgb(108,117,125), rule: Rgb(222,226,230) });
        // `H=` ist keine Kosmetik: `vh` und `min-height:100vh` haengen daran,
        // und eine Seite, die ihr Fenster fuellt, liegt sonst 225 px zu hoch.
        eng.set_viewport_h(std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(993));
        eng.set_scripted_dom(Some(dom));
        let mut lay = eng.layout_ext(&html, &css, width);
        for _ in 0..4 {
            // Dieselbe Runde wie der Wirt: erst die `data:`-Gesichter, die
            // gar kein Netz brauchen ([[feedback_the_test_path_must_be_the_real_path]]).
            let inline = eng.load_inline_fonts();
            let want = eng.take_pending_fonts();
            if want.is_empty() && !inline { break }
            for (url, family, weight, italic) in want {
                let u = resolve_path(&format!("{}/", origin()), &url);
                if let Ok(b) = std::fs::read(local(&dir, &u)) {
                    let _ = eng.add_font(family, weight, italic, &b);
                }
            }
            lay = eng.layout_ext(&html, &css, width);
        }
        // `IMGOPS=1` nennt jeden Bildbefehl mit seinem Kasten — die einzige
        // Art, einen Bildkasten zu pruefen, der auf 1 px zusammenfaellt.
        if std::env::var("IMGOPS").is_ok() {
            for o in lay.ops.iter() {
                if let beak_engine::layout::DrawOp::Image { x, y, w, h, src, .. } = o {
                    println!("IMG {x:5},{y:<5} {w:5}x{h:<5} {src}");
                }
            }
        }
        println!("  Schriften der Seite: {}", eng.web_font_count());
        let h = lay.height.clamp(1, 20000);
        let mut buf = vec![0u8; (width * h * 4) as usize];
        eng.paint(&lay, width, h, 0, &mut buf);
        std::fs::write(&out, to_bmp(&buf, width, h)).expect("schreiben");
        println!("RENDER {out}: {width}x{h}, {sheets} Blaetter ({} B CSS), {} Ops",
                 css.len(), lay.ops.len());
    }
    let listeners = sess.interp.doc.as_ref().is_some_and(|d| d.has_listeners);
    // Ein Deckel, der still zuschlaegt, macht jede Bisektion zur Messung des
    // Deckels: die letzte gedruckte Marke war die 200., nicht die letzte
    // gelaufene ([[feedback_a_read_cap_decides_what_exists]]).
    if sess.interp.console_dropped > 0 {
        println!("Konsole: {} Zeilen verworfen (Deckel)", sess.interp.console_dropped);
    }
    println!("\n{ran} gelaufen, {failed} gescheitert, {timers} Zeitgeber, {}",
             if listeners { "Ereignisse SCHARF" } else { "keine Behandler" });
}

fn pos(src: &str, at: usize) -> String {
    let mut at = at.min(src.len());
    while at > 0 && !src.is_char_boundary(at) { at -= 1; }
    let line = src[..at].bytes().filter(|b| *b == b'\n').count() + 1;
    let ls = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let le = src[at..].find('\n').map(|i| at + i).unwrap_or(src.len());
    let mut from = at.saturating_sub(48).max(ls);
    while from < at && !src.is_char_boundary(from) { from += 1; }
    let mut to = (at + 48).min(le);
    while to > at && !src.is_char_boundary(to) { to -= 1; }
    format!("{line}:{} ...{}<<HIER>>{}...", at - ls + 1, &src[from..at], &src[at..to])
}

/// Hat der Quelltext `import`/`export` auf oberster Ebene? Die Datei sagt es
/// nicht, also entscheidet der Parser: was NUR als Modul parst, ist eines.
fn is_module(src: &str) -> bool {
    beak_engine::js::parse(src, false).is_err() && beak_engine::js::parse(src, true).is_ok()
}

/// Der Ursprung, gegen den Modul-Adressen absolut werden. `import.meta.url`
/// muss eine ECHTE Adresse sein — die Komponenten der Fritzbox bauen daraus
/// `new URL(x, import.meta.url)`, und ein blosser Pfad ist da keine Basis.
fn origin() -> String {
    let u = std::env::var("URL").unwrap_or_default();
    match u.find("://") {
        Some(i) => match u[i + 3..].find('/') {
            Some(j) => u[..i + 3 + j].to_string(),
            None => u.trim_end_matches('/').to_string(),
        },
        None => String::new(),
    }
}

/// Eine Adresse gegen die des Importeurs aufloesen.
///
/// **Dieselbe Funktion, die beak am Geraet faehrt.** Die Probe hatte hier
/// erst ihre eigene — und die normalisierte `.`/`..`, waehrend beaks Wirt es
/// nicht tat. Ergebnis: host-seitig lief der Modulgraph, am Geraet explodierte
/// er (106 geladen, 179 offen). Eine Probe, die einen ANDEREN Pfad misst als
/// das Ziel, ist keine ([[feedback_the_test_path_must_be_the_real_path]]).
fn resolve_path(base: &str, spec: &str) -> String {
    use beak_engine::js::url;
    match url::parse_abs(base) {
        Some(b) => url::resolve(spec, &b).href(),
        None => spec.to_string(),
    }
}

/// Wo die Datei zu einer Adresse liegt: `curl` hat sie unter dem Pfad mit
/// `_` statt `/` abgelegt.
fn local(dir: &str, url: &str) -> String {
    let org = origin();
    let p = url.strip_prefix(&org).unwrap_or(url);
    // **Abfrage und Fragment gehoeren nicht in den Dateinamen.** `mirror.py`
    // schneidet sie ab (`urlparse(...).path`), und eine Probe, die das nicht
    // tut, sucht `css_main.css?v=1` und meldet „Blatt fehlt" fuer eine Datei,
    // die daliegt — der Cache-Buster `?v=1` ist auf echten Seiten die Regel
    // ([[feedback_the_probe_must_use_the_targets_resolver]]).
    let p = p.split(['?', '#']).next().unwrap_or(p);
    format!("{dir}/{}", p.trim_start_matches('/').replace('/', "_"))
}

fn run_module_graph(sess: &mut beak_engine::js::Session, label: &str, src: &str, dir: &str)
    -> Result<(), String> {
    // **Die Einstiegsadresse eines EXTERNEN Moduls ist seine eigene.** Der
    // Kunstname `__entry__js/main.js` war eine Basis, gegen die `./state.js`
    // zu `__entry__js/state.js` wurde — eine Datei, die es nirgends gibt.
    // Nur ein INLINE-Modul hat keine Adresse und braucht eine erfundene.
    let entry = if label.starts_with("inline ") {
        format!("{}/__entry__{}", origin(), label.replace(' ', "_"))
    } else {
        resolve_path(&format!("{}/", origin()), label)
    };
    let prog = beak_engine::js::parse(src, true).map_err(|e| format!("SyntaxError: {} @{}", e.msg, pos(src, e.at)))?;
    sess.interp.add_module(&entry, std::rc::Rc::new(prog));
    // Holen, bis der Graph geschlossen ist.
    let mut queue = vec![entry.clone()];
    while let Some(u) = queue.pop() {
        for spec in sess.interp.module_requests(&u) {
            let r = resolve_path(&u, &spec);
            sess.interp.map_module_dep(&u, &spec, &r);
            if sess.interp.has_module(&r) { continue }
            let text = std::fs::read_to_string(local(dir, &r))
                .map_err(|e| format!("{r}: nicht da ({e})"))?;
            let p = beak_engine::js::parse(&text, true)
                .map_err(|e| format!("{r}: SyntaxError: {} @{}", e.msg, pos(&text, e.at)))?;
            sess.interp.add_module(&r, std::rc::Rc::new(p));
            queue.push(r);
        }
    }
    sess.interp.module_fail = None;
    sess.interp.eval_module(&entry).map_err(|e| {
        let msg = beak_engine::js::modules::describe(&mut sess.interp, e);
        match sess.interp.module_fail.clone() {
            Some(u) if &*u != entry.as_str() => format!("{msg}   [in {u}]"),
            _ => msg,
        }
    })
}

/// `CLICK=<id>[,<id>…]` — einen Knopf druecken, auf dem Weg des WIRTS.
///
/// Nicht `dispatch` auf den Knoten aus dem Baum: beak nimmt die Zustellkette
/// aus dem LAYOUT (`element_chain` auf der Mitte des gemalten Kastens), und
/// genau dort ist schon einmal ein Klick gestorben, den der Baumweg als
/// zugestellt meldete ([[feedback_the_test_path_must_be_the_real_path]]).
/// Auch der Schnellweg-Wächter des Wirts steht hier — sonst behauptet die
/// Probe eine Zustellung, die am Geraet gar nicht erst versucht wird.
fn click_ids(sess: &mut beak_engine::js::Session, html: &str, dir: &str, spec: &str) {
    for id in spec.split(',') {
        let id = id.trim();
        if id.is_empty() { continue }
        // Je Klick neu auslegen — ein Behandler, der den Baum aendert,
        // verschiebt die Kaesten fuer den naechsten.
        let Some(lay) = page_layout(&mut sess.interp, html, dir) else { return };
        let Some(dom) = sess.interp.doc.as_mut().map(|d| d.to_dom()) else { return };
        let Some(seq) = find_seq(&dom.root, id) else {
            println!("CLICK: kein Element mit id={id}"); continue };
        let Some(r) = lay.element_rects().into_iter().find(|r| r.seq == seq) else {
            println!("CLICK #{id}: kein Kasten im Layout — unsichtbar oder nicht gemalt");
            continue };
        let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
        let chain = lay.element_chain(cx, cy);
        let Some(doc) = sess.interp.doc.as_ref() else { return };
        let nodes: Vec<u32> = chain.iter().filter_map(|s| doc.by_seq(*s)).collect();
        let listeners = doc.has_listeners;
        let hit = chain.contains(&seq);
        let on_label = nodes.last().is_some_and(|n|
            beak_engine::js::dombind::label_target(&sess.interp, *n).is_some());
        if nodes.is_empty() || (!listeners && !on_label) {
            println!("CLICK #{id} (seq {seq}) bei ({cx},{cy}): der Wirt stellt hier NICHT zu \
                      (Kette {} Knoten, Behandler {listeners}, Schild {on_label})", nodes.len());
            continue;
        }
        let n0 = sess.interp.console.len();
        let prevented = matches!(
            beak_engine::js::dombind::dispatch(&mut sess.interp, "click", &nodes), Ok(true));
        let mut timers = 0;
        for _ in 0..64 { let t = sess.interp.run_timers(); timers += t; if t == 0 { break } }
        let changed = sess.interp.doc.as_ref().is_some_and(|d| d.dirty);
        println!("CLICK #{id} (seq {seq}) bei ({cx},{cy}): Kette {} Knoten (eigenes seq {}), {}, \
{timers} Zeitgeber, Baum {}",
                 nodes.len(), if hit { "drin" } else { "FEHLT" },
                 if prevented { "abgefangen" } else { "durchgelassen" },
                 if changed { "GEAENDERT" } else { "unveraendert" });
        for l in &sess.interp.console[n0..] { println!("       | {l}"); }
        if let Some(n) = sess.interp.take_nav() { println!("       NAVIGATION -> {}", n.url); }
        for seq in sess.interp.take_submits() { println!("       Absende-Auftrag: seq={seq}"); }
    }
}

/// Der ROHE Text eines Elements — ohne Kuerzung, ohne Umbau.
///
/// `DUMP=1` schneidet Text bei 60 Zeichen ab; fuer eine Sonde, die ihr
/// Ergebnis in ein `<pre>` schreibt, ist das wertlos. **Und die Konsole ist
/// der falsche Weg**: sie haelt 200 Zeilen, danach faellt still weg, was
/// kommt — eine Messung, die auf halber Strecke aufhoert und wie eine Luecke
/// im Layout aussieht ([[feedback_a_read_cap_decides_what_exists]]).
fn raw_text(e: &beak_engine::dom::Element, out: &mut String) {
    for c in &e.children {
        match c {
            beak_engine::dom::Node::Text(t) => out.push_str(t),
            beak_engine::dom::Node::Element(x) => raw_text(x, out),
            _ => {}
        }
    }
}

fn find_el<'a>(e: &'a beak_engine::dom::Element, id: &str)
    -> Option<&'a beak_engine::dom::Element> {
    if e.attr("id") == Some(id) { return Some(e) }
    for c in &e.children {
        if let beak_engine::dom::Node::Element(x) = c {
            if let Some(f) = find_el(x, id) { return Some(f) }
        }
    }
    None
}

/// Den Baum als Umriss: Marke, id/class, und Text gekuerzt.
fn dump(e: &beak_engine::dom::Element, depth: usize, out: &mut String) {
    if depth > 12 { return }
    let pad = "  ".repeat(depth);
    let id = e.attr("id").map(|v| format!("#{v}")).unwrap_or_default();
    let cls = e.attr("class").map(|v| format!(".{}", v.replace(' ', "."))).unwrap_or_default();
    out.push_str(&format!("{pad}<{}{id}{cls}>\n", e.tag));
    for c in &e.children {
        match c {
            beak_engine::dom::Node::Element(x) => dump(x, depth + 1, out),
            beak_engine::dom::Node::Text(t) => {
                let t = t.trim();
                if !t.is_empty() {
                    let t: String = t.chars().take(60).collect();
                    out.push_str(&format!("{pad}  \"{t}\"\n"));
                }
            }
            _ => {}
        }
    }
}

/// Die `seq` des Elements mit dieser id.
fn find_seq(el: &beak_engine::dom::Element, id: &str) -> Option<u32> {
    if el.attr("id") == Some(id) { return Some(el.seq) }
    for c in &el.children {
        if let beak_engine::dom::Node::Element(e) = c {
            if let Some(s) = find_seq(e, id) { return Some(s) }
        }
    }
    None
}

/// Ein Blatt anhaengen — seine `@import`e ZUERST, denn ein Import wirkt, als
/// staende sein Inhalt an seiner Stelle, also vor allem, was im Blatt folgt.
/// Dieselbe Reihenfolge, die der Wirt baut.
fn push_sheet(text: &str, url: &str, dir: &str, out: &mut String, n: &mut usize, depth: usize) {
    if depth < 4 {
        for href in beak_engine::import_urls(text) {
            let u = resolve_path(url, &href);
            match std::fs::read_to_string(local(dir, &u)) {
                Ok(t) => push_sheet(&t, &u, dir, out, n, depth + 1),
                Err(_) => eprintln!("  Import fehlt: {u}"),
            }
        }
    }
    out.push_str(text);
    out.push('\n');
    *n += 1;
}

/// Jedes `<link rel=stylesheet>` im Baum, in Baumreihenfolge, aus dem
/// Verzeichnis gelesen. Dieselbe Reihenfolge, in der der Wirt sie anhaengt.
fn collect_links(el: &beak_engine::dom::Element, dir: &str, out: &mut String, n: &mut usize) {
    if el.tag == "link"
        && el.attr("rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet")) {
        if let Some(h) = el.attr("href") {
            let u = resolve_path(&format!("{}/", origin()), h);
            match std::fs::read_to_string(local(dir, &u)) {
                Ok(t) => {
                    // **`@import` MUSS die Probe auch fahren.** Der Wirt tut es
                    // seit 0.139.0; eine Probe, die es nicht tut, misst sich
                    // selbst und nicht beak — bei sandbox.nopeek.ch haengt die
                    // ganze Gestaltung an fuenfzehn `@import`-Zeilen, und ohne
                    // sie sieht jeder Vergleich wie ein Layoutfehler aus
                    // ([[feedback_the_test_path_must_be_the_real_path]]).
                    push_sheet(&t, &u, dir, out, n, 0);
                }
                Err(_) => eprintln!("  Blatt fehlt: {u}"),
            }
        }
    }
    for c in &el.children {
        if let beak_engine::dom::Node::Element(e) = c { collect_links(e, dir, out, n); }
    }
}

/// BGRA nach BMP, von unten nach oben — wie das Format es will.
fn to_bmp(px: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = (w * 3 + 3) & !3;
    let size = 54 + (row * h) as usize;
    let mut o = Vec::with_capacity(size);
    o.extend_from_slice(b"BM");
    o.extend_from_slice(&(size as u32).to_le_bytes());
    o.extend_from_slice(&[0; 4]);
    o.extend_from_slice(&54u32.to_le_bytes());
    o.extend_from_slice(&40u32.to_le_bytes());
    o.extend_from_slice(&(w as i32).to_le_bytes());
    o.extend_from_slice(&(h as i32).to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&24u16.to_le_bytes());
    o.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            o.extend_from_slice(&[px[i], px[i + 1], px[i + 2]]);
        }
        for _ in 0..(row - w * 3) { o.push(0); }
    }
    o
}

/// Einmal auslegen und der Maschine die Kaesten reichen — sonst antwortet
/// `getBoundingClientRect` mit Nullen, und eine Probe, die misst, misst nichts.
/// Die Seite auslegen, wie der Wirt sie auslegt: der GESKRIPTETE Baum, alle
/// Blaetter aus dem Baum, die Schriftrunde. Zwei Aufrufer — die Geometrie fuer
/// `getBoundingClientRect`, und der Klickpunkt fuer `CLICK`. **Eine Quelle**,
/// sonst misst die eine Seite etwas anderes als die andere.
thread_local! {
    /// Das HTML und das Spiegelverzeichnis fuer den Relayout-Haken. Ein
    /// `fn`-Zeiger faengt nichts ein, also muessen die zwei Dinge, die
    /// `feed_geometry` braucht, hier stehen — beim Wirt sind es ohnehin
    /// Globale (`html_str()`, `css_str()`).
    static HTML: core::cell::RefCell<String> = const { core::cell::RefCell::new(String::new()) };
    static DIR: core::cell::RefCell<String> = const { core::cell::RefCell::new(String::new()) };
}

/// **Neu auslegen auf Verlangen** — derselbe Weg, den der Wirt je Bild faehrt,
/// nur jetzt aus der Maschine heraus gerufen. Ohne ihn misst die Probe eine
/// Engine ohne diesen Weg und findet den Fehler nicht, den sie suchen soll
/// ([[feedback_the_test_path_must_be_the_real_path]]).
fn host_relayout(ip: &mut beak_engine::js::interp::Interp) {
    let dir = DIR.with(|d| d.borrow().clone());
    HTML.with(|h| {
        let html = h.borrow();
        feed_geometry(ip, &html, &dir);
    });
}

/// Mikrosekunden seit Prozessstart — die Uhr, aus der `Layout::phase` seine
/// drei Zahlen rechnet. Ohne sie steht dort dreimal null.
fn mono_us() -> u64 {
    use std::sync::OnceLock;
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_micros() as u64
}

fn page_layout(ip: &mut beak_engine::js::interp::Interp, html: &str, dir: &str)
    -> Option<beak_engine::layout::Layout> {
    let t_dom = std::time::Instant::now();
    let dom = ip.doc.as_mut().map(|d| d.to_dom())?;
    let d_dom = t_dom.elapsed();
    let t_css = std::time::Instant::now();
    let mut css = String::new();
    let mut n = 0;
    collect_links(dom.body(), dir, &mut css, &mut n);
    collect_links(&dom.root, dir, &mut css, &mut n);
    let d_css = t_css.elapsed();
    if std::env::var("CSSDBG").is_ok() {
        eprintln!("[css] {n} Blaetter, {} B, .main-area: {}", css.len(), css.contains(".main-area"));
    }
    let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
    use beak_engine::layout::{Rgb, Theme};
    // **EINE Engine fuer alle Auslegungen, wie beim Wirt.** Eine frische je
    // Messung hat jeden Zwischenspeicher kalt — Dokument, Blatt, Schriften —
    // und die Probe misst dann den ersten Aufbau statt das Neuauslegen, das
    // sie messen will ([[feedback_the_test_path_must_be_the_real_path]]).
    thread_local! {
        static ENG: beak_engine::Engine = {
            let mut e = beak_engine::Engine::new();
            // Das Thema entscheidet `prefers-color-scheme` in der Kaskade —
            // es MUSS zur Medienlage passen, sonst rechnet das Layout mit
            // einem anderen Schema als das Skript liest.
            e.set_theme(if std::env::var("DARK").is_ok() {
                Theme { bg: Rgb(18,18,18), text: Rgb(222,226,230), heading: Rgb(240,240,240),
                        link: Rgb(110,168,254), muted: Rgb(150,155,160), rule: Rgb(60,63,66) }
            } else {
                Theme { bg: Rgb(255,255,255), text: Rgb(33,37,41), heading: Rgb(33,37,41),
                        link: Rgb(13,110,253), muted: Rgb(108,117,125), rule: Rgb(222,226,230) }
            });
            e
        };
    }
    ENG.with(|eng| {
    // `H=` gehoert zur MESSUNG, nicht zur Kosmetik: `vh` und
    // `min-height:100vh` haengen daran. Mit der Vorgabe 600 statt der echten
    // Fensterhoehe lag die ganze Fritzbox-Seite 225 px zu hoch, und der
    // Vergleich mit Chromium meldete 26 Abweichungen, die es nicht gibt.
    eng.set_viewport_h(std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(993));
    // Ohne diese Zeile zeichnet das Layout gar keine Element-Kaesten auf, und
    // `element_rects()` ist leer — derselbe Schalter, den der Wirt setzt,
    // sobald eine Seite Skripte faehrt.
    eng.set_hit_all(true);
    eng.set_clock(mono_us);
    eng.set_scripted_dom(Some(dom));
    let t_lay = std::time::Instant::now();
    let mut lay = eng.layout_ext(html, &css, width);
    let d_lay = t_lay.elapsed();
    let mut n_lay = 1;
    // Die Schriften der Seite holen und NOCHMAL auslegen — dieselbe Runde,
    // die der Wirt faehrt. Ohne den zweiten Lauf misst die Probe mit der
    // eingebauten Schrift und vergleicht dann Breiten, die es nicht gibt.
    let (mut ok, mut bad) = (0usize, 0usize);
    for _ in 0..4 {
        let inline = eng.load_inline_fonts();
        let want = eng.take_pending_fonts();
        if want.is_empty() && !inline { break }
        for (url, family, weight, italic) in want {
            let u = resolve_path(&format!("{}/", origin()), &url);
            match std::fs::read(local(dir, &u)) {
                Ok(b) if eng.add_font(family, weight, italic, &b) => ok += 1,
                _ => { bad += 1; eprintln!("  Schrift nicht ladbar: {u}"); }
            }
        }
        lay = eng.layout_ext(html, &css, width);
        n_lay += 1;
    }
    if ok + bad > 0 { println!("Schriften: {ok} geladen, {bad} gescheitert"); }
    // **Wo die Zeit eines erzwungenen Neuauslegens stuende.** `to_dom` baut den
    // Baum zurueck, `collect_links` sammelt die Blaetter, `layout_ext` parst
    // das HTML, faehrt die Kaskade und legt die Kaesten. Drei Zahlen statt
    // einer, weil nur eine davon unvermeidlich ist.
    if std::env::var("PHASEDBG").is_ok() {
        eprintln!("  PHASE to_dom {:.1} ms · Blaetter {:.1} ms ({} B) · layout_ext {:.1} ms [parse {:.1} · cascade {:.1} · box {:.1}] · {} Layouts, {} Kaesten",
            d_dom.as_secs_f64()*1000.0, d_css.as_secs_f64()*1000.0, css.len(),
            d_lay.as_secs_f64()*1000.0,
            lay.phase[0] as f64/1000.0, lay.phase[1] as f64/1000.0, lay.phase[2] as f64/1000.0,
            n_lay, lay.element_rects().len());
    }
    Some(lay)
    })
}

fn feed_geometry(ip: &mut beak_engine::js::interp::Interp, html: &str, dir: &str) {
    let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
    let Some(lay) = page_layout(ip, html, dir) else { return };
    let rects = lay.element_rects();
    if std::env::var("GEOMDBG").is_ok() {
        eprintln!("  Geometrie: {} Kaesten, Layouthoehe {}", rects.len(), lay.height);
        for r in rects.iter().take(8) { eprintln!("    seq={} {},{} {}x{}", r.seq, r.x, r.y, r.w, r.h); }
    }
    ip.set_geometry(beak_engine::js::interp::Geometry {
        boxes: std::rc::Rc::new(rects), scroll: (0, 0),
        content: (width as i32, lay.height as i32),
    });
    ip.set_media(width as f64, 1080.0, std::env::var("DARK").is_ok());
}
