//! amdgpu/vcn_v2_0.c and amdgpu_vcn.c (Linux 6.18.55): the VCN block's
//! early_init, which requests the VCN image, and its sw_init.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::{amdgpu_ucode_ip_version_decode, amdgpu_ucode_request};
use crate::regs::*;
use crate::linux::Line;
use crate::amdgpu_ring::{amdgpu_irq_src_id, amdgpu_ring, amdgpu_ring_init, ring_funcs};

/// `amdgpu_vcn_early_init`, instance 0.
pub fn amdgpu_vcn_early_init(adev: &mut amdgpu_device, i: usize) -> i32 {
    adev.vcn.inst[i].inst = i as u32;
    let ucode_prefix = amdgpu_ucode_ip_version_decode(adev, UVD_HWIP);
    if adev.vcn.inst[0].fw.is_some() {
        return 0;
    }
    let (fw, r) = amdgpu_ucode_request(&alloc::format!("amdgpu/{}.bin", ucode_prefix));
    adev.vcn.inst[0].fw = if r == 0 { fw } else { None };
    r
}

/// `vcn_v2_0_early_init`. Bare metal: two encode rings.
pub fn vcn_v2_0_early_init(adev: &mut amdgpu_device) -> i32 {
    let v = &mut adev.vcn.inst[0];
    v.num_enc_rings = 2;
    v.set_pg_state = Some("vcn_v2_0_set_pg_state");
    // vcn_v2_0_set_dec_ring_funcs
    v.ring_dec_funcs = Some("vcn_v2_0_dec_ring_vm_funcs");
    // vcn_v2_0_set_enc_ring_funcs
    for i in 0..v.num_enc_rings as usize {
        v.ring_enc_funcs[i] = Some("vcn_v2_0_enc_ring_vm_funcs");
    }
    // vcn_v2_0_set_irq_funcs
    v.irq.num_types = v.num_enc_rings + 1;
    v.irq.funcs = Some("vcn_v2_0_irq_funcs");

    amdgpu_vcn_early_init(adev, 0)
}

/// `amdgpu_vcn_sw_init`. VCN 2.0: the pre-VCN4 `amdgpu_fw_shared`, and no
/// Steam Deck quirk.
pub fn amdgpu_vcn_sw_init(adev: &mut amdgpu_device, i: usize) -> i32 {
    let psp = adev.firmware.load_type == AMDGPU_FW_LOAD_PSP;
    if psp && adev.pg_flags & AMD_PG_SUPPORT_VCN_DPG != 0 {
        adev.vcn.inst[i].indirect_sram = true;
    }
    // from vcn4 and above, only unified queue is used
    adev.vcn.inst[i].using_unified_queue = adev.ip_version(UVD_HWIP, 0) >= 4 << 24;

    let Some(fw) = adev.vcn.inst[i].fw.as_ref() else { return -22 };
    let ucode_version = crate::amdgpu_ucode::le32(fw, UCODE_HDR_UCODE_VERSION);
    let ucode_size_bytes = crate::amdgpu_ucode::le32(fw, UCODE_HDR_UCODE_SIZE_BYTES);
    adev.vcn.inst[i].fw_version = ucode_version;
    adev.vcn.fw_version = ucode_version;

    // Bit 20-23, it is encode major and non-zero for new naming convention.
    let fw_check = (ucode_version >> 20) & 0xf;
    if fw_check != 0 {
        let fw_rev = ucode_version & 0xfff;
        let enc_minor = (ucode_version >> 12) & 0xff;
        let enc_major = fw_check;
        let dec_ver = (ucode_version >> 24) & 0xf;
        let vep = (ucode_version >> 28) & 0xf;
        Line::new().s("[VCN instance ").dec(i as u64).s("] Found VCN firmware Version ENC: ").dec(enc_major as u64)
            .s(".").dec(enc_minor as u64).s(" DEC: ").dec(dec_ver as u64).s(" VEP: ").dec(vep as u64)
            .s(" Revision: ").dec(fw_rev as u64).say();
    } else {
        let family_id = ucode_version & 0xff;
        let version_major = (ucode_version >> 24) & 0xff;
        let version_minor = (ucode_version >> 8) & 0xff;
        Line::new().s("[VCN instance ").dec(i as u64).s("] Found VCN firmware Version: ").dec(version_major as u64)
            .s(".").dec(version_minor as u64).s(" Family ID: ").dec(family_id as u64).say();
    }

    let mut bo_size = (AMDGPU_VCN_STACK_SIZE + AMDGPU_VCN_CONTEXT_SIZE) as u64;
    if !psp {
        bo_size += crate::linux::align(ucode_size_bytes as u64 + 8, AMDGPU_GPU_PAGE_SIZE as u64);
    }
    let fw_shared_size = crate::linux::align(SIZEOF_amdgpu_fw_shared as u64, AMDGPU_GPU_PAGE_SIZE as u64);
    bo_size += fw_shared_size;
    if crate::tables::amdgpu_vcnfw_log[0] != 0 {
        Line::new().s("amdgpu_vcnfw_log: the firmware log is not ported").say();
        return -22;
    }

    let both = AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT;
    let h = match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, bo_size, crate::amdgpu_object::PAGE_SIZE, both, true) {
        Ok(Some(h)) => h,
        Ok(None) => return -12,
        Err(r) => {
            Line::new().s("(").dec((-r) as u64).s(") failed to allocate vcn bo").say();
            return r;
        }
    };
    let gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
    let cpu_addr = crate::amdgpu_object::bo(adev, h).kptr;
    let v = &mut adev.vcn.inst[i];
    v.vcpu_bo = Some(h);
    v.gpu_addr = gpu_addr;
    v.cpu_addr = cpu_addr;
    v.fw_shared.cpu_off = bo_size - fw_shared_size;
    v.fw_shared.gpu_addr = gpu_addr + bo_size - fw_shared_size;
    v.fw_shared.mem_size = fw_shared_size as u32;

    if adev.vcn.inst[i].indirect_sram {
        match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, 64 * 2 * 4, crate::amdgpu_object::PAGE_SIZE, both, true) {
            Ok(Some(h)) => {
                let a = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
                let p = crate::amdgpu_object::bo(adev, h).kptr;
                let v = &mut adev.vcn.inst[i];
                (v.dpg_sram_bo, v.dpg_sram_gpu_addr, v.dpg_sram_cpu_addr) = (Some(h), a, p);
            }
            Ok(None) => return -12,
            Err(r) => {
                Line::new().s("VCN ").dec(i as u64).s(" (").dec((-r) as u64).s(") failed to allocate DPG bo").say();
                return r;
            }
        }
    }
    0
}

/// `amdgpu_vcn_setup_ucode`.
pub fn amdgpu_vcn_setup_ucode(adev: &mut amdgpu_device, i: usize) {
    if adev.firmware.load_type != AMDGPU_FW_LOAD_PSP {
        return;
    }
    if adev.vcn.harvest_config & (1 << i) != 0 {
        return;
    }
    let Some(fw) = adev.vcn.inst[i].fw.as_ref() else { return };
    // currently only support 2 FW instances
    if i >= 2 {
        Line::new().s("More then 2 VCN FW instances!").say();
        return;
    }
    let size = crate::amdgpu_ucode::le32(fw, UCODE_HDR_UCODE_SIZE_BYTES);
    let idx = AMDGPU_UCODE_ID_VCN + i as u32;
    let info = &mut adev.firmware.ucode[idx as usize];
    info.ucode_id = idx;
    info.fw = Some(crate::amdgpu_ucode::fw_source::vcn(i as u32));
    adev.firmware.fw_size = adev.firmware.fw_size
        .wrapping_add(crate::linux::align(size as u64, crate::amdgpu_object::PAGE_SIZE) as u32);
}

/// `amdgpu_vcn_resume`: nothing saved yet, and with PSP loading the
/// firmware is not copied, so the whole buffer is cleared.
pub fn amdgpu_vcn_resume(adev: &mut amdgpu_device, i: usize) -> i32 {
    if adev.vcn.harvest_config & (1 << i) != 0 {
        return 0;
    }
    let Some(h) = adev.vcn.inst[i].vcpu_bo else { return -22 };
    let mut size = crate::amdgpu_object::amdgpu_bo_size(adev, h);
    let Some(k) = adev.vcn.inst[i].cpu_addr else { return -22 };
    let mut ptr = 0u64;
    if adev.firmware.load_type != AMDGPU_FW_LOAD_PSP {
        let Some(fw) = adev.vcn.inst[i].fw.as_ref() else { return -22 };
        let offset = crate::amdgpu_ucode::le32(fw, UCODE_HDR_UCODE_ARRAY_OFFSET_BYTES) as usize;
        let n = crate::amdgpu_ucode::le32(fw, UCODE_HDR_UCODE_SIZE_BYTES) as usize;
        let image = fw.data.get(offset..offset + n).unwrap_or(&[]).to_vec();
        crate::amdgpu_gart::kptr_write(adev, k, 0, &image);
        size -= n as u64;
        ptr += n as u64;
    }
    crate::amdgpu_gart::kptr_fill32(adev, k, ptr, size, 0);
    0
}

/// `SOC15_REG_OFFSET(UVD, 0, reg)`.
fn uvd_reg(adev: &amdgpu_device, reg: u32, base_idx: u32) -> u32 {
    adev.reg_offset[UVD_HWIP as usize][0].get(base_idx as usize).copied().unwrap_or(0) + reg
}

/// `vcn_v2_0_sw_init`, bare metal.
pub fn vcn_v2_0_sw_init(adev: &mut amdgpu_device) -> i32 {
    let irq = adev.vcn.inst[0].irq;
    // VCN DEC TRAP
    let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_VCN,
        VCN_2_0__SRCID__UVD_SYSTEM_MESSAGE_INTERRUPT, &irq, "vcn.inst[0].irq");
    if r != 0 {
        return r;
    }
    // VCN ENC TRAP
    for i in 0..adev.vcn.inst[0].num_enc_rings {
        let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_VCN,
            i + VCN_2_0__SRCID__UVD_ENC_GENERAL_PURPOSE, &irq, "vcn.inst[0].irq");
        if r != 0 {
            return r;
        }
    }

    let r = amdgpu_vcn_sw_init(adev, 0);
    if r != 0 {
        return r;
    }
    amdgpu_vcn_setup_ucode(adev, 0);
    let r = amdgpu_vcn_resume(adev, 0);
    if r != 0 {
        return r;
    }

    let Some(funcs) = ring_funcs(adev.vcn.inst[0].ring_dec_funcs) else { return -22 };
    let mut ring = amdgpu_ring::new(funcs, alloc::string::String::from("vcn_dec"));
    ring.use_doorbell = true;
    ring.doorbell_index = adev.doorbell_index.vcn_ring0_1 << 1;
    ring.vm_hub = AMDGPU_MMHUB0_0_;
    match amdgpu_ring_init(adev, ring, 512, amdgpu_irq_src_id::vcn(0), 0, AMDGPU_RING_PRIO_DEFAULT) {
        Ok(k) => adev.vcn.inst[0].ring_dec = Some(k),
        Err(r) => return r,
    }

    let external = crate::amdgpu_device::amdgpu_vcn_reg {
        scratch9: uvd_reg(adev, mmUVD_SCRATCH9, mmUVD_SCRATCH9_BASE_IDX),
        data0: uvd_reg(adev, mmUVD_GPCOM_VCPU_DATA0, mmUVD_GPCOM_VCPU_DATA0_BASE_IDX),
        data1: uvd_reg(adev, mmUVD_GPCOM_VCPU_DATA1, mmUVD_GPCOM_VCPU_DATA1_BASE_IDX),
        cmd: uvd_reg(adev, mmUVD_GPCOM_VCPU_CMD, mmUVD_GPCOM_VCPU_CMD_BASE_IDX),
        nop: uvd_reg(adev, mmUVD_NO_OP, mmUVD_NO_OP_BASE_IDX),
        ..Default::default()
    };
    let v = &mut adev.vcn.inst[0];
    v.internal = crate::amdgpu_device::amdgpu_vcn_reg {
        context_id: mmUVD_CONTEXT_ID_INTERNAL_OFFSET,
        ib_vmid: mmUVD_LMI_RBC_IB_VMID_INTERNAL_OFFSET,
        ib_bar_low: mmUVD_LMI_RBC_IB_64BIT_BAR_LOW_INTERNAL_OFFSET,
        ib_bar_high: mmUVD_LMI_RBC_IB_64BIT_BAR_HIGH_INTERNAL_OFFSET,
        ib_size: mmUVD_RBC_IB_SIZE_INTERNAL_OFFSET,
        gp_scratch8: mmUVD_GP_SCRATCH8_INTERNAL_OFFSET,
        scratch9: mmUVD_SCRATCH9_INTERNAL_OFFSET,
        data0: mmUVD_GPCOM_VCPU_DATA0_INTERNAL_OFFSET,
        data1: mmUVD_GPCOM_VCPU_DATA1_INTERNAL_OFFSET,
        cmd: mmUVD_GPCOM_VCPU_CMD_INTERNAL_OFFSET,
        nop: mmUVD_NO_OP_INTERNAL_OFFSET,
    };
    v.external = external;

    for i in 0..adev.vcn.inst[0].num_enc_rings as usize {
        // amdgpu_vcn_get_enc_ring_prio
        let hw_prio = match i {
            1 => AMDGPU_RING_PRIO_1,
            2 => AMDGPU_RING_PRIO_2,
            _ => AMDGPU_RING_PRIO_0,
        };
        let Some(funcs) = ring_funcs(adev.vcn.inst[0].ring_enc_funcs[i]) else { return -22 };
        let mut ring = amdgpu_ring::new(funcs, alloc::format!("vcn_enc{}", i));
        ring.use_doorbell = true;
        ring.vm_hub = AMDGPU_MMHUB0_0_;
        ring.doorbell_index = (adev.doorbell_index.vcn_ring0_1 << 1) + 2 + i as u32;
        match amdgpu_ring_init(adev, ring, 512, amdgpu_irq_src_id::vcn(0), 0, hw_prio) {
            Ok(k) => adev.vcn.inst[0].ring_enc[i] = Some(k),
            Err(r) => return r,
        }
    }

    // pause_dpg_mode and reset: vcn_v2_0_pause_dpg_mode, vcn_v2_0_reset.
    let enc0 = adev.vcn.inst[0].ring_enc[0];
    adev.vcn.supported_reset = crate::amdgpu_ring::amdgpu_get_soft_full_reset_mask(adev, enc0);
    adev.vcn.supported_reset |= AMDGPU_RESET_TYPE_PER_QUEUE;
    // amdgpu_virt_alloc_mm_table: bare metal, nothing.

    let v = &adev.vcn.inst[0];
    if let Some(k) = v.cpu_addr {
        let at = v.fw_shared.cpu_off + FW_SHARED_PRESENT_FLAG_0 as u64;
        crate::amdgpu_gart::kptr_w32(adev, k, at, AMDGPU_VCN_MULTI_QUEUE_FLAG);
    }

    // amdgpu_vcn_reg_dump_init
    adev.vcn.ip_dump = alloc::vec![0; (adev.vcn.num_vcn_inst * VCN_REG_LIST_2_0_COUNT) as usize];
    adev.vcn.reg_count = VCN_REG_LIST_2_0_COUNT;
    // amdgpu_vcn_sysfs_reset_mask_init: no sysfs.
    0
}

