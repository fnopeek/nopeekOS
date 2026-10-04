//! `rtw_vif_port_config` and the hardware-touching part of
//! `rtw_ops_add_interface`.
//!
//! Without the port configuration the chip's port register holds no
//! address, and the RX filter (`WLAN_RCR_CFG` has `BIT_APM`) then passes
//! only broadcast and multicast. A unicast probe response would never
//! arrive, which looks exactly like an AP that did not answer.
#![allow(dead_code)]

use crate::host;
use crate::regs::*;

/// main.h:594-600 `struct rtw_vif_port`, entry 0 from mac80211.c:108-115.
pub struct VifPort {
    pub mac_addr: u32,
    pub bssid: u32,
    pub net_type: (u32, u32),
    pub aid: (u32, u32),
    pub bcn_ctrl: (u32, u32),
}

/// mac80211.c:108-115 `rtw_vif_port[0]`.
///
/// Linux has five ports and assigns the first free one
/// (`find_first_zero_bit(rtwdev->hw_port, RTW_PORT_NUM)`). With a single
/// interface that is always port 0, so the other four are omitted.
pub const PORT0: VifPort = VifPort {
    mac_addr: PORT0_MAC_ADDR,
    bssid: PORT0_BSSID,
    net_type: (PORT0_NET_TYPE, PORT0_NET_TYPE_MASK),
    aid: (PORT0_AID, PORT0_AID_MASK),
    bcn_ctrl: (PORT0_BCN_CTRL, PORT0_BCN_CTRL_MASK),
};

/// main.h:812-830 `struct rtw_vif`: the fields the port carries.
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

/// main.c:659-665 `rtw_vif_write_addr`: six individual byte writes.
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

/// mac80211.c:155-225 `rtw_ops_add_interface`, branch
/// `NL80211_IFTYPE_STATION`.
///
/// Not ported from the surrounding code:
/// * `rtw_acquire_macid` picks the first free bit in `mac_id_map`; for the
///   only interface that is 0.
/// * `rtw_add_rsvd_page_sta` appends PS-Poll, null and QoS-null frames to
///   `rsvd_page_list`. The list touches no hardware until
///   `rtw_fw_download_rsvd_page`, which Linux calls from
///   `rtw_bss_info_changed` on association.
/// * `rtw_core_port_switch` returns on its first line unless the type is
///   AP (main.c), so it has no effect here.
/// * `rtw_recalc_lps` and `rtw_leave_lps_deep` belong to power saving,
///   which this driver does not implement.
/// * `rtw_txq_init`, statistics and `rtwvif->bfee` are upper-layer state.
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
