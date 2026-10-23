//! display/amdgpu_dm/amdgpu_dm_helpers.c + the AUX/I2C glue of
//! amdgpu_dm_mst_types.c and amdgpu_dm.c (Linux 6.18.55): what DC asks the
//! DM for during detection — DPCD through DRM's AUX helpers, the EDID
//! through DRM's EDID reader — and the way back from DRM into DC's AUX
//! engines. The DM connector is `link->priv`; Linux reaches the link from
//! the connector and the DDC service with `container_of`, the port hands
//! down `dm_ctx` (the device, DC and the link's index).
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_ddc_types::*;
use crate::dc_sink::*;
use crate::dc_types::dc_edid_status;
use crate::drm_connector::*;
use crate::drm_dp_helper::*;
use crate::drm_edid::*;
use crate::i2c::{i2c_adapter, i2c_algo, i2c_bus, i2c_msg};
use crate::link_ddc::link_aux_transfer_raw;
use crate::linux::{Line, EBUSY, EIO, ENODEV, ETIMEDOUT};
use crate::regs::*;
use alloc::boxed::Box;
use alloc::format;

/// What Linux's DM code finds through `container_of` and `ctx`.
pub struct dm_ctx<'a> {
    pub adev: &'a amdgpu_device,
    pub dc: &'a mut dc,
    /// The link the DDC service, the AUX channel and the I2C adapter belong to.
    pub li: usize,
}

/// `struct amdgpu_i2c_adapter` (amdgpu_mode.h); its `ddc_service` is the
/// link's.
pub struct amdgpu_i2c_adapter {
    pub base: i2c_adapter,
    pub oem: bool,
}

/// `struct amdgpu_dm_dp_aux` (amdgpu_dm.h); its `ddc_service` is the link's.
#[derive(Default)]
pub struct amdgpu_dm_dp_aux {
    pub aux: drm_dp_aux,
}

/// `struct amdgpu_dm_connector`, the members detection reaches.
pub struct amdgpu_dm_connector {
    pub base: drm_connector,
    pub i2c: Option<Box<amdgpu_i2c_adapter>>,
    pub dm_dp_aux: amdgpu_dm_dp_aux,
}

impl core::fmt::Debug for amdgpu_dm_connector {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.base.name)
    }
}

/// `to_drm_connector_type` (amdgpu_dm.c).
pub fn to_drm_connector_type(st: u32) -> u32 {
    match st {
        LF_SIGNAL_TYPE_HDMI_TYPE_A => DRM_MODE_CONNECTOR_HDMIA,
        LF_SIGNAL_TYPE_EDP => DRM_MODE_CONNECTOR_eDP,
        LF_SIGNAL_TYPE_LVDS => DRM_MODE_CONNECTOR_LVDS,
        LF_SIGNAL_TYPE_RGB => DRM_MODE_CONNECTOR_VGA,
        LF_SIGNAL_TYPE_DISPLAY_PORT | LF_SIGNAL_TYPE_DISPLAY_PORT_MST => DRM_MODE_CONNECTOR_DisplayPort,
        LF_SIGNAL_TYPE_DVI_DUAL_LINK | LF_SIGNAL_TYPE_DVI_SINGLE_LINK => DRM_MODE_CONNECTOR_DVID,
        LF_SIGNAL_TYPE_VIRTUAL => DRM_MODE_CONNECTOR_VIRTUAL,
        _ => DRM_MODE_CONNECTOR_Unknown,
    }
}

/// `create_i2c` (amdgpu_dm.c): the adapter on DC's I2C engines.
fn create_i2c(link_index: u32, oem: bool) -> Box<amdgpu_i2c_adapter> {
    let name = if oem {
        alloc::string::String::from("AMDGPU DM i2c OEM bus")
    } else {
        format!("AMDGPU DM i2c hw bus {}", link_index)
    };
    Box::new(amdgpu_i2c_adapter {
        base: i2c_adapter { name, algo: Some(i2c_algo::amdgpu_dm_i2c_algo), retries: 0 },
        oem,
    })
}

