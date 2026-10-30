//! Address configuration of the active interface: the DHCP client of
//! `net_core::dhcp`, or the `static_ip` setting. Either address is probed
//! with `net_core::acd` (RFC 5227) before it is used and defended after,
//! as networkd does with `SendDecline=yes`. A lease is applied as networkd's
//! `networkd-dhcp4.c` applies it: address and prefix, the routes of
//! `Lease::routes`, the DNS servers, the MTU; all of it goes when the
//! lease does.
//!
//! Everything runs on Core 0 (`tick`). Datagrams to port 68 wait in an
//! inbox until then, and wake the shell loop.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};
use net_core::{acd, dhcp};
use net_core::dhcp::{Client, Dest, Event, Lease, Route, Rx, State};
use spin::Mutex;
use crate::kprintln;
use super::{arp, dns, ipv4, parse_ipv4, udp};
use crate::netdev;

const SERVER_PORT: u16 = 67;
const CLIENT_PORT: u16 = 68;
/// Replies held for Core 0; more than one exchange has in flight.
const INBOX_MAX: usize = 16;

/// A static address (`static_ip`, `static_mask`, `static_gw`, `static_dns`).
#[derive(Clone, PartialEq)]
struct Static {
    addr: [u8; 4],
    prefix: u8,
    gw: Option<[u8; 4]>,
    dns: Option<[u8; 4]>,
}

/// What a probe guards, and what is in use once it is bound.
#[derive(Clone)]
enum Config {
    Dhcp(Lease),
    Static(Static),
}

impl Config {
    fn addr(&self) -> [u8; 4] {
        match self {
            Config::Dhcp(l) => l.address,
            Config::Static(s) => s.addr,
        }
    }
}

struct Inner {
    /// One client per interface, so each keeps its last address for
    /// INIT-REBOOT across stop and start.
    clients: Vec<Client>,
    /// The MAC whose client may run.
    active: Option<[u8; 6]>,
    acd: Option<acd::Acd>,
    probing: Option<Config>,
    applied: Option<Config>,
    /// "Still no offer" is said once per cycle.
    told_failure: bool,
}

static INNER: Mutex<Inner> = Mutex::new(Inner {
    clients: Vec::new(),
    active: None,
    acd: None,
    probing: None,
    applied: None,
    told_failure: false,
});

static INBOX: Mutex<VecDeque<(Vec<u8>, bool)>> = Mutex::new(VecDeque::new());

/// The running client's `Rx`, readable from the receive path.
static RX: AtomicU8 = AtomicU8::new(0);

fn rx_code(rx: Rx) -> u8 {
    match rx { Rx::None => 0, Rx::Raw => 1, Rx::Udp => 2 }
}

/// What a step leaves to do once the lock is let go.
enum Act {
    Udp(Dest, Vec<u8>),
    Arp(acd::Send),
    Apply(Config, bool),
    Lost,
    Offer([u8; 4], [u8; 4]),
    NoOffer,
    Conflict([u8; 4]),
}

fn now() -> u64 { crate::interrupts::uptime_us() }

fn rnd() -> impl FnMut() -> u64 { crate::csprng::random_u64 }

fn own_macs() -> Vec<[u8; 6]> {
    netdev::list().iter().map(|i| i.mac).collect()
}

impl Inner {
    fn client(&mut self, mac: [u8; 6]) -> &mut Client {
        if let Some(i) = self.clients.iter().position(|c| c.hw() == mac) {
            return &mut self.clients[i];
        }
        self.clients.push(Client::new(mac, ipv4::DEVICE_MTU));
        self.clients.last_mut().unwrap()
    }

    fn running(&mut self) -> Option<&mut Client> {
        let mac = self.active?;
        self.clients.iter_mut().find(|c| c.hw() == mac && c.is_running())
    }

    fn publish_rx(&mut self) {
        let rx = self.running().map_or(Rx::None, |c| c.rx());
        RX.store(rx_code(rx), Ordering::Relaxed);
    }

