#![cfg_attr(not(test), no_std)]
//! beak-engine — portable browser engine core (nopeekOS `beak`).
//!
//! Pure `no_std` + `alloc`, **no host-fn dependencies** → the whole engine
//! builds and unit-tests on any target with no OS in the loop (BROWSER.md
//! §10). The pipeline is the real browser shape, grown incrementally:
//!
//! ```text
//!   HTML ──▶ [dom] tree ──▶ [style] cascade (UA sheet + inline) ──▶
//!   [layout] block + inline flow ──▶ display list ──▶ [raster] BGRA pixels
//! ```
//!
//! Everything is host-testable: `dom`/`style`/`layout` have native `cargo
//! test`s, and the `demo` renders a page to a BMP you can eyeball on the dev
//! box (§10) — the CSS conformance oracle is a render-and-compare, no browser.

extern crate alloc;

pub mod color;
pub mod css;
pub mod dom;
pub mod image;
pub mod layout;
pub mod raster;
pub mod style;
pub mod values;
pub mod vars;

pub use dom::{parse, title, Dom, Element, Node};
pub use image::{Image, ImageMap};
pub use layout::{Layout, Rgb, Theme};
pub use raster::Engine;

/// Hrefs of every `<link rel="stylesheet">` in an HTML document. The shell
/// fetches these as sub-resources and feeds the bytes back via
/// `Engine::layout_ext` (the engine cannot fetch — it is host-free).
pub fn stylesheet_links(html: &str) -> alloc::vec::Vec<alloc::string::String> {
    css::stylesheet_links(&dom::parse(html))
}

/// `src` of every `<img>` in an HTML document (as written), for the shell to
/// fetch + hand back via `Engine::set_images`.
pub fn image_srcs(html: &str) -> alloc::vec::Vec<alloc::string::String> {
    fn walk(el: &Element, out: &mut alloc::vec::Vec<alloc::string::String>) {
        for c in &el.children {
            if let Node::Element(e) = c {
                if e.tag == "img" {
                    if let Some(s) = e.attr("src") {
                        if !s.trim().is_empty() {
                            out.push(alloc::string::ToString::to_string(s.trim()));
                        }
                    }
                }
                walk(e, out);
            }
        }
    }
    let dom = dom::parse(html);
    let mut out = alloc::vec::Vec::new();
    walk(&dom.root, &mut out);
    out
}

// Host demo: render a representative page to a BMP so the layout + text can be
// eyeballed on the dev box without booting the OS (BROWSER.md §10).
// Run: `cargo test --release render_sample_to_bmp -- --nocapture`
// → writes `tools/wasm/beak-engine/sample.bmp`.
#[cfg(test)]
mod demo {
    use crate::Engine;

