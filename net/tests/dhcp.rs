//! The DHCP client against systemd's `sd-dhcp-client` behaviour.

use net_core::dhcp::message::{Message, BOOTREPLY};
use net_core::dhcp::*;

const HW: [u8; 6] = [2, 0, 0, 0, 0, 1];
const SERVER: [u8; 4] = [192, 168, 1, 1];
const OTHER_SERVER: [u8; 4] = [192, 168, 1, 2];
const ADDR: [u8; 4] = [192, 168, 1, 50];
const S: u64 = USEC_PER_SEC;

/// A random source whose values all fall in the middle of the fuzz range
/// (no fuzz), each a different xid.
fn rnd() -> impl FnMut() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    || S + 2 * S * N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn parse(b: &[u8]) -> Message {
    Message::parse(b, 0, None, None).unwrap()
}

fn kind(b: &[u8]) -> u8 {
    parse(b).option_u8(OPT_MESSAGE_TYPE).unwrap()
}

struct Reply {
    kind: u8,
    xid: u32,
    yiaddr: [u8; 4],
    server: Option<[u8; 4]>,
    opts: Vec<(u8, Vec<u8>)>,
}

impl Reply {
    fn new(kind: u8, xid: u32) -> Self {
        let mut opts = vec![
            (OPT_SUBNET_MASK, vec![255, 255, 255, 0]),
            (OPT_ROUTER, SERVER.to_vec()),
            (OPT_DOMAIN_NAME_SERVER, SERVER.to_vec()),
            (OPT_LEASE_TIME, 3600u32.to_be_bytes().to_vec()),
        ];
        if kind == DHCP_NAK {
            opts.clear();
        }
        Reply { kind, xid, yiaddr: if kind == DHCP_NAK { [0; 4] } else { ADDR }, server: Some(SERVER), opts }
    }
    fn opt(mut self, code: u8, v: &[u8]) -> Self {
        self.opts.retain(|(c, _)| *c != code);
        self.opts.push((code, v.to_vec()));
        self
    }
    fn without(mut self, code: u8) -> Self {
        self.opts.retain(|(c, _)| *c != code);
        self
    }
    fn bytes(&self) -> Vec<u8> {
        let mut m = Message::new(BOOTREPLY, self.xid, &HW);
        m.yiaddr = self.yiaddr;
        m.append_option(OPT_MESSAGE_TYPE, &[self.kind]).unwrap();
        if let Some(s) = self.server {
            m.append_option(OPT_SERVER_IDENTIFIER, &s).unwrap();
        }
        for (c, v) in &self.opts {
            m.append_option(*c, v).unwrap();
        }
        m.build().unwrap()
    }
}

/// Start a client and take its first DISCOVER.
fn selecting() -> (Client, Vec<u8>) {
    let mut c = Client::new(HW, 1500);
    let mut out = Out::default();
    c.start(0, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Selecting);
    assert_eq!(out.send.len(), 1);
    let (dest, b) = out.send.pop().unwrap();
    assert_eq!(dest, Dest::Broadcast);
    (c, b)
}

/// A client bound to ADDR at time 0.
fn bound() -> Client {
    let (mut c, _) = selecting();
    let mut out = Out::default();
    let xid = c.xid();
    c.receive(&Reply::new(DHCP_OFFER, xid).bytes(), false, 0, &mut rnd(), &mut out);
    c.receive(&Reply::new(DHCP_ACK, xid).bytes(), false, 0, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Bound);
    c
}

