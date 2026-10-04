# `kernel/src/microvm/cpu/guest_msr.rs` @ 5e0102684

## L1-7 · `pub type MsrResult<T> = Result<T, ()>;`

```
//! Vendor-neutral half of guest MSR emulation — KVM `kvm_get_msr_common` /
//! `kvm_set_msr_common` and `kvm_mtrr_*`. `svm::msr` and `vmx::msr` handle
//! their vendor's MSRs and fall through to `read`/`write` here.
//!
//! Every MSR a guest reaches through here is intercepted. Nothing it writes
//! lands in a host MSR: unknown MSRs read 0 and swallow writes (KVM with
//! `ignore_msrs`), so the host keeps its MTRRs, TSC, CET and APIC state.
```

## L9 · `pub type MsrResult<T> = Result<T, ()>;`

```
/// Result of an emulated access: `Err(())` = inject #GP.
```

## L40 · `const MTRR_FIXED: [u32; 11] = [`

```
/// Fixed-range MTRRs, in the order KVM's `fixed_msr_to_seg_unit` uses.
```

## L44 · `const MTRR_CAP_VALUE: u64 = 0x508;`

```
/// MTRRcap: 8 variable ranges, fixed ranges, WC — KVM's `KVM_NR_VAR_MTRR | 0x500`.
```

## L46-48 · `const MTRR_DEF_TYPE_RESET: u64 = 0x806;`

```
/// No firmware runs in the guest, so start where a BIOS leaves it: MTRRs on,
/// default type WB, no ranges. The guest's MTRRs never reach hardware — with
/// NPT/EPT the memory type comes from the host tables and the guest PAT.
```

## L51 · `pub const HWCR_TSC_FREQ_SEL: u64 = 1 << 24;`

```
/// HWCR.TscFreqSel (AMD): the TSC counts at P0 — true with invariant TSC.
```

## L53-55 · `const MISC_ENABLE_RESET: u64 = (1 << 0) | (1 << 11) | (1 << 12);`

```
/// IA32_MISC_ENABLE (Intel): fast strings on, BTS/PEBS unavailable — KVM's
/// reset value plus the fast-string bit every BIOS sets (without it Linux
/// drops ERMS and REP_GOOD).
```

## L58 · `pub struct GuestMsrs {`

```
/// Per-vCPU emulated MSR state (both vendors; unused fields stay at reset).
```

## L93 · `unsafe {`

```
// SAFETY: callers pass only MSRs architectural on the running vendor.
```

## L102 · `unsafe {`

```
// SAFETY: caller guarantees msr/v are valid on this CPU.
```

## L109 · `pub fn read(st: &GuestMsrs, msr: u32, apic_id: u8, lapic_on: bool, spec_valid: u64) -> MsrResult<u64> {`

```
/// Common RDMSR. `spec_valid` = the vendor's SPEC_CTRL bits (0 = no MSR).
```

## L113-114 · `let mut v = crate::microvm::cpu::svm::lapic::APIC_BASE_MSR_VALUE;`

```
// BSP bit (8) only for the boot vCPU — Linux's topology code
// otherwise mistakes the guest for a kdump kernel and caps it at 1 CPU.
```

## L125 · `MSR_PRED_CMD => return Err(()), // write-only`

```
// write-only
```

## L134 · `MSR_MCG_CAP => 0,`

```
// No machine-check banks (KVM with mcg_cap = 0).
```

## L139 · `MSR_XSS => 0,`

```
// No supervisor XSAVE states.
```

## L145 · `pub fn write(st: &mut GuestMsrs, msr: u32, val: u64, spec_valid: u64) -> MsrResult<()> {`

```
/// Common WRMSR.
```

## L148 · `MSR_APIC_BASE => {} // fixed base — the trap page sits at 0xFEE00000`

```
// fixed base — the trap page sits at 0xFEE00000
```

## L149 · `MSR_TSC_ADJUST => st.tsc_adjust = val,`

```
// Stored only; the guest's TSC offset stays ours.
```

## L178 · `fn unknown(msr: u32, write: Option<u64>) -> u64 {`

```
/// Capped log of MSRs nobody emulates yet — the list to work through.
```

## L191-192 · `pub fn spec_ctrl_enter(guest: u64) -> Option<u64> {`

```
/// Load the guest's SPEC_CTRL before VM entry; returns the host value to
/// restore. KVM without V_SPEC_CTRL does the same swap (`x86_spec_ctrl_set_guest`).
```

## L197 · `unsafe { host_wrmsr(MSR_SPEC_CTRL, guest); }`

```
// SAFETY: guest was validated against the host's SPEC_CTRL bits in `write`.
```

## L204 · `unsafe { host_wrmsr(MSR_SPEC_CTRL, h); }`

```
// SAFETY: restoring the value read from this very MSR before entry.
```

