//! amdgpu/amdgpu_discovery.c (Linux 6.18.55): the IP discovery binary, read
//! from the top of VRAM, checked, and parsed into the IP versions and
//! register bases (`amdgpu_discovery_reg_base_init`).
//!
//! Renoir takes the default branch of `amdgpu_discovery_set_ip_blocks`, has
//! no discovery file (`amdgpu_discovery_get_fw_name` returns NULL for it),
//! so the binary comes from memory.

use crate::amdgpu_device::{amdgpu_device, HWIP_MAX_INSTANCE, MAX_HWIP};
use crate::linux::{self, Line};
use crate::regs::*;
use crate::tables::hw_id_map;
use alloc::vec;

fn le16(b: &[u8], at: u32) -> u16 {
    let at = at as usize;
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: u32) -> u32 {
    let at = at as usize;
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// `amdgpu_discovery_calculate_checksum`.
fn amdgpu_discovery_calculate_checksum(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |c, &b| c.wrapping_add(b as u16))
}

/// `amdgpu_discovery_verify_checksum`.
fn amdgpu_discovery_verify_checksum(data: &[u8], expected: u16) -> bool {
    amdgpu_discovery_calculate_checksum(data) == expected
}

/// The bytes `[offset, offset + size)` of the binary, if they lie inside it.
fn span(bin: &[u8], offset: u32, size: u32) -> Option<&[u8]> {
    let end = (offset as usize).checked_add(size as usize)?;
    bin.get(offset as usize..end)
}

/// `amdgpu_discovery_read_binary_from_mem`.
fn amdgpu_discovery_read_binary_from_mem(adev: &amdgpu_device, binary: &mut [u8]) -> Result<(), i32> {
    // Up to two seconds for the IFWI to finish; on an APU it has long since.
    let mut ready = false;
    for _ in 0..2000 {
        if adev.rreg32(mmMP0_SMN_C2PMSG_33).is_some_and(|m| m & 0x8000_0000 != 0) {
            ready = true;
            break;
        }
        npk_sys::sleep(1);
    }
    if !ready {
        Line::new().s("MP0_SMN_C2PMSG_33 never reported the IFWI done, reading anyway").say();
    }
    let raw = adev.rreg32(mmRCC_CONFIG_MEMSIZE).unwrap_or(0);
    let sz_valid = raw != 0 && raw != u32::MAX;
    if !sz_valid {
        // amdgpu_discovery_read_binary_from_sysmem: the TMR from ACPI
        // (amdgpu_acpi_get_tmr_info), not ported.
        Line::new().s("failed to read discovery info from memory, vram size read: ").hex(raw)
            .s(" (the ACPI TMR path is not ported)").say();
        return Err(-2);
    }
    let vram_size = (raw as u64) << 20;
    Line::new().s("RCC_CONFIG_MEMSIZE: ").dec(raw as u64).s(" MB of VRAM (carve-out)").say();
    let pos = vram_size - DISCOVERY_TMR_OFFSET as u64;
    adev.vram_access_read(pos, binary);
    Ok(())
}

