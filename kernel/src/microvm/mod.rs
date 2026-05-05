//! MicroVM subsystem (Phase 12).
//!
//! Hosts the per-app Linux MicroVM plumbing:
//!   * `cpu/`   — Intel VMX + AMD SVM backends, vendor dispatch
//!   * `linux/` — bzImage / Boot Protocol parsing + loading
//!
//! Public API is vendor-agnostic. Callers (`intent::microvm_*`)
//! invoke `microvm::run_linux(...)`, `microvm::run_substrate_test()`
//! etc.; dispatch to the matching backend (Intel VMX or AMD SVM)
//! happens in `cpu::*`.
//!
//! Long-term shape (`MICROKERNEL_REFACTOR.md`):
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

pub mod cpu;
pub mod devices;
pub mod linux;

#[allow(unused_imports)] // LaunchOutcome is part of the public surface
pub use cpu::{
    decode_io_exit_qualification, init, report, run_linux, run_substrate_test, LaunchOutcome,
};
