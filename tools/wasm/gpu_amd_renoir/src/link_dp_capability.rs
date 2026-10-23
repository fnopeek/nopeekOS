//! display/dc/link/protocols/link_dp_capability.c + the DPCD parts of
//! link_dp_dpia.c and link_edp_panel_control.c (Linux 6.18.55): what DC
//! reads off a DisplayPort or eDP receiver when it detects it. LTTPR
//! transparent mode, the DP-to-HDMI PCON (FRL) extension, the receiver
//! power-up of a DPCD 0.0 sink, the USB-C cable id over DMUB and the OLED
//! AUX backlight are named where Linux calls them and not yet ported: an
//! eDP panel on DCN 2.1 reaches none of them.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_ddc_types::*;
use crate::dc_dp_types::*;
use crate::dc_types::{dc_status, dce_version};
use crate::link_ddc::{set_dongle_type, try_to_configure_aux_timeout};
use crate::link_dpcd::{core_link_read_dpcd, core_link_write_dpcd};
use crate::linux::{msleep, Line};
use crate::regs::*;

/// `LINK_AUX_DEFAULT_LTTPR_TIMEOUT_PERIOD` / `LINK_AUX_DEFAULT_TIMEOUT_PERIOD`
/// (link_dp_capability.h), in us.
const LINK_AUX_DEFAULT_LTTPR_TIMEOUT_PERIOD: u32 = 3200;
const LINK_AUX_DEFAULT_TIMEOUT_PERIOD: u32 = 552;

/// `DPCD_REV_*` (dc_dp_types.h).
const DPCD_REV_11: u8 = 0x11;
const DPCD_REV_12: u8 = 0x12;
const DPCD_REV_13: u8 = 0x13;
const DPCD_REV_14: u8 = 0x14;

fn not_ported(name: &str) {
    Line::new().s("link_dp_capability: ").s(name).s(" not ported").say();
}

/// `dp_parse_lttpr_repeater_count`.
pub fn dp_parse_lttpr_repeater_count(lttpr_repeater_count: u8) -> u8 {
    match lttpr_repeater_count {
        0x80 => 1, // 1 LTTPR(s)
        0x40 => 2, // 2 LTTPR(s)
        0x20 => 3, // 3 LTTPR(s)
        0x10 => 4, // 4 LTTPR(s)
        0x08 => 5, // 5 LTTPR(s)
        0x04 => 6, // 6 LTTPR(s)
        0x02 => 7, // 7 LTTPR(s)
        0x01 => 8, // 8 LTTPR(s)
        _ => 0,    // invalid value
    }
}

/// `linkRateInKHzToLinkRateMultiplier`.
#[allow(non_snake_case)]
fn linkRateInKHzToLinkRateMultiplier(link_rate_in_khz: u32) -> u32 {
    // LinkRate is normally stored as a multiplier of 0.27 Gbps per lane. Do the translation.
    match link_rate_in_khz {
        1620000 => LINK_RATE_LOW,    // Rate_1 (RBR)	- 1.62 Gbps/Lane
        2160000 => LINK_RATE_RATE_2, // Rate_2	- 2.16 Gbps/Lane
        2430000 => LINK_RATE_RATE_3, // Rate_3	- 2.43 Gbps/Lane
        2700000 => LINK_RATE_HIGH,   // Rate_4 (HBR)	- 2.70 Gbps/Lane
        3240000 => LINK_RATE_RBR2,   // Rate_5 (RBR2)- 3.24 Gbps/Lane
        4320000 => LINK_RATE_RATE_6, // Rate_6	- 4.32 Gbps/Lane
        5400000 => LINK_RATE_HIGH2,  // Rate_7 (HBR2)- 5.40 Gbps/Lane
        6750000 => LINK_RATE_RATE_8, // Rate_8	- 6.75 Gbps/Lane
        8100000 => LINK_RATE_HIGH3,  // Rate_9 (HBR3)- 8.10 Gbps/Lane
        _ => LINK_RATE_UNKNOWN,
    }
}

/// `dp_wa_power_up_0010FA`.
fn dp_wa_power_up_0010FA(adev: &amdgpu_device, dc: &mut dc, li: usize, dpcd_data: &mut [u8]) {
    if dc.links[li].dpcd_caps.dpcd_rev == 0 {
        not_ported("dpcd_write_rx_power_ctrl (dp_wa_power_up_0010FA)");
        let mut retry = 0;
        loop {
            core_link_read_dpcd(adev, dc, li, DP_DPCD_REV, dpcd_data);
            dc.links[li].dpcd_caps.dpcd_rev = dpcd_data[0];
            let r = retry;
            retry += 1;
            if !(r < 4 && dc.links[li].dpcd_caps.dpcd_rev == 0) {
                break;
            }
        }
    }

    let link = &mut dc.links[li];
    if link.dpcd_caps.dongle_type == DISPLAY_DONGLE_DP_VGA_CONVERTER {
        link.wa_flags.dp_keep_receiver_powered = matches!(
            link.dpcd_caps.branch_dev_id,
            // 0010FA active dongles (DP-VGA, DP-DLDVI converters) power down
            // all internal circuits including AUX communication preventing
            // reading DPCD table and EDID (spec violation).
            // Encoder will skip DP RX power down on disable_output to
            // keep receiver powered all the time.
            DP_BRANCH_DEVICE_ID_0010FA | DP_BRANCH_DEVICE_ID_0080E1 | DP_BRANCH_DEVICE_ID_00E04C
        );
    } else {
        link.wa_flags.dp_keep_receiver_powered = false;
    }
}

/// `dp_is_lttpr_present`.
pub fn dp_is_lttpr_present(dc: &dc, li: usize) -> bool {
    let caps = &dc.links[li].dpcd_caps.lttpr_caps;
    // Some sink devices report invalid LTTPR revision, so don't validate against that cap
    let lttpr_count = dp_parse_lttpr_repeater_count(caps.phy_repeater_cnt);
    let is_lttpr_present = lttpr_count > 0 && caps.max_lane_count > 0 && caps.max_lane_count <= 4;

    if lttpr_count > 0 && !is_lttpr_present {
        Line::new().s("LTTPR count is nonzero but invalid lane count reported. Assuming no LTTPR present.").say();
    }

    is_lttpr_present
}

/// `get_link_rate_from_max_link_bw`.
fn get_link_rate_from_max_link_bw(max_link_bw: u8) -> u32 {
    let max_link_bw = max_link_bw as u32;

    if max_link_bw >= LINK_RATE_HIGH3 {
        LINK_RATE_HIGH3
    } else if max_link_bw < LINK_RATE_HIGH3 && max_link_bw >= LINK_RATE_HIGH2 {
        LINK_RATE_HIGH2
    } else if max_link_bw < LINK_RATE_HIGH2 && max_link_bw >= LINK_RATE_HIGH {
        LINK_RATE_HIGH
    } else if max_link_bw < LINK_RATE_HIGH && max_link_bw >= LINK_RATE_LOW {
        LINK_RATE_LOW
    } else {
        LINK_RATE_UNKNOWN
    }
}

/// `get_cable_max_link_rate`.
fn get_cable_max_link_rate(dc: &dc, li: usize) -> u32 {
    let c = dc.links[li].dpcd_caps.cable_id;
    let mut cable_max_link_rate = LINK_RATE_UNKNOWN;

    if cable_id_UHBR10_20_CAPABILITY(c) as u32 & DP_UHBR20 != 0 {
        cable_max_link_rate = LINK_RATE_UHBR20;
    } else if cable_id_UHBR13_5_CAPABILITY(c) != 0 {
        cable_max_link_rate = LINK_RATE_UHBR13_5;
    } else if cable_id_UHBR10_20_CAPABILITY(c) as u32 & DP_UHBR10 != 0 {
        // allow DP40 cables to do UHBR13.5 for passive or unknown cable type
        cable_max_link_rate = if cable_id_CABLE_TYPE(c) < 2 { LINK_RATE_UHBR13_5 } else { LINK_RATE_UHBR10 };
    }

    cable_max_link_rate
}

/// `link_dp_get_encoding_format`.
pub fn link_dp_get_encoding_format(link_settings: &dc_link_settings) -> dp_link_encoding {
    if link_settings.link_rate >= LINK_RATE_LOW && link_settings.link_rate <= LINK_RATE_HIGH3 {
        dp_link_encoding::DP_8b_10b_ENCODING
    } else if link_settings.link_rate >= LINK_RATE_UHBR10 && link_settings.link_rate <= LINK_RATE_UHBR20 {
        dp_link_encoding::DP_128b_132b_ENCODING
    } else {
        dp_link_encoding::DP_UNKNOWN_ENCODING
    }
}

/// `read_dp_device_vendor_id`.
fn read_dp_device_vendor_id(adev: &amdgpu_device, dc: &mut dc, li: usize) {
    // struct dp_device_vendor_id: ieee_oui[3], ieee_device_id[6]
    let mut dp_id = [0u8; 9];

    // read IEEE branch device id
    core_link_read_dpcd(adev, dc, li, DP_BRANCH_OUI, &mut dp_id);

    let caps = &mut dc.links[li].dpcd_caps;
    caps.branch_dev_id = ((dp_id[0] as u32) << 16) + ((dp_id[1] as u32) << 8) + dp_id[2] as u32;

    caps.branch_dev_name.copy_from_slice(&dp_id[3..9]);
}

