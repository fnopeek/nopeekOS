# `kernel/src/intent/http.rs` @ 5e0102684

## L1 · `use crate::{kprint, kprintln, capability};`

```
//! HTTP/HTTPS intents
```

## L7 · `const HTTP_MAX_RESPONSE: usize = 128 * 1024; // 128 KB`

```
// 128 KB
```

## L9-16 · `static HTTP_QUIET: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);`

```
/// Per-request chatter — the connect breakdown and the HTTP status lines.
///
/// On by default: for `https <host>` the transfer IS what you asked about,
/// and the `dns + arp + tcp + tls` split is the tool that names a slow leg
/// (see the netbench work). But an intent that makes several requests just to
/// answer a question of its own — `update` fetches three manifests before it
/// can even say what changed — drowns its own output in them. Such a caller
/// takes a `quiet()` guard for its duration; errors still speak.
```

## L23-24 · `pub struct QuietGuard(bool);`

```
/// Silence per-request chatter until the returned guard is dropped — so an
/// early `return` in the middle of a fetch cannot leave the shell mute.
```

## L37-60 · `pub(crate) const USER_AGENT: &str = concat!(`

```
/// How we identify ourselves. Deliberately NOT a borrowed browser string.
///
/// This used to read `Mozilla/5.0 (X11; Linux x86_64) beak/0.1`, added in the
/// belief that the `Mozilla` prefix bought a friendlier rate-limit bucket at
/// Wikimedia. Measured on 2026-07-22, that was wrong twice over: the 429s came
/// from speaking HTTP/1.1, not from the name, and the same burst over h2 is
/// served in full with this honest string. Sending *no* User-Agent is not an
/// option either — that earns a 403, and Wikimedia's policy rightly asks for
/// an identifiable client.
///
/// One string covers OTA as well as page fetches, since both go through this
/// client. Splitting them would mean threading a parameter through every
/// layer for little gain -- which is also why the name here is the OS and not
/// `beak`: an OTA request does not come from the browser.
///
/// Two things the honest string still got wrong, both found on 2026-08-25 when
/// Wikimedia answered a test burst with 429 and a pointer to its robot policy:
///
/// * **It said `0.1` while beak stood at 0.35.3.** A version that is typed by
///   hand goes stale the moment nobody remembers it exists. `env!` cannot.
/// * **It named no contact.** Wikimedia's User-Agent policy asks for a way to
///   reach whoever is running the client, and an unidentifiable client is the
///   one that gets throttled. Adding that is the OPPOSITE of the masquerade
///   this comment argues against: it says MORE about who we are, not less.
```

## L66 · `struct HttpFlags {`

```
/// Flags parsed from HTTP/HTTPS arguments.
```

## L68 · `headers_only: bool,  // -h: show only headers`

```
// -h: show only headers
```

## L69 · `body_only: bool,     // -b: show only body`

```
// -b: show only body
```

## L70 · `silent: bool,        // -s: no status output`

```
// -s: no status output
```

## L71 · `discard: bool,       // -d: stream + count + report MB/s, DON'T store (RAM only)`

```
// -d: stream + count + report MB/s, DON'T store (RAM only)
```

## L74 · `fn parse_http_args(args: &str) -> (HttpFlags, String) {`

```
/// Parse flags from anywhere in the args, return flags + cleaned args.
```

## L104-105 · `super::clear_cancel();`

```
// Arm Ctrl+C cancellation for this download (cleared so a stale earlier
// press doesn't abort us immediately).
```

## L120-124 · `let (url_no_redirect, store_as) = if let Some(idx) = url.find('>') {`

```
// Step 1: peel off `> store-redirect` first. Has to happen
// BEFORE the host/path split because for inputs like
// `host/long/url/path > tmp/file` the first whitespace lives
// before the `>`, so a naive whitespace split would assign
// `host/long/url/path` to the host and break DNS.
```

## L134-135 · `let (host, path) = if let Some(idx) = url_no_redirect.find(' ') {`

```
// Step 2: split host from URL path on the first whitespace OR
// first slash, on the redirect-free remainder.
```

## L146-163 · `if flags.discard || store_as.is_some() {`

```
// Streaming fast-path: `-d` or `> name` writes the body straight into
// npkFS via the ChunkedWriter so a multi-GB ISO / movie download doesn't
// fill the heap. Peak RAM = one 16 MiB chunk regardless of total size.
//
// **Fuer BEIDE Schemata.** Hier stand, Klartext-HTTP bleibe beim
// Speichern auf dem gepufferten Weg mit seinen 128 KB — das tut es
// nicht, und die Bedingung unten sagt es auch nicht: sie fragt nur nach
// `-d` oder `> name`. `HTTP_MAX_RESPONSE` gilt allein fuer den Weg
// DARUNTER, der die Antwort auf den Schirm schreibt. Der Satz hat mich
// einmal eine falsche Auskunft gekostet.
//
// ⚠ Und was hier NICHT geprueft wird: `net.allow_plain_http`. Der
// Schalter sitzt in `parse_url`, und die ruft nur der MODULweg
// (`npk_http_*`, also beak). Wer `http …` in die Shell tippt, bekommt
// Klartext ohne Schalter und ohne die KLARTEXT-Zeile, die `parse_url`
// sonst druckt. Das ist vertretbar — am eigenen Prompt ist der Mensch
// die Instanz, nicht die Seite —, aber es ist eine Asymmetrie und
// gehoert benannt statt entdeckt.
```

## L165-169 · `let mut writer: Option<(String, crate::npkfs::fs::StreamingWriter)> = if flags.discard {`

```
// Sink for the streamed body. With -d we DON'T open npkFS — bytes are
// counted + thrown away, so this measures the pure net throughput
// and rules the disk OUT as a bottleneck. Otherwise stream to npkFS.
// Works for both https (TLS) and http (plain) — plain http sidesteps
// our minimal TLS for arbitrary hosts and is a cleaner speed test.
```

## L176 · `crate::drivers::rtl8153::tally_reset(); // zero chip stats for this run`

```
// zero chip stats for this run
```

## L222 · `use core::sync::atomic::Ordering::Relaxed;`

```
// Profiler: avg ns in poll_rx_only vs recv per loop iter.
```

## L236 · `let (rxb, deliv, empty, armed, txc, txcyc) = crate::xhci::nic_take_stats();`

```
// USB-transport layer: is the bulk RX itself the cap?
```

## L244-252 · `let (wnd, srtt, cap) = crate::net::tcp::window_diag();`

```
// **Was haben WIR angeboten, und woher kam der Deckel?**
// Auf einer Leitung mit Latenz ist das die Frage: `srtt`
// wird in 100-Hz-Takten gemessen, also mit 10 ms Koernung.
// Ein RTT unter 10 ms ergibt die Probe 0 — und die wird
// VERWORFEN (`(1..6000).contains`), also bleibt `srtt` auf
// null und der Deckel faellt auf die 50-ms-Annahme zurueck.
// Ein RTT von 20 ms ergibt 1-2 Takte, und daraus wird ein
// KLEINERER Deckel als auf der schnellen Leitung. Genau
// verkehrt herum, und hier steht es als Zahl.
```

## L256 · `let (frames, trunc, discard) = crate::drivers::rtl8153::take_rx_parse_stats();`

```
// Do WE discard received bytes in the rx_desc walker?
```

## L264-266 · `user_download_streaming(host, path, use_tls, max_size, &mut sink)`

```
// Cross-scheme redirect-following download: lets `https cdimage…`
// chase its 302 to a fast plain-http mirror (the user's gigabit
// source) where strict https would dead-end.
```

## L275-277 · `if crate::xhci::nic_attached() {`

```
// Die Zaehler des Chips gehoeren auf JEDEN Ausgang — sie
// standen nur hinter dem Erfolgsfall, also genau dort nicht,
// wo man sie braucht.
```

## L282 · `return;`

```
// `writer` drops here → StreamingWriter::drop cleans up the partial.
```

## L285 · `let dt = crate::interrupts::ticks().wrapping_sub(start_tick).max(1);`

```
// Final throughput summary (the headline number for a speed test).
```

## L302 · `let ip = if let Some(ip) = parse_ip(host) {`

```
// Resolve hostname
```

## L320 · `let gw = crate::net::ipv4::gateway();`

```
// ARP resolve gateway
```

## L322 · `let _ = crate::net::arp::resolve(gw, 100); // see open_tls: not a blind spin`

```
// see open_tls: not a blind spin
```

## L334 · `let mut tls_session = if use_tls {`

```
// TLS handshake (if HTTPS)
```

## L356 · `let http_ver = if use_tls { "1.1" } else { "1.0" };`

```
// Send HTTP GET
```

## L375 · `let mut response = alloc::vec::Vec::new();`

```
// Receive response (buffer >= max TLS record to avoid data loss)
```

## L385-388 · `if empty_count > 40 && response.is_empty() { break; } // 200ms`

```
// Response can arrive across multiple TCP segments. Poll the
// net stack and wait ~5ms between zero-reads instead of
// tight-looping in microseconds (starves slow links like
// QEMU user-mode NAT before the response arrives).
```

## L389 · `if empty_count > 40 && response.is_empty() { break; } // 200ms`

```
// 200ms
```

## L390 · `if empty_count > 10 && !response.is_empty() { break; } // 50ms`

```
// 50ms
```

## L393 · `+ crate::interrupts::tsc_freq() / 200; // 5ms`

```
// 5ms
```

## L407-408 · `Err(_) => break,`

```
// A timeout now arrives as an error rather than as Ok(0); for
// this plain-HTTP reader both still mean "stop".
```

## L421 · `let header_end = response.windows(4)`

```
// Find header/body boundary
```

