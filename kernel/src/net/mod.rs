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
use core::sync::atomic::{AtomicBool, Ordering};

/// Serialises NIC RX-ring access across cores. With guest-SMP, Core 0's loop
/// AND the BSP vCPU pump both call `poll()`. `netdev::recv` drains a
/// single-consumer ring; two cores draining it concurrently race → reordered /
/// dropped frames → the guest's TCP collapses (observed: speedtest stalled at
/// high throughput once the pump stopped idle-parking and polled continuously).
/// Best practice for a shared polled device: one drainer at a time. This is a
/// NON-blocking guard — whoever holds it does the work, the other core skips
/// (its data is drained by the holder + picked up next pass).
static POLLING: AtomicBool = AtomicBool::new(false);

/// Process incoming packets and TCP timers. Cross-core mutually exclusive.
pub fn poll() {
    if POLLING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return; // another core is already draining the NIC
    }
    let mut buf = [0u8; netdev::MTU];
    while let Some(len) = netdev::recv(&mut buf) {
        if len >= 14 {
            eth::handle_frame(&buf[..len]);
        }
    }
    tcp::tick_connections();
    // Progressive shade render (shows output during long network operations)
    crate::shade::poll_render();
    POLLING.store(false, Ordering::Release);
}

/// Network stack statistics
#[allow(dead_code)]
pub fn is_up() -> bool {
    netdev::is_available()
}
