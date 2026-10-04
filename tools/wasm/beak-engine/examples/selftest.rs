// Runs the built-in test page host-side, the same file beak ships. A line
// that fails here should fail in beak too; a difference is a finding.
use beak_engine::js::dombind::Doc;

/// Randomness for host tools, from `/dev/urandom`. Without it there is no
/// `crypto`, and the tool would measure a different platform than beak.
fn host_random(out: &mut [u8]) -> bool {
    use std::io::Read;
    match std::fs::File::open("/dev/urandom") {
        Ok(mut f) => f.read_exact(out).is_ok(),
        Err(_) => false,
    }
}

thread_local! {
    /// The engine the forced relayout comes from, the same one that later
    /// builds the click chains. A `fn` pointer captures nothing, so it has to
    /// live here; in beak it is a global for the same reason
    /// (`beak/src/lib.rs`).
    static ENG: beak_engine::Engine = {
        let mut e = beak_engine::Engine::new();
        e.set_theme(beak_engine::Theme {
            bg: beak_engine::Rgb(255, 255, 255), text: beak_engine::Rgb(0, 0, 0),
            heading: beak_engine::Rgb(0, 0, 0), link: beak_engine::Rgb(0, 0, 238),
            muted: beak_engine::Rgb(96, 96, 96), rule: beak_engine::Rgb(128, 128, 128) });
        e
    };
}

/// Relayout on demand, host-side; the same path as `host_relayout` in the
/// host. The test path must be the real path, or the `fresh` line differs.
fn host_relayout(ip: &mut beak_engine::js::interp::Interp) {
    let html = include_str!("../../beak/src/selftest.html");
    ENG.with(|eng| {
        if let Some(d) = ip.doc.as_mut() { eng.set_scripted_dom(Some(d.to_dom())); }
        let forms = eng.last_forms();
        let lay = eng.layout_forms(html, "", 1024, &forms);
        ip.set_geometry(beak_engine::js::interp::Geometry {
            boxes: std::rc::Rc::new(lay.element_rects()),
            scroll: (0, 0),
            content: (1024, lay.height as i32),
        });
    });
}

