//! `fw.c` from Linux 6.18.26 rtw88: reserved page download, the two H2C
//! paths, C2H parsing and the watchdog's firmware updates.
#![allow(dead_code)]

use crate::host;
use crate::pci::{self, Trx};
use crate::regs::*;

/// PCI configuration space: command and status.
///
/// The command register tells whether the chip is bus master at all;
/// without that it cannot fetch descriptors from memory, which shows up as
/// working MMIO and failing DMA.
///
/// The status register is the other half: bit 13 (received master abort)
/// and bit 12 (received target abort) are set when the chip tried and was
/// rejected. If they stay clear with bus mastering on, it never tried.
pub fn dump_pci_cmd(tag: &str) {
    let v = host::pci_read_config(0x04);
    let cmd = (v & 0xFFFF) as u16;
    let sts = (v >> 16) as u16;
    host::print("    PCI cfg04 ");
    host::print(tag);
    host::print(": cmd=0x");
    host::print_hex16(cmd);
    host::print(" (io ");
    host::print(if cmd & 1 != 0 { "an" } else { "AUS" });
    host::print(", mem ");
    host::print(if cmd & 2 != 0 { "an" } else { "AUS" });
    host::print(", busmaster ");
    host::print(if cmd & 4 != 0 { "an" } else { "AUS" });
    host::print(")  status=0x");
    host::print_hex16(sts);
    if sts & (1 << 13) != 0 {
        host::print(" MASTER-ABORT");
    }
    if sts & (1 << 12) != 0 {
        host::print(" TARGET-ABORT");
    }
    if sts & (1 << 8) != 0 {
        host::print(" PARITY");
    }
    host::print("\n");
}

pub fn dump_reg32(h: i32, name: &str, off: u32) {
    host::print("    ");
    host::print(name);
    host::print(" @0x");
    host::print_hex16(off as u16);
    host::print(" = 0x");
    host::print_hex32(host::r32(h, off));
    host::print("\n");
}

/// util.c `check_hw_ready`: 1000 rounds 10 us apart, read with
/// `rtw_read32_mask`, i.e. 32 bits even if the register is a byte.
pub fn check_hw_ready(h: i32, addr: u32, mask: u32, target: u32) -> bool {
    check_hw_ready_for(h, addr, mask, target, LINUX_FRIST_US).0
}

/// Linux: 1000 rounds of `udelay(10)`, so the deadline is 10 ms; the
/// round count is only how it is counted there.
pub const LINUX_FRIST_US: u64 = 1000 * 10;

/// util.c `check_hw_ready`, timed by the clock instead of a round count,
/// since a tight loop of 1000 reads takes far less than Linux's 10 ms.
///
/// Returns whether it succeeded and how long it took; the second value
/// distinguishes "too tight" from "never comes".
pub fn check_hw_ready_for(
    h: i32, addr: u32, mask: u32, target: u32, frist_us: u64,
) -> (bool, u64) {
    let shift = mask.trailing_zeros();
    let start = host::now_us();
    loop {
        if (host::r32(h, addr) & mask) >> shift == target {
            return (true, host::now_us() - start);
        }
        let waited = host::now_us() - start;
        if waited >= frist_us {
            return (false, waited);
        }
        // 10 us is below our sleep resolution, so read tightly. After 10 ms yield
        // between reads so a long deadline does not block the core.
        if waited > 10_000 {
            host::sleep_ms(1);
        }
    }
}

