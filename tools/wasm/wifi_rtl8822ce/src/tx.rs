//! `tx.c` aus Linux 6.18.26 rtw88 — nur der Teil, den der Firmware-Download
//! braucht: der 48-Byte-Sendedeskriptor einer Reserved Page.
//!
//! Portiert: `rtw_tx_pkt_info_update_rate` · `rtw_tx_pkt_info_update_sec` ·
//! `rtw_tx_rsvd_page_pkt_info_update` (Typ `RSVD_BEACON`) ·
//! `rtw_tx_fill_tx_desc`.
#![allow(dead_code)]

use crate::regs::{DESC_RATEMCS7, DESC_RATEMCS15};

// main.h:232-248
pub const RTW_RATEID_G: u8 = 7;
pub const RTW_RATEID_B_20M: u8 = 8;
// main.h:250-262
pub const DESC_RATE1M: u8 = 0x00;
pub const DESC_RATE6M: u8 = 0x04;
pub const DESC_RATE24M: u8 = 0x08;
// main.h:87
pub const RTW_BAND_2G: u8 = 1;
pub const RTW_BAND_5G: u8 = 2;

/// Das Band zu einem Kanal. **Kein Rateschritt, sondern die Frage, ob es
/// die Rate ueberhaupt gibt**: `pkt_info_update_rate` waehlt fuer 2,4 GHz
/// 1 Mbit DSSS, und DSSS gibt es oberhalb von Kanal 14 nicht.
pub fn band_of(channel: u8) -> u8 {
    if channel > 14 { RTW_BAND_5G } else { RTW_BAND_2G }
}

// tx.h:79-83 — abgelesen, nicht abgeleitet. HIGH ist 17 und H2C 19; mit
// einem geratenen 17 fuer H2C haette `more_data` (qsel == HIGH) still
// mitgefeuert.
pub const TX_DESC_QSEL_BEACON: u8 = 16;
pub const TX_DESC_QSEL_HIGH: u8 = 17;
pub const TX_DESC_QSEL_MGMT: u8 = 18;
pub const TX_DESC_QSEL_H2C: u8 = 19;

/// rtw8822c.c `rtw8822c_hw_spec.tx_pkt_desc_sz`
pub const TX_PKT_DESC_SZ: usize = 48;

/// `struct rtw_tx_pkt_info` (tx.h) — nur die Felder, die der
/// Reserved-Page-Weg setzt. Alles andere bleibt null, wie in Linux, wo
/// `pkt_info` als `{0}` angelegt wird.
#[derive(Default)]
pub struct TxPktInfo {
    pub tx_pkt_size: u32,
    pub offset: u8,
    pub bmc: bool,
    pub ls: bool,
    pub dis_qselseq: bool,
    pub mac_id: u8,
    pub qsel: u8,
    pub rate_id: u8,
    pub sec_type: u8,
    pub pkt_offset: u8,
    pub ampdu_en: bool,
    pub report: bool,
    pub ampdu_density: u8,
    pub bt_null: bool,
    pub hw_ssn_sel: u8,
    pub use_rate: bool,
    pub dis_rate_fallback: bool,
    pub rts: bool,
    pub nav_use_hdr: bool,
    pub ampdu_factor: u8,
    pub rate: u8,
    pub short_gi: bool,
    pub bw: u8,
    pub ldpc: bool,
    pub stbc: u8,
    pub sn: u16,
    pub en_hwseq: bool,
    pub seq: u16,
    pub tim_offset: u16,
}

#[inline]
fn bits(v: u32, mask: u32) -> u32 {
    (v << mask.trailing_zeros()) & mask
}

/// tx.c:44-58 `rtw_get_mgmt_rate`.
///
/// Ohne vif, ohne `basic_rates`, mit `ignore_rate` oder mit
/// `RTW_FLAG_FORCE_LOWEST_RATE` kommt `lowest_rate` heraus. Vor dem
/// Verbinden gibt es keine `basic_rates` — die kennt erst
/// `bss_conf`, also frueher als Stufe 5c nie. Der `__ffs`-Zweig ist
/// damit noch nicht erreichbar und steht bewusst nicht da; er waere eine
/// Behauptung ueber einen Zustand, den der Treiber nicht fuehrt.
fn get_mgmt_rate(lowest_rate: u8) -> u8 {
    lowest_rate
}

