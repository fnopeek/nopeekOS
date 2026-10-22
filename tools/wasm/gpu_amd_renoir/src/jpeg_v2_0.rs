//! amdgpu/jpeg_v2_0.c and amdgpu_jpeg.c (Linux 6.18.55): the JPEG
//! block, its ring and the parser that checks a user's command stream.

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

// ---- hw_init, start and stop, the ring (stage 6) ----

/// `msecs_to_jiffies(1000)` (`JPEG_IDLE_TIMEOUT`), in microseconds.
const JPEG_IDLE_TIMEOUT_US: u64 = 1_000_000;

/// `SOC15_REG_OFFSET(JPEG, 0, reg)`.
fn jreg(adev: &amdgpu_device, reg: u32, base_idx: u32) -> u32 {
    adev.reg_offset[JPEG_HWIP as usize][0].get(base_idx as usize).copied().unwrap_or(0) + reg
}

/// `RREG32_SOC15(JPEG, 0, reg)`.
fn rreg(adev: &amdgpu_device, reg: u32, base_idx: u32) -> u32 {
    adev.rreg32(jreg(adev, reg, base_idx)).unwrap_or(0)
}

/// `WREG32_SOC15(JPEG, 0, reg, v)`.
fn wreg(adev: &amdgpu_device, reg: u32, base_idx: u32, v: u32) {
    adev.wreg32(jreg(adev, reg, base_idx), v);
}

/// `WREG32_P(SOC15_REG_OFFSET(JPEG, 0, reg), val, mask)`.
fn wreg_p(adev: &amdgpu_device, reg: u32, base_idx: u32, val: u32, mask: u32) {
    let mut tmp = rreg(adev, reg, base_idx);
    tmp &= mask;
    tmp |= val & !mask;
    wreg(adev, reg, base_idx, tmp);
}

/// `PACKETJ(reg, r, cond, type)` (soc15d.h).
fn packetj(reg: u32, r: u32, cond: u32, type_: u32) -> u32 {
    (reg & 0x3FFFF) | (r & 0x3F) << 18 | (cond & 0xF) << 24 | (type_ & 0xF) << 28
}

/// `PACKET0(reg, n)` (soc15d.h).
fn packet0(reg: u32, n: u32) -> u32 {
    (reg & 0xFFFF) | (n & 0x3FFF) << 16
}

/// `jpeg_v2_0_hw_init`.
pub fn jpeg_v2_0_hw_init(adev: &mut amdgpu_device) -> i32 {
    let Some(ring) = adev.jpeg.inst0_ring_dec else { return -22 };
    let use_db = adev.rings[ring].use_doorbell;
    crate::nbio_v7_0::nbio_v7_0_vcn_doorbell_range(adev, use_db, adev.doorbell_index.vcn_ring0_1 << 1, 0);
    crate::amdgpu_ring::amdgpu_ring_test_helper(adev, ring)
}

/// `jpeg_v2_0_hw_fini`.
pub fn jpeg_v2_0_hw_fini(adev: &mut amdgpu_device) -> i32 {
    crate::workqueue::cancel_delayed_work(adev, crate::workqueue::work::jpeg_idle);
    if adev.jpeg.cur_state != AMD_PG_STATE_GATE && rreg(adev, mmUVD_JRBC_STATUS, mmUVD_JRBC_STATUS_BASE_IDX) != 0 {
        jpeg_v2_0_set_powergating_state(adev, AMD_PG_STATE_GATE);
    }
    0
}

/// `jpeg_v2_0_suspend`: `amdgpu_jpeg_suspend` only cancels the idle work.
pub fn jpeg_v2_0_suspend(adev: &mut amdgpu_device) -> i32 {
    let r = jpeg_v2_0_hw_fini(adev);
    if r != 0 {
        return r;
    }
    crate::workqueue::cancel_delayed_work(adev, crate::workqueue::work::jpeg_idle);
    0
}

/// `jpeg_v2_0_resume`: `amdgpu_jpeg_resume` does nothing.
pub fn jpeg_v2_0_resume(adev: &mut amdgpu_device) -> i32 {
    jpeg_v2_0_hw_init(adev)
}

