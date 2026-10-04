# `kernel/src/microvm/cpu/vmx/vmcs.rs` @ 5e0102684

## L1-25 · `use super::rdmsr;`

```
//! VMCS field setup — Phase 12.1.0d-1 / 12.1.0d-2b.
//!
//! Provides VMWRITE/VMREAD wrappers, the SDM Appendix-B field
//! encodings we need, and the host-state / guest-state / execution-
//! control / VMLAUNCH pipeline that runs after VMPTRLD inside VMX
//! root mode.
//!
//! 12.1.0d-1 (shipped, NUC-validated):
//!   - All HOST_* fields written + read back to validate the
//!     VMWRITE / VMREAD pipe and the host-state math.
//!
//! 12.1.0d-2b (this file, post-NUC fix v0.96.0):
//!   - Long-mode flat-segment guest with shared CR3 (no EPT). All
//!     GUEST_* fields written.
//!   - Pin/Proc/Entry/Exit execution controls computed via the
//!     allowed-0 / allowed-1 mask MSRs.
//!   - `launch_test()` overrides HOST_RIP/HOST_RSP just-in-time to a
//!     resume label inside its own asm! block, runs VMLAUNCH; the
//!     guest hits `hlt` (HLT-exiting=1), VM-exit fires, the CPU
//!     loads host state and lands at the resume label. We VMREAD
//!     VM_EXIT_REASON and return it.
//!
//! Reference: Intel SDM Vol. 3C §24 (Virtual-Machine Control
//! Structures), §26.2-§26.4 (Host/Guest State Checks, Loading on
//! VM Entry), §27 (VM Exits), Appendix B (Field Encoding in VMCS).
```

## L29-31 · `const HOST_ES_SELECTOR: u64 = 0x0C00;`

```
// ── VMCS field encodings (SDM Appendix B) ──────────────────────────
// Only the host-state set we touch in 12.1.0d-1 plus VM_EXIT_REASON
// for the trampoline.
```

## L33 · `const HOST_ES_SELECTOR: u64 = 0x0C00;`

```
// 16-bit host-state.
```

## L42 · `const HOST_IA32_EFER: u64 = 0x2C02;`

```
// 64-bit host-state.
```

## L46 · `const HOST_IA32_SYSENTER_CS: u64 = 0x4C00;`

```
// 32-bit host-state.
```

## L49 · `const HOST_CR0: u64 = 0x6C00;`

```
// Natural-width host-state.
```

## L63 · `const GUEST_ES_SELECTOR: u64 = 0x0800;`

```
// 16-bit guest-state.
```

## L73 · `const VMCS_LINK_POINTER: u64 = 0x2800;`

```
// 64-bit guest-state.
```

## L77 · `const PAT_RESET: u64 = 0x0007_0406_0007_0406;`

```
/// Architectural PAT reset value (SDM §13.12.4).
```

## L80 · `const GUEST_ES_LIMIT: u64 = 0x4800;`

```
// 32-bit guest-state.
```

## L103 · `const GUEST_CR0: u64 = 0x6800;`

```
// Natural-width guest-state.
```

## L125 · `const PIN_BASED_VM_EXEC_CONTROL: u64 = 0x4000;`

```
// Execution controls (32-bit).
```

## L139 · `const IO_BITMAP_A_FULL: u64 = 0x2000;`

```
// 64-bit control.
```

## L145 · `#[allow(dead_code)] // SDM Vol3 §28.3 field; reserved for fault-path decoding`

```
// Natural-width VM-exit info.
```

## L146 · `#[allow(dead_code)] // SDM Vol3 §28.3 field; reserved for fault-path decoding`

```
// SDM Vol3 §28.3 field; reserved for fault-path decoding
```

## L150 · `const VM_EXIT_GUEST_PHYS_ADDR: u64 = 0x2400;`

```
// 64-bit VM-exit info.
```

## L153 · `const CR0_GUEST_HOST_MASK: u64 = 0x6000;`

```
// Natural-width controls.
```

## L159 · `const VM_INSTRUCTION_ERROR: u64 = 0x4400;`

```
// VM-exit information (read-only).
```

## L166-167 · `const IA32_VMX_BASIC: u32 = 0x480;`

```
// VM_EXIT_REASON encoding 0x4402 is referenced as a literal inside
// the run_guest_once asm! block (Intel-syntax `mov rcx, 0x4402`).
```

## L169 · `const IA32_VMX_BASIC: u32 = 0x480;`

```
// VMX capability MSRs for control allowed-0 / allowed-1 masking.
```

## L176-183 · `const IA32_VMX_TRUE_PINBASED_CTLS: u32 = 0x48D;`

```
// "TRUE" variants exist when IA32_VMX_BASIC bit 55 = 1 (Alder Lake-N
// definitely has them). The TRUE MSRs relax "default-1" bits in the
// classic MSRs to "may-be-0", letting us actually clear bits like
// CR3-load-exiting (no point with EPT) and ACK_INTR_ON_EXIT. KVM uses
// these universally; we did not, which forced our control fields into
// a state that diverges from a minimal-VMX setup and from SVM
// semantics. Discovered while chasing the bare-metal-NUC reason-33
// VM-entry failure (v0.172.61+).
```

## L189-190 · `fn vmx_has_true_controls() -> bool {`

```
/// True if IA32_VMX_BASIC bit 55 reports TRUE-controls support.
/// Cached after first call to avoid repeated rdmsr.
```

## L193 · `static CACHED: AtomicU8 = AtomicU8::new(0xFF); // 0xFF = uncomputed`

```
// 0xFF = uncomputed
```

## L198 · `let basic = unsafe { rdmsr(IA32_VMX_BASIC) };`

```
// SAFETY: IA32_VMX_BASIC is architectural once CR4.VMXE supported.
```

## L205-207 · `fn pinbased_ctls_msr()   -> u32 { if vmx_has_true_controls() { IA32_VMX_TRUE_PINBASED_CTLS }   else { IA32_VMX_PINBASED_`

```
/// Pick the right control MSR — TRUE variant when supported, classic
/// when not. The TRUE variants relax default-1 bits so we can choose
/// to clear them (e.g. CR3-load-exiting with EPT enabled).
```

## L213 · `const IA32_EFER: u32 = 0xC000_0080;`

```
// Architectural MSRs we mirror into host-state.
```

## L224 · `pub(super) fn vmwrite(field: u64, value: u64) -> Result<(), &'static str> {`

```
// ── VMWRITE / VMREAD primitives ────────────────────────────────────
```

## L226-230 · `pub(super) fn vmwrite(field: u64, value: u64) -> Result<(), &'static str> {`

```
/// VMWRITE the given field with `value`. Caller must be in VMX root
/// mode with a current VMCS loaded (VMPTRLD'd).
///
/// On VMfailInvalid (CF=1) — no current VMCS — or VMfailValid (ZF=1)
/// — invalid field encoding for the loaded VMCS — returns `Err`.
```

## L233-235 · `unsafe {`

```
// SAFETY: VMWRITE has no architectural side effects beyond the
// VMCS state and RFLAGS. Caller-guaranteed VMX root mode +
// current VMCS. pushfq/pop touches the stack.
```

## L255-256 · `pub(super) fn vmread(field: u64) -> Result<u64, &'static str> {`

```
/// VMREAD the given field. Same VMX-root-mode + current-VMCS
/// preconditions as `vmwrite`.
```

## L260-261 · `unsafe {`

```
// SAFETY: as `vmwrite`. VMREAD writes only into the destination
// register and RFLAGS.
```

## L281-284 · `fn vmwrite_check(field: u64, value: u64, name: &'static str) -> Result<(), &'static str> {`

