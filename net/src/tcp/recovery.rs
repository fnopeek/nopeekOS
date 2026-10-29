//! The sender's side of an ACK and its loss recovery, after Linux 6.18
//! `tcp_input.c`, `tcp_recovery.c` and `tcp_output.c`: the retransmit
//! queue and its scoreboard (`tcp_sacktag_write_queue`,
//! `tcp_clean_rtx_queue`), RACK loss detection (RFC 8985), NewReno without
//! SACK (RFC 6582), the congestion state machine (`tcp_fastretrans_alert`),
//! Proportional Rate Reduction (RFC 6937, `tcp_cwnd_reduction`), the
//! retransmissions they lead to (`tcp_xmit_retransmit_queue`), and the
//! window after idle and while the application sends less
//! (`tcp_cwnd_restart`, `tcp_cwnd_application_limited`). The window itself
//! grows and shrinks by CUBIC (`cubic.rs`).
//!
//! A reduction found needless is taken back (`tcp_try_undo_*`): by D-SACKs
//! for every retransmission, by an echoed timestamp older than the first
//! one, or by F-RTO after a timeout (RFC 5682).
//!
//! A tail loss shows itself by a probe two RTTs after the last send
//! (TLP, RFC 8985 §7) instead of the RTO.
//!
//! Each ACK also yields a delivery rate sample (`tcp_rate.c`), from which
//! RACK widens its reordering window after D-SACKs.
//!
//! Every segment is one skb with `tcp_skb_pcount` 1.

use super::*;
use super::{ack, cubic};

/// `TCPCB_SACKED_ACKED`, `TCPCB_SACKED_RETRANS`, `TCPCB_LOST`,
/// `TCPCB_EVER_RETRANS`.
const SACKED_ACKED: u8 = 0x01;
const SACKED_RETRANS: u8 = 0x02;
const LOST: u8 = 0x04;
const EVER_RETRANS: u8 = 0x80;
const RETRANS: u8 = SACKED_RETRANS | EVER_RETRANS;

const FLAG_DATA: u32 = 0x01;
const FLAG_WIN_UPDATE: u32 = 0x02;
const FLAG_DATA_ACKED: u32 = 0x04;
const FLAG_RETRANS_DATA_ACKED: u32 = 0x08;
const FLAG_DATA_SACKED: u32 = 0x20;
const FLAG_LOST_RETRANS: u32 = 0x80;
const FLAG_SND_UNA_ADVANCED: u32 = 0x400;
const FLAG_DSACKING_ACK: u32 = 0x800;
const FLAG_SET_XMIT_TIMER: u32 = 0x1000;
const FLAG_ORIG_SACK_ACKED: u32 = 0x200;
const FLAG_SACK_RENEGING: u32 = 0x2000;
const FLAG_ACK_MAYBE_DELAYED: u32 = 0x10000;
const FLAG_DSACK_TLP: u32 = 0x20000;
const FLAG_ACKED: u32 = FLAG_DATA_ACKED;
const FLAG_NOT_DUP: u32 = FLAG_DATA | FLAG_WIN_UPDATE | FLAG_ACKED;
const FLAG_CA_ALERT: u32 = FLAG_DATA_SACKED | FLAG_DSACKING_ACK;
const FLAG_FORWARD_PROGRESS: u32 = FLAG_ACKED | FLAG_DATA_SACKED;

/// `sysctl_tcp_reordering`, `sysctl_tcp_max_reordering`.
const TCP_REORDERING: u32 = 3;
const TCP_MAX_REORDERING: u32 = 300;
/// `TCP_NUM_SACKS`.
const TCP_NUM_SACKS: usize = 4;
/// `TCP_TIMEOUT_MIN_US`.
const TCP_TIMEOUT_MIN_US: u64 = 2000;
/// `TCP_RTO_MIN` in µs.
const TCP_RTO_MIN_US: u64 = 200_000;
/// `TCP_RACK_RECOVERY_THRESH`.
const TCP_RACK_RECOVERY_THRESH: u32 = 16;
/// `sysctl_tcp_min_rtt_wlen`: 300 s, in ticks.
const MIN_RTT_WLEN: u32 = 300 * 100;

fn before(a: u32, b: u32) -> bool { (a.wrapping_sub(b) as i32) < 0 }
fn after(a: u32, b: u32) -> bool { before(b, a) }

/// `usecs_to_jiffies`: rounded up.
fn us_to_ticks(us: u64) -> u64 { us.div_ceil(TICK_US) }

/// One segment in flight: an skb of the retransmit queue.
#[derive(Clone, Copy)]
pub(super) struct TxSeg {
    seq: u32,
    /// Past the FIN when it carries one.
    end: u32,
    fin: bool,
    /// `TCPCB_*` tag bits.
    sacked: u8,
    /// `skb_mstamp`: when it last went out, in µs.
    sent_us: u64,
    /// `tx`: the connection's delivery state when it went out
    /// (`tcp_rate_skb_sent`); `delivered_mstamp` 0 once counted.
    tx_delivered: u32,
    tx_delivered_mstamp: u64,
    tx_first_tx_mstamp: u64,
    tx_app_limited: bool,
}

impl TxSeg {
    fn len(&self) -> usize { self.end.wrapping_sub(self.seq) as usize - self.fin as usize }
}

/// `struct minmax`, `lib/win_minmax.c`: the windowed minimum of the RTT.
#[derive(Clone, Copy)]
struct MinMax { s: [(u32, u32); 3] }

impl MinMax {
    fn reset(&mut self, t: u32, v: u32) -> u32 {
        self.s = [(t, v); 3];
        v
    }

    fn subwin_update(&mut self, win: u32, val: (u32, u32)) -> u32 {
        let dt = val.0.wrapping_sub(self.s[0].0);
        if dt > win {
            self.s[0] = self.s[1];
            self.s[1] = self.s[2];
            self.s[2] = val;
            if val.0.wrapping_sub(self.s[0].0) > win {
                self.s[0] = self.s[1];
                self.s[1] = self.s[2];
                self.s[2] = val;
            }
        } else if self.s[1].0 == self.s[0].0 && dt > win / 4 {
            self.s[1] = val;
            self.s[2] = val;
        } else if self.s[2].0 == self.s[1].0 && dt > win / 2 {
            self.s[2] = val;
        }
        self.s[0].1
    }

    /// `minmax_running_min`.
    fn running_min(&mut self, win: u32, t: u32, v: u32) -> u32 {
        let val = (t, v);
        if v <= self.s[0].1 || t.wrapping_sub(self.s[2].0) > win {
            return self.reset(t, v);
        }
        if v <= self.s[1].1 {
            self.s[1] = val;
            self.s[2] = val;
        } else if v <= self.s[2].1 {
            self.s[2] = val;
        }
        self.subwin_update(win, val)
    }
}

/// `tp->rack`.
struct Rack {
    /// The most recent (re)transmission among (s)acked segments, and its
    /// end.
    mstamp: u64,
    end_seq: u32,
    rtt_us: u64,
    advanced: bool,
    reo_wnd_steps: u32,
    /// `tp->delivered` when the window last grew; a D-SACK is waiting; the
    /// recoveries the grown window still holds for.
    last_delivered: u32,
    dsack_seen: bool,
    reo_wnd_persist: u32,
}

/// The sender's `tcp_sock` fields recovery keeps.
pub(super) struct Tx {
    /// `tcp_rtx_queue`: what was sent and not yet acknowledged, in order.
    queue: VecDeque<TxSeg>,
    pub(super) packets_out: u32,
    pub(super) sacked_out: u32,
    lost_out: u32,
    retrans_out: u32,
    /// `tp->delivered`, `tp->lost`: running totals.
    delivered: u32,
    lost: u32,
    reordering: u32,
    reord_seen: u32,
    total_retrans: u32,
    dsack_dups: u32,
    /// The end of the highest SACKed segment (`tcp_highest_sack_seq` while
    /// `sacked_out` is not 0).
    highest_sack: u32,
    prior_cwnd: u32,
    prr_delivered: u32,
    prr_out: u32,
    is_cwnd_limited: bool,
    max_packets_out: u32,
    cwnd_usage_seq: u32,
    rack: Rack,
    rtt_min: MinMax,
    /// `tcp_rate.c`: when the current sending window began, the last
    /// delivery, the `delivered` count that ends an application-limited
    /// stretch (0: none), and the last rate sample kept.
    first_tx_mstamp: u64,
    delivered_mstamp: u64,
    app_limited: u32,
    pub(super) rate_delivered: u32,
    pub(super) rate_interval_us: u64,
    rate_app_limited: bool,
    /// `is_sack_reneg`: the receiver dropped SACKed data, until Open.
    is_sack_reneg: bool,
    /// `icsk_pending`: what the retransmit timer (`rtx_at`) is armed for.
    pub(super) pending: Pending,
    /// `tlp_high_seq`: `snd_nxt` when the loss probe went out; `tlp_retrans`:
    /// the probe was a retransmission.
    tlp_high_seq: Option<u32>,
    tlp_retrans: bool,
    /// `SOCK_NOSPACE`: a send found the buffer full.
    pub(super) nospace: bool,
    /// `undo_marker`: `snd_una` when the reduction began that may still be
    /// taken back; `undo_retrans` the retransmissions not yet shown needless
    /// by a D-SACK (-1: none sent); `prior_ssthresh` what to go back to.
    undo_marker: Option<u32>,
    undo_retrans: i32,
    prior_ssthresh: u32,
    /// `tp->frto`: F-RTO runs for this timeout.
    frto: bool,
    /// The echoed timestamp of the ACK being processed (`rcv_tsecr` with
    /// `saw_tstamp`).
    rcv_tsecr: Option<u32>,
    /// `lsndtime`: the tick data last went out.
    pub(super) lsndtime: u64,
    /// `snd_cwnd_used`, `snd_cwnd_stamp`: the most in flight while the
    /// window was not filled, and when cwnd last changed or was full.
    snd_cwnd_used: u32,
    snd_cwnd_stamp: u64,
    pub(super) cubic: cubic::Cubic,
}

