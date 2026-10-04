# `kernel/src/microvm/linux/mptable.rs` @ 5e0102684

## L1-21 · `use crate::microvm::devices::guest_mem::GuestMem;`

```
//! Intel MP-table builder — enumerates the guest's vCPUs for a Linux
//! guest booted `acpi=off` (no MADT). Guest-SMP Stage 2.
//!
//! Linux scans three fixed windows for the 16-byte floating pointer
//! `_MP_` (`mpparse_find_mptable`, mpparse.c:612): `[0,0x400)`,
//! `[0x9FC00,0xA0000)`, `[0xF0000,0x10000)`. We place the floating
//! pointer at 0xF0000 (the BIOS window, RESERVED in our e820 so Linux
//! won't reuse it) and the config table right behind it at 0xF0010.
//!
//! Layout + validation are ported 1:1 from the kernel that runs against
//! it (`~/.cache/nopeekos/linux-src/linux-6.18.26`):
//!   * structs — `arch/x86/include/asm/mpspec_def.h`
//!   * scan / checksum / parse — `arch/x86/kernel/mpparse.c`
//!     (`smp_scan_config`, `smp_check_mpc`, `smp_read_mpc`).
//!
//! Stage 2 emits the minimum that makes Linux count 2 CPUs: the floating
//! pointer + a `PCMP` header (non-zero LAPIC address, mandatory) + two
//! `mpc_cpu` (type 0) entries; with the I/O APIC also the bus / IOAPIC /
//! INTSRC / LINTSRC entries of `io_entries`. The AP is enumerated but not started:
//! INIT/SIPI at the LAPIC ICR is decoded + logged in `svm::lapic`, Linux
//! times out on the AP and continues with 1 CPU online (Stage 3 spawns it).
```

## L25 · `const MPF_GUEST_PHYS: u64 = 0xF_0000;`

```
/// Floating pointer — first 16-byte slot of the BIOS scan window.
```

## L27 · `const MPC_GUEST_PHYS: u64 = 0xF_0010;`

```
/// Config table — directly behind the floating pointer.
```

## L30-32 · `const LAPIC_PHYS: u32 = 0xFEE0_0000;`

```
/// LAPIC MMIO base reported in the config table. MUST be non-zero or
/// `smp_check_mpc` rejects the whole table (mpparse.c:156). Matches
/// `svm::lapic::LAPIC_BASE` / `APIC_DEFAULT_PHYS_BASE`.
```

## L35-37 · `const MP_SPEC: u8 = 4;`

```
/// MP spec version we claim (1.4). `smp_scan_config` accepts 1 or 4 in
/// the floating pointer; `smp_check_mpc` accepts 0x01 or 0x04 in the
/// config header.
```

## L40 · `const MP_PROCESSOR: u8 = 0; // entry types`

```
// entry types
```

## L45 · `const MP_INT: u8 = 0;`

```
// mp_irq_source_types
```

## L49 · `const MP_IRQ_EDGE_HIGH: u16 = 0x1 | (0x1 << 2);`

```
/// irqflag: active-high, edge — what our devices send (pulses).
```

## L59 · `const APIC_VERSION: u8 = 0x14;`

```
/// Integrated xAPIC version (matches `svm::lapic` LVR low byte).
```

## L66-67 · `fn checksum(buf: &[u8]) -> u8 {`

```
/// One's-complement checksum byte: makes the byte-sum over `buf` zero
/// mod 256, as both `mpf_checksum` callers require.
```

## L73-75 · `fn push_cpu(buf: &mut alloc::vec::Vec<u8>, apicid: u8, bsp: bool) {`

```
/// Append one `mpc_cpu` (type 0, 20 bytes) entry. `cpufeature` /
/// `featureflag` are unused by Linux 6.18's `MP_processor_info` (it only
/// calls `topology_register_apic`), so we leave them zero.
```

## L82 · `buf.extend_from_slice(&[0u8; 4]); // cpufeature`

```
// cpufeature
```

## L83 · `buf.extend_from_slice(&[0u8; 4]); // featureflag`

```
// featureflag
```

## L84 · `buf.extend_from_slice(&[0u8; 8]); // reserved[2]`

```
// reserved[2]
```

## L87-93 · `fn io_entries(ncpu: u8) -> (alloc::vec::Vec<u8>, u16) {`

```
/// The I/O part (`with_ioapic`): an ISA bus, the I/O APIC at 0xFEC00000
/// with APIC ID `ncpu`, one explicit edge/active-high source per ISA IRQ
/// (IRQ 0 → pin 2, n → n; IRQ 2 is the cascade), and the local sources
/// (ExtINT on LINT0, NMI on LINT1). Explicit, so Linux does not guess: with
/// no bus entry `mp_bus_not_pci` is clear and bus 0 counts as PCI, whose
/// default is level-low. PCI devices find no entry and keep their config-
/// space line (`pirq_enable_irq`) — an ISA IRQ here, routed as above.
```

## L124-127 · `pub fn install(mem: &GuestMem, ncpu: u8, with_ioapic: bool) -> bool {`

```
/// Build + write the floating pointer and a `PCMP` config table with
/// `ncpu` processor entries (apicid 0..ncpu, 0 = BSP) into the guest's
/// BIOS window, plus the I/O APIC part when `with_ioapic`. Returns false if
/// a guest-RAM write is rejected.
```

## L130 · `let total_len = MPC_HEADER_LEN + MPC_CPU_LEN * ncpu as usize + io.len();`

```
// ── Config table: PCMP header + N processor entries (+ I/O part) ──
```

## L134 · `mpc.extend_from_slice(&(total_len as u16).to_le_bytes()); // length`

```
// length
```

## L136 · `mpc.push(0); // checksum, filled below`

```
// checksum, filled below
```

## L137 · `mpc.extend_from_slice(b"NOPEEK  "); // oem[8]`

```
// oem[8]
```

## L138 · `mpc.extend_from_slice(b"MICROVM     "); // productid[12]`

```
// productid[12]
```

## L139 · `mpc.extend_from_slice(&0u32.to_le_bytes()); // oemptr`

```
// oemptr
```

## L140 · `mpc.extend_from_slice(&0u16.to_le_bytes()); // oemsize`

```
// oemsize
```

## L141 · `mpc.extend_from_slice(&(ncpu as u16 + io_count).to_le_bytes()); // oemcount (entry count)`

```
// oemcount (entry count)
```

## L142 · `mpc.extend_from_slice(&LAPIC_PHYS.to_le_bytes()); // lapic`

```
// lapic
```

## L143 · `mpc.extend_from_slice(&0u32.to_le_bytes()); // reserved`

```
// reserved
```

## L152 · `let mut mpf = [0u8; MPF_LEN];`

```
// ── Floating pointer (16 bytes, length field = 1 paragraph) ──────
```

## L155 · `mpf[4..8].copy_from_slice(&(MPC_GUEST_PHYS as u32).to_le_bytes()); // physptr`

```
// physptr
```

## L156 · `mpf[8] = 1; // length (paragraphs) — smp_scan_config requires == 1`

```
// length (paragraphs) — smp_scan_config requires == 1
```

## L157 · `mpf[9] = MP_SPEC; // specification`

```
// specification
```

## L158-159 · `mpf[10] = checksum(&mpf);`

```
// mpf[10] checksum, mpf[11] feature1 = 0 (config table present, not
// a default config), mpf[12] feature2 = 0 (virtual-wire, no IMCR).
```