fn main() {
    beak_engine::js::random::set_source(host_random);
    use beak_engine::js::dombind::{Doc, ScriptRef, page_scripts};

    let html = include_str!("../../beak/src/selftest.html");
    let dom = beak_engine::dom::parse(html);
    let doc = Doc::from_dom(&dom);
    // With the node: it is `document.currentScript` and the insertion point
    // of `document.write`, as in beak.
    let scripts: Vec<(String, u32)> = page_scripts(&doc)
        .into_iter()
        .filter_map(|r| match r { ScriptRef::Inline(t, _, n) => Some((t, n)), _ => None })
        .collect();
    println!("{} eingebettete Skripte, {} B HTML", scripts.len(), html.len());

    let mut sess = beak_engine::js::Session::new(50_000_000);
    sess.interp.set_document(doc);
    // The hook as the host sets it. The test page inserts an element and
    // measures it in the same step; without the hook it reports 0.
    ENG.with(|eng| eng.set_hit_all(true));
    // `NORELAYOUT=1` removes the hook, to check that the `fresh` line can
    // still fail (it then reports "0/0 statt 240").
    if std::env::var("NORELAYOUT").is_err() { sess.interp.relayout = Some(host_relayout); }
    sess.interp.set_media(1024.0, 768.0, false);
    // Same URL as beak (`selftest::URL`), so host and beak runs are
    // comparable.
    sess.interp.set_location("beak:selftest");
    // And the clock, set as `beak/src/lib.rs` sets it from `npk_unix_time()`.
    // Without it `Date.now()` would sit at 1970 host-side and the line would
    // be permanently red; set on both sides it checks that the host clock
    // reaches the engine.
    sess.interp.epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    // The cascade context exactly as beak submits it (`beak/src/lib.rs`).
    // Without it `getComputedStyle` takes the inline-style-only fallback
    // instead of the real path.
    let theme = beak_engine::Theme {
        bg: beak_engine::Rgb(255, 255, 255), text: beak_engine::Rgb(0, 0, 0),
        heading: beak_engine::Rgb(0, 0, 0), link: beak_engine::Rgb(0, 0, 238),
        muted: beak_engine::Rgb(96, 96, 96), rule: beak_engine::Rgb(128, 128, 128) };
    let media = beak_engine::css::Media::new(1024.0, false);
    let sheet = beak_engine::css::collect_all(&dom, "", media);
    sess.interp.set_style_context(beak_engine::js::interp::StyleCtx {
        sheet: std::rc::Rc::new(sheet),
        theme,
        viewport_w: 1024.0,
    });
    for (n, (src, node)) in scripts.iter().enumerate() {
        let prog = match beak_engine::js::parse(src, false) {
            Ok(p) => p,
            Err(e) => { println!("Skript {n}: PARSE-FEHLER {e:?}"); continue }
        };
        sess.interp.current_script = Some(*node);
        let r = sess.run(&prog);
        sess.interp.current_script = None;
        if let Err(e) = r {
            println!("Skript {n}: LAUFFEHLER {e:?}");
        }
    }
    sess.interp.run_timers();

    for line in sess.interp.take_console() {
        println!("{line}");
    }
    let d = sess.interp.doc.as_ref().unwrap();
    println!("Behandler angemeldet: {}", d.has_listeners);

    // The clicks, routed through the layout and not the tree: beak takes the
    // dispatch chain from `lay.element_chain(x, y)`, and a chain built from
    // `ancestors()` would skip exactly that step.
    println!("\n── Klicks (Kette aus dem LAYOUT, wie in beak) ──");
    let lay = ENG.with(|engine| {
        engine.set_hit_all(sess.interp.doc.as_ref().unwrap().has_listeners);
        engine.set_scripted_dom(Some(sess.interp.doc.as_mut().unwrap().to_dom()));
        engine.layout_forms(html, "", 1024, &Default::default())
    });
    // Submit the boxes exactly as beak does each frame. Without it
    // `getBoundingClientRect` returns zeros and the `geom` line would differ
    // from beak.
    sess.interp.set_geometry(beak_engine::js::interp::Geometry {
        boxes: std::rc::Rc::new(lay.element_rects()),
        scroll: (0, 0),
        content: (1024, lay.height as i32),
    });
    for (id, times) in [("b1", 2usize), ("b2", 1), ("b3", 1), ("b4", 1)] {
        let Some(n) = find_id(sess.interp.doc.as_ref().unwrap(), id) else {
            println!("{id}: nicht gefunden"); continue;
        };
        let seq = sess.interp.doc.as_ref().unwrap().nodes[n as usize].seq;
        let Some(c) = lay.controls.iter().find(|c| c.seq == seq) else {
            println!("{id}: KEIN Kasten im Layout — der Klick koennte ihn nie treffen");
            continue;
        };
        let (cx, cy) = (c.x + c.w / 2, c.y + c.h / 2);
        let chain = lay.element_chain(cx, cy);
        let doc = sess.interp.doc.as_ref().unwrap();
        let nodes: Vec<u32> = chain.iter().filter_map(|s| doc.by_seq(*s)).collect();
        if !chain.contains(&seq) {
            println!("{id}: der Klickpunkt ({cx},{cy}) findet das Element NICHT — Kette {chain:?}");
        }
        for _ in 0..times {
            // With the position, as the host sends it (`dispatch_at`).
            // Without coordinates `e.clientX` would be `undefined` here and
            // the `mausev` line would differ from beak.
            match beak_engine::js::dombind::dispatch_at(&mut sess.interp, "click", &nodes,
                Some((cx as f64, cy as f64, cx as f64, cy as f64))) {
                Ok(p) => { let _ = p; }
                Err(_) => println!("{id}: LAUFFEHLER"),
            }
        }
    }
    // Several rounds: the promise chain ends in a `setTimeout` that only
    // becomes due after the chain has run.
    for _ in 0..8 { if sess.interp.run_timers() == 0 { break } }
    for line in sess.interp.take_console() { println!("{line}"); }
    let d = sess.interp.doc.as_ref().unwrap();
    for id in ["count", "inline", "bubble", "timer", "micro"] {
        match find_id(d, id) {
            Some(n) => println!("#{id}: {:?}", d.text_of(n)),
            None => println!("#{id}: nicht gefunden"),
        }
    }
    let li = d.nodes.iter().filter(|n| &*n.tag == "li").count();
    println!("<li> im Baum: {li}   (Baum geaendert: {})", d.dirty);
}

fn find_id(d: &Doc, id: &str) -> Option<u32> {
    let mut all = Vec::new();
    d.descendants(d.doc, &mut all);
    all.into_iter().find(|&x| d.nodes[x as usize].attr("id").map(|v| v.to_string()).as_deref() == Some(id))
}
