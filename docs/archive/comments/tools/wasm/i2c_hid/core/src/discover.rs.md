# `tools/wasm/i2c_hid/core/src/discover.rs` @ 5e0102684

## L1-14 · `use aml_core::{crs, path_str, seg, Machine, Namespace, Obj, Path, Value};`

```
//! Entdeckung: welches Geraet spricht HID over I2C, an welchem Controller,
//! unter welcher Adresse — alles aus der DSDT, nichts hartkodiert.
//!
//! Portiert aus Linux 6.18.26:
//!
//! * `drivers/hid/i2c-hid/i2c-hid-acpi.c` — die Treffertabelle
//!   (`ACPI0C50`/`PNP0C50`), die Blockliste und `_DSM` fuer die Adresse des
//!   HID-Deskriptors.
//! * `drivers/i2c/i2c-core-acpi.c` — `i2c_acpi_get_i2c_resource` /
//!   `i2c_acpi_fill_info`: Slave-Adresse, Busfrequenz und der ACPI-PFAD des
//!   Controllers aus dem `I2cSerialBus`-Deskriptor.
//! * `drivers/acpi/acpi_apd.c` — der Eingangstakt je Controller-Kennung.
//! * `drivers/i2c/busses/i2c-designware-common.c` — `i2c_dw_acpi_params`:
//!   `SSCN`/`FMCN` liefern `hcnt`/`lcnt`/`sda_hold` fertig aus der Firmware.
```

## L19 · `const HID_MATCH: [&str; 2] = ["ACPI0C50", "PNP0C50"];`

```
/// `i2c_hid_acpi_match` — HID/CID, auf die der Treiber greift.
```

## L22 · `const HID_BLACKLIST: [&str; 2] = ["CHPN0001", "IDEA5002"];`

```
/// `i2c_hid_acpi_blacklist` — traegt `PNP0C50`, ist aber nicht HID-faehig.
```

## L25-27 · `const I2C_HID_GUID: [u8; 16] = [`

```
/// `i2c_hid_guid` — 3cdff6f7-4267-4555-ad05-b30a3d8938de, in der
/// Bytefolge, in der ACPI eine GUID in einem Puffer erwartet (die ersten
/// drei Felder little-endian).
```

## L33-38 · `fn input_clock_hz(ids: &[String]) -> u32 {`

```
/// Eingangstakt des Designware-Blocks je ACPI-Kennung.
///
/// `acpi_apd.c`: `AMDI0010`/`AMDI0019` → `wt_i2c_desc` (150 MHz),
/// `AMD0010` → `cz_i2c_desc` (133 MHz). **Nicht geraten — jede Zahl steht
/// dort.** Ist die Kennung unbekannt, gibt es 0, und dann MUSS die Firmware
/// die Zaehler ueber `SSCN`/`FMCN` liefern; sonst wird nichts gerechnet.
```

## L44 · `"AMDI0015" => return 125_000_000, // wt_i3c_desc`

```
// wt_i3c_desc
```

## L51 · `#[derive(Clone, Debug)]`

```
/// Ein Controller, wie die Firmware ihn beschreibt.
```

## L59-61 · `pub pci_adr: Option<u64>,`

```
/// `_ADR`, wenn der Controller an PCI haengt (Intel LPSS) statt an
/// fester MMIO (AMD FCH). Dann gibt es kein `Memory32Fixed`, und das
/// ist kein Fehler, sondern eine andere Anbindung.
```

## L63-67 · `pub present: bool,`

```
/// Sagt `_STA` des CONTROLLERS, dass er da und funktionsfaehig ist?
///
/// Vor dem ersten MMIO-Zugriff gefragt: ein abgeschalteter Block
/// antwortet im besten Fall mit lauter Einsen und im schlechteren gar
/// nicht. Die Firmware weiss es, also wird sie gefragt.
```

## L69 · `pub sta: Option<u64>,`

```
/// Der rohe `_STA`-Wert (`None` = keine Methode, gilt als vorhanden).
```

## L71 · `pub input_clock_hz: u32,`

