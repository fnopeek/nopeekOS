//! AMD-Vi (the AMD IOMMU), after Linux 6.18.55 drivers/iommu/amd.
//!
//! Stage 0 of docs/plan/AMD_VI.md (`dump`): the IVRS table and the IOMMU's
//! registers read the way init.c reads them, printed in the shape of its
//! `amd_iommu_dump` lines.
//!
//! Stage 1 (`enable`): the IOMMU switched on as `state_next` does it up to
//! the event buffer, with a device table whose entries are all `V = 0`.
//! A DTE without V is passed through untranslated, so no device's reach
//! changes; the entries carry only the IVHD's persistent flags.
//! `init_device_table_dma`, which blocks every device without a domain,
//! is stage 6. Events are polled; `EVT_INT_EN` stays off.
//!
//! Stage 2 (`protect`): one device in a protection domain with a v1 page
//! table, IOVA = phys, reaching its IVMD ranges and what `grant` adds.
//! Every other DTE stays `V = 0`. No ATS (`DTE_FLAG_IOTLB` stays clear):
//! a device with its own translations is what a module is not trusted
//! with.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use crate::drivers::pci::{self, PciAddr};
use crate::hw::{DmaRegion, Mmio};
use super::iommu::Fault;
use crate::kprintln;
use super::amd_vi_regs as r;

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
/// A `struct unity_map_entry`: memory the devices of the range reach 1:1.
#[derive(Clone, Copy)]
struct UnityMap {
    devid_start: u32,
    devid_end: u32,
    address_start: u64,
    address_end: u64,
    prot: u8,
}

/// `init_unity_map_range` (init.c:2645), for an entry whose flags name a
/// unity map or an exclusion range; with the name of its type.
fn unity_map_range(t: &[u8], m: usize, last_bdf: u32) -> Option<(UnityMap, &'static str)> {
    let (ty, flags) = (u8_at(t, m), u8_at(t, m + IVMD_FLAGS));
    let devid = u16_at(t, m + IVMD_DEVID) as u32;
    let (s, devid_start, devid_end) = match ty {
        ACPI_IVMD_TYPE => ("IVMD_TYPEi", devid, devid),
        ACPI_IVMD_TYPE_ALL => ("IVMD_TYPE_ALL", 0, last_bdf),
        ACPI_IVMD_TYPE_RANGE => ("IVMD_TYPE_RANGE", devid, u16_at(t, m + IVMD_AUX) as u32),
        _ => return None,
    };
    let page_align = |a: u64| (a + 0xfff) & !0xfff;
    let address_start = page_align(u64_at(t, m + IVMD_RANGE_START));
    let address_end = address_start + page_align(u64_at(t, m + IVMD_RANGE_LENGTH));
    // Treat per-device exclusion ranges as r/w unity-mapped regions.
    let prot = if flags & IVMD_FLAG_EXCL_RANGE != 0 { (IVMD_FLAG_IW | IVMD_FLAG_IR) >> 1 } else { flags >> 1 };
    Some((UnityMap { devid_start, devid_end, address_start, address_end, prot }, s))
}

/// `init_memory_definitions` (init.c:2708).
fn init_memory_definitions(t: &[u8], last_bdf: u32) -> Vec<UnityMap> {
    let mut maps = Vec::new();
    let mut p = IVRS_HEADER_LENGTH;
    while p + IVMD_LENGTH + 2 <= t.len() {
        let len = u16_at(t, p + IVMD_LENGTH) as usize;
        if len == 0 {
            break;
        }
        if u8_at(t, p + IVMD_FLAGS) & (IVMD_FLAG_UNITY_MAP | IVMD_FLAG_EXCL_RANGE) != 0 {
            if let Some((e, _)) = unity_map_range(t, p, last_bdf) {
                maps.push(e);
            }
        }
        p += len;
    }
    maps
}

