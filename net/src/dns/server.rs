//! One upstream server and what it is known to handle, after systemd v262
//! `resolved-dns-server.c`: a query goes out at the best feature level the
//! server may support; lost or unparsable replies, a missing OPT RR or
//! error codes that go away one level lower downgrade it, and after a
//! grace period of 5 min, doubling to 6 h, the full set is tried again.
//! Without DNSSEC and DNS-over-TLS the levels are TCP, UDP and EDNS(0).

use super::*;

const GRACE_PERIOD_MIN_USEC: u64 = 5 * 60 * USEC_PER_SEC;
const GRACE_PERIOD_MAX_USEC: u64 = 6 * 3600 * USEC_PER_SEC;
/// `DNS_SERVER_FEATURE_RETRY_ATTEMPTS`.
const RETRY_ATTEMPTS: u32 = 3;
/// `udp_header_size(AF_INET)`.
const UDP_IP_HEADER: usize = 28;

/// `DnsServerFeatureLevel`, ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Tcp,
    Udp,
    Edns0,
}

impl Level {
    pub const BEST: Level = Level::Edns0;

    pub fn name(self) -> &'static str {
        match self { Level::Tcp => "TCP", Level::Udp => "UDP", Level::Edns0 => "UDP+EDNS0" }
    }

    fn down(self) -> Level {
        match self { Level::Edns0 => Level::Udp, _ => Level::Tcp }
    }

    fn is_udp(self) -> bool { self >= Level::Udp }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    Udp,
    Tcp,
}

pub struct Server {
    pub addr: [u8; 4],
    possible: Level,
    verified: Option<Level>,
    verified_usec: u64,
    grace_usec: u64,
    n_failed_udp: u32,
    n_failed_tcp: u32,
    packet_truncated: bool,
    packet_invalid: bool,
    packet_bad_opt: bool,
    received_udp_fragment_max: usize,
    packet_fragmented: bool,
}

impl Server {
    /// `dns_server_new`.
    pub fn new(addr: [u8; 4]) -> Self {
        Server {
            addr,
            possible: Level::BEST,
            verified: None,
            verified_usec: 0,
            grace_usec: GRACE_PERIOD_MIN_USEC,
            n_failed_udp: 0,
            n_failed_tcp: 0,
            packet_truncated: false,
            packet_invalid: false,
            packet_bad_opt: false,
            received_udp_fragment_max: packet::UNICAST_SIZE_MAX,
            packet_fragmented: false,
        }
    }

    pub fn level(&self) -> Level { self.possible }

    fn set_verified(&mut self, level: Level, now: u64) {
        if self.verified.is_some_and(|v| v > level) {
            return;
        }
        self.verified = Some(level);
        self.verified_usec = now;
    }

    fn reset_counters(&mut self) {
        self.n_failed_udp = 0;
        self.n_failed_tcp = 0;
        self.packet_truncated = false;
        self.packet_invalid = false;
        self.verified_usec = 0;
    }

    /// `dns_server_packet_received`.
    pub fn packet_received(&mut self, proto: Proto, mut level: Level, size: usize, now: u64) {
        match proto {
            Proto::Udp => {
                if self.possible == level {
                    self.n_failed_udp = 0;
                }
            }
            Proto::Tcp => {
                if self.possible == level {
                    self.n_failed_tcp = 0;
                }
                // A TCP answer verifies only the TCP level.
                level = Level::Tcp;
            }
        }
        if self.packet_bad_opt && level >= Level::Edns0 {
            level = Level::Udp;
        }
        self.set_verified(level, now);
        if proto == Proto::Udp && self.received_udp_fragment_max < size {
            self.received_udp_fragment_max = size;
        }
    }

    /// `dns_server_packet_lost`.
    pub fn packet_lost(&mut self, proto: Proto, level: Level) {
        if self.possible != level {
            return;
        }
        match proto {
            Proto::Udp => self.n_failed_udp += 1,
            Proto::Tcp => self.n_failed_tcp += 1,
        }
    }

    /// `dns_server_packet_truncated`.
    pub fn packet_truncated(&mut self, level: Level) {
        if self.possible == level {
            self.packet_truncated = true;
        }
    }

    /// `dns_server_packet_bad_opt`.
    pub fn packet_bad_opt(&mut self, level: Level) {
        if level < Level::Edns0 {
            return;
        }
        if self.verified.is_some_and(|v| v >= Level::Edns0) {
            self.verified = Some(Level::Udp);
        }
        self.packet_bad_opt = true;
    }

    /// `dns_server_packet_rcode_downgrade`: a lower level cured an error
    /// code; the server goes there at once.
    pub fn packet_rcode_downgrade(&mut self, level: Level) {
        if self.verified.is_some_and(|v| v > level) {
            self.verified = Some(level);
        }
        if self.possible > level {
            self.possible = level;
            self.reset_counters();
        }
    }

    /// `dns_server_packet_invalid`.
    pub fn packet_invalid(&mut self, level: Level) {
        if self.possible == level {
            self.packet_invalid = true;
        }
    }

    fn grace_period_expired(&mut self, now: u64) -> bool {
        if self.verified_usec == 0 || self.verified_usec + self.grace_usec > now {
            return false;
        }
        self.grace_usec = (self.grace_usec * 2).min(GRACE_PERIOD_MAX_USEC);
        true
    }

    /// `dns_server_possible_feature_level`. The flag says the grace
    /// period ended, which flushes the cache.
    pub fn possible_feature_level(&mut self, now: u64) -> (Level, bool) {
        let best = Level::BEST;
        if self.possible > best {
            self.possible = best;
        }
        if self.possible < best && self.grace_period_expired(now) {
            self.possible = best;
            self.reset_counters();
            self.packet_bad_opt = false;
            return (self.possible, true);
        }
        if self.verified.is_some_and(|v| self.possible <= v) {
            self.possible = self.verified.unwrap();
            return (self.possible, false);
        }
        let p = self.possible;
        if self.n_failed_tcp >= RETRY_ATTEMPTS && self.possible == Level::Tcp {
            // TCP failed a few times at the lowest level: UDP again.
            self.possible = Level::Udp;
        } else if self.packet_invalid && self.possible > Level::Udp {
            self.possible = Level::Udp;
        } else if self.packet_bad_opt && self.possible >= Level::Edns0 {
            self.possible = Level::Udp;
        } else if self.n_failed_udp >= RETRY_ATTEMPTS && self.possible.is_udp() {
            // Lost too many datagrams in a row: maybe it cannot parse them.
            self.possible = self.possible.down();
        } else if self.n_failed_tcp >= RETRY_ATTEMPTS && self.packet_truncated
            && self.possible > Level::Udp
        {
            // TCP is no fallback here; a smaller UDP reply might still fit.
            self.possible = Level::Udp;
        }
        if p != self.possible {
            self.reset_counters();
        }
        (self.possible, false)
    }

    /// `dns_server_adjust_opt`: the EDNS(0) payload size to announce at
    /// `level`, none below EDNS(0). `mtu` is the path MTU to the server.
    pub fn opt_udp_size(&self, level: Level, mtu: usize) -> Option<u16> {
        if level < Level::Edns0 {
            return None;
        }
        let mut size = mtu.saturating_sub(UDP_IP_HEADER);
        if self.packet_fragmented {
            size = size.min(self.received_udp_fragment_max);
        }
        size = size.min(4096);
        Some(size.clamp(packet::UNICAST_SIZE_MAX, 65535) as u16)
    }
}
