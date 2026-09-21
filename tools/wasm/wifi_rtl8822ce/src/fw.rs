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

/// fw.c:1123-1132 `rtw_fw_media_status_report` — Kommando 0x01, Postfach.
///
/// Sagt der Firmware, dass diese `mac_id` verbunden ist. Sie richtet
/// daraufhin ihre eigene Buchfuehrung ein (Ratenanpassung, Stromsparen).
pub fn media_status_report(h: i32, st: &mut H2cState, mac_id: u8,
                           connect: bool) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_MEDIA_STATUS_RPT);
    h2c_set(&mut pkt, 0, 1 << 8, connect as u32); // SET_OP_MODE
    h2c_set(&mut pkt, 0, 0x00ff_0000, mac_id as u32); // SET_MACID
    send_h2c_command(h, st, &pkt)
}

/// fw.h:73-77 `struct rtw_c2h_cmd` — Kennung, Folgenummer, Nutzlast.
pub struct C2hCmd<'a> {
    pub id: u8,
    pub seq: u8,
    pub payload: &'a [u8],
}

/// fw.c:334-380 `rtw_fw_c2h_cmd_handle`, der Verteiler.
///
/// **Was hinter den Kennungen liegt, ist noch nicht gebaut** — und das
/// steht hier namentlich statt als stiller `_ =>`. Jede dieser Zeilen ist
/// ein eigener Posten: `rtw_tx_report_handle` braucht die Sendequittungen,
/// `rtw_coex_bt_info_notify` die laufende Koexistenz,
/// `rtw_fw_ra_report_handle` die Ratenanpassung. Gemeldet wird jede
/// Nachricht trotzdem, denn eine Firmware, die etwas sagt, sagt es aus
/// einem Grund.
pub fn c2h_name(id: u8) -> &'static str {
    // Eine Tabelle statt `match`: die Kennungen stehen in `regs.rs` teils
    // als `u8` (weil jemand sie in ein Byteregister schreibt) und teils
    // als `u32`, und ein Muster darf keine Umwandlung tragen.
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

/// Den C2H-Kopf aus einem Empfangspuffer ziehen. `pkt_offset` ist in Linux
/// derselbe Versatz wie beim Funkrahmen: Deskriptor + drv_info + shift.
pub fn c2h_parse(frame: &[u8]) -> Option<C2hCmd<'_>> {
    if frame.len() < 2 {
        return None;
    }
    Some(C2hCmd { id: frame[0], seq: frame[1], payload: &frame[2..] })
}

/// fw.c:1023-1063 `rtw_fw_send_ra_info` — Kommando 0x40, Postfach.
///
/// Der Treiber schickt eine MASKE, keine Rate: welche der 64 Raten dieses
/// Gegenueber kann. Die Firmware waehlt daraus laufend und meldet ihre
/// Wahl als `C2H_RA_RPT` zurueck.
///
/// Der `H2C_CMD_RA_INFO_HI`-Teil daneben gilt nur fuer den 8814A (vier
/// Sendeketten, Maske breiter als 32 Bit); `chip->id` ist hier 8822C, und
/// Linux kehrt an derselben Stelle um.
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
    // GENMASK(29, 28) — ZWEI Bit. Bei einem `bool` schreibt eine
    // Einzelbitmaske denselben Wert, loescht aber Bit 29 nicht. Hier ist
    // das folgenlos (der Puffer beginnt bei null), und trotzdem steht die
    // Maske der Quelle da: wer spaeter einen Wert > 1 setzt, bekaeme mit
    // der falschen Maske stillschweigend etwas anderes.
    h2c_set(&mut pkt, 0, 0x3000_0000, si.vht_enable as u32); // VHT_EN
    h2c_set(&mut pkt, 0, 1 << 30, 1); // DIS_PT — Linux: `disable_pt = true`
    h2c_set(&mut pkt, 1, 0x0000_00ff, (si.ra_mask & 0xff) as u32);
    h2c_set(&mut pkt, 1, 0x0000_ff00, ((si.ra_mask & 0xff00) >> 8) as u32);
    h2c_set(&mut pkt, 1, 0x00ff_0000, ((si.ra_mask & 0xff_0000) >> 16) as u32);
    h2c_set(&mut pkt, 1, 0xff00_0000, ((si.ra_mask & 0xff00_0000) >> 24) as u32);

    si.init_ra_lv = 0;
    send_h2c_command(h, st, &pkt)
}