/// fw.c `rtw_fw_write_data_rsvd_page`, 3081 branch, PCIe.
///
/// `rsvd_boundary` is still 0 during the firmware download: it is set by
/// `rtw_set_trx_fifo_info` in `rtw_mac_init`, which runs after the
/// download. Linux writes back 0 here as well.
#[allow(clippy::too_many_arguments)]
pub fn write_data_rsvd_page(
    h: i32, trx: &Trx, stage: i32, pg_addr: u16, payload: &[u8],
    rsvd_boundary: u16, current_band_type: u8, verbose: bool,
) -> bool {
    if payload.is_empty() {
        return false;
    }

    let bckp2 = host::r8(h, REG_BCN_CTRL);

    let pg = (pg_addr & BIT_MASK_BCN_HEAD_1_V1) | (BIT_BCN_VALID_V1 as u16);
    host::w16(h, REG_FIFOPAGE_CTRL_2, pg);

    let bckp0 = host::r8(h, REG_CR + 1);
    host::w8(h, REG_CR + 1, bckp0 | (BIT_ENSWBCN >> 8) as u8);

    host::w8(h, REG_BCN_CTRL, (bckp2 & !BIT_EN_BCN_FUNCTION) | BIT_DIS_TSF_UDT);

    // PCIe: disable the queue's beacon download while we use it.
    let bckp1 = host::r8(h, REG_FWHW_TXQ_CTRL + 2);
    host::w8(h, REG_FWHW_TXQ_CTRL + 2, bckp1 & !((BIT_EN_BCNQ_DL >> 16) as u8));

    // `rtw_hci_write_data_rsvd_page` -> `rtw_pci_write_data_rsvd_page`
    if verbose {
        host::print("  [dump] vor dem Schreiben:\n");
        dump_reg32(h, "CR        ", REG_CR);
        dump_reg32(h, "FIFOPG_C2 ", REG_FIFOPAGE_CTRL_2);
        dump_reg32(h, "FIFOPG_I1 ", REG_FIFOPAGE_INFO_1);
        dump_reg32(h, "RQPN_CTRL2", REG_RQPN_CTRL_2);
        dump_reg32(h, "FWHW_TXQ  ", REG_FWHW_TXQ_CTRL);
        dump_reg32(h, "BCN_CTRL  ", REG_BCN_CTRL);
        dump_reg32(h, "TXDMA_STAT", REG_TXDMA_STATUS);
        dump_reg32(h, "TXDMA_PQ  ", REG_TXDMA_PQ_MAP);
        dump_reg32(h, "H2CQ_CSR  ", REG_H2CQ_CSR);
        dump_reg32(h, "PCI_CTRL  ", pci::RTK_PCI_CTRL);
        dump_reg32(h, "MCUFW_CTRL", REG_MCUFW_CTRL);
        dump_pci_cmd("vorher ");
    }

    let mut ok = pci::write_data_rsvd_page(h, trx, stage, payload, current_band_type, verbose);

    if ok && !check_hw_ready(h, REG_FIFOPAGE_CTRL_2, BIT_BCN_VALID_V1, 1) {
        host::print("[rtl8822ce] error beacon valid\n");
        if verbose {
            host::print("  [dump] nach dem Anstoss:\n");
            dump_reg32(h, "FIFOPG_C2 ", REG_FIFOPAGE_CTRL_2);
            dump_reg32(h, "TXDMA_STAT", REG_TXDMA_STATUS);
            dump_reg32(h, "PCI_CTRL  ", pci::RTK_PCI_CTRL);
            dump_reg32(h, "CR        ", REG_CR);
            dump_reg32(h, "FWHW_TXQ  ", REG_FWHW_TXQ_CTRL);
            // 0x382 is not 4-byte aligned; a 32-bit read there returns 0xffffffff.
            host::print("    BCN_WORK  @0x383 = 0x");
            host::print_hex8(host::r8(h, pci::RTK_PCI_TXBD_BCN_WORK));
            host::print("   (noch gesetzt = nie abgeholt)\n");
            host::print("    BCN-Ring[0..16] =");
            for i in 0..4 {
                host::print(" 0x");
                host::print_hex32(host::dma_r32(trx.tx[pci::Q_BCN].handle, i * 4));
            }
            host::print("\n      OWN-Bit ");
            let psb = host::dma_r32(trx.tx[pci::Q_BCN].handle, 0) >> 16;
            host::print(if psb & 0x8000 != 0 {
                "steht noch -> der Chip hat den Eintrag nie angefasst\n"
            } else {
                "ist GELOESCHT -> der Chip hat ihn geholt\n"
            });
            dump_pci_cmd("nachher");
        }
        ok = false;
    }

    // restore
    host::w16(h, REG_FIFOPAGE_CTRL_2, rsvd_boundary | (BIT_BCN_VALID_V1 as u16));
    host::w8(h, REG_BCN_CTRL, bckp2);
    host::w8(h, REG_FWHW_TXQ_CTRL + 2, bckp1);
    host::w8(h, REG_CR + 1, bckp0);

    ok
}