## L428-432 · `let status = core::str::from_utf8(&response[..header_end])`

```
// Guard: don't write a "successful" file from a redirect or
// error response. The legacy non-TLS path doesn't follow
// 3xx, so `http github.com/...` (which 301s to https) would
// otherwise store a 0-byte file and print "Stored". Point
// the user at `https` instead of silently succeeding.
```

## L467 · `if flags.headers_only {`

```
// Display based on flags
```

## L475 · `print_response_data(&response);`

```
// Full response: headers + body
```

## L491-501 · `fn resolve_store_target(dest: &str, url_path: &str) -> Option<String> {`

```
/// Resolve a `> dest` store target, wget-style. `url_path` is the
/// HTTP path of the request (everything after the host) and is used
/// to infer a filename when `dest` names a directory rather than a
/// file:
///   `.`        → URL basename, in CWD
///   `dir/`     → URL basename, in `dir`
///   `dir/.`    → URL basename, in `dir`
///   `dir/name` → exact (unchanged behavior)
///
/// Returns the CWD-resolved npkFS path, or `None` if a basename was
/// required but the URL has none (path ends in `/`, or is just `/`).
```

## L512-513 · `let dir = dest.trim_end_matches('.').trim_end_matches('/');`

```
// Strip the trailing dir markers ("." / "/") and join the
// inferred basename onto whatever directory prefix remains.
```

## L523-525 · `fn url_basename(url_path: &str) -> Option<String> {`

