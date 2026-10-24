//! display/dc/gpio/gpio_service.c + hw_translate.c + hw_factory.c +
//! dcn21/hw_translate_dcn21.c + dcn21/hw_factory_dcn21.c (Linux 6.18.55):
//! the GPIO service `dc_construct` creates and the pin plumbing behind it:
//! busyness, open/close, the DCN 2.1 offset/id translation and the register
//! tables of hw_factory_dcn21.c (read out of Linux's statics into tables.rs).
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc_types::{dce_environment, dce_version};
use crate::gpio_base::*;
use crate::regs::*;
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

/// `is_pin_busy` / `set_pin_busy` / `set_pin_free` (gpio_service.c).
fn is_pin_busy(service: &gpio_service, id: Option<gpio_id>, en: u32) -> bool {
    let Some(id) = id else { return false };
    service.busyness[id as usize].as_ref().is_some_and(|b| b.get(en as usize).is_some_and(|&x| x != 0))
}

fn set_pin(service: &mut gpio_service, id: Option<gpio_id>, en: u32, busy: bool) {
    let Some(id) = id else { return };
    if let Some(b) = service.busyness[id as usize].as_mut().and_then(|b| b.get_mut(en as usize)) {
        *b = busy as u8;
    }
}

/// `dal_gpio_service_lock` (gpio_service.c).
pub fn dal_gpio_service_lock(service: &mut gpio_service, id: Option<gpio_id>, en: u32) -> gpio_result {
    if let Some(i) = id {
        if service.busyness[i as usize].is_none() {
            return gpio_result::GPIO_RESULT_OPEN_FAILED;
        }
    }
    set_pin(service, id, en, true);
    gpio_result::GPIO_RESULT_OK
}

/// `dal_gpio_service_unlock` (gpio_service.c).
pub fn dal_gpio_service_unlock(service: &mut gpio_service, id: Option<gpio_id>, en: u32) -> gpio_result {
    if let Some(i) = id {
        if service.busyness[i as usize].is_none() {
            return gpio_result::GPIO_RESULT_OPEN_FAILED;
        }
    }
    set_pin(service, id, en, false);
    gpio_result::GPIO_RESULT_OK
}

/// `define_hpd_registers` (hw_factory_dcn21.c): `hpd_regs[en]` and the
/// shared shift/mask. The table has five entries; a sixth HPD would read
/// past it in Linux, and stays without registers here.
fn define_hpd_registers(hpd: &mut hw_hpd, en: u32) {
    const N: usize = 14;
    let regs = crate::tables::hpd_regs.get(en as usize * N..en as usize * N + N).map(|v| hpd_registers {
        gpio: gpio_registers::from(&v[..12]),
        int_status: v[12],
        toggle_filt_cntl: v[13],
    });
    hpd.regs = regs;
    let sm = |t: &[u32]| hpd_sh_mask {
        DC_HPD_SENSE_DELAYED: t[0],
        DC_HPD_SENSE: t[1],
        DC_HPD_CONNECT_INT_DELAY: t[2],
        DC_HPD_DISCONNECT_INT_DELAY: t[3],
    };
    hpd.shifts = sm(&crate::tables::hpd_shift);
    hpd.masks = sm(&crate::tables::hpd_mask);
    hpd.base.regs = regs.map(|r| r.gpio);
}

/// `define_ddc_registers` (hw_factory_dcn21.c).
fn define_ddc_registers(ddc: &mut hw_ddc, en: u32) {
    const N: usize = 15;
    const M: usize = 10;
    let table: &[u32] = match ddc.base.id {
        Some(gpio_id::GPIO_ID_DDC_DATA) => &crate::tables::ddc_data_regs_dcn,
        Some(gpio_id::GPIO_ID_DDC_CLOCK) => &crate::tables::ddc_clk_regs_dcn,
        _ => return,
    };
    let regs = table.get(en as usize * N..en as usize * N + N).map(|v| ddc_registers {
        gpio: gpio_registers::from(&v[..12]),
        ddc_setup: v[12],
        phy_aux_cntl: v[13],
        dc_gpio_aux_ctrl_5: v[14],
    });
    ddc.regs = regs;
    ddc.base.regs = regs.map(|r| r.gpio);
    let sm = |t: &[u32]| {
        t.get(en as usize * M..en as usize * M + M).map(|t| ddc_sh_mask {
            DC_I2C_DDC1_ENABLE: t[0],
            DC_I2C_DDC1_EDID_DETECT_ENABLE: t[1],
            DC_I2C_DDC1_EDID_DETECT_MODE: t[2],
            DC_GPIO_DDC1DATA_PD_EN: t[3],
            DC_GPIO_DDC1CLK_PD_EN: t[4],
            AUX_PAD1_MODE: t[5],
            DC_GPIO_SDA_PD_DIS: t[6],
            DC_GPIO_SCL_PD_DIS: t[7],
            AUX_PAD_RXSEL: t[8],
            DDC_PAD_I2CMODE: t[9],
        }).unwrap_or_default()
    };
    ddc.shifts = sm(&crate::tables::ddc_shift);
    ddc.masks = sm(&crate::tables::ddc_mask);
}

