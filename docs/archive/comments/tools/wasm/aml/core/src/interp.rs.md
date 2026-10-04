# `tools/wasm/aml/core/src/interp.rs` @ 5e0102684

## L1 · `use crate::value::{obj, path_str, Obj, Path, Place, Seg, Value};`

```
//! AML method evaluator — enough opcodes to run the firmware's battery methods.
```

## L12-23 · `dyn_nodes: BTreeMap<Path, Node>,`

```
/// Knoten, die eine METHODE zur Laufzeit deklariert (`OpRegion`, `Field`
/// im Rumpf). Die stehen nicht in der geladenen Tabelle: unser Lader
/// stellt Methodenruempfe zurueck und sieht sie erst beim Ausfuehren.
///
/// ACPICA teilt das anders auf — der Knoten entsteht beim PARSEN, und
/// `acpi_ds_eval_region_operands` wertet Adresse und Laenge erst beim
/// AUSFUEHREN aus (dsopcode.c). Bei uns faellt beides zusammen, weil wir
/// den Rumpf ohnehin erst zur Laufzeit ansehen; das Ergebnis ist
/// dasselbe. Unterschied, bewusst: ACPICA loescht die Knoten beim
/// Verlassen der Methode (owner id), wir behalten sie bis zum Ende des
/// Laufs — `decode` baut je Runde einen frischen Namespace, also leben
/// sie nicht laenger als eine Messung.
```

## L27-32 · `trace: bool,`

```
/// Jeden gelesenen NAMEN melden.
///
/// Gebaut fuer die Frage „warum sagt `_STA` null?" — die Antwort steht
/// in den drei, vier Werten, die die Methode dafuer liest, und die
/// sieht man sonst nirgends. Ein Interpreter, der eine 0 errechnet,
/// muss sagen koennen, WORAUS.
```

## L34-38 · `mem: BTreeMap<(u8, u64), u8>,`

```
/// In-memory backing for non-EmbeddedControl regions (SystemIO,
/// SystemMemory, PCI config, ...). Keyed by (region_space, absolute_byte).
/// Only EmbeddedControl touches real hardware; the firmware's SMI/init
/// handshakes thus become harmless writes-readable-back here, so their
/// write-then-poll loops terminate without any real port I/O.
```

## L58 · `pub fn find_batteries(ns: &Namespace) -> Vec<Path> {`

```
// ── public entry points ───────────────────────────────────────────────
```

## L67 · `dev.pop(); // drop _HID -> the device path`

```
// drop _HID -> the device path
```

## L77 · `match v {`

```
// _HID may be an EisaId-encoded integer or a string "PNP0C0A".
```

## L85 · `fn eisa_id(s: &str) -> u64 {`

```
/// EisaId packing (ACPI): 7-bit compressed mfg + hex product, big-endian dword.
```

## L99 · `((swapped >> 24) & 0xFF)`

```
// Stored little-endian in the dword -> byte-swap for comparison.
```

## L106-108 · `pub fn ec_gpe(ns: &Namespace) -> Option<u32> {`

```
/// The EC's GPE number: `_GPE` in the scope of an EmbeddedControl region
/// (ACPI 6.5 §12.11, an Integer here; the Package form for a GPE block
/// device is not handled).
```

## L127-129 · `it.run_deferred();`

```
// Reihenfolge wie ACPICA in `acpi_initialize_objects`: erst die
// Operationsregionen freigeben (`_REG`), dann die Geraete anlaufen
// lassen (`_STA`/`_INI`).
```

## L137-147 · `let mut qcount = 0u64;`

```
// Wieviele `_Qxx` fuehrt diese DSDT?
//
// Das sind die Abfragebehandler des EC: der Baustein meldet ein
// Ereignis (Akku rein/raus, Kabel), das Betriebssystem holt die
// Ereignisnummer mit QR_EC ab und ruft `_Q<nr>`. **Wir tun das nicht**,
// und wenn die Firmware ihr "Akku steckt"-Flag dort setzt, bleibt es
// auf seinem Anfangswert — genau das Bild, das `_STA` hier zeigt
// (0x0F: Geraet da, Bit4 frei = kein Akku), ohne dass ein einziger
// EC-Zugriff stattfindet.
//
// Die Zahl sagt, ob dieser Weg ueberhaupt in Frage kommt.
```

## L156-167 · `let drained = it.drain_ec_queries();`

```
// Liegengebliebene EC-EREIGNISSE abholen und ihre `_Qxx` ausfuehren.
//
// Das ist `acpi_ec_clear` aus Linux `drivers/acpi/ec.c`, und es ist der
// Schritt, der hier gefehlt hat: der EC sammelt Ereignisse (Akku
// eingelegt, Netzteil dran, Deckel), setzt Bit5 seines Statusregisters
// und wartet, dass jemand sie mit `QR_EC` abholt. Erst im `_Q<nr>`
// traegt die Firmware ihren Zustand nach. Holt sie keiner ab, bleibt
// `_STA` auf seinem Anfangswert — auf Florians IdeaPad 0x0F, also
// "kein Akku", bei vollem Akku am Netz.
//
// Deckel 100 wie `ACPI_EC_CLEAR_MAX`; Linux warnt, wenn er greift, und
// wertet das als haengenden EC.
```

## L171-175 · `let mut sta = bat.clone();`

```
// `_STA` des Akkugeraets — das fragt ein Betriebssystem VOR `_BST`, und
// es beantwortet die Frage direkt: Bit0 vorhanden, Bit3 funktionsfaehig,
// **Bit4 = Akku eingelegt** (ACPI 6.5 §10.2.1). Bisher sind wir ohne
// diese Auskunft gleich auf `_BST` gegangen und hatten hinterher nur
// 0xFFFFFFFF, das beides heissen kann.
```

## L178-185 · `let mut sta_present: Option<bool> = None;`

