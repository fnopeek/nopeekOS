//! test262 as a parse oracle: a language score independent of evaluation.
//!
//! test262 states for each file whether it is valid JavaScript:
//!
//! - `negative: { phase: parse }` → the parser must reject.
//! - everything else → the parser must accept. Including `phase: resolution`
//!   and `phase: runtime`: those are syntactically fine and fail later.
//!
//! That is a complete verdict on the grammar without an interpreter.
//!
//! The corpus is not in the repo. Path via `TEST262=`; without the variable
//! the test skips itself and says how to enable it.
//!
//!   TEST262=~/…/tools/test262-upstream cargo test --release \
//!     --manifest-path tools/wasm/beak-engine/Cargo.toml --test test262 -- --nocapture
//!
//! `T262_FILTER=<substr>` narrows · `T262_SHOW=<n>` shows n failures.
//!
//! Compared against `tools/test262/out/baseline-v8.json`: a test we fail and
//! V8 passes is our gap. The own percentage alone says little, since test262
//! runs ahead of the engines.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Directories outside the target; the same policy as
/// `tools/test262/subset.json`, reduced to what matters for parsing.
const SKIP_DIRS: &[&str] = &["intl402", "staging"];

/// Only syntax proposals we deliberately do not build. `Temporal` parses fine
/// (only builtins are missing), so it is not excluded here; only what changes
/// the grammar itself is.
const SKIP_FEATURES: &[&str] = &[
    "decorators",
    "explicit-resource-management",   // `using x = …`
    "import-attributes",
    "import-assertions",
    "source-phase-imports",
    "import-defer",
];

/// The second denominator, for the execution run; the same policy as
/// `tools/test262/subset.json`.
///
/// It is longer than the parse list, which is consistent: `Temporal` parses
/// fine, but only an engine that implements it can execute it, and we
/// deliberately do not.
const SKIP_FEATURES_EXEC: &[&str] = &[
    "Temporal", "Intl.Era-monthcode", "explicit-resource-management", "decorators",
    "Atomics", "Atomics.pause", "SharedArrayBuffer", "import-assertions",
    "import-attributes", "source-phase-imports", "iterator-sequencing",
    "Math.sumPrecise", "uint8array-base64", "ShadowRealm", "await-dictionary",
    "joint-iteration", "iterator-chunking", "iterator-includes", "import-defer",
    "immutable-arraybuffer", "error-stack-accessor",
];

/// The `$DONE` an `async` test calls; our version of
/// `harness/doneprintHandle.js`.
///
/// A truthy reason is a failure; anything else (no argument, `undefined`,
/// `null`) is success, matching the `if (error)` of the harness file. A
/// second call is itself a failure: the flag's spec says "the sole
/// asynchronous test of a file", and a test that finishes twice fired one
/// callback too many.
const DONE_SRC: &str = r#"
var __t262_done = 0, __t262_err = undefined;
function $DONE(error) {
  if (__t262_done !== 0) { __t262_done = 3; return; }
  if (error) { __t262_done = 2; __t262_err = error; } else { __t262_done = 1; }
}
"#;

/// Drains the job queue and reads what `$DONE` reported.
///
/// Called only for `async` tests: for any other, running the queue would be
/// a second semantics; an ordinary test is done when its last statement ran.
fn async_outcome(
    s: &mut beak_engine::js::Session,
    r: Result<(), String>,
) -> Result<(), String> {
    use beak_engine::js::value::Value;
    // A throw in the script itself stays the throw; `$DONE` was never reached.
    r.as_ref().map_err(|e| e.clone())?;
    beak_engine::js::promise::run_jobs(&mut s.interp);
    let g = s.interp.realm.global.clone();
    let read = |k: &str| g.borrow().get_own(k).and_then(|p| p.value.clone());
    match read("__t262_done") {
        Some(Value::Num(n)) if n == 1.0 => Ok(()),
        Some(Value::Num(n)) if n == 3.0 => Err(String::from("$DONE zweimal gerufen")),
        Some(Value::Num(n)) if n == 2.0 => {
            let e = read("__t262_err").unwrap_or(Value::Undefined);
            let name = s.interp.get(&e, "name").ok().and_then(|v| s.interp.to_string(&v).ok());
            let msg = s.interp.get(&e, "message").ok().and_then(|v| s.interp.to_string(&v).ok());
            Err(match (name, msg) {
                (Some(n), Some(m)) if !m.is_empty() => format!("{n}: {m}"),
                (Some(n), _) if !n.is_empty() => n.to_string(),
                _ => s.interp.to_string(&e).map(|v| v.to_string())
                        .unwrap_or_else(|_| String::from("$DONE mit einem Grund")),
            })
        }
        // The most common legitimate failure, not a runner bug: the chain
        // stalled somewhere, usually at a missing feature. It must be named
        // so, or the reader looks for the error in the harness.
        _ => Err(String::from("$DONE wurde nie gerufen")),
    }
}

