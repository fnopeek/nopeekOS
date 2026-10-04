# `tools/wasm/beak-engine/tests/test262.rs` @ 5e0102684

## L1-22 · `use std::collections::BTreeMap;`

```
//! test262 als PARSE-Orakel — die Sprachzahl, bevor es eine Auswertung gibt.
//!
//! test262 sagt zu jeder Datei, ob sie gueltiges JavaScript ist:
//!
//! - `negative: { phase: parse }` → der Parser MUSS ablehnen.
//! - alles andere → der Parser MUSS annehmen. Auch `phase: resolution` und
//!   `phase: runtime`: die sind syntaktisch einwandfrei und scheitern spaeter.
//!
//! Das ist ein vollstaendiges, hartes Urteil ueber die Grammatik, ganz ohne
//! Interpreter — und deshalb die Leiter, die vor der Maschine steht.
//!
//! Der Korpus liegt NICHT im Repo (273 MB). Pfad ueber `TEST262=`; ohne die
//! Variable ueberspringt der Test sich selbst und sagt, wie man ihn anschaltet.
//!
//!   TEST262=~/…/tools/test262-upstream cargo test --release \
//!     --manifest-path tools/wasm/beak-engine/Cargo.toml --test test262 -- --nocapture
//!
//! `T262_FILTER=<substr>` grenzt ein · `T262_SHOW=<n>` zeigt n Fehler.
//!
//! Verglichen wird gegen `tools/test262/out/baseline-v8.json`: ein Test, den
//! wir reissen und V8 besteht, ist UNSERE Luecke. Die eigene Prozentzahl allein
//! sagt wenig — test262 laeuft den Motoren voraus.
```

## L28-29 · `const SKIP_DIRS: &[&str] = &["intl402", "staging"];`

```
/// Verzeichnisse ausserhalb des Ziels — dieselbe Politik wie
/// `tools/test262/subset.json`, hier auf das reduziert, was fuers PARSEN zaehlt.
```

## L32-35 · `const SKIP_FEATURES: &[&str] = &[`

```
/// Nur SYNTAX-Vorschlaege, die wir bewusst nicht bauen. Der Unterschied zum
/// Ausfuehrungslauf ist gross und lehrreich: `Temporal` parst tadellos (es
/// fehlen nur Builtins), also faellt es hier NICHT weg. Ausgeschlossen ist
/// allein, was die Grammatik selbst aendert.
```

## L38 · `"explicit-resource-management",   // using x = …`

```
// `using x = …`
```

## L45-53 · `const SKIP_FEATURES_EXEC: &[&str] = &[`

```
/// Der ZWEITE Nenner fuer den Ausfuehrungslauf — dieselbe Politik wie
/// `tools/test262/subset.json`.
///
/// Er ist LAENGER als der fuers Parsen, und das ist kein Widerspruch:
/// `Temporal` parst tadellos und faellt dort zurecht nicht weg, aber
/// AUSFUEHREN kann es nur, wer es gebaut hat — und wir bauen es erklaertermassen
/// nicht. Die erste Fassung dieses Laufs zaehlte 4436 Temporal-Tests als
/// Misserfolg mit; das ist kein ehrlicher Nenner, das ist eine
/// selbstgemachte Niederlage.
```

## L63-70 · `const DONE_SRC: &str = r#"`

```
/// Der `$DONE`, den ein `async`-Test ruft — unsere Fassung von
/// `harness/doneprintHandle.js`.
///
/// **Ein WAHRER Grund ist ein Fehler**, alles andere (kein Argument,
/// `undefined`, `null`) ist Erfolg — wortgleich mit dem `if (error)` der
/// mitgelieferten Datei. Ein zweiter Aufruf ist selbst ein Fehler: die
/// Spezifikation der Fahne sagt „the sole asynchronous test of a file", und
/// ein Test, der zweimal fertig wird, hat einen Rueckruf zu viel gefeuert.
```

## L79-83 · `fn async_outcome(`

```
/// Die Schlange leeren und ablesen, was `$DONE` gemeldet hat.
///
/// Gerufen wird das NUR fuer `async`-Tests: fuer jeden anderen waere das
/// Fahren der Schlange eine zweite Semantik — ein gewoehnlicher Test ist
/// fertig, wenn sein letzter Befehl gelaufen ist.
```

## L89 · `r.as_ref().map_err(|e| e.clone())?;`

