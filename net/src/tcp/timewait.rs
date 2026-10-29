//! TIME-WAIT, after Linux 6.18 `tcp_minisocks.c`: once a closed
//! connection waits only for stray segments, a compact
//! `inet_timewait_sock` takes its place for 60 s (`tcp_time_wait`), and
//! `tcp_timewait_state_process` answers what comes: ACKs out-of-window
//! segments, restarts the wait on a retransmitted FIN, lets a newer SYN
//! open the 4-tuple again, and applies PAWS. An orphaned FIN-WAIT-2 waits
//! here too, for the peer's FIN or `tcp_fin_timeout`.

use super::*;

/// `TCP_TIMEWAIT_LEN`: 60 s, in ticks.
pub(super) const TCP_TIMEWAIT_LEN: u64 = 60 * 100;
/// `TCP_PAWS_MSL`: a RST older than its timestamp passes after this (s).
const TCP_PAWS_MSL: u64 = 60;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Sub { FinWait2, TimeWait }

/// `tcp_timewait_sock`.
pub(super) struct Tw {
    pub(super) sub: Sub,
    local_ip: [u8; 4],
    rcv_nxt: u32,
    snd_nxt: u32,
    /// `tw_rcv_wnd` in bytes and our window scale.
    rcv_wnd: u32,
    rcv_wscale: u8,
    ts_recent: u32,
    /// Seconds; 0 without timestamps.
    ts_recent_stamp: u64,
    /// The tick the wait ends.
    pub(super) expires: u64,
    last_oow_ack: u64,
}

/// What the segment leads to (`enum tcp_tw_status`).
pub(super) enum Verdict {
    /// Nothing to send.
    Success,
    /// Kill the entry.
    Kill,
    Ack,
    /// Reset, and kill the entry.
    Rst,
    /// A new connection may start, with this ISN.
    Syn(u32),
}

/// `tcp_time_wait`: the entry replacing `c`, for `timeo` ticks (60 s in
/// TIME-WAIT; for FIN-WAIT-2 at least 3.5 RTOs).
pub(super) fn from_conn(c: &TcpConn, sub: Sub, timeo: u64, now: u64) -> Tw {
    let rto = (c.rto << 2) - (c.rto >> 1);
    let timeo = if sub == Sub::TimeWait { TCP_TIMEWAIT_LEN } else { timeo.max(rto) };
    Tw {
        sub,
        local_ip: c.local_ip,
        rcv_nxt: c.rcv_nxt,
        snd_nxt: c.snd_nxt,
        rcv_wnd: c.receive_window(),
        rcv_wscale: if c.wscale_ok { OUR_WSCALE } else { 0 },
        ts_recent: c.ts_recent,
        ts_recent_stamp: if c.ts_ok { c.ts_recent_stamp.max(1) } else { 0 },
        expires: now + timeo,
        last_oow_ack: 0,
    }
}

pub(super) fn name(tw: &Tw) -> &'static str {
    match tw.sub { Sub::FinWait2 => "FIN_WAIT_2", Sub::TimeWait => "TIME_WAIT" }
}

/// `tcp_fin_time`: `tcp_fin_timeout`, but no less than 3.5 RTOs.
pub(super) fn fin_time(c: &TcpConn) -> u64 {
    let rto = c.rto;
    TCP_TIMEWAIT_LEN.max((rto << 2) - (rto >> 1))
}

fn before(a: u32, b: u32) -> bool { (a.wrapping_sub(b) as i32) < 0 }
fn after(a: u32, b: u32) -> bool { before(b, a) }

/// `tcp_in_window`.
fn in_window(seq: u32, end_seq: u32, s_win: u32, e_win: u32) -> bool {
    if seq == s_win { return true; }
    if after(end_seq, s_win) && before(seq, e_win) { return true; }
    seq == e_win && seq == end_seq
}

/// `tcp_timewait_check_oow_rate_limit`: answers to segments without data
/// at most one per 500 ms.
fn oow_rate_limit(tw: &mut Tw, has_data: bool, now: u64) -> Verdict {
    if !has_data && tw.last_oow_ack != 0 && now.saturating_sub(tw.last_oow_ack) < TCP_INVALID_RATELIMIT {
        return Verdict::Success;
    }
    if !has_data { tw.last_oow_ack = now.max(1); }
    Verdict::Ack
}

