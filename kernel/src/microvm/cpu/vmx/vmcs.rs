//! VMCS field setup — Phase 12.1.0d-1 / 12.1.0d-2b.
//!
//! Provides VMWRITE/VMREAD wrappers, the SDM Appendix-B field
//! encodings we need, and the host-state / guest-state / execution-
//! control / VMLAUNCH pipeline that runs after VMPTRLD inside VMX
//! root mode.
//!
//! 12.1.0d-1 (shipped, NUC-validated):
//!   - All HOST_* fields written + read back to validate the
//!     VMWRITE / VMREAD pipe and the host-state math.
//!
//! 12.1.0d-2b (this file, post-NUC fix v0.96.0):
//!   - Long-mode flat-segment guest with shared CR3 (no EPT). All
//!     GUEST_* fields written.
//!   - Pin/Proc/Entry/Exit execution controls computed via the
//!     allowed-0 / allowed-1 mask MSRs.
//!   - `launch_test()` overrides HOST_RIP/HOST_RSP just-in-time to a
//!     resume label inside its own asm! block, runs VMLAUNCH; the
//!     guest hits `hlt` (HLT-exiting=1), VM-exit fires, the CPU
//!     loads host state and lands at the resume label. We VMREAD
//!     VM_EXIT_REASON and return it.
//!
//! Reference: Intel SDM Vol. 3C §24 (Virtual-Machine Control
//! Structures), §26.2-§26.4 (Host/Guest State Checks, Loading on
//! VM Entry), §27 (VM Exits), Appendix B (Field Encoding in VMCS).

use super::rdmsr;

// ── VMCS field encodings (SDM Appendix B) ──────────────────────────
// Only the host-state set we touch in 12.1.0d-1 plus VM_EXIT_REASON
// for the trampoline.

// 16-bit host-state.
const HOST_ES_SELECTOR: u64 = 0x0C00;
const HOST_CS_SELECTOR: u64 = 0x0C02;
const HOST_SS_SELECTOR: u64 = 0x0C04;
const HOST_DS_SELECTOR: u64 = 0x0C06;
const HOST_FS_SELECTOR: u64 = 0x0C08;
const HOST_GS_SELECTOR: u64 = 0x0C0A;
const HOST_TR_SELECTOR: u64 = 0x0C0C;

// 64-bit host-state.
const HOST_IA32_EFER: u64 = 0x2C02;

// 32-bit host-state.
const HOST_IA32_SYSENTER_CS: u64 = 0x4C00;

// Natural-width host-state.
const HOST_CR0: u64 = 0x6C00;
const HOST_CR3: u64 = 0x6C02;
const HOST_CR4: u64 = 0x6C04;
const HOST_FS_BASE: u64 = 0x6C06;
const HOST_GS_BASE: u64 = 0x6C08;
const HOST_TR_BASE: u64 = 0x6C0A;
const HOST_GDTR_BASE: u64 = 0x6C0C;
const HOST_IDTR_BASE: u64 = 0x6C0E;
const HOST_IA32_SYSENTER_ESP: u64 = 0x6C10;
const HOST_IA32_SYSENTER_EIP: u64 = 0x6C12;
const HOST_RSP: u64 = 0x6C14;
const HOST_RIP: u64 = 0x6C16;

// 16-bit guest-state.
const GUEST_ES_SELECTOR: u64 = 0x0800;
const GUEST_CS_SELECTOR: u64 = 0x0802;
const GUEST_SS_SELECTOR: u64 = 0x0804;
const GUEST_DS_SELECTOR: u64 = 0x0806;
const GUEST_FS_SELECTOR: u64 = 0x0808;
const GUEST_GS_SELECTOR: u64 = 0x080A;
const GUEST_LDTR_SELECTOR: u64 = 0x080C;
const GUEST_TR_SELECTOR: u64 = 0x080E;

// 64-bit guest-state.
const VMCS_LINK_POINTER: u64 = 0x2800;
const GUEST_IA32_EFER: u64 = 0x2806;

// 32-bit guest-state.
const GUEST_ES_LIMIT: u64 = 0x4800;
const GUEST_CS_LIMIT: u64 = 0x4802;
const GUEST_SS_LIMIT: u64 = 0x4804;
const GUEST_DS_LIMIT: u64 = 0x4806;
const GUEST_FS_LIMIT: u64 = 0x4808;
const GUEST_GS_LIMIT: u64 = 0x480A;
const GUEST_LDTR_LIMIT: u64 = 0x480C;
const GUEST_TR_LIMIT: u64 = 0x480E;
const GUEST_GDTR_LIMIT: u64 = 0x4810;
const GUEST_IDTR_LIMIT: u64 = 0x4812;
const GUEST_ES_AR_BYTES: u64 = 0x4814;
const GUEST_CS_AR_BYTES: u64 = 0x4816;
const GUEST_SS_AR_BYTES: u64 = 0x4818;
const GUEST_DS_AR_BYTES: u64 = 0x481A;
const GUEST_FS_AR_BYTES: u64 = 0x481C;
const GUEST_GS_AR_BYTES: u64 = 0x481E;
const GUEST_LDTR_AR_BYTES: u64 = 0x4820;
const GUEST_TR_AR_BYTES: u64 = 0x4822;
const GUEST_INTERRUPTIBILITY_INFO: u64 = 0x4824;
const GUEST_ACTIVITY_STATE: u64 = 0x4826;
const GUEST_SYSENTER_CS: u64 = 0x482A;

// Natural-width guest-state.
const GUEST_CR0: u64 = 0x6800;
const GUEST_CR3: u64 = 0x6802;
const GUEST_CR4: u64 = 0x6804;
const GUEST_ES_BASE: u64 = 0x6806;
const GUEST_CS_BASE: u64 = 0x6808;
const GUEST_SS_BASE: u64 = 0x680A;
const GUEST_DS_BASE: u64 = 0x680C;
const GUEST_FS_BASE: u64 = 0x680E;
const GUEST_GS_BASE: u64 = 0x6810;
const GUEST_LDTR_BASE: u64 = 0x6812;
const GUEST_TR_BASE: u64 = 0x6814;
const GUEST_GDTR_BASE: u64 = 0x6816;
const GUEST_IDTR_BASE: u64 = 0x6818;
const GUEST_DR7: u64 = 0x681A;
const GUEST_RSP: u64 = 0x681C;
const GUEST_RIP: u64 = 0x681E;
const GUEST_RFLAGS: u64 = 0x6820;
const GUEST_PENDING_DBG_EXCEPTIONS: u64 = 0x6822;
const GUEST_SYSENTER_ESP: u64 = 0x6824;
const GUEST_SYSENTER_EIP: u64 = 0x6826;

// Execution controls (32-bit).
const PIN_BASED_VM_EXEC_CONTROL: u64 = 0x4000;
const CPU_BASED_VM_EXEC_CONTROL: u64 = 0x4002;
const EXCEPTION_BITMAP: u64 = 0x4004;
const CR3_TARGET_COUNT: u64 = 0x400A;
const VM_EXIT_CONTROLS: u64 = 0x400C;
const VM_EXIT_MSR_STORE_COUNT: u64 = 0x400E;
const VM_EXIT_MSR_LOAD_COUNT: u64 = 0x4010;
const VM_ENTRY_CONTROLS: u64 = 0x4012;
const VM_ENTRY_MSR_LOAD_COUNT: u64 = 0x4014;
const VM_ENTRY_INTR_INFO_FIELD: u64 = 0x4016;
const SECONDARY_VM_EXEC_CONTROL: u64 = 0x401E;

// 64-bit control.
const IO_BITMAP_A_FULL: u64 = 0x2000;
const IO_BITMAP_B_FULL: u64 = 0x2002;
const MSR_BITMAPS_FULL: u64 = 0x2004;
const EPT_POINTER: u64 = 0x201A;

// Natural-width VM-exit info.
#[allow(dead_code)] // SDM Vol3 §28.3 field; reserved for fault-path decoding
const VM_EXIT_QUALIFICATION: u64 = 0x6400;
const VM_EXIT_GUEST_LINEAR_ADDR: u64 = 0x640A;

// 64-bit VM-exit info.
const VM_EXIT_GUEST_PHYS_ADDR: u64 = 0x2400;

// Natural-width controls.
const CR0_GUEST_HOST_MASK: u64 = 0x6000;
const CR4_GUEST_HOST_MASK: u64 = 0x6002;
const CR0_READ_SHADOW: u64 = 0x6004;
const CR4_READ_SHADOW: u64 = 0x6006;

// VM-exit information (read-only).
const VM_INSTRUCTION_ERROR: u64 = 0x4400;
const VM_EXIT_INTR_INFO: u64 = 0x4404;
const VM_EXIT_INTR_ERROR_CODE: u64 = 0x4406;
const IDT_VECTORING_INFO_FIELD: u64 = 0x4408;
const IDT_VECTORING_ERROR_CODE: u64 = 0x440A;
const VM_EXIT_INSTRUCTION_LEN: u64 = 0x440C;
// VM_EXIT_REASON encoding 0x4402 is referenced as a literal inside
// the run_guest_once asm! block (Intel-syntax `mov rcx, 0x4402`).

// VMX capability MSRs for control allowed-0 / allowed-1 masking.
const IA32_VMX_BASIC: u32 = 0x480;
const IA32_VMX_PINBASED_CTLS: u32 = 0x481;
const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;
const IA32_VMX_EXIT_CTLS: u32 = 0x483;
const IA32_VMX_ENTRY_CTLS: u32 = 0x484;
const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;
// "TRUE" variants exist when IA32_VMX_BASIC bit 55 = 1 (Alder Lake-N
// definitely has them). The TRUE MSRs relax "default-1" bits in the
// classic MSRs to "may-be-0", letting us actually clear bits like
// CR3-load-exiting (no point with EPT) and ACK_INTR_ON_EXIT. KVM uses
// these universally; we did not, which forced our control fields into
// a state that diverges from a minimal-VMX setup and from SVM
// semantics. Discovered while chasing the bare-metal-NUC reason-33
// VM-entry failure (v0.172.61+).
const IA32_VMX_TRUE_PINBASED_CTLS: u32 = 0x48D;
const IA32_VMX_TRUE_PROCBASED_CTLS: u32 = 0x48E;
const IA32_VMX_TRUE_EXIT_CTLS: u32 = 0x48F;
const IA32_VMX_TRUE_ENTRY_CTLS: u32 = 0x490;

/// True if IA32_VMX_BASIC bit 55 reports TRUE-controls support.
/// Cached after first call to avoid repeated rdmsr.
fn vmx_has_true_controls() -> bool {
    use core::sync::atomic::{AtomicU8, Ordering};
    static CACHED: AtomicU8 = AtomicU8::new(0xFF); // 0xFF = uncomputed
    let v = CACHED.load(Ordering::Relaxed);
    if v != 0xFF {
        return v != 0;
    }
    // SAFETY: IA32_VMX_BASIC is architectural once CR4.VMXE supported.
    let basic = unsafe { rdmsr(IA32_VMX_BASIC) };
    let has = (basic >> 55) & 1 != 0;
    CACHED.store(has as u8, Ordering::Relaxed);
    has
}

/// Pick the right control MSR — TRUE variant when supported, classic
/// when not. The TRUE variants relax default-1 bits so we can choose
/// to clear them (e.g. CR3-load-exiting with EPT enabled).
fn pinbased_ctls_msr()   -> u32 { if vmx_has_true_controls() { IA32_VMX_TRUE_PINBASED_CTLS }   else { IA32_VMX_PINBASED_CTLS } }
fn procbased_ctls_msr()  -> u32 { if vmx_has_true_controls() { IA32_VMX_TRUE_PROCBASED_CTLS }  else { IA32_VMX_PROCBASED_CTLS } }
fn exit_ctls_msr()       -> u32 { if vmx_has_true_controls() { IA32_VMX_TRUE_EXIT_CTLS }       else { IA32_VMX_EXIT_CTLS } }
fn entry_ctls_msr()      -> u32 { if vmx_has_true_controls() { IA32_VMX_TRUE_ENTRY_CTLS }      else { IA32_VMX_ENTRY_CTLS } }

