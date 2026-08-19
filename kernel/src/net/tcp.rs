//! TCP — Transmission Control Protocol
//!
//! nopeekOS-optimized defaults:
//! - No Nagle (low latency for request/response)
//! - 40ms delayed ACK (not 200ms)
//! - Initial window: 10 segments
//! - 3 retries, max 10s timeout (fast failure)
//! - Capability-gated: no cap = no connection

use alloc::vec::Vec;
use alloc::collections::VecDeque;
use alloc::collections::BTreeMap;
use spin::{Mutex, Once};
use super::{ipv4, arp};

// RFC 6528 — Initial Sequence Number generation. Predictable ISNs (e.g. a
// raw tick counter) let an off-path attacker forge in-window segments on a
// listening socket. We mix a per-boot CSPRNG secret with the connection
// 4-tuple via BLAKE3-keyed-hash, then add a tick-derived monotonic counter
// so retried connections still grow forward.
static ISN_SECRET: Once<[u8; 32]> = Once::new();

fn isn_secret() -> &'static [u8; 32] {
    ISN_SECRET.call_once(|| crate::csprng::random_256())
}

fn generate_isn(saddr: [u8; 4], daddr: [u8; 4], sport: u16, dport: u16) -> u32 {
    let mut buf = [0u8; 12];
    buf[0..4].copy_from_slice(&saddr);
    buf[4..8].copy_from_slice(&daddr);
    buf[8..10].copy_from_slice(&sport.to_be_bytes());
    buf[10..12].copy_from_slice(&dport.to_be_bytes());
    let h = blake3::keyed_hash(isn_secret(), &buf);
    let b = h.as_bytes();
    let hash_part = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    // Monotonic component: 100 Hz tick × 2500 ≈ 4 µs ISN step (RFC 6528 §3
    // suggests a ~250 kHz clock). Wrap is fine — the secret-keyed hash
    // ensures the absolute value is unguessable per 4-tuple.
    let timer = (crate::interrupts::ticks() as u32).wrapping_mul(2500);
    hash_part.wrapping_add(timer)
}

// A single LibreWolf page load opens ~20+ parallel TLS connections
// (CDNs, telemetry, OCSP, …). 16 was a single-`https`-intent ceiling;
// the browser exhausts it instantly → connects fail / stall →
// PR_IO_TIMEOUT_ERROR. 128 matches the NAT session table.
const MAX_CONNECTIONS: usize = 128;
const MSS: u16 = 1460; // standard Ethernet MSS
// The SYN/SYN-ACK window is never scaled (RFC 7323), so it's capped at 16-bit.
const INITIAL_WINDOW: u16 = 65535;
// TCP Window Scaling (RFC 7323). Without it the window is capped at 64 KiB and
// throughput = 64 KiB / RTT (~4 MB/s on a CDN regardless of link/NIC — the
// observed global slowness). We advertise `free >> OUR_WSCALE`.
// WSCALE 8 so the 16-bit window field can express the full 8 MiB buffer
// (8 MiB >> 8 = 32768 ≤ 65535). History: WSCALE 5 / 1 MiB capped a flow at
// ~727 Mbit; WSCALE 7 / 4 MiB reached ~650 avg but never plateaued (4 MiB ≈
// the BDP to a ~35 ms-RTT mirror = throughput·RTT, so zero headroom → any RTT
// jitter underfills). 8 MiB = ~2× BDP headroom → fill the pipe to ~native.
const OUR_WSCALE: u8 = 8;

const RCV_WND_MIN: usize = 256 * 1024;
const RCV_WND_MAX: usize = RECV_BUF_SIZE;

// Link receive-capacity hint (bytes/sec) declared by the active NIC driver.
// Default u32::MAX = uncapped (full buffer window) — native gigabit, SuperSpeed,
// virtio. A link whose clean sustainable TCP rate is far below the buffer's
// implied window — a gigabit-wire dongle behind 480-Mbit USB — sets this to that
// rate, and the receive window is then `rate × measured RTT` (= BDP). Scaling by
// RTT keeps the offered RATE constant near and far, so it's not URL/RTT-tuned;
// the constant is the link CLASS's capacity (a hardware property), and it only
// applies to the NIC that sets it — so it can't clash with faster links.
static LINK_RX_RATE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(u32::MAX);

/// Declare the active link's clean RX capacity in bytes/sec (u32::MAX = uncapped).
pub fn set_link_rx_rate(bytes_per_sec: u32) {
    LINK_RX_RATE.store(bytes_per_sec, core::sync::atomic::Ordering::Relaxed);
}

/// Advertised receive window = min(free buffer, BDP) where BDP = link capacity ×
/// smoothed RTT (50 ms assumed until the first TSecr-derived RTT). Keeps the
/// sender's in-flight near the bandwidth-delay product instead of ramping the
/// whole buffer and overflowing a slow bottleneck — without a per-link constant.
fn recv_window(conn: &TcpConn) -> u16 {
    let rate = LINK_RX_RATE.load(core::sync::atomic::Ordering::Relaxed);
    let cap = if rate == u32::MAX {
        RCV_WND_MAX
    } else {
        let rtt_ticks = if conn.srtt_ticks > 0 { conn.srtt_ticks as u64 } else { 5 };
        // bytes/sec × ticks ÷ 100 Hz = bytes-in-flight for one RTT.
        ((rate as u64 * rtt_ticks / 100) as usize).clamp(RCV_WND_MIN, RCV_WND_MAX)
    };
    let free = RECV_BUF_SIZE.saturating_sub(conn.recv_buf.len()).min(cap);
    if conn.wscale_ok {
        (free >> OUR_WSCALE).min(65535) as u16
    } else {
        free.min(65535) as u16
    }
}

/// Merge [s,e) into the coalesced out-of-order run set (offsets from rcv_irs).
fn ooo_runs_add(runs: &mut BTreeMap<u32, u32>, s: u32, e: u32) {
    let mut s = s;
    let mut e = e;
    // Absorb a contiguous/overlapping left neighbour (greatest start < s).
    if let Some((&ls, &le)) = runs.range(..s).next_back() {
        if le >= s { s = ls; }
    }
    // Absorb every run starting within [s, e] (overlap or adjacency).
    let keys: alloc::vec::Vec<u32> = runs.range(s..=e).map(|(&k, _)| k).collect();
    for k in keys {
        if runs[&k] > e { e = runs[&k]; }
        runs.remove(&k);
    }
    runs.insert(s, e);
}

