# `kernel/src/net/tcp.rs` @ 5e0102684

## L1-8 · `use alloc::vec::Vec;`

```
//! TCP — Transmission Control Protocol
//!
//! nopeekOS-optimized defaults:
//! - No Nagle (low latency for request/response)
//! - 40ms delayed ACK (not 200ms)
//! - Initial window: 10 segments
//! - 3 retries, max 10s timeout (fast failure)
//! - Capability-gated: no cap = no connection
```

## L17-21 · `static ISN_SECRET: Once<[u8; 32]> = Once::new();`

```
// RFC 6528 — Initial Sequence Number generation. Predictable ISNs (e.g. a
// raw tick counter) let an off-path attacker forge in-window segments on a
// listening socket. We mix a per-boot CSPRNG secret with the connection
// 4-tuple via BLAKE3-keyed-hash, then add a tick-derived monotonic counter
// so retried connections still grow forward.
```

## L37-39 · `let timer = (crate::interrupts::ticks() as u32).wrapping_mul(2500);`

```
// Monotonic component: 100 Hz tick × 2500 ≈ 4 µs ISN step (RFC 6528 §3
// suggests a ~250 kHz clock). Wrap is fine — the secret-keyed hash
// ensures the absolute value is unguessable per 4-tuple.
```

## L44-47 · `const MAX_CONNECTIONS: usize = 128;`

```
// A single LibreWolf page load opens ~20+ parallel TLS connections
// (CDNs, telemetry, OCSP, …). 16 was a single-`https`-intent ceiling;
// the browser exhausts it instantly → connects fail / stall →
// PR_IO_TIMEOUT_ERROR. 128 matches the NAT session table.
```

## L49 · `const MSS: u16 = 1460; // standard Ethernet MSS`

```
// standard Ethernet MSS
```

## L50 · `const INITIAL_WINDOW: u16 = 65535;`

```
// The SYN/SYN-ACK window is never scaled (RFC 7323), so it's capped at 16-bit.
```

## L52-59 · `const OUR_WSCALE: u8 = 8;`

```
// TCP Window Scaling (RFC 7323). Without it the window is capped at 64 KiB and
// throughput = 64 KiB / RTT (~4 MB/s on a CDN regardless of link/NIC — the
// observed global slowness). We advertise `free >> OUR_WSCALE`.
// WSCALE 8 so the 16-bit window field can express the full 8 MiB buffer
// (8 MiB >> 8 = 32768 ≤ 65535). History: WSCALE 5 / 1 MiB capped a flow at
// ~727 Mbit; WSCALE 7 / 4 MiB reached ~650 avg but never plateaued (4 MiB ≈
// the BDP to a ~35 ms-RTT mirror = throughput·RTT, so zero headroom → any RTT
// jitter underfills). 8 MiB = ~2× BDP headroom → fill the pipe to ~native.
```

## L65-72 · `static WND_LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);`

```
// **`netdev::active_rx_rate()` wird hier nicht mehr gelesen.** Bis
// 0.402.0 kam der Deckel aus „Leitungsrate x geglaettete RTT" — und die
// Leitungsrate ist eine Zahl, die jeder Treiber UEBER SICH SELBST
// behauptet (in `rtl8153.rs` stehen 20 MB/s, gemessen auf einem anderen
// Blech und auf diesem nie nachgeprueft). Seit 0.403.0 misst DRS, was
// die ANWENDUNG pro RTT wirklich abholt; die Rate wird dafuer nicht
// gebraucht. Die Funktion bleibt, weil `netdev` sie fuer die
// Schnittstellenauswahl fuehrt.
```

## L74-78 · `static WND_LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);`

```
/// Advertised receive window = min(free buffer, DRS window).
/// Was `recv_window` zuletzt gerechnet hat: (angebotenes Fenster in Bytes,
/// srtt in MILLISEKUNDEN, Deckel in Bytes). **Die Frage, die eine Messung auf
/// einer langsamen Leitung stellt, ist nicht „wieviel kam an", sondern „wieviel
/// haben WIR angeboten" — und wovon der Deckel kam.**
```

## L83 · `pub fn window_diag() -> (u32, u32, u32) {`

```
/// (Fenster B, srtt ms, Deckel B) der letzten Berechnung.
```

## L89-95 · `static RCV_WND_FORCE: core::sync::atomic::AtomicU32 =`

```
/// Handfester Deckel fuer das angebotene Fenster in BYTES, 0 = aus.
///
/// **Ein Werkzeug, kein Schalter.** Bei gesaettigter Strecke gilt
/// `RTT = Fenster / Rate`, also stehen Fenster und RTT im Gleichschritt und
/// EINE Messung sagt nicht, ob die Luft oder wir der Deckel sind. Das sagt
/// nur die FORM der Kurve ueber mehrere Fenster: steigt der Durchsatz mit,
/// waren wir es; bleibt er stehen und nur die RTT waechst, ist es die Luft.
```

## L99 · `pub fn set_rcv_window_force(bytes: u32) {`

```
/// `net window <KB>` setzt den Deckel, `net window auto` nimmt ihn weg.
```

## L104 · `pub fn rcv_window_force() -> u32 {`

```
/// Aktueller handfester Deckel in Bytes (0 = aus).
```

## L109-116 · `fn ts_now_ms() -> u32 {`

```
/// Unsere Zeitmarke fuer die TCP-Timestamp-Option, in Millisekunden.
///
/// **Hier stand `ticks()`, also 100 Hz.** Der Rueckweg `jetzt - TSecr`
/// ist damit eine RTT in 10-ms-Stufen: 5 ms messen sich als null, 20 ms
/// als ein bis zwei Stufen. Linux tickt seine Marke mit
/// `TCP_TS_HZ = 1000` (tcp.h), und RFC 7323 §4 laesst alles zwischen
/// 1 ms und 1 s zu — schneller waere falsch, weil PAWS den Umlauf
/// braucht (2^32 ms sind 49 Tage, 2^32 us waeren 71 Minuten).
```

## L121-127 · `fn rcv_rtt_update(conn: &mut TcpConn, sample_us: u32) {`

```
/// tcp_input.c:812 `tcp_rcv_rtt_update`.
///
/// **Das MINIMUM zaehlt, nicht der Mittelwert.** Steht der neue Wert
/// unter dem alten, gilt er sofort; sonst wird geglaettet — und auch das
/// nur, wenn die Empfangsschlange LEER ist. Liegt dort noch etwas, misst
/// die Probe, wie schnell unsere Anwendung liest, nicht wie schnell die
/// Strecke ist.
```

## L141-146 · `fn rcvbuf_grow(conn: &mut TcpConn, newval: u32) {`

```
/// tcp_input.c:894 `tcp_rcvbuf_grow`.
///
/// `tcp_space_from_win`/`tcp_win_from_space` fallen weg: sie rechnen bei
/// Linux das Verhaeltnis `skb->len / skb->truesize` heraus, also den
/// Verschnitt der Paketpuffer. Unser `recv_buf` haelt ROHE Bytes — das
/// Verhaeltnis ist eins, die Umrechnung die Identitaet.
```

## L151 · `let mut rcvwin = (newval as u64) << 1;`

```
// „DRS is always one RTT late."
```

## L153 · `let grow = rcvwin * (newval.saturating_sub(oldval)) as u64 / oldval as u64;`

```
// „slow start: allow the sender to double its rate."
```

## L156 · `rcvwin += conn.ooo.values().map(|v| v.len() as u64).sum::<u64>();`

```
// Was ausser der Reihe liegt, braucht zusaetzlich Platz.
```

## L160 · `if rcvbuf > conn.drs_win {`

```
// **Nur wachsen.** tcp_input.c:922.
```

## L166-167 · `fn rcv_space_adjust(conn: &mut TcpConn) {`

```
/// tcp_input.c:933 `tcp_rcv_space_adjust` — gerufen, sooft die Anwendung
/// gelesen hat.
```

## L171 · `if conn.rcv_rtt_us8 == 0 || time < (conn.rcv_rtt_us8 >> 3) as u64 {`

```
// Ueber weniger als eine RTT sagt die Messung nichts.
```

## L176-178 · `let copied = copied.saturating_sub(conn.recv_buf.len() as u64);`

```
// **Was noch in der Schlange liegt, wird abgezogen.** Staut es sich,
// ist die ANWENDUNG der Engpass, und ein groesseres Fenster hilft ihr
// nicht (tcp_input.c:948-949).
```

## L187-206 · `fn sndbuf_grow(conn: &mut TcpConn, newval: usize) {`

```
/// tcp_input.c:587 `tcp_sndbuf_expand` — das Gegenstueck zu
/// `rcvbuf_grow`.
///
/// **Linux rechnet den Sendepuffer aus dem STAUFENSTER**:
/// `2 * max(TCP_INIT_CWND, snd_cwnd, reordering+1) * per_mss`, gedeckelt
/// durch `tcp_wmem[2]`. Der Faktor 2 steht dort mit Begruendung — CUBIC
/// braucht 1,7, aufgerundet, plus Polster fuer eine Anwendung, die
/// langsam auf `EPOLLOUT` reagiert.
///
/// **Uns fehlt `snd_cwnd`, also fehlt Linux' Eingang in diese Formel.**
/// Das ist hier benannt und nicht umschifft: wir haben keine
/// Staukontrolle. Was wir stattdessen haben, ist dieselbe Groesse
/// GEMESSEN statt gerechnet — wieviel in einer Umlaufzeit wirklich
/// quittiert wurde. Der Faktor 2 bleibt Linux'.
///
/// Und `tcp_should_expand_sndbuf` (tcp_input.c:5804) uebersetzt sich
/// nicht: sein Gatter ist „wenn wir das Staufenster gefuellt haben,
/// nicht wachsen". Bei uns gibt es keins — der Puffer IST die einzige
/// Bremse, und genau das hat die Messung gezeigt (4,5 Mio WouldBlock bei
/// 0,85 % belegter Luft).
```

## L210 · `if want > conn.snd_buf_limit {`

```
// **Nur wachsen** — dieselbe Regel wie tcp_input.c:922.
```

## L216-217 · `fn snd_space_adjust(conn: &mut TcpConn) {`

```
/// Der Spiegel von `rcv_space_adjust`: gerufen, sooft eine Quittung
/// Daten abgeraeumt hat.
```

## L222-223 · `if srtt_us == 0 || time < srtt_us {`

