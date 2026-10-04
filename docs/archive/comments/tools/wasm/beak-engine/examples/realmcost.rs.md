# `tools/wasm/beak-engine/examples/realmcost.rs` @ 5e0102684

## L1-13 · `fn rss_kb() -> u64 {`

```
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
```

