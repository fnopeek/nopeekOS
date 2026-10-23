//! display/dc/gpio/gpio_base.c + hw_gpio.c + hw_hpd.c + hw_ddc.c +
//! hw_generic.c (Linux 6.18.55): a GPIO handle and the pin behind it. Linux
//! keeps `gpio->pin` as a pointer into `gpio->hw_container`; the port keeps
//! the container and whether the pin is taken (`pin_open`). The service a
//! gpio belongs to is passed in where Linux follows `gpio->service`.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc_helper::{generic_reg_get, generic_reg_get_n, generic_reg_set_ex, generic_reg_update_ex};
use crate::gpio_service::{dal_gpio_service_close, dal_gpio_service_lock, dal_gpio_service_open,
    dal_gpio_service_unlock, gpio_id, gpio_service};
use crate::linux::msleep;
use alloc::boxed::Box;

/// `enum gpio_result` (gpio_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum gpio_result {
    GPIO_RESULT_OK,
    GPIO_RESULT_NULL_HANDLE,
    GPIO_RESULT_INVALID_DATA,
    GPIO_RESULT_DEVICE_BUSY,
    GPIO_RESULT_OPEN_FAILED,
    GPIO_RESULT_ALREADY_OPENED,
    GPIO_RESULT_NON_SPECIFIC_ERROR,
}

/// `enum gpio_mode` (gpio_types.h). `GPIO_MODE_UNKNOWN` is -1.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum gpio_mode {
    #[default]
    GPIO_MODE_UNKNOWN = -1,
    GPIO_MODE_INPUT = 0,
    GPIO_MODE_OUTPUT,
    GPIO_MODE_FAST_OUTPUT,
    GPIO_MODE_HARDWARE,
    GPIO_MODE_INTERRUPT,
}

/// `enum gpio_pin_output_state` (gpio_types.h); DEFAULT is ACTIVE_LOW.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum gpio_pin_output_state {
    #[default]
    GPIO_PIN_OUTPUT_STATE_ACTIVE_LOW,
    GPIO_PIN_OUTPUT_STATE_ACTIVE_HIGH,
}

/// `GPIO_DDC_LINE_*` (enum gpio_ddc_line).
pub const GPIO_DDC_LINE_DDC1: u32 = 0;
pub const GPIO_DDC_LINE_DDC2: u32 = 1;
pub const GPIO_DDC_LINE_DDC3: u32 = 2;
pub const GPIO_DDC_LINE_DDC4: u32 = 3;
pub const GPIO_DDC_LINE_DDC5: u32 = 4;
pub const GPIO_DDC_LINE_DDC6: u32 = 5;
pub const GPIO_DDC_LINE_DDC_VGA: u32 = 6;
pub const GPIO_DDC_LINE_VIP_PAD: u32 = 7;
pub const GPIO_DDC_LINE_I2C_PAD: u32 = GPIO_DDC_LINE_VIP_PAD;
pub const GPIO_DDC_LINE_MAX: u32 = GPIO_DDC_LINE_I2C_PAD;

/// `struct gpio_registers` (gpio_regs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct gpio_registers {
    pub MASK_reg: u32,
    pub MASK_mask: u32,
    pub MASK_shift: u32,
    pub A_reg: u32,
    pub A_mask: u32,
    pub A_shift: u32,
    pub EN_reg: u32,
    pub EN_mask: u32,
    pub EN_shift: u32,
    pub Y_reg: u32,
    pub Y_mask: u32,
    pub Y_shift: u32,
}

impl gpio_registers {
    pub fn from(v: &[u32]) -> Self {
        gpio_registers {
            MASK_reg: v[0],
            MASK_mask: v[1],
            MASK_shift: v[2],
            A_reg: v[3],
            A_mask: v[4],
            A_shift: v[5],
            EN_reg: v[6],
            EN_mask: v[7],
            EN_shift: v[8],
            Y_reg: v[9],
            Y_mask: v[10],
            Y_shift: v[11],
        }
    }
}

/// `struct hpd_registers` (hpd_regs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct hpd_registers {
    pub gpio: gpio_registers,
    pub int_status: u32,
    pub toggle_filt_cntl: u32,
}

