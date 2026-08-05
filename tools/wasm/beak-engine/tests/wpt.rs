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
//!
//! The run is the project's tempo — every decision waits on this number — so it
//! is built to be cheap: tests are GROUPED BY REFERENCE (228 of them share
//! `ref-if-there-is-no-red.xht`, and 2605 of 5736 reference renders were pure
//! duplicate work), and the groups are spread over every core.
//!
//! `WPT_FILTER=<substr>`  narrow to one feature · `WPT_DUMP=<dir>` write BMPs ·
//! `WPT_JOBS=<n>` thread count · `WPT_BLESS=1` rewrite the baseline.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

/// Write a BGRA buffer as a 24-bit bottom-up BMP, so a failing reftest can be
/// looked at instead of guessed about (`WPT_DUMP=<dir>`).
fn write_bmp(path: &Path, buf: &[u8], w: u32, h: u32) {
    let row = (w * 3).div_ceil(4) * 4;
    let mut bmp = Vec::with_capacity(54 + (row * h) as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(54 + row * h).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&w.to_le_bytes());
    bmp.extend_from_slice(&h.to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        let mut n = 0;
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            bmp.extend_from_slice(&[buf[o], buf[o + 1], buf[o + 2]]);
            n += 3;
        }
        while n < row {
            bmp.push(0);
            n += 1;
        }
    }
    let _ = fs::write(path, &bmp);
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

/// Pass threshold: reftests are near-exact; allow a hair for AA/rounding.
const PASS_MAX_DIFF: f64 = 0.005; // ≤0.5% of pixels may differ
/// A reference must render at least this much non-white ink to be a conclusive
/// comparison — otherwise a blank test trivially "matches" it.
const MIN_REF_INK: f64 = 0.001; // 0.1% of the canvas

/// What one reftest came out as. `Skip` is a corpus problem (missing reference
/// file), not a result — it stays out of the tally, as it always has.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    Inconclusive,
    Skip,
}

impl Outcome {
    fn tag(self) -> &'static str {
        match self {
            Outcome::Pass => "PASS",
            Outcome::Fail => "FAIL",
            Outcome::Inconclusive => "INCONCLUSIVE",
            Outcome::Skip => "SKIP",
        }
    }
}

struct Res {
    rel: String,
    out: Outcome,
    diff: f64,
    note: String,
}

/// One reference and every test that points at it. Rendering the reference once
/// per group instead of once per test is where the duplicate work goes.
struct Group {
    ref_path: PathBuf,
    tests: Vec<PathBuf>,
}

/// Run one group: render the reference once, then each test against it.
fn run_group(g: &Group, root: &Path, dump: Option<&str>, out: &mut Vec<Res>) {
    let rel_of = |p: &Path| p.strip_prefix(root).unwrap().to_str().unwrap().to_string();
    let Ok(ref_html) = fs::read_to_string(&g.ref_path) else {
        for t in &g.tests {
            out.push(Res {
                rel: rel_of(t),
                out: Outcome::Skip,
                diff: 0.0,
                note: format!("ref not found: {}", g.ref_path.display()),
            });
        }
        return;
    };
    let ra = render(&ref_html);
    // Guard: if the reference renders (near-)blank, we can't tell "correct"
    // from "unrendered" — mark INCONCLUSIVE instead of a false PASS. The test
    // side is not even rendered then; nothing would be done with it.
    if ink_fraction(&ra) < MIN_REF_INK {
        for t in &g.tests {
            out.push(Res { rel: rel_of(t), out: Outcome::Inconclusive, diff: 0.0, note: String::new() });
        }
        return;
    }
    for t in &g.tests {
        let html = fs::read_to_string(t).unwrap_or_default();
        let ta = render(&html);
        let d = diff_fraction(&ta, &ra);
        if let Some(dir) = dump {
            let stem = t.file_stem().unwrap().to_str().unwrap();
            write_bmp(&Path::new(dir).join(format!("{stem}-test.bmp")), &ta, W, H);
            write_bmp(&Path::new(dir).join(format!("{stem}-ref.bmp")), &ra, W, H);
        }
        let out_kind = if d <= PASS_MAX_DIFF { Outcome::Pass } else { Outcome::Fail };
        out.push(Res { rel: rel_of(t), out: out_kind, diff: d, note: String::new() });
    }
}

