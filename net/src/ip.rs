//! IPv4 address checks on the way in and out.
//!
//! Receive: `ip_rcv_core` (the frame's link-layer type), `ip_rcv_options`
//! with `__ip_options_compile` (validation only: we neither forward nor
//! echo options), and the martian checks of `ip_route_input_slow` and
//! `fib_validate_source`. Send: `__mkroute_output` and `tcp_v4_connect`.
//!
//! Two choices differ from Linux's defaults. A unicast datagram that came
//! in a link-layer broadcast or multicast frame is dropped (RFC 1122 §3.3.6
//! SHOULD; Linux's `drop_unicast_in_l2_multicast`), since on WLAN any
//! station of the network can send such a frame. And 240/4 is no
//! destination (RFC 6890).

/// Our address and mask; `ip` is 0.0.0.0 while unconfigured.
#[derive(Clone, Copy)]
pub struct Iface {
    pub ip: [u8; 4],
    pub mask: [u8; 4],
}

/// The link-layer destination of a received frame (`skb->pkt_type`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PktType {
    Host,
    Broadcast,
    Multicast,
    OtherHost,
}

/// Where a received datagram is delivered (the route's type).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dest {
    /// To our address (`RTN_LOCAL`).
    Local,
    /// To a broadcast address of our network (`RTN_BROADCAST`).
    Broadcast,
}

/// Why a datagram was dropped, by Linux's `SKB_DROP_REASON_*`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum Drop {
    /// `OTHERHOST`: the frame was for another station.
    OtherHost,
    /// `UNICAST_IN_L2_MULTICAST`.
    UnicastInL2Multicast,
    /// `IP_INHDR`: malformed options.
    BadOptions,
    /// A source route, refused as with `accept_source_route` 0.
    SourceRoute,
    /// `IP_INVALID_SOURCE`, `IP_LOCALNET` for a source.
    MartianSource,
    /// `IP_INVALID_DEST`, `IP_LOCALNET` for a destination.
    MartianDest,
    /// Not addressed to us, and we do not forward (`IP_NOT_FOR_US`).
    NotForUs,
}

impl Drop {
    pub const COUNT: usize = 7;
    pub const ALL: [Drop; Drop::COUNT] = [
        Drop::OtherHost, Drop::UnicastInL2Multicast, Drop::BadOptions, Drop::SourceRoute,
        Drop::MartianSource, Drop::MartianDest, Drop::NotForUs,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Drop::OtherHost => "other host",
            Drop::UnicastInL2Multicast => "unicast in L2 broadcast",
            Drop::BadOptions => "bad options",
            Drop::SourceRoute => "source route",
            Drop::MartianSource => "martian source",
            Drop::MartianDest => "martian destination",
            Drop::NotForUs => "not for us",
        }
    }
}

fn u32_of(a: [u8; 4]) -> u32 { u32::from_be_bytes(a) }

pub fn is_zeronet(a: [u8; 4]) -> bool { a[0] == 0 }
pub fn is_loopback(a: [u8; 4]) -> bool { a[0] == 127 }
pub fn is_multicast(a: [u8; 4]) -> bool { a[0] & 0xf0 == 0xe0 }
pub fn is_lbcast(a: [u8; 4]) -> bool { a == [255; 4] }
/// 240/4 without the limited broadcast.
pub fn is_reserved(a: [u8; 4]) -> bool { a[0] & 0xf0 == 0xf0 && !is_lbcast(a) }

impl Iface {
    fn configured(&self) -> bool { self.ip != [0; 4] }

    /// A broadcast address of our network: the subnet broadcast, or the
    /// network address itself, as Linux's `fib_add_ifaddr` adds both as
    /// `RTN_BROADCAST` for prefixes shorter than 31.
    pub fn is_subnet_broadcast(&self, a: [u8; 4]) -> bool {
        if !self.configured() { return false; }
        let (ip, mask, a) = (u32_of(self.ip), u32_of(self.mask), u32_of(a));
        if mask.count_ones() >= 31 || a & mask != ip & mask { return false; }
        a | mask == u32::MAX || a & !mask == 0
    }

    /// `fib_validate_source`: our own address or one of our broadcast
    /// addresses is no source.
    fn source_ok(&self, src: [u8; 4]) -> bool {
        !(self.configured() && src == self.ip) && !self.is_subnet_broadcast(src)
    }

