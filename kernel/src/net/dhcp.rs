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

// ── The exchange, as a state machine ─────────────────────────────────────
//
// This used to be straight-line code with a three-second busy-spin per reply,
// run up to three times over: up to nine seconds in which Core 0 did nothing
// else. Core 0 is the terminal, so that was the whole machine stopping — and it
// stopped LONGEST exactly when the link was broken, which is when a user most
// wants a prompt. Every step below returns immediately; `tick()` picks the
// reply up on a later pass of the Core-0 loop.
//
// This is safe because a UDP listener keeps the last datagram for its port
// until someone takes it: a reply landing between two ticks waits for us.

const BCAST: [u8; 4] = [255, 255, 255, 255];

/// How long one attempt waits before it is re-sent. A server on a working link
/// answers in milliseconds; the old three seconds only ever elapsed on a link
/// that was not going to answer at all.
const REPLY_WAIT_MS: u64 = 1000;
const DISCOVER_TRIES: u8 = 3;
const REQUEST_TRIES: u8 = 3;

#[derive(Clone, Copy)]
enum Phase {
    /// INIT-REBOOT (RFC 2131 §4.4.2): we still hold an address, waiting for ACK.
    Reboot,
    Discover,
    Request { offer: [u8; 4], server: [u8; 4] },
}

struct Exchange {
    phase: Phase,
    mac: [u8; 6],
    hint: [u8; 4],
    tries: u8,
    deadline: u64, // rdtsc
}

static EXCHANGE: spin::Mutex<Option<Exchange>> = spin::Mutex::new(None);

/// A lease is in, but the gateway has not answered ARP yet. Its MAC is what
/// lets the NEXT link change skip the exchange entirely, so it is worth
/// recording late rather than waiting for it now.
static GW_PENDING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// What `start()` did — it never blocks, so the caller needs to be told.
pub enum Start {
    /// The old lease is still valid; nothing is on the wire.
    Kept,
    /// An exchange is running. `tick()` will finish it.
    Running,
    /// No interface to do it on.
    Unavailable,
}

fn deadline_in(ms: u64) -> u64 {
    crate::interrupts::rdtsc()
        + crate::interrupts::tsc_freq().saturating_mul(ms) / 1000
}

pub fn is_running() -> bool {
    EXCHANGE.lock().is_some()
}

