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

use crate::virtio_net;

/// Process incoming packets and TCP timers
pub fn poll() {
    let mut buf = [0u8; virtio_net::MTU];
    while let Some(len) = virtio_net::recv(&mut buf) {
        if len >= 14 {
            eth::handle_frame(&buf[..len]);
        }
    }
    tcp::tick_connections();
}

/// Network stack statistics
#[allow(dead_code)]
pub fn is_up() -> bool {
    virtio_net::is_available()
}