#[test]
fn discover_carries_what_systemd_sends() {
    let (_, b) = selecting();
    let m = parse(&b);
    assert!(b.len() >= 300, "padded to a BOOTP message");
    assert_eq!(m.option_u8(OPT_MESSAGE_TYPE), Some(DHCP_DISCOVER));
    assert_eq!(m.option(OPT_CLIENT_IDENTIFIER).unwrap(), [1, 2, 0, 0, 0, 0, 1]);
    assert_eq!(m.option(OPT_PARAMETER_REQUEST_LIST).unwrap(), [1, 3, 6, 12, 15, 26, 33, 121]);
    assert_eq!(m.option_u16(OPT_MAXIMUM_MESSAGE_SIZE), Some(1500));
    assert!(m.option_exact(OPT_RAPID_COMMIT, 0).is_some());
    assert!(!m.has_option(OPT_REQUESTED_IP_ADDRESS));
    assert!(!m.has_option(OPT_SERVER_IDENTIFIER));
    assert_eq!(m.secs, 1, "never zero");
    assert_eq!(m.flags, 0);
}

#[test]
fn the_offer_is_only_selected_and_the_ack_is_the_lease() {
    let (mut c, _) = selecting();
    let xid = c.xid();
    let mut out = Out::default();
    let offer = Reply::new(DHCP_OFFER, xid).opt(OPT_DOMAIN_NAME_SERVER, &[9, 9, 9, 9]);
    c.receive(&offer.bytes(), false, 5 * S / 10, &mut rnd(), &mut out);
    assert_eq!(out.events, [Event::Selecting], "nothing to apply yet");
    assert_eq!(c.state(), State::Requesting);
    let (dest, req) = out.send.pop().unwrap();
    assert_eq!(dest, Dest::Broadcast);
    let m = parse(&req);
    assert_eq!(m.option_u8(OPT_MESSAGE_TYPE), Some(DHCP_REQUEST));
    assert_eq!(m.option_addr(OPT_SERVER_IDENTIFIER), Some(SERVER));
    assert_eq!(m.option_addr(OPT_REQUESTED_IP_ADDRESS), Some(ADDR));
    assert_eq!(m.ciaddr, [0; 4]);
    assert_eq!(m.xid, xid);

    let mut out = Out::default();
    c.receive(&Reply::new(DHCP_ACK, xid).bytes(), false, S, &mut rnd(), &mut out);
    assert_eq!(out.events, [Event::IpAcquire]);
    let l = c.lease().unwrap();
    assert_eq!(l.dns, [SERVER], "the ACK's options, not the offer's");
    assert_eq!(l.timestamp, S);
    assert_eq!(c.rx(), Rx::None, "nothing is listened for while bound");
    assert_eq!(c.lease_times(), Some((S + 1800 * S, S + 3150 * S, S + 3600 * S)));
}

#[test]
fn replies_for_another_exchange_or_client_are_ignored() {
    let (mut c, _) = selecting();
    let xid = c.xid();
    let mut out = Out::default();
    c.receive(&Reply::new(DHCP_OFFER, xid ^ 1).bytes(), false, 0, &mut rnd(), &mut out);
    let mut other = Reply::new(DHCP_OFFER, xid).bytes();
    other[28 + 5] ^= 1;
    c.receive(&other, false, 0, &mut rnd(), &mut out);
    c.receive(&Reply::new(DHCP_ACK, xid).bytes(), false, 0, &mut rnd(), &mut out);
    assert!(out.events.is_empty() && out.send.is_empty());
    assert_eq!(c.state(), State::Selecting);
}

#[test]
fn discover_backs_off_exponentially_to_64_s() {
    let (mut c, _) = selecting();
    let mut out = Out::default();
    let mut now = 0;
    for gap in [4, 8, 16, 32, 64, 64, 64] {
        assert_eq!(c.next_deadline(), Some(now + gap * S));
        now += gap * S;
        c.tick(now - 1, &mut rnd(), &mut out);
        assert!(out.send.is_empty(), "not before the timeout");
        c.tick(now, &mut rnd(), &mut out);
        assert_eq!(out.send.len(), 1);
        assert_eq!(kind(&out.send.pop().unwrap().1), DHCP_DISCOVER);
    }
    assert!(out.events.contains(&Event::TransientFailure));
}

