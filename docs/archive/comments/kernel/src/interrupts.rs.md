# `kernel/src/interrupts.rs` @ 5e0102684

## L1-4 · `use crate::serial::outb;`

```
//! Interrupt Descriptor Table + PIC 8259
//!
//! Exception handlers + timer IRQ for hlt wakeup.
//! Phase 2+: keyboard IRQ, serial IRQ, TSS with IST for double fault
```

## L10-12 · `static TICKS: AtomicU64 = AtomicU64::new(0);`

```
/// Timer interrupts taken on Core 0. NOT a clock: only used to tell
/// whether the PIT is running (`init_apic_timer`) and to pace the once-a-
/// second statistics in the tick handler.
```

## L15 · `static BOOT_TSC: AtomicU64 = AtomicU64::new(0);`

```
/// TSC value at boot — the zero point of every clock below.
```

## L18 · `pub fn init_tsc_ticks() {`

```
/// Call once at boot after calibrate_tsc().
```

## L23-29 · `pub fn ticks() -> u64 {`

```
/// 100 Hz ticks since boot, derived from the TSC.
///
/// **The clock is the TSC, on every machine.** This used to return the PIT
/// interrupt count whenever the PIT was running: a clock that only advances
/// when Core 0 takes an interrupt stops while Core 0 runs with IF=0, and
/// could not survive the tick going away (`docs/plan/CORES_AND_EVENTS.md`
/// §3.2). The unit stays 10 ms so the 223 callers read the same numbers.
```

## L32 · `let period = freq / 100; // TSC cycles per 10ms tick`

```
// TSC cycles per 10ms tick
```

## L38 · `pub fn uptime_secs() -> u64 {`

```
/// Seconds since boot (approximate)
```

## L43-49 · `pub fn uptime_us() -> u64 {`

```
/// Microseconds since boot, from the TSC.
///
/// **`ticks()` is 100 Hz, and that is too coarse for a receive-side RTT.**
/// On this link an RTT of 5 ms measures as "0 ticks" and one of 20 ms as
/// one or two — so any window derived from it is a step function with
/// 10 ms steps. Linux' DRS (`tcp_rcv_space_adjust`) compares an elapsed
/// time against `rcv_rtt_est.rtt_us`, in microseconds; this is that clock.
```

## L57-58 · `pub fn tsc_to_ns(cycles: u64) -> u64 {`

```
/// TSC-Takte in Nanosekunden. Fuer Messungen, die kleiner sind als eine
/// Mikrosekunde — die Kosten EINES TCP-Segments zum Beispiel.
```

## L64 · `pub fn rdtsc() -> u64 {`

```
/// Read CPU Time Stamp Counter (works on all x86_64, no PIC needed).
```

## L72 · `static TSC_FREQ: AtomicU64 = AtomicU64::new(2_000_000_000); // default 2GHz`

```
/// Estimate TSC frequency by calibrating against PIT (called once at boot).
```

## L73 · `static TSC_FREQ: AtomicU64 = AtomicU64::new(2_000_000_000); // default 2GHz`

```
// default 2GHz
```

## L75 · `static CPUID15_EAX: AtomicU64 = AtomicU64::new(0);`

```
/// Raw CPUID 0x15 values for diagnostics (stored at boot)
```

## L80 · `pub fn cpuid15() -> (u32, u32, u32) {`

```
/// Get raw CPUID 0x15 values: (eax, ebx, ecx)
```

## L88-89 · `let eax: u32;`

```
// CPUID leaf 0x15: TSC frequency = ECX * EBX / EAX
// EAX = denominator, EBX = numerator, ECX = crystal clock (Hz)
```

## L94 · `let ebx_out: u64;`

```
// rbx is reserved by LLVM, so save/restore manually
```

## L126-132 · `if let Some(freq) = pit_calibrate_tsc() {`

```
// CPUID 0x15 absent (always on AMD — Intel-only leaf). Calibrate
// against PIT channel 2 (vendor-independent; Linux's
// quick_pit_calibrate approach). MUST work or every TSC-derived
// time is wrong: host `top` per-app time, delay_ms, run_slice's
// SLICE_MS, AND the guest's `tsc_early_khz=` cmdline (→ guest
// clock 2-2.3× fast → librewolf timed-wait crash). The old
// hardcoded 2 GHz was that bug.
```