/// `amdgpu_discovery_init`: read the binary and check its signatures and
/// checksums, table by table.
pub fn amdgpu_discovery_init(adev: &mut amdgpu_device) -> Result<(), i32> {
    adev.mman.discovery_tmr_size = DISCOVERY_TMR_SIZE;
    let mut bin = vec![0u8; DISCOVERY_TMR_SIZE as usize];
    amdgpu_discovery_read_binary_from_mem(adev, &mut bin)?;
    adev.mman.discovery_bin = bin;
    let bin = &adev.mman.discovery_bin;

    if le32(bin, BHDR_SIGNATURE) != BINARY_SIGNATURE {
        linux::drm_err("get invalid ip discovery binary signature");
        return Err(-22);
    }
    let offset = BHDR_CHECKSUM + 2;
    let size = (le16(bin, BHDR_SIZE) as u32).saturating_sub(offset);
    let checksum = le16(bin, BHDR_CHECKSUM);
    if !span(bin, offset, size).is_some_and(|d| amdgpu_discovery_verify_checksum(d, checksum)) {
        linux::drm_err("invalid ip discovery binary checksum");
        return Err(-22);
    }

    let info = |t: u32| BHDR_TABLE_LIST + t * TABLE_INFO_SIZE;
    let (offset, checksum) = (le16(bin, info(IP_DISCOVERY) + TABLE_INFO_OFFSET) as u32,
                              le16(bin, info(IP_DISCOVERY) + TABLE_INFO_CHECKSUM));
    if offset != 0 {
        if le32(bin, offset + IHDR_SIGNATURE) != DISCOVERY_TABLE_SIGNATURE {
            linux::drm_err("invalid ip discovery data table signature");
            return Err(-22);
        }
        let size = le16(bin, offset + IHDR_SIZE) as u32;
        if !span(bin, offset, size).is_some_and(|d| amdgpu_discovery_verify_checksum(d, checksum)) {
            linux::drm_err("invalid ip discovery data table checksum");
            return Err(-22);
        }
    }

    let (offset, checksum) = (le16(bin, info(GC) + TABLE_INFO_OFFSET) as u32, le16(bin, info(GC) + TABLE_INFO_CHECKSUM));
    if offset != 0 {
        if le32(bin, offset + GHDR_TABLE_ID) != GC_TABLE_ID {
            linux::drm_err("invalid ip discovery gc table id");
            return Err(-22);
        }
        let size = le32(bin, offset + GHDR_SIZE);
        if !span(bin, offset, size).is_some_and(|d| amdgpu_discovery_verify_checksum(d, checksum)) {
            linux::drm_err("invalid gc data table checksum");
            return Err(-22);
        }
    }

    let (offset, checksum) = (le16(bin, info(HARVEST_INFO) + TABLE_INFO_OFFSET) as u32,
                              le16(bin, info(HARVEST_INFO) + TABLE_INFO_CHECKSUM));
    if offset != 0 {
        if le32(bin, offset + HHDR_SIGNATURE) != HARVEST_TABLE_SIGNATURE {
            linux::drm_err("invalid ip discovery harvest table signature");
            return Err(-22);
        }
        if !span(bin, offset, HARVEST_TABLE_SIZE).is_some_and(|d| amdgpu_discovery_verify_checksum(d, checksum)) {
            linux::drm_err("invalid harvest data table checksum");
            return Err(-22);
        }
    }

    let (offset, checksum) = (le16(bin, info(VCN_INFO) + TABLE_INFO_OFFSET) as u32,
                              le16(bin, info(VCN_INFO) + TABLE_INFO_CHECKSUM));
    if offset != 0 {
        if le32(bin, offset + VHDR_TABLE_ID) != VCN_INFO_TABLE_ID {
            linux::drm_err("invalid ip discovery vcn table id");
            return Err(-22);
        }
        let size = le32(bin, offset + VHDR_SIZE_BYTES);
        if !span(bin, offset, size).is_some_and(|d| amdgpu_discovery_verify_checksum(d, checksum)) {
            linux::drm_err("invalid vcn data table checksum");
            return Err(-22);
        }
    }
    // MALL_INFO is not checked in Linux (`if (0 && offset)`), NPS_INFO only
    // on chips with NPS.
    Ok(())
}