/// `define_generic_registers` (hw_factory_dcn21.c): one generic block (A).
fn define_generic_registers(generic: &mut hw_generic, en: u32) {
    const N: usize = 13;
    let regs = crate::tables::generic_regs.get(en as usize * N..en as usize * N + N).map(|v| generic_registers {
        gpio: gpio_registers::from(&v[..12]),
        mux: v[12],
    });
    generic.regs = regs;
    generic.base.regs = regs.map(|r| r.gpio);
    let sm = |t: &[u32]| {
        t.get(en as usize * 2..en as usize * 2 + 2)
            .map(|t| generic_sh_mask { GENERIC_EN: t[0], GENERIC_SEL: t[1] })
            .unwrap_or_default()
    };
    generic.shifts = sm(&crate::tables::generic_shift);
    generic.masks = sm(&crate::tables::generic_mask);
}

/// `dal_gpio_service_open` (gpio_service.c): take the pin, define its
/// registers, open it in `gpio.mode`.
pub fn dal_gpio_service_open(adev: &amdgpu_device, service: &mut gpio_service, g: &mut gpio) -> gpio_result {
    let (id, en, mode) = (g.id, g.en, g.mode);

    let Some(i) = id else { return gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR };
    if service.busyness[i as usize].is_none() {
        return gpio_result::GPIO_RESULT_OPEN_FAILED;
    }

    if is_pin_busy(service, id, en) {
        return gpio_result::GPIO_RESULT_DEVICE_BUSY;
    }

    match (&mut g.hw_container, i) {
        (hw_container::Ddc(d), gpio_id::GPIO_ID_DDC_DATA | gpio_id::GPIO_ID_DDC_CLOCK) => define_ddc_registers(d, en),
        (hw_container::Generic(x), gpio_id::GPIO_ID_GENERIC) => define_generic_registers(x, en),
        (hw_container::Hpd(h), gpio_id::GPIO_ID_HPD) => define_hpd_registers(h, en),
        (_, gpio_id::GPIO_ID_SYNC | gpio_id::GPIO_ID_GSL) => {}
        _ => return gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR,
    }

    let Some(pin) = g.hw_container.gpio() else { return gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR };
    g.pin_open = true;

    if !dal_hw_gpio_open(adev, pin, mode) {
        dal_gpio_service_close(adev, service, g);
        return gpio_result::GPIO_RESULT_OPEN_FAILED;
    }

    set_pin(service, id, en, true);
    gpio_result::GPIO_RESULT_OK
}

/// `dal_gpio_service_close` (gpio_service.c): free the pin and restore its
/// registers.
pub fn dal_gpio_service_close(adev: &amdgpu_device, service: &mut gpio_service, g: &mut gpio) {
    if !g.pin_open {
        return;
    }
    if let Some(pin) = g.hw_container.gpio() {
        let (id, en) = (pin.id, pin.en);
        set_pin(service, id, en, false);
        dal_hw_gpio_close(adev, pin);
    }
    g.pin_open = false;
}