## L139 · `TSC_FREQ.store(2_000_000_000, Ordering::Relaxed);`

```
// Last resort: 2 GHz default (PIT also unavailable).
```

## L147 · `unsafe {`

```
// SAFETY: port I/O read; the caller picks the port (PIT calibration only).
```

## L157 · `unsafe {`

```
// SAFETY: port I/O write; the caller picks the port (PIT calibration only).
```

## L164-169 · `fn pit_calibrate_tsc() -> Option<u64> {`

```
/// Measure TSC over a fixed PIT channel-2 window (no IRQs needed —
/// polled via the timer-2 output bit in port 0x61). Channel 2 is the
/// "speaker" timer; gating it via 0x61 starts a one-shot countdown we
/// can poll, so this works before any IRQ/timer setup and on both
/// bare-metal AMD and QEMU/KVM. Returns the TSC frequency in Hz, or
/// None if the PIT isn't counting / the result is implausible.
```

## L171 · `unsafe {`

```
// SAFETY: legacy PIT/0x61 ports, single-threaded boot context.
```

## L173 · `let p61 = cal_inb(0x61);`

```
// Gate2 on, speaker off (preserve the other bits to restore).
```

## L176-178 · `cal_outb(0x43, 0xB0);`

```
// Channel 2, access lobyte+hibyte, mode 0 (terminal count),
// binary. Mode 0: OUT (0x61 bit 5) is low while counting,
// goes high at terminal count.
```

## L180-181 · `cal_outb(0x42, 0xFF);`

```
// Initial count 0xFFFF → 0x10000 input clocks @ 1.193182 MHz
// ≈ 54.9 ms calibration window.
```

## L190 · `cal_outb(0x61, p61 & 0xFC); // gate off, restore`

```
// gate off, restore
```

## L191 · `return None; // PIT not advancing`

```
// PIT not advancing
```

## L195 · `cal_outb(0x61, p61 & 0xFC); // gate off, restore original bits`

```
// gate off, restore original bits
```

## L198 · `let freq = cycles.saturating_mul(PIT_BASE_FREQ as u64) / 0x10000;`

```
// 0x10000 PIT clocks at PIT_BASE_FREQ Hz.
```

## L208 · `pub fn tsc_freq() -> u64 {`

```
/// Get TSC frequency in Hz.
```

## L213 · `pub fn delay_ms(ms: u64) {`

```
/// Busy-wait for approximately `ms` milliseconds using TSC.
```

## L222-223 · `#[allow(dead_code)]`

```
// Kept next to the ports we do drive: a register map with holes in it is
// worse than one unused constant (see feedback_verify_reg_addrs).
```

## L229 · `const TARGET_FREQ: u32 = 100; // 100 Hz = 10ms per tick`

```
// 100 Hz = 10ms per tick
```

## L231 · `#[derive(Clone, Copy)]`

```
// IDT entry: 64-bit interrupt gate descriptor (16 bytes)
```

## L256 · `self.selector = 0x08; // GDT code segment from boot.s`

```
// GDT code segment from boot.s
```

## L258-259 · `self.type_attr = 0x8E;`

```
// 0x8E = Present | DPL=0 | 64-bit Interrupt Gate
// DPL=0: no ring-3 software interrupt injection possible
```

## L282 · `static mut IDT: [IdtEntry; IDT_SIZE] = [IdtEntry::missing(); IDT_SIZE];`

```
// SAFETY: Written exactly once in init() before sti, then only read by CPU
```

## L290 · `const PIC_OFFSET_MASTER: u8 = 32; // IRQ0-7 → vectors 32-39`

```
// IRQ0-7 → vectors 32-39
```

## L291 · `#[allow(dead_code)] // the master's twin; the map stays complete`

```
// the master's twin; the map stays complete
```

## L292 · `const PIC_OFFSET_SLAVE: u8 = 40;  // IRQ8-15 → vectors 40-47`

```
// IRQ8-15 → vectors 40-47
```

## L296-299 · `IDT[0].set_handler(crate::forge_rt::forge_de_stub as *const () as u64);`

```
// Exception handlers
// Both go through a stub that first asks whether the fault came from
// compiled wasm; if it did, it is that module's trap and not the
// kernel's. Anything else lands on the handler that was here before.
```

## L308 · `IDT[PIC_OFFSET_MASTER as usize].set_handler(timer_handler as *const () as u64);`

