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
const EPT_POINTER: u64 = 0x201A;

// Natural-width VM-exit info.
const VM_EXIT_QUALIFICATION: u64 = 0x6400;

// Natural-width controls.
const CR0_GUEST_HOST_MASK: u64 = 0x6000;
const CR4_GUEST_HOST_MASK: u64 = 0x6002;
const CR0_READ_SHADOW: u64 = 0x6004;
const CR4_READ_SHADOW: u64 = 0x6006;

// VM-exit information (read-only).
const VM_INSTRUCTION_ERROR: u64 = 0x4400;
// VM_EXIT_REASON encoding 0x4402 is referenced as a literal inside
// the launch_test asm! block (Intel-syntax `mov rcx, 0x4402`).

// VMX capability MSRs for control allowed-0 / allowed-1 masking.
const IA32_VMX_PINBASED_CTLS: u32 = 0x481;
const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;
const IA32_VMX_EXIT_CTLS: u32 = 0x483;
const IA32_VMX_ENTRY_CTLS: u32 = 0x484;
const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;

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
/// IO_BITMAP_B (ports 0x8000-0xFFFF), zero them, set bits for the
/// ports we want to trap. Bit `n` of bitmap A means "exit on port n
/// access". B is left zero (ports above 0x8000 pass through).
///
/// Trapped ports:
///   - 0x80          — substrate-test stub (`out 0x80, al`)
///   - 0x3F8-0x3FF   — UART COM1 (Linux earlyprintk + 8250 register
///                     range; Linux probes the whole 8-port window)
///
/// Returns (io_bitmap_a_phys, io_bitmap_b_phys). Frames are leaked
/// so future entries reuse them; same lifecycle as VMXON / VMCS /
/// EPT regions.
fn allocate_and_populate_io_bitmaps() -> Result<(u64, u64), &'static str> {
    use crate::mm::memory;

    let bitmap_a = memory::allocate_frame().ok_or("OOM: IO_BITMAP_A")?;
    let bitmap_b = memory::allocate_frame().ok_or("OOM: IO_BITMAP_B")?;

    // SAFETY: identity-mapped, freshly allocated, exclusive. Each
    // bit covers one port: byte index = port / 8, bit-in-byte =
    // port % 8.
    unsafe {
        core::ptr::write_bytes(bitmap_a as *mut u8, 0, 4096);
        core::ptr::write_bytes(bitmap_b as *mut u8, 0, 4096);

        // Port 0x80 → byte 0x10, bit 0.
        let a = bitmap_a as *mut u8;
        a.add(0x80 / 8).write_volatile(1 << (0x80 % 8));

        // Ports 0x3F8..0x3FF → byte 0x7F, all 8 bits.
        a.add(0x3F8 / 8).write_volatile(0xFF);
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

// ── Execution controls ─────────────────────────────────────────────

// CPU-based control bits we care about.
const CPU_HLT_EXITING: u32 = 1 << 7;
const CPU_USE_IO_BITMAPS: u32 = 1 << 25;
const CPU_ACTIVATE_SECONDARY: u32 = 1 << 31;

// Secondary control bits.
const SEC_ENABLE_EPT: u32 = 1 << 1;
const SEC_UNRESTRICTED_GUEST: u32 = 1 << 7;

// VM-entry control bits. Long-mode-guest bit dropped — 12.1.1b boots
// the guest in real mode (CR0.PE=0) which requires IA-32e-mode-
// guest=0 (LMA=0 is implied).
//
// const ENTRY_IA32E_MODE_GUEST: u32 = 1 << 9;

// VM-exit control bits.
const EXIT_HOST_ADDR_SPACE_SIZE: u32 = 1 << 9;

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

    let pin = fixed_ctrl(0, IA32_VMX_PINBASED_CTLS);
    let cpu = fixed_ctrl(
        CPU_HLT_EXITING | CPU_USE_IO_BITMAPS | CPU_ACTIVATE_SECONDARY,
        IA32_VMX_PROCBASED_CTLS,
    );
    let secondary = fixed_ctrl(
        SEC_ENABLE_EPT | SEC_UNRESTRICTED_GUEST,
        IA32_VMX_PROCBASED_CTLS2,
    );
    let entry = fixed_ctrl(0, IA32_VMX_ENTRY_CTLS);
    let exit = fixed_ctrl(EXIT_HOST_ADDR_SPACE_SIZE, IA32_VMX_EXIT_CTLS);

    vmwrite(PIN_BASED_VM_EXEC_CONTROL, pin as u64)?;
    vmwrite(CPU_BASED_VM_EXEC_CONTROL, cpu as u64)?;
    vmwrite(SECONDARY_VM_EXEC_CONTROL, secondary as u64)?;
    vmwrite(IO_BITMAP_A_FULL, io_bitmap_a)?;
    vmwrite(IO_BITMAP_B_FULL, io_bitmap_b)?;
    vmwrite(EPT_POINTER, eptp)?;
    vmwrite(VM_ENTRY_CONTROLS, entry as u64)?;
    vmwrite(VM_EXIT_CONTROLS, exit as u64)?;

    // Inert ancillary controls — clear bitmaps + counts so VMX
    // doesn't dereference them.
    vmwrite(EXCEPTION_BITMAP, 0)?;
    vmwrite(CR3_TARGET_COUNT, 0)?;
    vmwrite(VM_EXIT_MSR_STORE_COUNT, 0)?;
    vmwrite(VM_EXIT_MSR_LOAD_COUNT, 0)?;
    vmwrite(VM_ENTRY_MSR_LOAD_COUNT, 0)?;
    vmwrite(VM_ENTRY_INTR_INFO_FIELD, 0)?;

    // Don't trap any guest CR-bit writes.
    vmwrite(CR0_GUEST_HOST_MASK, 0)?;
    vmwrite(CR4_GUEST_HOST_MASK, 0)?;
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

// 16-bit real-mode code: type=0xB (executable, readable, accessed),
// S=1, DPL=0, P=1, L=0 (not 64-bit), D/B=0 (16-bit operand size),
// G=0 (byte-granular). The accessed bit (type bit 0) is required
// even with unrestricted-guest=1 — the CPU's segment register
// loading would set it on a real selector load, but we write VMCS
// directly so we must encode the post-load value.
const AR_CODE16: u32 = 0xB | 0x10 | 0x80;
// 16-bit real-mode data: type=0x3 (RW, accessed), S=1, DPL=0, P=1.
const AR_DATA16: u32 = 0x3 | 0x10 | 0x80;
// Busy 64-bit TSS: type=0xB, S=0, DPL=0, P=1. Real mode doesn't
// actually use TR, but VMX still validates the AR encoding.
const AR_TSS_BUSY: u32 = 0xB | 0x80;
// Unusable segment marker (bit 16). Used for guest LDTR (no LDT).
const AR_UNUSABLE: u32 = 1 << 16;

/// Fill GUEST_* fields for an unrestricted real-mode guest. CR0
/// is computed by applying IA32_VMX_CR0_FIXED0/1 to a base of 0,
/// then forcibly clearing PE and PG (unrestricted-guest=1 relaxes
/// CR0_FIXED for those two bits per SDM §26.3.1.1). All six
/// segment-base fields use a configurable `code_base` for CS so
/// that real-mode 16-bit IP can index a high-physical guest page —
/// real-mode IP is 16-bit but the VMCS GUEST_CS_BASE is 64-bit and
/// not constrained to (CS << 4).
///
/// `guest_phys` is the host-physical address of the guest code
/// page (also the EPT-identity-mapped guest-physical address). RIP
/// is set to 0 (relative to CS_BASE = guest_phys), so the first
/// instruction fetched by the CPU is at host address `guest_phys`.
pub(super) fn setup_guest_state(guest_phys: u64) -> Result<(), &'static str> {
    // Compute a real-mode CR0: must satisfy CR0_FIXED0/1 except for
    // PE (bit 0) and PG (bit 31), which unrestricted-guest=1 lets
    // us clear regardless. Typical FIXED0 forces NE (bit 5) and ET
    // (bit 4) on; we keep those.
    let cr0_f0 = unsafe { rdmsr(0x486) };
    let cr0_f1 = unsafe { rdmsr(0x487) };
    let cr0_real = ((0u64 | cr0_f0) & cr0_f1) & !((1u64 << 0) | (1u64 << 31));

    // CR4: take host CR4 with VMXE etc. — real mode ignores most
    // CR4 bits but VMX still requires CR4_FIXED conformance. Host
    // CR4 already satisfies that.
    let host_cr4: u64;
    // SAFETY: pure register read.
    unsafe { core::arch::asm!("mov {}, cr4", out(reg) host_cr4, options(nostack, preserves_flags)); }

    vmwrite(GUEST_CR0, cr0_real)?;
    vmwrite(GUEST_CR3, 0)?;
    vmwrite(GUEST_CR4, host_cr4)?;

    // Real-mode selectors. Set CS=0 so the selector "matches" CS_BASE
    // = guest_phys — VMX with unrestricted-guest accepts any base
    // even when the selector encoding wouldn't normally produce it.
    vmwrite(GUEST_CS_SELECTOR, 0)?;
    vmwrite(GUEST_SS_SELECTOR, 0)?;
    vmwrite(GUEST_DS_SELECTOR, 0)?;
    vmwrite(GUEST_ES_SELECTOR, 0)?;
    vmwrite(GUEST_FS_SELECTOR, 0)?;
    vmwrite(GUEST_GS_SELECTOR, 0)?;
    vmwrite(GUEST_LDTR_SELECTOR, 0)?;
    vmwrite(GUEST_TR_SELECTOR, 0)?;

    // Bases. CS_BASE = guest_phys puts the 3-byte stub at IP=0.
    // Other segments base at 0 — the stub doesn't touch DS/SS/etc.
    vmwrite(GUEST_CS_BASE, guest_phys)?;
    vmwrite(GUEST_SS_BASE, 0)?;
    vmwrite(GUEST_DS_BASE, 0)?;
    vmwrite(GUEST_ES_BASE, 0)?;
    vmwrite(GUEST_FS_BASE, 0)?;
    vmwrite(GUEST_GS_BASE, 0)?;
    vmwrite(GUEST_LDTR_BASE, 0)?;
    vmwrite(GUEST_TR_BASE, 0)?;
    vmwrite(GUEST_GDTR_BASE, 0)?;
    vmwrite(GUEST_IDTR_BASE, 0)?;

    // Limits — real-mode segments default to 0xFFFF (16-bit).
    vmwrite(GUEST_CS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_SS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_DS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_ES_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_FS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_GS_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_LDTR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_TR_LIMIT, 0xFFFF)?;
    // Real-mode IDT: 256 entries × 4 bytes − 1 = 0x3FF.
    vmwrite(GUEST_GDTR_LIMIT, 0xFFFF)?;
    vmwrite(GUEST_IDTR_LIMIT, 0x3FF)?;

    // AR bytes. Real-mode 16-bit code/data, busy TSS, unusable LDTR.
    vmwrite(GUEST_CS_AR_BYTES, AR_CODE16 as u64)?;
    vmwrite(GUEST_SS_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_DS_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_ES_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_FS_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_GS_AR_BYTES, AR_DATA16 as u64)?;
    vmwrite(GUEST_LDTR_AR_BYTES, AR_UNUSABLE as u64)?;
    vmwrite(GUEST_TR_AR_BYTES, AR_TSS_BUSY as u64)?;

    // Misc guest state. EFER cleared (LME=LMA=0 — real mode,
    // long-mode disabled).
    vmwrite(GUEST_DR7, 0x400)?;
    vmwrite(GUEST_RFLAGS, 0x2)?;
    vmwrite(GUEST_RSP, 0)?;
    vmwrite(GUEST_RIP, 0)?; // CS_BASE = guest_phys, so IP=0 → host_phys = guest_phys
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

// ── VMLAUNCH + in-line resume ──────────────────────────────────────

/// Outcome of one VMLAUNCH invocation: basic exit reason + the
/// natural-width exit-qualification field (interpretation depends
/// on `exit_reason`) + guest RAX captured at the moment of VM-exit
/// (before host code clobbers it).
pub struct LaunchOutcome {
    pub exit_reason: u64,
    pub exit_qualification: u64,
    pub guest_rax: u64,
}

/// Run VMLAUNCH against the current VMCS, return the guest's first
/// VM-exit details. Caller must already have written host-state,
/// guest-state and execution-controls.
///
/// The asm! block has two control-flow paths that converge at
/// label 3:
///   1. VMLAUNCH succeeds: control transfers to the guest. Guest
///      executes its first trapping instruction (HLT or an I/O on
///      a port set in the I/O bitmap), the CPU loads host state
///      from VMCS — including HOST_RIP set to label 2 and HOST_RSP
///      set to our current stack pointer — and we land at label 2
///      with the guest's RAX still in rax. We push it onto the
///      stack first thing, then VMREAD the exit qualification
///      and reason fields.
///   2. VMLAUNCH fails (VMfail{Invalid,Valid}): no transition,
///      execution falls through. We push placeholders (0,0) for
///      qualification + guest_rax, set `failed`=1, jump to
///      convergence.
///
/// At label 3 both paths pop the same items in reverse order
/// (qualification, guest_rax, then the 6 callee-saved we pushed
/// up front). Rust unpacks the outputs.
pub(super) fn launch_test() -> Result<LaunchOutcome, &'static str> {
    let exit_reason: u64;
    let launch_failed: u64;
    let exit_qualification: u64;
    let guest_rax: u64;
    // SAFETY: caller guarantees VMX root mode + valid host/guest/
    // controls. The asm pushes 6 regs + 2 outcome slots to the
    // stack (no `nostack`). Both control-flow paths write to rax,
    // rdx, and the two output regs, satisfying Rust's lateout
    // invariants on every exit. Outputs in explicit registers
    // (rax, rdx) coexist with the lateout(reg) named operands and
    // `clobber_abi("C")`.
    unsafe {
        core::arch::asm!(
            // Save callee-saved regs across the guest boundary.
            "push rbp",
            "push rbx",
            "push r12",
            "push r13",
            "push r14",
            "push r15",

            // VMWRITE HOST_RSP, rsp.
            "mov rcx, 0x6C14",
            "vmwrite rcx, rsp",
            // VMWRITE HOST_RIP, label 2 (post-exit landing).
            "mov rcx, 0x6C16",
            "lea rax, [rip + 2f]",
            "vmwrite rcx, rax",

            "vmlaunch",

            // VMLAUNCH fall-through path: failed. Push placeholder
            // outcome slots (guest_rax = 0, qual = 0) so the pop
            // sequence at label 3 finds the same stack shape both
            // paths converge on.
            "mov rdx, 1",
            "xor rax, rax",
            "push rax",     // guest_rax = 0
            "push rax",     // exit_qualification = 0
            "jmp 3f",

            // Post-VM-exit landing pad. rax still holds the guest's
            // RAX at this exact instant; preserve it before any
            // VMREAD clobbers rax.
            "2:",
            "push rax",                 // save guest_rax
            "mov rcx, 0x6400",          // VM_EXIT_QUALIFICATION
            "vmread rax, rcx",
            "push rax",                 // save exit_qualification
            "mov rcx, 0x4402",          // VM_EXIT_REASON
            "vmread rax, rcx",          // rax = exit_reason
            "xor rdx, rdx",             // launch_failed = 0

            // Convergence — both paths land here with stack shape
            // [exit_qual, guest_rax, r15, r14, r13, r12, rbx, rbp].
            "3:",
            "pop rsi",          // exit_qualification
            "pop rdi",          // guest_rax
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbx",
            "pop rbp",

            // VM-exit unconditionally clears RFLAGS to 0x00000002
            // (SDM §27.5.3) — IF=0, no interrupts. The kernel ran
            // with IF=1 before; re-enable.
            "sti",

            lateout("rax") exit_reason,
            lateout("rdx") launch_failed,
            lateout("rsi") exit_qualification,
            lateout("rdi") guest_rax,
            out("rcx") _,
            clobber_abi("C"),
        );
    }

    if launch_failed != 0 {
        let err = vmread(VM_INSTRUCTION_ERROR).unwrap_or(0);
        use crate::kprintln;
        kprintln!("[vmx] VMLAUNCH failed: VM_INSTRUCTION_ERROR = {}", err);
        return Err("VMLAUNCH failed (see kernel log for VM_INSTRUCTION_ERROR)");
    }

    Ok(LaunchOutcome {
        exit_reason,
        exit_qualification,
        guest_rax,
    })
}

/// Read VM_EXIT_REASON's basic-reason field (bits 15:0). Convenience
/// for callers that already have the raw 32-bit value.
pub fn basic_exit_reason(raw: u64) -> u16 {
    (raw & 0xFFFF) as u16
}
