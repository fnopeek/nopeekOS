//! `fw.c` aus Linux 6.18.26 rtw88 — der Teil, den der Download braucht.
//!
//! Portiert: `rtw_fw_write_data_rsvd_page` (der 3081-Zweig).
#![allow(dead_code)]

use crate::host;
use crate::pci::{self, Trx};
use crate::regs::*;

/// PCI-Konfigurationsraum: Kommando und Status.
///
/// Das Kommandoregister sagt, ob der Chip ueberhaupt Busmaster ist — ohne
/// das kann er keinen Deskriptor aus dem Hauptspeicher holen, und genau so
/// sieht es aus: MMIO geht, DMA nicht.
///
/// Das STATUSregister ist die zweite Haelfte der Frage. Bit 13 (Received
/// Master Abort) und Bit 12 (Received Target Abort) stehen, wenn der Chip
/// es VERSUCHT hat und abgewiesen wurde. Bleiben sie leer und Busmaster ist
/// an, hat er gar nicht erst hingesehen. Das sind zwei verschiedene Fehler.
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

/// util.c `check_hw_ready`: 1000 Runden, 10 us auseinander, und gelesen wird
/// mit `rtw_read32_mask` — also 32 Bit, auch wenn das Register ein Byte ist.
pub fn check_hw_ready(h: i32, addr: u32, mask: u32, target: u32) -> bool {
    check_hw_ready_for(h, addr, mask, target, LINUX_FRIST_US).0
}

/// Linux: 1000 Runden mit je `udelay(10)` — die Frist ist also **10 ms**,
/// und die Rundenzahl ist nur die Art, wie sie dort gezaehlt wird.
pub const LINUX_FRIST_US: u64 = 1000 * 10;

/// util.c `check_hw_ready`, aber an der UHR statt an der Rundenzahl.
///
/// Die erste Fassung hielt 1000 Runden ohne Pause — das sind hier eine bis
/// zwei Millisekunden statt zehn, also ein Zehntel von Linux' Frist. Der
/// Kommentar daneben behauptete schon die Uhr; der Code tat etwas anderes,
/// und die Firmware bekam zu wenig Zeit, ihr FW_INIT_RDY zu setzen.
///
/// Gibt zurueck, ob es geklappt hat UND wie lange es gedauert hat — die
/// zweite Zahl ist der Unterschied zwischen „zu knapp" und „kommt nie".
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
        // 10 us liegen unter unserer Schlafaufloesung, also wird eng
        // gelesen. Ab 10 ms wird zwischen den Lesungen abgegeben, damit
        // eine lange Frist nicht den Kern blockiert.
        if waited > 10_000 {
            host::sleep_ms(1);
        }
    }
}

/// fw.c `rtw_fw_write_data_rsvd_page`, 3081-Zweig, PCIe.
///
/// `rsvd_boundary` ist beim Firmware-Download noch 0: es wird erst von
/// `rtw_set_trx_fifo_info` gesetzt, und das laeuft in `rtw_mac_init` — also
/// NACH dem Download. Linux schreibt hier also ebenfalls eine 0 zurueck.
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

    // PCIe: Beacon-Download der Queue abschalten, solange wir sie benutzen.
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
            // 0x382 ist NICHT vierfach ausgerichtet; der 32-Bit-Lesezugriff
            // im letzten Lauf gab 0xffffffff und war damit mein eigener
            // Messfehler, nicht die Antwort des Chips.
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
// Stufe 4a: die zwei H2C-Wege
// ════════════════════════════════════════════════════════════════
//
// Sie sehen gleich aus und sind es nicht:
//
//   `rtw_fw_send_h2c_command`  schreibt ACHT Byte in eines von vier
//                              HMEBOX-Registern. Die Koexistenz redet so.
//   `rtw_fw_send_h2c_packet`   schiebt ZWEIUNDDREISSIG Byte durch die
//                              H2C-QUEUE, also durch den Ring, dessen
//                              Adresse `init_h2c` gesetzt hat. General-
//                              und PHYDM-Info gehen so.
//
// Wer den einen fuer den anderen haelt, schickt alles ins Leere — und
// merkt es nicht, weil beide Wege stumm sind.

/// `struct rtw_h2c_cmd` (fw.h) — acht Byte, als zwei Woerter.
/// Der Zustand zwischen zwei Kommandos: welches Postfach als naechstes
/// drankommt und welche Folgenummer ein PAKET traegt.
#[derive(Default, Clone, Copy)]
/// `struct rtw_dev.h2c` (main.h) — **EINER fuer das ganze Geraet.**
///
/// Bis 0.17.0 legte jede Stufe einen eigenen an. Damit fing Stufe 5d
/// wieder bei Fach 0 an, obwohl 5c es zuletzt beschrieben hatte, und
/// `seq` lief mehrfach von null los. In Linux gibt es genau ein
/// `rtwdev->h2c`, und die Reihenfolge der Faecher ist der ganze Sinn:
/// der Treiber reicht sie im Kreis weiter, damit die Firmware Zeit hat,
/// das vorige zu leeren.
pub struct H2cState {
    pub last_box_num: u8,
    pub seq: u8,
}

