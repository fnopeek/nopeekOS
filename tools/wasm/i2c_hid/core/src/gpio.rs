//! Der GPIO-Block des AMD-FCH — nur so viel, wie die eine Frage braucht:
//! **liegt gerade ein Bericht an?**
//!
//! Portiert aus Linux 6.18 `drivers/pinctrl/pinctrl-amd.{c,h}`. Dort steht
//! beides, was hier gerechnet wird: ein Register je Pin bei `base + pin * 4`
//! (`amd_gpio_get_value`) und der LEBENDE Pegel in Bit 16 (`PIN_STS_OFF`).
//! `base` ist der erste `Memory32Fixed` des ACPI-Geraets — dieselbe Quelle,
//! aus der Linux' `devm_platform_get_and_ioremap_resource(pdev, 0, …)`
//! schoepft, also keine festverdrahtete 0xFED81500.
//!
//! **Warum das ueberhaupt gebraucht wird.** HID over I2C ist
//! PEGELgesteuert: das Geraet zieht seine Leitung, sobald ein Bericht
//! bereitliegt, und laesst sie erst los, wenn der Bericht geholt ist.
//! Linux ruft `i2c_hid_get_input` deshalb ausschliesslich aus
//! `i2c_hid_irq`. Ohne Interrupt lasen wir blind — und ein blinder Versuch
//! ist nicht billig: `wMaxInputLength` sind 64 Bytes, bei 400 kHz also
//! 1,4 ms, in denen der Kern auf dem Bus wartet. Ein Blick auf den Pin
//! kostet EIN Register.
//!
//! Der Registeraufbau ist AMD-eigen. Ein Intel-Block (`INT34BB` auf
//! Florians HP) fuehrt an derselben Stelle etwas anderes — deshalb wird
//! [`is_amd_block`] gefragt, bevor irgendetwas gelesen wird, und nicht
//! geraten.

use alloc::string::String;

/// `PIN_STS_OFF` (pinctrl-amd.h) — der Pegel, den der Pin GERADE fuehrt.
pub const PIN_STS: u32 = 1 << 16;

/// Die ACPI-Kennungen, unter denen Linux `pinctrl-amd` bindet
/// (`amd_gpio_acpi_match`).
pub const AMD_GPIO_IDS: [&str; 3] = ["AMD0030", "AMDI0030", "AMDI0031"];

/// Ist der GPIO-Block einer, dessen Register wir kennen?
pub fn is_amd_block(ids: &[String]) -> bool {
    ids.iter().any(|i| AMD_GPIO_IDS.iter().any(|a| i == a))
}

/// Wohin der Pin abgebildet werden muss, und wo sein Register dann liegt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinWindow {
    /// Seitenausgerichtete Basis — `npk_mmio_map_phys` weist alles andere ab.
    pub map_base: u32,
    /// Wieviele Seiten, um DIESEN Pin zu erreichen.
    pub pages: u32,
    /// Versatz seines Registers, relativ zu `map_base`.
    pub reg_off: u32,
}

/// Das Fenster fuer einen Pin ausrechnen.
///
/// Der AMD-Block steht in der Firmware typisch als
/// `Memory32Fixed(ReadWrite, 0xFED81500, 0x300)` — also NICHT
/// seitenausgerichtet. Abgebildet wird deshalb ab der Seite darunter, und
/// der Rest der Adresse faehrt in `reg_off` mit.
///
/// `None`, wenn der Pin ausserhalb des angesagten Fensters liegt: dann
/// haben wir den falschen Block oder den falschen Pin, und ein Register
/// daneben zu lesen waere geraten.
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
    // Nur so viele Seiten, wie dieser Pin braucht — der ganze Block waere
    // bei einem 64-KB-Fenster mehr als die sechzehn, die der Kernel gibt.
    let pages = (reg_off + 4 + 4095) / 4096;
    if pages == 0 || pages > 16 {
        return None;
    }
    Some(PinWindow { map_base, pages, reg_off })
}

