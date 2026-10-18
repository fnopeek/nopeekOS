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