#[derive(Default)]
struct Meta {
    description: String,
    flags: Vec<String>,
    includes: Vec<String>,
    features: Vec<String>,
    negative_parse: bool,
    negative_other: bool,
}

fn frontmatter(src: &str) -> Meta {
    let mut m = Meta::default();
    let Some(a) = src.find("/*---") else { return m };
    let Some(b) = src[a..].find("---*/") else { return m };
    let y = &src[a + 5..a + b];
    let list = |key: &str| -> Vec<String> {
        for line in y.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix(key) {
                let rest = rest.trim();
                if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    return inner.split(',').map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()).collect();
                }
            }
        }
        Vec::new()
    };
    m.flags = list("flags:");
    m.features = list("features:");
    m.includes = list("includes:");
    for line in y.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("description:") {
            m.description = rest.trim().to_string();
            break;
        }
    }
    if let Some(np) = y.find("negative:") {
        let tail = &y[np..];
        if tail.contains("phase: parse") { m.negative_parse = true; } else { m.negative_other = true; }
    }
    m
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, out); }
        else if p.extension().is_some_and(|x| x == "js") {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            if !n.contains("_FIXTURE") { out.push(p); }
        }
    }
}

#[test]
fn test262_parse() {
    let Ok(root) = std::env::var("TEST262") else {
        eprintln!("[test262] uebersprungen — setze TEST262=<pfad zum test262-checkout>.");
        eprintln!("[test262] Der Korpus liegt bewusst neben dem Repo (273 MB); `tools/test262/README.md` sagt wie.");
        return;
    };
    let tests = Path::new(&root).join("test");
    assert!(tests.is_dir(), "TEST262 zeigt nicht auf einen test262-Checkout: {}", tests.display());
    let filter = std::env::var("T262_FILTER").unwrap_or_default();
    let show: usize = std::env::var("T262_SHOW").ok().and_then(|s| s.parse().ok()).unwrap_or(25);

    let mut files = Vec::new();
    walk(&tests, &mut files);
    files.sort();

    let (mut run, mut pass) = (0usize, 0usize);
    let (mut skip_dir, mut skip_feat) = (0usize, 0usize);
    // Counted separately because they weigh very differently: a file we wrongly
    // reject costs the whole page; one we wrongly accept is a missing early
    // error, annoying but the page runs.
    let (mut n_reject, mut n_accept) = (0usize, 0usize);
    let mut accept_fam: std::collections::BTreeMap<String, usize> = Default::default();
    // Always counted, collected only up to the cap, or the report would show
    // the cap instead of the count.
    let mut wrong_reject: Vec<(String, String)> = Vec::new();
    let mut wrong_accept: Vec<String> = Vec::new();

    for p in &files {
        let rel = p.strip_prefix(&tests).unwrap().to_string_lossy().replace('\\', "/");
        if !filter.is_empty() && !rel.contains(&filter) { continue; }
        if SKIP_DIRS.iter().any(|d| rel.starts_with(d)) { skip_dir += 1; continue; }
        let Ok(src) = fs::read_to_string(p) else { continue };
        let m = frontmatter(&src);
        if m.features.iter().any(|f| SKIP_FEATURES.contains(&f.as_str())) { skip_feat += 1; continue; }

        let module = m.flags.iter().any(|f| f == "module");
        let raw = m.flags.iter().any(|f| f == "raw");
        let modes: &[bool] = if raw || module { &[false] }
            else if m.flags.iter().any(|f| f == "onlyStrict") { &[true] }
            else if m.flags.iter().any(|f| f == "noStrict") { &[false] }
            else { &[true, false] };

        for &strict in modes {
            run += 1;
            let text = if strict && !raw {
                let mut s = String::from("\"use strict\";\n");
                s.push_str(&src);
                s
            } else { src.clone() };

            let got = beak_engine::js::parses(&text, module);
            let want_reject = m.negative_parse;
            match (want_reject, got) {
                (false, Ok(())) | (true, Err(_)) => pass += 1,
                (false, Err(e)) => {
                    n_reject += 1;
                    if wrong_reject.len() < 5000 {
                        wrong_reject.push((format!("{rel}{}", if strict { " [strict]" } else { "" }), e.msg));
                    }
                }
                (true, Ok(())) => {
                    n_accept += 1;
                    // Grouped by rule, not individually or by directory:
                    // thousands of paths are no information. test262 names
                    // the rule in `description`; the part before the first
                    // colon/parenthesis carries it.
                    let d = m.description.trim_start_matches(['|', '>', ' ']);
                    let rule: String = d.split(['(', ':']).next().unwrap_or(d)
                        .chars().take(64).collect();
                    let rule = if rule.trim().is_empty() {
                        rel.split('/').take(3).collect::<Vec<_>>().join("/")
                    } else { rule.trim().to_string() };
                    *accept_fam.entry(rule).or_insert(0usize) += 1;
                    if wrong_accept.len() < 40 {
                        wrong_accept.push(format!("{rel}{}", if strict { " [strict]" } else { "" }));
                    }
                }
            }
        }
    }

    let pct = |n: usize, d: usize| if d == 0 { 0.0 } else { 100.0 * n as f64 / d as f64 };
    eprintln!("\n── test262, PARSE-Orakel ──");
    eprintln!("   {} Dateien gesehen, {} nach Verzeichnis / {} nach Syntax-Feature uebergangen",
        files.len(), skip_dir, skip_feat);
    eprintln!("   bestanden {pass} von {run} Varianten = {:.2} %", pct(pass, run));
    eprintln!("   faelschlich ABGELEHNT:  {n_reject:5}   (kostet die Seite — das ist die Zahl)");
    eprintln!("   faelschlich ANGENOMMEN: {n_accept:5}   (fehlender Fruehfehler — die Seite laeuft trotzdem)");

    // Grouped by reason, with one example path per reason: which message how
    // often, and where to look, says what to build next.
    let mut by_msg: std::collections::BTreeMap<&str, (usize, &str)> = Default::default();
    for (path, msg) in &wrong_reject {
        let e = by_msg.entry(msg.as_str()).or_insert((0, path.as_str()));
        e.0 += 1;
    }
    let mut v: Vec<_> = by_msg.into_iter().collect();
    v.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    eprintln!("\n   Warum wir ablehnen, was gueltig ist:");
    for (msg, (n, ex)) in v.iter().take(15) { eprintln!("      {n:5}  {msg}\n             z.B. {ex}"); }

    eprintln!("\n   Erste {} faelschlich abgelehnte:", show.min(wrong_reject.len()));
    for (p, m) in wrong_reject.iter().take(show) { eprintln!("      {p}\n         {m}"); }

    let mut fams: Vec<_> = accept_fam.into_iter().collect();
    fams.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    eprintln!("\n   Fehlende Fruehfehler, nach REGEL:");
    for (f, n) in fams.iter().take(28) { eprintln!("      {n:5}  {f}"); }
}

