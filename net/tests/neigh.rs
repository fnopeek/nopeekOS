//! The neighbour table against Linux's ARP behaviour.

use net_core::neigh::{Nud, Out, Queued, Reply, Table};

const US: [u8; 4] = [192, 0, 2, 1];
const GW: [u8; 4] = [192, 0, 2, 254];
const GW_HA: [u8; 6] = [2, 0, 0, 0, 0, 0xfe];
const OTHER_HA: [u8; 6] = [2, 0, 0, 0, 0, 0x77];

#[derive(Debug, PartialEq)]
struct Pkt(u32);

impl Queued for Pkt {
    fn truesize(&self) -> usize { 1000 }
}

type TestEnv = Out<Pkt>;

fn arp(op: u16, sha: [u8; 6], sip: [u8; 4], tha: [u8; 6], tip: [u8; 4]) -> Vec<u8> {
    let mut a = vec![0, 1, 8, 0, 6, 4];
    a.extend(op.to_be_bytes());
    a.extend(sha);
    a.extend(sip);
    a.extend(tha);
    a.extend(tip);
    a
}

fn reply_from_gw() -> Vec<u8> { arp(2, GW_HA, GW, [2, 0, 0, 0, 0, 1], US) }

fn run_ticks(t: &mut Table<Pkt>, env: &mut TestEnv, from: u64, to: u64) {
    for now in from..=to { t.tick(env, now, 0); }
}

fn state(t: &Table<Pkt>, ip: [u8; 4]) -> Option<Nud> {
    t.entries(0).into_iter().find(|e| e.0 == ip).map(|e| e.2)
}

#[test]
fn an_unresolved_packet_waits_instead_of_going_to_broadcast() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    t.output(&mut env, 10, GW, Pkt(1));
    t.output(&mut env, 10, GW, Pkt(2));
    assert!(env.send.is_empty(), "nothing goes out before the address is known");
    assert_eq!(env.solicit, [(GW, None)], "one broadcast request");
    assert_eq!(state(&t, GW), Some(Nud::Incomplete));

    t.arp_input(&mut env, 20, &reply_from_gw(), true, US);
    assert_eq!(env.send, [(GW_HA, Pkt(1)), (GW_HA, Pkt(2))], "the queue goes out in order");
    assert_eq!(state(&t, GW), Some(Nud::Reachable));

    t.output(&mut env, 21, GW, Pkt(3));
    assert_eq!(env.send.last(), Some(&(GW_HA, Pkt(3))));
}

#[test]
fn an_unanswered_next_hop_fails_after_three_requests_and_reports_its_packets() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    t.output(&mut env, 0, GW, Pkt(1));
    run_ticks(&mut t, &mut env, 1, 299);
    assert_eq!(env.solicit.len(), 3, "a request at 0, 1 and 2 s (RETRANS_TIME)");
    assert!(env.solicit.iter().all(|s| s.1.is_none()), "all broadcast");
    assert!(env.errors.is_empty());
    t.tick(&mut env, 300, 0);
    assert_eq!(state(&t, GW), Some(Nud::Failed));
    assert_eq!(env.errors, [Pkt(1)]);
    assert!(env.send.is_empty());

    // A new packet starts over.
    t.output(&mut env, 400, GW, Pkt(2));
    assert_eq!(state(&t, GW), Some(Nud::Incomplete));
    assert_eq!(env.solicit.len(), 4);
}

#[test]
fn unsolicited_arp_creates_no_entry() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    assert!(t.arp_input(&mut env, 0, &reply_from_gw(), true, US).is_none());
    assert!(t.arp_input(&mut env, 0, &arp(1, GW_HA, GW, [0; 6], [192, 0, 2, 9]), false, US).is_none());
    assert_eq!(t.lookup(GW), None);
}

#[test]
fn a_request_for_us_is_answered_and_its_sender_learned_stale() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    let r = t.arp_input(&mut env, 0, &arp(1, GW_HA, GW, [0; 6], US), false, US);
    assert!(matches!(r, Some(Reply::Answer { dst_ha: GW_HA, dst_ip: GW })));
    assert_eq!(state(&t, GW), Some(Nud::Stale));
    assert_eq!(t.lookup(GW), Some(GW_HA));
}

#[test]
fn unconfigured_we_answer_nothing() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    assert!(t.arp_input(&mut env, 0, &arp(1, GW_HA, GW, [0; 6], [0; 4]), false, [0; 4]).is_none());
    assert!(t.arp_input(&mut env, 0, &arp(1, GW_HA, [0; 4], [0; 6], [0; 4]), false, [0; 4]).is_none());
}

#[test]
fn an_address_probe_is_answered_but_not_learned() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    let r = t.arp_input(&mut env, 0, &arp(1, GW_HA, [0; 4], [0; 6], US), false, US);
    assert!(matches!(r, Some(Reply::Answer { dst_ha: GW_HA, dst_ip: [0, 0, 0, 0] })));
    assert!(t.entries(0).is_empty());
}

