//! The DHCPv4 client, after systemd v262 `sd-dhcp-client.c` and
//! `dhcp-client-send.c` (RFC 2131): INIT, SELECTING, REQUESTING and BOUND,
//! INIT-REBOOT when it held an address, RENEWING at T1 by unicast,
//! REBINDING at T2 by broadcast, back to INIT when the lease expires or a
//! server says NAK. The caller sends what `Out` holds, delivers what
//! arrives on port 68, and calls `tick` by `next_deadline`. Time is
//! microseconds on a clock that does not jump.
//!
//! Not ported: BOOTP, RFC 7844 anonymity, RELEASE, IPv6-only preferred,
//! hostname, vendor and user class options, DUID client identifiers (the
//! identifier is the MAC, systemd's `ClientIdentifier=mac`).

pub mod lease;
pub mod message;

use alloc::vec::Vec;
pub use lease::{Lease, Route};
use message::{Message, BOOTREPLY, BOOTREQUEST};

pub const USEC_PER_SEC: u64 = 1_000_000;
/// `USEC_INFINITY`.
pub const INFINITY: u64 = u64::MAX;

pub const OPT_SUBNET_MASK: u8 = 1;
pub const OPT_ROUTER: u8 = 3;
pub const OPT_DOMAIN_NAME_SERVER: u8 = 6;
pub const OPT_HOST_NAME: u8 = 12;
pub const OPT_DOMAIN_NAME: u8 = 15;
pub const OPT_MTU_INTERFACE: u8 = 26;
pub const OPT_BROADCAST: u8 = 28;
pub const OPT_STATIC_ROUTE: u8 = 33;
pub const OPT_NTP_SERVER: u8 = 42;
pub const OPT_REQUESTED_IP_ADDRESS: u8 = 50;
pub const OPT_LEASE_TIME: u8 = 51;
pub const OPT_MESSAGE_TYPE: u8 = 53;
pub const OPT_SERVER_IDENTIFIER: u8 = 54;
pub const OPT_PARAMETER_REQUEST_LIST: u8 = 55;
pub const OPT_MAXIMUM_MESSAGE_SIZE: u8 = 57;
pub const OPT_RENEWAL_TIME: u8 = 58;
pub const OPT_REBINDING_TIME: u8 = 59;
pub const OPT_CLIENT_IDENTIFIER: u8 = 61;
pub const OPT_RAPID_COMMIT: u8 = 80;
pub const OPT_CLASSLESS_STATIC_ROUTE: u8 = 121;
pub const OPT_PRIVATE_CLASSLESS_STATIC_ROUTE: u8 = 249;

pub const DHCP_DISCOVER: u8 = 1;
pub const DHCP_OFFER: u8 = 2;
pub const DHCP_REQUEST: u8 = 3;
pub const DHCP_DECLINE: u8 = 4;
pub const DHCP_ACK: u8 = 5;
pub const DHCP_NAK: u8 = 6;

const RESTART_AFTER_NAK_MIN_USEC: u64 = USEC_PER_SEC;
const RESTART_AFTER_NAK_MAX_USEC: u64 = 30 * 60 * USEC_PER_SEC;
const MAX_REQUEST_ATTEMPTS_ON_REBOOTING: u64 = 2;
const MAX_REQUEST_ATTEMPTS: u64 = 5;
const TRANSIENT_FAILURE_ATTEMPTS: u64 = 3;
/// `IPV4_MIN_REASSEMBLY_SIZE`.
const IPV4_MIN_REASSEMBLY_SIZE: u32 = 576;

/// `default_req_opts`, and what networkd adds for `UseMTU=` and `UseRoutes=`.
const REQ_OPTS: [u8; 8] = [
    OPT_SUBNET_MASK, OPT_ROUTER, OPT_DOMAIN_NAME_SERVER, OPT_HOST_NAME,
    OPT_DOMAIN_NAME, OPT_MTU_INTERFACE, OPT_STATIC_ROUTE, OPT_CLASSLESS_STATIC_ROUTE,
];

