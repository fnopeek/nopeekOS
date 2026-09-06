//! Eine WOFF2-Datei entpacken und gegen eine Referenz-TTF stellen.
//!
//!   cargo run --release --example woffcheck -- x.woff2 [x.ttf]
//!
//! Verglichen werden die TABELLEN, nicht die Datei: Reihenfolge, Ausrichtung
//! und Pruefsummen darf ein Entpacker anders schreiben, der INHALT nicht.
fn main() {
    let a = std::env::args().nth(1).expect("woff2");
    let src = std::fs::read(&a).expect("lesen");
    let mut tr = beak_engine::woff2::Trace::default();
    let got = match beak_engine::woff2::to_sfnt_traced(&src, &mut tr) {
        Some(v) => v,
        None => {
            println!("{a}: NICHT entpackbar — Stelle: {} (Glyphe {})",
                     tr.step, if tr.glyph == usize::MAX { -1 } else { tr.glyph as i64 });
            std::process::exit(1)
        }
    };
    println!("{}: {} B -> {} B", a.rsplit('/').next().unwrap(), src.len(), got.len());
    let mine = tables(&got);
    // Laedt fontdue es?
    match fontdue::Font::from_bytes(got.as_slice(), fontdue::FontSettings::default()) {
        Ok(f) => {
            let m = f.metrics('A', 16.0);
            println!("  fontdue: ok, {} Glyphen, 'A' {}x{} adv {:.1}",
                     f.glyph_count(), m.width, m.height, m.advance_width);
        }
        Err(e) => println!("  fontdue: FEHLER {e}"),
    }
    let Some(refp) = std::env::args().nth(2) else { return };
    let Ok(rf) = std::fs::read(&refp) else { println!("  keine Referenz"); return };
    let theirs = tables(&rf);
    let mut bad = 0;
    for (tag, d) in &theirs {
        match mine.iter().find(|(t, _)| t == tag) {
            None => { println!("  FEHLT   {tag}"); bad += 1; }
            Some((_, m)) if m == d => {}
            Some((_, m)) => {
                let same = m.iter().zip(d.iter()).take_while(|(a, b)| a == b).count();
                println!("  ANDERS  {tag}: {} B statt {} B, erste Abweichung bei {}",
                         m.len(), d.len(), same);
                bad += 1;
            }
        }
    }
    for (tag, _) in &mine {
        if !theirs.iter().any(|(t, _)| t == tag) { println!("  ZUVIEL  {tag}"); }
    }
    println!("  {} von {} Tabellen gleich", theirs.len() - bad, theirs.len());

    // **Der eigentliche Beweis: dieselben PIXEL.** Ein `glyf`, das anders
    // kodiert ist, darf denselben Umriss haben — und genau das muss geprueft
    // werden, nicht die Bytezahl.
    let (Ok(a), Ok(b)) = (
        fontdue::Font::from_bytes(got.as_slice(), fontdue::FontSettings::default()),
        fontdue::Font::from_bytes(rf.as_slice(), fontdue::FontSettings::default()),
    ) else { println!("  Rasterprobe: eine Seite laedt nicht"); return };
    let mut checked = 0;
    let mut diff = 0;
    for (ch, _) in b.chars() {
        let (ma, ba) = a.rasterize(*ch, 24.0);
        let (mb, bbm) = b.rasterize(*ch, 24.0);
        checked += 1;
        if ma.width != mb.width || ma.height != mb.height
            || (ma.advance_width - mb.advance_width).abs() > 0.01 || ba != bbm {
            if diff < 5 {
                println!("  PIXEL ANDERS bei {:?}: {}x{} adv {:.2}  vs  {}x{} adv {:.2}",
                         ch, ma.width, ma.height, ma.advance_width,
                         mb.width, mb.height, mb.advance_width);
            }
            diff += 1;
        }
    }
    println!("  Rasterprobe: {} Zeichen, {} abweichend", checked, diff);
}

fn tables(d: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    if d.len() < 12 { return out }
    let n = u16::from_be_bytes([d[4], d[5]]) as usize;
    for i in 0..n {
        let o = 12 + i * 16;
        if o + 16 > d.len() { break }
        let tag = String::from_utf8_lossy(&d[o..o + 4]).to_string();
        let off = u32::from_be_bytes([d[o+8], d[o+9], d[o+10], d[o+11]]) as usize;
        let len = u32::from_be_bytes([d[o+12], d[o+13], d[o+14], d[o+15]]) as usize;
        if off + len <= d.len() { out.push((tag, d[off..off + len].to_vec())); }
    }
    out
}
