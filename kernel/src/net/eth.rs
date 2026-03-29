//! Ethernet frame handling

use crate::{kprintln, virtio_net};

pub const ETHERTYPE_ARP: u16  = 0x0806;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const BROADCAST: [u8; 6] = [0xFF; 6];
pub const HEADER_LEN: usize = 14;

pub fn handle_frame(frame: &[u8]) {
    if frame.len() < HEADER_LEN { return; }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);

    match ethertype {
        ETHERTYPE_ARP => super::arp::handle_arp(&frame[HEADER_LEN..]),
        ETHERTYPE_IPV4 => super::ipv4::handle_ipv4(&frame[HEADER_LEN..]),
        _ => {} // ignore unknown
    }
}

/// Build and send an Ethernet frame
pub fn send_frame(dst: &[u8; 6], ethertype: u16, payload: &[u8]) -> Result<(), virtio_net::NetError> {
    let src = virtio_net::mac().unwrap_or([0; 6]);
    let mut frame = alloc::vec![0u8; HEADER_LEN + payload.len()];
    frame[0..6].copy_from_slice(dst);
    frame[6..12].copy_from_slice(&src);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
    frame[HEADER_LEN..].copy_from_slice(payload);
    virtio_net::send(&frame)
}
