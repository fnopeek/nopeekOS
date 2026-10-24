//! display/dc/irq/irq_service.c and irq/dcn21/irq_service_dcn21.c (Linux
//! 6.18.55): DC's interrupt sources, their enable and acknowledge
//! registers, and the IV source ids they arrive under.
#![allow(dead_code, non_camel_case_types)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc_helper::{dm_read_reg, dm_write_reg};
use crate::linux::Line;
use crate::regs::*;

/// Words per `struct irq_source_info` in `irq_source_info_dcn21`.
const INFO_WORDS: usize = (SIZEOF_irq_source_info / 4) as usize;

/// `struct irq_source_info` (irq_service.h). `funcs` is the table entry's
/// `struct irq_source_info_funcs`, by name; `None` for NULL.
#[derive(Clone, Copy, Debug)]
pub struct irq_source_info {
    pub src_id: u32,
    pub ext_id: u32,
    pub enable_reg: u32,
    pub enable_mask: u32,
    pub enable_value: [u32; 2],
    pub ack_reg: u32,
    pub ack_mask: u32,
    pub ack_value: u32,
    pub status_reg: u32,
    pub funcs: Option<&'static str>,
}

/// `struct irq_service` (irq_service.h): DCN 2.1's table and source map.
#[derive(Debug)]
pub struct irq_service {
    pub info: &'static [u32],
    pub to_dal_irq_source: fn(u32, u32) -> u32,
}

/// `dal_irq_service_dcn21_create` → `dcn21_irq_construct`
/// (irq_service_dcn21.c).
pub fn dal_irq_service_dcn21_create() -> alloc::boxed::Box<irq_service> {
    alloc::boxed::Box::new(irq_service {
        info: &crate::tables::irq_source_info_dcn21,
        to_dal_irq_source: to_dal_irq_source_dcn21,
    })
}

/// `find_irq_source_info` (irq_service.c).
fn find_irq_source_info(irq_service: &irq_service, source: u32) -> Option<irq_source_info> {
    if source >= DAL_IRQ_SOURCES_NUMBER {
        return None;
    }

    let w = &irq_service.info[source as usize * INFO_WORDS..][..INFO_WORDS];
    let funcs = match w[INFO_FUNCS] {
        0 => None,
        n => Some(crate::tables::irq_source_info_dcn21_SYMS[n as usize - 1]),
    };
    Some(irq_source_info {
        src_id: w[INFO_SRC_ID],
        ext_id: w[INFO_EXT_ID],
        enable_reg: w[INFO_ENABLE_REG],
        enable_mask: w[INFO_ENABLE_MASK],
        enable_value: [w[INFO_ENABLE_VALUE], w[INFO_ENABLE_VALUE + 1]],
        ack_reg: w[INFO_ACK_REG],
        ack_mask: w[INFO_ACK_MASK],
        ack_value: w[INFO_ACK_VALUE],
        status_reg: w[INFO_STATUS_REG],
        funcs,
    })
}

const INFO_SRC_ID: usize = (IRQINFO_SRC_ID / 4) as usize;
const INFO_EXT_ID: usize = (IRQINFO_EXT_ID / 4) as usize;
const INFO_ENABLE_REG: usize = (IRQINFO_ENABLE_REG / 4) as usize;
const INFO_ENABLE_MASK: usize = (IRQINFO_ENABLE_MASK / 4) as usize;
const INFO_ENABLE_VALUE: usize = (IRQINFO_ENABLE_VALUE / 4) as usize;
const INFO_ACK_REG: usize = (IRQINFO_ACK_REG / 4) as usize;
const INFO_ACK_MASK: usize = (IRQINFO_ACK_MASK / 4) as usize;
const INFO_ACK_VALUE: usize = (IRQINFO_ACK_VALUE / 4) as usize;
const INFO_STATUS_REG: usize = (IRQINFO_STATUS_REG / 4) as usize;
const INFO_FUNCS: usize = (IRQINFO_FUNCS / 4) as usize;

/// `dal_irq_service_set_generic` (irq_service.c).
pub fn dal_irq_service_set_generic(adev: &amdgpu_device, info: &irq_source_info, enable: bool) {
    let addr = info.enable_reg;
    let mut value = dm_read_reg(adev, addr);

    value = (value & !info.enable_mask) | (info.enable_value[if enable { 0 } else { 1 }] & info.enable_mask);
    dm_write_reg(adev, addr, value);
}

