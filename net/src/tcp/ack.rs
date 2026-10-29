//! When the receiver ACKs, after Linux 6.18: `icsk_ack` with quick-ACK
//! and interactive (pingpong) modes and the adaptive ACK timeout
//! (`tcp_event_data_recv`, `tcp_measure_rcv_mss`), `__tcp_ack_snd_check`
//! (an ACK for every second full segment, RFC 5681 §4.2), the delayed-ACK
//! timer (`tcp_send_delayed_ack`, `tcp_delack_timer_handler`), SACK
//! compression while out-of-order data waits, and the ACK a read earns
//! (`__tcp_cleanup_rbuf`).
//!
//! The SACK compression timer runs on the 10 ms tick, where Linux uses a
//! high-resolution timer of at most 1 ms.

use super::*;

/// `ICSK_ACK_*`.
const ACK_SCHED: u8 = 1;
const ACK_TIMER: u8 = 2;
const ACK_PUSHED: u8 = 4;
const ACK_PUSHED2: u8 = 8;
pub(super) const ACK_NOW: u8 = 16;
pub(super) const ACK_NOMEM: u8 = 32;

/// `TCP_ATO_MIN`, `TCP_DELACK_MIN`, `TCP_DELACK_MAX` at HZ = 100.
const TCP_ATO_MIN: u64 = 4;
const TCP_DELACK_MIN: u64 = 4;
const TCP_DELACK_MAX: u64 = 20;
/// `TCP_MAX_QUICKACKS`.
pub(super) const TCP_MAX_QUICKACKS: u32 = 16;
/// `TCP_FASTRETRANS_THRESH`.
const TCP_FASTRETRANS_THRESH: u32 = 3;
/// `sysctl_tcp_comp_sack_nr`, `sysctl_tcp_comp_sack_delay_ns` (in µs).
const TCP_COMP_SACK_NR: u32 = 44;
const TCP_COMP_SACK_DELAY_US: u64 = 1000;
/// `sysctl_tcp_pingpong_thresh`.
const TCP_PINGPONG_THRESH: u8 = 1;
/// `TCPOLEN_TSTAMP_ALIGNED`.
const TCPOLEN_TSTAMP_ALIGNED: usize = 12;

/// `icsk_ack` and the SACK compression state of `tcp_sock`.
#[derive(Default)]
pub(super) struct AckState {
    /// `ICSK_ACK_*` bits.
    pub(super) pending: u8,
    /// ACKs still to send at once.
    quick: u32,
    pingpong: u8,
    /// The delayed-ACK timeout, in ticks (0 until the first data).
    ato: u64,
    /// When the delayed-ACK timer fires.
    timeout: u64,
    /// When data last came.
    lrcvtime: u64,
    last_seg_size: usize,
    /// ACKs held back while out-of-order data waits, the `rcv_nxt` they
    /// were for, the duplicates already sent for it, and when the held
    /// one goes (0: no timer).
    compressed_ack: u32,
    compressed_ack_rcv_nxt: u32,
    dup_ack_counter: u32,
    comp_at: u64,
}

pub(super) fn schedule_ack(c: &mut TcpConn) { c.ack.pending |= ACK_SCHED; }

pub(super) fn ack_scheduled(c: &TcpConn) -> bool { c.ack.pending & ACK_SCHED != 0 }

fn in_pingpong_mode(c: &TcpConn) -> bool { c.ack.pingpong >= TCP_PINGPONG_THRESH }

/// `inet_csk_enter_pingpong_mode`: the session looks interactive.
pub(super) fn enter_pingpong_mode(c: &mut TcpConn) { c.ack.pingpong = TCP_PINGPONG_THRESH; }

/// `inet_csk_inc_pingpong_cnt`, from `tcp_event_data_sent`: we answer
/// within the ACK timeout of the last data received.
pub(super) fn data_sent(c: &mut TcpConn, now: u64) {
    if now.saturating_sub(c.ack.lrcvtime) < c.ack.ato {
        c.ack.pingpong = c.ack.pingpong.saturating_add(1);
    }
}

/// `tcp_incr_quickack`: as many quick ACKs as half the window holds
/// segments, at most `max`.
fn incr_quickack(c: &mut TcpConn, max: u32) {
    let mut quickacks = c.rcv_wnd.get() / (2 * c.rcv_mss.max(1) as u32);
    if quickacks == 0 { quickacks = 2; }
    quickacks = quickacks.min(max);
    if quickacks > c.ack.quick { c.ack.quick = quickacks; }
}

/// `tcp_enter_quickack_mode`.
pub(super) fn enter_quickack_mode(c: &mut TcpConn, max: u32) {
    incr_quickack(c, max);
    c.ack.pingpong = 0;
    c.ack.ato = TCP_ATO_MIN;
}