/// The name of a discovery hardware id (`hw_id_names`).
fn hw_id_name(id: u32) -> &'static str {
    const NAMES: &[(u32, &str)] = &[
        (MP1_HWID, "MP1"), (MP2_HWID, "MP2"), (THM_HWID, "THM"), (SMUIO_HWID, "SMUIO"), (FUSE_HWID, "FUSE"),
        (CLKA_HWID, "CLKA"), (PWR_HWID, "PWR"), (GC_HWID, "GC"), (UVD_HWID, "UVD"), (AUDIO_AZ_HWID, "AUDIO_AZ"),
        (ACP_HWID, "ACP"), (DCI_HWID, "DCI"), (DMU_HWID, "DMU"), (DCO_HWID, "DCO"), (DIO_HWID, "DIO"),
        (XDMA_HWID, "XDMA"), (DCEAZ_HWID, "DCEAZ"), (DAZ_HWID, "DAZ"), (SDPMUX_HWID, "SDPMUX"), (NTB_HWID, "NTB"),
        (IOHC_HWID, "IOHC"), (L2IMU_HWID, "L2IMU"), (VCE_HWID, "VCE"), (MMHUB_HWID, "MMHUB"), (ATHUB_HWID, "ATHUB"),
        (DBGU_NBIO_HWID, "DBGU_NBIO"), (DFX_HWID, "DFX"), (DBGU0_HWID, "DBGU0"), (DBGU1_HWID, "DBGU1"),
        (OSSSYS_HWID, "OSSSYS"), (HDP_HWID, "HDP"), (SDMA0_HWID, "SDMA0"), (SDMA1_HWID, "SDMA1"), (ISP_HWID, "ISP"),
        (DBGU_IO_HWID, "DBGU_IO"), (DF_HWID, "DF"), (CLKB_HWID, "CLKB"), (FCH_HWID, "FCH"), (DFX_DAP_HWID, "DFX_DAP"),
        (L1IMU_PCIE_HWID, "L1IMU_PCIE"), (L1IMU_NBIF_HWID, "L1IMU_NBIF"), (L1IMU_IOAGR_HWID, "L1IMU_IOAGR"),
        (WAFLC_HWID, "WAFLC"), (FCH_USB_PD_HWID, "FCH_USB_PD"), (PCIE_HWID, "PCIE"), (PCS_HWID, "PCS"),
        (DDCL_HWID, "DDCL"), (SST_HWID, "SST"), (IOAGR_HWID, "IOAGR"), (NBIF_HWID, "NBIF"), (IOAPIC_HWID, "IOAPIC"),
        (SYSTEMHUB_HWID, "SYSTEMHUB"), (NTBCCP_HWID, "NTBCCP"), (UMC_HWID, "UMC"), (SATA_HWID, "SATA"),
        (USB_HWID, "USB"), (CCXSEC_HWID, "CCXSEC"), (XGMI_HWID, "XGMI"), (XGBE_HWID, "XGBE"), (MP0_HWID, "MP0"),
    ];
    NAMES.iter().find(|n| n.0 == id).map_or("?", |n| n.1)
}

/// `IP_VERSION_FULL(mj, mn, rv, var, srev)` (amdgpu.h).
pub fn ip_version_full(mj: u32, mn: u32, rv: u32, var: u32, srev: u32) -> u32 {
    (mj << 24) | (mn << 16) | (rv << 8) | (var << 4) | srev
}

