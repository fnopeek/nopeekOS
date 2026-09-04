// Was malt beak für ein Schnipsel — Befehl für Befehl?
fn main() {
    let css = include_str!("../assets/bootstrap.min.css");
    let body = std::env::var("BODY").unwrap_or_else(|_|
        "<button class=\"btn btn-primary\">Primary</button>".into());
    let doc = format!("<!DOCTYPE html><html><body>{body}</body></html>");
    let mut eng = beak_engine::Engine::new();
    let lay = eng.layout_ext(&doc, css, 800);
    use beak_engine::layout::DrawOp;
    println!("\n   {body}\n");
    for o in lay.ops.iter() {
        match o {
            DrawOp::Rect { x, y, w, h, color } =>
                println!("   Rect      {x:>5},{y:<5} {w:>4}x{h:<4} rgba({},{},{},{})",
                         color.c.0, color.c.1, color.c.2, color.a),
            DrawOp::RoundRect { x, y, w, h, r, color, ring } =>
                println!("   RoundRect {x:>5},{y:<5} {w:>4}x{h:<4} rgba({},{},{},{}) r={:?} ring={ring}",
                         color.c.0, color.c.1, color.c.2, color.a, r),
            DrawOp::Text { x, y, text, color, size, .. } =>
                println!("   Text      {x:>5},{y:<5} {size:>4.0}px rgba({},{},{},{}) {:?}",
                         color.c.0, color.c.1, color.c.2, color.a, text),
            DrawOp::Shadow { x, y, w, h, blur, color } =>
                println!("   Shadow    {x:>5},{y:<5} {w:>4}x{h:<4} rgba({},{},{},{}) blur={blur}",
                         color.c.0, color.c.1, color.c.2, color.a),
            other => println!("   {:?}", core::mem::discriminant(other)),
        }
    }
}
