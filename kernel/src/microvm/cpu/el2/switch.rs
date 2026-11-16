//! The world switch, after Linux `arch/arm64/kvm/hyp/entry.S`
//! (`__guest_enter`, `__guest_exit`), `hyp/vhe/switch.c`
//! (`__kvm_vcpu_run_vhe`, `__activate_traps`, `__deactivate_traps`) and
//! `hyp/include/hyp/sysreg-sr.h`.
//!
//! The host runs at EL2 with HCR_EL2.{E2H,TGE} set, so the EL1 register
//! names reach EL2's own registers; the guest's EL1 state lives in the real
//! EL1 registers, which EL2 reaches through the `_EL12`/`_EL02` names. Those
//! are written as generic `S<op0>_<op1>_C<n>_C<m>_<op2>` encodings, which
//! assemble without the assembler's VHE feature. TPIDR_EL1 is not
//! redirected: it holds the host's core index, so it is swapped in the
//! assembly, next to the general and SIMD registers.
//!
//! An exception from the guest arrives at the lower-EL entries of the one
//! vector table (`arch/aarch64/vectors.s`), which jump to `npk_guest_exit`
//! with the kind in x0; TPIDR_EL2 names the running vCPU's register file,
//! as it names Linux's per-CPU data under VHE.

use core::mem::offset_of;

/// The guest's general and SIMD registers (`struct kvm_cpu_context` gp/fp
/// part). Offsets are fixed by the assembly below.
#[repr(C, align(16))]
pub struct GuestRegs {
    pub x: [u64; 31],
    pub sp_el0: u64,
    pub pc: u64,
    pub pstate: u64,
    pub fpsr: u64,
    pub fpcr: u64,
    pub v: [u128; 32],
    /// The host save area while the guest runs.
    host: u64,
    pub tpidr_el1: u64,
}

#[repr(C, align(16))]
pub struct HostSave {
    x19_x30: [u64; 12],
    sp: u64,
    d8_d15: [u64; 8],
    fpsr: u64,
    fpcr: u64,
    sp_el0: u64,
    tpidr_el1: u64,
}

const _: () = {
    assert!(offset_of!(GuestRegs, sp_el0) == 248);
    assert!(offset_of!(GuestRegs, pc) == 256);
    assert!(offset_of!(GuestRegs, fpsr) == 272);
    assert!(offset_of!(GuestRegs, v) == 288);
    assert!(offset_of!(GuestRegs, host) == 800);
    assert!(offset_of!(GuestRegs, tpidr_el1) == 808);
    assert!(offset_of!(HostSave, sp) == 96);
    assert!(offset_of!(HostSave, d8_d15) == 104);
    assert!(offset_of!(HostSave, fpsr) == 168);
    assert!(offset_of!(HostSave, sp_el0) == 184);
    assert!(offset_of!(HostSave, tpidr_el1) == 192);
};

impl GuestRegs {
    pub const fn new() -> Self {
        GuestRegs { x: [0; 31], sp_el0: 0, pc: 0, pstate: 0, fpsr: 0, fpcr: 0, v: [0; 32], host: 0, tpidr_el1: 0 }
    }

    /// Register `n` of an instruction's Rt field: 31 is the zero register.
    pub fn get(&self, n: usize) -> u64 {
        if n < 31 { self.x[n] } else { 0 }
    }

    pub fn set(&mut self, n: usize, v: u64) {
        if n < 31 { self.x[n] = v; }
    }
}

impl HostSave {
    pub const fn new() -> Self {
        HostSave { x19_x30: [0; 12], sp: 0, d8_d15: [0; 8], fpsr: 0, fpcr: 0, sp_el0: 0, tpidr_el1: 0 }
    }
}

/// Exit kinds, the lower-EL vector entry the exception came through.
pub const EXIT_SYNC: u64 = 0;
pub const EXIT_IRQ: u64 = 1;
pub const EXIT_FIQ: u64 = 2;
pub const EXIT_SERROR: u64 = 3;

/// Iterations of the branch-history loop on exit (Spectre-BHB,
/// `spectre_bhb_loop_affected`); 0 = not needed. Set before the first entry.
#[unsafe(no_mangle)]
pub static NPK_EL2_BHB_K: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

