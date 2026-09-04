// Eine eingefrorene Seite rendern und als BMP ablegen — zum ANSEHEN.
//
// Die Komponentenvorlage prueft, was ich prüfen wollte. Diese hier prüft, was
// ein Betreiber wirklich ausliefert — eingefroren, also zweimal gleich.
//
//   PAGE=<pfad ohne .html> W=1902 OUT=x.bmp cargo run --release --example pagerender
fn main() {
    let base = std::env::var("PAGE").expect("PAGE=<pfad ohne .html>");
    let html = std::fs::read_to_string(format!("{base}.html")).expect("html");
    let css = std::fs::read_to_string(format!("{base}.css")).unwrap_or_default();
    let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
    let out = std::env::var("OUT").unwrap_or_else(|_| "page.bmp".into());
    use beak_engine::layout::{Rgb, Theme};
    let mut eng = beak_engine::Engine::new();
    eng.set_theme(Theme { bg: Rgb(255, 255, 255), text: Rgb(33, 37, 41), heading: Rgb(33, 37, 41),
                          link: Rgb(13, 110, 253), muted: Rgb(108, 117, 125), rule: Rgb(222, 226, 230) });
    let lay = eng.layout_ext(&html, &css, width);
    let h = lay.height.clamp(1, 20000);
    let mut buf = vec![0u8; (width * h * 4) as usize];
    eng.paint(&lay, width, h, 0, &mut buf);
    std::fs::write(&out, to_bmp(&buf, width, h)).expect("write");
    eprintln!("{} -> {out}: {width}x{h}, {} ops, {} links",
              base.rsplit('/').next().unwrap_or(&base), lay.ops.len(), lay.links.len());
}

/// BGRA nach BMP. Von unten nach oben, wie das Format es will.
fn to_bmp(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = (w * 4) as usize;
    let pixels = row * h as usize;
    let mut b = Vec::with_capacity(122 + pixels);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&((122 + pixels) as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&122u32.to_le_bytes());
    b.extend_from_slice(&108u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(&3u32.to_le_bytes());
    b.extend_from_slice(&(pixels as u32).to_le_bytes());
    b.extend_from_slice(&[0; 16]);
    b.extend_from_slice(&0x00FF0000u32.to_le_bytes());
    b.extend_from_slice(&0x0000FF00u32.to_le_bytes());
    b.extend_from_slice(&0x000000FFu32.to_le_bytes());
    b.extend_from_slice(&0xFF000000u32.to_le_bytes());
    b.extend_from_slice(b"BGRs");
    b.extend_from_slice(&[0; 48]);
    for y in (0..h as usize).rev() {
        b.extend_from_slice(&bgra[y * row..y * row + row]);
    }
    b
}
