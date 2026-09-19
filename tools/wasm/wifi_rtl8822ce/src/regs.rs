//! Register und Bits, 1:1 aus Linux 6.18.26
//! `drivers/net/wireless/realtek/rtw88/reg.h` (und `pci.h`, `main.h`).
//!
//! Jede Zeile traegt ihre Quelle. Wer hier einen Wert aendert, ohne die
//! genannte Zeile gelesen zu haben, hat geraten — und genau das ist die
//! Regel, die bei diesem Chip nicht verhandelbar ist.
#![allow(dead_code)]

// ── PCI-Identitaet ───────────────────────────────────────────────
// rtw8822ce.c: PCI_DEVICE(PCI_VENDOR_ID_REALTEK, 0xC822) und 0xC82F.
pub const RTL_VENDOR: u16 = 0x10ec;
pub const RTL8822CE_DEVICE: u16 = 0xc822;
pub const RTL8822CE_DEVICE_ALT: u16 = 0xc82f;

/// pci.c `rtw_pci_io_mapping`: `u8 bar_id = 2` — NICHT BAR0.
pub const BAR_REG: u8 = 2;
/// 16 Seiten = 64 KiB. Die hoechste PCIe-Registeradresse, die rtw88 anfasst,
/// ist `RTK_PCI_TXBD_H2CQ_CSR` 0x1330; BB/RF liegen direkt im Fenster bis
/// ~0x5000. Der Kernel klemmt ohnehin auf die echte BAR-Groesse.
pub const BAR_PAGES: u16 = 16;

// ── Systemregister (reg.h) ───────────────────────────────────────
pub const REG_SYS_FUNC_EN: u32 = 0x0002; // reg.h:8
pub const REG_SYS_PW_CTRL: u32 = 0x0004; // reg.h:18
pub const REG_RSV_CTRL: u32 = 0x001C; // reg.h:33
pub const REG_MCUFW_CTRL: u32 = 0x0080; // reg.h:131
pub const REG_SYS_CFG1: u32 = 0x00F0; // reg.h:186
pub const REG_SYS_STATUS1: u32 = 0x00F4; // reg.h:202
pub const REG_SYS_CFG2: u32 = 0x00FC; // reg.h:204
pub const REG_CR: u32 = 0x0100; // reg.h:207

// REG_SYS_CFG1-Felder, reg.h:187-201
pub const BIT_RTL_ID: u32 = 1 << 23;
pub const BIT_LDO: u32 = 1 << 24;
pub const BIT_RF_TYPE_ID: u32 = 1 << 27;
pub const BIT_SHIFT_VENDOR_ID: u32 = 16;
pub const BIT_MASK_VENDOR_ID: u32 = 0xf;
pub const BIT_SHIFT_CHIP_VER: u32 = 12;
pub const BIT_MASK_CHIP_VER: u32 = 0xf;

/// reg.h:201 — `BIT_GET_CHIP_VER(x)`
#[inline]
pub fn bit_get_chip_ver(x: u32) -> u8 {
    ((x >> BIT_SHIFT_CHIP_VER) & BIT_MASK_CHIP_VER) as u8
}

/// reg.h:195 — `BIT_GET_VENDOR_ID(x)`
#[inline]
pub fn bit_get_vendor_id(x: u32) -> u8 {
    ((x >> BIT_SHIFT_VENDOR_ID) & BIT_MASK_VENDOR_ID) as u8
}

/// mac.h:9 — `cut_version_to_mask(cut)`. Waehlt in der Power-Sequenz aus,
/// welche Kommandos fuer DIESEN Chipschnitt gelten.
/// In C ist das ein int-Shift, der erst bei der Zuweisung auf 8 Bit
/// verkuerzt wird — `1u8 << n` waere ab cut 7 etwas anderes.
#[inline]
pub fn cut_version_to_mask(cut: u8) -> u8 {
    (1u32 << (cut as u32 + 1)) as u8
}

// ── Zustaende, die Linux als Klartextvergleich liest ─────────────
/// mac.c `rtw_mac_power_switch`: `rtw_read8(rtwdev, REG_CR) == 0xea`
/// heisst „MAC ist AUS". Ein 8-Bit-Lesezugriff, ausdruecklich.
pub const CR_POWER_OFF: u8 = 0xea;
/// mac.c `rtw_mac_power_switch`: `rtw_read16(rtwdev, REG_MCUFW_CTRL) == 0xC078`
/// heisst „die Firmware laeuft noch".
pub const MCUFW_CTRL_FW_ALIVE: u16 = 0xc078;

// ── HCI-Parameter, main.c `rtw_chip_parameter_setup` ─────────────
pub const PCIE_RPWM_ADDR: u32 = 0x03d9;
pub const PCIE_CPWM_ADDR: u32 = 0x03da;

// ── Erwartung fuer den 8822C (rtw8822c.c `rtw8822c_hw_spec`) ─────
pub const TX_PKT_DESC_SZ: u32 = 48;
pub const TX_BUF_DESC_SZ: u32 = 16;
pub const RX_PKT_DESC_SZ: u32 = 24;
pub const RX_BUF_DESC_SZ: u32 = 8;
pub const PHY_EFUSE_SIZE: u32 = 512;
pub const LOG_EFUSE_SIZE: u32 = 768;
pub const PTCT_EFUSE_SIZE: u32 = 124;
