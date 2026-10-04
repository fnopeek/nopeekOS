# `kernel/src/net/mod.rs` @ 5e0102684

## L1-5 · `pub mod eth;`

```
//! Network Stack
//!
//! Capability-gated TCP/IP implementation.
//! Layers: Ethernet → ARP → IPv4 → ICMP/UDP/TCP
//! Every connection requires a capability token.
```

## L22-29 · `static POLLING: AtomicBool = AtomicBool::new(false);`

```
/// Serialises NIC RX-ring access across cores. With guest-SMP, Core 0's loop
/// AND the BSP vCPU pump both call `poll()`. `netdev::recv` drains a
/// single-consumer ring; two cores draining it concurrently race → reordered /
/// dropped frames → the guest's TCP collapses (observed: speedtest stalled at
/// high throughput once the pump stopped idle-parking and polled continuously).
/// Best practice for a shared polled device: one drainer at a time. This is a
/// NON-blocking guard — whoever holds it does the work, the other core skips
/// (its data is drained by the holder + picked up next pass).
```

## L32-36 · `pub fn reset_poll_guard() {`

```
/// Force-clear the NIC-drain guard. Called on microvm teardown so a stuck
/// guard (e.g. a vCPU fiber that held it when something went wrong) can never
/// brick the HOST's own networking — without this, a stuck `POLLING=true` makes
/// every host `net::poll()` skip the NIC drain forever → host DNS / OTA dead
/// until reboot. Safe to call any time (the microvm fiber is gone by teardown).
```

## L41-43 · `static PROF_NETDEV: AtomicU64 = AtomicU64::new(0);`

```
// TEMP profiler: split poll_rx_only's per-call cost. TSC cycles in the netdev
// drain (driver + virtio doorbells), the IP/TCP stack (handle_frame), tx_flush,
// and poll_render — plus packets processed. Read+reset via take_poll_prof().
```

## L50-58 · `static NIC_FRAMES: AtomicU64 = AtomicU64::new(0);`

```
/// Frames actually pulled off the host NIC, and passes that pulled NOTHING
/// because another core held the single-drainer guard.
///
/// Without these, "the worker ran 372 times per second" is compatible with 372
/// real drains AND with 372 complete no-ops: `poll_rx_only` returns normally
/// when the CAS fails, and the producer counter is incremented by its CALLER,
/// before the call. That blind spot is the difference between "nothing arrived
/// on the wire" and "we never looked", which is the whole question when the
/// staging queue is empty and the consumer is alive.
```

## L62 · `pub fn nic_drain_stats() -> (u64, u64) {`

```
/// (frames pulled off the NIC, passes that lost the drain guard). Monotonic.
```

## L67 · `pub fn take_poll_prof() -> (u64, u64, u64, u64, u64) {`

```
/// (netdev_cyc, stack_cyc, txflush_cyc, render_cyc, packets) since last call; resets.
```

## L75 · `pub fn poll() {`

```
/// Process incoming packets and TCP timers.
```

## L77-87 · `let skip_nic_drain = crate::microvm::vm_active()`

```
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
```

## L107-113 · `crate::smp::fiber::pump_peers();`

```
// Give this core's fibers a turn. Every blocking network wait in the kernel
// — ARP, DNS, ICMP, TCP connect — spins on this function, and every one of
// them runs as a NATIVE task on a worker core. A WASM NIC driver whose fiber
// sits on that same core is then frozen for the whole command, so the device
// is never polled and the answer we are waiting for is sitting in the card's
// ring. It arrives the moment the command gives up. No-op on Core 0 and
// inside a fiber.
```

## L115-117 · `crate::virtio_net::tx_flush();`

```
// Flush the batched virtio-net TX doorbell once per cycle (send() defers the
// per-frame notify to avoid a VM-exit per uploaded packet). No-op when no
// virtio NIC / nothing pending.
```

## L119-123 · `crate::shade::poll_render();`

```
// ALWAYS run (even if we skipped the drain above): progressive shade render
// + mouse + auto-hide dock reveal/tick. Internally gated to Core 0, so only
// Core 0 executes it — no race despite being outside the guard. This is the
// heartbeat that drives the dock hover-reveal from the shell-prompt idle
// loop (read_line_with_tab), which polls net but never renders directly.
```

## L127-134 · `static LAST_TCP_TICK: core::sync::atomic::AtomicU64 =`