/// `struct hpd_sh_mask` (hpd_regs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct hpd_sh_mask {
    pub DC_HPD_SENSE_DELAYED: u32,
    pub DC_HPD_SENSE: u32,
    pub DC_HPD_CONNECT_INT_DELAY: u32,
    pub DC_HPD_DISCONNECT_INT_DELAY: u32,
}

/// `struct ddc_registers` (ddc_regs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct ddc_registers {
    pub gpio: gpio_registers,
    pub ddc_setup: u32,
    pub phy_aux_cntl: u32,
    pub dc_gpio_aux_ctrl_5: u32,
}

/// `struct ddc_sh_mask` (ddc_regs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct ddc_sh_mask {
    pub DC_I2C_DDC1_ENABLE: u32,
    pub DC_I2C_DDC1_EDID_DETECT_ENABLE: u32,
    pub DC_I2C_DDC1_EDID_DETECT_MODE: u32,
    pub DC_GPIO_DDC1DATA_PD_EN: u32,
    pub DC_GPIO_DDC1CLK_PD_EN: u32,
    pub AUX_PAD1_MODE: u32,
    pub DC_GPIO_SDA_PD_DIS: u32,
    pub DC_GPIO_SCL_PD_DIS: u32,
    pub AUX_PAD_RXSEL: u32,
    pub DDC_PAD_I2CMODE: u32,
}

/// `struct generic_registers` (generic_regs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct generic_registers {
    pub gpio: gpio_registers,
    pub mux: u32,
}

/// `struct generic_sh_mask` (generic_regs.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct generic_sh_mask {
    pub GENERIC_EN: u32,
    pub GENERIC_SEL: u32,
}

/// The saved register values (`struct hw_gpio`'s `store`).
#[derive(Default, Clone, Copy, Debug)]
pub struct hw_gpio_store {
    pub mask: u32,
    pub a: u32,
    pub en: u32,
    pub mux: u32,
}

/// `struct hw_gpio_pin` + `struct hw_gpio` (hw_gpio.h). `regs` is None
/// until the factory defines the pin's registers on open.
#[derive(Default, Clone, Copy, Debug)]
pub struct hw_gpio {
    pub id: Option<gpio_id>,
    pub en: u32,
    pub mode: gpio_mode,
    pub opened: bool,
    pub regs: Option<gpio_registers>,
    pub store: hw_gpio_store,
    pub mux_supported: bool,
}

/// `struct hw_hpd` (hw_hpd.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct hw_hpd {
    pub base: hw_gpio,
    pub regs: Option<hpd_registers>,
    pub shifts: hpd_sh_mask,
    pub masks: hpd_sh_mask,
}

/// `struct hw_ddc` (hw_ddc.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct hw_ddc {
    pub base: hw_gpio,
    pub regs: Option<ddc_registers>,
    pub shifts: ddc_sh_mask,
    pub masks: ddc_sh_mask,
}

/// `struct hw_generic` (hw_generic.h).
#[derive(Default, Clone, Copy, Debug)]
pub struct hw_generic {
    pub base: hw_gpio,
    pub regs: Option<generic_registers>,
    pub shifts: generic_sh_mask,
    pub masks: generic_sh_mask,
}

/// `gpio->hw_container`: the union of the three pin kinds.
#[derive(Debug, Default)]
pub enum hw_container {
    #[default]
    None,
    Ddc(Box<hw_ddc>),
    Hpd(Box<hw_hpd>),
    Generic(Box<hw_generic>),
}

impl hw_container {
    /// The `hw_gpio` every pin kind starts with.
    pub fn gpio(&mut self) -> Option<&mut hw_gpio> {
        match self {
            hw_container::None => None,
            hw_container::Ddc(d) => Some(&mut d.base),
            hw_container::Hpd(h) => Some(&mut h.base),
            hw_container::Generic(g) => Some(&mut g.base),
        }
    }
}

/// `struct gpio` (gpio.h).
#[derive(Debug, Default)]
pub struct gpio {
    pub id: Option<gpio_id>,
    pub en: u32,
    pub mode: gpio_mode,
    pub output_state: gpio_pin_output_state,
    pub hw_container: hw_container,
    /// `gpio->pin != NULL`.
    pub pin_open: bool,
}