```
/// 0 = unbekannt; dann sind `sscn`/`fmcn` die einzige Quelle.
```

## L73 · `pub sscn: Option<(u16, u16, u32)>,`

```
/// `(hcnt, lcnt, sda_hold)` aus `SSCN` (Standard Mode).
```

## L75 · `pub fmcn: Option<(u16, u16, u32)>,`

```
/// dasselbe aus `FMCN` (Fast Mode).
```

## L79 · `#[derive(Clone, Debug)]`

```
/// Ein HID-over-I2C-Geraet, wie die Firmware es beschreibt.
```

## L86 · `pub controller_source: String,`

```
/// Der Pfad des Controllers, WIE ER IM DESKRIPTOR STEHT.
```

## L88 · `pub controller: Option<Controller>,`

```
/// Aufgeloest, wenn der Pfad im Namespace steht.
```

## L90 · `pub descriptor_address: Option<u16>,`

```
/// Adresse des HID-Deskriptors — `_DSM` Funktion 1.
```

## L92 · `pub gpio_source: String,`

```
/// GPIO-Controller und Pin des „Daten liegen bereit"-Signals.
```

## L96-101 · `pub gpio_int_flags: u16,`

```
/// Die rohen Interrupt-Flaggen des `GpioInt`.
///
/// Sie sagen, WAS am Pin „es liegt etwas an" heisst, und ohne sie
/// waere die Polaritaet geraten. ACPICA (`acpi_rs_convert_gpio` in
/// `rsserial.c`) zerlegt dasselbe Feld: Bit 0 Triggerart, Bits 2:1
/// Polaritaet, Bit 3 geteilt, Bit 4 weckfaehig.
```

## L106-110 · `pub fn gpio_level_triggered(&self) -> bool {`

```
/// Pegel- statt flankengesteuert (ACPI Bit 0, `ACPI_LEVEL_SENSITIVE`).
///
/// Nur ein PEGEL laesst sich abfragen: er steht an, bis der Bericht
/// geholt ist. Eine Flanke ist im Augenblick des Hinsehens meist
/// schon vorbei.
```

## L115 · `pub fn gpio_active_low(&self) -> bool {`

```
/// Aktiv LOW (ACPI Bits 2:1, `ACPI_ACTIVE_LOW` = 1).
```

## L121 · `#[derive(Clone, Debug)]`

```
/// Der GPIO-Block, auf den der `GpioInt` des Geraets zeigt.
```

## L128-131 · `pub irq: Option<(u32, u8)>,`

```
/// Its own interrupt line — the first `ExtendedIrq` of its `_CRS` and
/// the descriptor's raw flags (ACPI 6.4.3.6: bit 1 edge, bit 2 active
/// low). One line for ALL pins; `pinctrl-amd` takes it as
/// `platform_get_irq(pdev, 0)`.
```

## L135-139 · `pub fn parse_acpi_path(ns: &Namespace, scope: &Path, s: &str) -> Option<Path> {`

```
/// Einen ACPI-Pfad aus einer Zeichenkette in einen Namespace-Pfad wandeln.
///
/// `resource_source` ist in der Praxis absolut (`\_SB.I2CB`), darf aber
/// relativ sein; `acpi_get_handle` loest es dann gegen das Geraet auf.
/// Fuehrende `^` steigen je eine Ebene.
```

## L166 · `fn resources(m: &mut Machine, dev: &Path) -> Vec<crs::Resource> {`

```
/// `_CRS` eines Knotens holen und zerlegen.
```

## L174-175 · `fn scl_params(m: &mut Machine, dev: &Path, name: &str) -> Option<(u16, u16, u32)> {`

```
/// `i2c_dw_acpi_params` — `SSCN`/`FMCN`/`FPCN`/`HSCN` liefern ein Package
/// aus drei Zahlen: `hcnt`, `lcnt`, `sda_hold`.
```

## L242-243 · `fn dsm_descriptor_address(m: &mut Machine, dev: &Path) -> Option<u16> {`