/// `DHCPState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Stopped,
    Init,
    Selecting,
    InitReboot,
    Rebooting,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Stopped => "stopped",
            State::Init => "init",
            State::Selecting => "selecting",
            State::InitReboot => "init-reboot",
            State::Rebooting => "rebooting",
            State::Requesting => "requesting",
            State::Bound => "bound",
            State::Renewing => "renewing",
            State::Rebinding => "rebinding",
        }
    }
}

/// `SD_DHCP_CLIENT_EVENT_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// An offer was taken; `lease()` holds it.
    Selecting,
    /// A first lease; `lease()` holds it.
    IpAcquire,
    /// A renewed lease with another address, mask or router.
    IpChange,
    /// The same binding, renewed.
    Renew,
    /// The lease is gone: drop its address and everything from it.
    Expired,
    Stop,
    TransientFailure,
}

/// Where a message goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// From 0.0.0.0 to the link-layer and IP broadcast (`client_send_raw`).
    Broadcast,
    /// From the leased address to the server (`client_send_udp`).
    Server([u8; 4]),
}

/// Which replies the client listens for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rx {
    None,
    /// Every datagram to port 68 (the packet socket).
    Raw,
    /// Only those to the leased address (the UDP socket of RENEWING).
    Udp,
}

#[derive(Default)]
pub struct Out {
    pub send: Vec<(Dest, Vec<u8>)>,
    pub events: Vec<Event>,
}

/// `RFC2131_RANDOM_FUZZ`: -1 to +1 s.
fn fuzz(rnd: &mut impl FnMut() -> u64) -> i64 {
    (rnd() % (2 * USEC_PER_SEC)) as i64 - USEC_PER_SEC as i64
}

/// `usec_sub_signed`.
fn sub_signed(t: u64, delta: i64) -> u64 {
    if delta < 0 {
        t.saturating_add(delta.unsigned_abs())
    } else if t == INFINITY {
        INFINITY
    } else {
        t.saturating_sub(delta as u64)
    }
}

/// `client_compute_request_timeout`: 4 s doubling to 64 s, ±1 s
/// (RFC 2131 §4.1); `attempt` starts at 1.
fn request_timeout(attempt: u64, rnd: &mut impl FnMut() -> u64) -> u64 {
    let t = (1u64 << (attempt + 1).min(6)) * USEC_PER_SEC;
    sub_signed(t, fuzz(rnd))
}

/// `client_compute_reacquisition_timeout` (RFC 2131 §4.4.5).
fn reacquisition_timeout(now: u64, expire: u64) -> u64 {
    (expire.saturating_sub(now) / 2).max(60 * USEC_PER_SEC)
}

/// `usec_to_be16_sec`.
fn usec_to_sec16(t: u64) -> u16 {
    if t == INFINITY {
        u16::MAX
    } else if t >= (u16::MAX as u64 - 1) * USEC_PER_SEC {
        u16::MAX - 1
    } else {
        t.div_ceil(USEC_PER_SEC) as u16
    }
}

/// `sd_dhcp_client`.
pub struct Client {
    state: State,
    hw: [u8; 6],
    /// The interface MTU, for option 57.
    mtu: u32,
    rapid_commit: bool,
    request_broadcast: bool,
    last_addr: [u8; 4],
    xid: u32,
    start_time: u64,
    start_delay: u64,
    t1_time: u64,
    t2_time: u64,
    expire_time: u64,
    discover_attempt: u64,
    request_attempt: u64,
    max_discover_attempts: u64,
    lease: Option<Lease>,
    rx: Rx,
    timeout_resend: Option<u64>,
    timeout_t1: Option<u64>,
    timeout_t2: Option<u64>,
    timeout_expire: Option<u64>,
}