/// `wake_up_aux_channel`.
fn wake_up_aux_channel(adev: &amdgpu_device, dc: &mut dc, li: usize) -> dc_status {
    let mut status = dc_status::DC_ERROR_UNEXPECTED;
    let mut aux_channel_retry_cnt = 0;
    let mut dpcd_power_state = [0u8; 1];

    while status != dc_status::DC_OK && aux_channel_retry_cnt < 10 {
        status = core_link_read_dpcd(adev, dc, li, DP_SET_POWER, &mut dpcd_power_state);

        // Delay 1 ms if AUX CH is in power down state. Based on spec
        // section 2.3.1.2, if AUX CH may be powered down due to
        // write to DPCD 600h = 2. Sink AUX CH is monitoring differential
        // signal and may need up to 1 ms before being able to reply.
        if status != dc_status::DC_OK || dpcd_power_state[0] as u32 == DP_SET_POWER_D3 {
            // fsleep(1000)
            crate::linux::usleep_range(1000, 1000);
            aux_channel_retry_cnt += 1;
        }
    }

    if status != dc_status::DC_OK {
        core_link_write_dpcd(adev, dc, li, DP_SET_POWER, &[DP_SET_POWER_D0 as u8]);

        core_link_write_dpcd(adev, dc, li, DP_SET_POWER, &[DP_SET_POWER_D3 as u8]);
        return dc_status::DC_ERROR_UNEXPECTED;
    }

    dc_status::DC_OK
}

/// `get_active_converter_info`.
fn get_active_converter_info(adev: &amdgpu_device, dc: &mut dc, li: usize, data: u8) {
    let ds_port = data;
    dc.links[li].dpcd_caps.dongle_caps = dc_dongle_caps::default();

    // decode converter info
    if dp_downstream_port_present_PORT_PRESENT(ds_port) == 0 {
        let link = &mut dc.links[li];
        link.dpcd_caps.dongle_type = DISPLAY_DONGLE_NONE;
        set_dongle_type(link.ddc.as_deref_mut().unwrap(), link.dpcd_caps.dongle_type);
        link.dpcd_caps.is_branch_dev = false;
        return;
    }

    {
        let caps = &mut dc.links[li].dpcd_caps;
        // DPCD 0x5 bit 0 = 1, it indicate it's branch device
        caps.is_branch_dev = dp_downstream_port_present_PORT_PRESENT(ds_port) != 0;

        caps.dongle_type = match dp_downstream_port_present_PORT_TYPE(ds_port) as u32 {
            LD_DOWNSTREAM_VGA => DISPLAY_DONGLE_DP_VGA_CONVERTER,
            // At this point we don't know is it DVI or HDMI or DP++,
            // assume DVI.
            LD_DOWNSTREAM_DVI_HDMI_DP_PLUS_PLUS => DISPLAY_DONGLE_DP_DVI_CONVERTER,
            _ => DISPLAY_DONGLE_NONE,
        };
    }

    if dc.links[li].dpcd_caps.dpcd_rev >= DPCD_REV_11 {
        // CTS 4.2.2.7 expects source to read Detailed Capabilities Info : 00080h-0008F.
        let mut det_caps = [0u8; 16];
        if core_link_read_dpcd(adev, dc, li, DP_DOWNSTREAM_PORT_0, &mut det_caps) == dc_status::DC_OK {
            let caps = &mut dc.links[li].dpcd_caps;
            match dwnstream_port_caps_byte0_DWN_STRM_PORTX_TYPE(det_caps[0]) as u32 {
                // Handle DP case as DONGLE_NONE
                LD_DOWN_STREAM_DETAILED_DP => caps.dongle_type = DISPLAY_DONGLE_NONE,
                LD_DOWN_STREAM_DETAILED_VGA => caps.dongle_type = DISPLAY_DONGLE_DP_VGA_CONVERTER,
                LD_DOWN_STREAM_DETAILED_DVI => caps.dongle_type = DISPLAY_DONGLE_DP_DVI_CONVERTER,
                LD_DOWN_STREAM_DETAILED_HDMI | LD_DOWN_STREAM_DETAILED_DP_PLUS_PLUS => {
                    // Handle DP++ active converter case, process DP++ case as HDMI case according DP1.4 spec
                    caps.dongle_type = DISPLAY_DONGLE_DP_HDMI_CONVERTER;

                    caps.dongle_caps.dongle_type = caps.dongle_type;
                    if dp_downstream_port_present_DETAILED_CAPS(ds_port) != 0 {
                        not_ported("the DP-HDMI converter's detailed caps");
                    }
                }
                _ => {}
            }
        }
    }

    {
        let link = &mut dc.links[li];
        set_dongle_type(link.ddc.as_deref_mut().unwrap(), link.dpcd_caps.dongle_type);
    }

    {
        // struct dp_sink_hw_fw_revision: ieee_hw_rev, ieee_fw_rev[2]
        let mut dp_hw_fw_revision = [0u8; 3];

        core_link_read_dpcd(adev, dc, li, DP_BRANCH_REVISION_START, &mut dp_hw_fw_revision);

        let caps = &mut dc.links[li].dpcd_caps;
        caps.branch_hw_revision = dp_hw_fw_revision[0];

        caps.branch_fw_revision.copy_from_slice(&dp_hw_fw_revision[1..3]);
    }

    let mut vs = [0u8; 4];
    core_link_read_dpcd(adev, dc, li, DP_BRANCH_VENDOR_SPECIFIC_START, &mut vs);
    dc.links[li].dpcd_caps.branch_vendor_specific_data = vs;

    if dc.links[li].dpcd_caps.dpcd_rev >= DPCD_REV_14 && dc.links[li].dpcd_caps.dongle_type != DISPLAY_DONGLE_NONE {
        not_ported("the DFP capability extension");
    }
}

/// `dpcd_set_source_specific_data`.
pub fn dpcd_set_source_specific_data(adev: &amdgpu_device, dc: &mut dc, li: usize) {
    if !dc.vendor_signature.is_valid {
        if dc.links[li].is_dds {
            let mut dpcd_dp_edp_backlight_mode = [0u8; 1];

            // Write 0 to bits 0:1 for dp_edp_backlight_mode_set register
            // if platform is DDS
            core_link_read_dpcd(adev, dc, li, DP_EDP_BACKLIGHT_MODE_SET_REGISTER, &mut dpcd_dp_edp_backlight_mode);
            dpcd_dp_edp_backlight_mode[0] &= !0x3;

            core_link_write_dpcd(adev, dc, li, DP_EDP_BACKLIGHT_MODE_SET_REGISTER, &dpcd_dp_edp_backlight_mode);
        }

        // struct dpcd_amd_device_id: device id (2), zero[4], dce_version,
        // dal_version (2)
        let chip_id = dc.ctx.asic_id.chip_id;
        let amd_device_id = [chip_id as u8, (chip_id >> 8) as u8, 0, 0, 0, 0, dc.ctx.dce_version as i32 as u8, 0x0, 0x0];
        // struct dpcd_amd_signature: AMD_IEEE_TxSignature_byte1..3
        let mut amd_signature = [0u8; 3];

        core_link_read_dpcd(adev, dc, li, DP_SOURCE_OUI, &mut amd_signature);

        if !(amd_signature[0] == 0x0 && amd_signature[1] == 0x0 && amd_signature[2] == 0x1A) {
            amd_signature = [0x0, 0x0, 0x1A];

            core_link_write_dpcd(adev, dc, li, DP_SOURCE_OUI, &amd_signature);
        }

        core_link_write_dpcd(adev, dc, li, DP_SOURCE_OUI + 0x03, &amd_device_id);

        if dc.ctx.dce_version as i32 >= dce_version::DCN_VERSION_2_0 as i32
            && dc.caps.min_horizontal_blanking_period != 0
        {
            let hblank_size = [dc.caps.min_horizontal_blanking_period as u8];

            core_link_write_dpcd(adev, dc, li, DP_SOURCE_MINIMUM_HBLANK_SUPPORTED, &hblank_size);
        }
    } else {
        let raw = dc.vendor_signature.data;
        core_link_write_dpcd(adev, dc, li, DP_SOURCE_OUI, &raw);
    }
}

/// `read_is_mst_supported`.
pub fn read_is_mst_supported(adev: &amdgpu_device, dc: &mut dc, li: usize) -> bool {
    let mut mst = false;
    let mut rev = [0u8; 1];
    let mut cap = [0u8; 1];

    if dc.links[li].preferred_training_settings.mst_enable == Some(false) {
        return false;
    }

    let mut st = core_link_read_dpcd(adev, dc, li, DP_DPCD_REV, &mut rev);

    if st == dc_status::DC_OK && rev[0] >= DPCD_REV_12 {
        st = core_link_read_dpcd(adev, dc, li, DP_MSTM_CAP, &mut cap);
        if st == dc_status::DC_OK && mstm_cap_MST_CAP(cap[0]) == 1 {
            mst = true;
        }
    }
    mst
}

