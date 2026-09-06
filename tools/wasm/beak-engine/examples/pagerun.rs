//! Die GANZE Skriptrunde einer Seite host-seitig fahren — in EINER Sitzung,
//! in Dokumentreihenfolge, mit demselben Modul-Rueckfall wie beak.
//!
//! `jsrun` faehrt eine Datei allein, und das beantwortet die falsche Frage:
//! am Geraet teilen sich alle Skripte einen globalen Bereich, und ein Skript,
//! das allein `x is not defined` wirft, laeuft in der Kette sauber. Genau so
//! ist die Fritzbox-Anmeldeseite zuerst falsch gelesen worden.
//!
//!   cargo run --release --example pagerun -- seite.html verzeichnis/
//!
//! Die externen Skripte werden NICHT geholt — sie werden im Verzeichnis unter
//! dem letzten Pfadbestandteil ihrer `src` erwartet (`curl` legt sie so ab).
//! Fehlt eine Datei, sagt der Lauf das, statt sie still zu ueberspringen.
use beak_engine::js::dombind::ScriptRef;

fn main() {
    let html_path = std::env::args().nth(1).unwrap_or_default();
    let dir = std::env::args().nth(2).unwrap_or_else(|| ".".into());
    let html = std::fs::read_to_string(&html_path).expect("HTML-Datei");

    let dom = beak_engine::dom::parse(&html);
    let doc = beak_engine::js::dombind::Doc::from_dom(&dom);
    let refs = beak_engine::js::dombind::page_scripts(&doc);

    let mut sess = beak_engine::js::Session::new(5_000_000);
    sess.interp.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
    let media = beak_engine::css::Media::new(1902.0, false);
    let sheet = beak_engine::css::collect_all(&dom, "", media);
    sess.interp.set_style_context(beak_engine::js::interp::StyleCtx {
        sheet: std::rc::Rc::new(sheet),
        theme: beak_engine::layout::Theme {
            bg: beak_engine::layout::Rgb(255, 255, 255),
            text: beak_engine::layout::Rgb(33, 37, 41),
            heading: beak_engine::layout::Rgb(33, 37, 41),
            link: beak_engine::layout::Rgb(13, 110, 253),
            muted: beak_engine::layout::Rgb(108, 117, 125),
            rule: beak_engine::layout::Rgb(222, 226, 230),
        },
        viewport_w: 1902.0,
    });
    sess.interp.set_media(1902.0, 1000.0, false);
    // `DEPTH=` hebt den Aufrufdeckel — die Frage „echte Endlosschleife oder
    // nur tiefer als 400?" ist sonst nicht zu beantworten.
    if let Ok(d) = std::env::var("DEPTH") {
        if let Ok(n) = d.parse() { sess.interp.max_depth = n; }
    }
    if let Ok(u) = std::env::var("URL") { sess.interp.set_location(&u); }

    let (mut ran, mut failed) = (0usize, 0usize);
    let mut inline_n = 0usize;
    for r in refs {
        let (src, label, is_mod) = match r {
            ScriptRef::Inline(t, m) => { inline_n += 1; (t, format!("inline #{inline_n}"), m) }
            ScriptRef::External(u, m) => {
                let name = u.rsplit('/').next().unwrap_or(&u).to_string();
                match std::fs::read_to_string(format!("{dir}/{name}")) {
                    Ok(t) => (t, u, m),
                    Err(e) => {
                        failed += 1;
                        println!("FAIL {u}: nicht im Verzeichnis ({e})");
                        continue;
                    }
                }
            }
        };
        let n0 = sess.interp.console.len();
        // Ein Modul: erst den GANZEN Graphen holen, dann verknuepfen, dann
        // auswerten — genau die Reihenfolge, die beak am Geraet faehrt.
        if is_mod || is_module(&src) {
            match run_module_graph(&mut sess, &label, &src, &dir) {
                Ok(()) => { ran += 1; println!("ok   {label} (Modul, {} B)", src.len()); }
                Err(e) => { failed += 1; println!("FAIL {label}: {e}"); }
            }
            for l in &sess.interp.console[n0..] { println!("       | {l}"); }
            continue;
        }
        let prog = match beak_engine::js::parse(&src, false) {
            Ok(p) => p,
            Err(e) => match beak_engine::js::parse(&src, true) {
                Ok(p) => p,
                Err(em) => {
                    failed += 1;
                    println!("FAIL {label}: SyntaxError: {} @{} | als Modul: {} @{}",
                             e.msg, pos(&src, e.at), em.msg, pos(&src, em.at));
                    continue;
                }
            },
        };
        match sess.run(&prog) {
            Ok(()) => { ran += 1; println!("ok   {label} ({} B)", src.len()); }
            Err(e) => { failed += 1; println!("FAIL {label}: {e}"); }
        }
        for l in &sess.interp.console[n0..] { println!("       | {l}"); }
    }
    let n0 = sess.interp.console.len();
    let mut timers = 0;
    for _ in 0..64 {
        let t = sess.interp.run_timers();
        timers += t;
        if t == 0 { break }
    }
    for l in &sess.interp.console[n0..] { println!("  timer| {l}"); }
    // `DUMP=1` zeigt, was am Ende im Baum steht — die Frage „laufen die
    // Skripte" ist nicht dieselbe wie „haben sie etwas gebaut".
    if std::env::var("DUMP").is_ok() {
        if let Some(d) = sess.interp.doc.as_mut() {
            let dom = d.to_dom();
            let mut out = String::new();
            dump(dom.body(), 0, &mut out);
            println!("\n── Baum nach den Skripten ──\n{out}");
        }
    }
    let listeners = sess.interp.doc.as_ref().is_some_and(|d| d.has_listeners);
    println!("\n{ran} gelaufen, {failed} gescheitert, {timers} Zeitgeber, {}",
             if listeners { "Ereignisse SCHARF" } else { "keine Behandler" });
}