/// `tcp_timewait_state_process`.
#[allow(clippy::too_many_arguments)]
pub(super) fn state_process(tw: &mut Tw, flags: u8, seq: u32, end_seq: u32, has_data: bool,
                            tsval: Option<u32>, now: u64, now_s: u64) -> Verdict {
    let rcv_nxt = tw.rcv_nxt;
    let rst = flags & RST != 0;
    // PAWS against the timestamp the connection last took.
    let tsval = tsval.filter(|_| tw.ts_recent_stamp != 0);
    let paws_reject = tsval.is_some_and(|v| {
        let ok = (tw.ts_recent.wrapping_sub(v) as i32) <= 0
            || now_s >= tw.ts_recent_stamp + TCP_PAWS_WRAP
            || tw.ts_recent == 0;
        !ok && !(rst && now_s >= tw.ts_recent_stamp + TCP_PAWS_MSL)
    });

    if tw.sub == Sub::FinWait2 {
        // The checks of tcp_rcv_state_process once more.
        if paws_reject || !in_window(seq, end_seq, rcv_nxt, rcv_nxt.wrapping_add(tw.rcv_wnd)) {
            return oow_rate_limit(tw, has_data, now);
        }
        if rst { return Verdict::Kill; }
        if flags & SYN != 0 && !before(seq, rcv_nxt) { return Verdict::Rst; }
        // A duplicate ACK.
        if flags & ACK == 0 || !after(end_seq, rcv_nxt) || end_seq == seq { return Verdict::Success; }
        // Data after our side closed resets.
        if flags & FIN == 0 || end_seq != rcv_nxt.wrapping_add(1) { return Verdict::Rst; }
        // The FIN: TIME-WAIT proper.
        tw.sub = Sub::TimeWait;
        tw.rcv_nxt = end_seq;
        if let Some(v) = tsval {
            tw.ts_recent = v;
            tw.ts_recent_stamp = now_s.max(1);
        }
        tw.expires = now + TCP_TIMEWAIT_LEN;
        return Verdict::Ack;
    }

    // In window: only a bare ACK or a reset can be. A reset ends the
    // wait (RFC 1337 is not applied, as in Linux by default).
    if !paws_reject && seq == rcv_nxt && (seq == end_seq || rst) {
        if rst { return Verdict::Kill; }
        tw.expires = now + TCP_TIMEWAIT_LEN;
        if let Some(v) = tsval {
            tw.ts_recent = v;
            tw.ts_recent_stamp = now_s.max(1);
        }
        return Verdict::Success;
    }

    // A new SYN may reopen the 4-tuple (RFC 1122 §4.2.2.13) if it is not
    // an old duplicate: later in sequence or by timestamp. Its ISN lies
    // past the old connection's.
    if flags & SYN != 0 && !rst && flags & ACK == 0 && !paws_reject
        && (after(seq, rcv_nxt) || tsval.is_some_and(|v| (tw.ts_recent.wrapping_sub(v) as i32) < 0))
    {
        let mut isn = tw.snd_nxt.wrapping_add(65535 + 2);
        if isn == 0 { isn = 1; }
        return Verdict::Syn(isn);
    }

    if !rst {
        // Everything else is ACKed, and restarts the wait unless it may
        // be a new SYN with a random sequence number.
        if paws_reject || flags & ACK != 0 { tw.expires = now + TCP_TIMEWAIT_LEN; }
        return oow_rate_limit(tw, has_data, now);
    }
    Verdict::Success
}

/// `tcp_v4_timewait_ack`: an ACK from what the connection left.
pub(super) fn send_ack<E: Env + ?Sized>(io: &mut Io<E>, tw: &Tw, (local_port, remote_ip, remote_port): Tuple) {
    if tw.local_ip != io.env.our_ip() { return; }
    let w = (tw.rcv_wnd >> tw.rcv_wscale).min(u16::MAX as u32) as u16;
    let mut opts = [0u8; 12];
    let len = if tw.ts_recent_stamp != 0 {
        opts[..4].copy_from_slice(&[1, 1, 8, 10]);
        opts[4..8].copy_from_slice(&io.ts_now_ms().to_be_bytes());
        opts[8..12].copy_from_slice(&tw.ts_recent.to_be_bytes());
        12
    } else {
        0
    };
    send_segment_with_opts(io, remote_ip, local_port, remote_port, tw.snd_nxt, tw.rcv_nxt, ACK, w,
        &[], &opts[..len], 0);
}
