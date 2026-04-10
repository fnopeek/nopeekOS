//! Minimal ACPI parser for power management
//!
//! Finds RSDP → RSDT/XSDT → FADT → PM1a_CNT_BLK port for S5 power-off.

use core::sync::atomic::{AtomicU16, Ordering};

/// Cached PM1a control block I/O port (0 = not found yet)
static PM1A_CNT_PORT: AtomicU16 = AtomicU16::new(0);
/// Cached SLP_TYPa value for S5 state (default 5, works on most Intel)
static SLP_TYP_S5: AtomicU16 = AtomicU16::new(5);
/// Cached ACPI reset register info (address_space, address, value)
static RESET_PORT: AtomicU16 = AtomicU16::new(0);
static RESET_VAL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Cached RSDP address from Multiboot2 (set before init)
static RSDP_ADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Parse Multiboot2 tags for ACPI RSDP (call early, before init).
pub fn parse_multiboot2_rsdp(mb_info_addr: u32) {
    let base = mb_info_addr as usize;
    let total_size = unsafe { *(base as *const u32) } as usize;

    let mut offset = 8;
    while offset + 8 <= total_size {
        let tag_type = unsafe { *((base + offset) as *const u32) };
        let tag_size = unsafe { *((base + offset + 4) as *const u32) } as usize;
        if tag_size == 0 { break; }

        // Type 15 = ACPI new RSDP (XSDP, 36 bytes), Type 14 = ACPI old RSDP (20 bytes)
        if tag_type == 15 || tag_type == 14 {
            let rsdp_addr = base + offset + 8; // RSDP data starts after tag header
            RSDP_ADDR.store(rsdp_addr, core::sync::atomic::Ordering::Release);
            break;
        }

        if tag_type == 0 { break; }
        offset += (tag_size + 7) & !7;
    }
}

/// Initialize ACPI: find FADT and cache PM1a_CNT_BLK port.
pub fn init() {
    // Disable on bare metal for now if RSDP not found via Multiboot2
    let mb_rsdp = RSDP_ADDR.load(core::sync::atomic::Ordering::Acquire);
    if mb_rsdp == 0 {
        crate::kprintln!("[npk] ACPI: no RSDP in Multiboot2 tags");
        return;
    }

    if let Some(port) = find_pm1a_cnt() {
        PM1A_CNT_PORT.store(port, Ordering::Release);
        crate::kprintln!("[npk] ACPI: PM1a_CNT at {:#x}", port);
    } else {
        crate::kprintln!("[npk] ACPI: PM1a_CNT not found");
    }
}

/// Perform ACPI reset via FADT reset register.
pub fn reset() {
    let port = RESET_PORT.load(Ordering::Acquire);
    let val = RESET_VAL.load(Ordering::Acquire);
    if port == 0 { return; }
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val);
    }
}

/// Perform ACPI S5 power-off.
pub fn power_off() {
    let port = PM1A_CNT_PORT.load(Ordering::Acquire);
    if port == 0 { return; }

    let slp_typ = SLP_TYP_S5.load(Ordering::Acquire);
    let val: u16 = (slp_typ << 10) | (1 << 13); // SLP_TYPa | SLP_EN

    unsafe {
        core::arch::asm!("out dx, ax", in("dx") port, in("ax") val);
    }
}

/// Find an ACPI table by 4-byte signature (e.g., b"APIC" for MADT).
/// Returns the physical address of the table header.
pub fn find_table(sig: &[u8; 4]) -> Option<usize> {
    let rsdp = find_rsdp()?;
    let revision = unsafe { *rsdp.add(15) };

    if revision >= 2 {
        let xsdt_addr = unsafe { *(rsdp.add(24) as *const u64) } as usize;
        ensure_mapped(xsdt_addr, 4096);
        find_table_in_xsdt(xsdt_addr, sig)
    } else {
        let rsdt_addr = unsafe { *(rsdp.add(16) as *const u32) } as usize;
        ensure_mapped(rsdt_addr, 4096);
        find_table_in_rsdt(rsdt_addr, sig)
    }
}

