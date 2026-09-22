//! `rtw_sta_info` und `rtw_update_sta_info` — die Ratenanpassung
//! (Stufe 5f), main.c:1117-1240.
//!
//! Der Treiber schickt der Firmware KEINE einzelne Rate, sondern eine
//! MASKE: welche der 64 Raten dieses Gegenueber ueberhaupt kann. Die
//! Firmware waehlt daraus laufend und meldet ihre Wahl als C2H zurueck.
//!
//! **Woher die Maske kommt, ist der ganze Punkt.** In Linux steht sie in
//! `ieee80211_sta`, das mac80211 aus der Anmeldeantwort baut. Bei uns gibt
//! es kein mac80211, also wird die Antwort hier selbst gelesen: HT- und
//! VHT-Element, unterstuetzte Raten. Was dabei NICHT herauskommt, ist
//! geraten — und eine geratene Ratenmaske sendet zu schnell und faellt bei
//! jedem Paket aus.
#![allow(dead_code)]

use crate::regs::*;

/// main.h:775-799 `struct rtw_sta_info` — die Felder, die `send_ra_info`
/// liest. Der Rest ist Linux' Buchfuehrung (Arbeitsschlangen, Mittelwerte).
#[derive(Default, Clone, Copy)]
pub struct StaInfo {
    pub mac_id: u8,
    pub rate_id: u8,
    pub bw_mode: u8,
    pub stbc_en: u8,
    pub ldpc_en: u8,
    pub sgi_enable: bool,
    pub vht_enable: bool,
    pub init_ra_lv: u8,
    pub ra_mask: u64,
    pub rssi_level: u8,
    /// main.h:760 `DECLARE_EWMA(rssi, 10, 16)` — von `rtw_rx_addr_match`
    /// je Rahmen gefuettert, vom Watchdog alle zwei Sekunden gelesen.
    pub avg_rssi: crate::dm::Ewma,
    /// main.h `si->ra_report.desc_rate` — die Rate, die die FIRMWARE
    /// zuletzt gewaehlt hat. Sie kommt als C2H `RA_RPT` herein und ist
    /// die Eingabe von `rtw_phy_rrsr_update`.
    pub ra_report_desc_rate: u8,
}

/// Was aus der Anmeldeantwort des AP herausfaellt — bei Linux
/// `ieee80211_sta`, von mac80211 gefuellt.
#[derive(Default, Clone, Copy)]
pub struct PeerCaps {
    pub ht_supported: bool,
    pub ht_cap: u16,
    /// `ht_cap.mcs.rx_mask[0..4]`
    pub ht_mcs: [u8; 4],
    /// Byte 2 des HT-CAPABILITIES-Elements, zerlegt: der
    /// Laengen-Exponent (Bit 1:0) und der Mindestabstand (Bit 4:2).
    pub ht_ampdu_factor: u8,
    pub ht_ampdu_density: u8,
    pub vht_supported: bool,
    pub vht_cap: u32,
    /// `vht_cap.vht_mcs.rx_mcs_map` — was das Gegenueber EMPFANGEN kann.
    /// Daraus baut `get_vht_ra_mask` die Sendemaske (main.c:1011).
    pub vht_mcs_map: u16,
    /// `vht_cap.vht_mcs.tx_mcs_map` — was es SENDEN kann. Eine andere
    /// Karte und eine andere Frage: `get_highest_vht_tx_rate` liest diese
    /// (tx.c:132), und ein AP darf sich hier anders eintragen.
    pub vht_tx_mcs_map: u16,
    /// Bitmaske der Grundraten, wie `supp_rates[NL80211_BAND_2GHZ]`:
    /// Bit 0..3 = CCK 1/2/5,5/11, Bit 4..11 = OFDM 6..54.
    pub supp_rates: u16,
    /// 0 = 20 MHz, 1 = 40, 2 = 80 (`ieee80211_sta.bandwidth`)
    pub bandwidth: u8,
}

