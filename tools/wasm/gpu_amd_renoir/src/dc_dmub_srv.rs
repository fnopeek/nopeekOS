//! display/dc/dc_dmub_srv.{h,c} (Linux 6.18.55): DC's handle on the DMUB
//! service and the command path through it. Linux points at the DM's
//! `adev->dm.dmub_srv` from here; the port moves the service in when the
//! handle is created, so DC code reaches it through `dc.ctx` and the DM
//! through `amdgpu_dm::dm_dmub_srv`.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::{dc, dc_debug_options};
use crate::dmub_dcn20::dmub_fw_boot_status;
use crate::dmub_srv::*;
use crate::linux::Line;
use crate::regs::*;
use alloc::boxed::Box;

/// `struct dc_reg_helper_state` (dc_dmub_srv.h): the register-offload
/// gather state. The port writes every register directly, which is
/// Linux' behaviour while `gather_in_progress` is false.
#[derive(Default, Clone, Copy)]
pub struct dc_reg_helper_state {
    pub gather_in_progress: bool,
}

/// `struct dc_dmub_srv` (dc_dmub_srv.h). `driver_signals` is the
/// `union dmub_shared_state_ips_driver_signals` word.
#[derive(Default)]
pub struct dc_dmub_srv {
    pub dmub: Option<Box<dmub_srv>>,
    pub reg_helper_offload: dc_reg_helper_state,
    pub idle_exit_counter: i32,
    pub driver_signals: u32,
    pub idle_allowed: bool,
    pub needs_idle_wake: bool,
}

/// `dc_dmub_srv_construct`: `ctx` is what the port threads instead.
fn dc_dmub_srv_construct(dc_srv: &mut dc_dmub_srv, dmub: Box<dmub_srv>) {
    dc_srv.dmub = Some(dmub);
}

/// `dc_dmub_srv_create`.
pub fn dc_dmub_srv_create(dmub: Box<dmub_srv>) -> Option<Box<dc_dmub_srv>> {
    let mut dc_srv = Box::new(dc_dmub_srv::default());
    dc_dmub_srv_construct(&mut dc_srv, dmub);
    Some(dc_srv)
}

/// `dc_dmub_srv_wait_for_pending`.
pub fn dc_dmub_srv_wait_for_pending(adev: &amdgpu_device, dc_srv: &mut dc_dmub_srv, debug: &dc_debug_options) -> bool {
    let Some(dmub) = dc_srv.dmub.as_deref() else { return false };
    let mut status;
    loop {
        status = dmub_srv_wait_for_pending(adev, dmub, 100000);
        if !(debug.disable_timeout && status != DMUB_STATUS_OK) {
            break;
        }
    }
    if status != DMUB_STATUS_OK {
        Line::new().s("Error waiting for DMUB idle: status=").dec(status as u64).say();
        dc_dmub_srv_log_diagnostic_data(adev, dc_srv);
    }
    status == DMUB_STATUS_OK
}

/// `dc_dmub_srv_fb_cmd_list_queue_execute`.
fn dc_dmub_srv_fb_cmd_list_queue_execute(adev: &amdgpu_device, dc_srv: &mut dc_dmub_srv, debug: &dc_debug_options,
                                         cmd_list: &[dmub_rb_cmd]) -> bool {
    let Some(dmub) = dc_srv.dmub.as_deref_mut() else { return false };
    let count = cmd_list.len() as u32;
    let mut failed = None;

    for (i, cmd) in cmd_list.iter().enumerate() {
        let i = i as u32;
        // Queue command
        let mut status = if !cmd.multi_cmd_pending() || dmub_rb_num_free(&dmub.inbox1.rb) >= count - i {
            dmub_srv_fb_cmd_queue(adev, dmub, cmd)
        } else {
            DMUB_STATUS_QUEUE_FULL
        };

        if status == DMUB_STATUS_QUEUE_FULL {
            // Execute and wait for queue to become empty again.
            status = dmub_srv_fb_cmd_execute(adev, dmub);
            if status == DMUB_STATUS_POWER_STATE_D3 {
                return false;
            }
            loop {
                status = dmub_srv_wait_for_inbox_free(adev, dmub, 100000, count - i);
                if !(debug.disable_timeout && status != DMUB_STATUS_OK) {
                    break;
                }
            }
            // Requeue the command.
            status = dmub_srv_fb_cmd_queue(adev, dmub, cmd);
        }

        if status != DMUB_STATUS_OK {
            failed = Some(("Error queueing DMUB command: status=", status));
            break;
        }
    }

    if failed.is_none() {
        let status = dmub_srv_fb_cmd_execute(adev, dmub);
        if status != DMUB_STATUS_OK {
            failed = Some(("Error starting DMUB execution: status=", status));
        }
    }

    if let Some((msg, status)) = failed {
        if status != DMUB_STATUS_POWER_STATE_D3 {
            Line::new().s(msg).dec(status as u64).say();
            dc_dmub_srv_log_diagnostic_data(adev, dc_srv);
        }
        return false;
    }
    true
}