fn pos(src: &str, at: usize) -> String {
    let mut at = at.min(src.len());
    while at > 0 && !src.is_char_boundary(at) { at -= 1; }
    let line = src[..at].bytes().filter(|b| *b == b'\n').count() + 1;
    let ls = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let le = src[at..].find('\n').map(|i| at + i).unwrap_or(src.len());
    let mut from = at.saturating_sub(48).max(ls);
    while from < at && !src.is_char_boundary(from) { from += 1; }
    let mut to = (at + 48).min(le);
    while to > at && !src.is_char_boundary(to) { to -= 1; }
    format!("{line}:{} ...{}<<HIER>>{}...", at - ls + 1, &src[from..at], &src[at..to])
}

/// Hat der Quelltext `import`/`export` auf oberster Ebene? Die Datei sagt es
/// nicht, also entscheidet der Parser: was NUR als Modul parst, ist eines.
fn is_module(src: &str) -> bool {
    beak_engine::js::parse(src, false).is_err() && beak_engine::js::parse(src, true).is_ok()
}

/// Der Ursprung, gegen den Modul-Adressen absolut werden. `import.meta.url`
/// muss eine ECHTE Adresse sein — die Komponenten der Fritzbox bauen daraus
/// `new URL(x, import.meta.url)`, und ein blosser Pfad ist da keine Basis.
fn origin() -> String {
    let u = std::env::var("URL").unwrap_or_default();
    match u.find("://") {
        Some(i) => match u[i + 3..].find('/') {
            Some(j) => u[..i + 3 + j].to_string(),
            None => u.trim_end_matches('/').to_string(),
        },
        None => String::new(),
    }
}

/// Eine Adresse gegen die des Importeurs aufloesen.
fn resolve_path(base: &str, spec: &str) -> String {
    let org = origin();
    let path = |u: &str| u.strip_prefix(&org).unwrap_or(u).to_string();
    if spec.contains("://") { return spec.to_string(); }
    if spec.starts_with('/') { return format!("{org}{spec}"); }
    let b = path(base);
    let dir = b.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for p in spec.split('/') {
        match p { "." | "" => {}, ".." => { parts.pop(); }, x => parts.push(x) }
    }
    format!("{org}/{}", parts.join("/"))
}

/// Wo die Datei zu einer Adresse liegt: `curl` hat sie unter dem Pfad mit
/// `_` statt `/` abgelegt.
fn local(dir: &str, url: &str) -> String {
    let org = origin();
    let p = url.strip_prefix(&org).unwrap_or(url);
    format!("{dir}/{}", p.trim_start_matches('/').replace('/', "_"))
}

fn run_module_graph(sess: &mut beak_engine::js::Session, label: &str, src: &str, dir: &str)
    -> Result<(), String> {
    let entry = format!("{}/__entry__{}", origin(), label.replace(' ', "_"));
    let prog = beak_engine::js::parse(src, true).map_err(|e| format!("SyntaxError: {} @{}", e.msg, pos(src, e.at)))?;
    sess.interp.add_module(&entry, std::rc::Rc::new(prog));
    // Holen, bis der Graph geschlossen ist.
    let mut queue = vec![entry.clone()];
    while let Some(u) = queue.pop() {
        for spec in sess.interp.module_requests(&u) {
            let r = resolve_path(&u, &spec);
            sess.interp.map_module_dep(&u, &spec, &r);
            if sess.interp.has_module(&r) { continue }
            let text = std::fs::read_to_string(local(dir, &r))
                .map_err(|e| format!("{r}: nicht da ({e})"))?;
            let p = beak_engine::js::parse(&text, true)
                .map_err(|e| format!("{r}: SyntaxError: {} @{}", e.msg, pos(&text, e.at)))?;
            sess.interp.add_module(&r, std::rc::Rc::new(p));
            queue.push(r);
        }
    }
    sess.interp.module_fail = None;
    sess.interp.eval_module(&entry).map_err(|e| {
        let msg = beak_engine::js::modules::describe(&mut sess.interp, e);
        match sess.interp.module_fail.clone() {
            Some(u) if &*u != entry.as_str() => format!("{msg}   [in {u}]"),
            _ => msg,
        }
    })
}

/// Den Baum als Umriss: Marke, id/class, und Text gekuerzt.
fn dump(e: &beak_engine::dom::Element, depth: usize, out: &mut String) {
    if depth > 12 { return }
    let pad = "  ".repeat(depth);
    let id = e.attr("id").map(|v| format!("#{v}")).unwrap_or_default();
    let cls = e.attr("class").map(|v| format!(".{}", v.replace(' ', "."))).unwrap_or_default();
    out.push_str(&format!("{pad}<{}{id}{cls}>\n", e.tag));
    for c in &e.children {
        match c {
            beak_engine::dom::Node::Element(x) => dump(x, depth + 1, out),
            beak_engine::dom::Node::Text(t) => {
                let t = t.trim();
                if !t.is_empty() {
                    let t: String = t.chars().take(60).collect();
                    out.push_str(&format!("{pad}  \"{t}\"\n"));
                }
            }
            _ => {}
        }
    }
}
