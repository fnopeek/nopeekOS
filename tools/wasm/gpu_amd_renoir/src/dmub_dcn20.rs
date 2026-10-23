//! display/dmub/src/dmub_dcn20.{h,c} + dmub_dcn21.c + dmub_reg.{h,c} (Linux
//! 6.18.55): the DMCUB hardware functions of the DCN 2.0 family and the
//! register table DCN 2.1 binds them to. `dmub_srv_hw_setup` points
//! `dmub_srv.hw_funcs` at these for `DMUB_ASIC_DCN21`.
//!
//! Register access goes through `funcs.reg_read` / `reg_write`, which
//! amdgpu_dm binds to `dm_read_reg` / `dm_write_reg`; `adev` stands in for
//! `user_ctx`. The va_list of REG_SET_N / REG_UPDATE_N is a slice of
//! (shift, mask, value) triples.
#![allow(dead_code)]
#![allow(non_snake_case)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc_helper::{dm_read_reg, dm_write_reg};
use crate::dmub_srv::{dmub_region, dmub_srv, dmub_srv_hw_params, dmub_window};
use crate::regs::*;

/// `struct dmub_srv_common_regs` (dmub_dcn20.h).
pub struct dmub_srv_common_regs {
    pub offset: dmub_srv_common_reg_offset,
    pub mask: dmub_srv_common_reg_mask,
    pub shift: dmub_srv_common_reg_shift,
}

/// `struct dmub_srv_common_reg_offset` (dmub_dcn20.h).
#[derive(Clone, Copy)]
pub struct dmub_srv_common_reg_offset {
    pub DMCUB_CNTL: u32,
    pub DMCUB_MEM_CNTL: u32,
    pub DMCUB_SEC_CNTL: u32,
    pub DMCUB_INBOX0_SIZE: u32,
    pub DMCUB_INBOX0_RPTR: u32,
    pub DMCUB_INBOX0_WPTR: u32,
    pub DMCUB_INBOX1_BASE_ADDRESS: u32,
    pub DMCUB_INBOX1_SIZE: u32,
    pub DMCUB_INBOX1_RPTR: u32,
    pub DMCUB_INBOX1_WPTR: u32,
    pub DMCUB_OUTBOX0_BASE_ADDRESS: u32,
    pub DMCUB_OUTBOX0_SIZE: u32,
    pub DMCUB_OUTBOX0_RPTR: u32,
    pub DMCUB_OUTBOX0_WPTR: u32,
    pub DMCUB_OUTBOX1_BASE_ADDRESS: u32,
    pub DMCUB_OUTBOX1_SIZE: u32,
    pub DMCUB_OUTBOX1_RPTR: u32,
    pub DMCUB_OUTBOX1_WPTR: u32,
    pub DMCUB_REGION3_CW0_OFFSET: u32,
    pub DMCUB_REGION3_CW1_OFFSET: u32,
    pub DMCUB_REGION3_CW2_OFFSET: u32,
    pub DMCUB_REGION3_CW3_OFFSET: u32,
    pub DMCUB_REGION3_CW4_OFFSET: u32,
    pub DMCUB_REGION3_CW5_OFFSET: u32,
    pub DMCUB_REGION3_CW6_OFFSET: u32,
    pub DMCUB_REGION3_CW7_OFFSET: u32,
    pub DMCUB_REGION3_CW0_OFFSET_HIGH: u32,
    pub DMCUB_REGION3_CW1_OFFSET_HIGH: u32,
    pub DMCUB_REGION3_CW2_OFFSET_HIGH: u32,
    pub DMCUB_REGION3_CW3_OFFSET_HIGH: u32,
    pub DMCUB_REGION3_CW4_OFFSET_HIGH: u32,
    pub DMCUB_REGION3_CW5_OFFSET_HIGH: u32,
    pub DMCUB_REGION3_CW6_OFFSET_HIGH: u32,
    pub DMCUB_REGION3_CW7_OFFSET_HIGH: u32,
    pub DMCUB_REGION3_CW0_BASE_ADDRESS: u32,
    pub DMCUB_REGION3_CW1_BASE_ADDRESS: u32,
    pub DMCUB_REGION3_CW2_BASE_ADDRESS: u32,
    pub DMCUB_REGION3_CW3_BASE_ADDRESS: u32,
    pub DMCUB_REGION3_CW4_BASE_ADDRESS: u32,
    pub DMCUB_REGION3_CW5_BASE_ADDRESS: u32,
    pub DMCUB_REGION3_CW6_BASE_ADDRESS: u32,
    pub DMCUB_REGION3_CW7_BASE_ADDRESS: u32,
    pub DMCUB_REGION3_CW0_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW1_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW2_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW3_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW4_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW5_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW6_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW7_TOP_ADDRESS: u32,
    pub DMCUB_REGION4_OFFSET: u32,
    pub DMCUB_REGION4_OFFSET_HIGH: u32,
    pub DMCUB_REGION4_TOP_ADDRESS: u32,
    pub DMCUB_REGION5_OFFSET: u32,
    pub DMCUB_REGION5_OFFSET_HIGH: u32,
    pub DMCUB_REGION5_TOP_ADDRESS: u32,
    pub DMCUB_SCRATCH0: u32,
    pub DMCUB_SCRATCH1: u32,
    pub DMCUB_SCRATCH2: u32,
    pub DMCUB_SCRATCH3: u32,
    pub DMCUB_SCRATCH4: u32,
    pub DMCUB_SCRATCH5: u32,
    pub DMCUB_SCRATCH6: u32,
    pub DMCUB_SCRATCH7: u32,
    pub DMCUB_SCRATCH8: u32,
    pub DMCUB_SCRATCH9: u32,
    pub DMCUB_SCRATCH10: u32,
    pub DMCUB_SCRATCH11: u32,
    pub DMCUB_SCRATCH12: u32,
    pub DMCUB_SCRATCH13: u32,
    pub DMCUB_SCRATCH14: u32,
    pub DMCUB_SCRATCH15: u32,
    pub DMCUB_GPINT_DATAIN1: u32,
    pub CC_DC_PIPE_DIS: u32,
    pub MMHUBBUB_SOFT_RESET: u32,
    pub DCN_VM_FB_LOCATION_BASE: u32,
    pub DCN_VM_FB_OFFSET: u32,
    pub DMCUB_INTERRUPT_ACK: u32,
    pub DMCUB_TIMER_CURRENT: u32,
    pub DMCUB_INST_FETCH_FAULT_ADDR: u32,
    pub DMCUB_UNDEFINED_ADDRESS_FAULT_ADDR: u32,
    pub DMCUB_DATA_WRITE_FAULT_ADDR: u32,
}