// ---- hw_init, the DPG start and stop, the rings (stage 6) ----

/// `msecs_to_jiffies(1000)` (`VCN_IDLE_TIMEOUT`), in microseconds.
const VCN_IDLE_TIMEOUT_US: u64 = 1_000_000;

/// `RREG32_SOC15(UVD, 0, reg)`.
fn rreg(adev: &amdgpu_device, reg: u32, base_idx: u32) -> u32 {
    adev.rreg32(uvd_reg(adev, reg, base_idx)).unwrap_or(0)
}

/// `WREG32_SOC15(UVD, 0, reg, v)`.
fn wreg(adev: &amdgpu_device, reg: u32, base_idx: u32, v: u32) {
    adev.wreg32(uvd_reg(adev, reg, base_idx), v);
}

/// `WREG32_P(reg, val, mask)`: the bits outside `mask` from `val`.
fn wreg_p(adev: &amdgpu_device, reg: u32, base_idx: u32, val: u32, mask: u32) {
    let mut tmp = rreg(adev, reg, base_idx);
    tmp &= mask;
    tmp |= val & !mask;
    wreg(adev, reg, base_idx, tmp);
}

/// `SOC15_WAIT_ON_RREG(UVD, 0, reg, expected, mask)`.
fn wait_on(adev: &amdgpu_device, reg: u32, base_idx: u32, name: &str, expected: u32, mask: u32) -> i32 {
    crate::amdgpu_device::amdgpu_device_wait_on_rreg(adev, 0, uvd_reg(adev, reg, base_idx), name, expected, mask)
}

/// `SOC15_DPG_MODE_OFFSET(UVD, 0, reg)`: the register as the DPG engine
/// names it.
fn dpg_offset(adev: &amdgpu_device, reg: u32, base_idx: u32) -> u32 {
    let addr = (uvd_reg(adev, reg, base_idx) << 2) & 0xFFFFF;
    let video = (VCN_VID_SOC_ADDRESS_2_0..VCN_VID_SOC_ADDRESS_2_0 + 0x2600).contains(&addr);
    let video1 = (VCN1_VID_SOC_ADDRESS_3_0..VCN1_VID_SOC_ADDRESS_3_0 + 0x2600).contains(&addr);
    let aon = (VCN_AON_SOC_ADDRESS_2_0..VCN_AON_SOC_ADDRESS_2_0 + 0x600).contains(&addr);
    let aon1 = (VCN1_AON_SOC_ADDRESS_3_0..VCN1_AON_SOC_ADDRESS_3_0 + 0x600).contains(&addr);
    let internal = if video {
        addr - VCN_VID_SOC_ADDRESS_2_0 + VCN_VID_IP_ADDRESS_2_0
    } else if aon {
        addr - VCN_AON_SOC_ADDRESS_2_0 + VCN_AON_IP_ADDRESS_2_0
    } else if video1 {
        addr - VCN1_VID_SOC_ADDRESS_3_0 + VCN_VID_IP_ADDRESS_2_0
    } else if aon1 {
        addr - VCN1_AON_SOC_ADDRESS_3_0 + VCN_AON_IP_ADDRESS_2_0
    } else {
        addr
    };
    internal >> 2
}

/// `WREG32_SOC15_DPG_MODE(0, offset, value, mask_en, indirect)`: through
/// the LMA window, or into the DPG SRAM the PSP loads.
fn wreg_dpg(adev: &mut amdgpu_device, offset: u32, value: u32, mask_en: u32, indirect: bool) {
    if !indirect {
        wreg(adev, mmUVD_DPG_LMA_DATA, mmUVD_DPG_LMA_DATA_BASE_IDX, value);
        wreg(adev, mmUVD_DPG_LMA_CTL, mmUVD_DPG_LMA_CTL_BASE_IDX,
             1 << UVD_DPG_LMA_CTL__READ_WRITE__SHIFT | mask_en << UVD_DPG_LMA_CTL__MASK_EN__SHIFT
                 | offset << UVD_DPG_LMA_CTL__READ_WRITE_ADDR__SHIFT);
    } else {
        let v = &mut adev.vcn.inst[0];
        let (Some(k), at) = (v.dpg_sram_cpu_addr, v.dpg_sram_curr) else { return };
        v.dpg_sram_curr += 8;
        crate::amdgpu_gart::kptr_w32(adev, k, at, offset);
        crate::amdgpu_gart::kptr_w32(adev, k, at + 4, value);
    }
}

/// A `uint8_t` field of `amdgpu_fw_shared` at byte `off`, as `f` changes it.
fn fw_shared_u8(adev: &amdgpu_device, off: u32, f: impl FnOnce(u8) -> u8) {
    let v = &adev.vcn.inst[0];
    let Some(k) = v.cpu_addr else { return };
    let at = v.fw_shared.cpu_off + off as u64;
    let word = at & !3;
    let shift = (at & 3) * 8;
    let w = crate::amdgpu_gart::kptr_r32(adev, k, word);
    let b = f((w >> shift) as u8);
    crate::amdgpu_gart::kptr_w32(adev, k, word, (w & !(0xff << shift)) | (b as u32) << shift);
}

/// `order_base_2`.
fn order_base_2(n: u32) -> u32 {
    if n > 1 { 32 - (n - 1).leading_zeros() } else { 0 }
}

/// `REG_SET_FIELD`.
fn set_field(v: u32, mask: u32, shift: u32, f: u32) -> u32 {
    (v & !mask) | (mask & (f << shift))
}

/// `vcn_v2_0_hw_init`: bare metal, so the rings' tests are all; they
/// start the block through `begin_use`.
pub fn vcn_v2_0_hw_init(adev: &mut amdgpu_device) -> i32 {
    let Some(dec) = adev.vcn.inst[0].ring_dec else { return -22 };
    let (use_db, db) = (adev.rings[dec].use_doorbell, adev.rings[dec].doorbell_index);
    crate::nbio_v7_0::nbio_v7_0_vcn_doorbell_range(adev, use_db, db, 0);
    let r = crate::amdgpu_ring::amdgpu_ring_test_helper(adev, dec);
    if r != 0 {
        return r;
    }
    for i in 0..adev.vcn.inst[0].num_enc_rings as usize {
        let Some(ring) = adev.vcn.inst[0].ring_enc[i] else { return -22 };
        let r = crate::amdgpu_ring::amdgpu_ring_test_helper(adev, ring);
        if r != 0 {
            return r;
        }
    }
    0
}

/// `vcn_v2_0_hw_fini`.
pub fn vcn_v2_0_hw_fini(adev: &mut amdgpu_device) -> i32 {
    crate::workqueue::cancel_delayed_work(adev, crate::workqueue::work::vcn_idle { inst: 0 });
    if adev.pg_flags & AMD_PG_SUPPORT_VCN_DPG != 0
        || (adev.vcn.inst[0].cur_state != AMD_PG_STATE_GATE && rreg(adev, mmUVD_STATUS, mmUVD_STATUS_BASE_IDX) != 0)
    {
        vcn_v2_0_set_pg_state(adev, AMD_PG_STATE_GATE);
    }
    0
}

/// `vcn_v2_0_suspend`: the VCPU buffer is not saved; `amdgpu_vcn_resume`
/// clears it again.
pub fn vcn_v2_0_suspend(adev: &mut amdgpu_device) -> i32 {
    vcn_v2_0_hw_fini(adev)
}

/// `vcn_v2_0_resume`.
pub fn vcn_v2_0_resume(adev: &mut amdgpu_device) -> i32 {
    let r = amdgpu_vcn_resume(adev, 0);
    if r != 0 {
        return r;
    }
    vcn_v2_0_hw_init(adev)
}

