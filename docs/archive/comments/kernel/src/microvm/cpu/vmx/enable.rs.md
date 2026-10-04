# `kernel/src/microvm/cpu/vmx/enable.rs` @ 5e0102684

## L1-19 · `use super::{ept, rdmsr, vmcs, wrmsr};`

```
//! VMX root-mode entry/exit + VMCS round-trip + VMLAUNCH — 12.1.0b…12.1.1c-3b3b2.
//!
//! Two consumer-facing entry points:
//!   - `enable_and_test()` — real-mode/32-bit-prot substrate test
//!     (9-byte stub `mov al,'O'; out 0x80,al; mov al,'K'; out 0x80,al; hlt`)
//!     used by `microvm test`.
//!   - `run_linux(bzimage, cmdline)` — Linux Boot Protocol 32-bit
//!     entry, used by `microvm linux`.
//!
//! Both share the VMXON region setup, VMCS allocation, VMCLEAR /
//! VMPTRLD, VMXOFF tear-down via `with_vmx_root_and_vmcs`. Guest
//! state, EPT, I/O bitmap, run-loop are caller-supplied.
//!
//! VMXON + VMCS regions are allocated and *kept* (never freed) per
//! call. CR4.VMXE is left set across calls (harmless).
//!
//! Reference: Intel SDM Vol. 3C §23.7 (Enabling VMX), §24.11.3
//! (Initializing a VMCS), §26.2-§26.4 (Host/Guest State), §27
//! (VM Exits).
```

## L26-28 · `use crate::microvm::cpu::svm::lapic::{self, LocalApic};`

```
// LAPIC emulation is pure register state (only uses rdtsc) — reuse the SVM
// module's `LocalApic` rather than duplicate it. Intel parity #2. `IcrWrite`
// + `ICR_DM_*` are also reused by the cross-vCPU IPI router (guest SMP #4).
```

## L31-34 · `const VEC_UD: u8 = 6;`

```
/// True when the guest LAPIC is emulated on this (VMX ⇒ Intel) host. Cheap
/// const check (no vendor lock — vmx code only runs on Intel). Gates every
/// LAPIC-related VMX path; false ⇒ the validated `nolapic` boot, byte-
/// identical. See `cpu::VMX_GUEST_LAPIC`.
```

## L43-55 · `static VM_BIG_LOCK: spin::Mutex<()> = spin::Mutex::new(());`

```
// ── Guest-SMP (VMX #4) — mirror of svm/enable.rs ───────────────────────
//
// The Intel twin of the AMD guest-SMP machinery. Same architecture: split
// `VmContext` into a shared `VmShared` (one guest address space + device
// model, behind a `SharedRef` so an AP vCPU aliases the BSP's heap box) and a
// per-vCPU `Vcpu` (its own VMXON/VMCS/regs/FPU/LAPIC/apic_id). The BSP and AP
// run their VMRESUME loops as pinned pool fibers on separate worker cores, in
// parallel, serialising only their post-exit device handling under
// `VM_BIG_LOCK`. The validated AMD stumbling blocks are pre-empted here:
// per-CORE VMXON/VMCS (each vCPU enters VMX root on its own core), the AP
// never locks VENDOR (it learns its vendor lock-free via `detect_vendor`),
// IPIs inject only when interruptible (reason-33 guard), idle-park is gated on
// AP_ESTABLISHED, and devices/timer/nat/input are BSP-only.
```

## L57-61 · `static VM_BIG_LOCK: spin::Mutex<()> = spin::Mutex::new(());`

```
/// Big-VM lock (guest SMP). Held by a vCPU only around its post-exit
/// device/MMIO/IO handling — NEVER across `run_guest_once` or a fiber yield —
/// so the BSP and an AP serialise all access to the shared `VmShared` while
/// their VMRESUMEs run truly in parallel on separate host cores. Uncontended
/// (never even taken) until an AP is admitted (`AP_ACTIVE`).
```

## L64-67 · `pub static AP_ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// True once a second vCPU (AP) shares this VM. While false the BSP runs
/// exactly as the single-vCPU path did — the lock is not taken, so the hot
/// loop is byte-identical. Set by the orchestration layer when it spawns an
/// AP fiber, cleared after the last vCPU exits.
```

## L76-83 · `const HALT_POLL_MIN_US: u64 = 5;   // floor — always poll a little, recoverable`

```
/// Bounded adaptive halt-polling (KVM `halt_poll_ns` model). When a vCPU goes
/// idle it polls — re-pumping + re-checking for a wake event (RX / IRQ / IPI) —
/// for up to its current window before truly parking. A wake within the window
/// grows the window (×2, latency pays off); an expiry shrinks it (÷2, the vCPU
/// was idle, save power). This replaces the old global `recently_active` spin:
/// idle vCPUs shrink toward the floor (~park), a latency-sensitive vCPU (the
/// network BSP between RX bursts) grows toward the cap, and a busy vCPU never
/// reaches the idle path at all. Per-vCPU, no global state. µs units.
```

## L84 · `const HALT_POLL_MIN_US: u64 = 5;   // floor — always poll a little, recoverable`

```
// floor — always poll a little, recoverable
```

## L85 · `const HALT_POLL_MAX_US: u64 = 200; // KVM default cap`

```
// KVM default cap
```

## L102 · `fn with_vmx_root_and_vmcs<F, T>(inner: F) -> Result<T, &'static str>`

```
// ── VMXON / VMCS plumbing (shared by all entry points) ─────────────
```

## L104-110 · `fn with_vmx_root_and_vmcs<F, T>(inner: F) -> Result<T, &'static str>`

```
/// Run `inner` inside VMX root mode with a fresh, current VMCS.
/// Handles all the VMXON-region / FEATURE_CONTROL / CR0+CR4 fixed-bit
/// dance once, allocates a 4-KB VMCS region, runs VMCLEAR + VMPTRLD,
/// then calls `inner` (which operates on the current VMCS via
/// VMREAD/VMWRITE / EPT / etc.). VMXOFF runs unconditionally on
/// return, even on inner error, so the CPU never strands in VMX
/// root mode.
```

## L115 · `let region_phys = memory::allocate_frame().ok_or("OOM allocating VMXON region")?;`

```
// 1. VMXON region.
```

## L120 · `unsafe {`

```
// SAFETY: identity-mapped, freshly-allocated, exclusive.
```

## L127 · `let feat = unsafe { rdmsr(IA32_FEATURE_CONTROL) };`

```
// 2. FEATURE_CONTROL.
```

## L131 · `unsafe { wrmsr(IA32_FEATURE_CONTROL, new); }`

```
// SAFETY: writing lock + outside-SMX bits to architectural MSR.
```

## L137 · `let cr0_f0 = unsafe { rdmsr(IA32_VMX_CR0_FIXED0) };`

```
// 3. CR0/CR4 fixed bits + CR4.VMXE.
```

## L145 · `unsafe {`

```
// SAFETY: CR reads cannot fault.
```

## L152 · `unsafe {`

```
// SAFETY: values satisfy fixed-bit constraints; VMXE is allowed.
```

