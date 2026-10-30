//! The stub resolver, after systemd v262 `resolved`: a query follows
//! CNAMEs (`resolved-dns-query.c`, at most 16), each step is a transaction
//! for one (name, type) shared by every query that asks it
//! (`resolved-dns-transaction.c`): answered from the cache when it can,
//! else sent to the current server over UDP at that server's feature
//! level, every 5 s, moving on to the next server on each timeout and
//! giving up after 24 attempts; a truncated reply is asked again over TCP,
//! error codes are retried one feature level lower or on another server.
//! The caller moves the bytes: it sends what `Out` holds and hands back
//! replies, ICMP errors and TCP results, and calls `tick` by
//! `next_deadline`. Time is microseconds.
//!
//! Not ported: DNSSEC, DNS-over-TLS, LLMNR, mDNS, search domains,
//! /etc/hosts, stale answers, fallback servers, fragment tracking.

pub mod cache;
pub mod packet;
pub mod server;

use alloc::string::String;
use alloc::vec::Vec;
use cache::Cache;
use packet::{RData, Rr};
use server::{Level, Proto, Server};

pub const USEC_PER_SEC: u64 = 1_000_000;

pub const TYPE_A: u16 = 1;
pub const TYPE_CNAME: u16 = 5;
pub const TYPE_SOA: u16 = 6;
pub const TYPE_OPT: u16 = 41;
pub const TYPE_ANY: u16 = 255;
pub const CLASS_IN: u16 = 1;

pub const RCODE_SUCCESS: u16 = 0;
pub const RCODE_FORMERR: u16 = 1;
pub const RCODE_SERVFAIL: u16 = 2;
pub const RCODE_NXDOMAIN: u16 = 3;
pub const RCODE_NOTIMP: u16 = 4;
pub const RCODE_REFUSED: u16 = 5;

/// `DNS_HOSTNAME_MAX`: a name in text, without the trailing dot.
pub const NAME_MAX: usize = 253;
/// `DNS_LABEL_MAX`.
const LABEL_MAX: usize = 63;
/// `DNS_TRANSACTION_ATTEMPTS_MAX`.
const ATTEMPTS_MAX: u32 = 24;
/// `TRANSACTION_UDP_TIMEOUT_USEC`: the 120 s query budget over 24 attempts.
const UDP_TIMEOUT_USEC: u64 = 120 * USEC_PER_SEC / ATTEMPTS_MAX as u64;
/// `TRANSACTION_TCP_TIMEOUT_USEC`.
const TCP_TIMEOUT_USEC: u64 = 10 * USEC_PER_SEC;
/// `SD_RESOLVED_QUERY_TIMEOUT_USEC`: a query ends after this, whatever
/// its transactions are doing.
const QUERY_TIMEOUT_USEC: u64 = 120 * USEC_PER_SEC;
/// `CNAME_REDIRECTS_MAX`.
const CNAME_REDIRECTS_MAX: u32 = 16;

/// A lookup key, the name lower case without the trailing dot. Class is
/// always IN.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub name: String,
    pub rtype: u16,
}

/// `dns_name_normalize` for a host name: one trailing dot dropped, lower
/// case, every label 1 to 63 bytes, at most 253 in all (RFC 1035 §2.3.4).
pub fn normalize(name: &str) -> Option<String> {
    let n = name.strip_suffix('.').unwrap_or(name);
    if n.is_empty() || n.len() > NAME_MAX {
        return None;
    }
    for label in n.split('.') {
        if label.is_empty() || label.len() > LABEL_MAX {
            return None;
        }
        if !label.bytes().all(|c| c.is_ascii_graphic()) {
            return None;
        }
    }
    Some(n.to_ascii_lowercase())
}

/// `dns_name_endswith`: `name` is `suffix` or below it.
pub fn name_endswith(name: &str, suffix: &str) -> bool {
    suffix.is_empty() || name == suffix
        || (name.len() > suffix.len() && name.ends_with(suffix)
            && name.as_bytes()[name.len() - suffix.len() - 1] == b'.')
}

/// Why a transaction ended without an answer (`DnsTransactionState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// `ATTEMPTS_MAX_REACHED`.
    Timeout,
    NoServers,
    NetworkDown,
    InvalidReply,
    /// An ICMP error and no other server.
    Unreachable,
    /// More than 16 CNAMEs, or one to itself.
    CnameLoop,
}