/// `offset_to_id` (hw_translate_dcn21.c): the GPIO and line a VBIOS pin
/// (register A offset + mask) names. DDC lines leave `id` untouched.
fn offset_to_id(offset: u32, mask: u32, id: &mut Option<gpio_id>, en: &mut u32) -> bool {
    match offset {
        GPIOT_DC_GPIO_GENERIC_A => {
            *id = Some(gpio_id::GPIO_ID_GENERIC);
            *en = match mask {
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICA_A_MASK => 0,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICB_A_MASK => 1,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICC_A_MASK => 2,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICD_A_MASK => 3,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICE_A_MASK => 4,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICF_A_MASK => 5,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICG_A_MASK => 6,
                _ => return false,
            };
            true
        }
        GPIOT_DC_GPIO_HPD_A => {
            *id = Some(gpio_id::GPIO_ID_HPD);
            *en = match mask {
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD1_A_MASK => 0,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD2_A_MASK => 1,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD3_A_MASK => 2,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD4_A_MASK => 3,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD5_A_MASK => 4,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD6_A_MASK => 5,
                _ => return false,
            };
            true
        }
        GPIOT_DC_GPIO_GENLK_A => {
            *id = Some(gpio_id::GPIO_ID_GSL);
            *en = match mask {
                GPIOT_DC_GPIO_GENLK_A__DC_GPIO_GENLK_CLK_A_MASK => 0,
                GPIOT_DC_GPIO_GENLK_A__DC_GPIO_GENLK_VSYNC_A_MASK => 1,
                GPIOT_DC_GPIO_GENLK_A__DC_GPIO_SWAPLOCK_A_A_MASK => 2,
                GPIOT_DC_GPIO_GENLK_A__DC_GPIO_SWAPLOCK_B_A_MASK => 3,
                _ => return false,
            };
            true
        }
        // DDC: the id does not matter, the DDC handle uses DDC_DATA/CLOCK
        GPIOT_DC_GPIO_DDC1_A => { *en = GPIO_DDC_LINE_DDC1; true }
        GPIOT_DC_GPIO_DDC2_A => { *en = GPIO_DDC_LINE_DDC2; true }
        GPIOT_DC_GPIO_DDC3_A => { *en = GPIO_DDC_LINE_DDC3; true }
        GPIOT_DC_GPIO_DDC4_A => { *en = GPIO_DDC_LINE_DDC4; true }
        GPIOT_DC_GPIO_DDC5_A => { *en = GPIO_DDC_LINE_DDC5; true }
        GPIOT_DC_GPIO_DDCVGA_A => { *en = GPIO_DDC_LINE_DDC_VGA; true }
        _ => false,
    }
}

/// `id_to_offset` (hw_translate_dcn21.c).
pub fn id_to_offset(id: Option<gpio_id>, en: u32, info: &mut crate::bios_parser2::gpio_pin_info) -> bool {
    let ddc_line = |en: u32| match en {
        GPIO_DDC_LINE_DDC1 => Some(GPIOT_DC_GPIO_DDC1_A),
        GPIO_DDC_LINE_DDC2 => Some(GPIOT_DC_GPIO_DDC2_A),
        GPIO_DDC_LINE_DDC3 => Some(GPIOT_DC_GPIO_DDC3_A),
        GPIO_DDC_LINE_DDC4 => Some(GPIOT_DC_GPIO_DDC4_A),
        GPIO_DDC_LINE_DDC5 => Some(GPIOT_DC_GPIO_DDC5_A),
        GPIO_DDC_LINE_DDC_VGA => Some(GPIOT_DC_GPIO_DDCVGA_A),
        _ => None,
    };
    let result = match id {
        Some(gpio_id::GPIO_ID_DDC_DATA) => {
            info.mask = GPIOT_DC_GPIO_DDC5_A__DC_GPIO_DDC5DATA_A_MASK;
            ddc_line(en).map(|o| info.offset = o).is_some()
        }
        Some(gpio_id::GPIO_ID_DDC_CLOCK) => {
            info.mask = GPIOT_DC_GPIO_DDC5_A__DC_GPIO_DDC5CLK_A_MASK;
            ddc_line(en).map(|o| info.offset = o).is_some()
        }
        Some(gpio_id::GPIO_ID_GENERIC) => {
            info.offset = GPIOT_DC_GPIO_GENERIC_A;
            let m = [
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICA_A_MASK,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICB_A_MASK,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICC_A_MASK,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICD_A_MASK,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICE_A_MASK,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICF_A_MASK,
                GPIOT_DC_GPIO_GENERIC_A__DC_GPIO_GENERICG_A_MASK,
            ];
            m.get(en as usize).map(|&x| info.mask = x).is_some()
        }
        Some(gpio_id::GPIO_ID_HPD) => {
            info.offset = GPIOT_DC_GPIO_HPD_A;
            let m = [
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD1_A_MASK,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD2_A_MASK,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD3_A_MASK,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD4_A_MASK,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD5_A_MASK,
                GPIOT_DC_GPIO_HPD_A__DC_GPIO_HPD6_A_MASK,
            ];
            m.get(en as usize).map(|&x| info.mask = x).is_some()
        }
        // GSL is not implemented in Linux either
        _ => false,
    };
    if result {
        info.offset_y = info.offset + 2;
        info.offset_en = info.offset + 1;
        info.offset_mask = info.offset - 1;
        info.mask_y = info.mask;
        info.mask_en = info.mask;
        info.mask_mask = info.mask;
    }
    result
}