```
// Praesenz kommt aus `_STA` Bit4 (ACPI 6.5 §10.2.1 "Battery is
// present"), nicht aus einem geratenen `remaining`.
//
// Bisher stand unten `remaining == 0xFFFFFFFF -> present = false`. Das
// war eine Kruecke aus der Zeit, als `_STA` gar nicht gefragt wurde —
// und sie ist falsch: 0xFFFFFFFF heisst nach Spezifikation
// "UNBEKANNT", nicht "nicht vorhanden". Ein voller Akku am Netz, dessen
// Restkapazitaet die Firmware nicht beziffert, verschwand damit ganz.
```

## L199-207 · `it.ec.note("[aml]  phase _BIF");`

```
// REIHENFOLGE wie Linux: erst die Beschreibung, dann der Zustand.
//
// `drivers/acpi/battery.c` ruft `acpi_battery_get_info` (_BIX/_BIF) VOR
// `acpi_battery_get_state` (_BST). Wir hatten es umgekehrt, und das ist
// nicht gleichgueltig: manche Firmware setzt in `_BIF` ihre Akkuauswahl
// oder latcht die Messwerte, und ein `_BST` davor meldet dann
// pflichtgemaess "unbekannt". Genau das Bild auf Florians IdeaPad, wo
// `_BST` sieben RICHTIGE Bytes liest (Rest 4745, Spannung 11971 mV) und
// trotzdem dreimal Ones zurueckgibt.
```

## L214 · `let mut p = bat.clone();`

```
// _BST -> Package { State, PresentRate, RemainingCapacity, Voltage }
```

## L220-222 · `for (i, el) in e.iter().enumerate().take(4) {`

```
// Das ganze Paket zeigen: 0xFFFFFFFF in JEDEM Feld heisst "kein
// Akku", 0xFFFFFFFF nur in einem heisst "unbekannt" — und aus
// `remaining` allein war das nicht zu sehen.
```

## L235-242 · `if sta_present == Some(false) {`

```
// Absent batteries report 0xFFFFFFFF in every field.
//
// Was `_BST` WIRKLICH gesagt hat, faehrt mit: mit `..Default::default()`
// kam aus diesem Zweig `state=0 remaining=0` heraus, und das sah aus wie
// eine Messung. Der Rufer konnte "Firmware meldet keinen Akku" nicht von
// "wir haben nichts gelesen" unterscheiden.
// Sagt `_STA` ausdruecklich "kein Akku", ist es keiner — sonst gilt
// ein unbekanntes `remaining` als unbekannt und nicht als abwesend.
```

## L260-267 · `let percent = if full > 0 && remaining != 0xFFFF_FFFF {`

```
// `remaining == 0xFFFFFFFF` heisst UNBEKANNT (ACPI 6.5 §10.2.2), und
// eine unbekannte Restkapazitaet darf keinen Prozentwert ergeben.
//
// Vorher lief sie durch dieselbe Rechnung wie ein Messwert: 4294967295
// mal 100 durch 53530 ist riesig, `.min(100)` macht daraus **100 %** —
// eine Zahl, die aussieht wie eine Messung, sich nie aendert und keinen
// Ursprung hat. Zum fuenften Mal an einem Abend dasselbe Muster: ein
// fehlender Wert, der als plausibler getarnt wird.
```

## L287-288 · `fn read_full_charge(&mut self, bat: &Path) -> R<(u32, u32)> {`

```
/// (LastFullChargeCap, power unit) — the unit says whether `_BST`'s
/// rate and capacities are mW/mWh (0) or mA/mAh (1).
```

## L290 · `let mut p = bat.clone();`

```
// _BIF: Package[0]=PowerUnit, [1]=DesignCap, [2]=LastFullChargeCap.
```

## L301 · `let mut p = bat.clone();`

```
// _BIX: [0]=Revision, [1]=PowerUnit, [2]=DesignCap, [3]=LastFullChargeCap.
```

## L315-316 · `fn register_ec_regions(&mut self) -> R<()> {`

```
/// Run every EmbeddedControl region's parent `_REG(3, 1)` so the firmware
/// sets its "EC ready" gate (e.g. ECRG = 1). Generic — no name hardcoded.
```

## L318-330 · `let mut pairs: Vec<(Path, u8)> = Vec::new();`

```
// `_REG(space, 1)` fuer JEDE Regionsart, die wir bedienen — nicht nur
// fuer den EC.
//
// ACPICA ruft `_REG` fuer jeden Raum, fuer den ein Handler steht
// (`acpi_ev_initialize_op_regions`), und damit sagt das
// Betriebssystem der Firmware: "dieser Raum ist jetzt benutzbar."
// Wir haben das nur fuer Raum 3 getan, obwohl `region_byte` die
// uebrigen mit Speicher hinterlegt — eine DSDT, die ihren Zustand in
// einem `_REG` fuer SystemIO oder SystemMemory einrichtet, blieb
// dadurch halb angelaufen.
//
// Paare (Elternscope, Raum), damit ein Scope mit zwei Regionen auch
// zwei Aufrufe bekommt.
```

## L342-343 · `pairs.sort_by_key(|(_, sp)| if *sp == 3 { 1 } else { 0 });`

```
// EC zuletzt: die uebrigen Raeume richten oft erst das Tor ein, durch
// das der EC danach ueberhaupt antwortet.
```

## L350-351 · `let _ = self.call_path(&reg, args);`

```
// Eine meckernde `_REG` darf die uebrigen nicht abbrechen —
// dasselbe Verhalten wie bei `_INI`.
```

## L358 · `fn has(&self, p: &Path) -> bool {`

```
/// Gibt es diesen Pfad — in der Tabelle ODER zur Laufzeit deklariert?
```

## L363 · `fn node(&self, p: &Path) -> Option<&Node> {`

