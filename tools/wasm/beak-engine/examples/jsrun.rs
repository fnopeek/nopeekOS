//! Eine JS-Datei laufen lassen und sagen, was sie auf die Konsole geschrieben
//! hat.
//!
//! `js::run` sammelt die Konsole ein und wirft sie weg — fuer eine Probe ist
//! genau die Konsole aber das Ergebnis. `CAP=1` setzt die Schrittgrenze, damit
//! eine Endlosschleife als `RangeError` endet statt als haengender Lauf.
//!
//!   cargo run --release --example jsrun -- probe.js
fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let script = std::fs::read_to_string(&arg).unwrap_or(arg);
    let prog = match beak_engine::js::parse(&script, false) {
        Ok(p) => p,
        Err(e) => { println!("SyntaxError: {} @{}", e.msg, e.at); return; }
    };
    let mut i = beak_engine::js::interp::Interp::new();
    // `HTML=<datei|text>` haengt ein Dokument an. Ohne das gibt es `document`
    // GAR NICHT — das ist Absicht der Engine und keine Luecke des Werkzeugs.
    if let Ok(h) = std::env::var("HTML") {
        let html = std::fs::read_to_string(&h).unwrap_or(h);
        let dom = beak_engine::dom::parse(&html);
        i.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
        // Ein Fenster dazu, sonst gibt es `matchMedia` nicht. `DARK=1` dreht
        // das Farbschema.
        i.set_media(1024.0, 768.0, std::env::var("DARK").is_ok());
    }
    if std::env::var("CAP").is_ok() { i.max_steps = 2_000_000; }
    let r = i.run_program(&prog);
    // Zeitgeber UND Microtasks nachlaufen lassen — eine Probe, die auf
    // `setTimeout` endet, haette sonst kein Ergebnis.
    for _ in 0..64 { if i.run_timers() == 0 { break } }
    for l in &i.console { println!("{l}"); }
    if let Err(beak_engine::js::interp::Abrupt::Throw(v)) = r {
        let m = i.get(&v, "message").ok().and_then(|m| i.to_string(&m).ok());
        println!("UNCAUGHT: {}", m.as_deref().unwrap_or("?"));
    }
}