    /// `ip_rcv_core`, `ip_rcv_finish_core` and `ip_route_input_slow` for a
    /// datagram from `src` to `dst`. `dhcp_reply` says it is UDP to port 68
    /// while a DHCP exchange runs; it is taken at the address offered, ours
    /// or not yet: our DHCP client reads its unicast replies through IP,
    /// where Linux's reads them from a packet socket.
    pub fn input_route(&self, l2: PktType, src: [u8; 4], dst: [u8; 4], dhcp_reply: bool)
        -> Result<Dest, Drop>
    {
        early(l2, src, dst)?;
        let brd = is_lbcast(dst) || (src == [0; 4] && dst == [0; 4]);
        let dest = if brd || self.is_subnet_broadcast(dst) {
            Dest::Broadcast
        } else if self.configured() && dst == self.ip {
            Dest::Local
        } else if dhcp_reply && !is_multicast(dst) && !is_reserved(dst) {
            Dest::Local
        } else {
            // No multicast group is joined, and nothing is forwarded.
            return Err(Drop::NotForUs);
        };
        if dest == Dest::Local && l2 != PktType::Host {
            return Err(Drop::UnicastInL2Multicast);
        }
        if !is_zeronet(src) && !self.source_ok(src) { return Err(Drop::MartianSource); }
        Ok(dest)
    }

    /// `__mkroute_output` for a datagram to `dst`: there is no loopback
    /// device, and 0/8 and 240/4 lead nowhere. `stream`: TCP, which
    /// `tcp_v4_connect` refuses to broadcast and multicast routes.
    pub fn output_ok(&self, dst: [u8; 4], stream: bool) -> bool {
        if is_loopback(dst) || is_zeronet(dst) || is_reserved(dst) { return false; }
        if stream && (is_lbcast(dst) || is_multicast(dst) || self.is_subnet_broadcast(dst)) {
            return false;
        }
        true
    }
}

/// The part of `input_route` that does not depend on our address: the
/// frame was for us, and neither address is a martian. It holds for a
/// datagram that goes on to the microVM's masquerade, which keeps flows on
/// addresses we no longer have.
pub fn early(l2: PktType, src: [u8; 4], dst: [u8; 4]) -> Result<(), Drop> {
    if l2 == PktType::OtherHost { return Err(Drop::OtherHost); }
    if is_multicast(src) || is_lbcast(src) { return Err(Drop::MartianSource); }
    if is_lbcast(dst) || (src == [0; 4] && dst == [0; 4]) { return Ok(()); }
    // Zero addresses only to the limited broadcast.
    if is_zeronet(src) { return Err(Drop::MartianSource); }
    if is_zeronet(dst) { return Err(Drop::MartianDest); }
    if is_loopback(dst) { return Err(Drop::MartianDest); }
    if is_loopback(src) { return Err(Drop::MartianSource); }
    Ok(())
}

const IPOPT_END: u8 = 0;
const IPOPT_NOOP: u8 = 1;
const IPOPT_RR: u8 = 7;
const IPOPT_TIMESTAMP: u8 = 68;
const IPOPT_LSRR: u8 = 131;
const IPOPT_CIPSO: u8 = 134;
const IPOPT_SSRR: u8 = 137;
const IPOPT_RA: u8 = 148;
const IPOPT_TS_TSONLY: u8 = 0;
const IPOPT_TS_TSANDADDR: u8 = 1;
const IPOPT_TS_PRESPEC: u8 = 3;

