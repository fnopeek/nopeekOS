//! Runs a page's whole script round host-side: one session, document order,
//! the same module fallback as beak.
//!
//! `jsrun` runs one file alone, which answers the wrong question: in beak all
//! scripts share one global scope, and a script that throws
//! `x is not defined` alone may run cleanly in the chain.
//!
//!   cargo run --release --example pagerun -- page.html dir/
//!
//! External scripts are not fetched; they are expected in the directory under
//! the last path component of their `src` (as `curl` stores them). A missing
//! file is reported, not silently skipped.
use beak_engine::js::dombind::ScriptRef;

/// A `fetch` host served from the mirror directory, so a page's API layer can
/// run host-side.
///
/// It does not guess: a missing file becomes a rejection with `TypeError`,
/// like a network error in a browser, not an empty 200 response.
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
            Err(e) => {
                println!("fetch fehlt: {} {}", f.method, u);
                beak_engine::js::fetch::fetch_failed(&mut sess.interp, f.id, &e.to_string())
            }
        }
    }
    n
}

/// The `<script src=…>` a script inserted, served from the mirror directory
/// as the host serves them from the network. Every code-splitting bundler
/// loads this way; without it a promise that never settles looks like a
/// hanging page.
/// `import()`: deliver what the waiting graphs need from the mirror, then
/// let the engine evaluate and settle them. The same order beak uses.
fn serve_dyn_imports(sess: &mut beak_engine::js::Session, dir: &str) -> usize {
    let mut n = 0;
    for _ in 0..32 {
        let want = sess.interp.dynamic_import_wants();
        if want.is_empty() { break }
        for u in want {
            n += 1;
            let parsed = std::fs::read_to_string(local(dir, &u)).ok()
                .and_then(|t| beak_engine::js::parse(&t, true).map_err(|e| {
                    println!("  import() {u}: SyntaxError: {} @{}", e.msg, pos(&t, e.at));
                }).ok());
            match parsed {
                Some(p) => sess.interp.add_module(&u, std::rc::Rc::new(p)),
                None => {
                    println!("  import() fehlt: {u}");
                    sess.interp.module_unavailable(&u);
                }
            }
        }
    }
    n + sess.interp.settle_dynamic_imports()
}

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

/// Randomness for host tools, from `/dev/urandom`. Without it there is no
/// `crypto`, and the tool would measure a different platform than beak.
fn host_random(out: &mut [u8]) -> bool {
    use std::io::Read;
    match std::fs::File::open("/dev/urandom") {
        Ok(mut f) => f.read_exact(out).is_ok(),
        Err(_) => false,
    }
}

/// An allocator that reports large requests with a backtrace (`LOUD=<bytes>`).
///
/// RSS shows a leak but not who allocates: a sampling profile shows CPU time,
/// and a single huge allocation costs none.
struct Loud;
static LOUD_LIMIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);
/// What is really allocated right now: allocations minus frees.
///
/// RSS is the wrong figure: it shows what the host allocator kept from the
/// system, not what the page holds, and beak's limit applies to the latter.
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