/// `dpcd_read_sink_ext_caps`.
fn dpcd_read_sink_ext_caps(adev: &amdgpu_device, dc: &mut dc, li: usize) -> bool {
    let mut dpcd_data = [0u8; 1];
    let mut edp_general_cap2 = [0u8; 1];

    if core_link_read_dpcd(adev, dc, li, DP_SOURCE_SINK_CAP, &mut dpcd_data) != dc_status::DC_OK {
        return false;
    }

    let link = &mut dc.links[li];
    link.dpcd_sink_ext_caps = dpcd_data[0];
    if link.is_dds && sink_ext_caps_oled(link.dpcd_sink_ext_caps) == 0 {
        link.dpcd_sink_ext_caps = 0;
        return false;
    }

    if core_link_read_dpcd(adev, dc, li, DP_EDP_GENERAL_CAP_2, &mut edp_general_cap2) != dc_status::DC_OK {
        return false;
    }

    dc.links[li].dpcd_caps.panel_luminance_control =
        edp_general_cap2[0] as u32 & DP_EDP_PANEL_LUMINANCE_CONTROL_CAPABLE != 0;

    true
}

/// `dp_retrieve_lttpr_cap`.
pub fn dp_retrieve_lttpr_cap(adev: &amdgpu_device, dc: &mut dc, li: usize) -> dc_status {
    let mut lttpr_dpcd_data = [0u8; 10];

    // Logic to determine LTTPR support
    let vbios_lttpr_interop = dc.caps.vbios_lttpr_aware;

    if !vbios_lttpr_interop || !dc.caps.extended_aux_timeout_support {
        return dc_status::DC_NOT_SUPPORTED;
    }

    // By reading LTTPR capability, RX assumes that we will enable
    // LTTPR extended aux timeout if LTTPR is present.
    let status = core_link_read_dpcd(adev, dc, li, DP_LT_TUNABLE_PHY_REPEATER_FIELD_DATA_STRUCTURE_REV,
        &mut lttpr_dpcd_data);

    let base = DP_LT_TUNABLE_PHY_REPEATER_FIELD_DATA_STRUCTURE_REV;
    let at = |a: u32| lttpr_dpcd_data[(a - base) as usize];
    let link = &mut dc.links[li];
    let caps = &mut link.dpcd_caps.lttpr_caps;
    caps.revision = at(DP_LT_TUNABLE_PHY_REPEATER_FIELD_DATA_STRUCTURE_REV);
    caps.max_link_rate = at(DP_MAX_LINK_RATE_PHY_REPEATER);
    caps.phy_repeater_cnt = at(DP_PHY_REPEATER_CNT);
    caps.max_lane_count = at(DP_MAX_LANE_COUNT_PHY_REPEATER);
    caps.mode = at(DP_PHY_REPEATER_MODE);
    caps.max_ext_timeout = at(DP_PHY_REPEATER_EXTENDED_WAIT_TIMEOUT);
    caps.main_link_channel_coding = at(DP_MAIN_LINK_CHANNEL_CODING_PHY_REPEATER);
    caps.supported_128b_132b_rates = at(DP_PHY_REPEATER_128B132B_RATES);
    caps.alpm = at(DP_LTTPR_ALPM_CAPABILITIES);

    let mut lttpr_count = dp_parse_lttpr_repeater_count(caps.phy_repeater_cnt);

    // If this chip cap is set, at least one retimer must exist in the chain
    // Override count to 1 if we receive a known bad count (0 or an invalid value)
    if (link.chip_caps as u32 & LF_AMD_EXT_DISPLAY_PATH_CAPS__EXT_CHIP_MASK) == LF_AMD_EXT_DISPLAY_PATH_CAPS__DP_FIXED_VS_EN
        && lttpr_count == 0
    {
        // If you see this message consistently, either the host platform has FIXED_VS flag
        // incorrectly configured or the sink device is returning an invalid count.
        Line::new().s("lttpr_caps phy_repeater_cnt is ").hex(link.dpcd_caps.lttpr_caps.phy_repeater_cnt as u32)
            .s(", forcing it to 0x80.").say();
        link.dpcd_caps.lttpr_caps.phy_repeater_cnt = 0x80;
        lttpr_count = 1;
    }
    let _ = lttpr_count;

    let is_lttpr_present = dp_is_lttpr_present(dc, li);

    if is_lttpr_present {
        not_ported("dp_get_closest_lttpr_offset (the LTTPR ids)");
    }

    status
}

/// `get_usbc_cable_id`: only a USB-C PHY link asks the DMUB.
fn get_usbc_cable_id(dc: &dc, li: usize, _cable_id: &mut u8) -> bool {
    let link = &dc.links[li];
    let is_usb_c = link.link_enc.as_ref()
        .is_some_and(|e| e.base().features.flags & crate::dcn21_link_encoder::DP_IS_USB_C != 0);

    if dc.ctx.dmub_srv.is_none() || link.ep_type != LF_DISPLAY_ENDPOINT_PHY || !is_usb_c {
        return false;
    }

    not_ported("DMUB_CMD_GET_USBC_CABLE_ID (get_usbc_cable_id)");
    false
}

/// `retrieve_cable_id`.
fn retrieve_cable_id(adev: &amdgpu_device, dc: &mut dc, li: usize) {
    let mut usbc_cable_id = 0u8;

    dc.links[li].dpcd_caps.cable_id = 0;
    let mut c = [0u8; 1];
    core_link_read_dpcd(adev, dc, li, DP_CABLE_ATTRIBUTES_UPDATED_BY_DPRX, &mut c);
    dc.links[li].dpcd_caps.cable_id = c[0];

    if get_usbc_cable_id(dc, li, &mut usbc_cable_id) {
        not_ported("intersect_cable_id");
    }
}

/// `dpcd_write_cable_id_to_dprx`.
pub fn dpcd_write_cable_id_to_dprx(adev: &amdgpu_device, dc: &mut dc, li: usize) {
    let link = &dc.links[li];
    if channel_coding_cap_DP_128b_132b_SUPPORTED(link.dpcd_caps.channel_coding_cap) == 0
        || link.dpcd_caps.cable_id == 0
        || link.dprx_states.cable_id_written
    {
        return;
    }

    let c = [link.dpcd_caps.cable_id];
    core_link_write_dpcd(adev, dc, li, DP_CABLE_ATTRIBUTES_UPDATED_BY_DPTX, &c);

    dc.links[li].dprx_states.cable_id_written = true;
}

/// `dpcd_get_tunneling_device_data` (link_dp_dpia.c).
fn dpcd_get_tunneling_device_data(adev: &amdgpu_device, dc: &mut dc, li: usize) -> dc_status {
    let mut dpcd_dp_tun_data = [0u8; 3];

    let mut status = core_link_read_dpcd(adev, dc, li, DP_TUNNELING_CAPABILITIES_SUPPORT, &mut dpcd_dp_tun_data);

    if status != dc_status::DC_OK {
        return status;
    }

    let base = DP_TUNNELING_CAPABILITIES_SUPPORT;
    let tun = &mut dc.links[li].dpcd_caps.usb4_dp_tun_info;
    tun.dp_tun_cap = dpcd_dp_tun_data[(DP_TUNNELING_CAPABILITIES_SUPPORT - base) as usize];

    if dp_tun_cap_dp_tunneling(tun.dp_tun_cap) == 0 {
        return status;
    }

    tun.dpia_info = dpcd_dp_tun_data[(DP_IN_ADAPTER_INFO - base) as usize];
    tun.usb4_driver_id = dpcd_dp_tun_data[(DP_USB4_DRIVER_ID - base) as usize];

    if dp_tun_cap_dpia_bw_alloc(tun.dp_tun_cap) != 0 {
        let mut d = [0u8; 2];
        status = core_link_read_dpcd(adev, dc, li, USB4_DRIVER_BW_CAPABILITY, &mut d);

        if status != dc_status::DC_OK {
            return status;
        }

        let tun = &mut dc.links[li].dpcd_caps.usb4_dp_tun_info;
        tun.driver_bw_cap = d[(USB4_DRIVER_BW_CAPABILITY - USB4_DRIVER_BW_CAPABILITY) as usize];
        tun.dpia_tunnel_info = d[(DP_IN_ADAPTER_TUNNEL_INFO - USB4_DRIVER_BW_CAPABILITY) as usize];
    }

    let mut dpcd_topology_data = [0u8; 5];
    status = core_link_read_dpcd(adev, dc, li, DP_USB4_ROUTER_TOPOLOGY_ID, &mut dpcd_topology_data);

    if status != dc_status::DC_OK {
        return status;
    }

    dc.links[li].dpcd_caps.usb4_dp_tun_info.usb4_topology_id = dpcd_topology_data;

    status
}