/// The second score, and for beak the more important one: does what the
/// target corpus actually ships parse?
///
/// test262 measures the language, this measures the web. The scripts are
/// those Chromium parsed while loading the target pages (`tools/jsscope/js/`,
/// stored by `measure.mjs`): real, shipped, minified code, not test cases.
///
///   JSCORPUS=~/…/tools/jsscope/js cargo test --release \
///     --manifest-path tools/wasm/beak-engine/Cargo.toml --test test262 -- --nocapture
#[test]
fn corpus_parse() {
    let Ok(root) = std::env::var("JSCORPUS") else {
        eprintln!("[korpus] uebersprungen — setze JSCORPUS=<tools/jsscope/js>.");
        return;
    };
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files);
    files.sort();
    if files.is_empty() { eprintln!("[korpus] keine Skripte unter {root}"); return; }

    let (mut ok, mut bytes_ok, mut bytes_all) = (0usize, 0usize, 0usize);
    let mut fails: Vec<(String, String, usize)> = Vec::new();
    let mut by_page: std::collections::BTreeMap<String, (usize, usize)> = Default::default();

    for p in &files {
        let Ok(src) = fs::read_to_string(p) else { continue };
        let page = p.parent().and_then(|d| d.file_name())
            .map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let e = by_page.entry(page).or_insert((0, 0));
        e.1 += 1;
        bytes_all += src.len();
        // A shipped script may be a script or a module, and the file does not
        // say. Try both: only if neither parses is it a gap.
        if beak_engine::js::parses(&src, false).is_ok()
            || beak_engine::js::parses(&src, true).is_ok() {
            ok += 1; e.0 += 1; bytes_ok += src.len();
        } else {
            // The error from the module attempt. `or_else` yields the second
            // error, not the first, and the script error for a module is
            // always just "unexpected keyword" at `export`, which says nothing.
            let err = beak_engine::js::parses(&src, true).unwrap_err();
            if fails.len() < 40 {
                fails.push((p.strip_prefix(&root).unwrap().to_string_lossy().to_string(),
                    err.msg, err.at));
            }
        }
    }
    eprintln!("\n── Zielkorpus: parst der ausgelieferte Code? ──");
    eprintln!("   {ok} von {} Skripten = {:.1} %   ({:.1} % der Bytes)",
        files.len(), 100.0 * ok as f64 / files.len() as f64,
        100.0 * bytes_ok as f64 / bytes_all.max(1) as f64);
    eprintln!("\n   Nach Seite:");
    for (page, (o, n)) in &by_page {
        let mark = if o == n { "   " } else { " ! " };
        eprintln!("     {mark}{o:3}/{n:3}  {page}");
    }
    if !fails.is_empty() {
        eprintln!("\n   Was nicht parst:");
        for (f, m, at) in fails.iter().take(20) { eprintln!("      {f} @{at}\n         {m}"); }
    }
}


