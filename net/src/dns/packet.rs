//! DNS messages, after systemd v262 `dns-packet.c`: the query a
//! transaction sends, with an OPT RR from EDNS(0) on, and the parts of a
//! reply the stub needs: header, question, A, CNAME and SOA records of
//! every section, and the OPT RR with the checks of
//! `dns_packet_extract_answer`. Other types are kept as opaque records.

use alloc::string::String;
use alloc::vec::Vec;
use super::*;

/// `DNS_PACKET_HEADER_SIZE`.
pub const HEADER_SIZE: usize = 12;
/// `DNS_PACKET_UNICAST_SIZE_MAX`.
pub const UNICAST_SIZE_MAX: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Answer,
    Authority,
    Additional,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RData {
    A([u8; 4]),
    Cname(String),
    Soa { minimum: u32 },
    Other,
}

/// A resource record of a reply. `name` is lower case, without the
/// trailing dot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rr {
    pub name: String,
    pub rtype: u16,
    pub class: u16,
    /// Seconds; RFC 2181 §8: a value with the top bit set reads as 0.
    pub ttl: u32,
    pub data: RData,
    pub section: Section,
}

impl Rr {
    /// `DNS_ANSWER_CACHEABLE`: only the answer section is cached (RFC 4795
    /// §2.9 for LLMNR, which resolved applies to DNS too).
    pub fn cacheable(&self) -> bool { self.section == Section::Answer }

    pub fn key(&self) -> Key { Key { name: self.name.clone(), rtype: self.rtype } }
}

/// The OPT pseudo-RR of a reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opt {
    pub udp_size: u16,
    pub ext_rcode: u8,
    pub version: u8,
}

pub struct Reply {
    pub id: u16,
    pub qr: bool,
    pub tc: bool,
    /// The 4-bit header code, extended by the OPT RR when there is one.
    pub rcode: u16,
    pub question: Vec<(Key, u16)>,
    pub rrs: Vec<Rr>,
    pub opt: Option<Opt>,
}

