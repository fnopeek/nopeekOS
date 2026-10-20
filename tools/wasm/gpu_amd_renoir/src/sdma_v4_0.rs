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

// ---- hw_init (stage 4) ----

/// `sdma_v4_0_get_reg_offset`: Renoir has one instance.
fn sdma_v4_0_get_reg_offset(adev: &amdgpu_device, instance: u32, offset: u32) -> u32 {
    match instance {
        0 => adev.reg_offset[SDMA0_HWIP as usize][0][0] + offset,
        1 => adev.reg_offset[SDMA1_HWIP as usize][0][0] + offset,
        _ => 0,
    }
}

/// `RREG32_SDMA(i, reg)`.
fn rd(adev: &amdgpu_device, i: u32, reg: u32) -> u32 {
    adev.rreg32(sdma_v4_0_get_reg_offset(adev, i, reg)).unwrap_or(0)
}

/// `WREG32_SDMA(i, reg, v)`.
fn wr(adev: &amdgpu_device, i: u32, reg: u32, v: u32) {
    adev.wreg32(sdma_v4_0_get_reg_offset(adev, i, reg), v);
}

/// `REG_SET_FIELD`.
fn set_field(v: u32, mask: u32, shift: u32, f: u32) -> u32 {
    (v & !mask) | (mask & (f << shift))
}

/// `sdma_v4_0_init_golden_registers`: SDMA 4.1.2, `golden_settings_sdma_4_3`.
fn sdma_v4_0_init_golden_registers(adev: &amdgpu_device) {
    if adev.ip_version(SDMA0_HWIP, 0) == 4 << 24 | 1 << 16 | 2 << 8 {
        crate::gfx_v9_0::soc15_program_register_sequence(adev, &crate::tables::golden_settings_sdma_4_3);
    }
}

/// `sdma_v4_0_ctx_switch_enable`. `amdgpu_sdma_phase_quantum` is 32.
fn sdma_v4_0_ctx_switch_enable(adev: &amdgpu_device, enable: bool) {
    let mut phase_quantum = 0;
    let q = crate::tables::amdgpu_sdma_phase_quantum[0];
    if q != 0 {
        let mut value = q;
        let mut unit = 0;
        let vmax = SDMA0_SDMA0_PHASE0_QUANTUM__VALUE_MASK >> SDMA0_SDMA0_PHASE0_QUANTUM__VALUE__SHIFT;
        let umax = SDMA0_SDMA0_PHASE0_QUANTUM__UNIT_MASK >> SDMA0_SDMA0_PHASE0_QUANTUM__UNIT__SHIFT;
        while value > vmax {
            value = (value + 1) >> 1;
            unit += 1;
        }
        if unit > umax {
            value = vmax;
            unit = umax;
            Line::new().s("clamping sdma_phase_quantum to ").dec((value << unit) as u64).s("K clock cycles").say();
        }
        phase_quantum = value << SDMA0_SDMA0_PHASE0_QUANTUM__VALUE__SHIFT | unit << SDMA0_SDMA0_PHASE0_QUANTUM__UNIT__SHIFT;
    }
    for i in 0..adev.sdma.num_instances {
        let mut f32_cntl = rd(adev, i, SDMA0_mmSDMA0_CNTL);
        f32_cntl = set_field(f32_cntl, SDMA0_SDMA0_CNTL__AUTO_CTXSW_ENABLE_MASK, SDMA0_SDMA0_CNTL__AUTO_CTXSW_ENABLE__SHIFT,
                             enable as u32);
        if enable && q != 0 {
            wr(adev, i, SDMA0_mmSDMA0_PHASE0_QUANTUM, phase_quantum);
            wr(adev, i, SDMA0_mmSDMA0_PHASE1_QUANTUM, phase_quantum);
            wr(adev, i, SDMA0_mmSDMA0_PHASE2_QUANTUM, phase_quantum);
        }
        wr(adev, i, SDMA0_mmSDMA0_CNTL, f32_cntl);
        // Enable SDMA utilization. Its only supported on Arcturus for the
        // moment and firmware version 14 and above.
        if adev.ip_version(SDMA0_HWIP, 0) == 4 << 24 | 2 << 16 | 2 << 8
            && adev.sdma.instance[i as usize].fw_version >= 14
        {
            wr(adev, i, SDMA0_mmSDMA0_PUB_DUMMY_REG2, enable as u32);
        }
        // Extend page fault timeout to avoid interrupt storm
        wr(adev, i, SDMA0_mmSDMA0_UTCL1_TIMEOUT, 0x00800080);
    }
}

/// `sdma_v4_0_enable(adev, true)`: unhalt the MEs.
fn sdma_v4_0_enable(adev: &amdgpu_device) {
    for i in 0..adev.sdma.num_instances {
        let mut f32_cntl = rd(adev, i, SDMA0_mmSDMA0_F32_CNTL);
        f32_cntl = set_field(f32_cntl, SDMA0_SDMA0_F32_CNTL__HALT_MASK, SDMA0_SDMA0_F32_CNTL__HALT__SHIFT, 0);
        wr(adev, i, SDMA0_mmSDMA0_F32_CNTL, f32_cntl);
    }
}

