//! test262 als PARSE-Orakel — die Sprachzahl, bevor es eine Auswertung gibt.
//!
//! test262 sagt zu jeder Datei, ob sie gueltiges JavaScript ist:
//!
//! - `negative: { phase: parse }` → der Parser MUSS ablehnen.
//! - alles andere → der Parser MUSS annehmen. Auch `phase: resolution` und
//!   `phase: runtime`: die sind syntaktisch einwandfrei und scheitern spaeter.
//!
//! Das ist ein vollstaendiges, hartes Urteil ueber die Grammatik, ganz ohne
//! Interpreter — und deshalb die Leiter, die vor der Maschine steht.
//!
//! Der Korpus liegt NICHT im Repo (273 MB). Pfad ueber `TEST262=`; ohne die
//! Variable ueberspringt der Test sich selbst und sagt, wie man ihn anschaltet.
//!
//!   TEST262=~/…/tools/test262-upstream cargo test --release \
//!     --manifest-path tools/wasm/beak-engine/Cargo.toml --test test262 -- --nocapture
//!
//! `T262_FILTER=<substr>` grenzt ein · `T262_SHOW=<n>` zeigt n Fehler.
//!
//! Verglichen wird gegen `tools/test262/out/baseline-v8.json`: ein Test, den
//! wir reissen und V8 besteht, ist UNSERE Luecke. Die eigene Prozentzahl allein
//! sagt wenig — test262 laeuft den Motoren voraus.

use std::fs;
use std::path::{Path, PathBuf};

/// Verzeichnisse ausserhalb des Ziels — dieselbe Politik wie
/// `tools/test262/subset.json`, hier auf das reduziert, was fuers PARSEN zaehlt.
const SKIP_DIRS: &[&str] = &["intl402", "staging"];

/// Nur SYNTAX-Vorschlaege, die wir bewusst nicht bauen. Der Unterschied zum
/// Ausfuehrungslauf ist gross und lehrreich: `Temporal` parst tadellos (es
/// fehlen nur Builtins), also faellt es hier NICHT weg. Ausgeschlossen ist
/// allein, was die Grammatik selbst aendert.
const SKIP_FEATURES: &[&str] = &[
    "decorators",
    "explicit-resource-management",   // `using x = …`
    "import-attributes",
    "import-assertions",
    "source-phase-imports",
    "import-defer",
];

/// Der ZWEITE Nenner fuer den Ausfuehrungslauf — dieselbe Politik wie
/// `tools/test262/subset.json`.
///
/// Er ist LAENGER als der fuers Parsen, und das ist kein Widerspruch:
/// `Temporal` parst tadellos und faellt dort zurecht nicht weg, aber
/// AUSFUEHREN kann es nur, wer es gebaut hat — und wir bauen es erklaertermassen
/// nicht. Die erste Fassung dieses Laufs zaehlte 4436 Temporal-Tests als
/// Misserfolg mit; das ist kein ehrlicher Nenner, das ist eine
/// selbstgemachte Niederlage.
const SKIP_FEATURES_EXEC: &[&str] = &[
    "Temporal", "Intl.Era-monthcode", "explicit-resource-management", "decorators",
    "Atomics", "Atomics.pause", "SharedArrayBuffer", "import-assertions",
    "import-attributes", "source-phase-imports", "iterator-sequencing",
    "Math.sumPrecise", "uint8array-base64", "ShadowRealm", "await-dictionary",
    "joint-iteration", "iterator-chunking", "iterator-includes", "import-defer",
    "immutable-arraybuffer", "error-stack-accessor",
];

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
    // Getrennt gezaehlt, weil sie voellig verschieden wiegen: eine Datei, die
    // wir faelschlich ABLEHNEN, kostet die ganze Seite. Eine, die wir
    // faelschlich ANNEHMEN, ist ein fehlender Fruehfehler — laestig, aber die
    // Seite laeuft.
    let (mut n_reject, mut n_accept) = (0usize, 0usize);
    let mut accept_fam: std::collections::BTreeMap<String, usize> = Default::default();
    // Gezaehlt wird immer, gesammelt nur bis zum Deckel — sonst meldet der
    // Bericht die Groesse des Deckels und nicht die Zahl.
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
                    // Nach Familie gebuendelt statt einzeln: 5000 Pfade sind
                    // keine Information, "welche Fruehfehler-Familie fehlt"
                    // ist eine ([[feedback_census_by_family_not_suite]]).
                    // Nach der REGEL gebuendelt, nicht nach dem Verzeichnis:
                    // test262 nennt sie in `description`, und "Klassen 1810"
                    // ist eine Adresse, keine Diagnose. Der Teil vor dem
                    // ersten Doppelpunkt/Klammer traegt die Regel.
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

    // Nach Grund gruppiert, mit EINEM Beispielpfad je Grund — eine Liste von
    // 400 Pfaden sagt nichts, "welche Meldung wie oft, und wo nachsehen" sagt,
    // was als naechstes zu bauen ist.
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

