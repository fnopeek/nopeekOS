# `tools/wasm/aml/core/src/lib.rs` @ 5e0102684

## L1-8 · `#![cfg_attr(not(test), no_std)]`

```
//! aml_core — a minimal ACPI AML interpreter, enough to evaluate the
//! Control-Method-Battery objects (`_BST`, `_BIF`, `_BIX`) on any laptop by
//! running the firmware's own AML, exactly the way ACPICA/Linux do — no
//! per-device hardcoded EC offsets.
//!
//! no_std + alloc. The same crate compiles for the std dev-harness and for the
//! wasm32 battery driver. Hardware access (EmbeddedControl region reads/writes)
//! is abstracted behind the [`Ec`] trait.
```

## L24-26 · `pub trait Ec {`

```
/// Embedded-controller access (ACPI RegionSpace 3). The only hardware the
/// battery path needs. On real hardware this drives ports 0x62/0x66; in the
/// dev-harness it is a mock.
```

## L30-33 · `fn sleep_ms(&mut self, _ms: u32) {}`

```
/// `Sleep(ms)` / `Stall(us)` aus dem AML. Die Firmware wartet damit auf
/// ihre eigene Hardware — meist zwischen einem Schreib- und einem
/// Lesezugriff auf den EC. Wir haben beides verworfen und also zu frueh
/// gelesen. Vorgabe: nichts tun, damit der Pruefstand ohne Uhr auskommt.
```

## L35-42 · `fn query(&mut self) -> Option<u8> { None }`

```
/// Diagnosekanal fuer den Interpreter.
///
/// `aml_core` ist `no_std` und hat kein Log. Ohne das laesst sich nicht
/// sagen, aus WELCHEM Abschnitt ein EC-Zugriff kommt — `_REG`, `_INI`
/// oder `_BST` sehen im Mitschnitt gleich aus, und das war genau die
/// Frage, an der wir zuletzt haengen blieben.
/// Eine anstehende EC-ABFRAGE abholen (`QR_EC`), oder `None`.
/// Vorgabe: es gibt keine — der Pruefstand hat keinen echten EC.
```

## L44-49 · `fn mem_read(&mut self, _addr: u64) -> Option<u8> { None }`

```
/// Ein Byte aus einer SystemMemory-Operationsregion lesen.
///
/// Manche Firmware spricht mit ihrem EC nicht ueber die ISA-Ports,
/// sondern ueber ein speichergemapptes Fenster. Vorgabe: `None` — der
/// Pruefstand hat keinen physischen Speicher, und der Interpreter
/// faellt dann auf seinen Notizblock zurueck.
```

## L53-54 · `fn ec_event(&mut self, _q: u8, _handled: bool) {}`

```
/// An EC query was fetched (`handled` = a `_Qxx` for it exists and ran).
/// Always reported, unlike `note`: a hotkey is one of these.
```

## L56-58 · `fn notify(&mut self, _path: &[[u8; 4]], _value: u64) {}`

```
/// `Notify(object, value)` executed — what the firmware tells the OS.
/// Brightness hotkeys end here (0x86 up / 0x87 down on the display
/// output device, ACPI 6.5 §B.7).
```

## L62 · `pub enum Node {`

```
/// A namespace object.
```

## L64 · `Scope,`

```
/// Pure container (Scope / Device / Processor / PowerRes / ThermalZone).
```

## L66 · `Name(Obj),`

```
/// A named data object with a mutable value cell.
```

## L68 · `Method { flags: u8, body: Vec<u8>, scope: Path },`

```
/// A control method: flags + deferred body bytes + the scope it lives in.
```

## L70 · `Region { space: u8, offset: u64, len: u64 },`

```
/// OperationRegion.
```

## L72 · `Field { region: Path, bit_offset: u64, bit_width: u64 },`

```
/// A field unit inside a region.
```

## L74 · `BufferField { buf: Obj, bit_offset: u64, bit_width: u64 },`

```
/// Ein Bitausschnitt eines Puffers (`CreateWordField` & Co.).
```

## L76 · `Other,`

```
/// Mutex / Event / External declaration — presence only.
```

## L80 · `pub struct Namespace {`

```
/// The loaded ACPI namespace.
```

## L83-90 · `pub deferred: Vec<(Path, alloc::vec::Vec<u8>)>,`

