//! display/dc/gpio/gpio_service.c + hw_translate.c + hw_factory.c +
//! dcn21/hw_translate_dcn21.c + dcn21/hw_factory_dcn21.c (Linux 6.18.55):
//! the GPIO service `dc_construct` creates. Creation binds the DCN 2.1
//! translate and factory tables and allocates the busyness storage; the pin
//! register tables join with their first user (the link's DDC/HPD pins).
#![allow(dead_code)]

use crate::dc_types::{dce_environment, dce_version};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// `enum gpio_id` (include/gpio_types.h). `GPIO_ID_UNKNOWN` (-1) is left
/// out: the port names it with `None`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum gpio_id {
    GPIO_ID_DDC_DATA = 0,
    GPIO_ID_DDC_CLOCK,
    GPIO_ID_GENERIC,
    GPIO_ID_HPD,
    GPIO_ID_GPIO_PAD,
    GPIO_ID_VIP_PAD,
    GPIO_ID_SYNC,
    GPIO_ID_GSL,
}

pub const GPIO_ID_COUNT: usize = 8;

/// Which `hw_translate_funcs` table is bound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum hw_translate_funcs_id {
    dcn21,
}

/// `struct hw_translate` (hw_translate.h).
#[derive(Default)]
pub struct hw_translate {
    pub funcs: Option<hw_translate_funcs_id>,
}

/// Which `hw_factory_funcs` table is bound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum hw_factory_funcs_id {
    dcn21,
}

/// `struct hw_factory` (hw_factory.h).
#[derive(Default)]
pub struct hw_factory {
    pub number_of_pins: [u32; GPIO_ID_COUNT],
    pub funcs: Option<hw_factory_funcs_id>,
}

/// `struct gpio_service` (gpio_service.h). `busyness[id]` is a byte per pin,
/// None where the factory has no pins of that kind.
#[derive(Default)]
pub struct gpio_service {
    pub translate: hw_translate,
    pub factory: hw_factory,
    pub busyness: [Option<Vec<u8>>; GPIO_ID_COUNT],
}

/// `dal_hw_translate_dcn21_init` (hw_translate_dcn21.c).
fn dal_hw_translate_dcn21_init(tr: &mut hw_translate) {
    tr.funcs = Some(hw_translate_funcs_id::dcn21);
}

/// `dal_hw_translate_init` (hw_translate.c).
fn dal_hw_translate_init(translate: &mut hw_translate, dce_version: dce_version, _env: dce_environment) -> bool {
    match dce_version {
        dce_version::DCN_VERSION_2_1 => {
            dal_hw_translate_dcn21_init(translate);
            true
        }
        _ => false,
    }
}

/// `dal_hw_factory_dcn21_init` (hw_factory_dcn21.c).
fn dal_hw_factory_dcn21_init(factory: &mut hw_factory) {
    factory.number_of_pins[gpio_id::GPIO_ID_DDC_DATA as usize] = 8;
    factory.number_of_pins[gpio_id::GPIO_ID_DDC_CLOCK as usize] = 8;
    factory.number_of_pins[gpio_id::GPIO_ID_GENERIC as usize] = 4;
    factory.number_of_pins[gpio_id::GPIO_ID_HPD as usize] = 6;
    factory.number_of_pins[gpio_id::GPIO_ID_GPIO_PAD as usize] = 28;
    factory.number_of_pins[gpio_id::GPIO_ID_VIP_PAD as usize] = 0;
    factory.number_of_pins[gpio_id::GPIO_ID_SYNC as usize] = 0;
    factory.number_of_pins[gpio_id::GPIO_ID_GSL as usize] = 0;

    factory.funcs = Some(hw_factory_funcs_id::dcn21);
}

/// `dal_hw_factory_init` (hw_factory.c).
fn dal_hw_factory_init(factory: &mut hw_factory, dce_version: dce_version, _env: dce_environment) -> bool {
    match dce_version {
        dce_version::DCN_VERSION_2_1 => {
            dal_hw_factory_dcn21_init(factory);
            true
        }
        _ => false,
    }
}

/// `dal_gpio_service_create` (gpio_service.c).
pub fn dal_gpio_service_create(dce_version: dce_version, dce_environment: dce_environment) -> Option<Box<gpio_service>> {
    let mut service = Box::new(gpio_service::default());

    if !dal_hw_translate_init(&mut service.translate, dce_version, dce_environment) {
        return None;
    }

    if !dal_hw_factory_init(&mut service.factory, dce_version, dce_environment) {
        return None;
    }

    // allocate and initialize busyness storage
    for index_of_id in 0..GPIO_ID_COUNT {
        let number_of_bits = service.factory.number_of_pins[index_of_id];
        service.busyness[index_of_id] = if number_of_bits != 0 {
            Some(alloc::vec![0u8; number_of_bits as usize])
        } else {
            None
        };
    }

    Some(service)
}
