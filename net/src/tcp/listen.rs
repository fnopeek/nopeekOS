//! Passive open, after Linux 6.18: a SYN to a listener becomes a request
//! sock (`tcp_conn_request`) answered by a SYN-ACK with the options the
//! SYN offered; the SYN-ACK is resent on a backing-off timer
//! (`reqsk_timer_handler`); the ACK that completes the handshake is
//! checked against it (`tcp_check_req`) and turns it into a connection
//! waiting in the listener's accept queue (`tcp_v4_syn_recv_sock`,
//! `tcp_create_openreq_child`) until `accept` takes it
//! (`inet_csk_accept`). When the SYN queue is full, SYN cookies
//! (`syncookies.c`) carry the request in the ISN instead.
//!
//! The cookie hash is BLAKE3 keyed by the stack's secret where Linux uses
//! SipHash.

use super::*;

/// `sysctl_tcp_synack_retries`.
const TCP_SYNACK_RETRIES: u8 = 5;
/// `SOMAXCONN`: the most a backlog may ask for.
const SOMAXCONN: u32 = 4096;
/// `TCP_SYNCOOKIE_VALID`: cookies are checked this long after the queue
/// last overflowed, in ticks.
const TCP_SYNCOOKIE_VALID: u64 = 2 * 60 * 100;
/// `MAX_SYNCOOKIE_AGE`, in minutes.
const MAX_SYNCOOKIE_AGE: u32 = 2;
const COOKIEBITS: u32 = 24;
const COOKIEMASK: u32 = (1 << COOKIEBITS) - 1;
const TS_OPT_WSCALE_MASK: u32 = 0xf;
const TS_OPT_SACK: u32 = 1 << 4;
const TSBITS: u32 = 6;
/// `msstab`.
const MSSTAB: [u16; 4] = [536, 1300, 1440, 1460];

/// The listener's queues (`icsk_accept_queue`).
#[derive(Default)]
pub(super) struct Listener {
    /// `sk_max_ack_backlog`.
    max_backlog: u32,
    /// Established connections not yet accepted.
    accept_queue: VecDeque<Handle>,
    /// When the SYN queue last overflowed into cookies (0: never).
    last_overflow: u64,
}

/// A request sock in SYN-RECEIVED (`tcp_request_sock`).
pub(super) struct Req {
    listener: Handle,
    local_ip: [u8; 4],
    rcv_isn: u32,
    snt_isn: u32,
    rcv_nxt: u32,
    /// The peer's MSS (`req->mss`) and options.
    mss: u16,
    wscale_ok: bool,
    snd_wscale: u8,
    sack_ok: bool,
    ts_ok: bool,
    ts_recent: u32,
    /// The window the SYN-ACK offered (`rsk_rcv_wnd`).
    rcv_wnd: u32,
    num_retrans: u8,
    num_timeout: u8,
    expires: u64,
    /// When the first SYN-ACK went out (µs), and its first and last TSval.
    snt_synack_us: u64,
    snt_tsval_first: u32,
    snt_tsval_last: u32,
    last_oow_ack: u64,
    /// The handshake's ACK came but the accept queue was full.
    acked: bool,
}

fn before(a: u32, b: u32) -> bool { (a.wrapping_sub(b) as i32) < 0 }
fn after(a: u32, b: u32) -> bool { before(b, a) }

/// `reqsk_timeout`: 1 s doubling per timeout, at most `TCP_RTO_MAX`.
fn reqsk_timeout(req: &Req) -> u64 {
    (TCP_TIMEOUT_INIT << req.num_timeout.min(16)).min(TCP_RTO_MAX)
}

/// `inet_csk_listen_start` with the backlog of `listen(2)`.
pub(super) fn listener(backlog: u32) -> Listener {
    Listener { max_backlog: backlog.min(SOMAXCONN), ..Listener::default() }
}

