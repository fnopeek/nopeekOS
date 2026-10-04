# `kernel/src/microvm/devices/nat.rs` @ 5e0102684

## L1-21 · `#![allow(dead_code)]`

```
//! NAT for the microvm's virtio-net device.
//!
//! The guest sees a `10.99.0.0/24` link with a single peer at
//! `10.99.0.1` (the synthetic gateway). Per guest TX frame:
//!
//!   * ARP-Request for 10.99.0.1 → synth ARP-Reply (link-layer)
//!   * UDP to 10.99.0.1:53        → host `net::dns::resolve`, synth
//!                                  DNS-Reply
//!   * everything else (TCP/UDP/QUIC to real remotes) → **L3
//!     masquerade**: we do NOT terminate. Outbound packets are SNAT'd
//!     to our host IP + a masquerade port and sent via the host IP
//!     layer; replies are intercepted in `net::ipv4` (`tap_inbound`),
//!     rewritten back to the guest, put in the tap, and injected by the
//!     data-plane worker. The
//!     guest's real Linux TCP/UDP/QUIC runs end-to-end with the
//!     server — reliability/ordering/SACK/window-scaling are theirs,
//!     not ours. See the `L3 masquerade NAT` section below.
//!
//! ARP/DNS handlers return a fully-built virtio-net frame (virtio hdr
//! + ethernet + IPv4/UDP/payload); `virtio_net_pci.rs` walks the
//! avail-ring, writes it to a driver buffer, signals used + IRQ.
```

## L31-32 · `pub const GUEST_MAC:   [u8; 6] = [0x52, 0x54, 0x00, 0x6E, 0x70, 0x6B];`

```
/// Locally-administered guest MAC. Must match the one virtio-net
/// advertises through its device-cfg MAC field.
```

## L34 · `pub const GATEWAY_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x6E, 0x70, 0x01];`

```
/// MAC the host pretends to be on the synthetic gateway.
```

## L36 · `pub const GATEWAY_IP:  [u8; 4] = [10, 99, 0, 1];`

```
/// Synthetic gateway IP. ARP, DNS, and (later) NAT all live here.
```

## L38 · `pub const GUEST_IP:    [u8; 4] = [10, 99, 0, 2];`

```
/// Guest IP — PID-1 hard-codes the same value via SIOCSIFADDR.
```

## L59-60 · `const TCP_HDR_LEN: usize = 20;`

```
/// TCP header length without options — used to bound L4 checksum
/// recompute in the L3 path.
```

## L63-64 · `const TCP_FIN: u8 = 0x01;`

```
/// TCP flags the outbound segmenter has to carry correctly across a split
/// (`tcp_gso_segment`, net/ipv4/tcp_offload.c).
```

## L68-69 · `const VNET_HDR_GSO_TCPV4: u8 = 1;`

```
/// virtio-net header gso_type for a TCPv4 super-frame on guest TX (the ECN flag
/// 0x80 is OR'd on top and must be masked off before comparing).
```

## L73-76 · `#[derive(Clone, Copy, Debug)]`

```
/// Per-VM network policy. The browser default (`dns_tcp`) allows DNS +
/// TCP + UDP (QUIC) through the L3 masquerade; ICMP still needs an
/// explicit cap. Future work threads this through the microvm session
/// cap so different apps can have different policies.
```

## L89-90 · `pub const fn dns_tcp() -> Self {`

```
/// Browser default: DNS + TCP + UDP (QUIC/HTTP-3) + ICMP echo (ping /
/// reachability probes) via L3 masquerade.
```

## L100-110 · `use core::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtOrd};`

```
// ===========================================================================
// L3 masquerade NAT
//
// We do NOT terminate TCP. The guest's real Linux TCP/UDP/QUIC talks
// end-to-end with the real server; we only rewrite IP packets:
//   outbound  guest(10.99.0.2:p → R:q)  →  send from our_ip:HP → R:q
//   inbound   R:q → our_ip:HP           →  inject  R:q → 10.99.0.2:p
// Reliability, ordering, SACK, window-scaling, QUIC: all owned by Linux
// and the server. We are a stateless-ish packet rewriter + a 4-tuple
// table — no rtx/ack/RTO logic, none of the termination brittleness.
// ===========================================================================
```

## L116-120 · `static NS_RX_BYTES: AtomicU64 = AtomicU64::new(0);`

```
// ── Throughput / NAT-usage instrumentation (page-load perf diagnosis) ──
// Cheap relaxed counters; `pump` prints a one-line host-side `[netstat]`
// summary every ~5 s while a VM is active (NOT guest kmsg → no [guest] spam),
// so we can see whether page-load slowness is throughput, NAT-table drops, or
// latency. Window counters reset each summary; HIGHWATER + DROPS are lifetime.
```

## L125-136 · `static NS_GUEST_KICKS: AtomicU64 = AtomicU64::new(0);   // guest rang the TX doorbell`

```
/// Three different walls, counted apart. One shared counter is how the ax200
/// TX path hid a partial leak for a boot, and the same trap sits here: a full
/// staging queue is BACKPRESSURE (healthy, TCP slows down), a full masquerade
/// table means NO NEW FLOW CAN OPEN (the browser dies while old sockets live),
/// and an egress refusal means the frame never reached the wire. They look
/// identical from outside — throughput just sags — and only apart do they say
/// where to look.
/// What the guest actually handed us, before any classification. `NS_TX_PKTS`
/// counts only MASQUERADED egress, so a guest that has sent nothing but ARP and
/// IPv6 router solicitations reads as zero there — and "the guest never spoke"
/// and "the guest spoke and we dropped it" are opposite faults. Counted here,
/// at the door.
```

## L137 · `static NS_GUEST_KICKS: AtomicU64 = AtomicU64::new(0);   // guest rang the TX doorbell`

```
// guest rang the TX doorbell
```

## L138 · `static NS_GUEST_FRAMES: AtomicU64 = AtomicU64::new(0);  // frames taken off its TX ring`

```
// frames taken off its TX ring
```

## L139 · `static NS_GUEST_ARP: AtomicU64 = AtomicU64::new(0);     // …of which ARP`

```
// …of which ARP
```

