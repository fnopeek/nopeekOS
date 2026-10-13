//! Host speculation mitigations against a guest, chosen per CPU the way
//! Linux `arch/x86/kernel/cpu/bugs.c` chooses them for a kernel built
//! without retpolines or return thunks (which this kernel is).
//!
//! - spectre_v2: enhanced IBRS where the CPU has it — Intel `SPEC_CTRL.IBRS`
//!   with `ARCH_CAPABILITIES.IBRS_ALL`, AMD `EFER.AUTOIBRS`
//!   (`spectre_v2_select_mitigation`).
//! - BHI on Intel eIBRS parts: `SPEC_CTRL.BHI_DIS_S` where CPUID 7.2 EDX[4]
//!   offers it (`bhi_select_mitigation`).
//! - SRSO on AMD 0x17/0x19/0x1A without `SRSO_NO`: `BP_CFG` bit 4 where
//!   `SRSO_BP_SPEC_REDUCE` exists, else IBPB on VM exit
//!   (`srso_select_mitigation`). Linux sets that bit only while a VM exists;
//!   here it stays set from boot.
//!
//! - BHI on Intel eIBRS parts without `BHI_DIS_S`: `clear_bhb_loop` after
//!   every VM exit (`CLEAR_BHB_VMEXIT`), ported below.
//!
//! IBPB after every VM exit stays wherever Linux would rely on a thunk we do
//! not have: CPUs without eIBRS and Zen 2/3 (retbleed, SRSO). The host value
//! of SPEC_CTRL is restored in the VM-exit asm before the first RET.
//!
//! Which bugs the CPU has comes from `cpu_vulns` (Linux `cpu_set_bug_bits`).
//! On top of the branch-predictor work above:
//! - VERW before every VM entry where Linux sets `CLEAR_CPU_BUF` (MDS, TAA,
//!   RFDS with `RFDS_CLEAR`, MMIO together with MDS/TAA) or, on AMD,
//!   `CLEAR_CPU_BUF_VM` (TSA with `VERW_CLEAR`). MMIO stale data alone needs
//!   it only for guests with host MMIO mapped, which no guest has here.
//! - L1D flush before every VM entry under L1TF unless
//!   `SKIP_VMENTRY_L1DFLUSH` (KVM's `always` mode).
//! - Legacy IBRS on retbleed-affected Intel parts without eIBRS
//!   (`SPECTRE_V2_IBRS`), and STIBP on SMT AMD parts whose retbleed
//!   mitigation is IBPB (`SPECTRE_V2_USER_STRICT_PREFERRED`).
//!
//! Whatever stays open is listed on the boot line as not mitigated.

use crate::hw::msr;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MSR_SPEC_CTRL: u32 = 0x48;
const MSR_ARCH_CAPABILITIES: u32 = 0x10A;
const MSR_EFER: u32 = 0xC000_0080;
const MSR_AMD64_BP_CFG: u32 = 0xC001_102E;

const SPEC_CTRL_IBRS: u64 = 1 << 0;
const SPEC_CTRL_STIBP: u64 = 1 << 1;
const SPEC_CTRL_BHI_DIS_S: u64 = 1 << 10;
const ARCH_CAP_IBRS_ALL: u64 = 1 << 1;
const ARCH_CAP_PBRSB_NO: u64 = 1 << 24;
const EFER_AUTOIBRS: u64 = 1 << 21;
const BP_CFG_SPEC_REDUCE: u64 = 1 << 4;

/// CPUID 0x8000_0021 EAX.
const AMD_AUTOIBRS: u32 = 1 << 8;
const AMD_SRSO_NO: u32 = 1 << 29;
const AMD_SRSO_BP_SPEC_REDUCE: u32 = 1 << 31;

static DECIDED: AtomicBool = AtomicBool::new(false);
static HOST_SPEC_CTRL: AtomicU64 = AtomicU64::new(0);
static AUTOIBRS: AtomicBool = AtomicBool::new(false);
static BP_SPEC_REDUCE: AtomicBool = AtomicBool::new(false);
static IBPB_ON_EXIT: AtomicBool = AtomicBool::new(true);
static CLEAR_BHB_ON_EXIT: AtomicBool = AtomicBool::new(false);
static VERW_ON_ENTRY: AtomicBool = AtomicBool::new(false);
static L1D_FLUSH_ON_ENTRY: AtomicBool = AtomicBool::new(false);
static RSB_FILL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(RSB_FULL);

