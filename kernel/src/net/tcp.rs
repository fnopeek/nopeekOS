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
use core::sync::atomic::{AtomicU64, Ordering};
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

// A single browser page load opens 20+ parallel TLS connections (CDNs,
// telemetry, OCSP, …). 128 matches the NAT session table.
const MAX_CONNECTIONS: usize = 128;
const MSS: u16 = 1460; // standard Ethernet MSS
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

/// What `recv_window` last computed: (advertised window in bytes, srtt in
/// milliseconds, cap in bytes). On a slow link the question is how much we
/// offered and where the cap came from, not how much arrived.
static WND_LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static WND_SRTT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static WND_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// (window bytes, srtt ms, cap bytes) of the last computation.
pub fn window_diag() -> (u32, u32, u32) {
    use core::sync::atomic::Ordering::Relaxed;
    (WND_LAST.load(Relaxed), WND_SRTT.load(Relaxed), WND_CAP.load(Relaxed))
}

/// Manual cap on the advertised window in bytes, 0 = off.
///
/// A diagnostic tool, not a setting. On a saturated path RTT = window /
/// rate, so one measurement cannot tell whether the air or we are the
/// limit; the shape of throughput over several windows can: if it rises
/// with the window, we were the limit; if only the RTT grows, the air is.
static RCV_WND_FORCE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// `net window <KB>` sets the cap, `net window auto` removes it.
pub fn set_rcv_window_force(bytes: u32) {
    RCV_WND_FORCE.store(bytes, core::sync::atomic::Ordering::Relaxed);
}

