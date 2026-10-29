//! Neighbours: IPv4 next hops and their link-layer addresses.
//!
//! Ported from Linux `net/core/neighbour.c` (the NUD state machine,
//! `__neigh_event_send`, `neigh_timer_handler`, `__neigh_update`,
//! `neigh_invalidate`, the garbage collector) and `net/ipv4/arp.c`
//! (`arp_rcv`, `arp_process`, `arp_solicit`, the parameters of `arp_tbl`).
//! A packet to a next hop whose address is not known waits in that
//! neighbour's queue until ARP answers, and is reported back if it never
//! does; it is never sent to the link-layer broadcast address.
//!
//! Time is in 100 Hz ticks, so Linux's `HZ` values carry over unchanged.
//!
//! Not ported: proxy ARP, permanent and NOARP entries, `arp_accept`,
//! `arp_ignore`/`arp_announce`/`arp_filter` other than their defaults,
//! application probes, and the hardware header cache.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

const HZ: u64 = 100;

// `arp_tbl.parms` (arp.c).
const MCAST_PROBES: u32 = 3;
const UCAST_PROBES: u32 = 3;
const RETRANS_TIME: u64 = HZ;
const BASE_REACHABLE_TIME: u64 = 30 * HZ;
const DELAY_PROBE_TIME: u64 = 5 * HZ;
const GC_STALETIME: u64 = 60 * HZ;
/// `SK_WMEM_DEFAULT`.
const QUEUE_LEN_BYTES: usize = 212_992;
const LOCKTIME: u64 = HZ;
const GC_INTERVAL: u64 = 30 * HZ;
const GC_THRESH1: usize = 128;
const GC_THRESH2: usize = 512;
const GC_THRESH3: usize = 1024;

const ARPOP_REQUEST: u16 = 1;
const ARPOP_REPLY: u16 = 2;
const ARPHRD_ETHER: u16 = 1;
const ARPHRD_IEEE802: u16 = 6;
const ETH_P_IP: u16 = 0x0800;

/// Neighbour Unreachability Detection states (`NUD_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nud {
    None,
    Incomplete,
    Reachable,
    Stale,
    Delay,
    Probe,
    Failed,
}

impl Nud {
    /// `NUD_VALID`: the address may be used.
    pub fn valid(self) -> bool {
        matches!(self, Nud::Reachable | Nud::Stale | Nud::Delay | Nud::Probe)
    }
    /// `NUD_CONNECTED`: the address is confirmed.
    fn connected(self) -> bool {
        self == Nud::Reachable
    }
    /// `NUD_IN_TIMER`.
    fn in_timer(self) -> bool {
        matches!(self, Nud::Incomplete | Nud::Reachable | Nud::Delay | Nud::Probe)
    }
    pub fn name(self) -> &'static str {
        match self {
            Nud::None => "NONE",
            Nud::Incomplete => "INCOMPLETE",
            Nud::Reachable => "REACHABLE",
            Nud::Stale => "STALE",
            Nud::Delay => "DELAY",
            Nud::Probe => "PROBE",
            Nud::Failed => "FAILED",
        }
    }
}

/// A packet waiting for its next hop.
pub trait Queued {
    /// What it counts against `QUEUE_LEN_BYTES` (`skb->truesize`).
    fn truesize(&self) -> usize;
}

/// What a call into the table leaves to do once its lock is let go: the
/// packets to send, the ARP requests, the packets that will never be sent
/// because their next hop did not answer (`arp_error_report`).
pub struct Out<P> {
    pub send: Vec<([u8; 6], P)>,
    /// A request for the address, to the given link-layer address or to
    /// broadcast.
    pub solicit: Vec<([u8; 4], Option<[u8; 6]>)>,
    pub errors: Vec<P>,
}

impl<P> Default for Out<P> {
    fn default() -> Self { Out { send: Vec::new(), solicit: Vec::new(), errors: Vec::new() } }
}

