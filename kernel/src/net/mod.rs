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

// TEMP profiler: split poll_rx_only's per-call cost. TSC cycles in the netdev
// drain (driver + virtio doorbells), the IP/TCP stack (handle_frame), tx_flush,
// and poll_render — plus packets processed. Read+reset via take_poll_prof().
static PROF_NETDEV: AtomicU64 = AtomicU64::new(0);
static PROF_STACK: AtomicU64 = AtomicU64::new(0);
static PROF_TXFLUSH: AtomicU64 = AtomicU64::new(0);
static PROF_RENDER: AtomicU64 = AtomicU64::new(0);
static PROF_PKTS: AtomicU64 = AtomicU64::new(0);

/// Frames actually pulled off the host NIC, and passes that pulled NOTHING
/// because another core held the single-drainer guard.
///
/// Without these, "the worker ran 372 times per second" is compatible with 372
/// real drains AND with 372 complete no-ops: `poll_rx_only` returns normally
/// when the CAS fails, and the producer counter is incremented by its CALLER,
/// before the call. That blind spot is the difference between "nothing arrived
/// on the wire" and "we never looked", which is the whole question when the
/// staging queue is empty and the consumer is alive.
static NIC_FRAMES: AtomicU64 = AtomicU64::new(0);
static NIC_SKIPPED: AtomicU64 = AtomicU64::new(0);

