//! `rx.c` aus Linux 6.18.26 rtw88, plus `query_phy_status_page0/1` und die
//! dB-Umrechnung aus `phy.c` — Stufe 5a.
//!
//! Portiert: `rtw_rx_query_rx_desc` · `query_phy_status` ·
//! `query_phy_status_page0` · `query_phy_status_page1` ·
//! `rtw_phy_power_2_db` · `rtw_phy_db_2_linear` · `rtw_phy_linear_2_db` ·
//! `rtw_phy_rf_power_2_rssi`.
//!
//! **Nicht portiert und benannt:** `rtw_rx_fill_rx_status` fuellt eine
//! `ieee80211_rx_status` fuer mac80211 — die gibt es hier nicht. Was daraus
//! gebraucht wird (Rate, Bandbreite, Signalstaerke, Kanal), steht in
//! `RxPktStat` und geht von dort an den Netzweg. Ebenso
//! `rtw_phy_parsing_cfo` und die Pfaddiversitaet: beide fuettern die
//! laufende Regelung, die es ohne Verbindung nicht gibt.
#![allow(dead_code)]

use crate::regs::*;
use crate::tables;

/// main.h:640-670 `struct rtw_rx_pkt_stat` — die Felder, die der
/// Empfangsweg fuellt.
#[derive(Default, Clone, Copy)]
pub struct RxPktStat {
    pub pkt_len: u16,
    pub crc_err: bool,
    pub icv_err: bool,
    pub drv_info_sz: u8,
    pub shift: u8,
    pub phy_status: bool,
    pub decrypted: bool,
    pub cam_id: u8,
    pub is_c2h: bool,
    pub ppdu_cnt: u8,
    pub rate: u8,
    pub bw: u8,
    pub tsf_low: u32,
    // aus query_phy_status
    pub channel_invalid: bool,
    pub rssi: u8,
    pub signal_power: i8,
    pub rx_power: [i8; 4],
    pub rx_snr: [i8; 4],
    /// main.h:657 — in Linux `u8`, und in `query_phy_status_page1` wird es
    /// nach `s8` genommen. Beides steht hier so.
    pub rx_evm: [u8; 4],
    pub cfo_tail: [i8; 4],
    pub freq: u16,
    pub band: u8,
    /// Nicht aus Linux: `rtw_rx_pkt_stat` fuehrt nur `freq`/`band`. Fuer
    /// den Bericht ist die Kanalnummer die Zahl, die man lesen will.
    pub channel: u8,
}

/// main.c:720-730 `rtw_set_rx_freq_band` (+ `IS_CH_*_BAND`, main.h:73-84).
fn set_rx_freq_band(s: &mut RxPktStat, channel: u8) {
    if channel <= 14 {
        s.band = NL80211_BAND_2GHZ;
    } else if is_ch_5g_band(channel) {
        s.band = NL80211_BAND_5GHZ;
    } else {
        return;
    }
    s.channel = channel;
    s.freq = channel_to_frequency(channel, s.band);
}

pub const NL80211_BAND_2GHZ: u8 = 0;
pub const NL80211_BAND_5GHZ: u8 = 1;

fn is_ch_5g_band(c: u8) -> bool {
    (36..=48).contains(&c)
        || (52..=64).contains(&c)
        || (100..=144).contains(&c)
        || (149..=177).contains(&c)
}

/// cfg80211 `ieee80211_channel_to_frequency`, in MHz.
fn channel_to_frequency(chan: u8, band: u8) -> u16 {
    match band {
        NL80211_BAND_2GHZ if chan == 14 => 2484,
        NL80211_BAND_2GHZ if chan < 14 => 2407 + chan as u16 * 5,
        NL80211_BAND_5GHZ if (182..=196).contains(&chan) => {
            4000 + chan as u16 * 5
        }
        NL80211_BAND_5GHZ => 5000 + chan as u16 * 5,
        _ => 0,
    }
}

