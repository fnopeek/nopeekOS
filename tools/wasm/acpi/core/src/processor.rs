//! The ACPI processor objects as Linux drives them for idle states:
//! `acpi_early_processor_osc` (drivers/acpi/acpi_processor.c) hands every
//! present processor the OS's capabilities through `_OSC`, which is where
//! firmware loads its C-state tables, and `acpi_processor_evaluate_cst`
//! reads them back from `_CST`.

use crate::interp::Machine;
use crate::value::{obj, path_str, seg, Path, Value};
use crate::Namespace;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// `ACPI_PROCESSOR_DEVICE_HID`.
const ACPI_PROCESSOR_DEVICE_HID: &str = "ACPI0007";

/// The processor `_OSC` UUID of an Intel platform (`sb_uuid_str`,
/// 4077A616-290C-47BE-9EBD-D87058713953) in its byte order.
const PROC_OSC_UUID: [u8; 16] = [0x16, 0xA6, 0x77, 0x40, 0x0C, 0x29, 0xBE, 0x47,
                                 0x9E, 0xBD, 0xD8, 0x70, 0x58, 0x71, 0x39, 0x53];

/// `ACPI_ADR_SPACE_FIXED_HARDWARE` and `ACPI_ADR_SPACE_SYSTEM_IO`.
pub const SPACE_FFH: u8 = 0x7F;
pub const SPACE_SYSTEM_IO: u8 = 0x01;

/// A processor object: its path, its ACPI processor id, and whether it is
/// declared as a device (`ACPI0007`, id from `_UID`) rather than with
/// `Processor`.
pub struct Processor {
    pub path: Path,
    pub acpi_id: u32,
    pub device: bool,
}

/// Every processor object of the namespace (`acpi_walk_namespace` over
/// `ACPI_TYPE_PROCESSOR`, then `acpi_get_devices(ACPI0007)`).
pub fn processors(m: &mut Machine, ns: &Namespace) -> Vec<Processor> {
    let mut out: Vec<Processor> = ns.processors.iter()
        .map(|(p, &id)| Processor { path: p.clone(), acpi_id: id as u32, device: false })
        .collect();
    for dev in crate::interp::devices_with_ids(ns) {
        if !m.device_ids(&dev).iter().any(|h| h == ACPI_PROCESSOR_DEVICE_HID) {
            continue;
        }
        if let Ok(uid) = m.eval_child(&dev, "_UID") {
            if let Value::Int(id) = uid {
                out.push(Processor { path: dev, acpi_id: id as u32, device: true });
            }
        }
    }
    out
}

/// `processor_physically_present` with `acpi_map_madt_entry`: the processor
/// id has an enabled local APIC (or x2APIC) in the MADT. `madt` is the whole
/// table. The `_MAT` of the object, which Linux asks first, is not read.
pub fn physically_present(madt: &[u8], p: &Processor) -> bool {
    let mut o = 44usize;
    while o + 2 <= madt.len() {
        let (t, len) = (madt[o], madt[o + 1] as usize);
        if len < 2 || o + len > madt.len() {
            break;
        }
        let e = &madt[o..o + len];
        match t {
            // ACPI_MADT_TYPE_LOCAL_APIC: processor_id, id, flags.
            0 if len >= 8 => {
                let flags = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
                if flags & 1 != 0 && e[2] as u32 == p.acpi_id {
                    return true;
                }
            }
            // ACPI_MADT_TYPE_LOCAL_X2APIC: id, flags, uid.
            9 if len >= 16 => {
                let flags = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
                let uid = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);
                if flags & 1 != 0 && uid == p.acpi_id && (p.device || p.acpi_id < 255) {
                    return true;
                }
            }
            _ => {}
        }
        o += len;
    }
    false
}

