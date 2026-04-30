//! VMX (Intel VT-x) bring-up — Phase 12 MicroVM substrate.
//!
//! Layered as `kernel-side primitives only` per
//! `MICROKERNEL_REFACTOR.md` and `PHASE12_MICROVM.md`:
//! kernel owns VMX/VMCS/EPT/VT-d/VCPU-threads, WASM-Manager owns
//! lifecycle + bridges.
//!
//! Phase 12.1 milestones:
//!   12.1.0a  probe + report                           ← this file
//!   12.1.0b  VMXON region + VMX-root-mode entry
//!   12.1.0c  VMCS skeleton + Mini-Guest hlt; round-trip
//!   12.1.1   EPT + Linux 6.18 LTS bzImage to early-panic
//!   12.1.2   virtio-console backend
//!   12.1.3   initramfs + Rust-PID-1 + bash
//!   12.1.4   inject_console round-trip

mod probe;

use probe::probe;

/// One-shot bring-up: log VMX capabilities once at boot. Side-effect-
/// free (read-side only); 12.1.0b will add CR4.VMXE + VMXON.
pub fn init() {
    report();
}

/// Print VMX capability snapshot. Used by both `init()` at boot and the
/// `vmx` shell-intent on demand.
pub fn report() {
    use crate::kprintln;
    match probe() {
        Some(c) => {
            kprintln!("[vmx] VT-x supported");
            kprintln!("[vmx]   revision_id     = {:#010x}", c.revision_id);
            kprintln!("[vmx]   vmxon_region_sz = {} bytes", c.vmxon_region_size);
            kprintln!("[vmx]   ept_supported   = {}", c.ept_supported);
            kprintln!("[vmx]   unrestricted    = {}", c.unrestricted_guest);
            kprintln!("[vmx]   vpid            = {}", c.vpid);
            kprintln!("[vmx]   bring-up        = not yet (12.1.0a probe-only)");
        }
        None => {
            kprintln!("[vmx] VT-x NOT available — MicroVM disabled this boot");
            kprintln!("[vmx]   check BIOS: 'Intel Virtualization Technology' must be enabled");
        }
    }
}