/// `tcp_in_quickack_mode`.
fn in_quickack_mode(c: &TcpConn) -> bool {
    c.ack.quick != 0 && !in_pingpong_mode(c)
}

/// `tcp_measure_rcv_mss`: the peer's segment size, for "two full
/// segments". `len` is the payload, `doff` the header with options.
fn measure_rcv_mss(c: &mut TcpConn, len: usize, doff: usize, flags: u8) {
    let lss = c.ack.last_seg_size;
    c.ack.last_seg_size = 0;
    if len >= c.rcv_mss as usize {
        c.rcv_mss = (len as u16).min(c.advmss);
        // A full segment with PSH likely ends a write: ACK once the
        // application has read it.
        if flags & PSH != 0 { c.ack.pending |= ACK_PUSHED; }
    } else {
        // Header included, so variable SACK options do not count.
        let mut len = len + doff;
        let remnant = FIN | SYN | RST | PSH;
        if len >= TCP_MSS_DEFAULT as usize + HEADER_LEN
            || (len >= TCP_MIN_MSS as usize + HEADER_LEN && flags & remnant == 0)
        {
            len -= HEADER_LEN + if c.ts_ok { TCPOLEN_TSTAMP_ALIGNED } else { 0 };
            c.ack.last_seg_size = len;
            if len == lss {
                c.rcv_mss = len as u16;
                return;
            }
        }
        if c.ack.pending & ACK_PUSHED != 0 { c.ack.pending |= ACK_PUSHED2; }
        c.ack.pending |= ACK_PUSHED;
    }
}

/// `tcp_event_data_recv`: data came; adapt the ACK timeout to the gaps
/// between segments.
pub(super) fn event_data_recv(c: &mut TcpConn, len: usize, doff: usize, flags: u8, now: u64) {
    schedule_ack(c);
    measure_rcv_mss(c, len, doff, flags);
    if c.ack.ato == 0 {
        // The first data: start in quick-ACK mode.
        incr_quickack(c, TCP_MAX_QUICKACKS);
        c.ack.ato = TCP_ATO_MIN;
    } else {
        let m = now.saturating_sub(c.ack.lrcvtime);
        if m <= TCP_ATO_MIN / 2 {
            c.ack.ato = (c.ack.ato >> 1) + TCP_ATO_MIN / 2;
        } else if m < c.ack.ato {
            c.ack.ato = ((c.ack.ato >> 1) + m).min(c.rto).min(TCP_DELACK_MAX);
        } else if m > c.rto {
            // A long gap: the sender failed to restart its window.
            incr_quickack(c, TCP_MAX_QUICKACKS);
        }
    }
    c.ack.lrcvtime = now;
}

/// `tcp_event_ack_sent` with `tcp_dec_quickack_mode`: an ACK for
/// `rcv_nxt` left, so nothing is pending any more.
pub(super) fn event_ack_sent(c: &mut TcpConn) {
    // A D-SACK goes out once.
    c.dsack = None;
    c.ack.compressed_ack = 0;
    c.ack.comp_at = 0;
    if c.ack.quick != 0 {
        let pkts = if ack_scheduled(c) { 1 } else { 0 };
        if pkts >= c.ack.quick {
            c.ack.quick = 0;
            c.ack.ato = TCP_ATO_MIN;
        } else {
            c.ack.quick -= pkts;
        }
    }
    c.ack.pending = 0;
}

/// `tcp_send_ack`.
pub(super) fn send_ack<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>) {
    let w = recv_window(c, io.st);
    send_seg(c, io, c.snd_nxt, c.rcv_nxt, ACK, w, &[]);
}

/// `tcp_send_delayed_ack`.
fn send_delayed_ack<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>) {
    let now = io.ticks();
    let mut ato = c.ack.ato;
    if ato > TCP_DELACK_MIN {
        let mut max_ato = 100 / 2;
        if in_pingpong_mode(c) || c.ack.pending & ACK_PUSHED != 0 { max_ato = TCP_DELACK_MAX; }
        // Bounded by the RTT where one is known.
        if c.srtt_us != 0 {
            let rtt = ((c.srtt_us >> 3) as u64).div_ceil(TICK_US).max(TCP_DELACK_MIN);
            if rtt < max_ato { max_ato = rtt; }
        }
        ato = ato.min(max_ato);
    }
    ato = ato.min(TCP_DELACK_MAX);
    let mut timeout = now + ato;
    if c.ack.pending & ACK_TIMER != 0 {
        // About to fire anyway: ACK now.
        if c.ack.timeout <= now + (ato >> 2) {
            send_ack(c, io);
            return;
        }
        if timeout >= c.ack.timeout { timeout = c.ack.timeout; }
    }
    c.ack.pending |= ACK_SCHED | ACK_TIMER;
    c.ack.timeout = timeout;
}