/// Die Elemente einer Anmeldeantwort lesen.
///
/// **Das ist die obere Haelfte** — in Linux baut mac80211 daraus
/// `ieee80211_sta`. Es steht hier, weil `rtw_update_sta_info` ohne diese
/// Zahlen nichts rechnen kann; `wifid` loest es spaeter ab.
pub fn parse_assoc_resp(f: &[u8]) -> PeerCaps {
    let mut c = PeerCaps::default();
    // 24 Kopf + Faehigkeiten(2) + Status(2) + AID(2) = 30, dann Elemente.
    let mut i = 30usize;
    while i + 2 <= f.len() {
        let id = f[i] as u32;
        let len = f[i + 1] as usize;
        if i + 2 + len > f.len() {
            break;
        }
        let b = &f[i + 2..i + 2 + len];
        if id == WLAN_EID_SUPP_RATES || id == WLAN_EID_EXT_SUPP_RATES {
            for &r in b {
                // Das hohe Bit markiert eine GRUNDrate und gehoert nicht
                // zum Wert.
                c.supp_rates |= rate_bit(r & 0x7f);
            }
        } else if id == WLAN_EID_HT_CAPABILITY && len >= 26 {
            c.ht_supported = true;
            c.ht_cap = u16::from_le_bytes([b[0], b[1]]);
            // Byte 2 sind die A-MPDU-Parameter: Bit 1:0 der
            // Laengen-Exponent, Bit 4:2 der Mindestabstand. Sie sagen,
            // wieviel der AP am Stueck EMPFANGEN kann — also genau die
            // zwei Zahlen, die im Sendedeskriptor stehen muessen.
            //
            // **Zerlegt wird hier und nicht beim Gebrauch**, weil das in
            // Linux auch hier geschieht: `ieee80211_ht_cap_ie_to_sta_ht_cap`
            // (net/mac80211/ht.c) legt `ampdu_factor` und `ampdu_density`
            // getrennt ab, und `get_tx_ampdu_factor` in `tx.c` bekommt sie
            // fertig. Wer die Maske in die Treiberfunktion zieht, hat sie
            // eine Schicht zu tief.
            c.ht_ampdu_factor = b[2] & 0x03; // IEEE80211_HT_AMPDU_PARM_FACTOR
            c.ht_ampdu_density = (b[2] & 0x1c) >> 2; // ..._PARM_DENSITY
            // `ht_cap.mcs` beginnt bei Versatz 3 (nach cap und ampdu).
            c.ht_mcs.copy_from_slice(&b[3..7]);
        } else if id == WLAN_EID_VHT_CAPABILITY && len >= 12 {
            c.vht_supported = true;
            c.vht_cap = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            c.vht_mcs_map = u16::from_le_bytes([b[4], b[5]]);
            // Versatz 8:9 — hinter rx_mcs_map(4:5) und rx_highest(6:7).
            c.vht_tx_mcs_map = u16::from_le_bytes([b[8], b[9]]);
        }
        i += 2 + len;
    }

    // `ieee80211_sta.bandwidth` — mac80211 rechnet sie aus den
    // Faehigkeiten UND der Kanalbreite der Zelle. Ohne die Zelle bleibt
    // das, was das Gegenueber kann.
    c.bandwidth = if c.vht_supported {
        2
    } else if c.ht_supported && c.ht_cap & IEEE80211_HT_CAP_SUP_WIDTH_20_40 as u16 != 0 {
        1
    } else {
        0
    };
    c
}

/// Eine Rate in halben Mbit/s auf ihr Bit in `supp_rates` abbilden.
/// Die Reihenfolge ist die von `ieee80211_rate` im 2,4-GHz-Band.
fn rate_bit(half_mbps: u8) -> u16 {
    match half_mbps {
        2 => 1 << 0,    // 1 Mbit
        4 => 1 << 1,    // 2
        11 => 1 << 2,   // 5,5
        22 => 1 << 3,   // 11
        12 => 1 << 4,   // 6
        18 => 1 << 5,   // 9
        24 => 1 << 6,   // 12
        36 => 1 << 7,   // 18
        48 => 1 << 8,   // 24
        72 => 1 << 9,   // 36
        96 => 1 << 10,  // 48
        108 => 1 << 11, // 54
        _ => 0,
    }
}

/// main.c:1139-1163 `get_vht_ra_mask`
fn get_vht_ra_mask(mut mcs_map: u16) -> u64 {
    let mut ra_mask = 0u64;
    let mut nss = 12u32;
    for _ in 0..4 {
        match mcs_map & 0x3 {
            2 => ra_mask |= 0x3ffu64 << nss, // MCS9
            1 => ra_mask |= 0x1ffu64 << nss, // MCS8
            0 => ra_mask |= 0x0ffu64 << nss, // MCS7
            _ => {}
        }
        mcs_map >>= 2;
        nss += 10;
    }
    ra_mask
}

/// main.c:1165-1183 `rtw_rate_mask_rssi`
fn rate_mask_rssi(rssi_level: u8, wireless_set: u32) -> u64 {
    if wireless_set == WIRELESS_CCK {
        return 0xffff_ffff_ffff_ffff;
    }
    match rssi_level {
        0 => 0xffff_ffff_ffff_ffff,
        1 => 0xffff_ffff_ffff_fff0,
        2 => 0xffff_ffff_ffff_efe0,
        3 => 0xffff_ffff_ffff_cfc0,
        4 => 0xffff_ffff_ffff_8f80,
        _ => 0xffff_ffff_ffff_0f00,
    }
}

