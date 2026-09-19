//! ERZEUGT von gen_pwrseq.py aus Linux 6.18.26 rtw8822c.c — nicht von Hand
//! aendern. Die vier Power-Sequenz-Tabellen des RTL8822C, Feld fuer Feld.
//!
//! Reihenfolge der Felder wie `struct rtw_pwr_seq_cmd` (main.h:955):
//! offset, cut_mask, intf_mask, base, cmd, mask, value.
#![allow(dead_code)]

/// `struct rtw_pwr_seq_cmd` — `base` und `cmd` sind in C 4-Bit-Bitfelder in
/// EINEM Byte; hier zwei Felder, weil der Interpreter sie einzeln liest.
#[derive(Clone, Copy)]
pub struct PwrCmd {
    pub offset: u16,
    pub cut_mask: u8,
    pub intf_mask: u8,
    pub base: u8,
    pub cmd: u8,
    pub mask: u8,
    pub value: u8,
}

pub const RTW_PWR_CMD_READ: u8 = 0x00;
pub const RTW_PWR_CMD_WRITE: u8 = 0x01;
pub const RTW_PWR_CMD_POLLING: u8 = 0x02;
pub const RTW_PWR_CMD_DELAY: u8 = 0x03;
pub const RTW_PWR_CMD_END: u8 = 0x04;

pub const RTW_PWR_ADDR_MAC: u8 = 0x00;
pub const RTW_PWR_ADDR_SDIO: u8 = 0x03;

pub const RTW_PWR_INTF_PCI_MSK: u8 = 1 << 2;

pub const RTW_PWR_DELAY_US: u8 = 0;
pub const RTW_PWR_DELAY_MS: u8 = 1;

/// main.h:922
pub const RTW_PWR_POLLING_CNT: u32 = 20000;

/// rtw8822c.c `trans_carddis_to_cardemu_8822c` — 8 Kommandos
pub static TRANS_CARDDIS_TO_CARDEMU: [PwrCmd; 8] = [
    PwrCmd { offset: 0x0086, cut_mask: 0xff, intf_mask: 0x01, base: 3, cmd: 1, mask: 0x01, value: 0x00 },
    PwrCmd { offset: 0x0086, cut_mask: 0xff, intf_mask: 0x01, base: 3, cmd: 2, mask: 0x02, value: 0x02 },
    PwrCmd { offset: 0x002e, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x04, value: 0x04 },
    PwrCmd { offset: 0x002d, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x01, value: 0x00 },
    PwrCmd { offset: 0x007f, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x80, value: 0x00 },
    PwrCmd { offset: 0x004a, cut_mask: 0xff, intf_mask: 0x02, base: 0, cmd: 1, mask: 0x01, value: 0x00 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x98, value: 0x00 },
    PwrCmd { offset: 0xffff, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 4, mask: 0x00, value: 0x00 },
];

/// rtw8822c.c `trans_cardemu_to_act_8822c` — 22 Kommandos
pub static TRANS_CARDEMU_TO_ACT: [PwrCmd; 22] = [
    PwrCmd { offset: 0x0000, cut_mask: 0xff, intf_mask: 0x03, base: 0, cmd: 1, mask: 0x20, value: 0x00 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x1c, value: 0x00 },
    PwrCmd { offset: 0x0075, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0x01, value: 0x01 },
    PwrCmd { offset: 0x0006, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 2, mask: 0x02, value: 0x02 },
    PwrCmd { offset: 0x0075, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0x01, value: 0x00 },
    PwrCmd { offset: 0xff1a, cut_mask: 0xff, intf_mask: 0x02, base: 0, cmd: 1, mask: 0xff, value: 0x00 },
    PwrCmd { offset: 0x002e, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x08, value: 0x00 },
    PwrCmd { offset: 0x0006, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x01, value: 0x01 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x18, value: 0x00 },
    PwrCmd { offset: 0x1018, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x04, value: 0x04 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x01, value: 0x01 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 2, mask: 0x01, value: 0x00 },
    PwrCmd { offset: 0x0074, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0x20, value: 0x20 },
    PwrCmd { offset: 0x0071, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0x10, value: 0x00 },
    PwrCmd { offset: 0x0062, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0xe0, value: 0xe0 },
    PwrCmd { offset: 0x0061, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0xe0, value: 0x00 },
    PwrCmd { offset: 0x001f, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0xc0, value: 0x80 },
    PwrCmd { offset: 0x00ef, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0xc0, value: 0x80 },
    PwrCmd { offset: 0x1045, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x10, value: 0x10 },
    PwrCmd { offset: 0x0010, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x04, value: 0x04 },
    PwrCmd { offset: 0x1064, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x02, value: 0x02 },
    PwrCmd { offset: 0xffff, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 4, mask: 0x00, value: 0x00 },
];

