//! The cookie jar (RFC 6265 subset).
//!
//! Policy lives here, in the browser, not in the kernel: which cookie belongs
//! on which request is a browser rule, and the kernel only carries bytes.
//!
//! **This jar is session-only — nothing is written to disk.** A cookie is a
//! session credential, and a credential at rest is a separate decision with a
//! separate security discussion (where it lives, who else can read it, whether
//! it is encrypted). Until that decision is made, closing beak logs you out,
//! which is the safe end of that trade.
//!
//! Not implemented: `SameSite` (needs a notion of the initiating context,
//! which arrives with scripting), public-suffix rejection beyond the crude
//! check in `domain_ok`, and cookies on sub-resource requests (only the
//! document request carries them today).

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// One stored cookie. `domain` is stored without a leading dot; `host_only`
/// records whether the server named a `Domain` at all, because a host-only
/// cookie must NOT be sent to sub-domains.
struct Cookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    host_only: bool,
    secure: bool,
    /// Absolute expiry, seconds since the epoch. `None` = session cookie,
    /// which for this jar means "until beak quits".
    expires: Option<i64>,
}

/// A jar big enough for real browsing. Past the cap the oldest entry is
/// dropped rather than the newest refused, so a site that sets a tracking
/// cookie per page view cannot lock out the session cookie you need.
const MAX_COOKIES: usize = 256;

/// A set of cookies. The browser holds one; a test holds its own, which is
/// what keeps the domain-matching rules testable without a shared global.
#[derive(Default)]
pub struct Jar {
    cookies: Vec<Cookie>,
}

static mut JAR: Option<Jar> = None;

/// The browser's jar. One per process, created on first use.
fn global() -> &'static mut Jar {
    unsafe {
        let p = core::ptr::addr_of_mut!(JAR);
        if (*p).is_none() {
            *p = Some(Jar::default());
        }
        (*p).as_mut().unwrap()
    }
}

/// File everything a response said with `Set-Cookie`, into the browser's jar.
pub fn store(url: &str, headers: &str, now: i64) {
    global().store(url, headers, now)
}

/// The `Cookie:` header value for `url` from the browser's jar.
pub fn header_for(url: &str, now: i64) -> String {
    global().header_for(url, now)
}

/// How many cookies are held, for the diagnostic line.
pub fn count() -> usize {
    global().cookies.len()
}

/// Split a URL into (host, path, is_https). Accepts what beak's address bar
/// produces: `https://host/path`, or a bare `host/path` (https assumed).
fn split_url(url: &str) -> (String, String, bool) {
    let (rest, secure) = match url.strip_prefix("https://") {
        Some(r) => (r, true),
        None => match url.strip_prefix("http://") {
            Some(r) => (r, false),
            None => (url, true),
        },
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end].split('@').next_back().unwrap_or("").to_ascii_lowercase();
    // Strip a port: cookies do not distinguish them (RFC 6265 §8.5).
    let host = host.split(':').next().unwrap_or("").to_string();
    let path = match rest[end..].find(['?', '#']) {
        Some(i) => &rest[end..end + i],
        None => &rest[end..],
    };
    let path = if path.is_empty() { "/" } else { path };
    (host, path.to_string(), secure)
}

/// The default path of a cookie set from `path` (RFC 6265 §5.1.4): the
/// directory, not the document. Without this a cookie set at `/login` would
/// never be sent to `/account`.
fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => path[..i].to_string(),
    }
}

