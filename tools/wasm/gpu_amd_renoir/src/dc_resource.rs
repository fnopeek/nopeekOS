//! display/dc/inc/resource.h (Linux 6.18.55): shared resource types used by
//! every DCN resource pool. Only `resource_caps` for now — the per-ASIC count
//! table a pool points `res_pool->res_cap` at (e.g. `res_cap_rn` for Renoir).
#![allow(dead_code)]

/// `struct resource_caps` (resource.h): how many of each block the ASIC has.
#[derive(Default, Clone, Copy)]
pub struct resource_caps {
    pub num_timing_generator: i32,
    pub num_opp: i32,
    pub num_dpp: i32,
    pub num_video_plane: i32,
    pub num_audio: i32,
    pub num_stream_encoder: i32,
    pub num_pll: i32,
    pub num_dwb: i32,
    pub num_ddc: i32,
    pub num_vmid: i32,
    pub num_dsc: i32,
    /// Total number of DIGs (digital encoders) in DIO.
    pub num_dig_link_enc: u32,
    /// Total number of USB4 DPIA (DisplayPort Input Adapters).
    pub num_usb4_dpia: u32,
    pub num_hpo_dp_stream_encoder: i32,
    pub num_hpo_dp_link_encoder: i32,
    pub num_mpc_3dlut: i32,
}