impl Failure {
    pub fn name(self) -> &'static str {
        match self {
            Failure::Timeout => "no answer",
            Failure::NoServers => "no DNS server",
            Failure::NetworkDown => "network down",
            Failure::InvalidReply => "invalid reply",
            Failure::Unreachable => "server unreachable",
            Failure::CnameLoop => "CNAME loop",
        }
    }
}

/// What a query found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The addresses, and the lowest TTL along the CNAME chain in seconds.
    Addrs(Vec<[u8; 4]>, u32),
    NxDomain,
    /// The name exists without a record of the type.
    NoData,
    /// An error code from the server (SERVFAIL, REFUSED, ...).
    Rcode(u16),
    Failed(Failure),
}

/// Bytes for the caller to move; `tx` names the transaction's socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Send {
    /// A datagram to port 53, from the transaction's own port (a new one
    /// whenever the server changes).
    Udp { tx: u32, server: [u8; 4], bytes: Vec<u8> },
    /// A query over TCP to port 53: connect, send it with its 2-byte
    /// length, read one reply, then `tcp_reply`.
    Tcp { tx: u32, server: [u8; 4], bytes: Vec<u8> },
    /// The transaction's socket is no longer needed.
    Close { tx: u32 },
}

#[derive(Default)]
pub struct Out {
    pub send: Vec<Send>,
}

#[derive(Clone, Debug, PartialEq)]
enum TxState {
    Pending,
    Success { rcode: u16, answer: Vec<Rr> },
    RcodeFailure { rcode: u16 },
    Failed(Failure),
}

/// `DnsTransaction`.
struct Tx {
    handle: u32,
    id: u16,
    key: Key,
    state: TxState,
    server: Option<[u8; 4]>,
    level: Level,
    clamp_servfail: Option<Level>,
    n_attempts: u32,
    n_picked_servers: usize,
    seen_timeout: bool,
    /// The transport of the attempt in flight.
    proto: Proto,
    timeout_at: Option<u64>,
    /// A UDP socket exists for `server`.
    udp_open: bool,
}

/// `DnsQuery`.
struct Query {
    handle: u64,
    key: Key,
    n_cname_redirects: u32,
    /// The lowest TTL of the CNAMEs followed so far.
    chain_ttl: u32,
    tx: Option<u32>,
    outcome: Option<Outcome>,
    /// `on_query_timeout`.
    deadline: u64,
}

pub struct Resolver {
    servers: Vec<Server>,
    current: Option<usize>,
    pub cache: Cache,
    txs: Vec<Tx>,
    queries: Vec<Query>,
    next_tx: u32,
    next_query: u64,
    /// `manager_routable`: we have an address and a route.
    pub routable: bool,
    /// The path MTU to the servers, for the EDNS(0) payload size.
    pub mtu: usize,
}

impl Default for Resolver {
    fn default() -> Self { Self::new() }
}

impl Resolver {
    pub fn new() -> Self {
        Resolver {
            servers: Vec::new(),
            current: None,
            cache: Cache::new(),
            txs: Vec::new(),
            queries: Vec::new(),
            next_tx: 1,
            next_query: 1,
            routable: false,
            mtu: 1500,
        }
    }

    /// The configured servers, in order; known ones keep what was learnt
    /// about them. A new list starts at its first server and empties the
    /// cache when that is a different one.
    pub fn set_servers(&mut self, addrs: &[[u8; 4]]) {
        let old_current = self.current_server();
        let mut old = core::mem::take(&mut self.servers);
        for &a in addrs {
            if self.servers.iter().any(|s| s.addr == a) {
                continue;
            }
            let s = match old.iter().position(|s| s.addr == a) {
                Some(i) => old.swap_remove(i),
                None => Server::new(a),
            };
            self.servers.push(s);
        }
        self.current = if self.servers.is_empty() { None } else { Some(0) };
        if self.current_server() != old_current {
            self.cache.flush();
        }
    }

    pub fn servers(&self) -> Vec<([u8; 4], &'static str)> {
        self.servers.iter().map(|s| (s.addr, s.level().name())).collect()
    }

