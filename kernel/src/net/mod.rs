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

/// Acquire the single-consumer NIC-drain guard for the off-vCPU data plane
/// (`net_dataplane`), which pulls host-NIC frames one at a time (`netdev::recv`)
/// and decides per-frame whether to keep pulling — vhost-net backpressure: stop
/// when the guest RX ring is full, so the NIC ring fills and the sender flow-
/// controls, instead of a synthetic staging-queue drop. Returns false if Core 0
/// is mid-`poll()`; the worker then skips this pass (Core 0 already skips the
/// drain while the data plane is `active()`, so this is just the race backstop).
pub fn try_acquire_drain() -> bool {
    POLLING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
}

/// Release the guard taken by [`try_acquire_drain`].
pub fn release_drain() {
    POLLING.store(false, Ordering::Release);
}

// TEMP profiler: split poll_rx_only's per-call cost. TSC cycles in the netdev
// drain (driver + virtio doorbells), the IP/TCP stack (handle_frame), tx_flush,
// and poll_render — plus packets processed. Read+reset via take_poll_prof().
static PROF_NETDEV: AtomicU64 = AtomicU64::new(0);
static PROF_STACK: AtomicU64 = AtomicU64::new(0);
static PROF_TXFLUSH: AtomicU64 = AtomicU64::new(0);
static PROF_RENDER: AtomicU64 = AtomicU64::new(0);
static PROF_PKTS: AtomicU64 = AtomicU64::new(0);

/// (netdev_cyc, stack_cyc, txflush_cyc, render_cyc, packets) since last call; resets.
pub fn take_poll_prof() -> (u64, u64, u64, u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (PROF_NETDEV.swap(0, Relaxed), PROF_STACK.swap(0, Relaxed),
     PROF_TXFLUSH.swap(0, Relaxed), PROF_RENDER.swap(0, Relaxed),
     PROF_PKTS.swap(0, Relaxed))
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
    // Core 0 must not drain the host NIC while a microvm owns it: either the old
    // BSP-pump-as-sole-drainer mode (vm_active) OR the dedicated RX producer
    // fiber (net_dataplane::active). In both cases Core 0 would fill INBOUND_Q it
    // cannot inject, racing the real drainer and overflowing the queue.
    let skip_nic_drain = (crate::microvm::vm_active()
        || crate::microvm::devices::net_dataplane::active())
        && crate::smp::per_core::current_core_id() == 0;
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
    // Give this core's fibers a turn. Every blocking network wait in the kernel
    // — ARP, DNS, ICMP, TCP connect — spins on this function, and every one of
    // them runs as a NATIVE task on a worker core. A WASM NIC driver whose fiber
    // sits on that same core is then frozen for the whole command, so the device
    // is never polled and the answer we are waiting for is sitting in the card's
    // ring. It arrives the moment the command gives up. No-op on Core 0 and
    // inside a fiber.
    crate::smp::fiber::pump_peers();
    // Flush the batched virtio-net TX doorbell once per cycle (send() defers the
    // per-frame notify to avoid a VM-exit per uploaded packet). No-op when no
    // virtio NIC / nothing pending.
    crate::virtio_net::tx_flush();
    // ALWAYS run (even if we skipped the drain above): progressive shade render
    // + mouse + auto-hide dock reveal/tick. Internally gated to Core 0, so only
    // Core 0 executes it — no race despite being outside the guard. This is the
    // heartbeat that drives the dock hover-reveal from the shell-prompt idle
    // loop (read_line_with_tab), which polls net but never renders directly.
    crate::shade::poll_render();
}

/// NIC-drain-only poll for the hot recv busy-spins (`tcp_recv_poll` /
/// `tls_recv_poll`). Drains the RX ring into the IP stack but SKIPS
/// `tcp::tick_connections` (128-slot scan + CONNECTIONS lock) and
/// `shade::poll_render`. Those don't need to run at busy-spin rate (~1 M/s) —
/// Core 0's `poll()` runs the TCP timers. Calling the full `poll()` in the spin
/// burned the RX core on ~1 M CONNECTIONS-lock acquisitions + ~128 M slot-checks
/// per second between chunks, starving the actual packet processing.
pub fn poll_rx_only() {
    use core::sync::atomic::Ordering::Relaxed;
    let rd = crate::interrupts::rdtsc;
    if POLLING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        let mut buf = [0u8; netdev::MTU];
        loop {
            let a = rd();
            let r = netdev::recv(&mut buf);
            let b = rd();
            PROF_NETDEV.fetch_add(b.wrapping_sub(a), Relaxed);
            match r {
                Some(len) => {
                    if len >= 14 { eth::handle_frame(&buf[..len]); }
                    PROF_STACK.fetch_add(rd().wrapping_sub(b), Relaxed);
                    PROF_PKTS.fetch_add(1, Relaxed);
                }
                None => break,
            }
        }
        POLLING.store(false, Ordering::Release);
    }
    // Same reason as in poll(): this spin owns a worker core, and a driver fiber
    // parked on it would never refill the ring this loop is draining.
    crate::smp::fiber::pump_peers();
    let c = rd();
    crate::virtio_net::tx_flush();
    PROF_TXFLUSH.fetch_add(rd().wrapping_sub(c), Relaxed);
    // NO shade::poll_render() here. poll_rx_only runs ONLY on the worker recv
    // loops (tcp_recv_poll / recv_blocking), where poll_render did nothing but
    // its own ~6 µs core-gate (current_core_id = rdmsr + APIC-MMIO = 2 VM-exits)
    // before bailing — ~30 % of poll_rx_only's cost, pure waste. Core 0 renders
    // via its own run-loop / net::poll().
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
static NEXT_DHCP_RETRY: AtomicU64 = AtomicU64::new(0);