```
// Ueber weniger als eine Umlaufzeit sagt die Messung nichts —
// dieselbe Schranke wie auf der Empfangsseite.
```

## L235-237 · `fn snd_allowed(conn: &TcpConn) -> usize {`

```
/// Wieviel darf unterwegs sein: unser Puffer UND das Fenster des
/// Gegenuebers. Vor 0.405.0 stand hier nur `MAX_UNACKED`, und das zweite
/// gab es gar nicht.
```

## L240-241 · `SND_BUF_INIT`

```
// Vor der ersten Quittung wissen wir es nicht. Ein Nullfenster
// NACH dem Handschlag ist dagegen echt und bremst uns richtig.
```

## L251-252 · `let cap = if forced > 0 {`

```
// **`window <KB>` bleibt, und es bleibt ein WERKZEUG.** Ohne den
// Deckel gilt DRS; mit ihm misst man, was DRS haette finden sollen.
```

## L272 · `fn ooo_runs_add(runs: &mut BTreeMap<u32, u32>, s: u32, e: u32) {`

```
/// Merge [s,e) into the coalesced out-of-order run set (offsets from rcv_irs).
```

## L276 · `if let Some((&ls, &le)) = runs.range(..s).next_back() {`

```
// Absorb a contiguous/overlapping left neighbour (greatest start < s).
```

## L280 · `let keys: alloc::vec::Vec<u32> = runs.range(s..=e).map(|(&k, _)| k).collect();`

```
// Absorb every run starting within [s, e] (overlap or adjacency).
```

## L289 · `fn ooo_runs_trim(runs: &mut BTreeMap<u32, u32>, want: u32) {`

```
/// Drop/trim runs now delivered (everything below offset `want`).
```

## L299 · `const RETRY_TICKS_BASE: u64 = 100; // 1 second (100Hz)`

```
// 1 second (100Hz)
```

## L300-302 · `const ARP_RETRANS_TICKS: u64 = 5; // 50 ms`

```
// Cold-cache ARP while a SYN waits: one WiFi round trip between probes, and a
// total budget matching the old blocking pre-resolve (~500 ms) before the SYN
// goes out to broadcast regardless.
```

## L303 · `const ARP_RETRANS_TICKS: u64 = 5; // 50 ms`

```
// 50 ms
```

## L305 · `const FIN_TIMEOUT_TICKS: u64 = 6000; // 60 s, like Linux's tcp_fin_timeout`

```
// 60 s, like Linux's tcp_fin_timeout
```

## L306-308 · `const RTO_TICKS_BASE: u64 = 20; // 200 ms = Linux TCP_RTO_MIN (HZ/5)`

```
// Retransmit timeout for DATA. Base 200 ms, doubled per attempt (RFC 6298
// style), give up after MAX_DATA_RETRIES, then the connection is honestly
// dead instead of silently one-way.
```

## L309 · `const RTO_TICKS_BASE: u64 = 20; // 200 ms = Linux TCP_RTO_MIN (HZ/5)`

```
// 200 ms = Linux TCP_RTO_MIN (HZ/5)
```

## L310-318 · `const MAX_DATA_RETRIES: u8 = 15;`

```
/// Retransmissions before an established connection is declared dead.
/// Linux's `TCP_RETR2` is 15 (include/net/tcp.h:119); mine was 5, chosen
/// without reference when the retransmit engine went in — about 6 s with the
/// backoff below. Six seconds is nothing on a WiFi link carrying a saturating
/// download: measured on the device, the `debug` mirror died with
/// "no ACK for ~6 s" in the middle of a 1 GB transfer that itself completed
/// fine. A stalled OTA connection was given the same six seconds.
/// With the shift capped at 5 the RTO tops out at 6.4 s, so 15 attempts span
/// roughly 70 s — patient, and still bounded.
```

## L320-337 · `const SND_BUF_INIT: usize = 256 * 1024;`

```
// Ceiling on unacknowledged bytes held for retransmit. A peer that stops
// acknowledging must not grow this without bound; `send` refuses past it,
// which is the backpressure the caller needs to see.
/// Womit ein Sendepuffer anfaengt, bevor eine Messung vorliegt.
///
/// **Hier standen `20 * 1460` — TCP_INIT_CWND mal Linux' Faktor 2 — und
/// das hat den Upload totgelegt.** `tcp::send` ist alles-oder-nichts,
/// und `http_post_zeros` uebergibt Stuecke von 64 KiB: 65536 passt nie
/// in 29200, in keinem Zustand. Wachsen konnte der Puffer nicht, weil
/// Wachstum an Quittungen fuer Daten haengt, die nie hinausgingen.
///
/// Die Regel daraus: **ein Anfangswert unter der Stueckgroesse des
/// Rufers ist ein Stillstand, kein langsamer Start.** Linux hat das
/// Problem nicht, weil `tcp_sendmsg` nimmt, was hineinpasst, und eine
/// KURZE Schreibung meldet; unsere Schnittstelle kann das nicht.
///
/// Also der alte Deckel als Anfang. Zusammen mit „nur wachsen" heisst
/// das: nie schlechter als vor 0.405.0.
```

## L339-359 · `const SND_BUF_MAX: usize = 256 * 1024;`

```
/// Der Deckel.
///
/// **Linux nimmt hier `sysctl_tcp_wmem[2]` = 4 MB. Wir nicht, und das
/// ist bewusst.**
///
/// Am Geraet gemessen (2026-09-22): mit 4 MB wuchs der Puffer auf
/// 1-2,6 MB, waehrend die Strecke rund 62 KB traegt (250 Mbit x 2 ms).
/// Das Vierzigfache des Bandbreiten-Verzoegerungs-Produkts ging auf die
/// Leitung, die Puffer dazwischen liefen ueber — und **unsere Erholung
/// kann das nicht bezahlen**: ohne SACK ist jeder Verlust ein RTO,
/// `cwnd` faellt auf 2, und mit zwei Paketen unterwegs gibt es nie die
/// drei Doppelquittungen, die eine schnelle Wiederholung braucht.
/// Gemessen: 45 Zeitueberschreitungen in zehn Sekunden, 1,3 Mbit.
///
/// Linux kann sich 4 MB leisten, weil darunter SACK, PRR und Limited
/// Transmit stehen. Solange die fehlen, ist der Deckel die Grenze, die
/// sie ersetzt — und 256 KB ist der Wert, mit dem diese Strecke
/// nachweislich 269 Mbit geliefert hat.
///
/// **Das ist ein Deckel aus einer MESSUNG, nicht aus dem Bauch**, und er
/// hat ein Ablaufdatum: er gehoert angehoben, sobald SACK steht.
```

## L362-367 · `const SEND_SPIN_BUDGET: u32 = 4096;`

```
/// Wieviele leere Blicke auf `ACK_GEN`, bevor wir abgeben.
///
/// Die Quittung kommt vom Treiber-Fiber auf einem ANDEREN Kern; hier zu
/// warten heisst, sie um einen Planertakt zu verpassen. 4096 Umlaeufe
/// sind wenige Mikrosekunden und damit kuerzer als jede Umlaufzeit, die
/// wir je gemessen haben.
```

## L370-398 · `const CONG_CONTROL: bool = false;`

```
/// **Die Staukontrolle ist AUS — und das ist eine Messung, kein
/// Geschmack.**
///
/// Sie braucht eine Buchfuehrung, die wir nicht haben. Linux rechnet
/// `tcp_packets_in_flight = packets_out - sacked_out - lost_out +
/// retrans_out` und senkt `sacked_out` bei JEDER Doppelquittung
/// (`tcp_add_reno_sack`), auch ohne SACK. Bei uns ist „unterwegs"
/// schlicht `snd_nxt - snd_una`, und **diese Zahl schrumpft bei Verlust
/// nie**.
///
/// Was daraus folgt, am Geraet gemessen (2026-09-22): nach dem ersten
/// verlorenen Segment steht `snd_nxt` weit vorn — 199 KB, also 137
/// Pakete —, `cwnd` faellt auf 1, und `write_xmit` sendet ab da NIE
/// wieder etwas, weil `in_flight >= cwnd`. Das Einzige, was sich noch
/// bewegt, ist eine Wiederholung je RTO: ein Segment pro 200 ms.
/// `cwnd 1 · ssthresh 2 · 121x Zeitueberschreitung`, 10 Mbit.
///
/// **Ohne Verlust-Buchfuehrung ist ein Staufenster schlimmer als
/// keines.** Ohne sie (0.405.0) lief dieselbe Strecke mit 269 Mbit
/// durch, weil ein verlorenes Segment nur 200 ms kostete und der Rest
/// weiterlief.
///
/// Die schnelle Wiederholung bleibt an: drei Doppelquittungen holen das
/// fehlende Segment sofort statt nach 200 ms, und das kostet nichts.
/// Aus ist nur das, was das Fenster ZUSAMMENZIEHT.
///
/// **Anschalten, wenn und nur wenn das hier steht**, in dieser
/// Reihenfolge: `packets_out`/`sacked_out`/`lost_out` als echte Zaehler,
/// `tcp_add_reno_sack`, SACK auf der Sendeseite, dann PRR.
```

## L401 · `const TCP_INIT_CWND: u32 = 10;`

```
/// `TCP_INIT_CWND` — zehn Segmente (RFC 6928).
```

## L403-404 · `const DUPACK_THRESH: u32 = 3;`

```
/// `tp->reordering` in seiner Vorgabe: drei Doppelquittungen loesen die
/// schnelle Wiederholung aus (RFC 5681 §3.2).
```

## L407-409 · `const RETRY_ROUNDS: u32 = 16;`

```
/// Nach wievielen Abgabe-Runden ohne Weckung trotzdem ein neuer Versuch
/// gemacht wird. Kostet im Normalfall nichts (die Weckung kommt lange
/// vorher) und macht aus einem toten Warten ein langsames.
```

## L411-416 · `const RECV_BUF_SIZE: usize = 8 * 1024 * 1024;`

```
// 4 MiB receive buffer → ~4 MiB window with scaling → fills the bandwidth-delay
// product for ~gigabit even at tens-of-ms RTT (1 MiB was the cap at ~11 ms;
// higher-RTT CDNs need more). Grown lazily (VecDeque::new), so an idle
// connection costs nothing and only an actively-bursting one approaches 4 MiB.
// Host TCP only ever has a handful of live connections (OTA/https/dns), so the
// worst-case footprint is small; the guest browser uses its own (microvm) TCP.
```

## L418 · `const DELAYED_ACK_TICKS: u64 = 4; // 40ms at 100Hz`

