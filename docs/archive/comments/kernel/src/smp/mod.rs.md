# `kernel/src/smp/mod.rs` @ 5e0102684

## L1-7 · `core::arch::global_asm!(include_str!("trampoline.s"), options(att_syntax));`

```
//! Symmetric Multiprocessing (SMP)
//!
//! Discovers CPU cores via ACPI MADT, boots Application Processors
//! using INIT-SIPI-SIPI, and manages per-core state.
//!
//! Design: Core 0 = Kernel/IRQ (fixed), Cores 1..N = Worker Pool.
//! No hardcoded core limit — scales from 2 to 1024+.
```

## L20-29 · `const AP_STACK_SIZE: usize = 2 * 1024 * 1024;`

```
// 2 MB per AP — matches the BSP boot stack (linker.ld). Native intents
// dispatched from the GUI run on a worker core's AP stack via
// `scheduler::spawn` (NOT a fiber), and an `update` drives the full OTA chain
// there: https_get_once alone keeps a 17 KB record buffer live for the whole
// body stream, on top of a deep TLS → StreamingWriter → storage::put →
// btree::insert chain whose split path nests several 4 KB node buffers plus
// BLAKE3. The first B-tree split (~16 MiB into a large asset, once the tree
// fills) tipped the old 64 KB stack over → silent overflow smashed return
// addresses → wild RIP page fault. There's no guard page, so the size is the
// only safety margin — the BSP learned this exact lesson (256 KB → 2 MB).
```

## L32-34 · `const OFF_GDT64: usize = 0xF0;`

```
// Data area offsets within trampoline (must match trampoline.s).
// All shifted up by 0x10 in v0.85.5 to give the AVX bring-up
// (XSETBV) room before the GDT block.
```

## L43 · `static AP_STARTED: AtomicUsize = AtomicUsize::new(0);`

```
/// Counter incremented by each AP when it reaches Rust entry
```

## L51 · `pub fn init() {`

```
/// Initialize SMP: discover cores via MADT, boot all APs.
```

## L59 · `let _ = crate::paging::map_page(`

```
// Ensure APIC MMIO page is accessible
```

## L72 · `if per_core::enable_hwp() {`

```
// Enable HWP (hardware frequency scaling) on BSP
```

## L79 · `let ap_ids = parse_madt(bsp_id);`

```
// Discover APs from ACPI MADT
```

## L89 · `setup_trampoline(apic_base);`

```
// Prepare trampoline at 0x8000
```

## L92 · `let mut online = 0u32;`

```
// Boot each AP sequentially
```

## L108 · `scheduler::init(online as usize);`

```
// Initialize scheduler and wake APs into their work loops
```

## L110-111 · `per_core::init_dedicated_vm_core(online as usize);`

```
// Decide the dedicated microvm core before APs enter their
// loops (carve-out observed on first iteration).
```

## L122-124 · `fn enable_deep_idle() {`

```
/// Deep idle by default where the CPU offers it (AMD Zen on bare metal,
/// with ARAT): CStateBaseAddr+2. The deepest port belongs to ACPI `_CST`
/// (Linux `acpi_idle`); until aml passes it through, +2 is the usual C3.
```

## L134-138 · `fn log_tsc_sync(online: usize) {`

```
/// Measure every AP's TSC against core 0 (wake round trip, ±rt/2) once the
/// APs are parked. Firmware may leave core 0 behind; since the kernel
/// compares TSC stamps across cores, a uniform offset is corrected once by
/// writing IA32_TSC on core 0. Linux instead marks the TSC unstable without
/// TSC_ADJUST (AMD) — not an option for a tickless kernel built on the TSC.
```

## L141 · `let settle = crate::interrupts::tsc_freq() / 20; // 50 ms for the APs to park`

```
// 50 ms for the APs to park
```

## L154-156 · `let (off, rt) = best;`

