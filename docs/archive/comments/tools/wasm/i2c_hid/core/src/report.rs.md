# `tools/wasm/i2c_hid/core/src/report.rs` @ 5e0102684

## L1-10 · `use alloc::{format, string::String, vec::Vec};`

```
//! Der HID-Report-Deskriptor — was ein Byte im Bericht bedeutet.
//!
//! Portiert aus Linux 6.18.26 `drivers/hid/hid-core.c`
//! (`hid_parser_main`/`_global`/`_local`, `hid_add_field`), auf das
//! eingedampft, was ein Zeigergeraet braucht: je Bericht eine Liste von
//! Feldern mit Bitversatz, Breite und Usage.
//!
//! **Warum ueberhaupt parsen?** Weil sonst jeder Treiber fuer genau ein
//! Modell gilt. Die Elan im IdeaPad, die Wacom daneben und das naechste
//! Geraet legen ihre Bytes verschieden — im Deskriptor steht, wie.
```

## L14 · `pub const PAGE_GENERIC_DESKTOP: u16 = 0x01;`

```
// Usage Pages, die uns angehen.
```

## L19 · `pub const USAGE_X: u16 = 0x30;`

```
// Usages (Generic Desktop)
```

## L24 · `pub const USAGE_TIP_SWITCH: u16 = 0x42;`

```
// Usages (Digitizer)
```

## L27-29 · `pub const USAGE_INPUT_MODE: u16 = 0x52;`

```
/// „Device Mode" im Feature-Bericht: 0 = Maus-Kompatibilitaet,
/// 3 = Praezisions-Touchpad. Ohne diesen Schalter liefert ein Touchpad
/// gar keine Mehrfingerdaten — es TUT so, als waere es eine Maus.
```

## L33-37 · `#[derive(Clone, Copy, Debug, PartialEq)]`

```
/// Zu welchem Berichtstyp ein Feld gehoert.
///
/// Report-IDs sind je Typ eigenstaendig: derselbe Bericht 3 kann als
/// Eingabe und als Feature ganz verschieden aussehen, und beide haben
/// ihre eigenen Bitversaetze.
```

## L41 · `#[derive(Clone, Copy, Debug)]`

```
/// Ein Feld in einem Eingabebericht.
```

## L48 · `pub bit_offset: u32,`

```
/// Bitversatz IM BERICHT, ohne das Report-ID-Byte.
```

## L53 · `pub relative: bool,`

```
/// Bit 2 des Input-Items: 0 = absolut, 1 = relativ.
```

## L55 · `pub constant: bool,`

```
/// Bit 0: 1 = Konstante (Fuellbits), fuer uns uninteressant.
```

## L59 · `#[derive(Default)]`

```
/// Das Ergebnis: alle Eingabefelder, in Deskriptor-Reihenfolge.
```

## L63-64 · `pub uses_ids: bool,`

```
/// Hat der Deskriptor ueberhaupt Report-IDs benutzt? Wenn nicht,
/// traegt der Bericht kein ID-Byte.
```

## L66-72 · `lens: Vec<(Kind, u8, u32)>,`

```
/// Die GANZE Laenge je Bericht, in Bit.
///
/// Aus den Feldern allein ist sie NICHT herleitbar: Fuellbits sind
/// keine Felder, stehen aber im Bericht. Und wer einen Feature-Bericht
/// zu kurz schickt, bekommt auf dem Bus ein ACK und trotzdem keine
/// Wirkung — genau daran ist der Umschalter in den Praezisionsmodus
/// gescheitert (Elan: `Report Size 16`, wir schickten ein Byte).
```

## L77 · `pub fn report_bytes(&self, kind: Kind, id: u8) -> usize {`

```
/// Wie lang ist dieser Bericht, in BYTES, ohne das Report-ID-Byte?
```

## L96-100 · `pub fn parse(desc: &[u8]) -> ReportMap {`

