# `kernel/src/microvm/cpu/guest_cpuid.rs` @ 5e0102684

## L1-7 · `fn host_cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {`

```
//! Guest CPUID — an allowlist over the host's answer, after KVM's
//! `kvm_cpu_cap_init` and `__do_cpuid_func`.
//!
//! Passing host CPUID through with a few bits cleared told the guest about
//! hardware we do not virtualize: SMCA machine-check banks, the AMD extended
//! APIC space, IBS, the PMU, SVM itself. Each of those makes Linux touch an
//! MSR or APIC register that either reaches the host or is not there.
```

## L17-18 · `const MAX_EXT_LEAF: u32 = 0x8000_0021;`

```
/// Highest extended leaf we answer (KVM's table ends at 0x8000_0021 for AMD
/// features we can back).
```

## L23-24 · `unsafe {`

```
// SAFETY: XGETBV(0) is valid whenever CR4.OSXSAVE=1, which the kernel
// sets at boot (it saves FPU state with XSAVE).
```

## L39-41 · `const EXT1_ECX: u32 = bits(&[0, 1, 4, 5, 6, 7, 8, 11, 16, 21]);`

```
/// 0x8000_0001 ECX: LAHF_LM, CMP_LEGACY, CR8_LEGACY, ABM, SSE4A, MISALIGNSSE,
/// 3DNOWPREFETCH, XOP, FMA4, TBM. Dropped vs KVM: SVM (no nested), OSVW,
/// TOPOEXT (we give no 0x8000_001E) and PERFCTR_CORE (no vPMU).
```

## L43-44 · `const EXT1_EDX: u32 = bits(&[`

```
/// 0x8000_0001 EDX: the leaf-1 aliases plus SYSCALL, NX, MMXEXT, FXSR_OPT,
/// GBPAGES, RDTSCP, LM, 3DNOWEXT, 3DNOW.
```

## L49-51 · `const EXT8_EBX: u32 = bits(&[0, 2, 9, 12, 14, 15, 17, 24, 26, 28, 29, 30]);`

```
/// 0x8000_0008 EBX: CLZERO, XSAVEERPTR, WBNOINVD, IBPB, IBRS, STIBP,
/// STIBP_ALWAYS_ON, SSBD, SSB_NO, PSFD, BTC_NO, IBPB_RET. Not VIRT_SSBD (its
/// MSR is not emulated) and not RDPRU (intercepted, #UD).
```

## L53-54 · `const EXT21_EAX: u32 = bits(&[0, 2, 6, 8, 9, 27, 28, 29]);`

```
/// 0x8000_0021 EAX: NO_NESTED_DATA_BP, LFENCE_RDTSC, NULL_SEL_CLR_BASE,
/// AUTOIBRS, NO_SMM_CTL_MSR, SBPB, IBPB_BRTYPE, SRSO_NO.
```

## L57-62 · `const L1_ECX_DROP: u32 = bits(&[3, 5, 6, 7, 8, 10, 14, 18]);`

```
/// Leaf 1 ECX bits never shown: MONITOR, VMX, SMX, EST, TM2, CNXT-ID, xTPR,
/// DCA. X2APIC, TSC_DEADLINE and HYPERVISOR are set by us, not taken from the
/// host: they describe the emulated platform. TSC_DEADLINE (MSR 0x6E0, the
/// emulated LAPIC) is what KVM guests clock from — without it Linux calibrates
/// the LAPIC timer against the PIT, and through the I/O APIC that fails
/// ("APIC timer disabled due to verification failure": no hrtimers at all).
```

## L68-72 · `const KVM_CPUID_SIGNATURE: u32 = 0x4000_0000;`

```
/// KVM paravirt leaves (`KVM_CPUID_SIGNATURE`, `KVM_CPUID_FEATURES`). The
/// guest's Linux then takes the KVM paths we back: PV EOI (its EOI is a flag
/// in its memory, no exit), PV send-IPI (one hypercall for a set of vCPUs),
/// NOP io_delay, and x2APIC without interrupt remapping (`kvm_para_available`
/// is `x2apic_available`). Nothing else — no kvmclock, steal time, async PF.
```

