# `kernel/src/microvm/cpu/svm/probe.rs` @ 5e0102684

## L1-8 · `#[derive(Debug, Clone, Copy)]`

```
//! SVM capability probe.
//!
//! Does NOT enable SVM, does NOT touch EFER. Pure read-side detection.
//! Bring-up (EFER.SVME, host-save area, VM_HSAVE_PA MSR) lives in
//! 12.1.0b-svm (`enable.rs`).
//!
//! Reference: AMD APM Vol. 2, §15.4 (Enabling SVM) and §15.3
//! (CPUID Function 8000_000Ah).
```

## L10-12 · `#[derive(Debug, Clone, Copy)]`

```
/// SVM capability snapshot returned by `probe()` when AMD-V is
/// available. All fields come from CPUID + the VM_CR MSR and are
/// stable across the boot.
```

## L15-17 · `pub revision: u8,`

```
/// SVM revision identifier (CPUID 8000_000A EAX[7:0]). Tracks
/// the ISA generation; rev 1 = original Pacifica, rev 2+ adds
/// Decode Assists, NP, NRIPS, etc.
```

## L19-21 · `pub asid_count: u32,`

```
/// Number of address-space identifiers supported (CPUID
/// 8000_000A EBX). VM_CB.guest_asid is masked to this count.
/// Need ≥ 1 (we use ASID 1 for our single guest).
```

## L23-26 · `pub nested_paging: bool,`

```
/// Nested paging available (CPUID 8000_000A EDX[0]). AMD's
/// equivalent of Intel EPT — required for our 256 MB guest-RAM
/// window. nopeekOS bails if this is false; supported on every
/// AMD CPU since Barcelona/K10 (2007).
```

## L28-32 · `pub nrip_save: bool,`

```
/// Next-RIP save (CPUID 8000_000A EDX[3]). On VM-exit the CPU
/// stores the address of the instruction *following* the
/// faulting one in VMCB.NRIP, sparing us from manually decoding
/// instruction lengths. nopeekOS uses this throughout the
/// I/O-exit / CPUID-exit handlers.
```

## L34-36 · `pub vmsave_vmload: bool,`

```
/// VMSave/VMLoad virtualization (CPUID 8000_000A EDX[15]).
/// Allows the guest to run VMSAVE/VMLOAD without a #VMEXIT —
/// useful for nested guests but not strictly required.
```

## L38-40 · `pub decode_assists: bool,`

```
/// Decode Assists (CPUID 8000_000A EDX[7]). When set, the CPU
/// populates VMCB exit-info fields with decoded operand info,
/// avoiding instruction-stream re-fetch on string I/O exits.
```

## L42-48 · `pub avic: bool,`

```
/// AVIC — Advanced Virtual Interrupt Controller (CPUID 8000_000A
/// EDX[13]). Hardware delivers guest→guest IPIs (incl. TLB-shootdown
/// IPIs) directly into the target vCPU's vAPIC with NO #VMEXIT when the
/// target is running — exactly the `csd_lock_wait` fix. CRITICAL for us:
/// nopeekOS runs nested under KVM, so this bit is only set if KVM
/// virtualizes AVIC for its guest (nested AVIC). If false here, AVIC is
/// bare-metal-only and not worth building for the QEMU path.
```

## L52-54 · `pub fn probe() -> Option<Capabilities> {`

```
/// Probe the running CPU for SVM. Returns `None` if SVM is either
/// absent (non-AMD or pre-Pacifica), or fused off in firmware via
/// VM_CR.SVMDIS with VM_CR.SVMLOCK set. Side-effect-free.
```

## L60-62 · `let vm_cr = unsafe { super::rdmsr(VM_CR) };`

```
// VM_CR (0xC001_0114) — bit 4 SVMDIS, bit 3 SVMLOCK.
// If SVMDIS=1 and the lock bit (CPUID 8000_000A EDX[2] SVM-Lock)
// is set, BIOS sealed SVM off. Surface cleanly.
```

## L86 · `const VM_CR: u32 = 0xC001_0114;`

```
// ── private constants ──────────────────────────────────────────────
```

## L91 · `fn cpuid_svm_bit() -> bool {`

```
/// CPUID 8000_0001 ECX[2] — SVM present.
```

