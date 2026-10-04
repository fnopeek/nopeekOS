//! Origin and site, the two notions every web boundary rests on.
//!
//! Origin = scheme + host + port. Documents of the same origin may read
//! each other.
//!
//! Site = scheme + registrable domain. Coarser than the origin:
//! `app.example.com` and `api.example.com` are different origins but the
//! same site, which is what lets an app talk to its own API without a
//! cookie leaking to strangers.
//!
//! The registrable domain is not "the last two labels": `a.github.io` and
//! `b.github.io` would both become `github.io`, and two unrelated user
//! sites would share cookies. So the real Public Suffix List is embedded;
//! a security boundary is not approximated when the exact answer is cheap.
//! See `docs/plan/BROWSER_FETCH_ORIGIN.md` §5.3.

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The list, sorted and without comments, so lookup is a binary search.
///
/// To regenerate:
///
/// 1. `curl -O https://publicsuffix.org/list/public_suffix_list.dat`
/// 2. Drop comments (`//`) and blank lines.
/// 3. Add the Punycode form of every non-ASCII rule. The upstream list
///    has `公司.cn` only in Unicode, but a host from a URL is Punycode;
///    without this no IDN suffix matches.
/// 4. Sort and dedupe (`sorted(set(...))`).
///
/// `psl_vectors.txt` comes from the project's `tests/test_psl.txt`,
/// reduced to ASCII cases; commented-out lines there are not vectors.
const PSL: &str = include_str!("public_suffix_list.dat");

/// An origin: scheme, host, port. The port is resolved, so
/// `https://a.de` and `https://a.de:443` are the same origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl Origin {
    /// Serialized form for an `Origin:` header. The default port is omitted,
    /// otherwise it would not match what a server echoes in
    /// `Access-Control-Allow-Origin`.
    pub fn header(&self) -> String {
        let default = if self.scheme == "https" { 443 } else { 80 };
        if self.port == default {
            alloc::format!("{}://{}", self.scheme, self.host)
        } else {
            alloc::format!("{}://{}:{}", self.scheme, self.host, self.port)
        }
    }
}

/// The origin of a URL, or `None` for URLs without one (`about:`,
/// `data:`, `beak:selftest`, anything without an authority).
///
/// A document without an origin gets no cookies and may not fetch.
pub fn origin_of(url: &str) -> Option<Origin> {
    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = url.strip_prefix("http://") {
        ("http", r)
    } else {
        return None;
    };
    let hostport = match rest.find(['/', '?', '#']) {
        Some(i) => &rest[..i],
        None => rest,
    };
    if hostport.is_empty() {
        return None;
    }
    // The userinfo part (`user@host`) is not part of the origin.
    let hostport = match hostport.rfind('@') {
        Some(i) => &hostport[i + 1..],
        None => hostport,
    };
    // IPv6 is bracketed and contains colons itself.
    let (host, port_s) = if let Some(end) = hostport.strip_prefix('[').and_then(|r| r.find(']')) {
        let h = &hostport[..end + 2];
        let rest = &hostport[end + 2..];
        (h, rest.strip_prefix(':'))
    } else {
        match hostport.rfind(':') {
            Some(i) => (&hostport[..i], Some(&hostport[i + 1..])),
            None => (hostport, None),
        }
    };
    if host.is_empty() {
        return None;
    }
    let default = if scheme == "https" { 443u16 } else { 80 };
    let port = match port_s {
        None | Some("") => default,
        Some(p) => p.parse().ok()?,
    };
    Some(Origin {
        scheme: scheme.to_string(),
        // Hosts compare case-insensitively, paths do not; normalized once here.
        host: host.to_ascii_lowercase(),
        port,
    })
}

