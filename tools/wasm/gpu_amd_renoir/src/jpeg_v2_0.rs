//! amdgpu/jpeg_v2_0.c and amdgpu_jpeg.c (Linux 6.18.55): the JPEG
//! block's early_init and sw_init.

use crate::amdgpu_device::amdgpu_device;
use crate::linux::Line;
use crate::regs::*;

/// `jpeg_v2_0_early_init`.
pub fn jpeg_v2_0_early_init(adev: &mut amdgpu_device) -> i32 {
    adev.jpeg.num_jpeg_inst = 1;
    adev.jpeg.num_jpeg_rings = 1;
    // jpeg_v2_0_set_dec_ring_funcs
    adev.jpeg.inst0_ring_dec_funcs = Some("jpeg_v2_0_dec_ring_vm_funcs");
    // jpeg_v2_0_set_irq_funcs
    adev.jpeg.inst0_irq.num_types = 1;
    adev.jpeg.inst0_irq.funcs = Some("jpeg_v2_0_irq_funcs");
    0
}

/// `amdgpu_jpeg_sw_init`.
fn amdgpu_jpeg_sw_init(adev: &mut amdgpu_device) -> i32 {
    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP && adev.pg_flags & AMD_PG_SUPPORT_JPEG_DPG != 0 {
        adev.jpeg.indirect_sram = true;
    }
    for i in 0..adev.jpeg.num_jpeg_inst {
        if adev.jpeg.harvest_config & (1 << i) != 0 {
            continue;
        }
        if adev.jpeg.indirect_sram {
            Line::new().s("JPEG ").dec(i as u64).s(": the DPG buffer is not ported").say();
            return -22;
        }
    }
    0
}

/// `jpeg_v2_0_sw_init`, bare metal; `amdgpu_jpeg_resume` does nothing.
pub fn jpeg_v2_0_sw_init(adev: &mut amdgpu_device) -> i32 {
    // JPEG TRAP
    let irq = adev.jpeg.inst0_irq;
    let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_VCN, VCN_2_0__SRCID__JPEG_DECODE, &irq,
        "jpeg.inst[0].irq");
    if r != 0 {
        return r;
    }
    let r = amdgpu_jpeg_sw_init(adev);
    if r != 0 {
        return r;
    }

    let Some(funcs) = crate::amdgpu_ring::ring_funcs(adev.jpeg.inst0_ring_dec_funcs) else { return -22 };
    let mut ring = crate::amdgpu_ring::amdgpu_ring::new(funcs, alloc::string::String::from("jpeg_dec"));
    ring.use_doorbell = true;
    ring.doorbell_index = (adev.doorbell_index.vcn_ring0_1 << 1) + 1;
    ring.vm_hub = AMDGPU_MMHUB0_0_;
    match crate::amdgpu_ring::amdgpu_ring_init(adev, ring, 512, crate::amdgpu_ring::amdgpu_irq_src_id::jpeg, 0,
        AMDGPU_RING_PRIO_DEFAULT) {
        Ok(k) => adev.jpeg.inst0_ring_dec = Some(k),
        Err(r) => return r,
    }

    adev.jpeg.internal_jpeg_pitch0 = mmUVD_JPEG_PITCH_INTERNAL_OFFSET;
    adev.jpeg.inst0_external_jpeg_pitch0 = adev.reg_offset[JPEG_HWIP as usize][0]
        .get(mmUVD_JPEG_PITCH_BASE_IDX as usize).copied().unwrap_or(0) + mmUVD_JPEG_PITCH;

    // amdgpu_jpeg_reg_dump_init
    adev.jpeg.ip_dump = alloc::vec![0; (adev.jpeg.num_jpeg_inst * JPEG_REG_LIST_2_0_COUNT) as usize];
    adev.jpeg.reg_count = JPEG_REG_LIST_2_0_COUNT;

    let dec0 = adev.jpeg.inst0_ring_dec;
    adev.jpeg.supported_reset = crate::amdgpu_ring::amdgpu_get_soft_full_reset_mask(adev, dec0);
    adev.jpeg.supported_reset |= AMDGPU_RESET_TYPE_PER_QUEUE;
    // amdgpu_jpeg_sysfs_reset_mask_init: no sysfs.
    0
}