fn le32(b: &[u8], i: usize) -> u32 {
    let o = i * 4;
    if o + 4 > b.len() {
        return 0;
    }
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn bits(v: u32, mask: u32) -> u32 {
    (v & mask) >> mask.trailing_zeros()
}

// rx.h:26-44 — die Felder des Empfangsdeskriptors.
const W0_PKT_LEN: u32 = 0x3fff; // GENMASK(13, 0)
const W0_CRC32: u32 = 1 << 14;
const W0_ICV_ERR: u32 = 1 << 15;
const W0_DRV_INFO_SIZE: u32 = 0x000f_0000; // GENMASK(19, 16)
const W0_ENC_TYPE: u32 = 0x0070_0000; // GENMASK(22, 20)
const W0_SHIFT: u32 = 0x0300_0000; // GENMASK(25, 24)
const W0_PHYST: u32 = 1 << 26;
const W0_SWDEC: u32 = 1 << 27;
const W1_MACID: u32 = 0x7f; // GENMASK(6, 0)
const W2_C2H: u32 = 1 << 28;
const W2_PPDU_CNT: u32 = 0x6000_0000; // GENMASK(30, 29)
const W3_RX_RATE: u32 = 0x7f; // GENMASK(6, 0)
const W4_BW: u32 = 0x30; // GENMASK(5, 4)
/// rx.h:9 `RX_DESC_ENC_NONE = 0`
const RX_DESC_ENC_NONE: u32 = 0;

/// rx.c:264-313 `rtw_rx_query_rx_desc`, nur der Deskriptorteil.
///
/// Reicht, um zu wissen, WIE VIEL zu lesen ist — der PHY-Status liegt
/// dahinter und wird erst in `query_rx_desc_full` ausgewertet.
pub fn query_rx_desc(d: &[u8]) -> RxPktStat {
    let w0 = le32(d, 0);
    let enc_type = bits(w0, W0_ENC_TYPE);
    let swdec = bits(w0, W0_SWDEC) != 0;

    RxPktStat {
        pkt_len: bits(w0, W0_PKT_LEN) as u16,
        crc_err: bits(w0, W0_CRC32) != 0,
        icv_err: bits(w0, W0_ICV_ERR) != 0,
        // „drv_info_sz is in unit of 8-bytes"
        drv_info_sz: (bits(w0, W0_DRV_INFO_SIZE) * 8) as u8,
        shift: bits(w0, W0_SHIFT) as u8,
        phy_status: bits(w0, W0_PHYST) != 0,
        decrypted: !swdec && enc_type != RX_DESC_ENC_NONE,
        cam_id: bits(le32(d, 1), W1_MACID) as u8,
        is_c2h: bits(le32(d, 2), W2_C2H) != 0,
        ppdu_cnt: bits(le32(d, 2), W2_PPDU_CNT) as u8,
        rate: bits(le32(d, 3), W3_RX_RATE) as u8,
        bw: bits(le32(d, 4), W4_BW) as u8,
        tsf_low: le32(d, 5),
        ..Default::default()
    }
}

/// rx.c:120-170, der Rest: PHY-Status auswerten, wenn er da ist.
///
/// `d` muss den ganzen Puffer ab dem Deskriptor enthalten.
pub fn query_rx_desc_full(d: &[u8], dm: &mut crate::dm::DmInfo,
                          path_div: &mut crate::dm::PathDiv,
                          rf_path_num: u8, current_band_width: u8)
    -> RxPktStat
{
    let mut s = query_rx_desc(d);

    // „c2h cmd pkt's rx/phy status is not interested"
    if s.is_c2h {
        return s;
    }

    let off = RX_PKT_DESC_SZ as usize + s.shift as usize;
    if s.phy_status && off < d.len() {
        query_phy_status(&d[off..], &mut s, dm, path_div, rf_path_num,
                         current_band_width);
    }
    s
}

/// rtw8822c.c:2673-2691 `query_phy_status`
fn query_phy_status(p: &[u8], s: &mut RxPktStat, dm: &mut crate::dm::DmInfo,
                    path_div: &mut crate::dm::PathDiv, rf_path_num: u8,
                    current_band_width: u8) {
    if p.is_empty() {
        return;
    }
    match p[0] & 0xf {
        0 => query_phy_status_page0(p, s, dm, rf_path_num),
        1 => query_phy_status_page1(p, s, dm, path_div, rf_path_num,
                                    current_band_width),
        // Linux: `default: rtw_warn("unused phy status page")`
        _ => {}
    }
}

// rtw8822c.h:143-154 — Seite 0. Die Zahl hinter `+` ist ein WORTindex,
// kein Byteversatz: `*((__le32 *)(phy_stat) + 0x04)`.
const P0_PWDB_A: (usize, u32) = (0x00, 0x0000_ff00); // GENMASK(15, 8)
const P0_PWDB_B: (usize, u32) = (0x04, 0x0000_00ff); // GENMASK(7, 0)
const P0_GAIN_A: (usize, u32) = (0x00, 0x003f_0000); // GENMASK(21, 16)
const P0_CHANNEL: (usize, u32) = (0x01, 0x00ff_0000); // GENMASK(23, 16)
const P0_GAIN_B: (usize, u32) = (0x04, 0x3f00_0000); // GENMASK(29, 24)

// rtw8822c.h:156-178 — Seite 1.
const P1_PWDB_A: (usize, u32) = (0x00, 0x0000_ff00); // GENMASK(15, 8)
const P1_PWDB_B: (usize, u32) = (0x00, 0x00ff_0000); // GENMASK(23, 16)
const P1_L_RXSC: (usize, u32) = (0x01, 0x0000_0f00); // GENMASK(11, 8)
const P1_HT_RXSC: (usize, u32) = (0x01, 0x0000_f000); // GENMASK(15, 12)
const P1_CHANNEL: (usize, u32) = (0x01, 0x00ff_0000); // GENMASK(23, 16)
const P1_RXEVM_A: (usize, u32) = (0x04, 0x0000_00ff); // GENMASK(7, 0)
const P1_RXEVM_B: (usize, u32) = (0x04, 0x0000_ff00); // GENMASK(15, 8)
const P1_CFO_TAIL_A: (usize, u32) = (0x05, 0x0000_00ff); // GENMASK(7, 0)
const P1_CFO_TAIL_B: (usize, u32) = (0x05, 0x0000_ff00); // GENMASK(15, 8)
const P1_RXSNR_A: (usize, u32) = (0x06, 0x0000_00ff); // GENMASK(7, 0)
const P1_RXSNR_B: (usize, u32) = (0x06, 0x0000_ff00); // GENMASK(15, 8)

fn stat(p: &[u8], f: (usize, u32)) -> u32 {
    bits(le32(p, f.0), f.1)
}

/// main.h:44-47 `DESC_RATE11M` = 3, `DESC_RATEMCS0` = 12.
const DESC_RATE11M: u8 = 0x03;
const DESC_RATEMCS0: u8 = 0x0c;

/// rtw8822c.c:2548-2596 `query_phy_status_page0` — CCK.
fn query_phy_status_page0(p: &[u8], s: &mut RxPktStat,
                          dm: &mut crate::dm::DmInfo, rf_path_num: u8) {
    let min_rx_power: i8 = -120;
    let mut rx_power = [0i8; 4];

    rx_power[RF_PATH_A] = stat(p, P0_PWDB_A) as i8;
    rx_power[RF_PATH_B] = stat(p, P0_PWDB_B) as i8;
    // Die beiden Grenzen gehoeren dem TREIBER: `rtw8822c_phy_set_param`
    // liest sie einmal aus der Hardware (Stufe 3c), hier werden sie nur
    // angewandt. Am Geraet l/u = 16/63.
    let l_bnd = dm.cck_gi_l_bnd;
    let u_bnd = dm.cck_gi_u_bnd;
    let gain_a = stat(p, P0_GAIN_A) as u8;
    let gain_b = stat(p, P0_GAIN_B) as u8;

    if gain_a < l_bnd {
        rx_power[RF_PATH_A] = rx_power[RF_PATH_A]
            .wrapping_add(((l_bnd - gain_a) << 1) as i8);
    } else if gain_a > u_bnd {
        rx_power[RF_PATH_A] = rx_power[RF_PATH_A]
            .wrapping_sub(((gain_a - u_bnd) << 1) as i8);
    }
    if gain_b < l_bnd {
        rx_power[RF_PATH_B] = rx_power[RF_PATH_B]
            .wrapping_add(((l_bnd - gain_b) << 1) as i8);
    } else if gain_b > u_bnd {
        rx_power[RF_PATH_B] = rx_power[RF_PATH_B]
            .wrapping_sub(((gain_b - u_bnd) << 1) as i8);
    }

    rx_power[RF_PATH_A] = rx_power[RF_PATH_A].wrapping_sub(110);
    rx_power[RF_PATH_B] = rx_power[RF_PATH_B].wrapping_sub(110);

    let channel = stat(p, P0_CHANNEL) as u8;
    if channel != 0 {
        set_rx_freq_band(s, channel);
    } else {
        s.channel_invalid = true;
    }

    s.rx_power[RF_PATH_A] = rx_power[RF_PATH_A];
    s.rx_power[RF_PATH_B] = rx_power[RF_PATH_B];

    // `path <= rf_path_num` steht so in Linux — bei zwei Pfaden laeuft die
    // Schleife DREIMAL und schreibt `rssi[2]`. Das Feld hat vier Plaetze,
    // also ist es folgenlos; abgeschrieben wird es trotzdem, weil eine
    // stillschweigend korrigierte Schleife kein 1:1-Port mehr ist.
    for path in 0..=rf_path_num as usize {
        dm.rssi[path] = rf_power_2_rssi(&s.rx_power[path..path + 1], 1);
    }

    s.rssi = rf_power_2_rssi(&s.rx_power, 1);
    s.bw = RTW_CHANNEL_WIDTH_20;
    s.signal_power = s.rx_power[RF_PATH_A].max(min_rx_power);
}

/// rtw8822c.c:2598-2671 `query_phy_status_page1` — OFDM/HT/VHT.
fn query_phy_status_page1(p: &[u8], s: &mut RxPktStat,
                          dm: &mut crate::dm::DmInfo,
                          path_div: &mut crate::dm::PathDiv,
                          rf_path_num: u8, current_band_width: u8) {
    let min_rx_power: i8 = -120;
    let mut evm_dbm = 0u8;

    let rxsc = if s.rate > DESC_RATE11M && s.rate < DESC_RATEMCS0 {
        stat(p, P1_L_RXSC) as u8
    } else {
        stat(p, P1_HT_RXSC) as u8
    };

    let bw = if rxsc == 0 {
        current_band_width
    } else if (1..=8).contains(&rxsc) {
        RTW_CHANNEL_WIDTH_20
    } else if (9..=12).contains(&rxsc) {
        RTW_CHANNEL_WIDTH_40
    } else {
        RTW_CHANNEL_WIDTH_80
    };

    // Ohne `if channel != 0` — Linux prueft das auf Seite 1 nicht, und
    // `set_rx_freq_band` kehrt bei einer Zahl ausserhalb beider Baender
    // von selbst um.
    let channel = stat(p, P1_CHANNEL) as u8;
    set_rx_freq_band(s, channel);

    s.rx_power[RF_PATH_A] = (stat(p, P1_PWDB_A) as i8).wrapping_sub(110);
    s.rx_power[RF_PATH_B] = (stat(p, P1_PWDB_B) as i8).wrapping_sub(110);
    s.rssi = rf_power_2_rssi(&s.rx_power, 2);
    s.bw = bw;
    s.signal_power = s.rx_power[RF_PATH_A]
        .max(s.rx_power[RF_PATH_B])
        .max(min_rx_power);

    dm.curr_rx_rate = s.rate;

    s.rx_evm[RF_PATH_A] = stat(p, P1_RXEVM_A) as u8;
    s.rx_evm[RF_PATH_B] = stat(p, P1_RXEVM_B) as u8;

    s.rx_snr[RF_PATH_A] = stat(p, P1_RXSNR_A) as i8;
    s.rx_snr[RF_PATH_B] = stat(p, P1_RXSNR_B) as i8;

    s.cfo_tail[RF_PATH_A] = stat(p, P1_CFO_TAIL_A) as i8;
    s.cfo_tail[RF_PATH_B] = stat(p, P1_CFO_TAIL_B) as i8;

    // Dieselbe `<=`-Schleife wie auf Seite 0.
    for path in 0..=rf_path_num as usize {
        let rssi = rf_power_2_rssi(&s.rx_power[path..path + 1], 1);
        dm.rssi[path] = rssi;
        if path == RF_PATH_A {
            path_div.path_a_sum += rssi as u32;
            path_div.path_a_cnt += 1;
        } else if path == RF_PATH_B {
            path_div.path_b_sum += rssi as u32;
            path_div.path_b_cnt += 1;
        }
        dm.rx_snr[path] = s.rx_snr[path] >> 1;
        dm.cfo_tail[path] = (s.cfo_tail[path] as i16 * 5) >> 1;

        let rx_evm = s.rx_evm[path] as i8;
        if rx_evm < 0 {
            evm_dbm = if rx_evm == i8::MIN {
                0
            } else {
                (rx_evm.unsigned_abs()) >> 1
            };
        }
        dm.rx_evm_dbm[path] = evm_dbm;
    }
    // `rtw_phy_parsing_cfo` braucht die angemeldeten Schnittstellen einer
    // Verbindung (`rtw_iterate_vifs_atomic`). Ohne Verbindung gibt es
    // keine, der Aufruf waere in Linux hier ein Leerlauf.
}

const RF_PATH_A: usize = 0;
const RF_PATH_B: usize = 1;
const RTW_CHANNEL_WIDTH_20: u8 = 0;
const RTW_CHANNEL_WIDTH_40: u8 = 1;
const RTW_CHANNEL_WIDTH_80: u8 = 2;

// ── Die dB-Umrechnung (phy.c:128-235) ────────────────────────────

/// phy.c `FRAC_BITS`
const FRAC_BITS: u32 = 3;

/// phy.c:826-834 `rtw_phy_power_2_db`
fn power_2_db(power: i8) -> u8 {
    if power <= -100 || power >= 20 {
        0
    } else if power >= 0 {
        100
    } else {
        (100 + power as i16) as u8
    }
}

/// phy.c:836-854 `rtw_phy_db_2_linear`
fn db_2_linear(power_db: u8) -> u64 {
    let power_db = if power_db > 96 { 96 } else { power_db };
    if power_db < 1 {
        return 1;
    }
    let i = ((power_db - 1) >> 3) as usize;
    let j = ((power_db - 1) - ((i as u8) << 3)) as usize;
    let linear = tables::DB_INVERT_TABLE[i][j] as u64;
    if i > 2 { linear << FRAC_BITS } else { linear }
}

/// phy.c:856-901 `rtw_phy_linear_2_db`
fn linear_2_db(linear: u64) -> u8 {
    let t = &tables::DB_INVERT_TABLE;
    let (mut i, mut j) = (0usize, 0usize);
    let mut found = false;
    'outer: for a in 0..12usize {
        for b in 0..8usize {
            let hit = if a <= 2 {
                (linear << FRAC_BITS) <= t[a][b] as u64
            } else {
                linear <= t[a][b] as u64
            };
            if hit {
                i = a;
                j = b;
                found = true;
                break 'outer;
            }
        }
    }
    if !found {
        return 96; // maximum 96 dB
    }
    if !(j == 0 && i == 0) {
        if j == 0 {
            if i != 3 {
                if t[i][0] as u64 - linear > linear - t[i - 1][7] as u64 {
                    i -= 1;
                    j = 7;
                }
            } else if t[3][0] as u64 - linear > linear - t[2][7] as u64 {
                i = 2;
                j = 7;
            }
        } else if t[i][j] as u64 - linear > linear - t[i][j - 1] as u64 {
            j -= 1;
        }
    }
    ((i << 3) + j + 1) as u8
}