/// The registrable domain of a host: one label more than its public
/// suffix ("eTLD+1").
///
/// `www.bbc.co.uk` -> `bbc.co.uk` · `a.github.io` -> `a.github.io` ·
/// `example.com` -> `example.com` · `co.uk` -> `None` (a bare suffix is
/// not registrable, and a cookie on it belongs to nobody).
///
/// An IP address has none.
pub fn registrable_domain(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || is_ip_literal(&host) {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    // An empty label means this is not a host (`.example.com`).
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {
        return None;
    }
    // Longest matching rule wins (PSL algorithm). Candidates are tried from
    // longest to shortest, so the first match is the longest.
    let mut best: Option<usize> = None; // number of suffix labels
    let mut exception = false;
    for start in 0..labels.len() {
        let cand = labels[start..].join(".");
        let n = labels.len() - start;
        // An exception rule (`!city.kawasaki.jp`) beats everything; the suffix
        // is then one label shorter than the rule.
        if psl_has(&alloc::format!("!{cand}")) {
            best = Some(n - 1);
            exception = true;
            break;
        }
        if psl_has(&cand) {
            best = Some(n);
            break;
        }
        // Wildcard rule (`*.ck`): matches when the remainder matches.
        if start + 1 <= labels.len() {
            let parent = labels[start + 1..].join(".");
            if !parent.is_empty() && psl_has(&alloc::format!("*.{parent}")) {
                best = Some(n);
                break;
            }
        }
    }
    // No rule matched: the PSL default is `*`, so the last label is the
    // suffix (`example.com`).
    let suffix_labels = best.unwrap_or(1);
    let _ = exception;
    if suffix_labels >= labels.len() {
        // The host is itself a public suffix (`co.uk`, `github.io`): no
        // registrable domain.
        return None;
    }
    Some(labels[labels.len() - suffix_labels - 1..].join("."))
}

/// Whether two hosts belong to the same site; the question behind
/// `SameSite`. Hosts without a registrable domain (IP addresses) are the
/// same site only if they are the same host.
pub fn same_site(a: &str, b: &str) -> bool {
    match (registrable_domain(a), registrable_domain(b)) {
        (Some(x), Some(y)) => x == y,
        // No suffix knowledge applies (IP, `localhost`): compare the host itself.
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// Same site and same scheme, i.e. schemeful same-site, which is what
/// `SameSite` means. `http://x.de` and `https://x.de` differ, otherwise a
/// plaintext hop would defeat the rule.
pub fn same_site_url(a: &str, b: &str) -> bool {
    match (origin_of(a), origin_of(b)) {
        (Some(x), Some(y)) => x.scheme == y.scheme && same_site(&x.host, &y.host),
        _ => false,
    }
}

fn is_ip_literal(host: &str) -> bool {
    if host.starts_with('[') {
        return true;
    }
    !host.is_empty() && host.split('.').all(|l| !l.is_empty() && l.bytes().all(|c| c.is_ascii_digit()))
}

/// Binary search in the sorted list, on lines of one text block (cheaper
/// than splitting it into a `Vec` at startup).
///
/// Compares bytes, not `str`: the list contains Unicode IDN suffixes, and
/// a midpoint inside a character would make `&PSL[..mid]` panic. Byte order
/// is also the correct order: the list is sorted bytewise and hosts from
/// URLs are ASCII (Punycode).
fn psl_has(rule: &str) -> bool {
    let hay = PSL.as_bytes();
    let needle = rule.as_bytes();
    let (mut lo, mut hi) = (0usize, hay.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        // Back up to the start of the line containing `mid`.
        let start = hay[..mid].iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);
        let end = hay[start..].iter().position(|&c| c == b'\n').map_or(hay.len(), |i| start + i);
        let line = &hay[start..end];
        match line.cmp(needle) {
            core::cmp::Ordering::Equal => return true,
            core::cmp::Ordering::Less => {
                // Line at `mid` is too small; continue after it. The progress check
                // keeps the loop from stalling when `mid` keeps landing in this line.
                if end + 1 <= lo {
                    return false;
                }
                lo = end + 1;
            }
            core::cmp::Ordering::Greater => {
                if start == 0 {
                    return false;
                }
                hi = start - 1;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_lookup_finds_the_real_rules() {
        assert!(psl_has("com"));
        assert!(psl_has("co.uk"));
        assert!(psl_has("github.io"));
        assert!(psl_has("s3.amazonaws.com"));
        assert!(!psl_has("example.com"));
        assert!(!psl_has("zzzz.not.a.suffix"));
    }

    /// The Public Suffix List's own test vectors (`tests/test_psl.txt`),
    /// reduced to the ASCII cases (a host from a URL is Punycode).
    #[test]
    fn the_lists_own_test_vectors() {
        let vectors = include_str!("psl_vectors.txt");
        let mut checked = 0;
        let mut bad: Vec<String> = Vec::new();
        for line in vectors.lines() {
            let Some((host, want)) = line.split_once('\t') else { continue };
            checked += 1;
            let got = registrable_domain(host);
            let got_s = got.as_deref().unwrap_or("");
            if got_s != want {
                bad.push(alloc::format!("{host}: erwartet {want:?}, bekommen {got_s:?}"));
            }
        }
        assert!(checked >= 60, "zu wenige Vektoren gelesen: {checked}");
        assert!(bad.is_empty(), "{} von {checked} falsch:\n{}", bad.len(), bad.join("\n"));
    }

    #[test]
    fn registrable_domain_matches_the_spec_examples() {
        // The examples as given on publicsuffix.org.
        assert_eq!(registrable_domain("com"), None);
        assert_eq!(registrable_domain("example.com").as_deref(), Some("example.com"));
        assert_eq!(registrable_domain("www.example.com").as_deref(), Some("example.com"));
        assert_eq!(registrable_domain("uk.com").as_deref(), None);
        assert_eq!(registrable_domain("example.uk.com").as_deref(), Some("example.uk.com"));
        assert_eq!(registrable_domain("a.b.example.uk.com").as_deref(), Some("example.uk.com"));
    }

    /// Hosts where "the last two labels" would be wrong.
    #[test]
    fn the_seven_hosts_the_corpus_measured() {
        assert_eq!(registrable_domain("bakkot.github.io").as_deref(), Some("bakkot.github.io"));
        assert_eq!(registrable_domain("tc39.github.io").as_deref(), Some("tc39.github.io"));
        assert_eq!(registrable_domain("pajhome.org.uk").as_deref(), Some("pajhome.org.uk"));
        assert_eq!(
            registrable_domain("github-cloud.s3.amazonaws.com").as_deref(),
            Some("github-cloud.s3.amazonaws.com")
        );
        // Two unrelated user sites under the same suffix are not the same site.
        assert!(!same_site("tc39.github.io", "bakkot.github.io"));
        assert!(!same_site("a.s3.amazonaws.com", "b.s3.amazonaws.com"));
    }

    /// IDN suffixes must be present in Punycode, otherwise `a.公司.cn` and
    /// `b.公司.cn` would be the same site. Guards step 3 of regeneration.
    #[test]
    fn idn_suffixes_are_present_in_punycode() {
        assert!(psl_has("xn--55qx5d.cn"), "公司.cn fehlt als Punycode");
        assert!(!same_site("xn--85x722f.xn--55qx5d.cn", "shishi.xn--55qx5d.cn"));
        assert_eq!(
            registrable_domain("www.xn--85x722f.xn--55qx5d.cn").as_deref(),
            Some("xn--85x722f.xn--55qx5d.cn")
        );
    }

    #[test]
    fn same_site_is_coarser_than_origin() {
        assert!(same_site("app.example.com", "api.example.com"));
        assert!(same_site("example.com", "www.example.com"));
        assert!(!same_site("example.com", "example.org"));
        assert!(!same_site("evil.com", "example.com"));
        // Two-label country suffixes.
        assert!(same_site("www.bbc.co.uk", "news.bbc.co.uk"));
        assert!(!same_site("bbc.co.uk", "itv.co.uk"));
    }

    #[test]
    fn same_site_is_schemeful() {
        assert!(same_site_url("https://a.example.com/x", "https://b.example.com/y"));
        assert!(!same_site_url("http://a.example.com/x", "https://b.example.com/y"));
    }

    #[test]
    fn origins_split_the_way_the_header_needs() {
        let o = origin_of("https://Example.COM/pfad?q=1").unwrap();
        assert_eq!(o.host, "example.com");
        assert_eq!(o.port, 443);
        assert_eq!(o.header(), "https://example.com");
        // The default port is omitted from the header, others are not.
        assert_eq!(origin_of("https://x.de:443/").unwrap().header(), "https://x.de");
        assert_eq!(origin_of("https://x.de:8443/").unwrap().header(), "https://x.de:8443");
        assert_eq!(origin_of("http://x.de/").unwrap().header(), "http://x.de");
        // Same origin means all three parts equal.
        assert_eq!(origin_of("https://a.de/x"), origin_of("https://a.de:443/y"));
        assert_ne!(origin_of("https://a.de/x"), origin_of("http://a.de/x"));
        assert_ne!(origin_of("https://a.de/x"), origin_of("https://b.de/x"));
    }

    #[test]
    fn things_without_an_origin_have_none() {
        assert!(origin_of("beak:selftest").is_none());
        assert!(origin_of("about:blank").is_none());
        assert!(origin_of("data:text/html,x").is_none());
        assert!(origin_of("https://").is_none());
    }

    #[test]
    fn ip_literals_are_their_own_site() {
        assert_eq!(registrable_domain("192.168.1.1"), None);
        assert!(same_site("192.168.1.1", "192.168.1.1"));
        assert!(!same_site("192.168.1.1", "192.168.1.2"));
        assert_eq!(origin_of("https://[::1]:8443/").unwrap().host, "[::1]");
    }
}