/// `jpeg_v2_0_disable_power_gating`.
fn jpeg_v2_0_disable_power_gating(adev: &amdgpu_device) -> i32 {
    if adev.pg_flags & AMD_PG_SUPPORT_JPEG != 0 {
        let data = 1 << UVD_PGFSM_CONFIG__UVDJ_PWR_CONFIG__SHIFT;
        wreg(adev, mmUVD_PGFSM_CONFIG, mmUVD_PGFSM_CONFIG_BASE_IDX, data);
        let r = crate::amdgpu_device::amdgpu_device_wait_on_rreg(adev, 0,
            jreg(adev, mmUVD_PGFSM_STATUS, mmUVD_PGFSM_STATUS_BASE_IDX), "mmUVD_PGFSM_STATUS",
            UVD_PGFSM_STATUS_UVDJ_PWR_ON, UVD_PGFSM_STATUS__UVDJ_PWR_STATUS_MASK);
        if r != 0 {
            Line::new().s("amdgpu: JPEG disable power gating failed").say();
            return r;
        }
    }
    // Removing the anti hang mechanism to indicate the UVDJ tile is ON
    let data = rreg(adev, mmUVD_JPEG_POWER_STATUS, mmUVD_JPEG_POWER_STATUS_BASE_IDX) & !0x1;
    wreg(adev, mmUVD_JPEG_POWER_STATUS, mmUVD_JPEG_POWER_STATUS_BASE_IDX, data);
    0
}

/// `jpeg_v2_0_enable_power_gating`.
fn jpeg_v2_0_enable_power_gating(adev: &amdgpu_device) -> i32 {
    if adev.pg_flags & AMD_PG_SUPPORT_JPEG != 0 {
        let mut data = rreg(adev, mmUVD_JPEG_POWER_STATUS, mmUVD_JPEG_POWER_STATUS_BASE_IDX);
        data &= !UVD_JPEG_POWER_STATUS__JPEG_POWER_STATUS_MASK;
        data |= 0x1; // UVD_JPEG_POWER_STATUS__JPEG_POWER_STATUS_TILES_OFF
        wreg(adev, mmUVD_JPEG_POWER_STATUS, mmUVD_JPEG_POWER_STATUS_BASE_IDX, data);

        let data = 2 << UVD_PGFSM_CONFIG__UVDJ_PWR_CONFIG__SHIFT;
        wreg(adev, mmUVD_PGFSM_CONFIG, mmUVD_PGFSM_CONFIG_BASE_IDX, data);

        let r = crate::amdgpu_device::amdgpu_device_wait_on_rreg(adev, 0,
            jreg(adev, mmUVD_PGFSM_STATUS, mmUVD_PGFSM_STATUS_BASE_IDX), "mmUVD_PGFSM_STATUS",
            2 << UVD_PGFSM_STATUS__UVDJ_PWR_STATUS__SHIFT, UVD_PGFSM_STATUS__UVDJ_PWR_STATUS_MASK);
        if r != 0 {
            Line::new().s("amdgpu: JPEG enable power gating failed").say();
            return r;
        }
    }
    0
}

/// `jpeg_v2_0_disable_clock_gating`.
fn jpeg_v2_0_disable_clock_gating(adev: &amdgpu_device) {
    let mut data = rreg(adev, mmJPEG_CGC_CTRL, mmJPEG_CGC_CTRL_BASE_IDX);
    if adev.cg_flags & AMD_CG_SUPPORT_JPEG_MGCG as u64 != 0 {
        data |= 1 << JPEG_CGC_CTRL__DYN_CLOCK_MODE__SHIFT;
    } else {
        data &= !JPEG_CGC_CTRL__DYN_CLOCK_MODE__SHIFT;
    }
    data |= 1 << JPEG_CGC_CTRL__CLK_GATE_DLY_TIMER__SHIFT;
    data |= 4 << JPEG_CGC_CTRL__CLK_OFF_DELAY__SHIFT;
    wreg(adev, mmJPEG_CGC_CTRL, mmJPEG_CGC_CTRL_BASE_IDX, data);

    let mut data = rreg(adev, mmJPEG_CGC_GATE, mmJPEG_CGC_GATE_BASE_IDX);
    data &= !(JPEG_CGC_GATE__JPEG_DEC_MASK | JPEG_CGC_GATE__JPEG2_DEC_MASK | JPEG_CGC_GATE__JPEG_ENC_MASK
        | JPEG_CGC_GATE__JMCIF_MASK | JPEG_CGC_GATE__JRBBM_MASK);
    wreg(adev, mmJPEG_CGC_GATE, mmJPEG_CGC_GATE_BASE_IDX, data);
}