// Architectural MSRs we mirror into host-state.
const IA32_EFER: u32 = 0xC000_0080;
const IA32_FS_BASE: u32 = 0xC000_0100;
const IA32_GS_BASE: u32 = 0xC000_0101;
const IA32_SYSENTER_CS: u32 = 0x174;
const IA32_SYSENTER_ESP: u32 = 0x175;
const IA32_SYSENTER_EIP: u32 = 0x176;

const RFLAGS_CF: u64 = 1 << 0;
const RFLAGS_ZF: u64 = 1 << 6;

// ── VMWRITE / VMREAD primitives ────────────────────────────────────

/// VMWRITE the given field with `value`. Caller must be in VMX root
/// mode with a current VMCS loaded (VMPTRLD'd).
///
/// On VMfailInvalid (CF=1) — no current VMCS — or VMfailValid (ZF=1)
/// — invalid field encoding for the loaded VMCS — returns `Err`.
pub(super) fn vmwrite(field: u64, value: u64) -> Result<(), &'static str> {
    let rflags: u64;
    // SAFETY: VMWRITE has no architectural side effects beyond the
    // VMCS state and RFLAGS. Caller-guaranteed VMX root mode +
    // current VMCS. pushfq/pop touches the stack.
    unsafe {
        core::arch::asm!(
            "vmwrite {field}, {val}",
            "pushfq",
            "pop {flags}",
            field = in(reg) field,
            val = in(reg) value,
            flags = lateout(reg) rflags,
        );
    }
    if rflags & RFLAGS_CF != 0 {
        return Err("VMWRITE VMfailInvalid (no current VMCS)");
    }
    if rflags & RFLAGS_ZF != 0 {
        return Err("VMWRITE VMfailValid (bad field encoding)");
    }
    Ok(())
}

/// VMREAD the given field. Same VMX-root-mode + current-VMCS
/// preconditions as `vmwrite`.
pub(super) fn vmread(field: u64) -> Result<u64, &'static str> {
    let value: u64;
    let rflags: u64;
    // SAFETY: as `vmwrite`. VMREAD writes only into the destination
    // register and RFLAGS.
    unsafe {
        core::arch::asm!(
            "vmread {val}, {field}",
            "pushfq",
            "pop {flags}",
            val = lateout(reg) value,
            field = in(reg) field,
            flags = lateout(reg) rflags,
        );
    }
    if rflags & RFLAGS_CF != 0 {
        return Err("VMREAD VMfailInvalid (no current VMCS)");
    }
    if rflags & RFLAGS_ZF != 0 {
        return Err("VMREAD VMfailValid (bad field encoding)");
    }
    Ok(value)
}

/// VMWRITE then VMREAD; assert the round-trip matches. Catches both
/// the VMWRITE/VMREAD path *and* the silent truncation cases (a few
/// host-state fields are 32-bit on the wire even though we pass
/// `u64`).
fn vmwrite_check(field: u64, value: u64, name: &'static str) -> Result<(), &'static str> {
    vmwrite(field, value)?;
    let back = vmread(field)?;
    if back != value {
        // Stash the field name so the caller can report which one
        // tripped. Static-string identity is enough — we only debug
        // by reading the source.
        let _ = name;
        return Err("VMREAD round-trip mismatch on host-state field");
    }
    Ok(())
}

// ── Current host-CPU snapshot ──────────────────────────────────────

/// Read CR0/CR3/CR4 and the segment selectors we copy into the host
/// area. Done once before the VMWRITE storm so we get a consistent
/// snapshot — the trampoline never returns, so these values describe
/// "the kernel state we want to wake up in" if a VM-exit ever fires.
struct HostSnapshot {
    cr0: u64,
    cr3: u64,
    cr4: u64,
    cs: u16,
    ss: u16,
    ds: u16,
    es: u16,
    fs: u16,
    gs: u16,
    tr: u16,
    gdtr_base: u64,
    idtr_base: u64,
}

fn snapshot_host() -> HostSnapshot {
    let cr0: u64;
    let cr3: u64;
    let cr4: u64;
    let cs: u16;
    let ss: u16;
    let ds: u16;
    let es: u16;
    let fs: u16;
    let gs: u16;
    let tr: u16;
    // SDT/IDT pseudo-descriptors: 10 bytes (limit:2 + base:8) in long mode.
    let mut gdtr_buf: [u8; 10] = [0; 10];
    let mut idtr_buf: [u8; 10] = [0; 10];

    // SAFETY: pure register reads, no faulting paths. `str` is legal
    // in long mode regardless of whether a TSS was actually loaded
    // (returns 0 if none). sgdt/sidt write 10 bytes to the operand.
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
        core::arch::asm!("mov {:x}, cs", out(reg) cs, options(nostack, preserves_flags));
        core::arch::asm!("mov {:x}, ss", out(reg) ss, options(nostack, preserves_flags));
        core::arch::asm!("mov {:x}, ds", out(reg) ds, options(nostack, preserves_flags));
        core::arch::asm!("mov {:x}, es", out(reg) es, options(nostack, preserves_flags));
        core::arch::asm!("mov {:x}, fs", out(reg) fs, options(nostack, preserves_flags));
        core::arch::asm!("mov {:x}, gs", out(reg) gs, options(nostack, preserves_flags));
        core::arch::asm!("str {:x}", out(reg) tr, options(nostack, preserves_flags));
        core::arch::asm!("sgdt [{}]", in(reg) &mut gdtr_buf, options(nostack, preserves_flags));
        core::arch::asm!("sidt [{}]", in(reg) &mut idtr_buf, options(nostack, preserves_flags));
    }

    let gdtr_base = u64::from_le_bytes([
        gdtr_buf[2], gdtr_buf[3], gdtr_buf[4], gdtr_buf[5],
        gdtr_buf[6], gdtr_buf[7], gdtr_buf[8], gdtr_buf[9],
    ]);
    let idtr_base = u64::from_le_bytes([
        idtr_buf[2], idtr_buf[3], idtr_buf[4], idtr_buf[5],
        idtr_buf[6], idtr_buf[7], idtr_buf[8], idtr_buf[9],
    ]);

    HostSnapshot {
        cr0, cr3, cr4,
        cs, ss, ds, es, fs, gs, tr,
        gdtr_base, idtr_base,
    }
}

/// Resolve the TSS base from the GDT entry that TR selects. In long
/// mode a TSS descriptor is 16 bytes (system-segment with the upper
/// 32 bits of base in bytes 8..12). When TR is 0 — the boot-time
/// state on this kernel today — there is no TSS and we report base 0.
/// 12.1.0d-2 will install a real TSS before VMLAUNCH.
fn resolve_tr_base(tr_selector: u16, gdtr_base: u64) -> u64 {
    let index = (tr_selector >> 3) as u64;
    if index == 0 {
        return 0;
    }
    let desc_addr = gdtr_base + index * 8;
    // SAFETY: GDT is identity-mapped kernel rodata. We only read.
    // If the selector were bogus we could read garbage, but that is
    // already a kernel bug elsewhere — we trust `str`'s output.
    unsafe {
        let lo = core::ptr::read_volatile(desc_addr as *const u64);
        let hi = core::ptr::read_volatile((desc_addr + 8) as *const u64);
        // base[15:0] in lo[16..32], base[23:16] in lo[32..40],
        // base[31:24] in lo[56..64], base[63:32] in hi[0..32].
        let base_lo = ((lo >> 16) & 0xFFFF)
            | (((lo >> 32) & 0xFF) << 16)
            | (((lo >> 56) & 0xFF) << 24);
        let base_hi = hi & 0xFFFF_FFFF;
        base_lo | (base_hi << 32)
    }
}

// ── Host-state setup ───────────────────────────────────────────────

/// Write the full host-state subset and read every field back. Runs
/// inside VMX root mode after VMPTRLD. On any mismatch returns the
/// failing-step error string; the caller still issues VMXOFF.
///
/// `host_rsp` is captured at the call site (one frame above) so it
/// describes a slot that is still live when the function returns.
/// 12.1.0d-2 will replace this with a dedicated VM-exit stack.
pub(super) fn setup_host_state(host_rsp: u64) -> Result<(), &'static str> {
    let snap = snapshot_host();
    let tr_base = resolve_tr_base(snap.tr, snap.gdtr_base);

    // Selector fields must have TI=0 and RPL=0 per SDM §26.2.3 — we
    // mask the bottom 3 bits so a stray RPL doesn't cause VMLAUNCH
    // to fail later on. (Today CS=0x08, DS/SS/ES=0x10, FS/GS=0, TR=0
    // — all already RPL-0; the mask is defence in depth.)
    let cs = (snap.cs & 0xFFF8) as u64;
    let ss = (snap.ss & 0xFFF8) as u64;
    let ds = (snap.ds & 0xFFF8) as u64;
    let es = (snap.es & 0xFFF8) as u64;
    let fs = (snap.fs & 0xFFF8) as u64;
    let gs = (snap.gs & 0xFFF8) as u64;
    let tr = (snap.tr & 0xFFF8) as u64;

    // Architectural MSRs that mirror into host-state.
    // SAFETY: all four MSRs are architectural since SYSENTER (P6) /
    // EFER (AMD64) — always present on x86_64.
    let efer = unsafe { rdmsr(IA32_EFER) };
    let fs_base = unsafe { rdmsr(IA32_FS_BASE) };
    let gs_base = unsafe { rdmsr(IA32_GS_BASE) };
    let sysenter_cs = unsafe { rdmsr(IA32_SYSENTER_CS) };
    let sysenter_esp = unsafe { rdmsr(IA32_SYSENTER_ESP) };
    let sysenter_eip = unsafe { rdmsr(IA32_SYSENTER_EIP) };

    let host_rip = exit_trampoline_addr();

    // Control registers.
    vmwrite_check(HOST_CR0, snap.cr0, "HOST_CR0")?;
    vmwrite_check(HOST_CR3, snap.cr3, "HOST_CR3")?;
    vmwrite_check(HOST_CR4, snap.cr4, "HOST_CR4")?;

    // Segment selectors.
    vmwrite_check(HOST_CS_SELECTOR, cs, "HOST_CS_SELECTOR")?;
    vmwrite_check(HOST_SS_SELECTOR, ss, "HOST_SS_SELECTOR")?;
    vmwrite_check(HOST_DS_SELECTOR, ds, "HOST_DS_SELECTOR")?;
    vmwrite_check(HOST_ES_SELECTOR, es, "HOST_ES_SELECTOR")?;
    vmwrite_check(HOST_FS_SELECTOR, fs, "HOST_FS_SELECTOR")?;
    vmwrite_check(HOST_GS_SELECTOR, gs, "HOST_GS_SELECTOR")?;
    vmwrite_check(HOST_TR_SELECTOR, tr, "HOST_TR_SELECTOR")?;

    // Segment / table bases.
    vmwrite_check(HOST_FS_BASE, fs_base, "HOST_FS_BASE")?;
    vmwrite_check(HOST_GS_BASE, gs_base, "HOST_GS_BASE")?;
    vmwrite_check(HOST_TR_BASE, tr_base, "HOST_TR_BASE")?;
    vmwrite_check(HOST_GDTR_BASE, snap.gdtr_base, "HOST_GDTR_BASE")?;
    vmwrite_check(HOST_IDTR_BASE, snap.idtr_base, "HOST_IDTR_BASE")?;

    // SYSENTER MSRs.
    vmwrite_check(HOST_IA32_SYSENTER_CS, sysenter_cs, "HOST_IA32_SYSENTER_CS")?;
    vmwrite_check(HOST_IA32_SYSENTER_ESP, sysenter_esp, "HOST_IA32_SYSENTER_ESP")?;
    vmwrite_check(HOST_IA32_SYSENTER_EIP, sysenter_eip, "HOST_IA32_SYSENTER_EIP")?;

    // EFER (long-mode bit lives here; required when "load IA32_EFER"
    // VM-exit control is set, harmless otherwise).
    vmwrite_check(HOST_IA32_EFER, efer, "HOST_IA32_EFER")?;

    // RIP / RSP last so a failure earlier doesn't leave a stale RSP
    // pointing into a dead frame.
    vmwrite_check(HOST_RSP, host_rsp, "HOST_RSP")?;
    vmwrite_check(HOST_RIP, host_rip, "HOST_RIP")?;

    Ok(())
}

