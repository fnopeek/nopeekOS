//! display/dc/dc_dmub_srv.{h,c} (Linux 6.18.55): DC's handle on the DMUB
//! service. The service itself is the DM's (`adev.dm.dmub_srv`); Linux
//! points at it from here, the port reaches it through `adev`.
#![allow(dead_code)]

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
    pub reg_helper_offload: dc_reg_helper_state,
    pub idle_exit_counter: i32,
    pub driver_signals: u32,
    pub idle_allowed: bool,
    pub needs_idle_wake: bool,
}

/// `dc_dmub_srv_construct`: `dmub` and `ctx` are what the port threads
/// instead of storing.
fn dc_dmub_srv_construct(_dc_srv: &mut dc_dmub_srv) {}

/// `dc_dmub_srv_create`.
pub fn dc_dmub_srv_create() -> Option<Box<dc_dmub_srv>> {
    let mut dc_srv = Box::new(dc_dmub_srv::default());
    dc_dmub_srv_construct(&mut dc_srv);
    Some(dc_srv)
}
