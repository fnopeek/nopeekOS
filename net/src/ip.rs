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
/// route.
pub fn options_ok(opts: &[u8]) -> Result<(), Drop> {
    let (mut srr, mut rr, mut ts, mut cipso) = (false, false, false, false);
    let mut i = 0;
    while i < opts.len() {
        let l = opts.len() - i;
        match opts[i] {
            IPOPT_END => break,
            IPOPT_NOOP => { i += 1; continue; }
            _ => {}
        }
        if l < 2 { return Err(Drop::BadOptions); }
        let optlen = opts[i + 1] as usize;
        if optlen < 2 || optlen > l { return Err(Drop::BadOptions); }
        let o = &opts[i..i + optlen];
        match o[0] {
            IPOPT_SSRR | IPOPT_LSRR => {
                if optlen < 3 || o[2] < 4 || srr { return Err(Drop::BadOptions); }
                srr = true;
            }
            IPOPT_RR => {
                if rr || optlen < 3 || o[2] < 4 { return Err(Drop::BadOptions); }
                if o[2] as usize <= optlen && o[2] as usize + 3 > optlen { return Err(Drop::BadOptions); }
                rr = true;
            }
            IPOPT_TIMESTAMP => {
                if ts || optlen < 4 || o[2] < 5 { return Err(Drop::BadOptions); }
                let ptr = o[2] as usize;
                if ptr <= optlen {
                    if ptr + 3 > optlen { return Err(Drop::BadOptions); }
                    match o[3] & 0xf {
                        IPOPT_TS_TSONLY => {}
                        IPOPT_TS_TSANDADDR | IPOPT_TS_PRESPEC => {
                            if ptr + 7 > optlen { return Err(Drop::BadOptions); }
                        }
                        _ => {}
                    }
                } else if o[3] & 0xf != IPOPT_TS_PRESPEC && o[3] >> 4 == 15 {
                    return Err(Drop::BadOptions);
                }
                ts = true;
            }
            IPOPT_RA => {
                if optlen < 4 { return Err(Drop::BadOptions); }
            }
            IPOPT_CIPSO => {
                // `cipso_v4_validate` without NetLabel: a DOI and tags that
                // fit the option.
                if cipso || optlen < 8 || o[2..6] == [0; 4] { return Err(Drop::BadOptions); }
                let mut j = 6;
                while j < optlen {
                    if j + 1 == optlen { return Err(Drop::BadOptions); }
                    let tag_len = o[j + 1] as usize;
                    if tag_len == 0 || tag_len > optlen - j { return Err(Drop::BadOptions); }
                    j += tag_len;
                }
                cipso = true;
            }
            _ => {}
        }
        i += optlen;
    }
    if srr { Err(Drop::SourceRoute) } else { Ok(()) }
}
