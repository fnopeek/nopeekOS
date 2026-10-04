# `kernel/src/net/dns.rs` @ 5e0102684

## L1-4 · `use alloc::collections::VecDeque;`

```
//! DNS — Domain Name System
//!
//! Stub resolver over UDP port 53.
//! Queries A records, caches results.
```

## L14-16 · `const CACHE_SIZE: usize = 64;`

```
/// 16 was one page load. A browser opening a single site touches twenty to
/// forty names, so every entry was evicted before it was used twice and the
/// cache never answered anything.
```

## L19 · `static DNS_SERVER: Mutex<[u8; 4]> = Mutex::new([10, 0, 2, 3]); // QEMU user-mode DNS`

```
// QEMU user-mode DNS
```

## L21-22 · `static NEXT_ID: Mutex<u16> = Mutex::new(0xABCD);`

```
/// The local port is fixed, so the transaction ID is the only thing that tells
/// this query's reply from a late one for an earlier name.
```

## L28-29 · `valid: bool,`

```
/// true = an address. false = the resolver tried and got nothing; the entry
/// exists to keep the next asker from starting the same doomed lookup.
```

## L31-35 · `denied: bool,`

```
/// **Zwei Sorten Fehlschlag, und nur eine ist eine Auskunft.** `true`
/// heisst: der Aufloeser hat GEANTWORTET und den Namen nicht gekannt.
/// `false` heisst: nichts kam zurueck — das kann eine verlorene Frame
/// sein, und daraus eine halbe Minute Ausfall zu machen waere aus einer
/// Stoerung ein Defekt.
```

## L37-38 · `stamp: u64,`

```
/// Tick this entry was written. Evicts the oldest instead of always slot 0,
/// and expires a negative entry.
```

## L46-48 · `const NEG_TTL_TICKS: u64 = 3_000; // ~30 s at 100 Hz`

```
/// How long a failed lookup keeps a name out of the resolver. Without it a name
/// that does not resolve is retried on every single query for it — and each
/// retry costs the full budget.
```

## L49 · `const NEG_TTL_TICKS: u64 = 3_000; // ~30 s at 100 Hz`

```
// ~30 s at 100 Hz
```

## L51-52 · `static WANTED: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());`

```
/// Names queued for the background resolver. Filled by `want()` from callers
/// that must not block, drained by `pump_wanted()` on Core 0.
```

## L56 · `pub enum Cached {`

```
/// What the cache knows about a name, without touching the network.
```

## L59 · `Failed,`

```
/// Looked up and failed, recently enough to still count.
```

## L67-75 · `pub fn cached(name: &str) -> Cached {`

```
/// Cache-only lookup. Never sends, never waits.
///
/// For callers that MUST NOT block. The microvm data plane is one: it runs on
/// the vCPU fiber inside the virtio-net MMIO exit, and a blocking resolve there
/// does not just freeze the guest — `fiber::pump_peers()` bails out inside a
/// fiber, so the WASM NIC driver fiber sharing that core stops posting receive
/// buffers. The card has ~50 ms of them at 116 Mbit. After that the answer we
/// are waiting for is one of the frames that can no longer arrive, so the wait
/// runs its full budget and the radio is gone with it.
```

## L86-88 · `pub fn want(name: &str) {`

```
/// Queue a name for the background resolver. Deduplicates against the cache and
/// against the queue; drops silently when the queue is full — the caller that
/// could not be answered asks again, and that retry IS the retry.
```

## L97-102 · `if crate::smp::scheduler::worker_count() > 0`

```
// A worker resolves it (`docs/plan/CORES_AND_EVENTS.md`, stage 3). Until
// 0.418 Core 0 did, in its shell loop: `resolve` blocks up to 5.5 s, and
// everything else on Core 0 — cursor, rendering, the shell — stood still
// meanwhile; every DNS query of the microVM's browser went this way. A
// native task may block: placement gives it a free worker and marks the
// core busy, like any intent. Without workers, Core 0 keeps pumping.
```

## L110-112 · `static PUMP_RUNNING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);`

```
/// A resolver task is running (or queued). Set by `want`, cleared by the
/// task once the queue is empty — re-checked after clearing, so a name
/// queued in between is not left waiting.
```

## L130-131 · `pub fn pump_wanted() {`

```
/// Resolve one queued name on Core 0 — only on a machine without workers;
/// otherwise `want` hands the queue to a worker task.
```

