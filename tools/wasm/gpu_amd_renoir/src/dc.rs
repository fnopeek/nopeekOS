//! display/dc/dc.h + dc/inc/core_types.h (Linux 6.18.55): the Display Core
//! spine — dc_context, dc_init_data and `struct dc`. Ported grow-as-reached:
//! fields appear as the construct/create path reaches them, so the struct
//! tracks the port rather than carrying forward-declared blocks that are not
//! built yet (the oracle compares the values set, not struct completeness).
//! `driver_context` (Linux' amdgpu_device pointer) is not stored here — the
//! module threads `&amdgpu_device` to the register helpers, as the rest of it
//! does.
#![allow(dead_code)]

use crate::dc_resource::resource_pool;
use crate::dc_types::{dc_bios, dc_caps, dce_environment, dce_version, dc_versions, hw_asic_id};
use alloc::boxed::Box;

/// `struct dc_context` (dc_types.h): the per-device DC context. Sub-block
/// handles (dmub_srv, perf_trace, …) join as their stages land. Linux' `dc_bios`
/// is a pointer; the port holds it by value until bios_parser2 lands (see
/// `dc_bios`).
#[derive(Default)]
pub struct dc_context {
    pub dce_environment: dce_environment,
    pub asic_id: hw_asic_id,
    pub dce_version: dce_version,
    pub dc_bios: dc_bios,
    pub dc_sink_id_count: u32,
    pub dc_stream_id_count: u32,
    pub dc_edp_id_count: u32,
    pub fbc_gpu_addr: u64,
}

impl Default for dce_environment {
    fn default() -> Self { dce_environment::DCE_ENV_PRODUCTION_DRV }
}
impl Default for dce_version {
    fn default() -> Self { dce_version::DCE_VERSION_UNKNOWN }
}

/// `struct dc_init_data` (dc.h): what `dm_hw_init` hands `dc_create`. The
/// complex override/config members join when their consumers are ported.
#[derive(Default)]
pub struct dc_init_data {
    pub asic_id: hw_asic_id,
    pub num_virtual_links: i32,
    pub dce_environment: dce_environment,
    pub log_mask: u64,
    pub force_smu_not_present: bool,
}

/// Which `dc_cap_funcs` entry is bound (dcn21 sets only
/// `get_dcc_compression_cap`; its body and `get_subvp_en` are later stages).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum cap_func_id {
    dcn20_get_dcc_compression_cap,
}

/// `struct dc_cap_funcs` (dc.h): the capability-query vtable.
#[derive(Default)]
pub struct dc_cap_funcs {
    pub get_dcc_compression_cap: Option<cap_func_id>,
}

/// `struct dc` (dc.h): the Display Core instance.
#[derive(Default)]
pub struct dc {
    pub ctx: dc_context,
    pub caps: dc_caps,
    pub res_pool: Option<Box<resource_pool>>,
    pub link_count: u32,
    pub versions: dc_versions,
    /// `hwseq`: the hw-sequencer register bank (`resource_construct` creates it).
    pub hwseq: Option<Box<crate::dce_hwseq::dce_hwseq>>,
    /// `dml`: the DML instance (`dml_init_instance`). Boxed because the SoC
    /// bounding box is not Default (its `clock_limits` array is > 32 entries).
    pub dml: Option<Box<crate::dml_structs::display_mode_lib>>,
    /// `cap_funcs`: the capability-query vtable (set at the end of the construct).
    pub cap_funcs: dc_cap_funcs,
}
