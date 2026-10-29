//! TCP — Transmission Control Protocol
//!
//! nopeekOS-optimized defaults:
//! - No Nagle (low latency for request/response)
//! - 40ms delayed ACK (not 200ms)
//! - Initial window: 10 segments
//! - 3 retries, max 10s timeout (fast failure)
//! - Capability-gated: no cap = no connection
//!
//! The connection table is a [`Stack`]; everything it needs from the
//! machine comes through an [`Env`], and its counters live in a [`Stats`].

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// What the stack needs from the machine it runs on.
pub trait Env {
    /// The 100 Hz tick count.
    fn ticks(&self) -> u64;
    /// Microseconds since boot.
    fn uptime_us(&self) -> u64;
    /// Our IPv4 address.
    fn our_ip(&self) -> [u8; 4];
    fn random_u64(&mut self) -> u64;
    /// Free memory in MiB, for the receive budget.
    fn free_mib(&self) -> usize;
    /// The address whose link-layer address a packet to `ip` needs: the
    /// gateway off-link, `ip` itself on-link, broadcast for broadcast.
    fn arp_target_for(&self, ip: [u8; 4]) -> [u8; 4];
    /// Whether the link-layer address of `ip` is known.
    fn arp_known(&self, ip: [u8; 4]) -> bool;
    fn arp_request(&mut self, ip: [u8; 4]);
    /// Hand a TCP segment to IP, with DF set and identification `id`.
    /// False when the driver queue refused it.
    fn ip_send(&mut self, dst: [u8; 4], segment: &[u8], id: u16) -> bool;
    /// The path MTU to `dst`: the device's, or less once a router said so.
    fn mtu(&self, dst: [u8; 4]) -> u32;
    /// A router said the path to `dst` carries only `mtu` (the route's
    /// `__ip_rt_update_pmtu`).
    fn update_pmtu(&mut self, dst: [u8; 4], mtu: u32);
}

/// The stack's counters. Atomic, so diagnostics and `send_blocking` read
/// them without the table's lock.
pub struct Stats {
    /// What `recv_window` last computed: (advertised window in bytes, srtt
    /// in milliseconds, cap in bytes). On a slow link the question is how
    /// much we offered and where the cap came from, not how much arrived.
    pub wnd_last: AtomicU32,
    pub wnd_srtt: AtomicU32,
    pub wnd_cap: AtomicU32,
    /// Manual cap on the advertised window in bytes, 0 = off.
    ///
    /// A diagnostic tool, not a setting. On a saturated path RTT = window /
    /// rate, so one measurement cannot tell whether the air or we are the
    /// limit; the shape of throughput over several windows can: if it rises
    /// with the window, we were the limit; if only the RTT grows, the air is.
    pub rcv_wnd_force: AtomicU32,
    /// Buffer bytes all connections hold together (received in order and
    /// in reassembly, sent and not yet acknowledged), and `tcp_mem[2]`, the
    /// ceiling on it. The per-connection caps allow over a gigabyte across
    /// the table; a peer that ignores flow control, or a module that opens
    /// many connections and never reads, would otherwise fill the kernel
    /// heap. Raised on every charge, recomputed exactly on each tick, so it
    /// cannot drift.
    pub mem_held: AtomicUsize,
    pub mem_limit: AtomicUsize,
    /// Segments dropped because `tcp_mem` was full (diagnostic).
    pub mem_drops: AtomicU32,
    /// Out-of-order receive counters (diagnostic). `ooo_ahead` = a segment
    /// past rcv_nxt (a gap → the sender will have to retransmit);
    /// `ooo_behind` = a duplicate at/below rcv_nxt (a retransmit we already
    /// have). A burst of ahead during a download = packet loss + go-back-N.
    pub ooo_ahead: AtomicU32,
    pub ooo_behind: AtomicU32,
    /// Max recv_buf depth seen since last read (diagnostic). High (→
    /// RECV_BUF_SIZE) = our consumer/core can't drain fast enough → window
    /// closes → sender stalls (consumer-limited). Low = buffer drains fine →
    /// a tail slowdown is the sender throttling, not us.
    pub max_rxbuf: AtomicUsize,
    /// Segments we transmitted (mostly ACKs) — diagnostic. A bulk download
    /// flooding one ACK per packet shows up here as ~tens of thousands/s.
    pub tx_segs: AtomicU32,
    /// Counts every ACK that freed send-buffer space.
    ///
    /// Lets `send_blocking` wait without spinning on the table's lock, the
    /// lock the receive path needs to process the very ACK being waited
    /// for. The lock-free analogue of Linux `sk_stream_wait_memory` /
    /// `sk_write_space`: nothing new is worth trying while it stands still.
    pub ack_gen: AtomicU64,
    // ── Where send time goes ─────────────────────────────────────────
    //
    // Producer-limited or window-limited: `send_wouldblock` tells them
    // apart. If it stays zero we never waited for ACKs and the limit is time
    // spent in `send`; if it is high, the send buffer limit is the cap.
    pub send_tsc: AtomicU64,
    pub send_blocked_tsc: AtomicU64,
    pub send_segs: AtomicU64,
    pub send_wouldblock: AtomicU64,
    pub send_maxbuf: AtomicU64,
    /// How often the driver queue refused a segment. This is backpressure,
    /// not loss; the count says how often it applies.
    pub send_refused: AtomicU64,
    /// Fast retransmits and RTO expiries. Without these, a too-small window
    /// cannot be told apart from a lost segment.
    pub fast_retrans: AtomicU64,
    pub rto_fired: AtomicU64,
    /// Cumulative duplicate ACKs, not the connection's live counter, which
    /// is reset by every ACK that clears data.
    pub dupacks_seen: AtomicU64,
}

impl Stats {
    pub const fn new() -> Self {
        Stats {
            wnd_last: AtomicU32::new(0), wnd_srtt: AtomicU32::new(0), wnd_cap: AtomicU32::new(0),
            rcv_wnd_force: AtomicU32::new(0),
            mem_held: AtomicUsize::new(0), mem_limit: AtomicUsize::new(0),
            mem_drops: AtomicU32::new(0),
            ooo_ahead: AtomicU32::new(0), ooo_behind: AtomicU32::new(0),
            max_rxbuf: AtomicUsize::new(0), tx_segs: AtomicU32::new(0),
            ack_gen: AtomicU64::new(0),
            send_tsc: AtomicU64::new(0), send_blocked_tsc: AtomicU64::new(0),
            send_segs: AtomicU64::new(0), send_wouldblock: AtomicU64::new(0),
            send_maxbuf: AtomicU64::new(0), send_refused: AtomicU64::new(0),
            fast_retrans: AtomicU64::new(0), rto_fired: AtomicU64::new(0),
            dupacks_seen: AtomicU64::new(0),
        }
    }
}

/// The machine and the counters, for the duration of one call into the
/// stack.
struct Io<'a, E: Env + ?Sized> {
    env: &'a mut E,
    st: &'static Stats,
}

impl<E: Env + ?Sized> Io<'_, E> {
    fn ticks(&self) -> u64 { self.env.ticks() }

    /// Our clock for the TCP timestamp option, in milliseconds.
    ///
    /// A 100 Hz tick would measure RTTs in 10 ms steps. Linux uses
    /// `TCP_TS_HZ = 1000` (tcp.h); RFC 7323 §4 allows 1 ms to 1 s, and a
    /// faster clock would break PAWS (2^32 ms is 49 days, 2^32 us only 71
    /// minutes).
    fn ts_now_ms(&self) -> u32 {
        (self.env.uptime_us() / 1000) as u32
    }
}

// RFC 6528 — Initial Sequence Number generation. Predictable ISNs (e.g. a
// raw tick counter) let an off-path attacker forge in-window segments on a
// listening socket. We mix a per-boot CSPRNG secret (`Stack::new`) with the
// connection 4-tuple via BLAKE3-keyed-hash, then add a tick-derived
// monotonic counter so retried connections still grow forward.
fn generate_isn(secret: &[u8; 32], ticks: u64, saddr: [u8; 4], daddr: [u8; 4], sport: u16, dport: u16) -> u32 {
    let mut buf = [0u8; 12];
    buf[0..4].copy_from_slice(&saddr);
    buf[4..8].copy_from_slice(&daddr);
    buf[8..10].copy_from_slice(&sport.to_be_bytes());
    buf[10..12].copy_from_slice(&dport.to_be_bytes());
    let h = blake3::keyed_hash(secret, &buf);
    let b = h.as_bytes();
    let hash_part = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    // Monotonic component: 100 Hz tick × 2500 ≈ 4 µs ISN step (RFC 6528 §3
    // suggests a ~250 kHz clock). Wrap is fine — the secret-keyed hash
    // ensures the absolute value is unguessable per 4-tuple.
    let timer = (ticks as u32).wrapping_mul(2500);
    hash_part.wrapping_add(timer)
}

/// `TCP_MSS_DEFAULT`: the send MSS when the peer names none (RFC 9293
/// §3.7.1 MUST-15).
const TCP_MSS_DEFAULT: u16 = 536;
/// `TCP_MIN_SND_MSS` (`sysctl_tcp_min_snd_mss`).
const TCP_MIN_SND_MSS: u16 = 48;
/// `TCP_MSS_DESIRED`: above it the MSS is bounded by half the largest window.
const TCP_MSS_DESIRED: u32 = 1220;
/// IPv4 and TCP header without options.
const HEADERS: u32 = 40;
// The SYN/SYN-ACK window is never scaled (RFC 7323), so it's capped at 16-bit.
const INITIAL_WINDOW: u16 = 65535;
// TCP Window Scaling (RFC 7323). Without it the window is capped at 64 KiB and
// throughput at 64 KiB / RTT. We advertise `free >> OUR_WSCALE`.
// WSCALE 8 so the 16-bit window field can express the full 8 MiB buffer
// (8 MiB >> 8 = 32768 ≤ 65535), which leaves headroom over the
// bandwidth-delay product so RTT jitter does not underfill the pipe.
const OUR_WSCALE: u8 = 8;

const RCV_WND_MIN: usize = 256 * 1024;
const RCV_WND_MAX: usize = RECV_BUF_SIZE;

// The receive window is sized by DRS (what the application actually reads
// per RTT), not by a link rate the driver claims about itself.

/// tcp_input.c:812 `tcp_rcv_rtt_update`.
///
/// The minimum counts, not the mean: a lower sample applies at once, a
/// higher one is smoothed, and only while the receive queue is empty.
/// Otherwise the sample measures how fast our application reads, not the
/// path.
fn rcv_rtt_update(conn: &mut TcpConn, sample_us: u32) {
    let m = sample_us.saturating_mul(8);
    let old = conn.rcv_rtt_us8;
    if old == 0 || m < old {
        conn.rcv_rtt_us8 = m;
        return;
    }
    if !conn.recv_buf.is_empty() {
        return;
    }
    conn.rcv_rtt_us8 = old - (old >> 3) + sample_us;
}

/// tcp_input.c:894 `tcp_rcvbuf_grow`.
///
/// `tcp_space_from_win`/`tcp_win_from_space` are omitted: in Linux they
/// account for `skb->len / skb->truesize` overhead, and our `recv_buf`
/// holds raw bytes, so the conversion is the identity.
fn rcvbuf_grow(conn: &mut TcpConn, newval: u32) {
    let oldval = conn.rcvq_space.max(1);
    conn.rcvq_space = newval;

    // „DRS is always one RTT late."
    let mut rcvwin = (newval as u64) << 1;
    // „slow start: allow the sender to double its rate."
    let grow = rcvwin * (newval.saturating_sub(oldval)) as u64 / oldval as u64;
    rcvwin += grow << 1;
    // Out-of-order data needs room on top.
    rcvwin += conn.ooo.values().map(|v| v.len() as u64).sum::<u64>();

    let rcvbuf = rcvwin.min(RCV_WND_MAX as u64) as u32;
    // Only grow. tcp_input.c:922.
    if rcvbuf > conn.drs_win {
        conn.drs_win = rcvbuf;
    }
}

/// tcp_input.c:933 `tcp_rcv_space_adjust` — called whenever the application
/// has read.
fn rcv_space_adjust(conn: &mut TcpConn, now: u64) {
    let time = now.saturating_sub(conn.rcvq_time_us);
    // Less than one RTT says nothing.
    if conn.rcv_rtt_us8 == 0 || time < (conn.rcv_rtt_us8 >> 3) as u64 {
        return;
    }
    let copied = conn.copied_total.saturating_sub(conn.rcvq_copied0);
    // Subtract what is still queued: if it backs up, the application is the
    // bottleneck and a larger window does not help (tcp_input.c:948-949).
    let copied = copied.saturating_sub(conn.recv_buf.len() as u64);
    if copied > conn.rcvq_space as u64 {
        rcvbuf_grow(conn, copied.min(u32::MAX as u64) as u32);
    }
    conn.rcvq_copied0 = conn.copied_total;
    conn.rcvq_time_us = now;
}

