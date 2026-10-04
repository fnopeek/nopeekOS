# `kernel/src/net/dhcp.rs` @ 5e0102684

## L1-4 · `use alloc::vec::Vec;`

```
//! DHCP — Dynamic Host Configuration Protocol
//!
//! Auto-configures IP address, gateway, and DNS server.
//! DHCP discover → offer → request → ack over UDP 67/68.
```

## L13 · `const DHCP_MAGIC: [u8; 4] = [99, 130, 83, 99]; // DHCP magic cookie`

```
// DHCP magic cookie
```

## L20-26 · `static LEASE_MAC: spin::Mutex<[u8; 6]> = spin::Mutex::new([0; 6]);`

```
/// What to leave behind when DHCP fails. The QEMU user-mode address only means
/// something under QEMU; on real hardware it is a fiction that makes `net` show
/// an address nobody can reach — and, worse, makes the retry logic think a lease
/// exists. Outside QEMU we leave 0.0.0.0, which is the truth and what the
/// link-state tick keys its retry on.
/// MAC the current lease was issued to. A lease is only ours to re-request from
/// the interface that got it.
```

## L29-33 · `static LEASE_GW_MAC: spin::Mutex<[u8; 6]> = spin::Mutex::new([0; 6]);`

```
/// The gateway's MAC at the time the lease was granted. If the same one answers
/// after a link came back, we are on the same segment and the lease still holds:
/// no DHCP is needed at all. This is what dhcpcd does before it considers any
/// exchange, and it is what turns a mesh hand-off from a multi-second stall into
/// one ARP round trip.
```

## L40 · `arp::set_ip([10, 0, 2, 15]); // QEMU user-mode default`

```
// QEMU user-mode default
```

## L46-56 · `const BCAST: [u8; 4] = [255, 255, 255, 255];`

```
// ── The exchange, as a state machine ─────────────────────────────────────
//
// This used to be straight-line code with a three-second busy-spin per reply,
// run up to three times over: up to nine seconds in which Core 0 did nothing
// else. Core 0 is the terminal, so that was the whole machine stopping — and it
// stopped LONGEST exactly when the link was broken, which is when a user most
// wants a prompt. Every step below returns immediately; `tick()` picks the
// reply up on a later pass of the Core-0 loop.
//
// This is safe because a UDP listener keeps the last datagram for its port
// until someone takes it: a reply landing between two ticks waits for us.
```

## L60-62 · `const REPLY_WAIT_MS: u64 = 1000;`

```
/// How long one attempt waits before it is re-sent. A server on a working link
/// answers in milliseconds; the old three seconds only ever elapsed on a link
/// that was not going to answer at all.
```

## L69 · `Reboot,`

```
/// INIT-REBOOT (RFC 2131 §4.4.2): we still hold an address, waiting for ACK.
```

## L80 · `deadline: u64, // rdtsc`

```
// rdtsc
```

## L85-87 · `static GW_PENDING: core::sync::atomic::AtomicBool =`

```
/// A lease is in, but the gateway has not answered ARP yet. Its MAC is what
/// lets the NEXT link change skip the exchange entirely, so it is worth
/// recording late rather than waiting for it now.
```

## L91 · `pub enum Start {`

```
/// What `start()` did — it never blocks, so the caller needs to be told.
```

## L93 · `Kept,`

```
/// The old lease is still valid; nothing is on the wire.
```

## L95 · `Running,`

```
/// An exchange is running. `tick()` will finish it.
```

## L97 · `Unavailable,`

```
/// No interface to do it on.
```

## L110 · `pub fn start() -> Start {`

```
/// Begin a lease. Returns at once.
```

## L120-126 · `let prev = arp::our_ip();`

```
// Keep our previous lease if we had a real one, so re-running DHCP (e.g. on
// a link switch) doesn't needlessly churn the address — but ONLY if it was
// this interface's lease. The address is global state while a lease belongs
// to one MAC: hinting the wired NIC's address from the WiFi NIC asks the
// server for something it has already given to someone else, so it hands out
// a different one, and switching back repeats it in reverse. That is the
// .72/.73 ping-pong on a machine with both interfaces up.
```

## L135-139 · `if hint != [0, 0, 0, 0] {`

```
// Rung 0 (dhcpcd's shortcut, not in the RFC): the same gateway MAC means the
// same segment, so the lease we hold is still good and there is nothing to
// renegotiate. Cache-only — this used to block up to 300 ms on an ARP round
// trip, and the whole point here is that nothing blocks. A cold cache just
// means we take the rung below instead.
```

