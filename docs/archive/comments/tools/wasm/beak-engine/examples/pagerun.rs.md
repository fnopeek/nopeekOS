# `tools/wasm/beak-engine/examples/pagerun.rs` @ 5e0102684

## L1-13 · `use beak_engine::js::dombind::ScriptRef;`

```
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
```

## L16-25 · `fn serve_fetches(sess: &mut beak_engine::js::Session, dir: &str) -> usize {`

```
/// Ein Wirt fuer `fetch`, aus dem Spiegelverzeichnis.
///
/// **Damit laesst sich die API-Schicht einer Seite host-seitig fahren.** Die
/// Fritzbox-Oberflaeche baut ihr `rest-helper.js` schon im MODULKOPF auf
/// einem `AbortController` — das Modul scheiterte daran, bevor irgendetwas
/// von seinem Inhalt lief, und `fetch` gab es daneben auch nicht.
///
/// Was der Wirt hier NICHT tut: raten. Eine Datei, die es nicht gibt, wird
/// zur Ablehnung mit `TypeError`, genau wie ein Netzfehler im Browser — nicht
/// zu einer leeren 200er-Antwort.
```

## L48-54 · `fn serve_dyn_scripts(sess: &mut beak_engine::js::Session, dir: &str) -> usize {`

```
/// Die `<script src=…>`, die ein Skript eingehaengt hat — aus dem
/// Spiegelverzeichnis bedient, wie der Wirt sie aus dem Netz bedient.
///
/// **Ohne diese Zeilen misst die Probe eine andere Plattform als das
/// Geraet**: jeder code-geteilte Bundler laedt so nach, und ein Versprechen,
/// das nie faellt, sieht host-seitig wie eine haengende Seite aus
/// ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L72-74 · `fn host_random(out: &mut [u8]) -> bool {`

```
/// Zufall fuer die HOST-Werkzeuge — aus `/dev/urandom`, nicht aus einer
/// Bequemlichkeit. Ohne sie gaebe es hier kein `crypto`, und dann misst das
/// Werkzeug eine andere Plattform als das Geraet.
```

## L83-86 · `struct Loud;`

```
/// Ein Allokator, der GROSSE Anforderungen meldet — mit Rueckwaertsspur.
///
/// Ein Leck sieht man am RSS, aber nicht, WER es anfordert: ein Abtastprofil
/// zeigt Rechenzeit, und eine einzige Allokation von 1,8 GB kostet keine.
```

## L89-94 · `pub static LIVE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);`

```
/// Was gerade WIRKLICH belegt ist — Allokationen minus Freigaben.
///
/// **Der RSS ist die falsche Zahl.** Er zeigt, was der Wirtsallokator vom
/// System behalten hat, nicht was die Seite haelt; auf dem Geraet entscheidet
/// aber die zweite. Der Unterschied war auf DuckDuckGos Ergebnisseite der
/// zwischen 1,4 GB und dem, was wirklich lebt.
```

## L130-134 · `fn rss_mb() -> u64 {`

```
/// Die ECHTE Uhr fuer die Probe. Ohne sie ist `performance.now()` ein
/// Aufrufzaehler, und dann misst das Werkzeug eine andere Plattform als das
/// Geraet ([[feedback_the_test_path_must_be_the_real_path]]).
/// Was der Prozess gerade belegt — die einzige Zahl, die einen Leck-Verdacht
/// bestaetigt oder ausraeumt.
```

## L164-165 · `let mut sess = beak_engine::js::Session::new(20_000_000_000);`

```
// Derselbe Deckel wie im Wirt — die Probe soll nicht an einer Grenze
// scheitern, die es am Geraet nicht gibt.
```

## L167-169 · `if std::env::var("NOVM").is_ok() { sess.interp.vm_off = true; }`

```
// `NOVM=1`: den Baumlaeufer erzwingen. Der Vergleich beantwortet die
// Frage, die kein Zaehler beantwortet — laeuft der heisse Code der Seite
// ueberhaupt auf der Befehlsmaschine?
```

## L173-178 · `let mut linked = String::new();`

```
// **Die verlinkten Blaetter gehoeren in den Stilkontext.** Ohne sie
// antwortet `getComputedStyle` nur aus den `<style>`-Bloecken der Seite,
// und jede Bootstrap-Klasse sieht aus, als gaebe es sie nicht: `.row`
// meldete `block` statt `flex` — ein Fehler der PROBE, der wie ein
// Kaskadenfehler in beak aussah
// ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L196-199 · `let dark = std::env::var("DARK").is_ok();`

```
// `DARK=1` faehrt die Probe im Dunkelmodus — dieselbe Lage, die der Wirt
// aus `query_theme().is_dark()` einreicht. Ohne den Schalter misst man
// immer hell und kann nicht sagen, ob eine Seite das Schema ueberhaupt
// liest.
```

## L202-203 · `if let Ok(d) = std::env::var("DEPTH") {`

```
// `DEPTH=` hebt den Aufrufdeckel — die Frage „echte Endlosschleife oder
// nur tiefer als 400?" ist sonst nicht zu beantworten.
```

## L207-208 · `if let Ok(d) = std::env::var("STEPS") {`

```
// `STEPS=` hebt den Schrittdeckel — die Frage „wie teuer ist die Rechnung
// dieser Seite wirklich?" ist sonst nicht zu beantworten.
```

## L213-214 · `if std::env::var("NORELAYOUT").is_err() {`

```
// **Neu auslegen auf Verlangen.** `NORELAYOUT=1` nimmt es heraus — fuer
// das A/B, und damit eine Messung sagen kann, WAS sie misst.
```

## L221-223 · `if let Ok(v) = std::env::var("BUDGET") {`

```
// `BUDGET=<sekunden>`: dieselbe Frist, die der Wirt am Geraet stellt.
// Ohne sie laeuft eine Seite host-seitig unbegrenzt, und „haengt" ist
// dann keine Messung, sondern ein Abbruch durch die Uhr daneben.
```

## L237-242 · `match std::fs::read_to_string(local(&dir, &u)) {`

```
// **Denselben Weg wie die Blaetter**: `local` schneidet die
// Abfrage ab und legt den Pfad flach, wie `mirror.py` es tut.
// Der alte Weg nahm nur den Dateinamen — `js/main.js?v=1`
// wurde `main.js?v=1`, und die Probe meldete „nicht im
// Verzeichnis" fuer eine Datei, die daliegt
// ([[feedback_the_probe_must_use_the_targets_resolver]]).
```

## L254-255 · `if is_mod || is_module(&src) {`

```
// Ein Modul: erst den GANZEN Graphen holen, dann verknuepfen, dann
// auswerten — genau die Reihenfolge, die beak am Geraet faehrt.
```

## L276-277 · `sess.interp.current_script = Some(node);`

```
// `document.currentScript` — ein Modul hat keinen (HTML §4.12.1),
// und genau deshalb steht es NUR an diesem Zweig.
```

## L288-292 · `let mut timers = 0;`

```
// **Die Reihenfolge des WIRTS, und sie ist der ganze Punkt.** Erst die
// Stilblattrunden (ein geholtes Blatt laesst eine Komponente fertig
// bauen), dann `DOMContentLoaded`, dann EIN LAYOUT — und erst danach
// `load`. Wer die Geometrie vor den Runden einreicht, misst einen Baum,
// den es so nie gab: die Komponenten sind dann noch leer.
```

## L314-319 · `#[cfg(feature = "heap-census")]`

```
// **Die Halde zaehlt der Allokator, den Zensus die Engine.** Nur
// der zweite haengt an `heap-census`; ohne das Merkmal soll die
// Probe TROTZDEM bauen. Sie tat es seit 0.175.3 nicht mehr, und
// daran fiel der Galerie-Vergleich aus — still, weil `cargo run`
// seinen Baufehler nach stderr schreibt und der Aufrufer stdout
// liest ([[feedback_a_silent_failure_hides_every_bug_upstream_of_it]]).
```

## L335-338 · `fetches += serve_fetches(&mut sess, &dir);`

```
// **Erst bedienen, dann die Uhr laufen lassen.** Wer wartet, darf die
// Zeitgeber nicht vorziehen — sonst faellt webpacks
// Zeitueberschreitung vor der Zustellung des Stuecks, auf das sie
// sich bezieht.
```

## L345-348 · `if std::env::var("GEOMEVERY").is_ok() { feed_geometry(&mut sess.interp, &html, &dir); }`

```
// Siehe `GEOMEVERY` weiter unten: der Wirt misst je Bild, auch
// WAEHREND die Skripte noch laufen. Genau in dieser Runde haengt
// React seine Bausteine ein, und wer hier nicht misst, laesst jedes
// frisch eingehaengte Element `offsetWidth == 0` melden.
```

## L368-373 · `let geom_every = std::env::var("GEOMEVERY").is_ok();`

```
// **`GEOMEVERY=1` legt zwischen zwei Zeitgeber-Runden ein BILD ein.**
// Der Wirt misst je Bild neu (`set_geometry` in `beak/src/lib.rs`); die
// Probe tut es sonst genau einmal, und dann meldet jedes Element, das ein
// Skript spaeter einhaengt, fuer immer `offsetWidth == 0`. Wer eine Seite
// untersucht, die sich selbst vermisst, misst ohne diesen Schalter die
// Probe statt beak.
```

## L387-390 · `if let Some(n) = sess.interp.take_nav() {`

```
// Was die Seite per `location` verlangt hat. Ohne diese Zeile sieht eine
// Seite, die sich selbst weiterschickt, genauso aus wie eine, die nichts
// tut — und genau daran ist Googles Sperrseite eine Woche lang
// vorbeigelaufen.
```

## L398-399 · `if std::env::var("DUMP").is_ok() {`

```
// `DUMP=1` zeigt, was am Ende im Baum steht — die Frage „laufen die
// Skripte" ist nicht dieselbe wie „haben sie etwas gebaut".
```

## L408-413 · `if let Ok(spec) = std::env::var("TYPE") {`

```
// `SUBMIT=<id>` faehrt den GANZEN Absendeweg: `submit`-Ereignis, was der
// Behandler ausrechnet, die Auftraege aus `form.submit()`, und am Ende
// die fertige Eingabe. Genau die Reihenfolge, die der Wirt faehrt.
// `TYPE=id=wert[,id=wert]` tippt in Felder, bevor abgeschickt wird — der
// Weg, den der Benutzer nimmt. Geschrieben wird der SCHMUTZIGE Wert, also
// genau das, was ein Tastendruck im Wirt auch setzt.
```

## L421-426 · `let mut acc = String::new();`

```
// **Zeichen fuer Zeichen, mit den Ereignissen dazu** — das
// ist der Weg des Wirts seit 0.186.0 (`edit_key`). Den Wert
// in einem Zug hineinzuschreiben hiesse, eine Seite zu
// messen, die nie getippt bekommt: die Vorschlagsliste haengt
// an `input`, nicht am Wert
// ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L453-454 · `for _ in 0..32 {`

```
// Was ein Behandler angestossen hat, zu Ende fahren: eine
// Vorschlagsliste holt ihre Antwort mit `fetch`.
```

## L465-471 · `if sess.interp.console.len() > n1 {`

```
// `TEXT=<id>` gibt den Inhalt EINES Elements ungekuerzt aus — der Weg,
// auf dem eine eingehaengte Sonde ihr Ergebnis herausreicht, und zwar
// derselbe, den Chromium mit `--dump-dom` nimmt.
// **Was WAEHREND `TYPE`/`CLICK`/`SUBMIT` gesagt wurde.** Die Zeile oben
// leert die Konsole nach der Skriptrunde; alles, was ein Behandler danach
// schreibt, stand bis 0.186.0 nirgends — und eine Probe, die das Tippen
// misst, sah ihre eigenen Ausgaben nicht.
```

## L484-486 · `#[cfg(feature = "heap-census")]`

```
// `SWEEP=1`: die unerreichbaren Ringe brechen und nachmessen. Die Zahl,
// die sagt, was ein Sammler wirklich braechte — alles davor ist eine
// Schaetzung ueber Objektzahlen.
```

## L529 · `let dom = sess.interp.doc.as_mut().map(|d| d.to_dom()).unwrap();`

```
// Der Baum kann sich geaendert haben — neu einsammeln.
```

## L532 · `if let Some(d) = sess.interp.doc.as_ref() {`

```
// Dieselbe Bruecke wie im Wirt — nicht eine zweite.
```

## L548-552 · `if let Ok(out) = std::env::var("RENDER") {`

```
// `RENDER=<datei.bmp>` malt die Seite so, wie beak sie malt: der
// GESKRIPTETE Baum plus ALLE Stilblaetter, die im Baum stehen — die aus
// dem Quelltext und die, die Skripte nachgelegt haben, in Baumreihenfolge.
// Das ist der einzige Weg, „schaut falsch aus" in etwas Nachpruefbares zu
// verwandeln ([[feedback_trust_the_pixels_not_the_tool]]).
```

## L558 · `collect_links(&dom.root, &dir, &mut css, &mut sheets);`

```
// `<head>` steht nicht unter `body()` — beide Seiten des Baums.
```

## L565-566 · `eng.set_viewport_h(std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(993));`

```
// `H=` ist keine Kosmetik: `vh` und `min-height:100vh` haengen daran,
// und eine Seite, die ihr Fenster fuellt, liegt sonst 225 px zu hoch.
```

## L571-572 · `let inline = eng.load_inline_fonts();`

```
// Dieselbe Runde wie der Wirt: erst die `data:`-Gesichter, die
// gar kein Netz brauchen ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L584-585 · `if std::env::var("IMGOPS").is_ok() {`

```
// `IMGOPS=1` nennt jeden Bildbefehl mit seinem Kasten — die einzige
// Art, einen Bildkasten zu pruefen, der auf 1 px zusammenfaellt.
```

## L602-604 · `if sess.interp.console_dropped > 0 {`

```
// Ein Deckel, der still zuschlaegt, macht jede Bisektion zur Messung des
// Deckels: die letzte gedruckte Marke war die 200., nicht die letzte
// gelaufene ([[feedback_a_read_cap_decides_what_exists]]).
```

## L625-626 · `fn is_module(src: &str) -> bool {`

```
/// Hat der Quelltext `import`/`export` auf oberster Ebene? Die Datei sagt es
/// nicht, also entscheidet der Parser: was NUR als Modul parst, ist eines.
```

## L631-633 · `fn origin() -> String {`

```
/// Der Ursprung, gegen den Modul-Adressen absolut werden. `import.meta.url`
/// muss eine ECHTE Adresse sein — die Komponenten der Fritzbox bauen daraus
/// `new URL(x, import.meta.url)`, und ein blosser Pfad ist da keine Basis.
```

## L645-651 · `fn resolve_path(base: &str, spec: &str) -> String {`

```
/// Eine Adresse gegen die des Importeurs aufloesen.
///
/// **Dieselbe Funktion, die beak am Geraet faehrt.** Die Probe hatte hier
/// erst ihre eigene — und die normalisierte `.`/`..`, waehrend beaks Wirt es
/// nicht tat. Ergebnis: host-seitig lief der Modulgraph, am Geraet explodierte
/// er (106 geladen, 179 offen). Eine Probe, die einen ANDEREN Pfad misst als
/// das Ziel, ist keine ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L660-661 · `fn local(dir: &str, url: &str) -> String {`

```
/// Wo die Datei zu einer Adresse liegt: `curl` hat sie unter dem Pfad mit
/// `_` statt `/` abgelegt.
```

## L665-669 · `let p = p.split(['?', '#']).next().unwrap_or(p);`

```
// **Abfrage und Fragment gehoeren nicht in den Dateinamen.** `mirror.py`
// schneidet sie ab (`urlparse(...).path`), und eine Probe, die das nicht
// tut, sucht `css_main.css?v=1` und meldet „Blatt fehlt" fuer eine Datei,
// die daliegt — der Cache-Buster `?v=1` ist auf echten Seiten die Regel
// ([[feedback_the_probe_must_use_the_targets_resolver]]).
```

## L676-679 · `let entry = if label.starts_with("inline ") {`

```
// **Die Einstiegsadresse eines EXTERNEN Moduls ist seine eigene.** Der
// Kunstname `__entry__js/main.js` war eine Basis, gegen die `./state.js`
// zu `__entry__js/state.js` wurde — eine Datei, die es nirgends gibt.
// Nur ein INLINE-Modul hat keine Adresse und braucht eine erfundene.
```

## L687 · `let mut queue = vec![entry.clone()];`

```
// Holen, bis der Graph geschlossen ist.
```

## L712-719 · `fn click_ids(sess: &mut beak_engine::js::Session, html: &str, dir: &str, spec: &str) {`

```
/// `CLICK=<id>[,<id>…]` — einen Knopf druecken, auf dem Weg des WIRTS.
///
/// Nicht `dispatch` auf den Knoten aus dem Baum: beak nimmt die Zustellkette
/// aus dem LAYOUT (`element_chain` auf der Mitte des gemalten Kastens), und
/// genau dort ist schon einmal ein Klick gestorben, den der Baumweg als
/// zugestellt meldete ([[feedback_the_test_path_must_be_the_real_path]]).
/// Auch der Schnellweg-Wächter des Wirts steht hier — sonst behauptet die
/// Probe eine Zustellung, die am Geraet gar nicht erst versucht wird.
```

## L724-725 · `let Some(lay) = page_layout(&mut sess.interp, html, dir) else { return };`

```
// Je Klick neu auslegen — ein Behandler, der den Baum aendert,
// verschiebt die Kaesten fuer den naechsten.
```

## L763-769 · `fn raw_text(e: &beak_engine::dom::Element, out: &mut String) {`

```
/// Der ROHE Text eines Elements — ohne Kuerzung, ohne Umbau.
///
/// `DUMP=1` schneidet Text bei 60 Zeichen ab; fuer eine Sonde, die ihr
/// Ergebnis in ein `<pre>` schreibt, ist das wertlos. **Und die Konsole ist
/// der falsche Weg**: sie haelt 200 Zeilen, danach faellt still weg, was
/// kommt — eine Messung, die auf halber Strecke aufhoert und wie eine Luecke
/// im Layout aussieht ([[feedback_a_read_cap_decides_what_exists]]).
```

## L791 · `fn dump(e: &beak_engine::dom::Element, depth: usize, out: &mut String) {`

```
/// Den Baum als Umriss: Marke, id/class, und Text gekuerzt.
```

## L813 · `fn find_seq(el: &beak_engine::dom::Element, id: &str) -> Option<u32> {`

```
/// Die `seq` des Elements mit dieser id.
```

## L824-826 · `fn push_sheet(text: &str, url: &str, dir: &str, out: &mut String, n: &mut usize, depth: usize) {`

```
/// Ein Blatt anhaengen — seine `@import`e ZUERST, denn ein Import wirkt, als
/// staende sein Inhalt an seiner Stelle, also vor allem, was im Blatt folgt.
/// Dieselbe Reihenfolge, die der Wirt baut.
```

## L842-843 · `fn collect_links(el: &beak_engine::dom::Element, dir: &str, out: &mut String, n: &mut usize) {`

```
/// Jedes `<link rel=stylesheet>` im Baum, in Baumreihenfolge, aus dem
/// Verzeichnis gelesen. Dieselbe Reihenfolge, in der der Wirt sie anhaengt.
```

## L851-856 · `push_sheet(&t, &u, dir, out, n, 0);`

```
// **`@import` MUSS die Probe auch fahren.** Der Wirt tut es
// seit 0.139.0; eine Probe, die es nicht tut, misst sich
// selbst und nicht beak — bei sandbox.nopeek.ch haengt die
// ganze Gestaltung an fuenfzehn `@import`-Zeilen, und ohne
// sie sieht jeder Vergleich wie ein Layoutfehler aus
// ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L868 · `fn to_bmp(px: &[u8], w: u32, h: u32) -> Vec<u8> {`

```
/// BGRA nach BMP, von unten nach oben — wie das Format es will.
```

## L893-898 · `thread_local! {`

```
/// Einmal auslegen und der Maschine die Kaesten reichen — sonst antwortet
/// `getBoundingClientRect` mit Nullen, und eine Probe, die misst, misst nichts.
/// Die Seite auslegen, wie der Wirt sie auslegt: der GESKRIPTETE Baum, alle
/// Blaetter aus dem Baum, die Schriftrunde. Zwei Aufrufer — die Geometrie fuer
/// `getBoundingClientRect`, und der Klickpunkt fuer `CLICK`. **Eine Quelle**,
/// sonst misst die eine Seite etwas anderes als die andere.
```

## L900-903 · `static HTML: core::cell::RefCell<String> = const { core::cell::RefCell::new(String::new()) };`

```
/// Das HTML und das Spiegelverzeichnis fuer den Relayout-Haken. Ein
/// `fn`-Zeiger faengt nichts ein, also muessen die zwei Dinge, die
/// `feed_geometry` braucht, hier stehen — beim Wirt sind es ohnehin
/// Globale (`html_str()`, `css_str()`).
```

## L908-911 · `fn host_relayout(ip: &mut beak_engine::js::interp::Interp) {`

```
/// **Neu auslegen auf Verlangen** — derselbe Weg, den der Wirt je Bild faehrt,
/// nur jetzt aus der Maschine heraus gerufen. Ohne ihn misst die Probe eine
/// Engine ohne diesen Weg und findet den Fehler nicht, den sie suchen soll
/// ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L920-921 · `fn mono_us() -> u64 {`

```
/// Mikrosekunden seit Prozessstart — die Uhr, aus der `Layout::phase` seine
/// drei Zahlen rechnet. Ohne sie steht dort dreimal null.
```

## L944-947 · `thread_local! {`

```
// **EINE Engine fuer alle Auslegungen, wie beim Wirt.** Eine frische je
// Messung hat jeden Zwischenspeicher kalt — Dokument, Blatt, Schriften —
// und die Probe misst dann den ersten Aufbau statt das Neuauslegen, das
// sie messen will ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L951-953 · `e.set_theme(if std::env::var("DARK").is_ok() {`

```
// Das Thema entscheidet `prefers-color-scheme` in der Kaskade —
// es MUSS zur Medienlage passen, sonst rechnet das Layout mit
// einem anderen Schema als das Skript liest.
```

## L965-968 · `eng.set_viewport_h(std::env::var("H").ok().and_then(|v| v.parse().ok()).unwrap_or(993));`

```
// `H=` gehoert zur MESSUNG, nicht zur Kosmetik: `vh` und
// `min-height:100vh` haengen daran. Mit der Vorgabe 600 statt der echten
// Fensterhoehe lag die ganze Fritzbox-Seite 225 px zu hoch, und der
// Vergleich mit Chromium meldete 26 Abweichungen, die es nicht gibt.
```

## L970-972 · `eng.set_hit_all(true);`

```
// Ohne diese Zeile zeichnet das Layout gar keine Element-Kaesten auf, und
// `element_rects()` ist leer — derselbe Schalter, den der Wirt setzt,
// sobald eine Seite Skripte faehrt.
```

## L980-982 · `let (mut ok, mut bad) = (0usize, 0usize);`

```
// Die Schriften der Seite holen und NOCHMAL auslegen — dieselbe Runde,
// die der Wirt faehrt. Ohne den zweiten Lauf misst die Probe mit der
// eingebauten Schrift und vergleicht dann Breiten, die es nicht gibt.
```

## L999-1002 · `if std::env::var("PHASEDBG").is_ok() {`

```
// **Wo die Zeit eines erzwungenen Neuauslegens stuende.** `to_dom` baut den
// Baum zurueck, `collect_links` sammelt die Blaetter, `layout_ext` parst
// das HTML, faehrt die Kaskade und legt die Kaesten. Drei Zahlen statt
// einer, weil nur eine davon unvermeidlich ist.
```