impl Tx {
    pub(super) fn new() -> Self {
        Tx {
            queue: VecDeque::new(),
            packets_out: 0, sacked_out: 0, lost_out: 0, retrans_out: 0,
            delivered: 0, lost: 0,
            reordering: TCP_REORDERING, reord_seen: 0, total_retrans: 0, dsack_dups: 0,
            highest_sack: 0,
            prior_cwnd: 0, prr_delivered: 0, prr_out: 0,
            is_cwnd_limited: false, max_packets_out: 0, cwnd_usage_seq: 0,
            rack: Rack {
                mstamp: 0, end_seq: 0, rtt_us: 0, advanced: false, reo_wnd_steps: 1,
                last_delivered: 0, dsack_seen: false, reo_wnd_persist: 0,
            },
            first_tx_mstamp: 0,
            delivered_mstamp: 0,
            app_limited: u32::MAX,
            rate_delivered: 0,
            rate_interval_us: 0,
            rate_app_limited: true,
            is_sack_reneg: false,
            rtt_min: MinMax { s: [(0, u32::MAX); 3] },
            pending: Pending::Retrans,
            tlp_high_seq: None,
            tlp_retrans: false,
            nospace: false,
            undo_marker: None,
            undo_retrans: -1,
            prior_ssthresh: 0,
            frto: false,
            rcv_tsecr: None,
            lsndtime: 0,
            snd_cwnd_used: 0,
            snd_cwnd_stamp: 0,
            cubic: cubic::Cubic::default(),
        }
    }

    /// `tcp_packets_in_flight`.
    pub(super) fn in_flight(&self) -> u32 {
        self.packets_out.saturating_sub(self.sacked_out + self.lost_out) + self.retrans_out
    }

    fn min_rtt(&self) -> u32 { self.rtt_min.s[0].1 }
}

/// The kinds of the retransmit timer (`ICSK_TIME_RETRANS`,
/// `ICSK_TIME_REO_TIMEOUT`, `ICSK_TIME_LOSS_PROBE`). One slot: arming one
/// replaces the other.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Pending { Retrans, Reo, LossProbe }

/// `tcp_reset_xmit_timer`.
fn reset_xmit_timer(c: &mut TcpConn, what: Pending, when: u64, now: u64) {
    c.tx.pending = what;
    c.rtx_at = now + when.min(TCP_RTO_MAX);
}

fn is_reno(c: &TcpConn) -> bool { !c.sack_ok }

fn in_cwnd_reduction(c: &TcpConn) -> bool { matches!(c.ca_state, CaState::Cwr | CaState::Recovery) }

/// `tcp_highest_sack_seq`.
fn highest_sack_seq(c: &TcpConn) -> u32 {
    if c.tx.sacked_out == 0 { c.snd_una } else { c.tx.highest_sack }
}

/// `tcp_any_retrans_done`.
fn any_retrans_done(c: &TcpConn) -> bool {
    c.tx.retrans_out != 0 || c.tx.queue.front().is_some_and(|s| s.sacked & EVER_RETRANS != 0)
}

/// `tcp_mark_skb_lost`.
fn mark_skb_lost(c: &mut TcpConn, i: usize) {
    let sacked = c.tx.queue[i].sacked;
    if sacked & SACKED_ACKED != 0 { return; }
    if sacked & LOST != 0 {
        if sacked & SACKED_RETRANS != 0 {
            c.tx.queue[i].sacked &= !SACKED_RETRANS;
            c.tx.retrans_out -= 1;
            c.tx.lost = c.tx.lost.wrapping_add(1);
        }
    } else {
        c.tx.lost_out += 1;
        c.tx.queue[i].sacked |= LOST;
        c.tx.lost = c.tx.lost.wrapping_add(1);
    }
}

/// `tcp_fragment` in the retransmit queue: segment `i` keeps its first
/// `len` bytes, the rest (and the FIN) becomes the next one.
fn fragment(c: &mut TcpConn, i: usize, len: usize) {
    let s = &mut c.tx.queue[i];
    let split = s.seq.wrapping_add(len as u32);
    let rest = TxSeg { seq: split, end: s.end, fin: s.fin, ..*s };
    s.end = split;
    s.fin = false;
    let sacked = rest.sacked;
    c.tx.queue.insert(i + 1, rest);
    // tcp_adjust_pcount: one packet became two.
    c.tx.packets_out += 1;
    if sacked & SACKED_ACKED != 0 { c.tx.sacked_out += 1; }
    if sacked & SACKED_RETRANS != 0 { c.tx.retrans_out += 1; }
    if sacked & LOST != 0 { c.tx.lost_out += 1; }
}

/// `tcp_event_new_data_sent`: a new segment left.
pub(super) fn new_data_sent(c: &mut TcpConn, seq: u32, end: u32, fin: bool, now: u64, now_us: u64) {
    let prior_packets = c.tx.packets_out;
    c.tx.queue.push_back(TxSeg {
        seq, end, fin, sacked: 0, sent_us: now_us,
        tx_delivered: 0, tx_delivered_mstamp: 0, tx_first_tx_mstamp: 0, tx_app_limited: false,
    });
    let i = c.tx.queue.len() - 1;
    rate_skb_sent(c, i, prior_packets);
    c.tx.packets_out += 1;
    if prior_packets == 0 || c.tx.pending == Pending::LossProbe { rearm_rto(c, now, now_us); }
}

/// `tcp_rearm_rto`.
fn rearm_rto(c: &mut TcpConn, now: u64, now_us: u64) {
    if c.tx.packets_out == 0 {
        c.tx.pending = Pending::Retrans;
        return;
    }
    let rto = if c.tx.pending != Pending::Retrans {
        // Offset the time since the regular RTO was installed.
        us_to_ticks(rto_delta_us(c, now_us).max(1) as u64)
    } else {
        c.rto
    };
    reset_xmit_timer(c, Pending::Retrans, rto, now);
}

/// `tcp_rto_delta_us`: how far in the future the RTO is, counted from the
/// head's last send.
fn rto_delta_us(c: &TcpConn, now_us: u64) -> i64 {
    let head = c.tx.queue.front().map_or(now_us, |s| s.sent_us);
    (head + c.rto * TICK_US) as i64 - now_us as i64
}

/// `tcp_schedule_loss_probe`: in two RTTs, for a SACK connection not in
/// loss recovery.
fn schedule_loss_probe(c: &mut TcpConn, advancing_rto: bool, now: u64, now_us: u64) -> bool {
    if c.tx.packets_out == 0 || is_reno(c) || !matches!(c.ca_state, CaState::Open | CaState::Cwr) {
        return false;
    }
    // Plus the minimum RTO for a delayed ACK when one packet is out.
    let mut timeout = if c.srtt_us != 0 {
        let mut t = (c.srtt_us >> 2) as u64;
        t += if c.tx.packets_out == 1 { TCP_RTO_MIN_US } else { TCP_TIMEOUT_MIN_US };
        us_to_ticks(t)
    } else {
        TCP_TIMEOUT_INIT
    };
    // An earlier RTO wins.
    let rto_delta = if advancing_rto { (c.rto * TICK_US) as i64 } else { rto_delta_us(c, now_us) };
    if rto_delta > 0 { timeout = timeout.min(us_to_ticks(rto_delta as u64)); }
    reset_xmit_timer(c, Pending::LossProbe, timeout, now);
    true
}

/// `tcp_set_xmit_timer`: a loss probe if it may, else the RTO.
fn set_xmit_timer(c: &mut TcpConn, now: u64, now_us: u64) {
    if !schedule_loss_probe(c, true, now, now_us) { rearm_rto(c, now, now_us); }
}

/// What `tcp_write_xmit` adds after sending; `push_one == 2` is a loss
/// probe, which schedules no further one.
pub(super) fn xmit_scheduled(c: &mut TcpConn, sent_pkts: u32, push_one: u8, now: u64, now_us: u64) {
    if sent_pkts > 0 && push_one != 2 { schedule_loss_probe(c, false, now, now_us); }
}

/// `tcp_send_loss_probe`: new data if the window takes it, else the last
/// segment again.
pub(super) fn send_loss_probe<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>) {
    let now = io.ticks();
    let now_us = io.env.uptime_us();
    if c.tx.tlp_high_seq.is_none() {
        c.tx.tlp_retrans = false;
        let sent = c.snd_nxt.wrapping_sub(c.snd_una) as usize;
        let unsent = c.send_buf.len().saturating_sub(sent);
        let mut probed = false;
        if unsent > 0 && sent + unsent.min(eff_mss(c)) <= c.snd_wnd as usize {
            let prior = c.tx.packets_out;
            write_xmit_one(c, io, 2);
            if c.tx.packets_out > prior { probed = true; }
        } else if let Some(i) = c.tx.queue.len().checked_sub(1)
            && retransmit_skb_raw(c, io, i)
        {
            c.tx.tlp_retrans = true;
            probed = true;
        }
        if probed {
            c.tx.tlp_high_seq = Some(c.snd_nxt);
            // rearm_rto restarts the RTO from now.
            c.tx.pending = Pending::Retrans;
        }
    }
    rearm_rto(c, now, now_us);
}

/// `tcp_process_tlp_ack`: an ACK during a loss probe episode.
fn process_tlp_ack(c: &mut TcpConn, ack: u32, flag: u32, now: u64) {
    let Some(high) = c.tx.tlp_high_seq else { return };
    if before(ack, high) { return; }
    if !c.tx.tlp_retrans {
        // The probe was new data, and it was acknowledged.
        c.tx.tlp_high_seq = None;
    } else if flag & FLAG_DSACK_TLP != 0 {
        // Original and probe both arrived: no loss.
        c.tx.tlp_high_seq = None;
    } else if after(ack, high) {
        // The probe repaired a loss: reduce once.
        init_cwnd_reduction(c);
        c.ca_state = CaState::Cwr;
        end_cwnd_reduction(c, now);
        try_keep_open(c);
    } else if flag & (FLAG_SND_UNA_ADVANCED | FLAG_NOT_DUP | FLAG_DATA_SACKED) == 0 {
        // A pure duplicate: original and probe both arrived.
        c.tx.tlp_high_seq = None;
    }
}

/// `tcp_cwnd_validate` (RFC 7661): track how much of the window is used;
/// a window unused for an RTO shrinks towards what was used.
fn cwnd_validate(c: &mut TcpConn, is_cwnd_limited: bool, now: u64) {
    if !before(c.snd_una, c.tx.cwnd_usage_seq)
        || is_cwnd_limited
        || (!c.tx.is_cwnd_limited && c.tx.packets_out > c.tx.max_packets_out)
    {
        c.tx.is_cwnd_limited = is_cwnd_limited;
        c.tx.max_packets_out = c.tx.packets_out;
        c.tx.cwnd_usage_seq = c.snd_nxt;
    }
    if self::is_cwnd_limited(c) {
        c.tx.snd_cwnd_used = 0;
        c.tx.snd_cwnd_stamp = now;
    } else {
        if c.tx.packets_out > c.tx.snd_cwnd_used { c.tx.snd_cwnd_used = c.tx.packets_out; }
        if now.saturating_sub(c.tx.snd_cwnd_stamp) >= c.rto {
            cwnd_application_limited(c, now);
        }
    }
}