/// `dal_irq_service_set` (irq_service.c).
pub fn dal_irq_service_set(adev: &amdgpu_device, irq_service: &irq_service, source: u32, enable: bool) -> bool {
    let Some(info) = find_irq_source_info(irq_service, source) else {
        Line::new().s("dal_irq_service_set: cannot find irq info table entry for ").dec(source as u64).say();
        return false;
    };

    dal_irq_service_ack(adev, irq_service, source);

    match info.funcs {
        Some("dummy_irq_info_funcs") => {
            Line::new().s("dal_irq_service_set: src: ").dec(source as u64).s(", st: ").dec(enable as u64).say();
            dal_irq_service_dummy_set(&info, enable)
        }
        // Every other funcs table of irq_service_dcn21.c has set NULL.
        _ => {
            dal_irq_service_set_generic(adev, &info, enable);
            true
        }
    }
}

/// `dal_irq_service_ack_generic` (irq_service.c).
pub fn dal_irq_service_ack_generic(adev: &amdgpu_device, info: &irq_source_info) {
    let addr = info.ack_reg;
    let mut value = dm_read_reg(adev, addr);

    value = (value & !info.ack_mask) | (info.ack_value & info.ack_mask);
    dm_write_reg(adev, addr, value);
}

/// `dal_irq_service_ack` (irq_service.c).
pub fn dal_irq_service_ack(adev: &amdgpu_device, irq_service: &irq_service, source: u32) -> bool {
    let Some(info) = find_irq_source_info(irq_service, source) else {
        Line::new().s("dal_irq_service_ack: cannot find irq info table entry for ").dec(source as u64).say();
        return false;
    };

    match info.funcs {
        Some("dummy_irq_info_funcs") => {
            Line::new().s("dal_irq_service_ack: src: ").dec(source as u64).say();
            dal_irq_service_dummy_ack(&info)
        }
        Some("hpd_irq_info_funcs") => hpd0_ack(adev, &info),
        _ => {
            dal_irq_service_ack_generic(adev, &info);
            true
        }
    }
}

/// `dal_irq_service_to_irq_source` (irq_service.c).
pub fn dal_irq_service_to_irq_source(irq_service: &irq_service, src_id: u32, ext_id: u32) -> u32 {
    (irq_service.to_dal_irq_source)(src_id, ext_id)
}

/// `hpd0_ack` (irq_service.c): acknowledge, then arm the HPD interrupt for
/// the opposite level of what the sense line shows now.
pub fn hpd0_ack(adev: &amdgpu_device, info: &irq_source_info) -> bool {
    let addr = info.status_reg;
    let value = dm_read_reg(adev, addr);
    let current_status = (value & HPD0_DC_HPD_INT_STATUS__DC_HPD_SENSE_DELAYED_MASK)
        >> HPD0_DC_HPD_INT_STATUS__DC_HPD_SENSE_DELAYED__SHIFT;

    dal_irq_service_ack_generic(adev, info);

    let mut value = dm_read_reg(adev, info.enable_reg);

    value = (value & !HPD0_DC_HPD_INT_CONTROL__DC_HPD_INT_POLARITY_MASK)
        | ((if current_status != 0 { 0 } else { 1 }) << HPD0_DC_HPD_INT_CONTROL__DC_HPD_INT_POLARITY__SHIFT
            & HPD0_DC_HPD_INT_CONTROL__DC_HPD_INT_POLARITY_MASK);

    dm_write_reg(adev, info.enable_reg, value);

    true
}

/// `dal_irq_service_dummy_set` (irq_service_dce110.c).
fn dal_irq_service_dummy_set(info: &irq_source_info, _enable: bool) -> bool {
    Line::new().s("dal_irq_service_dummy_set: called for non-implemented irq source, src_id=")
        .dec(info.src_id as u64).s(", ext_id=").dec(info.ext_id as u64).say();
    false
}

/// `dal_irq_service_dummy_ack` (irq_service_dce110.c).
fn dal_irq_service_dummy_ack(info: &irq_source_info) -> bool {
    Line::new().s("dal_irq_service_dummy_ack: called for non-implemented irq source, src_id=")
        .dec(info.src_id as u64).s(", ext_id=").dec(info.ext_id as u64).say();
    false
}