/// phy.c:903-934 `rtw_phy_rf_power_2_rssi`
pub fn rf_power_2_rssi(rf_power: &[i8], path_num: u8) -> u8 {
    let mut sum: u64 = 0;
    for path in 0..path_num as usize {
        if path >= rf_power.len() {
            break;
        }
        sum += db_2_linear(power_2_db(rf_power[path]));
    }
    sum = (sum + (1 << (FRAC_BITS - 1))) >> FRAC_BITS;
    match path_num {
        2 => sum >>= 1,
        3 => sum = (sum + (sum << 1) + (sum << 3)) >> 5,
        4 => sum >>= 2,
        _ => {}
    }
    linear_2_db(sum)
}

/// rx.h:56-62 `rtw_update_rx_freq_for_invalid` +
/// rx.c:155-193 `rtw_update_rx_freq_from_ie`.
///
/// Ein CCK-Rahmen kann mit Kanal 0 kommen (`query_phy_status_page0` setzt
/// dann `channel_invalid`). Linux nimmt dann den LAUFENDEN Kanal — und
/// liest die Kanalnummer nur dann aus dem Beacon, wenn gerade GESUCHT
/// wird, weil nur beim Suchen ein Rahmen von einem anderen Kanal
/// hereinkommen kann. Ohne Suche ist der Zweig von `RTW_FLAG_SCANNING`
/// tot, und deshalb steht hier nur seine Wirkung.
pub fn update_rx_freq_for_invalid(s: &mut RxPktStat, current_channel: u8,
                                  scanning: bool) {
    if !s.channel_invalid {
        return;
    }
    if scanning {
        // `cfg80211_get_ies_channel_number` auf dem DS-Parameter-Set des
        // Beacons. BENANNT UND NICHT GEBAUT: es gibt noch keine Suche
        // (Stufe 5c). Bis dahin waere ein Parser fuer Informationselemente
        // Code ohne Rufer.
    }
    set_rx_freq_band(s, current_channel);
}

