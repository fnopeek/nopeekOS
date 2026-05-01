//! VMX root-mode entry/exit + VMCS round-trip + VMLAUNCH — 12.1.0b…0d-2b.
//!
//! Bring-up dance:
//!   1. Allocate a 4-KB VMXON region, write the VMCS revision-id.
//!   2. Unlock IA32_FEATURE_CONTROL if firmware left it open.
//!   3. Apply IA32_VMX_CR0/CR4_FIXED0/1 constraints to CR0/CR4 and
//!      set CR4.VMXE.
//!   4. Execute VMXON; check VMfailInvalid via RFLAGS.CF/ZF.       (12.1.0b)
//!   5. Allocate a 4-KB VMCS region, write the revision-id, run
//!      VMCLEAR + VMPTRLD against it.                              (12.1.0c)
//!   6. VMWRITE the host-state subset, VMREAD it back.             (12.1.0d-1)
//!   7. VMWRITE guest-state + execution controls; allocate a guest-
//!      code page with `hlt; jmp .`; VMLAUNCH; on the resulting
//!      VM-exit read VM_EXIT_REASON.                               (12.1.0d-2b)
//!   8. Execute VMXOFF.                                            (12.1.0b)
//!
//! VMXON + VMCS regions are allocated and *kept* (never freed). The
//! guest code page is allocated once and reused. CR4.VMXE is left
//! set — harmless and saves a write next time.
//!
//! Reference: Intel SDM Vol. 3C §23.7 (Enabling and Entering VMX
//! Operation), §24.11 (VMCS-Maintenance Instructions), §26.2
//! (VM-Entry Checks on Host State), §A.7-A.8 (VMX-Fixed Bits in
//! CR0/CR4).

use super::{ept, rdmsr, vmcs, wrmsr};
use crate::mm::memory;

const IA32_FEATURE_CONTROL: u32 = 0x3A;
const IA32_VMX_BASIC: u32 = 0x480;
const IA32_VMX_CR0_FIXED0: u32 = 0x486;
const IA32_VMX_CR0_FIXED1: u32 = 0x487;
const IA32_VMX_CR4_FIXED0: u32 = 0x488;
const IA32_VMX_CR4_FIXED1: u32 = 0x489;

const FEAT_CTRL_LOCK: u64 = 1 << 0;
const FEAT_CTRL_VMX_OUTSIDE_SMX: u64 = 1 << 2;

const CR4_VMXE: u64 = 1 << 13;

const RFLAGS_CF: u64 = 1 << 0;
const RFLAGS_ZF: u64 = 1 << 6;