/// `tcp_cwnd_application_limited`: limited by the application or the
/// receiver's window, not by the network.
fn cwnd_application_limited(c: &mut TcpConn, now: u64) {
    if c.ca_state == CaState::Open && !c.tx.nospace {
        let win_used = c.tx.snd_cwnd_used.max(TCP_INIT_CWND);
        if win_used < c.snd_cwnd {
            c.snd_ssthresh = current_ssthresh(c);
            c.snd_cwnd = (c.snd_cwnd + win_used) >> 1;
        }
        c.tx.snd_cwnd_used = 0;
    }
    c.tx.snd_cwnd_stamp = now;
}

/// `tcp_cwnd_restart`: after idle longer than an RTO the window falls back
/// towards the initial one, halving per RTO idle.
fn cwnd_restart(c: &mut TcpConn, delta: u64, now: u64) {
    let restart_cwnd = TCP_INIT_CWND.min(c.snd_cwnd);
    let mut cwnd = c.snd_cwnd;
    c.snd_ssthresh = current_ssthresh(c);
    let mut delta = delta as i64;
    loop {
        delta -= c.rto as i64;
        if delta <= 0 || cwnd <= restart_cwnd { break; }
        cwnd >>= 1;
    }
    c.snd_cwnd = cwnd.max(restart_cwnd);
    c.tx.snd_cwnd_stamp = now;
    c.tx.snd_cwnd_used = 0;
}

/// `tcp_slow_start_after_idle_check`.
pub(super) fn slow_start_after_idle_check(c: &mut TcpConn, now: u64) {
    if c.tx.packets_out != 0 { return; }
    let delta = now.saturating_sub(c.tx.lsndtime);
    if delta > c.rto { cwnd_restart(c, delta, now); }
}

/// `tcp_event_data_sent`: a segment with data leaves.
pub(super) fn event_data_sent(c: &mut TcpConn, now: u64) {
    if c.tx.in_flight() == 0 { cubic::tx_start(c, now); }
    c.tx.lsndtime = now;
    ack::data_sent(c, now);
}

/// `tcp_init_congestion_control` and the stamps `tcp_init_transfer` sets,
/// once the connection is established.
pub(super) fn init_congestion_control(c: &mut TcpConn, now: u64, now_us: u64) {
    c.tx.lsndtime = now;
    c.tx.snd_cwnd_stamp = now;
    cubic::init(c, now_us);
}

/// What `tcp_write_xmit` adds to the window once it sent `sent_pkts` new
/// segments.
pub(super) fn xmit_done(c: &mut TcpConn, sent_pkts: u32, mut is_cwnd_limited: bool, now: u64) {
    is_cwnd_limited |= c.tx.in_flight() >= c.snd_cwnd;
    if sent_pkts > 0 || is_cwnd_limited { cwnd_validate(c, is_cwnd_limited, now); }
    if sent_pkts > 0 && in_cwnd_reduction(c) { c.tx.prr_out += sent_pkts; }
}

/// `tcp_is_cwnd_limited`.
pub(super) fn is_cwnd_limited(c: &TcpConn) -> bool {
    if c.tx.is_cwnd_limited { return true; }
    if c.snd_cwnd < c.snd_ssthresh { return c.snd_cwnd < 2 * c.tx.max_packets_out; }
    false
}

/// `tcp_slow_start`: returns what is left of `acked` past ssthresh.
pub(super) fn slow_start(c: &mut TcpConn, acked: u32) -> u32 {
    let cwnd = (c.snd_cwnd + acked).min(c.snd_ssthresh);
    let left = acked - (cwnd - c.snd_cwnd);
    c.snd_cwnd = cwnd;
    left
}

/// `tcp_cong_avoid_ai`: one packet more per `w` packets ACKed.
pub(super) fn cong_avoid_ai(c: &mut TcpConn, w: u32, acked: u32) {
    let w = w.max(1);
    if c.snd_cwnd_cnt >= w {
        c.snd_cwnd_cnt = 0;
        c.snd_cwnd += 1;
    }
    c.snd_cwnd_cnt += acked;
    if c.snd_cwnd_cnt >= w {
        let delta = c.snd_cwnd_cnt / w;
        c.snd_cwnd_cnt -= delta * w;
        c.snd_cwnd += delta;
    }
}

/// `tcp_current_ssthresh`.
fn current_ssthresh(c: &TcpConn) -> u32 {
    if in_cwnd_reduction(c) {
        c.snd_ssthresh
    } else {
        c.snd_ssthresh.max((c.snd_cwnd >> 1) + (c.snd_cwnd >> 2))
    }
}

/// `tcp_sndbuf_expand`, in payload bytes: twice the larger of the
/// congestion window and the reordering degree.
fn sndbuf_expand(c: &mut TcpConn) {
    let nr_segs = TCP_INIT_CWND.max(c.snd_cwnd).max(c.tx.reordering + 1) as usize;
    let want = (2 * nr_segs * eff_mss(c)).min(SND_BUF_MAX);
    if want > c.snd_buf_limit { c.snd_buf_limit = want; }
}

/// `tcp_init_cwnd_reduction`.
fn init_cwnd_reduction(c: &mut TcpConn) {
    c.high_seq = c.snd_nxt;
    c.tx.tlp_high_seq = None;
    c.snd_cwnd_cnt = 0;
    c.tx.prior_cwnd = c.snd_cwnd;
    c.tx.prr_delivered = 0;
    c.tx.prr_out = 0;
    c.snd_ssthresh = cubic::recalc_ssthresh(c);
}

/// `tcp_cwnd_reduction`: PRR (RFC 6937).
fn cwnd_reduction(c: &mut TcpConn, newly_acked_sacked: u32, newly_lost: u32, flag: u32) {
    if newly_acked_sacked == 0 || c.tx.prior_cwnd == 0 { return; }
    let in_flight = c.tx.in_flight() as i64;
    let delta = c.snd_ssthresh as i64 - in_flight;
    c.tx.prr_delivered += newly_acked_sacked;
    let mut sndcnt;
    if delta < 0 {
        let dividend = c.snd_ssthresh as u64 * c.tx.prr_delivered as u64 + c.tx.prior_cwnd as u64 - 1;
        sndcnt = (dividend / c.tx.prior_cwnd as u64) as i64 - c.tx.prr_out as i64;
    } else {
        sndcnt = (c.tx.prr_delivered as i64 - c.tx.prr_out as i64).max(newly_acked_sacked as i64);
        if flag & FLAG_SND_UNA_ADVANCED != 0 && newly_lost == 0 { sndcnt += 1; }
        sndcnt = sndcnt.min(delta);
    }
    // Force a fast retransmit upon entering fast recovery.
    sndcnt = sndcnt.max(if c.tx.prr_out != 0 { 0 } else { 1 });
    c.snd_cwnd = (in_flight + sndcnt).max(0) as u32;
}

/// `tcp_end_cwnd_reduction`.
fn end_cwnd_reduction(c: &mut TcpConn, now: u64) {
    if c.snd_ssthresh != u32::MAX && (c.ca_state == CaState::Cwr || c.tx.undo_marker.is_some()) {
        c.snd_cwnd = c.snd_ssthresh;
        c.tx.snd_cwnd_stamp = now;
    }
}

/// `tcp_init_undo`.
fn init_undo(c: &mut TcpConn) {
    c.tx.undo_marker = Some(c.snd_una);
    // Retransmissions still in flight may bring D-SACKs later; 0 would mean
    // "undo now".
    c.tx.undo_retrans = c.tx.retrans_out as i32;
    if c.tx.tlp_high_seq.is_some() && c.tx.tlp_retrans { c.tx.undo_retrans += 1; }
    if c.tx.undo_retrans == 0 { c.tx.undo_retrans = -1; }
}

/// `tcp_enter_recovery`.
fn enter_recovery(c: &mut TcpConn) {
    if !any_retrans_done(c) { c.retrans_stamp = None; }
    c.tx.prior_ssthresh = 0;
    init_undo(c);
    if !in_cwnd_reduction(c) {
        c.tx.prior_ssthresh = current_ssthresh(c);
        init_cwnd_reduction(c);
    }
    c.ca_state = CaState::Recovery;
}

/// `tcp_tsopt_ecr_before`.
fn tsopt_ecr_before(c: &TcpConn, when: u32) -> bool {
    c.tx.rcv_tsecr.is_some_and(|t| t != 0 && before(t, when))
}

/// `tcp_packet_delayed`: nothing was retransmitted, or the echo predates
/// the first retransmission, so the original arrived.
fn packet_delayed(c: &TcpConn) -> bool {
    if let Some(stamp) = c.retrans_stamp { return tsopt_ecr_before(c, stamp); }
    if is_reno(c) && !before(c.snd_una, c.high_seq) { return false; }
    true
}

/// `tcp_may_undo`.
fn may_undo(c: &TcpConn) -> bool {
    c.tx.undo_marker.is_some() && (c.tx.undo_retrans == 0 || packet_delayed(c))
}

/// `tcp_undo_cwnd_reduction`.
fn undo_cwnd_reduction(c: &mut TcpConn, unmark_loss: bool, now: u64) {
    if unmark_loss {
        for s in c.tx.queue.iter_mut() { s.sacked &= !LOST; }
        c.tx.lost_out = 0;
    }
    if c.tx.prior_ssthresh != 0 {
        // tcp_reno_undo_cwnd, which CUBIC uses.
        c.snd_cwnd = c.snd_cwnd.max(c.tx.prior_cwnd);
        if c.tx.prior_ssthresh > c.snd_ssthresh { c.snd_ssthresh = c.tx.prior_ssthresh; }
    }
    c.tx.snd_cwnd_stamp = now;
    c.tx.undo_marker = None;
    c.tx.rack.advanced = true;
}

