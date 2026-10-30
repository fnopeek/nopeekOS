//! A DHCP message: the BOOTP header and its options, after systemd v262
//! `dhcp-message.c` and `tlv-util.c` (`TLV_DHCP4`). Options keep every
//! instance of a code; a variable-length option reads as their
//! concatenation (RFC 3396), a fixed-size one as the first instance of
//! that size. `file` and `sname` carry options when option 52 says so
//! (RFC 2131 §4.1).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub const BOOTREQUEST: u8 = 1;
pub const BOOTREPLY: u8 = 2;
pub const ARPHRD_ETHER: u8 = 1;

/// `sizeof(DHCPMessageHeader)`: the fixed fields through the magic cookie.
pub const HEADER_LEN: usize = 240;
/// `BOOTP_MESSAGE_SIZE`: the shortest message we send.
const BOOTP_MESSAGE_SIZE: usize = 236 + 64;
/// `UDP_PAYLOAD_MAX_SIZE`.
const UDP_PAYLOAD_MAX_SIZE: usize = 65535 - 20 - 8;
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
/// `TLV_MAX_ENTRIES`.
const MAX_ENTRIES: usize = 4096;

const OPT_PAD: u8 = 0;
const OPT_END: u8 = 255;
const OPT_OVERLOAD: u8 = 52;
const OVERLOAD_FILE: u8 = 1 << 0;
const OVERLOAD_SNAME: u8 = 1 << 1;

#[derive(Clone)]
pub struct Message {
    pub op: u8,
    pub htype: u8,
    pub hlen: u8,
    pub xid: u32,
    pub secs: u16,
    pub flags: u16,
    pub ciaddr: [u8; 4],
    pub yiaddr: [u8; 4],
    pub siaddr: [u8; 4],
    pub giaddr: [u8; 4],
    pub chaddr: [u8; 16],
    pub sname: [u8; 64],
    pub file: [u8; 128],
    /// Without the cookie the message is BOOTP and has no options.
    pub magic: bool,
    options: BTreeMap<u8, Vec<Vec<u8>>>,
    entries: usize,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    /// `-EBADMSG`.
    BadMsg,
    /// `-E2BIG`.
    TooBig,
}

impl Message {
    /// `dhcp_message_init_header`.
    pub fn new(op: u8, xid: u32, hw: &[u8; 6]) -> Self {
        let mut chaddr = [0; 16];
        chaddr[..6].copy_from_slice(hw);
        Message {
            op, htype: ARPHRD_ETHER, hlen: 6, xid, secs: 0, flags: 0,
            ciaddr: [0; 4], yiaddr: [0; 4], siaddr: [0; 4], giaddr: [0; 4],
            chaddr, sname: [0; 64], file: [0; 128], magic: true,
            options: BTreeMap::new(), entries: 0,
        }
    }

    pub fn set_broadcast_flag(&mut self, b: bool) {
        if b { self.flags |= 0x8000 } else { self.flags &= !0x8000 }
    }

    pub fn has_option(&self, code: u8) -> bool {
        self.options.contains_key(&code)
    }

    fn append_impl(&mut self, code: u8, data: &[u8]) -> Result<(), Error> {
        if self.entries >= MAX_ENTRIES {
            return Err(Error::TooBig);
        }
        self.options.entry(code).or_default().push(data.to_vec());
        self.entries += 1;
        Ok(())
    }

    /// `tlv_append` with `TLV_MERGE`: data longer than 255 bytes is split
    /// over several instances.
    pub fn append_option(&mut self, code: u8, data: &[u8]) -> Result<(), Error> {
        if code == OPT_PAD || code == OPT_END {
            return Err(Error::BadMsg);
        }
        let mut rest = data;
        while rest.len() > 255 {
            self.append_impl(code, &rest[..255])?;
            rest = &rest[255..];
        }
        self.append_impl(code, rest)
    }

    /// `dhcp_message_get_option`: the first instance of exactly `len` bytes.
    pub fn option_exact(&self, code: u8, len: usize) -> Option<&[u8]> {
        self.options.get(&code)?.iter().find(|v| v.len() == len).map(|v| &v[..])
    }

    /// `tlv_get_alloc`: every instance, concatenated.
    pub fn option(&self, code: u8) -> Option<Vec<u8>> {
        let all = self.options.get(&code)?;
        Some(all.iter().flatten().copied().collect())
    }

    pub fn option_u8(&self, code: u8) -> Option<u8> {
        self.option_exact(code, 1).map(|v| v[0])
    }

    pub fn option_u16(&self, code: u8) -> Option<u16> {
        self.option_exact(code, 2).map(|v| u16::from_be_bytes([v[0], v[1]]))
    }

    pub fn option_addr(&self, code: u8) -> Option<[u8; 4]> {
        self.option_exact(code, 4).map(|v| v.try_into().unwrap())
    }

