extern crate alloc;
// What a layout costs on a real page.
//
//   PAGE=<path without .html> W=1902 N=5 cargo run --release --example laytime
fn main() {
    let base = std::env::var("PAGE").expect("PAGE=<pfad ohne .html>");
    let html = std::fs::read_to_string(format!("{base}.html")).expect("html");
    let css = std::fs::read_to_string(format!("{base}.css")).unwrap_or_default();
    let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
    let n: u32 = std::env::var("N").ok().and_then(|w| w.parse().ok()).unwrap_or(5);
    println!("   {} — {} KB HTML, {} KB CSS, @{width}px",
             base.rsplit('/').next().unwrap_or(&base), html.len() / 1024, css.len() / 1024);
    let mut best = f64::MAX;
    for _ in 0..n {
        let t = std::time::Instant::now();
        let mut eng = beak_engine::Engine::new();
        let lay = eng.layout_ext(&html, &css, width);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        best = best.min(ms);
        std::hint::black_box(lay.height);
    }
    println!("   bestes von {n}: {best:.1} ms");

    // And the cost when only one form control changed, which is what every
    // keystroke in a field pays.
    let mut state = beak_engine::forms::FormState::default();
    let eng = beak_engine::Engine::new();
    let mut lay = eng.layout_forms(&html, &css, width, &state);
    let Some(c) = lay.controls.iter().find(|c| c.kind.is_text()).map(|c| c.seq) else {
        println!("   (kein Textfeld auf der Seite — kein Vergleich)");
        return;
    };
    state.focus = Some(c);
    let mut fast = f64::MAX;
    for i in 0..n {
        state.set_value(c, alloc::format!("{}", "abcdefghij".get(..(i as usize % 10) + 1).unwrap()));
        state.caret = state.value_or(c, "").len();
        let t = std::time::Instant::now();
        let ok = eng.repaint_controls(&mut lay, &state);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        assert!(ok, "Schnellweg gab auf: {}", eng.repaint_bail());
        fast = fast.min(ms);
    }
    println!("   ein Tastendruck: {fast:.3} ms  ({:.0}x billiger)", best / fast.max(1e-6));
}