```
/// Anweisungen auf SCOPE-Ebene, die einen Interpreter brauchen.
///
/// ACPICA FUEHRT die Termliste einer Tabelle beim Laden aus
/// (`acpi_ns_execute_table`), unser Lader liest sie nur. `CreateWordField
/// (SBFG, 0x17, INT1)` steht aber genau dort und braucht den Wert seines
/// Quellpuffers — also wird die Anweisung mit ihrem Scope aufgehoben und
/// beim ersten Anlauf nachgeholt. Paare (Scope, rohe AML-Bytes der
/// Anweisung).
```

## L95 · `pub fn load(table: &[u8]) -> Result<Namespace, String> {`

```
/// Parse a DSDT/SSDT table (with its 36-byte ACPI header) into a namespace.
```

## L100-104 · `pub fn find_by_segment(&self, name: &str) -> alloc::vec::Vec<Path> {`

```
/// Jeden Knoten, dessen LETZTES Segment so heisst — egal wo.
///
/// Die Frage „gibt es den Namen ueberhaupt?" ist bei einem
/// „unresolved name" die einzige, die weiterhilft: sie trennt „steht
/// woanders im Baum" von „steht in keiner Tabelle, die wir sehen".
```

## L110-123 · `pub fn resolve_conditionals(&mut self, ec: &mut dyn Ec) -> (usize, usize) {`

```
/// Bedingte Bloecke auf Scope-Ebene aufloesen.
///
/// ACPICA FUEHRT die Termliste einer Tabelle beim Laden aus
/// (`acpi_ns_execute_table`); ein `If` dort ist eine Verzweigung, kein
/// Text. Unser Lader kann keine Bedingung auswerten und hob sie
/// deshalb auf — hier werden sie nachgeholt: Praedikat durch den
/// Interpreter, genommener Zweig durch den Lader.
///
/// Mehrfach, weil ein genommener Zweig selbst wieder bedingt sein
/// kann. Der Deckel ist grosszuegig und endlich.
///
/// Rueckgabe: `(betrachtet, genommen)`. Die erste Zahl trennt „wir
/// haben keine gefunden" von „viele gefunden, eine galt" — ohne sie
/// sagt eine 1 nichts.
```

## L128 · `let mut conds: Vec<(Path, alloc::vec::Vec<u8>)> = Vec::new();`

```
// Bedingte von den uebrigen aufgehobenen Anweisungen trennen.
```

## L139 · `let mut chosen: Vec<(Path, alloc::vec::Vec<u8>, usize, usize)> = Vec::new();`

```
// Erst ENTSCHEIDEN (nur lesender Zugriff auf den Namespace) …
```

## L149 · `for (scope, bytes, a, b) in chosen {`

```
// … dann LADEN (schreibender Zugriff).
```

## L159 · `pub fn load_more(&mut self, table: &[u8]) -> Result<(), String> {`

```
/// Eine weitere Tabelle (SSDT) in denselben Namespace laden.
```

## L168-169 · `pub fn resolve(&self, scope: &Path, rooted: bool, carets: usize, segs: &[Seg]) -> Option<Path> {`

```
/// Resolve a name reference using the ACPI search rules, relative to
/// `scope`. Single unqualified NameSegs search upward to the root.
```

## L182 · `loop {`

```
// Upward search: try scope, then each parent, down to root.
```

## L207 · `#[derive(Clone, Copy, Debug, Default)]`

```
/// Result of `_BST` + `_BIF`, decoded for the bar.
```

## L211 · `pub state: u32,`

```
/// State bits from _BST[0]: bit0 = discharging, bit1 = charging.
```

## L215 · `pub percent: u8,`

```
/// 0..100, computed remaining/full.
```

## L217-218 · `pub rate: u32,`

```
/// _BST[1] present rate and _BST[3] voltage (mV), raw; 0xFFFFFFFF =
/// unknown (ACPI 6.5 §10.2.2.11).
```

## L221 · `pub power_unit: u32,`

```
/// _BIF[0] / _BIX[1] power unit: 0 = mW/mWh, 1 = mA/mAh.
```

## L225-227 · `pub fn read_battery(ns: &Namespace, ec: &mut dyn Ec, bat: &Path) -> Result<BatteryInfo, String> {`

```
/// Evaluate `\_SB...BAT0._BST` and `_BIF`/`_BIX` for the battery device whose
/// absolute path is given (e.g. resolved by scanning for `_HID == PNP0C0A`),
/// and decode the result.
```

## L232 · `pub fn ec_gpe(ns: &Namespace) -> Option<u32> {`

```
/// The EC's GPE number (`_GPE` next to its EmbeddedControl region).
```

## L237 · `pub fn find_batteries(ns: &Namespace) -> Vec<Path> {`

```
/// Find every Control-Method-Battery device (`_HID == "PNP0C0A"`).
```

## L257-258 · `#[test]`

```
/// Real HP Elite Dragonfly G1 DSDT + injected EC battery values; the
/// firmware AML must compute 5100/6496 = 79% with no hardcoded offsets.
```

## L267 · `m.insert(0x84u8, 0x11u8); // ADP + BATP[0]`

```
// ADP + BATP[0]
```

## L269 · `m.insert(0x8A, 0x1C); // BDC 7300`

```
// BDC 7300
```

## L271 · `m.insert(0x8E, 0x19); // BFC 6496`

```
// BFC 6496
```

## L272 · `m.insert(0x99, 0x02); // BST charging`

```
// BST charging
```

## L274 · `m.insert(0xA2, 0x13); // BRC 5100`

```
// BRC 5100
```

## L284 · `let info1 = read_battery(&ns, &mut ec, &bats[1]).expect("read1");`

```
// Second battery slot is empty on this machine.
```

