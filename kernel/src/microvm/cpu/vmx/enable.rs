//! VMX root-mode entry/exit + VMCS round-trip + VMLAUNCH — 12.1.0b…12.1.1c-3b3b2.
//!
//! Two consumer-facing entry points:
//!   - `enable_and_test()` — real-mode/32-bit-prot substrate test
//!     (9-byte stub `mov al,'O'; out 0x80,al; mov al,'K'; out 0x80,al; hlt`)
//!     used by `microvm test`.
//!   - `run_linux(bzimage, cmdline)` — Linux Boot Protocol 32-bit
//!     entry, used by `microvm linux`.
//!
//! Both share the VMXON region setup, VMCS allocation, VMCLEAR /
//! VMPTRLD, VMXOFF tear-down via `with_vmx_root_and_vmcs`. Guest
//! state, EPT, I/O bitmap, run-loop are caller-supplied.
//!
//! VMXON + VMCS regions are allocated and *kept* (never freed) per
//! call. CR4.VMXE is left set across calls (harmless).
//!
//! Reference: Intel SDM Vol. 3C §23.7 (Enabling VMX), §24.11.3
//! (Initializing a VMCS), §26.2-§26.4 (Host/Guest State), §27
//! (VM Exits).

use super::{ept, rdmsr, vmcs, wrmsr};
use crate::microvm::devices::guest_mem::GuestMem;
use crate::microvm::linux::bzimage;
use crate::mm::memory;
use core::sync::atomic::{AtomicBool, Ordering};
// LAPIC emulation is pure register state (only uses rdtsc) — reuse the SVM
// module's `LocalApic` rather than duplicate it. Intel parity #2. `IcrWrite`
// + `ICR_DM_*` are also reused by the cross-vCPU IPI router (guest SMP #4).
use crate::microvm::cpu::svm::lapic::{self, LocalApic};

/// True when the guest LAPIC is emulated on this (VMX ⇒ Intel) host. Cheap
/// const check (no vendor lock — vmx code only runs on Intel). Gates every
/// LAPIC-related VMX path; false ⇒ the validated `nolapic` boot, byte-
/// identical. See `cpu::VMX_GUEST_LAPIC`.
#[inline]
fn vmx_lapic_on() -> bool {
    crate::microvm::cpu::GUEST_LAPIC && crate::microvm::cpu::VMX_GUEST_LAPIC
}

// ── Guest-SMP (VMX #4) — mirror of svm/enable.rs ───────────────────────
//
// The Intel twin of the AMD guest-SMP machinery. Same architecture: split
// `VmContext` into a shared `VmShared` (one guest address space + device
// model, behind a `SharedRef` so an AP vCPU aliases the BSP's heap box) and a
// per-vCPU `Vcpu` (its own VMXON/VMCS/regs/FPU/LAPIC/apic_id). The BSP and AP
// run their VMRESUME loops as pinned pool fibers on separate worker cores, in
// parallel, serialising only their post-exit device handling under
// `VM_BIG_LOCK`. The validated AMD stumbling blocks are pre-empted here:
// per-CORE VMXON/VMCS (each vCPU enters VMX root on its own core), the AP
// never locks VENDOR (it learns its vendor lock-free via `detect_vendor`),
// IPIs inject only when interruptible (reason-33 guard), idle-park is gated on
// AP_ESTABLISHED, and devices/timer/nat/input are BSP-only.

/// Big-VM lock (guest SMP). Held by a vCPU only around its post-exit
/// device/MMIO/IO handling — NEVER across `run_guest_once` or a fiber yield —
/// so the BSP and an AP serialise all access to the shared `VmShared` while
/// their VMRESUMEs run truly in parallel on separate host cores. Uncontended
/// (never even taken) until an AP is admitted (`AP_ACTIVE`).
static VM_BIG_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// True once a second vCPU (AP) shares this VM. While false the BSP runs
/// exactly as the single-vCPU path did — the lock is not taken, so the hot
/// loop is byte-identical. Set by the orchestration layer when it spawns an
/// AP fiber, cleared after the last vCPU exits.
pub static AP_ACTIVE: AtomicBool = AtomicBool::new(false);

#[inline]
fn ap_active() -> bool {
    AP_ACTIVE.load(Ordering::Acquire)
}

/// Maximum vCPUs per guest (guest SMP). Sizes the per-vCPU IPI bitmaps;
/// `cpu::guest_vcpus()` (the count actually enumerated) must be ≤ this.
pub const MAX_VCPUS: usize = 8;

/// Bounded adaptive halt-polling (KVM `halt_poll_ns` model). When a vCPU goes
/// idle it polls — re-pumping + re-checking for a wake event (RX / IRQ / IPI) —
/// for up to its current window before truly parking. A wake within the window
/// grows the window (×2, latency pays off); an expiry shrinks it (÷2, the vCPU
/// was idle, save power). This replaces the old global `recently_active` spin:
/// idle vCPUs shrink toward the floor (~park), a latency-sensitive vCPU (the
/// network BSP between RX bursts) grows toward the cap, and a busy vCPU never
/// reaches the idle path at all. Per-vCPU, no global state. µs units.
const HALT_POLL_MIN_US: u64 = 5;   // floor — always poll a little, recoverable
const HALT_POLL_MAX_US: u64 = 200; // KVM default cap

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

// ── VMXON / VMCS plumbing (shared by all entry points) ─────────────

/// Run `inner` inside VMX root mode with a fresh, current VMCS.
/// Handles all the VMXON-region / FEATURE_CONTROL / CR0+CR4 fixed-bit
/// dance once, allocates a 4-KB VMCS region, runs VMCLEAR + VMPTRLD,
/// then calls `inner` (which operates on the current VMCS via
/// VMREAD/VMWRITE / EPT / etc.). VMXOFF runs unconditionally on
/// return, even on inner error, so the CPU never strands in VMX
/// root mode.
fn with_vmx_root_and_vmcs<F, T>(inner: F) -> Result<T, &'static str>
where
    F: FnOnce() -> Result<T, &'static str>,
{
    // 1. VMXON region.
    let region_phys = memory::allocate_frame().ok_or("OOM allocating VMXON region")?;
    let basic = unsafe { rdmsr(IA32_VMX_BASIC) };
    let revision_id = (basic & 0x7FFF_FFFF) as u32;

    // SAFETY: identity-mapped, freshly-allocated, exclusive.
    unsafe {
        let region = region_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }

    // 2. FEATURE_CONTROL.
    let feat = unsafe { rdmsr(IA32_FEATURE_CONTROL) };
    if feat & FEAT_CTRL_LOCK == 0 {
        let new = feat | FEAT_CTRL_LOCK | FEAT_CTRL_VMX_OUTSIDE_SMX;
        // SAFETY: writing lock + outside-SMX bits to architectural MSR.
        unsafe { wrmsr(IA32_FEATURE_CONTROL, new); }
    } else if feat & FEAT_CTRL_VMX_OUTSIDE_SMX == 0 {
        return Err("IA32_FEATURE_CONTROL locked with VMX disabled (BIOS lock)");
    }

    // 3. CR0/CR4 fixed bits + CR4.VMXE.
    let cr0_f0 = unsafe { rdmsr(IA32_VMX_CR0_FIXED0) };
    let cr0_f1 = unsafe { rdmsr(IA32_VMX_CR0_FIXED1) };
    let cr4_f0 = unsafe { rdmsr(IA32_VMX_CR4_FIXED0) };
    let cr4_f1 = unsafe { rdmsr(IA32_VMX_CR4_FIXED1) };

    let mut cr0: u64;
    let mut cr4: u64;
    // SAFETY: CR reads cannot fault.
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
    }
    cr0 = (cr0 | cr0_f0) & cr0_f1;
    cr4 = ((cr4 | cr4_f0) & cr4_f1) | CR4_VMXE;
    // SAFETY: values satisfy fixed-bit constraints; VMXE is allowed.
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }

    // 4. VMXON.
    let region_addr_slot: u64 = region_phys;
    let rflags: u64;
    // SAFETY: VMXON requires CR4.VMXE (set above) + valid 4-KB
    // region with revision-id (set above).
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

    // 5. VMCS region + VMCLEAR + VMPTRLD.
    let inner_result = vmcs_setup_then_inner(revision_id, inner);

    // 6. VMXOFF — always runs.
    // SAFETY: in VMX root mode (verified above).
    unsafe {
        core::arch::asm!("vmxoff", options(nostack, preserves_flags));
    }

    inner_result
}

fn vmcs_setup_then_inner<F, T>(revision_id: u32, inner: F) -> Result<T, &'static str>
where
    F: FnOnce() -> Result<T, &'static str>,
{
    let vmcs_phys = memory::allocate_frame().ok_or("OOM allocating VMCS region")?;

    // SAFETY: identity-mapped, freshly-allocated, exclusive.
    unsafe {
        let region = vmcs_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }

    let vmcs_addr_slot: u64 = vmcs_phys;

    // VMCLEAR.
    let rflags_clear: u64;
    // SAFETY: in VMX root mode; valid VMCS region.
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

    // VMPTRLD.
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

    inner()
}

/// Sample current RSP and write it into HOST_RSP as a placeholder.
/// The real run-loop overrides HOST_RSP just-in-time before each
/// VMLAUNCH/VMRESUME — but the field must be canonical between
/// `setup_host_state` and the launch.
fn write_host_state_with_current_rsp() -> Result<(), &'static str> {
    let host_rsp: u64;
    // SAFETY: pure register read.
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) host_rsp, options(nostack, preserves_flags));
    }
    vmcs::setup_host_state(host_rsp)
}

/// Allocate a fresh 64 MB contiguous host-physical region for the
/// guest, install the EPT mapping it onto guest-phys [0, 64 MB),
/// return (host_base, eptp).
/// Allocate `guest_bytes` of contiguous guest RAM (+ 2-MB-align
/// slack) and install the EPT window over it. `close()` frees exactly
/// `ept::total_frames_for(guest_bytes)` from `raw_base`.
/// B3: allocate only the **contiguous boot window** (256 MiB or the
/// whole guest if smaller); `[boot, guest_bytes)` is demand-paged 4 KB
/// and needs no upfront allocation. Returns
/// `(boot_base, eptp, pml4_phys, boot_raw_base)`.
fn alloc_guest_ram_and_ept(guest_bytes: u64) -> Result<(u64, u64, u64, u64), &'static str> {
    let raw_base = memory::allocate_contiguous(ept::boot_frames_for(guest_bytes))
        .ok_or("OOM allocating guest boot window (+ slack)")?;
    let boot_base = ept::round_up_to_2mb(raw_base);
    let (eptp, pml4_phys) = ept::install_window(boot_base, guest_bytes)?;
    Ok((boot_base, eptp, pml4_phys, raw_base))
}

// ── Substrate test (12.1.1c-3b3a / 3b3b1) ──────────────────────────

/// Real-mode I/O-loop substrate test. Allocates fresh resources,
/// runs the 9-byte `out 0x80, 'O'; out 0x80, 'K'; hlt` stub,
/// returns the final VM-exit outcome. Used by `microvm test`.
pub fn enable_and_test() -> Result<vmcs::LaunchOutcome, &'static str> {
    with_vmx_root_and_vmcs(|| {
        // Substrate test runs a tiny real-mode stub at gpa 0x10000;
        // size-insensitive → a 4 MiB all-contiguous boot window (no
        // demand PTs). host_base is the boot block base.
        let (host_base, eptp, _pml4, _raw_base) =
            alloc_guest_ram_and_ept(4 * 1024 * 1024)?;

        // 9-byte substrate stub at guest-phys 0x10000.
        let stub_host = host_base + 0x10000;
        // SAFETY: host_base is 2-MB-aligned and the [host_base,
        // host_base + 64 MB) window is exclusively ours.
        unsafe {
            let page = stub_host as *mut u8;
            core::ptr::write_bytes(page, 0, 4096);
            page.add(0).write_volatile(0xB0); page.add(1).write_volatile(0x4F); // mov al, 'O'
            page.add(2).write_volatile(0xE6); page.add(3).write_volatile(0x80); // out 0x80, al
            page.add(4).write_volatile(0xB0); page.add(5).write_volatile(0x4B); // mov al, 'K'
            page.add(6).write_volatile(0xE6); page.add(7).write_volatile(0x80); // out 0x80, al
            page.add(8).write_volatile(0xF4);                                    // hlt
        }

        write_host_state_with_current_rsp()?;
        vmcs::setup_guest_state(0x10000)?;
        vmcs::setup_execution_controls(eptp)?;

        run_substrate_loop()
    })
}

/// Loop dispatch for the substrate test: HLT terminates, OUT
/// captures the byte for the "OK" reconstruction, anything else
/// breaks with a log line.
fn run_substrate_loop() -> Result<vmcs::LaunchOutcome, &'static str> {
    use crate::kprintln;

    const MAX_ITERATIONS: u32 = 1024;

    let mut regs = vmcs::GuestRegs::default();
    let mut launched = false;
    let mut last_outcome: Option<vmcs::LaunchOutcome> = None;
    let mut io_count: u32 = 0;
    let mut io_bytes: [u8; 32] = [0; 32];
    let mut io_byte_n: usize = 0;

    // FPU areas required by run_guest_once's in-asm xsave/xrstor.
    // The substrate stub doesn't touch FPU but the asm unconditionally
    // brackets vmresume with host/guest FPU swap; pass valid areas.
    let mut host_fpu = crate::microvm::cpu::FpuArea::boxed();
    let mut guest_fpu = crate::microvm::cpu::FpuArea::boxed();

    for _ in 0..MAX_ITERATIONS {
        let outcome = vmcs::run_guest_once(
            &mut regs, launched, &mut *host_fpu, &mut *guest_fpu)?;
        launched = true;
        let basic = vmcs::basic_exit_reason(outcome.exit_reason);

        match basic {
            1 => {
                // External interrupt — host IRQ that arrived during
                // guest run. The `sti` at the tail of run_guest_once
                // already let the host IDT dispatch it; just resume.
                last_outcome = Some(outcome);
            }
            12 => {
                kprintln!("[microvm] guest HLT after {} I/O exit(s)", io_count);
                if io_byte_n > 0 {
                    let mut printable = [0u8; 32];
                    for i in 0..io_byte_n {
                        printable[i] = if io_bytes[i].is_ascii_graphic() || io_bytes[i] == b' ' {
                            io_bytes[i]
                        } else {
                            b'.'
                        };
                    }
                    let s = core::str::from_utf8(&printable[..io_byte_n]).unwrap_or("?");
                    kprintln!("[microvm]   captured byte stream: \"{}\"", s);
                }
                last_outcome = Some(outcome);
                break;
            }
            30 => {
                io_count += 1;
                let (port, dir_in, size) =
                    vmcs::decode_io_exit_qualification(outcome.exit_qualification);
                let value = regs.rax & match size {
                    1 => 0xFF, 2 => 0xFFFF, 4 => 0xFFFF_FFFF, _ => 0xFF,
                };
                let dir = if dir_in { "IN" } else { "OUT" };
                kprintln!(
                    "[microvm]   {} port {:#06x} size={} value={:#x}",
                    dir, port, size, value,
                );
                if !dir_in && size == 1 && io_byte_n < io_bytes.len() {
                    io_bytes[io_byte_n] = value as u8;
                    io_byte_n += 1;
                }
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            _ => {
                kprintln!(
                    "[microvm] guest unhandled exit reason {} qual {:#x}",
                    basic, outcome.exit_qualification,
                );
                last_outcome = Some(outcome);
                break;
            }
        }
    }

    last_outcome.ok_or("guest exceeded max iterations without HLT")
}