/// `vcn_v2_0_mc_resume_dpg_mode`: PSP loading, so window 0 is the TMR.
fn vcn_v2_0_mc_resume_dpg_mode(adev: &mut amdgpu_device, indirect: bool) {
    let fw_size = adev.vcn.inst[0].fw.as_ref().map_or(0, |f| f.data.len() as u64);
    let size = crate::linux::align(fw_size + 4, AMDGPU_GPU_PAGE_SIZE as u64) as u32;
    let gpu_addr = adev.vcn.inst[0].gpu_addr;
    let o = |adev: &amdgpu_device, reg: u32, idx: u32| dpg_offset(adev, reg, idx);
    if adev.firmware.load_type != AMDGPU_FW_LOAD_PSP {
        Line::new().s("vcn_v2_0_mc_resume_dpg_mode: an APU's VCN firmware is loaded by the PSP").say();
        return;
    }
    // cache window 0: fw
    if !indirect {
        let u = adev.firmware.ucode[AMDGPU_UCODE_ID_VCN as usize];
        let x = o(adev, mmUVD_LMI_VCPU_CACHE_64BIT_BAR_LOW, mmUVD_LMI_VCPU_CACHE_64BIT_BAR_LOW_BASE_IDX);
        wreg_dpg(adev, x, u.tmr_mc_addr_lo, 0, indirect);
        let x = o(adev, mmUVD_LMI_VCPU_CACHE_64BIT_BAR_HIGH, mmUVD_LMI_VCPU_CACHE_64BIT_BAR_HIGH_BASE_IDX);
        wreg_dpg(adev, x, u.tmr_mc_addr_hi, 0, indirect);
        let x = o(adev, mmUVD_VCPU_CACHE_OFFSET0, mmUVD_VCPU_CACHE_OFFSET0_BASE_IDX);
        wreg_dpg(adev, x, 0, 0, indirect);
    } else {
        let x = o(adev, mmUVD_LMI_VCPU_CACHE_64BIT_BAR_LOW, mmUVD_LMI_VCPU_CACHE_64BIT_BAR_LOW_BASE_IDX);
        wreg_dpg(adev, x, 0, 0, indirect);
        let x = o(adev, mmUVD_LMI_VCPU_CACHE_64BIT_BAR_HIGH, mmUVD_LMI_VCPU_CACHE_64BIT_BAR_HIGH_BASE_IDX);
        wreg_dpg(adev, x, 0, 0, indirect);
        let x = o(adev, mmUVD_VCPU_CACHE_OFFSET0, mmUVD_VCPU_CACHE_OFFSET0_BASE_IDX);
        wreg_dpg(adev, x, 0, 0, indirect);
    }
    let offset = 0u64;
    let x = o(adev, mmUVD_VCPU_CACHE_SIZE0, mmUVD_VCPU_CACHE_SIZE0_BASE_IDX);
    wreg_dpg(adev, x, if !indirect { size } else { 0 }, 0, indirect);

    // cache window 1: stack
    if !indirect {
        let x = o(adev, mmUVD_LMI_VCPU_CACHE1_64BIT_BAR_LOW, mmUVD_LMI_VCPU_CACHE1_64BIT_BAR_LOW_BASE_IDX);
        wreg_dpg(adev, x, (gpu_addr + offset) as u32, 0, indirect);
        let x = o(adev, mmUVD_LMI_VCPU_CACHE1_64BIT_BAR_HIGH, mmUVD_LMI_VCPU_CACHE1_64BIT_BAR_HIGH_BASE_IDX);
        wreg_dpg(adev, x, ((gpu_addr + offset) >> 32) as u32, 0, indirect);
        let x = o(adev, mmUVD_VCPU_CACHE_OFFSET1, mmUVD_VCPU_CACHE_OFFSET1_BASE_IDX);
        wreg_dpg(adev, x, 0, 0, indirect);
    } else {
        let x = o(adev, mmUVD_LMI_VCPU_CACHE1_64BIT_BAR_LOW, mmUVD_LMI_VCPU_CACHE1_64BIT_BAR_LOW_BASE_IDX);
        wreg_dpg(adev, x, 0, 0, indirect);
        let x = o(adev, mmUVD_LMI_VCPU_CACHE1_64BIT_BAR_HIGH, mmUVD_LMI_VCPU_CACHE1_64BIT_BAR_HIGH_BASE_IDX);
        wreg_dpg(adev, x, 0, 0, indirect);
        let x = o(adev, mmUVD_VCPU_CACHE_OFFSET1, mmUVD_VCPU_CACHE_OFFSET1_BASE_IDX);
        wreg_dpg(adev, x, 0, 0, indirect);
    }
    let x = o(adev, mmUVD_VCPU_CACHE_SIZE1, mmUVD_VCPU_CACHE_SIZE1_BASE_IDX);
    wreg_dpg(adev, x, AMDGPU_VCN_STACK_SIZE, 0, indirect);

    // cache window 2: context
    let ctx = gpu_addr + offset + AMDGPU_VCN_STACK_SIZE as u64;
    let x = o(adev, mmUVD_LMI_VCPU_CACHE2_64BIT_BAR_LOW, mmUVD_LMI_VCPU_CACHE2_64BIT_BAR_LOW_BASE_IDX);
    wreg_dpg(adev, x, ctx as u32, 0, indirect);
    let x = o(adev, mmUVD_LMI_VCPU_CACHE2_64BIT_BAR_HIGH, mmUVD_LMI_VCPU_CACHE2_64BIT_BAR_HIGH_BASE_IDX);
    wreg_dpg(adev, x, (ctx >> 32) as u32, 0, indirect);
    let x = o(adev, mmUVD_VCPU_CACHE_OFFSET2, mmUVD_VCPU_CACHE_OFFSET2_BASE_IDX);
    wreg_dpg(adev, x, 0, 0, indirect);
    let x = o(adev, mmUVD_VCPU_CACHE_SIZE2, mmUVD_VCPU_CACHE_SIZE2_BASE_IDX);
    wreg_dpg(adev, x, AMDGPU_VCN_CONTEXT_SIZE, 0, indirect);

    // non-cache window
    let shared = adev.vcn.inst[0].fw_shared.gpu_addr;
    let x = o(adev, mmUVD_LMI_VCPU_NC0_64BIT_BAR_LOW, mmUVD_LMI_VCPU_NC0_64BIT_BAR_LOW_BASE_IDX);
    wreg_dpg(adev, x, shared as u32, 0, indirect);
    let x = o(adev, mmUVD_LMI_VCPU_NC0_64BIT_BAR_HIGH, mmUVD_LMI_VCPU_NC0_64BIT_BAR_HIGH_BASE_IDX);
    wreg_dpg(adev, x, (shared >> 32) as u32, 0, indirect);
    let x = o(adev, mmUVD_VCPU_NONCACHE_OFFSET0, mmUVD_VCPU_NONCACHE_OFFSET0_BASE_IDX);
    wreg_dpg(adev, x, 0, 0, indirect);
    let x = o(adev, mmUVD_VCPU_NONCACHE_SIZE0, mmUVD_VCPU_NONCACHE_SIZE0_BASE_IDX);
    wreg_dpg(adev, x, crate::linux::align(SIZEOF_amdgpu_fw_shared as u64, AMDGPU_GPU_PAGE_SIZE as u64) as u32, 0,
             indirect);

    // VCN global tiling registers
    let x = o(adev, mmUVD_GFX10_ADDR_CONFIG, mmUVD_GFX10_ADDR_CONFIG_BASE_IDX);
    let cfg = adev.gfx.config.gb_addr_config;
    wreg_dpg(adev, x, cfg, 0, indirect);
}

/// `vcn_v2_0_clock_gating_dpg_mode`.
fn vcn_v2_0_clock_gating_dpg_mode(adev: &mut amdgpu_device, sram_sel: u32, indirect: bool) {
    // enable sw clock gating control
    let mut reg_data = if adev.cg_flags & AMD_CG_SUPPORT_VCN_MGCG as u64 != 0 {
        1 << UVD_CGC_CTRL__DYN_CLOCK_MODE__SHIFT
    } else {
        0 << UVD_CGC_CTRL__DYN_CLOCK_MODE__SHIFT
    };
    reg_data |= 1 << UVD_CGC_CTRL__CLK_GATE_DLY_TIMER__SHIFT;
    reg_data |= 4 << UVD_CGC_CTRL__CLK_OFF_DELAY__SHIFT;
    reg_data &= !(UVD_CGC_CTRL__UDEC_RE_MODE_MASK | UVD_CGC_CTRL__UDEC_CM_MODE_MASK | UVD_CGC_CTRL__UDEC_IT_MODE_MASK
        | UVD_CGC_CTRL__UDEC_DB_MODE_MASK | UVD_CGC_CTRL__UDEC_MP_MODE_MASK | UVD_CGC_CTRL__SYS_MODE_MASK
        | UVD_CGC_CTRL__UDEC_MODE_MASK | UVD_CGC_CTRL__MPEG2_MODE_MASK | UVD_CGC_CTRL__REGS_MODE_MASK
        | UVD_CGC_CTRL__RBC_MODE_MASK | UVD_CGC_CTRL__LMI_MC_MODE_MASK | UVD_CGC_CTRL__LMI_UMC_MODE_MASK
        | UVD_CGC_CTRL__IDCT_MODE_MASK | UVD_CGC_CTRL__MPRD_MODE_MASK | UVD_CGC_CTRL__MPC_MODE_MASK
        | UVD_CGC_CTRL__LBSI_MODE_MASK | UVD_CGC_CTRL__LRBBM_MODE_MASK | UVD_CGC_CTRL__WCB_MODE_MASK
        | UVD_CGC_CTRL__VCPU_MODE_MASK | UVD_CGC_CTRL__SCPU_MODE_MASK);
    let x = dpg_offset(adev, mmUVD_CGC_CTRL, mmUVD_CGC_CTRL_BASE_IDX);
    wreg_dpg(adev, x, reg_data, sram_sel, indirect);

    // turn off clock gating
    let x = dpg_offset(adev, mmUVD_CGC_GATE, mmUVD_CGC_GATE_BASE_IDX);
    wreg_dpg(adev, x, 0, sram_sel, indirect);

    // turn on SUVD clock gating
    let x = dpg_offset(adev, mmUVD_SUVD_CGC_GATE, mmUVD_SUVD_CGC_GATE_BASE_IDX);
    wreg_dpg(adev, x, 1, sram_sel, indirect);

    // turn on sw mode in UVD_SUVD_CGC_CTRL
    let x = dpg_offset(adev, mmUVD_SUVD_CGC_CTRL, mmUVD_SUVD_CGC_CTRL_BASE_IDX);
    wreg_dpg(adev, x, 0, sram_sel, indirect);
}

/// `vcn_v2_0_enable_static_power_gating`.
fn vcn_v2_0_enable_static_power_gating(adev: &amdgpu_device) {
    if adev.pg_flags & AMD_PG_SUPPORT_VCN == 0 {
        return;
    }
    // Before power off, this indicator has to be turned on
    let mut data = rreg(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX);
    data &= !UVD_POWER_STATUS__UVD_POWER_STATUS_MASK;
    data |= UVD_POWER_STATUS__UVD_POWER_STATUS_TILES_OFF;
    wreg(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, data);

    let data = 2 << UVD_PGFSM_CONFIG__UVDM_PWR_CONFIG__SHIFT | 2 << UVD_PGFSM_CONFIG__UVDU_PWR_CONFIG__SHIFT
        | 2 << UVD_PGFSM_CONFIG__UVDF_PWR_CONFIG__SHIFT | 2 << UVD_PGFSM_CONFIG__UVDC_PWR_CONFIG__SHIFT
        | 2 << UVD_PGFSM_CONFIG__UVDB_PWR_CONFIG__SHIFT | 2 << UVD_PGFSM_CONFIG__UVDIL_PWR_CONFIG__SHIFT
        | 2 << UVD_PGFSM_CONFIG__UVDIR_PWR_CONFIG__SHIFT | 2 << UVD_PGFSM_CONFIG__UVDTD_PWR_CONFIG__SHIFT
        | 2 << UVD_PGFSM_CONFIG__UVDTE_PWR_CONFIG__SHIFT | 2 << UVD_PGFSM_CONFIG__UVDE_PWR_CONFIG__SHIFT;
    wreg(adev, mmUVD_PGFSM_CONFIG, mmUVD_PGFSM_CONFIG_BASE_IDX, data);

    let data = 2 << UVD_PGFSM_STATUS__UVDM_PWR_STATUS__SHIFT | 2 << UVD_PGFSM_STATUS__UVDU_PWR_STATUS__SHIFT
        | 2 << UVD_PGFSM_STATUS__UVDF_PWR_STATUS__SHIFT | 2 << UVD_PGFSM_STATUS__UVDC_PWR_STATUS__SHIFT
        | 2 << UVD_PGFSM_STATUS__UVDB_PWR_STATUS__SHIFT | 2 << UVD_PGFSM_STATUS__UVDIL_PWR_STATUS__SHIFT
        | 2 << UVD_PGFSM_STATUS__UVDIR_PWR_STATUS__SHIFT | 2 << UVD_PGFSM_STATUS__UVDTD_PWR_STATUS__SHIFT
        | 2 << UVD_PGFSM_STATUS__UVDTE_PWR_STATUS__SHIFT | 2 << UVD_PGFSM_STATUS__UVDE_PWR_STATUS__SHIFT;
    wait_on(adev, mmUVD_PGFSM_STATUS, mmUVD_PGFSM_STATUS_BASE_IDX, "mmUVD_PGFSM_STATUS", data, 0xFFFFF);
}

