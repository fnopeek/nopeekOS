// What `getComputedStyle` costs, with and without a preceding tree mutation.
//
// The cascade runs on the live tree, which is built from the JS arena and
// cached; every mutation invalidates the cache.
//
//   PAGE=<path without .html> N=200 cargo run --release --example gcstime
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

    // The loop count is inserted, not substituted: `replace("N", …)` would
    // also hit the N in `tagName`.
    let mut run = |sess: &mut beak_engine::js::Session, body: &str| -> f64 {
        let src = format!("var e = document.documentElement;\nfor (var i = 0; i < {n}; i++) {{ {body} }}");
        let prog = beak_engine::js::parse(&src, false).expect("parst");
        let t = std::time::Instant::now();
        sess.run(&prog).expect("laeuft");
        t.elapsed().as_secs_f64() * 1e6 / n as f64
    };

    // No mutation in between: the tree stands, the cache hits; this measures
    // the cascade over an ancestor chain.
    let warm = run(&mut sess, "getComputedStyle(e).color;");
    // With a mutation before each call: every one forces a tree rebuild.
    let cold = run(&mut sess, "e.setAttribute('class', 'x' + i); getComputedStyle(e).color;");

    println!("   Abfrage, Baum unveraendert  : {warm:8.1} µs");
    println!("   Abfrage nach einer Aenderung: {cold:8.1} µs");

    // The rebuild alone, so the figure above can be checked.
    let d = sess.interp.doc.as_ref().expect("doc");
    let t = std::time::Instant::now();
    for _ in 0..20 { std::hint::black_box(d.live_dom()); }
    println!("   davon der Neubau            : {:8.1} µs", t.elapsed().as_secs_f64() * 1e6 / 20.0);
    let _ = sess.interp.take_console();
}