/// One register field as (shift, mask) for the dc_helper slices.
fn f(shift: u32, mask: u32) -> (u8, u32) {
    (shift as u8, mask)
}

/// `store_registers` (hw_gpio.c).
fn store_registers(adev: &amdgpu_device, gpio: &mut hw_gpio) {
    let r = gpio.regs.unwrap_or_default();
    gpio.store.mask = generic_reg_get(adev, r.MASK_reg, r.MASK_shift as u8, r.MASK_mask).1;
    gpio.store.a = generic_reg_get(adev, r.A_reg, r.A_shift as u8, r.A_mask).1;
    gpio.store.en = generic_reg_get(adev, r.EN_reg, r.EN_shift as u8, r.EN_mask).1;
}

/// `restore_registers` (hw_gpio.c).
fn restore_registers(adev: &amdgpu_device, gpio: &hw_gpio) {
    let r = gpio.regs.unwrap_or_default();
    generic_reg_update_ex(adev, r.MASK_reg, &[(r.MASK_shift as u8, r.MASK_mask, gpio.store.mask)]);
    generic_reg_update_ex(adev, r.A_reg, &[(r.A_shift as u8, r.A_mask, gpio.store.a)]);
    generic_reg_update_ex(adev, r.EN_reg, &[(r.EN_shift as u8, r.EN_mask, gpio.store.en)]);
}

/// `dal_hw_gpio_config_mode` (hw_gpio.c).
pub fn dal_hw_gpio_config_mode(adev: &amdgpu_device, gpio: &mut hw_gpio, mode: gpio_mode) -> gpio_result {
    gpio.mode = mode;
    let r = gpio.regs.unwrap_or_default();
    let mask = |v: u32| generic_reg_update_ex(adev, r.MASK_reg, &[(r.MASK_shift as u8, r.MASK_mask, v)]);
    let a = |v: u32| generic_reg_update_ex(adev, r.A_reg, &[(r.A_shift as u8, r.A_mask, v)]);
    let en = |v: u32| generic_reg_update_ex(adev, r.EN_reg, &[(r.EN_shift as u8, r.EN_mask, v)]);

    match mode {
        gpio_mode::GPIO_MODE_INPUT => {
            // turn off output enable, act as input pin
            en(0);
            mask(1);
            gpio_result::GPIO_RESULT_OK
        }
        gpio_mode::GPIO_MODE_OUTPUT | gpio_mode::GPIO_MODE_FAST_OUTPUT => {
            a(0);
            mask(1);
            gpio_result::GPIO_RESULT_OK
        }
        // tri-state, driven by HW; interrupt mode is HPD (IrqGpio) pins only
        gpio_mode::GPIO_MODE_HARDWARE | gpio_mode::GPIO_MODE_INTERRUPT => {
            mask(0);
            gpio_result::GPIO_RESULT_OK
        }
        _ => gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR,
    }
}

/// `dal_hw_gpio_open` (hw_gpio.c): every pin kind's `open`.
pub fn dal_hw_gpio_open(adev: &amdgpu_device, pin: &mut hw_gpio, mode: gpio_mode) -> bool {
    store_registers(adev, pin);
    pin.opened = dal_hw_gpio_config_mode(adev, pin, mode) == gpio_result::GPIO_RESULT_OK;
    pin.opened
}

/// `dal_hw_gpio_close` (hw_gpio.c): every pin kind's `close`.
pub fn dal_hw_gpio_close(adev: &amdgpu_device, pin: &mut hw_gpio) {
    restore_registers(adev, pin);
    pin.mode = gpio_mode::GPIO_MODE_UNKNOWN;
    pin.opened = false;
}

/// `dal_hw_gpio_get_value` (hw_gpio.c).
pub fn dal_hw_gpio_get_value(adev: &amdgpu_device, pin: &hw_gpio, value: &mut u32) -> gpio_result {
    match pin.mode {
        gpio_mode::GPIO_MODE_INPUT | gpio_mode::GPIO_MODE_OUTPUT | gpio_mode::GPIO_MODE_HARDWARE
        | gpio_mode::GPIO_MODE_FAST_OUTPUT => {
            let r = pin.regs.unwrap_or_default();
            *value = generic_reg_get(adev, r.Y_reg, r.Y_shift as u8, r.Y_mask).1;
            gpio_result::GPIO_RESULT_OK
        }
        _ => gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR,
    }
}

