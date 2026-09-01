// Der Beweis fuer die Zustellung: ein Klick loest einen Behandler aus, der
// den Baum aendert — und das Layout malt die Aenderung.
fn main() {
    use beak_engine::{Engine, Rgb, Theme};
    use beak_engine::js::dombind;
    let theme = Theme { bg: Rgb(255,255,255), text: Rgb(0,0,0), heading: Rgb(0,0,0),
                        link: Rgb(0,0,238), muted: Rgb(96,96,96), rule: Rgb(128,128,128) };
    let html = r##"<html><head><style>
        .panel { display: none }
        .panel.open { display: block; width: 200px; height: 30px; background: #0a0 }
        #btn { width: 80px; height: 24px; background: #ccc }
      </style></head><body>
      <div id="btn">Menue</div><div id="p" class="panel">Inhalt</div></body></html>"##;

    let mut engine = Engine::new();
    engine.set_theme(theme);

    let dom = beak_engine::dom::parse(html);
    let mut sess = beak_engine::js::Session::new(u64::MAX);
    sess.interp.set_document(dombind::Doc::from_dom(&dom));
    let script = r##"
        var n = 0;
        document.getElementById("btn").addEventListener("click", function(e){
            n++;
            document.getElementById("p").classList.toggle("open");
            e.preventDefault();
        });
        document.body.addEventListener("click", function(){ window.__bubbled = true });
    "##;
    sess.run(&beak_engine::js::parse(script, false).unwrap()).expect("Skript");
    engine.set_hit_all(sess.interp.doc.as_ref().unwrap().has_listeners);
    engine.set_scripted_dom(Some(sess.interp.doc.as_mut().unwrap().to_dom()));

    let count = |e: &Engine| {
        use beak_engine::layout::DrawOp;
        let l = e.layout_forms(html, "", 400, &Default::default());
        (l.ops.iter().filter(|o| matches!(o, DrawOp::Rect{..}|DrawOp::RoundRect{..})).count(), l)
    };
    let (r0, lay) = count(&engine);

    // Wo liegt der Knopf? Aus dem Layout, nicht geraten.
    let btn_seq = sess.interp.doc.as_ref().unwrap()
        .by_seq(0).map(|_| 0); // Platzhalter, gleich richtig
    let _ = btn_seq;
    let (bx, by) = lay.ops.iter().find_map(|o| match o {
        beak_engine::layout::DrawOp::Rect { x, y, w, h, .. } if *w == 80 && *h == 24 => Some((*x+5, *y+5)),
        _ => None }).unwrap_or((5, 5));
    let chain = lay.hover_at(bx, by);
    println!("Treffer bei ({bx},{by}) -> seq-Kette {chain:?}");

    // seq -> Arena-Knoten
    let nodes: Vec<u32> = chain.iter()
        .filter_map(|s| sess.interp.doc.as_ref().unwrap().by_seq(*s)).collect();
    let prevented = matches!(dombind::dispatch(&mut sess.interp, "click", &nodes), Ok(true));
    let bubbled = sess.run(&beak_engine::js::parse(
        "if (!window.__bubbled) throw new Error('nicht geblasen')", false).unwrap()).is_ok();

    let changed = sess.interp.doc.as_ref().unwrap().dirty;
    if changed {
        engine.set_scripted_dom(Some(sess.interp.doc.as_mut().unwrap().to_dom()));
    }
    let (r1, _) = count(&engine);
    println!("Rechtecke {r0} -> {r1} | preventDefault: {prevented} | geblasen: {bubbled} | Baum geaendert: {changed}");
    println!("\n{}", if r1 > r0 && prevented && bubbled && changed {
        "JA — der Klick hat den Behandler ausgeloest und die Seite veraendert." }
        else { "NEIN." });
}
