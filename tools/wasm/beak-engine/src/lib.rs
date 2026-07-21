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
pub mod fonts;
pub mod forms;
pub mod image;
pub mod layout;
pub mod raster;
pub mod style;
pub mod svg;
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
<h2>Formulare (GET)</h2>\
<form action=\"/search\">\
<p>Suche: <input name=\"q\" value=\"nopeekOS\" size=\"22\"> <input type=\"submit\" value=\"Los\"> \
<input type=\"checkbox\" name=\"exact\" checked> nur exakt \
<select name=\"lang\"><option value=\"de\">Deutsch</option>\
<option value=\"en\" selected>English</option></select></p>\
<p><input name=\"leer\" placeholder=\"Platzhalter, wenn leer\" size=\"26\"> \
<button>Senden</button></p>\
<p><textarea name=\"msg\" rows=\"2\" cols=\"38\">Mehrzeiliger Text im textarea</textarea></p>\
</form>\
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

    // ── SVG rasteriser demo (eyeball svg_demo.bmp) ─────────────────────────
    // Run: `cargo test --release render_svg_to_bmp -- --nocapture`
    const SVG_DEMO: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 160" width="240" height="160">
      <rect x="4" y="4" width="232" height="152" rx="16" ry="16" fill="#1f2933"/>
      <g transform="translate(20,20)">
        <circle cx="30" cy="30" r="26" fill="#4fd1c5" fill-opacity="0.9"/>
        <path d="M70 4 L118 4 L94 56 Z" fill="#e0662c"/>
        <path d="M130 8 C130 8 200 8 200 40 S150 72 130 56 Z" fill="#f6c453"/>
      </g>
      <g transform="translate(120,96) rotate(12)">
        <rect x="0" y="0" width="90" height="44" rx="8" fill="#6b7280"/>
        <path d="M8 22 a14 14 0 1 0 28 0 a14 14 0 1 0 -28 0 M44 22 h38" fill="none"/>
        <circle cx="22" cy="22" r="9" fill="#e5e7eb"/>
      </g>
      <path fill-rule="evenodd" fill="#9aa0a6"
            d="M30 120 h60 v30 h-60 Z M42 130 h36 v10 h-36 Z"/>
    </svg>"##;

    #[test]
    fn render_svg_to_bmp() {
        let img = crate::svg::render(SVG_DEMO).expect("svg render");
        let (w, h) = (img.w, img.h);
        // composite the straight-BGRA (with alpha) over a white page background
        let mut buf = alloc::vec![255u8; (w * h * 4) as usize];
        for i in 0..(w * h) as usize {
            let a = img.bgra[i * 4 + 3] as u32;
            if a == 0 {
                continue;
            }
            let ia = 255 - a;
            for c in 0..3 {
                buf[i * 4 + c] =
                    ((img.bgra[i * 4 + c] as u32 * a + buf[i * 4 + c] as u32 * ia) / 255) as u8;
            }
        }
        let painted = img.bgra.chunks_exact(4).filter(|p| p[3] > 0).count();
        std::fs::write("svg_demo.bmp", to_bmp(&buf, w, h)).expect("write svg_demo.bmp");
        std::eprintln!("svg render: {w}x{h} px, {painted} painted px → svg_demo.bmp");
        assert!(painted > 2000, "expected the icon to paint");
    }

    // ── real icon-set contact sheet (eyeball icons_sheet.bmp) ──────────────
    // Renders every *.svg in ICONS_DIR into a tiled sheet + reports how many
    // painted (stroke-only icons paint 0 px in v1 → the stroke gap, measured).
    // Run: `ICONS_DIR=../../../icons/phosphor cargo test --release
    //       render_icons_sheet -- --nocapture`
    #[test]
    fn render_icons_sheet() {
        let dir = std::env::var("ICONS_DIR").unwrap_or_else(|_| "../../../icons/phosphor".into());
        let mut files: alloc::vec::Vec<std::path::PathBuf> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().map(|x| x == "svg").unwrap_or(false))
                .collect(),
            Err(_) => {
                std::eprintln!("ICONS_DIR '{dir}' not readable — skipping");
                return;
            }
        };
        files.sort();
        if files.is_empty() {
            return;
        }

        const CELL: u32 = 96;
        const PAD: u32 = 8;
        let cols = 8u32;
        let rows = (files.len() as u32).div_ceil(cols);
        let sw = cols * CELL;
        let sh = rows * CELL;
        let mut sheet = alloc::vec![245u8; (sw * sh * 4) as usize]; // light grey page

        let (mut ok, mut blank, mut fail) = (0u32, 0u32, 0u32);
        for (idx, path) in files.iter().enumerate() {
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let img = match crate::svg::render(&bytes) {
                Some(i) => i,
                None => {
                    fail += 1;
                    continue;
                }
            };
            let painted = img.bgra.chunks_exact(4).filter(|p| p[3] > 0).count();
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            if painted == 0 {
                blank += 1;
                std::eprintln!("  BLANK {name}");
            } else {
                ok += 1;
            }
            // scale img into CELL-2*PAD, keep aspect, centre; composite over sheet
            let box_ = CELL - 2 * PAD;
            let s = (box_ as f32 / img.w as f32).min(box_ as f32 / img.h as f32);
            let dw = ((img.w as f32 * s) as u32).max(1);
            let dh = ((img.h as f32 * s) as u32).max(1);
            let (col, row) = (idx as u32 % cols, idx as u32 / cols);
            let ox = col * CELL + (CELL - dw) / 2;
            let oy = row * CELL + (CELL - dh) / 2;
            for py in 0..dh {
                for px in 0..dw {
                    let sx = (px * img.w / dw).min(img.w - 1);
                    let sy = (py * img.h / dh).min(img.h - 1);
                    let si = ((sy * img.w + sx) * 4) as usize;
                    let a = img.bgra[si + 3] as u32;
                    if a == 0 {
                        continue;
                    }
                    let di = (((oy + py) * sw + (ox + px)) * 4) as usize;
                    let ia = 255 - a;
                    for c in 0..3 {
                        sheet[di + c] =
                            ((img.bgra[si + c] as u32 * a + sheet[di + c] as u32 * ia) / 255) as u8;
                    }
                }
            }
        }
        std::fs::write("icons_sheet.bmp", to_bmp(&sheet, sw, sh)).expect("write sheet");
        std::eprintln!(
            "icons: {} total → {ok} painted, {blank} blank (stroke-only), {fail} unsupported → icons_sheet.bmp",
            files.len()
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
