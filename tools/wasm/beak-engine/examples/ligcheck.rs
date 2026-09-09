// Was macht beak aus einem Ligatur-Icon-Namen? `argv[1]` ist die Schrift.
fn main() {
    let path = std::env::args().nth(1).expect("schrift");
    let bytes = std::fs::read(&path).expect("lesbar");
    let fam = beak_engine::style::family_hash("micons");
    let mut fonts = beak_engine::fonts::Fonts::new();
    println!("geladen: {}", fonts.add_web(fam, 400, false, &bytes));
    if std::env::var("COST").is_ok() { cost(&bytes); }
    let face = fonts.pick(false, false, false, fam);
    println!("ligaturen: {}", face.ligatures().is_some());
    for t in ["home", "search", "abc", "zzqq", "home search"] {
        let sh = face.shape(t);
        let w: f32 = sh.iter().map(|(g, _, _)| face.metrics_indexed(*g, 24.0).advance_width).sum();
        let per_char: f32 = t.chars().map(|c| face.metrics(c, 24.0).advance_width).sum();
        println!("{t:<10} glyphen: {:<2} breite geformt: {w:>6.1}   ohne Formung: {per_char:>6.1}",
                 sh.len());
    }
}

// Was die Substitutionen kosten: dieselbe Schrift zweimal geparst.
fn cost(bytes: &[u8]) {
    use fontdue::{Font, FontSettings};
    for on in [false, true] {
        let t = std::time::Instant::now();
        let f = Font::from_bytes(bytes, FontSettings { load_substitutions: on, ..Default::default() }).unwrap();
        println!("load_substitutions={on:<5} {:>7.1} ms   glyphen im Font: {}",
                 t.elapsed().as_secs_f64() * 1000.0, f.glyph_count());
    }
}
