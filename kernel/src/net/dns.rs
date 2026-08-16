//! DNS — Domain Name System
//!
//! Stub resolver over UDP port 53.
//! Queries A records, caches results.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use super::udp;

const DNS_PORT: u16 = 53;
const LOCAL_PORT: u16 = 10053;
const CACHE_SIZE: usize = 16;

static DNS_SERVER: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 3]); // QEMU user-mode DNS

/// The local port is fixed, so the transaction ID is the only thing that tells
/// this query's reply from a late one for an earlier name.
static NEXT_ID: Mutex<u16> = Mutex::new(0xABCD);

struct DnsEntry {
    name: String,
    ip: [u8; 4],
    valid: bool,
}

static CACHE: Mutex<[Option<DnsEntry>; CACHE_SIZE]> = Mutex::new(
    [const { None }; CACHE_SIZE]
);

pub fn set_server(ip: [u8; 4]) { *DNS_SERVER.lock() = ip; }
pub fn server() -> [u8; 4] { *DNS_SERVER.lock() }

/// Resolve a hostname to IPv4 address. Blocking (polls for reply).
pub fn resolve(name: &str) -> Option<[u8; 4]> {
    // Check cache first
    {
        let cache = CACHE.lock();
        if let Some(entry) = cache.iter().flatten().find(|e| e.valid && e.name == name) {
            return Some(entry.ip);
        }
    }

    let id = {
        let mut n = NEXT_ID.lock();
        *n = n.wrapping_add(1);
        *n
    };
    let query = build_query(name, id);

    // Warm the next hop's MAC. `arp::resolve` returns at once on a cache hit;
    // the blind 100 ms spin that stood here paid its timeout on EVERY uncached
    // name — five per Wikipedia load — which is why `dns` read a constant
    // ~110 ms while a TCP round trip to the same network took 20.
    // `arp_target_for` because a resolver off our subnet answers via the
    // gateway, and warming the resolver's own IP would never complete.
    //
    // 300 ms, not 100: the window is paid ONCE, on a cold cache, because every
    // name shares the same next hop. 100 ms held exactly one WiFi round trip,
    // so a single lost frame sent the query to L2 broadcast and the first
    // lookup after boot failed.
    let dns_server = *DNS_SERVER.lock();
    let hop = super::ipv4::arp_target_for(dns_server);
    if super::arp::resolve(hop, 30).is_none() {
        // Say it. The first lookup after boot fails often enough to be a known
        // annoyance, and from the outside "no MAC for the next hop" and "the
        // resolver did not answer" are the same silence — with opposite causes.
        // Only the failing case prints, so a warm cache stays quiet.
        crate::kprintln!("[npk] dns: next hop {}.{}.{}.{} did not answer ARP in 300 ms \
                          - query goes out to L2 broadcast", hop[0], hop[1], hop[2], hop[3]);
    }

    udp::listen(LOCAL_PORT);

    // Split into legs: UDP has no retransmit of its own, so a single dropped
    // datagram used to cost the whole timeout AND then fail. Now it costs one leg.
    //
    // The total was 2 s and that was simply too short. Measured on the device:
    // the first lookup after boot failed while the next hop's MAC was already
    // known — so nothing was lost on our side, the resolver just had not answered
    // yet. It is the RECURSION that takes the time; the same name a second later
    // comes out of the router's cache instantly. glibc gives a server 5 s before
    // it gives up, twice over, and undercutting that by more than half turned a
    // slow answer into a failed one. 5.5 s, still front-loaded so the common fast
    // case is unaffected.
    const LEGS: [u64; 4] = [50, 100, 200, 200]; // 100 Hz ticks
    let mut result = None;
    let mut answered_on = 0usize;
    // Datagrams that reached our port at all, and the id of the first one we
    // rejected. "Nothing arrived", "something arrived that was not ours" and
    // "ours arrived and parsed to nothing" are three different faults that all
    // end as one silent failure, and we have now guessed wrong about which one
    // it is twice.
    let mut seen = 0u32;
    let mut foreign_id = 0u16;
    'legs: for (n, leg) in LEGS.iter().enumerate() {
        udp::send(dns_server, LOCAL_PORT, DNS_PORT, &query);
        let t0 = crate::interrupts::ticks();
        while crate::interrupts::ticks().wrapping_sub(t0) < *leg {
            super::poll();
            if let Some((_src_ip, _src_port, data)) = udp::recv(LOCAL_PORT) {
                seen += 1;
                // Only OUR reply ends the wait — a negative answer is an
                // answer, a stale one is not.
                if is_reply_to(&data, id) {
                    result = parse_response(&data);
                    answered_on = n + 1;
                    if result.is_none() {
                        crate::kprintln!("[npk] dns: reply for {} carried no A record                                           ({} bytes, an={})", name, data.len(),
                            if data.len() >= 8 { u16::from_be_bytes([data[6], data[7]]) } else { 0 });
                    }
                    break 'legs;
                }
                if foreign_id == 0 && data.len() >= 2 {
                    foreign_id = u16::from_be_bytes([data[0], data[1]]);
                }
            }
            core::hint::spin_loop();
        }
    }
    // Which leg answered is the whole question: leg 1 is a healthy resolver, a
    // later one means the budget was the thing that used to fail us.
    if answered_on > 1 {
        crate::kprintln!("[npk] dns: {} answered on attempt {} (slow recursion, not a lost frame)",
            name, answered_on);
    }

    udp::unlisten(LOCAL_PORT);

    // A failed lookup names what it had to work with. Whether the next hop's MAC
    // was known decides where to look next, and reconstructing that afterwards
    // is impossible — by the time anyone asks, the cache is warm.
    if result.is_none() {
        crate::kprintln!("[npk] dns: no answer for {} after 5.5 s ({} attempts), next hop {}, \
                          {} datagram(s) on our port (id 0x{:04x} wanted, 0x{:04x} seen)",
            name, LEGS.len(),
            if super::arp::lookup(hop).is_some() { "was resolved" } else { "still UNRESOLVED" },
            seen, id, foreign_id);
    }

    // Cache result
    if let Some(ip) = result {
        let mut cache = CACHE.lock();
        let name_str = String::from(name);
        if let Some(slot) = cache.iter_mut().find(|s| s.is_none()) {
            *slot = Some(DnsEntry { name: name_str, ip, valid: true });
        } else if let Some(slot) = cache.iter_mut().find(|s| s.is_some()) {
            *slot = Some(DnsEntry { name: name_str, ip, valid: true });
        }
    }

    result
}