/// `tcp_is_non_sack_preventing_reopen`: Reno holds the state until
/// something above `high_seq` is acknowledged (RFC 6582 §3.2 step 5).
fn is_non_sack_preventing_reopen(c: &mut TcpConn) -> bool {
    if c.snd_una == c.high_seq && is_reno(c) {
        if !any_retrans_done(c) { c.retrans_stamp = None; }
        return true;
    }
    false
}

/// `tcp_try_undo_dsack`: D-SACKs covered every retransmission.
fn try_undo_dsack(c: &mut TcpConn, now: u64) -> bool {
    if c.tx.undo_marker.is_some() && c.tx.undo_retrans == 0 {
        c.tx.rack.reo_wnd_persist = (c.tx.rack.reo_wnd_persist + 1).min(TCP_RACK_RECOVERY_THRESH);
        undo_cwnd_reduction(c, false, now);
        return true;
    }
    false
}

/// `tcp_try_undo_loss`: after a timeout that was not needed.
fn try_undo_loss(c: &mut TcpConn, frto_undo: bool, now: u64) -> bool {
    if frto_undo || may_undo(c) {
        undo_cwnd_reduction(c, true, now);
        c.retries = 0;
        if is_non_sack_preventing_reopen(c) { return true; }
        if frto_undo || !is_reno(c) {
            c.ca_state = CaState::Open;
            c.tx.is_sack_reneg = false;
        }
        return true;
    }
    false
}

/// `tcp_try_undo_partial`: a partial ACK the original transmission
/// brought.
fn try_undo_partial(c: &mut TcpConn, prior_una: u32, now: u64) -> bool {
    if c.tx.undo_marker.is_some() && packet_delayed(c) {
        check_sack_reordering(c, prior_una);
        if c.tx.retrans_out != 0 { return true; }
        if !any_retrans_done(c) { c.retrans_stamp = None; }
        undo_cwnd_reduction(c, true, now);
        try_keep_open(c);
    }
    false
}

/// `tcp_try_keep_open`.
fn try_keep_open(c: &mut TcpConn) {
    let state = if c.tx.sacked_out + c.tx.lost_out != 0 || any_retrans_done(c) {
        CaState::Disorder
    } else {
        CaState::Open
    };
    if c.ca_state != state {
        c.ca_state = state;
        c.high_seq = c.snd_nxt;
    }
}

/// `tcp_try_to_open`.
fn try_to_open(c: &mut TcpConn) {
    if !any_retrans_done(c) { c.retrans_stamp = None; }
    try_keep_open(c);
}

/// `tcp_try_undo_recovery`: recovery is over; take the reduction back if
/// it was needless.
fn try_undo_recovery(c: &mut TcpConn, now: u64) -> bool {
    if may_undo(c) {
        undo_cwnd_reduction(c, false, now);
    } else if c.tx.rack.reo_wnd_persist != 0 {
        c.tx.rack.reo_wnd_persist -= 1;
    }
    if is_non_sack_preventing_reopen(c) { return true; }
    c.ca_state = CaState::Open;
    c.tx.is_sack_reneg = false;
    false
}

/// `tcp_limit_reno_sacked`.
fn limit_reno_sacked(c: &mut TcpConn) -> bool {
    let holes = c.tx.lost_out.max(1).min(c.tx.packets_out);
    if c.tx.sacked_out + holes > c.tx.packets_out {
        c.tx.sacked_out = c.tx.packets_out - holes;
        return true;
    }
    false
}

/// `tcp_check_reno_reordering`.
fn check_reno_reordering(c: &mut TcpConn, addend: u32) {
    if !limit_reno_sacked(c) { return; }
    c.tx.reordering = (c.tx.packets_out + addend).min(TCP_MAX_REORDERING);
    c.tx.reord_seen += 1;
}

/// `tcp_add_reno_sack`: a duplicate ACK is a segment that left the
/// network.
fn add_reno_sack(c: &mut TcpConn, num_dupack: u32) {
    if num_dupack == 0 { return; }
    let prior = c.tx.sacked_out;
    c.tx.sacked_out += num_dupack;
    check_reno_reordering(c, 0);
    c.tx.delivered = c.tx.delivered.wrapping_add(c.tx.sacked_out.saturating_sub(prior));
}

/// `tcp_remove_reno_sacks`.
fn remove_reno_sacks(c: &mut TcpConn, acked: u32) {
    if acked > 0 {
        c.tx.delivered = c.tx.delivered.wrapping_add(acked.saturating_sub(c.tx.sacked_out).max(1));
        if acked > c.tx.sacked_out {
            c.tx.sacked_out = 0;
        } else {
            c.tx.sacked_out -= acked - 1;
        }
    }
    check_reno_reordering(c, acked);
}

/// `tcp_check_sack_reordering`.
fn check_sack_reordering(c: &mut TcpConn, low_seq: u32) {
    let mss = c.mss_cache as u32;
    let fack = highest_sack_seq(c);
    if !before(low_seq, fack) { return; }
    let metric = fack.wrapping_sub(low_seq);
    if mss != 0 && metric > c.tx.reordering * mss {
        c.tx.reordering = metric.div_ceil(mss).min(TCP_MAX_REORDERING);
    }
    c.tx.reord_seen += 1;
}

/// The sender's part of `tcp_get_info`.
pub(super) fn fill_info(c: &TcpConn, i: &mut TcpInfo, now: u64) {
    i.ca_state = match c.ca_state {
        CaState::Open => "open", CaState::Disorder => "disorder", CaState::Cwr => "cwr",
        CaState::Recovery => "recovery", CaState::Loss => "loss",
    };
    i.reordering = c.tx.reordering;
    i.unacked = c.tx.packets_out;
    i.sacked = c.tx.sacked_out;
    i.lost = c.tx.lost_out;
    i.retrans = c.tx.retrans_out;
    i.total_retrans = c.tx.total_retrans;
    i.last_data_sent_ms = now.saturating_sub(c.tx.lsndtime) * (TICK_US / 1000);
    i.min_rtt_us = if c.tx.min_rtt() == u32::MAX { 0 } else { c.tx.min_rtt() };
    i.delivered = c.tx.delivered;
    i.dsack_dups = c.tx.dsack_dups;
    i.reord_seen = c.tx.reord_seen;
    i.app_limited = c.tx.rate_app_limited;
    // tcp_compute_delivery_rate
    i.delivery_rate = if c.tx.rate_interval_us != 0 {
        c.tx.rate_delivered as u64 * c.mss_cache as u64 * 1_000_000 / c.tx.rate_interval_us
    } else {
        0
    };
}

// ── Delivery rate (tcp_rate.c) ───────────────────────────────────────

/// `struct rate_sample`.
#[derive(Default)]
struct RateSample {
    prior_mstamp: u64,
    prior_delivered: u32,
    delivered: i32,
    interval_us: i64,
    last_end_seq: u32,
    is_app_limited: bool,
    is_retrans: bool,
}

/// `tcp_rate_skb_sent`: what segment `i` takes along for its sample.
fn rate_skb_sent(c: &mut TcpConn, i: usize, packets_out: u32) {
    let sent_us = c.tx.queue[i].sent_us;
    if packets_out == 0 {
        c.tx.first_tx_mstamp = sent_us;
        c.tx.delivered_mstamp = sent_us;
    }
    let (first, dmst, del, app) = (c.tx.first_tx_mstamp, c.tx.delivered_mstamp, c.tx.delivered,
                                   c.tx.app_limited != 0);
    let s = &mut c.tx.queue[i];
    s.tx_first_tx_mstamp = first;
    s.tx_delivered_mstamp = dmst;
    s.tx_delivered = del;
    s.tx_app_limited = app;
}

/// `tcp_rate_skb_delivered`: the sample is taken from the most recently
/// sent of the segments an ACK delivers.
fn rate_skb_delivered(c: &mut TcpConn, s: &TxSeg, rs: &mut RateSample) {
    if s.tx_delivered_mstamp == 0 { return; }
    if rs.prior_delivered == 0
        || sent_after(s.sent_us, c.tx.first_tx_mstamp, s.end, rs.last_end_seq)
    {
        rs.prior_delivered = s.tx_delivered;
        rs.prior_mstamp = s.tx_delivered_mstamp;
        rs.is_app_limited = s.tx_app_limited;
        rs.is_retrans = s.sacked & RETRANS != 0;
        rs.last_end_seq = s.end;
        c.tx.first_tx_mstamp = s.sent_us;
        // The send phase of this window.
        rs.interval_us = s.sent_us as i64 - s.tx_first_tx_mstamp as i64;
    }
}

/// `tcp_rate_gen`.
fn rate_gen(c: &mut TcpConn, delivered: u32, rs: &mut RateSample, is_sack_reneg: bool, now_us: u64) {
    if c.tx.app_limited != 0 && after(c.tx.delivered, c.tx.app_limited) { c.tx.app_limited = 0; }
    if delivered != 0 { c.tx.delivered_mstamp = now_us; }
    if rs.prior_mstamp == 0 || is_sack_reneg {
        rs.delivered = -1;
        rs.interval_us = -1;
        return;
    }
    rs.delivered = c.tx.delivered.wrapping_sub(rs.prior_delivered) as i32;
    // Sending and ACKing as two pipeline phases: the slower decides.
    let snd_us = rs.interval_us;
    let ack_us = now_us as i64 - rs.prior_mstamp as i64;
    rs.interval_us = snd_us.max(ack_us);
    if rs.interval_us < c.tx.min_rtt() as i64 {
        rs.interval_us = -1;
        return;
    }
    // The last sample not limited by the application, or the fastest.
    if !rs.is_app_limited
        || rs.delivered as u64 * c.tx.rate_interval_us >= c.tx.rate_delivered as u64 * rs.interval_us as u64
    {
        c.tx.rate_delivered = rs.delivered as u32;
        c.tx.rate_interval_us = rs.interval_us as u64;
        c.tx.rate_app_limited = rs.is_app_limited;
    }
}

/// `tcp_rate_check_app_limited`: the application gives less than the
/// window takes, so samples now say little about the path.
pub(super) fn rate_check_app_limited(c: &mut TcpConn) {
    let unsent = c.send_buf.len().saturating_sub(c.snd_nxt.wrapping_sub(c.snd_una) as usize);
    if unsent < c.mss_cache as usize
        && c.tx.in_flight() < c.snd_cwnd
        && c.tx.lost_out <= c.tx.retrans_out
    {
        let v = c.tx.delivered.wrapping_add(c.tx.in_flight());
        c.tx.app_limited = if v == 0 { 1 } else { v };
    }
}