/// Drop/trim runs now delivered (everything below offset `want`).
fn ooo_runs_trim(runs: &mut BTreeMap<u32, u32>, want: u32) {
    let keys: alloc::vec::Vec<u32> =
        runs.range(..want).filter(|&(_, &e)| e <= want).map(|(&k, _)| k).collect();
    for k in keys { runs.remove(&k); }
    if let Some((&ks, &ke)) = runs.range(..want).next_back() {
        if ke > want { runs.remove(&ks); runs.insert(want, ke); }
    }
}
const MAX_RETRIES: u8 = 3;
const RETRY_TICKS_BASE: u64 = 100; // 1 second (100Hz)
// Cold-cache ARP while a SYN waits: one WiFi round trip between probes, and a
// total budget matching the old blocking pre-resolve (~500 ms) before the SYN
// goes out to broadcast regardless.
const ARP_RETRANS_TICKS: u64 = 5; // 50 ms
const ARP_MAX_TRIES: u8 = 10;
const FIN_TIMEOUT_TICKS: u64 = 6000; // 60 s, like Linux's tcp_fin_timeout
// Retransmit timeout for DATA. Base 200 ms (= Linux TCP_RTO_MIN, HZ/5),
// doubled per attempt (RFC 6298 style). 5 attempts was a number without a
// model: ~6 s, and a 1 GB transfer that saturated the send path killed the
// debug mirror mid-flight. Linux gives TCP_RETR2 = 15 (include/net/tcp.h).
// With the shift capped at 5 the RTO tops out at 6.4 s, so 15 attempts span
// ~70 s — patient and still bounded.
const RTO_TICKS_BASE: u64 = 20; // 200 ms
const MAX_DATA_RETRIES: u8 = 15;
// Ceiling on unacknowledged bytes held for retransmit. A peer that stops
// acknowledging must not grow this without bound; `send` refuses past it,
// which is the backpressure the caller needs to see.
const MAX_UNACKED: usize = 256 * 1024;
// 4 MiB receive buffer → ~4 MiB window with scaling → fills the bandwidth-delay
// product for ~gigabit even at tens-of-ms RTT (1 MiB was the cap at ~11 ms;
// higher-RTT CDNs need more). Grown lazily (VecDeque::new), so an idle
// connection costs nothing and only an actively-bursting one approaches 4 MiB.
// Host TCP only ever has a handful of live connections (OTA/https/dns), so the
// worst-case footprint is small; the guest browser uses its own (microvm) TCP.
const RECV_BUF_SIZE: usize = 8 * 1024 * 1024;
const DELAYED_ACK_TICKS: u64 = 4; // 40ms at 100Hz
// ACK coalescing: send one ACK per N in-order segments (a held ACK is still
// flushed by the 40 ms timer). 8 ≈ one ACK per ~11.7 KB at 1460 MSS, cutting
// our TX-ACK packet rate ~4× (38500→9600/s at 850 Mbit) — fewer packets through
// the single-threaded path (QEMU slirp) and less work on the busy-spin RX core.
// Safe now that timestamps give the sender a per-segment RTT regardless.
const ACK_COALESCE: u16 = 8;
// Cap on buffered out-of-order data per connection. Beyond this, new
// ahead-segments are dropped (the sender will retransmit) so a lossy link
// can't blow up the heap.
const OOO_MAX_BYTES: usize = 2 * 1024 * 1024;

// Out-of-order receive counters (diagnostic). `AHEAD` = a segment past rcv_nxt
// (a gap → the sender will have to retransmit); `BEHIND` = a duplicate at/below
// rcv_nxt (a retransmit we already have). A burst of AHEAD during a download =
// packet loss + go-back-N. Read+reset via take_ooo_stats().
static TCP_OOO_AHEAD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static TCP_OOO_BEHIND: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// (ahead, behind) out-of-order segment counts since the last call; resets both.
pub fn take_ooo_stats() -> (u32, u32) {
    use core::sync::atomic::Ordering::Relaxed;
    (TCP_OOO_AHEAD.swap(0, Relaxed), TCP_OOO_BEHIND.swap(0, Relaxed))
}

// Max recv_buf depth seen since last read (diagnostic). High (→RECV_BUF_SIZE) =
// our consumer/core can't drain fast enough → window closes → sender stalls
// (consumer-limited). Low = buffer drains fine → a tail slowdown is the sender
// throttling (bufferbloat backoff), not us.
static TCP_MAX_RXBUF: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Max recv-buffer depth (bytes) seen since the last call; resets to 0.
pub fn take_max_rxbuf() -> usize {
    TCP_MAX_RXBUF.swap(0, core::sync::atomic::Ordering::Relaxed)
}