/// `amdgpu_dm_connector_init`'s part DC's detection sees: the hardware I2C
/// adapter and, on DP and eDP, the AUX channel
/// (`amdgpu_dm_initialize_dp_connector`, which turns the DPCD probe off);
/// then `link->priv`. The DRM object, its properties and the MST manager
/// are DRM's userspace side.
pub fn amdgpu_dm_connector_init(dc: &mut dc, li: usize) {
    let link = &dc.links[li];
    let connector_type = to_drm_connector_type(link.connector_signal);
    let mut aconnector = Box::new(amdgpu_dm_connector {
        base: drm_connector::new(format!("connector-{}", link.link_index), connector_type),
        i2c: Some(create_i2c(link.link_index, false)),
        dm_dp_aux: amdgpu_dm_dp_aux::default(),
    });

    if connector_type == DRM_MODE_CONNECTOR_DisplayPort || connector_type == DRM_MODE_CONNECTOR_eDP {
        let aux = &mut aconnector.dm_dp_aux.aux;
        aux.name = format!("AMDGPU DM aux hw bus {}", link.link_index);
        aux.transfer = Some(dm_dp_aux_transfer);
        drm_dp_aux_init(aux);
        drm_dp_dpcd_set_probe(aux, false);
    }

    dc.links[li].priv_ = Some(aconnector);
}

/// `dm_dp_aux_transfer` (amdgpu_dm_mst_types.c): one DRM AUX message
/// through DC's AUX engine of the link.
fn dm_dp_aux_transfer(_aux: &mut drm_dp_aux, ctx: &mut dm_ctx, msg: &mut drm_dp_aux_msg, buf: &mut [u8]) -> isize {
    let mut operation_result = aux_return_code_type::AUX_RET_SUCCESS;
    let mut copy = [0u8; 16];

    if msg.size > 16 {
        return -(crate::linux::E2BIG);
    }

    let (address, size, request) = (msg.address, msg.size, msg.request);
    let write = request & DP_AUX_I2C_READ == 0;
    let data: &mut [u8] = if write {
        copy[..size].copy_from_slice(&buf[msg.buffer..msg.buffer + size]);
        &mut copy[..size]
    } else {
        &mut buf[msg.buffer..msg.buffer + size]
    };
    let mut payload = aux_payload {
        i2c_over_aux: request & DP_AUX_NATIVE_WRITE == 0,
        write,
        mot: request & DP_AUX_I2C_MOT != 0,
        write_status_update: request & DP_AUX_I2C_WRITE_STATUS_UPDATE != 0,
        address,
        length: size as u32,
        data,
        reply: Some(&mut msg.reply),
        defer_delay: 0,
    };

    let mut result = link_aux_transfer_raw(ctx.adev, ctx.dc, ctx.li, &mut payload, &mut operation_result) as isize;

    // w/a on certain intel platform where hpd is unexpected to pull low during
    // 1st sideband message transaction by return AUX_RET_ERROR_HPD_DISCON
    // aux transaction is succuess in such case, therefore bypass the error
    if ctx.adev.dm.aux_hpd_discon_quirk
        && address == DP_SIDEBAND_MSG_DOWN_REQ_BASE
        && operation_result == aux_return_code_type::AUX_RET_ERROR_HPD_DISCON
    {
        result = size as isize;
        operation_result = aux_return_code_type::AUX_RET_SUCCESS;
    }

    let reply = payload.reply.as_deref().copied().unwrap_or(0);
    // result equals to 0 includes the cases of AUX_DEFER/I2C_DEFER
    if payload.write && result >= 0 {
        if result != 0 {
            // one byte indicating partially written bytes
            result = payload.data[0] as isize;
        } else if reply == 0 {
            // I2C_ACK|AUX_ACK
            result = size as isize;
        }
    }

    if result < 0 {
        use aux_return_code_type::*;
        match operation_result {
            AUX_RET_SUCCESS => {}
            AUX_RET_ERROR_HPD_DISCON | AUX_RET_ERROR_UNKNOWN | AUX_RET_ERROR_INVALID_OPERATION
            | AUX_RET_ERROR_PROTOCOL_ERROR => result = -EIO,
            AUX_RET_ERROR_INVALID_REPLY | AUX_RET_ERROR_ENGINE_ACQUIRE => result = -EBUSY,
            AUX_RET_ERROR_TIMEOUT => result = -ETIMEDOUT,
        }
    }

    result
}