fn build_query(name: &str, id: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(512);

    // Header
    pkt.extend_from_slice(&id.to_be_bytes());   // Transaction ID
    pkt.extend_from_slice(&0x0100u16.to_be_bytes()); // Flags: standard query, recursion desired
    pkt.extend_from_slice(&1u16.to_be_bytes());  // Questions: 1
    pkt.extend_from_slice(&0u16.to_be_bytes());  // Answers: 0
    pkt.extend_from_slice(&0u16.to_be_bytes());  // Authority: 0
    pkt.extend_from_slice(&0u16.to_be_bytes());  // Additional: 0

    // Question: QNAME
    for label in name.split('.') {
        let len = label.len().min(63);
        pkt.push(len as u8);
        pkt.extend_from_slice(&label.as_bytes()[..len]);
    }
    pkt.push(0); // root label

    pkt.extend_from_slice(&1u16.to_be_bytes());  // QTYPE: A (IPv4)
    pkt.extend_from_slice(&1u16.to_be_bytes());  // QCLASS: IN

    pkt
}

/// Is this datagram a response carrying our transaction ID?
fn is_reply_to(data: &[u8], id: u16) -> bool {
    data.len() >= 12
        && u16::from_be_bytes([data[0], data[1]]) == id
        && u16::from_be_bytes([data[2], data[3]]) & 0x8000 != 0
}

fn parse_response(data: &[u8]) -> Option<[u8; 4]> {
    if data.len() < 12 { return None; }

    let flags = u16::from_be_bytes([data[2], data[3]]);
    if flags & 0x8000 == 0 { return None; } // not a response
    let rcode = flags & 0x0F;
    if rcode != 0 { return None; } // error

    let qdcount = u16::from_be_bytes([data[4], data[5]]) as usize;
    let ancount = u16::from_be_bytes([data[6], data[7]]) as usize;
    if ancount == 0 { return None; }

    // Skip questions
    let mut pos = 12;
    for _ in 0..qdcount {
        pos = skip_name(data, pos)?;
        pos += 4; // QTYPE + QCLASS
        if pos > data.len() { return None; }
    }

    // Parse answers, look for A record
    for _ in 0..ancount {
        if pos >= data.len() { return None; }
        pos = skip_name(data, pos)?;
        if pos + 10 > data.len() { return None; }

        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let _rclass = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
        let _ttl = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let rdlength = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;

        if rtype == 1 && rdlength == 4 && pos + 4 <= data.len() {
            return Some([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        }

        pos += rdlength;
    }

    None
}

fn skip_name(data: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        if pos >= data.len() { return None; }
        let len = data[pos] as usize;
        if len == 0 { return Some(pos + 1); }
        if len & 0xC0 == 0xC0 {
            // Compression pointer
            return Some(pos + 2);
        }
        pos += 1 + len;
    }
}