/// (frames pulled off the NIC, passes that lost the drain guard). Monotonic.
pub fn nic_drain_stats() -> (u64, u64) {
    (NIC_FRAMES.load(Ordering::Relaxed), NIC_SKIPPED.load(Ordering::Relaxed))
}

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
    // Core 0 drains the host NIC even while a microvm runs. That used to be
    // forbidden because what it pulled in landed in a queue only the vCPU could
    // empty — Core 0 filled it, could not inject, and overflowed it. With the tap
    // there IS an consumer: whatever Core 0 puts in, the data-plane worker takes
    // out. The old guard, on hardware, meant nobody drained the card at all.
    let skip_nic_drain = crate::microvm::vm_active()
        && crate::smp::per_core::current_core_id() == 0;
    if !skip_nic_drain
        && POLLING
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    {
        let mut buf = [0u8; netdev::MTU];
        while let Some(len) = netdev::recv(&mut buf) {
            NIC_FRAMES.fetch_add(1, Ordering::Relaxed);
            if len >= 14 {
                eth::handle_frame(&buf[..len]);
            }
        }
        tcp::tick_connections();
        POLLING.store(false, Ordering::Release);
    } else if !skip_nic_drain {
        NIC_SKIPPED.fetch_add(1, Ordering::Relaxed);
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
                    NIC_FRAMES.fetch_add(1, Ordering::Relaxed);
                    if len >= 14 { eth::handle_frame(&buf[..len]); }
                    PROF_STACK.fetch_add(rd().wrapping_sub(b), Relaxed);
                    PROF_PKTS.fetch_add(1, Relaxed);
                }
                None => break,
            }
        }
        POLLING.store(false, Ordering::Release);
    } else {
        NIC_SKIPPED.fetch_add(1, Ordering::Relaxed);
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
        netdev::note_rx_available(1);
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
/// How long a carrier has to stay down before it counts as a link change.
/// Covers a reconnect (scan + auth + assoc + 4-way) that succeeds — the address
/// and the gateway are unchanged across it, so there is nothing to reconfigure.
const LINK_DOWN_GRACE_S: u64 = 5;
/// TSC at which the current carrier-down started, 0 = carrier is up.
static DOWN_SINCE: AtomicU64 = AtomicU64::new(0);

/// Interface name for a previously-recorded `link_state_id` (the live one comes
/// from `netdev::active_name`).
fn iface_name(id: u8) -> &'static str {
    match id {
        1 => "intel",
        2 => "usb-lan",
        3 => "wifi",
        4 => "virtio",
        _ => "none",
    }
}

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
    // Before the throttle: a lease in flight is stepped on EVERY pass, not once
    // a second. It is a `udp::recv` peek and a deadline compare, and nothing at
    // all when no exchange is running.
    dhcp::tick();
    // Also before the throttle: one queued name per pass. The microvm data plane
    // may not block on a resolver (see `dns::cached`), so it queues the name and
    // Core 0 does the waiting — here, where no driver fiber sits behind it.
    dns::pump_wanted();
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
    // Linux does NOT act on a carrier edge directly. `link_watch.c` queues the
    // event and runs its worker at most once a second — with one asymmetry:
    //
    //     /* Minimise down-time: drop delay for up event. */
    //
    // UP is urgent, DOWN is delayed, and a link that returns inside the window
    // produces no event at all. We treated both directions alike at a 1 Hz
    // sample, so ONE sample of a down carrier — a reconnect that succeeded
    // three seconds later, nothing more — cost a full DHCP round, and the
    // address went with it. Observed as `link changed -> requesting DHCP` out
    // of nowhere, after which nothing worked.
    let state = link_state_id();
    let prev = LAST_ACTIVE.load(Ordering::Relaxed);
    if state != prev {
        let iface_changed = (state & 0x0F) != (prev & 0x0F);
        let now_up = state & 0x10 != 0;
        if now_up || iface_changed {
            // Urgent: coming up, or a genuinely different interface.
            LAST_ACTIVE.store(state, Ordering::Relaxed);
            DOWN_SINCE.store(0, Ordering::Relaxed);
            crate::kprintln!("[npk] net: link {} ({}) -> {} ({})",
                iface_name(prev & 0x0F), if prev & 0x10 != 0 { "up" } else { "down" },
                netdev::active_name(), if now_up { "up" } else { "down" });
            if act != 0 && up {
                reconfigure();
                return;
            }
        } else {
            // Carrier lost on the SAME interface. Let it prove itself: while
            // LAST_ACTIVE still says "up", a return needs no event at all.
            let since = DOWN_SINCE.load(Ordering::Relaxed);
            if since == 0 {
                DOWN_SINCE.store(now, Ordering::Relaxed);
            } else if now.saturating_sub(since)
                >= crate::interrupts::tsc_freq().saturating_mul(LINK_DOWN_GRACE_S)
            {
                LAST_ACTIVE.store(state, Ordering::Relaxed);
                DOWN_SINCE.store(0, Ordering::Relaxed);
                crate::kprintln!("[npk] net: link {} down for {} s — giving up on it",
                    netdev::active_name(), LINK_DOWN_GRACE_S);
            }
        }
    } else {
        DOWN_SINCE.store(0, Ordering::Relaxed);
    }
    // Belt and braces: a usable link but still no address means the edge was
    // missed or the lease attempt failed. Keep asking on a slow retry instead
    // of waiting for an edge that has already gone by — this is what a real
    // DHCP client does, and it makes the outcome independent of boot ordering.
    //
    // The backoff no longer buys back a frozen terminal (the exchange is
    // asynchronous now); it is there so a link that answers nothing is not
    // broadcast at every second for as long as the fault lasts.
    if up && act != 0 {
        if arp::our_ip() == [0, 0, 0, 0] {
            if !dhcp::is_running() && now >= NEXT_DHCP_RETRY.load(Ordering::Relaxed) {
                let wait = DHCP_RETRY_S.load(Ordering::Relaxed);
                NEXT_DHCP_RETRY.store(
                    now + crate::interrupts::tsc_freq().saturating_mul(wait), Ordering::Relaxed);
                DHCP_RETRY_S.store((wait * 2).min(DHCP_RETRY_MAX_S), Ordering::Relaxed);
                crate::kprintln!("[npk] net: link up but no address - retrying DHCP (next in {} s)",
                    DHCP_RETRY_S.load(Ordering::Relaxed));
                reconfigure();
            }
        } else {
            // An address arrived — the lease may have landed a second or a
            // minute after the call that asked for it, so the reset belongs
            // here rather than at a return value nobody waits for any more.
            DHCP_RETRY_S.store(DHCP_RETRY_MIN_S, Ordering::Relaxed);
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
    // Returns at once. The ARP announcement and the gateway warm-up moved into
    // the exchange itself (dhcp::succeed) — they belong where the lease lands,
    // which is no longer here.
    if let dhcp::Start::Kept = dhcp::start() {
        DHCP_RETRY_S.store(DHCP_RETRY_MIN_S, Ordering::Relaxed);
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
