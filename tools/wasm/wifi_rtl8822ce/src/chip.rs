//! `rtw8822c.c` aus Linux 6.18.26 — was NUR dieser Chip tut.
//!
//! Der Schnitt ist derselbe wie in Linux: `mac.c` ist fuer alle rtw88-Chips
//! gleich und ruft an genau zwei Stellen in den Chip hinein
//! (`chip->ops->mac_init`, `chip->page_table`/`rqpn_table`). Was hier steht,
//! ist diese Hinein-Haelfte fuer den 8822C.
//!
//! Portiert: `page_table_8822c` · `rqpn_table_8822c` · `rtw8822c_mac_init`.

use crate::host;
use crate::regs::*;

/// main.h:1038-1044 `struct rtw_page_table`. **Die Reihenfolge der Felder
/// ist nicht die der Namen im Initialisierer** — `{64, 64, 64, 64, 1}` ist
/// hq, nq, lq, exq, gapq, und `__priority_queue_cfg` schreibt sie in einer
/// ANDEREN Reihenfolge in die Register (hq, lq, nq, exq).
pub struct PageTable {
    pub hq_num: u16,
    pub nq_num: u16,
    pub lq_num: u16,
    pub exq_num: u16,
    pub gapq_num: u16,
}

/// rtw8822c.c:4917-4923 `page_table_8822c[]`. Index 1 = PCIe.
pub const PAGE_TABLE: [PageTable; 5] = [
    PageTable { hq_num: 64, nq_num: 64, lq_num: 64, exq_num: 64, gapq_num: 1 },
    PageTable { hq_num: 64, nq_num: 64, lq_num: 64, exq_num: 64, gapq_num: 1 },
    PageTable { hq_num: 64, nq_num: 64, lq_num: 0, exq_num: 0, gapq_num: 1 },
    PageTable { hq_num: 64, nq_num: 64, lq_num: 64, exq_num: 0, gapq_num: 1 },
    PageTable { hq_num: 64, nq_num: 64, lq_num: 64, exq_num: 64, gapq_num: 1 },
];

/// main.h:1019-1026 `struct rtw_rqpn` — wohin jede der sechs Sendequeues
/// im DMA-Prioritaetsraum zeigt.
pub struct Rqpn {
    pub dma_map_vo: u8,
    pub dma_map_vi: u8,
    pub dma_map_be: u8,
    pub dma_map_bk: u8,
    pub dma_map_mg: u8,
    pub dma_map_hi: u8,
}

/// rtw8822c.c:4925-4938 `rqpn_table_8822c[]`. Index 1 = PCIe.
///
/// Gebaut sind nur die beiden Eintraege, die es auf PCIe geben kann: Linux
/// waehlt [1] fuer PCIe, [0] fuer SDIO und [2..4] nach der Zahl der
/// USB-Bulkout-Endpunkte. Ein Bus, den dieser Treiber nicht hat, braucht
/// keine Zeile — und eine Zeile, die niemand liest, ist eine Behauptung.
pub const RQPN_PCIE: Rqpn = Rqpn {
    dma_map_vo: RTW_DMA_MAPPING_NORMAL,
    dma_map_vi: RTW_DMA_MAPPING_NORMAL,
    dma_map_be: RTW_DMA_MAPPING_LOW,
    dma_map_bk: RTW_DMA_MAPPING_LOW,
    dma_map_mg: RTW_DMA_MAPPING_EXTRA,
    dma_map_hi: RTW_DMA_MAPPING_HIGH,
};

/// rtw8822c.c:2004-2130 `rtw8822c_mac_init`.
///
/// Eine gerade Liste von Schreibzugriffen — SIFS, Ratenrueckfall, EDCA,
/// Beacon, WMAC, Empfangsfilter. Sie steht hier Zeile fuer Zeile in Linux'
/// Reihenfolge und mit Linux' Schreibbreiten; jede Umgruppierung waere eine
/// Abweichung ohne Gewinn.
pub fn mac_init(h: i32) -> bool {
    // txq control
    let mut value8 = host::r8(h, REG_FWHW_TXQ_CTRL);
    // Linux schreibt `BIT(7) & ~BIT(1) & ~BIT(2)`. Das ist 0x80 — die zwei
    // Ausmaskierungen treffen ein Bit, das gar nicht gesetzt ist. Der
    // Ausdruck bleibt stehen, damit sichtbar ist, dass hier NICHTS
    // geloescht wird, obwohl es so aussieht.
    value8 |= (1 << 7) & !(1 << 1) & !(1u8 << 2);
    host::w8(h, REG_FWHW_TXQ_CTRL, value8);
    host::w8(h, REG_FWHW_TXQ_CTRL + 1, WLAN_TXQ_RPT_EN);

    // sifs control
    host::w16(h, REG_SPEC_SIFS, WLAN_SIFS_DUR_TUNE);
    host::w32(h, REG_SIFS, WLAN_SIFS_CFG);
    host::w16(h, REG_RESP_SIFS_CCK, WLAN_SIFS_CCK_CTX | (WLAN_SIFS_CCK_IRX << 8));
    host::w16(h, REG_RESP_SIFS_OFDM, WLAN_SIFS_OFDM_CTX | (WLAN_SIFS_OFDM_IRX << 8));

    // rate fallback control
    host::w32(h, REG_DARFRC, WLAN_DATA_RATE_FB_CNT_1_4);
    host::w32(h, REG_DARFRCH, WLAN_DATA_RATE_FB_CNT_5_8);
    host::w32(h, REG_RARFRCH, WLAN_RTS_RATE_FB_CNT_5_8);
    host::w32(h, REG_ARFR0, WLAN_DATA_RATE_FB_RATE0);
    host::w32(h, REG_ARFRH0, WLAN_DATA_RATE_FB_RATE0_H);
    host::w32(h, REG_ARFR1_V1, WLAN_RTS_RATE_FB_RATE1);
    host::w32(h, REG_ARFRH1_V1, WLAN_RTS_RATE_FB_RATE1_H);
    host::w32(h, REG_ARFR4, WLAN_RTS_RATE_FB_RATE4);
    host::w32(h, REG_ARFRH4, WLAN_RTS_RATE_FB_RATE4_H);
    host::w32(h, REG_ARFR5, WLAN_RTS_RATE_FB_RATE5);
    host::w32(h, REG_ARFRH5, WLAN_RTS_RATE_FB_RATE5_H);

    // protocol configuration
    host::w8(h, REG_AMPDU_MAX_TIME_V1, WLAN_AMPDU_MAX_TIME);
    host::set8(h, REG_TX_HANG_CTRL, BIT_EN_EOF_V1);
    let pre_txcnt: u16 = WLAN_PRE_TXCNT_TIME_TH | BIT_EN_PRECNT;
    host::w8(h, REG_PRECNT_CTRL, (pre_txcnt & 0xFF) as u8);
    host::w8(h, REG_PRECNT_CTRL + 1, (pre_txcnt >> 8) as u8);
    let value32 = WLAN_RTS_LEN_TH
        | (WLAN_RTS_TX_TIME_TH << 8)
        | (WLAN_MAX_AGG_PKT_LIMIT << 16)
        | (WLAN_RTS_MAX_AGG_PKT_LIMIT << 24);
    host::w32(h, REG_PROT_MODE_CTRL, value32);
    host::w16(h, REG_BAR_MODE_CTRL + 2,
              WLAN_BAR_RETRY_LIMIT | (WLAN_RA_TRY_RATE_AGG_LIMIT << 8));
    host::w8(h, REG_FAST_EDCA_VOVI_SETTING, FAST_EDCA_VO_TH);
    host::w8(h, REG_FAST_EDCA_VOVI_SETTING + 2, FAST_EDCA_VI_TH);
    host::w8(h, REG_FAST_EDCA_BEBK_SETTING, FAST_EDCA_BE_TH);
    host::w8(h, REG_FAST_EDCA_BEBK_SETTING + 2, FAST_EDCA_BK_TH);

    // close BA parser
    host::clr8(h, REG_LIFETIME_EN, BIT_BA_PARSER_EN);
    host::clr32(h, REG_RRSR, BITS_RRSR_RSC);

    // EDCA configuration
    host::w32(h, REG_EDCA_VO_PARAM, WLAN_EDCA_VO_PARAM);
    host::w32(h, REG_EDCA_VI_PARAM, WLAN_EDCA_VI_PARAM);
    host::w32(h, REG_EDCA_BE_PARAM, WLAN_EDCA_BE_PARAM);
    host::w32(h, REG_EDCA_BK_PARAM, WLAN_EDCA_BK_PARAM);
    host::w8(h, REG_PIFS, WLAN_PIFS_TIME);
    host::clr8(h, REG_TX_PTCL_CTRL + 1, (BIT_SIFS_BK_EN >> 8) as u8);
    host::set8(h, REG_RD_CTRL + 1,
               ((BIT_DIS_TXOP_CFE | BIT_DIS_LSIG_CFE | BIT_DIS_STBC_CFE) >> 8) as u8);

    // MAC clock configuration
    host::clr32(h, REG_AFE_CTRL1, BIT_MAC_CLK_SEL);
    host::w8(h, REG_USTIME_TSF, MAC_CLK_SPEED);
    host::w8(h, REG_USTIME_EDCA, MAC_CLK_SPEED);

    host::set8(h, REG_MISC_CTRL, BIT_EN_FREE_CNT | BIT_DIS_SECOND_CCA);
    host::clr8(h, REG_TIMER0_SRC_SEL, BIT_TSFT_SEL_TIMER0);
    host::w16(h, REG_TXPAUSE, 0x0000);
    host::w8(h, REG_SLOT, WLAN_SLOT_TIME);
    host::w32(h, REG_RD_NAV_NXT, WLAN_NAV_CFG);
    host::w16(h, REG_RXTSF_OFFSET_CCK, WLAN_RX_TSF_CFG);

    // Set beacon control - enable TSF and other related functions
    host::set8(h, REG_BCN_CTRL, BIT_EN_BCN_FUNCTION);

    // Set send beacon related registers
    host::w32(h, REG_TBTT_PROHIBIT, WLAN_TBTT_TIME);
    host::w8(h, REG_DRVERLYINT, WLAN_DRV_EARLY_INT);
    host::w8(h, REG_BCN_CTRL_CLINT0, WLAN_BCN_CTRL_CLT0);
    host::w8(h, REG_BCNDMATIM, WLAN_BCN_DMA_TIME);
    host::w8(h, REG_BCN_MAX_ERR, WLAN_BCN_MAX_ERR);

    // WMAC configuration
    host::w32(h, REG_MAR, WLAN_MULTI_ADDR);
    host::w32(h, REG_MAR + 4, WLAN_MULTI_ADDR);
    host::w8(h, REG_BBPSF_CTRL + 2, WLAN_RESP_TXRATE);
    host::w8(h, REG_ACKTO, WLAN_ACK_TO);
    host::w8(h, REG_ACKTO_CCK, WLAN_ACK_TO_CCK);
    host::w16(h, REG_EIFS, WLAN_EIFS_DUR_TUNE);
    host::w8(h, REG_NAV_CTRL + 2, WLAN_NAV_MAX);
    host::w8(h, REG_WMAC_TRXPTCL_CTL_H + 2, WLAN_BAR_ACK_TYPE);
    host::w32(h, REG_RXFLTMAP0, WLAN_RX_FILTER0);
    host::w16(h, REG_RXFLTMAP2, WLAN_RX_FILTER2);
    host::w32(h, REG_RCR, WLAN_RCR_CFG);
    host::w8(h, REG_RX_PKT_LIMIT, WLAN_RXPKT_MAX_SZ_512);
    host::w8(h, REG_TCR + 2, WLAN_TX_FUNC_CFG2);
    host::w8(h, REG_TCR + 1, WLAN_TX_FUNC_CFG1);
    host::set32(h, REG_GENERAL_OPTION, BIT_DUMMY_FCS_READY_MASK_EN);
    host::w32(h, REG_WMAC_OPTION_FUNCTION + 8, WLAN_MAC_OPT_FUNC2);
    host::w8(h, REG_WMAC_OPTION_FUNCTION_1, WLAN_MAC_OPT_NORM_FUNC1);

    // init low power
    let mut value16 = host::r16(h, REG_RXPSF_CTRL + 2) & 0xF00F;
    value16 |= ((bit_rxgck_vht_fifothr(1) | bit_rxgck_ht_fifothr(1)
                 | bit_rxgck_ofdm_fifothr(1) | bit_rxgck_cck_fifothr(1)) >> 16) as u16;
    host::w16(h, REG_RXPSF_CTRL + 2, value16);

    let mut value16: u16 = 0;
    value16 = bit_set_rxpsf_pktlenthr(value16, 1);
    value16 |= BIT_RXPSF_CTRLEN | BIT_RXPSF_VHTCHKEN | BIT_RXPSF_HTCHKEN
        | BIT_RXPSF_OFDMCHKEN | BIT_RXPSF_CCKCHKEN | BIT_RXPSF_OFDMRST;
    host::w16(h, REG_RXPSF_CTRL, value16);
    host::w32(h, REG_RXPSF_TYPE_CTRL, 0xFFFF_FFFF);

    // rx ignore configuration
    let mut value16 = host::r16(h, REG_RXPSF_CTRL);
    value16 &= !(BIT_RXPSF_MHCHKEN | BIT_RXPSF_CCKRST | BIT_RXPSF_CONT_ERRCHKEN);
    value16 = bit_set_rxpsf_errthr(value16, 0x07);
    host::w16(h, REG_RXPSF_CTRL, value16);
    host::set8(h, REG_SND_PTCL_CTRL, BIT_DIS_CHK_VHTSIGB_CRC);

    // Interrupt migration configuration
    host::w32(h, REG_INT_MIG, WLAN_MAC_INT_MIG_CFG);

    true
}

