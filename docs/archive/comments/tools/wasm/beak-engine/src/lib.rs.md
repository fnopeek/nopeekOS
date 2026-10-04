# `tools/wasm/beak-engine/src/lib.rs` @ 5e0102684

## L2-15 · `extern crate alloc;`

```
//! beak-engine — portable browser engine core (nopeekOS `beak`).
//!
//! Pure `no_std` + `alloc`, **no host-fn dependencies** → the whole engine
//! builds and unit-tests on any target with no OS in the loop (docs/spec/BROWSER.md
//! §10). The pipeline is the real browser shape, grown incrementally:
//!
//! `​``text
//!   HTML ──▶ [dom] tree ──▶ [style] cascade (UA sheet + inline) ──▶
//!   [layout] block + inline flow ──▶ display list ──▶ [raster] BGRA pixels
//! `​``
//!
//! Everything is host-testable: `dom`/`style`/`layout` have native `cargo
//! test`s, and the `demo` renders a page to a BMP you can eyeball on the dev
//! box (§10) — the CSS conformance oracle is a render-and-compare, no browser.
```

## L39-45 · `#[cfg(test)]`

```
/// **Die Reichweitenregel des KERNELS, hier nur zum Fahren.**
///
/// Der Kernel hat keine Testinfrastruktur, und eine Sicherheitsregel, die
/// niemand fahren kann, ist eine Behauptung. Die Datei wird deshalb von dort
/// EINGEHAENGT statt kopiert: es gibt genau eine Fassung, und ihre Tabelle
/// laeuft bei jedem `cargo test` dieses Crates mit.
/// Siehe `docs/plan/BROWSER_FETCH_ORIGIN.md` §3.1 V2.
```

## L59-61 · `pub fn stylesheet_links(html: &str) -> alloc::vec::Vec<alloc::string::String> {`

```
/// Hrefs of every `<link rel="stylesheet">` in an HTML document. The shell
/// fetches these as sub-resources and feeds the bytes back via
/// `Engine::layout_ext` (the engine cannot fetch — it is host-free).
```

## L66-68 · `pub fn import_urls(css: &str) -> alloc::vec::Vec<alloc::string::String> {`

```
/// The `@import` targets of ONE stylesheet, in source order. They resolve
/// against that sheet's own URL, not the document's, so the shell has to ask
/// per sheet rather than over the concatenated buffer.
```

## L73-77 · `pub fn image_srcs(html: &str, width: u32) -> alloc::vec::Vec<alloc::string::String> {`

```
/// `src` of every `<img>` in an HTML document (as written), for the shell to
/// fetch + hand back via `Engine::set_images`.
/// Every `<img src>` in the document, in document order — the shell's fetch
/// list. `width` is the viewport: `<picture>`/`srcset` is resolved first, so
/// this returns exactly the URLs layout will ask for at that width.
```

## L85-88 · `let inline = s.starts_with("data:") || s.starts_with("DATA:");`

```
// A `data:` src carries its own bytes — the engine
// decodes it during layout and the shell never hears
// about it. Reporting it would send the whole payload
// to the network as if it were a URL.
```

## L106-107 · `pub(crate) fn collect_img_srcs(el: &Element, out: &mut alloc::vec::Vec<alloc::string::String>) {`

```
/// Die `src` aller `<img>` unter `el` — dieselbe Regel wie in `image_srcs`,
/// als eigene Funktion, weil zwei Baeume sie brauchen.
```

## L114-116 · `let inline = s.starts_with("data:") || s.starts_with("DATA:");`

```
// Ein `data:` traegt seine Bytes selbst — es dem Wirt zu
// melden hiesse, die ganze Nutzlast als Adresse ins Netz
// zu schicken.
```

## L128-131 · `#[cfg(test)]`

```
// Host demo: render a representative page to a BMP so the layout + text can be
// eyeballed on the dev box without booting the OS (docs/spec/BROWSER.md §10).
// Run: `cargo test --release render_sample_to_bmp -- --nocapture`
// → writes `tools/wasm/beak-engine/sample.bmp`.
```

## L227 · `for y in (0..h).rev() {`

```
// our buffer is top-down; BMP is bottom-up → reverse rows.
```

## L238-239 · `eng.set_images(&[(`

```
// Feed the demo <img src="demo.png"> a real decoded image (the shell
// fetches these over the network; here we embed one for the host demo).
```

## L250 · `let bg_b = crate::layout::Theme::DARK.bg.2;`

```
// sanity: something was actually drawn (not pure background).
```

## L261-262 · `const SVG_DEMO: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 160" width="240" height="160">`

```
// ── SVG rasteriser demo (eyeball svg_demo.bmp) ─────────────────────────
// Run: `cargo test --release render_svg_to_bmp -- --nocapture`
```

## L283 · `let mut buf = alloc::vec![255u8; (w * h * 4) as usize];`

```
// composite the straight-BGRA (with alpha) over a white page background
```

## L302-306 · `#[test]`

```
// ── real icon-set contact sheet (eyeball icons_sheet.bmp) ──────────────
// Renders every *.svg in ICONS_DIR into a tiled sheet + reports how many
// painted (stroke-only icons paint 0 px in v1 → the stroke gap, measured).
// Run: `ICONS_DIR=../../../icons/phosphor cargo test --release
//       render_icons_sheet -- --nocapture`
```

## L330 · `let mut sheet = alloc::vec![245u8; (sw * sh * 4) as usize]; // light grey page`

```
// light grey page
```

## L353 · `let box_ = CELL - 2 * PAD;`

```
// scale img into CELL-2*PAD, keep aspect, centre; composite over sheet
```

## L386-395 · `const BOOTSTRAP_SAMPLE: &str = include_str!("../../../fixtures/components.html");`

```
// ── Bootstrap fidelity oracle ──────────────────────────────────────────
// Renders a representative Bootstrap 5 page with the REAL bootstrap.min.css
// (assets/) so we can measure "does it look as the author intended".
// Run: `cargo test --release render_bootstrap_to_bmp -- --nocapture`
// → writes `tools/wasm/beak-engine/bootstrap.bmp`.
// Die Vorlage liegt als DATEI da, nicht als Zeichenkette hier: derselbe
// Byte-fuer-Byte gleiche Text geht host-seitig durch diesen Test und
// ueber `tools/pageserver.py` ans Geraet. Eine Pruefseite, die in zwei
// Fassungen existiert, vergleicht zwei Dinge und nicht eins
// ([[project_beak_selftest_page]] macht es genauso).
```

## L402-403 · `eng.set_theme(Theme {`

```
// Bootstrap targets a light body; seed a light palette so an unresolved
// body background still reads correctly.
```

## L413-416 · `let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);`

```
// Die Breite, die das Geraet fährt (`layout @1902px` im Log). Eine
// andere Breite waehlt andere Bootstrap-Haltepunkte, und dann
// vergleicht man zwei Layouts statt zweier Maschinen
// ([[feedback_host_profile_is_not_the_device]]).
```

## L419-420 · `let height = lay.height.clamp(1, 20000);`

```
// Der Deckel stand auf 4000 — die Komponentenseite ist hoeher, und ein
// abgeschnittenes Bild sagt ueber die letzten Bloecke nichts.
```