/// main.c:1185-1194 `rtw_rate_mask_recover`
fn rate_mask_recover(mut ra_mask: u64, bak: u64) -> u64 {
    let legacy = RA_MASK_CCK_RATES as u64 | RA_MASK_OFDM_RATES as u64;
    if ra_mask & !legacy == 0 {
        ra_mask |= bak & !legacy;
    }
    if ra_mask == 0 {
        ra_mask |= bak & legacy;
    }
    ra_mask
}

/// main.c:1240-1310 `get_rate_id`
fn get_rate_id(wireless_set: u32, bw_mode: u8, tx_num: u8) -> u8 {
    let cck = WIRELESS_CCK;
    let ofdm = WIRELESS_OFDM;
    let ht = WIRELESS_HT;
    let vht = WIRELESS_VHT;
    if wireless_set == cck {
        crate::tx::RTW_RATEID_B_20M
    } else if wireless_set == ofdm {
        crate::tx::RTW_RATEID_G
    } else if wireless_set == cck | ofdm {
        RTW_RATEID_BG as u8
    } else if wireless_set == ofdm | ht {
        match tx_num {
            1 => RTW_RATEID_GN_N1SS as u8,
            2 => RTW_RATEID_GN_N2SS as u8,
            _ => 0,
        }
    } else if wireless_set == cck | ofdm | ht {
        if bw_mode == 1 {
            match tx_num {
                1 => RTW_RATEID_BGN_40M_1SS as u8,
                2 => RTW_RATEID_BGN_40M_2SS as u8,
                _ => 0,
            }
        } else {
            match tx_num {
                1 => RTW_RATEID_BGN_20M_1SS as u8,
                2 => RTW_RATEID_BGN_20M_2SS as u8,
                _ => 0,
            }
        }
    } else if wireless_set == ofdm | vht {
        match tx_num {
            1 => RTW_RATEID_ARFR1_AC_1SS as u8,
            2 => RTW_RATEID_ARFR0_AC_2SS as u8,
            _ => 0,
        }
    } else if wireless_set == cck | ofdm | vht {
        if bw_mode >= 2 {
            match tx_num {
                1 => RTW_RATEID_ARFR1_AC_1SS as u8,
                2 => RTW_RATEID_ARFR0_AC_2SS as u8,
                _ => 0,
            }
        } else {
            match tx_num {
                1 => RTW_RATEID_ARFR2_AC_2G_1SS as u8,
                2 => RTW_RATEID_ARFR3_AC_2G_2SS as u8,
                _ => 0,
            }
        }
    } else {
        0
    }
    // Die 3SS- und 4SS-Zweige stehen nicht da: `hw_cap.nss` ist auf
    // diesem Chip hoechstens 2, und `tx_num` kommt allein daher.
}

