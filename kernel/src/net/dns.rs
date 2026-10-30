//! DNS: the kernel's side of `net_core::dns`, the stub resolver after
//! systemd-resolved. Every transaction gets its own random UDP port, and a
//! reply counts only from its server's port 53 with the transaction's ID
//! and question; a truncated one is asked again over TCP.
//!
//! `resolve` blocks its caller and drives the resolver until the answer.
//! The microVM path never blocks: `cached` reads the cache, `want` queues
//! a name for a worker, and what the worker learnt waits in `RECENT` for
//! the guest's retry, so a TTL-0 answer or a failure reaches it once.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use net_core::dns::{Out, Outcome, Resolver, Send};
use spin::Mutex;
use super::{tcp, udp};

const DNS_PORT: u16 = 53;

static RESOLVER: Mutex<Option<Resolver>> = Mutex::new(None);

/// A transaction's UDP socket: (transaction, local port, server).
static SOCKS: Mutex<Vec<(u32, u16, [u8; 4])>> = Mutex::new(Vec::new());

/// Names queued for the background resolver. Filled by `want()` from callers
/// that must not block, drained by `pump_wanted()` on Core 0.
static WANTED: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const WANTED_MAX: usize = 16;

/// What the background resolver found, held for the asker's retry.
static RECENT: Mutex<VecDeque<(String, Outcome, u64)>> = Mutex::new(VecDeque::new());
const RECENT_MAX: usize = 64;
/// How long a held result waits for its retry.
const RECENT_USEC: u64 = 10_000_000;

fn now() -> u64 { crate::interrupts::uptime_us() }

fn with<R>(f: impl FnOnce(&mut Resolver) -> R) -> R {
    let mut g = RESOLVER.lock();
    f(g.get_or_insert_with(Resolver::new))
}

/// What the cache knows about a name, without touching the network.
pub enum Cached {
    Ip([u8; 4]),
    /// The name does not exist.
    NxDomain,
    /// The name exists without an address.
    NoData,
    /// The lookup failed; asking again may work.
    TempFail,
    Unknown,
}

/// The servers of a new configuration, in order of preference.
pub fn set_servers(list: &[[u8; 4]]) {
    with(|r| r.set_servers(list));
}

/// Each server and the feature level it is used at.
pub fn server_levels() -> Vec<([u8; 4], &'static str)> {
    with(|r| r.servers())
}

/// The server queries go to now, 0.0.0.0 without one.
pub fn server() -> [u8; 4] {
    with(|r| r.current_server()).unwrap_or([0; 4])
}

/// Forget every cached answer.
pub fn flush() {
    with(|r| r.flush());
    RECENT.lock().clear();
}

fn cached_of(o: &Outcome) -> Cached {
    match o {
        Outcome::Addrs(a, _) => a.first().map_or(Cached::NoData, |ip| Cached::Ip(*ip)),
        Outcome::NxDomain => Cached::NxDomain,
        Outcome::NoData => Cached::NoData,
        Outcome::Rcode(_) | Outcome::Failed(_) => Cached::TempFail,
    }
}

/// Cache-only lookup. Never sends, never waits.
///
/// For callers that must not block. The microvm data plane is one: it runs on
/// the vCPU fiber inside the virtio-net MMIO exit, and a blocking resolve there
/// does not just freeze the guest: `fiber::pump_peers()` bails out inside a
/// fiber, so the WASM NIC driver fiber sharing that core stops posting receive
/// buffers, and the reply being waited for can then never arrive.
pub fn cached(name: &str) -> Cached {
    let Some(key) = net_core::dns::normalize(name) else { return Cached::NxDomain };
    let t = now();
    {
        let mut recent = RECENT.lock();
        recent.retain(|e| t.saturating_sub(e.2) < RECENT_USEC);
        if let Some(i) = recent.iter().position(|e| e.0 == key) {
            let (_, o, _) = recent.remove(i).unwrap();
            return cached_of(&o);
        }
    }
    match with(|r| r.lookup_cached(&key, t)) {
        Some(o) => cached_of(&o),
        None => Cached::Unknown,
    }
}

