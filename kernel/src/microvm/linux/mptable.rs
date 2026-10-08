//! Intel MP-table builder — enumerates the guest's vCPUs for a Linux
//! guest booted `acpi=off` (no MADT).
//!
//! Linux scans three fixed windows for the 16-byte floating pointer
//! `_MP_` (`mpparse_find_mptable`, mpparse.c:612): `[0,0x400)`,
//! `[0x9FC00,0xA0000)`, `[0xF0000,0x10000)`. We place the floating
//! pointer at 0xF0000 (the BIOS window, RESERVED in our e820 so Linux
//! won't reuse it) and the config table right behind it at 0xF0010.
//!
//! Layout + validation are ported 1:1 from the Linux kernel that parses it:
//!   * structs — `arch/x86/include/asm/mpspec_def.h`
//!   * scan / checksum / parse — `arch/x86/kernel/mpparse.c`
//!     (`smp_scan_config`, `smp_check_mpc`, `smp_read_mpc`).
//!
//! Emits the floating pointer + a `PCMP` header (non-zero LAPIC address,
//! mandatory) + one `mpc_cpu` (type 0) entry per vCPU; with the I/O APIC
//! also the bus / IOAPIC / INTSRC / LINTSRC entries of `io_entries`.

use crate::microvm::devices::guest_mem::GuestMem;
use crate::microvm::devices::ioapic::isa_pin;
use crate::microvm::devices::pci_bus::INTX_LINES;

/// Floating pointer — first 16-byte slot of the BIOS scan window.
const MPF_GUEST_PHYS: u64 = 0xF_0000;
/// Config table — directly behind the floating pointer.
const MPC_GUEST_PHYS: u64 = 0xF_0010;

/// LAPIC MMIO base reported in the config table. Must be non-zero or
/// `smp_check_mpc` rejects the whole table (mpparse.c:156). Matches
/// `svm::lapic::LAPIC_BASE` / `APIC_DEFAULT_PHYS_BASE`.
const LAPIC_PHYS: u32 = 0xFEE0_0000;

/// MP spec version we claim (1.4). `smp_scan_config` accepts 1 or 4 in
/// the floating pointer; `smp_check_mpc` accepts 0x01 or 0x04 in the
/// config header.
const MP_SPEC: u8 = 4;

const MP_PROCESSOR: u8 = 0; // entry types
const MP_BUS: u8 = 1;
const MP_IOAPIC: u8 = 2;
const MP_INTSRC: u8 = 3;
const MP_LINTSRC: u8 = 4;
// mp_irq_source_types
const MP_INT: u8 = 0;
const MP_NMI: u8 = 1;
const MP_EXTINT: u8 = 3;
/// irqflag: active-high, edge — what our devices send (pulses).
const MP_IRQ_EDGE_HIGH: u16 = 0x1 | (0x1 << 2);
const MPC_IOAPIC_ENABLED: u8 = 0x01;
const IOAPIC_VERSION: u8 = 0x11;
const IOAPIC_PHYS: u32 = 0xFEC0_0000;
const MPC_BUS_LEN: usize = 8;
const MPC_IOAPIC_LEN: usize = 8;
const MPC_INTSRC_LEN: usize = 8;
/// MP bus IDs. The PCI bus must carry ID 0: `IO_APIC_get_PCI_irq_vector`
/// matches an entry's source bus against the PCI bus number.
const BUS_PCI: u8 = 0;
const BUS_ISA: u8 = 1;
const CPU_ENABLED: u8 = 0x01;
const CPU_BOOTPROCESSOR: u8 = 0x02;
/// Integrated xAPIC version (matches `svm::lapic` LVR low byte).
const APIC_VERSION: u8 = 0x14;

const MPF_LEN: usize = 16;
const MPC_HEADER_LEN: usize = 44;
const MPC_CPU_LEN: usize = 20;

/// One's-complement checksum byte: makes the byte-sum over `buf` zero
/// mod 256, as both `mpf_checksum` callers require.
fn checksum(buf: &[u8]) -> u8 {
    let sum = buf.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    sum.wrapping_neg()
}

/// Append one `mpc_cpu` (type 0, 20 bytes) entry. `cpufeature` /
/// `featureflag` are unused by Linux's `MP_processor_info` (it only
/// calls `topology_register_apic`), so we leave them zero.
fn push_cpu(buf: &mut alloc::vec::Vec<u8>, apicid: u8, bsp: bool) {
    let cpuflag = CPU_ENABLED | if bsp { CPU_BOOTPROCESSOR } else { 0 };
    buf.push(MP_PROCESSOR);
    buf.push(apicid);
    buf.push(APIC_VERSION);
    buf.push(cpuflag);
    buf.extend_from_slice(&[0u8; 4]); // cpufeature
    buf.extend_from_slice(&[0u8; 4]); // featureflag
    buf.extend_from_slice(&[0u8; 8]); // reserved[2]
}

