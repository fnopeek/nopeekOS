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

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;
use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;

/// Locally-administered guest MAC. Must match the one virtio-net
/// advertises through its device-cfg MAC field.
pub const GUEST_MAC:   [u8; 6] = [0x52, 0x54, 0x00, 0x6E, 0x70, 0x6B];
/// MAC the host pretends to be on the synthetic gateway.
pub const GATEWAY_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x6E, 0x70, 0x01];
/// Synthetic gateway IP. ARP, DNS, and (later) NAT all live here.
pub const GATEWAY_IP:  [u8; 4] = [10, 99, 0, 1];
/// Guest IP — PID-1 hard-codes the same value via SIOCSIFADDR.
pub const GUEST_IP:    [u8; 4] = [10, 99, 0, 2];

const VNET_HDR_LEN: usize = 12;
const ETH_HDR_LEN:  usize = 14;
const ARP_LEN:      usize = 28;
const IPV4_HDR_LEN: usize = 20;
const UDP_HDR_LEN:  usize = 8;
const DNS_HDR_LEN:  usize = 12;

const ETHERTYPE_IPV4: u16 = 0x0800;
const ETHERTYPE_ARP:  u16 = 0x0806;

const PROTO_ICMP: u8 = 1;
const PROTO_UDP:  u8 = 17;
const PROTO_TCP:  u8 = 6;
const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY:   u8 = 0;

const PORT_DNS: u16 = 53;

/// TCP header length without options — used to bound L4 checksum
/// recompute in the L3 path.
const TCP_HDR_LEN: usize = 20;

/// TCP flags the outbound segmenter has to carry correctly across a split
/// (`tcp_gso_segment`, net/ipv4/tcp_offload.c).
const TCP_FIN: u8 = 0x01;
const TCP_PSH: u8 = 0x08;
const TCP_CWR: u8 = 0x80;
/// virtio-net header gso_type for a TCPv4 super-frame on guest TX (the ECN flag
/// 0x80 is OR'd on top and must be masked off before comparing).
const VNET_HDR_GSO_TCPV4: u8 = 1;
const VNET_HDR_GSO_ECN: u8 = 0x80;

/// Per-VM network policy. The browser default (`dns_tcp`) allows DNS +
/// TCP + UDP (QUIC) through the L3 masquerade; ICMP still needs an
/// explicit cap. Future work threads this through the microvm session
/// cap so different apps can have different policies.
#[derive(Clone, Copy, Debug)]
pub struct NetCaps {
    pub allow_dns:  bool,
    pub allow_icmp: bool,
    pub allow_udp:  bool,
    pub allow_tcp:  bool,
}

impl NetCaps {
    pub const fn dns_only() -> Self {
        Self { allow_dns: true, allow_icmp: false, allow_udp: false, allow_tcp: false }
    }
    /// Browser default: DNS + TCP + UDP (QUIC/HTTP-3) + ICMP echo (ping /
    /// reachability probes) via L3 masquerade.
    pub const fn dns_tcp() -> Self {
        Self { allow_dns: true, allow_icmp: true, allow_udp: true, allow_tcp: true }
    }
}

impl Default for NetCaps {
    fn default() -> Self { Self::dns_tcp() }
}

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

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtOrd};

const L3_MAX: usize = 1024;

// ── Throughput / NAT-usage instrumentation (page-load perf diagnosis) ──
// Cheap relaxed counters; `pump` prints a one-line host-side `[netstat]`
// summary every ~5 s while a VM is active (NOT guest kmsg → no [guest] spam),
// so we can see whether page-load slowness is throughput, NAT-table drops, or
// latency. Window counters reset each summary; HIGHWATER + DROPS are lifetime.
static NS_RX_BYTES: AtomicU64 = AtomicU64::new(0);
static NS_RX_PKTS: AtomicU64 = AtomicU64::new(0);
static NS_TX_BYTES: AtomicU64 = AtomicU64::new(0);
static NS_TX_PKTS: AtomicU64 = AtomicU64::new(0);
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
static NS_GUEST_KICKS: AtomicU64 = AtomicU64::new(0);   // guest rang the TX doorbell
static NS_GUEST_FRAMES: AtomicU64 = AtomicU64::new(0);  // frames taken off its TX ring
static NS_GUEST_ARP: AtomicU64 = AtomicU64::new(0);     // …of which ARP
static NS_GUEST_OTHER: AtomicU64 = AtomicU64::new(0);   // …neither ARP nor IPv4 (IPv6…)
/// Tick the guest started, so a report can say how long it has had to speak.
static NS_START_TICK: AtomicU64 = AtomicU64::new(0);

pub fn note_guest_kick() { NS_GUEST_KICKS.fetch_add(1, AtOrd::Relaxed); }

/// Where a guest IPv4 frame goes when it does NOT come out the other side.
/// `handle_ipv4` has five silent `return None`s; between them they can swallow
/// every packet a guest sends and leave the report showing a healthy zero in
/// every loss column. Counted, so the gap between "frames in" and "packets out"
/// has to name itself.
static NS_IP_MALFORMED: AtomicU64 = AtomicU64::new(0); // length / total-len clamp
static NS_IP_TO_GW: AtomicU64 = AtomicU64::new(0);     // addressed to the gateway, not DNS
static NS_IP_DNS: AtomicU64 = AtomicU64::new(0);       // answered (or queued) by our resolver
static NS_IP_PROTO: AtomicU64 = AtomicU64::new(0);     // not TCP / UDP / ICMP
/// The rest of the silent exits, outbound and in. Every one of these could
/// swallow a guest's whole session while the report showed zeroes everywhere.
static NS_TX_RUNT: AtomicU64 = AtomicU64::new(0);      // frame shorter than vnet+eth
static NS_TX_BADTCP: AtomicU64 = AtomicU64::new(0);    // emit_tcp_out bailed on the header
static NS_TX_ARPMISS: AtomicU64 = AtomicU64::new(0);   // went out to L2 broadcast
static NS_TX_RINGBAD: AtomicU64 = AtomicU64::new(0);   // guest TX queue unusable
static NS_TX_TRUNC: AtomicU64 = AtomicU64::new(0);     // descriptor chain broke mid-frame
/// An inbound TCP segment addressed to a port in OUR masquerade range that
/// matched no mapping. It does not stop here: `tap_inbound` returns false, the
/// host stack takes it, finds no socket, and answers the server with a RST
/// (tcp.rs:894). That is our own machine tearing down the guest's connection.
/// A page that loads for a second and then dies looks exactly like this, and
/// nothing counted it.
static NS_RX_UNMATCHED: AtomicU64 = AtomicU64::new(0);

pub fn note_tx_ring_bad() { NS_TX_RINGBAD.fetch_add(1, AtOrd::Relaxed); }
pub fn note_tx_truncated() { NS_TX_TRUNC.fetch_add(1, AtOrd::Relaxed); }

static NS_DROP_TABLE: AtomicU64 = AtomicU64::new(0);  // L3 masquerade table full
static NS_DROP_EGRESS: AtomicU64 = AtomicU64::new(0); // host NIC refused the frame
static NS_HIGHWATER: AtomicU64 = AtomicU64::new(0);
static NS_LAST_TICK: AtomicU64 = AtomicU64::new(0);
/// The guest RX ring had no buffer posted, so the frame stayed in the tap.
/// High = the GUEST is the limiter, not us.
static NS_INJECT_FALSE: AtomicU64 = AtomicU64::new(0);
/// New-flow counts by transport, to spot QUIC: a cold page that opens lots of
/// UDP flows is using HTTP/3.
static NS_TCP_FLOWS: AtomicU64 = AtomicU64::new(0);
static NS_UDP_FLOWS: AtomicU64 = AtomicU64::new(0);

/// Last successful RX inject (TSC). Drives the BSP vCPU's idle park decision:
/// while RX is recently active the vCPU parks event-driven on the host NIC RX
/// IRQ (`irq_wait`) instead of a blind 10 ms timer sleep, so a download lull is
/// woken the moment the next batch arrives (drainmax 10 ms → ~sub-ms).
static NS_LAST_ACTIVITY: AtomicU64 = AtomicU64::new(0);

/// GPU-throttle signal: TSC of the last *bulk* RX frame (a GRO superframe >4 KB,
/// only produced by a sustained download — browsing/idle frames are <1500 B).
/// The virtio-gpu framebuffer copy runs INLINE on the vCPU exit (steals net-
/// processing cycles + the memory bus); while this is recent it backs off from
/// ~30 fps to ~8 fps so the download isn't throttled by pixel copies. Florian's
/// "smaller window / hidden desktop = faster download" observation exposed the
/// coupling. Probe to size the win before the full off-vCPU GPU copy.
static DL_LAST_BULK_TSC: AtomicU64 = AtomicU64::new(0);

/// True if a bulk RX frame arrived in the last ~250 ms (= an active download).
pub fn download_active() -> bool {
    let last = DL_LAST_BULK_TSC.load(AtOrd::Relaxed);
    if last == 0 { return false; }
    let win = (crate::interrupts::tsc_freq() / 1000) * 250; // 250 ms in TSC ticks
    crate::interrupts::rdtsc().wrapping_sub(last) < win
}

