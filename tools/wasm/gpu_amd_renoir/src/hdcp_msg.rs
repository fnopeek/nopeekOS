//! display/dc/hdcp/hdcp_msg.c (Linux 6.18.55): an HDCP message to a sink,
//! over DPCD on DisplayPort and eDP. The HDMI/DVI path goes over the I2C
//! engines (`hdmi_14_process_transaction` → `dm_helpers_submit_i2c`) and
//! is not yet ported. The message tables are Linux's statics (tables.rs).
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_ddc_types::DEFAULT_AUX_MAX_DATA_SIZE;
use crate::dc_dp_types::*;
use crate::dc_types::dc_status;
use crate::link_dpcd::{core_link_read_dpcd, core_link_write_dpcd};
use crate::linux::Line;
use crate::regs::*;

const HDCP14_KSV_SIZE: u32 = 5;
const HDCP14_MAX_KSV_FIFO_SIZE: u32 = 127 * HDCP14_KSV_SIZE;

/// `enum hdcp_message_status`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum hdcp_message_status {
    HDCP_MESSAGE_SUCCESS,
    HDCP_MESSAGE_FAILURE,
    HDCP_MESSAGE_UNSUPPORTED,
}

/// `struct hdcp_protection_message`; `data` is the caller's buffer.
pub struct hdcp_protection_message<'a> {
    /// `enum hdcp_version`.
    pub version: u32,
    /// `enum hdcp_message_id`.
    pub msg_id: u32,
    pub length: u32,
    pub max_retries: u8,
    pub data: &'a mut [u8],
    /// `enum hdcp_link`.
    pub link: u32,
}

/// `struct protection_properties`: which transaction a message takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum protection_properties {
    non_supported_protection,
    hdmi_14_protection,
    dp_11_protection,
}

/// `dpcd_access_helper`.
fn dpcd_access_helper(adev: &amdgpu_device, dc: &mut dc, li: usize, mut length: u32, data: &mut [u8], dpcd_addr: u32,
                      is_read: bool) -> bool {
    let mut offset = 0u32;
    let ksv_read_size = 0x6803b - 0x6802c;

    // Read KSV, need repeatedly handle
    if dpcd_addr == 0x6802c {
        if length % HDCP14_KSV_SIZE != 0 {
            Line::new().s("dpcd_access_helper: KsvFifo Size is not a multiple of HDCP14_KSV_SIZE").say();
        }
        if length > HDCP14_MAX_KSV_FIFO_SIZE {
            Line::new().s("dpcd_access_helper: KsvFifo Size is greater than HDCP14_MAX_KSV_FIFO_SIZE").say();
        }

        Line::new().s("dpcd_access_helper: Reading ").dec((length / HDCP14_KSV_SIZE) as u64)
            .s(" Ksv(s) from KsvFifo").say();

        // Linux advances `data` and indexes it with `offset` as well; offset
        // stays 0 here, so `data` alone moves.
        let mut d = 0usize;
        while length > 0 {
            let n = if length > ksv_read_size { ksv_read_size } else { length };
            let status = core_link_read_dpcd(adev, dc, li, dpcd_addr + offset,
                &mut data[d + offset as usize..d + offset as usize + n as usize]);

            d += n as usize;
            length -= n;

            if status != dc_status::DC_OK {
                return false;
            }
        }
    } else {
        while length > 0 {
            let cur_length = if length > DEFAULT_AUX_MAX_DATA_SIZE { DEFAULT_AUX_MAX_DATA_SIZE } else { length };
            let buf = &mut data[offset as usize..(offset + cur_length) as usize];

            let status = if is_read {
                core_link_read_dpcd(adev, dc, li, dpcd_addr + offset, buf)
            } else {
                core_link_write_dpcd(adev, dc, li, dpcd_addr + offset, buf)
            };

            if status != dc_status::DC_OK {
                return false;
            }

            length -= cur_length;
            offset += cur_length;
        }
    }
    true
}

/// `dp_11_process_transaction`.
fn dp_11_process_transaction(adev: &amdgpu_device, dc: &mut dc, li: usize, message_info: &mut hdcp_protection_message)
    -> bool {
    if message_info.msg_id == HDCP_MESSAGE_ID_INVALID as u32 {
        Line::new().s("dp_11_process_transaction: Invalid message_info msg_id").say();
        return false;
    }

    let id = message_info.msg_id as usize;
    dpcd_access_helper(adev, dc, li, message_info.length, message_info.data, crate::tables::hdcp_dpcd_addrs[id],
        crate::tables::hdcp_cmd_is_read[id] != 0)
}