/// fw.c:1065-1080 `rtw_fw_default_port`.
///
/// Kehrt um, solange der Port nicht verbunden ist — das ist keine
/// Abkuerzung, es steht so in der ersten Zeile.
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
// Was `rtw_watch_dog_work` alle zwei Sekunden an die Firmware schickt.
// ═══════════════════════════════════════════════════════════════════

/// fw.c:713-727 `rtw_fw_send_rssi_info`.
///
/// **Die Firmware waehlt die Rate, und das hier ist ihre Eingabe.** Ohne
/// sie rechnet die Ratenwahl auf dem Wert, den sie beim Anmelden bekommen
/// hat — auch noch, wenn die Leitung laengst schlechter ist.
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
/// Der `rtw_edcca_enabled`-Zweig ist ein debugfs-Schalter (Vorgabe an);
/// debugfs bauen wir nicht, also gilt hier immer der eingeschaltete Fall.
pub fn adaptivity(h: i32, st: &mut H2cState, dm: &crate::dm::DmInfo) -> bool {
    let mut pkt = [0u8; H2C_PKT_SIZE];
    set_cmd_id_class(&mut pkt, H2C_CMD_ADAPTIVITY);
    h2c_set(&mut pkt, 0, 0x0000_0f00, dm.edcca_mode as u32); // MODE
    h2c_set(&mut pkt, 0, 0x0000_f000, 1); // OPTION — Linux: fest 1
    h2c_set(&mut pkt, 0, 0x00ff_0000, dm.igi_history[0] as u32); // IGI
    h2c_set(&mut pkt, 0, 0xff00_0000, dm.l2h_th_ini as u32); // L2H
    h2c_set(&mut pkt, 1, 0x0000_00ff, dm.scan_density as u32); // DENSITY
    send_h2c_command(h, st, &pkt)
}

/// fw.c:265-323 `rtw_fw_ra_report_handle` + `_iter`.
///
/// **Die Rueckmeldung, welche Rate die FIRMWARE gerade fliegt.** Zwei
/// Posten des Watchdogs haengen daran: `rtw_phy_config_swing_table`
/// waehlt ueber `dm_info->tx_rate` zwischen CCK- und OFDM-Kurve, und
/// `rtw_phy_rrsr_update` rechnet aus `si->ra_report.desc_rate` die
/// Antwortraten. Ohne diesen Weg steht beides auf dem Anfangswert.
///
/// Der Rest von `_iter` (Flags, `bit_rate`, `max_rc_amsdu_len`) fuellt
/// `struct rate_info` fuer `cfg80211` und hat bei uns keinen Leser.
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

/// tx.c:229-256 `rtw_tx_report_handle`. Gibt `(Folgenummer, quittiert)`.
///
/// **rtw88 hat ZWEI Wege fuer dieselbe Quittung**, und `src` entscheidet
/// die Aufteilung:
///
/// * `C2H_CCX_TX_RPT` (0x03) — ein eigenes C2H, Aufteilung **V0**:
///   Nummer in `payload[6]`, Status in `payload[0]`.
/// * `C2H_CCX_RPT` (0x0f) — ein UNTERkommando von `C2H_HALMAC`
///   (fw.c:93-113), Aufteilung **V1**: Nummer in `payload[8]`, Status in
///   `payload[9]`, und `payload[0]` ist die Unterkommandokennung.
///
/// Welchen eine Firmware nimmt, steht in keinem Header. Der erste
/// Geraetelauf mit 0.26.0 hat es beantwortet: `0 ok, 0 ohne ACK, 49 ohne
/// bericht` — wir hoerten nur auf 0x03, und diese Firmware nimmt den
/// anderen Weg.
///
/// `st == 0` heisst quittiert — die zwei Statusbits sind ein Code, und
/// jeder von null verschiedene ist ein Misserfolg.
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

/// fw.c:383-390 `rtw_fw_c2h_cmd_isr` — die Firmware meldet ihren EIGENEN
/// Absturz.
///
/// **In Linux ist das eine Unterbrechung**; wir haben keine und sehen
/// deshalb im Watchdog nach. Derselbe Test, dieselbe Stelle, anderer
/// Takt: steht in `REG_MCU_TST_CFG` der Ausloeserwert, hat die Firmware
/// sich selbst fuer tot erklaert. Bis hierher wurde daraus eine stille
/// Leitung.
pub fn fw_crashed(h: i32) -> bool {
    host::r8(h, REG_MCU_TST_CFG) as u32 == VAL_FW_TRIGGER
}