/// Round-trip the full VMX bring-up: enter VMX root mode, set up a
/// minimal long-mode HLT-loop guest, VMLAUNCH, observe the VM-exit,
/// VMXOFF. On success, returns the basic VM-exit reason (expected:
/// 12 = HLT). On failure, a static string naming the step.
pub fn enable_and_test() -> Result<u16, &'static str> {
    // 1. Allocate a 4-KB frame for the VMXON region. Kernel memory
    //    is identity-mapped (virt == phys), so we can cast and write
    //    directly. The frame is leaked deliberately: 12.1.0c re-uses
    //    it.
    let region_phys = memory::allocate_frame().ok_or("OOM allocating VMXON region")?;

    // 2. Zero the page and write the VMCS revision-id at offset 0.
    //    SDM §A.1: bit 31 of the revision dword must be clear; the
    //    upper 1 bit signals "shadow VMCS" which only applies to
    //    VMCS pages, not VMXON. We're using the same constant for
    //    both per Intel guidance.
    let basic = unsafe { rdmsr(IA32_VMX_BASIC) };
    let revision_id = (basic & 0x7FFF_FFFF) as u32;

    // SAFETY: identity-mapped, freshly-allocated, exclusive ownership
    // — nothing else has a pointer to this frame.
    unsafe {
        let region = region_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }

    // 3. Unlock IA32_FEATURE_CONTROL if firmware left it unlocked,
    //    or verify the firmware-locked state is compatible. The
    //    probe() pre-check already rejected the locked-off case;
    //    this step covers the unlocked + locked-on cases.
    let feat = unsafe { rdmsr(IA32_FEATURE_CONTROL) };
    if feat & FEAT_CTRL_LOCK == 0 {
        // Unlocked — we set lock-bit + VMX-outside-SMX in one write.
        // Once the lock-bit is set, this MSR is read-only until next
        // boot (RESET clears it).
        let new = feat | FEAT_CTRL_LOCK | FEAT_CTRL_VMX_OUTSIDE_SMX;
        // SAFETY: writing lock + outside-SMX bits to architectural
        // MSR; value cannot fault (no reserved bits set).
        unsafe { wrmsr(IA32_FEATURE_CONTROL, new); }
    } else if feat & FEAT_CTRL_VMX_OUTSIDE_SMX == 0 {
        return Err("IA32_FEATURE_CONTROL locked with VMX disabled (BIOS lock)");
    }

    // 4. Apply VMX-fixed bits to CR0 and CR4, set CR4.VMXE.
    //    SDM §A.7: CR0_FIXED0 = "must be 1", CR0_FIXED1 = "may be 1"
    //    (so & FIXED1 clears must-be-0 bits). Same for CR4.
    let cr0_f0 = unsafe { rdmsr(IA32_VMX_CR0_FIXED0) };
    let cr0_f1 = unsafe { rdmsr(IA32_VMX_CR0_FIXED1) };
    let cr4_f0 = unsafe { rdmsr(IA32_VMX_CR4_FIXED0) };
    let cr4_f1 = unsafe { rdmsr(IA32_VMX_CR4_FIXED1) };

    let mut cr0: u64;
    let mut cr4: u64;
    // SAFETY: CR0/CR4 reads cannot fault.
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
    }
    cr0 = (cr0 | cr0_f0) & cr0_f1;
    cr4 = ((cr4 | cr4_f0) & cr4_f1) | CR4_VMXE;
    // SAFETY: values satisfy the architectural fixed-bit constraints
    // by construction; VMXE is always allowed when VMX is supported.
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }

    // 5. VMXON. Operand is a memory location holding the phys-addr
    //    of the VMXON region. We stash region_phys in a stack slot
    //    and pass its address.
    //    On success: RFLAGS.CF = 0 and RFLAGS.ZF = 0.
    //    VMfailInvalid (no current-VMCS): CF = 1.
    //    VMfailValid (current-VMCS error): ZF = 1; not applicable
    //    on first VMXON because no VMCS is loaded.
    let region_addr_slot: u64 = region_phys;
    let rflags: u64;
    // SAFETY: VMXON requires CR4.VMXE=1 (set above) and a valid
    // 4-KB-aligned region with revision-id (set above). pushfq/pop
    // touches the stack, hence no `nostack` option.
    unsafe {
        core::arch::asm!(
            "vmxon [{addr}]",
            "pushfq",
            "pop {flags}",
            addr = in(reg) &region_addr_slot,
            flags = lateout(reg) rflags,
        );
    }
    if rflags & RFLAGS_CF != 0 {
        return Err("VMXON returned VMfailInvalid (CF=1)");
    }
    if rflags & RFLAGS_ZF != 0 {
        return Err("VMXON returned VMfailValid (ZF=1) — unexpected on first call");
    }

    // 6. Now in VMX root mode. Run the VMCS + VMLAUNCH path. If any
    //    step fails we MUST still execute VMXOFF before returning,
    //    otherwise the CPU stays in VMX root mode forever.
    let result = vmcs_round_trip(revision_id);

    // 7. VMXOFF. Cleanly leave VMX root mode regardless of the inner
    //    test result. SAFETY: in VMX root mode (verified above).
    unsafe {
        core::arch::asm!("vmxoff", options(nostack, preserves_flags));
    }

    result
}