/// Current manual cap in bytes (0 = off).
pub fn rcv_window_force() -> u32 {
    RCV_WND_FORCE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Our clock for the TCP timestamp option, in milliseconds.
///
/// A 100 Hz tick would measure RTTs in 10 ms steps. Linux uses
/// `TCP_TS_HZ = 1000` (tcp.h); RFC 7323 §4 allows 1 ms to 1 s, and a faster
/// clock would break PAWS (2^32 ms is 49 days, 2^32 us only 71 minutes).
fn ts_now_ms() -> u32 {
    (crate::interrupts::uptime_us() / 1000) as u32
}

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
fn rcv_space_adjust(conn: &mut TcpConn) {
    let now = crate::interrupts::uptime_us();
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
fn snd_space_adjust(conn: &mut TcpConn) {
    let now = crate::interrupts::uptime_us();
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

fn recv_window(conn: &TcpConn) -> u16 {
    let forced = RCV_WND_FORCE.load(core::sync::atomic::Ordering::Relaxed);
    // Without a manual cap DRS decides; with one, `net window` measures what
    // DRS should have found.
    let cap = if forced > 0 {
        (forced as usize).min(RCV_WND_MAX)
    } else {
        (conn.drs_win as usize).clamp(RCV_WND_MIN, RCV_WND_MAX)
    };
    let free = RECV_BUF_SIZE.saturating_sub(conn.recv_buf.len()).min(cap);
    {
        use core::sync::atomic::Ordering::Relaxed;
        WND_LAST.store(free as u32, Relaxed);
        WND_SRTT.store(conn.srtt_ms, Relaxed);
        WND_CAP.store(cap as u32, Relaxed);
    }
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

/// Empty looks at `ACK_GEN` before yielding.
///
/// The ACK comes from the driver fiber on another core; yielding here
/// means missing it by a scheduler pass. 4096 spins are a few
/// microseconds, shorter than any realistic RTT.
const SEND_SPIN_BUDGET: u32 = 4096;

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

/// Yield rounds without a wakeup after which we try again anyway. Costs
/// nothing normally (the wakeup comes long before) and turns a dead wait
/// into a slow one.
const RETRY_ROUNDS: u32 = 16;
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
    // stream offset (seq - rcv_irs). Without it a single lost packet forces
    // the sender into go-back-N (retransmit the whole window), and the re-burst
    // can overflow small NIC FIFOs again. Bounded by OOO_MAX_BYTES (else
    // dropped → the sender retransmits). Offsets assume < 4 GiB per connection.
    ooo: BTreeMap<u32, Vec<u8>>,
    ooo_bytes: usize,
    // Coalesced [start,end) runs of `ooo`, kept in sync — so building SACK
    // blocks is O(runs), not a scan over thousands of `ooo` entries per ACK
    // with a large window. Advisory: a desync only makes SACK suboptimal,
    // never corrupts data (the bytes still come from `ooo`).
    ooo_runs: BTreeMap<u32, u32>,
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

/// Open a TCP connection without waiting for the handshake: the handle comes
/// back at once, the caller asks `connect_status` until it answers.
///
/// This is the form modules get. A blocking wait inside a host call freezes
/// every other fiber on that worker core — including the WiFi driver, whose
/// card then goes unpolled for the whole wait (the RB pool holds milliseconds).
/// `fiber::pump_peers` cannot cover it: it returns early when called from
/// inside a fiber, and a module is a fiber.
///
/// The cold-cache ARP wait becomes part of the same state machine: we ask
/// once here and hold the SYN back (`arp_pending`) until `tick_connections`
/// sees the answer. Sending it to broadcast meanwhile is what most gateways
/// drop.
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
        srtt_ms: 0,
        rcv_rtt_us8: 0,
        rcvq_copied0: 0,
        copied_total: 0,
        rcvq_time_us: 0,
        // `tcp_init_buffer_space`: start from what a fresh connection offers
        // anyway, `TCP_INIT_CWND * advmss`. Without a start value
        // `rcvbuf_grow` divides by zero.
        rcvq_space: 10 * MSS as u32,
        drs_win: RCV_WND_MIN as u32,
        send_buf: Vec::new(),
        retries: 0,
        last_send_tick: crate::interrupts::ticks(),
        rto_tick: 0,
        dsack: None,
        ack_pending: false,
        ack_tick: 0,
        acks_held: 0,
        freed_since_winupd: 0,
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
/// cleanly, -2 = it failed (reset, or we ran out of retransmits — i.e. the
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
        // up. `recv` never tells it — it just returns 0 bytes forever.
        Some(ref c) if c.established && c.state == State::Established => 1,
        Some(_) => 0,
        None => -1,
    }
}

/// Open a TCP connection, blocking until established. Native callers only —
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
            // -2 is our own retry budget running out; -1 is the peer closing
            // the connection (a RST answers a SYN with a refusal). They call
            // for opposite investigations, so they get different errors.
            -2 => {
                if let Some(ref c) = CONNECTIONS.lock()[handle] {
                    crate::kprintln!(
                        "[tcp] connect gave up: state {:?} arp_tries {} syn_retries {}",
                        c.state, c.arp_tries, c.retries);
                }
                close_cleanup(handle);
                return Err(TcpError::Timeout);
            }
            n if n < 0 => {
                close_cleanup(handle);
                return Err(TcpError::ConnectionRefused);
            }
            _ => {}
        }

        if crate::interrupts::ticks() - t0 > 1000 { // 10s timeout
            // Say how far it got. A connect that dies has three distinct
            // shapes: next hop never resolved (`arp_pending` still set, or it
            // gave up and broadcast), SYN sent and never answered (`retries`
            // climbing), or the state machine stuck somewhere else entirely.
            if let Some(ref c) = CONNECTIONS.lock()[handle] {
                crate::kprintln!(
                    "[tcp] connect timeout: state {:?} arp_pending {} arp_tries {} syn_retries {}",
                    c.state, c.arp_pending, c.arp_tries, c.retries);
            }
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
        srtt_ms: 0,
        rcv_rtt_us8: 0,
        rcvq_copied0: 0,
        copied_total: 0,
        rcvq_time_us: 0,
        // `tcp_init_buffer_space`: start from what a fresh connection offers
        // anyway, `TCP_INIT_CWND * advmss`. Without a start value
        // `rcvbuf_grow` divides by zero.
        rcvq_space: 10 * MSS as u32,
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
        srtt_ms: 0,
        rcv_rtt_us8: 0,
        rcvq_copied0: 0,
        copied_total: 0,
        rcvq_time_us: 0,
        // `tcp_init_buffer_space`: start from what a fresh connection offers
        // anyway, `TCP_INIT_CWND * advmss`. Without a start value
        // `rcvbuf_grow` divides by zero.
        rcvq_space: 10 * MSS as u32,
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
    };
    Ok(())
}