// ════════════════════════════════════════════════════════════════
// Stufe 3c: rtw8822c_phy_set_param (rtw8822c.c:1862-1913)
// ════════════════════════════════════════════════════════════════

use crate::dm::{DmInfo, PathDiv};
use crate::efuse::Efuse;
use crate::phy::{self, RFREG_MASK, RF_PATH_A, RF_PATH_B};

/// rtw8822c.c:5361 `.default_1ss_tx_path = BB_PATH_A`
pub const DEFAULT_1SS_TX_PATH: u8 = BB_PATH_A;

/// rtw8822c.c:88-99 `rtw8822c_header_file_init`
fn header_file_init(h: i32, pre: bool) {
    host::set32(h, REG_3WIRE, BIT_3WIRE_TX_EN | BIT_3WIRE_RX_EN);
    host::set32(h, REG_3WIRE, BIT_3WIRE_PI_ON);
    host::set32(h, REG_3WIRE2, BIT_3WIRE_TX_EN | BIT_3WIRE_RX_EN);
    host::set32(h, REG_3WIRE2, BIT_3WIRE_PI_ON);

    if pre {
        host::clr32(h, REG_ENCCK, BIT_CCK_OFDM_BLK_EN);
    } else {
        host::set32(h, REG_ENCCK, BIT_CCK_OFDM_BLK_EN);
    }
}

/// rtw8822c.c:101-106 `rtw8822c_bb_reset` — aus, an, aus? Nein: AN, aus, AN.
/// Der mittlere Schritt ist der Reset, die zwei aeusseren halten ihn.
fn bb_reset(h: i32) {
    host::set16(h, REG_SYS_FUNC_EN, BIT_FEN_BB_RSTB as u16);
    host::clr16(h, REG_SYS_FUNC_EN, BIT_FEN_BB_RSTB as u16);
    host::set16(h, REG_SYS_FUNC_EN, BIT_FEN_BB_RSTB as u16);
}

/// rtw8822c.c:2444-2459 `rtw8822c_config_cck_rx_path`
fn config_cck_rx_path(h: i32, rx_path: u8) {
    if rx_path == BB_PATH_A || rx_path == BB_PATH_B {
        host::w32_mask(h, REG_CCANRX, 0x0006_0000, 0x0);
        host::w32_mask(h, REG_CCANRX, 0x0060_0000, 0x0);
    } else if rx_path == BB_PATH_AB {
        host::w32_mask(h, REG_CCANRX, 0x0060_0000, 0x1);
        host::w32_mask(h, REG_CCANRX, 0x0006_0000, 0x1);
    }

    if rx_path == BB_PATH_A {
        host::w32_mask(h, REG_RXCCKSEL, 0x0f00_0000, 0x0);
    } else if rx_path == BB_PATH_B {
        host::w32_mask(h, REG_RXCCKSEL, 0x0f00_0000, 0x5);
    } else if rx_path == BB_PATH_AB {
        host::w32_mask(h, REG_RXCCKSEL, 0x0f00_0000, 0x1);
    }
}

/// rtw8822c.c:2461-2479 `rtw8822c_config_ofdm_rx_path`
fn config_ofdm_rx_path(h: i32, rx_path: u8) {
    if rx_path == BB_PATH_A || rx_path == BB_PATH_B {
        host::w32_mask(h, REG_RXFNCTL, 0x300, 0x0);
        host::w32_mask(h, REG_RXFNCTL, 0x60_0000, 0x0);
        host::w32_mask(h, REG_AGCSWSH, 1 << 17, 0x0);
        host::w32_mask(h, REG_ANTWTPD, 1 << 20, 0x0);
        host::w32_mask(h, REG_MRCM, 1 << 24, 0x0);
    } else if rx_path == BB_PATH_AB {
        host::w32_mask(h, REG_RXFNCTL, 0x300, 0x1);
        host::w32_mask(h, REG_RXFNCTL, 0x60_0000, 0x1);
        host::w32_mask(h, REG_AGCSWSH, 1 << 17, 0x1);
        host::w32_mask(h, REG_ANTWTPD, 1 << 20, 0x1);
        host::w32_mask(h, REG_MRCM, 1 << 24, 0x1);
    }

    host::w32_mask(h, 0x824, 0x0f00_0000, rx_path as u32);
    host::w32_mask(h, 0x824, 0x000f_0000, rx_path as u32);
}

/// rtw8822c.c:2481-2497 `rtw8822c_config_cck_tx_path`
fn config_cck_tx_path(h: i32, tx_path: u8, is_tx2_path: bool) {
    if tx_path == BB_PATH_A {
        host::w32_mask(h, REG_RXCCKSEL, 0xf000_0000, 0x8);
    } else if tx_path == BB_PATH_B {
        host::w32_mask(h, REG_RXCCKSEL, 0xf000_0000, 0x4);
    } else if is_tx2_path {
        host::w32_mask(h, REG_RXCCKSEL, 0xf000_0000, 0xc);
    } else {
        host::w32_mask(h, REG_RXCCKSEL, 0xf000_0000, 0x8);
    }
    bb_reset(h);
}

/// rtw8822c.c:2499-2521 `rtw8822c_config_ofdm_tx_path`
fn config_ofdm_tx_path(h: i32, tx_path: u8, tx_path_sel_1ss: u8) {
    if tx_path == BB_PATH_A {
        host::w32_mask(h, REG_ANTMAP0, 0xff, 0x11);
        host::w32_mask(h, REG_TXLGMAP, 0xff, 0x0);
    } else if tx_path == BB_PATH_B {
        host::w32_mask(h, REG_ANTMAP0, 0xff, 0x12);
        host::w32_mask(h, REG_TXLGMAP, 0xff, 0x0);
    } else if tx_path_sel_1ss == BB_PATH_AB {
        host::w32_mask(h, REG_ANTMAP0, 0xff, 0x33);
        host::w32_mask(h, REG_TXLGMAP, 0xffff, 0x0404);
    } else if tx_path_sel_1ss == BB_PATH_B {
        host::w32_mask(h, REG_ANTMAP0, 0xff, 0x32);
        host::w32_mask(h, REG_TXLGMAP, 0xffff, 0x0400);
    } else if tx_path_sel_1ss == BB_PATH_A {
        host::w32_mask(h, REG_ANTMAP0, 0xff, 0x31);
        host::w32_mask(h, REG_TXLGMAP, 0xffff, 0x0400);
    }
    bb_reset(h);
}

