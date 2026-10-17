//! AMD-Vi (the AMD IOMMU), stage 0 of docs/plan/AMD_VI.md: the IVRS table
//! and the IOMMU's registers read the way Linux 6.18.55
//! drivers/iommu/amd/init.c reads them, and printed in the shape of its
//! `amd_iommu_dump` lines. Nothing is written: no device table, no command
//! buffer, translation stays as the firmware left it.

use alloc::string::String;
use alloc::vec::Vec;
use crate::hw::Mmio;
use crate::kprintln;

/// init.c:43
const IVRS_HEADER_LENGTH: usize = 48;
/// init.c:45-48
const ACPI_IVHD_TYPE_MAX_SUPPORTED: u8 = 0x40;
const ACPI_IVMD_TYPE_ALL: u8 = 0x20;
const ACPI_IVMD_TYPE: u8 = 0x21;
const ACPI_IVMD_TYPE_RANGE: u8 = 0x22;
/// init.c:50-59
const IVHD_DEV_ALL: u8 = 0x01;
const IVHD_DEV_SELECT: u8 = 0x02;
const IVHD_DEV_SELECT_RANGE_START: u8 = 0x03;
const IVHD_DEV_RANGE_END: u8 = 0x04;
const IVHD_DEV_ALIAS: u8 = 0x42;
const IVHD_DEV_ALIAS_RANGE: u8 = 0x43;
const IVHD_DEV_EXT_SELECT: u8 = 0x46;
const IVHD_DEV_EXT_SELECT_RANGE: u8 = 0x47;
const IVHD_DEV_SPECIAL: u8 = 0x48;
const IVHD_DEV_ACPI_HID: u8 = 0xf0;
/// init.c:65-66
const IVHD_SPECIAL_IOAPIC: u8 = 1;
const IVHD_SPECIAL_HPET: u8 = 2;
/// init.c:73-76
const IVMD_FLAG_EXCL_RANGE: u8 = 0x08;
const IVMD_FLAG_IW: u8 = 0x04;
const IVMD_FLAG_IR: u8 = 0x02;
const IVMD_FLAG_UNITY_MAP: u8 = 0x01;
/// amd_iommu_types.h:59-84
const MMIO_CONTROL_OFFSET: u32 = 0x0018;
const MMIO_EXT_FEATURES: u32 = 0x0030;
const MMIO_EXT_FEATURES2: u32 = 0x01a0;
const MMIO_STATUS_OFFSET: u32 = 0x2020;
const MMIO_CNTR_CONF_OFFSET: u64 = 0x4000;
const MMIO_REG_END_OFFSET: u64 = 0x80000;
/// amd_iommu_types.h:163
const CONTROL_IOMMU_EN: u32 = 0;
/// amd_iommu_types.h:462-464
const IOMMU_IVINFO_OFFSET: usize = 36;
const IOMMU_IVINFO_EFRSUP: u32 = 1 << 0;
const IOMMU_IVINFO_DMA_REMAP: u32 = 1 << 1;
/// amd_iommu_types.h:470
const IOMMU_IVHD_ATTR_HATDIS_SHIFT: u32 = 0;
/// amd_iommu_types.h:98-107: HATS, GAM_VAPIC, SNP.
const FEATURE_HATS_SHIFT: u32 = 10;
const FEATURE_HATS_MASK: u64 = 0b11 << FEATURE_HATS_SHIFT;
const FEATURE_GAM_VAPIC: u64 = 1 << 21;
const FEATURE_SNP: u64 = 1 << 63;

// `struct ivhd_header` (init.c:101), packed.
const IVHD_TYPE: usize = 0;
const IVHD_FLAGS: usize = 1;
const IVHD_LENGTH: usize = 2;
const IVHD_DEVID: usize = 4;
const IVHD_CAP_PTR: usize = 6;
const IVHD_MMIO_PHYS: usize = 8;
const IVHD_PCI_SEG: usize = 16;
const IVHD_INFO: usize = 18;
const IVHD_EFR_ATTR: usize = 20;
const IVHD_EFR_REG: usize = 24;
const IVHD_EFR_REG2: usize = 32;
// `struct ivhd_entry` (init.c:121), packed: type, devid, flags, ext.
const IVHD_ENTRY_DEVID: usize = 1;
const IVHD_ENTRY_FLAGS: usize = 3;
const IVHD_ENTRY_EXT: usize = 4;
// `struct ivmd_header` (init.c:142), packed.
const IVMD_FLAGS: usize = 1;
const IVMD_LENGTH: usize = 2;
const IVMD_DEVID: usize = 4;
const IVMD_AUX: usize = 6;
const IVMD_PCI_SEG: usize = 8;
const IVMD_RANGE_START: usize = 16;
const IVMD_RANGE_LENGTH: usize = 24;