```
// Hardware interrupt handlers
```

## L311-313 · `IDT[WORKER_TIMER_VECTOR as usize]`

```
// Per-core worker idle timer (one per AP) — pure EOI, see
// `init_worker_timer`. Shared IDT; each worker raises it on its
// own LAPIC so its idle HLT wakes without a host tick.
```

## L316 · `IDT[WORKER_WAKE_VECTOR as usize]`

```
// Wake IPI for an idle worker (new work in the inbox) — pure EOI.
```

## L320 · `IDT[VCPU_KICK_VECTOR as usize]`

```
// Cross-vCPU kick IPI (guest SMP) — prompt inter-vCPU IPI delivery.
```

## L323-324 · `IDT[PS2_VECTOR as usize].set_handler(ps2_irq_handler as *const () as u64);`

```
// Input by interrupt (stage 3b): the i8042 through the I/O APIC,
// the xHCI controllers through MSI-X — both to Core 0.
```

## L327 · `install_device_isrs();`

```
// Device-IRQ pool (MSI-X → LAPIC vector → fiber wake). See `crate::irq`.
```

## L330 · `let idt_reg = IdtRegister {`

```
// Load IDT
```

## L335 · `core::arch::asm!("lidt [{}]", in(reg) &idt_reg);`

```
// SAFETY: IDT is fully initialized above
```

## L338-347 · `outb(PIC1_DATA, 0xFF);`

```
// Do NOT re-initialize the legacy 8259 PIC. Writing ICW1 (0x11)
// to its command port (0x20/0xA0) traps via SMI on some UEFI
// firmware (HP/Insyde virtualize the legacy PIC in SMM under APIC
// mode) and resets the machine post-ExitBootServices — confirmed
// by colour-bisecting a serial-less HP notebook. UEFI hands control
// over in APIC mode, so we just MASK the PIC fully via its data
// ports (plain mask registers — safe) and drive ticks from the
// Local APIC timer (interrupts::init_apic_timer). A masked,
// un-remapped PIC delivers no IRQs, so the default 0x08-0x0F vector
// collision with CPU exceptions never happens.
```

## L351 · `core::arch::asm!("sti");`

```
// SAFETY: IDT loaded, PIC fully masked, handlers set.
```

## L363 · `#[unsafe(no_mangle)]`

```
// === Exception Handlers ===
```

## L365-366 · `#[unsafe(no_mangle)]`

```
// Reached from `forge_de_stub` when the fault did not come from compiled
// wasm. Named for the assembler, which cannot see through Rust's mangling.
```