// ── HOST_RIP placeholder ───────────────────────────────────────────

// HOST_RIP needs *some* canonical kernel-text address at the time
// `setup_host_state` runs — but `launch_test()` always overrides it
// just-in-time (with the runtime address of its in-line resume label)
// before VMLAUNCH. The placeholder here exists only so the field is
// canonical between the host-state pass and the launch.
fn exit_trampoline_addr() -> u64 {
    setup_host_state as *const () as usize as u64
}

// ── I/O bitmaps ────────────────────────────────────────────────────

/// Allocate two 4-KB pages for IO_BITMAP_A (ports 0x0000-0x7FFF) and
/// IO_BITMAP_B (ports 0x8000-0xFFFF), fill them with 0xFF so every
/// I/O instruction the guest executes triggers a VM-exit.
///
/// Why trap-everything: a real-mode/prot-mode guest like Linux
/// will probe PCI config (0xCF8/0xCFC), keyboard (0x60/0x64), CMOS
/// (0x70/0x71), PIC (0x20/0xA0), PIT (0x40-0x43) and so on. Without
/// trapping, those I/O instructions execute natively against the
/// host's real hardware on the NUC — quietly corrupting host
/// state. We trap everything and ignore (return 0 for IN, no-op
/// for OUT) anything our handler doesn't explicitly understand.
///
/// Returns (io_bitmap_a_phys, io_bitmap_b_phys). Frames leaked
/// (same lifecycle as VMXON / VMCS / EPT regions).
fn allocate_and_populate_io_bitmaps() -> Result<(u64, u64), &'static str> {
    use crate::mm::memory;

    let bitmap_a = memory::allocate_frame().ok_or("OOM: IO_BITMAP_A")?;
    let bitmap_b = memory::allocate_frame().ok_or("OOM: IO_BITMAP_B")?;

    // SAFETY: identity-mapped, freshly allocated, exclusive.
    unsafe {
        core::ptr::write_bytes(bitmap_a as *mut u8, 0xFF, 4096);
        core::ptr::write_bytes(bitmap_b as *mut u8, 0xFF, 4096);
    }

    Ok((bitmap_a, bitmap_b))
}

/// Decode a VM-exit-qualification value from an I/O instruction
/// VM-exit (basic reason 30) per SDM §27.2.1 Table 27-5. Returns
/// (port, direction_in, size_bytes).
pub fn decode_io_exit_qualification(qual: u64) -> (u16, bool, u8) {
    let port = ((qual >> 16) & 0xFFFF) as u16;
    let direction_in = (qual >> 3) & 1 != 0;
    let size_bytes = ((qual & 7) + 1) as u8;
    (port, direction_in, size_bytes)
}

/// Allocate a 4-KB MSR bitmap, fill with zeros so RDMSR/WRMSR don't
/// VM-exit at all. Native MSR access in the guest is fine for our
/// 12.1.1c milestone — the MSRs that matter for guest correctness
/// (EFER, FS_BASE, GS_BASE) have GUEST_IA32_* VMCS fields the CPU
/// auto-manages on entry/exit. Returns the bitmap phys addr.
///
/// Why bother with a zero bitmap at all: with use-MSR-bitmaps=0,
/// EVERY RDMSR/WRMSR exits — Linux's startup hits dozens of MSR
/// reads in the first few microseconds and we'd drown in exit
/// dispatch.
fn allocate_zero_msr_bitmap() -> Result<u64, &'static str> {
    use crate::mm::memory;
    let bitmap = memory::allocate_frame().ok_or("OOM: MSR bitmap")?;
    // SAFETY: identity-mapped, freshly allocated, exclusive.
    unsafe { core::ptr::write_bytes(bitmap as *mut u8, 0, 4096); }

    // Intel parity #2: intercept IA32_APIC_BASE (0x1B) reads AND writes so
    // the EXIT_MSR handler emulates it (report LAPIC enabled@0xFEE00000 with
    // the BSP bit). The WRITE intercept is critical — with a zero bitmap a
    // guest WRMSR 0x1B would pass straight to the HOST core's APIC base
    // register. Layout (SDM §25.6.9): read-low region @0x000, write-low @
    // 0x800; MSR 0x1B → byte 3, bit 3. Gated on the LAPIC flag (vmx ⇒ Intel
    // here) so the `nolapic` rollback keeps the validated zero bitmap.
    if crate::microvm::cpu::GUEST_LAPIC && crate::microvm::cpu::VMX_GUEST_LAPIC {
        // SAFETY: bytes 3 and 0x803 are within the 4 KB bitmap we own.
        unsafe {
            *((bitmap + 3) as *mut u8) |= 1 << 3;        // RDMSR 0x1B exits
            *((bitmap + 0x803) as *mut u8) |= 1 << 3;    // WRMSR 0x1B exits
        }
    }
    Ok(bitmap)
}

/// Run CPUID on the host with the guest's input leaf+subleaf,
/// return (eax, ebx, ecx, edx). LLVM reserves rbx, so we save it
/// across the cpuid instruction.
pub fn host_cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    // SAFETY: CPUID has no privileged side effects beyond what the
    // architectural docs spell out (returns CPU info in A/B/C/D).
    // rbx is preserved via push/pop because LLVM forbids using it
    // as a clobbered or output register directly.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            lateout("esi") ebx,
            lateout("edx") edx,
            options(nostack, preserves_flags),
        );
    }
    (eax, ebx, ecx, edx)
}

// ── Execution controls ─────────────────────────────────────────────

// CPU-based control bits we care about.
const CPU_HLT_EXITING: u32 = 1 << 7;
const CPU_USE_IO_BITMAPS: u32 = 1 << 25;
const CPU_USE_MSR_BITMAPS: u32 = 1 << 28;
const CPU_ACTIVATE_SECONDARY: u32 = 1 << 31;

// Secondary control bits.
const SEC_ENABLE_EPT: u32 = 1 << 1;
const SEC_UNRESTRICTED_GUEST: u32 = 1 << 7;
/// Bit 3: enable native RDTSCP/RDPID execution in guest. Without
/// this, RDTSCP raises #UD even when CPUID indicates support —
/// Linux's `read_tsc` (clocksource switch) uses RDTSCP and faults
/// during clocksource init otherwise.
const SEC_ENABLE_RDTSCP: u32 = 1 << 3;
/// Bit 12: enable native INVPCID execution in guest. Without this,
/// INVPCID raises #UD even when CPUID indicates support — Linux's
/// `native_flush_tlb_global` uses INVPCID and faults during
/// setup_arch otherwise.
const SEC_ENABLE_INVPCID: u32 = 1 << 12;
/// Bit 20: enable native XSAVES/XRSTORS in guest. Without this,
/// XSAVES raises #UD even when CPUID Leaf 0xD subleaf 1:EAX[3]
/// indicates support — Linux uses XSAVES for context switch when
/// supervisor xstates are present (CET_S = XFEATURE bit 12, which
/// Alpine virt has in the active set 0x1807). First userspace
/// context switch would fault otherwise.
const SEC_ENABLE_XSAVES: u32 = 1 << 20;
/// Bit 26: enable native UMONITOR/UMWAIT/TPAUSE (WAITPKG) in guest.
/// Without this, TPAUSE raises #UD even when CPUID Leaf 7:ECX[5]
/// indicates support — Linux's `delay_halt_tpause` uses TPAUSE for
/// short kernel-mode delays (e.g. i8042 probe) and faults otherwise.
const SEC_ENABLE_USER_WAIT_PAUSE: u32 = 1 << 26;

// Pin-based control bits.
/// Bit 0: External-interrupt exiting. When 1, host-targeted external
/// interrupts cause VM-exits (basic reason 1) instead of being
/// delivered to the guest's IDT. Without this, host LAPIC/PIC IRQs
/// arriving during guest run get acknowledged by the real APIC into
/// the guest's IDT — but the guest's EOI write to the LAPIC MMIO
/// goes through EPT into our scratch page (not the real LAPIC), so
/// real-LAPIC ISR stays set forever and the host stops receiving
/// interrupts after VMXOFF (= post-`microvm linux` keyboard freeze
/// observed on N100, 2026-05-02).
const PIN_EXT_INTR_EXITING: u32 = 1 << 0;

// VM-entry control bits.
const ENTRY_IA32E_MODE_GUEST: u32 = 1 << 9;
const ENTRY_LOAD_IA32_EFER: u32 = 1 << 15;

// VM-exit control bits.
const EXIT_HOST_ADDR_SPACE_SIZE: u32 = 1 << 9;
const EXIT_SAVE_IA32_EFER: u32 = 1 << 20;
const EXIT_LOAD_IA32_EFER: u32 = 1 << 21;

/// Compute a control field's value from the desired-set, applying
/// the must-be-0 / must-be-1 mask MSR per SDM §A.3.1: low 32 bits
/// are allowed-0 (must be 1 if set there), high 32 bits are
/// allowed-1 (must be 0 if clear there). Result is `(desired |
/// allowed_0) & allowed_1`.
fn fixed_ctrl(desired: u32, msr: u32) -> u32 {
    // SAFETY: VMX control MSRs are architectural once the CPU has
    // VMX (gated by `probe()` upstream).
    let raw = unsafe { rdmsr(msr) };
    let allowed_0 = raw as u32;
    let allowed_1 = (raw >> 32) as u32;
    (desired | allowed_0) & allowed_1
}

