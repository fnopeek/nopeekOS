// Wieviel kostet ein Realm, und wird er beim Fallenlassen frei?
//
// Gebaut am 2026-09-04, nachdem der test262-Lauf mit 59 GB vom OOM-Killer
// erschossen wurde. Die Antwort war: 973 KB je Realm, und NICHTS wurde frei —
// die Form eines JS-Realms ist ringfoermig, und `Rc` kommt aus einem Ring nie
// auf null. Seit `Interp::teardown` misst die dritte Zeile +0 KB; wer an den
// Prototypen oder am globalen Gegenstand etwas hinzufuegt, prueft sie hier.
//
//   cargo run --release --example realmcost      (N=<zahl> fuer mehr Laeufe)
//
// Die erste Zeile misst den PREIS eines gehaltenen Realms, die zweite sagt
// nichts ueber ein Leck (der Zuteiler gibt nicht an das System zurueck), und
// die dritte ist die eigentliche Frage: kostet der n+1-te Realm noch etwas?
fn rss_kb() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap();
    for l in s.lines() {
        if let Some(r) = l.strip_prefix("VmRSS:") {
            return r.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        }
    }
    0
}
fn main() {
    let n: usize = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let base = rss_kb();
    {
        let mut v = Vec::new();
        for _ in 0..n { v.push(beak_engine::js::interp::Interp::new()); }
        println!("{n} Interps GEHALTEN: +{} KB  = {} KB je Realm", rss_kb() - base, (rss_kb() - base) / n as u64);
    }
    println!("nach dem Fallenlassen:     +{} KB", rss_kb() - base);
    let b2 = rss_kb();
    for _ in 0..n { let _ = beak_engine::js::interp::Interp::new(); }
    println!("{n} einzeln erzeugt+fallen: +{} KB", rss_kb() - b2);
}
