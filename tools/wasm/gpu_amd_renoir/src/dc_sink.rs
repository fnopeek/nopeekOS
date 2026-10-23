//! display/dc/core/dc_sink.c + the sink types of dc.h / dc_types.h (Linux
//! 6.18.55): what detection learns about the display on a link. A sink has
//! one owner here (the link's `local_sink`), so the reference count of
//! Linux becomes ownership; `link` and `ctx` are implied by that owner.
#![allow(dead_code)]

/// `DC_MAX_EDID_BUFFER_SIZE` / `DC_EDID_BLOCK_SIZE` (dc_ddc_types.h).
pub const DC_MAX_EDID_BUFFER_SIZE: usize = 2048;
pub const DC_EDID_BLOCK_SIZE: usize = 128;
/// `DC_MAX_AUDIO_DESC_COUNT` / `AUDIO_INFO_DISPLAY_NAME_SIZE_IN_CHARS`.
pub const DC_MAX_AUDIO_DESC_COUNT: usize = 16;
pub const AUDIO_INFO_DISPLAY_NAME_SIZE_IN_CHARS: usize = 20;
/// `DEFAULT_SPEAKER_LOCATION` (dc_types.h... dm_helpers).
pub const DEFAULT_SPEAKER_LOCATION: u8 = 5;

/// `struct dc_edid`.
#[derive(Clone, Debug)]
pub struct dc_edid {
    pub length: u32,
    pub raw_edid: [u8; DC_MAX_EDID_BUFFER_SIZE],
}

impl Default for dc_edid {
    fn default() -> Self {
        dc_edid { length: 0, raw_edid: [0; DC_MAX_EDID_BUFFER_SIZE] }
    }
}

/// `struct dc_cea_audio_mode` (the union byte is `sample_size`).
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_cea_audio_mode {
    pub format_code: u8,
    pub channel_count: u8,
    pub sample_rate: u8,
    pub sample_size: u8,
}

/// `struct dc_panel_patch`.
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_panel_patch {
    pub dppowerup_delay: u32,
    pub extra_t12_ms: u32,
    pub extra_delay_backlight_off: u32,
    pub extra_t7_ms: u32,
    pub skip_scdc_overwrite: u32,
    pub delay_ignore_msa: u32,
    pub disable_fec: u32,
    pub extra_t3_ms: u32,
    pub max_dsc_target_bpp_limit: u32,
    pub embedded_tiled_slave: u32,
    pub disable_fams: u32,
    pub skip_avmute: u32,
    pub skip_audio_sab_check: u32,
    pub mst_start_top_delay: u32,
    pub remove_sink_ext_caps: u32,
    pub disable_second_tile: bool,
    pub disable_colorimetry: u32,
    pub blankstream_before_otg_off: u8,
    pub oled_optimize_display_on: bool,
    pub force_mst_blocked_discovery: u32,
    pub wait_after_dpcd_poweroff_ms: u32,
}

/// `struct dc_edid_caps`.
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_edid_caps {
    // sink identification
    pub manufacturer_id: u16,
    pub product_id: u16,
    pub serial_number: u32,
    pub manufacture_week: u8,
    pub manufacture_year: u8,
    pub display_name: [u8; AUDIO_INFO_DISPLAY_NAME_SIZE_IN_CHARS],
    // audio caps
    pub speaker_flags: u8,
    pub audio_mode_count: u32,
    pub audio_modes: [dc_cea_audio_mode; DC_MAX_AUDIO_DESC_COUNT],
    pub audio_latency: u32,
    pub video_latency: u32,
    pub qs_bit: u8,
    pub qy_bit: u8,
    pub max_tmds_clk_mhz: u32,
    // HDMI 2.0 caps
    pub lte_340mcsc_scramble: bool,
    pub edid_hdmi: bool,
    pub hdr_supported: bool,
    pub rr_capable: bool,
    pub panel_patch: dc_panel_patch,
}

/// `struct dc_sink`, the members detection writes.
#[derive(Clone, Default, Debug)]
pub struct dc_sink {
    /// `enum signal_type`.
    pub sink_signal: u32,
    pub dc_edid: dc_edid,
    pub edid_caps: dc_edid_caps,
    pub dongle_max_pix_clk: u32,
    pub converter_disable_audio: bool,
    pub sink_id: u32,
}

/// `struct dc_sink_init_data`; `link` is the caller's.
pub struct dc_sink_init_data {
    pub sink_signal: u32,
    pub dongle_max_pix_clk: u32,
    pub converter_disable_audio: bool,
}

/// `dc_sink_construct` + `dc_sink_create`: `dc_sink_id_count` is the
/// link's context's counter, advanced so no two sinks share an id.
pub fn dc_sink_create(init_params: &dc_sink_init_data, dc_sink_id_count: &mut u32) -> alloc::boxed::Box<dc_sink> {
    let mut sink = alloc::boxed::Box::new(dc_sink::default());

    sink.sink_signal = init_params.sink_signal;
    sink.dongle_max_pix_clk = init_params.dongle_max_pix_clk;
    sink.converter_disable_audio = init_params.converter_disable_audio;
    sink.sink_id = *dc_sink_id_count;
    // increment dc_sink_id_count because we don't want two sinks with same ID
    // unless they are actually the same
    *dc_sink_id_count += 1;

    sink
}