```
/// Einen Report-Deskriptor zerlegen.
///
/// Kurze Items: `bSize` in Bit 1..0, `bType` in 3..2, `bTag` in 7..4.
/// `bSize == 3` heisst VIER Bytes, nicht drei — die Stelle, an der sich
/// ein selbstgeschriebener Parser als erstes vertut.
```

## L104-105 · `let mut stack: Vec<Global> = Vec::new();`

```
// Ein Stapel fuer Push/Pop (Tag 0xA4/0xB4) — selten, aber wenn er
// fehlt, verrutscht alles danach.
```

## L109-110 · `let mut off_in: [u32; 256] = [0; 256];`

```
// Bitversatz je Report-ID.
// Je Typ eigene Versaetze — siehe [`Kind`].
```

## L119 · `let len = *desc.get(i + 1).unwrap_or(&0) as usize;`

```
// Long item: Laenge in Byte 1.
```

## L132 · `let sval: i32 = match size {`

```
// Vorzeichenbehaftet fuer logical min/max.
```

## L142 · `0 => match tag {`

```
// ── Main ───────────────────────────────────────────────
```

## L145-146 · `let kind = match tag {`

```
// Input / Output / Feature — dieselbe Buchfuehrung,
// nur ein anderer Topf.
```

## L159-170 · `let distinct = usages.len() > 1 || usage_min.is_some();`

```
// Ein Block, dessen Elemente ALLE DIESELBE Usage
// tragen, braucht keinen Eintrag je Element.
//
// Linux speichert Anzahl und Groesse einmal je Feld;
// ich lege je Element einen an, und ein
// Hersteller-Feature mit `Report Count (0x488)` macht
// daraus 1160 Eintraege — auf Florians Touchpad kamen
// so 1583 Felder aus 381 Bytes zusammen. Wo die Usages
// sich unterscheiden (Kontaktpunkte, Tastenreihen ueber
// `Usage Minimum`), bleibt es bei einem Eintrag je
// Element; sonst genuegt einer, und der Versatz
// springt ueber den ganzen Block.
```

## L198-206 · `if constant {`

```
// Fuellbits erzeugen KEIN Feld.
//
// Sie werden nie gelesen (`find` filtert sie
// ohnehin), aber sie kosten: ein Herstellerblock
// mit `Report Count (0x488)` legte 1160 Eintraege
// an. Auf Florians Touchpad kamen so 1634 Felder
// aus 381 Bytes zusammen. Der Versatz muss
// trotzdem weiterlaufen — sonst verrutscht alles
// danach.
```

## L229 · `usages.clear();`

```
// Collection / End Collection
```

## L235 · `1 => match tag {`

```
// ── Global ─────────────────────────────────────────────
```

## L247 · `2 => match tag {`

```
// ── Local ──────────────────────────────────────────────
```

## L251 · `0x2 => {} // Usage Maximum: die Spanne ergibt sich aus min + n`

```
// Usage Maximum: die Spanne ergibt sich aus min + n
```

## L270 · `pub fn find(&self, report_id: u8, page: u16, usage: u16) -> Option<&Field> {`

```
/// Das erste EINGABEfeld mit dieser Usage in diesem Bericht.
```

## L275-276 · `pub fn find_all(&self, kind: Kind, report_id: u8, page: u16, usage: u16) -> Vec<&Field> {`

```
/// ALLE Felder dieser Usage — ein Mehrfinger-Touchpad fuehrt X und Y
/// je Kontaktpunkt, also mehrfach im selben Bericht.
```

## L284-285 · `pub fn find_feature(&self, page: u16, usage: u16) -> Option<&Field> {`

```
/// Ein FEATURE-Feld mit dieser Usage, irgendwo — samt seiner
/// Berichtsnummer.
```

## L292 · `pub fn report_ids(&self) -> Vec<u8> {`

```
/// Alle Report-IDs, die Eingabefelder tragen.
```

## L301 · `pub fn pointer_report(&self) -> Option<u8> {`

```
/// Ein Bericht, der sich als ZEIGER auswerten laesst: X und Y darin.
```

## L309-310 · `pub fn touchpad_report(&self) -> Option<u8> {`