/// Return-stack fill after a VM exit (`spectre_v2_determine_rsb_fill_type_at_vmexit`):
/// none under eIBRS without PBRSB, one CALL with PBRSB (`RSB_VMEXIT_LITE`),
/// all 32 entries otherwise (`RSB_VMEXIT`).
pub const RSB_NONE: u8 = 0;
pub const RSB_LITE: u8 = 1;
pub const RSB_FULL: u8 = 2;

fn cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let r = core::arch::x86_64::__cpuid_count(leaf, sub);
    (r.eax, r.ebx, r.ecx, r.edx)
}

/// A hypervisor may report `SRSO_BP_SPEC_REDUCE` without emulating BP_CFG,
/// and a write to an MSR it does not know is a #GP; Linux only writes it as
/// the host.
fn under_hypervisor() -> bool {
    cpuid(1, 0).2 & (1 << 31) != 0
}

/// The SPEC_CTRL value every host core runs with.
pub fn host_spec_ctrl() -> u64 {
    HOST_SPEC_CTRL.load(Ordering::Relaxed)
}

/// Whether a VM exit still needs an IBPB on this CPU.
pub fn ibpb_on_exit() -> bool {
    IBPB_ON_EXIT.load(Ordering::Relaxed)
}

/// Whether every VM entry needs `verw` (CPU buffer clearing) on this CPU.
pub fn verw_on_entry() -> bool {
    VERW_ON_ENTRY.load(Ordering::Relaxed)
}

/// Whether every VM entry needs an L1D flush on this CPU.
pub fn l1d_flush_on_entry() -> bool {
    L1D_FLUSH_ON_ENTRY.load(Ordering::Relaxed)
}

/// The return-stack fill a VM exit needs on this CPU when no IBPB runs.
pub fn rsb_fill_on_exit() -> u8 {
    RSB_FILL.load(Ordering::Relaxed)
}

/// Whether a VM exit needs the branch history overwritten on this CPU.
pub fn clear_bhb_on_exit() -> bool {
    CLEAR_BHB_ON_EXIT.load(Ordering::Relaxed)
}

fn decide() {
    let (_, b, c, d) = cpuid(0, 0);
    let intel = (b, d, c) == (0x756e_6547, 0x4965_6e69, 0x6c65_746e);
    let amd = (b, d, c) == (0x6874_7541, 0x6974_6e65, 0x444d_4163);
    let max = cpuid(0, 0).0;
    let (_, _, _, l7d) = if max >= 7 { cpuid(7, 0) } else { (0, 0, 0, 0) };
    let has_spec_ctrl = l7d & (1 << 26) != 0;
    let mut host_spec = 0u64;
    let mut ibpb = true;
    let mut report = "IBPB on every VM exit";

    if intel && has_spec_ctrl && l7d & (1 << 29) != 0 {
        // SAFETY: CPUID 7.0 EDX[29] says IA32_ARCH_CAPABILITIES exists.
        let caps = unsafe { msr::read(MSR_ARCH_CAPABILITIES) };
        if caps & ARCH_CAP_IBRS_ALL != 0 {
            let sub2 = cpuid(7, 0).0 >= 2;
            let bhi_ctrl = sub2 && cpuid(7, 2).3 & (1 << 4) != 0;
            host_spec = SPEC_CTRL_IBRS | if bhi_ctrl { SPEC_CTRL_BHI_DIS_S } else { 0 };
            ibpb = false;
            CLEAR_BHB_ON_EXIT.store(!bhi_ctrl, Ordering::Relaxed);
            // X86_BUG_EIBRS_PBRSB (common.c); the Atom whitelist is left out,
            // which keeps the single CALL there.
            RSB_FILL.store(if caps & ARCH_CAP_PBRSB_NO != 0 { RSB_NONE } else { RSB_LITE },
                Ordering::Relaxed);
            report = if bhi_ctrl {
                "eIBRS + BHI_DIS_S, no IBPB on VM exit"
            } else {
                "eIBRS + BHB clearing on VM exit, no IBPB"
            };
        }
    } else if amd && cpuid(0x8000_0000, 0).0 >= 0x8000_0021 {
        let ext = cpuid(0x8000_0021, 0).0;
        if ext & AMD_AUTOIBRS != 0 {
            AUTOIBRS.store(true, Ordering::Relaxed);
            let (a, ..) = cpuid(1, 0);
            let family = ((a >> 8) & 0xF) + if (a >> 8) & 0xF == 0xF { (a >> 20) & 0xFF } else { 0 };
            let srso = matches!(family, 0x17 | 0x19 | 0x1A) && ext & AMD_SRSO_NO == 0;
            // AutoIBRS is eIBRS to Linux, and AMD is NO_EIBRS_PBRSB.
            if !srso {
                ibpb = false;
                RSB_FILL.store(RSB_NONE, Ordering::Relaxed);
                report = "AutoIBRS, no IBPB on VM exit";
            } else if ext & AMD_SRSO_BP_SPEC_REDUCE != 0 && !under_hypervisor() {
                BP_SPEC_REDUCE.store(true, Ordering::Relaxed);
                RSB_FILL.store(RSB_NONE, Ordering::Relaxed);
                ibpb = false;
                report = "AutoIBRS + SRSO BP_SPEC_REDUCE, no IBPB on VM exit";
            } else {
                report = "AutoIBRS, IBPB on VM exit for SRSO";
            }
        }
    }
    let bugs = crate::cpu_vulns::detect();
    if intel && has_spec_ctrl && !bugs.eibrs && bugs.retbleed {
        host_spec |= SPEC_CTRL_IBRS;
        report = "legacy IBRS, IBPB on every VM exit";
    }
    if amd && bugs.retbleed && ibpb && bugs.smt && bugs.stibp {
        host_spec |= SPEC_CTRL_STIBP;
    }
    HOST_SPEC_CTRL.store(host_spec, Ordering::Relaxed);
    IBPB_ON_EXIT.store(ibpb, Ordering::Relaxed);
    let rsb = ["no RSB fill", "one RSB entry", "full RSB fill"][RSB_FILL.load(Ordering::Relaxed) as usize];
    crate::kprintln!("[npk] guest isolation: {}, {} (SPEC_CTRL {:#x})", report, rsb, host_spec);
    report_bugs(&bugs, ibpb, host_spec);
}