/// `vcn_v2_0_start_dpg_mode`.
fn vcn_v2_0_start_dpg_mode(adev: &mut amdgpu_device, indirect: bool) -> i32 {
    let Some(dec) = adev.vcn.inst[0].ring_dec else { return -22 };
    vcn_v2_0_enable_static_power_gating(adev);

    // enable dynamic power gating mode
    let mut tmp = rreg(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX);
    tmp |= UVD_POWER_STATUS__UVD_PG_MODE_MASK;
    tmp |= UVD_POWER_STATUS__UVD_PG_EN_MASK;
    wreg(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, tmp);

    if indirect {
        adev.vcn.inst[0].dpg_sram_curr = 0;
    }

    // enable clock gating
    vcn_v2_0_clock_gating_dpg_mode(adev, 0, indirect);

    // enable VCPU clock
    let mut tmp = 0xFF << UVD_VCPU_CNTL__PRB_TIMEOUT_VAL__SHIFT;
    tmp |= UVD_VCPU_CNTL__CLK_EN_MASK;
    tmp |= UVD_VCPU_CNTL__MIF_WR_LOW_THRESHOLD_BP_MASK;
    let x = dpg_offset(adev, mmUVD_VCPU_CNTL, mmUVD_VCPU_CNTL_BASE_IDX);
    wreg_dpg(adev, x, tmp, 0, indirect);

    // disable master interupt
    let x = dpg_offset(adev, mmUVD_MASTINT_EN, mmUVD_MASTINT_EN_BASE_IDX);
    wreg_dpg(adev, x, 0, 0, indirect);

    // setup mmUVD_LMI_CTRL
    let tmp = UVD_LMI_CTRL__WRITE_CLEAN_TIMER_EN_MASK | UVD_LMI_CTRL__REQ_MODE_MASK | UVD_LMI_CTRL__CRC_RESET_MASK
        | UVD_LMI_CTRL__MASK_MC_URGENT_MASK | UVD_LMI_CTRL__DATA_COHERENCY_EN_MASK
        | UVD_LMI_CTRL__VCPU_DATA_COHERENCY_EN_MASK | 8 << UVD_LMI_CTRL__WRITE_CLEAN_TIMER__SHIFT | 0x00100000;
    let x = dpg_offset(adev, mmUVD_LMI_CTRL, mmUVD_LMI_CTRL_BASE_IDX);
    wreg_dpg(adev, x, tmp, 0, indirect);

    let x = dpg_offset(adev, mmUVD_MPC_CNTL, mmUVD_MPC_CNTL_BASE_IDX);
    wreg_dpg(adev, x, 0x2 << UVD_MPC_CNTL__REPLACEMENT_MODE__SHIFT, 0, indirect);

    let x = dpg_offset(adev, mmUVD_MPC_SET_MUXA0, mmUVD_MPC_SET_MUXA0_BASE_IDX);
    wreg_dpg(adev, x, 0x1 << UVD_MPC_SET_MUXA0__VARA_1__SHIFT | 0x2 << UVD_MPC_SET_MUXA0__VARA_2__SHIFT
        | 0x3 << UVD_MPC_SET_MUXA0__VARA_3__SHIFT | 0x4 << UVD_MPC_SET_MUXA0__VARA_4__SHIFT, 0, indirect);

    let x = dpg_offset(adev, mmUVD_MPC_SET_MUXB0, mmUVD_MPC_SET_MUXB0_BASE_IDX);
    wreg_dpg(adev, x, 0x1 << UVD_MPC_SET_MUXB0__VARB_1__SHIFT | 0x2 << UVD_MPC_SET_MUXB0__VARB_2__SHIFT
        | 0x3 << UVD_MPC_SET_MUXB0__VARB_3__SHIFT | 0x4 << UVD_MPC_SET_MUXB0__VARB_4__SHIFT, 0, indirect);

    let x = dpg_offset(adev, mmUVD_MPC_SET_MUX, mmUVD_MPC_SET_MUX_BASE_IDX);
    wreg_dpg(adev, x, 0x0 << UVD_MPC_SET_MUX__SET_0__SHIFT | 0x1 << UVD_MPC_SET_MUX__SET_1__SHIFT
        | 0x2 << UVD_MPC_SET_MUX__SET_2__SHIFT, 0, indirect);

    vcn_v2_0_mc_resume_dpg_mode(adev, indirect);

    let x = dpg_offset(adev, mmUVD_REG_XX_MASK, mmUVD_REG_XX_MASK_BASE_IDX);
    wreg_dpg(adev, x, 0x10, 0, indirect);
    let x = dpg_offset(adev, mmUVD_RBC_XX_IB_REG_CHECK, mmUVD_RBC_XX_IB_REG_CHECK_BASE_IDX);
    wreg_dpg(adev, x, 0x3, 0, indirect);

    // release VCPU reset to boot
    let x = dpg_offset(adev, mmUVD_SOFT_RESET, mmUVD_SOFT_RESET_BASE_IDX);
    wreg_dpg(adev, x, 0, 0, indirect);

    // enable LMI MC and UMC channels
    let x = dpg_offset(adev, mmUVD_LMI_CTRL2, mmUVD_LMI_CTRL2_BASE_IDX);
    wreg_dpg(adev, x, 0x1F << UVD_LMI_CTRL2__RE_OFLD_MIF_WR_REQ_NUM__SHIFT, 0, indirect);

    // enable master interrupt
    let x = dpg_offset(adev, mmUVD_MASTINT_EN, mmUVD_MASTINT_EN_BASE_IDX);
    wreg_dpg(adev, x, UVD_MASTINT_EN__VCPU_EN_MASK, 0, indirect);

    if indirect {
        let r = amdgpu_vcn_psp_update_sram(adev, 0, 0);
        if r != 0 {
            Line::new().s("vcn sram load failed ").dec((-r) as u64).say();
            return r;
        }
    }

    // force RBC into idle state
    let rb_bufsz = order_base_2(adev.rings[dec].ring_size);
    let mut tmp = set_field(0, UVD_RBC_RB_CNTL__RB_BUFSZ_MASK, UVD_RBC_RB_CNTL__RB_BUFSZ__SHIFT, rb_bufsz);
    tmp = set_field(tmp, UVD_RBC_RB_CNTL__RB_BLKSZ_MASK, UVD_RBC_RB_CNTL__RB_BLKSZ__SHIFT, 1);
    tmp = set_field(tmp, UVD_RBC_RB_CNTL__RB_NO_FETCH_MASK, UVD_RBC_RB_CNTL__RB_NO_FETCH__SHIFT, 1);
    tmp = set_field(tmp, UVD_RBC_RB_CNTL__RB_NO_UPDATE_MASK, UVD_RBC_RB_CNTL__RB_NO_UPDATE__SHIFT, 1);
    tmp = set_field(tmp, UVD_RBC_RB_CNTL__RB_RPTR_WR_EN_MASK, UVD_RBC_RB_CNTL__RB_RPTR_WR_EN__SHIFT, 1);
    wreg(adev, mmUVD_RBC_RB_CNTL, mmUVD_RBC_RB_CNTL_BASE_IDX, tmp);

    // Stall DPG before WPTR/RPTR reset
    wreg_p(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, UVD_POWER_STATUS__STALL_DPG_POWER_UP_MASK,
           !UVD_POWER_STATUS__STALL_DPG_POWER_UP_MASK);
    fw_shared_u8(adev, FW_SHARED_DECODE_QUEUE_MODE, |m| m | FW_QUEUE_RING_RESET as u8);

    // set the write pointer delay
    wreg(adev, mmUVD_RBC_RB_WPTR_CNTL, mmUVD_RBC_RB_WPTR_CNTL_BASE_IDX, 0);

    // set the wb address
    let gpu = adev.rings[dec].gpu_addr;
    wreg(adev, mmUVD_RBC_RB_RPTR_ADDR, mmUVD_RBC_RB_RPTR_ADDR_BASE_IDX, ((gpu >> 32) as u32) >> 2);

    // program the RB_BASE for ring buffer
    wreg(adev, mmUVD_LMI_RBC_RB_64BIT_BAR_LOW, mmUVD_LMI_RBC_RB_64BIT_BAR_LOW_BASE_IDX, gpu as u32);
    wreg(adev, mmUVD_LMI_RBC_RB_64BIT_BAR_HIGH, mmUVD_LMI_RBC_RB_64BIT_BAR_HIGH_BASE_IDX, (gpu >> 32) as u32);

    // Initialize the ring buffer's read and write pointers
    wreg(adev, mmUVD_RBC_RB_RPTR, mmUVD_RBC_RB_RPTR_BASE_IDX, 0);

    wreg(adev, mmUVD_SCRATCH2, mmUVD_SCRATCH2_BASE_IDX, 0);

    let wptr = rreg(adev, mmUVD_RBC_RB_RPTR, mmUVD_RBC_RB_RPTR_BASE_IDX) as u64;
    adev.rings[dec].wptr = wptr;
    wreg(adev, mmUVD_RBC_RB_WPTR, mmUVD_RBC_RB_WPTR_BASE_IDX, wptr as u32);

    fw_shared_u8(adev, FW_SHARED_DECODE_QUEUE_MODE, |m| m & !(FW_QUEUE_RING_RESET as u8));
    // Unstall DPG
    wreg_p(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, 0, !UVD_POWER_STATUS__STALL_DPG_POWER_UP_MASK);

    // Keeping one read-back to ensure all register writes are done,
    // otherwise it may introduce race conditions.
    rreg(adev, mmUVD_STATUS, mmUVD_STATUS_BASE_IDX);
    0
}

/// `amdgpu_vcn_psp_update_sram`: the DPG SRAM written so far, through
/// the PSP.
fn amdgpu_vcn_psp_update_sram(adev: &mut amdgpu_device, inst_idx: usize, ucode_id: u32) -> i32 {
    let v = &adev.vcn.inst[inst_idx];
    let mut ucode = crate::amdgpu_ucode::amdgpu_firmware_info {
        ucode_id: if ucode_id != 0 {
            ucode_id
        } else if inst_idx != 0 {
            AMDGPU_UCODE_ID_VCN1_RAM
        } else {
            AMDGPU_UCODE_ID_VCN0_RAM
        },
        mc_addr: v.dpg_sram_gpu_addr,
        ucode_size: v.dpg_sram_curr as u32,
        ..Default::default()
    };
    crate::amdgpu_psp::psp_execute_ip_fw_load(adev, &mut ucode)
}

/// `vcn_v2_0_start`: Renoir's `pg_flags` carry `VCN_DPG`
/// (`soc15_common_early_init`), so this is the DPG start.
fn vcn_v2_0_start(adev: &mut amdgpu_device) -> i32 {
    if adev.pm.dpm_enabled {
        amdgpu_dpm_enable_vcn(adev, true, 0);
    }
    if adev.pg_flags & AMD_PG_SUPPORT_VCN_DPG != 0 {
        let indirect = adev.vcn.inst[0].indirect_sram;
        return vcn_v2_0_start_dpg_mode(adev, indirect);
    }
    Line::new().s("vcn_v2_0_start: the start without DPG is not Renoir's").say();
    -22
}

/// `vcn_v2_0_stop_dpg_mode`.
fn vcn_v2_0_stop_dpg_mode(adev: &mut amdgpu_device) -> i32 {
    vcn_v2_0_pause_dpg_mode(adev, VCN_DPG_STATE__UNPAUSE);
    // Wait for power status to be 1
    wait_on(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, "mmUVD_POWER_STATUS", 1,
            UVD_POWER_STATUS__UVD_POWER_STATUS_MASK);

    // wait for read ptr to be equal to write ptr
    let tmp = rreg(adev, mmUVD_RB_WPTR, mmUVD_RB_WPTR_BASE_IDX);
    wait_on(adev, mmUVD_RB_RPTR, mmUVD_RB_RPTR_BASE_IDX, "mmUVD_RB_RPTR", tmp, 0xFFFFFFFF);

    let tmp = rreg(adev, mmUVD_RB_WPTR2, mmUVD_RB_WPTR2_BASE_IDX);
    wait_on(adev, mmUVD_RB_RPTR2, mmUVD_RB_RPTR2_BASE_IDX, "mmUVD_RB_RPTR2", tmp, 0xFFFFFFFF);

    let tmp = rreg(adev, mmUVD_RBC_RB_WPTR, mmUVD_RBC_RB_WPTR_BASE_IDX) & 0x7FFFFFFF;
    wait_on(adev, mmUVD_RBC_RB_RPTR, mmUVD_RBC_RB_RPTR_BASE_IDX, "mmUVD_RBC_RB_RPTR", tmp, 0xFFFFFFFF);

    wait_on(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, "mmUVD_POWER_STATUS", 1,
            UVD_POWER_STATUS__UVD_POWER_STATUS_MASK);

    // disable dynamic power gating mode
    wreg_p(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, 0, !UVD_POWER_STATUS__UVD_PG_MODE_MASK);

    // Keeping one read-back to ensure all register writes are done,
    // otherwise it may introduce race conditions.
    rreg(adev, mmUVD_STATUS, mmUVD_STATUS_BASE_IDX);
    0
}

/// `vcn_v2_0_stop`: the DPG stop (see `vcn_v2_0_start`).
fn vcn_v2_0_stop(adev: &mut amdgpu_device) -> i32 {
    if adev.pg_flags & AMD_PG_SUPPORT_VCN_DPG == 0 {
        Line::new().s("vcn_v2_0_stop: the stop without DPG is not Renoir's").say();
        return -22;
    }
    let r = vcn_v2_0_stop_dpg_mode(adev);
    if r != 0 {
        return r;
    }
    // power_off:
    if adev.pm.dpm_enabled {
        amdgpu_dpm_enable_vcn(adev, false, 0);
    }
    0
}

/// `amdgpu_dpm_enable_vcn`.
fn amdgpu_dpm_enable_vcn(adev: &mut amdgpu_device, enable: bool, inst: usize) {
    let r = crate::amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_VCN, !enable, inst);
    if r != 0 {
        Line::new().s("Dpm ").s(if enable { "enable" } else { "disable" }).s(" uvd failed, ret = ").dec((-r) as u64)
            .say();
    }
}