```
/// Ein Bericht, der KONTAKTPUNKTE traegt — also ein echter
/// Touchpad-Bericht und keine Maus-Nachahmung.
```

## L318-323 · `pub fn contact_slots(&self, report_id: u8) -> usize {`

```
/// Wieviele Kontaktpunkte dieser Bericht fuehrt.
///
/// **Das ist nicht die Zahl der Finger, die das Geraet erkennt.** Ein
/// Praezisions-Touchpad, dessen Bericht nur einen Platz hat, schickt
/// MEHRERE Berichte je Bild — `Contact Count` im ersten sagt, wieviele
/// insgesamt kommen. Florians Elan macht genau das.
```

## L342-344 · `pub fn extract(data: &[u8], f: &Field) -> i32 {`

```
/// Ein Feld aus einem Bericht herausziehen — vorzeichenrichtig.
///
/// `data` ist der Bericht OHNE Report-ID-Byte.
```

## L356 · `if f.logical_min < 0 && f.bit_size < 32 {`

```
// Ein Feld mit negativem Minimum ist vorzeichenbehaftet.
```

## L366-367 · `pub fn insert(data: &mut [u8], f: &Field, value: i32) {`

```
/// Einen Wert IN einen Bericht schreiben — das Gegenstueck zu
/// [`extract`]. Bits ausserhalb des Puffers fallen weg.
```

## L384-385 · `const BOOT_MOUSE: &[u8] = &[`

```
/// Der Boot-Maus-Deskriptor aus der HID-Spezifikation (Appendix E.10).
/// Drei Knoepfe, fuenf Fuellbits, X und Y als relative Bytes.
```

## L387 · `0x05, 0x01, // Usage Page (Generic Desktop)`

```
// Usage Page (Generic Desktop)
```

## L388 · `0x09, 0x02, // Usage (Mouse)`

```
// Usage (Mouse)
```

## L389 · `0xA1, 0x01, // Collection (Application)`

```
// Collection (Application)
```

## L390 · `0x09, 0x01, //   Usage (Pointer)`

```
//   Usage (Pointer)
```

## L391 · `0xA1, 0x00, //   Collection (Physical)`

```
//   Collection (Physical)
```

## L392 · `0x05, 0x09, //     Usage Page (Button)`

```
//     Usage Page (Button)
```

## L393 · `0x19, 0x01, //     Usage Minimum (1)`

```
//     Usage Minimum (1)
```

## L394 · `0x29, 0x03, //     Usage Maximum (3)`

```
//     Usage Maximum (3)
```

## L395 · `0x15, 0x00, //     Logical Minimum (0)`

```
//     Logical Minimum (0)
```

## L396 · `0x25, 0x01, //     Logical Maximum (1)`

```
//     Logical Maximum (1)
```

## L397 · `0x95, 0x03, //     Report Count (3)`

```
//     Report Count (3)
```

## L398 · `0x75, 0x01, //     Report Size (1)`

```
//     Report Size (1)
```

## L399 · `0x81, 0x02, //     Input (Data,Var,Abs)`

```
//     Input (Data,Var,Abs)
```

## L400 · `0x95, 0x01, //     Report Count (1)`

```
//     Report Count (1)
```

## L401 · `0x75, 0x05, //     Report Size (5)`

```
//     Report Size (5)
```

## L402 · `0x81, 0x01, //     Input (Cnst)`

```
//     Input (Cnst)
```

## L403 · `0x05, 0x01, //     Usage Page (Generic Desktop)`

```
//     Usage Page (Generic Desktop)
```

## L404 · `0x09, 0x30, //     Usage (X)`

```
//     Usage (X)
```

## L405 · `0x09, 0x31, //     Usage (Y)`

```
//     Usage (Y)
```

## L406 · `0x15, 0x81, //     Logical Minimum (-127)`

```
//     Logical Minimum (-127)
```

## L407 · `0x25, 0x7F, //     Logical Maximum (127)`

```
//     Logical Maximum (127)
```

## L408 · `0x75, 0x08, //     Report Size (8)`

```
//     Report Size (8)
```

