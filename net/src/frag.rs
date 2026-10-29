//! IPv4 reassembly, after Linux `net/ipv4/ip_fragment.c` and
//! `net/ipv4/inet_fragment.c`: `ip_defrag`, `ip_frag_queue`,
//! `inet_frag_queue_insert`, `ip_frag_too_far`, `ip_frag_reasm` and
//! `ip_expire`, with the limits of `ipv4_frags_init_net`.
//!
//! A queue is keyed by (source, destination, identification, protocol).
//! Overlapping fragments drop the whole datagram (RFC 5722, as Linux does
//! for IPv4 too); an exact duplicate is dropped alone. A queue lives 30 s.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

const HZ: u64 = 100;
/// `IP_FRAG_TIME`.
pub const FRAG_TIME: u64 = 30 * HZ;
/// `fqdir->high_thresh`: above it no fragment is taken until queues expire.
const HIGH_THRESH: usize = 4 * 1024 * 1024;
/// `fqdir->max_dist`: fragments from the same source between two of one
/// datagram, after which its queue starts over.
const MAX_DIST: u32 = 64;
/// `inet_peer_threshold`.
const PEERS_MAX: usize = 65536 + 128;
/// What one fragment costs beyond its bytes (an skb's overhead).
const FRAG_OVERHEAD: usize = 256;

const IP_MF: u16 = 0x2000;
const IP_OFFSET: u16 = 0x1fff;

type Key = ([u8; 4], [u8; 4], u16, u8);

struct Queue {
    /// Fragment data by offset; never overlapping.
    frags: BTreeMap<usize, Vec<u8>>,
    /// The header of fragment 0, once it came.
    head: Option<Vec<u8>>,
    /// Fragment 0 whole, for the reassembly timeout's ICMP error.
    first: Option<Vec<u8>>,
    /// The datagram's length, as far as known.
    len: usize,
    meat: usize,
    last_in: bool,
    ecn: u8,
    rid: u32,
    expires: u64,
    mem: usize,
}

/// What a fragment led to.
pub enum Defragged {
    /// The datagram is whole.
    Complete(Vec<u8>),
    /// It waits for more fragments.
    Pending,
    /// It was dropped (and with it maybe its queue).
    Dropped,
}

/// The reassembly queues (`fqdir`).
pub struct Defrag {
    queues: BTreeMap<Key, Queue>,
    mem: usize,
    /// Per source: `inet_peer->rid`, and when it was last used.
    peers: BTreeMap<[u8; 4], (u32, u64)>,
    /// `ReasmFails`, `ReasmTimeout`, `ReasmOKs`.
    pub fails: u64,
    pub timeouts: u64,
    pub oks: u64,
}

/// `IPFRAG_ECN_*` of a header's TOS.
fn frag_ecn(tos: u8) -> u8 { 1 << (tos & 3) }

/// `ip_frag_ecn_table`: the ECN field of the whole datagram, or `None` for
/// a mix that cannot be (a not-ECT fragment beside an ECT or CE one).
fn ecn_of(bits: u8) -> Option<u8> {
    const NOT_ECT: u8 = 1;
    const CE: u8 = 8;
    if bits & NOT_ECT != 0 && bits != NOT_ECT { return None; }
    Some(if bits & CE != 0 { 3 } else { 0 })
}

impl Defrag {
    pub const fn new() -> Self {
        Defrag { queues: BTreeMap::new(), mem: 0, peers: BTreeMap::new(), fails: 0, timeouts: 0, oks: 0 }
    }

    pub fn mem(&self) -> usize { self.mem }

    fn kill(&mut self, key: &Key) {
        if let Some(q) = self.queues.remove(key) {
            self.mem -= q.mem;
        }
        self.fails += 1;
    }