/// `dc_dmub_srv_cmd_list_queue_execute`. The DMCUB of DCN 2.1 has the
/// frame-buffer inbox only (`inbox_type` is DMUB_CMD_INTERFACE_DEFAULT).
pub fn dc_dmub_srv_cmd_list_queue_execute(adev: &amdgpu_device, dc_srv: &mut dc_dmub_srv, debug: &dc_debug_options,
                                          cmd_list: &[dmub_rb_cmd]) -> bool {
    if dc_srv.dmub.is_none() {
        return false;
    }
    let mut res = dc_dmub_srv_fb_cmd_list_queue_execute(adev, dc_srv, debug, cmd_list);
    if res {
        let dmub = dc_srv.dmub.as_deref_mut().unwrap();
        res = dmub_srv_update_inbox_status(adev, dmub) == DMUB_STATUS_OK;
    }
    res
}

/// `dc_dmub_srv_wait_for_idle`.
pub fn dc_dmub_srv_wait_for_idle(adev: &amdgpu_device, dc_srv: &mut dc_dmub_srv, debug: &dc_debug_options,
                                 wait_type: u32, cmd_list: Option<&mut dmub_rb_cmd>) -> bool {
    let Some(dmub) = dc_srv.dmub.as_deref_mut() else { return false };

    // Wait for DMUB to process command
    if wait_type != DM_DMUB_WAIT_TYPE_NO_WAIT {
        let mut status;
        loop {
            status = dmub_srv_wait_for_idle(adev, dmub, 100000);
            if !(debug.disable_timeout && status != DMUB_STATUS_OK) {
                break;
            }
        }

        if status != DMUB_STATUS_OK {
            Line::new().s("No reply for DMUB command: status=").dec(status as u64).dbg();
            if !dmub.debug.timeout_info.timeout_occured {
                dmub.debug.timeout_info.timeout_occured = true;
                if let Some(cmd) = cmd_list.as_deref() {
                    dmub.debug.timeout_info.timeout_cmd = *cmd;
                }
                dmub.debug.timeout_info.timestamp = (npk_sys::now_us() as u64).wrapping_mul(1000);
            }
            dc_dmub_srv_log_diagnostic_data(adev, dc_srv);
            return false;
        }

        // Copy data back from ring buffer into command
        if wait_type == DM_DMUB_WAIT_TYPE_WAIT_WITH_REPLY {
            if let Some(cmd) = cmd_list {
                dmub_srv_cmd_get_response(adev, dmub, cmd);
            }
        }
    }
    true
}

/// `dc_dmub_srv_cmd_run`.
pub fn dc_dmub_srv_cmd_run(adev: &amdgpu_device, dc_srv: &mut dc_dmub_srv, debug: &dc_debug_options,
                           cmd: &mut dmub_rb_cmd, wait_type: u32) -> bool {
    dc_dmub_srv_cmd_run_list(adev, dc_srv, debug, core::slice::from_mut(cmd), wait_type)
}

/// `dc_dmub_srv_cmd_run_list`.
pub fn dc_dmub_srv_cmd_run_list(adev: &amdgpu_device, dc_srv: &mut dc_dmub_srv, debug: &dc_debug_options,
                                cmd_list: &mut [dmub_rb_cmd], wait_type: u32) -> bool {
    if !dc_dmub_srv_cmd_list_queue_execute(adev, dc_srv, debug, cmd_list) {
        return false;
    }
    dc_dmub_srv_wait_for_idle(adev, dc_srv, debug, wait_type, cmd_list.first_mut())
}

/// `dc_dmub_srv_optimized_init_done`.
pub fn dc_dmub_srv_optimized_init_done(adev: &amdgpu_device, dc_srv: Option<&dc_dmub_srv>) -> bool {
    let Some(dmub) = dc_srv.and_then(|s| s.dmub.as_deref()) else { return false };
    let mut boot_status = dmub_fw_boot_status::default();
    let status = dmub_srv_get_fw_boot_status(adev, dmub, &mut boot_status);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error querying DMUB boot status: error=").dec(status as u64).say();
        return false;
    }
    boot_status.optimized_init_done()
}

/// `dc_dmub_srv_get_diagnostic_data`.
fn dc_dmub_srv_get_diagnostic_data(adev: &amdgpu_device, dc_srv: &mut dc_dmub_srv) -> bool {
    match dc_srv.dmub.as_deref_mut() {
        Some(dmub) => dmub_srv_get_diagnostic_data(adev, dmub),
        None => false,
    }
}

