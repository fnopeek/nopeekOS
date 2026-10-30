//! The stub resolver against systemd-resolved's behaviour.

use net_core::dns::packet::{self, read_name};
use net_core::dns::*;

const S: u64 = USEC_PER_SEC;
const NS1: [u8; 4] = [192, 168, 1, 1];
const NS2: [u8; 4] = [192, 168, 1, 2];

fn rnd() -> impl FnMut() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    || N.fetch_add(7919, std::sync::atomic::Ordering::Relaxed)
}

fn resolver(servers: &[[u8; 4]]) -> Resolver {
    let mut r = Resolver::new();
    r.routable = true;
    r.set_servers(servers);
    r
}

fn name_bytes(n: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for l in n.split('.') {
        v.push(l.len() as u8);
        v.extend_from_slice(l.as_bytes());
    }
    v.push(0);
    v
}

/// A reply to `query`.
struct R {
    rcode: u16,
    tc: bool,
    opt: bool,
    an: Vec<(String, u16, u32, Vec<u8>)>,
    ns: Vec<(String, u16, u32, Vec<u8>)>,
}

impl R {
    fn ok() -> Self { R { rcode: 0, tc: false, opt: true, an: vec![], ns: vec![] } }
    fn rcode(rcode: u16) -> Self { R { rcode, ..R::ok() } }
    fn a(mut self, name: &str, ttl: u32, ip: [u8; 4]) -> Self {
        self.an.push((name.into(), TYPE_A, ttl, ip.to_vec()));
        self
    }
    fn cname(mut self, name: &str, ttl: u32, target: &str) -> Self {
        self.an.push((name.into(), TYPE_CNAME, ttl, name_bytes(target)));
        self
    }
    fn soa(mut self, zone: &str, ttl: u32, minimum: u32) -> Self {
        let mut d = name_bytes(&format!("ns.{zone}"));
        d.extend(name_bytes(&format!("host.{zone}")));
        for v in [1u32, 7200, 3600, 1209600, minimum] {
            d.extend(v.to_be_bytes());
        }
        self.ns.push((zone.into(), TYPE_SOA, ttl, d));
        self
    }
    fn bytes(&self, query: &[u8]) -> Vec<u8> {
        let (_, qend) = read_name(query, 12).unwrap();
        let mut p = query[0..2].to_vec();
        let flags = 0x8180 | self.rcode | if self.tc { 0x0200 } else { 0 };
        p.extend(flags.to_be_bytes());
        p.extend(1u16.to_be_bytes());
        p.extend((self.an.len() as u16).to_be_bytes());
        p.extend((self.ns.len() as u16).to_be_bytes());
        p.extend((self.opt as u16).to_be_bytes());
        p.extend_from_slice(&query[12..qend + 4]);
        for (n, t, ttl, d) in self.an.iter().chain(&self.ns) {
            p.extend(name_bytes(n));
            p.extend(t.to_be_bytes());
            p.extend(CLASS_IN.to_be_bytes());
            p.extend(ttl.to_be_bytes());
            p.extend((d.len() as u16).to_be_bytes());
            p.extend(d);
        }
        if self.opt {
            p.push(0);
            p.extend(TYPE_OPT.to_be_bytes());
            p.extend(1232u16.to_be_bytes());
            p.extend(0u32.to_be_bytes());
            p.extend(0u16.to_be_bytes());
        }
        p
    }
}

fn udp(out: &mut Out) -> (u32, [u8; 4], Vec<u8>) {
    let i = out.send.iter().position(|s| matches!(s, Send::Udp { .. })).expect("a datagram");
    match out.send.remove(i) {
        Send::Udp { tx, server, bytes } => (tx, server, bytes),
        _ => unreachable!(),
    }
}

fn has_opt(query: &[u8]) -> bool {
    u16::from_be_bytes([query[10], query[11]]) == 1
}

fn qname(query: &[u8]) -> String {
    read_name(query, 12).unwrap().0
}

fn sends(out: &Out) -> usize {
    out.send.iter().filter(|s| !matches!(s, Send::Close { .. })).count()
}

