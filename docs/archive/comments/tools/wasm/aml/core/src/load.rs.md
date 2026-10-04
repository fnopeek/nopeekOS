# `tools/wasm/aml/core/src/load.rs` @ 5e0102684

## L1-3 · `use crate::value::{obj, seg, Obj, Path, Seg, Value};`

```
//! AML table loader: bytes -> namespace. Registers all named objects, defers
//! method bodies, and skips control-flow blocks at scope level (the battery
//! objects are all unconditional definitions).
```

## L16-20 · `pub fn load_range(ns: &mut Namespace, bytes: &[u8], start: usize, end: usize, scope: Path)`

```
/// Einen Ausschnitt roher AML-Bytes als Termliste in einen Scope laden.
///
/// Gebraucht fuer den genommenen Zweig eines `If` auf Scope-Ebene: die
/// BEDINGUNG entscheidet der Interpreter, die DEKLARATIONEN darin gehoeren
/// hierher.
```

## L28-35 · `pub fn load_into(ns: &mut Namespace, table: &[u8]) -> Result<(), String> {`

```
/// Eine WEITERE Tabelle in denselben Namespace legen.
///
/// Eine Firmware verteilt ihre Deklarationen ueber die DSDT und beliebig
/// viele SSDTs, und sie bilden EINEN Namespace — Linux laedt sie
/// entsprechend alle (`acpi_tb_load_namespace`). Wer nur die DSDT liest,
/// dem fehlen Namen, die woanders stehen: auf einem Lenovo IdeaPad die
/// Basis der Region mit den Freigabebits der I2C-Controller (`FRTB`), und
/// ohne sie meldet `_STA` beider Controller „abgeschaltet".
```

## L46-57 · `fn predefine_root(ns: &mut Namespace) {`

```
/// Die Namen, die das BETRIEBSSYSTEM mitbringt — nicht die Tabelle.
///
/// ACPICA legt sie in `acpi_ns_root_initialize` (nsaccess.c) an, und das ist
/// kein Schoenheitsdetail: eine Firmware fragt
/// `If (CondRefOf (\_OSI, Local0))`, BEVOR sie `_OSI` benutzt. Wer die
/// Namen nur beim AUFRUF abfaengt, sagt dort Nein — und die Tabelle nimmt
/// dann ihren Pfad fuer ein Betriebssystem von vor 2001. Auf der
/// HP-Tabelle blieb `OSYS` damit auf 0x07D0, und das `_CRS` des Touchpads
/// gab statt Bus + Interrupt nur den Interrupt zurueck.
///
/// Die Scope-Namen (`_SB_`, `_TZ_`, …) legt ACPICA ebenfalls an; die
/// deklariert jede Tabelle selbst, also bleiben sie hier weg.
```

## L59-60 · `ns.nodes.insert(alloc::vec![seg("_OSI")], Node::Other);`

```
// `_OSI` ist bei ACPICA eine Methode; wir fangen den Aufruf in
// `eval_name` ab, brauchen hier also nur die PRAESENZ.
```

## L62-63 · `ns.nodes.insert(alloc::vec![seg("_GL_")], Node::Other);`

```
// `_GL_` ist der globale Mutex (ACPI 6.5 §5.7.1). Unser `Acquire` ist
// ein No-op, aber `super_name` muss den Namen finden.
```

## L65-66 · `ns.nodes.insert(`

```
// `_OS_` und `_REV` sind Namen mit Werten. `eval_name` antwortet
// ohnehin; der Knoten macht sie fuer `CondRefOf` sichtbar.
```

## L79 · `struct NameRef {`

```
/// A parsed NameString.
```

## L95 · `fn term(&mut self, scope: &Path, mut p: usize, end: usize) -> Result<usize, String> {`

```
/// Process one scope-level term starting at `p`, return the next position.
```

## L101 · `let (name, np) = self.name_ref(p);`

```
// NameOp NameString DataRefObject
```

## L110 · `let (_a, p1) = self.name_ref(p);`

```
// AliasOp NameString NameString — register alias target as Other.
```

## L118 · `let (pkg_end, p1) = self.pkg_length(p);`

```
// ScopeOp PkgLength NameString TermList
```

## L127 · `let (pkg_end, p1) = self.pkg_length(p);`

```
// MethodOp PkgLength NameString MethodFlags TermList(body deferred)
```

## L140 · `let (name, p1) = self.name_ref(p);`

```
// ExternalOp NameString ObjectType ArgCount
```

## L147 · `let op_start = p - 1;`

```
// CreateDWord/Word/Byte/Bit/QWordField(source, index, NameString)
```

## L154-155 · `self.ns.nodes.insert(t, Node::Other);`

```
// Platzhalter, damit der Name aufloesbar ist, BEVOR die
// aufgehobene Anweisung laeuft; sie ersetzt ihn dann.
```

## L161-178 · `let op_start = p - 1;`

