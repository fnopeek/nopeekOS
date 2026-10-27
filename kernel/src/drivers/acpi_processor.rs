//! What Linux does with the ACPI processor objects for idle, on the
//! kernel's side: the capabilities the OS hands `_OSC`/`_PDC`
//! (`arch_acpi_set_proc_cap_bits`, arch/x86/include/asm/acpi.h) and the
//! `_CST` states `intel_idle` takes on a CPU without a table of its own
//! (`acpi_processor_ffh_cstate_probe`, arch/x86/kernel/acpi/cstate.c;
//! `intel_idle_cst_usable`, `intel_idle_init_cstates_acpi`,
//! drivers/idle/intel_idle.c). The AML runs in the `acpi` module, which
//! reports the `_CST` entries here.

use crate::kprintln;

// include/acpi/proc_cap_intel.h
const ACPI_PROC_CAP_P_FFH: u32 = 0x0001;
const ACPI_PROC_CAP_C_C1_HALT: u32 = 0x0002;
const ACPI_PROC_CAP_T_FFH: u32 = 0x0004;
const ACPI_PROC_CAP_SMP_C1PT: u32 = 0x0008;
const ACPI_PROC_CAP_SMP_C2C3: u32 = 0x0010;
const ACPI_PROC_CAP_SMP_P_SWCOORD: u32 = 0x0020;
const ACPI_PROC_CAP_SMP_T_SWCOORD: u32 = 0x0080;
const ACPI_PROC_CAP_C_C1_FFH: u32 = 0x0100;
const ACPI_PROC_CAP_C_C2C3_FFH: u32 = 0x0200;
const ACPI_PROC_CAP_SMP_P_HWCOORD: u32 = 0x0800;
const ACPI_PROC_CAP_COLLAB_PROC_PERF: u32 = 0x1000;
const ACPI_PROC_CAP_EST_CAPABILITY_SWSMP: u32 = ACPI_PROC_CAP_SMP_C1PT | ACPI_PROC_CAP_C_C1_HALT
    | ACPI_PROC_CAP_SMP_P_SWCOORD | ACPI_PROC_CAP_SMP_P_HWCOORD | ACPI_PROC_CAP_P_FFH;
const ACPI_PROC_CAP_C_CAPABILITY_SMP: u32 = ACPI_PROC_CAP_SMP_C2C3 | ACPI_PROC_CAP_SMP_C1PT
    | ACPI_PROC_CAP_C_C1_HALT | ACPI_PROC_CAP_C_C1_FFH | ACPI_PROC_CAP_C_C2C3_FFH;

// arch/x86/include/asm/mwait.h
const MWAIT_SUBSTATE_MASK: u32 = 0xf;
const MWAIT_CSTATE_MASK: u32 = 0xf;
const MWAIT_SUBSTATE_SIZE: u32 = 4;
const CPUID5_ECX_EXTENSIONS_SUPPORTED: u32 = 0x1;
const CPUID5_ECX_INTERRUPT_BREAK: u32 = 0x2;
/// `NATIVE_CSTATE_BEYOND_HALT` (arch/x86/kernel/acpi/cstate.c).
const NATIVE_CSTATE_BEYOND_HALT: u8 = 2;
/// `ACPI_ADR_SPACE_FIXED_HARDWARE`.
const SPACE_FFH: u8 = 0x7F;
const ACPI_STATE_C1: u32 = 1;
/// `CPUIDLE_STATE_MAX`.
const CPUIDLE_STATE_MAX: usize = 10;

fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let (a, b, c, d): (u32, u32, u32, u32);
    // SAFETY: CPUID with a leaf the caller checked against the maximum
    // leaf (or leaf 0/1/6, which every x86-64 has); rbx is reserved by
    // LLVM, so it is moved out by hand.
    unsafe {
        core::arch::asm!("push rbx", "cpuid", "mov {0:e}, ebx", "pop rbx",
            out(reg) b, inout("eax") leaf => a, inout("ecx") sub => c, out("edx") d);
    }
    (a, b, c, d)
}

