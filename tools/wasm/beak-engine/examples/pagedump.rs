//! Eine GANZE HTML-Datei so rendern, wie der WPT-Laeufer es tut, und die
//! Zeichenbefehle nennen. `opdump` nimmt nur ein Schnipsel — fuer einen
//! Reftest braucht es die Datei samt ihrem eigenen `<style>`.
fn main() {
    let p = std::env::args().nth(1).expect("datei");
    let html = std::fs::read_to_string(&p).expect("lesen");
    let w: u32 = std::env::var("W").ok().and_then(|v| v.parse().ok()).unwrap_or(800);
    let mut eng = beak_engine::Engine::new();
    eng.set_theme(beak_engine::Theme {
        bg: beak_engine::Rgb(255, 255, 255), text: beak_engine::Rgb(0, 0, 0),
        heading: beak_engine::Rgb(0, 0, 0), link: beak_engine::Rgb(0, 0, 238),
        muted: beak_engine::Rgb(96, 96, 96), rule: beak_engine::Rgb(128, 128, 128),
    });
    // `CSSFILE=` fuer eine Seite, deren Blatt nicht im `<style>` steht.
    let css = std::env::var("CSSFILE").ok()
        .map(|f| std::fs::read_to_string(f).expect("css")).unwrap_or_default();
    let lay = eng.layout_ext(&html, &css, w);
    use beak_engine::layout::DrawOp;
    for o in lay.ops.iter() {
        match o {
            DrawOp::Rect { x, y, w, h, color } =>
                println!("Rect      {x:5},{y:<5} {w:4}x{h:<4} {color:?}"),
            DrawOp::RoundRect { x, y, w, h, color, .. } =>
                println!("RoundRect {x:5},{y:<5} {w:4}x{h:<4} {color:?}"),
            DrawOp::Text { x, y, size, text, .. } =>
                println!("Text      {x:5},{y:<5} {size}px {text:?}"),
            DrawOp::Gradient { x, y, w, h, g, .. } =>
                println!("Gradient  {x:5},{y:<5} {w:4}x{h:<4} {:?} {}deg rep={} {:?}",
                         g.kind, g.angle, g.repeating, g.stops()),
            _ => {}
        }
    }
}