/// `jpeg_v2_0_enable_clock_gating`.
fn jpeg_v2_0_enable_clock_gating(adev: &amdgpu_device) {
    let mut data = rreg(adev, mmJPEG_CGC_CTRL, mmJPEG_CGC_CTRL_BASE_IDX);
    if adev.cg_flags & AMD_CG_SUPPORT_JPEG_MGCG as u64 != 0 {
        data |= 1 << JPEG_CGC_CTRL__DYN_CLOCK_MODE__SHIFT;
    } else {
        data |= 0 << JPEG_CGC_CTRL__DYN_CLOCK_MODE__SHIFT;
    }
    data |= 1 << JPEG_CGC_CTRL__CLK_GATE_DLY_TIMER__SHIFT;
    data |= 4 << JPEG_CGC_CTRL__CLK_OFF_DELAY__SHIFT;
    wreg(adev, mmJPEG_CGC_CTRL, mmJPEG_CGC_CTRL_BASE_IDX, data);

    let mut data = rreg(adev, mmJPEG_CGC_GATE, mmJPEG_CGC_GATE_BASE_IDX);
    data |= JPEG_CGC_GATE__JPEG_DEC_MASK | JPEG_CGC_GATE__JPEG2_DEC_MASK | JPEG_CGC_GATE__JPEG_ENC_MASK
        | JPEG_CGC_GATE__JMCIF_MASK | JPEG_CGC_GATE__JRBBM_MASK;
    wreg(adev, mmJPEG_CGC_GATE, mmJPEG_CGC_GATE_BASE_IDX, data);
}

/// `amdgpu_dpm_enable_jpeg`.
fn amdgpu_dpm_enable_jpeg(adev: &mut amdgpu_device, enable: bool) {
    let r = crate::amdgpu_smu::amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_JPEG, !enable, 0);
    if r != 0 {
        Line::new().s("Dpm ").s(if enable { "enable" } else { "disable" }).s(" jpeg failed, ret = ").dec((-r) as u64)
            .say();
    }
}

/// `jpeg_v2_0_start`.
fn jpeg_v2_0_start(adev: &mut amdgpu_device) -> i32 {
    let Some(ring) = adev.jpeg.inst0_ring_dec else { return -22 };
    if adev.pm.dpm_enabled {
        amdgpu_dpm_enable_jpeg(adev, true);
    }
    // disable power gating
    let r = jpeg_v2_0_disable_power_gating(adev);
    if r != 0 {
        return r;
    }
    // JPEG disable CGC
    jpeg_v2_0_disable_clock_gating(adev);

    let cfg = adev.gfx.config.gb_addr_config;
    wreg(adev, mmJPEG_DEC_GFX10_ADDR_CONFIG, mmJPEG_DEC_GFX10_ADDR_CONFIG_BASE_IDX, cfg);

    // enable JMI channel
    wreg_p(adev, mmUVD_JMI_CNTL, mmUVD_JMI_CNTL_BASE_IDX, 0, !UVD_JMI_CNTL__SOFT_RESET_MASK);

    // enable System Interrupt for JRBC
    wreg_p(adev, mmJPEG_SYS_INT_EN, mmJPEG_SYS_INT_EN_BASE_IDX, JPEG_SYS_INT_EN__DJRBC_MASK,
           !JPEG_SYS_INT_EN__DJRBC_MASK);

    let (gpu, size) = (adev.rings[ring].gpu_addr, adev.rings[ring].ring_size);
    wreg(adev, mmUVD_LMI_JRBC_RB_VMID, mmUVD_LMI_JRBC_RB_VMID_BASE_IDX, 0);
    wreg(adev, mmUVD_JRBC_RB_CNTL, mmUVD_JRBC_RB_CNTL_BASE_IDX, 0x00000001 | 0x00000002);
    wreg(adev, mmUVD_LMI_JRBC_RB_64BIT_BAR_LOW, mmUVD_LMI_JRBC_RB_64BIT_BAR_LOW_BASE_IDX, gpu as u32);
    wreg(adev, mmUVD_LMI_JRBC_RB_64BIT_BAR_HIGH, mmUVD_LMI_JRBC_RB_64BIT_BAR_HIGH_BASE_IDX, (gpu >> 32) as u32);
    wreg(adev, mmUVD_JRBC_RB_RPTR, mmUVD_JRBC_RB_RPTR_BASE_IDX, 0);
    wreg(adev, mmUVD_JRBC_RB_WPTR, mmUVD_JRBC_RB_WPTR_BASE_IDX, 0);
    wreg(adev, mmUVD_JRBC_RB_CNTL, mmUVD_JRBC_RB_CNTL_BASE_IDX, 0x00000002);
    wreg(adev, mmUVD_JRBC_RB_SIZE, mmUVD_JRBC_RB_SIZE_BASE_IDX, size / 4);
    adev.rings[ring].wptr = rreg(adev, mmUVD_JRBC_RB_WPTR, mmUVD_JRBC_RB_WPTR_BASE_IDX) as u64;
    0
}