/// tcp_input.c:587 `tcp_sndbuf_expand` — the counterpart of `rcvbuf_grow`.
///
/// Linux derives the send buffer from the congestion window:
/// `2 * max(TCP_INIT_CWND, snd_cwnd, reordering+1) * per_mss`, capped by
/// `tcp_wmem[2]`. The factor 2 covers CUBIC's 1.7 plus slack for an
/// application slow to react to `EPOLLOUT`.
///
/// Without congestion control there is no `snd_cwnd`; instead we use the
/// same quantity measured: how much was acknowledged in one RTT. The
/// factor 2 stays. `tcp_should_expand_sndbuf` (tcp_input.c:5804) does not
/// translate: its gate is a filled congestion window, and here the buffer
/// is the only brake.
fn sndbuf_grow(conn: &mut TcpConn, newval: usize) {
    conn.snd_space = newval;
    let want = newval.saturating_mul(2).clamp(SND_BUF_INIT, SND_BUF_MAX);
    // Only grow — the same rule as tcp_input.c:922.
    if want > conn.snd_buf_limit {
        conn.snd_buf_limit = want;
    }
}

/// The mirror of `rcv_space_adjust`: called whenever an ACK has cleared
/// data.
fn snd_space_adjust(conn: &mut TcpConn, now: u64) {
    let time = now.saturating_sub(conn.sndq_time_us);
    let srtt_us = (conn.srtt_ms as u64).saturating_mul(1000);
    // Less than one RTT says nothing — the same bound as on the receive side.
    if srtt_us == 0 || time < srtt_us {
        return;
    }
    let acked = conn.acked_total.saturating_sub(conn.sndq_acked0);
    if acked > conn.snd_space as u64 {
        sndbuf_grow(conn, acked.min(SND_BUF_MAX as u64) as usize);
    }
    conn.sndq_acked0 = conn.acked_total;
    conn.sndq_time_us = now;
}

/// How much may be in flight: bounded by our buffer and by the peer's
/// window.
fn snd_allowed(conn: &TcpConn) -> usize {
    let peer = if conn.snd_wnd == 0 {
        // Unknown before the first ACK. A zero window after the handshake
        // is real and correctly stops us.
        SND_BUF_INIT
    } else {
        conn.snd_wnd as usize
    };
    conn.snd_buf_limit.min(peer).max(eff_mss(conn))
}

fn recv_window(conn: &TcpConn, st: &Stats) -> u16 {
    let forced = st.rcv_wnd_force.load(Ordering::Relaxed);
    // Without a manual cap DRS decides; with one, `net window` measures what
    // DRS should have found.
    let cap = if forced > 0 {
        (forced as usize).min(RCV_WND_MAX)
    } else {
        (conn.drs_win as usize).clamp(RCV_WND_MIN, RCV_WND_MAX)
    };
    let free = RECV_BUF_SIZE.saturating_sub(conn.recv_buf.len()).min(cap);
    st.wnd_last.store(free as u32, Ordering::Relaxed);
    st.wnd_srtt.store(conn.srtt_ms, Ordering::Relaxed);
    st.wnd_cap.store(cap as u32, Ordering::Relaxed);
    if conn.wscale_ok {
        (free >> OUR_WSCALE).min(65535) as u16
    } else {
        free.min(65535) as u16
    }
}

/// Merge [s,e) into the coalesced out-of-order run set (stream offsets).
fn ooo_runs_add(runs: &mut BTreeMap<u64, u64>, s: u64, e: u64) {
    let mut s = s;
    let mut e = e;
    // Absorb a contiguous/overlapping left neighbour (greatest start < s).
    if let Some((&ls, &le)) = runs.range(..s).next_back() {
        if le >= s { s = ls; }
    }
    // Absorb every run starting within [s, e] (overlap or adjacency).
    let keys: alloc::vec::Vec<u64> = runs.range(s..=e).map(|(&k, _)| k).collect();
    for k in keys {
        if runs[&k] > e { e = runs[&k]; }
        runs.remove(&k);
    }
    runs.insert(s, e);
}

/// Drop/trim runs now delivered (everything below offset `want`).
fn ooo_runs_trim(runs: &mut BTreeMap<u64, u64>, want: u64) {
    let keys: alloc::vec::Vec<u64> =
        runs.range(..want).filter(|&(_, &e)| e <= want).map(|(&k, _)| k).collect();
    for k in keys { runs.remove(&k); }
    if let Some((&ks, &ke)) = runs.range(..want).next_back() {
        if ke > want { runs.remove(&ks); runs.insert(want, ke); }
    }
}
const MAX_RETRIES: u8 = 3;
const RETRY_TICKS_BASE: u64 = 100; // 1 second (100Hz)
// Cold-cache ARP while a SYN waits: one WiFi round trip between probes, and a
// total budget of ~500 ms before the SYN goes out to broadcast regardless.
const ARP_RETRANS_TICKS: u64 = 5; // 50 ms
const ARP_MAX_TRIES: u8 = 10;
const FIN_TIMEOUT_TICKS: u64 = 6000; // 60 s, like Linux's tcp_fin_timeout
// Retransmit timeout for data. Base 200 ms, doubled per attempt (RFC 6298
// style), give up after MAX_DATA_RETRIES, then the connection is honestly
// dead instead of silently one-way.
const RTO_TICKS_BASE: u64 = 20; // 200 ms = Linux TCP_RTO_MIN (HZ/5)
/// Retransmissions before an established connection is declared dead.
/// Same as Linux `TCP_RETR2` (include/net/tcp.h:119). With the shift capped
/// at 5 the RTO tops out at 6.4 s, so 15 attempts span roughly 70 s; a
/// saturated WiFi link can stall for several seconds without being dead.
const MAX_DATA_RETRIES: u8 = 15;
// Ceiling on unacknowledged bytes held for retransmit. A peer that stops
// acknowledging must not grow this without bound; `send` refuses past it,
// which is the backpressure the caller needs to see.
/// Initial send buffer, before any measurement.
///
/// `tcp::send` is all-or-nothing (unlike Linux `tcp_sendmsg`, which takes
/// what fits and reports a short write), and callers pass 64 KiB chunks.
/// A starting value below the caller's chunk size is a deadlock, not a slow
/// start: growth depends on ACKs for data that could never be sent.
const SND_BUF_INIT: usize = 256 * 1024;
/// Send buffer ceiling.
///
/// Linux uses `sysctl_tcp_wmem[2]` = 4 MB, which it can afford because of
/// SACK, PRR and Limited Transmit. Without them, every loss overflowing
/// intermediate buffers is an RTO, so a buffer far above the
/// bandwidth-delay product collapses throughput. Raise this once SACK
/// exists.
const SND_BUF_MAX: usize = 256 * 1024;

/// Congestion control is off: it needs loss accounting we do not have.
///
/// Linux computes `tcp_packets_in_flight = packets_out - sacked_out -
/// lost_out + retrans_out` and lowers `sacked_out` on every duplicate ACK
/// (`tcp_add_reno_sack`), even without SACK. Here "in flight" is simply
/// `snd_nxt - snd_una`, which never shrinks on loss: after one lost
/// segment `cwnd` drops and `write_xmit` would never send again, leaving
/// one retransmit per RTO. Without loss accounting a congestion window is
/// worse than none.
///
/// Fast retransmit stays on (three duplicate ACKs resend at once); only
/// window reduction is off. Enable only once these exist, in order:
/// `packets_out`/`sacked_out`/`lost_out` as real counters,
/// `tcp_add_reno_sack`, sender-side SACK, then PRR.
const CONG_CONTROL: bool = false;

/// `TCP_INIT_CWND` — ten segments (RFC 6928).
const TCP_INIT_CWND: u32 = 10;
/// Default `tp->reordering`: three duplicate ACKs trigger fast retransmit
/// (RFC 5681 §3.2).
const DUPACK_THRESH: u32 = 3;

// 8 MiB receive buffer → 8 MiB window with scaling → fills the bandwidth-delay
// product for ~gigabit even at tens-of-ms RTT. Grown lazily (VecDeque::new),
// so an idle connection costs nothing and only an actively bursting one
// approaches the limit.
// Host TCP only ever has a handful of live connections (OTA/https/dns), so the
// worst-case footprint is small; the guest browser uses its own (microvm) TCP.
const RECV_BUF_SIZE: usize = 8 * 1024 * 1024;
const DELAYED_ACK_TICKS: u64 = 4; // 40ms at 100Hz
// ACK coalescing: send one ACK per N in-order segments (a held ACK is still
// flushed by the 40 ms timer). 8 ≈ one ACK per ~11.7 KB at 1460 MSS, which
// cuts the ACK packet rate and the work on the busy-spin RX core. Safe because
// timestamps give the sender a per-segment RTT regardless.
const ACK_COALESCE: u16 = 8;
// Cap on buffered out-of-order data per connection. Beyond this, new
// ahead-segments are dropped (the sender will retransmit) so a lossy link
// can't blow up the heap.
const OOO_MAX_BYTES: usize = 2 * 1024 * 1024;

/// `sysctl_tcp_mem[2]` in bytes, from the memory free when first asked:
/// tcp.c `tcp_init_mem`, `limit = nr_free_buffer_pages() / 16` (at least
/// 128 pages), `tcp_mem[0] = limit / 4 * 3`, `tcp_mem[2] = tcp_mem[0] * 2`.
/// Above it Linux refuses to charge more buffer memory to any socket.
fn tcp_mem_limit<E: Env + ?Sized>(io: &Io<E>) -> usize {
    let b = io.st.mem_limit.load(Ordering::Relaxed);
    if b != 0 { return b; }
    let free_pages = io.env.free_mib() * 256;
    let limit = (free_pages / 16).max(128);
    let b = limit / 4 * 3 * 2 * 4096;
    io.st.mem_limit.store(b, Ordering::Relaxed);
    b
}

