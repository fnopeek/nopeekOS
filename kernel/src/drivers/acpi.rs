//! Minimal ACPI parser for power management
//!
//! Finds RSDP → RSDT/XSDT → FADT → PM1a_CNT_BLK port for S5 power-off.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};
use crate::hw::{PhysView, Port};

/// PM1a control block, from the FADT.
static PM1A_CNT: spin::Once<Port> = spin::Once::new();
/// Cached SLP_TYPa value for S5 state (default 5, works on most Intel)
static SLP_TYP_S5: AtomicU16 = AtomicU16::new(5);
/// FADT reset register (System I/O only) and the value to write.
static RESET: spin::Once<(Port, u8)> = spin::Once::new();

/// Cached RSDP physical address (set by UEFI stub via `set_rsdp`).
static RSDP_ADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Largest RSDT/XSDT accepted.
const MAX_ROOT: usize = 0x10000;
/// Largest table accepted.
const MAX_TABLE: usize = 0x40_0000;

/// A mapped ACPI table. Reads are bounded by the table's own length: a read
/// past it returns `None`.
#[derive(Clone, Copy)]
pub struct AcpiTable(PhysView);

impl AcpiTable {
    /// The table at `addr`, mapped whole, if its length is in `36..=max`.
    ///
    /// # Safety
    /// `addr` must be where the firmware placed an ACPI table: the RSDP's
    /// root pointer, an RSDT/XSDT entry or a FADT DSDT pointer.
    unsafe fn at(addr: usize, max: usize) -> Option<Self> {
        // SAFETY: the caller's contract.
        let len = unsafe { header(addr) }.u32(4u64)? as usize;
        if !(36..=max).contains(&len) { return None; }
        ensure_mapped(addr, len);
        // SAFETY: mapped just above; firmware tables neither move nor change.
        Some(AcpiTable(unsafe { PhysView::new(addr as u64, len as u64) }))
    }

    pub fn addr(&self) -> usize { self.0.addr() as usize }
    pub fn len(&self) -> usize { self.0.len() as usize }
    pub fn u8(&self, off: usize) -> Option<u8> { self.0.u8(off) }
    pub fn u16(&self, off: usize) -> Option<u16> { self.0.u16(off) }
    pub fn u32(&self, off: usize) -> Option<u32> { self.0.u32(off) }
    pub fn u64(&self, off: usize) -> Option<u64> { self.0.u64(off) }
    pub fn bytes(&self) -> &'static [u8] { self.0.bytes() }

    /// The I/O port number in the 32-bit field at `off` (a FADT block
    /// address), or `None` if it is outside the table, zero or above 0xFFFF.
    pub fn io_port(&self, off: usize) -> Option<u16> {
        let v = self.u32(off)?;
        (v != 0 && v <= 0xFFFF).then_some(v as u16)
    }
}

/// Signature and length of the table at `addr`, mapped.
///
/// # Safety
/// As for `AcpiTable::at`.
unsafe fn header(addr: usize) -> PhysView {
    ensure_mapped(addr, 8);
    // SAFETY: mapped just above; the caller's contract.
    unsafe { PhysView::new(addr as u64, 8) }
}

/// Stash the ACPI RSDP address handed to us by the UEFI stub. Called
/// from `kernel_main` before `init`.
pub fn set_rsdp(rsdp_phys: u64) {
    RSDP_ADDR.store(rsdp_phys as usize, core::sync::atomic::Ordering::Release);
}

/// Initialize ACPI: find FADT and cache PM1a_CNT_BLK port.
pub fn init() {
    let mb_rsdp = RSDP_ADDR.load(core::sync::atomic::Ordering::Acquire);
    if mb_rsdp == 0 {
        crate::kprintln!("[npk] ACPI: no RSDP from firmware");
        return;
    }

    if let Some(port) = find_pm1a_cnt() {
        // SAFETY: PM1a_CNT_BLK, the FADT's control port of the ACPI hardware
        // this module drives.
        PM1A_CNT.call_once(|| unsafe { Port::new(port) });
        crate::kdebug!("[npk] ACPI: PM1a_CNT at {:#x}", port);
    } else {
        crate::kprintln!("[npk] ACPI: PM1a_CNT not found");
    }
}