core::arch::global_asm!(
    r#"
.pushsection .text, "ax"
.balign 16
.global npk_guest_enter
npk_guest_enter:
    stp     x19, x20, [x1, #0]
    stp     x21, x22, [x1, #16]
    stp     x23, x24, [x1, #32]
    stp     x25, x26, [x1, #48]
    stp     x27, x28, [x1, #64]
    stp     x29, x30, [x1, #80]
    mov     x2, sp
    str     x2, [x1, #96]
    stp     d8, d9, [x1, #104]
    stp     d10, d11, [x1, #120]
    stp     d12, d13, [x1, #136]
    stp     d14, d15, [x1, #152]
    mrs     x2, FPSR
    mrs     x3, FPCR
    stp     x2, x3, [x1, #168]
    mrs     x2, SP_EL0
    mrs     x3, TPIDR_EL1
    stp     x2, x3, [x1, #184]
    str     x1, [x0, #800]
    msr     TPIDR_EL2, x0
    ldr     x2, [x0, #808]
    msr     TPIDR_EL1, x2
    ldp     x2, x3, [x0, #272]
    msr     FPSR, x2
    msr     FPCR, x3
    add     x2, x0, #288
    ldp     q0, q1, [x2, #0]
    ldp     q2, q3, [x2, #32]
    ldp     q4, q5, [x2, #64]
    ldp     q6, q7, [x2, #96]
    ldp     q8, q9, [x2, #128]
    ldp     q10, q11, [x2, #160]
    ldp     q12, q13, [x2, #192]
    ldp     q14, q15, [x2, #224]
    ldp     q16, q17, [x2, #256]
    ldp     q18, q19, [x2, #288]
    ldp     q20, q21, [x2, #320]
    ldp     q22, q23, [x2, #352]
    ldp     q24, q25, [x2, #384]
    ldp     q26, q27, [x2, #416]
    ldp     q28, q29, [x2, #448]
    ldp     q30, q31, [x2, #480]
    ldr     x2, [x0, #248]
    msr     SP_EL0, x2
    ldp     x2, x3, [x0, #256]
    msr     ELR_EL2, x2
    msr     SPSR_EL2, x3
    ldp     x2, x3, [x0, #16]
    ldp     x4, x5, [x0, #32]
    ldp     x6, x7, [x0, #48]
    ldp     x8, x9, [x0, #64]
    ldp     x10, x11, [x0, #80]
    ldp     x12, x13, [x0, #96]
    ldp     x14, x15, [x0, #112]
    ldp     x16, x17, [x0, #128]
    ldp     x18, x19, [x0, #144]
    ldp     x20, x21, [x0, #160]
    ldp     x22, x23, [x0, #176]
    ldp     x24, x25, [x0, #192]
    ldp     x26, x27, [x0, #208]
    ldp     x28, x29, [x0, #224]
    ldr     x30, [x0, #240]
    ldp     x0, x1, [x0, #0]
    eret
    dsb     nsh
    isb

/* x0 = exit kind, the guest's x0/x1 on the stack (the vector pushed them). */
.global npk_guest_exit
npk_guest_exit:
    mrs     x1, TPIDR_EL2
    cbz     x1, 9f
    stp     x2, x3, [x1, #16]
    stp     x4, x5, [x1, #32]
    stp     x6, x7, [x1, #48]
    stp     x8, x9, [x1, #64]
    stp     x10, x11, [x1, #80]
    stp     x12, x13, [x1, #96]
    stp     x14, x15, [x1, #112]
    stp     x16, x17, [x1, #128]
    stp     x18, x19, [x1, #144]
    stp     x20, x21, [x1, #160]
    stp     x22, x23, [x1, #176]
    stp     x24, x25, [x1, #192]
    stp     x26, x27, [x1, #208]
    stp     x28, x29, [x1, #224]
    str     x30, [x1, #240]
    ldp     x2, x3, [sp], #16
    stp     x2, x3, [x1, #0]
    mrs     x2, SP_EL0
    str     x2, [x1, #248]
    mrs     x2, ELR_EL2
    mrs     x3, SPSR_EL2
    stp     x2, x3, [x1, #256]
    mrs     x2, FPSR
    mrs     x3, FPCR
    stp     x2, x3, [x1, #272]
    add     x2, x1, #288
    stp     q0, q1, [x2, #0]
    stp     q2, q3, [x2, #32]
    stp     q4, q5, [x2, #64]
    stp     q6, q7, [x2, #96]
    stp     q8, q9, [x2, #128]
    stp     q10, q11, [x2, #160]
    stp     q12, q13, [x2, #192]
    stp     q14, q15, [x2, #224]
    stp     q16, q17, [x2, #256]
    stp     q18, q19, [x2, #288]
    stp     q20, q21, [x2, #320]
    stp     q22, q23, [x2, #352]
    stp     q24, q25, [x2, #384]
    stp     q26, q27, [x2, #416]
    stp     q28, q29, [x2, #448]
    stp     q30, q31, [x2, #480]
    mrs     x2, TPIDR_EL1
    str     x2, [x1, #808]
    msr     TPIDR_EL2, xzr
    adrp    x2, NPK_EL2_BHB_K
    ldr     x2, [x2, :lo12:NPK_EL2_BHB_K]
    cbz     x2, 2f
1:  b       . + 4
    subs    x2, x2, #1
    b.ne    1b
    dsb     nsh
    isb
2:  ldr     x1, [x1, #800]
    ldp     x2, x3, [x1, #184]
    msr     SP_EL0, x2
    msr     TPIDR_EL1, x3
    ldp     x2, x3, [x1, #168]
    msr     FPSR, x2
    msr     FPCR, x3
    ldp     d8, d9, [x1, #104]
    ldp     d10, d11, [x1, #120]
    ldp     d12, d13, [x1, #136]
    ldp     d14, d15, [x1, #152]
    ldr     x2, [x1, #96]
    mov     sp, x2
    ldp     x19, x20, [x1, #0]
    ldp     x21, x22, [x1, #16]
    ldp     x23, x24, [x1, #32]
    ldp     x25, x26, [x1, #48]
    ldp     x27, x28, [x1, #64]
    ldp     x29, x30, [x1, #80]
    ret
9:  ldp     x0, x1, [sp], #16
    b       lower_el_report
.popsection
"#
);

unsafe extern "C" {
    fn npk_guest_enter(guest: *mut GuestRegs, host: *mut HostSave) -> u64;
}

/// Read a system register by name or generic encoding.
macro_rules! rd {
    ($r:literal) => {{
        let v: u64;
        // SAFETY: an EL2 read of an EL1/EL2 register, no side effect.
        unsafe { core::arch::asm!(concat!("mrs {}, ", $r), out(reg) v, options(nomem, nostack, preserves_flags)) };
        v
    }};
}

macro_rules! wr {
    ($r:literal, $v:expr) => {{
        let v: u64 = $v;
        // SAFETY: the caller is the world switch; the register belongs to
        // the guest context it loads or to the host state it restores.
        unsafe { core::arch::asm!(concat!("msr ", $r, ", {}"), in(reg) v, options(nostack, preserves_flags)) };
    }};
}
pub(crate) use rd;
#[allow(unused_imports)]
pub(crate) use wr;

/// The guest's EL1 system registers (`__sysreg_save_el1_state` /
/// `__sysreg_restore_el1_state`, user and common state).
#[derive(Clone, Copy, Default)]
pub struct SysRegs {
    pub sctlr: u64,
    pub cpacr: u64,
    pub ttbr0: u64,
    pub ttbr1: u64,
    pub tcr: u64,
    pub esr: u64,
    pub afsr0: u64,
    pub afsr1: u64,
    pub far: u64,
    pub mair: u64,
    pub amair: u64,
    pub vbar: u64,
    pub contextidr: u64,
    pub cntkctl: u64,
    pub spsr: u64,
    pub elr: u64,
    pub sp_el1: u64,
    pub par: u64,
    pub csselr: u64,
    pub tpidr_el0: u64,
    pub tpidrro_el0: u64,
    pub cntv_ctl: u64,
    pub cntv_cval: u64,
}

/// The EL2 controls a vCPU runs with.
#[derive(Clone, Copy)]
pub struct Traps {
    pub hcr: u64,
    pub vttbr: u64,
    pub vtcr: u64,
    pub cnthctl: u64,
    pub mdcr: u64,
    pub vpidr: u64,
    pub vmpidr: u64,
    pub cntvoff: u64,
}

/// The host's values of what the switch changes.
#[derive(Clone, Copy)]
pub struct HostCtl {
    pub hcr: u64,
    pub cnthctl: u64,
    pub mdcr: u64,
}

pub fn host_ctl() -> HostCtl {
    HostCtl { hcr: rd!("HCR_EL2"), cnthctl: rd!("CNTHCTL_EL2"), mdcr: rd!("MDCR_EL2") }
}

/// Load the guest's EL1 state, then its traps (`__kvm_vcpu_run_vhe`: stage
/// 1 and 2 in place before TGE is cleared, erratum 1165522).
fn load(s: &SysRegs, t: &Traps) {
    wr!("S3_5_C1_C0_0", s.sctlr);
    wr!("S3_5_C1_C0_2", s.cpacr);
    wr!("S3_5_C2_C0_0", s.ttbr0);
    wr!("S3_5_C2_C0_1", s.ttbr1);
    wr!("S3_5_C2_C0_2", s.tcr);
    wr!("S3_5_C5_C2_0", s.esr);
    wr!("S3_5_C5_C1_0", s.afsr0);
    wr!("S3_5_C5_C1_1", s.afsr1);
    wr!("S3_5_C6_C0_0", s.far);
    wr!("S3_5_C10_C2_0", s.mair);
    wr!("S3_5_C10_C3_0", s.amair);
    wr!("S3_5_C12_C0_0", s.vbar);
    wr!("S3_5_C13_C0_1", s.contextidr);
    wr!("S3_5_C14_C1_0", s.cntkctl);
    wr!("S3_5_C4_C0_0", s.spsr);
    wr!("S3_5_C4_C0_1", s.elr);
    wr!("SP_EL1", s.sp_el1);
    wr!("PAR_EL1", s.par);
    wr!("CSSELR_EL1", s.csselr);
    wr!("TPIDR_EL0", s.tpidr_el0);
    wr!("TPIDRRO_EL0", s.tpidrro_el0);
    wr!("VPIDR_EL2", t.vpidr);
    wr!("VMPIDR_EL2", t.vmpidr);
    wr!("CNTVOFF_EL2", t.cntvoff);
    wr!("S3_5_C14_C3_2", s.cntv_cval);
    wr!("S3_5_C14_C3_1", s.cntv_ctl);
    wr!("VTCR_EL2", t.vtcr);
    wr!("VTTBR_EL2", t.vttbr);
    wr!("CNTHCTL_EL2", t.cnthctl);
    wr!("MDCR_EL2", t.mdcr);
    // SAFETY: a barrier; the guest's translation regime is complete.
    unsafe { core::arch::asm!("isb", options(nostack, preserves_flags)) };
    wr!("HCR_EL2", t.hcr);
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("isb", options(nostack, preserves_flags)) };
}

/// Back to the host (`__deactivate_traps`), then save the guest's EL1 state
/// and switch its virtual timer off so it cannot interrupt the host.
fn save(s: &mut SysRegs, h: &HostCtl) {
    // SAFETY: a barrier (erratum 1165522/1530923: the guest regime is
    // done with before the host's comes back).
    unsafe { core::arch::asm!("isb", options(nostack, preserves_flags)) };
    wr!("HCR_EL2", h.hcr);
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("isb", options(nostack, preserves_flags)) };
    wr!("CNTVOFF_EL2", 0);
    wr!("CNTHCTL_EL2", h.cnthctl);
    wr!("MDCR_EL2", h.mdcr);
    s.sctlr = rd!("S3_5_C1_C0_0");
    s.cpacr = rd!("S3_5_C1_C0_2");
    s.ttbr0 = rd!("S3_5_C2_C0_0");
    s.ttbr1 = rd!("S3_5_C2_C0_1");
    s.tcr = rd!("S3_5_C2_C0_2");
    s.esr = rd!("S3_5_C5_C2_0");
    s.afsr0 = rd!("S3_5_C5_C1_0");
    s.afsr1 = rd!("S3_5_C5_C1_1");
    s.far = rd!("S3_5_C6_C0_0");
    s.mair = rd!("S3_5_C10_C2_0");
    s.amair = rd!("S3_5_C10_C3_0");
    s.vbar = rd!("S3_5_C12_C0_0");
    s.contextidr = rd!("S3_5_C13_C0_1");
    s.cntkctl = rd!("S3_5_C14_C1_0");
    s.spsr = rd!("S3_5_C4_C0_0");
    s.elr = rd!("S3_5_C4_C0_1");
    s.sp_el1 = rd!("SP_EL1");
    s.par = rd!("PAR_EL1");
    s.csselr = rd!("CSSELR_EL1");
    s.tpidr_el0 = rd!("TPIDR_EL0");
    s.tpidrro_el0 = rd!("TPIDRRO_EL0");
    s.cntv_ctl = rd!("S3_5_C14_C3_1");
    s.cntv_cval = rd!("S3_5_C14_C3_2");
    wr!("S3_5_C14_C3_1", 0);
}

/// What the exit left in the syndrome registers.
#[derive(Clone, Copy, Default)]
pub struct Fault {
    pub esr: u64,
    pub far: u64,
    /// HPFAR_EL2 of a stage-2 abort, `None` when the guest's stage 1 no
    /// longer translates FAR and the guest should simply retry.
    pub hpfar: Option<u64>,
    /// DISR_EL1 after the ESB at the vector: an SError the guest caused.
    pub disr: u64,
}

const EC_IABT_LOW: u64 = 0x20;
const EC_DABT_LOW: u64 = 0x24;
const ESR_S1PTW: u64 = 1 << 7;
const FSC_TYPE: u64 = 0x3c;
const FSC_FAULT: u64 = 0x04;
const FSC_ACCESS: u64 = 0x08;
const FSC_PERM: u64 = 0x0c;
const FSC_FAULT_LM1: u64 = 0x2b;
const FSC_EXCL_ATOMIC: u64 = 0x35;
const PAR_F: u64 = 1;
/// PAR_EL1.PA, bits 51:12.
const PAR_PA: u64 = ((1 << 52) - 1) & !0xfff;

/// `__hpfar_valid`: the architecture fills HPFAR_EL2 for translation,
/// access flag and exclusive-atomic faults and for a permission fault on
/// a stage-1 table walk; for any other abort it is UNKNOWN.
fn hpfar_valid(esr: u64) -> bool {
    let fsc = esr & 0x3f;
    let translation = fsc & FSC_TYPE == FSC_FAULT || fsc == FSC_FAULT_LM1;
    let access = fsc & FSC_TYPE == FSC_ACCESS;
    let perm = fsc & FSC_TYPE == FSC_PERM;
    translation || access || (esr & ESR_S1PTW != 0 && perm) || fsc == FSC_EXCL_ATOMIC
}

/// `__get_fault_info` / `__translate_far_to_hpfar`: HPFAR_EL2, or the IPA
/// of FAR resolved through the guest's stage 1 when HPFAR_EL2 is UNKNOWN.
/// Runs while the guest's EL1 regime and HCR_EL2 are still in place.
fn fault_hpfar(esr: u64, far: u64) -> Option<u64> {
    let ec = esr >> 26;
    if (ec != EC_IABT_LOW && ec != EC_DABT_LOW) || hpfar_valid(esr) {
        return Some(rd!("HPFAR_EL2"));
    }
    // Stage 1 already checked the access rights, so the EL1 regime with a
    // read serves for EL0 and EL1 accesses alike. PAR_EL1 is the guest's
    // and not saved yet.
    let par = rd!("PAR_EL1");
    // SAFETY: an address translation of the guest's own regime; it only
    // writes PAR_EL1, which is restored below.
    unsafe { core::arch::asm!("at s1e1r, {}", "isb", in(reg) far, options(nostack, preserves_flags)) };
    let tmp = rd!("PAR_EL1");
    wr!("PAR_EL1", par);
    if tmp & PAR_F != 0 {
        return None;
    }
    Some(((tmp & PAR_PA) >> 12) << 4)
}

/// Run the guest once. Host interrupts are masked by the caller and stay
/// masked; the exit kind and syndrome come back.
pub fn run(regs: &mut GuestRegs, host: &mut HostSave, s: &mut SysRegs, t: &Traps, h: &HostCtl, ras: bool) -> (u64, Fault) {
    load(s, t);
    // SAFETY: `load` set up the guest's EL1 context and traps on this core;
    // `regs` and `host` stay borrowed across the call, and the exit path
    // restores every register the AAPCS64 keeps.
    let kind = unsafe { npk_guest_enter(regs, host) };
    let esr = rd!("ESR_EL2");
    let far = rd!("FAR_EL2");
    let f = Fault {
        esr,
        far,
        hpfar: fault_hpfar(esr, far),
        disr: if ras { rd!("S3_0_C12_C1_1") } else { 0 },
    };
    if f.disr != 0 {
        wr!("S3_0_C12_C1_1", 0);
    }
    save(s, h);
    (kind, f)
}