/// Public wrapper: ensure a physical address range is identity-mapped.
pub fn ensure_mapped_pub(addr: usize, size: usize) {
    ensure_mapped(addr, size);
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
    let rsdp = find_rsdp()?;

    // RSDP: signature at 0, revision at 15, RSDT at 16, XSDT at 24 (if revision >= 2)
    let revision = unsafe { *rsdp.add(15) };

    let fadt_addr = if revision >= 2 {
        let xsdt_addr = unsafe { *(rsdp.add(24) as *const u64) } as usize;
        ensure_mapped(xsdt_addr, 4096);
        find_table_in_xsdt(xsdt_addr, b"FACP")?
    } else {
        let rsdt_addr = unsafe { *(rsdp.add(16) as *const u32) } as usize;
        ensure_mapped(rsdt_addr, 4096);
        find_table_in_rsdt(rsdt_addr, b"FACP")?
    };

    // Map FADT and read PM1a_CNT_BLK at offset 64
    ensure_mapped(fadt_addr, 256);
    let pm1a = unsafe { *((fadt_addr + 64) as *const u32) };
    if pm1a == 0 || pm1a > 0xFFFF { return None; }

    // FADT offset 116: RESET_REG (Generic Address Structure)
    // GAS: address_space(1) + bit_width(1) + bit_offset(1) + access_size(1) + address(8)
    // FADT offset 128: RESET_VALUE (1 byte)
    let fadt_len = unsafe { *((fadt_addr + 4) as *const u32) } as usize;
    if fadt_len >= 129 {
        let reset_space = unsafe { *((fadt_addr + 116) as *const u8) };
        let reset_addr = unsafe { *((fadt_addr + 120) as *const u64) };
        let reset_val = unsafe { *((fadt_addr + 128) as *const u8) };
        // address_space 1 = System I/O
        if reset_space == 1 && reset_addr > 0 && reset_addr <= 0xFFFF {
            RESET_PORT.store(reset_addr as u16, Ordering::Release);
            RESET_VAL.store(reset_val, Ordering::Release);
            crate::kprintln!("[npk] ACPI: reset register at {:#x} val={:#x}", reset_addr, reset_val);
        }
    }

    // Try to read SLP_TYPa from DSDT \_S5 object
    let dsdt_addr = unsafe { *((fadt_addr + 40) as *const u32) } as usize;
    if dsdt_addr != 0 {
        ensure_mapped(dsdt_addr, 64);
        let dsdt_len = unsafe { *((dsdt_addr + 4) as *const u32) } as usize;
        if dsdt_len > 36 && dsdt_len < 0x100000 {
            ensure_mapped(dsdt_addr, dsdt_len);
            if let Some(slp_typ) = find_s5_slp_typ(dsdt_addr, dsdt_len) {
                SLP_TYP_S5.store(slp_typ, Ordering::Release);
                crate::kprintln!("[npk] ACPI: SLP_TYPa for S5 = {}", slp_typ);
            }
        }
    }

    Some(pm1a as u16)
}

/// Parse DSDT AML to find \_S5 object and extract SLP_TYPa value.
fn find_s5_slp_typ(dsdt_addr: usize, dsdt_len: usize) -> Option<u16> {
    let data = unsafe { core::slice::from_raw_parts(dsdt_addr as *const u8, dsdt_len) };

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

/// Find RSDP: first from Multiboot2 tag, then scan legacy BIOS regions.
fn find_rsdp() -> Option<*const u8> {
    // Prefer Multiboot2-provided RSDP (works on UEFI systems)
    let mb_rsdp = RSDP_ADDR.load(core::sync::atomic::Ordering::Acquire);
    if mb_rsdp != 0 {
        let p = mb_rsdp as *const u8;
        let sig_match = unsafe { *(p as *const u64) == *(b"RSD PTR ".as_ptr() as *const u64) };
        if sig_match {
            return Some(p);
        }
    }

    // Fallback: scan legacy BIOS regions
    let ebda_seg = unsafe { *(0x40E as *const u16) } as usize;
    let ebda_base = ebda_seg << 4;
    if ebda_base > 0 && ebda_base < 0x100000 {
        if let Some(p) = scan_rsdp(ebda_base, ebda_base + 1024) {
            return Some(p);
        }
    }
    scan_rsdp(0xE0000, 0x100000)
}

fn scan_rsdp(start: usize, end: usize) -> Option<*const u8> {
    let sig = b"RSD PTR ";
    let mut addr = start;
    while addr + 20 <= end {
        let p = addr as *const u8;
        let matches = unsafe {
            *(p as *const u64) == *(sig.as_ptr() as *const u64)
        };
        if matches {
            // Verify checksum (first 20 bytes sum to 0)
            let mut sum: u8 = 0;
            for i in 0..20 {
                sum = sum.wrapping_add(unsafe { *p.add(i) });
            }
            if sum == 0 {
                return Some(p);
            }
        }
        addr += 16; // RSDP is always 16-byte aligned
    }
    None
}

fn find_table_in_rsdt(rsdt_addr: usize, sig: &[u8; 4]) -> Option<usize> {
    let length = unsafe { *((rsdt_addr + 4) as *const u32) } as usize;
    if length < 36 || length > 0x10000 { return None; }
    let entries = (length - 36) / 4;

    for i in 0..entries {
        let entry_addr = unsafe { *((rsdt_addr + 36 + i * 4) as *const u32) } as usize;
        if entry_addr == 0 { continue; }
        ensure_mapped(entry_addr, 8);
        let table_sig = unsafe { core::slice::from_raw_parts(entry_addr as *const u8, 4) };
        if table_sig == sig {
            return Some(entry_addr);
        }
    }
    None
}

fn find_table_in_xsdt(xsdt_addr: usize, sig: &[u8; 4]) -> Option<usize> {
    let length = unsafe { *((xsdt_addr + 4) as *const u32) } as usize;
    if length < 36 || length > 0x10000 { return None; }
    let entries = (length - 36) / 8;

    for i in 0..entries {
        let entry_addr = unsafe { *((xsdt_addr + 36 + i * 8) as *const u64) } as usize;
        if entry_addr == 0 { continue; }
        ensure_mapped(entry_addr, 8);
        let table_sig = unsafe { core::slice::from_raw_parts(entry_addr as *const u8, 4) };
        if table_sig == sig {
            return Some(entry_addr);
        }
    }
    None
}
