//! Network Stack
//!
//! Capability-gated TCP/IP implementation.
//! Layers: Ethernet → ARP → IPv4 → ICMP/UDP/TCP
//! Every connection requires a capability token.

pub mod eth;
pub mod fq_codel;
pub mod arp;
pub mod ipv4;
pub mod icmp;
pub mod udp;
pub mod dns;
pub mod dhcp;
pub mod ntp;
pub mod tcp;

use crate::netdev;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

/// Serialises NIC RX-ring access across cores. With guest-SMP, Core 0's loop
/// AND the BSP vCPU pump both call `poll()`. `netdev::recv` drains a
/// single-consumer ring; two cores draining it concurrently race → reordered /
/// dropped frames → the guest's TCP collapses (observed: speedtest stalled at
/// high throughput once the pump stopped idle-parking and polled continuously).
/// Best practice for a shared polled device: one drainer at a time. This is a
/// NON-blocking guard — whoever holds it does the work, the other core skips
/// (its data is drained by the holder + picked up next pass).
static POLLING: AtomicBool = AtomicBool::new(false);

/// Force-clear the NIC-drain guard. Called on microvm teardown so a stuck
/// guard (e.g. a vCPU fiber that held it when something went wrong) can never
/// brick the HOST's own networking — without this, a stuck `POLLING=true` makes
/// every host `net::poll()` skip the NIC drain forever → host DNS / OTA dead
/// until reboot. Safe to call any time (the microvm fiber is gone by teardown).
pub fn reset_poll_guard() {
    POLLING.store(false, Ordering::Release);
}

/// Process incoming packets and TCP timers.
pub fn poll() {
    // The guard wraps ONLY the single-consumer NIC drain + host-TCP tick
    // (cross-core mutually exclusive). It must NOT cover the compositor below:
    // the BSP vCPU pump holds this guard while it spin-pumps under network
    // load, so if Core 0's poll() returned early here it would never render or
    // poll the mouse → UI + mouse freeze (observed as a notebook kernel freeze:
    // a slow NIC keeps the BSP pumping ~continuously → Core 0 fully starved).
    // While a microvm is active, Core 0 must NOT drain the host NIC: it pulls
    // guest-bound (slirp) packets into the NAT's INBOUND_Q but CANNOT inject them
    // (no VmContext) — only the BSP vCPU pump can. Core 0 winning the drain race
    // outran the pump, overflowing INBOUND_Q → packet drops EVEN THOUGH the guest
    // always had RX buffers (measured: injfalse=0 yet ~49k drops). So the BSP
    // pump (a worker core, calls net::poll() itself) becomes the sole NIC drainer:
    // fill + inject happen together, no race, no drops, throughput = pump rate.
    let skip_nic_drain =
        crate::microvm::vm_active() && crate::smp::per_core::current_core_id() == 0;
    if !skip_nic_drain
        && POLLING
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
    // Flush the batched virtio-net TX doorbell once per cycle (send() defers the
    // per-frame notify to avoid a VM-exit per uploaded packet). No-op when no
    // virtio NIC / nothing pending.
    crate::virtio_net::tx_flush();
    // ALWAYS run (even if we skipped the drain above): progressive shade render
    // + mouse. Internally gated to Core 0, so only Core 0 executes it — no race
    // despite being outside the guard.
    crate::shade::poll_render();
}

/// A WASM NIC driver delivers a received Ethernet frame straight into the IP
/// stack from its own (worker-core) fiber context — the Linux NAPI topology:
/// drain → stack in one context, no relay-ring + Core-0 hop (that double poll
/// was the WiFi latency/throughput bottleneck). Uses the single-drainer POLLING
/// guard so it never races Core 0's net::poll(). If Core 0 is mid-drain we can't
/// take the guard → spill to the fallback ring (Core 0 picks it up next pass),
/// never dropping. Any frames already spilled there are flushed first so order
/// is preserved.
pub fn wasm_deliver_rx(frame: &[u8]) {
    if frame.len() < 14 {
        return;
    }
    if POLLING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        let mut buf = [0u8; netdev::MTU];
        while let Some(len) = netdev::wasm_nic_poll_rx(&mut buf) {
            if len >= 14 {
                eth::handle_frame(&buf[..len]);
            }
        }
        eth::handle_frame(frame);
        POLLING.store(false, Ordering::Release);
    } else {
        netdev::wasm_nic_submit_rx(frame);
    }
}

/// Tracks the active interface so a change (cable pulled/plugged, WiFi
/// associated) triggers a fresh IP config. 0xff = not yet seeded.
static LAST_ACTIVE: AtomicU8 = AtomicU8::new(0xff);
static NEXT_LINK_CHECK: AtomicU64 = AtomicU64::new(0);

/// Seed the active-interface tracker WITHOUT reconfiguring — call once after the
/// boot-time DHCP so the first tick doesn't redundantly re-DHCP the same link.
pub fn seed_active() {
    netdev::refresh_link_state();
    LAST_ACTIVE.store(netdev::active_id(), Ordering::Relaxed);
}

/// Core 0, ~1 Hz: refresh the wired carrier cache, and when the active interface
/// changes, reconfigure IP — a static config if set, else DHCP. Replaces the
/// boot-only one-shot + the manual `dhcp`: pull the LAN cable and WiFi takes
/// over with a fresh lease automatically. Must NOT run in IRQ context (it does
/// USB reads + DHCP can block); call from the Core 0 shell loop.
pub fn tick_link_and_reconfigure() {
    if crate::smp::per_core::current_core_id() != 0 { return; }
    let now = crate::interrupts::rdtsc();
    if now < NEXT_LINK_CHECK.load(Ordering::Relaxed) { return; }
    NEXT_LINK_CHECK.store(now + crate::interrupts::tsc_freq(), Ordering::Relaxed); // +~1 s

    netdev::refresh_link_state();
    let act = netdev::active_id();
    if act != LAST_ACTIVE.swap(act, Ordering::Relaxed) && act != 0 {
        reconfigure();
    }
}

/// Apply a static IP config if `static_ip` is set, else run DHCP. Sets gateway
/// (`static_gw`) + DNS (`static_dns`) when given.
fn reconfigure() {
    if let Some(ip) = crate::config::get("static_ip").and_then(|s| parse_ipv4(s.trim())) {
        arp::set_ip(ip);
        if let Some(gw) = crate::config::get("static_gw").and_then(|s| parse_ipv4(s.trim())) {
            ipv4::set_gateway(gw);
        }
        if let Some(d) = crate::config::get("static_dns").and_then(|s| parse_ipv4(s.trim())) {
            dns::set_server(d);
        }
        crate::kprintln!("[npk] net: static IP {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        return;
    }
    crate::kprintln!("[npk] net: link changed -> requesting DHCP lease...");
    let _ = dhcp::configure();
}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut it = s.split('.');
    let a = it.next()?.parse::<u8>().ok()?;
    let b = it.next()?.parse::<u8>().ok()?;
    let c = it.next()?.parse::<u8>().ok()?;
    let d = it.next()?.parse::<u8>().ok()?;
    if it.next().is_some() { return None; }
    Some([a, b, c, d])
}

/// Network stack statistics
#[allow(dead_code)]
pub fn is_up() -> bool {
    netdev::is_available()
}
