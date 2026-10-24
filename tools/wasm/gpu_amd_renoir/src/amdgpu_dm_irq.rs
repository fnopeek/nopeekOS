//! display/amdgpu_dm/amdgpu_dm_irq.c (Linux 6.18.55): the DM's interrupt
//! sources towards the base driver, and per DAL irq source the handlers DM
//! components register, called in the interrupt (high) or from work (low).
#![allow(dead_code, non_camel_case_types)]

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_irq::amdgpu_iv_entry;
use crate::linux::Line;
use crate::regs::*;
use alloc::vec::Vec;

/// A registered handler with its argument, in place of
/// `struct amdgpu_dm_irq_handler_data`'s function and `handler_arg`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dm_irq_handler {
    /// `dm_vupdate_high_irq` with `&adev->dm.vupdate_params[params]`.
    dm_vupdate_high_irq { params: usize },
    /// `dm_dmub_outbox1_low_irq`.
    dm_dmub_outbox1_low_irq,
}

/// `enum dc_interrupt_context` (irq_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dc_interrupt_context {
    INTERRUPT_LOW_IRQ_CONTEXT = 0,
    INTERRUPT_HIGH_IRQ_CONTEXT,
}

/// `struct dc_interrupt_params` (irq_types.h), the members DM sets.
#[derive(Clone, Copy, Debug)]
pub struct dc_interrupt_params {
    pub int_context: dc_interrupt_context,
    pub irq_source: u32,
}

/// `struct common_irq_params` (amdgpu_dm.h).
#[derive(Clone, Copy, Default, Debug)]
pub struct common_irq_params {
    pub irq_src: u32,
    pub previous_timestamp: u64,
}

/// `amdgpu_dm_irq_init`: an empty low and high handler list per DAL irq
/// source.
pub fn amdgpu_dm_irq_init(adev: &mut amdgpu_device) -> i32 {
    adev.dm.irq_handler_list_low_tab = (0..DAL_IRQ_SOURCES_NUMBER).map(|_| Vec::new()).collect();
    adev.dm.irq_handler_list_high_tab = (0..DAL_IRQ_SOURCES_NUMBER).map(|_| Vec::new()).collect();
    0
}

/// `validate_irq_registration_params`.
fn validate_irq_registration_params(int_params: &dc_interrupt_params) -> bool {
    // DAL_VALID_IRQ_SRC_NUM
    if !(int_params.irq_source < DAL_IRQ_SOURCES_NUMBER && int_params.irq_source > DC_IRQ_SOURCE_INVALID) {
        Line::new().s("DM_IRQ: invalid irq_source: ").dec(int_params.irq_source as u64).say();
        return false;
    }

    true
}

/// `amdgpu_dm_irq_register_interrupt`: false where Linux returns
/// `DAL_INVALID_IRQ_HANDLER_IDX`.
pub fn amdgpu_dm_irq_register_interrupt(adev: &mut amdgpu_device, int_params: &dc_interrupt_params,
                                        ih: dm_irq_handler) -> bool {
    if !validate_irq_registration_params(int_params) {
        return false;
    }

    let irq_source = int_params.irq_source as usize;

    let hnd_list = match int_params.int_context {
        dc_interrupt_context::INTERRUPT_HIGH_IRQ_CONTEXT => &mut adev.dm.irq_handler_list_high_tab[irq_source],
        dc_interrupt_context::INTERRUPT_LOW_IRQ_CONTEXT => &mut adev.dm.irq_handler_list_low_tab[irq_source],
    };

    hnd_list.push(ih);

    true
}

/// `amdgpu_dm_irq_immediate_work`: every handler registered for immediate
/// notification of `irq_source`.
fn amdgpu_dm_irq_immediate_work(adev: &mut amdgpu_device, irq_source: u32) {
    let handlers = adev.dm.irq_handler_list_high_tab[irq_source as usize].clone();
    for h in handlers {
        // Call a subcomponent which registered for immediate interrupt
        // notification
        call_handler(adev, h);
    }
}