```
/// VMWRITE then VMREAD; assert the round-trip matches. Catches both
/// the VMWRITE/VMREAD path *and* the silent truncation cases (a few
/// host-state fields are 32-bit on the wire even though we pass
/// `u64`).
```

## L289-291 · `let _ = name;`

```
// Stash the field name so the caller can report which one
// tripped. Static-string identity is enough — we only debug
// by reading the source.
```

## L298 · `struct HostSnapshot {`

```
// ── Current host-CPU snapshot ──────────────────────────────────────
```

## L300-303 · `struct HostSnapshot {`

```
/// Read CR0/CR3/CR4 and the segment selectors we copy into the host
/// area. Done once before the VMWRITE storm so we get a consistent
/// snapshot — the trampoline never returns, so these values describe
/// "the kernel state we want to wake up in" if a VM-exit ever fires.
```

## L330 · `let mut gdtr_buf: [u8; 10] = [0; 10];`

```
// SDT/IDT pseudo-descriptors: 10 bytes (limit:2 + base:8) in long mode.
```

## L334-336 · `unsafe {`

```
// SAFETY: pure register reads, no faulting paths. `str` is legal
// in long mode regardless of whether a TSS was actually loaded
// (returns 0 if none). sgdt/sidt write 10 bytes to the operand.
```

## L368-372 · `fn resolve_tr_base(tr_selector: u16, gdtr_base: u64) -> u64 {`

```
/// Resolve the TSS base from the GDT entry that TR selects. In long
/// mode a TSS descriptor is 16 bytes (system-segment with the upper
/// 32 bits of base in bytes 8..12). When TR is 0 — the boot-time
/// state on this kernel today — there is no TSS and we report base 0.
/// 12.1.0d-2 will install a real TSS before VMLAUNCH.
```

## L379-381 · `unsafe {`

```
// SAFETY: GDT is identity-mapped kernel rodata. We only read.
// If the selector were bogus we could read garbage, but that is
// already a kernel bug elsewhere — we trust `str`'s output.
```

## L385-386 · `let base_lo = ((lo >> 16) & 0xFFFF)`

```
// base[15:0] in lo[16..32], base[23:16] in lo[32..40],
// base[31:24] in lo[56..64], base[63:32] in hi[0..32].
```

## L395 · `pub(super) fn setup_host_state(host_rsp: u64) -> Result<(), &'static str> {`

```
// ── Host-state setup ───────────────────────────────────────────────
```

## L397-403 · `pub(super) fn setup_host_state(host_rsp: u64) -> Result<(), &'static str> {`

```
/// Write the full host-state subset and read every field back. Runs
/// inside VMX root mode after VMPTRLD. On any mismatch returns the
/// failing-step error string; the caller still issues VMXOFF.
///
/// `host_rsp` is captured at the call site (one frame above) so it
/// describes a slot that is still live when the function returns.
/// 12.1.0d-2 will replace this with a dedicated VM-exit stack.
```

## L408-411 · `let cs = (snap.cs & 0xFFF8) as u64;`

```
// Selector fields must have TI=0 and RPL=0 per SDM §26.2.3 — we
// mask the bottom 3 bits so a stray RPL doesn't cause VMLAUNCH
// to fail later on. (Today CS=0x08, DS/SS/ES=0x10, FS/GS=0, TR=0
// — all already RPL-0; the mask is defence in depth.)
```

## L420-422 · `let efer = unsafe { rdmsr(IA32_EFER) };`

```
// Architectural MSRs that mirror into host-state.
// SAFETY: all four MSRs are architectural since SYSENTER (P6) /
// EFER (AMD64) — always present on x86_64.
```

## L432 · `vmwrite_check(HOST_CR0, snap.cr0, "HOST_CR0")?;`

```
// Control registers.
```

## L437 · `vmwrite_check(HOST_CS_SELECTOR, cs, "HOST_CS_SELECTOR")?;`

```
// Segment selectors.
```

## L446 · `vmwrite_check(HOST_FS_BASE, fs_base, "HOST_FS_BASE")?;`

```
// Segment / table bases.
```

## L453 · `vmwrite_check(HOST_IA32_SYSENTER_CS, sysenter_cs, "HOST_IA32_SYSENTER_CS")?;`

```
// SYSENTER MSRs.
```

## L458-459 · `vmwrite_check(HOST_IA32_EFER, efer, "HOST_IA32_EFER")?;`

```
// EFER (long-mode bit lives here; required when "load IA32_EFER"
// VM-exit control is set, harmless otherwise).
```

## L461-462 · `let pat = unsafe { rdmsr(0x277) };`

```
// Host PAT (carries the framebuffer's WC entry), reloaded on every exit.
// SAFETY: IA32_PAT is architectural on every VMX-capable CPU.
```

## L466-467 · `vmwrite_check(HOST_RSP, host_rsp, "HOST_RSP")?;`

```
// RIP / RSP last so a failure earlier doesn't leave a stale RSP
// pointing into a dead frame.
```

## L474 · `fn exit_trampoline_addr() -> u64 {`

```
// ── HOST_RIP placeholder ───────────────────────────────────────────
```

## L476-480 · `fn exit_trampoline_addr() -> u64 {`

```
// HOST_RIP needs *some* canonical kernel-text address at the time
// `setup_host_state` runs — but `launch_test()` always overrides it
// just-in-time (with the runtime address of its in-line resume label)
// before VMLAUNCH. The placeholder here exists only so the field is
// canonical between the host-state pass and the launch.
```

## L485 · `fn allocate_and_populate_io_bitmaps() -> Result<(u64, u64), &'static str> {`

```
// ── I/O bitmaps ────────────────────────────────────────────────────
```

## L487-500 · `fn allocate_and_populate_io_bitmaps() -> Result<(u64, u64), &'static str> {`

```
/// Allocate two 4-KB pages for IO_BITMAP_A (ports 0x0000-0x7FFF) and
/// IO_BITMAP_B (ports 0x8000-0xFFFF), fill them with 0xFF so every
/// I/O instruction the guest executes triggers a VM-exit.
///
/// Why trap-everything: a real-mode/prot-mode guest like Linux
/// will probe PCI config (0xCF8/0xCFC), keyboard (0x60/0x64), CMOS
/// (0x70/0x71), PIC (0x20/0xA0), PIT (0x40-0x43) and so on. Without
/// trapping, those I/O instructions execute natively against the
/// host's real hardware on the NUC — quietly corrupting host
/// state. We trap everything and ignore (return 0 for IN, no-op
/// for OUT) anything our handler doesn't explicitly understand.
///
/// Returns (io_bitmap_a_phys, io_bitmap_b_phys). Frames leaked
/// (same lifecycle as VMXON / VMCS / EPT regions).
```

## L507 · `unsafe {`

```
// SAFETY: identity-mapped, freshly allocated, exclusive.
```

## L516-518 · `pub fn decode_io_exit_qualification(qual: u64) -> (u16, bool, u8) {`

```
/// Decode a VM-exit-qualification value from an I/O instruction
/// VM-exit (basic reason 30) per SDM §27.2.1 Table 27-5. Returns
/// (port, direction_in, size_bytes).
```

## L526-527 · `fn allocate_msr_bitmap() -> Result<u64, &'static str> {`

```
/// Allocate the 4 KB MSR bitmap: intercept-all with the KVM pass-through set
/// (`vmx::msr::init_msr_bitmap`). Returns the bitmap phys addr.
```

## L531 · `unsafe { super::msr::init_msr_bitmap(bitmap); }`

```
// SAFETY: identity-mapped, freshly allocated, exclusive.
```

## L536-538 · `pub fn host_cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {`

```
/// Run CPUID on the host with the guest's input leaf+subleaf,
/// return (eax, ebx, ecx, edx). LLVM reserves rbx, so we save it
/// across the cpuid instruction.
```

