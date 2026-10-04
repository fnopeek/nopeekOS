# `tools/wasm/beak-engine/src/cookies.rs` @ 5e0102684

## L1-15 · `extern crate alloc;`

```
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
```

## L21-23 · `struct Cookie {`

```
/// One stored cookie. `domain` is stored without a leading dot; `host_only`
/// records whether the server named a `Domain` at all, because a host-only
/// cookie must NOT be sent to sub-domains.
```

## L31-33 · `http_only: bool,`

```
/// `HttpOnly`: der Keks geht auf die Anfrage, aber NICHT an ein Skript.
/// Der ganze Sinn der Fahne — sie ist die Gegenmassnahme gegen XSS, und
/// sie zaehlt erst, seit beak Seitenskripte laufen laesst.
```

## L35-36 · `expires: Option<i64>,`

```
/// Absolute expiry, seconds since the epoch. `None` = session cookie,
/// which for this jar means "until beak quits".
```

## L40-42 · `const MAX_COOKIES: usize = 256;`

```
/// A jar big enough for real browsing. Past the cap the oldest entry is
/// dropped rather than the newest refused, so a site that sets a tracking
/// cookie per page view cannot lock out the session cookie you need.
```

## L45-46 · `#[derive(Default)]`

```
/// A set of cookies. The browser holds one; a test holds its own, which is
/// what keeps the domain-matching rules testable without a shared global.
```

## L54 · `fn global() -> &'static mut Jar {`

```
/// The browser's jar. One per process, created on first use.
```

## L65 · `pub fn store(url: &str, headers: &str, now: i64) {`

```
/// File everything a response said with `Set-Cookie`, into the browser's jar.
```

## L70 · `pub fn header_for(url: &str, now: i64) -> String {`

```
/// The `Cookie:` header value for `url` from the browser's jar.
```

## L75-76 · `pub fn script_header_for(url: &str, now: i64) -> String {`

```
/// The `document.cookie` value for `url` from the browser's jar — without
/// the `HttpOnly` ones.
```

## L81 · `pub fn store_from_script(url: &str, decl: &str, now: i64) {`

```
/// File one `document.cookie = "..."` against the browser's jar.
```

## L86-91 · `pub fn names_for(url: &str, now: i64) -> String {`

```
/// Die NAMEN der Kekse, die fuer diese Adresse mitgehen — ohne Werte.
///
/// **Ein Keks ist ein Geheimnis, sein Name ist es nicht.** Und ohne die Namen
/// ist „5 held" keine Auskunft: Googles Einwilligung schickte im Kreis, und
/// aus dem Log war nicht zu sehen, ob `SOCS` ueberhaupt mitging. Genau die
/// Frage, die eine Zeile beantwortet und ein Nachmittag nicht.
```

## L102-104 · `pub fn names_held(host_url: &str) -> String {`

```
/// Was der Behaelter fuer diesen Host haelt, nach Namen — auch die, die
/// gerade NICHT mitgehen (falscher Pfad, `Secure` auf http, abgelaufen).
/// Der Unterschied zu `names_for` ist die halbe Diagnose.
```

## L119 · `pub fn count() -> usize {`

```
/// How many cookies are held, for the diagnostic line.
```

## L124-125 · `fn split_url(url: &str) -> (String, String, bool) {`