struct Neighbour<P> {
    state: Nud,
    ha: [u8; 6],
    /// When the address was last confirmed reachable.
    confirmed: u64,
    /// When the state or the address last changed.
    updated: u64,
    /// When a packet last went to it through the slow path.
    used: u64,
    probes: u32,
    /// When the timer fires, if it runs.
    timer: Option<u64>,
    queue: VecDeque<P>,
    queue_bytes: usize,
}

/// What an ARP packet asks of us.
pub enum Reply {
    /// Send an ARP reply to `dst_ha` for the request of `tip`'s owner.
    Answer { dst_ha: [u8; 6], dst_ip: [u8; 4] },
}

/// The ARP table (`arp_tbl`).
pub struct Table<P> {
    neighs: BTreeMap<[u8; 4], Neighbour<P>>,
    /// `parms->reachable_time`, drawn anew every 300 s.
    reachable_time: u64,
    last_rand: u64,
    last_flush: u64,
    next_gc: u64,
    /// Packets dropped because the table was full or the queue overflowed.
    pub drops: u64,
}

/// `neigh_rand_reach_time`: uniformly in [base/2, 3*base/2).
fn rand_reach_time(rnd: u64, base: u64) -> u64 {
    if base == 0 { 0 } else { (rnd % base) + (base >> 1) }
}

/// `neigh_max_probes`.
fn max_probes(state: Nud) -> u32 {
    UCAST_PROBES + if state == Nud::Probe { 0 } else { MCAST_PROBES }
}

fn is_multicast(ip: [u8; 4]) -> bool { ip[0] & 0xf0 == 0xe0 }
fn is_loopback(ip: [u8; 4]) -> bool { ip[0] == 127 }

impl<P: Queued> Table<P> {
    pub const fn new() -> Self {
        Table {
            neighs: BTreeMap::new(),
            reachable_time: BASE_REACHABLE_TIME,
            last_rand: 0,
            last_flush: 0,
            next_gc: GC_INTERVAL,
            drops: 0,
        }
    }

    /// The address of `ip`, if it may be used.
    pub fn lookup(&self, ip: [u8; 4]) -> Option<[u8; 6]> {
        self.neighs.get(&ip).filter(|n| n.state.valid()).map(|n| n.ha)
    }

    /// `neigh_alloc` + `___neigh_create`: a new entry in NUD_NONE, unless
    /// the table stays full after a forced collection.
    fn create(&mut self, now: u64, ip: [u8; 4]) -> bool {
        if self.neighs.contains_key(&ip) { return true; }
        let entries = self.neighs.len();
        if (entries >= GC_THRESH3 || (entries >= GC_THRESH2 && now > self.last_flush + 5 * HZ))
            && self.forced_gc(now) == 0 && entries >= GC_THRESH3
        {
            return false;
        }
        self.neighs.insert(ip, Neighbour {
            state: Nud::None, ha: [0; 6], confirmed: now, updated: now, used: now,
            probes: 0, timer: None, queue: VecDeque::new(), queue_bytes: 0,
        });
        true
    }

    /// `neigh_forced_gc`: drop FAILED entries and those not updated for
    /// 5 s, outside a timer, down to `GC_THRESH2`.
    fn forced_gc(&mut self, now: u64) -> usize {
        let max_clean = self.neighs.len().saturating_sub(GC_THRESH2);
        let tref = now.saturating_sub(5 * HZ);
        let victims: Vec<[u8; 4]> = self.neighs.iter()
            .filter(|(_, n)| n.state == Nud::Failed || !(tref..=now).contains(&n.updated))
            .filter(|(_, n)| n.timer.is_none() && n.queue.is_empty())
            .map(|(ip, _)| *ip)
            .take(max_clean)
            .collect();
        for ip in &victims { self.neighs.remove(ip); }
        self.last_flush = now;
        victims.len()
    }

