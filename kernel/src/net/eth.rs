//! Ethernet frame handling

use crate::netdev;

pub const ETHERTYPE_ARP: u16  = 0x0806;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const BROADCAST: [u8; 6] = [0xFF; 6];
pub const HEADER_LEN: usize = 14;

/// `csum_ok`: the card verified the frame's TCP/UDP checksum.
pub fn handle_frame(frame: &[u8], csum_ok: bool) {
    if frame.len() < HEADER_LEN { return; }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);

    match ethertype {
        ETHERTYPE_ARP => super::arp::handle_arp(frame),
        ETHERTYPE_IPV4 => super::ipv4::handle_ipv4(&frame[HEADER_LEN..], csum_ok),
        _ => {} // ignore unknown
    }
}

/// Build and send an Ethernet frame
pub fn send_frame(dst: &[u8; 6], ethertype: u16, payload: &[u8]) -> Result<(), crate::virtio_net::NetError> {
    netdev::send(&build_frame(dst, ethertype, payload))
}

/// `send_frame` through `netdev::try_send`: a full ring is refused at once.
pub fn try_send_frame(dst: &[u8; 6], ethertype: u16, payload: &[u8]) -> Result<(), crate::virtio_net::NetError> {
    netdev::try_send(&build_frame(dst, ethertype, payload))
}

/// `try_send_frame` for an IPv4 packet whose TCP/UDP check holds the
/// pseudo-header seed; `l4_off` is the L4 header's offset in `payload`.
pub fn try_send_frame_csum(dst: &[u8; 6], ethertype: u16, payload: &[u8], l4_off: usize,
                           tcp: bool) -> Result<(), crate::virtio_net::NetError> {
    let mut frame = build_frame(dst, ethertype, payload);
    netdev::try_send_csum(&mut frame, HEADER_LEN + l4_off, tcp)
}

fn build_frame(dst: &[u8; 6], ethertype: u16, payload: &[u8]) -> alloc::vec::Vec<u8> {
    let src = netdev::mac().unwrap_or([0; 6]);
    let mut frame = alloc::vec![0u8; HEADER_LEN + payload.len()];
    frame[0..6].copy_from_slice(dst);
    frame[6..12].copy_from_slice(&src);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
    frame[HEADER_LEN..].copy_from_slice(payload);
    frame
}
