//! WPT reftest oracle — the objective CSS-fidelity gate (BROWSER.md §10).
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

use std::fs;
use std::path::{Path, PathBuf};

use beak_engine::{Engine, Rgb, Theme};

const W: u32 = 800;
const H: u32 = 600;

/// A white-background light theme (WPT reftests assume a white canvas).
fn light() -> Theme {
    Theme {
        bg: Rgb(255, 255, 255),
        text: Rgb(0, 0, 0),
        heading: Rgb(0, 0, 0),
        link: Rgb(0, 0, 238),
        muted: Rgb(96, 96, 96),
        rule: Rgb(128, 128, 128),
    }
}

/// Render an HTML document (its inline `<style>` is the author sheet) to BGRA.
fn render(html: &str) -> Vec<u8> {
    let mut eng = Engine::new();
    eng.set_theme(light());
    let lay = eng.layout_ext(html, "", W);
    let mut buf = vec![0u8; (W * H * 4) as usize];
    eng.paint(&lay, W, H, 0, &mut buf);
    buf
}

/// The `href` of the `<link rel="match" href="…">` in a reftest, if any.
fn match_href(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(i) = rest.find("<link") {
        let tag_end = rest[i..].find('>').map(|e| i + e + 1).unwrap_or(rest.len());
        let tag = &rest[i..tag_end];
        let is_match = tag.contains("rel=\"match\"") || tag.contains("rel='match'") || tag.contains("rel=match");
        if is_match {
            for q in ['"', '\''] {
                let pat = format!("href={q}");
                if let Some(h) = tag.find(&pat) {
                    let start = h + pat.len();
                    if let Some(end) = tag[start..].find(q) {
                        return Some(tag[start..start + end].to_string());
                    }
                }
            }
        }
        rest = &rest[tag_end..];
    }
    None
}

/// Fraction of pixels differing beyond a small per-channel tolerance (accounts
/// for anti-aliasing at glyph/box edges shared by test + ref).
fn diff_fraction(a: &[u8], b: &[u8]) -> f64 {
    const TOL: i32 = 20;
    let mut differ = 0usize;
    let px = a.len() / 4;
    for i in 0..px {
        let o = i * 4;
        let d = (a[o] as i32 - b[o] as i32).abs().max(
            (a[o + 1] as i32 - b[o + 1] as i32).abs().max((a[o + 2] as i32 - b[o + 2] as i32).abs()),
        );
        if d > TOL {
            differ += 1;
        }
    }
    differ as f64 / px as f64
}

/// Fraction of pixels that are NOT the white canvas — the reference's "ink".
/// If a reference renders (near-)blank, the reftest is INCONCLUSIVE: a blank
/// test would trivially "match" a blank reference, so equality proves nothing.
/// This guards against false passes on features we don't render yet (e.g. an
/// empty `<div>` sized only by `height` that collapses to nothing).
fn ink_fraction(buf: &[u8]) -> f64 {
    const TOL: i32 = 20;
    let mut ink = 0usize;
    let px = buf.len() / 4;
    for i in 0..px {
        let o = i * 4;
        let d = (255 - buf[o] as i32)
            .abs()
            .max((255 - buf[o + 1] as i32).abs().max((255 - buf[o + 2] as i32).abs()));
        if d > TOL {
            ink += 1;
        }
    }
    ink as f64 / px as f64
}

/// Collect every reftest (a `*.html` that is not a `-ref.html`) under `dir`.
fn collect_tests(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_tests(&p, out);
        } else if let Some(n) = p.file_name().and_then(|s| s.to_str()) {
            // WPT reftests come as .html AND .xht (XHTML). Exclude the *-ref.*
            // reference files (they're loaded via each test's rel=match href).
            let is_ref = n.ends_with("-ref.html") || n.ends_with("-ref.xht");
            let is_test = n.ends_with(".html") || n.ends_with(".xht");
            if is_test && !is_ref {
                out.push(p);
            }
        }
    }
}

#[test]
fn wpt_reftests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt");
    if !root.exists() {
        eprintln!("  [wpt] no tests/wpt dir — nothing to run");
        return;
    }
    let mut tests = Vec::new();
    collect_tests(&root, &mut tests);
    tests.sort();

    // Pass threshold: reftests are near-exact; allow a hair for AA/rounding.
    const PASS_MAX_DIFF: f64 = 0.005; // ≤0.5% of pixels may differ
    // A reference must render at least this much non-white ink to be a
    // conclusive comparison — otherwise a blank test trivially "matches" it.
    const MIN_REF_INK: f64 = 0.001; // 0.1% of the canvas
    let (mut pass, mut fail, mut inconclusive) = (0usize, 0usize, 0usize);
    for t in &tests {
        let html = fs::read_to_string(t).unwrap_or_default();
        let Some(href) = match_href(&html) else { continue };
        let ref_path = t.parent().unwrap().join(&href);
        let Ok(ref_html) = fs::read_to_string(&ref_path) else {
            eprintln!("  [wpt]  SKIP  {} (ref not found: {href})", t.file_name().unwrap().to_str().unwrap());
            continue;
        };
        let rel = t.strip_prefix(&root).unwrap().to_str().unwrap();
        let ta = render(&html);
        let ra = render(&ref_html);
        // Guard: if the reference renders (near-)blank, we can't tell "correct"
        // from "unrendered" — mark INCONCLUSIVE instead of a false PASS.
        if ink_fraction(&ra) < MIN_REF_INK {
            inconclusive += 1;
            eprintln!("  [wpt]   ----   INCONCLUSIVE (ref blank)  {rel}");
            continue;
        }
        let d = diff_fraction(&ta, &ra);
        let ok = d <= PASS_MAX_DIFF;
        if ok {
            pass += 1;
        } else {
            fail += 1;
        }
        eprintln!("  [wpt] {:>6.2}% diff  {}  {rel}", d * 100.0, if ok { "PASS" } else { "FAIL" });
    }
    eprintln!(
        "  [wpt] ===== {pass} pass / {fail} fail / {inconclusive} inconclusive  (of {} reftests) =====",
        pass + fail + inconclusive
    );
}