/// `vcn_v2_0_pause_dpg_mode` to `new_state` (`fw_based`).
fn vcn_v2_0_pause_dpg_mode(adev: &mut amdgpu_device, new_state: u32) -> i32 {
    // pause/unpause if state is changed
    if adev.vcn.inst[0].pause_state == new_state {
        return 0;
    }
    let mut reg_data = rreg(adev, mmUVD_DPG_PAUSE, mmUVD_DPG_PAUSE_BASE_IDX) & !UVD_DPG_PAUSE__NJ_PAUSE_DPG_ACK_MASK;
    if new_state == VCN_DPG_STATE__PAUSE {
        let ret_code = wait_on(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, "mmUVD_POWER_STATUS", 0x1,
                               UVD_POWER_STATUS__UVD_POWER_STATUS_MASK);
        if ret_code == 0 {
            // pause DPG
            reg_data |= UVD_DPG_PAUSE__NJ_PAUSE_DPG_REQ_MASK;
            wreg(adev, mmUVD_DPG_PAUSE, mmUVD_DPG_PAUSE_BASE_IDX, reg_data);

            // wait for ACK
            wait_on(adev, mmUVD_DPG_PAUSE, mmUVD_DPG_PAUSE_BASE_IDX, "mmUVD_DPG_PAUSE",
                    UVD_DPG_PAUSE__NJ_PAUSE_DPG_ACK_MASK, UVD_DPG_PAUSE__NJ_PAUSE_DPG_ACK_MASK);

            // Stall DPG before WPTR/RPTR reset
            wreg_p(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, UVD_POWER_STATUS__STALL_DPG_POWER_UP_MASK,
                   !UVD_POWER_STATUS__STALL_DPG_POWER_UP_MASK);
            // Restore
            let enc = adev.vcn.inst[0].ring_enc;
            fw_shared_u8(adev, FW_SHARED_ENCODE_GENERALPURPOSE_QUEUE_MODE, |m| m | FW_QUEUE_RING_RESET as u8);
            if let Some(r) = enc[0] {
                adev.rings[r].wptr = 0;
                let (gpu, size) = (adev.rings[r].gpu_addr, adev.rings[r].ring_size);
                wreg(adev, mmUVD_RB_BASE_LO, mmUVD_RB_BASE_LO_BASE_IDX, gpu as u32);
                wreg(adev, mmUVD_RB_BASE_HI, mmUVD_RB_BASE_HI_BASE_IDX, (gpu >> 32) as u32);
                wreg(adev, mmUVD_RB_SIZE, mmUVD_RB_SIZE_BASE_IDX, size / 4);
                wreg(adev, mmUVD_RB_RPTR, mmUVD_RB_RPTR_BASE_IDX, 0);
                wreg(adev, mmUVD_RB_WPTR, mmUVD_RB_WPTR_BASE_IDX, 0);
            }
            fw_shared_u8(adev, FW_SHARED_ENCODE_GENERALPURPOSE_QUEUE_MODE, |m| m & !(FW_QUEUE_RING_RESET as u8));

            fw_shared_u8(adev, FW_SHARED_ENCODE_LOWLATENCY_QUEUE_MODE, |m| m | FW_QUEUE_RING_RESET as u8);
            if let Some(r) = enc[1] {
                adev.rings[r].wptr = 0;
                let (gpu, size) = (adev.rings[r].gpu_addr, adev.rings[r].ring_size);
                wreg(adev, mmUVD_RB_BASE_LO2, mmUVD_RB_BASE_LO2_BASE_IDX, gpu as u32);
                wreg(adev, mmUVD_RB_BASE_HI2, mmUVD_RB_BASE_HI2_BASE_IDX, (gpu >> 32) as u32);
                wreg(adev, mmUVD_RB_SIZE2, mmUVD_RB_SIZE2_BASE_IDX, size / 4);
                wreg(adev, mmUVD_RB_RPTR2, mmUVD_RB_RPTR2_BASE_IDX, 0);
                wreg(adev, mmUVD_RB_WPTR2, mmUVD_RB_WPTR2_BASE_IDX, 0);
            }
            fw_shared_u8(adev, FW_SHARED_ENCODE_LOWLATENCY_QUEUE_MODE, |m| m & !(FW_QUEUE_RING_RESET as u8));

            fw_shared_u8(adev, FW_SHARED_DECODE_QUEUE_MODE, |m| m | FW_QUEUE_RING_RESET as u8);
            let w = rreg(adev, mmUVD_SCRATCH2, mmUVD_SCRATCH2_BASE_IDX) & 0x7FFFFFFF;
            wreg(adev, mmUVD_RBC_RB_WPTR, mmUVD_RBC_RB_WPTR_BASE_IDX, w);
            fw_shared_u8(adev, FW_SHARED_DECODE_QUEUE_MODE, |m| m & !(FW_QUEUE_RING_RESET as u8));
            // Unstall DPG
            wreg_p(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, 0,
                   !UVD_POWER_STATUS__STALL_DPG_POWER_UP_MASK);

            wait_on(adev, mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX, "mmUVD_POWER_STATUS",
                    UVD_PGFSM_CONFIG__UVDM_UVDU_PWR_ON, UVD_POWER_STATUS__UVD_POWER_STATUS_MASK);
        }
    } else {
        // unpause dpg, no need to wait
        reg_data &= !UVD_DPG_PAUSE__NJ_PAUSE_DPG_REQ_MASK;
        wreg(adev, mmUVD_DPG_PAUSE, mmUVD_DPG_PAUSE_BASE_IDX, reg_data);
    }
    adev.vcn.inst[0].pause_state = new_state;
    0
}

/// `vcn_v2_0_is_idle`.
pub fn vcn_v2_0_is_idle(adev: &amdgpu_device) -> bool {
    rreg(adev, mmUVD_STATUS, mmUVD_STATUS_BASE_IDX) == UVD_STATUS__IDLE
}

/// `vcn_v2_0_wait_for_idle`.
pub fn vcn_v2_0_wait_for_idle(adev: &amdgpu_device) -> i32 {
    wait_on(adev, mmUVD_STATUS, mmUVD_STATUS_BASE_IDX, "mmUVD_STATUS", UVD_STATUS__IDLE, UVD_STATUS__IDLE)
}

/// `vcn_v2_0_set_pg_state`: the SMU gates the block; this restarts it.
pub fn vcn_v2_0_set_pg_state(adev: &mut amdgpu_device, state: u32) -> i32 {
    if state == adev.vcn.inst[0].cur_state {
        return 0;
    }
    let r = if state == AMD_PG_STATE_GATE { vcn_v2_0_stop(adev) } else { vcn_v2_0_start(adev) };
    if r == 0 {
        adev.vcn.inst[0].cur_state = state;
    }
    r
}

/// `vcn_set_powergating_state`.
pub fn vcn_set_powergating_state(adev: &mut amdgpu_device, state: u32) -> i32 {
    let mut ret = 0;
    for _ in 0..adev.vcn.num_vcn_inst {
        ret |= vcn_v2_0_set_pg_state(adev, state);
    }
    ret
}

/// `amdgpu_vcn_get_profile`.
fn amdgpu_vcn_get_profile(adev: &mut amdgpu_device) {
    if adev.vcn.workload_profile_active {
        return;
    }
    let r = crate::amdgpu_smu::amdgpu_dpm_switch_power_profile(adev, PP_SMC_POWER_PROFILE_VIDEO, true);
    if r != 0 {
        Line::new().s("(").dec((-r) as u64).s(") failed to enable video power profile mode").say();
    } else {
        adev.vcn.workload_profile_active = true;
    }
}

/// `amdgpu_vcn_put_profile`.
fn amdgpu_vcn_put_profile(adev: &mut amdgpu_device) {
    let pg = (0..adev.vcn.num_vcn_inst as usize).all(|i| adev.vcn.inst[i].cur_state == AMD_PG_STATE_GATE);
    if pg {
        let r = crate::amdgpu_smu::amdgpu_dpm_switch_power_profile(adev, PP_SMC_POWER_PROFILE_VIDEO, false);
        if r != 0 {
            Line::new().s("(").dec((-r) as u64).s(") failed to disable video power profile mode").say();
        } else {
            adev.vcn.workload_profile_active = false;
        }
    }
}

/// `amdgpu_vcn_idle_work_handler`.
pub fn amdgpu_vcn_idle_work_handler(adev: &mut amdgpu_device, i: usize) {
    if adev.vcn.harvest_config & (1 << i) != 0 {
        return;
    }
    let v = &adev.vcn.inst[i];
    let (enc, dec, n) = (v.ring_enc, v.ring_dec, v.num_enc_rings as usize);
    let mut fence = 0;
    for r in enc[..n].iter().flatten() {
        fence += amdgpu_fence_count_emitted(adev, *r);
    }
    // Only set DPG pause for VCN3 or below, VCN4 and above will be handled
    // by FW
    if adev.pg_flags & AMD_PG_SUPPORT_VCN_DPG != 0 && !adev.vcn.inst[i].using_unified_queue {
        let new_state = if fence != 0 || adev.vcn.inst[i].dpg_enc_submission_cnt != 0 {
            VCN_DPG_STATE__PAUSE
        } else {
            VCN_DPG_STATE__UNPAUSE
        };
        vcn_v2_0_pause_dpg_mode(adev, new_state);
    }
    if let Some(d) = dec {
        fence += amdgpu_fence_count_emitted(adev, d);
    }
    if fence == 0 && adev.vcn.inst[i].total_submission_cnt == 0 {
        vcn_v2_0_set_pg_state(adev, AMD_PG_STATE_GATE);
        amdgpu_vcn_put_profile(adev);
    } else {
        crate::workqueue::mod_delayed_work(adev, crate::workqueue::work::vcn_idle { inst: i }, VCN_IDLE_TIMEOUT_US);
    }
}

/// `amdgpu_fence_count_emitted`.
pub fn amdgpu_fence_count_emitted(adev: &amdgpu_device, r: usize) -> u32 {
    let f = &adev.rings[r].fence_drv;
    let emitted = 0x1_0000_0000u64 - f.last_seq as u64 + f.sync_seq as u64;
    emitted as u32
}

/// `amdgpu_vcn_ring_begin_use`.
pub fn amdgpu_vcn_ring_begin_use(adev: &mut amdgpu_device, r: usize) {
    let me = adev.rings[r].me as usize;
    adev.vcn.inst[me].total_submission_cnt += 1;
    crate::workqueue::cancel_delayed_work(adev, crate::workqueue::work::vcn_idle { inst: me });
    vcn_v2_0_set_pg_state(adev, AMD_PG_STATE_UNGATE);

    // Only set DPG pause for VCN3 or below, VCN4 and above will be handled
    // by FW
    if adev.pg_flags & AMD_PG_SUPPORT_VCN_DPG != 0 && !adev.vcn.inst[me].using_unified_queue {
        let new_state = if adev.rings[r].funcs.type_ == AMDGPU_RING_TYPE_VCN_ENC {
            adev.vcn.inst[me].dpg_enc_submission_cnt += 1;
            VCN_DPG_STATE__PAUSE
        } else {
            let v = &adev.vcn.inst[me];
            let (enc, n) = (v.ring_enc, v.num_enc_rings as usize);
            let fences: u32 = enc[..n].iter().flatten().map(|&e| amdgpu_fence_count_emitted(adev, e)).sum();
            if fences != 0 || adev.vcn.inst[me].dpg_enc_submission_cnt != 0 {
                VCN_DPG_STATE__PAUSE
            } else {
                VCN_DPG_STATE__UNPAUSE
            }
        };
        vcn_v2_0_pause_dpg_mode(adev, new_state);
    }
    amdgpu_vcn_get_profile(adev);
}

/// `amdgpu_vcn_ring_end_use`.
pub fn amdgpu_vcn_ring_end_use(adev: &mut amdgpu_device, r: usize) {
    let me = adev.rings[r].me as usize;
    // Only set DPG pause for VCN3 or below, VCN4 and above will be handled
    // by FW
    if adev.pg_flags & AMD_PG_SUPPORT_VCN_DPG != 0 && adev.rings[r].funcs.type_ == AMDGPU_RING_TYPE_VCN_ENC
        && !adev.vcn.inst[me].using_unified_queue
    {
        adev.vcn.inst[me].dpg_enc_submission_cnt -= 1;
    }
    adev.vcn.inst[me].total_submission_cnt -= 1;
    crate::workqueue::mod_delayed_work(adev, crate::workqueue::work::vcn_idle { inst: me }, VCN_IDLE_TIMEOUT_US);
}

// ---- the rings ----

use crate::amdgpu_ring::amdgpu_ring_write as rw;

/// `PACKET0(reg, n)` (soc15d.h).
fn packet0(reg: u32, n: u32) -> u32 {
    (reg & 0xFFFF) | (n & 0x3FFF) << 16
}

