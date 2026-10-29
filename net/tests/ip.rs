//! IPv4 receive and send checks against RFC 1122 and Linux.

use net_core::ip::{options_ok, Dest, Drop, Iface, PktType};

const IF: Iface = Iface { ip: [192, 168, 1, 10], mask: [255, 255, 255, 0] };
const NONE: Iface = Iface { ip: [0; 4], mask: [0; 4] };
const PEER: [u8; 4] = [192, 168, 1, 20];
const FAR: [u8; 4] = [8, 8, 8, 8];

fn rx(src: [u8; 4], dst: [u8; 4]) -> Result<Dest, Drop> {
    IF.input_route(PktType::Host, src, dst, false)
}

#[test]
fn ordinary_traffic_is_delivered() {
    assert_eq!(rx(PEER, IF.ip), Ok(Dest::Local));
    assert_eq!(rx(FAR, IF.ip), Ok(Dest::Local));
    assert_eq!(IF.input_route(PktType::Broadcast, PEER, [255; 4], false), Ok(Dest::Broadcast));
}

#[test]
fn every_broadcast_form_is_recognised() {
    for dst in [[255; 4], [192, 168, 1, 255], [192, 168, 1, 0]] {
        assert_eq!(IF.input_route(PktType::Broadcast, PEER, dst, false), Ok(Dest::Broadcast), "{dst:?}");
    }
}

#[test]
fn martian_sources_are_dropped() {
    for src in [[0, 0, 0, 7], [127, 0, 0, 1], [224, 0, 0, 1], [255; 4], IF.ip, [192, 168, 1, 255], [192, 168, 1, 0]] {
        assert_eq!(rx(src, IF.ip), Err(Drop::MartianSource), "{src:?}");
    }
}

#[test]
fn a_zero_source_may_only_broadcast() {
    assert_eq!(IF.input_route(PktType::Broadcast, [0; 4], [255; 4], false), Ok(Dest::Broadcast));
    assert_eq!(rx([0; 4], IF.ip), Err(Drop::MartianSource));
}

#[test]
fn martian_and_foreign_destinations_are_dropped() {
    assert_eq!(rx(PEER, [0, 1, 2, 3]), Err(Drop::MartianDest));
    assert_eq!(rx(PEER, [127, 0, 0, 1]), Err(Drop::MartianDest));
    assert_eq!(rx(PEER, [192, 168, 1, 99]), Err(Drop::NotForUs));
    assert_eq!(rx(PEER, [224, 0, 0, 251]), Err(Drop::NotForUs));
}

#[test]
fn the_link_layer_destination_counts() {
    assert_eq!(IF.input_route(PktType::OtherHost, PEER, IF.ip, false), Err(Drop::OtherHost));
    assert_eq!(IF.input_route(PktType::Broadcast, PEER, IF.ip, false), Err(Drop::UnicastInL2Multicast));
    assert_eq!(IF.input_route(PktType::Multicast, PEER, IF.ip, false), Err(Drop::UnicastInL2Multicast));
    assert_eq!(IF.input_route(PktType::Host, PEER, [255; 4], false), Ok(Dest::Broadcast));
}

#[test]
fn unconfigured_only_dhcp_and_broadcast_come_in() {
    assert_eq!(NONE.input_route(PktType::Host, PEER, [192, 168, 1, 77], true), Ok(Dest::Local));
    assert_eq!(NONE.input_route(PktType::Host, PEER, [192, 168, 1, 77], false), Err(Drop::NotForUs));
    assert_eq!(NONE.input_route(PktType::Broadcast, PEER, [255; 4], false), Ok(Dest::Broadcast));
    // A new lease offered while the old address is still set.
    assert_eq!(IF.input_route(PktType::Host, PEER, [10, 0, 0, 5], true), Ok(Dest::Local));
    assert_eq!(IF.input_route(PktType::Host, PEER, [10, 0, 0, 5], false), Err(Drop::NotForUs));
}

#[test]
fn output_refuses_what_leads_nowhere() {
    for dst in [[127, 0, 0, 1], [0, 0, 0, 0], [0, 9, 9, 9], [240, 0, 0, 1]] {
        assert!(!IF.output_ok(dst, false), "{dst:?}");
    }
    for dst in [[255; 4], [192, 168, 1, 255], [224, 0, 0, 1]] {
        assert!(!IF.output_ok(dst, true), "TCP to {dst:?}");
        assert!(IF.output_ok(dst, false), "UDP to {dst:?}");
    }
    assert!(IF.output_ok(FAR, true));
    assert!(IF.output_ok(PEER, true));
}

#[test]
fn well_formed_options_pass() {
    assert_eq!(options_ok(&[]).map_err(|e| e.0), Ok(()));
    assert_eq!(options_ok(&[1, 1, 1, 0]).map_err(|e| e.0), Ok(()));
    // Record route with room for two addresses, pointer at the first.
    assert_eq!(options_ok(&[7, 11, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0]).map_err(|e| e.0), Ok(()));
    // Timestamp only, room for one.
    assert_eq!(options_ok(&[68, 8, 5, 0, 0, 0, 0, 0]).map_err(|e| e.0), Ok(()));
    // Router alert.
    assert_eq!(options_ok(&[148, 4, 0, 0]).map_err(|e| e.0), Ok(()));
    // An unknown option is skipped.
    assert_eq!(options_ok(&[130, 3, 0, 0]).map_err(|e| e.0), Ok(()));
}

