//! display/dc/link/protocols/link_ddc.c (Linux 6.18.55): the DDC service of
//! a link, the construct path. `link` is a back pointer in Linux; the port
//! keeps whether one was given.
#![allow(dead_code)]

use crate::bios_parser2::{bios_parser, bios_parser_get_i2c_info, bp_result, graphics_object_i2c_info};
use crate::dc_types::{graphics_object_id, object_type};
use crate::gpio_service::{dal_gpio_create_ddc, ddc, gpio_ddc_hw_info, gpio_service};
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