## L140 · `static NS_GUEST_OTHER: AtomicU64 = AtomicU64::new(0);   // …neither ARP nor IPv4 (IPv6…)`

```
// …neither ARP nor IPv4 (IPv6…)
```

## L141 · `static NS_START_TICK: AtomicU64 = AtomicU64::new(0);`

```
/// Tick the guest started, so a report can say how long it has had to speak.
```

## L146-150 · `static NS_IP_MALFORMED: AtomicU64 = AtomicU64::new(0); // length / total-len clamp`

```
/// Where a guest IPv4 frame goes when it does NOT come out the other side.
/// `handle_ipv4` has five silent `return None`s; between them they can swallow
/// every packet a guest sends and leave the report showing a healthy zero in
/// every loss column. Counted, so the gap between "frames in" and "packets out"
/// has to name itself.
```

## L151 · `static NS_IP_MALFORMED: AtomicU64 = AtomicU64::new(0); // length / total-len clamp`

```
// length / total-len clamp
```

## L152 · `static NS_IP_TO_GW: AtomicU64 = AtomicU64::new(0);     // addressed to the gateway, not DNS`

```
// addressed to the gateway, not DNS
```

## L153 · `static NS_IP_DNS: AtomicU64 = AtomicU64::new(0);       // answered (or queued) by our resolver`

```
// answered (or queued) by our resolver
```

## L154 · `static NS_IP_PROTO: AtomicU64 = AtomicU64::new(0);     // not TCP / UDP / ICMP`

```
// not TCP / UDP / ICMP
```

## L155-156 · `static NS_TX_RUNT: AtomicU64 = AtomicU64::new(0);      // frame shorter than vnet+eth`

```
/// The rest of the silent exits, outbound and in. Every one of these could
/// swallow a guest's whole session while the report showed zeroes everywhere.
```

## L157 · `static NS_TX_RUNT: AtomicU64 = AtomicU64::new(0);      // frame shorter than vnet+eth`

```
// frame shorter than vnet+eth
```

## L158 · `static NS_TX_BADTCP: AtomicU64 = AtomicU64::new(0);    // emit_tcp_out bailed on the header`

```
// emit_tcp_out bailed on the header
```

## L159 · `static NS_TX_ARPMISS: AtomicU64 = AtomicU64::new(0);   // went out to L2 broadcast`

```
// went out to L2 broadcast
```

## L160 · `static NS_TX_RINGBAD: AtomicU64 = AtomicU64::new(0);   // guest TX queue unusable`

```
// guest TX queue unusable
```

## L161 · `static NS_TX_TRUNC: AtomicU64 = AtomicU64::new(0);     // descriptor chain broke mid-frame`

```
// descriptor chain broke mid-frame
```

## L162-167 · `static NS_RX_UNMATCHED: AtomicU64 = AtomicU64::new(0);`

```
/// An inbound TCP segment addressed to a port in OUR masquerade range that
/// matched no mapping. It does not stop here: `tap_inbound` returns false, the
/// host stack takes it, finds no socket, and answers the server with a RST
/// (tcp.rs:894). That is our own machine tearing down the guest's connection.
/// A page that loads for a second and then dies looks exactly like this, and
/// nothing counted it.
```

## L173 · `static NS_DROP_TABLE: AtomicU64 = AtomicU64::new(0);  // L3 masquerade table full`

```
// L3 masquerade table full
```

## L174 · `static NS_DROP_EGRESS: AtomicU64 = AtomicU64::new(0); // host NIC refused the frame`

```
// host NIC refused the frame
```

## L177-178 · `static NS_INJECT_FALSE: AtomicU64 = AtomicU64::new(0);`

```
/// The guest RX ring had no buffer posted, so the frame stayed in the tap.
/// High = the GUEST is the limiter, not us.
```

## L180-181 · `static NS_TCP_FLOWS: AtomicU64 = AtomicU64::new(0);`

```
/// New-flow counts by transport, to spot QUIC: a cold page that opens lots of
/// UDP flows is using HTTP/3.
```

## L185-188 · `static NS_LAST_ACTIVITY: AtomicU64 = AtomicU64::new(0);`

```
/// Last successful RX inject (TSC). Drives the BSP vCPU's idle park decision:
/// while RX is recently active the vCPU parks event-driven on the host NIC RX
/// IRQ (`irq_wait`) instead of a blind 10 ms timer sleep, so a download lull is
/// woken the moment the next batch arrives (drainmax 10 ms → ~sub-ms).
```

## L191-197 · `static DL_LAST_BULK_TSC: AtomicU64 = AtomicU64::new(0);`

```
/// GPU-throttle signal: TSC of the last *bulk* RX frame (a GRO superframe >4 KB,
/// only produced by a sustained download — browsing/idle frames are <1500 B).
/// The virtio-gpu framebuffer copy runs INLINE on the vCPU exit (steals net-
/// processing cycles + the memory bus); while this is recent it backs off from
/// ~30 fps to ~8 fps so the download isn't throttled by pixel copies. Florian's
/// "smaller window / hidden desktop = faster download" observation exposed the
/// coupling. Probe to size the win before the full off-vCPU GPU copy.
```

## L200 · `pub fn download_active() -> bool {`

```
/// True if a bulk RX frame arrived in the last ~250 ms (= an active download).
```

## L204 · `let win = (crate::interrupts::tsc_freq() / 1000) * 250; // 250 ms in TSC ticks`

```
// 250 ms in TSC ticks
```

## L208-210 · `pub fn recently_active() -> bool {`

```
/// True if RX delivered a packet in the last ~50 ms — i.e. a download is in
/// flight and the BSP vCPU should wake on the host NIC RX IRQ rather than
/// deep-parking on the 100 Hz worker timer.
```

## L213 · `let window = crate::interrupts::tsc_freq() / 20; // ~50 ms in TSC`

```
// ~50 ms in TSC
```

## L217-219 · `pub fn mark_active() {`

```
/// Mark the data plane active NOW (a frame moved RX or TX). The off-vCPU
/// `net_dataplane` worker calls this each pass it does real work, so
/// `recently_active()` gates its halt-poll.
```

## L224-226 · `const L3_PORT_LO: u16 = 20000;`

