# `kernel/src/irq.rs` @ 5e0102684

## L1-49 · `use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};`

```
//! Device-interrupt subsystem: MSI-X routing → LAPIC vector → fiber wake.
//!
//! The host kernel has no IOAPIC and the legacy 8259 PIC is fully masked
//! (re-init traps via SMI on HP/Insyde firmware — see `interrupts::init`).
//! So real-hardware device interrupts are routed via **MSI-X**: we program
//! a device's MSI-X table entry to deliver to a chosen LAPIC vector with a
//! chosen destination APIC ID. MSI-X writes go straight to the LAPIC
//! (message address `0xFEE0_0000 | apic<<12`), bypassing the PIC/IOAPIC
//! entirely, so the HP firmware quirk never bites.
//!
//! The ISR (in `interrupts.rs`) does the minimum: bump a per-vector atomic
//! fired-count + LAPIC EOI. A driver fiber parks via `wait()` until the
//! count advances (or a timeout). Crucially we target the device's MSI-X at
//! the APIC of the core running the driver fiber, so the interrupt itself
//! wakes that core out of HLT → the worker loop re-runs the scheduler →
//! the parked fiber resumes. No polling, no IPI: the IRQ is the wake.
//!
//! This closes the fiber scheduler's open "event-wake" hole and is the
//! foundation every poll-based HW driver (NVMe/NIC/audio_hda/xHCI) migrates
//! onto. First beneficiary: NVMe completion (HW-validated on the Intel H10).
//!
//! ## Driver contract (host + WASM)
//!
//! Run the driver as a **resident fiber** (pinned to its core). Once, after
//! binding the device:
//! `​``text
//!   let vec = irq::register(dev, entry);   // host  — or npk_irq_register(entry) in WASM
//! `​``
//! Then loop, servicing on the SAME fiber:
//! `​``text
//!   loop {
//!       let since = irq::arm(vec);         // snapshot + route the IRQ to THIS core
//!       // enable / submit the device work that will raise the IRQ
//!       irq::wait(vec, since, timeout_ms); // park until it fires (or timeout)
//!       // service (drain the ring / read the completion)
//!   }
//! `​``
//! **Rules that keep it correct + general:**
//! - `arm()` BEFORE the device submit closes the lost-wakeup window (an IRQ that
//!   races the park still advances the count past the snapshot).
//! - `arm()` re-routes the MSI-X dest to the calling core, so the driver may run
//!   on any core / migrate; the IRQ always wakes the core that's about to wait.
//! - **Never hold a lock across `wait()`** — a same-core fiber spinning on it
//!   would deadlock the parked waiter. Snapshot/submit under the lock, drop it,
//!   then `arm`/`wait`, then re-acquire to service.
//!
//! WASM drivers use the mirror host-fns `npk_irq_register` / `npk_irq_arm` /
//! `npk_irq_wait` (see `wasm.rs`). MSI-X when the device has it (NIC / NVMe /
//! AX200 / modern virtio), plain MSI with one vector otherwise (RTL8822CE).
```

## L57-60 · `#[derive(Clone, Copy)]`

```
/// Per-vector registration: which device/MSI-X-entry a vector drives, and the
/// LAPIC the IRQ is currently pointed at. Lets `arm()` re-route the IRQ to the
/// core that is about to wait on it — so a device's interrupt always wakes the
/// right core out of HLT regardless of which fiber/core services it.
```

## L64 · `entry: u16,`

```
/// MSI-X table entry, or `MSI` for a plain-MSI device (one vector).
```

## L66 · `last_dest: u32, // APIC ID the message currently targets`

```
// APIC ID the message currently targets
```

## L69 · `const MSI: u16 = u16::MAX;`

```
/// `IrqReg::entry` of a device driven by plain MSI instead of MSI-X.
```

## L71-72 · `const GSI: u16 = u16::MAX - 1;`

```
/// `IrqReg::entry` of a line routed through an I/O APIC; the GSI is in
/// `GSI_OF[vector]`.
```

