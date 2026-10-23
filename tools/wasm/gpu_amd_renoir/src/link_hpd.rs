//! display/dc/link/protocols/link_hpd.c (Linux 6.18.55): the HPD GPIO of a
//! connector, from its VBIOS HPD record and GPIO pin LUT entry.
#![allow(dead_code)]

use crate::bios_parser2::{bios_parser, bios_parser_get_gpio_pin_info, bios_parser_get_hpd_info, bp_result,
    gpio_pin_info, graphics_object_hpd_info};
use crate::dc_types::graphics_object_id;
use crate::gpio_base::gpio;
use crate::gpio_service::{dal_gpio_service_create_irq, gpio_service};
use alloc::boxed::Box;

/// `link_get_hpd_gpio` (link_hpd.c).
pub fn link_get_hpd_gpio(dcb: &bios_parser, bios: &[u8], link_id: graphics_object_id,
                         gpio_service: &gpio_service) -> Option<Box<gpio>> {
    let mut hpd_info = graphics_object_hpd_info::default();
    let mut pin_info = gpio_pin_info::default();

    if bios_parser_get_hpd_info(dcb, bios, link_id, &mut hpd_info) != bp_result::BP_RESULT_OK {
        return None;
    }

    if bios_parser_get_gpio_pin_info(dcb, bios, hpd_info.hpd_int_gpio_uid as u32, &mut pin_info)
        != bp_result::BP_RESULT_OK
    {
        return None;
    }

    dal_gpio_service_create_irq(gpio_service, pin_info.offset, pin_info.mask)
}