/// `struct dmub_srv_common_reg_mask` (dmub_dcn20.h).
#[derive(Clone, Copy)]
pub struct dmub_srv_common_reg_mask {
    pub DMCUB_CNTL__DMCUB_ENABLE: u32,
    pub DMCUB_CNTL__DMCUB_SOFT_RESET: u32,
    pub DMCUB_CNTL__DMCUB_TRACEPORT_EN: u32,
    pub DMCUB_MEM_CNTL__DMCUB_MEM_READ_SPACE: u32,
    pub DMCUB_MEM_CNTL__DMCUB_MEM_WRITE_SPACE: u32,
    pub DMCUB_SEC_CNTL__DMCUB_SEC_RESET: u32,
    pub DMCUB_SEC_CNTL__DMCUB_MEM_UNIT_ID: u32,
    pub DMCUB_SEC_CNTL__DMCUB_SEC_RESET_STATUS: u32,
    pub DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_ENABLE: u32,
    pub DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_ENABLE: u32,
    pub DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_ENABLE: u32,
    pub DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_ENABLE: u32,
    pub DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_ENABLE: u32,
    pub DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_ENABLE: u32,
    pub DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_ENABLE: u32,
    pub DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_TOP_ADDRESS: u32,
    pub DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_ENABLE: u32,
    pub DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_TOP_ADDRESS: u32,
    pub DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_ENABLE: u32,
    pub DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_TOP_ADDRESS: u32,
    pub DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_ENABLE: u32,
    pub CC_DC_PIPE_DIS__DC_DMCUB_ENABLE: u32,
    pub MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET: u32,
    pub DCN_VM_FB_LOCATION_BASE__FB_BASE: u32,
    pub DCN_VM_FB_OFFSET__FB_OFFSET: u32,
    pub DMCUB_INTERRUPT_ACK__DMCUB_OUTBOX0_READY_INT_ACK: u32,
}

/// `struct dmub_srv_common_reg_shift` (dmub_dcn20.h).
#[derive(Clone, Copy)]
pub struct dmub_srv_common_reg_shift {
    pub DMCUB_CNTL__DMCUB_ENABLE: u8,
    pub DMCUB_CNTL__DMCUB_SOFT_RESET: u8,
    pub DMCUB_CNTL__DMCUB_TRACEPORT_EN: u8,
    pub DMCUB_MEM_CNTL__DMCUB_MEM_READ_SPACE: u8,
    pub DMCUB_MEM_CNTL__DMCUB_MEM_WRITE_SPACE: u8,
    pub DMCUB_SEC_CNTL__DMCUB_SEC_RESET: u8,
    pub DMCUB_SEC_CNTL__DMCUB_MEM_UNIT_ID: u8,
    pub DMCUB_SEC_CNTL__DMCUB_SEC_RESET_STATUS: u8,
    pub DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_TOP_ADDRESS: u8,
    pub DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_ENABLE: u8,
    pub DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_TOP_ADDRESS: u8,
    pub DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_ENABLE: u8,
    pub DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_TOP_ADDRESS: u8,
    pub DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_ENABLE: u8,
    pub DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_TOP_ADDRESS: u8,
    pub DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_ENABLE: u8,
    pub DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_TOP_ADDRESS: u8,
    pub DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_ENABLE: u8,
    pub DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_TOP_ADDRESS: u8,
    pub DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_ENABLE: u8,
    pub DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_TOP_ADDRESS: u8,
    pub DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_ENABLE: u8,
    pub DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_TOP_ADDRESS: u8,
    pub DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_ENABLE: u8,
    pub DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_TOP_ADDRESS: u8,
    pub DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_ENABLE: u8,
    pub DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_TOP_ADDRESS: u8,
    pub DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_ENABLE: u8,
    pub CC_DC_PIPE_DIS__DC_DMCUB_ENABLE: u8,
    pub MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET: u8,
    pub DCN_VM_FB_LOCATION_BASE__FB_BASE: u8,
    pub DCN_VM_FB_OFFSET__FB_OFFSET: u8,
    pub DMCUB_INTERRUPT_ACK__DMCUB_OUTBOX0_READY_INT_ACK: u8,
}