```
/// Split a URL into (host, path, is_https). Accepts what beak's address bar
/// produces: `https://host/path`, or a bare `host/path` (https assumed).
```

## L136 · `let host = host.split(':').next().unwrap_or("").to_string();`

```
// Strip a port: cookies do not distinguish them (RFC 6265 §8.5).
```

## L146-148 · `fn default_path(path: &str) -> String {`

```
/// The default path of a cookie set from `path` (RFC 6265 §5.1.4): the
/// directory, not the document. Without this a cookie set at `/login` would
/// never be sent to `/account`.
```

## L156-158 · `fn domain_match(host: &str, domain: &str) -> bool {`

```
/// Does `host` fall under `domain` (RFC 6265 §5.1.3)? Either identical, or a
/// sub-domain — and the boundary must be a dot, so `evil-example.com` does
/// not match `example.com`.
```

## L166 · `fn path_match(req: &str, path: &str) -> bool {`

```
/// Does `req` fall under the cookie's `path` (RFC 6265 §5.1.4)?
```

## L177-184 · `fn domain_ok(host: &str, domain: &str) -> bool {`

```
/// May `host` set a cookie for `domain`? A server may widen to a parent
/// domain it belongs to, but never to a public suffix — `Domain=.com` would
/// hand the cookie to every site on the internet.
///
/// The real rule needs the Public Suffix List. This is the crude stand-in:
/// the domain must contain a dot and must not be the bare two-label tail of
/// a well-known multi-label suffix. It errs toward REFUSING, which costs a
/// cookie; erring the other way costs the session.
```

## L196-197 · `const REGISTRY_2LD: &[&str] = &["co", "com", "net", "org", "gov", "edu", "ac"];`

```
// `co.uk`, `com.au`, `co.jp`… — two labels where the first is one of the
// usual second-level registry names is a suffix, not a site.
```

## L209-224 · `fn parse_http_date(s: &str) -> Option<i64> {`

```
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
```

## L243-244 · `let y = if mon <= 2 { year - 1 } else { year };`

```
// Howard Hinnant's days_from_civil — the same algorithm loft uses to show
// an npkFS mtime, run the other way.
```

## L256-268 · `pub fn store(&mut self, url: &str, headers: &str, now: i64) {`

```
/// Take everything a response said with `Set-Cookie` and update the jar.
///
/// `headers` may cover a whole redirect CHAIN: the host writes a `:hop <url>`
/// marker before each response's block. That is not a detail — a login is a
/// POST answered by a 303 that carries the session cookie, and the cookie
/// belongs to the host that SENT it, not to wherever the chain ended.
/// `url` is the origin for a block that carries no marker.
///
/// ⚠ The marker is trusted, and it may be trusted for exactly one reason:
/// no HTTP field name may begin with a colon, and the host DROPS any
/// response line that does (`capture_headers`). A server that could write
/// its own `:hop` would file its cookies against a host it does not own.
/// Whoever changes either side owns both.
```

## L327-328 · `let expiry = match max_age {`

```
// Max-Age wins over Expires (RFC 6265 §5.2.2) and is relative, so it
// needs no agreement with the server about what time it is.
```

## L334-337 · `if name.starts_with("__Secure-") && !secure {`

```
// Cookie name prefixes (RFC 6265bis §4.1.3). These are a promise the NAME
// itself carries, so a server that breaks the promise gets nothing —
// otherwise `__Host-session` means nothing, and meaning nothing is worse
// than not existing. Google already ships `__Secure-ENID`.
```

## L341-343 · `if name.starts_with("__Host-") && (!secure || !domain.is_empty() || path != "/") {`

```
// `__Host-` demands the Path=/ ATTRIBUTE, not merely a path that happens
// to be "/": with no attribute the default path is the request's own
// directory, which is not the promise the name makes.
```

## L348-349 · `if from_script {`

```
// Ein Skript kann `HttpOnly` nicht vergeben — sonst waere die Fahne ein
// Selbstbedienungsladen und schuetzte nichts (RFC 6265bis 5.7).
```

## L360 · `return;`

```
// A server reaching for a domain it does not own gets nothing.
```

## L365-366 · `let jar = &mut self.cookies;`

```
// Replacing on (name, domain, path) is what makes deletion work: a server
// logs you out by re-sending the same cookie with an expiry in the past.
```

## L368-371 · `if from_script && jar.iter().any(|c| c.name == name && c.domain == domain`

```
// Und es kann einen HttpOnly-Keks auch nicht UEBERSCHREIBEN. Ohne diese
// Zeile waere die Fahne zu umgehen: erst den Keks mit eigenem Wert neu
// setzen, dann zurueckzulesen, was man selbst geschrieben hat — das ist
// kein Leck mehr, aber es ist eine Uebernahme der Sitzung.
```

## L378 · `return; // an already-expired cookie IS the delete`

```
// an already-expired cookie IS the delete
```

## L395-396 · `pub fn header_for(&mut self, url: &str, now: i64) -> String {`

```
/// The `Cookie:` header value for `url`, or an empty string if the jar has
/// nothing for it. Longer paths first (RFC 6265 §5.4).
```

## L401-403 · `pub fn script_header_for(&mut self, url: &str, now: i64) -> String {`

```
/// Was `document.cookie` einem Skript zeigt: dasselbe, ohne die
/// `HttpOnly`-Kekse. Getrennte Funktion und nicht ein Argument an
/// `header_for`, damit an jeder Aufrufstelle STEHT, wer fragt.
```

## L408-411 · `pub fn store_from_script(&mut self, url: &str, decl: &str, now: i64) {`

```
/// Was ein Skript mit `document.cookie = "..."` gesetzt hat. Eine einzelne
/// Erklaerung, ohne `Set-Cookie:` davor — genau das, was die Zuweisung
/// uebergibt. Die Regeln sind dieselben wie fuer den Server, mit zwei
/// Ausnahmen, die in `store_one` stehen.
```

## L452-465 · `const FILE_TAG: &str = "npkcookies 1";`

```
// ── Ueber einen Neustart hinweg ───────────────────────────────────────────
//
// **Nur die DAUERHAFTEN.** Ein Keks ohne `Expires`/`Max-Age` ist ein
// Sitzungskeks, und „Sitzung" heisst: bis der Browser endet. Ihn zu
// speichern waere nicht bequemer, sondern falsch — die Seite hat
// ausdruecklich gesagt, dass er nicht bleiben soll, und bei einem
// Anmeldekeks ist das eine Sicherheitsaussage.
//
// Das Format ist eine Zeile je Keks, Felder durch Tabulator getrennt. Ein
// Kekswert DARF laut RFC 6265 §4.1.1 weder Steuerzeichen noch Komma,
// Semikolon, Anfuehrungszeichen oder Rueckstrich enthalten, ein Tabulator
// ist also nie darin — und eine Zeile, in der doch einer steckt, wird
// UEBERSPRUNGEN statt geschrieben. Eine kaputte Zeile darf die Datei nicht
// unlesbar machen.
```

## L469 · `pub fn serialize(&self, now: i64) -> String {`

```
/// Die dauerhaften Kekse als Text. Leer, wenn keiner bleiben soll.
```

## L473 · `let Some(exp) = c.expires else { continue };   // Sitzungskeks`

```
// Sitzungskeks
```

## L474 · `if exp <= now { continue }                     // schon abgelaufen`

```
// schon abgelaufen
```

## L485-489 · `pub fn load(&mut self, text: &str, now: i64) -> usize {`

```
/// Zurueckgelesene Kekse einfuegen. Liefert, wie viele ankamen.
///
/// Abgelaufene und unlesbare Zeilen werden still uebergangen: die Datei
/// ist Zustand, kein Vertrag, und eine halbe Zeile darf nicht den Rest
/// kosten.
```

## L513 · `pub fn serialize(now: i64) -> String { global().serialize(now) }`

```
/// Die dauerhaften Kekse des Browsers als Text.
```

## L516 · `pub fn load(text: &str, now: i64) -> usize { global().load(text, now) }`

```
/// Gespeicherte Kekse ins Glas des Browsers.
```

## L523-524 · `fn jar() -> Jar {`

```
/// Each test owns its jar — the harness runs them in parallel, and a
/// shared global would make every one of these a race rather than a test.
```

## L536 · `#[test]`

```
/// The logout idiom: the same cookie re-sent with an expiry in the past.
```

## L545 · `#[test]`

```
/// Die Fahne, wegen der es `script_header_for` ueberhaupt gibt.
```

## L558 · `assert_eq!(j.script_header_for("https://example.com/", 1000), "sid=abc");`

```
// Gesetzt ist er — aber ohne die Fahne, also sieht das Skript ihn.
```

## L562-563 · `#[test]`

```
/// Der Angriff, den die vorige Regel allein offen liesse: den Keks nicht
/// lesen, sondern ueberschreiben und dann das Eigene zurueckholen.
```

## L579 · `assert_eq!(j.header_for("https://sub.example.com/", 1000), "");`

```
// Host-only: set without a Domain, so not even a sub-domain gets it.
```

## L588 · `j.store("https://www.example.com/", "set-cookie: b=2; Domain=com\r\n", 1000);`

```
// Reaching for a public suffix gets nothing at all.
```

## L614 · `j.store("https://example.com/", "set-cookie: a=1; Expires=Tue, 01 Jan 2030 00:00:00 GMT\r\n", 1000);`

```
// 2030-01-01T00:00:00Z = 1893456000.
```

## L624-627 · `#[test]`

```
/// Header blocks copied verbatim from live responses on 2026-08-09. The
/// synthetic tests above all used one date spelling; these caught that two
/// are in use, and that reading only one silently turned every Google and
/// Amazon cookie into a session cookie.
```

## L630 · `let mut j = jar();`

```
// Google: Netscape date (dashes), Domain=.google.com, a __Secure- name.
```

## L640 · `assert!(j.header_for("https://news.google.com/", 1_786_294_929).contains("SOCS="));`

```
// Domain=.google.com reaches a sub-domain, but never a neighbour.
```

## L643-644 · `assert!(!j.header_for("https://www.google.com/", 1_800_000_000).is_empty());`

```
// A dated cookie must OUTLIVE the session — that is the whole point
// of the date, and the dash spelling is what got it wrong.
```

## L646-649 · `assert_eq!(j.header_for("https://www.google.com/", 1_830_297_600), "");`

```
// …and still expire when it says. 2028-01-01 = 1830297600. This one
// goes LAST: reading the jar prunes what has expired, so a test that
// then asks about an earlier moment is asking a jar that already
// threw those cookies away. Time only moves forward in a browser.
```

## L652-653 · `let mut j = jar();`

```
// Wikipedia: no space after the semicolons, lowercase `secure`,
// RFC 1123 date, one host-only and one Domain cookie side by side.
```

## L663-664 · `let h2 = j.header_for("https://en.wikipedia.org/wiki/X", 1_786_294_929);`

```
// The host-only one does NOT cross to another wikipedia sub-domain;
// the Domain= ones do.
```

## L669-670 · `let mut j = jar();`

```
// GitHub's login page: the session cookie has no date at all, which is
// exactly what a login cookie looks like.
```

## L678 · `let mut j = jar();`

```
// Amazon: Netscape dates again, everything on .amazon.de.
```

## L689-692 · `#[test]`

```
/// A login is a POST answered by a 303 that carries the session cookie.
/// Reading only the last response in the chain threw it away — Google's
/// consent page took the click, saved nothing, and sent you straight back
/// to itself.
```

## L706-707 · `assert!(j.header_for("https://www.google.com/search?q=x", 1_786_294_929).contains("SOCS="));`

```
// Set by consent.google.com with Domain=.google.com — so it rides the
// search request, which is the whole point of accepting it.
```

## L710-711 · `let mut j = jar();`

```
// A hop that sets a HOST-ONLY cookie scopes it to that hop's host and
// to no other — the marker is what makes the difference visible.
```

## L725-726 · `#[test]`

```
/// Headers with no marker at all — every sub-resource response, and any
/// caller that hands over a single block — still scope to the URL given.
```

## L735-736 · `#[test]`

```
/// A name that promises something has to keep it, or it means nothing
/// (RFC 6265bis §4.1.3).
```

## L744 · `j.store("https://example.com/", "set-cookie: __Host-d=4; Secure; Path=/\r\n", 1000);`

```
// Kept when the promise holds.
```

## L749-751 · `#[test]`

```
/// The exact bytes Google answers a consent POST with, captured
/// 2026-08-09 — the 303 that carries the decision. If this cookie does
/// not land, the consent page sends you straight back to itself, forever.
```

## L778-780 · `#[test]`

```
/// **Ein Sitzungskeks ueberlebt den Neustart NICHT.** Das ist keine
/// Sparsamkeit: die Seite hat gesagt, dass er nicht bleiben soll, und
/// bei einer Anmeldung ist das eine Sicherheitsaussage.
```

## L796-798 · `#[test]`

```
/// Hin und zurueck muss jede Fahne mitnehmen — `Secure` und `HttpOnly`
/// sind Grenzen, und eine Grenze, die beim Speichern verloren geht, ist
/// schlimmer als keine.
```

## L808 · `assert_eq!(fresh.header_for("http://example.com/app/", 1000), "");`

```
// Secure: nicht ueber http.
```

## L811 · `assert_eq!(fresh.header_for("https://example.com/", 1000), "");`

```
// Path: nicht ausserhalb.
```

## L813 · `assert_eq!(fresh.script_header_for("https://example.com/app/", 1000), "");`

```
// HttpOnly: nicht ans Skript.
```

## L817 · `#[test]`

```
/// Eine kaputte Zeile darf die Datei nicht kosten.
```

## L832 · `#[test]`

```
/// Abgelaufenes kommt nicht zurueck — weder beim Schreiben noch beim Lesen.
```