/// main.c:1312-1430 `rtw_update_sta_info`, Band 2,4 GHz.
///
/// `rtw_rate_mask_cfg` faellt weg: es greift nur bei `use_cfg_mask`, und
/// das setzt in Linux ein Nutzerbefehl (`cfg80211_bitrate_mask`), den es
/// hier nicht gibt — die Funktion kehrt dann unveraendert um.
pub fn update_sta_info(si: &mut StaInfo, c: &PeerCaps, nss: u8,
                       band_2g: bool) -> u32 {
    let mut ra_mask = 0u64;
    let mut stbc_en = 0u8;
    let mut ldpc_en = 0u8;
    let mut tx_num = 1u8;
    let mut is_vht_enable = false;

    if c.vht_supported {
        is_vht_enable = true;
        ra_mask |= get_vht_ra_mask(c.vht_mcs_map);
        if c.vht_cap & IEEE80211_VHT_CAP_RXSTBC_MASK != 0 {
            stbc_en = VHT_STBC_EN;
        }
        if c.vht_cap & IEEE80211_VHT_CAP_RXLDPC != 0 {
            ldpc_en = VHT_LDPC_EN;
        }
    } else if c.ht_supported {
        ra_mask |= ((c.ht_mcs[3] as u64) << 36)
            | ((c.ht_mcs[2] as u64) << 28)
            | ((c.ht_mcs[1] as u64) << 20)
            | ((c.ht_mcs[0] as u64) << 12);
        if c.ht_cap & IEEE80211_HT_CAP_RX_STBC as u16 != 0 {
            stbc_en = HT_STBC_EN;
        }
        if c.ht_cap & IEEE80211_HT_CAP_LDPC_CODING as u16 != 0 {
            ldpc_en = HT_LDPC_EN;
        }
    }

    if nss == 1 {
        ra_mask &= RA_MASK_VHT_RATES_1SS as u64 | RA_MASK_HT_RATES_1SS as u64;
    } else if nss == 2 {
        ra_mask &= RA_MASK_VHT_RATES_2SS as u64 | RA_MASK_HT_RATES_2SS as u64
            | RA_MASK_VHT_RATES_1SS as u64 | RA_MASK_HT_RATES_1SS as u64;
    }

    let wireless_set;
    let ra_mask_bak;
    if band_2g {
        ra_mask |= c.supp_rates as u64;
        ra_mask_bak = ra_mask;
        if c.vht_supported {
            ra_mask &= RA_MASK_VHT_RATES as u64 | RA_MASK_CCK_IN_VHT as u64
                | RA_MASK_OFDM_IN_VHT as u64;
            wireless_set = WIRELESS_CCK | WIRELESS_OFDM | WIRELESS_HT
                | WIRELESS_VHT;
        } else if c.ht_supported {
            ra_mask &= RA_MASK_HT_RATES as u64 | RA_MASK_CCK_IN_HT as u64
                | RA_MASK_OFDM_IN_HT_2G as u64;
            wireless_set = WIRELESS_CCK | WIRELESS_OFDM | WIRELESS_HT;
        } else if c.supp_rates <= 0xf {
            wireless_set = WIRELESS_CCK;
        } else {
            ra_mask &= RA_MASK_OFDM_RATES as u64 | RA_MASK_CCK_IN_BG as u64;
            wireless_set = WIRELESS_CCK | WIRELESS_OFDM;
        }
    } else {
        ra_mask |= (c.supp_rates as u64) << 4;
        ra_mask_bak = ra_mask;
        if c.vht_supported {
            ra_mask &= RA_MASK_VHT_RATES as u64 | RA_MASK_OFDM_IN_VHT as u64;
            wireless_set = WIRELESS_OFDM | WIRELESS_VHT;
        } else if c.ht_supported {
            ra_mask &= RA_MASK_HT_RATES as u64 | RA_MASK_OFDM_IN_HT_5G as u64;
            wireless_set = WIRELESS_OFDM | WIRELESS_HT;
        } else {
            wireless_set = WIRELESS_OFDM;
        }
    }

    let (bw_mode, is_support_sgi) = match c.bandwidth {
        2 => (2u8, c.vht_supported
              && c.vht_cap & IEEE80211_VHT_CAP_SHORT_GI_80 != 0),
        1 => (1u8, c.ht_supported
              && c.ht_cap & IEEE80211_HT_CAP_SGI_40 as u16 != 0),
        _ => (0u8, c.ht_supported
              && c.ht_cap & IEEE80211_HT_CAP_SGI_20 as u16 != 0),
    };

    if c.vht_supported || c.ht_supported {
        tx_num = nss;
    }

    let rate_id = get_rate_id(wireless_set, bw_mode, tx_num);

    ra_mask &= rate_mask_rssi(si.rssi_level, wireless_set);
    ra_mask = rate_mask_recover(ra_mask, ra_mask_bak);

    si.bw_mode = bw_mode;
    si.stbc_en = stbc_en;
    si.ldpc_en = ldpc_en;
    si.sgi_enable = is_support_sgi;
    si.vht_enable = is_vht_enable;
    si.ra_mask = ra_mask;
    si.rate_id = rate_id;
    wireless_set
}

// ════════════════════════════════════════════════════════════════
// Was WIR koennen — und damit auch anbieten muessen
// ════════════════════════════════════════════════════════════════