/// `jpeg_v2_0_stop`.
fn jpeg_v2_0_stop(adev: &mut amdgpu_device) -> i32 {
    // reset JMI
    wreg_p(adev, mmUVD_JMI_CNTL, mmUVD_JMI_CNTL_BASE_IDX, UVD_JMI_CNTL__SOFT_RESET_MASK, !UVD_JMI_CNTL__SOFT_RESET_MASK);
    // enable JPEG CGC
    jpeg_v2_0_enable_clock_gating(adev);
    // enable power gating
    let r = jpeg_v2_0_enable_power_gating(adev);
    if r != 0 {
        return r;
    }
    if adev.pm.dpm_enabled {
        amdgpu_dpm_enable_jpeg(adev, false);
    }
    0
}

/// `jpeg_v2_0_is_idle`.
pub fn jpeg_v2_0_is_idle(adev: &amdgpu_device) -> bool {
    rreg(adev, mmUVD_JRBC_STATUS, mmUVD_JRBC_STATUS_BASE_IDX) & UVD_JRBC_STATUS__RB_JOB_DONE_MASK
        == UVD_JRBC_STATUS__RB_JOB_DONE_MASK
}

/// `jpeg_v2_0_set_powergating_state`.
pub fn jpeg_v2_0_set_powergating_state(adev: &mut amdgpu_device, state: u32) -> i32 {
    if state == adev.jpeg.cur_state {
        return 0;
    }
    let r = if state == AMD_PG_STATE_GATE { jpeg_v2_0_stop(adev) } else { jpeg_v2_0_start(adev) };
    if r == 0 {
        adev.jpeg.cur_state = state;
    }
    r
}

/// `amdgpu_jpeg_idle_work_handler`.
pub fn amdgpu_jpeg_idle_work_handler(adev: &mut amdgpu_device) {
    let mut fences = 0;
    for i in 0..adev.jpeg.num_jpeg_inst {
        if adev.jpeg.harvest_config & (1 << i) != 0 {
            continue;
        }
        if let Some(r) = adev.jpeg.inst0_ring_dec {
            fences += crate::vcn_v2_0::amdgpu_fence_count_emitted(adev, r);
        }
    }
    if fences == 0 && adev.jpeg.total_submission_cnt == 0 {
        crate::amdgpu_device::amdgpu_device_ip_set_powergating_state(adev, AMD_IP_BLOCK_TYPE_JPEG, true);
    } else {
        crate::workqueue::mod_delayed_work(adev, crate::workqueue::work::jpeg_idle, JPEG_IDLE_TIMEOUT_US);
    }
}