#[test]
fn retransmissions_carry_up_to_one_second_of_fuzz() {
    let mut c = Client::new(HW, 1500);
    let mut out = Out::default();
    let mut zero = || 0u64;
    c.start(0, &mut zero, &mut out);
    assert_eq!(c.next_deadline(), Some(5 * S), "-1 s of fuzz subtracted");
    let mut c = Client::new(HW, 1500);
    let mut top = || 2 * S - 1;
    c.start(0, &mut top, &mut out);
    assert_eq!(c.next_deadline(), Some(3 * S + 1));
}

#[test]
fn an_unanswered_request_goes_back_to_discover() {
    let (mut c, _) = selecting();
    let xid = c.xid();
    let mut out = Out::default();
    c.receive(&Reply::new(DHCP_OFFER, xid).bytes(), false, 0, &mut rnd(), &mut out);
    out.send.clear();
    let mut now = 0;
    for gap in [4, 8, 16, 32] {
        now += gap * S;
        c.tick(now, &mut rnd(), &mut out);
        assert_eq!(kind(&out.send.pop().unwrap().1), DHCP_REQUEST);
    }
    now += 64 * S;
    c.tick(now, &mut rnd(), &mut out);
    assert!(out.send.is_empty(), "five REQUESTs, then a restart after 1 s");
    assert_eq!(c.state(), State::Init);
    c.tick(now + S, &mut rnd(), &mut out);
    assert_eq!(kind(&out.send.pop().unwrap().1), DHCP_DISCOVER);
    assert_ne!(c.xid(), xid, "a new exchange");
}

#[test]
fn a_nak_restarts_with_growing_delay_and_must_name_the_server() {
    let (mut c, _) = selecting();
    let xid = c.xid();
    let mut out = Out::default();
    c.receive(&Reply::new(DHCP_OFFER, xid).bytes(), false, 0, &mut rnd(), &mut out);
    out.send.clear();

    let mut nak = Reply::new(DHCP_NAK, xid);
    nak.server = None;
    c.receive(&nak.bytes(), false, 0, &mut rnd(), &mut out);
    nak.server = Some(OTHER_SERVER);
    c.receive(&nak.bytes(), false, 0, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Requesting, "a NAK without or from another server is ignored");

    c.receive(&Reply::new(DHCP_NAK, xid).bytes(), false, 10, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Init);
    assert!(out.send.is_empty());
    assert_eq!(c.next_deadline(), Some(10 + S));
    c.tick(10 + S, &mut rnd(), &mut out);
    assert_eq!(kind(&out.send.pop().unwrap().1), DHCP_DISCOVER);

    let xid = c.xid();
    c.receive(&Reply::new(DHCP_OFFER, xid).bytes(), false, 20 * S, &mut rnd(), &mut out);
    c.receive(&Reply::new(DHCP_NAK, xid).bytes(), false, 20 * S, &mut rnd(), &mut out);
    assert_eq!(c.next_deadline(), Some(22 * S), "the delay doubles");
}

#[test]
fn a_nak_while_selecting_is_ignored() {
    let (mut c, _) = selecting();
    let xid = c.xid();
    let mut out = Out::default();
    c.receive(&Reply::new(DHCP_NAK, xid).bytes(), false, 0, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Selecting);
}

#[test]
fn init_reboot_asks_for_the_old_address_then_discovers() {
    let mut c = Client::new(HW, 1500);
    c.set_request_address(Some(ADDR));
    let mut out = Out::default();
    c.start(0, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Rebooting);
    let m = parse(&out.send.pop().unwrap().1);
    assert_eq!(m.option_u8(OPT_MESSAGE_TYPE), Some(DHCP_REQUEST));
    assert_eq!(m.option_addr(OPT_REQUESTED_IP_ADDRESS), Some(ADDR));
    assert!(!m.has_option(OPT_SERVER_IDENTIFIER));
    assert_eq!(m.ciaddr, [0; 4]);
    assert_eq!(c.next_deadline(), Some(S), "a quarter of 4 s");
    c.tick(S, &mut rnd(), &mut out);
    assert_eq!(kind(&out.send.pop().unwrap().1), DHCP_REQUEST);
    assert_eq!(c.next_deadline(), Some(3 * S));
    c.tick(3 * S, &mut rnd(), &mut out);
    let m = parse(&out.send.pop().unwrap().1);
    assert_eq!(m.option_u8(OPT_MESSAGE_TYPE), Some(DHCP_DISCOVER), "no delay after reboot");
    assert_eq!(m.option_addr(OPT_REQUESTED_IP_ADDRESS), Some(ADDR), "still the hint");
}