fn ivmd(t: &[u8], m: usize, last_bdf: u32) {
    let flags = u8_at(t, m + IVMD_FLAGS);
    if flags & (IVMD_FLAG_UNITY_MAP | IVMD_FLAG_EXCL_RANGE) == 0 {
        return;
    }
    let Some((e, s)) = unity_map_range(t, m, last_bdf) else { return };
    let seg = u16_at(t, m + IVMD_PCI_SEG);
    kprintln!("[amd-vi] {} devid_start: {:04x}:{} devid_end: {:04x}:{} range_start: {:016x} range_end: {:016x} flags: {:x} (prot {:x})",
        s, seg, bdf(e.devid_start), seg, bdf(e.devid_end), e.address_start, e.address_end, flags, e.prot);
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
    let Some(regs) = map_regs(phys, end) else {
        kprintln!("[amd-vi] the register window at {:#x} could not be mapped", phys);
        return;
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

/// The IOMMU's register window, mapped once and kept.
fn map_regs(phys: u64, end: u64) -> Option<Mmio> {
    let mut mapped = MAPPED.lock();
    if let Some(m) = mapped.iter().find(|m| m.0 == phys) {
        return Some(m.1);
    }
    // SAFETY: the IVHD names this window as the IOMMU's registers, reserved
    // as platform MMIO by acpi::platform_mmio_ranges; only this driver
    // touches it.
    let regs = unsafe { Mmio::map(phys & !0xfff, end) }.ok()?;
    mapped.push((phys, regs));
    Some(regs)
}

/// `find_last_devid_acpi` (init.c:606): the highest devid the IVHDs of the
/// chosen type name.
fn find_last_devid_acpi(t: &[u8], target: u8) -> u32 {
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
    last_bdf
}

/// One event of the event log (`iommu_print_event`, iommu.c:886).
#[derive(Clone, Copy)]
pub struct Event {
    pub kind: u32,
    pub devid: u16,
    pub pasid: u32,
    pub flags: u32,
    pub address: u64,
}

/// What `status` reports for `gpu iommu`.
pub struct Status {
    pub mmio_phys: u64,
    pub devid: u16,
    pub last_bdf: u32,
    pub control: u64,
    pub status: u64,
    pub events_total: u64,
    pub completion_waits: u64,
    /// The protected device and the pages it reaches.
    pub domain: Option<(PciAddr, usize)>,
}

/// The one protection domain's id; Linux allocates them from 1.
const DOMAIN_ID: u16 = 1;
/// Fault records kept for `take_faults`, and events printed in full.
const FAULTS_KEPT: usize = 16;
const PAGE: u64 = 4096;

/// A protection domain with its v1 page table (`struct protection_domain`
/// with `struct amd_io_pgtable`).
struct Domain {
    dev: PciAddr,
    devid: u16,
    /// `pgtable->root` (identity-mapped) and `pgtable->mode`.
    root: u64,
    mode: u64,
    /// `amd_iommu_hpt_level`.
    hpt_level: u64,
    /// The device's DTE is written: from here on a new root needs
    /// `amd_iommu_update_and_flush_device_table` (`domain->dev_list`).
    attached: bool,
    /// Table pages; freed only with the domain, like `free_sub_pt`.
    tables: Vec<DmaRegion>,
    granted: BTreeMap<u64, u32>,
    faults: Vec<Fault>,
    faults_total: u64,
}

fn pm_level_shift(x: u64) -> u64 {
    12 + x * 9
}

fn pm_level_size(x: u64) -> u64 {
    if x < 6 { (1u64 << pm_level_shift(x)) - 1 } else { u64::MAX }
}

fn pm_level_index(x: u64, a: u64) -> u64 {
    (a >> pm_level_shift(x)) & 0x1ff
}

fn pm_level_enc(x: u64) -> u64 {
    (x << 9) & 0xe00
}

fn pm_level_pde(x: u64, a: u64) -> u64 {
    a | pm_level_enc(x) | r::IOMMU_PTE_PR | r::IOMMU_PTE_IR | r::IOMMU_PTE_IW
}

fn pm_pte_level(pte: u64) -> u64 {
    (pte >> 9) & 7
}

fn pte_read(at: u64) -> u64 {
    // SAFETY: `at` is an entry of a table page this driver allocated
    // (identity-mapped, 8-byte aligned).
    unsafe { core::ptr::read_volatile(at as *const u64) }
}

fn pte_write(at: u64, v: u64) {
    // SAFETY: as in `pte_read`.
    unsafe { core::ptr::write_volatile(at as *mut u64, v) }
}

/// `amd_iommu_atomic128_set`: one 128-bit store, as the IOMMU reads the
/// DTE in a single 256-bit transaction.
fn atomic128_set(at: u64, lo: u64, hi: u64) {
    let p = at as *mut u64;
    // SAFETY: `at` is a 16-byte aligned half of a DTE in the device table
    // (identity-mapped, owned by this driver, IOMMU lock held). CMPXCHG16B
    // was checked in `enable`. rbx is reserved by LLVM, so `lo` travels
    // in a scratch register and is swapped in around the instruction.
    unsafe {
        let (mut old_lo, mut old_hi) = (core::ptr::read_volatile(p), core::ptr::read_volatile(p.add(1)));
        core::arch::asm!(
            "xchg {lo}, rbx",
            "cmpxchg16b [{p}]",
            "mov rbx, {lo}",
            p = in(reg) p,
            lo = inout(reg) lo => _,
            inout("rax") old_lo,
            inout("rdx") old_hi,
            in("rcx") hi,
            options(nostack),
        );
        let _ = (old_lo, old_hi);
    }
}

/// `build_inv_address` (iommu.c:1225).
fn build_inv_address(mut address: u64, size: u64) -> u64 {
    let pages = ((address & (PAGE - 1)) + size).div_ceil(PAGE);
    if pages == 1 {
        return address & !(PAGE - 1);
    }
    let end = address + size - 1;
    // msb_diff would hold the index of the most significant bit that
    // flipped between the start and the end.
    let msb_diff = 63 - (end ^ address).leading_zeros() as u64;
    // Bits 63:52 are sign extended. If for some reason bit 51 is
    // different between the start and the end, invalidate everything.
    if msb_diff > 51 {
        address = r::CMD_INV_IOMMU_ALL_PAGES_ADDRESS;
    } else {
        // The msb-bit must be clear on the address. Just set all the
        // lower bits.
        address |= (1u64 << msb_diff) - 1;
    }
    // Clear bits 11:0
    address &= !(PAGE - 1);
    // Set the size bit - we flush more than one 4kb page
    address | r::CMD_INV_IOMMU_PAGES_SIZE_MASK as u64
}

/// `build_inv_iommu_pages` (iommu.c:1263), host table, no PASID.
fn build_inv_iommu_pages(address: u64, size: u64, domid: u16) -> [u32; 4] {
    let inv_address = build_inv_address(address, size);
    [
        0,
        domid as u32 | r::CMD_INV_IOMMU_PAGES << 28,
        // PDE bit - we want to flush everything, not only the PTEs
        inv_address as u32 | r::CMD_INV_IOMMU_PAGES_PDE_MASK,
        (inv_address >> 32) as u32,
    ]
}

/// The IOMMU after stage 1.
struct Iommu {
    regs: Mmio,
    mmio_phys: u64,
    devid: u16,
    /// `acpi_flags`: the IVHD's recommended feature bits.
    acpi_flags: u8,
    efr: u64,
    last_bdf: u32,
    dev_table: DmaRegion,
    dev_table_size: u64,
    /// `alias_table[devid]`, identity where the IVHD names no alias.
    alias_table: Vec<u16>,
    /// The IVHD's persistent DTE flags, `amd_ivhd_dev_flags_list`:
    /// (first, last, dte).
    ivhd_dte_flags: Vec<(u16, u16, [u64; 4])>,
    cmd_buf: DmaRegion,
    cmd_buf_head: u32,
    cmd_buf_tail: u32,
    cmd_sem: DmaRegion,
    cmd_sem_val: u64,
    need_sync: bool,
    evt_buf: DmaRegion,
    events: Vec<Event>,
    events_total: u64,
    completion_waits: u64,
    /// `amd_iommu_np_cache`: the IOMMU caches non-present entries.
    np_cache: bool,
    /// `pci_seg->unity_map`.
    unity_map: Vec<UnityMap>,
    domain: Option<Domain>,
}

static IOMMU: spin::Mutex<Option<Iommu>> = spin::Mutex::new(None);

/// `udelay`.
fn udelay(us: u64) {
    let freq = crate::interrupts::tsc_freq();
    if freq == 0 {
        for _ in 0..(us * 100) { core::hint::spin_loop(); }
        return;
    }
    let deadline = crate::interrupts::rdtsc() + freq / 1_000_000 * us;
    while crate::interrupts::rdtsc() < deadline { core::hint::spin_loop(); }
}

impl Iommu {
    /// `iommu_feature_set` (init.c:421).
    fn feature_set(&self, val: u64, mask: u64, shift: u32) {
        let mut ctrl = self.regs.r64(r::MMIO_CONTROL_OFFSET);
        let mask = mask << shift;
        ctrl &= !mask;
        ctrl |= (val << shift) & mask;
        self.regs.w64(r::MMIO_CONTROL_OFFSET, ctrl);
    }

    /// `iommu_feature_enable`.
    fn feature_enable(&self, bit: u32) {
        self.feature_set(1, 1, bit);
    }

    /// `iommu_feature_disable`.
    fn feature_disable(&self, bit: u32) {
        self.feature_set(0, 1, bit);
    }

    /// `iommu_disable` (init.c:449).
    fn disable(&self) {
        // Disable command buffer
        self.feature_disable(r::CONTROL_CMDBUF_EN);
        // Disable event logging and event interrupts
        self.feature_disable(r::CONTROL_EVT_INT_EN);
        self.feature_disable(r::CONTROL_EVT_LOG_EN);
        // Disable IOMMU GA_LOG
        self.feature_disable(r::CONTROL_GALOG_EN);
        self.feature_disable(r::CONTROL_GAINT_EN);
        // Disable IOMMU PPR logging
        self.feature_disable(r::CONTROL_PPRLOG_EN);
        self.feature_disable(r::CONTROL_PPRINT_EN);
        // Disable IOMMU hardware itself
        self.feature_disable(r::CONTROL_IOMMU_EN);
    }

    /// `iommu_init_flags` (init.c:2773).
    fn init_flags(&self) {
        let f = self.acpi_flags as u32;
        let set = |mask: u32, bit: u32| if f & mask != 0 { self.feature_enable(bit) } else { self.feature_disable(bit) };
        set(r::IVHD_FLAG_HT_TUN_EN_MASK, r::CONTROL_HT_TUN_EN);
        set(r::IVHD_FLAG_PASSPW_EN_MASK, r::CONTROL_PASSPW_EN);
        set(r::IVHD_FLAG_RESPASSPW_EN_MASK, r::CONTROL_RESPASSPW_EN);
        set(r::IVHD_FLAG_ISOC_EN_MASK, r::CONTROL_ISOC_EN);
        // make IOMMU memory accesses cache coherent
        self.feature_enable(r::CONTROL_COHERENT_EN);
        // Set IOTLB invalidation timeout to 1s
        self.feature_set(r::CTRL_INV_TO_1S as u64, r::CTRL_INV_TO_MASK as u64, r::CONTROL_INV_TIMEOUT);
        // Enable Enhanced Peripheral Page Request Handling
        if self.efr & r::FEATURE_EPHSUP != 0 {
            self.feature_enable(r::CONTROL_EPH_EN);
        }
    }

    /// `iommu_set_device_table` (init.c:404).
    fn set_device_table(&self) {
        let entry = self.dev_table.phys() | ((self.dev_table_size >> 12) - 1);
        self.regs.w64(r::MMIO_DEV_TABLE_OFFSET, entry);
    }

    /// `amd_iommu_reset_cmd_buffer` (init.c:805).
    fn reset_cmd_buffer(&mut self) {
        self.feature_disable(r::CONTROL_CMDBUF_EN);
        self.regs.w32(r::MMIO_CMD_HEAD_OFFSET, 0);
        self.regs.w32(r::MMIO_CMD_TAIL_OFFSET, 0);
        self.cmd_buf_head = 0;
        self.cmd_buf_tail = 0;
        self.feature_enable(r::CONTROL_CMDBUF_EN);
    }

    /// `iommu_enable_command_buffer` (init.c:821).
    fn enable_command_buffer(&mut self) {
        let entry = self.cmd_buf.phys() | r::MMIO_CMD_SIZE_512;
        self.regs.w64(r::MMIO_CMD_BUF_OFFSET, entry);
        self.reset_cmd_buffer();
    }

    /// `iommu_enable_gt` (init.c:1139).
    fn enable_gt(&self) {
        if self.efr & r::FEATURE_GT == 0 {
            return;
        }
        self.feature_enable(r::CONTROL_GT_EN);
    }

    /// `iommu_enable_ga` (init.c:2847). `amd_iommu_guest_ir` is
    /// AMD_IOMMU_GUEST_IR_VAPIC by default and stays so with GAM_VAPIC
    /// (early_amd_iommu_init); the 128-bit IRTE format matters only to
    /// interrupt remapping, which is stage 5.
    fn enable_ga(&self) {
        if self.efr & r::FEATURE_GAM_VAPIC != 0 {
            self.feature_enable(r::CONTROL_GA_EN);
        }
    }

    /// `copy_cmd_to_buffer` (iommu.c:1182).
    fn copy_cmd_to_buffer(&mut self, cmd: [u32; 4]) {
        let tail = self.cmd_buf_tail;
        self.cmd_buf.write_obj(tail as u64, cmd);
        let tail = (tail + r::IOMMU_CMD_SIZE as u32) % r::CMD_BUFFER_SIZE;
        self.cmd_buf_tail = tail;
        // Tell the IOMMU about it
        self.regs.w32(r::MMIO_CMD_TAIL_OFFSET, tail);
    }

    /// `__iommu_queue_command_sync` (iommu.c:1338).
    fn queue_command_sync(&mut self, cmd: [u32; 4], sync: bool) -> Result<(), &'static str> {
        let mut count = 0u32;
        let next_tail = (self.cmd_buf_tail + r::IOMMU_CMD_SIZE as u32) % r::CMD_BUFFER_SIZE;
        loop {
            let left = self.cmd_buf_head.wrapping_sub(next_tail) % r::CMD_BUFFER_SIZE;
            if left > 0x20 {
                break;
            }
            // Skip udelay() the first time around
            if count > 0 {
                if count == r::LOOP_TIMEOUT {
                    kprintln!("[amd-vi] Command buffer timeout");
                    return Err("command buffer timeout");
                }
                udelay(1);
            }
            count += 1;
            // Update head and recheck remaining space
            self.cmd_buf_head = self.regs.r32(r::MMIO_CMD_HEAD_OFFSET);
        }
        self.copy_cmd_to_buffer(cmd);
        // Do we need to make sure all commands are processed?
        self.need_sync = sync;
        Ok(())
    }

    /// `wait_on_sem` (iommu.c:1160).
    fn wait_on_sem(&self, data: u64) -> Result<(), &'static str> {
        let mut i = 0;
        while (self.cmd_sem.r64(0u64).wrapping_sub(data) as i64) < 0 && i < r::LOOP_TIMEOUT {
            udelay(1);
            i += 1;
        }
        if i == r::LOOP_TIMEOUT {
            kprintln!("[amd-vi] Completion-Wait loop timed out");
            return Err("completion wait timed out");
        }
        Ok(())
    }

    /// `iommu_completion_wait` with `build_completion_wait` (iommu.c:1404,
    /// 1200): the IOMMU stores `data` at the semaphore when every command
    /// before it is done.
    fn completion_wait(&mut self) -> Result<(), &'static str> {
        if !self.need_sync {
            return Ok(());
        }
        self.cmd_sem_val += 1;
        let data = self.cmd_sem_val;
        let paddr = self.cmd_sem.phys();
        let cmd = [
            paddr as u32 | r::CMD_COMPL_WAIT_STORE_MASK,
            (paddr >> 32) as u32 | r::CMD_COMPL_WAIT << 28,
            data as u32,
            (data >> 32) as u32,
        ];
        self.queue_command_sync(cmd, false)?;
        self.completion_waits += 1;
        self.wait_on_sem(data)
    }

    /// `amd_iommu_flush_all_caches` (iommu.c:1547): with FEATURE_IA one
    /// INVALIDATE_ALL (`amd_iommu_flush_all`, `build_inv_all`).
    fn flush_all_caches(&mut self) -> Result<(), &'static str> {
        if self.efr & r::FEATURE_IA == 0 {
            return Err("the IOMMU has no INVALIDATE_ALL; flushing entry by entry is not ported");
        }
        self.queue_command_sync([0, r::CMD_INV_ALL << 28, 0, 0], true)?;
        self.completion_wait()
    }

    /// `iommu_enable_event_buffer` (init.c:888).
    fn enable_event_buffer(&self) {
        let entry = self.evt_buf.phys() | r::EVTLOG_LEN_MASK_DEF;
        self.regs.w64(r::MMIO_EVT_BUF_OFFSET, entry);
        // set head and tail to zero manually
        self.regs.w32(r::MMIO_EVT_HEAD_OFFSET, 0);
        self.regs.w32(r::MMIO_EVT_TAIL_OFFSET, 0);
        self.feature_enable(r::CONTROL_EVT_LOG_EN);
    }

    /// `iommu_print_event` (iommu.c:886): one entry, recorded and printed.
    fn print_event(&mut self, at: u32) {
        let mut count = 0;
        let ev = loop {
            let e: [u32; 4] = self.evt_buf.read_obj(at as u64);
            let kind = (e[1] >> r::EVENT_TYPE_SHIFT) & r::EVENT_TYPE_MASK;
            if kind != 0 {
                break e;
            }
            // Did we hit the erratum?
            count += 1;
            if count == r::LOOP_TIMEOUT {
                kprintln!("[amd-vi] No event written to event log");
                return;
            }
            udelay(1);
        };
        let event = Event {
            kind: (ev[1] >> r::EVENT_TYPE_SHIFT) & r::EVENT_TYPE_MASK,
            devid: ((ev[0] >> r::EVENT_DEVID_SHIFT) & r::EVENT_DEVID_MASK) as u16,
            pasid: (ev[0] & r::EVENT_DOMID_MASK_HI) | (ev[1] & r::EVENT_DOMID_MASK_LO),
            flags: (ev[1] >> r::EVENT_FLAGS_SHIFT) & r::EVENT_FLAGS_MASK,
            address: (ev[3] as u64) << 32 | ev[2] as u64,
        };
        self.events_total += 1;
        if event.kind == r::EVENT_TYPE_IO_FAULT {
            if let Some(dom) = self.domain.as_mut().filter(|d| d.devid == event.devid) {
                dom.faults_total += 1;
                if dom.faults.len() >= FAULTS_KEPT {
                    dom.faults.remove(0);
                }
                dom.faults.push(Fault {
                    addr: event.address,
                    source: event.devid,
                    reason: event.flags as u16,
                    read: event.flags & r::EVENT_FLAG_RW == 0,
                });
            }
        }
        if self.events.len() >= FAULTS_KEPT {
            self.events.remove(0);
        }
        self.events.push(event);
        let d = bdf(event.devid as u32);
        let name = match event.kind {
            r::EVENT_TYPE_IO_FAULT => "IO_PAGE_FAULT",
            r::EVENT_TYPE_ILL_DEV => "ILLEGAL_DEV_TABLE_ENTRY",
            r::EVENT_TYPE_DEV_TAB_ERR => "DEV_TAB_HARDWARE_ERROR",
            r::EVENT_TYPE_PAGE_TAB_ERR => "PAGE_TAB_HARDWARE_ERROR",
            r::EVENT_TYPE_ILL_CMD => "ILLEGAL_COMMAND_ERROR",
            r::EVENT_TYPE_CMD_HARD_ERR => "COMMAND_HARDWARE_ERROR",
            r::EVENT_TYPE_IOTLB_INV_TO => "IOTLB_INV_TIMEOUT",
            r::EVENT_TYPE_INV_DEV_REQ => "INVALID_DEVICE_REQUEST",
            r::EVENT_TYPE_INV_PPR_REQ => "INVALID_PPR_REQUEST",
            r::EVENT_TYPE_RMP_FAULT => "RMP_PAGE_FAULT",
            r::EVENT_TYPE_RMP_HW_ERR => "RMP_HW_ERROR",
            _ => "UNKNOWN",
        };
        // Linux rate-limits these lines; past the first few only the count grows.
        if self.events_total <= FAULTS_KEPT as u64 {
            kprintln!("[amd-vi] Event logged [{} device=0000:{} pasid={:#07x} address={:#x} flags={:#06x}]",
                name, d, event.pasid, event.address, event.flags);
        }
        // To detect the hardware errata 732 we need to clear the entry back
        // to zero; an IO_PAGE_FAULT returns before that in Linux.
        if event.kind != r::EVENT_TYPE_IO_FAULT {
            self.evt_buf.write_obj(at as u64, [0u32; 4]);
        }
    }

    /// `iommu_poll_events` (iommu.c:984).
    fn poll_events(&mut self) {
        let mut head = self.regs.r32(r::MMIO_EVT_HEAD_OFFSET);
        let tail = self.regs.r32(r::MMIO_EVT_TAIL_OFFSET);
        while head != tail {
            self.print_event(head);
            // Update head pointer of hardware ring-buffer
            head = (head + r::EVTLOG_ENTRY_SIZE) % r::EVTLOG_SIZE_DEF;
            self.regs.w32(r::MMIO_EVT_HEAD_OFFSET, head);
        }
    }

    /// `iommu_queue_command`: queued with a completion wait owed.
    fn queue_command(&mut self, cmd: [u32; 4]) -> Result<(), &'static str> {
        self.queue_command_sync(cmd, true)
    }

    /// `iommu_flush_dte` + `iommu_completion_wait` (iommu.c:1455-1470).
    fn flush_dte_sync(&mut self, devid: u16) -> Result<(), &'static str> {
        self.queue_command([devid as u32, r::CMD_INV_DEV_ENTRY << 28, 0, 0])?;
        self.completion_wait()
    }

    /// `device_flush_dte` (iommu.c:1584): the devid and its alias.
    fn device_flush_dte(&mut self, devid: u16) -> Result<(), &'static str> {
        self.queue_command([devid as u32, r::CMD_INV_DEV_ENTRY << 28, 0, 0])?;
        let alias = self.alias_table.get(devid as usize).copied().unwrap_or(devid);
        if alias != devid {
            self.queue_command([alias as u32, r::CMD_INV_DEV_ENTRY << 28, 0, 0])?;
        }
        Ok(())
    }

    /// `amd_iommu_flush_tlb_domid` (iommu.c).
    fn flush_tlb_domid(&mut self, domid: u16) -> Result<(), &'static str> {
        self.queue_command(build_inv_iommu_pages(0, r::CMD_INV_IOMMU_ALL_PAGES_ADDRESS, domid))?;
        self.completion_wait()
    }

    /// `amd_iommu_get_ivhd_dte_flags`: the persistent bits for `devid`.
    fn ivhd_dte_flags(&self, devid: u16) -> Option<[u64; 4]> {
        self.ivhd_dte_flags.iter().find(|e| devid >= e.0 && devid <= e.1).map(|e| e.2)
    }

    /// `update_dte256` (iommu.c:134) for a host-table DTE: V written last
    /// when it was clear, and both halves before a flush.
    fn update_dte256(&mut self, devid: u16, mut new: [u64; 4]) -> Result<(), &'static str> {
        let at = devid as u64 * r::DEV_TABLE_ENTRY_SIZE;
        let ptr = self.dev_table.phys() + at;
        let old2 = self.dev_table.r64(at + 16);
        // write_dte_upper128: preserve DTE_DATA2_INTR_MASK.
        let upper = |new: &mut [u64; 4]| {
            new[2] &= !r::DTE_DATA2_INTR_MASK;
            new[2] |= old2 & r::DTE_DATA2_INTR_MASK;
            atomic128_set(ptr + 16, new[2], new[3]);
        };
        if self.dev_table.r64(at) & r::DTE_FLAG_V == 0 {
            // Existing DTE is not valid.
            upper(&mut new);
            atomic128_set(ptr, new[0], new[1]);
        } else if new[0] & r::DTE_FLAG_V == 0 {
            // Existing DTE is valid. New DTE is not valid.
            atomic128_set(ptr, new[0], new[1]);
            upper(&mut new);
        } else {
            // Both DTEs are valid; neither has a guest page table.
            upper(&mut new);
            atomic128_set(ptr, new[0], new[1]);
        }
        self.flush_dte_sync(devid)
    }

    /// `set_dte_entry` with `make_clear_dte` (iommu.c:2074, 2030).
    fn set_dte_entry(&mut self) -> Result<(), &'static str> {
        let d = self.domain.as_ref().ok_or("no domain")?;
        let devid = d.devid;
        // All existing DTE must have V bit set
        let mut new = [r::DTE_FLAG_V, 0, 0, 0];
        if d.mode != r::PAGE_MODE_NONE as u64 {
            new[0] |= d.root;
        }
        new[0] |= (d.mode & r::DEV_ENTRY_MODE_MASK as u64) << r::DEV_ENTRY_MODE_SHIFT;
        new[0] |= r::DTE_FLAG_IR | r::DTE_FLAG_IW;
        new[0] |= r::DTE_FLAG_TV;
        let at = devid as u64 * r::DEV_TABLE_ENTRY_SIZE;
        let old_domid = (self.dev_table.r64(at + 8) & r::DEV_DOMID_MASK) as u16;
        new[1] |= DOMAIN_ID as u64;
        // Restore cached persistent DTE bits from the IVRS table.
        if let Some(initial) = self.ivhd_dte_flags(devid) {
            for (n, i) in new.iter_mut().zip(initial) {
                *n |= i;
            }
        }
        self.update_dte256(devid, new)?;
        if old_domid != 0 {
            self.flush_tlb_domid(old_domid)?;
        }
        Ok(())
    }

    /// `amd_iommu_update_and_flush_device_table` (iommu.c:1797).
    fn update_and_flush_device_table(&mut self) -> Result<(), &'static str> {
        let Some(devid) = self.domain.as_ref().filter(|d| d.attached).map(|d| d.devid) else { return Ok(()) };
        self.set_dte_entry()?;
        self.device_flush_dte(devid)?;
        self.completion_wait()
    }

    /// `__domain_flush_pages` with `domain_flush_pages_v1` (iommu.c:1669,
    /// 1641); no device has ATS.
    fn domain_flush_pages_once(&mut self, address: u64, size: u64) -> Result<(), &'static str> {
        self.queue_command(build_inv_iommu_pages(address, size, DOMAIN_ID))
    }

    /// `amd_iommu_domain_flush_pages` (iommu.c:1697), then
    /// `domain_flush_complete`.
    fn domain_flush_pages(&mut self, mut address: u64, mut size: u64) -> Result<(), &'static str> {
        if !self.np_cache || size >= 1 << 52 {
            self.domain_flush_pages_once(address, size)?;
            return self.completion_wait();
        }
        // With NpCache, repeated flushes of naturally aligned ranges.
        while size != 0 {
            let addr_alignment = address.trailing_zeros();
            let size_alignment = 63 - size.leading_zeros();
            let min_alignment = if address != 0 { addr_alignment.min(size_alignment) } else { size_alignment };
            let flush_size = 1u64 << min_alignment;
            self.domain_flush_pages_once(address, flush_size)?;
            address += flush_size;
            size -= flush_size;
        }
        self.completion_wait()
    }

    /// A zeroed table page for the domain.
    fn alloc_table(&mut self) -> Option<u64> {
        let page = DmaRegion::alloc_zeroed(1)?;
        let d = self.domain.as_mut()?;
        d.tables.push(page);
        Some(page.phys())
    }

    /// `increase_address_space` (io_pgtable.c:106) for 4K pages.
    fn increase_address_space(&mut self, address: u64) -> bool {
        let Some(d) = self.domain.as_ref() else { return false };
        if address <= pm_level_size(d.mode) && d.mode >= 1 {
            return true;
        }
        if d.mode == d.hpt_level {
            kprintln!("[amd-vi] the page table is at its deepest level ({})", d.mode);
            return false;
        }
        let Some(pte) = self.alloc_table() else { return false };
        let d = self.domain.as_mut().unwrap();
        pte_write(pte, pm_level_pde(d.mode, d.root));
        d.root = pte;
        d.mode += 1;
        if let Err(e) = self.update_and_flush_device_table() {
            kprintln!("[amd-vi] {}", e);
        }
        true
    }

    /// `alloc_pte` (io_pgtable.c:151) for a 4K page: the leaf entry,
    /// with the directories on the way allocated.
    fn alloc_pte(&mut self, address: u64, updated: &mut bool) -> Option<u64> {
        let last_addr = address + (PAGE - 1);
        while last_addr > pm_level_size(self.domain.as_ref()?.mode) {
            if !self.increase_address_space(last_addr) {
                return None;
            }
        }
        let d = self.domain.as_ref()?;
        let mut level = d.mode - 1;
        let mut pte = d.root + 8 * pm_level_index(level, address);
        while level > 0 {
            let cur = pte_read(pte);
            let pte_level = pm_pte_level(cur);
            let present = cur & r::IOMMU_PTE_PR != 0;
            // This driver maps 4K pages only, so a large PTE is never there.
            if present && pte_level == r::PAGE_MODE_7_LEVEL as u64 {
                return None;
            }
            if !present || pte_level == r::PAGE_MODE_NONE as u64 {
                let page = self.alloc_table()?;
                pte_write(pte, pm_level_pde(level, page));
                if present {
                    *updated = true;
                }
                continue;
            }
            // No level skipping support yet
            if pte_level != level {
                return None;
            }
            level -= 1;
            pte = (cur & r::PM_ADDR_MASK) + 8 * pm_level_index(level, address);
        }
        Some(pte)
    }

    /// `fetch_pte` (io_pgtable.c:258) for a 4K leaf.
    fn fetch_pte(&self, address: u64) -> Option<u64> {
        let d = self.domain.as_ref()?;
        if address > pm_level_size(d.mode) {
            return None;
        }
        let mut level = d.mode - 1;
        let mut pte = d.root + 8 * pm_level_index(level, address);
        while level > 0 {
            let cur = pte_read(pte);
            // Not Present
            if cur & r::IOMMU_PTE_PR == 0 {
                return None;
            }
            let l = pm_pte_level(cur);
            if l == r::PAGE_MODE_7_LEVEL as u64 || l == r::PAGE_MODE_NONE as u64 || l != level {
                return None;
            }
            level -= 1;
            pte = (cur & r::PM_ADDR_MASK) + 8 * pm_level_index(level, address);
        }
        Some(pte)
    }

    /// `iommu_v1_map_pages` (io_pgtable.c:337) for one 4K page at IOVA =
    /// phys. False when the table could not grow.
    fn map_page(&mut self, paddr: u64, prot: u32, updated: &mut bool) -> bool {
        if prot & (r::IOMMU_PROT_IR | r::IOMMU_PROT_IW) == 0 {
            return false;
        }
        let Some(pte) = self.alloc_pte(paddr, updated) else { return false };
        // free_clear_pte: a 4K leaf has no sub-table to free.
        let mut v = paddr | r::IOMMU_PTE_PR | r::IOMMU_PTE_FC;
        if prot & r::IOMMU_PROT_IR != 0 {
            v |= r::IOMMU_PTE_IR;
        }
        if prot & r::IOMMU_PROT_IW != 0 {
            v |= r::IOMMU_PTE_IW;
        }
        pte_write(pte, v);
        true
    }

    /// `iommu_v1_unmap_pages` (io_pgtable.c:414) for one 4K page.
    fn unmap_page(&mut self, iova: u64) {
        if let Some(pte) = self.fetch_pte(iova) {
            pte_write(pte, 0);
        }
    }

    /// One more grant of each page; the flush `iommu_v1_map_pages` owes
    /// when it replaced an entry, or every time with NpCache
    /// (`amd_iommu_iotlb_sync_map`).
    fn grant_pages(&mut self, pages: &mut dyn Iterator<Item = u64>, prot: u32) {
        let (mut lo, mut hi, mut updated, mut fresh) = (u64::MAX, 0, false, false);
        for p in pages {
            let p = p & !(PAGE - 1);
            let Some(d) = self.domain.as_mut() else { return };
            let n = d.granted.entry(p).or_insert(0);
            *n += 1;
            if *n != 1 {
                continue;
            }
            if !self.map_page(p, prot, &mut updated) {
                kprintln!("[npk] iommu: no memory for the page table, {:#x} stays unreachable", p);
            }
            fresh = true;
            lo = lo.min(p);
            hi = hi.max(p + PAGE);
        }
        if fresh && (updated || self.np_cache) {
            if let Err(e) = self.domain_flush_pages(lo, hi - lo) {
                kprintln!("[npk] iommu: {}", e);
            }
        }
    }

    /// One grant less of each page; the pages whose last grant this was
    /// are unmapped and flushed as one gathered range
    /// (`amd_iommu_iotlb_sync`).
    fn revoke_pages(&mut self, pages: &mut dyn Iterator<Item = u64>) {
        let (mut lo, mut hi) = (u64::MAX, 0);
        for p in pages {
            let p = p & !(PAGE - 1);
            let Some(d) = self.domain.as_mut() else { return };
            let Some(n) = d.granted.get_mut(&p) else { continue };
            *n -= 1;
            if *n == 0 {
                d.granted.remove(&p);
                self.unmap_page(p);
                lo = lo.min(p);
                hi = hi.max(p + PAGE);
            }
        }
        if hi > lo {
            if let Err(e) = self.domain_flush_pages(lo, hi - lo) {
                kprintln!("[npk] iommu: {}", e);
            }
        }
    }

    /// `protection_domain_alloc` + `v1_alloc_pgtable` + the IVMD ranges
    /// (`amd_iommu_get_resv_regions`, mapped 1:1 like the IOMMU core's
    /// direct mappings) + `attach_device`.
    fn attach(&mut self, dev: PciAddr) -> Result<usize, &'static str> {
        let devid = (dev.bus as u16) << 8 | (dev.device as u16) << 3 | dev.function as u16;
        if devid as u32 > self.last_bdf {
            return Err("the IVHD does not name this device");
        }
        if self.alias_table[devid as usize] != devid
            || self.alias_table.iter().enumerate().any(|(i, a)| *a == devid && i != devid as usize) {
            return Err("the device shares its requester ID through an IVHD alias");
        }
        let hats = (self.efr & r::FEATURE_HATS) >> r::FEATURE_HATS.trailing_zeros();
        if hats == 3 {
            return Err("invalid host translation level (HATS 3)");
        }
        let root = DmaRegion::alloc_zeroed(1).ok_or("no memory for the page table")?;
        self.domain = Some(Domain {
            dev, devid, root: root.phys(), mode: r::PAGE_MODE_3_LEVEL as u64,
            hpt_level: hats + r::PAGE_MODE_4_LEVEL as u64, attached: false, tables: alloc::vec![root],
            granted: BTreeMap::new(), faults: Vec::new(), faults_total: 0,
        });
        let maps: Vec<UnityMap> = self.unity_map.iter()
            .filter(|u| devid as u32 >= u.devid_start && devid as u32 <= u.devid_end).copied().collect();
        let mut pages = 0;
        for u in &maps {
            let mut it = (u.address_start..u.address_end).step_by(PAGE as usize);
            self.grant_pages(&mut it, u.prot as u32);
            pages += ((u.address_end - u.address_start) / PAGE) as usize;
        }
        self.set_dte_entry()?;
        self.device_flush_dte(devid)?;
        self.completion_wait()?;
        self.domain.as_mut().unwrap().attached = true;
        Ok(pages)
    }

    /// `early_enable_iommu` (init.c:2901).
    fn early_enable(&mut self) -> Result<(), &'static str> {
        self.disable();
        self.init_flags();
        self.set_device_table();
        self.enable_command_buffer();
        // iommu_set_exclusion_range: exclusion_start is never set.
        self.enable_gt();
        self.enable_ga();
        // iommu_enable_xt: x2APIC mode only with interrupt remapping.
        // iommu_enable_irtcachedis: amd_iommu_irtcachedis is off.
        // iommu_enable_2k_int: EFR2 without NUM_INT_REMAP_SUP_2K on Renoir;
        // it belongs to interrupt remapping like the two above.
        self.feature_enable(r::CONTROL_IOMMU_EN);
        self.flush_all_caches()
    }
}