// ═══════════════════════════════════════════════════════════════════
// Was der Empfangsweg dem Watchdog zutraegt (rx.c:42-133, phy.c:678-704)
//
// Ohne diese vier Zeilen rechnet `rtw_watch_dog_work` auf Nullen: der
// Frequenzversatz wird NICHT aufsummiert, die Ratenzaehler bleiben leer,
// und `min_rssi` ist 255. Die Nachfuehrung tut dann nichts und meldet
// auch nichts — der schlimmste Zustand von allen.
// ═══════════════════════════════════════════════════════════════════

/// Was EIN Ringdurchlauf dem Watchdog zutraegt.
///
/// **Im Rueckruf gesammelt, danach eingetragen.** Waehrend `rx_poll`
/// laeuft, haelt es `DmInfo` selbst (es schreibt den PHY-Status hinein);
/// zwei Schreiber auf denselben Zustand gibt es nicht, und ein roher
/// Zeiger waere hier eine Umgehung des Ausleihers statt einer Loesung.
#[derive(Clone, Copy)]
pub struct WdAcc {
    pub cfo_tail: [i32; 4],
    pub cfo_cnt: [i32; 4],
    pub packet_count: u32,
    pub num_bcn_pkt: u16,
    pub num_qry_pkt: [u16; DESC_RATE_MAX],
    pub curr_rx_rate: u8,
    pub avg_rssi: crate::dm::Ewma,
    /// Der Ratenbericht der Firmware: `(rate, mac_id)`.
    pub ra_rpt: Option<(u8, u8)>,
    /// Die Sendequittungen dieses Durchlaufs: `(Folgenummer, quittiert)`.
    pub tx_rpt: [(u8, bool); 8],
    pub n_tx_rpt: usize,
    /// Die Kennungen der C2H, die wir NICHT behandeln — gezaehlt statt
    /// verworfen.
    pub c2h_seen: [u8; 8],
    pub n_c2h_seen: usize,
    /// Verwaltungsrahmen dieses Durchlaufs: `(Subtyp, (Kategorie,
    /// Aktion))`, `0xff` wo es keine Aktion gibt.
    pub mgmt: [(u8, (u8, u8)); 8],
    pub n_mgmt: usize,
    /// Die ADDBA Requests dieses Durchlaufs — beantwortet werden sie
    /// draussen, mit freiem `trx`.
    ///
    /// **Es war EINER, und das war zu wenig.** Am Geraet stand
    /// `ADDBA 14 erbeten, 8 angenommen`: ein Ringdurchlauf bringt
    /// mehrere Rahmen auf einmal, und alles nach dem ersten fiel weg.
    /// Der AP wiederholt zwar, aber jede Wiederholung ist eine
    /// Sendegelegenheit, in der er NICHT aggregiert — und bis zur
    /// Antwort bleibt seine Sitzung zu.
    pub addba: [crate::sta::AddbaReq; 4],
    pub n_addba: usize,
    /// **Haben wir in diesem Durchlauf ueberhaupt etwas vom AP
    /// gehoert?** mlme.c:131-145 `ieee80211_sta_reset_conn_monitor`:
    /// jeder Rahmen von ihm setzt die Wache zurueck, nicht nur eine
    /// Bake. Ein Download ohne Baken ist eine lebende Verbindung.
    pub heard_ap: bool,
    /// Der Pegel der letzten Bake DIESER Zelle, in dBm. Er fuettert den
    /// geglaetteten Wert, an dem das Roaming haengt
    /// (`ieee80211_handle_beacon_sig`).
    pub beacon_dbm: Option<i8>,
    /// Kam in diesem Durchlauf eine Bake DIESER Zelle OHNE Ansage?
    /// **Das bricht einen angekuendigten Wechsel ab** (mlme.c:2822).
    pub beacon_ohne_csa: bool,
    /// Die Wechselansage aus einer Bake dieser Zelle, wenn eine da war.
    /// Sie faehrt heraus, weil der Kanalwechsel `trx` braucht und der
    /// Rueckruf es nicht halten darf.
    pub csa: Option<crate::Csa>,
    /// Und was auch in vier Plaetze nicht passte. Eine Zahl, damit ein
    /// zu kleiner Puffer nicht wieder still kuerzt.
    pub addba_drop: u32,
    /// Und die Antwort auf UNSERE Frage. Sie faehrt denselben Weg, aus
    /// demselben Grund: der Zustandswechsel gehoert nach dem Ringleeren
    /// hin, wo `link` veraenderlich ist.
    pub addba_resp: Option<crate::sta::AddbaResp>,
    /// rx.c:14-32 `rtw_rx_stats` — Bytes und Rahmen, nur Unicast.
    pub rx_unicast: u64,
    pub rx_cnt: u64,
    /// Die BREITE, in der die Rahmen dieses Durchlaufs hereinkamen:
    /// 20/40/80 und ein vierter Platz fuer alles andere. Sie kommt aus
    /// dem Empfangsstatus des Chips, ist also eine Messung und keine
    /// Einstellung.
    pub bw_cnt: [u32; 4],
}