/// `order_base_2`.
fn order_base_2(n: u32) -> u32 {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() }
}

/// `sdma_v4_0_gfx_resume`: bare metal, so no F32 wptr polling.
fn sdma_v4_0_gfx_resume(adev: &mut amdgpu_device, i: u32) {
    let Some(r) = adev.sdma.instance[i as usize].ring else { return };
    let mut rb_cntl = rd(adev, i, SDMA0_mmSDMA0_GFX_RB_CNTL);
    // sdma_v4_0_rb_cntl: set ring buffer size in dwords
    let rb_bufsz = order_base_2(adev.rings[r].ring_size / 4);
    rb_cntl = set_field(rb_cntl, SDMA0_SDMA0_GFX_RB_CNTL__RB_SIZE_MASK, SDMA0_SDMA0_GFX_RB_CNTL__RB_SIZE__SHIFT, rb_bufsz);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_CNTL, rb_cntl);

    // Initialize the ring buffer's read and write pointers
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_RPTR, 0);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_RPTR_HI, 0);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_WPTR, 0);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_WPTR_HI, 0);

    // set the wb address whether it's enabled or not
    let ring = &adev.rings[r];
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_RPTR_ADDR_HI, (ring.rptr_gpu_addr >> 32) as u32);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_RPTR_ADDR_LO, ring.rptr_gpu_addr as u32 & 0xFFFFFFFC);

    rb_cntl = set_field(rb_cntl, SDMA0_SDMA0_GFX_RB_CNTL__RPTR_WRITEBACK_ENABLE_MASK,
                        SDMA0_SDMA0_GFX_RB_CNTL__RPTR_WRITEBACK_ENABLE__SHIFT, 1);

    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_BASE, (ring.gpu_addr >> 8) as u32);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_BASE_HI, (ring.gpu_addr >> 40) as u32);
    let (use_doorbell, doorbell_index, wptr_gpu_addr) = (ring.use_doorbell, ring.doorbell_index, ring.wptr_gpu_addr);

    adev.rings[r].wptr = 0;

    // before programing wptr to a less value, need set minor_ptr_update
    // first
    wr(adev, i, SDMA0_mmSDMA0_GFX_MINOR_PTR_UPDATE, 1);

    let mut doorbell = rd(adev, i, SDMA0_mmSDMA0_GFX_DOORBELL);
    let mut doorbell_offset = rd(adev, i, SDMA0_mmSDMA0_GFX_DOORBELL_OFFSET);
    doorbell = set_field(doorbell, SDMA0_SDMA0_GFX_DOORBELL__ENABLE_MASK, SDMA0_SDMA0_GFX_DOORBELL__ENABLE__SHIFT,
                         use_doorbell as u32);
    doorbell_offset = set_field(doorbell_offset, SDMA0_SDMA0_GFX_DOORBELL_OFFSET__OFFSET_MASK,
                                SDMA0_SDMA0_GFX_DOORBELL_OFFSET__OFFSET__SHIFT, doorbell_index);
    wr(adev, i, SDMA0_mmSDMA0_GFX_DOORBELL, doorbell);
    wr(adev, i, SDMA0_mmSDMA0_GFX_DOORBELL_OFFSET, doorbell_offset);

    sdma_v4_0_ring_set_wptr(adev, r);

    // set minor_ptr_update to 0 after wptr programed
    wr(adev, i, SDMA0_mmSDMA0_GFX_MINOR_PTR_UPDATE, 0);

    // setup the wptr shadow polling
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_WPTR_POLL_ADDR_LO, wptr_gpu_addr as u32);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_WPTR_POLL_ADDR_HI, (wptr_gpu_addr >> 32) as u32);
    let mut wptr_poll_cntl = rd(adev, i, SDMA0_mmSDMA0_GFX_RB_WPTR_POLL_CNTL);
    wptr_poll_cntl = set_field(wptr_poll_cntl, SDMA0_SDMA0_GFX_RB_WPTR_POLL_CNTL__F32_POLL_ENABLE_MASK,
                               SDMA0_SDMA0_GFX_RB_WPTR_POLL_CNTL__F32_POLL_ENABLE__SHIFT, 0);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_WPTR_POLL_CNTL, wptr_poll_cntl);

    // enable DMA RB
    rb_cntl = set_field(rb_cntl, SDMA0_SDMA0_GFX_RB_CNTL__RB_ENABLE_MASK, SDMA0_SDMA0_GFX_RB_CNTL__RB_ENABLE__SHIFT, 1);
    wr(adev, i, SDMA0_mmSDMA0_GFX_RB_CNTL, rb_cntl);

    let mut ib_cntl = rd(adev, i, SDMA0_mmSDMA0_GFX_IB_CNTL);
    ib_cntl = set_field(ib_cntl, SDMA0_SDMA0_GFX_IB_CNTL__IB_ENABLE_MASK, SDMA0_SDMA0_GFX_IB_CNTL__IB_ENABLE__SHIFT, 1);
    // enable DMA IBs
    wr(adev, i, SDMA0_mmSDMA0_GFX_IB_CNTL, ib_cntl);
}