/// Register windows mapped so far, one per IOMMU, kept for the boot.
static MAPPED: spin::Mutex<Vec<(u64, Mmio)>> = spin::Mutex::new(Vec::new());

fn u8_at(t: &[u8], o: usize) -> u8 {
    t.get(o).copied().unwrap_or(0)
}
fn u16_at(t: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([u8_at(t, o), u8_at(t, o + 1)])
}
fn u32_at(t: &[u8], o: usize) -> u32 {
    u16_at(t, o) as u32 | (u16_at(t, o + 2) as u32) << 16
}
fn u64_at(t: &[u8], o: usize) -> u64 {
    u32_at(t, o) as u64 | (u32_at(t, o + 4) as u64) << 32
}

/// `PCI_BUS_NUM` / `PCI_SLOT` / `PCI_FUNC` of a 16-bit device id.
fn bdf(devid: u32) -> String {
    alloc::format!("{:02x}:{:02x}.{:x}", (devid >> 8) & 0xff, (devid >> 3) & 0x1f, devid & 7)
}

/// `check_ivrs_checksum` (init.c:585).
fn check_ivrs_checksum(t: &[u8]) -> bool {
    t.iter().fold(0u8, |c, &b| c.wrapping_add(b)) == 0
}

/// `get_highest_supported_ivhd_type` (init.c:2011).
fn get_highest_supported_ivhd_type(t: &[u8]) -> u8 {
    let mut p = IVRS_HEADER_LENGTH;
    let mut last_type = u8_at(t, p + IVHD_TYPE);
    let devid = u16_at(t, p + IVHD_DEVID);
    while p < t.len() && u8_at(t, p + IVHD_TYPE) <= ACPI_IVHD_TYPE_MAX_SUPPORTED {
        if u16_at(t, p + IVHD_DEVID) == devid {
            last_type = u8_at(t, p + IVHD_TYPE);
        }
        let len = u16_at(t, p + IVHD_LENGTH) as usize;
        if len == 0 {
            break;
        }
        p += len;
    }
    last_type
}

/// `get_ivhd_header_size` (init.c:499).
fn get_ivhd_header_size(ty: u8) -> usize {
    match ty {
        0x10 => 24,
        0x11 | 0x40 => 40,
        _ => 0,
    }
}

/// `ivhd_entry_length` (init.c:527).
fn ivhd_entry_length(t: &[u8], p: usize) -> usize {
    let ty = u8_at(t, p);
    if ty < 0x80 {
        0x04 << (ty >> 6)
    } else if ty == IVHD_DEV_ACPI_HID {
        u8_at(t, p + 21) as usize + 22
    } else {
        0
    }
}

/// `find_last_devid_from_ivhd` (init.c:544).
fn find_last_devid_from_ivhd(t: &[u8], h: usize) -> i32 {
    let size = get_ivhd_header_size(u8_at(t, h + IVHD_TYPE));
    if size == 0 {
        return -22;
    }
    let (mut p, end) = (h + size, h + u16_at(t, h + IVHD_LENGTH) as usize);
    let mut last_devid = -22;
    while p < end {
        match u8_at(t, p) {
            IVHD_DEV_ALL => return 0xffff,
            IVHD_DEV_SELECT | IVHD_DEV_RANGE_END | IVHD_DEV_ALIAS | IVHD_DEV_EXT_SELECT => {
                last_devid = last_devid.max(u16_at(t, p + IVHD_ENTRY_DEVID) as i32);
            }
            _ => {}
        }
        let n = ivhd_entry_length(t, p);
        if n == 0 {
            break;
        }
        p += n;
    }
    last_devid
}