/// `ip_rcv_options`: the options after the 20-byte header are well formed
/// (`__ip_options_compile` for a received packet), and none is a source
/// route. A malformed list names the offending byte (`pp_ptr`) as an
/// offset into the options, for the Parameter Problem.
pub fn options_ok(opts: &[u8]) -> Result<(), (Drop, usize)> {
    let bad = |at: usize| Err((Drop::BadOptions, at));
    let (mut srr, mut rr, mut ts, mut cipso) = (false, false, false, false);
    let mut i = 0;
    while i < opts.len() {
        let l = opts.len() - i;
        match opts[i] {
            IPOPT_END => break,
            IPOPT_NOOP => { i += 1; continue; }
            _ => {}
        }
        if l < 2 { return bad(i); }
        let optlen = opts[i + 1] as usize;
        if optlen < 2 || optlen > l { return bad(i); }
        let o = &opts[i..i + optlen];
        match o[0] {
            IPOPT_SSRR | IPOPT_LSRR => {
                if optlen < 3 { return bad(i + 1); }
                if o[2] < 4 { return bad(i + 2); }
                if srr { return bad(i); }
                srr = true;
            }
            IPOPT_RR => {
                if rr { return bad(i); }
                if optlen < 3 { return bad(i + 1); }
                if o[2] < 4 { return bad(i + 2); }
                if o[2] as usize <= optlen && o[2] as usize + 3 > optlen { return bad(i + 2); }
                rr = true;
            }
            IPOPT_TIMESTAMP => {
                if ts { return bad(i); }
                if optlen < 4 { return bad(i + 1); }
                if o[2] < 5 { return bad(i + 2); }
                let ptr = o[2] as usize;
                if ptr <= optlen {
                    if ptr + 3 > optlen { return bad(i + 2); }
                    match o[3] & 0xf {
                        IPOPT_TS_TSONLY => {}
                        IPOPT_TS_TSANDADDR | IPOPT_TS_PRESPEC => {
                            if ptr + 7 > optlen { return bad(i + 2); }
                        }
                        _ => {}
                    }
                } else if o[3] & 0xf != IPOPT_TS_PRESPEC && o[3] >> 4 == 15 {
                    return bad(i + 3);
                }
                ts = true;
            }
            IPOPT_RA => {
                if optlen < 4 { return bad(i + 1); }
            }
            IPOPT_CIPSO => {
                // `cipso_v4_validate` without NetLabel: a DOI and tags that
                // fit the option.
                if cipso { return bad(i); }
                if optlen < 8 { return bad(i + 1); }
                if o[2..6] == [0; 4] { return bad(i + 2); }
                let mut j = 6;
                while j < optlen {
                    if j + 1 == optlen { return bad(i + j); }
                    let tag_len = o[j + 1] as usize;
                    if tag_len == 0 || tag_len > optlen - j { return bad(i + j + 1); }
                    j += tag_len;
                }
                cipso = true;
            }
            _ => {}
        }
        i += optlen;
    }
    if srr { Err((Drop::SourceRoute, 0)) } else { Ok(()) }
}

// ── Identification (`ip_select_ident_segs`) ──────────────────────────

/// `ip_idents`/`ip_tstamps`: per-bucket ID counters for datagrams that may
/// be fragmented, the bucket chosen by a keyed hash of (dst, src, protocol)
/// (`__ip_select_ident`). A bucket idle for a while jumps ahead by a random
/// amount, so its counter says nothing about how much was sent.
pub struct Idents {
    key: [u8; 32],
    ids: alloc::vec::Vec<u32>,
    stamps: alloc::vec::Vec<u32>,
}

impl Idents {
    /// `alloc_large_system_hash("IP idents", .., scale 16, .., 2048,
    /// 256 * 1024)`, the counters seeded at random.
    pub fn new(key: [u8; 32], ram_bytes: u64, mut rnd: impl FnMut() -> u32) -> Self {
        let n = ((ram_bytes >> 16) as usize).next_power_of_two().clamp(2048, 256 * 1024);
        Idents { key, ids: (0..n).map(|_| rnd()).collect(), stamps: alloc::vec![0; n] }
    }

    /// `ip_idents_reserve` for one datagram; `now` in ticks.
    pub fn select(&mut self, dst: [u8; 4], src: [u8; 4], proto: u8, now: u32, rnd: impl FnOnce(u32) -> u32) -> u16 {
        let mut msg = [0u8; 9];
        msg[0..4].copy_from_slice(&dst);
        msg[4..8].copy_from_slice(&src);
        msg[8] = proto;
        let h = blake3::keyed_hash(&self.key, &msg);
        let hash = u32::from_le_bytes(h.as_bytes()[0..4].try_into().unwrap());
        let b = hash as usize & (self.ids.len() - 1);
        let old = self.stamps[b];
        let mut delta = 0;
        if old != now {
            self.stamps[b] = now;
            delta = rnd(now.wrapping_sub(old));
        }
        let id = self.ids[b].wrapping_add(delta);
        self.ids[b] = id.wrapping_add(1);
        id as u16
    }
}

// ── Path MTU (`__ip_rt_update_pmtu`, the route's nexthop exceptions) ──

/// `ip_rt_min_pmtu`: a router may not talk the path below this.
pub const MIN_PMTU: u32 = 512 + 20 + 20;
/// `ip_rt_mtu_expires`, in ticks: 10 minutes.
pub const MTU_EXPIRES: u64 = 10 * 60 * 100;
/// Exceptions kept at most: `FNHE_HASH_SIZE` buckets of
/// `FNHE_RECLAIM_DEPTH`.
const PMTU_MAX: usize = 2048 * 5;

/// Path MTUs learned from Fragmentation Needed, per destination.
#[derive(Default)]
pub struct Pmtu {
    map: alloc::collections::BTreeMap<[u8; 4], (u32, u64)>,
}

