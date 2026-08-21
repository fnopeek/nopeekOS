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
//!     layer; replies are intercepted in `net::ipv4` (`l3_inbound`),
//!     rewritten back to the guest, and injected by `pump`. The
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
use super::guest_mem::GuestMem;
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
use alloc::collections::VecDeque;

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

static NS_DROP_QUEUE: AtomicU64 = AtomicU64::new(0);  // INBOUND_Q overflow
static NS_DROP_TABLE: AtomicU64 = AtomicU64::new(0);  // L3 masquerade table full
static NS_DROP_EGRESS: AtomicU64 = AtomicU64::new(0); // host NIC refused the frame
static NS_HIGHWATER: AtomicU64 = AtomicU64::new(0);
static NS_LAST_TICK: AtomicU64 = AtomicU64::new(0);
/// High-water mark of the INBOUND_Q staging depth. If this rides near
/// INBOUND_MAX the download is backpressure-capped; if shallow, the cap is
/// upstream (slirp/NIC).
static NS_IQ_HI: AtomicU64 = AtomicU64::new(0);
/// RX delivery latency = how long a guest-bound packet sat in INBOUND_Q before
/// the BSP pump injected it (TSC ticks, summed + counted for an avg, plus a
/// max). THE decisive page-load metric: high (~ms) ⇒ our RX delivery is the
/// bottleneck (the BSP parks between bursts → event-driven wake is the fix);
/// low (~µs) ⇒ the cap is upstream (RTT / DNS / QUIC / server), not our code.
static NS_RXLAT_SUM: AtomicU64 = AtomicU64::new(0);
static NS_RXLAT_N: AtomicU64 = AtomicU64::new(0);
static NS_RXLAT_MAX: AtomicU64 = AtomicU64::new(0);
/// Consumer-bottleneck diagnostics (which serial limit caps RX): how often the
/// pump runs, how often inject_rx bailed because the guest RX ring had no buffer
/// (= guest-NAPI-bound), and total injected (→ avg batch per pump call).
static NS_PUMP_CALLS: AtomicU64 = AtomicU64::new(0);
static NS_INJECT_FALSE: AtomicU64 = AtomicU64::new(0);
/// New-flow counts by transport, to spot QUIC: a cold page that opens lots of
/// UDP flows is using HTTP/3 — if those churn/stall it points at QUIC-over-NAT.
static NS_TCP_FLOWS: AtomicU64 = AtomicU64::new(0);
static NS_UDP_FLOWS: AtomicU64 = AtomicU64::new(0);
/// TEMP profiler: total TSC cycles spent inside net.inject_rx (the consumer /
/// guest-side cost — guest-mem walk + scatter write + used-ring). Divided by
/// NS_RXLAT_N (successful injects) → cycles per delivered packet. Tells us
/// whether the bridge cap is per-packet inject cost vs pump cadence. Strip when
/// done (same dev-tool class as the http -d poll-split profiler).
static NS_INJECT_CYC: AtomicU64 = AtomicU64::new(0);
/// Pump-starvation probe: longest gap (TSC) between two `drain_inbound` calls
/// this window. If `drainmax` (µs) tracks `rxlat max`, the latency spikes are
/// the BSP pump not running (vCPU busy / descheduled), NOT guest ring
/// exhaustion (injfalse) or INBOUND_Q backlog (iq). Plus the min count of RX
/// buffers the guest had posted (`rxring min`) — near 0 ⇒ shallow big-packets
/// ring is the limiter. Both reset per window.
static NS_DRAIN_GAP_MAX: AtomicU64 = AtomicU64::new(0);
static NS_LAST_DRAIN: AtomicU64 = AtomicU64::new(0);
static NS_RXRING_MIN: AtomicU64 = AtomicU64::new(u64::MAX);

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
/// `recently_active()` gates its halt-poll (the legacy `drain_inbound` that used
/// to set this isn't on the full-mode path).
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
    host_port: u16,
    last_tick: u64,
}

static L3: Mutex<[Option<L3Map>; L3_MAX]> = Mutex::new([const { None }; L3_MAX]);
/// Gates the host-RX inbound intercept. Off ⇒ `l3_inbound` is a cheap
/// `false` so a guest-less host (plain OTA/https) is never touched.
static L3_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Rewritten guest-bound frames produced from the host-RX context,
/// drained + injected by `pump` on the VM thread (same split the old
/// termination pump used, to keep virtio access on one thread).
/// (push TSC, frame) — the TSC lets the pump measure RX delivery latency.
static INBOUND_Q: Mutex<VecDeque<(u64, Vec<u8>)>> = Mutex::new(VecDeque::new());

/// Recycled frame buffers for the inbound staging path. Each download packet
/// used to `vec![0u8; ~1514]` (allocator free-list walk + memset) and free it
/// after injection — the dominant inbound per-packet cost (~2.6µs/pkt: the
/// first-fit allocator walks an O(n) free list under churn, plus the memset).
/// Linux solves this with skb pools; we keep a small ring of buffers that the
/// producer (l3_inbound) borrows and the consumer (drain_inbound) returns, so
/// after warmup the datapath does ZERO heap alloc/free — only the unavoidable
/// payload memcpy. Bounded so it can't grow without limit; only used while a VM
/// is active (the BSP is the sole accessor, so the lock is uncontended).
static FRAME_POOL: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
const FRAME_POOL_MAX: usize = INBOUND_MAX + 16;
const FRAME_BUF_CAP: usize = 2048; // ≥ vnet+eth+MTU, so resize never reallocs

/// Borrow a frame buffer sized to `len` from the pool (or allocate once if the
/// pool is cold). The contents are uninitialised beyond what the caller writes —
/// l3_inbound overwrites every byte (vnet hdr + eth hdr + full IP copy).
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
/// Backpressure cap on the staging queue between the NIC drain and the guest
/// inject. ANTI-BUFFERBLOAT: kept SMALL (≈2× the guest's 256-entry RX queue) on
/// purpose — a big queue lets TCP fill it, ballooning the latency under load
/// (measured: 600 ms loaded latency vs ~20 ms native), which is what makes page
/// loads crawl (every round-trip waits behind a fat download queue). Small ⇒ it
/// fills fast ⇒ we drop ⇒ TCP backs off ⇒ the queue (and latency) stays short.
/// For a browser low latency beats peak throughput. Also bounds staging memory.
///
/// 2026-06-09: the drops were NOT really a consume-rate problem. Diagnostics
/// showed injfalse=0 (the guest ALWAYS had RX buffers) yet tens of thousands of
/// drops — because Core 0 was racing the BSP pump to fill this queue and winning
/// (it can't drain — no VmContext). Fixed by making the BSP pump the sole NIC
/// drainer while a VM is active (net::poll skip on Core 0).
///
/// 2026-06-19: that old 256 ("one fat burst") rested on a regime where the pump
/// drained every call. Today's bottleneck is different: during a download-to-disk
/// the inline 9p→npkFS write stalls the pump for whole `rxlat max` windows (8–16
/// ms), and at ~1 Gbit the host NIC fills the queue in those windows → overflow
/// drops → TCP sawtooth (~10 MB/s where QEMU should do 80–90). 256 is now the
/// binding cap, not bufferbloat protection. Bumped to 1024 ≈ one 12 ms stall
/// burst at line rate, so a transient stall is absorbed instead of dropped. This
/// is NOT bufferbloat as long as the queue still drains between stalls (the pump
/// now also runs on 9p exits) — and `rxlat avg` in [netstat] MEASURES that: if it
/// stays low the deep buffer only absorbs transients; if it balloons, switch to
/// time-based AQM (drop by head-of-queue standing age via the per-packet
/// `push_tsc` we already record), which gives low latency AND throughput.
const INBOUND_MAX: usize = 1024;