```
/// Masquerade host-port pool. Strictly below the host TCP stack's own
/// ephemeral range (49152..=65534, net/tcp.rs) so a guest flow can
/// never alias a host-originated connection (OTA `update`, `https`).
```

## L229 · `const L3_TCP_IDLE_TICKS: u64 = 12_000; // ~2 min  (~100 ticks/s)`

```
// ~2 min  (~100 ticks/s)
```

## L230 · `const L3_UDP_IDLE_TICKS: u64 = 3_000;  // ~30 s`

```
// ~30 s
```

## L238-243 · `host_ip: [u8; 4],`

```
/// The host address this flow went out with. NOT `arp::our_ip()` at read
/// time: a DHCP renewal or a carrier blink mid-session changes that, and a
/// mapping keyed on "the address we happen to hold now" is silently orphaned
/// the moment it moves — outbound leaves under a port the server never saw,
/// inbound is discarded before anything looks at it. The flow is keyed on
/// the address it was BORN with, which is the address the replies carry.
```

## L249-253 · `struct L3Table {`

```
/// Masquerade table plus a bit per host port in `[L3_PORT_LO, L3_PORT_HI)`
/// and two indexes into it, as conntrack hashes both tuple directions: every
/// packet used to walk the table linearly under this lock (32 KB per
/// packet, and slower with every flow a long test opened). One lock over
/// all of it, so no index can disagree with the table.
```

## L257-258 · `by_port: [u16; PORT_RANGE],`

```
/// Inbound: host port → slot. A host port belongs to one flow at a time
/// (the `used` bitmap is shared by all protocols).
```

## L260-261 · `by_flow: [u16; FLOW_BUCKETS],`

```
/// Outbound: open-addressed hash of (proto, guest port, remote) → slot,
/// linear probing, half full at most.
```

## L277-279 · `const NAT_MAX_ATTEMPTS: usize = 128;`

```
/// `nf_nat_l4proto_unique_tuple` probes at most this many ports before giving
/// up and re-rolling the offset — "we are in softirq; doing a search of the
/// entire range risks soft lockup when all tuples are already used".
```

## L292 · `fn find_flow(&self, proto: u8, gport: u16, rip: [u8; 4], rport: u16) -> Option<usize> {`

```
/// Slot of the outbound flow, if mapped.
```

## L308 · `fn insert(&mut self, i: usize, m: L3Map) {`

```
/// Occupy slot `i` with `m` and index it.
```

## L317-318 · `fn unindex_flow(&mut self, i: usize, key: usize) {`

```
/// Take slot `i` out of the flow hash: backward-shift deletion, so a
/// probe chain never breaks and no tombstones pile up.
```

## L330 · `let dist_home = j.wrapping_sub(home) & (FLOW_BUCKETS - 1);`

```
// Move j into the hole unless its home lies cyclically in (hole, j].
```

## L351 · `fn release(&mut self, i: usize) {`

```
/// Free slot `i` and its port together.
```

## L362-363 · `static L3_ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// Gates the host-RX inbound intercept. Off ⇒ `tap_inbound` is a cheap
/// `false` so a guest-less host (plain OTA/https) is never touched.
```

## L365-373 · `static FRAME_POOL: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());`

```
/// Recycled frame buffers for the inbound staging path. Each download packet
/// used to `vec![0u8; ~1514]` (allocator free-list walk + memset) and free it
/// after injection — the dominant inbound per-packet cost (~2.6µs/pkt: the
/// first-fit allocator walks an O(n) free list under churn, plus the memset).
/// Linux solves this with skb pools; we keep a small ring of buffers that the
/// producer (tap_inbound) borrows and the consumer (the worker) returns, so
/// after warmup the datapath does ZERO heap alloc/free — only the unavoidable
/// payload memcpy. Bounded so it can't grow without limit; only used while a VM
/// is active (the BSP is the sole accessor, so the lock is uncontended).
```

## L376 · `const FRAME_BUF_CAP: usize = 2048; // ≥ vnet+eth+MTU, so resize never reallocs`

```
// ≥ vnet+eth+MTU, so resize never reallocs
```

## L378-380 · `fn frame_pool_get(len: usize) -> Vec<u8> {`

```
/// Borrow a frame buffer sized to `len` from the pool (or allocate once if the
/// pool is cold). The contents are uninitialised beyond what the caller writes —
/// tap_inbound overwrites every byte (vnet hdr + eth hdr + full IP copy).
```

## L386-388 · `unsafe { buf.set_len(len); }`

```
// SAFETY: capacity ≥ len after the reserve above. The caller writes all
// `len` bytes before the buffer is read (vnet[0..12]=0, eth[12..26],
// ip-copy[26..len]), so no uninitialised byte is ever observed.
```

## L393 · `fn frame_pool_put(buf: Vec<u8>) {`

```
/// Return a frame buffer to the pool for reuse (dropped if the pool is full).
```

## L395-396 · `if buf.capacity() > FRAME_BUF_CAP { return; }`

```
// GRO superframes grow well past a normal frame; don't pool them or we'd
// hand a 60 KB buffer back for a 1.5 KB frame forever.
```

## L401-405 · `fn note_table_full() {`

```
/// Find an existing mapping for this guest flow or allocate one.
/// Returns the masquerade host port.
/// The masquerade table is full: no new flow can open. Budgeted, because if it
/// fires it fires for every packet of every new connection — and a log that
/// writes the flood is no longer a log. `netstat` carries the running count.
```

## L417-420 · `if let Some(i) = tbl.find_flow(proto, gport, rip, rport) {`

```
// Existing? A hit whose `host_ip` is no longer ours describes a flow the
// far end can no longer answer — the address moved. Retire it here instead
// of leaving it to time out; the caller gets a fresh mapping on the new
// address, which is the only thing that can still work.
```

## L431-435 · `let mut off = (crate::interrupts::rdtsc() as usize) % PORT_RANGE;`

```
// `nf_nat_l4proto_unique_tuple`: start at a varying offset, probe forward
// with an O(1) used-test, and give up after a BOUNDED number of attempts
// (then re-roll once). The old code walked all 1024 entries per candidate
// port — ~10^6 comparisons under the lock for one new flow on a full table,
// and a browser opens ~65 UDP flows per page.
```