// ── Linux launcher (12.1.1c-3b3b2) ─────────────────────────────────

/// Boot a Linux bzImage in our MicroVM substrate. Loads the bzImage
/// parts into a fresh 64 MB guest, configures 32-bit-prot-mode
/// entry per Linux Boot Protocol, runs a serial-aware exit loop
/// that captures Linux's earlyprintk output via the I/O bitmap.
///
/// `bzimage` is the raw bzImage bytes. `cmdline` is the kernel
/// command line (no NUL — loader appends one).
// ── Re-entrant VM context (Phase 12.4 step 1a) ─────────────────────
//
// The Linux run-loop is split into open() / run_slice() / close() so
// the Core-0 event loop can interleave Shade rendering between bounded
// slices instead of blocking until guest exit (see
// PHASE12_DISPLAY_BRIDGE.md, R1). Step 1a is behaviour-preserving:
// `run_linux` calls run_slice(u32::MAX) once, identical to the old
// `run_linux_loop`. Slicing + interleave is step 1b.
//
// vmx_enter_root/vmx_exit_root duplicate the VMXON + VMCS asm from
// `with_vmx_root_and_vmcs` (which the substrate-test path still uses
// unchanged — zero risk to proven code). TODO(cleanup): dedupe once
// the VmContext path is NUC-validated. Tracked.

/// Outcome of one bounded slice of guest execution.
pub enum SliceOutcome {
    /// Budget exhausted, guest still running — caller may re-enter
    /// immediately (busy guest).
    StillRunning,
    /// Guest is idle (halted / waiting on its timer) — caller should
    /// re-enter but may host-idle first so the dedicated core doesn't
    /// spin VMRUN while the guest has nothing to do.
    Idle,
    /// Guest exited (HLT / panic / triple-fault / idle / cap).
    Exited(vmcs::LaunchOutcome),
}

/// Enter VMX root mode and load a fresh current VMCS. Returns the
/// (kept, never-freed) VMXON + VMCS region phys addrs. Faithful copy
/// of `with_vmx_root_and_vmcs` steps 1-5 + `vmcs_setup_then_inner`.
fn vmx_enter_root() -> Result<(u64, u64), &'static str> {
    // 1. VMXON region.
    let region_phys = memory::allocate_frame().ok_or("OOM allocating VMXON region")?;
    let basic = unsafe { rdmsr(IA32_VMX_BASIC) };
    let revision_id = (basic & 0x7FFF_FFFF) as u32;
    // SAFETY: identity-mapped, freshly-allocated, exclusive.
    unsafe {
        let region = region_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }

    // 2. FEATURE_CONTROL.
    let feat = unsafe { rdmsr(IA32_FEATURE_CONTROL) };
    if feat & FEAT_CTRL_LOCK == 0 {
        let new = feat | FEAT_CTRL_LOCK | FEAT_CTRL_VMX_OUTSIDE_SMX;
        // SAFETY: writing lock + outside-SMX bits to architectural MSR.
        unsafe { wrmsr(IA32_FEATURE_CONTROL, new); }
    } else if feat & FEAT_CTRL_VMX_OUTSIDE_SMX == 0 {
        return Err("IA32_FEATURE_CONTROL locked with VMX disabled (BIOS lock)");
    }

    // 3. CR0/CR4 fixed bits + CR4.VMXE.
    let cr0_f0 = unsafe { rdmsr(IA32_VMX_CR0_FIXED0) };
    let cr0_f1 = unsafe { rdmsr(IA32_VMX_CR0_FIXED1) };
    let cr4_f0 = unsafe { rdmsr(IA32_VMX_CR4_FIXED0) };
    let cr4_f1 = unsafe { rdmsr(IA32_VMX_CR4_FIXED1) };
    let mut cr0: u64;
    let mut cr4: u64;
    // SAFETY: CR reads cannot fault.
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
    }
    cr0 = (cr0 | cr0_f0) & cr0_f1;
    cr4 = ((cr4 | cr4_f0) & cr4_f1) | CR4_VMXE;
    // SAFETY: values satisfy fixed-bit constraints; VMXE is allowed.
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }

    // 4. VMXON.
    let region_addr_slot: u64 = region_phys;
    let rflags: u64;
    // SAFETY: VMXON requires CR4.VMXE (set above) + valid 4-KB region.
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

    // 5. VMCS region + VMCLEAR + VMPTRLD.
    let vmcs_phys = match memory::allocate_frame() {
        Some(p) => p,
        None => {
            // Back out of VMX root so the CPU isn't stranded.
            unsafe { vmx_exit_root(); }
            return Err("OOM allocating VMCS region");
        }
    };
    // SAFETY: identity-mapped, freshly-allocated, exclusive.
    unsafe {
        let region = vmcs_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }
    let vmcs_addr_slot: u64 = vmcs_phys;
    let rflags_clear: u64;
    // SAFETY: in VMX root mode; valid VMCS region.
    unsafe {
        core::arch::asm!(
            "vmclear [{addr}]",
            "pushfq",
            "pop {flags}",
            addr = in(reg) &vmcs_addr_slot,
            flags = lateout(reg) rflags_clear,
        );
    }
    if rflags_clear & (RFLAGS_CF | RFLAGS_ZF) != 0 {
        unsafe { vmx_exit_root(); }
        return Err("VMCLEAR failed (VMfail)");
    }
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
    if rflags_load & (RFLAGS_CF | RFLAGS_ZF) != 0 {
        unsafe { vmx_exit_root(); }
        return Err("VMPTRLD failed (VMfail)");
    }

    Ok((region_phys, vmcs_phys))
}

/// Leave VMX root mode. Safe to call exactly once per successful
/// `vmx_enter_root`.
///
/// # Safety
/// Caller must be in VMX root mode (a prior `vmx_enter_root` Ok).
unsafe fn vmx_exit_root() {
    // SAFETY: precondition documented; VMXOFF in VMX root is defined.
    unsafe {
        core::arch::asm!("vmxoff", options(nostack, preserves_flags));
    }
}

/// State shared by ALL vCPUs of one microvm: the single guest address space
/// (RAM + EPT), the device model, and the host-tick/display bookkeeping. The
/// BSP owns it heap-boxed (behind `SharedRef`); an AP aliases it. Mirror of
/// svm `VmShared`.
pub struct VmShared {
    guest_mem: GuestMem,
    /// EPT PML4 phys — `close()` passes it to `ept::release` to free
    /// every demand-faulted 4 KB frame + the demand PT pages + the
    /// fixed tables.
    ept_pml4: u64,
    /// EPTP (PML4 phys | walk-length | mem-type) to VMWRITE into each
    /// vCPU's VMCS::EPT_POINTER. The AP reuses it (one shared address
    /// space) in `open_ap`'s `setup_execution_controls`.
    eptp: u64,
    /// Base of the **contiguous boot-window** allocation (pre-2 MB-
    /// align). `close()` frees `ept::boot_frames_for(guest_mem.len())`
    /// from here; the demand region is freed via `ept::release`.
    guest_raw_base: u64,
    serial: SerialState,
    pci: crate::microvm::devices::PciBus,
    pic: crate::microvm::devices::pic8259::Pic8259,
    /// Host tick of the last injected guest timer IRQ0 (≈100 Hz). The only
    /// thing that wakes a time-blocked guest task (no PIT/LAPIC source).
    last_timer_tick: u64,
    /// `ticks()` of the last virtio-gpu display config-change IRQ — rate-
    /// limits the resize round-trip (R2 debounce).
    last_cfg_tick: u64,
    /// Device IRQ lines raised while the guest was NOT interruptible (IF=0 /
    /// STI / MOV-SS shadow). Injecting a type-0 external interrupt then fails
    /// VM-entry with reason 33 on bare-metal Intel (SDM §26.3.1.5); the run
    /// loop drains these once the guest is interruptible. SVM's `pending_irqs`
    /// twin (the reason-33 fix).
    pending_irqs: u16,
    /// True until Linux writes PIT mode-0 (port 0x43 ← 0x30) to disable the
    /// i8253 after adopting the LAPIC timer. Gates IRQ0 so jiffies don't
    /// double-count. During calibration BOTH tick 1:1 (else "APIC timer
    /// disabled").
    pit_enabled: bool,
    /// Pending IPI vectors per target vCPU, indexed by apic_id: a 256-bit
    /// bitmap each. A vCPU's ICR (FIXED) write sets bits in the target's word;
    /// each vCPU drains its own word + injects the lowest pending vector when
    /// interruptible. Carries reschedule/TLB IPIs + the AP→BSP `complete()`
    /// wakeup the cpuhp bring-up needs. Guest SMP only.
    ipi_pending: [[u64; 4]; MAX_VCPUS],
}

impl VmShared {
    /// Mark interrupt `vector` pending for the vCPU with `apic_id` (an IPI
    /// target). No-op if the id is out of range.
    fn ipi_set(&mut self, apic_id: u8, vector: u8) {
        let t = apic_id as usize;
        if t < MAX_VCPUS {
            self.ipi_pending[t][(vector >> 6) as usize] |= 1u64 << (vector & 63);
        }
    }

    /// Take (and clear) the lowest pending IPI vector for `apic_id`, if any.
    fn ipi_take(&mut self, apic_id: u8) -> Option<u8> {
        let t = apic_id as usize;
        if t >= MAX_VCPUS {
            return None;
        }
        for (w, word) in self.ipi_pending[t].iter_mut().enumerate() {
            if *word != 0 {
                let bit = word.trailing_zeros();
                *word &= !(1u64 << bit);
                return Some((w as u32 * 64 + bit) as u8);
            }
        }
        None
    }
}

/// Per-vCPU state: its own VMXON region + VMCS (VMX root is PER-CORE, so each
/// vCPU enters root on its own worker core), register file, FPU areas, LAPIC,
/// and per-vCPU exit bookkeeping. Guest SMP = N of these against one shared
/// `VmShared`. Mirror of svm `Vcpu`.
pub struct Vcpu {
    /// This vCPU's physical (x)APIC ID — BSP = 0, APs = 1.. Reported
    /// identically by CPUID (leaf 1 EBX[31:24], leaf 0xB/0x1F EDX), the
    /// emulated LAPIC, the IA32_APICBASE BSP bit (set only when 0), and the
    /// MP-table, or Linux's topology code rejects the CPU.
    apic_id: u8,
    vmxon_phys: u64,
    vmcs_phys: u64,
    regs: vmcs::GuestRegs,
    trace: ExitTrace,
    io_stats: IoStats,
    launched: bool,
    iter: u32,
    io_dropped: u32,
    msr_log_count: u32,
    consecutive_idle: u32,
    /// Event mid-vectoring at the last exit (IDT_VECTORING_INFO; re-injected
    /// next entry — only type 0/2). SDM §27.2.4. See the field doc on the SVM
    /// twin for the type-gate rationale.
    reinject: u64,
    /// Per-vCPU emulated local APIC (Intel parity #2 / guest SMP). Inert while
    /// booting `nolapic`; drives the timer + IPIs once Linux enables it.
    lapic: LocalApic,
    /// `ticks()` of the last injected LAPIC-timer (LVTT) IRQ, paced like the
    /// PIT's `last_timer_tick`.
    last_lapic_tick: u64,
    /// Bounded adaptive halt-poll window (µs) + its current TSC deadline (0 =
    /// not polling). See `HALT_POLL_MIN_US`.
    halt_poll_us: u64,
    halt_poll_deadline: u64,
    /// Host/guest FPU (XSAVE) save areas — VMRESUME preserves neither.
    host_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,
    guest_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,
}

/// Handle to a microvm's `VmShared`. The BSP vCPU **owns** it (heap-boxed so
/// its address is stable while the BSP's `VmContext` moves on the fiber
/// stack); an AP holds `Borrowed` — a raw pointer to the same box, valid for
/// the VM lifetime via the last-one-out refcount. `Deref`/`DerefMut` make
/// every `self.shared.X` access work for both; access is serialised by
/// `VM_BIG_LOCK`, not this handle. Mirror of svm `SharedRef`.
pub enum SharedRef {
    Owned(alloc::boxed::Box<VmShared>),
    Borrowed(*mut VmShared),
}

// SAFETY: Borrowed is a raw pointer moved only into AP fiber tasks; all
// pointee access is VM_BIG_LOCK-serialised. The Send marker lets it cross
// into the spawned fiber.
unsafe impl Send for SharedRef {}

impl SharedRef {
    fn owned(s: VmShared) -> Self {
        SharedRef::Owned(alloc::boxed::Box::new(s))
    }
}

impl core::ops::Deref for SharedRef {
    type Target = VmShared;
    fn deref(&self) -> &VmShared {
        match self {
            SharedRef::Owned(b) => b,
            // SAFETY: valid for the VM lifetime (see SharedRef doc).
            SharedRef::Borrowed(p) => unsafe { &**p },
        }
    }
}

impl core::ops::DerefMut for SharedRef {
    fn deref_mut(&mut self) -> &mut VmShared {
        match self {
            SharedRef::Owned(b) => b,
            // SAFETY: as above; access is VM_BIG_LOCK-serialised.
            SharedRef::Borrowed(p) => unsafe { &mut **p },
        }
    }
}

/// Persistent state of one Linux microvm across cooperative slices.
/// Core-agnostic: nothing here assumes which core it runs on (forward-compat
/// contract #1). Composed of `VmShared` (all-vCPU, behind `SharedRef`) + one
/// `Vcpu`.
pub struct VmContext {
    shared: SharedRef,
    vcpu: Vcpu,
}

impl VmContext {
    /// Enter VMX root, load the VMCS, place the guest image, set up
    /// guest/host state + execution controls, pre-inject the UART RX
    /// FIFO. On any post-VMXON failure, VMXOFF before returning Err so
    /// the CPU never strands in VMX root.
    pub fn open(
        bzimage: &[u8],
        cmdline: &[u8],
        initramfs: Option<&[u8]>,
        inject: &[u8],
    ) -> Result<VmContext, &'static str> {
        use crate::kprintln;

        let (vmxon_phys, vmcs_phys) = vmx_enter_root()?;

