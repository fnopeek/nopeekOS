//! amdgpu/amdgpu_discovery.c (Linux 6.18.55): the IP discovery binary, read
//! from the top of VRAM, checked, and parsed into the IP versions and
//! register bases (`amdgpu_discovery_reg_base_init`).
//!
//! Renoir takes the default branch of `amdgpu_discovery_set_ip_blocks`, has
//! no discovery file (`amdgpu_discovery_get_fw_name` returns NULL for it),
//! so the binary comes from memory.

use crate::amdgpu_device::*;
use crate::amdgpu_device::{HWIP_MAX_INSTANCE, MAX_HWIP};
use crate::ip_blocks::*;
use crate::linux::{self, Line};
use crate::regs::*;
use crate::tables::{amdgpu_dpm, hw_id_map};
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
    Line::new().s("RCC_CONFIG_MEMSIZE: ").dec(raw as u64).s(" MB of VRAM (carve-out)").dbg();
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

/// `amdgpu_discovery_validate_ip`.
fn amdgpu_discovery_validate_ip(instance: u32, hw_id: u32) -> Result<(), i32> {
    if instance as usize >= HWIP_MAX_INSTANCE {
        Line::new().s("Unexpected instance_number (").dec(instance as u64).s(") from ip discovery blob").say();
        return Err(-22);
    }
    if hw_id >= HW_ID_MAX {
        Line::new().s("Unexpected hw_id (").dec(hw_id as u64).s(") from ip discovery blob").say();
        return Err(-22);
    }
    Ok(())
}

/// The offset of the IP discovery table in the binary.
fn ip_discovery_offset(bin: &[u8]) -> u32 {
    le16(bin, BHDR_TABLE_LIST + IP_DISCOVERY * TABLE_INFO_SIZE + TABLE_INFO_OFFSET) as u32
}

/// The offset of table `t` in the binary, 0 when absent.
fn table_offset(bin: &[u8], t: u32) -> u32 {
    le16(bin, BHDR_TABLE_LIST + t * TABLE_INFO_SIZE + TABLE_INFO_OFFSET) as u32
}