## L544-547 · `unsafe {`

```
// SAFETY: CPUID has no privileged side effects beyond what the
// architectural docs spell out (returns CPU info in A/B/C/D).
// rbx is preserved via push/pop because LLVM forbids using it
// as a clobbered or output register directly.
```

## L564 · `const CPU_HLT_EXITING: u32 = 1 << 7;`

```
// ── Execution controls ─────────────────────────────────────────────
```

## L566 · `const CPU_HLT_EXITING: u32 = 1 << 7;`

```
// CPU-based control bits we care about.
```

## L568 · `const CPU_INTR_WINDOW_EXITING: u32 = 1 << 2;`

```
/// Bit 2: exit as soon as the guest can take an external interrupt.
```

## L577 · `const SEC_ENABLE_EPT: u32 = 1 << 1;`

```
// Secondary control bits.
```

## L582-583 · `const GUEST_VPID: u64 = 1;`

```
/// The guest's VPID. One tag for every vCPU: VPIDs are per logical processor,
/// and each vCPU owns its core.
```

## L586-588 · `fn vpid_supported() -> bool {`

```
/// VPID usable: allowed in the secondary controls, plus INVVPID with the
/// all-context type (EPT_VPID_CAP bits 32 and 42) to drop a previous run's
/// entries under the same tag.
```

## L590 · `let sec = unsafe { super::rdmsr(IA32_VMX_PROCBASED_CTLS2) };`

```
// SAFETY: both capability MSRs exist whenever VMX does (checked by probe).
```

## L596 · `fn invvpid_all() {`

```
/// INVVPID all-context (type 2): drop every VPID-tagged translation on this core.
```

## L599-600 · `unsafe {`

```
// SAFETY: VMX root operation (after VMXON); the descriptor is 16 bytes and
// aligned by the array; type 2 support was checked in `vpid_supported`.
```

## L608-611 · `const SEC_ENABLE_RDTSCP: u32 = 1 << 3;`

```
/// Bit 3: enable native RDTSCP/RDPID execution in guest. Without
/// this, RDTSCP raises #UD even when CPUID indicates support —
/// Linux's `read_tsc` (clocksource switch) uses RDTSCP and faults
/// during clocksource init otherwise.
```

## L613-616 · `const SEC_ENABLE_INVPCID: u32 = 1 << 12;`

```
/// Bit 12: enable native INVPCID execution in guest. Without this,
/// INVPCID raises #UD even when CPUID indicates support — Linux's
/// `native_flush_tlb_global` uses INVPCID and faults during
/// setup_arch otherwise.
```

## L618-623 · `const SEC_ENABLE_XSAVES: u32 = 1 << 20;`

```
/// Bit 20: enable native XSAVES/XRSTORS in guest. Without this,
/// XSAVES raises #UD even when CPUID Leaf 0xD subleaf 1:EAX[3]
/// indicates support — Linux uses XSAVES for context switch when
/// supervisor xstates are present (CET_S = XFEATURE bit 12, which
/// Alpine virt has in the active set 0x1807). First userspace
/// context switch would fault otherwise.
```

## L626-635 · `const PIN_EXT_INTR_EXITING: u32 = 1 << 0;`

```
// Pin-based control bits.
/// Bit 0: External-interrupt exiting. When 1, host-targeted external
/// interrupts cause VM-exits (basic reason 1) instead of being
/// delivered to the guest's IDT. Without this, host LAPIC/PIC IRQs
/// arriving during guest run get acknowledged by the real APIC into
/// the guest's IDT — but the guest's EOI write to the LAPIC MMIO
/// goes through EPT into our scratch page (not the real LAPIC), so
/// real-LAPIC ISR stays set forever and the host stops receiving
/// interrupts after VMXOFF (= post-`microvm linux` keyboard freeze
/// observed on N100, 2026-05-02).
```

## L637-638 · `const PIN_NMI_EXITING: u32 = 1 << 3;`

```
/// Bit 3: NMI exiting. Without it a host NMI arriving in non-root mode is
/// delivered through the GUEST's IDT (KVM always sets it).
```

## L641 · `const ENTRY_IA32E_MODE_GUEST: u32 = 1 << 9;`

```
// VM-entry control bits.
```

## L646 · `const EXIT_HOST_ADDR_SPACE_SIZE: u32 = 1 << 9;`

```
// VM-exit control bits.
```

## L653-657 · `fn fixed_ctrl(desired: u32, msr: u32) -> u32 {`