```
// Correct only a real, uniform offset: the APs must agree with each
// other (else there is no single clock to join), and the offset must be
// well above the measurement's own error.
```

## L165-166 · `let _ = rt;`

```
// The offset was taken against the midpoint of the round trip, so it
// carries ±rt/2; the smallest round trip is the best estimate.
```

## L170-172 · `unsafe {`

```
// SAFETY: IA32_TSC (MSR 0x10) is architectural on x86_64 and
// writable at CPL 0; it moves only this core's (core 0's) counter.
// Interrupts are off, so nothing on this core sees the jump halfway.
```

## L184-185 · `fn measure_offsets(online: usize) -> (i64, i64, usize, (i64, u64)) {`

```
/// (min, max, answered, (offset, round trip) with the smallest round trip),
/// all in TSC cycles, of every AP against core 0.
```

## L206 · `fn read_apic_base() -> u64 {`

```
/// Read Local APIC base from MSR 0x1B
```

## L209 · `unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }`

```
// SAFETY: MSR 0x1B is the APIC base, always readable on x86_64
```

## L214 · `fn read_apic_id(apic_base: u64) -> u32 {`

```
/// Read this core's APIC ID from the Local APIC register
```

## L216 · `let raw = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) };`

```
// SAFETY: APIC page is mapped, register at offset 0x20 is read-only
```

## L221 · `fn setup_trampoline(_apic_base: u64) {`

```
/// Copy trampoline to 0x8000 and fill in shared data (CR3, GDT, IDT, entry)
```

## L228 · `core::ptr::copy_nonoverlapping(start, TRAMPOLINE_BASE as *mut u8, size);`

```
// SAFETY: 0x8000 is in first 1MB (reserved, identity-mapped, not used by kernel)
```

## L231-234 · `let cr3: u64;`

```
// CR3 — BSP's page table root
// Trampoline loads CR3 from 32-bit protected mode (only 32-bit mov available).
// PML4 is in kernel BSS (~7MB) — well below 4GB. Assert guards against future
// changes where frame allocator might place page tables in high RAM.
```

## L240-242 · `let mut gdtr = [0u8; 10];`

```
// GDT64 pointer — copy BSP's GDTR (SGDT stores 10 bytes in 64-bit mode)
// Trampoline uses lgdt in 32-bit mode (reads 2+4 bytes). GDT base is in kernel
// .rodata (~1MB), fits in 32 bits. Same assert for safety.
```

## L253 · `let mut idtr = [0u8; 10];`

```
// IDTR — copy BSP's IDT register (SIDT stores 10 bytes)
```

## L262 · `per_core::smp_ap_entry as *const () as u64;`

```
// Rust AP entry point
```

## L268 · `fn boot_ap(apic_base: u64, target_apic_id: u32, core_id: u32) -> bool {`

```
/// Boot a single AP: allocate stack, write per-AP data, send INIT-SIPI-SIPI
```

## L275-277 · `unsafe {`

```
// Write per-AP fields to trampoline data area
// SAFETY: trampoline at 0x8000 is set up, no AP is using it yet
//         (we boot APs sequentially)
```

## L283 · `core::sync::atomic::fence(Ordering::SeqCst);`

```
// Ensure all writes are visible before SIPI
```

## L288 · `let vector = (TRAMPOLINE_BASE / 0x1000) as u32; // SIPI vector = page number`

```
// SIPI vector = page number
```

## L290 · `send_ipi(apic_base, target_apic_id, 0x0000_4500);`

```
// INIT IPI
```

## L294 · `send_ipi(apic_base, target_apic_id, 0x0000_4600 | vector);`

```
// SIPI #1
```

## L298 · `if AP_STARTED.load(Ordering::Acquire) > started_before {`

```
// Check if AP started
```

## L303 · `send_ipi(apic_base, target_apic_id, 0x0000_4600 | vector);`

```
// SIPI #2 (spec says retry once)
```