#[test]
fn names_are_normalized_and_checked() {
    assert_eq!(normalize("Example.COM."), Some("example.com".into()));
    assert_eq!(normalize("a..b"), None, "empty label");
    assert_eq!(normalize("."), None);
    assert_eq!(normalize(&"a".repeat(64)), None, "label over 63");
    assert_eq!(normalize(&"a".repeat(63)), Some("a".repeat(63)));
    let long = ["a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61)].join(".");
    assert_eq!(long.len(), 253);
    assert!(normalize(&long).is_some());
    assert!(normalize(&format!("{long}x")).is_none(), "over 253");
    let mut r = resolver(&[NS1]);
    assert_eq!(r.query("exa mple.com", 0, &mut rnd(), &mut Out::default()), None);
}

#[test]
fn an_answer_is_cached_case_insensitively() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("example.com.", 0, &mut rnd(), &mut out).unwrap();
    let (tx, server, query) = udp(&mut out);
    assert_eq!(server, NS1);
    assert_eq!(qname(&query), "example.com");
    assert!(has_opt(&query), "EDNS(0) first");
    assert_eq!(u16::from_be_bytes([query[2], query[3]]), 0x0100, "RD");
    r.udp_reply(tx, &R::ok().a("example.com", 300, [1, 2, 3, 4]).bytes(&query), S, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Addrs(vec![[1, 2, 3, 4]], 300)));
    assert!(out.send.iter().any(|s| matches!(s, Send::Close { tx: t } if *t == tx)));

    let mut out = Out::default();
    let q = r.query("EXAMPLE.Com", 100 * S, &mut rnd(), &mut out).unwrap();
    assert_eq!(sends(&out), 0, "from the cache");
    assert_eq!(r.take(q), Some(Outcome::Addrs(vec![[1, 2, 3, 4]], 201)));
    assert_eq!(r.lookup_cached("example.com", 100 * S), Some(Outcome::Addrs(vec![[1, 2, 3, 4]], 201)));
    assert_eq!(r.lookup_cached("example.com", 302 * S), None, "expired");
}

#[test]
fn a_reply_with_another_id_or_question_is_not_ours() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("example.com", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    let mut wrong = R::ok().a("example.com", 300, [6, 6, 6, 6]).bytes(&query);
    wrong[1] ^= 1;
    r.udp_reply(tx, &wrong, 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), None, "ignored, still waiting");
    let other = packet::build_query(u16::from_be_bytes([query[0], query[1]]), "evil.com", TYPE_A, None);
    r.udp_reply(tx, &R::ok().a("evil.com", 300, [6, 6, 6, 6]).bytes(&other), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Failed(Failure::InvalidReply)));
}

#[test]
fn a_cname_chain_in_one_answer() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("www.example.com", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    let reply = R::ok()
        .cname("www.example.com", 600, "cdn.example.net")
        .cname("cdn.example.net", 60, "edge.example.net")
        .a("edge.example.net", 300, [5, 5, 5, 5])
        .a("edge.example.net", 300, [5, 5, 5, 6]);
    r.udp_reply(tx, &reply.bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Addrs(vec![[5, 5, 5, 5], [5, 5, 5, 6]], 60)));
    assert_eq!(r.lookup_cached("www.example.com", S), Some(Outcome::Addrs(vec![[5, 5, 5, 5], [5, 5, 5, 6]], 59)));
}

#[test]
fn a_cname_without_its_target_asks_for_the_target() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("www.example.com", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::ok().cname("www.example.com", 600, "target.example.net").bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), None);
    let (tx, _, query) = udp(&mut out);
    assert_eq!(qname(&query), "target.example.net");
    r.udp_reply(tx, &R::ok().a("target.example.net", 30, [7, 7, 7, 7]).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Addrs(vec![[7, 7, 7, 7]], 30)));
}

#[test]
fn a_cname_loop_ends() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("a.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::ok().cname("a.example", 60, "a.example").bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Failed(Failure::CnameLoop)));
}

