# `kernel/src/microvm/devices/virtqueue.rs` @ 5e0102684

## L1-14 · `#![allow(dead_code)]`

```
//! Virtqueue + virtio-blk request servicing.
//!
//! virtio 1.x split-virtqueue layout (per spec §2.7):
//!
//! `​``text
//!   desc:     array of vring_desc[queue_size]                    16 B each
//!   avail:    flags(2) | idx(2) | ring[size](2 each) | used_event(2)
//!   used:     flags(2) | idx(2) | elem[size](id u32 + len u32)   8 B each
//! `​``
//!
//! virtio-blk request shape (3+ descriptors per request):
//!   [0]   driver-readable header  (16 B: type + reserved + sector)
//!   [1..] data buffer(s)          (read or write per VRING_DESC_F_WRITE)
//!   [n]   driver-writable status  (1 B)
```

## L20 · `pub const VRING_DESC_F_NEXT:     u16 = 1;`

```
// vring_desc flags
```

## L25 · `const VIRTIO_BLK_T_IN:     u32 = 0;`

```
// virtio-blk request types
```

## L31 · `const VIRTIO_BLK_S_OK:     u8 = 0;`

```
// Status return codes (1 byte at the tail descriptor)
```

## L57 · `pub fn avail_idx(mem: &GuestMem, avail_gpa: u64) -> Option<u16> {`

```
/// Read the current driver-side avail-ring head index.
```

## L62-65 · `pub const VIRTQ_AVAIL_F_NO_INTERRUPT: u16 = 1;`

```
/// VIRTQ_AVAIL_F_NO_INTERRUPT — the driver sets this in avail.flags (@ +0) while
/// it polls the ring (Linux NAPI does this between the IRQ and re-enabling), to
/// tell the device NOT to send an interrupt. Honouring it avoids a spurious-IRQ
/// storm that preempts the guest's ring-drain.
```

## L68-69 · `pub fn avail_no_interrupt(mem: &GuestMem, avail_gpa: u64) -> bool {`

```
/// True if the driver has suppressed interrupts on this queue (NAPI poll in
/// progress). Falls back to "interrupt wanted" if the flags can't be read.
```

## L74 · `pub fn avail_ring(mem: &GuestMem, avail_gpa: u64, queue_size: u16, slot: u16) -> Option<u16> {`

```
/// Read the descriptor head index for a given slot in the avail-ring.
```

## L80-81 · `pub fn used_push(mem: &GuestMem, used_gpa: u64, queue_size: u16, used_idx: &mut u16, head: u16, len: u32) {`

```
/// Push a (head, len) pair to the used ring and bump used.idx with a
/// release-fence so the descriptor writes settle first.
```

## L92-94 · `pub fn service_blk_queue(`

```
/// Walk the available ring from `last_avail_idx` to the current head
/// and service each request. Returns true if any request completed
/// (caller should set ISR + inject IRQ).
```

## L105-106 · `let avail_idx = match mem.read_u16(avail + 2) {`

```
// avail.flags @ +0 (ignored — VIRTIO_F_RING_EVENT_IDX off)
// avail.idx   @ +2
```

## L118 · `let ring_slot = (*last_avail_idx % queue_size) as u64;`

```
// ring[i] @ avail + 4 + (i % size) * 2
```

## L129 · `let used_slot = (*used_idx % queue_size) as u64;`

```
// used.elem[used_idx % size] = (head_idx, total_written)
```

## L141-143 · `core::sync::atomic::fence(core::sync::atomic::Ordering::Release);`

```
// Memory ordering: descriptor writes must be visible before
// the used.idx update. On x86 this is implicit (writes are
// ordered) but we add a compiler fence for clarity.
```

## L145 · `mem.write_u16(used + 2, *used_idx);`

```
// used.idx @ +2
```

## L152-155 · `fn service_one_request(`

```
/// Walk one descriptor chain, perform the I/O, write the status byte.
/// Returns the number of bytes written into the guest's buffers (used
/// in the used-ring `len` field — virtio-blk convention is data bytes
/// + 1 for the status byte on a successful read).
```

## L163 · `let head = match read_desc(mem, desc_table, head_idx, queue_size) {`

```
// [0] header
```

## L169 · `return 0; // malformed`

```
// malformed
```

## L181 · `return 0; // header alone — invalid`

```
// header alone — invalid
```

## L184-185 · `let mut idx = head.next;`

```
// Walk descriptors after the header. The chain ends with a 1-byte
// writable descriptor for status.
```

## L201 · `if !writable || d.len < 1 {`

```
// Tail descriptor — must be 1-byte writable status.
```

## L203 · `return bytes_written;`

```
// No status slot — best-effort, just bail.
```

## L210 · `match req_type {`

```
// Middle descriptor — data buffer.
```

## L213 · `if !writable { status = VIRTIO_BLK_S_IOERR; }`

```
// Device writes data into guest memory.
```

## L241 · `if writable {`

```
// Device writes a 20-byte ASCII serial number.
```

## L250 · `}`

```
// No-op — backing is in-RAM.
```

## L260-261 · `bytes_written = bytes_written.saturating_add(1);`

```
// Spec convention: include the status byte in the bytes-written
// count for read responses. Linux's blk layer relies on this.
```