/// `retrieve_link_cap`.
fn retrieve_link_cap(adev: &amdgpu_device, dc: &mut dc, li: usize) -> bool {
    // DP_ADAPTER_CAP - DP_DPCD_REV + 1 == 16 and also DP_DSC_BITS_PER_PIXEL_INC - DP_DSC_SUPPORT + 1 == 16,
    // which means size 16 will be good for both of those DPCD register block reads
    let mut dpcd_data = [0u8; 16];
    // Only need to read 1 byte starting from DP_DPRX_FEATURE_ENUMERATION_LIST.
    let mut dpcd_dprx_data = [0u8; 1];
    let mut status;
    let read_dpcd_retry_cnt = 20;
    let post_oui_delay = 30; // 30ms

    // if extended timeout is supported in hardware,
    // default to LTTPR timeout (3.2ms) first as a W/A for DP link layer
    // CTS 4.2.1.1 regression introduced by CTS specs requirement update.
    try_to_configure_aux_timeout(adev, dc, li, LINK_AUX_DEFAULT_LTTPR_TIMEOUT_PERIOD);

    status = dp_retrieve_lttpr_cap(adev, dc, li);

    if status != dc_status::DC_OK {
        status = wake_up_aux_channel(adev, dc, li);
        if status == dc_status::DC_OK {
            dp_retrieve_lttpr_cap(adev, dc, li);
        } else {
            return false;
        }
    }

    if dp_is_lttpr_present(dc, li) {
        not_ported("configure_lttpr_mode_transparent");

        // Echo TOTAL_LTTPR_CNT back downstream
        let c = [dc.links[li].dpcd_caps.lttpr_caps.phy_repeater_cnt];
        core_link_write_dpcd(adev, dc, li, DP_TOTAL_LTTPR_CNT, &c);
    }

    dpcd_set_source_specific_data(adev, dc, li);

    for _ in 0..read_dpcd_retry_cnt {
        // Sink may need to configure internals based on vendor, so allow some
        // time before proceeding with possibly vendor specific transactions
        msleep(post_oui_delay);
        status = core_link_read_dpcd(adev, dc, li, DP_DPCD_REV, &mut dpcd_data);
        if status == dc_status::DC_OK {
            break;
        }
    }

    if status != dc_status::DC_OK {
        Line::new().s("retrieve_link_cap: Read receiver caps dpcd data failed.").say();
        return false;
    }

    if !dp_is_lttpr_present(dc, li) {
        try_to_configure_aux_timeout(adev, dc, li, LINK_AUX_DEFAULT_TIMEOUT_PERIOD);
    }

    {
        let aux_rd_interval = dpcd_data[DP_TRAINING_AUX_RD_INTERVAL as usize];

        dc.links[li].dpcd_caps.ext_receiver_cap_field_present =
            training_aux_rd_interval_EXT_RECEIVER_CAP_FIELD_PRESENT(aux_rd_interval) == 1;

        if training_aux_rd_interval_EXT_RECEIVER_CAP_FIELD_PRESENT(aux_rd_interval) == 1 {
            let mut ext_cap_data = [0u8; 16];

            for _ in 0..read_dpcd_retry_cnt {
                status = core_link_read_dpcd(adev, dc, li, DP_DP13_DPCD_REV, &mut ext_cap_data);
                if status == dc_status::DC_OK {
                    dpcd_data = ext_cap_data;
                    break;
                }
            }
            if status != dc_status::DC_OK {
                Line::new().s("retrieve_link_cap: Read extend caps data failed, use cap from dpcd 0.").say();
            }
        }
    }

    dc.links[li].dpcd_caps.dpcd_rev = dpcd_data[(DP_DPCD_REV - DP_DPCD_REV) as usize];

    if dc.links[li].dpcd_caps.ext_receiver_cap_field_present {
        for _ in 0..read_dpcd_retry_cnt {
            status = core_link_read_dpcd(adev, dc, li, DP_DPRX_FEATURE_ENUMERATION_LIST, &mut dpcd_dprx_data);
            if status == dc_status::DC_OK {
                break;
            }
        }

        dc.links[li].dpcd_caps.dprx_feature = dpcd_dprx_data[0];

        if status != dc_status::DC_OK {
            Line::new().s("retrieve_link_cap: Read DPRX feature list failed.").say();
        }

        // AdaptiveSyncCapability
        dpcd_dprx_data[0] = 0;
        for _ in 0..read_dpcd_retry_cnt {
            status = core_link_read_dpcd(adev, dc, li, DP_DPRX_FEATURE_ENUMERATION_LIST_CONT_1, &mut dpcd_dprx_data);
            if status == dc_status::DC_OK {
                break;
            }
        }

        dc.links[li].dpcd_caps.adaptive_sync_caps = dpcd_dprx_data[0];

        if status != dc_status::DC_OK {
            Line::new().s("retrieve_link_cap: Read DPRX feature list_1 failed.").say();
        }
    } else {
        dc.links[li].dpcd_caps.dprx_feature = 0;
    }

    // Error condition checking...
    // It is impossible for Sink to report Max Lane Count = 0.
    // It is possible for Sink to report Max Link Rate = 0, if it is
    // an eDP device that is reporting specialized link rates in the
    // SUPPORTED_LINK_RATE table.
    if dpcd_data[(DP_MAX_LANE_COUNT - DP_DPCD_REV) as usize] == 0 {
        return false;
    }

    let ds_port = dpcd_data[(DP_DOWNSTREAMPORT_PRESENT - DP_DPCD_REV) as usize];

    read_dp_device_vendor_id(adev, dc, li);

    // TODO - decouple raw mst capability from policy decision
    dc.links[li].dpcd_caps.is_mst_capable = read_is_mst_supported(adev, dc, li);

    get_active_converter_info(adev, dc, li, ds_port);

    dp_wa_power_up_0010FA(adev, dc, li, &mut dpcd_data);

    {
        let link = &mut dc.links[li];
        let down_strm_port_count = dpcd_data[(DP_DOWN_STREAM_PORT_COUNT - DP_DPCD_REV) as usize];

        link.dpcd_caps.allow_invalid_MSA_timing_param =
            down_stream_port_count_IGNORE_MSA_TIMING_PARAM(down_strm_port_count) != 0;

        link.dpcd_caps.max_ln_count = dpcd_data[(DP_MAX_LANE_COUNT - DP_DPCD_REV) as usize];

        link.dpcd_caps.max_down_spread = dpcd_data[(DP_MAX_DOWNSPREAD - DP_DPCD_REV) as usize];

        link.reported_link_cap.lane_count = max_lane_count_MAX_LANE_COUNT(link.dpcd_caps.max_ln_count) as u32;
        link.reported_link_cap.link_rate =
            get_link_rate_from_max_link_bw(dpcd_data[(DP_MAX_LINK_RATE - DP_DPCD_REV) as usize]);
        link.reported_link_cap.link_spread = if max_down_spread_MAX_DOWN_SPREAD(link.dpcd_caps.max_down_spread) != 0 {
            LINK_SPREAD_05_DOWNSPREAD_30KHZ
        } else {
            LINK_SPREAD_DISABLED
        };

        let edp_config_cap = dpcd_data[(DP_EDP_CONFIGURATION_CAP - DP_DPCD_REV) as usize];
        link.dpcd_caps.panel_mode_edp = edp_configuration_cap_ALT_SCRAMBLER_RESET(edp_config_cap) != 0;
        link.dpcd_caps.dpcd_display_control_capable =
            edp_configuration_cap_DPCD_DISPLAY_CONTROL_CAPABLE(edp_config_cap) != 0;
        link.dpcd_caps.channel_coding_cap = dpcd_data[(DP_MAIN_LINK_CHANNEL_CODING - DP_DPCD_REV) as usize];
        link.test_pattern_enabled = false;
        link.compliance_test_state = 0;

        link.dpcd_caps.receive_port0_cap[0] = dpcd_data[(DP_RECEIVE_PORT_0_CAP_0 - DP_DPCD_REV) as usize];
        link.dpcd_caps.receive_port0_cap[1] = dpcd_data[(DP_RECEIVE_PORT_0_BUFFER_SIZE - DP_DPCD_REV) as usize];
    }

    // read sink count
    let mut sc = [0u8; 1];
    core_link_read_dpcd(adev, dc, li, DP_SINK_COUNT, &mut sc);
    dc.links[li].dpcd_caps.sink_count = sc[0];

    // read sink ieee oui
    let mut sink_id = [0u8; 9];
    core_link_read_dpcd(adev, dc, li, DP_SINK_OUI, &mut sink_id);

    {
        let caps = &mut dc.links[li].dpcd_caps;
        caps.sink_dev_id = ((sink_id[0] as u32) << 16) + ((sink_id[1] as u32) << 8) + sink_id[2] as u32;

        caps.sink_dev_id_str.copy_from_slice(&sink_id[3..9]);
    }

    let mut dp_hw_fw_revision = [0u8; 3];
    core_link_read_dpcd(adev, dc, li, DP_SINK_HW_REVISION_START, &mut dp_hw_fw_revision);

    {
        let link = &mut dc.links[li];
        link.dpcd_caps.sink_hw_revision = dp_hw_fw_revision[0];

        link.dpcd_caps.sink_fw_revision.copy_from_slice(&dp_hw_fw_revision[1..3]);

        // Quirk for Retina panels: wrong DP_MAX_LINK_RATE
        let str_mbp_2018: [u8; 6] = [101, 68, 21, 103, 98, 97];
        let fwrev_mbp_2018: [u8; 2] = [7, 4];
        let fwrev_mbp_2018_vega: [u8; 2] = [8, 4];

        // We also check for the firmware revision as 16,1 models have an
        // identical device id and are incorrectly quirked otherwise.
        if link.dpcd_caps.sink_dev_id == 0x0010fa
            && link.dpcd_caps.sink_dev_id_str == str_mbp_2018
            && (link.dpcd_caps.sink_fw_revision == fwrev_mbp_2018
                || link.dpcd_caps.sink_fw_revision == fwrev_mbp_2018_vega)
        {
            link.reported_link_cap.link_rate = LINK_RATE_RBR2;
        }

        link.dpcd_caps.dsc_caps = dpcd_dsc_capabilities::default();
        link.dpcd_caps.fec_cap = 0;
    }
    // Read DSC and FEC sink capabilities if DP revision is 1.4 and up
    if dc.links[li].dpcd_caps.dpcd_rev >= DPCD_REV_14 {
        let mut fec = [0u8; 1];
        status = core_link_read_dpcd(adev, dc, li, DP_FEC_CAPABILITY, &mut fec);
        dc.links[li].dpcd_caps.fec_cap = fec[0];
        if status != dc_status::DC_OK {
            Line::new().s("retrieve_link_cap: core_link_read_dpcd (DP_FEC_CAPABILITY) failed").say();
        }

        let mut dsc = [0u8; 16];
        status = core_link_read_dpcd(adev, dc, li, DP_DSC_SUPPORT, &mut dsc);
        dc.links[li].dpcd_caps.dsc_caps.dsc_basic_caps = dsc;
        if status == dc_status::DC_OK {
            // DC_LOG_DC: FEC_Sink_Support, DSC_Basic_Sink_Support, DSC_Passthrough_Sink_Support
        }
        if dc.links[li].dpcd_caps.dongle_type != DISPLAY_DONGLE_NONE {
            let mut b = [0u8; 3];
            status = core_link_read_dpcd(adev, dc, li, DP_DSC_BRANCH_OVERALL_THROUGHPUT_0, &mut b);
            dc.links[li].dpcd_caps.dsc_caps.dsc_branch_decoder_caps = b;
            if status != dc_status::DC_OK {
                Line::new().s("retrieve_link_cap: core_link_read_dpcd (DP_DSC_BRANCH_OVERALL_THROUGHPUT_0) failed")
                    .say();
            }
        }

        // Apply work around to disable FEC and DSC for USB4 tunneling in TBT3 compatibility mode
        // only if required. DCN 2.1 has no USB4 DPIA endpoint.
        dc.links[li].wa_flags.dpia_forced_tbt3_mode = false;
    }

    if !dpcd_read_sink_ext_caps(adev, dc, li) {
        dc.links[li].dpcd_sink_ext_caps = 0;
    }

    if channel_coding_cap_DP_128b_132b_SUPPORTED(dc.links[li].dpcd_caps.channel_coding_cap) != 0 {
        not_ported("the 128b/132b receiver caps (DP 2.x)");
    }

    let mut m = [0u8; 2];
    core_link_read_dpcd(adev, dc, li, DPCD_MAX_UNCOMPRESSED_PIXEL_RATE_CAP, &mut m);
    dc.links[li].dpcd_caps.max_uncompressed_pixel_rate_cap = m;

    // Read DP tunneling information.
    let _ = dpcd_get_tunneling_device_data(adev, dc, li);

    retrieve_cable_id(adev, dc, li);
    dpcd_write_cable_id_to_dprx(adev, dc, li);

    true
}