/// `tcp_rack_update_reo_wnd`: after a D-SACK the reordering window grows
/// by min_rtt/4, at most once per round, and holds for 16 recoveries.
fn rack_update_reo_wnd(c: &mut TcpConn, rs: &RateSample) {
    if rs.prior_delivered == 0 { return; }
    // A D-SACK from before the last growth says nothing new.
    if before(rs.prior_delivered, c.tx.rack.last_delivered) { c.tx.rack.dsack_seen = false; }
    if c.tx.rack.dsack_seen {
        c.tx.rack.reo_wnd_steps = (c.tx.rack.reo_wnd_steps + 1).min(0xff);
        c.tx.rack.dsack_seen = false;
        c.tx.rack.last_delivered = c.tx.delivered;
        c.tx.rack.reo_wnd_persist = TCP_RACK_RECOVERY_THRESH;
    } else if c.tx.rack.reo_wnd_persist == 0 {
        c.tx.rack.reo_wnd_steps = 1;
    }
}

// ── RACK (tcp_recovery.c) ────────────────────────────────────────────

/// `tcp_skb_sent_after`.
fn sent_after(t1: u64, t2: u64, seq1: u32, seq2: u32) -> bool {
    t1 > t2 || (t1 == t2 && after(seq1, seq2))
}

/// `tcp_rack_reo_wnd`, in µs.
fn rack_reo_wnd(c: &TcpConn) -> u64 {
    if c.tx.reord_seen == 0 {
        if c.ca_state >= CaState::Recovery { return 0; }
        if c.tx.sacked_out >= c.tx.reordering { return 0; }
    }
    ((c.tx.min_rtt() >> 2) as u64 * c.tx.rack.reo_wnd_steps as u64).min((c.srtt_us >> 3) as u64)
}

/// `tcp_rack_skb_timeout`.
fn rack_skb_timeout(c: &TcpConn, s: &TxSeg, reo_wnd: u64, now_us: u64) -> i64 {
    c.tx.rack.rtt_us as i64 + reo_wnd as i64 - now_us.saturating_sub(s.sent_us) as i64
}

/// `tcp_rack_advance`.
fn rack_advance(c: &mut TcpConn, sacked: u8, end_seq: u32, xmit_us: u64, now_us: u64) {
    let rtt_us = now_us.saturating_sub(xmit_us);
    // A retransmission acked sooner than the path's minimum RTT: the
    // original may have been what arrived.
    if rtt_us < c.tx.min_rtt() as u64 && sacked & RETRANS != 0 { return; }
    c.tx.rack.advanced = true;
    c.tx.rack.rtt_us = rtt_us;
    if sent_after(xmit_us, c.tx.rack.mstamp, end_seq, c.tx.rack.end_seq) {
        c.tx.rack.mstamp = xmit_us;
        c.tx.rack.end_seq = end_seq;
    }
}

/// `tcp_rack_detect_loss`: lost is what was sent before a delivered
/// segment and is older than the RTT plus the reordering window. Returns
/// how long the youngest of the rest still has, in µs.
fn rack_detect_loss(c: &mut TcpConn, now_us: u64) -> u64 {
    let reo_wnd = rack_reo_wnd(c);
    let mut reo_timeout = 0u64;
    for i in 0..c.tx.queue.len() {
        let s = &c.tx.queue[i];
        // Only what `tsorted_sent_queue` holds: neither SACKed nor marked
        // lost and not yet resent.
        if s.sacked & SACKED_ACKED != 0 { continue; }
        if s.sacked & LOST != 0 && s.sacked & SACKED_RETRANS == 0 { continue; }
        if !sent_after(c.tx.rack.mstamp, s.sent_us, c.tx.rack.end_seq, s.end) { continue; }
        let remaining = rack_skb_timeout(c, s, reo_wnd, now_us);
        if remaining <= 0 {
            mark_skb_lost(c, i);
        } else {
            reo_timeout = reo_timeout.max(remaining as u64);
        }
    }
    reo_timeout
}

/// `tcp_rack_mark_lost`. True when it armed the reordering timer.
fn rack_mark_lost(c: &mut TcpConn, now: u64, now_us: u64) -> bool {
    if !c.tx.rack.advanced { return false; }
    c.tx.rack.advanced = false;
    let timeout = rack_detect_loss(c, now_us);
    if timeout != 0 {
        let when = us_to_ticks(timeout + TCP_TIMEOUT_MIN_US).min(c.rto);
        reset_xmit_timer(c, Pending::Reo, when, now);
    }
    timeout != 0
}

/// `tcp_rack_reo_timeout`: the reordering window has passed.
pub(super) fn rack_reo_timeout<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>) {
    let now = io.ticks();
    let now_us = io.env.uptime_us();
    let lost = c.tx.lost;
    let prior_inflight = c.tx.in_flight();
    rack_detect_loss(c, now_us);
    if prior_inflight != c.tx.in_flight() {
        if c.ca_state != CaState::Recovery {
            enter_recovery(c);
            cwnd_reduction(c, 1, c.tx.lost.wrapping_sub(lost), 0);
        }
        xmit_retransmit_queue(c, io);
    }
    if c.tx.pending != Pending::Retrans { rearm_rto(c, now, now_us); }
}

/// `tcp_newreno_mark_lost` (RFC 6582).
fn newreno_mark_lost(c: &mut TcpConn, snd_una_advanced: bool) {
    let state = c.ca_state;
    if (state < CaState::Recovery && c.tx.sacked_out >= c.tx.reordering)
        || (state == CaState::Recovery && snd_una_advanced)
    {
        if c.tx.queue[0].sacked & LOST != 0 { return; }
        mark_skb_lost(c, 0);
    }
}

/// `tcp_identify_packet_loss`.
fn identify_packet_loss(c: &mut TcpConn, flag: &mut u32, now: u64, now_us: u64) {
    if c.tx.queue.is_empty() { return; }
    if is_reno(c) {
        newreno_mark_lost(c, *flag & FLAG_SND_UNA_ADVANCED != 0);
    } else {
        let prior_retrans = c.tx.retrans_out;
        if rack_mark_lost(c, now, now_us) { *flag &= !FLAG_SET_XMIT_TIMER; }
        if prior_retrans > c.tx.retrans_out { *flag |= FLAG_LOST_RETRANS; }
    }
}

// ── The state machine (tcp_fastretrans_alert) ────────────────────────

/// What an ACK leads to sending (`REXMIT_*`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rexmit { None, Lost, New }

/// `tcp_process_loss`: Loss ends once `high_seq` is acknowledged, or as
/// soon as the timeout shows itself spurious (F-RTO, RFC 5682 §3.1).
fn process_loss(c: &mut TcpConn, flag: u32, num_dupack: u32, rexmit: &mut Rexmit, now: u64) {
    let recovered = !before(c.snd_una, c.high_seq);
    if flag & FLAG_SND_UNA_ADVANCED != 0 && try_undo_loss(c, false, now) { return; }
    if c.tx.frto {
        // Step 3.b: never-retransmitted data was (S)ACKed, so not all was
        // lost.
        if flag & FLAG_ORIG_SACK_ACKED != 0 && try_undo_loss(c, true, now) { return; }
        if after(c.snd_nxt, c.high_seq) {
            // Step 3.a: the loss was real.
            if flag & FLAG_DATA_SACKED != 0 || num_dupack != 0 { c.tx.frto = false; }
        } else if flag & FLAG_SND_UNA_ADVANCED != 0 && !recovered {
            c.high_seq = c.snd_nxt;
            // Step 2.b: send new data and see what it brings.
            let unsent = c.send_buf.len() > c.snd_nxt.wrapping_sub(c.snd_una) as usize;
            if unsent && after(c.snd_una.wrapping_add(c.snd_wnd), c.snd_nxt) {
                *rexmit = Rexmit::New;
                return;
            }
            c.tx.frto = false;
        }
    }
    if recovered {
        try_undo_recovery(c, now);
        return;
    }
    if is_reno(c) {
        if after(c.snd_nxt, c.high_seq) && num_dupack != 0 {
            add_reno_sack(c, num_dupack);
        } else if flag & FLAG_SND_UNA_ADVANCED != 0 {
            c.tx.sacked_out = 0;
        }
    }
    *rexmit = Rexmit::Lost;
}

/// `tcp_fastretrans_alert`: loss and the congestion state. The window
/// itself changes in `cong_control`.
fn fastretrans_alert(c: &mut TcpConn, prior_una: u32, num_dupack: u32, flag: &mut u32,
                     rexmit: &mut Rexmit, now: u64, now_us: u64) {
    if c.tx.packets_out == 0 && c.tx.sacked_out != 0 { c.tx.sacked_out = 0; }

    // B. tcp_check_sack_reneging: the receiver dropped data it had SACKed.
    // Give it max(RTT/2, 10 ms) to correct itself before the RTO clears
    // the scoreboard.
    if *flag & FLAG_SACK_RENEGING != 0 && *flag & FLAG_SND_UNA_ADVANCED != 0 {
        let delay = us_to_ticks((c.srtt_us >> 4) as u64).max(1);
        reset_xmit_timer(c, Pending::Retrans, delay, now);
        *flag &= !FLAG_SET_XMIT_TIMER;
        return;
    }

    // D. State exit: `high_seq` acknowledged.
    if c.ca_state == CaState::Open {
        c.retrans_stamp = None;
    } else if !before(c.snd_una, c.high_seq) && c.ca_state == CaState::Cwr {
        // Held until something above `high_seq` is acknowledged.
        if c.snd_una != c.high_seq {
            end_cwnd_reduction(c, now);
            c.ca_state = CaState::Open;
        }
    } else if !before(c.snd_una, c.high_seq) && c.ca_state == CaState::Recovery {
        if is_reno(c) { c.tx.sacked_out = 0; }
        if try_undo_recovery(c, now) { return; }
        end_cwnd_reduction(c, now);
    }

    // E. Process the state.
    let mut open_path = false;
    match c.ca_state {
        CaState::Recovery => {
            if *flag & FLAG_SND_UNA_ADVANCED == 0 {
                if is_reno(c) { add_reno_sack(c, num_dupack); }
            } else if try_undo_partial(c, prior_una, now) {
                return;
            }
            if try_undo_dsack(c, now) { try_to_open(c); }
            identify_packet_loss(c, flag, now, now_us);
            if c.ca_state != CaState::Recovery {
                // Undone; a loss still evident starts a new recovery.
                if c.tx.lost_out == 0 { return; }
                enter_recovery(c);
            }
        }
        CaState::Loss => {
            process_loss(c, *flag, num_dupack, rexmit, now);
            identify_packet_loss(c, flag, now, now_us);
            if !(c.ca_state == CaState::Open || *flag & FLAG_LOST_RETRANS != 0) { return; }
            open_path = true;
        }
        CaState::Open | CaState::Disorder | CaState::Cwr => open_path = true,
    }
    if open_path {
        if is_reno(c) {
            if *flag & FLAG_SND_UNA_ADVANCED != 0 { c.tx.sacked_out = 0; }
            add_reno_sack(c, num_dupack);
        }
        if c.ca_state <= CaState::Disorder { try_undo_dsack(c, now); }
        identify_packet_loss(c, flag, now, now_us);
        if c.tx.lost_out == 0 {
            try_to_open(c);
            return;
        }
        enter_recovery(c);
    }
    *rexmit = Rexmit::Lost;
}