// Segments we transmitted (mostly ACKs) — diagnostic. A bulk download flooding
// one ACK per packet shows up here as ~tens of thousands/s.
static TCP_TX_SEGS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Count of connection-originated segments sent since the last call; resets.
pub fn take_tx_segs() -> u32 {
    TCP_TX_SEGS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

// TCP flags
const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

const HEADER_LEN: usize = 20; // no options (options added separately for SYN)

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
enum State {
    Closed,
    Listen,
    SynReceived,
    SynSent,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    LastAck,
    TimeWait,
}

#[allow(dead_code)]
struct TcpConn {
    state: State,
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,

    // Sequence numbers
    snd_nxt: u32, // next byte to send
    snd_una: u32, // oldest unacknowledged
    snd_iss: u32, // initial send seq
    rcv_nxt: u32, // next expected from remote
    rcv_irs: u32, // initial recv seq

    // Buffers
    recv_buf: VecDeque<u8>,
    send_buf: Vec<u8>,
    // Out-of-order reassembly: segments received ahead of a gap, keyed by
    // stream offset (seq - rcv_irs). Without this a single lost packet forced
    // the sender into go-back-N (retransmit the whole window) which re-burst
    // and re-overflowed the USB-NIC FIFO → collapse. With it only the one lost
    // segment is retransmitted. Bounded by OOO_MAX_BYTES (else dropped → the
    // sender retransmits). Offsets assume < 4 GiB per connection.
    ooo: BTreeMap<u32, Vec<u8>>,
    ooo_bytes: usize,
    // Coalesced [start,end) runs of `ooo`, kept in sync — so building SACK
    // blocks is O(runs), not an O(n)-segments full-map scan per ACK (that cost
    // ~38µs/pkt once a large window let `ooo` reach thousands of entries and
    // collapsed the pipeline). Advisory: a desync only makes SACK suboptimal,
    // never corrupts data (the bytes still come from `ooo`).
    ooo_runs: BTreeMap<u32, u32>,
    // Smoothed RTT in ticks, from the peer's echoed TSecr (our TSval is ticks()).
    // Used by recv_window() to size the advertised window to link-capacity × RTT
    // (BDP) — so the window scales with the path, not a hardcoded value.
    srtt_ticks: u32,

    // Retransmit. `send_buf` holds every byte we sent and the peer has not
    // acknowledged, starting at `snd_una`; `rto_tick` is when the oldest of
    // them went out. Without this a single lost segment was lost FOREVER:
    // the peer keeps a hole it can never fill, buffers everything after it
    // out-of-order and delivers nothing more to its application, while our
    // side happily reports every send as a success. Invisible for browsing
    // (there the PEER retransmits to us and our own sends are one short
    // request), fatal for anything that streams outward — `debug` went mute
    // at the first radio loss while its keyboard direction kept working.
    retries: u8,
    last_send_tick: u64,
    rto_tick: u64,

    // Delayed ACK
    ack_pending: bool,
    ack_tick: u64,
    // In-order segments received since our last ACK (ACK-coalescing counter).
    acks_held: u16,
    // Bytes drained since our last recv()-side window-update ACK. Rate-limits
    // those ACKs so a bulk download doesn't emit one per recv() call (~70k/s).
    freed_since_winupd: u32,

    // Connection complete flag
    established: bool,
    closed: bool,
    error: bool,

    // Window scaling (RFC 7323). `wscale_ok` once both SYNs carried the
    // option; `snd_wscale` is the peer's shift (to scale their advertised
    // window). Our own advertised window is scaled by OUR_WSCALE.
    wscale_ok: bool,
    snd_wscale: u8,

    // TCP Timestamps (RFC 7323). `ts_ok` once both SYNs carried the option;
    // `ts_recent` = the peer's most recent in-order TSval, echoed as our TSecr
    // so the sender measures RTT per-segment (robust to our ACK jitter) →
    // accurate RTO → no spurious retransmits.
    ts_ok: bool,
    ts_recent: u32,

    // Selective ACK (RFC 2018). `sack_ok` once both SYNs carried SACK-permitted.
    // As the receiver we then tell the sender which out-of-order ranges we
    // already hold (straight from `ooo`), so it retransmits ONLY the real holes
    // instead of everything past the cumulative ACK — the difference between a
    // loss collapsing throughput and a one-segment recovery.
    sack_ok: bool,

    // Next-hop MAC not yet known: the SYN is held back until ARP answers.
    // Sending it to L2 broadcast instead is what most gateways drop, and the
    // recovery is then a full 1 s SYN retry.
    arp_pending: bool,
    arp_tries: u8,
}

static CONNECTIONS: Mutex<[Option<TcpConn>; MAX_CONNECTIONS]> = Mutex::new(
    [const { None }; MAX_CONNECTIONS]
);

static NEXT_PORT: Mutex<u16> = Mutex::new(49152);

fn alloc_port() -> u16 {
    let mut port = NEXT_PORT.lock();
    let p = *port;
    *port = if *port >= 65534 { 49152 } else { *port + 1 };
    p
}

/// Open a TCP connection WITHOUT waiting for the handshake: the handle comes
/// back at once, the caller asks `connect_status` until it answers.
///
/// This is the form modules get. A blocking wait inside a host call freezes
/// every other fiber on that worker core — including the WiFi driver, whose
/// card then goes unpolled for the whole wait (the RB pool holds milliseconds).
/// `fiber::pump_peers` cannot cover it: it returns early when called from
/// inside a fiber, and a module IS a fiber.
///
/// The cold-cache ARP wait becomes part of the same state machine: we ask
/// once here and hold the SYN back (`arp_pending`) until `tick_connections`
/// sees the answer. Sending it to broadcast meanwhile is what most gateways
/// drop — the symptom was `debug <ip> <port>` needing 2–3 attempts on a
/// fresh boot unless a `ping` had warmed the cache.
pub fn connect_start(remote_ip: [u8; 4], remote_port: u16) -> Result<usize, TcpError> {
    let local_port = alloc_port();
    let iss = generate_isn(arp::our_ip(), remote_ip, local_port, remote_port);

    // Non-blocking lookup — no CONNECTIONS lock held yet, but no waiting either.
    let arp_target = super::ipv4::arp_target_for(remote_ip);
    let arp_pending = arp_target != [255, 255, 255, 255]
        && arp::lookup(arp_target).is_none();
    if arp_pending { arp::request(arp_target); }

    let conn = TcpConn {
        state: State::SynSent,
        local_port,
        remote_ip,
        remote_port,
        snd_nxt: iss.wrapping_add(1),
        snd_una: iss,
        snd_iss: iss,
        rcv_nxt: 0,
        rcv_irs: 0,
        recv_buf: VecDeque::new(),
        ooo: BTreeMap::new(),
        ooo_bytes: 0,
        ooo_runs: BTreeMap::new(),
        srtt_ticks: 0,
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: crate::interrupts::ticks(),
        rto_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        acks_held: 0,
        freed_since_winupd: 0,
        established: false,
        closed: false,
        error: false,
        wscale_ok: false,
        snd_wscale: 0,
        ts_ok: false,
        ts_recent: 0,
        sack_ok: false,
        arp_pending,
        arp_tries: 1,
    };

    // Find free slot
    let handle = {
        let mut conns = CONNECTIONS.lock();
        // Reclaim free OR fully-Closed slots. Without the Closed clause
        // a Closed conn pins its slot forever (tick_connections only
        // moves TimeWait→Closed, never frees it) — under browser churn
        // every slot ends up a Closed corpse and connect() starves.
        let slot = conns.iter()
            .position(|c| c.is_none())
            .or_else(|| conns.iter()
                .position(|c| matches!(c, Some(x) if x.state == State::Closed)))
            .ok_or(TcpError::TooManyConnections)?;
        conns[slot] = Some(conn);
        slot
    };

    // Only when the next hop is known. Otherwise `tick_connections` releases it.
    if !arp_pending { send_syn(handle)?; }

    Ok(handle)
}

/// Connection state: 1 = usable, 0 = still handshaking, -1 = the peer hung up
/// cleanly, -2 = it FAILED (reset, or we ran out of retransmits — i.e. the
/// link stopped acknowledging).
///
/// The two negatives are worth separating: "the far end closed" and "the link
/// went dead under us" look identical to a caller that only sees failure, and
/// they are opposite faults.
pub fn connect_status(handle: usize) -> i32 {
    if handle >= MAX_CONNECTIONS { return -1; }
    match CONNECTIONS.lock()[handle] {
        Some(ref c) if c.error => -2,
        Some(ref c) if c.closed || c.state == State::Closed => -1,
        // Same predicate as `conn_healthy`: a peer FIN moves us to CloseWait
        // and sets `closed`, so a module polling this learns the far end hung
        // up. `recv` never tells it — it just returns 0 bytes forever, which
        // is why `debug` kept running after `nc` was closed.
        Some(ref c) if c.established && c.state == State::Established => 1,
        Some(_) => 0,
        None => -1,
    }
}

/// Open a TCP connection, blocking until established. NATIVE callers only —
/// they run as a task on a worker core, where `super::poll` pumps the peer
/// fibers so the NIC keeps being drained while we wait. A module must use
/// `connect_start` + `connect_status` instead; see the note there.
pub fn connect(remote_ip: [u8; 4], remote_port: u16) -> Result<usize, TcpError> {
    let handle = connect_start(remote_ip, remote_port)?;

    // Wait for ESTABLISHED (blocking poll)
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        match connect_status(handle) {
            1 => break,
            n if n < 0 => {
                close_cleanup(handle);
                return Err(TcpError::ConnectionRefused);
            }
            _ => {}
        }

        if crate::interrupts::ticks() - t0 > 1000 { // 10s timeout
            close_cleanup(handle);
            return Err(TcpError::Timeout);
        }
        core::hint::spin_loop();
    }

    Ok(handle)
}

/// Listen on a local port. Returns handle. Use accept() to wait for connection.
#[allow(dead_code)]
pub fn listen(port: u16) -> Result<usize, TcpError> {
    let conn = TcpConn {
        state: State::Listen,
        local_port: port,
        remote_ip: [0; 4],
        remote_port: 0,
        snd_nxt: 0,
        snd_una: 0,
        snd_iss: 0,
        rcv_nxt: 0,
        rcv_irs: 0,
        recv_buf: VecDeque::new(),
        ooo: BTreeMap::new(),
        ooo_bytes: 0,
        ooo_runs: BTreeMap::new(),
        srtt_ticks: 0,
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: 0,
        rto_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        acks_held: 0,
        freed_since_winupd: 0,
        established: false,
        closed: false,
        error: false,
        wscale_ok: false,
        snd_wscale: 0,
        ts_ok: false,
        ts_recent: 0,
        sack_ok: false,
        arp_pending: false,
        arp_tries: 0,
    };

    let mut conns = CONNECTIONS.lock();
    let slot = conns.iter().position(|c| c.is_none())
        .ok_or(TcpError::TooManyConnections)?;
    conns[slot] = Some(conn);
    Ok(slot)
}

/// Wait for an incoming connection on a listening handle. Blocking.
#[allow(dead_code)]
pub fn accept(handle: usize, timeout_ticks: u64) -> Result<(), TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        let conns = CONNECTIONS.lock();
        if let Some(ref c) = conns[handle] {
            if c.established { return Ok(()); }
            if c.error || c.closed {
                drop(conns);
                return Err(TcpError::ConnectionFailed);
            }
        } else {
            return Err(TcpError::NotConnected);
        }
        drop(conns);

        if timeout_ticks > 0 && crate::interrupts::ticks() - t0 > timeout_ticks {
            return Err(TcpError::Timeout);
        }
        core::hint::spin_loop();
    }
}

/// Check if a listening handle has an established connection (non-blocking).
#[allow(dead_code)]
pub fn is_established(handle: usize) -> bool {
    let conns = CONNECTIONS.lock();
    conns[handle].as_ref().map_or(false, |c| c.established)
}