/// `vcn_v2_0_dec_ring_get_rptr`.
pub fn vcn_v2_0_dec_ring_get_rptr(adev: &amdgpu_device, _r: usize) -> u64 {
    rreg(adev, mmUVD_RBC_RB_RPTR, mmUVD_RBC_RB_RPTR_BASE_IDX) as u64
}

/// `vcn_v2_0_dec_ring_get_wptr`.
pub fn vcn_v2_0_dec_ring_get_wptr(adev: &amdgpu_device, r: usize) -> u64 {
    if adev.rings[r].use_doorbell {
        crate::amdgpu_ring::ring_wb_read64(adev, adev.rings[r].wptr_offs)
    } else {
        rreg(adev, mmUVD_RBC_RB_WPTR, mmUVD_RBC_RB_WPTR_BASE_IDX) as u64
    }
}

/// `vcn_v2_0_dec_ring_set_wptr`.
pub fn vcn_v2_0_dec_ring_set_wptr(adev: &mut amdgpu_device, r: usize) {
    let wptr = adev.rings[r].wptr as u32;
    if adev.pg_flags & AMD_PG_SUPPORT_VCN_DPG != 0 {
        wreg(adev, mmUVD_SCRATCH2, mmUVD_SCRATCH2_BASE_IDX, wptr | 0x80000000);
    }
    if adev.rings[r].use_doorbell {
        crate::amdgpu_ring::ring_wptr_cpu_set(adev, r, wptr as u64);
        crate::amdgpu_ring::amdgpu_mm_wdoorbell(adev, adev.rings[r].doorbell_index, wptr);
    } else {
        wreg(adev, mmUVD_RBC_RB_WPTR, mmUVD_RBC_RB_WPTR_BASE_IDX, wptr);
    }
}

/// `vcn_v2_0_dec_ring_insert_start`.
pub fn vcn_v2_0_dec_ring_insert_start(adev: &mut amdgpu_device, r: usize) {
    let i = &adev.vcn.inst[adev.rings[r].me as usize].internal;
    let (data0, cmd) = (i.data0, i.cmd);
    rw(adev, r, packet0(data0, 0));
    rw(adev, r, 0);
    rw(adev, r, packet0(cmd, 0));
    rw(adev, r, VCN_DEC_KMD_CMD | VCN_DEC_CMD_PACKET_START << 1);
}

/// `vcn_v2_0_dec_ring_insert_end`.
pub fn vcn_v2_0_dec_ring_insert_end(adev: &mut amdgpu_device, r: usize) {
    let cmd = adev.vcn.inst[0].internal.cmd;
    rw(adev, r, packet0(cmd, 0));
    rw(adev, r, VCN_DEC_KMD_CMD | VCN_DEC_CMD_PACKET_END << 1);
}

/// `vcn_v2_0_dec_ring_insert_nop`.
pub fn vcn_v2_0_dec_ring_insert_nop(adev: &mut amdgpu_device, r: usize, count: u32) {
    if adev.rings[r].wptr % 2 != 0 || count % 2 != 0 {
        Line::new().s("WARNING: vcn_v2_0_dec_ring_insert_nop on an odd count").say();
    }
    let nop = adev.vcn.inst[adev.rings[r].me as usize].internal.nop;
    for _ in 0..count / 2 {
        rw(adev, r, packet0(nop, 0));
        rw(adev, r, 0);
    }
}

/// `vcn_v2_0_dec_ring_emit_fence`.
pub fn vcn_v2_0_dec_ring_emit_fence(adev: &mut amdgpu_device, r: usize, addr: u64, seq: u64, flags: u32) {
    if flags & AMDGPU_FENCE_FLAG_64BIT != 0 {
        Line::new().s("WARNING: vcn_v2_0_dec_ring_emit_fence with a 64-bit fence").say();
    }
    let i = &adev.vcn.inst[adev.rings[r].me as usize].internal;
    let (context_id, data0, data1, cmd) = (i.context_id, i.data0, i.data1, i.cmd);
    rw(adev, r, packet0(context_id, 0));
    rw(adev, r, seq as u32);

    rw(adev, r, packet0(data0, 0));
    rw(adev, r, addr as u32);

    rw(adev, r, packet0(data1, 0));
    rw(adev, r, (addr >> 32) as u32 & 0xff);

    rw(adev, r, packet0(cmd, 0));
    rw(adev, r, VCN_DEC_KMD_CMD | VCN_DEC_CMD_FENCE << 1);

    rw(adev, r, packet0(data0, 0));
    rw(adev, r, 0);

    rw(adev, r, packet0(data1, 0));
    rw(adev, r, 0);

    rw(adev, r, packet0(cmd, 0));

    rw(adev, r, VCN_DEC_KMD_CMD | VCN_DEC_CMD_TRAP << 1);
}

/// `vcn_v2_0_dec_ring_emit_ib`.
pub fn vcn_v2_0_dec_ring_emit_ib(adev: &mut amdgpu_device, r: usize, vmid: u32, ib: &crate::amdgpu_ib::amdgpu_ib,
                                 _flags: u32) {
    let i = &adev.vcn.inst[adev.rings[r].me as usize].internal;
    let (ib_vmid, lo, hi, size) = (i.ib_vmid, i.ib_bar_low, i.ib_bar_high, i.ib_size);
    rw(adev, r, packet0(ib_vmid, 0));
    rw(adev, r, vmid);

    rw(adev, r, packet0(lo, 0));
    rw(adev, r, ib.gpu_addr as u32);
    rw(adev, r, packet0(hi, 0));
    rw(adev, r, (ib.gpu_addr >> 32) as u32);
    rw(adev, r, packet0(size, 0));
    rw(adev, r, ib.length_dw);
}

/// `vcn_v2_0_dec_ring_emit_reg_wait`.
pub fn vcn_v2_0_dec_ring_emit_reg_wait(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32, mask: u32) {
    let i = &adev.vcn.inst[adev.rings[r].me as usize].internal;
    let (data0, data1, gp_scratch8, cmd) = (i.data0, i.data1, i.gp_scratch8, i.cmd);
    rw(adev, r, packet0(data0, 0));
    rw(adev, r, reg << 2);

    rw(adev, r, packet0(data1, 0));
    rw(adev, r, val);

    rw(adev, r, packet0(gp_scratch8, 0));
    rw(adev, r, mask);

    rw(adev, r, packet0(cmd, 0));

    rw(adev, r, VCN_DEC_KMD_CMD | VCN_DEC_CMD_REG_READ_COND_WAIT << 1);
}

/// `vcn_v2_0_dec_ring_emit_vm_flush`.
pub fn vcn_v2_0_dec_ring_emit_vm_flush(adev: &mut amdgpu_device, r: usize, vmid: u32, pd_addr: u64) {
    let hub = adev.vmhub[adev.rings[r].vm_hub as usize];
    let pd_addr = crate::gmc_v9_0::gmc_v9_0_emit_flush_gpu_tlb(adev, r, vmid, pd_addr);

    // wait for register write
    let data0 = hub.ctx0_ptb_addr_lo32 + vmid * hub.ctx_addr_distance;
    let data1 = pd_addr as u32;
    let mask = 0xffffffff;
    vcn_v2_0_dec_ring_emit_reg_wait(adev, r, data0, data1, mask);
}

/// `vcn_v2_0_dec_ring_emit_wreg`.
pub fn vcn_v2_0_dec_ring_emit_wreg(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32) {
    let i = &adev.vcn.inst[adev.rings[r].me as usize].internal;
    let (data0, data1, cmd) = (i.data0, i.data1, i.cmd);
    rw(adev, r, packet0(data0, 0));
    rw(adev, r, reg << 2);

    rw(adev, r, packet0(data1, 0));
    rw(adev, r, val);

    rw(adev, r, packet0(cmd, 0));

    rw(adev, r, VCN_DEC_KMD_CMD | VCN_DEC_CMD_WRITE_REG << 1);
}

/// Whether ring `r` is the first encode ring.
fn is_enc0(adev: &amdgpu_device, r: usize) -> bool {
    adev.vcn.inst[0].ring_enc[0] == Some(r)
}

/// `vcn_v2_0_enc_ring_get_rptr`.
pub fn vcn_v2_0_enc_ring_get_rptr(adev: &amdgpu_device, r: usize) -> u64 {
    if is_enc0(adev, r) {
        rreg(adev, mmUVD_RB_RPTR, mmUVD_RB_RPTR_BASE_IDX) as u64
    } else {
        rreg(adev, mmUVD_RB_RPTR2, mmUVD_RB_RPTR2_BASE_IDX) as u64
    }
}

/// `vcn_v2_0_enc_ring_get_wptr`.
pub fn vcn_v2_0_enc_ring_get_wptr(adev: &amdgpu_device, r: usize) -> u64 {
    if adev.rings[r].use_doorbell {
        crate::amdgpu_ring::ring_wb_read64(adev, adev.rings[r].wptr_offs)
    } else if is_enc0(adev, r) {
        rreg(adev, mmUVD_RB_WPTR, mmUVD_RB_WPTR_BASE_IDX) as u64
    } else {
        rreg(adev, mmUVD_RB_WPTR2, mmUVD_RB_WPTR2_BASE_IDX) as u64
    }
}

/// `vcn_v2_0_enc_ring_set_wptr`.
pub fn vcn_v2_0_enc_ring_set_wptr(adev: &mut amdgpu_device, r: usize) {
    let wptr = adev.rings[r].wptr as u32;
    if adev.rings[r].use_doorbell {
        crate::amdgpu_ring::ring_wptr_cpu_set(adev, r, wptr as u64);
        crate::amdgpu_ring::amdgpu_mm_wdoorbell(adev, adev.rings[r].doorbell_index, wptr);
    } else if is_enc0(adev, r) {
        wreg(adev, mmUVD_RB_WPTR, mmUVD_RB_WPTR_BASE_IDX, wptr);
    } else {
        wreg(adev, mmUVD_RB_WPTR2, mmUVD_RB_WPTR2_BASE_IDX, wptr);
    }
}

/// `vcn_v2_0_enc_ring_emit_fence`.
pub fn vcn_v2_0_enc_ring_emit_fence(adev: &mut amdgpu_device, r: usize, addr: u64, seq: u64, flags: u32) {
    if flags & AMDGPU_FENCE_FLAG_64BIT != 0 {
        Line::new().s("WARNING: vcn_v2_0_enc_ring_emit_fence with a 64-bit fence").say();
    }
    rw(adev, r, VCN_ENC_CMD_FENCE);
    rw(adev, r, addr as u32);
    rw(adev, r, (addr >> 32) as u32);
    rw(adev, r, seq as u32);
    rw(adev, r, VCN_ENC_CMD_TRAP);
}

/// `vcn_v2_0_enc_ring_insert_end`.
pub fn vcn_v2_0_enc_ring_insert_end(adev: &mut amdgpu_device, r: usize) {
    rw(adev, r, VCN_ENC_CMD_END);
}

/// `vcn_v2_0_enc_ring_emit_ib`.
pub fn vcn_v2_0_enc_ring_emit_ib(adev: &mut amdgpu_device, r: usize, vmid: u32, ib: &crate::amdgpu_ib::amdgpu_ib,
                                 _flags: u32) {
    rw(adev, r, VCN_ENC_CMD_IB);
    rw(adev, r, vmid);
    rw(adev, r, ib.gpu_addr as u32);
    rw(adev, r, (ib.gpu_addr >> 32) as u32);
    rw(adev, r, ib.length_dw);
}

/// `vcn_v2_0_enc_ring_emit_reg_wait`.
pub fn vcn_v2_0_enc_ring_emit_reg_wait(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32, mask: u32) {
    rw(adev, r, VCN_ENC_CMD_REG_WAIT);
    rw(adev, r, reg << 2);
    rw(adev, r, mask);
    rw(adev, r, val);
}

/// `vcn_v2_0_enc_ring_emit_vm_flush`.
pub fn vcn_v2_0_enc_ring_emit_vm_flush(adev: &mut amdgpu_device, r: usize, vmid: u32, pd_addr: u64) {
    let hub = adev.vmhub[adev.rings[r].vm_hub as usize];
    let pd_addr = crate::gmc_v9_0::gmc_v9_0_emit_flush_gpu_tlb(adev, r, vmid, pd_addr);

    // wait for reg writes
    vcn_v2_0_enc_ring_emit_reg_wait(adev, r, hub.ctx0_ptb_addr_lo32 + vmid * hub.ctx_addr_distance, pd_addr as u32,
                                    0xffffffff);
}

/// `vcn_v2_0_enc_ring_emit_wreg`.
pub fn vcn_v2_0_enc_ring_emit_wreg(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32) {
    rw(adev, r, VCN_ENC_CMD_REG_WRITE);
    rw(adev, r, reg << 2);
    rw(adev, r, val);
}