```
/// Knoten nachschlagen; Laufzeitdeklarationen verdecken die Tabelle.
```

## L368-369 · `fn resolve(&self, scope: &Path, rooted: bool, carets: usize, segs: &[Seg]) -> Option<Path> {`

```
/// Wie `Namespace::resolve`, aber ueber BEIDE Karten. Ein Feld, das eine
/// Methode gerade selbst angelegt hat, muss sie auch finden koennen.
```

## L391-392 · `fn def_path(&self, scope: &Path, n: &NRef) -> Path {`

```
/// Absoluter Pfad einer DEKLARATION (kein Aufwaertssuchen) — wie
/// `Loader::def_path`.
```

## L405-429 · `fn run_ini_methods(&mut self) -> u32 {`

```
/// `_INI` ueber den ganzen Namespace — der Anlauf, den das
/// BETRIEBSSYSTEM macht.
///
/// Nachgebaut aus ACPICA `acpi_ns_initialize_devices` /
/// `acpi_ns_init_one_device` (nsinit.c): von oben nach unten, die Wurzel
/// zuerst, und `_STA` entscheidet.
///
///   * kein `_STA`  -> vorhanden UND funktionsfaehig
///   * Bit0 gesetzt -> vorhanden, `_INI` laeuft
///   * weder Bit0 noch Bit3 -> Geraet ist weder vorhanden noch
///     funktionsfaehig: der ganze TEILBAUM wird uebersprungen
///     ("don't look at the children of such a device")
///   * abwesend, aber funktionsfaehig -> `_INI` nicht, Kinder schon
///
/// Wir haben das nie getan, und die Referenz-DSDT allein hat 44 solche
/// Methoden. Darin richtet die Firmware ihren Zustand ein — unter
/// anderem den, an dem der EC seinen Akku meldet.
///
/// Fehler werden verschluckt, und das ist hier ACPICAs Verhalten: eine
/// `_INI`, die meckert, darf den Anlauf der uebrigen Geraete nicht
/// abbrechen.
///
/// Abweichung, bewusst: ACPICA laeuft das EINMAL beim Hochfahren, wir
/// je Messrunde — `decode` baut den Namespace jede Runde neu, und
/// `_REG` laeuft aus demselben Grund ebenfalls jedes Mal.
```

## L434 · `let root_ini: Path = vec![ini];`

```
// Die Wurzel zuerst (ACPICA: \_INI, dann \_SB._INI, dann der Rest).
```

## L440-451 · `let ini_seg = ini;`

```
// Ueber die `_INI`-METHODEN gehen, nicht ueber Knoten, die wie ein
// Geraet AUSSEHEN.
//
// Vorher filterte das auf `Node::Scope` — und der Lader vergibt den
// fuer Device, Scope, Processor, PowerRes und ThermalZone
// gleichermassen. Was er NICHT so einsortiert, fiel heraus, und
// damit auch dessen `_INI`. Auf Florians Geraet lief genau EINE von
// 56 Abfragebehandlern begleiteten Firmware — eine verdaechtig
// kleine Zahl.
//
// Der Elter einer `_INI` IST das Geraet. Damit haengt der Gang an
// dem, was wir suchen, statt an einer Einsortierung.
```

## L477-479 · `Err(_) => 0x0F,`

```
// Meckert `_STA`, behandeln wir das Geraet wie ACPICA:
// vorhanden und funktionsfaehig, damit ein Fehler nicht
// einen ganzen Teilbaum stilllegt.
```

## L495-499 · `match self.call_path(&ip, Vec::new()) {`

```
// Ein `_INI`, das WIRFT, muss es sagen. Es richtet das
// Geraet ein — auf der HP-Tabelle setzt das `_INI` des
// Touchpads die Adresse seines HID-Deskriptors —, und
// ein verschlucktes `let _ =` laesst danach jeden
// Folgefehler wie eine Eigenheit der Firmware aussehen.
```

## L514-519 · `fn drain_ec_queries(&mut self) -> u32 {`

```
/// Anstehende EC-Abfragen abholen und ihre `_Q<nr>` ausfuehren.
///
/// Der Name ist `_Q` plus die Nummer in HEX — Linux liest ihn mit
/// `sscanf(node_name, "_Q%x", &value)`, also zwei Grossbuchstaben-
/// Ziffern. Gesucht wird er im Scope des EC-Geraets, und das ist der
/// Elter einer EmbeddedControl-Region.
```

## L521 · `let mut scopes: Vec<Path> = Vec::new();`

```
// Scopes, in denen eine EC-Region haengt.
```

## L553-554 · `self.ec.note("[aml]   (no handler for that query)");`

```
// Linux protokolliert das ebenfalls und macht weiter: ein
// Ereignis ohne Behandler ist kein Fehler, es ist abgeholt.
```

## L572-575 · `if let Some(Node::BufferField { buf, bit_offset, bit_width }) =`

```
// Zur Laufzeit deklarierte Feldeinheit: die steht nur in
// `dyn_nodes`. Methoden koennen dort nie stehen, deshalb
// bleibt der Zugriff oben absichtlich auf der Tabelle — er
// leiht `body` aus, und das muss die Methode ueberleben.
```

## L589-590 · `let mut locals = Vec::with_capacity(8);`

```
// The method's own scope is the path itself (names it creates live here);
// unqualified lookups search upward from here.
```

## L596 · `let _ = scope; // body uses path as its scope anchor`

```
// body uses `path` as its scope anchor
```

## L607 · `fn exec_list(&mut self, f: &Frame, start: usize, end: usize) -> R<Flow> {`

```
// ── statement execution ───────────────────────────────────────────
```

## L625 · `let (pkg_end, p1) = pkg_length(b, p + 1);`

```
// If
```

## L633 · `return Ok((Flow::Normal, skip_else(b, pkg_end)));`

```
// fall through past a possible Else
```