#[test]
fn a_nak_on_reboot_discovers_at_once() {
    let mut c = Client::new(HW, 1500);
    c.set_request_address(Some(ADDR));
    let mut out = Out::default();
    c.start(0, &mut rnd(), &mut out);
    out.send.clear();
    let mut nak = Reply::new(DHCP_NAK, c.xid());
    nak.server = Some(OTHER_SERVER);
    c.receive(&nak.bytes(), false, 5, &mut rnd(), &mut out);
    assert_eq!(kind(&out.send.pop().unwrap().1), DHCP_DISCOVER);
    assert!(out.events.is_empty(), "nothing was bound");
}

#[test]
fn rapid_commit_binds_on_an_ack_with_option_80_only() {
    let (mut c, _) = selecting();
    let xid = c.xid();
    let mut out = Out::default();
    c.receive(&Reply::new(DHCP_ACK, xid).bytes(), false, 0, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Selecting);
    c.receive(&Reply::new(DHCP_ACK, xid).opt(OPT_RAPID_COMMIT, &[]).bytes(), false, 0, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Bound);
    assert_eq!(out.events, [Event::IpAcquire]);
}

#[test]
fn renewing_is_unicast_and_takes_only_replies_to_our_address() {
    let mut c = bound();
    let mut out = Out::default();
    let t1 = 1800 * S;
    c.tick(t1, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Renewing);
    let (dest, b) = out.send.pop().unwrap();
    assert_eq!(dest, Dest::Server(SERVER));
    let m = parse(&b);
    assert_eq!(m.ciaddr, ADDR);
    assert!(!m.has_option(OPT_REQUESTED_IP_ADDRESS));
    assert!(!m.has_option(OPT_SERVER_IDENTIFIER));
    assert!(!m.has_option(OPT_RAPID_COMMIT));
    assert_eq!(c.rx(), Rx::Udp);
    // Half the time to T2 (3150 s), at least 60 s.
    assert_eq!(c.next_deadline(), Some(t1 + 675 * S));

    let xid = c.xid();
    c.receive(&Reply::new(DHCP_ACK, xid).bytes(), false, t1 + S, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Renewing, "a broadcast does not reach the UDP socket");
    c.receive(&Reply::new(DHCP_ACK, xid).bytes(), true, t1 + S, &mut rnd(), &mut out);
    assert_eq!(out.events, [Event::Renew]);
    assert_eq!(c.state(), State::Bound);
    assert_eq!(c.lease_times().unwrap().2, t1 + S + 3600 * S, "counted from the new ACK");
}

#[test]
fn rebinding_broadcasts_and_a_new_address_is_a_change() {
    let mut c = bound();
    let mut out = Out::default();
    c.tick(3150 * S, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Rebinding);
    let (dest, b) = out.send.pop().unwrap();
    assert_eq!(dest, Dest::Broadcast, "only the last of T1's REQUESTs and T2's");
    assert_eq!(parse(&b).ciaddr, ADDR);
    assert_eq!(c.rx(), Rx::Raw);
    assert_eq!(c.next_deadline(), Some(3150 * S + 225 * S));
    let mut ack = Reply::new(DHCP_ACK, c.xid());
    ack.yiaddr = [192, 168, 1, 77];
    c.receive(&ack.bytes(), false, 3200 * S, &mut rnd(), &mut out);
    assert_eq!(out.events, [Event::IpChange]);
    assert_eq!(c.lease().unwrap().address, [192, 168, 1, 77]);
}