/// The device entries of one IVHD as `init_iommu_from_acpi` (init.c:1461)
/// walks them, its DUMP_printk lines, and the aliases it records.
fn ivhd_entries(t: &[u8], h: usize, seg: u16, last_bdf: u32, aliases: &mut Vec<(u32, u32)>) {
    let ty = u8_at(t, h + IVHD_TYPE);
    let size = get_ivhd_header_size(ty);
    if size == 0 {
        kprintln!("[amd-vi] Unsupported IVHD type {:#x}", ty);
        return;
    }
    let (mut p, end) = (h + size, h + u16_at(t, h + IVHD_LENGTH) as usize);
    let (mut devid_start, mut devid_to, mut alias) = (0u32, 0u32, false);
    while p < end {
        let e_ty = u8_at(t, p);
        let devid = u16_at(t, p + IVHD_ENTRY_DEVID) as u32;
        let flags = u8_at(t, p + IVHD_ENTRY_FLAGS);
        let ext = u32_at(t, p + IVHD_ENTRY_EXT);
        match e_ty {
            IVHD_DEV_ALL => kprintln!("[amd-vi]   DEV_ALL setting: {:#04x} (0000 .. {:04x})", flags, last_bdf),
            IVHD_DEV_SELECT => kprintln!("[amd-vi]   DEV_SELECT devid: {:04x}:{} flags: {:#04x}", seg, bdf(devid), flags),
            IVHD_DEV_SELECT_RANGE_START => {
                kprintln!("[amd-vi]   DEV_SELECT_RANGE_START devid: {:04x}:{} flags: {:#04x}", seg, bdf(devid), flags);
                devid_start = devid;
                alias = false;
            }
            IVHD_DEV_ALIAS => {
                kprintln!("[amd-vi]   DEV_ALIAS devid: {:04x}:{} flags: {:#04x} devid_to: {}",
                    seg, bdf(devid), flags, bdf(ext >> 8));
                aliases.push((devid, (ext >> 8) & 0xffff));
            }
            IVHD_DEV_ALIAS_RANGE => {
                kprintln!("[amd-vi]   DEV_ALIAS_RANGE devid: {:04x}:{} flags: {:#04x} devid_to: {:04x}:{}",
                    seg, bdf(devid), flags, seg, bdf(ext >> 8));
                devid_start = devid;
                devid_to = (ext >> 8) & 0xffff;
                alias = true;
            }
            IVHD_DEV_EXT_SELECT => kprintln!("[amd-vi]   DEV_EXT_SELECT devid: {:04x}:{} flags: {:#04x} ext: {:08x}",
                seg, bdf(devid), flags, ext),
            IVHD_DEV_EXT_SELECT_RANGE => {
                kprintln!("[amd-vi]   DEV_EXT_SELECT_RANGE devid: {:04x}:{} flags: {:#04x} ext: {:08x}",
                    seg, bdf(devid), flags, ext);
                devid_start = devid;
                alias = false;
            }
            IVHD_DEV_RANGE_END => {
                kprintln!("[amd-vi]   DEV_RANGE_END devid: {:04x}:{}", seg, bdf(devid));
                if alias {
                    for d in devid_start..=devid {
                        aliases.push((d, devid_to));
                    }
                }
            }
            IVHD_DEV_SPECIAL => {
                let (handle, special, ty) = (ext & 0xff, (ext >> 8) & 0xffff, ((ext >> 24) & 0xff) as u8);
                let var = match ty {
                    IVHD_SPECIAL_IOAPIC => "IOAPIC",
                    IVHD_SPECIAL_HPET => "HPET",
                    _ => "UNKNOWN",
                };
                kprintln!("[amd-vi]   DEV_SPECIAL({}[{}]) devid: {:04x}:{}, flags: {:#04x}", var, handle, seg, bdf(special), flags);
            }
            IVHD_DEV_ACPI_HID => {
                if ty != 0x40 {
                    kprintln!("[amd-vi] [Firmware Bug]: Invalid IVHD device type {:#x}", e_ty);
                } else {
                    let hid: String = t.get(p + 4..p + 12).unwrap_or(&[]).iter()
                        .take_while(|&&c| c != 0).map(|&c| c as char).collect();
                    kprintln!("[amd-vi]   DEV_ACPI_HID({}) devid: {:04x}:{}, flags: {:#04x}", hid, seg, bdf(devid), flags);
                }
            }
            _ => {}
        }
        let n = ivhd_entry_length(t, p);
        if n == 0 {
            break;
        }
        p += n;
    }
}

