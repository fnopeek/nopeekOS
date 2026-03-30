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

struct DnsEntry {
    name: String,
    ip: [u8; 4],
    valid: bool,
}

static CACHE: Mutex<[Option<DnsEntry>; CACHE_SIZE]> = Mutex::new(
    [const { None }; CACHE_SIZE]
);

pub fn set_server(ip: [u8; 4]) { *DNS_SERVER.lock() = ip; }

/// Resolve a hostname to IPv4 address. Blocking (polls for reply).
pub fn resolve(name: &str) -> Option<[u8; 4]> {
    // Check cache first
    {
        let cache = CACHE.lock();
        if let Some(entry) = cache.iter().flatten().find(|e| e.valid && e.name == name) {
            return Some(entry.ip);
        }
    }

    // Build DNS query
    let query = build_query(name, 0xABCD);

    // Ensure ARP for DNS server
    let dns_server = *DNS_SERVER.lock();
    super::arp::request(dns_server);
    for _ in 0..50_000 {
        super::poll();
        core::hint::spin_loop();
    }

    // Send query
    udp::listen(LOCAL_PORT);
    udp::send(dns_server, LOCAL_PORT, DNS_PORT, &query);

    // Poll for reply (2 second timeout)
    let t0 = crate::interrupts::ticks();
    let result = loop {
        super::poll();
        if let Some((_src_ip, _src_port, data)) = udp::recv(LOCAL_PORT) {
            break parse_response(&data);
        }
        if crate::interrupts::ticks() - t0 > 200 { break None; }
        core::hint::spin_loop();
    };

    udp::unlisten(LOCAL_PORT);

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