/// tx.c:60-77 `rtw_tx_pkt_info_update_rate`.
fn pkt_info_update_rate(info: &mut TxPktInfo, current_band_type: u8) {
    if current_band_type == RTW_BAND_2G {
        info.rate_id = RTW_RATEID_B_20M;
        info.rate = get_mgmt_rate(DESC_RATE1M);
    } else {
        info.rate_id = RTW_RATEID_G;
        info.rate = get_mgmt_rate(DESC_RATE6M);
    }
    info.use_rate = true;
    info.dis_rate_fallback = true;
}

/// tx.c `rtw_tx_rsvd_page_pkt_info_update` fuer `type == RSVD_BEACON`.
///
/// **`bmc` wird aus dem NUTZDATEN gelesen**, und das ist kein Versehen:
/// Linux legt `struct ieee80211_hdr *hdr = skb->data` auf den Puffer, auch
/// wenn darin Firmware steht. `addr1` liegt bei Offset 4. Wir machen es
/// genauso — eine Abweichung hier waere ein anderer Deskriptor als der, den
/// der Chip unter Linux bekommt.
pub fn rsvd_page_pkt_info_update(payload: &[u8], current_band_type: u8) -> TxPktInfo {
    let mut info = TxPktInfo::default();

    // type == RSVD_BEACON -> qsel wird hier NICHT gesetzt (pci.c tut es).
    pkt_info_update_rate(&mut info, current_band_type);

    let a1 = &payload[4..10.min(payload.len())];
    let bcast = a1.len() == 6 && a1.iter().all(|&b| b == 0xff);
    let mcast = !a1.is_empty() && a1[0] & 0x01 != 0;
    info.bmc = bcast || mcast;

    info.tx_pkt_size = payload.len() as u32;
    info.offset = TX_PKT_DESC_SZ as u8;
    info.ls = true;
    // RSVD_BEACON ist kein RSVD_PS_POLL:
    info.dis_qselseq = true;
    info.en_hwseq = true;
    info.hw_ssn_sel = 0;
    // RSVD_BEACON mit leerer rsvd_page_list -> tim_offset bleibt 0.
    // `rtw_tx_pkt_info_update_sec` ohne hw_key -> sec_type bleibt 0.
    info
}