/// 12.1.0c…0d-2b: full VMCS life cycle inside VMX root mode.
/// Allocates the VMCS region, runs VMCLEAR + VMPTRLD, writes host
/// + guest + control state, runs VMLAUNCH, returns the basic exit
/// reason from the resulting VM-exit. All regions leaked
/// deliberately so subsequent invocations can reuse them.
///
/// Reference: Intel SDM Vol. 3C §24.11.3 (Initializing a VMCS),
/// §27 (VM Exits).
fn vmcs_round_trip(revision_id: u32) -> Result<u16, &'static str> {
    let vmcs_phys = memory::allocate_frame().ok_or("OOM allocating VMCS region")?;

    // SAFETY: identity-mapped, freshly-allocated, exclusive. Same
    // initialization pattern as the VMXON region — the revision-id
    // is the first dword, rest is zero.
    unsafe {
        let region = vmcs_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }

    let vmcs_addr_slot: u64 = vmcs_phys;

    // VMCLEAR — initialize the launch-state of the VMCS. Required
    // before the first VMPTRLD per SDM §24.11.3.
    let rflags_clear: u64;
    // SAFETY: in VMX root mode; argument is a valid 4-KB-aligned
    // VMCS region with revision-id set.
    unsafe {
        core::arch::asm!(
            "vmclear [{addr}]",
            "pushfq",
            "pop {flags}",
            addr = in(reg) &vmcs_addr_slot,
            flags = lateout(reg) rflags_clear,
        );
    }
    if rflags_clear & RFLAGS_CF != 0 {
        return Err("VMCLEAR returned VMfailInvalid (CF=1)");
    }
    if rflags_clear & RFLAGS_ZF != 0 {
        return Err("VMCLEAR returned VMfailValid (ZF=1)");
    }

    // VMPTRLD — make this VMCS current. Subsequent VMREAD/VMWRITE
    // operate on it.
    let rflags_load: u64;
    // SAFETY: in VMX root mode; VMCS just successfully VMCLEAR'd.
    unsafe {
        core::arch::asm!(
            "vmptrld [{addr}]",
            "pushfq",
            "pop {flags}",
            addr = in(reg) &vmcs_addr_slot,
            flags = lateout(reg) rflags_load,
        );
    }
    if rflags_load & RFLAGS_CF != 0 {
        return Err("VMPTRLD returned VMfailInvalid (CF=1)");
    }
    if rflags_load & RFLAGS_ZF != 0 {
        return Err("VMPTRLD returned VMfailValid (ZF=1)");
    }

    // 12.1.0d-1: write the host-state subset and read every field
    // back. host_rsp is a placeholder — the launch path overrides
    // HOST_RSP just-in-time with its own stack pointer.
    let host_rsp: u64;
    // SAFETY: pure register read.
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) host_rsp, options(nostack, preserves_flags));
    }
    vmcs::setup_host_state(host_rsp)?;

    // 12.1.1c-1: allocate a contiguous 16 MB host-physical region
    // for guest RAM, build a non-identity EPT that maps guest-phys
    // [0, 16 MB) → host-phys [host_base, host_base + 16 MB), copy
    // the real-mode stub at guest-phys 0x10000 (= host host_base +
    // 0x10000). The guest sees its code at guest-phys 0x10000;
    // EPT translates each fetch onto the host-allocated region so
    // the guest never touches kernel.bin's own load region (which
    // sits at host-phys 0x100000 = Multiboot2 1 MB).
    let host_base = memory::allocate_contiguous(ept::GUEST_RAM_FRAMES)
        .ok_or("OOM allocating 16 MB guest RAM")?;
    let eptp = ept::install_window_16mb(host_base)?;

    let stub_host = host_base + 0x10000;
    // SAFETY: host_base is 16-MB-aligned by `allocate_contiguous`'s
    // top-down search; stub_host is freshly allocated, exclusive.
    unsafe {
        let page = stub_host as *mut u8;
        core::ptr::write_bytes(page, 0, 4096);
        page.add(0).write_volatile(0xE6); // out imm8, al
        page.add(1).write_volatile(0x80); // port 0x80
        page.add(2).write_volatile(0xF4); // hlt
    }

    // Guest code lives at guest-phys 0x10000 (a Linux-Boot-Protocol-
    // friendly setup-segment address — bzImage will land here in
    // 12.1.1c-2). setup_guest_state sets GUEST_CS_BASE = guest_phys
    // and GUEST_RIP = 0, so guest linear address 0x10000 + 0 =
    // 0x10000 → EPT → host_base + 0x10000.
    let guest_phys: u64 = 0x10000;

    vmcs::setup_guest_state(guest_phys)?;
    vmcs::setup_execution_controls(eptp)?;

    let raw_reason = vmcs::launch_test()?;
    Ok(vmcs::basic_exit_reason(raw_reason))
}