/// Begin a lease. Returns at once.
pub fn start() -> Start {
    if is_running() {
        return Start::Running;
    }
    let mac = match netdev::mac() {
        Some(m) => m,
        None => return Start::Unavailable,
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

    // Rung 0 (dhcpcd's shortcut, not in the RFC): the same gateway MAC means the
    // same segment, so the lease we hold is still good and there is nothing to
    // renegotiate. Cache-only — this used to block up to 300 ms on an ARP round
    // trip, and the whole point here is that nothing blocks. A cold cache just
    // means we take the rung below instead.
    if hint != [0, 0, 0, 0] {
        let gw = super::ipv4::gateway();
        let known = *LEASE_GW_MAC.lock();
        if gw != [0, 0, 0, 0] && known != [0; 6] {
            match arp::lookup(gw) {
                Some(now) if now == known => {
                    kprintln!("[npk] DHCP: same gateway after link change - lease {}.{}.{}.{} kept",
                        hint[0], hint[1], hint[2], hint[3]);
                    return Start::Kept;
                }
                Some(_) => {} // a different gateway — really is a new segment
                None => arp::request(gw), // warm it for next time, don't wait
            }
        }
    }

    udp::listen(CLIENT_PORT);
    arp::set_ip([0, 0, 0, 0]);

    // Rung 1, INIT-REBOOT: we still hold an address, so ask for THAT one — a
    // broadcast REQUEST carrying it in option 50 and no server identifier. One
    // round trip when the server still knows us. Silence falls through to the
    // full exchange, which is the point of the rung.
    let phase = if hint != [0, 0, 0, 0] {
        let reboot = build_dhcp(&mac, MSG_REQUEST, hint, [0; 4]);
        udp::send(BCAST, CLIENT_PORT, SERVER_PORT, &reboot);
        Phase::Reboot
    } else {
        let discover = build_dhcp(&mac, MSG_DISCOVER, hint, [0; 4]);
        udp::send(BCAST, CLIENT_PORT, SERVER_PORT, &discover);
        Phase::Discover
    };

    *EXCHANGE.lock() = Some(Exchange {
        phase,
        mac,
        hint,
        tries: 1,
        deadline: deadline_in(REPLY_WAIT_MS),
    });
    Start::Running
}

/// Step the exchange. Cheap and a no-op when nothing is running — call it from
/// the Core-0 loop as often as convenient.
pub fn tick() {
    settle_gateway();

    // Taken out of the lock for the duration: the step below sends packets and
    // prints, and nothing else may start a second exchange meanwhile.
    let mut ex = match EXCHANGE.lock().take() {
        Some(e) => e,
        None => return,
    };
    if let Step::Continue = step(&mut ex) {
        *EXCHANGE.lock() = Some(ex);
    }
}

enum Step {
    Continue,
    Done,
}

fn step(ex: &mut Exchange) -> Step {
    if let Some((_src_ip, _src_port, data)) = udp::recv(CLIENT_PORT) {
        match ex.phase {
            Phase::Reboot => {
                if let Some((ack, _)) = parse_dhcp_reply(&data, MSG_ACK) {
                    succeed(ex.mac, ack, true);
                    return Step::Done;
                }
            }
            Phase::Discover => {
                if let Some((offer, server)) = parse_dhcp_reply(&data, MSG_OFFER) {
                    let req = build_dhcp(&ex.mac, MSG_REQUEST, offer, server);
                    udp::send(BCAST, CLIENT_PORT, SERVER_PORT, &req);
                    ex.phase = Phase::Request { offer, server };
                    ex.tries = 1;
                    ex.deadline = deadline_in(REPLY_WAIT_MS);
                    return Step::Continue;
                }
            }
            Phase::Request { .. } => {
                if let Some((ack, _)) = parse_dhcp_reply(&data, MSG_ACK) {
                    succeed(ex.mac, ack, false);
                    return Step::Done;
                }
            }
        }
    }

    if crate::interrupts::rdtsc() < ex.deadline {
        return Step::Continue;
    }

    match ex.phase {
        Phase::Reboot => {
            kprintln!("[npk] DHCP: lease not reconfirmed - full exchange");
            let discover = build_dhcp(&ex.mac, MSG_DISCOVER, ex.hint, [0; 4]);
            udp::send(BCAST, CLIENT_PORT, SERVER_PORT, &discover);
            ex.phase = Phase::Discover;
            ex.tries = 1;
            ex.deadline = deadline_in(REPLY_WAIT_MS);
            Step::Continue
        }
        Phase::Discover => {
            if ex.tries >= DISCOVER_TRIES {
                kprintln!("[npk] DHCP: no offer received after {} attempts", DISCOVER_TRIES);
                give_up();
                return Step::Done;
            }
            ex.tries += 1;
            kprintln!("[npk] DHCP: retry {}...", ex.tries);
            let discover = build_dhcp(&ex.mac, MSG_DISCOVER, ex.hint, [0; 4]);
            udp::send(BCAST, CLIENT_PORT, SERVER_PORT, &discover);
            ex.deadline = deadline_in(REPLY_WAIT_MS);
            Step::Continue
        }
        Phase::Request { offer, server } => {
            if ex.tries >= REQUEST_TRIES {
                kprintln!("[npk] DHCP: no ack received");
                give_up();
                return Step::Done;
            }
            ex.tries += 1;
            let req = build_dhcp(&ex.mac, MSG_REQUEST, offer, server);
            udp::send(BCAST, CLIENT_PORT, SERVER_PORT, &req);
            ex.deadline = deadline_in(REPLY_WAIT_MS);
            Step::Continue
        }
    }
}

fn succeed(mac: [u8; 6], ip: [u8; 4], reconfirmed: bool) {
    udp::unlisten(CLIENT_PORT);
    *LEASE_MAC.lock() = mac;
    arp::set_ip(ip);
    // Tell the segment which MAC owns this address now — without it the router
    // keeps sending to the interface we just left.
    arp::announce();
    let gw = super::ipv4::gateway();
    if gw != [0, 0, 0, 0] {
        match arp::lookup(gw) {
            Some(m) => *LEASE_GW_MAC.lock() = m,
            None => {
                arp::request(gw);
                GW_PENDING.store(true, core::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    if reconfirmed {
        kprintln!("[npk] DHCP: lease {}.{}.{}.{} reconfirmed (no full exchange)",
            ip[0], ip[1], ip[2], ip[3]);
    } else {
        kprintln!("[npk] DHCP: configured {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    }
}

fn give_up() {
    udp::unlisten(CLIENT_PORT);
    no_lease();
}

/// Record the gateway's MAC once ARP answers, so the next link change can take
/// rung 0 above. Costs a cache lookup per tick while outstanding, nothing after.
fn settle_gateway() {
    use core::sync::atomic::Ordering::Relaxed;
    if !GW_PENDING.load(Relaxed) {
        return;
    }
    let gw = super::ipv4::gateway();
    if gw == [0, 0, 0, 0] {
        GW_PENDING.store(false, Relaxed);
        return;
    }
    if let Some(m) = arp::lookup(gw) {
        *LEASE_GW_MAC.lock() = m;
        GW_PENDING.store(false, Relaxed);
    }
}

/// Boot only: start an exchange and step it to a conclusion. Nothing else runs
/// this early and the rest of boot (NTP) wants an address, so waiting here is
/// honest — unlike the ~1 Hz link tick, which must never block the terminal.
pub fn run_blocking(max_ms: u64) -> bool {
    match start() {
        Start::Kept => return true,
        Start::Unavailable => return false,
        Start::Running => {}
    }
    let end = deadline_in(max_ms);
    while is_running() && crate::interrupts::rdtsc() < end {
        super::poll();
        tick();
        core::hint::spin_loop();
    }
    arp::our_ip() != [0, 0, 0, 0]
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