    /// The server queries go to now.
    pub fn current_server(&self) -> Option<[u8; 4]> {
        self.current.map(|i| self.servers[i].addr)
    }

    fn server_mut(&mut self, addr: [u8; 4]) -> Option<&mut Server> {
        self.servers.iter_mut().find(|s| s.addr == addr)
    }

    /// `manager_next_dns_server`: on from `if_current`, unless that has
    /// changed already. A switch empties the cache.
    fn next_server(&mut self, if_current: Option<[u8; 4]>) {
        let Some(i) = self.current else { return };
        if if_current.is_some_and(|a| self.servers[i].addr != a) {
            return;
        }
        let n = (i + 1) % self.servers.len();
        if n != i {
            self.current = Some(n);
            self.cache.flush();
        }
    }

    /// Ask for the A records of `name`. `None` for a name that is not
    /// one (`normalize`).
    pub fn query(&mut self, name: &str, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) -> Option<u64> {
        let name = normalize(name)?;
        let handle = self.next_query;
        self.next_query += 1;
        self.queries.push(Query {
            handle,
            key: Key { name, rtype: TYPE_A },
            n_cname_redirects: 0,
            chain_ttl: u32::MAX,
            tx: None,
            outcome: None,
            deadline: now + QUERY_TIMEOUT_USEC,
        });
        self.query_go(handle, now, rnd, out);
        Some(handle)
    }

    /// The outcome of a query, once; the query is then gone.
    pub fn take(&mut self, q: u64) -> Option<Outcome> {
        let i = self.queries.iter().position(|x| x.handle == q)?;
        self.queries[i].outcome.as_ref()?;
        let o = self.queries.swap_remove(i).outcome;
        o
    }

    /// Give up on a query.
    pub fn cancel(&mut self, q: u64, out: &mut Out) {
        self.queries.retain(|x| x.handle != q);
        self.gc(out);
    }

    /// The cache's answer for `name`, without the network; `None` when it
    /// has none. CNAMEs are followed within the cache.
    pub fn lookup_cached(&self, name: &str, now: u64) -> Option<Outcome> {
        let mut key = Key { name: normalize(name)?, rtype: TYPE_A };
        let mut ttl = u32::MAX;
        for _ in 0..=CNAME_REDIRECTS_MAX {
            let hit = self.cache.lookup(&key, now)?;
            if hit.rcode != RCODE_SUCCESS && hit.rcode != RCODE_NXDOMAIN {
                return Some(Outcome::Rcode(hit.rcode));
            }
            match follow(&key, &hit.answer, hit.rcode, &mut ttl) {
                Step::Done(o) => return Some(o),
                Step::Redirect(k) => key = k,
            }
        }
        Some(Outcome::Failed(Failure::CnameLoop))
    }

    pub fn flush(&mut self) {
        self.cache.flush();
    }

    /// The earliest timeout, absolute.
    pub fn next_deadline(&self) -> Option<u64> {
        let q = self.queries.iter().filter(|q| q.outcome.is_none()).map(|q| q.deadline);
        self.txs.iter().filter_map(|t| t.timeout_at).chain(q).min()
    }

    fn tx_index(&self, handle: u32) -> Option<usize> {
        self.txs.iter().position(|t| t.handle == handle)
    }

    /// Start or join the transaction for the query's key
    /// (`dns_scope_find_transaction`).
    fn query_go(&mut self, q: u64, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let Some(qi) = self.queries.iter().position(|x| x.handle == q) else { return };
        let key = self.queries[qi].key.clone();
        if let Some(t) = self.txs.iter().find(|t| t.key == key && t.state == TxState::Pending) {
            self.queries[qi].tx = Some(t.handle);
            return;
        }
        let handle = self.next_tx;
        self.next_tx = self.next_tx.wrapping_add(1).max(1);
        let id = self.pick_new_id(rnd);
        self.txs.push(Tx {
            handle,
            id,
            key,
            state: TxState::Pending,
            server: None,
            level: Level::BEST,
            clamp_servfail: None,
            n_attempts: 0,
            n_picked_servers: 0,
            seen_timeout: false,
            proto: Proto::Udp,
            timeout_at: None,
            udp_open: false,
        });
        self.queries[qi].tx = Some(handle);
        self.tx_go(handle, now, rnd, out);
    }

