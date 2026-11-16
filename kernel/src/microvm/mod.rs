//! MicroVM subsystem.
//!
//! Hosts the per-app Linux MicroVM plumbing:
//!   * `cpu/`   — Intel VMX, AMD SVM and Arm EL2 backends, vendor dispatch
//!   * `linux/` — bzImage and arm64 Image loading, the guest device tree
//!
//! Public API is vendor-agnostic. Callers (`intent::microvm_*`)
//! invoke `microvm::vm_open(...)` / `run_substrate_test()`
//! etc.; dispatch to the matching backend (Intel VMX or AMD SVM)
//! happens in `cpu::*`.
//!
//! Layout:
//! ```text
//! microvm/
//! ├── mod.rs         — public API re-export
//! ├── cpu/
//! │   ├── mod.rs     — Vendor enum + dispatch
//! │   ├── vmx/       — Intel backend
//! │   └── svm/       — AMD backend
//! └── linux/
//!     ├── mod.rs
//!     └── bzimage.rs — Linux Boot Protocol loader
//! ```

pub mod app;
pub mod boottime;
pub mod cpu;
pub mod devices;
pub mod linux;
#[cfg(target_arch = "aarch64")]
pub mod selftest;

#[allow(unused_imports)] // LaunchOutcome is part of the public surface
pub use cpu::{
    guest_running, init, report, run_substrate_test, vm_active,
    output_of_window, output_window, take_outputs_changed, vm_bind_window, vm_close_for_window,
    vm_busy, vm_has_window, vm_open, vm_poll_slice, LaunchOutcome,
};
#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use cpu::{decode_io_exit_qualification, vm_core_serve};