/// `init_unity_map_range`'s line for one IVMD (init.c:2645), for the ones
/// `init_memory_definitions` (init.c:2708) takes.
fn ivmd(t: &[u8], m: usize, last_bdf: u32) {
    let (ty, flags) = (u8_at(t, m), u8_at(t, m + IVMD_FLAGS));
    if flags & (IVMD_FLAG_UNITY_MAP | IVMD_FLAG_EXCL_RANGE) == 0 {
        return;
    }
    let devid = u16_at(t, m + IVMD_DEVID) as u32;
    let (s, start, end) = match ty {
        ACPI_IVMD_TYPE => ("IVMD_TYPEi", devid, devid),
        ACPI_IVMD_TYPE_ALL => ("IVMD_TYPE_ALL", 0, last_bdf),
        ACPI_IVMD_TYPE_RANGE => ("IVMD_TYPE_RANGE", devid, u16_at(t, m + IVMD_AUX) as u32),
        _ => return,
    };
    let page_align = |a: u64| (a + 0xfff) & !0xfff;
    let address_start = page_align(u64_at(t, m + IVMD_RANGE_START));
    let address_end = address_start + page_align(u64_at(t, m + IVMD_RANGE_LENGTH));
    let prot = if flags & IVMD_FLAG_EXCL_RANGE != 0 { (IVMD_FLAG_IW | IVMD_FLAG_IR) >> 1 } else { flags >> 1 };
    let seg = u16_at(t, m + IVMD_PCI_SEG);
    kprintln!("[amd-vi] {} devid_start: {:04x}:{} devid_end: {:04x}:{} range_start: {:016x} range_end: {:016x} flags: {:x} (prot {:x})",
        s, seg, bdf(start), seg, bdf(end), address_start, address_end, flags, prot);
}

/// `print_iommu_info`'s feature line (init.c:2283).
fn features(efr: u64, efr2: u64) -> String {
    const FEAT_STR: [&str; 10] = ["PreF", "PPR", "X2APIC", "NX", "GT", "[5]", "IA", "GA", "HE", "PC"];
    let mut s = alloc::format!("Extended features ({:#x}, {:#x}):", efr, efr2);
    for (i, f) in FEAT_STR.iter().enumerate() {
        if efr & (1 << i) != 0 {
            s.push(' ');
            s.push_str(f);
        }
    }
    if efr & FEATURE_GAM_VAPIC != 0 {
        s.push_str(" GA_vAPIC");
    }
    if efr & FEATURE_SNP != 0 {
        s.push_str(" SNP");
    }
    s
}

/// Whether the firmware describes an AMD IOMMU.
pub fn present() -> bool {
    crate::drivers::acpi::find_table(b"IVRS").is_some()
}

/// Stage 0: everything `early_amd_iommu_init` learns before it writes the
/// first register, printed.
pub fn dump() {
    let Some(ivrs) = crate::drivers::acpi::table(b"IVRS") else {
        kprintln!("  AMD-Vi:   no IVRS table");
        return;
    };
    let t = ivrs.bytes();
    if !check_ivrs_checksum(t) {
        kprintln!("[amd-vi] [Firmware Bug]: IVRS invalid checksum");
        return;
    }
    let ivinfo = u32_at(t, IOMMU_IVINFO_OFFSET);
    let target = get_highest_supported_ivhd_type(t);
    kprintln!("  AMD-Vi:   IVRS {} bytes, IVinfo {:#x} (EFRSUP {}, DMA_REMAP {}), target IVHD type {:#x}",
        t.len(), ivinfo, ivinfo & IOMMU_IVINFO_EFRSUP != 0, ivinfo & IOMMU_IVINFO_DMA_REMAP != 0, target);

    // find_last_devid_acpi (init.c:606), segment by segment.
    let mut last_bdf = 0u32;
    let mut p = IVRS_HEADER_LENGTH;
    while p + IVHD_LENGTH + 2 <= t.len() {
        let len = u16_at(t, p + IVHD_LENGTH) as usize;
        if len == 0 {
            break;
        }
        if u8_at(t, p + IVHD_TYPE) == target {
            let d = find_last_devid_from_ivhd(t, p);
            if d > last_bdf as i32 {
                last_bdf = d as u32;
            }
        }
        p += len;
    }
    kprintln!("[amd-vi] last_bdf {:04x}", last_bdf);

    // init_iommu_all (init.c:2035) phase 1, without allocating anything.
    let mut aliases = Vec::new();
    let mut p = IVRS_HEADER_LENGTH;
    while p + IVHD_LENGTH + 2 <= t.len() {
        let ty = u8_at(t, p + IVHD_TYPE);
        let len = u16_at(t, p + IVHD_LENGTH) as usize;
        if len == 0 {
            break;
        }
        if ty == target {
            let (seg, devid) = (u16_at(t, p + IVHD_PCI_SEG), u16_at(t, p + IVHD_DEVID) as u32);
            kprintln!("[amd-vi] device: {:04x}:{} cap: {:04x} flags: {:x} info {:04x}",
                seg, bdf(devid), u16_at(t, p + IVHD_CAP_PTR), u8_at(t, p + IVHD_FLAGS), u16_at(t, p + IVHD_INFO));
            let mmio_phys = u64_at(t, p + IVHD_MMIO_PHYS);
            kprintln!("[amd-vi]        mmio-addr: {:016x}", mmio_phys);
            let efr_attr = u32_at(t, p + IVHD_EFR_ATTR);
            // init_iommu_one (init.c:1891): the register window's size and
            // the EFR the IVHD carries (early_iommu_features_init, init.c:305).
            let (end, ivhd_efr) = match ty {
                0x10 => {
                    let cntr = efr_attr != 0 && efr_attr & (0xf << 13) != 0 && efr_attr & (0x3f << 17) != 0;
                    (if cntr { MMIO_REG_END_OFFSET } else { MMIO_CNTR_CONF_OFFSET }, None)
                }
                _ => {
                    let efr_reg = u64_at(t, p + IVHD_EFR_REG);
                    if efr_attr & (1 << IOMMU_IVHD_ATTR_HATDIS_SHIFT) != 0 {
                        kprintln!("[amd-vi] Host Address Translation is not supported.");
                    }
                    (if efr_reg & (1 << 9) != 0 { MMIO_REG_END_OFFSET } else { MMIO_CNTR_CONF_OFFSET },
                     (ivinfo & IOMMU_IVINFO_EFRSUP != 0).then(|| (efr_reg, u64_at(t, p + IVHD_EFR_REG2))))
                }
            };
            ivhd_entries(t, p, seg, last_bdf, &mut aliases);
            mmio(mmio_phys, end, ivhd_efr);
        }
        p += len;
    }
    // An alias that folds the display controller with a neighbour would
    // give both one device table entry (docs/plan/AMD_VI.md §5).
    match crate::drivers::pci::find_by_class(0x03, 0x00) {
        Some(g) => {
            let gpu = (g.addr.bus as u32) << 8 | (g.addr.device as u32) << 3 | g.addr.function as u32;
            let hits: Vec<_> = aliases.iter().filter(|(d, to)| *d == gpu || *to == gpu).collect();
            kprintln!("[amd-vi] {} alias entries, {} involving the display controller {}", aliases.len(), hits.len(), bdf(gpu));
            for (d, to) in hits {
                kprintln!("[amd-vi]   alias {} -> {}", bdf(*d), bdf(*to));
            }
        }
        None => kprintln!("[amd-vi] {} alias entries; no display controller on the bus", aliases.len()),
    }

    // init_memory_definitions (init.c:2708).
    let mut p = IVRS_HEADER_LENGTH;
    while p + IVMD_LENGTH + 2 <= t.len() {
        let len = u16_at(t, p + IVMD_LENGTH) as usize;
        if len == 0 {
            break;
        }
        ivmd(t, p, last_bdf);
        p += len;
    }
}