    /// `pick_new_id`: random, and unused by the other transactions.
    fn pick_new_id(&self, rnd: &mut impl FnMut() -> u64) -> u16 {
        loop {
            let id = rnd() as u16;
            if id != 0 && !self.txs.iter().any(|t| t.id == id) {
                return id;
            }
        }
    }

    /// `dns_transaction_complete`: settle it, tell its queries, and drop
    /// it if nobody waits any more.
    fn tx_complete(&mut self, handle: u32, state: TxState, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let Some(ti) = self.tx_index(handle) else { return };
        let t = &mut self.txs[ti];
        t.state = state.clone();
        t.timeout_at = None;
        if t.udp_open {
            t.udp_open = false;
            out.send.push(Send::Close { tx: handle });
        }
        let waiting: Vec<u64> = self.queries.iter().filter(|q| q.tx == Some(handle)).map(|q| q.handle).collect();
        for q in waiting {
            self.query_ready(q, &state, now, rnd, out);
        }
        self.gc(out);
    }

    /// Drop finished transactions and those nobody waits for.
    fn gc(&mut self, out: &mut Out) {
        let queries = &self.queries;
        self.txs.retain(|t| {
            let keep = t.state == TxState::Pending && queries.iter().any(|q| q.tx == Some(t.handle));
            if !keep && t.udp_open {
                out.send.push(Send::Close { tx: t.handle });
            }
            keep
        });
    }

    /// `dns_query_ready` and `dns_query_process_cname_many`.
    fn query_ready(&mut self, q: u64, state: &TxState, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let Some(qi) = self.queries.iter().position(|x| x.handle == q) else { return };
        self.queries[qi].tx = None;
        let (rcode, answer) = match state {
            TxState::Success { rcode, answer } => (*rcode, answer),
            TxState::RcodeFailure { rcode } => {
                self.queries[qi].outcome = Some(Outcome::Rcode(*rcode));
                return;
            }
            TxState::Failed(f) => {
                self.queries[qi].outcome = Some(Outcome::Failed(*f));
                return;
            }
            TxState::Pending => return,
        };
        let mut key = self.queries[qi].key.clone();
        let mut ttl = self.queries[qi].chain_ttl;
        let mut redirected = false;
        loop {
            match follow(&key, answer, rcode, &mut ttl) {
                Step::Done(o) => {
                    // An answer for the alias that ends without the target's
                    // records: ask for the target itself.
                    if redirected && !matches!(o, Outcome::Addrs(..)) {
                        break;
                    }
                    self.queries[qi].outcome = Some(o);
                    return;
                }
                Step::Redirect(k) => {
                    let qq = &mut self.queries[qi];
                    if qq.n_cname_redirects >= CNAME_REDIRECTS_MAX || k == key {
                        qq.outcome = Some(Outcome::Failed(Failure::CnameLoop));
                        return;
                    }
                    qq.n_cname_redirects += 1;
                    key = k;
                    redirected = true;
                }
            }
        }
        let qq = &mut self.queries[qi];
        qq.key = key;
        qq.chain_ttl = ttl;
        self.query_go(q, now, rnd, out);
    }

    /// `dns_transaction_go`, after `dns_transaction_prepare`.
    fn tx_go(&mut self, handle: u32, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let Some(ti) = self.tx_index(handle) else { return };
        self.txs[ti].timeout_at = None;
        if !self.routable {
            return self.tx_complete(handle, TxState::Failed(Failure::NetworkDown), now, rnd, out);
        }
        if self.txs[ti].n_attempts >= ATTEMPTS_MAX {
            return self.tx_complete(handle, TxState::Failed(Failure::Timeout), now, rnd, out);
        }
        self.txs[ti].n_attempts += 1;

        self.cache.prune(now);
        let key = self.txs[ti].key.clone();
        if let Some(hit) = self.cache.lookup(&key, now) {
            let state = if hit.rcode == RCODE_SUCCESS || hit.rcode == RCODE_NXDOMAIN {
                TxState::Success { rcode: hit.rcode, answer: hit.answer }
            } else {
                TxState::RcodeFailure { rcode: hit.rcode }
            };
            return self.tx_complete(handle, state, now, rnd, out);
        }

        match self.emit(handle, false, now) {
            Ok(Some(send)) => {
                out.send.push(send);
                let i = self.tx_index(handle).unwrap();
                let t = &mut self.txs[i];
                let timeout = if t.proto == Proto::Tcp { TCP_TIMEOUT_USEC } else { UDP_TIMEOUT_USEC };
                t.timeout_at = Some(now + timeout);
            }
            Ok(None) => {}
            Err(f) => self.tx_complete(handle, TxState::Failed(f), now, rnd, out),
        }
    }