/// Does `host` fall under `domain` (RFC 6265 §5.1.3)? Either identical, or a
/// sub-domain — and the boundary must be a dot, so `evil-example.com` does
/// not match `example.com`.
fn domain_match(host: &str, domain: &str) -> bool {
    host == domain
        || (host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

/// Does `req` fall under the cookie's `path` (RFC 6265 §5.1.4)?
fn path_match(req: &str, path: &str) -> bool {
    if req == path {
        return true;
    }
    if !req.starts_with(path) {
        return false;
    }
    path.ends_with('/') || req.as_bytes().get(path.len()) == Some(&b'/')
}

/// May `host` set a cookie for `domain`? A server may widen to a parent
/// domain it belongs to, but never to a public suffix — `Domain=.com` would
/// hand the cookie to every site on the internet.
///
/// The real rule needs the Public Suffix List. This is the crude stand-in:
/// the domain must contain a dot and must not be the bare two-label tail of
/// a well-known multi-label suffix. It errs toward REFUSING, which costs a
/// cookie; erring the other way costs the session.
fn domain_ok(host: &str, domain: &str) -> bool {
    if domain.is_empty() || !domain.contains('.') || domain.starts_with('.') {
        return false;
    }
    if !domain_match(host, domain) {
        return false;
    }
    let labels = domain.split('.').count();
    if labels < 2 {
        return false;
    }
    // `co.uk`, `com.au`, `co.jp`… — two labels where the first is one of the
    // usual second-level registry names is a suffix, not a site.
    const REGISTRY_2LD: &[&str] = &["co", "com", "net", "org", "gov", "edu", "ac"];
    if labels == 2 {
        let first = domain.split('.').next().unwrap_or("");
        let tail = domain.rsplit('.').next().unwrap_or("");
        if tail.len() == 2 && REGISTRY_2LD.contains(&first) {
            return false;
        }
    }
    true
}

/// Parse the `Expires` date, into seconds since the epoch.
///
/// TWO spellings are in live use and a jar has to take both — measured
/// 2026-08-09 against six real sites:
/// * `Wdy, DD Mon YYYY HH:MM:SS GMT` — RFC 7231 IMF-fixdate (Wikipedia,
///   GitHub)
/// * `Wdy, DD-Mon-YYYY HH:MM:SS GMT` — the old Netscape cookie date, which
///   RFC 6265 §5.1.1 requires a parser to accept (Google, Amazon)
///
/// Reading only the first spelling turned every Google and Amazon cookie
/// into a session cookie — and, worse, would have ignored a server logging
/// you out with an expiry in the past, so we would have kept sending a
/// cookie we were told to drop.
///
/// Anything unparseable returns `None` → the cookie is treated as a session
/// cookie. That is the safe direction: it lives no longer than beak does.
fn parse_http_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let rest = s.split_once(',').map(|(_, r)| r).unwrap_or(s).trim();
    let mut it = rest.split(['-', ' ', '\t']).filter(|t| !t.is_empty());
    let day: i64 = it.next()?.parse().ok()?;
    let mon = it.next()?;
    let year: i64 = it.next()?.parse().ok()?;
    let time = it.next().unwrap_or("00:00:00");
    let mon = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|m| m.eq_ignore_ascii_case(mon))? as i64
        + 1;
    let mut t = time.split(':');
    let (h, mi, sec): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next().unwrap_or("0").parse().ok()?,
        t.next().unwrap_or("0").parse().ok()?,
    );
    // Howard Hinnant's days_from_civil — the same algorithm loft uses to show
    // an npkFS mtime, run the other way.
    let y = if mon <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mon + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec)
}

impl Jar {
/// Take everything a response said with `Set-Cookie` and update the jar.
/// `url` is the URL the response came from (after redirects), `headers` the
/// raw response header block.
pub fn store(&mut self, url: &str, headers: &str, now: i64) {
    let (host, path, _) = split_url(url);
    if host.is_empty() {
        return;
    }
    for line in headers.split('\n') {
        let line = line.trim_end_matches('\r');
        let Some((name, value)) = line.split_once(':') else { continue };
        if !name.trim().eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        self.store_one(&host, &path, value.trim(), now);
    }
}

fn store_one(&mut self, host: &str, req_path: &str, decl: &str, now: i64) {
    let mut parts = decl.split(';');
    let Some(pair) = parts.next() else { return };
    let Some((name, value)) = pair.split_once('=') else { return };
    let (name, value) = (name.trim(), value.trim());
    if name.is_empty() {
        return;
    }

    let mut domain = String::new();
    let mut path = String::new();
    let mut secure = false;
    let mut expires: Option<i64> = None;
    let mut max_age: Option<i64> = None;
    for attr in parts {
        let (k, v) = match attr.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (attr.trim(), ""),
        };
        if k.eq_ignore_ascii_case("domain") {
            domain = v.trim_start_matches('.').to_ascii_lowercase();
        } else if k.eq_ignore_ascii_case("path") && v.starts_with('/') {
            path = v.to_string();
        } else if k.eq_ignore_ascii_case("secure") {
            secure = true;
        } else if k.eq_ignore_ascii_case("expires") {
            expires = parse_http_date(v);
        } else if k.eq_ignore_ascii_case("max-age") {
            max_age = v.parse::<i64>().ok();
        }
    }