/// rtw8822c.c `trans_act_to_cardemu_8822c` — 12 Kommandos
pub static TRANS_ACT_TO_CARDEMU: [PwrCmd; 12] = [
    PwrCmd { offset: 0x0093, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x08, value: 0x00 },
    PwrCmd { offset: 0x001f, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0xff, value: 0x00 },
    PwrCmd { offset: 0x00ef, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0xff, value: 0x00 },
    PwrCmd { offset: 0x1045, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x10, value: 0x00 },
    PwrCmd { offset: 0xff1a, cut_mask: 0xff, intf_mask: 0x02, base: 0, cmd: 1, mask: 0xff, value: 0x30 },
    PwrCmd { offset: 0x0049, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x02, value: 0x00 },
    PwrCmd { offset: 0x0006, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x01, value: 0x01 },
    PwrCmd { offset: 0x0002, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x02, value: 0x00 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x02, value: 0x02 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 2, mask: 0x02, value: 0x00 },
    PwrCmd { offset: 0x0000, cut_mask: 0xff, intf_mask: 0x03, base: 0, cmd: 1, mask: 0x20, value: 0x20 },
    PwrCmd { offset: 0xffff, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 4, mask: 0x00, value: 0x00 },
];

/// rtw8822c.c `trans_cardemu_to_carddis_8822c` — 12 Kommandos
pub static TRANS_CARDEMU_TO_CARDDIS: [PwrCmd; 12] = [
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x01, base: 0, cmd: 1, mask: 0x80, value: 0x80 },
    PwrCmd { offset: 0x0007, cut_mask: 0xff, intf_mask: 0x03, base: 0, cmd: 1, mask: 0xff, value: 0x00 },
    PwrCmd { offset: 0x0067, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x20, value: 0x00 },
    PwrCmd { offset: 0x004a, cut_mask: 0xff, intf_mask: 0x02, base: 0, cmd: 1, mask: 0x01, value: 0x00 },
    PwrCmd { offset: 0x0081, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0xc0, value: 0x00 },
    PwrCmd { offset: 0x0090, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 1, mask: 0x02, value: 0x00 },
    PwrCmd { offset: 0x0092, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0xff, value: 0x20 },
    PwrCmd { offset: 0x0093, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0xff, value: 0x04 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x03, base: 0, cmd: 1, mask: 0x18, value: 0x08 },
    PwrCmd { offset: 0x0005, cut_mask: 0xff, intf_mask: 0x04, base: 0, cmd: 1, mask: 0x04, value: 0x04 },
    PwrCmd { offset: 0x0086, cut_mask: 0xff, intf_mask: 0x01, base: 3, cmd: 1, mask: 0x01, value: 0x01 },
    PwrCmd { offset: 0xffff, cut_mask: 0xff, intf_mask: 0x0f, base: 0, cmd: 4, mask: 0x00, value: 0x00 },
];

/// rtw8822c.c `card_enable_flow_8822c`
pub static CARD_ENABLE_FLOW: [&[PwrCmd]; 2] =
    [&TRANS_CARDDIS_TO_CARDEMU, &TRANS_CARDEMU_TO_ACT];

/// rtw8822c.c `card_disable_flow_8822c`
pub static CARD_DISABLE_FLOW: [&[PwrCmd]; 2] =
    [&TRANS_ACT_TO_CARDEMU, &TRANS_CARDEMU_TO_CARDDIS];