/// `RREG32(SOC15_REG_OFFSET(SDMA0, 0, reg))`.
fn rd0(adev: &amdgpu_device, idx: u32, reg: u32) -> u32 {
    crate::nbio_v7_0::rreg32_soc15(adev, SDMA0_HWIP, 0, idx, reg).unwrap_or(0)
}

/// `WREG32(SOC15_REG_OFFSET(SDMA0, 0, reg), v)`.
fn wr0(adev: &amdgpu_device, idx: u32, reg: u32, v: u32) {
    crate::nbio_v7_0::wreg32_soc15(adev, SDMA0_HWIP, 0, idx, reg, v);
}

/// `sdma_v4_1_init_power_gating`.
fn sdma_v4_1_init_power_gating(adev: &amdgpu_device) {
    // Enable HW based PG.
    let def = rd0(adev, SDMA0_mmSDMA0_POWER_CNTL_BASE_IDX, SDMA0_mmSDMA0_POWER_CNTL);
    let data = def | SDMA0_SDMA0_POWER_CNTL__PG_CNTL_ENABLE_MASK;
    if data != def {
        wr0(adev, SDMA0_mmSDMA0_POWER_CNTL_BASE_IDX, SDMA0_mmSDMA0_POWER_CNTL, data);
    }
    // enable interrupt
    let def = rd0(adev, SDMA0_mmSDMA0_CNTL_BASE_IDX, SDMA0_mmSDMA0_CNTL);
    let data = def | SDMA0_SDMA0_CNTL__CTXEMPTY_INT_ENABLE_MASK;
    if data != def {
        wr0(adev, SDMA0_mmSDMA0_CNTL_BASE_IDX, SDMA0_mmSDMA0_CNTL, data);
    }
    // Configure hold time to filter in-valid power on/off request. Use
    // default right now
    let def = rd0(adev, SDMA0_mmSDMA0_POWER_CNTL_BASE_IDX, SDMA0_mmSDMA0_POWER_CNTL);
    let mut data = def & !SDMA0_SDMA0_POWER_CNTL__ON_OFF_CONDITION_HOLD_TIME_MASK;
    data |= SDMA0_mmSDMA0_POWER_CNTL_DEFAULT & SDMA0_SDMA0_POWER_CNTL__ON_OFF_CONDITION_HOLD_TIME_MASK;
    // Configure switch time for hysteresis purpose. Use default right now
    data &= !SDMA0_SDMA0_POWER_CNTL__ON_OFF_STATUS_DURATION_TIME_MASK;
    data |= SDMA0_mmSDMA0_POWER_CNTL_DEFAULT & SDMA0_SDMA0_POWER_CNTL__ON_OFF_STATUS_DURATION_TIME_MASK;
    if data != def {
        wr0(adev, SDMA0_mmSDMA0_POWER_CNTL_BASE_IDX, SDMA0_mmSDMA0_POWER_CNTL, data);
    }
}

/// `sdma_v4_1_update_power_gating`.
fn sdma_v4_1_update_power_gating(adev: &amdgpu_device, enable: bool) {
    let def = rd0(adev, SDMA0_mmSDMA0_CNTL_BASE_IDX, SDMA0_mmSDMA0_CNTL);
    let data = if enable && adev.pg_flags & AMD_PG_SUPPORT_SDMA != 0 {
        // enable idle interrupt
        def | SDMA0_SDMA0_CNTL__CTXEMPTY_INT_ENABLE_MASK
    } else {
        // disable idle interrupt
        def & !SDMA0_SDMA0_CNTL__CTXEMPTY_INT_ENABLE_MASK
    };
    if data != def {
        wr0(adev, SDMA0_mmSDMA0_CNTL_BASE_IDX, SDMA0_mmSDMA0_CNTL, data);
    }
}

/// `sdma_v4_0_rlc_resume`: `sdma_v4_0_init_pg`.
fn sdma_v4_0_rlc_resume(adev: &amdgpu_device) -> i32 {
    if adev.pg_flags & AMD_PG_SUPPORT_SDMA == 0 {
        return 0;
    }
    let v = adev.ip_version(SDMA0_HWIP, 0);
    if v == 4 << 24 | 1 << 16 || v == 4 << 24 | 1 << 16 | 1 << 8 || v == 4 << 24 | 1 << 16 | 2 << 8 {
        sdma_v4_1_init_power_gating(adev);
        sdma_v4_1_update_power_gating(adev, true);
    }
    0
}

