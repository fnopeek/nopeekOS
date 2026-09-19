//! `fw.c` aus Linux 6.18.26 rtw88 — der Teil, den der Download braucht.
//!
//! Portiert: `rtw_fw_write_data_rsvd_page` (der 3081-Zweig).
#![allow(dead_code)]

use crate::host;
use crate::pci::{self, Trx};
use crate::regs::*;

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
    let shift = mask.trailing_zeros();
    let start = host::now_us();
    for cnt in 0..1000u32 {
        if (host::r32(h, addr) & mask) >> shift == target {
            return true;
        }
        // 10 us liegen unter unserer Schlafaufloesung. Eng lesen und die
        // Gesamtfrist (1000 x 10 us = 10 ms) an der Uhr halten; ab 10 ms
        // abgeben, damit ein haengendes Register nicht den Kern blockiert.
        if cnt > 0 && cnt % 64 == 0 && host::now_us() - start > 10_000 {
            host::sleep_ms(1);
        }
    }
    false
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
    }

    let mut ok = pci::write_data_rsvd_page(h, trx, stage, payload, current_band_type, verbose);

    if ok && !check_hw_ready(h, REG_FIFOPAGE_CTRL_2, BIT_BCN_VALID_V1, 1) {
        host::print("[rtl8822ce] error beacon valid\n");
        if verbose {
            host::print("  [dump] nach dem Anstoss:\n");
            dump_reg32(h, "FIFOPG_C2 ", REG_FIFOPAGE_CTRL_2);
            dump_reg32(h, "TXDMA_STAT", REG_TXDMA_STATUS);
            dump_reg32(h, "PCI_CTRL  ", pci::RTK_PCI_CTRL);
            dump_reg32(h, "BCN_WORK16", pci::RTK_PCI_RXBD_NUM_MPDUQ);
            dump_reg32(h, "CR        ", REG_CR);
            dump_reg32(h, "FWHW_TXQ  ", REG_FWHW_TXQ_CTRL);
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
