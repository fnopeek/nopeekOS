//! Network Stack
//!
//! Capability-gated TCP/IP implementation.
//! Layers: Ethernet → ARP → IPv4 → ICMP/UDP/TCP
//! Every connection requires a capability token.

pub mod eth;
pub mod arp;
pub mod ipv4;
pub mod icmp;
pub mod udp;
pub mod dns;
pub mod dhcp;
pub mod ntp;
pub mod tcp;

use crate::netdev;

/// Process incoming packets and TCP timers
pub fn poll() {
    let mut buf = [0u8; netdev::MTU];
    while let Some(len) = netdev::recv(&mut buf) {
        if len >= 14 {
            eth::handle_frame(&buf[..len]);
        }
    }
    tcp::tick_connections();
    // Progressive shade render (shows output during long network operations)
    crate::shade::poll_render();
}

/// Network stack statistics
#[allow(dead_code)]
pub fn is_up() -> bool {
    netdev::is_available()
}
