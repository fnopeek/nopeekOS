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
