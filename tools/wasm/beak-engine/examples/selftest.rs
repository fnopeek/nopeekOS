// Die eingebaute Pruefseite host-seitig durchspielen — dieselbe Datei, die
// beak ausliefert. Was hier NEIN sagt, sagt auf dem Geraet auch NEIN; der
// Unterschied waere ein Befund fuer sich.
use beak_engine::js::dombind::Doc;

fn main() {
    use beak_engine::js::dombind::{Doc, ScriptRef, page_scripts};

    let html = include_str!("../../beak/src/selftest.html");
    let dom = beak_engine::dom::parse(html);
    let doc = Doc::from_dom(&dom);
    let scripts: Vec<String> = page_scripts(&doc)
        .into_iter()
        .filter_map(|r| match r { ScriptRef::Inline(t) => Some(t), _ => None })
        .collect();
    println!("{} eingebettete Skripte, {} B HTML", scripts.len(), html.len());

    let mut sess = beak_engine::js::Session::new(50_000_000);
    sess.interp.set_document(doc);
    sess.interp.set_media(1024.0, 768.0, false);
    for (n, src) in scripts.iter().enumerate() {
        let prog = match beak_engine::js::parse(src, false) {
            Ok(p) => p,
            Err(e) => { println!("Skript {n}: PARSE-FEHLER {e:?}"); continue }
        };
        if let Err(e) = sess.run(&prog) {
            println!("Skript {n}: LAUFFEHLER {e:?}");
        }
    }
    sess.interp.run_timers();

    for line in sess.interp.take_console() {
        println!("{line}");
    }
    let d = sess.interp.doc.as_ref().unwrap();
    println!("Behandler angemeldet: {}", d.has_listeners);

    // Die Klicks, die auf dem Geraet der Finger macht. Ohne sie ist die
    // halbe Pruefseite nur host-seitig behauptet.
    println!("\n── Klicks ──");
    for (id, times) in [("b1", 2usize), ("b2", 1), ("b3", 1), ("b4", 1)] {
        let Some(n) = find_id(sess.interp.doc.as_ref().unwrap(), id) else {
            println!("{id}: nicht gefunden"); continue;
        };
        let chain = ancestors(sess.interp.doc.as_ref().unwrap(), n);
        for _ in 0..times {
            match beak_engine::js::dombind::dispatch(&mut sess.interp, "click", &chain) {
                Ok(p) => { let _ = p; }
                Err(_) => println!("{id}: LAUFFEHLER"),
            }
        }
    }
    // Mehrere Runden: die Promise-Kette endet in einem `setTimeout`, das
    // erst faellig wird, nachdem die Kette durch ist.
    for _ in 0..8 { if sess.interp.run_timers() == 0 { break } }
    for line in sess.interp.take_console() { println!("{line}"); }
    let d = sess.interp.doc.as_ref().unwrap();
    for id in ["count", "inline", "bubble", "timer", "micro"] {
        match find_id(d, id) {
            Some(n) => println!("#{id}: {:?}", d.text_of(n)),
            None => println!("#{id}: nicht gefunden"),
        }
    }
    let li = d.nodes.iter().filter(|n| &*n.tag == "li").count();
    println!("<li> im Baum: {li}   (Baum geaendert: {})", d.dirty);
}

fn find_id(d: &Doc, id: &str) -> Option<u32> {
    let mut all = Vec::new();
    d.descendants(d.doc, &mut all);
    all.into_iter().find(|&x| d.nodes[x as usize].attr("id").map(|v| v.to_string()).as_deref() == Some(id))
}

/// Die Kette vom Wurzelknoten bis zum Ziel — dieselbe Form, die das Layout
/// unter dem Zeiger ausgibt.
fn ancestors(d: &Doc, n: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut cur = Some(n);
    while let Some(x) = cur {
        out.push(x);
        cur = d.nodes[x as usize].parent;
    }
    out.reverse();
    out
}