/// `amdgpu_discovery_reg_base_init`: every IP of every die, its version
/// and base addresses into `adev`. As in Linux, the VCN revision and the
/// base addresses are rewritten in the binary itself.
pub fn amdgpu_discovery_reg_base_init(adev: &mut amdgpu_device) -> Result<(), i32> {
    amdgpu_discovery_init(adev)?;
    let mut bin = core::mem::take(&mut adev.mman.discovery_bin);

    let mut wafl_ver = 0;
    adev.gfx.xcc_mask = 0;
    adev.sdma.sdma_mask = 0;
    adev.vcn.inst_mask = 0;
    adev.jpeg.inst_mask = 0;
    let ihdr = ip_discovery_offset(&bin);
    let num_dies = le16(&bin, ihdr + IHDR_NUM_DIES) as u32;
    let version = le16(&bin, ihdr + IHDR_VERSION) as u32;
    // `ihdr->base_addr_64_bit`: bit 0 of the byte after the die list.
    let base_addr_64_bit = bin[(ihdr + IHDR_FLAGS) as usize] & 1 != 0;
    Line::new().s("ip discovery binary v").dec(le16(&bin, BHDR_VERSION_MAJOR) as u64).s(".")
        .dec(le16(&bin, BHDR_VERSION_MINOR) as u64).s(", table v").dec(version as u64)
        .s(", ").dec(le16(&bin, BHDR_SIZE) as u64).s(" bytes, number of dies: ").dec(num_dies as u64).dbg();

    for i in 0..num_dies {
        let die_offset = le16(&bin, ihdr + IHDR_DIE_INFO + i * DIE_INFO_SIZE + DIE_INFO_OFFSET) as u32;
        let num_ips = le16(&bin, die_offset + DHDR_NUM_IPS) as u32;
        let mut ip_offset = die_offset + DIE_HEADER_SIZE;
        let die_id = le16(&bin, die_offset + DHDR_DIE_ID) as u32;
        if die_id != i {
            Line::new().s("invalid die id ").dec(die_id as u64).s(", expected ").dec(i as u64).say();
            adev.mman.discovery_bin = bin;
            return Err(-22);
        }
        Line::new().s("number of hardware IPs on die").dec(die_id as u64).s(": ").dec(num_ips as u64).dbg();

        for _ in 0..num_ips {
            let inst = bin[(ip_offset + IP_V4_INSTANCE) as usize] as u32;
            let hw_id = le16(&bin, ip_offset + IP_V4_HW_ID) as u32;
            let num_base_address = bin[(ip_offset + IP_V4_NUM_BASE) as usize] as u32;
            if amdgpu_discovery_validate_ip(inst, hw_id).is_ok() {
                let major = bin[(ip_offset + IP_V4_MAJOR) as usize] as u32;
                let minor = bin[(ip_offset + IP_V4_MINOR) as usize] as u32;
                let rev_at = (ip_offset + IP_V4_REVISION) as usize;
                let mut l = Line::new();
                l.s(hw_id_name(hw_id)).s("(").dec(hw_id as u64).s(") #").dec(inst as u64).s(" v")
                    .dec(major as u64).s(".").dec(minor as u64).s(".").dec(bin[rev_at] as u64);

                if hw_id == VCN_HWID {
                    // Bit [5:0]: original revision value
                    // Bit [7:6]: en/decode capability:
                    //     0b00 : VCN function normally
                    //     0b10 : encode is disabled
                    //     0b01 : decode is disabled
                    if adev.vcn.num_vcn_inst < AMDGPU_MAX_VCN_INSTANCES {
                        adev.vcn.inst[adev.vcn.num_vcn_inst as usize].vcn_config = bin[rev_at] as u32 & 0xc0;
                        adev.vcn.num_vcn_inst += 1;
                        adev.vcn.inst_mask |= 1 << inst;
                        adev.jpeg.inst_mask |= 1 << inst;
                    } else {
                        Line::new().s("Too many VCN instances: ").dec(adev.vcn.num_vcn_inst as u64 + 1).s(" vs ")
                            .dec(AMDGPU_MAX_VCN_INSTANCES as u64).say();
                    }
                    bin[rev_at] &= !0xc0;
                }
                if hw_id == SDMA0_HWID || hw_id == SDMA1_HWID || hw_id == SDMA2_HWID || hw_id == SDMA3_HWID {
                    if adev.sdma.num_instances < AMDGPU_MAX_SDMA_INSTANCES {
                        adev.sdma.num_instances += 1;
                        adev.sdma.sdma_mask |= 1 << inst;
                    } else {
                        Line::new().s("Too many SDMA instances: ").dec(adev.sdma.num_instances as u64 + 1).s(" vs ")
                            .dec(AMDGPU_MAX_SDMA_INSTANCES as u64).say();
                    }
                }
                if hw_id == VPE_HWID {
                    if adev.vpe.num_instances < AMDGPU_MAX_VPE_INSTANCES {
                        adev.vpe.num_instances += 1;
                    } else {
                        Line::new().s("Too many VPE instances: ").dec(adev.vpe.num_instances as u64 + 1).s(" vs ")
                            .dec(AMDGPU_MAX_VPE_INSTANCES as u64).say();
                    }
                }
                if hw_id == UMC_HWID {
                    adev.gmc.num_umc += 1;
                    adev.umc.node_inst_num += 1;
                }
                if hw_id == GC_HWID {
                    adev.gfx.xcc_mask |= 1 << inst;
                }
                let revision = bin[rev_at] as u32;
                if wafl_ver == 0 && hw_id == WAFLC_HWID {
                    wafl_ver = ip_version_full(major, minor, revision, 0, 0);
                }

                l.s(", ").dec(num_base_address as u64).s(" base");
                let base = ip_offset + IP_V4_BASE_ADDRESS;
                let mut bases = alloc::vec::Vec::new();
                for k in 0..num_base_address {
                    // Truncate a 64-bit base to its low 32 bits and the
                    // dword address bits; the binary keeps the result.
                    let b = if base_addr_64_bit {
                        le32(&bin, base + k * 8) & 0x3FFF_FFFF
                    } else {
                        le32(&bin, base + k * 4)
                    };
                    let at = (base + k * 4) as usize;
                    bin[at..at + 4].copy_from_slice(&b.to_le_bytes());
                    bases.push(b);
                    l.s(" ").hex(b);
                }
                l.dbg();

                let last = bin[rev_at + 1] as u32;
                for hw_ip in 0..MAX_HWIP {
                    let mapped = hw_id_map[hw_ip] as u32;
                    if mapped == hw_id && mapped != 0 {
                        adev.reg_offset[hw_ip][inst as usize] = bases.clone();
                        let (subrev, variant) = if version < 3 { (0, 0) } else { (last & 0xf, last >> 4) };
                        adev.ip_versions[hw_ip][inst as usize] =
                            ip_version_full(major, minor, revision, variant, subrev);
                    }
                }
            }
            ip_offset += IP_V4_BASE_ADDRESS + num_base_address * if base_addr_64_bit { 8 } else { 4 };
        }
    }

    if wafl_ver != 0 && adev.ip_versions[XGMI_HWIP as usize][0] == 0 {
        adev.ip_versions[XGMI_HWIP as usize][0] = wafl_ver;
    }
    adev.mman.discovery_bin = bin;
    Ok(())
}

