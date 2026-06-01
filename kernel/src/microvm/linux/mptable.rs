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
//! `mpc_cpu` (type 0) entries. No bus / IOAPIC / INTSRC entries — we boot
//! `noapic`, device IRQs go through the 8259 PIC, and `smp_read_mpc`
//! accepts a processor-only table. The AP is enumerated but not started:
//! INIT/SIPI at the LAPIC ICR is decoded + logged in `svm::lapic`, Linux
//! times out on the AP and continues with 1 CPU online (Stage 3 spawns it).

use crate::microvm::devices::guest_mem::GuestMem;

/// Floating pointer — first 16-byte slot of the BIOS scan window.
const MPF_GUEST_PHYS: u64 = 0xF_0000;
/// Config table — directly behind the floating pointer.
const MPC_GUEST_PHYS: u64 = 0xF_0010;

/// LAPIC MMIO base reported in the config table. MUST be non-zero or
/// `smp_check_mpc` rejects the whole table (mpparse.c:156). Matches
/// `svm::lapic::LAPIC_BASE` / `APIC_DEFAULT_PHYS_BASE`.
const LAPIC_PHYS: u32 = 0xFEE0_0000;

/// MP spec version we claim (1.4). `smp_scan_config` accepts 1 or 4 in
/// the floating pointer; `smp_check_mpc` accepts 0x01 or 0x04 in the
/// config header.
const MP_SPEC: u8 = 4;

const MP_PROCESSOR: u8 = 0; // entry type
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
/// `featureflag` are unused by Linux 6.18's `MP_processor_info` (it only
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

/// Build + write the floating pointer and a `PCMP` config table with
/// `ncpu` processor entries (apicid 0..ncpu, 0 = BSP) into the guest's
/// BIOS window. Returns false if a guest-RAM write is rejected.
pub fn install(mem: &GuestMem, ncpu: u8) -> bool {
    // ── Config table: PCMP header + N processor entries ──────────────
    let total_len = MPC_HEADER_LEN + MPC_CPU_LEN * ncpu as usize;
    let mut mpc = alloc::vec::Vec::with_capacity(total_len);
    mpc.extend_from_slice(b"PCMP");
    mpc.extend_from_slice(&(total_len as u16).to_le_bytes()); // length
    mpc.push(MP_SPEC);
    mpc.push(0); // checksum, filled below
    mpc.extend_from_slice(b"NOPEEK  "); // oem[8]
    mpc.extend_from_slice(b"MICROVM     "); // productid[12]
    mpc.extend_from_slice(&0u32.to_le_bytes()); // oemptr
    mpc.extend_from_slice(&0u16.to_le_bytes()); // oemsize
    mpc.extend_from_slice(&(ncpu as u16).to_le_bytes()); // oemcount (entry count)
    mpc.extend_from_slice(&LAPIC_PHYS.to_le_bytes()); // lapic
    mpc.extend_from_slice(&0u32.to_le_bytes()); // reserved
    debug_assert_eq!(mpc.len(), MPC_HEADER_LEN);
    for id in 0..ncpu {
        push_cpu(&mut mpc, id, id == 0);
    }
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