/// True if RX delivered a packet in the last ~50 ms — i.e. a download is in
/// flight and the BSP vCPU should wake on the host NIC RX IRQ rather than
/// deep-parking on the 100 Hz worker timer.
pub fn recently_active() -> bool {
    let now = crate::interrupts::rdtsc();
    let window = crate::interrupts::tsc_freq() / 20; // ~50 ms in TSC
    now.wrapping_sub(NS_LAST_ACTIVITY.load(AtOrd::Relaxed)) < window
}

/// Mark the data plane active NOW (a frame moved RX or TX). The off-vCPU
/// `net_dataplane` worker calls this each pass it does real work, so
/// `recently_active()` gates its halt-poll.
pub fn mark_active() {
    NS_LAST_ACTIVITY.store(crate::interrupts::rdtsc(), AtOrd::Relaxed);
}

/// Masquerade host-port pool. Strictly below the host TCP stack's own
/// ephemeral range (49152..=65534, net/tcp.rs) so a guest flow can
/// never alias a host-originated connection (OTA `update`, `https`).
const L3_PORT_LO: u16 = 20000;
const L3_PORT_HI: u16 = 40000;
const L3_TCP_IDLE_TICKS: u64 = 12_000; // ~2 min  (~100 ticks/s)
const L3_UDP_IDLE_TICKS: u64 = 3_000;  // ~30 s

#[derive(Clone, Copy)]
struct L3Map {
    proto: u8,
    guest_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    /// The host address this flow went out with. NOT `arp::our_ip()` at read
    /// time: a DHCP renewal or a carrier blink mid-session changes that, and a
    /// mapping keyed on "the address we happen to hold now" is silently orphaned
    /// the moment it moves — outbound leaves under a port the server never saw,
    /// inbound is discarded before anything looks at it. The flow is keyed on
    /// the address it was BORN with, which is the address the replies carry.
    host_ip: [u8; 4],
    host_port: u16,
    last_tick: u64,
}

/// Masquerade table plus a bit per host port in `[L3_PORT_LO, L3_PORT_HI)`.
/// One lock over both, so the index can never disagree with the table.
struct L3Table {
    maps: [Option<L3Map>; L3_MAX],
    used: [u64; PORT_WORDS],
}

const PORT_RANGE: usize = (L3_PORT_HI - L3_PORT_LO) as usize;
const PORT_WORDS: usize = PORT_RANGE.div_ceil(64);
/// `nf_nat_l4proto_unique_tuple` probes at most this many ports before giving
/// up and re-rolling the offset — "we are in softirq; doing a search of the
/// entire range risks soft lockup when all tuples are already used".
const NAT_MAX_ATTEMPTS: usize = 128;

impl L3Table {
    const fn new() -> Self {
        L3Table { maps: [const { None }; L3_MAX], used: [0; PORT_WORDS] }
    }
    #[inline]
    fn port_used(&self, hp: u16) -> bool {
        let i = (hp - L3_PORT_LO) as usize;
        self.used[i / 64] & (1u64 << (i % 64)) != 0
    }
    #[inline]
    fn set_port(&mut self, hp: u16, on: bool) {
        let i = (hp - L3_PORT_LO) as usize;
        let (w, b) = (i / 64, 1u64 << (i % 64));
        if on { self.used[w] |= b; } else { self.used[w] &= !b; }
    }
    /// Free slot `i` and its port together.
    fn release(&mut self, i: usize) {
        if let Some(m) = self.maps[i].take() { self.set_port(m.host_port, false); }
    }
}

static L3: Mutex<L3Table> = Mutex::new(L3Table::new());
/// Gates the host-RX inbound intercept. Off ⇒ `tap_inbound` is a cheap
/// `false` so a guest-less host (plain OTA/https) is never touched.
static L3_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Recycled frame buffers for the inbound staging path. Each download packet
/// used to `vec![0u8; ~1514]` (allocator free-list walk + memset) and free it
/// after injection — the dominant inbound per-packet cost (~2.6µs/pkt: the
/// first-fit allocator walks an O(n) free list under churn, plus the memset).
/// Linux solves this with skb pools; we keep a small ring of buffers that the
/// producer (tap_inbound) borrows and the consumer (the worker) returns, so
/// after warmup the datapath does ZERO heap alloc/free — only the unavoidable
/// payload memcpy. Bounded so it can't grow without limit; only used while a VM
/// is active (the BSP is the sole accessor, so the lock is uncontended).
static FRAME_POOL: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
const FRAME_POOL_MAX: usize = TAP_RING + 16;
const FRAME_BUF_CAP: usize = 2048; // ≥ vnet+eth+MTU, so resize never reallocs

/// Borrow a frame buffer sized to `len` from the pool (or allocate once if the
/// pool is cold). The contents are uninitialised beyond what the caller writes —
/// tap_inbound overwrites every byte (vnet hdr + eth hdr + full IP copy).
fn frame_pool_get(len: usize) -> Vec<u8> {
    let mut buf = FRAME_POOL.lock().pop()
        .unwrap_or_else(|| Vec::with_capacity(FRAME_BUF_CAP.max(len)));
    buf.clear();
    if buf.capacity() < len { buf.reserve(len - buf.capacity()); }
    // SAFETY: capacity ≥ len after the reserve above. The caller writes all
    // `len` bytes before the buffer is read (vnet[0..12]=0, eth[12..26],
    // ip-copy[26..len]), so no uninitialised byte is ever observed.
    unsafe { buf.set_len(len); }
    buf
}

/// Return a frame buffer to the pool for reuse (dropped if the pool is full).
fn frame_pool_put(buf: Vec<u8>) {
    // GRO superframes grow well past a normal frame; don't pool them or we'd
    // hand a 60 KB buffer back for a 1.5 KB frame forever.
    if buf.capacity() > FRAME_BUF_CAP { return; }
    let mut pool = FRAME_POOL.lock();
    if pool.len() < FRAME_POOL_MAX { pool.push(buf); }
}
/// Find an existing mapping for this guest flow or allocate one.
/// Returns the masquerade host port.
/// The masquerade table is full: no new flow can open. Budgeted, because if it
/// fires it fires for every packet of every new connection — and a log that
/// writes the flood is no longer a log. `netstat` carries the running count.
fn note_table_full() {
    let n = NS_DROP_TABLE.fetch_add(1, AtOrd::Relaxed);
    if n < 4 {
        kprintln!("[nat] masquerade table full ({} flows) - no new connection \
                   can open; see netstat", L3_MAX);
    }
}

fn l3_map_out(proto: u8, gport: u16, rip: [u8; 4], rport: u16, now: u64) -> Option<u16> {
    let our_ip = crate::net::arp::our_ip();
    let mut tbl = L3.lock();
    // Existing? A hit whose `host_ip` is no longer ours describes a flow the
    // far end can no longer answer — the address moved. Retire it here instead
    // of leaving it to time out; the caller gets a fresh mapping on the new
    // address, which is the only thing that can still work.
    for i in 0..L3_MAX {
        let Some(m) = tbl.maps[i].as_mut() else { continue };
        if m.proto == proto && m.guest_port == gport
            && m.remote_ip == rip && m.remote_port == rport
        {
            if m.host_ip == our_ip {
                m.last_tick = now;
                return Some(m.host_port);
            }
            tbl.release(i);
            NS_MAP_REHOMED.fetch_add(1, AtOrd::Relaxed);
            break;
        }
    }
    // `nf_nat_l4proto_unique_tuple`: start at a varying offset, probe forward
    // with an O(1) used-test, and give up after a BOUNDED number of attempts
    // (then re-roll once). The old code walked all 1024 entries per candidate
    // port — ~10^6 comparisons under the lock for one new flow on a full table,
    // and a browser opens ~65 UDP flows per page.
    let mut off = (crate::interrupts::rdtsc() as usize) % PORT_RANGE;
    let mut hp: Option<u16> = None;
    for _round in 0..2 {
        for i in 0..NAT_MAX_ATTEMPTS.min(PORT_RANGE) {
            let cand = L3_PORT_LO + ((off + i) % PORT_RANGE) as u16;
            if !tbl.port_used(cand) { hp = Some(cand); break; }
        }
        if hp.is_some() { break; }
        off = (off + PORT_RANGE / 2 + 1) % PORT_RANGE;
    }
    let hp = hp?;
    let slot = (0..L3_MAX).find(|&i| tbl.maps[i].is_none())?;
    tbl.maps[slot] = Some(L3Map { proto, guest_port: gport, remote_ip: rip,
                                   remote_port: rport, host_ip: our_ip,
                                   host_port: hp, last_tick: now });
    tbl.set_port(hp, true);
    // New flow — count by transport (UDP-heavy cold load = QUIC/HTTP-3).
    match proto {
        PROTO_TCP => { NS_TCP_FLOWS.fetch_add(1, AtOrd::Relaxed); }
        PROTO_UDP => { NS_UDP_FLOWS.fetch_add(1, AtOrd::Relaxed); }
        _ => {}
    }
    Some(hp)
}