/// `tcp_cong_control`: PRR while recovering, else growth.
fn cong_control(c: &mut TcpConn, acked_sacked: u32, newly_lost: u32, flag: u32, now: u64) {
    if in_cwnd_reduction(c) {
        cwnd_reduction(c, acked_sacked, newly_lost, flag);
    } else {
        // tcp_may_raise_cwnd
        let may_raise = if c.tx.reordering > TCP_REORDERING {
            flag & FLAG_FORWARD_PROGRESS != 0
        } else {
            flag & FLAG_DATA_ACKED != 0
        };
        if may_raise {
            cubic::cong_avoid(c, acked_sacked, now);
            c.tx.snd_cwnd_stamp = now;
        }
    }
}

/// `tcp_enter_loss` with `tcp_timeout_mark_lost`: the RTO fired.
pub(super) fn enter_loss(c: &mut TcpConn, now_us: u64) {
    let new_recovery = c.ca_state < CaState::Recovery;
    let head_sacked = c.tx.queue.front().is_some_and(|s| s.sacked & SACKED_ACKED != 0);
    // A SACKed head means the receiver reneged: forget its SACKs.
    if head_sacked || is_reno(c) {
        c.tx.sacked_out = 0;
    }
    if head_sacked { c.tx.is_sack_reneg = true; }
    for i in 0..c.tx.queue.len() {
        if head_sacked {
            c.tx.queue[i].sacked &= !SACKED_ACKED;
        } else if i != 0 && rack_skb_timeout(c, &c.tx.queue[i], 0, now_us) > 0 {
            // Sent too recently to call lost yet.
            continue;
        }
        mark_skb_lost(c, i);
    }

    // Reduce ssthresh if it has not yet been made inside this window.
    if c.ca_state <= CaState::Disorder
        || !after(c.high_seq, c.snd_una)
        || (c.ca_state == CaState::Loss && c.retries == 0)
    {
        c.tx.prior_ssthresh = current_ssthresh(c);
        c.tx.prior_cwnd = c.snd_cwnd;
        c.snd_ssthresh = cubic::recalc_ssthresh(c);
        init_undo(c);
    }
    c.snd_cwnd = c.tx.in_flight() + 1;
    c.snd_cwnd_cnt = 0;
    c.tx.snd_cwnd_stamp = now_us / TICK_US;
    if c.ca_state <= CaState::Disorder && c.tx.sacked_out >= TCP_REORDERING {
        c.tx.reordering = c.tx.reordering.min(TCP_REORDERING);
    }
    c.ca_state = CaState::Loss;
    cubic::enter_loss(c, now_us);
    c.high_seq = c.snd_nxt;
    c.tx.tlp_high_seq = None;
    // F-RTO (RFC 5682 §3.1 step 1) unless recovery was already under way,
    // except for repeated timeouts of the same data (§3.2).
    c.tx.frto = new_recovery || c.retries != 0;
}

/// `tcp_simple_retransmit`: the path MTU fell, so every segment larger
/// than the new MSS is lost; resend without reducing the window
/// (`tcp_non_congestion_loss_retransmit`).
pub(super) fn simple_retransmit<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>) {
    let mss = eff_mss(c);
    for i in 0..c.tx.queue.len() {
        if c.tx.queue[i].len() > mss { mark_skb_lost(c, i); }
    }
    if c.tx.lost_out == 0 { return; }
    if is_reno(c) { limit_reno_sacked(c); }
    if c.ca_state != CaState::Loss {
        c.high_seq = c.snd_nxt;
        c.snd_ssthresh = current_ssthresh(c);
        c.tx.prior_ssthresh = 0;
        c.tx.undo_marker = None;
        c.ca_state = CaState::Loss;
        cubic::enter_loss(c, io.env.uptime_us());
    }
    xmit_retransmit_queue(c, io);
}

// ── Retransmission (tcp_output.c) ────────────────────────────────────

/// `tcp_retrans_try_collapse`: the segments after `i` join it while they
/// fit into `space` and the peer's window.
fn retrans_try_collapse(c: &mut TcpConn, i: usize, space: usize) {
    let wnd_end = c.snd_una.wrapping_add(c.snd_wnd);
    let mut space = space as i64 - c.tx.queue[i].len() as i64;
    while i + 1 < c.tx.queue.len() {
        let next = &c.tx.queue[i + 1];
        // tcp_can_collapse
        if next.sacked & SACKED_ACKED != 0 { break; }
        space -= next.len() as i64;
        if space < 0 { break; }
        if after(next.end, wnd_end) { break; }
        // tcp_collapse_retrans
        let next = c.tx.queue.remove(i + 1).unwrap();
        let to = &mut c.tx.queue[i];
        to.end = next.end;
        to.fin |= next.fin;
        to.sacked |= next.sacked & EVER_RETRANS;
        // tcp_adjust_pcount for the one that went.
        c.tx.packets_out -= 1;
        if next.sacked & SACKED_RETRANS != 0 { c.tx.retrans_out -= 1; }
        if next.sacked & LOST != 0 { c.tx.lost_out -= 1; }
    }
}

/// `__tcp_retransmit_skb`: resend segment `i`, cut to the MSS and to the
/// peer's window.
fn retransmit_skb_raw<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>, i: usize) -> bool {
    let cur_mss = eff_mss(c).min(1460);
    let seq = c.tx.queue[i].seq;
    let wnd_end = c.snd_una.wrapping_add(c.snd_wnd);
    let mut avail = wnd_end.wrapping_sub(seq) as i32 as i64;
    let mut ok = true;
    if avail <= 0 {
        // Only the head goes into a shrunk window, as a probe.
        if seq != c.snd_una {
            ok = false;
        }
        avail = cur_mss as i64;
    }
    if ok {
        let mut len = cur_mss;
        if len as i64 > avail {
            len = (avail as usize / cur_mss) * cur_mss;
            if len == 0 { len = avail as usize; }
        }
        if c.tx.queue[i].len() > len {
            fragment(c, i, len);
        } else {
            let space = (avail as usize).min(cur_mss);
            if c.tx.queue[i].len() < space { retrans_try_collapse(c, i, space); }
        }

        let s = &c.tx.queue[i];
        let n = s.len();
        let off = s.seq.wrapping_sub(c.snd_una) as usize;
        let fin = s.fin;
        let mut chunk = [0u8; 1460];
        copy_out(&c.send_buf, off, &mut chunk[..n]);
        let flags = match (fin, n) {
            (true, 0) => FIN | ACK,
            (true, _) => FIN | ACK | PSH,
            _ => ACK | PSH,
        };
        let w = recv_window(c, io.st);
        if n > 0 { event_data_sent(c, io.ticks()); }
        ok = send_seg(c, io, seq, c.rcv_nxt, flags, w, &chunk[..n]);
        if ok {
            c.tx.queue[i].sent_us = io.env.uptime_us();
            let packets_out = c.tx.packets_out;
            rate_skb_sent(c, i, packets_out);
            c.tx.total_retrans = c.tx.total_retrans.wrapping_add(1);
            c.last_send_tick = io.ticks();
        } else {
            io.st.send_refused.fetch_add(1, Ordering::Relaxed);
        }
        // Even when it failed: no RTT sample from a send that did not happen.
        c.tx.queue[i].sacked |= EVER_RETRANS;
    }
    ok
}

/// `tcp_retransmit_skb`: a retransmission that counts in `retrans_out`.
pub(super) fn retransmit_skb<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>, i: usize) -> bool {
    let ok = retransmit_skb_raw(c, io, i);
    if ok {
        c.tx.queue[i].sacked |= RETRANS;
        c.tx.retrans_out += 1;
    }
    // The stamp of the first (attempted) retransmission.
    if c.retrans_stamp.is_none() {
        c.retrans_stamp = Some((c.tx.queue[i].sent_us / 1000) as u32);
    }
    if c.tx.undo_retrans < 0 { c.tx.undo_retrans = 0; }
    c.tx.undo_retrans += 1;
    ok
}

/// `tcp_xmit_retransmit_queue`: resend what is marked lost, as far as the
/// congestion window lets.
pub(super) fn xmit_retransmit_queue<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>) {
    if c.tx.packets_out == 0 { return; }
    let mut rearm = false;
    let mut i = 0;
    while i < c.tx.queue.len() {
        if c.snd_cwnd as i64 - c.tx.in_flight() as i64 <= 0 { break; }
        let sacked = c.tx.queue[i].sacked;
        if c.tx.retrans_out >= c.tx.lost_out { break; }
        if sacked & LOST == 0 || sacked & (SACKED_ACKED | SACKED_RETRANS) != 0 {
            i += 1;
            continue;
        }
        if !retransmit_skb(c, io, i) { break; }
        if c.ca_state != CaState::Loss { io.st.fast_retrans.fetch_add(1, Ordering::Relaxed); }
        if in_cwnd_reduction(c) { c.tx.prr_out += 1; }
        if i == 0 && c.tx.pending != Pending::Reo { rearm = true; }
        i += 1;
    }
    if rearm {
        let rto = c.rto;
        reset_xmit_timer(c, Pending::Retrans, rto, io.ticks());
    }
}

// ── The ACK (tcp_ack) ────────────────────────────────────────────────

struct SackState {
    rate: RateSample,
    flag: u32,
    reord: u32,
    first_sackt: u64,
    last_sackt: u64,
    sack_delivered: u32,
}

