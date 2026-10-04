# `tools/wasm/i2c_hid/core/src/dw_i2c.rs` @ 5e0102684

## L1-17 · `use alloc::{format, string::String};`

```
//! Synopsys-Designware-I2C — der Bus, an dem das Touchpad haengt.
//!
//! Portiert aus Linux 6.18.26, `drivers/i2c/busses/`:
//! `i2c-designware-core.h` (Register), `-common.c` (Takt, SCL-Zaehler,
//! Abschalten, FIFO-Tiefe) und `-master.c` (Initialisierung und
//! Uebertragung). Die Funktionsnamen sind uebernommen, damit sich beides
//! nebeneinanderlegen laesst.
//!
//! **Eine bewusste Abweichung, und ihr Grund:** Linux fuellt die FIFOs aus
//! `i2c_dw_isr`. Fuer dieses Geraet gibt es bei uns keinen Interrupt —
//! `irq.rs` sagt „no IOAPIC, PIC fully masked", und der FCH-I2C hat kein
//! MSI-X. Also laeuft dieselbe Zustandsmaschine aus einer Warteschleife:
//! `read_clear_intrbits` → `process_transfer` → `xfer_msg`/`read`. Die
//! Funktionen und ihre Reihenfolge bleiben, nur der Ausloeser ist ein
//! anderer. Linux selbst faehrt in `amd_i2c_dw_xfer_quirk` einen Pollpfad,
//! es ist also keine erfundene Bauweise — und die Maskenlogik dafuer
//! (`ACCESS_POLLING`, `sw_mask`) steht im Original.
```

## L21 · `pub const DW_IC_CON: u32 = 0x00;`

```
// ── Register (i2c-designware-core.h) ─────────────────────────────────
```

## L54 · `pub const DW_IC_SDA_HOLD_MIN_VERS: u32 = 0x3131312A; // "111*"`

```
// "111*"
```

## L56 · `pub const DW_IC_COMP_TYPE_VALUE: u32 = 0x4457_0140; // "DW" + 0x0140`

```
// "DW" + 0x0140
```

## L94 · `const ABRT_7B_ADDR_NOACK: u32 = 1 << 0;`

```
/// Abbruchgruende, die einen Namen verdienen (DW_IC_TX_ABRT_SOURCE).
```

## L102-103 · `pub trait Bus {`

```
/// Der Zugang zur Hardware. Der Treiber rechnet, der Rufer greift zu —
/// damit laeuft derselbe Code im Modul und im Pruefstand.
```

## L107 · `fn udelay(&mut self, us: u32);`

```
/// Mindestens `us` Mikrosekunden warten.
```

## L109 · `fn now_us(&mut self) -> u64;`

```
/// Monotone Zeit in Mikrosekunden, fuer Zeitablaeufe.
```

## L116 · `NotDesignware(u32),`

```
/// Kein Designware-Block an dieser Adresse.
```

## L118 · `BusBusy,`

```
/// Der Bus wurde nicht frei.
```

## L120 · `AddrNack,`

```
/// Das Geraet hat die Adresse nicht bestaetigt — meist: da ist nichts.
```

## L122 · `DataNack,`

```
/// Das Geraet hat mitten im Schreiben abgebrochen.
```

## L124 · `ArbLost,`

```
/// Arbitrierung verloren.
```

## L126 · `Abort(u32),`

```
/// Anderer Abbruch, mit dem rohen Quellregister.
```

## L128 · `Timeout,`

```
/// Zeitablauf.
```

## L132 · `fn div_round_closest(n: u64, d: u64) -> u64 {`

```
/// Auf ganze Zahlen gerundete Division — Linux `DIV_ROUND_CLOSEST_ULL`.
```

## L137-141 · `pub fn scl_hcnt(ic_clk_khz: u32, tsymbol_ns: u32, tf_ns: u32, offset: i32) -> u32 {`

```
/// `i2c_dw_scl_hcnt` (common.c).
///
/// `IC_[FS]S_SCL_HCNT + 3 >= IC_CLK * (tHD;STA + tf)`. Der Takt kommt in
/// **kHz** herein, die Zeiten in **ns** — so rechnet Linux, und `MICRO`
/// ist der Nenner, der beides zusammenbringt.
```

## L148-151 · `pub fn scl_lcnt(ic_clk_khz: u32, tlow_ns: u32, tf_ns: u32, offset: i32) -> u32 {`