/// `acpi_processor_osc` / `acpi_run_osc`: `_OSC` with the processor UUID,
/// revision 1 and the capabilities in the support dword. Ok with the
/// returned support dword; Err when there is no `_OSC` or it refused.
pub fn run_osc(m: &mut Machine, p: &Path, caps: u32) -> Result<u32, String> {
    let mut osc = p.clone();
    osc.push(seg("_OSC"));
    if !m.has(&osc) {
        return Err(format!("{}: no _OSC", path_str(p)));
    }
    let mut capbuf = alloc::vec![0u8; 8];
    capbuf[4..8].copy_from_slice(&caps.to_le_bytes());
    let args = alloc::vec![obj(Value::Buffer(PROC_OSC_UUID.to_vec())), obj(Value::Int(1)),
                           obj(Value::Int(2)), obj(Value::Buffer(capbuf))];
    let out = m.call(&osc, args)?;
    let Value::Buffer(b) = out else { return Err(format!("{}: _OSC returned wrong type", path_str(p))) };
    if b.len() != 8 {
        return Err(format!("{}: _OSC returned wrong type", path_str(p)));
    }
    // Need to ignore the bit0 in result code
    let errors = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) & !1;
    if errors != 0 {
        return Err(format!("{}: _OSC errors {errors:#x}", path_str(p)));
    }
    Ok(u32::from_le_bytes([b[4], b[5], b[6], b[7]]))
}

/// `acpi_processor_set_pdc`: `_PDC` with revision 1, one dword, the
/// capabilities.
pub fn set_pdc(m: &mut Machine, p: &Path, caps: u32) -> Result<(), String> {
    let mut pdc = p.clone();
    pdc.push(seg("_PDC"));
    if !m.has(&pdc) {
        return Err(format!("{}: no _PDC", path_str(p)));
    }
    let mut buf = alloc::vec![0u8; 12];
    buf[0..4].copy_from_slice(&1u32.to_le_bytes());
    buf[4..8].copy_from_slice(&1u32.to_le_bytes());
    buf[8..12].copy_from_slice(&caps.to_le_bytes());
    m.call(&pdc, alloc::vec![obj(Value::Buffer(buf))]).map(|_| ())
}

/// One `_CST` entry as `acpi_processor_evaluate_cst` reads it: the
/// register (`struct acpi_power_register`), the type, latency (µs) and
/// power (mW).
#[derive(Clone, Copy, Debug, Default)]
pub struct Cx {
    pub kind: u32,
    pub latency: u32,
    pub power: u32,
    pub space_id: u8,
    pub bit_width: u8,
    pub bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

/// `_CST` of processor `p`: the entries that are well formed, skipped ones
/// named in the error list.
pub fn evaluate_cst(m: &mut Machine, p: &Path) -> Result<(Vec<Cx>, Vec<String>), String> {
    let mut cst = p.clone();
    cst.push(seg("_CST"));
    if !m.has(&cst) {
        return Err(format!("{}: No _CST", path_str(p)));
    }
    let v = m.value_of(&cst)?;
    // There must be at least 2 elements.
    let Value::Package(e) = v else { return Err(format!("{}: Invalid _CST output", path_str(p))) };
    if e.len() < 2 {
        return Err(format!("{}: Invalid _CST output", path_str(p)));
    }
    let count = e[0].borrow().as_int();
    // Validate the number of C-states.
    if count < 1 || count != e.len() as u64 - 1 {
        return Err(format!("{}: Inconsistent _CST data", path_str(p)));
    }
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for i in 1..=count as usize {
        let el = e[i].borrow().clone();
        let Value::Package(f) = el else {
            skipped.push(format!("_CST C{i} is not a package"));
            continue;
        };
        if f.len() != 4 {
            skipped.push(format!("_CST C{i} package count {} is not 4", f.len()));
            continue;
        }
        let reg = f[0].borrow().clone();
        let Value::Buffer(r) = reg else {
            skipped.push(format!("_CST C{i} element[0] is not a buffer"));
            continue;
        };
        // struct acpi_power_register: descriptor, length(2), space_id,
        // bit_width, bit_offset, access_size, address(8).
        if r.len() < 15 {
            skipped.push(format!("_CST C{i} register is {} bytes", r.len()));
            continue;
        }
        let ints: Vec<Option<u64>> = (1..4).map(|k| match &*f[k].borrow() {
            Value::Int(n) => Some(*n),
            _ => None,
        }).collect();
        let (Some(kind), Some(latency), Some(power)) = (ints[0], ints[1], ints[2]) else {
            skipped.push(format!("_CST C{i} element is not an integer"));
            continue;
        };
        let mut addr = [0u8; 8];
        addr.copy_from_slice(&r[7..15]);
        out.push(Cx {
            kind: kind as u32,
            latency: latency as u32,
            power: power as u32,
            space_id: r[3],
            bit_width: r[4],
            bit_offset: r[5],
            access_size: r[6],
            address: u64::from_le_bytes(addr),
        });
    }
    Ok((out, skipped))
}