/// Reset a connection back to Listen state (for accepting next client).
#[allow(dead_code)]
pub fn reset_to_listen(handle: usize) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;
    let port = conn.local_port;

    *conn = TcpConn {
        state: State::Listen,
        local_port: port,
        remote_ip: [0; 4],
        remote_port: 0,
        snd_nxt: 0,
        snd_una: 0,
        snd_iss: 0,
        rcv_nxt: 0,
        rcv_irs: 0,
        recv_buf: VecDeque::new(),
        ooo: BTreeMap::new(),
        ooo_bytes: 0,
        ooo_runs: BTreeMap::new(),
        srtt_ticks: 0,
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: 0,
        rto_tick: 0,
        ack_pending: false,
        ack_tick: 0,
        acks_held: 0,
        freed_since_winupd: 0,
        established: false,
        closed: false,
        error: false,
        wscale_ok: false,
        snd_wscale: 0,
        ts_ok: false,
        ts_recent: 0,
        sack_ok: false,
        arp_pending: false,
        arp_tries: 0,
    };
    Ok(())
}

/// Send data on a connection. Buffers and sends immediately (no Nagle).
///
/// The bytes are ALSO kept in `send_buf` until the peer acknowledges them,
/// so `tick_connections` can retransmit. Returns `WouldBlock` when too much
/// is already unacknowledged — that is real backpressure, not an error:
/// before, every send was reported as a success and a lost segment simply
/// vanished.
pub fn send(handle: usize, data: &[u8]) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;
    if conn.state != State::Established { return Err(TcpError::NotConnected); }
    if conn.send_buf.len() + data.len() > MAX_UNACKED {
        return Err(TcpError::WouldBlock);
    }

    let now = crate::interrupts::ticks();
    // Oldest unacked byte starts its clock now if nothing was in flight.
    if conn.send_buf.is_empty() { conn.rto_tick = now; conn.retries = 0; }
    conn.send_buf.extend_from_slice(data);

    // Send in MSS-sized chunks immediately (no Nagle)
    for chunk in data.chunks(MSS as usize) {
        let seq = conn.snd_nxt;
        conn.snd_nxt = conn.snd_nxt.wrapping_add(chunk.len() as u32);
        conn.last_send_tick = now;
        let w = recv_window(conn);
        send_seg(conn, seq, conn.rcv_nxt, ACK | PSH, w, chunk);
    }

    Ok(())
}

/// Send, waiting out backpressure. NATIVE callers only — the same rule as
/// `connect`: this polls, which pumps the peer fibers on a worker core. A
/// module must handle `WouldBlock` itself and sleep between tries.
pub fn send_blocking(handle: usize, data: &[u8], timeout_ticks: u64) -> Result<(), TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        match send(handle, data) {
            Err(TcpError::WouldBlock) => {}
            other => return other,
        }
        if crate::interrupts::ticks() - t0 > timeout_ticks { return Err(TcpError::Timeout); }
        super::poll();
        tick_connections();
        core::hint::spin_loop();
    }
}

/// Receive data. Returns available data (may be empty if nothing received yet).
/// Sends a window update ACK if significant buffer space was freed.
pub fn recv(handle: usize, buf: &mut [u8]) -> Result<usize, TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;

    let pre_len = conn.recv_buf.len();
    let available = pre_len.min(buf.len());
    // Bulk copy out of the ring buffer instead of byte-by-byte pop_front
    // (that was ~112M pop_front/s at 100 MB/s — pure call overhead). The
    // VecDeque exposes its contents as up to two contiguous slices; memcpy
    // each, then drain in one shot.
    {
        let (a, b) = conn.recv_buf.as_slices();
        let na = a.len().min(available);
        buf[..na].copy_from_slice(&a[..na]);
        if na < available {
            buf[na..available].copy_from_slice(&b[..available - na]);
        }
        conn.recv_buf.drain(..available);
    }

    // Window-update ACK — RATE-LIMITED.
    //
    // We must re-advertise the window the consumer just reopened so a
    // trickle / zero-window sender resumes (the case this ACK was added for:
    // a TLS sender that bursts then goes quiet, no more handle_tcp data-ACKs,
    // → window stuck small → peer zero-window-probes → ~31 KiB/s sawtooth).
    // But a BULK plain-http download calls recv() once PER PACKET (~70k/s, not
    // per ~16 KiB TLS record), so ACKing on every drain floods the TX path:
    // ~70k ACKs/s, each an alloc + a virtio TX-doorbell VM-exit → pegs the
    // worker core AND defeats the handle_tcp ACK-coalescing.
    //
    // So: ACK immediately only when the window was actually CONSTRAINED
    // (buffer >1/4 full → window shrinking, the trickle/zero-window case),
    // otherwise at most once per ~64 KiB freed. handle_tcp's coalesced
    // data-ACKs carry the (wide-open) window the rest of the time.
    conn.freed_since_winupd = conn.freed_since_winupd.saturating_add(available as u32);
    let constrained = pre_len > RECV_BUF_SIZE / 4;
    if available > 0 && conn.state == State::Established
        && (constrained || conn.freed_since_winupd >= 64 * 1024) {
        let w = recv_window(conn);
        send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
        conn.freed_since_winupd = 0;
        conn.ack_pending = false;
        conn.acks_held = 0;
    }

    Ok(available)
}

/// Receive with blocking wait (polls until data or timeout).
pub fn recv_blocking(handle: usize, buf: &mut [u8], timeout_ticks: u64) -> Result<usize, TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        // NIC-drain only (the TLS / OTA-https recv hot path). The old code ran
        // the FULL super::poll() — tcp::tick_connections (128-slot scan +
        // CONNECTIONS lock) + shade::poll_render — AND then tick_connections()
        // AGAIN, every spin iteration at ~1 M/s: double the 128-slot scan + lock,
        // contending the CONNECTIONS lock with actual packet processing →
        // pegged the worker core AND throttled https/OTA throughput. Core 0's
        // poll() runs the TCP timers; here we just drain RX, like tcp_recv_poll.
        super::poll_rx_only();

        let n = recv(handle, buf)?;
        if n > 0 { return Ok(n); }

        // Check if connection closed
        {
            let conns = CONNECTIONS.lock();
            if let Some(ref c) = conns[handle] {
                if c.closed || c.error { return Ok(0); }
            } else {
                return Err(TcpError::NotConnected);
            }
        }

        if crate::interrupts::ticks() - t0 > timeout_ticks {
            return Ok(0);
        }
        // Timer-NAPI: HLT instead of spinning (the OTA-update / https core-peg
        // Florian saw — same root as tcp_recv_poll). Records the halt so `cores`
        // is honest. Wakes on the per-core timer (100 Hz here; OTA payloads are
        // small so the latency is fine), the NIC re-fills the ring in the gap.
        crate::interrupts::worker_idle_hlt();
    }
}

/// True if `handle` is an established, un-closed, un-errored connection —
/// i.e. safe to send another request on (HTTP keep-alive reuse). A peer
/// FIN moves the state out of `Established` (→ CloseWait) and sets
/// `closed`, so a server that dropped an idle keep-alive connection reads
/// as unhealthy here and the caller reconnects instead of hanging.
pub fn conn_healthy(handle: usize) -> bool {
    let conns = CONNECTIONS.lock();
    matches!(conns.get(handle), Some(Some(c))
        if c.state == State::Established && !c.closed && !c.error)
}

/// Close a connection gracefully (sends FIN).
/// Close without waiting for the peer's FIN. For MODULES: the waiting form
/// below spins up to 2 s, and a host call that spins freezes every other
/// fiber on that worker core — the same trap as the old blocking connect.
/// Aborting a connection that never came up costs those 2 s for nothing:
/// its state is SynSent, which the wait loop does not even accept as an end.
///
/// The FIN goes out, the slot stays in FinWait1, and `tick_connections`
/// reaps it if the peer never answers.
pub fn close_nowait(handle: usize) -> Result<(), TcpError> {
    if handle >= MAX_CONNECTIONS { return Err(TcpError::NotConnected); }
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;
    if conn.state == State::Established {
        let seq = conn.snd_nxt;
        conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
        conn.state = State::FinWait1;
        conn.last_send_tick = crate::interrupts::ticks();
        send_seg(conn, seq, conn.rcv_nxt, FIN | ACK, 0, &[]);
    } else {
        // Never established, or already shutting down — nothing to say.
        conns[handle] = None;
    }
    Ok(())
}

