//! DNS — Domain Name System
//!
//! Stub resolver over UDP port 53.
//! Queries A records, caches results.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use super::udp;

const DNS_PORT: u16 = 53;
const LOCAL_PORT: u16 = 10053;
/// A browser opening a single site touches twenty to forty names; the cache
/// must hold more than one page load or nothing is ever used twice.
const CACHE_SIZE: usize = 64;

static DNS_SERVER: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 3]); // QEMU user-mode DNS

/// The local port is fixed, so the transaction ID is the only thing that tells
/// this query's reply from a late one for an earlier name.
static NEXT_ID: Mutex<u16> = Mutex::new(0xABCD);

struct DnsEntry {
    name: String,
    ip: [u8; 4],
    /// true = an address. false = the resolver tried and got nothing; the entry
    /// exists to keep the next asker from starting the same doomed lookup.
    valid: bool,
    /// Two kinds of failure, and only one is information. `true`: the
    /// resolver answered and did not know the name. `false`: nothing came
    /// back, which may be a single lost frame and must not block the name
    /// for the whole negative TTL.
    denied: bool,
    /// Tick this entry was written. Evicts the oldest instead of always slot 0,
    /// and expires a negative entry.
    stamp: u64,
}

static CACHE: Mutex<[Option<DnsEntry>; CACHE_SIZE]> = Mutex::new(
    [const { None }; CACHE_SIZE]
);

/// How long a failed lookup keeps a name out of the resolver. Without it a name
/// that does not resolve is retried on every single query for it — and each
/// retry costs the full budget.
const NEG_TTL_TICKS: u64 = 3_000; // ~30 s at 100 Hz

/// Names queued for the background resolver. Filled by `want()` from callers
/// that must not block, drained by `pump_wanted()` on Core 0.
static WANTED: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const WANTED_MAX: usize = 16;

/// What the cache knows about a name, without touching the network.
pub enum Cached {
    Ip([u8; 4]),
    /// Looked up and failed, recently enough to still count.
    Failed,
    Unknown,
}

pub fn set_server(ip: [u8; 4]) { *DNS_SERVER.lock() = ip; }
pub fn server() -> [u8; 4] { *DNS_SERVER.lock() }

/// Cache-only lookup. Never sends, never waits.
///
/// For callers that must not block. The microvm data plane is one: it runs on
/// the vCPU fiber inside the virtio-net MMIO exit, and a blocking resolve there
/// does not just freeze the guest — `fiber::pump_peers()` bails out inside a
/// fiber, so the WASM NIC driver fiber sharing that core stops posting receive
/// buffers, and the reply being waited for can then never arrive.
pub fn cached(name: &str) -> Cached {
    let now = crate::interrupts::ticks();
    let cache = CACHE.lock();
    match cache.iter().flatten().find(|e| e.name == name) {
        Some(e) if e.valid => Cached::Ip(e.ip),
        Some(e) if now.wrapping_sub(e.stamp) < NEG_TTL_TICKS => Cached::Failed,
        _ => Cached::Unknown,
    }
}

/// Queue a name for the background resolver. Deduplicates against the cache and
/// against the queue; drops silently when the queue is full — the caller that
/// could not be answered asks again, and that is the retry.
pub fn want(name: &str) {
    if name.is_empty() || name.len() > 255 { return; }
    if !matches!(cached(name), Cached::Unknown) { return; }
    {
        let mut q = WANTED.lock();
        if q.len() >= WANTED_MAX || q.iter().any(|n| n == name) { return; }
        q.push_back(String::from(name));
    }
    // A worker resolves it (`docs/plan/CORES_AND_EVENTS.md`, stage 3):
    // `resolve` blocks up to 5.5 s, which Core 0 (cursor, rendering, shell)
    // cannot afford. A native task may block; placement gives it a free
    // worker. Without workers, Core 0 keeps pumping.
    if crate::smp::scheduler::worker_count() > 0
        && !PUMP_RUNNING.swap(true, core::sync::atomic::Ordering::AcqRel)
    {
        crate::smp::scheduler::spawn("dns", pump_task, 0);
    }
}

/// A resolver task is running (or queued). Set by `want`, cleared by the
/// task once the queue is empty — re-checked after clearing, so a name
/// queued in between is not left waiting.
static PUMP_RUNNING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn pump_task(_: u64) {
    use core::sync::atomic::Ordering;
    loop {
        while let Some(name) = { WANTED.lock().pop_front() } {
            if resolve(&name).is_none() {
                remember(&name, [0; 4], false);
            }
        }
        PUMP_RUNNING.store(false, Ordering::Release);
        if WANTED.lock().is_empty() || PUMP_RUNNING.swap(true, Ordering::AcqRel) {
            return;
        }
    }
}