    /// `dhcp_message_get_option_addresses`: `Err` when the length is not a
    /// multiple of four, `Ok(None)` when absent or empty.
    pub fn option_addresses(&self, code: u8) -> Result<Option<Vec<[u8; 4]>>, Error> {
        let Some(v) = self.option(code) else { return Ok(None) };
        if v.len() % 4 != 0 {
            return Err(Error::BadMsg);
        }
        if v.is_empty() {
            return Ok(None);
        }
        Ok(Some(v.chunks(4).map(|c| c.try_into().unwrap()).collect()))
    }

    /// `tlv_parse` with `TLV_DHCP4`: PAD skipped, END stops.
    fn parse_options(&mut self, mut b: &[u8]) -> Result<(), Error> {
        while !b.is_empty() {
            let code = b[0];
            b = &b[1..];
            if code == OPT_PAD {
                continue;
            }
            if code == OPT_END {
                break;
            }
            let Some(&len) = b.first() else { return Err(Error::BadMsg) };
            b = &b[1..];
            let len = len as usize;
            if b.len() < len {
                return Err(Error::BadMsg);
            }
            self.append_impl(code, &b[..len])?;
            b = &b[len..];
        }
        Ok(())
    }

    /// `dhcp_message_parse`: `op` 0 takes either; `xid` and `hw`, when
    /// given, must match.
    pub fn parse(b: &[u8], op: u8, xid: Option<u32>, hw: Option<&[u8; 6]>) -> Result<Self, Error> {
        if b.len() < HEADER_LEN || b.len() > UDP_PAYLOAD_MAX_SIZE {
            return Err(Error::BadMsg);
        }
        if b[0] != BOOTREQUEST && b[0] != BOOTREPLY {
            return Err(Error::BadMsg);
        }
        if op != 0 && b[0] != op {
            return Err(Error::BadMsg);
        }
        let mxid = u32::from_be_bytes(b[4..8].try_into().unwrap());
        if xid.is_some_and(|x| x != mxid) {
            return Err(Error::BadMsg);
        }
        if b[1] != ARPHRD_ETHER {
            return Err(Error::BadMsg);
        }
        let hlen = b[2];
        if hlen as usize > 16 {
            return Err(Error::BadMsg);
        }
        if hw.is_some_and(|h| b[28..28 + hlen as usize] != h[..]) {
            return Err(Error::BadMsg);
        }
        let a = |o: usize| -> [u8; 4] { b[o..o + 4].try_into().unwrap() };
        let mut m = Message {
            op: b[0], htype: b[1], hlen, xid: mxid,
            secs: u16::from_be_bytes([b[8], b[9]]),
            flags: u16::from_be_bytes([b[10], b[11]]),
            ciaddr: a(12), yiaddr: a(16), siaddr: a(20), giaddr: a(24),
            chaddr: b[28..44].try_into().unwrap(),
            sname: b[44..108].try_into().unwrap(),
            file: b[108..236].try_into().unwrap(),
            magic: b[236..240] == MAGIC_COOKIE,
            options: BTreeMap::new(), entries: 0,
        };
        if !m.magic {
            return Ok(m);
        }
        m.parse_options(&b[HEADER_LEN..])?;
        let overload = m.option_u8(OPT_OVERLOAD).unwrap_or(0);
        if overload & OVERLOAD_FILE != 0 {
            let file = m.file;
            m.parse_options(&file)?;
            m.file = [0; 128];
        }
        if overload & OVERLOAD_SNAME != 0 {
            let sname = m.sname;
            m.parse_options(&sname)?;
            m.sname = [0; 64];
        }
        Ok(m)
    }

    /// `dhcp_message_build`: options sorted by code, END, padded to the
    /// size of a BOOTP message.
    pub fn build(&self) -> Result<Vec<u8>, Error> {
        let mut b = Vec::with_capacity(BOOTP_MESSAGE_SIZE + 64);
        b.extend_from_slice(&[self.op, self.htype, self.hlen, 0]);
        b.extend_from_slice(&self.xid.to_be_bytes());
        b.extend_from_slice(&self.secs.to_be_bytes());
        b.extend_from_slice(&self.flags.to_be_bytes());
        for a in [self.ciaddr, self.yiaddr, self.siaddr, self.giaddr] {
            b.extend_from_slice(&a);
        }
        b.extend_from_slice(&self.chaddr);
        b.extend_from_slice(&self.sname);
        b.extend_from_slice(&self.file);
        b.extend_from_slice(if self.magic { &MAGIC_COOKIE } else { &[0; 4] });
        for (&code, all) in &self.options {
            for v in all {
                b.push(code);
                b.push(v.len() as u8);
                b.extend_from_slice(v);
            }
        }
        b.push(OPT_END);
        if b.len() > UDP_PAYLOAD_MAX_SIZE {
            return Err(Error::TooBig);
        }
        if b.len() < BOOTP_MESSAGE_SIZE {
            b.resize(BOOTP_MESSAGE_SIZE, 0);
        }
        Ok(b)
    }
}