## L306 · `let timeout = crate::interrupts::tsc_freq() / 10;`

```
// Wait up to 100ms
```

## L319-324 · `pub fn kick_host_core(core_id: usize) {`

```
/// Force the host core with sequential id `core_id` out of VMRUN by sending it
/// the vCPU-kick IPI from the CALLING core's LAPIC. Used by guest-SMP IPI
/// delivery so a target vCPU takes a cross-vCPU interrupt within microseconds
/// (its VMRUN #VMEXIT(INTR)s on receipt) instead of at its next natural exit
/// (~10 ms host-timer tick). No-op if the core id is unknown. The kick vector's
/// host ISR is a pure EOI — the receipt itself is the wakeup.
```

## L326-328 · `crate::smp::fiber::net_kick_bump(core_id);`

```
// Event-wake a consumer fiber parked in `kick_wait` on the target core
// BEFORE the IPI, so the wake is never lost. Harmless for non-net kicks
// (only `kick_wait` fibers observe the generation).
```

## L337-338 · `let base = {`

```
// xAPIC base (core-invariant physical address). SAFETY: MSR 0x1B always
// readable in ring 0; we mask to the 4 KiB-aligned base.
```

## L347 · `send_ipi(base, apic_id, 0x0000_4000 | crate::interrupts::VCPU_KICK_VECTOR as u32);`

```
// FIXED delivery (mode 000), level assert (bit 14), physical dest.
```

## L351 · `pub fn send_wake_ipi(apic_id: u32) {`

```
/// Send the worker wake IPI to the core with xAPIC id `apic_id`.
```

## L354-355 · `crate::interrupts::without_interrupts(|| {`

```
// FIXED delivery, level assert, physical destination. IF masked so an
// interrupt cannot land between the ICR-high and ICR-low writes.
```

## L361 · `fn send_ipi(apic_base: u64, target_apic_id: u32, icr_low: u32) {`

```
/// Send IPI via Local APIC ICR (wait for idle first)
```

## L363 · `unsafe {`

```
// SAFETY: APIC MMIO is mapped. ICR write triggers IPI.
```

## L365 · `while core::ptr::read_volatile((apic_base + 0x300) as *const u32) & (1 << 12) != 0 {`

```
// Wait for delivery status = idle
```

## L369 · `core::ptr::write_volatile((apic_base + 0x310) as *mut u32, target_apic_id << 24);`

```
// Destination APIC ID (bits 24-31 of ICR high)
```

## L371 · `core::ptr::write_volatile((apic_base + 0x300) as *mut u32, icr_low);`

```
// Command (writing ICR low triggers the IPI)
```

## L376 · `fn allocate_ap_stack() -> u64 {`

```
/// Allocate the per-AP stack (`AP_STACK_SIZE`). Returns stack top (grows down).
```

## L385 · `fn parse_madt(bsp_apic_id: u32) -> Vec<u32> {`

```
// ── ACPI MADT Parsing ──────────────────────────────────────────
```

## L387 · `fn parse_madt(bsp_apic_id: u32) -> Vec<u32> {`

```
/// Parse MADT (signature "APIC") and return all AP APIC IDs
```

## L399 · `let madt_len = unsafe { *((madt_addr + 4) as *const u32) } as usize;`

```
// SAFETY: MADT is in identity-mapped memory. We validate bounds before reads.
```

## L406 · `let mut offset = 44;`

```
// Walk Interrupt Controller Structures (start at offset 44)
```

## L414 · `0 if entry_len >= 8 => {`

```
// Type 0: Processor Local APIC (8-bit APIC ID)
```

## L418 · `if (flags & 0x03) != 0 && apic_id != bsp_apic_id {`

```
// bit 0 = Enabled, bit 1 = Online Capable
```

## L423 · `9 if entry_len >= 16 => {`

```
// Type 9: Processor Local x2APIC (32-bit APIC ID, for >255 cores)
```

