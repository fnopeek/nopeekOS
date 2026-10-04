# `kernel/src/drivers/netdev.rs` @ 5e0102684

## L1-3 · `use crate::{virtio_net, intel_nic, rtl8153};`

```
//! Network Device Abstraction
//!
//! Dispatches to Intel NIC, WASM driver NIC, or virtio-net (in that order).
```

## L12 · `static WASM_NIC_ACTIVE: AtomicBool = AtomicBool::new(false);`

```
// ── WASM-backed NIC (registered by WASM driver modules) ──
```

## L16-24 · `static WASM_NIC_RX: Mutex<Ring<RX_RING>> = Mutex::new(Ring::new());`

```
/// The spill ring lives in its OWN static, and that is not cosmetic: a static
/// goes into `.bss` only if it is entirely zero, and the granularity is the
/// whole symbol. Sharing one with `FqCodel` — whose `link: [EMPTY; CAP]` is
/// `[0xFFFF; 64]` — put 128 non-zero bytes next to three quarters of a
/// megabyte of zeros and wrote all of it into the kernel image. Measured:
/// `.data` 351 672 -> 1 030 840, `.bss` unchanged, kernel.efi +663 KB — on an
/// image that ships over the very WiFi link it was meant to fix.
/// Never nest this lock inside another; `register_wasm_nic` is the one place
/// that holds both, and it takes WASM_NIC first.
```

## L27-32 · `struct Ring<const N: usize> {`

```
// Frame ring between the kernel net stack and a WASM NIC driver. Unlike a
// single-slot mailbox (which overwrites — and so DROPS — an undrained frame on
// the next submit), this absorbs bursts: the producer drops only when the ring
// is genuinely full, never clobbering a frame already queued. One slot is kept
// empty to distinguish full from empty. All access is under the WASM_NIC_RX
// lock, so plain indices suffice (no atomics needed).
```

## L36 · `head: usize, // consumer reads here`

```
// consumer reads here
```

## L37 · `tail: usize, // producer writes here`

```
// producer writes here
```

## L44 · `fn push(&mut self, frame: &[u8]) -> bool {`

```
/// Enqueue a frame. Returns false (frame dropped) only if the ring is full.
```

## L54 · `fn pop(&mut self, out: &mut [u8; MTU]) -> Option<usize> {`

```
/// Dequeue the oldest frame into `out`. None if empty.
```

## L65-79 · `const RX_RING: usize = 512;`