/// `dmub_srv_dcn21_regs` (dmub_dcn21.c).
pub static dmub_srv_dcn21_regs: dmub_srv_common_regs = dmub_srv_common_regs {
    offset: dmub_srv_common_reg_offset {
        DMCUB_CNTL: DMUB_DMCUB_CNTL,
        DMCUB_MEM_CNTL: DMUB_DMCUB_MEM_CNTL,
        DMCUB_SEC_CNTL: DMUB_DMCUB_SEC_CNTL,
        DMCUB_INBOX0_SIZE: DMUB_DMCUB_INBOX0_SIZE,
        DMCUB_INBOX0_RPTR: DMUB_DMCUB_INBOX0_RPTR,
        DMCUB_INBOX0_WPTR: DMUB_DMCUB_INBOX0_WPTR,
        DMCUB_INBOX1_BASE_ADDRESS: DMUB_DMCUB_INBOX1_BASE_ADDRESS,
        DMCUB_INBOX1_SIZE: DMUB_DMCUB_INBOX1_SIZE,
        DMCUB_INBOX1_RPTR: DMUB_DMCUB_INBOX1_RPTR,
        DMCUB_INBOX1_WPTR: DMUB_DMCUB_INBOX1_WPTR,
        DMCUB_OUTBOX0_BASE_ADDRESS: DMUB_DMCUB_OUTBOX0_BASE_ADDRESS,
        DMCUB_OUTBOX0_SIZE: DMUB_DMCUB_OUTBOX0_SIZE,
        DMCUB_OUTBOX0_RPTR: DMUB_DMCUB_OUTBOX0_RPTR,
        DMCUB_OUTBOX0_WPTR: DMUB_DMCUB_OUTBOX0_WPTR,
        DMCUB_OUTBOX1_BASE_ADDRESS: DMUB_DMCUB_OUTBOX1_BASE_ADDRESS,
        DMCUB_OUTBOX1_SIZE: DMUB_DMCUB_OUTBOX1_SIZE,
        DMCUB_OUTBOX1_RPTR: DMUB_DMCUB_OUTBOX1_RPTR,
        DMCUB_OUTBOX1_WPTR: DMUB_DMCUB_OUTBOX1_WPTR,
        DMCUB_REGION3_CW0_OFFSET: DMUB_DMCUB_REGION3_CW0_OFFSET,
        DMCUB_REGION3_CW1_OFFSET: DMUB_DMCUB_REGION3_CW1_OFFSET,
        DMCUB_REGION3_CW2_OFFSET: DMUB_DMCUB_REGION3_CW2_OFFSET,
        DMCUB_REGION3_CW3_OFFSET: DMUB_DMCUB_REGION3_CW3_OFFSET,
        DMCUB_REGION3_CW4_OFFSET: DMUB_DMCUB_REGION3_CW4_OFFSET,
        DMCUB_REGION3_CW5_OFFSET: DMUB_DMCUB_REGION3_CW5_OFFSET,
        DMCUB_REGION3_CW6_OFFSET: DMUB_DMCUB_REGION3_CW6_OFFSET,
        DMCUB_REGION3_CW7_OFFSET: DMUB_DMCUB_REGION3_CW7_OFFSET,
        DMCUB_REGION3_CW0_OFFSET_HIGH: DMUB_DMCUB_REGION3_CW0_OFFSET_HIGH,
        DMCUB_REGION3_CW1_OFFSET_HIGH: DMUB_DMCUB_REGION3_CW1_OFFSET_HIGH,
        DMCUB_REGION3_CW2_OFFSET_HIGH: DMUB_DMCUB_REGION3_CW2_OFFSET_HIGH,
        DMCUB_REGION3_CW3_OFFSET_HIGH: DMUB_DMCUB_REGION3_CW3_OFFSET_HIGH,
        DMCUB_REGION3_CW4_OFFSET_HIGH: DMUB_DMCUB_REGION3_CW4_OFFSET_HIGH,
        DMCUB_REGION3_CW5_OFFSET_HIGH: DMUB_DMCUB_REGION3_CW5_OFFSET_HIGH,
        DMCUB_REGION3_CW6_OFFSET_HIGH: DMUB_DMCUB_REGION3_CW6_OFFSET_HIGH,
        DMCUB_REGION3_CW7_OFFSET_HIGH: DMUB_DMCUB_REGION3_CW7_OFFSET_HIGH,
        DMCUB_REGION3_CW0_BASE_ADDRESS: DMUB_DMCUB_REGION3_CW0_BASE_ADDRESS,
        DMCUB_REGION3_CW1_BASE_ADDRESS: DMUB_DMCUB_REGION3_CW1_BASE_ADDRESS,
        DMCUB_REGION3_CW2_BASE_ADDRESS: DMUB_DMCUB_REGION3_CW2_BASE_ADDRESS,
        DMCUB_REGION3_CW3_BASE_ADDRESS: DMUB_DMCUB_REGION3_CW3_BASE_ADDRESS,
        DMCUB_REGION3_CW4_BASE_ADDRESS: DMUB_DMCUB_REGION3_CW4_BASE_ADDRESS,
        DMCUB_REGION3_CW5_BASE_ADDRESS: DMUB_DMCUB_REGION3_CW5_BASE_ADDRESS,
        DMCUB_REGION3_CW6_BASE_ADDRESS: DMUB_DMCUB_REGION3_CW6_BASE_ADDRESS,
        DMCUB_REGION3_CW7_BASE_ADDRESS: DMUB_DMCUB_REGION3_CW7_BASE_ADDRESS,
        DMCUB_REGION3_CW0_TOP_ADDRESS: DMUB_DMCUB_REGION3_CW0_TOP_ADDRESS,
        DMCUB_REGION3_CW1_TOP_ADDRESS: DMUB_DMCUB_REGION3_CW1_TOP_ADDRESS,
        DMCUB_REGION3_CW2_TOP_ADDRESS: DMUB_DMCUB_REGION3_CW2_TOP_ADDRESS,
        DMCUB_REGION3_CW3_TOP_ADDRESS: DMUB_DMCUB_REGION3_CW3_TOP_ADDRESS,
        DMCUB_REGION3_CW4_TOP_ADDRESS: DMUB_DMCUB_REGION3_CW4_TOP_ADDRESS,
        DMCUB_REGION3_CW5_TOP_ADDRESS: DMUB_DMCUB_REGION3_CW5_TOP_ADDRESS,
        DMCUB_REGION3_CW6_TOP_ADDRESS: DMUB_DMCUB_REGION3_CW6_TOP_ADDRESS,
        DMCUB_REGION3_CW7_TOP_ADDRESS: DMUB_DMCUB_REGION3_CW7_TOP_ADDRESS,
        DMCUB_REGION4_OFFSET: DMUB_DMCUB_REGION4_OFFSET,
        DMCUB_REGION4_OFFSET_HIGH: DMUB_DMCUB_REGION4_OFFSET_HIGH,
        DMCUB_REGION4_TOP_ADDRESS: DMUB_DMCUB_REGION4_TOP_ADDRESS,
        DMCUB_REGION5_OFFSET: DMUB_DMCUB_REGION5_OFFSET,
        DMCUB_REGION5_OFFSET_HIGH: DMUB_DMCUB_REGION5_OFFSET_HIGH,
        DMCUB_REGION5_TOP_ADDRESS: DMUB_DMCUB_REGION5_TOP_ADDRESS,
        DMCUB_SCRATCH0: DMUB_DMCUB_SCRATCH0,
        DMCUB_SCRATCH1: DMUB_DMCUB_SCRATCH1,
        DMCUB_SCRATCH2: DMUB_DMCUB_SCRATCH2,
        DMCUB_SCRATCH3: DMUB_DMCUB_SCRATCH3,
        DMCUB_SCRATCH4: DMUB_DMCUB_SCRATCH4,
        DMCUB_SCRATCH5: DMUB_DMCUB_SCRATCH5,
        DMCUB_SCRATCH6: DMUB_DMCUB_SCRATCH6,
        DMCUB_SCRATCH7: DMUB_DMCUB_SCRATCH7,
        DMCUB_SCRATCH8: DMUB_DMCUB_SCRATCH8,
        DMCUB_SCRATCH9: DMUB_DMCUB_SCRATCH9,
        DMCUB_SCRATCH10: DMUB_DMCUB_SCRATCH10,
        DMCUB_SCRATCH11: DMUB_DMCUB_SCRATCH11,
        DMCUB_SCRATCH12: DMUB_DMCUB_SCRATCH12,
        DMCUB_SCRATCH13: DMUB_DMCUB_SCRATCH13,
        DMCUB_SCRATCH14: DMUB_DMCUB_SCRATCH14,
        DMCUB_SCRATCH15: DMUB_DMCUB_SCRATCH15,
        DMCUB_GPINT_DATAIN1: DMUB_DMCUB_GPINT_DATAIN1,
        CC_DC_PIPE_DIS: DMUB_CC_DC_PIPE_DIS,
        MMHUBBUB_SOFT_RESET: DMUB_MMHUBBUB_SOFT_RESET,
        DCN_VM_FB_LOCATION_BASE: DMUB_DCN_VM_FB_LOCATION_BASE,
        DCN_VM_FB_OFFSET: DMUB_DCN_VM_FB_OFFSET,
        DMCUB_INTERRUPT_ACK: DMUB_DMCUB_INTERRUPT_ACK,
        DMCUB_TIMER_CURRENT: DMUB_DMCUB_TIMER_CURRENT,
        DMCUB_INST_FETCH_FAULT_ADDR: DMUB_DMCUB_INST_FETCH_FAULT_ADDR,
        DMCUB_UNDEFINED_ADDRESS_FAULT_ADDR: DMUB_DMCUB_UNDEFINED_ADDRESS_FAULT_ADDR,
        DMCUB_DATA_WRITE_FAULT_ADDR: DMUB_DMCUB_DATA_WRITE_FAULT_ADDR,
    },
    mask: dmub_srv_common_reg_mask {
        DMCUB_CNTL__DMCUB_ENABLE: DMCUB_CNTL__DMCUB_ENABLE_MASK,
        DMCUB_CNTL__DMCUB_SOFT_RESET: DMCUB_CNTL__DMCUB_SOFT_RESET_MASK,
        DMCUB_CNTL__DMCUB_TRACEPORT_EN: DMCUB_CNTL__DMCUB_TRACEPORT_EN_MASK,
        DMCUB_MEM_CNTL__DMCUB_MEM_READ_SPACE: DMCUB_MEM_CNTL__DMCUB_MEM_READ_SPACE_MASK,
        DMCUB_MEM_CNTL__DMCUB_MEM_WRITE_SPACE: DMCUB_MEM_CNTL__DMCUB_MEM_WRITE_SPACE_MASK,
        DMCUB_SEC_CNTL__DMCUB_SEC_RESET: DMCUB_SEC_CNTL__DMCUB_SEC_RESET_MASK,
        DMCUB_SEC_CNTL__DMCUB_MEM_UNIT_ID: DMCUB_SEC_CNTL__DMCUB_MEM_UNIT_ID_MASK,
        DMCUB_SEC_CNTL__DMCUB_SEC_RESET_STATUS: DMCUB_SEC_CNTL__DMCUB_SEC_RESET_STATUS_MASK,
        DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_TOP_ADDRESS: DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_TOP_ADDRESS_MASK,
        DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_ENABLE: DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_ENABLE_MASK,
        DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_TOP_ADDRESS: DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_TOP_ADDRESS_MASK,
        DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_ENABLE: DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_ENABLE_MASK,
        DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_TOP_ADDRESS: DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_TOP_ADDRESS_MASK,
        DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_ENABLE: DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_ENABLE_MASK,
        DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_TOP_ADDRESS: DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_TOP_ADDRESS_MASK,
        DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_ENABLE: DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_ENABLE_MASK,
        DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_TOP_ADDRESS: DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_TOP_ADDRESS_MASK,
        DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_ENABLE: DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_ENABLE_MASK,
        DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_TOP_ADDRESS: DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_TOP_ADDRESS_MASK,
        DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_ENABLE: DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_ENABLE_MASK,
        DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_TOP_ADDRESS: DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_TOP_ADDRESS_MASK,
        DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_ENABLE: DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_ENABLE_MASK,
        DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_TOP_ADDRESS: DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_TOP_ADDRESS_MASK,
        DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_ENABLE: DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_ENABLE_MASK,
        DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_TOP_ADDRESS: DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_TOP_ADDRESS_MASK,
        DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_ENABLE: DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_ENABLE_MASK,
        DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_TOP_ADDRESS: DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_TOP_ADDRESS_MASK,
        DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_ENABLE: DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_ENABLE_MASK,
        CC_DC_PIPE_DIS__DC_DMCUB_ENABLE: CC_DC_PIPE_DIS__DC_DMCUB_ENABLE_MASK,
        MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET: MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET_MASK,
        DCN_VM_FB_LOCATION_BASE__FB_BASE: DCN_VM_FB_LOCATION_BASE__FB_BASE_MASK,
        DCN_VM_FB_OFFSET__FB_OFFSET: DCN_VM_FB_OFFSET__FB_OFFSET_MASK,
        DMCUB_INTERRUPT_ACK__DMCUB_OUTBOX0_READY_INT_ACK: DMCUB_INTERRUPT_ACK__DMCUB_OUTBOX0_READY_INT_ACK_MASK,
    },
    shift: dmub_srv_common_reg_shift {
        DMCUB_CNTL__DMCUB_ENABLE: DMCUB_CNTL__DMCUB_ENABLE__SHIFT as u8,
        DMCUB_CNTL__DMCUB_SOFT_RESET: DMCUB_CNTL__DMCUB_SOFT_RESET__SHIFT as u8,
        DMCUB_CNTL__DMCUB_TRACEPORT_EN: DMCUB_CNTL__DMCUB_TRACEPORT_EN__SHIFT as u8,
        DMCUB_MEM_CNTL__DMCUB_MEM_READ_SPACE: DMCUB_MEM_CNTL__DMCUB_MEM_READ_SPACE__SHIFT as u8,
        DMCUB_MEM_CNTL__DMCUB_MEM_WRITE_SPACE: DMCUB_MEM_CNTL__DMCUB_MEM_WRITE_SPACE__SHIFT as u8,
        DMCUB_SEC_CNTL__DMCUB_SEC_RESET: DMCUB_SEC_CNTL__DMCUB_SEC_RESET__SHIFT as u8,
        DMCUB_SEC_CNTL__DMCUB_MEM_UNIT_ID: DMCUB_SEC_CNTL__DMCUB_MEM_UNIT_ID__SHIFT as u8,
        DMCUB_SEC_CNTL__DMCUB_SEC_RESET_STATUS: DMCUB_SEC_CNTL__DMCUB_SEC_RESET_STATUS__SHIFT as u8,
        DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_TOP_ADDRESS: DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_ENABLE: DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_ENABLE__SHIFT as u8,
        DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_TOP_ADDRESS: DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_ENABLE: DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_ENABLE__SHIFT as u8,
        DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_TOP_ADDRESS: DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_ENABLE: DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_ENABLE__SHIFT as u8,
        DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_TOP_ADDRESS: DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_ENABLE: DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_ENABLE__SHIFT as u8,
        DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_TOP_ADDRESS: DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_ENABLE: DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_ENABLE__SHIFT as u8,
        DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_TOP_ADDRESS: DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_ENABLE: DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_ENABLE__SHIFT as u8,
        DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_TOP_ADDRESS: DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_ENABLE: DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_ENABLE__SHIFT as u8,
        DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_TOP_ADDRESS: DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_ENABLE: DMCUB_REGION3_CW7_TOP_ADDRESS__DMCUB_REGION3_CW7_ENABLE__SHIFT as u8,
        DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_TOP_ADDRESS: DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_ENABLE: DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_ENABLE__SHIFT as u8,
        DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_TOP_ADDRESS: DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_TOP_ADDRESS__SHIFT as u8,
        DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_ENABLE: DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_ENABLE__SHIFT as u8,
        CC_DC_PIPE_DIS__DC_DMCUB_ENABLE: CC_DC_PIPE_DIS__DC_DMCUB_ENABLE__SHIFT as u8,
        MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET: MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET__SHIFT as u8,
        DCN_VM_FB_LOCATION_BASE__FB_BASE: DCN_VM_FB_LOCATION_BASE__FB_BASE__SHIFT as u8,
        DCN_VM_FB_OFFSET__FB_OFFSET: DCN_VM_FB_OFFSET__FB_OFFSET__SHIFT as u8,
        DMCUB_INTERRUPT_ACK__DMCUB_OUTBOX0_READY_INT_ACK: DMCUB_INTERRUPT_ACK__DMCUB_OUTBOX0_READY_INT_ACK__SHIFT as u8,
    },
};

