# `tools/wasm/beak-engine/src/js/mod.rs` @ 5e0102684

## L1-14 · `pub mod ast;`

```
//! JavaScript: Lexer, Syntaxbaum, Parser.
//!
//! **Warum selbst geschrieben und nicht portiert.** Bei WebP war die ehrliche
//! Antwort die umgekehrte (`super::webp`): `image-webp` beruehrte `std` in vier
//! Zeilen, also wurde portiert statt neu gebaut. Hier liegt der Fall anders —
//! die guten JS-Parser (swc, oxc, boa) sind gross, `std`-gebunden und auf
//! Arenen gebaut; sie nach `no_std` zu ziehen waere mehr Arbeit als diese
//! Datei, und der Baum, den sie liefern, ist auf ihre eigenen Motoren
//! zugeschnitten. Der Rest der Engine ist aus demselben Grund handgeschrieben.
//!
//! Gemessen wird gegen **test262** (`tools/test262/`), mit der V8-Grundlinie
//! als Vergleich: ein Test, den wir reissen und V8 besteht, ist unsere Luecke.
//!
//! Was hier NICHT ist: eine Auswertung. Der Parser baut den Baum, mehr nicht.
```

## L17-18 · `pub mod code;`

```
/// Der Befehlssatz und der Uebersetzer dorthin — siehe `code.rs` fuer die
/// Begruendung des Umbaus.
```

## L52-53 · `pub fn run(src: &str, module: bool) -> Result<(), alloc::string::String> {`

```
/// Ein Programm laufen lassen. Fehler kommen als geworfener JS-Wert zurueck,
/// nicht als Rust-Fehler — ein `throw` ist ein normaler Ausgang.
```

## L58 · `pub fn run_capped(src: &str, module: bool, max_steps: u64) -> Result<(), alloc::string::String> {`

```
/// Wie `run`, aber mit einer Schrittgrenze — was ein Testlaeufer braucht.
```

## L79-84 · `pub struct Session {`

```
/// Eine Ausfuehrungseinheit, in der MEHRERE Programme nacheinander laufen.
///
/// Der Testlaeufer braucht genau das: der Vorspann (`assert.js` + `sta.js`)
/// wird EINMAL geparst und dann vor jedem Test nur noch ausgefuehrt. Ihn je
/// Variante neu zu parsen war der erste Entwurf, und bei 78 000 Varianten sind
/// das ein halbes Gigabyte Parsen fuer nichts.
```

## L96-97 · `pub fn new_without_vm(max_steps: u64) -> Session {`

```
/// Dieselbe Sitzung, aber ohne die Befehlsmaschine — fuer die Gegenprobe,
/// die sagt, WELCHE Tests die Umstellung kostet.
```

## L104 · `pub fn run(&mut self, prog: &Program) -> Result<(), alloc::string::String> {`

```
/// Ein bereits geparstes Programm laufen lassen.
```

## L126-129 · `pub fn parses(src: &str, module: bool) -> Result<(), ParseError> {`

```
/// Nur pruefen, ob es parst — ohne den Baum zu behalten.
///
/// Das ist die Form, die der Konformanzlauf braucht: bei 50 000 Dateien ist
/// die Frage „nimmt der Parser das an?" und nicht „wie sieht es aus".
```