## L376-378 · `pub static NMI_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// NMIs the kernel does not raise itself. They reach the host when a guest is
/// interrupted (SVM intercepts NMI) or from firmware. Counted, not printed —
/// an NMI may land while the console lock is held.
```

## L419 · `unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2); }`

```
// SAFETY: Reading CR2 is side-effect-free
```

## L429-433 · `let rsp = frame.stack_pointer;`

```
// Best-effort backtrace: scan the stack for words that look like return
// addresses into kernel code (link base 0x1000_0000 .. ~+8 MB). On this HW
// the PIE kernel runs at its link base, so these map directly via
// `addr2line -e target/.../nopeekos-kernel <addr>`. Only scanned when RSP
// is inside the identity-mapped range so the scan itself can't fault.
```

## L441-442 · `let v = unsafe { core::ptr::read_volatile(p as *const u64) };`

```
// SAFETY: rsp is within the identity-mapped range (checked above),
// so this read cannot page-fault (re-entry would triple-fault).
```

## L455 · `extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {`

```
// === IRQ Handlers ===
```

## L459 · `crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_TIMER);`

```
// Wake attribution: IRQ0 fires on the BSP (Core 0).
```

## L461 · `crate::xhci::poll_events_irq();`

```
// Drain USB events from interrupt context (try_lock, never blocks)
```

## L463-465 · `crate::keyboard::poll_ps2_irq();`

```
// Drain the polled PS/2 mouse/keyboard here too (no-op unless a PS/2
// mouse is up) — same model as the USB drain → smooth cursor regardless
// of the run loop's HLT/spin.
```

## L467-476 · `if tick % 100 == 0 {`

```
// NO busy-TSC fabrication here.
//
// This used to add `freq / 100` — one whole tick of cycles — to Core 0
// on every tick, with the note "Core 0 runs event loop". That is the
// wall clock itself, so Core 0's usage was pinned at 99-100 % by
// construction and said nothing about the machine. The shell loop has
// executed `hlt` when idle for a long time; the accounting never
// followed. Usage now comes from `record_halt` at the HLT sites, like
// every other core (`update_core_freq`).
// Update BSP frequency once per second (for top display)
```

## L484 · `crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_KEYBOARD);`

```
// Wake attribution: IRQ1 fires on the BSP (Core 0).
```

## L490-491 · `extern "x86-interrupt" fn apic_timer_handler(_frame: InterruptStackFrame) {`

```
/// APIC timer handler — fires on hardware without PIT (NUC, UEFI-only).
/// Same function as PIT timer: tick counter + USB event drain.
```

## L494 · `crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_TIMER);`

```
// Wake attribution: the APIC timer is armed only on the BSP (Core 0).
```

## L498-507 · `if tick % 100 == 0 {`

```
// NO busy-TSC fabrication here.
//
// This used to add `freq / 100` — one whole tick of cycles — to Core 0
// on every tick, with the note "Core 0 runs event loop". That is the
// wall clock itself, so Core 0's usage was pinned at 99-100 % by
// construction and said nothing about the machine. The shell loop has
// executed `hlt` when idle for a long time; the accounting never
// followed. Usage now comes from `record_halt` at the HLT sites, like
// every other core (`update_core_freq`).
// Update BSP frequency once per second (for top display)
```

## L511 · `let apic_base = APIC_BASE.load(Ordering::Relaxed);`

```
// APIC EOI: write 0 to End-of-Interrupt register
```

## L518 · `static APIC_BASE: AtomicU64 = AtomicU64::new(0);`

```
/// APIC base address (from MSR 0x1B).
```

## L522 · `pub fn apic_base() -> u64 { APIC_BASE.load(Ordering::Relaxed) }`

```
/// Get cached APIC base (for per-core identification via LAPIC ID register).
```

## L525-526 · `pub fn apic_base_any() -> u64 {`

```
/// The xAPIC base, read from MSR 0x1B if no timer path cached it yet.
/// Physical address is the same on every core.
```

## L531 · `unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi) };`

```
// SAFETY: MSR 0x1B (APIC base) is always readable in ring 0.
```

## L536-555 · `const WORKER_TIMER_VECTOR: u8 = 50;`

```
// ── Per-core worker timer: one-shot to the next deadline ────────────
//
// Stage 1 of `docs/plan/CORES_AND_EVENTS.md`. A worker used to arm a
// PERIODIC 100 Hz LAPIC timer and wake on every tick to look for work: 100
// wakes a second per idle core, and every sleep under 10 ms needed the
// period shortened (`arm_worker_wake_in`). Now the timer is one-shot —
// armed to the exact TSC deadline the core waits for, or not at all — and
// new work wakes an idle core with an IPI (`WORKER_WAKE_VECTOR`), not a tick.
//
// TSC-deadline mode (CPUID.1:ECX[24]) when the CPU has it: the LAPIC
// compares against the TSC itself, no conversion. Otherwise one-shot count
// mode, converted with the LAPIC rate calibrated against the TSC.
//
// A core running a guest vCPU uses the same one-shot: before every VM entry
// it is armed to the guest's next timer deadline (`arm_vcpu_timer`) — KVM's
// hrtimer on the vCPU's core — so the guest's clock events land on time and
// not on a host tick.
//
// Both vectors are pure EOI: the interrupt returning the core from HLT IS
// the effect.
```

## L557 · `pub const WORKER_WAKE_VECTOR: u8 = 52;`

```
/// IPI to an idle worker: new work is in the inbox.
```

## L560 · `static WORKER_TIMER_INITIAL: AtomicU32 = AtomicU32::new(0);`

```
/// LAPIC counts per 10 ms at divide 16 — for one-shot count mode.
```

## L566-568 · `static DEEP_IDLE_PORT: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);`

```
/// Does this CPU have the LAPIC TSC-deadline mode (CPUID.1:ECX[24])?
/// Deep idle via an ACPI SystemIO C-state port; 0 = off (plain `hlt`, C1).
/// Set by `set_deep_idle` (today: the `power cstate` experiment).
```

## L570-572 · `static DEEP_IDLE_MIN_TSC: AtomicU64 = AtomicU64::new(u64::MAX);`

```
/// A halt shorter than this stays in C1 — the exit latency of a deep state
/// would cost more than it saves (Linux's menu governor makes the same cut
/// against the state's target residency).
```

## L574 · `pub static DEEP_IDLE_COUNT: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];`

```
/// Deep entries per core (how often the deep path was actually taken).
```

## L577-580 · `pub fn set_deep_idle(port: u16, min_us: u64) -> Result<(), &'static str> {`