## L142 · `fn remember(name: &str, ip: [u8; 4], valid: bool) {`

```
/// Write an entry, replacing the oldest when the table is full.
```

## L168 · `pub fn resolve(name: &str) -> Option<[u8; 4]> {`

```
/// Resolve a hostname to IPv4 address. Blocking (polls for reply).
```

## L170-172 · `if name.is_empty() || name.len() > 255 {`

```
// Ein leerer Name ist keine Frage. `want()` prueft das seit je, `resolve`
// nicht — und am Geraet stand deshalb `dns: reply for  carried no A
// record` im Log, eine Abfrage nach der Wurzelzone.
```

## L176-181 · `{`

```
// Check cache first — the POSITIVE one and the negative one.
//
// **Ein „den Namen gibt es nicht" ist eine Antwort und gehoert in den
// Cache.** `cached()` fuehrt das Negativfach seit je, aber `resolve`
// schrieb es nie und las es nie: jeder Aufruf mit demselben toten Namen
// zahlte die volle Runde noch einmal.
```

## L188-189 · `if e.denied && crate::interrupts::ticks().wrapping_sub(e.stamp) < NEG_TTL_TICKS {`

```
// Nur ein AUSGESPROCHENES Nein spart die Runde. Eine
// Zeitueberschreitung wird wieder versucht.
```

## L203-213 · `let dns_server = *DNS_SERVER.lock();`

```
// Warm the next hop's MAC. `arp::resolve` returns at once on a cache hit;
// the blind 100 ms spin that stood here paid its timeout on EVERY uncached
// name — five per Wikipedia load — which is why `dns` read a constant
// ~110 ms while a TCP round trip to the same network took 20.
// `arp_target_for` because a resolver off our subnet answers via the
// gateway, and warming the resolver's own IP would never complete.
//
// 300 ms, not 100: the window is paid ONCE, on a cold cache, because every
// name shares the same next hop. 100 ms held exactly one WiFi round trip,
// so a single lost frame sent the query to L2 broadcast and the first
// lookup after boot failed.
```

## L217-220 · `crate::kprintln!("[npk] dns: next hop {}.{}.{}.{} did not answer ARP in 300 ms \`

```
// Say it. The first lookup after boot fails often enough to be a known
// annoyance, and from the outside "no MAC for the next hop" and "the
// resolver did not answer" are the same silence — with opposite causes.
// Only the failing case prints, so a warm cache stays quiet.
```

## L225-226 · `if !udp::listen(LOCAL_PORT) {`

```
// The return value matters: eight listener slots exist, and a full table
// means we send four queries and listen on nothing at all.
```

## L232-242 · `const LEGS: [u64; 4] = [50, 100, 200, 200]; // 100 Hz ticks`

```
// Split into legs: UDP has no retransmit of its own, so a single dropped
// datagram used to cost the whole timeout AND then fail. Now it costs one leg.
//
// The total was 2 s and that was simply too short. Measured on the device:
// the first lookup after boot failed while the next hop's MAC was already
// known — so nothing was lost on our side, the resolver just had not answered
// yet. It is the RECURSION that takes the time; the same name a second later
// comes out of the router's cache instantly. glibc gives a server 5 s before
// it gives up, twice over, and undercutting that by more than half turned a
// slow answer into a failed one. 5.5 s, still front-loaded so the common fast
// case is unaffected.
```

## L243 · `const LEGS: [u64; 4] = [50, 100, 200, 200]; // 100 Hz ticks`

```
// 100 Hz ticks
```

## L246-250 · `let mut seen = 0u32;`

```
// Datagrams that reached our port at all, and the id of the first one we
// rejected. "Nothing arrived", "something arrived that was not ours" and
// "ours arrived and parsed to nothing" are three different faults that all
// end as one silent failure, and we have now guessed wrong about which one
// it is twice.
```

## L253-254 · `let mut answered = false;`

```
// Hat UNSERE Antwort den Weg zurueck gefunden? Das ist nicht dasselbe wie
// „steht eine Adresse darin": eine Antwort ohne A-Satz ist eine Antwort.
```

## L268-269 · `if is_reply_to(&data, id) {`

```
// Only OUR reply ends the wait — a negative answer is an
// answer, a stale one is not.
```

## L275-278 · `crate::kprintln!(`

```
// **Das ist kein Fehlschlag, das ist ein Nein.** Frueher
// folgten hier noch zwei Zeilen, die „nach 5,5 s (4
// Versuche)" behaupteten — beide Zahlen fest im Code,
// waehrend die Antwort auf Bein 1 in Millisekunden kam.
```

## L297-298 · `if answered_on > 1 {`

```
// Which leg answered is the whole question: leg 1 is a healthy resolver, a
// later one means the budget was the thing that used to fail us.
```

## L306-312 · `if result.is_none() && !answered {`

```
// A failed lookup names what it had to work with. Whether the next hop's MAC
// was known decides where to look next, and reconstructing that afterwards
// is impossible — by the time anyone asks, the cache is warm.
// **Nur wenn wirklich nichts kam.** Eine Antwort ohne A-Satz hat oben
// schon ihre eine Zeile bekommen; sie hier noch einmal als „keine Antwort"
// zu melden war die Meldung, die am Geraet sechzehnmal untereinander stand
// und nach einem Netzfehler aussah.
```

## L314-316 · `let waited_ms = crate::interrupts::ticks().wrapping_sub(t_start) * 10;`

```
// Die GEMESSENE Zeit und die Zahl der wirklich abgeschickten Beine —
// „5,5 s (4 Versuche)" stand als Konstante da und log jedes Mal, wenn
// der Weg frueher endete ([[feedback_a_value_written_once_is_not_a_measurement]]).
```

## L323-325 · `if let Some(f) = foreign_id {`

```
// Eine fremde Kennung ist ein EIGENER Befund — sie nur dann nennen,
// wenn wirklich eine kam. „0x0000 seen" stand auch da, wenn gar
// nichts Fremdes eintraf, und las sich wie ein zweiter Fehler.
```

## L334-336 · `let (no_link, tx_err) = crate::netdev::tx_reject_stats();`

```
// Did the query even reach the air? Every layer between here and the NIC
// throws the send Result away, so without this the question cannot be
// asked at all.
```

## L344-349 · `match result {`

```
// Cache result — und das NEIN genauso.
//
// Nur ein „geantwortet, keine Adresse" wird negativ gemerkt. Eine
// Zeitueberschreitung nicht: die kann eine verlorene Frame sein, und sie
// fuer die ganze TTL festzuschreiben macht aus einer Stoerung einen
// Ausfall.
```

## L362 · `pkt.extend_from_slice(&id.to_be_bytes());   // Transaction ID`

```
// Header
```

## L363 · `pkt.extend_from_slice(&id.to_be_bytes());   // Transaction ID`

```
// Transaction ID
```

## L364 · `pkt.extend_from_slice(&0x0100u16.to_be_bytes()); // Flags: standard query, recursion desired`

```
// Flags: standard query, recursion desired
```

## L365 · `pkt.extend_from_slice(&1u16.to_be_bytes());  // Questions: 1`

```
// Questions: 1
```

## L366 · `pkt.extend_from_slice(&0u16.to_be_bytes());  // Answers: 0`

```
// Answers: 0
```

## L367 · `pkt.extend_from_slice(&0u16.to_be_bytes());  // Authority: 0`

```
// Authority: 0
```

## L368 · `pkt.extend_from_slice(&0u16.to_be_bytes());  // Additional: 0`

```
// Additional: 0
```

## L370 · `for label in name.split('.') {`

```
// Question: QNAME
```

## L376 · `pkt.push(0); // root label`

```
// root label
```

## L378 · `pkt.extend_from_slice(&1u16.to_be_bytes());  // QTYPE: A (IPv4)`

```
// QTYPE: A (IPv4)
```

## L379 · `pkt.extend_from_slice(&1u16.to_be_bytes());  // QCLASS: IN`

```
// QCLASS: IN
```

## L384 · `fn is_reply_to(data: &[u8], id: u16) -> bool {`

```
/// Is this datagram a response carrying our transaction ID?
```

## L395 · `if flags & 0x8000 == 0 { return None; } // not a response`

```
// not a response
```

## L397 · `if rcode != 0 { return None; } // error`

```
// error
```

## L403 · `let mut pos = 12;`

```
// Skip questions
```

## L407 · `pos += 4; // QTYPE + QCLASS`

```
// QTYPE + QCLASS
```

## L411 · `for _ in 0..ancount {`

```
// Parse answers, look for A record
```

## L439 · `return Some(pos + 2);`

```
// Compression pointer
```