/// The I/O part (`with_ioapic`): a PCI and an ISA bus, the I/O APIC at
/// 0xFEC00000 with APIC ID `ncpu`, one source per ISA IRQ (IRQ 0 → pin 2,
/// n → n; IRQ 2 is the cascade), one per PCI slot's INTA onto the pin of
/// its line, and the local sources (ExtINT on LINT0, NMI on LINT1). Every
/// source is explicitly edge/active-high — our devices send pulses — so
/// Linux does not fall back to the PCI default of level-low.
fn io_entries(ncpu: u8) -> (alloc::vec::Vec<u8>, u16) {
    let mut v = alloc::vec::Vec::new();
    let mut count = 0u16;
    for (id, name) in [(BUS_PCI, b"PCI   "), (BUS_ISA, b"ISA   ")] {
        v.extend_from_slice(&[MP_BUS, id]);
        v.extend_from_slice(name);
        count += 1;
    }
    v.extend_from_slice(&[MP_IOAPIC, ncpu, IOAPIC_VERSION, MPC_IOAPIC_ENABLED]);
    v.extend_from_slice(&IOAPIC_PHYS.to_le_bytes());
    count += 1;
    let mut intsrc = |bus: u8, busirq: u8, pin: u8| {
        v.extend_from_slice(&[MP_INTSRC, MP_INT]);
        v.extend_from_slice(&MP_IRQ_EDGE_HIGH.to_le_bytes());
        v.extend_from_slice(&[bus, busirq, ncpu, pin]);
        count += 1;
    };
    for irq in 0u8..16 {
        if irq == 2 { continue; }
        intsrc(BUS_ISA, irq, isa_pin(irq) as u8);
    }
    // srcbusirq for PCI: slot in bits 6:2, pin (0 = INTA) in bits 1:0.
    for (slot, line) in INTX_LINES {
        intsrc(BUS_PCI, slot << 2, isa_pin(line) as u8);
    }
    for (kind, lint) in [(MP_EXTINT, 0u8), (MP_NMI, 1u8)] {
        v.extend_from_slice(&[MP_LINTSRC, kind]);
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&[0, 0, 0xFF, lint]);
        count += 1;
    }
    debug_assert_eq!(
        v.len(),
        2 * MPC_BUS_LEN + MPC_IOAPIC_LEN + (count as usize - 3) * MPC_INTSRC_LEN,
    );
    (v, count)
}

/// Build + write the floating pointer and a `PCMP` config table with
/// `ncpu` processor entries (apicid 0..ncpu, 0 = BSP) into the guest's
/// BIOS window, plus the I/O APIC part when `with_ioapic`. Returns false if
/// a guest-RAM write is rejected.
pub fn install(mem: &GuestMem, ncpu: u8, with_ioapic: bool) -> bool {
    let (io, io_count) = if with_ioapic { io_entries(ncpu) } else { (alloc::vec::Vec::new(), 0) };
    // ── Config table: PCMP header + N processor entries (+ I/O part) ──
    let total_len = MPC_HEADER_LEN + MPC_CPU_LEN * ncpu as usize + io.len();
    let mut mpc = alloc::vec::Vec::with_capacity(total_len);
    mpc.extend_from_slice(b"PCMP");
    mpc.extend_from_slice(&(total_len as u16).to_le_bytes()); // length
    mpc.push(MP_SPEC);
    mpc.push(0); // checksum, filled below
    mpc.extend_from_slice(b"NOPEEK  "); // oem[8]
    mpc.extend_from_slice(b"MICROVM     "); // productid[12]
    mpc.extend_from_slice(&0u32.to_le_bytes()); // oemptr
    mpc.extend_from_slice(&0u16.to_le_bytes()); // oemsize
    mpc.extend_from_slice(&(ncpu as u16 + io_count).to_le_bytes()); // oemcount (entry count)
    mpc.extend_from_slice(&LAPIC_PHYS.to_le_bytes()); // lapic
    mpc.extend_from_slice(&0u32.to_le_bytes()); // reserved
    debug_assert_eq!(mpc.len(), MPC_HEADER_LEN);
    for id in 0..ncpu {
        push_cpu(&mut mpc, id, id == 0);
    }
    mpc.extend_from_slice(&io);
    debug_assert_eq!(mpc.len(), total_len);
    mpc[7] = checksum(&mpc);

    // ── Floating pointer (16 bytes, length field = 1 paragraph) ──────
    let mut mpf = [0u8; MPF_LEN];
    mpf[0..4].copy_from_slice(b"_MP_");
    mpf[4..8].copy_from_slice(&(MPC_GUEST_PHYS as u32).to_le_bytes()); // physptr
    mpf[8] = 1; // length (paragraphs) — smp_scan_config requires == 1
    mpf[9] = MP_SPEC; // specification
    // mpf[10] checksum, mpf[11] feature1 = 0 (config table present, not
    // a default config), mpf[12] feature2 = 0 (virtual-wire, no IMCR).
    mpf[10] = checksum(&mpf);

    mem.write_bytes(MPC_GUEST_PHYS, &mpc) && mem.write_bytes(MPF_GUEST_PHYS, &mpf)
}
