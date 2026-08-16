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
/// What to leave behind when DHCP fails. The QEMU user-mode address only means
/// something under QEMU; on real hardware it is a fiction that makes `net` show
/// an address nobody can reach — and, worse, makes the retry logic think a lease
/// exists. Outside QEMU we leave 0.0.0.0, which is the truth and what the
/// link-state tick keys its retry on.
/// MAC the current lease was issued to. A lease is only ours to re-request from
/// the interface that got it.
static LEASE_MAC: spin::Mutex<[u8; 6]> = spin::Mutex::new([0; 6]);

/// The gateway's MAC at the time the lease was granted. If the same one answers
/// after a link came back, we are on the same segment and the lease still holds:
/// no DHCP is needed at all. This is what dhcpcd does before it considers any
/// exchange, and it is what turns a mesh hand-off from a multi-second stall into
/// one ARP round trip.
static LEASE_GW_MAC: spin::Mutex<[u8; 6]> = spin::Mutex::new([0; 6]);

fn no_lease() {
    *LEASE_MAC.lock() = [0; 6];
    *LEASE_GW_MAC.lock() = [0; 6];
    if crate::virtio_net::is_available() {
        arp::set_ip([10, 0, 2, 15]); // QEMU user-mode default
    } else {
        arp::set_ip([0, 0, 0, 0]);
    }
}

pub fn configure() -> bool {
    let mac = match netdev::mac() {
        Some(m) => m,
        None => return false,
    };

    // Keep our previous lease if we had a real one, so re-running DHCP (e.g. on
    // a link switch) doesn't needlessly churn the address — but ONLY if it was
    // this interface's lease. The address is global state while a lease belongs
    // to one MAC: hinting the wired NIC's address from the WiFi NIC asks the
    // server for something it has already given to someone else, so it hands out
    // a different one, and switching back repeats it in reverse. That is the
    // .72/.73 ping-pong on a machine with both interfaces up.
    let prev = arp::our_ip();
    let same_iface = *LEASE_MAC.lock() == mac;
    let hint = if same_iface && prev != [0, 0, 0, 0] && prev != [10, 0, 2, 15] {
        prev
    } else {
        [0; 4]
    };

    // ── Before starting over: is this even a different network? ──────────
    //
    // Re-running the full four-way exchange on every link event is not what a
    // DHCP client is supposed to do, and it is expensive in exactly the moment
    // it hurts: an AP hand-off inside one ESS keeps the subnet, so the lease we
    // hold is still valid. RFC 2131 gives the ladder, cheapest first, and a link
    // that came back on the same segment should never reach the bottom of it.
    //
    // Rung 0 (dhcpcd's shortcut, not in the RFC): ask the gateway who it is. The
    // same MAC means the same segment, so there is nothing to renegotiate.
    if hint != [0, 0, 0, 0] {
        let gw = super::ipv4::gateway();
        let known = *LEASE_GW_MAC.lock();
        if gw != [0, 0, 0, 0] && known != [0; 6] {
            if let Some(mac_now) = arp::resolve(gw, 30) {
                if mac_now == known {
                    kprintln!("[npk] DHCP: same gateway after link change - lease {}.{}.{}.{} kept",
                        hint[0], hint[1], hint[2], hint[3]);
                    return true;
                }
            }
        }
    }

    udp::listen(CLIENT_PORT);

    // Rung 1, INIT-REBOOT (RFC 2131 §4.4.2): we still hold an address, so ask
    // for THAT one — a broadcast REQUEST carrying it in option 50 and no server
    // identifier. One round trip when the server still knows us, against
    // DISCOVER/OFFER/REQUEST/ACK with up to three retries. A NAK or silence
    // falls through to the full exchange below, which is the point of the rung.
    if hint != [0, 0, 0, 0] {
        arp::set_ip([0, 0, 0, 0]);
        let reboot = build_dhcp(&mac, MSG_REQUEST, hint, [0; 4]);
        udp::send([255, 255, 255, 255], CLIENT_PORT, SERVER_PORT, &reboot);
        if let Some((ack_ip, _)) = wait_dhcp_reply(MSG_ACK) {
            udp::unlisten(CLIENT_PORT);
            *LEASE_MAC.lock() = mac;
            arp::set_ip(ack_ip);
            remember_gateway();
            kprintln!("[npk] DHCP: lease {}.{}.{}.{} reconfirmed (no full exchange)",
                ack_ip[0], ack_ip[1], ack_ip[2], ack_ip[3]);
            return true;
        }
        kprintln!("[npk] DHCP: lease not reconfirmed - full exchange");
    }

    // Temporarily set IP to 0.0.0.0 for DHCP
    arp::set_ip([0, 0, 0, 0]);

    // 1. DISCOVER (with retries), hinting our previous lease.
    let discover = build_dhcp(&mac, MSG_DISCOVER, hint, [0; 4]);
    let mut offer = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            kprintln!("[npk] DHCP: retry {}...", attempt + 1);
        }
        udp::send([255, 255, 255, 255], CLIENT_PORT, SERVER_PORT, &discover);
        if let Some(v) = wait_dhcp_reply(MSG_OFFER) {
            offer = Some(v);
            break;
        }
    }

    // 2. Check for OFFER
    let (offered_ip, server_ip) = match offer {
        Some(v) => v,
        None => {
            kprintln!("[npk] DHCP: no offer received after 3 attempts");
            udp::unlisten(CLIENT_PORT);
            no_lease();
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
            no_lease();
            return false;
        }
    };

    udp::unlisten(CLIENT_PORT);

    *LEASE_MAC.lock() = mac;
    arp::set_ip(ack_ip);
    remember_gateway();
    kprintln!("[npk] DHCP: configured {}.{}.{}.{}",
        ack_ip[0], ack_ip[1], ack_ip[2], ack_ip[3]);

    true
}