pub fn close(handle: usize) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;

    if conn.state == State::Established {
        let seq = conn.snd_nxt;
        conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
        conn.state = State::FinWait1;
        send_seg(conn, seq, conn.rcv_nxt, FIN | ACK, 0, &[]);
    }
    drop(conns);

    // Wait briefly for FIN-ACK
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        tick_connections();

        let conns = CONNECTIONS.lock();
        match conns[handle].as_ref().map(|c| c.state) {
            Some(State::TimeWait) | Some(State::Closed) | None => break,
            _ => {}
        }
        drop(conns);
        if crate::interrupts::ticks() - t0 > 200 { break; } // 2s
        core::hint::spin_loop();
    }

    close_cleanup(handle);
    Ok(())
}

/// Handle incoming TCP segment (called from ipv4)
pub fn handle_tcp(ip_packet: &[u8], data: &[u8]) {
    if data.len() < HEADER_LEN { return; }

    let src_port = u16::from_be_bytes([data[0], data[1]]);
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ack = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let data_offset = ((data[12] >> 4) as usize) * 4;
    let flags = data[13];
    let _window = u16::from_be_bytes([data[14], data[15]]);

    let src_ip = <[u8; 4]>::try_from(&ip_packet[12..16]).unwrap();
    let payload = if data_offset < data.len() { &data[data_offset..] } else { &[] };

    let mut conns = CONNECTIONS.lock();

    // Find matching connection
    let idx = conns.iter().position(|c| {
        c.as_ref().map_or(false, |c|
            c.local_port == dst_port && c.remote_port == src_port && c.remote_ip == src_ip
        )
    });

    let idx = match idx {
        Some(i) => i,
        None => {
            // Check for a listener on this port
            if flags & SYN != 0 {
                let listen_idx = conns.iter().position(|c| {
                    c.as_ref().map_or(false, |c|
                        c.local_port == dst_port && c.state == State::Listen
                    )
                });
                if let Some(li) = listen_idx {
                    // Accept the SYN on the listening socket
                    let iss = generate_isn(arp::our_ip(), src_ip, dst_port, src_port);
                    let peer_ws = parse_wscale(data, data_offset);
                    let conn = conns[li].as_mut().unwrap();
                    conn.state = State::SynReceived;
                    conn.remote_ip = src_ip;
                    conn.remote_port = src_port;
                    conn.rcv_irs = seq;
                    conn.rcv_nxt = seq.wrapping_add(1);
                    conn.snd_iss = iss;
                    conn.snd_nxt = iss.wrapping_add(1);
                    conn.snd_una = iss;
                    conn.last_send_tick = crate::interrupts::ticks();
                    // Scaling is active only if the peer offered it too.
                    conn.wscale_ok = peer_ws.is_some();
                    conn.snd_wscale = peer_ws.unwrap_or(0);

                    // SYN-ACK: MSS, and Window Scale only if the peer asked for it.
                    let opts: &[u8] = if peer_ws.is_some() {
                        &[2, 4, (MSS >> 8) as u8, MSS as u8, 1, 3, 3, OUR_WSCALE]
                    } else {
                        &[2, 4, (MSS >> 8) as u8, MSS as u8]
                    };
                    send_segment_with_opts(
                        src_ip, dst_port, src_port,
                        iss, seq.wrapping_add(1), SYN | ACK, INITIAL_WINDOW, &[], opts,
                    );
                    return;
                }
            }
            // No connection and no listener: send RST if not RST
            if flags & RST == 0 {
                send_segment(src_ip, dst_port, src_port, ack, seq.wrapping_add(1), RST | ACK, 0, &[]);
            }
            return;
        }
    };

    let conn = conns[idx].as_mut().unwrap();

    // RST handling
    if flags & RST != 0 {
        conn.error = true;
        conn.state = State::Closed;
        return;
    }

    match conn.state {
        State::SynReceived => {
            // Waiting for ACK of our SYN-ACK
            if flags & ACK != 0 {
                conn.snd_una = ack;
                conn.state = State::Established;
                conn.established = true;
            }
        }

        State::SynSent => {
            if flags & SYN != 0 && flags & ACK != 0 {
                // SYN-ACK received
                conn.rcv_irs = seq;
                conn.rcv_nxt = seq.wrapping_add(1);
                conn.snd_una = ack;
                conn.state = State::Established;
                conn.established = true;

                // We always offer WScale in our SYN, so scaling is active iff
                // the SYN-ACK carries it. Set before the ACK so it advertises
                // the scaled window immediately.
                if let Some(ws) = parse_wscale(data, data_offset) {
                    conn.snd_wscale = ws;
                    conn.wscale_ok = true;
                }
                // Timestamps active iff the SYN-ACK echoes the option (RFC 7323).
                // Seed ts_recent with the peer's TSval so our handshake ACK
                // already carries a valid TSecr.
                if let Some(ts) = parse_ts(data, data_offset) {
                    conn.ts_ok = true;
                    conn.ts_recent = ts;
                }
                // SACK active iff the SYN-ACK also carried SACK-permitted.
                conn.sack_ok = parse_sack_permitted(data, data_offset);

                // Send ACK with full window
                let w = recv_window(conn);
                send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
            }
        }

        State::Established => {
            // ACK processing
            if flags & ACK != 0 {
                if ack_in_range(conn.snd_una, ack, conn.snd_nxt) {
                    // Drop the acknowledged prefix from the retransmit queue
                    // and restart the timer for whatever is still in flight.
                    let acked = ack.wrapping_sub(conn.snd_una) as usize;
                    let drop_n = acked.min(conn.send_buf.len());
                    conn.send_buf.drain(..drop_n);
                    conn.snd_una = ack;
                    conn.retries = 0;
                    conn.rto_tick = crate::interrupts::ticks();
                }
            }

            // Data processing
            if !payload.is_empty() {
                if seq == conn.rcv_nxt {
                    // RFC 7323: advance ts_recent to this in-order segment's
                    // TSval so our echoed TSecr gives the sender a fresh RTT.
                    if conn.ts_ok {
                        if let Some(ts) = parse_ts(data, data_offset) {
                            conn.ts_recent = ts;
                        }
                    }
                    let space = RECV_BUF_SIZE - conn.recv_buf.len();
                    let copy = payload.len().min(space);
                    // Bulk append — NOT byte-by-byte push_back (that was ~87M
                    // push_back/s at ~700 Mbit). extend reserves once + copies.
                    conn.recv_buf.extend(payload[..copy].iter().copied());
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(copy as u32);
                    // Gap just filled — pull any now-contiguous segments out of
                    // the reassembly queue. Only the lowest stored offset can be
                    // next; if it doesn't meet rcv_nxt there's still a hole.
                    let mut filled = false;
                    loop {
                        let want = conn.rcv_nxt.wrapping_sub(conn.rcv_irs);
                        // Peek the lowest stored offset (copy out k+len so the
                        // immutable borrow ends before we remove).
                        let (k, seglen) = match conn.ooo.iter().next() {
                            Some((&k, seg)) => (k, seg.len()),
                            None => break,
                        };
                        // Drop fully-stale segments (already delivered).
                        if (k as usize) + seglen <= want as usize {
                            conn.ooo.remove(&k); conn.ooo_bytes -= seglen; continue;
                        }
                        if k != want { break; }                 // still a gap before it
                        if conn.recv_buf.len() + seglen > RECV_BUF_SIZE { break; }
                        let seg = conn.ooo.remove(&k).unwrap();
                        conn.ooo_bytes -= seg.len();
                        conn.rcv_nxt = conn.rcv_nxt.wrapping_add(seg.len() as u32);
                        conn.recv_buf.extend(seg.into_iter());
                        filled = true;
                    }
                    TCP_MAX_RXBUF.fetch_max(conn.recv_buf.len(),
                        core::sync::atomic::Ordering::Relaxed);

                    // Keep the SACK run-set in sync with what's now delivered, and
                    // refresh the RTT estimate from the peer's echoed TSecr (our
                    // TSval is ticks(), so ticks()-TSecr = RTT). recv_window() turns
                    // that into the window via link-capacity × RTT.
                    let delivered = conn.rcv_nxt.wrapping_sub(conn.rcv_irs);
                    ooo_runs_trim(&mut conn.ooo_runs, delivered);
                    if conn.ts_ok {
                        if let Some(tsecr) = parse_tsecr(data, data_offset) {
                            if tsecr != 0 {
                                let sample = (crate::interrupts::ticks() as u32).wrapping_sub(tsecr);
                                if (1..6000).contains(&sample) {
                                    conn.srtt_ticks = if conn.srtt_ticks == 0 { sample }
                                        else { (conn.srtt_ticks * 7 + sample) / 8 };
                                }
                            }
                        }
                    }
                    // A filled gap must be ACKed immediately so the sender stops
                    // retransmitting and advances — don't let it sit in coalescing.
                    if filled {
                        let w = recv_window(conn);
                        send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                        conn.acks_held = 0;
                        conn.ack_pending = false;
                    } else {
                    // Coalesced ACK: one ACK per ACK_COALESCE in-order segments.
                    // A lone held ACK is flushed by the 40 ms timer in
                    // tick_connections so a trickle/idle never strands the sender.
                    conn.acks_held += 1;
                    if conn.acks_held >= ACK_COALESCE {
                        let w = recv_window(conn);
                        send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                        conn.acks_held = 0;
                        conn.ack_pending = false;
                    } else {
                        conn.ack_pending = true;
                        conn.ack_tick = crate::interrupts::ticks();
                    }
                    }
                } else {
                    use core::sync::atomic::Ordering::Relaxed;
                    if (seq.wrapping_sub(conn.rcv_nxt) as i32) > 0 {
                        // AHEAD = a real gap (an earlier segment was lost). Buffer
                        // this segment for reassembly + send a duplicate ACK so the
                        // sender fast-retransmits ONLY the hole (RFC 5681) — not the
                        // whole window. Bounded; over budget or already-have → skip.
                        TCP_OOO_AHEAD.fetch_add(1, Relaxed);
                        let off = seq.wrapping_sub(conn.rcv_irs);
                        if !payload.is_empty()
                            && !conn.ooo.contains_key(&off)
                            && conn.ooo_bytes + payload.len() <= OOO_MAX_BYTES
                        {
                            conn.ooo_bytes += payload.len();
                            conn.ooo.insert(off, payload.to_vec());
                            ooo_runs_add(&mut conn.ooo_runs,
                                off, off.wrapping_add(payload.len() as u32));
                        }
                        let w = recv_window(conn);
                        send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                        conn.ack_pending = false;
                    } else {
                        // BEHIND = a pure duplicate (data we already have). Do NOT
                        // re-ACK: that ACK is itself a duplicate ACK and 3 of them
                        // make the sender fast-retransmit → a spurious-retransmit
                        // feedback loop (v0.219.7/8 doubled dup this way). Count
                        // only; the next in-order segment's ACK re-syncs the sender.
                        TCP_OOO_BEHIND.fetch_add(1, Relaxed);
                    }
                }
            }

            // FIN from remote
            if flags & FIN != 0 {
                conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                conn.state = State::CloseWait;
                conn.closed = true;
                // ACK the FIN
                send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, 0, &[]);
            }

        }

        State::FinWait1 => {
            if flags & ACK != 0 {
                conn.snd_una = ack;
                if flags & FIN != 0 {
                    conn.rcv_nxt = seq.wrapping_add(1);
                    conn.state = State::TimeWait;
                    send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, 0, &[]);
                } else {
                    conn.state = State::FinWait2;
                }
            }
        }

        State::FinWait2 => {
            if flags & FIN != 0 {
                conn.rcv_nxt = seq.wrapping_add(1);
                conn.state = State::TimeWait;
                send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, 0, &[]);
            }
        }

        State::LastAck => {
            if flags & ACK != 0 {
                conn.state = State::Closed;
            }
        }

        _ => {}
    }
}