fn hold(name: &str, o: Outcome) {
    let Some(key) = net_core::dns::normalize(name) else { return };
    let mut recent = RECENT.lock();
    recent.retain(|e| e.0 != key);
    if recent.len() >= RECENT_MAX {
        recent.pop_front();
    }
    recent.push_back((key, o, now()));
}

/// Queue a name for the background resolver. Deduplicates against the cache and
/// against the queue; drops silently when the queue is full: the caller that
/// could not be answered asks again, and that is the retry.
pub fn want(name: &str) {
    let Some(name) = net_core::dns::normalize(name) else { return };
    if !matches!(peek(&name), Cached::Unknown) { return; }
    {
        let mut q = WANTED.lock();
        if q.len() >= WANTED_MAX || q.iter().any(|n| *n == name) { return; }
        q.push_back(name);
    }
    // A worker resolves it: `resolve` blocks for up to two minutes, which
    // Core 0 (cursor, rendering, shell) cannot afford. A native task may
    // block; placement gives it a free worker. Without workers, Core 0
    // keeps pumping.
    if crate::smp::scheduler::worker_count() > 0
        && !PUMP_RUNNING.swap(true, core::sync::atomic::Ordering::AcqRel)
    {
        crate::smp::scheduler::spawn("dns", pump_task, 0);
    }
}

/// `cached` without taking a held result.
fn peek(name: &str) -> Cached {
    if let Some(e) = RECENT.lock().iter().find(|e| e.0 == name) {
        return cached_of(&e.1);
    }
    match with(|r| r.lookup_cached(name, now())) {
        Some(o) => cached_of(&o),
        None => Cached::Unknown,
    }
}

/// A resolver task is running (or queued). Set by `want`, cleared by the
/// task once the queue is empty, and checked again after clearing, so a name
/// queued in between is not left waiting.
static PUMP_RUNNING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn pump_task(_: u64) {
    use core::sync::atomic::Ordering;
    loop {
        while let Some(name) = { WANTED.lock().pop_front() } {
            let o = lookup(&name);
            hold(&name, o);
        }
        PUMP_RUNNING.store(false, Ordering::Release);
        if WANTED.lock().is_empty() || PUMP_RUNNING.swap(true, Ordering::AcqRel) {
            return;
        }
    }
}

/// Resolve one queued name on Core 0, only on a machine without workers;
/// otherwise `want` hands the queue to a worker task.
pub fn pump_wanted() {
    if crate::smp::per_core::current_core_id() != 0 { return; }
    if crate::smp::scheduler::worker_count() > 0 { return; }
    let name = { WANTED.lock().pop_front() };
    let Some(name) = name else { return };
    let o = lookup(&name);
    hold(&name, o);
}

/// Send what the resolver asked for. TCP is done here and now: the caller
/// blocks anyway, and its reply goes straight back in.
fn run(mut out: Out) {
    while !out.send.is_empty() {
        let mut next = Out::default();
        for s in out.send.drain(..) {
            match s {
                Send::Udp { tx, server, bytes } => send_udp(tx, server, &bytes, &mut next),
                Send::Tcp { tx, server, bytes } => {
                    let reply = tcp_exchange(server, &bytes);
                    let t = now();
                    with(|r| r.tcp_reply(tx, reply.as_deref(), t, &mut crate::csprng::random_u64, &mut next));
                }
                Send::Close { tx } => {
                    let mut socks = SOCKS.lock();
                    if let Some(i) = socks.iter().position(|s| s.0 == tx) {
                        udp::unlisten(socks.swap_remove(i).1);
                    }
                }
            }
        }
        out = next;
    }
}

/// A datagram from the transaction's port, a new one when its server
/// changed (resolved connects one socket per transaction and server).
fn send_udp(tx: u32, server: [u8; 4], bytes: &[u8], next: &mut Out) {
    let port = {
        let mut socks = SOCKS.lock();
        match socks.iter().position(|s| s.0 == tx) {
            Some(i) if socks[i].2 == server => Some(socks[i].1),
            found => {
                if let Some(i) = found {
                    udp::unlisten(socks.swap_remove(i).1);
                }
                let p = udp::listen_ephemeral();
                if let Some(p) = p {
                    socks.push((tx, p, server));
                }
                p
            }
        }
    };
    let Some(port) = port else {
        crate::kprintln!("[npk] dns: no UDP listener slot free");
        let t = now();
        with(|r| r.udp_error(tx, t, &mut crate::csprng::random_u64, next));
        return;
    };
    if !udp::send(server, port, DNS_PORT, bytes) {
        let t = now();
        with(|r| r.udp_send_failed(tx, t, &mut crate::csprng::random_u64, next));
    }
}

