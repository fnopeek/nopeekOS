# `kernel/src/microvm/cpu/vmx/probe.rs` @ 5e0102684

## L1-7 · `#[derive(Debug, Clone, Copy)]`

```
//! VMX capability probe.
//!
//! Does NOT enable VMX, does NOT touch CR4. Pure read-side detection.
//! Bring-up (CR4.VMXE, IA32_FEATURE_CONTROL, VMXON) lives in 12.1.0b.
//!
//! Reference: Intel SDM Vol. 3C §23.6 (Discovering Support for VMX),
//! §A.1 (Basic VMX Information).
```

## L9-11 · `#[derive(Debug, Clone, Copy)]`

```
/// VMX capability snapshot returned by `probe()` when the CPU supports
/// virtualization. All fields come from architectural MSRs and are
/// stable across the boot.
```

## L14-16 · `pub revision_id: u32,`

```
/// VMCS revision identifier (IA32_VMX_BASIC[30:0]). Must be the
/// first dword of every VMXON region and every VMCS this CPU
/// loads.
```

## L18-19 · `pub vmxon_region_size: u32,`

```
/// Required size of VMXON / VMCS regions in bytes. Always ≤ 4096
/// per SDM §A.1, but explicit because allocators must honour it.
```

## L21-22 · `pub ept_supported: bool,`

```
/// IA32_VMX_EPT_VPID_CAP MSR is readable (i.e. secondary
/// processor-based controls expose EPT). Required for Phase 12.1.1+.
```

## L24-26 · `pub unrestricted_guest: bool,`

```
/// Unrestricted-guest secondary control available — lets the guest
/// run in real mode without trampolining through paged 32-bit.
/// Phase 12.1.1+ Linux boot benefits from this.
```

## L28-29 · `pub vpid: bool,`

```
/// VPID (tagged TLB) available. Reduces TLB flush cost on
/// VM-entry/exit. Required for sane Phase 12.6 multi-VM density.
```

## L33-34 · `pub fn probe() -> Option<Capabilities> {`

```
/// Probe the running CPU for VMX support. Returns `None` if VMX is
/// either absent or fused off in firmware. Side-effect-free.
```

## L40-41 · `let feat_ctrl = unsafe { super::rdmsr(IA32_FEATURE_CONTROL) };`

```
// IA32_FEATURE_CONTROL gates VMXON. Bit 2 (VMX outside SMX) must
// be set AND bit 0 (lock) decides whether we can still toggle it.
```

## L46 · `return None;`

```
// Firmware locked us out of VMX. Surface cleanly.
```

## L65 · `const IA32_FEATURE_CONTROL: u32 = 0x3A;`

```
// ── private helpers ────────────────────────────────────────────────
```

## L75 · `fn cpuid_vmx_bit() -> bool {`

```
/// CPUID.1:ECX[5] — VMX present.
```

## L78-79 · `unsafe {`

```
// SAFETY: CPUID has no privileged side-effects. ebx is preserved
// explicitly because Rust reserves it for LLVM internals.
```

## L94-96 · `fn secondary_caps() -> (bool, bool, bool) {`

```
/// Decode the secondary-controls-allowed bitmap to surface the three
/// MicroVM-relevant feature flags. SDM §A.3.3 specifies the layout:
/// each capability MSR has its allowed-1 bits in the upper dword.
```

## L98-100 · `let prim = unsafe { super::rdmsr(IA32_VMX_PROCBASED_CTLS) };`

```
// Step 1: confirm secondary controls themselves are exposed.
// IA32_VMX_PROCBASED_CTLS[63] = "activate secondary controls"
// allowed-1 (bit 63 of the 64-bit MSR = bit 31 of the upper dword).
```

## L107 · `let sec = unsafe { super::rdmsr(IA32_VMX_PROCBASED_CTLS2) };`

```
// Step 2: read secondary capabilities. Allowed-1 bits in upper 32.
```

