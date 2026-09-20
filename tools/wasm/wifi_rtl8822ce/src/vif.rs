//! `rtw_vif_port_config` und der Teil von `rtw_ops_add_interface`, der
//! Hardware anfasst — Stufe 5b.
//!
//! Ohne diese Funktion steht im Port-Register des Chips keine Adresse, und
//! der Empfangsfilter (`WLAN_RCR_CFG` hat `BIT_APM`) laesst dann nur
//! Broadcast und Multicast durch. Eine Probe Response ist an UNSERE
//! Adresse gerichtet — sie kaeme nie an, und das sieht am Geraet genauso
//! aus wie „der AP hat nicht geantwortet".
#![allow(dead_code)]

use crate::host;
use crate::regs::*;

/// main.h:594-600 `struct rtw_vif_port`, Eintrag 0 aus mac80211.c:108-115.
pub struct VifPort {
    pub mac_addr: u32,
    pub bssid: u32,
    pub net_type: (u32, u32),
    pub aid: (u32, u32),
    pub bcn_ctrl: (u32, u32),
}

/// mac80211.c:108-115 `rtw_vif_port[0]`.
///
/// Linux fuehrt fuenf Ports und vergibt den ersten freien
/// (`find_first_zero_bit(rtwdev->hw_port, RTW_PORT_NUM)`). Bei EINER
/// Schnittstelle ist das immer die Null; die anderen vier stehen hier
/// nicht, weil kein Rufer sie waehlen kann.
pub const PORT0: VifPort = VifPort {
    mac_addr: PORT0_MAC_ADDR,
    bssid: PORT0_BSSID,
    net_type: (PORT0_NET_TYPE, PORT0_NET_TYPE_MASK),
    aid: (PORT0_AID, PORT0_AID_MASK),
    bcn_ctrl: (PORT0_BCN_CTRL, PORT0_BCN_CTRL_MASK),
};

/// main.h:812-830 `struct rtw_vif` — die Felder, die der Port traegt.
#[derive(Default, Clone, Copy)]
pub struct Vif {
    pub mac_addr: [u8; 6],
    pub bssid: [u8; 6],
    pub net_type: u32,
    pub aid: u32,
    pub bcn_ctrl: u8,
    pub mac_id: u8,
    pub port: u8,
}

/// main.c:659-665 `rtw_vif_write_addr` — sechs EINZELNE Bytes.
fn write_addr(h: i32, start: u32, addr: &[u8; 6]) {
    for (i, &b) in addr.iter().enumerate() {
        host::w8(h, start + i as u32, b);
    }
}

/// main.c:667-694 `rtw_vif_port_config`
pub fn port_config(h: i32, v: &Vif, config: u32) {
    let c = &PORT0;
    if config & PORT_SET_MAC_ADDR != 0 {
        write_addr(h, c.mac_addr, &v.mac_addr);
    }
    if config & PORT_SET_BSSID != 0 {
        write_addr(h, c.bssid, &v.bssid);
    }
    if config & PORT_SET_NET_TYPE != 0 {
        host::w32_mask(h, c.net_type.0, c.net_type.1, v.net_type);
    }
    if config & PORT_SET_AID != 0 {
        host::w32_mask(h, c.aid.0, c.aid.1, v.aid);
    }
    if config & PORT_SET_BCN_CTRL != 0 {
        host::w8_mask(h, c.bcn_ctrl.0, c.bcn_ctrl.1, v.bcn_ctrl);
    }
}

/// mac80211.c:155-225 `rtw_ops_add_interface`, Zweig
/// `NL80211_IFTYPE_STATION`.
///
/// **Was daneben steht und hier NICHT:**
/// * `rtw_acquire_macid` sucht das erste freie Bit in `mac_id_map` — bei
///   der ersten Schnittstelle ist das die 0, und eine zweite gibt es bei
///   uns nicht. Die Karte selbst waere eine Behauptung ueber Stufe 5c.
/// * `rtw_add_rsvd_page_sta` haengt PS-Poll, Null- und QoS-Null-Rahmen an
///   `rsvd_page_list`. Die LISTE fasst keine Hardware an; sie wird erst von
///   `rtw_fw_download_rsvd_page` geschrieben, und das ruft in Linux
///   `rtw_bss_info_changed` beim Verbinden. Kein Rufer, also kein Code.
/// * `rtw_core_port_switch` kehrt in seiner ERSTEN Zeile um, wenn der Typ
///   nicht AP ist (main.c). Fuer uns ist es kein weggelassener Aufruf,
///   sondern ein Aufruf ohne Wirkung.
/// * `rtw_recalc_lps` gehoert zum Stromsparen, das dieser Treiber nicht
///   fuehrt — `rtw_leave_lps_deep` daneben ebenso.
/// * `rtw_txq_init`, die Statistik und `rtwvif->bfee` sind Zustand der
///   oberen Haelfte.
pub fn add_interface_station(h: i32, mac_addr: [u8; 6]) -> Vif {
    let v = Vif {
        mac_addr,
        mac_id: 0,
        port: 0,
        net_type: RTW_NET_NO_LINK,
        bcn_ctrl: BIT_EN_BCN_FUNCTION,
        ..Default::default()
    };
    port_config(h, &v, PORT_SET_MAC_ADDR | PORT_SET_NET_TYPE
                       | PORT_SET_BCN_CTRL);
    v
}