## L636 · `if pkg_end < b.len() && b[pkg_end] == 0xA1 {`

```
// skip then-block; run Else if present
```

## L646-647 · `let (else_end, _e1) = pkg_length(b, p + 1);`

```
// Stray Else (then-branch was taken and consumed it via skip_else,
// so reaching here means skip it).
```

## L652 · `let (pkg_end, p1) = pkg_length(b, p + 1);`

```
// While
```

## L672 · `0xA3 => Ok((Flow::Normal, p + 1)), // Noop`

```
// Noop
```

## L674 · `let (v, np) = self.eval(f, p + 1)?;`

```
// Return TermArg
```

## L680-682 · `0x08 => {`

```
// Name(x, wert) im Methodenrumpf — die haeufigste Deklaration
// ueberhaupt: eine Methode legt ihr Ergebnispaket an, fuellt es
// und gibt es zurueck. Genau daran starb `GBIF`.
```

## L690-692 · `0x5B if b[p + 1] == 0x01 => {`

```
// Mutex(name, flags) / Event(name) — im Rumpf zulaessig. Wir
// fuehren sie als "vorhanden"; `Acquire`/`Release` sind bei
// einem einzigen Rechenweg ohnehin folgenlos.
```

## L697 · `Ok((Flow::Normal, p1 + 1)) // + SyncFlags`

```
// + SyncFlags
```

## L705-708 · `0x5B if b[p + 1] == 0x80 => {`

```
// DEKLARATIONEN im Methodenrumpf. Legales, verbreitetes AML: eine
// Methode legt ihre Operationsregion und deren Felder selbst an,
// typisch fuer gemultiplexte EC-Register. Der Lader sieht sie nie,
// weil er Methodenruempfe zurueckstellt.
```

## L710-713 · `let (nref, p1) = name_at(b, p + 2);`

```
// OpRegionOp NameString RegionSpace RegionOffset RegionLen.
// Offset und Laenge sind TermArgs und werden HIER ausgewertet
// — genau der Schritt, den ACPICA in
// `acpi_ds_eval_region_operands` macht.
```

## L726 · `let (pkg_end, p1) = pkg_length(b, p + 2);`

```
// FieldOp PkgLength NameString FieldFlags FieldList
```

## L742 · `let (_v, np) = self.eval(f, p)?;`

```
// Expression statement (Store, method call, op with target...).
```

## L749 · `fn eval(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {`

```
// ── expression evaluation ─────────────────────────────────────────
```

## L779 · `let (pkg_end, p1) = pkg_length(b, p + 1);`

```
// Buffer
```

## L789 · `let (pkg_end, p1) = pkg_length(b, p + 1);`

```
// Package
```

## L819 · `let (src, p1) = self.eval(f, p + 1)?;`

```
// Store(src, SuperName)
```

## L828 · `let (place, p1) = self.super_name(f, p + 1)?;`

```
// RefOf(SuperName)
```

## L838 · `let (a, p1) = self.eval(f, p + 1)?;`

```
// Binary op: a, b, target
```

## L863 · `let (a, p1) = self.eval(f, p + 1)?;`

```
// Divide(a, b, remainder_target, quotient_target) -> quotient
```

## L884 · `let (a, p1) = self.eval(f, p + 1)?;`

```
// Not(operand, target)
```

## L894 · `let (place, p1) = self.super_name(f, p + 1)?;`

```
// Increment / Decrement (SuperName)
```

## L903 · `let (a, p1) = self.eval(f, p + 1)?;`

```
// LAnd / LOr
```

## L914 · `let nb = b[p + 1];`

```
// LNot, or combined LNotEqual/LLessEqual/LGreaterEqual
```

## L922 · `0x93 => x != y,      // LNotEqual`

```
// LNotEqual
```

## L923 · `0x94 => !(x > y),    // LLessEqual`

```
// LLessEqual
```

## L924 · `0x95 => !(x < y),    // LGreaterEqual`

```
// LGreaterEqual
```

## L936 · `let (a, p1) = self.eval(f, p + 1)?;`

```
// LEqual / LGreater / LLess
```

## L949 · `let (place, p1) = self.index_place(f, p + 1)?;`

```
// Index(source, index, target?) -> reference
```

## L951 · `let (tgt, p2) = self.super_name(f, p1)?;`

```
// optional target
```

## L960 · `let (v, p1) = self.eval(f, p + 1)?;`

```
// DerefOf(operand)
```

## L969 · `let (place, p1) = self.super_name(f, p + 1)?;`

```
// SizeOf(SuperName)
```

## L983 · `let (a, p1) = self.eval(f, p + 1)?;`

```
// Concatenate(a, b, target)
```

## L994-1004 · `let (a, p1) = self.eval(f, p + 1)?;`

```
// ConcatenateResTemplate(a, b, target) — ACPI 2.0.
//
// 1:1 aus ACPICA `acpi_ex_concat_template` (exconcat.c):
// in BEIDEN Vorlagen das End-Tag suchen, die Teile davor
// hintereinanderlegen und EIN neues End-Tag anhaengen,
// dessen Pruefsumme 0 ist („ignorieren"). Ein leerer Puffer
// ist erlaubt und zaehlt wie ein reines End-Tag.
//
// Ohne diesen Operator gibt das `_CRS` jedes
// I2C-HID-Geraets nichts zurueck — die Firmware setzt ihre
// Vorlage aus Bus- und GPIO-Teil zusammen.
```

## L1015 · `let (a, p1) = self.eval(f, p + 1)?;`

```
// ToInteger(operand, target)
```

## L1025-1026 · `let b = f.body;`

```
// Notify(object, value): nothing acts on it here, but the
// host hears it — that is where a firmware hotkey surfaces.
```

## L1043 · `0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => self.eval_name(f, p),`

```
// name-ish first byte -> NameString (method call or name/field read)
```