/// Die zweite Zahl, und fuer beak die wichtigere: parst das, was der
/// ZIELKORPUS wirklich ausliefert?
///
/// test262 misst die Sprache, dieser Test misst das Web. Die Skripte sind die,
/// die Chromium beim Laden der zwoelf Seiten geparst hat (`tools/jsscope/js/`,
/// abgelegt von `measure.mjs`) — also echter, ausgelieferter, minifizierter
/// Code und keine Testfaelle.
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
        // Ein ausgeliefertes Skript kann Script ODER Modul sein, und die Datei
        // sagt es nicht. Beides versuchen: nur wenn KEINES parst, ist es eine
        // Luecke.
        if beak_engine::js::parses(&src, false).is_ok()
            || beak_engine::js::parses(&src, true).is_ok() {
            ok += 1; e.0 += 1; bytes_ok += src.len();
        } else {
            // Die Meldung aus dem MODUL-Versuch: die drei Ausreisser des
            // ersten Laufs waren allesamt Module, und der Skript-Fehler
            // ("unexpected keyword" bei `export`) sagte darueber nichts.
            // Der Fehler aus dem MODUL-Versuch. `or_else` liefert den zweiten
            // Fehler, nicht den ersten — und der Skript-Fehler bei einem Modul
            // ist immer nur "unexpected keyword" beim `export`, also nutzlos.
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


/// Der AUSFUEHRUNGSLAUF. test262 sagt hier nicht mehr nur „ist das gueltige
/// Syntax", sondern „tut es das Richtige" — und das ist die Zahl, gegen die
/// jede weitere Arbeit an der Maschine gemessen wird.
///
/// Verglichen wird gegen `tools/test262/out/baseline-v8.json` (V8: 99,41 %).
/// Die eigene Zahl allein sagt wenig; die DIFFERENZ sagt alles.
///
///   TEST262=<…> cargo test --release --test test262 exec -- --nocapture
///
/// `T262_FILTER` grenzt ein · `T262_SHOW` zeigt n Fehler.
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

    let hread = |f: &str| fs::read_to_string(harness.join(f)).unwrap_or_default();
    let mut hmap: std::collections::BTreeMap<String, String> = Default::default();
    let mut hcache = |f: &str| -> String {
        hmap.entry(f.to_string()).or_insert_with(|| fs::read_to_string(harness.join(f)).unwrap_or_default()).clone()
    };
    // EINMAL geparst, dann nur noch ausgefuehrt. Der Vorspann je Variante neu
    // zu parsen war der erste Entwurf und hat den Lauf allein damit verbracht.
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
    // Die Phasen IM echten Lauf, nicht in einer Nebenmessung. Die
    // Nebenmessung sagte 46 µs je Variante und lag um den Faktor 100 daneben,
    // weil sie den teuren Fall nicht enthielt: sie parste nichts.
    let (mut t_read, mut t_parse, mut t_exec) = (0u128, 0u128, 0u128);
    let mut by_msg: std::collections::BTreeMap<String, (usize, String)> = Default::default();

    for p in &files {
        let rel = p.strip_prefix(&tests).unwrap().to_string_lossy().replace('\\', "/");
        if !filter.is_empty() && !rel.contains(&filter) { continue; }
        if SKIP_DIRS.iter().any(|d| rel.starts_with(d)) { skip_dir += 1; continue; }
        let Ok(src) = fs::read_to_string(p) else { continue };
        let m = frontmatter(&src);

        // Module und async brauchen Auflöser bzw. Promises — beides gibt es
        // noch nicht. Eigene Zeile im Bericht, NICHT unter "bestanden".
        if m.flags.iter().any(|f| f == "module" || f == "async") { skip_kind += 1; continue; }
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
            // Die Hilfsdateien aus dem Zwischenspeicher: `propertyHelper.js`
            // allein sind 510 Zeilen, und sie je Variante von der Platte zu
            // holen ist Arbeit fuer nichts.
            if !raw { for inc in &m.includes { text.push_str(&hcache(inc)); text.push('\n'); } }
            text.push_str(&src);

            let neg = m.negative_parse || m.negative_other;
            // Wer laenger braucht als das, wird SOFORT genannt — mit
            // `flush`, damit die Zeile auch dann steht, wenn der Lauf danach
            // haengt. Ein Testlaeufer, der ohne Angabe stehenbleiben kann,
            // ist nicht fertig: dreimal in dieser Sitzung habe ich stattdessen
            // geraten, wo die Zeit bleibt.
            // Der Name VOR dem Lauf, nicht danach. Ein Test, der nie
            // zurueckkehrt, taucht in einer Meldung danach nie auf — genau
            // daran ist die Suche nach dem Haenger in `built-ins/Object`
            // zweimal vorbeigelaufen. Nur mit `T262_TRACE`, weil eine Datei je
            // Variante sonst selbst Zeit kostet.
            if let Some(mark) = &trace {
                let _ = fs::write(mark, format!("{rel}{}", if strict { " [strict]" } else { "" }));
            }
            let t_r = std::time::Instant::now();
            // Ein Absturz im Interpreter darf den LAUF nicht beenden — sonst
            // misst ein einziger `unwrap` gar nichts mehr. Getrennt gezaehlt.
            t_read += t_r.elapsed().as_nanos();
            let t0 = std::time::Instant::now();
            let mut np = 0u128;
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let tp = std::time::Instant::now();
                let prog = match beak_engine::js::parse(&text, false) {
                    Ok(p) => p,
                    Err(e) => return Err(format!("SyntaxError: {} @{}", e.msg, e.at)),
                };
                np = tp.elapsed().as_nanos();
                let mut s = beak_engine::js::Session::new(beak_engine::js::TEST_STEPS);
                if raw { return s.run(&prog); }
                s.run(if strict { &prologue_strict } else { &prologue })?;
                s.run(&prog)
            }));
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
            if ok { pass += 1; continue; }
            let why = match out {
                Err(_) => "LAEUFER: Absturz".to_string(),
                Ok(Err(e)) => e,
                Ok(Ok(())) => "erwartete einen Fehler, es lief durch".to_string(),
            };
            // Nach der GANZEN Meldung gebuendelt, nicht nur nach ihrer Art.
            // "30221 ReferenceError" ist keine Diagnose; "Symbol is not
            // defined" ist eine. Zahlen und Anfuehrungszeichen fallen weg,
            // damit dieselbe Ursache nicht in tausend Varianten zerfaellt.
            let norm: String = why.chars()
                .map(|c| if c.is_ascii_digit() { '#' } else { c })
                .collect::<String>()
                .replace('"', "'");
            let key: String = norm.chars().take(64).collect();
            let e = by_msg.entry(key).or_insert((0, rel.clone()));
            e.0 += 1;
            if fails.len() < 5000 { fails.push((rel.clone(), why)); }
        }
    }

    let pct = |n: usize, d: usize| if d == 0 { 0.0 } else { 100.0 * n as f64 / d as f64 };
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