```
/// `i2c_hid_acpi_get_descriptor` — `_DSM(guid, 1, 1, {})` gibt die Adresse
/// des HID-Deskriptors als Zahl zurueck.
```

## L260-262 · `if n == 0 { None } else { Some(n as u16) }`

```
// Ein `_DSM`, das die Funktion nicht kennt, gibt einen leeren
// Puffer zurueck — der liest sich als 0, und 0 ist keine
// gueltige Registeradresse.
```

## L269-270 · `pub fn find(ns: &Namespace, m: &mut Machine) -> Vec<HidDevice> {`

```
/// Der ganze Gang: jedes Geraet mit einer Kennung, das auf die
/// HID-over-I2C-Tabelle passt, samt aufgeloestem Controller.
```

## L278-279 · `let name = path_str(&dev);`

```
// Ab hier IST es ein Kandidat, und jeder Ausstieg sagt, warum.
// Ein stiller Uebersprung verdeckt alles, was darueber liegt.
```

## L314 · `crs::Resource::I2cSerialBus {`

```
// `i2c_acpi_fill_info`: der ERSTE I2cSerialBus zaehlt.
```

## L368-369 · `pub fn report(d: &HidDevice) -> Vec<String> {`

```
/// Ein Fund in einer Zeile je Sache — das ist der Bericht, den das Modul am
/// Geraet ins Log schreibt.
```

## L407-408 · `Some(adr) => out.push(format!(`

```
// Intel LPSS: der Controller IST ein PCI-Geraet, seine
// Register stehen in einem BAR. Kein Fehler.
```

## L469-480 · `#[test]`

```
/// Echte HP-DSDT (Intel-Notebook, Synaptics-Touchpad auf I2C1).
///
/// Das ist der Pruefstand ohne Geraet: jede Zahl hier kommt aus der
/// Firmware, keine steht im Code. Faellt einer der Werte weg, ist ein
/// Stueck des ACPI-Wegs kaputt — und GENAU diese vier Stellen haben
/// beim Bauen je einen Fehler aufgedeckt:
///
/// * `PNP0C50` kam nur mit `_HID` ALS METHODE heraus,
/// * das `_CRS` brauchte `ConcatenateResTemplate`,
/// * die 75 Bytes darin brauchten `_OSI` als NAMEN (sonst nimmt die
///   Firmware ihren Pfad fuer ein System von vor 2012),
/// * die Deskriptor-Adresse brauchte `CreateWordField`.
```

## L501 · `let g = d.gpio_controller.as_ref().expect("GPIO controller");`

```
// Der GPIO-Block, dessen Pin das „Daten liegen bereit" traegt.
```

## L506-508 · `assert_eq!(d.gpio_int_flags, 0x0012);`

```
// Was am Pin „es liegt etwas an" heisst — aus ECHTER Firmware,
// nicht aus der Annahme, dass HID over I2C es immer so macht.
// 0x12 = Pegel (Bit 0 = 0), aktiv LOW (Bits 2:1 = 1), weckfaehig.
```

## L513-515 · `assert!(!crate::gpio::is_amd_block(&g.ids));`

```
// Und dieser Block ist ein INTEL-Block: sein Registeraufbau ist
// nicht der, den `gpio.rs` rechnet, und er darf deshalb nicht
// angefasst werden.
```

## L519-529 · `#[test]`

```
/// Dieselbe Tabelle, aber mit aufgeloesten Bedingungen auf Scope-Ebene.
///
/// `Device (I2C1)` deklariert sein `_HID` INNERHALB eines
/// `If ((SMD1 != One))`. Ein Lader, der Bedingungen ueberspringt, sieht
/// einen Controller ohne Kennung — und ohne Kennung gibt es keinen
/// Eingangstakt, also keine SCL-Zaehler, also keinen Bus.
///
/// **Was der Test NICHT prueft:** was hinter den Bedingungen steht,
/// haengt an NVS-Werten der Firmware, und die kann ein Pruefstand ohne
/// Geraet nicht hinterlegen. Deshalb hier nur das, was von ihnen
/// unabhaengig ist.
```