/// `amdgpu_jpeg_ring_begin_use`.
pub fn amdgpu_jpeg_ring_begin_use(adev: &mut amdgpu_device, _r: usize) {
    adev.jpeg.total_submission_cnt += 1;
    crate::workqueue::cancel_delayed_work(adev, crate::workqueue::work::jpeg_idle);
    crate::amdgpu_device::amdgpu_device_ip_set_powergating_state(adev, AMD_IP_BLOCK_TYPE_JPEG, false);
}

/// `amdgpu_jpeg_ring_end_use`.
pub fn amdgpu_jpeg_ring_end_use(adev: &mut amdgpu_device, _r: usize) {
    adev.jpeg.total_submission_cnt -= 1;
    crate::workqueue::mod_delayed_work(adev, crate::workqueue::work::jpeg_idle, JPEG_IDLE_TIMEOUT_US);
}

use crate::amdgpu_ring::amdgpu_ring_write as rw;

/// `jpeg_v2_0_dec_ring_get_rptr`.
pub fn jpeg_v2_0_dec_ring_get_rptr(adev: &amdgpu_device, _r: usize) -> u64 {
    rreg(adev, mmUVD_JRBC_RB_RPTR, mmUVD_JRBC_RB_RPTR_BASE_IDX) as u64
}

/// `jpeg_v2_0_dec_ring_get_wptr`.
pub fn jpeg_v2_0_dec_ring_get_wptr(adev: &amdgpu_device, r: usize) -> u64 {
    if adev.rings[r].use_doorbell {
        crate::amdgpu_ring::ring_wb_read64(adev, adev.rings[r].wptr_offs)
    } else {
        rreg(adev, mmUVD_JRBC_RB_WPTR, mmUVD_JRBC_RB_WPTR_BASE_IDX) as u64
    }
}

/// `jpeg_v2_0_dec_ring_set_wptr`.
pub fn jpeg_v2_0_dec_ring_set_wptr(adev: &mut amdgpu_device, r: usize) {
    let wptr = adev.rings[r].wptr as u32;
    if adev.rings[r].use_doorbell {
        crate::amdgpu_ring::ring_wptr_cpu_set(adev, r, wptr as u64);
        crate::amdgpu_ring::amdgpu_mm_wdoorbell(adev, adev.rings[r].doorbell_index, wptr);
    } else {
        wreg(adev, mmUVD_JRBC_RB_WPTR, mmUVD_JRBC_RB_WPTR_BASE_IDX, wptr);
    }
}