```
/// NIC-drain-only poll for the hot recv busy-spins (`tcp_recv_poll` /
/// `tls_recv_poll`). Drains the RX ring into the IP stack but SKIPS
/// `tcp::tick_connections` (128-slot scan + CONNECTIONS lock) and
/// `shade::poll_render`. Those don't need to run at busy-spin rate (~1 M/s) —
/// Core 0's `poll()` runs the TCP timers. Calling the full `poll()` in the spin
/// burned the RX core on ~1 M CONNECTIONS-lock acquisitions + ~128 M slot-checks
/// per second between chunks, starving the actual packet processing.
/// Der Takt, auf dem `poll_rx_only` zuletzt den TCP-Zeitgeber gefahren hat.
```

## L165-190 · `{`

```
// **Der TCP-Zeitgeber, gedrosselt auf den 100-Hz-Takt.**
//
// Hier stand, `tick_connections` sei ueberfluessig, weil „Core 0's
// poll() runs the TCP timers". Das stimmt am PROMPT und nur dort:
// `net::poll()` wird aus `read_line_with_tab` gerufen. Waehrend ein
// Befehl laeuft — also genau waehrend eines Downloads — ruft es
// niemand. In dieser Zeit gab es KEINEN Zeitgeber:
//
//   * keine verzoegerte Quittung. Quittiert wurde nur ueber
//     ACK_COALESCE (jedes achte Segment in Reihenfolge).
//   * KEINE WIEDERHOLUNG. Ein verlorenes Segment kam nie nach.
//   * keine Fensteraktualisierung ausser ueber recv().
//
// Bei hohem Durchsatz faellt das nicht auf: acht Segmente treffen in
// Mikrosekunden ein, der Zaehler feuert, der Zeitgeber wird nie
// gebraucht. Die Luecke ist latent seit Juni und schlaegt zu, sobald
// IRGENDETWAS Verluste einfuehrt — dann gibt es keinen Weg zurueck.
//
// Gemessen am Geraet (2026-09-21, USB-Dongle): 5264 Pakete in 63,6 s
// = 83/s, dazu 871 gesendete Rahmen = 13,7/s. 83/8 ist genau die
// Quittungsrate — der Achter-Zaehler allein, ohne jeden Zeitgeber.
//
// Der Deckel ist der Grund, warum es hier ueberhaupt stehen darf:
// `tick_connections` nimmt das Verbindungsschloss und geht 128
// Plaetze durch. Bei einer Million Runden je Sekunde waere das der
// Ruin — einmal je Takt ist es nichts.
```

## L198-199 · `crate::smp::fiber::pump_peers();`

```
// Same reason as in poll(): this spin owns a worker core, and a driver fiber
// parked on it would never refill the ring this loop is draining.
```

## L204-208 · `}`

```
// NO shade::poll_render() here. poll_rx_only runs ONLY on the worker recv
// loops (tcp_recv_poll / recv_blocking), where poll_render did nothing but
// its own ~6 µs core-gate (current_core_id = rdmsr + APIC-MMIO = 2 VM-exits)
// before bailing — ~30 % of poll_rx_only's cost, pure waste. Core 0 renders
// via its own run-loop / net::poll().
```

## L211-218 · `pub fn wasm_deliver_rx(frame: &[u8]) {`

```
/// A WASM NIC driver delivers a received Ethernet frame straight into the IP
/// stack from its own (worker-core) fiber context — the Linux NAPI topology:
/// drain → stack in one context, no relay-ring + Core-0 hop (that double poll
/// was the WiFi latency/throughput bottleneck). Uses the single-drainer POLLING
/// guard so it never races Core 0's net::poll(). If Core 0 is mid-drain we can't
/// take the guard → spill to the fallback ring (Core 0 picks it up next pass),
/// never dropping. Any frames already spilled there are flushed first so order
/// is preserved.
```

## L238-239 · `crate::intent::wake_shell();`

```
// Core 0 drains this ring in its loop, which no longer runs every
// 10 ms (stage 3e): wake it.
```

## L244-245 · `static LAST_ACTIVE: AtomicU8 = AtomicU8::new(0xff);`

```
/// Tracks the active interface so a change (cable pulled/plugged, WiFi
/// associated) triggers a fresh IP config. 0xff = not yet seeded.
```

## L250-256 · `const LINK_DOWN_GRACE_S: u64 = 5;`

```
/// Interface id plus carrier in one byte — the value the link tick compares
/// against. Seed and tick MUST encode it the same way; seeding the bare id left
/// the carrier bit clear, so the very first tick saw a "change" and re-ran DHCP
/// about a second after every boot.
/// How long a carrier has to stay down before it counts as a link change.
/// Covers a reconnect (scan + auth + assoc + 4-way) that succeeds — the address
/// and the gateway are unchanged across it, so there is nothing to reconfigure.
```

## L258 · `static DOWN_SINCE: AtomicU64 = AtomicU64::new(0);`

```
/// TSC at which the current carrier-down started, 0 = carrier is up.
```