    /// `dns_transaction_pick_server`: the current server and its level;
    /// true when it is another server than before.
    fn pick_server(&mut self, ti: usize, now: u64) -> Result<bool, Failure> {
        let Some(si) = self.current else { return Err(Failure::NoServers) };
        let addr = self.servers[si].addr;
        let changed = self.txs[ti].server != Some(addr);
        if changed {
            self.txs[ti].clamp_servfail = None;
        }
        let (mut level, flush) = self.servers[si].possible_feature_level(now);
        if flush {
            self.cache.flush();
        }
        if let Some(c) = self.txs[ti].clamp_servfail {
            level = level.min(c);
        }
        let t = &mut self.txs[ti];
        t.level = level;
        if changed {
            t.server = Some(addr);
            t.n_picked_servers += 1;
        }
        Ok(changed)
    }

    /// `dns_transaction_emit_udp`, falling back to `dns_transaction_emit_tcp`
    /// below the UDP level or when `tcp` asks for it.
    fn emit(&mut self, handle: u32, tcp: bool, now: u64) -> Result<Option<Send>, Failure> {
        let ti = self.tx_index(handle).ok_or(Failure::InvalidReply)?;
        let changed = self.pick_server(ti, now)?;
        let mtu = self.mtu;
        let t = &self.txs[ti];
        let server = t.server.unwrap();
        let opt = self.servers[self.current.unwrap()].opt_udp_size(t.level, mtu);
        let bytes = packet::build_query(t.id, &t.key.name, t.key.rtype, opt);
        let t = &mut self.txs[ti];
        if tcp || t.level < Level::Udp {
            t.proto = Proto::Tcp;
            return Ok(Some(Send::Tcp { tx: handle, server, bytes }));
        }
        t.proto = Proto::Udp;
        if changed {
            t.udp_open = true;
        }
        Ok(Some(Send::Udp { tx: handle, server, bytes }))
    }

    /// `dns_transaction_retry`.
    fn retry(&mut self, handle: u32, next_server: bool, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        if next_server {
            let s = self.tx_index(handle).and_then(|i| self.txs[i].server);
            self.next_server(s);
        }
        self.tx_go(handle, now, rnd, out);
    }

    /// `dns_transaction_limited_retry`: another server, if one is left.
    fn limited_retry(&mut self, handle: u32, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) -> bool {
        let Some(ti) = self.tx_index(handle) else { return false };
        if self.txs[ti].n_picked_servers >= self.servers.len() {
            return false;
        }
        self.retry(handle, true, now, rnd, out);
        true
    }

    /// `dns_transaction_maybe_restart`: the server's level fell below the
    /// one this attempt used; start over under a new ID.
    fn maybe_restart(&mut self, handle: u32, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) -> bool {
        let Some(ti) = self.tx_index(handle) else { return false };
        let Some(addr) = self.txs[ti].server else { return false };
        let level = self.txs[ti].level;
        let Some(s) = self.server_mut(addr) else { return false };
        let (possible, flush) = s.possible_feature_level(now);
        if flush {
            self.cache.flush();
        }
        if level <= possible {
            return false;
        }
        let id = self.pick_new_id(rnd);
        self.txs[ti].id = id;
        self.tx_go(handle, now, rnd, out);
        true
    }