/// `struct dmub_reg_value_masks` (dmub_reg.c).
#[derive(Default, Clone, Copy)]
struct dmub_reg_value_masks {
    value: u32,
    mask: u32,
}

/// `set_reg_field_value_masks` (dmub_reg.c).
fn set_reg_field_value_masks(field_value_mask: &mut dmub_reg_value_masks, value: u32, mask: u32, shift: u8) {
    field_value_mask.value = (field_value_mask.value & !mask) | (mask & (value << shift));
    field_value_mask.mask |= mask;
}

/// `set_reg_field_values` (dmub_reg.c).
fn set_reg_field_values(field_value_mask: &mut dmub_reg_value_masks, fields: &[(u8, u32, u32)]) {
    for &(shift, mask, field_value) in fields {
        set_reg_field_value_masks(field_value_mask, field_value, mask, shift);
    }
}

/// `get_reg_field_value_ex` (dmub_reg.c).
fn get_reg_field_value_ex(reg_value: u32, mask: u32, shift: u8) -> u32 {
    (mask & reg_value) >> shift
}

/// `dmub_reg_update` (dmub_reg.c).
pub fn dmub_reg_update(adev: &amdgpu_device, addr: u32, fields: &[(u8, u32, u32)]) {
    let mut field_value_mask = dmub_reg_value_masks::default();
    set_reg_field_values(&mut field_value_mask, fields);
    let mut reg_val = dm_read_reg(adev, addr);
    reg_val = (reg_val & !field_value_mask.mask) | field_value_mask.value;
    dm_write_reg(adev, addr, reg_val);
}