/// Write the execution-control fields for an unrestricted real-mode
/// guest with EPT + selective I/O exiting. CPU-based: HLT-exiting +
/// use-IO-bitmaps + activate-secondary. Secondary: enable-EPT +
/// unrestricted-guest. VM-exit: host-address-space-size. VM-entry:
/// IA-32e-mode-guest stays 0.
///
/// I/O bitmaps trap accesses to port 0x80 (substrate-test stub) and
/// ports 0x3F8-0x3FF (UART COM1 — Linux's earlyprintk target). All
/// other ports pass through natively.
pub(super) fn setup_execution_controls(eptp: u64) -> Result<(), &'static str> {
    let (io_bitmap_a, io_bitmap_b) = allocate_and_populate_io_bitmaps()?;
    let msr_bitmap = allocate_zero_msr_bitmap()?;

    // Use IA32_VMX_TRUE_*_CTLS when the CPU supports them (Alder
    // Lake-N does). The TRUE variants relax classic "default-1"
    // bits — most notably CR3-load/store-exiting (bits 15/16 of
    // PROCBASED) which the classic MSR forces on but with EPT
    // active provides no benefit (the guest's CR3 is opaque to
    // the host; EPT walks it without our help). Forcing CR3-exit
    // also produces a guest-trap sequence that does NOT exist on
    // SVM (where we never trap CR3) and our trap-handler's
    // write_guest_cr3 + advance_rip leaves the GUEST_CR3 / RIP /
    // VPID-TLB combo in a state that the next VMRESUME's
    // consistency check rejects (reason 33 on bare-metal-NUC).
    // Match KVM's universal use of TRUE_*_CTLS to get a minimal
    // control set close to SVM's.
    let pin = fixed_ctrl(PIN_EXT_INTR_EXITING, pinbased_ctls_msr());
    let cpu = fixed_ctrl(
        CPU_HLT_EXITING
            | CPU_USE_IO_BITMAPS
            | CPU_USE_MSR_BITMAPS
            | CPU_ACTIVATE_SECONDARY,
        procbased_ctls_msr(),
    );
    // Secondary controls don't have a TRUE variant — IA32_VMX_PROCBASED_
    // CTLS2 is used directly. SDM Appendix A.3.3 / A.3.4.
    let secondary = fixed_ctrl(
        SEC_ENABLE_EPT
            | SEC_UNRESTRICTED_GUEST
            | SEC_ENABLE_RDTSCP
            | SEC_ENABLE_INVPCID
            | SEC_ENABLE_XSAVES
            | SEC_ENABLE_USER_WAIT_PAUSE,
        IA32_VMX_PROCBASED_CTLS2,
    );
    // EFER state management: CPU saves/loads guest EFER via VMCS
    // GUEST_IA32_EFER on each entry/exit. Without these, guest's
    // WRMSR EFER (LME=1) updates the real MSR but is lost on the
    // next entry (uncertain behaviour) and the host's EFER stays
    // unchanged across exit (also dangerous on transitioning the
    // host back to long mode).
    let entry = fixed_ctrl(ENTRY_LOAD_IA32_EFER, entry_ctls_msr());
    let exit = fixed_ctrl(
        EXIT_HOST_ADDR_SPACE_SIZE | EXIT_SAVE_IA32_EFER | EXIT_LOAD_IA32_EFER,
        exit_ctls_msr(),
    );

    vmwrite(PIN_BASED_VM_EXEC_CONTROL, pin as u64)?;
    vmwrite(CPU_BASED_VM_EXEC_CONTROL, cpu as u64)?;
    vmwrite(SECONDARY_VM_EXEC_CONTROL, secondary as u64)?;
    vmwrite(IO_BITMAP_A_FULL, io_bitmap_a)?;
    vmwrite(IO_BITMAP_B_FULL, io_bitmap_b)?;
    vmwrite(MSR_BITMAPS_FULL, msr_bitmap)?;
    vmwrite(EPT_POINTER, eptp)?;
    vmwrite(VM_ENTRY_CONTROLS, entry as u64)?;
    vmwrite(VM_EXIT_CONTROLS, exit as u64)?;

    // Inert ancillary controls — clear bitmaps + counts so VMX
    // doesn't dereference them.
    //
    // EXCEPTION_BITMAP: trap only #CP (vector 21, CET control-flow
    // protection). Linux's early boot specifically RELIES on its
    // own #PF handler (`early_idt_handler_array` → `early_make_pgtable`)
    // to build the direct-map page tables lazily — trapping #PF
    // breaks that lazy-PT mechanism (12.1.1c-3b3b6 lesson).
    //
    // #CP is different: when CR4.CET=1 inherits from host into the
    // guest and Linux hits an indirect call to a non-ENDBR target
    // (e.g. asm reboot stubs after panic), Linux's exc_control_protection
    // BUG()s and recursively re-faults. Without us trapping it, the
    // recursion runs entirely in non-root mode with no exits — host
    // CPU is stuck in VMRESUME-loop forever (no I/O, no HLT, our
    // MAX_ITERATIONS cap never fires). Trapping #CP surfaces it as
    // exit reason 0 and lets the loop terminate cleanly.
    //
    // Triple faults (basic reason 2) still surface independently of
    // EXCEPTION_BITMAP if Linux truly fails.
    const TRAP_CP: u64 = 1 << 21;
    vmwrite(EXCEPTION_BITMAP, TRAP_CP)?;
    vmwrite(CR3_TARGET_COUNT, 0)?;
    vmwrite(VM_EXIT_MSR_STORE_COUNT, 0)?;
    vmwrite(VM_EXIT_MSR_LOAD_COUNT, 0)?;
    vmwrite(VM_ENTRY_MSR_LOAD_COUNT, 0)?;
    vmwrite(VM_ENTRY_INTR_INFO_FIELD, 0)?;

    // CR0/CR4 shadowing — hide VMXE from the guest so Linux's
    // unconditional `mov cr4, rax` doesn't try to clear it (which
    // would either #GP because IA32_VMX_CR4_FIXED0 has VMXE
    // must-be-1, or break VMX operation).
    //
    // Mechanism (SDM §25.4): bits set in CR4_GUEST_HOST_MASK are
    // "host-owned". Guest CR4 reads return SHADOW for those bits,
    // real CR4 for others. Guest CR4 writes that don't change a
    // masked bit relative to shadow → no exit, real-CR4-bit
    // preserved. Writes that would change a masked bit → VM-exit
    // (guest-CR-access reason 28).
    //
    // Setting mask = VMXE only, shadow.VMXE = 0: Linux reads CR4
    // and sees VMXE=0. Linux's `mov cr4, value-with-VMXE=0` keeps
    // shadow.VMXE = 0 = matches → no exit, real CR4.VMXE stays 1.
    // Linux never tries to set VMXE=1 (it doesn't know about VMX),
    // so the exit branch never fires.
    vmwrite(CR0_GUEST_HOST_MASK, 0)?;
    vmwrite(CR4_GUEST_HOST_MASK, 1 << 13)?; // VMXE
    vmwrite(CR0_READ_SHADOW, 0)?;
    vmwrite(CR4_READ_SHADOW, 0)?;

    Ok(())
}

// ── Guest state ────────────────────────────────────────────────────

// VMCS guest segment AR-byte layout (SDM §24.4.1 Table 24-2):
//   bits 3:0  = type
//   bit  4    = S (1 = code/data, 0 = system)
//   bits 6:5  = DPL
//   bit  7    = P
//   bit  12   = AVL
//   bit  13   = L (64-bit code)
//   bit  14   = D/B
//   bit  15   = G

// 32-bit protected-mode code (CS): type=0xB (executable, readable,
// accessed), S=1, DPL=0, P=1, L=0 (not 64-bit), D/B=1 (32-bit
// operand size), G=1 (4 KB granularity). Limit interpreted as
// 4 KB pages so 0xFFFFFFFF means full 4 GB. The accessed bit is
// required even with unrestricted-guest=1.
const AR_CODE32: u32 = 0xB | 0x10 | 0x80 | (1 << 14) | (1 << 15);
// 32-bit data (SS, DS, ES, FS, GS): type=0x3 (RW, accessed),
// S=1, DPL=0, P=1, D/B=1, G=1.
const AR_DATA32: u32 = 0x3 | 0x10 | 0x80 | (1 << 14) | (1 << 15);
// Busy TSS: type=0xB, S=0, DPL=0, P=1. Same value for 32-bit and
// 64-bit busy TSS per SDM Vol. 3A §3.5 Table 3-2.
const AR_TSS_BUSY: u32 = 0xB | 0x80;
// Unusable segment marker (bit 16). Used for guest LDTR (no LDT).
const AR_UNUSABLE: u32 = 1 << 16;
// 16-bit real-mode code (CS): type=0xB, S=1, DPL=0, P=1, no G/D (16-bit).
// Matches SVM `ATTR_CODE_RM`. Used for an AP's SIPI entry (guest SMP).
const AR_CODE16: u32 = 0x9B;
// 16-bit real-mode data (SS/DS/ES/FS/GS): type=0x3, S=1, P=1.
const AR_DATA16: u32 = 0x93;

/// Fill GUEST_* fields for an unrestricted 32-bit protected-mode
/// guest with flat segments. CR0 has PE=1 PG=0 (paging off — Linux
/// will set up its own); unrestricted-guest=1 lets us clear PG
/// despite CR0_FIXED requirements (per SDM §26.3.1.1). All segments
/// are flat (base=0, limit=0xFFFFFFFF) so guest linear == guest
/// physical and EPT translates to host. EFER cleared (no long mode).
///
/// `guest_rip` is the linear (== EPT-mapped guest-physical) address
/// where the first instruction will be fetched. For the substrate
/// test that's 0x10000 (where the stub is copied). For Linux that's
/// `code32_start` from the bzImage setup-header (typically 0x100000).
///
/// `guest_rsp` and the boot-protocol register conventions (RSI =
/// boot_params phys for Linux 32-bit boot) are configured by the
/// caller via the GuestRegs struct passed to `run_guest_once`.
pub(super) fn setup_guest_state(guest_rip: u64) -> Result<(), &'static str> {
    // 32-bit prot mode CR0: must satisfy CR0_FIXED0/1 with PE
    // forced 1 and PG forced 0 (unrestricted-guest=1 relaxes
    // CR0_FIXED for both bits per SDM §26.3.1.1).
    let cr0_f0 = unsafe { rdmsr(0x486) };
    let cr0_f1 = unsafe { rdmsr(0x487) };
    let cr0_prot = (((1u64 << 0) | cr0_f0) & cr0_f1) & !(1u64 << 31);

    // CR4: take host CR4 with VMXE etc., but clear CET (bit 23).
    // Host nopeekOS has CR4.CET=1 for IBT defense — inheriting
    // that into the guest enables CET-IBT enforcement against
    // Alpine vmlinuz's hand-written asm stubs that lack ENDBR64,
    // raising #CP with err_code=3 (ENDBRANCH violation) early in
    // boot. CPUID Leaf 7 is also filtered (see enable.rs CPUID
    // handler) so Linux never tries to re-enable CET via cr4_init.
    let host_cr4: u64;
    // SAFETY: pure register read.
    unsafe { core::arch::asm!("mov {}, cr4", out(reg) host_cr4, options(nostack, preserves_flags)); }
    let guest_cr4 = host_cr4 & !(1u64 << 23);

    vmwrite(GUEST_CR0, cr0_prot)?;
    vmwrite(GUEST_CR3, 0)?;
    vmwrite(GUEST_CR4, guest_cr4)?;

    // Selectors. Standard kernel-style values; with unrestricted-
    // guest the AR-byte determines validity, not the selector itself.
    vmwrite(GUEST_CS_SELECTOR, 0x08)?;
    vmwrite(GUEST_SS_SELECTOR, 0x10)?;
    vmwrite(GUEST_DS_SELECTOR, 0x10)?;
    vmwrite(GUEST_ES_SELECTOR, 0x10)?;
    vmwrite(GUEST_FS_SELECTOR, 0x10)?;
    vmwrite(GUEST_GS_SELECTOR, 0x10)?;
    vmwrite(GUEST_LDTR_SELECTOR, 0)?;
    vmwrite(GUEST_TR_SELECTOR, 0)?;

    // Flat segments — base=0, limit=4 GB. Guest linear addr = guest
    // physical addr, EPT does the rest.
    vmwrite(GUEST_CS_BASE, 0)?;
    vmwrite(GUEST_SS_BASE, 0)?;
    vmwrite(GUEST_DS_BASE, 0)?;
    vmwrite(GUEST_ES_BASE, 0)?;
    vmwrite(GUEST_FS_BASE, 0)?;
    vmwrite(GUEST_GS_BASE, 0)?;
    vmwrite(GUEST_LDTR_BASE, 0)?;
    vmwrite(GUEST_TR_BASE, 0)?;
    vmwrite(GUEST_GDTR_BASE, 0)?;
    vmwrite(GUEST_IDTR_BASE, 0)?;

    // Limits — 4 GB flat for code/data; TR/GDTR/IDTR small/sane.
    vmwrite(GUEST_CS_LIMIT, 0xFFFF_FFFF)?;
    vmwrite(GUEST_SS_LIMIT, 0xFFFF_FFFF)?;
    vmwrite(GUEST_DS_LIMIT, 0xFFFF_FFFF)?;
    vmwrite(GUEST_ES_LIMIT, 0xFFFF_FFFF)?;
    vmwrite(GUEST_FS_LIMIT, 0xFFFF_FFFF)?;
    vmwrite(GUEST_GS_LIMIT, 0xFFFF_FFFF)?;
    vmwrite(GUEST_LDTR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_TR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_GDTR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_IDTR_LIMIT, 0xFFFF)?;

    // AR bytes. 32-bit code/data, busy TSS (validity-only;
    // guest doesn't actually use TR), unusable LDTR.
    vmwrite(GUEST_CS_AR_BYTES, AR_CODE32 as u64)?;
    vmwrite(GUEST_SS_AR_BYTES, AR_DATA32 as u64)?;
    vmwrite(GUEST_DS_AR_BYTES, AR_DATA32 as u64)?;
    vmwrite(GUEST_ES_AR_BYTES, AR_DATA32 as u64)?;
    vmwrite(GUEST_FS_AR_BYTES, AR_DATA32 as u64)?;
    vmwrite(GUEST_GS_AR_BYTES, AR_DATA32 as u64)?;
    vmwrite(GUEST_LDTR_AR_BYTES, AR_UNUSABLE as u64)?;
    vmwrite(GUEST_TR_AR_BYTES, AR_TSS_BUSY as u64)?;

    // Misc guest state. EFER cleared (no LME, no LMA — 32-bit
    // prot mode).
    vmwrite(GUEST_DR7, 0x400)?;
    vmwrite(GUEST_RFLAGS, 0x2)?; // IF=0, reserved bit 1 set
    vmwrite(GUEST_RSP, 0x80000)?; // arbitrary; real stack comes from caller GPRs if used
    vmwrite(GUEST_RIP, guest_rip)?;
    vmwrite(GUEST_INTERRUPTIBILITY_INFO, 0)?;
    vmwrite(GUEST_ACTIVITY_STATE, 0)?;
    vmwrite(GUEST_PENDING_DBG_EXCEPTIONS, 0)?;
    vmwrite(GUEST_SYSENTER_CS, 0)?;
    vmwrite(GUEST_SYSENTER_ESP, 0)?;
    vmwrite(GUEST_SYSENTER_EIP, 0)?;
    vmwrite(GUEST_IA32_EFER, 0)?;

    vmwrite(VMCS_LINK_POINTER, !0u64)?;

    Ok(())
}

