# `kernel/src/ioapic.rs` @ 5e0102684

## L1-25 · `use alloc::vec::Vec;`

```
//! I/O APIC — the router for interrupts that are not MSI.
//!
//! Everything so far came in as MSI/MSI-X, which goes straight to a LAPIC.
//! What does not — the PS/2 keyboard (ISA IRQ 1), the ACPI SCI (EC and
//! battery events), a GPIO controller's line (the IdeaPad touchpads) — goes
//! through an I/O APIC: one redirection entry per input pin ("GSI") says
//! which vector, which core, edge or level, high or low.
//! `docs/plan/CORES_AND_EVENTS.md`, Stufe 3a.
//!
//! Ported from Linux `arch/x86/kernel/apic/io_apic.c` (register access,
//! entry layout, `clear_IO_APIC_pin`, `__eoi_ioapic_pin`) and
//! `arch/x86/kernel/acpi/boot.c` (MADT I/O APIC and Interrupt Source
//! Override entries, `mp_override_legacy_irq`).
//!
//! **One deliberate difference from Linux at boot:** Linux masks every pin
//! (`clear_IO_APIC`). We leave pins with SMI or ExtINT delivery as the
//! firmware set them, like Linux leaves SMI pins: the PIT still reaches
//! Core 0 through the legacy PIC on some machines, and that path may run
//! through an ExtINT pin ("virtual wire"). Stage 3e removes the PIT tick;
//! then this exception can go. Every other pin is masked until a driver
//! routes it.
//!
//! Locking: the index/data register pair must not be interleaved, and the
//! device-IRQ ISR masks level lines — so every access takes `LOCK` with
//! interrupts off on the calling core (`without_interrupts`).
```

## L32 · `#[derive(Clone, Copy)]`

```
/// One I/O APIC from the MADT.
```

## L42-43 · `#[derive(Clone, Copy)]`

```
/// MADT type 2: ISA IRQ `bus_irq` arrives on `gsi` with MPS INTI `flags`
/// (bits 1:0 polarity, 3:2 trigger; 0 = conforms to the bus).
```

## L54 · `routed: Vec<u32>,`

```
/// GSIs a route has been written for — one owner each.
```

## L60 · `const RTE_DELIVERY_SHIFT: u32 = 8; // 3 bits`

```
// Redirection entry, low dword (io_apic.h `struct IO_APIC_route_entry`).
```

## L61 · `const RTE_DELIVERY_SHIFT: u32 = 8; // 3 bits`

```
// 3 bits
```

## L70-71 · `unsafe {`

```
// SAFETY: the I/O APIC's register window was mapped NO_CACHE in `init`;
// IOREGSEL at +0, IOWIN at +0x10 (`struct io_apic`).
```

## L79 · `unsafe {`

```
// SAFETY: as in `read`.
```

## L90-91 · `fn write_entry(a: &IoApic, pin: u32, lo: u32, hi: u32) {`

```
/// `__ioapic_write_entry`: the high dword first, so the entry is never live
/// with a new vector and an old destination.
```

## L97 · `fn eoi_pin(a: &IoApic, pin: u32, vector: u8) {`

```
/// `__eoi_ioapic_pin`: clear a level line's Remote-IRR.
```

## L100 · `unsafe { core::ptr::write_volatile((a.base + 0x40) as *mut u32, vector as u32) };`

```
// SAFETY: the EOI register at +0x40 exists from version 0x20 on.
```

## L103 · `let (lo, hi) = read_entry(a, pin);`

```
// Masked and edge for a moment, then the level entry again.
```

## L110 · `fn clear_pin(a: &IoApic, pin: u32) {`

```
/// `clear_IO_APIC_pin`, minus ExtINT (see the module note).
```

## L123 · `if lo & RTE_LEVEL == 0 {`

```
// An explicit EOI clears Remote-IRR only in level mode.
```

## L130 · `write_entry(a, pin, RTE_MASKED, 0);`

```
// `ioapic_mask_entry`: everything cleared except the mask bit.
```

## L137-139 · `pub fn init() {`

```
/// Parse the MADT's I/O APICs and overrides, map them, and mask every pin
/// the firmware did not reserve. Changes nothing a driver relies on: no
/// interrupt reaches us through an I/O APIC before this.
```

## L145 · `let len = unsafe { *((madt + 4) as *const u32) } as usize;`

```
// SAFETY: the MADT header is identity-mapped; length checked below.
```

## L156 · `let ty = unsafe { *((madt + off) as *const u8) };`

```
// SAFETY: inside the MADT, bounds checked by the loop and entry length.
```

## L163 · `1 if elen >= 12 => {`

```
// Type 1: I/O APIC — id, reserved, address (u32), GSI base (u32).
```

## L165 · `let (id, addr, gsi_base) = unsafe {`

```
// SAFETY: elen >= 12 covers the fields read.
```

## L173 · `2 if elen >= 10 => {`

```
// Type 2: Interrupt Source Override — bus, source, GSI, flags.
```

## L175 · `let (bus_irq, gsi, flags) = unsafe {`

```
// SAFETY: elen >= 10 covers the fields read.
```

## L195 · `let r1 = read(a, 1); // IOAPIC version register`

```
// IOAPIC version register
```

## L221-223 · `pub fn isa_irq(irq: u8) -> (u32, bool, bool) {`

```
/// Where ISA IRQ `irq` arrives, and how: (GSI, level, active_low).
/// `mp_override_legacy_irq`: an override wins; ISA conforms to edge/high;
/// a level override on IRQ 0 is a known firmware bug and taken as edge.
```

## L249-251 · `pub fn route(gsi: u32, vector: u8, dest_apic: u32, level: bool, active_low: bool) -> bool {`

```
/// Route `gsi` to `vector` on the LAPIC `dest_apic` (fixed delivery,
/// physical destination), left MASKED — `unmask` when the driver is ready.
/// False if no I/O APIC serves this GSI.
```

## L256 · `return false; // one owner per line`

```
// one owner per line
```

## L271 · `pub fn is_free(gsi: u32) -> bool {`

```
/// Is `gsi` served by an I/O APIC and still without an owner?
```

## L294 · `pub fn set_dest(gsi: u32, dest_apic: u32) {`

```
/// Re-point `gsi` at LAPIC `dest_apic` (the core that waits on it).
```