/// Resolve one queued name on Core 0 — only on a machine without workers;
/// otherwise `want` hands the queue to a worker task.
pub fn pump_wanted() {
    if crate::smp::per_core::current_core_id() != 0 { return; }
    if crate::smp::scheduler::worker_count() > 0 { return; }
    let name = { WANTED.lock().pop_front() };
    let Some(name) = name else { return };
    if resolve(&name).is_none() {
        remember(&name, [0; 4], false);
    }
}

/// Write an entry, replacing the oldest when the table is full.
fn remember(name: &str, ip: [u8; 4], valid: bool) {
    remember_kind(name, ip, valid, false)
}

fn remember_kind(name: &str, ip: [u8; 4], valid: bool, denied: bool) {
    let stamp = crate::interrupts::ticks();
    let mut cache = CACHE.lock();
    if let Some(slot) = cache.iter_mut().find(|s| {
        s.as_ref().is_some_and(|e| e.name == name)
    }) {
        *slot = Some(DnsEntry { name: String::from(name), ip, valid, denied, stamp });
        return;
    }
    if let Some(slot) = cache.iter_mut().find(|s| s.is_none()) {
        *slot = Some(DnsEntry { name: String::from(name), ip, valid, denied, stamp });
        return;
    }
    let oldest = cache
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| s.as_ref().map_or(0, |e| e.stamp))
        .map_or(0, |(i, _)| i);
    cache[oldest] = Some(DnsEntry { name: String::from(name), ip, valid, denied, stamp });
}