    /// Probe `cfg`'s address on the active interface.
    fn probe(&mut self, cfg: Config, now: u64, aout: &mut acd::Out) {
        let Some(mac) = self.active else { return };
        let mut a = acd::Acd::new(mac, 0);
        a.start(cfg.addr(), true, now, &mut rnd(), aout);
        self.acd = Some(a);
        self.probing = Some(cfg);
    }

    /// Drop what is applied and what is probed.
    fn lose(&mut self, acts: &mut Vec<Act>) {
        if let Some(a) = self.acd.as_mut() {
            a.stop();
        }
        self.acd = None;
        self.probing = None;
        if self.applied.take().is_some() {
            acts.push(Act::Lost);
        }
    }

    /// Turn what the client and the probe said into actions, until neither
    /// has anything left; a conflict feeds the client, a lease the probe.
    fn drain(&mut self, now: u64, mut cout: dhcp::Out, mut aout: acd::Out, acts: &mut Vec<Act>) {
        loop {
            if cout.send.is_empty() && cout.events.is_empty()
                && aout.send.is_empty() && aout.events.is_empty()
            {
                break;
            }
            for (dest, b) in cout.send.drain(..) {
                acts.push(Act::Udp(dest, b));
            }
            for ev in core::mem::take(&mut cout.events) {
                let lease = self.running().and_then(|c| c.lease().cloned());
                match ev {
                    Event::Selecting => {
                        if let Some(l) = lease {
                            acts.push(Act::Offer(l.address, l.server_address));
                        }
                    }
                    Event::IpAcquire | Event::IpChange => {
                        self.told_failure = false;
                        if ev == Event::IpChange {
                            self.lose(acts);
                        }
                        if let Some(l) = lease {
                            self.probe(Config::Dhcp(l), now, &mut aout);
                        }
                    }
                    Event::Renew => {
                        if let Some(l) = lease {
                            if matches!(self.applied, Some(Config::Dhcp(_))) {
                                self.applied = Some(Config::Dhcp(l.clone()));
                                acts.push(Act::Apply(Config::Dhcp(l), true));
                            }
                        }
                    }
                    Event::Expired | Event::Stop => self.lose(acts),
                    Event::TransientFailure => {
                        if !self.told_failure {
                            self.told_failure = true;
                            acts.push(Act::NoOffer);
                        }
                    }
                }
            }
            for s in aout.send.drain(..) {
                acts.push(Act::Arp(s));
            }
            for ev in core::mem::take(&mut aout.events) {
                match ev {
                    acd::Event::Bind => {
                        if let Some(cfg) = self.probing.take() {
                            self.applied = Some(cfg.clone());
                            acts.push(Act::Apply(cfg, false));
                        }
                    }
                    acd::Event::Conflict => {
                        let addr = self.acd.as_ref().map_or([0; 4], |a| a.address());
                        acts.push(Act::Conflict(addr));
                        let dynamic = matches!(self.probing.as_ref().or(self.applied.as_ref()),
                            Some(Config::Dhcp(_)));
                        self.lose(acts);
                        // networkd `dhcp4_address_on_conflict`.
                        if dynamic {
                            if let Some(c) = self.running() {
                                c.send_decline(now, &mut rnd(), &mut cout);
                            }
                        }
                    }
                }
            }
        }
        self.publish_rx();
    }

    fn stop_client(&mut self, now: u64, acts: &mut Vec<Act>) {
        let mut cout = dhcp::Out::default();
        if let Some(c) = self.running() {
            c.stop(&mut cout);
        }
        self.drain(now, cout, acd::Out::default(), acts);
        self.lose(acts);
    }
}