/// Periodic tick: retransmit, delayed ACKs, timeouts
pub fn tick_connections() {
    let now = crate::interrupts::ticks();

    // Collect the segments to send WHILE holding the lock (they read conn
    // state), then drop the lock and hit the NIC. Holding CONNECTIONS
    // across the TX doorbell blocked worker-core `recv` behind Core-0's
    // periodic ACKs/retries (contention ④).
    let mut pending: alloc::vec::Vec<PendingSeg> = alloc::vec::Vec::new();
    // Same reason: `arp::request` hits the NIC, so collect and fire after the
    // lock is gone.
    let mut arp_probes: alloc::vec::Vec<[u8; 4]> = alloc::vec::Vec::new();
    // Retransmits carry a payload, so they cannot ride in `pending`.
    let mut retrans: alloc::vec::Vec<(PendingSeg, alloc::vec::Vec<u8>)> =
        alloc::vec::Vec::new();
    {
        let mut conns = CONNECTIONS.lock();
        for slot in conns.iter_mut().flatten() {
            // Delayed ACK
            if slot.ack_pending && now - slot.ack_tick >= DELAYED_ACK_TICKS {
                let w = recv_window(slot);
                let mut opts = [0u8; 40];
                let len = build_seg_opts(slot, ACK, &mut opts);
                pending.push(PendingSeg {
                    dst_ip: slot.remote_ip, src_port: slot.local_port,
                    dst_port: slot.remote_port, seq: slot.snd_nxt,
                    ack: slot.rcv_nxt, flags: ACK, window: w, opts, opts_len: len,
                });
                slot.ack_pending = false;
                slot.acks_held = 0;
            }

            // SYN retry
            if slot.state == State::SynSent {
                let mut opts = [0u8; 40];
                let opts_len = syn_opts(&mut opts);
                let mut send_syn_now = false;

                if slot.arp_pending {
                    // Next hop still unknown, SYN held back. Re-ask every
                    // RETRANS window — one request is a coin flip over WiFi,
                    // and both the request and the reply can be the loss.
                    let target = ipv4::arp_target_for(slot.remote_ip);
                    if arp::lookup(target).is_some() {
                        slot.arp_pending = false;
                        send_syn_now = true;
                    } else if now.wrapping_sub(slot.last_send_tick) >= ARP_RETRANS_TICKS {
                        slot.arp_tries += 1;
                        slot.last_send_tick = now;
                        if slot.arp_tries > ARP_MAX_TRIES {
                            // Give up asking and send anyway (to broadcast),
                            // exactly as the old ~500 ms pre-resolve did on
                            // timeout. From here the normal SYN retry runs.
                            slot.arp_pending = false;
                            send_syn_now = true;
                        } else {
                            arp_probes.push(target);
                        }
                    }
                } else {
                    let retry_interval = RETRY_TICKS_BASE << slot.retries.min(4);
                    if now - slot.last_send_tick > retry_interval {
                        if slot.retries >= MAX_RETRIES {
                            slot.error = true;
                            slot.state = State::Closed;
                        } else {
                            slot.retries += 1;
                            send_syn_now = true;
                        }
                    }
                }

                if send_syn_now {
                    slot.last_send_tick = now;
                    pending.push(PendingSeg {
                        dst_ip: slot.remote_ip, src_port: slot.local_port,
                        dst_port: slot.remote_port, seq: slot.snd_iss,
                        ack: 0, flags: SYN, window: INITIAL_WINDOW,
                        opts, opts_len,
                    });
                }
            }

            // Data retransmit. `send_buf` starts at snd_una, so the head of
            // it is exactly the segment the peer is missing.
            if slot.state == State::Established && !slot.send_buf.is_empty() {
                let rto = RTO_TICKS_BASE << slot.retries.min(5);
                if now.saturating_sub(slot.rto_tick) > rto {
                    if slot.retries >= MAX_DATA_RETRIES {
                        slot.error = true;
                        slot.state = State::Closed;
                    } else {
                        slot.retries += 1;
                        slot.rto_tick = now;
                        slot.last_send_tick = now;
                        let n = slot.send_buf.len().min(MSS as usize);
                        let w = recv_window(slot);
                        let mut opts = [0u8; 40];
                        let len = build_seg_opts(slot, ACK, &mut opts);
                        retrans.push((
                            PendingSeg {
                                dst_ip: slot.remote_ip, src_port: slot.local_port,
                                dst_port: slot.remote_port, seq: slot.snd_una,
                                ack: slot.rcv_nxt, flags: ACK | PSH, window: w,
                                opts, opts_len: len,
                            },
                            slot.send_buf[..n].to_vec(),
                        ));
                    }
                }
            }

            // TimeWait cleanup (2 seconds)
            if slot.state == State::TimeWait && now - slot.last_send_tick > 200 {
                slot.state = State::Closed;
            }

            // Half-closed with a peer that never answers. `close_nowait`
            // leaves FinWait1 behind on purpose and nothing else frees it —
            // without this the slot is pinned for the rest of the boot.
            // 60 s = Linux's tcp_fin_timeout.
            if matches!(slot.state, State::FinWait1 | State::FinWait2 | State::LastAck)
                && now.saturating_sub(slot.last_send_tick) > FIN_TIMEOUT_TICKS
            {
                slot.state = State::Closed;
            }
        }
    }

    for t in &arp_probes {
        arp::request(*t);
    }
    for (p, payload) in &retrans {
        TCP_TX_SEGS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        send_segment_with_opts(p.dst_ip, p.src_port, p.dst_port,
            p.seq, p.ack, p.flags, p.window, payload, &p.opts[..p.opts_len]);
    }
    for p in &pending {
        send_pending(p);
    }
}