impl WdAcc {
    pub fn new(avg_rssi: crate::dm::Ewma) -> Self {
        WdAcc {
            cfo_tail: [0; 4], cfo_cnt: [0; 4], packet_count: 0,
            num_bcn_pkt: 0, num_qry_pkt: [0; DESC_RATE_MAX],
            curr_rx_rate: 0, avg_rssi, ra_rpt: None,
            tx_rpt: [(0, false); 8], n_tx_rpt: 0,
            c2h_seen: [0; 8], n_c2h_seen: 0,
            mgmt: [(0, (0, 0)); 8], n_mgmt: 0,
            addba: [crate::sta::AddbaReq::default(); 4], n_addba: 0,
            addba_drop: 0, heard_ap: false, beacon_dbm: None,
            beacon_ohne_csa: false, csa: None,
            addba_resp: None,
            rx_unicast: 0, rx_cnt: 0, bw_cnt: [0; 4],
        }
    }

    /// Nach dem Ringdurchlauf in den langlebigen Zustand eintragen.
    pub fn merge(&self, dm: &mut crate::dm::DmInfo,
                 si: &mut crate::sta::StaInfo) {
        for i in 0..4 {
            dm.cfo_track.cfo_tail[i] += self.cfo_tail[i];
            dm.cfo_track.cfo_cnt[i] += self.cfo_cnt[i];
        }
        dm.cfo_track.packet_count =
            dm.cfo_track.packet_count.wrapping_add(self.packet_count);
        dm.cur_pkt_count.num_bcn_pkt =
            dm.cur_pkt_count.num_bcn_pkt.saturating_add(self.num_bcn_pkt);
        for i in 0..DESC_RATE_MAX {
            dm.cur_pkt_count.num_qry_pkt[i] =
                dm.cur_pkt_count.num_qry_pkt[i]
                    .saturating_add(self.num_qry_pkt[i]);
        }
        if self.packet_count > 0 {
            dm.curr_rx_rate = self.curr_rx_rate;
        }
        si.avg_rssi = self.avg_rssi;
    }
}

