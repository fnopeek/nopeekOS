# `kernel/src/microvm/cpu/vmx/msr.rs` @ 5e0102684

## L1-9 · `use crate::microvm::cpu::guest_msr::{self as common, *};`

```
//! Guest MSR policy, VMX half — KVM's `vmx_possible_passthrough_msrs` and
//! `vmx_get_msr`/`vmx_set_msr`. Vendor-neutral MSRs live in `cpu::guest_msr`.
//!
//! Every MSR is intercepted except those the CPU switches through VMCS fields
//! (FS/GS base, SYSENTER, EFER), the SYSCALL set and TSC_AUX (the host kernel
//! uses neither SYSCALL/SWAPGS nor RDTSCP/RDPID, so the guest's values are the
//! only ones that matter on its core), and the write-only barriers. An all-zero
//! bitmap let the guest write IA32_S_CET (the host's IBT), the x2APIC ICR
//! (host IPIs) and PAT (host memory types).
```

## L28 · `const MSR_CET_FIRST: u32 = 0x6A0;`

```
/// CET: U_CET, S_CET, PL0-3_SSP, INT_SSP_TAB. S_CET carries the host's IBT.
```

## L34 · `fn is_pmu(msr: u32) -> bool {`

```
/// Architectural PMU: PMCs, event selects, fixed counters, global control.
```

## L39-40 · `const ARCH_CAP_MASK: u64 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6)`

```
/// ARCH_CAPABILITIES bits describing the hardware that a guest may see
/// (KVM `KVM_SUPPORTED_ARCH_CAP` minus TSX_CTRL, whose MSR is not emulated).
```

## L56 · `fn spec_ctrl_valid_bits() -> u64 {`

```
/// SPEC_CTRL bits the host CPU implements (KVM `kvm_spec_ctrl_valid_bits`).
```

## L59 · `if l7_edx(26) { bits |= 1 << 0; }                  // IBRS`

```
// IBRS
```

## L60 · `if l7_edx(27) { bits |= 1 << 1; }                  // STIBP`

```
// STIBP
```

## L61 · `if l7_edx(31) { bits |= 1 << 2; }                  // SSBD`

```
// SSBD
```

## L62 · `if l7_2_edx(0) { bits |= 1 << 7; }                 // PSFD`

```
// PSFD
```

## L63 · `if l7_2_edx(1) { bits |= (1 << 3) | (1 << 4); }    // IPRED_DIS_U/S`

```
// IPRED_DIS_U/S
```

## L64 · `if l7_2_edx(2) { bits |= (1 << 5) | (1 << 6); }    // RRSBA_DIS_U/S`

```
// RRSBA_DIS_U/S
```

## L65 · `if l7_2_edx(4) { bits |= 1 << 10; }                // BHI_DIS_S`

```
// BHI_DIS_S
```

## L69-72 · `pub unsafe fn init_msr_bitmap(bitmap_phys: u64) {`

```
/// Fill the 4 KB MSR bitmap: intercept everything, then open the pass-through set.
///
/// # Safety
/// `bitmap_phys` must be an exclusively owned, identity-mapped 4 KB frame.
```

## L74 · `unsafe { core::ptr::write_bytes(bitmap_phys as *mut u8, 0xFF, 4096); }`

```
// SAFETY: caller guarantees ownership of the 4 KB frame.
```

## L77 · `MSR_FS_BASE, MSR_GS_BASE, MSR_SYSENTER_CS, MSR_SYSENTER_ESP, MSR_SYSENTER_EIP,`

```
// Switched by the CPU through VMCS guest/host fields.
```

## L80 · `MSR_KERNEL_GS_BASE, MSR_STAR, MSR_LSTAR, MSR_CSTAR, MSR_SFMASK, MSR_TSC_AUX,`

```
// Not switched, and not used by the host (see module doc).
```

## L83 · `unsafe { set_intercept(bitmap_phys, msr, false, false); }`

```
// SAFETY: as above.
```

## L86 · `if l7_edx(26) {`

```
// Write-only barriers; reads stay intercepted (#GP).
```

## L88 · `unsafe { set_intercept(bitmap_phys, MSR_PRED_CMD, true, false); }`

```
// SAFETY: as above.
```

## L92 · `unsafe { set_intercept(bitmap_phys, MSR_FLUSH_CMD, true, false); }`

```
// SAFETY: as above.
```

## L97-98 · `unsafe fn set_intercept(bitmap_phys: u64, msr: u32, read: bool, write: bool) {`

```
/// SDM §25.6.9: read-low @0x000, read-high @0x400, write-low @0x800,
/// write-high @0xC00; one bit per MSR.
```

## L103 · `_ => return, // outside the map: always intercepted`

```
// outside the map: always intercepted
```

## L107 · `unsafe {`

```
// SAFETY: both bytes lie inside the caller-owned 4 KB bitmap.
```

## L116 · `pub fn read(st: &GuestMsrs, apic_id: u8, lapic_on: bool, msr: u32) -> MsrResult<u64> {`

```
/// Emulated RDMSR.
```

## L121 · `MSR_FEATURE_CONTROL => 1,`

```
// Locked, VMX off — the guest has no VMX (KVM without nested).
```

## L133 · `MSR_CET_FIRST..=MSR_CET_LAST | MSR_X2APIC_FIRST..=MSR_X2APIC_LAST => return Err(()),`

```
// CET and x2APIC are hidden in CPUID; real hardware faults here too.
```

## L139 · `pub fn write(st: &mut GuestMsrs, msr: u32, val: u64) -> MsrResult<()> {`

```
/// Emulated WRMSR.
```

## L147 · `MSR_FEATURE_CONTROL => return Err(()), // locked`

```
// locked
```