/// The SACK blocks of a segment's options, at most four.
fn parse_sack(seg: &[u8], data_offset: usize, out: &mut [(u32, u32); TCP_NUM_SACKS]) -> usize {
    let mut n = 0;
    let mut i = HEADER_LEN;
    while i < data_offset {
        let kind = seg[i];
        if kind == 0 { break; }
        if kind == 1 { i += 1; continue; }
        if i + 1 >= data_offset { break; }
        let len = seg[i + 1] as usize;
        if len < 2 || i + len > data_offset { break; }
        if kind == 5 && len >= 10 && (len - 2).is_multiple_of(8) {
            for b in seg[i + 2..i + len].chunks_exact(8).take(TCP_NUM_SACKS) {
                out[n] = (u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                          u32::from_be_bytes([b[4], b[5], b[6], b[7]]));
                n += 1;
            }
            break;
        }
        i += len;
    }
    n
}

/// `tcp_is_sackblock_valid`: inside what is outstanding; a D-SACK may
/// also lie below `snd_una`, down to `undo_marker`.
fn is_sackblock_valid(c: &TcpConn, is_dsack: bool, start: u32, end: u32) -> bool {
    if after(end, c.snd_nxt) || !before(start, end) { return false; }
    if !before(start, c.snd_nxt) { return false; }
    if after(start, c.snd_una) { return true; }
    let Some(marker) = c.tx.undo_marker.filter(|_| is_dsack) else { return false };
    if after(end, c.snd_una) { return false; }
    if !before(start, marker) { return true; }
    if !after(end, marker) { return false; }
    // Crossing the marker (overestimated).
    !before(start, end.wrapping_sub(c.max_window))
}

/// `tcp_check_dsack` and `tcp_dsack_seen`: is the first block a D-SACK
/// (RFC 2883)?
fn check_dsack(c: &mut TcpConn, ack: u32, sp: &[(u32, u32)], prior_una: u32, st: &mut SackState) -> bool {
    let (s0, e0) = sp[0];
    if !before(s0, ack) {
        if sp.len() < 2 { return false; }
        let (s1, e1) = sp[1];
        if after(e0, e1) || before(s0, s1) { return false; }
    }
    if !before(s0, e0) { return false; }
    let seq_len = e0.wrapping_sub(s0);
    if seq_len > c.max_window { return false; }
    let mss = (c.mss_cache as u32).max(1);
    let dup_segs = if seq_len > mss {
        seq_len.div_ceil(mss)
    } else {
        if c.tx.tlp_high_seq == Some(e0) { st.flag |= FLAG_DSACK_TLP; }
        1
    };
    c.tx.dsack_dups = c.tx.dsack_dups.wrapping_add(dup_segs);
    // Not what we retransmitted: dubious.
    if c.tx.dsack_dups > c.tx.total_retrans { return false; }
    // A D-SACK after reordering was seen, not one for a loss probe: maybe
    // reordering beyond RACK's window made a retransmission needless.
    if c.tx.reord_seen != 0 && st.flag & FLAG_DSACK_TLP == 0 { c.tx.rack.dsack_seen = true; }
    st.flag |= FLAG_DSACKING_ACK;
    st.sack_delivered += dup_segs;
    // A D-SACK for data already forgotten: count it against the
    // retransmissions.
    if let Some(marker) = c.tx.undo_marker
        && c.tx.undo_retrans > 0 && !after(e0, prior_una) && after(e0, marker)
    {
        c.tx.undo_retrans = (c.tx.undo_retrans - dup_segs as i32).max(0);
    }
    true
}

/// `tcp_sacktag_one`.
fn sacktag_one(c: &mut TcpConn, st: &mut SackState, i: usize, dup_sack: bool, now_us: u64) {
    let TxSeg { seq: start_seq, end: end_seq, sent_us, mut sacked, .. } = c.tx.queue[i];
    // A D-SACK for a retransmission.
    if dup_sack && sacked & RETRANS != 0 {
        if let Some(marker) = c.tx.undo_marker
            && c.tx.undo_retrans > 0 && after(end_seq, marker)
        {
            c.tx.undo_retrans = (c.tx.undo_retrans - 1).max(0);
        }
        if sacked & SACKED_ACKED != 0 && before(start_seq, st.reord) { st.reord = start_seq; }
    }
    // Acked already; about to be dropped.
    if !after(end_seq, c.snd_una) { return; }

    if sacked & SACKED_ACKED == 0 {
        rack_advance(c, sacked, end_seq, sent_us, now_us);
        if sacked & SACKED_RETRANS != 0 {
            // Not marked lost: the retransmission is believed in flight.
            if sacked & LOST != 0 {
                sacked &= !(LOST | SACKED_RETRANS);
                c.tx.lost_out -= 1;
                c.tx.retrans_out -= 1;
            }
        } else {
            if sacked & RETRANS == 0 {
                // A hole filled by an original transmission: reordering.
                if before(start_seq, highest_sack_seq(c)) && before(start_seq, st.reord) {
                    st.reord = start_seq;
                }
                if !after(end_seq, c.high_seq) { st.flag |= FLAG_ORIG_SACK_ACKED; }
                if st.first_sackt == 0 { st.first_sackt = sent_us; }
                st.last_sackt = sent_us;
            }
            if sacked & LOST != 0 {
                sacked &= !LOST;
                c.tx.lost_out -= 1;
            }
        }
        sacked |= SACKED_ACKED;
        st.flag |= FLAG_DATA_SACKED;
        c.tx.sacked_out += 1;
        st.sack_delivered += 1;
    }
    if dup_sack && sacked & SACKED_RETRANS != 0 {
        sacked &= !SACKED_RETRANS;
        c.tx.retrans_out -= 1;
    }
    c.tx.queue[i].sacked = sacked;
}

/// `tcp_sacktag_write_queue`.
fn sacktag_write_queue(c: &mut TcpConn, ack: u32, blocks: &[(u32, u32)], prior_una: u32,
                       st: &mut SackState, now_us: u64) {
    st.flag = 0;
    st.reord = c.snd_nxt;
    if c.tx.sacked_out == 0 { c.tx.highest_sack = c.snd_una; }
    let found_dup = check_dsack(c, ack, blocks, prior_una, st);
    if before(ack, prior_una.wrapping_sub(c.max_window)) {
        st.flag = 0;
        return;
    }
    if c.tx.packets_out == 0 { return; }

    let mut sp: [(u32, u32, bool); TCP_NUM_SACKS] = [(0, 0, false); TCP_NUM_SACKS];
    let mut used = 0;
    for (i, &(s, e)) in blocks.iter().enumerate() {
        let dup = i == 0 && found_dup;
        if !is_sackblock_valid(c, dup, s, e) { continue; }
        if !after(e, prior_una) { continue; }
        sp[used] = (s, e, dup);
        used += 1;
    }
    let sp = &mut sp[..used];
    sp.sort_unstable_by_key(|b| b.0.wrapping_sub(prior_una));

    for &(start, end, dup) in sp.iter() {
        let mut i = 0;
        while i < c.tx.queue.len() {
            let s = &c.tx.queue[i];
            if !before(s.seq, end) { break; }
            // tcp_match_skb_to_sack with one packet per skb: wholly inside.
            if !after(start, s.seq) && !before(end, s.end) {
                sacktag_one(c, st, i, dup, now_us);
                let seg = c.tx.queue[i];
                rate_skb_delivered(c, &seg, &mut st.rate);
                if seg.sacked & SACKED_ACKED != 0 { c.tx.queue[i].tx_delivered_mstamp = 0; }
                let s = &c.tx.queue[i];
                if s.sacked & SACKED_ACKED != 0 && !before(s.seq, highest_sack_seq(c)) {
                    c.tx.highest_sack = s.end;
                }
            }
            i += 1;
        }
    }
    if c.ca_state != CaState::Loss || c.tx.undo_marker.is_some() {
        check_sack_reordering(c, st.reord);
    }
}

/// `tcp_ack_update_window`, then `snd_una` moves.
fn ack_update_window(c: &mut TcpConn, ack: u32, ack_seq: u32, nwin: u32, now: u64) -> u32 {
    let mut flag = 0;
    // tcp_may_update_window (RFC 9293 §3.10.7.4 SND.WL1/WL2).
    if after(ack, c.snd_una)
        || after(ack_seq, c.snd_wl1)
        || (ack_seq == c.snd_wl1 && (nwin > c.snd_wnd || nwin == 0))
    {
        flag |= FLAG_WIN_UPDATE;
        c.snd_wl1 = ack_seq;
        if c.snd_wnd != nwin {
            c.snd_wnd = nwin;
            if c.send_buf.len() > c.snd_nxt.wrapping_sub(c.snd_una) as usize {
                slow_start_after_idle_check(c, now);
            }
            // A larger window may lift the half-window bound on the MSS.
            if nwin > c.max_window {
                c.max_window = nwin;
                c.sync_mss();
            }
        }
    }
    c.snd_una = ack;
    flag
}

/// `tcp_ack_update_rtt`: the timed segment's RTT, else a SACKed one's,
/// else the echoed timestamp; none if retransmitted data is acknowledged
/// (Karn).
#[allow(clippy::too_many_arguments)]
fn update_rtt(c: &mut TcpConn, flag: u32, seq_rtt: Option<u64>, sack_rtt: Option<u64>,
                  mut ca_rtt: Option<u64>, tsecr: Option<u32>, ts_now_ms: u32, now: u64)
                  -> (bool, Option<u64>) {
    let mut seq_rtt = seq_rtt.or(sack_rtt);
    if seq_rtt.is_none() && c.ts_ok && flag & FLAG_ACKED != 0
        && let Some(t) = tsecr.filter(|&t| t != 0)
    {
        // tcp_rtt_tsopt_us
        let delta = ts_now_ms.wrapping_sub(t);
        if delta < i32::MAX as u32 / 1000 {
            seq_rtt = Some(delta.max(1) as u64 * 1000);
            ca_rtt = seq_rtt;
        }
    }
    let Some(m) = seq_rtt else { return (false, ca_rtt) };
    // tcp_update_rtt_min: a suspected delayed ACK would raise the minimum.
    let ca = ca_rtt.unwrap_or(m);
    if !(flag & FLAG_ACK_MAYBE_DELAYED != 0 && ca > c.tx.min_rtt() as u64) {
        rtt_min_sample(c, ca, now);
    }
    c.ack_update_rtt(m.max(1));
    (true, ca_rtt)
}