## L1057 · `let (_pl, p1) = self.super_name(f, p + 2)?;`

```
// Acquire(mutex, timeout-u16) -> bool (0 = acquired)
```

## L1062 · `let (_pl, p1) = self.super_name(f, p + 2)?;`

```
// Release(mutex)
```

## L1067 · `let (place, p1) = self.super_name_opt(f, p + 2)?;`

```
// CondRefOf(SuperName, target) -> bool
```

## L1077 · `let (a, p1) = self.eval(f, p + 2)?;`

```
// FromBCD(value, target)
```

## L1087 · `let (a, p1) = self.eval(f, p + 2)?;`

```
// ToBCD(value, target)
```

## L1097-1103 · `let (a, p1) = self.eval(f, p + 2)?;`

```
// Stall(usec) = 0x21, Sleep(msec) = 0x22.
//
// Beides war ein No-op, und das ist keine Kleinigkeit: die
// Firmware wartet damit auf ihre EIGENE Hardware, typisch
// zwischen einem Schreib- und einem Lesezugriff auf den EC.
// Wer nicht wartet, liest zu frueh und bekommt den alten
// Wert — ohne dass irgendwo ein Fehler entsteht.
```

## L1106-1107 · `((a.as_int() + 999) / 1000) as u32`

```
// Stall rechnet in Mikrosekunden; aufrunden, damit ein
// Stall(1) nicht zu null wird.
```

## L1112 · `if ms > 0 { self.ec.sleep_ms(ms.min(50)); }`

```
// Deckel: eine DSDT darf uns nicht minutenlang anhalten.
```

## L1117 · `Ok((Value::Int(0), p + 2))`

```
// DebugObj as a value (rare) — treat as 0.
```

## L1120-1123 · `0x86 => Err(String::from(`

```
// Die Luecke, die nach diesem Abend noch steht, und zwar mit
// Namen statt als Zahl: Pufferfelder brauchen eine eigene
// Knotenart (Quelle + Bitversatz + Breite), IndexField ein
// Index/Daten-Paar. Beides ist ECHTE Semantik, kein Ueberspringen.
```

## L1132 · `fn eval_name(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {`

```
/// Evaluate a NameString as a value: method invocation, name read, or field read.
```

## L1136-1147 · `if nref.carets == 0 && nref.segs.len() == 1 {`

```
// ── Namen, die das BETRIEBSSYSTEM liefert ────────────────────────
//
// `_OSI`, `_OS` und `_REV` stehen NICHT in der DSDT (ACPI 6.5 §5.7).
// Die Firmware fragt damit, mit wem sie es zu tun hat, und
// verzweigt danach — jeder Interpreter (ACPICA, Linux, Windows)
// bringt sie mit. Uns fehlten sie ganz, und deshalb starb auf einem
// Lenovo IdeaPad schon `\_SB_.PCI0.LPC0.EC0_._REG` an
// "unresolved name _OSI" — also der Aufruf, der dem EC seine
// Operationsregion freigibt. Ohne den gibt es keinen Akku.
//
// Auf dem Intel-Notebook faellt es nicht auf: dessen DSDT ruft auf
// diesem Pfad kein `_OSI`. Die Luecke war immer da.
```

## L1151 · `let (arg, q) = self.eval(f, p1)?;`

```
// Genau ein Argument (die abgefragte Zeichenkette).
```

## L1157 · `return Ok((Value::Int(if yes { 0xFFFF_FFFF } else { 0 }), q));`

```
// ACPI: "Ones" ist wahr, 0 ist falsch.
```

## L1161-1163 · `return Ok((Value::Str(String::from("Microsoft Windows NT")), p1));`

```
// Was Linux meldet, und zwar mit Absicht: eine DSDT, die
// hier etwas Unbekanntes liest, nimmt ihren aeltesten
// Pfad.
```

## L1167 · `return Ok((Value::Int(2), p1));`

```
// ACPI-Revision, die wir auswerten koennen.
```

## L1177-1178 · `enum Kind { Method(u8), Name(Value), Field, BufField(Obj, u64, u64), Other }`

```
// Erst auslesen, dann handeln: `node()` leiht `self`, und die Arme
// darunter rufen `&mut self`-Methoden.
```

## L1231-1232 · `fn super_name(&mut self, f: &Frame, p: usize) -> R<(Option<Place>, usize)> {`

```
/// Parse a SuperName / Target. Returns None for the null target (0x00) and
/// for the Debug object (writes are discarded).
```

## L1241 · `0x00 => Ok((None, p + 1)), // NullName target`

```
// NullName target
```

## L1250 · `let (_t, p2) = self.super_name(f, p1)?;`

```
// optional nested target of Index is ignored when used as a target
```

## L1255 · `let (v, p1) = self.eval(f, p + 1)?;`

```
// DerefOf used as a target -> the referenced place
```

## L1262 · `0x5B if b[p + 1] == 0x31 => Ok((None, p + 2)), // DebugObj sink`

```
// DebugObj sink
```

## L1282 · `fn index_place(&mut self, f: &Frame, p: usize) -> R<(Place, usize)> {`

```
/// Parse `Index(source, index)` into a Place (without the optional target).
```

## L1300-1301 · `Place::BufIndex(obj(src), i)`

```
// Need the underlying cell for write-back. Best-effort: re-resolve
// not possible from a value copy, so wrap a throwaway cell.
```

## L1309 · `fn create_buffer_field(&mut self, f: &Frame, p: usize) -> R<usize> {`

```
// ── places ────────────────────────────────────────────────────────
```

## L1311-1321 · `fn create_buffer_field(&mut self, f: &Frame, p: usize) -> R<usize> {`

