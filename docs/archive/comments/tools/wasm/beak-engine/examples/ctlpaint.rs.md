# `tools/wasm/beak-engine/examples/ctlpaint.rs` @ 5e0102684

## L1-14 · `use beak_engine::js::dombind::ScriptRef;`

```
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
```

## L27-28 · `let mut sess = beak_engine::js::Session::new(20_000_000_000);`

```
// Derselbe Deckel wie im Wirt — die Probe soll nicht an einer Grenze
// scheitern, die es am Geraet nicht gibt.
```

## L46-47 · `if let Ok(d) = std::env::var("DEPTH") {`

```
// `DEPTH=` hebt den Aufrufdeckel — die Frage „echte Endlosschleife oder
// nur tiefer als 400?" ist sonst nicht zu beantworten.
```

## L51-52 · `if let Ok(d) = std::env::var("STEPS") {`

```
// `STEPS=` hebt den Schrittdeckel — die Frage „wie teuer ist die Rechnung
// dieser Seite wirklich?" ist sonst nicht zu beantworten.
```

## L76-77 · `if is_mod || is_module(&src) {`

```
// Ein Modul: erst den GANZEN Graphen holen, dann verknuepfen, dann
// auswerten — genau die Reihenfolge, die beak am Geraet faehrt.
```

## L108-112 · `let mut timers = 0;`

```
// **Die Reihenfolge des WIRTS, und sie ist der ganze Punkt.** Erst die
// Stilblattrunden (ein geholtes Blatt laesst eine Komponente fertig
// bauen), dann `DOMContentLoaded`, dann EIN LAYOUT — und erst danach
// `load`. Wer die Geometrie vor den Runden einreicht, misst einen Baum,
// den es so nie gab: die Komponenten sind dann noch leer.
```

## L139-140 · `if std::env::var("DUMP").is_ok() {`

```
// `DUMP=1` zeigt, was am Ende im Baum steht — die Frage „laufen die
// Skripte" ist nicht dieselbe wie „haben sie etwas gebaut".
```

## L149-154 · `if let Ok(spec) = std::env::var("TYPE") {`

```
// `SUBMIT=<id>` faehrt den GANZEN Absendeweg: `submit`-Ereignis, was der
// Behandler ausrechnet, die Auftraege aus `form.submit()`, und am Ende
// die fertige Eingabe. Genau die Reihenfolge, die der Wirt faehrt.
// `TYPE=id=wert[,id=wert]` tippt in Felder, bevor abgeschickt wird — der
// Weg, den der Benutzer nimmt. Geschrieben wird der SCHMUTZIGE Wert, also
// genau das, was ein Tastendruck im Wirt auch setzt.
```

## L195 · `let dom = sess.interp.doc.as_mut().map(|d| d.to_dom()).unwrap();`

```
// Der Baum kann sich geaendert haben — neu einsammeln.
```

## L198 · `if let Some(d) = sess.interp.doc.as_ref() {`

```
// Dieselbe Bruecke wie im Wirt — nicht eine zweite.
```

## L215 · `let Some(dom) = sess.interp.doc.as_mut().map(|d| d.to_dom()) else { return };`

```
// ── Der Vergleich ──────────────────────────────────────────────────────
```

## L227-228 · `eng.set_viewport_h(std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(993));`

```
// `H=` ist keine Kosmetik: `vh` und `min-height:100vh` haengen daran,
// und eine Seite, die ihr Fenster fuellt, liegt sonst 225 px zu hoch.
```

## L261-264 · `if let Ok(v) = std::env::var("STATE0") {`

```
// `STATE0=<wert>`: so, wie das Feld schon AUSGELEGT ist, bevor getippt
// wird. Damit laesst sich der zweite Tastendruck pruefen — der erste
// faellt bei einem Feld, das nichts malt, ohnehin auf ein Auslegen
// zurueck, und danach ist die Spanne nicht mehr leer.
```

