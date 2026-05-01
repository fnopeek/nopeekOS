//! VMCS field setup — Phase 12.1.0d-1.
//!
//! Provides VMWRITE/VMREAD wrappers, the SDM Appendix-B field
//! encodings we need for host-state, and the host-state setup pass
//! that runs after VMPTRLD inside VMX root mode.
//!
//! 12.1.0d-1 scope:
//!   - All HOST_* fields written + read back to validate the
//!     VMWRITE / VMREAD pipe and the host-state math.
//!   - HOST_RIP points at `vmx_exit_trampoline` (defined below) so
//!     the field is canonical and lands in real kernel code, even
//!     though no VM-exit can fire yet (no VMLAUNCH).
//!   - No guest-state, no execution-controls, no VMLAUNCH — those
//!     land in 12.1.0d-2.
//!
//! Reference: Intel SDM Vol. 3C §26.2.2 (Checks on Host Control
//! Registers, MSRs, and Segment Registers), Appendix B (Field
//! Encoding in VMCS).

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

// ── VM-exit trampoline ─────────────────────────────────────────────

// Symbol address used as HOST_RIP. On a VM-exit, the CPU loads host
// state from the VMCS and resumes execution here. 12.1.0d-1 doesn't
// VMLAUNCH so this is never actually entered — but the symbol must
// resolve to real code so HOST_RIP is canonical.
//
// In 12.1.0d-2 a guest will execute `hlt`, fault here, the stub
// VMREADs VM_EXIT_REASON into RDI and tail-calls `vmx_handle_exit`
// for logging.
core::arch::global_asm!(
    "
    .section .text.vmx_exit_trampoline, \"ax\"
    .global vmx_exit_trampoline
    .type vmx_exit_trampoline, @function
vmx_exit_trampoline:
    cli
    mov rax, 0x4402            // VM_EXIT_REASON encoding
    vmread rdi, rax            // basic exit reason → RDI
    call vmx_handle_exit
1:  hlt
    jmp 1b
    .size vmx_exit_trampoline, . - vmx_exit_trampoline
    "
);

unsafe extern "C" {
    fn vmx_exit_trampoline();
}

fn exit_trampoline_addr() -> u64 {
    vmx_exit_trampoline as *const () as usize as u64
}

/// Called from the trampoline on a VM-exit. Logs the basic exit
/// reason and returns; the trampoline halts immediately after.
#[unsafe(no_mangle)]
pub extern "C" fn vmx_handle_exit(reason: u64) {
    use crate::kprintln;
    kprintln!("[vmx] VM-EXIT basic_reason={:#06x}", reason & 0xFFFF);
}