/// `dal_hw_gpio_set_value` (hw_gpio.c).
pub fn dal_hw_gpio_set_value(adev: &amdgpu_device, pin: &hw_gpio, value: u32) -> gpio_result {
    let r = pin.regs.unwrap_or_default();
    match pin.mode {
        gpio_mode::GPIO_MODE_OUTPUT => {
            generic_reg_update_ex(adev, r.A_reg, &[(r.A_shift as u8, r.A_mask, value)]);
            gpio_result::GPIO_RESULT_OK
        }
        gpio_mode::GPIO_MODE_FAST_OUTPUT => {
            // the EN register switches faster (used in DDC GPIO)
            generic_reg_update_ex(adev, r.EN_reg, &[(r.EN_shift as u8, r.EN_mask, !value)]);
            gpio_result::GPIO_RESULT_OK
        }
        _ => gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR,
    }
}

/// `dal_hw_gpio_construct` (hw_gpio.c).
fn dal_hw_gpio_construct(pin: &mut hw_gpio, id: gpio_id, en: u32) {
    pin.id = Some(id);
    pin.en = en;
    pin.mode = gpio_mode::GPIO_MODE_UNKNOWN;
    pin.opened = false;
    pin.store = hw_gpio_store::default();
    pin.mux_supported = false;
}

/// `dal_hw_hpd_get_value` (hw_hpd.c): in interrupt mode the DELAYED sense
/// bit is read and dropped — Linux returns OK without storing it.
pub fn dal_hw_hpd_get_value(adev: &amdgpu_device, hpd: &hw_hpd, value: &mut u32) -> gpio_result {
    if hpd.base.mode == gpio_mode::GPIO_MODE_INTERRUPT {
        let r = hpd.regs.unwrap_or_default();
        generic_reg_get(adev, r.int_status, hpd.shifts.DC_HPD_SENSE_DELAYED as u8, hpd.masks.DC_HPD_SENSE_DELAYED);
        return gpio_result::GPIO_RESULT_OK;
    }
    dal_hw_gpio_get_value(adev, &hpd.base, value)
}

/// `dal_hw_hpd_set_config` (hw_hpd.c): the connect/disconnect filters, in
/// units of 10 ms.
pub fn dal_hw_hpd_set_config(adev: &amdgpu_device, hpd: &hw_hpd, delay_on_connect: u32,
                             delay_on_disconnect: u32) -> gpio_result {
    let r = hpd.regs.unwrap_or_default();
    generic_reg_update_ex(adev, r.toggle_filt_cntl, &[
        (hpd.shifts.DC_HPD_CONNECT_INT_DELAY as u8, hpd.masks.DC_HPD_CONNECT_INT_DELAY, delay_on_connect / 10),
        (hpd.shifts.DC_HPD_DISCONNECT_INT_DELAY as u8, hpd.masks.DC_HPD_DISCONNECT_INT_DELAY,
            delay_on_disconnect / 10),
    ]);
    gpio_result::GPIO_RESULT_OK
}

/// `dal_hw_hpd_init` (hw_hpd.c).
pub fn dal_hw_hpd_init(id: gpio_id, en: u32) -> Box<hw_hpd> {
    let mut hpd = Box::new(hw_hpd::default());
    dal_hw_gpio_construct(&mut hpd.base, id, en);
    hpd
}

/// `dal_hw_ddc_init` (hw_ddc.c).
pub fn dal_hw_ddc_init(id: gpio_id, en: u32) -> Box<hw_ddc> {
    let mut ddc = Box::new(hw_ddc::default());
    dal_hw_gpio_construct(&mut ddc.base, id, en);
    ddc
}

/// `dal_hw_generic_init` (hw_generic.c).
pub fn dal_hw_generic_init(id: gpio_id, en: u32) -> Box<hw_generic> {
    let mut generic = Box::new(hw_generic::default());
    dal_hw_gpio_construct(&mut generic.base, id, en);
    generic
}

