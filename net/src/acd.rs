//! IPv4 address conflict detection (RFC 5227), after systemd v262
//! `sd-ipv4acd.c` and `arp-util.c`: probe an address before using it,
//! announce it twice, then defend it, once per 10 s; a second conflict
//! inside that window gives it up. The RFC's second is a "time unit",
//! a seventh of the whole probe (200 ms by default). The caller sends the
//! ARP frames `Out` holds and delivers every ARP packet that arrives.
//! Time is microseconds.

use alloc::vec::Vec;

const USEC_PER_SEC: u64 = 1_000_000;
const PROBE_WAIT: u64 = 1;
const PROBE_MIN: u64 = 1;
const PROBE_MAX: u64 = 2;
const ANNOUNCE_WAIT: u64 = 2;
const TOTAL_TIME_UNITS: u64 = 7;
const ANNOUNCE_INTERVAL_USEC: u64 = 2 * USEC_PER_SEC;
const RATE_LIMIT_INTERVAL_USEC: u64 = 60 * USEC_PER_SEC;
const DEFEND_INTERVAL_USEC: u64 = 10 * USEC_PER_SEC;
const PROBE_NUM: u32 = 3;
const ANNOUNCE_NUM: u32 = 2;
const MAX_CONFLICTS: u32 = 10;
/// `DEFAULT_ACD_TIMEOUT_USEC`.
pub const DEFAULT_TIMEOUT_USEC: u64 = 200_000;

const ARPOP_REQUEST: u16 = 1;
const ARPOP_REPLY: u16 = 2;
/// `sizeof(struct ether_arp)`.
const ARP_LEN: usize = 28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Init,
    Started,
    WaitingProbe,
    Probing,
    WaitingAnnounce,
    Announcing,
    Running,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The address is ours: no one answered the probes.
    Bind,
    /// Someone else has it; the client is stopped.
    Conflict,
}

/// An ARP request to broadcast (`arp_send_packet`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Send {
    /// Sender address 0.0.0.0, target the address.
    Probe([u8; 4]),
    /// Sender and target the address.
    Announcement([u8; 4]),
}

#[derive(Default)]
pub struct Out {
    pub send: Vec<Send>,
    pub events: Vec<Event>,
}

/// `sd_ipv4acd`.
pub struct Acd {
    state: State,
    n_iteration: u32,
    n_conflict: u32,
    time_unit_usec: u64,
    defend_window: u64,
    address: [u8; 4],
    mac: [u8; 6],
    timer: Option<u64>,
}

impl Acd {
    /// `sd_ipv4acd_new` and `sd_ipv4acd_set_timeout`: the whole probe
    /// takes up to `timeout_usec` (0 for the default).
    pub fn new(mac: [u8; 6], timeout_usec: u64) -> Self {
        let t = if timeout_usec == 0 { DEFAULT_TIMEOUT_USEC } else { timeout_usec };
        Acd {
            state: State::Init,
            n_iteration: 0,
            n_conflict: 0,
            time_unit_usec: t.clamp(1_000, 60 * USEC_PER_SEC).div_ceil(TOTAL_TIME_UNITS),
            defend_window: 0,
            address: [0; 4],
            mac,
            timer: None,
        }
    }

    pub fn state(&self) -> State { self.state }
    pub fn address(&self) -> [u8; 4] { self.address }
    pub fn next_deadline(&self) -> Option<u64> { self.timer }

    /// `sd_ipv4acd_is_running`.
    pub fn is_running(&self) -> bool { self.state != State::Init }

    /// `sd_ipv4acd_is_bound`.
    pub fn is_bound(&self) -> bool {
        matches!(self.state, State::Announcing | State::Running)
    }

    fn set_state(&mut self, st: State, reset_counter: bool) {
        if st == self.state && !reset_counter {
            self.n_iteration += 1;
        } else {
            self.state = st;
            self.n_iteration = 0;
        }
    }

    /// `ipv4acd_reset`.
    fn reset(&mut self) {
        self.timer = None;
        self.set_state(State::Init, true);
    }

    /// `sd_ipv4acd_stop`.
    pub fn stop(&mut self) {
        self.reset();
    }

    fn set_next_wakeup(&mut self, now: u64, usec: u64, random_usec: u64, rnd: &mut impl FnMut() -> u64) {
        let mut next = usec;
        if random_usec > 0 {
            next += rnd() % random_usec;
        }
        self.timer = Some(now + next);
    }