/// main.c:1580-1600 `rtw_init_ht_cap`, als fertiges HT-Element
/// (802.11 §9.4.2.55: id 45, 26 Byte Rumpf).
///
/// **Ohne dieses Element im Anmeldeantrag nimmt der AP uns als
/// LEGACY-Station an** — und laesst HT dann auch in seiner Antwort weg.
/// Genau das ist in 0.19.0 passiert: `ra_mask 0x0ff5`, keine MCS-Bits, und
/// die Firmware waehlte OFDM 54M als Bestes, das sie DURFTE.
///
/// `rx_ldpc` und `tx_stbc` sind beim 8822C beide `true`
/// (rtw8822c.c:5391-5392).
pub fn build_ht_cap_ie(out: &mut [u8], hw_cap_bw: u8, nss: u8) -> usize {
    let mut cap = IEEE80211_HT_CAP_SGI_20
        | IEEE80211_HT_CAP_MAX_AMSDU
        | (1 << IEEE80211_HT_CAP_RX_STBC_SHIFT);
    cap |= IEEE80211_HT_CAP_LDPC_CODING; // rx_ldpc
    cap |= IEEE80211_HT_CAP_TX_STBC; // tx_stbc
    // **SM Power Save: AUS — und das ist die Zutat, die rtw88 nicht hat.**
    //
    // `rtw_init_ht_cap` setzt diese zwei Bits NIE, und das ist dort richtig:
    // in Linux baut MAC80211 das Element und traegt sie beim Anmeldeantrag
    // nach (`mlme.c:1425-1444`, `cap &= ~IEEE80211_HT_CAP_SM_PS` gefolgt vom
    // `switch (smps)`; eine gewoehnliche Station steht auf
    // `IEEE80211_SMPS_OFF` und bekommt `WLAN_HT_CAP_SM_PS_DISABLED`).
    //
    // Wir haben kein mac80211. Ungesetzt sind die Bits **null**, und null
    // ist nicht „egal", sondern `WLAN_HT_CAP_SM_PS_STATIC`: *ich halte nur
    // EINE Empfangskette aktiv*. Ein AP, der sich daran haelt — und sie
    // halten sich daran —, schickt ab da nur noch EINEN raeumlichen Strom,
    // also hoechstens MCS7.
    //
    // Am Geraet gemessen (2026-09-21, IvyPie_New auf K7, -40 dBm): wir
    // boten `nss 2` an, der AP meldete `MCS ff:ff:00:00` (kann selbst 2),
    // unsere Sendemaske trug MCS0-15 und wir SENDETEN mit MCS15 -- und
    // empfingen trotzdem MCS7 in 29804 von 35605 Rahmen. 49 Mbit aus
    // 72 Mbit brutto sind 68 % Effizienz, der Empfangsweg war also nie das
    // Problem. Es fehlte der zweite Strom, und wir hatten ihn selbst
    // abbestellt.
    cap |= WLAN_HT_CAP_SM_PS_DISABLED << IEEE80211_HT_CAP_SM_PS_SHIFT;
    // `hw_cap.bw & BIT(RTW_CHANNEL_WIDTH_40)`
    if hw_cap_bw & (1 << 1) != 0 {
        cap |= IEEE80211_HT_CAP_SUP_WIDTH_20_40
            | IEEE80211_HT_CAP_DSSSCCK40
            | IEEE80211_HT_CAP_SGI_40;
    }

    out[0] = WLAN_EID_HT_CAPABILITY as u8;
    out[1] = 26;
    let b = &mut out[2..28];
    b.fill(0);
    b[0..2].copy_from_slice(&(cap as u16).to_le_bytes());
    // A-MPDU: Faktor in Bit 1:0, Dichte in Bit 4:2.
    b[2] = (IEEE80211_HT_MAX_AMPDU_64K as u8 & 0x3)
        | ((IEEE80211_HT_MPDU_DENSITY_2 as u8 & 0x7) << 2);
    // Supported MCS Set: rx_mask[0..10], rx_highest(2), tx_params(1), Rest 0.
    for i in 0..nss.min(4) as usize {
        b[3 + i] = 0xff;
    }
    b[3 + 4] = 0x01; // `mcs.rx_mask[4] = 0x01`
    b[3 + 10..3 + 12].copy_from_slice(&(150u16 * nss as u16).to_le_bytes());
    b[3 + 12] = IEEE80211_HT_MCS_TX_DEFINED as u8;
    28
}