/// Charge `n` buffer bytes, if `tcp_mem` allows (`__sk_mem_raise_allocated`
/// past the hard limit). A refused segment is dropped like one that does
/// not fit the buffer; the peer sends it again.
fn mem_charge<E: Env + ?Sized>(io: &Io<E>, n: usize) -> bool {
    if n == 0 { return true; }
    if io.st.mem_held.load(Ordering::Relaxed) + n > tcp_mem_limit(io) {
        io.st.mem_drops.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    io.st.mem_held.fetch_add(n, Ordering::Relaxed);
    true
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
    Closing,
    LastAck,
    TimeWait,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Closed => "CLOSED",
            State::Listen => "LISTEN",
            State::SynReceived => "SYN_RCVD",
            State::SynSent => "SYN_SENT",
            State::Established => "ESTABLISHED",
            State::FinWait1 => "FIN_WAIT_1",
            State::FinWait2 => "FIN_WAIT_2",
            State::CloseWait => "CLOSE_WAIT",
            State::Closing => "CLOSING",
            State::LastAck => "LAST_ACK",
            State::TimeWait => "TIME_WAIT",
        }
    }
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
    /// Stream offset of `rcv_nxt`: data bytes delivered so far. 64 bits, so
    /// reassembly offsets never wrap however long the connection lives.
    rcv_off: u64,

    // Buffers
    recv_buf: VecDeque<u8>,
    send_buf: Vec<u8>,
    // Out-of-order reassembly: segments received ahead of a gap, keyed by
    // stream offset (see `rcv_off`). Without it a single lost packet forces
    // the sender into go-back-N (retransmit the whole window), and the re-burst
    // can overflow small NIC FIFOs again. Bounded by OOO_MAX_BYTES (else
    // dropped → the sender retransmits).
    ooo: BTreeMap<u64, Vec<u8>>,
    ooo_bytes: usize,
    // Coalesced [start,end) runs of `ooo`, kept in sync — so building SACK
    // blocks is O(runs), not a scan over thousands of `ooo` entries per ACK
    // with a large window. Advisory: a desync only makes SACK suboptimal,
    // never corrupts data (the bytes still come from `ooo`).
    ooo_runs: BTreeMap<u64, u64>,
    // Smoothed RTT in milliseconds, from the peer's echoed TSecr. Kept as a
    // diagnostic only; the advertised window comes from DRS below.
    srtt_ms: u32,

    // ── DRS: Dynamic Right Sizing (Linux `tcp_rcv_space_adjust`) ────────
    //
    // The receiver measures how much the application actually reads per RTT
    // and derives the window from that; no link rate, no constant. Three
    // rules from tcp_input.c:
    //
    // * It only grows (`if (rcvbuf > sk->sk_rcvbuf)`, tcp_input.c:922, and
    //   `if (copied <= space) goto new_measure`, :950). A window that may
    //   shrink spirals down: less window, less throughput, less measured need.
    // * The RTT comes from the minimum, not the smoothed value
    //   (`if (old_sample == 0 || m < old_sample)`, :817). The smoothed value
    //   includes our own queueing, a positive feedback loop.
    // * No RTT sample while the receive queue is non-empty
    //   (`if (tp->rcv_nxt != tp->copied_seq) return`, :833), or it measures
    //   our application instead of the path.

    /// `rcv_rtt_est.rtt_us` — in eighths of a microsecond, like Linux
    /// (`long m = sample << 3`).
    rcv_rtt_us8: u32,
    /// `rcvq_space.seq` — total bytes the application had read at the last
    /// measurement point. Counted absolutely instead of in sequence numbers;
    /// the delta is the same.
    rcvq_copied0: u64,
    /// Total bytes the application has read (`copied_seq`).
    copied_total: u64,
    /// `rcvq_space.time`
    rcvq_time_us: u64,
    /// `rcvq_space.space` — the measured need of one RTT.
    rcvq_space: u32,
    /// `sk_rcvbuf` — the window DRS allows.
    drs_win: u32,

    // Retransmit. `send_buf` holds every byte we sent and the peer has not
    // acknowledged, starting at `snd_una`; `rto_tick` is when the oldest of
    // them went out. Without retransmit a single lost segment leaves a hole
    // the peer can never fill, so it delivers nothing more to its application
    // while our sends all report success.
    retries: u8,
    last_send_tick: u64,
    rto_tick: u64,

    // Delayed ACK
    /// A one-shot D-SACK block (RFC 2883): the range of a segment we already
    /// had. Emitted as the first SACK block of the next ACK, then cleared.
    dsack: Option<(u32, u32)>,
    ack_pending: bool,
    ack_tick: u64,
    // In-order segments received since our last ACK (ACK-coalescing counter).
    acks_held: u16,
    // Bytes drained since our last recv()-side window-update ACK. Rate-limits
    // those ACKs so a bulk download doesn't emit one per recv() call (~70k/s).
    freed_since_winupd: u32,

    /// The owner closed: a FIN follows the last byte of `send_buf`
    /// (`tcp_send_fin`), and `fin_sent` once it went out at that position.
    fin_queued: bool,
    fin_sent: bool,

    // Connection complete flag
    established: bool,
    closed: bool,
    error: bool,

    // Window scaling (RFC 7323). `wscale_ok` once both SYNs carried the
    // option; `snd_wscale` is the peer's shift (to scale their advertised
    // window). Our own advertised window is scaled by OUR_WSCALE.
    wscale_ok: bool,
    snd_wscale: u8,
    /// The peer's window, scaled (RFC 9293 §3.8.6).
    snd_wnd: u32,
    /// Our send buffer, which grows — the counterpart of `drs_win` on the
    /// receive side. Linux derives it from the congestion window in
    /// `tcp_sndbuf_expand` (tcp_input.c:587); without `snd_cwnd` we grow it
    /// like `tcp_rcv_space_adjust` does: if the last RTT filled the buffer
    /// on a clean path, it is too small. See `sndbuf_grow`.
    snd_buf_limit: usize,
    /// Mirror of `rcvq_space.seq`: total bytes the peer has acknowledged.
    acked_total: u64,
    /// Value at the last measurement point.
    sndq_acked0: u64,
    /// Time of the last measurement point.
    sndq_time_us: u64,
    /// The measured need of one RTT — mirror of `rcvq_space`.
    snd_space: usize,

    // ── Congestion control, RFC 5681 / RFC 6582 (New Reno) ───────────
    //
    // Limits how much of the send buffer `write_xmit` pushes at once; see
    // `CONG_CONTROL` for why window reduction is currently off.
    /// `tcp_snd_cwnd` — in packets, like Linux.
    snd_cwnd: u32,
    /// `snd_ssthresh`. Initially infinite: the first loss sets it.
    snd_ssthresh: u32,
    /// `snd_cwnd_cnt` — the fractional packets of `tcp_cong_avoid_ai`.
    snd_cwnd_cnt: u32,
    /// Consecutive duplicate ACKs.
    dupacks: u32,
    /// Whether we are in fast recovery (RFC 6582 "recover").
    in_recovery: bool,
    /// `snd_nxt` on entry; recovery ends only beyond it (RFC 6582 §3.2,
    /// otherwise one loss event halves the window several times).
    recovery_end: u32,

    // TCP Timestamps (RFC 7323). `ts_ok` once both SYNs carried the option;
    // `ts_recent` = the peer's most recent in-order TSval, echoed as our TSecr
    // so the sender measures RTT per-segment (robust to our ACK jitter) →
    // accurate RTO → no spurious retransmits.
    ts_ok: bool,
    ts_recent: u32,

    // Selective ACK (RFC 2018). `sack_ok` once both SYNs carried SACK-permitted.
    // As the receiver we then tell the sender which out-of-order ranges we
    // already hold (straight from `ooo`), so it retransmits only the real holes
    // instead of everything past the cumulative ACK.
    sack_ok: bool,

    // Next-hop MAC not yet known: the SYN is held back until ARP answers.
    // Sending it to L2 broadcast instead is what most gateways drop, and the
    // recovery is then a full 1 s SYN retry.
    arp_pending: bool,
    arp_tries: u8,
    /// The owner has let go (`close`). Until then a Closed slot still
    /// belongs to whoever holds the handle and is not reused under it.
    released: bool,
    /// Who holds the handle: `KERNEL`, or a module's pid.
    owner: Owner,

    // ── MSS (RFC 9293 §3.7.1, Linux `tcp_sync_mss`) ─────────────────────
    /// `rx_opt.mss_clamp`: the MSS the peer announced, or 536.
    mss_clamp: u16,
    /// `mss_cache`: payload per segment, the options every segment carries
    /// taken off.
    mss_cache: u16,
    /// `advmss`: what we announce, from the device MTU.
    advmss: u16,
    /// `icsk_pmtu_cookie`: the path MTU `mss_cache` was computed for.
    pmtu: u32,
    /// `max_window`: the largest window the peer offered.
    max_window: u32,

    /// `inet_id`: the IP identification of the next segment (DF is set, so
    /// it only has to be unique enough for duplicate detection).
    inet_id: core::cell::Cell<u16>,
    /// What an ICMP error said: fatal in SYN-SENT, otherwise `sk_err_soft`.
    sock_err: Option<SockErr>,
}

/// The error a connection ended with, or last learned of (`sk_err`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SockErr {
    /// ECONNREFUSED: port unreachable.
    Refused,
    /// EHOSTUNREACH and its kin.
    HostUnreachable,
    /// ENETUNREACH.
    NetUnreachable,
    /// ENOPROTOOPT, EPROTO: the peer does not speak TCP, or a parameter
    /// problem.
    Protocol,
    /// EMSGSIZE: the path carries less than the smallest MSS we send.
    MsgSize,
}

/// Who holds a connection: the kernel itself, or a module by its pid.
pub type Owner = u32;
pub const KERNEL: Owner = 0;

/// Connections one module may hold at once (Linux bounds a process's
/// sockets by `RLIMIT_NOFILE`).
pub const PER_OWNER_MAX: usize = 32;

impl TcpConn {
    /// A connection in `state` with nothing negotiated, nothing sent and
    /// nothing received yet.
    fn blank(state: State, local_port: u16, owner: Owner) -> Self {
        TcpConn {
            state,
            local_port,
            remote_ip: [0; 4],
            remote_port: 0,
            snd_nxt: 0,
            snd_una: 0,
            snd_iss: 0,
            rcv_nxt: 0,
            rcv_off: 0,
            recv_buf: VecDeque::new(),
            ooo: BTreeMap::new(),
            ooo_bytes: 0,
            ooo_runs: BTreeMap::new(),
            srtt_ms: 0,
            rcv_rtt_us8: 0,
            rcvq_copied0: 0,
            copied_total: 0,
            rcvq_time_us: 0,
            // `tcp_init_buffer_space`: start from what a fresh connection offers
            // anyway, `TCP_INIT_CWND * advmss`. Without a start value
            // `rcvbuf_grow` divides by zero.
            rcvq_space: 10 * 1460,
            drs_win: RCV_WND_MIN as u32,
            send_buf: Vec::new(),
            retries: 0,
            last_send_tick: 0,
            rto_tick: 0,
            dsack: None,
            ack_pending: false,
            ack_tick: 0,
            acks_held: 0,
            freed_since_winupd: 0,
            fin_queued: false,
            fin_sent: false,
            established: false,
            closed: false,
            error: false,
            wscale_ok: false,
            snd_wscale: 0,
            snd_wnd: 0,
            snd_buf_limit: SND_BUF_INIT,
            acked_total: 0,
            sndq_acked0: 0,
            sndq_time_us: 0,
            snd_space: 0,
            snd_cwnd: TCP_INIT_CWND,
            snd_ssthresh: u32::MAX,
            snd_cwnd_cnt: 0,
            dupacks: 0,
            in_recovery: false,
            recovery_end: 0,
            ts_ok: false,
            ts_recent: 0,
            sack_ok: false,
            arp_pending: false,
            arp_tries: 0,
            released: false,
            owner,
            mss_clamp: TCP_MSS_DEFAULT,
            mss_cache: TCP_MSS_DEFAULT,
            advmss: 1460,
            pmtu: 1500,
            max_window: 0,
            inet_id: core::cell::Cell::new(0),
            sock_err: None,
        }
    }

    /// A state in which we still send data or our FIN.
    fn sends(&self) -> bool {
        matches!(self.state, State::Established | State::CloseWait | State::FinWait1 | State::Closing | State::LastAck)
    }

    /// `ip_select_ident_segs` for a connected socket.
    fn next_id(&self) -> u16 {
        let id = self.inet_id.get();
        self.inet_id.set(id.wrapping_add(1));
        id
    }

    /// `ipv4_default_advmss` and `tcp_mtup_init`: what we announce and the
    /// path MTU we start from.
    fn set_path<E: Env + ?Sized>(&mut self, env: &E) {
        self.pmtu = env.mtu(self.remote_ip);
        self.advmss = (self.pmtu.saturating_sub(HEADERS).max(256)) as u16;
        self.rcvq_space = 10 * self.advmss as u32;
    }

    /// `tcp_sync_mss`: `tcp_mtu_to_mss` for the path MTU, bounded by half
    /// the largest window (`tcp_bound_to_half_wnd`).
    fn sync_mss(&mut self) {
        let opts = data_opts_len(self) as i32;
        let mut mss = self.pmtu as i32 - HEADERS as i32;
        mss = mss.min(self.mss_clamp as i32).max(TCP_MIN_SND_MSS as i32);
        mss -= opts;
        let cutoff = if self.max_window > TCP_MSS_DESIRED { self.max_window >> 1 } else { self.max_window };
        if cutoff != 0 && mss as u32 > cutoff {
            mss = (cutoff as i32).max(68 - (20 + opts));
        }
        self.mss_cache = mss.max(1) as u16;
    }
}

/// How far a connection got, for a caller that gave up on it.
#[derive(Debug, Clone, Copy)]
pub struct ConnDebug {
    pub state: &'static str,
    pub arp_pending: bool,
    pub arp_tries: u8,
    pub retries: u8,
}

/// A connection's place in the table: `(generation << 32) | index`. The
/// generation moves on when the place is freed, so a handle kept past its
/// connection never reaches the next one in the same place.
pub type Handle = usize;

const _: () = assert!(usize::BITS == 64);

/// What a connection is found by: our port, the peer's address and port.
type Tuple = (u16, [u8; 4], u16);

struct Entry {
    generation: u32,
    conn: Option<Box<TcpConn>>,
}

/// The connections, as Linux keeps sockets: objects found by their 4-tuple
/// (`tcp_hashinfo.ehash`) or, while listening, by their port (`lhash2`).
/// It grows with the connections; what bounds it is memory (`tcp_mem`),
/// the orphan and TIME-WAIT limits, and per module `PER_OWNER_MAX`.
/// Ordered maps rather than hash tables: the peer chooses half of every
/// key, and a tree has no bucket to flood.
struct Table {
    entries: Vec<Entry>,
    free: Vec<u32>,
    ehash: BTreeMap<Tuple, u32>,
    lhash: BTreeMap<u16, u32>,
}

impl Table {
    fn new() -> Self {
        Table { entries: Vec::new(), free: Vec::new(), ehash: BTreeMap::new(), lhash: BTreeMap::new() }
    }

    fn index_of(&self, h: Handle) -> Option<usize> {
        let (i, generation) = (h & 0xffff_ffff, (h >> 32) as u32);
        let e = self.entries.get(i)?;
        (e.generation == generation && e.conn.is_some()).then_some(i)
    }

    fn handle_at(&self, i: usize) -> Handle {
        ((self.entries[i].generation as usize) << 32) | i
    }

    fn get(&self, h: Handle) -> Option<&TcpConn> {
        self.entries[self.index_of(h)?].conn.as_deref()
    }

    fn get_mut(&mut self, h: Handle) -> Option<&mut TcpConn> {
        let i = self.index_of(h)?;
        self.entries[i].conn.as_deref_mut()
    }

    fn link(&mut self, i: usize) {
        let Some(c) = self.entries[i].conn.as_deref() else { return };
        if c.state == State::Listen {
            self.lhash.insert(c.local_port, i as u32);
        } else {
            self.ehash.insert((c.local_port, c.remote_ip, c.remote_port), i as u32);
        }
    }

    fn unlink(&mut self, i: usize) {
        let Some(c) = self.entries[i].conn.as_deref() else { return };
        if c.state == State::Listen {
            if self.lhash.get(&c.local_port) == Some(&(i as u32)) {
                self.lhash.remove(&c.local_port);
            }
        } else {
            let t = (c.local_port, c.remote_ip, c.remote_port);
            if self.ehash.get(&t) == Some(&(i as u32)) {
                self.ehash.remove(&t);
            }
        }
    }

    fn insert(&mut self, conn: TcpConn) -> Handle {
        let i = match self.free.pop() {
            Some(i) => i as usize,
            None => {
                self.entries.push(Entry { generation: 0, conn: None });
                self.entries.len() - 1
            }
        };
        self.entries[i].conn = Some(Box::new(conn));
        self.link(i);
        self.handle_at(i)
    }