## L451 · `match proto {`

```
// New flow — count by transport (UDP-heavy cold load = QUIC/HTTP-3).
```

## L460-461 · `fn l3_map_in(proto: u8, dst_ip: [u8; 4], hport: u16, rip: [u8; 4], rport: u16,`

```
/// Reverse lookup for an inbound reply: (proto, host_port) + remote
/// must match. Returns the guest port to deliver to.
```

## L478-499 · `fn fix_l4_checksum(proto: u8, src_ip: [u8; 4], dst_ip: [u8; 4], l4: &mut [u8]) {`

```
/// Recompute the TCP/UDP checksum after an address/port rewrite.
///
/// UDP used to be handled by ZEROING the field, on the grounds that a zero
/// checksum is legal for UDP-over-IPv4 (RFC 768) and cheaper than a pass over
/// the payload. Both halves of that are true and the conclusion is still wrong
/// for a masquerade: the datagram ARRIVED with a checksum, and throwing it away
/// is not translation, it is damage. What we hand on is a packet that claims to
/// be unprotected — and the far end is entitled to treat it accordingly.
///
/// It hid for as long as it did because of who reads the packet next. Under
/// QEMU the masqueraded datagram goes to slirp, a userspace stack that
/// terminates the flow and re-originates it on the outside; it never looks at
/// the field. On real hardware the very same packet goes straight onto the
/// wire to a real server. And the guest is a browser: `flows 6 tcp 25 udp` —
/// four out of five of its connections are HTTP/3, which is QUIC, which is UDP.
///
/// Outbound we must compute in full: with `VIRTIO_NET_F_CSUM` negotiated the
/// guest hands us CHECKSUM_PARTIAL, so the field holds a pseudo-header seed and
/// not a checksum. Inbound the datagram arrives complete and we could update
/// incrementally, but the same full pass keeps ONE implementation for both
/// directions — a few hundred nanoseconds against a class of bug that cost an
/// evening.
```

## L508-510 · `let declared = u16::from_be_bytes([l4[4], l4[5]]) as usize;`

```
// The length FIELD is the authority, not the slice: a minimum-size
// ethernet frame carries padding that is not part of the datagram, and
// checksumming it would produce a value the receiver cannot reproduce.
```

## L523-525 · `fn udp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], udp: &[u8]) -> u16 {`

```
/// UDP checksum over the IPv4 pseudo-header + datagram (RFC 768). A computed
/// zero goes on the wire as 0xFFFF, because zero is the "no checksum" escape and
/// would undo the whole point.
```

## L549-553 · `fn csum_update(old_check: u16, changes: &[(u16, u16)]) -> u16 {`

