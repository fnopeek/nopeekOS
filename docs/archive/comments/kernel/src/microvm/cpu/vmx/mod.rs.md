# `kernel/src/microvm/cpu/vmx/mod.rs` @ 5e0102684

## L1-31 · `mod enable;`

```
//! VMX (Intel VT-x) — Phase 12 MicroVM substrate.
//!
//! Layered as `kernel-side primitives only` per
//! `docs/plan/MICROKERNEL_REFACTOR.md` and `docs/archive/PHASE12_MICROVM.md`:
//! kernel owns VMX/VMCS/EPT/VT-d/VCPU-threads, WASM-Manager owns
//! lifecycle + bridges.
//!
//! As of v0.100.0 the boot path no longer enters VMX root mode —
//! `init()` only probes capabilities. The VMXON/VMCS/EPT/VMLAUNCH
//! pipeline is exercised on demand via the `microvm` shell-intent
//! (`run_substrate_test`), which matches the eventual per-app
//! lifecycle: `microvm <appname>` will spawn a per-app VM, not a
//! single boot-time VM.
//!
//! Phase 12.1 milestones:
//!   12.1.0a   probe + report                          ✓ v0.90.0
//!   12.1.0b   VMXON region + CR4.VMXE + round-trip    ✓ v0.91.0
//!   12.1.0c   VMCS region + VMCLEAR + VMPTRLD         ✓ v0.92.0
//!   12.1.0d-1 Host-state VMWRITE/VMREAD + trampoline  ✓ v0.93.0
//!   12.1.0d-2a TSS install (HOST_TR_SELECTOR ≠ 0)     ✓ v0.94.0
//!   12.1.0d-2b Guest-state + controls + VMLAUNCH      ✓ v0.95.0…0.96.0
//!   12.1.1a   EPT identity-map (1 GB)                 ✓ v0.97.0
//!   12.1.1b   Real-mode unrestricted guest + I/O exit ✓ v0.98.0
//!   12.1.1c-1 Non-identity 16 MB EPT window           ✓ v0.99.0…0.99.1
//!   12.1.1c-2 VMX bring-up off the boot path          ✓ v0.100.x
//!   12.1.1c-3 Alpine bzImage loader + microvm linux   ✓ v0.101…0.127
//!             (Linux booted to rootfs-panic = expected)
//!   12.1.1d   Formal panic detection                  ✓ v0.129.0
//!   12.1.3    initramfs + Rust-PID-1                  ✓ v0.130.0
//!   12.1.4    inject_console round-trip               ✓ v0.137.0
//!   12.1.2    virtio-console backend                  ← next
```

## L34 · `pub mod ept; // demand_fault_in / boot_window_bytes used by guest_mem (B3)`

```
// demand_fault_in / boot_window_bytes used by guest_mem (B3)
```

## L37 · `mod msr; // guest MSR policy: intercept-all + emulation (KVM model)`

```
// guest MSR policy: intercept-all + emulation (KVM model)
```

## L43-44 · `#[derive(Debug, Clone, Copy)]`

```
/// VMX availability as observed at boot. Set by `init()`, never
/// changes afterwards (capabilities are CPUID-fixed).
```

## L54-56 · `pub fn init() {`

```
/// Boot-time probe — no MSR writes, no VMXON. Stores the capability
/// snapshot for later `microvm`-intent invocations and prints a one-
/// shot status line.
```

## L66-67 · `pub fn report() {`

```
/// Print VMX capability snapshot. Used by `init()` once at boot and
/// the `vmx` shell-intent on demand.
```

## L91-93 · `pub fn run_substrate_test() -> Result<vmcs::LaunchOutcome, &'static str> {`

```
/// Run the substrate test (32-bit prot mode HLT-loop OK-stub).
/// Allocates a fresh VMXON region, VMCS, EPT, 64 MB guest RAM,
/// I/O bitmaps (all leaked). Returns the final VM-exit outcome.
```

## L104-105 · `pub fn vm_open(`

```
/// Open a re-entrant VM context (12.4 step 1b). Probe-gated like
/// `run_linux`. The caller drives `run_slice` + `close`.
```

## L112-117 · `crate::tss::ensure_core(crate::smp::per_core::current_core_id());`

```
// Fiber/dedicated mode runs this on a WORKER core, not Core 0. VMX
// rejects HOST_TR_SELECTOR=0 at VM-entry; worker cores keep the boot
// GDT with TR=0 (only the BSP `ltr`'d at boot). Install a private TSS
// on this core first so `write_host_state` captures a valid HOST_TR.
// No-op on the BSP (core 0) and when already installed. AMD never
// reaches here (it opens via svm::vm_open).
```

## L126-130 · `pub fn vm_open_ap(`

```
/// Open an AP vCPU context (guest SMP) sharing the BSP's `VmShared` at
/// `shared_ptr` (a `*mut VmShared` as a u64), entering real mode at
/// `sipi_vector`. Enters VMX root ON THIS (the AP's) worker core. The caller
/// drives `run_slice` then `close_ap` (NOT `close` — the BSP owns the shared
/// state).
```

## L139-142 · `pub fn set_ap_active(on: bool) {`

```
/// Set/clear the guest-SMP big-VM-lock engagement (see `enable::AP_ACTIVE`).
/// Mirror of `svm::set_ap_active` — but VMX needs no idle-park `AP_ESTABLISHED`
/// gate (VMX clears the STI shadow on a HLT exit, so an idle HLT is already
/// interruptible; see the run-loop gate).
```

## L150 · `pub(super) unsafe fn rdmsr(msr: u32) -> u64 {`

```
// ── shared CPU primitives for submodules ───────────────────────────
```

## L152-154 · `pub(super) unsafe fn rdmsr(msr: u32) -> u64 {`

```
/// Read MSR. Caller must guarantee the MSR exists on this CPU,
/// otherwise #GP. All MSRs we touch are architectural since Nehalem
/// or VMX-gated by `probe()`.
```

## L158 · `unsafe {`

```
// SAFETY: caller-guaranteed MSR validity.
```

## L171-172 · `pub(super) unsafe fn wrmsr(msr: u32, val: u64) {`

```
/// Write MSR. Same caveat as `rdmsr`. WRMSR can also fail with #GP if
/// the value violates reserved bits — caller handles that case.
```

## L176 · `unsafe {`

```
// SAFETY: caller-guaranteed MSR + value validity.
```