/// Resolve a hostname to IPv4 address. Blocking (polls for reply).
pub fn resolve(name: &str) -> Option<[u8; 4]> {
    // An empty name would be a query for the root zone.
    if name.is_empty() || name.len() > 255 {
        return None;
    }
    // Check the cache first, positive and negative: "this name does not
    // exist" is an answer, and asking again would pay the full round trip.
    {
        let cache = CACHE.lock();
        if let Some(e) = cache.iter().flatten().find(|e| e.name == name) {
            if e.valid {
                return Some(e.ip);
            }
            // Only an explicit no skips the query; a timeout is retried.
            if e.denied && crate::interrupts::ticks().wrapping_sub(e.stamp) < NEG_TTL_TICKS {
                return None;
            }
        }
    }

    let id = {
        let mut n = NEXT_ID.lock();
        *n = n.wrapping_add(1);
        *n
    };
    let query = build_query(name, id);

    // Warm the next hop's MAC. `arp::resolve` returns at once on a cache hit,
    // so this costs nothing when warm. `arp_target_for` because a resolver off
    // our subnet answers via the gateway, and warming the resolver's own IP
    // would never complete.
    //
    // 300 ms: the window is paid only on a cold cache, since every name shares
    // the same next hop, and it must survive a lost frame over WiFi.
    let dns_server = *DNS_SERVER.lock();
    let hop = super::ipv4::arp_target_for(dns_server);
    if super::arp::resolve(hop, 30).is_none() {
        // Say it: from the outside "no MAC for the next hop" and "the resolver
        // did not answer" are the same silence, with opposite causes. Only the
        // failing case prints, so a warm cache stays quiet.
        crate::kprintln!("[npk] dns: next hop {}.{}.{}.{} did not answer ARP in 300 ms \
                          - query goes out to L2 broadcast", hop[0], hop[1], hop[2], hop[3]);
    }

    // The return value matters: eight listener slots exist, and a full table
    // means we send four queries and listen on nothing at all.
    if !udp::listen(LOCAL_PORT) {
        crate::kprintln!("[npk] dns: no UDP listener slot free - the query would go out deaf");
        return None;
    }

    // Split into legs: UDP has no retransmit of its own, so a dropped datagram
    // costs one leg instead of the whole budget.
    //
    // 5.5 s in total, front-loaded so the common fast case is unaffected.
    // Recursion on a cold resolver can take seconds; glibc gives a server 5 s
    // before giving up.
    const LEGS: [u64; 4] = [50, 100, 200, 200]; // 100 Hz ticks
    let mut result = None;
    let mut answered_on = 0usize;
    // Datagrams that reached our port at all, and the id of the first one we
    // rejected. "Nothing arrived", "something arrived that was not ours" and
    // "ours arrived and parsed to nothing" are three different faults that
    // would otherwise end as one silent failure.
    let mut seen = 0u32;
    let mut foreign_id: Option<u16> = None;
    // Did our reply come back at all? Not the same as "it carries an
    // address": a reply without an A record is still an answer.
    let mut answered = false;
    let mut sent = 0usize;
    let t_start = crate::interrupts::ticks();
    let udp_before = udp::rx_total();
    let (nl_before, _) = udp::no_listener_stats();
    'legs: for (n, leg) in LEGS.iter().enumerate() {
        udp::send(dns_server, LOCAL_PORT, DNS_PORT, &query);
        sent += 1;
        let t0 = crate::interrupts::ticks();
        while crate::interrupts::ticks().wrapping_sub(t0) < *leg {
            super::poll();
            if let Some((_src_ip, _src_port, data)) = udp::recv(LOCAL_PORT) {
                seen += 1;
                // Only our reply ends the wait — a negative answer is an
                // answer, a stale one is not.
                if is_reply_to(&data, id) {
                    result = parse_response(&data);
                    answered_on = n + 1;
                    answered = true;
                    if result.is_none() {
                        // Not a failure but a no: report the measured time.
                        crate::kprintln!(
                            "[npk] dns: {} gibt es nicht — der Aufloeser hat nach {} ms \
                             geantwortet ({} Bytes, an={}, rcode={})",
                            name,
                            crate::interrupts::ticks().wrapping_sub(t_start) * 10,
                            data.len(),
                            if data.len() >= 8 { u16::from_be_bytes([data[6], data[7]]) } else { 0 },
                            if data.len() >= 4 { u16::from_be_bytes([data[2], data[3]]) & 0x0F } else { 0 });
                    }
                    break 'legs;
                }
                if foreign_id.is_none() && data.len() >= 2 {
                    foreign_id = Some(u16::from_be_bytes([data[0], data[1]]));
                }
            }
            core::hint::spin_loop();
        }
    }
    // Which leg answered matters: leg 1 is a healthy resolver, a later one
    // means slow recursion rather than a lost frame.
    if answered_on > 1 {
        crate::kprintln!("[npk] dns: {} answered on attempt {} (slow recursion, not a lost frame)",
            name, answered_on);
    }

    udp::unlisten(LOCAL_PORT);

    // A failed lookup names what it had to work with. Whether the next hop's MAC
    // was known decides where to look next, and reconstructing that afterwards
    // is impossible — by the time anyone asks, the cache is warm.
    // Only when nothing came back: a reply without an A record was already
    // reported above.
    if result.is_none() && !answered {
        // The measured time and the number of legs actually sent.
        let waited_ms = crate::interrupts::ticks().wrapping_sub(t_start) * 10;
        crate::kprintln!("[npk] dns: no answer for {} after {} ms ({} attempts sent), \
                          next hop {}, {} datagram(s) on our port, id 0x{:04x} wanted",
            name, waited_ms, sent,
            if super::arp::lookup(hop).is_some() { "was resolved" } else { "still UNRESOLVED" },
            seen, id);
        // A foreign id is a separate finding; name it only if one arrived.
        if let Some(f) = foreign_id {
            crate::kprintln!("[npk] dns: ein Datagramm auf unserem Port trug die fremde \
                              Kennung 0x{:04x} — nicht unsere Antwort", f);
        }
        let (nl, nlp) = udp::no_listener_stats();
        crate::kprintln!("[npk] dns: during this lookup {} UDP datagram(s) reached the stack, \
                          {} of them with nobody listening (last such port {})",
            udp::rx_total().saturating_sub(udp_before), nl.saturating_sub(nl_before), nlp);
        // Did the query even reach the air? Every layer between here and the NIC
        // throws the send Result away, so without this the question cannot be
        // asked at all.
        let (no_link, tx_err) = crate::netdev::tx_reject_stats();
        if no_link > 0 || tx_err > 0 {
            crate::kprintln!("[npk] dns: TX refused since boot: {} for no link, {} by the driver",
                no_link, tx_err);
        }
    }

    // Cache the result, including an explicit no. A timeout is not cached:
    // it may be a single lost frame, and pinning it for the whole TTL would
    // turn a glitch into an outage.
    match result {
        Some(ip) => remember(name, ip, true),
        None if answered => remember_kind(name, [0; 4], false, true),
        None => {}
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
