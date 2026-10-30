//! SNMP MIB counters under the names Linux's `nstat` prints
//! (`net/ipv4/proc.c`: `Ip`, `Icmp`, `Tcp`, `Udp`, `TcpExt`), so a run can
//! be compared with Linux's counters line by line. Counted where Linux
//! counts them.

use core::sync::atomic::{AtomicU64, Ordering};

macro_rules! mib {
    ($($v:ident = $n:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        pub enum Mib { $($v,)* }

        impl Mib {
            pub const ALL: &'static [Mib] = &[$(Mib::$v,)*];
            pub fn name(self) -> &'static str {
                match self { $(Mib::$v => $n,)* }
            }
        }
    };
}

mib! {
    IpInReceives = "IpInReceives",
    IpInHdrErrors = "IpInHdrErrors",
    IpInAddrErrors = "IpInAddrErrors",
    IpInUnknownProtos = "IpInUnknownProtos",
    IpInDelivers = "IpInDelivers",
    IpOutRequests = "IpOutRequests",
    IpOutNoRoutes = "IpOutNoRoutes",
    IpReasmReqds = "IpReasmReqds",
    IpReasmOKs = "IpReasmOKs",
    IpReasmFails = "IpReasmFails",
    IpReasmTimeout = "IpReasmTimeout",
    IpFragFails = "IpFragFails",
    IcmpInMsgs = "IcmpInMsgs",
    IcmpInErrors = "IcmpInErrors",
    IcmpInCsumErrors = "IcmpInCsumErrors",
    IcmpInDestUnreachs = "IcmpInDestUnreachs",
    IcmpInTimeExcds = "IcmpInTimeExcds",
    IcmpInEchos = "IcmpInEchos",
    IcmpInEchoReps = "IcmpInEchoReps",
    IcmpOutMsgs = "IcmpOutMsgs",
    IcmpOutRateLimitHost = "IcmpOutRateLimitHost",
    IcmpOutDestUnreachs = "IcmpOutDestUnreachs",
    IcmpOutTimeExcds = "IcmpOutTimeExcds",
    IcmpOutParmProbs = "IcmpOutParmProbs",
    IcmpOutEchos = "IcmpOutEchos",
    IcmpOutEchoReps = "IcmpOutEchoReps",
    TcpActiveOpens = "TcpActiveOpens",
    TcpPassiveOpens = "TcpPassiveOpens",
    TcpAttemptFails = "TcpAttemptFails",
    TcpEstabResets = "TcpEstabResets",
    TcpInSegs = "TcpInSegs",
    TcpOutSegs = "TcpOutSegs",
    TcpRetransSegs = "TcpRetransSegs",
    TcpInErrs = "TcpInErrs",
    TcpOutRsts = "TcpOutRsts",
    TcpInCsumErrors = "TcpInCsumErrors",
    UdpInDatagrams = "UdpInDatagrams",
    UdpNoPorts = "UdpNoPorts",
    UdpInErrors = "UdpInErrors",
    UdpOutDatagrams = "UdpOutDatagrams",
    UdpRcvbufErrors = "UdpRcvbufErrors",
    UdpInCsumErrors = "UdpInCsumErrors",
    UdpIgnoredMulti = "UdpIgnoredMulti",
    TcpExtTCPTimeouts = "TcpExtTCPTimeouts",
    TcpExtTCPLossProbes = "TcpExtTCPLossProbes",
    TcpExtTCPRcvQDrop = "TcpExtTCPRcvQDrop",
}

const N: usize = Mib::ALL.len();

static COUNTERS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

pub fn inc(m: Mib) {
    COUNTERS[m as usize].fetch_add(1, Ordering::Relaxed);
}

pub fn add(m: Mib, n: u64) {
    COUNTERS[m as usize].fetch_add(n, Ordering::Relaxed);
}

pub fn get(m: Mib) -> u64 {
    COUNTERS[m as usize].load(Ordering::Relaxed)
}