```
/// Turn deep idle on (`port` != 0) or off (0). Refuses anything outside the
/// CPU's own C-state trap range and a CPU whose LAPIC timer would stop in a
/// deep state (no ARAT) — there a core sleeping on its deadline timer would
/// never wake.
```

## L601-602 · `pub fn has_arat() -> bool {`

```
/// CPUID.06H:EAX[2] — Always Running APIC Timer (keeps counting in deep
/// C-states).
```

## L605-606 · `unsafe {`

```
// SAFETY: CPUID leaf 6 exists on every CPU this kernel runs on; rbx is
// reserved by LLVM.
```

## L616 · `unsafe {`

```
// SAFETY: CPUID leaf 1 exists on every x86_64; rbx is reserved by LLVM.
```

## L623 · `static CORE0_TICKLESS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);`

```
/// Core 0 runs on the one-shot timer too (stage 3e, `make_core0_tickless`).
```

## L626-632 · `pub fn make_core0_tickless() {`

```
/// Give Core 0 the worker's one-shot timer and stop its periodic 100 Hz
/// tick (the LVT is rewritten from periodic vector 48 to one-shot vector
/// 50). From here Core 0 wakes only when something is due or something
/// happens — and the tick's work is done elsewhere: the wall clock is the
/// TSC (0.411), input comes by interrupt (0.416) or is drained by the shell
/// when no interrupt exists, and the frequency statistics run in
/// `core0_loop`.
```

## L640-641 · `static ARMED: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];`

```
/// TSC value the core's one-shot is armed to; 0 = not armed (fired or
/// disarmed). Lets a vCPU skip re-arming the same deadline on every entry.
```

## L643 · `static POLL_PERIOD: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];`

```
/// Wake period of `worker_idle_hlt` in TSC cycles; 0 = 10 ms.
```

## L646-647 · `extern "x86-interrupt" fn worker_wake_handler(_frame: InterruptStackFrame) {`

```
/// Wake IPI for an idle worker — pure EOI; the interrupt ending the HLT is
/// the effect.
```

## L654 · `unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }`

```
// SAFETY: LAPIC MMIO, identity-mapped; each core EOIs its own LAPIC.
```

## L665-666 · `if CORE0_TICKLESS.load(Ordering::Relaxed) && crate::smp::per_core::current_core_id() == 0 {`

```
// Core 0 attributes its own wakes (halt_until skips it): a timer or a
// wake IPI on this vector counts as `timer` in `cores`.
```

## L672 · `unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }`

```
// SAFETY: LAPIC MMIO, identity-mapped; each core EOIs its own LAPIC.
```

## L677-683 · `pub const VCPU_KICK_VECTOR: u8 = 51;`

```
/// Cross-vCPU kick IPI (guest SMP). When a guest vCPU sends an inter-processor
/// interrupt to another vCPU (reschedule / call-function / TLB-shootdown), the
/// sender's host core fires this vector at the TARGET vCPU's host core, forcing
/// its VMRUN to #VMEXIT(INTR) so it injects the guest IPI within microseconds
/// instead of waiting up to a host-timer tick (~10 ms). That ~10 ms latency is
/// what made `smp_call_function`'s `csd_lock_wait` spin burn ~50% of guest CPU
/// across all vCPUs (measured via rip_sample). Pure EOI — receipt IS the effect.
```

## L689 · `unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }`

```
// SAFETY: LAPIC MMIO, identity-mapped; each core EOIs its own LAPIC.
```

## L694-696 · `pub fn init_worker_timer() {`

```
/// Set up the calling worker's LAPIC timer: one-shot (or TSC-deadline)
/// mode on `WORKER_TIMER_VECTOR`, nothing armed. Each AP calls it once at
/// boot.
```

## L699 · `unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }`

```
// SAFETY: MSR 0x1B (APIC base) is always readable on x86_64 ring 0.
```