/// Fill GUEST_* fields for an AP vCPU's SIPI start (guest SMP): 16-bit
/// real mode at `CS = (vector<<8):0000`, i.e. physical entry `vector<<12`
/// — where Linux's BSP copied the real-mode AP trampoline. The trampoline
/// takes the AP into protected→long mode and `start_secondary`; the
/// IA-32e/EFER transition is then picked up by `sync_entry_ia32e_with_efer`
/// exactly as on the BSP's 32-bit→64-bit boot. Real mode is allowed because
/// unrestricted-guest=1 (set in `setup_execution_controls`, shared with the
/// BSP). Mirrors svm `setup_vmcb_ap`.
pub(super) fn setup_guest_state_ap(sipi_vector: u8) -> Result<(), &'static str> {
    // Real-mode CR0: PE=0, PG=0. Unrestricted-guest relaxes the
    // CR0_FIXED0 must-be-1 on PE/PG (SDM §26.3.1.1); keep the other
    // fixed bits, force ET (bit 4).
    let cr0_f0 = unsafe { rdmsr(0x486) };
    let cr0_f1 = unsafe { rdmsr(0x487) };
    let cr0_rm = ((cr0_f0 & cr0_f1) & !((1u64 << 0) | (1u64 << 31))) | (1u64 << 4);

    let host_cr4: u64;
    // SAFETY: pure register read.
    unsafe { core::arch::asm!("mov {}, cr4", out(reg) host_cr4, options(nostack, preserves_flags)); }
    let guest_cr4 = host_cr4 & !(1u64 << 23); // clear CET (see setup_guest_state)

    vmwrite(GUEST_CR0, cr0_rm)?;
    vmwrite(GUEST_CR3, 0)?;
    vmwrite(GUEST_CR4, guest_cr4)?;

    let cs_sel = (sipi_vector as u64) << 8;
    let cs_base = (sipi_vector as u64) << 12;
    vmwrite(GUEST_CS_SELECTOR, cs_sel)?;
    vmwrite(GUEST_SS_SELECTOR, 0)?;
    vmwrite(GUEST_DS_SELECTOR, 0)?;
    vmwrite(GUEST_ES_SELECTOR, 0)?;
    vmwrite(GUEST_FS_SELECTOR, 0)?;
    vmwrite(GUEST_GS_SELECTOR, 0)?;
    vmwrite(GUEST_LDTR_SELECTOR, 0)?;
    vmwrite(GUEST_TR_SELECTOR, 0)?;

    vmwrite(GUEST_CS_BASE, cs_base)?;
    vmwrite(GUEST_SS_BASE, 0)?;
    vmwrite(GUEST_DS_BASE, 0)?;
    vmwrite(GUEST_ES_BASE, 0)?;
    vmwrite(GUEST_FS_BASE, 0)?;
    vmwrite(GUEST_GS_BASE, 0)?;
    vmwrite(GUEST_LDTR_BASE, 0)?;
    vmwrite(GUEST_TR_BASE, 0)?;
    vmwrite(GUEST_GDTR_BASE, 0)?;
    vmwrite(GUEST_IDTR_BASE, 0)?;

    // 16-bit real-mode limits (0xFFFF) for the segments; TR/GDTR/IDTR sane.
    vmwrite(GUEST_CS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_SS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_DS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_ES_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_FS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_GS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_LDTR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_TR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_GDTR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_IDTR_LIMIT, 0xFFFF)?;

    vmwrite(GUEST_CS_AR_BYTES, AR_CODE16 as u64)?;
    vmwrite(GUEST_SS_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_DS_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_ES_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_FS_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_GS_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_LDTR_AR_BYTES, AR_UNUSABLE as u64)?;
    vmwrite(GUEST_TR_AR_BYTES, AR_TSS_BUSY as u64)?;

    vmwrite(GUEST_DR7, 0x400)?;
    vmwrite(GUEST_RFLAGS, 0x2)?; // IF=0, reserved bit 1 set
    vmwrite(GUEST_RSP, 0)?;
    vmwrite(GUEST_RIP, 0)?; // real-mode offset 0 within CS=vector<<12
    vmwrite(GUEST_INTERRUPTIBILITY_INFO, 0)?;
    vmwrite(GUEST_ACTIVITY_STATE, 0)?;
    vmwrite(GUEST_PENDING_DBG_EXCEPTIONS, 0)?;
    vmwrite(GUEST_SYSENTER_CS, 0)?;
    vmwrite(GUEST_SYSENTER_ESP, 0)?;
    vmwrite(GUEST_SYSENTER_EIP, 0)?;
    vmwrite(GUEST_IA32_EFER, 0)?;

    vmwrite(VMCS_LINK_POINTER, !0u64)?;

    Ok(())
}

// ── VMRESUME loop with full GPR save/restore ───────────────────────

/// Guest general-purpose registers preserved across VMLAUNCH /
/// VMRESUME boundaries. Layout is `#[repr(C)]` with a fixed offset
/// per field so the asm! block can address them via `[rdi + N]`.
/// rsp lives in VMCS::GUEST_RSP; the corresponding slot here is
/// unused (kept so offsets stay sequential / canonical).
///
/// All-zero default = "fresh guest state on first launch". Updated
/// by every VM-exit so subsequent VMRESUME enters with the values
/// the guest had at exit (modulo Rust-side handler edits).
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct GuestRegs {
    pub rax: u64,    // offset   0
    pub rcx: u64,    //          8
    pub rdx: u64,    //         16
    pub rbx: u64,    //         24
    pub _rsp: u64,   //         32 — unused (VMCS::GUEST_RSP holds it)
    pub rbp: u64,    //         40
    pub rsi: u64,    //         48
    pub rdi: u64,    //         56
    pub r8:  u64,    //         64
    pub r9:  u64,    //         72
    pub r10: u64,    //         80
    pub r11: u64,    //         88
    pub r12: u64,    //         96
    pub r13: u64,    //        104
    pub r14: u64,    //        112
    pub r15: u64,    //        120
}

/// Outcome of one VM-entry/exit cycle.
pub struct LaunchOutcome {
    pub exit_reason: u64,
    pub exit_qualification: u64,
    /// Guest RAX captured at VM-exit. Convenience mirror of
    /// `regs.rax` after `run_guest_once`; redundant but kept for
    /// callers that don't have a regs reference handy.
    pub guest_rax: u64,
}