/// `amdgpu_discovery_read_harvest_bit_per_ip`.
fn amdgpu_discovery_read_harvest_bit_per_ip(adev: &mut amdgpu_device, vcn_harvest_count: &mut u32) {
    let bin = core::mem::take(&mut adev.mman.discovery_bin);
    let ihdr = ip_discovery_offset(&bin);
    let num_dies = le16(&bin, ihdr + IHDR_NUM_DIES) as u32;

    // scan harvest bit of all IP data structures
    for i in 0..num_dies {
        let die_offset = le16(&bin, ihdr + IHDR_DIE_INFO + i * DIE_INFO_SIZE + DIE_INFO_OFFSET) as u32;
        let num_ips = le16(&bin, die_offset + DHDR_NUM_IPS) as u32;
        let mut ip_offset = die_offset + DIE_HEADER_SIZE;

        for _ in 0..num_ips {
            let inst = bin[(ip_offset + IP_number_instance) as usize] as u32;
            let hw_id = le16(&bin, ip_offset + IP_hw_id) as u32;
            if amdgpu_discovery_validate_ip(inst, hw_id).is_ok() {
                let hdr = u64::from_le_bytes(bin[ip_offset as usize..ip_offset as usize + 8].try_into().unwrap_or([0; 8]));
                let harvest = (hdr & IP_HARVEST_MASK64) >> IP_HARVEST_MASK64.trailing_zeros();
                if harvest == 1 {
                    if hw_id == VCN_HWID {
                        *vcn_harvest_count += 1;
                        if inst == 0 {
                            adev.vcn.harvest_config |= AMDGPU_VCN_HARVEST_VCN0;
                            adev.vcn.inst_mask &= !AMDGPU_VCN_HARVEST_VCN0;
                            adev.jpeg.inst_mask &= !AMDGPU_VCN_HARVEST_VCN0;
                        } else {
                            adev.vcn.harvest_config |= AMDGPU_VCN_HARVEST_VCN1;
                            adev.vcn.inst_mask &= !AMDGPU_VCN_HARVEST_VCN1;
                            adev.jpeg.inst_mask &= !AMDGPU_VCN_HARVEST_VCN1;
                        }
                    } else if hw_id == DMU_HWID {
                        adev.harvest_ip_mask |= AMD_HARVEST_IP_DMU_MASK;
                    }
                }
            }
            ip_offset += IP_SIZE + 4 * bin[(ip_offset + IP_num_base_address) as usize] as u32;
        }
    }
    adev.mman.discovery_bin = bin;
}

/// `amdgpu_discovery_read_from_harvest_table`.
fn amdgpu_discovery_read_from_harvest_table(adev: &mut amdgpu_device, vcn_harvest_count: &mut u32,
                                            umc_harvest_count: &mut u32) {
    let bin = core::mem::take(&mut adev.mman.discovery_bin);
    let offset = table_offset(&bin, HARVEST_INFO);
    let mut umc_harvest_config = 0u32;

    if offset == 0 {
        Line::new().s("invalid harvest table offset").say();
        adev.mman.discovery_bin = bin;
        return;
    }

    for i in 0..32 {
        let e = offset + HARVEST_TABLE_list + i * HARVEST_INFO_SIZE;
        let hw_id = le16(&bin, e + HARVEST_INFO_hw_id) as u32;
        if hw_id == 0 {
            break;
        }
        let n = bin[(e + HARVEST_INFO_number_instance) as usize] as u32;
        if hw_id == VCN_HWID {
            *vcn_harvest_count += 1;
            adev.vcn.harvest_config |= 1 << n;
            adev.jpeg.harvest_config |= 1 << n;
            adev.vcn.inst_mask &= !(1u32 << n);
            adev.jpeg.inst_mask &= !(1u32 << n);
        } else if hw_id == DMU_HWID {
            adev.harvest_ip_mask |= AMD_HARVEST_IP_DMU_MASK;
        } else if hw_id == UMC_HWID {
            umc_harvest_config |= 1 << n;
            *umc_harvest_count += 1;
        } else if hw_id == GC_HWID {
            adev.gfx.xcc_mask &= !(1u32 << n);
        } else if hw_id == SDMA0_HWID {
            adev.sdma.sdma_mask &= !(1u32 << n);
        }
    }

    adev.umc.active_mask = ((1u32 << adev.umc.node_inst_num).wrapping_sub(1)) & !umc_harvest_config;
    adev.mman.discovery_bin = bin;
}