/// `dmub_reg_set` (dmub_reg.c).
pub fn dmub_reg_set(adev: &amdgpu_device, addr: u32, mut reg_val: u32, fields: &[(u8, u32, u32)]) {
    let mut field_value_mask = dmub_reg_value_masks::default();
    set_reg_field_values(&mut field_value_mask, fields);
    reg_val = (reg_val & !field_value_mask.mask) | field_value_mask.value;
    dm_write_reg(adev, addr, reg_val);
}

/// `dmub_reg_get` (dmub_reg.c).
pub fn dmub_reg_get(adev: &amdgpu_device, addr: u32, shift: u8, mask: u32) -> u32 {
    let reg_val = dm_read_reg(adev, addr);
    get_reg_field_value_ex(reg_val, mask, shift)
}

fn regs(dmub: &dmub_srv) -> &'static dmub_srv_common_regs {
    dmub.regs.unwrap_or(&dmub_srv_dcn21_regs)
}

/// REG_SET_2 of a `*_TOP_ADDRESS` register: its TOP_ADDRESS and ENABLE fields.
fn set_top(adev: &amdgpu_device, addr: u32, top: (u8, u32, u32), enable: (u8, u32, u32)) {
    dmub_reg_set(adev, addr, 0, &[top, enable]);
}

/// `union dmub_gpint_data_register` (dmub_cmd.h): param 15:0,
/// command_code 27:16, status 31:28.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub struct dmub_gpint_data_register {
    pub all: u32,
}

impl dmub_gpint_data_register {
    pub fn new(status: u32, command_code: u32, param: u32) -> Self {
        dmub_gpint_data_register { all: (param & 0xffff) | (command_code & 0xfff) << 16 | (status & 0xf) << 28 }
    }
    pub fn set_status(&mut self, status: u32) {
        self.all = (self.all & 0x0fff_ffff) | (status & 0xf) << 28;
    }
}

/// `union dmub_fw_boot_status` (dmub_cmd.h).
#[derive(Default, Clone, Copy)]
pub struct dmub_fw_boot_status {
    pub all: u32,
}

impl dmub_fw_boot_status {
    pub fn dal_fw(self) -> bool {
        self.all & 1 != 0
    }
    pub fn mailbox_rdy(self) -> bool {
        self.all & 2 != 0
    }
}

/// `skip_phy_init_panel_sequence`, bit 5 of `union dmub_fw_boot_options`.
const BOOT_OPTIONS_SKIP_PHY_INIT_PANEL_SEQUENCE: u32 = 1 << 5;

/// `dmub_dcn20_get_fb_base_offset`.
fn dmub_dcn20_get_fb_base_offset(adev: &amdgpu_device, dmub: &dmub_srv) -> (u64, u64) {
    if dmub.fb_base != 0 || dmub.fb_offset != 0 {
        return (dmub.fb_base, dmub.fb_offset);
    }
    let r = regs(dmub);
    let tmp = dmub_reg_get(adev, r.offset.DCN_VM_FB_LOCATION_BASE, r.shift.DCN_VM_FB_LOCATION_BASE__FB_BASE,
        r.mask.DCN_VM_FB_LOCATION_BASE__FB_BASE);
    let fb_base = (tmp as u64) << 24;
    let tmp = dmub_reg_get(adev, r.offset.DCN_VM_FB_OFFSET, r.shift.DCN_VM_FB_OFFSET__FB_OFFSET,
        r.mask.DCN_VM_FB_OFFSET__FB_OFFSET);
    let fb_offset = (tmp as u64) << 24;
    (fb_base, fb_offset)
}

/// `dmub_dcn20_translate_addr`.
fn dmub_dcn20_translate_addr(addr_in: u64, fb_base: u64, fb_offset: u64) -> u64 {
    addr_in.wrapping_sub(fb_base).wrapping_add(fb_offset)
}

/// `DMUB_FW_VERSION` (dmub_srv.h).
pub const fn DMUB_FW_VERSION(major: u32, minor: u32, revision: u32) -> u32 {
    (major & 0xff) << 24 | (minor & 0xff) << 16 | (revision & 0xff) << 8
}

/// `dmub_dcn20_use_cached_inbox`: cached inbox is not supported in the fw
/// version range 1.0.0 to 1.10.0.
pub fn dmub_dcn20_use_cached_inbox(dmub: &dmub_srv) -> bool {
    !(dmub.fw_version >= DMUB_FW_VERSION(1, 0, 0) && dmub.fw_version <= DMUB_FW_VERSION(1, 10, 0))
}