/// `set_dte_bit` (init.c:1148).
fn set_dte_bit(dte: &mut [u64; 4], bit: u32) {
    dte[((bit >> 6) & 3) as usize] |= 1u64 << (bit & 0x3f);
}

/// `set_dev_entry_from_acpi_range` (init.c:1289): the IVHD's flags as
/// DTE bits, into the table and the persistent list, for `first..=last`.
fn set_dev_entry_from_acpi_range(io: &mut Iommu, first: u32, last: u32, flags: u8) {
    if flags == 0 {
        return;
    }
    if io.ivhd_dte_flags.iter().any(|e| e.0 as u32 == first && e.1 as u32 == last) {
        return;
    }
    let mut dte = [0u64; 4];
    let f = flags as u32;
    let bits = [
        (r::ACPI_DEVFLAG_INITPASS, r::DEV_ENTRY_INIT_PASS), (r::ACPI_DEVFLAG_EXTINT, r::DEV_ENTRY_EINT_PASS),
        (r::ACPI_DEVFLAG_NMI, r::DEV_ENTRY_NMI_PASS), (r::ACPI_DEVFLAG_SYSMGT1, r::DEV_ENTRY_SYSMGT1),
        (r::ACPI_DEVFLAG_SYSMGT2, r::DEV_ENTRY_SYSMGT2), (r::ACPI_DEVFLAG_LINT0, r::DEV_ENTRY_LINT0_PASS),
        (r::ACPI_DEVFLAG_LINT1, r::DEV_ENTRY_LINT1_PASS),
    ];
    for (mask, bit) in bits {
        if f & mask != 0 {
            set_dte_bit(&mut dte, bit);
        }
    }
    // Apply erratum 63, which needs info in initial_dte
    let mask = r::DTE_DATA1_SYSMGT_MASK;
    if (dte[1] & mask) >> mask.trailing_zeros() == 0x1 {
        dte[0] |= r::DTE_FLAG_IW;
    }
    io.ivhd_dte_flags.push((first as u16, last as u16, dte));
    for i in first..=last.min(io.last_bdf) {
        let at = i as u64 * r::DEV_TABLE_ENTRY_SIZE;
        for (k, d) in dte.iter().enumerate() {
            io.dev_table.w64(at + 8 * k as u64, *d);
        }
    }
}