/// `amdgpu_discovery_harvest_config_quirk`: only Navy Flounder (GC 10.3.2
/// with VCN1 3.0.1) has a bad VCN harvest table.
fn amdgpu_discovery_harvest_config_quirk(adev: &mut amdgpu_device) {
    if adev.ip_version(UVD_HWIP, 1) == ip_version_full(3, 0, 1, 0, 0)
        && adev.ip_version(GC_HWIP, 0) == ip_version_full(10, 3, 2, 0, 0)
    {
        if matches!(adev.revision, 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC7 | 0xCF | 0xDF) {
            adev.vcn.harvest_config |= AMDGPU_VCN_HARVEST_VCN1;
            adev.vcn.inst_mask &= !AMDGPU_VCN_HARVEST_VCN1;
        }
    }
}

/// `amdgpu_discovery_harvest_ip`.
pub fn amdgpu_discovery_harvest_ip(adev: &mut amdgpu_device) {
    let mut vcn_harvest_count = 0;
    let mut umc_harvest_count = 0;
    let ihdr_ver = le16(&adev.mman.discovery_bin, ip_discovery_offset(&adev.mman.discovery_bin) + IHDR_VERSION) as u32;

    // Harvest table does not fit Navi1x and legacy GPUs,
    // so read harvest bit per IP data structure to set
    // harvest configuration.
    if adev.ip_version(GC_HWIP, 0) < ip_version_full(10, 2, 0, 0, 0) && ihdr_ver <= 2 {
        if (adev.device == 0x731E && (adev.revision == 0xC6 || adev.revision == 0xC7))
            || (adev.device == 0x7340 && adev.revision == 0xC9)
            || (adev.device == 0x7360 && adev.revision == 0xC7)
        {
            amdgpu_discovery_read_harvest_bit_per_ip(adev, &mut vcn_harvest_count);
        }
    } else {
        amdgpu_discovery_read_from_harvest_table(adev, &mut vcn_harvest_count, &mut umc_harvest_count);
    }

    amdgpu_discovery_harvest_config_quirk(adev);

    if vcn_harvest_count == adev.vcn.num_vcn_inst {
        adev.harvest_ip_mask |= AMD_HARVEST_IP_VCN_MASK;
        adev.harvest_ip_mask |= AMD_HARVEST_IP_JPEG_MASK;
    }

    if umc_harvest_count < adev.gmc.num_umc {
        adev.gmc.num_umc -= umc_harvest_count;
    }
}