/// `dmub_dcn20_reset`. Each register check of the GPINT handshake takes
/// more than 1 us, so the 30 tries bound it without a delay.
pub fn dmub_dcn20_reset(adev: &amdgpu_device, dmub: &mut dmub_srv) {
    let r = regs(dmub);
    let timeout = 30;
    let in_reset = dmub_reg_get(adev, r.offset.DMCUB_CNTL, r.shift.DMCUB_CNTL__DMCUB_SOFT_RESET,
        r.mask.DMCUB_CNTL__DMCUB_SOFT_RESET);

    if in_reset == 0 {
        let mut cmd = dmub_gpint_data_register::new(1, DMUB_GPINT__STOP_FW, 0);
        let hw = dmub.hw_funcs;
        if let Some(set_gpint) = hw.set_gpint {
            set_gpint(adev, dmub, cmd);
        }

        // The timeout covers both the ACK and the wait for remaining work.
        for _ in 0..timeout {
            if hw.is_gpint_acked.is_some_and(|f| f(adev, dmub, cmd)) {
                break;
            }
        }
        for _ in 0..timeout {
            let scratch = hw.get_gpint_response.map_or(0, |f| f(adev, dmub));
            if scratch == DMUB_GPINT__STOP_FW_RESPONSE {
                break;
            }
        }

        // Clear the GPINT command manually so we don't reset again.
        cmd.all = 0;
        if let Some(set_gpint) = hw.set_gpint {
            set_gpint(adev, dmub, cmd);
        }
        // Force reset in case we timed out, DMCUB is likely hung.
    }

    dmub_reg_update(adev, r.offset.DMCUB_CNTL,
        &[(r.shift.DMCUB_CNTL__DMCUB_SOFT_RESET, r.mask.DMCUB_CNTL__DMCUB_SOFT_RESET, 1)]);
    dmub_reg_update(adev, r.offset.DMCUB_CNTL,
        &[(r.shift.DMCUB_CNTL__DMCUB_ENABLE, r.mask.DMCUB_CNTL__DMCUB_ENABLE, 0)]);
    dmub_reg_update(adev, r.offset.MMHUBBUB_SOFT_RESET,
        &[(r.shift.MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET, r.mask.MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET, 1)]);
    dm_write_reg(adev, r.offset.DMCUB_INBOX1_RPTR, 0);
    dm_write_reg(adev, r.offset.DMCUB_INBOX1_WPTR, 0);
    dm_write_reg(adev, r.offset.DMCUB_OUTBOX1_RPTR, 0);
    dm_write_reg(adev, r.offset.DMCUB_OUTBOX1_WPTR, 0);
    dm_write_reg(adev, r.offset.DMCUB_SCRATCH0, 0);
}

/// `dmub_dcn20_reset_release`.
pub fn dmub_dcn20_reset_release(adev: &amdgpu_device, dmub: &mut dmub_srv) {
    let r = regs(dmub);
    dmub_reg_update(adev, r.offset.MMHUBBUB_SOFT_RESET,
        &[(r.shift.MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET, r.mask.MMHUBBUB_SOFT_RESET__DMUIF_SOFT_RESET, 0)]);
    dm_write_reg(adev, r.offset.DMCUB_SCRATCH15, dmub.psp_version & 0x0011_00FF);
    dmub_reg_update(adev, r.offset.DMCUB_CNTL, &[
        (r.shift.DMCUB_CNTL__DMCUB_ENABLE, r.mask.DMCUB_CNTL__DMCUB_ENABLE, 1),
        (r.shift.DMCUB_CNTL__DMCUB_TRACEPORT_EN, r.mask.DMCUB_CNTL__DMCUB_TRACEPORT_EN, 1),
    ]);
    dmub_reg_update(adev, r.offset.DMCUB_CNTL,
        &[(r.shift.DMCUB_CNTL__DMCUB_SOFT_RESET, r.mask.DMCUB_CNTL__DMCUB_SOFT_RESET, 0)]);
}

/// `dmub_dcn20_backdoor_load`.
pub fn dmub_dcn20_backdoor_load(adev: &amdgpu_device, dmub: &mut dmub_srv, cw0: &dmub_window, cw1: &dmub_window) {
    let r = regs(dmub);
    let (fb_base, fb_offset) = dmub_dcn20_get_fb_base_offset(adev, dmub);

    dmub_reg_update(adev, r.offset.DMCUB_SEC_CNTL,
        &[(r.shift.DMCUB_SEC_CNTL__DMCUB_SEC_RESET, r.mask.DMCUB_SEC_CNTL__DMCUB_SEC_RESET, 1)]);
    dmub_reg_update(adev, r.offset.DMCUB_MEM_CNTL, &[
        (r.shift.DMCUB_MEM_CNTL__DMCUB_MEM_READ_SPACE, r.mask.DMCUB_MEM_CNTL__DMCUB_MEM_READ_SPACE, 0x3),
        (r.shift.DMCUB_MEM_CNTL__DMCUB_MEM_WRITE_SPACE, r.mask.DMCUB_MEM_CNTL__DMCUB_MEM_WRITE_SPACE, 0x3),
    ]);

    let offset = dmub_dcn20_translate_addr(cw0.offset, fb_base, fb_offset);
    dm_write_reg(adev, r.offset.DMCUB_REGION3_CW0_OFFSET, offset as u32);
    dm_write_reg(adev, r.offset.DMCUB_REGION3_CW0_OFFSET_HIGH, (offset >> 32) as u32);
    dm_write_reg(adev, r.offset.DMCUB_REGION3_CW0_BASE_ADDRESS, cw0.region.base);
    set_top(adev, r.offset.DMCUB_REGION3_CW0_TOP_ADDRESS,
        (r.shift.DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_TOP_ADDRESS,
         r.mask.DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_TOP_ADDRESS, cw0.region.top),
        (r.shift.DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_ENABLE,
         r.mask.DMCUB_REGION3_CW0_TOP_ADDRESS__DMCUB_REGION3_CW0_ENABLE, 1));

    let offset = dmub_dcn20_translate_addr(cw1.offset, fb_base, fb_offset);
    dm_write_reg(adev, r.offset.DMCUB_REGION3_CW1_OFFSET, offset as u32);
    dm_write_reg(adev, r.offset.DMCUB_REGION3_CW1_OFFSET_HIGH, (offset >> 32) as u32);
    dm_write_reg(adev, r.offset.DMCUB_REGION3_CW1_BASE_ADDRESS, cw1.region.base);
    set_top(adev, r.offset.DMCUB_REGION3_CW1_TOP_ADDRESS,
        (r.shift.DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_TOP_ADDRESS,
         r.mask.DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_TOP_ADDRESS, cw1.region.top),
        (r.shift.DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_ENABLE,
         r.mask.DMCUB_REGION3_CW1_TOP_ADDRESS__DMCUB_REGION3_CW1_ENABLE, 1));

    dmub_reg_update(adev, r.offset.DMCUB_SEC_CNTL, &[
        (r.shift.DMCUB_SEC_CNTL__DMCUB_SEC_RESET, r.mask.DMCUB_SEC_CNTL__DMCUB_SEC_RESET, 0),
        (r.shift.DMCUB_SEC_CNTL__DMCUB_MEM_UNIT_ID, r.mask.DMCUB_SEC_CNTL__DMCUB_MEM_UNIT_ID, 0x20),
    ]);
}

/// One `DMCUB_REGION3_CWn` window: OFFSET, OFFSET_HIGH, BASE_ADDRESS and
/// TOP_ADDRESS with its enable, as each window of setup_windows writes it.
fn program_cw(adev: &amdgpu_device, offset: u64, w: &dmub_window, regs4: (u32, u32, u32, u32),
              top: (u8, u32), enable: (u8, u32)) {
    dm_write_reg(adev, regs4.0, offset as u32);
    dm_write_reg(adev, regs4.1, (offset >> 32) as u32);
    dm_write_reg(adev, regs4.2, w.region.base);
    set_top(adev, regs4.3, (top.0, top.1, w.region.top), (enable.0, enable.1, 1));
}