#[test]
fn nxdomain_is_cached_for_the_soa_minimum_and_for_every_type() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("nope.example.com", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::rcode(RCODE_NXDOMAIN).soa("example.com", 3600, 20).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::NxDomain));
    assert_eq!(r.lookup_cached("nope.example.com", 19 * S), Some(Outcome::NxDomain));
    assert_eq!(r.lookup_cached("nope.example.com", 21 * S), None, "RFC 2308: min(SOA TTL, MINIMUM)");
}

#[test]
fn a_negative_answer_without_soa_is_not_cached() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("nope.example.com", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::rcode(RCODE_NXDOMAIN).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::NxDomain));
    assert_eq!(r.lookup_cached("nope.example.com", 0), None);
}

#[test]
fn nodata_is_its_own_answer() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("v6only.example.com", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::ok().soa("example.com", 60, 300).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::NoData));
    assert_eq!(r.lookup_cached("v6only.example.com", 59 * S), Some(Outcome::NoData));
}

#[test]
fn servfail_is_retried_lower_then_accepted_and_cached_ten_seconds() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("broken.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    assert!(has_opt(&query));
    r.udp_reply(tx, &R::rcode(RCODE_SERVFAIL).bytes(&query), 0, &mut rnd(), &mut out);
    let (tx, _, query) = udp(&mut out);
    assert!(has_opt(&query), "once more at the same level");
    r.udp_reply(tx, &R::rcode(RCODE_SERVFAIL).bytes(&query), 0, &mut rnd(), &mut out);
    let (tx, _, query) = udp(&mut out);
    assert!(!has_opt(&query), "then without EDNS(0)");
    r.udp_reply(tx, &R::rcode(RCODE_SERVFAIL).opt_off().bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Rcode(RCODE_SERVFAIL)));
    assert_eq!(r.lookup_cached("broken.example", 9 * S), Some(Outcome::Rcode(RCODE_SERVFAIL)));
    assert_eq!(r.lookup_cached("broken.example", 11 * S), None);
}

impl R {
    fn opt_off(mut self) -> Self { self.opt = false; self }
}

#[test]
fn servfail_on_the_lowest_level_tries_the_next_server() {
    let mut r = resolver(&[NS1, NS2]);
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    for _ in 0..2 {
        let (tx, s, query) = udp(&mut out);
        assert_eq!(s, NS1);
        r.udp_reply(tx, &R::rcode(RCODE_SERVFAIL).bytes(&query), 0, &mut rnd(), &mut out);
    }
    let (tx, s, query) = udp(&mut out);
    assert_eq!(s, NS1);
    r.udp_reply(tx, &R::rcode(RCODE_SERVFAIL).opt_off().bytes(&query), 0, &mut rnd(), &mut out);
    let (tx, s, query) = udp(&mut out);
    assert_eq!(s, NS2, "RFC 1123 §6.1.3.1: redundant servers");
    r.udp_reply(tx, &R::ok().a("x.example", 60, [9, 9, 9, 9]).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Addrs(vec![[9, 9, 9, 9]], 60)));
    assert_eq!(r.current_server(), Some(NS2), "the switch sticks");
}

#[test]
fn refused_moves_to_the_next_server() {
    let mut r = resolver(&[NS1, NS2]);
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::rcode(RCODE_REFUSED).bytes(&query), 0, &mut rnd(), &mut out);
    let (tx, s, query) = udp(&mut out);
    assert_eq!(s, NS2);
    r.udp_reply(tx, &R::rcode(RCODE_REFUSED).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Rcode(RCODE_REFUSED)), "every server refused");
}