#[test]
fn malformed_arp_is_dropped() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    t.output(&mut env, 0, GW, Pkt(1));
    let mut bad = reply_from_gw();
    bad[4] = 8; // hardware length
    assert!(t.arp_input(&mut env, 1, &bad, true, US).is_none());
    let mut bad = reply_from_gw();
    bad[3] = 6; // protocol 0x0806
    t.arp_input(&mut env, 1, &bad, true, US);
    let mut bad = reply_from_gw();
    bad[7] = 9; // op
    t.arp_input(&mut env, 1, &bad, true, US);
    t.arp_input(&mut env, 1, &reply_from_gw()[..20], true, US);
    assert_eq!(state(&t, GW), Some(Nud::Incomplete));
    let mc = arp(2, GW_HA, GW, [0; 6], [224, 0, 0, 1]);
    t.arp_input(&mut env, 1, &mc, true, US);
    assert_eq!(state(&t, GW), Some(Nud::Incomplete), "a multicast target is dropped");
}

#[test]
fn the_first_of_back_to_back_replies_wins() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    t.output(&mut env, 0, GW, Pkt(1));
    t.arp_input(&mut env, 10, &reply_from_gw(), true, US);
    t.arp_input(&mut env, 50, &arp(2, OTHER_HA, GW, [0; 6], US), true, US);
    assert_eq!(t.lookup(GW), Some(GW_HA), "within LOCKTIME the first stays");
    t.arp_input(&mut env, 200, &arp(2, OTHER_HA, GW, [0; 6], US), true, US);
    assert_eq!(t.lookup(GW), Some(OTHER_HA), "after it the new one is taken");
}

#[test]
fn a_reachable_entry_ages_to_stale_and_is_revalidated_by_unicast() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    t.output(&mut env, 0, GW, Pkt(1));
    t.arp_input(&mut env, 10, &reply_from_gw(), true, US);
    env.solicit.clear();
    // Nothing used it through the slow path: after reachable_time, STALE.
    run_ticks(&mut t, &mut env, 11, 10 + 3000 + 1);
    assert_eq!(state(&t, GW), Some(Nud::Stale));
    // A packet to a stale entry goes out at once and starts DELAY.
    t.output(&mut env, 4000, GW, Pkt(2));
    assert_eq!(env.send.last(), Some(&(GW_HA, Pkt(2))));
    assert_eq!(state(&t, GW), Some(Nud::Delay));
    // After DELAY_PROBE_TIME, unicast probes to the cached address.
    run_ticks(&mut t, &mut env, 4001, 4500);
    assert_eq!(state(&t, GW), Some(Nud::Probe));
    assert!(!env.solicit.is_empty() && env.solicit.iter().all(|s| s.1 == Some(GW_HA)));
    // Answered: reachable again.
    t.arp_input(&mut env, 4550, &reply_from_gw(), true, US);
    assert_eq!(state(&t, GW), Some(Nud::Reachable));
}

#[test]
fn the_queue_keeps_the_newest_within_its_byte_limit() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    for i in 0..300 { t.output(&mut env, 0, GW, Pkt(i)); }
    t.arp_input(&mut env, 1, &reply_from_gw(), true, US);
    // SK_WMEM_DEFAULT / 1000 bytes each = 212 packets, the latest ones.
    assert_eq!(env.send.len(), 212);
    assert_eq!(env.send.first().unwrap().1, Pkt(88));
    assert_eq!(env.send.last().unwrap().1, Pkt(299));
    assert_eq!(t.drops, 88);
}

#[test]
fn the_table_grows_to_gc_thresh3_and_collects() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    for i in 0..1100u32 {
        let ip = [10, (i >> 16) as u8, (i >> 8) as u8, i as u8];
        t.resolve(&mut env, 0, ip);
    }
    assert_eq!(t.entries(0).len(), 1024, "creation stops at gc_thresh3 while all are in a timer");
    // Unanswered: after 3 s all have failed; failed entries are collected.
    run_ticks(&mut t, &mut env, 1, 300);
    assert!(t.entries(0).iter().all(|e| e.2 == Nud::Failed));
    assert!(t.resolve(&mut env, 400, [10, 9, 9, 9]), "a new entry finds room again");
}

#[test]
fn flush_forgets_everything() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    t.output(&mut env, 0, GW, Pkt(1));
    t.arp_input(&mut env, 10, &reply_from_gw(), true, US);
    t.flush();
    assert_eq!(t.lookup(GW), None);
    assert!(t.entries(0).is_empty());
}

#[test]
fn a_request_from_a_martian_or_our_own_address_is_ignored() {
    let mut t: Table<Pkt> = Table::new();
    let mut env = Out::default();
    for sip in [[224, 0, 0, 5], [255, 255, 255, 255], [127, 0, 0, 1], US] {
        assert!(t.arp_input(&mut env, 0, &arp(1, OTHER_HA, sip, [0; 6], US), false, US).is_none());
    }
    assert!(t.entries(0).is_empty());
}