/// `amdgpu_discovery_get_gfx_info`.
pub fn amdgpu_discovery_get_gfx_info(adev: &mut amdgpu_device) -> i32 {
    let bin = &adev.mman.discovery_bin;
    if bin.is_empty() {
        Line::new().s("ip discovery uninitialized").say();
        return -22;
    }
    let offset = table_offset(bin, GC);
    if offset == 0 {
        return 0;
    }
    let f = |field: u32| le32(bin, offset + field);
    let c = &mut adev.gfx.config;
    let cu = &mut adev.gfx.cu_info;
    let major = le16(bin, offset + GC_INFO_V1_0_header_version_major);
    let minor = le16(bin, offset + GC_INFO_V1_0_header_version_minor);
    match major {
        1 => {
            c.max_shader_engines = f(GC_INFO_V1_0_gc_num_se);
            c.max_cu_per_sh = 2 * (f(GC_INFO_V1_0_gc_num_wgp0_per_sa) + f(GC_INFO_V1_0_gc_num_wgp1_per_sa));
            c.max_sh_per_se = f(GC_INFO_V1_0_gc_num_sa_per_se);
            c.max_backends_per_se = f(GC_INFO_V1_0_gc_num_rb_per_se);
            c.max_texture_channel_caches = f(GC_INFO_V1_0_gc_num_gl2c);
            c.max_gprs = f(GC_INFO_V1_0_gc_num_gprs);
            c.max_gs_threads = f(GC_INFO_V1_0_gc_num_max_gs_thds);
            c.gs_vgt_table_depth = f(GC_INFO_V1_0_gc_gs_table_depth);
            c.gs_prim_buffer_depth = f(GC_INFO_V1_0_gc_gsprim_buff_depth);
            c.double_offchip_lds_buf = f(GC_INFO_V1_0_gc_double_offchip_lds_buffer);
            cu.wave_front_size = f(GC_INFO_V1_0_gc_wave_size);
            cu.max_waves_per_simd = f(GC_INFO_V1_0_gc_max_waves_per_simd);
            cu.max_scratch_slots_per_cu = f(GC_INFO_V1_0_gc_max_scratch_slots_per_cu);
            cu.lds_size = f(GC_INFO_V1_0_gc_lds_size);
            c.num_sc_per_sh = f(GC_INFO_V1_0_gc_num_sc_per_se).checked_div(f(GC_INFO_V1_0_gc_num_sa_per_se)).unwrap_or(0);
            c.num_packer_per_sc = f(GC_INFO_V1_0_gc_num_packer_per_sc);
            if minor >= 1 {
                c.gc_num_tcp_per_sa = f(GC_INFO_V1_1_gc_num_tcp_per_sa);
                c.gc_num_sdp_interface = f(GC_INFO_V1_1_gc_num_sdp_interface);
                c.gc_num_tcps = f(GC_INFO_V1_1_gc_num_tcps);
            }
            if minor >= 2 {
                c.gc_num_tcp_per_wpg = f(GC_INFO_V1_2_gc_num_tcp_per_wpg);
                c.gc_tcp_l1_size = f(GC_INFO_V1_2_gc_tcp_l1_size);
                c.gc_num_sqc_per_wgp = f(GC_INFO_V1_2_gc_num_sqc_per_wgp);
                c.gc_l1_instruction_cache_size_per_sqc = f(GC_INFO_V1_2_gc_l1_instruction_cache_size_per_sqc);
                c.gc_l1_data_cache_size_per_sqc = f(GC_INFO_V1_2_gc_l1_data_cache_size_per_sqc);
                c.gc_gl1c_per_sa = f(GC_INFO_V1_2_gc_gl1c_per_sa);
                c.gc_gl1c_size_per_instance = f(GC_INFO_V1_2_gc_gl1c_size_per_instance);
                c.gc_gl2c_per_gpu = f(GC_INFO_V1_2_gc_gl2c_per_gpu);
            }
            if minor >= 3 {
                c.gc_tcp_size_per_cu = f(GC_INFO_V1_3_gc_tcp_size_per_cu);
                c.gc_tcp_cache_line_size = f(GC_INFO_V1_3_gc_tcp_cache_line_size);
                c.gc_instruction_cache_size_per_sqc = f(GC_INFO_V1_3_gc_instruction_cache_size_per_sqc);
                c.gc_instruction_cache_line_size = f(GC_INFO_V1_3_gc_instruction_cache_line_size);
                c.gc_scalar_data_cache_size_per_sqc = f(GC_INFO_V1_3_gc_scalar_data_cache_size_per_sqc);
                c.gc_scalar_data_cache_line_size = f(GC_INFO_V1_3_gc_scalar_data_cache_line_size);
                c.gc_tcc_size = f(GC_INFO_V1_3_gc_tcc_size);
                c.gc_tcc_cache_line_size = f(GC_INFO_V1_3_gc_tcc_cache_line_size);
            }
        }
        2 => {
            c.max_shader_engines = f(GC_INFO_V2_0_gc_num_se);
            c.max_cu_per_sh = f(GC_INFO_V2_0_gc_num_cu_per_sh);
            c.max_sh_per_se = f(GC_INFO_V2_0_gc_num_sh_per_se);
            c.max_backends_per_se = f(GC_INFO_V2_0_gc_num_rb_per_se);
            c.max_texture_channel_caches = f(GC_INFO_V2_0_gc_num_tccs);
            c.max_gprs = f(GC_INFO_V2_0_gc_num_gprs);
            c.max_gs_threads = f(GC_INFO_V2_0_gc_num_max_gs_thds);
            c.gs_vgt_table_depth = f(GC_INFO_V2_0_gc_gs_table_depth);
            c.gs_prim_buffer_depth = f(GC_INFO_V2_0_gc_gsprim_buff_depth);
            c.double_offchip_lds_buf = f(GC_INFO_V2_0_gc_double_offchip_lds_buffer);
            cu.wave_front_size = f(GC_INFO_V2_0_gc_wave_size);
            cu.max_waves_per_simd = f(GC_INFO_V2_0_gc_max_waves_per_simd);
            cu.max_scratch_slots_per_cu = f(GC_INFO_V2_0_gc_max_scratch_slots_per_cu);
            cu.lds_size = f(GC_INFO_V2_0_gc_lds_size);
            c.num_sc_per_sh = f(GC_INFO_V2_0_gc_num_sc_per_se).checked_div(f(GC_INFO_V2_0_gc_num_sh_per_se)).unwrap_or(0);
            c.num_packer_per_sc = f(GC_INFO_V2_0_gc_num_packer_per_sc);
            if le16(bin, offset + GC_INFO_V2_0_header_version_minor) == 1 {
                c.gc_num_tcp_per_sa = f(GC_INFO_V2_1_gc_num_tcp_per_sh);
                c.gc_tcp_size_per_cu = f(GC_INFO_V2_1_gc_tcp_size_per_cu);
                c.gc_num_sdp_interface = f(GC_INFO_V2_1_gc_num_sdp_interface); // per XCD
                c.gc_num_cu_per_sqc = f(GC_INFO_V2_1_gc_num_cu_per_sqc);
                c.gc_l1_instruction_cache_size_per_sqc = f(GC_INFO_V2_1_gc_instruction_cache_size_per_sqc);
                c.gc_l1_data_cache_size_per_sqc = f(GC_INFO_V2_1_gc_scalar_data_cache_size_per_sqc);
                c.gc_tcc_size = f(GC_INFO_V2_1_gc_tcc_size); // per XCD
            }
        }
        _ => {
            Line::new().s("Unhandled GC info table ").dec(major as u64).s(".").dec(minor as u64).say();
            return -22;
        }
    }
    0
}