/// Interface id plus carrier in one byte — the value the link tick compares
/// against. Seed and tick MUST encode it the same way; seeding the bare id left
/// the carrier bit clear, so the very first tick saw a "change" and re-ran DHCP
/// about a second after every boot.
fn link_state_id() -> u8 {
    netdev::active_id() | if netdev::active_link_up() { 0x10 } else { 0 }
}

/// Seed the active-interface tracker WITHOUT reconfiguring — call once after the
/// boot-time DHCP so the first tick doesn't redundantly re-DHCP the same link.
/// Warms the gateway's MAC too: with the tracker seeded correctly nothing else
/// runs on this path at boot, and a cold ARP cache makes the first DNS query
/// look like a broken resolver.
pub fn seed_active() {
    netdev::refresh_link_state();
    LAST_ACTIVE.store(link_state_id(), Ordering::Relaxed);
    if arp::our_ip() != [0, 0, 0, 0] {
        prime_gateway_arp();
    }
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
    let up = netdev::active_link_up();
    // The trigger has to include the CARRIER, not just which interface is
    // selected. A WiFi NIC counts as active from the moment its driver
    // registers — which is before the association, let alone the 4-way. Keyed
    // on the id alone, the only edge fires while the link is still down: DHCP
    // goes out over a dead interface, gets no offer, and because the id never
    // changes again no further attempt is ever made.
    let state = link_state_id();
    let changed = state != LAST_ACTIVE.swap(state, Ordering::Relaxed);
    if changed && act != 0 && up {
        reconfigure();
        return;
    }
    // Belt and braces: a usable link but still no address means the edge was
    // missed or the lease attempt failed. Keep asking on a slow retry instead
    // of waiting for an edge that has already gone by — this is what a real
    // DHCP client does, and it makes the outcome independent of boot ordering.
    //
    // Backing off matters as much as retrying. A lease attempt BLOCKS Core 0 for
    // its whole timeout, and Core 0 is the terminal: against a link that answers
    // nothing, a fixed 10 s retry took the machine away from its user every few
    // seconds, for as long as the fault lasted. Doubling up to two minutes keeps
    // the recovery and gives the prompt back.
    if up && act != 0 && arp::our_ip() == [0, 0, 0, 0] {
        if now >= NEXT_DHCP_RETRY.load(Ordering::Relaxed) {
            let wait = DHCP_RETRY_S.load(Ordering::Relaxed);
            NEXT_DHCP_RETRY.store(
                now + crate::interrupts::tsc_freq().saturating_mul(wait), Ordering::Relaxed);
            DHCP_RETRY_S.store((wait * 2).min(DHCP_RETRY_MAX_S), Ordering::Relaxed);
            crate::kprintln!("[npk] net: link up but no address - retrying DHCP (next in {} s)",
                DHCP_RETRY_S.load(Ordering::Relaxed));
            reconfigure();
        }
    }
}

/// Seconds until the next unsolicited DHCP attempt, doubled per failure.
static DHCP_RETRY_S: AtomicU64 = AtomicU64::new(DHCP_RETRY_MIN_S);
const DHCP_RETRY_MIN_S: u64 = 10;
const DHCP_RETRY_MAX_S: u64 = 120;

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
    if dhcp::configure() {
        // A link that works again starts the backoff over.
        DHCP_RETRY_S.store(DHCP_RETRY_MIN_S, Ordering::Relaxed);
        // Tell the segment which MAC now owns our address, then warm the
        // gateway entry. Without the announcement the router keeps sending to
        // the interface we just left.
        arp::announce();
        prime_gateway_arp();
    }
}

/// Resolve the gateway's MAC right after a lease instead of leaving it to
/// whatever request happens to go out first. On a link that has just come up
/// that first request is usually a DNS lookup, and it is the one that eats the
/// ARP round trip — or fails outright, which reads as "name resolution is
/// broken" rather than "the ARP cache was cold".
fn prime_gateway_arp() {
    let gw = ipv4::gateway();
    if gw == [0, 0, 0, 0] {
        return;
    }
    if arp::resolve(gw, 50).is_none() { // 100 Hz ticks → 500 ms
        crate::kprintln!("[npk] net: gateway {}.{}.{}.{} did not answer ARP yet",
            gw[0], gw[1], gw[2], gw[3]);
    }
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