// ════════════════════════════════════════════════════════════════
// The two H2C paths
// ════════════════════════════════════════════════════════════════
//
// They look alike and are not:
//
//   `rtw_fw_send_h2c_command`  writes eight bytes into one of four
//                              HMEBOX registers. Coexistence uses this.
//   `rtw_fw_send_h2c_packet`   pushes 32 bytes through the H2C queue, the
//                              ring whose address `init_h2c` set. General
//                              and PHYDM info use this.
//
// Mixing them up sends everything into the void, silently, since neither
// path reports back.

/// `struct rtw_h2c_cmd` (fw.h): eight bytes as two words.
/// The state between two commands: which mailbox is next and which
/// sequence number a packet carries.
#[derive(Default, Clone, Copy)]
/// `struct rtw_dev.h2c` (main.h): one instance for the whole device.
///
/// The mailbox order is the point: the driver rotates through them so the
/// firmware has time to drain the previous one, and `seq` must not restart.
pub struct H2cState {
    pub last_box_num: u8,
    pub seq: u8,
}

/// The four mailbox flags as the chip currently reports them.
pub fn hmetfr(h: i32) -> u8 {
    host::r8(h, REG_HMETFR)
}

/// Place a field at its shift position in word `word` of the H2C buffer.
/// This is `le32p_replace_bits((__le32 *)(h2c) + word, value, mask)`.
fn h2c_set(pkt: &mut [u8; H2C_PKT_SIZE], word: usize, mask: u32, value: u32) {
    let o = word * 4;
    let cur = u32::from_le_bytes([pkt[o], pkt[o + 1], pkt[o + 2], pkt[o + 3]]);
    let v = (cur & !mask) | ((value << mask.trailing_zeros()) & mask);
    pkt[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// ── Path 1: the mailbox (fw.c:76-127) ────────────────────────────

/// fw.c `rtw_fw_send_h2c_command`.
///
/// Linux polls with `read_poll_timeout_atomic(rtw_read8, ..., 100, 3000, ...)`,
/// every 100 us with a 3 ms deadline. The deadline is enforced here, not
/// the round count.
pub fn send_h2c_command(h: i32, st: &mut H2cState, pkt: &[u8; H2C_PKT_SIZE]) -> bool {
    let box_num = st.last_box_num;
    let (box_reg, box_ex_reg) = match box_num {
        0 => (REG_HMEBOX0, REG_HMEBOX0_EX),
        1 => (REG_HMEBOX1, REG_HMEBOX1_EX),
        2 => (REG_HMEBOX2, REG_HMEBOX2_EX),
        3 => (REG_HMEBOX3, REG_HMEBOX3_EX),
        _ => {
            host::print("[rtl8822ce] invalid h2c mail box number\n");
            return false;
        }
    };

    let start = host::now_us();
    loop {
        let flags = host::r8(h, REG_HMETFR);
        if (flags >> box_num) & 0x1 == 0 {
            break;
        }
        if host::now_us() - start >= 3000 {
            // Linux only says "failed to send h2c command"; the mailbox and flags
            // are added for diagnosis.
            host::print("[rtl8822ce] failed to send h2c command (Fach ");
            host::print_dec(box_num as u32);
            host::print(", HMETFR 0x");
            host::print_hex8(flags);
            host::print(")\n");
            return false;
        }
        host::delay_us(100);
    }

    // `h2c_cmd->msg` is bytes 0..4, `msg_ext` bytes 4..8, and the EX register
    // is written first.
    let msg = u32::from_le_bytes([pkt[0], pkt[1], pkt[2], pkt[3]]);
    let msg_ext = u32::from_le_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
    host::w32(h, box_ex_reg, msg_ext);
    host::w32(h, box_reg, msg);

    st.last_box_num += 1;
    if st.last_box_num >= 4 {
        st.last_box_num = 0;
    }
    true
}

/// fw.h:586 `SET_H2C_CMD_ID_CLASS`
fn set_cmd_id_class(pkt: &mut [u8; H2C_PKT_SIZE], value: u32) {
    h2c_set(pkt, 0, 0xff, value);
}

/// fw.c `rtw_fw_bt_wifi_control`, fw.h:568-574, command 0x69.
pub fn bt_wifi_control(h: i32, st: &mut H2cState, op_code: u8, data: &[u8; 5]) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_BT_WIFI_CONTROL);
    h2c_set(&mut pkt, 0, 0x0000_ff00, op_code as u32); // OP_CODE
    h2c_set(&mut pkt, 0, 0x00ff_0000, data[0] as u32); // DATA1
    h2c_set(&mut pkt, 0, 0xff00_0000, data[1] as u32); // DATA2
    h2c_set(&mut pkt, 1, 0x0000_00ff, data[2] as u32); // DATA3
    h2c_set(&mut pkt, 1, 0x0000_ff00, data[3] as u32); // DATA4
    h2c_set(&mut pkt, 1, 0x00ff_0000, data[4] as u32); // DATA5
    send_h2c_command(h, st, &pkt)
}