/// `amdgpu_discovery_get_mall_info`.
pub fn amdgpu_discovery_get_mall_info(adev: &mut amdgpu_device) -> i32 {
    let bin = &adev.mman.discovery_bin;
    if bin.is_empty() {
        Line::new().s("ip discovery uninitialized").say();
        return -22;
    }
    let offset = table_offset(bin, MALL_INFO);
    if offset == 0 {
        return 0;
    }
    let major = le16(bin, offset + MALL_INFO_V1_0_header_version_major);
    match major {
        1 => {
            let mut mall_size = 0u64;
            let mall_size_per_umc = le32(bin, offset + MALL_INFO_V1_0_mall_size_per_m);
            let m_s_present = le32(bin, offset + MALL_INFO_V1_0_m_s_present);
            let half_use = le32(bin, offset + MALL_INFO_V1_0_m_half_use);
            for u in 0..adev.gmc.num_umc {
                if m_s_present & (1 << u) != 0 {
                    mall_size += (mall_size_per_umc.wrapping_mul(2)) as u64;
                } else if half_use & (1 << u) != 0 {
                    mall_size += (mall_size_per_umc / 2) as u64;
                } else {
                    mall_size += mall_size_per_umc as u64;
                }
            }
            adev.gmc.mall_size = mall_size;
            adev.gmc.m_half_use = half_use;
        }
        2 => {
            let mall_size_per_umc = le32(bin, offset + MALL_INFO_V2_0_mall_size_per_umc);
            adev.gmc.mall_size = mall_size_per_umc as u64 * adev.gmc.num_umc as u64;
        }
        _ => {
            Line::new().s("Unhandled MALL info table ").dec(major as u64).s(".")
                .dec(le16(bin, offset + MALL_INFO_V1_0_header_version_minor) as u64).say();
            return -22;
        }
    }
    0
}

/// `amdgpu_discovery_get_vcn_info`.
pub fn amdgpu_discovery_get_vcn_info(adev: &mut amdgpu_device) -> i32 {
    if adev.mman.discovery_bin.is_empty() {
        Line::new().s("ip discovery uninitialized").say();
        return -22;
    }
    // num_vcn_inst is currently limited to AMDGPU_MAX_VCN_INSTANCES
    // which is smaller than VCN_INFO_TABLE_MAX_NUM_INSTANCES
    // but that may change in the future with new GPUs so keep this
    // check for defensive purposes.
    if adev.vcn.num_vcn_inst > VCN_INFO_TABLE_MAX_NUM_INSTANCES {
        Line::new().s("invalid vcn instances").say();
        return -22;
    }
    let bin = &adev.mman.discovery_bin;
    let offset = table_offset(bin, VCN_INFO);
    if offset == 0 {
        return 0;
    }
    let major = le16(bin, offset + VCN_INFO_V1_0_header_version_major);
    match major {
        1 => {
            // num_vcn_inst is currently limited to AMDGPU_MAX_VCN_INSTANCES
            // so this won't overflow.
            for v in 0..adev.vcn.num_vcn_inst {
                let e = offset + VCN_INFO_V1_0_instance_info + v * VCN_INSTANCE_INFO_V1_0_SIZE;
                adev.vcn.inst[v as usize].vcn_codec_disable_mask =
                    le32(bin, e + VCN_INSTANCE_INFO_V1_0_fuse_data_all_bits);
            }
        }
        _ => {
            Line::new().s("Unhandled VCN info table ").dec(major as u64).s(".")
                .dec(le16(bin, offset + VCN_INFO_V1_0_header_version_minor) as u64).say();
            return -22;
        }
    }
    0
}

/// `IP_VERSION(mj, mn, rv)`.
const fn ip_version(mj: u32, mn: u32, rv: u32) -> u32 {
    (mj << 24) | (mn << 16) | (rv << 8)
}

/// One `dev_err` of the `amdgpu_discovery_set_*_ip_blocks` default branches.
fn failed_to_add(what: &str, hwip: &str, v: u32) -> i32 {
    Line::new().s("Failed to add ").s(what).s(" ip block(").s(hwip).s(":").hex(v).s(")").say();
    -22
}

