//! Runs the image decoder host-side and reports why an image was dropped.
//! `image dropped: undecodable or over budget` names two failures in one
//! sentence; this says which one.
//!
//!   cargo run --release --example imgcheck -- image.jpg [...]
//!   cargo run --release --example imgcheck -- page.html   # list only
//!
//! An `.html` is not decoded but enumerated with `image_srcs`, i.e. the same
//! list beak itself fetches.

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