/// main.c:1602-1643 `rtw_init_vht_cap`, als fertiges VHT-Element
/// (802.11 §9.4.2.157: id 191, 12 Byte Rumpf).
///
/// Kehrt um, wenn die efuse etwas anderes als VHT ansagt — dieselbe
/// Bedingung wie in Linux. `bfee_sts_cap` ist 3 (main.c:1905),
/// `rf_path_num > 1` gilt hier.
pub fn build_vht_cap_ie(out: &mut [u8], hw_cap_ptcl: u8, nss: u8,
                        rf_path_num: u8, ap_vht_cap: Option<u32>) -> usize {
    if hw_cap_ptcl != EFUSE_HW_CAP_IGNORE as u8
        && hw_cap_ptcl != EFUSE_HW_CAP_PTCL_VHT as u8
    {
        return 0;
    }
    let mut cap = IEEE80211_VHT_CAP_MAX_MPDU_LENGTH_11454
        | IEEE80211_VHT_CAP_SHORT_GI_80
        | IEEE80211_VHT_CAP_RXSTBC_1
        | IEEE80211_VHT_CAP_HTC_VHT
        | IEEE80211_VHT_CAP_MAX_A_MPDU_LENGTH_EXPONENT_MASK;
    if rf_path_num > 1 {
        cap |= IEEE80211_VHT_CAP_TXSTBC;
    }
    cap |= IEEE80211_VHT_CAP_MU_BEAMFORMEE_CAPABLE
        | IEEE80211_VHT_CAP_SU_BEAMFORMEE_CAPABLE;
    cap |= 3 << IEEE80211_VHT_CAP_BEAMFORMEE_STS_SHIFT; // bfee_sts_cap
    cap |= IEEE80211_VHT_CAP_RXLDPC; // rx_ldpc

    // ── Und hier stutzt mac80211, was rtw88 gesetzt hat ──────────
    //
    // `ieee80211_add_vht_ie` (mlme.c:1481-1526). Bis hierher war diese
    // Funktion eine treue Portierung von `rtw_init_vht_cap` — und
    // genau das war zu wenig: in Linux geht das Ergebnis NICHT so
    // hinaus, wie der Treiber es baut. Dazwischen liegt eine Schicht,
    // und ihr Kommentar sagt woertlich, wofuer sie da ist:
    //
    //     Some APs apparently get confused if our capabilities are
    //     better than theirs, so restrict what we advertise in the
    //     assoc request.
    //
    // Dasselbe Muster wie bei SM Power Save (0.48.0) und beim
    // Duplikatsfilter: die Zutat sitzt eine Schicht UEBER dem Treiber,
    // und wer nur den Treiber portiert, liefert ein Element aus, das so
    // nie auf der Luft war.
    //
    // Ohne den Bezugspunkt bleibt alles stehen — ein Suchlauf ohne
    // Bake des AP soll nicht anders anbieten als einer mit.
    if let Some(ap) = ap_vht_cap {
        if ap & IEEE80211_VHT_CAP_SU_BEAMFORMER_CAPABLE == 0 {
            cap &= !(IEEE80211_VHT_CAP_SU_BEAMFORMEE_CAPABLE
                | IEEE80211_VHT_CAP_MU_BEAMFORMEE_CAPABLE);
        } else if ap & IEEE80211_VHT_CAP_MU_BEAMFORMER_CAPABLE == 0 {
            cap &= !IEEE80211_VHT_CAP_MU_BEAMFORMEE_CAPABLE;
        }
        // Und die Zahl der Raumzeit-Stroeme, die wir als Beamformee
        // annehmen: nie mehr, als der AP zu senden angibt.
        let ap_sts = ap & IEEE80211_VHT_CAP_BEAMFORMEE_STS_MASK;
        let our_sts = cap & IEEE80211_VHT_CAP_BEAMFORMEE_STS_MASK;
        if ap_sts < our_sts {
            cap &= !IEEE80211_VHT_CAP_BEAMFORMEE_STS_MASK;
            cap |= ap_sts;
        }
    }

    let mut mcs_map = 0u16;
    for i in 0..8u16 {
        let v = if (i as u8) < nss {
            IEEE80211_VHT_MCS_SUPPORT_0_9 as u16
        } else {
            IEEE80211_VHT_MCS_NOT_SUPPORTED as u16
        };
        mcs_map |= v << (i * 2);
    }
    let highest = 390u16 * nss as u16;

    out[0] = WLAN_EID_VHT_CAPABILITY as u8;
    out[1] = 12;
    let b = &mut out[2..14];
    b[0..4].copy_from_slice(&cap.to_le_bytes());
    b[4..6].copy_from_slice(&mcs_map.to_le_bytes()); // rx_mcs_map
    b[6..8].copy_from_slice(&highest.to_le_bytes()); // rx_highest
    b[8..10].copy_from_slice(&mcs_map.to_le_bytes()); // tx_mcs_map
    b[10..12].copy_from_slice(&highest.to_le_bytes()); // tx_highest
    14
}

// ═══════════════════════════════════════════════════════════════════
// Block Ack: die Antwort auf den ADDBA Request des AP
//
// **In rtw88 macht das mac80211, nicht der Treiber** —
// `rtw_ops_ampdu_action` behandelt `IEEE80211_AMPDU_RX_START` mit einem
// leeren `break`. Die Empfangs-Aggregation ist also reine
// 802.11-Verwaltung: wer zustimmt, bekommt Aggregate; die BlockAcks
// darauf erzeugt die HARDWARE, weil sie eine SIFS nach dem Aggregat
// hinaus muessen (16 us) — kein Treiber der Welt schafft das.
// ═══════════════════════════════════════════════════════════════════

/// Was in einem ADDBA Request steht (802.11 §9.6.7.2,
/// `struct ieee80211_mgmt.u.action.u.addba_req`).
#[derive(Clone, Copy, Default)]
pub struct AddbaReq {
    pub dialog_token: u8,
    pub amsdu: bool,
    pub policy: u16,
    pub tid: u8,
    pub buf_size: u16,
    pub timeout: u16,
    pub ssn: u16,
}

/// Den Rahmen lesen. `f` ist der ganze 802.11-Rahmen ab `frame_control`;
/// Kategorie und Aktionscode stehen hinter dem 24 Byte langen Kopf.
pub fn parse_addba_req(f: &[u8]) -> Option<AddbaReq> {
    // 24 Kopf + Kategorie + Aktion + Token + capab + timeout + ssn
    if f.len() < 24 + 1 + 1 + 1 + 2 + 2 + 2 {
        return None;
    }
    if f[24] != DOT11_ACTION_CAT_BA || f[25] != DOT11_ACTION_ADDBA_REQ {
        return None;
    }
    let capab = u16::from_le_bytes([f[27], f[28]]);
    Some(AddbaReq {
        dialog_token: f[26],
        amsdu: capab & ADDBA_PARAM_AMSDU_MASK != 0,
        policy: (capab & ADDBA_PARAM_POLICY_MASK) >> 1,
        tid: ((capab & ADDBA_PARAM_TID_MASK) >> 2) as u8,
        buf_size: (capab & ADDBA_PARAM_BUF_SIZE_MASK) >> 6,
        timeout: u16::from_le_bytes([f[29], f[30]]),
        ssn: u16::from_le_bytes([f[31], f[32]]),
    })
}

