//! Die KAESTEN einer Datei nennen — Etikett, Ort, Groesse.
//!
//! `pagedump` zeigt, was gemalt wird; ein Kasten ohne Farbe malt nichts und
//! ist trotzdem der, um den es geht (Raender, `min-height`, Zusammenfall).
//! Das ist die Seite, die man gegen `getBoundingClientRect` eines echten
//! Browsers halten kann.
//!
//!   cargo run --release --example boxprobe <datei.html>
//!   W=800 BOX=div.p cargo run --release --example boxprobe <datei.html>
fn main() {
    let p = std::env::args().nth(1).expect("datei");
    let html = std::fs::read_to_string(&p).expect("lesen");
    let w: u32 = std::env::var("W").ok().and_then(|v| v.parse().ok()).unwrap_or(800);
    let want = std::env::var("BOX").ok();
    let mut eng = beak_engine::Engine::new();
    eng.set_theme(beak_engine::Theme {
        bg: beak_engine::Rgb(255, 255, 255), text: beak_engine::Rgb(0, 0, 0),
        heading: beak_engine::Rgb(0, 0, 0), link: beak_engine::Rgb(0, 0, 238),
        muted: beak_engine::Rgb(96, 96, 96), rule: beak_engine::Rgb(128, 128, 128),
    });
    eng.set_inspect(true);
    let lay = eng.layout_ext(&html, "", w);
    for b in lay.inspect.iter() {
        if want.as_deref().is_some_and(|s| !b.label.starts_with(s)) {
            continue;
        }
        println!("{:5},{:<5} {:4}x{:<4} d{:<2} {}", b.x, b.y, b.w, b.h, b.depth, b.label);
    }
}