## L83 · `static GSI_OF: [core::sync::atomic::AtomicU32; 256] =`

```
/// GSI of a vector routed through an I/O APIC.
```

## L86-91 · `static LEVEL: [core::sync::atomic::AtomicBool; 256] =`

```
/// The vector's line is LEVEL-triggered: the ISR masks it, `arm` unmasks it.
///
/// Linux' `IRQF_ONESHOT` for a threaded handler: the device keeps its line
/// asserted until the driver has serviced it, and unmasked it would fire
/// again the moment the LAPIC EOI lands — an interrupt storm on the driver's
/// core that never lets the driver run.
```

## L95-97 · `pub fn register_gsi(gsi: u32, level: bool, active_low: bool) -> Option<u8> {`

```
/// Route I/O APIC input `gsi` to a fresh vector on the CURRENT core,
/// masked. The driver unmasks it by `arm`ing it before its first `wait`.
/// None if the pool is exhausted or no I/O APIC serves the GSI.
```

## L99 · `if !crate::ioapic::is_free(gsi) {`

```
// Checked first: the vector pool is never freed.
```

## L120-121 · `pub fn isr(vector: u8) {`

```
/// Interrupt-context part of a device vector, before the LAPIC EOI: a
/// level line is masked (see `LEVEL`), then the waiter is told.
```

## L130-132 · `static IRQ_FIRED: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];`

```
/// Per-vector fired count, bumped by the ISR. Indexed by IDT vector (full
/// 256 so the ISR indexes without a bounds branch). A driver snapshots the
/// count before submitting a command, then waits for it to advance.
```

## L135-137 · `#[inline]`

```
/// Bump the fired count for `vector`. Called ONLY from the device ISR.
/// Release so a parked fiber observing the advance also sees the device
/// data the IRQ signalled.
```

## L147-148 · `static IRQ_WAITER: [core::sync::atomic::AtomicU32; 256] =`

```
/// The fiber waiting on each vector (`fiber::irq_wait`). The ISR signals it,
/// so the park ends on the interrupt itself instead of a re-check.
```

## L152 · `pub fn set_waiter(vector: u8, w: crate::smp::fiber::Waker) {`

```
/// Register `w` as the fiber waiting on `vector`.
```

## L157 · `#[inline]`

```
/// Current fired count for `vector`. Acquire pairs with `note_fired`.
```

## L163-165 · `static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);`

```
/// Next free device-IRQ vector slot. Vectors are never freed (a driver lives
/// for the boot); the pool (`DEVICE_IRQ_VEC_COUNT`) is sized for all expected
/// HW drivers.
```

## L168-169 · `pub fn alloc_vector() -> Option<u8> {`

```
/// Allocate a fresh LAPIC vector from the device-IRQ pool, or None if
/// exhausted. The matching IDT entry is already installed (`interrupts::init`).
```

## L179-188 · `pub fn arm(vector: u8) -> u64 {`

```
/// Snapshot the fired count for `vector` BEFORE submitting the device
/// command (ringing the doorbell). Pass the returned token to `wait`. This
/// closes the lost-wakeup window: an IRQ that fires between submit and park
/// still advances the count past the snapshot, so `wait` returns at once.
///
/// Also routes the IRQ to the CURRENT core (where the caller will `wait`), so
/// the device's interrupt wakes this core out of HLT → its scheduler resumes
/// the parked fiber with ~no latency. Reprograms the MSI-X dest only when the
/// waiting core changed — a no-op for a driver that always services on its own
/// pinned fiber; cheap (one MMIO write) for one that moves between cores.
```

## L196-197 · `if LEVEL[vector as usize].load(Ordering::Acquire) {`

```
// A level line was masked by the ISR: the driver has serviced the
// device, so let the next assertion through.
```

## L204-207 · `pub fn route_to_current(vector: u8) {`

```
/// Route an already-registered device IRQ to the CURRENT core. For the
/// "wake this core" usage where no fiber calls `arm`/`wait` — e.g. the host
/// NIC RX-IRQ, which targets the vCPU core so RX arrival wakes the vCPU to
/// pump. No-op if `vector` isn't registered or already targets this core.
```