    /// Fire due timeouts (`on_query_timeout`, `on_transaction_timeout`).
    pub fn tick(&mut self, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let mut expired = false;
        for q in self.queries.iter_mut().filter(|q| q.outcome.is_none() && q.deadline <= now) {
            q.outcome = Some(Outcome::Failed(Failure::Timeout));
            q.tx = None;
            expired = true;
        }
        if expired {
            self.gc(out);
        }
        loop {
            let due = self.txs.iter()
                .filter(|t| t.timeout_at.is_some_and(|d| d <= now))
                .map(|t| t.handle)
                .next();
            let Some(h) = due else { return };
            let ti = self.tx_index(h).unwrap();
            let t = &mut self.txs[ti];
            t.timeout_at = None;
            t.seen_timeout = true;
            let (addr, proto, level) = (t.server, t.proto, t.level);
            if let Some(s) = addr.and_then(|a| self.server_mut(a)) {
                s.packet_lost(proto, level);
            }
            // Packet loss: another server, even if all were tried.
            self.retry(h, true, now, rnd, out);
        }
    }

    /// A hard ICMP error for the transaction's datagram: as a loss, and
    /// another server if one is left.
    pub fn udp_error(&mut self, tx: u32, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let Some(ti) = self.tx_index(tx) else { return };
        if self.txs[ti].state != TxState::Pending || self.txs[ti].proto != Proto::Udp {
            return;
        }
        let (addr, level) = (self.txs[ti].server, self.txs[ti].level);
        if let Some(s) = addr.and_then(|a| self.server_mut(a)) {
            s.packet_lost(Proto::Udp, level);
        }
        if self.txs[ti].udp_open {
            self.txs[ti].udp_open = false;
            out.send.push(Send::Close { tx });
        }
        if self.limited_retry(tx, now, rnd, out) {
            return;
        }
        self.tx_complete(tx, TxState::Failed(Failure::Unreachable), now, rnd, out);
    }

    /// A datagram from the transaction's server to its port.
    pub fn udp_reply(&mut self, tx: u32, b: &[u8], now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let Some(ti) = self.tx_index(tx) else { return };
        let t = &self.txs[ti];
        if t.proto != Proto::Udp || b.len() < packet::HEADER_SIZE {
            return;
        }
        // `dns_packet_validate_reply`, and the ID.
        if b[2] & 0x80 == 0 || u16::from_be_bytes([b[0], b[1]]) != t.id {
            return;
        }
        self.process_reply(tx, b, Proto::Udp, now, rnd, out);
    }

    /// The result of a `Send::Tcp`: the reply without its length prefix,
    /// or `None` when the connection failed or timed out.
    pub fn tcp_reply(&mut self, tx: u32, b: Option<&[u8]>, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let Some(ti) = self.tx_index(tx) else { return };
        if self.txs[ti].state != TxState::Pending || self.txs[ti].proto != Proto::Tcp {
            return;
        }
        match b {
            Some(b) => self.process_reply(tx, b, Proto::Tcp, now, rnd, out),
            None => {
                let (addr, level) = (self.txs[ti].server, self.txs[ti].level);
                if let Some(s) = addr.and_then(|a| self.server_mut(a)) {
                    s.packet_lost(Proto::Tcp, level);
                }
                self.retry(tx, true, now, rnd, out);
            }
        }
    }

