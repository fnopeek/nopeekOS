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
    assert_eq!(options_ok(&[]), Ok(()));
    assert_eq!(options_ok(&[1, 1, 1, 0]), Ok(()));
    // Record route with room for two addresses, pointer at the first.
    assert_eq!(options_ok(&[7, 11, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0]), Ok(()));
    // Timestamp only, room for one.
    assert_eq!(options_ok(&[68, 8, 5, 0, 0, 0, 0, 0]), Ok(()));
    // Router alert.
    assert_eq!(options_ok(&[148, 4, 0, 0]), Ok(()));
    // An unknown option is skipped.
    assert_eq!(options_ok(&[130, 3, 0, 0]), Ok(()));
}

#[test]
fn malformed_options_are_dropped() {
    assert_eq!(options_ok(&[7]), Err(Drop::BadOptions), "no length");
    assert_eq!(options_ok(&[7, 1, 4, 0]), Err(Drop::BadOptions), "length below 2");
    assert_eq!(options_ok(&[7, 9, 4, 0]), Err(Drop::BadOptions), "longer than the header");
    assert_eq!(options_ok(&[7, 7, 3, 0, 0, 0, 0, 0]), Err(Drop::BadOptions), "pointer below 4");
    assert_eq!(options_ok(&[7, 6, 4, 0, 0, 0, 0, 0]), Err(Drop::BadOptions), "no room at the pointer");
    assert_eq!(options_ok(&[68, 3, 5, 0]), Err(Drop::BadOptions), "timestamp too short");
    assert_eq!(options_ok(&[68, 8, 9, 0xf0, 0, 0, 0, 0]), Err(Drop::BadOptions), "overflow count full");
    assert_eq!(options_ok(&[7, 7, 4, 0, 0, 0, 0, 7, 7, 4, 0, 0, 0, 0]), Err(Drop::BadOptions), "record route twice");
}

#[test]
fn a_source_route_is_refused() {
    assert_eq!(options_ok(&[131, 7, 4, 1, 2, 3, 4, 0]), Err(Drop::SourceRoute));
    assert_eq!(options_ok(&[137, 7, 4, 1, 2, 3, 4, 0]), Err(Drop::SourceRoute));
    assert_eq!(options_ok(&[131, 2]), Err(Drop::BadOptions), "a source route too short to read");
}
