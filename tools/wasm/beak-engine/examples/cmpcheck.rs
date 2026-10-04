// One line per Bootstrap component: height and draw-command count.
//
// A diff over the whole page says "4.2 % different"; a per-block table names
// the block that is wrong.
//
//   cargo run --release --example cmpcheck            (width 1902)
//   W=1000 cargo run --release --example cmpcheck
//   CMP=c-modal cargo run --release --example cmpcheck   one block only
//
// Each block is laid out on its own with the page's head. That isolates it:
// an error in one block cannot shift the next, and the height is the block's
// own.
fn main() {
    // Two fixtures, one tool: `FIX=tailwind` checks the utility families,
    // without `FIX` the Bootstrap components.
    let tw = std::env::var("FIX").is_ok_and(|f| f == "tailwind");
    let page = if tw { include_str!("../../../fixtures/tailwind.html") }
               else { include_str!("../../../fixtures/components.html") };
    let css = if tw { include_str!("../assets/tailwind.css") }
              else { include_str!("../assets/bootstrap.min.css") };
    let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
    let only = std::env::var("CMP").ok();

    // The fixture's own <style> block draws the frame around each block and
    // belongs to the page being measured.
    let own = between(page, "<style>", "</style>").unwrap_or_default();

    println!("\n── {} einzeln, @{width}px ──\n", if tw { "Tailwind-Utilities" } else { "Bootstrap-Komponenten" });
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

/// Every `<section id="…">…</section>` block as (id, html).
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