```
/// `CreateBitField` / `CreateByteField` / `CreateWordField` /
/// `CreateDWordField` / `CreateQWordField` / `CreateField`.
///
/// ACPI 6.5 §19.6.20-25: ein benanntes BUFFER-FELD, also ein
/// Bitausschnitt eines bestehenden Puffers — kein eigener Speicher.
/// Schreibt jemand hinein, aendert sich der Puffer.
///
/// Die Quelle wird als SuperName geholt, nicht mit `eval`: `eval` gaebe
/// eine KOPIE des Puffers, und `INT1 = GNUM (GPDI)` schriebe dann ins
/// Leere — die Ressourcenvorlage, aus der `_CRS` seine Pinnummer nimmt,
/// bliebe auf null.
```

## L1329-1332 · `other => {`

```
// Eine Quelle, die kein schlichter Name ist (Index(...), ein
// Methodenergebnis): dann gibt es keinen Puffer zum Anbinden.
// Ein Wegwerfpuffer haelt den Lauf am Leben und ist als solcher
// benannt.
```

## L1345 · `0x8D => (idx.as_int(), 1u64, p2),                 // CreateBitField`

```
// CreateBitField
```

## L1346 · `0x8C => (idx.as_int() * 8, 8, p2),                // CreateByteField`

```
// CreateByteField
```

## L1347 · `0x8B => (idx.as_int() * 8, 16, p2),               // CreateWordField`

```
// CreateWordField
```

## L1348 · `0x8A => (idx.as_int() * 8, 32, p2),               // CreateDWordField`

```
// CreateDWordField
```

## L1349 · `0x8F => (idx.as_int() * 8, 64, p2),               // CreateQWordField`

```
// CreateQWordField
```

## L1351 · `let (n, q) = self.eval(f, p2)?;`

```
// CreateField(source, bit-index, num-bits, name)
```

## L1363-1367 · `fn run_deferred(&mut self) -> Vec<(Path, Vec<u8>)> {`

```
/// Aufgehobene Anweisungen der Tabelle nachholen.
///
/// ACPICA fuehrt die Termliste beim Laden aus; wir holen genau die
/// Anweisungen nach, die dafuer einen Interpreter brauchen. Vor `_REG`
/// und `_INI`, weil die sie benutzen.
```

## L1373-1374 · `fn run_deferred_items(&mut self, items: Vec<(Path, Vec<u8>)>, loud: bool)`

```
/// Eine Liste aufgehobener Anweisungen ausfuehren; zurueck kommt, was
/// NICHT ging.
```

## L1385-1387 · `if let Err(e) = self.stmt(&f, 0, bytes.len()) {`

```
// Ueber DENSELBEN Verteiler wie ein Methodenrumpf: dort sind
// `Create*Field` und `OpRegion` schon richtig behandelt, und
// eine zweite Fassung waere eine zweite Semantik.
```

## L1392-1396 · `if let Some(want) = e.strip_prefix("unresolved name ") {`

```
// „Unaufloesbar" ist eine halbe Auskunft. Die andere
// Haelfte ist, ob es den Namen ueberhaupt gibt — das
// trennt „steht woanders im Baum" von „steht in keiner
// Tabelle, die wir sehen", und nur das eine davon ist
// ein Ladefehler.
```

## L1466 · `fn region_byte(&mut self, space: u8, addr: u64) -> u8 {`

```
// ── field access via the EC region ────────────────────────────────
```

## L1472-1482 · `if let Some(v) = self.mem.get(&(space, addr)).copied() {`

```
// Jede ANDERE Regionsart — SystemMemory(0), SystemIO(1),
// PCI-Config(2), SMBus(4) — liegt bei uns auf einem Notizblock und
// liefert dort, wo noch nichts geschrieben wurde, eine 0.
//
// Das ist als Bremse gedacht (die Handshakes der Firmware laufen
// damit ins Leere statt auf echte Ports), war aber STILL: eine
// gelesene 0 aus einem echten Register ist von einer erfundenen
// nicht zu unterscheiden, und die DSDT rechnet mit beiden weiter.
// Genau daran haben wir heute schon dreimal geglaubt.
// Was WIR geschrieben haben, gilt zuerst: die Handshakes der
// Firmware sollen ihren eigenen Wert zurueckbekommen.
```

## L1486-1488 · `if space == 0 {`

```
// SystemMemory(0): das echte Fenster fragen, bevor etwas erfunden
// wird. Auf einem Lenovo IdeaPad lesen `_STA` und `_BST` des Akkus
// 0xFE800008 — der EC haengt dort im Speicher statt an den Ports.
```

## L1491-1493 · `self.ec.note_num("[aml]   sysmem [", addr);`

```
// MIT Adresse: sieben Werte ohne Herkunft sagen nicht, ob
// ein zusammenhaengender Block gelesen wird oder siebenmal
// dieselbe Stelle.
```

## L1499-1507 · `use core::sync::atomic::{AtomicU32, Ordering};`

```
// EINE Zeile mit allem darin, und nur die ersten paar.
//
// Vorher waren es drei Zeilen ueber `note_num` — und ein Rufer, der
// nur `note` liefert (der i2c-hid-Treiber tut das), bekam davon
// ausgerechnet die beiden mit den ZAHLEN nicht. Uebrig blieb ein
// Dutzend nackter „erfunden", das nicht sagte, wo.
//
// Ein erfundener Wert ist EINMAL eine Auskunft und danach Laerm:
// die Firmware liest solche Register in Schleifen.
```

## L1525-1530 · `self.ec.note_num("[aml]   scratch write space=", space as u64);`

```
// Ein Schreibzugriff auf eine Nicht-EC-Region landet auf dem
// Notizblock und erreicht die Hardware NICHT. Das ist Absicht
// (Schreiben auf beliebiges MMIO koennte Geraete umprogrammieren),
// war aber still — und wenn die Firmware hier ein Auswahlregister
// bedient und danach liest, bekommt sie die Daten der falschen
// Auswahl, ohne dass irgendwo etwas auffaellt.
```