## L150 · `Some(_) => {} // a different gateway — really is a new segment`

```
// a different gateway — really is a new segment
```

## L151 · `None => arp::request(gw), // warm it for next time, don't wait`

```
// warm it for next time, don't wait
```

## L159-162 · `let phase = if hint != [0, 0, 0, 0] {`

```
// Rung 1, INIT-REBOOT: we still hold an address, so ask for THAT one — a
// broadcast REQUEST carrying it in option 50 and no server identifier. One
// round trip when the server still knows us. Silence falls through to the
// full exchange, which is the point of the rung.
```

## L183-184 · `pub fn tick() {`

```
/// Step the exchange. Cheap and a no-op when nothing is running — call it from
/// the Core-0 loop as often as convenient.
```

## L188-189 · `let mut ex = match EXCHANGE.lock().take() {`

```
// Taken out of the lock for the duration: the step below sends packets and
// prints, and nothing else may start a second exchange meanwhile.
```

## L278-279 · `arp::announce();`

```
// Tell the segment which MAC owns this address now — without it the router
// keeps sending to the interface we just left.
```

## L304-305 · `fn settle_gateway() {`

```
/// Record the gateway's MAC once ARP answers, so the next link change can take
/// rung 0 above. Costs a cache lookup per tick while outstanding, nothing after.
```

## L322-324 · `pub fn run_blocking(max_ms: u64) -> bool {`

```
/// Boot only: start an exchange and step it to a conclusion. Nothing else runs
/// this early and the rest of boot (NTP) wants an address, so waiting here is
/// honest — unlike the ~1 Hz link tick, which must never block the terminal.
```

## L343 · `pkt[0] = 1;      // op: BOOTREQUEST`

```
// op: BOOTREQUEST
```

## L344 · `pkt[1] = 1;      // htype: Ethernet`

```
// htype: Ethernet
```

## L345 · `pkt[2] = 6;      // hlen: MAC length`

```
// hlen: MAC length
```

## L346 · `pkt[3] = 0;      // hops`

```
// hops
```

## L347 · `pkt[4..8].copy_from_slice(&0xDEADBEEFu32.to_be_bytes()); // xid`

```
// xid
```

## L348-352 · `pkt[28..34].copy_from_slice(mac); // chaddr (16 bytes, MAC + padding)`

```
// secs, flags at 8..12 = 0
// ciaddr at 12..16 = 0
// yiaddr at 16..20 = 0
// siaddr at 20..24 = 0
// giaddr at 24..28 = 0
```

## L353 · `pkt[28..34].copy_from_slice(mac); // chaddr (16 bytes, MAC + padding)`

```
// chaddr (16 bytes, MAC + padding)
```

## L355 · `pkt[236..240].copy_from_slice(&DHCP_MAGIC);`

```
// DHCP magic cookie at offset 236
```

## L358 · `let mut pos = 240;`

```
// Options start at 240
```

## L361 · `pkt[pos] = 53; pkt[pos + 1] = 1; pkt[pos + 2] = msg_type;`

```
// Option 53: DHCP Message Type
```

## L365-366 · `if requested_ip != [0; 4] {`

```
// Option 50: Requested IP — on the REQUEST (the offered address) and, as a
// hint, on a DISCOVER when we want the server to keep our previous lease.
```

## L372 · `if msg_type == MSG_REQUEST && server_ip != [0; 4] {`

```
// Option 54: Server Identifier (REQUEST only)
```

## L379 · `pkt[pos] = 55; pkt[pos + 1] = 3;`

```
// Option 55: Parameter Request List (router, DNS, subnet mask)
```

## L381 · `pkt[pos + 2] = 1;  // Subnet mask`

```
// Subnet mask
```

## L382 · `pkt[pos + 3] = 3;  // Router`

```
// Router
```

## L383 · `pkt[pos + 4] = 6;  // DNS`

```
// DNS
```

## L386 · `pkt[pos] = 255;`

```
// End option
```

## L395 · `if data[0] != 2 { return None; } // not BOOTREPLY`

```
// not BOOTREPLY
```

## L397 · `if data[236..240] != DHCP_MAGIC { return None; }`

```
// Check magic cookie
```

## L402 · `let mut pos = 240;`

```
// Parse options
```

## L412 · `if opt == 255 { break; } // end`

```
// end
```

## L413 · `if opt == 0 { pos += 1; continue; } // padding`

```
// padding
```

## L433 · `if router != [0; 4] {`

```
// Apply gateway, subnet, and DNS
```