/// `enum gpio_ddc_config_type` (gpio_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum gpio_ddc_config_type {
    GPIO_DDC_CONFIG_TYPE_MODE_AUX,
    GPIO_DDC_CONFIG_TYPE_MODE_I2C,
    GPIO_DDC_CONFIG_TYPE_POLL_FOR_CONNECT,
    GPIO_DDC_CONFIG_TYPE_POLL_FOR_DISCONNECT,
    GPIO_DDC_CONFIG_TYPE_DISABLE_POLLING,
}

/// `struct gpio_ddc_config` (gpio_types.h), with the outer
/// `gpio_config_data.type` reduced to whether it is I2C_AUX_DUAL_MODE.
#[derive(Clone, Copy, Debug)]
pub struct gpio_ddc_config {
    pub r#type: gpio_ddc_config_type,
    pub data_en_bit_present: bool,
    pub clock_en_bit_present: bool,
    pub i2c_aux_dual_mode: bool,
}

/// `set_config` (hw_ddc.c).
pub fn dal_hw_ddc_set_config(adev: &amdgpu_device, ddc: &hw_ddc, config: &gpio_ddc_config) -> gpio_result {
    let r = ddc.regs.unwrap_or_default();
    let (s, m) = (&ddc.shifts, &ddc.masks);
    let mut out = [0u32; 3];
    let regval = generic_reg_get_n(adev, r.gpio.MASK_reg, &[
        f(s.DC_GPIO_DDC1DATA_PD_EN, m.DC_GPIO_DDC1DATA_PD_EN),
        f(s.DC_GPIO_DDC1CLK_PD_EN, m.DC_GPIO_DDC1CLK_PD_EN),
        f(s.AUX_PAD1_MODE, m.AUX_PAD1_MODE),
    ], &mut out);
    let [ddc_data_pd_en, ddc_clk_pd_en, aux_pad_mode] = out;
    let en = ddc.base.en;

    match config.r#type {
        gpio_ddc_config_type::GPIO_DDC_CONFIG_TYPE_MODE_I2C => {
            // On plug-in, a transient level on the pad must be discharged
            // through the internal pull-down (2.5 ms for AUX detection).
            if en != GPIO_DDC_LINE_VIP_PAD {
                if ddc_data_pd_en == 0 || ddc_clk_pd_en == 0 {
                    if en == GPIO_DDC_LINE_DDC_VGA {
                        // bit 4 of mask has different usage in some cases
                        generic_reg_set_ex(adev, r.gpio.MASK_reg, regval,
                            &[(s.DC_GPIO_DDC1DATA_PD_EN as u8, m.DC_GPIO_DDC1DATA_PD_EN, 1)]);
                    } else {
                        generic_reg_set_ex(adev, r.gpio.MASK_reg, regval, &[
                            (s.DC_GPIO_DDC1DATA_PD_EN as u8, m.DC_GPIO_DDC1DATA_PD_EN, 1),
                            (s.DC_GPIO_DDC1CLK_PD_EN as u8, m.DC_GPIO_DDC1CLK_PD_EN, 1),
                        ]);
                    }
                    if config.i2c_aux_dual_mode {
                        msleep(3);
                    }
                }
            } else {
                let mut pd = [0u32; 2];
                generic_reg_get_n(adev, r.gpio.MASK_reg, &[
                    f(s.DC_GPIO_SDA_PD_DIS, m.DC_GPIO_SDA_PD_DIS),
                    f(s.DC_GPIO_SCL_PD_DIS, m.DC_GPIO_SCL_PD_DIS),
                ], &mut pd);
                let [sda_pd_dis, scl_pd_dis] = pd;

                if sda_pd_dis != 0 {
                    generic_reg_set_ex(adev, r.gpio.MASK_reg, regval,
                        &[(s.DC_GPIO_SDA_PD_DIS as u8, m.DC_GPIO_SDA_PD_DIS, 0)]);
                    if config.i2c_aux_dual_mode {
                        msleep(3);
                    }
                }
                if scl_pd_dis == 0 {
                    generic_reg_set_ex(adev, r.gpio.MASK_reg, regval,
                        &[(s.DC_GPIO_SCL_PD_DIS as u8, m.DC_GPIO_SCL_PD_DIS, 1)]);
                    if config.i2c_aux_dual_mode {
                        msleep(3);
                    }
                }
            }

            if aux_pad_mode != 0 {
                // let pins de-assert before setting the pad to I2C mode
                if config.data_en_bit_present || config.clock_en_bit_present {
                    msleep(2);
                }
                // the register is read again: some bits may have changed
                generic_reg_update_ex(adev, r.gpio.MASK_reg, &[(s.AUX_PAD1_MODE as u8, m.AUX_PAD1_MODE, 0)]);
            }

            if r.dc_gpio_aux_ctrl_5 != 0 {
                generic_reg_update_ex(adev, r.dc_gpio_aux_ctrl_5, &[(s.DDC_PAD_I2CMODE as u8, m.DDC_PAD_I2CMODE, 1)]);
            }
            // set DC_IO_aux_rxsel = 2'b01
            if r.phy_aux_cntl != 0 {
                generic_reg_update_ex(adev, r.phy_aux_cntl, &[(s.AUX_PAD_RXSEL as u8, m.AUX_PAD_RXSEL, 1)]);
            }
            gpio_result::GPIO_RESULT_OK
        }
        gpio_ddc_config_type::GPIO_DDC_CONFIG_TYPE_MODE_AUX => {
            if aux_pad_mode == 0 {
                generic_reg_set_ex(adev, r.gpio.MASK_reg, regval, &[(s.AUX_PAD1_MODE as u8, m.AUX_PAD1_MODE, 1)]);
            }
            if r.dc_gpio_aux_ctrl_5 != 0 {
                generic_reg_update_ex(adev, r.dc_gpio_aux_ctrl_5, &[(s.DDC_PAD_I2CMODE as u8, m.DDC_PAD_I2CMODE, 0)]);
            }
            gpio_result::GPIO_RESULT_OK
        }
        t => {
            if en <= GPIO_DDC_LINE_DDC_VGA {
                let fields: &[(u8, u32, u32)] = match t {
                    gpio_ddc_config_type::GPIO_DDC_CONFIG_TYPE_POLL_FOR_CONNECT => &[
                        (s.DC_I2C_DDC1_ENABLE as u8, m.DC_I2C_DDC1_ENABLE, 1),
                        (s.DC_I2C_DDC1_EDID_DETECT_ENABLE as u8, m.DC_I2C_DDC1_EDID_DETECT_ENABLE, 1),
                        (s.DC_I2C_DDC1_EDID_DETECT_MODE as u8, m.DC_I2C_DDC1_EDID_DETECT_MODE, 0),
                    ],
                    gpio_ddc_config_type::GPIO_DDC_CONFIG_TYPE_POLL_FOR_DISCONNECT => &[
                        (s.DC_I2C_DDC1_ENABLE as u8, m.DC_I2C_DDC1_ENABLE, 1),
                        (s.DC_I2C_DDC1_EDID_DETECT_ENABLE as u8, m.DC_I2C_DDC1_EDID_DETECT_ENABLE, 1),
                        (s.DC_I2C_DDC1_EDID_DETECT_MODE as u8, m.DC_I2C_DDC1_EDID_DETECT_MODE, 1),
                    ],
                    _ => &[
                        (s.DC_I2C_DDC1_ENABLE as u8, m.DC_I2C_DDC1_ENABLE, 0),
                        (s.DC_I2C_DDC1_EDID_DETECT_ENABLE as u8, m.DC_I2C_DDC1_EDID_DETECT_ENABLE, 0),
                    ],
                };
                generic_reg_update_ex(adev, r.ddc_setup, fields);
                return gpio_result::GPIO_RESULT_OK;
            }
            gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR
        }
    }
}

