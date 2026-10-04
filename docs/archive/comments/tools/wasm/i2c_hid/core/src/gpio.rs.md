# `tools/wasm/i2c_hid/core/src/gpio.rs` @ 5e0102684

## L1-23 · `use alloc::string::String;`

```
//! Der GPIO-Block des AMD-FCH — nur so viel, wie die eine Frage braucht:
//! **liegt gerade ein Bericht an?**
//!
//! Portiert aus Linux 6.18 `drivers/pinctrl/pinctrl-amd.{c,h}`. Dort steht
//! beides, was hier gerechnet wird: ein Register je Pin bei `base + pin * 4`
//! (`amd_gpio_get_value`) und der LEBENDE Pegel in Bit 16 (`PIN_STS_OFF`).
//! `base` ist der erste `Memory32Fixed` des ACPI-Geraets — dieselbe Quelle,
//! aus der Linux' `devm_platform_get_and_ioremap_resource(pdev, 0, …)`
//! schoepft, also keine festverdrahtete 0xFED81500.
//!
//! **Warum das ueberhaupt gebraucht wird.** HID over I2C ist
//! PEGELgesteuert: das Geraet zieht seine Leitung, sobald ein Bericht
//! bereitliegt, und laesst sie erst los, wenn der Bericht geholt ist.
//! Linux ruft `i2c_hid_get_input` deshalb ausschliesslich aus
//! `i2c_hid_irq`. Ohne Interrupt lasen wir blind — und ein blinder Versuch
//! ist nicht billig: `wMaxInputLength` sind 64 Bytes, bei 400 kHz also
//! 1,4 ms, in denen der Kern auf dem Bus wartet. Ein Blick auf den Pin
//! kostet EIN Register.
//!
//! Der Registeraufbau ist AMD-eigen. Ein Intel-Block (`INT34BB` auf
//! Florians HP) fuehrt an derselben Stelle etwas anderes — deshalb wird
//! [`is_amd_block`] gefragt, bevor irgendetwas gelesen wird, und nicht
//! geraten.
```

## L27 · `pub const PIN_STS: u32 = 1 << 16;`

```
/// `PIN_STS_OFF` (pinctrl-amd.h) — der Pegel, den der Pin GERADE fuehrt.
```

## L30-31 · `pub const AMD_GPIO_IDS: [&str; 3] = ["AMD0030", "AMDI0030", "AMDI0031"];`

```
/// Die ACPI-Kennungen, unter denen Linux `pinctrl-amd` bindet
/// (`amd_gpio_acpi_match`).
```

## L34 · `pub fn is_amd_block(ids: &[String]) -> bool {`

```
/// Ist der GPIO-Block einer, dessen Register wir kennen?
```

## L39 · `#[derive(Clone, Copy, Debug, PartialEq)]`

```
/// Wohin der Pin abgebildet werden muss, und wo sein Register dann liegt.
```

## L42 · `pub map_base: u32,`

```
/// Seitenausgerichtete Basis — `npk_mmio_map_phys` weist alles andere ab.
```

## L44 · `pub pages: u32,`

```
/// Wieviele Seiten, um DIESEN Pin zu erreichen.
```

## L46 · `pub reg_off: u32,`

```
/// Versatz seines Registers, relativ zu `map_base`.
```

## L50-59 · `pub fn pin_window(mmio_base: u32, mmio_len: u32, pin: u16) -> Option<PinWindow> {`

```
/// Das Fenster fuer einen Pin ausrechnen.
///
/// Der AMD-Block steht in der Firmware typisch als
/// `Memory32Fixed(ReadWrite, 0xFED81500, 0x300)` — also NICHT
/// seitenausgerichtet. Abgebildet wird deshalb ab der Seite darunter, und
/// der Rest der Adresse faehrt in `reg_off` mit.
///
/// `None`, wenn der Pin ausserhalb des angesagten Fensters liegt: dann
/// haben wir den falschen Block oder den falschen Pin, und ein Register
/// daneben zu lesen waere geraten.
```

## L71-72 · `let pages = (reg_off + 4 + 4095) / 4096;`

```
// Nur so viele Seiten, wie dieser Pin braucht — der ganze Block waere
// bei einem 64-KB-Fenster mehr als die sechzehn, die der Kernel gibt.
```

## L80-85 · `pub const LEVEL_TRIG: u32 = 1 << 8; // LEVEL_TRIG_OFF`