impl Client {
    /// `sd_dhcp_client_new` with networkd's defaults (`RapidCommit=yes`).
    pub fn new(hw: [u8; 6], mtu: u32) -> Self {
        Client {
            state: State::Stopped,
            hw,
            mtu,
            rapid_commit: true,
            request_broadcast: false,
            last_addr: [0; 4],
            xid: 0,
            start_time: 0,
            start_delay: 0,
            t1_time: 0,
            t2_time: 0,
            expire_time: 0,
            discover_attempt: 0,
            request_attempt: 0,
            max_discover_attempts: u64::MAX,
            lease: None,
            rx: Rx::None,
            timeout_resend: None,
            timeout_t1: None,
            timeout_t2: None,
            timeout_expire: None,
        }
    }

    pub fn state(&self) -> State { self.state }
    pub fn hw(&self) -> [u8; 6] { self.hw }
    pub fn lease(&self) -> Option<&Lease> { self.lease.as_ref() }
    pub fn rx(&self) -> Rx { self.rx }
    pub fn xid(&self) -> u32 { self.xid }
    pub fn is_running(&self) -> bool { self.state != State::Stopped }

    /// T1, T2 and expiry of the bound lease, absolute.
    pub fn lease_times(&self) -> Option<(u64, u64, u64)> {
        self.timeout_expire.map(|_| (self.t1_time, self.t2_time, self.expire_time))
    }

    /// `sd_dhcp_client_set_request_address`: the address to ask for again.
    pub fn set_request_address(&mut self, a: Option<[u8; 4]>) {
        debug_assert!(!self.is_running());
        self.last_addr = a.unwrap_or([0; 4]);
    }

    /// `sd_dhcp_client_set_mtu`: the MTU may change while bound.
    pub fn set_mtu(&mut self, mtu: u32) {
        if mtu >= 68 {
            self.mtu = mtu;
        }
    }

    /// The earliest timer, absolute.
    pub fn next_deadline(&self) -> Option<u64> {
        [self.timeout_resend, self.timeout_t1, self.timeout_t2, self.timeout_expire]
            .into_iter().flatten().min()
    }

    fn set_state(&mut self, state: State) {
        if self.state == state {
            return;
        }
        self.state = state;
        match state {
            // The next DISCOVER is the first of a new cycle.
            State::Stopped | State::Bound => self.discover_attempt = 0,
            // The next REQUEST is the first in this state.
            State::Rebooting | State::Requesting | State::Renewing | State::Rebinding =>
                self.request_attempt = 0,
            _ => {}
        }
    }

    fn disable_event_sources(&mut self) {
        self.rx = Rx::None;
        self.timeout_resend = None;
        self.timeout_t1 = None;
        self.timeout_t2 = None;
        self.timeout_expire = None;
    }

    /// `client_initialize`.
    fn initialize(&mut self) {
        self.disable_event_sources();
        self.set_state(State::Stopped);
        self.xid = 0;
        self.lease = None;
    }

    /// `client_stop`.
    fn stop_with(&mut self, out: &mut Out) {
        if self.is_running() {
            out.events.push(Event::Stop);
        }
        self.initialize();
    }

    /// `sd_dhcp_client_stop`.
    pub fn stop(&mut self, out: &mut Out) {
        self.stop_with(out);
    }

    /// `client_start_delayed`.
    fn start_delayed(&mut self, now: u64, rnd: &mut impl FnMut() -> u64) {
        self.disable_event_sources();
        self.lease = None;
        self.xid = rnd() as u32;
        self.start_time = now;
        if self.state != State::InitReboot {
            self.set_state(State::Init);
        }
        self.timeout_resend = Some(now.saturating_add(self.start_delay));
    }

    fn start_now(&mut self, now: u64, rnd: &mut impl FnMut() -> u64) {
        self.start_delay = 0;
        self.start_delayed(now, rnd);
    }

    /// `sd_dhcp_client_start`: INIT-REBOOT when an address is to be asked
    /// for again (RFC 7844 §3.3). The first message goes out on `tick`.
    pub fn start(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        if self.last_addr != [0; 4] {
            self.set_state(State::InitReboot);
        }
        self.start_now(now, rnd);
        self.tick(now, rnd, out);
    }