/// `amdgpu_discovery_reg_base_init`: every IP of every die, its version
/// and base addresses into `adev`, as Linux's DRM_DEBUG lines print them.
pub fn amdgpu_discovery_reg_base_init(adev: &mut amdgpu_device) -> Result<(), i32> {
    amdgpu_discovery_init(adev)?;
    let bin = core::mem::take(&mut adev.mman.discovery_bin);
    let ihdr = le16(&bin, BHDR_TABLE_LIST + IP_DISCOVERY * TABLE_INFO_SIZE + TABLE_INFO_OFFSET) as u32;
    let num_dies = le16(&bin, ihdr + IHDR_NUM_DIES) as u32;
    let version = le16(&bin, ihdr + IHDR_VERSION) as u32;
    // `ihdr->base_addr_64_bit`: bit 0 of the byte after the die list (the
    // union with version <= 3's padding, which Linux reads the same way).
    let base_addr_64_bit = bin[(ihdr + IHDR_FLAGS) as usize] & 1 != 0;
    Line::new().s("ip discovery binary v").dec(le16(&bin, BHDR_VERSION_MAJOR) as u64).s(".")
        .dec(le16(&bin, BHDR_VERSION_MINOR) as u64).s(", table v").dec(version as u64)
        .s(", ").dec(le16(&bin, BHDR_SIZE) as u64).s(" bytes, number of dies: ").dec(num_dies as u64).say();
    let mut wafl_ver = 0;
    for i in 0..num_dies {
        let die_offset = le16(&bin, ihdr + IHDR_DIE_INFO + i * DIE_INFO_SIZE + DIE_INFO_OFFSET) as u32;
        let num_ips = le16(&bin, die_offset + DHDR_NUM_IPS) as u32;
        let mut ip_offset = die_offset + DIE_HEADER_SIZE;
        if le16(&bin, die_offset + DHDR_DIE_ID) as u32 != i {
            Line::new().s("invalid die id ").dec(le16(&bin, die_offset + DHDR_DIE_ID) as u64).s(", expected ")
                .dec(i as u64).say();
            return Err(-22);
        }
        Line::new().s("number of hardware IPs on die").dec(i as u64).s(": ").dec(num_ips as u64).say();
        for _ in 0..num_ips {
            let inst = bin[(ip_offset + IP_V4_INSTANCE) as usize] as u32;
            let hw_id = le16(&bin, ip_offset + IP_V4_HW_ID) as u32;
            let num_base_address = bin[(ip_offset + IP_V4_NUM_BASE) as usize] as u32;
            let major = bin[(ip_offset + IP_V4_MAJOR) as usize] as u32;
            let minor = bin[(ip_offset + IP_V4_MINOR) as usize] as u32;
            let mut revision = bin[(ip_offset + IP_V4_REVISION) as usize] as u32;
            // The byte after `revision`: harvest (v1/v2) or sub_revision | variant << 4.
            let last = bin[(ip_offset + IP_V4_REVISION + 1) as usize] as u32;
            let stride = if base_addr_64_bit { 8 } else { 4 };
            // amdgpu_discovery_validate_ip
            let valid = (inst as usize) < HWIP_MAX_INSTANCE && hw_id < HW_ID_MAX;
            if valid {
                let mut l = Line::new();
                l.s(hw_id_name(hw_id)).s("(").dec(hw_id as u64).s(") #").dec(inst as u64).s(" v")
                    .dec(major as u64).s(".").dec(minor as u64).s(".").dec((revision & !0xc0) as u64);
                if hw_id == VCN_HWID {
                    l.s(" vcn_config ").hex(revision & 0xc0);
                    if adev.num_vcn_inst < AMDGPU_MAX_VCN_INSTANCES {
                        adev.num_vcn_inst += 1;
                        adev.vcn_inst_mask |= 1 << inst;
                        adev.jpeg_inst_mask |= 1 << inst;
                    }
                    revision &= !0xc0;
                }
                if hw_id == SDMA0_HWID || hw_id == SDMA1_HWID || hw_id == SDMA2_HWID || hw_id == SDMA3_HWID {
                    if adev.sdma_num_instances < AMDGPU_MAX_SDMA_INSTANCES {
                        adev.sdma_num_instances += 1;
                        adev.sdma_mask |= 1 << inst;
                    }
                }
                if hw_id == VPE_HWID && adev.vpe_num_instances < AMDGPU_MAX_VPE_INSTANCES {
                    adev.vpe_num_instances += 1;
                }
                if hw_id == UMC_HWID {
                    adev.num_umc += 1;
                    adev.umc_node_inst_num += 1;
                }
                if hw_id == GC_HWID {
                    adev.gfx_xcc_mask |= 1 << inst;
                }
                if wafl_ver == 0 && hw_id == WAFLC_HWID {
                    wafl_ver = ip_version_full(major, minor, revision, 0, 0);
                }
                l.s(", ").dec(num_base_address as u64).s(" base");
                let mut bases = alloc::vec::Vec::new();
                for k in 0..num_base_address {
                    let at = ip_offset + IP_V4_BASE_ADDRESS + k * stride;
                    let b = if base_addr_64_bit { le32(&bin, at) & 0x3FFF_FFFF } else { le32(&bin, at) };
                    bases.push(b);
                    l.s(" ").hex(b);
                }
                l.say();
                let (subrev, variant) = if version < 3 { (0, 0) } else { (last & 0xf, last >> 4) };
                for hw_ip in 0..MAX_HWIP {
                    let mapped = hw_id_map.get(hw_ip).copied().unwrap_or(0) as u32;
                    if mapped == hw_id && mapped != 0 {
                        adev.reg_offset[hw_ip][inst as usize] = bases.clone();
                        adev.ip_versions[hw_ip][inst as usize] = ip_version_full(major, minor, revision, variant, subrev);
                    }
                }
            } else {
                Line::new().s("Unexpected instance_number (").dec(inst as u64).s(") or hw_id (").dec(hw_id as u64)
                    .s(") from ip discovery blob").say();
            }
            ip_offset += IP_V4_BASE_ADDRESS + num_base_address * stride;
        }
    }
    if wafl_ver != 0 && adev.ip_versions[XGMI_HWIP as usize][0] == 0 {
        adev.ip_versions[XGMI_HWIP as usize][0] = wafl_ver;
    }
    adev.mman.discovery_bin = bin;
    Ok(())
}