    // Max-Age wins over Expires (RFC 6265 §5.2.2) and is relative, so it
    // needs no agreement with the server about what time it is.
    let expiry = match max_age {
        Some(secs) => Some(now.saturating_add(secs)),
        None => expires,
    };

    // Cookie name prefixes (RFC 6265bis §4.1.3). These are a promise the NAME
    // itself carries, so a server that breaks the promise gets nothing —
    // otherwise `__Host-session` means nothing, and meaning nothing is worse
    // than not existing. Google already ships `__Secure-ENID`.
    if name.starts_with("__Secure-") && !secure {
        return;
    }
    // `__Host-` demands the Path=/ ATTRIBUTE, not merely a path that happens
    // to be "/": with no attribute the default path is the request's own
    // directory, which is not the promise the name makes.
    if name.starts_with("__Host-") && (!secure || !domain.is_empty() || path != "/") {
        return;
    }

    let host_only = domain.is_empty();
    let domain = if host_only {
        host.to_string()
    } else if domain_ok(host, &domain) {
        domain
    } else {
        // A server reaching for a domain it does not own gets nothing.
        return;
    };
    let path = if path.is_empty() { default_path(req_path) } else { path };

    // Replacing on (name, domain, path) is what makes deletion work: a server
    // logs you out by re-sending the same cookie with an expiry in the past.
    let jar = &mut self.cookies;
    jar.retain(|c| !(c.name == name && c.domain == domain && c.path == path));
    if expiry.is_some_and(|e| e <= now) {
        return; // an already-expired cookie IS the delete
    }
    if jar.len() >= MAX_COOKIES {
        jar.remove(0);
    }
    jar.push(Cookie {
        name: name.to_string(),
        value: value.to_string(),
        domain,
        path,
        host_only,
        secure,
        expires: expiry,
    });
}