/// `dal_gpio_create_irq` (gpio_service.c): HPD and GPIO pad only.
pub fn dal_gpio_create_irq(id: Option<gpio_id>, en: u32) -> Option<Box<gpio>> {
    match id {
        Some(gpio_id::GPIO_ID_HPD | gpio_id::GPIO_ID_GPIO_PAD) => {}
        _ => return None,
    }
    Some(dal_gpio_create(id, en, gpio_pin_output_state::GPIO_PIN_OUTPUT_STATE_ACTIVE_LOW))
}

/// `dal_gpio_service_create_irq` (gpio_service.c).
pub fn dal_gpio_service_create_irq(service: &gpio_service, offset: u32, mask: u32) -> Option<Box<gpio>> {
    let mut id = Some(gpio_id::GPIO_ID_DDC_DATA);
    let mut en = 0;
    if service.translate.funcs.is_none() || !offset_to_id(offset, mask, &mut id, &mut en) {
        return None;
    }
    dal_gpio_create_irq(id, en)
}

/// `struct gpio_ddc_hw_info` (gpio_types.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct gpio_ddc_hw_info {
    pub hw_supported: bool,
    pub ddc_channel: u32,
}

/// `struct ddc` (gpio/ddc.h... gpio_service_interface).
#[derive(Debug)]
pub struct ddc {
    pub pin_data: Box<gpio>,
    pub pin_clock: Box<gpio>,
    pub hw_info: gpio_ddc_hw_info,
}

/// `dal_gpio_create_ddc` (gpio_service.c).
pub fn dal_gpio_create_ddc(service: &gpio_service, offset: u32, mask: u32, info: &gpio_ddc_hw_info) -> Option<Box<ddc>> {
    let mut id = None;
    let mut en = 0;
    if service.translate.funcs.is_none() || !offset_to_id(offset, mask, &mut id, &mut en) {
        return None;
    }

    Some(Box::new(ddc {
        pin_data: dal_gpio_create(Some(gpio_id::GPIO_ID_DDC_DATA), en, gpio_pin_output_state::GPIO_PIN_OUTPUT_STATE_ACTIVE_LOW),
        pin_clock: dal_gpio_create(Some(gpio_id::GPIO_ID_DDC_CLOCK), en, gpio_pin_output_state::GPIO_PIN_OUTPUT_STATE_ACTIVE_LOW),
        hw_info: *info,
    }))
}

/// `dal_irq_get_source` (gpio_service.c).
pub fn dal_irq_get_source(irq: &gpio) -> u32 {
    match irq.id {
        Some(gpio_id::GPIO_ID_HPD) => DC_IRQ_SOURCE_HPD1 + irq.en,
        Some(gpio_id::GPIO_ID_GPIO_PAD) => DC_IRQ_SOURCE_GPIOPAD0 + irq.en,
        _ => DC_IRQ_SOURCE_INVALID,
    }
}

/// `dal_irq_get_rx_source` (gpio_service.c).
pub fn dal_irq_get_rx_source(irq: &gpio) -> u32 {
    match irq.id {
        Some(gpio_id::GPIO_ID_HPD) => DC_IRQ_SOURCE_HPD1RX + irq.en,
        _ => DC_IRQ_SOURCE_INVALID,
    }
}

/// `dal_irq_get_read_request` (gpio_service.c).
pub fn dal_irq_get_read_request(irq: &gpio) -> u32 {
    match irq.id {
        Some(gpio_id::GPIO_ID_HPD) => DC_IRQ_SOURCE_DCI2C_RR_DDC1 + irq.en,
        _ => DC_IRQ_SOURCE_INVALID,
    }
}