/// `dal_gpio_create` (gpio_base.c). SYNC and GSL get no container; an id
/// outside the switch leaves the gpio without one too.
pub fn dal_gpio_create(id: Option<gpio_id>, en: u32, output_state: gpio_pin_output_state) -> Box<gpio> {
    let mut g = Box::new(gpio {
        id,
        en,
        mode: gpio_mode::GPIO_MODE_UNKNOWN,
        output_state,
        hw_container: hw_container::None,
        pin_open: false,
    });

    g.hw_container = match id {
        Some(i @ (gpio_id::GPIO_ID_DDC_DATA | gpio_id::GPIO_ID_DDC_CLOCK)) => hw_container::Ddc(dal_hw_ddc_init(i, en)),
        Some(i @ gpio_id::GPIO_ID_GENERIC) => hw_container::Generic(dal_hw_generic_init(i, en)),
        Some(i @ gpio_id::GPIO_ID_HPD) => hw_container::Hpd(dal_hw_hpd_init(i, en)),
        _ => hw_container::None,
    };
    g
}

/// `dal_gpio_open_ex` (gpio_base.c).
pub fn dal_gpio_open_ex(adev: &amdgpu_device, service: &mut gpio_service, g: &mut gpio, mode: gpio_mode) -> gpio_result {
    if g.pin_open {
        return gpio_result::GPIO_RESULT_ALREADY_OPENED;
    }

    // No action if allocation failed during gpio construct
    if matches!(g.hw_container, hw_container::None) {
        return gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR;
    }
    g.mode = mode;
    dal_gpio_service_open(adev, service, g)
}