/// `amdgpu_discovery_set_common_ip_blocks`.
fn amdgpu_discovery_set_common_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    match adev.ip_version(GC_HWIP, 0) {
        v if v == ip_version(9, 3, 0) => amdgpu_device_ip_block_add(adev, &vega10_common_ip_block),
        v => failed_to_add("common", "GC_HWIP", v),
    }
}

/// `amdgpu_discovery_set_gmc_ip_blocks`.
fn amdgpu_discovery_set_gmc_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    match adev.ip_version(GC_HWIP, 0) {
        v if v == ip_version(9, 3, 0) => amdgpu_device_ip_block_add(adev, &gmc_v9_0_ip_block),
        v => failed_to_add("gmc", "GC_HWIP", v),
    }
}

/// `amdgpu_discovery_set_ih_ip_blocks`.
fn amdgpu_discovery_set_ih_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    match adev.ip_version(OSSSYS_HWIP, 0) {
        v if v == ip_version(4, 3, 0) => amdgpu_device_ip_block_add(adev, &vega10_ih_ip_block),
        v => failed_to_add("ih", "OSSSYS_HWIP", v),
    }
}

/// `amdgpu_discovery_set_psp_ip_blocks`.
fn amdgpu_discovery_set_psp_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    match adev.ip_version(MP0_HWIP, 0) {
        v if v == ip_version(11, 0, 3) => amdgpu_device_ip_block_add(adev, &psp_v12_0_ip_block),
        v => failed_to_add("psp", "MP0_HWIP", v),
    }
}

/// `amdgpu_discovery_set_smu_ip_blocks`.
fn amdgpu_discovery_set_smu_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    match adev.ip_version(MP1_HWIP, 0) {
        v if v == ip_version(12, 0, 0) => amdgpu_device_ip_block_add(adev, &smu_v12_0_ip_block),
        v => failed_to_add("smu", "MP1_HWIP", v),
    }
}

/// `amdgpu_discovery_set_display_ip_blocks`.
fn amdgpu_discovery_set_display_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    // `amdgpu_vkms_ip_block` with `enable_virtual_display`, which is off.
    if adev.enable_virtual_display {
        return 0;
    }
    if !amdgpu_device_has_dc_support(adev) {
        return 0;
    }
    if adev.ip_version(DCE_HWIP, 0) != 0 {
        match adev.ip_version(DCE_HWIP, 0) {
            v if v == ip_version(2, 1, 0) => {
                // Bare metal: not amdgpu_discovery_set_sriov_display.
                amdgpu_device_ip_block_add(adev, &dm_ip_block)
            }
            v => failed_to_add("dm", "DCE_HWIP", v),
        }
    } else if adev.ip_version(DCI_HWIP, 0) != 0 {
        failed_to_add("dm", "DCI_HWIP", adev.ip_version(DCI_HWIP, 0))
    } else {
        0
    }
}

/// `amdgpu_discovery_set_gc_ip_blocks`.
fn amdgpu_discovery_set_gc_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    match adev.ip_version(GC_HWIP, 0) {
        v if v == ip_version(9, 3, 0) => amdgpu_device_ip_block_add(adev, &gfx_v9_0_ip_block),
        v => failed_to_add("gfx", "GC_HWIP", v),
    }
}

/// `amdgpu_discovery_set_sdma_ip_blocks`.
fn amdgpu_discovery_set_sdma_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    match adev.ip_version(SDMA0_HWIP, 0) {
        v if v == ip_version(4, 1, 2) => amdgpu_device_ip_block_add(adev, &sdma_v4_0_ip_block),
        v => failed_to_add("sdma", "SDMA0_HWIP", v),
    }
}

/// `amdgpu_discovery_set_mm_ip_blocks`. With VCE, Linux takes the UVD 7 /
/// VCE 4 blocks of Vega, which are not ported; Renoir has no VCE.
fn amdgpu_discovery_set_mm_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    if adev.ip_version(VCE_HWIP, 0) != 0 {
        Line::new().s("VCE ").hex(adev.ip_version(VCE_HWIP, 0)).s(": the UVD 7 / VCE 4 blocks are not ported").say();
        return -22;
    }
    match adev.ip_version(UVD_HWIP, 0) {
        v if v == ip_version(2, 2, 0) => {
            amdgpu_device_ip_block_add(adev, &vcn_v2_0_ip_block);
            // Bare metal: JPEG too.
            amdgpu_device_ip_block_add(adev, &jpeg_v2_0_ip_block);
            0
        }
        v => failed_to_add("vcn/jpeg", "UVD_HWIP", v),
    }
}

/// `amdgpu_discovery_set_mes_ip_blocks`: MES is GC 11 and later.
fn amdgpu_discovery_set_mes_ip_blocks(_adev: &mut amdgpu_device) -> i32 {
    0
}