/// `amdgpu_dm_i2c_xfer` (amdgpu_dm.c): the I2C messages as one DC I2C
/// command on the link's engines.
pub fn amdgpu_dm_i2c_xfer(i2c: &mut amdgpu_i2c_adapter, ctx: &mut dm_ctx, msgs: &mut [i2c_msg]) -> i32 {
    let num = msgs.len();
    let has_pin = ctx.dc.links[ctx.li].ddc.as_deref().is_some_and(|d| d.ddc_pin.is_some());

    if !has_pin {
        return -(EIO as i32);
    }

    let mut payloads: alloc::vec::Vec<i2c_payload> = msgs.iter_mut()
        .map(|m| i2c_payload { write: m.flags & crate::i2c::I2C_M_RD == 0, address: m.addr as u8,
            length: m.len as u32, data: &mut *m.buf })
        .collect();
    let mut cmd = i2c_command {
        payloads: &mut payloads,
        number_of_payloads: num as u8,
        engine: i2c_command_engine::I2C_COMMAND_ENGINE_DEFAULT,
        speed: 100,
    };

    let ok = if i2c.oem {
        dc_submit_i2c_oem(ctx.dc, &mut cmd)
    } else {
        dc_submit_i2c(ctx.adev, ctx.dc, ctx.li, &mut cmd)
    };
    if ok { num as i32 } else { -(EIO as i32) }
}

/// `dc_submit_i2c` (dc_link_exports.c) → `dce_i2c_submit_command`: the
/// I2C engines are reached by a sink on an HDMI or DVI connector, not by
/// an eDP panel; not ported yet.
fn dc_submit_i2c(_adev: &amdgpu_device, _dc: &mut dc, _li: usize, _cmd: &mut i2c_command) -> bool {
    Line::new().s("dc_submit_i2c: dce_i2c_submit_command not ported").say();
    false
}

/// `dc_submit_i2c_oem` (dc_link_exports.c): no OEM adapter is created.
fn dc_submit_i2c_oem(_dc: &mut dc, _cmd: &mut i2c_command) -> bool {
    Line::new().s("dc_submit_i2c_oem: not ported").say();
    false
}

/// Takes the DM connector out of `link->priv` for the length of `f`, so the
/// context can hold all of DC.
fn with_connector<R>(adev: &amdgpu_device, dc: &mut dc, li: usize,
                     f: impl FnOnce(&mut amdgpu_dm_connector, &mut dm_ctx) -> R) -> Option<R> {
    let mut aconnector = dc.links[li].priv_.take()?;
    let r = {
        let mut ctx = dm_ctx { adev, dc: &mut *dc, li };
        f(&mut aconnector, &mut ctx)
    };
    dc.links[li].priv_ = Some(aconnector);
    Some(r)
}

/// `dm_helpers_dp_read_dpcd`.
pub fn dm_helpers_dp_read_dpcd(adev: &amdgpu_device, dc: &mut dc, li: usize, address: u32, data: &mut [u8]) -> bool {
    let size = data.len();
    with_connector(adev, dc, li, |a, ctx| drm_dp_dpcd_read(&mut a.dm_dp_aux.aux, ctx, address, data) == size as isize)
        .unwrap_or(false)
}

/// `dm_helpers_dp_write_dpcd`.
pub fn dm_helpers_dp_write_dpcd(adev: &amdgpu_device, dc: &mut dc, li: usize, address: u32, data: &[u8]) -> bool {
    let mut buf = data.to_vec();
    with_connector(adev, dc, li, |a, ctx| drm_dp_dpcd_write(&mut a.dm_dp_aux.aux, ctx, address, &mut buf) > 0)
        .unwrap_or(false)
}