/// rtw8822c.c:2436-2442 `rtw8822c_toggle_igi` — den IGI zwei Schritte
/// herunter und wieder zurueck. Das stoesst die Verstaerkungsregelung an.
fn toggle_igi(h: i32) {
    let igi = host::r32_mask(h, REG_RXIGI, 0x7f);
    // `igi - 2` laeuft in C um, wenn igi unter 2 liegt, und die Maske
    // schneidet das Ergebnis danach ohnehin auf sieben Bit. In Rust waere
    // dasselbe je nach Bauart eine PANIK — also ausdruecklich umlaufend.
    let lower = igi.wrapping_sub(2);
    host::w32_mask(h, REG_RXIGI, 0x7f, lower);
    host::w32_mask(h, REG_RXIGI, 0x7f00, lower);
    host::w32_mask(h, REG_RXIGI, 0x7f, igi);
    host::w32_mask(h, REG_RXIGI, 0x7f00, igi);
}

/// rtw8822c.c:2529-2546 `rtw8822c_config_trx_mode`
fn config_trx_mode(h: i32, tx_path: u8, rx_path: u8, is_tx2_path: bool) {
    if (tx_path | rx_path) & BB_PATH_A != 0 {
        host::w32_mask(h, REG_ORITXCODE, MASK20BITS, 0x33312);
    } else {
        host::w32_mask(h, REG_ORITXCODE, MASK20BITS, 0x11111);
    }
    if (tx_path | rx_path) & BB_PATH_B != 0 {
        host::w32_mask(h, REG_ORITXCODE2, MASK20BITS, 0x33312);
    } else {
        host::w32_mask(h, REG_ORITXCODE2, MASK20BITS, 0x11111);
    }

    config_cck_rx_path(h, rx_path);
    config_ofdm_rx_path(h, rx_path);

    config_cck_tx_path(h, BB_PATH_A, is_tx2_path);
    config_ofdm_tx_path(h, tx_path, BB_PATH_A);
    bb_reset(h);

    toggle_igi(h);
}

// ── rtw8822c_rf_init (rtw8822c.c:1838-1845) ──────────────────────

/// rtw8822c.c:1010-1022 `rtw8822c_rf_x2_check`
fn rf_x2_check(h: i32) {
    host::sleep_ms(1);
    let x2k_busy = phy::read_rf(h, RF_PATH_A, 0xb8, 1 << 15);
    if x2k_busy == 1 {
        phy::write_rf_reg_mix(h, RF_PATH_A, 0xb8, RFREG_MASK, 0xC4440);
        phy::write_rf_reg_mix(h, RF_PATH_A, 0xba, RFREG_MASK, 0x6840D);
        phy::write_rf_reg_mix(h, RF_PATH_A, 0xb8, RFREG_MASK, 0x80440);
        host::sleep_ms(1);
    }
}

/// rtw8822c.c:1024-1053 `rtw8822c_set_power_trim`.
///
/// Die fuenfzehn Zeilen des Makros `RF_SET_POWER_TRIM(path, seq, idx)` — die
/// Reihenfolge der Indizes ist NICHT fortlaufend (2 kommt zweimal, dann 3-7,
/// dann noch einmal 3-7 und 7): sie bildet die Sendekanalgruppen auf die
/// acht gemessenen Verstaerkungen ab.
const POWER_TRIM_SEQ: [(u32, usize); 15] = [
    (0x0, 0), (0x1, 1), (0x2, 2), (0x3, 2), (0x4, 3),
    (0x5, 4), (0x6, 5), (0x7, 6), (0x8, 7), (0x9, 3),
    (0xa, 4), (0xb, 5), (0xc, 6), (0xd, 7), (0xe, 7),
];

fn set_power_trim(h: i32, rf_path_num: u8, bb_gain: &[[i8; 8]; 2]) {
    for path in 0..rf_path_num as usize {
        phy::write_rf_reg_mix(h, path, 0xee, 1 << 19, 1);
        for &(seq, idx) in POWER_TRIM_SEQ.iter() {
            phy::write_rf_reg_mix(h, path, 0x33, RFREG_MASK, seq);
            // `bb_gain` ist in C `s8`; der Wert geht als Rohbitmuster in ein
            // 20-Bit-RF-Register, also wird vorzeichenerhaltend erweitert
            // und dann geklemmt — genau das tut die implizite Umwandlung
            // `s8 -> u32` in C.
            phy::write_rf_reg_mix(h, path, 0x3f, RFREG_MASK,
                                  bb_gain[path][idx] as i32 as u32);
        }
        phy::write_rf_reg_mix(h, path, 0xee, 1 << 19, 0);
    }
}

/// rtw8822c.c:1056-1091 `rtw8822c_power_trim`
fn power_trim(h: i32, rf_path_num: u8) {
    let mut bb_gain = [[0i8; 8]; 2];
    let rf_efuse_2g = [PPG_2GL_TXAB, PPG_2GM_TXAB, PPG_2GH_TXAB];
    let rf_efuse_5g: [[u16; 5]; 2] = [
        [PPG_5GL1_TXA, PPG_5GL2_TXA, PPG_5GM1_TXA, PPG_5GM2_TXA, PPG_5GH1_TXA],
        [PPG_5GL1_TXB, PPG_5GL2_TXB, PPG_5GM1_TXB, PPG_5GM2_TXB, PPG_5GH1_TXB],
    ];
    let mut set = false;

    for (i, &addr) in rf_efuse_2g.iter().enumerate() {
        let pg_pwr = crate::efuse::read8_physical(h, addr);
        if pg_pwr == EFUSE_READ_FAIL {
            continue;
        }
        set = true;
        bb_gain[RF_PATH_A][i] = (pg_pwr & PPG_2G_A_MASK) as i8;
        bb_gain[RF_PATH_B][i] = ((pg_pwr & PPG_2G_B_MASK) >> 4) as i8;
    }

    for i in 0..5usize {
        for path in 0..rf_path_num as usize {
            let pg_pwr = crate::efuse::read8_physical(h, rf_efuse_5g[path][i]);
            if pg_pwr == EFUSE_READ_FAIL {
                continue;
            }
            set = true;
            let idx = i + rf_efuse_2g.len();
            bb_gain[path][idx] = (pg_pwr & PPG_5G_MASK) as i8;
        }
    }
    if set {
        set_power_trim(h, rf_path_num, &bb_gain);
    }

    host::w32_mask(h, REG_DIS_DPD, DIS_DPD_MASK, DIS_DPD_RATEALL);
}

/// rtw8822c.c:1093-1109 `rtw8822c_thermal_trim`.
///
/// Der Kommentar in Linux erklaert die schraege Umsortierung: Bit 0 der
/// efuse wandert auf Bit 3, und die Bits 1-3 ruecken eines nach unten.
fn thermal_trim(h: i32, rf_path_num: u8) {
    let rf_efuse = [PPG_THERMAL_A, PPG_THERMAL_B];
    for path in 0..rf_path_num as usize {
        let pg_therm = crate::efuse::read8_physical(h, rf_efuse[path]);
        if pg_therm == EFUSE_READ_FAIL {
            return;
        }
        let mut thermal = (pg_therm & 0x0e) >> 1; // FIELD_GET(GENMASK(3, 1))
        thermal |= (pg_therm & 0x01) << 3; // FIELD_PREP(BIT(3), pg & BIT(0))
        phy::write_rf_reg_mix(h, path, 0x43, RF_THEMAL_MASK, thermal as u32);
    }
}

/// rtw8822c.c:1111-1132 `rtw8822c_pa_bias`.
///
/// **Die zweite Schleife prueft `EFUSE_READ_FAIL` NICHT** — sie schreibt
/// auch einen Fehlwert weiter. Das steht so in Linux; ein 0xff wird von
/// `PPG_PABIAS_MASK` ohnehin auf 0xf beschnitten.
fn pa_bias(h: i32, rf_path_num: u8) {
    let rf_efuse_2g = [PPG_PABIAS_2GA, PPG_PABIAS_2GB];
    let rf_efuse_5g = [PPG_PABIAS_5GA, PPG_PABIAS_5GB];

    for path in 0..rf_path_num as usize {
        let pg = crate::efuse::read8_physical(h, rf_efuse_2g[path]);
        if pg == EFUSE_READ_FAIL {
            return;
        }
        let pg = pg & PPG_PABIAS_MASK;
        phy::write_rf_reg_mix(h, path, RF_PA, RF_PABIAS_2G_MASK, pg as u32);
    }
    for path in 0..rf_path_num as usize {
        let pg = crate::efuse::read8_physical(h, rf_efuse_5g[path]);
        let pg = pg & PPG_PABIAS_MASK;
        phy::write_rf_reg_mix(h, path, RF_PA, RF_PABIAS_5G_MASK, pg as u32);
    }
}

/// rtw8822c.c:1838-1845 `rtw8822c_rf_init`
fn rf_init(h: i32, dm: &mut DmInfo, rf_path_num: u8) -> bool {
    let t0 = host::now_us();
    let dack_ok = crate::rfk::rf_dac_cal(h, dm);
    host::print("    DACK ");
    host::print_dec((host::now_us() - t0) as u32 / 1000);
    host::print(" ms\n");

    rf_x2_check(h);
    thermal_trim(h, rf_path_num);
    power_trim(h, rf_path_num);
    pa_bias(h, rf_path_num);
    dack_ok
}

