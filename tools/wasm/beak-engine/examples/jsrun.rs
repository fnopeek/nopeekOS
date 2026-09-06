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
    // `MODULE=1` parst als Modul. beak versucht am Geraet BEIDES — die Datei
    // sagt nicht, was sie ist —, also muss die Probe das auch koennen.
    let module = std::env::var("MODULE").is_ok();
    let prog = match beak_engine::js::parse(&script, module) {
        Ok(p) => p,
        Err(e) => {
            let at = e.at.min(script.len());
            let head = script[..at].rsplit('\n').next().unwrap_or("");
            let tail = &script[at..(at + 60).min(script.len())];
            println!("SyntaxError: {} @{} ...{}<<HIER>>{}...",
                     e.msg, e.at,
                     &head[head.len().saturating_sub(60)..],
                     tail.split('\n').next().unwrap_or(""));
            return;
        }
    };
    let mut i = beak_engine::js::interp::Interp::new();
    // `NOVM=1` faehrt dieselbe Datei ohne die Befehlsmaschine. Der Diff der
    // beiden Ausgaben ist die einzige Art, zu pruefen, dass die zwei
    // Maschinen dieselbe Bedeutung haben — und nicht nur dieselbe Zahl.
    if std::env::var("NOVM").is_ok() { i.vm_off = true; }
    // `HTML=<datei|text>` haengt ein Dokument an. Ohne das gibt es `document`
    // GAR NICHT — das ist Absicht der Engine und keine Luecke des Werkzeugs.
    if let Ok(h) = std::env::var("HTML") {
        let html = std::fs::read_to_string(&h).unwrap_or(h);
        let dom = beak_engine::dom::parse(&html);
        i.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
        // Denselben Kaskadenkontext einreichen, den beak einreicht — sonst
        // antwortet `getComputedStyle` hier anders als am Geraet, und die
        // Probe prueft eine Maschine, die es so nicht gibt.
        let media = beak_engine::css::Media::new(1024.0, std::env::var("DARK").is_ok());
        let ext = std::env::var("CSS").ok().and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let sheet = beak_engine::css::collect_all(&dom, &ext, media);
        i.set_style_context(beak_engine::js::interp::StyleCtx {
            sheet: std::rc::Rc::new(sheet),
            theme: beak_engine::layout::Theme {
                bg: beak_engine::layout::Rgb(255, 255, 255),
                text: beak_engine::layout::Rgb(33, 37, 41),
                heading: beak_engine::layout::Rgb(33, 37, 41),
                link: beak_engine::layout::Rgb(13, 110, 253),
                muted: beak_engine::layout::Rgb(108, 117, 125),
                rule: beak_engine::layout::Rgb(222, 226, 230),
            },
            viewport_w: 1024.0,
        });
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