/// Run one VM-entry/exit cycle. Loads guest GPRs from `regs`,
/// VMLAUNCHes (if `launched=false`) or VMRESUMEs (if true), saves
/// guest GPRs back to `regs` on the resulting VM-exit, returns the
/// exit info.
///
/// Caller must already have written host-state, guest-state and
/// execution-controls into the current VMCS, and must be in VMX
/// root mode.
///
/// The asm! block has two control-flow paths that converge at
/// label 3:
///   1. VMLAUNCH/VMRESUME succeeds: control transfers to the guest.
///      Guest executes its first trapping instruction. The CPU
///      loads host state from VMCS — including HOST_RIP set to
///      label 2 and HOST_RSP set to our current stack pointer —
///      and we land at label 2 with the guest's GPRs still in
///      CPU registers. We push all 15 (rax..r15, skipping rsp) to
///      stack, then pop one-by-one storing into `regs` via the
///      saved struct pointer. Then VMREAD exit info.
///   2. VMLAUNCH/VMRESUME fails (VMfail{Invalid,Valid}): no
///      transition, execution falls through. Set vmfail=1 and
///      converge.
///
/// SAFETY: caller guarantees VMX root mode + current VMCS +
/// validated host/guest/control state. The asm pushes a variable
/// amount onto the stack across the boundary; HOST_RSP is set to
/// the post-prologue rsp so the post-exit landing finds the right
/// stack shape.
pub(super) fn run_guest_once(
    regs: &mut GuestRegs,
    launched: bool,
    host_fpu: *mut crate::microvm::cpu::FpuArea,
    guest_fpu: *mut crate::microvm::cpu::FpuArea,
) -> Result<LaunchOutcome, &'static str> {
    let exit_reason: u64;
    let exit_qualification: u64;
    let vmfail: u64;
    let regs_ptr: *mut GuestRegs = regs;

    // SAFETY: see fn-level docs. The asm respects every register
    // dependency by ordering: launched-flag check sets ZF before
    // r10 is overwritten with the guest's r10; rdi (struct ptr) is
    // overwritten LAST; post-exit save spills all guest GPRs to
    // stack first (so rdi stays guest's value), then reloads struct
    // ptr from the saved slot before storing.
    //
    // FPU xsave/xrstor lives INSIDE this asm block (mirror of SVM
    // v0.172.53 fix). Rust-helper FPU swap around the call left a
    // window where the +avx2 compiler could spill `vmovups ymm`
    // between the helper and the asm, clobbering the just-restored
    // guest FPU. Putting xsave64/xrstor64 inside the asm — with
    // only GPR/vm* instructions between xrstor and vmresume —
    // eliminates the window. v53 was applied to SVM only at the
    // time ("VMX keeps the helper form (untested path)"); now that
    // VMX is the path under test on bare-metal NUC, port it.
    unsafe {
        core::arch::asm!(
            // ── PROLOGUE: save host callee-saved + 3 pointers ─────
            "push rbp",
            "push rbx",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "push rdi",                     // struct ptr
            "push r8",                      // host_fpu  ptr
            "push r9",                      // guest_fpu ptr
            // Stack now: [rsp+0]=guest_fpu [+8]=host_fpu
            //   [+16]=struct_ptr [+24..71]=host_callee_saved

            // VMCS host pointer fields, set every entry so we never
            // depend on stale VMCS state across calls.
            "mov rcx, 0x6C14",              // HOST_RSP
            "vmwrite rcx, rsp",
            "mov rcx, 0x6C16",              // HOST_RIP
            "lea rax, [rip + 2f]",
            "vmwrite rcx, rax",

            // ── FPU SAVE/RESTORE (KVM kvm_load_guest_fpu): VMRESUME
            // preserves no x87/SSE/AVX/AVX-512. Must be in-asm,
            // adjacent to vmresume — a +avx2-built kernel spills
            // `ymm` anywhere between a Rust helper and the asm,
            // clobbering the just-restored guest state. Mask = -1
            // (all XCR0-enabled components; guest owns XCR0 via
            // XSETBV). Only GPR/vm* instructions sit between this
            // xrstor and vmresume, and between vmresume and the
            // paired xsave below.
            "mov rcx, [rsp + 8]",           // host_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xsave64 [rcx]",                // save host FPU
            "mov rcx, [rsp + 0]",           // guest_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xrstor64 [rcx]",               // restore guest FPU

            // ── ENTRY: load guest GPRs from struct ───────────────
            // r10 is caller-saved per System V ABI, so we use it as
            // a scratch for the launched flag without preservation.
            "mov r10, rsi",                 // r10 = launched
            "test r10, r10",                // ZF = !launched

            "mov rax, [rdi +   0]",
            "mov rcx, [rdi +   8]",
            "mov rdx, [rdi +  16]",
            "mov rbx, [rdi +  24]",
            // rsp at offset 32 is unused (VMCS::GUEST_RSP holds it).
            "mov rbp, [rdi +  40]",
            "mov rsi, [rdi +  48]",
            "mov r8,  [rdi +  64]",
            "mov r9,  [rdi +  72]",
            "mov r10, [rdi +  80]",         // overwrites flag — but
                                            // ZF was set by `test`
                                            // above and `mov`
                                            // doesn't touch flags
            "mov r11, [rdi +  88]",
            "mov r12, [rdi +  96]",
            "mov r13, [rdi + 104]",
            "mov r14, [rdi + 112]",
            "mov r15, [rdi + 120]",
            "mov rdi, [rdi +  56]",         // rdi LAST (overwrites
                                            // struct ptr)
            "jz 9f",                        // ZF set → !launched →
                                            // VMLAUNCH path
            "vmresume",
            "jmp 4f",                       // fall-through =
                                            // VMfail{Invalid,Valid}

            "9:",                           // VMLAUNCH path
            "vmlaunch",
            // fall-through to fail handler

            // ── FAIL HANDLER ─────────────────────────────────────
            // VMRESUME/VMLAUNCH failed — no transition happened.
            // GPRs still hold the guest values we loaded. FPU still
            // has the guest state we xrestored. Restore host FPU
            // before returning to Rust so the host doesn't run on
            // guest FPU.
            "4:",
            "mov rcx, [rsp + 0]",           // guest_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xsave64 [rcx]",                // save guest FPU
            "mov rcx, [rsp + 8]",           // host_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xrstor64 [rcx]",               // restore host FPU
            "mov rcx, 1",                   // vmfail = 1
            "xor rax, rax",                 // exit_reason = 0
            "xor rdx, rdx",                 // exit_qual = 0
            "add rsp, 24",                  // discard guest_fpu, host_fpu, struct_ptr
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbx",
            "pop rbp",
            "sti",
            "jmp 5f",

            // ── POST-VM-EXIT: save guest GPRs ────────────────────
            "2:",
            // Push all 15 guest GPRs onto stack so we can reuse rdi
            // (currently guest's rdi) without losing it. Order is
            // canonical: lowest-offset (rax) pushed first, so it
            // ends up deepest on stack.
            "push rax",
            "push rcx",
            "push rdx",
            "push rbx",
            "push rbp",
            "push rsi",
            "push rdi",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            // Stack now: 15 GPRs [rsp+0..119], guest_fpu [+120],
            //   host_fpu [+128], struct_ptr [+136], callee_saved [+144..]

            // ── FPU SAVE/RESTORE (paired exit half) ──────────────
            // Guest GPRs are on the stack now → rax/rcx/rdx free to
            // clobber. Save guest FPU, restore host's.
            "mov rcx, [rsp + 120]",         // guest_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xsave64 [rcx]",                // save guest FPU
            "mov rcx, [rsp + 128]",         // host_fpu
            "mov eax, 0xffffffff",
            "mov edx, 0xffffffff",
            "xrstor64 [rcx]",               // restore host FPU

            "mov rdi, [rsp + 136]",         // struct ptr
            // Pop in reverse-push order, store at the right offset.
            "pop rax", "mov [rdi + 120], rax",      // r15
            "pop rax", "mov [rdi + 112], rax",      // r14
            "pop rax", "mov [rdi + 104], rax",      // r13
            "pop rax", "mov [rdi +  96], rax",      // r12
            "pop rax", "mov [rdi +  88], rax",      // r11
            "pop rax", "mov [rdi +  80], rax",      // r10
            "pop rax", "mov [rdi +  72], rax",      // r9
            "pop rax", "mov [rdi +  64], rax",      // r8
            "pop rax", "mov [rdi +  56], rax",      // rdi (guest's)
            "pop rax", "mov [rdi +  48], rax",      // rsi
            "pop rax", "mov [rdi +  40], rax",      // rbp
            "pop rax", "mov [rdi +  24], rax",      // rbx
            "pop rax", "mov [rdi +  16], rax",      // rdx
            "pop rax", "mov [rdi +   8], rax",      // rcx
            "pop rax", "mov [rdi +   0], rax",      // rax

            // Read VM-exit info now that GPRs are safe.
            "mov rcx, 0x4402",              // VM_EXIT_REASON
            "vmread rax, rcx",
            "mov rcx, 0x6400",              // VM_EXIT_QUALIFICATION
            "vmread rdx, rcx",
            "xor rcx, rcx",                 // vmfail = 0

            "add rsp, 24",                  // discard guest_fpu, host_fpu, struct_ptr
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbx",
            "pop rbp",
            "sti",                          // VM-exit cleared IF; re-enable

            "5:",
            in("rdi") regs_ptr,
            in("rsi") launched as u64,
            in("r8") host_fpu,
            in("r9") guest_fpu,
            lateout("rax") exit_reason,
            lateout("rcx") vmfail,
            lateout("rdx") exit_qualification,
            clobber_abi("C"),
        );
    }

    if vmfail != 0 {
        let err = vmread(VM_INSTRUCTION_ERROR).unwrap_or(0);
        use crate::kprintln;
        kprintln!("[vmx] VM-entry failed: VM_INSTRUCTION_ERROR = {}", err);
        return Err("VM-entry failed (see kernel log for VM_INSTRUCTION_ERROR)");
    }

    Ok(LaunchOutcome {
        exit_reason,
        exit_qualification,
        guest_rax: regs.rax,
    })
}

/// Read VM_EXIT_REASON's basic-reason field (bits 15:0). Convenience
/// for callers that already have the raw 32-bit value.
pub fn basic_exit_reason(raw: u64) -> u16 {
    (raw & 0xFFFF) as u16
}

/// Advance GUEST_RIP past the just-exited instruction. The CPU
/// records the instruction length in VM_EXIT_INSTRUCTION_LEN; we
/// add it to the current GUEST_RIP. Required after I/O exits
/// (otherwise VMRESUME re-executes the trapping OUT/IN forever).
pub(super) fn advance_guest_rip() -> Result<(), &'static str> {
    let len = vmread(VM_EXIT_INSTRUCTION_LEN)?;
    let rip = vmread(GUEST_RIP)?;
    vmwrite(GUEST_RIP, rip.wrapping_add(len))
}

/// Read GUEST_CR3 from the current VMCS. Used by the CR-access
/// exit handler when the guest does `MOV reg, CR3`.
pub fn read_guest_cr3() -> Result<u64, &'static str> {
    vmread(GUEST_CR3)
}

/// Write GUEST_CR3 to the current VMCS. Used by the CR-access
/// exit handler when the guest does `MOV CR3, reg`.
pub fn write_guest_cr3(value: u64) -> Result<(), &'static str> {
    vmwrite(GUEST_CR3, value)
}

/// Read GUEST_RSP from the current VMCS. RSP is not in `GuestRegs`
/// (the CPU loads/saves it from VMCS GUEST_RSP across entry/exit).
pub fn read_guest_rsp() -> Result<u64, &'static str> {
    vmread(GUEST_RSP)
}

/// Write GUEST_RSP to the current VMCS.
pub fn write_guest_rsp(value: u64) -> Result<(), &'static str> {
    vmwrite(GUEST_RSP, value)
}

/// Inject an external interrupt on the next VM-entry. `vector` is the
/// IDT vector to deliver. The CPU honours the guest's RFLAGS.IF / NMI
/// blocking state — if interrupts are disabled the entry will fail
/// (caller must check), but in practice we inject from MMIO-trap exits
/// where the guest had IF=1 already.
pub fn inject_external_irq(vector: u8) -> Result<(), &'static str> {
    // Bit 31 = valid, bits 10:8 = type (0 = external interrupt),
    // bits 7:0 = vector.
    let info: u64 = (vector as u64) | (1u64 << 31);
    vmwrite(VM_ENTRY_INTR_INFO_FIELD, info)
}

/// Read VMCS GUEST_RIP — the linear address of the guest
/// instruction that caused the most recent VM-exit.
pub fn read_guest_rip() -> Result<u64, &'static str> {
    vmread(GUEST_RIP)
}

/// Read VMCS GUEST_CR0.
pub fn read_guest_cr0() -> Result<u64, &'static str> {
    vmread(GUEST_CR0)
}

/// Read VMCS GUEST_CR4.
pub fn read_guest_cr4() -> Result<u64, &'static str> {
    vmread(GUEST_CR4)
}

/// Read VMCS GUEST_IA32_EFER.
pub fn read_guest_efer() -> Result<u64, &'static str> {
    vmread(GUEST_IA32_EFER)
}

/// Read VMCS GUEST_CS_SELECTOR.
pub fn read_guest_cs_selector() -> Result<u64, &'static str> {
    vmread(GUEST_CS_SELECTOR)
}

/// Read VMCS GUEST_CS_AR_BYTES — bit 13 = L (64-bit code), bit 14 =
/// D/B. Diagnostic: lets the run loop log when the guest enters long
/// mode and whether re-entry into that mode succeeds.
pub fn read_guest_cs_ar() -> Result<u64, &'static str> {
    vmread(GUEST_CS_AR_BYTES)
}

/// Read VM_ENTRY_INTR_INFO_FIELD — the event (if any) we asked the CPU
/// to inject on the entry that just ran. Diagnostic for the bare-metal
/// reason-33 path: confirms whether an injection was pending at a
/// failing VM-entry (a failed entry never delivers, so the field still
/// holds what we wrote).
pub fn read_entry_intr_info() -> Result<u64, &'static str> {
    vmread(VM_ENTRY_INTR_INFO_FIELD)
}

/// True if the guest can accept a maskable external interrupt right
/// now: RFLAGS.IF=1 and no interruptibility blocking (bit 0 blocking-
/// by-STI, bit 1 blocking-by-MOV-SS). This is the gate KVM applies in
/// `vmx_interrupt_allowed()` before injecting an external interrupt.
/// Injecting one while the guest is NOT interruptible (e.g. IF=0 in a
/// CLI'd critical section — observed mid i8042 status poll) makes the
/// VM-entry fail with reason 33 on bare-metal Intel; AMD's VMRUN
/// tolerates it (delivers a guest #GP), which is why QEMU/AMD never
/// tripped on it. NMI-blocking (bit 3) is irrelevant to maskable
/// external interrupts and is not checked.
pub fn guest_interruptible() -> bool {
    const IF: u64 = 1 << 9;
    let rflags = vmread(GUEST_RFLAGS).unwrap_or(0);
    let intr = vmread(GUEST_INTERRUPTIBILITY_INFO).unwrap_or(0);
    (rflags & IF) != 0 && (intr & 0b11) == 0
}

