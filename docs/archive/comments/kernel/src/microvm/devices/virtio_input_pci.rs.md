# `kernel/src/microvm/devices/virtio_input_pci.rs` @ 5e0102684

## L1-20 · `#![allow(dead_code)]`

```
//! virtio-input-pci device emulation (Phase 12.4c).
//!
//! Modern virtio (1.0+) input device — vendor 0x1AF4, device 0x1052
//! (= 0x1040 + 18, virtio-spec device-id 18). Two virtqueues:
//!   q0 = eventq  (host fills with input_event structs, driver reads)
//!   q1 = statusq (driver writes LED state / etc., host acks)
//!
//! Device-cfg layout (virtio 1.2 §5.8.5):
//!
//! `​``text
//!   off  0  u8   select       (write-only, picks a property)
//!   off  1  u8   subsel       (write-only, sub-select within property)
//!   off  2  u8   size         (read-only, length of u valid for this select)
//!   off  3  u8[5] reserved
//!   off  8  u8[128] u         (read-only, content depends on select)
//! `​``
//!
//! Smoke-test scope for 12.4c: device appears in PCI scan, Linux probes
//! it, /dev/input/event0 falls out — but we never inject any events.
//! Real event injection (Shade compositor → guest) comes in 12.4d.
```

## L57 · `const CC_DEVICE_FEATURE_SELECT:  u32 = 0x00;`

```
// Common Cfg register offsets (virtio 1.2 §4.1.4.3)
```

## L78 · `const VIRTIO_INPUT_CFG_UNSET:     u8 = 0x00;`

```
// virtio-input device-cfg selectors (virtio 1.2 §5.8.4)
```

## L87 · `const EV_SYN: u8 = 0x00;`

```
// Linux input event-type codes we care about (linux/input-event-codes.h)
```

## L99-103 · `const BTN_LEFT:   u16 = 0x110;`

```
// Absolute-pointer model (qemu usb-tablet shape): the host already
// owns the cursor inside the tile, so we feed an absolute position
// normalised to a fixed logical range and let the guest's input
// stack scale it onto its display. Decouples the device from the
// live tile size (no resize round-trip on the input path).
```

## L111 · `pub const ABS_MAX: u32 = 32767;`

```
/// Logical range reported for ABS_X/ABS_Y (matches qemu usb-tablet).
```

## L117-122 · `static INPUT_Q: spin::Mutex<alloc::collections::VecDeque<(u16, u16, u32)>> =`

```
/// Pending host→guest input events (type, code, value), pushed by the
/// Shade Surface-window branch (Core 0) and drained into the eventq
/// by `drain_injected` on the timer tick. Bounded — drop oldest on
/// overflow (a wedged guest must not OOM the host). Single global:
/// one focused Surface VM at a time (forward-compat #2 — keyed
/// generalisation later, with the registry).
```

## L128-130 · `pub fn push_input_event(etype: u16, code: u16, value: u32) {`

```
/// Queue one evdev event for delivery to the guest's virtio-input
/// eventq. Caller (Shade) pushes EV_KEY then EV_SYN(SYN_REPORT) so
/// the guest's evdev dispatches it.
```

## L138 · `crate::microvm::cpu::kick_bsp_net_irq();`

```
// The BSP moves it into the eventq and raises IRQ 12; wake it.
```

## L179-181 · `cfg_select: u8,`

```
/// Driver writes `select` here; we recompute (size, data) so any
/// subsequent read of the size/data fields returns a coherent
/// snapshot. virtio 1.2 §5.8.5 explicitly requires this latching.
```

## L184-185 · `cfg_size: u8,`

```
/// Pre-computed response for the current (select, subsel).
/// `cfg_data[0..cfg_size]` is the meaningful prefix.
```

## L233-236 · `pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {`

```
/// Process queue notify. q0 = eventq (driver fills with empty
/// buffers, host will populate when events arrive — for 12.4c we
/// have no events to inject, just leave buffers queued).
/// q1 = statusq (driver writes LED state, we ack via used-ring).
```

## L244-250 · `pub fn drain_injected(&mut self, mem: &GuestMem) -> bool {`

```
/// Drain pending host events into the eventq (q0). The guest's
/// virtio-input driver keeps the eventq stocked with empty
/// writable buffers; we write one `virtio_input_event`
/// {u16 type; u16 code; u32 value} (8 bytes, LE) per buffer and
/// publish it on the used-ring. Returns true if anything was
/// delivered (caller injects the input IRQ). Called on the timer
/// tick (mirrors the NAT pump).
```

## L271 · `None => break, // no more events; leave buffers queued`

```
// no more events; leave buffers queued
```

## L295-297 · `fn service_statusq(&mut self, mem: &GuestMem) -> bool {`

```
/// Drain the statusq: every buffer the driver wrote (LED state,
/// repeat-rate, etc.) gets immediately pushed onto the used-ring.
/// We don't actually do anything with the state.
```

## L329 · `0x04 => (0x0010 << 16) | 0x0007,`

```
// status (cap-list bit) | command (mem + bus-master + io)
```

## L331 · `0x08 => (0x09_00_00 << 8) | 0x01,`

```
// class 09_00_00 (Input device controller) | revision 0x01
```

## L338 · `0x2C => (0x0012 << 16) | 0x1AF4,`

```
// Subsystem vendor 0x1AF4 / Subsystem device 0x0012 (input)
```

## L343-344 · `0x3C => 0x0000_010C,`

```
// Interrupt: line=12, pin=INTA. Conventional PS/2-mouse IRQ
// line, free in our microvm (no real PS/2 controller).
```

## L347 · `0x40 => 0x09 | ((CAP_NOTIFY_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_COMMON_CFG as u32) << 24),`

```
// Modern virtio capability list — same shape as the rest.
```

## L426 · `1 // VIRTIO_F_VERSION_1 (bit 32)`

```
// VIRTIO_F_VERSION_1 (bit 32)
```

## L428 · `0 // no virtio-input feature bits in <32 range`

```
// no virtio-input feature bits in <32 range
```

## L503-504 · `let mask = width_mask(width);`

```
// Device-cfg map: 0=select, 1=subsel, 2=size, 3..7=reserved,
//                 8..136=u (128 bytes).
```

## L530 · `_ => {} // size + reserved + u are RO from driver POV`

```
// size + reserved + u are RO from driver POV
```

## L536-539 · `fn recompute_cfg(&mut self) {`

```
/// Latch the (size, u) response for the current (select, subsel).
/// virtio 1.2 §5.8.5: the device MUST guarantee that once `select`
/// is set, `size` and `u` reflect that selection coherently — i.e.
/// we re-derive on every selector write, not lazily on read.
```

## L550 · `self.cfg_data[0] = 0x06; self.cfg_data[1] = 0x00;     // bustype VIRTUAL`

```
// virtio_input_devids { bustype, vendor, product, version } LE u16 × 4
```

## L551 · `self.cfg_data[0] = 0x06; self.cfg_data[1] = 0x00;     // bustype VIRTUAL`

```
// bustype VIRTUAL
```

## L552 · `self.cfg_data[2] = 0xF4; self.cfg_data[3] = 0x1A;     // vendor 0x1AF4`

```
// vendor 0x1AF4
```

## L553 · `self.cfg_data[4] = 0x01; self.cfg_data[5] = 0x00;     // product 1`

```
// product 1
```

## L554 · `self.cfg_data[6] = 0x01; self.cfg_data[7] = 0x00;     // version 1`

```
// version 1
```

## L557 · `VIRTIO_INPUT_CFG_PROP_BITS => 0, // no input device properties`

```
// no input device properties
```

## L560-562 · `match self.cfg_subsel as u16 {`

```
// virtio_input_absinfo { min, max, fuzz, flat, res }
// LE u32 × 5. Same range for both axes; the guest
// scales 0..ABS_MAX onto its display.
```

## L567 · `20`

```
// fuzz / flat / res stay zero (8..20).
```

## L577-579 · `fn fill_ev_bits(&mut self) -> u8 {`

```
/// Bitmap of supported codes for the requested EV_TYPE in
/// `cfg_subsel`. Linux iterates subsel=0..EV_CNT; a non-zero size
/// signals "this type supported".
```

## L582 · `x if x == EV_SYN => {`

```
// EV_SYN — bit 0 (SYN_REPORT)
```

## L587-594 · `x if x == EV_KEY => {`

```
// EV_KEY — advertise the full 0..=255 keycode range (32
// bytes). Linux's input core drops any EV_KEY whose code
// isn't set in dev->keybit (test_bit in input_handle_event)
// — so declaring only KEY_ESC/KEY_A meant every other
// injected key (e.g. KEY_SPACE=57) was silently filtered
// before evdev, even though the eventq/IRQ path worked.
// Covering the standard keyboard range fixes injection now
// and is what Phase B's real per-scancode mapping needs.
```

## L596 · `let _ = (KEY_ESC, KEY_A); // (kept as named refs)`

```
// (kept as named refs)
```

## L600-605 · `self.set_bit(BTN_LEFT as usize);`

```
// Pointer buttons live above the keyboard range
// (BTN_LEFT = 0x110). Set exactly the three we inject —
// NOT a blanket 0x100..0x2ff — so the guest input stack
// classifies this as a plain pointer, not a tablet
// (BTN_TOOL_*/BTN_TOUCH would flip libinput to tablet
// semantics later in Phase B).
```

## L609-612 · `(BTN_MIDDLE / 8 + 1) as u8 // 35: covers up to BTN_MIDDLE`

```
// u16 math: BTN_MIDDLE=0x112 → 35. Must NOT cast to u8
// before the divide (0x112 as u8 = 18 → size 3, which
// truncates the bitmap below KEY_SPACE=57 and Linux's
// input core then filters every real key).
```

## L613 · `(BTN_MIDDLE / 8 + 1) as u8 // 35: covers up to BTN_MIDDLE`

```
// 35: covers up to BTN_MIDDLE
```

## L615-616 · `x if x == EV_REL => {`

```
// EV_REL — wheel only (absolute model handles motion via
// EV_ABS; no REL_X/REL_Y). Browsers need vertical scroll.
```

## L620 · `(REL_WHEEL / 8 + 1) as u8 // 2 bytes`

```
// 2 bytes
```

## L622 · `x if x == EV_ABS => {`

```
// EV_ABS — ABS_X / ABS_Y (0..ABS_MAX), see ABS_INFO below.
```

## L628 · `x if x == EV_MSC => 0,`

```
// EV_MSC / EV_REP — declare unsupported (size=0).
```