/// fw.c `rtw_fw_query_bt_info`, command 0x61.
pub fn query_bt_info(h: i32, st: &mut H2cState) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_QUERY_BT_INFO);
    h2c_set(&mut pkt, 0, 1 << 8, 1); // SET_QUERY_BT_INFO(h2c_pkt, true)
    send_h2c_command(h, st, &pkt)
}

// ── Path 2: the packet through the H2C queue (fw.c:495-565) ──────

/// fw.c `rtw_h2c_pkt_set_header`
fn h2c_pkt_set_header(pkt: &mut [u8; H2C_PKT_SIZE], sub_id: u32) {
    h2c_set(pkt, 0, 0x0000_007f, H2C_PKT_CATEGORY); // SET_PKT_H2C_CATEGORY
    h2c_set(pkt, 0, 0x0000_ff00, H2C_PKT_CMD_ID); // SET_PKT_H2C_CMD_ID
    h2c_set(pkt, 0, 0xffff_0000, sub_id); // SET_PKT_H2C_SUB_CMD_ID
}

/// fw.c `rtw_fw_send_h2c_packet`
fn send_h2c_packet(h: i32, trx: &mut Trx, stage: i32, st: &mut H2cState,
                   pkt: &mut [u8; H2C_PKT_SIZE]) -> bool {
    h2c_set(pkt, 1, 0xffff_0000, st.seq as u32); // FW_OFFLOAD_H2C_SET_SEQ_NUM
    let ok = crate::pci::write_data_h2c(h, trx, stage, pkt);
    if !ok {
        host::print("[rtl8822ce] failed to send h2c packet\n");
    }
    // Linux increments `seq` even on failure.
    st.seq = st.seq.wrapping_add(1);
    ok
}

/// fw.c:517-535 `rtw_fw_send_general_info`.
///
/// Tells the firmware how many pages it has behind `rsvd_boundary` for its
/// own TX buffer.
pub fn send_general_info(h: i32, trx: &mut Trx, stage: i32, st: &mut H2cState,
                         fifo: &crate::mac::Fifo) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    let total_size = H2C_PKT_HDR_SIZE + 4;

    h2c_pkt_set_header(&mut pkt, H2C_PKT_GENERAL_INFO);
    h2c_set(&mut pkt, 1, 0x0000_ffff, total_size as u32); // SET_PKT_H2C_TOTAL_LEN
    h2c_set(&mut pkt, 2, 0x00ff_0000, // GENERAL_INFO_SET_FW_TX_BOUNDARY
            (fifo.rsvd_fw_txbuf_addr - fifo.rsvd_boundary) as u32);

    send_h2c_packet(h, trx, stage, st, &mut pkt)
}