```
// Ein Wurf im Skript selbst bleibt der Wurf — `$DONE` kam dann nie dazu.
```

## L108-111 · `_ => Err(String::from("$DONE wurde nie gerufen")),`

```
// **Das ist die haeufigste ehrliche Absage**, nicht ein Laeuferfehler:
// die Kette blieb irgendwo stehen, meist an einem Merkmal, das es
// nicht gibt. Sie muss so heissen, sonst sucht der naechste Leser den
// Fehler im Geruest.
```

## L191-194 · `let (mut n_reject, mut n_accept) = (0usize, 0usize);`

```
// Getrennt gezaehlt, weil sie voellig verschieden wiegen: eine Datei, die
// wir faelschlich ABLEHNEN, kostet die ganze Seite. Eine, die wir
// faelschlich ANNEHMEN, ist ein fehlender Fruehfehler — laestig, aber die
// Seite laeuft.
```

## L197-198 · `let mut wrong_reject: Vec<(String, String)> = Vec::new();`

```
// Gezaehlt wird immer, gesammelt nur bis zum Deckel — sonst meldet der
// Bericht die Groesse des Deckels und nicht die Zahl.
```

## L237-243 · `let d = m.description.trim_start_matches(['|', '>', ' ']);`

```
// Nach Familie gebuendelt statt einzeln: 5000 Pfade sind
// keine Information, "welche Fruehfehler-Familie fehlt"
// ist eine ([[feedback_census_by_family_not_suite]]).
// Nach der REGEL gebuendelt, nicht nach dem Verzeichnis:
// test262 nennt sie in `description`, und "Klassen 1810"
// ist eine Adresse, keine Diagnose. Der Teil vor dem
// ersten Doppelpunkt/Klammer traegt die Regel.
```

## L267-269 · `let mut by_msg: std::collections::BTreeMap<&str, (usize, &str)> = Default::default();`

```
// Nach Grund gruppiert, mit EINEM Beispielpfad je Grund — eine Liste von
// 400 Pfaden sagt nichts, "welche Meldung wie oft, und wo nachsehen" sagt,
// was als naechstes zu bauen ist.
```

## L289-298 · `#[test]`

```
/// Die zweite Zahl, und fuer beak die wichtigere: parst das, was der
/// ZIELKORPUS wirklich ausliefert?
///
/// test262 misst die Sprache, dieser Test misst das Web. Die Skripte sind die,
/// die Chromium beim Laden der zwoelf Seiten geparst hat (`tools/jsscope/js/`,
/// abgelegt von `measure.mjs`) — also echter, ausgelieferter, minifizierter
/// Code und keine Testfaelle.
///
///   JSCORPUS=~/…/tools/jsscope/js cargo test --release \
///     --manifest-path tools/wasm/beak-engine/Cargo.toml --test test262 -- --nocapture
```

## L321-323 · `if beak_engine::js::parses(&src, false).is_ok()`

```
// Ein ausgeliefertes Skript kann Script ODER Modul sein, und die Datei
// sagt es nicht. Beides versuchen: nur wenn KEINES parst, ist es eine
// Luecke.
```

## L328-333 · `let err = beak_engine::js::parses(&src, true).unwrap_err();`

```
// Die Meldung aus dem MODUL-Versuch: die drei Ausreisser des
// ersten Laufs waren allesamt Module, und der Skript-Fehler
// ("unexpected keyword" bei `export`) sagte darueber nichts.
// Der Fehler aus dem MODUL-Versuch. `or_else` liefert den zweiten
// Fehler, nicht den ersten — und der Skript-Fehler bei einem Modul
// ist immer nur "unexpected keyword" beim `export`, also nutzlos.
```

## L357-366 · `#[test]`

```
/// Der AUSFUEHRUNGSLAUF. test262 sagt hier nicht mehr nur „ist das gueltige
/// Syntax", sondern „tut es das Richtige" — und das ist die Zahl, gegen die
/// jede weitere Arbeit an der Maschine gemessen wird.
///
/// Verglichen wird gegen `tools/test262/out/baseline-v8.json` (V8: 99,41 %).
/// Die eigene Zahl allein sagt wenig; die DIFFERENZ sagt alles.
///
///   TEST262=<…> cargo test --release --test test262 exec -- --nocapture
///
/// `T262_FILTER` grenzt ein · `T262_SHOW` zeigt n Fehler.
```