/// Take the platform out of legacy (SMM) mode into ACPI mode — ACPICA
/// `acpi_enable` → `acpi_hw_set_mode(ACPI_SYS_MODE_ACPI)`: if PM1_CNT.SCI_EN
/// is clear, write FADT.ACPI_ENABLE to FADT.SMI_CMD, then poll SCI_EN for up
/// to 3 s (30000 × 100 us). Called by `sci::arm_ec`.
pub fn enable_acpi_mode() {
    let Some(&pm1a_cnt) = PM1A_CNT.get() else { return };
    let Some(fadt) = table(b"FACP") else { return };
    // SMI_CMD at 48 (u32), ACPI_ENABLE at 52 (u8).
    let smi_cmd = fadt.u32(48).unwrap_or(0);
    let acpi_enable = fadt.u8(52).unwrap_or(0);
    let sci_en = || -> bool { (pm1a_cnt.inw() & 1) != 0 };
    if sci_en() {
        crate::kprintln!("[npk] ACPI: already in ACPI mode");
        return;
    }
    // ACPICA: no SMI_CMD, or no enable value, means the platform has no
    // legacy mode to leave (hardware-reduced / ACPI-only).
    if smi_cmd == 0 || smi_cmd > 0xFFFF || acpi_enable == 0 {
        crate::kprintln!("[npk] ACPI: SCI_EN clear but no SMI_CMD/ACPI_ENABLE — left as is");
        return;
    }
    // SAFETY: SMI_CMD is the FADT's I/O port for exactly this command.
    unsafe { Port::new(smi_cmd as u16) }.outb(acpi_enable);
    let tsc_100us = (crate::interrupts::tsc_freq() / 10_000).max(1);
    for retry in 0..30_000u32 {
        if sci_en() {
            crate::kprintln!("[npk] ACPI: legacy -> ACPI mode ({:#x} to SMI_CMD {:#x}, {} us)",
                acpi_enable, smi_cmd, retry * 100);
            return;
        }
        let t = crate::interrupts::rdtsc();
        while crate::interrupts::rdtsc().wrapping_sub(t) < tsc_100us {
            core::hint::spin_loop();
        }
    }
    crate::kprintln!("[npk] ACPI: firmware did not set SCI_EN within 3 s — still legacy");
}

/// Perform ACPI reset via FADT reset register.
pub fn reset() {
    if let Some(&(port, val)) = RESET.get() {
        port.outb(val);
    }
}

/// Perform ACPI S5 power-off.
pub fn power_off() {
    let Some(port) = PM1A_CNT.get() else { return };

    let slp_typ = SLP_TYP_S5.load(Ordering::Acquire);
    let val: u16 = (slp_typ << 10) | (1 << 13); // SLP_TYPa | SLP_EN

    port.outw(val);
}

/// Find an ACPI table by 4-byte signature (e.g., b"APIC" for MADT).
/// Returns the physical address of the table header.
pub fn find_table(sig: &[u8; 4]) -> Option<usize> {
    let (root, stride) = root()?;
    entries(root, stride).find(|&addr| sig_at(addr, sig))
}

/// The first table with this signature, mapped whole.
pub fn table(sig: &[u8; 4]) -> Option<AcpiTable> {
    let addr = find_table(sig)?;
    // SAFETY: an RSDT/XSDT entry.
    unsafe { AcpiTable::at(addr, MAX_TABLE) }
}