impl Pmtu {
    pub const fn new() -> Self { Pmtu { map: alloc::collections::BTreeMap::new() } }

    /// The MTU to `dst`, `dev_mtu` without a live exception.
    pub fn get(&self, dst: [u8; 4], dev_mtu: u32, now: u64) -> u32 {
        match self.map.get(&dst) {
            Some(&(mtu, expires)) if now < expires => mtu.min(dev_mtu),
            _ => dev_mtu,
        }
    }

    /// A router said the path to `dst` carries only `mtu`. Only lowers, and
    /// never below `MIN_PMTU`.
    pub fn update(&mut self, dst: [u8; 4], mtu: u32, dev_mtu: u32, now: u64) {
        let old = self.get(dst, dev_mtu, now);
        if old < mtu { return; }
        let mtu = if mtu < MIN_PMTU { old.min(MIN_PMTU) } else { mtu };
        if self.map.len() >= PMTU_MAX && !self.map.contains_key(&dst) {
            if let Some(oldest) = self.map.iter().min_by_key(|(_, v)| v.1).map(|(k, _)| *k) {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(dst, (mtu, now + MTU_EXPIRES));
    }

    pub fn flush(&mut self) { self.map.clear(); }
}

// ── ICMP errors (`icmp_unreach`) ──────────────────────────────────────

pub const ICMP_DEST_UNREACH: u8 = 3;
pub const ICMP_TIME_EXCEEDED: u8 = 11;
pub const ICMP_PARAMETERPROB: u8 = 12;
pub const ICMP_FRAG_NEEDED: u8 = 4;
const ICMP_EXC_FRAGTIME: u8 = 1;
const NR_ICMP_UNREACH: u8 = 15;

/// An ICMP error about a datagram of ours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IcmpError<'a> {
    pub kind: u8,
    pub code: u8,
    /// The next-hop MTU of Fragmentation Needed, the pointer of a Parameter
    /// Problem, 0 otherwise.
    pub info: u32,
    /// The datagram's header and at least the first 8 bytes after it.
    pub inner: &'a [u8],
}

impl IcmpError<'_> {
    pub fn inner_dst(&self) -> [u8; 4] { self.inner[16..20].try_into().unwrap() }
    pub fn inner_proto(&self) -> u8 { self.inner[9] }
    /// The transport header of the datagram, its first 8 bytes at least.
    pub fn transport(&self) -> &[u8] { &self.inner[((self.inner[0] & 0x0f) as usize) * 4..] }
}

/// `icmp_unreach` for an ICMP message `icmp` that says something about a
/// datagram `our_ip` sent: Destination Unreachable, Time Exceeded (not the
/// reassembly timer's) or Parameter Problem, quoting a whole IPv4 header and
/// 8 bytes of its payload.
pub fn icmp_error(icmp: &[u8], our_ip: [u8; 4]) -> Option<IcmpError<'_>> {
    if icmp.len() < 8 { return None; }
    let (kind, code) = (icmp[0], icmp[1]);
    let info = match kind {
        ICMP_DEST_UNREACH => {
            if code > NR_ICMP_UNREACH { return None; }
            if code == ICMP_FRAG_NEEDED { u16::from_be_bytes([icmp[6], icmp[7]]) as u32 } else { 0 }
        }
        ICMP_PARAMETERPROB => icmp[4] as u32,
        ICMP_TIME_EXCEEDED if code == ICMP_EXC_FRAGTIME => return None,
        ICMP_TIME_EXCEEDED => 0,
        _ => return None,
    };
    let inner = &icmp[8..];
    if inner.len() < 20 || inner[0] >> 4 != 4 { return None; }
    let ihl = ((inner[0] & 0x0f) as usize) * 4;
    if ihl < 20 || inner.len() < ihl + 8 { return None; }
    if inner[12..16] != our_ip { return None; }
    Some(IcmpError { kind, code, info, inner })
}

// ── Sending ICMP errors (`__icmp_send`) ───────────────────────────────

pub const ICMP_PORT_UNREACH: u8 = 3;
pub const ICMP_PROT_UNREACH: u8 = 2;
/// `IPTOS_PREC_INTERNETCONTROL`.
pub const TOS_INTERNETCONTROL: u8 = 0xc0;

/// `icmp_pointers[type].error`; an unknown type counts as an error.
fn is_error_type(t: u8) -> bool {
    !matches!(t, 0 | 8 | 13 | 14 | 15 | 16 | 17 | 18)
}

