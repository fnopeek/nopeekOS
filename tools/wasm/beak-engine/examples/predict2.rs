// Was zeigt der Geraetetest MIT externen Skripten? Aendert sich der Baum?
//
// `wallcheck` fragt "laufen die Skripte durch", dieses hier fragt "tun sie
// etwas SICHTBARES" — und das ist die Frage, die der Anwender stellt.
//
// Die Zuordnung `<script src>` -> abgelegte Datei kommt aus `measure.json`:
//
//   python3 -c "import json;[print(f\"{p[\'name\']}\\t{s[\'url\']}\\t{s[\'file\']}\")
//     for p in json.load(open(\'out/measure.json\')) for s in (p.get(\'scripts\') or [])]" > map.tsv
//   SCRIPTMAP=map.tsv JSSCOPE=<…>/tools/jsscope cargo run --release --example predict2
fn main() {
    use beak_engine::js::dombind::{Doc, ScriptRef, page_scripts};
    let base = std::env::var("JSSCOPE").unwrap();
    // Zuordnung Seite -> (URL, Datei), von `measure.json` abgeleitet.
    let tsv = std::fs::read_to_string(std::env::var("SCRIPTMAP").unwrap()).unwrap();
    let mut pages: Vec<String> = Vec::new();
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for line in tsv.lines() {
        let mut it = line.split('\t');
        let (Some(p), Some(u), Some(f)) = (it.next(), it.next(), it.next()) else { continue };
        if !pages.iter().any(|x| x == p) { pages.push(p.to_string()); }
        rows.push((p.to_string(), u.to_string(), f.to_string()));
    }
    for name in &pages {
        let name = name.clone();
        let Ok(html) = std::fs::read_to_string(format!("{base}/html/{name}.html")) else { continue };
        let map: Vec<(String, String)> = rows.iter().filter(|(p, _, _)| *p == name)
            .map(|(_, u, f)| (u.clone(), f.clone())).collect();
        let dom = beak_engine::dom::parse(&html);
        let doc = Doc::from_dom(&dom);
        let refs = page_scripts(&doc);
        let mut texts = Vec::new();
        let (mut inl, mut ext, mut miss) = (0, 0, 0);
        for r in refs {
            match r {
                ScriptRef::Inline(t) => { inl += 1; texts.push(t) }
                ScriptRef::External(src) => {
                    let tail = src.rsplit('/').next().unwrap_or("").to_string();
                    match map.iter().find(|(u, _)| u.ends_with(&tail) && !tail.is_empty()) {
                        Some((_, f)) => {
                            ext += 1;
                            texts.push(std::fs::read_to_string(format!("{base}/js/{name}/{f}")).unwrap_or_default());
                        }
                        None => miss += 1,
                    }
                }
            }
        }
        let before = snap(&doc);
        let n0 = doc.nodes.len();
        let mut sess = beak_engine::js::Session::new(5_000_000);
        sess.interp.set_document(doc);
        let t = std::time::Instant::now();
        let (mut ok, mut bad) = (0, 0);
        let mut first_err = String::new();
        for s in &texts {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let p = beak_engine::js::parse(s, false)
                    .or_else(|_| beak_engine::js::parse(s, true)).map_err(|e| e.msg.to_string())?;
                sess.run(&p)
            }));
            match r {
                Ok(Ok(())) => ok += 1,
                Ok(Err(e)) => { if bad == 0 { first_err = e; } bad += 1 }
                Err(_) => { if bad == 0 { first_err = "ABSTURZ".into(); } bad += 1 }
            }
        }
        let ms = t.elapsed().as_millis();
        let d = sess.interp.doc.as_ref().unwrap();
        let changed = snap(d) != before;
        println!("  {:<17} {:2} inline + {:2} extern ({} fehlend) | {} ok {} Fehler | {:4} ms | Baum: {} {}",
            name, inl, ext, miss, ok, bad, ms,
            if changed { "GEAENDERT" } else { "gleich   " },
            if d.nodes.len() > n0 { format!("+{} Knoten", d.nodes.len()-n0) } else { String::new() });
        if !first_err.is_empty() { println!("        erster Fehler: {}", &first_err[..first_err.len().min(80)]); }
    }
}
fn snap(d: &beak_engine::js::dombind::Doc) -> String {
    let mut s = String::new();
    let mut all = Vec::new();
    d.descendants(d.doc, &mut all);
    for id in all {
        let n = &d.nodes[id as usize];
        s.push_str(&n.tag);
        for (k, v) in &n.attrs { s.push('|'); s.push_str(k); s.push('='); s.push_str(v); }
        s.push(';');
    }
    s
}