#[test]
fn renewal_retransmits_no_more_often_than_every_60_s() {
    let mut c = bound();
    let mut out = Out::default();
    c.tick(3100 * S, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Renewing);
    assert_eq!(c.next_deadline(), Some(3150 * S), "T2 comes first");
    c.tick(3150 * S, &mut rnd(), &mut out);
    assert_eq!(c.next_deadline(), Some(3150 * S + 225 * S));
    c.tick(3570 * S, &mut rnd(), &mut out);
    assert_eq!(c.next_deadline(), Some(3600 * S), "the expiry");
}

#[test]
fn an_expired_lease_is_dropped_and_discovery_starts_over() {
    let mut c = bound();
    let mut out = Out::default();
    let mut now = 0;
    while now < 3600 * S {
        now = c.next_deadline().unwrap();
        c.tick(now, &mut rnd(), &mut out);
    }
    assert!(out.events.contains(&Event::Expired));
    assert!(c.lease().is_none());
    assert_eq!(c.state(), State::Selecting);
    let (dest, b) = out.send.pop().unwrap();
    assert_eq!(dest, Dest::Broadcast);
    let m = parse(&b);
    assert_eq!(m.option_u8(OPT_MESSAGE_TYPE), Some(DHCP_DISCOVER));
    assert_eq!(m.option_addr(OPT_REQUESTED_IP_ADDRESS), Some(ADDR), "the last address as a hint");
}

#[test]
fn a_nak_while_renewing_expires_the_lease() {
    let mut c = bound();
    let mut out = Out::default();
    c.tick(1800 * S, &mut rnd(), &mut out);
    c.receive(&Reply::new(DHCP_NAK, c.xid()).bytes(), true, 1801 * S, &mut rnd(), &mut out);
    assert_eq!(out.events, [Event::Expired]);
    assert_eq!(c.state(), State::Init);
}

#[test]
fn renew_on_request_and_decline_on_conflict() {
    let mut c = bound();
    let mut out = Out::default();
    c.send_renew(10 * S, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Renewing);
    assert_eq!(out.send.pop().unwrap().0, Dest::Server(SERVER));

    let mut c = bound();
    let mut out = Out::default();
    c.send_decline(10 * S, &mut rnd(), &mut out);
    let (dest, b) = out.send.pop().unwrap();
    assert_eq!(dest, Dest::Broadcast);
    let m = parse(&b);
    assert_eq!(m.option_u8(OPT_MESSAGE_TYPE), Some(DHCP_DECLINE));
    assert_eq!(m.option_addr(OPT_REQUESTED_IP_ADDRESS), Some(ADDR));
    assert_eq!(m.option_addr(OPT_SERVER_IDENTIFIER), Some(SERVER));
    assert!(!m.has_option(OPT_PARAMETER_REQUEST_LIST), "nothing else");
    assert_eq!(out.events, [Event::Expired]);
    assert_eq!(c.next_deadline(), Some(11 * S));
}

#[test]
fn stop_drops_everything() {
    let mut c = bound();
    let mut out = Out::default();
    c.stop(&mut out);
    assert_eq!(out.events, [Event::Stop]);
    assert!(c.lease().is_none() && c.next_deadline().is_none());
    assert!(!c.is_running());
}

fn lease_of(r: Reply) -> Option<Lease> {
    let m = Message::parse(&r.bytes(), BOOTREPLY, None, None).unwrap();
    Lease::from_message(&m, 0)
}