/// rtw8822c.c:1847-1860 `rtw8822c_pwrtrack_init`. Reiner Treiberzustand.
///
/// `ewma_thermal_init` legt einen gleitenden Mittelwert an — bei uns ist das
/// die 0 in `thermal_avg`, die `rtw_phy_pwrtrack_avg` beim ersten Wert
/// ersetzt. Die Mittelung selbst gehoert zum Nachfuehren der Sendeleistung
/// und damit zur Stufe, die sendet.
fn pwrtrack_init(dm: &mut DmInfo, thermal_meter_k: u8) {
    for path in 0..4usize {
        dm.delta_power_index[path] = 0;
        dm.thermal_avg[path] = 0xff;
    }
    dm.pwr_trk_triggered = false;
    dm.thermal_meter_k = thermal_meter_k;
    dm.thermal_meter_lck = thermal_meter_k;
}

/// rtw8822c.c:1961-1968, ein Stueck aus `rtw8822c_phy_set_param`.
///
/// Herausgeloest, weil der EMPFANGSweg (`query_phy_status_page0`) dieselben
/// zwei Zahlen braucht: sie sind der Massstab, an dem ein CCK-Paket seine
/// RSSI bekommt. In Linux stehen sie in `dm_info` und werden genau hier
/// einmal gefuellt; sie gehoeren dem TREIBER, nicht dem Paket.
pub fn read_cck_gi_bnd(h: i32, dm: &mut crate::dm::DmInfo) {
    let cck_gi_u_bnd_msb = host::r32_mask(h, 0x1a98, 0xc000) as u8;
    let cck_gi_u_bnd_lsb = host::r32_mask(h, 0x1aa8, 0xf0000) as u8;
    let cck_gi_l_bnd_msb = host::r32_mask(h, 0x1a98, 0xc0) as u8;
    let cck_gi_l_bnd_lsb = host::r32_mask(h, 0x1a70, 0x0f00_0000) as u8;

    dm.cck_gi_u_bnd = (cck_gi_u_bnd_msb << 4) | cck_gi_u_bnd_lsb;
    dm.cck_gi_l_bnd = (cck_gi_l_bnd_msb << 4) | cck_gi_l_bnd_lsb;
}


/// rtw8822c.c:1862-1913 `rtw8822c_phy_set_param`.
///
/// `hal->antenna_tx`/`antenna_rx` kommen aus `rtw_chip_parameter_setup`:
/// bei 2T2R beide `BB_PATH_AB`. `is_tx2_path` ist dort fest `false`.
///
/// Gibt zurueck: (Tabellen wie gerechnet, DAC-Kalibrierung konvergiert) —
/// die Gates der Stufen 3b und 3c.
#[allow(clippy::too_many_arguments)]
pub fn phy_set_param(h: i32, dm: &mut DmInfo, path_div: &mut PathDiv,
                     e: &Efuse, cut_version: u8, rf_path_num: u8,
                     antenna_tx: u8, antenna_rx: u8) -> (bool, bool) {
    // power on BB/RF domain
    host::set8(h, REG_SYS_FUNC_EN, BIT_FEN_BB_GLB_RST | BIT_FEN_BB_RSTB);
    host::set8(h, REG_RF_CTRL, BIT_RF_EN | BIT_RF_RSTB | BIT_RF_SDM_RSTB);
    host::set32(h, REG_WLRF1, BIT_WLRF1_BBRF_EN);

    // disable low rate DPD
    host::w32_mask(h, REG_DIS_DPD, DIS_DPD_MASK, DIS_DPD_RATEALL);

    // pre init before header files config
    header_file_init(h, true);

    let drv = phy::setup_phy_cond(cut_version, e.rfe_option);
    host::print("  phy_cond 0x");
    host::print_hex32(drv.0);
    host::print(" (cut ");
    host::print_dec(drv.cut());
    host::print(", rfe ");
    host::print_dec(drv.rfe());
    host::print(", intf ");
    host::print_dec(drv.intf());
    host::print(", pkg ");
    host::print_dec(drv.pkg());
    host::print(")\n");

    let t0 = host::now_us();
    let tables_ok = phy::load_tables(h, rf_path_num, drv);
    host::print("    Tabellen ");
    host::print_dec((host::now_us() - t0) as u32 / 1000);
    host::print(" ms\n");

    let crystal_cap = e.crystal_cap & XCAP_MASK as u8;
    host::w32_mask(h, REG_ANAPAR_XTAL_0, 0xfffc00,
                   crystal_cap as u32 | ((crystal_cap as u32) << 7));

    // post init after header files config
    header_file_init(h, false);

    let is_tx2_path = false;
    config_trx_mode(h, antenna_tx, antenna_rx, is_tx2_path);
    phy::phy_init(h, dm, path_div, e.crystal_cap, DEFAULT_1SS_TX_PATH);

    read_cck_gi_bnd(h, dm);

    let dack_ok = rf_init(h, dm, rf_path_num);
    pwrtrack_init(dm, e.thermal_meter_k);

    crate::bf::phy_init(h);

    (tables_ok, dack_ok)
}

/// rtw8822c.c:2004 `rtw8822c_false_alarm_statistics`.
///
/// Sie ist hier nicht, weil Stufe 3 sie braeuchte, sondern weil sie das
/// GATE ist: zaehlt der Empfaenger Falschalarme und CCA-Ereignisse, hoert
/// er. Ein stiller Zaehler heisst, dass die PHY nicht laeuft.
pub fn false_alarm_statistics(h: i32, dm: &mut DmInfo) {
    let cck_enable = host::r32(h, REG_ENCCK) & BIT_CCK_BLK_EN;
    let cck_fa_cnt = host::r16(h, REG_CCK_FACNT) as u32;

    let ofdm_fa_cnt1 = host::r32(h, REG_OFDM_FACNT1);
    let ofdm_fa_cnt2 = host::r32(h, REG_OFDM_FACNT2);
    let ofdm_fa_cnt3 = host::r32(h, REG_OFDM_FACNT3);
    let ofdm_fa_cnt4 = host::r32(h, REG_OFDM_FACNT4);
    let ofdm_fa_cnt5 = host::r32(h, REG_OFDM_FACNT5);

    let parity_fail = ofdm_fa_cnt1 >> 16;
    let rate_illegal = ofdm_fa_cnt2 & 0xffff;
    let crc8_fail = ofdm_fa_cnt2 >> 16;
    let crc8_fail_vhta = ofdm_fa_cnt3 & 0xffff;
    let mcs_fail = ofdm_fa_cnt4 & 0xffff;
    let mcs_fail_vht = ofdm_fa_cnt4 >> 16;
    let fast_fsync = ofdm_fa_cnt5 & 0xffff;
    let sb_search_fail = ofdm_fa_cnt5 >> 16;

    let ofdm_fa_cnt = parity_fail + rate_illegal + crc8_fail + crc8_fail_vhta
        + mcs_fail + mcs_fail_vht + fast_fsync + sb_search_fail;

    dm.cck_fa_cnt = cck_fa_cnt;
    dm.ofdm_fa_cnt = ofdm_fa_cnt;
    dm.total_fa_cnt = ofdm_fa_cnt;
    if cck_enable != 0 {
        dm.total_fa_cnt += cck_fa_cnt;
    }

    let crc32_cnt = host::r32(h, 0x2c04);
    dm.cck_ok_cnt = crc32_cnt & 0xffff;
    dm.cck_err_cnt = crc32_cnt >> 16;
    let crc32_cnt = host::r32(h, 0x2c14);
    dm.ofdm_ok_cnt = crc32_cnt & 0xffff;
    dm.ofdm_err_cnt = crc32_cnt >> 16;
    let crc32_cnt = host::r32(h, 0x2c10);
    dm.ht_ok_cnt = crc32_cnt & 0xffff;
    dm.ht_err_cnt = crc32_cnt >> 16;
    let crc32_cnt = host::r32(h, 0x2c0c);
    dm.vht_ok_cnt = crc32_cnt & 0xffff;
    dm.vht_err_cnt = crc32_cnt >> 16;

    let cca32_cnt = host::r32(h, 0x2c08);
    dm.ofdm_cca_cnt = cca32_cnt >> 16;
    dm.cck_cca_cnt = cca32_cnt & 0xffff;
    dm.total_cca_cnt = dm.ofdm_cca_cnt;
    if cck_enable != 0 {
        dm.total_cca_cnt += dm.cck_cca_cnt;
    }

    host::w32_mask(h, REG_CCANRX, BIT_CCK_FA_RST, 0);
    host::w32_mask(h, REG_CCANRX, BIT_CCK_FA_RST, 2);
    host::w32_mask(h, REG_CCANRX, BIT_OFDM_FA_RST, 0);
    host::w32_mask(h, REG_CCANRX, BIT_OFDM_FA_RST, 2);

    // disable rx clk gating to reset counters
    host::clr32(h, REG_RX_BREAK, BIT_COM_RX_GCK_EN);
    host::set32(h, REG_CNT_CTRL, BIT_ALL_CNT_RST);
    host::clr32(h, REG_CNT_CTRL, BIT_ALL_CNT_RST);
    host::set32(h, REG_RX_BREAK, BIT_COM_RX_GCK_EN);
}

// ════════════════════════════════════════════════════════════════
// Stufe 4a: die Koexistenz-Ops des Chips (rtw8822c.c)
// ════════════════════════════════════════════════════════════════

use crate::coex::Coex;