## L78 · `const L1_EDX_DROP: u32 = bits(&[18, 21, 22, 29, 30, 31]);`

```
/// Leaf 1 EDX bits never shown: PSN, DS, ACPI, TM, IA64, PBE.
```

## L80 · `const INTEL_L1_ECX_DROP: u32 = bits(&[11, 15]);`

```
/// Intel only, leaf 1 ECX: SDBG (IA32_DEBUG_INTERFACE), PDCM (PERF_CAPABILITIES).
```

## L82-84 · `const INTEL_L7_EBX_DROP: u32 = bits(&[14]);`

```
/// Intel only, leaf 7.0: MPX (its BND state is not in our XCR0), ENQCMD
/// (PASID MSR), SGX_LC, Key Locker, PCONFIG, ARCH_LBR, CORE_CAPABILITIES
/// (split-lock MSR).
```

## L88-89 · `const INTEL_L7_1_EAX_DROP: u32 = bits(&[17, 18, 26]);`

```
/// Intel only, leaf 7.1 EAX: FRED, LKGS, LAM — each changes entry/paging
/// semantics we do not virtualize.
```

## L92 · `fn xstate_size(xfeatures: u64, compacted: bool) -> u32 {`

```
/// Size of an XSAVE area for `xfeatures` (KVM `xstate_required_size`).
```

## L122 · `c = (c & !(1 << 27)) | ((((guest_cr4 >> 18) & 1) as u32) << 27);`

```
// OSXSAVE mirrors the GUEST's CR4, not the host's.
```

## L125 · `b = (b & 0x00FF_FFFF) | ((apic_id as u32) << 24);`

```
// Initial APIC ID = this vCPU, matching the emulated LAPIC and MP table.
```

## L128-129 · `5 | 0xA | 0xF | 0x10 | 0x12 | 0x14 | 0x19 | 0x23 => return (0, 0, 0, 0),`

```
// MONITOR/MWAIT hidden; Intel perfmon; RDT; SGX; PT.
// Key Locker; architectural perfmon extension.
```

## L131 · `6 => return (4, 0, 0, 0),`

```
// Thermal/power: only ARAT (KVM).
```

## L134 · `b &= !bits(&[2, 12, 15, 25]); // SGX, PQM, PQE, Intel PT`

```
// SGX, PQM, PQE, Intel PT
```

## L135 · `c &= !bits(&[3, 4, 5, 7]);`

```
// WAITPKG; PKU/OSPKE (PKRU would change the XSAVE layout); CET_SS.
```

## L137 · `d &= !(1 << 20); // CET_IBT — Linux's asm stubs lack ENDBR64`

```
// CET_IBT — Linux's asm stubs lack ENDBR64
```

## L145 · `0xB | 0x1F => d = apic_id as u32,`

```
// Extended topology: EDX is the x2APIC ID → this vCPU's.
```

## L157-159 · `if intel { a &= 0x7; }`

```
// Intel: XSAVEOPT/XSAVEC/XGETBV1 only. VMX does not switch
// IA32_XSS (XSAVES would run with the host's value) and
// XFD belongs to AMX, which is not in our XCR0.
```

## L161 · `b = xstate_size(guest_xcr0, true);`

```
// No supervisor states (IA32_XSS stays 0).
```

## L170 · `KVM_CPUID_SIGNATURE => return (KVM_CPUID_FEATURES, 0x4b4d_564b, 0x564b_4d56, 0x4d),`

```
// "KVMKVMKVM\0\0\0"
```

## L178 · `0x8000_0007 => return (0, 0, 0, d & (1 << 8)),`

```
// Only invariant TSC.
```

## L181 · `0x8000_000A | 0x8000_001E | 0x8000_001F | 0x8000_0020 => return (0, 0, 0, 0),`

```
// SVM features, topology, SEV, RDT-A: none of it is ours to give.
```

