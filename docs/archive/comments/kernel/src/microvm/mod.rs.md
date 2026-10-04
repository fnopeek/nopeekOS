# `kernel/src/microvm/mod.rs` @ 5e0102684

## L1-23 · `pub mod cpu;`

```
//! MicroVM subsystem (Phase 12).
//!
//! Hosts the per-app Linux MicroVM plumbing:
//!   * `cpu/`   — Intel VMX + AMD SVM backends, vendor dispatch
//!   * `linux/` — bzImage / Boot Protocol parsing + loading
//!
//! Public API is vendor-agnostic. Callers (`intent::microvm_*`)
//! invoke `microvm::vm_open(...)` / `run_substrate_test()`
//! etc.; dispatch to the matching backend (Intel VMX or AMD SVM)
//! happens in `cpu::*`.
//!
//! Long-term shape (`docs/plan/MICROKERNEL_REFACTOR.md`):
//! `​``text
//! microvm/
//! ├── mod.rs         — public API re-export
//! ├── cpu/
//! │   ├── mod.rs     — Vendor enum + dispatch
//! │   ├── vmx/       — Intel backend
//! │   └── svm/       — AMD backend
//! └── linux/
//!     ├── mod.rs
//!     └── bzimage.rs — Linux Boot Protocol loader
//! `​``
```

## L29 · `#[allow(unused_imports)] // LaunchOutcome is part of the public surface`

```
// LaunchOutcome is part of the public surface
```