/// `vcn_v2_0_process_interrupt`.
pub fn vcn_v2_0_process_interrupt(adev: &mut amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry) -> i32 {
    let v = &adev.vcn.inst[0];
    let ring = match entry.src_id {
        VCN_2_0__SRCID__UVD_SYSTEM_MESSAGE_INTERRUPT => v.ring_dec,
        VCN_2_0__SRCID__UVD_ENC_GENERAL_PURPOSE => v.ring_enc[0],
        VCN_2_0__SRCID__UVD_ENC_LOW_LATENCY => v.ring_enc[1],
        _ => {
            Line::new().s("Unhandled interrupt: ").dec(entry.src_id as u64).s(" ").dec(entry.src_data[0] as u64)
                .say();
            None
        }
    };
    if let Some(r) = ring {
        crate::amdgpu_fence::amdgpu_fence_process(adev, r);
    }
    0
}

/// `vcn_v2_0_dec_ring_test_ring`.
pub fn vcn_v2_0_dec_ring_test_ring(adev: &mut amdgpu_device, r: usize) -> i32 {
    let me = adev.rings[r].me as usize;
    let (ext, int) = (adev.vcn.inst[me].external.scratch9, &adev.vcn.inst[me].internal);
    let (cmd, scratch9) = (int.cmd, int.scratch9);
    adev.wreg32(ext, 0xCAFEDEAD);
    let ret = crate::amdgpu_ring::amdgpu_ring_alloc(adev, r, 4);
    if ret != 0 {
        return ret;
    }
    rw(adev, r, packet0(cmd, 0));
    rw(adev, r, VCN_DEC_KMD_CMD | VCN_DEC_CMD_PACKET_START << 1);
    rw(adev, r, packet0(scratch9, 0));
    rw(adev, r, 0xDEADBEEF);
    crate::amdgpu_ring::amdgpu_ring_commit(adev, r);
    for _ in 0..adev.usec_timeout {
        if adev.rreg32(ext).unwrap_or(0) == 0xDEADBEEF {
            return 0;
        }
        crate::linux::udelay(1);
    }
    -110
}

/// `amdgpu_vcn_enc_ring_test_ring`.
pub fn amdgpu_vcn_enc_ring_test_ring(adev: &mut amdgpu_device, r: usize) -> i32 {
    let ret = crate::amdgpu_ring::amdgpu_ring_alloc(adev, r, 16);
    if ret != 0 {
        return ret;
    }
    let rptr = vcn_v2_0_enc_ring_get_rptr(adev, r);
    rw(adev, r, VCN_ENC_CMD_END);
    crate::amdgpu_ring::amdgpu_ring_commit(adev, r);
    for _ in 0..adev.usec_timeout {
        if vcn_v2_0_enc_ring_get_rptr(adev, r) != rptr {
            return 0;
        }
        crate::linux::udelay(1);
    }
    -110
}

/// `amdgpu_vcn_dec_send_msg`: a direct job that hands the decoder the
/// message in `ib_msg`; its fence.
fn amdgpu_vcn_dec_send_msg(adev: &mut amdgpu_device, r: usize, ib_msg: &mut crate::amdgpu_ib::amdgpu_ib)
                           -> Result<crate::dma_fence::Fence, i32> {
    let addr = crate::linux::align(ib_msg.gpu_addr, AMDGPU_GPU_PAGE_SIZE as u64);
    let int = &adev.vcn.inst[adev.rings[r].me as usize].internal;
    let (data0, data1, cmd, nop) = (int.data0, int.data1, int.cmd, int.nop);
    let j = match crate::amdgpu_job::amdgpu_job_alloc_with_ib(adev, None, crate::amdgpu_sync::AMDGPU_FENCE_OWNER_UNDEFINED,
                                                             64, AMDGPU_IB_POOL_DIRECT) {
        Ok(j) => j,
        Err(e) => {
            crate::amdgpu_ib::amdgpu_ib_free(adev, ib_msg, None);
            return Err(e);
        }
    };
    let ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
    ib.set(adev, 0, packet0(data0, 0));
    ib.set(adev, 1, addr as u32);
    ib.set(adev, 2, packet0(data1, 0));
    ib.set(adev, 3, (addr >> 32) as u32);
    ib.set(adev, 4, packet0(cmd, 0));
    ib.set(adev, 5, 0);
    for i in (6..16).step_by(2) {
        ib.set(adev, i, packet0(nop, 0));
        ib.set(adev, i + 1, 0);
    }
    crate::amdgpu_job::job(adev, j).ibs[0].length_dw = 16;
    match crate::amdgpu_job::amdgpu_job_submit_direct(adev, j, r) {
        Ok(f) => {
            crate::amdgpu_ib::amdgpu_ib_free(adev, ib_msg, Some(f.clone()));
            Ok(f)
        }
        Err(e) => {
            crate::amdgpu_job::amdgpu_job_free(adev, j);
            crate::amdgpu_ib::amdgpu_ib_free(adev, ib_msg, None);
            Err(e)
        }
    }
}

/// `amdgpu_vcn_dec_get_create_msg` / `_destroy_msg`: an IB holding the
/// message at its first page boundary.
fn amdgpu_vcn_dec_get_msg(adev: &mut amdgpu_device, handle: u32, create: bool, ib: &mut crate::amdgpu_ib::amdgpu_ib)
                          -> i32 {
    *ib = crate::amdgpu_ib::amdgpu_ib::new();
    let r = crate::amdgpu_ib::amdgpu_ib_get(adev, AMDGPU_GPU_PAGE_SIZE * 2, AMDGPU_IB_POOL_DIRECT, ib);
    if r != 0 {
        return r;
    }
    let Some((k, off)) = ib.ptr else { return -22 };
    let at = crate::linux::align(off, AMDGPU_GPU_PAGE_SIZE as u64);
    let msg: &[u32] = if create {
        &[0x00000028, 0x00000038, 0x00000001, 0x00000000, handle, 0x00000000, 0x00000001, 0x00000028, 0x00000010,
          0x00000000, 0x00000007, 0x00000000, 0x00000780, 0x00000440]
    } else {
        &[0x00000028, 0x00000018, 0x00000000, 0x00000002, handle, 0x00000000]
    };
    for i in 0..1024u64 {
        crate::amdgpu_gart::kptr_w32(adev, k, at + i * 4, msg.get(i as usize).copied().unwrap_or(0));
    }
    0
}

/// `amdgpu_vcn_dec_ring_test_ib`.
pub fn amdgpu_vcn_dec_ring_test_ib(adev: &mut amdgpu_device, r: usize, timeout: i64) -> i32 {
    let mut ib = crate::amdgpu_ib::amdgpu_ib::new();
    let ret = amdgpu_vcn_dec_get_msg(adev, 1, true, &mut ib);
    if ret != 0 {
        return ret;
    }
    if let Err(e) = amdgpu_vcn_dec_send_msg(adev, r, &mut ib) {
        return e;
    }
    let ret = amdgpu_vcn_dec_get_msg(adev, 1, false, &mut ib);
    if ret != 0 {
        return ret;
    }
    let f = match amdgpu_vcn_dec_send_msg(adev, r, &mut ib) {
        Ok(f) => f,
        Err(e) => return e,
    };
    if crate::dma_fence::dma_fence_wait_timeout(adev, &f, timeout) == 0 { -110 } else { 0 }
}

/// `amdgpu_vcn_enc_get_create_msg` / `_destroy_msg`: the session opened
/// (`op` 0x08000001) or closed (0x08000002) on the message buffer at
/// `ib_msg`.
fn amdgpu_vcn_enc_send(adev: &mut amdgpu_device, r: usize, handle: u32, op: u32, ib_msg: &crate::amdgpu_ib::amdgpu_ib)
                       -> Result<crate::dma_fence::Fence, i32> {
    let ib_size_dw = 16u32;
    let j = crate::amdgpu_job::amdgpu_job_alloc_with_ib(adev, None, crate::amdgpu_sync::AMDGPU_FENCE_OWNER_UNDEFINED,
                                                        ib_size_dw * 4, AMDGPU_IB_POOL_DIRECT)?;
    let ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
    let addr = crate::linux::align(ib_msg.gpu_addr, AMDGPU_GPU_PAGE_SIZE as u64);
    let words = [0x00000018, 0x00000001, handle, (addr >> 32) as u32, addr as u32, 0x00000000, 0x00000014,
                 0x00000002, 0x0000001c, 0x00000000, 0x00000000, 0x00000008, op];
    for i in 0..ib_size_dw {
        ib.set(adev, i, words.get(i as usize).copied().unwrap_or(0));
    }
    crate::amdgpu_job::job(adev, j).ibs[0].length_dw = words.len() as u32;
    match crate::amdgpu_job::amdgpu_job_submit_direct(adev, j, r) {
        Ok(f) => Ok(f),
        Err(e) => {
            crate::amdgpu_job::amdgpu_job_free(adev, j);
            Err(e)
        }
    }
}

/// `amdgpu_vcn_enc_ring_test_ib`.
pub fn amdgpu_vcn_enc_ring_test_ib(adev: &mut amdgpu_device, r: usize, timeout: i64) -> i32 {
    let mut ib = crate::amdgpu_ib::amdgpu_ib::new();
    let ret = crate::amdgpu_ib::amdgpu_ib_get(adev, (128 << 10) + AMDGPU_GPU_PAGE_SIZE, AMDGPU_IB_POOL_DIRECT, &mut ib);
    if ret != 0 {
        return ret;
    }
    let mut fence = None;
    let mut ret = match amdgpu_vcn_enc_send(adev, r, 1, 0x08000001, &ib) {
        Err(e) => e,
        Ok(_) => match amdgpu_vcn_enc_send(adev, r, 1, 0x08000002, &ib) {
            Err(e) => e,
            Ok(f) => {
                fence = Some(f);
                0
            }
        },
    };
    if let Some(f) = &fence {
        if crate::dma_fence::dma_fence_wait_timeout(adev, f, timeout) == 0 {
            ret = -110;
        }
    }
    crate::amdgpu_ib::amdgpu_ib_free(adev, &mut ib, fence);
    ret
}

/// `vcn_v2_0_reset`.
fn vcn_v2_0_reset(adev: &mut amdgpu_device) -> i32 {
    let r = vcn_v2_0_stop(adev);
    if r != 0 {
        return r;
    }
    vcn_v2_0_start(adev)
}

/// `amdgpu_vcn_reset_engine`.
fn amdgpu_vcn_reset_engine(adev: &mut amdgpu_device, ring: usize, timedout: &crate::dma_fence::Fence) -> i32 {
    let vinst = &adev.vcn.inst[adev.rings[ring].me as usize];
    let dec = vinst.ring_dec.unwrap();
    let enc: alloc::vec::Vec<usize> = vinst.ring_enc[..vinst.num_enc_rings as usize].iter().map(|r| r.unwrap()).collect();
    // Stop the scheduler's work queue for the dec and enc rings if they are
    // running. This ensures that no new tasks are submitted to the queues
    // while the reset is in progress.
    crate::drm_sched::drm_sched_wqueue_stop(adev, dec);
    for &e in &enc {
        crate::drm_sched::drm_sched_wqueue_stop(adev, e);
    }
    // Perform the VCN reset for the specified instance
    let r = vcn_v2_0_reset(adev);
    if r != 0 {
        return r;
    }
    let r = crate::amdgpu_ring::amdgpu_ring_test_ring(adev, dec);
    if r != 0 {
        return r;
    }
    for &e in &enc {
        let r = crate::amdgpu_ring::amdgpu_ring_test_ring(adev, e);
        if r != 0 {
            return r;
        }
    }
    crate::amdgpu_fence::amdgpu_fence_driver_force_completion(adev, dec, (dec == ring).then_some(timedout));
    for &e in &enc {
        crate::amdgpu_fence::amdgpu_fence_driver_force_completion(adev, e, (e == ring).then_some(timedout));
    }
    // Restart the scheduler's work queue for the dec and enc rings if they
    // were stopped by this function. This allows new tasks to be submitted
    // to the queues after the reset is complete.
    crate::drm_sched::drm_sched_wqueue_start(adev, dec);
    for &e in &enc {
        crate::drm_sched::drm_sched_wqueue_start(adev, e);
    }
    0
}

/// `amdgpu_vcn_ring_reset`: VCN 2.0 has no unified queue.
pub fn amdgpu_vcn_ring_reset(adev: &mut amdgpu_device, ring: usize, _vmid: u32,
                             timedout: &crate::dma_fence::Fence) -> i32 {
    amdgpu_vcn_reset_engine(adev, ring, timedout)
}

// ---- the IP dump of a job timeout ----