## L1574-1576 · `fn dyn_field_list(&mut self, b: &[u8], region: &Path, start: usize, end: usize) {`

```
/// FieldList einer Laufzeit-Deklaration, Bit fuer Bit — dieselbe Regel
/// wie `Loader::field_list`: Feldeinheiten sind GESCHWISTER der Region,
/// nicht ihre Kinder.
```

## L1583 · `let (pe, p1) = pkg_length(b, p + 1);`

```
// ReservedField / Offset(): PkgLength-WERT ist eine Bitluecke.
```

## L1588 · `0x01 => p += 3,                         // AccessField`

```
// AccessField
```

## L1589 · `0x02 => { let (_n, p1) = name_at(b, p + 1); p = p1; } // ConnectField`

```
// ConnectField
```

## L1590 · `0x03 => p += 4,                         // ExtendedAccessField`

```
// ExtendedAccessField
```

## L1611 · `fn field_geom(&self, path: &Path) -> R<(u8, u64, u64, u64)> {`

```
/// (region_space, region_byte_base, field_bit_offset, field_bit_width)
```

## L1627 · `fn kind(v: &Value) -> &'static str {`

```
// ── free helpers ───────────────────────────────────────────────────────
```

## L1641 · `match (a, b) {`

```
// Strings concatenate as strings; otherwise produce a buffer.
```

## L1670 · `if *n >= 0x20 && *n < 0x7f {`

```
// single-char if it's a small ASCII code (ISTR builds from NIST chars)
```

## L1742 · `pub struct NRef {`

```
// ── NameString parsing (mirrors the loader) ─────────────────────────────
```

## L1794-1804 · `fn osi_supported(s: &str) -> bool {`

```
/// Antwort auf `_OSI("…")`.
///
/// Wahr fuer die Windows-Zeichenketten, und das ist kein Zufall: fast jede
/// Firmware fragt danach, und wer mit Nein antwortet, bekommt den aeltesten
/// Pfad der DSDT — oder gar keinen. Alles andere ist falsch, insbesondere
/// "Linux": das hat Linux selbst abgeschafft, weil Firmware daraufhin
/// kaputte Sonderwege nimmt.
///
/// Bewusst OHNE obere Grenze bei der Jahreszahl. Eine Liste hier waere eine
/// Zahl aus dem Bauch, die auf dem naechsten Geraet unter dem Normalfall
/// liegt — dieselbe Bauart Fehler wie ein Deckel, der nie gerissen ist.
```

## L1836 · `fn skip_else(b: &[u8], p: usize) -> usize {`

```
/// After a taken If-then block, skip a trailing Else block if present.
```

## L1846-1850 · `fn resource_body_len(b: &[u8]) -> usize {`

```
/// Laenge einer Ressourcen-Vorlage BIS zu ihrem End-Tag.
///
/// ACPICA `acpi_ut_get_resource_end_tag`: die Deskriptoren durchgehen und
/// beim kleinen Typ 0x0F stehenbleiben. Ein leerer Puffer gilt als Vorlage
/// mit nichts als einem End-Tag, also Laenge 0.
```

## L1857 · `return i; // End-Tag: Laenge ist alles davor`

```
// End-Tag: Laenge ist alles davor
```

## L1868 · `b.len().min(i)`

```
// Kein End-Tag gefunden: alles gilt als Rumpf.
```

## L1872-1874 · `fn buf_field_read(b: &[u8], off: u64, width: u64) -> Value {`

```
/// Bits `[off, off+width)` aus einem Puffer lesen — LSB zuerst innerhalb
/// jedes Bytes (ACPI 6.5 §19.6.20 ff.). Bis 64 Bit ist das Ergebnis eine
/// Zahl, darueber ein Puffer.
```

## L1904-1905 · `fn buf_field_write(b: &mut [u8], off: u64, width: u64, val: &Value) {`

```
/// Dieselben Bits schreiben. Der Puffer WAECHST nicht — was ausserhalb
/// liegt, faellt weg, wie bei ACPICA.
```

## L1929 · `fn describe(v: &Value) -> String {`

```
/// Einen Wert kurz benennen — fuer die Spur, nicht fuer Menschen mit Zeit.
```

## L1950 · `out.push(0x79); // ACPI_RESOURCE_NAME_END_TAG | 1`

```
// ACPI_RESOURCE_NAME_END_TAG | 1
```

## L1951 · `out.push(0x00); // Pruefsumme 0 = "ignorieren"`

```
// Pruefsumme 0 = "ignorieren"
```

## L1955-1960 · `pub struct Machine<'a> {`

```
// ── Allgemeiner Zugang: Geraete finden und Methoden auswerten ─────────
//
// Bis hierher war der Interpreter auf den Akku zugeschnitten. Die Wege, die
// ein Bustreiber braucht, sind dieselben — nur ohne die Akku-Frage
// davor: Geraete nach `_HID`/`_CID` suchen, `_STA` fragen, `_CRS`/`_DSM`
// auswerten.
```

## L1962-1967 · `pub struct Machine<'a> {`

```
/// Ein initialisierter Interpreter, den ein Aufrufer mehrfach befragen kann.
///
/// `read_battery` baut sich seinen eigenen und faehrt eine feste Folge; wer
/// Geraete SUCHT, braucht stattdessen einen, der stehen bleibt — jede
/// Auswertung auf einem frischen Interpreter hiesse, `_REG`/`_INI` je Frage
/// erneut zu fahren.
```

## L1979-1986 · `pub fn taken_branch(&mut self, scope: &Path, bytes: &[u8]) -> Option<(usize, usize)> {`