/// `set_default_brightness_aux` (link_edp_panel_control.c).
pub fn set_default_brightness_aux(dc: &mut dc, li: usize) -> bool {
    if sink_ext_caps_oled(dc.links[li].dpcd_sink_ext_caps) == 1 {
        not_ported("read_default_bl_aux / edp_set_backlight_level_nits (OLED)");
    }
    false
}

/// `detect_edp_sink_caps`.
pub fn detect_edp_sink_caps(adev: &amdgpu_device, dc: &mut dc, li: usize) {
    let mut supported_link_rates = [0u8; 16];
    let mut backlight_adj_cap = [0u8; 1];
    let mut general_edp_cap = [0u8; 1];

    retrieve_link_cap(adev, dc, li);
    dc.links[li].dpcd_caps.edp_supported_link_rates_count = 0;

    // edp_supported_link_rates_count is only valid for eDP v1.4 or higher.
    // Per VESA eDP spec, "The DPCD revision for eDP v1.4 is 13h"
    if dc.links[li].dpcd_caps.dpcd_rev >= DPCD_REV_13 {
        // Read DPCD 00010h - 0001Fh 16 bytes at one shot
        core_link_read_dpcd(adev, dc, li, DP_SUPPORTED_LINK_RATES, &mut supported_link_rates);

        let caps = &mut dc.links[li].dpcd_caps;
        for entry in (0..16).step_by(2) {
            // DPCD register reports per-lane link rate = 16-bit link rate capability
            // value X 200 kHz. Need multiplier to find link rate in kHz.
            let link_rate_in_khz =
                (supported_link_rates[entry + 1] as u32 * 0x100 + supported_link_rates[entry] as u32) * 200;

            if link_rate_in_khz != 0 {
                let link_rate = linkRateInKHzToLinkRateMultiplier(link_rate_in_khz);
                caps.edp_supported_link_rates[caps.edp_supported_link_rates_count as usize] = link_rate;
                caps.edp_supported_link_rates_count += 1;
            }
        }
    }

    core_link_read_dpcd(adev, dc, li, DP_EDP_BACKLIGHT_ADJUSTMENT_CAP, &mut backlight_adj_cap);

    dc.links[li].dpcd_caps.dynamic_backlight_capable_edp =
        backlight_adj_cap[0] as u32 & DP_EDP_DYNAMIC_BACKLIGHT_CAP != 0;

    core_link_read_dpcd(adev, dc, li, DP_EDP_GENERAL_CAP_1, &mut general_edp_cap);

    dc.links[li].dpcd_caps.set_power_state_capable_edp = general_edp_cap[0] as u32 & DP_EDP_SET_POWER_CAP != 0;

    set_default_brightness_aux(dc, li);

    let mut b = [0u8; 1];
    core_link_read_dpcd(adev, dc, li, DP_EDP_DPCD_REV, &mut b);
    dc.links[li].dpcd_caps.edp_rev = b[0];
    // PSR is only valid for eDP v1.3 or higher.
    if dc.links[li].dpcd_caps.edp_rev as u32 >= DP_EDP_13 {
        core_link_read_dpcd(adev, dc, li, DP_PSR_SUPPORT, &mut b);
        dc.links[li].dpcd_caps.psr_info.psr_version = b[0];
        if dc.links[li].dpcd_caps.sink_dev_id == DP_BRANCH_DEVICE_ID_001CF8 {
            core_link_read_dpcd(adev, dc, li, DP_FORCE_PSRSU_CAPABILITY, &mut b);
            dc.links[li].dpcd_caps.psr_info.force_psrsu_cap = b[0];
        }
        core_link_read_dpcd(adev, dc, li, DP_PSR_CAPS, &mut b);
        dc.links[li].dpcd_caps.psr_info.psr_dpcd_caps = b[0];
        if psr_dpcd_caps_Y_COORDINATE_REQUIRED(dc.links[li].dpcd_caps.psr_info.psr_dpcd_caps) != 0 {
            core_link_read_dpcd(adev, dc, li, DP_PSR2_SU_Y_GRANULARITY, &mut b);
            dc.links[li].dpcd_caps.psr_info.psr2_su_y_granularity_cap = b[0];
        }
    }

    // ALPM is only valid for eDP v1.4 or higher.
    if dc.links[li].dpcd_caps.dpcd_rev as u32 >= DP_EDP_14 {
        core_link_read_dpcd(adev, dc, li, DP_RECEIVER_ALPM_CAP, &mut b);
        dc.links[li].dpcd_caps.alpm_caps = b[0];
    }

    // Read REPLAY info
    core_link_read_dpcd(adev, dc, li, DP_SINK_PR_PIXEL_DEVIATION_PER_LINE, &mut b);
    dc.links[li].dpcd_caps.pr_info.pixel_deviation_per_line = b[0];
    core_link_read_dpcd(adev, dc, li, DP_SINK_PR_MAX_NUMBER_OF_DEVIATION_LINE, &mut b);
    dc.links[li].dpcd_caps.pr_info.max_deviation_line = b[0];

    // OLED Emission Rate info
    if sink_ext_caps_emission_output(dc.links[li].dpcd_sink_ext_caps) != 0 {
        let mut e = [0u8; 2];
        core_link_read_dpcd(adev, dc, li, DP_SINK_EMISSION_RATE, &mut e);
        dc.links[li].dpcd_caps.edp_oled_emission_rate = u16::from_le_bytes(e);
    }

    // Read Multi-SST (Single Stream Transport) capability
    // for eDP version 1.4 or higher.
    if dc.links[li].dpcd_caps.dpcd_rev as u32 >= DP_EDP_14 {
        core_link_read_dpcd(adev, dc, li, DP_EDP_MSO_LINK_CAPABILITIES, &mut b);
        dc.links[li].dpcd_caps.mso_cap_sst_links_supported = b[0];
    }
    // Read eDP general capability 2
    core_link_read_dpcd(adev, dc, li, DP_EDP_GENERAL_CAP_2, &mut b);
    dc.links[li].dpcd_caps.dp_edp_general_cap_2 = b[0];
}