## L703-704 · `let _ = APIC_BASE.compare_exchange(0, base, Ordering::Relaxed, Ordering::Relaxed);`

```
// Also publish the (core-invariant) xAPIC base for the device-IRQ ISR EOI
// path, in case Core 0 runs on the PIT and never set APIC_BASE itself.
```

## L710 · `unsafe {`

```
// SAFETY: LAPIC MMIO regs, identity-mapped; this core's own LAPIC.
```

## L715 · `core::ptr::write_volatile(b.add(0x3E0) as *mut u32, 0x03);`

```
// Divide = 16.
```

## L718-720 · `core::ptr::write_volatile(b.add(0x320) as *mut u32, (1 << 16) | WORKER_TIMER_VECTOR as u32);`

```
// Calibrate counts/10ms against the TSC, once — the LAPIC timer
// clock is the same on every core. Periodic mode for the
// measurement only; the counter just has to run down.
```

## L732-733 · `core::ptr::write_volatile(b.add(0x380) as *mut u32, 0);`

```
// Stop whatever ran before, then set the mode:
// TSC-deadline = LVT bits 18:17 = 10b, one-shot = 00b.
```

## L739 · `unsafe { wrmsr_deadline(0) };`

```
// SAFETY: TSC-deadline mode is supported (checked above); 0 disarms.
```

## L748 · `unsafe fn wrmsr_deadline(tsc: u64) {`

```
/// SAFETY: only with TSC-deadline mode supported and selected in the LVT.
```

## L750-751 · `unsafe {`

```
// SAFETY: the SDM asks for the LVT write to be ordered before the MSR
// write; Linux fences the same way (`weak_wrmsr_fence`).
```

## L760-761 · `fn arm_at(deadline: u64) {`

```
/// Arm this core's worker timer to fire at TSC `deadline`. A deadline in
/// the past fires at once.
```

## L768 · `unsafe { wrmsr_deadline(deadline.max(1)) };`

```
// SAFETY: mode selected by `init_worker_timer`; never 0 (= disarm).
```

## L779-780 · `unsafe { core::ptr::write_volatile((base + 0x380) as *mut u32, count) };`

```
// SAFETY: this core's own LAPIC initial-count register; a write starts
// the one-shot countdown.
```

## L790 · `unsafe { wrmsr_deadline(0) };`

```
// SAFETY: mode selected by `init_worker_timer`; 0 disarms.
```

## L796 · `unsafe { core::ptr::write_volatile((base + 0x380) as *mut u32, 0) };`

```
// SAFETY: this core's own LAPIC initial-count register; 0 stops it.
```

## L801-804 · `pub fn arm_vcpu_timer(deadline: u64) {`

```
/// Arm this core's one-shot to `deadline` before a VM entry, unless it is
/// armed there already. The fire is a host interrupt → #VMEXIT → the run
/// loop delivers the guest's due timer (KVM: the APIC/PIT hrtimer kicks the
/// vCPU). Any other exit, a park or a fire resets it; the loop re-arms.
```

## L813-823 · `pub fn halt_until(deadline: Option<u64>, cause: usize) {`

```
/// THE idle primitive of a worker: halt until an interrupt, and — if
/// `deadline` is given — no later than that TSC value. IF is restored as
/// found. `cause` is the wake-attribution slot for `cores`.
///
/// Arming and halting happen with IF=0 and the halt is entered as
/// `sti; hlt`: an interrupt landing between the arm and the halt stays
/// pending and ends the halt, instead of being consumed before it and
/// leaving the core asleep with nothing armed.
///
/// On Core 0 before it went tickless the timer is not touched; the halt
/// ends at the next tick there.
```

## L829 · `unsafe { core::arch::asm!("pushfq; pop {}; cli", out(reg) rflags) };`

```
// SAFETY: save RFLAGS and clear IF; restored below exactly as found.
```

## L834 · `unsafe { core::arch::asm!("sti") };`

```
// SAFETY: IF was set on entry.
```

## L849-857 · `unsafe {`

```
// ACPI SystemIO C-state (Linux `io_idle`): a read of the port the
// core traps as its C-state entry request. Entered with IF=0, like
// Linux's cpuidle: a pending interrupt is a break event regardless
// of IF, so there is no window between arming and sleeping. The
// `sti; nop; cli` afterwards takes that interrupt here, as the hlt
// path does. No dummy PM-timer read: Linux restricts it to old Intel.
// SAFETY: `port` was accepted by `set_deep_idle` only inside the
// CPU's own C-state trap range (CStateBaseAddr..+8); reading it has
// no effect but the idle entry.
```