/// `init_iommu_from_acpi` (init.c:1461): the entries' flags and aliases.
fn init_iommu_from_acpi(io: &mut Iommu, t: &[u8], h: usize) -> Result<(), &'static str> {
    let ty = u8_at(t, h + IVHD_TYPE);
    io.acpi_flags = u8_at(t, h + IVHD_FLAGS);
    let size = get_ivhd_header_size(ty);
    if size == 0 {
        return Err("unsupported IVHD type");
    }
    let (mut p, end) = (h + size, h + u16_at(t, h + IVHD_LENGTH) as usize);
    let (mut devid_start, mut devid_to, mut flags, mut alias) = (0u32, 0u32, 0u8, false);
    while p < end {
        let e_ty = u8_at(t, p);
        let devid = u16_at(t, p + IVHD_ENTRY_DEVID) as u32;
        let e_flags = u8_at(t, p + IVHD_ENTRY_FLAGS);
        let ext = u32_at(t, p + IVHD_ENTRY_EXT);
        match e_ty {
            IVHD_DEV_ALL => set_dev_entry_from_acpi_range(io, 0, io.last_bdf, e_flags),
            IVHD_DEV_SELECT | IVHD_DEV_EXT_SELECT => set_dev_entry_from_acpi_range(io, devid, devid, e_flags),
            IVHD_DEV_SELECT_RANGE_START | IVHD_DEV_EXT_SELECT_RANGE => {
                devid_start = devid;
                flags = e_flags;
                alias = false;
            }
            IVHD_DEV_ALIAS => {
                let to = (ext >> 8) & 0xffff;
                set_dev_entry_from_acpi_range(io, devid, devid, e_flags);
                set_dev_entry_from_acpi_range(io, to, to, e_flags);
                if let Some(a) = io.alias_table.get_mut(devid as usize) {
                    *a = to as u16;
                }
            }
            IVHD_DEV_ALIAS_RANGE => {
                devid_start = devid;
                flags = e_flags;
                devid_to = (ext >> 8) & 0xffff;
                alias = true;
            }
            IVHD_DEV_RANGE_END => {
                if alias {
                    for d in devid_start..=devid {
                        if let Some(a) = io.alias_table.get_mut(d as usize) {
                            *a = devid_to as u16;
                        }
                    }
                    set_dev_entry_from_acpi_range(io, devid_to, devid_to, flags);
                }
                set_dev_entry_from_acpi_range(io, devid_start, devid, flags);
            }
            IVHD_DEV_SPECIAL => {
                let special = (ext >> 8) & 0xffff;
                set_dev_entry_from_acpi_range(io, special, special, e_flags);
            }
            IVHD_DEV_ACPI_HID if ty == 0x40 => set_dev_entry_from_acpi_range(io, devid, devid, e_flags),
            _ => {}
        }
        let n = ivhd_entry_length(t, p);
        if n == 0 {
            break;
        }
        p += n;
    }
    Ok(())
}

