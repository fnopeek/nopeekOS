//! Network Device Abstraction
//!
//! Dispatches to Intel NIC, WASM driver NIC, or virtio-net (in that order).

use crate::{virtio_net, intel_nic, rtl8153};
use crate::virtio_net::NetError;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

pub const MTU: usize = 1514;

// ── WASM-backed NIC (registered by WASM driver modules) ──

static WASM_NIC_ACTIVE: AtomicBool = AtomicBool::new(false);
static WASM_NIC: Mutex<WasmNic> = Mutex::new(WasmNic::empty());

struct WasmNic {
    mac_addr: [u8; 6],
    /// TX mailbox: kernel writes frames here for the WASM driver to transmit
    tx_buf: [u8; MTU],
    tx_len: u16,
    /// RX mailbox: WASM driver writes received frames here for the kernel
    rx_buf: [u8; MTU],
    rx_len: u16,
    /// Carrier/link state, set by the driver via npk_netdev_set_link. For a
    /// WiFi NIC this is "associated + keyed" (data path live), distinct from
    /// mere registration. A wired NIC has no such notion (always linked once
    /// present), so this only refines the WASM NIC's reported state.
    link_up: bool,
}

impl WasmNic {
    const fn empty() -> Self {
        WasmNic {
            mac_addr: [0; 6],
            tx_buf: [0; MTU],
            tx_len: 0,
            rx_buf: [0; MTU],
            rx_len: 0,
            link_up: false,
        }
    }
}

/// Called by WASM host function npk_netdev_register
pub fn register_wasm_nic(mac: [u8; 6]) {
    let mut nic = WASM_NIC.lock();
    nic.mac_addr = mac;
    nic.tx_len = 0;
    nic.rx_len = 0;
    nic.link_up = false;
    WASM_NIC_ACTIVE.store(true, Ordering::Release);
}

/// Called by cleanup_hw_state when WASM driver exits
pub fn unregister_wasm_nic() {
    WASM_NIC.lock().link_up = false;
    WASM_NIC_ACTIVE.store(false, Ordering::Release);
}

pub fn wasm_nic_available() -> bool {
    WASM_NIC_ACTIVE.load(Ordering::Acquire)
}

/// Driver reports its carrier/link state (associated + keyed) via
/// npk_netdev_set_link. Lets `net` show a real UP/DOWN for `wlan`.
pub fn set_wasm_nic_link(up: bool) {
    WASM_NIC.lock().link_up = up;
}

pub fn wasm_nic_link_up() -> bool {
    wasm_nic_available() && WASM_NIC.lock().link_up
}

/// WASM driver calls this to submit a received frame to the kernel network stack
pub fn wasm_nic_submit_rx(frame: &[u8]) {
    if frame.len() > MTU { return; }
    let mut nic = WASM_NIC.lock();
    nic.rx_buf[..frame.len()].copy_from_slice(frame);
    nic.rx_len = frame.len() as u16;
}

/// WASM driver calls this to get a frame to transmit
pub fn wasm_nic_poll_tx(buf: &mut [u8; MTU]) -> Option<usize> {
    let mut nic = WASM_NIC.lock();
    if nic.tx_len == 0 { return None; }
    let len = nic.tx_len as usize;
    buf[..len].copy_from_slice(&nic.tx_buf[..len]);
    nic.tx_len = 0;
    Some(len)
}

pub fn send(frame: &[u8]) -> Result<(), NetError> {
    // An associated + keyed WiFi link wins: the user explicitly brought it up,
    // and a wired NIC may report stale-available (USB unplug isn't detected),
    // which would otherwise keep swallowing DHCP. Wired is used only when WiFi
    // is down.
    if wasm_nic_link_up() {
        let mut nic = WASM_NIC.lock();
        let len = frame.len().min(MTU);
        nic.tx_buf[..len].copy_from_slice(&frame[..len]);
        nic.tx_len = len as u16;
        Ok(())
    } else if intel_nic::is_available() {
        intel_nic::send(frame)
    } else if rtl8153::is_available() {
        rtl8153::send(frame)
    } else if wasm_nic_available() {
        let mut nic = WASM_NIC.lock();
        let len = frame.len().min(MTU);
        nic.tx_buf[..len].copy_from_slice(&frame[..len]);
        nic.tx_len = len as u16;
        Ok(())
    } else {
        virtio_net::send(frame)
    }
}

pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {
    if wasm_nic_link_up() {
        let mut nic = WASM_NIC.lock();
        if nic.rx_len == 0 { return None; }
        let len = nic.rx_len as usize;
        buf[..len].copy_from_slice(&nic.rx_buf[..len]);
        nic.rx_len = 0;
        Some(len)
    } else if intel_nic::is_available() {
        intel_nic::recv(buf)
    } else if rtl8153::is_available() {
        rtl8153::recv(buf)
    } else if wasm_nic_available() {
        let mut nic = WASM_NIC.lock();
        if nic.rx_len == 0 { return None; }
        let len = nic.rx_len as usize;
        buf[..len].copy_from_slice(&nic.rx_buf[..len]);
        nic.rx_len = 0;
        Some(len)
    } else {
        virtio_net::recv(buf)
    }
}

pub fn mac() -> Option<[u8; 6]> {
    if wasm_nic_link_up() {
        Some(WASM_NIC.lock().mac_addr)
    } else if intel_nic::is_available() {
        intel_nic::mac()
    } else if rtl8153::is_available() {
        rtl8153::mac()
    } else if wasm_nic_available() {
        Some(WASM_NIC.lock().mac_addr)
    } else {
        virtio_net::mac()
    }
}

pub fn is_available() -> bool {
    intel_nic::is_available() || rtl8153::is_available() || wasm_nic_available() || virtio_net::is_available()
}

// ── Interface enumeration ──

#[derive(Clone, Copy)]
pub struct IfaceInfo {
    pub name: &'static str,
    pub driver: &'static str,
    pub mac: [u8; 6],
    /// True for the interface that carries the global IP/Gateway/DNS config.
    pub primary: bool,
    /// Carrier/link state. Wired NICs are linked once present; the WiFi NIC is
    /// linked only once associated + keyed (driver reports via set_wasm_nic_link).
    pub link_up: bool,
}

/// List all active network interfaces. The first UP interface (Intel → WASM → virtio)
/// is marked primary and carries the global IPv4/Gateway/DNS config.
pub fn list() -> alloc::vec::Vec<IfaceInfo> {
    let mut v = alloc::vec::Vec::new();
    // An associated WiFi link is the primary interface (carries the global IP),
    // overriding a stale-available wired NIC.
    let wifi_up = wasm_nic_available() && wasm_nic_link_up();
    let mut primary_taken = false;

    if intel_nic::is_available() {
        if let Some(mac) = intel_nic::mac() {
            v.push(IfaceInfo { name: "eth", driver: "Intel I226-V", mac, primary: !primary_taken && !wifi_up, link_up: true });
            primary_taken = true;
        }
    }
    if rtl8153::is_available() {
        if let Some(mac) = rtl8153::mac() {
            v.push(IfaceInfo { name: "eth", driver: "Realtek RTL8153 (USB)", mac, primary: !primary_taken && !wifi_up, link_up: true });
            primary_taken = true;
        }
    }
    if wasm_nic_available() {
        let mac = WASM_NIC.lock().mac_addr;
        v.push(IfaceInfo { name: "wlan", driver: "WiFi (WASM)", mac, primary: wifi_up || !primary_taken, link_up: wasm_nic_link_up() });
        primary_taken = true;
    }
    if virtio_net::is_available() {
        if let Some(mac) = virtio_net::mac() {
            v.push(IfaceInfo { name: "eth", driver: "virtio-net", mac, primary: !primary_taken, link_up: true });
        }
    }
    v
}
