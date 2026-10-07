//! VMX root-mode entry/exit + VMCS round-trip + VMLAUNCH.
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
use alloc::sync::Arc;
use core::sync::atomic::Ordering;
// LAPIC emulation is pure register state (only uses rdtsc), so the SVM
// module's `LocalApic` is reused rather than duplicated. `IcrWrite` +
// `ICR_DM_*` are also reused by the cross-vCPU IPI router (guest SMP).
use crate::microvm::cpu::svm::lapic::{self, LocalApic};

/// True when the guest LAPIC is emulated on this (VMX ⇒ Intel) host. Cheap
/// const check (no vendor lock — vmx code only runs on Intel). Gates every
/// LAPIC-related VMX path; false ⇒ the `nolapic` boot. See
/// `cpu::VMX_GUEST_LAPIC`.
const VEC_UD: u8 = 6;
const VEC_GP: u8 = 13;

#[inline]
fn vmx_lapic_on() -> bool {
    crate::microvm::cpu::GUEST_LAPIC && crate::microvm::cpu::VMX_GUEST_LAPIC
}

// ── Guest SMP — mirror of svm/enable.rs ─────────────────────────────
//
// Same architecture as the AMD backend: `VmContext` is split into a shared
// `VmShared` (one guest address space + device model; every vCPU holds an
// `Arc` of it) and a per-vCPU `Vcpu` (its own VMXON/VMCS/regs/FPU/LAPIC/
// apic_id). The BSP and AP run their VMRESUME loops as pinned pool fibers on
// separate worker cores, in parallel, serialising only their device handling
// under `VmShared::dev`. VMXON/VMCS are per core (each vCPU enters VMX root
// on its own core), the AP never locks VENDOR (it learns its vendor
// lock-free via `detect_vendor`), IPIs inject only when interruptible
// (reason-33 guard), and devices/timer/nat/input are BSP-only.

/// The open VM's shared state, for AP vCPUs to join (`open_ap`). Set by the
/// BSP once it has opened the VM, cleared at teardown.
static AP_SHARED: spin::Mutex<Option<Arc<VmShared>>> = spin::Mutex::new(None);
/// The BSP's last device deadline read under `VmShared::dev` (0 = none).
static LAST_DEV_DEADLINE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Withdraw the shared state offered to AP vCPUs (VM teardown).
pub fn clear_ap_shared() {
    let old = AP_SHARED.lock().take();
    drop(old);
}


/// Bounded adaptive halt-polling (KVM `halt_poll_ns` model). When a vCPU goes
/// idle it polls — re-pumping + re-checking for a wake event (RX / IRQ / IPI) —
/// for up to its current window before truly parking. A wake within the window
/// grows the window (×2, latency pays off); an expiry shrinks it (÷2, the vCPU
/// was idle, save power). Idle vCPUs shrink toward the floor (~park), a
/// latency-sensitive vCPU (the network BSP between RX bursts) grows toward
/// the cap, and a busy vCPU never reaches the idle path at all. Per-vCPU, no
/// global state. µs units.
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
    // SAFETY: only called in VMX root after VMPTRLD (open, open_ap, the
    // substrate test); `host_rsp` is this frame's live stack.
    unsafe { vmcs::setup_host_state(host_rsp) }
}

/// Allocate only the contiguous boot window (256 MiB or the whole guest
/// if smaller, plus 2-MB-align slack) and install the EPT window over
/// `guest_bytes`; `[boot, guest_bytes)` is demand-paged 4 KB and needs no
/// upfront allocation. Returns `(boot_base, eptp, pml4_phys, boot_raw_base)`.
fn alloc_guest_ram_and_ept(guest_bytes: u64) -> Result<(u64, u64, u64, u64), &'static str> {
    let raw_base = memory::allocate_contiguous(ept::boot_frames_for(guest_bytes))
        .ok_or("OOM allocating guest boot window (+ slack)")?;
    // Freed frames are not scrubbed, and the guest can read every byte of
    // its window: whatever the host kept there before (decrypted store
    // blocks, page data, keys) must not be in it. Demand-paged frames are
    // zeroed when faulted in; this is the window that is not.
    // SAFETY: the frames were just allocated to this VM and are
    // identity-mapped; nothing else holds them.
    unsafe {
        core::ptr::write_bytes(raw_base as *mut u8, 0,
            ept::boot_frames_for(guest_bytes) * 4096);
    }
    let boot_base = ept::round_up_to_2mb(raw_base);
    let (eptp, pml4_phys) = ept::install_window(boot_base, guest_bytes)?;
    Ok((boot_base, eptp, pml4_phys, raw_base))
}

// ── Substrate test ─────────────────────────────────────────────────

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
        // SAFETY: host_base is 2-MB-aligned and the boot window starting
        // there is exclusively ours.
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
        // Kept for the test's lifetime, like the rest of its frames.
        // SAFETY: in VMX root with the VMCS current; `eptp` is this test's
        // own EPT from `alloc_guest_ram_and_ept`.
        let _bitmaps = unsafe { vmcs::setup_execution_controls(eptp)? };

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
        // SAFETY: `enable_and_test` entered VMX root, loaded the VMCS and
        // wrote host, guest and control state before this loop.
        let outcome = unsafe {
            vmcs::run_guest_once(&mut regs, launched, &mut host_fpu, &mut guest_fpu)?
        };
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

// ── Linux launcher: re-entrant VM context ──────────────────────────
//
// The Linux run loop is split into open() / run_slice() / close() so the
// caller can interleave other work between bounded slices instead of
// blocking until guest exit.
//
// vmx_enter_root/vmx_exit_root duplicate the VMXON + VMCS asm from
// `with_vmx_root_and_vmcs`, which the substrate-test path still uses.

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
/// (kept, never-freed) VMXON + VMCS region phys addrs. Same steps as
/// `with_vmx_root_and_vmcs` 1-5 + `vmcs_setup_then_inner`.
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

/// Which target a nested page fault at `gpa` hits (`cores` NPF breakdown).
fn npf_kind(sh: &VmDevices, guest_len: u64, gpa: u64) -> usize {
    use crate::microvm::cpu as c;
    if sh.pci.virtio_blk.bar0_in_range(gpa) { c::NPF_BLK }
    else if crate::microvm::devices::net_backend::bar0_in_range(gpa) { c::NPF_NET }
    else if crate::microvm::devices::gpu_backend::bar0_in_range(gpa) { c::NPF_GPU }
    else if sh.pci.virtio_input.bar0_in_range(gpa) { c::NPF_INPUT }
    else if sh.pci.virtio_9p.bar0_in_range(gpa) { c::NPF_P9 }
    else if sh.pci.virtio_blk_sqfs.bar0_in_range(gpa) { c::NPF_SQFS }
    else if sh.pci.virtio_snd.bar0_in_range(gpa) { c::NPF_SND }
    else if (lapic::LAPIC_BASE..lapic::LAPIC_BASE + lapic::LAPIC_SIZE).contains(&gpa) { c::NPF_LAPIC }
    else if (crate::microvm::devices::ioapic::IOAPIC_BASE
        ..crate::microvm::devices::ioapic::IOAPIC_BASE + crate::microvm::devices::ioapic::IOAPIC_SIZE)
        .contains(&gpa) { c::NPF_IOAPIC }
    else if gpa < guest_len { c::NPF_RAM }
    else { c::NPF_OTHER }
}

