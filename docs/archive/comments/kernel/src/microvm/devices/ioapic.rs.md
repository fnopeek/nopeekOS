# `kernel/src/microvm/devices/ioapic.rs` @ 5e0102684

## L1-14 · `use crate::microvm::cpu::svm::lapic;`

```
//! I/O APIC, ported from Linux `arch/x86/kvm/ioapic.c` (version 0x11, 24
//! pins, MMIO at 0xFEC00000: IOREGSEL at +0x00, IOWIN at +0x10).
//!
//! The guest's device lines reach it and the 8259 alike (KVM's default GSI
//! routing); which one delivers is the guest's choice — with an I/O APIC in
//! the MP table Linux masks the 8259. A serviced pin becomes a fixed-vector
//! message to its destination LAPIC(s) through the posted-interrupt path
//! (`lapic::deliver_ipi`), so a vCPU on another core is kicked like for an IPI.
//!
//! Every pin our devices drive is declared edge-triggered in the MP table
//! (they signal completions as pulses). A pin the guest nonetheless programs
//! level-triggered is served like an edge and never sets Remote IRR: without
//! the LAPIC's EOI broadcast wired back here, a set Remote IRR would block the
//! pin for good.
```

## L22 · `const VERSION: u32 = 0x11 | ((NUM_PINS as u32 - 1) << 16);`

```
/// `IOAPIC_VERSION_ID`, max redirection entry 23 in bits 23:16.
```

## L33 · `const RTE_VECTOR: u64 = 0xFF;`

```
// Redirection entry fields (`union kvm_ioapic_redirect_entry`).
```

## L42-43 · `pub fn isa_pin(irq: u8) -> usize {`

```
/// ISA IRQ → pin, as the MP table declares it: IRQ 0 (the PIT) on pin 2,
/// every other line on its own number.
```

## L51 · `irr: u32,`

```
/// Line state per pin (edge detection).
```

## L57 · `pub const fn new() -> Self {`

```
/// `kvm_ioapic_reset`: every pin masked.
```

## L62 · `pub fn set_id(&mut self, id: u8) { self.id = id as u32; }`

```
/// The APIC ID the MP table gives the I/O APIC (after the vCPUs').
```

## L65 · `pub fn set_irq(&mut self, pin: usize, level: bool, from: u8) {`

```
/// `ioapic_set_irq` for an edge pin: deliver on the rising edge.
```

## L80 · `pub fn pulse(&mut self, pin: usize, from: u8) {`

```
/// A device event: a rising and a falling edge.
```

## L86 · `fn service(&mut self, pin: usize, from: u8) {`

```
/// `ioapic_service` + `kvm_irq_delivery_to_apic`.
```

## L99 · `fn read_indirect(&self) -> u32 {`

```
/// `ioapic_read_indirect`.
```

## L113 · `fn write_indirect(&mut self, val: u32) {`

```
/// `ioapic_write_indirect`: Remote IRR and delivery status are read-only.
```

## L128 · `}`

```
// Edge pins never hold Remote IRR (KVM clears it the same way).
```

## L135 · `pub fn mmio(&mut self, off: u32, write: Option<u32>) -> Option<u32> {`

```
/// MMIO at `off` into the I/O APIC page. `Some(value)` for a read.
```