/// `sdma_v4_0_start`: bare metal, firmware from the PSP, no page queue.
fn sdma_v4_0_start(adev: &mut amdgpu_device) -> i32 {
    // unhalt the MEs
    sdma_v4_0_enable(adev);
    // enable sdma ring preemption
    sdma_v4_0_ctx_switch_enable(adev, true);

    // start the gfx rings and rlc compute queues
    for i in 0..adev.sdma.num_instances {
        wr(adev, i, SDMA0_mmSDMA0_SEM_WAIT_FAIL_TIMER_CNTL, 0);
        sdma_v4_0_gfx_resume(adev, i);
        // set utc l1 enable flag always to 1
        let mut temp = rd(adev, i, SDMA0_mmSDMA0_CNTL);
        temp = set_field(temp, SDMA0_SDMA0_CNTL__UTC_L1_ENABLE_MASK, SDMA0_SDMA0_CNTL__UTC_L1_ENABLE__SHIFT, 1);
        wr(adev, i, SDMA0_mmSDMA0_CNTL, temp);
        // unhalt engine
        let mut temp = rd(adev, i, SDMA0_mmSDMA0_F32_CNTL);
        temp = set_field(temp, SDMA0_SDMA0_F32_CNTL__HALT_MASK, SDMA0_SDMA0_F32_CNTL__HALT__SHIFT, 0);
        wr(adev, i, SDMA0_mmSDMA0_F32_CNTL, temp);
    }

    let r = sdma_v4_0_rlc_resume(adev);
    if r != 0 {
        return r;
    }

    for i in 0..adev.sdma.num_instances as usize {
        let Some(ring) = adev.sdma.instance[i].ring else { continue };
        let r = crate::amdgpu_ring::amdgpu_ring_test_helper(adev, ring);
        if r != 0 {
            return r;
        }
    }
    0
}

/// `sdma_v4_0_hw_init`: an APU, so the SMU powers SDMA up first.
pub fn sdma_v4_0_hw_init(adev: &mut amdgpu_device) -> i32 {
    if adev.flags & AMD_IS_APU != 0 {
        crate::amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_SDMA, false, 0);
    }
    sdma_v4_0_init_golden_registers(adev);
    sdma_v4_0_start(adev)
}

/// `amdgpu_sdma_get_instance_from_ring`.
fn amdgpu_sdma_get_instance_from_ring(adev: &amdgpu_device, r: usize) -> Option<usize> {
    (0..adev.sdma.num_instances as usize)
        .find(|&i| adev.sdma.instance[i].ring == Some(r) || adev.sdma.instance[i].page == Some(r))
}

/// `sdma_v4_0_ring_insert_nop`.
pub fn sdma_v4_0_ring_insert_nop(adev: &mut amdgpu_device, r: usize, count: u32) {
    let burst_nop = amdgpu_sdma_get_instance_from_ring(adev, r).is_some_and(|i| adev.sdma.instance[i].burst_nop);
    let nop = adev.rings[r].funcs.nop;
    for i in 0..count {
        if burst_nop && i == 0 {
            crate::amdgpu_ring::amdgpu_ring_write(adev, r, nop | ((count - 1) & SDMA_PKT_NOP_HEADER_count_mask) << SDMA_PKT_NOP_HEADER_count_shift);
        } else {
            crate::amdgpu_ring::amdgpu_ring_write(adev, r, nop);
        }
    }
}

/// `sdma_v4_0_ring_set_wptr`.
pub fn sdma_v4_0_ring_set_wptr(adev: &mut amdgpu_device, r: usize) {
    let ring = &adev.rings[r];
    let (wptr, db, me) = (ring.wptr, ring.doorbell_index, ring.me);
    if ring.use_doorbell {
        // XXX check if swapping is necessary on BE
        crate::amdgpu_ring::ring_wptr_cpu_set(adev, r, wptr << 2);
        crate::amdgpu_ring::amdgpu_mm_wdoorbell64(adev, db, wptr << 2);
    } else {
        wr(adev, me, SDMA0_mmSDMA0_GFX_RB_WPTR, (wptr << 2) as u32);
        wr(adev, me, SDMA0_mmSDMA0_GFX_RB_WPTR_HI, ((wptr << 2) >> 32) as u32);
    }
}

/// `sdma_v4_0_page_ring_set_wptr`: Renoir has no page queue.
pub fn sdma_v4_0_page_ring_set_wptr(_adev: &mut amdgpu_device, _r: usize) {
    Line::new().s("sdma_v4_0_page_ring_set_wptr: no page queue on SDMA 4.1.2").say();
}