/// Physical ranges the platform owns that no driver may map: PCI ECAM
/// (MCFG), the HPET, and IOMMU register blocks (DMAR, IVRS). Each as
/// `(base, length)`. Absent tables contribute nothing.
pub fn platform_mmio_ranges() -> Vec<(u64, u64)> {
    fn table(sig: &[u8; 4]) -> Option<&'static [u8]> {
        Some(table_nth(sig, 0)?.bytes())
    }
    fn u64_at(t: &[u8], off: usize) -> Option<u64> {
        Some(u64::from_le_bytes(t.get(off..off + 8)?.try_into().ok()?))
    }
    fn u16_at(t: &[u8], off: usize) -> Option<u16> {
        Some(u16::from_le_bytes(t.get(off..off + 2)?.try_into().ok()?))
    }

    let mut out = Vec::new();
    if let Some(t) = table(b"MCFG") {
        // Allocation entries of 16 bytes after the 44-byte header.
        for e in t.get(44..).unwrap_or(&[]).chunks_exact(16) {
            let base = u64::from_le_bytes(e[0..8].try_into().unwrap_or([0; 8]));
            let (start, end) = (e[10] as u64, e[11] as u64);
            if base != 0 && end >= start {
                out.push((base + (start << 20), (end - start + 1) << 20));
            }
        }
    }
    if let Some(base) = table(b"HPET").and_then(|t| u64_at(t, 44)) {
        if base != 0 { out.push((base & !0xFFF, 0x1000)); }
    }
    // Remapping structures start at 48 in both; each says its own length.
    for (sig, is_dmar) in [(b"DMAR", true), (b"IVRS", false)] {
        let Some(t) = table(sig) else { continue };
        let mut off = 48;
        while let Some(len) = u16_at(t, off + 2) {
            let len = len as usize;
            if len < 16 { break; }
            let kind = if is_dmar { u16_at(t, off).unwrap_or(u16::MAX) } else { t[off] as u16 };
            // DMAR type 0 = DRHD, 2^size pages; IVRS 0x10/0x11/0x40 = IVHD,
            // whose register block is at most 512 KiB.
            let span = if is_dmar && kind == 0 {
                Some(0x1000u64 << (t.get(off + 5).copied().unwrap_or(0) & 0xF))
            } else if !is_dmar && matches!(kind, 0x10 | 0x11 | 0x40) {
                Some(0x8_0000)
            } else {
                None
            };
            if let (Some(span), Some(base)) = (span, u64_at(t, off + 8)) {
                if base != 0 { out.push((base, span)); }
            }
            off += len;
        }
    }
    out
}

/// The `index`-th table with this signature, with its length.
///
/// There can be more than one SSDT. Linux loads the DSDT and every SSDT into
/// one namespace (`acpi_tb_load_namespace`); reading only the DSDT misses
/// names declared elsewhere (e.g. an operation region base used by the DSDT).
///
/// `find_table` always returns the first; this one can enumerate.
pub fn find_table_nth(sig: &[u8; 4], index: usize) -> Option<(usize, usize)> {
    let t = table_nth(sig, index)?;
    Some((t.addr(), t.len()))
}

fn table_nth(sig: &[u8; 4], index: usize) -> Option<AcpiTable> {
    let (root, stride) = root()?;
    let mut seen = 0usize;
    for addr in entries(root, stride) {
        if !sig_at(addr, sig) { continue; }
        if seen != index { seen += 1; continue; }
        // SAFETY: an RSDT/XSDT entry.
        return unsafe { AcpiTable::at(addr, MAX_TABLE) };
    }
    None
}

/// Public wrapper: ensure a physical address range is identity-mapped.
pub fn ensure_mapped_pub(addr: usize, size: usize) {
    ensure_mapped(addr, size);
}

/// Locate the DSDT (AML) via the FADT: DSDT pointer at offset 40 (32-bit)
/// or X_DSDT at 140 (64-bit). Returns (addr, len) with the region mapped.
pub fn dsdt() -> Option<(usize, usize)> {
    let fadt = table(b"FACP")?;
    let mut addr = fadt.u32(40)? as usize;
    if let Some(x) = fadt.u64(140) {
        if x != 0 { addr = x as usize; }
    }
    if addr == 0 { return None; }
    // SAFETY: the FADT's DSDT pointer.
    let t = unsafe { AcpiTable::at(addr, 0x200000) }?;
    Some((t.addr(), t.len()))
}

/// FADT "Preferred PM Profile" (offset 45): 2 = Mobile (laptop). Used to
/// gate the EC battery driver so desktops (profile 1) never probe for
/// a battery and show a phantom one.
pub fn is_mobile() -> bool {
    table(b"FACP").and_then(|f| f.u8(45)) == Some(2)
}

