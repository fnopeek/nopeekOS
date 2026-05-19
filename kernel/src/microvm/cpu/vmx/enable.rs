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

    for _ in 0..MAX_ITERATIONS {
        let outcome = vmcs::run_guest_once(&mut regs, launched)?;
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
    /// Budget exhausted, guest still running — caller may re-enter.
    StillRunning,
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

/// Persistent state of one Linux microvm across cooperative slices.
/// Owns the VMX-root resources and all run-loop state that must
/// survive between `run_slice` calls. Core-agnostic: nothing here
/// assumes which core it runs on (forward-compat contract #1).
pub struct VmContext {
    vmxon_phys: u64,
    vmcs_phys: u64,
    guest_mem: GuestMem,
    /// EPT PML4 phys — `close()` passes it to `ept::release` to free
    /// every demand-faulted 4 KB frame + the demand PT pages + the
    /// fixed tables.
    ept_pml4: u64,
    /// Base of the **contiguous boot-window** allocation (pre-2 MB-
    /// align). `close()` frees `ept::boot_frames_for(guest_mem.len())`
    /// from here; the demand region is freed via `ept::release`.
    /// Without this teardown a second `microvm linux` OOMs.
    guest_raw_base: u64,
    regs: vmcs::GuestRegs,
    serial: SerialState,
    pci: crate::microvm::devices::PciBus,
    pic: crate::microvm::devices::pic8259::Pic8259,
    trace: ExitTrace,
    io_stats: IoStats,
    launched: bool,
    iter: u32,
    io_dropped: u32,
    msr_log_count: u32,
    consecutive_idle: u32,
    /// Host tick of the last injected guest timer IRQ0. No PIT/LAPIC
    /// timer source exists in the microvm, so this is the only thing
    /// that wakes a time-blocked guest task (nanosleep/timerfd/poll).
    /// One IRQ0 per host tick (≈100 Hz). Mirrors the SVM side.
    last_timer_tick: u64,
    /// `ticks()` of the last virtio-gpu display config-change IRQ.
    /// Rate-limits the resize round-trip: a tile drag retiles every
    /// frame → without this the guest gets a config-change storm and
    /// wlroots rescans connectors in a tight loop, never settling on
    /// the final size (R2 debounce). The dirty flag persists, so the
    /// final size is still delivered once the window reopens.
    last_cfg_tick: u64,
    /// Host/guest FPU (XSAVE) save areas — VMRESUME preserves neither.
    /// See `cpu::FpuArea`. guest_fpu starts zeroed = FPU init state.
    host_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,
    guest_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,
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
                vmxon_phys,
                vmcs_phys,
                guest_mem: gm,
                ept_pml4,
                guest_raw_base,
                regs,
                serial,
                pci: crate::microvm::devices::PciBus::new(),
                pic: crate::microvm::devices::pic8259::Pic8259::new(),
                trace: ExitTrace::new(),
                io_stats: IoStats::new(),
                launched: false,
                iter: 0,
                io_dropped: 0,
                msr_log_count: 0,
                consecutive_idle: 0,
                last_timer_tick: 0,
                last_cfg_tick: 0,
                host_fpu: crate::microvm::cpu::FpuArea::boxed(),
                guest_fpu: crate::microvm::cpu::FpuArea::boxed(),
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

    /// Leave VMX root and free the per-VM allocations so the VM can be
    /// relaunched in the same boot. Order matters: VMXOFF first (the
    /// VMCS must not be current/in-use when its frame is freed), then
    /// reclaim frames. The profile image was already persisted inside
    /// run_slice on the guest-exit path.
    ///
    /// TODO(12.x): the EPT page-table frames from `ept::install_window`
    /// still leak (~tens of KB per run — negligible vs. the 1 GB guest
    /// RAM this now reclaims). Tracked; needs an EPT teardown walker.
    pub fn close(&mut self) {
        // SAFETY: a VmContext only exists after a successful
        // vmx_enter_root, so the CPU is in VMX root. After VMXOFF the
        // VMCS is no longer current and its frame is safe to free.
        unsafe { vmx_exit_root(); }
        // Demand-faulted frames + demand PTs + EPT tables.
        ept::release(self.ept_pml4, self.guest_mem.len());
        // Contiguous boot window.
        memory::deallocate_contiguous(
            self.guest_raw_base,
            ept::boot_frames_for(self.guest_mem.len()),
        );
        memory::deallocate_frame(self.vmcs_phys);
        memory::deallocate_frame(self.vmxon_phys);
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
            Ok(SliceOutcome::StillRunning) => continue,
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

    while self.iter < MAX_ITERATIONS || crate::microvm::vm_window() != 0 {
        if slice_n >= budget || crate::interrupts::rdtsc() >= slice_deadline {
            return Ok(SliceOutcome::StillRunning);
        }
        self.iter = self.iter.saturating_add(1);
        slice_n += 1;
        // Before each entry, sync the IA-32e-mode-guest control to
        // the current GUEST_IA32_EFER.LMA — once Linux flips into
        // long mode (after CR0.PG=1 with EFER.LME=1), the entry
        // control must match or VMX rejects the entry.
        if self.launched {
            vmcs::sync_entry_ia32e_with_efer()?;
        }
        // FPU swap (KVM kvm_load/put_guest_fpu): VMRESUME preserves no
        // x87/SSE/AVX/AVX-512. Restore host FPU before `?` so an entry
        // error can't strand the host on guest FPU.
        // SAFETY: CR4.OSXSAVE=1 on every core; areas valid + aligned.
        unsafe {
            crate::microvm::cpu::fpu_xsave(&mut *self.host_fpu);
            crate::microvm::cpu::fpu_xrstor(&*self.guest_fpu);
        }
        let result = vmcs::run_guest_once(&mut self.regs, self.launched);
        // SAFETY: as above; save guest FPU, restore host's.
        unsafe {
            crate::microvm::cpu::fpu_xsave(&mut *self.guest_fpu);
            crate::microvm::cpu::fpu_xrstor(&*self.host_fpu);
        }
        let outcome = result?;
        self.launched = true;
        let basic = vmcs::basic_exit_reason(outcome.exit_reason);
        self.trace.record(basic, outcome.exit_qualification);

        // Reset idle-counter on any non-timer-tick activity. The
        // counter only progresses on a clean run of pure reason-1
        // exits (= guest sitting in pause(2)).
        if basic != 1 {
            self.consecutive_idle = 0;
        }

        match basic {
            0 => {
                // Exception/NMI. EXCEPTION_BITMAP=0 in production —
                // exceptions go to Linux's IDT directly. This arm
                // only fires for NMIs (which we don't generate
                // intentionally) or if Linux somehow re-enables
                // exception trapping. Kept as a safety net + the
                // dump remains useful if it ever fires.
                self.serial.flush();
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
                    dump_page_walk(&self.guest_mem, cr3, outcome.exit_qualification);
                }
                self.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            1 => {
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
                {
                    let wid = crate::microvm::vm_window();
                    if wid != 0 && crate::shade::surface::display_dirty_peek(wid) {
                        // R2 debounce: at most one config-change per
                        // ~250 ms (25 host ticks). A tile drag retiles
                        // every frame; firing each one made wlroots
                        // rescan DRM connectors in a tight loop and
                        // never settle on the final size. We only
                        // `take_` (clear) the flag when we actually
                        // fire, so a drag in progress keeps it dirty
                        // and the FINAL size is delivered ≤250 ms
                        // after the drag ends.
                        let now = crate::interrupts::ticks();
                        if now.wrapping_sub(self.last_cfg_tick) >= 25 {
                            let _ = crate::shade::surface::take_display_dirty(wid);
                            self.last_cfg_tick = now;
                            self.pci.virtio_gpu.signal_display_change();
                            let vector = self.pic.vector_for_irq(9);
                            let _ = vmcs::inject_external_irq(vector);
                            self.consecutive_idle = 0;
                            if let Some((tw, th)) =
                                crate::shade::surface::tile_size(wid)
                            {
                                kprintln!(
                                    "[gpu] display-change IRQ fired (tile {}x{}, guest should re-query)",
                                    tw, th,
                                );
                            }
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
                if self.pic.irq_unmasked(0)
                    && now.wrapping_sub(self.last_timer_tick) >= TIMER_MAX_SKIP
                {
                    self.last_timer_tick = now;
                    let vector = self.pic.vector_for_irq(0);
                    let _ = vmcs::inject_external_irq(vector);
                    self.consecutive_idle = 0;
                    continue;
                }
                let pumped = crate::microvm::devices::nat::pump(
                    &mut self.pci.virtio_net, &self.guest_mem);
                if self.pci.virtio_input.drain_injected(&self.guest_mem) {
                    let vector = self.pic.vector_for_irq(12);
                    let _ = vmcs::inject_external_irq(vector);
                    self.consecutive_idle = 0;
                    continue;
                }
                if pumped {
                    let vector = self.pic.vector_for_irq(10);
                    let _ = vmcs::inject_external_irq(vector);
                    self.consecutive_idle = 0;
                    continue;
                }
                if now != self.last_timer_tick && self.pic.irq_unmasked(0) {
                    self.last_timer_tick = now;
                    let vector = self.pic.vector_for_irq(0);
                    let _ = vmcs::inject_external_irq(vector);
                    self.consecutive_idle = 0;
                    continue;
                }
                // Pure timer-tick → idle counter advances. Reset when
                // there are active NAT sessions (guest is blocked on
                // I/O, not actually idle).
                if pumped || crate::microvm::devices::nat::active_session_count() > 0 {
                    self.consecutive_idle = 0;
                } else {
                    self.consecutive_idle = self.consecutive_idle.saturating_add(1);
                }
                // Idle-auto-exit only for an UNWINDOWED VM (legacy
                // blocking-smoke model: "guest done, stop wasting Core
                // 0"). A window-bound VM is an app — idle is normal
                // (a parked guest, an idle browser) and must NOT be
                // killed. It ends only on real guest exit (HLT /
                // shutdown) or window close (VM_CLOSE_REQUESTED, handled
                // in vm_poll_slice). Cheap to keep slicing: idle =
                // timer-tick exits, near-free.
                if self.consecutive_idle >= IDLE_THRESHOLD
                    && crate::microvm::vm_window() == 0
                {
                    self.serial.flush();
                    kprintln!(
                        "[microvm] guest idle in userspace ({} consecutive timer ticks after {} iters) — exiting cleanly",
                        self.consecutive_idle, self.iter,
                    );
                    last_outcome = Some(outcome);
                    break;
                }
                // Guest idle → give Core 0 back NOW (StillRunning, not
                // Exited — the guest stays alive). consecutive_idle
                // persists in the VmContext across yields, so the
                // unwindowed idle-exit above still triggers once it
                // reaches IDLE_THRESHOLD over many yield cycles.
                if self.consecutive_idle >= IDLE_YIELD {
                    return Ok(SliceOutcome::StillRunning);
                }
                last_outcome = Some(outcome);
            }
            12 => {
                self.serial.flush();
                kprintln!("[microvm] guest HLT after {} VM-exits", self.iter);
                self.io_stats.dump();
                last_outcome = Some(outcome);
                break;
            }
            10 => {
                // CPUID — VMX always exits on CPUID. Pass through
                // to host; guest sees real CPU features. Linux uses
                // this for early feature detection. Filtered for
                // features we can't safely expose to the guest.
                let leaf = self.regs.rax as u32;
                let subleaf = self.regs.rcx as u32;
                let (eax, ebx, mut ecx, mut edx) =
                    vmcs::host_cpuid(leaf, subleaf);
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
                }
                self.regs.rax = eax as u64;
                self.regs.rbx = ebx as u64;
                self.regs.rcx = ecx as u64;
                self.regs.rdx = edx as u64;
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
                    self.serial.flush();
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
                        let val = read_gpr(&self.regs, gp_reg)?;
                        vmcs::write_guest_cr3(val)?;
                    }
                    1 => {
                        // MOV from CR3.
                        let val = vmcs::read_guest_cr3()?;
                        write_gpr(&mut self.regs, gp_reg, val)?;
                    }
                    _ => {
                        self.serial.flush();
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
                self.io_stats.record(port, dir_in);
                if port == 0x3F8 && !dir_in && !self.serial.dlab && size == 1 {
                    self.io_stats.record_serial_byte((self.regs.rax & 0xFF) as u8);
                }
                handle_linux_io(&mut self.serial, &mut self.pci, &mut self.pic, &mut self.regs, port, dir_in, size, &mut self.io_dropped);
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
                let msr = self.regs.rcx as u32;
                if !msr_is_known_noise(msr) && self.msr_log_count < MSR_LOG_CAP {
                    kprintln!("[microvm] RDMSR {:#010x} → 0 (unhandled)", msr);
                    self.msr_log_count += 1;
                }
                self.regs.rax = 0;
                self.regs.rdx = 0;
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            32 => {
                // WRMSR — same gating as RDMSR. Silently absorb
                // (don't propagate to host — writing arbitrary MSRs
                // would break the host's pstate/PMU/etc).
                let msr = self.regs.rcx as u32;
                if self.msr_log_count < MSR_LOG_CAP {
                    let val = (self.regs.rdx << 32) | (self.regs.rax & 0xFFFF_FFFF);
                    kprintln!("[microvm] WRMSR {:#010x} = {:#018x} (absorbed)", msr, val);
                    self.msr_log_count += 1;
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
                if self.pci.virtio_blk.bar0_in_range(gpa) {
                    if handle_mmio_ept_blk(&mut self.regs, &mut self.pci.virtio_blk, &self.pic, gpa, &self.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if self.pci.virtio_net.bar0_in_range(gpa) {
                    if handle_mmio_ept_net(&mut self.regs, &mut self.pci.virtio_net, &self.pic, gpa, &self.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if self.pci.virtio_gpu.bar0_in_range(gpa) {
                    if handle_mmio_ept_gpu(&mut self.regs, &mut self.pci.virtio_gpu, &self.pic, gpa, &self.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if self.pci.virtio_input.bar0_in_range(gpa) {
                    if handle_mmio_ept_input(&mut self.regs, &mut self.pci.virtio_input, &self.pic, gpa, &self.guest_mem) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if self.pci.virtio_blk_sqfs.bar0_in_range(gpa) {
                    // Same handler — VirtioBlk carries its own IRQ line.
                    if handle_mmio_ept_blk(&mut self.regs, &mut self.pci.virtio_blk_sqfs, &self.pic, gpa, &self.guest_mem) {
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
                if self.guest_mem.ensure(gpa) {
                    last_outcome = Some(outcome);
                    continue;
                }
                self.serial.flush();
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
                self.io_stats.dump();
                self.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            2 => {
                // Triple fault. Linux uses this as `emergency_restart`
                // when ACPI/PIIX/EFI reset paths are unavailable —
                // i.e. the standard exit path on `panic=1` here.
                self.serial.flush();
                if self.serial.panic_observed {
                    kprintln!(
                        "[microvm] linux kernel panicked (after {} iters): {}",
                        self.iter, self.serial.panic_msg_str(),
                    );
                    kprintln!("[microvm] guest then triple-faulted via emergency_restart (= expected reboot path)");
                } else {
                    kprintln!(
                        "[microvm] guest triple-faulted after {} iters (no kernel-panic seen on console)",
                        self.iter,
                    );
                }
                self.io_stats.dump();
                self.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            _ => {
                self.serial.flush();
                kprintln!(
                    "[microvm] unhandled exit reason {} qual {:#x} after {} iters",
                    basic, outcome.exit_qualification, self.iter,
                );
                if self.serial.panic_observed {
                    kprintln!(
                        "[microvm]   note: kernel panic was observed: {}",
                        self.serial.panic_msg_str(),
                    );
                }
                self.io_stats.dump();
                self.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
        }
    }

    if self.iter >= MAX_ITERATIONS && crate::microvm::vm_window() == 0 {
        self.serial.flush();
        kprintln!(
            "[microvm] iteration cap ({}) reached — guest still running, ({} I/O drops)",
            MAX_ITERATIONS, self.io_dropped,
        );
        self.io_stats.dump();
        self.trace.dump();
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
    self.pci.virtio_blk.save();

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
fn handle_mmio_ept_blk(
    regs: &mut vmcs::GuestRegs,
    blk: &mut crate::microvm::devices::virtio_blk_pci::VirtioBlk,
    pic: &crate::microvm::devices::pic8259::Pic8259,
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
            let vector = pic.vector_for_irq(blk.irq_line());
            let _ = vmcs::inject_external_irq(vector);
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
            let vector = pic.vector_for_irq(10);
            let _ = vmcs::inject_external_irq(vector);
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
            let vector = pic.vector_for_irq(9);
            let _ = vmcs::inject_external_irq(vector);
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
            let vector = pic.vector_for_irq(12);
            let _ = vmcs::inject_external_irq(vector);
        }
    }

    if vmcs::advance_guest_rip().is_err() { return false; }
    true
}