        // Everything past here is in VMX root: VMXOFF on any error.
        let build = || -> Result<VmContext, &'static str> {
            let guest_bytes = crate::microvm::cpu::choose_guest_ram_bytes();
            let (boot_base, eptp, ept_pml4, guest_raw_base) =
                alloc_guest_ram_and_ept(guest_bytes)?;
            let gm = GuestMem::new(
                boot_base,
                ept::boot_window_bytes(guest_bytes),
                guest_bytes,
                ept_pml4,
                crate::microvm::devices::guest_mem::SecondLevel::Ept,
            );
            let load = bzimage::load_into_guest_ram(&gm, bzimage, cmdline, initramfs)?;
            write_host_state_with_current_rsp()?;
            vmcs::setup_guest_state(load.entry_rip)?;
            vmcs::setup_execution_controls(eptp)?;

            let mut regs = vmcs::GuestRegs::default();
            regs.rsi = load.boot_params_phys;

            let mut serial = SerialState::new();
            if !inject.is_empty() {
                serial.inject(inject);
                kprintln!("[microvm] pre-injected {} bytes into UART RX FIFO", inject.len());
            }

            Ok(VmContext {
                shared: SharedRef::owned(VmShared {
                    guest_mem: gm,
                    ept_pml4,
                    eptp,
                    guest_raw_base,
                    serial,
                    pci: crate::microvm::devices::PciBus::new(),
                    pic: crate::microvm::devices::pic8259::Pic8259::new(),
                    last_timer_tick: 0,
                    last_cfg_tick: 0,
                    pending_irqs: 0,
                    pit_enabled: true,
                    ipi_pending: [[0; 4]; MAX_VCPUS],
                }),
                vcpu: Vcpu {
                    apic_id: 0, // BSP
                    vmxon_phys,
                    vmcs_phys,
                    regs,
                    trace: ExitTrace::new(),
                    io_stats: IoStats::new(),
                    launched: false,
                    iter: 0,
                    io_dropped: 0,
                    msr_log_count: 0,
                    consecutive_idle: 0,
                    reinject: 0,
                    lapic: LocalApic::new(0),
                    last_lapic_tick: 0,
                    halt_poll_us: HALT_POLL_MIN_US,
                    halt_poll_deadline: 0,
                    host_fpu: crate::microvm::cpu::FpuArea::boxed(),
                    guest_fpu: crate::microvm::cpu::FpuArea::boxed(),
                },
            })
        };

        match build() {
            Ok(ctx) => Ok(ctx),
            Err(e) => {
                // SAFETY: vmx_enter_root succeeded → we are in VMX root.
                unsafe { vmx_exit_root(); }
                Err(e)
            }
        }
    }

    /// Build an AP vCPU that **shares** an already-open BSP's `VmShared`
    /// (guest SMP). Enters VMX root ON THIS (the AP's) worker core — VMX root
    /// + VMCS are per-physical-core, so the AP allocates its own VMXON/VMCS
    /// here — installs this core's TSS (valid HOST_TR, like `vm_open`),
    /// configures a real-mode SIPI-entry guest state, and reuses the BSP's
    /// shared EPTP / device model. No guest RAM / EPT / device allocation
    /// (those belong to the BSP, freed last-one-out). `apic_id` is 1.. Mirror
    /// of svm `open_ap`.
    pub fn open_ap(
        shared: *mut VmShared,
        sipi_vector: u8,
        apic_id: u8,
    ) -> Result<VmContext, &'static str> {
        // Worker-core TSS so HOST_TR is valid at VM-entry (see vmx::vm_open).
        crate::tss::ensure_core(crate::smp::per_core::current_core_id());

        let (vmxon_phys, vmcs_phys) = vmx_enter_root()?;

        // In VMX root now: VMXOFF on any error so the core never strands.
        let build = || -> Result<VmContext, &'static str> {
            // Read the shared EPTP (set once at BSP open, never mutated). Take
            // VM_BIG_LOCK: AP_ACTIVE is set by now, so the BSP may be touching
            // `*shared` under the lock — holding it keeps this `&*shared`
            // borrow from aliasing the BSP's `&mut`.
            let eptp = {
                let _big = VM_BIG_LOCK.lock();
                // SAFETY: `shared` is valid for the VM lifetime (SharedRef),
                // and the lock excludes any concurrent `&mut` to `*shared`.
                unsafe { (*shared).eptp }
            };
            write_host_state_with_current_rsp()?;
            vmcs::setup_guest_state_ap(sipi_vector)?;
            vmcs::setup_execution_controls(eptp)?;

            Ok(VmContext {
                shared: SharedRef::Borrowed(shared),
                vcpu: Vcpu {
                    apic_id,
                    vmxon_phys,
                    vmcs_phys,
                    regs: vmcs::GuestRegs::default(),
                    trace: ExitTrace::new(),
                    io_stats: IoStats::new(),
                    launched: false,
                    iter: 0,
                    io_dropped: 0,
                    msr_log_count: 0,
                    consecutive_idle: 0,
                    reinject: 0,
                    lapic: LocalApic::new(apic_id),
                    last_lapic_tick: 0,
                    halt_poll_us: HALT_POLL_MIN_US,
                    halt_poll_deadline: 0,
                    host_fpu: crate::microvm::cpu::FpuArea::boxed(),
                    guest_fpu: crate::microvm::cpu::FpuArea::boxed(),
                },
            })
        };

        match build() {
            Ok(ctx) => Ok(ctx),
            Err(e) => {
                // SAFETY: vmx_enter_root succeeded → we are in VMX root.
                unsafe { vmx_exit_root(); }
                Err(e)
            }
        }
    }

    /// Raw pointer to this VM's shared state, for an AP vCPU to alias
    /// (`open_ap`). Only valid on the BSP's `Owned` context.
    pub fn shared_ptr(&mut self) -> *mut VmShared {
        &mut *self.shared as *mut VmShared
    }

    /// Leave VMX root and free the per-VM allocations so the VM can be
    /// relaunched in the same boot. Order matters: VMXOFF first (the
    /// VMCS must not be current/in-use when its frame is freed), then
    /// reclaim frames. The profile image was already persisted inside
    /// run_slice on the guest-exit path.
    ///
    /// BSP-only (`Owned`): frees the shared guest RAM + EPT + device-backed
    /// frames. An AP must use `close_ap` (its own VMX root only — the shared
    /// state belongs to the BSP, freed last-one-out).
    ///
    /// TODO(12.x): the EPT page-table frames from `ept::install_window`
    /// still leak (~tens of KB per run — negligible vs. the GB guest RAM
    /// this now reclaims). Tracked; needs an EPT teardown walker.
    pub fn close(&mut self) {
        // Persist the home image BEFORE teardown — see the svm mirror:
        // the Mod+Q window-close path reaches close() without run_slice's
        // loop-end save(), so without this the profile is lost on close.
        self.shared.pci.virtio_blk.save();
        // SAFETY: this vCPU entered VMX root on this core → VMXOFF is valid.
        unsafe { vmx_exit_root(); }
        // Demand-faulted frames + demand PTs + EPT tables.
        ept::release(self.shared.ept_pml4, self.shared.guest_mem.len());
        // Contiguous boot window.
        memory::deallocate_contiguous(
            self.shared.guest_raw_base,
            ept::boot_frames_for(self.shared.guest_mem.len()),
        );
        memory::deallocate_frame(self.vcpu.vmcs_phys);
        memory::deallocate_frame(self.vcpu.vmxon_phys);
    }

    /// Tear down ONLY this AP vCPU's VMX root (VMXOFF on its core + free its
    /// VMXON/VMCS frames). Does NOT touch the shared state — the BSP owns +
    /// frees that once the last vCPU has exited. VMX-specific: unlike SVM
    /// (where the AP just stops VMRUNning), the AP entered VMX root via
    /// `vmx_enter_root`, so it must VMXOFF on its own core.
    pub fn close_ap(&mut self) {
        // SAFETY: this AP entered VMX root on this core → VMXOFF is valid.
        unsafe { vmx_exit_root(); }
        memory::deallocate_frame(self.vcpu.vmcs_phys);
        memory::deallocate_frame(self.vcpu.vmxon_phys);
    }
}

pub fn run_linux(
    bzimage: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<vmcs::LaunchOutcome, &'static str> {
    let mut ctx = VmContext::open(bzimage, cmdline, initramfs, inject)?;
    // Step 1a: single unbounded slice — identical to the old
    // run_linux_loop. Step 1b bounds the budget and interleaves with
    // Shade on the Core-0 event loop.
    let result = loop {
        match ctx.run_slice(u32::MAX) {
            Ok(SliceOutcome::Exited(o)) => break Ok(o),
            Ok(SliceOutcome::StillRunning) | Ok(SliceOutcome::Idle) => continue,
            Err(e) => break Err(e),
        }
    };
    ctx.close();
    result
}

/// Per-guest serial UART state across exits.
struct SerialState {
    /// LCR.DLAB bit. When set, OUT to 0x3F8 / 0x3F9 means
    /// divisor-latch low/high (we ignore). When clear, 0x3F8 is
    /// THR (the byte the kernel wants to print).
    dlab: bool,
    /// Buffered output line — flushed via kprintln on '\n' or
    /// when the buffer is full. Linux's printk emits one line at
    /// a time so this rarely fills.
    line: [u8; 256],
    line_n: usize,
    /// Set on first observed `Kernel panic - not syncing:` line.
    /// Used by the loop's exit summary so the post-panic triple-fault
    /// is reported as the expected reboot path rather than an
    /// "unhandled exit reason 2".
    panic_observed: bool,
    /// Captured trailing text of the panic line (after the
    /// `Kernel panic - not syncing: ` prefix), e.g. the VFS
    /// `Unable to mount root fs` reason.
    panic_msg: [u8; 192],
    panic_msg_n: usize,
    /// Set on the first `reboot: System halted` / `Power down` line — the
    /// guest shut itself down (LibreWolf X → cage exit → PID-1 halt). Drives
    /// the auto-close-and-save so the user doesn't need a second Mod+Q.
    halt_observed: bool,
    /// Phase 12.1.4 — RX FIFO. Bytes pre-injected by the host before
    /// VMLAUNCH; drained one at a time when the guest reads RBR
    /// (0x3F8 IN with DLAB=0). LSR.DR (bit 0) on 0x3FD IN reflects
    /// `rx_pos < rx_n`. The guest-side counterpart in microvm-init
    /// busy-polls 0x3FD via iopl(3) + inb.
    rx: [u8; 128],
    rx_pos: usize,
    rx_n: usize,
}

const PANIC_PREFIX: &[u8] = b"Kernel panic - not syncing: ";

impl SerialState {
    fn new() -> Self {
        Self {
            dlab: false,
            line: [0; 256],
            line_n: 0,
            panic_observed: false,
            panic_msg: [0; 192],
            panic_msg_n: 0,
            halt_observed: false,
            rx: [0; 128],
            rx_pos: 0,
            rx_n: 0,
        }
    }

    /// Pre-load the RX FIFO with bytes the host wants the guest to
    /// receive on its next 0x3F8 reads. Truncates silently to capacity.
    fn inject(&mut self, bytes: &[u8]) {
        let cap = self.rx.len();
        let n = bytes.len().min(cap);
        self.rx[..n].copy_from_slice(&bytes[..n]);
        self.rx_pos = 0;
        self.rx_n = n;
    }

    fn rx_has_data(&self) -> bool {
        self.rx_pos < self.rx_n
    }

    fn rx_take(&mut self) -> u8 {
        if self.rx_pos < self.rx_n {
            let b = self.rx[self.rx_pos];
            self.rx_pos += 1;
            b
        } else {
            0
        }
    }

    fn put_char(&mut self, byte: u8) {
        use crate::kprintln;
        if byte == b'\n' || self.line_n == self.line.len() {
            let n = self.line_n;
            self.scan_for_panic(n);
            self.scan_for_shutdown(n);
            let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
            kprintln!("[guest] {}", s);
            self.line_n = 0;
            return;
        }
        if byte != b'\r' {
            self.line[self.line_n] = byte;
            self.line_n += 1;
        }
    }

    fn flush(&mut self) {
        use crate::kprintln;
        if self.line_n > 0 {
            let n = self.line_n;
            self.scan_for_panic(n);
            self.scan_for_shutdown(n);
            let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
            kprintln!("[guest] {}", s);
            self.line_n = 0;
        }
    }

    /// Search the just-completed `self.line[..n]` for the kernel-
    /// panic marker. Linux's printk frame is `<level>timestamp> body`,
    /// so the marker can sit anywhere on the line — substring match.
    fn scan_for_panic(&mut self, n: usize) {
        if self.panic_observed { return; }
        let line = &self.line[..n];
        let prefix = PANIC_PREFIX;
        if line.len() < prefix.len() { return; }
        for start in 0..=(line.len() - prefix.len()) {
            if &line[start..start + prefix.len()] == prefix {
                self.panic_observed = true;
                let body_start = start + prefix.len();
                let body = &line[body_start..];
                let copy_n = body.len().min(self.panic_msg.len());
                self.panic_msg[..copy_n].copy_from_slice(&body[..copy_n]);
                self.panic_msg_n = copy_n;
                return;
            }
        }
    }

    fn panic_msg_str(&self) -> &str {
        core::str::from_utf8(&self.panic_msg[..self.panic_msg_n]).unwrap_or("?")
    }

    /// Detect a clean guest self-shutdown in the printk stream. Linux's
    /// reboot path emits `reboot: System halted` (PID-1 `halt`) or
    /// `reboot: Power down`. The marker is specific to kernel/reboot.c, so
    /// an app log line can't false-trigger it. On the first hit, ask the run
    /// loop to take the normal Mod+Q close path (break → save → window close).
    fn scan_for_shutdown(&mut self, n: usize) {
        if self.halt_observed { return; }
        // Require the `reboot: ` prefix (kernel/reboot.c only). PID-1 mirrors
        // cage/moz app logs to /dev/kmsg → serial, so a bare "Power down"
        // substring could false-trigger; the prefix can't appear in app text.
        let line = &self.line[..n];
        let hit = line_contains(line, b"reboot: System halted")
            || line_contains(line, b"reboot: Power down")
            || line_contains(line, b"reboot: Restarting");
        if hit {
            self.halt_observed = true;
            crate::kprintln!("[microvm] guest shut down — closing window + saving profile");
            crate::microvm::cpu::note_guest_shutdown();
        }
    }
}

/// True if `hay` contains `needle` as a contiguous byte substring.
fn line_contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() { return false; }
    (0..=(hay.len() - needle.len())).any(|i| &hay[i..i + needle.len()] == needle)
}

/// Per-port I/O exit counter. Linux's boot touches dozens of unique
/// ports (PCI config, PIC, PIT, RTC, serial, keyboard, etc.).
/// Counting them tells us what the guest actually did when no
/// `[guest]` lines appeared.
struct IoStats {
    counts: [(u16, u32); 64],
    n: usize,
    /// First N bytes written to UART THR (port 0x3F8 with DLAB=0).
    serial_bytes: [u8; 256],
    serial_n: usize,
}