/// rtw8822c.c `rtw8822c_coex_cfg_init` — `chip->ops->coex_set_init`.
pub fn coex_cfg_init(h: i32) {
    // enable TBTT interrupt
    host::set8(h, REG_BCN_CTRL, BIT_EN_BCN_FUNCTION);

    // BT report packet sample rate: 0x790[5:0]=0x5
    host::w8_mask(h, REG_BT_TDMA_TIME, BIT_MASK_SAMPLE_RATE, 0x5);

    // enable BT counter statistics
    host::w8(h, REG_BT_STAT_CTRL, 0x1);

    // enable PTA (3-wire function form BT side)
    host::set32(h, REG_GPIO_MUXCFG, BIT_BT_PTA_EN);
    host::set32(h, REG_GPIO_MUXCFG, BIT_PO_BT_PTA_PINS);

    // enable PTA (tx/rx signal form WiFi side)
    host::set8(h, REG_QUEUE_CTRL, BIT_PTA_WL_TX_EN);
    // wl tx signal to PTA not case EDCCA
    host::clr8(h, REG_QUEUE_CTRL, BIT_PTA_EDCCA_EN);
    // GNT_BT=1 while select both
    host::set16(h, REG_BT_COEX_V2, BIT_GNT_BT_POLARITY);
    // BT_CCA = ~GNT_WL_BB, not or GNT_BT_BB, LTE_Rx
    host::clr8(h, REG_DUMMY_PAGE4_V1, BIT_BTCCA_CTRL);

    // to avoid RF parameter error
    phy::write_rf_reg_mix(h, RF_PATH_B, RF_MODOPT, 0xfffff, 0x40000);
}

/// rtw8822c.c `rtw8822c_coex_cfg_gnt_debug`
pub fn coex_cfg_gnt_debug(h: i32) {
    host::w8_mask(h, REG_PAD_CTRL1 + 2, BIT_BTGP_SPI_EN >> 16, 0);
    host::w8_mask(h, REG_PAD_CTRL1 + 3, BIT_BTGP_JTAG_EN >> 24, 0);
    host::w8_mask(h, REG_GPIO_MUXCFG + 2, BIT_FSPI_EN >> 16, 0);
    host::w8_mask(h, REG_PAD_CTRL1 + 1, BIT_LED1DIS >> 8, 0);
    host::w8_mask(h, REG_SYS_SDIO_CTRL + 3, BIT_DBG_GNT_WL_BT >> 24, 0);
}

/// rtw8822c.c `rtw8822c_coex_cfg_rfe_type`.
///
/// Setzt den Beschreibungssatz des Antennen-Frontends — und schaltet dabei
/// die LTE-Koexistenz auf der WLAN-Seite AB. **`ant_switch_exist` bleibt
/// `false`**, und das ist der Grund, warum `rtw_coex_set_ant_switch` auf
/// diesem Chip nie etwas tut.
pub fn coex_cfg_rfe_type(h: i32, c: &mut Coex, share_ant: bool, rfe_option: u8) {
    c.rfe_module_type = rfe_option;
    c.ant_switch_polarity = 0;
    c.ant_switch_exist = false;
    c.ant_switch_with_bt = false;
    c.ant_switch_diversity = false;
    c.wlg_at_btg = share_ant;

    // disable LTE coex in wifi side
    crate::coex::write_indirect_reg(h, LTE_COEX_CTRL, BIT_LTE_COEX_EN, 0x0);
    crate::coex::write_indirect_reg(h, LTE_WL_TRX_CTRL, MASKLWORD, 0xffff);
    crate::coex::write_indirect_reg(h, LTE_BT_TRX_CTRL, MASKLWORD, 0xffff);
}

/// rtw8822c.c `rtw8822c_coex_cfg_gnt_fix`.
///
/// Nicht im Anlaufweg — Linux ruft es aus `rtw_coex_run_coex`, also im
/// laufenden Betrieb. Es steht hier, weil es zu den Coex-Ops des Chips
/// gehoert und weil der naechste Posten es braucht; wer es erst dann
/// schreibt, schreibt es unter Zeitdruck.
#[allow(dead_code)]
pub fn coex_cfg_gnt_fix(h: i32, c: &mut Coex, share_ant: bool) {
    const COEX_WLINK_2GFREE: u8 = 0x7; // coex.h:176

    if c.gnt_workaround_state == c.wl_coex_mode {
        return;
    }
    c.gnt_workaround_state = c.wl_coex_mode;

    let mut rf_0x1 = if (c.kt_ver == 0 && c.under_5g) || c.freerun {
        0x40021u32
    } else {
        0x40000u32
    };
    // BT at S1 for Shared-Ant
    if share_ant {
        rf_0x1 |= 1 << 13;
    }
    phy::write_rf_reg_mix(h, RF_PATH_B, 0x1, 0xfffff, rf_0x1);

    if c.wl_coex_mode == COEX_WLINK_2GFREE {
        host::w8_mask(h, REG_ANAPAR + 2, BIT_ANAPAR_BTPS >> 16, 0);
    } else {
        host::w8_mask(h, REG_ANAPAR + 2, BIT_ANAPAR_BTPS >> 16, 1);
        host::w8_mask(h, REG_RSTB_SEL + 1, BIT_DAC_OFF_ENABLE, 0);
        host::w8_mask(h, REG_RSTB_SEL + 3, BIT_DAC_OFF_ENABLE, 1);
    }

    // disable WL-S1 BB chage RF mode if GNT_BT, since RF TRx mask can do it
    host::w8_mask(h, REG_IGN_GNTBT4, BIT_PI_IGNORE_GNT_BT, 1);

    if c.wl_coex_mode == COEX_WLINK_2GFREE {
        host::w8_mask(h, REG_IGN_GNT_BT1, BIT_PI_IGNORE_GNT_BT, 1);
        host::w8_mask(h, REG_NOMASK_TXBT, BIT_NOMASK_TXBT_ENABLE, 1);
    } else if c.wl_coex_mode == COEX_WLINK_5G || c.under_5g || !share_ant {
        if c.kt_ver >= 3 {
            host::w8_mask(h, REG_IGN_GNT_BT1, BIT_PI_IGNORE_GNT_BT, 0);
            host::w8_mask(h, REG_NOMASK_TXBT, BIT_NOMASK_TXBT_ENABLE, 1);
        } else {
            host::w8_mask(h, REG_IGN_GNT_BT1, BIT_PI_IGNORE_GNT_BT, 1);
        }
    } else {
        // shared-antenna
        host::w8_mask(h, REG_IGN_GNT_BT1, BIT_PI_IGNORE_GNT_BT, 0);
        if c.kt_ver >= 3 {
            host::w8_mask(h, REG_NOMASK_TXBT, BIT_NOMASK_TXBT_ENABLE, 0);
        }
    }
}

// ════════════════════════════════════════════════════════════════
// Stufe 4c: rtw8822c_set_channel (rtw8822c.c:2529)
// ════════════════════════════════════════════════════════════════

/// main.h:73-79 — die Bandpruefungen, die `set_channel_bb` und
/// `set_channel_rf` ueberall benutzen.
#[inline] fn is_ch_2g(ch: u8) -> bool { ch <= 14 }
#[inline] fn is_ch_5g(ch: u8) -> bool { ch >= 36 && ch <= 177 }
#[inline] fn is_ch_5g_band_1(ch: u8) -> bool { (36..=48).contains(&ch) }
#[inline] fn is_ch_5g_band_2(ch: u8) -> bool { (52..=64).contains(&ch) }
#[inline] fn is_ch_5g_band_3(ch: u8) -> bool { (100..=144).contains(&ch) }
#[inline] fn is_ch_5g_band_4(ch: u8) -> bool { (149..=177).contains(&ch) }

/// rtw8822c.c:2229-2239 `rtw8822c_rstb_3wire`
fn rstb_3wire(h: i32, enable: bool) {
    if enable {
        host::w32_mask(h, REG_RSTB, BIT_RSTB_3WIRE, 0x1);
        host::w32_mask(h, REG_ANAPAR_A, BIT_ANAPAR_UPDATE, 0x1);
        host::w32_mask(h, REG_ANAPAR_B, BIT_ANAPAR_UPDATE, 0x1);
    } else {
        host::w32_mask(h, REG_RSTB, BIT_RSTB_3WIRE, 0x0);
    }
}