/// The execution run: not only "is this valid syntax" but "does it do the
/// right thing".
///
/// Compared against `tools/test262/out/baseline-v8.json`. The own score alone
/// says little; the difference says everything.
///
///   TEST262=<…> cargo test --release --test test262 exec -- --nocapture
///
/// `T262_FILTER` narrows · `T262_SHOW` shows n failures.
#[test]
fn test262_exec() {
    let Ok(root) = std::env::var("TEST262") else {
        eprintln!("[test262] uebersprungen — setze TEST262=<pfad zum test262-checkout>.");
        return;
    };
    let tests = Path::new(&root).join("test");
    let harness = Path::new(&root).join("harness");
    if !tests.is_dir() { eprintln!("[test262] kein Checkout unter {}", tests.display()); return; }
    let filter = std::env::var("T262_FILTER").unwrap_or_default();
    let show: usize = std::env::var("T262_SHOW").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    // `T262_FAILLIST=<file>` writes every failed name there.
    let faillist = std::env::var("T262_FAILLIST").ok();
    let mut all_fails: Vec<String> = Vec::new();
    // How much already runs on the bytecode machine.
    let (mut vm_ran, mut vm_declined) = (0u64, 0u64);
    let (mut vm_calls, mut vm_calls_slow) = (0u64, 0u64);
    let mut vm_calls_native = 0u64;
    let mut by_decline: BTreeMap<&'static str, u64> = BTreeMap::new();
    // The same count for function bodies: a body can decline (generators,
    // async) without its program declining, and would otherwise be invisible.
    let mut by_fdecline: BTreeMap<&'static str, u64> = BTreeMap::new();
    // The strict-mode probe (`--features strict-probe`). Counted per variant
    // and by outcome: only that says how many failures passed one of these
    // points; the tests' flags do not.
    #[cfg(feature = "strict-probe")]
    let mut probe_fail = [0u64; beak_engine::js::STRICT_SITES];
    #[cfg(feature = "strict-probe")]
    let mut probe_pass = [0u64; beak_engine::js::STRICT_SITES];
    #[cfg(feature = "strict-probe")]
    let (mut probe_fail_any, mut probe_pass_any) = (0u64, 0u64);
    // Which point hit which failed test, for the ranking by directory.
    #[cfg(feature = "strict-probe")]
    let mut probe_names: Vec<(String, [u32; beak_engine::js::STRICT_SITES], String)> = Vec::new();

    // Register `$262`: the runner is the host. The engine builds it only on
    // request; a page never sees it.
    beak_engine::js::test262::enable();

    let hread = |f: &str| fs::read_to_string(harness.join(f)).unwrap_or_default();
    let mut hmap: std::collections::BTreeMap<String, String> = Default::default();
    let mut hcache = |f: &str| -> String {
        hmap.entry(f.to_string()).or_insert_with(|| fs::read_to_string(harness.join(f)).unwrap_or_default()).clone()
    };
    // Parsed once, then only executed; reparsing the prelude per variant
    // would dominate the run.
    let prologue_src = format!("{}\n{}\n", hread("assert.js"), hread("sta.js"));
    let strict_src = format!("\"use strict\";\n{prologue_src}");
    let Ok(prologue) = beak_engine::js::parse(&prologue_src, false) else {
        panic!("der test262-Vorspann parst nicht — ohne ihn misst dieser Lauf nichts");
    };
    let Ok(prologue_strict) = beak_engine::js::parse(&strict_src, false) else {
        panic!("der strenge Vorspann parst nicht");
    };

    let mut files = Vec::new();
    walk(&tests, &mut files);
    files.sort();

    let (mut run, mut pass) = (0usize, 0usize);
    let (mut skip_dir, mut skip_kind, mut skip_feat) = (0usize, 0usize, 0usize);
    let mut panics = 0usize;
    let mut fails: Vec<(String, String)> = Vec::new();
    let mut slow: Vec<(u128, String)> = Vec::new();
    let trace = std::env::var("T262_TRACE").ok();
    // Phase timings inside the real run, not in a side measurement, which
    // would miss the expensive cases.
    let (mut t_read, mut t_parse, mut t_exec) = (0u128, 0u128, 0u128);
    let mut by_msg: std::collections::BTreeMap<String, (usize, String)> = Default::default();

    for p in &files {
        let rel = p.strip_prefix(&tests).unwrap().to_string_lossy().replace('\\', "/");
        if !filter.is_empty() && !rel.contains(&filter) { continue; }
        if SKIP_DIRS.iter().any(|d| rel.starts_with(d)) { skip_dir += 1; continue; }
        let Ok(src) = fs::read_to_string(p) else { continue };
        let m = frontmatter(&src);

        // Modules need a resolver, which this runner lacks; they get their own
        // line in the report, not under "passed".
        if m.flags.iter().any(|f| f == "module") { skip_kind += 1; continue; }
        let is_async = m.flags.iter().any(|f| f == "async");
        if m.features.iter().any(|f| SKIP_FEATURES_EXEC.contains(&f.as_str())) {
            skip_feat += 1; continue;
        }
        let raw = m.flags.iter().any(|f| f == "raw");
        let modes: &[bool] = if raw { &[false] }
            else if m.flags.iter().any(|f| f == "onlyStrict") { &[true] }
            else if m.flags.iter().any(|f| f == "noStrict") { &[false] }
            else { &[true, false] };

        for &strict in modes {
            run += 1;
            let mut text = String::new();
            if strict && !raw { text.push_str("\"use strict\";\n"); }
            // Harness files come from the cache rather than disk per variant.
            // `$DONE` goes before the harness files: `asyncHelpers.js` checks
            // `hasOwnProperty(globalThis, "$DONE")` and throws otherwise. The
            // stock `doneprintHandle.js` prints its result, since a
            // command-line runner has nothing else; we drive the session
            // ourselves and read it from the global object afterwards (same
            // semantics: a truthy reason is a failure).
            if is_async && !raw { text.push_str(DONE_SRC); }
            if !raw { for inc in &m.includes { text.push_str(&hcache(inc)); text.push('\n'); } }
            text.push_str(&src);

            let neg = m.negative_parse || m.negative_other;
            // Slow tests are reported immediately, with `flush`, so the line
            // stands even if the run hangs afterwards.
            // The name is written before the run, not after: a test that never
            // returns would never appear in a report afterwards. Only with
            // `T262_TRACE`, since a file per variant costs time itself.
            if let Some(mark) = &trace {
                let _ = fs::write(mark, format!("{rel}{}", if strict { " [strict]" } else { "" }));
            }
            let t_r = std::time::Instant::now();
            // A crash in the interpreter must not end the run, or a single
            // `unwrap` stops all measurement. Counted separately.
            t_read += t_r.elapsed().as_nanos();
            let t0 = std::time::Instant::now();
            let mut np = 0u128;
            let mut vm_seen = (0u64, 0u64, None);
            let mut calls_seen = (0u64, 0u64, 0u64);
            let mut fdecl: Vec<(&'static str, u64)> = Vec::new();
            #[cfg(feature = "strict-probe")]
            let mut probe = [0u32; beak_engine::js::STRICT_SITES];
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let tp = std::time::Instant::now();
                let prog = match beak_engine::js::parse(&text, false) {
                    Ok(p) => p,
                    Err(e) => return Err(format!("SyntaxError: {} @{}", e.msg, e.at)),
                };
                np = tp.elapsed().as_nanos();
                // `T262_NOVM=1` runs the same pass without the bytecode
                // machine. Diffing both failure lists checks the two machines
                // against each other.
                let mut s = if std::env::var("T262_NOVM").is_ok() {
                    beak_engine::js::Session::new_without_vm(beak_engine::js::TEST_STEPS)
                } else {
                    beak_engine::js::Session::new(beak_engine::js::TEST_STEPS)
                };
                // Only the test counts for coverage, not the prelude: it is
                // always the same and would dilute the figure.
                let r = if raw {
                    s.run(&prog)
                } else {
                    match s.run(if strict { &prologue_strict } else { &prologue }) {
                        Err(e) => Err(e),
                        Ok(()) => {
                            // Read before the test program and subtract after:
                            // the prelude hits the points itself and would
                            // otherwise colour every line the same.
                            #[cfg(feature = "strict-probe")]
                            let probe0 = s.interp.strict_probe;
                            let (a, b) = (s.interp.vm_ran, s.interp.vm_declined);
                            let (ca, cb) = (s.interp.vm_calls, s.interp.vm_calls_slow);
                            let cn = s.interp.vm_calls_native;
                            let r = s.run(&prog);
                            // An async test is only done when the job queue is
                            // empty. Without draining it no `.then` would run
                            // and every such test would report `$DONE` as
                            // never called.
                            let r = if is_async { async_outcome(&mut s, r) } else { r };
                            fdecl = s.interp.func_declines.iter()
                                .map(|(k, v)| (*k, *v)).collect();
                            #[cfg(feature = "strict-probe")]
                            for k in 0..beak_engine::js::STRICT_SITES {
                                probe[k] = s.interp.strict_probe[k] - probe0[k];
                            }
                            vm_seen = (s.interp.vm_ran - a, s.interp.vm_declined - b,
                                       s.interp.vm_decline);
                            calls_seen = (s.interp.vm_calls - ca, s.interp.vm_calls_slow - cb,
                                          s.interp.vm_calls_native - cn);
                            r
                        }
                    }
                };
                r
            }));
            for (k, n) in fdecl.drain(..) {
                *by_fdecline.entry(k).or_insert(0) += n;
            }
            vm_calls += calls_seen.0;
            vm_calls_slow += calls_seen.1;
            vm_calls_native += calls_seen.2;
            vm_ran += vm_seen.0;
            vm_declined += vm_seen.1;
            if let Some(w) = vm_seen.2 {
                *by_decline.entry(w).or_insert(0u64) += 1;
            }
            t_parse += np;
            t_exec += t0.elapsed().as_nanos().saturating_sub(np);
            let ms = t0.elapsed().as_millis();
            if ms >= 25 {
                use std::io::Write;
                eprintln!("   [langsam] {ms:5} ms  {rel}{}", if strict { " [strict]" } else { "" });
                let _ = std::io::stderr().flush();
                slow.push((ms, rel.clone()));
            }
            let ok = match &out {
                Err(_) => false,
                Ok(Ok(())) => !neg,
                Ok(Err(_)) => neg,
            };
            if matches!(out, Err(_)) { panics += 1; }
            #[cfg(feature = "strict-probe")]
            {
                let hit = probe.iter().any(|&n| n > 0);
                let t = if ok { &mut probe_pass } else { &mut probe_fail };
                for k in 0..beak_engine::js::STRICT_SITES {
                    if probe[k] > 0 { t[k] += 1; }
                }
                if hit {
                    if ok { probe_pass_any += 1; } else { probe_fail_any += 1; }
                }
                if !ok && hit {
                    // With the message: a hit point does not mean the test
                    // dies there (`propertyHelper.js` writes to non-writable
                    // properties itself and catches the error). Only the
                    // message says whether the missing throw was the cause.
                    let why = match &out {
                        Err(_) => "LAEUFER: Absturz".to_string(),
                        Ok(Err(e)) => e.clone(),
                        Ok(Ok(())) => "erwartete einen Fehler, es lief durch".to_string(),
                    };
                    probe_names.push((format!("{rel}{}",
                        if strict { " [strict]" } else { "" }), probe, why));
                }
            }
            if ok { pass += 1; continue; }
            let why = match out {
                Err(_) => "LAEUFER: Absturz".to_string(),
                Ok(Err(e)) => e,
                Ok(Ok(())) => "erwartete einen Fehler, es lief durch".to_string(),
            };
            // Grouped by the whole message, not just its kind:
            // "ReferenceError" is no diagnosis, "Symbol is not defined" is.
            // Numbers and quotes are dropped so one cause does not split into
            // a thousand variants.
            let norm: String = why.chars()
                .map(|c| if c.is_ascii_digit() { '#' } else { c })
                .collect::<String>()
                .replace('"', "'");
            let key: String = norm.chars().take(64).collect();
            let e = by_msg.entry(key).or_insert((0, rel.clone()));
            e.0 += 1;
            // The cap applies to the screen output; `T262_FAILDETAIL` wants
            // everything.
            if fails.len() < 5000 || std::env::var("T262_FAILDETAIL").is_ok() {
                fails.push((rel.clone(), why));
            }
            // All names, not just the first ones: only a complete list can be
            // diffed against a second run.
            //
            // With the mode: the variant is what is counted (a file without a
            // flag runs twice), and without the marker both collapse into one
            // name, so a fix that moves only strict mode would not change
            // the list.
            all_fails.push(format!("{rel}{}", if strict { " [strict]" } else { "" }));
        }
    }

    // `T262_FAILDETAIL=<file>`: every failure with its message. The grouped
    // report shows twenty lines and one example file; this answers which
    // directories lie behind a given message.
    if let Ok(path) = std::env::var("T262_FAILDETAIL") {
        let mut out = String::new();
        for (rel, why) in &fails {
            out.push_str(&format!("{}\t{}\n", rel, why.replace('\t', " ").replace('\n', " ")));
        }
        let _ = fs::write(&path, out);
        eprintln!("   {} Zeilen mit Meldung -> {path}", fails.len());
    }
    if let Some(path) = &faillist {
        all_fails.sort();
        all_fails.dedup();
        let _ = fs::write(path, all_fails.join("\n"));
        eprintln!("   {} Namen -> {path}", all_fails.len());
    }

    let pct = |n: usize, d: usize| if d == 0 { 0.0 } else { 100.0 * n as f64 / d as f64 };
    let tot = vm_ran + vm_declined;
    if tot > 0 {
        eprintln!("\n   Befehlsmaschine: {vm_ran} von {tot} Programmen = {:.1} %",
                  100.0 * vm_ran as f64 / tot as f64);
        let mut d: Vec<(&&str, &u64)> = by_decline.iter().collect();
        d.sort_by(|a, b| b.1.cmp(a.1));
        let ct = vm_calls + vm_calls_slow;
        if ct > 0 {
            eprintln!("   JS-Aufrufe als RAHMEN: {vm_calls} von {ct} = {:.1} %  ({vm_calls_native} eingebaute daneben)",
                      100.0 * vm_calls as f64 / ct as f64);
        }
        eprintln!("   Woran der Uebersetzer bei einem PROGRAMM absagt:");
        for (k, n) in d.iter().take(12) {
            eprintln!("      {n:6}  {k}");
        }
        let mut fd: Vec<(&&str, &u64)> = by_fdecline.iter().collect();
        fd.sort_by(|a, b| b.1.cmp(a.1));
        eprintln!("   … und bei einem FUNKTIONSRUMPF:");
        for (k, n) in fd.iter().take(12) {
            eprintln!("      {n:6}  {k}");
        }
    }
    #[cfg(feature = "strict-probe")]
    {
        eprintln!("\n── Der STRENGE MODUS: was haengt wirklich daran ──");
        eprintln!("   Gezaehlt wird die VARIANTE, die an der Stelle vorbeikam —");
        eprintln!("   nicht, wie oft. Eine Variante kann mehrere Stellen treffen.");
        eprintln!("   {:>7} {:>7}   {}", "gerissen", "bestanden", "Stelle");
        let mut rows: Vec<usize> = (0..beak_engine::js::STRICT_SITES).collect();
        rows.sort_by_key(|&k| std::cmp::Reverse(probe_fail[k]));
        for k in rows {
            eprintln!("   {:>7} {:>9}   {}", probe_fail[k], probe_pass[k],
                      beak_engine::js::STRICT_SITE_NAMES[k]);
        }
        eprintln!("   ──");
        eprintln!("   {probe_fail_any} GESCHEITERTE Varianten kamen an mindestens einer Stelle vorbei");
        eprintln!("   {probe_pass_any} bestandene ebenfalls — die sind die Gegenprobe:");
        eprintln!("   eine Stelle zu treffen heisst NICHT, dass der Test daran stirbt.");
        if let Ok(path) = std::env::var("T262_PROBELIST") {
            let mut out = String::new();
            for (n, p, why) in &probe_names {
                let sites: Vec<String> = (0..beak_engine::js::STRICT_SITES)
                    .filter(|&k| p[k] > 0).map(|k| k.to_string()).collect();
                let w = why.replace('\t', " ").replace('\n', " ");
                out.push_str(&format!("{}\t{}\t{}\n", sites.join(","), n, w));
            }
            let _ = fs::write(&path, out);
            eprintln!("   {} Zeilen -> {path}", probe_names.len());
        }
    }
    eprintln!("\n── test262, AUSFUEHRUNG ──");
    eprintln!("   {} Dateien; uebergangen: {skip_dir} Verzeichnis, {skip_feat} Feature, {skip_kind} Modul+async",
        files.len());
    eprintln!("   bestanden {pass} von {run} gefahren = {:.2} %", pct(pass, run));
    eprintln!("   (V8 auf demselben Korpus: 99,41 % — die DIFFERENZ ist die Arbeit)");
    if panics > 0 { eprintln!("   ⚠ {panics} Abstuerze im Laeufer"); }
    eprintln!("   Phasen: {:.1} s lesen+zusammenbauen · {:.1} s parsen · {:.1} s ausfuehren",
        t_read as f64 / 1e9, t_parse as f64 / 1e9, t_exec as f64 / 1e9);
    if !slow.is_empty() {
        slow.sort_by_key(|(ms, _)| std::cmp::Reverse(*ms));
        let total: u128 = slow.iter().map(|(ms, _)| ms).sum();
        eprintln!("   ⏱ {} Tests ueber 25 ms, zusammen {:.1} s — die teuersten:",
            slow.len(), total as f64 / 1000.0);
        for (ms, p) in slow.iter().take(12) { eprintln!("      {ms:6} ms  {p}"); }
    }

    let mut v: Vec<_> = by_msg.into_iter().collect();
    v.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    eprintln!("\n   Woran es scheitert, nach Meldung:");
    for (msg, (n, ex)) in v.iter().take(24) { eprintln!("      {n:6}  {msg}\n              z.B. {ex}"); }
    if show > 0 {
        eprintln!("\n   Erste {} Fehler:", show.min(fails.len()));
        for (p, w) in fails.iter().take(show) { eprintln!("      {p}\n         {w}"); }
    }
}