```
/// `i2c_dw_scl_lcnt` (common.c).
///
/// `IC_[FS]S_SCL_LCNT + 1 >= IC_CLK * (tLOW + tf)`. Die Fallzeit zaehlt
/// mit, weil der Block ab dem Ziehen der Leitung zaehlt.
```

## L158 · `pub struct Dw {`

```
/// Der eingestellte Controller.
```

## L172-177 · `pub fn setup(`

```
/// `i2c_dw_set_timings_master` + `i2c_dw_set_fifo_size`.
///
/// `sscn`/`fmcn` sind die Werte aus der Firmware (`SSCN`/`FMCN`), wenn
/// sie welche liefert — **zuerst die Firmung fragen, erst dann
/// rechnen**, wie `i2c_dw_acpi_params` es tut. Florians IdeaPad liefert
/// keine; dann zaehlt `ic_clk_khz`.
```

## L185-187 · `let comp = bus.read32(DW_IC_COMP_TYPE);`

```
// Ist da ueberhaupt ein Designware-Block? Die Kennung ist die
// billigste Probe, dass Abbildung und Adresse stimmen — und sie
// sagt es, bevor irgendein Schreibzugriff ins Leere geht.
```

## L193 · `let sda_fall = 300u32;`

```
// Fallzeiten: Linux nimmt 300 ns, wenn die Plattform schweigt.
```

## L200 · `scl_hcnt(ic_clk_khz, 4000, sda_fall, 0), // tHD;STA = tHIGH = 4.0 us`

```
// tHD;STA = tHIGH = 4.0 us
```

## L201 · `scl_lcnt(ic_clk_khz, 4700, scl_fall, 0), // tLOW = 4.7 us`

```
// tLOW = 4.7 us
```

## L207 · `scl_hcnt(ic_clk_khz, 600, sda_fall, 0),  // tHD;STA = tHIGH = 0.6 us`

```
// tHD;STA = tHIGH = 0.6 us
```

## L208 · `scl_lcnt(ic_clk_khz, 1300, scl_fall, 0), // tLOW = 1.3 us`

```
// tLOW = 1.3 us
```

## L211 · `if ic_clk_khz == 0 && (sscn.is_none() || fmcn.is_none()) {`

```
// Ohne Takt UND ohne Firmwarewerte gibt es nichts zu rechnen.
```

## L217-218 · `let ver = bus.read32(DW_IC_COMP_VERSION);`

```
// `i2c_dw_set_sda_hold`: nur ab Version 1.11a, sonst gibt es das
// Register nicht.
```

## L227 · `let param = bus.read32(DW_IC_COMP_PARAM_1);`

```
// `i2c_dw_set_fifo_size`: Tiefe steht in COMP_PARAM_1.
```

## L232 · `let speed_bits = if bus_freq_hz <= I2C_MAX_STANDARD_MODE_FREQ {`

```
// `i2c_dw_configure_master`.
```

## L267-268 · `pub fn disable(bus: &mut dyn Bus, bus_freq_hz: u32) {`

```
/// `__i2c_dw_disable` (common.c) — samt dem Abbruch, den die Databook-
/// Anmerkung verlangt, wenn der Block noch auf der Leitung haelt.
```

## L279-280 · `let us = div_round_closest(10 * 1_000_000, bus_freq_hz.max(1) as u64) as u32;`

```
// Zehn Taktperioden warten, damit ENABLE wirklich steht —
// bei 400 kHz sind das 25 us.
```

## L294 · `if bus.read32(DW_IC_ENABLE_STATUS) & 1 == 0 { return; }`

```
// Das Statusregister darf fehlen; dann liest es 0 und wir sind fertig.
```

## L301 · `pub fn init_master(bus: &mut dyn Bus, dw: &Dw) {`

```
/// `i2c_dw_init_master` (master.c) — Schreibreihenfolge 1:1.
```

## L305-306 · `bus.write32(DW_IC_SMBUS_INTR_MASK, 0);`

```
// SMBus-Interrupts stummschalten: eine Firmware, die IC_SMBUS=1
// stehen laesst, erzeugt sonst einen Sturm, den niemand bedient.
```

## L318 · `bus.write32(DW_IC_TX_TL, dw.tx_fifo_depth / 2);`

```
// `i2c_dw_configure_fifo_master`
```

