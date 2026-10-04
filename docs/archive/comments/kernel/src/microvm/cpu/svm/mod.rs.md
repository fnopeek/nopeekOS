# `kernel/src/microvm/cpu/svm/mod.rs` @ 5e0102684

## L1-30 · `mod enable;`

```
//! SVM (AMD-V) — Phase 12 MicroVM substrate, AMD backend.
//!
//! Status: 12.1.0a-svm — probe + report. Bring-up
//! (EFER.SVME, host-save, VMCB, VMRUN) lands in 12.1.0b-svm.
//!
//! The AMD equivalent of Intel VMX is documented in *AMD64
//! Architecture Programmer's Manual, Volume 2: System Programming*
//! Chapter 15 ("Secure Virtual Machine"). Mapping vs. VMX:
//!
//! | Concept              | Intel VMX        | AMD SVM             |
//! |----------------------|------------------|---------------------|
//! | Enable bit           | CR4.VMXE         | EFER.SVME           |
//! | Per-VM control struct| VMCS (4 KB, opaque, accessed via VMREAD/VMWRITE) | VMCB (4 KB, normal struct, MMIO-style) |
//! | Host save area       | Implicit (VMCS host-state region) | Host-save MSR (VM_HSAVE_PA) |
//! | Enter guest          | VMLAUNCH / VMRESUME | VMRUN |
//! | Exit reason          | 32-bit field, encoded in VMCS | VMCB.EXITCODE u64 |
//! | Nested paging        | EPT (4-level)    | NPT (4-level, same shape, different MSR) |
//! | I/O intercept        | I/O bitmap (2×4 KB) | IOPM (12 KB, ports 0..0xFFFF) |
//! | MSR intercept        | MSR bitmap (4 KB) | MSRPM (8 KB) |
//!
//! Phase 12.1 milestones (mirroring VMX bring-up):
//!   12.1.0a-svm  probe + report                          ← this
//!   12.1.0b-svm  EFER.SVME + host-save + trivial VMRUN
//!   12.1.0c/d-svm  VMCB save-area complete, host VMSAVE/VMLOAD
//!   12.1.1a-svm  NPT identity-map (256 MB)
//!   12.1.1b-svm  Real-mode unrestricted guest + I/O bitmap
//!   12.1.1c-svm  Linux bzImage 32-bit boot protocol entry
//!   12.1.1d-svm  Panic detection (shared SerialState scanner)
//!   12.1.3-svm   initramfs + Rust-PID-1 (init crate already exists)
//!   12.1.4-svm   inject_console echo round-trip
```

## L33 · `mod msr; // guest MSR policy: intercept-all + emulation (KVM model)`

```
// guest MSR policy: intercept-all + emulation (KVM model)
```

## L34 · `pub mod lapic; // per-vCPU local-APIC emulation (guest-SMP Stage 1)`

```
// per-vCPU local-APIC emulation (guest-SMP Stage 1)
```

## L35 · `pub mod npt; // demand_fault_in / boot_window_bytes used by guest_mem (B3)`

```
// demand_fault_in / boot_window_bytes used by guest_mem (B3)
```

## L43-44 · `#[derive(Debug, Clone, Copy)]`

```
/// SVM availability as observed at boot. Set by `init()`, never
/// changes afterwards (capabilities are CPUID-fixed).
```

## L54-56 · `pub fn init() {`

```
/// Boot-time probe — no MSR writes, no SVME. Stores the capability
/// snapshot for later `microvm`-intent invocations and prints a one-
/// shot status line.
```

## L102-103 · `pub fn vm_open(`

```
/// Open a re-entrant VM context (12.4 step 1b). Probe-gated like
/// `run_linux`. The caller drives `run_slice` + `close`.
```

## L117-120 · `pub fn vm_open_ap(`

```
/// Open an AP vCPU context (guest SMP, Stage 3b) sharing the BSP's
/// `VmShared` at `shared_ptr` (a `*mut VmShared` as a u64), entering real
/// mode at `sipi_vector`. The caller drives `run_slice` (NOT `close` — the
/// BSP owns the shared state).
```

## L129 · `pub fn set_ap_active(on: bool) {`

```
/// Set/clear the guest-SMP big-VM-lock engagement (see `enable::AP_ACTIVE`).
```

## L134 · `pub(super) unsafe fn rdmsr(msr: u32) -> u64 {`

```
// ── shared CPU primitives for SVM submodules ───────────────────────
```

## L136-139 · `pub(super) unsafe fn rdmsr(msr: u32) -> u64 {`

```
/// Read MSR. Caller must guarantee the MSR exists on this CPU,
/// otherwise #GP. Mirrors `vmx::rdmsr` — both backends need the
/// same primitive but vendor isolation keeps each tree self-
/// contained.
```

## L143 · `unsafe {`

```
// SAFETY: caller-guaranteed MSR validity.
```

## L156-157 · `#[allow(dead_code)] // 12.1.0b will call this for VM_HSAVE_PA + EFER`

```
/// Write MSR. Same caveat as `rdmsr`. WRMSR can fail with #GP if
/// the value violates reserved bits — caller handles that case.
```

## L158 · `#[allow(dead_code)] // 12.1.0b will call this for VM_HSAVE_PA + EFER`

```
// 12.1.0b will call this for VM_HSAVE_PA + EFER
```

## L162 · `unsafe {`

```
// SAFETY: caller-guaranteed MSR + value validity.
```

## L174-176 · `pub(super) fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {`

```
/// CPUID with explicit subleaf. Returns (eax, ebx, ecx, edx).
/// Rust reserves rbx for LLVM internals so we save/restore it
/// manually. CPUID has no privileged side-effects.
```

## L182 · `unsafe {`

```
// SAFETY: CPUID is unprivileged.
```

