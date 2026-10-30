//! The answer cache, after systemd v262 `resolved-dns-cache.c`: records
//! of the answer section by (name, type), for the lowest TTL of the whole
//! answer and at most 2 h; NODATA and NXDOMAIN for the SOA's TTL and
//! MINIMUM (RFC 2308 §5), NXDOMAIN under the pseudo-type ANY so it holds
//! for every type; SERVFAIL for 10 s. A full cache drops the entries that
//! expire first. Time is microseconds.
//!
//! Not ported: DNSSEC state, NSEC/DNAME, stale serving, mDNS shared
//! owners.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use super::packet::{RData, Rr};
use super::*;

/// `CACHE_TTL_MAX_USEC`.
const CACHE_TTL_MAX_USEC: u64 = 2 * 3600 * USEC_PER_SEC;
/// `CACHE_TTL_STRANGE_RCODE_USEC`.
const CACHE_TTL_STRANGE_RCODE_USEC: u64 = 10 * USEC_PER_SEC;
/// `DEFAULT_CACHE_MAX`.
pub const DEFAULT_CACHE_MAX: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Positive(Rr),
    NoData,
    NxDomain,
    Rcode(u16),
}

#[derive(Clone, Debug)]
struct Item {
    kind: Kind,
    until: u64,
    /// Learnt by a lookup of this key (or its CNAME), not on the side.
    primary: bool,
}

/// What a lookup found.
#[derive(Debug, PartialEq)]
pub struct Hit {
    pub rcode: u16,
    /// The records, with their TTL clamped to what is left.
    pub answer: Vec<Rr>,
}

pub struct Cache {
    by_key: BTreeMap<Key, Vec<Item>>,
    n_items: usize,
    pub cache_max: usize,
}

/// `dns_answer_min_ttl`.
fn answer_min_ttl(answer: &[Rr]) -> u32 {
    answer.iter().map(|r| r.ttl).min().unwrap_or(u32::MAX)
}

/// `calculate_until_valid`: the TTL in seconds from `timestamp`, at most
/// 2 h; for a SOA in negative caching, at most its MINIMUM.
fn until_valid(rr: &Rr, min_ttl: u32, timestamp: u64, use_soa_minimum: bool) -> u64 {
    let mut ttl = min_ttl;
    if let (true, RData::Soa { minimum }) = (use_soa_minimum, &rr.data) {
        ttl = ttl.min(*minimum);
    }
    timestamp + (ttl as u64 * USEC_PER_SEC).min(CACHE_TTL_MAX_USEC)
}

impl Default for Cache {
    fn default() -> Self { Self::new() }
}

impl Cache {
    pub fn new() -> Self {
        Cache { by_key: BTreeMap::new(), n_items: 0, cache_max: DEFAULT_CACHE_MAX }
    }

    pub fn len(&self) -> usize { self.n_items }
    pub fn is_empty(&self) -> bool { self.n_items == 0 }

    /// `dns_cache_flush`.
    pub fn flush(&mut self) {
        self.by_key.clear();
        self.n_items = 0;
    }

    fn remove_by_key(&mut self, k: &Key) {
        if let Some(v) = self.by_key.remove(k) {
            self.n_items -= v.len();
        }
    }

    /// `dns_cache_prune`: everything past its TTL goes.
    pub fn prune(&mut self, now: u64) {
        let expired: Vec<Key> = self.by_key.iter()
            .filter(|(_, v)| v.iter().any(|i| i.until <= now))
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired {
            self.remove_by_key(&k);
        }
    }

    /// `dns_cache_make_space`: room for `add` more, dropping the keys that
    /// expire first.
    fn make_space(&mut self, add: usize) {
        if add == 0 || self.cache_max == 0 {
            return;
        }
        while self.n_items > 0 && self.n_items + add >= self.cache_max {
            let first = self.by_key.iter()
                .min_by_key(|(_, v)| v.iter().map(|i| i.until).min().unwrap_or(0))
                .map(|(k, _)| k.clone());
            match first {
                Some(k) => self.remove_by_key(&k),
                None => break,
            }
        }
    }

    fn link(&mut self, key: Key, item: Item) {
        self.by_key.entry(key).or_default().push(item);
        self.n_items += 1;
    }

    /// `dns_cache_put_positive`.
    fn put_positive(&mut self, rr: &Rr, answer: &[Rr], primary: bool, timestamp: u64) {
        let min_ttl = answer_min_ttl(answer).min(rr.ttl);
        let key = rr.key();
        if min_ttl == 0 {
            // A zero TTL replaces what was cached for it.
            if let Some(v) = self.by_key.get_mut(&key) {
                let before = v.len();
                v.retain(|i| i.kind != Kind::Positive(rr.clone()));
                self.n_items -= before - v.len();
                if v.is_empty() {
                    self.by_key.remove(&key);
                }
            }
            return;
        }
        let until = until_valid(rr, min_ttl, timestamp, false);
        if let Some(v) = self.by_key.get_mut(&key) {
            if let Some(i) = v.iter_mut().find(|i| matches!(&i.kind, Kind::Positive(r) if r.data == rr.data)) {
                i.kind = Kind::Positive(rr.clone());
                i.until = until;
                i.primary = primary;
                return;
            }
        }
        self.make_space(1);
        self.link(key, Item { kind: Kind::Positive(rr.clone()), until, primary });
    }