```
/// Compute a control field's value from the desired-set, applying
/// the must-be-0 / must-be-1 mask MSR per SDM §A.3.1: low 32 bits
/// are allowed-0 (must be 1 if set there), high 32 bits are
/// allowed-1 (must be 0 if clear there). Result is `(desired |
/// allowed_0) & allowed_1`.
```

## L659-660 · `let raw = unsafe { rdmsr(msr) };`

```
// SAFETY: VMX control MSRs are architectural once the CPU has
// VMX (gated by `probe()` upstream).
```

## L667-675 · `pub(super) fn setup_execution_controls(eptp: u64) -> Result<(), &'static str> {`

```
/// Write the execution-control fields for an unrestricted real-mode
/// guest with EPT + selective I/O exiting. CPU-based: HLT-exiting +
/// use-IO-bitmaps + activate-secondary. Secondary: enable-EPT +
/// unrestricted-guest. VM-exit: host-address-space-size. VM-entry:
/// IA-32e-mode-guest stays 0.
///
/// I/O bitmaps trap accesses to port 0x80 (substrate-test stub) and
/// ports 0x3F8-0x3FF (UART COM1 — Linux's earlyprintk target). All
/// other ports pass through natively.
```

## L680-692 · `let pin = fixed_ctrl(PIN_EXT_INTR_EXITING | PIN_NMI_EXITING, pinbased_ctls_msr());`

```
// Use IA32_VMX_TRUE_*_CTLS when the CPU supports them (Alder
// Lake-N does). The TRUE variants relax classic "default-1"
// bits — most notably CR3-load/store-exiting (bits 15/16 of
// PROCBASED) which the classic MSR forces on but with EPT
// active provides no benefit (the guest's CR3 is opaque to
// the host; EPT walks it without our help). Forcing CR3-exit
// also produces a guest-trap sequence that does NOT exist on
// SVM (where we never trap CR3) and our trap-handler's
// write_guest_cr3 + advance_rip leaves the GUEST_CR3 / RIP /
// VPID-TLB combo in a state that the next VMRESUME's
// consistency check rejects (reason 33 on bare-metal-NUC).
// Match KVM's universal use of TRUE_*_CTLS to get a minimal
// control set close to SVM's.
```

## L704-709 · `let vpid = vpid_supported();`

```
// Secondary controls don't have a TRUE variant — IA32_VMX_PROCBASED_
// CTLS2 is used directly. SDM Appendix A.3.3 / A.3.4.
// VPID (KVM `enable_vpid`): without it every VM entry and exit flushes
// the guest's TLB, so each of thousands of exits a second restarts the
// guest on a cold TLB. EPT entries are only ever added while it runs,
// which needs no flush.
```

## L719-720 · `IA32_VMX_PROCBASED_CTLS2,`

```
// No WAITPKG: CPUID hides it (guest_cpuid), so Linux never uses
// TPAUSE, and IA32_UMWAIT_CONTROL needs no switching.
```

## L723-730 · `let entry = fixed_ctrl(ENTRY_LOAD_IA32_EFER | ENTRY_LOAD_IA32_PAT, entry_ctls_msr());`

```
// EFER state management: CPU saves/loads guest EFER via VMCS
// GUEST_IA32_EFER on each entry/exit. Without these, guest's
// WRMSR EFER (LME=1) updates the real MSR but is lost on the
// next entry (uncertain behaviour) and the host's EFER stays
// unchanged across exit (also dangerous on transitioning the
// host back to long mode).
// PAT likewise: the guest's WRMSR 0x277 lands in GUEST_IA32_PAT, the host's
// PAT (framebuffer WC) comes back on every exit.
```

## L752-771 · `const TRAP_CP: u64 = 1 << 21;`

```
// Inert ancillary controls — clear bitmaps + counts so VMX
// doesn't dereference them.
//
// EXCEPTION_BITMAP: trap only #CP (vector 21, CET control-flow
// protection). Linux's early boot specifically RELIES on its
// own #PF handler (`early_idt_handler_array` → `early_make_pgtable`)
// to build the direct-map page tables lazily — trapping #PF
// breaks that lazy-PT mechanism (12.1.1c-3b3b6 lesson).
//
// #CP is different: when CR4.CET=1 inherits from host into the
// guest and Linux hits an indirect call to a non-ENDBR target
// (e.g. asm reboot stubs after panic), Linux's exc_control_protection
// BUG()s and recursively re-faults. Without us trapping it, the
// recursion runs entirely in non-root mode with no exits — host
// CPU is stuck in VMRESUME-loop forever (no I/O, no HLT, our
// MAX_ITERATIONS cap never fires). Trapping #CP surfaces it as
// exit reason 0 and lets the loop terminate cleanly.
//
// Triple faults (basic reason 2) still surface independently of
// EXCEPTION_BITMAP if Linux truly fails.
```

## L780-796 · `vmwrite(CR0_GUEST_HOST_MASK, 0)?;`

```
// CR0/CR4 shadowing — hide VMXE from the guest so Linux's
// unconditional `mov cr4, rax` doesn't try to clear it (which
// would either #GP because IA32_VMX_CR4_FIXED0 has VMXE
// must-be-1, or break VMX operation).
//
// Mechanism (SDM §25.4): bits set in CR4_GUEST_HOST_MASK are
// "host-owned". Guest CR4 reads return SHADOW for those bits,
// real CR4 for others. Guest CR4 writes that don't change a
// masked bit relative to shadow → no exit, real-CR4-bit
// preserved. Writes that would change a masked bit → VM-exit
// (guest-CR-access reason 28).
//
// Setting mask = VMXE only, shadow.VMXE = 0: Linux reads CR4
// and sees VMXE=0. Linux's `mov cr4, value-with-VMXE=0` keeps
// shadow.VMXE = 0 = matches → no exit, real CR4.VMXE stays 1.
// Linux never tries to set VMXE=1 (it doesn't know about VMX),
// so the exit branch never fires.
```

## L798 · `vmwrite(CR4_GUEST_HOST_MASK, 1 << 13)?; // VMXE`

```
// VMXE
```

## L805 · `const AR_CODE32: u32 = 0xB | 0x10 | 0x80 | (1 << 14) | (1 << 15);`

```
// ── Guest state ────────────────────────────────────────────────────
```

## L807-815 · `const AR_CODE32: u32 = 0xB | 0x10 | 0x80 | (1 << 14) | (1 << 15);`

```
// VMCS guest segment AR-byte layout (SDM §24.4.1 Table 24-2):
//   bits 3:0  = type
//   bit  4    = S (1 = code/data, 0 = system)
//   bits 6:5  = DPL
//   bit  7    = P
//   bit  12   = AVL
//   bit  13   = L (64-bit code)
//   bit  14   = D/B
//   bit  15   = G
```

## L817-821 · `const AR_CODE32: u32 = 0xB | 0x10 | 0x80 | (1 << 14) | (1 << 15);`

```
// 32-bit protected-mode code (CS): type=0xB (executable, readable,
// accessed), S=1, DPL=0, P=1, L=0 (not 64-bit), D/B=1 (32-bit
// operand size), G=1 (4 KB granularity). Limit interpreted as
// 4 KB pages so 0xFFFFFFFF means full 4 GB. The accessed bit is
// required even with unrestricted-guest=1.
```

## L823-824 · `const AR_DATA32: u32 = 0x3 | 0x10 | 0x80 | (1 << 14) | (1 << 15);`

```
// 32-bit data (SS, DS, ES, FS, GS): type=0x3 (RW, accessed),
// S=1, DPL=0, P=1, D/B=1, G=1.
```

## L826-827 · `const AR_TSS_BUSY: u32 = 0xB | 0x80;`

```
// Busy TSS: type=0xB, S=0, DPL=0, P=1. Same value for 32-bit and
// 64-bit busy TSS per SDM Vol. 3A §3.5 Table 3-2.
```

## L829 · `const AR_UNUSABLE: u32 = 1 << 16;`

```
// Unusable segment marker (bit 16). Used for guest LDTR (no LDT).
```

## L831-832 · `const AR_CODE16: u32 = 0x9B;`

```
// 16-bit real-mode code (CS): type=0xB, S=1, DPL=0, P=1, no G/D (16-bit).
// Matches SVM `ATTR_CODE_RM`. Used for an AP's SIPI entry (guest SMP).
```

## L834 · `const AR_DATA16: u32 = 0x93;`

```
// 16-bit real-mode data (SS/DS/ES/FS/GS): type=0x3, S=1, P=1.
```

## L837-851 · `pub(super) fn setup_guest_state(guest_rip: u64) -> Result<(), &'static str> {`

```
/// Fill GUEST_* fields for an unrestricted 32-bit protected-mode
/// guest with flat segments. CR0 has PE=1 PG=0 (paging off — Linux
/// will set up its own); unrestricted-guest=1 lets us clear PG
/// despite CR0_FIXED requirements (per SDM §26.3.1.1). All segments
/// are flat (base=0, limit=0xFFFFFFFF) so guest linear == guest
/// physical and EPT translates to host. EFER cleared (no long mode).
///
/// `guest_rip` is the linear (== EPT-mapped guest-physical) address
/// where the first instruction will be fetched. For the substrate
/// test that's 0x10000 (where the stub is copied). For Linux that's
/// `code32_start` from the bzImage setup-header (typically 0x100000).
///
/// `guest_rsp` and the boot-protocol register conventions (RSI =
/// boot_params phys for Linux 32-bit boot) are configured by the
/// caller via the GuestRegs struct passed to `run_guest_once`.
```

## L853-855 · `let cr0_f0 = unsafe { rdmsr(0x486) };`

```
// 32-bit prot mode CR0: must satisfy CR0_FIXED0/1 with PE
// forced 1 and PG forced 0 (unrestricted-guest=1 relaxes
// CR0_FIXED for both bits per SDM §26.3.1.1).
```

## L860-866 · `let host_cr4: u64;`

```
// CR4: take host CR4 with VMXE etc., but clear CET (bit 23).
// Host nopeekOS has CR4.CET=1 for IBT defense — inheriting
// that into the guest enables CET-IBT enforcement against
// Alpine vmlinuz's hand-written asm stubs that lack ENDBR64,
// raising #CP with err_code=3 (ENDBRANCH violation) early in
// boot. CPUID Leaf 7 is also filtered (see enable.rs CPUID
// handler) so Linux never tries to re-enable CET via cr4_init.
```

## L868 · `unsafe { core::arch::asm!("mov {}, cr4", out(reg) host_cr4, options(nostack, preserves_flags)); }`

```
// SAFETY: pure register read.
```

## L876-877 · `vmwrite(GUEST_CS_SELECTOR, 0x08)?;`

```
// Selectors. Standard kernel-style values; with unrestricted-
// guest the AR-byte determines validity, not the selector itself.
```

## L887-888 · `vmwrite(GUEST_CS_BASE, 0)?;`

```
// Flat segments — base=0, limit=4 GB. Guest linear addr = guest
// physical addr, EPT does the rest.
```

## L900 · `vmwrite(GUEST_CS_LIMIT, 0xFFFF_FFFF)?;`

```
// Limits — 4 GB flat for code/data; TR/GDTR/IDTR small/sane.
```

## L912-913 · `vmwrite(GUEST_CS_AR_BYTES, AR_CODE32 as u64)?;`

```
// AR bytes. 32-bit code/data, busy TSS (validity-only;
// guest doesn't actually use TR), unusable LDTR.
```

## L923-924 · `vmwrite(GUEST_DR7, 0x400)?;`

```
// Misc guest state. EFER cleared (no LME, no LMA — 32-bit
// prot mode).
```

## L926 · `vmwrite(GUEST_RFLAGS, 0x2)?; // IF=0, reserved bit 1 set`

```
// IF=0, reserved bit 1 set
```

## L927 · `vmwrite(GUEST_RSP, 0x80000)?; // arbitrary; real stack comes from caller GPRs if used`

```
// arbitrary; real stack comes from caller GPRs if used
```

## L943-950 · `pub(super) fn setup_guest_state_ap(sipi_vector: u8) -> Result<(), &'static str> {`

```
/// Fill GUEST_* fields for an AP vCPU's SIPI start (guest SMP): 16-bit
/// real mode at `CS = (vector<<8):0000`, i.e. physical entry `vector<<12`
/// — where Linux's BSP copied the real-mode AP trampoline. The trampoline
/// takes the AP into protected→long mode and `start_secondary`; the
/// IA-32e/EFER transition is then picked up by `sync_entry_ia32e_with_efer`
/// exactly as on the BSP's 32-bit→64-bit boot. Real mode is allowed because
/// unrestricted-guest=1 (set in `setup_execution_controls`, shared with the
/// BSP). Mirrors svm `setup_vmcb_ap`.
```

## L952-954 · `let cr0_f0 = unsafe { rdmsr(0x486) };`

```
// Real-mode CR0: PE=0, PG=0. Unrestricted-guest relaxes the
// CR0_FIXED0 must-be-1 on PE/PG (SDM §26.3.1.1); keep the other
// fixed bits, force ET (bit 4).
```

## L960 · `unsafe { core::arch::asm!("mov {}, cr4", out(reg) host_cr4, options(nostack, preserves_flags)); }`

```
// SAFETY: pure register read.
```

## L962 · `let guest_cr4 = host_cr4 & !(1u64 << 23); // clear CET (see setup_guest_state)`

```
// clear CET (see setup_guest_state)
```

## L990 · `vmwrite(GUEST_CS_LIMIT, 0xFFFF)?;`

```
// 16-bit real-mode limits (0xFFFF) for the segments; TR/GDTR/IDTR sane.
```

## L1012 · `vmwrite(GUEST_RFLAGS, 0x2)?; // IF=0, reserved bit 1 set`

```
// IF=0, reserved bit 1 set
```

## L1014 · `vmwrite(GUEST_RIP, 0)?; // real-mode offset 0 within CS=vector<<12`

```
// real-mode offset 0 within CS=vector<<12
```

## L1029 · `#[repr(C)]`

```
// ── VMRESUME loop with full GPR save/restore ───────────────────────
```

## L1031-1039 · `#[repr(C)]`

```
/// Guest general-purpose registers preserved across VMLAUNCH /
/// VMRESUME boundaries. Layout is `#[repr(C)]` with a fixed offset
/// per field so the asm! block can address them via `[rdi + N]`.
/// rsp lives in VMCS::GUEST_RSP; the corresponding slot here is
/// unused (kept so offsets stay sequential / canonical).
///
/// All-zero default = "fresh guest state on first launch". Updated
/// by every VM-exit so subsequent VMRESUME enters with the values
/// the guest had at exit (modulo Rust-side handler edits).
```

## L1043 · `pub rax: u64,    // offset   0`

```
// offset   0
```

## L1044 · `pub rcx: u64,    //          8`

```
//          8
```

## L1045 · `pub rdx: u64,    //         16`

```
//         16
```

## L1046 · `pub rbx: u64,    //         24`

```
//         24
```

## L1047 · `pub _rsp: u64,   //         32 — unused (VMCS::GUEST_RSP holds it)`

```
//         32 — unused (VMCS::GUEST_RSP holds it)
```

## L1048 · `pub rbp: u64,    //         40`

```
//         40
```

## L1049 · `pub rsi: u64,    //         48`

```
//         48
```

## L1050 · `pub rdi: u64,    //         56`

```
//         56
```

## L1051 · `pub r8:  u64,    //         64`

```
//         64
```

## L1052 · `pub r9:  u64,    //         72`

```
//         72
```

## L1053 · `pub r10: u64,    //         80`

```
//         80
```

## L1054 · `pub r11: u64,    //         88`

```
//         88
```

## L1055 · `pub r12: u64,    //         96`

```
//         96
```

## L1056 · `pub r13: u64,    //        104`

```
//        104
```

## L1057 · `pub r14: u64,    //        112`

```
//        112
```

## L1058 · `pub r15: u64,    //        120`

```
//        120
```

## L1061 · `pub struct LaunchOutcome {`

```
/// Outcome of one VM-entry/exit cycle.
```

## L1065-1067 · `pub guest_rax: u64,`

```
/// Guest RAX captured at VM-exit. Convenience mirror of
/// `regs.rax` after `run_guest_once`; redundant but kept for
/// callers that don't have a regs reference handy.
```

## L1071-1098 · `pub(super) fn run_guest_once(`

```
/// Run one VM-entry/exit cycle. Loads guest GPRs from `regs`,
/// VMLAUNCHes (if `launched=false`) or VMRESUMEs (if true), saves
/// guest GPRs back to `regs` on the resulting VM-exit, returns the
/// exit info.
///
/// Caller must already have written host-state, guest-state and
/// execution-controls into the current VMCS, and must be in VMX
/// root mode.
///
/// The asm! block has two control-flow paths that converge at
/// label 3:
///   1. VMLAUNCH/VMRESUME succeeds: control transfers to the guest.
///      Guest executes its first trapping instruction. The CPU
///      loads host state from VMCS — including HOST_RIP set to
///      label 2 and HOST_RSP set to our current stack pointer —
///      and we land at label 2 with the guest's GPRs still in
///      CPU registers. We push all 15 (rax..r15, skipping rsp) to
///      stack, then pop one-by-one storing into `regs` via the
///      saved struct pointer. Then VMREAD exit info.
///   2. VMLAUNCH/VMRESUME fails (VMfail{Invalid,Valid}): no
///      transition, execution falls through. Set vmfail=1 and
///      converge.
///
/// SAFETY: caller guarantees VMX root mode + current VMCS +
/// validated host/guest/control state. The asm pushes a variable
/// amount onto the stack across the boundary; HOST_RSP is set to
/// the post-prologue rsp so the post-exit landing finds the right
/// stack shape.
```

## L1110-1125 · `unsafe {`

```
// SAFETY: see fn-level docs. The asm respects every register
// dependency by ordering: launched-flag check sets ZF before
// r10 is overwritten with the guest's r10; rdi (struct ptr) is
// overwritten LAST; post-exit save spills all guest GPRs to
// stack first (so rdi stays guest's value), then reloads struct
// ptr from the saved slot before storing.
//
// FPU xsave/xrstor lives INSIDE this asm block (mirror of SVM
// v0.172.53 fix). Rust-helper FPU swap around the call left a
// window where the +avx2 compiler could spill `vmovups ymm`
// between the helper and the asm, clobbering the just-restored
// guest FPU. Putting xsave64/xrstor64 inside the asm — with
// only GPR/vm* instructions between xrstor and vmresume —
// eliminates the window. v53 was applied to SVM only at the
// time ("VMX keeps the helper form (untested path)"); now that
// VMX is the path under test on bare-metal NUC, port it.
```

## L1128 · `"push rbp",`

```
// ── PROLOGUE: save host callee-saved + 3 pointers ─────
```

## L1135 · `"push rdi",                     // struct ptr`

```
// struct ptr
```

## L1136 · `"push r8",                      // host_fpu  ptr`

```
// host_fpu  ptr
```

## L1137 · `"push r9",                      // guest_fpu ptr`

```
// guest_fpu ptr
```

## L1138-1139 · `"mov rcx, 0x6C14",              // HOST_RSP`

```
// Stack now: [rsp+0]=guest_fpu [+8]=host_fpu
//   [+16]=struct_ptr [+24..71]=host_callee_saved
```

## L1141-1142 · `"mov rcx, 0x6C14",              // HOST_RSP`

```
// VMCS host pointer fields, set every entry so we never
// depend on stale VMCS state across calls.
```

## L1143 · `"mov rcx, 0x6C14",              // HOST_RSP`

```
// HOST_RSP
```

## L1145 · `"mov rcx, 0x6C16",              // HOST_RIP`

```
// HOST_RIP
```

## L1149-1157 · `"mov rcx, [rsp + 8]",           // host_fpu`

```
// ── FPU SAVE/RESTORE (KVM kvm_load_guest_fpu): VMRESUME
// preserves no x87/SSE/AVX/AVX-512. Must be in-asm,
// adjacent to vmresume — a +avx2-built kernel spills
// `ymm` anywhere between a Rust helper and the asm,
// clobbering the just-restored guest state. Mask = -1
// (all XCR0-enabled components; guest owns XCR0 via
// XSETBV). Only GPR/vm* instructions sit between this
// xrstor and vmresume, and between vmresume and the
// paired xsave below.
```

## L1158 · `"mov rcx, [rsp + 8]",           // host_fpu`

```
// host_fpu
```

## L1161 · `"xsave64 [rcx]",                // save host FPU`

```
// save host FPU
```

## L1162 · `"mov rcx, [rsp + 0]",           // guest_fpu`

```
// guest_fpu
```

## L1165 · `"xrstor64 [rcx]",               // restore guest FPU`

```
// restore guest FPU
```

## L1167-1169 · `"mov r10, rsi",                 // r10 = launched`

```
// ── ENTRY: load guest GPRs from struct ───────────────
// r10 is caller-saved per System V ABI, so we use it as
// a scratch for the launched flag without preservation.
```

## L1170 · `"mov r10, rsi",                 // r10 = launched`

```
// r10 = launched
```

## L1171 · `"test r10, r10",                // ZF = !launched`

```
// ZF = !launched
```

## L1177 · `"mov rbp, [rdi +  40]",`

```
// rsp at offset 32 is unused (VMCS::GUEST_RSP holds it).
```

## L1182 · `"mov r10, [rdi +  80]",         // overwrites flag — but`

```
// overwrites flag — but
```

## L1183-1185 · `"mov r11, [rdi +  88]",`

```
// ZF was set by `test`
// above and `mov`
// doesn't touch flags
```

## L1191 · `"mov rdi, [rdi +  56]",         // rdi LAST (overwrites`

```
// rdi LAST (overwrites
```

## L1192 · `"jz 9f",                        // ZF set → !launched →`

```
// struct ptr)
```

## L1193 · `"jz 9f",                        // ZF set → !launched →`

```
// ZF set → !launched →
```

## L1194 · `"vmresume",`

```
// VMLAUNCH path
```

## L1196 · `"jmp 4f",                       // fall-through =`

```
// fall-through =
```

## L1197 · `"9:",                           // VMLAUNCH path`

```
// VMfail{Invalid,Valid}
```

## L1199 · `"9:",                           // VMLAUNCH path`

```
// VMLAUNCH path
```

## L1201 · `"4:",`

```
// fall-through to fail handler
```

## L1203-1208 · `"4:",`

```
// ── FAIL HANDLER ─────────────────────────────────────
// VMRESUME/VMLAUNCH failed — no transition happened.
// GPRs still hold the guest values we loaded. FPU still
// has the guest state we xrestored. Restore host FPU
// before returning to Rust so the host doesn't run on
// guest FPU.
```

## L1210 · `"mov rcx, [rsp + 0]",           // guest_fpu`

```
// guest_fpu
```

## L1213 · `"xsave64 [rcx]",                // save guest FPU`

```
// save guest FPU
```

## L1214 · `"mov rcx, [rsp + 8]",           // host_fpu`

```
// host_fpu
```

## L1217 · `"xrstor64 [rcx]",               // restore host FPU`

```
// restore host FPU
```

## L1218 · `"mov rcx, 1",                   // vmfail = 1`

```
// vmfail = 1
```

## L1219 · `"xor rax, rax",                 // exit_reason = 0`

```
// exit_reason = 0
```

## L1220 · `"xor rdx, rdx",                 // exit_qual = 0`

```
// exit_qual = 0
```

## L1221 · `"add rsp, 24",                  // discard guest_fpu, host_fpu, struct_ptr`

```
// discard guest_fpu, host_fpu, struct_ptr
```

## L1231 · `"2:",`

```
// ── POST-VM-EXIT: save guest GPRs ────────────────────
```

## L1233-1236 · `"push rax",`

```
// Push all 15 guest GPRs onto stack so we can reuse rdi
// (currently guest's rdi) without losing it. Order is
// canonical: lowest-offset (rax) pushed first, so it
// ends up deepest on stack.
```

## L1252-1253 · `"mov rcx, [rsp + 120]",         // guest_fpu`

```
// Stack now: 15 GPRs [rsp+0..119], guest_fpu [+120],
//   host_fpu [+128], struct_ptr [+136], callee_saved [+144..]
```

## L1255-1257 · `"mov rcx, [rsp + 120]",         // guest_fpu`

```
// ── FPU SAVE/RESTORE (paired exit half) ──────────────
// Guest GPRs are on the stack now → rax/rcx/rdx free to
// clobber. Save guest FPU, restore host's.
```

## L1258 · `"mov rcx, [rsp + 120]",         // guest_fpu`

```
// guest_fpu
```

## L1261 · `"xsave64 [rcx]",                // save guest FPU`

```
// save guest FPU
```

## L1262 · `"mov rcx, [rsp + 128]",         // host_fpu`

```
// host_fpu
```

## L1265 · `"xrstor64 [rcx]",               // restore host FPU`

```
// restore host FPU
```

## L1267 · `"mov rdi, [rsp + 136]",         // struct ptr`

```
// struct ptr
```

## L1268 · `"pop rax", "mov [rdi + 120], rax",      // r15`

```
// Pop in reverse-push order, store at the right offset.
```

## L1269 · `"pop rax", "mov [rdi + 120], rax",      // r15`

```
// r15
```

## L1270 · `"pop rax", "mov [rdi + 112], rax",      // r14`

```
// r14
```

## L1271 · `"pop rax", "mov [rdi + 104], rax",      // r13`

```
// r13
```

## L1272 · `"pop rax", "mov [rdi +  96], rax",      // r12`

```
// r12
```

## L1273 · `"pop rax", "mov [rdi +  88], rax",      // r11`

```
// r11
```

## L1274 · `"pop rax", "mov [rdi +  80], rax",      // r10`

```
// r10
```

## L1275 · `"pop rax", "mov [rdi +  72], rax",      // r9`

```
// r9
```

## L1276 · `"pop rax", "mov [rdi +  64], rax",      // r8`

```
// r8
```

## L1277 · `"pop rax", "mov [rdi +  56], rax",      // rdi (guest's)`

```
// rdi (guest's)
```

## L1278 · `"pop rax", "mov [rdi +  48], rax",      // rsi`

```
// rsi
```

## L1279 · `"pop rax", "mov [rdi +  40], rax",      // rbp`

```
// rbp
```

## L1280 · `"pop rax", "mov [rdi +  24], rax",      // rbx`

```
// rbx
```

## L1281 · `"pop rax", "mov [rdi +  16], rax",      // rdx`

```
// rdx
```

## L1282 · `"pop rax", "mov [rdi +   8], rax",      // rcx`

```
// rcx
```

## L1283 · `"pop rax", "mov [rdi +   0], rax",      // rax`

```
// rax
```

## L1285 · `"mov rcx, 0x4402",              // VM_EXIT_REASON`

```
// Read VM-exit info now that GPRs are safe.
```

## L1286 · `"mov rcx, 0x4402",              // VM_EXIT_REASON`

```
// VM_EXIT_REASON
```

## L1288 · `"mov rcx, 0x6400",              // VM_EXIT_QUALIFICATION`

```
// VM_EXIT_QUALIFICATION
```

## L1290 · `"xor rcx, rcx",                 // vmfail = 0`

```
// vmfail = 0
```

## L1292 · `"add rsp, 24",                  // discard guest_fpu, host_fpu, struct_ptr`

```
// discard guest_fpu, host_fpu, struct_ptr
```

## L1299 · `"sti",                          // VM-exit cleared IF; re-enable`

```
// VM-exit cleared IF; re-enable
```

## L1327-1328 · `pub fn basic_exit_reason(raw: u64) -> u16 {`

```
/// Read VM_EXIT_REASON's basic-reason field (bits 15:0). Convenience
/// for callers that already have the raw 32-bit value.
```

## L1333-1337 · `pub(super) fn guest_cpl() -> Result<u8, &'static str> {`

```
/// Advance GUEST_RIP past the just-exited instruction. The CPU
/// records the instruction length in VM_EXIT_INSTRUCTION_LEN; we
/// add it to the current GUEST_RIP. Required after I/O exits
/// (otherwise VMRESUME re-executes the trapping OUT/IN forever).
/// Guest CPL = SS.DPL (SDM 27.3.1.5: the SS attribute DPL is the CPL).
```

## L1346-1347 · `let v = vmread(GUEST_INTERRUPTIBILITY_INFO)?;`

```
// KVM `skip_emulated_instruction` → `vmx_set_interrupt_shadow(0)`: the
// skipped instruction was the one an STI / MOV SS shadow covered.
```

## L1355-1356 · `pub fn read_guest_cr3() -> Result<u64, &'static str> {`

```
/// Read GUEST_CR3 from the current VMCS. Used by the CR-access
/// exit handler when the guest does `MOV reg, CR3`.
```

## L1361-1362 · `pub fn write_guest_cr3(value: u64) -> Result<(), &'static str> {`

```
/// Write GUEST_CR3 to the current VMCS. Used by the CR-access
/// exit handler when the guest does `MOV CR3, reg`.
```

## L1367-1368 · `pub fn read_guest_rsp() -> Result<u64, &'static str> {`

```
/// Read GUEST_RSP from the current VMCS. RSP is not in `GuestRegs`
/// (the CPU loads/saves it from VMCS GUEST_RSP across entry/exit).
```

## L1373 · `pub fn write_guest_rsp(value: u64) -> Result<(), &'static str> {`

```
/// Write GUEST_RSP to the current VMCS.
```

## L1378-1382 · `pub fn inject_external_irq(vector: u8) -> Result<(), &'static str> {`

```
/// Inject an external interrupt on the next VM-entry. `vector` is the
/// IDT vector to deliver. The CPU honours the guest's RFLAGS.IF / NMI
/// blocking state — if interrupts are disabled the entry will fail
/// (caller must check), but in practice we inject from MMIO-trap exits
/// where the guest had IF=1 already.
```

## L1384-1385 · `let info: u64 = (vector as u64) | (1u64 << 31);`

```
// Bit 31 = valid, bits 10:8 = type (0 = external interrupt),
// bits 7:0 = vector.
```

## L1390-1391 · `pub fn inject_exception(vector: u8, error_code: Option<u32>) -> Result<(), &'static str> {`

```
/// Queue a hardware exception for the next VM-entry (type 3). RIP is NOT
/// advanced: the faulting instruction is the one reported.
```

## L1401 · `pub fn set_interrupt_window_exiting(on: bool) -> Result<(), &'static str> {`

```
/// `vmx_enable_irq_window` / its clear: toggle interrupt-window exiting.
```

## L1412-1413 · `pub fn read_guest_rip() -> Result<u64, &'static str> {`

```
/// Read VMCS GUEST_RIP — the linear address of the guest
/// instruction that caused the most recent VM-exit.
```

## L1418 · `pub fn read_guest_cr0() -> Result<u64, &'static str> {`

```
/// Read VMCS GUEST_CR0.
```

## L1423 · `pub fn read_guest_cr4() -> Result<u64, &'static str> {`

```
/// Read VMCS GUEST_CR4.
```

## L1428 · `pub fn read_guest_efer() -> Result<u64, &'static str> {`

```
/// Read VMCS GUEST_IA32_EFER.
```

## L1433 · `pub fn read_guest_cs_selector() -> Result<u64, &'static str> {`

```
/// Read VMCS GUEST_CS_SELECTOR.
```

## L1438-1440 · `pub fn read_guest_cs_ar() -> Result<u64, &'static str> {`

```
/// Read VMCS GUEST_CS_AR_BYTES — bit 13 = L (64-bit code), bit 14 =
/// D/B. Diagnostic: lets the run loop log when the guest enters long
/// mode and whether re-entry into that mode succeeds.
```

## L1445-1449 · `pub fn read_entry_intr_info() -> Result<u64, &'static str> {`

```
/// Read VM_ENTRY_INTR_INFO_FIELD — the event (if any) we asked the CPU
/// to inject on the entry that just ran. Diagnostic for the bare-metal
/// reason-33 path: confirms whether an injection was pending at a
/// failing VM-entry (a failed entry never delivers, so the field still
/// holds what we wrote).
```

## L1454-1463 · `pub fn guest_interruptible() -> bool {`

```
/// True if the guest can accept a maskable external interrupt right
/// now: RFLAGS.IF=1 and no interruptibility blocking (bit 0 blocking-
/// by-STI, bit 1 blocking-by-MOV-SS). This is the gate KVM applies in
/// `vmx_interrupt_allowed()` before injecting an external interrupt.
/// Injecting one while the guest is NOT interruptible (e.g. IF=0 in a
/// CLI'd critical section — observed mid i8042 status poll) makes the
/// VM-entry fail with reason 33 on bare-metal Intel; AMD's VMRUN
/// tolerates it (delivers a guest #GP), which is why QEMU/AMD never
/// tripped on it. NMI-blocking (bit 3) is irrelevant to maskable
/// external interrupts and is not checked.
```

## L1471-1472 · `pub fn read_vm_entry_controls() -> Result<u64, &'static str> {`

```
/// Read VMCS VM_ENTRY_CONTROLS — the live entry-control field
/// after our last VMWRITE (or fixed_ctrl-applied initial value).
```

## L1477-1479 · `pub fn read_guest_phys_addr() -> Result<u64, &'static str> {`

```
/// Read VMCS GUEST_PHYSICAL_ADDRESS — set by the CPU on EPT
/// violations and EPT misconfigurations. Tells us what guest-phys
/// address the guest tried to access when EPT translation failed.
```

## L1484-1486 · `pub fn read_guest_linear_addr() -> Result<u64, &'static str> {`

```
/// Read VMCS GUEST_LINEAR_ADDRESS — set by the CPU when bit 7 of
/// the EPT-violation qualification is set. The linear address that
/// triggered the violation (vs the qual which is the guest-phys).
```

## L1491-1496 · `pub fn read_exit_intr_info() -> Result<u64, &'static str> {`

```
/// Read VM_EXIT_INTR_INFO. For exception VM-exits (basic reason 0),
/// the relevant fields are:
///   bits 7:0  = vector (0..31)
///   bits 10:8 = interruption type (3 = HW exception, 6 = SW exception)
///   bit  11   = error code valid
///   bit  31   = valid (always set on exit info)
```

## L1501-1503 · `pub fn read_exit_intr_error_code() -> Result<u64, &'static str> {`

```
/// Read VM_EXIT_INTR_ERROR_CODE — the error code architecturally
/// pushed by certain exceptions (#PF, #GP, #SS, #DF, etc.). Only
/// meaningful when bit 11 of VM_EXIT_INTR_INFO is set.
```

## L1508-1520 · `pub fn read_idt_vectoring_info() -> Result<u64, &'static str> {`

```
/// Read IDT_VECTORING_INFORMATION_FIELD (SDM Vol 3 §27.2.4). The CPU
/// sets this when a VM-exit interrupted the guest in the middle of
/// vectoring an event through its IDT (i.e. the host IRQ landed
/// between the CPU's "decided to deliver vector V" and "actually
/// pushed it onto the guest stack"). Encoding mirrors VM_ENTRY_
/// INTR_INFO exactly so the value can be re-injected verbatim:
///   bits 7:0  = vector,  bits 10:8 = type (0=ext IRQ, 2=NMI,
///   3=hw exc, 4=sw int, 5=privsw, 6=sw exc), bit 11 = error-code
///   valid, bit 31 = valid.
/// Must be re-injected on the next entry or the guest loses an
/// interrupt mid-vectoring → corrupt state (the VMX equivalent of
/// the SVM EXITINTINFO/`svm_complete_interrupts` path; see svm
/// enable.rs v0.172.36+).
```

## L1525-1529 · `#[allow(dead_code)]`

```
/// Read IDT_VECTORING_ERROR_CODE — companion to IDT_VECTORING_INFO_
/// FIELD when bit 11 is set. We currently only re-inject external
/// IRQs (type 0) and NMIs (type 2), neither of which carry an error
/// code, so this is unused but kept for symmetry with the exit-info
/// readers and future use if exception re-injection lands.
```

## L1535-1544 · `pub fn write_entry_intr_info(info: u64) -> Result<(), &'static str> {`

```
/// Raw write to VM_ENTRY_INTR_INFO_FIELD. Used to (a) inject a
/// freshly-decided event with a full info word (e.g. when the value
/// comes from `IDT_VECTORING_INFO_FIELD` and must be replayed
/// verbatim), and (b) defensively clear the slot to 0 after every
/// VMRESUME. On bare-metal VMX the CPU auto-clears the valid bit
/// after a successful injection (SDM §27.6), but under nested VMX
/// (KVM emulating VMX on AMD) it MAY leave the bit set — left there,
/// the next VMRESUME re-injects the same event = phantom duplicate
/// interrupt → cumulative guest corruption. This is the VMX
/// equivalent of the SVM EVENTINJ-stale bug fixed in v0.172.42.
```

## L1549-1553 · `pub fn dump_entry_fail_state() {`

```
/// Diagnostic dump after a VM-entry-fail exit (reason 33/34/41).
/// Pulls the fields the CPU consistency-checks at entry so we can tell
/// which one tripped. SDM Vol 3 §26.3.1 enumerates the rules. Common
/// failures: IA-32e/CR/EFER mismatch (long-mode triad not coherent),
/// CS access rights vs CS selector RPL, segment limits, etc.
```

## L1556-1557 · `const GUEST_IA32_DEBUGCTL:  u64 = 0x2802;`

```
// Encodings of fields not declared at file top (kept local so the
// module's public surface doesn't grow for diagnostic-only use).
```

## L1588-1589 · `const HOST_SS_SEL:   u64 = 0x0C04;`

```
// Additional host fields: any HOST_* canonicity/coherency failure
// also fires reason 33 (host-state checked together with guest).
```

## L1611-1613 · `const IA32_VMX_CR0_FIXED0_MSR: u32 = 0x486;`

```
// VMX-fixed MSRs: tell us which CR0/CR4 bits the CPU rejects. If
// guest CR0/CR4 has a bit cleared that FIXED0 says must-be-1, or
// a bit set that FIXED1 says must-be-0, entry fails reason 33.
```

## L1675-1679 · `kprintln!("[vmx-fail] CS   sel={:#06x} base={:#018x} lim={:#x} ar={:#x} (L={} D={} unusable={})",`

```
// Segment-by-segment dump. AR bit 16 = unusable (segment skipped
// by VMX consistency checks). For IA-32e mode the load-bearing
// checks are: CS.L vs CS.D mutex; SS.RPL == CS.RPL even when SS
// null; TR type = 11 (busy 64-bit TSS); LDTR S=0 if usable;
// DS/ES/FS/GS S=1 + P=1 if usable. SDM Vol 3 §26.3.1.4.
```

## L1702-1703 · `kprintln!("[vmx-fail] IA32_DEBUGCTL={:#018x}  IA32_PAT={:#018x}",`

```
// MSR-like guest fields. Reserved-bit issues here trip §26.3.1.1
// even when the visible CR/EFER triad is consistent.
```

## L1711-1712 · `kprintln!("[vmx-fail] HOST CR0={:#018x} CR3={:#018x} CR4={:#018x}",`

```
// Host state — a HOST_* canonicity/coherency issue can also fail
// entry (basic reason stays 33 because guest is loaded first).
```

## L1717-1719 · `kprintln!("[vmx-fail] VM_EXIT_REASON raw={:#x} (bit31={} = entry-fail flag)",`

```
// Raw VM_EXIT_REASON. Bit 31 = VM-entry-failure indication; bit 12
// = pending MTF. Confirms this really is an entry-fail (vs an exit
// we misclassified).
```

## L1734-1738 · `let pin_live   = vmread(PIN_BASED_VM_EXEC_CONTROL).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);`

```
// Live execution-control fields + their fixed-bit MSRs. Reveals
// forced-on bits we don't expect (e.g. VPID, NMI-exiting, virtual-
// NMIs). For VMX-entry-fail with all guest-state checks passing,
// a control-field rule like "VPID enabled + VPID field = 0" is
// the remaining culprit-class.
```

## L1758-1759 · `let eptp_f      = vmread(EPT_POINTER).unwrap_or(0xDEAD_DEAD_DEAD_DEAD);`

```
// EPT/IO/MSR pointer fields. If any of these are corrupt/zero
// while their enable bit is on, entry fails reason 33.
```

## L1777-1783 · `pub fn sync_entry_ia32e_with_efer() -> Result<(), &'static str> {`

```
/// Sync VM_ENTRY_CONTROLS' IA-32e-mode-guest bit to GUEST_IA32_EFER.LMA.
/// VMX requires the two to match at entry: if the guest is currently
/// in long mode (LMA=1, set automatically by the CPU when CR0.PG=1
/// and EFER.LME=1), entry control bit 9 must be 1. If guest is in
/// 32-bit mode, bit 9 must be 0. Callers invoke this between
/// VM-exits and the next VMRESUME so the control tracks the live
/// guest mode across long-mode transitions.
```

