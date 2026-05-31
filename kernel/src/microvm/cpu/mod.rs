//! CPU virtualization extensions — vendor dispatch.
//!
//! Detects the host CPU vendor at boot via CPUID leaf 0 (vendor
//! string) and dispatches MicroVM operations to the matching
//! backend:
//!
//!   * `vmx` — Intel VT-x (VMCS, EPT)
//!   * `svm` — AMD-V (VMCB, NPT)  — stub, returns Err for now
//!
//! Public API (`init`, `report`, `run_substrate_test`, `run_linux`,
//! `decode_io_exit_qualification`) is re-exported one level up at
//! `crate::microvm` so callers stay vendor-agnostic.
//!
//! ## Why dispatch-enum, not a Hypervisor trait
//!
//! The two backends share no concrete code paths: VMX uses VMCS
//! reads/writes, SVM mutates a VMCB struct directly; VMX uses EPT,
//! SVM uses NPT; exit reasons / I/O bitmaps / control registers all
//! differ in encoding. A trait pulled across that boundary would be
//! method-by-method passthrough with vendor-specific Output types,
//! providing zero shared implementation. Once both backends ship
//! and we can see what actually generalizes (likely guest-RAM
//! window setup + Linux loader integration), a real trait can be
//! lifted from the convergent code. For now: simple match.

pub mod svm;
pub mod vmx;

use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

// ── Guest/host FPU (XSAVE) swap ────────────────────────────────────
//
// `vmrun`/VMRESUME do NOT save or restore x87/SSE/AVX/AVX-512 — host
// and guest share the one physical vector-register file. Any host FPU
// use between two guest entries (nat::pump memcpy/checksum, virtio
// buffer copies, kprintln, the cooperative Shade/fontdue pass)
// silently corrupts the guest's live vector state. musl + librewolf
// use AVX-512 pervasively (memcpy/strlen); corruption mid signal
// restore → the `ret` after rt_sigprocmask faults → SIGSEGV. Rate ∝
// VMRUN/s, so it bites the busy dedicated core hard and the mostly-
// HLT-idle cooperative one rarely. KVM swaps unconditionally
// (kvm_load_guest_fpu / kvm_put_guest_fpu); we do the same.

/// 64-byte-aligned XSAVE area. 4 KiB ≫ the ~2.4 KiB the host XCR0
/// (incl AVX-512) needs (CPUID.0xD.0:EBX). Zeroed = XSTATE_BV/XCOMP_BV
/// 0 → `xrstor64` loads the architectural FPU *init* state (x87 init,
/// MXCSR 0x1F80, vectors zeroed) — the correct fresh-guest FPU.
#[repr(C, align(64))]
pub(crate) struct FpuArea(pub(crate) [u8; 4096]);

impl FpuArea {
    pub(crate) fn boxed() -> alloc::boxed::Box<FpuArea> {
        alloc::boxed::Box::new(FpuArea([0u8; 4096]))
    }
}

// We do NOT touch the XCR0 register. XSETBV is not intercepted, so the
// guest's Linux owns XCR0 exactly as it did before this swap existed
// (it boots fine that way — managing XCR0 ourselves only ever caused
// regressions: a forced host-mask vs the guest's CPUID-0xD-masked set
// → fpu__init_system_xstate panic; a forced reset-mask while our
// +avx2-built host code runs → #UD on `vmovups ymm` → KVM emulation
// failure). Mask = -1: xsave64/xrstor64 then operate on every
// component enabled in the *current* XCR0. Guest XCR0 ⊇ host XCR0
// (guest Linux enables ≥ x87+SSE+AVX = our host's set; any extra
// AVX-512 bits it adds are still covered by -1), so host save/restore
// under the guest's XCR0 preserves the host's subset, and guest
// save/restore under it covers all guest state.

/// `xsave64 [area]`, all XCR0-enabled components (EDX:EAX = -1).
#[inline(never)]
pub(crate) unsafe fn fpu_xsave(area: *mut FpuArea) {
    // SAFETY: CR4.OSXSAVE=1 on every core (trampoline.s — blake3 AVX2
    // runs AP-side); area is valid + 64-aligned.
    unsafe {
        core::arch::asm!("xsave64 [{p}]", p = in(reg) area,
            in("eax") 0xffff_ffffu32, in("edx") 0xffff_ffffu32,
            options(nostack));
    }
}