/// Note whose MAC the gateway had for this lease, so the next link event can be
/// answered with one ARP instead of a new lease.
fn remember_gateway() {
    let gw = super::ipv4::gateway();
    if gw == [0, 0, 0, 0] {
        return;
    }
    if let Some(m) = arp::resolve(gw, 50) {
        *LEASE_GW_MAC.lock() = m;
    }
}

fn wait_dhcp_reply(expected_type: u8) -> Option<([u8; 4], [u8; 4])> {
    let start = crate::interrupts::rdtsc();
    let timeout_ticks = crate::interrupts::tsc_freq() * 3; // 3 seconds
    loop {
        super::poll();
        if let Some((_src_ip, _src_port, data)) = udp::recv(CLIENT_PORT) {
            if let Some(result) = parse_dhcp_reply(&data, expected_type) {
                return Some(result);
            }
        }
        if crate::interrupts::rdtsc() - start > timeout_ticks {
            return None;
        }
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

    // Option 50: Requested IP — on the REQUEST (the offered address) and, as a
    // hint, on a DISCOVER when we want the server to keep our previous lease.
    if requested_ip != [0; 4] {
        pkt[pos] = 50; pkt[pos + 1] = 4;
        pkt[pos + 2..pos + 6].copy_from_slice(&requested_ip);
        pos += 6;
    }
    // Option 54: Server Identifier (REQUEST only)
    if msg_type == MSG_REQUEST && server_ip != [0; 4] {
        pkt[pos] = 54; pkt[pos + 1] = 4;
        pkt[pos + 2..pos + 6].copy_from_slice(&server_ip);
        pos += 6;
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
    let mut subnet = [0u8; 4];

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
            1 if len >= 4 => subnet.copy_from_slice(&data[val_start..val_start + 4]),
            6 if len >= 4 => dns_ip.copy_from_slice(&data[val_start..val_start + 4]),
            _ => {}
        }

        pos = val_start + len;
    }

    if msg_type != expected_type { return None; }

    // Apply gateway, subnet, and DNS
    if router != [0; 4] {
        super::ipv4::set_gateway(router);
    }
    if subnet != [0; 4] {
        super::ipv4::set_subnet(subnet);
    }
    if dns_ip != [0; 4] {
        dns::set_server(dns_ip);
    }

    Some((your_ip, server_ip))
}