#[test]
fn lease_times_follow_rfc_2131_defaults() {
    let l = lease_of(Reply::new(DHCP_ACK, 0)).unwrap();
    assert_eq!((l.t1, l.t2, l.lifetime), (1800 * S, 3150 * S, 3600 * S));
    let l = lease_of(Reply::new(DHCP_ACK, 0)
        .opt(OPT_RENEWAL_TIME, &1000u32.to_be_bytes())
        .opt(OPT_REBINDING_TIME, &2000u32.to_be_bytes())).unwrap();
    assert_eq!((l.t1, l.t2), (1000 * S, 2000 * S));
    let l = lease_of(Reply::new(DHCP_ACK, 0)
        .opt(OPT_RENEWAL_TIME, &3000u32.to_be_bytes())
        .opt(OPT_REBINDING_TIME, &2000u32.to_be_bytes())).unwrap();
    assert_eq!((l.t1, l.t2), (1800 * S, 2000 * S), "T1 not before T2");
    let l = lease_of(Reply::new(DHCP_ACK, 0).opt(OPT_LEASE_TIME, &10u32.to_be_bytes())).unwrap();
    assert_eq!(l.lifetime, 30 * S, "at least 30 s");
    let l = lease_of(Reply::new(DHCP_ACK, 0).opt(OPT_LEASE_TIME, &[255; 4])).unwrap();
    assert_eq!((l.lifetime, l.t1, l.t2), (INFINITY, 0, 0));
    assert!(lease_of(Reply::new(DHCP_ACK, 0).without(OPT_LEASE_TIME)).is_none());
    assert!(lease_of(Reply::new(DHCP_ACK, 0).opt(OPT_LEASE_TIME, &[0; 4])).is_none());
}

#[test]
fn an_infinite_lease_sets_no_timers() {
    let (mut c, _) = selecting();
    let xid = c.xid();
    let mut out = Out::default();
    c.receive(&Reply::new(DHCP_OFFER, xid).bytes(), false, 0, &mut rnd(), &mut out);
    c.receive(&Reply::new(DHCP_ACK, xid).opt(OPT_LEASE_TIME, &[255; 4]).bytes(), false, 0, &mut rnd(), &mut out);
    assert_eq!(c.state(), State::Bound);
    assert_eq!(c.next_deadline(), None);
}

#[test]
fn a_lease_needs_a_usable_address_and_a_server() {
    let mut r = Reply::new(DHCP_ACK, 0);
    r.server = None;
    assert!(lease_of(r).is_none());
    for a in [[0, 0, 0, 0], [0, 1, 2, 3], [127, 0, 0, 1], [224, 0, 0, 1], [240, 0, 0, 1], [255, 255, 255, 255]] {
        let mut r = Reply::new(DHCP_ACK, 0);
        r.yiaddr = a;
        assert!(lease_of(r).is_none(), "{a:?}");
    }
}

#[test]
fn a_missing_mask_is_the_classful_one_and_a_ragged_one_is_covered() {
    let mut r = Reply::new(DHCP_ACK, 0).without(OPT_SUBNET_MASK);
    r.yiaddr = [172, 16, 5, 5];
    assert_eq!(lease_of(r).unwrap().prefixlen(), 16);
    let l = lease_of(Reply::new(DHCP_ACK, 0).opt(OPT_SUBNET_MASK, &[255, 0, 255, 0])).unwrap();
    assert_eq!(l.prefixlen(), 24);
}

#[test]
fn split_options_are_joined_and_overload_is_read() {
    let mut m = Message::new(BOOTREPLY, 0, &HW);
    m.yiaddr = ADDR;
    let dns: Vec<u8> = (0..70u8).flat_map(|i| [10, 0, 0, i]).collect();
    m.append_option(OPT_MESSAGE_TYPE, &[DHCP_ACK]).unwrap();
    m.append_option(OPT_LEASE_TIME, &3600u32.to_be_bytes()).unwrap();
    m.append_option(OPT_DOMAIN_NAME_SERVER, &dns).unwrap();
    m.append_option(52, &[1]).unwrap();
    // The server identifier lives in 'file'.
    m.file[..6].copy_from_slice(&[OPT_SERVER_IDENTIFIER, 4, 192, 168, 1, 1]);
    m.file[6] = 255;
    let b = m.build().unwrap();
    let p = Message::parse(&b, BOOTREPLY, None, None).unwrap();
    let l = Lease::from_message(&p, 0).unwrap();
    assert_eq!(l.dns.len(), 70, "280 bytes over two instances");
    assert_eq!(l.dns[69], [10, 0, 0, 69]);
    assert_eq!(l.server_address, SERVER);
}

