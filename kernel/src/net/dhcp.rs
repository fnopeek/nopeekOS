//! DHCP — Dynamic Host Configuration Protocol
//!
//! Auto-configures IP address, gateway, and DNS server.
//! DHCP discover → offer → request → ack over UDP 67/68.

use alloc::vec::Vec;
use crate::kprintln;
use super::{udp, arp, dns};
use crate::netdev;

const SERVER_PORT: u16 = 67;
const CLIENT_PORT: u16 = 68;
const DHCP_MAGIC: [u8; 4] = [99, 130, 83, 99]; // DHCP magic cookie

const MSG_DISCOVER: u8 = 1;
const MSG_OFFER: u8    = 2;
const MSG_REQUEST: u8  = 3;
const MSG_ACK: u8      = 5;

/// Run DHCP to get network configuration. Blocking.
pub fn configure() -> bool {
    let mac = match netdev::mac() {
        Some(m) => m,
        None => return false,
    };

    // Temporarily set IP to 0.0.0.0 for DHCP
    arp::set_ip([0, 0, 0, 0]);

    udp::listen(CLIENT_PORT);

    // 1. DISCOVER
    let discover = build_dhcp(&mac, MSG_DISCOVER, [0; 4], [0; 4]);
    udp::send([255, 255, 255, 255], CLIENT_PORT, SERVER_PORT, &discover);

    // 2. Wait for OFFER
    let (offered_ip, server_ip) = match wait_dhcp_reply(MSG_OFFER) {
        Some(v) => v,
        None => {
            kprintln!("[npk] DHCP: no offer received");
            udp::unlisten(CLIENT_PORT);
            arp::set_ip([10, 0, 2, 15]); // fallback
            return false;
        }
    };

    // 3. REQUEST
    let request = build_dhcp(&mac, MSG_REQUEST, offered_ip, server_ip);
    udp::send([255, 255, 255, 255], CLIENT_PORT, SERVER_PORT, &request);

    // 4. Wait for ACK
    let (ack_ip, _) = match wait_dhcp_reply(MSG_ACK) {
        Some(v) => v,
        None => {
            kprintln!("[npk] DHCP: no ack received");
            udp::unlisten(CLIENT_PORT);
            arp::set_ip([10, 0, 2, 15]);
            return false;
        }
    };

    udp::unlisten(CLIENT_PORT);

    arp::set_ip(ack_ip);
    kprintln!("[npk] DHCP: configured {}.{}.{}.{}",
        ack_ip[0], ack_ip[1], ack_ip[2], ack_ip[3]);

    true
}

fn wait_dhcp_reply(expected_type: u8) -> Option<([u8; 4], [u8; 4])> {
    let t0 = crate::interrupts::ticks();
    loop {
        super::poll();
        if let Some((_src_ip, _src_port, data)) = udp::recv(CLIENT_PORT) {
            if let Some(result) = parse_dhcp_reply(&data, expected_type) {
                return Some(result);
            }
        }
        if crate::interrupts::ticks() - t0 > 300 { return None; } // 3s timeout
        core::hint::spin_loop();
    }
}

fn build_dhcp(mac: &[u8; 6], msg_type: u8, requested_ip: [u8; 4], server_ip: [u8; 4]) -> Vec<u8> {
    let mut pkt = alloc::vec![0u8; 300];

    pkt[0] = 1;      // op: BOOTREQUEST
    pkt[1] = 1;      // htype: Ethernet
    pkt[2] = 6;      // hlen: MAC length
    pkt[3] = 0;      // hops
    pkt[4..8].copy_from_slice(&0xDEADBEEFu32.to_be_bytes()); // xid
    // secs, flags at 8..12 = 0
    // ciaddr at 12..16 = 0
    // yiaddr at 16..20 = 0
    // siaddr at 20..24 = 0
    // giaddr at 24..28 = 0
    pkt[28..34].copy_from_slice(mac); // chaddr (16 bytes, MAC + padding)

    // DHCP magic cookie at offset 236
    pkt[236..240].copy_from_slice(&DHCP_MAGIC);

    // Options start at 240
    let mut pos = 240;

    // Option 53: DHCP Message Type
    pkt[pos] = 53; pkt[pos + 1] = 1; pkt[pos + 2] = msg_type;
    pos += 3;

    if msg_type == MSG_REQUEST {
        // Option 50: Requested IP
        pkt[pos] = 50; pkt[pos + 1] = 4;
        pkt[pos + 2..pos + 6].copy_from_slice(&requested_ip);
        pos += 6;

        // Option 54: Server Identifier
        if server_ip != [0; 4] {
            pkt[pos] = 54; pkt[pos + 1] = 4;
            pkt[pos + 2..pos + 6].copy_from_slice(&server_ip);
            pos += 6;
        }
    }

    // Option 55: Parameter Request List (router, DNS, subnet mask)
    pkt[pos] = 55; pkt[pos + 1] = 3;
    pkt[pos + 2] = 1;  // Subnet mask
    pkt[pos + 3] = 3;  // Router
    pkt[pos + 4] = 6;  // DNS
    pos += 5;

    // End option
    pkt[pos] = 255;

    pkt.truncate(pos + 1);
    pkt
}

fn parse_dhcp_reply(data: &[u8], expected_type: u8) -> Option<([u8; 4], [u8; 4])> {
    if data.len() < 240 { return None; }
    if data[0] != 2 { return None; } // not BOOTREPLY

    // Check magic cookie
    if data[236..240] != DHCP_MAGIC { return None; }

    let your_ip = <[u8; 4]>::try_from(&data[16..20]).unwrap();

    // Parse options
    let mut pos = 240;
    let mut msg_type = 0u8;
    let mut server_ip = [0u8; 4];
    let mut router = [0u8; 4];
    let mut dns_ip = [0u8; 4];

    while pos < data.len() {
        let opt = data[pos];
        if opt == 255 { break; } // end
        if opt == 0 { pos += 1; continue; } // padding
        if pos + 1 >= data.len() { break; }
        let len = data[pos + 1] as usize;
        let val_start = pos + 2;
        if val_start + len > data.len() { break; }

        match opt {
            53 if len >= 1 => msg_type = data[val_start],
            54 if len >= 4 => server_ip.copy_from_slice(&data[val_start..val_start + 4]),
            3 if len >= 4 => router.copy_from_slice(&data[val_start..val_start + 4]),
            6 if len >= 4 => dns_ip.copy_from_slice(&data[val_start..val_start + 4]),
            _ => {}
        }

        pos = val_start + len;
    }

    if msg_type != expected_type { return None; }

    // Apply gateway and DNS if provided
    if router != [0; 4] {
        // Store gateway for later use (ARP cache will resolve it)
    }
    if dns_ip != [0; 4] {
        dns::set_server(dns_ip);
    }

    Some((your_ip, server_ip))
}
