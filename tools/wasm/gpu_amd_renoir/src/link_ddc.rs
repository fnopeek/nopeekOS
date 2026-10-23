//! display/dc/link/protocols/link_ddc.c (Linux 6.18.55): the DDC service of
//! a link, the construct path. `link` is a back pointer in Linux; the port
//! keeps whether one was given.
#![allow(dead_code)]

use crate::bios_parser2::{bios_parser, bios_parser_get_i2c_info, bp_result, graphics_object_i2c_info};
use crate::dc_types::{graphics_object_id, object_type};
use crate::gpio_service::{dal_gpio_create_ddc, ddc, gpio_ddc_hw_info, gpio_service};
use crate::amdgpu_device::amdgpu_device;
use crate::dc::dc;
use crate::dc_ddc_types::{aux_payload, aux_return_code_type, ddc_transaction_type};
use crate::dc_types::dce_version;
use crate::dce_aux::dce_aux_transfer_raw;
use crate::regs::*;
use alloc::boxed::Box;

/// `CONNECTOR_ID_EDP` / `CONNECTOR_ID_LVDS` (grph_object_id.h).
pub const CONNECTOR_ID_LVDS: u32 = 14;
pub const CONNECTOR_ID_EDP: u32 = 20;

/// `struct ddc_flags` (link_ddc.h... dc_ddc_types.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct ddc_flags {
    pub EDID_QUERY_DONE_ONCE: bool,
    pub IS_INTERNAL_DISPLAY: bool,
    pub FORCE_READ_REPEATED_START: bool,
    pub EDID_STRESS_READ: bool,
}

/// `struct ddc_service` (dc_ddc_types.h), the construct's members.
#[derive(Debug, Default)]
pub struct ddc_service {
    pub ddc_pin: Option<Box<ddc>>,
    pub flags: ddc_flags,
    pub wa: u32,
    pub transaction_type: ddc_transaction_type,
    /// `enum display_dongle_type`.
    pub dongle_type: u32,
    pub has_link: bool,
}

/// `struct ddc_service_init_data` (dc_ddc_types.h).
pub struct ddc_service_init_data {
    pub id: graphics_object_id,
    pub has_link: bool,
    pub is_dpia_link: bool,
}

/// `ddc_service_construct` (link_ddc.c).
fn ddc_service_construct(ddc_service: &mut ddc_service, dcb: &bios_parser, bios: &[u8],
                         gpio_service: &gpio_service, init_data: &ddc_service_init_data) {
    // dal_graphics_object_id_get_connector_id: UNKNOWN unless a connector
    let connector_id = if init_data.id.r#type == object_type::OBJECT_TYPE_CONNECTOR { init_data.id.id } else { 0 };
    let mut i2c_info = graphics_object_i2c_info::default();

    ddc_service.has_link = init_data.has_link;

    if init_data.is_dpia_link
        || bios_parser_get_i2c_info(dcb, bios, init_data.id, &mut i2c_info) != bp_result::BP_RESULT_OK
    {
        ddc_service.ddc_pin = None;
    } else {
        let hw_info = gpio_ddc_hw_info {
            ddc_channel: i2c_info.i2c_line,
            hw_supported: if init_data.has_link { i2c_info.i2c_hw_assist } else { false },
        };

        ddc_service.ddc_pin = dal_gpio_create_ddc(
            gpio_service,
            i2c_info.gpio_info.clk_a_register_index,
            1u32.wrapping_shl(i2c_info.gpio_info.clk_a_shift),
            &hw_info,
        );
    }

    ddc_service.flags.EDID_QUERY_DONE_ONCE = false;
    ddc_service.flags.FORCE_READ_REPEATED_START = false;
    ddc_service.flags.EDID_STRESS_READ = false;

    ddc_service.flags.IS_INTERNAL_DISPLAY = connector_id == CONNECTOR_ID_EDP || connector_id == CONNECTOR_ID_LVDS;

    ddc_service.wa = 0;
}

/// `link_create_ddc_service` (link_ddc.c).
pub fn link_create_ddc_service(dcb: &bios_parser, bios: &[u8], gpio_service: &gpio_service,
                               init_data: &ddc_service_init_data) -> Box<ddc_service> {
    let mut ddc_service = Box::new(ddc_service::default());
    ddc_service_construct(&mut ddc_service, dcb, bios, gpio_service, init_data);
    ddc_service
}

