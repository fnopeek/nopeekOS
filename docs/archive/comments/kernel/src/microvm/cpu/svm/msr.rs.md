# `kernel/src/microvm/cpu/svm/msr.rs` @ 5e0102684

## L1-7 · `use super::vmcb;`

```
//! Guest MSR policy, SVM half — KVM's `svm_recalc_msr_intercepts` and
//! `svm_get_msr`/`svm_set_msr`. Vendor-neutral MSRs live in `cpu::guest_msr`.
//!
//! Every MSR is intercepted. Pass-through is limited to what the CPU switches
//! for us (VMLOAD/VMSAVE state), TSC_AUX (the host never reads it) and the
//! write-only barrier PRED_CMD. With an all-zero MSRPM a guest could rewrite
//! the host's MTRRs, TSC, SYSCFG or microcode loader.
```

## L20 · `const MSR_ZEN2_SPECTRAL_CHICKEN: u32 = 0xC001_10E3;`

```
/// Host-owned chicken bit (see `cpu_errata`); the guest's write stays here.
```

## L22 · `const MSR_K7_PERF_FIRST: u32 = 0xC001_0000;`

```
/// Legacy K7 perf counters (EVNTSEL0-3, PERFCTR0-3) and the core extension.
```

## L38-41 · `pub unsafe fn init_msrpm(msrpm_phys: u64) {`

```
/// Fill an 8 KB MSRPM: intercept everything, then open the pass-through set.
///
/// # Safety
/// `msrpm_phys` must be an exclusively owned, identity-mapped 8 KB region.
```

## L43 · `unsafe { core::ptr::write_bytes(msrpm_phys as *mut u8, 0xFF, 2 * 4096); }`

```
// SAFETY: caller guarantees ownership of the 8 KB region.
```

## L45 · `for msr in [`

```
// Saved/loaded by VMSAVE/VMLOAD around every VMRUN (run_guest_once).
```

## L50 · `MSR_TSC_AUX,`

```
// Only RDTSCP/RDPID read it; the host kernel uses neither.
```

## L53 · `unsafe { set_intercept(msrpm_phys, msr, false, false); }`

```
// SAFETY: as above.
```

## L57 · `unsafe { set_intercept(msrpm_phys, MSR_PRED_CMD, true, false); }`

```
// SAFETY: as above. Write-only barrier; reads stay intercepted (#GP).
```

## L62 · `unsafe fn set_intercept(msrpm_phys: u64, msr: u32, read: bool, write: bool) {`

```
/// APM Vol 2 §15.11: three 2 KB ranges, two bits per MSR (read, write).
```

## L68 · `_ => return, // outside the map: always intercepted`

```
// outside the map: always intercepted
```

## L73 · `unsafe {`

```
// SAFETY: byte lies inside the caller-owned 8 KB MSRPM.
```

## L91 · `fn spec_ctrl_valid_bits() -> u64 {`

```
/// SPEC_CTRL bits the host CPU implements (KVM `kvm_spec_ctrl_valid_bits`).
```

## L94 · `if host_ext_8(14) { bits |= 1 << 0; } // IBRS`

```
// IBRS
```

## L95 · `if host_ext_8(15) { bits |= 1 << 1; } // STIBP`

```
// STIBP
```

## L96 · `if host_ext_8(24) { bits |= 1 << 2; } // SSBD`

```
// SSBD
```

## L97 · `if host_ext_8(28) { bits |= 1 << 7; } // PSFD`

```
// PSFD
```

## L112 · `pub fn read(st: &GuestMsrs, vmcb: &vmcb::Vmcb, apic_id: u8, msr: u32) -> MsrResult<u64> {`

```
/// Emulated RDMSR. `apic_id` decides the APIC_BASE BSP bit.
```

## L120 · `MSR_EFER => vmcb.read_u64(vmcb::OFF_SAVE_EFER) & !EFER_SVME,`

```
// Guest view: SVME is ours, not the guest's (KVM `svm_set_efer`).
```

## L123 · `MSR_SYSCFG | MSR_NB_CFG | MSR_CPUID_7_FEATURES | MSR_ZEN2_SPECTRAL_CHICKEN => 0,`

```
// KVM answers these with 0: no SME/SEV, no NB config, no PMU (enable_pmu=0).
```

## L126 · `MSR_DE_CFG => host_rdmsr(MSR_DE_CFG) & (1 << 1),`

```
// Feature MSR: only the LFENCE-serialising bit (KVM `kvm_get_feature_msr`).
```

## L134 · `pub fn write(st: &mut GuestMsrs, vmcb: &mut vmcb::Vmcb, msr: u32, val: u64) -> MsrResult<()> {`

```
/// Emulated WRMSR.
```

## L137 · `MSR_TSC => {}`

```
// The guest does not own the TSC offset; Linux never writes it.
```

## L146 · `let cur = vmcb.read_u64(vmcb::OFF_SAVE_EFER);`

```
// LMA is the CPU's to set; SVME must stay on for VMRUN (APM §15.5.1).
```

