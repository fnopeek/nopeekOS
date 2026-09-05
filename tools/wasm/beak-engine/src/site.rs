//! Herkunft und Site — die zwei Begriffe, auf denen jede Grenze im Web steht.
//!
//! `cookies.rs` sagte bis 0.101.0 im Kopf: `SameSite` sei nicht gebaut, weil
//! es „a notion of the initiating context" brauche, „which arrives with
//! scripting". Das Skripting ist da. Hier ist der Begriff.
//!
//! **Herkunft** (*origin*) = Schema + Host + Port. Zwei Dokumente derselben
//! Herkunft dürfen einander lesen.
//!
//! **Site** = Schema + *registrierbare Domain*. Gröber als die Herkunft:
//! `app.example.com` und `api.example.com` sind verschiedene Herkünfte, aber
//! dieselbe Site — und genau darauf beruht, dass eine Anwendung mit ihrer
//! eigenen API sprechen kann, ohne dass ein Keks zu Fremden fliesst.
//!
//! ## Warum die echte Liste eingebettet ist
//!
//! Die registrierbare Domain ist NICHT „die letzten zwei Bestandteile". Bei
//! `a.github.io` und `b.github.io` wären das beide `github.io` — zwei
//! fremde Nutzerseiten würden als dieselbe Site gelten und einander Kekse
//! schicken. Am Zielkorpus gemessen (691 Hosts) trifft das 7 Hosts, alle in
//! die gefährliche Richtung.
//!
//! Deshalb liegt die echte Public Suffix List daneben (10 321 Regeln,
//! 144 KB). Gegen 3,86 MB `beak.wasm` ist das nichts, und eine
//! Sicherheitsgrenze approximiert man nicht, wenn die genaue Antwort so
//! wenig kostet. Siehe `docs/plan/BROWSER_FETCH_ORIGIN.md` §5.3.

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Die Liste, sortiert und ohne Kommentare — sortiert, damit die Suche eine
/// Binärsuche ist und kein Durchlauf über 10 780 Zeilen je Keks.
///
/// **So entsteht sie neu** (die Liste ändert sich, also gehört das
/// aufgeschrieben statt erraten):
///
/// 1. `curl -O https://publicsuffix.org/list/public_suffix_list.dat`
/// 2. Kommentare (`//`) und Leerzeilen weg.
/// 3. **Jede Nicht-ASCII-Regel bekommt ihre Punycode-Form DAZU** — die
///    Originalliste führt `公司.cn` nur in Unicode, ein Host aus einer URL
///    ist aber Punycode. Ohne diesen Schritt greift keine einzige
///    IDN-Endung, und zwei fremde Seiten darunter gelten als dieselbe Site.
///    Das offizielle Testorakel hat genau das gefunden; meine eigenen
///    Proben nicht.
/// 4. Sortieren und entdoppeln (`sorted(set(...))`).
///
/// Die Vektoren in `psl_vectors.txt` kommen aus `tests/test_psl.txt`
/// desselben Projekts, ASCII-normalisiert — **auskommentierte Zeilen sind
/// keine Vektoren**, auch das war ein Fehler des ersten Entwurfs.
const PSL: &str = include_str!("public_suffix_list.dat");

/// Eine Herkunft: Schema, Host, Port. Der Port ist ausgerechnet, nicht
/// geraten — `https://a.de` und `https://a.de:443` sind dieselbe Herkunft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl Origin {
    /// Die Textform, wie sie in einen `Origin:`-Kopf gehört. Der
    /// Vorgabeport steht NICHT drin — sonst passt sie nicht auf das, was
    /// ein Server in `Access-Control-Allow-Origin` zurückschreibt.
    pub fn header(&self) -> String {
        let default = if self.scheme == "https" { 443 } else { 80 };
        if self.port == default {
            alloc::format!("{}://{}", self.scheme, self.host)
        } else {
            alloc::format!("{}://{}:{}", self.scheme, self.host, self.port)
        }
    }
}