## L158 · `let region_addr_slot: u64 = region_phys;`

```
// 4. VMXON.
```

## L161-162 · `unsafe {`

```
// SAFETY: VMXON requires CR4.VMXE (set above) + valid 4-KB
// region with revision-id (set above).
```

## L179 · `let inner_result = vmcs_setup_then_inner(revision_id, inner);`

```
// 5. VMCS region + VMCLEAR + VMPTRLD.
```

## L182-183 · `unsafe {`

```
// 6. VMXOFF — always runs.
// SAFETY: in VMX root mode (verified above).
```

## L197 · `unsafe {`

```
// SAFETY: identity-mapped, freshly-allocated, exclusive.
```

## L206 · `let rflags_clear: u64;`

```
// VMCLEAR.
```

## L208 · `unsafe {`

```
// SAFETY: in VMX root mode; valid VMCS region.
```

## L225 · `let rflags_load: u64;`

```
// VMPTRLD.
```

## L227 · `unsafe {`

```
// SAFETY: in VMX root mode; VMCS just successfully VMCLEAR'd.
```

## L247-250 · `fn write_host_state_with_current_rsp() -> Result<(), &'static str> {`

```
/// Sample current RSP and write it into HOST_RSP as a placeholder.
/// The real run-loop overrides HOST_RSP just-in-time before each
/// VMLAUNCH/VMRESUME — but the field must be canonical between
/// `setup_host_state` and the launch.
```

## L253 · `unsafe {`

```
// SAFETY: pure register read.
```

## L260-269 · `fn alloc_guest_ram_and_ept(guest_bytes: u64) -> Result<(u64, u64, u64, u64), &'static str> {`

```
/// Allocate a fresh 64 MB contiguous host-physical region for the
/// guest, install the EPT mapping it onto guest-phys [0, 64 MB),
/// return (host_base, eptp).
/// Allocate `guest_bytes` of contiguous guest RAM (+ 2-MB-align
/// slack) and install the EPT window over it. `close()` frees exactly
/// `ept::total_frames_for(guest_bytes)` from `raw_base`.
/// B3: allocate only the **contiguous boot window** (256 MiB or the
/// whole guest if smaller); `[boot, guest_bytes)` is demand-paged 4 KB
/// and needs no upfront allocation. Returns
/// `(boot_base, eptp, pml4_phys, boot_raw_base)`.
```

## L278 · `pub fn enable_and_test() -> Result<vmcs::LaunchOutcome, &'static str> {`

```
// ── Substrate test (12.1.1c-3b3a / 3b3b1) ──────────────────────────
```

## L280-282 · `pub fn enable_and_test() -> Result<vmcs::LaunchOutcome, &'static str> {`

```
/// Real-mode I/O-loop substrate test. Allocates fresh resources,
/// runs the 9-byte `out 0x80, 'O'; out 0x80, 'K'; hlt` stub,
/// returns the final VM-exit outcome. Used by `microvm test`.
```

## L285-287 · `let (host_base, eptp, _pml4, _raw_base) =`

```
// Substrate test runs a tiny real-mode stub at gpa 0x10000;
// size-insensitive → a 4 MiB all-contiguous boot window (no
// demand PTs). host_base is the boot block base.
```

## L291 · `let stub_host = host_base + 0x10000;`

```
// 9-byte substrate stub at guest-phys 0x10000.
```

## L293-294 · `unsafe {`

```
// SAFETY: host_base is 2-MB-aligned and the [host_base,
// host_base + 64 MB) window is exclusively ours.
```

## L298 · `page.add(0).write_volatile(0xB0); page.add(1).write_volatile(0x4F); // mov al, 'O'`

```
// mov al, 'O'
```

## L299 · `page.add(2).write_volatile(0xE6); page.add(3).write_volatile(0x80); // out 0x80, al`

```
// out 0x80, al
```

## L300 · `page.add(4).write_volatile(0xB0); page.add(5).write_volatile(0x4B); // mov al, 'K'`

```
// mov al, 'K'
```

## L301 · `page.add(6).write_volatile(0xE6); page.add(7).write_volatile(0x80); // out 0x80, al`

```
// out 0x80, al
```

## L302 · `page.add(8).write_volatile(0xF4);                                    // hlt`

```
// hlt
```

## L313-315 · `fn run_substrate_loop() -> Result<vmcs::LaunchOutcome, &'static str> {`

```
/// Loop dispatch for the substrate test: HLT terminates, OUT
/// captures the byte for the "OK" reconstruction, anything else
/// breaks with a log line.
```

## L328-330 · `let mut host_fpu = crate::microvm::cpu::FpuArea::boxed();`

```
// FPU areas required by run_guest_once's in-asm xsave/xrstor.
// The substrate stub doesn't touch FPU but the asm unconditionally
// brackets vmresume with host/guest FPU swap; pass valid areas.
```

## L342-344 · `last_outcome = Some(outcome);`

```
// External interrupt — host IRQ that arrived during
// guest run. The `sti` at the tail of run_guest_once
// already let the host IDT dispatch it; just resume.
```

## L397 · `pub enum SliceOutcome {`

```
// ── Linux launcher (12.1.1c-3b3b2) ─────────────────────────────────
```

## L399-418 · `pub enum SliceOutcome {`

```
/// Boot a Linux bzImage in our MicroVM substrate. Loads the bzImage
/// parts into a fresh 64 MB guest, configures 32-bit-prot-mode
/// entry per Linux Boot Protocol, runs a serial-aware exit loop
/// that captures Linux's earlyprintk output via the I/O bitmap.
///
/// `bzimage` is the raw bzImage bytes. `cmdline` is the kernel
/// command line (no NUL — loader appends one).
// ── Re-entrant VM context (Phase 12.4 step 1a) ─────────────────────
//
// The Linux run-loop is split into open() / run_slice() / close() so
// the Core-0 event loop can interleave Shade rendering between bounded
// slices instead of blocking until guest exit (see
// docs/archive/PHASE12_DISPLAY_BRIDGE.md, R1). Step 1a is behaviour-preserving:
// `run_linux` calls run_slice(u32::MAX) once, identical to the old
// `run_linux_loop`. Slicing + interleave is step 1b.
//
// vmx_enter_root/vmx_exit_root duplicate the VMXON + VMCS asm from
// `with_vmx_root_and_vmcs` (which the substrate-test path still uses
// unchanged — zero risk to proven code). TODO(cleanup): dedupe once
// the VmContext path is NUC-validated. Tracked.
```

## L420 · `pub enum SliceOutcome {`

```
/// Outcome of one bounded slice of guest execution.
```

## L422-423 · `StillRunning,`

```
/// Budget exhausted, guest still running — caller may re-enter
/// immediately (busy guest).
```

## L425-427 · `Idle,`

```
/// Guest is idle (halted / waiting on its timer) — caller should
/// re-enter but may host-idle first so the dedicated core doesn't
/// spin VMRUN while the guest has nothing to do.
```

## L429 · `Exited(vmcs::LaunchOutcome),`

```
/// Guest exited (HLT / panic / triple-fault / idle / cap).
```