/// fw.c:538-565 `rtw_fw_send_phydm_info`.
///
/// `rx_ant_status`/`tx_ant_status` come from `hal->antenna_rx`/`antenna_tx`,
/// both `BB_PATH_AB` for 2T2R.
#[allow(clippy::too_many_arguments)]
pub fn send_phydm_info(h: i32, trx: &mut Trx, stage: i32, st: &mut H2cState,
                       rfe_option: u8, rf_2t2r: bool, cut_version: u8,
                       antenna_rx: u8, antenna_tx: u8) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    let total_size = H2C_PKT_HDR_SIZE + 8;
    let fw_rf_type = if rf_2t2r { FW_RF_2T2R } else { FW_RF_1T1R };

    h2c_pkt_set_header(&mut pkt, H2C_PKT_PHYDM_INFO);
    h2c_set(&mut pkt, 1, 0x0000_ffff, total_size as u32);
    h2c_set(&mut pkt, 2, 0x0000_00ff, rfe_option as u32); // REF_TYPE
    h2c_set(&mut pkt, 2, 0x0000_ff00, fw_rf_type as u32); // RF_TYPE
    h2c_set(&mut pkt, 2, 0x00ff_0000, cut_version as u32); // CUT_VER
    h2c_set(&mut pkt, 2, 0x0f00_0000, antenna_rx as u32); // RX_ANT_STATUS
    h2c_set(&mut pkt, 2, 0xf000_0000, antenna_tx as u32); // TX_ANT_STATUS

    send_h2c_packet(h, trx, stage, st, &mut pkt)
}

/// fw.c `rtw_fw_coex_tdma_type`, mailbox command 0x60, fw.h:567.
#[allow(clippy::too_many_arguments)]
pub fn coex_tdma_type(h: i32, st: &mut H2cState,
                      para1: u8, para2: u8, para3: u8, para4: u8, para5: u8) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_COEX_TDMA_TYPE);
    h2c_set(&mut pkt, 0, 0x0000_ff00, para1 as u32); // PARA1
    h2c_set(&mut pkt, 0, 0x00ff_0000, para2 as u32); // PARA2
    h2c_set(&mut pkt, 0, 0xff00_0000, para3 as u32); // PARA3
    h2c_set(&mut pkt, 1, 0x0000_00ff, para4 as u32); // PARA4
    h2c_set(&mut pkt, 1, 0x0000_ff00, para5 as u32); // PARA5
    send_h2c_command(h, st, &pkt)
}

/// fw.c:1080-1088 `rtw_fw_scan_notify`, command 0x59.
pub fn scan_notify(h: i32, st: &mut H2cState, start: bool) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_SCAN);
    h2c_set(&mut pkt, 0, 1 << 8, start as u32); // SET_SCAN_START
    send_h2c_command(h, st, &pkt)
}

/// fw.c:437-446 `rtw_fw_inform_rfk_status`, command 0x6d, mailbox.
pub fn inform_rfk_status(h: i32, st: &mut H2cState, start: bool) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_WIFI_CALIBRATION);
    h2c_set(&mut pkt, 0, 1 << 8, start as u32); // RFK_SET_INFORM_START
    send_h2c_command(h, st, &pkt)
}

/// fw.c:448-459 `rtw_fw_do_iqk`, via the queue, not the mailbox.
///
/// The firmware computes the IQK; the driver only triggers it and then
/// waits for `REG_RPT_CIP`.
pub fn do_iqk(h: i32, trx: &mut Trx, stage: i32, st: &mut H2cState,
              clear: bool, segment_iqk: bool) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    let total_size = H2C_PKT_HDR_SIZE + 1;
    h2c_pkt_set_header(&mut pkt, H2C_PKT_IQK as u32);
    h2c_set(&mut pkt, 1, 0x0000_ffff, total_size as u32); // SET_PKT_H2C_TOTAL_LEN
    h2c_set(&mut pkt, 2, 1 << 0, clear as u32); // IQK_SET_CLEAR
    h2c_set(&mut pkt, 2, 1 << 1, segment_iqk as u32); // IQK_SET_SEGMENT_IQK
    send_h2c_packet(h, trx, stage, st, &mut pkt)
}

