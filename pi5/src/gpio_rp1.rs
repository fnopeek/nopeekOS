//! RP1's GPIO as a plain output, after Linux `drivers/pinctrl/pinctrl-rp1.c`:
//! what gpiolib asks of the chip for a line a driver claims with
//! `GPIOD_OUT_*` and then drives (`phy-reset-gpios`).
//!
//! Every pin write is preceded by six writes of 0 to the PCIe root
//! complex's Slot Control register (`rp1_pin_writel` with `pace_pin_updates`,
//! the default): they stall the CPU while the link wakes from L1, so pin
//! updates reach RP1 in order.

use crate::hw::{Block, Hw};

const IO_BANK_BASE: u64 = 0xd_0000;
const SYS_RIO_BASE: u64 = 0xe_0000;
const PADS_BASE: u64 = 0xf_0000;
/// The root complex's register `pinctrl-rp1` paces with.
const PACE_REG: u64 = 0x0c0;

const RP1_SET_OFFSET: u64 = 0x2000;
const RP1_CLR_OFFSET: u64 = 0x3000;
const RP1_GPIO_CTRL: u64 = 0x4;
const RP1_RIO_OUT: u64 = 0x0;
const RP1_RIO_OE: u64 = 0x4;

const CTRL_FUNCSEL_MASK: u32 = 0x1f;
const CTRL_OUTOVER_MASK: u32 = 0x3 << 12;
const CTRL_OEOVER_MASK: u32 = 0x3 << 14;
const RP1_FSEL_GPIO: u32 = 5;
const RP1_FSEL_NONE: u32 = 9;
const RP1_FSEL_NONE_HW: u32 = 0x1f;
const RP1_FSEL_COUNT: u32 = 9;
const RP1_OEOVER_DISABLE: u32 = 2;
const RP1_OEOVER_PERI: u32 = 0;
const RP1_OUTOVER_PERI: u32 = 0;
const PAD_IN_ENABLE: u32 = 1 << 6;
const PAD_OUT_DISABLE: u32 = 1 << 7;

/// `rp1_iobanks`: first GPIO, CTRL offset, RIO and pads offsets.
const BANKS: [(u32, u64, u64, u64); 3] = [
    (0, 0x0000, 0x0000, 0x0004),
    (28, 0x4000, 0x4000, 0x4004),
    (34, 0x8000, 0x8000, 0x8004),
];

/// One of RP1's 54 GPIOs (`rp1_pin_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub num: u32,
    offset: u32,
    gpio: u64,
    rio: u64,
    pad: u64,
}

impl Pin {
    pub fn new(num: u32) -> Option<Pin> {
        if num >= 54 { return None; }
        let &(min, gpio_off, rio_off, pads_off) = BANKS.iter().rev().find(|b| num >= b.0)?;
        let offset = num - min;
        Some(Pin {
            num,
            offset,
            gpio: IO_BANK_BASE + gpio_off + offset as u64 * 8,
            rio: SYS_RIO_BASE + rio_off,
            pad: PADS_BASE + pads_off + offset as u64 * 4,
        })
    }

    /// `rp1_pin_writel`.
    fn writel(hw: &mut impl Hw, reg: u64, v: u32) {
        for _ in 0..6 {
            hw.w32(Block::Pcie, PACE_REG, 0);
        }
        hw.w32(Block::Rp1Bar1, reg, v);
    }

    /// `rp1_gpio_get_direction`: true for an output.
    pub fn is_output(&self, hw: &mut impl Hw) -> bool {
        hw.r32(Block::Rp1Bar1, self.rio + RP1_RIO_OE) & (1 << self.offset) != 0
    }

    /// `rp1_set_value`, the physical level.
    pub fn set_value(&self, hw: &mut impl Hw, high: bool) {
        let alias = if high { RP1_SET_OFFSET } else { RP1_CLR_OFFSET };
        Self::writel(hw, self.rio + RP1_RIO_OUT + alias, 1 << self.offset);
    }

    /// `rp1_set_dir`.
    fn set_dir(&self, hw: &mut impl Hw, input: bool) {
        let alias = if input { RP1_CLR_OFFSET } else { RP1_SET_OFFSET };
        Self::writel(hw, self.rio + RP1_RIO_OE + alias, 1 << self.offset);
    }

    /// `rp1_pad_update`.
    fn pad_update(&self, hw: &mut impl Hw, clr: u32, set: u32) {
        let pad = (hw.r32(Block::Rp1Bar1, self.pad) & !clr) | set;
        Self::writel(hw, self.pad, pad);
    }

    /// `rp1_set_fsel`.
    fn set_fsel(&self, hw: &mut impl Hw, fsel: u32) {
        let mut ctrl = hw.r32(Block::Rp1Bar1, self.gpio + RP1_GPIO_CTRL);
        let fsel = if fsel >= RP1_FSEL_COUNT { RP1_FSEL_NONE_HW } else { fsel };
        self.pad_update(hw, PAD_IN_ENABLE, PAD_IN_ENABLE);
        self.pad_update(hw, PAD_OUT_DISABLE, 0);
        if fsel == RP1_FSEL_NONE {
            crate::hw::replace_bits(&mut ctrl, RP1_OEOVER_DISABLE, CTRL_OEOVER_MASK);
        } else {
            crate::hw::replace_bits(&mut ctrl, RP1_OUTOVER_PERI, CTRL_OUTOVER_MASK);
            crate::hw::replace_bits(&mut ctrl, RP1_OEOVER_PERI, CTRL_OEOVER_MASK);
        }
        crate::hw::replace_bits(&mut ctrl, fsel, CTRL_FUNCSEL_MASK);
        Self::writel(hw, self.gpio + RP1_GPIO_CTRL, ctrl);
    }

    /// `rp1_gpio_direction_output`: level first, then direction, then the
    /// pin's function becomes SYS_RIO.
    pub fn direction_output(&self, hw: &mut impl Hw, high: bool) {
        self.set_value(hw, high);
        self.set_dir(hw, false);
        self.set_fsel(hw, RP1_FSEL_GPIO);
    }
}

/// A GPIO as a consumer sees it: the logical level, which an active-low
/// line inverts (`gpiod_*` with `GPIO_ACTIVE_LOW`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub pin: Pin,
    pub active_low: bool,
}

impl Line {
    /// `gpiod_get_optional(..., GPIOD_OUT_LOW|HIGH)`: the request reads the
    /// direction, then the line becomes an output at `value`.
    pub fn request_output(hw: &mut impl Hw, pin: Pin, active_low: bool, value: bool) -> Line {
        let line = Line { pin, active_low };
        pin.is_output(hw);
        pin.direction_output(hw, value != active_low);
        line
    }

    /// `gpiod_set_value_cansleep`.
    pub fn set(&self, hw: &mut impl Hw, value: bool) {
        self.pin.set_value(hw, value != self.active_low);
    }
}
