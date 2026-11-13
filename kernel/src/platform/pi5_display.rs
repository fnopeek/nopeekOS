//! The BCM2712 display as the firmware leaves it, read only: what the
//! `gpu_bcm2712` port takes over (`gpu survey`). The register sets are
//! the ones Linux's vc4 prints in debugfs (`hvs_regs`, `crtc*_regs`,
//! `hdmi*_regs`, `hvs_dlists`), so every read here is one Linux makes too.

use pi5_core::rpi_fw as fwid;
use pi5_core::vc4_regs::{self as r, HdmiKind};

use super::dt::{Fdt, Node};
use crate::hw::Mmio;
use crate::kprintln;

/// Where `gpu survey` keeps its last run, beside the boot log.
const SURVEY_PATH: &str = "sys/log/gpu-survey";

/// The survey's lines, printed and stored once it is done.
static OUT: spin::Mutex<alloc::string::String> = spin::Mutex::new(alloc::string::String::new());

macro_rules! out {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let mut o = OUT.lock();
        let _ = writeln!(o, $($arg)*);
    });
}

/// `gpu survey`: everything below; `gpu` alone says what there is.
pub fn intent_gpu(args: &str) {
    match args.trim() {
        "survey" => survey(),
        _ => kprintln!("  gpu survey   the display hardware as the firmware set it up (read only), also stored in {}", SURVEY_PATH),
    }
}

fn survey() {
    OUT.lock().clear();
    survey_all();
    let text = core::mem::take(&mut *OUT.lock());
    for l in text.lines() {
        kprintln!("{}", l);
    }
    match crate::npkfs::upsert(SURVEY_PATH, text.as_bytes(), crate::capability::CAP_NULL) {
        Ok(_) => kprintln!("  stored in {} ({} bytes)", SURVEY_PATH, text.len()),
        Err(e) => kprintln!("  not stored in {}: {:?}", SURVEY_PATH, e),
    }
}

fn survey_all() {
    let Some(fdt) = super::dt::boot() else {
        out!("  no device tree");
        return;
    };
    let hvs = fdt.find_compatible(b"brcm,bcm2712-hvs");
    match hvs {
        Some(n) if fdt.is_enabled(&n) => survey_hvs(&fdt, &n),
        Some(_) => out!("  HVS: disabled in the device tree (no dtoverlay=vc4-kms-v3d?)"),
        None => out!("  HVS: none in the device tree"),
    }
    for (i, compat) in [&b"brcm,bcm2712-pixelvalve0"[..], b"brcm,bcm2712-pixelvalve1"].iter().enumerate() {
        match fdt.find_compatible(compat).filter(|n| fdt.is_enabled(n)) {
            Some(n) => dump_window(&fdt, &n, 0, &alloc::format!("PV{}", i), r::PV),
            None => out!("  PV{}: none enabled", i),
        }
    }
    for (i, compat, fields) in [
        (0, &b"brcm,bcm2712-hdmi0"[..], r::HDMI0_FIELDS),
        (1, b"brcm,bcm2712-hdmi1", r::HDMI1_FIELDS),
    ] {
        match fdt.find_compatible(compat).filter(|n| fdt.is_enabled(n)) {
            Some(n) => survey_hdmi(&fdt, &n, i, fields),
            None => out!("  HDMI{}: none enabled", i),
        }
    }
    if let Some(n) = hvs {
        survey_iommu(&fdt, &n);
    }
    survey_clocks();
}

/// `vc6_hvs_regs` or `vc6_d_hvs_regs` by `SCALER6_VERSION`, as
/// `vc4_hvs_bind` picks the generation; then `vc6_hvs_debugfs_dlist`.
fn survey_hvs(fdt: &Fdt, node: &Node) {
    let Some((base, len)) = fdt.reg0(node) else { return };
    // SAFETY: the HVS node's register window, read only.
    let Ok(m) = (unsafe { Mmio::map(base, len) }) else {
        out!("  HVS: window {:#x}+{:#x} not mappable", base, len);
        return;
    };
    let version = r::SCALER6_VERSION_MASK & m.r32(r::SCALER6_VERSION as u64);
    let d = version == r::SCALER6_VERSION_D0;
    let cxm = m.r32(r::SCALER6_CXM_SIZE as u64);
    out!("  HVS: {:#x}+{:#x}, version {:#x} ({}), CXM {} words",
        base, len, version, if d { "D0" } else { "C" }, cxm);
    dump(&m, len, "HVS", if d { r::HVS6_D } else { r::HVS6_C });

    let status = if d {
        [r::SCALER6D_DISP0_STATUS, r::SCALER6D_DISP1_STATUS, r::SCALER6D_DISP2_STATUS]
    } else {
        [r::SCALER6_DISP0_STATUS, r::SCALER6_DISP1_STATUS, r::SCALER6_DISP2_STATUS]
    };
    let dl = if d {
        [r::SCALER6D_DISP0_DL, r::SCALER6D_DISP1_DL, r::SCALER6D_DISP2_DL]
    } else {
        [r::SCALER6_DISP0_DL, r::SCALER6_DISP1_DL, r::SCALER6_DISP2_DL]
    };
    // The dlist memory, bounded by the window as well.
    let words = (cxm as u64).min(len.saturating_sub(r::SCALER5_DLIST_START as u64) / 4) as u32;
    for i in 0..r::SCALER_CHANNELS_COUNT as usize {
        let mode = field(m.r32(status[i] as u64), r::SCALER6_DISPX_STATUS_MODE_MASK);
        if mode == r::SCALER6_DISPX_STATUS_MODE_DISABLED || mode == r::SCALER6_DISPX_STATUS_MODE_EOF {
            out!("  HVS chan {} disabled", i);
            continue;
        }
        out!("  HVS chan {}:", i);
        let active = field(m.r32(dl[i] as u64), r::SCALER6_DISPX_DL_LACT_MASK);
        let mut next_entry_start = 0;
        for j in active..words {
            let w = m.r32(r::SCALER5_DLIST_START as u64 + 4 * j as u64);
            out!("  dlist: {:02}: {:#010x}", j, w);
            if next_entry_start == 0 || next_entry_start == j {
                if w & r::SCALER_CTL0_END != 0 { break; }
                next_entry_start = j + field(w, r::SCALER_CTL0_SIZE_MASK);
            }
        }
    }
}