/// `sdma_v4_0_ring_test_ring`.
pub fn sdma_v4_0_ring_test_ring(adev: &mut amdgpu_device, r: usize) -> i32 {
    let index = match crate::amdgpu_ring::amdgpu_device_wb_get(adev) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let gpu_addr = adev.wb.gpu_addr + index as u64 * 4;
    let Some(wb) = adev.wb.wb else { return -22 };
    crate::amdgpu_gart::kptr_w32(adev, wb, index as u64 * 4, 0xCAFEDEAD);

    let mut ret = crate::amdgpu_ring::amdgpu_ring_alloc(adev, r, 5);
    if ret == 0 {
        let w = |adev: &mut amdgpu_device, v: u32| crate::amdgpu_ring::amdgpu_ring_write(adev, r, v);
        w(adev, SDMA_PKT_HEADER_OP_WRITE | SDMA_PKT_HEADER_SUB_OP_WRITE_LINEAR);
        w(adev, gpu_addr as u32);
        w(adev, (gpu_addr >> 32) as u32);
        w(adev, (0 & SDMA_PKT_WRITE_UNTILED_DW_3_count_mask) << SDMA_PKT_WRITE_UNTILED_DW_3_count_shift);
        w(adev, 0xDEADBEEF);
        crate::amdgpu_ring::amdgpu_ring_commit(adev, r);
        let mut i = 0;
        while i < adev.usec_timeout {
            if crate::amdgpu_gart::kptr_r32(adev, wb, index as u64 * 4) == 0xDEADBEEF {
                break;
            }
            crate::linux::udelay(1);
            i += 1;
        }
        if i >= adev.usec_timeout {
            ret = -110;
        }
    }
    crate::amdgpu_ring::amdgpu_device_wb_free(adev, index);
    ret
}

// ---- fences, IBs and interrupts (stage 4) ----

use crate::amdgpu_ring::amdgpu_ring_write as rw;

/// `sdma_v4_0_ring_emit_ib`.
pub fn sdma_v4_0_ring_emit_ib(adev: &mut amdgpu_device, r: usize, vmid: u32, ib: &crate::amdgpu_ib::amdgpu_ib,
                              _flags: u32) {
    // IB packet must end on a 8 DW boundary
    let pad = (2u32.wrapping_sub(adev.rings[r].wptr as u32)) & 7;
    sdma_v4_0_ring_insert_nop(adev, r, pad);
    rw(adev, r, SDMA_PKT_HEADER_OP_INDIRECT
        | ((vmid & 0xf) & SDMA_PKT_INDIRECT_HEADER_vmid_mask) << SDMA_PKT_INDIRECT_HEADER_vmid_shift);
    // base must be 32 byte aligned
    rw(adev, r, ib.gpu_addr as u32 & 0xffffffe0);
    rw(adev, r, (ib.gpu_addr >> 32) as u32);
    rw(adev, r, ib.length_dw);
    rw(adev, r, 0);
    rw(adev, r, 0);
}

/// `sdma_v4_0_ring_emit_fence`.
pub fn sdma_v4_0_ring_emit_fence(adev: &mut amdgpu_device, r: usize, addr: u64, seq: u64, flags: u32) {
    let write64bit = flags & AMDGPU_FENCE_FLAG_64BIT != 0;
    // write the fence
    rw(adev, r, SDMA_PKT_HEADER_OP_FENCE);
    // zero in first two bits
    if addr & 0x3 != 0 {
        Line::new().s("WARNING: sdma fence address not dword aligned").say();
    }
    rw(adev, r, addr as u32);
    rw(adev, r, (addr >> 32) as u32);
    rw(adev, r, seq as u32);
    // optionally write high bits as well
    if write64bit {
        let addr = addr + 4;
        rw(adev, r, SDMA_PKT_HEADER_OP_FENCE);
        rw(adev, r, addr as u32);
        rw(adev, r, (addr >> 32) as u32);
        rw(adev, r, (seq >> 32) as u32);
    }
    // generate an interrupt
    rw(adev, r, SDMA_PKT_HEADER_OP_TRAP);
    rw(adev, r, (0 & SDMA_PKT_TRAP_INT_CONTEXT_int_context_mask) << SDMA_PKT_TRAP_INT_CONTEXT_int_context_shift);
}

/// `sdma_v4_0_wait_reg_mem`: wait until `ref` == `*addr & mask`.
fn sdma_v4_0_wait_reg_mem(adev: &mut amdgpu_device, r: usize, mem_space: u32, hdp: u32, addr0: u32, addr1: u32,
                          ref_: u32, mask: u32, inv: u32) {
    rw(adev, r, SDMA_PKT_HEADER_OP_POLL_REGMEM
        | (hdp & SDMA_PKT_POLL_REGMEM_HEADER_hdp_flush_mask) << SDMA_PKT_POLL_REGMEM_HEADER_hdp_flush_shift
        | (mem_space & SDMA_PKT_POLL_REGMEM_HEADER_mem_poll_mask) << SDMA_PKT_POLL_REGMEM_HEADER_mem_poll_shift
        | SDMA_PKT_POLL_REGMEM_HEADER_FUNC_3); // ==
    if mem_space != 0 {
        // memory
        rw(adev, r, addr0);
        rw(adev, r, addr1);
    } else {
        // registers
        rw(adev, r, addr0 << 2);
        rw(adev, r, addr1 << 2);
    }
    rw(adev, r, ref_); // reference
    rw(adev, r, mask); // mask
    rw(adev, r, SDMA_PKT_POLL_REGMEM_DW5_RETRY_COUNT_FFF
        | (inv & SDMA_PKT_POLL_REGMEM_DW5_interval_mask) << SDMA_PKT_POLL_REGMEM_DW5_interval_shift); // retry count, poll interval
}