/// A test's FAMILY: its name with the trailing numbering stripped
/// (`CSS2/margin-collapse-042.xht` → `CSS2/margin-collapse`). Counting failures
/// by family rather than by suite is what surfaces a single missing lever —
/// the biggest suite always looks like the biggest problem.
fn family(rel: &str) -> String {
    let (suite, rest) = rel.split_once('/').unwrap_or(("", rel));
    let name = rest.rsplit('/').next().unwrap_or(rest);
    let name = name.strip_suffix(".html").or_else(|| name.strip_suffix(".xht")).unwrap_or(name);
    // Twice, so `-004a` loses both the letter-suffixed number and any second one.
    let name = strip_numbering(strip_numbering(name));
    format!("{suite}/{name}")
}

fn strip_numbering(s: &str) -> &str {
    let b = s.as_bytes();
    let mut e = b.len();
    // A single trailing variant letter (`-004a`) belongs to the number.
    if e >= 2 && b[e - 1].is_ascii_lowercase() && b[e - 2].is_ascii_digit() {
        e -= 1;
    }
    let after_digits = e;
    while e > 0 && b[e - 1].is_ascii_digit() {
        e -= 1;
    }
    if e == after_digits {
        return s; // no number to strip — leave the name alone
    }
    if e > 0 && (b[e - 1] == b'-' || b[e - 1] == b'_') {
        e -= 1;
    }
    &s[..e]
}

/// Compare against the committed baseline and print the DELTA by name. The
/// total alone never says which side moved: a correct feature routinely makes
/// a *reference* render for the first time, and the honest score dips.
fn report_baseline(results: &[Res], baseline: &Path) {
    let Ok(text) = fs::read_to_string(baseline) else {
        eprintln!("  [wpt] no baseline at {} — run with WPT_BLESS=1 to write one", baseline.display());
        return;
    };
    let old: BTreeMap<&str, &str> =
        text.lines().filter_map(|l| l.split_once('\t')).map(|(s, n)| (n, s)).collect();
    let (mut gained, mut lost, mut fresh) = (Vec::new(), Vec::new(), 0usize);
    for r in results {
        if r.out == Outcome::Skip {
            continue;
        }
        match old.get(r.rel.as_str()) {
            None => fresh += 1,
            Some(&was) => {
                let now = r.out.tag();
                if was != "PASS" && now == "PASS" {
                    gained.push(r);
                } else if was == "PASS" && now != "PASS" {
                    lost.push(r);
                }
            }
        }
    }
    let seen: std::collections::BTreeSet<&str> = results.iter().map(|r| r.rel.as_str()).collect();
    let vanished = old.keys().filter(|k| !seen.contains(*k)).count();

    eprintln!("  [wpt] ----- vs baseline: +{} / -{} -----", gained.len(), lost.len());
    for r in &gained {
        eprintln!("  [wpt]   +  {:>6.2}%  {}", r.diff * 100.0, r.rel);
    }
    for r in &lost {
        eprintln!("  [wpt]   -  {:>6.2}%  {}  ({})", r.diff * 100.0, r.rel, r.out.tag());
    }
    if fresh > 0 || vanished > 0 {
        eprintln!("  [wpt]   ({fresh} not in baseline, {vanished} baseline entries gone — bless to resync)");
    }
}

/// Rank the remaining failures so the next lever can be *queried* rather than
/// guessed at. Two views, because they answer different questions: which family
/// is biggest, and which family is one detail away from green.
fn report_census(results: &[Res]) {
    let mut fam: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for r in results.iter().filter(|r| r.out == Outcome::Fail) {
        fam.entry(family(&r.rel)).or_default().push(r.diff * 100.0);
    }
    for v in fam.values_mut() {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }

    let mut by_size: Vec<_> = fam.iter().collect();
    by_size.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    eprintln!("  [wpt] ----- biggest failing families (median diff — high means real layout work) -----");
    for (k, v) in by_size.iter().take(15) {
        eprintln!("  [wpt]   {:4}  med {:6.2}%  max {:6.2}%   {k}", v.len(), v[v.len() / 2], v[v.len() - 1]);
    }

    let mut near: Vec<_> = fam.iter().filter(|(_, v)| v.len() >= 4 && v[v.len() - 1] < 2.0).collect();
    near.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    let total: usize = near.iter().map(|(_, v)| v.len()).sum();
    eprintln!("  [wpt] ----- NEAR MISSES: >=4 failures, none over 2% ({total} tests, one detail each) -----");
    for (k, v) in near.iter().take(15) {
        eprintln!("  [wpt]   {:4}  max {:5.2}%   {k}", v.len(), v[v.len() - 1]);
    }

    let bands = [(0.0, 1.0), (1.0, 2.0), (2.0, 5.0), (5.0, 25.0), (25.0, 101.0)];
    let line: Vec<String> = bands
        .iter()
        .map(|(lo, hi)| {
            let n = results
                .iter()
                .filter(|r| r.out == Outcome::Fail)
                .filter(|r| {
                    let d = r.diff * 100.0;
                    d >= *lo && d < *hi
                })
                .count();
            format!("{lo:.0}-{hi:.0}%: {n}")
        })
        .collect();
    eprintln!("  [wpt] failures by diff — {}", line.join("  ·  "));
}