/// `dc_dmub_srv_log_diagnostic_data`. The PC snapshot and the outbox1
/// pointers are not read on DCN 2.0 and print as zero, as in Linux.
pub fn dc_dmub_srv_log_diagnostic_data(adev: &amdgpu_device, dc_srv: &mut dc_dmub_srv) {
    if dc_srv.dmub.is_none() {
        Line::new().s("dc_dmub_srv_log_diagnostic_data: invalid parameters.").say();
        return;
    }
    Line::new().s("dc_dmub_srv_log_diagnostic_data: DMCUB error - collecting diagnostic data").say();
    if !dc_dmub_srv_get_diagnostic_data(adev, dc_srv) {
        Line::new().s("dc_dmub_srv_log_diagnostic_data: dc_dmub_srv_get_diagnostic_data failed.").say();
        return;
    }
    let d = &dc_srv.dmub.as_deref().unwrap().debug;
    Line::new().s("DMCUB STATE:").dbg();
    Line::new().s("    dmcub_version      : ").hexw(d.dmcub_version, 8).dbg();
    for i in 0..16 {
        Line::new().s("    scratch [").dec(i as u64).s("]       : ").hexw(d.scratch[i], 8).dbg();
    }
    for i in 0..DMUB_PC_SNAPSHOT_COUNT as usize {
        Line::new().s("    pc[").dec(i as u64).s("]             : ").hexw(d.pc[i], 8).dbg();
    }
    let rows: [(&str, u32); 12] = [
        ("    unk_fault_addr     : ", d.undefined_address_fault_addr),
        ("    inst_fault_addr    : ", d.inst_fetch_fault_addr),
        ("    data_fault_addr    : ", d.data_write_fault_addr),
        ("    inbox1_rptr        : ", d.inbox1_rptr),
        ("    inbox1_wptr        : ", d.inbox1_wptr),
        ("    inbox1_size        : ", d.inbox1_size),
        ("    inbox0_rptr        : ", d.inbox0_rptr),
        ("    inbox0_wptr        : ", d.inbox0_wptr),
        ("    inbox0_size        : ", d.inbox0_size),
        ("    outbox1_rptr       : ", d.outbox1_rptr),
        ("    outbox1_wptr       : ", d.outbox1_wptr),
        ("    outbox1_size       : ", d.outbox1_size),
    ];
    for (n, v) in rows {
        Line::new().s(n).hexw(v, 8).dbg();
    }
    let flags: [(&str, bool); 6] = [
        ("    is_enabled         : ", d.is_dmcub_enabled),
        ("    is_soft_reset      : ", d.is_dmcub_soft_reset),
        ("    is_secure_reset    : ", d.is_dmcub_secure_reset),
        ("    is_traceport_en    : ", d.is_traceport_en),
        ("    is_cw0_en          : ", d.is_cw0_enabled),
        ("    is_cw6_en          : ", d.is_cw6_enabled),
    ];
    for (n, v) in flags {
        Line::new().s(n).dec(v as u64).dbg();
    }
}

/// `dc_wake_and_execute_dmub_cmd_list`. Idle is allowed only through the
/// IPS sequence of DCN 3.5 and later, so `idle_allowed` stays false here
/// and the wake/re-allow around the command does not apply.
pub fn dc_wake_and_execute_dmub_cmd_list(adev: &amdgpu_device, dc: &mut dc, cmd: &mut [dmub_rb_cmd],
                                         wait_type: u32) -> bool {
    let debug = dc.debug;
    let Some(dc_srv) = dc.ctx.dmub_srv.as_deref_mut() else { return false };
    if dc_srv.dmub.is_none() {
        return false;
    }
    if cmd.is_empty() {
        return true;
    }
    if cmd.len() > 1 {
        crate::amdgpu_dm::dm_execute_dmub_cmd_list(adev, dc_srv, &debug, cmd, wait_type)
    } else {
        crate::amdgpu_dm::dm_execute_dmub_cmd(adev, dc_srv, &debug, &mut cmd[0], wait_type)
    }
}

/// `dc_wake_and_execute_dmub_cmd`.
pub fn dc_wake_and_execute_dmub_cmd(adev: &amdgpu_device, dc: &mut dc, cmd: &mut dmub_rb_cmd, wait_type: u32) -> bool {
    dc_wake_and_execute_dmub_cmd_list(adev, dc, core::slice::from_mut(cmd), wait_type)
}

/// `dc_dmub_srv_notify_fw_dc_power_state`.
pub fn dc_dmub_srv_notify_fw_dc_power_state(adev: &amdgpu_device, dc: &mut dc, power_state: u32) {
    if dc.ctx.dmub_srv.is_none() {
        return;
    }

    let mut cmd = dmub_rb_cmd::default();
    cmd.set_header(DMUB_CMD__IDLE_OPT, DMUB_CMD__IDLE_OPT_SET_DC_POWER_STATE,
        SIZEOF_dmub_rb_cmd_idle_opt_set_dc_power_state - SIZEOF_dmub_cmd_header);

    cmd.0[IDLE_OPT_DC_POWER_STATE_DATA as usize] = if power_state == DC_ACPI_CM_POWER_STATE_D0 {
        DMUB_IDLE_OPT_DC_POWER_STATE_D0
    } else if power_state == DC_ACPI_CM_POWER_STATE_D3 {
        DMUB_IDLE_OPT_DC_POWER_STATE_D3
    } else {
        DMUB_IDLE_OPT_DC_POWER_STATE_UNKNOWN
    } as u8;

    dc_wake_and_execute_dmub_cmd(adev, dc, &mut cmd, DM_DMUB_WAIT_TYPE_WAIT);
}