    /// `sd_ipv4acd_start` for `address`, after `sd_ipv4acd_set_address`.
    pub fn start(&mut self, address: [u8; 4], reset_conflicts: bool, now: u64,
                 rnd: &mut impl FnMut() -> u64, out: &mut Out)
    {
        debug_assert!(self.state == State::Init && address != [0; 4]);
        self.address = address;
        if reset_conflicts {
            self.n_conflict = 0;
        }
        self.set_next_wakeup(now, 0, 0, rnd);
        self.set_state(State::Started, true);
        self.tick(now, rnd, out);
    }

    /// Fire the timer if it is due.
    pub fn tick(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        while let Some(t) = self.timer.filter(|&t| t <= now) {
            self.timer = None;
            self.on_timeout(t.max(now), rnd, out);
        }
    }

    /// `ipv4acd_on_timeout`.
    fn on_timeout(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let unit = self.time_unit_usec;
        match self.state {
            State::Started => {
                self.defend_window = 0;
                self.set_state(State::WaitingProbe, true);
                if self.n_conflict >= MAX_CONFLICTS {
                    self.set_next_wakeup(now, RATE_LIMIT_INTERVAL_USEC, PROBE_WAIT * unit, rnd);
                } else {
                    self.set_next_wakeup(now, 0, PROBE_WAIT * unit, rnd);
                }
            }
            State::WaitingProbe | State::Probing => {
                out.send.push(Send::Probe(self.address));
                if self.n_iteration < PROBE_NUM - 2 {
                    self.set_state(State::Probing, false);
                    self.set_next_wakeup(now, PROBE_MIN * unit, (PROBE_MAX - PROBE_MIN) * unit, rnd);
                } else {
                    self.set_state(State::WaitingAnnounce, true);
                    self.set_next_wakeup(now, ANNOUNCE_WAIT * unit, 0, rnd);
                }
            }
            State::Announcing if self.n_iteration >= ANNOUNCE_NUM - 1 => {
                self.set_state(State::Running, false);
            }
            State::Announcing | State::WaitingAnnounce => {
                out.send.push(Send::Announcement(self.address));
                self.set_state(State::Announcing, false);
                self.set_next_wakeup(now, ANNOUNCE_INTERVAL_USEC, 0, rnd);
                if self.n_iteration == 0 {
                    self.n_conflict = 0;
                    out.events.push(Event::Bind);
                }
            }
            State::Init | State::Running => {}
        }
    }

    /// `ipv4acd_arp_conflict` (RFC 5227 §2.1.1).
    fn arp_conflict(&self, op: u16, sha: [u8; 6], spa: [u8; 4], tpa: [u8; 4], announced: bool,
                    own: &[[u8; 6]]) -> bool
    {
        if spa == self.address {
            return true;
        }
        if announced {
            return false;
        }
        // A probe for the same address from another host.
        op == ARPOP_REQUEST && spa == [0; 4] && tpa == self.address && !own.contains(&sha)
    }

    /// `ipv4acd_on_conflict`.
    fn on_conflict(&mut self, out: &mut Out) {
        self.n_conflict += 1;
        self.reset();
        out.events.push(Event::Conflict);
    }

    /// `ipv4acd_on_packet` for an ARP packet `arp` (no Ethernet header),
    /// after the filter of `arp_update_filter`. `own`: the MACs of our
    /// other interfaces.
    pub fn packet(&mut self, arp: &[u8], now: u64, own: &[[u8; 6]], out: &mut Out) {
        if !self.is_running() || arp.len() < ARP_LEN {
            return;
        }
        if arp[0..2] != [0, 1] || arp[2..4] != [8, 0] || arp[4] != 6 || arp[5] != 4 {
            return;
        }
        let op = u16::from_be_bytes([arp[6], arp[7]]);
        if op != ARPOP_REQUEST && op != ARPOP_REPLY {
            return;
        }
        let sha: [u8; 6] = arp[8..14].try_into().unwrap();
        let spa: [u8; 4] = arp[14..18].try_into().unwrap();
        let tpa: [u8; 4] = arp[24..28].try_into().unwrap();
        if sha == self.mac || (spa != self.address && tpa != self.address) {
            return;
        }
        match self.state {
            State::Announcing | State::Running => {
                if self.arp_conflict(op, sha, spa, tpa, true, own) {
                    if now > self.defend_window {
                        self.defend_window = now + DEFEND_INTERVAL_USEC;
                        out.send.push(Send::Announcement(self.address));
                    } else {
                        self.on_conflict(out);
                    }
                }
            }
            State::Started | State::WaitingProbe | State::Probing | State::WaitingAnnounce => {
                if self.arp_conflict(op, sha, spa, tpa, false, own) {
                    self.on_conflict(out);
                }
            }
            State::Init => {}
        }
    }
}