```
// ── Interrupts: `pinctrl-amd.{c,h}` ──────────────────────────────────
//
// Ein Register je Pin; die Bits aus pinctrl-amd.h. Der Block hat EINE
// Leitung fuer alle Pins (`_CRS`), und `do_amd_gpio_irq_handler` quittiert
// zuerst den Pin (das gelesene Register zurueckschreiben loescht die
// Statusbits) und dann die Einheit (`EOI_MASK` im WAKE_INT_MASTER_REG).
```

## L87 · `pub const LEVEL_TRIG: u32 = 1 << 8; // LEVEL_TRIG_OFF`

```
// LEVEL_TRIG_OFF
```

## L88 · `pub const ACTIVE_LEVEL_SHIFT: u32 = 9; // ACTIVE_LEVEL_OFF, 2 bits`

```
// ACTIVE_LEVEL_OFF, 2 bits
```

## L91 · `pub const INTERRUPT_MASK: u32 = 1 << 12; // 1 = NOT masked (irq_unmask sets it)`

```
// 1 = NOT masked (irq_unmask sets it)
```

## L94 · `pub const PIN_IRQ_PENDING: u32 = INTERRUPT_STS | WAKE_STS;`

```
/// `PIN_IRQ_PENDING`
```

## L96 · `pub const WAKE_INT_MASTER_REG: u32 = 0xfc;`

```
/// Relative to the block base.
```

## L98 · `pub const WAKE_INT_STATUS_REG0: u32 = 0x2f8;`

```
/// Which groups of four pins have something pending (bits 0-45).
```

## L103-106 · `pub fn irq_level_config(pin_reg: u32, active_low: bool) -> u32 {`

```
/// `amd_gpio_irq_set_type` for a LEVEL line (the only kind HID over I2C
/// uses): level trigger, the polarity, and `CLR_INTR_STAT` so a status left
/// from before is cleared. Returns the value WITHOUT the enable bit; the
/// caller does the debounce-settle dance and then `amd_gpio_irq_enable`.
```

## L114-121 · `pub fn asserted(pin_reg: u32, active_low: bool) -> bool {`

```
/// Sagt der Pin „ich habe etwas"?
///
/// `active_low` kommt aus dem `GpioInt` der Firmware (ACPI: `int_flags`
/// Bits 2:1), nicht aus einer Annahme.
///
/// **Lauter Einsen heissen: da hat niemand geantwortet.** Dann meldet
/// diese Funktion JA und der Rufer liest. Eine Fehlmeldung kostet eine
/// Busuebertragung; ein verschlucktes Ja kostet den Zeiger.
```

## L135-136 · `#[test]`

```
/// `amd_gpio_irq_set_type(IRQ_TYPE_LEVEL_LOW)`: Pegel, aktiv-niedrig,
/// Status loeschen — und die Pull-/Ausgangsbits unangetastet.
```

## L139 · `let before = (1 << 20) | (0x2 << ACTIVE_LEVEL_SHIFT); // pull-up, both-edges`

```
// pull-up, both-edges
```

## L150 · `#[test]`

```
/// Die zwei Pins aus Florians IdeaPad, gegen den ueblichen AMD-Block.
```

## L164 · `#[test]`

```
/// Ein Pin ausserhalb des angesagten Fensters ist kein Pin.
```

## L167 · `assert!(pin_window(0xFED8_1500, 0x300, 191).is_some());`

```
// 0x300 Bytes sind 192 Register: 0..=191.
```

## L173-174 · `#[test]`

```
/// Ein grosses Fenster darf nicht die ganze Abbildung sprengen: es
/// werden nur die Seiten bis zum Pin verlangt.
```

## L182 · `assert!(pin_window(0xFD00_0000, 0x10_0000, 20_000).is_none());`

```
// 16 Seiten sind der Deckel des Kernels.
```

## L186 · `#[test]`

```
/// Der Pegel, und die Polaritaet aus der Firmware.
```

## L189 · `assert!(!asserted(PIN_STS, true));`

```
// Aktiv LOW: Bit 16 gesetzt heisst „nichts da".
```

## L192 · `assert!(asserted(PIN_STS, false));`

```
// Aktiv HIGH: genau andersherum.
```

## L195-196 · `let noise = 0xFFFF_FFFFu32 & !PIN_STS;`

```
// Andere Bits duerfen nicht mitreden: Treiberstaerke, Pull-ups,
// Wake — alles gesetzt, nur Bit 16 nicht.
```

## L202 · `#[test]`

```
/// Ein Block, der gar nicht antwortet, darf den Zeiger nicht toeten.
```

## L209-210 · `#[test]`

```
/// Der Registeraufbau ist AMD-eigen — ein Intel-Block wird nicht
/// angefasst.
```