/// `jpeg_v2_0_dec_ring_insert_start`.
pub fn jpeg_v2_0_dec_ring_insert_start(adev: &mut amdgpu_device, r: usize) {
    rw(adev, r, packetj(mmUVD_JRBC_EXTERNAL_REG_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x68e04);
    rw(adev, r, packetj(JRBC_DEC_EXTERNAL_REG_WRITE_ADDR, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x80010000);
}

/// `jpeg_v2_0_dec_ring_insert_end`.
pub fn jpeg_v2_0_dec_ring_insert_end(adev: &mut amdgpu_device, r: usize) {
    rw(adev, r, packetj(mmUVD_JRBC_EXTERNAL_REG_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x68e04);
    rw(adev, r, packetj(JRBC_DEC_EXTERNAL_REG_WRITE_ADDR, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x00010000);
}

/// `jpeg_v2_0_dec_ring_emit_fence`.
pub fn jpeg_v2_0_dec_ring_emit_fence(adev: &mut amdgpu_device, r: usize, addr: u64, seq: u64, flags: u32) {
    if flags & AMDGPU_FENCE_FLAG_64BIT != 0 {
        Line::new().s("WARNING: jpeg_v2_0_dec_ring_emit_fence with a 64-bit fence").say();
    }
    rw(adev, r, packetj(mmUVD_JPEG_GPCOM_DATA0_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, seq as u32);

    rw(adev, r, packetj(mmUVD_JPEG_GPCOM_DATA1_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, seq as u32);

    rw(adev, r, packetj(mmUVD_LMI_JRBC_RB_MEM_WR_64BIT_BAR_LOW_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, addr as u32);

    rw(adev, r, packetj(mmUVD_LMI_JRBC_RB_MEM_WR_64BIT_BAR_HIGH_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, (addr >> 32) as u32);

    rw(adev, r, packetj(mmUVD_JPEG_GPCOM_CMD_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x8);

    rw(adev, r, packetj(mmUVD_JPEG_GPCOM_CMD_INTERNAL_OFFSET, 0, PACKETJ_CONDITION_CHECK0, PACKETJ_TYPE4));
    rw(adev, r, 0);

    rw(adev, r, packetj(mmUVD_JRBC_EXTERNAL_REG_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x3fbc);

    rw(adev, r, packetj(JRBC_DEC_EXTERNAL_REG_WRITE_ADDR, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x1);

    rw(adev, r, packetj(0, 0, 0, PACKETJ_TYPE7));
    rw(adev, r, 0);
}

/// `jpeg_v2_0_dec_ring_emit_ib`: the ring has `parse_cs`, so the IB is
/// read through VMID 0, a kernel copy the parser checked.
pub fn jpeg_v2_0_dec_ring_emit_ib(adev: &mut amdgpu_device, r: usize, vmid: u32, ib: &crate::amdgpu_ib::amdgpu_ib,
                                  _flags: u32) {
    rw(adev, r, packetj(mmUVD_JPEG_IH_CTRL_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, vmid << JPEG_IH_CTRL__IH_VMID__SHIFT);

    rw(adev, r, packetj(mmUVD_LMI_JRBC_IB_VMID_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0);

    rw(adev, r, packetj(mmUVD_LMI_JPEG_VMID_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, vmid | vmid << 4 | vmid << 8);

    rw(adev, r, packetj(mmUVD_LMI_JRBC_IB_64BIT_BAR_LOW_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, ib.gpu_addr as u32);

    rw(adev, r, packetj(mmUVD_LMI_JRBC_IB_64BIT_BAR_HIGH_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, (ib.gpu_addr >> 32) as u32);

    rw(adev, r, packetj(mmUVD_JRBC_IB_SIZE_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, ib.length_dw);

    let gpu = adev.rings[r].gpu_addr;
    rw(adev, r, packetj(mmUVD_LMI_JRBC_RB_MEM_RD_64BIT_BAR_LOW_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, gpu as u32);

    rw(adev, r, packetj(mmUVD_LMI_JRBC_RB_MEM_RD_64BIT_BAR_HIGH_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, (gpu >> 32) as u32);

    rw(adev, r, packetj(0, 0, PACKETJ_CONDITION_CHECK0, PACKETJ_TYPE2));
    rw(adev, r, 0);

    rw(adev, r, packetj(mmUVD_JRBC_RB_COND_RD_TIMER_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x01400200);

    rw(adev, r, packetj(mmUVD_JRBC_RB_REF_DATA_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x2);

    rw(adev, r, packetj(mmUVD_JRBC_STATUS_INTERNAL_OFFSET, 0, PACKETJ_CONDITION_CHECK3, PACKETJ_TYPE3));
    rw(adev, r, 0x2);
}

/// `jpeg_v2_0_dec_ring_emit_reg_wait`.
pub fn jpeg_v2_0_dec_ring_emit_reg_wait(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32, mask: u32) {
    let reg_offset = reg << 2;
    rw(adev, r, packetj(mmUVD_JRBC_RB_COND_RD_TIMER_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, 0x01400200);

    rw(adev, r, packetj(mmUVD_JRBC_RB_REF_DATA_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    rw(adev, r, val);

    rw(adev, r, packetj(mmUVD_JRBC_EXTERNAL_REG_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    if (0x10000..=0x105ff).contains(&reg_offset) {
        rw(adev, r, 0);
        rw(adev, r, packetj(reg_offset >> 2, 0, 0, PACKETJ_TYPE3));
    } else {
        rw(adev, r, reg_offset);
        rw(adev, r, packetj(JRBC_DEC_EXTERNAL_REG_WRITE_ADDR, 0, 0, PACKETJ_TYPE3));
    }
    rw(adev, r, mask);
}

/// `jpeg_v2_0_dec_ring_emit_vm_flush`.
pub fn jpeg_v2_0_dec_ring_emit_vm_flush(adev: &mut amdgpu_device, r: usize, vmid: u32, pd_addr: u64) {
    let hub = adev.vmhub[adev.rings[r].vm_hub as usize];
    let pd_addr = crate::gmc_v9_0::gmc_v9_0_emit_flush_gpu_tlb(adev, r, vmid, pd_addr);
    // wait for register write
    let data0 = hub.ctx0_ptb_addr_lo32 + vmid * hub.ctx_addr_distance;
    jpeg_v2_0_dec_ring_emit_reg_wait(adev, r, data0, pd_addr as u32, 0xffffffff);
}

/// `jpeg_v2_0_dec_ring_emit_wreg`.
pub fn jpeg_v2_0_dec_ring_emit_wreg(adev: &mut amdgpu_device, r: usize, reg: u32, val: u32) {
    let reg_offset = reg << 2;
    rw(adev, r, packetj(mmUVD_JRBC_EXTERNAL_REG_INTERNAL_OFFSET, 0, 0, PACKETJ_TYPE0));
    if (0x10000..=0x105ff).contains(&reg_offset) {
        rw(adev, r, 0);
        rw(adev, r, packetj(reg_offset >> 2, 0, 0, PACKETJ_TYPE0));
    } else {
        rw(adev, r, reg_offset);
        rw(adev, r, packetj(JRBC_DEC_EXTERNAL_REG_WRITE_ADDR, 0, 0, PACKETJ_TYPE0));
    }
    rw(adev, r, val);
}

/// `jpeg_v2_0_dec_ring_nop`.
pub fn jpeg_v2_0_dec_ring_nop(adev: &mut amdgpu_device, r: usize, count: u32) {
    if adev.rings[r].wptr % 2 != 0 || count % 2 != 0 {
        Line::new().s("WARNING: jpeg_v2_0_dec_ring_nop on an odd count").say();
    }
    for _ in 0..count / 2 {
        rw(adev, r, packetj(0, 0, 0, PACKETJ_TYPE6));
        rw(adev, r, 0);
    }
}

/// `jpeg_v2_0_process_interrupt`.
pub fn jpeg_v2_0_process_interrupt(adev: &mut amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry) -> i32 {
    match entry.src_id {
        VCN_2_0__SRCID__JPEG_DECODE => {
            if let Some(r) = adev.jpeg.inst0_ring_dec {
                crate::amdgpu_fence::amdgpu_fence_process(adev, r);
            }
        }
        _ => {
            Line::new().s("Unhandled interrupt: ").dec(entry.src_id as u64).s(" ").dec(entry.src_data[0] as u64)
                .say();
        }
    }
    0
}

/// `amdgpu_jpeg_dec_ring_test_ring`.
pub fn amdgpu_jpeg_dec_ring_test_ring(adev: &mut amdgpu_device, r: usize) -> i32 {
    let ret = crate::amdgpu_ring::amdgpu_ring_alloc(adev, r, 3);
    if ret != 0 {
        return ret;
    }
    let ext = adev.jpeg.inst0_external_jpeg_pitch0;
    adev.wreg32(ext, 0xCAFEDEAD);
    // Add a read register to make sure the write register is executed.
    adev.rreg32(ext);
    let internal = adev.jpeg.internal_jpeg_pitch0;
    rw(adev, r, packet0(internal, 0));
    rw(adev, r, 0xABADCAFE);
    crate::amdgpu_ring::amdgpu_ring_commit(adev, r);
    for _ in 0..adev.usec_timeout {
        if adev.rreg32(ext).unwrap_or(0) == 0xABADCAFE {
            return 0;
        }
        crate::linux::udelay(1);
    }
    -110
}

/// `amdgpu_jpeg_dec_set_reg`: a direct job that sets the pitch register.
fn amdgpu_jpeg_dec_set_reg(adev: &mut amdgpu_device, r: usize) -> Result<crate::dma_fence::Fence, i32> {
    let ib_size_dw = 16u32;
    let j = crate::amdgpu_job::amdgpu_job_alloc_with_ib(adev, None, crate::amdgpu_sync::AMDGPU_FENCE_OWNER_UNDEFINED,
                                                        ib_size_dw * 4, AMDGPU_IB_POOL_DIRECT)?;
    let ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
    ib.set(adev, 0, packetj(adev.jpeg.internal_jpeg_pitch0, 0, 0, PACKETJ_TYPE0));
    ib.set(adev, 1, 0xDEADBEEF);
    for i in (2..16).step_by(2) {
        ib.set(adev, i, packetj(0, 0, 0, PACKETJ_TYPE6));
        ib.set(adev, i + 1, 0);
    }
    crate::amdgpu_job::job(adev, j).ibs[0].length_dw = 16;
    match crate::amdgpu_job::amdgpu_job_submit_direct(adev, j, r) {
        Ok(f) => Ok(f),
        Err(e) => {
            crate::amdgpu_job::amdgpu_job_free(adev, j);
            Err(e)
        }
    }
}

/// `amdgpu_jpeg_dec_ring_test_ib`.
pub fn amdgpu_jpeg_dec_ring_test_ib(adev: &mut amdgpu_device, r: usize, timeout: i64) -> i32 {
    let f = match amdgpu_jpeg_dec_set_reg(adev, r) {
        Ok(f) => f,
        Err(e) => return e,
    };
    if crate::dma_fence::dma_fence_wait_timeout(adev, &f, timeout) == 0 {
        return -110;
    }
    let ext = adev.jpeg.inst0_external_jpeg_pitch0;
    for _ in 0..adev.usec_timeout {
        if adev.rreg32(ext).unwrap_or(0) == 0xDEADBEEF {
            return 0;
        }
        crate::linux::udelay(1);
    }
    -110
}

/// `amdgpu_jpeg_reg_valid`.
fn amdgpu_jpeg_reg_valid(reg: u32) -> bool {
    !(reg < JPEG_REG_RANGE_START || reg > JPEG_REG_RANGE_END
        || (JPEG_ATOMIC_RANGE_START..=JPEG_ATOMIC_RANGE_END).contains(&reg))
}

/// `amdgpu_jpeg_dec_parse_cs` over the IB's dwords: only register writes
/// and waits inside the JPEG range, no atomics, and the one NOP; -EINVAL
/// for anything else.
pub fn amdgpu_jpeg_dec_parse_cs(ib: &[u32]) -> i32 {
    let mut i = 0;
    while i < ib.len() {
        let p = ib[i];
        let reg = p & 0x3FFFF;
        let res = (p >> 18) & 0x3F;
        let cond = (p >> 24) & 0xF;
        let type_ = (p >> 28) & 0xF;
        // only support 0 at the moment
        if res != 0 {
            return -22;
        }
        match type_ {
            PACKETJ_TYPE0 if cond == PACKETJ_CONDITION_CHECK0 && amdgpu_jpeg_reg_valid(reg) => {}
            PACKETJ_TYPE3 if cond == PACKETJ_CONDITION_CHECK3 && amdgpu_jpeg_reg_valid(reg) => {}
            PACKETJ_TYPE6 if p == CP_PACKETJ_NOP => {}
            PACKETJ_TYPE0 | PACKETJ_TYPE3 | PACKETJ_TYPE6 => {
                Line::new().s("Invalid packet [0x").hexw(p, 8).s("]!").say();
                return -22;
            }
            _ => {
                Line::new().s("Unknown packet type ").dec(type_ as u64).s(" !").say();
                return -22;
            }
        }
        i += 2;
    }
    0
}

/// `jpeg_v2_0_ring_reset`.
pub fn jpeg_v2_0_ring_reset(adev: &mut amdgpu_device, ring: usize, _vmid: u32,
                            timedout: &crate::dma_fence::Fence) -> i32 {
    crate::amdgpu_ring::amdgpu_ring_reset_helper_begin(adev, ring, Some(timedout));
    let r = jpeg_v2_0_stop(adev);
    if r != 0 {
        return r;
    }
    let r = jpeg_v2_0_start(adev);
    if r != 0 {
        return r;
    }
    crate::amdgpu_ring::amdgpu_ring_reset_helper_end(adev, ring, Some(timedout))
}