/// Send data on a connection. Buffers and sends immediately (no Nagle).
///
/// The bytes are also kept in `send_buf` until the peer acknowledges them,
/// so `tick_connections` can retransmit. Returns `WouldBlock` when too much
/// is already unacknowledged — that is real backpressure, not an error.
pub fn send(handle: usize, data: &[u8]) -> Result<(), TcpError> {
    let t_enter = crate::interrupts::rdtsc();
    let r = send_inner(handle, data);
    let dt = crate::interrupts::rdtsc().wrapping_sub(t_enter);
    if r.is_err() {
        SEND_WOULDBLOCK.fetch_add(1, Ordering::Relaxed);
        SEND_BLOCKED_TSC.fetch_add(dt, Ordering::Relaxed);
    } else {
        SEND_TSC.fetch_add(dt, Ordering::Relaxed);
    }
    r
}

// ── Where send time goes ─────────────────────────────────────────
//
// Producer-limited or window-limited: `SEND_WOULDBLOCK` tells them apart.
// If it stays zero we never waited for ACKs and the limit is time spent in
// `send_inner`; if it is high, the send buffer limit is the cap.
/// Counts every ACK that freed send-buffer space.
///
/// Lets `send_blocking` wait without spinning on `CONNECTIONS`, the lock
/// the receive path needs to process the very ACK being waited for. The
/// lock-free analogue of Linux `sk_stream_wait_memory` / `sk_write_space`:
/// nothing new is worth trying while it stands still.
pub static ACK_GEN: AtomicU64 = AtomicU64::new(0);

pub static SEND_TSC: AtomicU64 = AtomicU64::new(0);
pub static SEND_BLOCKED_TSC: AtomicU64 = AtomicU64::new(0);
pub static SEND_SEGS: AtomicU64 = AtomicU64::new(0);
pub static SEND_WOULDBLOCK: AtomicU64 = AtomicU64::new(0);
pub static SEND_MAXBUF: AtomicU64 = AtomicU64::new(0);
/// How often the driver queue refused a segment. This is backpressure, not
/// loss; the count says how often it applies.
pub static SEND_REFUSED: AtomicU64 = AtomicU64::new(0);
/// Fast retransmits and RTO expiries. Without these, a too-small window
/// cannot be told apart from a lost segment.
pub static FAST_RETRANS: AtomicU64 = AtomicU64::new(0);
pub static RTO_FIRED: AtomicU64 = AtomicU64::new(0);
/// Cumulative duplicate ACKs, not the connection's live counter, which is
/// reset by every ACK that clears data.
pub static DUPACKS_SEEN: AtomicU64 = AtomicU64::new(0);

/// How far the send buffer has grown (with `snd_wnd_of`, what the peer last
/// offered). Diagnostics: shows whether `tcp_sndbuf_expand` took effect.
pub fn snd_limit_of(handle: usize) -> usize {
    CONNECTIONS.lock()[handle].as_ref().map_or(0, |c| c.snd_buf_limit)
}

/// Bytes currently unacknowledged in the send buffer.
pub fn snd_unacked_of(handle: usize) -> usize {
    CONNECTIONS.lock()[handle].as_ref().map_or(0, |c| c.send_buf.len())
}

/// (cwnd in packets, ssthresh, duplicate ACKs, in recovery)
pub fn cwnd_of(handle: usize) -> (u32, u32, u32, bool) {
    CONNECTIONS.lock()[handle].as_ref().map_or((0, 0, 0, false), |c| {
        (c.snd_cwnd,
         if c.snd_ssthresh == u32::MAX { 0 } else { c.snd_ssthresh },
         c.dupacks, c.in_recovery)
    })
}

pub fn snd_wnd_of(handle: usize) -> usize {
    CONNECTIONS.lock()[handle].as_ref().map_or(0, |c| c.snd_wnd as usize)
}

/// Reset the counters so a measurement sees only its own run.
pub fn send_stats_reset() {
    SEND_TSC.store(0, Ordering::Relaxed);
    SEND_BLOCKED_TSC.store(0, Ordering::Relaxed);
    SEND_SEGS.store(0, Ordering::Relaxed);
    SEND_WOULDBLOCK.store(0, Ordering::Relaxed);
    SEND_MAXBUF.store(0, Ordering::Relaxed);
    SEND_REFUSED.store(0, Ordering::Relaxed);
    FAST_RETRANS.store(0, Ordering::Relaxed);
    RTO_FIRED.store(0, Ordering::Relaxed);
    DUPACKS_SEEN.store(0, Ordering::Relaxed);
}