/// `dal_gpio_open` (gpio_base.c).
pub fn dal_gpio_open(adev: &amdgpu_device, service: &mut gpio_service, g: &mut gpio, mode: gpio_mode) -> gpio_result {
    dal_gpio_open_ex(adev, service, g, mode)
}

/// `dal_gpio_close` (gpio_base.c).
pub fn dal_gpio_close(adev: &amdgpu_device, service: &mut gpio_service, g: &mut gpio) {
    dal_gpio_service_close(adev, service, g);
    g.mode = gpio_mode::GPIO_MODE_UNKNOWN;
}

/// `dal_gpio_lock_pin` (gpio_base.c).
pub fn dal_gpio_lock_pin(service: &mut gpio_service, g: &gpio) -> gpio_result {
    dal_gpio_service_lock(service, g.id, g.en)
}

/// `dal_gpio_unlock_pin` (gpio_base.c).
pub fn dal_gpio_unlock_pin(service: &mut gpio_service, g: &gpio) -> gpio_result {
    dal_gpio_service_unlock(service, g.id, g.en)
}

/// `dal_gpio_get_id` / `dal_gpio_get_enum` / `dal_gpio_get_mode`.
pub fn dal_gpio_get_id(g: &gpio) -> Option<gpio_id> {
    g.id
}

pub fn dal_gpio_get_enum(g: &gpio) -> u32 {
    g.en
}

pub fn dal_gpio_get_mode(g: &gpio) -> gpio_mode {
    g.mode
}

/// `dal_gpio_get_value` (gpio_base.c), through the pin's `get_value`.
pub fn dal_gpio_get_value(adev: &amdgpu_device, g: &gpio, value: &mut u32) -> gpio_result {
    if !g.pin_open {
        return gpio_result::GPIO_RESULT_NULL_HANDLE;
    }
    match &g.hw_container {
        hw_container::Hpd(h) => dal_hw_hpd_get_value(adev, h, value),
        hw_container::Ddc(d) => dal_hw_gpio_get_value(adev, &d.base, value),
        hw_container::Generic(x) => dal_hw_gpio_get_value(adev, &x.base, value),
        hw_container::None => gpio_result::GPIO_RESULT_NULL_HANDLE,
    }
}

/// `dal_gpio_set_value` (gpio_base.c).
pub fn dal_gpio_set_value(adev: &amdgpu_device, g: &gpio, value: u32) -> gpio_result {
    if !g.pin_open {
        return gpio_result::GPIO_RESULT_NULL_HANDLE;
    }
    match &g.hw_container {
        hw_container::Hpd(h) => dal_hw_gpio_set_value(adev, &h.base, value),
        hw_container::Ddc(d) => dal_hw_gpio_set_value(adev, &d.base, value),
        hw_container::Generic(x) => dal_hw_gpio_set_value(adev, &x.base, value),
        hw_container::None => gpio_result::GPIO_RESULT_NULL_HANDLE,
    }
}

/// `dal_gpio_change_mode` (gpio_base.c).
pub fn dal_gpio_change_mode(adev: &amdgpu_device, g: &mut gpio, mode: gpio_mode) -> gpio_result {
    if !g.pin_open {
        return gpio_result::GPIO_RESULT_NULL_HANDLE;
    }
    match g.hw_container.gpio() {
        Some(p) => dal_hw_gpio_config_mode(adev, p, mode),
        None => gpio_result::GPIO_RESULT_NULL_HANDLE,
    }
}
