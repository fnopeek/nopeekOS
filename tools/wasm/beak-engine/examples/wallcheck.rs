// Woran stirbt echter, ausgelieferter Code ZUERST?
//
// Nicht test262, sondern die Skripte, die Chromium beim Laden der zwoelf
// Zielseiten geparst hat. Ohne DOM — `document` fehlt, das ist erwartet und
// genau der Punkt: die Frage ist, ob die SPRACHE die Wand ist oder das DOM.
fn main() {
    let root = std::env::var("JSCORPUS").unwrap();
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "js") { out.push(p) }
            }
        }
    }
    walk(std::path::Path::new(&root), &mut files);
    files.sort();

    let mut hist: std::collections::BTreeMap<String, usize> = Default::default();
    let (mut ok, mut n) = (0usize, 0usize);
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else { continue };
        n += 1;
        // Ein ausgeliefertes Skript kann Script ODER Modul sein, und die
        // Datei sagt es nicht. Beides versuchen — sonst misst man 250 mal
        // "unexpected keyword" beim `export` und haelt es fuer eine Luecke.
        let r = std::panic::catch_unwind(|| {
            match beak_engine::js::run_capped(&src, false, 500_000) {
                Ok(()) => Ok(()),
                Err(e1) => {
                    if e1.starts_with("SyntaxError") {
                        beak_engine::js::run_capped(&src, true, 500_000)
                    } else { Err(e1) }
                }
            }
        });
        let why = match r {
            Err(_) => "LAEUFER: Absturz".to_string(),
            Ok(Ok(())) => { ok += 1; continue }
            Ok(Err(e)) => e,
        };
        let key: String = why.chars().map(|c| if c.is_ascii_digit() { '#' } else { c })
            .collect::<String>().chars().take(62).collect();
        *hist.entry(key).or_default() += 1;
    }
    println!("\n── Echter Korpus, OHNE DOM: {ok} von {n} laufen durch\n");
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (k, c) in v.iter().take(18) { println!("  {c:4}  {k}"); }
}