/// `amdgpu_dm_irq_schedule_work`: the low handlers run as work, on the
/// module's one thread when its loop runs pending work.
fn amdgpu_dm_irq_schedule_work(adev: &mut amdgpu_device, irq_source: u32) {
    let handlers = adev.dm.irq_handler_list_low_tab[irq_source as usize].clone();
    if handlers.is_empty() {
        return;
    }

    for h in handlers {
        let queued = crate::workqueue::queue_work(adev, crate::workqueue::work::dm_irq_work { irq_source, handler: h });
        if queued {
            return;
        }
    }
}

/// `dm_irq_work_func`: a low handler from its work.
pub fn dm_irq_work_func(adev: &mut amdgpu_device, h: dm_irq_handler) {
    call_handler(adev, h);
}

fn call_handler(adev: &mut amdgpu_device, h: dm_irq_handler) {
    match h {
        dm_irq_handler::dm_vupdate_high_irq { params } => crate::amdgpu_dm::dm_vupdate_high_irq(adev, params),
        dm_irq_handler::dm_dmub_outbox1_low_irq => {
            Line::new().s("dm_dmub_outbox1_low_irq not ported").say();
        }
    }
}

/// `amdgpu_dm_irq_set`.
pub fn amdgpu_dm_irq_set(adev: &amdgpu_device, src: u32, enable: bool) -> bool {
    let Some(dc) = adev.dm.dc.as_deref() else { return false };
    crate::dc::dc_interrupt_set(adev, dc, src, enable)
}

/// `amdgpu_dm_irq_ack`.
pub fn amdgpu_dm_irq_ack(adev: &amdgpu_device, src: u32) {
    if let Some(dc) = adev.dm.dc.as_deref() {
        crate::dc::dc_interrupt_ack(adev, dc, src);
    }
}

/// `amdgpu_dm_irq_handler`: the `process` of every DM interrupt source.
/// Calls all registered high irq work immediately, and schedules work for
/// low irq.
pub fn amdgpu_dm_irq_handler(adev: &mut amdgpu_device, entry: &amdgpu_iv_entry) -> i32 {
    let Some(dc) = adev.dm.dc.as_deref() else { return 0 };
    let src = crate::dc::dc_interrupt_to_irq_source(dc, entry.src_id, entry.src_data[0]);

    amdgpu_dm_irq_ack(adev, src);

    // Call high irq work immediately
    amdgpu_dm_irq_immediate_work(adev, src);
    // Schedule low_irq work
    amdgpu_dm_irq_schedule_work(adev, src);

    0
}

/// `amdgpu_dm_hpd_to_dal_irq_source`.
fn amdgpu_dm_hpd_to_dal_irq_source(r#type: u32) -> u32 {
    match r#type {
        AMDGPU_HPD_1 => DC_IRQ_SOURCE_HPD1,
        AMDGPU_HPD_2 => DC_IRQ_SOURCE_HPD2,
        AMDGPU_HPD_3 => DC_IRQ_SOURCE_HPD3,
        AMDGPU_HPD_4 => DC_IRQ_SOURCE_HPD4,
        AMDGPU_HPD_5 => DC_IRQ_SOURCE_HPD5,
        AMDGPU_HPD_6 => DC_IRQ_SOURCE_HPD6,
        _ => DC_IRQ_SOURCE_INVALID,
    }
}