#[test]
fn malformed_options_are_dropped() {
    assert_eq!(options_ok(&[7]).map_err(|e| e.0), Err(Drop::BadOptions), "no length");
    assert_eq!(options_ok(&[7, 1, 4, 0]).map_err(|e| e.0), Err(Drop::BadOptions), "length below 2");
    assert_eq!(options_ok(&[7, 9, 4, 0]).map_err(|e| e.0), Err(Drop::BadOptions), "longer than the header");
    assert_eq!(options_ok(&[7, 7, 3, 0, 0, 0, 0, 0]).map_err(|e| e.0), Err(Drop::BadOptions), "pointer below 4");
    assert_eq!(options_ok(&[7, 6, 4, 0, 0, 0, 0, 0]).map_err(|e| e.0), Err(Drop::BadOptions), "no room at the pointer");
    assert_eq!(options_ok(&[68, 3, 5, 0]).map_err(|e| e.0), Err(Drop::BadOptions), "timestamp too short");
    assert_eq!(options_ok(&[68, 8, 9, 0xf0, 0, 0, 0, 0]).map_err(|e| e.0), Err(Drop::BadOptions), "overflow count full");
    assert_eq!(options_ok(&[7, 7, 4, 0, 0, 0, 0, 7, 7, 4, 0, 0, 0, 0]).map_err(|e| e.0), Err(Drop::BadOptions), "record route twice");
}

#[test]
fn a_source_route_is_refused() {
    assert_eq!(options_ok(&[131, 7, 4, 1, 2, 3, 4, 0]).map_err(|e| e.0), Err(Drop::SourceRoute));
    assert_eq!(options_ok(&[137, 7, 4, 1, 2, 3, 4, 0]).map_err(|e| e.0), Err(Drop::SourceRoute));
    assert_eq!(options_ok(&[131, 2]).map_err(|e| e.0), Err(Drop::BadOptions), "a source route too short to read");
}

#[test]
fn a_malformed_option_names_its_byte() {
    // Record route with a pointer below 4: the pointer, at offset 2.
    assert_eq!(options_ok(&[1, 7, 7, 3, 0, 0, 0, 0, 0]), Err((Drop::BadOptions, 3)));
}

use net_core::ip::{icmp_error_message, IcmpLimit};

fn datagram(proto: u8, frag: u16, payload: &[u8]) -> Vec<u8> {
    let mut d = vec![0x45, 0, 0, 0, 0, 0, (frag >> 8) as u8, frag as u8, 64, proto, 0, 0];
    d.extend(PEER);
    d.extend(IF.ip);
    d.extend(payload);
    let len = d.len() as u16;
    d[2..4].copy_from_slice(&len.to_be_bytes());
    d
}

#[test]
fn an_error_quotes_the_datagram_within_576_bytes() {
    let m = icmp_error_message(&datagram(17, 0, &[0x30, 0x39, 0, 53, 0, 8, 0, 0]), 3, 3, 0).unwrap();
    assert_eq!(&m[..2], &[3, 3]);
    assert_eq!(m.len(), 8 + 28);
    let big = datagram(17, 0, &[0; 1400]);
    assert_eq!(icmp_error_message(&big, 3, 3, 0).unwrap().len(), 576 - 20);
    // The checksum holds.
    let mut sum = 0u32;
    for c in m.chunks(2) { sum += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32; }
    while sum >> 16 != 0 { sum = (sum & 0xffff) + (sum >> 16); }
    assert_eq!(sum, 0xffff);
}

#[test]
fn no_error_about_a_fragment_or_an_error() {
    assert!(icmp_error_message(&datagram(17, 0x0010, &[0; 8]), 3, 3, 0).is_none(), "non-initial fragment");
    assert!(icmp_error_message(&datagram(1, 0, &[3, 3, 0, 0, 0, 0, 0, 0]), 3, 2, 0).is_none(), "an ICMP error");
    assert!(icmp_error_message(&datagram(1, 0, &[8, 0, 0, 0, 0, 0, 0, 0]), 3, 2, 0).is_some(), "an echo request");
}

#[test]
fn errors_are_limited_per_destination_and_overall() {
    let mut l = IcmpLimit::new();
    // Per destination: a burst of 6, then one a second.
    let sent = (0..20).filter(|_| l.allow(PEER, 3, 3, 1000, || 0)).count();
    assert_eq!(sent, 6);
    assert!(!l.allow(PEER, 3, 3, 1050, || 0));
    assert!(l.allow(PEER, 3, 3, 1100, || 0));
    // Overall: at most the burst of 50 at once, across destinations.
    let mut l = IcmpLimit::new();
    let sent = (0..200u32).filter(|i| l.allow([10, 0, (i >> 8) as u8, *i as u8], 3, 3, 1000, || 1)).count();
    assert_eq!(sent, 50);
    // Fragmentation Needed and echo replies are not limited.
    assert!(l.allow(PEER, 3, 4, 1000, || 1));
    assert!(l.allow(PEER, 0, 0, 1000, || 1));
}
