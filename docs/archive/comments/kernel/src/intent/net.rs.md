# `kernel/src/intent/net.rs` @ 5e0102684

## L1 · `use crate::kprintln;`

```
//! Network intents: ping, traceroute, netstat, resolve, net info, wlan
```

## L9-11 · `let count: u16 = it.next().and_then(|s| s.parse().ok()).unwrap_or(4).clamp(1, 32);`

```
// One probe is a coin flip on a radio link — the same lesson ARP taught us
// today. A single lost echo said "the host is down" when the host was fine,
// so the default is four and the summary says how many came back.
```

## L33-35 · `let hop = crate::net::ipv4::arp_target_for(ip);`

```
// Resolve the next hop properly instead of firing a blind request at a
// hardcoded QEMU address and spinning 100 000 times: that helped only under
// QEMU and cost 100 ms everywhere else.
```

## L47 · `let _ = crate::net::icmp::ping_received(); // clear any stale flag`

```
// clear any stale flag
```

## L51 · `while crate::interrupts::ticks().wrapping_sub(t0) < 100 { // 1 s per probe`

```
// 1 s per probe
```

## L65 · `if seq < count {`

```
// Space the probes out; back to back they share one fate on a bad link.
```

## L100 · `crate::net::arp::request([10, 0, 2, 2]);`

```
// ARP resolve gateway
```

## L122 · `return; // reached destination`

```
// reached destination
```

## L124 · `if crate::interrupts::ticks() - t0 > 100 { // 1s per hop`

```
// 1s per hop
```

## L152-166 · `fn bridge_report() {`

```
/// The microVM's side of the wire. Prints itself only when the bridge has
/// something to say — not gated on `vm_active()`, which despite the name means
/// "a VM is running on the COOPERATIVE Core-0 path" and is therefore always
/// false for the fiber-mode guest this report exists for. The counters are their
/// own gate: they are zeroed at VM teardown, so a host without a guest stays
/// silent, and a guest whose network just died still answers.
///
/// Read it as a decision tree, top to bottom. `guest -> host` still climbing
/// says the guest is alive and the masquerade is taking its packets; if
/// `host -> guest` has stopped with it, the replies are not coming back or the
/// mapping no longer matches. If BOTH climb and the guest still sees nothing,
/// the loss is in delivery, and the three `lost` numbers say which wall: a full
/// staging queue is backpressure, a full table means no new connection can open
/// at all, and an egress refusal means the frame never reached the wire. Call it
/// twice a few seconds apart — the per-second figures come from the gap.
```

## L175-177 · `kprintln!("  microVM bridge — guest up {} s, {} frames from it",`

```
// Say what is true and no more. `tx_pkts` counts only MASQUERADED
// egress, so gating on it called a guest that had sent nothing but ARP
// "silent". `frames_in` is counted at the door, before classification.
```

## L194-196 · `match b.worker_core {`

```
// The identity line. QEMU, the NUC and the notebook must print the SAME
// path id: for months they did not, and no number gathered on one of them
// said anything about the others.
```

## L230-233 · `kprintln!("  lost          {} tap-full(backpressure)   {} TABLE-FULL   {} egress-refused",`

```
// Three walls, never one number. A full tap is BACKPRESSURE and healthy in
// moderation; a full table means no new connection can open at all; an
// egress refusal means the frame never reached the wire. From outside all
// three look like "throughput sagged".
```

## L238-242 · `if b.window_ms > 0 {`

```
// The guest's own heartbeat. A guest whose jiffies crawl loses its TCP
// timers, its NAPI and its workqueues — and looks perfectly alive while
// doing it. Compare against the ~1000/s it programmed. A rate needs two
// readings: on the first call these are "not measured yet", not "zero",
// and printing 0 reads as a stopped guest clock. Say which it is.
```

## L248-253 · `if b.window_ms > 0 {`

```
// The vCPU that pumps this bridge is the SAME fiber that copies the guest's
// framebuffer, ~8 MB a frame, inline on its MMIO exit — on Intel, where the
// net has no off-vCPU worker to fall back on. Printed next to `rx wait` on
// purpose: a browser that starts painting and a delivery latency that goes
// to milliseconds in the same breath is the whole story, and neither number
// says it alone.
```