/// `amdgpu_discovery_set_vpe_ip_blocks`: Renoir has no VPE.
fn amdgpu_discovery_set_vpe_ip_blocks(_adev: &mut amdgpu_device) -> i32 {
    0
}

/// `amdgpu_discovery_set_umsch_mm_ip_blocks`: VCN 4.0.5 and 4.0.6 only.
fn amdgpu_discovery_set_umsch_mm_ip_blocks(_adev: &mut amdgpu_device) -> i32 {
    0
}

/// `amdgpu_discovery_set_isp_ip_blocks`: ISP 4.1.x only.
fn amdgpu_discovery_set_isp_ip_blocks(_adev: &mut amdgpu_device) -> i32 {
    0
}

/// `amdgpu_discovery_set_ip_blocks`, the default branch Renoir takes.
/// Every `switch` keeps the case of Renoir's versions
/// (tools/gpu_amd_renoir.ipversions); another version reaches the default
/// branch, which for the blocks fails as in Linux.
pub fn amdgpu_discovery_set_ip_blocks(adev: &mut amdgpu_device) -> i32 {
    if let Err(r) = amdgpu_discovery_reg_base_init(adev) {
        Line::new().s("discovery failed: ").dec((-r) as u64).say();
        return r;
    }
    amdgpu_discovery_harvest_ip(adev);
    amdgpu_discovery_get_gfx_info(adev);
    amdgpu_discovery_get_mall_info(adev);
    amdgpu_discovery_get_vcn_info(adev);

    // amdgpu_discovery_init_soc_config: GC 9.4.3, 9.4.4 and 9.5.0 only.
    // amdgpu_discovery_sysfs_init: nopeekOS has no sysfs.

    match adev.ip_version(GC_HWIP, 0) {
        v if v == ip_version(9, 3, 0) => adev.family = AMDGPU_FAMILY_RV,
        _ => return -22,
    }
    if adev.ip_version(GC_HWIP, 0) == ip_version(9, 3, 0) {
        adev.flags |= AMD_IS_APU;
    }

    // set NBIO version
    if adev.ip_version(NBIO_HWIP, 0) == ip_version(2, 5, 0) {
        adev.nbio_funcs = Some(amdgpu_nbio_funcs::nbio_v7_0);
    }
    if adev.ip_version(HDP_HWIP, 0) == ip_version(4, 1, 2) {
        adev.hdp_funcs = Some(amdgpu_hdp_funcs::hdp_v4_0);
    }
    if adev.ip_version(DF_HWIP, 0) == ip_version(3, 5, 1) {
        adev.df_funcs = Some(amdgpu_df_funcs::df_v1_7);
    }
    if adev.ip_version(SMUIO_HWIP, 0) == ip_version(10, 0, 2) {
        adev.smuio_funcs = Some(amdgpu_smuio_funcs::smuio_v9_0);
    }
    // LSDMA: Renoir has none.

    let r = amdgpu_discovery_set_common_ip_blocks(adev);
    if r != 0 {
        return r;
    }
    let r = amdgpu_discovery_set_gmc_ip_blocks(adev);
    if r != 0 {
        return r;
    }
    // Bare metal: IH before PSP.
    let r = amdgpu_discovery_set_ih_ip_blocks(adev);
    if r != 0 {
        return r;
    }
    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let r = amdgpu_discovery_set_psp_ip_blocks(adev);
        if r != 0 {
            return r;
        }
    }
    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let r = amdgpu_discovery_set_smu_ip_blocks(adev);
        if r != 0 {
            return r;
        }
    }
    let r = amdgpu_discovery_set_display_ip_blocks(adev);
    if r != 0 {
        return r;
    }
    let r = amdgpu_discovery_set_gc_ip_blocks(adev);
    if r != 0 {
        return r;
    }
    let r = amdgpu_discovery_set_sdma_ip_blocks(adev);
    if r != 0 {
        return r;
    }
    // Bare metal, so `!amdgpu_sriov_vf` holds.
    if adev.firmware.load_type == AMDGPU_FW_LOAD_DIRECT
        || (adev.firmware.load_type == AMDGPU_FW_LOAD_RLC_BACKDOOR_AUTO && amdgpu_dpm[0] == 1)
    {
        let r = amdgpu_discovery_set_smu_ip_blocks(adev);
        if r != 0 {
            return r;
        }
    }
    for set in [amdgpu_discovery_set_mm_ip_blocks, amdgpu_discovery_set_mes_ip_blocks,
                amdgpu_discovery_set_vpe_ip_blocks, amdgpu_discovery_set_umsch_mm_ip_blocks,
                amdgpu_discovery_set_isp_ip_blocks] {
        let r = set(adev);
        if r != 0 {
            return r;
        }
    }
    0
}