```
// If / Else / While auf SCOPE-Ebene: aufheben und beim
// Anlauf AUSFUEHREN.
//
// Hier stand „skip the whole block (the battery objects are
// never conditionally defined)". Fuer den Akku stimmte das.
// Fuer alles andere ist es ein Loch im Namespace: ACPICA
// FUEHRT die Termliste einer Tabelle beim Laden aus
// (`acpi_ns_execute_table`), ein `If` ist dort eine
// Verzweigung und kein Text. Was darin deklariert wird,
// existiert fuer einen Ueberspringer nicht.
//
// Auf Florians IdeaPad haengt daran `FRTB` — die Basis der
// Region mit den Freigabebits der I2C-Controller —, und auf
// Intel-Tabellen die `_HID` der I2C-Controller selbst.
//
// Ein `Else` gehoert zu seinem `If`: beide zusammen
// aufheben, sonst entscheidet der Interpreter ohne
// Gegenstueck.
```

## L196-197 · `0x00 | 0x01 | 0xFF => {}`

```
// Bare data tokens shouldn't appear at scope level; if they do,
// they're harmless no-ops we can skip.
```

## L214 · `let (name, p1) = self.name_ref(p);`

```
// MutexOp NameString SyncFlags
```

## L221 · `let (name, p1) = self.name_ref(p);`

```
// EventOp NameString
```

## L228-232 · `let op_start = p - 2; // 0x5B 0x80`

```
// OpRegionOp NameString RegionSpace RegionOffset RegionLen.
// Offset/Len are TermArgs (not PkgLength-delimited). For the
// battery we only need EmbeddedControl regions, whose offset is
// a constant; other regions may have computed offsets we just
// skip past.
```

## L233 · `let op_start = p - 2; // 0x5B 0x80`

```
// 0x5B 0x80
```

## L242-252 · `let bytes = self.b[op_start..p].to_vec();`

```
// GERECHNETE Basis. Hier stand frueher schlicht 0, und
// damit lasen alle Felder der Region ab Adresse null.
// Auf Florians IdeaPad sind das `IC0E`/`IC3E` — die
// Freigabebits der I2C-Controller —, und ihre erfundene
// 0 liess das `_STA` beider Controller „abgeschaltet"
// melden, obwohl beide laufen.
//
// ACPICA wertet Adresse und Laenge erst beim AUSFUEHREN
// aus (`acpi_ds_eval_region_operands`). Unser
// Interpreter tut das im Methodenrumpf laengst; hier
// wird die Anweisung dafuer aufgehoben.
```

## L258 · `let (pkg_end, p1) = self.pkg_length(p);`

```
// FieldOp PkgLength NameString FieldFlags FieldList
```

## L267 · `let (pkg_end, p1) = self.pkg_length(p);`

```
// DeviceOp PkgLength NameString TermList
```

## L276 · `let (pkg_end, p1) = self.pkg_length(p);`

```
// ProcessorOp PkgLength NameString ProcID PblkAddr PblkLen TermList
```

## L281 · `self.term_list(target, p2 + 6, pkg_end)?;`

```
// ProcID(1) + PblkAddr(4) + PblkLen(1) = 6 bytes
```

## L286 · `let (pkg_end, p1) = self.pkg_length(p);`

```
// PowerResOp PkgLength NameString SystemLevel ResourceOrder TermList
```

## L291 · `self.term_list(target, p2 + 3, pkg_end)?;`

```
// SystemLevel(1) + ResourceOrder(2) = 3 bytes
```

## L296 · `let (pkg_end, p1) = self.pkg_length(p);`

```
// ThermalZoneOp PkgLength NameString TermList
```

## L305-306 · `let (pkg_end, _p1) = self.pkg_length(p);`

```
// IndexFieldOp PkgLength NameString NameString FieldFlags FieldList
// Not needed for the battery (flat fields); skip the block.
```

## L311 · `let (pkg_end, _p1) = self.pkg_length(p);`

```
// BankFieldOp PkgLength ... — skip.
```

## L316 · `let op_start = p - 2; // 0x5B 0x13`

```
// CreateFieldOp source bit-index num-bits NameString
```

## L317 · `let op_start = p - 2; // 0x5B 0x13`

```
// 0x5B 0x13
```

## L335-336 · `fn field_list(&mut self, _scope: &Path, region: &Path, start: usize, end: usize) -> Result<(), String> {`

```
/// Parse a FieldList, registering each NamedField at its accumulated bit
/// offset. Reserved/Offset/Access entries advance or annotate the cursor.
```

## L343-344 · `let (_pe, p1) = self.pkg_length(p + 1);`

```
// ReservedField (also how Offset() is encoded): 0x00
// PkgLength, where the PkgLength *value* is a bit gap.
```

## L350 · `p += 3;`

```
// AccessField: 0x01 AccessType AccessAttrib
```

## L354 · `let (_n, p1) = self.name_ref(p + 1);`

```
// ConnectField: 0x02 (NameString | BufferData) — rare; skip a namestring.
```

## L359 · `p += 4;`

```
// ExtendedAccessField: 0x03 AccessType AccessAttrib AccessLength
```

## L363 · `let mut sg: Seg = [0; 4];`

```
// NamedField: NameSeg PkgLength(bitwidth)
```

