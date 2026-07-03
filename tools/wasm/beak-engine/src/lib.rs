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

pub mod css;
pub mod dom;
pub mod layout;
pub mod raster;
pub mod style;

pub use dom::{parse, title, Dom, Element, Node};
pub use layout::{Layout, Rgb, Theme};
pub use raster::Engine;

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
<pre>fn main() {\n    println!(\"pre erhält Whitespace + Zeilen\");\n}</pre>\
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
        let eng = Engine::new();
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
}