/// `sdma_v4_0_ring_emit_pipeline_sync`: wait for the ring's last fence.
pub fn sdma_v4_0_ring_emit_pipeline_sync(adev: &mut amdgpu_device, r: usize) {
    let seq = adev.rings[r].fence_drv.sync_seq;
    let addr = adev.rings[r].fence_drv.gpu_addr;
    // wait for idle
    sdma_v4_0_wait_reg_mem(adev, r, 1, 0, addr as u32 & 0xfffffffc, (addr >> 32) as u32, seq, 0xffffffff, 4);
}

/// `sdma_v4_0_ring_emit_vm_flush`.
pub fn sdma_v4_0_ring_emit_vm_flush(adev: &mut amdgpu_device, r: usize, vmid: u32, pd_addr: u64) {
    crate::gmc_v9_0::gmc_v9_0_emit_flush_gpu_tlb(adev, r, vmid, pd_addr);
}

/// `sdma_v4_0_ring_emit_wreg`.
pub fn sdma_v4_0_ring_emit_wreg(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32) {
    rw(adev, r, SDMA_PKT_HEADER_OP_SRBM_WRITE | SDMA_PKT_SRBM_WRITE_HEADER_BYTE_EN_F as u32);
    rw(adev, r, reg);
    rw(adev, r, val);
}

/// `sdma_v4_0_ring_emit_reg_wait`.
pub fn sdma_v4_0_ring_emit_reg_wait(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32, mask: u32) {
    sdma_v4_0_wait_reg_mem(adev, r, 0, 0, reg, 0, val, mask, 10);
}

/// `sdma_v4_0_ring_test_ib`.
pub fn sdma_v4_0_ring_test_ib(adev: &mut amdgpu_device, r: usize, timeout: i64) -> i32 {
    use crate::amdgpu_ib::{amdgpu_ib, amdgpu_ib_free, amdgpu_ib_get, amdgpu_ib_schedule};
    let index = match crate::amdgpu_ring::amdgpu_device_wb_get(adev) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let gpu_addr = adev.wb.gpu_addr + index as u64 * 4;
    let Some(wb) = adev.wb.wb else { return -22 };
    crate::amdgpu_gart::kptr_w32(adev, wb, index as u64 * 4, 0xCAFEDEAD);
    let mut ib = amdgpu_ib::new();
    let mut ret = amdgpu_ib_get(adev, 256, AMDGPU_IB_POOL_DIRECT, &mut ib);
    if ret == 0 {
        ib.set(adev, 0, SDMA_PKT_HEADER_OP_WRITE | SDMA_PKT_HEADER_SUB_OP_WRITE_LINEAR);
        ib.set(adev, 1, gpu_addr as u32);
        ib.set(adev, 2, (gpu_addr >> 32) as u32);
        ib.set(adev, 3, (0 & SDMA_PKT_WRITE_UNTILED_DW_3_count_mask) << SDMA_PKT_WRITE_UNTILED_DW_3_count_shift);
        ib.set(adev, 4, 0xDEADBEEF);
        ib.set(adev, 5, SDMA_PKT_NOP_HEADER_OP_NOP);
        ib.set(adev, 6, SDMA_PKT_NOP_HEADER_OP_NOP);
        ib.set(adev, 7, SDMA_PKT_NOP_HEADER_OP_NOP);
        ib.length_dw = 8;
        ret = match amdgpu_ib_schedule(adev, r, core::slice::from_ref(&ib), None) {
            Err(e) => e,
            Ok(f) => {
                let t = crate::dma_fence::dma_fence_wait_timeout(adev, &f, timeout);
                if t == 0 {
                    -110
                } else if crate::amdgpu_gart::kptr_r32(adev, wb, index as u64 * 4) == 0xDEADBEEF {
                    0
                } else {
                    -22
                }
            }
        };
        amdgpu_ib_free(adev, &mut ib, None);
    }
    crate::amdgpu_ring::amdgpu_device_wb_free(adev, index);
    ret
}

/// `sdma_v4_0_set_trap_irq_state`.
pub fn sdma_v4_0_set_trap_irq_state(adev: &mut amdgpu_device, type_: u32, state: u32) -> i32 {
    let mut sdma_cntl = rd(adev, type_, SDMA0_mmSDMA0_CNTL);
    sdma_cntl = set_field(sdma_cntl, SDMA0_SDMA0_CNTL__TRAP_ENABLE_MASK, SDMA0_SDMA0_CNTL__TRAP_ENABLE__SHIFT,
                          (state == AMDGPU_IRQ_STATE_ENABLE) as u32);
    wr(adev, type_, SDMA0_mmSDMA0_CNTL, sdma_cntl);
    0
}