#[test]
fn timeouts_rotate_the_servers_every_five_seconds_for_two_minutes() {
    let mut r = resolver(&[NS1, NS2]);
    let mut out = Out::default();
    let q = r.query("slow.example", 0, &mut rnd(), &mut out).unwrap();
    let mut seen = Vec::new();
    let mut now = 0;
    loop {
        if let Some(i) = out.send.iter().position(|s| matches!(s, Send::Udp { .. } | Send::Tcp { .. })) {
            match out.send.remove(i) {
                Send::Udp { server, .. } => seen.push((now / S, server, 'u')),
                Send::Tcp { server, tx, .. } => {
                    seen.push((now / S, server, 't'));
                    r.tcp_reply(tx, None, now, &mut rnd(), &mut out);
                    continue;
                }
                _ => unreachable!(),
            }
        }
        let Some(d) = r.next_deadline() else { break };
        now = d;
        r.tick(now, &mut rnd(), &mut out);
    }
    assert_eq!(r.take(q), Some(Outcome::Failed(Failure::Timeout)));
    assert_eq!(seen.len(), 24);
    assert_eq!(&seen[..3], &[(0, NS1, 'u'), (5, NS2, 'u'), (10, NS1, 'u')]);
    assert!(now <= 120 * S, "{now}");
}

#[test]
fn three_lost_datagrams_downgrade_the_server() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    for i in 0..3 {
        let (_, _, query) = udp(&mut out);
        assert!(has_opt(&query), "attempt {i}");
        r.tick((i + 1) * 5 * S, &mut rnd(), &mut out);
    }
    let (tx, _, query) = udp(&mut out);
    assert!(!has_opt(&query), "plain UDP after three losses at EDNS(0)");
    r.udp_reply(tx, &R::ok().opt_off().a("x.example", 60, [1, 1, 1, 1]).bytes(&query), 16 * S, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Addrs(vec![[1, 1, 1, 1]], 60)));
    assert_eq!(r.servers()[0].1, "UDP");
}

#[test]
fn a_reply_without_opt_restarts_without_edns() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    let id1 = u16::from_be_bytes([query[0], query[1]]);
    r.udp_reply(tx, &R::ok().opt_off().a("x.example", 60, [1, 1, 1, 1]).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), None, "restarted");
    let (tx, _, query) = udp(&mut out);
    assert!(!has_opt(&query));
    assert_ne!(u16::from_be_bytes([query[0], query[1]]), id1, "under a new ID");
    r.udp_reply(tx, &R::ok().opt_off().a("x.example", 60, [1, 1, 1, 1]).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Addrs(vec![[1, 1, 1, 1]], 60)));
}

#[test]
fn the_grace_period_brings_edns_back() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("x.example", S, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::ok().opt_off().a("x.example", 1, [1, 1, 1, 1]).bytes(&query), S, &mut rnd(), &mut out);
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::ok().opt_off().a("x.example", 1, [1, 1, 1, 1]).bytes(&query), S, &mut rnd(), &mut out);
    r.take(q).unwrap();
    let q = r.query("y.example", 300 * S, &mut rnd(), &mut out).unwrap();
    let (_, _, query) = udp(&mut out);
    assert!(!has_opt(&query));
    r.cancel(q, &mut out);
    out.send.clear();
    r.query("z.example", 302 * S, &mut rnd(), &mut out).unwrap();
    let (_, _, query) = udp(&mut out);
    assert!(has_opt(&query), "after 5 min the full set again");
}

#[test]
fn a_truncated_reply_is_asked_again_over_tcp_and_not_cached() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("big.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    let mut tc = R::ok().a("big.example", 60, [1, 1, 1, 1]);
    tc.tc = true;
    r.udp_reply(tx, &tc.bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.lookup_cached("big.example", 0), None);
    let Some(Send::Tcp { tx: ttx, server, bytes }) = out.send.iter().find(|s| matches!(s, Send::Tcp { .. })).cloned() else {
        panic!("TCP")
    };
    assert_eq!((ttx, server), (tx, NS1));
    let full = (0..40u8).fold(R::ok(), |r, i| r.a("big.example", 60, [10, 0, 0, i]));
    r.tcp_reply(tx, Some(&full.bytes(&bytes)), 0, &mut rnd(), &mut out);
    match r.take(q) {
        Some(Outcome::Addrs(a, 60)) => assert_eq!(a.len(), 40),
        o => panic!("{o:?}"),
    }
}