/// Clear blocking-by-STI (bit 0) + blocking-by-MOV-SS (bit 1) in the guest
/// interruptibility-state. Call after advancing past a HLT: the HLT IS the
/// single instruction the STI shadow covered, so executing it consumes the
/// shadow. Without this, an idle `sti; hlt` returns with blocking-by-STI
/// still set → `guest_interruptible()` is false → the HLT handler's gate
/// re-enters instead of injecting the wakeup + parking → a 3.3M-hlt/s spin
/// (SVM never hit it because AMD tolerates inject-while-shadowed; VMX rejects
/// it with reason 33, so we must clear the shadow ourselves).
pub fn consume_sti_shadow() -> Result<(), &'static str> {
    let v = vmread(GUEST_INTERRUPTIBILITY_INFO)?;
    if v & 0b11 != 0 {
        vmwrite(GUEST_INTERRUPTIBILITY_INFO, v & !0b11)?;
    }
    Ok(())
}

/// Read VMCS VM_ENTRY_CONTROLS — the live entry-control field
/// after our last VMWRITE (or fixed_ctrl-applied initial value).
pub fn read_vm_entry_controls() -> Result<u64, &'static str> {
    vmread(VM_ENTRY_CONTROLS)
}

/// Read VMCS GUEST_PHYSICAL_ADDRESS — set by the CPU on EPT
/// violations and EPT misconfigurations. Tells us what guest-phys
/// address the guest tried to access when EPT translation failed.
pub fn read_guest_phys_addr() -> Result<u64, &'static str> {
    vmread(VM_EXIT_GUEST_PHYS_ADDR)
}

/// Read VMCS GUEST_LINEAR_ADDRESS — set by the CPU when bit 7 of
/// the EPT-violation qualification is set. The linear address that
/// triggered the violation (vs the qual which is the guest-phys).
pub fn read_guest_linear_addr() -> Result<u64, &'static str> {
    vmread(VM_EXIT_GUEST_LINEAR_ADDR)
}

/// Read VM_EXIT_INTR_INFO. For exception VM-exits (basic reason 0),
/// the relevant fields are:
///   bits 7:0  = vector (0..31)
///   bits 10:8 = interruption type (3 = HW exception, 6 = SW exception)
///   bit  11   = error code valid
///   bit  31   = valid (always set on exit info)
pub fn read_exit_intr_info() -> Result<u64, &'static str> {
    vmread(VM_EXIT_INTR_INFO)
}

/// Read VM_EXIT_INTR_ERROR_CODE — the error code architecturally
/// pushed by certain exceptions (#PF, #GP, #SS, #DF, etc.). Only
/// meaningful when bit 11 of VM_EXIT_INTR_INFO is set.
pub fn read_exit_intr_error_code() -> Result<u64, &'static str> {
    vmread(VM_EXIT_INTR_ERROR_CODE)
}

/// Read IDT_VECTORING_INFORMATION_FIELD (SDM Vol 3 §27.2.4). The CPU
/// sets this when a VM-exit interrupted the guest in the middle of
/// vectoring an event through its IDT (i.e. the host IRQ landed
/// between the CPU's "decided to deliver vector V" and "actually
/// pushed it onto the guest stack"). Encoding mirrors VM_ENTRY_
/// INTR_INFO exactly so the value can be re-injected verbatim:
///   bits 7:0  = vector,  bits 10:8 = type (0=ext IRQ, 2=NMI,
///   3=hw exc, 4=sw int, 5=privsw, 6=sw exc), bit 11 = error-code
///   valid, bit 31 = valid.
/// Must be re-injected on the next entry or the guest loses an
/// interrupt mid-vectoring → corrupt state (the VMX equivalent of
/// the SVM EXITINTINFO/`svm_complete_interrupts` path; see svm
/// enable.rs v0.172.36+).
pub fn read_idt_vectoring_info() -> Result<u64, &'static str> {
    vmread(IDT_VECTORING_INFO_FIELD)
}

/// Read IDT_VECTORING_ERROR_CODE — companion to IDT_VECTORING_INFO_
/// FIELD when bit 11 is set. We currently only re-inject external
/// IRQs (type 0) and NMIs (type 2), neither of which carry an error
/// code, so this is unused but kept for symmetry with the exit-info
/// readers and future use if exception re-injection lands.
#[allow(dead_code)]
pub fn read_idt_vectoring_error_code() -> Result<u64, &'static str> {
    vmread(IDT_VECTORING_ERROR_CODE)
}

/// Raw write to VM_ENTRY_INTR_INFO_FIELD. Used to (a) inject a
/// freshly-decided event with a full info word (e.g. when the value
/// comes from `IDT_VECTORING_INFO_FIELD` and must be replayed
/// verbatim), and (b) defensively clear the slot to 0 after every
/// VMRESUME. On bare-metal VMX the CPU auto-clears the valid bit
/// after a successful injection (SDM §27.6), but under nested VMX
/// (KVM emulating VMX on AMD) it MAY leave the bit set — left there,
/// the next VMRESUME re-injects the same event = phantom duplicate
/// interrupt → cumulative guest corruption. This is the VMX
/// equivalent of the SVM EVENTINJ-stale bug fixed in v0.172.42.
pub fn write_entry_intr_info(info: u64) -> Result<(), &'static str> {
    vmwrite(VM_ENTRY_INTR_INFO_FIELD, info)
}