## L865 · `unsafe { core::arch::asm!("sti; hlt; cli") };`

```
// SAFETY: sti-shadow arms the HLT before any pending IRQ is taken.
```

## L874 · `if cid != 0 {`

```
// Core 0's wakes are attributed by its ISRs (timer, input).
```

## L878 · `if own_timer && deadline.is_some() { disarm(); }`

```
// Woken by something else: the one-shot is still pending — drop it.
```

## L881 · `unsafe { core::arch::asm!("sti") };`

```
// SAFETY: IF was set on entry.
```

## L886-888 · `pub fn set_worker_poll_hz(hz: u32) {`

```
/// How often `worker_idle_hlt` wakes on the calling worker: `hz` per second.
/// A native network loop (download) raises it to poll its NIC ring without a
/// device IRQ; pass 100 to go back to 10 ms. Goes away with NAPI (stage 2).
```

## L897-900 · `pub fn worker_idle_hlt() {`

```
/// Idle step of a worker-core wait loop that polls (the native network
/// receive loops: `tcp_recv_poll`, `recv_blocking`): halt for one poll
/// period — 10 ms, or what `set_worker_poll_hz` asked for — or until an
/// interrupt. Spin on Core 0, which has its own idle path.
```

## L912-920 · `pub const PS2_VECTOR: u8 = 53;`

```
// ── Input interrupts (stage 3b) ─────────────────────────────────────
//
// Keyboard and mouse used to be drained ONLY from the Core-0 timer tick —
// up to 10 ms late, and never while the tick is gone (stage 3e). Now the
// i8042 raises ISA IRQ 1 (and 12 for the aux port) through the I/O APIC,
// and each xHCI controller raises MSI-X; both land on Core 0 and drain
// right away. The tick keeps draining as a fallback until 3e: both paths
// run in interrupt context on Core 0, so they never interleave, and the
// xHCI path takes its controller lock with `try_lock`.
```

## L922 · `pub const PS2_VECTOR: u8 = 53;`

```
/// i8042 (keyboard IRQ 1, aux IRQ 12), routed by `keyboard::enable_irq`.
```

## L924 · `pub const XHCI_VECTOR: u8 = 54;`

```
/// Every xHCI controller's interrupter 0, programmed by `xhci::init`.
```

## L928 · `unsafe { core::ptr::write_volatile((apic_base_any() + 0xB0) as *mut u32, 0) };`

```
// SAFETY: LAPIC EOI register (xAPIC base + 0xB0), identity-mapped.
```

## L946-951 · `pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {`

```
/// Run `f` with this core's interrupts masked, restoring the previous IF.
///
/// For a spin lock that an ISR on the SAME core may also take: held with
/// IF=1, the timer IRQ lands inside the critical section and spins on the
/// lock its own core holds — the core is gone. Cheap (pushfq/cli/popfq), so
/// keep `f` short; it cannot be interrupted.
```

## L954 · `unsafe { core::arch::asm!("pushfq; pop {}; cli", out(reg) rflags) };`

```
// SAFETY: save RFLAGS and clear IF; restored below exactly as found.
```

## L958 · `unsafe { core::arch::asm!("sti") };`

```
// SAFETY: IF was set on entry — set it again.
```

## L964-970 · `pub const DEVICE_IRQ_VEC_BASE: u8 = 0x70;`

```
// ── Device interrupts (MSI-X → LAPIC vector → fiber wake) ───────────
//
// Real-hardware device IRQs route via MSI-X to a vector in this pool (the
// PIC is masked, no IOAPIC). Each vector has a tiny ISR that bumps a
// per-vector fired-count (`irq::note_fired`) + EOIs the LAPIC; a driver
// fiber parks on it via `irq::wait`. See `crate::irq`. Vectors 0x70..0x80
// sit above the timer vectors (32/48/49/50) and the masked PIC range.
```

## L974-977 · `#[inline]`

```
/// Shared body of every device-IRQ ISR: note the fire (wakes the parked
/// driver fiber via its count) + EOI this core's LAPIC. Minimal by design —
/// no fiber-state touch in interrupt context (the scheduler owns that; the
/// atomic bump is race-free across cores).
```