    /// Free the connection at `h`; its place takes the next generation.
    fn remove(&mut self, h: Handle) {
        let Some(i) = self.index_of(h) else { return };
        self.unlink(i);
        let e = &mut self.entries[i];
        e.conn = None;
        e.generation = e.generation.wrapping_add(1);
        self.free.push(i as u32);
    }

    /// Change what the connection at `h` is found by.
    fn relink(&mut self, h: Handle, f: impl FnOnce(&mut TcpConn)) {
        let Some(i) = self.index_of(h) else { return };
        self.unlink(i);
        if let Some(c) = self.entries[i].conn.as_deref_mut() { f(c); }
        self.link(i);
    }

    fn find(&self, t: Tuple) -> Option<Handle> {
        self.ehash.get(&t).map(|&i| self.handle_at(i as usize))
    }

    fn listener(&self, port: u16) -> Option<Handle> {
        self.lhash.get(&port).map(|&i| self.handle_at(i as usize))
    }

    fn port_in_use(&self, port: u16) -> bool {
        self.lhash.contains_key(&port)
            || self.ehash.range((port, [0; 4], 0)..=(port, [255; 4], u16::MAX)).next().is_some()
    }

    fn iter(&self) -> impl Iterator<Item = &TcpConn> {
        self.entries.iter().filter_map(|e| e.conn.as_deref())
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = &mut TcpConn> {
        self.entries.iter_mut().filter_map(|e| e.conn.as_deref_mut())
    }

    fn handles(&self) -> Vec<Handle> {
        (0..self.entries.len()).filter(|&i| self.entries[i].conn.is_some()).map(|i| self.handle_at(i)).collect()
    }
}

/// tcp.c `tcp_init`: the established hash gets one bucket per 128 KiB of
/// memory (`alloc_large_system_hash` at scale 17), rounded up to a power of
/// two, and orphans and TIME-WAIT sockets may each fill half of it.
fn ehash_entries(ram_bytes: u64) -> usize {
    ((ram_bytes >> 17) as usize).max(1).next_power_of_two()
}

/// The connection table and everything that runs on it.
pub struct Stack {
    conns: Table,
    isn_secret: [u8; 32],
    st: &'static Stats,
    /// `sysctl_tcp_max_orphans`: connections closed by their owner still
    /// finishing their FIN exchange.
    max_orphans: usize,
    /// `sysctl_max_tw_buckets`: connections in TIME-WAIT.
    max_tw_buckets: usize,
}

impl Stack {
    /// An empty table. `isn_secret` keys the initial sequence numbers and
    /// must be secret and fresh per boot (RFC 6528); `ram_bytes` sizes the
    /// limits as Linux does.
    pub fn new(isn_secret: [u8; 32], st: &'static Stats, ram_bytes: u64) -> Self {
        let cnt = ehash_entries(ram_bytes);
        Stack { conns: Table::new(), isn_secret, st, max_orphans: cnt / 2, max_tw_buckets: cnt / 2 }
    }

    pub fn stats(&self) -> &'static Stats {
        self.st
    }

    /// A random ephemeral port no connection uses (RFC 6056, algorithm 1).
    /// Sequential ports let an off-path sender guess the 4-tuple of every
    /// connection to a known server.
    fn alloc_port<E: Env + ?Sized>(&self, env: &mut E) -> u16 {
        let mut port = 0;
        for _ in 0..64 {
            port = 49152 + (env.random_u64() % 16384) as u16;
            if !self.conns.port_in_use(port) {
                break;
            }
        }
        port
    }

    /// Open a TCP connection without waiting for the handshake: the handle
    /// comes back at once, the caller asks `connect_status` until it answers.
    ///
    /// The cold-cache ARP wait becomes part of the same state machine: we
    /// ask once here and hold the SYN back (`arp_pending`) until `tick` sees
    /// the answer. Sending it to broadcast meanwhile is what most gateways
    /// drop.
    ///
    /// On behalf of `owner`: a module holds at most `PER_OWNER_MAX`.
    pub fn connect_start<E: Env + ?Sized>(
        &mut self, env: &mut E, owner: Owner, remote_ip: [u8; 4], remote_port: u16,
    ) -> Result<Handle, TcpError> {
        let local_port = self.alloc_port(env);
        let iss = generate_isn(&self.isn_secret, env.ticks(), env.our_ip(), remote_ip, local_port, remote_port);

        let arp_target = env.arp_target_for(remote_ip);
        let arp_pending = arp_target != [255, 255, 255, 255]
            && !env.arp_known(arp_target);
        if arp_pending { env.arp_request(arp_target); }

        let mut conn = TcpConn::blank(State::SynSent, local_port, owner);
        conn.remote_ip = remote_ip;
        conn.remote_port = remote_port;
        conn.snd_nxt = iss.wrapping_add(1);
        conn.snd_una = iss;
        conn.snd_iss = iss;
        conn.last_send_tick = env.ticks();
        conn.arp_pending = arp_pending;
        conn.arp_tries = 1;
        conn.set_path(env);
        // tcp_v4_connect: `inet_id` starts at random.
        conn.inet_id.set(env.random_u64() as u16);

        // A connection the owner still holds counts against it until the
        // owner lets go, even once it is Closed (reset, retransmits
        // exhausted).
        if owner != KERNEL
            && self.conns.iter().filter(|c| c.owner == owner).count() >= PER_OWNER_MAX
        {
            return Err(TcpError::TooManyConnections);
        }
        let handle = self.conns.insert(conn);

        // Only when the next hop is known. Otherwise `tick` releases it.
        if !arp_pending { self.send_syn(env, handle)?; }

        Ok(handle)
    }

    /// Connection state: 1 = usable, 0 = still handshaking, -1 = the peer
    /// hung up cleanly, -2 = it failed (reset, or we ran out of retransmits —
    /// i.e. the link stopped acknowledging).
    ///
    /// The two negatives are worth separating: "the far end closed" and "the
    /// link went dead under us" look identical to a caller that only sees
    /// failure, and they are opposite faults.
    pub fn connect_status(&self, handle: Handle) -> i32 {
        match self.conns.get(handle) {
            Some(c) if c.error => -2,
            Some(c) if c.closed || c.state == State::Closed => -1,
            // Same predicate as `conn_healthy`: a peer FIN moves us to
            // CloseWait and sets `closed`, so a module polling this learns the
            // far end hung up. `recv` never tells it — it just returns 0 bytes
            // forever.
            Some(c) if c.established && c.state == State::Established => 1,
            Some(_) => 0,
            None => -1,
        }
    }