// ─── RX GRO (generic receive offload) ──────────────────────────────────────
// At ~1 Gbit the guest sees ~30–44k packets/s of 1.5 KB TCP segments. Without
// offload the guest vCPU is CPU-bound just running its per-packet RX path
// (NAPI/softirq/TCP) → throughput caps at ~½ the host-direct rate AND latency-
// sensitive traffic (a speedtest ping) starves behind the bulk backlog INSIDE
// the guest (measured: ~500 ms loaded latency vs ~21 ms native; our own rxlat
// is only ~2.5 ms → the delay is in the guest, not our delivery).
//
// Fix: coalesce contiguous, same-flow bulk TCP segments into one large GSO
// frame before injecting — host-side GRO, exactly what a NIC's LRO does. The
// guest negotiates VIRTIO_NET_F_GUEST_TSO4 → "big packets" mode (page-chain RX
// buffers, which inject_rx already walks). One ~60 KB super-frame replaces ~40
// segments → ~40× fewer packets up the guest stack.
//
// Only FULL-size bulk segments are ever held; ACKs, SYN/FIN/RST, small packets
// and the latency probe pass through instantly, and GRO_TIMEOUT_US bounds any
// hold. Gated on the guest actually negotiating the feature → inert otherwise.

/// Set by the emulated virtio-net device once the driver negotiates
/// VIRTIO_NET_F_GUEST_TSO4 (DRIVER_OK); cleared on reset / VM teardown.
static GRO_ENABLED: AtomicBool = AtomicBool::new(false);
pub fn set_gro_enabled(on: bool) { GRO_ENABLED.store(on, AtOrd::Relaxed); }

// GRO effectiveness counters (printed in [netstat]): coalesced GSO frames
// emitted + total segments merged into them → avg segs/frame is the win factor.
static NS_GRO_FRAMES: AtomicU64 = AtomicU64::new(0);
static NS_GRO_SEGS:   AtomicU64 = AtomicU64::new(0);

/// Isolation switch: when false, the device still advertises the offloads
/// (guest in big-packets mode) but `gro_offer` passes every frame through
/// unchanged — no coalescing, no GSO frame.
///
/// v0.225.9 fixed the real root cause (feature-word read bug) → big-packets RX
/// confirmed working (gso_ok=true, injects deliver). Now TRUE for the actual
/// GRO test: coalesce bulk TCP into NEEDS_CSUM/CHECKSUM_PARTIAL GSO superframes.
/// The earlier "GSO frame broke the internet" results were the negotiation bug,
/// not the frame — this is the first clean test of the GSO frame itself.
// TEST (2026-06-25): single-connection download caps at ~2.2 MB/s = a ~33 KB
// guest receive window at ~15 ms RTT (host-direct uses ~1.2 MB). Hypothesis:
// GRO delivers one ~35 KB superframe per RTT (avg 24 segs/frame) instead of a
// smooth segment stream, so the guest's rcv_rtt_est / receive-buffer
// auto-tuning underestimates the BDP and never scales the window past ~33 KB.
// Set false to deliver frames individually (smooth) and see if the single-conn
// window ramps. If it does, GRO's burstiness is the cap → redesign GRO to pace
// delivery instead of one-burst-per-RTT.
const GRO_COALESCE: bool = true;

const GRO_SLOTS:       usize = 16;      // concurrent flows we coalesce
const GRO_MAX_PAYLOAD: usize = 60_000;  // < 64 KiB IP total-length limit
const GRO_MAX_SEGS:    usize = 44;      // ~ one 64 KB superframe of MSS≈1448
const GRO_START_MIN:   usize = 1000;    // only hold near-full (mid-burst) segs
const GRO_TIMEOUT_US:  u64   = 200;     // max hold before a forced flush

const VNET_HDR_F_NEEDS_CSUM: u8 = 1;
const VNET_HDR_GSO_TCPV4:    u8 = 1;
const TCP_CSUM_OFF:         u16 = 16; // checksum field offset within the TCP header
const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PSH: u8 = 0x08;
const TCP_URG: u8 = 0x20;
const TCP_CWR: u8 = 0x80;
/// virtio-net header gso_type for a TCPv4 GSO/TSO super-frame on TX (the ECN
/// flag 0x80 is OR'd on top and must be masked off before comparing).
const VNET_HDR_GSO_ECN: u8 = 0x80;

struct GroCtx {
    gport: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    next_seq: u32,        // sequence number the next segment must carry
    mss: u16,             // payload size of the merged segments (= gso_size)
    seg_count: usize,
    payload_bytes: usize,
    ip_off: usize,
    l4_off: usize,        // TCP header offset in the base frame
    tcp_hl: usize,
    last_tsc: u64,
    frame: Vec<u8>,       // accumulating vnet+eth+ip+tcp+payload frame
}

static GRO: Mutex<[Option<GroCtx>; GRO_SLOTS]> = Mutex::new([const { None }; GRO_SLOTS]);

/// IP total-length + checksum, then the virtio-net GSO header (only when >1
/// segment was actually merged; a lone segment keeps its original checksum).
fn gro_finalize(c: &mut GroCtx) {
    let ip_off = c.ip_off;
    let ihl = (c.frame[ip_off] & 0x0F) as usize * 4;
    let total = ihl + c.tcp_hl + c.payload_bytes;
    c.frame[ip_off + 2..ip_off + 4].copy_from_slice(&(total as u16).to_be_bytes());
    c.frame[ip_off + 10] = 0; c.frame[ip_off + 11] = 0;
    let ipc = ipv4_checksum(&c.frame[ip_off..ip_off + ihl]);
    c.frame[ip_off + 10..ip_off + 12].copy_from_slice(&ipc.to_be_bytes());
    if c.seg_count > 1 {
        // An LRO'd TCPv4 frame of `mss`-sized segments. Use NEEDS_CSUM +
        // CHECKSUM_PARTIAL semantics — exactly what Linux's own
        // virtio_net_hdr_from_skb emits for a GSO skb — so the guest takes the
        // well-trodden skb_partial_csum_set path (the DATA_VALID variant hit a
        // fragile flow-dissect path and the merged frames got dropped). The
        // TCP checksum field holds the pseudo-header partial (length 0); the
        // guest completes/validates it per segment.
        let l4 = c.l4_off;
        let src: [u8; 4] = c.frame[ip_off + 12..ip_off + 16].try_into().unwrap();
        let dst: [u8; 4] = c.frame[ip_off + 16..ip_off + 20].try_into().unwrap();
        let partial = tcp_pseudo_partial(src, dst);
        c.frame[l4 + 16..l4 + 18].copy_from_slice(&partial.to_be_bytes());

        c.frame[0] = VNET_HDR_F_NEEDS_CSUM;
        c.frame[1] = VNET_HDR_GSO_TCPV4;
        let hdr_len = (ETH_HDR_LEN + ihl + c.tcp_hl) as u16;
        let csum_start = (ETH_HDR_LEN + ihl) as u16; // TCP offset from eth start
        c.frame[2..4].copy_from_slice(&hdr_len.to_le_bytes());
        c.frame[4..6].copy_from_slice(&c.mss.to_le_bytes());
        c.frame[6..8].copy_from_slice(&csum_start.to_le_bytes());
        c.frame[8..10].copy_from_slice(&TCP_CSUM_OFF.to_le_bytes());
        c.frame[10..12].copy_from_slice(&1u16.to_le_bytes()); // num_buffers
        NS_GRO_FRAMES.fetch_add(1, AtOrd::Relaxed);
        NS_GRO_SEGS.fetch_add(c.seg_count as u64, AtOrd::Relaxed);
        // Active-download signal for the GPU-copy throttle. A GRO super-frame of
        // several segments only forms under sustained bulk RX (never idle or
        // browsing) — this is the POST-coalesce point, unlike the broken pre-GRO
        // per-packet check (always <1500 B → `dl N`, throttle never engaged).
        if c.seg_count >= 4 {
            DL_LAST_BULK_TSC.store(crate::interrupts::rdtsc(), AtOrd::Relaxed);
        }
    } else {
        c.frame[0..12].fill(0);
    }
}

/// One's-complement folded sum of the TCP pseudo-header with length 0 — the
/// CHECKSUM_PARTIAL seed for a GSO frame (the guest adds per-segment length +
/// data when completing each segment's checksum). Matches `~csum_tcpudp_magic
/// (src, dst, 0, IPPROTO_TCP, 0)`.
fn tcp_pseudo_partial(src: [u8; 4], dst: [u8; 4]) -> u16 {
    let mut sum: u32 = (u16::from_be_bytes([src[0], src[1]]) as u32)
        + (u16::from_be_bytes([src[2], src[3]]) as u32)
        + (u16::from_be_bytes([dst[0], dst[1]]) as u32)
        + (u16::from_be_bytes([dst[2], dst[3]]) as u32)
        + PROTO_TCP as u32; // + length 0
    while sum >> 16 != 0 { sum = (sum & 0xFFFF) + (sum >> 16); }
    sum as u16
}