/// The ICMP message reporting `kind`/`code` about the received datagram
/// `orig` (a whole IPv4 datagram to us), or `None` where `__icmp_send` says
/// nothing: a non-initial fragment, or an ICMP error itself. It quotes as
/// much of `orig` as a 576-byte datagram holds. The caller has made sure
/// the datagram came to our address in a unicast frame.
pub fn icmp_error_message(orig: &[u8], kind: u8, code: u8, info: u32) -> Option<alloc::vec::Vec<u8>> {
    if orig.len() < 20 { return None; }
    let ihl = ((orig[0] & 0x0f) as usize) * 4;
    if u16::from_be_bytes([orig[6], orig[7]]) & 0x1fff != 0 { return None; }
    if orig[9] == 1 && (orig.len() <= ihl || is_error_type(orig[ihl])) { return None; }
    let room = 576 - 20 - 8;
    let quoted = &orig[..orig.len().min(room)];
    let mut m = alloc::vec![kind, code, 0, 0];
    m.extend_from_slice(&info.to_be_bytes());
    m.extend_from_slice(quoted);
    let mut sum = 0u32;
    for c in m.chunks(2) {
        sum += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
    }
    while sum >> 16 != 0 { sum = (sum & 0xffff) + (sum >> 16); }
    m[2..4].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    Some(m)
}

const HZ: u64 = 100;
/// `sysctl_icmp_msgs_per_sec`, `sysctl_icmp_msgs_burst`.
const MSGS_PER_SEC: i64 = 1000;
const MSGS_BURST: i64 = 50;
/// `sysctl_icmp_ratelimit` (1 s) and `XRLIM_BURST_FACTOR`.
const RATELIMIT: u64 = HZ;
const XRLIM_BURST_FACTOR: u64 = 6;
/// `sysctl_icmp_ratemask`: Destination Unreachable, Source Quench, Time
/// Exceeded, Parameter Problem.
const RATEMASK: u32 = 0x1818;
/// `inet_peer_threshold`, and the idle time after which a peer may go.
const PEERS_MAX: usize = 65536 + 128;
const PEER_MINTTL: u64 = 120 * HZ;

/// The two limits on ICMP errors: one for all (`icmp_global_allow`), one
/// per destination (`icmpv4_xrlim_allow` on its `inet_peer`).
pub struct IcmpLimit {
    credit: i64,
    stamp: u64,
    /// Per destination: tokens and when they were last topped up.
    peers: alloc::collections::BTreeMap<[u8; 4], (u64, u64)>,
}

impl IcmpLimit {
    pub const fn new() -> Self {
        IcmpLimit { credit: 0, stamp: 0, peers: alloc::collections::BTreeMap::new() }
    }

    /// Whether an ICMP error of `kind`/`code` to `dst` may go out now.
    /// `rnd3` draws from 0..3, the credits a message costs.
    pub fn allow(&mut self, dst: [u8; 4], kind: u8, code: u8, now: u64, rnd3: impl FnOnce() -> i64) -> bool {
        // icmpv4_mask_allow: Fragmentation Needed and types outside the
        // mask are never limited.
        if kind > 18 || RATEMASK & (1 << kind) == 0 || (kind == ICMP_DEST_UNREACH && code == ICMP_FRAG_NEEDED) {
            return true;
        }
        // icmp_global_allow
        if self.credit <= 0 {
            let delta = now.saturating_sub(self.stamp).min(HZ);
            if delta < HZ / 50 { return false; }
            let incr = MSGS_PER_SEC * delta as i64 / HZ as i64;
            if incr == 0 { return false; }
            self.stamp = now;
            self.credit = (self.credit + incr).min(MSGS_BURST);
        }
        // inet_peer_xrlim_allow
        if self.peers.len() >= PEERS_MAX && !self.peers.contains_key(&dst) {
            self.peers.retain(|_, p| now.saturating_sub(p.1) < PEER_MINTTL);
        }
        let ok = if self.peers.len() >= PEERS_MAX && !self.peers.contains_key(&dst) {
            true
        } else {
            let p = self.peers.entry(dst).or_insert((XRLIM_BURST_FACTOR * RATELIMIT, now));
            let delta = now.saturating_sub(p.1);
            if delta > 0 {
                p.1 = now;
                p.0 = (p.0 + delta).min(XRLIM_BURST_FACTOR * RATELIMIT);
            }
            if p.0 >= RATELIMIT { p.0 -= RATELIMIT; true } else { false }
        };
        // icmp_global_consume
        if ok { self.credit -= rnd3(); }
        ok
    }
}