/// What the process occupies right now (RSS in MB).
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

    // Same cap as the host, so the probe does not fail at a limit beak does
    // not have.
    let mut sess = beak_engine::js::Session::new(20_000_000_000);
    // `NOVM=1`: force the tree walker. The comparison tells whether the page's
    // hot code runs on the bytecode machine at all.
    if std::env::var("NOVM").is_ok() { sess.interp.vm_off = true; }
    sess.interp.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
    let media = beak_engine::css::Media::new(1902.0, false);
    // Linked stylesheets belong in the style context. Without them
    // `getComputedStyle` answers only from the page's `<style>` blocks, and
    // every framework class looks absent.
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
    // `DARK=1` runs the probe in dark mode, the state the host submits from
    // `query_theme().is_dark()`. Without it every run is light and cannot
    // tell whether a page reads the scheme at all.
    let dark = std::env::var("DARK").is_ok();
    sess.interp.set_media(1902.0, 1000.0, dark);
    // `DEPTH=` raises the call-depth cap, to tell a real endless recursion
    // from one that is merely deeper than the default.
    if let Ok(d) = std::env::var("DEPTH") {
        if let Ok(n) = d.parse() { sess.interp.max_depth = n; }
    }
    // `STEPS=` raises the step cap, to see what the page really costs.
    if let Ok(d) = std::env::var("STEPS") {
        if let Ok(n) = d.parse() { sess.interp.max_steps = n; }
    }
    if std::env::var("NOCLOCK").is_err() { sess.interp.clock = Some(host_clock); }
    // Relayout on demand. `NORELAYOUT=1` removes it, for A/B comparisons.
    if std::env::var("NORELAYOUT").is_err() {
        HTML.with(|h| *h.borrow_mut() = html.clone());
        DIR.with(|d| *d.borrow_mut() = dir.clone());
        sess.interp.relayout = Some(host_relayout);
    }
    if let Ok(u) = std::env::var("URL") { sess.interp.set_location(&u); }
    sess.interp.load_import_maps();
    // `BUDGET=<seconds>`: the same deadline the host sets. Without it a page
    // runs unbounded host-side.
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
                // Same path mapping as the stylesheets: `local` strips the
                // query and flattens the path, as `mirror.py` does.
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
        // A module: fetch the whole graph, then link, then evaluate, the
        // order beak uses.
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
        // `document.currentScript`: a module has none (HTML §4.12.1), which
        // is why it is set only on this branch.
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
    // The host's order is the point: stylesheet rounds first (a fetched sheet
    // lets a component finish building), then `DOMContentLoaded`, then one
    // layout, and only then `load`. Submitting geometry before the rounds
    // measures a tree that never existed: the components are still empty.
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
            // The allocator counts the heap, the engine the census. Only the
            // latter depends on `heap-census`; without the feature the probe
            // must still build.
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
        // Deliver first, then run the clock. A waiting page must not have its
        // timers advanced, or a bundler's timeout fires before the chunk it
        // guards is delivered.
        fetches += serve_fetches(&mut sess, &dir);
        let want = sess.interp.take_pending_sheets();
        let js = serve_dyn_scripts(&mut sess, &dir) + serve_dyn_imports(&mut sess, &dir);
        dynjs += js;
        let t = sess.interp.run_timers();
        timers += t;
        // See `GEOMEVERY` below: the host measures every frame, also while
        // scripts are still running. Frameworks mount components in this
        // round; without measuring here every freshly inserted element reports
        // `offsetWidth == 0`.
        if std::env::var("GEOMEVERY").is_ok() { feed_geometry(&mut sess.interp, &html, &dir); }
        if t == 0 && js == 0 && want.is_empty() && sess.interp.pending_fetches.is_empty() { break }
        for (id, href) in want {
            let u = resolve_path(&format!("{}/", origin()), &href);
            let ok = std::fs::read_to_string(local(&dir, &u)).is_ok();
            if ok { sheets_ok += 1 } else { sheets_bad += 1; println!("  dyn-Blatt fehlt: {u}"); }
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
    // `GEOMEVERY=1` inserts a frame between two timer rounds. The host
    // re-measures every frame (`set_geometry` in `beak/src/lib.rs`); the probe
    // otherwise does it once, and every element a script inserts later
    // reports `offsetWidth == 0` forever.
    let geom_every = std::env::var("GEOMEVERY").is_ok();
    for _ in 0..64 {
        let f = serve_fetches(&mut sess, &dir);
        fetches += f;
        let js = serve_dyn_scripts(&mut sess, &dir) + serve_dyn_imports(&mut sess, &dir);
        dynjs += js;
        let t = sess.interp.run_timers();
        timers += t;
        if geom_every { feed_geometry(&mut sess.interp, &html, &dir); }
        if t == 0 && f == 0 && js == 0 { break }
    }
    if dynjs > 0 { println!("Skripte per Skript: {dynjs} eingehaengt"); }
    if fetches > 0 { println!("fetch: {fetches} Anfragen aus dem Spiegel bedient"); }
    // What the page requested via `location`. Without this a page that
    // redirects itself looks the same as one that does nothing.
    if let Some(n) = sess.interp.take_nav() {
        println!("NAVIGATION {}{} -> {}",
                 if n.replace { "replace" } else { "assign" },
                 if n.reload { "/reload" } else { "" }, n.url);
    }
    let mut n1 = sess.interp.console.len();
    for l in &sess.interp.console[n0..] { println!("  timer| {l}"); }
    // `DUMP=1` shows the final tree: "did the scripts run" is not the same
    // question as "did they build anything".
    if std::env::var("DUMP").is_ok() {
        if let Some(d) = sess.interp.doc.as_mut() {
            let dom = d.to_dom();
            let mut out = String::new();
            dump(dom.body(), 0, &mut out);
            println!("\n── Baum nach den Skripten ──\n{out}");
        }
    }
    // `SUBMIT=<id>` runs the whole submit path: the `submit` event, what the
    // handler computes, the jobs from `form.submit()`, and the resulting
    // request, in the host's order.
    // `TYPE=id=value[,id=value]` types into fields before submitting. It
    // writes the dirty value, exactly what a keystroke in the host sets.
    if let Ok(spec) = std::env::var("TYPE") {
        let dom = sess.interp.doc.as_mut().map(|d| d.to_dom());
        if let Some(dom) = dom {
            for pair in spec.split(',') {
                let Some((id, v)) = pair.split_once('=') else { continue };
                let Some(seq) = find_seq(dom.body(), id) else {
                    println!("TYPE: kein Element mit id={id}"); continue };
                // Character by character with the events, as the host does
                // (`edit_key`). Writing the value in one go would skip the
                // `input` events that e.g. a suggestion list depends on.
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
                // Finish what a handler started: a suggestion list fetches
                // its answer with `fetch`.
                for _ in 0..32 {
                    let f = serve_fetches(&mut sess, &dir);
                    let j = serve_dyn_scripts(&mut sess, &dir) + serve_dyn_imports(&mut sess, &dir);
                    let t = sess.interp.run_timers();
                    if f == 0 && j == 0 && t == 0 { break }
                }
            }
        }
    }
    if let Ok(spec) = std::env::var("CLICK") { click_ids(&mut sess, &html, &dir, &spec); }
    // `TEXT=<id>` prints the full content of one element: the way an inserted
    // probe hands out its result, the same one Chromium uses with `--dump-dom`.
    // Console output written during `TYPE`/`CLICK`/`SUBMIT`. The line above
    // drains the console after the script round; this shows what handlers
    // wrote afterwards.
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
    // `SWEEP=1`: break unreachable cycles and measure again. This is what a
    // collector would actually free; everything before is an estimate from
    // object counts.
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
                // The tree may have changed; collect again.
                let dom = sess.interp.doc.as_mut().map(|d| d.to_dom()).unwrap();
                let forms2 = beak_engine::forms::collect(&dom);
                // The same bridge as the host, not a second one.
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
    // `RENDER=<file.bmp>` paints the page as beak does: the scripted tree plus
    // all stylesheets in the tree, from the source and those added by
    // scripts, in tree order.
    if let Ok(out) = std::env::var("RENDER") {
        let Some(dom) = sess.interp.doc.as_mut().map(|d| d.to_dom()) else { return };
        let mut css = String::new();
        let mut sheets = 0;
        collect_links(dom.body(), &dir, &mut css, &mut sheets);
        // `<head>` is not under `body()`; both sides of the tree.
        collect_links(&dom.root, &dir, &mut css, &mut sheets);
        let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
        use beak_engine::layout::{Rgb, Theme};
        let mut eng = beak_engine::Engine::new();
        eng.set_theme(Theme { bg: Rgb(255,255,255), text: Rgb(33,37,41), heading: Rgb(33,37,41),
                              link: Rgb(13,110,253), muted: Rgb(108,117,125), rule: Rgb(222,226,230) });
        // `H=` matters: `vh` and `min-height:100vh` depend on it.
        eng.set_viewport_h(std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(993));
        eng.set_scripted_dom(Some(dom));
        let mut lay = eng.layout_ext(&html, &css, width);
        for _ in 0..4 {
            // Same round as the host: first the `data:` faces, which need no
            // network.
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
        // `IMGOPS=1` lists each image command with its box, to check an image
        // box that collapses to 1 px.
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
    // A cap that hits silently turns every bisection into a measurement of
    // the cap.
    if sess.interp.console_dropped > 0 {
        println!("Konsole: {} Zeilen verworfen (Deckel)", sess.interp.console_dropped);
    }
    // Function bodies the compiler declined run on the tree walker, which
    // cannot suspend: an async one among them fails at its first `await`.
    let mut d: Vec<(&&'static str, &u64)> = sess.interp.func_declines.iter().collect();
    if !d.is_empty() {
        d.sort_by_key(|(_, n)| core::cmp::Reverse(**n));
        let tot: u64 = d.iter().map(|(_, n)| **n).sum();
        println!("Rumpfe abgelehnt: {tot} in {} Sorten", d.len());
        for (why, n) in d.iter().take(8) { println!("  {why} x{n}"); }
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

/// Whether the source has top-level `import`/`export`. The file does not
/// say, so the parser decides: what parses only as a module is one.
fn is_module(src: &str) -> bool {
    beak_engine::js::parse(src, false).is_err() && beak_engine::js::parse(src, true).is_ok()
}

/// The origin module URLs are resolved against. `import.meta.url` must be a
/// real URL: components build `new URL(x, import.meta.url)`, and a bare path
/// is no base there.
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

/// Resolves a URL against the importer's.
///
/// The same function beak uses; a probe with its own resolution would
/// measure a different path than the target.
fn resolve_path(base: &str, spec: &str) -> String {
    use beak_engine::js::url;
    match url::parse_abs(base) {
        Some(b) => url::resolve(spec, &b).href(),
        None => spec.to_string(),
    }
}

/// Where the file for a URL lives: `curl` stored it under the path with `_`
/// instead of `/`.
fn local(dir: &str, url: &str) -> String {
    let org = origin();
    let p = url.strip_prefix(&org).unwrap_or(url);
    // Query and fragment do not belong in the file name: `mirror.py` strips
    // them (`urlparse(...).path`), and cache busters like `?v=1` are common.
    let p = p.split(['?', '#']).next().unwrap_or(p);
    format!("{dir}/{}", p.trim_start_matches('/').replace('/', "_"))
}

fn run_module_graph(sess: &mut beak_engine::js::Session, label: &str, src: &str, dir: &str)
    -> Result<(), String> {
    // The entry URL of an external module is its own; only an inline module
    // has no URL and needs a made-up one.
    let entry = if label.starts_with("inline ") {
        format!("{}/__entry__{}", origin(), label.replace(' ', "_"))
    } else {
        resolve_path(&format!("{}/", origin()), label)
    };
    let prog = beak_engine::js::parse(src, true).map_err(|e| format!("SyntaxError: {} @{}", e.msg, pos(src, e.at)))?;
    sess.interp.add_module(&entry, std::rc::Rc::new(prog));
    // Fetch until the graph is closed.
    let mut queue = vec![entry.clone()];
    while let Some(u) = queue.pop() {
        for spec in sess.interp.module_requests(&u) {
            let r = sess.interp.resolve_module(&u, &spec).map_err(|e| format!("{u}: {e}"))?;
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

/// `CLICK=<id>[,<id>…]`: press a button the way the host does.
///
/// Not `dispatch` on the tree node: beak takes the dispatch chain from the
/// layout (`element_chain` at the centre of the painted box). The host's
/// fast-path guard is here too, or the probe would claim a delivery the host
/// never attempts.
fn click_ids(sess: &mut beak_engine::js::Session, html: &str, dir: &str, spec: &str) {
    for id in spec.split(',') {
        let id = id.trim();
        if id.is_empty() { continue }
        // Lay out again per click: a handler that changes the tree moves the
        // boxes for the next one.
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

/// The raw text of an element, untruncated and unaltered.
///
/// `DUMP=1` cuts text at 60 characters, useless for a probe that writes its
/// result into a `<pre>`. The console is the wrong channel too: it holds a
/// limited number of lines and silently drops the rest.
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

/// The tree as an outline: tag, id/class, and truncated text.
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

/// The `seq` of the element with this id.
fn find_seq(el: &beak_engine::dom::Element, id: &str) -> Option<u32> {
    if el.attr("id") == Some(id) { return Some(el.seq) }
    for c in &el.children {
        if let beak_engine::dom::Node::Element(e) = c {
            if let Some(s) = find_seq(e, id) { return Some(s) }
        }
    }
    None
}

/// Appends a stylesheet with its `@import`s first: an import acts as if its
/// content stood in its place, i.e. before everything that follows in the
/// sheet. Same order the host builds.
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

/// Every `<link rel=stylesheet>` in the tree, in tree order, read from the
/// directory. Same order in which the host appends them.
fn collect_links(el: &beak_engine::dom::Element, dir: &str, out: &mut String, n: &mut usize) {
    if el.tag == "link"
        && el.attr("rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet")) {
        if let Some(h) = el.attr("href") {
            let u = resolve_path(&format!("{}/", origin()), h);
            match std::fs::read_to_string(local(dir, &u)) {
                Ok(t) => {
                    // The probe must follow `@import` as the host does; a
                    // page may carry its entire styling in imports.
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

/// BGRA to BMP, bottom-up as the format requires.
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

thread_local! {
    /// The HTML and mirror directory for the relayout hook. A `fn` pointer
    /// captures nothing, so the two things `feed_geometry` needs live here;
    /// in the host they are globals anyway (`html_str()`, `css_str()`).
    static HTML: core::cell::RefCell<String> = const { core::cell::RefCell::new(String::new()) };
    static DIR: core::cell::RefCell<String> = const { core::cell::RefCell::new(String::new()) };
}

/// Relayout on demand: the path the host runs per frame, called from inside
/// the machine.
fn host_relayout(ip: &mut beak_engine::js::interp::Interp) {
    let dir = DIR.with(|d| d.borrow().clone());
    HTML.with(|h| {
        let html = h.borrow();
        feed_geometry(ip, &html, &dir);
    });
}

/// Microseconds since process start, the clock `Layout::phase` computes its
/// three figures from. Without it all three are zero.
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
    // One engine for all layouts, as in the host. A fresh one per measurement
    // has every cache cold (document, sheet, fonts) and measures the first
    // build instead of the relayout.
    thread_local! {
        static ENG: beak_engine::Engine = {
            let mut e = beak_engine::Engine::new();
            // The theme decides `prefers-color-scheme` in the cascade; it must
            // match the media state, or layout and script see different
            // schemes.
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
    // `H=` is part of the measurement: `vh` and `min-height:100vh` depend on
    // it.
    eng.set_viewport_h(std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(993));
    // Without this the layout records no element boxes and `element_rects()`
    // is empty; the same switch the host sets once a page runs scripts.
    eng.set_hit_all(true);
    eng.set_clock(mono_us);
    eng.set_scripted_dom(Some(dom));
    let t_lay = std::time::Instant::now();
    let mut lay = eng.layout_ext(html, &css, width);
    let d_lay = t_lay.elapsed();
    let mut n_lay = 1;
    // Fetch the page's fonts and lay out again, the round the host runs.
    // Without it the probe measures with the built-in font.
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
    // Where the time of a forced relayout goes: `to_dom` rebuilds the tree,
    // `collect_links` gathers the sheets, `layout_ext` parses the HTML, runs
    // the cascade and lays out boxes. Three figures, because only one of them
    // is unavoidable.
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

/// Lays the page out as the host does (scripted tree, all sheets from the
/// tree, the font round) and hands the boxes to the machine, or
/// `getBoundingClientRect` answers with zeros. One source for both callers:
/// the geometry for `getBoundingClientRect` and the click point for `CLICK`.
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