    const SAMPLE: &str = "<!DOCTYPE html><html><head><title>beak — CSS + inline flow</title>\
<style>\
  h2 { color: #4fd1c5 }\
  .note { color: #e0662c; font-weight: bold }\
  .box a { color: #f6c453 }\
  blockquote { color: #9aa0a6; font-style: italic }\
</style>\
</head><body>\
<h1>Ein nativer Browser für <i>nopeekOS</i></h1>\
<p>Dieser Absatz wird von der eigenen Layout-Engine umgebrochen — die Wörter fließen \
auf die Content-Breite, und <b>fetter</b> wie <i>kursiver</i> Text sowie \
<a href=\"https://de.wikipedia.org/\">Links</a> fließen jetzt <b>inline</b> mitten \
im Satz statt jeweils auf eigener Zeile. Kein Linux, kein microVM, keine fremde Engine.</p>\
<h2>Was schon läuft</h2>\
<ul>\
<li>echter DOM-Baum statt flacher Blöcke</li>\
<li>UA-Stylesheet als Daten (Kaskade + Vererbung)</li>\
<li>Inline-Flow mit gemischten Stilen auf einer Zeile</li>\
<li><code>&lt;style&gt;</code>-Regeln (Selektoren + Spezifität) und <code>style=\"…\"</code></li>\
</ul>\
<p class=\"note\">Dieser Absatz trägt <code>class=\"note\"</code> — Farbe und Fettung kommen \
aus einer <b>Author-CSS-Regel</b> im <code>&lt;style&gt;</code>-Block, nicht aus dem Markup.</p>\
<div class=\"box\">In dieser <code>.box</code> ist der \
<a href=\"https://de.wikipedia.org/\">Link</a> per Descendant-Selektor \
(<code>.box a</code>) anders eingefärbt als sonst.</div>\
<blockquote>Standard-first: gemessen gegen die Strichliste, nicht nach Augenmaß.</blockquote>\
<table>\
<caption>Beispiel-Infobox (Tabellen-Layout)</caption>\
<tr><th>Land</th><td>Schweiz</td></tr>\
<tr><th>Kanton</th><td>Nidwalden</td></tr>\
<tr><th>Fläche</th><td>17,11 km² — Wörter in einer Zelle brechen bei Bedarf um</td></tr>\
</table>\
<pre>fn main() {\n    println!(\"pre erhält Whitespace + Zeilen\");\n}</pre>\
<div style=\"display:flex; gap:16px; justify-content:space-between\">\
<div style=\"font-weight:bold\">Flexbox-Zeile</div><div>Anfang</div>\
<div>Mitte</div><div style=\"color:#4fd1c5\">Ende (space-between)</div></div>\
<div style=\"display:flex; gap:12px\">\
<div style=\"flex:1; color:#e0662c\">flex:1 — linke Spalte</div>\
<div style=\"flex:2\">flex:2 — rechte Spalte, doppelt so breit; Text bricht \
innerhalb der Flex-Spalte um wenn er nicht in eine Zeile passt.</div></div>\
<div style=\"display:grid; grid-template-columns:repeat(3, 1fr); gap:10px\">\
<div style=\"color:#4fd1c5\">Grid A</div><div>Grid B</div><div>Grid C</div>\
<div style=\"grid-column:span 2; color:#e0662c\">Grid D (span 2)</div><div>Grid E</div></div>\
<div style=\"max-width:520px; margin:18px auto; padding:16px; background:#1f2430; border:1px solid #3a3f4b\">\
<b>Container</b> mit <code>max-width</code> + <code>margin:0 auto</code> (zentriert), \
<code>padding</code>, <code>background</code> und <code>border</code> — genau so steckt \
auf echten Seiten jeder Block in einem Container.</div>\
<div style=\"position:relative; background:#1f2430; padding:16px; margin-top:14px; border:1px solid #3a3f4b\">\
<span style=\"position:absolute; top:8px; right:12px; color:#4fd1c5\">position:absolute</span>\
<b>position:relative</b>-Container — das Badge oben rechts sitzt per \
<code>position:absolute</code> (aus dem Fluss) an der Container-Ecke.</div>\
<p>Ein echtes Bild (dekodiert + skaliert), daneben ein Platzhalter für ein \
nicht ladbares:</p>\
<img src=\"demo.png\" alt=\"Verlauf\" width=\"240\" height=\"160\">\
<img src=\"fehlt.jpg\" alt=\"nicht geladen (JPEG folgt)\" width=\"240\" height=\"90\">\
<hr>\
<h2>Nächste Schritte</h2>\
<p>Tabellen-Layout und Flexbox/Grid — der Reihe nach. Die Engine bleibt portabel: \
dieselbe Rechnerei läuft auf dem Desktop.</p>\
</body></html>";