    /// `dns_cache_put`: what a reply to `key` with `rcode` taught us.
    /// `answer` holds the records of every section.
    pub fn put(&mut self, key: &Key, rcode: u16, answer: &[Rr], timestamp: u64) {
        if self.cache_max == 0 {
            return;
        }
        // `dns_cache_remove_previous`.
        self.remove_by_key(key);
        for rr in answer.iter().filter(|r| r.cacheable()) {
            self.remove_by_key(&rr.key());
        }

        let weird_rcode = match rcode {
            RCODE_SUCCESS | RCODE_NXDOMAIN => {
                if answer.is_empty() {
                    return;
                }
                false
            }
            // Some zones answer SERVFAIL consistently; forwarders pass it on.
            RCODE_SERVFAIL => true,
            _ => return,
        };

        self.make_space(answer.len() + 1);
        for rr in answer.iter().filter(|r| r.cacheable()) {
            let primary = rr.name == key.name && (rr.rtype == key.rtype || rr.rtype == TYPE_CNAME);
            if !primary && self.by_key.get(&rr.key()).is_some_and(|v| v.first().is_some_and(|i| i.primary)) {
                return;
            }
            self.put_positive(rr, answer, primary, timestamp);
        }

        // Negative entries only when nothing answers the key itself; a
        // CNAME is cached negatively on its target, not here.
        if answer.iter().any(|r| r.name == key.name && (r.rtype == key.rtype || r.rtype == TYPE_CNAME)) {
            return;
        }
        // RFC 2308: a SOA of an enclosing zone enables negative caching.
        let soa = answer.iter().find(|r| r.rtype == TYPE_SOA && name_endswith(&key.name, &r.name));
        if soa.is_none() && !weird_rcode {
            return;
        }
        self.put_negative(key, rcode, answer, soa, timestamp);
    }

    /// `dns_cache_put_negative`.
    fn put_negative(&mut self, key: &Key, rcode: u16, answer: &[Rr], soa: Option<&Rr>, timestamp: u64) {
        if key.rtype == TYPE_ANY {
            return;
        }
        let until;
        let kind;
        if rcode == RCODE_SUCCESS || rcode == RCODE_NXDOMAIN {
            let Some(soa) = soa else { return };
            let RData::Soa { minimum } = soa.data else { return };
            if minimum == 0 || soa.ttl == 0 {
                return;
            }
            until = until_valid(soa, answer_min_ttl(answer), timestamp, true);
            kind = if rcode == RCODE_SUCCESS { Kind::NoData } else { Kind::NxDomain };
        } else if rcode == RCODE_SERVFAIL {
            until = timestamp + CACHE_TTL_STRANGE_RCODE_USEC;
            kind = Kind::Rcode(rcode);
        } else {
            return;
        }
        self.make_space(1);
        let k = if kind == Kind::NxDomain {
            Key { name: key.name.clone(), rtype: TYPE_ANY }
        } else {
            key.clone()
        };
        self.remove_by_key(&k);
        self.link(k, Item { kind, until, primary: true });
    }

    /// `dns_cache_get_by_key_follow_cname_dname_nsec`.
    fn get_follow_cname(&self, key: &Key) -> Option<&Vec<Item>> {
        if let Some(v) = self.by_key.get(key) {
            return Some(v);
        }
        let any = Key { name: key.name.clone(), rtype: TYPE_ANY };
        if let Some(v) = self.by_key.get(&any) {
            if v.iter().any(|i| i.kind == Kind::NxDomain) {
                return Some(v);
            }
        }
        if key.rtype != TYPE_CNAME {
            let cname = Key { name: key.name.clone(), rtype: TYPE_CNAME };
            if let Some(v) = self.by_key.get(&cname) {
                if v.iter().all(|i| i.kind != Kind::NoData) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// `dns_cache_lookup` with `SD_RESOLVED_NO_STALE` and clamped TTLs.
    pub fn lookup(&self, key: &Key, now: u64) -> Option<Hit> {
        if key.rtype == TYPE_ANY {
            return None;
        }
        let items = self.get_follow_cname(key)?;
        let mut answer = Vec::new();
        let mut nxdomain = false;
        let mut rcode = None;
        for i in items {
            if i.until < now {
                return None;
            }
            match &i.kind {
                Kind::NxDomain => nxdomain = true,
                Kind::Rcode(r) => rcode = Some(*r),
                Kind::NoData => {}
                Kind::Positive(rr) => {
                    let mut rr = rr.clone();
                    rr.ttl = rr.ttl.min(((i.until - now) / USEC_PER_SEC) as u32);
                    answer.push(rr);
                }
            }
        }
        if let Some(r) = rcode {
            return Some(Hit { rcode: r, answer });
        }
        let rcode = if answer.is_empty() && nxdomain { RCODE_NXDOMAIN } else { RCODE_SUCCESS };
        Some(Hit { rcode, answer })
    }
}
