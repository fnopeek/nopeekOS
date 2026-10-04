# `tools/wasm/aml/wasm/src/lib.rs` @ 5e0102684

## L1-10 · `#![no_std]`

```
//! aml.wasm — the AML battery driver.
//!
//! Resident background driver (autostart). Once at startup it fetches the
//! firmware DSDT; each tick it parses it, runs the device's own `_BST`/`_BIF`
//! AML (vendor-independent, via [`aml_core`]) against the embedded controller,
//! and pushes the decoded percentage to the kernel via `npk_battery_report`.
//! The bar reads it through the unchanged `npk_battery()`.
//!
//! No persisted device state: the DSDT is the source of truth, re-parsed each
//! tick (cheap), so the same binary works on any laptop.
```

## L18-19 · `#[unsafe(link_section = ".npk.caps")]`

```
// The driver needs raw firmware + EC access — declare the HARDWARE capability
// (bit 0x40) so the kernel grants exactly that and nothing else.
```

## L27-31 · `core::arch::wasm32::unreachable()`

```
// TRAPPEN, nicht drehen. `loop {}` verwandelte jeden Absturz in einen
// stillen Haenger: die Meldung ging in einen seriellen Port, den ein
// Notebook nicht hat, und danach drehte die Faser fuer immer. Ein Trap
// faengt der Kernel ab und druckt "forge: aml endete mit unreachable"
// ueber kprintln — also auf den Bildschirm.
```

## L35-37 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L56-70 · `fn log(s: &str) {`

```
/// In BEIDE Kanaele.
///
/// `npk_log_serial` geht nur in den UART und den Fernspiegel — auf einem
/// Notebook ohne seriellen Port ist das unsichtbar, und haengt die Maschine,
/// kommt auch der Spiegel nicht mehr heraus. `npk_print` laeuft ueber
/// `kprint!`, also auf den BILDSCHIRM und in den Bootlog-Mitschnitt (und
/// damit in `dmesg`). Deshalb beides: welcher Kanal lebt, weiss man erst
/// hinterher.
/// Ein Kanal, nicht zwei.
///
/// `npk_print` laeuft ueber `kprint!`, also auf den Bildschirm UND in den
/// Bootlog-Mitschnitt (`dmesg`). `npk_log_serial` schreibt daneben in den
/// UART und haengt an JEDEN Aufruf ein `\r\n` — in beide zu schreiben gab
/// jede Zeile doppelt und zerriss sie an jeder Zahl. Ein sichtbarer Kanal
/// genuegt.
```

## L76-77 · `fn lognum2(a: &str, x: u32, b: &str, y: u32) {`

```
/// `log` mit einer Zahl dahinter — ohne Formatierer, der Allokation braucht.
/// Zwei Zahlen in einer Zeile — fuer `[addr] -> wert`.
```

## L112 · `const HEAP_SIZE: usize = 16 * 1024 * 1024;`

```
// ── bump allocator: reset to zero each tick (re-parse is fully transient) ──
```

## L138 · `const DSDT_MAX: usize = 512 * 1024;`

```
// DSDT buffer: filled once, persists across heap resets (it's not on the heap).
```

## L142-149 · `struct HostEc { reads: u32, fails: u32, verbose: bool }`

```
/// Der EC-Zugang des Treibers.
///
/// `read` MUSS ein Byte liefern — die Schnittstelle des Interpreters laesst
/// kein "keine Antwort" zu, und eine 0 ist an dieser Stelle eine plausible
/// Luege: die DSDT rechnet damit weiter und schliesst auf "kein Akku".
/// Deshalb wird wenigstens GEZAEHLT, wie oft das passiert, und der Zaehler
/// steht danach im Log. Ohne ihn sieht ein stummer EC genauso aus wie eine
/// Firmware, die wirklich keinen Akku meldet.
```

## L156-161 · `if self.verbose {`

```
// Jedes gelesene BYTE zeigen, nicht nur zaehlen.
//
// "failed=0" heisst nur "kein Fehlercode" — nicht "sinnvoller Wert".
// Liefert der EC lauter Nullen, sieht das fuer die DSDT aus wie eine
// echte Messung, und sie schliesst auf "kein Akku". Genau diese zwei
// Faelle liessen sich bisher nicht trennen.
```