fn gro_flush_slot(tbl: &mut [Option<GroCtx>; GRO_SLOTS], i: usize) {
    if let Some(mut c) = tbl[i].take() {
        gro_finalize(&mut c);
        INBOUND_Q.lock().push_back((crate::interrupts::rdtsc(), c.frame));
    }
}

/// Flush contexts idle longer than GRO_TIMEOUT_US so a stalled burst is never
/// held back. Called from the pump (frequent, cheap).
fn gro_flush_expired(now: u64) {
    let timeout = (crate::interrupts::tsc_freq() / 1_000_000).max(1) * GRO_TIMEOUT_US;
    let mut tbl = GRO.lock();
    for i in 0..GRO_SLOTS {
        let stale = matches!(tbl[i].as_ref(), Some(c) if now.wrapping_sub(c.last_tsc) > timeout);
        if stale { gro_flush_slot(&mut tbl, i); }
    }
}

/// A free slot, or the least-recently-appended flow evicted (flushed).
fn gro_alloc_slot(tbl: &mut [Option<GroCtx>; GRO_SLOTS], now: u64) -> usize {
    if let Some(i) = tbl.iter().position(|c| c.is_none()) { return i; }
    let (mut oldest, mut oldest_age) = (0usize, 0u64);
    for (i, c) in tbl.iter().enumerate() {
        if let Some(c) = c {
            let age = now.wrapping_sub(c.last_tsc);
            if age >= oldest_age { oldest_age = age; oldest = i; }
        }
    }
    gro_flush_slot(tbl, oldest);
    oldest
}

/// Offer a freshly-rewritten inbound TCP frame to the GRO engine. Returns
/// `None` if consumed (held or appended), or `Some(frame)` to inject as-is.
fn gro_offer(frame: Vec<u8>) -> Option<Vec<u8>> {
    if !GRO_COALESCE { return Some(frame); } // diagnostic: big-packets, no coalesce
    let ip_off = VNET_HDR_LEN + ETH_HDR_LEN;
    if frame.len() < ip_off + IPV4_HDR_LEN { return Some(frame); }
    let ihl = (frame[ip_off] & 0x0F) as usize * 4;
    let l4_off = ip_off + ihl;
    if ihl < IPV4_HDR_LEN || frame.len() < l4_off + TCP_HDR_LEN { return Some(frame); }
    let tcp_hl = ((frame[l4_off + 12] >> 4) & 0x0F) as usize * 4;
    let payload_off = l4_off + tcp_hl;
    if tcp_hl < TCP_HDR_LEN || frame.len() < payload_off { return Some(frame); }
    let payload_len = frame.len() - payload_off;
    let flags = frame[l4_off + 13];
    let seq = u32::from_be_bytes([frame[l4_off + 4], frame[l4_off + 5], frame[l4_off + 6], frame[l4_off + 7]]);
    let remote_ip = [frame[ip_off + 12], frame[ip_off + 13], frame[ip_off + 14], frame[ip_off + 15]];
    let remote_port = u16::from_be_bytes([frame[l4_off], frame[l4_off + 1]]);
    let gport = u16::from_be_bytes([frame[l4_off + 2], frame[l4_off + 3]]);
    let now = crate::interrupts::rdtsc();

    let mut tbl = GRO.lock();
    let slot = tbl.iter().position(|c| matches!(c, Some(c)
        if c.gport == gport && c.remote_port == remote_port && c.remote_ip == remote_ip));

    // Control / zero-payload segments are never coalesced: flush this flow's
    // pending data first (ordering), then inject this one directly.
    if payload_len == 0 || flags & (TCP_SYN | TCP_FIN | TCP_RST | TCP_URG) != 0 {
        if let Some(i) = slot { gro_flush_slot(&mut tbl, i); }
        return Some(frame);
    }

    if let Some(i) = slot {
        let c = tbl[i].as_mut().unwrap();
        let appendable = seq == c.next_seq
            && payload_len <= c.mss as usize
            && c.payload_bytes + payload_len <= GRO_MAX_PAYLOAD
            && c.seg_count < GRO_MAX_SEGS;
        if appendable {
            c.frame.extend_from_slice(&frame[payload_off..]);
            c.next_seq = c.next_seq.wrapping_add(payload_len as u32);
            c.payload_bytes += payload_len;
            c.seg_count += 1;
            c.last_tsc = now;
            // Carry the latest ACK + window into the merged TCP header.
            let bl4 = c.l4_off;
            c.frame[bl4 + 8..bl4 + 12].copy_from_slice(&frame[l4_off + 8..l4_off + 12]);
            c.frame[bl4 + 14..bl4 + 16].copy_from_slice(&frame[l4_off + 14..l4_off + 16]);
            let tail = payload_len < c.mss as usize;
            if tail || flags & TCP_PSH != 0
                || c.payload_bytes >= GRO_MAX_PAYLOAD || c.seg_count >= GRO_MAX_SEGS {
                gro_flush_slot(&mut tbl, i);
            }
            frame_pool_put(frame);
            return None;
        }
        // Not contiguous / size change: flush the open burst, then maybe start
        // a fresh one with this segment below.
        gro_flush_slot(&mut tbl, i);
    }

    // Hold only a near-full, non-PSH segment (likely mid-burst). PSH or small
    // ⇒ end-of-burst ⇒ inject directly (no latency hold).
    if payload_len >= GRO_START_MIN && flags & TCP_PSH == 0 {
        let i = gro_alloc_slot(&mut tbl, now);
        tbl[i] = Some(GroCtx {
            gport, remote_ip, remote_port,
            next_seq: seq.wrapping_add(payload_len as u32),
            mss: payload_len as u16,
            seg_count: 1,
            payload_bytes: payload_len,
            ip_off, l4_off, tcp_hl,
            last_tsc: now,
            frame,
        });
        return None;
    }
    Some(frame)
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
    let mut tbl = L3.lock();
    // Existing?
    for m in tbl.iter_mut().flatten() {
        if m.proto == proto && m.guest_port == gport
            && m.remote_ip == rip && m.remote_port == rport
        {
            m.last_tick = now;
            return Some(m.host_port);
        }
    }
    // Allocate a host port not currently in the table.
    let mut hp = L3_PORT_LO;
    'scan: while hp < L3_PORT_HI {
        if !tbl.iter().flatten().any(|m| m.host_port == hp) { break 'scan; }
        hp += 1;
    }
    if hp >= L3_PORT_HI { return None; }
    let slot = tbl.iter_mut().find(|s| s.is_none())?;
    *slot = Some(L3Map { proto, guest_port: gport, remote_ip: rip,
                          remote_port: rport, host_port: hp, last_tick: now });
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
fn l3_map_in(proto: u8, hport: u16, rip: [u8; 4], rport: u16, now: u64) -> Option<u16> {
    let mut tbl = L3.lock();
    for m in tbl.iter_mut().flatten() {
        if m.proto == proto && m.host_port == hport
            && m.remote_ip == rip && m.remote_port == rport
        {
            m.last_tick = now;
            return Some(m.guest_port);
        }
    }
    None
}

/// Recompute the TCP/UDP checksum after an address/port rewrite.
/// TCP checksum is mandatory; UDP-over-IPv4 may be zero, which we use
/// (cheaper, always valid) so QUIC payload size isn't a concern.
fn fix_l4_checksum(proto: u8, src_ip: [u8; 4], dst_ip: [u8; 4], l4: &mut [u8]) {
    if proto == PROTO_TCP {
        if l4.len() < TCP_HDR_LEN { return; }
        l4[16] = 0; l4[17] = 0;
        let c = tcp_checksum(src_ip, dst_ip, l4);
        l4[16..18].copy_from_slice(&c.to_be_bytes());
    } else if proto == PROTO_UDP {
        if l4.len() < UDP_HDR_LEN { return; }
        l4[6] = 0; l4[7] = 0; // 0 = checksum disabled (valid for IPv4)
    }
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
        crate::net::ipv4::send(dst_ip, proto, &seg);
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
    if l4.len() < TCP_HDR_LEN { return; }
    let thlen = ((l4[12] >> 4) & 0x0F) as usize * 4;
    if thlen < TCP_HDR_LEN || l4.len() < thlen { return; }
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
        crate::net::ipv4::send(dst_ip, PROTO_TCP, &seg);
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
        crate::net::ipv4::send(dst_ip, PROTO_TCP, &seg);
        off += this;
    }
}