impl IoStats {
    fn new() -> Self {
        Self {
            counts: [(0, 0); 64],
            n: 0,
            serial_bytes: [0; 256],
            serial_n: 0,
        }
    }
    fn record(&mut self, port: u16, _dir_in: bool) {
        for i in 0..self.n {
            if self.counts[i].0 == port {
                self.counts[i].1 += 1;
                return;
            }
        }
        if self.n < self.counts.len() {
            self.counts[self.n] = (port, 1);
            self.n += 1;
        }
    }
    fn record_serial_byte(&mut self, byte: u8) {
        if self.serial_n < self.serial_bytes.len() {
            self.serial_bytes[self.serial_n] = byte;
            self.serial_n += 1;
        }
    }
    fn dump(&self) {
        use crate::kprintln;
        kprintln!("[microvm] I/O port summary ({} unique):", self.n);
        for i in 0..self.n {
            kprintln!("[microvm]   port {:#06x}: {:>5} accesses", self.counts[i].0, self.counts[i].1);
        }
        if self.serial_n > 0 {
            kprintln!("[microvm] {} bytes written to 0x3F8 (DLAB=0):", self.serial_n);
            // Print as ASCII-safe + hex-on-non-printable
            let mut buf: [u8; 256] = [0; 256];
            for i in 0..self.serial_n {
                let b = self.serial_bytes[i];
                buf[i] = if b.is_ascii_graphic() || b == b' ' || b == b'\n' { b } else { b'.' };
            }
            let s = core::str::from_utf8(&buf[..self.serial_n]).unwrap_or("?");
            kprintln!("[microvm]   '{}'", s);
        } else {
            kprintln!("[microvm] zero bytes ever reached 0x3F8 (DLAB=0)");
        }
    }
}

/// Per-iteration exit trace recorded for post-mortem on unhandled
/// exits. Keeps the last 32 (reason, qual_low32) tuples so we can
/// see what Linux was doing in the run-up to a triple-fault.
struct ExitTrace {
    items: [(u16, u32); 32],
    n: usize,
}

impl ExitTrace {
    fn new() -> Self {
        Self { items: [(0, 0); 32], n: 0 }
    }
    fn record(&mut self, reason: u16, qual: u64) {
        let idx = self.n % 32;
        self.items[idx] = (reason, qual as u32);
        self.n += 1;
    }
    fn dump(&self) {
        use crate::kprintln;
        let count = self.n.min(32);
        let start = if self.n > 32 { self.n - 32 } else { 0 };
        kprintln!("[microvm-trace] last {} exits:", count);
        for i in 0..count {
            let (r, q) = self.items[(start + i) % 32];
            kprintln!("[microvm-trace]   #{}: reason {:>3} qual {:#010x}", start + i, r, q);
        }
    }
}

/// Walk guest's 4-level page tables for `virt`, print each level's
/// entry. EPT identity-shifts guest-phys X → host-phys host_base+X
/// within the 64 MB window, so we just offset.
fn dump_page_walk(mem: &GuestMem, cr3: u64, virt: u64) {
    use crate::kprintln;
    const WINDOW: u64 = 64 * 1024 * 1024;
    const PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    let l4 = ((virt >> 39) & 0x1FF) as usize;
    let l3 = ((virt >> 30) & 0x1FF) as usize;
    let l2 = ((virt >> 21) & 0x1FF) as usize;
    let l1 = ((virt >> 12) & 0x1FF) as usize;
    kprintln!("[microvm-walk] CR3 = {:#018x}, virt = {:#018x}", cr3, virt);
    kprintln!("[microvm-walk] indices: L4={} L3={} L2={} L1={}", l4, l3, l2, l1);

    let pml4_phys = cr3 & PHYS_MASK;
    if pml4_phys >= WINDOW {
        kprintln!("[microvm-walk] PML4 phys {:#x} outside 64 MB window", pml4_phys);
        return;
    }
    let pml4_e = mem.read_u64(pml4_phys + (l4 as u64) * 8).unwrap_or(0);
    kprintln!("[microvm-walk]   PML4[{}] = {:#018x}", l4, pml4_e);
    if pml4_e & 1 == 0 { kprintln!("[microvm-walk]     L4 not present"); return; }

    let pdpt_phys = pml4_e & PHYS_MASK;
    if pdpt_phys >= WINDOW {
        kprintln!("[microvm-walk]   PDPT phys {:#x} outside window", pdpt_phys);
        return;
    }
    let pdpt_e = mem.read_u64(pdpt_phys + (l3 as u64) * 8).unwrap_or(0);
    kprintln!("[microvm-walk]   PDPT[{}] = {:#018x}", l3, pdpt_e);
    if pdpt_e & 1 == 0 { kprintln!("[microvm-walk]     L3 not present"); return; }
    if pdpt_e & (1 << 7) != 0 { kprintln!("[microvm-walk]     1 GB leaf"); return; }

    let pd_phys = pdpt_e & PHYS_MASK;
    if pd_phys >= WINDOW {
        kprintln!("[microvm-walk]   PD phys {:#x} outside window", pd_phys);
        return;
    }
    let pd_e = mem.read_u64(pd_phys + (l2 as u64) * 8).unwrap_or(0);
    kprintln!("[microvm-walk]   PD[{}] = {:#018x}", l2, pd_e);
    if pd_e & 1 == 0 { kprintln!("[microvm-walk]     L2 not present"); return; }
    if pd_e & (1 << 7) != 0 { kprintln!("[microvm-walk]     2 MB leaf"); return; }

    let pt_phys = pd_e & PHYS_MASK;
    if pt_phys >= WINDOW {
        kprintln!("[microvm-walk]   PT phys {:#x} outside window", pt_phys);
        return;
    }
    let pt_e = mem.read_u64(pt_phys + (l1 as u64) * 8).unwrap_or(0);
    kprintln!("[microvm-walk]   PT[{}] = {:#018x}", l1, pt_e);
    if pt_e & 1 == 0 { kprintln!("[microvm-walk]     L1 not present"); }
}

impl VmContext {
    /// Run the guest for up to `budget` VM-exits, or until it exits.
    /// `Ok(StillRunning)` = budget hit, re-enterable (step 1b);
    /// `Ok(Exited(o))` = guest left; `Err` = setup/VM fault. Body is
    /// the old `run_linux_loop` verbatim, `self.`-scoped.
    pub fn run_slice(&mut self, budget: u32) -> Result<SliceOutcome, &'static str> {
    use crate::kprintln;

    const MAX_ITERATIONS: u32 = 100_000;

    let mut last_outcome: Option<vmcs::LaunchOutcome> = None;
    let mut slice_n: u32 = 0;
    const MSR_LOG_CAP: u32 = 32;

    // Idle-detection. Once init enters its pause(2)/wait-loop, the
    // only exits we see are reason 1 (external interrupt, mostly
    // host timer ticks) at ~100 Hz. After IDLE_THRESHOLD consecutive
    // reason-1 exits with no other activity, declare the guest
    // idle-in-userspace and bail. Without this the shell is blocked
    // in run_linux_loop until MAX_ITERATIONS cap (~17 min at 100 Hz),
    // perceived as a host freeze. Real cancellation comes with
    // 12.1.4 inject_console.
    const IDLE_THRESHOLD: u32 = 200;
    // Yield Core 0 after this few consecutive idle (timer-tick) exits.
    // A parked/HLTed guest makes run_guest_once block until the next
    // timer tick, so spinning the full SLICE_BUDGET against it would
    // freeze Core 0 for tens of seconds (observed: VM window
    // unclosable, no shell, no keybinds, until it self-cleared).
    // Busy guests keep consecutive_idle at 0 → full budget, fast boot.
    const IDLE_YIELD: u32 = 4;

    // No lifetime cap for a window-bound app VM — see the SVM
    // mirror. MAX_ITERATIONS is cumulative across slices; a Wayland
    // compositor blows past it in seconds and a perfectly-running
    // guest gets torn down. Per-slice `slice_n >= budget` still
    // bounds each call cooperatively. saturating_add: no overflow on
    // a long-lived windowed VM.
    // Wall-clock slice cap. A busy guest (Wayland/LibreWolf
    // compositor) never reaches IDLE_YIELD — every non-timer exit
    // resets the idle counter — so the exit-count budget alone lets
    // one slice run for tens of ms, starving Shade (laggy UI,
    // sluggish Mod+Q: see intent::run_loop, the keybind read sits
    // after vm_poll_slice). Bound each slice to a few ms of wall
    // time so Core 0 returns to render Shade + read keybinds at a
    // steady cadence regardless of guest load. Boot (cheap exits)
    // still hits the exit budget first → same boot wall-time.
    const SLICE_MS: u64 = 3;
    let slice_deadline = crate::interrupts::rdtsc()
        + (crate::interrupts::tsc_freq() / 1000) * SLICE_MS;

