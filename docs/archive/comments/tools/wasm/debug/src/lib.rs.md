# `tools/wasm/debug/src/lib.rs` @ 5e0102684

## L1-8 · `#![no_std]`

```
//! debug — reverse debug shell (WASM module)
//!
//! Mirrors the window's terminal over TCP to a `nc -l <port>` listener
//! on the developer's machine. Dials out (reverse-shell style), no auth,
//! no crypto — feature is temporary, will be replaced by real SSH later.
//!
//! Usage:  run debug <ip> <port>
//! On laptop:  nc -l 22222
```

## L19 · `const VERSION: &str = "0.7.0";`

```
/// One source for the version, printed in the banner.
```

## L25-30 · `#[unsafe(link_section = ".npk.caps")]`

```
/// **`debug` braucht das Netz** — es schreibt sein Protokoll ueber einen
/// rohen TCP-Socket an ein `nc -lk` auf dem Entwicklerrechner.
///
/// Bis Kernel 0.337.0 stand hier nichts, und es lief trotzdem: die fuenf
/// `npk_tcp_*` prueften GAR KEINE Kapabilitaet. Jetzt tun sie es, und damit
/// muss das Recht dastehen, wie bei jedem anderen Modul auch.
```

## L33-35 · `static NPK_CAPS: [u8; 2] = [0x01 | 0x04 | 0x08, 0x01];   // READ|EXEC|RENDER, ext: NET`

```
// READ ist dabei, weil die Vorgabe ohne Sektion `READ | EXECUTE | RENDER`
// ist — wer eine Sektion hinschreibt, ERSETZT die Vorgabe und muss alles
// nennen, was er behalten will.
```

## L36 · `static NPK_CAPS: [u8; 2] = [0x01 | 0x04 | 0x08, 0x01];   // READ|EXEC|RENDER, ext: NET`

```
// READ|EXEC|RENDER, ext: NET
```

## L40-43 · `host::print("[debug] reverse-mirror ");`

```
// The banner carries the version because the failure mode of the OLD one —
// mirroring a single terminal, so the console goes quiet the moment output
// is routed elsewhere — looks exactly like a broken connection. Knowing
// which one is running is the first question, every time.
```

## L69-74 · `let mut sink = -1i32;`

```
// Open the everything-sink (-1): every write, whichever terminal the kernel
// routed it to. Bound to one index this mirror went quiet the moment output
// was redirected — background messages go to the primary loop, a command's
// output to the loop it was typed in — and it looked like the machine had
// stopped answering while it was still printing. Older kernels do not know
// -1, so fall back to our own terminal.
```

## L85-96 · `let mut attempt: u32 = 0;`

```
// Dial, then wait OUTSIDE the host call. The connect used to block in the
// kernel until ESTABLISHED or a 10 s timeout — and this module is a fiber,
// so those 10 s froze every other fiber on the same worker core, the WiFi
// driver included: a failing `debug` took the link down with it. Sleeping
// between polls leaves the core, so the driver keeps draining its card.
// Outer loop: a mirror that gives up the moment the link hiccups is
// useless exactly when it is needed. The far end must keep listening too
// (`nc -lk`, or `while true; do nc -l PORT; done`) — plain `nc -l` takes
// ONE connection and exits.
//
// The sink stays open across reconnects, so output produced while we were
// away is still in its 64 KB ring when we come back.
```

## L102 · `connected = false;`

```
// 10 s at 20 ms — same ceiling the kernel used to enforce.
```

## L115-116 · `if attempt <= 3 {`

```
// Say it the first few times, then stop — the retry itself is silent
// work and this print goes to the device's own screen.
```

## L122 · `let back = 2000u32.saturating_mul(1 << attempt.min(4)) / 2;`

```
// 2 s, 4 s, 8 s … capped at 30 s.
```

## L128-132 · `host::print("[debug] connected — reverse-mirror ");`

```
// Say who we are AFTER the sink and the socket exist — the banner above is
// printed before either, so it only ever reached the device's own screen.
// Which version is running, and whether it got the global mirror, is the
// first question when output stops arriving; it has to be answerable from
// the far end.
```

## L137 · `let mut tx_buf = [0u8; 1024];`

```
// Relay loop. Poll both directions with a short sleep to yield the core.
```

## L147-153 · `round = round.wrapping_add(1);`

```
// Did the far end hang up? `tcp_recv` never says so — it just returns
// 0 bytes forever in CloseWait, which is why closing `nc` used to
// leave this module running with nothing to talk to.
// Every 16th round only: this takes the kernel's CONNECTIONS lock,
// and under load the RX path takes the same lock tens of thousands
// of times a second. Asking every round put a third acquisition in
// the hot loop for an answer that changes once per session.
```

## L157-160 · `-2 => {`

```
// -2 is the interesting one: the connection did not end, it FAILED
// — five retransmits with no acknowledgement, i.e. the link went
// dead under us for about six seconds. Printing the same words for
// both hid exactly the event worth chasing.
```

## L171 · `let n = host::stream_read(sink, &mut tx_buf);`

```
// Terminal output → TCP
```

## L176-178 · `-2 => {`

```
// -2 = too much unacknowledged. Not a failure: give the
// retransmit a moment and offer the SAME bytes again, or the
// mirror loses exactly the output the developer is waiting for.
```

## L191-197 · `dropped = dropped.saturating_add(1);`

```
// Backpressure is not failure. A MIRROR may lose lines
// — the sink ring already drops the oldest on overflow
// — but it must never disconnect over it. Closing here
// meant that under a saturating download the mirror
// shut itself down after two seconds, and announced it
// over the very connection it was closing: from the far
// end, silence with no reason given.
```

## L212 · `let n = host::tcp_recv(sock, &mut rx_buf);`

```
// TCP input → keyboard inject
```

## L225 · `if did_work {`

```
// Adaptive sleep: brief when active, longer when idle.
```

## L235-237 · `host::tcp_close(sock);`

```
// Relay ended. Close OUR side and dial again — the reason was already
// printed above, and how long the gap lasted is visible from the two
// timestamps in the surrounding log.
```