/// Reverse lookup for an inbound reply: (proto, host_port) + remote
/// must match. Returns the guest port to deliver to.
fn l3_map_in(proto: u8, dst_ip: [u8; 4], hport: u16, rip: [u8; 4], rport: u16,
             now: u64) -> Option<u16> {
    let mut tbl = L3.lock();
    for m in tbl.maps.iter_mut().flatten() {
        if m.proto == proto && m.host_port == hport && m.host_ip == dst_ip
            && m.remote_ip == rip && m.remote_port == rport
        {
            m.last_tick = now;
            return Some(m.guest_port);
        }
    }
    None
}

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
fn fix_l4_checksum(proto: u8, src_ip: [u8; 4], dst_ip: [u8; 4], l4: &mut [u8]) {
    if proto == PROTO_TCP {
        if l4.len() < TCP_HDR_LEN { return; }
        l4[16] = 0; l4[17] = 0;
        let c = tcp_checksum(src_ip, dst_ip, l4);
        l4[16..18].copy_from_slice(&c.to_be_bytes());
    } else if proto == PROTO_UDP {
        if l4.len() < UDP_HDR_LEN { return; }
        // The length FIELD is the authority, not the slice: a minimum-size
        // ethernet frame carries padding that is not part of the datagram, and
        // checksumming it would produce a value the receiver cannot reproduce.
        let declared = u16::from_be_bytes([l4[4], l4[5]]) as usize;
        let len = if declared >= UDP_HDR_LEN && declared <= l4.len() {
            declared
        } else {
            l4.len()
        };
        l4[6] = 0; l4[7] = 0;
        let c = udp_checksum(src_ip, dst_ip, &l4[..len]);
        l4[6..8].copy_from_slice(&c.to_be_bytes());
    }
}