/// One affected bug on the boot line: its name and what covers it.
struct Line { buf: [u8; 512], len: usize }

impl Line {
    fn push(&mut self, s: &str) {
        let b = s.as_bytes();
        let n = b.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
    }
    fn item(&mut self, name: &str, how: &str) {
        if self.len != 0 { self.push(", "); }
        self.push(name);
        self.push(" ");
        self.push(how);
    }
}

/// Decide VERW and the L1D flush, and print every guest-relevant bug the
/// CPU has with what covers it, so a CPU nobody has tested says what is
/// open instead of looking safe.
fn report_bugs(b: &crate::cpu_vulns::Bugs, ibpb: bool, host_spec: u64) {
    use crate::cpu_vulns::{AC_RFDS_CLEAR, AC_SKIP_VMENTRY_L1DFLUSH};
    const OPEN: &str = "NOT MITIGATED";
    let mut l = Line { buf: [0; 512], len: 0 };
    let mut verw = false;
    if b.mds {
        if b.md_clear { verw = true; l.item("MDS", "VERW"); } else { l.item("MDS", OPEN); }
    }
    if b.taa {
        if b.md_clear { verw = true; l.item("TAA", "VERW"); } else { l.item("TAA", OPEN); }
    }
    if b.rfds {
        if b.arch_cap & AC_RFDS_CLEAR != 0 { verw = true; l.item("RFDS", "VERW"); } else { l.item("RFDS", OPEN); }
    }
    if b.mmio {
        if b.mds || b.taa { l.item("MMIO", if b.md_clear { "VERW" } else { OPEN }); }
        else { l.item("MMIO", "no host MMIO in guests"); }
    }
    if b.tsa {
        if b.verw_clear { verw = true; l.item("TSA", "VERW"); } else { l.item("TSA", OPEN); }
    }
    let mut l1d = false;
    if b.l1tf && b.intel {
        if b.arch_cap & AC_SKIP_VMENTRY_L1DFLUSH != 0 { l.item("L1TF", "flush not needed"); }
        else if b.flush_l1d { l1d = true; l.item("L1TF", "L1D flush"); }
        else { l.item("L1TF", OPEN); }
    }
    if b.retbleed {
        l.item("retbleed", if ibpb { "IBPB" } else { "eIBRS" });
        if b.amd && b.smt {
            l.item("retbleed SMT", if host_spec & SPEC_CTRL_STIBP != 0 { "STIBP" } else { OPEN });
        }
    }
    if b.srso {
        l.item("SRSO", if ibpb { "IBPB" } else { "BP_SPEC_REDUCE" });
    }
    if b.smt_rsb { l.item("SMT_RSB", "HLT/MWAIT intercepted"); }
    if b.bhi { l.item("BHI", if b.bhi_ctrl { "BHI_DIS_S" } else { "BHB clearing" }); }
    if b.eibrs_pbrsb { l.item("PBRSB", "one RSB entry"); }
    if b.its {
        l.item("ITS", if b.its_native_only { "host only" } else { OPEN });
    }
    if b.gds { l.item("GDS", if b.arch_cap & (1 << 25) != 0 { "microcode" } else { OPEN }); }
    if b.srbds {
        let ctrl = core::arch::x86_64::__cpuid_count(7, 0).edx & (1 << 9) != 0;
        l.item("SRBDS", if ctrl { "microcode" } else { OPEN });
    }
    VERW_ON_ENTRY.store(verw, Ordering::Relaxed);
    L1D_FLUSH_ON_ENTRY.store(l1d, Ordering::Relaxed);
    let text = core::str::from_utf8(&l.buf[..l.len]).unwrap_or("?");
    crate::kprintln!("[npk] cpu bugs: {}", if text.is_empty() { "none that reach a guest" } else { text });
}

