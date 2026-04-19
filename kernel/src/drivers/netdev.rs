//! Network Device Abstraction
//!
//! Dispatches to Intel NIC, WASM driver NIC, or virtio-net (in that order).

use crate::{virtio_net, intel_nic};
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
}

impl WasmNic {
    const fn empty() -> Self {
        WasmNic {
            mac_addr: [0; 6],
            tx_buf: [0; MTU],
            tx_len: 0,
            rx_buf: [0; MTU],
            rx_len: 0,
        }
    }
}

/// Called by WASM host function npk_netdev_register
pub fn register_wasm_nic(mac: [u8; 6]) {
    let mut nic = WASM_NIC.lock();
    nic.mac_addr = mac;
    nic.tx_len = 0;
    nic.rx_len = 0;
    WASM_NIC_ACTIVE.store(true, Ordering::Release);
}

/// Called by cleanup_hw_state when WASM driver exits
pub fn unregister_wasm_nic() {
    WASM_NIC_ACTIVE.store(false, Ordering::Release);
}

pub fn wasm_nic_available() -> bool {
    WASM_NIC_ACTIVE.load(Ordering::Acquire)
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
    if intel_nic::is_available() {
        intel_nic::send(frame)
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
    if intel_nic::is_available() {
        intel_nic::recv(buf)
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
    if intel_nic::is_available() {
        intel_nic::mac()
    } else if wasm_nic_available() {
        Some(WASM_NIC.lock().mac_addr)
    } else {
        virtio_net::mac()
    }
}

pub fn is_available() -> bool {
    intel_nic::is_available() || wasm_nic_available() || virtio_net::is_available()
}

// ── Interface enumeration ──

#[derive(Clone, Copy)]
pub struct IfaceInfo {
    pub name: &'static str,
    pub driver: &'static str,
    pub mac: [u8; 6],
    /// True for the interface that carries the global IP/Gateway/DNS config.
    pub primary: bool,
}

/// List all active network interfaces. The first UP interface (Intel → WASM → virtio)
/// is marked primary and carries the global IPv4/Gateway/DNS config.
pub fn list() -> alloc::vec::Vec<IfaceInfo> {
    let mut v = alloc::vec::Vec::new();
    let mut primary_taken = false;

    if intel_nic::is_available() {
        if let Some(mac) = intel_nic::mac() {
            v.push(IfaceInfo { name: "eth", driver: "Intel I226-V", mac, primary: !primary_taken });
            primary_taken = true;
        }
    }
    if wasm_nic_available() {
        let mac = WASM_NIC.lock().mac_addr;
        v.push(IfaceInfo { name: "wlan", driver: "WiFi (WASM)", mac, primary: !primary_taken });
        primary_taken = true;
    }
    if virtio_net::is_available() {
        if let Some(mac) = virtio_net::mac() {
            v.push(IfaceInfo { name: "eth", driver: "virtio-net", mac, primary: !primary_taken });
        }
    }
    v
}