/// rtw8822c.c:2241-2397 `rtw8822c_set_channel_bb`.
///
/// **Hier stehen AGC und CCA-Maske** — und das ist der Grund, warum die
/// Falschalarmzaehler vor dieser Funktion nichts zaehlen koennen.
fn set_channel_bb(h: i32, channel: u8, bw: usize, primary_ch_idx: u8) {
    if is_ch_2g(channel) {
        host::clr32(h, REG_BGCTRL, BITS_RX_IQ_WEIGHT);
        host::set32(h, REG_TXF4, 1 << 20);
        host::clr32(h, REG_CCK_CHECK, BIT_CHECK_CCK_EN as u32);
        host::clr32(h, REG_CCKTXONLY, BIT_BB_CCK_CHECK_EN);
        host::w32_mask(h, REG_CCAMSK, 0x3F00_0000, 0xF);

        match bw {
            0 => {
                host::w32_mask(h, REG_RXAGCCTL0, BITS_RXAGC_CCK, 0x5);
                host::w32_mask(h, REG_RXAGCCTL, BITS_RXAGC_CCK, 0x5);
                host::w32_mask(h, REG_RXAGCCTL0, BITS_RXAGC_OFDM, 0x6);
                host::w32_mask(h, REG_RXAGCCTL, BITS_RXAGC_OFDM, 0x6);
            }
            1 => {
                host::w32_mask(h, REG_RXAGCCTL0, BITS_RXAGC_CCK, 0x4);
                host::w32_mask(h, REG_RXAGCCTL, BITS_RXAGC_CCK, 0x4);
                host::w32_mask(h, REG_RXAGCCTL0, BITS_RXAGC_OFDM, 0x0);
                host::w32_mask(h, REG_RXAGCCTL, BITS_RXAGC_OFDM, 0x0);
            }
            _ => {}
        }

        if channel == 13 || channel == 14 {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x969);
        } else if channel == 11 || channel == 12 {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x96a);
        } else {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x9aa);
        }

        if channel == 14 {
            host::w32_mask(h, REG_TXF0, MASKHWORD, 0x3da0);
            host::w32_mask(h, REG_TXF1, MASKDWORD, 0x4962_c931);
            host::w32_mask(h, REG_TXF2, MASKLWORD, 0x6aa3);
            host::w32_mask(h, REG_TXF3, MASKHWORD, 0xaa7b);
            host::w32_mask(h, REG_TXF4, MASKLWORD, 0xf3d7);
            host::w32_mask(h, REG_TXF5, MASKDWORD, 0x0);
            host::w32_mask(h, REG_TXF6, MASKDWORD, 0xff01_2455);
            host::w32_mask(h, REG_TXF7, MASKDWORD, 0xffff);
        } else {
            host::w32_mask(h, REG_TXF0, MASKHWORD, 0x5284);
            host::w32_mask(h, REG_TXF1, MASKDWORD, 0x3e18_fec8);
            host::w32_mask(h, REG_TXF2, MASKLWORD, 0x0a88);
            host::w32_mask(h, REG_TXF3, MASKHWORD, 0xacc4);
            host::w32_mask(h, REG_TXF4, MASKLWORD, 0xc8b2);
            host::w32_mask(h, REG_TXF5, MASKDWORD, 0x00fa_f0de);
            host::w32_mask(h, REG_TXF6, MASKDWORD, 0x0012_2344);
            host::w32_mask(h, REG_TXF7, MASKDWORD, 0x0fff_ffff);
        }

        if channel == 13 {
            host::w32_mask(h, REG_TXDFIR0, 0x70, 0x3);
        } else {
            host::w32_mask(h, REG_TXDFIR0, 0x70, 0x1);
        }
    } else if is_ch_5g(channel) {
        host::set32(h, REG_CCKTXONLY, BIT_BB_CCK_CHECK_EN);
        host::set32(h, REG_CCK_CHECK, BIT_CHECK_CCK_EN as u32);
        host::set32(h, REG_BGCTRL, BITS_RX_IQ_WEIGHT);
        host::clr32(h, REG_TXF4, 1 << 20);
        host::w32_mask(h, REG_CCAMSK, 0x3F00_0000, 0x22);
        host::w32_mask(h, REG_TXDFIR0, 0x70, 0x3);

        if is_ch_5g_band_1(channel) || is_ch_5g_band_2(channel) {
            host::w32_mask(h, REG_RXAGCCTL0, BITS_RXAGC_OFDM, 0x1);
            host::w32_mask(h, REG_RXAGCCTL, BITS_RXAGC_OFDM, 0x1);
        } else if is_ch_5g_band_3(channel) {
            host::w32_mask(h, REG_RXAGCCTL0, BITS_RXAGC_OFDM, 0x2);
            host::w32_mask(h, REG_RXAGCCTL, BITS_RXAGC_OFDM, 0x2);
        } else if is_ch_5g_band_4(channel) {
            host::w32_mask(h, REG_RXAGCCTL0, BITS_RXAGC_OFDM, 0x3);
            host::w32_mask(h, REG_RXAGCCTL, BITS_RXAGC_OFDM, 0x3);
        }

        if (36..=51).contains(&channel) {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x494);
        } else if (52..=55).contains(&channel) {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x493);
        } else if (56..=111).contains(&channel) {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x453);
        } else if (112..=119).contains(&channel) {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x452);
        } else if (120..=172).contains(&channel) {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x412);
        } else if (173..=177).contains(&channel) {
            host::w32_mask(h, REG_SCOTRK, 0xfff, 0x411);
        }
    }

    match bw {
        0 => {
            host::w32_mask(h, REG_DFIRBW, 0x3FF0, 0x19B);
            host::w32_mask(h, REG_TXBWCTL, 0xf, 0x0);
            host::w32_mask(h, REG_TXBWCTL, 0xffc0, 0x0);
            host::w32_mask(h, REG_TXCLK, 0x700, 0x7);
            host::w32_mask(h, REG_TXCLK, 0x700000, 0x6);
            host::w32_mask(h, REG_CCK_SOURCE, BIT_NBI_EN, 0x0);
            host::w32_mask(h, REG_SBD, BITS_SUBTUNE, 0x1);
            host::w32_mask(h, REG_PT_CHSMO, BIT_PT_OPT, 0x0);
        }
        1 => {
            host::w32_mask(h, REG_CCKSB, 1 << 4,
                           if primary_ch_idx == RTW_SC_20_UPPER { 1 } else { 0 });
            host::w32_mask(h, REG_TXBWCTL, 0xf, 0x5);
            host::w32_mask(h, REG_TXBWCTL, 0xc0, 0x0);
            host::w32_mask(h, REG_TXBWCTL, 0xff00,
                           (primary_ch_idx as u32) | ((primary_ch_idx as u32) << 4));
            host::w32_mask(h, REG_CCK_SOURCE, BIT_NBI_EN, 0x1);
            host::w32_mask(h, REG_SBD, BITS_SUBTUNE, 0x1);
            host::w32_mask(h, REG_PT_CHSMO, BIT_PT_OPT, 0x1);
        }
        2 => {
            host::w32_mask(h, REG_TXBWCTL, 0xf, 0xa);
            host::w32_mask(h, REG_TXBWCTL, 0xc0, 0x0);
            host::w32_mask(h, REG_TXBWCTL, 0xff00,
                           (primary_ch_idx as u32) | ((primary_ch_idx as u32) << 4));
            host::w32_mask(h, REG_SBD, BITS_SUBTUNE, 0x6);
            host::w32_mask(h, REG_PT_CHSMO, BIT_PT_OPT, 0x1);
        }
        5 => {
            host::w32_mask(h, REG_DFIRBW, 0x3FF0, 0x2AB);
            host::w32_mask(h, REG_TXBWCTL, 0xf, 0x0);
            host::w32_mask(h, REG_TXBWCTL, 0xffc0, 0x1);
            host::w32_mask(h, REG_TXCLK, 0x700, 0x4);
            host::w32_mask(h, REG_TXCLK, 0x700000, 0x4);
            host::w32_mask(h, REG_CCK_SOURCE, BIT_NBI_EN, 0x0);
            host::w32_mask(h, REG_SBD, BITS_SUBTUNE, 0x1);
            host::w32_mask(h, REG_PT_CHSMO, BIT_PT_OPT, 0x0);
        }
        6 => {
            host::w32_mask(h, REG_DFIRBW, 0x3FF0, 0x2AB);
            host::w32_mask(h, REG_TXBWCTL, 0xf, 0x0);
            host::w32_mask(h, REG_TXBWCTL, 0xffc0, 0x2);
            host::w32_mask(h, REG_TXCLK, 0x700, 0x6);
            host::w32_mask(h, REG_TXCLK, 0x700000, 0x5);
            host::w32_mask(h, REG_CCK_SOURCE, BIT_NBI_EN, 0x0);
            host::w32_mask(h, REG_SBD, BITS_SUBTUNE, 0x1);
            host::w32_mask(h, REG_PT_CHSMO, BIT_PT_OPT, 0x0);
        }
        _ => {}
    }
}

/// rtw8822c.c:2399-2434 `rtw8822c_set_channel_rf`.
///
/// Eine einzige Zahl — RF-Register 0x18 — traegt Band, Kanal, RFSI und
/// Bandbreite; sie wird gelesen, feldweise geloescht und neu gesetzt.
fn set_channel_rf(h: i32, channel: u8, bw: usize) {
    const RF18_BAND_MASK: u32 = (1 << 16) | (1 << 9) | (1 << 8);
    const RF18_BAND_2G: u32 = 0;
    const RF18_BAND_5G: u32 = (1 << 16) | (1 << 8);
    const RF18_CHANNEL_MASK: u32 = MASKBYTE0;
    const RF18_RFSI_MASK: u32 = (1 << 18) | (1 << 17);
    const RF18_RFSI_GE_CH80: u32 = 1 << 17;
    const RF18_RFSI_GT_CH140: u32 = 1 << 18;
    const RF18_BW_MASK: u32 = (1 << 13) | (1 << 12);
    const RF18_BW_20M: u32 = (1 << 13) | (1 << 12);
    const RF18_BW_40M: u32 = 1 << 13;
    const RF18_BW_80M: u32 = 1 << 12;

    let mut rf_reg18 = phy::read_rf(h, RF_PATH_A, 0x18, RFREG_MASK);
    rf_reg18 &= !(RF18_BAND_MASK | RF18_CHANNEL_MASK | RF18_RFSI_MASK | RF18_BW_MASK);
    rf_reg18 |= if is_ch_2g(channel) { RF18_BAND_2G } else { RF18_BAND_5G };
    rf_reg18 |= (channel as u32) & RF18_CHANNEL_MASK;
    if is_ch_5g_band_4(channel) {
        rf_reg18 |= RF18_RFSI_GT_CH140;
    } else if is_ch_5g_band_3(channel) {
        rf_reg18 |= RF18_RFSI_GE_CH80;
    }

    let rf_rxbb: u32 = match bw {
        1 => { rf_reg18 |= RF18_BW_40M; 0x10 }
        2 => { rf_reg18 |= RF18_BW_80M; 0x8 }
        // RTW_CHANNEL_WIDTH_5/10/20 und Linux' `default:`
        _ => { rf_reg18 |= RF18_BW_20M; 0x18 }
    };

    rstb_3wire(h, false);

    for path in [RF_PATH_A, RF_PATH_B] {
        phy::write_rf_reg_mix(h, path, RF_LUTWE2, 0x04, 0x01);
        phy::write_rf_reg_mix(h, path, RF_LUTWA, 0x1f, 0x12);
        phy::write_rf_reg_mix(h, path, RF_LUTWD0, 0xfffff, rf_rxbb);
        phy::write_rf_reg_mix(h, path, RF_LUTWE2, 0x04, 0x00);
    }

    phy::write_rf_reg_mix(h, RF_PATH_A, RF_CFGCH, RFREG_MASK, rf_reg18);
    phy::write_rf_reg_mix(h, RF_PATH_B, RF_CFGCH, RFREG_MASK, rf_reg18);

    rstb_3wire(h, true);
}

