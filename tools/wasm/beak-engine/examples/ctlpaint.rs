//! Der SCHNELLWEG beim Tippen, gegen ein volles Auslegen gehalten.
//!
//! `repaint_controls` ersetzt die Befehle EINES Steuerelements an Ort und
//! Stelle. Stimmt seine Spanne nicht, frisst die Ersetzung den Nachbarn — und
//! am Geraet sieht das aus wie „der Text daneben wird ausgeblendet".
//!
//! Gefahren wird die Reihenfolge des WIRTS, nicht eine zweite:
//!   1. auslegen, wie die Seite ankommt (leerer Zustand)
//!   2. in ein Feld tippen und den Fokus setzen
//!   3. `repaint_controls` — der Weg jedes Tastendrucks
//!   4. dasselbe noch einmal GANZ auslegen
//!   5. beide Befehlslisten Zeile fuer Zeile vergleichen
//!
//!   URL=… W=1605 CTL=<id> TEXT=abc cargo run --release --example ctlpaint -- seite.html dir/

use beak_engine::js::dombind::ScriptRef;

fn main() {
    let html_path = std::env::args().nth(1).unwrap_or_default();
    let dir = std::env::args().nth(2).unwrap_or_else(|| ".".into());
    let html = std::fs::read_to_string(&html_path).expect("HTML-Datei");

    let dom = beak_engine::dom::parse(&html);
    let doc = beak_engine::js::dombind::Doc::from_dom(&dom);
    let refs = beak_engine::js::dombind::page_scripts(&doc);

    // Derselbe Deckel wie im Wirt — die Probe soll nicht an einer Grenze
    // scheitern, die es am Geraet nicht gibt.
    let mut sess = beak_engine::js::Session::new(20_000_000_000);
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
    // `STEPS=` hebt den Schrittdeckel — die Frage „wie teuer ist die Rechnung
    // dieser Seite wirklich?" ist sonst nicht zu beantworten.
    if let Ok(d) = std::env::var("STEPS") {
        if let Ok(n) = d.parse() { sess.interp.max_steps = n; }
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
    // **Die Reihenfolge des WIRTS, und sie ist der ganze Punkt.** Erst die
    // Stilblattrunden (ein geholtes Blatt laesst eine Komponente fertig
    // bauen), dann `DOMContentLoaded`, dann EIN LAYOUT — und erst danach
    // `load`. Wer die Geometrie vor den Runden einreicht, misst einen Baum,
    // den es so nie gab: die Komponenten sind dann noch leer.
    let mut timers = 0;
    let (mut sheets_ok, mut sheets_bad) = (0usize, 0usize);
    for _ in 0..64 {
        let t = sess.interp.run_timers();
        timers += t;
        let want = sess.interp.take_pending_sheets();
        if t == 0 && want.is_empty() { break }
        for (id, href) in want {
            let u = resolve_path(&format!("{}/", origin()), &href);
            let ok = std::fs::read_to_string(local(&dir, &u)).is_ok();
            if ok { sheets_ok += 1 } else { sheets_bad += 1 }
            beak_engine::js::dombind::sheet_done(&mut sess.interp, id, ok);
        }
    }
    if sheets_ok + sheets_bad > 0 {
        println!("Stilblaetter per Skript: {sheets_ok} geholt, {sheets_bad} gescheitert");
    }
    if let Some(dn) = sess.interp.doc.as_ref().map(|d| d.doc) {
        let _ = beak_engine::js::dombind::dispatch(&mut sess.interp, "DOMContentLoaded", &[dn]);
    }
    feed_geometry(&mut sess, &html, &dir);
    if let Some(dn) = sess.interp.doc.as_ref().map(|d| d.doc) {
        let _ = beak_engine::js::dombind::dispatch(&mut sess.interp, "load", &[dn]);
    }
    for _ in 0..64 { let t = sess.interp.run_timers(); timers += t; if t == 0 { break } }
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
    // `SUBMIT=<id>` faehrt den GANZEN Absendeweg: `submit`-Ereignis, was der
    // Behandler ausrechnet, die Auftraege aus `form.submit()`, und am Ende
    // die fertige Eingabe. Genau die Reihenfolge, die der Wirt faehrt.
    // `TYPE=id=wert[,id=wert]` tippt in Felder, bevor abgeschickt wird — der
    // Weg, den der Benutzer nimmt. Geschrieben wird der SCHMUTZIGE Wert, also
    // genau das, was ein Tastendruck im Wirt auch setzt.
    if let Ok(spec) = std::env::var("TYPE") {
        let dom = sess.interp.doc.as_mut().map(|d| d.to_dom());
        if let Some(dom) = dom {
            for pair in spec.split(',') {
                let Some((id, v)) = pair.split_once('=') else { continue };
                let Some(seq) = find_seq(dom.body(), id) else {
                    println!("TYPE: kein Element mit id={id}"); continue };
                if let Some(d) = sess.interp.doc.as_mut() {
                    if let Some(n) = d.by_seq(seq) {
                        d.nodes[n as usize].value = Some(std::rc::Rc::from(v));
                        d.touch();
                    }
                }
            }
        }
    }
    if let Ok(want) = std::env::var("SUBMIT") {
        let dom = sess.interp.doc.as_mut().map(|d| d.to_dom());
        let Some(dom) = dom else { return };
        let forms = beak_engine::forms::collect(&dom);
        let mut state = beak_engine::forms::FormState::default();
        let seq = find_seq(dom.body(), &want);
        match seq {
            None => println!("\nSUBMIT: kein Element mit id={want}"),
            Some(seq) => {
                let t0 = std::time::Instant::now();
                let s0 = sess.interp.steps;
                let prevented = beak_engine::js::dombind::dispatch_seq(&mut sess.interp, "submit", seq);
                let mut n = 0;
                for _ in 0..64 { let t = sess.interp.run_timers(); n += t; if t == 0 { break } }
                println!("  Kosten: {} Schritte, {:?}", sess.interp.steps - s0, t0.elapsed());
                println!("  Maschine: {} Programme gefahren, {} abgelehnt; Aufrufe {} (davon {} langsam, {} nativ)",
                         sess.interp.vm_ran, sess.interp.vm_declined,
                         sess.interp.vm_calls, sess.interp.vm_calls_slow, sess.interp.vm_calls_native);
                let mut d: Vec<(&&'static str, &u64)> = sess.interp.func_declines.iter().collect();
                d.sort_by_key(|(_, n)| core::cmp::Reverse(**n));
                for (why, n) in d.iter().take(6) { println!("    Rumpf abgelehnt: {why} x{n}"); }
                println!("\nSUBMIT auf #{want} (seq {seq}): {}, {n} Zeitgeber",
                         if prevented { "abgefangen" } else { "durchgelassen" });
                for l in &sess.interp.console[n0..] { println!("       | {l}"); }
                // Der Baum kann sich geaendert haben — neu einsammeln.
                let dom = sess.interp.doc.as_mut().map(|d| d.to_dom()).unwrap();
                let forms2 = beak_engine::forms::collect(&dom);
                // Dieselbe Bruecke wie im Wirt — nicht eine zweite.
                if let Some(d) = sess.interp.doc.as_ref() {
                    beak_engine::js::dombind::pull_control_values(d, &forms2, &mut state);
                }
                let asked = sess.interp.take_submits();
                let target = asked.first().copied().or(if prevented { None } else { Some(seq) });
                match target.and_then(|s| beak_engine::forms::submit_form(&forms2, &state, s)) {
                    Some(sub) => println!("  -> {} {}\n     {}",
                                          if sub.method_get { "GET" } else { "POST" },
                                          sub.action, sub.query),
                    None => println!("  -> nichts abgeschickt (Auftraege: {asked:?})"),
                }
                let _ = forms;
            }
        }
    }

    // ── Der Vergleich ──────────────────────────────────────────────────────
    let Some(dom) = sess.interp.doc.as_mut().map(|d| d.to_dom()) else { return };
    let dom2 = sess.interp.doc.as_mut().map(|d| d.to_dom()).unwrap();
    let mut css = String::new();
    let mut sheets = 0;
    collect_links(dom.body(), &dir, &mut css, &mut sheets);
    collect_links(&dom.root, &dir, &mut css, &mut sheets);
    let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
    use beak_engine::layout::{Rgb, Theme};
    let mut eng = beak_engine::Engine::new();
    eng.set_theme(Theme { bg: Rgb(255,255,255), text: Rgb(33,37,41), heading: Rgb(33,37,41),
                  link: Rgb(13,110,253), muted: Rgb(108,117,125), rule: Rgb(222,226,230) });
    // `H=` ist keine Kosmetik: `vh` und `min-height:100vh` haengen daran,
    // und eine Seite, die ihr Fenster fuellt, liegt sonst 225 px zu hoch.
    eng.set_viewport_h(std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(993));
    eng.set_scripted_dom(Some(dom2));
    let empty = beak_engine::forms::FormState::default();
    let mut lay = eng.layout_forms(&html, &css, width, &empty);
    for _ in 0..4 {
        let want = eng.take_pending_fonts();
        if want.is_empty() { break }
        for (url, family, weight, italic) in want {
            let u = resolve_path(&format!("{}/", origin()), &url);
            if let Ok(b) = std::fs::read(local(&dir, &u)) { let _ = eng.add_font(family, weight, italic, &b); }
        }
        lay = eng.layout_forms(&html, &css, width, &empty);
    }
    let forms = beak_engine::forms::collect(&dom);
    let want_id = std::env::var("CTL").unwrap_or_default();
    let text = std::env::var("TEXT").unwrap_or_else(|_| "abc".into());
    let seq = match want_id.strip_prefix("seq:").and_then(|v| v.parse::<u32>().ok())
        .or_else(|| find_seq(dom.body(), &want_id)) {
        Some(s) => s,
        None => { println!("CTL: kein Element mit id={want_id}; Steuerelemente:"); 
                  for c in &forms.controls { println!("   seq={} {:?}", c.seq, c.kind); } return }
    };
    let mut state = beak_engine::forms::FormState::default();
    state.focus = Some(seq);
    state.set_value(seq, text.clone());
    state.caret = text.len();

    for (c, (seq, at, len)) in lay.controls.iter().zip(lay.control_spans()) {
        println!("  seq={seq} {:?} {},{} {}x{} — Befehle {at}..{} ({len} Stueck)",
                 c.kind, c.x, c.y, c.w, c.h, at + len);
    }
    // `STATE0=<wert>`: so, wie das Feld schon AUSGELEGT ist, bevor getippt
    // wird. Damit laesst sich der zweite Tastendruck pruefen — der erste
    // faellt bei einem Feld, das nichts malt, ohnehin auf ein Auslegen
    // zurueck, und danach ist die Spanne nicht mehr leer.
    if let Ok(v) = std::env::var("STATE0") {
        let mut s0 = beak_engine::forms::FormState::default();
        s0.focus = Some(seq);
        s0.set_value(seq, v.clone());
        s0.caret = v.len();
        lay = eng.layout_forms(&html, &css, width, &s0);
        println!("  (Ausgangslage: #{seq} = {v:?})");
        for (c, (sq, at, len)) in lay.controls.iter().zip(lay.control_spans()) {
            println!("  seq={sq} {:?} {},{} {}x{} — Befehle {at}..{} ({len} Stueck)",
                     c.kind, c.x, c.y, c.w, c.h, at + len);
        }
    }
    let before = dump_ops(&lay);
    if std::env::var("FULL").is_ok() {
        println!("\n── vorher ──");
        for (i, o) in before.iter().enumerate() { println!("  [{i:2}] {o}"); }
    }
    let ok = eng.repaint_controls(&mut lay, &state);
    let fast = dump_ops(&lay);
    let fresh = dump_ops(&eng.layout_forms(&html, &css, width, &state));

    if ok { println!("\nSchnellweg: gelaufen"); } else { println!("\nSchnellweg: ABGELEHNT ({})", eng.repaint_bail()); }
    println!("Befehle: {} vorher, {} nach dem Neumalen, {} beim vollen Auslegen",
             before.len(), fast.len(), fresh.len());
    if std::env::var("FULL").is_ok() {
        println!("\n── schnell ──");
        for (i, o) in fast.iter().enumerate() { println!("  [{i:2}] {o}"); }
        println!("\n── ausgelegt ──");
        for (i, o) in fresh.iter().enumerate() { println!("  [{i:2}] {o}"); }
    }
    if fast == fresh { println!("\n✓ Schnellweg == volles Auslegen"); return }
    println!("\n✗ Sie gehen auseinander:\n");
    let n = fast.len().max(fresh.len());
    let mut shown = 0;
    for i in 0..n {
        let a = fast.get(i).map(|s| s.as_str()).unwrap_or("—");
        let b = fresh.get(i).map(|s| s.as_str()).unwrap_or("—");
        if a == b { continue }
        println!("  [{i}]\n     schnell: {a}\n     ausgelegt: {b}");
        shown += 1;
        if shown >= 25 { println!("  … abgeschnitten"); break }
    }
    // Was der Schnellweg VERLOREN hat, egal an welcher Stelle.
    let missing: Vec<&String> = fresh.iter().filter(|o| !fast.contains(o)).collect();
    let extra: Vec<&String> = fast.iter().filter(|o| !fresh.contains(o)).collect();
    if !missing.is_empty() {
        println!("\n  NUR im ausgelegten Bild ({}):", missing.len());
        for m in missing.iter().take(12) { println!("     {m}"); }
    }
    if !extra.is_empty() {
        println!("\n  NUR im schnell gemalten ({}):", extra.len());
        for m in extra.iter().take(12) { println!("     {m}"); }
    }
}

fn dump_ops(l: &beak_engine::layout::Layout) -> Vec<String> {
    use beak_engine::layout::DrawOp;
    use std::fmt::Write as _;
    let mut v = Vec::new();
    for o in &l.ops {
        let mut s = String::new();
        match o {
            DrawOp::Text { x, y, size, color, text, .. } =>
                { let _ = write!(s, "T {x},{y} {size:.1} {color:?} {text:?}"); }
            DrawOp::Rect { x, y, w, h, color } => { let _ = write!(s, "R {x},{y} {w}x{h} {color:?}"); }
            DrawOp::RoundRect { x, y, w, h, color, ring, .. } =>
                { let _ = write!(s, "Q {x},{y} {w}x{h} {color:?} ring={ring:.1}"); }
            DrawOp::Shadow { x, y, w, h, .. } => { let _ = write!(s, "S {x},{y} {w}x{h}"); }
            DrawOp::Image { x, y, w, h, src, .. } => { let _ = write!(s, "I {x},{y} {w}x{h} {src}"); }
            DrawOp::BgImage { x, y, w, h, key, .. } => { let _ = write!(s, "B {x},{y} {w}x{h} {key}"); }
            DrawOp::Gradient { x, y, w, h, .. } => { let _ = write!(s, "G {x},{y} {w}x{h}"); }
        }
        v.push(s);
    }
    v
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
///
/// **Dieselbe Funktion, die beak am Geraet faehrt.** Die Probe hatte hier
/// erst ihre eigene — und die normalisierte `.`/`..`, waehrend beaks Wirt es
/// nicht tat. Ergebnis: host-seitig lief der Modulgraph, am Geraet explodierte
/// er (106 geladen, 179 offen). Eine Probe, die einen ANDEREN Pfad misst als
/// das Ziel, ist keine ([[feedback_the_test_path_must_be_the_real_path]]).
fn resolve_path(base: &str, spec: &str) -> String {
    use beak_engine::js::url;
    match url::parse_abs(base) {
        Some(b) => url::resolve(spec, &b).href(),
        None => spec.to_string(),
    }
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

/// Die `seq` des Elements mit dieser id.
fn find_seq(el: &beak_engine::dom::Element, id: &str) -> Option<u32> {
    if el.attr("id") == Some(id) { return Some(el.seq) }
    for c in &el.children {
        if let beak_engine::dom::Node::Element(e) = c {
            if let Some(s) = find_seq(e, id) { return Some(s) }
        }
    }
    None
}

/// Jedes `<link rel=stylesheet>` im Baum, in Baumreihenfolge, aus dem
/// Verzeichnis gelesen. Dieselbe Reihenfolge, in der der Wirt sie anhaengt.
fn collect_links(el: &beak_engine::dom::Element, dir: &str, out: &mut String, n: &mut usize) {
    if el.tag == "link"
        && el.attr("rel").is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet")) {
        if let Some(h) = el.attr("href") {
            let u = resolve_path(&format!("{}/", origin()), h);
            if let Ok(t) = std::fs::read_to_string(local(dir, &u)) {
                out.push_str(&t);
                out.push('\n');
                *n += 1;
            } else {
                eprintln!("  Blatt fehlt: {u}");
            }
        }
    }
    for c in &el.children {
        if let beak_engine::dom::Node::Element(e) = c { collect_links(e, dir, out, n); }
    }
}

/// BGRA nach BMP, von unten nach oben — wie das Format es will.
fn to_bmp(px: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = (w * 3 + 3) & !3;
    let size = 54 + (row * h) as usize;
    let mut o = Vec::with_capacity(size);
    o.extend_from_slice(b"BM");
    o.extend_from_slice(&(size as u32).to_le_bytes());
    o.extend_from_slice(&[0; 4]);
    o.extend_from_slice(&54u32.to_le_bytes());
    o.extend_from_slice(&40u32.to_le_bytes());
    o.extend_from_slice(&(w as i32).to_le_bytes());
    o.extend_from_slice(&(h as i32).to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&24u16.to_le_bytes());
    o.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            o.extend_from_slice(&[px[i], px[i + 1], px[i + 2]]);
        }
        for _ in 0..(row - w * 3) { o.push(0); }
    }
    o
}

/// Einmal auslegen und der Maschine die Kaesten reichen — sonst antwortet
/// `getBoundingClientRect` mit Nullen, und eine Probe, die misst, misst nichts.
fn feed_geometry(sess: &mut beak_engine::js::Session, html: &str, dir: &str) {
    let Some(dom) = sess.interp.doc.as_mut().map(|d| d.to_dom()) else { return };
    let mut css = String::new();
    let mut n = 0;
    collect_links(dom.body(), dir, &mut css, &mut n);
    collect_links(&dom.root, dir, &mut css, &mut n);
    let width: u32 = std::env::var("W").ok().and_then(|w| w.parse().ok()).unwrap_or(1902);
    use beak_engine::layout::{Rgb, Theme};
    let mut eng = beak_engine::Engine::new();
    eng.set_theme(Theme { bg: Rgb(255,255,255), text: Rgb(33,37,41), heading: Rgb(33,37,41),
                          link: Rgb(13,110,253), muted: Rgb(108,117,125), rule: Rgb(222,226,230) });
    // Ohne diese Zeile zeichnet das Layout gar keine Element-Kaesten auf, und
    // `element_rects()` ist leer — derselbe Schalter, den der Wirt setzt,
    // sobald eine Seite Skripte faehrt.
    eng.set_hit_all(true);
    eng.set_scripted_dom(Some(dom));
    let mut lay = eng.layout_ext(html, &css, width);
    // Die Schriften der Seite holen und NOCHMAL auslegen — dieselbe Runde,
    // die der Wirt faehrt. Ohne den zweiten Lauf misst die Probe mit der
    // eingebauten Schrift und vergleicht dann Breiten, die es nicht gibt.
    let (mut ok, mut bad) = (0usize, 0usize);
    for _ in 0..4 {
        let want = eng.take_pending_fonts();
        if want.is_empty() { break }
        for (url, family, weight, italic) in want {
            let u = resolve_path(&format!("{}/", origin()), &url);
            match std::fs::read(local(dir, &u)) {
                Ok(b) if eng.add_font(family, weight, italic, &b) => ok += 1,
                _ => { bad += 1; eprintln!("  Schrift nicht ladbar: {u}"); }
            }
        }
        lay = eng.layout_ext(html, &css, width);
    }
    if ok + bad > 0 { println!("Schriften: {ok} geladen, {bad} gescheitert"); }
    let rects = lay.element_rects();
    if std::env::var("GEOMDBG").is_ok() {
        eprintln!("  Geometrie: {} Kaesten, Layouthoehe {}", rects.len(), lay.height);
        for r in rects.iter().take(8) { eprintln!("    seq={} {},{} {}x{}", r.seq, r.x, r.y, r.w, r.h); }
    }
    sess.interp.set_geometry(beak_engine::js::interp::Geometry {
        boxes: std::rc::Rc::new(rects), scroll: (0, 0),
    });
    sess.interp.set_media(width as f64, 1080.0, false);
}