/// tx.c `rtw_tx_fill_tx_desc`. Schreibt die 48 Bytes an den Anfang von `desc`.
pub fn fill_tx_desc(info: &TxPktInfo, desc: &mut [u8; TX_PKT_DESC_SZ]) {
    let more_data = info.qsel == TX_DESC_QSEL_HIGH;

    let w0 = bits(info.tx_pkt_size, 0x0000_FFFF)          // TXPKTSIZE 15:0
        | bits(info.offset as u32, 0x00FF_0000)           // OFFSET 23:16
        | bits(info.bmc as u32, 1 << 24)                  // BMC
        | bits(info.ls as u32, 1 << 26)                   // LS
        | bits(info.dis_qselseq as u32, 1 << 31);         // DISQSELSEQ

    let w1 = bits(info.mac_id as u32, 0x0000_00FF)        // MACID 7:0
        | bits(info.qsel as u32, 0x0000_1F00)             // QSEL 12:8
        | bits(info.rate_id as u32, 0x001F_0000)          // RATE_ID 20:16
        | bits(info.sec_type as u32, 0x00C0_0000)         // SEC_TYPE 23:22
        | bits(info.pkt_offset as u32, 0x1F00_0000)       // PKT_OFFSET 28:24
        | bits(more_data as u32, 1 << 29);                // MORE_DATA

    let w2 = bits(info.ampdu_en as u32, 1 << 12)          // AGG_EN
        | bits(info.report as u32, 1 << 19)               // SPE_RPT
        | bits(info.ampdu_density as u32, 0x0070_0000)    // AMPDU_DEN 22:20
        | bits(info.bt_null as u32, 1 << 23);             // BT_NULL

    let w3 = bits(info.hw_ssn_sel as u32, 0x0000_00C0)    // HW_SSN_SEL 7:6
        | bits(info.use_rate as u32, 1 << 8)              // USE_RATE
        | bits(info.dis_rate_fallback as u32, 1 << 10)    // DISDATAFB
        | bits(info.rts as u32, 1 << 12)                  // USE_RTS
        | bits(info.nav_use_hdr as u32, 1 << 15)          // NAVUSEHDR
        | bits(info.ampdu_factor as u32, 0x003E_0000);    // MAX_AGG_NUM 21:17

    // `old_datarate_fb_limit` ist beim 8822C false — der Zweig entfaellt.
    let mut w4 = bits(info.rate as u32, 0x0000_007F);     // DATARATE 6:0

    let mut w5 = bits(info.short_gi as u32, 1 << 4)       // DATA_SHORT
        | bits(info.bw as u32, 0x0000_0060)               // DATA_BW 6:5
        | bits(info.ldpc as u32, 1 << 7)                  // DATA_LDPC
        | bits(info.stbc as u32, 0x0000_0300);            // DATA_STBC 9:8

    let w6 = bits(info.sn as u32, 0x0000_0FFF);           // SW_DEFINE 11:0
    let w8 = bits(info.en_hwseq as u32, 1 << 15);         // EN_HWSEQ
    let mut w9 = bits(info.seq as u32, 0x00FF_F000);      // SW_SEQ 23:12

    if info.rts {
        w4 |= bits(DESC_RATE24M as u32, 0x1F00_0000);     // RTSRATE 28:24
        w5 |= bits(1, 1 << 12);                           // DATA_RTS_SHORT
    }
    if info.tim_offset != 0 {
        w9 |= bits(1, 1 << 7)                             // TIM_EN
            | bits(info.tim_offset as u32, 0x0000_007F);  // TIM_OFFSET 6:0
    }

    desc.fill(0);
    for (i, w) in [w0, w1, w2, w3, w4, w5, w6, 0, w8, w9].iter().enumerate() {
        desc[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
}

/// tx.c:524-546 `rtw_tx_write_data_h2c_get`.
///
/// Linux legt dafuer ein skb an, reserviert `tx_pkt_desc_sz` Vorlauf und
/// kopiert die 32 Bytes dahinter. Das ist bei uns der Staging-Puffer; vom
/// `pkt_info` setzt die Funktion **genau ein Feld**, alles andere bleibt
/// null — auch `offset` und `ls`, anders als beim Reserved-Page-Weg.
pub fn write_data_h2c_get(size: u32) -> TxPktInfo {
    TxPktInfo { tx_pkt_size: size, ..Default::default() }
}

// ── Stufe 5b: ein gewoehnlicher Rahmen statt einer Reserved Page ──

/// tx.c:391-398 `rtw_tx_mgmt_pkt_info_update`
fn mgmt_pkt_info_update(info: &mut TxPktInfo, current_band_type: u8) {
    pkt_info_update_rate(info, current_band_type);
    info.dis_qselseq = true;
    info.en_hwseq = true;
    info.hw_ssn_sel = 0;
    // Linux: „TODO: need to change hw port and hw ssn sel for multiple vifs"
}

/// main.h:202-215 — die Queues, plus `ac_to_hwq`.
pub const RTW_TX_QUEUE_BCN: usize = 4;
pub const RTW_TX_QUEUE_MGMT: usize = 5;
pub const RTW_TX_QUEUE_HI0: usize = 6;

/// tx.c:641-662 `rtw_tx_queue_mapping`.
///
/// `q_mapping` ist die Zugangsklasse, die mac80211 an den skb haengt; ohne
/// obere Haelfte gibt es sie nicht, und deshalb steht hier BE — derselbe
/// Wert, den Linux' `WARN_ON_ONCE`-Zweig nimmt, wenn sie fehlt.
pub fn queue_mapping(fc: u16, addr1: &[u8]) -> usize {
    let is_beacon = fc & 0xfc == 0x80;
    let ftype = (fc >> 2) & 0x3;
    let is_mgmt = ftype == 0;
    let is_ctl = ftype == 1;
    let bcast = addr1.len() == 6 && addr1.iter().all(|&b| b == 0xff);
    let mcast = !addr1.is_empty() && addr1[0] & 0x01 != 0;

    if is_beacon {
        RTW_TX_QUEUE_BCN
    } else if is_mgmt || is_ctl {
        RTW_TX_QUEUE_MGMT
    } else if bcast || mcast {
        RTW_TX_QUEUE_HI0
    } else {
        crate::pci::Q_BE
    }
}

/// tx.c:414-455 `rtw_tx_pkt_info_update`, fuer einen Rahmen OHNE `sta`.
///
/// **Was nicht da steht und warum:** `si`/`rtwvif` fuer `mac_id` kommt vom
/// Rufer (bei uns die 0 aus `vif::add_interface_station`) ·
/// `rtw_tx_data_pkt_info_update` gehoert zum Datenzweig, den es vor einer
/// Verbindung nicht gibt · `rtw_tx_report_enable` haengt an
/// `IEEE80211_TX_CTL_REQ_TX_STATUS`, einer Fahne der oberen Haelfte ·
/// `rtw_tx_pkt_info_update_sec` ohne `hw_key` laesst `sec_type` auf null,
/// wie beim Reserved-Page-Weg · `rtw_tx_stats` zaehlt fuer die obere
/// Haelfte.
pub fn pkt_info_update(frame: &[u8], mac_id: u8, current_band_type: u8)
    -> TxPktInfo
{
    let mut info = TxPktInfo { mac_id, ..Default::default() };
    let fc = u16::from_le_bytes([frame[0], frame[1]]);
    let ftype = (fc >> 2) & 0x3;
    let subtype = (fc >> 4) & 0xf;
    // `ieee80211_is_nullfunc`: Datentyp, Subtyp 4 (Null) — hier mitgefuehrt,
    // weil Linux ihn in DENSELBEN Zweig schickt wie die Verwaltung.
    let is_mgmt = ftype == 0;
    let is_nullfunc = ftype == 2 && subtype == 4;
    if is_mgmt || is_nullfunc {
        mgmt_pkt_info_update(&mut info, current_band_type);
    }

    let a1 = &frame[4..10.min(frame.len())];
    let bcast = a1.len() == 6 && a1.iter().all(|&b| b == 0xff);
    let mcast = !a1.is_empty() && a1[0] & 0x01 != 0;
    info.bmc = bcast || mcast;

    info.tx_pkt_size = frame.len() as u32;
    info.offset = TX_PKT_DESC_SZ as u8;
    // `pkt_info->qsel = skb->priority` — bei uns setzt es `tx_qsel` aus der
    // Queue, genau wie `rtw_pci_get_tx_qsel` es fuer jede andere Queue tut.
    info.ls = true;
    info
}

/// tx.c:400-412 `rtw_tx_data_pkt_info_update`, Zweig MIT `sta`.
///
/// **Was `!sta` (Broadcast/Multicast) angeht, steht in Linux VOR dem
/// Sprung**: Rate 6M, rate_id 6, 20 MHz. Das ist hier der Zweig
/// `si = None`.
///
/// `ampdu_en` haengt an `IEEE80211_TX_CTL_AMPDU` — eine Fahne, die
/// mac80211 setzt, wenn ein Block-Ack-Block offen ist. Ohne
/// Block-Ack-Aushandlung gibt es sie nicht, und ein erfundenes
/// A-MPDU-Flag waere schlimmer als keins. `dm_info->fix_rate` ist eine
/// debugfs-Einstellung und steht auf `DESC_RATE_MAX` (= aus).
pub fn data_pkt_info_update(info: &mut TxPktInfo, seq: u16,
                            si: Option<&crate::sta::StaInfo>,
                            highest_rate: u8) {
    let mut rate = DESC_RATE6M;
    let mut rate_id = 6u8;
    let mut bw = 0u8; // RTW_CHANNEL_WIDTH_20
    let mut stbc = 0u8;
    let mut ldpc = 0u8;

    if let Some(s) = si {
        rate = highest_rate;
        bw = s.bw_mode;
        rate_id = s.rate_id;
        stbc = s.stbc_en;
        ldpc = s.ldpc_en;
    }

    info.seq = seq;
    info.ampdu_en = false;
    info.rate = rate;
    info.rate_id = rate_id;
    info.bw = bw;
    info.stbc = stbc;
    info.ldpc = ldpc != 0;
}

/// tx.c:112-123 `get_highest_ht_tx_rate`.
///
/// Die Bedingung ist `rf_type == RF_2T2R`, nicht `nss > 1` — auf diesem
/// Chip dasselbe, aber die Quelle fragt nach dem RF-Aufbau und nicht nach
/// der Zahl der Stroeme.
pub fn highest_ht_tx_rate(ht_mcs: &[u8; 4], rf_2t2r: bool) -> u8 {
    if rf_2t2r && ht_mcs[1] != 0 {
        DESC_RATEMCS15 as u8
    } else {
        DESC_RATEMCS7 as u8
    }
}

/// tx.c:125-165 `get_highest_vht_tx_rate`.
///
/// **VHT geht VOR HT** (tx.c:367-370), und das ist der Grund, warum es
/// diese Funktion hier ueberhaupt geben muss: bis hierher fragte der
/// Treiber nur `ht_supported`, und damit stand auf einer VHT-Verbindung
/// `DESC_RATEMCS15` im Deskriptor — eine HT-Rate auf einer Strecke, die
/// gerade VHT faehrt. Der Bericht meldete sie als „angeboten", waehrend
/// die Firmware VHT-Raten zurueckmeldete: zwei Antworten auf eine Frage.
///
/// Gelesen wird die SENDE-Karte des Gegenuebers (`tx_mcs_map`), zwei Bit
/// je Strom. Linux fragt `efuse->hw_cap.nss`, also UNSERE Stroeme — die
/// Karte sagt, wie hoch der andere darf, die efuse, wie viele Stroeme wir
/// ueberhaupt haben.
pub fn highest_vht_tx_rate(tx_mcs_map: u16, nss: u8) -> u8 {
    // **Die Basiswerte kommen aus `regs.rs`, nicht von Hand.** Erster
    // Entwurf hatte hier 0x2d und 0x37 stehen — abgeschrieben aus der
    // zweiten, verschobenen Liste in `txpower.rs`, die im selben Zug
    // herausgeflogen ist. Beide Zahlen waren um eins zu hoch, und eine
    // Rate um eins daneben ist eine ANDERE Rate.
    //
    // Die Reihen sind lueckenlos, MCS9 liegt neun ueber MCS0
    // (main.h:297-317).
    //
    // `IEEE80211_VHT_MCS_SUPPORT_0_7/0_8/0_9` = 0/1/2; 3 heisst „gar
    // nicht". Linux faellt fuer 3 in den `default`-Zweig, also auf MCS9 —
    // `rtw_update_sta_info` hat den Strom dann ohnehin schon aus der
    // Ratenmaske genommen.
    let top = |code: u16| -> u8 {
        match code {
            0 => 7,
            1 => 8,
            _ => 9,
        }
    };
    let vht1ss_mcs0 = crate::regs::DESC_RATEVHT1SS_MCS0 as u8;
    if nss == 1 {
        vht1ss_mcs0 + top(tx_mcs_map & 0x3)
    } else if nss >= 2 {
        crate::regs::DESC_RATEVHT2SS_MCS0 as u8 + top((tx_mcs_map & 0xc) >> 2)
    } else {
        // `nss == 0` — Linux' dritter Zweig. Er ist hier unerreichbar
        // (die efuse meldet 1 oder 2), steht aber da, weil ein Port, der
        // einen Zweig weglaesst, nicht mehr nachpruefbar ist.
        vht1ss_mcs0 + 9
    }
}

/// tx.c:166-177 `rtw_tx_report_enable`.
///
/// **Die Firmware quittiert einen Rahmen nur, wenn man danach fragt.**
/// Die Folgenummer liegt in den Bits 7:2 — die unteren zwei gehoeren
/// der Firmware, die oberen vier des Feldes sind reserviert. Es gibt
/// also 64 unterscheidbare Nummern, und der Zaehler laeuft um.
pub fn report_seqnum(counter: &mut u8) -> u8 {
    *counter = counter.wrapping_add(1);
    ((*counter as u16) << 2) as u8 & 0xfc
}