/// util.h:28-41 `get_hdr_bssid` — welche der drei Adressen die BSSID ist,
/// haengt an den zwei DS-Bits.
pub fn hdr_bssid(f: &[u8]) -> Option<[u8; 6]> {
    if f.len() < 22 {
        return None;
    }
    let tods = f[1] & 0x01 != 0;
    let fromds = f[1] & 0x02 != 0;
    let at = if tods { 4 } else if fromds { 10 } else { 16 };
    let mut b = [0u8; 6];
    b.copy_from_slice(&f[at..at + 6]);
    Some(b)
}

/// `ieee80211_is_ctl` — Typ 01 im ersten Byte.
fn is_ctl(f: &[u8]) -> bool {
    f[0] & 0x0c == 0x04
}

/// `ieee80211_is_beacon` — Verwaltung, Subtyp 8.
fn is_beacon(f: &[u8]) -> bool {
    f[0] == 0x80
}

/// rx.c:100-133 `rtw_rx_addr_match` + `_iter`, und phy.c:690-704 in
/// EINEM Gang: beide laufen in Linux ueber dieselbe Adressprobe, nur aus
/// zwei Rufstellen.
///
/// `our_mac`/`bssid` ersetzen den vif-Iterator — wir fahren genau eine
/// Schnittstelle, und mehr als eine waere hier eine Erfindung.
pub fn watchdog_feed(a: &mut WdAcc, st: &RxPktStat, f: &[u8],
                     our_mac: &[u8; 6], bssid: &[u8; 6], path_num: u8) {
    if f.len() < 24 || st.crc_err || st.icv_err || !st.phy_status || is_ctl(f) {
        return;
    }
    let Some(from) = hdr_bssid(f) else { return };
    if &from != bssid {
        return;
    }

    // phy.c: der CFO-Zweig prueft NUR die BSSID.
    for i in 0..path_num as usize {
        a.cfo_tail[i] += st.cfo_tail[i] as i32;
        a.cfo_cnt[i] += 1;
    }
    a.packet_count = a.packet_count.wrapping_add(1);

    // rx.c: der Statistikzweig verlangt zusaetzlich, dass der Rahmen an
    // UNS gerichtet ist — oder ein Beacon.
    if &f[4..10] != our_mac && !is_beacon(f) {
        return;
    }
    // rx.c:42-99 `rtw_rx_phy_stat`
    a.curr_rx_rate = st.rate;
    if is_beacon(f) {
        a.num_bcn_pkt = a.num_bcn_pkt.saturating_add(1);
    }
    if (st.rate as usize) < DESC_RATE_MAX {
        let c = &mut a.num_qry_pkt[st.rate as usize];
        *c = c.saturating_add(1);
    }

    // `ewma_rssi_add(&si->avg_rssi, pkt_stat->rssi)` — nur, wenn der
    // Sender die bekannte Station ist.
    if f[10..16] == bssid[..] {
        a.avg_rssi.add(st.rssi as u32, crate::dm::EWMA_RSSI_PRECISION,
                       crate::dm::EWMA_RSSI_WEIGHT_RCP);
    }
}