/// `vcn_reg_list_2_0`: name, offset, base index.
static VCN_REG_LIST_2_0: [(&str, u32, u32); VCN_REG_LIST_2_0_COUNT as usize] = [
    ("mmUVD_POWER_STATUS", mmUVD_POWER_STATUS, mmUVD_POWER_STATUS_BASE_IDX),
    ("mmUVD_STATUS", mmUVD_STATUS, mmUVD_STATUS_BASE_IDX),
    ("mmUVD_CONTEXT_ID", mmUVD_CONTEXT_ID, mmUVD_CONTEXT_ID_BASE_IDX),
    ("mmUVD_CONTEXT_ID2", mmUVD_CONTEXT_ID2, mmUVD_CONTEXT_ID2_BASE_IDX),
    ("mmUVD_GPCOM_VCPU_DATA0", mmUVD_GPCOM_VCPU_DATA0, mmUVD_GPCOM_VCPU_DATA0_BASE_IDX),
    ("mmUVD_GPCOM_VCPU_DATA1", mmUVD_GPCOM_VCPU_DATA1, mmUVD_GPCOM_VCPU_DATA1_BASE_IDX),
    ("mmUVD_GPCOM_VCPU_CMD", mmUVD_GPCOM_VCPU_CMD, mmUVD_GPCOM_VCPU_CMD_BASE_IDX),
    ("mmUVD_RB_BASE_HI", mmUVD_RB_BASE_HI, mmUVD_RB_BASE_HI_BASE_IDX),
    ("mmUVD_RB_BASE_LO", mmUVD_RB_BASE_LO, mmUVD_RB_BASE_LO_BASE_IDX),
    ("mmUVD_RB_BASE_HI2", mmUVD_RB_BASE_HI2, mmUVD_RB_BASE_HI2_BASE_IDX),
    ("mmUVD_RB_BASE_LO2", mmUVD_RB_BASE_LO2, mmUVD_RB_BASE_LO2_BASE_IDX),
    ("mmUVD_RB_BASE_HI3", mmUVD_RB_BASE_HI3, mmUVD_RB_BASE_HI3_BASE_IDX),
    ("mmUVD_RB_BASE_LO3", mmUVD_RB_BASE_LO3, mmUVD_RB_BASE_LO3_BASE_IDX),
    ("mmUVD_RB_BASE_HI4", mmUVD_RB_BASE_HI4, mmUVD_RB_BASE_HI4_BASE_IDX),
    ("mmUVD_RB_BASE_LO4", mmUVD_RB_BASE_LO4, mmUVD_RB_BASE_LO4_BASE_IDX),
    ("mmUVD_RB_RPTR", mmUVD_RB_RPTR, mmUVD_RB_RPTR_BASE_IDX),
    ("mmUVD_RB_WPTR", mmUVD_RB_WPTR, mmUVD_RB_WPTR_BASE_IDX),
    ("mmUVD_RB_RPTR2", mmUVD_RB_RPTR2, mmUVD_RB_RPTR2_BASE_IDX),
    ("mmUVD_RB_WPTR2", mmUVD_RB_WPTR2, mmUVD_RB_WPTR2_BASE_IDX),
    ("mmUVD_RB_RPTR3", mmUVD_RB_RPTR3, mmUVD_RB_RPTR3_BASE_IDX),
    ("mmUVD_RB_WPTR3", mmUVD_RB_WPTR3, mmUVD_RB_WPTR3_BASE_IDX),
    ("mmUVD_RB_RPTR4", mmUVD_RB_RPTR4, mmUVD_RB_RPTR4_BASE_IDX),
    ("mmUVD_RB_WPTR4", mmUVD_RB_WPTR4, mmUVD_RB_WPTR4_BASE_IDX),
    ("mmUVD_RB_SIZE", mmUVD_RB_SIZE, mmUVD_RB_SIZE_BASE_IDX),
    ("mmUVD_RB_SIZE2", mmUVD_RB_SIZE2, mmUVD_RB_SIZE2_BASE_IDX),
    ("mmUVD_RB_SIZE3", mmUVD_RB_SIZE3, mmUVD_RB_SIZE3_BASE_IDX),
    ("mmUVD_RB_SIZE4", mmUVD_RB_SIZE4, mmUVD_RB_SIZE4_BASE_IDX),
    ("mmUVD_PGFSM_CONFIG", mmUVD_PGFSM_CONFIG, mmUVD_PGFSM_CONFIG_BASE_IDX),
    ("mmUVD_PGFSM_STATUS", mmUVD_PGFSM_STATUS, mmUVD_PGFSM_STATUS_BASE_IDX),
    ("mmUVD_DPG_LMA_CTL", mmUVD_DPG_LMA_CTL, mmUVD_DPG_LMA_CTL_BASE_IDX),
    ("mmUVD_DPG_LMA_DATA", mmUVD_DPG_LMA_DATA, mmUVD_DPG_LMA_DATA_BASE_IDX),
    ("mmUVD_DPG_LMA_MASK", mmUVD_DPG_LMA_MASK, mmUVD_DPG_LMA_MASK_BASE_IDX),
    ("mmUVD_DPG_PAUSE", mmUVD_DPG_PAUSE, mmUVD_DPG_PAUSE_BASE_IDX),
];

/// `amdgpu_vcn_dump_ip_state`: `mmUVD_POWER_STATUS` always, the rest only
/// while the tiles are powered.
pub fn amdgpu_vcn_dump_ip_state(adev: &mut amdgpu_device) {
    if adev.vcn.ip_dump.is_empty() {
        return;
    }
    let count = adev.vcn.reg_count as usize;
    for i in 0..adev.vcn.num_vcn_inst as usize {
        if adev.vcn.harvest_config & (1 << i) != 0 {
            continue;
        }
        let inst_off = i * count;
        let (_, reg, idx) = VCN_REG_LIST_2_0[0];
        adev.vcn.ip_dump[inst_off] = rreg(adev, reg, idx);
        let is_powered = adev.vcn.ip_dump[inst_off] & UVD_POWER_STATUS__UVD_POWER_STATUS_TILES_OFF
            != UVD_POWER_STATUS__UVD_POWER_STATUS_TILES_OFF;
        if is_powered {
            for j in 1..count {
                let (_, reg, idx) = VCN_REG_LIST_2_0[j];
                adev.vcn.ip_dump[inst_off + j] = rreg(adev, reg, idx);
            }
        }
    }
}

/// `amdgpu_vcn_print_ip_state`.
pub fn amdgpu_vcn_print_ip_state(adev: &amdgpu_device) {
    if adev.vcn.ip_dump.is_empty() {
        return;
    }
    Line::new().s("num_instances:").dec(adev.vcn.num_vcn_inst as u64).say();
    let count = adev.vcn.reg_count as usize;
    for i in 0..adev.vcn.num_vcn_inst as usize {
        if adev.vcn.harvest_config & (1 << i) != 0 {
            Line::new().s("Harvested Instance:VCN").dec(i as u64).s(" Skipping dump").say();
            continue;
        }
        let inst_off = i * count;
        let is_powered = adev.vcn.ip_dump[inst_off] & UVD_POWER_STATUS__UVD_POWER_STATUS_TILES_OFF
            != UVD_POWER_STATUS__UVD_POWER_STATUS_TILES_OFF;
        if is_powered {
            Line::new().s("Active Instance:VCN").dec(i as u64).say();
            for j in 0..count {
                Line::new().s(VCN_REG_LIST_2_0[j].0).s(" ").hex(adev.vcn.ip_dump[inst_off + j]).say();
            }
        } else {
            Line::new().s("Inactive Instance:VCN").dec(i as u64).say();
        }
    }
}

// ---- the VM flush probe (not Linux) ----

/// One probe on `vcn_dec`: `emit` between PACKET_START and the scratch
/// write of the ring test. Whether the scratch arrives says whether the
/// firmware got past `emit`.
fn dec_probe(adev: &mut amdgpu_device, r: usize, name: &str, emit: impl FnOnce(&mut amdgpu_device, usize)) -> bool {
    let me = adev.rings[r].me as usize;
    let (ext, int) = (adev.vcn.inst[me].external.scratch9, &adev.vcn.inst[me].internal);
    let (cmd, scratch9) = (int.cmd, int.scratch9);
    adev.wreg32(ext, 0xCAFEDEAD);
    if crate::amdgpu_ring::amdgpu_ring_alloc(adev, r, 64) != 0 {
        return false;
    }
    rw(adev, r, packet0(cmd, 0));
    rw(adev, r, VCN_DEC_KMD_CMD | VCN_DEC_CMD_PACKET_START << 1);
    emit(adev, r);
    rw(adev, r, packet0(scratch9, 0));
    rw(adev, r, 0xDEADBEEF);
    crate::amdgpu_ring::amdgpu_ring_commit(adev, r);
    let mut ok = false;
    for _ in 0..adev.usec_timeout {
        if adev.rreg32(ext).unwrap_or(0) == 0xDEADBEEF {
            ok = true;
            break;
        }
        crate::linux::udelay(1);
    }
    let hub = adev.vmhub[adev.rings[r].vm_hub as usize];
    let eng = adev.rings[r].vm_inv_eng;
    let ack = adev.rreg32(hub.vm_inv_eng0_ack + hub.eng_distance * eng).unwrap_or(0);
    let fault = adev.rreg32(hub.vm_l2_pro_fault_status).unwrap_or(0);
    Line::new().s("vcn_dec probe ").s(name).s(": ").s(if ok { "passed" } else { "STUCK" }).s(", eng ").dec(eng as u64)
        .s(" ack ").hex(ack).s(", mmhub fault status ").hex(fault).s(", rptr ")
        .hex(rreg(adev, mmUVD_RBC_RB_RPTR, mmUVD_RBC_RB_RPTR_BASE_IDX)).s(" wptr ")
        .hex(rreg(adev, mmUVD_RBC_RB_WPTR, mmUVD_RBC_RB_WPTR_BASE_IDX)).say();
    if !ok {
        vcn_v2_0_reset(adev);
    }
    ok
}

/// What a guest job's VM flush asks of `vcn_dec`, piece by piece, on
/// VMID 0 with the page table base it already has: the semaphore, the
/// invalidation and its ack, the page table base written and read back.
pub fn vcn_v2_0_dec_flush_probe(adev: &mut amdgpu_device) {
    let Some(r) = adev.vcn.inst[0].ring_dec else { return };
    let hub = adev.vmhub[adev.rings[r].vm_hub as usize];
    let eng = adev.rings[r].vm_inv_eng;
    let sem = hub.vm_inv_eng0_sem + hub.eng_distance * eng;
    let (req, ack) = (hub.vm_inv_eng0_req + hub.eng_distance * eng, hub.vm_inv_eng0_ack + hub.eng_distance * eng);
    let (lo, hi) = (hub.ctx0_ptb_addr_lo32, hub.ctx0_ptb_addr_hi32);
    let (ptb_lo, ptb_hi) = (adev.rreg32(lo).unwrap_or(0), adev.rreg32(hi).unwrap_or(0));
    Line::new().s("vcn_dec probe: hub ").dec(adev.rings[r].vm_hub as u64).s(" sem ").hexu(sem).s(" req ").hexu(req)
        .s(" ack ").hexu(ack).s(" ptb ").hex(ptb_hi).s(":").hex(ptb_lo).s(", semaphore ")
        .s(if crate::gmc_v9_0::gmc_v9_0_use_invalidate_semaphore(adev, adev.rings[r].vm_hub) { "used" } else { "not used" })
        .say();
    dec_probe(adev, r, "empty", |_, _| {});
    dec_probe(adev, r, "wreg scratch", |a, r| crate::amdgpu_ring::amdgpu_ring_emit_wreg(a, r, lo, ptb_lo));
    dec_probe(adev, r, "reg_wait ptb", |a, r| crate::amdgpu_ring::amdgpu_ring_emit_reg_wait(a, r, lo, ptb_lo, 0xffffffff));
    dec_probe(adev, r, "semaphore", |a, r| {
        crate::amdgpu_ring::amdgpu_ring_emit_reg_wait(a, r, sem, 1, 1);
        crate::amdgpu_ring::amdgpu_ring_emit_wreg(a, r, sem, 0);
    });
    let inv = crate::gmc_v9_0::gmc_v9_0_get_invalidate_req(0, 0);
    dec_probe(adev, r, "invalidate", |a, r| {
        crate::amdgpu_ring::amdgpu_ring_emit_reg_write_reg_wait(a, r, req, ack, inv, 1);
    });
    dec_probe(adev, r, "flush", |a, r| {
        crate::gmc_v9_0::gmc_v9_0_emit_flush_gpu_tlb(a, r, 0, (ptb_hi as u64) << 32 | ptb_lo as u64);
    });
    dec_probe(adev, r, "pasid mapping", |a, r| crate::gmc_v9_0::gmc_v9_0_emit_pasid_mapping(a, r, 0, 0));
}