/// `xrstor64 [area]`, all XCR0-enabled components. Zeroed area =
/// XSTATE_BV/XCOMP_BV 0 → architectural FPU init state (fresh guest).
#[inline(never)]
pub(crate) unsafe fn fpu_xrstor(area: *const FpuArea) {
    // SAFETY: see fpu_xsave; area is a valid XSAVE image or zeroed.
    unsafe {
        core::arch::asm!("xrstor64 [{p}]", p = in(reg) area,
            in("eax") 0xffff_ffffu32, in("edx") 0xffff_ffffu32,
            options(nostack, readonly));
    }
}

/// Guest-RAM size for the next VM, chosen at `vm_open` from live host
/// free memory instead of a fixed 1 GiB constant.
///
/// B2: the window is still one contiguous, single-PD ≤ 1 GiB block,
/// so advertised == committed == this value. B3 decouples them — the
/// guest will be *advertised* a generous size (demand-paged, scattered)
/// while only touched pages are *committed*, bounded by host free RAM.
///
/// Policy: take host free RAM minus a host reserve, clamp to
/// [`MIN`, cap], floor to the 2-MB EPT/NPT leaf granularity. On a fat
/// host (≥ ~1.3 GB free) this yields exactly the cap (= the validated
/// 1 GiB), so behaviour is unchanged where it was validated; it only
/// shrinks on a genuinely RAM-starved host instead of OOM-failing
/// `allocate_contiguous`.
pub fn choose_guest_ram_bytes() -> u64 {
    const RESERVE_MB: usize = 256;
    const MIN_MB: usize = 256;
    let cap_mb =
        (crate::microvm::devices::guest_mem::GUEST_RAM_BYTES / (1024 * 1024)) as usize;

    let (_free_frames, free_mb) = crate::mm::memory::stats();
    let mb = free_mb
        .saturating_sub(RESERVE_MB)
        .max(MIN_MB)
        .min(cap_mb);

    // Floor to 2 MB so it maps as whole EPT/NPT 2-MB leaves and the
    // frame count is a clean multiple of 512.
    const TWO_MB: u64 = 2 * 1024 * 1024;
    ((mb as u64) * 1024 * 1024) & !(TWO_MB - 1)
}

/// Host CPU vendor identified at boot from CPUID leaf 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Intel,
    Amd,
    /// CPUID returned a string we don't recognize. MicroVM stays
    /// disabled. The variant carries a short reason for `report()`.
    Unknown(&'static str),
}

static VENDOR: Mutex<Vendor> = Mutex::new(Vendor::Unknown("not detected yet"));

/// Identify the CPU via CPUID leaf 0 vendor string. Three known
/// strings: `GenuineIntel` (Intel), `AuthenticAMD` (AMD), anything
/// else returns `Unknown` with the raw bytes lost.
///
/// Standalone (no kernel state needed): safe to call from boot
/// init paths that run BEFORE `microvm::cpu::init()` has set the
/// cached `VENDOR` static. `smp::per_core::init_dedicated_vm_core`
/// uses this to vendor-gate A2 without writing its own CPUID
/// inline asm (the v0.172.62 attempt hung AMD QEMU).
pub fn detect_vendor() -> Vendor {
    let (_, ebx, ecx, edx) = vmx::host_cpuid(0, 0);
    // Vendor string is ebx, edx, ecx (yes, that order — Intel SDM
    // Vol. 2A §3.3 "CPUID Vendor String").
    let bytes = [
        (ebx & 0xFF) as u8, ((ebx >> 8) & 0xFF) as u8, ((ebx >> 16) & 0xFF) as u8, ((ebx >> 24) & 0xFF) as u8,
        (edx & 0xFF) as u8, ((edx >> 8) & 0xFF) as u8, ((edx >> 16) & 0xFF) as u8, ((edx >> 24) & 0xFF) as u8,
        (ecx & 0xFF) as u8, ((ecx >> 8) & 0xFF) as u8, ((ecx >> 16) & 0xFF) as u8, ((ecx >> 24) & 0xFF) as u8,
    ];
    match &bytes {
        b"GenuineIntel" => Vendor::Intel,
        b"AuthenticAMD" => Vendor::Amd,
        _ => Vendor::Unknown("CPUID vendor string not Intel/AMD"),
    }
}