/// vhost-style TX (symmetric with RX `l3_rewrite_inbound`): masquerade the
/// guest's outbound TCP segment IN PLACE — rewrite src port + adjust the
/// CHECKSUM_PARTIAL pseudo-header for the src-IP change (O(1), no full checksum,
/// no re-segmentation) — and hand the WHOLE frame (GSO super-frame and all) to
/// the host NIC, which segments + checksums it. The exact mirror of the inbound
/// rewrite — one data plane, same behaviour both directions. Returns None (the
/// frame went straight to the host NIC; TCP retransmits on any drop).
fn l3_outbound_offload(src_port: u16, dst_ip: [u8; 4], dst_port: u16,
                       l4: &[u8], gso_size: u16) -> Option<Vec<u8>> {
    if l4.len() < TCP_HDR_LEN { return None; }
    let now = crate::interrupts::ticks();
    let hp = match l3_map_out(PROTO_TCP, src_port, dst_ip, dst_port, now) {
        Some(p) => p,
        None => { note_table_full(); return None; }
    };
    let mut seg = l4.to_vec();
    seg[0..2].copy_from_slice(&hp.to_be_bytes()); // src port → masquerade port
    let our_ip = crate::net::arp::our_ip();
    if gso_size > 0 {
        // GSO super-frame: the device segments + checksums per segment, so we only
        // adjust the pseudo-header PARTIAL for the src-IP rewrite (GUEST_IP →
        // our_ip) — the incremental inbound csum mirror, minus the final
        // complement (CHECKSUM_PARTIAL stores the un-complemented sum).
        let old = u16::from_be_bytes([seg[16], seg[17]]);
        let new = adjust_partial(old, GUEST_IP, our_ip);
        seg[16..18].copy_from_slice(&new.to_be_bytes());
    } else {
        // Non-GSO frame (ACK / control / ≤ one MSS): compute the FULL TCP checksum
        // in SW (cheap) — this device's legacy F_GSO NEEDS_CSUM is unreliable for
        // small frames (HW test: download crawled to 9 Mbit). Still passthrough,
        // no re-segmentation.
        seg[16] = 0; seg[17] = 0;
        let c = tcp_checksum(our_ip, dst_ip, &seg);
        seg[16..18].copy_from_slice(&c.to_be_bytes());
    }

    NS_TX_PKTS.fetch_add(1, AtOrd::Relaxed);
    NS_TX_BYTES.fetch_add(seg.len() as u64, AtOrd::Relaxed);
    if !crate::net::ipv4::send_offload(dst_ip, PROTO_TCP, &seg, gso_size) {
        NS_DROP_EGRESS.fetch_add(1, AtOrd::Relaxed); // NIC refused it; TCP retransmits
    }
    L3_ACTIVE.store(true, AtOrd::Release);
    None
}

/// Incremental update of a CHECKSUM_PARTIAL (un-complemented pseudo-header sum)
/// for a 4-byte source-address change — like the inbound `csum_update` (RFC 1624)
/// but WITHOUT the final ones-complement (the device completes the un-complemented
/// partial).
fn adjust_partial(partial: u16, old_ip: [u8; 4], new_ip: [u8; 4]) -> u16 {
    let mut sum = partial as u32;
    sum += (!u16::from_be_bytes([old_ip[0], old_ip[1]])) as u32 & 0xFFFF;
    sum += (!u16::from_be_bytes([old_ip[2], old_ip[3]])) as u32 & 0xFFFF;
    sum += u16::from_be_bytes([new_ip[0], new_ip[1]]) as u32;
    sum += u16::from_be_bytes([new_ip[2], new_ip[3]]) as u32;
    while sum >> 16 != 0 { sum = (sum & 0xFFFF) + (sum >> 16); }
    sum as u16
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
    crate::net::ipv4::send(dst_ip, PROTO_ICMP, &seg);
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
pub fn l3_inbound(ip: &[u8]) -> bool {
    if !L3_ACTIVE.load(AtOrd::Acquire) { return false; }
    if ip.len() < IPV4_HDR_LEN { return false; }
    let ihl = (ip[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HDR_LEN || ip.len() < ihl { return false; }
    let proto = ip[9];
    if proto != PROTO_TCP && proto != PROTO_UDP && proto != PROTO_ICMP { return false; }
    let src_ip: [u8; 4] = ip[12..16].try_into().unwrap();
    let l4 = &ip[ihl..];
    // ICMP echo reply: the Identifier (l4[4..6]) is the masquerade key (no
    // ports). TCP/UDP: remote port + masquerade host port are l4[0..2]/[2..4].
    let (remote_port, host_port) = if proto == PROTO_ICMP {
        if l4.len() < 8 || l4[0] != ICMP_ECHO_REPLY { return false; }
        (0u16, u16::from_be_bytes([l4[4], l4[5]]))
    } else {
        if l4.len() < 4 { return false; }
        (u16::from_be_bytes([l4[0], l4[1]]), u16::from_be_bytes([l4[2], l4[3]]))
    };
    let now = crate::interrupts::ticks();
    let gport = match l3_map_in(proto, host_port, src_ip, remote_port, now) {
        Some(g) => g,
        None => return false,
    };
    // Backpressure: if the guest can't drain its RX queue fast enough, don't
    // grow the staging queue without bound (→ OOM). Drop → guest TCP
    // retransmits + slows. Still `true` (consumed — it IS guest traffic, the
    // host stack must not also process it).
    if INBOUND_Q.lock().len() >= INBOUND_MAX {
        NS_DROP_QUEUE.fetch_add(1, AtOrd::Relaxed);
        return true;
    }
    NS_RX_PKTS.fetch_add(1, AtOrd::Relaxed);
    NS_RX_BYTES.fetch_add(ip.len() as u64, AtOrd::Relaxed);

    // Rewrite: dst IP → guest, L4 dst port → guest port; recompute
    // both checksums. Wrap in vnet + eth (gateway → guest). Buffer is borrowed
    // from the recycle pool (no per-packet heap alloc/memset); every byte below
    // is written: vnet hdr zeroed, eth hdr, then the full IP copy.
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
        // Rewrite the echo-reply Identifier back to the guest's + recompute
        // the ICMP checksum (the IP dst rewrite above doesn't affect it).
        frame[l4_off + 4..l4_off + 6].copy_from_slice(&gport.to_be_bytes());
        fix_icmp_checksum(&mut frame[l4_off..]);
    } else if proto == PROTO_TCP && frame[l4_off..].len() >= TCP_HDR_LEN {
        // Incremental TCP checksum (RFC 1624): only the dst IP (host → guest, in
        // the pseudo-header) and the dst port changed, so adjust the server's
        // checksum in O(1) instead of recomputing over the whole ~1500 B segment
        // (that full recompute was the dominant inbound per-packet cost). Must
        // read the old values before overwriting the port bytes; `ip[16..20]` is
        // the original (host) dst IP, pre-rewrite.
        let old_check = u16::from_be_bytes([frame[l4_off + 16], frame[l4_off + 17]]);
        let new_check = csum_update(old_check, &[
            (u16::from_be_bytes([ip[16], ip[17]]), u16::from_be_bytes([GUEST_IP[0], GUEST_IP[1]])),
            (u16::from_be_bytes([ip[18], ip[19]]), u16::from_be_bytes([GUEST_IP[2], GUEST_IP[3]])),
            (host_port, gport),
        ]);
        frame[l4_off + 2..l4_off + 4].copy_from_slice(&gport.to_be_bytes());
        frame[l4_off + 16..l4_off + 18].copy_from_slice(&new_check.to_be_bytes());
    } else {
        // UDP: checksum set to 0 (disabled, valid for IPv4) — already cheap.
        frame[l4_off + 2..l4_off + 4].copy_from_slice(&gport.to_be_bytes());
        fix_l4_checksum(proto, src_ip, GUEST_IP, &mut frame[l4_off..]);
    }

    // TCP bulk → host-side GRO (coalesce into GSO superframes); everything
    // else (and GRO pass-throughs) goes straight to the staging queue.
    if proto == PROTO_TCP && GRO_ENABLED.load(AtOrd::Relaxed) {
        match gro_offer(frame) {
            None => return true,
            Some(f) => { INBOUND_Q.lock().push_back((crate::interrupts::rdtsc(), f)); }
        }
    } else {
        INBOUND_Q.lock().push_back((crate::interrupts::rdtsc(), frame));
    }
    true
}

/// vhost `handle_rx` rewrite (the new off-vCPU data plane, `net_dataplane`).
/// Translate ONE inbound host-network IPv4 packet back to the guest (dst IP/port
/// → guest, checksums fixed incrementally) and return the guest-bound ethernet
/// frame (vnet+eth+ip), or `None` if it isn't masqueraded guest traffic. PURE —
/// no staging queue, no GRO, no synthetic drop: the caller injects the returned
/// frame straight into the guest RX ring and applies real backpressure (stop
/// draining the NIC when the guest ring is full), exactly like vhost-net reading
/// a tap. `ip` is the IPv4 packet (no ethernet header). Recycle the returned
/// buffer via [`recycle_frame`] after injecting.
pub fn l3_rewrite_inbound(ip: &[u8]) -> Option<Vec<u8>> {
    if !L3_ACTIVE.load(AtOrd::Acquire) { return None; }
    if ip.len() < IPV4_HDR_LEN { return None; }
    let ihl = (ip[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HDR_LEN || ip.len() < ihl { return None; }
    let proto = ip[9];
    if proto != PROTO_TCP && proto != PROTO_UDP && proto != PROTO_ICMP { return None; }
    let src_ip: [u8; 4] = ip[12..16].try_into().unwrap();
    let l4 = &ip[ihl..];
    let (remote_port, host_port) = if proto == PROTO_ICMP {
        if l4.len() < 8 || l4[0] != ICMP_ECHO_REPLY { return None; }
        (0u16, u16::from_be_bytes([l4[4], l4[5]]))
    } else {
        if l4.len() < 4 { return None; }
        (u16::from_be_bytes([l4[0], l4[1]]), u16::from_be_bytes([l4[2], l4[3]]))
    };
    let now = crate::interrupts::ticks();
    let gport = l3_map_in(proto, host_port, src_ip, remote_port, now)?;
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
            (u16::from_be_bytes([ip[16], ip[17]]), u16::from_be_bytes([GUEST_IP[0], GUEST_IP[1]])),
            (u16::from_be_bytes([ip[18], ip[19]]), u16::from_be_bytes([GUEST_IP[2], GUEST_IP[3]])),
            (host_port, gport),
        ]);
        frame[l4_off + 2..l4_off + 4].copy_from_slice(&gport.to_be_bytes());
        frame[l4_off + 16..l4_off + 18].copy_from_slice(&new_check.to_be_bytes());
    } else {
        frame[l4_off + 2..l4_off + 4].copy_from_slice(&gport.to_be_bytes());
        fix_l4_checksum(proto, src_ip, GUEST_IP, &mut frame[l4_off..]);
    }
    Some(frame)
}

/// Return a frame buffer (from `l3_rewrite_inbound`) to the recycle pool after
/// it's been injected into the guest ring — no per-packet heap churn.
pub fn recycle_frame(buf: Vec<u8>) { frame_pool_put(buf); }

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
    for slot in tbl.iter_mut() {
        if let Some(m) = slot.as_ref() {
            let idle = now.wrapping_sub(m.last_tick);
            let max = if m.proto == PROTO_TCP { L3_TCP_IDLE_TICKS }
                      else { L3_UDP_IDLE_TICKS };
            if idle > max { *slot = None; }
        }
    }
}

