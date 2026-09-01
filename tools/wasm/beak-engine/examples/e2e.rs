// Der Beweis: ein Skript veraendert den Baum, und das BILD aendert sich.
//
// Ohne diesen Lauf ist die DOM-Bindung nur eine Behauptung — die Arena und
// beaks Baum waren bis eben zwei getrennte Welten.
fn main() {
    use beak_engine::{Engine, Rgb, Theme};
    let theme = Theme { bg: Rgb(255,255,255), text: Rgb(0,0,0), heading: Rgb(0,0,0),
                        link: Rgb(0,0,238), muted: Rgb(96,96,96), rule: Rgb(128,128,128) };
    let html = r##"<html><head><style>
        .hidden { display: none }
        .box { width: 100px; height: 40px; background: #c00 }
      </style></head><body>
      <div id="a" class="box hidden"></div><p id="t">alt</p></body></html>"##;

    let mut engine = Engine::new();
    engine.set_theme(theme);

    // Nicht am Debug-Text gemessen, sondern an den Zeichenbefehlen selbst:
    // wie viele Rechtecke gemalt werden, und welcher Text im Bild steht.
    let count = |e: &Engine| {
        use beak_engine::layout::DrawOp;
        let l = e.layout_forms(html, "", 400, &Default::default());
        let rects = l.ops.iter().filter(|o| matches!(o, DrawOp::Rect{..} | DrawOp::RoundRect{..})).count();
        let texts: String = l.ops.iter().filter_map(|o| match o {
            DrawOp::Text { text, .. } => Some(text.clone()), _ => None }).collect();
        (rects, texts)
    };

    let before = count(&engine);
    println!("vorher:  {} Rechtecke, Text: {:?}", before.0, before.1);

    // Skript laufen lassen — auf DEMSELBEN Dokument.
    let dom = beak_engine::dom::parse(html);
    let mut sess = beak_engine::js::Session::new(u64::MAX);
    sess.interp.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
    let script = r##"
        document.getElementById("a").classList.remove("hidden");
        document.getElementById("t").textContent = "neu";
        var extra = document.createElement("div");
        extra.className = "box";
        document.body.appendChild(extra);
    "##;
    let prog = beak_engine::js::parse(script, false).unwrap();
    sess.run(&prog).unwrap();
    engine.set_scripted_dom(Some(sess.interp.doc.as_mut().unwrap().to_dom()));

    let after = count(&engine);
    println!("nachher: {} Rechtecke, Text: {:?}", after.0, after.1);

    let ok = after.0 > before.0 && before.1.contains("alt") && after.1.contains("neu")
             && !after.1.contains("alt");
    println!("\n{}", if ok { "JA — das Skript hat das Bild veraendert." }
                     else { "NEIN — das Bild ist gleich geblieben." });
}