#[allow(dead_code)] // public surface for future vendor-aware decoders
pub fn current_vendor() -> Vendor {
    *VENDOR.lock()
}

/// Boot-time entry: detect vendor, run vendor-specific probe.
pub fn init() {
    let v = detect_vendor();
    *VENDOR.lock() = v;
    match v {
        Vendor::Intel => vmx::init(),
        Vendor::Amd => svm::init(),
        Vendor::Unknown(reason) => {
            use crate::kprintln;
            kprintln!("[microvm] CPU vendor unknown ({}) — MicroVM disabled", reason);
        }
    }
}

/// Print vendor-specific virt capability snapshot.
pub fn report() {
    match *VENDOR.lock() {
        Vendor::Intel => vmx::report(),
        Vendor::Amd => svm::report(),
        Vendor::Unknown(reason) => {
            use crate::kprintln;
            kprintln!("[microvm] no virt extensions: {}", reason);
        }
    }
}

/// Run the vendor-specific substrate test (`microvm test`).
pub fn run_substrate_test() -> Result<LaunchOutcome, &'static str> {
    match *VENDOR.lock() {
        Vendor::Intel => vmx::run_substrate_test(),
        Vendor::Amd => svm::run_substrate_test(),
        Vendor::Unknown(reason) => Err(reason),
    }
}

/// Boot a Linux bzImage in the MicroVM (`microvm linux`).
pub fn run_linux(
    bzimage: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<LaunchOutcome, &'static str> {
    match *VENDOR.lock() {
        Vendor::Intel => vmx::run_linux(bzimage, cmdline, initramfs, inject),
        Vendor::Amd => svm::run_linux(bzimage, cmdline, initramfs, inject),
        Vendor::Unknown(reason) => Err(reason),
    }
}

// ── Re-entrant active VM (12.4 step 1b — Core-0 cooperative) ───────
//
// One backend-agnostic active VM, driven by the Core-0 event loop
// via `vm_poll_slice()` instead of a blocking `run_linux`. Holds the
// VmContext so Shade keeps rendering between bounded slices. Single
// global (one VM for now); keyed-registry generalisation deferred per
// the forward-compat contract (consumer side never assumes count).
// Core-0-only access in practice; the Mutex guards against misuse.

enum ActiveVm {
    Vmx(vmx::VmContext),
    Svm(svm::VmContext),
}

static ACTIVE_VM: Mutex<Option<ActiveVm>> = Mutex::new(None);

/// Shade window the active VM's framebuffer is bound to (0 = none).
/// virtio-gpu FLUSH reads this to know which surface to write; the
/// teardown path closes it. One VM ↔ one window for now; keyed by id
/// so it generalises (forward-compat #2).
static ACTIVE_VM_WINDOW: AtomicU32 = AtomicU32::new(0);

/// Set when the user closes the VM's window so the next slice tears
/// the guest down instead of running it headless.
static VM_CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Cross-boundary "open my files in loft" trigger: the guest's browser
/// opens the magic 9p file `<root>/.open-in-loft`, the 9p server (on the
/// VM core) sets this, and Core 0 reaps it in `vm_poll_slice` to spawn
/// loft (a compositor op that MUST run on Core 0). Mirrors the
/// VM_CLOSE_REQUESTED cross-core handoff.
static OPEN_LOFT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request that the host open loft. Safe to call from the VM core; the
/// spawn itself happens on Core 0 (`vm_poll_slice`).
pub fn request_open_loft() {
    OPEN_LOFT_REQUESTED.store(true, Ordering::Release);
}