```
// 40ms at 100Hz
```

## L419-423 · `const ACK_COALESCE: u16 = 8;`

```
// ACK coalescing: send one ACK per N in-order segments (a held ACK is still
// flushed by the 40 ms timer). 8 ≈ one ACK per ~11.7 KB at 1460 MSS, cutting
// our TX-ACK packet rate ~4× (38500→9600/s at 850 Mbit) — fewer packets through
// the single-threaded path (QEMU slirp) and less work on the busy-spin RX core.
// Safe now that timestamps give the sender a per-segment RTT regardless.
```

## L425-427 · `const OOO_MAX_BYTES: usize = 2 * 1024 * 1024;`

```
// Cap on buffered out-of-order data per connection. Beyond this, new
// ahead-segments are dropped (the sender will retransmit) so a lossy link
// can't blow up the heap.
```

## L430-433 · `static TCP_OOO_AHEAD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);`

```
// Out-of-order receive counters (diagnostic). `AHEAD` = a segment past rcv_nxt
// (a gap → the sender will have to retransmit); `BEHIND` = a duplicate at/below
// rcv_nxt (a retransmit we already have). A burst of AHEAD during a download =
// packet loss + go-back-N. Read+reset via take_ooo_stats().
```

## L437 · `pub fn take_ooo_stats() -> (u32, u32) {`

```
/// (ahead, behind) out-of-order segment counts since the last call; resets both.
```

## L443-446 · `static TCP_MAX_RXBUF: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);`

```
// Max recv_buf depth seen since last read (diagnostic). High (→RECV_BUF_SIZE) =
// our consumer/core can't drain fast enough → window closes → sender stalls
// (consumer-limited). Low = buffer drains fine → a tail slowdown is the sender
// throttling (bufferbloat backoff), not us.
```

## L449 · `pub fn take_max_rxbuf() -> usize {`

```
/// Max recv-buffer depth (bytes) seen since the last call; resets to 0.
```

## L454-455 · `static TCP_TX_SEGS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);`

```
// Segments we transmitted (mostly ACKs) — diagnostic. A bulk download flooding
// one ACK per packet shows up here as ~tens of thousands/s.
```

## L458 · `pub fn take_tx_segs() -> u32 {`

```
/// Count of connection-originated segments sent since the last call; resets.
```

## L463 · `const FIN: u8 = 0x01;`

```
// TCP flags
```

## L470 · `const HEADER_LEN: usize = 20; // no options (options added separately for SYN)`

```
// no options (options added separately for SYN)
```

## L494 · `snd_nxt: u32, // next byte to send`

```
// Sequence numbers
```

## L495 · `snd_nxt: u32, // next byte to send`

```
// next byte to send
```

## L496 · `snd_una: u32, // oldest unacknowledged`

```
// oldest unacknowledged
```

## L497 · `snd_iss: u32, // initial send seq`

```
// initial send seq
```

## L498 · `rcv_nxt: u32, // next expected from remote`

```
// next expected from remote
```

## L499 · `rcv_irs: u32, // initial recv seq`

```
// initial recv seq
```

## L501 · `recv_buf: VecDeque<u8>,`

```
// Buffers
```

## L504-509 · `ooo: BTreeMap<u32, Vec<u8>>,`

```
// Out-of-order reassembly: segments received ahead of a gap, keyed by
// stream offset (seq - rcv_irs). Without this a single lost packet forced
// the sender into go-back-N (retransmit the whole window) which re-burst
// and re-overflowed the USB-NIC FIFO → collapse. With it only the one lost
// segment is retransmitted. Bounded by OOO_MAX_BYTES (else dropped → the
// sender retransmits). Offsets assume < 4 GiB per connection.
```

## L512-516 · `ooo_runs: BTreeMap<u32, u32>,`

```
// Coalesced [start,end) runs of `ooo`, kept in sync — so building SACK
// blocks is O(runs), not an O(n)-segments full-map scan per ACK (that cost
// ~38µs/pkt once a large window let `ooo` reach thousands of entries and
// collapsed the pipeline). Advisory: a desync only makes SACK suboptimal,
// never corrupts data (the bytes still come from `ooo`).
```

## L518-519 · `srtt_ms: u32,`

```
// Smoothed RTT in MILLISECONDS, from the peer's echoed TSecr. Kept as a
// diagnostic only; the advertised window comes from DRS below.
```

## L522-544 · `rcv_rtt_us8: u32,`

```
// ── DRS: Dynamic Right Sizing (Linux `tcp_rcv_space_adjust`) ────────
//
// **Der Empfaenger misst, wieviel die ANWENDUNG pro RTT wirklich
// abholt, und leitet das Fenster daraus ab.** Keine Leitungsrate,
// keine Konstante. Hier stand bis 0.403.0 `rate × srtt`, mit der Rate
// aus `netdev::active_rx_rate()` — einer Zahl, die jeder Treiber ueber
// sich selbst BEHAUPTET — und einer RTT in 10-ms-Stufen. Damit war
// `window 1024` von Hand noetig, und Florian hat recht: das ist eine
// Notloesung, keine Loesung.
//
// Drei Regeln aus tcp_input.c, und alle drei haben einen Grund:
//
// * **Es waechst nur** (`if (rcvbuf > sk->sk_rcvbuf)`, tcp_input.c:922,
//   und `if (copied <= space) goto new_measure`, :950). Ein Fenster,
//   das schrumpfen darf, geraet in eine Spirale — weniger Fenster,
//   weniger Durchsatz, weniger gemessener Bedarf.
// * **Die RTT kommt aus dem MINIMUM**, nicht aus dem geglaetteten Wert
//   (`if (old_sample == 0 || m < old_sample)`, :817). Der geglaettete
//   misst den eigenen Stau mit — ein Regelkreis mit positivem
//   Vorzeichen.
// * **Keine RTT-Probe, solange die Empfangsschlange nicht leer ist**
//   (`if (tp->rcv_nxt != tp->copied_seq) return`, :833). Sonst misst
//   man die eigene Anwendung statt der Strecke.
```

## L546-547 · `rcv_rtt_us8: u32,`

```
/// `rcv_rtt_est.rtt_us` — in ACHTELN einer Mikrosekunde, wie Linux
/// (`long m = sample << 3`).
```

## L549-551 · `rcvq_copied0: u64,`

```
/// `rcvq_space.seq` — wieviel die Anwendung beim letzten Messpunkt
/// insgesamt abgeholt hatte. Wir zaehlen absolut statt in
/// Sequenznummern; dasselbe Delta.
```

## L553 · `copied_total: u64,`

```
/// Wieviel sie insgesamt abgeholt hat (`copied_seq`).
```

## L555 · `rcvq_time_us: u64,`

```
/// `rcvq_space.time`
```

## L557 · `rcvq_space: u32,`

```
/// `rcvq_space.space` — der gemessene Bedarf einer RTT.
```

## L559 · `drs_win: u32,`

```
/// `sk_rcvbuf` — das Fenster, das DRS erlaubt.
```

## L562-570 · `retries: u8,`

```
// Retransmit. `send_buf` holds every byte we sent and the peer has not
// acknowledged, starting at `snd_una`; `rto_tick` is when the oldest of
// them went out. Without this a single lost segment was lost FOREVER:
// the peer keeps a hole it can never fill, buffers everything after it
// out-of-order and delivers nothing more to its application, while our
// side happily reports every send as a success. Invisible for browsing
// (there the PEER retransmits to us and our own sends are one short
// request), fatal for anything that streams outward — `debug` went mute
// at the first radio loss while its keyboard direction kept working.
```

## L575-578 · `dsack: Option<(u32, u32)>,`

```
// Delayed ACK
/// Ein EINMALIGER D-SACK-Block (RFC 2883): der Bereich eines Segments,
/// das wir schon hatten. Wird beim naechsten Quittungsbau als ERSTER
/// SACK-Block ausgegeben und danach sofort geloescht.
```

## L582 · `acks_held: u16,`

```
// In-order segments received since our last ACK (ACK-coalescing counter).
```

## L584-585 · `freed_since_winupd: u32,`

```
// Bytes drained since our last recv()-side window-update ACK. Rate-limits
// those ACKs so a bulk download doesn't emit one per recv() call (~70k/s).
```

## L588 · `established: bool,`

```
// Connection complete flag
```

## L593-595 · `wscale_ok: bool,`

```
// Window scaling (RFC 7323). `wscale_ok` once both SYNs carried the
// option; `snd_wscale` is the peer's shift (to scale their advertised
// window). Our own advertised window is scaled by OUR_WSCALE.
```

## L598-600 · `snd_wnd: u32,`

```
/// **Das Fenster des Gegenuebers, skaliert** (RFC 9293 §3.8.6).
/// Bis 0.405.0 gab es dieses Feld nicht: `_window` wurde gelesen und
/// verworfen, und die einzige Bremse war `MAX_UNACKED`.
```

## L602-616 · `snd_buf_limit: usize,`