    /// The state of the connection at `handle` (`netstat`'s names).
    pub fn state_name(&self, handle: Handle) -> Option<&'static str> {
        self.conns.get(handle).map(|c| c.state.name())
    }

    /// How far the connection at `handle` got.
    pub fn conn_debug(&self, handle: Handle) -> Option<ConnDebug> {
        self.conns.get(handle).map(|c| ConnDebug {
            state: c.state.name(),
            arp_pending: c.arp_pending,
            arp_tries: c.arp_tries,
            retries: c.retries,
        })
    }

    /// Listen on a local port. Returns handle.
    pub fn listen(&mut self, port: u16) -> Result<Handle, TcpError> {
        Ok(self.conns.insert(TcpConn::blank(State::Listen, port, KERNEL)))
    }

    /// Where a listening handle stands: `Ok(true)` once a connection is
    /// established on it, `Ok(false)` while it waits.
    pub fn accept_status(&self, handle: Handle) -> Result<bool, TcpError> {
        match self.conns.get(handle) {
            Some(c) if c.established => Ok(true),
            Some(c) if c.error || c.closed => Err(TcpError::ConnectionFailed),
            Some(_) => Ok(false),
            None => Err(TcpError::NotConnected),
        }
    }

    /// Check if a listening handle has an established connection.
    pub fn is_established(&self, handle: Handle) -> bool {
        self.conns.get(handle).map_or(false, |c| c.established)
    }

    /// Reset a connection back to Listen state (for accepting next client).
    pub fn reset_to_listen(&mut self, handle: Handle) -> Result<(), TcpError> {
        let port = self.conns.get(handle).ok_or(TcpError::NotConnected)?.local_port;
        self.conns.relink(handle, |c| *c = TcpConn::blank(State::Listen, port, KERNEL));
        Ok(())
    }

    /// How far the send buffer has grown (with `snd_wnd_of`, what the peer
    /// last offered). Diagnostics: shows whether `tcp_sndbuf_expand` took
    /// effect.
    pub fn snd_limit_of(&self, handle: Handle) -> usize {
        self.conns.get(handle).map_or(0, |c| c.snd_buf_limit)
    }

    /// Bytes currently unacknowledged in the send buffer.
    pub fn snd_unacked_of(&self, handle: Handle) -> usize {
        self.conns.get(handle).map_or(0, |c| c.send_buf.len())
    }

    /// (cwnd in packets, ssthresh, duplicate ACKs, in recovery)
    pub fn cwnd_of(&self, handle: Handle) -> (u32, u32, u32, bool) {
        self.conns.get(handle).map_or((0, 0, 0, false), |c| {
            (c.snd_cwnd,
             if c.snd_ssthresh == u32::MAX { 0 } else { c.snd_ssthresh },
             c.dupacks, c.in_recovery)
        })
    }

    pub fn snd_wnd_of(&self, handle: Handle) -> usize {
        self.conns.get(handle).map_or(0, |c| c.snd_wnd as usize)
    }

    /// Send data on a connection. Buffers and sends immediately (no Nagle).
    ///
    /// The bytes are also kept in `send_buf` until the peer acknowledges
    /// them, so `tick` can retransmit. Returns `WouldBlock` when too much is
    /// already unacknowledged — that is real backpressure, not an error.
    pub fn send<E: Env + ?Sized>(&mut self, env: &mut E, handle: Handle, data: &[u8]) -> Result<(), TcpError> {
        let mut io = Io { env, st: self.st };
        let conn = self.conns.get_mut(handle).ok_or(TcpError::NotConnected)?;
        // The peer's FIN closes only its direction: we may go on sending.
        if !matches!(conn.state, State::Established | State::CloseWait) || conn.fin_queued {
            return Err(TcpError::NotConnected);
        }
        // An empty buffer always accepts. Otherwise a call larger than the
        // limit could never get through, and a refusal that waiting cannot
        // change is a deadlock.
        if !conn.send_buf.is_empty()
            && (conn.send_buf.len() + data.len() > snd_allowed(conn) || !mem_charge(&io, data.len()))
        {
            return Err(TcpError::WouldBlock);
        }

        let now = io.ticks();
        // Oldest unacked byte starts its clock now if nothing was in flight.
        if conn.send_buf.is_empty() { conn.rto_tick = now; conn.retries = 0; }
        conn.send_buf.extend_from_slice(data);

        // Send in effective-MSS chunks immediately (no Nagle). Effective, not
        // MSS: the option bytes come out of the same 1514.
        let buf_now = conn.send_buf.len() as u64;
        if buf_now > io.st.send_maxbuf.load(Ordering::Relaxed) {
            io.st.send_maxbuf.store(buf_now, Ordering::Relaxed);
        }
        write_xmit(conn, &mut io);

        Ok(())
    }

    /// Receive data. Returns available data (may be empty if nothing
    /// received yet). Sends a window update ACK if significant buffer space
    /// was freed.
    pub fn recv<E: Env + ?Sized>(&mut self, env: &mut E, handle: Handle, buf: &mut [u8]) -> Result<Handle, TcpError> {
        let mut io = Io { env, st: self.st };
        let conn = self.conns.get_mut(handle).ok_or(TcpError::NotConnected)?;

        let pre_len = conn.recv_buf.len();
        let available = pre_len.min(buf.len());
        // Bulk copy out of the ring buffer instead of byte-by-byte pop_front.
        // The VecDeque exposes its contents as up to two contiguous slices;
        // memcpy each, then drain in one shot.
        {
            let (a, b) = conn.recv_buf.as_slices();
            let na = a.len().min(available);
            buf[..na].copy_from_slice(&a[..na]);
            if na < available {
                buf[na..available].copy_from_slice(&b[..available - na]);
            }
            conn.recv_buf.drain(..available);
        }
        // tcp_input.c:930-932 "This function should be called every time
        // data is copied to user space."
        conn.copied_total = conn.copied_total.saturating_add(available as u64);
        rcv_space_adjust(conn, io.env.uptime_us());

        // Window-update ACK, rate-limited.
        //
        // We must re-advertise the window the consumer just reopened so a
        // trickle / zero-window sender resumes (e.g. a TLS sender that bursts
        // then goes quiet, leaving the window stuck small). But a bulk
        // plain-http download calls recv() once per packet, so ACKing on
        // every drain would flood the TX path and defeat the `input` ACK
        // coalescing.
        //
        // So: ACK immediately only when the window was actually constrained
        // (buffer >1/4 full → window shrinking, the trickle/zero-window
        // case), otherwise at most once per ~64 KiB freed. `input`'s
        // coalesced data-ACKs carry the (wide-open) window the rest of the
        // time.
        conn.freed_since_winupd = conn.freed_since_winupd.saturating_add(available as u32);
        let constrained = pre_len > RECV_BUF_SIZE / 4;
        if available > 0 && conn.state == State::Established
            && (constrained || conn.freed_since_winupd >= 64 * 1024) {
            let w = recv_window(conn, io.st);
            send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
            conn.freed_since_winupd = 0;
            conn.ack_pending = false;
            conn.acks_held = 0;
        }

        Ok(available)
    }

    /// Whether a connection has nothing more to deliver: `Ok(true)` once
    /// the peer closed or the connection failed.
    pub fn recv_done(&self, handle: Handle) -> Result<bool, TcpError> {
        match self.conns.get(handle) {
            Some(c) => Ok(c.closed || c.error),
            None => Err(TcpError::NotConnected),
        }
    }

    /// True if `handle` is an established, un-closed, un-errored connection
    /// — i.e. safe to send another request on (HTTP keep-alive reuse). A
    /// peer FIN moves the state out of `Established` (→ CloseWait) and sets
    /// `closed`, so a server that dropped an idle keep-alive connection
    /// reads as unhealthy here and the caller reconnects instead of hanging.
    pub fn conn_healthy(&self, handle: Handle) -> bool {
        matches!(self.conns.get(handle), Some(c)
            if c.state == State::Established && !c.closed && !c.error)
    }

    /// Where this connection goes. For connection coalescing: two names may
    /// share a connection only if they lead to the same address.
    pub fn peer(&self, handle: Handle) -> Option<([u8; 4], u16)> {
        self.conns.get(handle).map(|c| (c.remote_ip, c.remote_port))
    }

    /// Close a connection gracefully (sends FIN) and return at once.
    ///
    /// Linux's `close()` does not wait either: the socket lingers in the
    /// background and only `SO_LINGER` — off by default — makes it block.
    ///
    /// The FIN goes out, the slot stays in FinWait1, and `tick` carries it
    /// to TimeWait or reaps it if the peer never answers.
    ///
    /// With more orphans than `tcp_max_orphans` the connection is reset
    /// instead, as `tcp_close` does when `tcp_check_oom` says so.
    pub fn close<E: Env + ?Sized>(&mut self, env: &mut E, handle: Handle) -> Result<(), TcpError> {
        let mut io = Io { env, st: self.st };
        let conn = self.conns.get_mut(handle).ok_or(TcpError::NotConnected)?;
        // tcp_close_state: who has a FIN to send, and the state it leads to.
        let next = match conn.state {
            State::Established | State::SynReceived => State::FinWait1,
            State::CloseWait => State::LastAck,
            // Never established, or already shutting down: nothing to say.
            _ => {
                self.conns.remove(handle);
                return Ok(());
            }
        };
        conn.released = true;
        // __tcp_close: data the owner never read means it did not want the
        // rest either; the peer learns so by a reset (RFC 2525 §2.17).
        if !conn.recv_buf.is_empty() || !conn.ooo.is_empty() {
            let w = recv_window(conn, io.st);
            send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, RST | ACK, w, &[]);
            self.conns.remove(handle);
            return Ok(());
        }
        conn.state = next;
        conn.fin_queued = true;
        conn.last_send_tick = io.ticks();
        if conn.snd_una == conn.snd_nxt {
            // The FIN is the oldest unacknowledged byte: its clock starts now.
            conn.rto_tick = io.ticks();
            conn.retries = 0;
        }
        // The FIN goes after everything still queued (RFC 9293 §3.10.4).
        write_xmit(conn, &mut io);

        let orphans = self.conns.iter()
            .filter(|c| c.released && matches!(c.state, State::FinWait1 | State::FinWait2 | State::Closing | State::LastAck))
            .count();
        if orphans > self.max_orphans {
            if let Some(conn) = self.conns.get(handle) {
                send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, RST | ACK, 0, &[]);
            }
            self.conns.remove(handle);
        }
        Ok(())
    }

    /// Drop the connection at `handle` without a word to the peer.
    pub fn remove(&mut self, handle: Handle) {
        self.conns.remove(handle);
    }

    /// Handle an incoming TCP segment from `src_ip` (called from ipv4).
    pub fn input<E: Env + ?Sized>(&mut self, env: &mut E, src_ip: [u8; 4], data: &[u8]) {
        let mut io = Io { env, st: self.st };
        let secret = &self.isn_secret;
        if data.len() < HEADER_LEN { return; }

        let src_port = u16::from_be_bytes([data[0], data[1]]);
        let dst_port = u16::from_be_bytes([data[2], data[3]]);
        let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ack = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        let data_offset = ((data[12] >> 4) as usize) * 4;
        if data_offset < HEADER_LEN { return; }
        let flags = data[13];
        let adv_window = u16::from_be_bytes([data[14], data[15]]);

        let payload = if data_offset < data.len() { &data[data_offset..] } else { &[] };

        let conns = &mut self.conns;

        let handle = match conns.find((dst_port, src_ip, src_port)) {
            Some(h) => h,
            None => {
                // Check for a listener on this port
                if flags & SYN != 0 {
                    if let Some(li) = conns.listener(dst_port) {
                        // Accept the SYN on the listening socket
                        let iss = generate_isn(secret, io.ticks(), io.env.our_ip(), src_ip, dst_port, src_port);
                        let peer_ws = parse_wscale(data, data_offset);
                        let peer_mss = parse_mss(data, data_offset);
                        let now = io.ticks();
                        let new_id = io.env.random_u64() as u16;
                        let env = &*io.env;
                        let mut advmss = 0;
                        conns.relink(li, |conn| {
                            conn.state = State::SynReceived;
                            conn.remote_ip = src_ip;
                            conn.remote_port = src_port;
                            conn.rcv_off = 0;
                            conn.rcv_nxt = seq.wrapping_add(1);
                            conn.snd_iss = iss;
                            conn.snd_nxt = iss.wrapping_add(1);
                            conn.snd_una = iss;
                            conn.last_send_tick = now;
                            // Scaling is active only if the peer offered it too.
                            conn.wscale_ok = peer_ws.is_some();
                            conn.snd_wscale = peer_ws.unwrap_or(0);
                            conn.set_path(env);
                            conn.inet_id.set(new_id);
                            conn.mss_clamp = peer_mss.unwrap_or(TCP_MSS_DEFAULT);
                            conn.sync_mss();
                            advmss = conn.advmss;
                        });

                        // SYN-ACK: MSS, and Window Scale only if the peer asked for it.
                        let m = advmss.to_be_bytes();
                        let opts: &[u8] = if peer_ws.is_some() {
                            &[2, 4, m[0], m[1], 1, 3, 3, OUR_WSCALE]
                        } else {
                            &[2, 4, m[0], m[1]]
                        };
                        send_segment_with_opts(
                            &mut io, src_ip, dst_port, src_port,
                            iss, seq.wrapping_add(1), SYN | ACK, INITIAL_WINDOW, &[], opts, 0,
                        );
                        return;
                    }
                }
                // No connection and no listener: send RST if not RST
                if flags & RST == 0 {
                    send_segment(&mut io, src_ip, dst_port, src_port, ack, seq.wrapping_add(1), RST | ACK, 0, &[], 0);
                }
                return;
            }
        };

        let Some(conn) = conns.get_mut(handle) else { return };
        let was_time_wait = conn.state == State::TimeWait;

        // RST (RFC 9293 §3.10.7, RFC 5961 §3). In SYN-SENT it must acknowledge
        // our SYN. Elsewhere it resets only at exactly rcv_nxt; anywhere else in
        // the window it gets a challenge ACK (a true peer then resends the RST
        // with the right number), outside it is dropped. Otherwise anyone who
        // guesses the 4-tuple can tear down a connection.
        if flags & RST != 0 {
            let accept = match conn.state {
                State::SynSent => flags & ACK != 0 && ack == conn.snd_nxt,
                State::Listen => false,
                _ => seq == conn.rcv_nxt,
            };
            if accept {
                conn.error = true;
                conn.state = State::Closed;
                conn.last_send_tick = io.ticks();
            } else if !matches!(conn.state, State::SynSent | State::Listen)
                && seq_in_window(conn, seq)
            {
                let w = recv_window(conn, io.st);
                send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
            }
            return;
        }

        // A synchronized connection takes a segment only if it starts inside
        // the receive window (RFC 9293 §3.10.7.4). Old retransmissions (behind
        // rcv_nxt) keep going to the D-SACK path below, which only ACKs; one
        // beyond the window gets an ACK and nothing else — no window update, no
        // ACK processing, no FIN.
        if matches!(conn.state, State::Established | State::FinWait1 | State::FinWait2 | State::CloseWait
                | State::Closing | State::LastAck | State::TimeWait)
            && (seq.wrapping_sub(conn.rcv_nxt) as i32) > 0
            && !seq_in_window(conn, seq)
        {
            let w = recv_window(conn, io.st);
            send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
            return;
        }
        // An ACK for data we never sent says nothing true: ACK it back and drop
        // (RFC 9293 §3.10.7.4, "ACK of something not yet sent").
        if flags & ACK != 0
            && !matches!(conn.state, State::SynSent | State::Listen | State::SynReceived)
            && (ack.wrapping_sub(conn.snd_nxt) as i32) > 0
        {
            let w = recv_window(conn, io.st);
            send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
            return;
        }

        match conn.state {
            State::SynReceived => {
                // Waiting for ACK of our SYN-ACK
                if flags & ACK != 0 && ack == conn.snd_nxt {
                    conn.snd_una = ack;
                    conn.state = State::Established;
                    conn.established = true;
                }
            }

            State::SynSent => {
                // Only the answer to our SYN: it must acknowledge exactly our
                // ISN + 1. Anything else would hand the connection a sequence
                // space chosen by whoever sent it.
                if flags & SYN != 0 && flags & ACK != 0 && ack == conn.snd_nxt {
                    // SYN-ACK received
                    conn.rcv_off = 0;
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
                    // The peer's MSS, or the default; then the MSS for the
                    // options now negotiated.
                    if let Some(m) = parse_mss(data, data_offset) { conn.mss_clamp = m; }
                    conn.sync_mss();

                    // Send ACK with full window
                    let w = recv_window(conn, io.st);
                    send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                }
            }

            State::Established | State::CloseWait | State::FinWait1 | State::FinWait2
            | State::Closing | State::LastAck | State::TimeWait => {
                // ACK processing
                if flags & ACK != 0 && conn.state != State::TimeWait {
                    // The peer's window: flow control per RFC 9293 §3.8.6. The
                    // scale is the SYN-ACK's (`snd_wscale`) and applies to every
                    // segment except the SYN itself (RFC 7323 §2.2).
                    let vorheriges_fenster = conn.snd_wnd;
                    conn.snd_wnd = (adv_window as u32) << conn.snd_wscale;
                    // tcp_ack_update_window: a larger window may lift the
                    // half-window bound on the MSS.
                    if conn.snd_wnd > conn.max_window {
                        conn.max_window = conn.snd_wnd;
                        conn.sync_mss();
                    }
                    // Every ACK wakes the sender, not only one that clears data:
                    // a pure window update carries `ack == snd_una`, and the
                    // sender must notice the reopened window. The wakeup does not
                    // claim anything changed, only that a retry might be worth it.
                    io.st.ack_gen.fetch_add(1, Ordering::Relaxed);
                    // The RTT sample belongs to the ACK, not to the data: during
                    // an upload the peer sends bare ACKs, and without a sample
                    // `snd_space_adjust` could never grow the send buffer. Linux
                    // samples in `tcp_ack_update_rtt`, called from
                    // `tcp_clean_rtx_queue`, where an ACK comes in.
                    if conn.ts_ok {
                        if let Some(tsecr) = parse_tsecr(data, data_offset) {
                            if tsecr != 0 {
                                let sample = io.ts_now_ms().wrapping_sub(tsecr);
                                if (1..6000).contains(&sample) {
                                    conn.srtt_ms = if conn.srtt_ms == 0 {
                                        sample
                                    } else {
                                        (conn.srtt_ms * 7 + sample) / 8
                                    };
                                }
                            }
                        }
                    }
                    // ── Duplicate ACK: RFC 5681 §3.2 ─────────────────────
                    //
                    // Three in a row mean "one segment is missing, the rest is
                    // arriving". Without fast retransmit the only recovery is
                    // the RTO, one MSS per round, which stalls a large window.
                    //
                    // A duplicate ACK acknowledges no new bytes, carries no
                    // payload and does not change the window; otherwise it is a
                    // window update.
                    let ist_dup = payload.is_empty()
                        && ack == conn.snd_una
                        && !conn.send_buf.is_empty()
                        && conn.snd_wnd == vorheriges_fenster;
                    if ist_dup {
                        conn.dupacks += 1;
                        io.st.dupacks_seen.fetch_add(1, Ordering::Relaxed);
                        if conn.dupacks == DUPACK_THRESH
                            && (!conn.in_recovery || !CONG_CONTROL)
                        {
                            // Halve, enter recovery, and resend the missing
                            // segment now instead of waiting for the RTO.
                            if CONG_CONTROL {
                                conn.snd_ssthresh = reno_ssthresh(conn);
                                conn.snd_cwnd =
                                    conn.snd_ssthresh + DUPACK_THRESH;
                                conn.in_recovery = true;
                                conn.recovery_end = conn.snd_nxt;
                                conn.snd_cwnd_cnt = 0;
                            }
                            conn.dupacks = 0;
                            // Only the missing segment, not the whole window:
                            // everything behind it is in flight and may stay
                            // so. Go-back-N (`snd_nxt = snd_una`) would make
                            // `write_xmit` burst hundreds of packets at once; it
                            // belongs to the RTO, where we really know nothing.
                            retransmit_head(conn, &mut io);
                            io.st.fast_retrans.fetch_add(1, Ordering::Relaxed);
                        } else if conn.in_recovery {
                            // "Inflate": every further duplicate ACK means a
                            // segment has left the network.
                            conn.snd_cwnd += 1;
                            write_xmit(conn, &mut io);
                        }
                    }
                    if ack_in_range(conn.snd_una, ack, conn.snd_nxt) {
                        // Drop the acknowledged prefix from the retransmit queue
                        // and restart the timer for whatever is still in flight.
                        let acked = ack.wrapping_sub(conn.snd_una) as usize;
                        let drop_n = acked.min(conn.send_buf.len());
                        conn.send_buf.drain(..drop_n);
                        conn.snd_una = ack;
                        conn.retries = 0;
                        conn.rto_tick = io.ticks();
                        // The send buffer grows here — where Linux calls
                        // `tcp_check_space` -> `tcp_new_space` ->
                        // `tcp_sndbuf_expand`: when an ACK has made room.
                        conn.acked_total =
                            conn.acked_total.wrapping_add(acked as u64);
                        snd_space_adjust(conn, io.env.uptime_us());
                        // ── Update the congestion window ────────────────
                        let mss_now = eff_mss(conn).max(1);
                        let acked_pkts = (acked.div_ceil(mss_now) as u32).max(1);
                        conn.dupacks = 0;
                        if conn.in_recovery {
                            // RFC 6582 §3.2: recovery ends only once everything
                            // in flight at entry is acknowledged; otherwise one
                            // loss event halves the window several times.
                            let noch_offen = (conn.recovery_end
                                .wrapping_sub(ack) as i32) > 0;
                            if !noch_offen {
                                conn.in_recovery = false;
                                conn.snd_cwnd = conn.snd_ssthresh;
                                conn.snd_cwnd_cnt = 0;
                            }
                        } else if CONG_CONTROL {
                            cong_avoid(conn, acked_pkts);
                        }
                        write_xmit(conn, &mut io);
                    }
                }

                // Our FIN acknowledged (everything up to it is).
                if conn.fin_sent && conn.snd_una == conn.snd_nxt {
                    match conn.state {
                        State::FinWait1 => conn.state = State::FinWait2,
                        State::Closing => {
                            conn.state = State::TimeWait;
                            conn.last_send_tick = io.ticks();
                        }
                        State::LastAck => conn.state = State::Closed,
                        _ => {}
                    }
                }

                // Data once a side has closed its direction resets the
                // connection: RFC 1122 §4.2.2.13 MUST, Linux
                // tcp_rcv_state_process (TCPAbortOnData).
                let shut = conn.released
                    || matches!(conn.state, State::CloseWait | State::Closing | State::LastAck | State::TimeWait);
                if shut && !payload.is_empty()
                    && (seq.wrapping_add(payload.len() as u32).wrapping_sub(conn.rcv_nxt) as i32) > 0
                {
                    // tcp_v4_send_reset answering this segment.
                    if flags & ACK != 0 {
                        send_segment(&mut io, src_ip, dst_port, src_port, ack, 0, RST, 0, &[], 0);
                    } else {
                        send_segment(&mut io, src_ip, dst_port, src_port, 0,
                            seq.wrapping_add(payload.len() as u32), RST | ACK, 0, &[], 0);
                    }
                    conn.error = true;
                    conn.state = State::Closed;
                    conn.last_send_tick = io.ticks();
                }

                // Data processing
                if !payload.is_empty() && matches!(conn.state, State::Established | State::FinWait1 | State::FinWait2) {
                    if seq == conn.rcv_nxt {
                        // RFC 7323: advance ts_recent to this in-order segment's
                        // TSval so our echoed TSecr gives the sender a fresh RTT.
                        if conn.ts_ok {
                            if let Some(ts) = parse_ts(data, data_offset) {
                                conn.ts_recent = ts;
                            }
                        }
                        let space = RECV_BUF_SIZE - conn.recv_buf.len();
                        let mut copy = payload.len().min(space);
                        if !mem_charge(&io, copy) { copy = 0; }
                        // Bulk append, not byte-by-byte push_back: extend reserves
                        // once and copies.
                        conn.recv_buf.extend(payload[..copy].iter().copied());
                        conn.rcv_nxt = conn.rcv_nxt.wrapping_add(copy as u32);
                        conn.rcv_off += copy as u64;
                        // Gap just filled — pull any now-contiguous segments out of
                        // the reassembly queue. Only the lowest stored offset can be
                        // next; if it doesn't meet rcv_nxt there's still a hole.
                        let mut filled = false;
                        loop {
                            let want = conn.rcv_off;
                            // Peek the lowest stored offset (copy out k+len so the
                            // immutable borrow ends before we remove).
                            let (k, seglen) = match conn.ooo.iter().next() {
                                Some((&k, seg)) => (k, seg.len()),
                                None => break,
                            };
                            // Drop fully-stale segments (already delivered).
                            if k + seglen as u64 <= want {
                                conn.ooo.remove(&k); conn.ooo_bytes -= seglen; continue;
                            }
                            if k != want { break; }                 // still a gap before it
                            if conn.recv_buf.len() + seglen > RECV_BUF_SIZE { break; }
                            let seg = conn.ooo.remove(&k).unwrap();
                            conn.ooo_bytes -= seg.len();
                            conn.rcv_nxt = conn.rcv_nxt.wrapping_add(seg.len() as u32);
                            conn.rcv_off += seg.len() as u64;
                            conn.recv_buf.extend(seg.into_iter());
                            filled = true;
                        }
                        io.st.max_rxbuf.fetch_max(conn.recv_buf.len(),
                            Ordering::Relaxed);

                        // Keep the SACK run-set in sync with what's now delivered, and
                        // refresh the RTT estimate from the peer's echoed TSecr (our
                        // TSval is `io.ts_now_ms()`, so now - TSecr = RTT); it feeds DRS
                        // via `rcv_rtt_update`.
                        let delivered = conn.rcv_off;
                        ooo_runs_trim(&mut conn.ooo_runs, delivered);
                        if conn.ts_ok {
                            if let Some(tsecr) = parse_tsecr(data, data_offset) {
                                if tsecr != 0 {
                                    // In milliseconds. Anything over six
                                    // seconds is not an RTT but a clock wrap or
                                    // an echo from another connection.
                                    let sample = io.ts_now_ms().wrapping_sub(tsecr);
                                    if (1..6000).contains(&sample) {
                                        conn.srtt_ms = if conn.srtt_ms == 0 { sample }
                                            else { (conn.srtt_ms * 7 + sample) / 8 };
                                        rcv_rtt_update(conn, sample * 1000);
                                    }
                                }
                            }
                        }
                        // A filled gap must be ACKed immediately so the sender stops
                        // retransmitting and advances — don't let it sit in coalescing.
                        if filled {
                            let w = recv_window(conn, io.st);
                            send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                            conn.acks_held = 0;
                            conn.ack_pending = false;
                        } else {
                        // Coalesced ACK: one ACK per ACK_COALESCE in-order segments.
                        // A lone held ACK is flushed by the 40 ms timer in
                        // tick_connections so a trickle/idle never strands the sender.
                        conn.acks_held += 1;
                        if conn.acks_held >= ACK_COALESCE {
                            let w = recv_window(conn, io.st);
                            send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                            conn.acks_held = 0;
                            conn.ack_pending = false;
                        } else {
                            conn.ack_pending = true;
                            conn.ack_tick = io.ticks();
                        }
                        }
                    } else {
                        if (seq.wrapping_sub(conn.rcv_nxt) as i32) > 0 {
                            // AHEAD = a real gap (an earlier segment was lost). Buffer
                            // this segment for reassembly + send a duplicate ACK so the
                            // sender fast-retransmits only the hole (RFC 5681) — not the
                            // whole window. Bounded; over budget or already-have → skip.
                            io.st.ooo_ahead.fetch_add(1, Ordering::Relaxed);
                            // Ahead by less than 2^31 (checked above), so this
                            // lands past `rcv_off` and never wraps.
                            let off = conn.rcv_off + seq.wrapping_sub(conn.rcv_nxt) as u64;
                            // Only what fits in the window we can offer: bytes
                            // beyond it were never invited.
                            let in_window = off + payload.len() as u64
                                <= conn.rcv_off + rcv_space(conn) as u64;
                            if !payload.is_empty()
                                && in_window
                                && !conn.ooo.contains_key(&off)
                                && conn.ooo_bytes + payload.len() <= OOO_MAX_BYTES
                                && mem_charge(&io, payload.len())
                            {
                                conn.ooo_bytes += payload.len();
                                conn.ooo.insert(off, payload.to_vec());
                                ooo_runs_add(&mut conn.ooo_runs,
                                    off, off + payload.len() as u64);
                            }
                            let w = recv_window(conn, io.st);
                            send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                            conn.ack_pending = false;
                        } else {
                            // BEHIND = we already have these bytes. Answer with a
                            // D-SACK (RFC 2883): an ACK naming an already
                            // acknowledged range as its first SACK block tells the
                            // sender its retransmit was spurious, so it undoes its
                            // window reduction (Linux: `tcp_dsack_seen` ->
                            // `tcp_undo_cwnd_reduction`). It is not a duplicate ACK
                            // and does not trigger fast retransmit.
                            io.st.ooo_behind.fetch_add(1, Ordering::Relaxed);
                            if !payload.is_empty() {
                                let end = seq.wrapping_add(payload.len() as u32);
                                // Only the part we really already have.
                                let hi = if (end.wrapping_sub(conn.rcv_nxt) as i32) > 0 {
                                    conn.rcv_nxt
                                } else {
                                    end
                                };
                                if (hi.wrapping_sub(seq) as i32) > 0 {
                                    conn.dsack = Some((seq, hi));
                                    let w = recv_window(conn, io.st);
                                    send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                                    conn.dsack = None;
                                    conn.ack_pending = false;
                                    conn.acks_held = 0;
                                }
                            }
                        }
                    }
                }

                // FIN from remote (tcp_fin): it counts only where it sits in
                // the stream, right after this segment's data, once that data
                // is in. A FIN ahead of a gap is dropped; the peer resends it.
                let end = seq.wrapping_add(payload.len() as u32);
                if flags & FIN != 0 && end == conn.rcv_nxt
                    && matches!(conn.state, State::Established | State::FinWait1 | State::FinWait2)
                {
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                    conn.closed = true;
                    conn.state = match conn.state {
                        State::Established => State::CloseWait,
                        // Ours not yet acknowledged: both closed at once.
                        State::FinWait1 => State::Closing,
                        _ => State::TimeWait,
                    };
                    if conn.state == State::TimeWait { conn.last_send_tick = io.ticks(); }
                    let w = recv_window(conn, io.st);
                    send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                } else if flags & FIN != 0 && end.wrapping_add(1) == conn.rcv_nxt
                    && matches!(conn.state, State::CloseWait | State::Closing | State::LastAck | State::TimeWait)
                {
                    // The peer sent its FIN again: our ACK was lost. In
                    // TIME-WAIT that restarts the wait (RFC 9293 §3.10.7.4).
                    if conn.state == State::TimeWait { conn.last_send_tick = io.ticks(); }
                    let w = recv_window(conn, io.st);
                    send_seg(conn, &mut io, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                }
            }

            _ => {}
        }

        // A connection its owner let go of is gone once it closes (tcp_done).
        if conn.state == State::Closed && conn.released {
            conns.remove(handle);
            return;
        }

        // `inet_twsk_alloc` finds no room past `max_tw_buckets`, and
        // `tcp_time_wait` then closes the connection outright.
        if !was_time_wait && conn.state == State::TimeWait {
            let tw = conns.iter().filter(|c| c.state == State::TimeWait).count();
            if tw - 1 >= self.max_tw_buckets {
                if let Some(c) = conns.get_mut(handle) { c.state = State::Closed; }
            }
        }
    }

    /// `tcp_v4_err`: an ICMP error about a segment we sent.
    ///
    /// The quoted sequence number must lie in [snd_una, snd_nxt] (RFC 5927
    /// §4.1), or anyone who guesses the 4-tuple could reset the connection
    /// or talk its MSS down. Fragmentation Needed lowers the path MTU and
    /// resends what is in flight at the new size (`tcp_v4_mtu_reduced`).
    /// Any other error ends a connection still in its handshake; later it
    /// is only remembered (RFC 1122 §4.2.3.9: soft errors).
    pub fn icmp_error<E: Env + ?Sized>(&mut self, env: &mut E, e: &crate::ip::IcmpError) {
        use crate::ip::{ICMP_DEST_UNREACH, ICMP_FRAG_NEEDED, ICMP_PARAMETERPROB, ICMP_TIME_EXCEEDED};
        let t = e.transport();
        let sport = u16::from_be_bytes([t[0], t[1]]);
        let dport = u16::from_be_bytes([t[2], t[3]]);
        let seq = u32::from_be_bytes([t[4], t[5], t[6], t[7]]);
        let Some(h) = self.conns.find((sport, e.inner_dst(), dport)) else { return };
        let mut io = Io { env, st: self.st };
        let Some(conn) = self.conns.get_mut(h) else { return };
        if conn.state != State::Listen
            && seq.wrapping_sub(conn.snd_una) > conn.snd_nxt.wrapping_sub(conn.snd_una)
        {
            return;
        }
        let err = match e.kind {
            ICMP_PARAMETERPROB => SockErr::Protocol,
            ICMP_DEST_UNREACH if e.code == ICMP_FRAG_NEEDED => {
                if matches!(conn.state, State::Listen | State::Closed) { return; }
                io.env.update_pmtu(conn.remote_ip, e.info);
                let mtu = io.env.mtu(conn.remote_ip);
                if mtu < conn.pmtu {
                    conn.pmtu = mtu;
                    conn.sync_mss();
                    // "Resend the TCP packet because it's clear that old
                    // packet has been dropped."
                    retransmit_all(conn, &mut io);
                }
                return;
            }
            // icmp_err_convert.
            ICMP_DEST_UNREACH => match e.code {
                2 => SockErr::Protocol,
                3 => SockErr::Refused,
                0 | 6 | 9 | 11 => SockErr::NetUnreachable,
                _ => SockErr::HostUnreachable,
            },
            ICMP_TIME_EXCEEDED => SockErr::HostUnreachable,
            _ => return,
        };
        conn.sock_err = Some(err);
        if matches!(conn.state, State::SynSent | State::SynReceived) {
            // tcp_done
            conn.error = true;
            conn.state = State::Closed;
            conn.last_send_tick = io.ticks();
        }
    }

    /// The error the connection at `handle` ended with or last learned of.
    pub fn conn_error(&self, handle: Handle) -> Option<SockErr> {
        self.conns.get(handle).and_then(|c| c.sock_err)
    }

    /// Periodic tick: retransmit, delayed ACKs, timeouts. Returns the segments
    /// and ARP probes to send once the caller let go of the table.
    pub fn tick<E: Env + ?Sized>(&mut self, env: &mut E) -> (Vec<PendingSeg>, Vec<[u8; 4]>) {
        let mut io = Io { env, st: self.st };
        let now = io.ticks();

        // Collect the segments to send while holding the table (they read conn
        // state); the caller sends them after letting go. Holding the table
        // across the TX doorbell would block worker-core `recv` behind Core-0's
        // periodic ACKs/retries.
        let mut pending: alloc::vec::Vec<PendingSeg> = alloc::vec::Vec::new();
        // Same reason: an ARP request hits the NIC, so collect and fire after
        // the table is let go.
        let mut arp_probes: alloc::vec::Vec<[u8; 4]> = alloc::vec::Vec::new();
        // Retransmits are not collected here: `retransmit_head` and `write_xmit`
        // send them from the send buffer like any other segment.
        {
            let conns = &mut self.conns;
            let held: usize = conns.iter()
                .map(|c| c.recv_buf.len() + c.ooo_bytes + c.send_buf.len())
                .sum();
            io.st.mem_held.store(held, Ordering::Relaxed);
            for slot in conns.iter_mut() {
                // Send what the driver queue refused earlier. Otherwise it would
                // wait for the caller's next `send`, while the caller waits for
                // an ACK for bytes that never went out.
                if slot.sends()
                    && (slot.snd_nxt != slot.snd_una.wrapping_add(slot.send_buf.len() as u32)
                        || (slot.fin_queued && !slot.fin_sent))
                {
                    write_xmit(slot, &mut io);
                }
                // Delayed ACK
                if slot.ack_pending && now - slot.ack_tick >= DELAYED_ACK_TICKS {
                    let w = recv_window(slot, io.st);
                    let mut opts = [0u8; 40];
                    let len = build_seg_opts(slot, ACK, &mut opts, false, io.ts_now_ms());
                    pending.push(PendingSeg {
                        dst_ip: slot.remote_ip, src_port: slot.local_port,
                        dst_port: slot.remote_port, seq: slot.snd_nxt,
                        ack: slot.rcv_nxt, flags: ACK, window: w, opts, opts_len: len,
                        id: slot.next_id(),
                    });
                    slot.ack_pending = false;
                    slot.acks_held = 0;
                }

                // SYN retry
                if slot.state == State::SynSent {
                    let mut opts = [0u8; 40];
                    let opts_len = syn_opts(&mut opts, io.ts_now_ms(), slot.advmss);
                    let mut send_syn_now = false;

                    if slot.arp_pending {
                        // Next hop still unknown, SYN held back. Re-ask every
                        // RETRANS window — one request is a coin flip over WiFi,
                        // and both the request and the reply can be the loss.
                        let target = io.env.arp_target_for(slot.remote_ip);
                        if io.env.arp_known(target) {
                            slot.arp_pending = false;
                            send_syn_now = true;
                        } else if now.wrapping_sub(slot.last_send_tick) >= ARP_RETRANS_TICKS {
                            slot.arp_tries += 1;
                            slot.last_send_tick = now;
                            if slot.arp_tries > ARP_MAX_TRIES {
                                // Give up asking and send anyway (to broadcast).
                                // From here the normal SYN retry runs.
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
                            opts, opts_len, id: slot.next_id(),
                        });
                    }
                }

                // Data retransmit. `send_buf` starts at snd_una, so the head of
                // it is exactly the segment the peer is missing; the FIN counts
                // too (tcp_retransmit_timer in every state that sends).
                if slot.sends() && slot.snd_una != slot.snd_nxt {
                    let rto = RTO_TICKS_BASE << slot.retries.min(5);
                    if now.saturating_sub(slot.rto_tick) > rto {
                        if slot.retries >= MAX_DATA_RETRIES {
                            slot.error = true;
                            slot.state = State::Closed;
                        } else {
                            slot.retries += 1;
                            slot.rto_tick = now;
                            slot.last_send_tick = now;
                            // ── `tcp_enter_loss` ───────────────────────
                            //
                            // An RTO is the strongest congestion signal
                            // (RFC 5681 §3.1): halve the threshold, window to
                            // one, slow start again.
                            //
                            // `snd_nxt` is not rewound. `ack_in_range(una, ack,
                            // nxt)` only accepts ACKs up to `snd_nxt`; after a
                            // rewind the peer's real cumulative ACK would fall
                            // outside the range and be discarded forever. Like
                            // Linux, `snd_nxt` stays the highest sequence ever
                            // sent and the head is resent from the retransmit
                            // queue (`tcp_xmit_retransmit_queue`).
                            if CONG_CONTROL {
                                slot.snd_ssthresh = reno_ssthresh(slot);
                                slot.snd_cwnd = 1;
                                slot.snd_cwnd_cnt = 0;
                                slot.in_recovery = false;
                            }
                            slot.dupacks = 0;
                            io.st.rto_fired.fetch_add(1, Ordering::Relaxed);
                            retransmit_head(slot, &mut io);
                        }
                    }
                }

                // TimeWait cleanup (2 seconds)
                if slot.state == State::TimeWait && now - slot.last_send_tick > 200 {
                    slot.state = State::Closed;
                }

                // Half-closed with a peer that never answers. `close`
                // leaves FinWait1 behind on purpose and nothing else frees it —
                // without this the slot is pinned for the rest of the boot.
                // 60 s = Linux's tcp_fin_timeout.
                if matches!(slot.state, State::FinWait1 | State::FinWait2 | State::Closing | State::LastAck)
                    && now.saturating_sub(slot.last_send_tick) > FIN_TIMEOUT_TICKS
                {
                    slot.state = State::Closed;
                }
            }
            // A Closed connection its owner let go of is gone; one the kernel
            // still holds after FIN_TIMEOUT_TICKS of silence too, so a
            // forgotten handle cannot pin it.
            for h in conns.handles() {
                if conns.get(h).is_some_and(|c| reusable_conn(c, now)) {
                    conns.remove(h);
                }
            }
        }

        (pending, arp_probes)
    }

    fn send_syn<E: Env + ?Sized>(&mut self, env: &mut E, handle: Handle) -> Result<(), TcpError> {
        let mut io = Io { env, st: self.st };
        let conn = self.conns.get_mut(handle).ok_or(TcpError::NotConnected)?;
        conn.last_send_tick = io.ticks();

        let mut opts = [0u8; 40];
        let len = syn_opts(&mut opts, io.ts_now_ms(), conn.advmss);
        send_segment_with_opts(
            &mut io, conn.remote_ip, conn.local_port, conn.remote_port,
            conn.snd_iss, 0, SYN, INITIAL_WINDOW, &[], &opts[..len], conn.next_id(),
        );
        Ok(())
    }

    /// Does any connection run a timer (retransmit, delayed ACK, SYN retry,
    /// TIME_WAIT, FIN timeout)? The shell loop drives `tick` and
    /// must keep coming back while this holds. As in Linux, an idle established
    /// connection has none: only data in flight or unsent, or an ACK held back.
    pub fn has_timers(&self) -> bool {
        self.conns.iter().any(|c| match c.state {
            State::Closed | State::Listen => false,
            State::Established | State::CloseWait => c.ack_pending || !c.send_buf.is_empty(),
            _ => true,
        })
    }

    pub fn list_connections(&self) -> Vec<(u16, [u8; 4], u16, &'static str)> {
        self.conns.iter()
            .map(|c| (c.local_port, c.remote_ip, c.remote_port, c.state.name()))
            .collect()
    }
}