// ── Dedicated-core path (substrate rework A2) ──────────────────────
//
// When `per_core::dedicated_vm_core()` is Some, the guest runs in a
// continuous loop on that worker core instead of cooperative Core-0
// slicing. VMXON / host-state capture / the run loop / VMXOFF must
// ALL execute on that one core — a cross-core open would restore
// Core 0's GDT/TR/RSP onto the VM core on the first VM-exit. So
// Core 0 only stashes a request (`PENDING_VM`); the dedicated core
// (`vm_core_serve`, driven from `smp_ap_entry`) owns the VmContext on
// its own stack for the VM's whole lifetime. Core 0 coordinates only
// via these atomics + the existing `ACTIVE_VM_WINDOW` /
// `VM_CLOSE_REQUESTED` — never the `ACTIVE_VM` mutex (cooperative
// path only), so it can't deadlock against the unbounded run loop.
// The cooperative path (≤2 cores) is byte-for-byte unchanged.

const VM_IDLE: u8 = 0;
const VM_REQUESTED: u8 = 1;
const VM_RUNNING: u8 = 2;
const VM_EXITED: u8 = 3;

/// Dedicated-core VM lifecycle. Only meaningful when a core is
/// dedicated (cooperative path uses `ACTIVE_VM` instead).
static VM_RUN_STATE: AtomicU8 = AtomicU8::new(VM_IDLE);

/// One pending launch request, owned copies so the caller's npkFS
/// buffers can drop while the dedicated core consumes them.
struct PendingVm {
    bzimage: alloc::vec::Vec<u8>,
    cmdline: alloc::vec::Vec<u8>,
    initramfs: Option<alloc::vec::Vec<u8>>,
    inject: alloc::vec::Vec<u8>,
}
static PENDING_VM: Mutex<Option<PendingVm>> = Mutex::new(None);

/// Bind the active VM to a Shade Surface window (called by the
/// microvm intent right after a successful `vm_open`).
pub fn vm_bind_window(window_id: u32) {
    ACTIVE_VM_WINDOW.store(window_id, Ordering::Release);
}

/// The Shade window the active VM renders into (0 = none/unbound).
pub fn vm_window() -> u32 {
    ACTIVE_VM_WINDOW.load(Ordering::Acquire)
}

/// Window closed by the user → ask the bound VM to power off on the
/// next slice. No-op if it isn't the active VM's window.
pub fn vm_close_for_window(window_id: u32) {
    if window_id != 0 && window_id == ACTIVE_VM_WINDOW.load(Ordering::Acquire) {
        VM_CLOSE_REQUESTED.store(true, Ordering::Release);
    }
}

/// Drop the VM↔window binding + its surface and close the Shade
/// window. MUST be called with the ACTIVE_VM lock NOT held (it locks
/// the compositor, whose close path re-enters microvm). Idempotent.
fn teardown_vm_window() {
    let wid = ACTIVE_VM_WINDOW.swap(0, Ordering::AcqRel);
    VM_CLOSE_REQUESTED.store(false, Ordering::Release);
    if wid != 0 {
        crate::shade::surface::remove_surface(wid);
        crate::shade::close_window(crate::shade::window::WindowId(wid));
    }
}

/// Exit-count cap per Core-0 poll, a secondary bound: `run_slice`
/// also enforces a ~3 ms wall-clock deadline (see vmx/svm
/// `SLICE_MS`), which is what actually keeps a busy guest from
/// starving Shade. Cheap boot exits hit this count first → same
/// boot wall-time; a busy compositor hits the deadline first.
const SLICE_BUDGET: u32 = 4096;

/// True if Core 0 should spin-feed a cooperative microvm. On the
/// dedicated path the guest runs on its own core, so Core 0 does NOT
/// spin — it idles/composites normally and reaps via `vm_poll_slice`.
/// Hence: false on the dedicated path (the guest is still composited
/// through the `focused_surface_id` branch, independent of this).
pub fn vm_active() -> bool {
    if crate::smp::per_core::dedicated_vm_core().is_some() {
        return false;
    }
    ACTIVE_VM.lock().is_some()
}