## L277-279 · `if std::env::var("DUPES").is_ok() {`

```
// `DUPES=1`: welche Elemente MEHRFACH einen Kasten aufgezeichnet haben.
// `getBoundingClientRect` gibt die VEREINIGUNG (`node_box`), also macht ein
// zweiter Eintrag den Kasten stillschweigend groesser.
```

## L321 · `let missing: Vec<&String> = fresh.iter().filter(|o| !fast.contains(o)).collect();`

```
// Was der Schnellweg VERLOREN hat, egal an welcher Stelle.
```

## L370-371 · `fn is_module(src: &str) -> bool {`

```
/// Hat der Quelltext `import`/`export` auf oberster Ebene? Die Datei sagt es
/// nicht, also entscheidet der Parser: was NUR als Modul parst, ist eines.
```

## L376-378 · `fn origin() -> String {`

```
/// Der Ursprung, gegen den Modul-Adressen absolut werden. `import.meta.url`
/// muss eine ECHTE Adresse sein — die Komponenten der Fritzbox bauen daraus
/// `new URL(x, import.meta.url)`, und ein blosser Pfad ist da keine Basis.
```

## L390-396 · `fn resolve_path(base: &str, spec: &str) -> String {`

```
/// Eine Adresse gegen die des Importeurs aufloesen.
///
/// **Dieselbe Funktion, die beak am Geraet faehrt.** Die Probe hatte hier
/// erst ihre eigene — und die normalisierte `.`/`..`, waehrend beaks Wirt es
/// nicht tat. Ergebnis: host-seitig lief der Modulgraph, am Geraet explodierte
/// er (106 geladen, 179 offen). Eine Probe, die einen ANDEREN Pfad misst als
/// das Ziel, ist keine ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L405-406 · `fn local(dir: &str, url: &str) -> String {`

```
/// Wo die Datei zu einer Adresse liegt: `curl` hat sie unter dem Pfad mit
/// `_` statt `/` abgelegt.
```

## L418 · `let mut queue = vec![entry.clone()];`

```
// Holen, bis der Graph geschlossen ist.
```

## L443 · `fn dump(e: &beak_engine::dom::Element, depth: usize, out: &mut String) {`

```
/// Den Baum als Umriss: Marke, id/class, und Text gekuerzt.
```

## L465 · `fn find_seq(el: &beak_engine::dom::Element, id: &str) -> Option<u32> {`

```
/// Die `seq` des Elements mit dieser id.
```

## L476-477 · `fn collect_links(el: &beak_engine::dom::Element, dir: &str, out: &mut String, n: &mut usize) {`

```
/// Jedes `<link rel=stylesheet>` im Baum, in Baumreihenfolge, aus dem
/// Verzeichnis gelesen. Dieselbe Reihenfolge, in der der Wirt sie anhaengt.
```

## L497 · `fn to_bmp(px: &[u8], w: u32, h: u32) -> Vec<u8> {`

```
/// BGRA nach BMP, von unten nach oben — wie das Format es will.
```

## L522-523 · `fn feed_geometry(sess: &mut beak_engine::js::Session, html: &str, dir: &str) {`

```
/// Einmal auslegen und der Maschine die Kaesten reichen — sonst antwortet
/// `getBoundingClientRect` mit Nullen, und eine Probe, die misst, misst nichts.
```

## L535-537 · `eng.set_hit_all(true);`

```
// Ohne diese Zeile zeichnet das Layout gar keine Element-Kaesten auf, und
// `element_rects()` ist leer — derselbe Schalter, den der Wirt setzt,
// sobald eine Seite Skripte faehrt.
```

## L541-543 · `let (mut ok, mut bad) = (0usize, 0usize);`

```
// Die Schriften der Seite holen und NOCHMAL auslegen — dieselbe Runde,
// die der Wirt faehrt. Ohne den zweiten Lauf misst die Probe mit der
// eingebauten Schrift und vergleicht dann Breiten, die es nicht gibt.
```

