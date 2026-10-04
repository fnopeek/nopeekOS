//! The AMD FCH GPIO block — only as much as one question needs: is a
//! report pending right now?
//!
//! Ported from Linux `drivers/pinctrl/pinctrl-amd.{c,h}`: one register per
//! pin at `base + pin * 4` (`amd_gpio_get_value`) with the live level in
//! bit 16 (`PIN_STS_OFF`). `base` is the first `Memory32Fixed` of the ACPI
//! device, the same source as Linux'
//! `devm_platform_get_and_ioremap_resource(pdev, 0, …)`; no hardwired
//! 0xFED81500.
//!
//! HID over I2C is level-triggered: the device asserts its line while a
//! report is pending and releases it once the report is read, which is why
//! Linux calls `i2c_hid_get_input` only from `i2c_hid_irq`. Without the
//! pin, every poll is a blind bus read of `wMaxInputLength` bytes; a look
//! at the pin costs one register read.
//!
//! The register layout is AMD-specific. An Intel block (e.g. `INT34BB`)
//! has something else at the same place, so [`is_amd_block`] is checked
//! before anything is read.

use alloc::string::String;

/// `PIN_STS_OFF` (pinctrl-amd.h) — the level the pin is driving right now.
pub const PIN_STS: u32 = 1 << 16;

/// The ACPI IDs Linux binds `pinctrl-amd` to (`amd_gpio_acpi_match`).
pub const AMD_GPIO_IDS: [&str; 3] = ["AMD0030", "AMDI0030", "AMDI0031"];

/// Is this a GPIO block whose registers we know?
pub fn is_amd_block(ids: &[String]) -> bool {
    ids.iter().any(|i| AMD_GPIO_IDS.iter().any(|a| i == a))
}

/// Where the pin must be mapped, and where its register then lies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinWindow {
    /// Page-aligned base; `npk_mmio_map_phys` rejects anything else.
    pub map_base: u32,
    /// Number of pages needed to reach this pin.
    pub pages: u32,
    /// Offset of its register relative to `map_base`.
    pub reg_off: u32,
}

/// Compute the mapping window for one pin.
///
/// Firmware typically declares the AMD block as
/// `Memory32Fixed(ReadWrite, 0xFED81500, 0x300)`, which is not page-aligned.
/// The mapping starts at the page below, and the remainder goes into
/// `reg_off`.
///
/// `None` if the pin lies outside the declared window: then the block or
/// the pin is wrong, and reading a neighbouring register would be a guess.
pub fn pin_window(mmio_base: u32, mmio_len: u32, pin: u16) -> Option<PinWindow> {
    if mmio_base == 0 || mmio_len == 0 {
        return None;
    }
    let reg = (pin as u32).checked_mul(4)?;
    if reg.checked_add(4)? > mmio_len {
        return None;
    }
    let within = mmio_base & 0xFFF;
    let map_base = mmio_base & !0xFFF;
    let reg_off = within.checked_add(reg)?;
    // Only as many pages as this pin needs; a whole 64 KB window would
    // exceed the sixteen pages the kernel grants.
    let pages = (reg_off + 4 + 4095) / 4096;
    if pages == 0 || pages > 16 {
        return None;
    }
    Some(PinWindow { map_base, pages, reg_off })
}

// ── Interrupts: `pinctrl-amd.{c,h}` ──────────────────────────────────
//
// One register per pin; bits from pinctrl-amd.h. The block has one line
// for all pins (`_CRS`), and `do_amd_gpio_irq_handler` acknowledges the pin
// first (writing back the read value clears the status bits) and then the
// unit (`EOI_MASK` in WAKE_INT_MASTER_REG).

pub const LEVEL_TRIG: u32 = 1 << 8; // LEVEL_TRIG_OFF
pub const ACTIVE_LEVEL_SHIFT: u32 = 9; // ACTIVE_LEVEL_OFF, 2 bits
pub const ACTIVE_LEVEL_MASK: u32 = 0x3 << ACTIVE_LEVEL_SHIFT;
pub const INTERRUPT_ENABLE: u32 = 1 << 11;
pub const INTERRUPT_MASK: u32 = 1 << 12; // 1 = NOT masked (irq_unmask sets it)
pub const INTERRUPT_STS: u32 = 1 << 28;
pub const WAKE_STS: u32 = 1 << 29;
/// `PIN_IRQ_PENDING`
pub const PIN_IRQ_PENDING: u32 = INTERRUPT_STS | WAKE_STS;
/// Relative to the block base.
pub const WAKE_INT_MASTER_REG: u32 = 0xfc;
/// Which groups of four pins have something pending (bits 0-45).
pub const WAKE_INT_STATUS_REG0: u32 = 0x2f8;
pub const WAKE_INT_STATUS_REG1: u32 = 0x2fc;
pub const EOI_MASK: u32 = 1 << 29;

