# `tools/wasm/beak-engine/tests/wpt.rs` @ 5e0102684

## L1-21 · `use std::collections::BTreeMap;`

```
//! WPT reftest oracle — the objective CSS-fidelity gate (docs/spec/BROWSER.md §10).
//!
//! For every vendored Web-Platform-Test reftest under `tests/wpt/`, render the
//! TEST file and its `<link rel="match">` REFERENCE through the beak engine and
//! pixel-compare them. A reftest passes when test ≈ reference (both rendered by
//! OUR engine, so identical structure/font cancels out — only the property
//! under test can differ). This turns fidelity into a measured pass/fail per
//! spec feature instead of eyeballing, and each failing reftest is a concrete,
//! self-verifying work item.
//!
//! Vendored from web-platform-tests/wpt (css/…). Run:
//!   cargo test --release --manifest-path tools/wasm/beak-engine/Cargo.toml \
//!     --test wpt -- --nocapture
//!
//! The run is the project's tempo — every decision waits on this number — so it
//! is built to be cheap: tests are GROUPED BY REFERENCE (228 of them share
//! `ref-if-there-is-no-red.xht`, and 2605 of 5736 reference renders were pure
//! duplicate work), and the groups are spread over every core.
//!
//! `WPT_FILTER=<substr>`  narrow to one feature · `WPT_DUMP=<dir>` write BMPs ·
//! `WPT_JOBS=<n>` thread count · `WPT_BLESS=1` rewrite the baseline.
```

## L33 · `fn light() -> Theme {`

```
/// A white-background light theme (WPT reftests assume a white canvas).
```

## L45 · `fn render(html: &str) -> Vec<u8> {`

```
/// Render an HTML document (its inline `<style>` is the author sheet) to BGRA.
```

## L55 · `fn match_href(html: &str) -> Option<String> {`

```
/// The `href` of the `<link rel="match" href="…">` in a reftest, if any.
```

## L78-79 · `fn diff_fraction(a: &[u8], b: &[u8]) -> f64 {`

```
/// Fraction of pixels differing beyond a small per-channel tolerance (accounts
/// for anti-aliasing at glyph/box edges shared by test + ref).
```

## L96-100 · `fn ink_fraction(buf: &[u8]) -> f64 {`

```
/// Fraction of pixels that are NOT the white canvas — the reference's "ink".
/// If a reference renders (near-)blank, the reftest is INCONCLUSIVE: a blank
/// test would trivially "match" a blank reference, so equality proves nothing.
/// This guards against false passes on features we don't render yet (e.g. an
/// empty `<div>` sized only by `height` that collapses to nothing).
```

## L117-118 · `fn write_bmp(path: &Path, buf: &[u8], w: u32, h: u32) {`

```
/// Write a BGRA buffer as a 24-bit bottom-up BMP, so a failing reftest can be
/// looked at instead of guessed about (`WPT_DUMP=<dir>`).
```

## L147 · `fn collect_tests(dir: &Path, out: &mut Vec<PathBuf>) {`

```
/// Collect every reftest (a `*.html` that is not a `-ref.html`) under `dir`.
```

## L155-156 · `let is_ref = n.ends_with("-ref.html") || n.ends_with("-ref.xht");`

```
// WPT reftests come as .html AND .xht (XHTML). Exclude the *-ref.*
// reference files (they're loaded via each test's rel=match href).
```

## L166 · `const PASS_MAX_DIFF: f64 = 0.005; // ≤0.5% of pixels may differ`

```
/// Pass threshold: reftests are near-exact; allow a hair for AA/rounding.
```

## L167 · `const PASS_MAX_DIFF: f64 = 0.005; // ≤0.5% of pixels may differ`

```
// ≤0.5% of pixels may differ
```

## L168-169 · `const MIN_REF_INK: f64 = 0.001; // 0.1% of the canvas`

```
/// A reference must render at least this much non-white ink to be a conclusive
/// comparison — otherwise a blank test trivially "matches" it.
```