/// `get_protection_properties_by_signal`.
fn get_protection_properties_by_signal(dc: &dc, li: usize, st: u32, version: u32) -> protection_properties {
    use protection_properties::*;
    match version {
        HDCP_VERSION_14 => match st {
            LF_SIGNAL_TYPE_DVI_SINGLE_LINK | LF_SIGNAL_TYPE_DVI_DUAL_LINK | LF_SIGNAL_TYPE_HDMI_TYPE_A => {
                hdmi_14_protection
            }
            LF_SIGNAL_TYPE_DISPLAY_PORT => {
                let caps = &dc.links[li].dpcd_caps;
                if caps.dongle_type == crate::dc_ddc_types::DISPLAY_DONGLE_DP_VGA_CONVERTER
                    || caps.dongle_caps.dongle_type == crate::dc_ddc_types::DISPLAY_DONGLE_DP_VGA_CONVERTER
                {
                    return non_supported_protection;
                }
                dp_11_protection
            }
            LF_SIGNAL_TYPE_DISPLAY_PORT_MST | LF_SIGNAL_TYPE_EDP => dp_11_protection,
            _ => non_supported_protection,
        },
        HDCP_VERSION_22 => match st {
            LF_SIGNAL_TYPE_DVI_SINGLE_LINK | LF_SIGNAL_TYPE_DVI_DUAL_LINK | LF_SIGNAL_TYPE_HDMI_TYPE_A => {
                hdmi_14_protection // todo version2.2
            }
            LF_SIGNAL_TYPE_DISPLAY_PORT | LF_SIGNAL_TYPE_DISPLAY_PORT_MST | LF_SIGNAL_TYPE_EDP => {
                dp_11_protection // todo version2.2
            }
            _ => non_supported_protection,
        },
        _ => non_supported_protection,
    }
}

/// `protection_props->process_transaction`.
fn process_transaction(p: protection_properties, adev: &amdgpu_device, dc: &mut dc, li: usize,
                       message_info: &mut hdcp_protection_message) -> bool {
    match p {
        protection_properties::dp_11_protection => dp_11_process_transaction(adev, dc, li, message_info),
        protection_properties::hdmi_14_protection => {
            Line::new().s("dc_process_hdcp_msg: hdmi_14_process_transaction not ported").say();
            false
        }
        protection_properties::non_supported_protection => false,
    }
}

/// `dc_process_hdcp_msg`.
pub fn dc_process_hdcp_msg(adev: &amdgpu_device, dc: &mut dc, signal: u32, li: usize,
                           message_info: &mut hdcp_protection_message) -> hdcp_message_status {
    let mut status = hdcp_message_status::HDCP_MESSAGE_FAILURE;

    if message_info.msg_id < HDCP_MESSAGE_ID_READ_BKSV as u32 || message_info.msg_id >= HDCP_MESSAGE_ID_MAX as u32 {
        return hdcp_message_status::HDCP_MESSAGE_UNSUPPORTED;
    }

    let protection_props = get_protection_properties_by_signal(dc, li, signal, message_info.version);

    if protection_props == protection_properties::non_supported_protection {
        return hdcp_message_status::HDCP_MESSAGE_UNSUPPORTED;
    }

    if process_transaction(protection_props, adev, dc, li, message_info) {
        status = hdcp_message_status::HDCP_MESSAGE_SUCCESS;
    } else {
        for _ in 0..message_info.max_retries {
            if process_transaction(protection_props, adev, dc, li, message_info) {
                status = hdcp_message_status::HDCP_MESSAGE_SUCCESS;
                break;
            }
        }
    }

    status
}

/// `query_hdcp_capability` (link_detection.c).
pub fn query_hdcp_capability(adev: &amdgpu_device, dc: &mut dc, signal: u32, li: usize) {
    let mut caps = hdcp_caps::default();

    let rx_full = (dc.links[li].connector_signal == LF_SIGNAL_TYPE_DISPLAY_PORT
        && dc.links[li].ddc.as_deref().unwrap().transaction_type
            == crate::dc_ddc_types::ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C_OVER_AUX)
        || dc.links[li].connector_signal == LF_SIGNAL_TYPE_EDP;
    {
        let (data, length, msg_id): (&mut [u8], u32, u32) = if rx_full {
            (&mut caps.rx_caps[..], 3, HDCP_MESSAGE_ID_RX_CAPS as u32)
        } else {
            (&mut caps.rx_caps[..1], 1, HDCP_MESSAGE_ID_HDCP2VERSION as u32)
        };
        let mut msg22 = hdcp_protection_message {
            version: HDCP_VERSION_22,
            msg_id,
            length,
            max_retries: 5,
            data,
            link: HDCP_LINK_PRIMARY,
        };
        dc_process_hdcp_msg(adev, dc, signal, li, &mut msg22);
    }

    if signal == LF_SIGNAL_TYPE_DISPLAY_PORT || signal == LF_SIGNAL_TYPE_DISPLAY_PORT_MST {
        let mut bcaps = [0u8; 1];
        let mut msg14 = hdcp_protection_message {
            data: &mut bcaps,
            length: 1,
            msg_id: HDCP_MESSAGE_ID_READ_BCAPS as u32,
            version: HDCP_VERSION_14,
            link: HDCP_LINK_PRIMARY,
            max_retries: 5,
        };

        dc_process_hdcp_msg(adev, dc, signal, li, &mut msg14);
        caps.bcaps = bcaps[0];
    } else {
        caps.bcaps = dc.links[li].hdcp_caps.bcaps;
    }

    dc.links[li].hdcp_caps = caps;
}