## L324 · `fn wait_bus_not_busy(bus: &mut dyn Bus) -> Result<(), Error> {`

```
/// `i2c_dw_wait_bus_not_busy` — 20 ms, wie Linux.
```

## L334 · `pub enum Msg<'a> {`

```
/// Eine Nachricht auf dem Bus.
```

## L347-348 · `struct Xfer {`

```
/// Der Stand einer laufenden Uebertragung — die Felder aus `dw_i2c_dev`,
/// die Linux' Zustandsmaschine fuehrt.
```

## L361-366 · `fn read_clear_intrbits(bus: &mut dyn Bus, x: &mut Xfer) -> u32 {`

```
/// `i2c_dw_read_clear_intrbits`, Pollfassung.
///
/// Im Pollbetrieb steht die HARDWARE-Maske auf 0 (sonst meldete der Block
/// Interrupts, die niemand abholt), und der Treiber fuehrt seine eigene.
/// Also den ROHEN Status lesen und selbst maskieren — genau das tut Linux
/// unter `ACCESS_POLLING`.
```

## L370-371 · `if stat & DW_IC_INTR_RX_UNDER != 0 { bus.read32(DW_IC_CLR_RX_UNDER); }`

```
// NICHT ueber IC_CLR_INTR loeschen — zwischen Lesen und Loeschen
// eingetroffene Meldungen gingen dabei verloren. Je Bit sein Register.
```

## L377-378 · `x.abort_source = bus.read32(DW_IC_TX_ABRT_SOURCE);`

```
// Die Quelle wird beim Lesen von CLR_TX_ABRT geloescht — vorher
// sichern, sonst ist der Grund weg.
```

## L394 · `fn xfer_init(bus: &mut dyn Bus, dw: &Dw, addr: u16, x: &mut Xfer) {`

```
/// `i2c_dw_xfer_init` (master.c).
```

## L399-400 · `bus.write32(DW_IC_INTR_MASK, 0);`

```
// Interrupts ausdruecklich aus (Hardwarefehler in manchen Fassungen),
// dann einschalten.
```

## L404 · `let _ = bus.read32(DW_IC_ENABLE_STATUS);`

```
// Blindlesen: auf Bay Trail bleibt das Register sonst haengen.
```

## L408 · `bus.write32(DW_IC_INTR_MASK, 0);`

```
// Im Pollbetrieb bleibt die Hardwaremaske auf 0; gefuehrt wird `sw_mask`.
```

## L414 · `fn xfer_msg(bus: &mut dyn Bus, dw: &Dw, msgs: &mut [Msg], x: &mut Xfer) {`

```
/// `i2c_dw_xfer_msg` (master.c) — die FIFOs fuellen.
```

## L425-426 · `if dw.master_cfg & DW_IC_CON_RESTART_EN != 0 && x.msg_write_idx > 0 {`

```
// Sind EMPTYFIFO_HOLD_MASTER_EN und RESTART_EN gesetzt, muss
// das Restart-Bit zwischen Nachrichten von Hand kommen.
```

## L439-441 · `if x.msg_write_idx == msgs.len() - 1 && total - x.tx_pos == 1 {`

```
// Das Stop-Bit laesst sich aus den Registern nicht ablesen,
// also wird es beim letzten Byte der letzten Nachricht immer
// gesetzt.
```

## L450 · `if x.rx_outstanding >= dw.rx_fifo_depth { break; }`

```
// Ueberlauf des Empfangspuffers vermeiden.
```

## L474 · `if x.msg_write_idx == msgs.len() {`

```
// Sind alle Nachmittel eingestellt, braucht es TX_EMPTY nicht mehr.
```

## L484 · `fn read_fifo(bus: &mut dyn Bus, msgs: &mut [Msg], x: &mut Xfer) {`

```
/// `i2c_dw_read` (master.c) — was im FIFO steht, herausholen.
```

## L516-517 · `fn abort_error(src: u32) -> Error {`

```
/// Einen Abbruchgrund benennen. `7B_ADDR_NOACK` ist der Normalfall „da ist
/// nichts" und kein Unglueck.
```

## L525-529 · `pub fn xfer(bus: &mut dyn Bus, dw: &Dw, addr: u16, msgs: &mut [Msg]) -> Result<(), Error> {`