/// Die vier Fachfahnen, wie der Chip sie gerade meldet.
pub fn hmetfr(h: i32) -> u8 {
    host::r8(h, REG_HMETFR)
}

/// Feld an seine Schiebestelle, im Wort `word` des H2C-Puffers.
/// Das ist `le32p_replace_bits((__le32 *)(h2c) + word, value, mask)`.
fn h2c_set(pkt: &mut [u8; H2C_PKT_SIZE], word: usize, mask: u32, value: u32) {
    let o = word * 4;
    let cur = u32::from_le_bytes([pkt[o], pkt[o + 1], pkt[o + 2], pkt[o + 3]]);
    let v = (cur & !mask) | ((value << mask.trailing_zeros()) & mask);
    pkt[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// ── Weg 1: die MAILBOX (fw.c:76-127) ─────────────────────────────

/// fw.c `rtw_fw_send_h2c_command`.
///
/// Linux pollt mit `read_poll_timeout_atomic(rtw_read8, ..., 100, 3000, ...)`
/// — alle 100 us, Frist **3 ms**. Gehalten wird hier die Frist, nicht die
/// Rundenzahl (derselbe Fehler wie in `check_hw_ready` soll sich nicht
/// wiederholen).
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
            // Linux sagt nur „failed to send h2c command". Welches Fach und
            // welche Fahnen — das ist der Unterschied zwischen einer
            // Meldung und einer Diagnose.
            host::print("[rtl8822ce] failed to send h2c command (Fach ");
            host::print_dec(box_num as u32);
            host::print(", HMETFR 0x");
            host::print_hex8(flags);
            host::print(")\n");
            return false;
        }
        host::delay_us(100);
    }

    // `h2c_cmd->msg` sind die Bytes 0..4, `msg_ext` die Bytes 4..8 —
    // und das EX-Register wird ZUERST geschrieben.
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

/// fw.c `rtw_fw_bt_wifi_control` — fw.h:568-574, Kommando 0x69.
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

/// fw.c `rtw_fw_query_bt_info` — Kommando 0x61.
pub fn query_bt_info(h: i32, st: &mut H2cState) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_QUERY_BT_INFO);
    h2c_set(&mut pkt, 0, 1 << 8, 1); // SET_QUERY_BT_INFO(h2c_pkt, true)
    send_h2c_command(h, st, &pkt)
}

// ── Weg 2: das PAKET durch die H2C-Queue (fw.c:495-565) ──────────

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
    // Linux erhoeht `seq` auch bei Fehlschlag.
    st.seq = st.seq.wrapping_add(1);
    ok
}

/// fw.c:517-535 `rtw_fw_send_general_info`.
///
/// Sagt der Firmware, wieviele Seiten sie hinter `rsvd_boundary` fuer ihren
/// eigenen Sendepuffer hat. Bei uns: 1994 − 1938 = **56**.
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
/// `rx_ant_status`/`tx_ant_status` kommen aus `hal->antenna_rx`/`antenna_tx`
/// — bei 2T2R beide `BB_PATH_AB`.
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

/// fw.c `rtw_fw_coex_tdma_type` — Mailbox-Kommando 0x60, fw.h:567.
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

/// fw.c:1080-1088 `rtw_fw_scan_notify` — Kommando 0x59.
pub fn scan_notify(h: i32, st: &mut H2cState, start: bool) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_SCAN);
    h2c_set(&mut pkt, 0, 1 << 8, start as u32); // SET_SCAN_START
    send_h2c_command(h, st, &pkt)
}

/// fw.c:437-446 `rtw_fw_inform_rfk_status` — Kommando 0x6d, Postfach.
pub fn inform_rfk_status(h: i32, st: &mut H2cState, start: bool) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_WIFI_CALIBRATION);
    h2c_set(&mut pkt, 0, 1 << 8, start as u32); // RFK_SET_INFORM_START
    send_h2c_command(h, st, &pkt)
}

/// fw.c:448-459 `rtw_fw_do_iqk` — der QUEUE-Weg, nicht das Postfach.
///
/// Die IQK rechnet die FIRMWARE; der Treiber stoesst sie nur an und wartet
/// danach auf `REG_RPT_CIP`.
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
