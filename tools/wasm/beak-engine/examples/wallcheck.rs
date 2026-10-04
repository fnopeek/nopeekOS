// Finds what real, shipped code dies on first: the scripts Chromium parsed
// while loading the target pages, not test262. Runs without a DOM by default,
// to tell whether the language or the host environment is the wall.
//
// One environment per page, not per file: scripts set globals for their
// siblings (e.g. `mw`), and a browser shares the environment. Running each
// file alone would measure the isolation, not the engine.
fn main() {
    let root = std::env::var("JSCORPUS").unwrap();
    let mut pages: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> = Default::default();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if !e.path().is_dir() { continue }
            let name = e.file_name().to_string_lossy().to_string();
            let mut fs_: Vec<_> = std::fs::read_dir(e.path()).unwrap().flatten()
                .map(|x| x.path()).filter(|p| p.extension().is_some_and(|x| x == "js")).collect();
            // In the order `measure.mjs` stored them, which is the order the
            // browser parsed them.
            fs_.sort_by_key(|p| {
                p.file_stem().and_then(|s| s.to_str())
                 .and_then(|s| s.rsplit("__").next().map(|n| n.parse::<u32>().unwrap_or(0)))
                 .unwrap_or(0)
            });
            pages.insert(name, fs_);
        }
    }

    let mut hist: std::collections::BTreeMap<String, usize> = Default::default();
    let (mut ok, mut n) = (0usize, 0usize);
    println!("\n── Echter Korpus: eine Umgebung je Seite, MIT ihrem DOM ──\n");
    for (page, files) in &pages {
        let (mut pok, mut pn) = (0usize, 0usize);
        // One environment for the whole page. A crash in one script must not
        // take the environment down, so each run is caught on its own.
        let mut sess = beak_engine::js::Session::new(2_000_000);
        // The page's real HTML, stored next to the scripts by `measure.mjs`.
        // Without a document this measures only the language; with it, what a
        // script finds in a browser.
        let html_path = std::path::Path::new(&root).parent().unwrap()
            .join("html").join(format!("{page}.html"));
        if let Ok(html) = std::fs::read_to_string(&html_path) {
            let dom = beak_engine::dom::parse(&html);
            sess.interp.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
            // Window and colour scheme as in a browser, or every page fails on
            // `innerWidth`/`matchMedia` and the run measures the tool.
            sess.interp.set_media(1280.0, 800.0, false);
        }
        for f in files {
            let Ok(src) = std::fs::read_to_string(f) else { continue };
            n += 1; pn += 1;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let prog = match beak_engine::js::parse(&src, false) {
                    Ok(p) => p,
                    Err(_) => match beak_engine::js::parse(&src, true) {
                        Ok(p) => p,
                        Err(e) => return Err(format!("SyntaxError: {}", e.msg)),
                    },
                };
                sess.run(&prog)
            }));
            let why = match r {
                Err(_) => "LAEUFER: Absturz".to_string(),
                Ok(Ok(())) => { ok += 1; pok += 1; continue }
                Ok(Err(e)) => e,
            };
            // `WCPAGE=<page>` (or `*`) prints file and reason. The histogram
            // says what the wall is, not where.
            let want = std::env::var("WCPAGE").unwrap_or_default();
            if want == page.as_str() || want == "*" {
                println!("    {} — {why}", f.file_name().unwrap().to_string_lossy());
            }
            let key: String = why.chars().map(|c| if c.is_ascii_digit() { '#' } else { c })
                .collect::<String>().chars().take(62).collect();
            *hist.entry(key).or_default() += 1;
        }
        let mark = if pok == pn { "   " } else { " ! " };
        println!("  {mark}{pok:3}/{pn:3}  {page}");
    }
    println!("\n  {ok} von {n} Skripten laufen durch\n");
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (k, c) in v.iter().take(20) { println!("  {c:4}  {k}"); }
}