```
/// Last path segment of an HTTP URL path, minus any `?query` or
/// `#fragment`. `None` when there is no segment (e.g. `/` or
/// `/dir/`).
```

## L540-544 · `struct HttpResponse {`

```
/// Status + Location of a single HTTPS round-trip. Body bytes are
/// not carried in this struct — `https_get_once` always pushes them
/// through the caller's sink closure as they arrive (`https_get`
/// installs a Vec-collecting sink, `https_get_streaming` passes the
/// caller's sink through directly).
```

## L548-550 · `content_type: Option<String>,`

```
/// Raw `Content-Type` value. A browser cannot decode a document without
/// it: a page declaring `charset=ISO-8859-1` is not UTF-8, and guessing
/// wrong costs the whole document.
```

## L552-555 · `headers: String,`

```
/// The response header block verbatim, minus the status line. A browser
/// needs headers the body cannot carry — `Set-Cookie` (which repeats, so
/// no single-value getter would do) and `Retry-After`. Capped, see
/// `MAX_REPLY_HEADERS`.
```

## L559-560 · `pub struct HttpRequest<'a> {`

```
/// What to send. `GET`, no extra headers, no body — what every caller before
/// the browser meant, and what [`HttpRequest::default`] gives you.
```

## L563-565 · `pub headers: &'a [String],`

```
/// Extra request headers as `Name: value`, without CRLF. The caller is
/// responsible for validating them; [`header_line_is_safe`] is the check
/// the WASM boundary uses.
```

## L568-574 · `pub accept_gzip: bool,`

```
/// Ask for `gzip` and inflate the answer here, before the sink sees it.
///
/// Off by default, and that is deliberate: OTA downloads are already
/// compressed and signed, so for them this would be work without an
/// answer. It is the BROWSER that pays for its absence — the same
/// document arrives 4,1x to 9,9x larger without it, measured across the
/// target corpus (`docs/plan/JS_SCOPE_CONTENT_WEB.md` §8).
```

## L576-581 · `pub plain: bool,`

```
/// Diese Anfrage geht im KLARTEXT, ohne TLS.
///
/// Aus by default, und das ist keine Vorsichtsmassnahme, sondern die
/// Politik: `parse_url` setzt sie nur, wenn `net.allow_plain_http` an ist
/// UND der Host eine literale private Adresse ist. Wer sie von Hand setzt,
/// umgeht diese Pruefung — also nicht tun.
```

## L583-591 · `pub try_h2: bool,`

```
/// Offer HTTP/2 for this request, falling back to HTTP/1.1 when the host
/// does not speak it.
///
/// Off by default, and that is a decision about blast radius, not about
/// h2: OTA and module installs come down this same path, and an update
/// that cannot download is the one failure this project cannot fix over
/// the air. The browser — which is the caller Wikimedia throttles (§8.1)
/// — asks for it. Flip the default once a release has h2 documents on
/// hardware behind it.
```

## L593-603 · `pub from_reach: Option<Reach>,`

```
/// Die Reichweite des DOKUMENTS, das diese Anfrage ausloest.
///
/// `None` heisst „nicht von einer Seite": OTA, Modulinstallation,
/// netbench. Die sind das Betriebssystem selbst und duerfen ueberallhin.
/// `Some(Reach::Public)` heisst „eine oeffentliche Seite fragt" — und
/// dann ist das private Netz des Nutzers zu.
///
/// Vorgabe ist `None`, und das ist hier ausnahmsweise die LOCKERE Wahl.
/// Sie ist trotzdem richtig: der Vorgabewert bedient die Aufrufer im
/// Kernel, und der einzige Weg, auf dem Seitencode je hierher kommt,
/// ist `npk_http_begin` — der setzt das Feld ausdruecklich.
```

## L614-620 · `fn capture_headers(hdr_str: &str) -> String {`

```
/// The response header block, minus the status line, capped and cleaned.
///
/// Drops any line starting with a colon. No HTTP field name may begin with
/// one, so nothing legitimate is lost — and it is what keeps a server from
/// writing its own `:hop https://yourbank.example` into its headers and
/// having the browser file the cookies that follow against a host it does
/// not own.
```

## L637-644 · `fn push_hop(out: &mut String, host: &str, path: &str, headers: &str, tls: bool) {`

```
/// Append one hop's response headers under a `:hop <url>` marker.
///
/// A redirect chain can cross origins, and a cookie belongs to the response
/// that SENT it — filing a login cookie against the URL the chain happened to
/// end at scopes it to the wrong host. So each block says where it came from.
///
/// `:hop` cannot be forged by a server: a field name may not start with a
/// colon, and `capture_headers` drops any line that does.
```

## L646-648 · `out.push_str(if tls { ":hop https://" } else { ":hop http://" });`

```
// Auch hier das ECHTE Schema. Der Behaelter liest aus dieser Marke, ob
// die Antwort ueber einen sicheren Kanal kam — ein `Secure`-Keks, der im
// Klartext ankam, darf nicht abgelegt werden, als waere er es nicht.
```

## L659-661 · `const MAX_REPLY_HEADERS: usize = 8 * 1024;`

```
/// Response headers we hand back to a guest, capped. Big enough for a page
/// that sets a dozen cookies, small enough that a hostile server cannot make
/// the kernel hold an unbounded string per request.
```

## L664-675 · `const RESERVED_HEADERS: &[&str] =`

```
/// Headers a guest may NOT set, because we own them.
///
/// `Host` decides which virtual host answers, and letting a caller state one
/// that differs from the TLS SNI name is a request for the wrong origin's
/// content under the wrong certificate. The other three frame the message: if
/// a caller could state its own `Content-Length` or `Transfer-Encoding`, the
/// body it sends and the body we announce could disagree, which is exactly
/// the shape of a request-smuggling bug.
// `accept-encoding` is ours for the same reason the framing headers are:
// we decode the answer. An app that asks for an encoding we do not unpack
// gets bytes it cannot read, and one that asks for none while we inflate
// anyway would be lied to about what came back.
```

## L679-684 · `pub fn header_line_is_safe(line: &str) -> bool {`

```
/// Is this a header line a guest is allowed to send?
///
/// Rejects CR, LF and NUL anywhere — a newline in a header VALUE ends the
/// header block early and everything after it is read as another request, so
/// this single check is what stops a sandboxed app from smuggling one. Also
/// rejects a missing colon, an empty name, and the reserved names above.
```

## L689-692 · `if line.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7F) {`

```
// No control characters anywhere. CR and LF are the smuggling vector, but
// the rest have no business in a field value either (RFC 9112 §5.5), and
// a NUL or a stray 0x01 is how a parser downstream gets confused. Tab is
// the one control character a value may legitimately hold.
```

## L697-701 · `if name.is_empty() || !name.bytes().all(|b| b.is_ascii_graphic()) {`

```
// NOT trimmed, deliberately. A leading space makes the line an obsolete
// folded continuation of the header before it, and a space before the
// colon is a name a server may read differently than we do. Both are
// ways to mean something other than what this line looks like, so both
// are refused: `is_ascii_graphic` excludes space and tab.
```

## L708-719 · `pub use super::reach::{classify_ip, Reach};`

```
/// Is this a method a guest is allowed to send? A token of ASCII letters,
// ── Reichweite ───────────────────────────────────────────────────────────
//
// Siehe `docs/plan/BROWSER_FETCH_ORIGIN.md` §3.1 V2. Kurz: eine oeffentliche
// Seite darf das private Netz des Nutzers nicht erreichen. CORS deckt das
// NICHT ab — es schuetzt den Zielserver, nicht das Netz, in dem der Browser
// steht. Browser haben die Regel als *Private Network Access* nachgerueckt
// und bis heute nicht vollstaendig; wir bauen sie von Anfang an.
//
// Warum im Kernel und nicht in beak: eine Grenze, die das Modul im
// Sandkasten selbst zieht, ist keine. beak sagt nur, WOHER eine Anfrage
// kommt; ob sie darf, entscheidet diese Datei.
```

## L723-732 · `pub fn resolve_checked(host: &str, from: Option<Reach>) -> Result<[u8; 4], &'static str> {`

```
/// Einen Host aufloesen UND die Reichweite pruefen — in EINEM Schritt.
///
/// Getrennt waeren es zwei, und die zweite wuerde irgendwo vergessen: im
/// Baum standen fuenf Stellen mit demselben
/// `parse_ip(h).or_else(|| dns::resolve(h))`. Wer hier durchgeht, ist
/// geprueft; wer die Pruefung umgehen will, muss es sichtbar tun.
///
/// `from` ist die Reichweite des Dokuments, das die Anfrage ausloest.
/// `None` heisst „nicht von einer Seite" — OTA, Modulinstallation, netbench.
/// Die duerfen ueberallhin, sie sind das Betriebssystem selbst.
```

## L749-753 · `pub fn reach_of_url(url: &str) -> Reach {`

```
/// Die Reichweite eines DOKUMENTS, aus seiner Adresse.
///
/// Aufgeloest wird hier, im Kernel. Alles, was keine brauchbare Adresse hat
/// oder sich nicht aufloesen laesst, ist `Public` — die strengste Klasse.
/// Ein Fehlschlag darf nie mehr erlauben als ein Erfolg.
```

## L770-771 · `pub fn method_is_safe(m: &str) -> bool {`

```
/// nothing else — the method sits at the very front of the request line, so
/// a space or a newline there rewrites the whole request.
```

## L776-779 · `#[derive(Default)]`

```
/// What the caller learns about a response besides its bytes.
///
/// Grouped rather than passed as more out-params, because every one of
/// these is "something the body alone cannot tell you" and the list grows.
```

## L782-783 · `pub final_url: String,`

```
/// The URL the body actually came from, after redirects (RFC 3986
/// §5.1.3 base URL). Empty if unknown.
```

## L785 · `pub content_type: String,`

```
/// The response's `Content-Type`, verbatim. Empty if the server sent none.
```

## L787 · `pub status: u16,`

```
/// The final response's status. 0 if the exchange never got that far.
```

## L789-791 · `pub headers: String,`

```
/// The final response's header block, minus the status line. Only filled
/// by [`https_request_streaming`] — a GET caller has no use for it and
/// would pay a copy per sub-resource.
```

## L795-806 · `pub fn https_get(host: &str, path: &str, max_size: usize) -> Result<alloc::vec::Vec<u8>, &'static str> {`

```
/// Reusable HTTPS GET — returns the response body as Vec<u8>.
///
/// Suitable for small responses (manifests, signatures, JSON, < ~32 MB
/// configs). For large downloads use [`https_get_streaming`] instead —
/// this function buffers the entire body in heap and will OOM the
/// kernel on multi-GB inputs.
///
/// Follows up to 3 redirects (301/302/303/307/308). Absolute and
/// relative Location values are both honored; absolute redirects to
/// a different host re-handshake TLS against that host (this is how
/// GitHub Releases work — `github.com/.../releases/download/...`
/// always 302s to a signed `objects.githubusercontent.com` URL).
```

## L808 · `https_get_ex(host, path, max_size, false)`

```
// OTA and module installs land here: signed, already-compressed payloads.
```

## L812-813 · `pub fn https_get_ex(host: &str, path: &str, max_size: usize, accept_gzip: bool)`

```
/// As [`https_get`], but the caller says whether it wants the transfer
/// compressed. The browser does; nothing else in the tree does.
```

## L820-822 · `fn https_get_req(host: &str, path: &str, max_size: usize, req: &HttpRequest)`

```
/// As [`https_get_ex`], but the caller states the whole request — which is
/// how the browser's sub-resource fallback asks for HTTP/2 and an OTA
/// download does not.
```

## L828-830 · `if let Err(e) = resolve_checked(&cur_host, req.from_reach) {`

```
// **Jeder Sprung neu.** Eine Weiterleitung auf `192.168.1.1` ist
// genau der Weg, den man sonst nimmt, wenn nur der erste Aufruf
// geprueft wird — und sie kostet den Angreifer nichts.
```

## L834 · `let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();`

```
// Vec-mode: accumulate into out, sink just extends it.
```

## L836-842 · `let step = core::cmp::max(max_size / 8, 256 * 1024);`

```
// Progress heartbeat. The streaming asset path reports every 8 MiB —
// but a kernel is ~4 MB and a module ~1.4 MB, so NEITHER ever crossed
// that threshold and both downloaded in complete silence. Over a slow
// WiFi link that is indistinguishable from a hang, and it is the path
// every update takes. Step from the expected size so any real download
// reports about eight times; manifests and signatures never reach
// 256 KiB and stay quiet.
```

## L884-894 · `pub fn https_get_streaming(`

```
/// Streaming HTTPS GET — drives body bytes through `on_chunk` as they
/// arrive, never buffering the full payload in memory. The caller
/// chooses where the bytes go (typical: `npkfs::open_streaming_write`
/// then `writer.write(chunk)`).
///
/// Returns the total number of body bytes pushed to the sink. Follows
/// up to 3 redirects, same rules as [`https_get`].
///
/// On non-2xx (other than a 3xx that's followed), the sink is NOT
/// called and an error is returned — so a half-failed download
/// never feeds garbage into the consumer.
```

## L901-902 · `https_get_streaming_ex(host, path, max_size, on_chunk, None, &HttpRequest::default())`

```
// OTA: the payload is already compressed and signed — asking for gzip
// would be a round of work with nothing at the end of it.
```

## L906-914 · `pub fn https_get_streaming_ex(`

```
/// As [`https_get_streaming`], but also reports what the caller needs to
/// interpret the bytes — see [`FetchInfo`].
///
/// A browser resolves a document's relative URLs against the *final*
/// address (the document base URL, RFC 3986 §5.1.3). Without it every
/// relative sub-resource is requested against the pre-redirect address and
/// pays a second round-trip through the same redirect — which is what drove
/// beak into Wikimedia's rate limit. It equally cannot decode the bytes
/// without the Content-Type.
```

## L931-940 · `pub fn https_request_streaming(`

```
/// The general exchange: any method, any (validated) headers, any body,
/// following redirects.
///
/// `want_headers` fills `FetchInfo::headers` — off for sub-resource GETs,
/// which have no use for it and would pay a copy each.
///
/// `keep_status` decides what a non-2xx means. An OTA download wants an
/// error; a BROWSER wants the bytes, because a 404 page and a 403 explaining
/// why are documents a person needs to read. With it set, the status is
/// reported in `FetchInfo` and the body is delivered whatever it says.
```

## L952-954 · `let mut cur_tls = !req.plain;`

```
// Welches Schema der AKTUELLE Sprung faehrt. Eine Umleitung darf
// hinaufgehen (Klartext -> TLS), aber niemals hinunter — das ist die
// Regel, die `parse_https_url` seit jeher durchsetzt, und sie bleibt.
```

## L956-957 · `let mut method = String::from(req.method);`

```
// A redirect can change the method, so the request travels by value from
// here on (RFC 9110 §15.4).
```

## L960-965 · `let mut carry_headers = true;`

```
// Caller headers stop at the first hop that leaves the origin they were
// written for. The caller computed its `Cookie:` (and anything else
// sensitive) for THIS host; replaying it to whatever a redirect names
// hands one site's session to another. We follow redirects on the
// caller's behalf, so this rule is ours to keep — the same reason the
// redirect path already refuses an http downgrade.
```

## L967-971 · `let mut hops = String::new();`

```
// Every hop's response headers, each under a `:hop <url>` marker, so the
// caller can scope what it finds to the response that actually sent it.
// A cookie set by the 303 of a login POST lives HERE and nowhere else:
// reporting only the final response's headers drops it, and the login
// silently does not take.
```

## L976-978 · `if let Err(e) = resolve_checked(&cur_host, req.from_reach) {`

```
// Auch hier JEDER Sprung. Das ist die Schleife, die der Browser
// faehrt — die andere (`https_get_req`) gehoert OTA. Sie zu
// uebersehen waere die ganze Regel gewesen.
```

## L1005-1006 · `_ if keep_status => true,`

```
// Any other status is a document too, once the caller asked for
// it. Without `keep_status` this stays the old hard error.
```

## L1013-1019 · `out.final_url.push_str(if cur_tls { "https://" } else { "http://" });`

```
// Das Schema, das WIRKLICH gefahren wurde — nicht immer
// `https`. beak macht aus dieser Zeichenkette seine
// Basisadresse, und `origin_of` einer Adresse ohne Schema
// setzt `https://` davor: die Seite kam im Klartext an, und
// jedes Stilblatt danach lief in einen TLS-Handschlag gegen
// einen Klartext-Server. Im Serverlog steht dann ein
// ClientHello als „Bad request version".
```

## L1028-1031 · `out.headers.clear();`

```
// The header blocks are copied only for the caller that asked
// to see statuses — the browser. A page load fans out to ~20
// sub-resource GETs, and none of them has any use for a
// second copy of their headers.
```

## L1040-1042 · `let loc = resp.location.ok_or("redirect without Location")?;`

```
// On 3xx the inner once-fn returns early without calling the sink, so
// the consumer never sees any bytes from the redirect response. Safe
// to retry against the Location target with a fresh TLS session.
```

## L1044-1047 · `if keep_status {`

```
// A redirect's OWN headers matter: a login POST is answered with a
// 303 that carries `Set-Cookie: session=…`, and the page it points at
// is only reachable because of it. Keeping just the last response's
// headers threw the session away and the login quietly did not take.
```

## L1051-1053 · `let (next_host, next_path, next_tls) = if cur_tls {`

```
// Auf TLS gilt weiter die strenge Fassung: sie verweigert jedes
// `http://` im `Location`. Faehrt der Lauf schon im Klartext, darf das
// Ziel auch `https://` sein — hinauf ist erlaubt.
```

## L1066-1069 · `if matches!(resp.status, 301 | 302 | 303) && method != "GET" && method != "HEAD" {`

```
// 303 says so outright, and 301/302 after a POST is the behaviour
// every browser settled on (RFC 9110 §15.4.3 note): the redirect
// points at a RESULT page, and re-POSTing the form to it would
// submit twice. 307/308 exist precisely to keep method and body.
```

## L1078-1089 · `struct PooledConn {`

```
// ── HTTPS keep-alive connection pool ───────────────────────────────
// A page load in beak fans out to ~20 sub-resources (CSS, images), most
// from one or two hosts. Without reuse each pays a full fresh DNS + TCP
// + TLS handshake, serially — the "Zeitlupe" page load Florian saw on
// the serial log. Holding the TLS session open and sending
// `Connection: keep-alive` collapses those ~20 handshakes to ~1 per host.
//
// A session is returned to the pool ONLY when its response was fully
// framed (Content-Length or chunked, and we read the whole body off the
// wire) and the peer did not signal `Connection: close`. Otherwise the
// message boundary on the wire is unknown and reuse would desync the
// byte stream.
```

## L1093-1097 · `idle_since: u64,`

```
/// When it went idle. A server keeps a connection open for 5-75 s and
/// then closes it; `is_healthy` sees only the LOCAL TCP state, so a peer
/// that hung up quietly still looks alive here. The age is what catches
/// that — one wasted round-trip per stale socket, and the delayed-ACK-
/// shaped 230 ms header times all sat on pooled connections.
```

## L1101-1103 · `const POOL_MAX_IDLE_TICKS: u64 = 500; // 5 s at 100 Hz`

```
/// How long a pooled connection may sit unused. Under every common server
/// keep-alive (nginx 75 s, most CDNs 5-10 s), above any page load's own
/// fan-out — the reuse we actually want is measured in the same second.
```

## L1104 · `const POOL_MAX_IDLE_TICKS: u64 = 500; // 5 s at 100 Hz`

```
// 5 s at 100 Hz
```

## L1109-1122 · `fn drain_before_reuse() {`

```
/// Take a *live* pooled session for `host`, if any. A session the server
/// has since closed (idle-timeout FIN → state left `Established`) is
/// closed and skipped here, so the caller never sends on a dead socket.
/// Vor jedem Griff in einen Pool: den NIC-Ring leeren.
///
/// `conn_healthy` liest den VERBINDUNGSZUSTAND, und der aendert sich nur,
/// wenn ein Paket verarbeitet wurde. Zwischen Einlegen und Herausnehmen
/// pollt niemand — das FIN des Servers liegt also unbearbeitet im Ring, der
/// Zustand sagt weiter `Established`, und die Pruefung nickt eine tote
/// Verbindung durch. Gemessen: `0/4 over one connection` auf
/// thumb.wikimedia.org, reproduzierbar, weil der Server nach jeder bedienten
/// Runde zumacht.
///
/// Ein Aufruf, und die Pruefung sieht, was schon angekommen ist.
```

## L1139-1140 · `kprintln!("[npk]   pool {}: Verbindung verworfen ({})", host,`

```
// Sagen, dass es gegriffen hat — sonst ist "kein Haenger mehr"
// nicht von "der Fall trat nicht ein" zu unterscheiden.
```

## L1150-1151 · `fn pool_put(host: &str, tls: crate::tls::TlsSession) {`

```
/// Return a reusable session to the pool. Evicts (and closes) the first
/// slot when the pool is full.
```

## L1170-1171 · `fn header_has_token(value: &str, token: &str) -> bool {`

```
/// True if `value` (an HTTP list header) contains `token` as a
/// comma-separated element, case-insensitively.
```

## L1176 · `fn finish_conn(host: &str, tls: crate::tls::TlsSession, reusable: bool) {`

```
/// Either pool `tls` for reuse or close it — exactly one, never both.
```

## L1186-1191 · `pub fn error_kind(msg: &str) -> &'static str {`

```
/// Classify a fetch failure into a stable token.
///
/// The message itself is written for a human reading the serial log and is
/// free to be reworded; this token is the contract a client branches on.
/// Keeping them apart means improving a message never silently changes
/// which error page the browser shows.
```

## L1206-1209 · `crate::tls::reasons::HANDSHAKE_REJECTED`

```
// Matched against the constants, not copies of the text — see
// `tls::reasons`. The peer aborting the handshake is the common
// shape here (a server that dislikes our ClientHello), and it is
// NOT a certificate problem, so it must not read like one.
```

## L1222 · `fn open_tls(host: &str) -> Result<crate::tls::TlsSession, &'static str> {`

```
/// Fresh DNS + ARP + TCP + TLS to `host:443`. Only paid on a pool miss.
```

## L1224-1225 · `let (bare, port) = split_host_port(host);`

```
// Der nackte Name fuer DNS und fuer die TLS-Kennung (SNI); der Port, wenn
// einer dasteht, sonst 443.
```

## L1235-1244 · `let gw = crate::net::ipv4::gateway();`

```
// Make sure the gateway's MAC is known before we SYN.
//
// This used to fire an ARP request and then spin 50_000 times over the
// FULL `net::poll()` — unconditionally, even when the MAC was already
// cached, and `net::poll()` also runs a shade render pass. So every fresh
// connect paid 50_000 render passes, and the price grew with whatever the
// browser had on screen: handshakes measured 350 ms against an empty page
// and 2250 ms once a real article was painted. `arp::resolve` returns
// immediately on a cache hit and otherwise polls only until the reply
// lands.
```

## L1246 · `let _ = crate::net::arp::resolve(gw, 100); // 1 s at 100 Hz`

```
// 1 s at 100 Hz
```

## L1254-1257 · `kprintln!("[npk] TLS error: {}", e);`

```
// Keep the reason. Collapsing every handshake failure into one
// message is what left a browser with nothing to say but a blank
// page — "untrusted root CA" and "expired" are the two things the
// person in front of the screen actually needs to be told apart.
```

## L1263-1265 · `let done = crate::interrupts::ticks();`

```
// Split so a slow connect names its own culprit. The whole thing swings
// between ~200 ms and ~2100 ms across runs, and 2 s is suspiciously
// exactly a retransmission timeout — this says which leg waits.
```

## L1276-1281 · `use super::http2::{self, Http2};`

```
// ── HTTP/2 ──────────────────────────────────────────────────────────────────
//
// Two callers: the browser's batch fetch for sub-resources (`get_all`, whole
// bodies buffered) and the document fetch (`request`, streamed into the
// caller's sink). NOT OTA — see `HttpRequest::try_h2` for why that is a
// decision about blast radius rather than about the protocol.
```

## L1289-1290 · `static H2_REFUSED: spin::Mutex<[Option<String>; H2_POOL_SIZE]> =`

```
/// Hosts that turned out not to speak h2. Without this, every batch pays a
/// fresh TLS handshake to re-learn the same answer.
```

## L1316-1317 · `fn h2_take_exact(host: &str) -> Option<Http2> {`

```
/// Eine Verbindung, die schon fuer GENAU diesen Namen offen ist. Kostet kein
/// DNS — deshalb zuerst.
```

## L1325-1326 · `let fresh = now.wrapping_sub(idle_since) < POOL_MAX_IDLE_TICKS;`

```
// Same rule as the HTTP/1.1 pool: a GOAWAY we have not read yet
// is indistinguishable from a quiet connection.
```

## L1329-1332 · `conn.reused = true;`

```
// Der Nehmer soll wissen, dass er wettet: eine Gegenstelle,
// die zwischen zwei Benutzungen still weggeht, sendet weder
// FIN noch RST — vorhersagen laesst sich das nicht, nur
// schneller merken.
```

## L1345-1359 · `fn h2_take_coalesced(host: &str) -> Option<Http2> {`

```
/// **Connection Coalescing, RFC 7540 §9.1.1.** Eine offene Verbindung darf
/// einen ZWEITEN Namen bedienen, wenn beides gilt: sie geht zur selben Adresse
/// UND ihr Zertifikat deckt den Namen.
///
/// Gemessen auf de.wikipedia.org/wiki/Stansstad: `de.wikipedia.org`,
/// `thumb.wikimedia.org` und `auth.wikimedia.org` loesen alle auf
/// 185.15.58.224 auf und bekamen je einen eigenen Handshake — 3 x 90 ms je
/// Seitenaufbau, zweimal in Folge so gemessen. `upload.wikimedia.org` liegt
/// auf .240 und bleibt zurecht getrennt; genau dafuer steht die Adresse in
/// der Bedingung und nicht nur das Zertifikat.
///
/// Die Namenspruefung ist DIESELBE Funktion wie im Handshake
/// (`certstore::covers`). Zwei Pruefungen nebeneinander laufen auseinander,
/// und die schwaechere gewinnt dann immer — hier waere das ein fremder Name
/// auf einer fremden Verbindung.
```

## L1363-1367 · `let ip = parse_ip(bare).or_else(|| crate::net::dns::resolve(bare))?;`

```
// Aufloesen, nicht nur den Zwischenspeicher fragen: die Runde faellt
// ohnehin an, `h2_connect` macht sie gleich danach. Sie hier zu machen
// kostet nichts und ist der Unterschied zwischen „beim ERSTEN Bild
// gespart" und „erst beim zweiten". VOR dem Lock — ein blockierendes DNS
// unter einem Spinlock haelt jeden anderen Nehmer an.
```

## L1373-1376 · `let hit = matches!(slot, Some((_, c, _))`

```
// `ip` ist nie 0.0.0.0 (ein Name loest nicht dorthin auf), aber eine
// Verbindung, deren TCP-Slot schon weg ist, MELDET 0.0.0.0:0. Die
// Gleichheit allein wuerde beide verwechseln — also erst gar keine
// unbekannte Gegenstelle zulassen.
```

## L1409-1410 · `fn h2_open(host: &str) -> Option<Http2> {`

```
/// A connection for `host`: the pooled one if it is still fresh, otherwise a
/// new one. The batch fetch's entry point.
```

## L1415-1418 · `fn h2_connect(host: &str) -> Option<Http2> {`

```
/// A NEW connection — no pool. Split out because the document path needs to
/// be able to say "not that one": a pooled connection can carry a GOAWAY we
/// have not read yet, and giving up on h2 for that would put the document
/// back under the HTTP/1.1 rate limit this whole path exists to leave.
```

## L1420-1423 · `let t_enter = crate::interrupts::ticks();`

```
// Everything before `t_dns` used to be the one unmeasured stretch of the
// connect, and a 2026-08-14 device log showed 2100 ms connect with every
// named leg at 90 ms. Do not "explain" that gap from a single clean run
// again — measure it.
```

## L1433-1434 · `let gw = crate::net::ipv4::gateway();`

```
// Gateway MAC, same as the h1 path (see `open_tls` for why this is a
// resolve and not a spin).
```

## L1443-1444 · `let t_pre = crate::interrupts::ticks();`

```
// The serial write above is itself unmeasured otherwise, and it sits
// INSIDE the span the caller reports as "connect".
```

## L1458-1460 · `Err(e) => {`

```
// Every other h2 failure falls back to HTTP/1.1 silently, which is the
// right behaviour but a terrible way to debug: the variants carry the
// reason, so say it when asked.
```

## L1470-1481 · `pub fn https_get_many(urls: &[String], cookies: &[String], max_size: usize,`

```
/// Fetch many URLs at once, multiplexed over one HTTP/2 connection per host
/// where the peer offers h2, and falling back to the existing sequential
/// HTTP/1.1 path where it does not.
///
/// Results are positional: entry `i` corresponds to `urls[i]`, and is `None`
/// if that resource could not be fetched. A response with a 4xx/5xx status
/// counts as a failure rather than returning the error page's body — the
/// caller is loading images and stylesheets, and decoding an HTML error page
/// as a PNG helps nobody.
/// `cookies` ist POSITIONELL zu `urls`: Eintrag `i` ist die fertige
/// `Cookie`-Zeile fuer `urls[i]` (ohne den Namen, nur der Wert), oder leer.
/// Wer sie fuellt, ist der Browser — der Kernel hat kein Keksglas.
```

## L1487-1490 · `let mut parsed: alloc::vec::Vec<Option<(String, String, bool)>> = alloc::vec::Vec::new();`

```
// Split into per-host groups, keeping the original positions.
// Das Schema bleibt DABEI: ein Klartext-Host kann kein h2 (h2c sprechen
// wir nicht) und muss den einfachen Weg nehmen. Es wegzuwerfen hiesse,
// jede Unterressource einer lokalen Vorlage gegen :443 zu versuchen.
```

## L1502-1504 · `let hosts: alloc::vec::Vec<String> = hosts.into_iter()`

```
// **Die Reichweite EINMAL je Host, vor allem anderen.** Ein verwehrter
// Host laesst seine Plaetze leer, genau wie ein kaputter Strom weiter
// unten — der Aufrufer sieht ein fehlendes Bild, keinen halben.
```

## L1524 · `let cks: alloc::vec::Vec<&str> = idxs.iter()`

```
// Dieselbe Reihenfolge wie `paths` — beide laufen ueber `idxs`.
```

## L1534-1536 · `let gz = r.header("content-encoding")`

```
// h2 buffers a stream's DATA frames into one
// Vec, so there is nothing to stream here —
// but the same cap applies.
```

## L1555-1557 · `if let Some(b) = body {`

```
// A damaged stream leaves the slot unset, so
// the h1 fallback below picks the URL up again
// — the same door a redirect goes through.
```

## L1563-1565 · `Ok(r) if (300..400).contains(&r.status) => {`

```
// A redirect needs the h1 path's follow logic (it
// may cross hosts); leave it unset and let the
// per-URL fallback below pick it up.
```

## L1574-1575 · `let now = crate::interrupts::ticks();`

```
// Split the time so a slow batch says WHERE it was slow:
// a fresh TLS handshake, or the transfer itself.
```

## L1599-1600 · `for &i in &idxs {`

```
// Anything h2 did not deliver — no h2, a protocol error, or a
// redirect — falls back to the ordinary sequential fetch.
```

## L1606-1613 · `let ck = cookies.get(i).map(|s| s.as_str()).unwrap_or("");`

```
// Still h2 where the host offers it — this door is the one a
// redirect goes through, and a redirected sub-resource is no
// less throttled than a direct one.
// Der h1-Rueckfall folgt Weiterleitungen, also traegt er die
// Reichweite mit — sonst waere er das Loch neben der Tuer.
// Der h1-Rueckfall traegt denselben Keks. Er ist der Weg, den
// eine Weiterleitung nimmt, und eine weitergeleitete
// Unterressource ist nicht weniger angemeldet als eine direkte.
```

## L1631-1635 · `enum ExchangeErr {`

```
/// Exchange failure mode. `Retry` = failed in the send/header phase,
/// before any body byte reached the sink → safe to retry on a fresh
/// connection (this is how a stale pooled socket surfaces, and how an h2
/// attempt hands the request to HTTP/1.1). `Fatal` = failed mid-body or a
/// protocol error → propagate, because a retry would deliver twice.
```

## L1641 · `struct H2Sink<'a> {`

```
// ── The document fetch over HTTP/2 ──────────────────────────────────────────
```

## L1643-1645 · `struct H2Sink<'a> {`

```
/// Adapter between an h2 stream and the sink the caller handed us. It exists
/// so that nothing above `https_get_once` has to know which protocol carried
/// the bytes: same clipping, same gzip, same "a 3xx never reaches the sink".
```

## L1649-1654 · `discard: bool,`

```
/// A 3xx body is courtesy text. `https_request_streaming` follows the
/// redirect instead and counts on the sink having stayed untouched.
///
/// Starts TRUE and is decided in `head`: DATA before HEADERS is a broken
/// (or hostile) peer, and the safe reading of "we do not know what this
/// response is yet" is that the caller may not have it.
```

## L1658-1659 · `touched: bool,`

```
/// Set once the body has started. After that, falling back to HTTP/1.1
/// would deliver the document twice.
```

## L1661-1662 · `t_head: u64,`

```
/// When the response headers landed, so the trace splits the wait from
/// the transfer the way the HTTP/1.1 path does.
```

## L1670-1673 · `if headers.iter().any(|h| {`

```
// Same link in the chain as HTTP/1.1 puts it: between the transport
// and the sink, streaming, with the caller's own cap as the budget —
// a zip bomb is then no more dangerous than a body of the size the
// caller already said it could take.
```

## L1703-1709 · `fn h2_header_block(headers: &[http2::Header]) -> String {`

```
/// The response header block in the shape every caller above already reads:
/// one `Name: value` line per field, capped.
///
/// Pseudo-headers are dropped for the same reason `capture_headers` drops
/// colon-prefixed lines — no HTTP field name may start with a colon, so
/// nothing legitimate is lost, and it is what stops a server from writing
/// its own `:hop` marker into the block.
```

## L1726-1727 · `fn h2_value<'a>(headers: &'a [http2::Header], name: &str) -> Option<&'a str> {`

```
/// Field lookup. h2 field names are lowercase on the wire (§8.2.1), so this
/// is an exact match and not a case-insensitive one.
```

## L1732-1742 · `fn h2_once(`

```
/// One HTTPS round-trip over HTTP/2, when the host speaks it.
///
/// `Retry` means nothing was delivered — no h2 here, no connection, or a
/// failure before the first body byte — so the caller may run the same
/// request over HTTP/1.1. `Fatal` means the sink has already seen bytes.
///
/// Redirects come back exactly as the HTTP/1.1 path hands them back: status
/// plus Location, body dropped. Following them stays one layer up, in
/// `https_request_streaming`, which is where the method switch, the
/// origin-crossing header rule and the per-hop `Set-Cookie` already live —
/// and every login runs through all three.
```

## L1750-1751 · `if let Some(conn) = h2_take(host) {`

```
// Attempt 1: the pooled connection. Same ladder as HTTP/1.1 below, for
// the same reason — the peer may have hung up while it sat idle.
```

## L1759 · `let Some(conn) = h2_connect(host) else {`

```
// Attempt 2: a fresh one. `None` here means this host does not speak h2.
```

## L1766 · `fn h2_exchange(`

```
/// The exchange itself, over a connection the caller already holds.
```

## L1809-1814 · `(Some(l), true) =>`

```
// WOHER die Umleitung kam, gehoert dazu. Ein Ziel allein
// laesst sich nicht beurteilen: die 301 auf dieser Seite
// zeigte auf eine Adresse, die genauso aussah wie die
// angefragte, und ohne die Quelle war nicht zu sehen,
// worin sie sich unterschieden
// ([[feedback-print-the-identifier-not-just-the-event]]).
```

## L1821-1823 · `match (gz, req.accept_gzip, redirect) {`

```
// Say that gzip ran. Silence is ambiguous three ways — never
// asked, server answered identity, or the path lost it — and
// the whole point of asking is a number.
```

## L1849-1851 · `fn drain_body(`

```
/// Discard a redirect's (small) body so the socket is left at a clean
/// message boundary and can be reused. Returns true iff the whole body
/// was consumed.
```

## L1873 · `false`

```
// A redirect with no framing → boundary unknown; not reusable.
```

## L1878-1882 · `fn https_exchange(`

```
/// One HTTPS round-trip over an OWNED TLS session (`Connection:
/// keep-alive`). Reads + parses the response, streams the body through
/// `on_chunk`, and hands the session back to the pool (if cleanly
/// reusable) or closes it. No redirect following — the returned
/// `HttpResponse` carries status + Location for the caller to follow.
```

## L1902-1905 · `if !req.body.is_empty() {`

```
// We state the length ourselves — always, when there is a body, and never
// from a caller-supplied header (`RESERVED_HEADERS`). Announcing a length
// that disagrees with the bytes we then send is how a request gets split
// in two on the far side.
```

## L1912-1914 · `let t_send = crate::interrupts::ticks();`

```
// h2 reports connect/transfer separately; h1 reported NOTHING between the
// handshake and "receiving body", which is where a 6.5 s document fetch
// hid on 2026-08-14 with every measured leg at 90 ms.
```

## L1917 · `let _ = crate::tls::tls_close(&mut tls);`

```
// Stale pooled socket (or a send error) — nothing delivered, retry fresh.
```

## L1922 · `let mut raw = alloc::vec::Vec::new();`

```
// ── Phase 1: read the header block (up to \r\n\r\n) ──
```

## L1924 · `let mut buf = [0u8; 17000]; // >= max TLS record (16KB)`

```
// >= max TLS record (16KB)
```

## L1925-1927 · `let t_hdr0 = crate::interrupts::ticks();`

```
// The loop leaves only two ways: it breaks WITH the header offset, or it
// returns. Yielding the offset out of `break` says that in the types, so
// there is no "we got here without a header" case left to handle.
```

## L1939-1940 · `let _ = crate::tls::tls_close(&mut tls);`

```
// A live server always answers — no header means the socket
// was dead/stale. No body delivered yet → safe to retry fresh.
```

## L1952 · `let hdr_str = match core::str::from_utf8(&raw[..hdr_end]) {`

```
// ── Phase 2: parse status + framing ──
```

## L1961-1962 · `let reply_headers = capture_headers(hdr_str);`

```
// Everything after the status line, capped. `Set-Cookie` repeats, so
// handing back parsed single values could never carry it.
```

## L1974 · `let http10 = hdr_str.starts_with("HTTP/1.0");`

```
// HTTP/1.1 is persistent by default; HTTP/1.0 is not.
```

## L1982-1984 · `if (300..400).contains(&status) {`

```
// ── Phase 3: consume the body ──
// Redirect: drain the courtesy body so the socket is clean, then hand
// the Location back to the caller (no bytes go to the sink).
```

## L2006-2010 · `let mut gunzip = match parse_header_value(hdr_str, "content-encoding") {`

```
// `Content-Encoding: gzip` — inflate between the transport and the sink,
// streaming, so the compressed body is never held a second time. The cap
// handed to the inflater is the caller's own `max_size`: a zip bomb is
// then no more dangerous than an uncompressed body of the size the caller
// already said it could take, and it is clipped in the same place.
```

## L2023-2025 · `let fully_drained;`

```
// 2xx / other: stream the body. The headers already proved the socket
// live, so a failure HERE is a genuine mid-body drop (partial bytes are
// already in the sink) → Fatal, never a retry.
```

## L2052-2053 · `fully_drained = delivered == cl;`

```
// Reusable only if we consumed the ENTIRE body — a max_size-clipped
// (truncated) read leaves unread bytes on the wire.
```

## L2056-2058 · `match stream_chunked_body(leading, &mut tls, &mut buf, max_size, &mut sink) {`

```
// True streaming chunked decoder (RFC 7230 §4.1) — GitHub codeload
// serves dynamically-generated tarballs chunked + binary, so the
// body can't be buffer-then-scanned.
```

## L2067-2068 · `let mut delivered = 0usize;`

```
// Neither Content-Length nor chunked → close-delimited body: read
// until the peer closes. Never reusable (no boundary to stop at).
```

## L2097-2100 · `match (gunzip.as_ref(), req.accept_gzip) {`

```
// Say that it ran. A gzip that quietly did not happen looks exactly
// like one that did, and the whole point of this path is a number.
// Silence used to be ambiguous three ways — old kernel, we never
// asked, or the server answered identity. Each now says which.
```

## L2115-2125 · `fn https_get_once(`

```
/// One HTTPS round-trip — no redirect following. Tries HTTP/2 when the
/// caller asked for it, then a pooled HTTP/1.1 keep-alive session for `host`,
/// then a fresh one. Body bytes are pushed through `on_chunk` as they arrive.
///
/// This is the layer h2 belongs in, and the reason is the redirect: a
/// redirect may change the host, and everything that follows from that —
/// the method switch, dropping the caller's headers at an origin boundary,
/// keeping each hop's `Set-Cookie` — lives ABOVE here, in
/// `https_request_streaming`. Lifting the document onto h2 anywhere higher
/// would have traded a throttling problem for a redirect problem, and every
/// login is a redirect chain.
```

## L2133-2139 · `crate::interrupts::set_worker_poll_hz(10_000);`

```
// Timer-NAPI, for the handshake AND the body. `http_get_once` has done this
// since it was written; the TLS path never did — so every OTA download
// polled its socket at 100 Hz and slept up to 10 ms between looks, while
// `netbench` over plain HTTP got 10 kHz and a 100 µs floor. A factor of a
// hundred on the receive path, and exactly the asymmetry we kept blaming on
// TLS itself: "update crawls, netbench flies". The guard restores 100 Hz on
// every exit path, including the `?` returns below.
```

## L2147-2154 · `if req.plain {`

```
// Klartext geht seinen eigenen Weg: kein h2 (h2c sprechen wir nicht),
// kein Sitzungsspeicher, kein TLS. `http_get_once` gibt es seit langem,
// es fehlte nur der Weg dorthin.
//
// Nur GET. Ein POST oder eigene Koepfe muessten durch `https_exchange`,
// und das setzt eine TLS-Sitzung voraus — den zweiten Rumpf dafuer zu
// bauen, bevor ihn jemand braucht, waere Arbeit auf Verdacht. Wer es
// versucht, bekommt eine Absage und keinen stillen Fehlschlag.
```

## L2162-2165 · `if req.try_h2 {`

```
// Attempt 0: HTTP/2. Wikimedia throttles HTTP/1.1 to ~0.5 requests/s and
// exempts h2 (§8.1, measured) — and one page load is FOUR document
// requests inside two seconds, so this path was the only one still
// paying. `Retry` means nothing was delivered; HTTP/1.1 runs below.
```

## L2174 · `if let Some(tls) = pool_take(host) {`

```
// Attempt 1: reuse a pooled session (no DNS/TCP/TLS handshake).
```

## L2178 · `Err(ExchangeErr::Retry) => {} // stale — reconnect below`

```
// stale — reconnect below
```

## L2182 · `let tls = open_tls(host)?;`

```
// Attempt 2: fresh connection.
```

## L2191-2198 · `fn parse_https_url(loc: &str, current_host: &str) -> Result<(String, String), &'static str> {`

```
/// Parse a Location header value into (host, path-with-query).
///
/// Accepts:
///   * absolute `https://host/path?query` → (host, "/path?query")
///   * absolute `https://host` → (host, "/")
///   * absolute-path `/path?query` → (current_host, "/path?query")
///
/// Rejects `http://...` (we never downgrade) and any other scheme.
```

## L2217-2220 · `pub(crate) fn parse_url(url: &str) -> Result<(String, String, bool), &'static str> {`

```
/// Parse a user-facing URL (from a WASM app, e.g. beak) into (host, path).
/// Accepts `https://host/path`, `host/path`, or a bare `host` (path
/// defaults to `/`). Scheme-less input is treated as https; plain `http://`
/// is refused (no downgrade). Reuses `parse_https_url`'s rules.
```

## L2239-2241 · `kprintln!("[npk]   http (KLARTEXT, kein TLS) -> {}", host);`

```
// JEDER Klartext-Abruf sagt es. Ein stiller Downgrade ist genau das,
// was die Regel verhindern soll — und wer den Schalter vergessen hat
// umzulegen, sieht es hier statt in einem Paketmitschnitt.
```

## L2251-2270 · `fn plain_http_allowed(host: &str) -> bool {`

```
/// Darf `http://<host>` ohne TLS geholt werden?
///
/// **Zwei Bedingungen, und beide muessen halten.**
///
/// 1. `net.allow_plain_http` steht auf `1`. Vorgabe ist AUS: ein Geraet, das
///    den Schluessel nie setzt, verhaelt sich wie vorher, und die Angriffs-
///    flaeche entsteht erst, wenn jemand sie einschaltet.
/// 2. Der Host ist eine LITERALE private Adresse — 10/8, 172.16/12,
///    192.168/16, 127/8, 169.254/16.
///
/// Der zweite Punkt ist nicht Bequemlichkeit, sondern der Kern: ein NAME
/// waere hier eine Luecke, weil sein DNS-Eintrag jederzeit auf eine oeffent-
/// liche Adresse zeigen kann (und beim zweiten Auflosen auf eine andere als
/// beim ersten). Eine Adresse, die im URL selbst steht, kann sich nicht
/// verwandeln.
///
/// ⚠ Was auch mit dem Schalter AN bestehen bleibt: eine fremde Seite kann
/// beak dazu bringen, Unterressourcen von `http://192.168.x.y` zu holen und
/// so das eigene Netz abzuklopfen. Deshalb ist der Schalter fuer die Dauer
/// einer Messung gedacht und nicht fuer den Dauerbetrieb.
```

## L2286-2294 · `pub(crate) fn split_host_port(host: &str) -> (&str, Option<u16>) {`

```
/// `host` oder `host:port` in beides zerlegen. Der Port ist `None`, wenn
/// keiner dasteht — dann entscheidet das Schema.
///
/// Bis hierher hat NICHTS im Kernel einen Port aus einer Adresse gelesen:
/// `connect(ip, 443)` stand fest, und `http://10.0.2.2:8080/x` waere auf :443
/// gelandet. Der Aufruf mit dem vollen `host:port` bleibt fuer den
/// `Host:`-Kopf richtig (RFC 9110 nennt den Port, wenn er nicht der
/// Vorgabeport ist); DNS, `parse_ip` und die TLS-Kennung brauchen den nackten
/// Namen.
```

## L2297-2299 · `Some((h, p)) if !h.contains(':') => match p.parse::<u16>() {`

```
// Ein Doppelpunkt im Namen ist auch eine IPv6-Adresse — die kann
// dieser Stapel nicht, aber sie darf hier nicht als Port gelesen
// werden.
```

## L2308 · `fn tls_recv_poll(tls: &mut crate::tls::TlsSession, buf: &mut [u8]) -> Result<usize, &'static str> {`

```
/// TLS recv with network polling. Retries on Ok(0) up to a hard timeout.
```

## L2313-2322 · `crate::net::poll_rx_only();`

```
// ONE poll per attempt. `net::poll()` already drains the
// entire NIC ring (`while let Some = netdev::recv`), ticks
// TCP, and runs a shade render pass — so a single call
// pulls everything currently available. The old code did
// this 2000× before every `tls_recv`, which on emulated
// NICs (each MMIO read traps to the hypervisor) cost ~0.5 s
// of pure overhead per 16 KiB TLS record → ~31 KiB/s
// ceiling regardless of link speed. Polling once and
// returning the instant a record is ready makes throughput
// bound by the actual network, not a fixed busy-wait tax.
```

## L2326-2328 · `if crate::interrupts::ticks().wrapping_sub(start) > 1500 {`

```
// No app data yet (partial record, or a control
// message like NewSessionTicket/CCS). Keep polling
// until the hard timeout.
```

## L2330 · `return Err("recv timeout"); // 15 seconds hard timeout`

```
// 15 seconds hard timeout
```

## L2340-2342 · `static PROF_POLL_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// Plain-TCP recv with polling (no TLS). Analog of `tls_recv_poll`.
// TEMP profiler: where does the recv loop's time go? TSC cycles + iteration
// count, read+reset in the http heartbeat. Pinpoints the ~13 µs/packet.
```

## L2365-2373 · `core::hint::spin_loop();`

```
// BUSY-SPIN, do NOT HLT. The USB NIC has no IRQ — its RX ring is
// re-armed ONLY by poll_rx_only() above. worker_idle_hlt() parks
// this core until the next 100 Hz worker tick (up to 10 ms);
// nothing re-arms the ring in that gap, so the chip exhausts all
// buffers in a few ms and then drops every frame → the ~24 Mbit
// cap + massive TCP reorder on rtl8153. (virtio/QEMU is immune:
// it delivers RX from a fiber, not this polled loop.) tcp_recv_poll
// only runs during an active download, so spinning is correct —
// and matches tls_recv_poll, which never had the HLT.
```

## L2382-2383 · `fn parse_http_url(loc: &str, current_host: &str) -> Result<(String, String), &'static str> {`

```
/// Parse a Location into (host, path) for the PLAIN-HTTP path: accepts
/// `http://…` and absolute-path; rejects https upgrade (our TLS is minimal).
```

## L2399-2403 · `pub fn http_get_streaming(`

```
/// Plain-HTTP streaming GET (no TLS) — for throughput tests + plain-http
/// mirrors (our TLS only handshakes with a couple of CAs, so arbitrary HTTPS
/// fails; plain HTTP sidesteps it AND is a cleaner pure-net speed test).
/// Follows up to 4 http→http (or absolute-path) redirects. Requires
/// Content-Length (true for static file mirrors); rejects chunked.
```

## L2437-2443 · `pub fn intent_netbench(args: &str) {`

```
/// Network throughput benchmark against a plain-HTTP server — isolates our net
/// stack from the WAN so we can see where the real bottleneck is. Reaches a
/// local server (e.g. `10.0.2.2:80` = QEMU slirp host alias).
///   netbench get <host> <path>        download into a counting sink (we time)
///   netbench put <host> <path> [MB]    upload a RAM buffer (the SERVER times it
///                                      and returns the rate — our send side has
///                                      no congestion control, so it can't)
```

## L2462 · `fn bench_report(label: &str, bytes: usize, cyc: u64) {`

```
/// Print bytes/elapsed as MB, ms, Mbit/s, MB/s using integer math (no float).
```

## L2467 · `let mbit_s = (bytes as u128 * 8 / us) as u64; // bits/us = Mbit/s`

```
// bits/us = Mbit/s
```

## L2468 · `let mbyte_s = (bytes as u128 / us) as u64;    // bytes/us = MB/s`

```
// bytes/us = MB/s
```

## L2487-2491 · `fn bench_cores() {`

```
/// **Auf welchem Kern lief die Messung, und auf welchem der WLAN-Treiber.**
/// Teilen sie sich einen, laufen Treiber und Leser abwechselnd statt
/// nebeneinander — und genau das sah 2026-09-22 aus wie ein Deckel der
/// Luft. Die Zeile sagt es in jedem Lauf, statt es aus Stillstaenden
/// zurueckzurechnen.
```

## L2510-2511 · `let r = reply.trim();`

```
// The server measures the true received rate (our send side has no
// congestion control); echo whatever it reported.
```

## L2520-2521 · `fn http_post_zeros(host: &str, path: &str, total: usize) -> Result<String, &'static str> {`

```
/// POST `total` zero-bytes to <host><path>; returns the server's response body
/// (which is expected to report the server-measured throughput).
```

## L2523-2528 · `let (bare, port) = split_host_port(host);`

```
// **Der Port stand hier hartcodiert auf 80**, und `parse_ip` bekam
// die ganze Zeichenkette samt `:8080` — also scheiterte schon die
// Aufloesung. Jeder andere HTTP-Weg dieser Datei geht seit je ueber
// `split_host_port`; der PUT-Weg als einziger nicht, und deshalb
// war `netbench put <host>:<port>` nie benutzbar. Der `Host:`-Kopf
// traegt weiterhin `host:port`, wie RFC 9110 es verlangt.
```

## L2533 · `let _ = crate::net::arp::resolve(gw, 100); // see open_tls: not a blind spin`

```
// see open_tls: not a blind spin
```

## L2534-2538 · `let handle = crate::net::tcp::connect(ip, port).map_err(|e| {`

```
// Name the failure. ConnectionRefused means the peer answered with a RST —
// nothing is listening there, go look at the server. Timeout means nobody
// answered at all — go look at ARP, routing, the air. Collapsing both into
// "TCP connect failed" sent us hunting the radio while a Python process on
// the other end had simply exited.
```

## L2560-2566 · `let chunk = alloc::vec![0u8; 64 * 1024];`

```
// 64 KiB, not 256. `tcp::send` refuses when `send_buf.len() + data.len()`
// exceeds MAX_UNACKED, which is itself 256 KiB — so a 256 KiB chunk could
// only ever be accepted with the send buffer at EXACTLY zero. That is
// stop-and-wait dressed up as a stream: every chunk had to be fully
// acknowledged before the next one could be queued, and one delayed ACK
// inside the 10 s window failed the whole transfer. A quarter of the cap
// leaves three chunks in flight.
```

## L2569-2573 · `crate::net::tcp::send_stats_reset();`

```
// **Erzeugerbegrenzt oder fensterbegrenzt.** Gemessen am 2026-09-22:
// 42 Mbit Upload bei 0,85 % belegter Luft und einem Treiber, der
// 63 000-mal je Sekunde vergeblich nach Sendearbeit sieht. Aus dem
// Durchsatz laesst sich beides zurueckrechnen, je nachdem welche RTT
// man einsetzt - also wird es jetzt gezaehlt statt gerechnet.
```

## L2579-2581 · `if let Err(e) = crate::net::tcp::send_blocking(handle, &chunk[..n], 1000) {`

```
// Say WHICH failure it was. "send body failed" covers a peer that
// closed the connection and a send window that never opened, and those
// want opposite investigations.
```

## L2584-2587 · `use core::sync::atomic::Ordering::Relaxed;`

```
// **Ein Stillstand muss seinen Zustand nennen.** Zweimal hat
// uns dieselbe Zeile ohne Zahlen einen ganzen Lauf gekostet:
// ein volles Fenster, ein geschlossenes Fenster und eine
// verpasste Weckung sehen von aussen gleich aus.
```

## L2598-2601 · `kprintln!("[netbench]   stau: cwnd {} · ssthresh {} · {} Doppelquittungen{} \`

```
// **Die Zahlen, ohne die ich vier Releases lang geraten
// habe.** Ein Staufenster, das bei eins klebt, und ein
// Sendepuffer, der voll ist, sehen von aussen gleich aus —
// und verlangen das Gegenteil voneinander.
```

## L2614-2615 · `let t_p = crate::interrupts::rdtsc();`

```
// Drive the stack so ACKs come in and the retransmit buffer is trimmed
// (send() has no flow control, so without this the send_buf grows).
```

## L2648-2650 · `kprintln!("[netbench] je Segment: {} ns in send, {} ns Wanduhr",`

```
// Die eine Zahl, die sagt, ob der Erzeuger der Deckel ist.
// 1448 Byte je Segment bei 42 Mbit sind 275 us - und alles,
// was ein Segment WIRKLICH kostet, steht hier.
```

## L2657 · `let mut raw = alloc::vec::Vec::new();`

```
// Read the server's response (it measured the receive rate).
```

## L2669 · `let body = match raw.windows(4).position(|w| w == b"\r\n\r\n") {`

```
// Return the body after the header (best-effort).
```

## L2677-2682 · `fn user_download_streaming(`

```
/// Streaming download for the USER `http`/`https` intents — follows redirects
/// across BOTH schemes, including https→http downgrade (which OTA's strict
/// `https_get` refuses on purpose). This lets `https cdimage.debian.org/…iso`
/// chase its 302 to a fast plain-http mirror — the user's confirmed gigabit
/// source — instead of dead-ending. `start_tls` = first hop's scheme.
/// NOT for OTA (that path stays strict, signatures aside).
```

## L2700 · `200..=299 => return Ok(0), // bytes already counted by the caller's sink`

```
// bytes already counted by the caller's sink
```

## L2717-2718 · `fn parse_any_url(loc: &str, current_host: &str, current_tls: bool) -> Result<(String, String, bool), &'static str> {`

```
/// Permissive redirect-URL parser: accepts http://, https://, and absolute
/// paths. Returns (host, path, is_tls).
```

## L2742-2747 · `struct PlainConn {`

```
/// Eine offene Klartext-Verbindung, die auf ihre naechste Anfrage wartet.
///
/// Der TLS-Pool daneben haelt `TlsSession`s; hier ist es der nackte
/// TCP-Griff. Getrennt zu halten ist kein Duplikat, sondern der Unterschied
/// zwischen den beiden Protokollen: eine TLS-Sitzung hat einen Zustand, der
/// mitgeschleppt werden muss, ein TCP-Griff ist eine Zahl.
```

## L2749-2751 · `key: String,`

```
/// `host:port` — der Port GEHOERT dazu. Zwei Dienste auf derselben
/// Maschine sind zwei Gegenstellen, und ein Griff, der beim falschen
/// landet, schickt die Anfrage an den falschen Server.
```

## L2760-2763 · `fn plain_take(key: &str) -> Option<usize> {`

```
/// Einen lebenden Klartext-Griff fuer `key` holen. Dieselbe Vorsicht wie im
/// TLS-Pool: erst den NIC-Ring leeren, dann den Zustand lesen — sonst liegt
/// das FIN der Gegenstelle unverarbeitet im Ring und `conn_healthy` nickt
/// eine tote Verbindung durch.
```

## L2798-2806 · `fn http_get_once(`

```
/// One plain-HTTP round-trip (no TLS, no redirect follow). Mirrors
/// `https_get_once` but over raw TCP. Content-Length bodies only.
///
/// **Die Verbindung wird wiederverwendet.** Bis 0.322.0 stand hier
/// `Connection: close`, und JEDE Anfrage baute neu auf — am Geraet gegen den
/// eigenen Vorlagenserver waren das zwei volle Handshakes je Seitenaufbau,
/// einer fuers Dokument und einer fuers Stilblatt, fuer nichts. Der Weg ist
/// derselbe wie im TLS-Pool: erst einen gepoolten Griff versuchen, und was
/// dort schiefgeht, BEVOR ein Byte ausgeliefert wurde, wird frisch wiederholt.
```

## L2817 · `if let Some(handle) = plain_take(&key) {`

```
// Versuch 1: eine offene Verbindung. Kein DNS, kein ARP, kein Handshake.
```

## L2821 · `Err(ExchangeErr::Retry) => {} // abgestanden — unten frisch`

```
// abgestanden — unten frisch
```

## L2826 · `let ip = match parse_ip(bare) {`

```
// Versuch 2: frisch aufbauen.
```

## L2833 · `let _ = crate::net::arp::resolve(gw, 100); // see open_tls: not a blind spin`

```
// see open_tls: not a blind spin
```

## L2844-2847 · `fn http_exchange(`

```
/// Eine Anfrage ueber einen schon offenen Griff. `Retry` heisst: es ist nichts
/// ausgeliefert worden, der Rufer darf frisch aufbauen und es nochmal
/// versuchen. Alles, was nach dem ersten `on_chunk` schiefgeht, ist `Fatal` —
/// ein zweiter Versuch wuerde dieselben Bytes ein zweites Mal liefern.
```

## L2856-2859 · `crate::interrupts::set_worker_poll_hz(10_000);`

```
// Timer-NAPI: speed this worker core's idle timer to ~10 kHz for the whole
// transfer so the recv loop's HLT wakes every ~100 µs (vs 10 ms at 100 Hz) —
// low-latency polling without burning the core. The guard restores 100 Hz on
// every exit path (success, error, redirect).
```

## L2877-2882 · `let mut buf = alloc::vec![0u8; 512 * 1024];`

```
// Large HEAP read buffer (a 512 KiB stack array would overflow the kernel
// stack). recv() returns at most buf.len() per call; the old 17 KB cap made
// the consumer drain far slower than poll_rx_only bulk-fills recv_buf (it
// empties the whole NIC ring per call) → recv_buf climbed to the 8 MiB
// window cap → window 0 → the sender stalled (measured rxbuf_max≈8191 KiB).
// A big drain per call keeps recv_buf near-empty → window stays open.
```

## L2904-2905 · `None => { let _ = crate::net::tcp::close(handle); return Err(ExchangeErr::Retry); }`

```
// Nichts oder nur Bruchstuecke: auf einer wiederverwendeten Verbindung
// ist das der Normalfall einer still geschlossenen Gegenstelle.
```

## L2912-2913 · `let reply_headers = capture_headers(hdr_str);`

```
// Everything after the status line, capped. `Set-Cookie` repeats, so
// handing back parsed single values could never carry it.
```

## L2926-2928 · `let _ = crate::net::tcp::close(handle);`

```
// Der Koerper einer Weiterleitung wird nicht gelesen, also steht die
// Verbindung nicht mehr auf einer Nachrichtengrenze. Sie zurueckzulegen
// hiesse, die naechste Anfrage mit fremden Bytes zu beantworten.
```

## L2960-2964 · `let clean = delivered == cl && cl <= max_size && leading.len() <= cl;`

```
// Zurueckgelegt wird NUR, was nachweislich auf einer Nachrichtengrenze
// steht: der ganze Koerper geliefert, nichts abgeschnitten, nichts
// uebriggelassen — und die Gegenstelle hat nicht `close` gesagt. Jede
// andere Verbindung traegt Reste, und die naechste Anfrage bekaeme sie als
// Antwort.
```

## L2974 · `fn parse_status_code(headers: &str) -> Option<u16> {`

```
/// Parse HTTP status code from first header line.
```

## L2976 · `let first_line = headers.lines().next()?;`

```
// "HTTP/1.1 200 OK" → 200
```

## L2979 · `parts.next()?; // "HTTP/1.1"`

```
// "HTTP/1.1"
```

## L2983 · `fn parse_header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {`

```
/// Find a header value by name (case-insensitive).
```

## L2995 · `enum ChunkSt {`

```
/// Chunked-decoder state, carried across TLS-record boundaries.
```

## L2997 · `Size,`

```
/// Accumulating the `<hex>[;ext]\r\n` size line.
```

## L2999 · `Data(usize),`

```
/// Inside a chunk's payload; `usize` bytes still to copy.
```

## L3001 · `AfterCr,`

```
/// Expecting the `\r` of the CRLF that follows chunk data.
```

## L3003 · `AfterLf,`

```
/// Expecting the `\n` of that CRLF.
```

## L3005 · `Done,`

```
/// Saw the 0-size chunk — body complete.
```

## L3009-3013 · `fn chunked_feed(`

```
/// Consume one input slice (header `leading` bytes, then each TLS
/// record), advancing the decoder and pushing payload slices to
/// `on_chunk`. Zero-copy: payload is handed out as sub-slices of
/// `input` — no intermediate buffer, no `Vec::drain`. Returns
/// `Ok(true)` once the terminal 0-chunk is seen.
```

## L3026 · `let mut j = i;`

```
// Accumulate up to (not including) the next '\n'.
```

## L3036 · `i = j + 1;`

```
// input[j] == '\n' — the size line is complete.
```

## L3041 · `let hex_end = size_line`

```
// Chunk extensions (";name=val") are ignored.
```

## L3058 · `i = j;`

```
// Ran out of input mid-line; resume next record.
```

## L3079-3082 · `if input[i] == b'\r' {`

```
// The byte should be '\r'; consume it if so. Either
// way move on — a non-CR here on valid chunked never
// happens, and tolerating it can't desync because
// the size-line parser re-validates.
```

## L3100-3116 · `fn stream_chunked_body(`

```
/// True streaming chunked-transfer decoder (RFC 7230 §4.1).
///
/// Parses the chunk-size framing exactly and pushes only decoded
/// payload bytes through `on_chunk` as they arrive. Linear and
/// zero-copy: each TLS record is walked in place and payload is
/// handed out as sub-slices — there is NO growing carry buffer and
/// NO `Vec::drain`. (The previous implementation drained from the
/// front of a `Vec` that it also extended at the back, which is
/// O(n²) over a transfer: fine for a 13 KB repo, ~31 KiB/s by
/// 500 KB, effectively dead at 250 MB. The Content-Length path was
/// always ~100× faster purely because it never did this.)
///
/// The only state kept across records is the decoder enum plus a
/// small `size_line` accumulator (a chunk-size line that straddles
/// a record boundary — capped at 64 bytes).
///
/// Returns the number of payload bytes delivered.
```

## L3136 · `Ok(0) => continue, // transient; tls_recv_poll caps the wait`

```
// transient; tls_recv_poll caps the wait
```

