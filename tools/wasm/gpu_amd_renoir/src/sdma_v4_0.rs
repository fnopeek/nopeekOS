//! amdgpu/sdma_v4_0.c and amdgpu_sdma.c (Linux 6.18.55): the SDMA block's
//! early_init, which requests and reads the SDMA microcode, and its sw_init.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::{amdgpu_ucode_ip_version_decode, amdgpu_ucode_request, fw_source, le16, le32, page_align};
use crate::gmc_v9_0::amdgpu_irq_src;
use crate::linux::{firmware, Line};
use crate::regs::*;

/// `struct amdgpu_sdma_instance`, the part early_init fills.
#[derive(Default)]
pub struct amdgpu_sdma_instance {
    pub fw: Option<firmware>,
    pub fw_version: u32,
    pub feature_version: u32,
    pub burst_nop: bool,
    pub ring_funcs: Option<&'static str>,
    pub ring_me: u32,
    pub page_funcs: Option<&'static str>,
    pub page_me: u32,
    /// Indices into `adev.rings`.
    pub ring: Option<usize>,
    pub page: Option<usize>,
}

/// `amdgpu_sdma_init_inst_ctx`.
fn amdgpu_sdma_init_inst_ctx(inst: &mut amdgpu_sdma_instance) -> i32 {
    let Some(fw) = inst.fw.as_ref() else { return -22 };
    let (v, f) = match le16(fw, COMMON_FIRMWARE_HEADER_header_version_major) {
        1 => (le32(fw, COMMON_FIRMWARE_HEADER_ucode_version), le32(fw, SDMA_FIRMWARE_HEADER_V1_0_ucode_feature_version)),
        2 => (le32(fw, COMMON_FIRMWARE_HEADER_ucode_version), le32(fw, SDMA_FIRMWARE_HEADER_V2_0_ucode_feature_version)),
        3 => (le32(fw, COMMON_FIRMWARE_HEADER_ucode_version), le32(fw, SDMA_FIRMWARE_HEADER_V3_0_ucode_feature_version)),
        _ => return -22,
    };
    inst.fw_version = v;
    inst.feature_version = f;
    if inst.feature_version >= 20 {
        inst.burst_nop = true;
    }
    0
}

/// `amdgpu_sdma_destroy_inst_ctx`.
fn amdgpu_sdma_destroy_inst_ctx(adev: &mut amdgpu_device) {
    for i in adev.sdma.instance.iter_mut() {
        *i = amdgpu_sdma_instance::default();
    }
}

/// `amdgpu_sdma_init_microcode`, one image per instance (`duplicate`
/// false: SDMA 4.1.2 is not Arcturus or Aldebaran).
fn amdgpu_sdma_init_microcode(adev: &mut amdgpu_device, instance: u32) -> i32 {
    let ucode_prefix = amdgpu_ucode_ip_version_decode(adev, SDMA0_HWIP);
    let name = if instance == 0 {
        alloc::format!("amdgpu/{}.bin", ucode_prefix)
    } else {
        alloc::format!("amdgpu/{}{}.bin", ucode_prefix, instance)
    };
    let mut err;
    'out: {
        let (fw, e) = amdgpu_ucode_request(&name);
        adev.sdma.instance[instance as usize].fw = fw;
        err = e;
        if err != 0 {
            break 'out;
        }
        let Some(fw) = adev.sdma.instance[instance as usize].fw.as_ref() else { break 'out };
        let version_major = le16(fw, COMMON_FIRMWARE_HEADER_header_version_major);
        let ucode_size_bytes = le32(fw, COMMON_FIRMWARE_HEADER_ucode_size_bytes);
        if version_major > 1 {
            err = -22;
            break 'out;
        }
        err = amdgpu_sdma_init_inst_ctx(&mut adev.sdma.instance[instance as usize]);
        if err != 0 {
            break 'out;
        }
        Line::new().s("psp_load == '").s(if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP { "true" } else { "false" })
            .s("'").dbg();
        if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
            // version 1, SDMA 4.1.2: one image per instance
            for i in 0..adev.sdma.num_instances {
                if instance != i {
                    continue;
                }
                let id = AMDGPU_UCODE_ID_SDMA0 + i;
                let info = &mut adev.firmware.ucode[id as usize];
                info.ucode_id = id;
                info.fw = Some(fw_source::sdma(i));
                adev.firmware.fw_size = adev.firmware.fw_size.wrapping_add(page_align(ucode_size_bytes));
            }
        }
    }
    if err != 0 {
        amdgpu_sdma_destroy_inst_ctx(adev);
    }
    err
}

/// `sdma_v4_0_init_microcode`.
fn sdma_v4_0_init_microcode(adev: &mut amdgpu_device) -> i32 {
    let mut ret = 0;
    for i in 0..adev.sdma.num_instances {
        ret = amdgpu_sdma_init_microcode(adev, i);
        if ret != 0 {
            return ret;
        }
    }
    ret
}

