//! Guest CPUID — an allowlist over the host's answer, after KVM's
//! `kvm_cpu_cap_init` and `__do_cpuid_func`.
//!
//! An allowlist rather than host CPUID with a few bits cleared: passing
//! host CPUID through would advertise hardware we do not virtualize (SMCA
//! machine-check banks, the AMD extended APIC space, IBS, the PMU, SVM
//! itself), and each of those makes Linux touch an MSR or APIC register
//! that either reaches the host or is not there.

fn host_cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let r = core::arch::x86_64::__cpuid_count(leaf, subleaf);
    (r.eax, r.ebx, r.ecx, r.edx)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Vendor { Amd, Intel }

/// Highest extended leaf we answer (KVM's table ends at 0x8000_0021 for AMD
/// features we can back).
const MAX_EXT_LEAF: u32 = 0x8000_0021;

pub fn host_xcr0() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: XGETBV(0) is valid whenever CR4.OSXSAVE=1, which the kernel
    // sets at boot (it saves FPU state with XSAVE).
    unsafe {
        core::arch::asm!("xgetbv", in("ecx") 0u32, out("eax") lo, out("edx") hi,
                         options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

const fn bits(list: &[u32]) -> u32 {
    let mut m = 0;
    let mut i = 0;
    while i < list.len() { m |= 1 << list[i]; i += 1; }
    m
}

/// 0x8000_0001 ECX: LAHF_LM, CMP_LEGACY, CR8_LEGACY, ABM, SSE4A, MISALIGNSSE,
/// 3DNOWPREFETCH, XOP, FMA4, TBM. Dropped vs KVM: SVM (no nested), OSVW,
/// TOPOEXT (we give no 0x8000_001E) and PERFCTR_CORE (no vPMU).
const EXT1_ECX: u32 = bits(&[0, 1, 4, 5, 6, 7, 8, 11, 16, 21]);
/// 0x8000_0001 EDX: the leaf-1 aliases plus SYSCALL, NX, MMXEXT, FXSR_OPT,
/// GBPAGES, RDTSCP, LM, 3DNOWEXT, 3DNOW.
const EXT1_EDX: u32 = bits(&[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17, 20, 22, 23, 24,
    25, 26, 27, 29, 30, 31,
]);
/// 0x8000_0008 EBX: CLZERO, XSAVEERPTR, WBNOINVD, IBPB, IBRS, STIBP,
/// STIBP_ALWAYS_ON, SSBD, SSB_NO, PSFD, BTC_NO, IBPB_RET. Not VIRT_SSBD (its
/// MSR is not emulated) and not RDPRU (intercepted, #UD).
const EXT8_EBX: u32 = bits(&[0, 2, 9, 12, 14, 15, 17, 24, 26, 28, 29, 30]);
/// 0x8000_0021 EAX: NO_NESTED_DATA_BP, LFENCE_RDTSC, NULL_SEL_CLR_BASE,
/// AUTOIBRS, NO_SMM_CTL_MSR, SBPB, IBPB_BRTYPE, SRSO_NO.
const EXT21_EAX: u32 = bits(&[0, 2, 6, 8, 9, 27, 28, 29]);

/// Leaf 1 ECX bits never shown: MONITOR, VMX, SMX, EST, TM2, CNXT-ID, xTPR,
/// DCA. X2APIC, TSC_DEADLINE and HYPERVISOR are set by us, not taken from the
/// host: they describe the emulated platform. TSC_DEADLINE (MSR 0x6E0, the
/// emulated LAPIC) is what KVM guests clock from — without it Linux calibrates
/// the LAPIC timer against the PIT, and through the I/O APIC that fails
/// ("APIC timer disabled due to verification failure": no hrtimers at all).
const L1_ECX_DROP: u32 = bits(&[3, 5, 6, 7, 8, 10, 14, 18]);
const L1_ECX_X2APIC: u32 = 1 << 21;
const L1_ECX_TSC_DEADLINE: u32 = 1 << 24;
const L1_ECX_HYPERVISOR: u32 = 1 << 31;

/// KVM paravirt leaves (`KVM_CPUID_SIGNATURE`, `KVM_CPUID_FEATURES`). The
/// guest's Linux then takes the KVM paths we back: PV EOI (its EOI is a flag
/// in its memory, no exit), PV send-IPI (one hypercall for a set of vCPUs),
/// NOP io_delay, poll control, x2APIC without interrupt remapping
/// (`kvm_para_available` is `x2apic_available`), and steal time with PV TLB
/// flush (`steal_time`). Nothing else — no kvmclock, async PF, PV unhalt.
///
/// A vCPU shares its core with host fibers and is preempted when one is due,
/// so `KVM_HINTS_REALTIME` is not given: it would switch off PV TLB flush.
/// The guest still idles through cpuidle-haltpoll (`cpuidle_haltpoll.force=1`
/// on its cmdline) — it polls with TIF_POLLING_NRFLAG set before HLT, so a
/// wakeup aimed at an idle vCPU is a flag store instead of a reschedule IPI
/// (measured: 2-3k HLT exits/s with it, 9-14k without). `set microvm_steal
/// off` gives the hint instead of steal time.
const KVM_CPUID_SIGNATURE: u32 = 0x4000_0000;
const KVM_CPUID_FEATURES: u32 = 0x4000_0001;
const KVM_FEATURE_NOP_IO_DELAY: u32 = 1 << 1;
const KVM_FEATURE_STEAL_TIME: u32 = 1 << 5;
const KVM_FEATURE_PV_EOI: u32 = 1 << 6;
const KVM_FEATURE_PV_TLB_FLUSH: u32 = 1 << 9;
const KVM_FEATURE_PV_SEND_IPI: u32 = 1 << 11;
const KVM_FEATURE_POLL_CONTROL: u32 = 1 << 12;
const KVM_HINTS_REALTIME: u32 = 1 << 0;
/// Leaf 1 EDX bits never shown: PSN, DS, ACPI, TM, IA64, PBE.
const L1_EDX_DROP: u32 = bits(&[18, 21, 22, 29, 30, 31]);
/// Intel only, leaf 1 ECX: SDBG (IA32_DEBUG_INTERFACE), PDCM (PERF_CAPABILITIES).
const INTEL_L1_ECX_DROP: u32 = bits(&[11, 15]);
/// Intel only, leaf 7.0: MPX (its BND state is not in our XCR0), ENQCMD
/// (PASID MSR), SGX_LC, Key Locker, PCONFIG, ARCH_LBR, CORE_CAPABILITIES
/// (split-lock MSR).
const INTEL_L7_EBX_DROP: u32 = bits(&[14]);
const INTEL_L7_ECX_DROP: u32 = bits(&[23, 29, 30]);
const INTEL_L7_EDX_DROP: u32 = bits(&[18, 19, 30]);
/// Intel only, leaf 7.1 EAX: FRED, LKGS, LAM — each changes entry/paging
/// semantics we do not virtualize.
const INTEL_L7_1_EAX_DROP: u32 = bits(&[17, 18, 26]);

/// Size of an XSAVE area for `xfeatures` (KVM `xstate_required_size`).
fn xstate_size(xfeatures: u64, compacted: bool) -> u32 {
    let mut size = 512 + 64;
    for i in 2..63 {
        if xfeatures & (1u64 << i) == 0 { continue; }
        let (sz, off, flags, _) = host_cpuid(0xD, i);
        size = if compacted {
            let base = if flags & 0b10 != 0 { (size + 63) & !63 } else { size };
            base + sz
        } else {
            size.max(off + sz)
        };
    }
    size
}

/// IBPB_BRTYPE (0x8000_0021 EAX[28]) for an AMD family 17h host: its IBPB
/// also flushes branch type predictions, which Linux assumes only on bare
/// metal (`init_amd`); a guest has to be told, as KVM's `SYNTHESIZED_F`
/// does. Without it the guest reports SRSO as unmitigated.
fn zen12_ibpb_brtype() -> u32 {
    let (sig, ..) = host_cpuid(1, 0);
    let family = ((sig >> 8) & 0xF) + ((sig >> 20) & 0xFF);
    let ibpb = host_cpuid(0x8000_0008, 0).1 & (1 << 12) != 0;
    if family == 0x17 && ibpb { 1 << 28 } else { 0 }
}

pub fn guest_cpuid(
    vendor: Vendor,
    leaf: u32,
    subleaf: u32,
    apic_id: u8,
    guest_cr4: u64,
    guest_xcr0: u64,
) -> (u32, u32, u32, u32) {
    let intel = vendor == Vendor::Intel;
    let (mut a, mut b, mut c, mut d) = host_cpuid(leaf, subleaf);
    match leaf {
        1 => {
            c = (c & !L1_ECX_DROP) | L1_ECX_X2APIC | L1_ECX_TSC_DEADLINE | L1_ECX_HYPERVISOR;
            if intel { c &= !INTEL_L1_ECX_DROP; }
            // OSXSAVE mirrors the guest's CR4, not the host's.
            c = (c & !(1 << 27)) | ((((guest_cr4 >> 18) & 1) as u32) << 27);
            d &= !L1_EDX_DROP;
            // Initial APIC ID = this vCPU, matching the emulated LAPIC and MP table.
            b = (b & 0x00FF_FFFF) | ((apic_id as u32) << 24);
        }
        // MONITOR/MWAIT hidden; Intel perfmon; RDT; SGX; PT.
        // Key Locker; architectural perfmon extension.
        5 | 0xA | 0xF | 0x10 | 0x12 | 0x14 | 0x19 | 0x23 => return (0, 0, 0, 0),
        // Thermal/power: only ARAT (KVM).
        6 => return (4, 0, 0, 0),
        7 if subleaf == 0 => {
            b &= !bits(&[2, 12, 15, 25]); // SGX, PQM, PQE, Intel PT
            // WAITPKG; PKU/OSPKE (PKRU would change the XSAVE layout); CET_SS.
            c &= !bits(&[3, 4, 5, 7]);
            d &= !(1 << 20); // CET_IBT — Linux's asm stubs lack ENDBR64
            if intel {
                b &= !INTEL_L7_EBX_DROP;
                c &= !INTEL_L7_ECX_DROP;
                d &= !INTEL_L7_EDX_DROP;
            }
        }
        7 if subleaf == 1 && intel => a &= !INTEL_L7_1_EAX_DROP,
        // Extended topology: EDX is the x2APIC ID → this vCPU's.
        0xB | 0x1F => d = apic_id as u32,
        0xD => {
            let host = host_xcr0();
            match subleaf {
                0 => {
                    a &= host as u32;
                    d &= (host >> 32) as u32;
                    b = xstate_size(guest_xcr0, false);
                    c = xstate_size(host, false);
                }
                1 => {
                    // Intel: XSAVEOPT/XSAVEC/XGETBV1 only. VMX does not switch
                    // IA32_XSS (XSAVES would run with the host's value) and
                    // XFD belongs to AMX, which is not in our XCR0.
                    if intel { a &= 0x7; }
                    // No supervisor states (IA32_XSS stays 0).
                    b = xstate_size(guest_xcr0, true);
                    c = 0;
                    d = 0;
                }
                i if i < 63 && host & (1u64 << i) != 0 => {}
                _ => return (0, 0, 0, 0),
            }
        }
        // "KVMKVMKVM\0\0\0"
        KVM_CPUID_SIGNATURE => return (KVM_CPUID_FEATURES, 0x4b4d_564b, 0x564b_4d56, 0x4d),
        KVM_CPUID_FEATURES => {
            let base = KVM_FEATURE_NOP_IO_DELAY | KVM_FEATURE_PV_EOI | KVM_FEATURE_PV_SEND_IPI
                | KVM_FEATURE_POLL_CONTROL;
            return if super::steal_time::enabled() {
                (base | KVM_FEATURE_STEAL_TIME | KVM_FEATURE_PV_TLB_FLUSH, 0, 0, 0)
            } else {
                (base, 0, 0, KVM_HINTS_REALTIME)
            };
        }
        0x4000_0002..=0x4000_FFFF => return (0, 0, 0, 0),
        // AMD: at least up to 0x8000_0021, where IBPB_BRTYPE is synthesized
        // (KVM does the same). Intel answers leaves past its maximum with
        // the highest basic leaf, so there the host's maximum stands.
        0x8000_0000 => a = if intel { a.min(MAX_EXT_LEAF) } else { MAX_EXT_LEAF },
        0x8000_0001 => { c &= EXT1_ECX; d &= EXT1_EDX; }
        // Only invariant TSC.
        0x8000_0007 => return (0, 0, 0, d & (1 << 8)),
        0x8000_0008 => b &= EXT8_EBX,
        // SVM features, topology, SEV, RDT-A: none of it is ours to give.
        0x8000_000A | 0x8000_001E | 0x8000_001F | 0x8000_0020 => return (0, 0, 0, 0),
        0x8000_0021 => {
            let host_has = host_cpuid(0x8000_0000, 0).0 >= 0x8000_0021;
            let a = if host_has { a & EXT21_EAX } else { 0 };
            return (a | zen12_ibpb_brtype(), 0, 0, 0);
        }
        l if l > MAX_EXT_LEAF && l < 0x8FFF_FFFF => return (0, 0, 0, 0),
        _ => {}
    }
    (a, b, c, d)
}