/// `tcp_update_rtt_min`.
pub(super) fn rtt_min_sample(c: &mut TcpConn, rtt_us: u64, now: u64) {
    let v = rtt_us.min(u32::MAX as u64) as u32;
    c.tx.rtt_min.running_min(MIN_RTT_WLEN, now as u32, if v == 0 { TICK_US as u32 } else { v });
}

/// `tcp_clean_rtx_queue`: drop what the ACK covers.
#[allow(clippy::too_many_arguments)]
fn clean_rtx_queue(c: &mut TcpConn, prior_fack: u32, prior_una: u32, st: &mut SackState,
                   tsecr: Option<u32>, ts_now_ms: u32, now: u64, now_us: u64) -> u32 {
    let prior_sacked = c.tx.sacked_out;
    let mut flag = 0;
    let mut reord = c.snd_nxt;
    let mut first_ackt = 0u64;
    let mut last_ackt = 0u64;
    let mut pkts_acked = 0u32;
    while let Some(s) = c.tx.queue.front() {
        if after(s.end, c.snd_una) { break; }
        let s = c.tx.queue.pop_front().unwrap();
        let sacked = s.sacked;
        if sacked & RETRANS != 0 {
            if sacked & SACKED_RETRANS != 0 { c.tx.retrans_out -= 1; }
            flag |= FLAG_RETRANS_DATA_ACKED;
        } else if sacked & SACKED_ACKED == 0 {
            last_ackt = s.sent_us;
            if first_ackt == 0 { first_ackt = last_ackt; }
            if before(s.seq, reord) { reord = s.seq; }
            if !after(s.end, c.high_seq) { flag |= FLAG_ORIG_SACK_ACKED; }
        }
        if sacked & SACKED_ACKED != 0 {
            c.tx.sacked_out -= 1;
        } else if !is_reno(c) {
            c.tx.delivered = c.tx.delivered.wrapping_add(1);
            // tcp_skb_spurious_retrans: the echo predates the retransmission.
            let spurious = sacked & RETRANS != 0 && c.ts_ok
                && tsecr.is_some_and(|t| t != 0 && before(t, (s.sent_us / 1000) as u32));
            if !spurious { rack_advance(c, sacked, s.end, s.sent_us, now_us); }
        }
        if sacked & LOST != 0 { c.tx.lost_out -= 1; }
        c.tx.packets_out -= 1;
        pkts_acked += 1;
        rate_skb_delivered(c, &s, &mut st.rate);
        flag |= FLAG_DATA_ACKED;
    }
    // A segment acknowledged in part keeps only what is left (what
    // `tcp_trim_head` does before a retransmission).
    if let Some(h) = c.tx.queue.front_mut() {
        if after(c.snd_una, h.seq) { h.seq = c.snd_una; }
        if h.sacked & SACKED_ACKED != 0 { flag |= FLAG_SACK_RENEGING; }
    }

    let mut seq_rtt = None;
    let mut ca_rtt = None;
    let mut sack_rtt = None;
    if first_ackt != 0 && flag & FLAG_RETRANS_DATA_ACKED == 0 {
        seq_rtt = Some(now_us.saturating_sub(first_ackt));
        ca_rtt = Some(now_us.saturating_sub(last_ackt));
        // A lone runt over the round trip to a receiver without
        // out-of-order data: likely a delayed ACK.
        if pkts_acked == 1 && prior_sacked == 0
            && c.snd_una.wrapping_sub(prior_una) < c.mss_cache as u32
            && st.rate.prior_delivered.wrapping_add(1) == c.tx.delivered
            && flag & FLAG_CA_ALERT == 0
        {
            flag |= FLAG_ACK_MAYBE_DELAYED;
        }
    }
    if st.first_sackt != 0 {
        sack_rtt = Some(now_us.saturating_sub(st.first_sackt));
        ca_rtt = Some(now_us.saturating_sub(st.last_sackt));
    }
    let (rtt_update, rs_rtt) = update_rtt(c, flag, seq_rtt, sack_rtt, ca_rtt, tsecr, ts_now_ms, now);

    if flag & FLAG_ACKED != 0 {
        flag |= FLAG_SET_XMIT_TIMER;
        if is_reno(c) {
            remove_reno_sacks(c, pkts_acked);
            // Without SACK bits an ACK covering a retransmission cannot
            // tell which copy arrived.
            if flag & FLAG_RETRANS_DATA_ACKED != 0 { flag &= !FLAG_ORIG_SACK_ACKED; }
        } else if before(reord, prior_fack) {
            // A hole filled by an original transmission: reordering.
            check_sack_reordering(c, reord);
        }
    } else if let (Some(h), Some(sack_rtt)) = (c.tx.queue.front(), sack_rtt) {
        // Rearm only when the SACKed data left before the head's last
        // (re)transmission, so the RTO does not slide in recovery.
        if rtt_update && sack_rtt > now_us.saturating_sub(h.sent_us) {
            flag |= FLAG_SET_XMIT_TIMER;
        }
    }
    // pkts_acked
    cubic::acked(c, rs_rtt, now, now_us);
    flag
}

/// `tcp_xmit_recovery`. F-RTO's new data first; retransmissions only if
/// it could not go.
fn xmit_recovery<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>, rexmit: Rexmit) {
    if rexmit == Rexmit::None { return; }
    if rexmit == Rexmit::New {
        write_xmit(c, io);
        if after(c.snd_nxt, c.high_seq) { return; }
        c.tx.frto = false;
    }
    xmit_retransmit_queue(c, io);
}

/// `tcp_ack` for an ACK in a synchronized state; `ack` is at most
/// `snd_nxt`. `has_data`: the segment carries data or a FIN.
#[allow(clippy::too_many_arguments)]
pub(super) fn ack<E: Env + ?Sized>(c: &mut TcpConn, io: &mut Io<E>, seg: &[u8], data_offset: usize,
                                   ack: u32, ack_seq: u32, nwin: u32, has_data: bool,
                                   tsval: Option<u32>, now_s: u64) {
    let now = io.ticks();
    let now_us = io.env.uptime_us();
    let ts_now = io.ts_now_ms();
    let prior_una = c.snd_una;
    let prior_packets = c.tx.packets_out;
    let delivered0 = c.tx.delivered;
    let lost0 = c.tx.lost;
    let mut flag = 0;
    let mut rexmit = Rexmit::None;
    let mut num_dupack = 0;
    c.tx.rcv_tsecr = if c.ts_ok { parse_tsecr(seg, data_offset) } else { None };
    let mut st = SackState {
        rate: RateSample::default(), flag: 0, reord: c.snd_nxt, first_sackt: 0, last_sackt: 0, sack_delivered: 0,
    };
    let is_sack_reneg = c.tx.is_sack_reneg;
    let mut blocks = [(0u32, 0u32); TCP_NUM_SACKS];
    let nblocks = if c.sack_ok { parse_sack(seg, data_offset, &mut blocks) } else { 0 };
    let blocks = &blocks[..nblocks];

    if before(ack, prior_una) {
        // An old ACK may still carry fresh SACK information.
        if !blocks.is_empty() {
            sacktag_write_queue(c, ack, blocks, prior_una, &mut st, now_us);
            flag |= st.flag;
            c.tx.delivered = c.tx.delivered.wrapping_add(st.sack_delivered);
            fastretrans_alert(c, prior_una, num_dupack, &mut flag, &mut rexmit, now, now_us);
            xmit_recovery(c, io, rexmit);
        }
        return;
    }

    if after(ack, prior_una) {
        flag |= FLAG_SND_UNA_ADVANCED;
        c.retries = 0;
    }
    c.replace_ts_recent(ack_seq, tsval, now_s);
    let prior_fack = if c.sack_ok { highest_sack_seq(c) } else { prior_una };
    if has_data { flag |= FLAG_DATA; }
    flag |= ack_update_window(c, ack, ack_seq, nwin, now);
    // The send buffer starts at `snd_una`.
    let acked = (ack.wrapping_sub(prior_una) as usize).min(c.send_buf.len());
    c.send_buf.drain(..acked);
    if !blocks.is_empty() {
        sacktag_write_queue(c, ack, blocks, prior_una, &mut st, now_us);
        flag |= st.flag;
    }
    c.tx.delivered = c.tx.delivered.wrapping_add(st.sack_delivered);

    c.probes_out = 0;
    c.last_ack_rx = now;
    if prior_packets == 0 {
        if flag & FLAG_DSACKING_ACK != 0 {
            fastretrans_alert(c, prior_una, num_dupack, &mut flag, &mut rexmit, now, now_us);
        }
        process_tlp_ack(c, ack, flag, now);
        return;
    }

    let tsecr = parse_tsecr(seg, data_offset);
    flag |= clean_rtx_queue(c, prior_fack, prior_una, &mut st, tsecr, ts_now, now, now_us);
    rack_update_reo_wnd(c, &st.rate);
    process_tlp_ack(c, ack, flag, now);

    // tcp_ack_is_dubious
    if flag & FLAG_NOT_DUP == 0 || flag & FLAG_CA_ALERT != 0 || c.ca_state != CaState::Open {
        if flag & (FLAG_SND_UNA_ADVANCED | FLAG_NOT_DUP | FLAG_DSACKING_ACK) == 0 {
            num_dupack = 1;
            io.st.dupacks_seen.fetch_add(1, Ordering::Relaxed);
        }
        fastretrans_alert(c, prior_una, num_dupack, &mut flag, &mut rexmit, now, now_us);
    }
    if flag & FLAG_SET_XMIT_TIMER != 0 { set_xmit_timer(c, now, now_us); }

    let delivered = c.tx.delivered.wrapping_sub(delivered0);
    let lost = c.tx.lost.wrapping_sub(lost0);
    rate_gen(c, delivered, &mut st.rate, is_sack_reneg, now_us);
    cong_control(c, delivered, lost, flag, now);
    xmit_recovery(c, io, rexmit);

    // tcp_check_space -> tcp_new_space: room was made for a blocked writer.
    if flag & FLAG_SND_UNA_ADVANCED != 0 && c.tx.nospace {
        c.tx.nospace = false;
        if c.tx.in_flight() < c.snd_cwnd {
            sndbuf_expand(c);
            c.tx.snd_cwnd_stamp = now;
        }
    }
}