    /// `ip_finish_output2` → `neigh_output`: send `p` to the next hop `ip`,
    /// or keep it until `ip` answers.
    pub fn output(&mut self, out: &mut Out<P>, now: u64, ip: [u8; 4], p: P) {
        if !self.create(now, ip) {
            self.drops += 1;
            return;
        }
        let n = self.neighs.get_mut(&ip).unwrap();
        if n.state.connected() {
            let ha = n.ha;
            out.send.push((ha, p));
            return;
        }
        // `neigh_resolve_output` → `neigh_event_send_probe`.
        n.used = now;
        if let Some(p) = self.event_send(out, now, ip, Some(p)) {
            let ha = self.neighs[&ip].ha;
            out.send.push((ha, p));
        }
    }

    /// Start resolving `ip` without a packet to send, as a caller that is
    /// about to need it does (`neigh_event_send` with no skb). Answers
    /// whether a request went out.
    pub fn resolve(&mut self, out: &mut Out<P>, now: u64, ip: [u8; 4]) -> bool {
        if !self.create(now, ip) { return false; }
        let before = self.neighs[&ip].probes;
        let _ = self.event_send(out, now, ip, None);
        self.neighs.get(&ip).is_some_and(|n| n.probes != before)
    }

    /// `__neigh_event_send`. Hands `p` back when it may go out now.
    fn event_send(&mut self, out: &mut Out<P>, now: u64, ip: [u8; 4], p: Option<P>) -> Option<P> {
        let n = self.neighs.get_mut(&ip)?;
        if matches!(n.state, Nud::Reachable | Nud::Delay | Nud::Probe) {
            return p;
        }
        let mut immediate_probe = false;
        if !matches!(n.state, Nud::Stale | Nud::Incomplete) {
            n.probes = UCAST_PROBES;
            n.state = Nud::Incomplete;
            n.updated = now;
            immediate_probe = true;
            n.timer = Some(now + RETRANS_TIME.max(HZ / 100));
        } else if n.state == Nud::Stale {
            n.state = Nud::Delay;
            n.updated = now;
            n.timer = Some(now + DELAY_PROBE_TIME);
        }
        let mut now_p = None;
        if n.state == Nud::Incomplete {
            if let Some(p) = p {
                while n.queue_bytes + p.truesize() > QUEUE_LEN_BYTES {
                    let Some(old) = n.queue.pop_front() else { break };
                    n.queue_bytes -= old.truesize();
                    self.drops += 1;
                }
                n.queue_bytes += p.truesize();
                n.queue.push_back(p);
            }
        } else {
            now_p = p;
        }
        if immediate_probe {
            self.probe(out, ip);
        }
        now_p
    }

    /// `neigh_probe` → `arp_solicit`: unicast to the cached address for the
    /// first `UCAST_PROBES` probes of a revalidation, broadcast otherwise.
    fn probe(&mut self, out: &mut Out<P>, ip: [u8; 4]) {
        let Some(n) = self.neighs.get_mut(&ip) else { return };
        let dst = (n.probes < UCAST_PROBES).then_some(n.ha);
        n.probes += 1;
        out.solicit.push((ip, dst));
    }

    /// `neigh_invalidate`: report and drop what waited for `ip`.
    fn invalidate(&mut self, out: &mut Out<P>, now: u64, ip: [u8; 4]) {
        let Some(n) = self.neighs.get_mut(&ip) else { return };
        n.updated = now;
        let q = core::mem::take(&mut n.queue);
        n.queue_bytes = 0;
        out.errors.extend(q);
    }