/// `sdma_v4_0_set_ecc_irq_state`.
pub fn sdma_v4_0_set_ecc_irq_state(adev: &mut amdgpu_device, type_: u32, state: u32) -> i32 {
    let mut cfg = rd(adev, type_, SDMA0_mmSDMA0_EDC_CONFIG);
    cfg = set_field(cfg, SDMA0_SDMA0_EDC_CONFIG__ECC_INT_ENABLE_MASK, SDMA0_SDMA0_EDC_CONFIG__ECC_INT_ENABLE__SHIFT,
                    (state == AMDGPU_IRQ_STATE_ENABLE) as u32);
    wr(adev, type_, SDMA0_mmSDMA0_EDC_CONFIG, cfg);
    0
}

/// `sdma_v4_0_irq_id_to_seq`.
fn sdma_v4_0_irq_id_to_seq(client_id: u32) -> i32 {
    match client_id {
        SOC15_IH_CLIENTID_SDMA0 => 0,
        SOC15_IH_CLIENTID_SDMA1 => 1,
        SOC15_IH_CLIENTID_SDMA2 => 2,
        SOC15_IH_CLIENTID_SDMA3 => 3,
        SOC15_IH_CLIENTID_SDMA4 => 4,
        SOC15_IH_CLIENTID_SDMA5 => 5,
        SOC15_IH_CLIENTID_SDMA6 => 6,
        SOC15_IH_CLIENTID_SDMA7 => 7,
        _ => -22,
    }
}

/// `sdma_v4_0_process_trap_irq`.
pub fn sdma_v4_0_process_trap_irq(adev: &mut amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry) -> i32 {
    Line::new().s("IH: SDMA trap").dbg();
    let instance = sdma_v4_0_irq_id_to_seq(entry.client_id);
    if instance < 0 {
        return instance;
    }
    let inst = &adev.sdma.instance[instance as usize];
    let v = adev.ip_version(SDMA0_HWIP, 0);
    let ring = match entry.ring_id {
        0 => inst.ring,
        1 if v == 4 << 24 | 2 << 16 => inst.page,
        // 2: XXX compute
        3 if v != 4 << 24 | 2 << 16 => inst.page,
        _ => None,
    };
    if let Some(r) = ring {
        crate::amdgpu_fence::amdgpu_fence_process(adev, r);
    }
    0
}

/// `sdma_v4_0_update_medium_grain_clock_gating`.
fn sdma_v4_0_update_medium_grain_clock_gating(adev: &amdgpu_device, enable: bool) {
    let m = SDMA0_SDMA0_CLK_CTRL__SOFT_OVERRIDE7_MASK | SDMA0_SDMA0_CLK_CTRL__SOFT_OVERRIDE6_MASK
        | SDMA0_SDMA0_CLK_CTRL__SOFT_OVERRIDE5_MASK | SDMA0_SDMA0_CLK_CTRL__SOFT_OVERRIDE4_MASK
        | SDMA0_SDMA0_CLK_CTRL__SOFT_OVERRIDE3_MASK | SDMA0_SDMA0_CLK_CTRL__SOFT_OVERRIDE2_MASK
        | SDMA0_SDMA0_CLK_CTRL__SOFT_OVERRIDE1_MASK | SDMA0_SDMA0_CLK_CTRL__SOFT_OVERRIDE0_MASK;
    let on = enable && adev.cg_flags & AMD_CG_SUPPORT_SDMA_MGCG as u64 != 0;
    for i in 0..adev.sdma.num_instances {
        let def = rd(adev, i, SDMA0_mmSDMA0_CLK_CTRL);
        let data = if on { def & !m } else { def | m };
        if def != data {
            wr(adev, i, SDMA0_mmSDMA0_CLK_CTRL, data);
        }
    }
}

/// `sdma_v4_0_update_medium_grain_light_sleep`: instance 0's register, once
/// per instance, as Linux has it.
fn sdma_v4_0_update_medium_grain_light_sleep(adev: &amdgpu_device, enable: bool) {
    let on = enable && adev.cg_flags & AMD_CG_SUPPORT_SDMA_LS as u64 != 0;
    for _ in 0..adev.sdma.num_instances {
        // 1-not override: enable sdma mem light sleep; 0-override: disable
        let def = rd(adev, 0, SDMA0_mmSDMA0_POWER_CNTL);
        let data = if on {
            def | SDMA0_SDMA0_POWER_CNTL__MEM_POWER_OVERRIDE_MASK
        } else {
            def & !SDMA0_SDMA0_POWER_CNTL__MEM_POWER_OVERRIDE_MASK
        };
        if def != data {
            wr(adev, 0, SDMA0_mmSDMA0_POWER_CNTL, data);
        }
    }
}

/// `sdma_v4_0_set_clockgating_state`.
pub fn sdma_v4_0_set_clockgating_state(adev: &amdgpu_device, gate: bool) -> i32 {
    sdma_v4_0_update_medium_grain_clock_gating(adev, gate);
    sdma_v4_0_update_medium_grain_light_sleep(adev, gate);
    0
}