/// tcp_output.c `tcp_write_xmit` — pushes out what is unsent in the buffer
/// and stops when the device refuses.
///
/// Like Linux with `netif_stop_queue`: when the driver queue says no, the
/// segment stays in the buffer instead of being lost. `snd_nxt` advances
/// only after an accepted handoff, and `tick_connections` sends the rest.
/// A larger send buffer therefore means more waiting bytes, not more
/// dropped ones; otherwise recovery would hinge on the RTO resending one
/// MSS per round.
fn write_xmit<E: Env + ?Sized>(conn: &mut TcpConn, io: &mut Io<E>) {
    let mss = eff_mss(conn).min(1460);
    let mut chunk = [0u8; 1460];
    loop {
        let sent = conn.snd_nxt.wrapping_sub(conn.snd_una) as usize;
        if conn.fin_sent || sent >= conn.send_buf.len() {
            // Everything queued is out; the FIN alone if it still waits.
            if conn.fin_queued && !conn.fin_sent && sent == conn.send_buf.len() {
                let w = recv_window(conn, io.st);
                if send_seg(conn, io, conn.snd_nxt, conn.rcv_nxt, FIN | ACK, w, &[]) {
                    conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
                    conn.fin_sent = true;
                    conn.last_send_tick = io.ticks();
                } else {
                    io.st.send_refused.fetch_add(1, Ordering::Relaxed);
                }
            }
            return;
        }
        // ── Gate 1: the congestion window (tcp_output.c:2238 `tcp_cwnd_test`)
        //
        // `in_flight >= cwnd`: nothing more until an ACK makes room.
        // Disabled while `CONG_CONTROL` is off.
        let in_flight = sent.div_ceil(mss) as u32;
        if CONG_CONTROL && in_flight >= conn.snd_cwnd {
            return;
        }
        // ── Gate 2: the peer's window
        //           (tcp_output.c:2295 `tcp_snd_wnd_test`)
        let n = (conn.send_buf.len() - sent).min(mss);
        if sent + n > conn.snd_wnd as usize && conn.snd_wnd != 0 {
            return;
        }
        chunk[..n].copy_from_slice(&conn.send_buf[sent..sent + n]);
        let seq = conn.snd_nxt;
        let w = recv_window(conn, io.st);
        // The FIN rides on the last byte queued (tcp_send_fin).
        let fin = conn.fin_queued && sent + n == conn.send_buf.len();
        let flags = if fin { FIN | ACK | PSH } else { ACK | PSH };
        if !send_seg(conn, io, seq, conn.rcv_nxt, flags, w, &chunk[..n]) {
            // Full. Nothing lost, just not sent yet.
            io.st.send_refused.fetch_add(1, Ordering::Relaxed);
            return;
        }
        io.st.send_segs.fetch_add(1, Ordering::Relaxed);
        conn.snd_nxt = conn.snd_nxt.wrapping_add(n as u32 + fin as u32);
        if fin { conn.fin_sent = true; }
        conn.last_send_tick = io.ticks();
    }
}