/// `dal_gpio_get_generic_pin_info` (gpio_service.c).
pub fn dal_gpio_get_generic_pin_info(service: &gpio_service, id: Option<gpio_id>, en: u32) -> crate::bios_parser2::gpio_pin_info {
    let mut pin = crate::bios_parser2::gpio_pin_info::default();
    if service.translate.funcs.is_some() {
        id_to_offset(id, en, &mut pin);
    } else {
        pin.mask = 0xFFFFFFFF;
        pin.offset = 0xFFFFFFFF;
    }
    pin
}

/// `dal_ddc_open` (gpio_service.c): both pins in `mode`, the pad through the
/// data pin. `GPIO_MODE_INPUT` is detect_sink_type's, with the dual-mode
/// settling delays.
pub fn dal_ddc_open(adev: &amdgpu_device, service: &mut gpio_service, ddc: &mut ddc, mode: gpio_mode,
                    config_type: gpio_ddc_config_type) -> gpio_result {
    let result = dal_gpio_open_ex(adev, service, &mut ddc.pin_data, mode);
    if result != gpio_result::GPIO_RESULT_OK {
        return result;
    }

    let result = dal_gpio_open_ex(adev, service, &mut ddc.pin_clock, mode);
    if result != gpio_result::GPIO_RESULT_OK {
        dal_gpio_close(adev, service, &mut ddc.pin_data);
        return result;
    }

    let store_en = |g: &mut gpio| g.hw_container.gpio().map_or(0, |p| p.store.en);
    let config = gpio_ddc_config {
        r#type: config_type,
        data_en_bit_present: store_en(&mut ddc.pin_data) != 0,
        clock_en_bit_present: store_en(&mut ddc.pin_clock) != 0,
        i2c_aux_dual_mode: mode == gpio_mode::GPIO_MODE_INPUT,
    };

    let result = dal_gpio_set_config(adev, &ddc.pin_data, &config);
    if result == gpio_result::GPIO_RESULT_OK {
        return result;
    }

    dal_gpio_close(adev, service, &mut ddc.pin_clock);
    dal_gpio_close(adev, service, &mut ddc.pin_data);
    result
}

/// `dal_ddc_close` (gpio_service.c).
pub fn dal_ddc_close(adev: &amdgpu_device, service: &mut gpio_service, ddc: &mut ddc) {
    dal_gpio_close(adev, service, &mut ddc.pin_clock);
    dal_gpio_close(adev, service, &mut ddc.pin_data);
}

/// `dal_gpio_service_create_generic_mux` (gpio_service.c).
pub fn dal_gpio_service_create_generic_mux(service: &gpio_service, offset: u32, mask: u32) -> Option<Box<gpio>> {
    let mut id = None;
    let mut en = 0;

    if service.translate.funcs.is_none() || !offset_to_id(offset, mask, &mut id, &mut en) {
        return None;
    }

    // GPIO_PIN_OUTPUT_STATE_DEFAULT is GPIO_PIN_OUTPUT_STATE_ACTIVE_LOW.
    Some(dal_gpio_create(id, en, gpio_pin_output_state::GPIO_PIN_OUTPUT_STATE_ACTIVE_LOW))
}

/// `struct gpio_generic_mux_config` (gpio_types.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct gpio_generic_mux_config {
    pub enable_output_from_mux: bool,
    /// `enum gpio_signal_source`.
    pub mux_select: u32,
}

/// `dal_mux_setup_config` (gpio_service.c), through the generic pin's
/// `set_config` (hw_generic.c).
pub fn dal_mux_setup_config(adev: &amdgpu_device, mux: &gpio, config: &gpio_generic_mux_config) -> gpio_result {
    if !mux.pin_open {
        return gpio_result::GPIO_RESULT_NULL_HANDLE;
    }
    let hw_container::Generic(generic) = &mux.hw_container else { return gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR };

    let r = generic.regs.unwrap_or_default();
    crate::dc_helper::generic_reg_update_ex(adev, r.mux, &[
        (generic.shifts.GENERIC_EN as u8, generic.masks.GENERIC_EN, config.enable_output_from_mux as u32),
        (generic.shifts.GENERIC_SEL as u8, generic.masks.GENERIC_SEL, config.mux_select),
    ]);

    gpio_result::GPIO_RESULT_OK
}