/// tx.c:95-105 `get_tx_ampdu_factor` — und der Kommentar dort ist der
/// ganze Grund fuer die Rechnung.
///
/// Im Deskriptor steht **nicht** der Exponent, sondern `MAX_AGG_NUM`, und
/// dessen Wert mal zwei ist die Zahl der Rahmen. Die kleinste
/// A-MPDU-Laenge ist 8 K, also ist die Basis 8/2 = 4.
///
/// Exponent 0..3 ergibt damit 3, 7, 15, 31 — und 31 ist genau der groesste
/// Wert, den das fuenf Bit breite Feld traegt.
pub fn tx_ampdu_factor(ampdu_factor: u8) -> u8 {
    // `0x4` und nicht `4`, damit `seqdiff.py` die Zahl gegen Linux'
    // `BIT(2)` halten kann — der Zahlenvergleich liest Hexliterale.
    (0x4u8 << ampdu_factor) - 1
}

/// tx.c:107-110 `get_tx_ampdu_density` — Bit 4:2, der Mindestabstand
/// zwischen zwei Rahmen im Aggregat.
pub fn tx_ampdu_density(ampdu_density: u8) -> u8 {
    ampdu_density
}

/// Die Fensterbreite, die wir ERBITTEN.
///
/// mac80211 nimmt fuer eine Station ohne HE `IEEE80211_MAX_AMPDU_BUF_HT`
/// (64) und schreibt daneben, warum es nicht die Zahl des Treibers ist:
/// manche APs stuerzen bei kleineren Werten ab (agg-tx.c:472-481).
pub const BA_TX_BUF_SIZE: u16 = 64;

/// Den ADDBA **Request** bauen — `ieee80211_send_addba_request`
/// (net/mac80211/agg-tx.c:61-101), Feld fuer Feld.
///
/// **Das ist die Haelfte, die uns die ganze Zeit gefehlt hat.** Seit
/// 0.29.0 beantworten wir die Bitte des AP und bekommen deshalb
/// Aggregate — gefragt haben wir nie, also sendet jeder unserer Rahmen
/// einzeln. Auf einer schnellen Strecke ist das der teuerste Posten, den
/// es gibt: die Kosten je Sendevorgang sind fast ganz fix (AIFS, Backoff,
/// Praeambel, SIFS, ACK), und die Datenzeit schrumpft mit der Rate.
///
/// `ssn` ist die Folgenummer, ab der die Sitzung zaehlt — sie faehrt um
/// vier Stellen nach links, weil die unteren vier Bit des Feldes die
/// Fragmentnummer sind.
///
/// **`amsdu` steht auf JA, obwohl wir keine A-MSDU bauen.** Linux setzt
/// das Bit bedingungslos; es sagt, was der Absender senden DARF, nicht
/// was er sendet. Wer hier weniger ansagt, bekommt nichts geschenkt und
/// weicht ohne Grund ab.
pub fn build_addba_req(out: &mut [u8; 256], mac: &[u8; 6], bssid: &[u8; 6],
                       tid: u8, dialog_token: u8, ssn: u16,
                       buf_size: u16, timeout: u16) -> usize {
    out.fill(0);
    out[0] = DOT11_FC_ACTION;
    out[1] = 0x00;
    out[4..10].copy_from_slice(bssid); // addr1 = Empfaenger
    out[10..16].copy_from_slice(mac); // addr2 = wir
    out[16..22].copy_from_slice(bssid); // addr3 = BSSID

    out[24] = DOT11_ACTION_CAT_BA;
    out[25] = DOT11_ACTION_ADDBA_REQ;
    out[26] = dialog_token;

    let capab = ADDBA_PARAM_AMSDU_MASK
        | ADDBA_PARAM_POLICY_MASK // 1 = sofortiger Block Ack
        | (((tid as u16) << 2) & ADDBA_PARAM_TID_MASK)
        | ((buf_size << 6) & ADDBA_PARAM_BUF_SIZE_MASK);
    out[27..29].copy_from_slice(&capab.to_le_bytes());
    out[29..31].copy_from_slice(&timeout.to_le_bytes());
    out[31..33].copy_from_slice(&(ssn << 4).to_le_bytes());
    33
}