```
/// Incremental ones-complement checksum update (RFC 1624). Adjust an existing
/// checksum for a set of changed 16-bit words in O(changes) instead of
/// recomputing over the whole segment — `HC' = ~(~HC + Σ(~old + new))`. NAT only
/// rewrites the IP + port (≤3 words), so this replaces the full ~1500-byte
/// `tcp_checksum` loop that dominated the inbound per-packet cost (~2.6µs/pkt).
```

## L564-566 · `fn l3_outbound(proto: u8, src_port: u16, dst_ip: [u8; 4],`

```
/// Outbound SNAT: rewrite the guest's L4 source port to a masquerade
/// host port and send from our host IP. The guest's TCP/UDP semantics
/// (seq/ack/window/options/QUIC) pass through untouched.
```

## L576-581 · `emit_tcp_out(hp, our_ip, dst_ip, l4, gso_size);`

```
// TCP: software TSO segmentation (gso_size > 0 + payload past one MSS) or
// a single segment. Either way the TCP checksum is recomputed in full —
// with VIRTIO_NET_F_CSUM negotiated the guest now offloads its checksum
// (CHECKSUM_PARTIAL: the field holds only the pseudo-header seed), so the
// old incremental update has no valid base. Full recompute is correct
// whether or not CSUM is on and is required per-segment anyway.
```

## L584-585 · `NS_TX_PKTS.fetch_add(1, AtOrd::Relaxed);`

```
// UDP (and anything else routed here): single datagram, port rewrite +
// checksum fix-up, no segmentation.
```

## L589 · `seg[0..2].copy_from_slice(&hp.to_be_bytes());        // src port → host port`

```
// src port → host port
```

## L597-603 · `fn emit_tcp_out(hp: u16, our_ip: [u8; 4], dst_ip: [u8; 4],`

```
/// Emit a guest TCP segment outbound, software-segmenting a GSO/TSO super-frame
/// into `gso_size`-byte segments when needed. Ports the field math of Linux's
/// `tcp_gso_segment` (net/ipv4/tcp_offload.c): per segment, seq advances by the
/// payload already emitted; FIN/PSH are kept only on the last; CWR is cleared on
/// every segment but the first; the TCP checksum is computed in full over each
/// independent segment. The IP layer (`ipv4::send`) builds the per-segment IPv4
/// header (src/total-len/checksum), so we only fix up the TCP header here.
```

## L614 · `if mss == 0 || payload.len() <= mss {`

```
// Non-GSO (or fits in one MSS): single segment.
```

## L617 · `seg[0..2].copy_from_slice(&hp.to_be_bytes());        // src port → host port`

```
// src port → host port
```

## L627-628 · `if crate::netdev::tso_capable() && emit_tcp_tso(hp, our_ip, dst_ip, l4, thlen, gso_size) {`

```
// The card segments it: one super-frame instead of ~44 software-cut
// segments, each with a full checksum and its own trip through ipv4::send.
```

## L642 · `seg.extend_from_slice(&l4[..thlen]);                  // verbatim TCP header`

```
// verbatim TCP header
```

## L643 · `seg.extend_from_slice(&payload[off..off + this]);     // this segment's data`

```
// this segment's data
```

## L645 · `seg[0..2].copy_from_slice(&hp.to_be_bytes());         // src port → host port`

```
// src port → host port
```

## L646 · `let seq = base_seq.wrapping_add(off as u32);          // seq += bytes emitted`

```
// seq += bytes emitted
```

## L649 · `let mut flags = orig_flags;`

```
// FIN/PSH only on the last segment; CWR only on the first.
```

## L655 · `seg[16] = 0; seg[17] = 0;                             // full TCP checksum`

```
// full TCP checksum
```

## L666-670 · `fn emit_tcp_tso(hp: u16, our_ip: [u8; 4], dst_ip: [u8; 4], l4: &[u8],`

```
/// Hand a guest TSO super-frame to the host card whole (`netdev::send_tso`):
/// port rewritten, TCP check = pseudo-header seed incl. length (Linux
/// `CHECKSUM_PARTIAL`, what the guest itself hands us), IPv4 header built for
/// the host address. False ⇒ nothing was sent; the caller segments in software
/// (no ARP entry yet, ring full, card refused).
```

## L688 · `ip[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF`

```
// DF
```

## L698 · `f[l4_off..l4_off + 2].copy_from_slice(&hp.to_be_bytes()); // src port → host port`

```
// src port → host port
```

## L699-700 · `let mut sum: u32 = PROTO_TCP as u32 + l4.len() as u32;`

```
// Seed = folded pseudo-header sum, NOT complemented; the card adds the
// segment's own sum and complements (`~tcp_v4_check(len, s, d, 0)`).
```

## L717-722 · `fn l3_icmp_outbound(dst_ip: [u8; 4], l4: &[u8]) -> Option<Vec<u8>> {`

```
/// Outbound NAT for a guest ICMP echo request (so ping + the browser's
/// reachability probes to the DNS servers work instead of cap-rejecting). The
/// ICMP Identifier is the flow key, masqueraded like a port; only echo
/// requests (type 8) are forwarded, everything else dropped. Mirrors
/// `l3_outbound` but rewrites the id + uses the pseudo-header-less ICMP
/// checksum.
```

## L731 · `seg[4..6].copy_from_slice(&hp.to_be_bytes()); // id → masquerade id`

```
// id → masquerade id
```

## L738-739 · `fn fix_icmp_checksum(icmp: &mut [u8]) {`

```
/// ICMP checksum: ones-complement sum over the whole ICMP message (no pseudo-
/// header, unlike TCP/UDP). Zero the field first, then fold carries.
```

## L762-767 · `pub fn recycle_frame(buf: Vec<u8>) { frame_pool_put(buf); }`

```
/// Host-RX intercept. `ip` is a full IPv4 packet already filtered to
/// our IP. If it matches a masquerade mapping, rewrite it back to the
/// guest, enqueue for `pump`, and return true (consume — the host
/// stack must NOT also process it). Cheap `false` when no VM is up.
/// Return a frame buffer (from `tap_inbound`) to the recycle pool after it has
/// been injected into the guest ring — no per-packet heap churn.
```

## L770-784 · `const TAP_RING: usize = 512;`

```
// ===========================================================================
// The tap — drivers/net/tun.c
//
// One ring between whoever drains the host NIC and the one worker that feeds
// the guest. `tun_net_xmit` produces into a `ptr_ring` and, when it is full,
// takes SKB_DROP_REASON_FULL_RING and bumps `tx_dropped` — a counted drop, not
// a silent overwrite; `tun_do_read` consumes and blocks on the socket's wait
// queue when it is empty.
//
// This replaces "the worker drains the host NIC". Where a frame ENTERS used to
// depend on the card — cable/virtio came through `netdev::recv` behind the
// POLLING guard the worker took for itself, while the AX200's WASM driver
// delivers straight into `eth::handle_frame`, which the worker could not see at
// all. Now every card ends in the same place and the worker never touches a NIC.
// ===========================================================================
```

## L786 · `const TAP_RING: usize = 512;`

```
/// tun's `dev->tx_queue_len = TUN_READQ_SIZE` is 500. 512 keeps the power of two.
```

## L791 · `head: usize, // consumer`

```
// consumer
```

## L792 · `tail: usize, // producer`

```
// producer
```

## L803 · `static TAP_LEN: AtomicU64 = AtomicU64::new(0);`

```
/// Lock-free depth, so the worker's "is there work" test takes no lock.
```

## L805-808 · `static NS_TAP_FULL: AtomicU64 = AtomicU64::new(0);`

```
/// `tun_net_xmit`'s `tx_dropped`: the ring was full. This is BACKPRESSURE and
/// healthy in moderation — it is not the masquerade table filling up and not an
/// egress refusal, and the whole point of counting the three apart is that from
/// outside all three look like "throughput sagged" (see the note at the top).
```

## L810-812 · `static NS_TAP_DELIVERED: AtomicU64 = AtomicU64::new(0);`

```
/// Frames actually handed to the guest. PROGRESS, not fill level: a ring that
/// sits at 40 of 512 tells you nothing, a delivered-count that stops moving
/// tells you everything.
```

## L814-816 · `static NS_MAP_REHOMED: AtomicU64 = AtomicU64::new(0);`

```
/// Flows retired because the host address moved under them (DHCP renewal,
/// carrier blink). Zero on a healthy link; non-zero explains a stall that looks
/// like the far end went quiet.
```

## L818-821 · `static WORKER_PARKED: AtomicBool = AtomicBool::new(false);`

```
/// The worker is parked on its doorbell. Linux's wait queue: `sk_data_ready`
/// wakes nobody when no reader sleeps there, so a producer feeding a RUNNING
/// consumer sends no wakeup at all. Without this the empty→occupied edge would
/// IPI a busy-polling worker at line rate.
```

## L824 · `pub fn set_worker_parked(parked: bool) { WORKER_PARKED.store(parked, AtOrd::SeqCst); }`

```
/// The worker announces whether it is about to sleep on the tap.
```

## L827 · `pub fn tap_len() -> u64 { TAP_LEN.load(AtOrd::Relaxed) }`

```
/// Lock-free depth for the worker's poll condition.
```

## L830-831 · `fn tap_push(frame: Vec<u8>) -> bool {`

```
/// `ptr_ring_produce` + `sk_data_ready`. False ⇒ the ring was full and the frame
/// was dropped (counted); the buffer goes back to the pool either way.
```

## L847-849 · `if was_empty && WORKER_PARKED.load(AtOrd::SeqCst) {`

```
// Wake the reader only on the empty→occupied edge, and only if one is
// actually asleep. During a burst the ring stays occupied, so a burst costs
// one wakeup, not one per frame.
```

## L852-854 · `crate::smp::kick_host_core(c);`

```
// Bumps the target core's kick generation BEFORE the IPI, so a wake
// racing the park is never lost — the scheduler re-tests the
// generation every scan and finds the fiber runnable.
```

## L861 · `pub fn tap_pop() -> Option<Vec<u8>> {`

```
/// `ptr_ring_consume`. The worker is the only caller.
```

## L873-875 · `pub fn tap_push_front(frame: Vec<u8>) {`

```
/// `vhost_discard_vq_desc`: `inject_rx` rolled its descriptors back, so this
/// frame was never consumed. Put it back at the head — order matters on a TCP
/// stream. Only the worker calls this, between a pop and the next pop.
```

## L891 · `pub fn note_tap_delivered() { NS_TAP_DELIVERED.fetch_add(1, AtOrd::Relaxed); }`

```
/// One frame reached the guest.
```

## L894 · `pub fn tap_reset() {`

```
/// Empty the tap (VM teardown), returning every buffer to the pool.
```

## L909-916 · `pub fn tap_inbound(ip: &[u8]) -> bool {`

```
/// THE inbound acceptance test, and the only translation. Called from
/// `ipv4::handle_ipv4` for every received IPv4 packet, BEFORE the "is this
/// addressed to the address we happen to hold right now" filter: a guest flow is
/// keyed on the address it went out with, and asking the other question first
/// discards the reply before anything has looked at it.
///
/// Returns true if the packet was guest traffic and is now the tap's problem —
/// the host stack must not also process it. A cheap `false` when no VM is up.
```

## L938-940 · `if proto == PROTO_TCP && (L3_PORT_LO..L3_PORT_HI).contains(&host_port) {`

```
// Not ours by the mapping. A TCP port inside our masquerade range is
// not the host's either, and the host stack answers it with a RST —
// our own machine tearing down the guest's connection.
```

## L951 · `frame[..VNET_HDR_LEN].fill(0); // empty virtio-net header`

```
// empty virtio-net header
```

## L964 · `let old_check = u16::from_be_bytes([frame[l4_off + 16], frame[l4_off + 17]]);`

```
// Incremental TCP checksum (RFC 1624): only dst IP + dst port changed.
```

## L981-983 · `pub fn path_id() -> &'static str { "tap-v1" }`

```
/// Which data path this kernel is running, for `netstat`. QEMU, the NUC and the
/// notebook must all print the SAME id — that, not any throughput number, is
/// the acceptance of the rebuild: one path, taken by every machine.
```

## L986-991 · `pub fn housekeep() {`

```
/// Lock-free NAT housekeeping for the full off-vCPU data plane: reap idle
/// masquerade mappings so the table can't fill over a long session. In full mode
/// the device-touching work (`tx_flush`/RX drain) is the `net_dataplane` worker's
/// job — the BSP only needs this, and it takes NO net-device lock (so the BSP
/// never contends with the worker on the hot path). The worker owns RX+TX; the
/// vCPU owns guest execution + IRQ injection. That's the single, unified path.
```

## L996 · `fn l3_reap(now: u64) {`

```
/// Drop idle mappings so the table can't fill over a long session.
```

## L1008 · `pub fn l3_reset() {`

```
/// Tear down all L3 state (VM stopped). Idempotent.
```

## L1011 · `{`

```
// In place: a fresh `L3Table` is ~80 KB, too much for a fiber stack.
```

## L1020 · `*FRAME_POOL.lock() = Vec::new(); // release recycled buffers`

```
// release recycled buffers
```

## L1023-1026 · `pub fn tap_outbound(payload: &[u8], caps: &NetCaps) -> Vec<Vec<u8>> {`

```
/// Egress half of the tap, symmetric with [`tap_inbound`]: classify a guest TX
/// frame (virtio-net hdr + ethernet) and produce zero or more RX frames to
/// inject back. Side-effects: kprintln on
/// cap-rejects so the operator can see why a packet went nowhere.
```

## L1033-1035 · `let gso_size = if (payload[1] & !VNET_HDR_GSO_ECN) == VNET_HDR_GSO_TCPV4 {`

```
// virtio-net header (12 B): byte 1 = gso_type, bytes 4..6 = gso_size (LE).
// With TX-GSO the guest hands us one ≤64 KB TCPv4 super-frame; gso_size is
// the MSS we re-segment to. Mask off the ECN flag (0x80) before comparing.
```

## L1055-1056 · `}`

```
// Quiet: IPv6 / LLDP / STP / etc. — guest has nothing
// useful to do with them on this synthetic link.
```

## L1062 · `fn handle_arp(frame: &[u8]) -> Option<Vec<u8>> {`

```
/// ARP-Request for `GATEWAY_IP` → build matching ARP-Reply.
```

## L1067 · `if oper != 1 { return None; }                   // not a request`

```
// not a request
```

## L1078 · `reply[arp_off + 0] = 0x00; reply[arp_off + 1] = 0x01; // htype = Ethernet`

```
// htype = Ethernet
```

## L1079 · `reply[arp_off + 2] = 0x08; reply[arp_off + 3] = 0x00; // ptype = IPv4`

```
// ptype = IPv4
```

## L1082 · `reply[arp_off + 6] = 0x00; reply[arp_off + 7] = 0x02; // oper = REPLY`

```
// oper = REPLY
```

## L1090-1091 · `fn handle_ipv4(frame: &[u8], caps: &NetCaps, gso_size: u16) -> Option<Vec<u8>> {`

```
/// IPv4 dispatch: only UDP→10.99.0.1:53 has a real handler today.
/// Everything else logs a cap-reject and returns None.
```

## L1101-1105 · `let ip_total = u16::from_be_bytes([ip[2], ip[3]]) as usize;`

```
// Clamp L4 to the IP total-length. The guest TX buffer is bigger
// than the packet (min-frame / driver padding); &ip[ihl..] would
// append that garbage to every outbound segment → the server
// misframes the response → Firefox reads a wild length → ~4 GiB
// alloc → crash. Inbound is already clamped in net/ipv4.rs.
```

## L1128 · `None    // other gateway-directed UDP: nothing here`

```
// other gateway-directed UDP: nothing here
```

## L1158-1162 · `enum DnsOutcome {`

```
/// rcode-relevant outcome of a lookup. The NoData vs NxDomain split is
/// load-bearing: a non-A query (AAAA / HTTPS-SVCB type 65) on a name
/// that exists MUST be NOERROR/NODATA, not NXDOMAIN. Firefox queries
/// the HTTPS RR before every connection and reads NXDOMAIN as "host
/// does not exist" → "secure site not available".
```

## L1164 · `Answer([u8; 4]),  // A record`

```
// A record
```

## L1165 · `NoData,           // NOERROR, no answer — name exists, no such RR type`

```
// NOERROR, no answer — name exists, no such RR type
```

## L1166 · `NxDomain,         // name does not exist`

```
// name does not exist
```

## L1169-1183 · `fn handle_dns(src_ip: [u8; 4], src_port: u16, dgram: &[u8]) -> Option<Vec<u8>> {`

```
/// Parse a DNS query, answer it from the host resolver's CACHE, and synthesize
/// the reply with the correct rcode.
///
/// Cache only. This runs on the vCPU fiber, inside the virtio-net MMIO exit,
/// with the device mutex held — `net::dns::resolve` would spin here for up to
/// its whole 5.5 s budget. That is not merely a frozen guest: `pump_peers()`
/// bails out inside a fiber, so the WASM NIC driver fiber sharing this core
/// stops posting receive buffers, the card runs dry after its ~50 ms worth, and
/// the reply that would end the wait is one of the frames that can no longer
/// arrive. Measured on the notebook: one page loaded, then the radio was gone
/// and the host had no network either.
///
/// So: hit → answer, known-bad → NXDOMAIN, unknown → hand the name to Core 0
/// and drop the query. UDP DNS is retried by whoever asked, and the retry
/// finds a warm cache.
```

## L1197-1198 · `DnsOutcome::NoData`

```
// AAAA / HTTPS-SVCB / etc.: we don't serve the record, but the
// name exists. NODATA — NXDOMAIN here poisons the whole host.
```

## L1211 · `name_bytes: Vec<u8>,   // raw labels incl. terminating 0, for reply echo`

```
// raw labels incl. terminating 0, for reply echo
```

## L1229 · `if len & 0xC0 != 0 { return None; }   // pointer in query — unusual`

```
// pointer in query — unusual
```

## L1233 · `if !(0x20..0x7F).contains(&b) { return None; }`

```
// Conservative: stringify printable ASCII only, else bail.
```

## L1253 · `p.extend_from_slice(&q.id.to_be_bytes());`

```
// Header
```

## L1255 · `let flags: u16 = 0x8180 | rcode;                                // QR | RD | RA | rcode`

```
// QR | RD | RA | rcode
```

## L1257 · `p.extend_from_slice(&1u16.to_be_bytes());                       // QDCOUNT`

```
// QDCOUNT
```

## L1258 · `p.extend_from_slice(&ancount.to_be_bytes());                    // ANCOUNT`

```
// ANCOUNT
```

## L1259 · `p.extend_from_slice(&0u16.to_be_bytes());                       // NSCOUNT`

```
// NSCOUNT
```

## L1260 · `p.extend_from_slice(&0u16.to_be_bytes());                       // ARCOUNT`

```
// ARCOUNT
```

## L1261 · `p.extend_from_slice(&q.name_bytes);`

```
// Question (echo)
```

## L1265 · `if let DnsOutcome::Answer(ip) = out {`

```
// Answer (only for an A hit)
```

## L1267 · `p.push(0xC0); p.push(0x0C);`

```
// NAME — compression pointer back to question (offset 12).
```

## L1269 · `p.extend_from_slice(&1u16.to_be_bytes());   // TYPE = A`

```
// TYPE = A
```

## L1270 · `p.extend_from_slice(&1u16.to_be_bytes());   // CLASS = IN`

```
// CLASS = IN
```

## L1271 · `p.extend_from_slice(&60u32.to_be_bytes());  // TTL = 60s`

```
// TTL = 60s
```

## L1272 · `p.extend_from_slice(&4u16.to_be_bytes());   // RDLENGTH`

```
// RDLENGTH
```

## L1278-1279 · `fn build_ipv4_udp_reply(`

```
/// Build a full virtio-net frame (12-byte virtio hdr + eth + IPv4 + UDP
/// + payload) addressed `GATEWAY → GUEST` for source/dst ports given.
```

## L1293 · `buf[ip_off + 0]  = 0x45;                                    // version + IHL`

```
// version + IHL
```

## L1294 · `buf[ip_off + 1]  = 0;                                        // TOS`

```
// TOS
```

## L1296 · `buf[ip_off + 4..ip_off + 6].copy_from_slice(&0u16.to_be_bytes()); // id`

```
// id
```

## L1297 · `buf[ip_off + 6..ip_off + 8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF`

```
// DF
```

## L1298 · `buf[ip_off + 8]  = 64;                                       // TTL`

```
// TTL
```

## L1300 · `buf[ip_off + 12..ip_off + 16].copy_from_slice(&GATEWAY_IP);`

```
// checksum = 0 placeholder
```

## L1311 · `buf[udp_off + UDP_HDR_LEN..].copy_from_slice(payload);`

```
// checksum zero (allowed for UDP-over-IPv4)
```

## L1318-1322 · `buf[10] = 1;`

```
// virtio_net_hdr at offset 0..12. Per virtio 1.2 §5.1.6.4.1, with
// VIRTIO_F_VERSION_1 negotiated and VIRTIO_NET_F_MRG_RXBUF NOT
// negotiated, num_buffers (bytes 10..12, LE) MUST be 1 or Linux's
// virtio_net driver drops the packet silently in receive_buf().
// Everything else stays zero (no offloads, no GSO).
```

## L1351 · `if n >= 8 { return; }   // limit log spam`

```
// limit log spam
```

## L1358-1360 · `fn tcp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], tcp_segment: &[u8]) -> u16 {`

```
/// TCP checksum: pseudo-header (src_ip, dst_ip, zero, proto, tcp_len) +
/// the segment itself. Caller passes the same src/dst IPs that go in
/// the IPv4 header.
```

## L1363 · `sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;`

```
// pseudo-header
```

## L1384-1386 · `static NS_GTIMER: AtomicU64 = AtomicU64::new(0);`

```
/// Guest timer-IRQ injections this window (PIT IRQ0 + LAPIC LVTT). Confirms the
/// CONFIG_HZ=1000 fix: should read ~1000/s (the guest's programmed rate), not
/// the old ~100/s (our wall-clock pacing). Incremented from the SVM inject path.
```

## L1389-1393 · `pub fn guest_timer_count() -> u64 { NS_GTIMER.load(AtOrd::Relaxed) }`

```
/// Cumulative guest timer-IRQ injections (LVTT+PIT). Surfaced in `cores` to
/// MEASURE the effective guest HZ: ~1000/s = the guest's programmed 1 kHz tick
/// is delivered; <1000/s = the BSP's 2ms parks are freezing the guest timer
/// (floor b) → the guest's delayed-ACK/RTO/pacing slow → the slow download
/// regime. Tests the "1000 vs 100, mal gut mal schlecht" hypothesis directly.
```

## L1395-1400 · `pub fn tx_stats() -> (u64, u64) {`

```
/// Cumulative outbound TX (segments, bytes) — surfaced in `cores` as segs/s +
/// avg segment size during an upload. The b1-vs-b2 discriminator: a high segs/s
/// with the worker core pegged = the SW-TSO emit pipeline is the cap (b1, the
/// lock-split + host-TX batching lift it); the same segs/s with an idle worker =
/// the cap is cwnd × inflated bridge RTT (b2, an ACK-clock the emit path can't
/// raise). Monotonic in full mode (pump's swap never runs); diff two snapshots.
```

## L1404-1405 · `static NS_NET_IRQ: AtomicU64 = AtomicU64::new(0);`

```
/// Count of net-RX IRQ10 actually raised to the guest (after ITR moderation).
/// vs the per-packet rate it would be without — the io-EOI-storm signal.
```

## L1408-1411 · `pub fn rx_health_snapshot() -> (u64, u64) {`

```
/// Bridge RX health for `cores`: (tap ring-full drops, guest-ring-full stalls).
/// A ring-full drop is BACKPRESSURE — the producer outran the guest and the far
/// end slows down. `inject_false` is the guest being the limiter: it had no RX
/// buffer posted, so the frame stayed in the tap and nothing was lost.
```

## L1416 · `pub fn note_inject_false() { NS_INJECT_FALSE.fetch_add(1, AtOrd::Relaxed); }`

```
/// The guest RX ring was full — the frame went back to the head of the tap.
```

## L1419-1422 · `static NS_GPU_BYTES: AtomicU64 = AtomicU64::new(0);`

```
/// virtio-gpu TRANSFER_TO_HOST pixel bytes copied on the vCPU core (the browser
/// rendering). If high during a download, the framebuffer copy is stealing vCPU
/// cycles from the net pump (the framebuffer↔pump contention) — Florian's
/// "graphics?" hypothesis, measured.
```

## L1432-1438 · `pub struct BridgeStats {`

```
/// Everything the bridge knows about itself, for `netstat`.
///
/// The counters were always there; they lived behind a debug const that had
/// been `false` for months, so the one path nobody could see was the one
/// between the guest and the wire. This is the `wlan` treatment: no console
/// traffic, one screen on demand, and the numbers arranged so the reader can
/// tell the failures APART rather than watching a single "throughput sagged".
```

## L1442-1444 · `pub path: &'static str,`

```
/// Identity of the data path, so the three machines can be COMPARED rather
/// than each believed on its own. Same path id on QEMU, NUC and notebook is
/// the acceptance test of the whole rebuild.
```

## L1450-1452 · `pub tap: u64, pub tap_cap: usize,`

```
/// Tap: current depth, capacity, frames delivered to the guest, and the
/// ring-full drops. Progress (`tap_delivered`) is the number that matters —
/// a fill level says nothing (feedback_watchdog_that_only_fires_at_full).
```

## L1466 · `pub gpu_pct: u64, pub gpu_us_each: u64,`

```
/// Percent of the last window the vCPU spent inside the framebuffer copy.
```

## L1474-1477 · `static RPT_TSC: AtomicU64 = AtomicU64::new(0);`

```
// Previous snapshot, so a second `netstat` a few seconds later reads as a RATE.
// Cumulative counters answer "did this ever work"; only the rate answers "is it
// working right now", which is the whole question when a link dies after five
// seconds.
```

## L1564-1566 · `pub fn active_session_count() -> usize {`

```
/// Number of currently-active (non-closed) TCP sessions. The run_linux
/// idle-detection uses this to extend the timeout when traffic is in
/// flight.
```

## L1568-1569 · `L3.lock().maps.iter().flatten().count() + tap_len() as usize`

```
// Drives VM idle-detection: keep the guest scheduled while any
// masquerade flow is live or a reply is still queued.
```

## L1573-1574 · `pub fn reset_sessions() {`

```
/// Tear down NAT state when a microvm run ends so the next launch —
/// and the host's own networking — start clean.
```

## L1577-1578 · `crate::net::reset_poll_guard();`

```
// Belt-and-suspenders: clear the shared NIC-drain guard so a microvm run
// can never leave the HOST's own networking (DNS / OTA) bricked.
```

## L1580 · `}`

```
// The COUNTERS deliberately survive teardown — see `reset_counters`.
```

## L1583-1590 · `pub fn reset_counters() {`

```
/// Zero the bridge counters. At VM **start**, not at teardown.
///
/// They used to be zeroed here on the way out, which meant the numbers existed
/// only while the guest was alive: close the browser, ask `netstat` what
/// happened, get nothing. The one moment anyone wants a post-mortem is right
/// after the thing died, and that was exactly the moment the evidence was
/// erased. Zeroing on the way IN gives every run a clean window and leaves the
/// last run readable until the next launch.
```