```
/// **Unser Sendepuffer, und er WAECHST** — das Gegenstueck zu
/// `drs_win` auf der Empfangsseite.
///
/// Linux fuehrt ihn in `tcp_sndbuf_expand` (tcp_input.c:587) aus dem
/// Staufenster nach: `2 * max(TCP_INIT_CWND, snd_cwnd, reordering+1)
/// * per_mss`, gedeckelt durch `tcp_wmem[2]`. Der Faktor 2 steht
/// dort mit Begruendung: CUBIC braucht 1,7, aufgerundet, plus
/// Polster fuer eine Anwendung, die langsam auf EPOLLOUT reagiert.
///
/// **Uns fehlt `snd_cwnd`, also fehlt Linux' Eingang in die
/// Formel** — das ist hier benannt und nicht versteckt. Was wir
/// haben, ist dieselbe Frage wie beim Empfangsfenster: haben wir den
/// Puffer im letzten Umlauf ganz gefuellt und ist die Strecke dabei
/// sauber geblieben? Dann ist er zu klein. Genau so waechst
/// `tcp_rcv_space_adjust`, und genau so waechst dieser hier.
```

## L618-619 · `acked_total: u64,`

```
/// `rcvq_space.seq` gespiegelt: wieviel das Gegenueber insgesamt
/// quittiert hat.
```

## L621 · `sndq_acked0: u64,`

```
/// Stand beim letzten Messpunkt.
```

## L623 · `sndq_time_us: u64,`

```
/// Zeitpunkt des letzten Messpunkts.
```

## L625 · `snd_space: usize,`

```
/// Der gemessene Bedarf EINER Umlaufzeit — `rcvq_space` gespiegelt.
```

## L628-636 · `snd_cwnd: u32,`

```
// ── Staukontrolle, RFC 5681 / RFC 6582 (New Reno) ───────────
//
// **Bis 0.406.0 gab es sie gar nicht.** `write_xmit` schob den
// GANZEN Sendepuffer auf einmal hinaus — bei 1,6 MB waren das 1100
// Segmente in einem Zug. Solange der Puffer fest auf 256 KB stand,
// ging der Burst gerade noch durch; sobald `tcp_sndbuf_expand` ihn
// wachsen liess, lief die Luft ueber, und die Erholung schickte EIN
// MSS je RTO. Am Geraet: 2070 Segmente in zehn Sekunden.
/// `tcp_snd_cwnd` — in PAKETEN, wie bei Linux.
```

## L638 · `snd_ssthresh: u32,`

```
/// `snd_ssthresh`. Anfangs unendlich: der erste Verlust setzt ihn.
```

## L640 · `snd_cwnd_cnt: u32,`

```
/// `snd_cwnd_cnt` — die Teilpakete aus `tcp_cong_avoid_ai`.
```

## L642 · `dupacks: u32,`

```
/// Wieviele Doppelquittungen in Folge.
```

## L644 · `in_recovery: bool,`

```
/// Ob wir in schneller Erholung sind (RFC 6582 „recover").
```

## L646-648 · `recovery_end: u32,`

```
/// `snd_nxt` beim Eintritt — erst darueber hinaus ist die Erholung
/// vorbei (RFC 6582 §3.2, sonst halbiert ein Verlustereignis das
/// Fenster mehrfach).
```

## L651-654 · `ts_ok: bool,`

```
// TCP Timestamps (RFC 7323). `ts_ok` once both SYNs carried the option;
// `ts_recent` = the peer's most recent in-order TSval, echoed as our TSecr
// so the sender measures RTT per-segment (robust to our ACK jitter) →
// accurate RTO → no spurious retransmits.
```

## L658-662 · `sack_ok: bool,`

```
// Selective ACK (RFC 2018). `sack_ok` once both SYNs carried SACK-permitted.
// As the receiver we then tell the sender which out-of-order ranges we
// already hold (straight from `ooo`), so it retransmits ONLY the real holes
// instead of everything past the cumulative ACK — the difference between a
// loss collapsing throughput and a one-segment recovery.
```

## L665-667 · `arp_pending: bool,`

```
// Next-hop MAC not yet known: the SYN is held back until ARP answers.
// Sending it to L2 broadcast instead is what most gateways drop, and the
// recovery is then a full 1 s SYN retry.
```

## L685-698 · `pub fn connect_start(remote_ip: [u8; 4], remote_port: u16) -> Result<usize, TcpError> {`

```
/// Open a TCP connection WITHOUT waiting for the handshake: the handle comes
/// back at once, the caller asks `connect_status` until it answers.
///
/// This is the form modules get. A blocking wait inside a host call freezes
/// every other fiber on that worker core — including the WiFi driver, whose
/// card then goes unpolled for the whole wait (the RB pool holds milliseconds).
/// `fiber::pump_peers` cannot cover it: it returns early when called from
/// inside a fiber, and a module IS a fiber.
///
/// The cold-cache ARP wait becomes part of the same state machine: we ask
/// once here and hold the SYN back (`arp_pending`) until `tick_connections`
/// sees the answer. Sending it to broadcast meanwhile is what most gateways
/// drop — the symptom was `debug <ip> <port>` needing 2–3 attempts on a
/// fresh boot unless a `ping` had warmed the cache.
```

## L703 · `let arp_target = super::ipv4::arp_target_for(remote_ip);`

```
// Non-blocking lookup — no CONNECTIONS lock held yet, but no waiting either.
```

## L728-731 · `rcvq_space: 10 * MSS as u32,`

```
// `tcp_init_buffer_space` setzt den Startwert aus dem, was eine
// frische Verbindung ohnehin anbietet. Zehn Segmente ist
// `TCP_INIT_CWND * advmss`; ohne Startwert teilt `rcvbuf_grow`
// durch null.
```

## L767 · `let handle = {`

```
// Find free slot
```

## L770-773 · `let slot = conns.iter()`

```
// Reclaim free OR fully-Closed slots. Without the Closed clause
// a Closed conn pins its slot forever (tick_connections only
// moves TimeWait→Closed, never frees it) — under browser churn
// every slot ends up a Closed corpse and connect() starves.
```

## L783 · `if !arp_pending { send_syn(handle)?; }`

```
// Only when the next hop is known. Otherwise `tick_connections` releases it.
```

## L789-795 · `pub fn connect_status(handle: usize) -> i32 {`

```
/// Connection state: 1 = usable, 0 = still handshaking, -1 = the peer hung up
/// cleanly, -2 = it FAILED (reset, or we ran out of retransmits — i.e. the
/// link stopped acknowledging).
///
/// The two negatives are worth separating: "the far end closed" and "the link
/// went dead under us" look identical to a caller that only sees failure, and
/// they are opposite faults.
```

## L801-804 · `Some(ref c) if c.established && c.state == State::Established => 1,`

```
// Same predicate as `conn_healthy`: a peer FIN moves us to CloseWait
// and sets `closed`, so a module polling this learns the far end hung
// up. `recv` never tells it — it just returns 0 bytes forever, which
// is why `debug` kept running after `nc` was closed.
```

## L811-814 · `pub fn connect(remote_ip: [u8; 4], remote_port: u16) -> Result<usize, TcpError> {`

```
/// Open a TCP connection, blocking until established. NATIVE callers only —
/// they run as a task on a worker core, where `super::poll` pumps the peer
/// fibers so the NIC keeps being drained while we wait. A module must use
/// `connect_start` + `connect_status` instead; see the note there.
```

## L818 · `let t0 = crate::interrupts::ticks();`

```
// Wait for ESTABLISHED (blocking poll)
```

## L826-830 · `-2 => {`

```
// -2 is our own retry budget running out; -1 is the peer closing
// the connection (a RST answers a SYN with a refusal). Reporting
// both as ConnectionRefused told us "nothing is listening" when the
// truth was "we gave up asking" — opposite investigations, third
// time today that one message covered two causes.
```

## L847 · `if crate::interrupts::ticks() - t0 > 1000 { // 10s timeout`

```
// 10s timeout
```

## L848-852 · `if let Some(ref c) = CONNECTIONS.lock()[handle] {`

```
// Say HOW FAR it got. A connect that dies has three distinct
// shapes and one message: next hop never resolved (`arp_pending`
// still set, or it gave up and broadcast), SYN sent and never
// answered (`retries` climbing), or the state machine stuck
// somewhere else entirely. Guessing between them costs an evening.
```

## L867 · `#[allow(dead_code)]`

```
/// Listen on a local port. Returns handle. Use accept() to wait for connection.
```

## L889-892 · `rcvq_space: 10 * MSS as u32,`

```
// `tcp_init_buffer_space` setzt den Startwert aus dem, was eine
// frische Verbindung ohnehin anbietet. Zehn Segmente ist
// `TCP_INIT_CWND * advmss`; ohne Startwert teilt `rcvbuf_grow`
// durch null.
```

## L935 · `#[allow(dead_code)]`

```
/// Wait for an incoming connection on a listening handle. Blocking.
```

## L962 · `#[allow(dead_code)]`

```
/// Check if a listening handle has an established connection (non-blocking).
```

## L969 · `#[allow(dead_code)]`

```
/// Reset a connection back to Listen state (for accepting next client).
```

## L995-998 · `rcvq_space: 10 * MSS as u32,`

```
// `tcp_init_buffer_space` setzt den Startwert aus dem, was eine
// frische Verbindung ohnehin anbietet. Zehn Segmente ist
// `TCP_INIT_CWND * advmss`; ohne Startwert teilt `rcvbuf_grow`
// durch null.
```

## L1036-1042 · `pub fn send(handle: usize, data: &[u8]) -> Result<(), TcpError> {`

```
/// Send data on a connection. Buffers and sends immediately (no Nagle).
///
/// The bytes are ALSO kept in `send_buf` until the peer acknowledges them,
/// so `tick_connections` can retransmit. Returns `WouldBlock` when too much
/// is already unacknowledged — that is real backpressure, not an error:
/// before, every send was reported as a success and a lost segment simply
/// vanished.
```

## L1056-1077 · `pub static ACK_GEN: AtomicU64 = AtomicU64::new(0);`

```
// ── Wo die Sendezeit hingeht ─────────────────────────────────────
//
// **Erzeugerbegrenzt oder fensterbegrenzt — das ist EINE Frage mit zwei
// entgegengesetzten Antworten**, und wir haben sie bisher aus dem
// Durchsatz zurueckgerechnet statt sie zu messen. `SEND_WOULDBLOCK` ist
// der Diskriminator: bleibt er null, haben wir nie auf Quittungen
// gewartet und der Deckel ist die Zeit in `send_inner`; steht er hoch,
// ist `MAX_UNACKED` der Deckel und die Konstante gehoert durch
// `tcp_sndbuf_expand` ersetzt.
/// Zaehlt jede Quittung, die Platz gemacht hat.
///
/// **Sie ist da, damit `send_blocking` NICHT auf der grossen Sperre
/// dreht.** Gemessen am 2026-09-22: 4 508 041 vergebliche `send()` in
/// 3,1 Sekunden, also 1,45 Mio `CONNECTIONS.lock()` je Sekunde — und
/// genau diese Sperre braucht der Empfangspfad, um eine Quittung zu
/// verbuchen. Der Sender hat seinen eigenen Quittungsweg ausgehungert;
/// die Strecke mass 7,8 ms Umlaufzeit, wo der Server 2,3 ms sah.
///
/// Linux hat dafuer `sk_stream_wait_memory`: der Sender SCHLAEFT, bis
/// `sk_write_space` ihn weckt. Wir haben keine Warteschlangen, aber ein
/// Zaehler ohne Sperre ist derselbe Gedanke — es gibt nichts Neues zu
/// versuchen, solange er steht.
```

## L1085-1087 · `pub static SEND_REFUSED: AtomicU64 = AtomicU64::new(0);`

```
/// Wie oft die Schlange zum Treiber ein Segment abgelehnt hat. Frueher
/// war das ein stiller Verlust (`tx drops full`), jetzt ist es Gegendruck
/// — und die Zahl sagt, wie oft er greift.
```

## L1089-1092 · `pub static FAST_RETRANS: AtomicU64 = AtomicU64::new(0);`

```
/// Schnelle Wiederholungen und Zeitueberschreitungen. **Ohne die zwei
/// Zahlen ist ein zu kleines Fenster nicht von einem verlorenen Segment
/// zu unterscheiden**, und genau daran habe ich vier Releases lang
/// vorbeigeraten.
```

## L1095-1098 · `pub static DUPACKS_SEEN: AtomicU64 = AtomicU64::new(0);`

```
/// **Kumulativ**, nicht der Live-Zaehler der Verbindung. Im Bericht
/// stand `0 Doppelquittungen`, und das hiess nur „die letzte Quittung
/// hat etwas abgeraeumt" — eine Zahl, die genau dann null ist, wenn man
/// sie braucht.
```

## L1101-1103 · `pub fn snd_limit_of(handle: usize) -> usize {`

```
/// Wohin der Sendepuffer gewachsen ist, und was das Gegenueber zuletzt
/// angeboten hat. Beides nur fuer den Bericht — ohne die zwei Zahlen ist
/// nicht zu sehen, ob `tcp_sndbuf_expand` ueberhaupt gegriffen hat.
```

## L1108 · `pub fn snd_unacked_of(handle: usize) -> usize {`

```
/// Wieviel gerade unquittiert im Sendepuffer liegt.
```

## L1113 · `pub fn cwnd_of(handle: usize) -> (u32, u32, u32, bool) {`

```
/// (cwnd in Paketen, ssthresh, Doppelquittungen, in Erholung)
```

## L1126 · `pub fn send_stats_reset() {`

```
/// Die Zaehler auf null, damit eine Messung nur ihren eigenen Lauf sieht.
```

## L1139-1140 · `pub fn send_stats() -> (u64, u64, u64, u64, u64) {`

```
/// (Takte in send, Takte in abgewiesenen send, Segmente, WouldBlock,
/// groesster send_buf)
```

## L1153-1156 · `if !conn.send_buf.is_empty()`

```
// **Ein leerer Puffer nimmt IMMER an.** Sonst kann ein Aufruf, der
// groesser ist als der Deckel, nie durchkommen — und eine Absage,
// die sich durch Warten nicht aendert, ist ein Stillstand. Genau das
// war der Fehler in 0.405.0.
```

## L1164 · `if conn.send_buf.is_empty() { conn.rto_tick = now; conn.retries = 0; }`

```
// Oldest unacked byte starts its clock now if nothing was in flight.
```

## L1168-1169 · `let buf_now = conn.send_buf.len() as u64;`

```
// Send in effective-MSS chunks immediately (no Nagle). Effective, not MSS:
// the option bytes come out of the same 1514.
```

## L1179-1197 · `fn write_xmit(conn: &mut TcpConn) {`

```
/// tcp_output.c `tcp_write_xmit` — schiebt hinaus, was ungesendet im
/// Puffer liegt, und **hoert auf, wenn das Geraet ablehnt**.
///
/// **Bis 0.405.2 gab es den Zustand „im Puffer, aber noch nicht auf der
/// Leitung" gar nicht.** `send` schrieb jedes Stueck sofort hinaus und
/// warf das Ergebnis weg. Lehnte die Schlange zum Treiber ab — am Geraet
/// `tx drops full 388`, sobald der Sendepuffer auf 2 MB gewachsen war —,
/// glaubte TCP trotzdem gesendet zu haben: `snd_nxt` lief weiter, und
/// die Rettung hing allein am RTO, der EIN MSS je Runde nachschickt. Bei
/// zwei Megabyte unterwegs ist das ein Stillstand, und genau so sah es
/// aus („PUT stalled after 2293760 bytes").
///
/// Linux bremst hier mit `netif_stop_queue`: die Schlange sagt nein, und
/// `tcp_write_xmit` laesst das Segment STEHEN, statt es zu verlieren.
/// Dasselbe hier — `snd_nxt` wird erst nach einer angenommenen
/// Uebergabe weitergesetzt, und `tick_connections` holt den Rest.
///
/// Damit ist das Wachstum des Sendepuffers auch sicher: mehr Puffer
/// heisst jetzt mehr WARTENDE Bytes, nicht mehr verworfene.
```

## L1206-1211 · `let in_flight = sent.div_ceil(mss) as u32;`

```
// ── Tor 1: das Staufenster (tcp_output.c:2238 `tcp_cwnd_test`)
//
// `in_flight >= cwnd` heisst: nichts mehr hinaus, bis eine
// Quittung Platz macht. **Das ist die Bremse, die uns gefehlt
// hat** — ohne sie ging der ganze Puffer in einem Zug auf die
// Luft, und was dort nicht hinpasste, war verloren.
```

## L1216-1217 · `let n = (conn.send_buf.len() - sent).min(mss);`

```
// ── Tor 2: das Fenster des Gegenuebers
//           (tcp_output.c:2295 `tcp_snd_wnd_test`)
```

## L1226 · `SEND_REFUSED.fetch_add(1, Ordering::Relaxed);`

```
// Voll. Nichts verloren, nur noch nicht hinaus.
```

## L1236-1237 · `fn cong_avoid(conn: &mut TcpConn, acked_pkts: u32) {`

```
/// `tcp_slow_start` (tcp_cong.c:454) + `tcp_cong_avoid_ai` (:468),
/// zusammengefasst wie `tcp_reno_cong_avoid` (:493).
```

## L1240 · `conn.snd_cwnd = (conn.snd_cwnd + acked_pkts).min(conn.snd_ssthresh);`

```
// „In safe area, increase."
```

## L1244 · `let w = conn.snd_cwnd.max(1);`

```
// „In dangerous area, increase slowly" — ein Paket je Fenster.
```

## L1258-1260 · `fn retransmit_head(conn: &mut TcpConn) {`

```
/// Das Segment an `snd_una` noch einmal — und nur dieses.
/// `tcp_retransmit_skb` auf dem Kopf der Wiederholungsschlange.
/// `snd_nxt` bleibt, wo es ist: was dahinter liegt, ist unterwegs.
```

## L1277 · `fn reno_ssthresh(conn: &TcpConn) -> u32 {`

```
/// `tcp_reno_ssthresh` (tcp_cong.c:512): die Haelfte, mindestens zwei.
```

## L1282-1284 · `pub fn send_blocking(handle: usize, data: &[u8], timeout_ticks: u64) -> Result<(), TcpError> {`

```
/// Send, waiting out backpressure. NATIVE callers only — the same rule as
/// `connect`: this polls, which pumps the peer fibers on a worker core. A
/// module must handle `WouldBlock` itself and sleep between tries.
```

## L1288-1292 · `let zuletzt = ACK_GEN.load(Ordering::Relaxed);`

```
// **VOR dem Versuch gelesen, nicht danach.** Kommt die Quittung
// waehrend `send()` laeuft, steht der Zaehler danach schon
// hoeher — wer ihn erst dann liest, wartet auf die NAECHSTE, und
// wenn es keine mehr gibt, bis zur Frist. Die klassische
// verpasste Weckung.
```

## L1298-1319 · `let mut leer = 0u32;`

```
// **Erst wenn eine Quittung Platz gemacht hat, lohnt ein zweiter
// Versuch.**
//
// Hier stand eine Schleife, die JEDEN Umlauf `send()` rief — und
// damit `CONNECTIONS.lock()` nahm. Gemessen: 4 508 041 Umlaeufe
// in 3,1 Sekunden. Die alte Begruendung („eine Antwort innerhalb
// des ersten Ticks sieht keinen Kontextwechsel") galt dem
// Download-Pfad, wo eine Antwort tatsaechlich sofort kommt; beim
// SENDEN wartet man auf eine Quittung, und die braucht genau die
// Sperre, die wir hier in der Hand halten.
//
// `yield_ready` meldet `false`, wenn wir gar nicht in einem Fiber
// laufen (Core 0, OTA); dort treiben wir den Stapel selbst an,
// sonst kaeme die Quittung nie.
//
// **Kurz drehen, DANN abgeben.** Blind abzugeben kostet den
// Planertakt: 0.405.1 wartete je Quittung bis zu 10 ms und fiel
// damit von 269 auf 104 Mbit, obwohl `WouldBlock` von 4,5 Mio
// auf 1215 gesunken war. Gedreht wird auf dem ATOMAR gelesenen
// Zaehler, nicht auf der Sperre — das war der ganze Punkt. Ein
// Budget leerer Blicke, dann erst schlafen: dieselbe Form wie im
// Empfangsweg des Treibers.
```

## L1332-1334 · `super::poll();`

```
// Den Stapel antreiben, falls niemand sonst es tut — auf
// Kern 0 gibt es keinen Treiber-Fiber, der die Quittung
// hereinholt.
```

## L1338-1345 · `runden += 1;`

```
// **Und nie fuer immer auf einen Zaehler warten.**
//
// Zweimal in Folge hat uns eine verpasste Weckung eine Frist
// gekostet — einmal, weil der Zaehler nach dem Versuch
// gelesen wurde, einmal, weil eine Fenster-Aktualisierung
// ihn nicht bewegte. Beide sind gefixt; die Klasse bleibt.
// Ein Warten, das SELBST wieder nachsieht, kann nur
// langsam werden, nicht tot.
```

## L1354-1355 · `pub fn recv(handle: usize, buf: &mut [u8]) -> Result<usize, TcpError> {`

```
/// Receive data. Returns available data (may be empty if nothing received yet).
/// Sends a window update ACK if significant buffer space was freed.
```

## L1362-1365 · `{`

```
// Bulk copy out of the ring buffer instead of byte-by-byte pop_front
// (that was ~112M pop_front/s at 100 MB/s — pure call overhead). The
// VecDeque exposes its contents as up to two contiguous slices; memcpy
// each, then drain in one shot.
```

## L1375-1376 · `conn.copied_total = conn.copied_total.saturating_add(available as u64);`

```
// tcp_input.c:930-932 „This function should be called every time
// data is copied to user space."
```

## L1380-1394 · `conn.freed_since_winupd = conn.freed_since_winupd.saturating_add(available as u32);`

```
// Window-update ACK — RATE-LIMITED.
//
// We must re-advertise the window the consumer just reopened so a
// trickle / zero-window sender resumes (the case this ACK was added for:
// a TLS sender that bursts then goes quiet, no more handle_tcp data-ACKs,
// → window stuck small → peer zero-window-probes → ~31 KiB/s sawtooth).
// But a BULK plain-http download calls recv() once PER PACKET (~70k/s, not
// per ~16 KiB TLS record), so ACKing on every drain floods the TX path:
// ~70k ACKs/s, each an alloc + a virtio TX-doorbell VM-exit → pegs the
// worker core AND defeats the handle_tcp ACK-coalescing.
//
// So: ACK immediately only when the window was actually CONSTRAINED
// (buffer >1/4 full → window shrinking, the trickle/zero-window case),
// otherwise at most once per ~64 KiB freed. handle_tcp's coalesced
// data-ACKs carry the (wide-open) window the rest of the time.
```

## L1409 · `pub fn recv_blocking(handle: usize, buf: &mut [u8], timeout_ticks: u64) -> Result<usize, TcpError> {`

```
/// Receive with blocking wait (polls until data or timeout).
```

## L1413-1419 · `super::poll_rx_only();`

```
// NIC-drain only (the TLS / OTA-https recv hot path). The old code ran
// the FULL super::poll() — tcp::tick_connections (128-slot scan +
// CONNECTIONS lock) + shade::poll_render — AND then tick_connections()
// AGAIN, every spin iteration at ~1 M/s: double the 128-slot scan + lock,
// contending the CONNECTIONS lock with actual packet processing →
// pegged the worker core AND throttled https/OTA throughput. Core 0's
// poll() runs the TCP timers; here we just drain RX, like tcp_recv_poll.
```

## L1425 · `{`

```
// Check if connection closed
```

## L1436-1442 · `return Err(TcpError::Timeout);`

```
// `Err(Timeout)`, NOT `Ok(0)`. Both used to mean the same thing
// here, and `Ok(0)` is how a caller learns the peer hung up — so a
// link that merely went quiet for the timeout read as end-of-file.
// Measured: an OTA module download reported
// "short download (113728 of 1432235)" after a run of one-second
// transmit stalls. The transfer was not aborted; it was declared
// finished. A caller that can distinguish the two can wait longer.
```

## L1445-1448 · `crate::interrupts::worker_idle_hlt();`

```
// Timer-NAPI: HLT instead of spinning (the OTA-update / https core-peg
// Florian saw — same root as tcp_recv_poll). Records the halt so `cores`
// is honest. Wakes on the per-core timer (100 Hz here; OTA payloads are
// small so the latency is fine), the NIC re-fills the ring in the gap.
```

## L1453-1457 · `pub fn conn_healthy(handle: usize) -> bool {`

```
/// True if `handle` is an established, un-closed, un-errored connection —
/// i.e. safe to send another request on (HTTP keep-alive reuse). A peer
/// FIN moves the state out of `Established` (→ CloseWait) and sets
/// `closed`, so a server that dropped an idle keep-alive connection reads
/// as unhealthy here and the caller reconnects instead of hanging.
```

## L1464-1465 · `pub fn peer(handle: usize) -> Option<([u8; 4], u16)> {`

```
/// Wohin diese Verbindung geht. Fuers Coalescing: zwei Namen duerfen sich
/// eine Verbindung nur teilen, wenn sie zur selben Adresse fuehren.
```

## L1474-1485 · `pub fn close(handle: usize) -> Result<(), TcpError> {`

```
/// Close a connection gracefully (sends FIN) and return at once.
///
/// Linux's `close()` does not wait either: the socket lingers in the
/// background and only `SO_LINGER` — off by default — makes it block.
/// Waiting here spun on the caller's core for up to 2 s. Measured on the
/// device: a peer that answers `Connection: close` turned a 140 ms document
/// fetch into 2150 ms, and the time landed outside every span the HTTP client
/// prints, so it read as an unexplained gap. A host call that spins also
/// freezes every other fiber on that worker core.
///
/// The FIN goes out, the slot stays in FinWait1, and `tick_connections`
/// carries it to TimeWait or reaps it if the peer never answers.
```

## L1497 · `conns[handle] = None;`

```
// Never established, or already shutting down — nothing to say.
```

## L1503 · `pub fn handle_tcp(ip_packet: &[u8], data: &[u8]) {`

```
/// Handle incoming TCP segment (called from ipv4)
```

## L1520 · `let idx = conns.iter().position(|c| {`

```
// Find matching connection
```

## L1530 · `if flags & SYN != 0 {`

```
// Check for a listener on this port
```

## L1538 · `let iss = generate_isn(arp::our_ip(), src_ip, dst_port, src_port);`

```
// Accept the SYN on the listening socket
```

## L1551 · `conn.wscale_ok = peer_ws.is_some();`

```
// Scaling is active only if the peer offered it too.
```

## L1555 · `let opts: &[u8] = if peer_ws.is_some() {`

```
// SYN-ACK: MSS, and Window Scale only if the peer asked for it.
```

## L1568 · `if flags & RST == 0 {`

```
// No connection and no listener: send RST if not RST
```

## L1578 · `if flags & RST != 0 {`

```
// RST handling
```

## L1587 · `if flags & ACK != 0 {`

```
// Waiting for ACK of our SYN-ACK
```

## L1597 · `conn.rcv_irs = seq;`

```
// SYN-ACK received
```

## L1604-1606 · `if let Some(ws) = parse_wscale(data, data_offset) {`

```
// We always offer WScale in our SYN, so scaling is active iff
// the SYN-ACK carries it. Set before the ACK so it advertises
// the scaled window immediately.
```

## L1611-1613 · `if let Some(ts) = parse_ts(data, data_offset) {`

```
// Timestamps active iff the SYN-ACK echoes the option (RFC 7323).
// Seed ts_recent with the peer's TSval so our handshake ACK
// already carries a valid TSecr.
```

## L1618 · `conn.sack_ok = parse_sack_permitted(data, data_offset);`

```
// SACK active iff the SYN-ACK also carried SACK-permitted.
```

## L1621 · `let w = recv_window(conn);`

```
// Send ACK with full window
```

## L1628 · `if flags & ACK != 0 {`

```
// ACK processing
```

## L1630-1640 · `let vorheriges_fenster = conn.snd_wnd;`

```
// **Das Fenster des Gegenuebers, und bis hierher hiess
// es `_window`.** Es wurde gelesen und weggeworfen: wir
// hatten keine Flusskontrolle, sondern einen Puffer
// (`MAX_UNACKED`), der zufaellig ungefaehr so gross war.
// Ein Empfaenger, der sein Fenster schliesst, konnte uns
// nicht bremsen — RFC 9293 §3.8.6 ist damit schlicht
// nicht gebaut gewesen.
//
// Die Skalierung ist die des SYN-ACK (`snd_wscale`), und
// sie gilt fuer JEDES Segment danach ausser dem SYN
// selbst (RFC 7323 §2.2).
```

## L1643-1658 · `ACK_GEN.fetch_add(1, Ordering::Relaxed);`

```
// **Und JEDE Quittung weckt den Sender**, nicht nur
// eine, die Daten abraeumt.
//
// Der Zaehler stand bis 0.405.3 unten im
// `ack_in_range`-Zweig — also nur bei NEU quittierten
// Bytes. Eine reine Fenster-Aktualisierung traegt
// `ack == snd_una` und faellt nicht hinein: der
// Empfaenger hat seinen Puffer geleert und macht wieder
// auf, und wir warten trotzdem bis zur Frist. Genau so
// sah es aus („PUT stalled after 3014656 bytes"), und
// eine Doppelquittung waehrend eines Verlusts hat
// dasselbe Problem.
//
// Die Weckung ist kein Urteil darueber, DASS sich etwas
// geaendert hat — sie sagt nur, dass ein neuer Versuch
// sich lohnen koennte.
```

## L1660-1674 · `if conn.ts_ok {`

```
// **Die Umlaufzeit gehoert an die QUITTUNG, nicht an die
// Daten.**
//
// Sie stand bisher nur im Datenzweig weiter unten — also
// nur dann, wenn das Gegenueber uns etwas SCHICKT. Beim
// Hochladen schickt es nackte Quittungen, `srtt_ms`
// blieb 0, und `snd_space_adjust` kehrte in der ersten
// Zeile um: der Sendepuffer konnte nicht wachsen, weil
// es keine Umlaufzeit gab, an der er haette wachsen
// koennen. Am Geraet gemessen: `Deckel 256 KB` bei
// `Gegenueber 1036 KB`.
//
// Linux nimmt die Probe in `tcp_ack_update_rtt`, gerufen
// aus `tcp_clean_rtx_queue` — also genau hier, wo eine
// Quittung hereinkommt.
```

## L1689-1700 · `let ist_dup = payload.is_empty()`

```
// ── Doppelquittung: RFC 5681 §3.2 ───────────────────
//
// Drei in Folge heissen „ein Segment fehlt, der Rest
// kommt an". Bis 0.406.0 loesten sie NICHTS aus: die
// einzige Erholung war der RTO, und der schickte ein MSS
// je 200 ms. Bei 1,5 MB unterwegs ist das kein
// Wiederanlauf, sondern ein Stillstand — am Geraet 2070
// Segmente in zehn Sekunden.
//
// Eine Doppelquittung ist eine Quittung ohne neue Bytes,
// ohne Nutzlast und ohne Fensteraenderung; sonst waere
// es eine Fensteraktualisierung.
```

## L1711-1712 · `if CONG_CONTROL {`

```
// Halbieren, eintreten, und das fehlende Segment
// SOFORT nachschicken statt auf den RTO zu warten.
```

## L1722-1737 · `retransmit_head(conn);`

```
// **NUR das fehlende Segment, nicht das ganze
// Fenster.**
//
// Hier stand `snd_nxt = snd_una` — Go-back-N.
// Damit wurde `in_flight` null, und `write_xmit`
// schob auf der Stelle `ssthresh + 3` Pakete in
// EINEM Zug hinaus: bei einem vorher grossen
// Fenster mehrere hundert. Am Geraet sprang
// `Schlange voll` von 0 auf 1384.
//
// Drei Doppelquittungen sagen „EIN Segment
// fehlt, der Rest kommt an" (RFC 5681 §3.2).
// Also genau dieses eine noch einmal; alles
// dahinter ist unterwegs und darf es bleiben.
// Go-back-N gehoert zum RTO, wo wir wirklich
// nichts mehr wissen.
```

## L1741-1742 · `conn.snd_cwnd += 1;`

```
// „Inflate": jede weitere Doppelquittung sagt,
// dass ein Segment die Leitung verlassen hat.
```

## L1748-1749 · `let acked = ack.wrapping_sub(conn.snd_una) as usize;`

```
// Drop the acknowledged prefix from the retransmit queue
// and restart the timer for whatever is still in flight.
```

## L1756-1759 · `conn.acked_total =`

```
// **Und hier waechst der Sendepuffer** — an derselben
// Stelle, an der Linux `tcp_check_space` ->
// `tcp_new_space` -> `tcp_sndbuf_expand` ruft: wenn
// eine Quittung Platz gemacht hat.
```

## L1763 · `let mss_now = eff_mss(conn).max(1);`

```
// ── Das Staufenster nachfuehren ────────────────
```

## L1768-1771 · `let noch_offen = (conn.recovery_end`

```
// RFC 6582 §3.2: erst wenn alles quittiert ist,
// was beim Eintritt unterwegs war, ist die
// Erholung vorbei. Sonst halbiert EIN
// Verlustereignis das Fenster mehrfach.
```

## L1786 · `if !payload.is_empty() {`

```
// Data processing
```

## L1789-1790 · `if conn.ts_ok {`

```
// RFC 7323: advance ts_recent to this in-order segment's
// TSval so our echoed TSecr gives the sender a fresh RTT.
```

## L1798-1799 · `conn.recv_buf.extend(payload[..copy].iter().copied());`

```
// Bulk append — NOT byte-by-byte push_back (that was ~87M
// push_back/s at ~700 Mbit). extend reserves once + copies.
```

## L1802-1804 · `let mut filled = false;`

```
// Gap just filled — pull any now-contiguous segments out of
// the reassembly queue. Only the lowest stored offset can be
// next; if it doesn't meet rcv_nxt there's still a hole.
```

## L1808-1809 · `let (k, seglen) = match conn.ooo.iter().next() {`

```
// Peek the lowest stored offset (copy out k+len so the
// immutable borrow ends before we remove).
```

## L1814 · `if (k as usize) + seglen <= want as usize {`

```
// Drop fully-stale segments (already delivered).
```

## L1818 · `if k != want { break; }                 // still a gap before it`

```
// still a gap before it
```

## L1829-1832 · `let delivered = conn.rcv_nxt.wrapping_sub(conn.rcv_irs);`

```
// Keep the SACK run-set in sync with what's now delivered, and
// refresh the RTT estimate from the peer's echoed TSecr (our
// TSval is ticks(), so ticks()-TSecr = RTT). recv_window() turns
// that into the window via link-capacity × RTT.
```

## L1838-1842 · `let sample = ts_now_ms().wrapping_sub(tsecr);`

```
// In MILLISEKUNDEN, seit die Marke
// `ts_now_ms()` ist. Alles ueber sechs
// Sekunden ist keine RTT, sondern ein
// Umlauf der Marke oder ein Echo aus
// einer anderen Verbindung.
```

## L1852-1853 · `if filled {`

```
// A filled gap must be ACKed immediately so the sender stops
// retransmitting and advances — don't let it sit in coalescing.
```

## L1860-1862 · `conn.acks_held += 1;`

```
// Coalesced ACK: one ACK per ACK_COALESCE in-order segments.
// A lone held ACK is flushed by the 40 ms timer in
// tick_connections so a trickle/idle never strands the sender.
```

## L1877-1880 · `TCP_OOO_AHEAD.fetch_add(1, Relaxed);`

```
// AHEAD = a real gap (an earlier segment was lost). Buffer
// this segment for reassembly + send a duplicate ACK so the
// sender fast-retransmits ONLY the hole (RFC 5681) — not the
// whole window. Bounded; over budget or already-have → skip.
```

## L1896-1914 · `TCP_OOO_BEHIND.fetch_add(1, Relaxed);`

```
// BEHIND = wir haben diese Bytes schon. **Jetzt mit
// D-SACK (RFC 2883) statt mit Schweigen.**
//
// Hier stand, man duerfe nicht erneut quittieren, weil
// drei gleiche Quittungen eine Schnellwiederholung
// ausloesen (v0.219.7/8). Der Schluss war zu breit: eine
// Quittung, die einen BEREITS QUITTIERTEN Bereich als
// ersten SACK-Block nennt, ist genau das Gegenteil eines
// Doppels — der Sender liest daran ab, dass seine
// Wiederholung ueberfluessig war, und nimmt seine
// Fensterkuerzung ZURUECK (Linux: `tcp_dsack_seen` ->
// `tcp_undo_cwnd_reduction`).
//
// Ohne das blutet er bei jedem Mal. Am Geraet gemessen
// (2026-09-21, WLAN): `retrans=371` bei `lost=0` — der
// Server wiederholte 371-mal, ohne ein einziges Paket
// als verloren zu fuehren, und wir konnten es ihm nicht
// sagen. Ein `dsack=0` in seinem `tcp_info` war deshalb
// nie eine Aussage ueber die Leitung, sondern ueber uns.
```

## L1918 · `let hi = if (end.wrapping_sub(conn.rcv_nxt) as i32) > 0 {`

```
// Nur der Teil, den wir WIRKLICH schon haben.
```

## L1937 · `if flags & FIN != 0 {`

```
// FIN from remote
```

## L1942 · `send_seg(conn, conn.snd_nxt, conn.rcv_nxt, ACK, 0, &[]);`

```
// ACK the FIN
```

## L1979 · `pub fn tick_connections() {`

```
/// Periodic tick: retransmit, delayed ACKs, timeouts
```

## L1983-1986 · `let mut pending: alloc::vec::Vec<PendingSeg> = alloc::vec::Vec::new();`

```
// Collect the segments to send WHILE holding the lock (they read conn
// state), then drop the lock and hit the NIC. Holding CONNECTIONS
// across the TX doorbell blocked worker-core `recv` behind Core-0's
// periodic ACKs/retries (contention ④).
```

## L1988-1989 · `let mut arp_probes: alloc::vec::Vec<[u8; 4]> = alloc::vec::Vec::new();`

```
// Same reason: `arp::request` hits the NIC, so collect and fire after the
// lock is gone.
```

## L1991-1997 · `let retrans: alloc::vec::Vec<(PendingSeg, alloc::vec::Vec<u8>)> =`

```
// **Wiederholungen fahren nicht mehr hier.** Sie brauchten einen
// eigenen Weg, weil sie eine Nutzlast tragen und `pending` nur
// leere Segmente kennt. Seit die Zeitueberschreitung ein echtes
// Verlustereignis ist (`snd_nxt = snd_una`, Fenster auf eins),
// schickt `write_xmit` sie aus demselben Puffer wie alles andere —
// eine Wiederholung ist dann nichts Besonderes mehr, sondern ein
// Segment, das noch einmal ungesendet ist.
```

## L2003-2006 · `if slot.state == State::Established`

```
// **Was die Schlange vorhin abgelehnt hat, geht jetzt
// hinaus.** Ohne diese Zeile bliebe es liegen, bis der Rufer
// das naechste Mal `send` ruft — und der wartet gerade auf
// eine Quittung fuer Bytes, die nie auf der Leitung waren.
```

## L2012 · `if slot.ack_pending && now - slot.ack_tick >= DELAYED_ACK_TICKS {`

```
// Delayed ACK
```

## L2026 · `if slot.state == State::SynSent {`

```
// SYN retry
```

## L2033-2035 · `let target = ipv4::arp_target_for(slot.remote_ip);`

```
// Next hop still unknown, SYN held back. Re-ask every
// RETRANS window — one request is a coin flip over WiFi,
// and both the request and the reply can be the loss.
```

## L2044-2046 · `slot.arp_pending = false;`

```
// Give up asking and send anyway (to broadcast),
// exactly as the old ~500 ms pre-resolve did on
// timeout. From here the normal SYN retry runs.
```

## L2077-2078 · `if slot.state == State::Established && !slot.send_buf.is_empty() {`

```
// Data retransmit. `send_buf` starts at snd_una, so the head of
// it is exactly the segment the peer is missing.
```

## L2089-2127 · `if CONG_CONTROL {`

```
// ── `tcp_enter_loss` ───────────────────────
//
// **Eine Zeitueberschreitung ist der haerteste
// Stauhinweis, den es gibt** (RFC 5681 §3.1):
// Schwelle auf die Haelfte, Fenster auf EINS,
// und von vorn im langsamen Start.
//
// Hier stand stattdessen: EIN MSS nachschicken,
// `snd_nxt` unberuehrt lassen, fertig. Damit galt
// nach jedem RTO der ganze Rest weiter als
// unterwegs, und wiederholt wurde ein einziges
// Segment je 200 ms mit Verdopplung. Bei 1,5 MB
// offener Daten ist das eine Erholung, die nie
// ankommt — am Geraet 2070 Segmente in zehn
// Sekunden.
//
// **`snd_nxt` wird NICHT zurueckgespult.**
//
// Hier stand `snd_nxt = snd_una`, als Go-back-N
// gedacht, und es hat die Verbindung getoetet:
// `ack_in_range(una, ack, nxt)` laesst nur
// Quittungen bis `snd_nxt` gelten. Nach dem
// Ruecksetzen liegt `snd_nxt` EIN Segment ueber
// `snd_una`, waehrend das Gegenueber laengst
// hunderte Kilobyte hat und seinen echten Stand
// quittiert — der faellt aus dem Bereich und
// wird VERWORFEN. `snd_una` steht fuer immer,
// `rto_tick` wird nie zurueckgesetzt, und der
// RTO feuert mit Verdopplung bis zur Frist. Am
// Geraet: `cwnd 1 · ssthresh 2 · 7x
// Zeitueberschreitung`, 200+400+...+6400 ms.
//
// Linux spult `snd_nxt` nie zurueck — es ist die
// hoechste je gesendete Folgenummer. Wiederholt
// wird aus der Wiederholungsschlange
// (`tcp_xmit_retransmit_queue`), ohne sie
// anzufassen, und neue Daten gehen erst wieder
// hinaus, wenn Quittungen `in_flight` unter das
// Staufenster gebracht haben.
```

## L2141 · `if slot.state == State::TimeWait && now - slot.last_send_tick > 200 {`

```
// TimeWait cleanup (2 seconds)
```

## L2146-2149 · `if matches!(slot.state, State::FinWait1 | State::FinWait2 | State::LastAck)`

```
// Half-closed with a peer that never answers. `close`
// leaves FinWait1 behind on purpose and nothing else frees it —
// without this the slot is pinned for the rest of the boot.
// 60 s = Linux's tcp_fin_timeout.
```

## L2171 · `fn syn_opts(opts: &mut [u8; 40]) -> usize {`

```
// === Internal ===
```

## L2173-2179 · `fn syn_opts(opts: &mut [u8; 40]) -> usize {`

```
/// SYN options: MSS(4) + SACK-permitted(2) + NOP,NOP + Timestamp(kind=8,10) +
/// NOP + WScale(3) = 22, padded to 24. Returns the length written.
///
/// Shared by the first SYN and every retransmit. The retry used to send a
/// bare SYN: whenever the first one was lost — the normal case on a cold ARP
/// cache — the connection silently came up without window scaling, SACK or
/// timestamps, i.e. capped at a 64 KiB window for its whole life.
```

## L2181 · `opts[0] = 2;  // MSS option kind`

```
// MSS option kind
```

## L2182 · `opts[1] = 4;  // MSS option length`

```
// MSS option length
```

## L2184 · `opts[4] = 4;            // SACK-permitted kind`

```
// SACK-permitted kind
```

## L2185 · `opts[5] = 2;            // length`

```
// length
```

## L2186 · `opts[6] = 1;            // NOP`

```
// NOP
```

## L2187 · `opts[7] = 1;            // NOP — align the 10-byte Timestamp to 4 bytes`

```
// NOP — align the 10-byte Timestamp to 4 bytes
```

## L2188 · `opts[8] = 8;            // Timestamp option kind`

```
// Timestamp option kind
```

## L2189 · `opts[9] = 10;           // length`

```
// length
```

## L2191 · `opts[10..14].copy_from_slice(&tsval.to_be_bytes()); // TSval`

```
// TSval
```

## L2192 · `opts[18] = 1;           // NOP — align the 3-byte WScale to a 4-byte boundary`

```
// opts[14..18] TSecr = 0 on a SYN
```

## L2193 · `opts[18] = 1;           // NOP — align the 3-byte WScale to a 4-byte boundary`

```
// NOP — align the 3-byte WScale to a 4-byte boundary
```

## L2194 · `opts[19] = 3;           // Window Scale option kind`

```
// Window Scale option kind
```

## L2195 · `opts[20] = 3;           // length`

```
// length
```

## L2196 · `opts[21] = OUR_WSCALE;  // shift count`

```
// shift count
```

## L2225 · `let opts_padded = (options.len() + 3) & !3; // pad to 4 bytes`

```
// pad to 4 bytes
```

## L2235 · `pkt[12] = ((header_len / 4) as u8) << 4; // data offset`

```
// data offset
```

## L2239 · `if !options.is_empty() {`

```
// Options
```

## L2244 · `pkt[header_len..].copy_from_slice(payload);`

```
// Payload
```

## L2247 · `let src_ip = arp::our_ip();`

```
// TCP checksum (pseudo-header + TCP segment)
```

## L2252-2254 · `ipv4::send(dst_ip, ipv4::PROTO_TCP, &pkt)`

```
// **Das Ergebnis wird nicht mehr weggeworfen.** `netdev::send` lehnt
// ab, wenn die Schlange zum Treiber voll ist (`tx drops full`), und
// bis 0.405.3 glaubte TCP trotzdem, gesendet zu haben.
```

## L2261 · `sum += u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;`

```
// Pseudo-header
```

## L2266 · `sum += 6u32; // protocol TCP`

```
// protocol TCP
```

## L2269 · `for i in (0..segment.len()).step_by(2) {`

```
// TCP segment
```

## L2285-2287 · `fn parse_sack_permitted(seg: &[u8], data_offset: usize) -> bool {`

```
/// Scan a segment's TCP options for the Window Scale option (kind 3) and
/// return its shift count. `data_offset` is the TCP header length in bytes.
/// Did the peer's options carry SACK-permitted (kind 4, len 2)?
```

## L2307-2310 · `fn build_sack_blocks(conn: &TcpConn, out: &mut [u8]) -> usize {`

```
/// Build the TCP SACK option (kind 5) into `out` from the connection's
/// out-of-order reassembly map: up to 3 contiguous [left,right) runs as
/// absolute sequence numbers. Returns bytes written (0 if nothing to report).
/// First block = the highest run (most recently relevant), per RFC 2018.
```

## L2315-2317 · `out[0] = 5;                       // SACK option kind`

```
// `ooo_runs` is already coalesced, so this is O(runs) — no per-ACK scan of
// the whole segment map. Emit the highest up-to-3 runs, highest first
// (RFC 2018 §4: the most recently received block goes first).
```

## L2318 · `out[0] = 5;                       // SACK option kind`

```
// SACK option kind
```

## L2321-2325 · `if let Some((l, r)) = conn.dsack {`

```
// **Der D-SACK-Block steht VORN, und das ist der ganze Vertrag.**
// RFC 2883 §4: der erste Block einer SACK-Option darf einen Bereich
// nennen, der bereits quittiert ist — daran und nur daran erkennt der
// Sender ein Duplikat. Steht er nicht an erster Stelle, ist er ein
// gewoehnlicher SACK-Block und sagt das Gegenteil.
```

## L2339 · `out[1] = (2 + 8 * take) as u8;    // length`

```
// length
```

## L2348 · `0 => break,        // End of Option List`

```
// End of Option List
```

## L2349 · `1 => i += 1,       // NOP`

```
// NOP
```

## L2353 · `if len < 2 { break; } // malformed`

```
// malformed
```

## L2364-2365 · `fn parse_ts(seg: &[u8], data_offset: usize) -> Option<u32> {`

```
/// Scan a segment's options for the Timestamp option (kind 8, len 10) and
/// return the peer's TSval. `data_offset` is the TCP header length in bytes.
```

## L2371 · `0 => break,        // End of Option List`

```
// End of Option List
```

## L2372 · `1 => i += 1,       // NOP`

```
// NOP
```

## L2376 · `if len < 2 { break; } // malformed`

```
// malformed
```

## L2388-2390 · `fn parse_tsecr(seg: &[u8], data_offset: usize) -> Option<u32> {`

```
/// Scan for the Timestamp option (kind 8) and return TSecr — the peer's echo of
/// OUR most recent TSval. Since our TSval is `ticks()`, `ticks() - TSecr` is a
/// receiver-measured RTT (used for window auto-tuning).
```

## L2413-2429 · `fn data_opts_len(conn: &TcpConn) -> usize {`

```
/// Send a segment for a known connection, adding the Timestamp option (our
/// TSval + the peer's echoed TSval) when timestamps were negotiated (RFC 7323).
/// All connection-originated segments (ACKs, data, FIN) must carry it so the
/// sender gets a clean per-segment RTT sample despite our ACK jitter.
/// Build the TCP option list (Timestamp, then SACK blocks during a gap;
/// both 4-byte aligned via leading NOPs) for `conn`/`flags` into `opts`,
/// returning its length. Shared by the inline `send_seg` and the deferred
/// tick path, which materializes segments under the CONNECTIONS lock and
/// sends them after dropping it.
/// Bytes `build_seg_opts` will add to a DATA segment on this connection. The
/// payload has to shrink by exactly this much, or the frame overruns the MTU.
///
/// It did. With timestamps negotiated every segment carries 12 option bytes, so
/// a full-size one was 14 + 20 + (20+12) + 1460 = 1526 against an MTU of 1514,
/// and `fq_codel::enqueue` dropped it without a word. Pure ACKs are 66 bytes
/// and sailed through, which is why every download worked and the first upload
/// ever attempted stalled after exactly MAX_UNACKED bytes with zero ACKs back.
```

## L2434-2435 · `fn eff_mss(conn: &TcpConn) -> usize {`

```
/// Payload per segment for this connection. `MSS` is the wire budget; what is
/// left for data is that minus the options every segment carries.
```

## L2443 · `opts[len] = 1; opts[len + 1] = 1;          // NOP, NOP`

```
// NOP, NOP
```

## L2444 · `opts[len + 2] = 8; opts[len + 3] = 10;     // Timestamp kind, len`

```
// Timestamp kind, len
```

## L2450-2454 · `if conn.sack_ok && flags & SYN == 0 && !has_payload`

```
// SACK blocks: only on a pure ACK while we hold out-of-order data (a gap).
// Never on a SYN — that advertises SACK-permitted instead.
// SACK blocks ride on a PURE ACK only. On a data segment they would push
// the frame past the MTU again — `eff_mss` budgets for the timestamp and
// nothing else, and a variable option length cannot be budgeted for at all.
```

## L2457 · `let mut sack = [0u8; 26]; // 2 + 8*3`

```
// 2 + 8*3
```

## L2460 · `opts[len] = 1; opts[len + 1] = 1;       // NOP, NOP align`

```
// NOP, NOP align
```

## L2482-2485 · `struct PendingSeg {`

```
/// A fully-resolved zero-payload segment captured under the CONNECTIONS
/// lock so it can be sent (the NIC doorbell) AFTER the lock is dropped.
/// Keeps `tick_connections` from holding the lock across TX, which blocked
/// worker-core `recv` behind Core-0's periodic delayed-ACKs / SYN retries.
```

## L2510 · `let diff_una = ack.wrapping_sub(una);`

```
// Check if ack is within (una, nxt] accounting for wrapping
```

## L2520-2523 · `pub fn has_timers() -> bool {`

```
/// Does any connection run a timer (retransmit, delayed ACK, SYN retry,
/// TIME_WAIT, FIN timeout)? Listening and closed sockets have none. The
/// shell loop drives `tick_connections` and must keep coming back while
/// this holds (stage 3e).
```

## L2557 · `WouldBlock,`

```
/// Too much already unacknowledged — retry the send later.
```