/// `dal_ddc_get_line` (gpio_service.c).
pub fn dal_ddc_get_line(ddc: &ddc) -> u32 {
    ddc.pin_data.en
}

/// `set_ddc_transaction_type` (link_ddc.c).
pub fn set_ddc_transaction_type(ddc: &mut ddc_service, r#type: ddc_transaction_type) {
    ddc.transaction_type = r#type;
}

/// `link_is_in_aux_transaction_mode` (link_ddc.c).
pub fn link_is_in_aux_transaction_mode(ddc: &ddc_service) -> bool {
    matches!(
        ddc.transaction_type,
        ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C_OVER_AUX
            | ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C_OVER_AUX_WITH_DEFER
            | ddc_transaction_type::DDC_TRANSACTION_TYPE_I2C_OVER_AUX_RETRY_DEFER
    )
}

/// `set_dongle_type` (link_ddc.c).
pub fn set_dongle_type(ddc: &mut ddc_service, dongle_type: u32) {
    ddc.dongle_type = dongle_type;
}

/// `link_aux_transfer_raw` (link_ddc.c) on link `li`'s DDC service. The
/// DMUB path (`enable_dmub_aux_for_legacy_ddc`, or a link without a pin)
/// is not taken on DCN 2.1: debug_defaults_drv leaves the option off and
/// every PHY link has its pin.
pub fn link_aux_transfer_raw(adev: &amdgpu_device, dc: &mut dc, li: usize, payload: &mut aux_payload,
                             operation_result: &mut aux_return_code_type) -> i32 {
    let has_pin = dc.links[li].ddc.as_deref().is_some_and(|d| d.ddc_pin.is_some());
    if dc.debug.enable_dmub_aux_for_legacy_ddc || !has_pin {
        crate::linux::Line::new().s("link_aux_transfer_raw: dce_aux_transfer_dmub_raw not ported").say();
        *operation_result = aux_return_code_type::AUX_RET_ERROR_UNKNOWN;
        return -1;
    }
    let ddc_pin = dc.links[li].ddc.as_deref_mut().and_then(|d| d.ddc_pin.as_deref_mut());
    let Some(gs) = dc.ctx.gpio_service.as_deref_mut() else { return -1 };
    let engines = &mut dc.res_pool.as_deref_mut().unwrap().engines;
    dce_aux_transfer_raw(adev, gs, engines, ddc_pin, payload, operation_result)
}

/// `try_to_configure_aux_timeout` (link_ddc.c).
pub fn try_to_configure_aux_timeout(adev: &amdgpu_device, dc: &mut dc, li: usize, mut timeout: u32) -> bool {
    let mut result = false;

    if (dc.links[li].chip_caps as u32 & LF_AMD_EXT_DISPLAY_PATH_CAPS__EXT_CHIP_MASK)
        == LF_AMD_EXT_DISPLAY_PATH_CAPS__DP_FIXED_VS_EN
        && !dc.debug.disable_fixed_vs_aux_timeout_wa
        && dc.ctx.dce_version == dce_version::DCN_VERSION_3_1
    {
        // Fixed VS workaround for AUX timeout
        let fixed_vs_address = 0xF004F;
        let fixed_vs_data: [u8; 4] = [0x1, 0x22, 0x63, 0xc];

        crate::link_dpcd::core_link_write_dpcd(adev, dc, li, fixed_vs_address, &fixed_vs_data);

        timeout = 3072;
    }

    // Do not try to access nonexistent DDC pin.
    if dc.links[li].ep_type != LF_DISPLAY_ENDPOINT_PHY {
        return true;
    }

    let Some(en) = dc.links[li].ddc.as_deref().and_then(|d| d.ddc_pin.as_deref()).map(|p| p.pin_data.en) else {
        return false;
    };
    let engine = dc.res_pool.as_deref_mut().unwrap().engines[en as usize].as_deref_mut().unwrap();
    if let Some(f) = engine.base.configure_timeout {
        f(adev, engine, timeout);
        result = true;
    }

    result
}