fn run(acts: Vec<Act>) {
    for a in acts {
        match a {
            // A failed send is left to the retransmission, as in systemd.
            Act::Udp(Dest::Broadcast, b) => {
                let _ = udp::send_from([0; 4], [255; 4], CLIENT_PORT, SERVER_PORT, &b);
            }
            Act::Udp(Dest::Server(s), b) => {
                let _ = udp::send(s, CLIENT_PORT, SERVER_PORT, &b);
            }
            Act::Arp(acd::Send::Probe(a)) => arp::send_acd([0; 4], a),
            Act::Arp(acd::Send::Announcement(a)) => arp::send_acd(a, a),
            Act::Apply(cfg, quiet) => apply(&cfg, quiet),
            Act::Lost => lost(),
            Act::Offer(a, s) => kprintln!("[npk] DHCP: offer {}.{}.{}.{} from {}.{}.{}.{}",
                a[0], a[1], a[2], a[3], s[0], s[1], s[2], s[3]),
            Act::NoOffer => kprintln!("[npk] DHCP: no offer yet - still asking"),
            Act::Conflict(a) => kprintln!("[npk] net: {}.{}.{}.{} is in use by another host - not using it",
                a[0], a[1], a[2], a[3]),
        }
    }
}

/// Configure the address, routes, DNS and MTU of `cfg`. `quiet`: a renewal
/// of what is in use.
fn apply(cfg: &Config, quiet: bool) {
    let (addr, prefix, routes, servers, mtu) = match cfg {
        Config::Dhcp(l) => (l.address, l.prefixlen(), l.routes(), l.dns.clone(),
            l.mtu.map_or(ipv4::DEVICE_MTU, u32::from)),
        Config::Static(s) => {
            let mut r = alloc::vec![Route { dst: dhcp::lease::mask(s.addr, s.prefix), len: s.prefix, gw: [0; 4] }];
            if let Some(gw) = s.gw {
                r.push(Route { dst: [0; 4], len: 0, gw });
            }
            (s.addr, s.prefix, r, s.dns.into_iter().collect(), ipv4::DEVICE_MTU)
        }
    };
    ipv4::set_subnet(dhcp::lease::prefixlen_to_netmask(prefix));
    arp::set_ip(addr);
    ipv4::set_routes(&routes);
    dns::set_servers(&servers);
    ipv4::set_mtu(mtu);
    if quiet {
        return;
    }
    let how = if matches!(cfg, Config::Static(_)) { "static IP" } else { "DHCP: configured" };
    kprintln!("[npk] {} {}.{}.{}.{}/{}", how, addr[0], addr[1], addr[2], addr[3], prefix);
    if let Some(gw) = ipv4::default_gateway() {
        // Warm the gateway now rather than on the first lookup.
        arp::request(gw);
    }
    crate::time::request_sync();
}

/// networkd `dhcp4_lease_lost`: nothing of the old configuration stays.
fn lost() {
    arp::set_ip([0; 4]);
    ipv4::clear_routes();
    ipv4::set_subnet([0; 4]);
    dns::set_servers(&[]);
    ipv4::set_mtu(ipv4::DEVICE_MTU);
    ipv4::flush_pmtu();
    kprintln!("[npk] net: address dropped");
}

/// The static configuration, if set and not empty: `static_ip` as an address or
/// `a.b.c.d/len`, otherwise `static_mask` or the classful length. A mask
/// that is not contiguous is refused, as Linux `inet_set_ifa` does.
fn static_config() -> Option<Result<Static, &'static str>> {
    let ip = crate::config::get("static_ip")?;
    let ip = ip.trim();
    if ip.is_empty() {
        return None;
    }
    let get = |k| crate::config::get(k).and_then(|s| parse_ipv4(s.trim()));
    let (addr, prefix) = match ip.split_once('/') {
        Some((a, p)) => (parse_ipv4(a), p.parse::<u8>().ok().filter(|&p| p <= 32)),
        None => {
            let a = parse_ipv4(ip);
            let p = match crate::config::get("static_mask") {
                Some(m) => parse_ipv4(m.trim()).and_then(|m| {
                    let len = dhcp::lease::netmask_to_prefixlen(m);
                    (dhcp::lease::prefixlen_to_netmask(len) == m).then_some(len)
                }),
                None => a.and_then(dhcp::lease::default_prefixlen),
            };
            (a, p)
        }
    };
    let Some(addr) = addr.filter(|&a| dhcp::lease::is_unicast(a)) else { return Some(Err("static_ip")) };
    let Some(prefix) = prefix else { return Some(Err("static_mask")) };
    Some(Ok(Static { addr, prefix, gw: get("static_gw"), dns: get("static_dns") }))
}