    /// `ip_defrag` for the fragment `ip` (a whole IPv4 datagram with MF set
    /// or an offset), its header already checked.
    pub fn input(&mut self, ip: &[u8], now: u64) -> Defragged {
        let ihl = ((ip[0] & 0x0f) as usize) * 4;
        let tot = (u16::from_be_bytes([ip[2], ip[3]]) as usize).min(ip.len());
        let frag_off = u16::from_be_bytes([ip[6], ip[7]]);
        let key: Key = (ip[12..16].try_into().unwrap(), ip[16..20].try_into().unwrap(),
                        u16::from_be_bytes([ip[4], ip[5]]), ip[9]);
        if self.mem > HIGH_THRESH {
            self.fails += 1;
            return Defragged::Dropped;
        }

        // inet_frag_find
        if !self.queues.contains_key(&key) {
            self.queues.insert(key, Queue {
                frags: BTreeMap::new(), head: None, first: None, len: 0, meat: 0, last_in: false,
                ecn: 0, rid: 0, expires: now + FRAG_TIME, mem: 0,
            });
        }

        // ip_frag_too_far: too many fragments from this source since this
        // queue's last one; start the queue over (ip_frag_reinit).
        if self.peers.len() >= PEERS_MAX && !self.peers.contains_key(&key.0) {
            self.peers.retain(|_, p| now.saturating_sub(p.1) < FRAG_TIME);
        }
        let rid = {
            let p = self.peers.entry(key.0).or_insert((0, now));
            p.0 = p.0.wrapping_add(1);
            p.1 = now;
            p.0
        };
        {
            let q = self.queues.get_mut(&key).unwrap();
            let start = core::mem::replace(&mut q.rid, rid);
            if !q.frags.is_empty() && rid.wrapping_sub(start) > MAX_DIST {
                self.mem -= q.mem;
                *q = Queue {
                    frags: BTreeMap::new(), head: None, first: None, len: 0, meat: 0, last_in: false,
                    ecn: 0, rid, expires: now + FRAG_TIME, mem: 0,
                };
                self.fails += 1;
            }
        }

        // ip_frag_queue
        let offset = ((frag_off & IP_OFFSET) as usize) << 3;
        let mf = frag_off & IP_MF != 0;
        let mut end = offset + tot.saturating_sub(ihl);
        let q = self.queues.get_mut(&key).unwrap();
        if !mf {
            if end < q.len || (q.last_in && end != q.len) {
                self.kill(&key);
                return Defragged::Dropped;
            }
            q.last_in = true;
            q.len = end;
        } else {
            end &= !7;
            if end > q.len {
                if q.last_in {
                    self.kill(&key);
                    return Defragged::Dropped;
                }
                q.len = end;
            }
        }
        if end == offset {
            self.kill(&key);
            return Defragged::Dropped;
        }

        // inet_frag_queue_insert: overlap drops the queue, a duplicate only
        // itself.
        let mut covered = 0;
        let mut overlap = false;
        for (&o, d) in q.frags.range(..end) {
            let e = o + d.len();
            if e <= offset { continue; }
            overlap = true;
            covered += e.min(end) - o.max(offset);
        }
        if overlap {
            if covered == end - offset {
                return Defragged::Dropped;
            }
            self.kill(&key);
            return Defragged::Dropped;
        }
        let data = ip[ihl..ihl + (end - offset)].to_vec();
        let cost = data.len() + FRAG_OVERHEAD;
        q.meat += data.len();
        q.mem += cost;
        q.ecn |= frag_ecn(ip[1]);
        q.frags.insert(offset, data);
        if offset == 0 {
            q.head = Some(ip[..ihl].to_vec());
            q.first = Some(ip[..tot].to_vec());
        }
        self.mem += cost;

        if !(q.head.is_some() && q.last_in && q.meat == q.len) {
            return Defragged::Pending;
        }

        // ip_frag_reasm
        let q = self.queues.remove(&key).unwrap();
        self.mem -= q.mem;
        let head = q.head.unwrap();
        let len = head.len() + q.len;
        let Some(ecn) = ecn_of(q.ecn) else {
            self.fails += 1;
            return Defragged::Dropped;
        };
        if len > 65535 {
            self.fails += 1;
            return Defragged::Dropped;
        }
        let mut out = Vec::with_capacity(len);
        out.extend_from_slice(&head);
        for d in q.frags.values() { out.extend_from_slice(d); }
        out[1] |= ecn;
        out[2..4].copy_from_slice(&(len as u16).to_be_bytes());
        out[6] = 0;
        out[7] = 0;
        out[10] = 0;
        out[11] = 0;
        let c = !crate::csum::fold(crate::csum::sum(&out[..head.len()]));
        out[10..12].copy_from_slice(&c.to_be_bytes());
        self.oks += 1;
        Defragged::Complete(out)
    }

    /// `ip_expire` for every queue past its 30 s: fragment 0 of each that
    /// had it, for the Time Exceeded its sender is owed.
    pub fn expire(&mut self, now: u64) -> Vec<Vec<u8>> {
        let due: Vec<Key> = self.queues.iter().filter(|(_, q)| now >= q.expires).map(|(k, _)| *k).collect();
        let mut firsts = Vec::new();
        for k in due {
            let q = self.queues.remove(&k).unwrap();
            self.mem -= q.mem;
            self.fails += 1;
            self.timeouts += 1;
            if let Some(f) = q.first { firsts.push(f); }
        }
        firsts
    }
}

impl Default for Defrag {
    fn default() -> Self { Self::new() }
}