/// `dns_packet_new_query` and `dns_packet_append_opt`: one question of
/// class IN with RD set, and an OPT RR when `edns_udp_size` is given.
pub fn build_query(id: u16, name: &str, rtype: u16, edns_udp_size: Option<u16>) -> Vec<u8> {
    let mut p = Vec::with_capacity(HEADER_SIZE + name.len() + 2 + 4 + 11);
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&0x0100u16.to_be_bytes());
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&(edns_udp_size.is_some() as u16).to_be_bytes());
    for label in name.split('.') {
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    p.extend_from_slice(&rtype.to_be_bytes());
    p.extend_from_slice(&CLASS_IN.to_be_bytes());
    if let Some(size) = edns_udp_size {
        // Root owner, type OPT, class = payload size, TTL = extended rcode,
        // version 0 and no flags, no options.
        p.push(0);
        p.extend_from_slice(&TYPE_OPT.to_be_bytes());
        p.extend_from_slice(&size.to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&0u16.to_be_bytes());
    }
    p
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// `dns_packet_read_name`: a possibly compressed name at `pos`, lower case
/// and without the trailing dot, and the position after it. Pointers must
/// go backwards, so they cannot loop.
pub fn read_name(b: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    let mut after = None;
    let mut limit = pos;
    loop {
        let len = *b.get(pos)? as usize;
        if len == 0 {
            return Some((name, after.unwrap_or(pos + 1)));
        }
        match len & 0xC0 {
            0xC0 => {
                let ptr = ((len & 0x3F) << 8) | *b.get(pos + 1)? as usize;
                if ptr >= limit {
                    return None;
                }
                after.get_or_insert(pos + 2);
                limit = ptr;
                pos = ptr;
            }
            0 => {
                let label = b.get(pos + 1..pos + 1 + len)?;
                if label.contains(&b'.') {
                    return None;
                }
                if !name.is_empty() {
                    name.push('.');
                }
                for &c in label {
                    name.push(c.to_ascii_lowercase() as char);
                }
                if name.len() > NAME_MAX {
                    return None;
                }
                pos += 1 + len;
            }
            _ => return None,
        }
    }
}

/// `dns_packet_extract`: header, question and records. `None` for a
/// message that does not parse.
pub fn parse(b: &[u8]) -> Option<Reply> {
    if b.len() < HEADER_SIZE {
        return None;
    }
    let id = u16_at(b, 0)?;
    let flags = u16_at(b, 2)?;
    let qdcount = u16_at(b, 4)? as usize;
    let ancount = u16_at(b, 6)? as usize;
    let nscount = u16_at(b, 8)? as usize;
    let arcount = u16_at(b, 10)? as usize;
    let mut pos = HEADER_SIZE;

    let mut question = Vec::new();
    for _ in 0..qdcount {
        let (name, p) = read_name(b, pos)?;
        let rtype = u16_at(b, p)?;
        let class = u16_at(b, p + 2)?;
        question.push((Key { name, rtype }, class));
        pos = p + 4;
    }

    let mut rrs = Vec::new();
    let mut opt: Option<Opt> = None;
    let mut bad_opt = false;
    let n = ancount + nscount + arcount;
    for i in 0..n {
        // More records declared than present: a common router bug,
        // accepted, but EDNS is not trusted from such a server.
        if pos == b.len() && opt.is_some() {
            bad_opt = true;
            break;
        }
        let (name, p) = read_name(b, pos)?;
        let rtype = u16_at(b, p)?;
        let class = u16_at(b, p + 2)?;
        let ttl_raw = u32::from_be_bytes(b.get(p + 4..p + 8)?.try_into().unwrap());
        let rdlen = u16_at(b, p + 8)? as usize;
        let rd = p + 10;
        let rdata = b.get(rd..rd + rdlen)?;
        pos = rd + rdlen;

        if rtype == TYPE_OPT {
            // One OPT, owned by the root, in the additional section.
            if opt.is_some() || bad_opt || !name.is_empty() || i < ancount + nscount {
                bad_opt = true;
                continue;
            }
            let version = (ttl_raw >> 16) as u8;
            // A reply with an EDNS version newer than our query's.
            if flags & 0x8000 != 0 && version != 0 {
                return None;
            }
            opt = Some(Opt { udp_size: class, ext_rcode: (ttl_raw >> 24) as u8, version });
            continue;
        }

        let ttl = if ttl_raw & 0x8000_0000 != 0 { 0 } else { ttl_raw };
        let data = match rtype {
            TYPE_A if class == CLASS_IN => {
                if rdlen != 4 {
                    return None;
                }
                RData::A(rdata.try_into().unwrap())
            }
            TYPE_CNAME => {
                let (target, end) = read_name(b, rd)?;
                if end != pos {
                    return None;
                }
                RData::Cname(target)
            }
            TYPE_SOA => {
                let (_, m) = read_name(b, rd)?;
                let (_, r) = read_name(b, m)?;
                if r + 20 != pos {
                    return None;
                }
                RData::Soa { minimum: u32::from_be_bytes(b[r + 16..r + 20].try_into().unwrap()) }
            }
            _ => RData::Other,
        };
        let section = if i < ancount {
            Section::Answer
        } else if i < ancount + nscount {
            Section::Authority
        } else {
            Section::Additional
        };
        rrs.push(Rr { name, rtype, class, ttl, data, section });
    }

    if bad_opt {
        opt = None;
    }
    let mut rcode = flags & 0x000F;
    if let Some(o) = opt {
        rcode |= (o.ext_rcode as u16) << 4;
    }
    Some(Reply { id, qr: flags & 0x8000 != 0, tc: flags & 0x0200 != 0, rcode, question, rrs, opt })
}

impl Reply {
    /// `dns_packet_is_reply_for`: a response with exactly this question.
    pub fn is_reply_for(&self, key: &Key) -> bool {
        self.qr && self.question.len() == 1
            && self.question[0].0 == *key && self.question[0].1 == CLASS_IN
    }
}