/// `dmub_dcn20_setup_windows`. `region6` is not programmed on DCN 2.0.
pub fn dmub_dcn20_setup_windows(adev: &amdgpu_device, dmub: &mut dmub_srv, cw2: &dmub_window, cw3: &dmub_window,
                                cw4: &dmub_window, cw5: &dmub_window, cw6: &dmub_window, _region6: &dmub_window) {
    let r = regs(dmub);
    let (o, m, s) = (&r.offset, &r.mask, &r.shift);
    let (fb_base, fb_offset) = dmub_dcn20_get_fb_base_offset(adev, dmub);

    if cw2.region.base != cw2.region.top {
        let offset = dmub_dcn20_translate_addr(cw2.offset, fb_base, fb_offset);
        program_cw(adev, offset, cw2,
            (o.DMCUB_REGION3_CW2_OFFSET, o.DMCUB_REGION3_CW2_OFFSET_HIGH, o.DMCUB_REGION3_CW2_BASE_ADDRESS,
             o.DMCUB_REGION3_CW2_TOP_ADDRESS),
            (s.DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_TOP_ADDRESS,
             m.DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_TOP_ADDRESS),
            (s.DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_ENABLE,
             m.DMCUB_REGION3_CW2_TOP_ADDRESS__DMCUB_REGION3_CW2_ENABLE));
    } else {
        dm_write_reg(adev, o.DMCUB_REGION3_CW2_OFFSET, 0);
        dm_write_reg(adev, o.DMCUB_REGION3_CW2_OFFSET_HIGH, 0);
        dm_write_reg(adev, o.DMCUB_REGION3_CW2_BASE_ADDRESS, 0);
        dm_write_reg(adev, o.DMCUB_REGION3_CW2_TOP_ADDRESS, 0);
    }

    let offset = dmub_dcn20_translate_addr(cw3.offset, fb_base, fb_offset);
    program_cw(adev, offset, cw3,
        (o.DMCUB_REGION3_CW3_OFFSET, o.DMCUB_REGION3_CW3_OFFSET_HIGH, o.DMCUB_REGION3_CW3_BASE_ADDRESS,
         o.DMCUB_REGION3_CW3_TOP_ADDRESS),
        (s.DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_TOP_ADDRESS,
         m.DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_TOP_ADDRESS),
        (s.DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_ENABLE,
         m.DMCUB_REGION3_CW3_TOP_ADDRESS__DMCUB_REGION3_CW3_ENABLE));

    let offset = dmub_dcn20_translate_addr(cw4.offset, fb_base, fb_offset);
    // Firmware outside 1.0.0..=1.10.0 supports CW4.
    if dmub_dcn20_use_cached_inbox(dmub) {
        program_cw(adev, offset, cw4,
            (o.DMCUB_REGION3_CW4_OFFSET, o.DMCUB_REGION3_CW4_OFFSET_HIGH, o.DMCUB_REGION3_CW4_BASE_ADDRESS,
             o.DMCUB_REGION3_CW4_TOP_ADDRESS),
            (s.DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_TOP_ADDRESS,
             m.DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_TOP_ADDRESS),
            (s.DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_ENABLE,
             m.DMCUB_REGION3_CW4_TOP_ADDRESS__DMCUB_REGION3_CW4_ENABLE));
    } else {
        dm_write_reg(adev, o.DMCUB_REGION4_OFFSET, offset as u32);
        dm_write_reg(adev, o.DMCUB_REGION4_OFFSET_HIGH, (offset >> 32) as u32);
        set_top(adev, o.DMCUB_REGION4_TOP_ADDRESS,
            (s.DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_TOP_ADDRESS, m.DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_TOP_ADDRESS,
             cw4.region.top.wrapping_sub(cw4.region.base).wrapping_sub(1)),
            (s.DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_ENABLE, m.DMCUB_REGION4_TOP_ADDRESS__DMCUB_REGION4_ENABLE, 1));
    }

    let offset = dmub_dcn20_translate_addr(cw5.offset, fb_base, fb_offset);
    program_cw(adev, offset, cw5,
        (o.DMCUB_REGION3_CW5_OFFSET, o.DMCUB_REGION3_CW5_OFFSET_HIGH, o.DMCUB_REGION3_CW5_BASE_ADDRESS,
         o.DMCUB_REGION3_CW5_TOP_ADDRESS),
        (s.DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_TOP_ADDRESS,
         m.DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_TOP_ADDRESS),
        (s.DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_ENABLE,
         m.DMCUB_REGION3_CW5_TOP_ADDRESS__DMCUB_REGION3_CW5_ENABLE));

    dm_write_reg(adev, o.DMCUB_REGION5_OFFSET, offset as u32);
    dm_write_reg(adev, o.DMCUB_REGION5_OFFSET_HIGH, (offset >> 32) as u32);
    set_top(adev, o.DMCUB_REGION5_TOP_ADDRESS,
        (s.DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_TOP_ADDRESS, m.DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_TOP_ADDRESS,
         cw5.region.top.wrapping_sub(cw5.region.base).wrapping_sub(1)),
        (s.DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_ENABLE, m.DMCUB_REGION5_TOP_ADDRESS__DMCUB_REGION5_ENABLE, 1));

    let offset = dmub_dcn20_translate_addr(cw6.offset, fb_base, fb_offset);
    program_cw(adev, offset, cw6,
        (o.DMCUB_REGION3_CW6_OFFSET, o.DMCUB_REGION3_CW6_OFFSET_HIGH, o.DMCUB_REGION3_CW6_BASE_ADDRESS,
         o.DMCUB_REGION3_CW6_TOP_ADDRESS),
        (s.DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_TOP_ADDRESS,
         m.DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_TOP_ADDRESS),
        (s.DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_ENABLE,
         m.DMCUB_REGION3_CW6_TOP_ADDRESS__DMCUB_REGION3_CW6_ENABLE));
}

/// `dmub_dcn20_setup_mailbox`. Firmware outside 1.0.0..=1.10.0 supports
/// CW4 for the inbox.
pub fn dmub_dcn20_setup_mailbox(adev: &amdgpu_device, dmub: &mut dmub_srv, inbox1: &dmub_region) {
    let r = regs(dmub);
    if dmub_dcn20_use_cached_inbox(dmub) {
        dm_write_reg(adev, r.offset.DMCUB_INBOX1_BASE_ADDRESS, inbox1.base);
    } else {
        dm_write_reg(adev, r.offset.DMCUB_INBOX1_BASE_ADDRESS, 0x8000_0000);
    }
    dm_write_reg(adev, r.offset.DMCUB_INBOX1_SIZE, inbox1.top.wrapping_sub(inbox1.base));
}

/// `dmub_dcn20_get_inbox1_wptr`.
pub fn dmub_dcn20_get_inbox1_wptr(adev: &amdgpu_device, dmub: &dmub_srv) -> u32 {
    dm_read_reg(adev, regs(dmub).offset.DMCUB_INBOX1_WPTR)
}