/// The SYN queue's length for `listener` (`inet_csk_reqsk_queue_len`) and
/// how many in it are young (no timeout yet).
fn queue_len(conns: &Table, listener: Handle) -> (u32, u32) {
    conns.reqs.values().filter(|r| r.listener == listener)
        .fold((0, 0), |(q, y), r| (q + 1, y + (r.num_timeout == 0) as u32))
}

/// The SYN's options, as `tcp_parse_options` takes them for a request.
struct SynOpts { mss: u16, wscale: Option<u8>, sack_ok: bool, tsval: Option<u32> }

fn syn_opts_of(seg: &[u8], doff: usize) -> SynOpts {
    SynOpts {
        mss: parse_mss(seg, doff).unwrap_or(TCP_MSS_DEFAULT),
        wscale: parse_wscale(seg, doff).map(|w| w.min(TCP_MAX_WSCALE)),
        sack_ok: parse_sack_permitted(seg, doff),
        tsval: parse_ts(seg, doff),
    }
}

/// `cookie_hash`.
fn cookie_hash(secret: &[u8; 32], (sport, saddr, dport): (u16, [u8; 4], u16), daddr: [u8; 4],
               count: u32, c: u8) -> u32 {
    let mut buf = [0u8; 18];
    buf[0..4].copy_from_slice(&saddr);
    buf[4..8].copy_from_slice(&daddr);
    buf[8..10].copy_from_slice(&sport.to_be_bytes());
    buf[10..12].copy_from_slice(&dport.to_be_bytes());
    buf[12..16].copy_from_slice(&count.to_be_bytes());
    buf[16] = c;
    buf[17] = b'c';
    let h = blake3::keyed_hash(secret, &buf);
    let b = h.as_bytes();
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// `tcp_cookie_time`: minutes.
fn cookie_time(now: u64) -> u32 { (now / (60 * 100)) as u32 }

/// `secure_tcp_syn_cookie`. `peer` is (their port, their address, our port).
fn syn_cookie(secret: &[u8; 32], peer: (u16, [u8; 4], u16), daddr: [u8; 4], sseq: u32, data: u32,
              now: u64) -> u32 {
    let count = cookie_time(now);
    cookie_hash(secret, peer, daddr, 0, 0)
        .wrapping_add(sseq)
        .wrapping_add(count << COOKIEBITS)
        .wrapping_add(cookie_hash(secret, peer, daddr, count, 1).wrapping_add(data) & COOKIEMASK)
}

/// `check_tcp_syn_cookie`: the data, or `None` if too old or forged.
fn check_syn_cookie(secret: &[u8; 32], cookie: u32, peer: (u16, [u8; 4], u16), daddr: [u8; 4],
                    sseq: u32, now: u64) -> Option<u32> {
    let count = cookie_time(now);
    let cookie = cookie.wrapping_sub(cookie_hash(secret, peer, daddr, 0, 0).wrapping_add(sseq));
    let diff = count.wrapping_sub(cookie >> COOKIEBITS) & (u32::MAX >> COOKIEBITS);
    if diff >= MAX_SYNCOOKIE_AGE { return None; }
    Some(cookie.wrapping_sub(cookie_hash(secret, peer, daddr, count.wrapping_sub(diff), 1)) & COOKIEMASK)
}

/// `cookie_init_timestamp`: the SYN's window scale and SACK in the low
/// bits of our TSval.
fn cookie_timestamp(req: &Req, ts_now: u32) -> u32 {
    let mut options = if req.wscale_ok { req.snd_wscale as u32 } else { TS_OPT_WSCALE_MASK };
    if req.sack_ok { options |= TS_OPT_SACK; }
    let mut ts = (ts_now >> TSBITS) << TSBITS | options;
    if ts > ts_now { ts = ts.wrapping_sub(1 << TSBITS); }
    ts
}

/// `tcp_v4_send_synack` with `tcp_synack_options`: MSS, then SACK-permitted
/// and the timestamp, then the window scale, as Linux lays them out.
fn send_synack<E: Env + ?Sized>(io: &mut Io<E>, req: &mut Req, (lport, rip, rport): Tuple,
                                advmss: u16, tsval: Option<u32>) -> bool {
    if req.local_ip != io.env.our_ip() { return false; }
    let mut o = [0u8; 40];
    let mut n = 0;
    o[n..n + 4].copy_from_slice(&[2, 4, (advmss >> 8) as u8, advmss as u8]);
    n += 4;
    if req.ts_ok {
        let tsval = tsval.unwrap_or_else(|| io.ts_now_ms());
        if req.sack_ok {
            o[n..n + 4].copy_from_slice(&[4, 2, 8, 10]);
        } else {
            o[n..n + 4].copy_from_slice(&[1, 1, 8, 10]);
        }
        o[n + 4..n + 8].copy_from_slice(&tsval.to_be_bytes());
        o[n + 8..n + 12].copy_from_slice(&req.ts_recent.to_be_bytes());
        n += 12;
        if req.snt_tsval_first == 0 { req.snt_tsval_first = tsval; }
        req.snt_tsval_last = tsval;
    }
    if req.wscale_ok {
        o[n..n + 4].copy_from_slice(&[1, 3, 3, OUR_WSCALE]);
        n += 4;
    }
    if req.sack_ok && !req.ts_ok {
        o[n..n + 4].copy_from_slice(&[1, 1, 4, 2]);
        n += 4;
    }
    if req.snt_synack_us == 0 { req.snt_synack_us = io.env.uptime_us().max(1); }
    let w = req.rcv_wnd.min(65535) as u16;
    send_segment_with_opts(io, rip, lport, rport, req.snt_isn, req.rcv_nxt, SYN | ACK, w, &[], &o[..n], 0)
}

/// The MSS we announce towards `dst` (`tcp_advmss`).
fn advmss_for<E: Env + ?Sized>(env: &E, dst: [u8; 4]) -> u16 {
    (env.mtu(dst).saturating_sub(HEADERS).max(256)) as u16
}

/// `tcp_conn_request`: a SYN for `listener`. `tw_isn` is the ISN a
/// TIME-WAIT entry hands over.
#[allow(clippy::too_many_arguments)]
pub(super) fn conn_request<E: Env + ?Sized>(conns: &mut Table, io: &mut Io<E>, secret: &[u8; 32],
                                            listener: Handle, key: Tuple, seg: &[u8], doff: usize,
                                            seq: u32, tw_isn: Option<u32>) {
    let now = io.ticks();
    let Some(l) = conns.get(listener).and_then(|c| c.listen.as_ref()) else { return };
    let (qlen, _) = queue_len(conns, listener);
    let max_backlog = l.max_backlog;
    let accept_full = l.accept_queue.len() as u32 > max_backlog;
    // Queue full: answer with a cookie and keep nothing.
    let want_cookie = qlen > max_backlog;
    if accept_full { return; }

    let mut o = syn_opts_of(seg, doff);
    if want_cookie && o.tsval.is_none() {
        o = SynOpts { mss: o.mss, wscale: None, sack_ok: false, tsval: None };
    }
    let (lport, rip, rport) = key;
    let local_ip = io.env.our_ip();
    let mut req = Req {
        listener,
        local_ip,
        rcv_isn: seq,
        snt_isn: 0,
        rcv_nxt: seq.wrapping_add(1),
        mss: o.mss,
        wscale_ok: o.wscale.is_some(),
        snd_wscale: o.wscale.unwrap_or(0),
        sack_ok: o.sack_ok,
        ts_ok: o.tsval.is_some(),
        ts_recent: o.tsval.unwrap_or(0),
        rcv_wnd: INITIAL_WINDOW as u32,
        num_retrans: 0,
        num_timeout: 0,
        expires: now + TCP_TIMEOUT_INIT,
        snt_synack_us: 0,
        snt_tsval_first: 0,
        snt_tsval_last: 0,
        last_oow_ack: 0,
        acked: false,
    };
    let advmss = advmss_for(&*io.env, rip);
    if want_cookie {
        // cookie_init_sequence with tcp_synq_overflow.
        if let Some(l) = conns.get_mut(listener).and_then(|c| c.listen.as_mut()) { l.last_overflow = now.max(1); }
        let mssind = (1..MSSTAB.len()).rev().find(|&i| req.mss >= MSSTAB[i]).unwrap_or(0);
        req.mss = MSSTAB[mssind];
        req.snt_isn = syn_cookie(secret, (rport, rip, lport), local_ip, seq, mssind as u32, now);
        let ts = req.ts_ok.then(|| cookie_timestamp(&req, io.ts_now_ms()));
        send_synack(io, &mut req, key, advmss, ts);
        return;
    }
    req.snt_isn = tw_isn.unwrap_or_else(|| generate_isn(secret, io.env.uptime_us(), local_ip, rip, lport, rport));
    send_synack(io, &mut req, key, advmss, None);
    conns.reqs.insert(key, req);
}

/// What `check_req` decided.
pub(super) enum Checked {
    /// Done with the segment.
    Drop,
    /// Answer as the listener would: a reset.
    Listener,
    /// The new connection, to process the segment further.
    Child(Handle),
}

/// `tcp_check_req`: a segment for a request sock.
#[allow(clippy::too_many_arguments)]
pub(super) fn check_req<E: Env + ?Sized>(conns: &mut Table, io: &mut Io<E>, key: Tuple, seg: &[u8],
                                         doff: usize, flags: u8, seq: u32, end_seq: u32, ack: u32,
                                         adv_window: u16) -> Checked {
    let now = io.ticks();
    let now_s = io.env.uptime_us() / 1_000_000;
    let Some(req) = conns.reqs.get_mut(&key) else { return Checked::Drop };
    let rst = flags & RST != 0;
    let mut flg = flags & (RST | SYN | ACK);
    let has_data = end_seq != seq && flags & SYN == 0;

    let tsval = parse_ts(seg, doff);
    let mut tsecr_reject = false;
    let mut paws_reject = false;
    if let Some(v) = tsval {
        if let Some(e) = parse_tsecr(seg, doff).filter(|&e| e != 0)
            && req.ts_ok
        {
            tsecr_reject = e.wrapping_sub(req.snt_tsval_first) > req.snt_tsval_last.wrapping_sub(req.snt_tsval_first);
        }
        let stamp = now_s.saturating_sub(reqsk_timeout(req) / 100);
        let ok = (req.ts_recent.wrapping_sub(v) as i32) <= 0 || req.ts_recent == 0;
        paws_reject = !ok && !(rst && now_s >= stamp + 60);
    }

    // The SYN again: our SYN-ACK was lost.
    if seq == req.rcv_isn && flg == SYN && !paws_reject {
        let limited = req.last_oow_ack != 0 && now.saturating_sub(req.last_oow_ack) < TCP_INVALID_RATELIMIT;
        if !limited {
            req.last_oow_ack = now.max(1);
            let advmss = advmss_for(&*io.env, key.1);
            if send_synack(io, req, key, advmss, None) {
                req.num_retrans = req.num_retrans.saturating_add(1);
                req.expires = now + reqsk_timeout(req);
            }
        }
        return Checked::Drop;
    }
    // An ACK of anything but our SYN-ACK: the listener resets it.
    if flg & ACK != 0 && !rst && ack != req.snt_isn.wrapping_add(1) {
        return Checked::Listener;
    }
    if paws_reject || tsecr_reject
        || !in_window(seq, end_seq, req.rcv_nxt, req.rcv_nxt.wrapping_add(req.rcv_wnd.min(65535)))
    {
        let limited = !has_data && req.last_oow_ack != 0
            && now.saturating_sub(req.last_oow_ack) < TCP_INVALID_RATELIMIT;
        if !rst && !limited {
            if !has_data { req.last_oow_ack = now.max(1); }
            // tcp_v4_reqsk_send_ack
            if req.local_ip == io.env.our_ip() {
                send_segment(io, key.1, key.0, key.2, req.snt_isn.wrapping_add(1), req.rcv_nxt, ACK,
                    req.rcv_wnd.min(65535) as u16, &[], 0);
            }
        }
        return Checked::Drop;
    }
    if seq == req.rcv_isn { flg &= !SYN; }
    if flg & RST != 0 && seq != req.rcv_nxt {
        // tcp_reqsk_send_challenge_ack
        if req.local_ip == io.env.our_ip() {
            send_segment(io, key.1, key.0, key.2, req.snt_isn.wrapping_add(1), req.rcv_nxt, ACK,
                req.rcv_wnd.min(65535) as u16, &[], 0);
        }
        return Checked::Drop;
    }
    if flg & (RST | SYN) != 0 {
        // embryonic_reset
        if flg & RST == 0 {
            send_reset(io, key, flags, end_seq, ack);
        }
        conns.reqs.remove(&key);
        return Checked::Drop;
    }
    if flg & ACK == 0 { return Checked::Drop; }

    let req = conns.reqs.remove(&key).unwrap();
    let tsecr = parse_tsecr(seg, doff);
    match syn_recv_sock(conns, io, key, req, adv_window, tsval, tsecr, seq) {
        Ok(child) => Checked::Child(child),
        Err(mut req) => {
            // listen_overflow without tcp_abort_on_overflow: wait for the
            // ACK again.
            req.acked = true;
            conns.reqs.insert(key, req);
            Checked::Drop
        }
    }
}

/// `tcp_in_window`.
fn in_window(seq: u32, end_seq: u32, s_win: u32, e_win: u32) -> bool {
    if seq == s_win { return true; }
    if after(end_seq, s_win) && before(seq, e_win) { return true; }
    seq == e_win && seq == end_seq
}

/// `tcp_v4_send_reset` for a segment no connection takes.
fn send_reset<E: Env + ?Sized>(io: &mut Io<E>, (lport, rip, rport): Tuple, flags: u8, end_seq: u32, ack: u32) {
    if flags & ACK != 0 {
        send_segment(io, rip, lport, rport, ack, 0, RST, 0, &[], 0);
    } else {
        send_segment(io, rip, lport, rport, 0, end_seq, RST | ACK, 0, &[], 0);
    }
}

/// `tcp_v4_syn_recv_sock` with `tcp_create_openreq_child`: the request
/// becomes a connection in the listener's accept queue, or `None` when that
/// queue is full, which hands the request back.
#[allow(clippy::too_many_arguments)]
fn syn_recv_sock<E: Env + ?Sized>(conns: &mut Table, io: &mut Io<E>, key: Tuple, req: Req, adv_window: u16,
                                  tsval: Option<u32>, tsecr: Option<u32>, seq: u32) -> Result<Handle, Req> {
    let listener = req.listener;
    let (owner, full) = match conns.get(listener) {
        Some(l) => (l.owner, l.listen.as_ref().is_none_or(|q| q.accept_queue.len() as u32 > q.max_backlog)),
        None => return Err(req),
    };
    if full { return Err(req); }
    let (lport, rip, rport) = key;
    let now = io.ticks();
    let now_us = io.env.uptime_us();
    let now_s = now_us / 1_000_000;
    let mut c = TcpConn::blank(State::Established, lport, owner);
    c.established = true;
    c.local_ip = req.local_ip;
    c.remote_ip = rip;
    c.remote_port = rport;
    c.rcv_off = 0;
    c.rcv_nxt = req.rcv_nxt;
    c.rcv_wup.set(req.rcv_nxt);
    c.rcv_wnd.set(req.rcv_wnd);
    c.snd_iss = req.snt_isn;
    c.snd_una = req.snt_isn.wrapping_add(1);
    c.snd_nxt = c.snd_una;
    c.wscale_ok = req.wscale_ok;
    c.snd_wscale = req.snd_wscale;
    c.sack_ok = req.sack_ok;
    c.ts_ok = req.ts_ok;
    c.ts_recent = req.ts_recent;
    c.ts_recent_stamp = now_s;
    // tcp_check_req: the ACK's own TSval when it starts at rcv_nxt.
    if let Some(v) = tsval.filter(|_| req.ts_ok && !after(seq, req.rcv_nxt)) { c.ts_recent = v; }
    // The window of the ACK that completes the handshake.
    c.snd_wnd = (adv_window as u32) << c.snd_wscale;
    c.snd_wl1 = req.rcv_isn;
    c.max_window = c.snd_wnd;
    c.mss_clamp = req.mss;
    c.set_path(&*io.env);
    c.sync_mss();
    c.init_rcv_mss();
    c.inet_id.set(io.env.random_u64() as u16);
    c.last_send_tick = now;
    c.last_ack_rx = now;
    // tcp_synack_rtt_meas: from the first SYN-ACK unless it was resent.
    c.retries = req.num_retrans;
    c.syn_sent_us = req.snt_synack_us;
    c.handshake_rtt(now_us, tsecr, io.ts_now_ms());
    recovery::init_congestion_control(&mut c, now, now_us);
    let child = conns.insert(c);
    crate::mib::inc(crate::mib::Mib::TcpPassiveOpens);
    if let Some(l) = conns.get_mut(listener).and_then(|c| c.listen.as_mut()) {
        l.accept_queue.push_back(child);
    }
    Ok(child)
}

/// `cookie_v4_check`: an ACK at a listener with no request sock may answer
/// one of our cookies.
#[allow(clippy::too_many_arguments)]
pub(super) fn cookie_check<E: Env + ?Sized>(conns: &mut Table, io: &mut Io<E>, secret: &[u8; 32],
                                            listener: Handle, key: Tuple, seg: &[u8], doff: usize,
                                            flags: u8, seq: u32, ack: u32, adv_window: u16) -> Option<Handle> {
    if flags & ACK == 0 || flags & RST != 0 { return None; }
    let now = io.ticks();
    let last = conns.get(listener)?.listen.as_ref()?.last_overflow;
    // tcp_synq_no_recent_overflow
    if last == 0 || now > last + TCP_SYNCOOKIE_VALID { return None; }
    let (lport, rip, rport) = key;
    let local_ip = io.env.our_ip();
    let mssind = check_syn_cookie(secret, ack.wrapping_sub(1), (rport, rip, lport), local_ip,
                                  seq.wrapping_sub(1), now)?;
    let mss = *MSSTAB.get(mssind as usize)?;
    let tsval = parse_ts(seg, doff);
    // cookie_timestamp_decode
    let (mut wscale_ok, mut snd_wscale, mut sack_ok) = (false, 0, false);
    if tsval.is_some() {
        let options = parse_tsecr(seg, doff).unwrap_or(0);
        sack_ok = options & TS_OPT_SACK != 0;
        if options & TS_OPT_WSCALE_MASK != TS_OPT_WSCALE_MASK {
            wscale_ok = true;
            snd_wscale = (options & TS_OPT_WSCALE_MASK) as u8;
        }
    }
    let req = Req {
        listener,
        local_ip,
        rcv_isn: seq.wrapping_sub(1),
        snt_isn: ack.wrapping_sub(1),
        rcv_nxt: seq,
        mss,
        wscale_ok,
        snd_wscale: snd_wscale.min(TCP_MAX_WSCALE),
        sack_ok,
        ts_ok: tsval.is_some(),
        ts_recent: tsval.unwrap_or(0),
        rcv_wnd: INITIAL_WINDOW as u32,
        num_retrans: 0,
        num_timeout: 0,
        expires: 0,
        snt_synack_us: 0,
        snt_tsval_first: 0,
        snt_tsval_last: 0,
        last_oow_ack: 0,
        acked: false,
    };
    let tsecr = parse_tsecr(seg, doff);
    syn_recv_sock(conns, io, key, req, adv_window, tsval, tsecr, seq).ok()
}

/// `reqsk_timer_handler` for every request sock whose timer ran out: the
/// SYN-ACK again, with backoff, up to `tcp_synack_retries`; fewer when the
/// queue fills with old requests.
/// When `timers` next has a request to retransmit or drop.
pub(super) fn timers_at(conns: &Table) -> Option<u64> {
    conns.reqs.values().map(|r| r.expires).min()
}

pub(super) fn timers<E: Env + ?Sized>(conns: &mut Table, io: &mut Io<E>) {
    let now = io.ticks();
    let due: Vec<Tuple> = conns.reqs.iter().filter(|(_, r)| now >= r.expires).map(|(k, _)| *k).collect();
    for key in due {
        let Some(listener) = conns.reqs.get(&key).map(|r| r.listener) else { continue };
        let Some(max_backlog) = conns.get(listener).and_then(|c| c.listen.as_ref()).map(|l| l.max_backlog) else {
            conns.reqs.remove(&key);
            continue;
        };
        let mut max_retries = TCP_SYNACK_RETRIES;
        let (qlen, young) = queue_len(conns, listener);
        if (qlen << 1) > max_backlog.max(8) {
            let mut young = young << 1;
            while max_retries > 2 {
                if qlen < young { break; }
                max_retries -= 1;
                young <<= 1;
            }
        }
        let advmss = advmss_for(&*io.env, key.1);
        let req = conns.reqs.get_mut(&key).unwrap();
        // syn_ack_recalc without TCP_DEFER_ACCEPT.
        if req.num_timeout >= max_retries {
            conns.reqs.remove(&key);
            continue;
        }
        if send_synack(io, req, key, advmss, None) {
            req.num_retrans = req.num_retrans.saturating_add(1);
        }
        req.num_timeout += 1;
        req.expires = now + reqsk_timeout(req);
    }
}

/// For `tcp_get_info`: the accept queue's length and bound.
pub(super) fn backlog_info(l: &Listener) -> (u32, u32) { (l.accept_queue.len() as u32, l.max_backlog) }

/// `inet_csk_accept`: the oldest established connection, if any.
pub(super) fn accept(conns: &mut Table, listener: Handle) -> Result<Option<Handle>, TcpError> {
    let l = conns.get_mut(listener).and_then(|c| c.listen.as_mut()).ok_or(TcpError::NotConnected)?;
    Ok(l.accept_queue.pop_front())
}

/// `inet_csk_listen_stop`: the listener closes. Requests are dropped;
/// connections nobody accepted are reset (`tcp_disconnect`).
pub(super) fn listen_stop<E: Env + ?Sized>(conns: &mut Table, io: &mut Io<E>, listener: Handle) {
    conns.reqs.retain(|_, r| r.listener != listener);
    let queued: Vec<Handle> = conns.get_mut(listener).and_then(|c| c.listen.as_mut())
        .map(|l| l.accept_queue.drain(..).collect()).unwrap_or_default();
    for h in queued {
        if let Some(c) = conns.get_mut(h)
            && matches!(c.state, State::Established | State::CloseWait | State::SynReceived)
        {
            let w = recv_window(c, io.st);
            send_seg(c, io, c.snd_nxt, c.rcv_nxt, RST | ACK, w, &[]);
        }
        conns.remove(h);
    }
}