/// `sdma_v4_0_early_init`. SDMA 4.1.2: no paging queue
/// (`sdma_v4_0_fw_support_paging_queue`), no RAS funcs.
pub fn sdma_v4_0_early_init(adev: &mut amdgpu_device) -> i32 {
    let r = sdma_v4_0_init_microcode(adev);
    if r != 0 {
        return r;
    }
    adev.sdma.has_page_queue = false;

    // sdma_v4_0_set_ring_funcs
    for i in 0..adev.sdma.num_instances as usize {
        adev.sdma.instance[i].ring_funcs = Some("sdma_v4_0_ring_funcs");
        adev.sdma.instance[i].ring_me = i as u32;
    }
    // sdma_v4_0_set_buffer_funcs
    adev.mman.buffer_funcs = Some("sdma_v4_0_buffer_funcs");
    adev.mman.buffer_funcs_ring = Some("sdma.instance[0].ring");
    // sdma_v4_0_set_vm_pte_funcs
    adev.vm_manager.vm_pte_funcs = Some("sdma_v4_0_vm_pte_funcs");
    adev.vm_manager.vm_pte_num_scheds = adev.sdma.num_instances;
    // sdma_v4_0_set_irq_funcs
    let n = adev.sdma.num_instances;
    adev.sdma.trap_irq = amdgpu_irq_src { num_types: n, funcs: Some("sdma_v4_0_trap_irq_funcs") };
    adev.sdma.ecc_irq = amdgpu_irq_src { num_types: n, funcs: Some("sdma_v4_0_ecc_irq_funcs") };
    adev.sdma.illegal_inst_irq.funcs = Some("sdma_v4_0_illegal_inst_irq_funcs");
    adev.sdma.vm_hole_irq.funcs = Some("sdma_v4_0_vm_hole_irq_funcs");
    adev.sdma.doorbell_invalid_irq.funcs = Some("sdma_v4_0_doorbell_invalid_irq_funcs");
    adev.sdma.pool_timeout_irq.funcs = Some("sdma_v4_0_pool_timeout_irq_funcs");
    adev.sdma.srbm_write_irq.funcs = Some("sdma_v4_0_srbm_write_irq_funcs");
    0
}

/// `sdma_v4_0_seq_to_irq_id`.
fn sdma_v4_0_seq_to_irq_id(seq_num: u32) -> u32 {
    match seq_num {
        0 => SOC15_IH_CLIENTID_SDMA0,
        1 => SOC15_IH_CLIENTID_SDMA1,
        _ => (-22i32) as u32,
    }
}

/// `sdma_v4_0_sw_init`: SDMA 4.1.2, one instance on MMHUB 0, no page
/// queue, no RAS block.
pub fn sdma_v4_0_sw_init(adev: &mut amdgpu_device) -> i32 {
    use crate::amdgpu_irq::amdgpu_irq_add_id;
    let n = adev.sdma.num_instances;
    let trap = adev.sdma.trap_irq;
    let ecc = adev.sdma.ecc_irq;
    let vm_hole = adev.sdma.vm_hole_irq;
    let doorbell_invalid = adev.sdma.doorbell_invalid_irq;
    let pool_timeout = adev.sdma.pool_timeout_irq;
    let srbm_write = adev.sdma.srbm_write_irq;
    // SDMA trap event
    for i in 0..n {
        let r = amdgpu_irq_add_id(adev, sdma_v4_0_seq_to_irq_id(i), SDMA0_4_0__SRCID__SDMA_TRAP, &trap, "sdma.trap_irq");
        if r != 0 {
            return r;
        }
    }
    // SDMA SRAM ECC event
    for i in 0..n {
        let r = amdgpu_irq_add_id(adev, sdma_v4_0_seq_to_irq_id(i), SDMA0_4_0__SRCID__SDMA_SRAM_ECC, &ecc, "sdma.ecc_irq");
        if r != 0 {
            return r;
        }
    }
    // SDMA VM_HOLE/DOORBELL_INV/POLL_TIMEOUT/SRBM_WRITE_PROTECTION event
    for i in 0..n {
        for (src_id, src, name) in [
            (SDMA0_4_0__SRCID__SDMA_VM_HOLE, &vm_hole, "sdma.vm_hole_irq"),
            (SDMA0_4_0__SRCID__SDMA_DOORBELL_INVALID, &doorbell_invalid, "sdma.doorbell_invalid_irq"),
            (SDMA0_4_0__SRCID__SDMA_POLL_TIMEOUT, &pool_timeout, "sdma.pool_timeout_irq"),
            (SDMA0_4_0__SRCID__SDMA_SRBMWRITE, &srbm_write, "sdma.srbm_write_irq"),
        ] {
            let r = amdgpu_irq_add_id(adev, sdma_v4_0_seq_to_irq_id(i), src_id, src, name);
            if r != 0 {
                return r;
            }
        }
    }

    if adev.ip_version(SDMA0_HWIP, 0) == 4 << 24 | 2 << 16 | 2 << 8 || adev.sdma.has_page_queue {
        Line::new().s("sdma_v4_0_sw_init: Arcturus hubs and page queues are not ported").say();
        return -22;
    }
    for i in 0..n as usize {
        let Some(funcs) = crate::amdgpu_ring::ring_funcs(adev.sdma.instance[i].ring_funcs) else { return -22 };
        let mut ring = crate::amdgpu_ring::amdgpu_ring::new(funcs, alloc::format!("sdma{}", i));
        ring.me = adev.sdma.instance[i].ring_me;
        ring.use_doorbell = true;
        // doorbell size is 2 dwords, get DWORD offset
        ring.doorbell_index = adev.doorbell_index.sdma_engine[i] << 1;
        ring.vm_hub = AMDGPU_MMHUB0_0_;
        match crate::amdgpu_ring::amdgpu_ring_init(adev, ring, 1024, crate::amdgpu_ring::amdgpu_irq_src_id::sdma_trap,
            AMDGPU_SDMA_IRQ_INSTANCE0 + i as u32, AMDGPU_RING_PRIO_DEFAULT) {
            Ok(k) => adev.sdma.instance[i].ring = Some(k),
            Err(r) => return r,
        }
    }
    // amdgpu_sdma_ras_sw_init: no adev->sdma.ras.
    // Allocate memory for SDMA IP Dump buffer
    adev.sdma.ip_dump = alloc::vec![0; (n * SDMA_REG_LIST_4_0_COUNT) as usize];
    0
}