/// rtw8822c.c:2529-2546 `rtw8822c_set_channel`
pub fn set_channel(h: i32, channel: u8, bw: usize, primary_ch_idx: u8) {
    set_channel_bb(h, channel, bw, primary_ch_idx);
    crate::mac::set_channel_mac(h, channel, bw, primary_ch_idx);
    set_channel_rf(h, channel, bw);
    toggle_igi(h);
}

/// rtw8822c.c:2693-2713 `rtw8822c_set_write_tx_power_ref`.
///
/// Zwei Bezugswerte je Pfad — CCK und OFDM — in vier festen Registern.
/// Vor JEDEM Schreibzugriff wird `0x1c90` Bit 15 geloescht; das ist kein
/// Versehen und keine Schleifeninvariante, es steht so da.
fn set_write_tx_power_ref(h: i32, rf_path_num: u8,
                          tx_pwr_ref_cck: [u8; 2], tx_pwr_ref_ofdm: [u8; 2]) {
    const TXREF_CCK: [u32; 2] = [0x18a0, 0x41a0];
    const TXREF_OFDM: [u32; 2] = [0x18e8, 0x41e8];

    for path in 0..rf_path_num as usize {
        host::w32_mask(h, 0x1c90, 1 << 15, 0);
        host::w32_mask(h, TXREF_CCK[path], 0x7f0000, tx_pwr_ref_cck[path] as u32);
    }
    for path in 0..rf_path_num as usize {
        host::w32_mask(h, 0x1c90, 1 << 15, 0);
        host::w32_mask(h, TXREF_OFDM[path], 0x1fc00, tx_pwr_ref_ofdm[path] as u32);
    }
}

/// rtw8822c.c:2566-2586 `rtw8822c_set_tx_power_diff`
fn set_tx_power_diff(h: i32, rate: u8, diff_idx: &[i8; 4]) {
    const OFFSET_TXAGC: u32 = 0x3a00;
    let rate_idx = (rate & 0xfc) as u32;
    let mut pwr_idx = [0u32; 4];
    for i in 0..4 {
        pwr_idx[i] = (diff_idx[i] as u8 & 0x7f) as u32;
    }
    let phy_pwr_idx = pwr_idx[0] | (pwr_idx[1] << 8) | (pwr_idx[2] << 16)
        | (pwr_idx[3] << 24);

    // rtw8822c.c:2578 — `0x1c90` ist REG_RSTB, Bit 15.
    host::w32_mask(h, 0x1c90, 1 << 15, 0x0);
    host::w32_mask(h, OFFSET_TXAGC + rate_idx, MASKDWORD, phy_pwr_idx);
}

