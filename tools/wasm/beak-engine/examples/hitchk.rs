//! Findet der KLICKPUNKT den Knopf? — der Schritt, den `selftest.rs` auslaesst.
//!
//! `selftest.rs` baut die Zustellkette aus dem BAUM (`ancestors`). beak baut
//! sie aus dem LAYOUT (`lay.element_chain(x, y)`). Damit prueft der
//! host-seitige Lauf alles ausser genau dem Schritt, an dem ein Geraetelauf
//! scheitern kann — und ein Geraetelauf hat es getan: vier `control-activate`,
//! keine einzige Behandler-Zeile.
//!
//! Hier wird die Kette so gebaut wie in beak: aus den gemalten Kaesten.
fn main() {
    use beak_engine::{Engine, Rgb, Theme};
    use beak_engine::js::dombind::Doc;
    let theme = Theme { bg: Rgb(255,255,255), text: Rgb(0,0,0), heading: Rgb(0,0,0),
                        link: Rgb(0,0,238), muted: Rgb(96,96,96), rule: Rgb(128,128,128) };
    let html = include_str!("../../beak/src/selftest.html");

    let mut engine = Engine::new();
    engine.set_theme(theme);
    let dom = beak_engine::dom::parse(html);
    let mut sess = beak_engine::js::Session::new(50_000_000);
    sess.interp.set_document(Doc::from_dom(&dom));
    sess.interp.set_media(1902.0, 800.0, false);

    use beak_engine::js::dombind::{ScriptRef, page_scripts};
    for r in page_scripts(sess.interp.doc.as_ref().unwrap()) {
        if let ScriptRef::Inline(t, _) = r {
            if let Ok(p) = beak_engine::js::parse(&t, false) { let _ = sess.run(&p); }
        }
    }
    sess.interp.run_timers();
    let doc = sess.interp.doc.as_ref().unwrap();
    println!("Behandler angemeldet: {}", doc.has_listeners);
    engine.set_hit_all(doc.has_listeners);
    engine.set_scripted_dom(Some(sess.interp.doc.as_mut().unwrap().to_dom()));

    let lay = engine.layout_forms(html, "", 1902, &Default::default());
    println!("hover_boxes: {}, controls: {}", lay.element_chain(-1, -1).len(), lay.controls.len());

    // Jeder Knopf ueber SEINEN Steuerkasten — dieselbe Quelle, aus der beaks
    // `hit_control` den Klick nimmt. Trifft `element_chain` dort nichts, dann
    // kommt der Klick nie bei der Seite an.
    let mut all_ok = true;
    for c in &lay.controls {
        let (cx, cy) = (c.x + c.w / 2, c.y + c.h / 2);
        let chain = lay.element_chain(cx, cy);
        let doc = sess.interp.doc.as_ref().unwrap();
        let nodes: Vec<u32> = chain.iter().filter_map(|s| doc.by_seq(*s)).collect();
        let id = doc.by_seq(c.seq)
            .and_then(|n| doc.nodes[n as usize].attr("id").map(|v| v.to_string()))
            .unwrap_or_else(|| "?".into());
        let hit = chain.contains(&c.seq);
        if !hit { all_ok = false; }
        println!("  Steuerkasten seq={} (#{id}) bei ({cx},{cy})  ->  Kette {chain:?}, Knoten {}, \
                  eigenes seq drin: {}",
                 c.seq, nodes.len(), if hit { "JA" } else { "NEIN" });
    }
    println!("\n{}", if all_ok && !lay.controls.is_empty() {
        "JA — jeder Klickpunkt findet sein Element, die Zustellung kann greifen." }
        else { "NEIN — der Klickpunkt findet das Element NICHT. Genau hier stirbt der Klick." });
}