/// `get_temp_dio_link_enc` (dc_resource.c): the DIG the link would use,
/// as the index of the link owning that encoder.
fn get_temp_dio_link_enc(dc: &dc, li: usize) -> Option<u32> {
    let link = &dc.links[li];
    let enc_index = if link.is_dig_mapping_flexible {
        crate::linux::Line::new().s("get_temp_dio_link_enc: find_acquired_dio_link_enc_for_link not ported").say();
        -1
    } else {
        link.eng_id as i32
    };

    if enc_index < 0 {
        crate::linux::Line::new().s("get_temp_dio_link_enc: find_free_dio_link_enc not ported").say();
        return None;
    }

    dc.res_pool.as_deref().unwrap().link_encoders.get(enc_index as usize).copied().flatten()
}

/// `dcn10_link_encoder_get_max_link_cap` (dcn10_link_encoder.c).
fn dcn10_link_encoder_get_max_link_cap(flags: u32, link_settings: &mut dc_link_settings) {
    use crate::dcn21_link_encoder::{IS_HBR2_CAPABLE, IS_HBR3_CAPABLE, IS_UHBR10_CAPABLE, IS_UHBR13_5_CAPABLE,
        IS_UHBR20_CAPABLE};
    // Set Default link settings
    let mut max_link_cap = dc_link_settings {
        lane_count: crate::dc_dp_types::LANE_COUNT_FOUR,
        link_rate: LINK_RATE_HIGH,
        link_spread: LINK_SPREAD_05_DOWNSPREAD_30KHZ,
        use_link_rate_set: false,
        link_rate_set: 0,
    };

    // Higher link settings based on feature supported
    if flags & IS_HBR2_CAPABLE != 0 {
        max_link_cap.link_rate = LINK_RATE_HIGH2;
    }

    if flags & IS_HBR3_CAPABLE != 0 {
        max_link_cap.link_rate = LINK_RATE_HIGH3;
    }

    if flags & IS_UHBR10_CAPABLE != 0 {
        max_link_cap.link_rate = LINK_RATE_UHBR10;
    }

    if flags & IS_UHBR13_5_CAPABLE != 0 {
        max_link_cap.link_rate = LINK_RATE_UHBR13_5;
    }

    if flags & IS_UHBR20_CAPABLE != 0 {
        max_link_cap.link_rate = LINK_RATE_UHBR20;
    }

    *link_settings = max_link_cap;
}

/// `dcn20_link_encoder_get_max_link_cap` (dcn20_link_encoder.c): in USB-C
/// DP2 mode the lanes are two.
fn dcn20_link_encoder_get_max_link_cap(adev: &amdgpu_device, enc: &crate::link_factory::link_enc,
                                       link_settings: &mut dc_link_settings) {
    dcn10_link_encoder_get_max_link_cap(enc.base().features.flags, link_settings);

    // in usb c dp2 mode, max lane count is 2
    if crate::dcn21_link_encoder::dcn20_link_encoder_is_in_alt_mode(adev, enc) {
        let is_in_usb_c_dp4_mode = crate::dcn21_link_encoder::rdpcs_phy_dpalt_dp4(adev, enc);
        if is_in_usb_c_dp4_mode == 0 {
            link_settings.lane_count = LANE_COUNT_TWO.min(link_settings.lane_count);
        }
    }
}

/// `dp_get_max_link_cap`.
pub fn dp_get_max_link_cap(adev: &amdgpu_device, dc: &dc, li: usize) -> dc_link_settings {
    let mut max_link_cap = dc_link_settings::default();
    let mut is_uhbr13_5_supported = true;

    let mut link_enc = get_temp_dio_link_enc(dc, li);

    if !dc.config.unify_link_enc_assignment {
        // link_enc_cfg_get_link_enc: a fixed DIG mapping is the link's own
        link_enc = if dc.links[li].is_dig_mapping_flexible {
            crate::linux::Line::new().s("link_enc_cfg_get_link_enc: dynamic assignment not ported").say();
            None
        } else {
            Some(li as u32)
        };
    }

    let link = &dc.links[li];
    // get max link encoder capability
    if let Some(owner) = link_enc {
        if let Some(enc) = dc.links[owner as usize].link_enc.as_ref() {
            dcn20_link_encoder_get_max_link_cap(adev, enc, &mut max_link_cap);
        }
    }

    // Lower link settings based on sink's link cap
    if link.reported_link_cap.lane_count < max_link_cap.lane_count {
        max_link_cap.lane_count = link.reported_link_cap.lane_count;
    }
    if link.reported_link_cap.link_rate < max_link_cap.link_rate {
        max_link_cap.link_rate = link.reported_link_cap.link_rate;
    }
    if link.reported_link_cap.link_spread < max_link_cap.link_spread {
        max_link_cap.link_spread = link.reported_link_cap.link_spread;
    }

    if rates_128b_132b_UHBR13_5(link.dpcd_caps.dp_128b_132b_supported_link_rates) == 0 {
        is_uhbr13_5_supported = false;
    }

    // Lower link settings based on cable attributes
    // Cable ID is a DP2 feature to identify max certified link rate that
    // a cable can carry. The cable identification method requires both
    // cable and display hardware support. Since the specs comes late, it is
    // anticipated that the first round of DP2 cables and displays may not
    // be fully compatible to reliably return cable ID data. Therefore the
    // decision of our cable id policy is that if the cable can return non
    // zero cable id data, we will take cable's link rate capability into
    // account. However if we get zero data, the cable link rate capability
    // is considered inconclusive. In this case, we will not take cable's
    // capability into account to avoid of over limiting hardware capability
    // from users. The max overall link rate capability is still determined
    // after actual dp pre-training. Cable id is considered as an auxiliary
    // method of determining max link bandwidth capability.
    let cable_max_link_rate = get_cable_max_link_rate(dc, li);

    if !dc.debug.ignore_cable_id && cable_max_link_rate != LINK_RATE_UNKNOWN {
        if cable_max_link_rate < max_link_cap.link_rate {
            max_link_cap.link_rate = cable_max_link_rate;
        }

        if cable_id_UHBR13_5_CAPABILITY(link.dpcd_caps.cable_id) == 0 && cable_id_CABLE_TYPE(link.dpcd_caps.cable_id) >= 2 {
            is_uhbr13_5_supported = false;
        }
    }

    // account for lttpr repeaters cap
    // notes: repeaters do not snoop in the DPRX Capabilities addresses (3.6.3).
    if dp_is_lttpr_present(dc, li) && link.dpcd_caps.lttpr_caps.revision >= DPCD_REV_14 {
        not_ported("get_lttpr_max_link_rate");
    }

    if max_link_cap.link_rate == LINK_RATE_UHBR13_5 && !is_uhbr13_5_supported {
        max_link_cap.link_rate = LINK_RATE_UHBR10;
    }

    if link_dp_get_encoding_format(&max_link_cap) == dp_link_encoding::DP_128b_132b_ENCODING && dc.debug.disable_uhbr {
        max_link_cap.link_rate = LINK_RATE_HIGH3;
    }

    max_link_cap
}

/// `get_max_edp_link_rate` (link_edp_panel_control.c).
pub fn get_max_edp_link_rate(adev: &amdgpu_device, dc: &dc, li: usize) -> u32 {
    let mut max_ilr_rate = LINK_RATE_UNKNOWN;
    let max_non_ilr_rate = dp_get_max_link_cap(adev, dc, li).link_rate;
    let caps = &dc.links[li].dpcd_caps;

    for i in 0..caps.edp_supported_link_rates_count as usize {
        if max_ilr_rate < caps.edp_supported_link_rates[i] {
            max_ilr_rate = caps.edp_supported_link_rates[i];
        }
    }

    if max_ilr_rate > max_non_ilr_rate { max_ilr_rate } else { max_non_ilr_rate }
}

/// `dp_is_fec_supported`.
pub fn dp_is_fec_supported(dc: &dc, li: usize) -> bool {
    // TODO - use asic cap instead of link_enc->features
    // we no longer know which link enc to use for this link before commit
    let mut link_enc = get_temp_dio_link_enc(dc, li);

    let link = &dc.links[li];
    if !dc.config.unify_link_enc_assignment {
        // link_enc_cfg_get_link_enc: a fixed DIG mapping is the link's own
        link_enc = if link.is_dig_mapping_flexible {
            crate::linux::Line::new().s("link_enc_cfg_get_link_enc: dynamic assignment not ported").say();
            None
        } else {
            link.link_enc.as_ref().map(|_| li as u32)
        };
    }

    crate::link_detection::dc_is_dp_signal(link.connector_signal)
        && link_enc.is_some_and(|l| dc.links[l as usize].link_enc.as_ref().unwrap().base().features.fec_supported)
        && fec_cap_FEC_CAPABLE(link.dpcd_caps.fec_cap) != 0
}

