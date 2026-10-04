# `kernel/src/drivers/sci.rs` @ 5e0102684

## L1-11 · `use crate::serial::{inb, inw, outb, outw};`

```
//! ACPI SCI: the fixed and general-purpose event registers of the FADT.
//!
//! The kernel owns the registers; the AML driver (`aml`) owns the meaning.
//! It asks for its EC's GPE with `npk_sci_arm(gpe)`, gets the SCI as an
//! ordinary level interrupt (oneshot: masked on fire, unmasked by the next
//! `npk_wait`), and on every wake calls `npk_sci_service` — which acks what
//! fired — before it drains the EC and runs the `_Qxx` methods.
//!
//! Order as ACPICA for an EDGE GPE (Linux `ec.c` installs its handler edge-
//! triggered): clear the status bit first, then handle, so an event that
//! arrives during the handling sets the bit again and fires again.
```

## L17 · `pub static VECTOR: AtomicU32 = AtomicU32::new(0);`

```
/// Vector the SCI was registered on (for `report`).
```

## L19 · `static CALLS: AtomicU32 = AtomicU32::new(0);`

```
/// `service` calls, and how many found the EC GPE / a PM1 event / nothing.
```

## L24 · `static EC_GPE: AtomicU32 = AtomicU32::new(0);`

```
/// GPE number the EC signals on.
```

## L39 · `let (sci, pm1a, pm1_len, gpe0, gpe0_len) = unsafe {`

```
// SAFETY: FADT mapped; fields at their ACPI 6.5 §5.2.9 offsets.
```

## L58-60 · `pub fn arm_ec(gpe: u32) -> Option<(u32, bool, bool)> {`

```
/// Take the SCI for the EC's `gpe`: every other GPE disabled (ACPICA
/// disables all at init and enables per handler), every status cleared,
/// `gpe` enabled. Returns the SCI's (GSI, level, active-low). Once only.
```

## L62-63 · `if crate::config::get("acpi.legacy").as_deref() == Some("1") {`

```
// Whoever takes the SCI switches to ACPI mode; `acpi.legacy = 1` keeps
// the firmware in legacy mode.
```

## L73-74 · `unsafe {`

```
// SAFETY: GPE0 and PM1a are the FADT's I/O blocks; status bits are
// write-1-to-clear, enable bits plain.
```

## L92-97 · `pub fn service() -> u32 {`

```
/// Ack what raised the SCI. Bit 0: the EC's GPE was set. Bit 1: the EC has
/// an EVENT to query (status SCI_EVT) — the EC also raises its GPE when a
/// transaction's output is ready, so bit 0 alone is mostly our own reads
/// (Linux `ec.c` queries only on SCI_EVT).
/// Bits 16..31: the PM1 fixed events that were set AND enabled (bit 8 =
/// power button).
```

## L104 · `unsafe {`

```
// SAFETY: as in `arm_ec`.
```

## L122 · `if unsafe { inb(0x66) } & 0x20 != 0 {`

```
// SAFETY: EC status port, read only.
```

## L132-134 · `pub fn report() {`

```
/// Counters and the raw registers, for `ec watch`: a line that keeps
/// firing shows as `fired` far above `EC`, with the culprit's status bit
/// standing in the dump.
```

## L148 · `unsafe {`

```
// SAFETY: GPE0 / PM1a are the FADT's I/O blocks; reads only.
```

## L159-160 · `pub fn set_ec_gpe(on: bool) -> bool {`

```
/// `ec gpe off|on` — A/B for power measurements: take the EC's GPE out of
/// the SCI (events are then only drained on aml's 10-s battery round).
```

## L166 · `unsafe {`

```
// SAFETY: GPE0 enable byte of the FADT's block.
```

## L175-177 · `pub fn set_acpi_mode(acpi: bool) -> Option<bool> {`

```
/// `ec mode legacy|acpi` — A/B: hand the events back to the firmware (SMM)
/// with FADT.ACPI_DISABLE, or take them again with ACPI_ENABLE. Returns
/// SCI_EN afterwards.
```

## L181-182 · `let (smi, en, dis, cnt) = unsafe {`

```
// SAFETY: FADT mapped; SMI_CMD 48, ACPI_ENABLE 52, ACPI_DISABLE 53,
// PM1a_CNT 64.
```

## L190 · `unsafe { outb(smi as u16, if acpi { en } else { dis }) };`

```
// SAFETY: the FADT's SMI command port and PM1a control port.
```

