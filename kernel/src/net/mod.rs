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

/// Process incoming packets and TCP timers.
pub fn poll() {
    // The guard wraps ONLY the single-consumer NIC drain + host-TCP tick
    // (cross-core mutually exclusive). It must NOT cover the compositor below:
    // the BSP vCPU pump holds this guard while it spin-pumps under network
    // load, so if Core 0's poll() returned early here it would never render or
    // poll the mouse → UI + mouse freeze (observed as a notebook kernel freeze:
    // a slow NIC keeps the BSP pumping ~continuously → Core 0 fully starved).
    if POLLING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        let mut buf = [0u8; netdev::MTU];
        while let Some(len) = netdev::recv(&mut buf) {
            if len >= 14 {
                eth::handle_frame(&buf[..len]);
            }
        }
        tcp::tick_connections();
        POLLING.store(false, Ordering::Release);
    }
    // ALWAYS run (even if we skipped the drain above): progressive shade render
    // + mouse. Internally gated to Core 0, so only Core 0 executes it — no race
    // despite being outside the guard.
    crate::shade::poll_render();
}

/// Network stack statistics
#[allow(dead_code)]
pub fn is_up() -> bool {
    netdev::is_available()
}