## L261-263 · `let (nic_frames, nic_skipped) = crate::net::nic_drain_stats();`

```
// The half of the evidence that was missing: did anything arrive on the
// WIRE at all? An empty staging queue with a live consumer means either
// nothing came in, or nobody looked. These two separate that.
```

## L274-291 · `const WIFI_CFG: &str = "sys/config/wifi";`

```
/// `wlan` — one screen with everything needed to diagnose the WiFi link.
///
/// Two halves that must be read together: what the KERNEL sees of the WASM NIC
/// (queues, drops, active interface) and what the DRIVER reports about the air
/// (rates, retries, airtime). A link that is slow because the negotiated rate is
/// legacy looks nothing like one that is slow because the TX queue keeps
/// overflowing, and only both halves side by side tell them apart.
///
/// `wlan reset` zeroes the kernel counters so a single speed test can be
/// measured on its own; the driver's counters are cumulative and it reports
/// throughput over its own 1-second window regardless.
// ── `wlan set` — one file, one key at a time ─────────────────────────────
//
// The wifi settings live in a single `key: value` object, but `store` REPLACES
// what it writes: typing a second key from the loop dropped the first, and no
// app may write this file (a module only gets `sys/config/<its own name>`).
// That left the file creatable and not editable. So the read-modify-write lives
// here, where the capability already exists.
```

## L295-296 · `const WIFI_KEYS: &[&str] = &["ssid", "band", "bw", "ampdu", "txagg", "roam", "ht40", "vht", "bawin", "ps", "btcoex", "se`

```
/// The keys the wifi stack actually reads. A typo that wrote silently is how an
/// afternoon gets spent measuring a setting that never arrived.
```

## L330 · `let (key, value) = match rest.split_once(' ') {`

```
// Only the first space splits: an SSID may contain them.
```

## L415-419 · `let (no_link, tx_err) = crate::netdev::tx_reject_stats();`

```
// Frames the TX path REFUSED, as opposed to sent and unanswered. The
// counters existed since the DNS hunt that motivated them and were never
// printed, so a SYN that never reached the air still looked exactly like a
// SYN the peer ignored — which is the question `connect timeout: state
// SynSent` leaves open.
```

## L434-436 · `let ip = crate::net::arp::our_ip();`

```
// Whether there is an address at all. "WiFi is up but there is no DHCP
// lease" and "the link never actually came up" look identical from a
// terminal, and they need opposite fixes.
```

## L444-446 · `let c = crate::wifi::stats();`

```
// The control channel between driver and supplicant. A dropped message here
// is a dropped 4-way handshake step, and events piling up means wifid has
// stopped reading — the usual reason a link associates but never authorizes.
```

## L453-456 · `kprintln!("             (no reply yet — expected while the AP has not sent EAPOL msg1)");`

```
// Only meaningful once the driver has actually handed over something
// that needs an answer. A lone READY with no reply means the AP never
// started the handshake — the supplicant has nothing to answer yet, and
// blaming it here sent the last diagnosis down the wrong path.
```

## L460-461 · `let mut any = false;`

```
// The driver half. Printed verbatim: the kernel does not know what an
// AX200 rate code means, and should not have to.
```

## L482-484 · `kprintln!();`

```
// wifid runs in an invisible autostart window, so its log is the only place
// the supplicant's side of the handshake is visible at all. Show the tail —
// the interesting lines ("missed READY", "4-way FAILED", "Idle") are there.
```

## L543-544 · `kprintln!("    State   {}", if iface.link_up { "UP" } else { "DOWN" });`

```
// State reflects the real carrier (WiFi: associated + keyed; wired:
// present), not whether this is the primary interface.
```

## L563-566 · `let table = crate::net::arp::table();`

```
// The next-hop table. A wrong MAC here is invisible from the outside — it
// looks exactly like the far end being down, and it takes out everything
// that leaves the segment while LAN-direct traffic keeps working. The
// gateway's row is the one to check first, so it is marked.
```