/// rtw8822c.c:2588-2620 `rtw8822c_set_tx_power_index`.
///
/// Schreibt nicht die Indizes selbst, sondern zwei BEZUGSwerte (CCK und
/// MCS7) und je Vierergruppe von Raten die ABWEICHUNG davon — und zwar das
/// Minimum beider Pfade.
pub fn set_tx_power_index(h: i32, rf_path_num: u8,
                          tbl: &[[u8; crate::txpower::DESC_RATE_MAX];
                                 crate::txpower::RTW_RF_PATH_MAX]) {
    const DESC_RATE11M: usize = 0x03;
    const DESC_RATEMCS7: usize = 0x13;
    const RATE_SECTION_2SS_MAX: usize = 5; // __RTW_RATE_SECTION_2SS_MAX

    let pwr_ref_cck = [tbl[RF_PATH_A][DESC_RATE11M], tbl[RF_PATH_B][DESC_RATE11M]];
    let pwr_ref_ofdm = [tbl[RF_PATH_A][DESC_RATEMCS7], tbl[RF_PATH_B][DESC_RATEMCS7]];

    set_write_tx_power_ref(h, rf_path_num, pwr_ref_cck, pwr_ref_ofdm);

    let mut diff_idx = [0i8; 4];
    for rs in 0..=RATE_SECTION_2SS_MAX {
        for &rate in crate::tables::RATE_SECTION[rs].iter() {
            let pwr_a = tbl[RF_PATH_A][rate as usize];
            let pwr_b = tbl[RF_PATH_B][rate as usize];
            let (diff_a, diff_b) = if rs == 0 {
                (pwr_a as i8 - pwr_ref_cck[0] as i8,
                 pwr_b as i8 - pwr_ref_cck[1] as i8)
            } else {
                (pwr_a as i8 - pwr_ref_ofdm[0] as i8,
                 pwr_b as i8 - pwr_ref_ofdm[1] as i8)
            };
            diff_idx[(rate % 4) as usize] = diff_a.min(diff_b);
            if rate % 4 == 3 {
                set_tx_power_diff(h, rate - 3, &diff_idx);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Was `rtw_watch_dog_work` alle zwei Sekunden an DIESEM Chip tut.
// rtw8822c.c — Quarz, Sendeleistung, CCK-Schwelle, Sendepfad.
// ═══════════════════════════════════════════════════════════════════

/// rtw8822c.c:2523-2527 `rtw8822c_config_tx_path`
pub fn config_tx_path(h: i32, tx_path: u8, tx_path_sel_1ss: u8,
                      tx_path_cck: u8, is_tx2_path: bool) {
    config_cck_tx_path(h, tx_path_cck, is_tx2_path);
    config_ofdm_tx_path(h, tx_path, tx_path_sel_1ss);
    bb_reset(h);
}

/// rtw8822c.c:4343-4352 `rtw8822c_cck_pd_reg[bw][nrx]` —
/// `(reg_pd, mask_pd, reg_cs, mask_cs)`.
const CCK_PD_REG: [[(u32, u32, u32, u32); 2]; 2] = [
    [(0x1ac8, 0x00ff, 0x1ad0, 0x0000_001f),
     (0x1ac8, 0xff00, 0x1ad0, 0x0000_03e0)],
    [(0x1acc, 0x00ff, 0x1ad0, 0x01f0_0000),
     (0x1acc, 0xff00, 0x1ad0, 0x3e00_0000)],
];

/// rtw8822c.c:4359-4390 `rtw8822c_phy_cck_pd_set_reg`
fn phy_cck_pd_set_reg(h: i32, pd_diff: i8, cs_diff: i8, bw: usize, nrx: usize) {
    if bw > 1 || nrx >= 2 {
        return; // Linux: WARN_ON und zurueck
    }
    let (reg_pd, mask_pd, reg_cs, mask_cs) = CCK_PD_REG[bw][nrx];

    let mut pd = host::r32_mask(h, reg_pd, mask_pd) as i32;
    let mut cs = host::r32_mask(h, reg_cs, mask_cs) as i32;
    pd += pd_diff as i32;
    cs += cs_diff as i32;
    if pd > RTW_CCK_PD_MAX as i32 {
        pd = RTW_CCK_PD_MAX as i32;
    }
    if cs == RTW_CCK_CS_ERR1 as i32 || cs == RTW_CCK_CS_ERR2 as i32 {
        cs += 1;
    } else if cs > RTW_CCK_CS_MAX as i32 {
        cs = RTW_CCK_CS_MAX as i32;
    }
    host::w32_mask(h, reg_pd, mask_pd, pd as u32);
    host::w32_mask(h, reg_cs, mask_cs, cs as u32);
}

/// rtw8822c.c:4392-4413 `rtw8822c_phy_cck_pd_set`
pub fn phy_cck_pd_set(h: i32, dm: &mut DmInfo, new_lvl: u8) {
    let pd_lvl: [i8; 5] = [0, 2, 4, 6, 8];
    let cs_lvl: [i8; 5] = [0, 2, 2, 2, 4];

    let nrx = host::r32_mask(h, 0x1a2c, 0x60000) as usize;
    let bw = host::r32_mask(h, 0x9b0, 0xc) as usize;
    if bw > 1 || nrx >= 4 {
        return;
    }

    if dm.cck_pd_lv[bw][nrx] == new_lvl {
        return;
    }
    let cur_lvl = dm.cck_pd_lv[bw][nrx];

    // „update cck pd info"
    dm.cck_fa_avg = CCK_FA_AVG_RESET;

    phy_cck_pd_set_reg(h,
                       pd_lvl[new_lvl as usize] - pd_lvl[cur_lvl as usize],
                       cs_lvl[new_lvl as usize] - cs_lvl[cur_lvl as usize],
                       bw, nrx.min(1));
    dm.cck_pd_lv[bw][nrx] = new_lvl;
}

// ── Der Quarz: rtw8822c_cfo_track und seine vier Helfer ──────────
//
// **Das ist die Sendeseite der Temperatur.** Ein Empfaenger rastet sich
// an jeder Praeambel neu auf die Frequenz des Gegenuebers ein; ein
// Sender laeuft auf dem eigenen Quarz. Waermt der Chip ueber Minuten
// auf, wandert der — und die Leitung wird einseitig, ohne dass
// irgendwo ein Fehler steht.

/// rtw8822c.c:4220 `#define XCAP_EXTEND(val) (val | val << 7)`
fn xcap_extend(v: u8) -> u32 {
    let v = v as u32;
    v | (v << 7)
}

/// rtw8822c.c:4222-4231 `rtw8822c_set_crystal_cap_reg`
fn set_crystal_cap_reg(h: i32, dm: &mut DmInfo, crystal_cap: u8) {
    let val = xcap_extend(crystal_cap);
    dm.cfo_track.crystal_cap = crystal_cap;
    host::w32_mask(h, REG_ANAPAR_XTAL_0, BIT_XCAP_0, val);
}

/// rtw8822c.c:4233-4241 `rtw8822c_set_crystal_cap`
fn set_crystal_cap(h: i32, dm: &mut DmInfo, crystal_cap: u8) {
    if dm.cfo_track.crystal_cap == crystal_cap {
        return;
    }
    set_crystal_cap_reg(h, dm, crystal_cap);
}

/// rtw8822c.c:4243-4255 `rtw8822c_cfo_tracking_reset`
fn cfo_tracking_reset(h: i32, dm: &mut DmInfo, efuse_crystal_cap: u8) {
    dm.cfo_track.is_adjust = true;

    if dm.cfo_track.crystal_cap > efuse_crystal_cap {
        let v = dm.cfo_track.crystal_cap - 1;
        set_crystal_cap(h, dm, v);
    } else if dm.cfo_track.crystal_cap < efuse_crystal_cap {
        let v = dm.cfo_track.crystal_cap + 1;
        set_crystal_cap(h, dm, v);
    }
}

/// rtw8822c.c:4265 `#define REPORT_TO_KHZ(val) ((val << 1) + (val >> 1))`
fn report_to_khz(v: i32) -> i32 {
    (v << 1) + (v >> 1)
}

/// rtw8822c.c:4267-4289 `rtw8822c_cfo_calc_avg` — und sie LEERT die
/// Summen, ist also nicht wiederholbar.
fn cfo_calc_avg(dm: &mut DmInfo, path_num: u8) -> i32 {
    let mut cfo_path_sum = 0i32;

    for i in 0..path_num as usize {
        let cfo_rpt_sum = report_to_khz(dm.cfo_track.cfo_tail[i]);
        let cfo_avg = if dm.cfo_track.cfo_cnt[i] != 0 {
            cfo_rpt_sum / dm.cfo_track.cfo_cnt[i]
        } else {
            0
        };
        cfo_path_sum += cfo_avg;
    }

    for i in 0..path_num as usize {
        dm.cfo_track.cfo_tail[i] = 0;
        dm.cfo_track.cfo_cnt[i] = 0;
    }

    if path_num == 0 { 0 } else { cfo_path_sum / path_num as i32 }
}

/// rtw8822c.c:4291-4308 `rtw8822c_cfo_need_adjust`.
///
/// **Der Riegel am Ende ist kein Detail:** laeuft Bluetooth im selben
/// Chip (`!rtw_coex_disabled`), stellt Linux die Nachfuehrung AB und
/// setzt den Quarz auf den Wert der efuse zurueck. Wer nur diese
/// Funktion portiert und die Koexistenz nicht, baut den Riegel mit —
/// und `bt_disabled` kommt aus dem Zustand, den der Watchdog pflegt.
fn cfo_need_adjust(h: i32, dm: &mut DmInfo, cfo_avg: i32,
                   efuse_crystal_cap: u8, bt_disabled: bool) {
    if !dm.cfo_track.is_adjust {
        if cfo_avg.abs() > CFO_TRK_ENABLE_TH {
            dm.cfo_track.is_adjust = true;
        }
    } else if cfo_avg.abs() <= CFO_TRK_STOP_TH {
        dm.cfo_track.is_adjust = false;
    }

    if !bt_disabled {
        dm.cfo_track.is_adjust = false;
        set_crystal_cap(h, dm, efuse_crystal_cap);
    }
}

/// rtw8822c.c:4310-4336 `rtw8822c_cfo_track`.
///
/// `sta_cnt != 1` heisst bei uns: keine Verbindung. Ohne genau EINE
/// Gegenstelle ist ein gemittelter Frequenzversatz sinnlos, und Linux
/// faehrt dann die Nachfuehrung schrittweise auf die efuse zurueck.
pub fn cfo_track(h: i32, dm: &mut DmInfo, path_num: u8,
                 efuse_crystal_cap: u8, linked: bool, bt_disabled: bool) {
    if !linked {
        cfo_tracking_reset(h, dm, efuse_crystal_cap);
        return;
    }

    if dm.cfo_track.packet_count == dm.cfo_track.packet_count_pre {
        return;
    }

    dm.cfo_track.packet_count_pre = dm.cfo_track.packet_count;
    let cfo_avg = cfo_calc_avg(dm, path_num);
    cfo_need_adjust(h, dm, cfo_avg, efuse_crystal_cap, bt_disabled);

    if dm.cfo_track.is_adjust {
        let mut crystal_cap = dm.cfo_track.crystal_cap as i8;
        if cfo_avg > CFO_TRK_ADJ_TH {
            crystal_cap += 1;
        } else if cfo_avg < -CFO_TRK_ADJ_TH {
            crystal_cap -= 1;
        }
        let crystal_cap = crystal_cap.clamp(0, XCAP_MASK as i8) as u8;
        set_crystal_cap(h, dm, crystal_cap);
    }
}

// ── Die Sendeleistung ueber die Temperatur ───────────────────────

/// rtw8822c.c:4405-4420 `rtw8822c_pwrtrack_set`
fn pwrtrack_set(h: i32, dm: &DmInfo, rf_path: usize) {
    match rf_path {
        RF_PATH_A => host::w32_mask(h, 0x18a0, PWR_TRACK_MASK,
                                    dm.delta_power_index[rf_path] as u32),
        RF_PATH_B => host::w32_mask(h, 0x41a0, PWR_TRACK_MASK,
                                    dm.delta_power_index[rf_path] as u32),
        _ => {}
    }
}

/// rtw8822c.c:4422-4431 `rtw8822c_pwr_track_stats`.
///
/// `0xff` in der efuse heisst „kein Thermometer fuer diesen Pfad" — dann
/// gibt es nichts zu mitteln, und ein Mittelwert aus 0xff waere eine
/// erfundene Temperatur.
fn pwr_track_stats(h: i32, dm: &mut DmInfo, thermal_meter: &[u8],
                   path: usize) {
    if thermal_meter[path] == 0xff {
        return;
    }
    let thermal_value = phy::read_rf(h, path, RF_T_METER, 0x7e) as u8;
    phy::pwrtrack_avg(dm, thermal_value, path);
}

/// rtw8822c.c:4433-4444 `rtw8822c_pwr_track_path`
fn pwr_track_path(h: i32, dm: &mut DmInfo, swing: &phy::SwingTable,
                  thermal_meter: &[u8], path: usize) {
    let delta = phy::pwrtrack_get_delta(dm, thermal_meter, path);
    dm.delta_power_index[path] =
        phy::pwrtrack_get_pwridx(dm, swing, thermal_meter, path, path, delta);
    pwrtrack_set(h, dm, path);
}

/// rtw8822c.c:4446-4459 `__rtw8822c_pwr_track`
fn pwr_track_inner(h: i32, dm: &mut DmInfo, thermal_meter: &[u8],
                   rf_path_num: u8, channel: u8) {
    let swing = phy::config_swing_table(channel, dm.tx_rate);

    for i in 0..rf_path_num as usize {
        pwr_track_stats(h, dm, thermal_meter, i);
    }
    if phy::pwrtrack_need_lck(dm) {
        do_lck(h);
    }
    for i in 0..rf_path_num as usize {
        pwr_track_path(h, dm, &swing, thermal_meter, i);
    }
}

/// rtw8822c.c:4461-4481 `rtw8822c_pwr_track`.
///
/// **Zwei Takte, nicht einer.** Im ersten wird das Thermometer nur
/// ANGESTOSSEN, im zweiten gelesen — die Wandlung braucht Zeit, und ein
/// Wert, der im selben Takt gelesen wird, ist der alte.
pub fn pwr_track(h: i32, dm: &mut DmInfo, power_track_type: u8,
                 thermal_meter: &[u8], rf_path_num: u8, channel: u8) {
    if power_track_type != 0 {
        return;
    }

    if !dm.pwr_trk_triggered {
        phy::write_rf_reg_mix(h, RF_PATH_A, RF_T_METER, 1 << 19, 0x01);
        phy::write_rf_reg_mix(h, RF_PATH_A, RF_T_METER, 1 << 19, 0x00);
        phy::write_rf_reg_mix(h, RF_PATH_A, RF_T_METER, 1 << 19, 0x01);

        phy::write_rf_reg_mix(h, RF_PATH_B, RF_T_METER, 1 << 19, 0x01);
        phy::write_rf_reg_mix(h, RF_PATH_B, RF_T_METER, 1 << 19, 0x00);
        phy::write_rf_reg_mix(h, RF_PATH_B, RF_T_METER, 1 << 19, 0x01);

        dm.pwr_trk_triggered = true;
        return;
    }

    pwr_track_inner(h, dm, thermal_meter, rf_path_num, channel);
    dm.pwr_trk_triggered = false;
}

/// rtw8822c.c:2136-2153 `rtw8822c_do_lck` — den Synthesizer neu
/// einrasten lassen, wenn die Temperatur weit genug gewandert ist.
fn do_lck(h: i32) {
    phy::write_rf_reg_mix(h, RF_PATH_A, RF_SYN_CTRL, RFREG_MASK, 0x80010);
    phy::write_rf_reg_mix(h, RF_PATH_A, RF_SYN_PFD, RFREG_MASK, 0x1F0FA);
    host::delay_us(1);
    phy::write_rf_reg_mix(h, RF_PATH_A, RF_AAC_CTRL, RFREG_MASK, 0x80000);
    phy::write_rf_reg_mix(h, RF_PATH_A, RF_SYN_AAC, RFREG_MASK, 0x80001);
    // read_poll_timeout(…, val != 0x1, 1000, 100000, …): 100 ms Frist,
    // alle 1000 us nachsehen.
    let t0 = host::now_us();
    while host::now_us() - t0 < 100_000 {
        if phy::read_rf(h, RF_PATH_A, RF_AAC_CTRL, 0x1000) != 0x1 {
            break;
        }
        host::delay_us(1000);
    }
    phy::write_rf_reg_mix(h, RF_PATH_A, RF_SYN_PFD, RFREG_MASK, 0x1F0F8);
    phy::write_rf_reg_mix(h, RF_PATH_B, RF_SYN_CTRL, RFREG_MASK, 0x80010);

    phy::write_rf_reg_mix(h, RF_PATH_A, RF_FAST_LCK, RFREG_MASK, 0x0f000);
    phy::write_rf_reg_mix(h, RF_PATH_A, RF_FAST_LCK, RFREG_MASK, 0x4f000);
    host::delay_us(1);
    phy::write_rf_reg_mix(h, RF_PATH_A, RF_FAST_LCK, RFREG_MASK, 0x0f000);
}
