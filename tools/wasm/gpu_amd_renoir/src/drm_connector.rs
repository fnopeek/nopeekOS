//! include/drm/drm_connector.h (Linux 6.18.55): the members of
//! `struct drm_connector` and `struct drm_display_info` that EDID reading and
//! parsing write. DRM's userspace interface (properties, modes list) is not
//! a part: the EDID blob a property would carry is kept as `edid_blob`.
#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;

/// `enum drm_connector_force`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum drm_connector_force {
    #[default]
    DRM_FORCE_UNSPECIFIED,
    DRM_FORCE_OFF,
    DRM_FORCE_ON,
    DRM_FORCE_ON_DIGITAL,
}

/// `struct drm_monitor_range_info`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct drm_monitor_range_info {
    pub min_vfreq: u16,
    pub max_vfreq: u16,
}

/// `struct drm_luminance_range_info`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct drm_luminance_range_info {
    pub min_luminance: u32,
    pub max_luminance: u32,
}

/// `struct drm_hdmi_info`: filled by the HDMI and HDMI Forum parsers of a
/// CTA extension, which grow it when they land; reset with the display info.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct drm_hdmi_info {}

/// `struct hdr_sink_metadata`: filled by the HDR static metadata parser
/// (as `drm_hdmi_info`); reset with the display info.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct hdr_sink_metadata {}

/// `struct drm_display_info`.
#[derive(Clone, Debug, Default)]
pub struct drm_display_info {
    pub width_mm: u32,
    pub height_mm: u32,
    pub bpc: u32,
    pub color_formats: u32,
    pub bus_formats: Vec<u32>,
    pub bus_flags: u32,
    pub max_tmds_clock: i32,
    pub dvi_dual: bool,
    pub is_hdmi: bool,
    pub has_audio: bool,
    pub has_hdmi_infoframe: bool,
    pub rgb_quant_range_selectable: bool,
    pub edid_hdmi_rgb444_dc_modes: u8,
    pub edid_hdmi_ycbcr444_dc_modes: u8,
    pub cea_rev: u8,
    pub hdmi: drm_hdmi_info,
    pub hdr_sink_metadata: hdr_sink_metadata,
    pub non_desktop: bool,
    pub monitor_range: drm_monitor_range_info,
    pub luminance_range: drm_luminance_range_info,
    pub mso_stream_count: u8,
    pub mso_pixel_overlap: u8,
    pub max_dsc_bpp: u32,
    pub vics: Vec<u8>,
    pub quirks: u32,
    pub source_physical_address: u16,
}

/// `MAX_ELD_BYTES` (drm_connector.h).
pub const MAX_ELD_BYTES: usize = 128;

/// `struct drm_connector`, the EDID members.
pub struct drm_connector {
    pub name: String,
    /// `DRM_MODE_CONNECTOR_*`.
    pub connector_type: u32,
    pub force: drm_connector_force,
    pub display_info: drm_display_info,
    pub edid_corrupt: bool,
    pub real_edid_checksum: u8,
    pub null_edid_counter: i32,
    pub bad_edid_counter: u32,
    pub eld: [u8; MAX_ELD_BYTES],
    pub latency_present: [bool; 2],
    pub video_latency: [i32; 2],
    pub audio_latency: [i32; 2],
    pub has_tile: bool,
    /// `tile_group != NULL`: the tile group's topology id.
    pub tile_group: Option<[u8; 8]>,
    pub tile_is_single_monitor: bool,
    pub num_h_tile: u8,
    pub num_v_tile: u8,
    pub tile_h_loc: u8,
    pub tile_v_loc: u8,
    pub tile_h_size: u16,
    pub tile_v_size: u16,
    /// `edid_blob_ptr`'s data: the EDID the connector last saw.
    pub edid_blob: Option<Vec<u8>>,
    pub epoch_counter: u64,
}

impl drm_connector {
    pub fn new(name: String, connector_type: u32) -> Self {
        drm_connector {
            name,
            connector_type,
            force: drm_connector_force::DRM_FORCE_UNSPECIFIED,
            display_info: drm_display_info::default(),
            edid_corrupt: false,
            real_edid_checksum: 0,
            null_edid_counter: 0,
            bad_edid_counter: 0,
            eld: [0; MAX_ELD_BYTES],
            latency_present: [false; 2],
            video_latency: [0; 2],
            audio_latency: [0; 2],
            has_tile: false,
            tile_group: None,
            tile_is_single_monitor: false,
            num_h_tile: 0,
            num_v_tile: 0,
            tile_h_loc: 0,
            tile_v_loc: 0,
            tile_h_size: 0,
            tile_v_size: 0,
            edid_blob: None,
            epoch_counter: 0,
        }
    }
}