/// `dmub_dcn20_get_inbox1_rptr`.
pub fn dmub_dcn20_get_inbox1_rptr(adev: &amdgpu_device, dmub: &dmub_srv) -> u32 {
    dm_read_reg(adev, regs(dmub).offset.DMCUB_INBOX1_RPTR)
}

/// `dmub_dcn20_set_inbox1_wptr`.
pub fn dmub_dcn20_set_inbox1_wptr(adev: &amdgpu_device, dmub: &dmub_srv, wptr_offset: u32) {
    dm_write_reg(adev, regs(dmub).offset.DMCUB_INBOX1_WPTR, wptr_offset);
}

/// `dmub_dcn20_setup_out_mailbox`. Firmware outside 1.0.0..=1.10.0
/// supports CW4 for the outbox.
pub fn dmub_dcn20_setup_out_mailbox(adev: &amdgpu_device, dmub: &mut dmub_srv, outbox1: &dmub_region) {
    let r = regs(dmub);
    if dmub_dcn20_use_cached_inbox(dmub) {
        dm_write_reg(adev, r.offset.DMCUB_OUTBOX1_BASE_ADDRESS, outbox1.base);
    } else {
        dm_write_reg(adev, r.offset.DMCUB_OUTBOX1_BASE_ADDRESS, 0x8000_2000);
    }
    dm_write_reg(adev, r.offset.DMCUB_OUTBOX1_SIZE, outbox1.top.wrapping_sub(outbox1.base));
}

/// `dmub_dcn20_get_outbox1_wptr`: read without locks, by
/// `dmub_srv_stat_get_notification` only.
pub fn dmub_dcn20_get_outbox1_wptr(adev: &amdgpu_device, dmub: &dmub_srv) -> u32 {
    dm_read_reg(adev, regs(dmub).offset.DMCUB_OUTBOX1_WPTR)
}

/// `dmub_dcn20_set_outbox1_rptr`: written without locks, by
/// `dmub_srv_stat_get_notification` only.
pub fn dmub_dcn20_set_outbox1_rptr(adev: &amdgpu_device, dmub: &dmub_srv, rptr_offset: u32) {
    dm_write_reg(adev, regs(dmub).offset.DMCUB_OUTBOX1_RPTR, rptr_offset);
}

/// `dmub_dcn20_setup_outbox0`.
pub fn dmub_dcn20_setup_outbox0(adev: &amdgpu_device, dmub: &mut dmub_srv, outbox0: &dmub_region) {
    let r = regs(dmub);
    dm_write_reg(adev, r.offset.DMCUB_OUTBOX0_BASE_ADDRESS, outbox0.base);
    dm_write_reg(adev, r.offset.DMCUB_OUTBOX0_SIZE, outbox0.top.wrapping_sub(outbox0.base));
}

/// `dmub_dcn20_get_outbox0_wptr`.
pub fn dmub_dcn20_get_outbox0_wptr(adev: &amdgpu_device, dmub: &dmub_srv) -> u32 {
    dm_read_reg(adev, regs(dmub).offset.DMCUB_OUTBOX0_WPTR)
}

/// `dmub_dcn20_set_outbox0_rptr`.
pub fn dmub_dcn20_set_outbox0_rptr(adev: &amdgpu_device, dmub: &dmub_srv, rptr_offset: u32) {
    dm_write_reg(adev, regs(dmub).offset.DMCUB_OUTBOX0_RPTR, rptr_offset);
}

/// `dmub_dcn20_is_hw_init`.
pub fn dmub_dcn20_is_hw_init(adev: &amdgpu_device, dmub: &dmub_srv) -> bool {
    let r = regs(dmub);
    dmub_reg_get(adev, r.offset.DMCUB_CNTL, r.shift.DMCUB_CNTL__DMCUB_ENABLE, r.mask.DMCUB_CNTL__DMCUB_ENABLE) != 0
}

/// `dmub_dcn20_is_supported`.
pub fn dmub_dcn20_is_supported(adev: &amdgpu_device, dmub: &dmub_srv) -> bool {
    let r = regs(dmub);
    dmub_reg_get(adev, r.offset.CC_DC_PIPE_DIS, r.shift.CC_DC_PIPE_DIS__DC_DMCUB_ENABLE,
        r.mask.CC_DC_PIPE_DIS__DC_DMCUB_ENABLE) != 0
}

/// `dmub_dcn20_set_gpint`.
pub fn dmub_dcn20_set_gpint(adev: &amdgpu_device, dmub: &dmub_srv, reg: dmub_gpint_data_register) {
    dm_write_reg(adev, regs(dmub).offset.DMCUB_GPINT_DATAIN1, reg.all);
}

/// `dmub_dcn20_is_gpint_acked`: the firmware acknowledges by clearing the
/// status bits.
pub fn dmub_dcn20_is_gpint_acked(adev: &amdgpu_device, dmub: &dmub_srv, mut reg: dmub_gpint_data_register) -> bool {
    reg.set_status(0);
    let test = dm_read_reg(adev, regs(dmub).offset.DMCUB_GPINT_DATAIN1);
    test == reg.all
}

/// `dmub_dcn20_get_gpint_response`.
pub fn dmub_dcn20_get_gpint_response(adev: &amdgpu_device, dmub: &dmub_srv) -> u32 {
    dm_read_reg(adev, regs(dmub).offset.DMCUB_SCRATCH7)
}

/// `dmub_dcn20_get_fw_boot_status`.
pub fn dmub_dcn20_get_fw_boot_status(adev: &amdgpu_device, dmub: &dmub_srv) -> dmub_fw_boot_status {
    dmub_fw_boot_status { all: dm_read_reg(adev, regs(dmub).offset.DMCUB_SCRATCH0) }
}

/// `dmub_dcn20_enable_dmub_boot_options`: DCN 2.0 reports no options.
pub fn dmub_dcn20_enable_dmub_boot_options(adev: &amdgpu_device, dmub: &dmub_srv, _params: &dmub_srv_hw_params) {
    dm_write_reg(adev, regs(dmub).offset.DMCUB_SCRATCH14, 0);
}

/// `dmub_dcn20_skip_dmub_panel_power_sequence`.
pub fn dmub_dcn20_skip_dmub_panel_power_sequence(adev: &amdgpu_device, dmub: &dmub_srv, skip: bool) {
    let r = regs(dmub);
    let mut all = dm_read_reg(adev, r.offset.DMCUB_SCRATCH14);
    all = (all & !BOOT_OPTIONS_SKIP_PHY_INIT_PANEL_SEQUENCE)
        | if skip { BOOT_OPTIONS_SKIP_PHY_INIT_PANEL_SEQUENCE } else { 0 };
    dm_write_reg(adev, r.offset.DMCUB_SCRATCH14, all);
}

/// `dmub_dcn20_get_current_time`.
pub fn dmub_dcn20_get_current_time(adev: &amdgpu_device, dmub: &dmub_srv) -> u32 {
    dm_read_reg(adev, regs(dmub).offset.DMCUB_TIMER_CURRENT)
}