/// Ensure a memory region is identity-mapped so we can read ACPI tables.
fn ensure_mapped(addr: usize, size: usize) {
    let start = addr & !0xFFF;
    let end = (addr + size + 0xFFF) & !0xFFF;
    for page in (start..end).step_by(4096) {
        let _ = crate::paging::map_page(
            page as u64, page as u64,
            crate::paging::PageFlags::PRESENT,
        );
    }
}

fn find_pm1a_cnt() -> Option<u16> {
    let fadt = table(b"FACP")?;

    // PM1a_CNT_BLK at offset 64
    let pm1a = fadt.io_port(64)?;

    // FADT offset 116: RESET_REG (Generic Address Structure)
    // GAS: address_space(1) + bit_width(1) + bit_offset(1) + access_size(1) + address(8)
    // FADT offset 128: RESET_VALUE (1 byte)
    if let (Some(reset_space), Some(reset_addr), Some(reset_val)) =
        (fadt.u8(116), fadt.u64(120), fadt.u8(128))
    {
        // address_space 1 = System I/O
        if reset_space == 1 && reset_addr > 0 && reset_addr <= 0xFFFF {
            // SAFETY: the FADT's reset register, in System I/O space.
            RESET.call_once(|| (unsafe { Port::new(reset_addr as u16) }, reset_val));
            crate::kprintln!("[npk] ACPI: reset register at {:#x} val={:#x}", reset_addr, reset_val);
        }
    }

    // Try to read SLP_TYPa from DSDT \_S5 object
    let dsdt_addr = fadt.u32(40).unwrap_or(0) as usize;
    if dsdt_addr != 0 {
        // SAFETY: the FADT's DSDT pointer.
        if let Some(dsdt) = unsafe { AcpiTable::at(dsdt_addr, 0xFFFFF) } {
            if dsdt.len() > 36 {
                if let Some(slp_typ) = find_s5_slp_typ(dsdt.bytes()) {
                    SLP_TYP_S5.store(slp_typ, Ordering::Release);
                    crate::kdebug!("[npk] ACPI: SLP_TYPa for S5 = {}", slp_typ);
                }
            }
        }
    }

    Some(pm1a)
}

/// Parse DSDT AML to find \_S5 object and extract SLP_TYPa value.
fn find_s5_slp_typ(data: &[u8]) -> Option<u16> {
    // Scan for "_S5_" (0x5F 0x53 0x35 0x5F)
    for i in 0..data.len().saturating_sub(20) {
        if &data[i..i + 4] == b"_S5_" {
            // Expected AML: NameOp(0x08) before _S5_, PackageOp(0x12) after
            if i == 0 || data[i - 1] != 0x08 { continue; }
            let rest = &data[i + 4..];
            if rest.is_empty() || rest[0] != 0x12 { continue; }

            // Skip PackageOp + PkgLength
            let mut pos = 1; // past PackageOp
            // PkgLength encoding: if top 2 bits of first byte are 0, length is 1 byte
            if pos >= rest.len() { continue; }
            let pkg_lead = rest[pos];
            if pkg_lead & 0xC0 == 0 {
                pos += 1; // 1-byte PkgLength
            } else {
                let extra = ((pkg_lead >> 6) & 3) as usize;
                pos += 1 + extra;
            }

            // Skip NumElements byte
            if pos >= rest.len() { continue; }
            pos += 1;

            // Read SLP_TYPa: may be BytePrefix(0x0A) + byte, or raw byte, or WordPrefix, etc.
            if pos >= rest.len() { continue; }
            let slp_typ = if rest[pos] == 0x0A && pos + 1 < rest.len() {
                rest[pos + 1] as u16 // BytePrefix
            } else if rest[pos] == 0x0B && pos + 2 < rest.len() {
                u16::from_le_bytes([rest[pos + 1], rest[pos + 2]]) // WordPrefix
            } else if rest[pos] <= 0x0F {
                rest[pos] as u16 // Small integer (0-15 encoded directly in some AML variants)
            } else {
                continue;
            };

            return Some(slp_typ);
        }
    }
    None
}

