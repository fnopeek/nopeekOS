//! Address conflict detection against systemd's `sd-ipv4acd` (RFC 5227).

use net_core::acd::*;

const MAC: [u8; 6] = [2, 0, 0, 0, 0, 1];
const OTHER: [u8; 6] = [2, 0, 0, 0, 0, 9];
const OURS_TOO: [u8; 6] = [2, 0, 0, 0, 0, 2];
const ADDR: [u8; 4] = [192, 168, 1, 50];
/// The 200 ms default in units of a seventh.
const UNIT: u64 = 28_572;

/// The lower end of every random wait.
fn rnd() -> impl FnMut() -> u64 { || 0 }

fn arp(op: u16, sha: [u8; 6], spa: [u8; 4], tpa: [u8; 4]) -> Vec<u8> {
    let mut a = vec![0, 1, 8, 0, 6, 4];
    a.extend(op.to_be_bytes());
    a.extend(sha);
    a.extend(spa);
    a.extend([0; 6]);
    a.extend(tpa);
    a
}

/// Step every timer to the end, collecting what went out.
fn run_until(acd: &mut Acd, until: u64, out: &mut Out) -> u64 {
    let mut now = 0;
    while let Some(t) = acd.next_deadline().filter(|&t| t <= until) {
        now = t;
        acd.tick(now, &mut rnd(), out);
    }
    now
}

#[test]
fn three_probes_then_two_announcements() {
    let mut acd = Acd::new(MAC, 0);
    let mut out = Out::default();
    acd.start(ADDR, true, 0, &mut rnd(), &mut out);
    assert!(acd.is_running() && !acd.is_bound());
    assert_eq!(out.send.len(), 1, "the first probe at once");
    let mut times = vec![0];
    while let Some(t) = acd.next_deadline() {
        let before = out.send.len();
        acd.tick(t, &mut rnd(), &mut out);
        if out.send.len() > before {
            times.push(t);
        }
    }
    assert_eq!(out.send, [
        Send::Probe(ADDR), Send::Probe(ADDR), Send::Probe(ADDR),
        Send::Announcement(ADDR), Send::Announcement(ADDR),
    ]);
    assert_eq!(times, [0, UNIT, 2 * UNIT, 4 * UNIT, 4 * UNIT + 2_000_000]);
    assert_eq!(out.events, [Event::Bind]);
    assert_eq!(acd.state(), State::Running);
}

#[test]
fn bound_after_the_first_announcement() {
    let mut acd = Acd::new(MAC, 0);
    let mut out = Out::default();
    acd.start(ADDR, true, 0, &mut rnd(), &mut out);
    run_until(&mut acd, 4 * UNIT, &mut out);
    assert_eq!(out.events, [Event::Bind]);
    assert!(acd.is_bound());
}

#[test]
fn a_reply_from_the_address_while_probing_is_a_conflict() {
    let mut acd = Acd::new(MAC, 0);
    let mut out = Out::default();
    acd.start(ADDR, true, 0, &mut rnd(), &mut out);
    acd.packet(&arp(2, OTHER, ADDR, [0; 4]), 10, &[], &mut out);
    assert_eq!(out.events, [Event::Conflict]);
    assert!(!acd.is_running());
}

#[test]
fn another_hosts_probe_for_the_address_is_a_conflict_but_not_our_own() {
    let mut acd = Acd::new(MAC, 0);
    let mut out = Out::default();
    acd.start(ADDR, true, 0, &mut rnd(), &mut out);
    acd.packet(&arp(1, MAC, [0; 4], ADDR), 10, &[], &mut out);
    acd.packet(&arp(1, OURS_TOO, [0; 4], ADDR), 10, &[OURS_TOO], &mut out);
    acd.packet(&arp(2, OTHER, [0; 4], ADDR), 10, &[], &mut out);
    assert!(out.events.is_empty(), "our own frames, and a reply is no probe");
    acd.packet(&arp(1, OTHER, [0; 4], ADDR), 10, &[], &mut out);
    assert_eq!(out.events, [Event::Conflict]);
}

#[test]
fn unrelated_arp_is_ignored() {
    let mut acd = Acd::new(MAC, 0);
    let mut out = Out::default();
    acd.start(ADDR, true, 0, &mut rnd(), &mut out);
    acd.packet(&arp(1, OTHER, [192, 168, 1, 9], [192, 168, 1, 1]), 10, &[], &mut out);
    acd.packet(&arp(1, OTHER, [192, 168, 1, 9], ADDR), 10, &[], &mut out);
    assert!(out.events.is_empty(), "someone asking for the address is no conflict");
}

#[test]
fn a_bound_address_is_defended_once_per_ten_seconds() {
    let mut acd = Acd::new(MAC, 0);
    let mut out = Out::default();
    acd.start(ADDR, true, 0, &mut rnd(), &mut out);
    let now = run_until(&mut acd, u64::MAX, &mut out);
    out.send.clear();
    out.events.clear();
    let t = now + 1_000_000;
    acd.packet(&arp(1, OTHER, ADDR, ADDR), t, &[], &mut out);
    assert_eq!(out.send, [Send::Announcement(ADDR)], "defend");
    assert!(out.events.is_empty());
    acd.packet(&arp(1, OTHER, ADDR, ADDR), t + 9_000_000, &[], &mut out);
    assert_eq!(out.events, [Event::Conflict], "a second conflict inside the window");
}

#[test]
fn defending_again_after_the_window() {
    let mut acd = Acd::new(MAC, 0);
    let mut out = Out::default();
    acd.start(ADDR, true, 0, &mut rnd(), &mut out);
    let now = run_until(&mut acd, u64::MAX, &mut out);
    out.send.clear();
    out.events.clear();
    acd.packet(&arp(2, OTHER, ADDR, [0; 4]), now + 1, &[], &mut out);
    acd.packet(&arp(2, OTHER, ADDR, [0; 4]), now + 10_000_002, &[], &mut out);
    assert_eq!(out.send, [Send::Announcement(ADDR), Send::Announcement(ADDR)]);
    assert!(out.events.is_empty());
}

#[test]
fn ten_conflicts_slow_the_next_probe_to_a_minute() {
    let mut acd = Acd::new(MAC, 0);
    let mut out = Out::default();
    for _ in 0..10 {
        acd.start(ADDR, false, 0, &mut rnd(), &mut out);
        acd.packet(&arp(2, OTHER, ADDR, [0; 4]), 1, &[], &mut out);
    }
    out.send.clear();
    acd.start(ADDR, false, 0, &mut rnd(), &mut out);
    assert!(out.send.is_empty());
    assert_eq!(acd.next_deadline(), Some(60_000_000));
}