#[test]
fn a_ttl_with_the_top_bit_set_is_zero_and_not_cached() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::ok().a("x.example", 0x8000_0001, [1, 1, 1, 1]).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Addrs(vec![[1, 1, 1, 1]], 0)));
    assert_eq!(r.lookup_cached("x.example", 0), None);
}

#[test]
fn identical_questions_share_one_transaction() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let a = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let b = r.query("X.example.", 0, &mut rnd(), &mut out).unwrap();
    assert_eq!(sends(&out), 1);
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::ok().a("x.example", 60, [1, 1, 1, 1]).bytes(&query), 0, &mut rnd(), &mut out);
    assert_eq!(r.take(a), Some(Outcome::Addrs(vec![[1, 1, 1, 1]], 60)));
    assert_eq!(r.take(b), Some(Outcome::Addrs(vec![[1, 1, 1, 1]], 60)));
}

#[test]
fn an_icmp_error_tries_the_next_server_or_fails() {
    let mut r = resolver(&[NS1, NS2]);
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, _) = udp(&mut out);
    r.udp_error(tx, 0, &mut rnd(), &mut out);
    let (tx, s, _) = udp(&mut out);
    assert_eq!(s, NS2);
    r.udp_error(tx, 0, &mut rnd(), &mut out);
    assert_eq!(r.take(q), Some(Outcome::Failed(Failure::Unreachable)));
}

#[test]
fn no_network_and_no_servers_fail_at_once() {
    let mut r = resolver(&[NS1]);
    r.routable = false;
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    assert_eq!(r.take(q), Some(Outcome::Failed(Failure::NetworkDown)));
    let mut r = resolver(&[]);
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    assert_eq!(r.take(q), Some(Outcome::Failed(Failure::NoServers)));
}

#[test]
fn a_new_server_list_empties_the_cache_only_when_the_server_changes() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, _, query) = udp(&mut out);
    r.udp_reply(tx, &R::ok().a("x.example", 60, [1, 1, 1, 1]).bytes(&query), 0, &mut rnd(), &mut out);
    r.take(q);
    r.set_servers(&[NS1, NS2]);
    assert!(r.lookup_cached("x.example", 0).is_some());
    r.set_servers(&[NS2]);
    assert!(r.lookup_cached("x.example", 0).is_none());
}

#[test]
fn edns_announces_the_mtu_less_headers() {
    let mut r = resolver(&[NS1]);
    r.mtu = 1400;
    let mut out = Out::default();
    r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let (_, _, query) = udp(&mut out);
    let n = query.len();
    assert_eq!(u16::from_be_bytes([query[n - 8], query[n - 7]]), 1372);
}

#[test]
fn a_query_ends_after_two_minutes_even_while_tcp_hangs() {
    let mut r = resolver(&[NS1]);
    let mut out = Out::default();
    let q = r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let mut now = 0;
    // Every TCP attempt takes its full 10 s and fails.
    loop {
        out.send.retain(|s| matches!(s, Send::Tcp { .. }));
        if let Some(Send::Tcp { tx, .. }) = out.send.pop() {
            now += 10 * S;
            r.tcp_reply(tx, None, now, &mut rnd(), &mut out);
            r.tick(now, &mut rnd(), &mut out);
        } else {
            let Some(d) = r.next_deadline() else { break };
            now = d;
            r.tick(now, &mut rnd(), &mut out);
        }
        if let Some(o) = r.take(q) {
            assert_eq!(o, Outcome::Failed(Failure::Timeout));
            assert!(now <= 130 * S, "{now}");
            return;
        }
    }
    panic!("no outcome");
}

#[test]
fn a_datagram_that_cannot_be_sent_moves_to_the_next_server_at_once() {
    let mut r = resolver(&[NS1, NS2]);
    let mut out = Out::default();
    r.query("x.example", 0, &mut rnd(), &mut out).unwrap();
    let (tx, s, _) = udp(&mut out);
    assert_eq!(s, NS1);
    r.udp_send_failed(tx, 0, &mut rnd(), &mut out);
    let (_, s, _) = udp(&mut out);
    assert_eq!(s, NS2);
}