/// `dp_should_enable_fec`.
pub fn dp_should_enable_fec(dc: &dc, li: usize) -> bool {
    let link = &dc.links[li];
    let force_disable = if link.fec_state == dc_link_fec_enabled {
        false
    } else if link.connector_signal != LF_SIGNAL_TYPE_DISPLAY_PORT_MST
        && link.local_sink.as_deref().is_some_and(|s| s.edid_caps.panel_patch.disable_fec != 0)
    {
        true
    } else {
        link.connector_signal == LF_SIGNAL_TYPE_EDP
            && (dsc_support_DSC_SUPPORT(link.dpcd_caps.dsc_caps.dsc_basic_caps[0]) == 0
                || link.panel_config.dsc_disable_dsc_edp
                || !dc.caps.edp_dsc_support)
    };

    !force_disable && dp_is_fec_supported(dc, li)
}

/// `dp_link_bandwidth_kbps` (link_validation.c).
pub fn dp_link_bandwidth_kbps(dc: &dc, li: usize, link_settings: &dc_link_settings) -> u32 {
    let mut total_data_bw_efficiency_x10000 = 0;
    let mut link_rate_per_lane_kbps = 0;

    match link_dp_get_encoding_format(link_settings) {
        dp_link_encoding::DP_8b_10b_ENCODING => {
            // For 8b/10b encoding:
            // link rate is defined in the unit of LINK_RATE_REF_FREQ_IN_KHZ per DP byte per lane.
            // data bandwidth efficiency is 80% with additional 3% overhead if FEC is supported.
            link_rate_per_lane_kbps = link_settings.link_rate * LINK_RATE_REF_FREQ_IN_KHZ * BITS_PER_DP_BYTE;
            total_data_bw_efficiency_x10000 = DATA_EFFICIENCY_8b_10b_x10000;
            if dp_should_enable_fec(dc, li) {
                total_data_bw_efficiency_x10000 /= 100;
                total_data_bw_efficiency_x10000 *= DATA_EFFICIENCY_8b_10b_FEC_EFFICIENCY_x100;
            }
        }
        dp_link_encoding::DP_128b_132b_ENCODING => {
            // For 128b/132b encoding:
            // link rate is defined in the unit of 10mbps per lane.
            // total data bandwidth efficiency is always 96.71%.
            link_rate_per_lane_kbps = link_settings.link_rate * 10000;
            total_data_bw_efficiency_x10000 = DATA_EFFICIENCY_128b_132b_x10000;
        }
        _ => {}
    }

    // overall effective link bandwidth = link rate per lane * lane count * total data bandwidth efficiency
    link_rate_per_lane_kbps * link_settings.lane_count / 10000 * total_data_bw_efficiency_x10000
}

/// `dc_link_get_highest_encoding_format` (dc_link_exports.c).
pub fn dc_link_get_highest_encoding_format(dc: &dc, li: usize) -> u32 {
    let link = &dc.links[li];
    if crate::link_detection::dc_is_dp_signal(link.connector_signal) {
        if link.dpcd_caps.dongle_type >= DISPLAY_DONGLE_DP_DVI_DONGLE
            && link.dpcd_caps.dongle_type <= DISPLAY_DONGLE_DP_HDMI_MISMATCHED_DONGLE
        {
            return DC_LINK_ENCODING_HDMI_TMDS;
        } else if link_dp_get_encoding_format(&link.verified_link_cap) == dp_link_encoding::DP_8b_10b_ENCODING {
            return DC_LINK_ENCODING_DP_8b_10b;
        } else if link_dp_get_encoding_format(&link.verified_link_cap) == dp_link_encoding::DP_128b_132b_ENCODING {
            return DC_LINK_ENCODING_DP_128b_132b;
        }
    }

    DC_LINK_ENCODING_UNSPECIFIED
}

/// `increase_lane_count`.
fn increase_lane_count(lane_count: u32) -> u32 {
    match lane_count {
        LANE_COUNT_ONE => LANE_COUNT_TWO,
        LANE_COUNT_TWO => crate::dc_dp_types::LANE_COUNT_FOUR,
        _ => LANE_COUNT_UNKNOWN,
    }
}

/// `increase_link_rate`.
fn increase_link_rate(dc: &dc, li: usize, link_rate: u32) -> u32 {
    match link_rate {
        LINK_RATE_LOW => LINK_RATE_HIGH,
        LINK_RATE_HIGH => LINK_RATE_HIGH2,
        LINK_RATE_HIGH2 => LINK_RATE_HIGH3,
        LINK_RATE_HIGH3 => LINK_RATE_UHBR10,
        // upto DP2.x specs UHBR13.5 is the only link rate that could be
        // not supported by DPRX when higher link rate is supported.
        // so we treat it as a special case for code simplicity. When we
        // have new specs with more link rates like this, we should
        // consider a more generic solution to handle discrete link
        // rate capabilities.
        LINK_RATE_UHBR10 => {
            if rates_128b_132b_UHBR13_5(dc.links[li].dpcd_caps.dp_128b_132b_supported_link_rates) != 0 {
                LINK_RATE_UHBR13_5
            } else {
                LINK_RATE_UHBR20
            }
        }
        LINK_RATE_UHBR13_5 => LINK_RATE_UHBR20,
        _ => LINK_RATE_UNKNOWN,
    }
}

/// `increase_edp_link_rate`.
fn increase_edp_link_rate(dc: &dc, li: usize, current_link_setting: &mut dc_link_settings) {
    let caps = &dc.links[li].dpcd_caps;
    if current_link_setting.use_link_rate_set {
        if current_link_setting.link_rate_set < caps.edp_supported_link_rates_count {
            current_link_setting.link_rate_set += 1;
            current_link_setting.link_rate = caps.edp_supported_link_rates[current_link_setting.link_rate_set as usize];
        } else {
            current_link_setting.use_link_rate_set = false;
            current_link_setting.link_rate = LINK_RATE_UHBR10;
        }
    } else {
        current_link_setting.link_rate = increase_link_rate(dc, li, current_link_setting.link_rate);
    }
}

/// `edp_is_ilr_optimization_enabled` (link_edp_panel_control.c).
pub fn edp_is_ilr_optimization_enabled(dc: &dc, li: usize) -> bool {
    let link = &dc.links[li];
    !(link.dpcd_caps.edp_supported_link_rates_count == 0 || !link.panel_config.ilr_optimize_edp_link_rate)
}

/// `decide_dp_link_settings`.
fn decide_dp_link_settings(dc: &dc, li: usize, link_setting: &mut dc_link_settings, req_bw: u32) -> bool {
    let link = &dc.links[li];
    let initial_link_setting = dc_link_settings {
        lane_count: LANE_COUNT_ONE,
        link_rate: LINK_RATE_LOW,
        link_spread: LINK_SPREAD_DISABLED,
        use_link_rate_set: false,
        link_rate_set: 0,
    };
    let mut current_link_setting = initial_link_setting;

    if req_bw > dp_link_bandwidth_kbps(dc, li, &link.verified_link_cap) {
        return false;
    }

    // search for the minimum link setting that:
    // 1. is supported according to the link training result
    // 2. could support the b/w requested by the timing
    while current_link_setting.link_rate <= link.verified_link_cap.link_rate {
        let link_bw = dp_link_bandwidth_kbps(dc, li, &current_link_setting);
        if req_bw <= link_bw {
            *link_setting = current_link_setting;
            return true;
        }

        if current_link_setting.lane_count < link.verified_link_cap.lane_count {
            current_link_setting.lane_count = increase_lane_count(current_link_setting.lane_count);
        } else {
            current_link_setting.link_rate = increase_link_rate(dc, li, current_link_setting.link_rate);
            current_link_setting.lane_count = initial_link_setting.lane_count;
        }
    }

    false
}

/// `edp_decide_link_settings`.
pub fn edp_decide_link_settings(dc: &dc, li: usize, link_setting: &mut dc_link_settings, req_bw: u32) -> bool {
    let link = &dc.links[li];

    // edp_supported_link_rates_count is only valid for eDP v1.4 or higher.
    // Per VESA eDP spec, "The DPCD revision for eDP v1.4 is 13h"
    if !edp_is_ilr_optimization_enabled(dc, li) {
        *link_setting = link.verified_link_cap;
        return true;
    }

    let initial_link_setting = dc_link_settings {
        lane_count: LANE_COUNT_ONE,
        link_rate: link.dpcd_caps.edp_supported_link_rates[0],
        link_spread: LINK_SPREAD_DISABLED,
        use_link_rate_set: true,
        link_rate_set: 0,
    };
    let mut current_link_setting = initial_link_setting;

    // search for the minimum link setting that:
    // 1. is supported according to the link training result
    // 2. could support the b/w requested by the timing
    while current_link_setting.link_rate <= link.verified_link_cap.link_rate {
        let link_bw = dp_link_bandwidth_kbps(dc, li, &current_link_setting);
        if req_bw <= link_bw {
            *link_setting = current_link_setting;
            return true;
        }

        if current_link_setting.lane_count < link.verified_link_cap.lane_count {
            current_link_setting.lane_count = increase_lane_count(current_link_setting.lane_count);
        } else {
            increase_edp_link_rate(dc, li, &mut current_link_setting);
        }
    }
    false
}

