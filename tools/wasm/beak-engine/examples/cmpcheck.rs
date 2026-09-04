// Je Bootstrap-Komponente eine Zeile: wie hoch, wieviele Zeichenbefehle?
//
// Der Sinn der Komponentenvorlage steht und faellt damit, dass ein Befund
// EINEN Block nennt. Ein Diff ueber die ganze Seite sagt „4,2 % anders" und
// hilft niemandem; diese Tabelle sagt „c-modal ist 14 000 px hoch", und dann
// weiss man, wo man hinschaut.
//
//   cargo run --release --example cmpcheck            (Breite 1902, wie am Geraet)
//   W=1000 cargo run --release --example cmpcheck
//   CMP=c-modal cargo run --release --example cmpcheck   nur einen Block
//
// Jeder Block wird EINZELN ausgelegt, mit demselben Kopf wie die ganze Seite.
// Das isoliert: was in einem Block schiefgeht, kann den naechsten nicht mehr
// verschieben, und die Hoehe ist die des Blocks und nicht die seiner Nachbarn.
fn main() {
    let page = include_str!("../../../fixtures/components.html");
    let css = include_str!("../assets/bootstrap.min.css");
    let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
    let only = std::env::var("CMP").ok();

    // Der eigene <style>-Block der Vorlage gehoert dazu — er zeichnet den
    // Rahmen um jeden Block, und ohne ihn misst man eine andere Seite.
    let own = between(page, "<style>", "</style>").unwrap_or_default();

    println!("\n── Bootstrap-Komponenten einzeln, @{width}px ──\n");
    println!("   {:<16} {:>7}  {:>6}  {:>6}", "block", "hoehe", "ops", "links");
    let mut total = 0f32;
    for (id, body) in sections(page) {
        if let Some(f) = &only { if &id != f { continue } }
        let doc = alloc_doc(&own, &body);
        let mut eng = beak_engine::Engine::new();
        eng.set_theme(light());
        let lay = eng.layout_ext(&doc, css, width);
        total += lay.height as f32;
        println!("   {:<16} {:>7}  {:>6}  {:>6}", id, lay.height, lay.ops.len(), lay.links.len());
    }
    println!("\n   zusammen {total} px");
}

fn light() -> beak_engine::layout::Theme {
    use beak_engine::layout::{Rgb, Theme};
    Theme { bg: Rgb(255, 255, 255), text: Rgb(33, 37, 41), heading: Rgb(33, 37, 41),
            link: Rgb(13, 110, 253), muted: Rgb(108, 117, 125), rule: Rgb(222, 226, 230) }
}

fn alloc_doc(own_css: &str, body: &str) -> String {
    format!("<!DOCTYPE html><html><head><style>{own_css}</style></head><body>{body}</body></html>")
}

fn between(s: &str, a: &str, b: &str) -> Option<String> {
    let i = s.find(a)? + a.len();
    let j = s[i..].find(b)? + i;
    Some(s[i..j].to_string())
}

/// Jeden `<section id="…">…</section>`-Block als (id, html).
fn sections(page: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = page;
    while let Some(i) = rest.find("<section id=\"") {
        let after = &rest[i + 13..];
        let Some(q) = after.find('"') else { break };
        let id = after[..q].to_string();
        let Some(end) = after.find("</section>") else { break };
        out.push((id, rest[i..i + 13 + end + 10].to_string()));
        rest = &after[end..];
    }
    out
}