/// **Der Zensus der Verwaltungsrahmen: zaehlen, was wir verwerfen.**
///
/// Dieselbe Regel, die der C2H-Zensus gerade bewiesen hat. Die Frage
/// dahinter ist konkret: **versucht der AP ueberhaupt, eine Aggregation
/// aufzubauen?** Er tut das mit einem Action-Rahmen (Kategorie 3,
/// Aktion 0 = ADDBA Request), und wir verwerfen bis heute jeden
/// Verwaltungsrahmen ausser Deauth und Disassoc. Kommt keiner, ist der
/// Durchsatzdeckel woanders; kommt einer, ist die Antwort darauf der
/// naechste Posten.
///
/// Gibt den Subtyp zurueck (0..15), und bei einem Action-Rahmen
/// zusaetzlich `(Kategorie, Aktion)`.
pub fn mgmt_census(f: &[u8], bssid: &[u8; 6]) -> Option<(u8, Option<(u8, u8)>)> {
    if f.len() < 24 || f[0] & DOT11_FC_TYPE_MASK != DOT11_FC_TYPE_MGMT {
        return None;
    }
    if f[10..16] != bssid[..] {
        return None;
    }
    let subtype = f[0] >> 4;
    // Action = Subtyp 13; Kategorie und Aktion stehen gleich hinter dem
    // 24 Byte langen Kopf.
    let act = if subtype == 13 && f.len() >= 26 {
        Some((f[24], f[25]))
    } else {
        None
    };
    Some((subtype, act))
}