/// `edid_extract_panel_id`.
fn edid_extract_panel_id(edid: &[u8]) -> u32 {
    (edid[8] as u32) << 24 | (edid[9] as u32) << 16 | (edid[10] as u32 | (edid[11] as u32) << 8)
}

/// `apply_edid_quirks`: Linux's panel table.
fn apply_edid_quirks(dc: &mut dc, li: usize, edid: &[u8], edid_caps: &mut dc_edid_caps) {
    let panel_id = edid_extract_panel_id(edid);
    let id = drm_edid_encode_panel_id;

    if panel_id == id(b'G', b'B', b'T', 0x3215) {
        // Workaround for monitors that need a delay after detecting the link
        edid_caps.panel_patch.wait_after_dpcd_poweroff_ms = 10000;
    } else if panel_id == id(b'S', b'A', b'M', 0x0E5E)
        || panel_id == id(b'S', b'A', b'M', 0x7053)
        || panel_id == id(b'S', b'A', b'M', 0x71AC)
    {
        // Workaround for some monitors which does not work well with FAMS
        edid_caps.panel_patch.disable_fams = 1;
    } else if panel_id == id(b'A', b'U', b'O', 0xA7AB)
        || panel_id == id(b'A', b'U', b'O', 0xE69B)
        || panel_id == id(b'B', b'O', b'E', 0x092A)
        || panel_id == id(b'L', b'G', b'D', 0x06D1)
        || panel_id == id(b'M', b'S', b'F', 0x1003)
    {
        // Workaround for some monitors that do not clear DPCD 0x317 if FreeSync is unsupported
        edid_caps.panel_patch.remove_sink_ext_caps = 1;
    } else if panel_id == id(b'S', b'D', b'C', 0x4154) || panel_id == id(b'S', b'D', b'C', 0x4171) {
        edid_caps.panel_patch.disable_colorimetry = 1;
    } else if panel_id == id(b'D', b'E', b'L', 0x4147) {
        // Workaround for monitors that get corrupted by the PHY SSC reduction
        dc.links[li].wa_flags.skip_phy_ssc_reduction = true;
    } else if panel_id == id(b'A', b'P', b'P', 0xAE3A)
        || panel_id == id(b'A', b'P', b'P', 0xAE42)
        || panel_id == id(b'A', b'P', b'P', 0xAE46)
    {
        // Workaround for Apple Studio Display which exposes a 2x1 tiled panel
        // over two SST DP links. Hide the secondary tile from userspace so
        // compositors drive a single 5K stream on the primary link only.
        edid_caps.panel_patch.disable_second_tile = true;
    }
}