/// `tcp_ack_snd_check` / `__tcp_ack_snd_check`: ACK now, later, or held
/// while out-of-order data waits.
pub(super) fn ack_snd_check<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>, ofo_possible: bool) {
    if !ack_scheduled(c) { return; }
    let unread = c.recv_buf.len();
    // More than one full segment, and the window's right edge moves far
    // enough; or quick-ACK mode; or a one-time immediate ACK.
    if (c.rcv_nxt.wrapping_sub(c.rcv_wup.get()) > c.rcv_mss as u32
        && (unread == 0 || select_window_raw(c, io.st) >= c.rcv_wnd.get()))
        || in_quickack_mode(c)
        || c.ack.pending & ACK_NOW != 0
    {
        send_ack(c, io);
        return;
    }
    if !ofo_possible || c.ooo.is_empty() {
        send_delayed_ack(c, io);
        return;
    }
    if !c.sack_ok || c.ack.compressed_ack >= TCP_COMP_SACK_NR {
        send_ack(c, io);
        return;
    }
    // The first duplicates go at once for the sender's fast retransmit;
    // the rest are held for a short while.
    if c.ack.compressed_ack_rcv_nxt != c.rcv_nxt {
        c.ack.compressed_ack_rcv_nxt = c.rcv_nxt;
        c.ack.dup_ack_counter = 0;
    }
    if c.ack.dup_ack_counter < TCP_FASTRETRANS_THRESH {
        c.ack.dup_ack_counter += 1;
        send_ack(c, io);
        return;
    }
    c.ack.compressed_ack += 1;
    if c.ack.comp_at != 0 { return; }
    // 5 % of the RTT, at most 1 ms.
    let mut rtt = (c.rcv_rtt_us8 >> 3) as u64;
    if c.srtt_us != 0 && ((c.srtt_us >> 3) as u64) < rtt { rtt = (c.srtt_us >> 3) as u64; }
    let delay_us = TCP_COMP_SACK_DELAY_US.min(rtt / 20);
    c.ack.comp_at = io.ticks() + us_to_ticks(delay_us).max(1);
}

fn us_to_ticks(us: u64) -> u64 { us.div_ceil(TICK_US) }

/// The ACK timers on a tick: SACK compression (`tcp_compressed_ack_kick`)
/// and the delayed ACK (`tcp_delack_timer_handler`). True when an ACK is
/// due now.
pub(super) fn timer_due(c: &mut TcpConn, now: u64) -> bool {
    if c.ack.comp_at != 0 && now >= c.ack.comp_at {
        c.ack.comp_at = 0;
        if c.ack.compressed_ack != 0 {
            c.ack.compressed_ack -= 1;
            return true;
        }
    }
    if c.ack.pending & ACK_TIMER == 0 || now < c.ack.timeout { return false; }
    c.ack.pending &= !ACK_TIMER;
    if !ack_scheduled(c) { return false; }
    if !in_pingpong_mode(c) {
        // The delayed ACK was missed: a longer timeout.
        c.ack.ato = (c.ack.ato << 1).min(c.rto).min(TCP_DELACK_MAX);
    } else {
        c.ack.pingpong = 0;
        c.ack.ato = TCP_ATO_MIN;
    }
    true
}

/// `__tcp_cleanup_rbuf`: after the application read `copied` bytes.
pub(super) fn cleanup_rbuf<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>, copied: usize) {
    let mut time_to_ack = false;
    if ack_scheduled(c) {
        // The every-second-segment ACK input did not send, or a read that
        // emptied the buffer after a small segment of a one-way flow.
        if c.rcv_nxt.wrapping_sub(c.rcv_wup.get()) > c.rcv_mss as u32
            || (copied > 0
                && (c.ack.pending & ACK_PUSHED2 != 0
                    || (c.ack.pending & ACK_PUSHED != 0 && !in_pingpong_mode(c)))
                && c.recv_buf.is_empty())
        {
            time_to_ack = true;
        }
    }
    // A window at least doubled is worth announcing, unless no more data
    // can come.
    if copied > 0 && !time_to_ack && !c.closed {
        let now_win = c.receive_window();
        if 2 * now_win as usize <= full_space(c, io.st) {
            let new = select_window_raw(c, io.st);
            if new != 0 && new >= 2 * now_win { time_to_ack = true; }
        }
    }
    if time_to_ack { send_ack(c, io); }
}
