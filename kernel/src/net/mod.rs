//! Network Stack
//!
//! Capability-gated TCP/IP implementation.
//! Layers: Ethernet → ARP → IPv4 → ICMP/UDP/TCP
//! Every connection requires a capability token.

pub mod eth;
pub mod arp;
pub mod ipv4;
pub mod icmp;

use crate::virtio_net;

/// Process incoming packets (called from intent loop or poll)
pub fn poll() {
    let mut buf = [0u8; virtio_net::MTU];
    while let Some(len) = virtio_net::recv(&mut buf) {
        if len >= 14 {
            eth::handle_frame(&buf[..len]);
        }
    }
}

/// Network stack statistics
pub fn is_up() -> bool {
    virtio_net::is_available()
}