/// `sdma_v4_0_set_powergating_state`.
pub fn sdma_v4_0_set_powergating_state(adev: &amdgpu_device, gate: bool) -> i32 {
    let v = adev.ip_version(SDMA0_HWIP, 0);
    if v == 4 << 24 | 1 << 16 || v == 4 << 24 | 1 << 16 | 1 << 8 || v == 4 << 24 | 1 << 16 | 2 << 8 {
        sdma_v4_1_update_power_gating(adev, gate);
    }
    0
}

/// `sdma_v4_0_late_init`: `sdma_v4_0_setup_ulv` is for VG10 and VG20 only,
/// and there is no RAS block to reset.
pub fn sdma_v4_0_late_init(_adev: &mut amdgpu_device) -> i32 {
    0
}

/// `sdma_v4_0_hw_fini`: bare metal, no RAS.
pub fn sdma_v4_0_hw_fini(adev: &mut amdgpu_device) -> i32 {
    sdma_v4_0_ctx_switch_enable(adev, false);
    // sdma_v4_0_enable(adev, false)
    for i in 0..adev.sdma.num_instances {
        // sdma_v4_0_gfx_enable(adev, false)
        let mut rb_cntl = rd(adev, i, SDMA0_mmSDMA0_GFX_RB_CNTL);
        rb_cntl = set_field(rb_cntl, SDMA0_SDMA0_GFX_RB_CNTL__RB_ENABLE_MASK, SDMA0_SDMA0_GFX_RB_CNTL__RB_ENABLE__SHIFT, 0);
        wr(adev, i, SDMA0_mmSDMA0_GFX_RB_CNTL, rb_cntl);
        let mut ib_cntl = rd(adev, i, SDMA0_mmSDMA0_GFX_IB_CNTL);
        ib_cntl = set_field(ib_cntl, SDMA0_SDMA0_GFX_IB_CNTL__IB_ENABLE_MASK, SDMA0_SDMA0_GFX_IB_CNTL__IB_ENABLE__SHIFT, 0);
        wr(adev, i, SDMA0_mmSDMA0_GFX_IB_CNTL, ib_cntl);
    }
    // sdma_v4_0_rlc_stop is empty; no page queue to stop.
    for i in 0..adev.sdma.num_instances {
        let mut f32_cntl = rd(adev, i, SDMA0_mmSDMA0_F32_CNTL);
        f32_cntl = set_field(f32_cntl, SDMA0_SDMA0_F32_CNTL__HALT_MASK, SDMA0_SDMA0_F32_CNTL__HALT__SHIFT, 1);
        wr(adev, i, SDMA0_mmSDMA0_F32_CNTL, f32_cntl);
    }
    if adev.flags & AMD_IS_APU != 0 {
        crate::amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_SDMA, true, 0);
    }
    0
}

// ---- buffer functions (stage 5) ----

/// `sdma_v4_0_buffer_funcs.copy_max_bytes`.
pub const SDMA_V4_0_COPY_MAX_BYTES: u32 = 0x400000;
/// `sdma_v4_0_buffer_funcs.copy_num_dw`.
pub const SDMA_V4_0_COPY_NUM_DW: u32 = 7;

/// `sdma_v4_0_ring_pad_ib`.
pub fn sdma_v4_0_ring_pad_ib(adev: &amdgpu_device, r: usize, ib: &mut crate::amdgpu_ib::amdgpu_ib) {
    let burst_nop = amdgpu_sdma_get_instance_from_ring(adev, r).is_some_and(|i| adev.sdma.instance[i].burst_nop);
    let pad_count = ib.length_dw.wrapping_neg() & 7;
    for i in 0..pad_count {
        if burst_nop && i == 0 {
            ib.set(adev, ib.length_dw, SDMA_PKT_HEADER_OP_NOP
                | ((pad_count - 1) & SDMA_PKT_NOP_HEADER_count_mask) << SDMA_PKT_NOP_HEADER_count_shift);
        } else {
            ib.set(adev, ib.length_dw, SDMA_PKT_HEADER_OP_NOP);
        }
        ib.length_dw += 1;
    }
}

/// `sdma_v4_0_emit_copy_buffer`: copy GPU buffers using the DMA engine.
pub fn sdma_v4_0_emit_copy_buffer(adev: &amdgpu_device, ib: &mut crate::amdgpu_ib::amdgpu_ib, src_offset: u64,
                                  dst_offset: u64, byte_count: u32, copy_flags: u32) {
    let tmz = (copy_flags & AMDGPU_COPY_FLAGS_TMZ != 0) as u32;
    let words = [
        SDMA_PKT_HEADER_OP_COPY | SDMA_PKT_HEADER_SUB_OP_COPY_LINEAR
            | (tmz & SDMA_PKT_COPY_LINEAR_HEADER_tmz_mask) << SDMA_PKT_COPY_LINEAR_HEADER_tmz_shift,
        byte_count - 1,
        // src/dst endian swap
        0,
        src_offset as u32,
        (src_offset >> 32) as u32,
        dst_offset as u32,
        (dst_offset >> 32) as u32,
    ];
    for w in words {
        ib.set(adev, ib.length_dw, w);
        ib.length_dw += 1;
    }
}