/// State shared by all vCPUs of one microvm. The guest address space (RAM,
/// EPT) is fixed at open and read without a lock; the device model is
/// behind `dev`. The BSP and every AP hold an `Arc` of it. Mirror of svm
/// `VmShared`.
pub struct VmShared {
    /// Shared handle to the active guest memory (owned by `guest_mem`'s
    /// `ACTIVE_GM`, freed at close); the off-vCPU net backend holds the same
    /// `&'static GuestMem`.
    guest_mem: &'static GuestMem,
    /// EPT PML4 phys — `close()` passes it to `ept::release` to free
    /// every demand-faulted 4 KB frame + the demand PT pages + the
    /// fixed tables.
    ept_pml4: u64,
    /// EPTP (PML4 phys | walk-length | mem-type) to VMWRITE into each
    /// vCPU's VMCS::EPT_POINTER. The AP reuses it (one shared address
    /// space) in `open_ap`'s `setup_execution_controls`.
    eptp: u64,
    /// Base of the contiguous boot-window allocation (pre-2 MB-
    /// align). `close()` frees `ept::boot_frames_for(guest_mem.len())`
    /// from here; the demand region is freed via `ept::release`.
    guest_raw_base: u64,
    /// The device model. A vCPU holds it around device work after an exit
    /// and around interrupt collection before an entry, never across
    /// VMRESUME or a yield. Lock order: `dev`, then the GPU or NET device lock.
    dev: spin::Mutex<VmDevices>,
}

/// The device model and host-tick bookkeeping of one microvm.
struct VmDevices {
    serial: SerialState,
    pci: crate::microvm::devices::PciBus,
    pic: crate::microvm::devices::pic8259::Pic8259,
    /// `ticks()` of the last virtio-gpu display config-change IRQ — rate-
    /// limits the resize round-trip (debounce).
    last_cfg_tick: u64,
    /// `ticks()` of the last NAT mapping reap.
    last_reap_tick: u64,
    /// i8253 channel 0; its output is IRQ0 on the PIC. See `pit8253`.
    pit: crate::microvm::devices::pit8253::Pit,
}

/// Per-vCPU state: its own VMXON region + VMCS (VMX root is per core, so each
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
    /// I/O + MSR bitmap frames of this vCPU's VMCS, freed after VMXOFF.
    bitmaps: vmcs::ExecBitmaps,
    regs: vmcs::GuestRegs,
    trace: ExitTrace,
    io_stats: IoStats,
    launched: bool,
    iter: u32,
    io_dropped: u32,
    /// Emulated MSR state (MTRR, SPEC_CTRL, MISC_ENABLE …) — see `vmx::msr`.
    msrs: super::msr::GuestMsrs,
    /// Event mid-vectoring at the last exit (IDT_VECTORING_INFO; re-injected
    /// next entry — only type 0/2). SDM §27.2.4. See the field doc on the SVM
    /// twin for the type-gate rationale.
    reinject: u64,
    /// Per-vCPU emulated local APIC. Inert while
    /// booting `nolapic`; drives the timer + IPIs once Linux enables it.
    lapic: LocalApic,
    /// Adaptive halt-poll window (µs), see `HALT_POLL_MIN_US`.
    halt_poll_us: u64,
    /// Interrupt-window exiting currently requested (mirrors the VMCS bit).
    irq_window: bool,
    /// Host/guest FPU (XSAVE) save areas — VMRESUME preserves neither.
    host_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,
    guest_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,
}

/// Persistent state of one Linux microvm across cooperative slices.
/// Core-agnostic: nothing here assumes which core it runs on. Composed of
/// the all-vCPU `VmShared` and this vCPU's `Vcpu`.
pub struct VmContext {
    shared: Arc<VmShared>,
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
            // Install as the active guest memory (out of VmShared); `gm` is now
            // the shared `&'static` handle, also reachable by the net backend.
            let gm = crate::microvm::devices::guest_mem::set_active(gm);
            write_host_state_with_current_rsp()?;
            vmcs::setup_guest_state(load.entry_rip)?;
            // SAFETY: in VMX root with the VMCS current; `eptp` is this VM's
            // own EPT from `alloc_guest_ram_and_ept`.
            let bitmaps = unsafe { vmcs::setup_execution_controls(eptp)? };

            let mut regs = vmcs::GuestRegs::default();
            regs.rsi = load.boot_params_phys;

            let mut serial = SerialState::new();
            if !inject.is_empty() {
                serial.inject(inject);
                kprintln!("[microvm] pre-injected {} bytes into UART RX FIFO", inject.len());
            }

            // virtio-net lives in net_backend (a static, out of VmShared) so the
            // off-vCPU backend can own it; re-arm it to power-on state per VM.
            crate::microvm::devices::net_backend::reset();
            crate::microvm::devices::gpu_backend::reset();