/// Diagnostic dump after a VM-entry-fail exit (reason 33/34/41).
/// Pulls the fields the CPU consistency-checks at entry so we can tell
/// which one tripped. SDM Vol 3 §26.3.1 enumerates the rules. Common
/// failures: IA-32e/CR/EFER mismatch (long-mode triad not coherent),
/// CS access rights vs CS selector RPL, segment limits, etc.
pub fn dump_entry_fail_state() {
    use crate::kprintln;
    // Encodings of fields not declared at file top (kept local so the
    // module's public surface doesn't grow for diagnostic-only use).
    const GUEST_IA32_DEBUGCTL:  u64 = 0x2802;
    const GUEST_IA32_PAT:       u64 = 0x2804;
    const GUEST_IA32_PERF_GCTL: u64 = 0x2808;
    const GUEST_SYSENTER_CS_F:  u64 = 0x482A;
    const GUEST_SYSENTER_ESP_F: u64 = 0x6824;
    const GUEST_SYSENTER_EIP_F: u64 = 0x6826;
    const VM_EXIT_REASON_F:     u64 = 0x4402;
    let cr0     = vmread(GUEST_CR0).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let cr3     = vmread(GUEST_CR3).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let cr4     = vmread(GUEST_CR4).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let efer    = vmread(GUEST_IA32_EFER).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let rip     = vmread(GUEST_RIP).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let rflags  = vmread(GUEST_RFLAGS).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let rsp     = vmread(GUEST_RSP).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let entry   = vmread(VM_ENTRY_CONTROLS).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let exitc   = vmread(VM_EXIT_CONTROLS).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let dbgctl  = vmread(GUEST_IA32_DEBUGCTL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let gpat    = vmread(GUEST_IA32_PAT).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let gperf   = vmread(GUEST_IA32_PERF_GCTL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let sysencs = vmread(GUEST_SYSENTER_CS_F).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let sysenes = vmread(GUEST_SYSENTER_ESP_F).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let sysenei = vmread(GUEST_SYSENTER_EIP_F).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_cr0   = vmread(HOST_CR0).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_cr3   = vmread(HOST_CR3).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_cr4   = vmread(HOST_CR4).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_efer  = vmread(HOST_IA32_EFER).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_rsp   = vmread(HOST_RSP).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_rip   = vmread(HOST_RIP).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_cs    = vmread(HOST_CS_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let raw_rsn = vmread(VM_EXIT_REASON_F).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    // Additional host fields: any HOST_* canonicity/coherency failure
    // also fires reason 33 (host-state checked together with guest).
    const HOST_SS_SEL:   u64 = 0x0C04;
    const HOST_DS_SEL:   u64 = 0x0C06;
    const HOST_FS_SEL:   u64 = 0x0C08;
    const HOST_GS_SEL:   u64 = 0x0C0A;
    const HOST_TR_SEL:   u64 = 0x0C0C;
    const HOST_SYS_CS:   u64 = 0x4C00;
    const HOST_SYS_ESP:  u64 = 0x6C10;
    const HOST_SYS_EIP:  u64 = 0x6C12;
    let h_ss    = vmread(HOST_SS_SEL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_ds    = vmread(HOST_DS_SEL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_fs    = vmread(HOST_FS_SEL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_gs    = vmread(HOST_GS_SEL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_tr    = vmread(HOST_TR_SEL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_fsb   = vmread(HOST_FS_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_gsb   = vmread(HOST_GS_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_trb   = vmread(HOST_TR_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_gdtr  = vmread(HOST_GDTR_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_idtr  = vmread(HOST_IDTR_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_syscs = vmread(HOST_SYS_CS).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_syses = vmread(HOST_SYS_ESP).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let h_sysei = vmread(HOST_SYS_EIP).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    // VMX-fixed MSRs: tell us which CR0/CR4 bits the CPU rejects. If
    // guest CR0/CR4 has a bit cleared that FIXED0 says must-be-1, or
    // a bit set that FIXED1 says must-be-0, entry fails reason 33.
    const IA32_VMX_CR0_FIXED0_MSR: u32 = 0x486;
    const IA32_VMX_CR0_FIXED1_MSR: u32 = 0x487;
    const IA32_VMX_CR4_FIXED0_MSR: u32 = 0x488;
    const IA32_VMX_CR4_FIXED1_MSR: u32 = 0x489;
    let cr0_f0  = unsafe { rdmsr(IA32_VMX_CR0_FIXED0_MSR) };
    let cr0_f1  = unsafe { rdmsr(IA32_VMX_CR0_FIXED1_MSR) };
    let cr4_f0  = unsafe { rdmsr(IA32_VMX_CR4_FIXED0_MSR) };
    let cr4_f1  = unsafe { rdmsr(IA32_VMX_CR4_FIXED1_MSR) };
    let cr0_violation_0 = (cr0_f0 & !cr0) & 0xFFFF_FFFF;
    let cr0_violation_1 = (cr0 & !cr0_f1) & 0xFFFF_FFFF;
    let cr4_violation_0 = (cr4_f0 & !cr4) & 0xFFFF_FFFF;
    let cr4_violation_1 = (cr4 & !cr4_f1) & 0xFFFF_FFFF;
    let cs_sel  = vmread(GUEST_CS_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let cs_base = vmread(GUEST_CS_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let cs_lim  = vmread(GUEST_CS_LIMIT).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let cs_ar   = vmread(GUEST_CS_AR_BYTES).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let ss_sel  = vmread(GUEST_SS_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let ss_ar   = vmread(GUEST_SS_AR_BYTES).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let ds_sel  = vmread(GUEST_DS_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let ds_ar   = vmread(GUEST_DS_AR_BYTES).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let es_sel  = vmread(GUEST_ES_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let es_ar   = vmread(GUEST_ES_AR_BYTES).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let fs_sel  = vmread(GUEST_FS_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let fs_ar   = vmread(GUEST_FS_AR_BYTES).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let fs_base = vmread(GUEST_FS_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let gs_sel  = vmread(GUEST_GS_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let gs_ar   = vmread(GUEST_GS_AR_BYTES).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let gs_base = vmread(GUEST_GS_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let tr_sel  = vmread(GUEST_TR_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let tr_base = vmread(GUEST_TR_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let tr_lim  = vmread(GUEST_TR_LIMIT).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let tr_ar   = vmread(GUEST_TR_AR_BYTES).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let ldtr_s  = vmread(GUEST_LDTR_SELECTOR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let ldtr_ar = vmread(GUEST_LDTR_AR_BYTES).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let gdtr_b  = vmread(GUEST_GDTR_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let gdtr_l  = vmread(GUEST_GDTR_LIMIT).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let idtr_b  = vmread(GUEST_IDTR_BASE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let idtr_l  = vmread(GUEST_IDTR_LIMIT).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let intr    = vmread(GUEST_INTERRUPTIBILITY_INFO).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let dr7     = vmread(GUEST_DR7).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let pdb     = vmread(GUEST_PENDING_DBG_EXCEPTIONS).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let vlink   = vmread(VMCS_LINK_POINTER).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let act     = vmread(GUEST_ACTIVITY_STATE).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let vminst  = vmread(VM_INSTRUCTION_ERROR).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let lma     = (efer >> 10) & 1;
    let lme     = (efer >> 8) & 1;
    let pg      = (cr0 >> 31) & 1;
    let pe      = cr0 & 1;
    let pae     = (cr4 >> 5) & 1;
    let ia32e   = (entry >> 9) & 1;
    kprintln!("[vmx-fail] VM_INSTRUCTION_ERROR = {}", vminst);
    kprintln!("[vmx-fail] CR0={:#018x} (PE={} PG={})  CR4={:#018x} (PAE={})",
        cr0, pe, pg, cr4, pae);
    kprintln!("[vmx-fail] CR3={:#018x}", cr3);
    kprintln!("[vmx-fail] EFER={:#018x} (LME={} LMA={})  ENTRY_CTLS={:#x} (IA32E={})  EXIT_CTLS={:#x}",
        efer, lme, lma, entry, ia32e, exitc);
    kprintln!("[vmx-fail] consistency: want IA32E == LMA: {} (have IA32E={}, LMA={}) → {}",
        if ia32e == lma { "OK" } else { "MISMATCH ←" },
        ia32e, lma,
        if ia32e == lma { "rule passes" } else { "would fail SDM §26.3.1.1" });
    kprintln!("[vmx-fail] RIP={:#018x} RSP={:#018x} RFLAGS={:#x}", rip, rsp, rflags);
    // Segment-by-segment dump. AR bit 16 = unusable (segment skipped
    // by VMX consistency checks). For IA-32e mode the load-bearing
    // checks are: CS.L vs CS.D mutex; SS.RPL == CS.RPL even when SS
    // null; TR type = 11 (busy 64-bit TSS); LDTR S=0 if usable;
    // DS/ES/FS/GS S=1 + P=1 if usable. SDM Vol 3 §26.3.1.4.
    kprintln!("[vmx-fail] CS   sel={:#06x} base={:#018x} lim={:#x} ar={:#x} (L={} D={} unusable={})",
        cs_sel, cs_base, cs_lim, cs_ar,
        (cs_ar >> 13) & 1, (cs_ar >> 14) & 1, (cs_ar >> 16) & 1);
    kprintln!("[vmx-fail] SS   sel={:#06x} ar={:#x} (DPL={} unusable={})  CS.RPL={}",
        ss_sel, ss_ar, (ss_ar >> 5) & 3, (ss_ar >> 16) & 1, cs_sel & 3);
    kprintln!("[vmx-fail] DS   sel={:#06x} ar={:#x} (unusable={})", ds_sel, ds_ar, (ds_ar >> 16) & 1);
    kprintln!("[vmx-fail] ES   sel={:#06x} ar={:#x} (unusable={})", es_sel, es_ar, (es_ar >> 16) & 1);
    kprintln!("[vmx-fail] FS   sel={:#06x} base={:#018x} ar={:#x} (unusable={})",
        fs_sel, fs_base, fs_ar, (fs_ar >> 16) & 1);
    kprintln!("[vmx-fail] GS   sel={:#06x} base={:#018x} ar={:#x} (unusable={})",
        gs_sel, gs_base, gs_ar, (gs_ar >> 16) & 1);
    kprintln!("[vmx-fail] TR   sel={:#06x} base={:#018x} lim={:#x} ar={:#x} (type={} unusable={})",
        tr_sel, tr_base, tr_lim, tr_ar, tr_ar & 0xF, (tr_ar >> 16) & 1);
    kprintln!("[vmx-fail] LDTR sel={:#06x} ar={:#x} (S={} unusable={})",
        ldtr_s, ldtr_ar, (ldtr_ar >> 4) & 1, (ldtr_ar >> 16) & 1);
    kprintln!("[vmx-fail] GDTR base={:#018x} limit={:#x}", gdtr_b, gdtr_l);
    kprintln!("[vmx-fail] IDTR base={:#018x} limit={:#x}", idtr_b, idtr_l);
    kprintln!("[vmx-fail] INTERRUPTIBILITY={:#x} (STI={} MOV_SS={} SMI={} NMI={})",
        intr, intr & 1, (intr >> 1) & 1, (intr >> 2) & 1, (intr >> 3) & 1);
    kprintln!("[vmx-fail] DR7={:#x}  PENDING_DBG={:#x}  VMCS_LINK={:#018x}",
        dr7, pdb, vlink);
    kprintln!("[vmx-fail] activity_state={}", act);
    // MSR-like guest fields. Reserved-bit issues here trip §26.3.1.1
    // even when the visible CR/EFER triad is consistent.
    kprintln!("[vmx-fail] IA32_DEBUGCTL={:#018x}  IA32_PAT={:#018x}",
        dbgctl, gpat);
    kprintln!("[vmx-fail] IA32_PERF_GLOBAL_CTRL={:#018x}", gperf);
    kprintln!("[vmx-fail] SYSENTER cs={:#x} esp={:#018x} eip={:#018x}",
        sysencs, sysenes, sysenei);
    kprintln!("[vmx-fail] FS.base={:#018x}  GS.base={:#018x}",
        fs_base, gs_base);
    // Host state — a HOST_* canonicity/coherency issue can also fail
    // entry (basic reason stays 33 because guest is loaded first).
    kprintln!("[vmx-fail] HOST CR0={:#018x} CR3={:#018x} CR4={:#018x}",
        h_cr0, h_cr3, h_cr4);
    kprintln!("[vmx-fail] HOST EFER={:#018x} CS={:#x} RIP={:#018x} RSP={:#018x}",
        h_efer, h_cs, h_rip, h_rsp);
    // Raw VM_EXIT_REASON. Bit 31 = VM-entry-failure indication; bit 12
    // = pending MTF. Confirms this really is an entry-fail (vs an exit
    // we misclassified).
    kprintln!("[vmx-fail] VM_EXIT_REASON raw={:#x} (bit31={} = entry-fail flag)",
        raw_rsn, (raw_rsn >> 31) & 1);
    kprintln!("[vmx-fail] HOST sel: CS={:#x} SS={:#x} DS={:#x} FS={:#x} GS={:#x} TR={:#x}",
        h_cs, h_ss, h_ds, h_fs, h_gs, h_tr);
    kprintln!("[vmx-fail] HOST FS.base={:#018x} GS.base={:#018x} TR.base={:#018x}",
        h_fsb, h_gsb, h_trb);
    kprintln!("[vmx-fail] HOST GDTR.base={:#018x} IDTR.base={:#018x}",
        h_gdtr, h_idtr);
    kprintln!("[vmx-fail] HOST SYSENTER cs={:#x} esp={:#018x} eip={:#018x}",
        h_syscs, h_syses, h_sysei);
    kprintln!("[vmx-fail] CR0_FIXED0={:#018x} CR0_FIXED1={:#018x}",
        cr0_f0, cr0_f1);
    kprintln!("[vmx-fail] CR4_FIXED0={:#018x} CR4_FIXED1={:#018x}",
        cr4_f0, cr4_f1);
    // Live execution-control fields + their fixed-bit MSRs. Reveals
    // forced-on bits we don't expect (e.g. VPID, NMI-exiting, virtual-
    // NMIs). For VMX-entry-fail with all guest-state checks passing,
    // a control-field rule like "VPID enabled + VPID field = 0" is
    // the remaining culprit-class.
    let pin_live   = vmread(PIN_BASED_VM_EXEC_CONTROL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let cpu_live   = vmread(CPU_BASED_VM_EXEC_CONTROL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let sec_live   = vmread(SECONDARY_VM_EXEC_CONTROL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    const VMX_PIN_TRUE:  u32 = 0x48D;
    const VMX_PROC_TRUE: u32 = 0x48E;
    const VMX_BASIC_MSR: u32 = 0x480;
    const VMX_PROC2_MSR: u32 = 0x48B;
    let basic_msr  = unsafe { rdmsr(VMX_BASIC_MSR) };
    let pin_msr    = unsafe { rdmsr(VMX_PIN_TRUE) };
    let proc_msr   = unsafe { rdmsr(VMX_PROC_TRUE) };
    let proc2_msr  = unsafe { rdmsr(VMX_PROC2_MSR) };
    kprintln!("[vmx-fail] PIN={:#x} CPU={:#x} SEC={:#x}", pin_live, cpu_live, sec_live);
    kprintln!("[vmx-fail] BASIC={:#018x} (true-ctls={})", basic_msr, (basic_msr >> 55) & 1);
    kprintln!("[vmx-fail] PIN-MSR (TRUE)={:#018x} → allowed-0={:#x} allowed-1={:#x}",
        pin_msr, pin_msr & 0xFFFF_FFFF, pin_msr >> 32);
    kprintln!("[vmx-fail] PROC-MSR (TRUE)={:#018x} → allowed-0={:#x} allowed-1={:#x}",
        proc_msr, proc_msr & 0xFFFF_FFFF, proc_msr >> 32);
    kprintln!("[vmx-fail] PROC2-MSR={:#018x} → allowed-0={:#x} allowed-1={:#x}",
        proc2_msr, proc2_msr & 0xFFFF_FFFF, proc2_msr >> 32);
    // EPT/IO/MSR pointer fields. If any of these are corrupt/zero
    // while their enable bit is on, entry fails reason 33.
    let eptp_f      = vmread(EPT_POINTER).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let iob_a       = vmread(IO_BITMAP_A_FULL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let iob_b       = vmread(IO_BITMAP_B_FULL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    let msrb        = vmread(MSR_BITMAPS_FULL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
    kprintln!("[vmx-fail] EPTP={:#018x}", eptp_f);
    kprintln!("[vmx-fail] IO_BITMAP_A={:#018x} IO_BITMAP_B={:#018x} MSR_BITMAP={:#018x}",
        iob_a, iob_b, msrb);
    if cr0_violation_0 != 0 || cr0_violation_1 != 0 {
        kprintln!("[vmx-fail] ← CR0 VIOLATION: missing-must-be-1={:#x}, set-must-be-0={:#x}",
            cr0_violation_0, cr0_violation_1);
    }
    if cr4_violation_0 != 0 || cr4_violation_1 != 0 {
        kprintln!("[vmx-fail] ← CR4 VIOLATION: missing-must-be-1={:#x}, set-must-be-0={:#x}",
            cr4_violation_0, cr4_violation_1);
    }
}

/// Sync VM_ENTRY_CONTROLS' IA-32e-mode-guest bit to GUEST_IA32_EFER.LMA.
/// VMX requires the two to match at entry: if the guest is currently
/// in long mode (LMA=1, set automatically by the CPU when CR0.PG=1
/// and EFER.LME=1), entry control bit 9 must be 1. If guest is in
/// 32-bit mode, bit 9 must be 0. Callers invoke this between
/// VM-exits and the next VMRESUME so the control tracks the live
/// guest mode across long-mode transitions.
pub fn sync_entry_ia32e_with_efer() -> Result<(), &'static str> {
    const LMA_BIT: u64 = 1 << 10;
    let efer = vmread(GUEST_IA32_EFER)?;
    let entry = vmread(VM_ENTRY_CONTROLS)?;
    let want_ia32e = (efer & LMA_BIT) != 0;
    let has_ia32e = (entry & ENTRY_IA32E_MODE_GUEST as u64) != 0;
    if want_ia32e != has_ia32e {
        let new = if want_ia32e {
            entry | ENTRY_IA32E_MODE_GUEST as u64
        } else {
            entry & !(ENTRY_IA32E_MODE_GUEST as u64)
        };
        vmwrite(VM_ENTRY_CONTROLS, new)?;
    }
    Ok(())
}