/// UDP checksum over the IPv4 pseudo-header + datagram (RFC 768). A computed
/// zero goes on the wire as 0xFFFF, because zero is the "no checksum" escape and
/// would undo the whole point.
fn udp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], udp: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
    sum += PROTO_UDP as u32;
    sum += udp.len() as u32;
    let mut i = 0;
    while i + 1 < udp.len() {
        sum += u16::from_be_bytes([udp[i], udp[i + 1]]) as u32;
        i += 2;
    }
    if i < udp.len() {
        sum += (udp[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let c = !(sum as u16);
    if c == 0 { 0xFFFF } else { c }
}

/// Incremental ones-complement checksum update (RFC 1624). Adjust an existing
/// checksum for a set of changed 16-bit words in O(changes) instead of
/// recomputing over the whole segment — `HC' = ~(~HC + Σ(~old + new))`. NAT only
/// rewrites the IP + port (≤3 words), so this replaces the full ~1500-byte
/// `tcp_checksum` loop that dominated the inbound per-packet cost (~2.6µs/pkt).
fn csum_update(old_check: u16, changes: &[(u16, u16)]) -> u16 {
    let mut sum = (!old_check) as u32;
    for &(old, new) in changes {
        sum += (!old) as u32 & 0xFFFF;
        sum += new as u32;
    }
    while sum >> 16 != 0 { sum = (sum & 0xFFFF) + (sum >> 16); }
    !(sum as u16)
}

/// Outbound SNAT: rewrite the guest's L4 source port to a masquerade
/// host port and send from our host IP. The guest's TCP/UDP semantics
/// (seq/ack/window/options/QUIC) pass through untouched.
fn l3_outbound(proto: u8, src_port: u16, dst_ip: [u8; 4],
                dst_port: u16, l4: &[u8], gso_size: u16) -> Option<Vec<u8>> {
    let now = crate::interrupts::ticks();
    let hp = match l3_map_out(proto, src_port, dst_ip, dst_port, now) {
        Some(p) => p,
        None => { note_table_full(); return None; }
    };
    let our_ip = crate::net::arp::our_ip();
    if proto == PROTO_TCP {
        // TCP: software TSO segmentation (gso_size > 0 + payload past one MSS) or
        // a single segment. Either way the TCP checksum is recomputed in full —
        // with VIRTIO_NET_F_CSUM negotiated the guest now offloads its checksum
        // (CHECKSUM_PARTIAL: the field holds only the pseudo-header seed), so the
        // old incremental update has no valid base. Full recompute is correct
        // whether or not CSUM is on and is required per-segment anyway.
        emit_tcp_out(hp, our_ip, dst_ip, l4, gso_size);
    } else {
        // UDP (and anything else routed here): single datagram, port rewrite +
        // checksum fix-up, no segmentation.
        NS_TX_PKTS.fetch_add(1, AtOrd::Relaxed);
        NS_TX_BYTES.fetch_add(l4.len() as u64, AtOrd::Relaxed);
        let mut seg = l4.to_vec();
        seg[0..2].copy_from_slice(&hp.to_be_bytes());        // src port → host port
        fix_l4_checksum(proto, our_ip, dst_ip, &mut seg);
        if !crate::net::ipv4::send(dst_ip, proto, &seg) { NS_TX_ARPMISS.fetch_add(1, AtOrd::Relaxed); }
    }
    L3_ACTIVE.store(true, AtOrd::Release);
    None
}

/// Emit a guest TCP segment outbound, software-segmenting a GSO/TSO super-frame
/// into `gso_size`-byte segments when needed. Ports the field math of Linux's
/// `tcp_gso_segment` (net/ipv4/tcp_offload.c): per segment, seq advances by the
/// payload already emitted; FIN/PSH are kept only on the last; CWR is cleared on
/// every segment but the first; the TCP checksum is computed in full over each
/// independent segment. The IP layer (`ipv4::send`) builds the per-segment IPv4
/// header (src/total-len/checksum), so we only fix up the TCP header here.
fn emit_tcp_out(hp: u16, our_ip: [u8; 4], dst_ip: [u8; 4],
                l4: &[u8], gso_size: u16) {
    if l4.len() < TCP_HDR_LEN { NS_TX_BADTCP.fetch_add(1, AtOrd::Relaxed); return; }
    let thlen = ((l4[12] >> 4) & 0x0F) as usize * 4;
    if thlen < TCP_HDR_LEN || l4.len() < thlen {
        NS_TX_BADTCP.fetch_add(1, AtOrd::Relaxed); return;
    }
    let payload = &l4[thlen..];
    let mss = gso_size as usize;

    // Non-GSO (or fits in one MSS): single segment.
    if mss == 0 || payload.len() <= mss {
        let mut seg = l4.to_vec();
        seg[0..2].copy_from_slice(&hp.to_be_bytes());        // src port → host port
        seg[16] = 0; seg[17] = 0;
        let c = tcp_checksum(our_ip, dst_ip, &seg);
        seg[16..18].copy_from_slice(&c.to_be_bytes());
        NS_TX_PKTS.fetch_add(1, AtOrd::Relaxed);
        NS_TX_BYTES.fetch_add(seg.len() as u64, AtOrd::Relaxed);
        if !crate::net::ipv4::send(dst_ip, PROTO_TCP, &seg) { NS_TX_ARPMISS.fetch_add(1, AtOrd::Relaxed); }
        return;
    }

    let base_seq = u32::from_be_bytes([l4[4], l4[5], l4[6], l4[7]]);
    let orig_flags = l4[13];
    let n = payload.len().div_ceil(mss);
    let mut off = 0usize;
    for k in 0..n {
        let this = (payload.len() - off).min(mss);
        let is_last = k == n - 1;

        let mut seg = Vec::with_capacity(thlen + this);
        seg.extend_from_slice(&l4[..thlen]);                  // verbatim TCP header
        seg.extend_from_slice(&payload[off..off + this]);     // this segment's data

        seg[0..2].copy_from_slice(&hp.to_be_bytes());         // src port → host port
        let seq = base_seq.wrapping_add(off as u32);          // seq += bytes emitted
        seg[4..8].copy_from_slice(&seq.to_be_bytes());

        // FIN/PSH only on the last segment; CWR only on the first.
        let mut flags = orig_flags;
        if !is_last { flags &= !(TCP_FIN | TCP_PSH); }
        if k != 0 { flags &= !TCP_CWR; }
        seg[13] = flags;

        seg[16] = 0; seg[17] = 0;                             // full TCP checksum
        let c = tcp_checksum(our_ip, dst_ip, &seg);
        seg[16..18].copy_from_slice(&c.to_be_bytes());

        NS_TX_PKTS.fetch_add(1, AtOrd::Relaxed);
        NS_TX_BYTES.fetch_add(seg.len() as u64, AtOrd::Relaxed);
        if !crate::net::ipv4::send(dst_ip, PROTO_TCP, &seg) { NS_TX_ARPMISS.fetch_add(1, AtOrd::Relaxed); }
        off += this;
    }
}

/// Outbound NAT for a guest ICMP echo request (so ping + the browser's
/// reachability probes to the DNS servers work instead of cap-rejecting). The
/// ICMP Identifier is the flow key, masqueraded like a port; only echo
/// requests (type 8) are forwarded, everything else dropped. Mirrors
/// `l3_outbound` but rewrites the id + uses the pseudo-header-less ICMP
/// checksum.
fn l3_icmp_outbound(dst_ip: [u8; 4], l4: &[u8]) -> Option<Vec<u8>> {
    if l4.len() < 8 || l4[0] != ICMP_ECHO_REQUEST {
        return None;
    }
    let guest_id = u16::from_be_bytes([l4[4], l4[5]]);
    let now = crate::interrupts::ticks();
    let hp = l3_map_out(PROTO_ICMP, guest_id, dst_ip, 0, now)?;
    let mut seg = l4.to_vec();
    seg[4..6].copy_from_slice(&hp.to_be_bytes()); // id → masquerade id
    fix_icmp_checksum(&mut seg);
    if !crate::net::ipv4::send(dst_ip, PROTO_ICMP, &seg) { NS_TX_ARPMISS.fetch_add(1, AtOrd::Relaxed); }
    L3_ACTIVE.store(true, AtOrd::Release);
    None
}

/// ICMP checksum: ones-complement sum over the whole ICMP message (no pseudo-
/// header, unlike TCP/UDP). Zero the field first, then fold carries.
fn fix_icmp_checksum(icmp: &mut [u8]) {
    if icmp.len() < 4 {
        return;
    }
    icmp[2] = 0;
    icmp[3] = 0;
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < icmp.len() {
        sum += u16::from_be_bytes([icmp[i], icmp[i + 1]]) as u32;
        i += 2;
    }
    if i < icmp.len() {
        sum += (icmp[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let c = !(sum as u16);
    icmp[2..4].copy_from_slice(&c.to_be_bytes());
}

/// Host-RX intercept. `ip` is a full IPv4 packet already filtered to
/// our IP. If it matches a masquerade mapping, rewrite it back to the
/// guest, enqueue for `pump`, and return true (consume — the host
/// stack must NOT also process it). Cheap `false` when no VM is up.
/// Return a frame buffer (from `tap_inbound`) to the recycle pool after it has
/// been injected into the guest ring — no per-packet heap churn.
pub fn recycle_frame(buf: Vec<u8>) { frame_pool_put(buf); }

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

/// tun's `dev->tx_queue_len = TUN_READQ_SIZE` is 500. 512 keeps the power of two.
const TAP_RING: usize = 512;

struct Tap {
    slots: [Option<Vec<u8>>; TAP_RING],
    head: usize, // consumer
    tail: usize, // producer
    len: usize,
}

impl Tap {
    const fn new() -> Self {
        Tap { slots: [const { None }; TAP_RING], head: 0, tail: 0, len: 0 }
    }
}

static TAP: Mutex<Tap> = Mutex::new(Tap::new());
/// Lock-free depth, so the worker's "is there work" test takes no lock.
static TAP_LEN: AtomicU64 = AtomicU64::new(0);
/// `tun_net_xmit`'s `tx_dropped`: the ring was full. This is BACKPRESSURE and
/// healthy in moderation — it is not the masquerade table filling up and not an
/// egress refusal, and the whole point of counting the three apart is that from
/// outside all three look like "throughput sagged" (see the note at the top).
static NS_TAP_FULL: AtomicU64 = AtomicU64::new(0);
/// Frames actually handed to the guest. PROGRESS, not fill level: a ring that
/// sits at 40 of 512 tells you nothing, a delivered-count that stops moving
/// tells you everything.
static NS_TAP_DELIVERED: AtomicU64 = AtomicU64::new(0);
/// Flows retired because the host address moved under them (DHCP renewal,
/// carrier blink). Zero on a healthy link; non-zero explains a stall that looks
/// like the far end went quiet.
static NS_MAP_REHOMED: AtomicU64 = AtomicU64::new(0);
/// The worker is parked on its doorbell. Linux's wait queue: `sk_data_ready`
/// wakes nobody when no reader sleeps there, so a producer feeding a RUNNING
/// consumer sends no wakeup at all. Without this the empty→occupied edge would
/// IPI a busy-polling worker at line rate.
static WORKER_PARKED: AtomicBool = AtomicBool::new(false);

/// The worker announces whether it is about to sleep on the tap.
pub fn set_worker_parked(parked: bool) { WORKER_PARKED.store(parked, AtOrd::SeqCst); }

/// Lock-free depth for the worker's poll condition.
pub fn tap_len() -> u64 { TAP_LEN.load(AtOrd::Relaxed) }

/// `ptr_ring_produce` + `sk_data_ready`. False ⇒ the ring was full and the frame
/// was dropped (counted); the buffer goes back to the pool either way.
fn tap_push(frame: Vec<u8>) -> bool {
    let mut t = TAP.lock();
    if t.len == TAP_RING {
        drop(t);
        NS_TAP_FULL.fetch_add(1, AtOrd::Relaxed);
        frame_pool_put(frame);
        return false;
    }
    let was_empty = t.len == 0;
    let tail = t.tail;
    t.slots[tail] = Some(frame);
    t.tail = (tail + 1) % TAP_RING;
    t.len += 1;
    TAP_LEN.store(t.len as u64, AtOrd::Relaxed);
    drop(t);
    // Wake the reader only on the empty→occupied edge, and only if one is
    // actually asleep. During a burst the ring stays occupied, so a burst costs
    // one wakeup, not one per frame.
    if was_empty && WORKER_PARKED.load(AtOrd::SeqCst) {
        if let Some(c) = super::net_backend::worker_core() {
            // Bumps the target core's kick generation BEFORE the IPI, so a wake
            // racing the park is never lost — the scheduler re-tests the
            // generation every scan and finds the fiber runnable.
            crate::smp::kick_host_core(c);
        }
    }
    true
}

/// `ptr_ring_consume`. The worker is the only caller.
pub fn tap_pop() -> Option<Vec<u8>> {
    let mut t = TAP.lock();
    if t.len == 0 { return None; }
    let head = t.head;
    let f = t.slots[head].take();
    t.head = (head + 1) % TAP_RING;
    t.len -= 1;
    TAP_LEN.store(t.len as u64, AtOrd::Relaxed);
    f
}

/// `vhost_discard_vq_desc`: `inject_rx` rolled its descriptors back, so this
/// frame was never consumed. Put it back at the head — order matters on a TCP
/// stream. Only the worker calls this, between a pop and the next pop.
pub fn tap_push_front(frame: Vec<u8>) {
    let mut t = TAP.lock();
    if t.len == TAP_RING {
        drop(t);
        NS_TAP_FULL.fetch_add(1, AtOrd::Relaxed);
        frame_pool_put(frame);
        return;
    }
    t.head = (t.head + TAP_RING - 1) % TAP_RING;
    let head = t.head;
    t.slots[head] = Some(frame);
    t.len += 1;
    TAP_LEN.store(t.len as u64, AtOrd::Relaxed);
}

/// One frame reached the guest.
pub fn note_tap_delivered() { NS_TAP_DELIVERED.fetch_add(1, AtOrd::Relaxed); }

/// Empty the tap (VM teardown), returning every buffer to the pool.
pub fn tap_reset() {
    let mut t = TAP.lock();
    while t.len > 0 {
        let head = t.head;
        if let Some(f) = t.slots[head].take() { frame_pool_put(f); }
        t.head = (head + 1) % TAP_RING;
        t.len -= 1;
    }
    t.head = 0;
    t.tail = 0;
    TAP_LEN.store(0, AtOrd::Relaxed);
    WORKER_PARKED.store(false, AtOrd::SeqCst);
}

/// THE inbound acceptance test, and the only translation. Called from
/// `ipv4::handle_ipv4` for every received IPv4 packet, BEFORE the "is this
/// addressed to the address we happen to hold right now" filter: a guest flow is
/// keyed on the address it went out with, and asking the other question first
/// discards the reply before anything has looked at it.
///
/// Returns true if the packet was guest traffic and is now the tap's problem —
/// the host stack must not also process it. A cheap `false` when no VM is up.
pub fn tap_inbound(ip: &[u8]) -> bool {
    if !L3_ACTIVE.load(AtOrd::Acquire) { return false; }
    if ip.len() < IPV4_HDR_LEN { return false; }
    let ihl = (ip[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HDR_LEN || ip.len() < ihl { return false; }
    let proto = ip[9];
    if proto != PROTO_TCP && proto != PROTO_UDP && proto != PROTO_ICMP { return false; }
    let src_ip: [u8; 4] = ip[12..16].try_into().unwrap();
    let dst_ip: [u8; 4] = ip[16..20].try_into().unwrap();
    let l4 = &ip[ihl..];
    let (remote_port, host_port) = if proto == PROTO_ICMP {
        if l4.len() < 8 || l4[0] != ICMP_ECHO_REPLY { return false; }
        (0u16, u16::from_be_bytes([l4[4], l4[5]]))
    } else {
        if l4.len() < 4 { return false; }
        (u16::from_be_bytes([l4[0], l4[1]]), u16::from_be_bytes([l4[2], l4[3]]))
    };
    let now = crate::interrupts::ticks();
    let gport = match l3_map_in(proto, dst_ip, host_port, src_ip, remote_port, now) {
        Some(g) => g,
        None => {
            // Not ours by the mapping. A TCP port inside our masquerade range is
            // not the host's either, and the host stack answers it with a RST —
            // our own machine tearing down the guest's connection.
            if proto == PROTO_TCP && (L3_PORT_LO..L3_PORT_HI).contains(&host_port) {
                NS_RX_UNMATCHED.fetch_add(1, AtOrd::Relaxed);
            }
            return false;
        }
    };
    NS_RX_PKTS.fetch_add(1, AtOrd::Relaxed);
    NS_RX_BYTES.fetch_add(ip.len() as u64, AtOrd::Relaxed);

    let mut frame = frame_pool_get(VNET_HDR_LEN + ETH_HDR_LEN + ip.len());
    frame[..VNET_HDR_LEN].fill(0); // empty virtio-net header
    write_eth(&mut frame, &GUEST_MAC, &GATEWAY_MAC, ETHERTYPE_IPV4);
    let ip_off = VNET_HDR_LEN + ETH_HDR_LEN;
    frame[ip_off..].copy_from_slice(ip);
    frame[ip_off + 16..ip_off + 20].copy_from_slice(&GUEST_IP);
    frame[ip_off + 10] = 0; frame[ip_off + 11] = 0;
    let ipc = ipv4_checksum(&frame[ip_off..ip_off + ihl]);
    frame[ip_off + 10..ip_off + 12].copy_from_slice(&ipc.to_be_bytes());
    let l4_off = ip_off + ihl;
    if proto == PROTO_ICMP {
        frame[l4_off + 4..l4_off + 6].copy_from_slice(&gport.to_be_bytes());
        fix_icmp_checksum(&mut frame[l4_off..]);
    } else if proto == PROTO_TCP && frame[l4_off..].len() >= TCP_HDR_LEN {
        // Incremental TCP checksum (RFC 1624): only dst IP + dst port changed.
        let old_check = u16::from_be_bytes([frame[l4_off + 16], frame[l4_off + 17]]);
        let new_check = csum_update(old_check, &[
            (u16::from_be_bytes([dst_ip[0], dst_ip[1]]), u16::from_be_bytes([GUEST_IP[0], GUEST_IP[1]])),
            (u16::from_be_bytes([dst_ip[2], dst_ip[3]]), u16::from_be_bytes([GUEST_IP[2], GUEST_IP[3]])),
            (host_port, gport),
        ]);
        frame[l4_off + 2..l4_off + 4].copy_from_slice(&gport.to_be_bytes());
        frame[l4_off + 16..l4_off + 18].copy_from_slice(&new_check.to_be_bytes());
    } else {
        frame[l4_off + 2..l4_off + 4].copy_from_slice(&gport.to_be_bytes());
        fix_l4_checksum(proto, src_ip, GUEST_IP, &mut frame[l4_off..]);
    }
    tap_push(frame);
    true
}

/// Which data path this kernel is running, for `netstat`. QEMU, the NUC and the
/// notebook must all print the SAME id — that, not any throughput number, is
/// the acceptance of the rebuild: one path, taken by every machine.
pub fn path_id() -> &'static str { "tap-v1" }

/// Lock-free NAT housekeeping for the full off-vCPU data plane: reap idle
/// masquerade mappings so the table can't fill over a long session. In full mode
/// the device-touching work (`tx_flush`/RX drain) is the `net_dataplane` worker's
/// job — the BSP only needs this, and it takes NO net-device lock (so the BSP
/// never contends with the worker on the hot path). The worker owns RX+TX; the
/// vCPU owns guest execution + IRQ injection. That's the single, unified path.
pub fn housekeep() {
    l3_reap(crate::interrupts::ticks());
}

/// Drop idle mappings so the table can't fill over a long session.
fn l3_reap(now: u64) {
    let mut tbl = L3.lock();
    for i in 0..L3_MAX {
        let Some(m) = tbl.maps[i].as_ref() else { continue };
        let idle = now.wrapping_sub(m.last_tick);
        let max = if m.proto == PROTO_TCP { L3_TCP_IDLE_TICKS }
                  else { L3_UDP_IDLE_TICKS };
        if idle > max { tbl.release(i); }
    }
}

/// Tear down all L3 state (VM stopped). Idempotent.
pub fn l3_reset() {
    L3_ACTIVE.store(false, AtOrd::Release);
    *L3.lock() = L3Table::new();
    tap_reset();
    *FRAME_POOL.lock() = Vec::new(); // release recycled buffers
}

/// Egress half of the tap, symmetric with [`tap_inbound`]: classify a guest TX
/// frame (virtio-net hdr + ethernet) and produce zero or more RX frames to
/// inject back. Side-effects: kprintln on
/// cap-rejects so the operator can see why a packet went nowhere.
pub fn tap_outbound(payload: &[u8], caps: &NetCaps) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if payload.len() < VNET_HDR_LEN + ETH_HDR_LEN {
        NS_TX_RUNT.fetch_add(1, AtOrd::Relaxed);
        return out;
    }
    // virtio-net header (12 B): byte 1 = gso_type, bytes 4..6 = gso_size (LE).
    // With TX-GSO the guest hands us one ≤64 KB TCPv4 super-frame; gso_size is
    // the MSS we re-segment to. Mask off the ECN flag (0x80) before comparing.
    let gso_size = if (payload[1] & !VNET_HDR_GSO_ECN) == VNET_HDR_GSO_TCPV4 {
        u16::from_le_bytes([payload[4], payload[5]])
    } else {
        0
    };
    let frame = &payload[VNET_HDR_LEN..];
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    NS_GUEST_FRAMES.fetch_add(1, AtOrd::Relaxed);

    match ethertype {
        ETHERTYPE_ARP => {
            NS_GUEST_ARP.fetch_add(1, AtOrd::Relaxed);
            if let Some(rep) = handle_arp(frame) { out.push(rep); }
        }
        ETHERTYPE_IPV4 => {
            if let Some(rep) = handle_ipv4(frame, caps, gso_size) { out.push(rep); }
        }
        _ => {
            NS_GUEST_OTHER.fetch_add(1, AtOrd::Relaxed);
            // Quiet: IPv6 / LLDP / STP / etc. — guest has nothing
            // useful to do with them on this synthetic link.
        }
    }
    out
}

/// ARP-Request for `GATEWAY_IP` → build matching ARP-Reply.
fn handle_arp(frame: &[u8]) -> Option<Vec<u8>> {
    if frame.len() < ETH_HDR_LEN + ARP_LEN { return None; }
    let arp = &frame[ETH_HDR_LEN..];
    let oper = u16::from_be_bytes([arp[6], arp[7]]);
    if oper != 1 { return None; }                   // not a request
    if &arp[24..28] != GATEWAY_IP { return None; }

    let mut sender_mac = [0u8; 6];
    sender_mac.copy_from_slice(&arp[8..14]);
    let mut sender_ip = [0u8; 4];
    sender_ip.copy_from_slice(&arp[14..18]);

    let mut reply = alloc::vec![0u8; VNET_HDR_LEN + ETH_HDR_LEN + ARP_LEN];
    write_eth(&mut reply, &sender_mac, &GATEWAY_MAC, ETHERTYPE_ARP);
    let arp_off = VNET_HDR_LEN + ETH_HDR_LEN;
    reply[arp_off + 0] = 0x00; reply[arp_off + 1] = 0x01; // htype = Ethernet
    reply[arp_off + 2] = 0x08; reply[arp_off + 3] = 0x00; // ptype = IPv4
    reply[arp_off + 4] = 6;
    reply[arp_off + 5] = 4;
    reply[arp_off + 6] = 0x00; reply[arp_off + 7] = 0x02; // oper = REPLY
    reply[arp_off +  8..arp_off + 14].copy_from_slice(&GATEWAY_MAC);
    reply[arp_off + 14..arp_off + 18].copy_from_slice(&GATEWAY_IP);
    reply[arp_off + 18..arp_off + 24].copy_from_slice(&sender_mac);
    reply[arp_off + 24..arp_off + 28].copy_from_slice(&sender_ip);
    Some(reply)
}

/// IPv4 dispatch: only UDP→10.99.0.1:53 has a real handler today.
/// Everything else logs a cap-reject and returns None.
fn handle_ipv4(frame: &[u8], caps: &NetCaps, gso_size: u16) -> Option<Vec<u8>> {
    if frame.len() < ETH_HDR_LEN + IPV4_HDR_LEN { NS_IP_MALFORMED.fetch_add(1, AtOrd::Relaxed); return None; }
    let ip = &frame[ETH_HDR_LEN..];
    let ihl = (ip[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HDR_LEN || frame.len() < ETH_HDR_LEN + ihl { NS_IP_MALFORMED.fetch_add(1, AtOrd::Relaxed); return None; }

    let proto = ip[9];
    let src_ip: [u8; 4] = ip[12..16].try_into().ok()?;
    let dst_ip: [u8; 4] = ip[16..20].try_into().ok()?;
    // Clamp L4 to the IP total-length. The guest TX buffer is bigger
    // than the packet (min-frame / driver padding); &ip[ihl..] would
    // append that garbage to every outbound segment → the server
    // misframes the response → Firefox reads a wild length → ~4 GiB
    // alloc → crash. Inbound is already clamped in net/ipv4.rs.
    let ip_total = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    if ip_total < ihl || ip_total > ip.len() { NS_IP_MALFORMED.fetch_add(1, AtOrd::Relaxed); return None; }
    let l4 = &ip[ihl..ip_total];

    match proto {
        PROTO_UDP => {
            if l4.len() < UDP_HDR_LEN { NS_IP_MALFORMED.fetch_add(1, AtOrd::Relaxed); return None; }
            let src_port = u16::from_be_bytes([l4[0], l4[1]]);
            let dst_port = u16::from_be_bytes([l4[2], l4[3]]);
            let udp_len  = u16::from_be_bytes([l4[4], l4[5]]) as usize;
            if udp_len < UDP_HDR_LEN || udp_len > l4.len() { return None; }
            let dgram = &l4[UDP_HDR_LEN..udp_len];

            if dst_ip == GATEWAY_IP && dst_port == PORT_DNS {
                if !caps.allow_dns {
                    kprintln!("[nat] DNS query dropped (cap allow_dns=false)");
                    return None;
                }
                NS_IP_DNS.fetch_add(1, AtOrd::Relaxed);
                handle_dns(src_ip, src_port, dgram)
            } else if dst_ip == GATEWAY_IP {
                NS_IP_TO_GW.fetch_add(1, AtOrd::Relaxed);
                None    // other gateway-directed UDP: nothing here
            } else {
                if !caps.allow_udp {
                    cap_reject("UDP", dst_ip, dst_port);
                    return None;
                }
                l3_outbound(PROTO_UDP, src_port, dst_ip, dst_port, l4, 0)
            }
        }
        PROTO_TCP => {
            if l4.len() < 4 { return None; }
            let src_port = u16::from_be_bytes([l4[0], l4[1]]);
            let dst_port = u16::from_be_bytes([l4[2], l4[3]]);
            if !caps.allow_tcp {
                cap_reject("TCP", dst_ip, dst_port);
                return None;
            }
            l3_outbound(PROTO_TCP, src_port, dst_ip, dst_port, l4, gso_size)
        }
        PROTO_ICMP => {
            if !caps.allow_icmp {
                cap_reject("ICMP", dst_ip, 0);
                return None;
            }
            l3_icmp_outbound(dst_ip, l4)
        }
        _ => { NS_IP_PROTO.fetch_add(1, AtOrd::Relaxed); None }
    }
}

/// rcode-relevant outcome of a lookup. The NoData vs NxDomain split is
/// load-bearing: a non-A query (AAAA / HTTPS-SVCB type 65) on a name
/// that exists MUST be NOERROR/NODATA, not NXDOMAIN. Firefox queries
/// the HTTPS RR before every connection and reads NXDOMAIN as "host
/// does not exist" → "secure site not available".
enum DnsOutcome {
    Answer([u8; 4]),  // A record
    NoData,           // NOERROR, no answer — name exists, no such RR type
    NxDomain,         // name does not exist
}

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
fn handle_dns(src_ip: [u8; 4], src_port: u16, dgram: &[u8]) -> Option<Vec<u8>> {
    let q = parse_dns_query(dgram)?;

    let outcome = if q.qtype == 1 {
        match crate::net::dns::cached(q.name.as_str()) {
            crate::net::dns::Cached::Ip(ip) => DnsOutcome::Answer(ip),
            crate::net::dns::Cached::Failed => DnsOutcome::NxDomain,
            crate::net::dns::Cached::Unknown => {
                crate::net::dns::want(q.name.as_str());
                return None;
            }
        }
    } else {
        // AAAA / HTTPS-SVCB / etc.: we don't serve the record, but the
        // name exists. NODATA — NXDOMAIN here poisons the whole host.
        DnsOutcome::NoData
    };

    let dns_payload = build_dns_reply(&q, &outcome);
    let frame = build_ipv4_udp_reply(src_ip, src_port, PORT_DNS, &dns_payload);
    let _ = outcome;
    Some(frame)
}

struct DnsQuery {
    id: u16,
    name: String,
    name_bytes: Vec<u8>,   // raw labels incl. terminating 0, for reply echo
    qtype: u16,
    qclass: u16,
}

fn parse_dns_query(dgram: &[u8]) -> Option<DnsQuery> {
    if dgram.len() < DNS_HDR_LEN { return None; }
    let id = u16::from_be_bytes([dgram[0], dgram[1]]);
    let qdcount = u16::from_be_bytes([dgram[4], dgram[5]]);
    if qdcount < 1 { return None; }

    let mut pos = DNS_HDR_LEN;
    let name_start = pos;
    let mut name = String::new();
    loop {
        if pos >= dgram.len() { return None; }
        let len = dgram[pos] as usize;
        if len == 0 { pos += 1; break; }
        if len & 0xC0 != 0 { return None; }   // pointer in query — unusual
        if pos + 1 + len > dgram.len() { return None; }
        if !name.is_empty() { name.push('.'); }
        for &b in &dgram[pos + 1..pos + 1 + len] {
            // Conservative: stringify printable ASCII only, else bail.
            if !(0x20..0x7F).contains(&b) { return None; }
            name.push(b as char);
        }
        pos += 1 + len;
    }
    let name_bytes = dgram[name_start..pos].to_vec();
    if pos + 4 > dgram.len() { return None; }
    let qtype  = u16::from_be_bytes([dgram[pos],     dgram[pos + 1]]);
    let qclass = u16::from_be_bytes([dgram[pos + 2], dgram[pos + 3]]);
    Some(DnsQuery { id, name, name_bytes, qtype, qclass })
}

fn build_dns_reply(q: &DnsQuery, out: &DnsOutcome) -> Vec<u8> {
    let (rcode, ancount): (u16, u16) = match out {
        DnsOutcome::Answer(_) => (0, 1),
        DnsOutcome::NoData    => (0, 0),
        DnsOutcome::NxDomain  => (3, 0),
    };
    let mut p = Vec::with_capacity(64);
    // Header
    p.extend_from_slice(&q.id.to_be_bytes());
    let flags: u16 = 0x8180 | rcode;                                // QR | RD | RA | rcode
    p.extend_from_slice(&flags.to_be_bytes());
    p.extend_from_slice(&1u16.to_be_bytes());                       // QDCOUNT
    p.extend_from_slice(&ancount.to_be_bytes());                    // ANCOUNT
    p.extend_from_slice(&0u16.to_be_bytes());                       // NSCOUNT
    p.extend_from_slice(&0u16.to_be_bytes());                       // ARCOUNT
    // Question (echo)
    p.extend_from_slice(&q.name_bytes);
    p.extend_from_slice(&q.qtype.to_be_bytes());
    p.extend_from_slice(&q.qclass.to_be_bytes());
    // Answer (only for an A hit)
    if let DnsOutcome::Answer(ip) = out {
        // NAME — compression pointer back to question (offset 12).
        p.push(0xC0); p.push(0x0C);
        p.extend_from_slice(&1u16.to_be_bytes());   // TYPE = A
        p.extend_from_slice(&1u16.to_be_bytes());   // CLASS = IN
        p.extend_from_slice(&60u32.to_be_bytes());  // TTL = 60s
        p.extend_from_slice(&4u16.to_be_bytes());   // RDLENGTH
        p.extend_from_slice(ip);
    }
    p
}

/// Build a full virtio-net frame (12-byte virtio hdr + eth + IPv4 + UDP
/// + payload) addressed `GATEWAY → GUEST` for source/dst ports given.
fn build_ipv4_udp_reply(
    dst_ip: [u8; 4],
    dst_port: u16,
    src_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let total_ip_len = IPV4_HDR_LEN + UDP_HDR_LEN + payload.len();
    let total = VNET_HDR_LEN + ETH_HDR_LEN + total_ip_len;
    let mut buf = alloc::vec![0u8; total];

    write_eth(&mut buf, &GUEST_MAC, &GATEWAY_MAC, ETHERTYPE_IPV4);

    let ip_off = VNET_HDR_LEN + ETH_HDR_LEN;
    buf[ip_off + 0]  = 0x45;                                    // version + IHL
    buf[ip_off + 1]  = 0;                                        // TOS
    buf[ip_off + 2..ip_off + 4].copy_from_slice(&(total_ip_len as u16).to_be_bytes());
    buf[ip_off + 4..ip_off + 6].copy_from_slice(&0u16.to_be_bytes()); // id
    buf[ip_off + 6..ip_off + 8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF
    buf[ip_off + 8]  = 64;                                       // TTL
    buf[ip_off + 9]  = PROTO_UDP;
    // checksum = 0 placeholder
    buf[ip_off + 12..ip_off + 16].copy_from_slice(&GATEWAY_IP);
    buf[ip_off + 16..ip_off + 20].copy_from_slice(&dst_ip);
    let ip_csum = ipv4_checksum(&buf[ip_off..ip_off + IPV4_HDR_LEN]);
    buf[ip_off + 10..ip_off + 12].copy_from_slice(&ip_csum.to_be_bytes());

    let udp_off = ip_off + IPV4_HDR_LEN;
    let udp_len = (UDP_HDR_LEN + payload.len()) as u16;
    buf[udp_off + 0..udp_off + 2].copy_from_slice(&src_port.to_be_bytes());
    buf[udp_off + 2..udp_off + 4].copy_from_slice(&dst_port.to_be_bytes());
    buf[udp_off + 4..udp_off + 6].copy_from_slice(&udp_len.to_be_bytes());
    // checksum zero (allowed for UDP-over-IPv4)
    buf[udp_off + UDP_HDR_LEN..].copy_from_slice(payload);

    buf
}

fn write_eth(buf: &mut [u8], dst: &[u8; 6], src: &[u8; 6], ethertype: u16) {
    // virtio_net_hdr at offset 0..12. Per virtio 1.2 §5.1.6.4.1, with
    // VIRTIO_F_VERSION_1 negotiated and VIRTIO_NET_F_MRG_RXBUF NOT
    // negotiated, num_buffers (bytes 10..12, LE) MUST be 1 or Linux's
    // virtio_net driver drops the packet silently in receive_buf().
    // Everything else stays zero (no offloads, no GSO).
    buf[10] = 1;
    buf[11] = 0;
    let off = VNET_HDR_LEN;
    buf[off..off + 6].copy_from_slice(dst);
    buf[off + 6..off + 12].copy_from_slice(src);
    buf[off + 12..off + 14].copy_from_slice(&ethertype.to_be_bytes());
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < header.len() {
        sum += u16::from_be_bytes([header[i], header[i + 1]]) as u32;
        i += 2;
    }
    if i < header.len() {
        sum += (header[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn cap_reject(proto: &str, ip: [u8; 4], port: u16) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= 8 { return; }   // limit log spam
    kprintln!(
        "[nat] {} {}.{}.{}.{}:{} dropped (cap-reject)",
        proto, ip[0], ip[1], ip[2], ip[3], port,
    );
}

/// TCP checksum: pseudo-header (src_ip, dst_ip, zero, proto, tcp_len) +
/// the segment itself. Caller passes the same src/dst IPs that go in
/// the IPv4 header.
fn tcp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], tcp_segment: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    // pseudo-header
    sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
    sum += PROTO_TCP as u32;
    sum += tcp_segment.len() as u32;
    let mut i = 0;
    while i + 1 < tcp_segment.len() {
        sum += u16::from_be_bytes([tcp_segment[i], tcp_segment[i + 1]]) as u32;
        i += 2;
    }
    if i < tcp_segment.len() {
        sum += (tcp_segment[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Guest timer-IRQ injections this window (PIT IRQ0 + LAPIC LVTT). Confirms the
/// CONFIG_HZ=1000 fix: should read ~1000/s (the guest's programmed rate), not
/// the old ~100/s (our wall-clock pacing). Incremented from the SVM inject path.
static NS_GTIMER: AtomicU64 = AtomicU64::new(0);
pub fn note_guest_timer() { NS_GTIMER.fetch_add(1, AtOrd::Relaxed); }
/// Cumulative guest timer-IRQ injections (LVTT+PIT). Surfaced in `cores` to
/// MEASURE the effective guest HZ: ~1000/s = the guest's programmed 1 kHz tick
/// is delivered; <1000/s = the BSP's 2ms parks are freezing the guest timer
/// (floor b) → the guest's delayed-ACK/RTO/pacing slow → the slow download
/// regime. Tests the "1000 vs 100, mal gut mal schlecht" hypothesis directly.
pub fn guest_timer_count() -> u64 { NS_GTIMER.load(AtOrd::Relaxed) }
/// Cumulative outbound TX (segments, bytes) — surfaced in `cores` as segs/s +
/// avg segment size during an upload. The b1-vs-b2 discriminator: a high segs/s
/// with the worker core pegged = the SW-TSO emit pipeline is the cap (b1, the
/// lock-split + host-TX batching lift it); the same segs/s with an idle worker =
/// the cap is cwnd × inflated bridge RTT (b2, an ACK-clock the emit path can't
/// raise). Monotonic in full mode (pump's swap never runs); diff two snapshots.
pub fn tx_stats() -> (u64, u64) {
    (NS_TX_PKTS.load(AtOrd::Relaxed), NS_TX_BYTES.load(AtOrd::Relaxed))
}
/// Count of net-RX IRQ10 actually raised to the guest (after ITR moderation).
/// vs the per-packet rate it would be without — the io-EOI-storm signal.
static NS_NET_IRQ: AtomicU64 = AtomicU64::new(0);
pub fn note_net_irq() { NS_NET_IRQ.fetch_add(1, AtOrd::Relaxed); }
/// Bridge RX health for `cores`: (tap ring-full drops, guest-ring-full stalls).
/// A ring-full drop is BACKPRESSURE — the producer outran the guest and the far
/// end slows down. `inject_false` is the guest being the limiter: it had no RX
/// buffer posted, so the frame stayed in the tap and nothing was lost.
pub fn rx_health_snapshot() -> (u64, u64) {
    (NS_TAP_FULL.load(AtOrd::Relaxed), NS_INJECT_FALSE.load(AtOrd::Relaxed))
}

/// The guest RX ring was full — the frame went back to the head of the tap.
pub fn note_inject_false() { NS_INJECT_FALSE.fetch_add(1, AtOrd::Relaxed); }

/// virtio-gpu TRANSFER_TO_HOST pixel bytes copied on the vCPU core (the browser
/// rendering). If high during a download, the framebuffer copy is stealing vCPU
/// cycles from the net pump (the framebuffer↔pump contention) — Florian's
/// "graphics?" hypothesis, measured.
static NS_GPU_BYTES: AtomicU64 = AtomicU64::new(0);
static NS_GPU_XFERS: AtomicU64 = AtomicU64::new(0);
static NS_GPU_CYC: AtomicU64 = AtomicU64::new(0);
pub fn note_gpu_transfer(bytes: u64, cycles: u64) {
    NS_GPU_CYC.fetch_add(cycles, AtOrd::Relaxed);
    NS_GPU_BYTES.fetch_add(bytes, AtOrd::Relaxed);
    NS_GPU_XFERS.fetch_add(1, AtOrd::Relaxed);
}

/// Everything the bridge knows about itself, for `netstat`.
///
/// The counters were always there; they lived behind a debug const that had
/// been `false` for months, so the one path nobody could see was the one
/// between the guest and the wire. This is the `wlan` treatment: no console
/// traffic, one screen on demand, and the numbers arranged so the reader can
/// tell the failures APART rather than watching a single "throughput sagged".
pub struct BridgeStats {
    pub active: bool,
    pub up_s: u64,
    /// Identity of the data path, so the three machines can be COMPARED rather
    /// than each believed on its own. Same path id on QEMU, NUC and notebook is
    /// the acceptance test of the whole rebuild.
    pub path: &'static str,
    pub version: &'static str,
    pub vendor: &'static str,
    pub nic: &'static str,
    pub worker_core: Option<usize>,
    /// Tap: current depth, capacity, frames delivered to the guest, and the
    /// ring-full drops. Progress (`tap_delivered`) is the number that matters —
    /// a fill level says nothing (feedback_watchdog_that_only_fires_at_full).
    pub tap: u64, pub tap_cap: usize,
    pub tap_delivered: u64, pub tap_delivered_ps: u64, pub tap_full: u64,
    pub rehomed: u64,
    pub kicks: u64, pub frames_in: u64, pub arp_in: u64, pub other_in: u64,
    pub rx_pkts: u64, pub rx_bytes: u64, pub rx_pps: u64,
    pub tx_pkts: u64, pub tx_bytes: u64, pub tx_pps: u64,
    pub window_ms: u64,
    pub flows_tcp: u64, pub flows_udp: u64,
    pub live: usize, pub cap: usize,
    pub drop_table: u64, pub drop_egress: u64,
    pub inject_false: u64,
    pub net_irq: u64,
    pub gpu_kb: u64, pub gpu_xfers: u64, pub gpu_kbps: u64,
    /// Percent of the last window the vCPU spent inside the framebuffer copy.
    pub gpu_pct: u64, pub gpu_us_each: u64,
    pub gtimer_ps: u64,
    pub ip_malformed: u64, pub ip_to_gw: u64, pub ip_dns: u64, pub ip_proto: u64,
    pub tx_runt: u64, pub tx_badtcp: u64, pub tx_arpmiss: u64,
    pub tx_ringbad: u64, pub tx_trunc: u64, pub rx_unmatched: u64,
}

// Previous snapshot, so a second `netstat` a few seconds later reads as a RATE.
// Cumulative counters answer "did this ever work"; only the rate answers "is it
// working right now", which is the whole question when a link dies after five
// seconds.
static RPT_TSC: AtomicU64 = AtomicU64::new(0);
static RPT_RX: AtomicU64 = AtomicU64::new(0);
static RPT_TX: AtomicU64 = AtomicU64::new(0);
static RPT_GPU: AtomicU64 = AtomicU64::new(0);
static RPT_GT: AtomicU64 = AtomicU64::new(0);
static RPT_GPUCYC: AtomicU64 = AtomicU64::new(0);
static RPT_GPUXF: AtomicU64 = AtomicU64::new(0);
static RPT_TAPDEL: AtomicU64 = AtomicU64::new(0);

pub fn bridge_stats() -> BridgeStats {
    let now = crate::interrupts::rdtsc();
    let khz = (crate::interrupts::tsc_freq() / 1000).max(1);
    let mhz = (crate::interrupts::tsc_freq() / 1_000_000).max(1);
    let rx_pkts = NS_RX_PKTS.load(AtOrd::Relaxed);
    let tx_pkts = NS_TX_PKTS.load(AtOrd::Relaxed);
    let prev_tsc = RPT_TSC.swap(now, AtOrd::Relaxed);
    let prev_rx = RPT_RX.swap(rx_pkts, AtOrd::Relaxed);
    let prev_tx = RPT_TX.swap(tx_pkts, AtOrd::Relaxed);
    let gpu_bytes = NS_GPU_BYTES.load(AtOrd::Relaxed);
    let prev_gpu = RPT_GPU.swap(gpu_bytes, AtOrd::Relaxed);
    let gcyc = NS_GPU_CYC.load(AtOrd::Relaxed);
    let prev_gcyc = RPT_GPUCYC.swap(gcyc, AtOrd::Relaxed);
    let gxf = NS_GPU_XFERS.load(AtOrd::Relaxed);
    let prev_gxf = RPT_GPUXF.swap(gxf, AtOrd::Relaxed);
    let d_gcyc = gcyc.saturating_sub(prev_gcyc);
    let d_gxf = gxf.saturating_sub(prev_gxf);
    let gt = NS_GTIMER.load(AtOrd::Relaxed);
    let prev_gt = RPT_GT.swap(gt, AtOrd::Relaxed);
    let window_ms = if prev_tsc == 0 { 0 } else { now.wrapping_sub(prev_tsc) / khz };
    let win_cyc = window_ms.saturating_mul(khz);
    let per_s = |d: u64| if window_ms > 0 { d * 1000 / window_ms } else { 0 };
    let start = NS_START_TICK.load(AtOrd::Relaxed);
    let delivered = NS_TAP_DELIVERED.load(AtOrd::Relaxed);
    let prev_deliv = RPT_TAPDEL.swap(delivered, AtOrd::Relaxed);
    BridgeStats {
        active: L3_ACTIVE.load(AtOrd::Acquire),
        up_s: if start == 0 { 0 } else { crate::interrupts::ticks().wrapping_sub(start) / 100 },
        path: path_id(),
        version: env!("CARGO_PKG_VERSION"),
        vendor: match crate::microvm::cpu::current_vendor() {
            crate::microvm::cpu::Vendor::Amd => "AMD/SVM",
            crate::microvm::cpu::Vendor::Intel => "Intel/VMX",
            _ => "none",
        },
        nic: crate::netdev::active_name(),
        worker_core: super::net_backend::worker_core(),
        tap: tap_len(), tap_cap: TAP_RING,
        tap_delivered: delivered,
        tap_delivered_ps: per_s(delivered.saturating_sub(prev_deliv)),
        tap_full: NS_TAP_FULL.load(AtOrd::Relaxed),
        rehomed: NS_MAP_REHOMED.load(AtOrd::Relaxed),
        kicks: NS_GUEST_KICKS.load(AtOrd::Relaxed),
        frames_in: NS_GUEST_FRAMES.load(AtOrd::Relaxed),
        arp_in: NS_GUEST_ARP.load(AtOrd::Relaxed),
        other_in: NS_GUEST_OTHER.load(AtOrd::Relaxed),
        rx_pkts, rx_bytes: NS_RX_BYTES.load(AtOrd::Relaxed),
        rx_pps: per_s(rx_pkts.saturating_sub(prev_rx)),
        tx_pkts, tx_bytes: NS_TX_BYTES.load(AtOrd::Relaxed),
        tx_pps: per_s(tx_pkts.saturating_sub(prev_tx)),
        window_ms,
        flows_tcp: NS_TCP_FLOWS.load(AtOrd::Relaxed),
        flows_udp: NS_UDP_FLOWS.load(AtOrd::Relaxed),
        live: L3.lock().maps.iter().flatten().count(), cap: L3_MAX,
        drop_table: NS_DROP_TABLE.load(AtOrd::Relaxed),
        drop_egress: NS_DROP_EGRESS.load(AtOrd::Relaxed),
        inject_false: NS_INJECT_FALSE.load(AtOrd::Relaxed),
        net_irq: NS_NET_IRQ.load(AtOrd::Relaxed),
        gpu_kb: gpu_bytes / 1024,
        gpu_xfers: NS_GPU_XFERS.load(AtOrd::Relaxed),
        gpu_kbps: per_s(gpu_bytes.saturating_sub(prev_gpu)) / 1024,
        gpu_pct: if win_cyc > 0 { (d_gcyc.saturating_mul(100)) / win_cyc } else { 0 },
        gpu_us_each: if d_gxf > 0 { d_gcyc / d_gxf / mhz } else { 0 },
        gtimer_ps: per_s(gt.saturating_sub(prev_gt)),
        ip_malformed: NS_IP_MALFORMED.load(AtOrd::Relaxed),
        ip_to_gw: NS_IP_TO_GW.load(AtOrd::Relaxed),
        ip_dns: NS_IP_DNS.load(AtOrd::Relaxed),
        ip_proto: NS_IP_PROTO.load(AtOrd::Relaxed),
        tx_runt: NS_TX_RUNT.load(AtOrd::Relaxed),
        tx_badtcp: NS_TX_BADTCP.load(AtOrd::Relaxed),
        tx_arpmiss: NS_TX_ARPMISS.load(AtOrd::Relaxed),
        tx_ringbad: NS_TX_RINGBAD.load(AtOrd::Relaxed),
        tx_trunc: NS_TX_TRUNC.load(AtOrd::Relaxed),
        rx_unmatched: NS_RX_UNMATCHED.load(AtOrd::Relaxed),
    }
}

/// Number of currently-active (non-closed) TCP sessions. The run_linux
/// idle-detection uses this to extend the timeout when traffic is in
/// flight.
pub fn active_session_count() -> usize {
    // Drives VM idle-detection: keep the guest scheduled while any
    // masquerade flow is live or a reply is still queued.
    L3.lock().maps.iter().flatten().count() + tap_len() as usize
}

/// Tear down NAT state when a microvm run ends so the next launch —
/// and the host's own networking — start clean.
pub fn reset_sessions() {
    l3_reset();
    // Belt-and-suspenders: clear the shared NIC-drain guard so a microvm run
    // can never leave the HOST's own networking (DNS / OTA) bricked.
    crate::net::reset_poll_guard();
    // The COUNTERS deliberately survive teardown — see `reset_counters`.
}

/// Zero the bridge counters. At VM **start**, not at teardown.
///
/// They used to be zeroed here on the way out, which meant the numbers existed
/// only while the guest was alive: close the browser, ask `netstat` what
/// happened, get nothing. The one moment anyone wants a post-mortem is right
/// after the thing died, and that was exactly the moment the evidence was
/// erased. Zeroing on the way IN gives every run a clean window and leaves the
/// last run readable until the next launch.
pub fn reset_counters() {
    for c in [&NS_RX_BYTES, &NS_RX_PKTS, &NS_TX_BYTES, &NS_TX_PKTS,
              &NS_DROP_TABLE, &NS_DROP_EGRESS,
              &NS_HIGHWATER, &NS_LAST_TICK, &NS_INJECT_FALSE,
              &NS_TCP_FLOWS, &NS_UDP_FLOWS,
              &NS_NET_IRQ, &RPT_TSC, &RPT_RX, &RPT_TX,
              &NS_GUEST_KICKS, &NS_GUEST_FRAMES, &NS_GUEST_ARP, &NS_GUEST_OTHER,
              &NS_GPU_BYTES, &NS_GPU_XFERS, &RPT_GPU, &NS_GPU_CYC,
              &RPT_GPUCYC, &RPT_GPUXF,
              &NS_GTIMER, &RPT_GT, &RPT_TAPDEL,
              &NS_IP_MALFORMED, &NS_IP_TO_GW, &NS_IP_DNS, &NS_IP_PROTO,
              &NS_TX_RUNT, &NS_TX_BADTCP, &NS_TX_ARPMISS, &NS_TX_RINGBAD,
              &NS_TX_TRUNC, &NS_RX_UNMATCHED,
              &NS_TAP_FULL, &NS_TAP_DELIVERED, &NS_MAP_REHOMED] {
        c.store(0, AtOrd::Relaxed);
    }
    NS_START_TICK.store(crate::interrupts::ticks().max(1), AtOrd::Relaxed);
    NS_LAST_ACTIVITY.store(0, AtOrd::Relaxed);
}