/// `arch_acpi_set_proc_cap_bits`: the capabilities of this CPU, for the
/// support dword of `_OSC` and the dword of `_PDC`.
pub fn proc_cap_bits() -> u32 {
    let (_, _, ecx1, edx1) = cpuid(1, 0);
    let (eax6, _, _, _) = cpuid(6, 0);
    let mut cap = ACPI_PROC_CAP_C_CAPABILITY_SMP;

    // Enable coordination with firmware's _TSD info
    cap |= ACPI_PROC_CAP_SMP_T_SWCOORD;

    // X86_FEATURE_EST
    if ecx1 & (1 << 7) != 0 {
        cap |= ACPI_PROC_CAP_EST_CAPABILITY_SWSMP;
    }

    // X86_FEATURE_ACPI
    if edx1 & (1 << 22) != 0 {
        cap |= ACPI_PROC_CAP_T_FFH;
    }

    // X86_FEATURE_HWP
    if eax6 & (1 << 7) != 0 {
        cap |= ACPI_PROC_CAP_COLLAB_PROC_PERF;
    }

    // If mwait/monitor is unsupported, C_C1_FFH and C2/C3_FFH will be
    // disabled.
    if !crate::smp::per_core::has_mwait() {
        cap &= !(ACPI_PROC_CAP_C_C1_FFH | ACPI_PROC_CAP_C_C2C3_FFH);
    }
    cap
}

/// A `_CST` entry as `acpi` reports it (`struct acpi_processor_cx` plus its
/// register). The register's access size steers bus-master avoidance,
/// which `acpi_idle` checks and `intel_idle` does not.
#[derive(Clone, Copy, Debug)]
pub struct Cx {
    pub kind: u32,
    pub latency: u32,
    pub power: u32,
    pub space_id: u8,
    pub bit_offset: u8,
    pub address: u64,
}

/// `acpi_processor_ffh_cstate_probe_cpu`: the hint names a C-state the
/// CPU has sub-states of, and MWAIT can break on an interrupt.
fn ffh_cstate_probe(cx: &Cx) -> bool {
    let (max, _, _, _) = cpuid(0, 0);
    if max < 5 {
        return false;
    }
    let (_, _, ecx, edx) = cpuid(5, 0);
    // Check whether this particular cx_type (in CST) is supported or not
    let cstate_type = (((cx.address as u32 >> MWAIT_SUBSTATE_SIZE) & MWAIT_CSTATE_MASK) + 1) & MWAIT_CSTATE_MASK;
    let edx_part = edx >> (cstate_type * MWAIT_SUBSTATE_SIZE);
    let num_cstate_subtype = edx_part & MWAIT_SUBSTATE_MASK;

    // If the HW does not support any sub-states in this C-state
    if num_cstate_subtype == 0 {
        kprintln!("[npk] acpi: MWAIT C-state 0x{:x} not supported by HW (0x{:x})", cx.address, edx_part);
        return false;
    }

    // mwait ecx extensions INTERRUPT_BREAK should be supported for C2/C3
    ecx & CPUID5_ECX_EXTENSIONS_SUPPORTED != 0 && ecx & CPUID5_ECX_INTERRUPT_BREAK != 0
}

/// Take the `_CST` of a processor for idle, as `intel_idle` does on a CPU
/// it has no table for: every state must be entered by MWAIT (FFH,
/// `intel_idle_cst_usable`), and each becomes a state with the exit
/// latency as target residency for C1 and three times it beyond. Returns
/// how many states idle may use; they are switched on with
/// `power cstate on`.
pub fn take_cst(cst: &[Cx]) -> usize {
    if crate::smp::per_core::intel_model().is_none() || !crate::smp::per_core::has_mwait() {
        kprintln!("[npk] acpi: _CST with {} state(s) — MWAIT idle is for Intel only", cst.len());
        return 0;
    }
    let mut states: alloc::vec::Vec<(u32, u64)> = alloc::vec::Vec::new();
    for (i, cx) in cst.iter().enumerate().take(CPUIDLE_STATE_MAX - 1) {
        let ffh = cx.space_id == SPACE_FFH && cx.bit_offset == NATIVE_CSTATE_BEYOND_HALT && ffh_cstate_probe(cx);
        kprintln!("[npk] acpi: _CST C{} type {} latency {} us power {} mW {}", i + 1, cx.kind, cx.latency, cx.power,
            if ffh { alloc::format!("MWAIT 0x{:02x}", cx.address) }
            else { alloc::format!("space 0x{:02x} address 0x{:x}", cx.space_id, cx.address) });
        if !ffh {
            kprintln!("[npk] acpi: _CST state C{} is not entered by MWAIT — not usable", i + 1);
            return 0;
        }
        let mut target = cx.latency as u64;
        if cx.kind > ACPI_STATE_C1 {
            target *= 3;
        }
        states.push((cx.address as u32, target));
    }
    let n = crate::interrupts::set_mwait_states(&states);
    kprintln!("[npk] acpi: {} MWAIT idle state(s) from _CST — 'power cstate on' to use them", n);
    n
}