/// Was in einer ADDBA **Response** steht (802.11 §9.6.7.3).
///
/// Die Reihenfolge ist eine andere als im Request: hier steht der
/// STATUS vor den Faehigkeiten, dort die Folgenummer dahinter. Wer die
/// zwei Rahmen mit einem Parser liest, liest den Status als Fenster.
#[derive(Clone, Copy)]
pub struct AddbaResp {
    pub dialog_token: u8,
    pub status: u16,
    pub tid: u8,
    pub buf_size: u16,
    pub amsdu: bool,
    pub timeout: u16,
}

/// `ieee80211_process_addba_resp` (net/mac80211/agg-tx.c:969-1000), der
/// lesende Teil.
pub fn parse_addba_resp(f: &[u8]) -> Option<AddbaResp> {
    // 24 Kopf + Kategorie + Aktion + Token + Status + capab + timeout
    if f.len() < 24 + 1 + 1 + 1 + 2 + 2 + 2 {
        return None;
    }
    if f[24] != DOT11_ACTION_CAT_BA || f[25] != DOT11_ACTION_ADDBA_RESP {
        return None;
    }
    let capab = u16::from_le_bytes([f[29], f[30]]);
    Some(AddbaResp {
        dialog_token: f[26],
        status: u16::from_le_bytes([f[27], f[28]]),
        amsdu: capab & ADDBA_PARAM_AMSDU_MASK != 0,
        tid: ((capab & ADDBA_PARAM_TID_MASK) >> 2) as u8,
        buf_size: (capab & ADDBA_PARAM_BUF_SIZE_MASK) >> 6,
        timeout: u16::from_le_bytes([f[31], f[32]]),
    })
}

/// Die Antwort bauen — `ieee80211_send_addba_resp` (net/mac80211/agg-rx.c)
/// Feld fuer Feld.
///
/// **`buf_size` ist unsere Entscheidung, nicht seine.** Der AP fragt, wie
/// viele Rahmen er offen haben darf; mac80211 antwortet mit
/// `min(erbeten, hw.max_rx_aggregation_subframes)` — und das ist
/// `IEEE80211_MAX_AMPDU_BUF_HT` = 64 (main.c:953), von rtw88 NICHT
/// ueberschrieben.
///
/// **64 ist keine Einstellung, sondern die Decke des Protokolls.** Der
/// komprimierte Block Ack traegt eine Bitmaske von 64 Bit, ein Bit je
/// Rahmen; mehr gaebe es erst mit 802.11ax (`..._BUF_HE` = 256) oder
/// 802.11be (`..._BUF_EHT` = 1024). Der 8822CE ist 802.11ac.
///
/// Hier stand bis 0.52.2 eine kleine Zahl, und die Begruendung darunter
/// war: *„Wir haben keinen Umsortierpuffer."* **Seit 0.49.0 haben wir
/// einen** (`RO_WIN` = 64, derselbe Wert), die Begruendung war also drei
/// Versionen alt und der Deckel blieb stehen. Wer `ampdu:` nicht in
/// seiner Konfig hatte, bekam acht offene Rahmen statt
/// vierundsechzig — und damit hoechstens ein Achtel der Aggregation,
/// die der AP angeboten hat.
///
/// `amsdu` melden wir als NEIN: A-MSDU im A-MPDU waere ein zweiter
/// Entpacker, den es hier nicht gibt.
///
/// `amsdu` melden wir als NEIN: A-MSDU im A-MPDU waere ein zweiter
/// Entpacker, den es hier nicht gibt.
pub fn build_addba_resp(out: &mut [u8; 256], mac: &[u8; 6], bssid: &[u8; 6],
                        req: &AddbaReq, buf_size: u16) -> usize {
    out.fill(0);
    out[0] = DOT11_FC_ACTION;
    out[1] = 0x00;
    out[4..10].copy_from_slice(bssid); // addr1 = Empfaenger
    out[10..16].copy_from_slice(mac); // addr2 = wir
    out[16..22].copy_from_slice(bssid); // addr3 = BSSID

    out[24] = DOT11_ACTION_CAT_BA;
    out[25] = DOT11_ACTION_ADDBA_RESP;
    out[26] = req.dialog_token;
    out[27..29].copy_from_slice(&WLAN_STATUS_SUCCESS.to_le_bytes());

    // capab: A-MSDU aus, Policy und TID wie erbeten, unsere Fenstergroesse.
    let capab = ((req.policy << 1) & ADDBA_PARAM_POLICY_MASK)
        | (((req.tid as u16) << 2) & ADDBA_PARAM_TID_MASK)
        | ((buf_size << 6) & ADDBA_PARAM_BUF_SIZE_MASK);
    out[29..31].copy_from_slice(&capab.to_le_bytes());
    out[31..33].copy_from_slice(&req.timeout.to_le_bytes());
    33
}