/// The `Cookie:` header value for `url`, or an empty string if the jar has
/// nothing for it. Longer paths first (RFC 6265 §5.4).
pub fn header_for(&mut self, url: &str, now: i64) -> String {
    let (host, path, secure_req) = split_url(url);
    if host.is_empty() {
        return String::new();
    }
    let jar = &mut self.cookies;
    jar.retain(|c| !c.expires.is_some_and(|e| e <= now));

    let mut hits: Vec<&Cookie> = jar
        .iter()
        .filter(|c| {
            (if c.host_only { host == c.domain } else { domain_match(&host, &c.domain) })
                && path_match(&path, &c.path)
                && (!c.secure || secure_req)
        })
        .collect();
    hits.sort_by(|a, b| b.path.len().cmp(&a.path.len()));

    let mut out = String::new();
    for c in hits {
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(&c.name);
        out.push('=');
        out.push_str(&c.value);
    }
    out
}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test owns its jar — the harness runs them in parallel, and a
    /// shared global would make every one of these a race rather than a test.
    fn jar() -> Jar {
        Jar::default()
    }

    #[test]
    fn a_session_cookie_comes_back_on_the_next_request() {
        let mut j = jar();
        j.store("https://example.com/login", "set-cookie: sid=abc; Path=/\r\n", 1000);
        assert_eq!(j.header_for("https://example.com/account", 1000), "sid=abc");
    }

    /// The logout idiom: the same cookie re-sent with an expiry in the past.
    #[test]
    fn an_expired_cookie_deletes_the_one_it_names() {
        let mut j = jar();
        j.store("https://example.com/", "set-cookie: sid=abc\r\n", 1000);
        j.store("https://example.com/", "set-cookie: sid=; Max-Age=0\r\n", 1000);
        assert_eq!(j.header_for("https://example.com/", 1000), "");
    }

    #[test]
    fn a_cookie_never_leaks_to_a_neighbouring_site() {
        let mut j = jar();
        j.store("https://example.com/", "set-cookie: sid=abc\r\n", 1000);
        assert_eq!(j.header_for("https://evil-example.com/", 1000), "");
        assert_eq!(j.header_for("https://other.com/", 1000), "");
        // Host-only: set without a Domain, so not even a sub-domain gets it.
        assert_eq!(j.header_for("https://sub.example.com/", 1000), "");
    }

    #[test]
    fn a_domain_cookie_reaches_sub_domains_but_a_server_cannot_widen_past_its_site() {
        let mut j = jar();
        j.store("https://www.example.com/", "set-cookie: a=1; Domain=example.com\r\n", 1000);
        assert_eq!(j.header_for("https://api.example.com/", 1000), "a=1");
        // Reaching for a public suffix gets nothing at all.
        j.store("https://www.example.com/", "set-cookie: b=2; Domain=com\r\n", 1000);
        j.store("https://www.example.co.uk/", "set-cookie: c=3; Domain=co.uk\r\n", 1000);
        assert!(!j.header_for("https://other.com/", 1000).contains("b=2"));
        assert!(!j.header_for("https://other.co.uk/", 1000).contains("c=3"));
    }

    #[test]
    fn path_scoping_follows_the_directory_not_the_document() {
        let mut j = jar();
        j.store("https://example.com/app/login", "set-cookie: s=1\r\n", 1000);
        assert_eq!(j.header_for("https://example.com/app/account", 1000), "s=1");
        assert_eq!(j.header_for("https://example.com/other", 1000), "");
    }

    #[test]
    fn a_secure_cookie_never_rides_a_plain_request() {
        let mut j = jar();
        j.store("https://example.com/", "set-cookie: s=1; Secure\r\n", 1000);
        assert_eq!(j.header_for("http://example.com/", 1000), "");
        assert_eq!(j.header_for("https://example.com/", 1000), "s=1");
    }

    #[test]
    fn an_absolute_expiry_is_honoured_and_an_unparseable_one_is_a_session_cookie() {
        let mut j = jar();
        // 2030-01-01T00:00:00Z = 1893456000.
        j.store("https://example.com/", "set-cookie: a=1; Expires=Tue, 01 Jan 2030 00:00:00 GMT\r\n", 1000);
        assert_eq!(super::parse_http_date("Tue, 01 Jan 2030 00:00:00 GMT"), Some(1_893_456_000));
        assert_eq!(j.header_for("https://example.com/", 1_000_000), "a=1");
        assert_eq!(j.header_for("https://example.com/", 2_000_000_000), "");
        let mut j = jar();
        j.store("https://example.com/", "set-cookie: b=2; Expires=whenever\r\n", 1000);
        assert_eq!(j.header_for("https://example.com/", 2_000_000_000), "b=2");
    }

    /// Header blocks copied verbatim from live responses on 2026-08-09. The
    /// synthetic tests above all used one date spelling; these caught that two
    /// are in use, and that reading only one silently turned every Google and
    /// Amazon cookie into a session cookie.
    #[test]
    fn real_sites_headers_land_in_the_jar_as_they_should() {
        // Google: Netscape date (dashes), Domain=.google.com, a __Secure- name.
        let mut j = jar();
        j.store("https://www.google.com/", concat!(
            "set-cookie: SOCS=CAAaBgiAht_TBg; expires=Wed, 08-Sep-2027 17:02:09 GMT; path=/; domain=.google.com; Secure; SameSite=lax\r\n",
            "set-cookie: AEC=AdJVEas; expires=Fri, 05-Feb-2027 17:02:09 GMT; path=/; domain=.google.com; Secure; HttpOnly; SameSite=lax\r\n",
            "set-cookie: __Secure-ENID=35.SE=Jkbdc; expires=Thu, 09-Sep-2027 09:20:27 GMT; path=/; domain=.google.com; Secure; HttpOnly; SameSite=lax\r\n",
        ), 1_786_294_929);
        let h = j.header_for("https://www.google.com/search?q=x", 1_786_294_929);
        assert!(h.contains("SOCS=CAAaBgiAht_TBg"), "{h}");
        assert!(h.contains("__Secure-ENID=35.SE=Jkbdc"), "{h}");
        // Domain=.google.com reaches a sub-domain, but never a neighbour.
        assert!(j.header_for("https://news.google.com/", 1_786_294_929).contains("SOCS="));
        assert_eq!(j.header_for("https://google.com.evil.test/", 1_786_294_929), "");
        // A dated cookie must OUTLIVE the session — that is the whole point
        // of the date, and the dash spelling is what got it wrong.
        assert!(!j.header_for("https://www.google.com/", 1_800_000_000).is_empty());
        // …and still expire when it says. 2028-01-01 = 1830297600. This one
        // goes LAST: reading the jar prunes what has expired, so a test that
        // then asks about an earlier moment is asking a jar that already
        // threw those cookies away. Time only moves forward in a browser.
        assert_eq!(j.header_for("https://www.google.com/", 1_830_297_600), "");

        // Wikipedia: no space after the semicolons, lowercase `secure`,
        // RFC 1123 date, one host-only and one Domain cookie side by side.
        let mut j = jar();
        j.store("https://de.wikipedia.org/wiki/Schweiz", concat!(
            "set-cookie: WMF-Last-Access=09-Aug-2026;Path=/;HttpOnly;secure;Expires=Thu, 10 Sep 2026 12:00:00 GMT\r\n",
            "set-cookie: WMF-Last-Access-Global=09-Aug-2026;Path=/;Domain=.wikipedia.org;HttpOnly;secure;Expires=Thu, 10 Sep 2026 12:00:00 GMT\r\n",
            "set-cookie: GeoIP=CH:ZH:Zweidlen-Dorf:47.56:8.47:v4; Path=/; secure; Domain=.wikipedia.org\r\n",
        ), 1_786_294_929);
        let h = j.header_for("https://de.wikipedia.org/wiki/X", 1_786_294_929);
        assert!(h.contains("WMF-Last-Access=09-Aug-2026"), "{h}");
        assert!(h.contains("GeoIP=CH:ZH:Zweidlen-Dorf:47.56:8.47:v4"), "a value may hold colons: {h}");
        // The host-only one does NOT cross to another wikipedia sub-domain;
        // the Domain= ones do.
        let h2 = j.header_for("https://en.wikipedia.org/wiki/X", 1_786_294_929);
        assert!(!h2.contains("WMF-Last-Access="), "{h2}");
        assert!(h2.contains("WMF-Last-Access-Global="), "{h2}");

        // GitHub's login page: the session cookie has no date at all, which is
        // exactly what a login cookie looks like.
        let mut j = jar();
        j.store("https://github.com/login", concat!(
            "set-cookie: _gh_sess=xQWvHNb1oK7%2Fiz; path=/; HttpOnly; secure; SameSite=Lax\r\n",
            "set-cookie: logged_in=no; expires=Mon, 09 Aug 2027 17:02:09 GMT; domain=.github.com; path=/; HttpOnly; secure; SameSite=Lax\r\n",
        ), 1_786_294_929);
        assert!(j.header_for("https://github.com/", 1_786_294_929).contains("_gh_sess=xQWvHNb1oK7%2Fiz"));

        // Amazon: Netscape dates again, everything on .amazon.de.
        let mut j = jar();
        j.store("https://www.amazon.de/", concat!(
            "set-cookie: session-id=258-2923272-2531756; Domain=.amazon.de; Expires=Mon, 09-Aug-2027 17:02:10 GMT; Path=/; Secure\r\n",
            "set-cookie: i18n-prefs=CHF; Domain=.amazon.de; Expires=Mon, 09-Aug-2027 17:02:10 GMT; Path=/\r\n",
        ), 1_786_294_929);
        let h = j.header_for("https://www.amazon.de/gp/cart", 1_786_294_929);
        assert!(h.contains("session-id=258-2923272-2531756"), "{h}");
        assert!(h.contains("i18n-prefs=CHF"), "{h}");
    }

    /// A name that promises something has to keep it, or it means nothing
    /// (RFC 6265bis §4.1.3).
    #[test]
    fn a_prefixed_name_that_breaks_its_promise_is_refused() {
        let mut j = jar();
        j.store("https://example.com/", "set-cookie: __Secure-a=1\r\n", 1000);
        j.store("https://example.com/app/x", "set-cookie: __Host-b=2; Secure\r\n", 1000);
        j.store("https://example.com/", "set-cookie: __Host-c=3; Secure; Domain=example.com; Path=/\r\n", 1000);
        assert_eq!(j.header_for("https://example.com/app/x", 1000), "");
        // Kept when the promise holds.
        j.store("https://example.com/", "set-cookie: __Host-d=4; Secure; Path=/\r\n", 1000);
        assert_eq!(j.header_for("https://example.com/", 1000), "__Host-d=4");
    }

    #[test]
    fn a_port_does_not_split_the_jar() {
        let mut j = jar();
        j.store("https://example.com:8443/", "set-cookie: s=1\r\n", 1000);
        assert_eq!(j.header_for("https://example.com/", 1000), "s=1");
    }
}