/// Open a microvm and register it as the active VM. Non-blocking:
/// does the (synchronous, one-time) substrate + guest-image setup,
/// then returns — slices run later via `vm_poll_slice`. Errors if a
/// VM is already active.
pub fn vm_open(
    bzimage: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<(), &'static str> {
    // Dedicated path: hand the request to the VM core (which opens it
    // on itself). Owned copies so the caller's npkFS Vecs can drop.
    if crate::smp::per_core::dedicated_vm_core().is_some() {
        if VM_RUN_STATE.load(Ordering::Acquire) != VM_IDLE {
            return Err("a microvm is already running");
        }
        *PENDING_VM.lock() = Some(PendingVm {
            bzimage: bzimage.to_vec(),
            cmdline: cmdline.to_vec(),
            initramfs: initramfs.map(<[u8]>::to_vec),
            inject: inject.to_vec(),
        });
        VM_CLOSE_REQUESTED.store(false, Ordering::Release);
        VM_RUN_STATE.store(VM_REQUESTED, Ordering::Release);
        return Ok(());
    }

    let mut slot = ACTIVE_VM.lock();
    if slot.is_some() {
        return Err("a microvm is already running");
    }
    let vm = match *VENDOR.lock() {
        Vendor::Intel => ActiveVm::Vmx(vmx::vm_open(bzimage, cmdline, initramfs, inject)?),
        Vendor::Amd => ActiveVm::Svm(svm::vm_open(bzimage, cmdline, initramfs, inject)?),
        Vendor::Unknown(reason) => return Err(reason),
    };
    *slot = Some(vm);
    VM_CLOSE_REQUESTED.store(false, Ordering::Release);
    Ok(())
}

/// Run one bounded slice of the active VM, if any. Called from the
/// Core-0 poll cadence (next to `net::poll`). Cheap no-op when no VM.
/// On guest exit / fault: log, free resources, clear the slot so a
/// new VM can be opened (relaunch).
pub fn vm_poll_slice() {
    // Cross-boundary trigger: the microvm browser asked (via 9p) to open
    // loft. Spawn it here on Core 0 (compositor op). Runs on both the
    // cooperative and dedicated paths since it's before the early return.
    if OPEN_LOFT_REQUESTED.swap(false, Ordering::AcqRel) {
        crate::shade::launch_app("loft");
    }

    // Dedicated path: Core 0 is only the reaper. The VM core owns the
    // VmContext for its whole lifetime and does its own VMXOFF; Core 0
    // just runs the compositor-locking teardown once it has exited
    // (teardown_vm_window must run on Core 0). Cheap atomic load on
    // the hot poll path when nothing has exited.
    if crate::smp::per_core::dedicated_vm_core().is_some() {
        if VM_RUN_STATE.load(Ordering::Acquire) == VM_EXITED {
            crate::microvm::devices::nat::reset_sessions();
            teardown_vm_window();
            VM_CLOSE_REQUESTED.store(false, Ordering::Release);
            VM_RUN_STATE.store(VM_IDLE, Ordering::Release);
            crate::kprintln!("[microvm] guest exited (dedicated core)");
        }
        return;
    }

    let mut slot = ACTIVE_VM.lock();
    if slot.is_none() {
        return;
    }

    // User closed the VM's window → force the guest down this tick
    // instead of running it headless until idle.
    if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
        match slot.as_mut() {
            Some(ActiveVm::Vmx(ctx)) => ctx.close(),
            Some(ActiveVm::Svm(ctx)) => ctx.close(),
            None => {}
        }
        *slot = None;
        drop(slot); // release before teardown — it locks the compositor
        crate::microvm::devices::nat::reset_sessions();
        teardown_vm_window();
        crate::kprintln!("[microvm] guest stopped (window closed)");
        return;
    }

    let finished: Option<Result<LaunchOutcome, &'static str>> = match slot.as_mut() {
        None => return,
        // Cooperative Core-0 path: Idle == StillRunning here — just hand
        // Core 0 back to the shell, whose own idle HLT throttles the loop.
        Some(ActiveVm::Vmx(ctx)) => match ctx.run_slice(SLICE_BUDGET) {
            Ok(vmx::SliceOutcome::StillRunning) | Ok(vmx::SliceOutcome::Idle) => None,
            Ok(vmx::SliceOutcome::Exited(o)) => Some(Ok(o)),
            Err(e) => Some(Err(e)),
        },
        Some(ActiveVm::Svm(ctx)) => match ctx.run_slice(SLICE_BUDGET) {
            Ok(svm::SliceOutcome::StillRunning) | Ok(svm::SliceOutcome::Idle) => None,
            Ok(svm::SliceOutcome::Exited(o)) => Some(Ok(o)),
            Err(e) => Some(Err(e)),
        },
    };
    let Some(result) = finished else { return };
    match slot.as_mut() {
        Some(ActiveVm::Vmx(ctx)) => ctx.close(),
        Some(ActiveVm::Svm(ctx)) => ctx.close(),
        None => {}
    }
    *slot = None;
    drop(slot); // release before teardown — it locks the compositor
    crate::microvm::devices::nat::reset_sessions();
    teardown_vm_window();
    match result {
        Ok(o) => crate::kprintln!(
            "[microvm] guest exited — final reason {:#x} qual {:#x}",
            (o.exit_reason & 0xFFFF) as u16, o.exit_qualification,
        ),
        Err(e) => crate::kprintln!("[microvm] launch FAILED: {}", e),
    }
}