## L222-225 · `pub fn wait(vector: u8, since: u64, timeout_ms: u64) -> bool {`

```
/// Park the current fiber until `vector` fires (its count moves past `since`)
/// or `timeout_ms` elapses. Returns true if the IRQ fired, false on timeout.
/// MUST be called from inside a fiber (returns false otherwise — the caller
/// should fall back to polling).
```

## L230-234 · `static IR_HANDLED: AtomicBool = AtomicBool::new(false);`

```
/// Allocate a vector and program `dev`'s MSI-X table `entry` to deliver it to
/// the CURRENT core's LAPIC. Call this from the driver fiber so the IRQ wakes
/// exactly the core that will service it. Returns the vector, or None if the
/// device has no usable MSI-X capability or the vector pool is exhausted.
/// One-shot guard for the VT-d interrupt-remapping check below.
```

## L237-247 · `fn ensure_msi_deliverable() {`

```
/// Make compatibility-format MSIs (our `0xFEE0_0000` messages) deliverable.
///
/// Intel VT-d interrupt remapping, when the platform/firmware enables it (e.g.
/// HP "Kernel DMA Protection"), BLOCKS compatibility-format interrupt requests
/// — it expects remappable-format requests indexing the IR table. That silently
/// drops every device MSI we emit (table programmed perfectly, zero IRQs). We
/// don't use IR anywhere (the LAPIC timer is a *local* interrupt; every device
/// polls today), so we disable it once → compatibility MSIs reach the LAPIC.
/// Linux works on the same HW by emitting remappable-format MSIs; we take the
/// simpler route. No DMAR (no VT-d) or IR already off → no-op. Safe: nothing in
/// this OS relies on a remapped I/O interrupt.
```

## L257-260 · `let len = unsafe { core::ptr::read_volatile((dmar + 4) as *const u32) } as usize;`

```
// DMAR: ACPI header (length @4 u32), Flags @37 (1 byte). Remapping
// structures start at +48; walk for a DRHD (type 0). Its register base is
// at DRHD+8 (after type:2 len:2 flags:1 rsvd:1 segment:2).
// SAFETY: DMAR table mapped above; reads bounded by `len` (< one page).
```

## L282 · `let page = (regbase as usize) & !0xFFF;`

```
// Map the IOMMU register page (defensive, uncached).
```

## L291 · `const GCMD: u64 = 0x18; // Global Command Register`

```
// Global Command Register
```

## L292 · `const GSTS: u64 = 0x1C; // Global Status Register`

```
// Global Status Register
```

## L293 · `let gsts = unsafe { core::ptr::read_volatile((regbase + GSTS) as *const u32) };`

```
// SAFETY: VT-d remapping-hardware MMIO at the DRHD register base.
```

## L295 · `let ires = (gsts >> 25) & 1; // Interrupt Remapping Enable Status`

```
// Interrupt Remapping Enable Status
```

## L301-304 · `let keep = gsts & ((1 << 31) | (1 << 26) | (1 << 23));`

```
// Disable IR (Linux pattern): re-assert the persistent enables we want
// to KEEP (TE bit31, QIE bit26, CFI bit23) with IRE (bit25) cleared,
// then wait for IRES to drop. Preserving TE keeps any active DMA
// translation intact; we only turn off interrupt remapping.
```

## L306 · `unsafe { core::ptr::write_volatile((regbase + GCMD) as *mut u32, keep); }`

```
// SAFETY: GCMD write per VT-d spec; only reached when IR is enabled.
```

## L325-326 · `ensure_msi_deliverable();`

```
// VT-d IR (if the platform enabled it) silently drops our compatibility-
// format MSIs — disable it once before programming any device MSI-X.
```

## L329-330 · `let (vector, entry) = if pci::has_msix(dev) {`

```
// MSI-X when the device has it; otherwise plain MSI with its one vector
// (the RTL8822CE). `entry` only means something for MSI-X.
```