/// `tcp_slow_start` (tcp_cong.c:454) + `tcp_cong_avoid_ai` (:468),
/// combined as in `tcp_reno_cong_avoid` (:493).
fn cong_avoid(conn: &mut TcpConn, acked_pkts: u32) {
    if conn.snd_cwnd < conn.snd_ssthresh {
        // "In safe area, increase."
        conn.snd_cwnd = (conn.snd_cwnd + acked_pkts).min(conn.snd_ssthresh);
        return;
    }
    // "In dangerous area, increase slowly" — one packet per window.
    let w = conn.snd_cwnd.max(1);
    if conn.snd_cwnd_cnt >= w {
        conn.snd_cwnd_cnt = 0;
        conn.snd_cwnd += 1;
    }
    conn.snd_cwnd_cnt += acked_pkts;
    if conn.snd_cwnd_cnt >= w {
        let delta = conn.snd_cwnd_cnt / w;
        conn.snd_cwnd_cnt -= delta * w;
        conn.snd_cwnd += delta;
    }
}

/// Resend the segment at `snd_una`, and only that one: `tcp_retransmit_skb`
/// on the head of the retransmit queue. `snd_nxt` stays put; what lies
/// beyond it is in flight.
fn retransmit_head<E: Env + ?Sized>(conn: &mut TcpConn, io: &mut Io<E>) {
    if conn.send_buf.is_empty() && !conn.fin_sent {
        return;
    }
    let mss = eff_mss(conn).min(1460);
    let n = conn.send_buf.len().min(mss);
    let mut chunk = [0u8; 1460];
    chunk[..n].copy_from_slice(&conn.send_buf[..n]);
    let w = recv_window(conn, io.st);
    // The FIN, alone or on the last data, as it went out first.
    let flags = if conn.fin_sent && n == conn.send_buf.len() {
        if n == 0 { FIN | ACK } else { FIN | ACK | PSH }
    } else {
        ACK | PSH
    };
    if send_seg(conn, io, conn.snd_una, conn.rcv_nxt, flags, w, &chunk[..n]) {
        conn.last_send_tick = io.ticks();
    } else {
        io.st.send_refused.fetch_add(1, Ordering::Relaxed);
    }
}