    fn to_bmp(bgra: &[u8], w: u32, h: u32) -> alloc::vec::Vec<u8> {
        let row = (w * 4) as usize;
        let pixels = row * h as usize;
        let mut b = alloc::vec::Vec::with_capacity(54 + pixels);
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&((54 + pixels) as u32).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&54u32.to_le_bytes());
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&(w as i32).to_le_bytes());
        b.extend_from_slice(&(h as i32).to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&(pixels as u32).to_le_bytes());
        b.extend_from_slice(&2835u32.to_le_bytes());
        b.extend_from_slice(&2835u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        // our buffer is top-down; BMP is bottom-up → reverse rows.
        for y in (0..h).rev() {
            let s = y as usize * row;
            b.extend_from_slice(&bgra[s..s + row]);
        }
        b
    }

    #[test]
    fn render_sample_to_bmp() {
        let mut eng = Engine::new();
        // Feed the demo <img src="demo.png"> a real decoded image (the shell
        // fetches these over the network; here we embed one for the host demo).
        eng.set_images(&[(
            alloc::string::String::from("demo.png"),
            include_bytes!("../assets/demo.png").to_vec(),
        )]);
        let width = 760u32;
        let lay = eng.layout(SAMPLE, width);
        let height = lay.height.min(3000);
        let mut buf = alloc::vec![0u8; (width * height * 4) as usize];
        eng.paint(&lay, width, height, 0, &mut buf);

        // sanity: something was actually drawn (not pure background).
        let bg_b = crate::layout::Theme::DARK.bg.2;
        assert!(buf.chunks(4).any(|p| p[0] != bg_b), "nothing rendered");

        std::fs::write("sample.bmp", to_bmp(&buf, width, height)).expect("write sample.bmp");
        std::eprintln!(
            "beak render: {}x{} px, {} draw ops, {} links → sample.bmp",
            width, height, lay.ops.len(), lay.links.len()
        );
    }

    // ── Bootstrap fidelity oracle ──────────────────────────────────────────
    // Renders a representative Bootstrap 5 page with the REAL bootstrap.min.css
    // (assets/) so we can measure "does it look as the author intended".
    // Run: `cargo test --release render_bootstrap_to_bmp -- --nocapture`
    // → writes `tools/wasm/beak-engine/bootstrap.bmp`.
    const BOOTSTRAP_SAMPLE: &str = "<!DOCTYPE html><html><head><title>Bootstrap</title></head><body>\
<nav class=\"navbar navbar-expand-lg navbar-dark bg-primary\"><div class=\"container\">\
<a class=\"navbar-brand\" href=\"#\">beak</a>\
<div class=\"navbar-nav\"><a class=\"nav-link active\" href=\"#\">Home</a>\
<a class=\"nav-link\" href=\"#\">Features</a><a class=\"nav-link\" href=\"#\">About</a></div>\
</div></nav>\
<div class=\"container mt-4\"><div class=\"row\">\
<div class=\"col-md-8\"><div class=\"card\"><div class=\"card-body\">\
<h5 class=\"card-title\">Card Title</h5>\
<p class=\"card-text\">Some quick example text to build on the card title and make up \
the bulk of the card's content.</p>\
<a href=\"#\" class=\"btn btn-primary\">Primary</a> \
<a href=\"#\" class=\"btn btn-secondary\">Secondary</a></div></div></div>\
<div class=\"col-md-4\">\
<div class=\"alert alert-warning\" role=\"alert\">A warning alert with \
<a href=\"#\" class=\"alert-link\">a link</a>.</div>\
<span class=\"badge bg-success\">Success</span> <span class=\"badge bg-danger\">Danger</span>\
</div></div>\
<div class=\"row mt-4\">\
<div class=\"col\"><div class=\"p-3 bg-light border\">Column one</div></div>\
<div class=\"col\"><div class=\"p-3 bg-light border\">Column two</div></div>\
<div class=\"col\"><div class=\"p-3 bg-light border\">Column three</div></div>\
</div></div></body></html>";

    #[test]
    fn render_bootstrap_to_bmp() {
        use crate::layout::{Rgb, Theme};
        let mut eng = Engine::new();
        // Bootstrap targets a light body; seed a light palette so an unresolved
        // body background still reads correctly.
        eng.set_theme(Theme {
            bg: Rgb(255, 255, 255),
            text: Rgb(33, 37, 41),
            heading: Rgb(33, 37, 41),
            link: Rgb(13, 110, 253),
            muted: Rgb(108, 117, 125),
            rule: Rgb(222, 226, 230),
        });
        let css = include_str!("../assets/bootstrap.min.css");
        let width = 1000u32;
        let lay_ua = eng.layout_ua(BOOTSTRAP_SAMPLE, width);
        // How many elements did the DOM actually parse?
        fn count(e: &crate::dom::Element, els: &mut u32, txt: &mut u32) {
            *els += 1;
            for c in &e.children {
                match c {
                    crate::dom::Node::Element(ce) => count(ce, els, txt),
                    crate::dom::Node::Text(t) if !t.trim().is_empty() => *txt += 1,
                    _ => {}
                }
            }
        }
        let d = crate::dom::parse(BOOTSTRAP_SAMPLE);
        let (mut els, mut txt) = (0u32, 0u32);
        count(&d.root, &mut els, &mut txt);
        std::eprintln!(
            "  [diag] DOM: {} elements, {} non-empty text nodes | UA-only: {} ops, {} links, {}px",
            els, txt, lay_ua.ops.len(), lay_ua.links.len(), lay_ua.height
        );
        let lay = eng.layout_ext(BOOTSTRAP_SAMPLE, css, width);
        let height = lay.height.clamp(1, 4000);
        let mut buf = alloc::vec![0u8; (width * height * 4) as usize];
        eng.paint(&lay, width, height, 0, &mut buf);
        std::fs::write("bootstrap.bmp", to_bmp(&buf, width, height)).expect("write bootstrap.bmp");
        std::eprintln!(
            "bootstrap render: {}x{} px, {} ops, {} links → bootstrap.bmp",
            width, height, lay.ops.len(), lay.links.len()
        );
    }
}