/// Dedicated-core entry point — called every iteration of the
/// dedicated worker core's `smp_ap_entry` loop. Cheap no-op unless a
/// launch is pending. When one is, this opens the VM **on this core**
/// (so VMXON / `write_host_state` / VMPTRLD / VMRESUME / VMXOFF all
/// bind here, never Core 0), runs it to exit / window-close in a
/// continuous loop, closes it, and signals Core 0 to reap. Blocks the
/// dedicated core for the guest's whole lifetime — that is the point:
/// the guest no longer fights Shade + the shell for Core 0. Never
/// touches `ACTIVE_VM` (cooperative-path only).
pub fn vm_core_serve() {
    if VM_RUN_STATE.load(Ordering::Acquire) != VM_REQUESTED {
        return;
    }
    let pending = match PENDING_VM.lock().take() {
        Some(p) => p,
        None => {
            VM_RUN_STATE.store(VM_IDLE, Ordering::Release);
            return;
        }
    };
    VM_RUN_STATE.store(VM_RUNNING, Ordering::Release);

    // Per-core periodic VMEXIT source + host interrupts ON, so a
    // pending tick is delivered to the EOI handler between VMRUNs —
    // exactly how Core 0 runs the cooperative path. Without IF=1 the
    // first tick would never EOI and the guest would freeze one
    // iteration later; without the timer there is no tick at all.
    crate::interrupts::arm_dedicated_vm_timer();
    // SAFETY: ring-0; CLGI/STGI still brackets the VMRUN-critical
    // region inside run_guest_once. This mirrors Core 0's IF=1.
    unsafe { core::arch::asm!("sti") };

    // Continuous run: identical primitive to `vmx::run_linux` step 1a
    // (open + `loop run_slice` + close) plus a window-close check so
    // the user can `Mod+Q` the tile. `run_slice` returns periodically
    // on its wall-clock deadline; we loop straight back (no Shade
    // composite, no hlt on this core) → near-native guest. The two
    // backends have distinct `SliceOutcome` enums, so match each
    // concretely.
    match *VENDOR.lock() {
        Vendor::Intel => {
            match vmx::vm_open(
                &pending.bzimage,
                &pending.cmdline,
                pending.initramfs.as_deref(),
                &pending.inject,
            ) {
                Ok(mut ctx) => {
                    loop {
                        if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
                            crate::kprintln!("[microvm] window closed — stopping guest");
                            break;
                        }
                        match ctx.run_slice(SLICE_BUDGET) {
                            Ok(vmx::SliceOutcome::StillRunning) => continue,
                            Ok(vmx::SliceOutcome::Idle) => {
                                // Guest idle — host-idle until the next
                                // dedicated-VM-timer tick (~1 ms) instead of
                                // spinning VMRUN, so this host core actually
                                // sleeps while the guest has nothing to do.
                                // SAFETY: ring-0; the dedicated VM timer
                                // (armed in vm_core_serve) wakes us.
                                unsafe { core::arch::asm!("sti; hlt") };
                                continue;
                            }
                            Ok(vmx::SliceOutcome::Exited(o)) => {
                                crate::kprintln!(
                                    "[microvm] guest exited — reason {:#x} qual {:#x}",
                                    (o.exit_reason & 0xFFFF) as u16,
                                    o.exit_qualification
                                );
                                break;
                            }
                            Err(e) => {
                                crate::kprintln!("[microvm] run FAILED: {}", e);
                                break;
                            }
                        }
                    }
                    ctx.close();
                }
                Err(e) => crate::kprintln!("[microvm] open FAILED: {}", e),
            }
        }
        Vendor::Amd => {
            match svm::vm_open(
                &pending.bzimage,
                &pending.cmdline,
                pending.initramfs.as_deref(),
                &pending.inject,
            ) {
                Ok(mut ctx) => {
                    loop {
                        if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
                            crate::kprintln!("[microvm] window closed — stopping guest");
                            break;
                        }
                        match ctx.run_slice(SLICE_BUDGET) {
                            Ok(svm::SliceOutcome::StillRunning) => continue,
                            Ok(svm::SliceOutcome::Idle) => {
                                // Guest idle — host-idle until the next
                                // dedicated-VM-timer tick (~1 ms) instead of
                                // spinning VMRUN, so this host core actually
                                // sleeps while the guest has nothing to do.
                                // SAFETY: ring-0; the dedicated VM timer
                                // (armed in vm_core_serve) wakes us.
                                unsafe { core::arch::asm!("sti; hlt") };
                                continue;
                            }
                            Ok(svm::SliceOutcome::Exited(o)) => {
                                crate::kprintln!(
                                    "[microvm] guest exited — reason {:#x} qual {:#x}",
                                    (o.exit_reason & 0xFFFF) as u16,
                                    o.exit_qualification
                                );
                                break;
                            }
                            Err(e) => {
                                crate::kprintln!("[microvm] run FAILED: {}", e);
                                break;
                            }
                        }
                    }
                    ctx.close();
                }
                Err(e) => crate::kprintln!("[microvm] open FAILED: {}", e),
            }
        }
        Vendor::Unknown(reason) => crate::kprintln!("[microvm] {}", reason),
    }

    // VM done — swap the 1 kHz VM timer back to the 100 Hz worker idle
    // timer so this core's park-loop HLT keeps a wake source (it idles
    // like any worker until the next launch). Restore the IF=0 state
    // `smp_ap_entry`'s park loop expects (it does its own sti;hlt;cli).
    crate::interrupts::disarm_dedicated_vm_timer();
    crate::interrupts::arm_worker_timer();
    // SAFETY: ring-0; return the core to the parked-loop invariant.
    unsafe { core::arch::asm!("cli") };

    drop(pending); // owned guest-image buffers freed
    // Hand off to Core 0's reaper (compositor-locking teardown).
    VM_RUN_STATE.store(VM_EXITED, Ordering::Release);
}