/// `tcp_simple_retransmit` after the MSS fell: everything in flight was
/// larger than the path now carries, so it goes out again, at the new size.
/// `snd_nxt` stays where it is.
fn retransmit_all<E: Env + ?Sized>(conn: &mut TcpConn, io: &mut Io<E>) {
    let mss = eff_mss(conn).min(1460);
    let in_flight = (conn.snd_nxt.wrapping_sub(conn.snd_una) as usize).min(conn.send_buf.len());
    let mut chunk = [0u8; 1460];
    let mut off = 0;
    while off < in_flight {
        let n = (in_flight - off).min(mss);
        chunk[..n].copy_from_slice(&conn.send_buf[off..off + n]);
        let w = recv_window(conn, io.st);
        let fin = conn.fin_sent && off + n == conn.send_buf.len();
        let flags = if fin { FIN | ACK | PSH } else { ACK | PSH };
        if !send_seg(conn, io, conn.snd_una.wrapping_add(off as u32), conn.rcv_nxt, flags, w, &chunk[..n]) {
            io.st.send_refused.fetch_add(1, Ordering::Relaxed);
            return;
        }
        off += n;
    }
    if in_flight == 0 && conn.fin_sent {
        let w = recv_window(conn, io.st);
        send_seg(conn, io, conn.snd_una, conn.rcv_nxt, FIN | ACK, w, &[]);
    }
    conn.last_send_tick = io.ticks();
}

/// `tcp_reno_ssthresh` (tcp_cong.c:512): half, at least two.
fn reno_ssthresh(conn: &TcpConn) -> u32 {
    (conn.snd_cwnd >> 1).max(2)
}

// === Internal ===

/// SYN options: MSS(4) + SACK-permitted(2) + NOP,NOP + Timestamp(kind=8,10) +
/// NOP + WScale(3) = 22, padded to 24. Returns the length written.
///
/// Shared by the first SYN and every retransmit: a retry with a bare SYN
/// would bring the connection up without window scaling, SACK or timestamps,
/// capped at a 64 KiB window for its whole life.
fn syn_opts(opts: &mut [u8; 40], tsval: u32, advmss: u16) -> usize {
    opts[0] = 2;  // MSS option kind
    opts[1] = 4;  // MSS option length
    opts[2..4].copy_from_slice(&advmss.to_be_bytes());
    opts[4] = 4;            // SACK-permitted kind
    opts[5] = 2;            // length
    opts[6] = 1;            // NOP
    opts[7] = 1;            // NOP — align the 10-byte Timestamp to 4 bytes
    opts[8] = 8;            // Timestamp option kind
    opts[9] = 10;           // length
    opts[10..14].copy_from_slice(&tsval.to_be_bytes()); // TSval
    // opts[14..18] TSecr = 0 on a SYN
    opts[18] = 1;           // NOP — align the 3-byte WScale to a 4-byte boundary
    opts[19] = 3;           // Window Scale option kind
    opts[20] = 3;           // length
    opts[21] = OUR_WSCALE;  // shift count
    24
}

fn send_segment<E: Env + ?Sized>(
    io: &mut Io<E>, dst_ip: [u8; 4], src_port: u16, dst_port: u16,
    seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8], id: u16,
) -> bool {
    send_segment_with_opts(io, dst_ip, src_port, dst_port, seq, ack, flags, window, payload, &[], id)
}

/// `id`: the connection's next identification; 0 for a segment no
/// connection sends (a reset, a SYN-ACK), as Linux's control socket and
/// listener do.
fn send_segment_with_opts<E: Env + ?Sized>(
    io: &mut Io<E>, dst_ip: [u8; 4], src_port: u16, dst_port: u16,
    seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8], options: &[u8], id: u16,
) -> bool {
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
    let src_ip = io.env.our_ip();
    let checksum = tcp_checksum(&src_ip, &dst_ip, &pkt);
    pkt[16..18].copy_from_slice(&checksum.to_be_bytes());

    // The result matters: the driver queue refuses when it is full, and the
    // caller must not treat the segment as sent.
    io.env.ip_send(dst_ip, &pkt, id)
}

fn tcp_checksum(src_ip: &[u8; 4], dst_ip: &[u8; 4], segment: &[u8]) -> u16 {
    use crate::csum::{fold, pseudo, sum};
    !fold(pseudo(*src_ip, *dst_ip, 6, segment.len()) + sum(segment))
}

/// The MSS option (kind 2, len 4) of a SYN; 0 counts as absent, as in
/// `tcp_parse_options`.
fn parse_mss(seg: &[u8], data_offset: usize) -> Option<u16> {
    let end = data_offset.min(seg.len());
    let mut i = HEADER_LEN;
    while i < end {
        match seg[i] {
            0 => break,
            1 => i += 1,
            kind => {
                if i + 1 >= end { break; }
                let len = seg[i + 1] as usize;
                if len < 2 || i + len > end { break; }
                if kind == 2 && len == 4 {
                    let m = u16::from_be_bytes([seg[i + 2], seg[i + 3]]);
                    return (m != 0).then_some(m);
                }
                i += len;
            }
        }
    }
    None
}

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
    if !conn.sack_ok || (conn.ooo_runs.is_empty() && conn.dsack.is_none()) {
        return 0;
    }
    // `ooo_runs` is already coalesced, so this is O(runs) — no per-ACK scan of
    // the whole segment map. Emit the highest up-to-3 runs, highest first
    // (RFC 2018 §4: the most recently received block goes first).
    out[0] = 5;                       // SACK option kind
    let mut p = 2;
    let mut take = 0usize;
    // The D-SACK block must come first (RFC 2883 §4): only the first block
    // of a SACK option may name an already acknowledged range, and that is
    // how the sender recognises a duplicate. Anywhere else it is an ordinary
    // SACK block and says the opposite.
    if let Some((l, r)) = conn.dsack {
        out[p..p + 4].copy_from_slice(&l.to_be_bytes()); p += 4;
        out[p..p + 4].copy_from_slice(&r.to_be_bytes()); p += 4;
        take += 1;
    }
    for (&s, &e) in conn.ooo_runs.iter().rev().take(3 - take) {
        let l = conn.rcv_nxt.wrapping_add(s.wrapping_sub(conn.rcv_off) as u32);
        let r = conn.rcv_nxt.wrapping_add(e.wrapping_sub(conn.rcv_off) as u32);
        out[p..p + 4].copy_from_slice(&l.to_be_bytes()); p += 4;
        out[p..p + 4].copy_from_slice(&r.to_be_bytes()); p += 4;
        take += 1;
    }
    if take == 0 { return 0; }
    out[1] = (2 + 8 * take) as u8;    // length
    p
}

/// Scan a segment's TCP options for the Window Scale option (kind 3) and
/// return its shift count. `data_offset` is the TCP header length in bytes.
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
/// our most recent TSval. Since our TSval is `ts_now_ms()`, now - TSecr is an
/// RTT in milliseconds (used for window auto-tuning).
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

/// Bytes `build_seg_opts` will add to a data segment on this connection. The
/// payload has to shrink by exactly this much, or the frame overruns the MTU:
/// with timestamps every segment carries 12 option bytes, and a full 1460-byte
/// payload would make a 1526-byte frame.
fn data_opts_len(conn: &TcpConn) -> usize {
    if conn.ts_ok { 12 } else { 0 }
}

/// Payload per segment for this connection (`mss_cache`).
fn eff_mss(conn: &TcpConn) -> usize {
    conn.mss_cache as usize
}

/// Build the TCP option list (Timestamp, then SACK blocks during a gap;
/// both 4-byte aligned via leading NOPs) for `conn`/`flags` into `opts`,
/// returning its length. Shared by the inline `send_seg` and the deferred
/// tick path, which materializes segments under the table's lock and
/// sends them after dropping it.
fn build_seg_opts(conn: &TcpConn, flags: u8, opts: &mut [u8; 40], has_payload: bool, tsval: u32) -> usize {
    let mut len = 0;
    if conn.ts_ok {
        opts[len] = 1; opts[len + 1] = 1;          // NOP, NOP
        opts[len + 2] = 8; opts[len + 3] = 10;     // Timestamp kind, len
        opts[len + 4..len + 8].copy_from_slice(&tsval.to_be_bytes());
        opts[len + 8..len + 12].copy_from_slice(&conn.ts_recent.to_be_bytes());
        len += 12;
    }
    // SACK blocks: only on a pure ACK while we hold out-of-order data (a gap).
    // Never on a SYN — that advertises SACK-permitted instead. On a data
    // segment they would push the frame past the MTU: `eff_mss` budgets for
    // the timestamp only, and a variable option length cannot be budgeted for.
    if conn.sack_ok && flags & SYN == 0 && !has_payload
        && (!conn.ooo.is_empty() || conn.dsack.is_some()) {
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

/// Send a segment for a known connection, adding the Timestamp option (our
/// TSval + the peer's echoed TSval) when timestamps were negotiated (RFC 7323).
/// All connection-originated segments (ACKs, data, FIN) must carry it so the
/// sender gets a clean per-segment RTT sample despite our ACK jitter.
fn send_seg<E: Env + ?Sized>(conn: &TcpConn, io: &mut Io<E>, seq: u32, ack: u32, flags: u8,
            window: u16, payload: &[u8]) -> bool {
    io.st.tx_segs.fetch_add(1, Ordering::Relaxed);
    let mut opts = [0u8; 40];
    let len = build_seg_opts(conn, flags, &mut opts, !payload.is_empty(), io.ts_now_ms());
    if len > 0 {
        send_segment_with_opts(io, conn.remote_ip, conn.local_port, conn.remote_port,
            seq, ack, flags, window, payload, &opts[..len], conn.next_id())
    } else {
        send_segment(io, conn.remote_ip, conn.local_port, conn.remote_port,
            seq, ack, flags, window, payload, conn.next_id())
    }
}

/// A fully-resolved zero-payload segment captured under the table's lock
/// so it can be sent (the NIC doorbell) after the lock is dropped. Keeps
/// `tick` from holding the lock across TX, which would block worker-core
/// `recv` behind Core-0's periodic delayed-ACKs / SYN retries.
pub struct PendingSeg {
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    opts: [u8; 40],
    opts_len: usize,
    id: u16,
}

/// Send a segment `tick` returned.
pub fn send_pending<E: Env + ?Sized>(env: &mut E, st: &'static Stats, p: &PendingSeg) {
    let mut io = Io { env, st };
    io.st.tx_segs.fetch_add(1, Ordering::Relaxed);
    if p.opts_len > 0 {
        send_segment_with_opts(&mut io, p.dst_ip, p.src_port, p.dst_port,
            p.seq, p.ack, p.flags, p.window, &[], &p.opts[..p.opts_len], p.id);
    } else {
        send_segment(&mut io, p.dst_ip, p.src_port, p.dst_port,
            p.seq, p.ack, p.flags, p.window, &[], p.id);
    }
}

/// Free receive buffer: the most this connection can ever advertise.
fn rcv_space(conn: &TcpConn) -> usize {
    RECV_BUF_SIZE.saturating_sub(conn.recv_buf.len()).min(RCV_WND_MAX)
}

/// Is `seq` inside [rcv_nxt, rcv_nxt + window)? With a zero window only
/// rcv_nxt itself is.
fn seq_in_window(conn: &TcpConn, seq: u32) -> bool {
    let off = seq.wrapping_sub(conn.rcv_nxt) as usize;
    let wnd = rcv_space(conn);
    if wnd == 0 { off == 0 } else { off < wnd }
}

fn ack_in_range(una: u32, ack: u32, nxt: u32) -> bool {
    // Check if ack is within (una, nxt] accounting for wrapping
    let diff_una = ack.wrapping_sub(una);
    let diff_nxt = nxt.wrapping_sub(una);
    diff_una > 0 && diff_una <= diff_nxt
}

/// Whether a Closed connection can be freed: its owner let go, or the
/// kernel left it alone for FIN_TIMEOUT_TICKS.
fn reusable_conn(c: &TcpConn, now: u64) -> bool {
    c.state == State::Closed && (c.released
        || (c.owner == KERNEL && now.saturating_sub(c.last_send_tick) > FIN_TIMEOUT_TICKS))
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
    /// No route leads to the address, or only a broadcast or multicast one.
    Unreachable,
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
            TcpError::Unreachable => write!(f, "network unreachable"),
        }
    }
}