    /// `__neigh_update` with `new` REACHABLE or STALE and an address.
    fn update(&mut self, out: &mut Out<P>, now: u64, ip: [u8; 4], ha: [u8; 6], mut new: Nud, overr: bool) {
        let reachable_time = self.reachable_time;
        let Some(n) = self.neighs.get_mut(&ip) else { return };
        let old = n.state;
        let same_ha = old.valid() && n.ha == ha;
        if new.connected() { n.confirmed = now; }
        if old.valid() {
            if !same_ha && !overr { return; }
            if same_ha && new == Nud::Stale { new = old; }
        }
        if new != old || !same_ha { n.updated = now; }
        if new != old {
            n.timer = None;
            if new == Nud::Probe { n.probes = 0; }
            if new.in_timer() {
                n.timer = Some(now + if new == Nud::Reachable { reachable_time } else { 0 });
            }
            n.state = new;
        }
        if !same_ha {
            n.ha = ha;
            if !new.connected() {
                n.confirmed = now.saturating_sub(BASE_REACHABLE_TIME << 1);
            }
        }
        if new == old { return; }
        if !old.valid() {
            let q = core::mem::take(&mut n.queue);
            n.queue_bytes = 0;
            let ha = n.ha;
            for p in q { out.send.push((ha, p)); }
        }
    }

    /// `neigh_timer_handler` for every expired timer, and the periodic
    /// collection (`neigh_periodic_work`).
    pub fn tick(&mut self, out: &mut Out<P>, now: u64, rnd: u64) {
        let due: Vec<[u8; 4]> = self.neighs.iter()
            .filter(|(_, n)| n.timer.is_some_and(|t| t <= now))
            .map(|(ip, _)| *ip)
            .collect();
        for ip in due { self.timer(out, now, ip); }
        if now >= self.next_gc {
            self.periodic(now, rnd);
            self.next_gc = now + (BASE_REACHABLE_TIME >> 1);
        }
    }

    fn timer(&mut self, out: &mut Out<P>, now: u64, ip: [u8; 4]) {
        let reachable_time = self.reachable_time;
        let Some(n) = self.neighs.get_mut(&ip) else { return };
        n.timer = None;
        let state = n.state;
        let mut next = now + HZ;
        if !state.in_timer() { return; }
        match state {
            Nud::Reachable => {
                if now <= n.confirmed + reachable_time {
                    next = n.confirmed + reachable_time;
                } else if now <= n.used + DELAY_PROBE_TIME {
                    n.state = Nud::Delay;
                    n.updated = now;
                    next = now + DELAY_PROBE_TIME;
                } else {
                    n.state = Nud::Stale;
                    n.updated = now;
                }
            }
            Nud::Delay => {
                if now <= n.confirmed + DELAY_PROBE_TIME {
                    n.state = Nud::Reachable;
                    n.updated = now;
                    next = n.confirmed + reachable_time;
                } else {
                    n.state = Nud::Probe;
                    n.updated = now;
                    n.probes = 0;
                    next = now + RETRANS_TIME.max(HZ / 100);
                }
            }
            _ => next = now + RETRANS_TIME.max(HZ / 100),
        }
        if matches!(n.state, Nud::Incomplete | Nud::Probe) && n.probes >= max_probes(n.state) {
            n.state = Nud::Failed;
            self.invalidate(out, now, ip);
            return;
        }
        if n.state.in_timer() {
            n.timer = Some(next.max(now + HZ / 100));
        }
        if matches!(n.state, Nud::Incomplete | Nud::Probe) {
            self.probe(out, ip);
        }
    }

    /// `neigh_periodic_work`.
    fn periodic(&mut self, now: u64, rnd: u64) {
        if now > self.last_rand + 300 * HZ || self.last_rand == 0 {
            self.last_rand = now.max(1);
            self.reachable_time = rand_reach_time(rnd, BASE_REACHABLE_TIME);
        }
        if self.neighs.len() < GC_THRESH1 { return; }
        self.neighs.retain(|_, n| {
            if n.state.in_timer() { return true; }
            if n.used < n.confirmed && n.confirmed <= now { n.used = n.confirmed; }
            !(n.state == Nud::Failed || now >= n.used + GC_STALETIME)
        });
    }

