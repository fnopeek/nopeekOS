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
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
fn detect_vendor() -> Vendor {
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

/// True if a microvm is currently open (running across slices).
pub fn vm_active() -> bool {
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
        Some(ActiveVm::Vmx(ctx)) => match ctx.run_slice(SLICE_BUDGET) {
            Ok(vmx::SliceOutcome::StillRunning) => None,
            Ok(vmx::SliceOutcome::Exited(o)) => Some(Ok(o)),
            Err(e) => Some(Err(e)),
        },
        Some(ActiveVm::Svm(ctx)) => match ctx.run_slice(SLICE_BUDGET) {
            Ok(svm::SliceOutcome::StillRunning) => None,
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