/// The IOMMU's registers, read only: Control (translation must still be off),
/// Status, EFR and EFR2 from MMIO next to the IVHD's copy.
fn mmio(phys: u64, end: u64, ivhd_efr: Option<(u64, u64)>) {
    let regs = {
        let mut mapped = MAPPED.lock();
        match mapped.iter().find(|m| m.0 == phys) {
            Some(m) => m.1,
            None => {
                // SAFETY: the IVHD names this window as the IOMMU's registers
                // (reserved as platform MMIO by acpi::platform_mmio_ranges);
                // it is only read.
                match unsafe { Mmio::map(phys & !0xfff, end) } {
                    Ok(r) => {
                        mapped.push((phys, r));
                        r
                    }
                    Err(_) => {
                        kprintln!("[amd-vi] the register window at {:#x} could not be mapped", phys);
                        return;
                    }
                }
            }
        }
    };
    let control = regs.r64_lo_hi(MMIO_CONTROL_OFFSET);
    let status = regs.r64_lo_hi(MMIO_STATUS_OFFSET);
    let efr = regs.r64_lo_hi(MMIO_EXT_FEATURES);
    let efr2 = regs.r64_lo_hi(MMIO_EXT_FEATURES2);
    kprintln!("[amd-vi] control {:#018x} (IOMMU_EN {}), status {:#x}, window {:#x}",
        control, control >> CONTROL_IOMMU_EN & 1, status, end);
    kprintln!("[amd-vi] {}", features(efr, efr2));
    if let Some((a, b)) = ivhd_efr {
        kprintln!("[amd-vi] IVHD EFR {:#x}, EFR2 {:#x} ({})", a, b, if (a, b) == (efr, efr2) { "same as MMIO" } else { "differs from MMIO" });
    }
    let hats = (efr & FEATURE_HATS_MASK) >> FEATURE_HATS_SHIFT;
    kprintln!("[amd-vi] HATS {}: host page table up to {} levels (amd_iommu_hpt_level = HATS + 4)", hats, hats + 4);
}
