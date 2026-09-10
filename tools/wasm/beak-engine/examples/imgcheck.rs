//! Warum ist dieses Bild nicht angekommen? — der Dekoder mit GRUND.
//!
//! `[beak] image dropped: undecodable or over budget` nennt zwei Fehler in
//! einem Satz, und nur einer davon ist je der richtige
//! ([[feedback_a_denial_and_a_timeout_are_two_failures]]). Hier laeuft
//! derselbe Weg host-seitig, und die Absage sagt, WELCHER.
//!
//!   cargo run --release --example imgcheck -- bild.jpg [...]
//!   cargo run --release --example imgcheck -- seite.html   # nur die Liste
//!
//! Eine `.html` wird nicht dekodiert, sondern AUFGEZAEHLT — mit `image_srcs`,
//! also der Liste, die beak selbst holt, nicht einer zweiten aus einem grep.

fn kind(b: &[u8]) -> &'static str {
    if b.len() >= 8 && b[0..8] == *b"\x89PNG\r\n\x1a\n" { "PNG" }
    else if b.len() >= 3 && b[0..3] == [0xFF, 0xD8, 0xFF] { "JPEG" }
    else if beak_engine::ico::looks_like_ico(b) { "ICO" }
    else if beak_engine::svg::looks_like_svg(b) { "SVG" }
    else if beak_engine::webp::looks_like_webp(b) { "WebP" }
    else { "UNBEKANNT" }
}

fn main() {
    let vw: u32 = std::env::var("W").ok().and_then(|s| s.parse().ok()).unwrap_or(1902);
    let mut total = 0usize;
    for p in std::env::args().skip(1) {
        let bytes = match std::fs::read(&p) {
            Ok(b) => b,
            Err(e) => { println!("{p}: nicht lesbar ({e})"); continue }
        };
        if p.ends_with(".html") {
            let html = String::from_utf8_lossy(&bytes);
            for s in beak_engine::image_srcs(&html, vw) { println!("{s}"); }
            continue;
        }
        let k = kind(&bytes);
        match beak_engine::image::decode(&bytes) {
            Some(img) => {
                total += img.bgra.len();
                println!("{p}: {k} {}x{} = {} B BGRA", img.w, img.h, img.bgra.len());
            }
            None => println!("{p}: {k} ABGELEHNT ({} B)", bytes.len()),
        }
    }
    if total > 0 {
        println!("--- Summe: {total} B = {:.1} MB BGRA", total as f64 / 1048576.0);
    }
}