// === Internal ===

/// SYN options: MSS(4) + SACK-permitted(2) + NOP,NOP + Timestamp(kind=8,10) +
/// NOP + WScale(3) = 22, padded to 24. Returns the length written.
///
/// Shared by the first SYN and every retransmit. The retry used to send a
/// bare SYN: whenever the first one was lost — the normal case on a cold ARP
/// cache — the connection silently came up without window scaling, SACK or
/// timestamps, i.e. capped at a 64 KiB window for its whole life.
fn syn_opts(opts: &mut [u8; 40]) -> usize {
    opts[0] = 2;  // MSS option kind
    opts[1] = 4;  // MSS option length
    opts[2..4].copy_from_slice(&MSS.to_be_bytes());
    opts[4] = 4;            // SACK-permitted kind
    opts[5] = 2;            // length
    opts[6] = 1;            // NOP
    opts[7] = 1;            // NOP — align the 10-byte Timestamp to 4 bytes
    opts[8] = 8;            // Timestamp option kind
    opts[9] = 10;           // length
    let tsval = crate::interrupts::ticks() as u32;
    opts[10..14].copy_from_slice(&tsval.to_be_bytes()); // TSval
    // opts[14..18] TSecr = 0 on a SYN
    opts[18] = 1;           // NOP — align the 3-byte WScale to a 4-byte boundary
    opts[19] = 3;           // Window Scale option kind
    opts[20] = 3;           // length
    opts[21] = OUR_WSCALE;  // shift count
    24
}

fn send_syn(handle: usize) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;
    conn.last_send_tick = crate::interrupts::ticks();

    let mut opts = [0u8; 40];
    let len = syn_opts(&mut opts);
    send_segment_with_opts(
        conn.remote_ip, conn.local_port, conn.remote_port,
        conn.snd_iss, 0, SYN, INITIAL_WINDOW, &[], &opts[..len],
    );
    Ok(())
}

fn send_segment(
    dst_ip: [u8; 4], src_port: u16, dst_port: u16,
    seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8],
) {
    send_segment_with_opts(dst_ip, src_port, dst_port, seq, ack, flags, window, payload, &[]);
}

fn send_segment_with_opts(
    dst_ip: [u8; 4], src_port: u16, dst_port: u16,
    seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8], options: &[u8],
) {
    let opts_padded = (options.len() + 3) & !3; // pad to 4 bytes
    let header_len = HEADER_LEN + opts_padded;
    let total_len = header_len + payload.len();

    let mut pkt = alloc::vec![0u8; total_len];

    pkt[0..2].copy_from_slice(&src_port.to_be_bytes());
    pkt[2..4].copy_from_slice(&dst_port.to_be_bytes());
    pkt[4..8].copy_from_slice(&seq.to_be_bytes());
    pkt[8..12].copy_from_slice(&ack.to_be_bytes());
    pkt[12] = ((header_len / 4) as u8) << 4; // data offset
    pkt[13] = flags;
    pkt[14..16].copy_from_slice(&window.to_be_bytes());

    // Options
    if !options.is_empty() {
        pkt[HEADER_LEN..HEADER_LEN + options.len()].copy_from_slice(options);
    }

    // Payload
    pkt[header_len..].copy_from_slice(payload);

    // TCP checksum (pseudo-header + TCP segment)
    let src_ip = arp::our_ip();
    let checksum = tcp_checksum(&src_ip, &dst_ip, &pkt);
    pkt[16..18].copy_from_slice(&checksum.to_be_bytes());

    ipv4::send(dst_ip, ipv4::PROTO_TCP, &pkt);
}