/// Die Herkunft einer Adresse. `None`, wenn die Adresse keine hat —
/// `about:`, `data:`, `beak:selftest` und alles andere ohne Autorität.
///
/// **Ein Dokument ohne Herkunft bekommt keine Kekse und darf nichts holen.**
/// Das ist kein Sonderfall, den man wegdrückt: `beak:selftest` hat wirklich
/// keine, und die Trennung wurde am Gerät schon einmal sichtbar
/// (`cookies: Seite setzte 2, 0 held`).
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
    // Ein Nutzer-Teil (`user@host`) gehört nicht zur Herkunft.
    let hostport = match hostport.rfind('@') {
        Some(i) => &hostport[i + 1..],
        None => hostport,
    };
    // IPv6 steht in eckigen Klammern und enthält selbst Doppelpunkte.
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
        // Der Host ist ohne Rücksicht auf Gross/Klein zu vergleichen; der
        // Pfad NICHT. Genau hier wird es entschieden, damit es nirgends
        // sonst nochmal getan werden muss.
        host: host.to_ascii_lowercase(),
        port,
    })
}

/// Die **registrierbare Domain** eines Hosts — ein Bestandteil mehr als das
/// öffentliche Suffix (ES: „eTLD+1").
///
/// `www.bbc.co.uk` -> `bbc.co.uk` · `a.github.io` -> `a.github.io` ·
/// `example.com` -> `example.com` · `co.uk` -> `None` (ein Suffix allein ist
/// keine registrierbare Domain, und ein Keks darauf gehört niemandem).
///
/// Eine IP-Adresse hat keine: sie IST schon die kleinste Einheit.
pub fn registrable_domain(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || is_ip_literal(&host) {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    // Ein LEERER Bestandteil heisst: das ist kein Host. `.example.com` sieht
    // wie einer aus und ist keiner — die offiziellen Vektoren prüfen genau
    // das, und der erste Entwurf hat brav `example.com` daraus gemacht.
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {
        return None;
    }
    // Die längste passende Regel gewinnt (PSL-Algorithmus). Gesucht wird von
    // der längsten Kandidatenform abwärts, damit die erste Übereinstimmung
    // schon die längste ist.
    let mut best: Option<usize> = None; // Anzahl Bestandteile des Suffix
    let mut exception = false;
    for start in 0..labels.len() {
        let cand = labels[start..].join(".");
        let n = labels.len() - start;
        // Ausnahmeregel (`!city.kawasaki.jp`) sticht alles: das Suffix ist
        // dann EIN Bestandteil kürzer als die Regel.
        if psl_has(&alloc::format!("!{cand}")) {
            best = Some(n - 1);
            exception = true;
            break;
        }
        if psl_has(&cand) {
            best = Some(n);
            break;
        }
        // Platzhalterregel (`*.ck`): trifft, wenn der REST danach passt.
        if start + 1 <= labels.len() {
            let parent = labels[start + 1..].join(".");
            if !parent.is_empty() && psl_has(&alloc::format!("*.{parent}")) {
                best = Some(n);
                break;
            }
        }
    }
    // Keine Regel getroffen: die Vorgabe der PSL ist „`*`", also ist das
    // letzte Bestandteil das Suffix. Das ist der Normalfall (`example.com`).
    let suffix_labels = best.unwrap_or(1);
    let _ = exception;
    if suffix_labels >= labels.len() {
        // Der Host IST ein öffentliches Suffix (`co.uk`, `github.io`).
        // Darauf gibt es keine registrierbare Domain — und damit auch
        // keinen Keks, der irgendwem gehört.
        return None;
    }
    Some(labels[labels.len() - suffix_labels - 1..].join("."))
}

/// Gehören zwei Hosts zur selben Site?
///
/// **Das ist die Frage hinter `SameSite`**, und sie ist gröber als die
/// Herkunft: `app.x.de` und `api.x.de` — verschiedene Herkünfte, dieselbe
/// Site. Zwei Hosts ohne registrierbare Domain (IP-Adressen) sind dieselbe
/// Site, wenn sie derselbe Host sind, sonst nicht.
pub fn same_site(a: &str, b: &str) -> bool {
    match (registrable_domain(a), registrable_domain(b)) {
        (Some(x), Some(y)) => x == y,
        // Kein Suffix-Wissen anwendbar (IP, `localhost`): dann zählt der
        // Host selbst. Erben tut hier niemand etwas.
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// Dieselbe Site UND dasselbe Schema — das ist, was `SameSite` wirklich
/// meint (*schemeful same-site*). `http://x.de` und `https://x.de` gelten
/// als verschieden, sonst hebelt ein Klartext-Zwischenstück die Regel aus.
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

/// Binärsuche in der sortierten Liste. Die Liste ist EIN Block mit
/// Zeilenumbrüchen; ein `Vec` daraus zu bauen hiesse, 10 321 Scheiben beim
/// Start anzulegen, und gesucht wird selten genug, dass die Suche über die
/// Zeilen billiger ist als das Aufbauen.
///
/// **Gesucht wird auf BYTES, nicht auf `str`.** Die Liste enthält die
/// Unicode-Schreibweise internationalisierter Endungen; eine Halbierung
/// landet dort mitten in einem Zeichen, und `&PSL[..mid]` bricht ab. Der
/// erste Entwurf tat genau das und ist im Test gestorben — auf einer
/// fremden Seite wäre es ein Absturz aus dem Nichts gewesen.
/// Byte-Vergleich ist hier ausserdem das Richtige und nicht bloss das
/// Sichere: die Liste ist byteweise sortiert, und ein Host aus einer URL
/// ist ASCII (Punycode).
fn psl_has(rule: &str) -> bool {
    let hay = PSL.as_bytes();
    let needle = rule.as_bytes();
    let (mut lo, mut hi) = (0usize, hay.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        // Auf den Anfang der Zeile zurückgehen, in der `mid` liegt.
        let start = hay[..mid].iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1);
        let end = hay[start..].iter().position(|&c| c == b'\n').map_or(hay.len(), |i| start + i);
        let line = &hay[start..end];
        match line.cmp(needle) {
            core::cmp::Ordering::Equal => return true,
            core::cmp::Ordering::Less => {
                // Die Zeile bei `mid` ist zu klein — hinter ihr weitersuchen.
                // Ohne den Fortschritt-Zwang stünde die Schleife still, wenn
                // `mid` immer wieder in dieselbe Zeile fällt.
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

    /// **Die offiziellen Vektoren der Public Suffix List selbst.**
    ///
    /// Meine eigenen Proben zu bestehen heisst wenig — ich habe sie
    /// geschrieben. Das hier ist ein Orakel, das jemand anders aufgestellt
    /// hat: `tests/test_psl.txt` aus dem PSL-Projekt, auf die ASCII-Fälle
    /// reduziert (ein Host aus einer URL ist Punycode).
    /// [[feedback-cross-check-the-probe-against-a-real-engine]]
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
        // Die Beispiele stehen so auf publicsuffix.org.
        assert_eq!(registrable_domain("com"), None);
        assert_eq!(registrable_domain("example.com").as_deref(), Some("example.com"));
        assert_eq!(registrable_domain("www.example.com").as_deref(), Some("example.com"));
        assert_eq!(registrable_domain("uk.com").as_deref(), None);
        assert_eq!(registrable_domain("example.uk.com").as_deref(), Some("example.uk.com"));
        assert_eq!(registrable_domain("a.b.example.uk.com").as_deref(), Some("example.uk.com"));
    }

    /// Genau die sieben Hosts, an denen „die letzten zwei Bestandteile"
    /// falsch gewesen wäre — gemessen am Zielkorpus, nicht ausgedacht.
    #[test]
    fn the_seven_hosts_the_corpus_measured() {
        assert_eq!(registrable_domain("bakkot.github.io").as_deref(), Some("bakkot.github.io"));
        assert_eq!(registrable_domain("tc39.github.io").as_deref(), Some("tc39.github.io"));
        assert_eq!(registrable_domain("pajhome.org.uk").as_deref(), Some("pajhome.org.uk"));
        assert_eq!(
            registrable_domain("github-cloud.s3.amazonaws.com").as_deref(),
            Some("github-cloud.s3.amazonaws.com")
        );
        // Und der Punkt der ganzen Übung: zwei fremde Nutzerseiten unter
        // derselben Endung sind NICHT dieselbe Site.
        assert!(!same_site("tc39.github.io", "bakkot.github.io"));
        assert!(!same_site("a.s3.amazonaws.com", "b.s3.amazonaws.com"));
    }

    /// Die IDN-Endungen müssen in PUNYCODE dastehen, nicht nur in Unicode —
    /// sonst greift keine von ihnen, und `a.公司.cn` wäre dieselbe Site wie
    /// `b.公司.cn`. Der Test bewacht Schritt 3 der Neuerzeugung.
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
        // Länderendungen mit zwei Teilen
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
        // Der Vorgabeport steht nicht im Kopf, ein anderer schon.
        assert_eq!(origin_of("https://x.de:443/").unwrap().header(), "https://x.de");
        assert_eq!(origin_of("https://x.de:8443/").unwrap().header(), "https://x.de:8443");
        assert_eq!(origin_of("http://x.de/").unwrap().header(), "http://x.de");
        // Gleiche Herkunft heisst: alle drei Teile gleich.
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