## L170 · `const MIN_REF_INK: f64 = 0.001; // 0.1% of the canvas`

```
// 0.1% of the canvas
```

## L172-173 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// What one reftest came out as. `Skip` is a corpus problem (missing reference
/// file), not a result — it stays out of the tally, as it always has.
```

## L200-201 · `struct Group {`

```
/// One reference and every test that points at it. Rendering the reference once
/// per group instead of once per test is where the duplicate work goes.
```

## L207 · `fn run_group(g: &Group, root: &Path, dump: Option<&str>, out: &mut Vec<Res>) {`

```
/// Run one group: render the reference once, then each test against it.
```

## L222-224 · `if ink_fraction(&ra) < MIN_REF_INK {`

```
// Guard: if the reference renders (near-)blank, we can't tell "correct"
// from "unrendered" — mark INCONCLUSIVE instead of a false PASS. The test
// side is not even rendered then; nothing would be done with it.
```

## L236-242 · `let name = t.file_name().unwrap().to_str().unwrap().replace('.', "_");`

```
// **Der ganze Dateiname, nicht der Stamm.** `…-001.html` und
// `…-001.xht` sind ZWEI Tests im selben Ordner, und mit dem Stamm
// schrieben sie dieselbe Datei: die zweite ueberschrieb die
// erste, und wer danach hinsah, verglich stillschweigend den
// falschen Test. Genau so ist mir eine Stunde an
// `margin-collapse-min-height-001` vergangen — die Bilder zeigten
// eine andere Seite, und nichts sagte es.
```

## L252-255 · `fn family(rel: &str) -> String {`

```
/// A test's FAMILY: its name with the trailing numbering stripped
/// (`CSS2/margin-collapse-042.xht` → `CSS2/margin-collapse`). Counting failures
/// by family rather than by suite is what surfaces a single missing lever —
/// the biggest suite always looks like the biggest problem.
```

## L260 · `let name = strip_numbering(strip_numbering(name));`

```
// Twice, so `-004a` loses both the letter-suffixed number and any second one.
```

## L268 · `if e >= 2 && b[e - 1].is_ascii_lowercase() && b[e - 2].is_ascii_digit() {`

```
// A single trailing variant letter (`-004a`) belongs to the number.
```

## L277 · `return s; // no number to strip — leave the name alone`

```
// no number to strip — leave the name alone
```

## L285-287 · `fn report_baseline(results: &[Res], baseline: &Path) {`

```
/// Compare against the committed baseline and print the DELTA by name. The
/// total alone never says which side moved: a correct feature routinely makes
/// a *reference* render for the first time, and the honest score dips.
```

## L327-329 · `fn report_census(results: &[Res]) {`

```
/// Rank the remaining failures so the next lever can be *queried* rather than
/// guessed at. Two views, because they answer different questions: which family
/// is biggest, and which family is one detail away from green.
```

## L374-375 · `let root = match std::env::var("WPT_DIR") {`

```
// Default corpus lives in-repo; WPT_DIR overrides it (e.g. a large vetted
// scratch corpus) so we can measure a broad baseline without committing.
```

## L387 · `if let Ok(f) = std::env::var("WPT_FILTER") {`

```
// WPT_FILTER=<substr> runs only the matching tests, to iterate one feature.
```

## L392 · `let mut groups: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();`

```
// Group by reference. A test with no `rel=match` is not a reftest at all.
```

## L401-402 · `groups.sort_by(|a, b| b.tests.len().cmp(&a.tests.len()).then(a.ref_path.cmp(&b.ref_path)));`

```
// Longest-processing-time first: hand the 228-test groups out before the
// singletons, or one thread finishes minutes after the rest.
```

## L428 · `let mut results = sink.into_inner().unwrap();`

```
// Sorted, so two logs diff cleanly and the per-test lines read as before.
```

## L463-464 · `eprintln!("  [wpt] (filtered run — baseline comparison skipped)");`

```
// A filtered run only saw part of the corpus — a delta against the full
// baseline would read as thousands of vanished tests.
```