/// `amdgpu_dm_set_hpd_irq_state`.
pub fn amdgpu_dm_set_hpd_irq_state(adev: &mut amdgpu_device, r#type: u32, state: u32) -> i32 {
    let src = amdgpu_dm_hpd_to_dal_irq_source(r#type);
    let st = state == AMDGPU_IRQ_STATE_ENABLE;

    amdgpu_dm_irq_set(adev, src, st);
    0
}

/// `dm_irq_state`: the CRTC's OTG instance picks the DAL irq source of
/// `dal_irq_type`.
fn dm_irq_state(adev: &mut amdgpu_device, crtc_id: u32, state: u32, dal_irq_type: u32, func: &str) -> i32 {
    let Some(acrtc) = adev.mode_info.crtcs.get(crtc_id as usize).copied().flatten() else {
        Line::new().s(func).s(": crtc is NULL at id :").dec(crtc_id as u64).say();
        return 0;
    };

    if acrtc.otg_inst == -1 {
        return 0;
    }

    let irq_source = dal_irq_type + acrtc.otg_inst as u32;

    let st = state == AMDGPU_IRQ_STATE_ENABLE;

    // dc->caps.ips_support is DCN 3.5's.

    amdgpu_dm_irq_set(adev, irq_source, st);
    0
}

/// `amdgpu_dm_set_pflip_irq_state`.
pub fn amdgpu_dm_set_pflip_irq_state(adev: &mut amdgpu_device, crtc_id: u32, state: u32) -> i32 {
    dm_irq_state(adev, crtc_id, state, IRQ_TYPE_PFLIP, "amdgpu_dm_set_pflip_irq_state")
}

/// `amdgpu_dm_set_crtc_irq_state`.
pub fn amdgpu_dm_set_crtc_irq_state(adev: &mut amdgpu_device, crtc_id: u32, state: u32) -> i32 {
    dm_irq_state(adev, crtc_id, state, IRQ_TYPE_VBLANK, "amdgpu_dm_set_crtc_irq_state")
}

/// `amdgpu_dm_set_vline0_irq_state`.
pub fn amdgpu_dm_set_vline0_irq_state(adev: &mut amdgpu_device, crtc_id: u32, state: u32) -> i32 {
    dm_irq_state(adev, crtc_id, state, IRQ_TYPE_VLINE0, "amdgpu_dm_set_vline0_irq_state")
}

/// `amdgpu_dm_set_dmub_outbox_irq_state`.
pub fn amdgpu_dm_set_dmub_outbox_irq_state(adev: &mut amdgpu_device, _crtc_id: u32, state: u32) -> i32 {
    let irq_source = DC_IRQ_SOURCE_DMCUB_OUTBOX;
    let st = state == AMDGPU_IRQ_STATE_ENABLE;

    amdgpu_dm_irq_set(adev, irq_source, st);
    0
}

/// `amdgpu_dm_set_vupdate_irq_state`.
pub fn amdgpu_dm_set_vupdate_irq_state(adev: &mut amdgpu_device, crtc_id: u32, state: u32) -> i32 {
    dm_irq_state(adev, crtc_id, state, IRQ_TYPE_VUPDATE, "amdgpu_dm_set_vupdate_irq_state")
}

/// `amdgpu_dm_set_dmub_trace_irq_state`.
pub fn amdgpu_dm_set_dmub_trace_irq_state(adev: &mut amdgpu_device, _type: u32, state: u32) -> i32 {
    let irq_source = DC_IRQ_SOURCE_DMCUB_OUTBOX0;
    let st = state == AMDGPU_IRQ_STATE_ENABLE;

    amdgpu_dm_irq_set(adev, irq_source, st);
    0
}

/// `amdgpu_dm_set_irq_funcs`: the DM interrupt sources, each with
/// `amdgpu_dm_irq_handler` as its process.
pub fn amdgpu_dm_set_irq_funcs(adev: &mut amdgpu_device) {
    let num_crtc = adev.mode_info.num_crtc;
    adev.crtc_irq.num_types = num_crtc;
    adev.crtc_irq.funcs = Some("dm_crtc_irq_funcs");

    adev.vline0_irq.num_types = num_crtc;
    adev.vline0_irq.funcs = Some("dm_vline0_irq_funcs");

    adev.dmub_outbox_irq.num_types = 1;
    adev.dmub_outbox_irq.funcs = Some("dm_dmub_outbox_irq_funcs");

    adev.vupdate_irq.num_types = num_crtc;
    adev.vupdate_irq.funcs = Some("dm_vupdate_irq_funcs");

    adev.dmub_trace_irq.num_types = 1;
    adev.dmub_trace_irq.funcs = Some("dm_dmub_trace_irq_funcs");

    adev.pageflip_irq.num_types = num_crtc;
    adev.pageflip_irq.funcs = Some("dm_pageflip_irq_funcs");

    adev.hpd_irq.num_types = adev.mode_info.num_hpd;
    adev.hpd_irq.funcs = Some("dm_hpd_irq_funcs");
}
