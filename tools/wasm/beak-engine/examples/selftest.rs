// Die eingebaute Pruefseite host-seitig durchspielen — dieselbe Datei, die
// beak ausliefert. Was hier NEIN sagt, sagt auf dem Geraet auch NEIN; der
// Unterschied waere ein Befund fuer sich.
use beak_engine::js::dombind::Doc;

fn main() {
    use beak_engine::js::dombind::{Doc, ScriptRef, page_scripts};

    let html = include_str!("../../beak/src/selftest.html");
    let dom = beak_engine::dom::parse(html);
    let doc = Doc::from_dom(&dom);
    let scripts: Vec<String> = page_scripts(&doc)
        .into_iter()
        .filter_map(|r| match r { ScriptRef::Inline(t) => Some(t), _ => None })
        .collect();
    println!("{} eingebettete Skripte, {} B HTML", scripts.len(), html.len());

    let mut sess = beak_engine::js::Session::new(50_000_000);
    sess.interp.set_document(doc);
    sess.interp.set_media(1024.0, 768.0, false);
    // Dieselbe Adresse wie am Geraet (`selftest::URL`). Ohne sie stuende hier
    // `about:blank` und dort `beak:selftest` — und die eine Sache, die diese
    // Seite kann, ist Wirt und Geraet VERGLEICHBAR zu machen.
    sess.interp.set_location("beak:selftest");
    // Und die Uhr. Am Geraet setzt `beak/src/lib.rs` sie aus
    // `npk_unix_time()`; ohne dieselbe Zeile hier stuende `Date.now()`
    // host-seitig bei 1970, und die Pruefzeile waere auf dem Rechner
    // DAUERHAFT rot — ein Warnlicht, das immer leuchtet, liest niemand mehr.
    // Beidseitig gesetzt prueft sie, was sie soll: kommt die Uhr des Wirts
    // in der Engine an?
    sess.interp.epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    // Der Kaskadenkontext — GENAU wie beak ihn einreicht (`beak/src/lib.rs`).
    //
    // Er fehlte hier, und dadurch lief `getComputedStyle` host-seitig auf dem
    // Ausweichpfad (nur Inline-Stil) statt auf dem echten. Die Zeile
    // `CSSStyleDeclaration benannt` war deshalb host GRUEN und am Geraet ROT:
    // ein Testpfad, der nicht der echte ist, ist kein Test
    // ([[feedback_the_test_path_must_be_the_real_path]]) — und diesmal hat er
    // die Luecke nicht nur verpasst, er hat sie ZUGEDECKT.
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
    for (n, src) in scripts.iter().enumerate() {
        let prog = match beak_engine::js::parse(src, false) {
            Ok(p) => p,
            Err(e) => { println!("Skript {n}: PARSE-FEHLER {e:?}"); continue }
        };
        if let Err(e) = sess.run(&prog) {
            println!("Skript {n}: LAUFFEHLER {e:?}");
        }
    }
    sess.interp.run_timers();

    for line in sess.interp.take_console() {
        println!("{line}");
    }
    let d = sess.interp.doc.as_ref().unwrap();
    println!("Behandler angemeldet: {}", d.has_listeners);

    // Die Klicks, die auf dem Geraet der Finger macht — und ZWAR UEBER DAS
    // LAYOUT, nicht ueber den Baum.
    //
    // Die erste Fassung baute die Kette mit `ancestors()` aus dem Baum. Damit
    // pruefte sie alles ausser dem einen Schritt, an dem es am Geraet
    // scheiterte: beak nimmt die Kette aus `lay.element_chain(x, y)`, und ein
    // `<button>` stand dort nicht drin. Host gruen, Geraet stumm — ein
    // Testpfad, der nicht der echte ist, ist kein Test
    // ([[feedback_verify_the_call_path]]).
    println!("\n── Klicks (Kette aus dem LAYOUT, wie in beak) ──");
    let mut engine = beak_engine::Engine::new();
    engine.set_theme(beak_engine::Theme {
        bg: beak_engine::Rgb(255, 255, 255), text: beak_engine::Rgb(0, 0, 0),
        heading: beak_engine::Rgb(0, 0, 0), link: beak_engine::Rgb(0, 0, 238),
        muted: beak_engine::Rgb(96, 96, 96), rule: beak_engine::Rgb(128, 128, 128) });
    engine.set_hit_all(sess.interp.doc.as_ref().unwrap().has_listeners);
    engine.set_scripted_dom(Some(sess.interp.doc.as_mut().unwrap().to_dom()));
    let lay = engine.layout_forms(html, "", 1024, &Default::default());
    // Die Kaesten einreichen — GENAU wie beak es je Bild tut. Ohne das
    // antwortet `getBoundingClientRect` hier mit Nullen, und die Zeile `geom`
    // waere host-seitig rot und am Geraet gruen: derselbe Fehler wie bei der
    // Klickkette und beim Kaskadenkontext, dritte Auspraegung
    // ([[feedback_the_test_path_must_be_the_real_path]]).
    sess.interp.set_geometry(beak_engine::js::interp::Geometry {
        boxes: std::rc::Rc::new(lay.element_rects()),
        scroll: (0, 0),
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
            match beak_engine::js::dombind::dispatch(&mut sess.interp, "click", &nodes) {
                Ok(p) => { let _ = p; }
                Err(_) => println!("{id}: LAUFFEHLER"),
            }
        }
    }
    // Mehrere Runden: die Promise-Kette endet in einem `setTimeout`, das
    // erst faellig wird, nachdem die Kette durch ist.
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