    // Host-time profiler: attribute cycles to guest-run vs each exit
    // handler. `prof_post` = TSC right after the previous VMRESUME;
    // `prof_bucket` = that exit's bucket. The gap to the next VMRESUME is
    // the handler cost. 0 = no previous sample yet.
    let mut prof_post: u64 = 0;
    let mut prof_bucket: usize = crate::microvm::cpu::VMX_OTHER;
    while self.vcpu.iter < MAX_ITERATIONS || crate::microvm::vm_window() != 0 {
        if slice_n >= budget || crate::interrupts::rdtsc() >= slice_deadline {
            return Ok(SliceOutcome::StillRunning);
        }
        self.vcpu.iter = self.vcpu.iter.saturating_add(1);
        slice_n += 1;

        // Halt-poll adapt: if we were polling (deadline armed) and the guest is
        // no longer idle (consecutive_idle reset by a delivered wake), the poll
        // caught an event → grow the window (×2, capped) and disarm. A busy
        // vCPU keeps deadline 0, so this is a no-op for it.
        if self.vcpu.halt_poll_deadline != 0 && self.vcpu.consecutive_idle == 0 {
            self.vcpu.halt_poll_us = (self.vcpu.halt_poll_us * 2).min(HALT_POLL_MAX_US);
            self.vcpu.halt_poll_deadline = 0;
        }

        // Before each entry, sync the IA-32e-mode-guest control to
        // the current GUEST_IA32_EFER.LMA — once Linux flips into
        // long mode (after CR0.PG=1 with EFER.LME=1), the entry
        // control must match or VMX rejects the entry.
        if self.vcpu.launched {
            vmcs::sync_entry_ia32e_with_efer()?;
        }
        // Re-inject an event that was mid-delivery on the previous
        // exit (SDM §27.2.4; mirrors SVM `svm_complete_interrupts` and
        // our v0.172.36 fix). The single VM_ENTRY_INTR_INFO slot is
        // claimed by re-inject with priority: a lost in-flight
        // interrupt corrupts the guest far worse than a skipped fresh
        // tick — fresh injects (timer / virtio IRQ) regenerate on the
        // next EXTERNAL_INTERRUPT exit, but a mid-vectored event is
        // gone forever if dropped. Overwrites any fresh inject a
        // handler wrote on the previous iteration; that is intended.
        //
        // BUT an EXTERNAL interrupt (type 0) may only be injected when
        // the guest is interruptible (RFLAGS.IF=1, no STI/MOV-SS
        // shadow): SDM §26.3.1.5 makes that a guest-non-register-state
        // consistency check, so an IF=0 inject fails VM-entry with
        // reason 33 on bare-metal Intel (AMD's VMRUN tolerates it,
        // delivering a guest #GP — why QEMU/AMD never tripped). The
        // normal injects below were gated on `guest_interruptible()`
        // in v0.172.77; this re-inject slot was the one ungated write
        // left, and it tripped reason 33 on the i5-8265U when a virtio
        // IRQ (IRQ6 = 9p, vector 0x36) was re-injected while the guest
        // sat IF=0 mid-cage-startup. KVM gates re-injected IRQs the
        // same way — `__vmx_complete_interrupts` requeues the in-flight
        // vector into the pending-interrupt queue, and
        // kvm_check_and_inject_events re-checks `interrupt_allowed`,
        // HOLDING it pending (never dropping) until the window opens.
        // So we DEFER: keep `self.reinject` set when not interruptible;
        // the next ~100 Hz external-interrupt exit retries once the
        // guest runs `sti`. An NMI (type 2) is not IF-gated (delivered
        // regardless), so it injects immediately. Nothing is lost — an
        // IF=0 window is µs–ms, far under any RCU-stall threshold.
        if self.vcpu.reinject != 0 {
            let rtype = (self.vcpu.reinject >> 8) & 0x7;
            if rtype != 0 || vmcs::guest_interruptible() {
                let _ = vmcs::write_entry_intr_info(self.vcpu.reinject);
                self.vcpu.reinject = 0;
            }
        }
        // FPU host↔guest swap is now embedded inside run_guest_once's
        // asm (mirror of SVM v0.172.53), bracketing VMRESUME with zero
        // compiler-emittable code between xrstor and vmresume. A +avx2
        // kernel can spill `vmovups ymm` anywhere between a Rust
        // helper and the asm, clobbering the just-restored guest FPU
        // — putting xsave/xrstor in-asm closes that window. Pass the
        // host/guest FPU pointers in; the asm save/restore mask=-1
        // (current-XCR0 components; guest XCR0 ⊇ host's via XSETBV
        // pass-through).
        let hf: *mut crate::microvm::cpu::FpuArea = &mut *self.vcpu.host_fpu;
        let gf: *mut crate::microvm::cpu::FpuArea = &mut *self.vcpu.guest_fpu;
        // Profiler: charge the time since the last exit to that exit's
        // handler bucket, then time the VMRESUME itself as guest cycles.
        let prof_pre = crate::interrupts::rdtsc();
        if prof_post != 0 {
            crate::microvm::cpu::record_exit_cycles(prof_bucket, prof_pre.wrapping_sub(prof_post));
        }
        let result = vmcs::run_guest_once(&mut self.vcpu.regs, self.vcpu.launched, hf, gf);
        prof_post = crate::interrupts::rdtsc();
        crate::microvm::cpu::record_guest_cycles(prof_post.wrapping_sub(prof_pre));
        let outcome = result?;
        self.vcpu.launched = true;
        // DIAG (bare-metal reason-33): snapshot the injection field that
        // was live for the entry we just ran, BEFORE the clear below. On
        // a VM-entry failure (reason 33) the event is never delivered, so
        // this still holds what we wrote — confirms inject-vs-no-inject.
        let entry_intr_used = vmcs::read_entry_intr_info().unwrap_or(0xDEAD);
        // Consume the VM_ENTRY_INTR_INFO slot. On bare-metal VMX the
        // CPU auto-clears the valid bit after a successful injection
        // (SDM §27.6), but under nested VMX (KVM emulating VMX) it
        // MAY leave the bit set — the next VMRESUME would then
        // re-inject the same event = phantom duplicate interrupt →
        // cumulative guest corruption. KVM clears the field on every
        // exit for exactly this; do the same. The reinject/handler
        // sites re-arm the slot for the next VMRESUME as needed.
        // This is the VMX equivalent of SVM v0.172.42's EVENTINJ
        // clear.
        let _ = vmcs::write_entry_intr_info(0);
        // Did an event abort mid-vectoring through the guest IDT?
        // IDT_VECTORING_INFO has the same encoding as VM_ENTRY_INTR_
        // INFO; stash it verbatim for re-injection on the next entry.
        // Type-gate to external IRQ (0) and NMI (2) only — exceptions
        // (type 3, e.g. #PF / #GP) and software ints (type 4) MUST
        // NOT be re-injected: the faulting RIP was not advanced
        // (e.g. EPT-violation doesn't retire the access), so the
        // guest re-executes the instruction and the exception
        // re-occurs naturally; re-injecting too = double delivery.
        // Mirrors SVM v0.172.38's type-gate.
        let idtv = vmcs::read_idt_vectoring_info().unwrap_or(0);
        if idtv & (1u64 << 31) != 0 {
            let idtv_type = (idtv >> 8) & 0x7;
            if idtv_type == 0 || idtv_type == 2 {
                self.vcpu.reinject = idtv;
            }
        }
        let basic = vmcs::basic_exit_reason(outcome.exit_reason);
        self.vcpu.trace.record(basic, outcome.exit_qualification);

        // Exit-reason histogram (diagnosis — `cores` shows the mix).
        prof_bucket = match basic {
            1 => crate::microvm::cpu::VMX_INTR,
            12 => crate::microvm::cpu::VMX_HLT,
            48 => crate::microvm::cpu::VMX_MMIO, // EPT violation
            30 => crate::microvm::cpu::VMX_IO,
            31 | 32 => crate::microvm::cpu::VMX_MSR,
            10 => crate::microvm::cpu::VMX_CPUID,
            _ => crate::microvm::cpu::VMX_OTHER,
        };
        crate::microvm::cpu::record_vm_exit(prof_bucket);

        // DIAG (bare-metal reason-33): per-exit mode trace for the first
        // ~14 exits. Shows EXACTLY when the guest enters long mode (CS.L
        // / EFER.LMA flip 0→1) and whether re-entry into long mode
        // succeeds — so we can tell "first long-mode entry fails" from
        // "only the post-external-interrupt re-entry fails". Bounded to
        // early boot so it doesn't spam a running browser. VMX-only.
        if self.vcpu.iter <= 14 {
            let efer = vmcs::read_guest_efer().unwrap_or(0);
            let cs_ar = vmcs::read_guest_cs_ar().unwrap_or(0);
            kprintln!(
                "[vmx-iter{}] #{} reason {} CS.L={} LMA={} entry_intr={:#x}",
                self.vcpu.apic_id, self.vcpu.iter, basic,
                (cs_ar >> 13) & 1, (efer >> 10) & 1, entry_intr_used,
            );
        }

        // Reset idle-counter on any non-timer-tick activity. The
        // counter only progresses on a clean run of pure reason-1
        // exits (= guest sitting in pause(2)) or HLT (= guest halted
        // waiting for its next IRQ — shares the keepalive path).
        if basic != 1 && basic != 12 {
            self.vcpu.consecutive_idle = 0;
        }

        // Take the big-VM lock around this exit's device/memory handling when
        // an AP shares the VM (guest SMP) so the two vCPUs serialise access to
        // `VmShared`. Both `_big` and `sh` drop at the loop-body end — `sh`'s
        // `&mut VmShared` borrow ends before the lock releases (reverse decl
        // order), so only the lock holder ever has a live `&mut` to the shared
        // state (sound aliasing across the BSP's Owned box + the AP's Borrowed
        // pointer). No AP active → lock not taken → byte-identical single-vCPU.
        let _big = if ap_active() { Some(VM_BIG_LOCK.lock()) } else { None };
        // Resolve the shared device/memory state once for this exit (a single
        // `SharedRef::DerefMut` borrow of `self.shared`, so the handlers below
        // keep their disjoint sub-field borrows; `self.vcpu` stays separately
        // borrowable). Re-bound per iteration; NEVER held across run_guest_once
        // / a yield. Device/timer/nat/input handling is BSP-only.
        let sh = &mut *self.shared;
        let is_bsp = self.vcpu.apic_id == 0;

        // Pace audio on EVERY exit, not just the 1|12 inject path. The snd pump
        // feeds the mailbox + advances the guest's hw_ptr (wall-clock-paced
        // completion). Running it only on external-interrupt/HLT exits starved it
        // under load: a busy guest exits via MMIO (reason 48) thousands of
        // times/sec but rarely 1/12, so completion fell behind the 100 Hz budget
        // → audio played progressively slow + stuttered. On every exit the pump
        // rate scales with load and bytes_completed tracks the budget. Completion
        // latches the snd IRQ into pending_irqs; the 1|12 drain injects it at a
        // safe point (one-inject-per-entry + interruptibility gate stay intact).
        if is_bsp && sh.pci.virtio_snd.pump(&sh.guest_mem) {
            sh.pending_irqs |= 1 << sh.pci.virtio_snd.irq_line();
        }

        match basic {
            0 => {
                // Exception/NMI. EXCEPTION_BITMAP=0 in production —
                // exceptions go to Linux's IDT directly. This arm
                // only fires for NMIs (which we don't generate
                // intentionally) or if Linux somehow re-enables
                // exception trapping. Kept as a safety net + the
                // dump remains useful if it ever fires.
                sh.serial.flush();
                let info = vmcs::read_exit_intr_info().unwrap_or(0);
                let vector = info & 0xFF;
                let intr_type = (info >> 8) & 0x7;
                let err_valid = (info >> 11) & 0x1 != 0;
                let err_code = if err_valid {
                    vmcs::read_exit_intr_error_code().unwrap_or(0)
                } else {
                    0
                };
                let mnemonic = match vector {
                    0 => "DE", 1 => "DB", 2 => "NMI", 3 => "BP",
                    4 => "OF", 5 => "BR", 6 => "UD", 7 => "NM",
                    8 => "DF", 10 => "TS", 11 => "NP", 12 => "SS",
                    13 => "GP", 14 => "PF", 16 => "MF", 17 => "AC",
                    18 => "MC", 19 => "XM", 20 => "VE", 21 => "CP",
                    _ => "??",
                };
                kprintln!(
                    "[microvm] guest exception #{} ({}) type={} qual={:#x} err_valid={} err_code={:#x}",
                    vector, mnemonic, intr_type, outcome.exit_qualification,
                    err_valid, err_code,
                );
                let rip   = vmcs::read_guest_rip().unwrap_or(0);
                let cr0   = vmcs::read_guest_cr0().unwrap_or(0);
                let cr4   = vmcs::read_guest_cr4().unwrap_or(0);
                let efer  = vmcs::read_guest_efer().unwrap_or(0);
                let cs    = vmcs::read_guest_cs_selector().unwrap_or(0);
                let entry = vmcs::read_vm_entry_controls().unwrap_or(0);
                kprintln!(
                    "[microvm]   GUEST_RIP  = {:#018x}  GUEST_CS = {:#06x}",
                    rip, cs,
                );
                kprintln!(
                    "[microvm]   GUEST_CR0  = {:#018x}  GUEST_CR4 = {:#018x}",
                    cr0, cr4,
                );
                kprintln!(
                    "[microvm]   GUEST_EFER = {:#018x}  ENTRY_CTLS = {:#010x}",
                    efer, entry,
                );
                if vector == 14 {
                    let cr3 = vmcs::read_guest_cr3().unwrap_or(0);
                    dump_page_walk(&sh.guest_mem, cr3, outcome.exit_qualification);
                }
                self.vcpu.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            1 | 12 => {
                // reason 12 = HLT: guest executed STI;HLT (idle, waiting
                // for its next IRQ). A headless test guest means "done" →
                // exit; a window-bound app idling is normal → fall through
                // to the same inject-due-IRQ + idle-accounting path as a
                // host-timer external interrupt (the injected IRQ is
                // delivered at the HLT, so the guest resumes past it). The
                // dedicated core host-idles between ticks via Idle.
                if basic == 12 && crate::microvm::vm_window() == 0 {
                    sh.serial.flush();
                    kprintln!("[vmx] guest HLT after {} VM-exits — exiting", self.vcpu.iter);
                    self.vcpu.io_stats.dump();
                    last_outcome = Some(outcome);
                    break;
                }
                // HLT is an intercepted instruction: advance RIP past it
                // (like KVM's skip_emulated_instruction) so that once an
                // injected IRQ's ISR returns, the guest continues into its
                // need_resched check and schedules — not back onto the HLT.
                if basic == 12 {
                    vmcs::advance_guest_rip()?;
                    // The HLT was the single instruction the preceding STI's
                    // interrupt-shadow covered — executing it consumes the
                    // shadow. Clear blocking-by-STI/MOV-SS so an idle `sti;hlt`
                    // (IF=1) reads as interruptible: the gate below then injects
                    // its wakeup IRQ + parks the core, instead of re-entering on
                    // a still-shadowed HLT forever (3.3M hlt/s, cores at 100%).
                    // A real `cli;hlt` keeps IF=0 → still gated (correct).
                    let _ = vmcs::consume_sti_shadow();
                }
                // SMP interruptibility gate: if the guest can't take an
                // interrupt right now (IF=0 / in an STI/MOV-SS shadow — e.g.
                // mid `spin_lock_irqsave`), do NOT inject anything. Forcing an
                // IRQ into a critical section deadlocks an SMP guest's
                // qspinlock (the 9p-mount livelock), AND on bare-metal Intel
                // VMX an external-interrupt inject while non-interruptible
                // FAILS VM-entry with reason 33 (SDM §26.3.1.5) — strict where
                // AMD's EVENTINJ tolerates it. Unlike SVM we need NO idle
                // `sti;hlt` special case: VMX clears blocking-by-STI in the
                // saved interruptibility state on a HLT VM-exit, so an idle
                // HLT is already interruptible here — it falls through and the
                // pending timer/IPI wakes it (proven by the single-vCPU idle
                // path). Re-enter; busy, not idle, so skip idle accounting.
                if !vmcs::guest_interruptible() {
                    last_outcome = Some(outcome);
                    continue;
                }
                // Past the gate → injectable (interruptible, or an idle
                // sti;hlt we wake). Cross-vCPU IPI first (guest SMP): a
                // reschedule / call-function / TLB / AP→BSP `complete()`
                // wakeup another vCPU routed here. Highest priority — carries
                // the SMP bring-up handshake. No-op on a UP guest.
                if let Some(vec) = sh.ipi_take(self.vcpu.apic_id) {
                    let _ = vmcs::inject_external_irq(vec);
                    self.vcpu.consecutive_idle = 0;
                    continue;
                }
                // External interrupt — host IRQ that arrived during
                // guest run. The `sti` at the tail of run_guest_once
                // already let the host IDT dispatch it; just resume.
                //
                // Guest timer tick first: the microvm has no PIT/
                // LAPIC timer source, so this injected IRQ0 is the
                // only thing that wakes a time-blocked guest task
                // (nanosleep/timerfd/poll-timeout — all of seatd/
                // cage/wlroots/libwayland). Pace to the host 100 Hz
                // tick; gate on Linux having unmasked IRQ0. One
                // VM-entry injects one IRQ → claim it and `continue`
                // (reason-1 recurs immediately, net/input lose
                // nothing). Mirrors the SVM side.
                // D4 live-resize config-change MUST be checked before
                // the timer-tick block. An idle guest (LibreWolf on
                // about:blank) HLTs between ticks, so its only reason-1
                // exits are fresh 100 Hz host-timer ticks (host USB is
                // drained ON that timer, not as extra IRQs) — the
                // timer block then `continue`s on EVERY exit and
                // nothing past it (net/input/config-change) ever runs.
                // This is rare (only on a Shade tile resize) and
                // one-shot; firing it here costs the timer at most one
                // tick (Linux tolerates the jitter) and is the only
                // place an idle guest will ever see it.
                //
                // GATE on irq_unmasked(9): never inject the virtio-gpu
                // config-change IRQ before the guest has unmasked line 9.
                // Early boot (Shade marks the fresh tile display-dirty on
                // its first layout) would otherwise inject vector 0x29 into
                // a guest whose initial IDT has only 32 gates (IDTR
                // limit=0x1ff) and whose PIC isn't initialised yet — the
                // injected vector exceeds the IDT limit, delivery faults
                // during VM-entry, and bare-metal Intel reports it as a
                // VM-entry failure (reason 33). AMD's VMRUN tolerates it
                // (delivers a guest #GP instead), which is why QEMU/AMD
                // never tripped on this. Mirrors the timer's irq_unmasked(0)
                // gate. The dirty flag is left set, so the resize lands the
                // moment the guest is ready to receive it.
                // KVM-style interruptibility gate (vmx_interrupt_allowed):
                // only inject an external interrupt when the guest can
                // actually take one (RFLAGS.IF=1, no STI / MOV-SS shadow).
                // Injecting into an IF=0 guest (e.g. mid i8042 status poll
                // in a CLI'd critical section) makes the VM-entry fail with
                // reason 33 on bare-metal Intel; AMD's VMRUN tolerates it.
                // The skipped IRQ re-fires on the next ~100 Hz external-
                // interrupt exit once the guest sets IF=1, so an IF=0 window
                // (µs–ms) costs at most a tick of jitter — far under any
                // RCU-stall threshold.
                // Device IRQs, the PIT tick, display-resize, NAT pump and
                // input are BSP-only (PIC mode delivers device lines to the
                // boot CPU); an AP vCPU only services its own LAPIC timer
                // (below) + cross-vCPU IPIs (above). Guest SMP.
                // Deferred device IRQ (a virtio queue-kick the guest issued
                // while IF=0, latched by `deliver_irq_vmx`) is now deliverable
                // — inject the lowest pending line. Mirrors the SVM drain.
                if is_bsp && sh.pending_irqs != 0 {
                    let line = sh.pending_irqs.trailing_zeros() as u8;
                    sh.pending_irqs &= !(1u16 << line);
                    let vector = sh.pic.vector_for_irq(line);
                    let _ = vmcs::inject_external_irq(vector);
                    self.vcpu.consecutive_idle = 0;
                    continue;
                }
                if is_bsp && sh.pic.irq_unmasked(9) {
                    let wid = crate::microvm::vm_window();
                    let now_d4 = crate::interrupts::ticks();
                    // Phase 2 of any in-flight D4 cycle: reconnect IRQ.
                    if sh.pci.virtio_gpu.tick_d4(now_d4) {
                        let vector = sh.pic.vector_for_irq(9);
                        let _ = vmcs::inject_external_irq(vector);
                        self.vcpu.consecutive_idle = 0;
                        let _ = wid;
                        continue;
                    }
                    // Phase 1: fresh dirty, no cycle in flight, debounce
                    // ok → disconnect IRQ; tick_d4 fires reconnect 100 ms
                    // later. R2 debounce ensures ≤4 cycles/sec during a
                    // drag — only the final size sticks.
                    if wid != 0
                        && !sh.pci.virtio_gpu.d4_disconnecting()
                        && crate::shade::surface::display_dirty_peek(wid)
                    {
                        if now_d4.wrapping_sub(sh.last_cfg_tick) >= 25 {
                            let _ = crate::shade::surface::take_display_dirty(wid);
                            sh.last_cfg_tick = now_d4;
                            sh.pci.virtio_gpu.signal_display_change(now_d4);
                            let vector = sh.pic.vector_for_irq(9);
                            let _ = vmcs::inject_external_irq(vector);
                            self.vcpu.consecutive_idle = 0;
                            continue;
                        }
                    }
                }
                // Single inject slot per VM-entry → a strict priority
                // order ALWAYS starves the loser: timer-first starves
                // input/net for an idle guest (every exit is a fresh
                // timer tick → continue); input/net-first starves the
                // TIMER for a busy guest (a page load makes nat::pump
                // true on almost every exit → continue → no IRQ0 →
                // guest jiffies freeze → "rcu_preempt kthread timer
                // wakeup didn't happen" RCU stall → guest hangs →
                // cage unscheduled → libwayland 4096 buffer overflow
                // → channel error → cage rc=139). Both observed.
                //
                // The timer is sacred (guest liveness/RCU/scheduler)
                // and must be NON-STARVABLE, but only needs ~100 Hz.
                // Fix: a bounded-skip floor — if the timer is overdue
                // by ≥ TIMER_MAX_SKIP host ticks it FORCES the slot
                // (caps timer jitter at ~30 ms even under saturating
                // net — far under any RCU-stall threshold). Otherwise
                // config-change > input > net, then the normal timer.
                // nat::pump still runs every entry for its drain side
                // effect; only the inject+`continue` is prioritized.
                const TIMER_MAX_SKIP: u64 = 3; // host ticks (~30 ms)
                let now = crate::interrupts::ticks();
                // Two timer sources, both paced at the host 100 Hz `ticks()`
                // so their rates match 1:1 — what Linux's APIC-timer
                // calibration verification needs (else "APIC timer disabled").
                // PIT IRQ0 until Linux disables the i8253 (pit_enabled → false
                // after a mode-0 write); the LAPIC LVTT vector once Linux
                // software-enables + unmasks it. One inject slot per VM-entry
                // → whichever isn't taken lands next entry (≥1 kHz). LAPIC
                // emulation (#2) only — None when booting `nolapic`.
                let pit_live = is_bsp && sh.pic.irq_unmasked(0) && sh.pit_enabled;
                let lapic_vec = if vmx_lapic_on() {
                    self.vcpu.lapic.timer_tick_vector()
                } else {
                    None
                };
                // NB: the periodic 100 Hz timer is NOT a wake — it must NOT
                // reset consecutive_idle. Resetting it here made the loop-top
                // halt-poll grower (consecutive_idle==0 + armed poll) treat every
                // tick as "the poll caught an event" → halt_poll_us doubled to MAX
                // every 10 ms → the core busy-polled instead of parking (idle=halt
                // exposed this: 3.3M hlt/s, cores pegged 100%, 0 host-HLTs). Only
                // real device wakes (net/input/audio/IPI, below) reset it.
                if pit_live && now.wrapping_sub(sh.last_timer_tick) >= TIMER_MAX_SKIP {
                    sh.last_timer_tick = now;
                    let vector = sh.pic.vector_for_irq(0);
                    let _ = vmcs::inject_external_irq(vector);
                    continue;
                }
                if let Some(vec) = lapic_vec {
                    if now.wrapping_sub(self.vcpu.last_lapic_tick) >= TIMER_MAX_SKIP {
                        self.vcpu.last_lapic_tick = now;
                        let _ = vmcs::inject_external_irq(vec);
                        continue;
                    }
                }
                // Real net data this pass (drives idle accounting below).
                // BSP-only: the AP never pumps the NAT (stays false).
                let mut pumped = false;
                if is_bsp {
                    pumped = crate::microvm::devices::nat::pump(
                        &mut sh.pci.virtio_net, &sh.guest_mem);
                    if sh.pci.virtio_input.drain_injected(&sh.guest_mem) {
                        let vector = sh.pic.vector_for_irq(12);
                        let _ = vmcs::inject_external_irq(vector);
                        self.vcpu.consecutive_idle = 0;
                        continue;
                    }
                    // (snd pump moved to the per-exit common path above; its IRQ
                    // is drained from pending_irqs at the top of this block.)
                    if pumped {
                        let vector = sh.pic.vector_for_irq(10);
                        let _ = vmcs::inject_external_irq(vector);
                        self.vcpu.consecutive_idle = 0;
                        continue;
                    }
                }
                // (periodic timer — does NOT reset consecutive_idle; see above.)
                if pit_live && now != sh.last_timer_tick {
                    sh.last_timer_tick = now;
                    let vector = sh.pic.vector_for_irq(0);
                    let _ = vmcs::inject_external_irq(vector);
                    continue;
                }
                if let Some(vec) = lapic_vec {
                    if now != self.vcpu.last_lapic_tick {
                        self.vcpu.last_lapic_tick = now;
                        let _ = vmcs::inject_external_irq(vec);
                        continue;
                    }
                }
                // Pure timer-tick → idle counter advances. Real net data
                // (`pumped`) resets it. Merely having open NAT sessions
                // does NOT for a windowed VM — a browser keeps connections
                // alive while idle, and counting that as "busy" pinned
                // consecutive_idle at 0 so it never yielded Idle → the
                // dedicated core spun at 100%. Net is still pumped every
                // iteration / host wake-up (≤1 ms latency). Headless test
                // guests keep the old behavior.
                if pumped
                    || (crate::microvm::devices::nat::active_session_count() > 0
                        && crate::microvm::vm_window() == 0)
                {
                    self.vcpu.consecutive_idle = 0;
                } else {
                    self.vcpu.consecutive_idle = self.vcpu.consecutive_idle.saturating_add(1);
                }
                // Idle-auto-exit only for an UNWINDOWED VM (legacy
                // blocking-smoke model: "guest done, stop wasting Core
                // 0"). A window-bound VM is an app — idle is normal
                // (a parked guest, an idle browser) and must NOT be
                // killed. It ends only on real guest exit (HLT /
                // shutdown) or window close (VM_CLOSE_REQUESTED, handled
                // in vm_poll_slice). Cheap to keep slicing: idle =
                // timer-tick exits, near-free.
                if self.vcpu.consecutive_idle >= IDLE_THRESHOLD
                    && crate::microvm::vm_window() == 0
                {
                    sh.serial.flush();
                    kprintln!(
                        "[microvm] guest idle in userspace ({} consecutive timer ticks after {} iters) — exiting cleanly",
                        self.vcpu.consecutive_idle, self.vcpu.iter,
                    );
                    last_outcome = Some(outcome);
                    break;
                }
                // Guest idle → yield as Idle (not Exited — guest stays
                // alive; not StillRunning — so the dedicated core host-
                // idles before re-entering instead of spinning VMRUN).
                // consecutive_idle persists in the VmContext across yields,
                // so the unwindowed idle-exit above still triggers once it
                // reaches IDLE_THRESHOLD over many yield cycles.
                // BUT do NOT deep-park while network data is in flight: parking
                // quantizes RX delivery to ~2 ms, which is invisible for a
                // single bulk stream (speedtest hit 42 MB/s) but kills a
                // page-load's MANY small request→response round-trips (each
                // gap = a 2 ms park → YouTube "ewig"). While recently active,
                // keep spin-pumping (the slice still yields the core every
                // SLICE_MS); park only once the link is quiet (power).
                // Bounded adaptive halt-polling (KVM halt_poll_ns model). Once
                // idle, poll the current per-vCPU window (re-pumping + checking
                // for a wake) before truly parking; on expiry shrink + park
                // (power). The grow-on-wake is handled at the loop top. Replaces
                // the old global `recently_active` spin: no core pegged at 100%
                // unless it's doing real work; idle vCPUs shrink toward the
                // floor and park; a latency-sensitive vCPU grows toward the cap.
                if self.vcpu.consecutive_idle >= IDLE_YIELD {
                    let now_tsc = crate::interrupts::rdtsc();
                    if self.vcpu.halt_poll_deadline == 0 {
                        self.vcpu.halt_poll_deadline = now_tsc
                            + (crate::interrupts::tsc_freq() / 1_000_000) * self.vcpu.halt_poll_us;
                    }
                    if now_tsc >= self.vcpu.halt_poll_deadline {
                        self.vcpu.halt_poll_us = (self.vcpu.halt_poll_us / 2).max(HALT_POLL_MIN_US);
                        self.vcpu.halt_poll_deadline = 0;
                        return Ok(SliceOutcome::Idle);
                    }
                    // else: within the poll window → keep looping (poll).
                }
                last_outcome = Some(outcome);
            }
            10 => {
                // CPUID — VMX always exits on CPUID. Pass through
                // to host; guest sees real CPU features. Linux uses
                // this for early feature detection. Filtered for
                // features we can't safely expose to the guest.
                let leaf = self.vcpu.regs.rax as u32;
                let subleaf = self.vcpu.regs.rcx as u32;
                let (mut eax, mut ebx, mut ecx, mut edx) =
                    vmcs::host_cpuid(leaf, subleaf);
                if leaf == 1 && vmx_lapic_on() {
                    // ECX bit 24: TSC-deadline timer. Hide it so Linux uses
                    // the classic APIC_TMICT/TMCCT periodic timer (which
                    // vmx::lapic emulates) instead of the TSC-deadline MSR
                    // (0x6E0), which we don't emulate.
                    ecx &= !(1u32 << 24);
                    // EBX bits 31:24: initial xAPIC ID. Pass-through reports
                    // the HOST worker core's id — mismatching our emulated
                    // LAPIC + MP-table. Report THIS vCPU's id (BSP=0, AP=1..).
                    ebx = (ebx & 0x00FF_FFFF) | ((self.vcpu.apic_id as u32) << 24);
                }
                // Leaf 0xB/0x1F (extended topology): EDX is the 32-bit x2APIC
                // ID modern Linux uses as the CPU's APIC ID. Same reason as
                // leaf 1 EBX — report this vCPU's id, not the host core's.
                if vmx_lapic_on() && (leaf == 0x0B || leaf == 0x1F) {
                    edx = self.vcpu.apic_id as u32;
                }
                if leaf == 7 && subleaf == 0 {
                    // Hide CET from the guest. Host nopeekOS has
                    // CR4.CET=1 for IBT, but Alpine vmlinuz has
                    // hand-written asm stubs without ENDBR64 — once
                    // CR4.CET is on, indirect calls to those stubs
                    // raise #CP and Linux BUG()s. Clearing both bits
                    // here + masking CR4.CET in initial GUEST_CR4
                    // (vmcs.rs) keeps CET fully off in the guest.
                    ecx &= !(1u32 << 7);   // CET_SS  (Shadow Stack)
                    edx &= !(1u32 << 20);  // CET_IBT (Indirect Branch Tracking)
                    // Hide PKU (Memory Protection Keys for Userspace).
                    // Host CPU reports PKRU as part of XSAVE state in
                    // CPUID 0xD (size 840 incl. 8 byte PKRU). Linux's
                    // own xstate calculation comes to 832 (no PKRU
                    // support gated by this PKU bit) → consistency-
                    // check WARN, XSAVE disabled, fpstate_reset NULL-
                    // deref panic. Easiest fix: hide PKU from the
                    // guest entirely. Our microvm doesn't need PK.
                    ecx &= !(1u32 << 3);   // PKU
                    ecx &= !(1u32 << 4);   // OSPKE (driven by PKU)
                    // Hide MPX — its XSAVE BND state is dropped in leaf
                    // 0xD below, so the feature bit must go too or the
                    // guest's xstate enumeration mismatches.
                    ebx &= !(1u32 << 14);  // MPX
                }
                if leaf == 0xD {
                    // Clamp the guest's XSAVE state to x87 + SSE + AVX only.
                    // The host CPU may expose extra components (MPX BNDREGS/
                    // BNDCSR on Intel 8th-gen Whiskey Lake, PKRU, AVX-512…);
                    // passing leaf 0xD through verbatim makes Linux's XSAVE
                    // size-consistency check fail (computed size != kernel
                    // size → "XSAVE disabled" → fpstate_reset NULL-deref
                    // panic). The N100 (Gracemont) lacks MPX, which is why
                    // this only bit the notebook. 832 = 512 legacy + 64
                    // header + 256 YMM_Hi128 is architectural for x87+SSE+AVX.
                    const XSAVE_BASE_AVX: u32 = 0x340; // 832
                    match subleaf {
                        0 => { eax &= 0x7; ebx = XSAVE_BASE_AVX; ecx = XSAVE_BASE_AVX; edx = 0; }
                        1 => { eax &= 0x7; ebx = XSAVE_BASE_AVX; ecx = 0; edx = 0; } // drop XSAVES/supervisor
                        2 => { ecx = 0; edx = 0; } // AVX component: keep host size(eax)+offset(ebx)
                        _ => { eax = 0; ebx = 0; ecx = 0; edx = 0; } // hide MPX/PKRU/AVX-512/…
                    }
                }
                self.vcpu.regs.rax = eax as u64;
                self.vcpu.regs.rbx = ebx as u64;
                self.vcpu.regs.rcx = ecx as u64;
                self.vcpu.regs.rdx = edx as u64;
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            28 => {
                // Control-register access. Most commonly Linux's
                // startup_32 doing MOV CR3, reg to load its own
                // page tables — IA32_VMX_PROCBASED_CTLS may force
                // CR3-load/store-exiting on this CPU even with EPT.
                let qual = outcome.exit_qualification;
                let cr_num = (qual & 0xF) as u8;
                let access_type = ((qual >> 4) & 0x3) as u8;
                let gp_reg = ((qual >> 8) & 0xF) as u8;

                if cr_num != 3 {
                    sh.serial.flush();
                    kprintln!(
                        "[microvm] unhandled CR{} access (type {}, reg {}, qual {:#x})",
                        cr_num, access_type, gp_reg, qual,
                    );
                    last_outcome = Some(outcome);
                    break;
                }
                match access_type {
                    0 => {
                        // MOV to CR3 (set page-table base).
                        let val = read_gpr(&self.vcpu.regs, gp_reg)?;
                        vmcs::write_guest_cr3(val)?;
                    }
                    1 => {
                        // MOV from CR3.
                        let val = vmcs::read_guest_cr3()?;
                        write_gpr(&mut self.vcpu.regs, gp_reg, val)?;
                    }
                    _ => {
                        sh.serial.flush();
                        kprintln!(
                            "[microvm] CR3 unusual access type {} (qual {:#x})",
                            access_type, qual,
                        );
                        last_outcome = Some(outcome);
                        break;
                    }
                }
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            30 => {
                let (port, dir_in, size) =
                    vmcs::decode_io_exit_qualification(outcome.exit_qualification);
                self.vcpu.io_stats.record(port, dir_in);
                if port == 0x3F8 && !dir_in && !sh.serial.dlab && size == 1 {
                    self.vcpu.io_stats.record_serial_byte((self.vcpu.regs.rax & 0xFF) as u8);
                }
                handle_linux_io(&mut sh.serial, &mut sh.pci, &mut sh.pic, &mut self.vcpu.regs, port, dir_in, size, &mut self.vcpu.io_dropped, &mut sh.pit_enabled);
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            55 => {
                // XSETBV — VMX always exits on this. Linux uses it
                // during FPU/AVX init to set XCR0. Host's XCR0
                // already has x87/SSE/AVX enabled (set in boot.s),
                // so Linux's intended write is effectively a no-op
                // for the bits it cares about. Just advance RIP.
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            31 => {
                // RDMSR — exits when ECX is outside MSR-bitmap ranges
                // (0-0x1FFF and 0xC0000000-0xC0001FFF) or when the
                // bitmap bit is set. Our bitmap is zero, so this is
                // an out-of-range MSR. Synthesize a zero return — the
                // safest answer for unknown info MSRs (Linux's
                // safe_rdmsr-style code copes with bogus values).
                let msr = self.vcpu.regs.rcx as u32;
                // IA32_APIC_BASE (0x1B): report the LAPIC enabled at its
                // architectural default base (Intel parity #2). Intercepted
                // via the MSR bitmap only when LAPIC is on. The BSP bit (8) is
                // reported ONLY for the boot vCPU (apic_id 0); an AP must read
                // it clear or Linux's topology mistakes it for a 2nd BSP.
                if msr == 0x1B && vmx_lapic_on() {
                    let mut v = lapic::APIC_BASE_MSR_VALUE;
                    if self.vcpu.apic_id != 0 {
                        v &= !(1u64 << 8);
                    }
                    self.vcpu.regs.rax = v & 0xFFFF_FFFF;
                    self.vcpu.regs.rdx = v >> 32;
                    vmcs::advance_guest_rip()?;
                    last_outcome = Some(outcome);
                    continue;
                }
                if !msr_is_known_noise(msr) && self.vcpu.msr_log_count < MSR_LOG_CAP {
                    kprintln!("[microvm] RDMSR {:#010x} → 0 (unhandled)", msr);
                    self.vcpu.msr_log_count += 1;
                }
                self.vcpu.regs.rax = 0;
                self.vcpu.regs.rdx = 0;
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            32 => {
                // WRMSR — same gating as RDMSR. Silently absorb
                // (don't propagate to host — writing arbitrary MSRs
                // would break the host's pstate/PMU/etc).
                let msr = self.vcpu.regs.rcx as u32;
                if self.vcpu.msr_log_count < MSR_LOG_CAP {
                    let val = (self.vcpu.regs.rdx << 32) | (self.vcpu.regs.rax & 0xFFFF_FFFF);
                    kprintln!("[microvm] WRMSR {:#010x} = {:#018x} (absorbed)", msr, val);
                    self.vcpu.msr_log_count += 1;
                }
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            48 => {
                // EPT violation — guest tried to access a guest-phys
                // address outside our 64 MB window (or with insufficient
                // EPT permissions). For accesses landing in virtio-blk's
                // BAR0 range we emulate; everything else dumps + bails.
                let gpa = vmcs::read_guest_phys_addr().unwrap_or(0);
                // LAPIC MMIO page (0xFEE00000) → trap-and-emulate (Intel
                // parity #2). Left EPT-not-present in ept.rs when LAPIC is on.
                if vmx_lapic_on()
                    && (lapic::LAPIC_BASE..lapic::LAPIC_BASE + lapic::LAPIC_SIZE).contains(&gpa)
                {
                    if handle_mmio_lapic(&mut self.vcpu.regs, &mut self.vcpu.lapic, self.vcpu.apic_id, sh, gpa) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                }
                if sh.pci.virtio_blk.bar0_in_range(gpa) {
                    if handle_mmio_ept_blk(&mut self.vcpu.regs, &mut sh.pci.virtio_blk, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_net.bar0_in_range(gpa) {
                    if handle_mmio_ept_net(&mut self.vcpu.regs, &mut sh.pci.virtio_net, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        // The guest just touched virtio-net (queue notify / ISR
                        // read) — drain RX into it NOW instead of waiting for the
                        // ~1500/s timer/HLT pump. This tracks RX delivery to the
                        // guest's device activity (~15k/s), lifting the download
                        // ceiling (pump/s was ≪ VM-exits/s). BSP only.
                        if is_bsp
                            && crate::microvm::devices::nat::pump_fast(
                                &mut sh.pci.virtio_net, &sh.guest_mem)
                        {
                            sh.pending_irqs |= 1 << 10;
                        }
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_gpu.bar0_in_range(gpa) {
                    if handle_mmio_ept_gpu(&mut self.vcpu.regs, &mut sh.pci.virtio_gpu, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_input.bar0_in_range(gpa) {
                    if handle_mmio_ept_input(&mut self.vcpu.regs, &mut sh.pci.virtio_input, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_9p.bar0_in_range(gpa) {
                    if handle_mmio_ept_p9(&mut self.vcpu.regs, &mut sh.pci.virtio_9p, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_blk_sqfs.bar0_in_range(gpa) {
                    // Same handler — VirtioBlk carries its own IRQ line.
                    if handle_mmio_ept_blk(&mut self.vcpu.regs, &mut sh.pci.virtio_blk_sqfs, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_snd.bar0_in_range(gpa) {
                    if handle_mmio_ept_snd(&mut self.vcpu.regs, &mut sh.pci.virtio_snd, &sh.pic, &mut sh.pending_irqs, gpa, &sh.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                }
                // B3: demand-paged guest RAM. A violation on a gpa
                // inside the advertised window but above the
                // contiguous boot block = first touch of a 4-KB
                // demand page → fault it in + re-enter. Ordering is
                // load-bearing: MMIO BAR ranges first (above),
                // RAM-demand here, fatal dump last.
                if sh.guest_mem.ensure(gpa) {
                    last_outcome = Some(outcome);
                    continue;
                }
                sh.serial.flush();
                let gla  = vmcs::read_guest_linear_addr().unwrap_or(0);
                let q    = outcome.exit_qualification;
                let read = q & 1 != 0;
                let write = q & 2 != 0;
                let fetch = q & 4 != 0;
                kprintln!(
                    "[microvm] EPT violation: gpa={:#018x} gla={:#018x} qual={:#x}",
                    gpa, gla, q,
                );
                kprintln!(
                    "[microvm]   access: {}{}{}",
                    if read { "R" } else { "" },
                    if write { "W" } else { "" },
                    if fetch { "X" } else { "" },
                );
                self.vcpu.io_stats.dump();
                self.vcpu.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            2 => {
                // Triple fault. Linux uses this as `emergency_restart`
                // when ACPI/PIIX/EFI reset paths are unavailable —
                // i.e. the standard exit path on `panic=1` here.
                sh.serial.flush();
                if sh.serial.panic_observed {
                    kprintln!(
                        "[microvm] linux kernel panicked (after {} iters): {}",
                        self.vcpu.iter, sh.serial.panic_msg_str(),
                    );
                    kprintln!("[microvm] guest then triple-faulted via emergency_restart (= expected reboot path)");
                } else {
                    kprintln!(
                        "[microvm] guest triple-faulted after {} iters (no kernel-panic seen on console)",
                        self.vcpu.iter,
                    );
                }
                self.vcpu.io_stats.dump();
                self.vcpu.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            _ => {
                sh.serial.flush();
                kprintln!(
                    "[microvm] unhandled exit reason {} qual {:#x} after {} iters",
                    basic, outcome.exit_qualification, self.vcpu.iter,
                );
                if sh.serial.panic_observed {
                    kprintln!(
                        "[microvm]   note: kernel panic was observed: {}",
                        sh.serial.panic_msg_str(),
                    );
                }
                // VM-entry failure (33/34/41): dump guest state so we can
                // tell *which* consistency check the CPU rejected.
                // Bare-metal NUC reports 33 right after Linux's CR3
                // long-mode trampoline — likely the IA-32e/CR/EFER
                // triad is inconsistent and we need to see exactly
                // which field. SDM Vol 3 §26.3.1 enumerates the checks.
                if basic == 33 || basic == 34 || basic == 41 {
                    kprintln!("[vmx] entry_intr at fail = {:#x}", entry_intr_used);
                    vmcs::dump_entry_fail_state();
                }
                self.vcpu.io_stats.dump();
                self.vcpu.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
        }
    }

    if self.vcpu.iter >= MAX_ITERATIONS && crate::microvm::vm_window() == 0 {
        self.shared.serial.flush();
        kprintln!(
            "[microvm] iteration cap ({}) reached — guest still running, ({} I/O drops)",
            MAX_ITERATIONS, self.vcpu.io_dropped,
        );
        self.vcpu.io_stats.dump();
        self.vcpu.trace.dump();
    }

    match &last_outcome {
        Some(o) => kprintln!(
            "[microvm] run_slice returning Ok(reason={} qual={:#x})",
            vmcs::basic_exit_reason(o.exit_reason), o.exit_qualification,
        ),
        None => kprintln!("[microvm] run_slice returning Err (no outcome captured)"),
    }

    // Persist the virtio-blk profile-image to npkFS (encrypted at rest).
    // Reached only when the loop ended (guest exit / cap), not on a
    // StillRunning yield or a `?` early-return — identical to the old
    // run_linux_loop.
    self.shared.pci.virtio_blk.save();

    match last_outcome {
        Some(o) => Ok(SliceOutcome::Exited(o)),
        None => Err("Linux guest exceeded max iterations without first VM-exit"),
    }
    }
}

/// MSRs that Linux probes via `safe_rdmsr` (catches #GP) but we
/// know are vendor-specific noise on a typical Intel host. Suppress
/// the per-exit log line so the kernel's actual unhandled-MSR list
/// stays readable. Linux behaves correctly on the synthesized 0 — its
/// safe_rdmsr_on_cpu callers all check the return value.
fn msr_is_known_noise(msr: u32) -> bool {
    match msr {
        // AMD K8/K10/Family-17h architectural MSRs that Linux probes
        // for power/temperature features (HWP, smbus, LS_CFG). Always
        // absent on Intel.
        0xC001_1029 |  // AMD LS_CFG
        0xC001_0015 |  // AMD HWCR
        0xC001_001F => true, // AMD NB_CFG
        _ => false,
    }
}

/// Read a guest GPR by ABI register index (0=rax, 1=rcx, 2=rdx,
/// 3=rbx, 4=rsp, 5=rbp, 6=rsi, 7=rdi, 8..15=r8..r15) for CR-access
/// VM-exit decoding. RSP comes from VMCS, the rest from the saved
/// GuestRegs struct.
fn read_gpr(regs: &vmcs::GuestRegs, idx: u8) -> Result<u64, &'static str> {
    Ok(match idx {
        0 => regs.rax,
        1 => regs.rcx,
        2 => regs.rdx,
        3 => regs.rbx,
        4 => vmcs::read_guest_rsp()?,
        5 => regs.rbp,
        6 => regs.rsi,
        7 => regs.rdi,
        8 => regs.r8,
        9 => regs.r9,
        10 => regs.r10,
        11 => regs.r11,
        12 => regs.r12,
        13 => regs.r13,
        14 => regs.r14,
        15 => regs.r15,
        _ => return Err("invalid GPR index"),
    })
}

/// Write a guest GPR by ABI register index. RSP goes to VMCS, the
/// rest to the saved GuestRegs struct.
fn write_gpr(regs: &mut vmcs::GuestRegs, idx: u8, value: u64) -> Result<(), &'static str> {
    match idx {
        0 => regs.rax = value,
        1 => regs.rcx = value,
        2 => regs.rdx = value,
        3 => regs.rbx = value,
        4 => vmcs::write_guest_rsp(value)?,
        5 => regs.rbp = value,
        6 => regs.rsi = value,
        7 => regs.rdi = value,
        8 => regs.r8 = value,
        9 => regs.r9 = value,
        10 => regs.r10 = value,
        11 => regs.r11 = value,
        12 => regs.r12 = value,
        13 => regs.r13 = value,
        14 => regs.r14 = value,
        15 => regs.r15 = value,
        _ => return Err("invalid GPR index"),
    }
    Ok(())
}

/// Dispatch a single I/O VM-exit. UART COM1 (0x3F8-0x3FF) gets
/// proper synthetic responses so Linux's earlyprintk poll-loop
/// thinks the transmitter is always ready; everything else is
/// silently absorbed (return 0 for IN, no-op for OUT).
fn handle_linux_io(
    serial: &mut SerialState,
    pci: &mut crate::microvm::devices::PciBus,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    regs: &mut vmcs::GuestRegs,
    port: u16,
    dir_in: bool,
    size: u8,
    io_dropped: &mut u32,
    pit_enabled: &mut bool,
) {
    use crate::microvm::devices::{handle_pci_io, PCI_CONFIG_ADDR, PCI_CONFIG_DATA_END, PCI_CONFIG_DATA_START};
    use crate::microvm::devices::pic8259::{handle_pic_io, PIC_MASTER_CMD, PIC_MASTER_IMR, PIC_SLAVE_CMD, PIC_SLAVE_IMR};

    let mask: u64 = match size { 1 => 0xFF, 2 => 0xFFFF, 4 => 0xFFFF_FFFF, _ => 0xFF };
    let val_out = (regs.rax & mask) as u32;

    // PCI config-space ports — dispatch to the bus emulator.
    if port == PCI_CONFIG_ADDR
        || (PCI_CONFIG_DATA_START..=PCI_CONFIG_DATA_END).contains(&port)
    {
        if let Some(v) = handle_pci_io(pci, port, dir_in, size, val_out) {
            regs.rax = (regs.rax & !mask) | (v & mask);
        }
        return;
    }

    // 8259 PIC stub — see microvm::devices::pic8259.
    if matches!(port, PIC_MASTER_CMD | PIC_MASTER_IMR | PIC_SLAVE_CMD | PIC_SLAVE_IMR) {
        if let Some(v) = handle_pic_io(pic, port, dir_in, val_out as u8) {
            regs.rax = (regs.rax & !mask) | (v & mask);
        }
        return;
    }

    match (port, dir_in) {
        // i8253 PIT mode/command (0x43) write. Track whether channel 0 is
        // still generating the timer tick: a mode-set (access bits 5:4 != 0)
        // on channel 0 (bits 7:6 == 0) sets pit_enabled = (operating mode !=
        // 0). Linux shuts the PIT down with mode 0 (0x30) after adopting the
        // LAPIC timer; periodic (0x34) / oneshot (0x38) keep it live. Latch
        // commands (bits 5:4 == 0) don't change the mode. Mirrors svm.
        (0x43, false) => {
            let v = val_out as u8;
            if (v >> 6) & 0x3 == 0 && (v >> 4) & 0x3 != 0 {
                *pit_enabled = ((v >> 1) & 0x7) != 0;
            }
        }
        // COM1 OUT.
        (0x3F8, false) => {
            if !serial.dlab {
                serial.put_char(val_out as u8);
            }
            // else: divisor-latch low byte, ignored.
        }
        (0x3F9, false) => {
            // IER (DLAB=0) or DLM (DLAB=1) — both ignored.
        }
        (0x3FB, false) => {
            // LCR — track DLAB bit.
            serial.dlab = (val_out & 0x80) != 0;
        }
        // COM1 IN — synthetic responses.
        (0x3F8, true) => {
            // RBR (DLAB=0): pop one byte from the host-injected RX
            // FIFO. DLL (DLAB=1): we don't model divisor latches —
            // return 0. With an empty FIFO this also returns 0,
            // matching real hardware where reading RBR with DR=0 is
            // undefined-but-typically-zero.
            let v = if !serial.dlab { serial.rx_take() as u64 } else { 0 };
            regs.rax = (regs.rax & !mask) | (v & mask);
        }
        (0x3FA, true) => {
            // IIR: bit 0 = "no interrupt pending" (which on read
            // also sources type=0 = no FIFO).
            regs.rax = (regs.rax & !mask) | (0x01u64 & mask);
        }
        (0x3FD, true) => {
            // LSR: bit 5 (THR empty) | bit 6 (TSR empty) always set,
            // plus bit 0 (DR — data ready) reflects the RX FIFO.
            // Polling loops in the guest see DR=1 the moment the
            // host has injected, and read RBR until the FIFO drains.
            let dr = if serial.rx_has_data() { 0x01u64 } else { 0 };
            regs.rax = (regs.rax & !mask) | ((0x60u64 | dr) & mask);
        }
        (0x3FE, true) => {
            // MSR: CTS asserted (bit 4) + DSR (bit 5) + DCD (bit 7).
            regs.rax = (regs.rax & !mask) | (0xB0u64 & mask);
        }
        // Other UART regs (0x3FC MCR, 0x3FF SCR): default 0.
        (0x3FA..=0x3FF, true) => {
            regs.rax = (regs.rax & !mask) | (0u64 & mask);
        }
        // Default IN: zero. Default OUT: drop.
        (_, true) => {
            regs.rax = (regs.rax & !mask) | (0u64 & mask);
            *io_dropped += 1;
        }
        (_, false) => {
            *io_dropped += 1;
        }
    }
}

/// Handle an EPT violation that targets virtio-blk's BAR0 MMIO range.
/// Walks the guest's page tables to fetch the faulting instruction
/// (VMX has no decode-assists), decodes the MOV form, emulates against
/// the device, advances RIP via `VM_EXIT_INSTRUCTION_LEN`.
///
/// Returns `true` if the fault was handled, `false` otherwise (page
/// walk failed, opcode unsupported).
/// EPT-violation on the LAPIC MMIO page (Intel parity #2 / guest SMP). Decode
/// the faulting MOV, service it against the per-vCPU `LocalApic`, advance RIP.
/// xAPIC registers are 32-bit; no device IRQ-kick. An ICR write returns the
/// decoded IPI, which `route_ipi` delivers cross-vCPU (INIT/SIPI bring-up +
/// FIXED reschedule/TLB). Mirrors svm `handle_mmio_npf_lapic`.
fn handle_mmio_lapic(
    regs: &mut vmcs::GuestRegs,
    apic: &mut LocalApic,
    apic_id: u8,
    sh: &mut VmShared,
    gpa: u64,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = match vmcs::read_guest_rip() { Ok(v) => v, Err(_) => return false };
    let cr3 = match vmcs::read_guest_cr3() { Ok(v) => v, Err(_) => return false };
    let buf = match fetch_inst(rip, cr3, &sh.guest_mem) {
        Some(b) => b,
        None => {
            kprintln!("[vmx] lapic mmio: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[vmx] lapic mmio: unsupported insn @ gpa={:#x} bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - lapic::LAPIC_BASE) as u32;
    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        if let Some(icr) = apic.write(off, value as u32) {
            route_ipi(sh, apic_id, &icr);
        }
    } else {
        let value = apic.read(off) as u64;
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }
    if vmcs::advance_guest_rip().is_err() {
        return false;
    }
    true
}

/// Route a decoded ICR write (guest SMP). FIXED/LOWEST → mark the vector
/// pending on the target vCPU(s); STARTUP → ask the orchestration layer to
/// spawn the AP at the SIPI vector; INIT/other → no-op (the AP starts from
/// its SIPI). Mirror of svm `route_ipi`.
fn route_ipi(sh: &mut VmShared, sender: u8, icr: &lapic::IcrWrite) {
    match icr.delivery_mode {
        lapic::ICR_DM_STARTUP => {
            crate::microvm::cpu::request_ap_spawn(icr.dest, icr.vector);
        }
        lapic::ICR_DM_FIXED | lapic::ICR_DM_LOWEST => {
            let n = crate::microvm::cpu::guest_vcpus();
            match icr.shorthand {
                0 => sh.ipi_set(icr.dest, icr.vector), // physical dest
                1 => sh.ipi_set(sender, icr.vector),   // self
                2 => {
                    for t in 0..n {
                        sh.ipi_set(t, icr.vector);
                    }
                } // all incl self
                _ => {
                    for t in 0..n {
                        if t != sender {
                            sh.ipi_set(t, icr.vector);
                        }
                    }
                } // all excl self
            }
        }
        _ => {}
    }
}

/// Deliver a device IRQ raised by an MMIO virtqueue-kick: inject it now
/// if the guest is interruptible, else latch the line in `pending` for
/// the run loop to drain once IF=1. The MMIO handlers run on EPT-
/// violation exits while the guest may sit IF=0 (it kicked the queue
/// from inside an ISR / `spin_lock_irqsave`); injecting a type-0
/// external interrupt then fails VM-entry with reason 33 on bare-metal
/// Intel (SDM §26.3.1.5). Mirrors SVM `deliver_irq`/`pending_irqs`.
fn deliver_irq_vmx(
    pending: &mut u16,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    line: u8,
) {
    if vmcs::guest_interruptible() {
        let _ = vmcs::inject_external_irq(pic.vector_for_irq(line));
    } else {
        *pending |= 1u16 << line;
    }
}

fn handle_mmio_ept_blk(
    regs: &mut vmcs::GuestRegs,
    blk: &mut crate::microvm::devices::virtio_blk_pci::VirtioBlk,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = match vmcs::read_guest_rip() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let cr3 = match vmcs::read_guest_cr3() {
        Ok(v) => v,
        Err(_) => return false,
    };

    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!(
                "[microvm] mmio: insn fetch failed (rip={:#x} cr3={:#x} gpa={:#x})",
                rip, cr3, gpa,
            );
            return false;
        }
    };

    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!(
                "[microvm] mmio: unsupported insn @ gpa={:#x}, bytes={:02x?}",
                gpa, &buf[..8],
            );
            return false;
        }
    };

    let off = (gpa - blk.bar0_base()) as u32;

    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        blk.mmio_write(off, dec.width, value);
    } else {
        let value = blk.mmio_read(off, dec.width);
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }

    // If the write was a queue-notify, service the queue and inject
    // IRQ 11 (virtio-blk's INTx line, mapped through our 8259 stub
    // to the vector Linux programmed via ICW2).
    if let Some(qidx) = blk.take_pending_kick() {
        let advanced = blk.service_queues(qidx, mem);
        if advanced {
            deliver_irq_vmx(pending, pic, blk.irq_line());
        }
    }

    if vmcs::advance_guest_rip().is_err() {
        return false;
    }
    true
}

fn read_gpr_vmx(regs: &vmcs::GuestRegs, idx: u8) -> u64 {
    match idx {
        0  => regs.rax,
        1  => regs.rcx,
        2  => regs.rdx,
        3  => regs.rbx,
        4  => 0, // RSP — VMCS holds it; never an MMIO source on Linux
        5  => regs.rbp,
        6  => regs.rsi,
        7  => regs.rdi,
        8  => regs.r8,
        9  => regs.r9,
        10 => regs.r10,
        11 => regs.r11,
        12 => regs.r12,
        13 => regs.r13,
        14 => regs.r14,
        15 => regs.r15,
        _  => 0,
    }
}

fn write_gpr_vmx(regs: &mut vmcs::GuestRegs, idx: u8, width: u8, value: u64) {
    use crate::microvm::devices::insn_decoder::merge_reg;
    match idx {
        0  => regs.rax = merge_reg(regs.rax, value, width),
        1  => regs.rcx = merge_reg(regs.rcx, value, width),
        2  => regs.rdx = merge_reg(regs.rdx, value, width),
        3  => regs.rbx = merge_reg(regs.rbx, value, width),
        4  => {} // RSP — silently drop
        5  => regs.rbp = merge_reg(regs.rbp, value, width),
        6  => regs.rsi = merge_reg(regs.rsi, value, width),
        7  => regs.rdi = merge_reg(regs.rdi, value, width),
        8  => regs.r8  = merge_reg(regs.r8,  value, width),
        9  => regs.r9  = merge_reg(regs.r9,  value, width),
        10 => regs.r10 = merge_reg(regs.r10, value, width),
        11 => regs.r11 = merge_reg(regs.r11, value, width),
        12 => regs.r12 = merge_reg(regs.r12, value, width),
        13 => regs.r13 = merge_reg(regs.r13, value, width),
        14 => regs.r14 = merge_reg(regs.r14, value, width),
        15 => regs.r15 = merge_reg(regs.r15, value, width),
        _  => {}
    }
}

/// Handle an EPT violation that targets virtio-net's BAR0. Identical
/// pattern to `handle_mmio_ept_blk` — only the device + IRQ line
/// differ. We'll de-duplicate via a trait once virtio-gpu joins (12.4).
fn handle_mmio_ept_net(
    regs: &mut vmcs::GuestRegs,
    net: &mut crate::microvm::devices::virtio_net_pci::VirtioNet,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = match vmcs::read_guest_rip() { Ok(v) => v, Err(_) => return false };
    let cr3 = match vmcs::read_guest_cr3() { Ok(v) => v, Err(_) => return false };
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!("[microvm] mmio-net: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[microvm] mmio-net: unsupported insn @ gpa={:#x}, bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - net.bar0_base()) as u32;
    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        net.mmio_write(off, dec.width, value);
    } else {
        let value = net.mmio_read(off, dec.width);
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }

    if let Some(qidx) = net.take_pending_kick() {
        let advanced = net.service_queues(qidx, mem);
        if advanced {
            // virtio-net IRQ line = 10 (per pci config 0x3C).
            deliver_irq_vmx(pending, pic, 10);
        }
    }

    if vmcs::advance_guest_rip().is_err() { return false; }
    true
}


/// Handle EPT-trap on virtio-gpu BAR0. Mirror of `handle_mmio_ept_net`
/// — only the device and IRQ line differ.
fn handle_mmio_ept_gpu(
    regs: &mut vmcs::GuestRegs,
    gpu: &mut crate::microvm::devices::virtio_gpu_pci::VirtioGpu,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = match vmcs::read_guest_rip() { Ok(v) => v, Err(_) => return false };
    let cr3 = match vmcs::read_guest_cr3() { Ok(v) => v, Err(_) => return false };
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!("[microvm] mmio-gpu: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[microvm] mmio-gpu: unsupported insn @ gpa={:#x}, bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - gpu.bar0_base()) as u32;
    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        gpu.mmio_write(off, dec.width, value);
    } else {
        let value = gpu.mmio_read(off, dec.width);
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }

    if let Some(qidx) = gpu.take_pending_kick() {
        let advanced = gpu.service_queues(qidx, mem);
        if advanced {
            // virtio-gpu IRQ line = 9.
            deliver_irq_vmx(pending, pic, 9);
        }
    }

    if vmcs::advance_guest_rip().is_err() { return false; }
    true
}

/// Handle EPT-trap on virtio-input BAR0. Mirror of `handle_mmio_ept_gpu`
/// — only the device + IRQ line differ.
fn handle_mmio_ept_input(
    regs: &mut vmcs::GuestRegs,
    input: &mut crate::microvm::devices::virtio_input_pci::VirtioInput,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = match vmcs::read_guest_rip() { Ok(v) => v, Err(_) => return false };
    let cr3 = match vmcs::read_guest_cr3() { Ok(v) => v, Err(_) => return false };
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!("[microvm] mmio-input: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[microvm] mmio-input: unsupported insn @ gpa={:#x}, bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - input.bar0_base()) as u32;
    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        input.mmio_write(off, dec.width, value);
    } else {
        let value = input.mmio_read(off, dec.width);
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }

    if let Some(qidx) = input.take_pending_kick() {
        let advanced = input.service_queues(qidx, mem);
        if advanced {
            // virtio-input IRQ line = 12.
            deliver_irq_vmx(pending, pic, 12);
        }
    }

    if vmcs::advance_guest_rip().is_err() { return false; }
    true
}

/// Handle an EPT violation on virtio-9p BAR0. Mirror of
/// `handle_mmio_ept_input` — only the device + IRQ line (6) differ.
fn handle_mmio_ept_p9(
    regs: &mut vmcs::GuestRegs,
    p9: &mut crate::microvm::devices::virtio_9p_pci::Virtio9p,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = match vmcs::read_guest_rip() { Ok(v) => v, Err(_) => return false };
    let cr3 = match vmcs::read_guest_cr3() { Ok(v) => v, Err(_) => return false };
    let buf = match fetch_inst(rip, cr3, mem) {
        Some(b) => b,
        None => {
            kprintln!("[microvm] mmio-9p: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
            return false;
        }
    };
    let dec = match decode_mov(&buf) {
        Some(d) => d,
        None => {
            kprintln!("[microvm] mmio-9p: unsupported insn @ gpa={:#x}, bytes={:02x?}", gpa, &buf[..8]);
            return false;
        }
    };

    let off = (gpa - p9.bar0_base()) as u32;
    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        p9.mmio_write(off, dec.width, value);
    } else {
        let value = p9.mmio_read(off, dec.width);
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }

    if let Some(qidx) = p9.take_pending_kick() {
        let advanced = p9.service_queues(qidx, mem);
        if advanced {
            deliver_irq_vmx(pending, pic, p9.irq_line());
        }
    }

    if vmcs::advance_guest_rip().is_err() { return false; }
    true
}

/// Handle EPT-trap on virtio-snd BAR0. Mirror of `handle_mmio_ept_net` —
/// only the device + IRQ line (8) differ.
fn handle_mmio_ept_snd(
    regs: &mut vmcs::GuestRegs,
    snd: &mut crate::microvm::devices::virtio_snd_pci::VirtioSnd,
    pic: &crate::microvm::devices::pic8259::Pic8259,
    pending: &mut u16,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = match vmcs::read_guest_rip() { Ok(v) => v, Err(_) => return false };
    let cr3 = match vmcs::read_guest_cr3() { Ok(v) => v, Err(_) => return false };
    let buf = match fetch_inst(rip, cr3, mem) { Some(b) => b, None => return false };
    let dec = match decode_mov(&buf) { Some(d) => d, None => return false };

    let off = (gpa - snd.bar0_base()) as u32;
    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        snd.mmio_write(off, dec.width, value);
    } else {
        let value = snd.mmio_read(off, dec.width);
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }

    if let Some(qidx) = snd.take_pending_kick() {
        let advanced = snd.service_queues(qidx, mem);
        if advanced {
            deliver_irq_vmx(pending, pic, snd.irq_line());
        }
    }

    if vmcs::advance_guest_rip().is_err() { return false; }
    true
}
