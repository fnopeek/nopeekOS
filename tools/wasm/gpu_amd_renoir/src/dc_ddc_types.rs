//! display/dc/dc_ddc_types.h + the AUX return codes of dmub_cmd.h (Linux
//! 6.18.55): the requests DC's AUX and I2C engines carry. A payload's
//! `data` is the caller's buffer; `reply` is optional as Linux's may be NULL.
#![allow(dead_code)]

/// `enum aux_transaction_type`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum aux_transaction_type {
    #[default]
    AUX_TRANSACTION_TYPE_DP,
    AUX_TRANSACTION_TYPE_I2C,
}

/// `enum i2caux_transaction_action`.
pub const I2CAUX_TRANSACTION_ACTION_I2C_WRITE: u32 = 0x00;
pub const I2CAUX_TRANSACTION_ACTION_I2C_READ: u32 = 0x10;
pub const I2CAUX_TRANSACTION_ACTION_I2C_STATUS_REQUEST: u32 = 0x20;
pub const I2CAUX_TRANSACTION_ACTION_I2C_WRITE_MOT: u32 = 0x40;
pub const I2CAUX_TRANSACTION_ACTION_I2C_READ_MOT: u32 = 0x50;
pub const I2CAUX_TRANSACTION_ACTION_I2C_STATUS_REQUEST_MOT: u32 = 0x60;
pub const I2CAUX_TRANSACTION_ACTION_DP_WRITE: u32 = 0x80;
pub const I2CAUX_TRANSACTION_ACTION_DP_READ: u32 = 0x90;

/// `struct aux_request_transaction_data`.
pub struct aux_request_transaction_data<'a> {
    pub r#type: aux_transaction_type,
    pub action: u32,
    /// 20-bit AUX channel transaction address
    pub address: u32,
    /// delay, in 100-microsecond units
    pub delay: u8,
    pub length: u32,
    pub data: &'a [u8],
}

/// `enum aux_transaction_reply`.
pub const AUX_TRANSACTION_REPLY_AUX_ACK: u8 = 0x00;
pub const AUX_TRANSACTION_REPLY_AUX_NACK: u8 = 0x01;
pub const AUX_TRANSACTION_REPLY_AUX_DEFER: u8 = 0x02;
pub const AUX_TRANSACTION_REPLY_I2C_OVER_AUX_NACK: u8 = 0x04;
pub const AUX_TRANSACTION_REPLY_I2C_OVER_AUX_DEFER: u8 = 0x08;
pub const AUX_TRANSACTION_REPLY_I2C_ACK: u8 = 0x00;
pub const AUX_TRANSACTION_REPLY_I2C_NACK: u8 = 0x10;
pub const AUX_TRANSACTION_REPLY_I2C_DEFER: u8 = 0x20;
pub const AUX_TRANSACTION_REPLY_HPD_DISCON: u8 = 0x40;
pub const AUX_TRANSACTION_REPLY_INVALID: u8 = 0xFF;

/// `struct aux_payload`.
pub struct aux_payload<'a> {
    /// set following flag to read/write I2C data,
    /// reset it to read/write DPCD data
    pub i2c_over_aux: bool,
    /// set following flag to write data,
    /// reset it to read data
    pub write: bool,
    pub mot: bool,
    pub write_status_update: bool,
    pub address: u32,
    pub length: u32,
    pub data: &'a mut [u8],
    /// used to return the reply type of the transaction
    /// ignored if None
    pub reply: Option<&'a mut u8>,
    /// expressed in milliseconds
    /// zero means "use default value"
    pub defer_delay: u32,
}

pub const DEFAULT_AUX_MAX_DATA_SIZE: u32 = 16;

/// `struct i2c_payload`.
pub struct i2c_payload<'a> {
    pub write: bool,
    pub address: u8,
    pub length: u32,
    pub data: &'a mut [u8],
}

/// `enum i2c_command_engine`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum i2c_command_engine {
    I2C_COMMAND_ENGINE_DEFAULT,
    I2C_COMMAND_ENGINE_SW,
    I2C_COMMAND_ENGINE_HW,
}

/// `struct i2c_command`.
pub struct i2c_command<'a, 'b> {
    pub payloads: &'b mut [i2c_payload<'a>],
    pub number_of_payloads: u8,
    pub engine: i2c_command_engine,
    /// expressed in KHz
    /// zero means "use default value"
    pub speed: u32,
}

/// `enum aux_return_code_type` (dmub_cmd.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum aux_return_code_type {
    #[default]
    AUX_RET_SUCCESS = 0,
    AUX_RET_ERROR_UNKNOWN,
    AUX_RET_ERROR_INVALID_REPLY,
    AUX_RET_ERROR_TIMEOUT,
    AUX_RET_ERROR_HPD_DISCON,
    AUX_RET_ERROR_ENGINE_ACQUIRE,
    AUX_RET_ERROR_INVALID_OPERATION,
    AUX_RET_ERROR_PROTOCOL_ERROR,
}

/// `enum ddc_transaction_type` (ddc_service_types.h... dc_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ddc_transaction_type {
    #[default]
    DDC_TRANSACTION_TYPE_NONE = 0,
    DDC_TRANSACTION_TYPE_I2C,
    DDC_TRANSACTION_TYPE_I2C_OVER_AUX,
    DDC_TRANSACTION_TYPE_I2C_OVER_AUX_WITH_DEFER,
    DDC_TRANSACTION_TYPE_I2C_OVER_AUX_RETRY_DEFER,
}

/// `enum display_dongle_type`.
pub const DISPLAY_DONGLE_NONE: u32 = 0;
pub const DISPLAY_DONGLE_DP_VGA_CONVERTER: u32 = 1;
pub const DISPLAY_DONGLE_DP_DVI_CONVERTER: u32 = 2;
pub const DISPLAY_DONGLE_DP_HDMI_CONVERTER: u32 = 3;
pub const DISPLAY_DONGLE_DP_DVI_DONGLE: u32 = 4;
pub const DISPLAY_DONGLE_DP_HDMI_DONGLE: u32 = 5;
pub const DISPLAY_DONGLE_DP_HDMI_MISMATCHED_DONGLE: u32 = 6;