/// Stage 1: the IOMMU on, every device passed through. Idempotent.
pub fn enable() -> Result<(), &'static str> {
    if IOMMU.lock().is_some() {
        return Ok(());
    }
    let ivrs = crate::drivers::acpi::table(b"IVRS").ok_or("no IVRS table")?;
    let t = ivrs.bytes();
    if !check_ivrs_checksum(t) {
        return Err("IVRS invalid checksum");
    }
    let ivinfo = u32_at(t, IOMMU_IVINFO_OFFSET);
    let target = get_highest_supported_ivhd_type(t);
    let last_bdf = find_last_devid_acpi(t, target);

    // The one IVHD of the target type; a second IOMMU is not ported.
    let mut ivhds = Vec::new();
    let mut p = IVRS_HEADER_LENGTH;
    while p + IVHD_LENGTH + 2 <= t.len() {
        let len = u16_at(t, p + IVHD_LENGTH) as usize;
        if len == 0 {
            break;
        }
        if u8_at(t, p + IVHD_TYPE) == target {
            ivhds.push(p);
        }
        p += len;
    }
    let h = match ivhds.as_slice() {
        [h] => *h,
        [] => return Err("no IVHD of the target type"),
        _ => return Err("more than one IOMMU: not ported"),
    };

    // alloc_pci_segment (init.c:1723): device table and alias table.
    let dev_table_size = (((last_bdf as u64 + 1) * r::DEV_TABLE_ENTRY_SIZE).next_power_of_two()).max(4096);
    let dev_table = DmaRegion::alloc_zeroed_dma32_aligned((dev_table_size / 4096) as usize)
        .ok_or("no memory for the device table")?;
    let alias_table: Vec<u16> = (0..=last_bdf as u16).collect();

    // init_iommu_one (init.c:1891) with early_iommu_features_init (305).
    let ty = u8_at(t, h + IVHD_TYPE);
    let mmio_phys = u64_at(t, h + IVHD_MMIO_PHYS);
    let efr_attr = u32_at(t, h + IVHD_EFR_ATTR);
    let (end, ivhd_efr) = match ty {
        0x10 => {
            let cntr = efr_attr != 0 && efr_attr & (0xf << 13) != 0 && efr_attr & (0x3f << 17) != 0;
            (if cntr { MMIO_REG_END_OFFSET } else { MMIO_CNTR_CONF_OFFSET }, None)
        }
        _ => {
            let efr_reg = u64_at(t, h + IVHD_EFR_REG);
            if efr_attr & (1 << IOMMU_IVHD_ATTR_HATDIS_SHIFT) != 0 {
                return Err("Host Address Translation is not supported");
            }
            (if efr_reg & (1 << 9) != 0 { MMIO_REG_END_OFFSET } else { MMIO_CNTR_CONF_OFFSET },
             (ivinfo & IOMMU_IVINFO_EFRSUP != 0).then(|| (efr_reg, u64_at(t, h + IVHD_EFR_REG2))))
        }
    };
    let regs = map_regs(mmio_phys, end).ok_or("cannot map the IOMMU's registers")?;
    let efr = ivhd_efr.map_or_else(|| regs.r64(r::MMIO_EXT_FEATURES), |(e, _)| e);

    // init_iommu_one_late (init.c:1970): alloc_iommu_buffers.
    let cmd_sem = DmaRegion::alloc_zeroed_dma32_aligned(1).ok_or("no memory for the completion semaphore")?;
    let cmd_buf = DmaRegion::alloc_zeroed_dma32_aligned((r::CMD_BUFFER_SIZE / 4096) as usize)
        .ok_or("no memory for the command buffer")?;
    let evt_buf = DmaRegion::alloc_zeroed_dma32_aligned((r::EVTLOG_SIZE_DEF / 4096) as usize)
        .ok_or("no memory for the event buffer")?;
    let mut io = Iommu {
        regs, mmio_phys, devid: u16_at(t, h + IVHD_DEVID), acpi_flags: 0, efr, last_bdf,
        dev_table, dev_table_size, alias_table, ivhd_dte_flags: Vec::new(),
        cmd_buf, cmd_buf_head: 0, cmd_buf_tail: 0, cmd_sem, cmd_sem_val: 0, need_sync: false,
        evt_buf, events: Vec::new(), events_total: 0, completion_waits: 0,
        np_cache: false, unity_map: init_memory_definitions(t, last_bdf), domain: None,
    };
    // iommu_init_pci (init.c:2185): the capability header.
    let id = io.devid;
    let iommu_pci = PciAddr { bus: (id >> 8) as u8, device: (id >> 3 & 0x1f) as u8, function: (id & 7) as u8 };
    let cap = pci::read32(iommu_pci, (u16_at(t, h + IVHD_CAP_PTR) as u32 + r::MMIO_CAP_HDR_OFFSET) as u8);
    if cap & (1 << r::IOMMU_CAP_NPCACHE) != 0 {
        kprintln!("[amd-vi] Using strict mode due to virtualization");
        io.np_cache = true;
    }
    init_iommu_from_acpi(&mut io, t, h)?;

    // init_translation_status (init.c:245): translation left on by the
    // firmware is switched off (not a kdump kernel).
    if io.regs.r64(r::MMIO_CONTROL_OFFSET) & (1 << r::CONTROL_IOMMU_EN) != 0 {
        kprintln!("[amd-vi] Translation was enabled for IOMMU but we are not in kdump mode");
        io.disable();
    }
    // early_amd_iommu_init: CMPXCHG16B is required for 256-bit DTE updates.
    if core::arch::x86_64::__cpuid(1).ecx & (1 << 13) == 0 {
        return Err("CMPXCHG16B is required");
    }
    // disable_iommus, then early_enable_iommus.
    io.disable();
    io.early_enable()?;
    // alloc_event_buffer + iommu_enable_event_buffer (state IOMMU_ENABLED).
    io.enable_event_buffer();
    // iommu_init_pci / late_iommu_features_init: EFR from MMIO against the IVHD's.
    let mmio_efr = io.regs.r64(r::MMIO_EXT_FEATURES);
    if ivhd_efr.is_some_and(|(e, _)| e != mmio_efr) {
        kprintln!("[amd-vi] [Firmware Bug]: EFR mismatch. Use IVHD EFR ({:#x} : {:#x}).", io.efr, mmio_efr);
    }
    // init_device_table_dma (block devices without a domain) is stage 6.
    let control = io.regs.r64(r::MMIO_CONTROL_OFFSET);
    kprintln!("[amd-vi] enabled: device table {:#x} ({} KiB, {} devids, {} persistent DTE flag ranges), \
               control {:#018x}, {} completion wait(s) OK",
        io.dev_table.phys(), io.dev_table_size >> 10, io.last_bdf + 1, io.ivhd_dte_flags.len(), control,
        io.completion_waits);
    io.poll_events();
    *IOMMU.lock() = Some(io);
    Ok(())
}