## L378 · `let faillist = std::env::var("T262_FAILLIST").ok();`

```
// `T262_FAILLIST=<datei>` schreibt JEDEN gescheiterten Namen dorthin.
```

## L381-382 · `let (mut vm_ran, mut vm_declined) = (0u64, 0u64);`

```
// Wieviel schon auf der BEFEHLSMASCHINE laeuft — die Zahl, die steigen
// soll, waehrend die Bestehensquote steht.
```

## L387-389 · `let mut by_fdecline: BTreeMap<&'static str, u64> = BTreeMap::new();`

```
// Und dieselbe Zaehlung fuer FUNKTIONSRUMPFE. Seit Generatoren und
// async/await eigene Maschinen bekommen, sagt ein Rumpf ab, ohne dass das
// Programm absagt — ohne diese Zeile waere die Absage unsichtbar.
```

## L391-394 · `#[cfg(feature = "strict-probe")]`

```
// Die Sonde fuer den strengen Modus (`--features strict-probe`). Gezaehlt
// wird JE VARIANTE und getrennt nach Ausgang: nur so sagt der Lauf, wie
// viele der FEHLER an einer dieser Stellen vorbeikamen — die Fahnen der
// Tests sagen es nicht.
```

## L401-402 · `#[cfg(feature = "strict-probe")]`

```
// Und: welche Stelle traf welchen gescheiterten Test — fuer die Rangliste
// nach Verzeichnis.
```

## L406-408 · `beak_engine::js::test262::enable();`

```
// **`$262` anmelden — der Laeufer ist der Wirt.** Die Engine baut es nur,
// wenn jemand es bestellt; eine Seite sieht es nie. 455 Dateien scheiterten
// ohne es mit `ReferenceError`, an einer Luecke im Geruest statt im Motor.
```

## L416-417 · `let prologue_src = format!("{}\n{}\n", hread("assert.js"), hread("sta.js"));`

```
// EINMAL geparst, dann nur noch ausgefuehrt. Der Vorspann je Variante neu
// zu parsen war der erste Entwurf und hat den Lauf allein damit verbracht.
```

## L437-439 · `let (mut t_read, mut t_parse, mut t_exec) = (0u128, 0u128, 0u128);`

```
// Die Phasen IM echten Lauf, nicht in einer Nebenmessung. Die
// Nebenmessung sagte 46 µs je Variante und lag um den Faktor 100 daneben,
// weil sie den teuren Fall nicht enthielt: sie parste nichts.
```

## L450-458 · `if m.flags.iter().any(|f| f == "module") { skip_kind += 1; continue; }`

```
// **Module brauchen einen Aufloeser, und den gibt es hier nicht** —
// eigene Zeile im Bericht, NICHT unter "bestanden".
//
// `async` stand hier bis 2026-09-11 daneben, mit der Begruendung, es
// gebe keine Promises. Die gibt es seit 0.92.0, Generatoren und
// async-Funktionen seit der Befehlsmaschine — 5485 Dateien lagen also
// als "uebergangen" im Bericht, weil niemand den Kommentar nachgelesen
// hat, als der Grund wegfiel.
// [[feedback_a_comment_that_names_its_condition_expires]]
```

## L474-484 · `if is_async && !raw { text.push_str(DONE_SRC); }`

```
// Die Hilfsdateien aus dem Zwischenspeicher: `propertyHelper.js`
// allein sind 510 Zeilen, und sie je Variante von der Platte zu
// holen ist Arbeit fuer nichts.
// **`$DONE` gehoert VOR die Hilfsdateien.** `asyncHelpers.js`
// prueft `hasOwnProperty(globalThis, "$DONE")` und wirft sonst,
// bevor der Test ueberhaupt anfaengt. Der mitgelieferte
// `doneprintHandle.js` schreibt sein Ergebnis mit `print` auf die
// AUSGABE, weil ein Kommandozeilen-Laeufer nichts anderes hat; wir
// fahren die Sitzung selbst und lesen es danach aus dem globalen
// Objekt — dieselbe Semantik (ein wahrer Grund ist ein Fehler),
// ohne den Umweg ueber Text.
```

## L490-499 · `if let Some(mark) = &trace {`

