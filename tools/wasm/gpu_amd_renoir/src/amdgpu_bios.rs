//! amdgpu/amdgpu_bios.c (Linux 6.18.55): the VBIOS of an APU from the ACPI
//! VFCT table, the first source `amdgpu_get_bios_apu` tries.

use crate::linux::Line;
use crate::regs::*;
use alloc::vec;
use alloc::vec::Vec;

fn le16(b: &[u8], at: u32) -> u16 {
    let at = at as usize;
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: u32) -> u32 {
    let at = at as usize;
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// `check_atom_bios`.
pub fn check_atom_bios(bios: &[u8]) -> bool {
    let size = bios.len();
    if size < 0x49 {
        Line::new().s("VBIOS mem is null or mem size is wrong").dbg();
        return false;
    }
    // AMD_IS_VALID_VBIOS
    if !(bios[0] == 0x55 && bios[1] == 0xAA) {
        Line::new().s("VBIOS signature incorrect ").hex(bios[0] as u32).s(" ").hex(bios[1] as u32).dbg();
        return false;
    }
    let bios_header_start = bios[0x48] as usize | (bios[0x49] as usize) << 8;
    if bios_header_start == 0 {
        Line::new().s("Can't locate VBIOS header").dbg();
        return false;
    }
    let tmp = bios_header_start + 4;
    if size < tmp + 4 {
        Line::new().s("VBIOS header is broken").dbg();
        return false;
    }
    &bios[tmp..tmp + 4] == b"ATOM" || &bios[tmp..tmp + 4] == b"MOTA"
}

/// `amdgpu_acpi_vfct_match`: the image is this card's.
fn amdgpu_acpi_vfct_match(t: &[u8], vhdr: u32, vendor: u16, device: u16, bus: u8, slot: u8, func: u8) -> bool {
    if le16(t, vhdr + VHDR_VENDOR_ID) != vendor || le16(t, vhdr + VHDR_DEVICE_ID) != device {
        return false;
    }
    if le32(t, vhdr + VHDR_PCI_DEVICE) != slot as u32 || le32(t, vhdr + VHDR_PCI_FUNCTION) != func as u32 {
        return false;
    }
    if le32(t, vhdr + VHDR_PCI_BUS) != bus as u32 {
        Line::new().s("VFCT bus number mismatch: table ").dec(le32(t, vhdr + VHDR_PCI_BUS) as u64).s(" != runtime ")
            .dec(bus as u64).s(", matching by device identity").say();
    }
    true
}

/// `amdgpu_acpi_vfct_bios`: the VBIOS image of this card, checked.
pub fn amdgpu_acpi_vfct_bios(vendor: u16, device: u16, bus: u8, slot: u8, func: u8) -> Option<Vec<u8>> {
    // acpi_get_table("VFCT", 1, &hdr): the first one.
    let need = npk_sys::acpi_table(*b"VFCT", 0, &mut []);
    if need <= 0 {
        return None;
    }
    let mut t = vec![0u8; need as usize];
    if npk_sys::acpi_table(*b"VFCT", 0, &mut t) != need {
        return None;
    }
    let tbl_size = le32(&t, ACPI_TABLE_HEADER_LENGTH).min(t.len() as u32);
    if tbl_size < VFCT_SIZE {
        Line::new().s("ACPI VFCT table present but broken (too short #1),skipping").say();
        return None;
    }
    let mut offset = le32(&t, VFCT_VBIOS_IMAGE_OFFSET);
    while offset < tbl_size {
        let vhdr = offset;
        offset += VFCT_IMAGE_HEADER_SIZE;
        if offset > tbl_size {
            Line::new().s("ACPI VFCT image header truncated,skipping").say();
            return None;
        }
        let len = le32(&t, vhdr + VHDR_IMAGE_LENGTH);
        offset = offset.saturating_add(len);
        if offset > tbl_size {
            Line::new().s("ACPI VFCT image truncated,skipping").say();
            return None;
        }
        if len != 0 && amdgpu_acpi_vfct_match(&t, vhdr, vendor, device, bus, slot, func) {
            let start = (vhdr + GOP_VBIOS_CONTENT_OFFSET) as usize;
            let bios = t[start..start + len as usize].to_vec();
            if !check_atom_bios(&bios) {
                return None;
            }
            return Some(bios);
        }
    }
    Line::new().s("ACPI VFCT table present but broken (too short #2),skipping").say();
    None
}

/// `amdgpu_get_bios_apu`: VFCT first. The VRAM BAR, ROM BAR and platform
/// sources after it are not ported; an APU whose firmware has no VFCT stops
/// here.
fn amdgpu_get_bios_apu(adev: &mut crate::amdgpu_device::amdgpu_device, bus: u8, slot: u8, func: u8) -> bool {
    if let Some(bios) = amdgpu_acpi_vfct_bios(0x1002, adev.device, bus, slot, func) {
        Line::new().s("Fetched VBIOS from VFCT").say();
        adev.bios = bios;
        return true;
    }
    Line::new().s("Unable to locate a BIOS ROM (VFCT only; VRAM BAR, ROM BAR and platform are not ported)").say();
    false
}

/// `amdgpu_get_bios`: an APU, so `amdgpu_get_bios_apu`; CHIP_RENOIR is past
/// CHIP_VEGA10, so the image is atomfirmware.
pub fn amdgpu_get_bios(adev: &mut crate::amdgpu_device::amdgpu_device, bus: u8, slot: u8, func: u8) -> bool {
    let found = amdgpu_get_bios_apu(adev, bus, slot, func);
    if found {
        adev.is_atom_fw = true;
    }
    found
}