/// Configure the active interface: the static address if set, else DHCP.
/// Returns at once; `tick` does the rest.
pub fn configure() {
    let Some(mac) = netdev::mac() else { return };
    let now = now();
    let mut acts = Vec::new();
    {
        let mut g = INNER.lock();
        if g.active != Some(mac) {
            g.stop_client(now, &mut acts);
            g.active = Some(mac);
        }
        match static_config() {
            Some(Ok(s)) => {
                let same = |c: &Option<Config>| matches!(c, Some(Config::Static(x)) if *x == s);
                if same(&g.applied) || same(&g.probing) {
                    return;
                }
                g.stop_client(now, &mut acts);
                let mut aout = acd::Out::default();
                g.probe(Config::Static(s), now, &mut aout);
                g.drain(now, dhcp::Out::default(), aout, &mut acts);
            }
            Some(Err(key)) => {
                drop(g);
                run(acts);
                kprintln!("[npk] net: {} is not valid - no address configured", key);
                return;
            }
            None => {
                let is_static = |c: &Option<Config>| matches!(c, Some(Config::Static(_)));
                if is_static(&g.applied) || is_static(&g.probing) {
                    g.lose(&mut acts);
                }
                if g.running().is_none() {
                    kprintln!("[npk] net: requesting a DHCP lease...");
                    let mut cout = dhcp::Out::default();
                    g.client(mac).start(now, &mut rnd(), &mut cout);
                    g.drain(now, cout, acd::Out::default(), &mut acts);
                }
            }
        }
    }
    run(acts);
}

/// The carrier is gone for good (networkd `link_stop_engines`): stop, drop
/// the configuration. The client keeps its last address for the return.
pub fn link_lost() {
    let mut acts = Vec::new();
    INNER.lock().stop_client(now(), &mut acts);
    run(acts);
}

/// The link joined another network: as `link_lost`, and the last address
/// belongs to the old network too.
pub fn forget() {
    let mut acts = Vec::new();
    {
        let mut g = INNER.lock();
        g.stop_client(now(), &mut acts);
        if let Some(mac) = g.active {
            g.client(mac).set_request_address(None);
        }
    }
    run(acts);
    dns::flush();
}

/// Renew a bound lease now (`networkctl renew`), or configure.
pub fn renew() {
    let mut acts = Vec::new();
    let bound = {
        let mut g = INNER.lock();
        let now = now();
        let mut cout = dhcp::Out::default();
        let bound = match g.running() {
            Some(c) if c.state() == State::Bound => {
                c.send_renew(now, &mut rnd(), &mut cout);
                true
            }
            _ => false,
        };
        g.drain(now, cout, acd::Out::default(), &mut acts);
        bound
    };
    run(acts);
    if !bound {
        configure();
    }
}

/// Take a datagram to port 68 for the client, if it listens. `to_us`: it
/// was addressed to our address.
pub fn deliver(payload: &[u8], to_us: bool) -> bool {
    let rx = RX.load(Ordering::Relaxed);
    if rx == rx_code(Rx::None) || (rx == rx_code(Rx::Udp) && !to_us) {
        return false;
    }
    let mut q = INBOX.lock();
    if q.len() < INBOX_MAX {
        q.push_back((payload.to_vec(), to_us));
    }
    drop(q);
    crate::intent::wake_shell();
    true
}

/// Datagrams to port 68 for any address are ours while the client listens
/// on its packet socket.
pub fn accepts_any_dst() -> bool {
    RX.load(Ordering::Relaxed) == rx_code(Rx::Raw)
}