```
/// Eine Folge von Nachrichten an `addr` uebertragen.
///
/// Dieselbe Zustandsmaschine wie Linux, nur aus einer Schleife gepumpt
/// statt aus der ISB: `read_clear_intrbits` → (`read_fifo` /
/// `xfer_msg`) → fertig bei `STOP_DET` und leerem Empfangskonto.
```

## L540-558 · `let deadline = bus.now_us() + 1_000_000;`

```
// Linux gibt einer Uebertragung 1 s (adapter.timeout = HZ). Dasselbe.
//
// ABER: Linux WARTET diese Sekunde (`wait_for_completion_timeout`), wir
// POLLEN sie. Eine Sekunde `udelay(10)` ist eine Sekunde voller Kern —
// fuer eine Uebertragung, bei der gerade gar nichts passiert. Deshalb
// wird abgegeben, sobald sie STEHT.
//
// Die Schwelle ist nicht geraten: `DW_IC_RX_TL` steht auf 0, der Block
// meldet also bei JEDEM empfangenen Byte, und ein Byte dauert selbst
// bei 100 kHz nur 90 us. Zwei Millisekunden ohne eine einzige Meldung
// heisst, dass nichts unterwegs ist. Eine gesunde Uebertragung
// erreicht die Schwelle nie.
//
// **Und ein Ueberlauf ist dabei ausgeschlossen, nicht bloss
// unwahrscheinlich:** `xfer_msg` stellt nie mehr Lesebefehle ein, als
// der Empfangs-FIFO tief ist (`rx_outstanding >= rx_fifo_depth` bricht
// ab). Was waehrend des Schlafens ankommen KANN, passt also immer
// hinein — die einzige Wirkung einer verschlafenen Millisekunde ist
// Verzoegerung, kein verlorenes Byte.
```

## L566 · `if stat & DW_IC_INTR_TX_ABRT != 0 {`

```
// `i2c_dw_process_transfer`
```

## L571-572 · `bus.write32(DW_IC_INTR_MASK, 0);`

```
// Nach einem Abbruch sind beide FIFOs geleert — nichts mehr
// nachschieben.
```

## L585 · `read_fifo(bus, msgs, &mut x);`

```
// Noch im FIFO stehende Bytes holen, bevor abgeschaltet wird.
```

## L596-597 · `bus.udelay(1000);`

```
// Steht sie, ABGEBEN statt drehen. `udelay` ab 1 ms gibt an den
// Scheduler ab; darunter bliebe es eine Warteschleife.
```

## L604-605 · `disable(bus, dw.bus_freq_hz);`

```
// Linux wartet hier, bis der Block nicht mehr aktiv ist, und schaltet
// dann ab (`i2c_dw_xfer` → `__i2c_dw_disable`).
```

## L614-620 · `#[test]`

```
/// Die Zaehler fuer Florians IdeaPad: `AMDI0010` faehrt mit 150 MHz,
/// das Touchpad mit 400 kHz, und die Firmware liefert kein `FMCN`.
///
/// Gegenprobe ueber die PERIODE statt ueber die Zahl: `hcnt + lcnt`
/// Takte bei 150 MHz muessen ungefaehr einen Buszyklus ergeben. Eine
/// Formel, die sich vertut, faellt hier auf — eine abgeschriebene Zahl
/// nicht.
```

## L623 · `let clk_khz = 150_000; // 150 MHz aus acpi_apd.c`

```
// 150 MHz aus acpi_apd.c
```

## L629 · `let period_ns = (h + l) as u64 * 1_000_000_000 / 150_000_000;`

```
// Periode: (hcnt + lcnt) / 150 MHz, in ns.
```

## L631-632 · `assert!(period_ns > 2_300 && period_ns < 2_600, "period {period_ns} ns");`

```
// 400 kHz sind 2500 ns. Der Block legt noch ein paar Takte drauf,
// also darf es knapp darunter liegen — aber nicht daneben.
```

## L636 · `#[test]`

```
/// Standardmodus, 100 kHz = 10 000 ns.
```

## L648-649 · `#[test]`

```
/// Der Wacom-Digitizer auf demselben Notebook faehrt 1 MHz — derselbe
/// Takt, andere Zahlen. Das prueft, dass nichts festverdrahtet ist.
```

## L654 · `let fmp = scl_hcnt(clk_khz, 260, 300, 0) + scl_lcnt(clk_khz, 500, 300, 0);`

```
// Fast-Mode-Plus: tHIGH 260 ns, tLOW 500 ns (css: i2c-designware).
```