    /// `arp_rcv` + `arp_process` for an ARP packet (the Ethernet payload).
    /// `to_us`: the frame was addressed to our link-layer address
    /// (`PACKET_HOST`). `our_ip` is 0 while unconfigured.
    pub fn arp_input(&mut self, out: &mut Out<P>, now: u64, arp: &[u8], to_us: bool, our_ip: [u8; 4])
        -> Option<Reply>
    {
        // arp_rcv: the header, then hlen 6 and plen 4.
        if arp.len() < 8 { return None; }
        let hrd = u16::from_be_bytes([arp[0], arp[1]]);
        let pro = u16::from_be_bytes([arp[2], arp[3]]);
        let (hln, pln) = (arp[4], arp[5]);
        let op = u16::from_be_bytes([arp[6], arp[7]]);
        if hln != 6 || pln != 4 || arp.len() < 28 { return None; }
        // arp_process: Ethernet accepts hardware types 1 and 6.
        if (hrd != ARPHRD_ETHER && hrd != ARPHRD_IEEE802) || pro != ETH_P_IP { return None; }
        if op != ARPOP_REPLY && op != ARPOP_REQUEST { return None; }
        let sha: [u8; 6] = arp[8..14].try_into().unwrap();
        let sip: [u8; 4] = arp[14..18].try_into().unwrap();
        let tha: [u8; 6] = arp[18..24].try_into().unwrap();
        let tip: [u8; 4] = arp[24..28].try_into().unwrap();

        if is_multicast(tip) || is_loopback(tip) { return None; }
        let local = |ip: [u8; 4]| our_ip != [0; 4] && ip == our_ip;

        // Duplicate address detection (RFC 2131, RFC 5227): answer a probe
        // for our address, learn nothing from it.
        if sip == [0; 4] {
            return (op == ARPOP_REQUEST && local(tip))
                .then_some(Reply::Answer { dst_ha: sha, dst_ip: sip });
        }

        // A request for our address: whoever asks will talk to us. Its
        // sender must be a valid source, or `ip_route_input_noref` refuses it
        // (a martian, or our own address: `fib_validate_source`).
        let martian = is_multicast(sip) || is_loopback(sip) || sip == [255; 4] || local(sip);
        if op == ARPOP_REQUEST && local(tip) && !martian {
            // neigh_event_ns
            if self.create(now, sip) {
                self.update(out, now, sip, sha, Nud::Stale, true);
            }
            return Some(Reply::Answer { dst_ha: sha, dst_ip: sip });
        }

        // Otherwise only an entry we already have is updated; unsolicited
        // ARP creates none (`arp_accept` 0).
        let Some(n) = self.neighs.get(&sip) else { return None };
        // arp_is_garp: sip == tip, a reply's target address its own, from a
        // unicast address.
        let is_garp = sip == tip
            && (op != ARPOP_REPLY || tha == sha)
            && !local(sip) && sip != [255; 4] && !is_multicast(sip);
        // Back-to-back replies: the first one wins for LOCKTIME.
        let overr = now > n.updated + LOCKTIME || is_garp;
        // Broadcast replies and requests do not assert reachability.
        let state = if op == ARPOP_REPLY && to_us { Nud::Reachable } else { Nud::Stale };
        self.update(out, now, sip, sha, state, overr);
        None
    }

    /// `neigh_changeaddr` / `neigh_carrier_down`: forget every entry, and
    /// what waited in them.
    pub fn flush(&mut self) {
        self.neighs.clear();
    }

    /// (address, link-layer address, state, seconds since the last change)
    pub fn entries(&self, now: u64) -> Vec<([u8; 4], [u8; 6], Nud, u64)> {
        self.neighs.iter().map(|(ip, n)| (*ip, n.ha, n.state, now.saturating_sub(n.updated) / HZ)).collect()
    }
}

impl<P: Queued> Default for Table<P> {
    fn default() -> Self { Self::new() }
}