/// `dm_helpers_parse_edid_caps`.
pub fn dm_helpers_parse_edid_caps(dc: &mut dc, li: usize, edid: &dc_edid, edid_caps: &mut dc_edid_caps)
    -> dc_edid_status {
    let mut edid_buf = edid.raw_edid;
    let mut result = dc_edid_status::EDID_OK;

    if !drm_edid_is_valid(&mut edid_buf) {
        result = dc_edid_status::EDID_BAD_CHECKSUM;
    }

    edid_caps.manufacturer_id = edid_buf[8] as u16 | (edid_buf[9] as u16) << 8;
    edid_caps.product_id = edid_buf[10] as u16 | (edid_buf[11] as u16) << 8;
    edid_caps.serial_number = u32::from_le_bytes(edid_buf[12..16].try_into().unwrap());
    edid_caps.manufacture_week = edid_buf[16];
    edid_caps.manufacture_year = edid_buf[17];

    drm_edid_get_monitor_name(Some(&edid_buf), &mut edid_caps.display_name);

    let info = &dc.links[li].priv_.as_deref().unwrap().base.display_info;
    edid_caps.edid_hdmi = info.is_hdmi;

    if edid_caps.edid_hdmi {
        edid_caps.qs_bit = info.rgb_quant_range_selectable as u8;
    }

    apply_edid_quirks(dc, li, &edid_buf, edid_caps);

    let mut sads = alloc::vec::Vec::new();
    let sad_count = drm_edid_to_sad(&edid.raw_edid, &mut sads);
    if sad_count <= 0 {
        return result;
    }

    edid_caps.audio_mode_count = (sad_count as u32).min(DC_MAX_AUDIO_DESC_COUNT as u32);
    for i in 0..edid_caps.audio_mode_count as usize {
        let sad = &sads[i];

        edid_caps.audio_modes[i].format_code = sad.format;
        edid_caps.audio_modes[i].channel_count = sad.channels + 1;
        edid_caps.audio_modes[i].sample_rate = sad.freq;
        edid_caps.audio_modes[i].sample_size = sad.byte2;
    }

    let mut sadb = alloc::vec::Vec::new();
    let mut sadb_count = drm_edid_to_speaker_allocation(&edid.raw_edid, &mut sadb);

    if sadb_count < 0 {
        Line::new().s("Couldn't read Speaker Allocation Data Block: ").dec(sadb_count as u64).say();
        sadb_count = 0;
    }

    edid_caps.speaker_flags = if sadb_count != 0 { sadb[0] } else { DEFAULT_SPEAKER_LOCATION };

    result
}

/// `dm_helpers_probe_acpi_edid`: the device has no ACPI companion in the
/// module (`ACPI_COMPANION` is NULL), so there is no `_DDC` EDID.
fn dm_helpers_probe_acpi_edid(_buf: &mut [u8], _block: u32) -> i32 {
    -(ENODEV as i32)
}

/// `dm_helpers_read_acpi_edid`.
fn dm_helpers_read_acpi_edid(aconnector: &mut amdgpu_dm_connector) -> Option<drm_edid> {
    let connector = &mut aconnector.base;

    if crate::tables::amdgpu_dc_debug_mask[0] & LD_DC_DISABLE_ACPI_EDID != 0 {
        return None;
    }

    match connector.connector_type {
        DRM_MODE_CONNECTOR_LVDS | DRM_MODE_CONNECTOR_eDP => {}
        _ => return None,
    }

    if connector.force == drm_connector_force::DRM_FORCE_OFF {
        return None;
    }

    drm_edid_read_custom(connector, &mut dm_helpers_probe_acpi_edid)
}