## L409 · `0x95, 0x02, //     Report Count (2)`

```
//     Report Count (2)
```

## L410 · `0x81, 0x06, //     Input (Data,Var,Rel)`

```
//     Input (Data,Var,Rel)
```

## L411 · `0xC0,       //   End Collection`

```
//   End Collection
```

## L412 · `0xC0,       // End Collection`

```
// End Collection
```

## L424 · `assert_eq!(x.bit_offset, 8);`

```
// 3 Knopfbits + 5 Fuellbits = Byte 1, dann X, dann Y.
```

## L431 · `let b1 = m.find(0, PAGE_BUTTON, 1).expect("Knopf 1");`

```
// Knoepfe: Usage Minimum 1 laeuft ueber die drei Felder hoch.
```

## L438 · `#[test]`

```
/// Ein Bericht: linke Taste, 5 nach rechts, 3 nach oben.
```

## L451-457 · `#[test]`

```
/// Der ECHTE Deskriptor von Florians Touchpad (Elan ELAN06FA,
/// `vid=0x04f3 pid=0x31ad`, 381 Bytes, vom Geraet gelesen).
///
/// Er haelt genau das fest, was uns eine Fehlersuche gekostet hat:
/// Bericht 4 fuehrt **einen** Kontaktplatz und ein `Contact Count`.
/// Wer daraus schliesst, das Pad erkenne nur einen Finger, sucht den
/// Fehler danach an der falschen Stelle.
```

## L482-486 · `let im = m.find_feature(PAGE_DIGITIZER, USAGE_INPUT_MODE).expect("Device Mode");`

```
// 88 Bit = 11 Byte, plus Berichtsnummer und zwei Laengenbytes = 14
// — genau das `wMaxInputLength`, das das Geraet ansagt.
//
// Und der Umschalter in den Praezisionsmodus muss auffindbar sein,
// sonst sendet es ueberhaupt keinen Bericht 4.
```

## L490-491 · `assert!(m.fields.len() < 100, "{} Felder aus 381 Bytes", m.fields.len());`

```
// Und der Block, der frueher 1634 Felder erzeugt hat, erzeugt jetzt
// keine: Fuellbits sind keine Felder.
```

## L495-502 · `#[test]`

```
/// Der Umschalter in den Praezisionsmodus ist ZWEI Bytes lang.
///
/// Am Geraet quittierte das Elan einen Ein-Byte-Feature-Bericht auf
/// dem Bus und schaltete NICHT um — es kam weiter nur die
/// Maus-Nachahmung. Im Deskriptor steht `Report Size 16` an
/// `Input Mode`, und ein zu kurzer Feature-Bericht wird verworfen.
/// Die Laenge gehoert deshalb aus dem Deskriptor gerechnet und nicht
/// geraten — samt Fuellbits, die keine Felder sind.
```

## L513 · `let mut payload = alloc::vec![0u8; m.report_bytes(Kind::Feature, 3)];`

```
// Und so sieht die Nutzlast aus, die das Geraet erwartet.
```

## L518-519 · `assert_eq!(m.report_bytes(Kind::Feature, 5), 2);`

```
// Bericht 5 endet auf 14 Fuellbits: aus den FELDERN allein kaeme
// ein Byte heraus, richtig sind zwei.
```

## L523-524 · `#[test]`

```
/// Schreiben und Lesen muessen sich treffen, auch quer ueber eine
/// Bytegrenze und mit Vorzeichen.
```

## L539-540 · `#[test]`

```
/// `bSize == 3` heisst VIER Bytes. Wer drei liest, verschiebt alles
/// danach — und merkt es erst an unsinnigen Koordinaten.
```

## L543 · `let d = &[`

```
// Logical Maximum (0x00FFFFFF) als 4-Byte-Item, dann X als 16 Bit.
```

## L546 · `0x27, 0xFF, 0xFF, 0xFF, 0x00, // Logical Maximum, bSize=3 -> 4 Bytes`

```
// Logical Maximum, bSize=3 -> 4 Bytes
```