/// fw.c:1123-1132 `rtw_fw_media_status_report`, command 0x01, mailbox.
///
/// Tells the firmware that this `mac_id` is connected; it then sets up its
/// own bookkeeping (rate adaptation, power saving).
pub fn media_status_report(h: i32, st: &mut H2cState, mac_id: u8,
                           connect: bool) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_MEDIA_STATUS_RPT);
    h2c_set(&mut pkt, 0, 1 << 8, connect as u32); // SET_OP_MODE
    h2c_set(&mut pkt, 0, 0x00ff_0000, mac_id as u32); // SET_MACID
    send_h2c_command(h, st, &pkt)
}

/// fw.h:73-77 `struct rtw_c2h_cmd`: ID, sequence number, payload.
pub struct C2hCmd<'a> {
    pub id: u8,
    pub seq: u8,
    pub payload: &'a [u8],
}

/// fw.c:334-380 `rtw_fw_c2h_cmd_handle`, the dispatcher.
///
/// Names the C2H IDs so unhandled messages are reported by name instead of
/// disappearing in a silent `_ =>`. The handlers behind them
/// (`rtw_tx_report_handle`, `rtw_coex_bt_info_notify`,
/// `rtw_fw_ra_report_handle`) are dispatched elsewhere or not ported.
pub fn c2h_name(id: u8) -> &'static str {
    // A table instead of `match`: the IDs in `regs.rs` are partly `u8` and
    // partly `u32`, and a pattern cannot carry a conversion.
    const NAMES: &[(u32, &str)] = &[
        (C2H_CCX_TX_RPT, "CCX_TX_RPT (Sendequittung)"),
        (C2H_BT_INFO, "BT_INFO"),
        (C2H_BT_MP_INFO, "BT_MP_INFO"),
        (C2H_BT_HID_INFO, "BT_HID_INFO"),
        (C2H_RA_RPT, "RA_RPT (Ratenanpassung)"),
        (C2H_HW_FEATURE_REPORT as u32, "HW_FEATURE_REPORT"),
        (C2H_WLAN_INFO, "WLAN_INFO"),
        (C2H_WLAN_RFON, "WLAN_RFON"),
        (C2H_BCN_FILTER_NOTIFY, "BCN_FILTER_NOTIFY"),
        (C2H_ADAPTIVITY, "ADAPTIVITY"),
        (C2H_SCAN_RESULT, "SCAN_RESULT"),
        (C2H_HW_FEATURE_DUMP as u32, "HW_FEATURE_DUMP"),
        (C2H_HALMAC, "HALMAC"),
    ];
    for &(k, v) in NAMES {
        if k == id as u32 {
            return v;
        }
    }
    "unbekannt"
}

/// Extract the C2H header from an RX buffer. `pkt_offset` is the same
/// offset as for a radio frame in Linux: descriptor + drv_info + shift.
pub fn c2h_parse(frame: &[u8]) -> Option<C2hCmd<'_>> {
    if frame.len() < 2 {
        return None;
    }
    Some(C2hCmd { id: frame[0], seq: frame[1], payload: &frame[2..] })
}