#[test]
fn wpt_reftests() {
    // Default corpus lives in-repo; WPT_DIR overrides it (e.g. a large vetted
    // scratch corpus) so we can measure a broad baseline without committing.
    let root = match std::env::var("WPT_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt"),
    };
    if !root.exists() {
        eprintln!("  [wpt] no tests/wpt dir — nothing to run");
        return;
    }
    let mut tests = Vec::new();
    collect_tests(&root, &mut tests);
    tests.sort();
    // WPT_FILTER=<substr> runs only the matching tests, to iterate one feature.
    if let Ok(f) = std::env::var("WPT_FILTER") {
        tests.retain(|t| t.to_str().is_some_and(|s| s.contains(&f)));
    }

    // Group by reference. A test with no `rel=match` is not a reftest at all.
    let mut groups: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for t in tests {
        let html = fs::read_to_string(&t).unwrap_or_default();
        let Some(href) = match_href(&html) else { continue };
        groups.entry(t.parent().unwrap().join(&href)).or_default().push(t);
    }
    let mut groups: Vec<Group> =
        groups.into_iter().map(|(ref_path, tests)| Group { ref_path, tests }).collect();
    // Longest-processing-time first: hand the 228-test groups out before the
    // singletons, or one thread finishes minutes after the rest.
    groups.sort_by(|a, b| b.tests.len().cmp(&a.tests.len()).then(a.ref_path.cmp(&b.ref_path)));

    let jobs = std::env::var("WPT_JOBS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(1)
        .max(1);
    let dump = std::env::var("WPT_DUMP").ok();

    let queue = Mutex::new(groups.into_iter());
    let sink: Mutex<Vec<Res>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| {
                let mut mine = Vec::new();
                loop {
                    let Some(g) = queue.lock().unwrap().next() else { break };
                    run_group(&g, &root, dump.as_deref(), &mut mine);
                }
                sink.lock().unwrap().append(&mut mine);
            });
        }
    });

    // Sorted, so two logs diff cleanly and the per-test lines read as before.
    let mut results = sink.into_inner().unwrap();
    results.sort_by(|a, b| a.rel.cmp(&b.rel));

    let (mut pass, mut fail, mut inconclusive) = (0usize, 0usize, 0usize);
    for r in &results {
        match r.out {
            Outcome::Pass => pass += 1,
            Outcome::Fail => fail += 1,
            Outcome::Inconclusive => inconclusive += 1,
            Outcome::Skip => {}
        }
        match r.out {
            Outcome::Skip => eprintln!("  [wpt]  SKIP  {} ({})", r.rel, r.note),
            Outcome::Inconclusive => eprintln!("  [wpt]   ----   INCONCLUSIVE (ref blank)  {}", r.rel),
            _ => eprintln!("  [wpt] {:>6.2}% diff  {}  {}", r.diff * 100.0, r.out.tag(), r.rel),
        }
    }
    eprintln!(
        "  [wpt] ===== {pass} pass / {fail} fail / {inconclusive} inconclusive  (of {} reftests, {jobs} threads) =====",
        pass + fail + inconclusive
    );

    let baseline = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt-baseline.tsv");
    if std::env::var("WPT_BLESS").is_ok() {
        let mut text = String::new();
        for r in results.iter().filter(|r| r.out != Outcome::Skip) {
            text.push_str(r.out.tag());
            text.push('\t');
            text.push_str(&r.rel);
            text.push('\n');
        }
        let _ = fs::write(&baseline, text);
        eprintln!("  [wpt] baseline written: {}", baseline.display());
    } else if std::env::var("WPT_FILTER").is_ok() {
        // A filtered run only saw part of the corpus — a delta against the full
        // baseline would read as thousands of vanished tests.
        eprintln!("  [wpt] (filtered run — baseline comparison skipped)");
    } else {
        report_baseline(&results, &baseline);
    }
    report_census(&results);
}