/// `to_dal_irq_source_dcn21` (irq_service_dcn21.c).
fn to_dal_irq_source_dcn21(src_id: u32, ext_id: u32) -> u32 {
    match src_id {
        DCN_1_0__SRCID__DC_D1_OTG_VSTARTUP => DC_IRQ_SOURCE_VBLANK1,
        DCN_1_0__SRCID__DC_D2_OTG_VSTARTUP => DC_IRQ_SOURCE_VBLANK2,
        DCN_1_0__SRCID__DC_D3_OTG_VSTARTUP => DC_IRQ_SOURCE_VBLANK3,
        DCN_1_0__SRCID__DC_D4_OTG_VSTARTUP => DC_IRQ_SOURCE_VBLANK4,
        DCN_1_0__SRCID__DC_D5_OTG_VSTARTUP => DC_IRQ_SOURCE_VBLANK5,
        DCN_1_0__SRCID__DC_D6_OTG_VSTARTUP => DC_IRQ_SOURCE_VBLANK6,
        DCN_1_0__SRCID__DMCUB_OUTBOX_LOW_PRIORITY_READY_INT => DC_IRQ_SOURCE_DMCUB_OUTBOX,
        DCN_1_0__SRCID__OTG1_VERTICAL_INTERRUPT0_CONTROL => DC_IRQ_SOURCE_DC1_VLINE0,
        DCN_1_0__SRCID__OTG2_VERTICAL_INTERRUPT0_CONTROL => DC_IRQ_SOURCE_DC2_VLINE0,
        DCN_1_0__SRCID__OTG3_VERTICAL_INTERRUPT0_CONTROL => DC_IRQ_SOURCE_DC3_VLINE0,
        DCN_1_0__SRCID__OTG4_VERTICAL_INTERRUPT0_CONTROL => DC_IRQ_SOURCE_DC4_VLINE0,
        DCN_1_0__SRCID__OTG5_VERTICAL_INTERRUPT0_CONTROL => DC_IRQ_SOURCE_DC5_VLINE0,
        DCN_1_0__SRCID__OTG6_VERTICAL_INTERRUPT0_CONTROL => DC_IRQ_SOURCE_DC6_VLINE0,
        DCN_1_0__SRCID__HUBP0_FLIP_INTERRUPT => DC_IRQ_SOURCE_PFLIP1,
        DCN_1_0__SRCID__HUBP1_FLIP_INTERRUPT => DC_IRQ_SOURCE_PFLIP2,
        DCN_1_0__SRCID__HUBP2_FLIP_INTERRUPT => DC_IRQ_SOURCE_PFLIP3,
        DCN_1_0__SRCID__HUBP3_FLIP_INTERRUPT => DC_IRQ_SOURCE_PFLIP4,
        DCN_1_0__SRCID__HUBP4_FLIP_INTERRUPT => DC_IRQ_SOURCE_PFLIP5,
        DCN_1_0__SRCID__HUBP5_FLIP_INTERRUPT => DC_IRQ_SOURCE_PFLIP6,
        DCN_1_0__SRCID__OTG0_IHC_V_UPDATE_NO_LOCK_INTERRUPT => DC_IRQ_SOURCE_VUPDATE1,
        DCN_1_0__SRCID__OTG1_IHC_V_UPDATE_NO_LOCK_INTERRUPT => DC_IRQ_SOURCE_VUPDATE2,
        DCN_1_0__SRCID__OTG2_IHC_V_UPDATE_NO_LOCK_INTERRUPT => DC_IRQ_SOURCE_VUPDATE3,
        DCN_1_0__SRCID__OTG3_IHC_V_UPDATE_NO_LOCK_INTERRUPT => DC_IRQ_SOURCE_VUPDATE4,
        DCN_1_0__SRCID__OTG4_IHC_V_UPDATE_NO_LOCK_INTERRUPT => DC_IRQ_SOURCE_VUPDATE5,
        DCN_1_0__SRCID__OTG5_IHC_V_UPDATE_NO_LOCK_INTERRUPT => DC_IRQ_SOURCE_VUPDATE6,
        // generic src_id for all HPD and HPDRX interrupts
        DCN_1_0__SRCID__DC_HPD1_INT => match ext_id {
            DCN_1_0__CTXID__DC_HPD1_INT => DC_IRQ_SOURCE_HPD1,
            DCN_1_0__CTXID__DC_HPD2_INT => DC_IRQ_SOURCE_HPD2,
            DCN_1_0__CTXID__DC_HPD3_INT => DC_IRQ_SOURCE_HPD3,
            DCN_1_0__CTXID__DC_HPD4_INT => DC_IRQ_SOURCE_HPD4,
            DCN_1_0__CTXID__DC_HPD5_INT => DC_IRQ_SOURCE_HPD5,
            DCN_1_0__CTXID__DC_HPD6_INT => DC_IRQ_SOURCE_HPD6,
            DCN_1_0__CTXID__DC_HPD1_RX_INT => DC_IRQ_SOURCE_HPD1RX,
            DCN_1_0__CTXID__DC_HPD2_RX_INT => DC_IRQ_SOURCE_HPD2RX,
            DCN_1_0__CTXID__DC_HPD3_RX_INT => DC_IRQ_SOURCE_HPD3RX,
            DCN_1_0__CTXID__DC_HPD4_RX_INT => DC_IRQ_SOURCE_HPD4RX,
            DCN_1_0__CTXID__DC_HPD5_RX_INT => DC_IRQ_SOURCE_HPD5RX,
            DCN_1_0__CTXID__DC_HPD6_RX_INT => DC_IRQ_SOURCE_HPD6RX,
            _ => DC_IRQ_SOURCE_INVALID,
        },
        _ => DC_IRQ_SOURCE_INVALID,
    }
}