/// An ARP packet (no Ethernet header), for the probe or the defence. The
/// receive path never waits for Core 0's step; a packet that meets it is
/// not looked at.
pub fn arp_input(arp: &[u8]) {
    let now = now();
    let mut acts = Vec::new();
    {
        let Some(mut g) = INNER.try_lock() else { return };
        let Some(a) = g.acd.as_mut() else { return };
        if !a.is_running() {
            return;
        }
        let mut aout = acd::Out::default();
        a.packet(arp, now, &own_macs(), &mut aout);
        g.drain(now, dhcp::Out::default(), aout, &mut acts);
    }
    run(acts);
}

/// Step the client and the probe. Cheap when nothing is due.
pub fn tick() {
    let now = now();
    let mut msgs: Vec<(Vec<u8>, bool)> = INBOX.lock().drain(..).collect();
    let mut acts = Vec::new();
    {
        let Some(mut g) = INNER.try_lock() else { return };
        let due = |t: Option<u64>| t.is_some_and(|t| t <= now);
        let client_due = g.running().is_some_and(|c| due(c.next_deadline()));
        let acd_due = g.acd.as_ref().is_some_and(|a| due(a.next_deadline()));
        if msgs.is_empty() && !client_due && !acd_due {
            return;
        }
        let mut cout = dhcp::Out::default();
        if let Some(c) = g.running() {
            for (b, to_us) in msgs.drain(..) {
                c.receive(&b, to_us, now, &mut rnd(), &mut cout);
            }
            c.tick(now, &mut rnd(), &mut cout);
        }
        let mut aout = acd::Out::default();
        if let Some(a) = g.acd.as_mut() {
            a.tick(now, &mut rnd(), &mut aout);
        }
        g.drain(now, cout, aout, &mut acts);
    }
    run(acts);
}

/// Does a reply wait, or a timer fall due within a second? The shell loop
/// wakes at least once a second anyway.
pub fn needs_tick() -> bool {
    if !INBOX.lock().is_empty() {
        return true;
    }
    let Some(mut g) = INNER.try_lock() else { return true };
    let soon = now() + 1_000_000;
    let near = |t: Option<u64>| t.is_some_and(|t| t <= soon);
    let acd_near = g.acd.as_ref().is_some_and(|a| near(a.next_deadline()));
    acd_near || g.running().is_some_and(|c| near(c.next_deadline()))
}

/// An address is configured.
pub fn configured() -> bool {
    INNER.lock().applied.is_some()
}

/// Whether the client of the active interface runs.
pub fn is_running() -> bool {
    INNER.lock().running().is_some()
}

/// For `net`: the client's state, the lease's server, and the seconds to
/// T1, T2 and expiry.
pub fn status() -> Option<(&'static str, [u8; 4], Option<(u64, u64, u64)>)> {
    let mut g = INNER.lock();
    if matches!(g.applied, Some(Config::Static(_))) {
        return Some(("static", [0; 4], None));
    }
    let probing = g.probing.is_some();
    let c = g.running()?;
    let now = now();
    let left = c.lease_times().map(|(t1, t2, e)| {
        let s = |t: u64| t.saturating_sub(now) / 1_000_000;
        (s(t1), s(t2), s(e))
    });
    let state = if probing { "probing" } else { c.state().name() };
    Some((state, c.lease().map_or([0; 4], |l| l.server_address), left))
}

/// Boot: configure, and wait up to `max_ms` for an address. Waiting is
/// right there; the link tick never waits.
pub fn run_blocking(max_ms: u64) -> bool {
    configure();
    wait(max_ms, || configured())
}

/// The `dhcp` command: renew or configure, and wait up to `max_ms` for a
/// bound lease.
pub fn renew_blocking(max_ms: u64) -> bool {
    renew();
    wait(max_ms, || configured()
        && INNER.lock().running().is_some_and(|c| c.state() == State::Bound))
}

fn wait(max_ms: u64, done: impl Fn() -> bool) -> bool {
    let end = now() + max_ms * 1000;
    while !done() && now() < end {
        super::poll();
        tick();
        core::hint::spin_loop();
    }
    done()
}
