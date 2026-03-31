//! ARP — Address Resolution Protocol
//!
//! Maps IPv4 addresses to MAC addresses.
//! Maintains a small in-memory ARP cache.

use spin::Mutex;
use crate::netdev;
use super::eth;

const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16   = 2;
const HTYPE_ETH: u16   = 1;
const PTYPE_IPV4: u16  = 0x0800;

const CACHE_SIZE: usize = 16;

struct ArpEntry {
    ip: [u8; 4],
    mac: [u8; 6],
    valid: bool,
}

static CACHE: Mutex<[ArpEntry; CACHE_SIZE]> = Mutex::new(
    [const { ArpEntry { ip: [0; 4], mac: [0; 6], valid: false } }; CACHE_SIZE]
);

/// Our IP address (set during network init)
static OUR_IP: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 15]); // QEMU user-mode default

pub fn set_ip(ip: [u8; 4]) { *OUR_IP.lock() = ip; }
pub fn our_ip() -> [u8; 4] { *OUR_IP.lock() }

pub fn handle_arp(data: &[u8]) {
    if data.len() < 28 { return; }
    let op = u16::from_be_bytes([data[6], data[7]]);
    let sender_mac = <[u8; 6]>::try_from(&data[8..14]).unwrap();
    let sender_ip = <[u8; 4]>::try_from(&data[14..18]).unwrap();
    let target_ip = <[u8; 4]>::try_from(&data[24..28]).unwrap();

    // Learn sender's MAC
    cache_insert(sender_ip, sender_mac);

    let our_ip = *OUR_IP.lock();

    if op == ARP_REQUEST && target_ip == our_ip {
        // Send ARP reply
        let our_mac = netdev::mac().unwrap_or([0; 6]);
        let mut reply = [0u8; 28];
        reply[0..2].copy_from_slice(&HTYPE_ETH.to_be_bytes());
        reply[2..4].copy_from_slice(&PTYPE_IPV4.to_be_bytes());
        reply[4] = 6; // hardware size
        reply[5] = 4; // protocol size
        reply[6..8].copy_from_slice(&ARP_REPLY.to_be_bytes());
        reply[8..14].copy_from_slice(&our_mac);
        reply[14..18].copy_from_slice(&our_ip);
        reply[18..24].copy_from_slice(&sender_mac);
        reply[24..28].copy_from_slice(&sender_ip);

        let _ = eth::send_frame(&sender_mac, eth::ETHERTYPE_ARP, &reply);
    }
}

/// Send an ARP request for the given IP
pub fn request(target_ip: [u8; 4]) {
    let our_mac = netdev::mac().unwrap_or([0; 6]);
    let our_ip = *OUR_IP.lock();

    let mut pkt = [0u8; 28];
    pkt[0..2].copy_from_slice(&HTYPE_ETH.to_be_bytes());
    pkt[2..4].copy_from_slice(&PTYPE_IPV4.to_be_bytes());
    pkt[4] = 6;
    pkt[5] = 4;
    pkt[6..8].copy_from_slice(&ARP_REQUEST.to_be_bytes());
    pkt[8..14].copy_from_slice(&our_mac);
    pkt[14..18].copy_from_slice(&our_ip);
    pkt[18..24].copy_from_slice(&[0; 6]); // unknown target MAC
    pkt[24..28].copy_from_slice(&target_ip);

    let _ = eth::send_frame(&eth::BROADCAST, eth::ETHERTYPE_ARP, &pkt);
}

/// Lookup MAC for IP in ARP cache
pub fn lookup(ip: [u8; 4]) -> Option<[u8; 6]> {
    let cache = CACHE.lock();
    cache.iter().find(|e| e.valid && e.ip == ip).map(|e| e.mac)
}

fn cache_insert(ip: [u8; 4], mac: [u8; 6]) {
    let mut cache = CACHE.lock();
    // Update existing or find empty slot
    if let Some(entry) = cache.iter_mut().find(|e| e.valid && e.ip == ip) {
        entry.mac = mac;
        return;
    }
    if let Some(entry) = cache.iter_mut().find(|e| !e.valid) {
        *entry = ArpEntry { ip, mac, valid: true };
    }
}