    /// `client_restart`: after a NAK or a REQUEST nobody answered.
    fn restart(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        if matches!(self.state, State::Bound | State::Renewing | State::Rebinding) {
            out.events.push(Event::Expired);
        }
        // On reboot the network may have changed: start over at once.
        if self.state == State::Rebooting {
            self.start_now(now, rnd);
            return;
        }
        self.start_delay = (self.start_delay * 2)
            .clamp(RESTART_AFTER_NAK_MIN_USEC, RESTART_AFTER_NAK_MAX_USEC);
        self.start_delayed(now, rnd);
    }

    /// `sd_dhcp_client_send_renew`: renew a bound lease now.
    pub fn send_renew(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        if self.state != State::Bound {
            return;
        }
        self.set_state(State::Renewing);
        self.start_delay = 0;
        self.timeout_resend = Some(now);
        self.tick(now, rnd, out);
    }

    /// `sd_dhcp_client_send_decline`: the address is in use (RFC 2131
    /// §3.1.5); tell the server and start over.
    pub fn send_decline(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        if !self.is_running() || self.lease.is_none() {
            return;
        }
        self.send_message(DHCP_DECLINE, now, out);
        self.restart(now, rnd, out);
        self.tick(now, rnd, out);
    }

    /// Fire every timer that is due, earliest first.
    pub fn tick(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        loop {
            let due = [
                (self.timeout_expire, 0),
                (self.timeout_t2, 1),
                (self.timeout_t1, 2),
                (self.timeout_resend, 3),
            ].into_iter()
                .filter_map(|(t, k)| t.filter(|&t| t <= now).map(|t| (t, k)))
                .min();
            let Some((_, k)) = due else { return };
            match k {
                0 => {
                    self.timeout_expire = None;
                    self.timeout_expire_fired(now, rnd, out);
                }
                1 => {
                    self.timeout_t2 = None;
                    // Unicast replies from RENEWING are not taken any more.
                    self.rx = Rx::None;
                    self.set_state(State::Rebinding);
                    self.timeout_resend_fired(now, rnd, out);
                }
                2 => {
                    self.timeout_t1 = None;
                    self.set_state(State::Renewing);
                    self.timeout_resend_fired(now, rnd, out);
                }
                _ => {
                    self.timeout_resend = None;
                    self.timeout_resend_fired(now, rnd, out);
                }
            }
        }
    }

    /// `client_timeout_expire`.
    fn timeout_expire_fired(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        out.events.push(Event::Expired);
        self.start_now(now, rnd);
    }

    /// `client_timeout_resend`.
    fn timeout_resend_fired(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let next = match self.state {
            State::Renewing => reacquisition_timeout(now, self.t2_time),
            State::Rebinding => reacquisition_timeout(now, self.expire_time),
            State::Init | State::Selecting => {
                self.set_state(State::Selecting);
                if self.discover_attempt >= self.max_discover_attempts {
                    self.stop_with(out);
                    return;
                }
                self.discover_attempt += 1;
                request_timeout(self.discover_attempt, rnd)
            }
            State::InitReboot | State::Rebooting => {
                self.set_state(State::Rebooting);
                // Slightly faster than REQUESTING, for a quick reboot.
                if self.request_attempt >= MAX_REQUEST_ATTEMPTS_ON_REBOOTING {
                    self.restart(now, rnd, out);
                    return;
                }
                self.request_attempt += 1;
                request_timeout(self.request_attempt, rnd) / 4
            }
            State::Requesting => {
                // RFC 2131 §3.1.5: neither ACK nor NAK after the
                // retransmissions, back to INIT.
                if self.request_attempt >= MAX_REQUEST_ATTEMPTS {
                    self.restart(now, rnd, out);
                    return;
                }
                self.request_attempt += 1;
                request_timeout(self.request_attempt, rnd)
            }
            State::Stopped | State::Bound => return,
        };
        self.timeout_resend = Some(now.saturating_add(next));
        match self.state {
            State::Selecting => {
                self.send_message(DHCP_DISCOVER, now, out);
                if self.discover_attempt >= TRANSIENT_FAILURE_ATTEMPTS {
                    out.events.push(Event::TransientFailure);
                }
            }
            _ => self.send_message(DHCP_REQUEST, now, out),
        }
    }

