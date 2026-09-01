// Woran stirbt echter, ausgelieferter Code ZUERST?
//
// Nicht test262, sondern die Skripte, die Chromium beim Laden der zwoelf
// Zielseiten geparst hat. Ohne DOM — das ist erwartet und genau der Punkt:
// die Frage ist, ob die SPRACHE die Wand ist oder die Wirtsumgebung.
//
// **Je SEITE eine Umgebung, nicht je Datei.** Die erste Fassung fuhr jedes
// Skript einzeln und meldete 98 mal `mw is not defined` — aber `mw` wird von
// einem SCHWESTERSKRIPT derselben Seite gesetzt. Ein Browser teilt die
// Umgebung, also muss diese Messung es auch tun, sonst misst sie die
// Isolation und nicht die Engine.
fn main() {
    let root = std::env::var("JSCORPUS").unwrap();
    let mut pages: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> = Default::default();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if !e.path().is_dir() { continue }
            let name = e.file_name().to_string_lossy().to_string();
            let mut fs_: Vec<_> = std::fs::read_dir(e.path()).unwrap().flatten()
                .map(|x| x.path()).filter(|p| p.extension().is_some_and(|x| x == "js")).collect();
            // In der Reihenfolge, in der `measure.mjs` sie abgelegt hat — das
            // ist die Reihenfolge, in der der Browser sie geparst hat.
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
        // Eine Umgebung fuer die ganze Seite. Ein Absturz in Skript 3 darf die
        // Umgebung nicht mitnehmen, also faengt jeder Lauf fuer sich.
        let mut sess = beak_engine::js::Session::new(2_000_000);
        // Das ECHTE HTML der Seite dazu — `measure.mjs` hat es neben den
        // Skripten abgelegt. Ohne Dokument misst dieser Lauf nur die Sprache;
        // mit ihm misst er, was ein Skript im Browser vorfindet.
        let html_path = std::path::Path::new(&root).parent().unwrap()
            .join("html").join(format!("{page}.html"));
        if let Ok(html) = std::fs::read_to_string(&html_path) {
            let dom = beak_engine::dom::parse(&html);
            sess.interp.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
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