/// Tear down all L3 state (VM stopped). Idempotent.
pub fn l3_reset() {
    L3_ACTIVE.store(false, AtOrd::Release);
    *L3.lock() = [const { None }; L3_MAX];
    INBOUND_Q.lock().clear();
    *GRO.lock() = [const { None }; GRO_SLOTS]; // drop held GRO bursts
    GRO_ENABLED.store(false, AtOrd::Relaxed);
    *FRAME_POOL.lock() = Vec::new(); // release recycled buffers
}

/// Classify a guest TX frame (virtio-net hdr + ethernet) and produce
/// zero or more RX frames to inject back. Side-effects: kprintln on
/// cap-rejects so the operator can see why a packet went nowhere.
pub fn process_tx(payload: &[u8], caps: &NetCaps) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if payload.len() < VNET_HDR_LEN + ETH_HDR_LEN { return out; }
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
    if frame.len() < ETH_HDR_LEN + IPV4_HDR_LEN { return None; }
    let ip = &frame[ETH_HDR_LEN..];
    let ihl = (ip[0] & 0x0F) as usize * 4;
    if ihl < IPV4_HDR_LEN || frame.len() < ETH_HDR_LEN + ihl { return None; }

    let proto = ip[9];
    let src_ip: [u8; 4] = ip[12..16].try_into().ok()?;
    let dst_ip: [u8; 4] = ip[16..20].try_into().ok()?;
    // Clamp L4 to the IP total-length. The guest TX buffer is bigger
    // than the packet (min-frame / driver padding); &ip[ihl..] would
    // append that garbage to every outbound segment → the server
    // misframes the response → Firefox reads a wild length → ~4 GiB
    // alloc → crash. Inbound is already clamped in net/ipv4.rs.
    let ip_total = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    if ip_total < ihl || ip_total > ip.len() { return None; }
    let l4 = &ip[ihl..ip_total];

    match proto {
        PROTO_UDP => {
            if l4.len() < UDP_HDR_LEN { return None; }
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
                handle_dns(src_ip, src_port, dgram)
            } else if dst_ip == GATEWAY_IP {
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
            // Unified data plane: when the host NIC does checksum + TSO, forward
            // the guest's GSO super-frame AS-IS (rewrite + incremental csum, the
            // mirror of RX) instead of SW re-segmenting it. Fall back to the SW
            // path only when offload isn't available.
            if crate::drivers::virtio_net::host_offload_ok() {
                l3_outbound_offload(src_port, dst_ip, dst_port, l4, gso_size)
            } else {
                l3_outbound(PROTO_TCP, src_port, dst_ip, dst_port, l4, gso_size)
            }
        }
        PROTO_ICMP => {
            if !caps.allow_icmp {
                cap_reject("ICMP", dst_ip, 0);
                return None;
            }
            l3_icmp_outbound(dst_ip, l4)
        }
        _ => None,
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

/// Drain host-side TCP recv buffers for every active session and
/// inject the resulting TCP segments into the guest's RX queue. Called
/// from the timer-tick path so async response data reaches the guest
/// without it having to emit fresh traffic.
///
/// Returns `true` iff at least one segment was injected (caller fires
/// IRQ 10 to wake the guest's virtio-net driver).
pub fn pump(
    net: &mut super::virtio_net_dev::VirtioNet,
    mem: &GuestMem,
) -> bool {
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    static PUMP_LOG: AtomicU32 = AtomicU32::new(0);
    static RX_IRQ_ROUTED: AtomicBool = AtomicBool::new(false);
    static NS_LAST_RXIRQ: AtomicU64 = AtomicU64::new(0); // for net-RX IRQ rate

    NS_PUMP_CALLS.fetch_add(1, AtOrd::Relaxed);

    let producer = super::net_dataplane::active();

    // net-RX: route the host NIC's RX IRQ to THIS (BSP-pump) core once, so RX
    // arrival wakes this core out of HLT → this pump delivers it promptly. Skip
    // when the dedicated producer is active — IT owns the RX IRQ (routed to its
    // own core); routing it here too would steal the wake from the producer.
    if !producer && !RX_IRQ_ROUTED.swap(true, Ordering::Relaxed) {
        crate::irq::route_to_current(crate::drivers::virtio_net::rx_irq_vector());
    }

    // Drain the host NIC RX ring (handle_frame → l3_inbound). The dedicated
    // producer does this on its own core when active, so the BSP must NOT also
    // drain (single-consumer ring); it only flushes the guest's batched TX
    // doorbell so host-originated/ACK egress isn't stranded.
    if producer {
        crate::virtio_net::tx_flush();
    } else {
        crate::net::poll();
    }

    let now = crate::interrupts::ticks();
    l3_reap(now);

    // Track NAT-table high-water + emit a host-side [netstat] summary every
    // ~5 s (≈500 ticks @ 100 Hz) so page-load slowness is diagnosable as
    // throughput vs NAT drops vs latency. Host log only — no [guest] spam.
    let inuse = L3.lock().iter().flatten().count() as u64;
    if inuse > NS_HIGHWATER.load(AtOrd::Relaxed) {
        NS_HIGHWATER.store(inuse, AtOrd::Relaxed);
    }
    let iq = INBOUND_Q.lock().len() as u64;
    if iq > NS_IQ_HI.load(AtOrd::Relaxed) {
        NS_IQ_HI.store(iq, AtOrd::Relaxed);
    }
    let last = NS_LAST_TICK.load(AtOrd::Relaxed);
    let dt = now.wrapping_sub(last);
    if last != 0 && dt >= 500 {
        let rxb = NS_RX_BYTES.swap(0, AtOrd::Relaxed);
        let rxp = NS_RX_PKTS.swap(0, AtOrd::Relaxed);
        let txb = NS_TX_BYTES.swap(0, AtOrd::Relaxed);
        let txp = NS_TX_PKTS.swap(0, AtOrd::Relaxed);
        let secs = dt / 100; // ticks → seconds (≥5)
        // RX delivery latency this window (TSC → µs): avg + max.
        let lat_n = NS_RXLAT_N.swap(0, AtOrd::Relaxed);
        let lat_sum = NS_RXLAT_SUM.swap(0, AtOrd::Relaxed);
        let lat_max = NS_RXLAT_MAX.swap(0, AtOrd::Relaxed);
        let mhz = (crate::interrupts::tsc_freq() / 1_000_000).max(1);
        let lat_avg_us = if lat_n > 0 { (lat_sum / lat_n) / mhz } else { 0 };
        let lat_max_us = lat_max / mhz;
        let pumps = NS_PUMP_CALLS.swap(0, AtOrd::Relaxed);
        let injfalse = NS_INJECT_FALSE.swap(0, AtOrd::Relaxed);
        // avg packets injected per pump call (batch size) — low pumps/s with a
        // big batch = pump-cadence-bound; high injfalse = guest-NAPI-bound.
        let batch = if pumps > 0 { rxp / pumps } else { 0 };
        // TEMP per-packet profiler: inject (consumer) ns/pkt + producer split
        // from poll_rx_only (netdev drain + l3_inbound "stack" + tx_flush).
        let inj_cyc = NS_INJECT_CYC.swap(0, AtOrd::Relaxed);
        let inj_ns = if lat_n > 0 { (inj_cyc / lat_n) * 1000 / mhz } else { 0 };
        let (p_netdev, p_stack, p_txflush, _p_render, p_pkts) = crate::net::take_poll_prof();
        let ns = |cyc: u64, n: u64| if n > 0 { (cyc / n) * 1000 / mhz } else { 0 };
        // Per-second microvm net stats. Re-enabled to verify net-RX: rxlat
        // (the latency root — should DROP), pump/s (should track RX), and the
        // RX-IRQ fire rate `rxirq/s` (proves the IRQ fires vs polling fallback).
        // The 5 s [netstat] dump BLOCKS the pump core ~60-120ms on the UART THRE
        // spin (serial.rs:93) — but v0.226.17 proved it's NOT the latency brake,
        // and we need `gpu KB/s` + `dl` to verify the GPU throttle engages.
        const NETSTAT_DEBUG: bool = false;
        // net-RX IRQ fire rate this window (0 + v0x0 = polling, IRQ never set up).
        let rx_vec = crate::drivers::virtio_net::rx_irq_vector();
        let rxirq_now = if rx_vec != 0 { crate::irq::fired_count(rx_vec) } else { 0 };
        let rxirq_per_s = rxirq_now
            .saturating_sub(NS_LAST_RXIRQ.swap(rxirq_now, AtOrd::Relaxed)) / secs;
        if NETSTAT_DEBUG && secs > 0 && (rxp + txp) > 0 {
            kprintln!(
                "[netstat] rx {} KB/s ({} pkt/s) tx {} KB/s ({} pkt/s) | rxlat avg {}us max {}us | rxirq {}/s (v{:#04x}) | iq hi {}/{} | flows {}tcp {}udp | {} drops | pump {}/s injfalse {}/s batch {}",
                rxb / 1024 / secs, rxp / secs, txb / 1024 / secs, txp / secs,
                lat_avg_us, lat_max_us,
                rxirq_per_s, rx_vec,
                NS_IQ_HI.load(AtOrd::Relaxed), INBOUND_MAX,
                NS_TCP_FLOWS.load(AtOrd::Relaxed), NS_UDP_FLOWS.load(AtOrd::Relaxed),
                NS_DROP_QUEUE.load(AtOrd::Relaxed),
                pumps / secs, injfalse / secs, batch,
            );
            kprintln!(
                "[netstat]   per-pkt: inject={}ns (consumer) | producer: netdev={}ns stack={}ns/pkt txflush={}ns/poll  poll_pkts={}",
                inj_ns, ns(p_netdev, p_pkts), ns(p_stack, p_pkts), ns(p_txflush, pumps), p_pkts,
            );
            // RX-GRO effectiveness: coalesced GSO frames/s + avg segments each
            // (the packet-reduction factor seen by the guest).
            let gro_frames = NS_GRO_FRAMES.swap(0, AtOrd::Relaxed);
            let gro_segs = NS_GRO_SEGS.swap(0, AtOrd::Relaxed);
            if GRO_ENABLED.load(AtOrd::Relaxed) {
                kprintln!(
                    "[netstat]   gro {}/s frames | avg {} segs/frame | {} segs/s merged",
                    gro_frames / secs,
                    if gro_frames > 0 { gro_segs / gro_frames } else { 0 },
                    gro_segs / secs,
                );
            }
            // Latency-spike attribution: drainmax = longest pump gap (µs),
            // rxring min = fewest RX buffers the guest had ready. If drainmax
            // ≈ rxlat max ⇒ pump starvation; if rxring min ≈ 0 ⇒ shallow ring.
            let drain_max = NS_DRAIN_GAP_MAX.swap(0, AtOrd::Relaxed);
            let rxring_min = NS_RXRING_MIN.swap(u64::MAX, AtOrd::Relaxed);
            let prod_drains = NS_PRODUCER_DRAINS.swap(0, AtOrd::Relaxed);
            let gtimer = NS_GTIMER.swap(0, AtOrd::Relaxed);
            let net_irq = NS_NET_IRQ.swap(0, AtOrd::Relaxed);
            let gpu_bytes = NS_GPU_BYTES.swap(0, AtOrd::Relaxed);
            let gpu_xfers = NS_GPU_XFERS.swap(0, AtOrd::Relaxed);
            let wraise = NS_WORKER_RAISE.swap(0, AtOrd::Relaxed);
            kprintln!(
                "[netstat]   drainmax {}us | rxring min {} | producer {} ({}/s) | gtimer {}/s | netirq {}/s wraise {}/s | gpu {}KB/s ({}/s) | dl {}",
                drain_max / mhz,
                if rxring_min == u64::MAX { 0 } else { rxring_min },
                if super::net_dataplane::active() { "on" } else { "off" },
                prod_drains / secs,
                gtimer / secs,
                net_irq / secs,
                wraise / secs,
                gpu_bytes / secs / 1024,
                gpu_xfers / secs,
                if download_active() { "Y" } else { "N" },
            );
        }
        NS_LAST_TICK.store(now, AtOrd::Relaxed);
    } else if last == 0 {
        NS_LAST_TICK.store(now, AtOrd::Relaxed);
    }

    let _ = PUMP_LOG;
    // Full RX backend: the worker fiber is the SOLE RX consumer. The BSP must NOT
    // also drain_inbound here — two consumers both call inject_rx +
    // rx_should_interrupt, and the double EVENT_IDX `signalled_used` update
    // corrupts the IRQ decision (the guest misses RX IRQs → NAPI stalls → no ACKs
    // → traffic dies). The tx_flush + reap + netstat above still run on the BSP.
    if super::net_backend::full_active() {
        return false;
    }
    drain_inbound(net, mem).want_irq
}

/// Lightweight pump for the HOT device-exit path: pull the host NIC + deliver to
/// the guest, WITHOUT the periodic NAT reap / netstat bookkeeping that pump()
/// does. Called on every guest virtio-net MMIO exit so RX delivery tracks the
/// guest's device activity instead of starving at the ~1500/s timer/HLT rate
/// (that starvation was the download ceiling: pump/s ≪ VM-exits/s).
pub fn pump_fast(
    net: &mut super::virtio_net_dev::VirtioNet,
    mem: &GuestMem,
) -> bool {
    NS_PUMP_CALLS.fetch_add(1, AtOrd::Relaxed);
    // Full backend (vhost): the `net_dataplane` worker is the SOLE RX consumer
    // (drains the NIC + injects directly) AND the TX servicer. The BSP must NOT
    // call drain_inbound — a SECOND `inject_rx` + `rx_should_interrupt` consumer
    // double-updates the EVENT_IDX `signalled_used`, corrupting the RX-IRQ
    // decision (the guest then misses RX IRQs → NAPI stalls → no ACKs → the
    // server's ACK-clock collapses). Only flush the guest's batched TX doorbell
    // so a host-originated frame egresses promptly between worker passes.
    if super::net_backend::full_active() {
        crate::virtio_net::tx_flush();
        return false;
    }
    // No producer: BSP is the sole drainer (legacy / VMX path). Drain backlog
    // FIRST so INBOUND_Q has headroom before poll_rx_only refills it (else a full
    // NIC-ring burst overflows an already-occupied queue → l3_inbound drops). Then
    // NIC-drain-only (NOT full poll(): the hot ~15k/s device-exit path must not run
    // tcp::tick_connections or shade::poll_render every call).
    let mut fired = drain_inbound(net, mem).want_irq;
    crate::net::poll_rx_only();
    fired |= drain_inbound(net, mem).want_irq;
    fired
}

/// Backend consumer half (Stage 2b): drain INBOUND_Q into the guest RX ring on
/// the dedicated worker core — the NIC drain (producer) already filled the queue
/// via `rx_producer_drain`. Returns true iff the guest wants its RX IRQ raised
/// (the worker then signals + kicks the BSP to inject IRQ10). Lets the whole RX
/// data-plane run off the vCPU.
pub fn drain_to_guest(
    net: &mut super::virtio_net_dev::VirtioNet,
    mem: &GuestMem,
) -> RxDrain {
    drain_inbound(net, mem)
}

/// Producer half (runs on the dedicated `net_dataplane` fiber, a separate core):
/// drain the host NIC RX ring through the IP stack into INBOUND_Q + flush any
/// GRO burst past its latency budget. Deliberately does NOT touch the guest
/// virtqueue — the BSP vCPU owns inject_rx. Keeps producer (NIC→queue) and
/// consumer (queue→guest) on different cores so they pipeline.
pub fn rx_producer_drain() {
    NS_PRODUCER_DRAINS.fetch_add(1, AtOrd::Relaxed);
    crate::net::poll_rx_only();
    if GRO_ENABLED.load(AtOrd::Relaxed) {
        gro_flush_expired(crate::interrupts::rdtsc());
    }
}

/// How often the dedicated RX producer fiber ran a drain pass this window
/// (printed in [netstat]). High = it's keeping up with the RX IRQ; the BSP's
/// `drainmax`/`pump` then reflect only the consumer (inject) cadence.
static NS_PRODUCER_DRAINS: AtomicU64 = AtomicU64::new(0);

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
/// DEBUG (Stage 2b): times the off-vCPU worker decided the guest wants an RX IRQ
/// (raise_irq + kick BSP). Compare to `netirq` (BSP actually injected IRQ10):
/// wraise≫netirq ⇒ the kick/inject isn't delivering; wraise≈netirq ⇒ delivery
/// is fine and any cap is RX volume / TX, not the IRQ path.
static NS_WORKER_RAISE: AtomicU64 = AtomicU64::new(0);
pub fn note_worker_raise() { NS_WORKER_RAISE.fetch_add(1, AtOrd::Relaxed); }
/// Times the worker kicked the parked BSP vCPU to inject RX it had STAGED while
/// the guest's EVENT_IDX suppressed the IRQ line (`injected && !want_irq`). Before
/// the scheduler-wake/IRQ-raise split these never woke the vCPU → it ate the 2ms
/// park timeout (the measured `kick_wait timeout` → the download throughput
/// lottery). High here = the decoupled wake is doing real work.
static NS_KICK_DECOUPLED: AtomicU64 = AtomicU64::new(0);
pub fn note_decoupled_kick() { NS_KICK_DECOUPLED.fetch_add(1, AtOrd::Relaxed); }
pub fn decoupled_kick_count() -> u64 { NS_KICK_DECOUPLED.load(AtOrd::Relaxed) }
/// Times the worker kicked the BSP because the guest RX ring was FULL (pending,
/// nothing injected) so the guest must run to drain+repost. High = the guest's
/// NAPI/repost is the limiter and the ring-full stall was real.
static NS_KICK_RINGFULL: AtomicU64 = AtomicU64::new(0);
pub fn note_ringfull_kick() { NS_KICK_RINGFULL.fetch_add(1, AtOrd::Relaxed); }
pub fn ringfull_kick_count() -> u64 { NS_KICK_RINGFULL.load(AtOrd::Relaxed) }

/// Bridge RX-backpressure health for `cores`, to find the SLOW-regime root:
/// (drops, inject_false, rxlat_max_tsc). drops = INBOUND_Q overflow drops — the
/// guest can't drain fast enough so RX is dropped → the SERVER retransmits +
/// collapses cwnd → the slow download regime. inject_false = guest RX ring full.
/// rxlat_max = worst staging latency (TSC ticks). Cumulative per VM run. If drops
/// climb during a slow GET, the lottery is server-cwnd-collapse from our drops.
pub fn rx_health_snapshot() -> (u64, u64, u64) {
    (NS_DROP_QUEUE.load(AtOrd::Relaxed),
     NS_INJECT_FALSE.load(AtOrd::Relaxed),
     NS_RXLAT_MAX.load(AtOrd::Relaxed))
}
/// virtio-gpu TRANSFER_TO_HOST pixel bytes copied on the vCPU core (the browser
/// rendering). If high during a download, the framebuffer copy is stealing vCPU
/// cycles from the net pump (the framebuffer↔pump contention) — Florian's
/// "graphics?" hypothesis, measured.
static NS_GPU_BYTES: AtomicU64 = AtomicU64::new(0);
static NS_GPU_XFERS: AtomicU64 = AtomicU64::new(0);
pub fn note_gpu_transfer(bytes: u64) {
    NS_GPU_BYTES.fetch_add(bytes, AtOrd::Relaxed);
    NS_GPU_XFERS.fetch_add(1, AtOrd::Relaxed);
}

/// Outcome of one staging-queue → guest RX-ring drain pass.
#[derive(Clone, Copy, Default)]
pub struct RxDrain {
    /// At least one RX buffer was injected into the guest this pass. Drives the
    /// vCPU SCHEDULER wake (a parked vCPU can't NAPI-poll a non-empty ring).
    pub injected: bool,
    /// The guest wants its RX IRQ line raised: EVENT_IDX threshold crossed AND
    /// NAPI hasn't suppressed. Drives `raise_irq` only. Implies `injected`.
    pub want_irq: bool,
    /// We had staged RX but the guest RX ring was FULL (inject_rx=false) — the
    /// frame is requeued. The guest must RUN to drain its ring + repost buffers,
    /// so the BSP must be kicked even though nothing was injected. Without this
    /// the parked BSP eats the full 2ms kick_wait timeout before reposting (the
    /// confirmed ring-full stall = a chunk of the download lottery).
    pub pending: bool,
}

/// Drain the staging queue into the guest RX ring. Shared by pump() + pump_fast().
fn drain_inbound(
    net: &mut super::virtio_net_dev::VirtioNet,
    mem: &GuestMem,
) -> RxDrain {
    // Pump-starvation probe: longest gap between drain passes this window.
    let drain_now = crate::interrupts::rdtsc();
    let last_drain = NS_LAST_DRAIN.swap(drain_now, AtOrd::Relaxed);
    if last_drain != 0 {
        let gap = drain_now.wrapping_sub(last_drain);
        if gap > NS_DRAIN_GAP_MAX.load(AtOrd::Relaxed) {
            NS_DRAIN_GAP_MAX.store(gap, AtOrd::Relaxed);
        }
    }
    // Min RX buffers the guest had ready at the start of a drain pass.
    let ring = net.rx_avail_count(mem);
    if ring < NS_RXRING_MIN.load(AtOrd::Relaxed) {
        NS_RXRING_MIN.store(ring, AtOrd::Relaxed);
    }

    // Push out any GRO burst that's been held past its latency budget, so its
    // superframe is delivered in this same drain pass.
    if GRO_ENABLED.load(AtOrd::Relaxed) {
        gro_flush_expired(crate::interrupts::rdtsc());
    }
    let mut any = false;
    let mut stalled = false;
    loop {
        let item = { INBOUND_Q.lock().pop_front() };
        let Some((push_tsc, frame)) = item else { break };
        // RX is flowing — mark activity even if the inject below fails on a full
        // guest ring, so the BSP keeps parking event-driven on the RX IRQ
        // (recently_active) through ring-full stretches instead of falling back
        // to the 10 ms timer sleep exactly when it's busiest.
        NS_LAST_ACTIVITY.store(drain_now, AtOrd::Relaxed);
        let inj_t0 = crate::interrupts::rdtsc();
        let injected = net.inject_rx(mem, &frame);
        NS_INJECT_CYC.fetch_add(crate::interrupts::rdtsc().saturating_sub(inj_t0), AtOrd::Relaxed);
        if injected {
            any = true;
            // RX delivery latency: how long this packet waited in the queue.
            let lat = crate::interrupts::rdtsc().saturating_sub(push_tsc);
            NS_RXLAT_SUM.fetch_add(lat, AtOrd::Relaxed);
            NS_RXLAT_N.fetch_add(1, AtOrd::Relaxed);
            if lat > NS_RXLAT_MAX.load(AtOrd::Relaxed) {
                NS_RXLAT_MAX.store(lat, AtOrd::Relaxed);
            }
            frame_pool_put(frame); // recycle the buffer (no per-packet free)
        } else {
            // Guest RX queue full (no avail buffer) — requeue + retry next pump.
            // High rate here = the GUEST is the limiter (NAPI/repost too slow).
            NS_INJECT_FALSE.fetch_add(1, AtOrd::Relaxed);
            INBOUND_Q.lock().push_front((push_tsc, frame));
            stalled = true;
            break;
        }
    }
    // Fire the guest's RX IRQ only if we delivered something AND the driver
    // hasn't suppressed interrupts (NAPI poll sets VIRTQ_AVAIL_F_NO_INTERRUPT).
    // Firing IRQ10 during a NAPI poll preempts the guest's ring-drain → it
    // reposts RX buffers slower → the ring exhausts (injfalse) → INBOUND_Q
    // overflows → drops → TCP sawtooth (the speedtest "200→60→200" oscillation).
    // The guest re-checks the used ring when it re-enables interrupts, so a
    // suppressed packet is never stranded.
    RxDrain { injected: any, want_irq: any && net.rx_wants_irq(mem), pending: stalled }
}

/// Everything the bridge knows about itself, for `netstat`.
///
/// The counters were always there; they lived behind a `NETSTAT_DEBUG` const
/// that has been `false` for months, so the one path nobody could see was the
/// one between the guest and the wire. This is the `wlan` treatment: no console
/// traffic, one screen on demand, and the numbers arranged so the reader can
/// tell the failures APART rather than watching a single "throughput sagged".
pub struct BridgeStats {
    pub active: bool,
    pub up_s: u64,
    pub kicks: u64, pub frames_in: u64, pub arp_in: u64, pub other_in: u64,
    pub rx_pkts: u64, pub rx_bytes: u64, pub rx_pps: u64,
    pub tx_pkts: u64, pub tx_bytes: u64, pub tx_pps: u64,
    pub window_ms: u64,
    pub flows_tcp: u64, pub flows_udp: u64,
    pub live: usize, pub cap: usize,
    pub iq: usize, pub iq_hi: u64, pub iq_cap: usize,
    pub rxring_min: u64,
    pub drop_queue: u64, pub drop_table: u64, pub drop_egress: u64,
    pub inject_false: u64,
    pub rxlat_avg_us: u64, pub rxlat_max_us: u64,
    pub net_irq: u64,
    pub gro: bool, pub gro_frames: u64, pub gro_segs: u64,
    pub gpu_kb: u64, pub gpu_xfers: u64, pub gpu_kbps: u64,
    pub gtimer_ps: u64, pub pump_ps: u64,
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
static RPT_PUMP: AtomicU64 = AtomicU64::new(0);

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
    let gt = NS_GTIMER.load(AtOrd::Relaxed);
    let prev_gt = RPT_GT.swap(gt, AtOrd::Relaxed);
    let pumps = NS_PUMP_CALLS.load(AtOrd::Relaxed);
    let prev_pump = RPT_PUMP.swap(pumps, AtOrd::Relaxed);
    let window_ms = if prev_tsc == 0 { 0 } else { now.wrapping_sub(prev_tsc) / khz };
    let per_s = |d: u64| if window_ms > 0 { d * 1000 / window_ms } else { 0 };
    let n = NS_RXLAT_N.load(AtOrd::Relaxed);
    let rxring_min = NS_RXRING_MIN.load(AtOrd::Relaxed);
    let start = NS_START_TICK.load(AtOrd::Relaxed);
    BridgeStats {
        active: L3_ACTIVE.load(AtOrd::Acquire),
        up_s: if start == 0 { 0 } else { crate::interrupts::ticks().wrapping_sub(start) / 100 },
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
        live: L3.lock().iter().flatten().count(), cap: L3_MAX,
        iq: INBOUND_Q.lock().len(),
        iq_hi: NS_IQ_HI.load(AtOrd::Relaxed), iq_cap: INBOUND_MAX,
        rxring_min: if rxring_min == u64::MAX { 0 } else { rxring_min },
        drop_queue: NS_DROP_QUEUE.load(AtOrd::Relaxed),
        drop_table: NS_DROP_TABLE.load(AtOrd::Relaxed),
        drop_egress: NS_DROP_EGRESS.load(AtOrd::Relaxed),
        inject_false: NS_INJECT_FALSE.load(AtOrd::Relaxed),
        rxlat_avg_us: if n > 0 { NS_RXLAT_SUM.load(AtOrd::Relaxed) / n / mhz } else { 0 },
        rxlat_max_us: NS_RXLAT_MAX.load(AtOrd::Relaxed) / mhz,
        net_irq: NS_NET_IRQ.load(AtOrd::Relaxed),
        gro: GRO_ENABLED.load(AtOrd::Relaxed),
        gro_frames: NS_GRO_FRAMES.load(AtOrd::Relaxed),
        gro_segs: NS_GRO_SEGS.load(AtOrd::Relaxed),
        gpu_kb: gpu_bytes / 1024,
        gpu_xfers: NS_GPU_XFERS.load(AtOrd::Relaxed),
        gpu_kbps: per_s(gpu_bytes.saturating_sub(prev_gpu)) / 1024,
        gtimer_ps: per_s(gt.saturating_sub(prev_gt)),
        pump_ps: per_s(pumps.saturating_sub(prev_pump)),
    }
}

/// Number of currently-active (non-closed) TCP sessions. The run_linux
/// idle-detection uses this to extend the timeout when traffic is in
/// flight.
pub fn active_session_count() -> usize {
    // Drives VM idle-detection: keep the guest scheduled while any
    // masquerade flow is live or a reply is still queued.
    L3.lock().iter().flatten().count() + INBOUND_Q.lock().len()
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
              &NS_DROP_QUEUE, &NS_DROP_TABLE, &NS_DROP_EGRESS,
              &NS_HIGHWATER, &NS_LAST_TICK, &NS_IQ_HI,
              &NS_RXLAT_SUM, &NS_RXLAT_N, &NS_RXLAT_MAX, &NS_INJECT_FALSE,
              &NS_TCP_FLOWS, &NS_UDP_FLOWS, &NS_GRO_FRAMES, &NS_GRO_SEGS,
              &NS_NET_IRQ, &RPT_TSC, &RPT_RX, &RPT_TX,
              &NS_GUEST_KICKS, &NS_GUEST_FRAMES, &NS_GUEST_ARP, &NS_GUEST_OTHER,
              &NS_GPU_BYTES, &NS_GPU_XFERS, &RPT_GPU,
              &NS_GTIMER, &NS_PUMP_CALLS, &RPT_GT, &RPT_PUMP] {
        c.store(0, AtOrd::Relaxed);
    }
    NS_START_TICK.store(crate::interrupts::ticks().max(1), AtOrd::Relaxed);
    NS_RXRING_MIN.store(u64::MAX, AtOrd::Relaxed);
    NS_LAST_ACTIVITY.store(0, AtOrd::Relaxed);
}