    /// `client_new_message`.
    fn new_message(&self, kind: u8, now: u64) -> Message {
        let mut m = Message::new(BOOTREQUEST, self.xid, &self.hw);
        // Some servers refuse a lease while 'secs' is zero.
        let elapsed = now.saturating_sub(self.start_time);
        m.secs = usec_to_sec16(if elapsed == 0 { USEC_PER_SEC } else { elapsed });
        // RFC 2131 §4.1: a hint to broadcast replies, off by default.
        m.set_broadcast_flag(self.request_broadcast);
        let lease_addr = self.lease.as_ref().map_or([0; 4], |l| l.address);
        let server = self.lease.as_ref().map_or([0; 4], |l| l.server_address);

        let _ = m.append_option(OPT_MESSAGE_TYPE, &[kind]);
        if (kind == DHCP_REQUEST && self.state == State::Requesting) || kind == DHCP_DECLINE {
            let _ = m.append_option(OPT_SERVER_IDENTIFIER, &server);
        }
        let mut client_id = [0u8; 7];
        client_id[0] = message::ARPHRD_ETHER;
        client_id[1..].copy_from_slice(&self.hw);
        let _ = m.append_option(OPT_CLIENT_IDENTIFIER, &client_id);

        // RFC 2131 §4.3.2 (SELECTING there means REQUESTING).
        let addr = match kind {
            DHCP_DISCOVER => self.last_addr,
            DHCP_REQUEST => match self.state {
                State::Requesting => lease_addr,
                State::Rebooting => self.last_addr,
                _ => {
                    m.ciaddr = lease_addr;
                    [0; 4]
                }
            },
            _ => lease_addr,
        };
        if addr != [0; 4] {
            let _ = m.append_option(OPT_REQUESTED_IP_ADDRESS, &addr);
        }
        if kind == DHCP_DECLINE {
            return m;
        }

        // RFC 2131 §3.5: the same list in DISCOVER and REQUEST, sorted.
        let mut prl = REQ_OPTS;
        prl.sort_unstable();
        let _ = m.append_option(OPT_PARAMETER_REQUEST_LIST, &prl);
        // RFC 2131 §3.5, RFC 3442: at least the interface MTU.
        let max = self.mtu.clamp(IPV4_MIN_REASSEMBLY_SIZE, u16::MAX as u32) as u16;
        let _ = m.append_option(OPT_MAXIMUM_MESSAGE_SIZE, &max.to_be_bytes());
        if self.rapid_commit && kind == DHCP_DISCOVER {
            let _ = m.append_option(OPT_RAPID_COMMIT, &[]);
        }
        m
    }

    /// `dhcp_client_send_message`.
    fn send_message(&mut self, kind: u8, now: u64, out: &mut Out) {
        let Ok(bytes) = self.new_message(kind, now).build() else { return };
        let (dest, rx) = match kind {
            DHCP_REQUEST if self.state == State::Renewing => {
                let Some(l) = &self.lease else { return };
                if l.address == [0; 4] {
                    return;
                }
                (Dest::Server(l.server_address), Rx::Udp)
            }
            DHCP_DECLINE => (Dest::Broadcast, Rx::None),
            _ => (Dest::Broadcast, Rx::Raw),
        };
        self.rx = rx;
        out.send.push((dest, bytes));
    }