/// The root table (XSDT for revision 2+, else RSDT) and its entry width.
fn root() -> Option<(AcpiTable, usize)> {
    let rsdp = find_rsdp()?;
    // RSDP: signature at 0, revision at 15, RSDT at 16, XSDT at 24 (if revision >= 2)
    let (addr, stride) = if rsdp.u8(15u64)? >= 2 {
        (rsdp.u64(24u64)? as usize, 8)
    } else {
        (rsdp.u32(16u64)? as usize, 4)
    };
    // SAFETY: the RSDP's root pointer.
    Some((unsafe { AcpiTable::at(addr, MAX_ROOT) }?, stride))
}

/// The non-zero table addresses in the root table.
fn entries(root: AcpiTable, stride: usize) -> impl Iterator<Item = usize> {
    (0..(root.len() - 36) / stride)
        .map(move |i| {
            let off = 36 + i * stride;
            if stride == 8 {
                root.u64(off).unwrap_or(0) as usize
            } else {
                root.u32(off).unwrap_or(0) as usize
            }
        })
        .filter(|&addr| addr != 0)
}

/// Whether the table at root entry `addr` has signature `sig`.
fn sig_at(addr: usize, sig: &[u8; 4]) -> bool {
    // SAFETY: an RSDT/XSDT entry.
    unsafe { header(addr) }.bytes()[..4] == sig[..]
}

/// The RSDP at `addr`: 20 bytes, 36 from revision 2 on.
///
/// # Safety
/// `addr` must hold an RSDP signature in firmware memory that the boot page
/// tables map.
unsafe fn rsdp_at(addr: usize) -> PhysView {
    // SAFETY: the caller's contract; every revision has 20 bytes.
    let v1 = unsafe { PhysView::new(addr as u64, 20) };
    if v1.u8(15u64).unwrap_or(0) >= 2 {
        // SAFETY: as above; revision 2 is 36 bytes long.
        unsafe { PhysView::new(addr as u64, 36) }
    } else {
        v1
    }
}

/// Find RSDP: first from Multiboot2 tag, then scan legacy BIOS regions.
fn find_rsdp() -> Option<PhysView> {
    // Prefer Multiboot2-provided RSDP (works on UEFI systems)
    let mb_rsdp = RSDP_ADDR.load(core::sync::atomic::Ordering::Acquire);
    if mb_rsdp != 0 {
        // SAFETY: the RSDP address the firmware handed over, in memory the
        // boot page tables map.
        let sig = unsafe { PhysView::new(mb_rsdp as u64, 8) };
        if sig.bytes() == b"RSD PTR " {
            // SAFETY: signature checked just above.
            return Some(unsafe { rsdp_at(mb_rsdp) });
        }
    }

    // Fallback: scan legacy BIOS regions
    // SAFETY: the BIOS data area word holding the EBDA segment; low memory
    // is mapped at boot.
    let ebda_seg = unsafe { PhysView::new(0x40E, 2) }.u16(0u64).unwrap_or(0) as usize;
    let ebda_base = ebda_seg << 4;
    if ebda_base > 0 && ebda_base < 0x100000 {
        if let Some(p) = scan_rsdp(ebda_base, ebda_base + 1024) {
            return Some(p);
        }
    }
    scan_rsdp(0xE0000, 0x100000)
}

fn scan_rsdp(start: usize, end: usize) -> Option<PhysView> {
    // SAFETY: legacy BIOS memory (EBDA or 0xE0000..0x100000), mapped at boot.
    let area = unsafe { PhysView::new(start as u64, (end - start) as u64) }.bytes();
    let mut off = 0;
    while off + 20 <= area.len() {
        if &area[off..off + 8] == b"RSD PTR " {
            // Verify checksum (first 20 bytes sum to 0)
            let sum = area[off..off + 20].iter().fold(0u8, |s, &b| s.wrapping_add(b));
            if sum == 0 {
                // SAFETY: signature found inside the BIOS area above.
                return Some(unsafe { rsdp_at(start + off) });
            }
        }
        off += 16; // RSDP is always 16-byte aligned
    }
    None
}