## L433-435 · `fn vmx_enter_root() -> Result<(u64, u64), &'static str> {`

```
/// Enter VMX root mode and load a fresh current VMCS. Returns the
/// (kept, never-freed) VMXON + VMCS region phys addrs. Faithful copy
/// of `with_vmx_root_and_vmcs` steps 1-5 + `vmcs_setup_then_inner`.
```

## L437 · `let region_phys = memory::allocate_frame().ok_or("OOM allocating VMXON region")?;`

```
// 1. VMXON region.
```

## L441 · `unsafe {`

```
// SAFETY: identity-mapped, freshly-allocated, exclusive.
```

## L448 · `let feat = unsafe { rdmsr(IA32_FEATURE_CONTROL) };`

```
// 2. FEATURE_CONTROL.
```

## L452 · `unsafe { wrmsr(IA32_FEATURE_CONTROL, new); }`

```
// SAFETY: writing lock + outside-SMX bits to architectural MSR.
```

## L458 · `let cr0_f0 = unsafe { rdmsr(IA32_VMX_CR0_FIXED0) };`

```
// 3. CR0/CR4 fixed bits + CR4.VMXE.
```

## L465 · `unsafe {`

```
// SAFETY: CR reads cannot fault.
```

## L472 · `unsafe {`

```
// SAFETY: values satisfy fixed-bit constraints; VMXE is allowed.
```

## L478 · `let region_addr_slot: u64 = region_phys;`

```
// 4. VMXON.
```

## L481 · `unsafe {`

```
// SAFETY: VMXON requires CR4.VMXE (set above) + valid 4-KB region.
```

## L498 · `let vmcs_phys = match memory::allocate_frame() {`

```
// 5. VMCS region + VMCLEAR + VMPTRLD.
```

## L502 · `unsafe { vmx_exit_root(); }`

```
// Back out of VMX root so the CPU isn't stranded.
```

## L507 · `unsafe {`

```
// SAFETY: identity-mapped, freshly-allocated, exclusive.
```

## L515 · `unsafe {`

```
// SAFETY: in VMX root mode; valid VMCS region.
```

## L530 · `unsafe {`

```
// SAFETY: in VMX root mode; VMCS just successfully VMCLEAR'd.
```

## L548-552 · `unsafe fn vmx_exit_root() {`

```
/// Leave VMX root mode. Safe to call exactly once per successful
/// `vmx_enter_root`.
///
/// # Safety
/// Caller must be in VMX root mode (a prior `vmx_enter_root` Ok).
```

## L554 · `unsafe {`

```
// SAFETY: precondition documented; VMXOFF in VMX root is defined.
```

## L560-564 · `fn npf_kind(sh: &VmShared, gpa: u64) -> usize {`

```
/// State shared by ALL vCPUs of one microvm: the single guest address space
/// (RAM + EPT), the device model, and the host-tick/display bookkeeping. The
/// BSP owns it heap-boxed (behind `SharedRef`); an AP aliases it. Mirror of
/// svm `VmShared`.
/// Which target a nested page fault at `gpa` hits (`cores` NPF breakdown).
```

## L583-587 · `guest_mem: &'static GuestMem,`