    /// `dhcp_client_parse_message`: the message type, and the lease an
    /// OFFER or ACK carries.
    fn parse_message(&self, b: &[u8]) -> Option<(u8, Option<Lease>)> {
        let m = Message::parse(b, BOOTREPLY, Some(self.xid), Some(&self.hw)).ok()?;
        let kind = m.option_u8(OPT_MESSAGE_TYPE)?;
        match kind {
            DHCP_OFFER => {
                if self.state != State::Selecting {
                    return None;
                }
                Some((DHCP_OFFER, Some(Lease::from_message(&m, 0)?)))
            }
            DHCP_ACK => {
                match self.state {
                    State::Selecting => {
                        if !self.rapid_commit || m.option_exact(OPT_RAPID_COMMIT, 0).is_none() {
                            return None;
                        }
                    }
                    State::Rebooting | State::Requesting | State::Renewing | State::Rebinding => {}
                    _ => return None,
                }
                Some((DHCP_ACK, Some(Lease::from_message(&m, 0)?)))
            }
            DHCP_NAK => {
                // A NAK answers a REQUEST only, and always names its server.
                if !matches!(self.state,
                    State::Rebooting | State::Requesting | State::Renewing | State::Rebinding)
                {
                    return None;
                }
                let server = m.option_addr(OPT_SERVER_IDENTIFIER)?;
                if self.lease.as_ref().is_some_and(|l| l.server_address != server) {
                    return None;
                }
                Some((DHCP_NAK, None))
            }
            _ => None,
        }
    }

    /// `client_receive_message`: a datagram to port 68. `to_lease_addr`:
    /// it was addressed to the leased address, which is all the RENEWING
    /// socket sees.
    pub fn receive(&mut self, b: &[u8], to_lease_addr: bool, now: u64,
                   rnd: &mut impl FnMut() -> u64, out: &mut Out)
    {
        match self.rx {
            Rx::None => return,
            Rx::Udp if !to_lease_addr => return,
            _ => {}
        }
        let Some((kind, lease)) = self.parse_message(b) else { return };
        match kind {
            DHCP_OFFER => {
                let mut lease = lease.unwrap();
                lease.timestamp = now;
                self.lease = Some(lease);
                out.events.push(Event::Selecting);
                self.enter_requesting(now);
            }
            DHCP_ACK => {
                let mut lease = lease.unwrap();
                lease.timestamp = now;
                self.enter_bound(lease, now, rnd, out);
            }
            _ => self.restart(now, rnd, out),
        }
        self.tick(now, rnd, out);
    }

    /// `client_enter_requesting`.
    fn enter_requesting(&mut self, now: u64) {
        self.disable_event_sources();
        self.set_state(State::Requesting);
        self.timeout_resend = Some(now);
    }

    /// `client_enter_bound`.
    fn enter_bound(&mut self, lease: Lease, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let event = match self.state {
            State::Renewing | State::Rebinding => {
                if self.lease.as_ref().is_some_and(|l| l.same_binding(&lease)) {
                    Event::Renew
                } else {
                    Event::IpChange
                }
            }
            _ => Event::IpAcquire,
        };
        self.lease = Some(lease);
        self.disable_event_sources();
        self.start_delay = 0;
        self.set_state(State::Bound);
        self.last_addr = self.lease.as_ref().unwrap().address;
        self.set_lease_timeouts(now, rnd);
        out.events.push(event);
    }

    /// `client_set_lease_timeouts`: T1 and T2 with ±1 s (RFC 2131 §4.4.5),
    /// none for a lease that never ends.
    fn set_lease_timeouts(&mut self, now: u64, rnd: &mut impl FnMut() -> u64) {
        let l = self.lease.as_ref().unwrap();
        if l.lifetime == INFINITY {
            self.timeout_t1 = None;
            self.timeout_t2 = None;
            self.timeout_expire = None;
            return;
        }
        self.expire_time = l.timestamp.saturating_add(l.lifetime);
        self.t1_time = l.timestamp.saturating_add(l.t1);
        self.t2_time = l.timestamp.saturating_add(l.t2);
        self.t1_time = sub_signed(self.t1_time, fuzz(rnd));
        self.t2_time = sub_signed(self.t2_time, fuzz(rnd));
        self.t2_time = self.t2_time.max(self.t1_time);

        self.timeout_expire = Some(self.expire_time);
        if self.expire_time <= now {
            return;
        }
        self.timeout_t2 = Some(self.t2_time);
        if self.t2_time <= now {
            return;
        }
        self.timeout_t1 = Some(self.t1_time);
    }
}