#[test]
fn small_mtu_is_ignored() {
    let l = lease_of(Reply::new(DHCP_ACK, 0).opt(OPT_MTU_INTERFACE, &500u16.to_be_bytes())).unwrap();
    assert_eq!(l.mtu, None);
    let l = lease_of(Reply::new(DHCP_ACK, 0).opt(OPT_MTU_INTERFACE, &1400u16.to_be_bytes())).unwrap();
    assert_eq!(l.mtu, Some(1400));
}

#[test]
fn classless_routes_replace_the_router_and_static_routes() {
    let gw2 = [192, 168, 1, 254];
    // 10.0.0.0/8 via gw2, and the default via gw2.
    let cl = [8, 10, 192, 168, 1, 254, 0, 192, 168, 1, 254];
    let l = lease_of(Reply::new(DHCP_ACK, 0)
        .opt(OPT_CLASSLESS_STATIC_ROUTE, &cl)
        .opt(OPT_STATIC_ROUTE, &[172, 16, 0, 0, 192, 168, 1, 1])).unwrap();
    let r = l.routes();
    assert_eq!(r, [
        Route { dst: [192, 168, 1, 0], len: 24, gw: [0; 4] },
        Route { dst: gw2, len: 32, gw: [0; 4] },
        Route { dst: [10, 0, 0, 0], len: 8, gw: gw2 },
        Route { dst: [0; 4], len: 0, gw: gw2 },
    ]);

    let l = lease_of(Reply::new(DHCP_ACK, 0)
        .opt(OPT_STATIC_ROUTE, &[172, 16, 9, 9, 192, 168, 1, 7, 0, 0, 0, 0, 1, 2, 3, 4])).unwrap();
    assert_eq!(l.routes(), [
        Route { dst: [192, 168, 1, 0], len: 24, gw: [0; 4] },
        Route { dst: [192, 168, 1, 7], len: 32, gw: [0; 4] },
        Route { dst: [172, 16, 0, 0], len: 16, gw: [192, 168, 1, 7] },
        Route { dst: SERVER, len: 32, gw: [0; 4] },
        Route { dst: [0; 4], len: 0, gw: SERVER },
    ], "classful static routes, no default among them, and the router");
}

#[test]
fn without_a_router_there_is_no_default_route() {
    let l = lease_of(Reply::new(DHCP_ACK, 0).without(OPT_ROUTER)).unwrap();
    assert_eq!(l.routes(), [Route { dst: [192, 168, 1, 0], len: 24, gw: [0; 4] }]);
}

#[test]
fn the_route_table_takes_the_longest_prefix() {
    let mut t = net_core::ip::Routes::new();
    t.set(&[
        Route { dst: [192, 168, 1, 0], len: 24, gw: [0; 4] },
        Route { dst: [0; 4], len: 0, gw: SERVER },
        Route { dst: [10, 0, 0, 0], len: 8, gw: [192, 168, 1, 254] },
    ]);
    assert_eq!(t.next_hop([192, 168, 1, 9]), Some([192, 168, 1, 9]));
    assert_eq!(t.next_hop([10, 1, 2, 3]), Some([192, 168, 1, 254]));
    assert_eq!(t.next_hop([8, 8, 8, 8]), Some(SERVER));
    assert_eq!(t.default_gateway(), Some(SERVER));
    t.set(&[Route { dst: [192, 168, 1, 0], len: 24, gw: [0; 4] }]);
    assert_eq!(t.next_hop([8, 8, 8, 8]), None);
}