## L369-370 · `fp.pop();`

```
// Field units live in the region's parent scope (siblings of
// the region), addressed by their NameSeg.
```

## L385 · `fn pkg_length(&self, p: usize) -> (usize, usize) {`

```
// ── primitives ─────────────────────────────────────────────────────
```

## L387-388 · `fn pkg_length(&self, p: usize) -> (usize, usize) {`

```
/// Decode a PkgLength, returning (absolute_end_position, position_after_pkglength_bytes).
/// The returned end is `pkglength_start + value`.
```

## L404 · `fn pkg_value(&self, p: usize) -> u64 {`

```
/// The raw PkgLength *value* (used for field bit widths / offsets).
```

## L434 · `p += 1; // NullName`

```
// NullName
```

## L437 · `p += 1;`

```
// DualNamePrefix
```

## L444 · `p += 1;`

```
// MultiNamePrefix SegCount
```

## L467 · `fn def_path(&self, scope: &Path, n: &NameRef) -> Path {`

```
/// Absolute path of a *definition* (no upward search).
```

## L484-488 · `fn region_arg(&mut self, scope: &Path, p: usize) -> Result<(u64, usize, bool), String> {`

```
/// A RegionOffset/RegionLen TermArg: a constant value if it is one,
/// otherwise 0 after skipping the (computed) expression.
/// Basis oder Laenge einer Operationsregion. Der dritte Rueckgabewert
/// sagt, ob es eine ECHTE Zahl war — sonst muss die Anweisung
/// nachgeholt werden.
```

## L503-504 · `fn arity(&self, scope: &Path, n: &NameRef) -> usize {`

```
/// Method arity (arg count) for a name reference resolved from `scope`, or 0
/// if it is not a (yet-loaded) method.
```

## L516-517 · `fn skip_term_arg(&self, scope: &Path, p: usize) -> Result<usize, String> {`

```
/// Advance past one TermArg expression without evaluating it. Enough for
/// computed region offsets (arithmetic / method calls on names + constants).
```

## L537 · `0x60..=0x6E => Ok(p + 1), // Local0-7 / Arg0-6`

```
// Local0-7 / Arg0-6
```

## L538 · `0x72 | 0x74 | 0x77 | 0x79 | 0x7A | 0x7B | 0x7C | 0x7D | 0x7E | 0x7F => {`

```
// binary ops: operand operand target
```

## L545 · `let p1 = self.skip_term_arg(scope, p + 1)?;`

```
// Divide: a b rem-target quo-target
```

## L552 · `let p1 = self.skip_term_arg(scope, p + 1)?;`

```
// Not: operand target
```

## L568 · `0x83 => self.skip_term_arg(scope, p + 1), // DerefOf`

```
// DerefOf
```

## L569 · `0x87 => self.skip_super_name(scope, p + 1), // SizeOf`

```
// SizeOf
```

## L571 · `let p1 = self.skip_term_arg(scope, p + 1)?;`

```
// Index: source index target
```

## L577 · `0x30 => Ok(p + 2), // Revision`

```
// Revision
```

## L580 · `0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => {`

```
// NameString: may be a method invocation — consume its args.
```

## L596 · `0x00 => Ok(p + 1), // null target`

```
// null target
```

## L604 · `0x5B if self.b[p + 1] == 0x31 => Ok(p + 2), // Debug`

```
// Debug
```

## L613-614 · `fn data_object(&mut self, p: usize, end: usize) -> Result<(Obj, usize), String> {`

```
/// Const-evaluate a DataRefObject (Name initializer, region offset/len,
/// package element). Returns a fresh Obj cell.
```

## L618 · `0x00 => Ok((obj(Value::Int(0)), p + 1)),       // Zero`

```
// Zero
```

## L619 · `0x01 => Ok((obj(Value::Int(1)), p + 1)),       // One`

```
// One
```

## L620 · `0xFF => Ok((obj(Value::Int(u64::MAX)), p + 1)), // Ones`

```
// Ones
```

## L638 · `let mut q = p + 1;`

```
// String: bytes until NUL
```

## L648 · `let (pkg_end, p1) = self.pkg_length(p + 1);`

```
// Buffer PkgLength BufferSize ByteList
```

## L658 · `let (pkg_end, p1) = self.pkg_length(p + 1);`

```
// Package PkgLength NumElements PackageElementList
```

## L674 · `let (pkg_end, p1) = self.pkg_length(p + 1);`

```
// VarPackage PkgLength NumElements(TermArg) PackageElementList
```

## L690 · `0x5B if self.b[p + 1] == 0x30 => Ok((obj(Value::Int(2)), p + 2)),`

```
// RevisionOp
```

## L692-693 · `0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => {`

```
// ObjectReference initializer (a NameString) — skip it; we don't
// need referenced-name initializers for the battery path.
```

## L702-703 · `fn package_element(&mut self, p: usize, end: usize) -> Result<(Obj, usize), String> {`

```
/// A package element is a DataRefObject or a NameString reference. We only
/// need the data ones for the battery; name references become Uninit.
```