            Ok(VmContext {
                shared: Arc::new(VmShared {
                    guest_mem: gm,
                    ept_pml4,
                    eptp,
                    guest_raw_base,
                    dev: spin::Mutex::new(VmDevices {
                        serial,
                        pci: crate::microvm::devices::PciBus::new(),
                        pic: {
                            // The I/O APIC's ID follows the vCPUs' in the MP table.
                            let mut pic = crate::microvm::devices::pic8259::Pic8259::new();
                            pic.ioapic.set_id(crate::microvm::cpu::guest_vcpus());
                            pic
                        },
                        last_cfg_tick: 0,
                        last_reap_tick: 0,
                        pit: crate::microvm::devices::pit8253::Pit::new(),
                    }),
                }),
                vcpu: Vcpu {
                    apic_id: 0, // BSP
                    vmxon_phys,
                    vmcs_phys,
                    bitmaps,
                    regs,
                    trace: ExitTrace::new(),
                    io_stats: IoStats::new(),
                    launched: false,
                    iter: 0,
                    io_dropped: 0,
                    msrs: super::msr::GuestMsrs::new(),
                    reinject: 0,
                    lapic: LocalApic::new(0),
                    halt_poll_us: HALT_POLL_MIN_US,
                    irq_window: false,
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

    /// Build an AP vCPU that shares an already-open BSP's `VmShared`
    /// (guest SMP). Enters VMX root on this (the AP's) worker core — VMX root
    /// + VMCS are per-physical-core, so the AP allocates its own VMXON/VMCS
    /// here, configures a real-mode SIPI-entry guest state, and reuses the BSP's
    /// shared EPTP / device model. No guest RAM / EPT / device allocation
    /// (those belong to the BSP, freed last-one-out). `apic_id` is 1.. Mirror
    /// of svm `open_ap`.
    pub fn open_ap(sipi_vector: u8, apic_id: u8) -> Result<VmContext, &'static str> {
        let shared = AP_SHARED.lock().clone().ok_or("no open VM for an AP to join")?;
        let (vmxon_phys, vmcs_phys) = vmx_enter_root()?;

        // In VMX root now: VMXOFF on any error so the core never strands.
        let build = || -> Result<VmContext, &'static str> {
            // Fixed at BSP open: no lock needed.
            let eptp = shared.eptp;
            write_host_state_with_current_rsp()?;
            vmcs::setup_guest_state_ap(sipi_vector)?;
            // SAFETY: in VMX root with this AP's VMCS current; `eptp` is the
            // BSP's EPT for the same VM, live until the last vCPU exits.
            let bitmaps = unsafe { vmcs::setup_execution_controls(eptp)? };

            Ok(VmContext {
                shared,
                vcpu: Vcpu {
                    apic_id,
                    vmxon_phys,
                    vmcs_phys,
                    bitmaps,
                    regs: vmcs::GuestRegs::default(),
                    trace: ExitTrace::new(),
                    io_stats: IoStats::new(),
                    launched: false,
                    iter: 0,
                    io_dropped: 0,
                    msrs: super::msr::GuestMsrs::new(),
                    reinject: 0,
                    lapic: LocalApic::new(apic_id),
                    halt_poll_us: HALT_POLL_MIN_US,
                    irq_window: false,
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

    /// Offer this VM's shared state to AP vCPUs (`open_ap`). BSP only.
    pub fn publish_for_aps(&self) {
        *AP_SHARED.lock() = Some(self.shared.clone());
    }

    /// Absolute host TSC of the BSP vCPU's next guest LAPIC-timer tick — the
    /// hrtimer deadline the idle park waits on (KVM `apic_timer_fn` model), so
    /// the guest 1 kHz clock advances independent of VMRUN. `None` when no guest
    /// timer is armed → the park falls back to its safety cap.
    pub fn next_timer_deadline_tsc(&self) -> Option<u64> {
        let lapic = self.vcpu.lapic.next_timer_deadline_tsc();
        let dev = if self.vcpu.apic_id == 0 { self.device_deadline() } else { None };
        match (lapic, dev) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The BSP's device timers: PIT, GPU resume, a playing sound stream, read
    /// under `dev`. This runs on every entry, so it does not wait: while an
    /// AP holds the lock, the last reading stands in, and the next entry
    /// reads afresh.
    fn device_deadline(&self) -> Option<u64> {
        let Some(sh) = self.shared.dev.try_lock() else {
            let v = LAST_DEV_DEADLINE.load(Ordering::Relaxed);
            return (v != 0).then_some(v);
        };
        let mut dev = sh.pit.next_deadline_tsc();
        if let Some(t) = crate::microvm::devices::gpu_backend::resume_tsc() {
            dev = Some(dev.map_or(t, |d| d.min(t)));
        }
        // A playing sound stream is serviced every millisecond.
        if sh.pci.virtio_snd.playing() {
            let t = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 1000;
            dev = Some(dev.map_or(t, |d| d.min(t)));
        }
        LAST_DEV_DEADLINE.store(dev.unwrap_or(0), Ordering::Relaxed);
        dev
    }
}


/// Feed every device line and the PIT into the PIC (BSP: in PIC mode the
/// device lines are wired there). The caller holds `VmShared::dev`. Mirror
/// of the SVM backend.
fn collect_device_irqs(sh: &mut VmDevices, gm: &GuestMem) {
    if crate::microvm::devices::net_backend::take_irq() {
        sh.pic.pulse(10);
        crate::microvm::devices::nat::note_net_irq();
    }
    if crate::microvm::devices::gpu_backend::take_irq() { sh.pic.pulse(9); }
    // vblank: a paused controlq runs its next frame (virtio-gpu IRQ 9).
    if crate::microvm::devices::gpu_backend::take_resume(crate::interrupts::rdtsc())
        && crate::microvm::devices::gpu_backend::lock()
            .service_queues(0, gm)
    {
        sh.pic.pulse(9);
    }
    if sh.pci.virtio_snd.pump(gm) {
        let l = sh.pci.virtio_snd.irq_line();
        sh.pic.pulse(l);
    }
    if sh.pci.virtio_9p.drain_async_done(gm) {
        let l = sh.pci.virtio_9p.irq_line();
        sh.pic.pulse(l);
    }
    if sh.pci.virtio_input.drain_injected(gm) { sh.pic.pulse(12); }
    sh.pit.poll(&mut sh.pic);

    let now = crate::interrupts::ticks();
    // Live resize: disconnect, then reconnect after 100 ms; a new
    // cycle at most every 250 ms while the window is dragged.
    if crate::microvm::devices::gpu_backend::d4_pending()
        && crate::microvm::devices::gpu_backend::lock().tick_d4(now)
    {
        sh.pic.pulse(9);
    } else {
        let wid = crate::microvm::vm_window();
        if wid != 0
            && !crate::microvm::devices::gpu_backend::d4_pending()
            && crate::shade::surface::display_dirty_peek(wid)
            && now.wrapping_sub(sh.last_cfg_tick) >= 25
        {
            let _ = crate::shade::surface::take_display_dirty(wid);
            sh.last_cfg_tick = now;
            crate::microvm::devices::gpu_backend::lock().signal_display_change(now);
            sh.pic.pulse(9);
        }
    }
    // NAT mapping reaper: an idle scan, once per 10 ms at most.
    if now != sh.last_reap_tick {
        sh.last_reap_tick = now;
        crate::microvm::devices::nat::housekeep();
    }
}

/// `kvm_cpu_has_interrupt`: the PIC's INTR if this vCPU takes it (LINT0 in
/// ExtINT, or no LAPIC), else a LAPIC vector above PPR. `dev` is the locked
/// device model on the BSP (the PIC is wired there), `None` on an AP.
fn pending_interrupt(vcpu: &mut Vcpu, dev: Option<&VmDevices>) -> Option<bool> {
    let lapic_on = vmx_lapic_on();
    if dev.is_some_and(|sh| sh.pic.output())
        && (!lapic_on || vcpu.lapic.accept_pic_intr())
    {
        return Some(true);
    }
    if lapic_on && vcpu.lapic.has_interrupt().is_some() {
        return Some(false);
    }
    None
}

impl Vcpu {
    /// Interrupt-window exiting (CPU-based control bit 2) — only VMWRITEs on a
    /// change.
    fn set_irq_window(&mut self, on: bool) -> Result<(), &'static str> {
        if self.irq_window != on {
            vmcs::set_interrupt_window_exiting(on)?;
            self.irq_window = on;
        }
        Ok(())
    }
}

impl VmContext {
    /// `inject_pending_event` (`vmx_inject_irq` / `vmx_enable_irq_window`),
    /// before every VM entry. An external interrupt may only be injected when
    /// the guest is interruptible — on Intel an IF=0 inject fails the entry
    /// (reason 33, SDM §26.3.1.5) — so otherwise the window is opened.
    fn inject_pending_event(&mut self) -> Result<(), &'static str> {
        // The BSP owns the device lines: collect them under `dev`.
        let mut dev = (self.vcpu.apic_id == 0).then(|| self.shared.dev.lock());
        if let Some(sh) = dev.as_deref_mut() { collect_device_irqs(sh, self.shared.guest_mem); }
        let vcpu = &mut self.vcpu;

        if vcpu.reinject != 0 {
            let rtype = (vcpu.reinject >> 8) & 0x7;
            if rtype != 0 || vmcs::guest_interruptible() {
                vmcs::write_entry_intr_info(vcpu.reinject)?;
                vcpu.reinject = 0;
                let more = pending_interrupt(vcpu, dev.as_deref()).is_some();
                vcpu.set_irq_window(more)?;
            } else {
                vcpu.set_irq_window(true)?;
            }
            return Ok(());
        }
        let Some(from_pic) = pending_interrupt(vcpu, dev.as_deref()) else {
            return vcpu.set_irq_window(false);
        };
        let queued = vmcs::read_entry_intr_info()? & (1u64 << 31) != 0;
        if queued || !vmcs::guest_interruptible() {
            return vcpu.set_irq_window(true);
        }
        // `kvm_cpu_get_interrupt`: ExtINT first, then the LAPIC.
        let vector = match (from_pic, dev.as_deref_mut()) {
            (true, Some(sh)) => sh.pic.read_irq(),
            (true, None) => return vcpu.set_irq_window(false),
            (false, _) => match vcpu.lapic.has_interrupt() {
                Some(v) => { vcpu.lapic.ack_interrupt(v); v }
                None => return vcpu.set_irq_window(false),
            },
        };
        vmcs::inject_external_irq(vector)?;
        vcpu.set_irq_window(false)
    }

    /// Anything deliverable right now (takes `dev` + collects on the BSP).
    fn event_pending(&mut self) -> bool {
        let mut dev = (self.vcpu.apic_id == 0).then(|| self.shared.dev.lock());
        if let Some(sh) = dev.as_deref_mut() { collect_device_irqs(sh, self.shared.guest_mem); }
        self.vcpu.reinject != 0 || pending_interrupt(&mut self.vcpu, dev.as_deref()).is_some()
    }

    /// `kvm_vcpu_halt` with adaptive halt-polling. Mirror of the SVM backend.
    fn halt_poll(&mut self) -> bool {
        lapic::phase(self.vcpu.apic_id, lapic::PH_HALTPOLL);
        if self.event_pending() {
            return true;
        }
        let cid = crate::smp::per_core::current_core_id();
        let kick_gen = crate::smp::fiber::net_kick_gen(cid);
        let deadline = self.next_timer_deadline_tsc();
        let start = crate::interrupts::rdtsc();
        let budget = (crate::interrupts::tsc_freq() / 1_000_000) * self.vcpu.halt_poll_us;
        loop {
            let now = crate::interrupts::rdtsc();
            let hit = crate::smp::fiber::net_kick_gen(cid) != kick_gen
                || lapic::posted_pending(self.vcpu.apic_id)
                || (self.vcpu.apic_id == 0 && crate::microvm::devices::net_backend::irq_pending())
                || deadline.is_some_and(|d| now >= d);
            if hit {
                self.vcpu.halt_poll_us = (self.vcpu.halt_poll_us * 2).min(HALT_POLL_MAX_US);
                return true;
            }
            if now.wrapping_sub(start) >= budget {
                break;
            }
            core::hint::spin_loop();
        }
        self.vcpu.halt_poll_us = (self.vcpu.halt_poll_us / 2).max(HALT_POLL_MIN_US);
        self.event_pending()
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
    pub fn close(&mut self) {
        // Stop the off-vCPU net backend first (it holds &'static GuestMem via
        // guest_mem::active(), freed by clear_active() below). Idempotent.
        let workers_gone = crate::microvm::devices::net_dataplane::stop_worker();
        // Persist the home image before teardown: the one save, see the svm
        // mirror.
        self.shared.dev.lock().pci.virtio_blk.save();
        // SAFETY: this vCPU entered VMX root on this core → VMXOFF is valid.
        unsafe { vmx_exit_root(); }
        if !workers_gone {
            // See the svm mirror: keep guest memory under a live reader.
            crate::kprintln!("[microvm] a device worker did not stop — guest memory is kept, not freed");
            memory::deallocate_frame(self.vcpu.vmcs_phys);
            memory::deallocate_frame(self.vcpu.vmxon_phys);
            self.vcpu.bitmaps.free();
            return;
        }
        // Demand-faulted frames + demand PTs + EPT tables.
        ept::release(self.shared.ept_pml4, self.shared.guest_mem.len());
        // Contiguous boot window.
        memory::deallocate_contiguous(
            self.shared.guest_raw_base,
            ept::boot_frames_for(self.shared.guest_mem.len()),
        );
        memory::deallocate_frame(self.vcpu.vmcs_phys);
        memory::deallocate_frame(self.vcpu.vmxon_phys);
        self.vcpu.bitmaps.free();
        // Free the active GuestMem (held outside VmShared); page tables are
        // released above and all vCPUs + the net backend have stopped.
        crate::microvm::devices::guest_mem::clear_active();
    }

    /// Tear down only this AP vCPU's VMX root (VMXOFF on its core + free its
    /// VMXON/VMCS frames). Does not touch the shared state — the BSP owns +
    /// frees that once the last vCPU has exited. VMX-specific: unlike SVM
    /// (where the AP just stops VMRUNning), the AP entered VMX root via
    /// `vmx_enter_root`, so it must VMXOFF on its own core.
    pub fn close_ap(&mut self) {
        // SAFETY: this AP entered VMX root on this core → VMXOFF is valid.
        unsafe { vmx_exit_root(); }
        memory::deallocate_frame(self.vcpu.vmcs_phys);
        memory::deallocate_frame(self.vcpu.vmxon_phys);
        self.vcpu.bitmaps.free();
    }
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
    /// guest shut itself down (PID-1 halt). Drives
    /// the auto-close-and-save so the user doesn't need a second Mod+Q.
    halt_observed: bool,
    /// RX FIFO. Bytes pre-injected by the host before
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
        crate::microvm::boottime::guest_byte();
        if byte == b'\n' || self.line_n == self.line.len() {
            let n = self.line_n;
            self.scan_for_panic(n);
            self.scan_for_shutdown(n);
            let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
            crate::microvm::boottime::guest_line(s);
            self.line_n = 0;
            return;
        }
        if byte != b'\r' {
            self.line[self.line_n] = byte;
            self.line_n += 1;
        }
    }

    fn flush(&mut self) {
        if self.line_n > 0 {
            let n = self.line_n;
            self.scan_for_panic(n);
            self.scan_for_shutdown(n);
            let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
            crate::microvm::boottime::guest_line(s);
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
/// entry. EPT identity-shifts guest-phys X → host-phys host_base+X,
/// so we just offset.
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
    /// `Ok(StillRunning)` = budget hit, re-enterable;
    /// `Ok(Exited(o))` = guest left; `Err` = setup/VM fault.
    pub fn run_slice(&mut self, budget: u32) -> Result<SliceOutcome, &'static str> {
    use crate::kprintln;

    const MAX_ITERATIONS: u32 = 100_000;

    let mut last_outcome: Option<vmcs::LaunchOutcome> = None;
    let mut slice_n: u32 = 0;

    // Wall-clock slice cap: return to the fiber scheduler every few ms so the
    // core's other fibers run; the exit budget bounds boot bursts.
    const SLICE_MS: u64 = 3;
    let slice_deadline = crate::interrupts::rdtsc()
        + (crate::interrupts::tsc_freq() / 1000) * SLICE_MS;

    // Host-time profiler: `prof_post` = TSC right after the previous VMRESUME,
    // `prof_bucket` = that exit's bucket; the gap to the next VMRESUME is the
    // handler cost.
    let mut prof_post: u64 = 0;
    let mut prof_bucket: usize = crate::microvm::cpu::VMX_OTHER;

    // Publish this vCPU's host core so a sender can kick it.
    let host_core = crate::smp::per_core::current_core_id();
    lapic::set_host_core(self.vcpu.apic_id, host_core);

    while self.vcpu.iter < MAX_ITERATIONS || crate::microvm::vm_window() != 0 {
        if slice_n >= budget || crate::interrupts::rdtsc() >= slice_deadline {
            return Ok(SliceOutcome::StillRunning);
        }
        self.vcpu.iter = self.vcpu.iter.saturating_add(1);
        slice_n += 1;

        // The IA-32e-mode-guest entry control must match GUEST_IA32_EFER.LMA.
        if self.vcpu.launched {
            vmcs::sync_entry_ia32e_with_efer()?;
        }
        let kick_gen = crate::smp::fiber::net_kick_gen(host_core);
        lapic::phase(self.vcpu.apic_id, lapic::PH_INJECT);
        self.inject_pending_event()?;
        self.vcpu.lapic.pv_eoi_sync_to(self.shared.guest_mem);

        // The guest's next timer, or the slice end, as a host one-shot on this
        // core: its fire is the exit that delivers the tick on time.
        let entry_deadline =
            self.next_timer_deadline_tsc().map_or(slice_deadline, |d| d.min(slice_deadline));
        // IF stays 0 into VMRESUME (external-interrupt exiting exits on a
        // pending interrupt regardless); the asm sets it again after the exit.
        if !crate::microvm::cpu::entry_irqs_off(kick_gen, host_core, entry_deadline) {
            continue;
        }
        // The host↔guest FPU swap is inside run_guest_once's asm,
        // bracketing VMRESUME with no compiler-emittable code between
        // xrstor and vmresume (a +avx2 kernel can spill `vmovups ymm`
        // anywhere between a Rust helper and the asm). The asm
        // save/restore mask is -1 (current-XCR0 components; guest XCR0 ⊇
        // host's via XSETBV pass-through).
        // Profiler: charge the time since the last exit to that exit's
        // handler bucket, then time the VMRESUME itself as guest cycles.
        let prof_pre = crate::interrupts::rdtsc();
        if prof_post != 0 {
            crate::microvm::cpu::record_exit_cycles(prof_bucket, prof_pre.wrapping_sub(prof_post));
        }
        let host_spec = super::msr::spec_ctrl_enter(self.vcpu.msrs.spec_ctrl);
        lapic::phase(self.vcpu.apic_id, lapic::PH_GUEST);
        // SAFETY: `open`/`open_ap` entered VMX root on this core, loaded this
        // vCPU's VMCS and wrote its host, guest and control state.
        let result = unsafe {
            vmcs::run_guest_once(
                &mut self.vcpu.regs, self.vcpu.launched,
                &mut self.vcpu.host_fpu, &mut self.vcpu.guest_fpu,
            )
        };
        lapic::phase(self.vcpu.apic_id, lapic::PH_EXIT);
        super::msr::spec_ctrl_exit(host_spec);
        prof_post = crate::interrupts::rdtsc();
        crate::microvm::cpu::record_guest_cycles(prof_post.wrapping_sub(prof_pre));
        let outcome = result?;
        self.vcpu.launched = true;
        self.vcpu.lapic.pv_eoi_sync_from(self.shared.guest_mem);
        // Diagnostic: snapshot the injection field that was live for the
        // entry we just ran, before the clear below. On a VM-entry failure
        // (reason 33) the event is never delivered, so this still holds
        // what we wrote.
        let entry_intr_used = vmcs::read_entry_intr_info().unwrap_or(0xDEAD);
        // Consume the VM_ENTRY_INTR_INFO slot. On bare-metal VMX the
        // CPU auto-clears the valid bit after a successful injection
        // (SDM §27.6), but under nested VMX (KVM emulating VMX) it
        // may leave the bit set — the next VMRESUME would then
        // re-inject the same event as a phantom duplicate interrupt.
        // KVM clears the field on every exit for exactly this; do the
        // same. The reinject/handler sites re-arm the slot for the next
        // VMRESUME as needed.
        let _ = vmcs::write_entry_intr_info(0);
        // Did an event abort mid-vectoring through the guest IDT?
        // IDT_VECTORING_INFO has the same encoding as VM_ENTRY_INTR_
        // INFO; stash it verbatim for re-injection on the next entry.
        // Type-gate to external IRQ (0) and NMI (2) only — exceptions
        // (type 3, e.g. #PF / #GP) and software ints (type 4) must not
        // be re-injected: the faulting RIP was not advanced (e.g. an
        // EPT violation doesn't retire the access), so the guest
        // re-executes the instruction and the exception re-occurs
        // naturally; re-injecting too = double delivery. Same type-gate
        // as the SVM backend.
        let idtv = vmcs::read_idt_vectoring_info().unwrap_or(0);
        if idtv & (1u64 << 31) != 0 {
            let idtv_type = (idtv >> 8) & 0x7;
            if idtv_type == 0 || idtv_type == 2 {
                self.vcpu.reinject = idtv;
            }
        }
        let basic = vmcs::basic_exit_reason(outcome.exit_reason);
        self.vcpu.trace.record(basic, outcome.exit_qualification);
        lapic::note_exit(self.vcpu.apic_id);

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

        // Diagnostic: per-exit mode trace for the first ~14 exits. Shows
        // when the guest enters long mode (CS.L / EFER.LMA flip 0→1) and
        // whether re-entry into long mode succeeds. Off by default: it
        // prints 14 lines per vCPU before the guest says anything.
        const ITER_TRACE: bool = false;
        if ITER_TRACE && self.vcpu.iter <= 14 {
            let efer = vmcs::read_guest_efer().unwrap_or(0);
            let cs_ar = vmcs::read_guest_cs_ar().unwrap_or(0);
            kprintln!(
                "[vmx-iter{}] #{} reason {} CS.L={} LMA={} entry_intr={:#x}",
                self.vcpu.apic_id, self.vcpu.iter, basic,
                (cs_ar >> 13) & 1, (efer >> 10) & 1, entry_intr_used,
            );
        }

        // Exits that touch only this vCPU take no device lock (see the SVM
        // run loop): the interrupt window, an MSR (x2APIC, PV-EOI), a hypercall.
        match basic {
            // A host interrupt (timer, device, kick IPI) pre-empted the guest;
            // the host took it on exit. Pending guest events inject at entry.
            1 => {
                last_outcome = Some(outcome);
                continue;
            }
            // Interrupt window open (`vmx_enable_irq_window`): the guest can
            // take the event now — the next entry injects it.
            7 => {
                self.vcpu.set_irq_window(false)?;
                last_outcome = Some(outcome);
                continue;
            }
            31 | 32 => {
                // RDMSR / WRMSR: every MSR outside the pass-through set is
                // intercepted and emulated in `vmx::msr`; none reaches the host.
                let msr = self.vcpu.regs.rcx as u32;
                let wval = (self.vcpu.regs.rdx << 32) | (self.vcpu.regs.rax & 0xFFFF_FFFF);
                let lapic_r = if vmx_lapic_on() {
                    self.vcpu.lapic.msr(msr, (basic == 32).then_some(wval))
                } else {
                    None
                };
                let ok = if let Some(r) = lapic_r {
                    // IA32_APIC_BASE, x2APIC registers, PV-EOI enable.
                    match r {
                        Ok((v, ipi)) => {
                            if let Some(icr) = ipi { lapic::deliver_ipi(self.vcpu.apic_id, &icr); }
                            if basic == 31 {
                                self.vcpu.regs.rax = v & 0xFFFF_FFFF;
                                self.vcpu.regs.rdx = v >> 32;
                            }
                            true
                        }
                        Err(()) => false,
                    }
                } else if basic == 32 {
                    let val = (self.vcpu.regs.rdx << 32) | (self.vcpu.regs.rax & 0xFFFF_FFFF);
                    super::msr::write(&mut self.vcpu.msrs, msr, val).is_ok()
                } else {
                    match super::msr::read(&self.vcpu.msrs, self.vcpu.apic_id, vmx_lapic_on(), msr) {
                        Ok(v) => {
                            self.vcpu.regs.rax = v & 0xFFFF_FFFF;
                            self.vcpu.regs.rdx = v >> 32;
                            true
                        }
                        Err(()) => false,
                    }
                };
                if ok {
                    vmcs::advance_guest_rip()?;
                } else {
                    vmcs::inject_exception(VEC_GP, Some(0))?;
                }
                last_outcome = Some(outcome);
                continue;
            }
            // VMCALL: KVM hypercall (`kvm_emulate_hypercall`), see the SVM arm.
            18 => {
                let r = &self.vcpu.regs;
                let ret = crate::microvm::cpu::kvm_hypercall(
                    self.vcpu.apic_id, vmcs::guest_cpl()?, r.rax, r.rbx, r.rcx, r.rdx, r.rsi);
                self.vcpu.regs.rax = ret as u64;
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
                continue;
            }
            _ => {}
        }

        // The device model is shared between vCPUs (guest SMP). Released at
        // the end of this iteration, or before the HLT path parks; never held
        // across run_guest_once or a yield.
        lapic::phase(self.vcpu.apic_id, lapic::PH_BIGLOCK);
        let mut dev = self.shared.dev.lock();
        lapic::phase(self.vcpu.apic_id, lapic::PH_EXIT);
        let sh = &mut *dev;
        let gm = self.shared.guest_mem;

        match basic {
            0 if (vmcs::read_exit_intr_info().unwrap_or(0) >> 8) & 0x7 == 2 => {
                // Host NMI while the guest ran (NMI exiting): the NMI is
                // consumed by the exit. Counted like the host handler does.
                crate::interrupts::NMI_COUNT.fetch_add(1, Ordering::Relaxed);
                last_outcome = Some(outcome);
            }
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
                    dump_page_walk(gm, cr3, outcome.exit_qualification);
                }
                self.vcpu.trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            12 => {
                // A headless test guest halting is done.
                if crate::microvm::vm_window() == 0 {
                    sh.serial.flush();
                    kprintln!("[vmx] guest HLT after {} VM-exits — exiting", self.vcpu.iter);
                    self.vcpu.io_stats.dump();
                    last_outcome = Some(outcome);
                    break;
                }
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
                drop(dev);
                // `kvm_vcpu_halt`: resume at once if something is deliverable,
                // else halt-poll, then block (the fiber parks until the next
                // timer deadline or a kick).
                if !self.halt_poll() {
                    return Ok(SliceOutcome::Idle);
                }
            }
            10 => {
                // CPUID — VMX always exits. Allowlist shared with SVM
                // (`cpu::guest_cpuid`, after KVM `kvm_cpu_cap_init`). XCR0
                // is the host's (7): XSETBV exits and never reaches hardware.
                let (eax, ebx, ecx, edx) = crate::microvm::cpu::guest_cpuid::guest_cpuid(
                    crate::microvm::cpu::guest_cpuid::Vendor::Intel,
                    self.vcpu.regs.rax as u32, self.vcpu.regs.rcx as u32, self.vcpu.apic_id,
                    vmcs::read_guest_cr4().unwrap_or(0),
                    crate::microvm::cpu::guest_cpuid::host_xcr0(),
                );
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
                handle_linux_io(&mut sh.serial, &mut sh.pci, &mut sh.pic, &mut self.vcpu.regs, port, dir_in, size, &mut self.vcpu.io_dropped, &mut sh.pit);
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            55 => {
                // XSETBV — VMX always exits, and the value never reaches
                // hardware: the host's XCR0 (x87|SSE|AVX) stays live, and
                // CPUID 0xD offers exactly that. Validate like KVM
                // `__kvm_set_xcr` so a bad value faults as on real hardware.
                let val = (self.vcpu.regs.rdx << 32) | (self.vcpu.regs.rax & 0xFFFF_FFFF);
                let host = crate::microvm::cpu::guest_cpuid::host_xcr0();
                let ok = self.vcpu.regs.rcx as u32 == 0
                    && val & 1 != 0 && val & !host == 0
                    && (val & 0b100 == 0 || val & 0b010 != 0);
                if ok {
                    vmcs::advance_guest_rip()?;
                } else {
                    vmcs::inject_exception(VEC_GP, Some(0))?;
                }
                last_outcome = Some(outcome);
            }
            // VMX, SMX, SGX and RSM: the guest has none of them (CPUID hides
            // VMX/SMX/SGX) → #UD, as KVM does without nesting. MONITOR/MWAIT
            // are hidden too.
            11 | 17 | 19..=27 | 36 | 39 | 50 | 53 | 59 | 60 => {
                vmcs::inject_exception(VEC_UD, None)?;
                last_outcome = Some(outcome);
            }
            // No vPMU: RDPMC faults.
            15 => {
                vmcs::inject_exception(VEC_GP, Some(0))?;
                last_outcome = Some(outcome);
            }
            // INVD / WBINVD: no non-coherent DMA into the guest → no-ops
            // (KVM `kvm_emulate_wbinvd`, INVD treated as WBINVD).
            13 | 54 => {
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            48 => {
                // EPT violation — guest tried to access a guest-phys
                // address that is not mapped (or with insufficient EPT
                // permissions). Device BAR ranges are emulated; demand
                // RAM is faulted in; everything else dumps + bails.
                let gpa = vmcs::read_guest_phys_addr().unwrap_or(0);
                crate::microvm::cpu::record_npf(npf_kind(sh, gm.len(), gpa));
                // LAPIC MMIO page (0xFEE00000) → trap-and-emulate. Left
                // EPT-not-present in ept.rs when the LAPIC is emulated.
                // I/O APIC page (0xFEC00000), EPT-not-present like the LAPIC's.
                {
                    use crate::microvm::devices::ioapic::{IOAPIC_BASE, IOAPIC_SIZE};
                    if (IOAPIC_BASE..IOAPIC_BASE + IOAPIC_SIZE).contains(&gpa)
                        && handle_mmio_ioapic(&mut self.vcpu.regs, &mut sh.pic, gpa, gm)
                    {
                        last_outcome = Some(outcome);
                        continue;
                    }
                }
                if vmx_lapic_on()
                    && (lapic::LAPIC_BASE..lapic::LAPIC_BASE + lapic::LAPIC_SIZE).contains(&gpa)
                {
                    if handle_mmio_lapic(&mut self.vcpu.regs, &mut self.vcpu.lapic, self.vcpu.apic_id, gpa, gm) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                }
                if sh.pci.virtio_blk.bar0_in_range(gpa) {
                    if handle_mmio_ept_blk(&mut self.vcpu.regs, &mut sh.pci.virtio_blk, &mut sh.pic, gpa, gm) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if crate::microvm::devices::net_backend::bar0_in_range(gpa) {
                    if handle_mmio_ept_net(&mut self.vcpu.regs, &mut sh.pic, gpa, gm) {
                        // Deliver a deferred device IRQ (esp. an async 9p
                        // write completion) now rather than at the next
                        // reason-1/12 exit.
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if crate::microvm::devices::gpu_backend::bar0_in_range(gpa) {
                    let mut gpu = crate::microvm::devices::gpu_backend::lock();
                    if handle_mmio_ept_gpu(&mut self.vcpu.regs, &mut *gpu, &mut sh.pic, gpa, gm) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_input.bar0_in_range(gpa) {
                    if handle_mmio_ept_input(&mut self.vcpu.regs, &mut sh.pci.virtio_input, &mut sh.pic, gpa, gm) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_9p.bar0_in_range(gpa) {
                    if handle_mmio_ept_p9(&mut self.vcpu.regs, &mut sh.pci.virtio_9p, &mut sh.pic, gpa, gm) {
                        // Deliver the freshly-completed 9p write-reply IRQ now
                        // (latched by drain_async_done at the loop top) instead
                        // of waiting for the next reason-1/12 exit.
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_blk_sqfs.bar0_in_range(gpa) {
                    // Same handler — VirtioBlk carries its own IRQ line.
                    if handle_mmio_ept_blk(&mut self.vcpu.regs, &mut sh.pci.virtio_blk_sqfs, &mut sh.pic, gpa, gm) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                } else if sh.pci.virtio_snd.bar0_in_range(gpa) {
                    if handle_mmio_ept_snd(&mut self.vcpu.regs, &mut sh.pci.virtio_snd, &mut sh.pic, gpa, gm) {
                        last_outcome = Some(outcome);
                        continue;
                    }
                }
                // Demand-paged guest RAM. A violation on a gpa
                // inside the advertised window but above the
                // contiguous boot block = first touch of a 4-KB
                // demand page → fault it in + re-enter. Ordering is
                // load-bearing: MMIO BAR ranges first (above),
                // RAM-demand here, fatal dump last.
                if gm.ensure(gpa) {
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
                // On Intel this typically means the IA-32e/CR/EFER triad is
                // inconsistent. SDM Vol 3 §26.3.1 enumerates the checks.
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
        self.shared.dev.lock().serial.flush();
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


    match last_outcome {
        Some(o) => Ok(SliceOutcome::Exited(o)),
        None => Err("Linux guest exceeded max iterations without first VM-exit"),
    }
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
    pit: &mut crate::microvm::devices::pit8253::Pit,
) {
    use crate::microvm::devices::{handle_pci_io, PCI_CONFIG_ADDR, PCI_CONFIG_DATA_END, PCI_CONFIG_DATA_START};
    use crate::microvm::devices::pic8259::{PIC_ELCR_MASTER, PIC_ELCR_SLAVE, PIC_MASTER_CMD, PIC_MASTER_IMR, PIC_SLAVE_CMD, PIC_SLAVE_IMR};

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
    if matches!(port, PIC_MASTER_CMD | PIC_MASTER_IMR | PIC_SLAVE_CMD | PIC_SLAVE_IMR
                    | PIC_ELCR_MASTER | PIC_ELCR_SLAVE) {
        if let Some(v) = pic.ioport(port, dir_in, val_out as u8) {
            regs.rax = (regs.rax & !mask) | (v & mask);
        }
        return;
    }

    match (port, dir_in) {
        // i8253 channel 0: mode/command (0x43) and the counter itself (0x40).
        // i8042: absent — all-ones, see the SVM `handle_linux_io`.
        (0x60 | 0x64, true) => {
            regs.rax = (regs.rax & !mask) | (0xFFu64 & mask);
        }
        (0x43, false) => pit.command(val_out as u8),
        (0x40, false) => pit.write_counter(val_out as u8),
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

/// EPT violation on the I/O APIC page → `devices::ioapic` (register window).
fn handle_mmio_ioapic(
    regs: &mut vmcs::GuestRegs,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
    gpa: u64,
    mem: &GuestMem,
) -> bool {
    use crate::kprintln;
    use crate::microvm::devices::guest_fetch::fetch_inst;
    use crate::microvm::devices::insn_decoder::{decode_mov, width_mask};

    let rip = match vmcs::read_guest_rip() { Ok(v) => v, Err(_) => return false };
    let cr3 = match vmcs::read_guest_cr3() { Ok(v) => v, Err(_) => return false };
    let Some(buf) = fetch_inst(rip, cr3, mem) else {
        kprintln!("[vmx] ioapic mmio: insn fetch failed (rip={:#x} gpa={:#x})", rip, gpa);
        return false;
    };
    let Some(dec) = decode_mov(&buf) else {
        kprintln!("[vmx] ioapic mmio: unsupported insn @ gpa={:#x} bytes={:02x?}", gpa, &buf[..8]);
        return false;
    };
    let off = (gpa - crate::microvm::devices::ioapic::IOAPIC_BASE) as u32;
    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        pic.ioapic.mmio(off, Some(value as u32));
    } else {
        let value = pic.ioapic.mmio(off, None).unwrap_or(0) as u64;
        write_gpr_vmx(regs, dec.reg, dec.width, value & width_mask(dec.width));
    }
    vmcs::advance_guest_rip_by(dec.length as u64).is_ok()
}

/// EPT violation on the LAPIC MMIO page. Decode the faulting MOV, service
/// it against the per-vCPU `LocalApic`, advance RIP. xAPIC registers are
/// 32-bit; no device IRQ kick. An ICR write returns the decoded IPI, which
/// `route_ipi` delivers cross-vCPU (INIT/SIPI bring-up + FIXED
/// reschedule/TLB). Mirrors svm `handle_mmio_npf_lapic`.
fn handle_mmio_lapic(
    regs: &mut vmcs::GuestRegs,
    apic: &mut LocalApic,
    apic_id: u8,
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
            lapic::deliver_ipi(apic_id, &icr);
        }
    } else {
        let value = apic.read(off) as u64;
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }
    if vmcs::advance_guest_rip_by(dec.length as u64).is_err() {
        return false;
    }
    true
}


/// Handle an EPT violation that targets virtio-blk's BAR0 MMIO range.
/// Walks the guest's page tables to fetch the faulting instruction
/// (VMX has no decode-assists), decodes the MOV form, emulates against
/// the device, advances RIP by the decoded instruction length.
///
/// Returns `true` if the fault was handled, `false` otherwise (page
/// walk failed, opcode unsupported).
fn handle_mmio_ept_blk(
    regs: &mut vmcs::GuestRegs,
    blk: &mut crate::microvm::devices::virtio_blk_pci::VirtioBlk,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
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
            pic.pulse(blk.irq_line());
        }
    }

    if vmcs::advance_guest_rip_by(dec.length as u64).is_err() {
        return false;
    }
    true
}

/// Read the decoded MOV register `reg` (see `DecodedMov::reg`).
fn read_gpr_vmx(regs: &vmcs::GuestRegs, reg: u8) -> u64 {
    use crate::microvm::devices::insn_decoder::{gpr_index, read_reg};
    let full = match gpr_index(reg) {
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
    };
    read_reg(reg, full)
}

/// Write the decoded MOV register `reg`, honouring x86 width rules.
fn write_gpr_vmx(regs: &mut vmcs::GuestRegs, reg: u8, width: u8, value: u64) {
    use crate::microvm::devices::insn_decoder::{gpr_index, merge_operand};
    let m = |old: u64| merge_operand(reg, old, value, width);
    match gpr_index(reg) {
        0  => regs.rax = m(regs.rax),
        1  => regs.rcx = m(regs.rcx),
        2  => regs.rdx = m(regs.rdx),
        3  => regs.rbx = m(regs.rbx),
        4  => {} // RSP — silently drop
        5  => regs.rbp = m(regs.rbp),
        6  => regs.rsi = m(regs.rsi),
        7  => regs.rdi = m(regs.rdi),
        8  => regs.r8  = m(regs.r8),
        9  => regs.r9  = m(regs.r9),
        10 => regs.r10 = m(regs.r10),
        11 => regs.r11 = m(regs.r11),
        12 => regs.r12 = m(regs.r12),
        13 => regs.r13 = m(regs.r13),
        14 => regs.r14 = m(regs.r14),
        15 => regs.r15 = m(regs.r15),
        _  => {}
    }
}

/// Handle an EPT violation that targets virtio-net's BAR0. Identical
/// pattern to `handle_mmio_ept_blk` — only the device + IRQ line
/// differ.
fn handle_mmio_ept_net(
    regs: &mut vmcs::GuestRegs,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
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

    let off = (gpa - crate::microvm::devices::virtio_net_dev::BAR0_BASE) as u32;

    // ISR read and TX doorbell: lock-free (the worker may hold the device).
    if let Some(v) = crate::microvm::devices::net_backend::mmio_fast(off, dec.is_write) {
        if !dec.is_write {
            write_gpr_vmx(regs, dec.reg, dec.width, v & width_mask(dec.width));
        }
        return vmcs::advance_guest_rip_by(dec.length as u64).is_ok();
    }

    let mut net = crate::microvm::devices::net_backend::lock();
    if dec.is_write {
        let value = read_gpr_vmx(regs, dec.reg) & width_mask(dec.width);
        net.mmio_write(off, dec.width, value);
    } else {
        let value = net.mmio_read(off, dec.width);
        write_gpr_vmx(regs, dec.reg, dec.width, value);
    }

    if let Some(qidx) = net.take_pending_kick() {
        if crate::microvm::devices::net_backend::full_active() && qidx == 1 {
            // The worker owns the guest TX ring. Ring its doorbell instead of
            // draining the ring here: two consumers on one virtqueue corrupt it.
            crate::microvm::devices::net_backend::note_tx_kick();
        } else {
            let advanced = net.service_queues(qidx, mem);
            // `service_queues` may complete TX and inject RX replies in one
            // go; under MSI-X both queues' vectors fire (a spurious one is
            // harmless: the guest finds nothing new), under INTx IRQ 10.
            if advanced && !(crate::microvm::devices::net_backend::msix_notify(0)
                | crate::microvm::devices::net_backend::msix_notify(1))
            {
                // virtio-net IRQ line = 10 (per pci config 0x3C).
                pic.pulse(10);
            }
        }
    }

    if vmcs::advance_guest_rip_by(dec.length as u64).is_err() { return false; }
    true
}


/// Handle EPT-trap on virtio-gpu BAR0. Mirror of `handle_mmio_ept_net`
/// — only the device and IRQ line differ.
fn handle_mmio_ept_gpu(
    regs: &mut vmcs::GuestRegs,
    gpu: &mut crate::microvm::devices::virtio_gpu_pci::VirtioGpu,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
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
        if crate::microvm::devices::gpu_backend::full_active() {
            // Off-vCPU: defer the heavy copy + write_frame to the GPU worker.
            crate::microvm::devices::gpu_backend::note_gpu_kick(qidx);
        } else {
            let advanced = gpu.service_queues(qidx, mem);
            if advanced {
                // virtio-gpu IRQ line = 9.
                pic.pulse(9);
            }
        }
    }

    if vmcs::advance_guest_rip_by(dec.length as u64).is_err() { return false; }
    true
}

/// Handle EPT-trap on virtio-input BAR0. Mirror of `handle_mmio_ept_gpu`
/// — only the device + IRQ line differ.
fn handle_mmio_ept_input(
    regs: &mut vmcs::GuestRegs,
    input: &mut crate::microvm::devices::virtio_input_pci::VirtioInput,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
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
            pic.pulse(12);
        }
    }

    if vmcs::advance_guest_rip_by(dec.length as u64).is_err() { return false; }
    true
}

/// Handle an EPT violation on virtio-9p BAR0. Mirror of
/// `handle_mmio_ept_input` — only the device + IRQ line (6) differ.
fn handle_mmio_ept_p9(
    regs: &mut vmcs::GuestRegs,
    p9: &mut crate::microvm::devices::virtio_9p_pci::Virtio9p,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
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
            pic.pulse(p9.irq_line());
        }
    }

    if vmcs::advance_guest_rip_by(dec.length as u64).is_err() { return false; }
    true
}

/// Handle EPT-trap on virtio-snd BAR0. Mirror of `handle_mmio_ept_net` —
/// only the device + IRQ line (8) differ.
fn handle_mmio_ept_snd(
    regs: &mut vmcs::GuestRegs,
    snd: &mut crate::microvm::devices::virtio_snd_pci::VirtioSnd,
    pic: &mut crate::microvm::devices::pic8259::Pic8259,
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
            pic.pulse(snd.irq_line());
        }
    }

    if vmcs::advance_guest_rip_by(dec.length as u64).is_err() { return false; }
    true
}