## L212-215 · `let loud = unsafe { npk_sys_info(50) } == 1;`

```
// Der ausfuehrliche Mitschrieb der ERSTEN Runde — jede Region, jeder
// EC-Zugriff, jedes Feld von `_BIF` — ist das Werkzeug, mit dem dieser
// Treiber gebaut wurde, und rund 150 Zeilen. Im Normalbetrieb bleibt
// er aus; `set log.drivers 1` holt ihn zurueck.
```

## L228-237 · `let sci_vec = {`

```
// Die Runde zaehlen. Haengt der Interpreter, sagt die letzte gedruckte
// Zahl, ob es beim ERSTEN Durchgang passiert oder erst spaeter — und
// das sind zwei ganz verschiedene Fehler.
// Die erste Runde erzaehlt, danach nur noch, wenn sich das ERGEBNIS
// aendert. Eine Wegmarke je Runde ist beim Suchen richtig und im
// Betrieb eine Flut — der Treiber laeuft fuer immer.
// Das SCI nehmen: der EC meldet ueber sein GPE, und ein Hotkey-Ereignis
// muss binnen Millisekunden abgeholt werden — nach 10 s antwortete der
// EC des IdeaPad auf QR_EC nur noch mit 0 (die Helligkeitstasten
// kamen nie an). Ohne SCI bleibt es beim 10-s-Takt.
```

## L257-261 · `let tsc_per_ms = (unsafe { npk_sys_info(10) } as u64).max(1) * 1000;`

```
// Wann der Akku das naechste Mal dran ist (TSC). Ein SCI allein ist
// KEIN Grund zu arbeiten: der EC meldet sein GPE auch nach jedem eigenen
// Lese-/Schreibvorgang, und wer darauf den Akku liest, weckt sich selbst
// — auf dem IdeaPad 60x je Sekunde, ein Kern auf 100 %. Gearbeitet wird
// nur bei SCI_EVT (ein echtes Ereignis) oder wenn der Akku faellig ist.
```

## L284 · `logln("[aml] no usable battery (packed=-1)");`

```
// Ein Akku, der sich nicht mehr meldet, ist ein Befund.
```

## L287-290 · `lognum("[aml] battery percent=", (packed & 0xFF) as u32);`

```
// Der Prozentwert nicht: er aendert sich ueber eine
// Entladung rund hundertmal, und die Bar zeigt ihn
// ohnehin. Eine Zeile, die dem Nutzer waehrend des
// Tippens in den Prompt faellt, ist keine Auskunft.
```

## L296-297 · `if let Some(i) = info {`

```
// Die Rohwerte fuer `battery` (Entnahme des ganzen Geraets) — die
// Bar braucht nur den Prozentwert, eine Strommessung braucht mehr.
```

## L310-312 · `fn decode(table: &[u8], verbose: bool) -> (i32, Option<aml_core::BatteryInfo>) {`

```
/// Parse the DSDT and evaluate the first present battery; return the bar's
/// packed encoding ((status<<8)|percent) or -1 if none, and the decoded
/// battery when there is one.
```

## L323-326 · `match read_battery(&ns, &mut ec, &bat) {`

```
// Warum -1? Bisher fielen "Methode/EC hat gemeckert" und "Akku
// meldet sich als nicht vorhanden" in dieselbe stille -1, und das
// sind zwei ganz verschiedene Fehler. `read_battery` traegt einen
// Fehlertext — der wurde weggeworfen.
```

## L342-344 · `if info.present && info.remaining_mah == 0xFFFF_FFFF {`

```
// Ein Akku, dessen Restkapazitaet die Firmware nicht beziffert,
// hat keinen Prozentwert — und eine erfundene Zahl in der Bar
// ist schlechter als gar keine Zelle.
```

## L353 · `let status = if info.state & 0x2 != 0 {`

```
// bar status: 0=discharging 1=charging 2=full 3=plugged-idle.
```