## L981-983 · `let base = APIC_BASE.load(Ordering::Relaxed);`

```
// LAPIC EOI: write 0 to offset 0xB0. The xAPIC base is the same physical
// address on every core; this write hits the LAPIC of the core that took
// the interrupt (the MSI-X destination = the driver fiber's core).
```

## L986 · `unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }`

```
// SAFETY: LAPIC MMIO EOI register, identity-mapped.
```

## L991-993 · `macro_rules! device_isrs {`

```
// Generate one `extern "x86-interrupt"` stub per pool vector (each hardcodes
// its vector, since the CPU passes none) + an installer that wires them into
// the IDT. Macro keeps the 16 stubs + 16 IDT writes in lockstep.
```

## L1001-1003 · `unsafe fn install_device_isrs() {`

```
/// Install the device-IRQ ISRs into the (global) IDT. Called from
/// `init()` on the BSP before APs boot, so every core that loads this
/// IDT sees them.
```

## L1005-1006 · `unsafe {`

```
// SAFETY: writing IDT entries before `sti`/AP-bringup; vectors are
// in the dedicated device pool, distinct from timer/exception ones.
```

## L1020-1022 · `pub fn current_apic_id() -> u32 {`

```
/// Hardware APIC ID of the calling core (xAPIC ID register, bits 31:24).
/// Used to set an MSI-X message's destination so the IRQ wakes this core.
/// Reads MSR 0x1B directly if `APIC_BASE` isn't cached yet.
```

## L1030 · `unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }`

```
// SAFETY: MSR 0x1B (APIC base) is always readable in ring 0.
```

## L1035 · `unsafe { core::ptr::read_volatile((base + 0x20) as *const u32) >> 24 }`

```
// SAFETY: LAPIC ID register at MMIO offset 0x20, identity-mapped.
```

## L1039-1040 · `pub fn init_apic_timer() {`

```
/// Initialize Local APIC timer (for hardware without PIT).
/// Call after init() — detects if PIT is working, sets up APIC timer if not.
```

## L1042 · `let t0 = TICKS.load(Ordering::Relaxed);`

```
// Check if PIT timer is already working (TICKS > 0 after a short delay)
```

## L1044 · `let freq = TSC_FREQ.load(Ordering::Relaxed);`

```
// Wait ~50ms using TSC
```

## L1046 · `let wait_cycles = freq / 20; // 50ms`

```
// 50ms
```

## L1052 · `return; // PIT works — no APIC timer needed`

```
// PIT works — no APIC timer needed
```

## L1055 · `let (lo, hi): (u32, u32);`

```
// Read APIC base from MSR 0x1B
```

## L1061 · `let _ = crate::paging::map_page(apic_base, apic_base,`

```
// Map APIC page (identity-mapped, uncacheable)
```

## L1068 · `IDT[APIC_TIMER_VECTOR as usize].set_handler(apic_timer_handler as *const () as u64);`

```
// Install APIC timer handler in IDT
```

## L1071-1072 · `let svr = core::ptr::read_volatile(base.add(0xF0) as *const u32);`

```
// Enable APIC: set Spurious Interrupt Vector Register (offset 0xF0)
// Bit 8 = APIC enable, bits 0-7 = spurious vector (use 0xFF)
```

## L1076 · `core::ptr::write_volatile(base.add(0x3E0) as *mut u32, 0x03);`

```
// Set timer divide = 16 (offset 0x3E0, value 0x03)
```

## L1079 · `core::ptr::write_volatile(base.add(0x380) as *mut u32, 0xFFFF_FFFF); // max initial count`

```
// Calibrate: measure APIC ticks per 10ms using TSC
```

## L1080 · `core::ptr::write_volatile(base.add(0x380) as *mut u32, 0xFFFF_FFFF); // max initial count`

```
// max initial count
```

## L1086-1087 · `core::ptr::write_volatile(base.add(0x320) as *mut u32,`

```
// Set timer: periodic mode, vector APIC_TIMER_VECTOR
// LVT Timer Register (offset 0x320): bit 17 = periodic, bits 0-7 = vector
```

## L1091 · `core::ptr::write_volatile(base.add(0x380) as *mut u32, elapsed);`

```
// Set initial count (ticks per 10ms = 100Hz)
```