/// Decode the I/O VM-exit qualification field from a substrate-test
/// `LaunchOutcome.exit_qualification`. Currently vendor-agnostic by
/// dispatch — only Intel populates I/O exits today; the AMD VMCB
/// EXITINFO1/2 layout will be plumbed through here when SVM lands.
pub fn decode_io_exit_qualification(qual: u64) -> (u16, bool, u8) {
    match *VENDOR.lock() {
        Vendor::Intel => vmx::decode_io_exit_qualification(qual),
        // AMD VMCB exitinfo1 layout differs (port in bits 16-31,
        // type in bit 0); plumb in svm:: when backend lands.
        Vendor::Amd | Vendor::Unknown(_) => (0, false, 0),
    }
}

/// Outcome of one VM-entry/exit cycle.
///
/// The numeric fields are vendor-specific in their meaning:
///   * Intel: `exit_reason` is the Intel basic exit reason
///     (SDM Vol. 3C App. C); `exit_qualification` is VMCS field
///     `VM_EXIT_QUALIFICATION`.
///   * AMD (future): `exit_reason` will be the VMCB EXITCODE;
///     `exit_qualification` will be a packed EXITINFO1/EXITINFO2.
///
/// Callers that decode reason values must currently dispatch on
/// `current_vendor()`. Once both backends ship we'll consider
/// hoisting a vendor-agnostic `ExitReason` enum here.
pub use vmx::LaunchOutcome;