/// (TSC in send, TSC in refused send, segments, WouldBlock, largest
/// send_buf)
pub fn send_stats() -> (u64, u64, u64, u64, u64) {
    (SEND_TSC.load(Ordering::Relaxed),
     SEND_BLOCKED_TSC.load(Ordering::Relaxed),
     SEND_SEGS.load(Ordering::Relaxed),
     SEND_WOULDBLOCK.load(Ordering::Relaxed),
     SEND_MAXBUF.load(Ordering::Relaxed))
}

fn send_inner(handle: usize, data: &[u8]) -> Result<(), TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;
    if conn.state != State::Established { return Err(TcpError::NotConnected); }
    // An empty buffer always accepts. Otherwise a call larger than the limit
    // could never get through, and a refusal that waiting cannot change is a
    // deadlock.
    if !conn.send_buf.is_empty()
        && conn.send_buf.len() + data.len() > snd_allowed(conn)
    {
        return Err(TcpError::WouldBlock);
    }

    let now = crate::interrupts::ticks();
    // Oldest unacked byte starts its clock now if nothing was in flight.
    if conn.send_buf.is_empty() { conn.rto_tick = now; conn.retries = 0; }
    conn.send_buf.extend_from_slice(data);

    // Send in effective-MSS chunks immediately (no Nagle). Effective, not MSS:
    // the option bytes come out of the same 1514.
    let buf_now = conn.send_buf.len() as u64;
    if buf_now > SEND_MAXBUF.load(Ordering::Relaxed) {
        SEND_MAXBUF.store(buf_now, Ordering::Relaxed);
    }
    write_xmit(conn);

    Ok(())
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
fn write_xmit(conn: &mut TcpConn) {
    let mss = eff_mss(conn).min(1460);
    let mut chunk = [0u8; 1460];
    loop {
        let sent = conn.snd_nxt.wrapping_sub(conn.snd_una) as usize;
        if sent >= conn.send_buf.len() {
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
        let w = recv_window(conn);
        if !send_seg(conn, seq, conn.rcv_nxt, ACK | PSH, w, &chunk[..n]) {
            // Full. Nothing lost, just not sent yet.
            SEND_REFUSED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        SEND_SEGS.fetch_add(1, Ordering::Relaxed);
        conn.snd_nxt = conn.snd_nxt.wrapping_add(n as u32);
        conn.last_send_tick = crate::interrupts::ticks();
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
fn retransmit_head(conn: &mut TcpConn) {
    if conn.send_buf.is_empty() {
        return;
    }
    let mss = eff_mss(conn).min(1460);
    let n = conn.send_buf.len().min(mss);
    let mut chunk = [0u8; 1460];
    chunk[..n].copy_from_slice(&conn.send_buf[..n]);
    let w = recv_window(conn);
    if send_seg(conn, conn.snd_una, conn.rcv_nxt, ACK | PSH, w, &chunk[..n]) {
        conn.last_send_tick = crate::interrupts::ticks();
    } else {
        SEND_REFUSED.fetch_add(1, Ordering::Relaxed);
    }
}

/// `tcp_reno_ssthresh` (tcp_cong.c:512): half, at least two.
fn reno_ssthresh(conn: &TcpConn) -> u32 {
    (conn.snd_cwnd >> 1).max(2)
}

/// Send, waiting out backpressure. Native callers only — the same rule as
/// `connect`: this polls, which pumps the peer fibers on a worker core. A
/// module must handle `WouldBlock` itself and sleep between tries.
pub fn send_blocking(handle: usize, data: &[u8], timeout_ticks: u64) -> Result<(), TcpError> {
    let t0 = crate::interrupts::ticks();
    loop {
        // Read before the attempt, not after: an ACK arriving during `send()`
        // would otherwise be missed, and we would wait for the next one (or
        // the timeout). The classic lost wakeup.
        let zuletzt = ACK_GEN.load(Ordering::Relaxed);
        match send(handle, data) {
            Err(TcpError::WouldBlock) => {}
            other => return other,
        }
        // A second attempt is only worth it once an ACK has made room.
        // Retrying `send()` in a loop would hammer `CONNECTIONS.lock()`, the
        // lock the ACK itself needs.
        //
        // `yield_ready` returns `false` outside a fiber (Core 0, OTA); there
        // we drive the stack ourselves, or the ACK would never arrive.
        //
        // Spin briefly on the atomic counter, then yield: yielding blindly
        // costs a scheduler pass per ACK. The same shape as the driver's
        // receive path.
        let mut leer = 0u32;
        let mut runden = 0u32;
        while ACK_GEN.load(Ordering::Relaxed) == zuletzt {
            if leer < SEND_SPIN_BUDGET {
                leer += 1;
                core::hint::spin_loop();
                continue;
            }
            leer = 0;
            if crate::interrupts::ticks() - t0 > timeout_ticks {
                return Err(TcpError::Timeout);
            }
            // Drive the stack in case nobody else does — on core 0 there
            // is no driver fiber to bring the ACK in.
            super::poll();
            tick_connections();
            crate::smp::fiber::yield_ready();
            // Never wait on the counter forever: a missed wakeup (e.g. a
            // window update that does not bump it) must make the wait slow,
            // not dead, so retry after a bounded number of rounds.
            runden += 1;
            if runden >= RETRY_ROUNDS {
                break;
            }
        }
    }
}

/// Receive data. Returns available data (may be empty if nothing received yet).
/// Sends a window update ACK if significant buffer space was freed.
pub fn recv(handle: usize, buf: &mut [u8]) -> Result<usize, TcpError> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns[handle].as_mut().ok_or(TcpError::NotConnected)?;

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
    rcv_space_adjust(conn);

    // Window-update ACK, rate-limited.
    //
    // We must re-advertise the window the consumer just reopened so a
    // trickle / zero-window sender resumes (e.g. a TLS sender that bursts then
    // goes quiet, leaving the window stuck small). But a bulk plain-http
    // download calls recv() once per packet, so ACKing on every drain would
    // flood the TX path and defeat the handle_tcp ACK coalescing.
    //
    // So: ACK immediately only when the window was actually constrained
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
        // NIC-drain only (the TLS / OTA-https recv hot path). The full
        // super::poll() at spin rate would contend the CONNECTIONS lock with
        // actual packet processing; poll_rx_only throttles the TCP timers.
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
            // `Err(Timeout)`, not `Ok(0)`: `Ok(0)` is how a caller learns the
            // peer hung up, and a link that merely went quiet must not read as
            // end-of-file. A caller that can distinguish the two can wait
            // longer.
            return Err(TcpError::Timeout);
        }
        // Timer-NAPI: HLT instead of spinning, so the core is not pegged.
        // Records the halt so `cores` is accurate. Wakes on the per-core timer;
        // the NIC re-fills the ring in the gap.
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

/// Where this connection goes. For connection coalescing: two names may
/// share a connection only if they lead to the same address.
pub fn peer(handle: usize) -> Option<([u8; 4], u16)> {
    let conns = CONNECTIONS.lock();
    match conns.get(handle) {
        Some(Some(c)) => Some((c.remote_ip, c.remote_port)),
        _ => None,
    }
}

/// Close a connection gracefully (sends FIN) and return at once.
///
/// Linux's `close()` does not wait either: the socket lingers in the
/// background and only `SO_LINGER` — off by default — makes it block.
/// Waiting here would spin on the caller's core for every `Connection: close`
/// peer, and a host call that spins also freezes every other fiber on that
/// worker core.
///
/// The FIN goes out, the slot stays in FinWait1, and `tick_connections`
/// carries it to TimeWait or reaps it if the peer never answers.
pub fn close(handle: usize) -> Result<(), TcpError> {
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

/// Handle incoming TCP segment (called from ipv4)
pub fn handle_tcp(ip_packet: &[u8], data: &[u8]) {
    if data.len() < HEADER_LEN { return; }

    let src_port = u16::from_be_bytes([data[0], data[1]]);
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ack = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let data_offset = ((data[12] >> 4) as usize) * 4;
    let flags = data[13];
    let adv_window = u16::from_be_bytes([data[14], data[15]]);

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
                // The peer's window: flow control per RFC 9293 §3.8.6. The
                // scale is the SYN-ACK's (`snd_wscale`) and applies to every
                // segment except the SYN itself (RFC 7323 §2.2).
                let vorheriges_fenster = conn.snd_wnd;
                conn.snd_wnd = (adv_window as u32) << conn.snd_wscale;
                // Every ACK wakes the sender, not only one that clears data:
                // a pure window update carries `ack == snd_una`, and the
                // sender must notice the reopened window. The wakeup does not
                // claim anything changed, only that a retry might be worth it.
                ACK_GEN.fetch_add(1, Ordering::Relaxed);
                // The RTT sample belongs to the ACK, not to the data: during
                // an upload the peer sends bare ACKs, and without a sample
                // `snd_space_adjust` could never grow the send buffer. Linux
                // samples in `tcp_ack_update_rtt`, called from
                // `tcp_clean_rtx_queue`, where an ACK comes in.
                if conn.ts_ok {
                    if let Some(tsecr) = parse_tsecr(data, data_offset) {
                        if tsecr != 0 {
                            let sample = ts_now_ms().wrapping_sub(tsecr);
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
                    DUPACKS_SEEN.fetch_add(1, Ordering::Relaxed);
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
                        retransmit_head(conn);
                        FAST_RETRANS.fetch_add(1, Ordering::Relaxed);
                    } else if conn.in_recovery {
                        // "Inflate": every further duplicate ACK means a
                        // segment has left the network.
                        conn.snd_cwnd += 1;
                        write_xmit(conn);
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
                    conn.rto_tick = crate::interrupts::ticks();
                    // The send buffer grows here — where Linux calls
                    // `tcp_check_space` -> `tcp_new_space` ->
                    // `tcp_sndbuf_expand`: when an ACK has made room.
                    conn.acked_total =
                        conn.acked_total.wrapping_add(acked as u64);
                    snd_space_adjust(conn);
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
                    write_xmit(conn);
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
                    // Bulk append, not byte-by-byte push_back: extend reserves
                    // once and copies.
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
                    // TSval is `ts_now_ms()`, so now - TSecr = RTT); it feeds DRS
                    // via `rcv_rtt_update`.
                    let delivered = conn.rcv_nxt.wrapping_sub(conn.rcv_irs);
                    ooo_runs_trim(&mut conn.ooo_runs, delivered);
                    if conn.ts_ok {
                        if let Some(tsecr) = parse_tsecr(data, data_offset) {
                            if tsecr != 0 {
                                // In milliseconds. Anything over six
                                // seconds is not an RTT but a clock wrap or
                                // an echo from another connection.
                                let sample = ts_now_ms().wrapping_sub(tsecr);
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
                        // sender fast-retransmits only the hole (RFC 5681) — not the
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
                        // BEHIND = we already have these bytes. Answer with a
                        // D-SACK (RFC 2883): an ACK naming an already
                        // acknowledged range as its first SACK block tells the
                        // sender its retransmit was spurious, so it undoes its
                        // window reduction (Linux: `tcp_dsack_seen` ->
                        // `tcp_undo_cwnd_reduction`). It is not a duplicate ACK
                        // and does not trigger fast retransmit.
                        TCP_OOO_BEHIND.fetch_add(1, Relaxed);
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
                                let w = recv_window(conn);
                                send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, w, &[]);
                                conn.dsack = None;
                                conn.ack_pending = false;
                                conn.acks_held = 0;
                            }
                        }
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

    // Collect the segments to send while holding the lock (they read conn
    // state), then drop the lock and hit the NIC. Holding CONNECTIONS
    // across the TX doorbell would block worker-core `recv` behind Core-0's
    // periodic ACKs/retries.
    let mut pending: alloc::vec::Vec<PendingSeg> = alloc::vec::Vec::new();
    // Same reason: `arp::request` hits the NIC, so collect and fire after the
    // lock is gone.
    let mut arp_probes: alloc::vec::Vec<[u8; 4]> = alloc::vec::Vec::new();
    // Retransmits are not collected here: `retransmit_head` and `write_xmit`
    // send them from the send buffer like any other segment. `retrans` stays
    // empty.
    let retrans: alloc::vec::Vec<(PendingSeg, alloc::vec::Vec<u8>)> =
        alloc::vec::Vec::new();
    {
        let mut conns = CONNECTIONS.lock();
        for slot in conns.iter_mut().flatten() {
            // Send what the driver queue refused earlier. Otherwise it would
            // wait for the caller's next `send`, while the caller waits for
            // an ACK for bytes that never went out.
            if slot.state == State::Established
                && slot.snd_nxt != slot.snd_una.wrapping_add(slot.send_buf.len() as u32)
            {
                write_xmit(slot);
            }
            // Delayed ACK
            if slot.ack_pending && now - slot.ack_tick >= DELAYED_ACK_TICKS {
                let w = recv_window(slot);
                let mut opts = [0u8; 40];
                let len = build_seg_opts(slot, ACK, &mut opts, false);
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
                        RTO_FIRED.fetch_add(1, Ordering::Relaxed);
                        retransmit_head(slot);
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
/// Shared by the first SYN and every retransmit: a retry with a bare SYN
/// would bring the connection up without window scaling, SACK or timestamps,
/// capped at a 64 KiB window for its whole life.
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
    let tsval = ts_now_ms();
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
) -> bool {
    send_segment_with_opts(dst_ip, src_port, dst_port, seq, ack, flags, window, payload, &[])
}

fn send_segment_with_opts(
    dst_ip: [u8; 4], src_port: u16, dst_port: u16,
    seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8], options: &[u8],
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
    let src_ip = arp::our_ip();
    let checksum = tcp_checksum(&src_ip, &dst_ip, &pkt);
    pkt[16..18].copy_from_slice(&checksum.to_be_bytes());

    // The result matters: `netdev::send` refuses when the driver queue is
    // full, and the caller must not treat the segment as sent.
    ipv4::send(dst_ip, ipv4::PROTO_TCP, &pkt)
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

/// Payload per segment for this connection. `MSS` is the wire budget; what is
/// left for data is that minus the options every segment carries.
fn eff_mss(conn: &TcpConn) -> usize {
    (MSS as usize).saturating_sub(data_opts_len(conn)).max(1)
}

/// Build the TCP option list (Timestamp, then SACK blocks during a gap;
/// both 4-byte aligned via leading NOPs) for `conn`/`flags` into `opts`,
/// returning its length. Shared by the inline `send_seg` and the deferred
/// tick path, which materializes segments under the CONNECTIONS lock and
/// sends them after dropping it.
fn build_seg_opts(conn: &TcpConn, flags: u8, opts: &mut [u8; 40], has_payload: bool) -> usize {
    let mut len = 0;
    if conn.ts_ok {
        opts[len] = 1; opts[len + 1] = 1;          // NOP, NOP
        opts[len + 2] = 8; opts[len + 3] = 10;     // Timestamp kind, len
        let tsval = ts_now_ms();
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
fn send_seg(conn: &TcpConn, seq: u32, ack: u32, flags: u8, window: u16,
            payload: &[u8]) -> bool {
    TCP_TX_SEGS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let mut opts = [0u8; 40];
    let len = build_seg_opts(conn, flags, &mut opts, !payload.is_empty());
    if len > 0 {
        send_segment_with_opts(conn.remote_ip, conn.local_port, conn.remote_port,
            seq, ack, flags, window, payload, &opts[..len])
    } else {
        send_segment(conn.remote_ip, conn.local_port, conn.remote_port,
            seq, ack, flags, window, payload)
    }
}

/// A fully-resolved zero-payload segment captured under the CONNECTIONS
/// lock so it can be sent (the NIC doorbell) after the lock is dropped.
/// Keeps `tick_connections` from holding the lock across TX, which would block
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

/// Does any connection run a timer (retransmit, delayed ACK, SYN retry,
/// TIME_WAIT, FIN timeout)? Listening and closed sockets have none. The
/// shell loop drives `tick_connections` and must keep coming back while
/// this holds.
pub fn has_timers() -> bool {
    CONNECTIONS.lock().iter().flatten()
        .any(|c| !matches!(c.state, State::Closed | State::Listen))
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
