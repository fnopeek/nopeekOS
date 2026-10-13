//! display/intel_pch.c (Linux 6.18.55): which PCH the display talks to.
//!
//! Linux's `drm_WARN_ON`s compare the PCH against the display platform;
//! with the platform fixed to Tiger Lake they reduce to "is Tiger Lake in
//! this PCH's list", which is what `tgl_ok` records.

use crate::linux::{self, Line};
use crate::regs::*;

/// `intel_pch_type`: PCH type for a masked device id, and whether Tiger
/// Lake is among the platforms Linux expects with it.
fn intel_pch_type(id: u32) -> (i32, &'static str, bool) {
    match id {
        INTEL_PCH_IBX_DEVICE_ID_TYPE => (PCH_IBX as i32, "Ibex Peak PCH", false),
        INTEL_PCH_CPT_DEVICE_ID_TYPE => (PCH_CPT as i32, "CougarPoint PCH", false),
        INTEL_PCH_PPT_DEVICE_ID_TYPE => (PCH_CPT as i32, "PantherPoint PCH", false),
        INTEL_PCH_LPT_DEVICE_ID_TYPE => (PCH_LPT_H as i32, "LynxPoint PCH", false),
        INTEL_PCH_LPT_LP_DEVICE_ID_TYPE => (PCH_LPT_LP as i32, "LynxPoint LP PCH", false),
        INTEL_PCH_WPT_DEVICE_ID_TYPE => (PCH_LPT_H as i32, "WildcatPoint PCH", false),
        INTEL_PCH_WPT_LP_DEVICE_ID_TYPE => (PCH_LPT_LP as i32, "WildcatPoint LP PCH", false),
        INTEL_PCH_SPT_DEVICE_ID_TYPE => (PCH_SPT as i32, "SunrisePoint PCH", false),
        INTEL_PCH_SPT_LP_DEVICE_ID_TYPE => (PCH_SPT as i32, "SunrisePoint LP PCH", false),
        INTEL_PCH_KBP_DEVICE_ID_TYPE => (PCH_SPT as i32, "Kaby Lake PCH (KBP)", false),
        INTEL_PCH_CNP_DEVICE_ID_TYPE => (PCH_CNP as i32, "Cannon Lake PCH (CNP)", false),
        INTEL_PCH_CNP_LP_DEVICE_ID_TYPE => (PCH_CNP as i32, "Cannon Lake LP PCH (CNP-LP)", false),
        INTEL_PCH_CMP_DEVICE_ID_TYPE | INTEL_PCH_CMP2_DEVICE_ID_TYPE =>
            (PCH_CNP as i32, "Comet Lake PCH (CMP)", false),
        INTEL_PCH_CMP_V_DEVICE_ID_TYPE => (PCH_SPT as i32, "Comet Lake V PCH (CMP-V)", false),
        INTEL_PCH_ICP_DEVICE_ID_TYPE | INTEL_PCH_ICP2_DEVICE_ID_TYPE =>
            (PCH_ICP as i32, "Ice Lake PCH", false),
        INTEL_PCH_MCC_DEVICE_ID_TYPE => (PCH_TGP as i32, "Mule Creek Canyon PCH", false),
        INTEL_PCH_TGP_DEVICE_ID_TYPE | INTEL_PCH_TGP2_DEVICE_ID_TYPE =>
            (PCH_TGP as i32, "Tiger Lake LP PCH", true),
        INTEL_PCH_JSP_DEVICE_ID_TYPE => (PCH_ICP as i32, "Jasper Lake PCH", false),
        INTEL_PCH_ADP_DEVICE_ID_TYPE | INTEL_PCH_ADP2_DEVICE_ID_TYPE
        | INTEL_PCH_ADP3_DEVICE_ID_TYPE | INTEL_PCH_ADP4_DEVICE_ID_TYPE =>
            (PCH_ADP as i32, "Alder Lake PCH", false),
        _ => (PCH_NONE as i32, "", true),
    }
}

/// `intel_is_virt_pch`.
fn intel_is_virt_pch(id: u32, svendor: u32, sdevice: u32) -> bool {
    id == INTEL_PCH_P2X_DEVICE_ID_TYPE
        || id == INTEL_PCH_P3X_DEVICE_ID_TYPE
        || (id == INTEL_PCH_QEMU_DEVICE_ID_TYPE && svendor == PCI_SUBVENDOR_ID_REDHAT_QUMRANET
            && sdevice == PCI_SUBDEVICE_ID_QEMU)
}

/// `intel_virt_detect_pch` for a Tiger Lake display.
fn intel_virt_detect_pch() -> i32 {
    let id = INTEL_PCH_TGP_DEVICE_ID_TYPE;
    intel_pch_type(id).0
}

fn cfg(bdf: u32, off: u32) -> u32 {
    npk_sys::pci_read_config_at(bdf as i32, off as i32) as u32
}

/// `pci_get_class(PCI_CLASS_BRIDGE_ISA << 8, from)`: the next function
/// after `from` (bus order) whose 24-bit class matches.
fn next_isa_bridge(from: Option<u32>) -> Option<u32> {
    let want = PCI_CLASS_BRIDGE_ISA << 8;
    let start = from.map_or(0, |b| b + 1);
    let mut bdf = start;
    while bdf <= 0xffff {
        let func = bdf & 7;
        let id = cfg(bdf, 0x00);
        if id == 0xffff_ffff || id & 0xffff == 0xffff {
            // No function here; no device either if function 0 is absent.
            bdf = if func == 0 { bdf + 8 } else { bdf + 1 };
            continue;
        }
        if func == 0 && cfg(bdf, 0x0c) & 0x0080_0000 == 0 {
            // Single-function device: the other functions do not exist.
            if cfg(bdf, 0x08) >> 8 == want {
                return Some(bdf);
            }
            bdf += 8;
            continue;
        }
        if cfg(bdf, 0x08) >> 8 == want {
            return Some(bdf);
        }
        bdf += 1;
    }
    None
}

/// `intel_pch_detect`. No fake PCH: that is DG1/DG2/MTL/BMG/Xe2.
pub fn intel_pch_detect(has_display: bool) -> i32 {
    let mut pch_type = PCH_NONE as i32;
    let mut pch = None;
    let mut cur = None;
    while let Some(bdf) = next_isa_bridge(cur) {
        cur = Some(bdf);
        let id_reg = cfg(bdf, 0x00);
        if id_reg & 0xffff != PCI_VENDOR_ID_INTEL {
            continue;
        }
        let id = (id_reg >> 16) & INTEL_PCH_DEVICE_ID_MASK;
        let (t, name, tgl_ok) = intel_pch_type(id);
        if t != PCH_NONE as i32 {
            Line::new().s("Found ").s(name).say();
            if !tgl_ok {
                linux::say("warning: PCH does not belong to a Tiger Lake display");
            }
            pch_type = t;
            pch = Some(bdf);
            break;
        }
        let sub = cfg(bdf, 0x2c);
        if intel_is_virt_pch(id, sub & 0xffff, sub >> 16) {
            pch_type = intel_virt_detect_pch();
            pch = Some(bdf);
            break;
        }
    }
    if pch.is_some() && !has_display {
        pch_type = PCH_NOP;
    } else if pch.is_none() {
        // i915_run_as_guest() is false on bare metal.
        linux::say("No PCH found.");
    }
    pch_type
}
