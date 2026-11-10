//! Broadcom STB GPIO lines (Linux `drivers/gpio/gpio-brcmstb.c` through
//! `gpio_generic` with `brcm,gpio-direct`): per bank a block of 8
//! registers, DATA and IODIR among them (IODIR bit 0 = output); a line
//! numbers through the banks by their `brcm,gpio-bank-widths`.

use super::dt::{Fdt, Node};

const BANK_BYTES: u64 = 8 * 4;
const GIO_DATA: u64 = 4;
const GIO_IODIR: u64 = 2 * 4;
const GPIO_ACTIVE_LOW: u32 = 1;

/// Read-modify-write on a bank is one step for every user of the block.
static LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// One line, as a `<&controller line flags>` specifier names it.
#[derive(Clone, Copy)]
pub struct Line {
    /// Physical address of the bank's registers.
    bank: u64,
    bit: u32,
    active_low: bool,
}

impl Line {
    /// Entry `index` of the specifier list `prop` of `node`, on a
    /// `brcm,brcmstb-gpio` controller with two cells.
    pub fn from_dt(fdt: &Fdt, node: &Node, prop: &[u8], index: usize) -> Option<Line> {
        let ph = fdt.prop_cell(node, prop, 3 * index)?;
        let line = fdt.prop_cell(node, prop, 3 * index + 1)?;
        let flags = fdt.prop_cell(node, prop, 3 * index + 2)?;
        let ctrl = fdt.find_phandle(ph)?;
        if !fdt.prop_has_str(&ctrl, b"compatible", b"brcm,brcmstb-gpio") { return None; }
        let (base, _) = fdt.reg0(&ctrl)?;
        // brcmstb_gpio_probe: banks of the widths given, 32 without.
        let (mut bank, mut first) = (0u32, 0u32);
        let mut i = 0;
        while let Some(w) = fdt.prop_cell(&ctrl, b"brcm,gpio-bank-widths", i) {
            if line < first + w { break; }
            first += w;
            bank += 1;
            i += 1;
        }
        if fdt.prop(&ctrl, b"brcm,gpio-bank-widths").is_none() {
            bank = line / 32;
            first = bank * 32;
        }
        Some(Line { bank: base + bank as u64 * BANK_BYTES, bit: line - first, active_low: flags & GPIO_ACTIVE_LOW != 0 })
    }

    fn read(&self, off: u64) -> u32 {
        // SAFETY: DATA or IODIR of a bank the device tree names; the
        // identity map covers it (Device memory on the board).
        unsafe { core::ptr::read_volatile((self.bank + off) as *const u32) }
    }

    fn rmw(&self, off: u64, set: bool) {
        let _g = LOCK.lock();
        let v = self.read(off);
        let v = if set { v | 1 << self.bit } else { v & !(1 << self.bit) };
        // SAFETY: as in `read`.
        unsafe { core::ptr::write_volatile((self.bank + off) as *mut u32, v) }
    }

    /// Drive the line, logical `on`; `gpio_generic`'s direction_output:
    /// the level first, then the pin.
    pub fn output(&self, on: bool) {
        self.set(on);
        self.rmw(GIO_IODIR, false);
    }

    /// Set the logical value of an output.
    pub fn set(&self, on: bool) {
        self.rmw(GIO_DATA, on ^ self.active_low);
    }

    /// The pin as an input.
    pub fn input(&self) {
        self.rmw(GIO_IODIR, true);
    }

    /// The logical value at the pin.
    pub fn get(&self) -> bool {
        (self.read(GIO_DATA) >> self.bit & 1 != 0) ^ self.active_low
    }
}