fn tcp_checksum(src_ip: &[u8; 4], dst_ip: &[u8; 4], segment: &[u8]) -> u16 {
    let mut sum = 0u32;

    // Pseudo-header
    sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    sum += u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    sum += u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    sum += u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;
    sum += 6u32; // protocol TCP
    sum += segment.len() as u32;

    // TCP segment
    for i in (0..segment.len()).step_by(2) {
        let word = if i + 1 < segment.len() {
            u16::from_be_bytes([segment[i], segment[i + 1]])
        } else {
            (segment[i] as u16) << 8
        };
        sum += word as u32;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Scan a segment's TCP options for the Window Scale option (kind 3) and
/// return its shift count. `data_offset` is the TCP header length in bytes.
/// Did the peer's options carry SACK-permitted (kind 4, len 2)?
fn parse_sack_permitted(seg: &[u8], data_offset: usize) -> bool {
    let end = data_offset.min(seg.len());
    let mut i = HEADER_LEN;
    while i < end {
        match seg[i] {
            0 => break,
            1 => i += 1,
            kind => {
                if i + 1 >= end { break; }
                let len = seg[i + 1] as usize;
                if len < 2 { break; }
                if kind == 4 && len == 2 { return true; }
                i += len;
            }
        }
    }
    false
}

/// Build the TCP SACK option (kind 5) into `out` from the connection's
/// out-of-order reassembly map: up to 3 contiguous [left,right) runs as
/// absolute sequence numbers. Returns bytes written (0 if nothing to report).
/// First block = the highest run (most recently relevant), per RFC 2018.
fn build_sack_blocks(conn: &TcpConn, out: &mut [u8]) -> usize {
    if !conn.sack_ok || conn.ooo_runs.is_empty() { return 0; }
    // `ooo_runs` is already coalesced, so this is O(runs) — no per-ACK scan of
    // the whole segment map. Emit the highest up-to-3 runs, highest first
    // (RFC 2018 §4: the most recently received block goes first).
    out[0] = 5;                       // SACK option kind
    let mut p = 2;
    let mut take = 0usize;
    for (&s, &e) in conn.ooo_runs.iter().rev().take(3) {
        let l = conn.rcv_irs.wrapping_add(s);
        let r = conn.rcv_irs.wrapping_add(e);
        out[p..p + 4].copy_from_slice(&l.to_be_bytes()); p += 4;
        out[p..p + 4].copy_from_slice(&r.to_be_bytes()); p += 4;
        take += 1;
    }
    if take == 0 { return 0; }
    out[1] = (2 + 8 * take) as u8;    // length
    p
}

fn parse_wscale(seg: &[u8], data_offset: usize) -> Option<u8> {
    let end = data_offset.min(seg.len());
    let mut i = HEADER_LEN;
    while i < end {
        match seg[i] {
            0 => break,        // End of Option List
            1 => i += 1,       // NOP
            kind => {
                if i + 1 >= end { break; }
                let len = seg[i + 1] as usize;
                if len < 2 { break; } // malformed
                if kind == 3 && len == 3 && i + 2 < end {
                    return Some(seg[i + 2]);
                }
                i += len;
            }
        }
    }
    None
}

/// Scan a segment's options for the Timestamp option (kind 8, len 10) and
/// return the peer's TSval. `data_offset` is the TCP header length in bytes.
fn parse_ts(seg: &[u8], data_offset: usize) -> Option<u32> {
    let end = data_offset.min(seg.len());
    let mut i = HEADER_LEN;
    while i < end {
        match seg[i] {
            0 => break,        // End of Option List
            1 => i += 1,       // NOP
            kind => {
                if i + 1 >= end { break; }
                let len = seg[i + 1] as usize;
                if len < 2 { break; } // malformed
                if kind == 8 && len == 10 && i + 6 <= end {
                    return Some(u32::from_be_bytes(
                        [seg[i + 2], seg[i + 3], seg[i + 4], seg[i + 5]]));
                }
                i += len;
            }
        }
    }
    None
}

/// Scan for the Timestamp option (kind 8) and return TSecr — the peer's echo of
/// OUR most recent TSval. Since our TSval is `ticks()`, `ticks() - TSecr` is a
/// receiver-measured RTT (used for window auto-tuning).
fn parse_tsecr(seg: &[u8], data_offset: usize) -> Option<u32> {
    let end = data_offset.min(seg.len());
    let mut i = HEADER_LEN;
    while i < end {
        match seg[i] {
            0 => break,
            1 => i += 1,
            kind => {
                if i + 1 >= end { break; }
                let len = seg[i + 1] as usize;
                if len < 2 { break; }
                if kind == 8 && len == 10 && i + 10 <= end {
                    return Some(u32::from_be_bytes(
                        [seg[i + 6], seg[i + 7], seg[i + 8], seg[i + 9]]));
                }
                i += len;
            }
        }
    }
    None
}

/// Send a segment for a known connection, adding the Timestamp option (our
/// TSval + the peer's echoed TSval) when timestamps were negotiated (RFC 7323).
/// All connection-originated segments (ACKs, data, FIN) must carry it so the
/// sender gets a clean per-segment RTT sample despite our ACK jitter.
/// Build the TCP option list (Timestamp, then SACK blocks during a gap;
/// both 4-byte aligned via leading NOPs) for `conn`/`flags` into `opts`,
/// returning its length. Shared by the inline `send_seg` and the deferred
/// tick path, which materializes segments under the CONNECTIONS lock and
/// sends them after dropping it.
fn build_seg_opts(conn: &TcpConn, flags: u8, opts: &mut [u8; 40]) -> usize {
    let mut len = 0;
    if conn.ts_ok {
        opts[len] = 1; opts[len + 1] = 1;          // NOP, NOP
        opts[len + 2] = 8; opts[len + 3] = 10;     // Timestamp kind, len
        let tsval = crate::interrupts::ticks() as u32;
        opts[len + 4..len + 8].copy_from_slice(&tsval.to_be_bytes());
        opts[len + 8..len + 12].copy_from_slice(&conn.ts_recent.to_be_bytes());
        len += 12;
    }
    // SACK blocks: only on a pure ACK while we hold out-of-order data (a gap).
    // Never on a SYN — that advertises SACK-permitted instead.
    if conn.sack_ok && flags & SYN == 0 && !conn.ooo.is_empty() {
        let mut sack = [0u8; 26]; // 2 + 8*3
        let slen = build_sack_blocks(conn, &mut sack);
        if slen > 0 && len + 2 + slen <= opts.len() {
            opts[len] = 1; opts[len + 1] = 1;       // NOP, NOP align
            opts[len + 2..len + 2 + slen].copy_from_slice(&sack[..slen]);
            len += 2 + slen;
        }
    }
    len
}

fn send_seg(conn: &TcpConn, seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8]) {
    TCP_TX_SEGS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let mut opts = [0u8; 40];
    let len = build_seg_opts(conn, flags, &mut opts);
    if len > 0 {
        send_segment_with_opts(conn.remote_ip, conn.local_port, conn.remote_port,
            seq, ack, flags, window, payload, &opts[..len]);
    } else {
        send_segment(conn.remote_ip, conn.local_port, conn.remote_port,
            seq, ack, flags, window, payload);
    }
}

/// A fully-resolved zero-payload segment captured under the CONNECTIONS
/// lock so it can be sent (the NIC doorbell) AFTER the lock is dropped.
/// Keeps `tick_connections` from holding the lock across TX, which blocked
/// worker-core `recv` behind Core-0's periodic delayed-ACKs / SYN retries.
struct PendingSeg {
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    opts: [u8; 40],
    opts_len: usize,
}

fn send_pending(p: &PendingSeg) {
    TCP_TX_SEGS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if p.opts_len > 0 {
        send_segment_with_opts(p.dst_ip, p.src_port, p.dst_port,
            p.seq, p.ack, p.flags, p.window, &[], &p.opts[..p.opts_len]);
    } else {
        send_segment(p.dst_ip, p.src_port, p.dst_port,
            p.seq, p.ack, p.flags, p.window, &[]);
    }
}

fn ack_in_range(una: u32, ack: u32, nxt: u32) -> bool {
    // Check if ack is within (una, nxt] accounting for wrapping
    let diff_una = ack.wrapping_sub(una);
    let diff_nxt = nxt.wrapping_sub(una);
    diff_una > 0 && diff_una <= diff_nxt
}

fn close_cleanup(handle: usize) {
    CONNECTIONS.lock()[handle] = None;
}

pub fn list_connections() -> alloc::vec::Vec<(u16, [u8; 4], u16, &'static str)> {
    let conns = CONNECTIONS.lock();
    let mut result = alloc::vec::Vec::new();
    for slot in conns.iter().flatten() {
        let state_str = match slot.state {
            State::Closed => "CLOSED",
            State::Listen => "LISTEN",
            State::SynReceived => "SYN_RCVD",
            State::SynSent => "SYN_SENT",
            State::Established => "ESTABLISHED",
            State::FinWait1 => "FIN_WAIT_1",
            State::FinWait2 => "FIN_WAIT_2",
            State::CloseWait => "CLOSE_WAIT",
            State::LastAck => "LAST_ACK",
            State::TimeWait => "TIME_WAIT",
        };
        result.push((slot.local_port, slot.remote_ip, slot.remote_port, state_str));
    }
    result
}

#[derive(Debug)]
pub enum TcpError {
    TooManyConnections,
    ConnectionRefused,
    ConnectionFailed,
    NotConnected,
    Timeout,
    /// Too much already unacknowledged — retry the send later.
    WouldBlock,
}

impl core::fmt::Display for TcpError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            TcpError::WouldBlock => write!(f, "send buffer full"),
            TcpError::TooManyConnections => write!(f, "too many connections"),
            TcpError::ConnectionRefused => write!(f, "connection refused"),
            TcpError::ConnectionFailed => write!(f, "connection failed"),
            TcpError::NotConnected => write!(f, "not connected"),
            TcpError::Timeout => write!(f, "connection timed out"),
        }
    }
}