## L261-262 · `fn iface_name(id: u8) -> &'static str {`

```
/// Interface name for a previously-recorded `link_state_id` (the live one comes
/// from `netdev::active_name`).
```

## L277-281 · `pub fn seed_active() {`

```
/// Seed the active-interface tracker WITHOUT reconfiguring — call once after the
/// boot-time DHCP so the first tick doesn't redundantly re-DHCP the same link.
/// Warms the gateway's MAC too: with the tracker seeded correctly nothing else
/// runs on this path at boot, and a cold ARP cache makes the first DNS query
/// look like a broken resolver.
```

## L290-298 · `pub fn needs_tick() -> bool {`

```
/// Core 0, ~1 Hz: refresh the wired carrier cache, and when the active interface
/// changes, reconfigure IP — a static config if set, else DHCP. Replaces the
/// boot-only one-shot + the manual `dhcp`: pull the LAN cable and WiFi takes
/// over with a fresh lease automatically. Must NOT run in IRQ context (it does
/// USB reads + DHCP can block); call from the Core 0 shell loop.
/// Does the network need Core 0's loop within the next frame? While a TCP
/// connection runs a timer, a DHCP exchange is in flight, or the active card
/// is drained only by polling (`net::poll`) — every card but the WASM NIC,
/// whose driver fiber delivers frames itself (stage 3e).
```

## L302-303 · `|| (!matches!(netdev::active(), netdev::Active::Wasm | netdev::Active::None)`

```
// A polled card needs Core 0's cadence; one with an RX interrupt is
// drained by the NAPI fiber.
```

## L310-312 · `dhcp::tick();`

```
// Before the throttle: a lease in flight is stepped on EVERY pass, not once
// a second. It is a `udp::recv` peek and a deadline compare, and nothing at
// all when no exchange is running.
```

## L314-315 · `dns::pump_wanted();`

```
// Also before the throttle: one queued name per pass — only on a machine
// without workers. Otherwise `dns::want` hands the queue to a worker task.
```

## L319 · `NEXT_LINK_CHECK.store(now + crate::interrupts::tsc_freq(), Ordering::Relaxed); // +~1 s`

```
// +~1 s
```

## L324-340 · `let state = link_state_id();`

```
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
```

## L347 · `LAST_ACTIVE.store(state, Ordering::Relaxed);`

```
// Urgent: coming up, or a genuinely different interface.
```

## L358-359 · `let since = DOWN_SINCE.load(Ordering::Relaxed);`

```
// Carrier lost on the SAME interface. Let it prove itself: while
// LAST_ACTIVE still says "up", a return needs no event at all.
```

## L375-382 · `if up && act != 0 {`

```
// Belt and braces: a usable link but still no address means the edge was
// missed or the lease attempt failed. Keep asking on a slow retry instead
// of waiting for an edge that has already gone by — this is what a real
// DHCP client does, and it makes the outcome independent of boot ordering.
//
// The backoff no longer buys back a frozen terminal (the exchange is
// asynchronous now); it is there so a link that answers nothing is not
// broadcast at every second for as long as the fault lasts.
```

## L395-397 · `DHCP_RETRY_S.store(DHCP_RETRY_MIN_S, Ordering::Relaxed);`

```
// An address arrived — the lease may have landed a second or a
// minute after the call that asked for it, so the reset belongs
// here rather than at a return value nobody waits for any more.
```

## L403 · `static DHCP_RETRY_S: AtomicU64 = AtomicU64::new(DHCP_RETRY_MIN_S);`

```
/// Seconds until the next unsolicited DHCP attempt, doubled per failure.
```

## L408-409 · `fn reconfigure() {`

```
/// Apply a static IP config if `static_ip` is set, else run DHCP. Sets gateway
/// (`static_gw`) + DNS (`static_dns`) when given.
```

## L423-425 · `if let dhcp::Start::Kept = dhcp::start() {`

```
// Returns at once. The ARP announcement and the gateway warm-up moved into
// the exchange itself (dhcp::succeed) — they belong where the lease lands,
// which is no longer here.
```

## L431-435 · `fn prime_gateway_arp() {`

```
/// Resolve the gateway's MAC right after a lease instead of leaving it to
/// whatever request happens to go out first. On a link that has just come up
/// that first request is usually a DNS lookup, and it is the one that eats the
/// ARP round trip — or fails outright, which reads as "name resolution is
/// broken" rather than "the ARP cache was cold".
```

## L441 · `if arp::resolve(gw, 50).is_none() { // 100 Hz ticks → 500 ms`

```
// 100 Hz ticks → 500 ms
```

## L457 · `#[allow(dead_code)]`

```
/// Network stack statistics
```