/// Every field of `fields` in the window of its kind (`reg-names`), as
/// `vc4_hdmi_build_regset` groups them.
fn survey_hdmi(fdt: &Fdt, node: &Node, port: usize, fields: &[(&str, HdmiKind, u32)]) {
    for kind in HdmiKind::ALL {
        let Some(idx) = fdt.prop_str_index(node, b"reg-names", kind.reg_name().as_bytes()) else {
            out!("  HDMI{} {}: no window", port, kind.reg_name());
            continue;
        };
        let set: alloc::vec::Vec<(&str, u32)> =
            fields.iter().filter(|f| f.1 == kind).map(|f| (f.0, f.2)).collect();
        dump_window(fdt, node, idx, &alloc::format!("HDMI{} {}", port, kind.reg_name()), &set);
    }
}

/// The HVS's IOMMU (its `iommus` phandle) and the shared TLB cache.
fn survey_iommu(fdt: &Fdt, hvs: &Node) {
    match fdt.prop_cell(hvs, b"iommus", 0).and_then(|ph| fdt.find_phandle(ph)) {
        Some(n) => dump_window(fdt, &n, 0, "IOMMU (HVS)", r::IOMMU),
        None => out!("  IOMMU: the HVS names none"),
    }
    match fdt.find_compatible(b"brcm,bcm2712-iommuc").filter(|n| fdt.is_enabled(n)) {
        // `bcm2712_iommu_cache_flush`'s control word at offset 0.
        Some(n) => dump_window(fdt, &n, 0, "IOMMU cache", &[("MMUC_CONTROL", 0)]),
        None => out!("  IOMMU cache: none enabled"),
    }
}

/// The firmware clocks the display and V3D run on (`clk-raspberrypi.c`).
fn survey_clocks() {
    for (name, id) in [
        ("core", fwid::RPI_FIRMWARE_CORE_CLK_ID),
        ("disp", fwid::RPI_FIRMWARE_DISP_CLK_ID),
        ("m2mc (HDMI HSM)", fwid::RPI_FIRMWARE_M2MC_CLK_ID),
        ("pixel_bvb", fwid::RPI_FIRMWARE_PIXEL_BVB_CLK_ID),
        ("v3d", fwid::RPI_FIRMWARE_V3D_CLK_ID),
    ] {
        // Asked before printing: `kprintln!` holds the console lock while
        // it formats, and a firmware request waits on an interrupt.
        let state = super::rpi_fw::clock_state(id);
        let (rate, measured) = (super::rpi_fw::clock_rate(id), super::rpi_fw::clock_measured(id));
        let (min, max) = (super::rpi_fw::clock_min_rate(id), super::rpi_fw::clock_max_rate(id));
        let mhz = |v: Option<u32>| v.map_or(alloc::string::String::from("-"), |v| alloc::format!("{}", v / 1_000_000));
        out!("  clock {} ({}): state {}, rate {} MHz, measured {}, min {}, max {}",
            id, name,
            state.map_or(alloc::string::String::from("-"), |s| alloc::format!("{:#x}", s)),
            mhz(rate), mhz(measured), mhz(min), mhz(max));
    }
}

/// `reg` entry `idx` of `node`, then `set` in it.
fn dump_window(fdt: &Fdt, node: &Node, idx: usize, label: &str, set: &[(&str, u32)]) {
    let mut win = None;
    let mut i = 0;
    fdt.reg(node, |a, s| {
        if i == idx { win = Some((a, s)); }
        i += 1;
    });
    let Some((base, len)) = win else {
        out!("  {}: no reg entry {}", label, idx);
        return;
    };
    // SAFETY: a `reg` window of a display node, read only.
    let Ok(m) = (unsafe { Mmio::map(base, len) }) else {
        out!("  {}: window {:#x}+{:#x} not mappable", label, base, len);
        return;
    };
    out!("  {}: {:#x}+{:#x}", label, base, len);
    dump(&m, len, label, set);
}

/// `drm_print_regset32`: `name = value` per register inside the window.
fn dump(m: &Mmio, len: u64, label: &str, set: &[(&str, u32)]) {
    for &(name, off) in set {
        if off as u64 + 4 > len {
            out!("  {} {:>32} = outside the window ({:#x})", label, name, off);
            continue;
        }
        out!("  {} {:>32} = {:#010x}", label, name, m.r32(off as u64));
    }
}

/// `FIELD_GET`.
fn field(v: u32, mask: u32) -> u32 {
    (v & mask) >> mask.trailing_zeros()
}