/// Read the event log (polled; no event interrupt).
pub fn poll() {
    if let Some(io) = IOMMU.lock().as_mut() {
        io.poll_events();
    }
}

/// One more `INVALIDATE_ALL` and completion wait, for a check that the
/// command path still runs.
pub fn flush_all() -> Result<(), &'static str> {
    match IOMMU.lock().as_mut() {
        Some(io) => io.flush_all_caches(),
        None => Err("AMD-Vi is off"),
    }
}

/// The IOMMU's state after stage 1, `None` while it is off.
pub fn status() -> Option<Status> {
    let g = IOMMU.lock();
    let io = g.as_ref()?;
    Some(Status {
        mmio_phys: io.mmio_phys,
        devid: io.devid,
        last_bdf: io.last_bdf,
        control: io.regs.r64(r::MMIO_CONTROL_OFFSET),
        status: io.regs.r64(r::MMIO_STATUS_OFFSET),
        events_total: io.events_total,
        completion_waits: io.completion_waits,
        domain: io.domain.as_ref().map(|d| (d.dev, d.granted.len())),
    })
}

/// Put `dev` behind the IOMMU (enabling it first). Afterwards the device
/// reaches its IVMD ranges and what `grant` adds; every other device stays
/// passed through. Translation stays on until reboot.
pub fn protect(dev: PciAddr) -> Result<(), &'static str> {
    enable()?;
    let mut g = IOMMU.lock();
    let io = g.as_mut().ok_or("AMD-Vi is off")?;
    if let Some(d) = io.domain.as_ref() {
        return if d.dev == dev { Ok(()) } else { Err("another device is protected already") };
    }
    match io.attach(dev) {
        Ok(pages) => {
            let d = io.domain.as_ref().unwrap();
            kprintln!("[npk] iommu: {:02x}:{:02x}.{} behind AMD-Vi @ {:#x} (domain {}, {}-level, HATS {}), {} pages direct-mapped",
                dev.bus, dev.device, dev.function, io.mmio_phys, DOMAIN_ID, d.mode, d.hpt_level, pages);
            io.poll_events();
            Ok(())
        }
        Err(e) => {
            io.domain = None;
            Err(e)
        }
    }
}

