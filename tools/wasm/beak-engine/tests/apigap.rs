//! Was von den WIRKLICH gerufenen DOM-Schnittstellen fehlt — nach Aufrufzahl.
//!
//! Die Rangfolge kommt nicht aus dem Bauch, sondern aus einer Chromium-Messung
//! auf denselben zwoelf Zielseiten: `tools/jsscope/out/apicensus.json` haelt
//! je Seite fest, welche DOM-Schnittstelle wie oft gerufen wurde, getrennt
//! nach Laden und Bedienen. Dieser Test setzt jeden Eintrag gegen das, was
//! die Engine hat, und gibt die Luecke geordnet aus.
//!
//! Warum das noetig war: der erste Anlauf dieser Runde baute nach der
//! test262-Fehlerkarte. Die nannte Generatoren als groesste Luecke — im
//! echten Korpus stirbt daran KEIN einziges Skript. Der Zensus nannte
//! stattdessen `addEventListener` (33 360 Aufrufe) und `atob` (10 426).
//!
//!   APICENSUS=<tools>/jsscope/out/apicensus.json \
//!     cargo test --test apigap -- --nocapture

use std::collections::BTreeMap;

#[test]
fn api_gap() {
    let Ok(path) = std::env::var("APICENSUS") else {
        println!("[apigap] uebersprungen — setze APICENSUS=<tools/jsscope/out/apicensus.json>.");
        return;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        println!("[apigap] {path} nicht lesbar"); return;
    };
    // Kein JSON-Crate in den Abhaengigkeiten und keins noetig: gebraucht
    // werden nur die `"Iface.member": zahl`-Paare, und die stehen flach da.
    let mut agg: BTreeMap<String, u64> = BTreeMap::new();
    // Nach `split('"')` stehen die Zeichenketten auf den UNGERADEN Plaetzen,
    // und was danach kommt, auf dem naechsten geraden.
    let parts: Vec<&str> = raw.split('"').collect();
    let mut k = 1;
    while k + 1 < parts.len() {
        let key = parts[k];
        let tail = parts[k + 1];
        k += 2;
        if !key.contains('.') { continue }
        let Some(rest) = tail.trim_start().strip_prefix(':') else { continue };
        let num: String = rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
        if num.is_empty() { continue }
        let name = key.trim_end_matches(" get").trim_end_matches(" set").to_string();
        *agg.entry(name).or_default() += num.parse::<u64>().unwrap_or(0);
    }

    // Die Probe laeuft IN der Engine: nur sie weiss, was sie hat.
    let mut src = String::from(PROBE);
    for (k, v) in &agg {
        let Some((iface, member)) = k.split_once('.') else { continue };
        if member.contains('.') { continue }
        src.push_str(&format!("chk({iface:?},{member:?},{v});\n"));
    }
    src.push_str("report();\n");

    let dom = beak_engine::dom::parse("<html><body></body></html>");
    let mut i = beak_engine::js::interp::Interp::new();
    i.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
    i.set_media(1280.0, 800.0, false);
    let prog = beak_engine::js::parse(&src, false).expect("Probe parst");
    if let Err(beak_engine::js::interp::Abrupt::Throw(v)) = i.run_program(&prog) {
        let m = i.get(&v, "message").ok().and_then(|m| i.to_string(&m).ok());
        panic!("Probe warf: {}", m.as_deref().unwrap_or("?"));
    }
    println!("\n── DOM-Aufrufzensus: was die Engine deckt ──\n");
    for line in i.take_console() { println!("   {line}"); }
}

const PROBE: &str = r#"
// **Vorhanden ist nicht dasselbe wie beantwortet.** Diese Namen GIBT es auf
// dem Prototyp, aber sie liefern eine feste Zahl statt einer gemessenen — und
// eine 0 sieht aus wie eine Antwort, also faellt niemandem etwas auf: die
// Seite malt nur falsch. Als „gedeckt" gezaehlt haben sie die Zahl oben um
// 2524 Aufrufe geschoent, den GROESSTEN Posten der ganzen Luecke.
//
// Wer hier etwas einbaut, streicht den Namen aus dieser Liste — und wer einen
// neuen Platzhalter einbaut, traegt ihn EIN. Sonst misst diese Probe wieder
// sich selbst.
var STUB = {
  "getBoundingClientRect": 1, "getClientRects": 1, "offsetParent": 1,
  "offsetWidth": 1, "offsetHeight": 1, "offsetTop": 1, "offsetLeft": 1,
  "clientWidth": 1, "clientHeight": 1,
  "scrollWidth": 1, "scrollHeight": 1, "scrollTop": 1, "scrollLeft": 1
};
var missing = [], have = 0, total = 0;
function chk(iface, member, count) {
  total += count;
  var host = iface === "window" ? globalThis
           : (typeof globalThis[iface] === "function" ? globalThis[iface].prototype : null);
  if (!host) { missing.push([count, iface + "." + member + "   (Schnittstelle fehlt ganz)"]); return; }
  // `STUB[member]` waere falsch: fuer `toString` faende es das ERBSTUECK von
  // `Object.prototype` und erklaerte einen intakten Namen zum Platzhalter.
  if (Object.prototype.hasOwnProperty.call(STUB, member)) {
    missing.push([count, iface + "." + member + "   (da, aber antwortet 0)"]); return;
  }
  var o = host, found = false;
  while (o) { if (Object.getOwnPropertyDescriptor(o, member)) { found = true; break; }
              o = Object.getPrototypeOf(o); }
  if (found) have += count; else missing.push([count, iface + "." + member]);
}
function report() {
  console.log("gedeckt: " + have + " von " + total + " Aufrufen ("
              + Math.round(have * 1000 / total) / 10 + " %)");
  missing.sort(function (a, b) { return b[0] - a[0]; });
  for (var i = 0; i < missing.length && i < 40; i++)
    console.log(missing[i][0] + "  " + missing[i][1]);
}
"#;