/// Decide once, then set this core up. Called on every core before any
/// guest can run there.
pub fn apply() {
    if !DECIDED.swap(true, Ordering::AcqRel) {
        decide();
    }
    let spec = host_spec_ctrl();
    if spec != 0 {
        // SAFETY: a non-zero value is chosen only when CPUID reports
        // SPEC_CTRL and each of its bits.
        unsafe { msr::write(MSR_SPEC_CTRL, spec) };
    }
    if AUTOIBRS.load(Ordering::Relaxed) {
        // SAFETY: CPUID 0x8000_0021 EAX[8] reports EFER.AUTOIBRS; the other
        // EFER bits are kept.
        unsafe { msr::write(MSR_EFER, msr::read(MSR_EFER) | EFER_AUTOIBRS) };
    }
    if BP_SPEC_REDUCE.load(Ordering::Relaxed) {
        // SAFETY: CPUID 0x8000_0021 EAX[31] reports this BP_CFG bit.
        unsafe { msr::write(MSR_AMD64_BP_CFG, msr::read(MSR_AMD64_BP_CFG) | BP_CFG_SPEC_REDUCE) };
    }
}

// Linux `clear_bhb_loop` (arch/x86/entry/entry_64.S): five rounds of five
// taken branches each overwrite the branch history before the host executes
// an indirect branch after a VM exit. The paddings keep each RET in the
// upper half of its cache line, as Linux's `.skip` does: 27 = 32 - the CALL
// before `.Lret1`, 14 = 32 - the 18 bytes before `.Lret2`. Clobbers rax, rcx.
core::arch::global_asm!(
    ".pushsection .text.npk_clear_bhb_loop, \"ax\", @progbits",
    ".global npk_clear_bhb_loop",
    ".p2align 6, 0xcc",
    "npk_clear_bhb_loop:",
    "    push rbp",
    "    mov rbp, rsp",
    "    mov ecx, 5",
    "    call 1f",
    "    jmp 5f",
    "    .p2align 6, 0xcc",
    "    .skip 27, 0xcc",
    "1:  call 2f",
    "    ret",
    "    .p2align 6, 0xcc",
    "    .skip 14, 0xcc",
    "2:  mov eax, 5",
    "3:  jmp 4f",
    "    nop",
    "4:  sub eax, 1",
    "    jnz 3b",
    "    sub ecx, 1",
    "    jnz 1b",
    "    ret",
    "5:  lfence",
    "    pop rbp",
    "    ret",
    ".popsection",
);

// Linux `x86_verw_sel` (entry.S): a word holding a writable data selector in
// its own cache line, the memory operand of `verw`, which is the form that
// clears the CPU buffers. 0x10 is the kernel data segment.
core::arch::global_asm!(
    ".pushsection .rodata.npk_verw_sel, \"a\", @progbits",
    ".global npk_verw_sel",
    ".p2align 6",
    "npk_verw_sel:",
    "    .word 0x10",
    "    .p2align 6",
    ".popsection",
);