/// `decide_edp_link_settings_with_dsc`.
pub fn decide_edp_link_settings_with_dsc(dc: &dc, li: usize, link_setting: &mut dc_link_settings, req_bw: u32,
                                         mut max_link_rate: u32) -> bool {
    let link = &dc.links[li];
    let policy = link.panel_config.dsc_force_dsc_edp_policy;
    if max_link_rate == LINK_RATE_UNKNOWN {
        max_link_rate = link.verified_link_cap.link_rate;
    }
    // edp_supported_link_rates_count is only valid for eDP v1.4 or higher.
    // Per VESA eDP spec, "The DPCD revision for eDP v1.4 is 13h"
    if !edp_is_ilr_optimization_enabled(dc, li) {
        // for DSC enabled case, we search for minimum lane count
        let initial_link_setting = dc_link_settings {
            lane_count: LANE_COUNT_ONE,
            link_rate: LINK_RATE_LOW,
            link_spread: LINK_SPREAD_DISABLED,
            use_link_rate_set: false,
            link_rate_set: 0,
        };
        let mut current_link_setting = initial_link_setting;
        if req_bw > dp_link_bandwidth_kbps(dc, li, &link.verified_link_cap) {
            return false;
        }

        // search for the minimum link setting that:
        // 1. is supported according to the link training result
        // 2. could support the b/w requested by the timing
        while current_link_setting.link_rate <= max_link_rate {
            let link_bw = dp_link_bandwidth_kbps(dc, li, &current_link_setting);
            if req_bw <= link_bw {
                *link_setting = current_link_setting;
                return true;
            }
            if policy != 0 {
                // minimize lane
                if current_link_setting.link_rate < max_link_rate {
                    increase_edp_link_rate(dc, li, &mut current_link_setting);
                } else if current_link_setting.lane_count < link.verified_link_cap.lane_count {
                    current_link_setting.lane_count = increase_lane_count(current_link_setting.lane_count);
                    current_link_setting.link_rate = initial_link_setting.link_rate;
                } else {
                    break;
                }
            } else {
                // minimize link rate
                if current_link_setting.lane_count < link.verified_link_cap.lane_count {
                    current_link_setting.lane_count = increase_lane_count(current_link_setting.lane_count);
                } else {
                    increase_edp_link_rate(dc, li, &mut current_link_setting);
                    current_link_setting.lane_count = initial_link_setting.lane_count;
                }
            }
        }
        return false;
    }

    // if optimize edp link is supported
    let initial_link_setting = dc_link_settings {
        lane_count: LANE_COUNT_ONE,
        link_rate: link.dpcd_caps.edp_supported_link_rates[0],
        link_spread: LINK_SPREAD_DISABLED,
        use_link_rate_set: true,
        link_rate_set: 0,
    };
    let mut current_link_setting = initial_link_setting;

    // search for the minimum link setting that:
    // 1. is supported according to the link training result
    // 2. could support the b/w requested by the timing
    while current_link_setting.link_rate <= max_link_rate {
        let link_bw = dp_link_bandwidth_kbps(dc, li, &current_link_setting);
        if req_bw <= link_bw {
            *link_setting = current_link_setting;
            return true;
        }
        if policy != 0 {
            // minimize lane
            if current_link_setting.link_rate < max_link_rate {
                increase_edp_link_rate(dc, li, &mut current_link_setting);
            } else if current_link_setting.lane_count < link.verified_link_cap.lane_count {
                current_link_setting.lane_count = increase_lane_count(current_link_setting.lane_count);
                current_link_setting.link_rate_set = initial_link_setting.link_rate_set;
                current_link_setting.use_link_rate_set = initial_link_setting.use_link_rate_set;
                current_link_setting.link_rate =
                    link.dpcd_caps.edp_supported_link_rates[current_link_setting.link_rate_set as usize];
            } else {
                break;
            }
        } else {
            // minimize link rate
            if current_link_setting.lane_count < link.verified_link_cap.lane_count {
                current_link_setting.lane_count = increase_lane_count(current_link_setting.lane_count);
            } else {
                increase_edp_link_rate(dc, li, &mut current_link_setting);
                if current_link_setting.link_rate == LINK_RATE_UNKNOWN {
                    break;
                }
            }
        }
    }
    false
}

/// `decide_mst_link_settings`.
fn decide_mst_link_settings(dc: &dc, li: usize, link_setting: &mut dc_link_settings) -> bool {
    *link_setting = dc.links[li].verified_link_cap;
    true
}

/// `link_decide_link_settings`, the link service's
/// `dp_decide_link_settings`.
pub fn link_decide_link_settings(dc: &dc, stream: &crate::dc_stream::dc_stream_state,
                                 link_setting: &mut dc_link_settings) -> bool {
    let li = stream.link;
    let link = &dc.links[li];
    let req_bw = crate::dc_dsc::dc_bandwidth_in_kbps_from_timing(&stream.timing,
        dc_link_get_highest_encoding_format(dc, li));

    *link_setting = dc_link_settings::default();

    if crate::link_detection::dc_is_dp_signal(stream.signal)
        && link.preferred_link_setting.lane_count != LANE_COUNT_UNKNOWN
        && link.preferred_link_setting.link_rate != LINK_RATE_UNKNOWN
    {
        // if preferred is specified through AMDDP, use it, if it's enough
        // to drive the mode
        *link_setting = link.preferred_link_setting;
    } else if stream.signal == LF_SIGNAL_TYPE_DISPLAY_PORT_MST {
        // MST doesn't perform link training for now
        // TODO: add MST specific link training routine
        decide_mst_link_settings(dc, li, link_setting);
    } else if stream.signal == LF_SIGNAL_TYPE_VIRTUAL {
        link_setting.lane_count = crate::dc_dp_types::LANE_COUNT_FOUR;
        link_setting.link_rate = LINK_RATE_HIGH3;
    } else if link.connector_signal == LF_SIGNAL_TYPE_EDP {
        // enable edp link optimization for DSC eDP case
        if stream.timing.flags.DSC {
            let mut max_link_rate = LINK_RATE_UNKNOWN;

            if link.panel_config.dsc_force_dsc_edp_policy != 0 {
                // calculate link max link rate cap
                let mut tmp_link_setting = dc_link_settings { link_rate: LINK_RATE_UNKNOWN, ..Default::default() };
                let mut tmp_timing = stream.timing;

                tmp_timing.flags.DSC = false;
                let orig_req_bw = crate::dc_dsc::dc_bandwidth_in_kbps_from_timing(&tmp_timing,
                    dc_link_get_highest_encoding_format(dc, li));
                edp_decide_link_settings(dc, li, &mut tmp_link_setting, orig_req_bw);
                max_link_rate = tmp_link_setting.link_rate;
            }
            decide_edp_link_settings_with_dsc(dc, li, link_setting, req_bw, max_link_rate);
        } else {
            edp_decide_link_settings(dc, li, link_setting, req_bw);
        }
    } else {
        decide_dp_link_settings(dc, li, link_setting, req_bw);
    }

    link_setting.lane_count != LANE_COUNT_UNKNOWN && link_setting.link_rate != LINK_RATE_UNKNOWN
}

/// `link_decide_dp_tunnel_settings` (link_dp_dpia.c).
pub fn link_decide_dp_tunnel_settings(dc: &dc, stream: &crate::dc_stream::dc_stream_state,
                                      dp_tunnel_setting: &mut crate::dc_state::dc_tunnel_settings) {
    let link = &dc.links[stream.link];

    *dp_tunnel_setting = Default::default();

    if stream.signal == LF_SIGNAL_TYPE_DISPLAY_PORT || stream.signal == LF_SIGNAL_TYPE_DISPLAY_PORT_MST {
        let tun = &link.dpcd_caps.usb4_dp_tun_info;
        dp_tunnel_setting.should_enable_dp_tunneling = dp_tun_cap_dp_tunneling(tun.dp_tun_cap) != 0;

        if dp_tun_cap_dpia_bw_alloc(tun.dp_tun_cap) != 0
            && usb4_driver_bw_cap_driver_bw_alloc_support(tun.driver_bw_cap) != 0
        {
            dp_tunnel_setting.should_use_dp_bw_allocation = true;
            dp_tunnel_setting.cm_id = tun.usb4_driver_id & 0x0F;
            // dpia_tunnel_info.bits.group_id
            dp_tunnel_setting.group_id = tun.dpia_tunnel_info & 0x7;
            dp_tunnel_setting.estimated_bw = link.dpia_bw_alloc_config.estimated_bw;
            dp_tunnel_setting.allocated_bw = link.dpia_bw_alloc_config.allocated_bw;
            dp_tunnel_setting.bw_granularity = link.dpia_bw_alloc_config.bw_granularity;
        }
    }
}

/// `dp_is_128b_132b_signal`: a pipe on an HPO DP encoder; DCN 2.1 has none.
pub fn dp_is_128b_132b_signal(_dc: &dc, _context: &crate::dc_state::dc_state, _pipe: usize) -> bool {
    false
}