// ── Interrupts: `pinctrl-amd.{c,h}` ──────────────────────────────────
//
// Ein Register je Pin; die Bits aus pinctrl-amd.h. Der Block hat EINE
// Leitung fuer alle Pins (`_CRS`), und `do_amd_gpio_irq_handler` quittiert
// zuerst den Pin (das gelesene Register zurueckschreiben loescht die
// Statusbits) und dann die Einheit (`EOI_MASK` im WAKE_INT_MASTER_REG).

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
/// from before is cleared. Returns the value WITHOUT the enable bit; the
/// caller does the debounce-settle dance and then `amd_gpio_irq_enable`.
pub fn irq_level_config(pin_reg: u32, active_low: bool) -> u32 {
    let mut v = pin_reg | LEVEL_TRIG;
    v &= !ACTIVE_LEVEL_MASK;
    v |= (if active_low { 1 } else { 0 }) << ACTIVE_LEVEL_SHIFT;
    v | INTERRUPT_STS
}

/// Sagt der Pin „ich habe etwas"?
///
/// `active_low` kommt aus dem `GpioInt` der Firmware (ACPI: `int_flags`
/// Bits 2:1), nicht aus einer Annahme.
///
/// **Lauter Einsen heissen: da hat niemand geantwortet.** Dann meldet
/// diese Funktion JA und der Rufer liest. Eine Fehlmeldung kostet eine
/// Busuebertragung; ein verschlucktes Ja kostet den Zeiger.
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

    /// `amd_gpio_irq_set_type(IRQ_TYPE_LEVEL_LOW)`: Pegel, aktiv-niedrig,
    /// Status loeschen — und die Pull-/Ausgangsbits unangetastet.
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

    /// Die zwei Pins aus Florians IdeaPad, gegen den ueblichen AMD-Block.
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

    /// Ein Pin ausserhalb des angesagten Fensters ist kein Pin.
    #[test]
    fn a_pin_past_the_window_is_refused() {
        // 0x300 Bytes sind 192 Register: 0..=191.
        assert!(pin_window(0xFED8_1500, 0x300, 191).is_some());
        assert!(pin_window(0xFED8_1500, 0x300, 192).is_none());
        assert!(pin_window(0, 0x300, 9).is_none());
    }

    /// Ein grosses Fenster darf nicht die ganze Abbildung sprengen: es
    /// werden nur die Seiten bis zum Pin verlangt.
    #[test]
    fn only_the_pages_up_to_the_pin_are_mapped() {
        let w = pin_window(0xFD00_0000, 0x1_0000, 9).expect("pin 9");
        assert_eq!(w.pages, 1, "Pin 9 liegt in der ersten Seite");
        let w = pin_window(0xFD00_0000, 0x1_0000, 2000).expect("pin 2000");
        assert_eq!(w.reg_off, 8000);
        assert_eq!(w.pages, 2);
        // 16 Seiten sind der Deckel des Kernels.
        assert!(pin_window(0xFD00_0000, 0x10_0000, 20_000).is_none());
    }

    /// Der Pegel, und die Polaritaet aus der Firmware.
    #[test]
    fn polarity_decides_what_asserted_means() {
        // Aktiv LOW: Bit 16 gesetzt heisst „nichts da".
        assert!(!asserted(PIN_STS, true));
        assert!(asserted(0, true));
        // Aktiv HIGH: genau andersherum.
        assert!(asserted(PIN_STS, false));
        assert!(!asserted(0, false));
        // Andere Bits duerfen nicht mitreden: Treiberstaerke, Pull-ups,
        // Wake — alles gesetzt, nur Bit 16 nicht.
        let noise = 0xFFFF_FFFFu32 & !PIN_STS;
        assert!(asserted(noise, true));
        assert!(!asserted(noise, false));
    }

    /// Ein Block, der gar nicht antwortet, darf den Zeiger nicht toeten.
    #[test]
    fn all_ones_means_read_anyway() {
        assert!(asserted(u32::MAX, true));
        assert!(asserted(u32::MAX, false));
    }

    /// Der Registeraufbau ist AMD-eigen — ein Intel-Block wird nicht
    /// angefasst.
    #[test]
    fn only_an_amd_block_is_touched() {
        assert!(is_amd_block(&vec!["AMDI0030".to_string()]));
        assert!(is_amd_block(&vec!["PNP0C02".to_string(), "AMD0030".to_string()]));
        assert!(!is_amd_block(&vec!["INT34BB".to_string()]));
        assert!(!is_amd_block(&[]));
    }
}