/// fw.c:1023-1063 `rtw_fw_send_ra_info`, command 0x40, mailbox.
///
/// The driver sends a mask, not a rate: which of the 64 rates the peer
/// supports. The firmware picks from it continuously and reports its choice
/// as `C2H_RA_RPT`.
///
/// The `H2C_CMD_RA_INFO_HI` part applies only to the 8814A (four TX
/// chains, mask wider than 32 bits); Linux returns at the same point for
/// the 8822C.
pub fn send_ra_info(h: i32, st: &mut H2cState, si: &mut crate::sta::StaInfo,
                    reset_ra_mask: bool) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_RA_INFO);

    h2c_set(&mut pkt, 0, 0x0000_ff00, si.mac_id as u32); // MACID
    h2c_set(&mut pkt, 0, 0x001f_0000, si.rate_id as u32); // RATE_ID
    h2c_set(&mut pkt, 0, 0x0060_0000, si.init_ra_lv as u32); // INIT_RA_LVL
    h2c_set(&mut pkt, 0, 1 << 23, si.sgi_enable as u32); // SGI_EN
    h2c_set(&mut pkt, 0, 0x0300_0000, si.bw_mode as u32); // BW_MODE
    h2c_set(&mut pkt, 0, 1 << 26, (si.ldpc_en != 0) as u32); // LDPC
    h2c_set(&mut pkt, 0, 1 << 27, (!reset_ra_mask) as u32); // NO_UPDATE
    // GENMASK(29, 28), two bits. For a `bool` a single-bit mask writes the
    // same value but would not clear bit 29; the source's mask is kept so a
    // later value > 1 is not silently mangled.
    h2c_set(&mut pkt, 0, 0x3000_0000, si.vht_enable as u32); // VHT_EN
    h2c_set(&mut pkt, 0, 1 << 30, 1); // DIS_PT; Linux: `disable_pt = true`
    h2c_set(&mut pkt, 1, 0x0000_00ff, (si.ra_mask & 0xff) as u32);
    h2c_set(&mut pkt, 1, 0x0000_ff00, ((si.ra_mask & 0xff00) >> 8) as u32);
    h2c_set(&mut pkt, 1, 0x00ff_0000, ((si.ra_mask & 0xff_0000) >> 16) as u32);
    h2c_set(&mut pkt, 1, 0xff00_0000, ((si.ra_mask & 0xff00_0000) >> 24) as u32);

    si.init_ra_lv = 0;
    send_h2c_command(h, st, &pkt)
}

/// fw.c:1065-1080 `rtw_fw_default_port`.
///
/// Returns early while the port is not connected, as the first line in
/// Linux does.
pub fn default_port(h: i32, st: &mut H2cState, port: u8, mac_id: u8,
                    net_type: u32) -> bool {
    if net_type != RTW_NET_MGD_LINKED {
        return false;
    }
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_DEFAULT_PORT);
    h2c_set(&mut pkt, 0, RTW_H2C_DEFAULT_PORT_W0_PORTID, port as u32);
    h2c_set(&mut pkt, 0, RTW_H2C_DEFAULT_PORT_W0_MACID, mac_id as u32);
    send_h2c_command(h, st, &pkt)
}

// ═══════════════════════════════════════════════════════════════════
// What `rtw_watch_dog_work` sends to the firmware every two seconds.
// ═══════════════════════════════════════════════════════════════════

/// fw.c:713-727 `rtw_fw_send_rssi_info`.
///
/// The firmware picks the rate and this is its input. Without it rate
/// selection keeps using the value from association even after the link
/// has degraded.
pub fn send_rssi_info(h: i32, st: &mut H2cState,
                      si: &crate::sta::StaInfo) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_RSSI_MONITOR);

    let rssi = si.avg_rssi.read(crate::dm::EWMA_RSSI_PRECISION);
    h2c_set(&mut pkt, 0, 0x0000_ff00, si.mac_id as u32); // MACID
    h2c_set(&mut pkt, 0, 0xff00_0000, rssi); // RSSI
    h2c_set(&mut pkt, 1, 1 << 1, (si.stbc_en != 0) as u32); // STBC
    send_h2c_command(h, st, &pkt)
}

/// fw.c:1000-1013 `rtw_fw_update_wl_phy_info`
pub fn update_wl_phy_info(h: i32, st: &mut H2cState, dm: &crate::dm::DmInfo,
                          tx_throughput: u32, rx_throughput: u32) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_WL_PHY_INFO);
    h2c_set(&mut pkt, 0, 0x0003_ff00, tx_throughput); // TX_TP  GENMASK(17, 8)
    h2c_set(&mut pkt, 0, 0x0ffc_0000, rx_throughput); // RX_TP  GENMASK(27, 18)
    h2c_set(&mut pkt, 1, 0x0000_00ff, dm.tx_rate as u32); // TX_RATE_DESC
    h2c_set(&mut pkt, 1, 0x0000_ff00, dm.curr_rx_rate as u32); // RX_RATE_DESC
    h2c_set(&mut pkt, 1, 0x00ff_0000, dm.rx_evm_dbm[0] as u32); // RX_EVM
    send_h2c_command(h, st, &pkt)
}