```
/// Das PRAEDIKAT eines aufgehobenen `If`-Blocks auswerten und sagen,
/// welcher Zweig gilt — als Byte-Bereich innerhalb von `bytes`.
///
/// Mehr tut der Interpreter hier nicht. Was im Zweig steht, sind
/// DEKLARATIONEN (`Method`, `Name`, `Device`, `OperationRegion`), und
/// die gehoeren dem Lader. Sie hier noch einmal zu behandeln waere eine
/// zweite Fassung derselben Semantik — der erste Versuch scheiterte
/// prompt an „unhandled eval opcode 0x14", also an `Method`.
```

## L2006 · `if pkg_end < bytes.len() && bytes[pkg_end] == 0xA1 {`

```
// Sonst der Else-Zweig, wenn es einen gibt.
```

## L2014-2016 · `pub fn init(&mut self) {`

```
/// `_REG` und dann `_INI` — die Reihenfolge aus ACPICAs
/// `acpi_initialize_objects`. Ohne das antwortet eine Firmware, deren
/// Regionen noch nicht freigegeben sind, mit ihren Anfangswerten.
```

## L2026-2033 · `if !failed.is_empty() {`

```
// Was beim ersten Mal nicht ging, NOCH EINMAL.
//
// ACPICA wertet die Operanden einer Operationsregion erst beim
// ERSTEN ZUGRIFF aus (`acpi_ds_eval_region_operands`), also
// fruehestens nach `_REG` und `_INI`. Wir holen sie beim Anlauf
// nach — und sind damit zu frueh, wenn die Basis ein Name ist, den
// erst `_INI` setzt. Ein zweiter Versuch danach kostet nichts und
// deckt genau diesen Fall.
```

## L2047 · `pub fn note(&mut self, s: &str) {`

```
/// Diagnosekanal — geht denselben Weg wie die Notizen des Interpreters.
```

## L2056-2062 · `pub fn value_of(&mut self, p: &Path) -> R<Value> {`

```
/// Den Wert eines Knotens holen: ein `Name` liefert seinen Inhalt, eine
/// `Method` wird AUSGEFUEHRT.
///
/// Genau hier lag die Falle: `find_batteries` sah nur `Node::Name`, und
/// `_HID` darf eine Methode sein (die HP-Tabelle schreibt woertlich
/// `Method (_HID) { Return ("SYNA30A1") }`). Eine Art ohne Zweig faellt
/// in den, der nichts sagt.
```

## L2072 · `pub fn eval_child(&mut self, dev: &Path, name: &str) -> R<Value> {`

```
/// Ein Kind des Geraets auswerten, z. B. `_CRS`.
```

## L2082-2088 · `pub fn device_present(&mut self, dev: &Path) -> bool {`

```
/// `_STA` nach ACPI 6.5 §6.3.7: fehlt die Methode, gilt das Geraet als
/// vorhanden. Sonst Bit0 = vorhanden, Bit3 = funktionsfaehig.
///
/// **ODER, nicht UND.** Linux `acpi_device_is_present` (scan.c):
/// `adev->status.present || adev->status.functional`. Ein Und ist
/// strenger als die Vorlage und sperrt Geraete aus, die die Firmware
/// als brauchbar meldet.
```

## L2093-2098 · `pub fn call_traced(&mut self, p: &Path, args: Vec<Obj>) -> R<Value> {`

```
/// Dieselbe Methode noch einmal, aber mit SPUR: jeder gelesene Name
/// und sein Wert gehen ins Log.
///
/// Gedacht fuer den Fall, dass eine Antwort nicht stimmen kann. Eine
/// gerechnete Null sagt nichts; die drei Werte, aus denen sie entstand,
/// sagen alles.
```

## L2106-2110 · `pub fn device_status(&mut self, dev: &Path) -> Option<u64> {`

```
/// Der rohe `_STA`-Wert, oder `None`, wenn es keinen gibt.
///
/// Eine Entscheidung ohne die Zahl dahinter ist am Geraet nicht
/// nachvollziehbar — „absent" sagt nicht, ob die Firmware 0 meinte
/// oder ob wir ihr eine 0 untergeschoben haben.
```

## L2123-2124 · `pub fn device_ids(&mut self, dev: &Path) -> Vec<String> {`

```
/// Alle Kennungen eines Geraets: `_HID` zuerst, dann jede aus `_CID`
/// (das ein Package mehrerer sein darf).
```

## L2132-2134 · `out.dedup();`

```
// Doppelte werfen: ein Geraet darf denselben Namen in `_HID` UND
// `_CID` fuehren (der AMD-GPIO-Block tut das), und zweimal
// dasselbe zu melden sieht nach zwei Geraeten aus.
```

## L2144-2145 · `fn push_ids(v: &Value, out: &mut Vec<String>) {`

```
/// Eine Kennung kann eine Zeichenkette, eine EisaId-Zahl oder ein Package
/// aus beidem sein (ACPI 6.5 §6.1.2 `_CID`).
```

## L2164-2167 · `pub fn eisa_str(n: u64) -> String {`

```
/// EisaId auspacken — die Umkehrung von [`eisa_id`].
///
/// Gegenrichtung statt Kandidatenvergleich: so faellt jede Kennung als NAME
/// an und kann berichtet werden, auch eine, nach der niemand gesucht hat.
```

## L2170 · `let s = ((n >> 24) & 0xFF) | (((n >> 16) & 0xFF) << 8) | (((n >> 8) & 0xFF) << 16) | ((n & 0xFF) << 24);`

```
// Rueck-Byteswap (eisa_id speichert little-endian).
```

## L2192-2196 · `pub fn devices_with_ids(ns: &Namespace) -> Vec<Path> {`

```
/// Jedes Geraet im Namespace, das ueberhaupt eine Kennung traegt.
///
/// Ein „Geraet" ist hier der ELTER eines `_HID`- oder `_CID`-Knotens. Die
/// Kennungen selbst werden erst beim Fragen ausgewertet — ein `_HID` als
/// Methode auszufuehren kostet, und die meisten Tabellen fuehren Dutzende.
```