```
// Wer laenger braucht als das, wird SOFORT genannt — mit
// `flush`, damit die Zeile auch dann steht, wenn der Lauf danach
// haengt. Ein Testlaeufer, der ohne Angabe stehenbleiben kann,
// ist nicht fertig: dreimal in dieser Sitzung habe ich stattdessen
// geraten, wo die Zeit bleibt.
// Der Name VOR dem Lauf, nicht danach. Ein Test, der nie
// zurueckkehrt, taucht in einer Meldung danach nie auf — genau
// daran ist die Suche nach dem Haenger in `built-ins/Object`
// zweimal vorbeigelaufen. Nur mit `T262_TRACE`, weil eine Datei je
// Variante sonst selbst Zeit kostet.
```

## L504-505 · `t_read += t_r.elapsed().as_nanos();`

```
// Ein Absturz im Interpreter darf den LAUF nicht beenden — sonst
// misst ein einziger `unwrap` gar nichts mehr. Getrennt gezaehlt.
```

## L521-523 · `let mut s = if std::env::var("T262_NOVM").is_ok() {`

```
// `T262_NOVM=1` faehrt denselben Lauf ohne die Befehlsmaschine.
// Der Diff der beiden Fehlerlisten ist die einzige Art, die
// Umstellung ehrlich zu pruefen.
```

## L529-530 · `let r = if raw {`

```
// Nur der TEST zaehlt fuer die Deckung, nicht der Vorspann:
// der ist immer derselbe und wuerde die Zahl verwaessern.
```

## L537-539 · `#[cfg(feature = "strict-probe")]`

```
// VOR dem Testprogramm ablesen und danach abziehen:
// der Vorspann laeuft locker und trifft die Stellen
// selbst, er wuerde sonst jede Zeile gleich faerben.
```

## L546-549 · `let r = if is_async { async_outcome(&mut s, r) } else { r };`

```
// **Ein async-Test ist erst fertig, wenn die
// Schlange leer ist.** Ohne sie zu fahren liefe
// kein einziges `.then`, und JEDER dieser Tests
// meldete "„$DONE wurde nie gerufen"".
```

## L604-608 · `let why = match &out {`

```
// MIT der Meldung. Eine getroffene Stelle heisst nicht,
// dass der Test daran stirbt — `propertyHelper.js` schreibt
// selbst auf nicht schreibbare Eigenschaften und faengt den
// Fehler ab. Erst die Meldung sagt, ob der fehlende Wurf
// die URSACHE war.
```

## L624-627 · `let norm: String = why.chars()`

```
// Nach der GANZEN Meldung gebuendelt, nicht nur nach ihrer Art.
// "30221 ReferenceError" ist keine Diagnose; "Symbol is not
// defined" ist eine. Zahlen und Anfuehrungszeichen fallen weg,
// damit dieselbe Ursache nicht in tausend Varianten zerfaellt.
```

## L635-637 · `if fails.len() < 5000 || std::env::var("T262_FAILDETAIL").is_ok() {`

```
// Der Deckel galt der Bildschirmausgabe; `T262_FAILDETAIL` will
// alle. Ein Deckel, der still die Haelfte der Karte abschneidet,
// ist schlimmer als eine lange Datei.
```

## L641-649 · `all_fails.push(format!("{rel}{}", if strict { " [strict]" } else { "" }));`

```
// ALLE Namen, nicht nur die ersten 5000: nur eine vollstaendige
// Liste laesst sich gegen einen zweiten Lauf diffen, und der Diff
// ist die einzige ehrliche Pruefung einer Umstellung.
//
// MIT der Betriebsart. Gezaehlt wird die VARIANTE (eine Datei ohne
// Fahne laeuft zweimal), und ohne die Marke fallen beide auf einen
// Namen zusammen: ein Fix, der nur den strengen Modus bewegt,
// aendert die Liste dann NICHT. Genau die Blindstelle, die bei der
// Arbeit am strengen Modus jede Messung wertlos machen wuerde.
```

## L654-657 · `if let Ok(path) = std::env::var("T262_FAILDETAIL") {`

```
// `T262_FAILDETAIL=<datei>`: JEDER Fehler mit seiner Meldung. Die
// Buendelung im Bericht zeigt zwanzig Zeilen und eine Beispieldatei —
// fuer „welche Verzeichnisse stecken hinter DIESER Meldung" reicht das
// nicht, und genau das ist die Frage vor jeder Planung.
```