```
/// Shared handle to the active guest memory (owned by `guest_mem`'s
/// `ACTIVE_GM`, freed at close). A reference — NOT the owned `GuestMem` — so
/// the off-vCPU net backend can hold the same `&'static GuestMem` without
/// aliasing this `&mut VmShared` (borrow governs the pointer, not the `Sync`
/// pointee). Mirror of svm.
```

## L589-591 · `ept_pml4: u64,`

```
/// EPT PML4 phys — `close()` passes it to `ept::release` to free
/// every demand-faulted 4 KB frame + the demand PT pages + the
/// fixed tables.
```

## L593-595 · `eptp: u64,`

```
/// EPTP (PML4 phys | walk-length | mem-type) to VMWRITE into each
/// vCPU's VMCS::EPT_POINTER. The AP reuses it (one shared address
/// space) in `open_ap`'s `setup_execution_controls`.
```

## L597-599 · `guest_raw_base: u64,`

```
/// Base of the **contiguous boot-window** allocation (pre-2 MB-
/// align). `close()` frees `ept::boot_frames_for(guest_mem.len())`
/// from here; the demand region is freed via `ept::release`.
```

## L604-611 · `last_cfg_tick: u64,`

```
/// Host TSC of the last raised net-RX IRQ10 — interrupt moderation (ITR).
/// Firing IRQ10 on every net-MMIO exit (~14k/s) gave the guest ~1 interrupt
/// per packet (io=18170/s EOI storm) → NAPI defeated, the vCPU pegged on
/// IRQ-entry/EOI instead of processing. Frames are still delivered into the
/// ring every exit; the interrupt is raised at most ~1 per NET_IRQ_GAP_US so
/// the guest's NAPI drains big batches like on a real NIC with coalescing.
/// `ticks()` of the last virtio-gpu display config-change IRQ — rate-
/// limits the resize round-trip (R2 debounce).
```

## L613 · `last_reap_tick: u64,`

```
/// `ticks()` of the last NAT mapping reap.
```

## L615-621 · `pit: crate::microvm::devices::pit8253::Pit,`

```
/// True until Linux writes PIT mode-0 (port 0x43 ← 0x30) to disable the
/// i8253 after adopting the LAPIC timer. Gates IRQ0 so jiffies don't
/// double-count. During calibration BOTH tick 1:1 (else "APIC timer
/// disabled").
/// i8253 channel 0. Was a bare `pit_enabled: bool` with no reload value,
/// which left the tick with no period — it had to be guessed at a hardcoded
/// 1 kHz to match the LAPIC. See `pit8253` for why the guess is the bug.
```

## L625-628 · `pub struct Vcpu {`

```
/// Per-vCPU state: its own VMXON region + VMCS (VMX root is PER-CORE, so each
/// vCPU enters root on its own worker core), register file, FPU areas, LAPIC,
/// and per-vCPU exit bookkeeping. Guest SMP = N of these against one shared
/// `VmShared`. Mirror of svm `Vcpu`.
```

## L630-633 · `apic_id: u8,`

```
/// This vCPU's physical (x)APIC ID — BSP = 0, APs = 1.. Reported
/// identically by CPUID (leaf 1 EBX[31:24], leaf 0xB/0x1F EDX), the
/// emulated LAPIC, the IA32_APICBASE BSP bit (set only when 0), and the
/// MP-table, or Linux's topology code rejects the CPU.
```

## L643 · `msrs: super::msr::GuestMsrs,`

```
/// Emulated MSR state (MTRR, SPEC_CTRL, MISC_ENABLE …) — see `vmx::msr`.
```

## L645-647 · `reinject: u64,`

```
/// Event mid-vectoring at the last exit (IDT_VECTORING_INFO; re-injected
/// next entry — only type 0/2). SDM §27.2.4. See the field doc on the SVM
/// twin for the type-gate rationale.
```

## L649-650 · `lapic: LocalApic,`

```
/// Per-vCPU emulated local APIC (Intel parity #2 / guest SMP). Inert while
/// booting `nolapic`; drives the timer + IPIs once Linux enables it.
```

## L652 · `halt_poll_us: u64,`

```
/// Adaptive halt-poll window (µs), see `HALT_POLL_MIN_US`.
```

## L654 · `irq_window: bool,`

```
/// Interrupt-window exiting currently requested (mirrors the VMCS bit).
```

## L656 · `host_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,`

```
/// Host/guest FPU (XSAVE) save areas — VMRESUME preserves neither.
```

## L661-666 · `pub enum SharedRef {`

```
/// Handle to a microvm's `VmShared`. The BSP vCPU **owns** it (heap-boxed so
/// its address is stable while the BSP's `VmContext` moves on the fiber
/// stack); an AP holds `Borrowed` — a raw pointer to the same box, valid for
/// the VM lifetime via the last-one-out refcount. `Deref`/`DerefMut` make
/// every `self.shared.X` access work for both; access is serialised by
/// `VM_BIG_LOCK`, not this handle. Mirror of svm `SharedRef`.
```

## L672-674 · `unsafe impl Send for SharedRef {}`

```
// SAFETY: Borrowed is a raw pointer moved only into AP fiber tasks; all
// pointee access is VM_BIG_LOCK-serialised. The Send marker lets it cross
// into the spawned fiber.
```

## L688 · `SharedRef::Borrowed(p) => unsafe { &**p },`

```
// SAFETY: valid for the VM lifetime (see SharedRef doc).
```

## L698 · `SharedRef::Borrowed(p) => unsafe { &mut **p },`

```
// SAFETY: as above; access is VM_BIG_LOCK-serialised.
```

## L704-707 · `pub struct VmContext {`

```
/// Persistent state of one Linux microvm across cooperative slices.
/// Core-agnostic: nothing here assumes which core it runs on (forward-compat
/// contract #1). Composed of `VmShared` (all-vCPU, behind `SharedRef`) + one
/// `Vcpu`.
```

## L714-717 · `pub fn open(`

```
/// Enter VMX root, load the VMCS, place the guest image, set up
/// guest/host state + execution controls, pre-inject the UART RX
/// FIFO. On any post-VMXON failure, VMXOFF before returning Err so
/// the CPU never strands in VMX root.
```

## L728 · `let build = || -> Result<VmContext, &'static str> {`

```
// Everything past here is in VMX root: VMXOFF on any error.
```

## L741-742 · `let gm = crate::microvm::devices::guest_mem::set_active(gm);`

```
// Install as the active guest memory (out of VmShared); `gm` is now
// the shared `&'static` handle, also reachable by the net backend.
```

## L757-758 · `crate::microvm::devices::net_backend::reset();`

```
// virtio-net lives in net_backend (a static, out of VmShared) so the
// off-vCPU backend can own it; re-arm it to power-on state per VM.
```

## L771 · `let mut pic = crate::microvm::devices::pic8259::Pic8259::new();`

```
// The I/O APIC's ID follows the vCPUs' in the MP table.
```

## L781 · `apic_id: 0, // BSP`

```
// BSP
```

## L804 · `unsafe { vmx_exit_root(); }`

```
// SAFETY: vmx_enter_root succeeded → we are in VMX root.
```

## L811-818 · `pub fn open_ap(`

```
/// Build an AP vCPU that **shares** an already-open BSP's `VmShared`
/// (guest SMP). Enters VMX root ON THIS (the AP's) worker core — VMX root
/// + VMCS are per-physical-core, so the AP allocates its own VMXON/VMCS
/// here — installs this core's TSS (valid HOST_TR, like `vm_open`),
/// configures a real-mode SIPI-entry guest state, and reuses the BSP's
/// shared EPTP / device model. No guest RAM / EPT / device allocation
/// (those belong to the BSP, freed last-one-out). `apic_id` is 1.. Mirror
/// of svm `open_ap`.
```

## L824 · `crate::tss::ensure_core(crate::smp::per_core::current_core_id());`

```
// Worker-core TSS so HOST_TR is valid at VM-entry (see vmx::vm_open).
```

## L829 · `let build = || -> Result<VmContext, &'static str> {`

```
// In VMX root now: VMXOFF on any error so the core never strands.
```

## L831-834 · `let eptp = {`

```
// Read the shared EPTP (set once at BSP open, never mutated). Take
// VM_BIG_LOCK: AP_ACTIVE is set by now, so the BSP may be touching
// `*shared` under the lock — holding it keeps this `&*shared`
// borrow from aliasing the BSP's `&mut`.
```

## L837-838 · `unsafe { (*shared).eptp }`

```
// SAFETY: `shared` is valid for the VM lifetime (SharedRef),
// and the lock excludes any concurrent `&mut` to `*shared`.
```

## L871 · `unsafe { vmx_exit_root(); }`

```
// SAFETY: vmx_enter_root succeeded → we are in VMX root.
```

## L878-879 · `pub fn shared_ptr(&mut self) -> *mut VmShared {`

```
/// Raw pointer to this VM's shared state, for an AP vCPU to alias
/// (`open_ap`). Only valid on the BSP's `Owned` context.
```

## L884-887 · `pub fn next_timer_deadline_tsc(&self) -> Option<u64> {`

```
/// Absolute host TSC of the BSP vCPU's next guest LAPIC-timer tick — the
/// hrtimer deadline the idle park waits on (KVM `apic_timer_fn` model), so
/// the guest 1 kHz clock advances independent of VMRUN. `None` when no guest
/// timer is armed → the park falls back to its safety cap.
```

## L896 · `if self.shared.pci.virtio_snd.playing() {`

```
// A playing sound stream is serviced every millisecond.
```

## L908-910 · `fn collect_device_irqs(&mut self) {`

```
/// Feed every device line and the PIT into the PIC (BSP: in PIC mode the
/// device lines are wired there). Lock held by the caller when APs run.
/// Mirror of the SVM backend.
```

## L918 · `if crate::microvm::devices::gpu_backend::take_resume(crate::interrupts::rdtsc())`

```
// vblank: a paused controlq runs its next frame (virtio-gpu IRQ 9).
```

## L937-938 · `if crate::microvm::devices::gpu_backend::d4_pending()`

```
// Live resize (D4): disconnect, then reconnect after 100 ms; a new
// cycle at most every 250 ms while the window is dragged.
```

## L956 · `if now != sh.last_reap_tick {`

```
// NAT mapping reaper: an idle scan, once per 10 ms at most.
```

## L963-964 · `fn pending_interrupt(&mut self) -> Option<bool> {`

```
/// `kvm_cpu_has_interrupt`: the PIC's INTR if this vCPU takes it (LINT0 in
/// ExtINT, or no LAPIC), else a LAPIC vector above PPR.
```

## L979-980 · `fn set_irq_window(&mut self, on: bool) -> Result<(), &'static str> {`

```
/// Interrupt-window exiting (CPU-based control bit 2) — only VMWRITEs on a
/// change.
```

## L989-992 · `fn inject_pending_event(&mut self) -> Result<(), &'static str> {`

```
/// `inject_pending_event` (`vmx_inject_irq` / `vmx_enable_irq_window`),
/// before every VM entry. An external interrupt may only be injected when
/// the guest is interruptible — on Intel an IF=0 inject fails the entry
/// (reason 33, SDM §26.3.1.5) — so otherwise the window is opened.
```

## L1017 · `let vector = if from_pic {`

```
// `kvm_cpu_get_interrupt`: ExtINT first, then the LAPIC.
```

## L1030 · `fn event_pending(&mut self) -> bool {`

```
/// Anything deliverable right now (takes the lock + collects on the BSP).
```

## L1038 · `fn halt_poll(&mut self) -> bool {`

```
/// `kvm_vcpu_halt` with adaptive halt-polling. Mirror of the SVM backend.
```

## L1068-1080 · `pub fn close(&mut self) {`

```
/// Leave VMX root and free the per-VM allocations so the VM can be
/// relaunched in the same boot. Order matters: VMXOFF first (the
/// VMCS must not be current/in-use when its frame is freed), then
/// reclaim frames. The profile image was already persisted inside
/// run_slice on the guest-exit path.
///
/// BSP-only (`Owned`): frees the shared guest RAM + EPT + device-backed
/// frames. An AP must use `close_ap` (its own VMX root only — the shared
/// state belongs to the BSP, freed last-one-out).
///
/// TODO(12.x): the EPT page-table frames from `ept::install_window`
/// still leak (~tens of KB per run — negligible vs. the GB guest RAM
/// this now reclaims). Tracked; needs an EPT teardown walker.
```

## L1082-1083 · `crate::microvm::devices::net_dataplane::stop_worker();`

```
// Stop the off-vCPU net backend FIRST (it holds &'static GuestMem via
// guest_mem::active(), freed by clear_active() below). Idempotent.
```

## L1085-1087 · `self.shared.pci.virtio_blk.save();`

```
// Persist the home image BEFORE teardown — see the svm mirror:
// the Mod+Q window-close path reaches close() without run_slice's
// loop-end save(), so without this the profile is lost on close.
```

## L1089 · `unsafe { vmx_exit_root(); }`

```
// SAFETY: this vCPU entered VMX root on this core → VMXOFF is valid.
```

## L1091 · `ept::release(self.shared.ept_pml4, self.shared.guest_mem.len());`

```
// Demand-faulted frames + demand PTs + EPT tables.
```

## L1093 · `memory::deallocate_contiguous(`

```
// Contiguous boot window.
```

## L1100-1101 · `crate::microvm::devices::guest_mem::clear_active();`

```
// Free the active GuestMem (held outside VmShared); page tables are
// released above and all vCPUs + the net backend have stopped.
```

## L1105-1109 · `pub fn close_ap(&mut self) {`

```
/// Tear down ONLY this AP vCPU's VMX root (VMXOFF on its core + free its
/// VMXON/VMCS frames). Does NOT touch the shared state — the BSP owns +
/// frees that once the last vCPU has exited. VMX-specific: unlike SVM
/// (where the AP just stops VMRUNning), the AP entered VMX root via
/// `vmx_enter_root`, so it must VMXOFF on its own core.
```

## L1111 · `unsafe { vmx_exit_root(); }`

```
// SAFETY: this AP entered VMX root on this core → VMXOFF is valid.
```

## L1118 · `struct SerialState {`

```
/// Per-guest serial UART state across exits.
```

## L1120-1122 · `dlab: bool,`

```
/// LCR.DLAB bit. When set, OUT to 0x3F8 / 0x3F9 means
/// divisor-latch low/high (we ignore). When clear, 0x3F8 is
/// THR (the byte the kernel wants to print).
```

## L1124-1126 · `line: [u8; 256],`

```
/// Buffered output line — flushed via kprintln on '\n' or
/// when the buffer is full. Linux's printk emits one line at
/// a time so this rarely fills.
```

## L1129-1132 · `panic_observed: bool,`

```
/// Set on first observed `Kernel panic - not syncing:` line.
/// Used by the loop's exit summary so the post-panic triple-fault
/// is reported as the expected reboot path rather than an
/// "unhandled exit reason 2".
```

## L1134-1136 · `panic_msg: [u8; 192],`

```
/// Captured trailing text of the panic line (after the
/// `Kernel panic - not syncing: ` prefix), e.g. the VFS
/// `Unable to mount root fs` reason.
```

## L1139-1141 · `halt_observed: bool,`

```
/// Set on the first `reboot: System halted` / `Power down` line — the
/// guest shut itself down (LibreWolf X → cage exit → PID-1 halt). Drives
/// the auto-close-and-save so the user doesn't need a second Mod+Q.
```

## L1143-1147 · `rx: [u8; 128],`

```
/// Phase 12.1.4 — RX FIFO. Bytes pre-injected by the host before
/// VMLAUNCH; drained one at a time when the guest reads RBR
/// (0x3F8 IN with DLAB=0). LSR.DR (bit 0) on 0x3FD IN reflects
/// `rx_pos < rx_n`. The guest-side counterpart in microvm-init
/// busy-polls 0x3FD via iopl(3) + inb.
```

## L1171-1172 · `fn inject(&mut self, bytes: &[u8]) {`

```
/// Pre-load the RX FIFO with bytes the host wants the guest to
/// receive on its next 0x3F8 reads. Truncates silently to capacity.
```

## L1224-1226 · `fn scan_for_panic(&mut self, n: usize) {`

```
/// Search the just-completed `self.line[..n]` for the kernel-
/// panic marker. Linux's printk frame is `<level>timestamp> body`,
/// so the marker can sit anywhere on the line — substring match.
```

## L1249-1253 · `fn scan_for_shutdown(&mut self, n: usize) {`

```
/// Detect a clean guest self-shutdown in the printk stream. Linux's
/// reboot path emits `reboot: System halted` (PID-1 `halt`) or
/// `reboot: Power down`. The marker is specific to kernel/reboot.c, so
/// an app log line can't false-trigger it. On the first hit, ask the run
/// loop to take the normal Mod+Q close path (break → save → window close).
```

## L1256-1258 · `let line = &self.line[..n];`

```
// Require the `reboot: ` prefix (kernel/reboot.c only). PID-1 mirrors
// cage/moz app logs to /dev/kmsg → serial, so a bare "Power down"
// substring could false-trigger; the prefix can't appear in app text.
```

## L1271 · `fn line_contains(hay: &[u8], needle: &[u8]) -> bool {`

```
/// True if `hay` contains `needle` as a contiguous byte substring.
```

## L1277-1280 · `struct IoStats {`

```
/// Per-port I/O exit counter. Linux's boot touches dozens of unique
/// ports (PCI config, PIC, PIT, RTC, serial, keyboard, etc.).
/// Counting them tells us what the guest actually did when no
/// `[guest]` lines appeared.
```

## L1284 · `serial_bytes: [u8; 256],`

```
/// First N bytes written to UART THR (port 0x3F8 with DLAB=0).
```

## L1324 · `let mut buf: [u8; 256] = [0; 256];`

```
// Print as ASCII-safe + hex-on-non-printable
```

## L1338-1340 · `struct ExitTrace {`

```
/// Per-iteration exit trace recorded for post-mortem on unhandled
/// exits. Keeps the last 32 (reason, qual_low32) tuples so we can
/// see what Linux was doing in the run-up to a triple-fault.
```

## L1367-1369 · `fn dump_page_walk(mem: &GuestMem, cr3: u64, virt: u64) {`

```
/// Walk guest's 4-level page tables for `virt`, print each level's
/// entry. EPT identity-shifts guest-phys X → host-phys host_base+X
/// within the 64 MB window, so we just offset.
```

## L1422-1425 · `pub fn run_slice(&mut self, budget: u32) -> Result<SliceOutcome, &'static str> {`

```
/// Run the guest for up to `budget` VM-exits, or until it exits.
/// `Ok(StillRunning)` = budget hit, re-enterable (step 1b);
/// `Ok(Exited(o))` = guest left; `Err` = setup/VM fault. Body is
/// the old `run_linux_loop` verbatim, `self.`-scoped.
```

## L1434-1435 · `const SLICE_MS: u64 = 3;`

```
// Wall-clock slice cap: return to the fiber scheduler every few ms so the
// core's other fibers run; the exit budget bounds boot bursts.
```

## L1440-1442 · `let mut prof_post: u64 = 0;`

```
// Host-time profiler: `prof_post` = TSC right after the previous VMRESUME,
// `prof_bucket` = that exit's bucket; the gap to the next VMRESUME is the
// handler cost.
```

## L1446 · `let host_core = crate::smp::per_core::current_core_id();`

```
// Publish this vCPU's host core so a sender can kick it.
```

## L1457 · `if self.vcpu.launched {`

```
// The IA-32e-mode-guest entry control must match GUEST_IA32_EFER.LMA.
```

## L1466-1467 · `let entry_deadline =`

```
// The guest's next timer, or the slice end, as a host one-shot on this
// core: its fire is the exit that delivers the tick on time.
```

## L1470-1471 · `if !crate::microvm::cpu::entry_irqs_off(kick_gen, host_core, entry_deadline) {`

```
// IF stays 0 into VMRESUME (external-interrupt exiting exits on a
// pending interrupt regardless); the asm sets it again after the exit.
```

## L1475-1483 · `let hf: *mut crate::microvm::cpu::FpuArea = &mut *self.vcpu.host_fpu;`

```
// FPU host↔guest swap is now embedded inside run_guest_once's
// asm (mirror of SVM v0.172.53), bracketing VMRESUME with zero
// compiler-emittable code between xrstor and vmresume. A +avx2
// kernel can spill `vmovups ymm` anywhere between a Rust
// helper and the asm, clobbering the just-restored guest FPU
// — putting xsave/xrstor in-asm closes that window. Pass the
// host/guest FPU pointers in; the asm save/restore mask=-1
// (current-XCR0 components; guest XCR0 ⊇ host's via XSETBV
// pass-through).
```

## L1486-1487 · `let prof_pre = crate::interrupts::rdtsc();`

```
// Profiler: charge the time since the last exit to that exit's
// handler bucket, then time the VMRESUME itself as guest cycles.
```

## L1502-1505 · `let entry_intr_used = vmcs::read_entry_intr_info().unwrap_or(0xDEAD);`

```
// DIAG (bare-metal reason-33): snapshot the injection field that
// was live for the entry we just ran, BEFORE the clear below. On
// a VM-entry failure (reason 33) the event is never delivered, so
// this still holds what we wrote — confirms inject-vs-no-inject.
```

## L1507-1516 · `let _ = vmcs::write_entry_intr_info(0);`

```
// Consume the VM_ENTRY_INTR_INFO slot. On bare-metal VMX the
// CPU auto-clears the valid bit after a successful injection
// (SDM §27.6), but under nested VMX (KVM emulating VMX) it
// MAY leave the bit set — the next VMRESUME would then
// re-inject the same event = phantom duplicate interrupt →
// cumulative guest corruption. KVM clears the field on every
// exit for exactly this; do the same. The reinject/handler
// sites re-arm the slot for the next VMRESUME as needed.
// This is the VMX equivalent of SVM v0.172.42's EVENTINJ
// clear.
```

## L1518-1527 · `let idtv = vmcs::read_idt_vectoring_info().unwrap_or(0);`

```
// Did an event abort mid-vectoring through the guest IDT?
// IDT_VECTORING_INFO has the same encoding as VM_ENTRY_INTR_
// INFO; stash it verbatim for re-injection on the next entry.
// Type-gate to external IRQ (0) and NMI (2) only — exceptions
// (type 3, e.g. #PF / #GP) and software ints (type 4) MUST
// NOT be re-injected: the faulting RIP was not advanced
// (e.g. EPT-violation doesn't retire the access), so the
// guest re-executes the instruction and the exception
// re-occurs naturally; re-injecting too = double delivery.
// Mirrors SVM v0.172.38's type-gate.
```

## L1539 · `prof_bucket = match basic {`

```
// Exit-reason histogram (diagnosis — `cores` shows the mix).
```

## L1543 · `48 => crate::microvm::cpu::VMX_MMIO, // EPT violation`

```
// EPT violation
```

## L1551-1560 · `const ITER_TRACE: bool = false;`

```
// DIAG (bare-metal reason-33): per-exit mode trace for the first
// ~14 exits. Shows EXACTLY when the guest enters long mode (CS.L
// / EFER.LMA flip 0→1) and whether re-entry into long mode
// succeeds — so we can tell "first long-mode entry fails" from
// "only the post-external-interrupt re-entry fails".
//
// The bug it was cut for is long fixed, and "bounded to early boot" is
// 14 lines PER vCPU: six of them on this notebook, 84 lines before the
// guest has printed its first word. Off unless someone is chasing a
// long-mode entry again.
```

## L1572-1580 · `match basic {`

```
// Take the big-VM lock around this exit's device/memory handling when
// an AP shares the VM (guest SMP) so the two vCPUs serialise access to
// `VmShared`. Both `_big` and `sh` drop at the loop-body end — `sh`'s
// `&mut VmShared` borrow ends before the lock releases (reverse decl
// order), so only the lock holder ever has a live `&mut` to the shared
// state (sound aliasing across the BSP's Owned box + the AP's Borrowed
// pointer). No AP active → lock not taken → byte-identical single-vCPU.
// Exits that touch only this vCPU take no VM_BIG_LOCK (see the SVM
// run loop): the interrupt window, an MSR (x2APIC, PV-EOI), a hypercall.
```

## L1582-1583 · `1 => {`

```
// A host interrupt (timer, device, kick IPI) pre-empted the guest;
// the host took it on exit. Pending guest events inject at entry.
```

## L1588-1589 · `7 => {`

```
// Interrupt window open (`vmx_enable_irq_window`): the guest can
// take the event now — the next entry injects it.
```

## L1596-1597 · `let msr = self.vcpu.regs.rcx as u32;`

```
// RDMSR / WRMSR: every MSR outside the pass-through set is
// intercepted and emulated in `vmx::msr`; none reaches the host.
```

## L1606 · `match r {`

```
// IA32_APIC_BASE, x2APIC registers, PV-EOI enable.
```

## L1654-1658 · `let sh = &mut *self.shared;`

```
// Resolve the shared device/memory state once for this exit (a single
// `SharedRef::DerefMut` borrow of `self.shared`, so the handlers below
// keep their disjoint sub-field borrows; `self.vcpu` stays separately
// borrowable). Re-bound per iteration; NEVER held across run_guest_once
// / a yield. Device/timer/nat/input handling is BSP-only.
```

## L1663-1664 · `crate::interrupts::NMI_COUNT.fetch_add(1, Ordering::Relaxed);`

```
// Host NMI while the guest ran (NMI exiting): the NMI is
// consumed by the exit. Counted like the host handler does.
```

## L1669-1674 · `sh.serial.flush();`

```
// Exception/NMI. EXCEPTION_BITMAP=0 in production —
// exceptions go to Linux's IDT directly. This arm
// only fires for NMIs (which we don't generate
// intentionally) or if Linux somehow re-enables
// exception trapping. Kept as a safety net + the
// dump remains useful if it ever fires.
```

## L1725 · `if crate::microvm::vm_window() == 0 {`

```
// A headless test guest halting is done.
```

## L1736-1738 · `if !self.halt_poll() {`

```
// `kvm_vcpu_halt`: resume at once if something is deliverable,
// else halt-poll, then block (the fiber parks until the next
// timer deadline or a kick).
```

## L1744-1746 · `let (eax, ebx, ecx, edx) = crate::microvm::cpu::guest_cpuid::guest_cpuid(`

```
// CPUID — VMX always exits. Allowlist shared with SVM
// (`cpu::guest_cpuid`, after KVM `kvm_cpu_cap_init`). XCR0
// is the host's (7): XSETBV exits and never reaches hardware.
```

## L1761-1764 · `let qual = outcome.exit_qualification;`

```
// Control-register access. Most commonly Linux's
// startup_32 doing MOV CR3, reg to load its own
// page tables — IA32_VMX_PROCBASED_CTLS may force
// CR3-load/store-exiting on this CPU even with EPT.
```

## L1781 · `let val = read_gpr(&self.vcpu.regs, gp_reg)?;`

```
// MOV to CR3 (set page-table base).
```

## L1786 · `let val = vmcs::read_guest_cr3()?;`

```
// MOV from CR3.
```

## L1815-1818 · `let val = (self.vcpu.regs.rdx << 32) | (self.vcpu.regs.rax & 0xFFFF_FFFF);`

```
// XSETBV — VMX always exits, and the value never reaches
// hardware: the host's XCR0 (x87|SSE|AVX) stays live, and
// CPUID 0xD offers exactly that. Validate like KVM
// `__kvm_set_xcr` so a bad value faults as on real hardware.
```

## L1831-1834 · `11 | 17 | 19..=27 | 36 | 39 | 50 | 53 | 59 | 60 => {`

```
// VMX, SMX, SGX and RSM: the guest has none of them (CPUID hides
// VMX/SMX/SGX) → #UD, as KVM does without nesting. MONITOR/MWAIT
// are hidden too.
// VMCALL: KVM hypercall (`kvm_emulate_hypercall`), see the SVM arm.
```

## L1839 · `15 => {`

```
// No vPMU: RDPMC faults.
```

## L1844-1845 · `13 | 54 => {`

```
// INVD / WBINVD: no non-coherent DMA into the guest → no-ops
// (KVM `kvm_emulate_wbinvd`, INVD treated as WBINVD).
```

## L1851-1854 · `let gpa = vmcs::read_guest_phys_addr().unwrap_or(0);`

```
// EPT violation — guest tried to access a guest-phys
// address outside our 64 MB window (or with insufficient
// EPT permissions). For accesses landing in virtio-blk's
// BAR0 range we emulate; everything else dumps + bails.
```

## L1857-1859 · `{`

```
// LAPIC MMIO page (0xFEE00000) → trap-and-emulate (Intel
// parity #2). Left EPT-not-present in ept.rs when LAPIC is on.
// I/O APIC page (0xFEC00000), EPT-not-present like the LAPIC's.
```

## L1884-1886 · `last_outcome = Some(outcome);`

```
// Deliver a deferred device IRQ (esp. an async 9p
// write-completion) NOW rather than at the next
// reason-1/12 exit ~10 ms out — the download rxlat fix.
```

## L1903-1905 · `last_outcome = Some(outcome);`

```
// Deliver the freshly-completed 9p write-reply IRQ NOW
// (latched by drain_async_done at the loop top) instead
// of waiting for the next reason-1/12 exit.
```

## L1910 · `if handle_mmio_ept_blk(&mut self.vcpu.regs, &mut sh.pci.virtio_blk_sqfs, &mut sh.pic, gpa, sh.guest_mem) {`

```
// Same handler — VirtioBlk carries its own IRQ line.
```

## L1921-1926 · `if sh.guest_mem.ensure(gpa) {`

```
// B3: demand-paged guest RAM. A violation on a gpa
// inside the advertised window but above the
// contiguous boot block = first touch of a 4-KB
// demand page → fault it in + re-enter. Ordering is
// load-bearing: MMIO BAR ranges first (above),
// RAM-demand here, fatal dump last.
```

## L1953-1955 · `sh.serial.flush();`

```
// Triple fault. Linux uses this as `emergency_restart`
// when ACPI/PIIX/EFI reset paths are unavailable —
// i.e. the standard exit path on `panic=1` here.
```

## L1986-1991 · `if basic == 33 || basic == 34 || basic == 41 {`

```
// VM-entry failure (33/34/41): dump guest state so we can
// tell *which* consistency check the CPU rejected.
// Bare-metal NUC reports 33 right after Linux's CR3
// long-mode trampoline — likely the IA-32e/CR/EFER
// triad is inconsistent and we need to see exactly
// which field. SDM Vol 3 §26.3.1 enumerates the checks.
```

## L2022-2025 · `self.shared.pci.virtio_blk.save();`

```
// Persist the virtio-blk profile-image to npkFS (encrypted at rest).
// Reached only when the loop ended (guest exit / cap), not on a
// StillRunning yield or a `?` early-return — identical to the old
// run_linux_loop.
```

## L2035-2038 · `fn read_gpr(regs: &vmcs::GuestRegs, idx: u8) -> Result<u64, &'static str> {`

```
/// Read a guest GPR by ABI register index (0=rax, 1=rcx, 2=rdx,
/// 3=rbx, 4=rsp, 5=rbp, 6=rsi, 7=rdi, 8..15=r8..r15) for CR-access
/// VM-exit decoding. RSP comes from VMCS, the rest from the saved
/// GuestRegs struct.
```

## L2061-2062 · `fn write_gpr(regs: &mut vmcs::GuestRegs, idx: u8, value: u64) -> Result<(), &'static str> {`

```
/// Write a guest GPR by ABI register index. RSP goes to VMCS, the
/// rest to the saved GuestRegs struct.
```

## L2086-2089 · `fn handle_linux_io(`

```
/// Dispatch a single I/O VM-exit. UART COM1 (0x3F8-0x3FF) gets
/// proper synthetic responses so Linux's earlyprintk poll-loop
/// thinks the transmitter is always ready; everything else is
/// silently absorbed (return 0 for IN, no-op for OUT).
```

## L2107 · `if port == PCI_CONFIG_ADDR`

```
// PCI config-space ports — dispatch to the bus emulator.
```

## L2117 · `if matches!(port, PIC_MASTER_CMD | PIC_MASTER_IMR | PIC_SLAVE_CMD | PIC_SLAVE_IMR`

```
// 8259 PIC stub — see microvm::devices::pic8259.
```

## L2127-2130 · `(0x60 | 0x64, true) => {`

```
// i8253 channel 0: mode/command (0x43) and the counter itself (0x40).
// The counter write is new — it was dropped on the floor before, which
// is why the tick had no period of its own.
// i8042: absent — all-ones, see the SVM `handle_linux_io`.
```

## L2136 · `(0x3F8, false) => {`

```
// COM1 OUT.
```

## L2141 · `}`

```
// else: divisor-latch low byte, ignored.
```

## L2144 · `}`

```
// IER (DLAB=0) or DLM (DLAB=1) — both ignored.
```

## L2147 · `serial.dlab = (val_out & 0x80) != 0;`

```
// LCR — track DLAB bit.
```

## L2150 · `(0x3F8, true) => {`

```
// COM1 IN — synthetic responses.
```

## L2152-2156 · `let v = if !serial.dlab { serial.rx_take() as u64 } else { 0 };`

```
// RBR (DLAB=0): pop one byte from the host-injected RX
// FIFO. DLL (DLAB=1): we don't model divisor latches —
// return 0. With an empty FIFO this also returns 0,
// matching real hardware where reading RBR with DR=0 is
// undefined-but-typically-zero.
```

## L2161-2162 · `regs.rax = (regs.rax & !mask) | (0x01u64 & mask);`

```
// IIR: bit 0 = "no interrupt pending" (which on read
// also sources type=0 = no FIFO).
```

## L2166-2169 · `let dr = if serial.rx_has_data() { 0x01u64 } else { 0 };`

```
// LSR: bit 5 (THR empty) | bit 6 (TSR empty) always set,
// plus bit 0 (DR — data ready) reflects the RX FIFO.
// Polling loops in the guest see DR=1 the moment the
// host has injected, and read RBR until the FIFO drains.
```

## L2174 · `regs.rax = (regs.rax & !mask) | (0xB0u64 & mask);`

```
// MSR: CTS asserted (bit 4) + DSR (bit 5) + DCD (bit 7).
```

## L2177 · `(0x3FA..=0x3FF, true) => {`

```
// Other UART regs (0x3FC MCR, 0x3FF SCR): default 0.
```

## L2181 · `(_, true) => {`

```
// Default IN: zero. Default OUT: drop.
```

## L2192-2204 · `fn handle_mmio_ioapic(`

```
/// Handle an EPT violation that targets virtio-blk's BAR0 MMIO range.
/// Walks the guest's page tables to fetch the faulting instruction
/// (VMX has no decode-assists), decodes the MOV form, emulates against
/// the device, advances RIP via `VM_EXIT_INSTRUCTION_LEN`.
///
/// Returns `true` if the fault was handled, `false` otherwise (page
/// walk failed, opcode unsupported).
/// EPT-violation on the LAPIC MMIO page (Intel parity #2 / guest SMP). Decode
/// the faulting MOV, service it against the per-vCPU `LocalApic`, advance RIP.
/// xAPIC registers are 32-bit; no device IRQ-kick. An ICR write returns the
/// decoded IPI, which `route_ipi` delivers cross-vCPU (INIT/SIPI bring-up +
/// FIXED reschedule/TLB). Mirrors svm `handle_mmio_npf_lapic`.
/// EPT violation on the I/O APIC page → `devices::ioapic` (register window).
```

## L2332-2334 · `if let Some(qidx) = blk.take_pending_kick() {`

```
// If the write was a queue-notify, service the queue and inject
// IRQ 11 (virtio-blk's INTx line, mapped through our 8259 stub
// to the vector Linux programmed via ICW2).
```

## L2354 · `4  => 0, // RSP — VMCS holds it; never an MMIO source on Linux`

```
// RSP — VMCS holds it; never an MMIO source on Linux
```

## L2377 · `4  => {} // RSP — silently drop`

```
// RSP — silently drop
```

## L2393-2395 · `fn handle_mmio_ept_net(`

```
/// Handle an EPT violation that targets virtio-net's BAR0. Identical
/// pattern to `handle_mmio_ept_blk` — only the device + IRQ line
/// differ. We'll de-duplicate via a trait once virtio-gpu joins (12.4).
```

## L2425 · `if let Some(v) = crate::microvm::devices::net_backend::mmio_fast(off, dec.is_write) {`

```
// ISR read and TX doorbell: lock-free (the worker may hold the device).
```

## L2444-2448 · `crate::microvm::devices::net_backend::note_tx_kick();`

```
// The worker owns the guest TX ring. Ring its doorbell instead of
// draining the ring here — two consumers on one virtqueue is
// corruption, not a race you get away with. SVM has had this guard
// since the off-vCPU TX landed; VMX did not, because on Intel the
// worker never ran. It runs now.
```

## L2452-2454 · `if advanced && !(crate::microvm::devices::net_backend::msix_notify(0)`

```
// `service_queues` may complete TX and inject RX replies in one
// go; under MSI-X both queues' vectors fire (a spurious one is
// harmless: the guest finds nothing new), under INTx IRQ 10.
```

## L2458 · `pic.pulse(10);`

```
// virtio-net IRQ line = 10 (per pci config 0x3C).
```

## L2469-2470 · `fn handle_mmio_ept_gpu(`

```
/// Handle EPT-trap on virtio-gpu BAR0. Mirror of `handle_mmio_ept_net`
/// — only the device and IRQ line differ.
```

## L2510 · `crate::microvm::devices::gpu_backend::note_gpu_kick(qidx);`

```
// Off-vCPU: defer the heavy copy + write_frame to the GPU worker.
```

## L2515 · `pic.pulse(9);`

```
// virtio-gpu IRQ line = 9.
```

## L2525-2526 · `fn handle_mmio_ept_input(`

```
/// Handle EPT-trap on virtio-input BAR0. Mirror of `handle_mmio_ept_gpu`
/// — only the device + IRQ line differ.
```

## L2567 · `pic.pulse(12);`

```
// virtio-input IRQ line = 12.
```

## L2576-2577 · `fn handle_mmio_ept_p9(`

```
/// Handle an EPT violation on virtio-9p BAR0. Mirror of
/// `handle_mmio_ept_input` — only the device + IRQ line (6) differ.
```

## L2626-2627 · `fn handle_mmio_ept_snd(`

```
/// Handle EPT-trap on virtio-snd BAR0. Mirror of `handle_mmio_ept_net` —
/// only the device + IRQ line (8) differ.
```