    /// `dns_transaction_process_reply`.
    fn process_reply(&mut self, tx: u32, b: &[u8], proto: Proto, now: u64, rnd: &mut impl FnMut() -> u64, out: &mut Out) {
        let Some(ti) = self.tx_index(tx) else { return };
        if self.txs[ti].state != TxState::Pending {
            return;
        }
        let tc = b.len() >= 4 && b[2] & 0x02 != 0;
        if proto == Proto::Tcp {
            // Truncated over TCP, or another ID: somebody meddles.
            if tc || b.len() < 2 || u16::from_be_bytes([b[0], b[1]]) != self.txs[ti].id {
                return self.tx_complete(tx, TxState::Failed(Failure::InvalidReply), now, rnd, out);
            }
        }
        if tc {
            // RFC 7766 §5: a truncated answer is asked again over TCP.
            let addr_level = (self.txs[ti].server, self.txs[ti].level);
            if let (Some(a), level) = addr_level {
                if let Some(s) = self.server_mut(a) {
                    s.packet_truncated(level);
                }
            }
            match self.emit(tx, true, now) {
                Ok(Some(send)) => {
                    out.send.push(send);
                    let i = self.tx_index(tx).unwrap();
                    self.txs[i].timeout_at = Some(now + TCP_TIMEOUT_USEC);
                }
                Ok(None) => {}
                Err(f) => self.tx_complete(tx, TxState::Failed(f), now, rnd, out),
            }
            return;
        }

        let level = self.txs[ti].level;
        let addr = self.txs[ti].server;
        let Some(reply) = packet::parse(b) else {
            if let Some(s) = addr.and_then(|a| self.server_mut(a)) {
                s.packet_invalid(level);
            }
            if self.maybe_restart(tx, now, rnd, out) {
                return;
            }
            return self.tx_complete(tx, TxState::Failed(Failure::InvalidReply), now, rnd, out);
        };

        let rcode = reply.rcode;
        if matches!(rcode, RCODE_FORMERR | RCODE_SERVFAIL | RCODE_NOTIMP) {
            if level <= Level::Udp {
                // Nothing lower to try here; maybe another server knows.
                if self.limited_retry(tx, now, rnd, out) {
                    return;
                }
            } else {
                let t = &mut self.txs[ti];
                // SERVFAIL may be transient: once more at the same level first.
                t.clamp_servfail = Some(if rcode == RCODE_SERVFAIL && t.clamp_servfail.is_none() {
                    level
                } else {
                    match level { Level::Edns0 => Level::Udp, _ => Level::Tcp }
                });
                return self.retry(tx, false, now, rnd, out);
            }
        } else if rcode == RCODE_REFUSED && self.limited_retry(tx, now, rnd, out) {
            return;
        }

        let clamp = self.txs[ti].clamp_servfail;
        if let Some(s) = addr.and_then(|a| self.server_mut(a)) {
            if (rcode == RCODE_SUCCESS || rcode == RCODE_NXDOMAIN) && clamp.is_some() {
                s.packet_rcode_downgrade(clamp.unwrap());
            }
            if reply.opt.is_none() {
                s.packet_bad_opt(level);
            }
            s.packet_received(proto, level, b.len(), now);
        }
        if self.maybe_restart(tx, now, rnd, out) {
            return;
        }

        let key = self.txs[ti].key.clone();
        if !reply.is_reply_for(&key) {
            return self.tx_complete(tx, TxState::Failed(Failure::InvalidReply), now, rnd, out);
        }
        // `dns_transaction_fix_rcode`: a positive answer under NXDOMAIN
        // counts for the first name of the chain (RFC 6604 §3).
        let mut rcode = rcode;
        if rcode == RCODE_NXDOMAIN
            && reply.rrs.iter().any(|r| r.cacheable() && r.name == key.name
                && (r.rtype == key.rtype || r.rtype == TYPE_CNAME))
        {
            rcode = RCODE_SUCCESS;
        }
        // `dns_transaction_cache_answer`.
        self.cache.put(&key, rcode, &reply.rrs, now);
        let state = if rcode == RCODE_SUCCESS || rcode == RCODE_NXDOMAIN {
            TxState::Success { rcode, answer: reply.rrs }
        } else {
            TxState::RcodeFailure { rcode }
        };
        self.tx_complete(tx, state, now, rnd, out);
    }
}

enum Step {
    Done(Outcome),
    Redirect(Key),
}

/// `dns_query_process_cname_one` over an answer: the records for `key`,
/// or the CNAME to follow from it. `ttl` keeps the chain's lowest TTL.
fn follow(key: &Key, answer: &[Rr], rcode: u16, ttl: &mut u32) -> Step {
    let here: Vec<&Rr> = answer.iter()
        .filter(|r| r.section == packet::Section::Answer && r.name == key.name && r.rtype == key.rtype)
        .collect();
    if !here.is_empty() {
        let addrs = here.iter().filter_map(|r| match r.data { RData::A(a) => Some(a), _ => None }).collect();
        let t = here.iter().map(|r| r.ttl).min().unwrap_or(0).min(*ttl);
        return Step::Done(Outcome::Addrs(addrs, t));
    }
    let cname = answer.iter().find(|r| {
        r.section == packet::Section::Answer && r.name == key.name && r.rtype == TYPE_CNAME
    });
    if let Some(Rr { data: RData::Cname(target), ttl: t, .. }) = cname {
        *ttl = (*ttl).min(*t);
        return Step::Redirect(Key { name: target.clone(), rtype: key.rtype });
    }
    Step::Done(if rcode == RCODE_NXDOMAIN { Outcome::NxDomain } else { Outcome::NoData })
}