/// `amd_gpio_irq_set_type` for a LEVEL line (the only kind HID over I2C
/// uses): level trigger, the polarity, and `CLR_INTR_STAT` so a status left
/// from before is cleared. Returns the value without the enable bit; the
/// caller does the debounce-settle dance and then `amd_gpio_irq_enable`.
pub fn irq_level_config(pin_reg: u32, active_low: bool) -> u32 {
    let mut v = pin_reg | LEVEL_TRIG;
    v &= !ACTIVE_LEVEL_MASK;
    v |= (if active_low { 1 } else { 0 }) << ACTIVE_LEVEL_SHIFT;
    v | INTERRUPT_STS
}

/// Does the pin say "I have something"?
///
/// `active_low` comes from the firmware's `GpioInt` (ACPI `int_flags`
/// bits 2:1), not from an assumption.
///
/// All ones means nobody answered; this then returns true and the caller
/// reads. A false positive costs one bus transfer; a missed one costs the
/// pointer.
pub fn asserted(pin_reg: u32, active_low: bool) -> bool {
    if pin_reg == u32::MAX {
        return true;
    }
    let high = pin_reg & PIN_STS != 0;
    if active_low { !high } else { high }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::ToString, vec};

    /// `amd_gpio_irq_set_type(IRQ_TYPE_LEVEL_LOW)`: level, active low,
    /// clear status, and leave the pull/output bits untouched.
    #[test]
    fn level_low_config_matches_set_type() {
        let before = (1 << 20) | (0x2 << ACTIVE_LEVEL_SHIFT); // pull-up, both-edges
        let v = irq_level_config(before, true);
        assert_eq!(v & LEVEL_TRIG, LEVEL_TRIG);
        assert_eq!((v & ACTIVE_LEVEL_MASK) >> ACTIVE_LEVEL_SHIFT, 1);
        assert_eq!(v & INTERRUPT_STS, INTERRUPT_STS);
        assert_eq!(v & (1 << 20), 1 << 20, "pull-up bleibt");
        assert_eq!(v & INTERRUPT_ENABLE, 0, "freigegeben wird erst danach");
        let hi = irq_level_config(0, false);
        assert_eq!(hi & ACTIVE_LEVEL_MASK, 0);
    }

    /// Two typical pins against the usual AMD block.
    #[test]
    fn ideapad_pins_land_in_the_first_page() {
        let w = pin_window(0xFED8_1500, 0x300, 9).expect("pin 9");
        assert_eq!(w.map_base, 0xFED8_1000);
        assert_eq!(w.pages, 1);
        assert_eq!(w.reg_off, 0x500 + 9 * 4);

        let w = pin_window(0xFED8_1500, 0x300, 89).expect("pin 89");
        assert_eq!(w.map_base, 0xFED8_1000);
        assert_eq!(w.pages, 1);
        assert_eq!(w.reg_off, 0x500 + 89 * 4);
    }

    /// A pin outside the declared window is not a pin.
    #[test]
    fn a_pin_past_the_window_is_refused() {
        // 0x300 bytes are 192 registers: 0..=191.
        assert!(pin_window(0xFED8_1500, 0x300, 191).is_some());
        assert!(pin_window(0xFED8_1500, 0x300, 192).is_none());
        assert!(pin_window(0, 0x300, 9).is_none());
    }

    /// A large window must not blow up the mapping: only the pages up to
    /// the pin are requested.
    #[test]
    fn only_the_pages_up_to_the_pin_are_mapped() {
        let w = pin_window(0xFD00_0000, 0x1_0000, 9).expect("pin 9");
        assert_eq!(w.pages, 1, "Pin 9 liegt in der ersten Seite");
        let w = pin_window(0xFD00_0000, 0x1_0000, 2000).expect("pin 2000");
        assert_eq!(w.reg_off, 8000);
        assert_eq!(w.pages, 2);
        // 16 pages is the kernel's limit.
        assert!(pin_window(0xFD00_0000, 0x10_0000, 20_000).is_none());
    }

    /// The level, and the polarity from the firmware.
    #[test]
    fn polarity_decides_what_asserted_means() {
        // Active low: bit 16 set means nothing pending.
        assert!(!asserted(PIN_STS, true));
        assert!(asserted(0, true));
        // Active high: the other way round.
        assert!(asserted(PIN_STS, false));
        assert!(!asserted(0, false));
        // Other bits must not matter: drive strength, pull-ups, wake — all
        // set, only bit 16 clear.
        let noise = 0xFFFF_FFFFu32 & !PIN_STS;
        assert!(asserted(noise, true));
        assert!(!asserted(noise, false));
    }

    /// A block that does not answer must not kill the pointer.
    #[test]
    fn all_ones_means_read_anyway() {
        assert!(asserted(u32::MAX, true));
        assert!(asserted(u32::MAX, false));
    }

    /// The register layout is AMD-specific; an Intel block is not touched.
    #[test]
    fn only_an_amd_block_is_touched() {
        assert!(is_amd_block(&vec!["AMDI0030".to_string()]));
        assert!(is_amd_block(&vec!["PNP0C02".to_string(), "AMD0030".to_string()]));
        assert!(!is_amd_block(&vec!["INT34BB".to_string()]));
        assert!(!is_amd_block(&[]));
    }
}