fn with_domain(dev: PciAddr, f: impl FnOnce(&mut Iommu)) {
    let mut g = IOMMU.lock();
    if let Some(io) = g.as_mut().filter(|io| io.domain.as_ref().is_some_and(|d| d.dev == dev)) {
        f(io);
    }
}

pub fn protected(dev: PciAddr) -> bool {
    IOMMU.lock().as_ref().is_some_and(|io| io.domain.as_ref().is_some_and(|d| d.dev == dev))
}

/// Let `dev` reach `pages` pages from `phys`. No-op for an unprotected device.
pub fn grant(dev: PciAddr, phys: u64, pages: usize) {
    let base = phys & !(PAGE - 1);
    with_domain(dev, |io| io.grant_pages(&mut (0..pages as u64).map(|i| base + i * PAGE),
        r::IOMMU_PROT_IR | r::IOMMU_PROT_IW));
}

/// Let `dev` reach every page of `list`, with one flush for the lot.
pub fn grant_list(dev: PciAddr, list: &[u64]) {
    with_domain(dev, |io| io.grant_pages(&mut list.iter().copied(), r::IOMMU_PROT_IR | r::IOMMU_PROT_IW));
}

/// Take back one grant of `pages` pages from `phys`. When it returns, the
/// device no longer reaches the pages whose last grant this was.
pub fn revoke(dev: PciAddr, phys: u64, pages: usize) {
    let base = phys & !(PAGE - 1);
    with_domain(dev, |io| io.revoke_pages(&mut (0..pages as u64).map(|i| base + i * PAGE)));
}

/// Take back one grant of every page of `list`, with one flush.
pub fn revoke_list(dev: PciAddr, list: &[u64]) {
    with_domain(dev, |io| io.revoke_pages(&mut list.iter().copied()));
}

/// New IO page faults of the protected device since the last call.
pub fn take_faults(dev: PciAddr) -> Vec<Fault> {
    let mut out = Vec::new();
    with_domain(dev, |io| {
        io.poll_events();
        out = core::mem::take(&mut io.domain.as_mut().unwrap().faults);
    });
    out
}

/// IO page faults of the protected device since it was protected.
pub fn faults_total() -> u64 {
    IOMMU.lock().as_ref().and_then(|io| io.domain.as_ref()).map_or(0, |d| d.faults_total)
}