```
// RX is a FALLBACK: the driver normally delivers each frame straight into the IP
// stack from its own fiber (net::wasm_deliver_rx, the NAPI topology) and only
// spills to this ring when Core 0 holds the drain guard.
//
// 64 was sized for "occasionally, briefly". Measured on the device at ~100
// Mbit: `rx ring in 798365 dropped 1508 (ring full — driver outran core-0
// drain)`, while the DRIVER's own pool reported `pool-exhausted 0` — so the
// frames survived the radio, survived the card, and were thrown away here.
// Every one of them is a retransmission the sender then has to make, which is
// what kept the congestion window small all evening.
//
// 512 entries ≈ 775 KB — of BSS, now that the ring has its own all-zero
// static. The first attempt at this number landed in `.data` and put 663 KB
// into every OTA kernel download. The guard is held for the length of one
// Core-0 drain, and at 100 Mbit 64 frames is under a millisecond of cover.
```

## L84-86 · `tx: crate::net::fq_codel::FqCodel,`

```
/// Frames the kernel queued for the driver to transmit. fq_codel (Linux
/// "Make WiFi Fast"): per-flow fair queueing + CoDel AQM so a ping isn't
/// stuck behind a bulk backlog and stale packets are dropped, not delayed.
```

## L88-90 · `link_up: bool,`

```
/// Carrier/link state, set by the driver via npk_netdev_set_link. For a
/// WiFi NIC this is "associated + keyed" (data path live), distinct from
/// mere registration.
```

## L104-108 · `static WASM_NIC_CORE: AtomicU32 = AtomicU32::new(0);`

```
/// Core the WASM NIC driver polls its card from, +1 (0 = none registered).
/// Recorded at registration because the driver registers from its own fiber,
/// and a fiber does not migrate. Read by the microvm so it never puts a vCPU on
/// that core: the two would be cooperative peers, and the driver would poll only
/// when the vCPU yields.
```

## L111 · `pub fn register_wasm_nic(mac: [u8; 6]) {`

```
/// Called by WASM host function npk_netdev_register
```

## L123 · `pub fn unregister_wasm_nic() {`

```
/// Called by cleanup_hw_state when WASM driver exits
```

## L130-131 · `pub fn wasm_nic_core() -> Option<usize> {`

```
/// The core a registered WASM NIC driver runs on. `None` when no such driver is
/// up, or when it sits on Core 0 (which never carries a vCPU anyway).
```

## L143-152 · `static WASM_CARRIER: AtomicBool = AtomicBool::new(false);`

```
// RFC 2863, as Linux implements it in `link_watch.c` / `rfc2863_policy`: a
// link has TWO independent facts, not one.
//
//   carrier — the physical/association link exists
//   dormant — it exists but is not usable yet (802.1X / WPA not done)
//   operstate UP = carrier && !dormant
//
// We had a single `link_up` carrying all three meanings, so every 4-way
// handshake — a second of `authorized == false` on a perfectly healthy
// association — read as "the link went away" and re-ran DHCP.
```

## L156-164 · `pub fn set_wasm_nic_link_state(carrier: bool, dormant: bool) {`

```
/// Driver reports carrier and dormant separately (npk_netdev_set_link_state).
///
/// **Jeder Wechsel geht ins Log, mit seinem Takt.** Der Zustand der Karte
/// entschied bisher still darueber, ob `netdev::send` ueberhaupt noch einen
/// Rahmen annimmt (`active_link_up`) — und ein Traegerverlust von einer
/// halben Sekunde sah hinterher genauso aus wie „das WLAN war die ganze
/// Zeit da". `tick_link_and_reconfigure` protokolliert zwar Linkwechsel,
/// laeuft aber nur am Prompt (aus `read_line_with_tab`), also waehrend
/// eines Downloads GAR NICHT. Genau dann faellt es aus.
```

## L176-177 · `pub fn set_wasm_nic_link(up: bool) {`

```
/// Legacy single-flag form (npk_netdev_set_link): a driver that knows only
/// "usable / not usable" reports it as carrier with no dormant phase.
```

## L182 · `pub fn wasm_nic_link_up() -> bool {`

```
/// operstate: usable right now.
```

## L188-189 · `pub fn wasm_nic_carrier() -> bool {`

```
/// Association exists, whether or not it is keyed yet. This is what must NOT
/// flap during a rekey, and what the link-change logic keys on.
```

## L194 · `pub fn wasm_nic_dormant() -> bool {`

```
/// Link is up but still being authorized.
```

## L199-203 · `static INTEL_LINK: AtomicBool = AtomicBool::new(false);`

```
// ── Wired link-state cache + active-NIC selection ─────────────────────────
// The wired NICs only report carrier live via an MMIO read (intel STATUS.LU) or
// a USB control transfer (rtl8153 PHY BMSR) — too costly for the dispatch path,
// which runs in IRQ context. refresh_link_state() (Core 0, ~1 Hz) reads them
// into this cache; dispatch + active() read the cache, staying cheap + IRQ-safe.
```

## L206-210 · `static PREFER_WIFI: AtomicBool = AtomicBool::new(false);`

```
/// Cached NIC preference: true = prefer WiFi (wlan), false = prefer wired (LAN).
/// Refreshed by refresh_link_state() (Core 0, ~1 Hz) from config `net_prefer`,
/// so active() stays cheap + IRQ-safe. Default = wired (the usual convention).
/// Whichever side is preferred wins ONLY when it has a usable link; otherwise we
/// fall back to the other interface if it has one.
```

## L213-219 · `pub fn refresh_link_state() {`

```
/// Refresh the cached wired link state. Core 0 only (~1 Hz). ONLY the intel NIC
/// is polled live — a cheap, safe MMIO STATUS.LU read. The rtl8153 carrier is
/// NOT polled: reading it needs a USB control transfer, which takes the xHCI NIC
/// lock, and a timer IRQ landing mid-lock (poll_mouse takes the same lock)
/// deadlocks Core 0 (observed: networking died after ~20 ticks, instantly when
/// the USB NIC also carried traffic). A USB-LAN NIC's cable state is inferred
/// from presence + the WiFi link instead — see active().
```

## L230-236 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// The active interface, honouring the `net_prefer` config (cached in
/// PREFER_WIFI). The preferred side (wired by default, or WiFi) wins ONLY when it
/// has a usable link; if it has none we fall back to the other interface if that
/// one does. "Usable link" = intel STATUS.LU live (read live), or a USB-LAN
/// (rtl8153) present (its carrier can't be safely probed, so presence is the best
/// signal), or WiFi associated + keyed. If nothing has a usable link we fall back
/// to mere presence.
```

## L250-254 · `if intel_up { return Active::Intel; }`

```
// Prefer wired — but only a PROVEN wired link (intel STATUS.LU) outranks
// a proven WiFi link. A USB-LAN (rtl8153) has no carrier detect, so a
// cable-less / merely-enumerated dongle must NOT strand a working WiFi
// link (that would route DHCP out the dead NIC → no lease). So it ranks
// BELOW associated WiFi.
```

## L259 · `if intel_nic::is_available() { return Active::Intel; }`

```
// Nothing with a usable link in the preferred order — fall back to presence.
```

## L266-285 · `pub fn active_rx_rate() -> u32 {`

```
/// Saubere Empfangskapazitaet der AKTIVEN Schnittstelle in Bytes/s.
/// `u32::MAX` = kein Deckel.
///
/// **Das hier war eine GLOBALE, und gesetzt hat sie genau ein Treiber.**
/// `rtl8153::init` rief `tcp::set_link_rx_rate(20_000_000)`, und damit galt
/// der Wert des USB-Dongles fuer JEDE Schnittstelle — auch fuer die
/// WLAN-Karte, die ihre Kapazitaet nie gemeldet hat. Steckte der Dongle,
/// bekam das WLAN versehentlich ein vernuenftiges Fenster; steckte er
/// nicht, blieb der Wert auf `u32::MAX` und das WLAN bot den GANZEN Puffer
/// an: 8 MiB auf einer 50-Mbit-Strecke, also das Dreissigfache ihres BDP.
///
/// Gemessen am Geraet (2026-09-21): `snd_wnd=8387072` und `rtt=47203` us
/// auf einer Strecke, die unbelastet 3-5 ms hat. Die uebrigen ~42 ms waren
/// unsere eigenen Pakete in der Warteschlange des AP — der lief ueber,
/// `lost=106` bei `retr=175` (8,6 %), und der Durchsatz fiel auf 1,4 Mbit.
/// Bufferbloat, von uns verursacht.
///
/// Die Kapazitaet ist eine Eigenschaft der LINK-KLASSE, und sie gehoert
/// deshalb hierher, wo die Klasse bekannt ist — nicht in eine Globale, die
/// der zuletzt gestartete Treiber gewinnt.
```

## L287-295 · `if wasm_nic_link_up() {`

```
// **Ohne `active()`, und das ist Absicht.** `recv_window` ruft das hier
// im SEGMENTpfad und unter dem Verbindungsschloss; `active()` nimmt im
// Rueckfallzweig `virtio_net::is_available()`, und das ist ein
// `DEVICE.lock()`. Eine Schlossnahme dort hat nichts verloren -- sie
// waere unter `CONNECTIONS` eine zweite Ordnung, und der Pfad laeuft
// auch aus dem Fiber eines Treibers. Gefragt werden nur die zwei
// Klassen, die ueberhaupt einen Deckel haben, beide ueber Atomics; die
// Reihenfolge ist die von `active()` (WLAN vor USB-LAN, weil ein
// assoziiertes WLAN einen Dongle ohne Traegererkennung ausrankt).
```

## L297 · `return 8_000_000;`

```
// 2x2 HT20 auf 2,4 GHz: brutto 144 Mbit, sauber etwa 64.
```

## L303 · `u32::MAX`

```
// Echtes Gigabit / virtio / nichts: der Puffer IST das Fenster.
```

## L307 · `#[allow(dead_code)]`

```
/// Nur noch fuer den Bericht: dieselbe Antwort ueber `active()`.
```

## L311-315 · `Active::Rtl => rtl8153::rx_rate(),`

```
// Gigabit-Draht hinter High-Speed-USB. **Offen und benannt:** ueber
// Kupfer wurden 342 Mbit sauber gemessen (retrans 0), also traegt
// diese Strecke mehr als die 160 Mbit, die hier stehen. Die Zahl
// stammt vom HP-Notebook und ist nicht nachgemessen; sie bleibt,
// bis sie EINZELN gemessen wird.
```

## L317-320 · `Active::Wasm => 8_000_000,`

```
// 2x2 HT20 auf 2,4 GHz: brutto 144 Mbit, sauber etwa 64. Mit dem
// BDP aus `rate x RTT` und der Untergrenze RCV_WND_MIN landet eine
// gesunde Strecke damit bei 256 KB — genug fuer 512 Mbit bei 4 ms,
// also kein Deckel, aber das Dreissigfache weniger Ueberschuss.
```

## L322 · `Active::Intel | Active::Virtio | Active::None => u32::MAX,`

```
// Echtes Gigabit / virtio: der Puffer IST das Fenster.
```

## L327-331 · `pub fn active_link_up() -> bool {`

```
/// Does the ACTIVE interface have a usable link right now? Distinct from
/// `active_id`, which only says which interface would be used: a WiFi NIC is
/// registered (and therefore "active" by fallback) from the moment the driver
/// starts, long before it is associated and keyed. Anything that waits for the
/// network to become usable has to look at this, not at the id.
```

## L336 · `Active::Rtl | Active::Virtio => true,`

```
// No carrier detect on either — presence is all we have.
```

## L342-345 · `pub fn active_name() -> &'static str {`

```
/// Name of the active interface, for the one log line that has to say WHY the
/// link "changed". Without it a WiFi carrier that blinks for a single sample
/// reads as "link changed -> requesting DHCP" with no hint that the stack
/// briefly routed over a cable-less NIC and back.
```

## L356 · `pub fn active_id() -> u8 {`

```
/// A cheap numeric id of the active interface, for change detection.
```

## L367-370 · `static RX_TO_RING: AtomicU64 = AtomicU64::new(0);   // frames put in the fallback ring`

```
// ── WASM-NIC path counters (read by the `wlan` intent) ────────────────────
// The whole point is to tell WHERE frames are lost between driver and stack:
// a spill ring that overflows and an AQM that drops look identical from the
// outside (throughput just sags), so each side counts its own drops.
```

## L371 · `static RX_TO_RING: AtomicU64 = AtomicU64::new(0);   // frames put in the fallback ring`

```
// frames put in the fallback ring
```

## L372 · `static RX_RING_DROP: AtomicU64 = AtomicU64::new(0); // …and lost because it was full`

```
// …and lost because it was full
```

## L373 · `static TX_ENQUEUED: AtomicU64 = AtomicU64::new(0);  // frames handed to fq_codel`

```
// frames handed to fq_codel
```

## L374 · `static TX_DEQUEUED: AtomicU64 = AtomicU64::new(0);  // …and picked up by the driver`

```
// …and picked up by the driver
```

## L413 · `pub fn wasm_nic_submit_rx(frame: &[u8]) {`

```
/// WASM driver calls this to submit a received frame to the kernel network stack
```

## L423-425 · `static NIC_WAKER: AtomicU32 = AtomicU32::new(crate::smp::fiber::NO_WAKER);`

```
/// WASM driver calls this to get a frame to transmit (fq_codel-scheduled)
/// The WASM NIC driver's fiber, registered when it waits for TX work
/// (`npk_wait`). A queued frame wakes it at once instead of on its next poll.
```

## L432 · `pub fn wasm_nic_tx_pending() -> bool {`

```
/// Frames waiting for the WASM NIC driver?
```

## L443-445 · `pub fn wasm_nic_poll_rx(buf: &mut [u8; MTU]) -> Option<usize> {`

```
/// Pop the oldest fallback-RX frame, if any. Used by net::wasm_deliver_rx to
/// flush frames that spilled to the ring (while Core 0 held the drain guard)
/// before the freshly-delivered one — preserving FIFO order.
```

## L451-458 · `if !active_link_up() {`

```
// Never hand a frame to an interface that has no link. active() falls back
// to mere PRESENCE when nothing has a carrier, which is right for display
// and wrong for the data path: on a machine whose only device is a WiFi card
// that has not finished associating, every DHCP attempt went to the driver,
// reached the firmware, and sat in its TX queue unsendable. in-flight never
// returned to zero and the association never completed — while the same
// machine with any wired device present worked, because the traffic went
// there instead and left the radio alone.
```

## L460-463 · `let n = TX_REJECT_NO_LINK.fetch_add(1, Ordering::Relaxed) + 1;`

```
// **Die erste Abweisung sagt es, danach jede tausendste.** Ohne sie
// ist ein geschlossenes Sendetor von einem stillen Netz nicht zu
// unterscheiden: beide Male passiert nichts, und der Zaehler wurde
// nur von `net`/`wlan` gedruckt — also erst, wenn jemand FRAGT.
```

## L473-474 · `if WASM_NIC.lock().tx.enqueue(frame) {`

```
// Count what was TAKEN, not what was offered. The old order counted
// first and threw the result away, so a refused frame read as sent.
```

## L493 · `pub fn tso_capable() -> bool {`

```
/// Can the active card segment + checksum a TCPv4 GSO super-frame itself?
```

## L502-504 · `pub fn send_tso(frame: &[u8], mss: u16, l4_off: usize, hdr_len: usize) -> Result<(), NetError> {`

```
/// Send a TCPv4 GSO super-frame (whole Ethernet frame, IPv4 without options,
/// TCP check = pseudo-header seed incl. length). The card cuts it into `mss`
/// segments. Err ⇒ nothing was queued; the caller segments in software.
```

## L518-522 · `pub fn tx_reject_stats() -> (u32, u32) {`

```
/// Frames this guard refused, and frames a driver refused. Every caller above
/// this line throws the Result away — `udp::send`, `ipv4::send` and
/// `eth::send_frame` all say `let _ =` — so a packet that never reached the air
/// is indistinguishable from one that got no answer. It cost two wrong theories
/// about DNS before anyone could ask the question.
```

## L532 · `Active::Wasm => WASM_NIC_RX.lock().pop(buf),`

```
// Already counted at `wasm_nic_submit_rx` / `note_rx_available`.
```

## L536 · `Active::Virtio | Active::None => virtio_net::recv(buf),`

```
// Sequence comes from the device's own used.idx — see `rx_seq`.
```

## L541-548 · `static RX_SEQ: AtomicU64 = AtomicU64::new(0);`

```
// ── RX wake signal, card-neutral ──────────────────────────────────────────
//
// The microvm data plane used to ask `drivers::virtio_net` directly whether an
// RX IRQ existed and how far the device's used ring had advanced. On the two
// target machines there IS no virtio NIC, so both answers were 0: the worker's
// "is there work" test became `0 != 0`, and the vector it parked on and routed
// was vector zero. That is not a regression — that path never ran on this
// hardware. Every card answers for itself here instead.
```

## L550-551 · `static RX_SEQ: AtomicU64 = AtomicU64::new(0);`

```
/// Frames a POLLED driver has handed the stack (intel / rtl8153 / WASM NIC).
/// Monotonic; only ever compared for inequality against a caller's snapshot.
```

## L553 · `static VIRTIO_RX_SEQ: AtomicU64 = AtomicU64::new(0);`

```
/// 64-bit extension of the virtio device's 16-bit RX used.idx.
```

## L559-561 · `#[inline]`

```
/// A driver made `n` received frames available to the stack. Called where the
/// frame ARRIVES, not where it is drained, so a consumer that is behind still
/// sees the sequence move.
```

## L565-567 · `fn virtio_rx_seq() -> u64 {`

```
/// Widen the device's wrapping 16-bit used.idx to a monotonic 64-bit count.
/// A concurrent caller can at worst make this return a value one wrap stale,
/// which costs one extra poll and never a missed frame.
```

## L577-579 · `pub fn rx_wake_vector() -> Option<u8> {`

```
/// LAPIC vector to park on for RX arrival, or `None` when the active card has
/// no RX MSI-X and must be polled. Only virtio-net has one today; the intel
/// NIC, the USB dongle and a WASM-driven card are all polled.
```

## L594-597 · `pub fn rx_seq() -> u64 {`

```
/// Monotonic count of frames the ACTIVE driver has provided. Changes when a
/// frame arrives, whether or not anyone has drained it. Switching cards
/// switches counters, so this is only ever compared for inequality — never
/// subtracted across a link change.
```

## L605-607 · `pub fn tx_flush() {`

```
/// Flush a batched TX doorbell. `send` defers the per-frame notify on virtio to
/// avoid a VM-exit per packet; the other drivers post as they go, so this is a
/// no-op for them. Card-neutral so the microvm data plane never names a driver.
```

## L627 · `#[derive(Clone, Copy)]`

```
// ── Interface enumeration ──
```

## L634 · `pub primary: bool,`

```
/// True for the interface that carries the global IP/Gateway/DNS config.
```

## L636-637 · `pub link_up: bool,`

```
/// Carrier/link state. Wired NICs are linked once present; the WiFi NIC is
/// linked only once associated + keyed (driver reports via set_wasm_nic_link).
```

## L641-642 · `pub fn list() -> alloc::vec::Vec<IfaceInfo> {`

```
/// List all active network interfaces. The first UP interface (Intel → WASM → virtio)
/// is marked primary and carries the global IPv4/Gateway/DNS config.
```

## L645-647 · `let act = active();`

```
// `primary` is the interface the dispatch actually uses (active()), and
// `link_up` is the cached REAL carrier — so a pulled cable shows DOWN and
// the WiFi link takes over, matching what the stack does.
```

## L656-657 · `v.push(IfaceInfo { name: "eth", driver: "Realtek RTL8153 (USB)", mac, primary: act == Active::Rtl, link_up: rtl8153::is_`

```
// No safe per-tick USB carrier read (see refresh_link_state); report
// presence. `primary` reflects what the stack actually uses.
```