/// One query over TCP (RFC 1035 §4.2.2, RFC 7766): connect, the message
/// with its length, one reply, close. `None` when any of it fails.
fn tcp_exchange(server: [u8; 4], q: &[u8]) -> Option<Vec<u8>> {
    let h = tcp::connect(server, DNS_PORT).ok()?;
    let mut msg = Vec::with_capacity(q.len() + 2);
    msg.extend_from_slice(&(q.len() as u16).to_be_bytes());
    msg.extend_from_slice(q);
    let r = (|| {
        tcp::send_blocking(h, &msg, 1000).ok()?;
        let mut got = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            if got.len() >= 2 {
                let want = u16::from_be_bytes([got[0], got[1]]) as usize + 2;
                if got.len() >= want {
                    got.truncate(want);
                    return Some(got.split_off(2));
                }
            }
            let n = tcp::recv_blocking(h, &mut buf, 1000).ok()?;
            if n == 0 {
                return None;
            }
            got.extend_from_slice(&buf[..n]);
        }
    })();
    let _ = tcp::close(h);
    r
}

/// Hand every datagram and ICMP error on the transactions' ports to the
/// resolver.
fn poll_socks(out: &mut Out) {
    let socks: Vec<(u32, u16, [u8; 4])> = SOCKS.lock().clone();
    for (tx, port, server) in socks {
        let t = now();
        if udp::take_error(port) == Some((server, DNS_PORT)) {
            with(|r| r.udp_error(tx, t, &mut crate::csprng::random_u64, out));
            continue;
        }
        if let Some((src, sport, data)) = udp::recv(port) {
            if src == server && sport == DNS_PORT {
                with(|r| r.udp_reply(tx, &data, t, &mut crate::csprng::random_u64, out));
            }
        }
    }
}

/// Look `name` up, blocking until the resolver has an outcome.
pub fn lookup(name: &str) -> Outcome {
    let routable = super::arp::our_ip() != [0; 4];
    let mtu = super::ipv4::mtu() as usize;
    let mut out = Out::default();
    let q = with(|r| {
        r.routable = routable;
        r.mtu = mtu;
        r.query(name, now(), &mut crate::csprng::random_u64, &mut out)
    });
    let Some(q) = q else { return Outcome::NxDomain };
    loop {
        run(core::mem::take(&mut out));
        if let Some(o) = with(|r| r.take(q)) {
            return o;
        }
        super::poll();
        poll_socks(&mut out);
        let t = now();
        with(|r| {
            if r.next_deadline().is_some_and(|d| d <= t) {
                r.tick(t, &mut crate::csprng::random_u64, &mut out);
            }
        });
        // Halt until the next poll period instead of spinning: the wait can
        // last two minutes.
        if out.send.is_empty() {
            crate::interrupts::worker_idle_hlt();
        }
    }
}

/// Resolve a hostname to an IPv4 address. Blocking.
pub fn resolve(name: &str) -> Option<[u8; 4]> {
    if net_core::dns::normalize(name).is_none() {
        crate::kprintln!("[npk] dns: {}: not a valid name", name);
        return None;
    }
    let t0 = now();
    match lookup(name) {
        Outcome::Addrs(a, _) if !a.is_empty() => Some(a[0]),
        o => {
            let why = match o {
                Outcome::Addrs(..) | Outcome::NoData => "no address",
                Outcome::NxDomain => "does not exist",
                Outcome::Rcode(2) => "server failure",
                Outcome::Rcode(5) => "refused",
                Outcome::Rcode(_) => "error from the server",
                Outcome::Failed(f) => f.name(),
            };
            crate::kprintln!("[npk] dns: {}: {} ({} ms)", name, why, (now() - t0) / 1000);
            None
        }
    }
}