/// fw.c:2465-2483 `rtw_fw_adaptivity`.
///
/// The `rtw_edcca_enabled` branch is a debugfs switch (default on); there
/// is no debugfs here, so the enabled case always applies.
pub fn adaptivity(h: i32, st: &mut H2cState, dm: &crate::dm::DmInfo) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_ADAPTIVITY);
    h2c_set(&mut pkt, 0, 0x0000_0f00, dm.edcca_mode as u32); // MODE
    h2c_set(&mut pkt, 0, 0x0000_f000, 1); // OPTION, fixed to 1 in Linux
    h2c_set(&mut pkt, 0, 0x00ff_0000, dm.igi_history[0] as u32); // IGI
    h2c_set(&mut pkt, 0, 0xff00_0000, dm.l2h_th_ini as u32); // L2H
    h2c_set(&mut pkt, 1, 0x0000_00ff, dm.scan_density as u32); // DENSITY
    send_h2c_command(h, st, &pkt)
}

/// fw.c:265-323 `rtw_fw_ra_report_handle` + `_iter`.
///
/// Reports which rate the firmware is currently using. Two watchdog items
/// depend on it: `rtw_phy_config_swing_table` chooses between the CCK and
/// OFDM curve via `dm_info->tx_rate`, and `rtw_phy_rrsr_update` derives
/// the response rates from `si->ra_report.desc_rate`.
///
/// The rest of `_iter` (flags, `bit_rate`, `max_rc_amsdu_len`) fills
/// `struct rate_info` for `cfg80211` and has no reader here.
pub fn ra_report_handle(payload: &[u8], dm: &mut crate::dm::DmInfo,
                        si: Option<&mut crate::sta::StaInfo>) {
    if payload.len() < C2H_RA_REPORT_SIZE {
        return;
    }
    let rate = payload[0] & RTW_C2H_RA_RPT_RATE as u8;
    let mac_id = payload[1];

    dm.tx_rate = rate;

    if let Some(si) = si {
        if si.mac_id != mac_id {
            return;
        }
        si.ra_report_desc_rate = rate;
    }
}

/// tx.c:229-256 `rtw_tx_report_handle`. Returns `(sequence number, acked)`.
///
/// rtw88 has two paths for the same report, and `src` selects the layout:
///
/// * `C2H_CCX_TX_RPT` (0x03), its own C2H, layout V0: number in
///   `payload[6]`, status in `payload[0]`.
/// * `C2H_CCX_RPT` (0x0f), a subcommand of `C2H_HALMAC` (fw.c:93-113),
///   layout V1: number in `payload[8]`, status in `payload[9]`, and
///   `payload[0]` is the subcommand ID.
///
/// Which one a firmware uses is not stated in any header, so both are
/// handled.
///
/// `st == 0` means acked; the two status bits are a code and any non-zero
/// value is a failure.
pub fn tx_report_parse(payload: &[u8], v1: bool) -> Option<(u8, bool)> {
    let (sn_off, st_off) = if v1 {
        (CCX_REPORT_V1_SEQNUM_OFF, CCX_REPORT_V1_STATUS_OFF)
    } else {
        (CCX_REPORT_V0_SEQNUM_OFF, CCX_REPORT_V0_STATUS_OFF)
    };
    if payload.len() <= sn_off.max(st_off) {
        return None;
    }
    let sn = payload[sn_off] & CCX_REPORT_V0_SEQNUM_MASK;
    let st = payload[st_off] & CCX_REPORT_V0_STATUS_MASK;
    Some((sn, st == 0))
}

/// fw.c:383-390 `rtw_fw_c2h_cmd_isr`: the firmware reports its own crash.
///
/// In Linux this is checked in the interrupt path; here the watchdog polls
/// it. Same test, same register: if `REG_MCU_TST_CFG` holds the trigger
/// value, the firmware has declared itself dead.
pub fn fw_crashed(h: i32) -> bool {
    host::r8(h, REG_MCU_TST_CFG) as u32 == VAL_FW_TRIGGER
}