/// `dm_helpers_read_local_edid`: the EDID over the link's DDC (I2C over
/// AUX in AUX mode), retried on a bad checksum or no answer, then the DP
/// compliance EDID test response.
pub fn dm_helpers_read_local_edid(adev: &amdgpu_device, dc: &mut dc, li: usize, sink: &mut dc_sink) -> dc_edid_status {
    let aux_mode = dc.links[li].aux_mode;
    let mut retry = 25;
    let mut edid_status = dc_edid_status::EDID_NO_RESPONSE;

    // some dongles read edid incorrectly the first time,
    // do check sum and retry to make sure read correct edid.
    loop {
        let step = with_connector(adev, dc, li, |a, ctx| {
            let mut drm_edid = dm_helpers_read_acpi_edid(a);
            if drm_edid.is_some() {
                Line::new().s("Using ACPI provided EDID for ").s(&a.base.name).say();
            } else if aux_mode {
                drm_edid = drm_edid_read_ddc(&mut a.base, &mut i2c_bus::aux(&mut a.dm_dp_aux.aux), ctx);
            } else {
                let i2c = a.i2c.as_deref_mut().unwrap();
                drm_edid = drm_edid_read_ddc(&mut a.base, &mut i2c_bus::dm(i2c), ctx);
            }
            drm_edid_connector_update(&mut a.base, drm_edid.as_ref());

            // DP Compliance Test 4.2.2.6
            if aux_mode && a.base.edid_corrupt {
                Line::new().s("dm_helpers_read_local_edid: drm_dp_send_real_edid_checksum not ported").say();
            }

            if drm_edid.is_none() && a.base.edid_corrupt {
                a.base.edid_corrupt = false;
                return Err(dc_edid_status::EDID_BAD_CHECKSUM);
            }
            Ok(drm_edid)
        });
        let drm_edid = match step {
            None => return edid_status,
            Some(Err(s)) => return s,
            Some(Ok(d)) => d,
        };

        if let Some(drm_edid) = drm_edid {
            // FIXME: Get rid of drm_edid_raw()
            let Some(edid) = drm_edid_raw(Some(&drm_edid)) else { return dc_edid_status::EDID_BAD_INPUT };
            if edid[126] as usize >= DC_MAX_EDID_BUFFER_SIZE / EDID_LENGTH {
                return dc_edid_status::EDID_BAD_INPUT;
            }

            sink.dc_edid.length = (EDID_LENGTH * (edid[126] as usize + 1)) as u32;
            let n = sink.dc_edid.length as usize;
            sink.dc_edid.raw_edid[..n].copy_from_slice(&edid[..n]);

            // We don't need the original edid anymore
            let dc_edid = sink.dc_edid.clone();
            edid_status = dm_helpers_parse_edid_caps(dc, li, &dc_edid, &mut sink.edid_caps);
        }

        retry -= 1;
        if !((edid_status == dc_edid_status::EDID_BAD_CHECKSUM || edid_status == dc_edid_status::EDID_NO_RESPONSE)
            && retry > 0)
        {
            break;
        }
    }

    if edid_status != dc_edid_status::EDID_OK {
        let name = dc.links[li].priv_.as_deref().map_or("", |a| a.base.name.as_str());
        Line::new().s("EDID err: ").dec(edid_status as u64).s(", on connector: ").s(name).say();
    }
    if aux_mode {
        let mut test_request = [0u8; 1];
        let mut test_response = 0u8;

        dm_helpers_dp_read_dpcd(adev, dc, li, DP_TEST_REQUEST, &mut test_request);

        // test_request.bits.EDID_READ
        if test_request[0] & (1 << 2) == 0 {
            return edid_status;
        }

        // test_response.bits.EDID_CHECKSUM_WRITE
        test_response |= 1 << 2;

        let last = sink.dc_edid.raw_edid[sink.dc_edid.length as usize - 1];
        dm_helpers_dp_write_dpcd(adev, dc, li, DP_TEST_EDID_CHECKSUM, &[last]);

        dm_helpers_dp_write_dpcd(adev, dc, li, DP_TEST_RESPONSE, &[test_response]);
    }

    edid_status
}

/// `dm_helpers_init_panel_settings`.
pub fn dm_helpers_init_panel_settings(panel_config: &mut crate::link_factory::panel_config, sink: &dc_sink) {
    // Extra Panel Power Sequence
    panel_config.pps_extra_t3_ms = sink.edid_caps.panel_patch.extra_t3_ms;
    panel_config.pps_extra_t7_ms = sink.edid_caps.panel_patch.extra_t7_ms;
    panel_config.pps_extra_delay_backlight_off = sink.edid_caps.panel_patch.extra_delay_backlight_off;
    panel_config.pps_extra_post_t7_ms = 0;
    panel_config.pps_extra_pre_t11_ms = 0;
    panel_config.pps_extra_t12_ms = sink.edid_caps.panel_patch.extra_t12_ms;
    panel_config.pps_extra_post_oui_ms = 0;
    // Feature DSC
    panel_config.dsc_disable_dsc_edp = false;
    panel_config.dsc_force_dsc_edp_policy = 0;
}

/// `dm_helpers_override_panel_settings`.
pub fn dm_helpers_override_panel_settings(panel_config: &mut crate::link_factory::panel_config) {
    // Feature DSC
    if crate::tables::amdgpu_dc_debug_mask[0] & LD_DC_DISABLE_DSC != 0 {
        panel_config.dsc_disable_dsc_edp = true;
    }
}
