// Was kostet `getComputedStyle` — und was kostet es, wenn das Skript vorher
// den Baum angefasst hat?
//
// Seit 0.74.0 rechnet die Kaskade auf dem LEBENDEN Baum. Der wird aus der
// JS-Arena gebaut und zwischengespeichert; jede Aenderung macht den Speicher
// ungueltig. Diese Probe sagt, was beides wirklich kostet — „sollte schnell
// genug sein" ist keine Zahl ([[feedback_remeasure_before_claiming_a_delta]]).
//
//   PAGE=<pfad ohne .html> N=200 cargo run --release --example gcstime
fn main() {
    let base = std::env::var("PAGE").expect("PAGE=<pfad ohne .html>");
    let html = std::fs::read_to_string(format!("{base}.html")).expect("html");
    let css = std::fs::read_to_string(format!("{base}.css")).unwrap_or_default();
    let n: u32 = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let width = 1902u32;

    let dom = beak_engine::dom::parse(&html);
    let doc = beak_engine::js::dombind::Doc::from_dom(&dom);
    let nodes = doc.nodes.len();
    let media = beak_engine::css::Media::new(width as f32, false);
    let sheet = beak_engine::css::collect_all(&dom, &css, media);
    let theme = beak_engine::Theme {
        bg: beak_engine::Rgb(255, 255, 255), text: beak_engine::Rgb(0, 0, 0),
        heading: beak_engine::Rgb(0, 0, 0), link: beak_engine::Rgb(0, 0, 238),
        muted: beak_engine::Rgb(96, 96, 96), rule: beak_engine::Rgb(128, 128, 128) };

    let mut sess = beak_engine::js::Session::new(500_000_000);
    sess.interp.set_document(doc);
    sess.interp.set_style_context(beak_engine::js::interp::StyleCtx {
        sheet: std::rc::Rc::new(sheet), theme, viewport_w: width as f32,
    });

    println!("   {} — {} Knoten in der Arena, {} KB CSS",
             base.rsplit('/').next().unwrap_or(&base), nodes, css.len() / 1024);

    // Die Schleifenzahl wird EINGESETZT, nicht ersetzt. `replace("N", …)` traf
    // auch das N in `tagName`; die Probe mass danach etwas anderes, als sie
    // behauptete, und das sah aus wie ein Fehler in der Engine.
    let mut run = |sess: &mut beak_engine::js::Session, body: &str| -> f64 {
        let src = format!("var e = document.documentElement;\nfor (var i = 0; i < {n}; i++) {{ {body} }}");
        let prog = beak_engine::js::parse(&src, false).expect("parst");
        let t = std::time::Instant::now();
        sess.run(&prog).expect("laeuft");
        t.elapsed().as_secs_f64() * 1e6 / n as f64
    };

    // Ohne Aenderung dazwischen: der Baum steht, der Zwischenspeicher greift,
    // gemessen wird die Kaskade auf einer Vorfahrenkette.
    let warm = run(&mut sess, "getComputedStyle(e).color;");
    // Mit einer Aenderung davor: jede erzwingt einen Neubau des Baums.
    let cold = run(&mut sess, "e.setAttribute('class', 'x' + i); getComputedStyle(e).color;");

    println!("   Abfrage, Baum unveraendert  : {warm:8.1} µs");
    println!("   Abfrage nach einer Aenderung: {cold:8.1} µs");

    // Der Neubau allein, damit die Zahl darueber nachpruefbar ist statt nur
    // plausibel.
    let d = sess.interp.doc.as_ref().expect("doc");
    let t = std::time::Instant::now();
    for _ in 0..20 { std::hint::black_box(d.live_dom()); }
    println!("   davon der Neubau            : {:8.1} µs", t.elapsed().as_secs_f64() * 1e6 / 20.0);
    let _ = sess.interp.take_console();
}
