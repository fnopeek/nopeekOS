# `kernel/src/microvm/cpu/svm/enable.rs` @ 5e0102684

## L1-25 · `use super::{lapic, npt, rdmsr, vmcb, wrmsr};`

```
//! SVM root-mode entry + VMRUN loops — 12.1.0b-svm through 12.1.1c-svm.
//!
//! Two consumer-facing entry points:
//!   - `enable_and_test()` — real-mode HLT/IOIO substrate test.
//!     Allocates a VMCB, host-save area, IOPM, MSRPM, NPT (identity)
//!     and a 5-byte stub `mov al,'O'; out 0x80,al; hlt`, enables
//!     EFER.SVME, VMRUNs, returns the resulting exit-code.
//!   - `run_linux()` — Linux Boot Protocol 32-bit entry. Allocates
//!     256 MB guest RAM, builds a non-identity NPT window, copies
//!     bzImage parts in via `microvm::linux::bzimage`, and dispatches
//!     a VMRUN/VMEXIT loop with handlers for HLT / IOIO / CPUID /
//!     MSR / INTR / SHUTDOWN / NPF.
//!
//! All allocations are *kept* (never freed) per call. EFER.SVME is
//! left set across calls (harmless — just enables SVM instructions).
//!
//! Reference: AMD64 APM Vol. 2 §15.4 (Enabling SVM), §15.5 (VMRUN
//! Instruction), §15.17 (Global Interrupt Flag), §15.10 (I/O
//! Intercepts), §15.11 (MSR Intercepts), §15.25 (Nested Paging).
//!
//! Compared to VMX 12.1.0b: SVM has no separate VMXON region — the
//! host-save area is conceptually similar, but selected by an MSR
//! rather than a region pointer. There's also no VMCLEAR/VMPTRLD
//! dance: VMRUN takes the VMCB physical address as an operand
//! (loaded into RAX), so multiple VMCBs can coexist trivially.
```

## L34-39 · `static VM_BIG_LOCK: spin::Mutex<()> = spin::Mutex::new(());`

```
/// Big-VM lock (guest SMP). Held by a vCPU only around its post-VMRUN
/// device/MMIO/IO handling — NEVER across `vmrun` or a fiber yield — so
/// the BSP and an AP vCPU serialize all access to the shared `VmShared`
/// (device model + guest memory) while their VMRUNs run truly in
/// parallel on separate host cores. Uncontended (and never even taken)
/// until an AP is admitted (`AP_ACTIVE`).
```

## L42-45 · `pub static AP_ACTIVE: AtomicBool = AtomicBool::new(false);`

```
/// True once a second vCPU (AP) shares this VM. While false the BSP runs
/// exactly as the single-vCPU path did — `VM_BIG_LOCK` is not taken, so
/// the hot loop is byte-identical. Set by the orchestration layer when it
/// spawns the AP fiber, cleared after the last vCPU exits.
```

## L54 · `const IA32_EFER: u32 = 0xC000_0080;`

```
// ── MSRs (APM Vol 2 §15.4) ─────────────────────────────────────────
```

## L56-57 · `const IA32_EFER: u32 = 0xC000_0080;`

```
/// Extended Feature Enable Register. Bit 12 = SVME (SVM enable).
/// Architectural MSR since K8.
```

## L61-63 · `const VM_HSAVE_PA: u32 = 0xC001_0117;`

```
/// VM_HSAVE_PA — physical address of the host-save area. The CPU
/// writes the host's state here on VMRUN and reads it back on
/// VMEXIT. Must be 4 KB aligned, page-sized region.
```

## L66 · `pub fn enable_and_test() -> Result<vmcb::LaunchOutcome, &'static str> {`

```
// ── Public entry point ─────────────────────────────────────────────
```

## L68-75 · `pub fn enable_and_test() -> Result<vmcb::LaunchOutcome, &'static str> {`

```
/// Run a trivial real-mode HLT-loop guest in a fresh SVM VM.
/// Returns `Ok(LaunchOutcome { exit_reason: 0x78, .. })` (= HLT
/// intercept) on success.
///
/// Allocates everything fresh on each call; nothing is freed. This
/// matches `vmx::enable::enable_and_test`, and at the rate
/// substrate-test runs (a couple of times per session) the leak is
/// negligible.
```

## L77-79 · `enable_efer_svme()?;`

```
// 1. EFER.SVME on. APM §15.4: "VMRUN faults with #UD if EFER.SVME=0".
//    The VM_HSAVE_PA host-save area is programmed per-core lazily on
//    the first VMRUN (`ensure_core_host_state` in `run_guest_once`).
```

## L82 · `let iopm_phys = memory::allocate_contiguous(3)`

```
// 3. IOPM — 3 contiguous frames (12 KB), zeroed = no I/O traps.
```

## L85 · `unsafe { core::ptr::write_bytes(iopm_phys as *mut u8, 0, 3 * 4096); }`

```
// SAFETY: freshly allocated, identity-mapped, exclusive.
```

## L88 · `let msrpm_phys = memory::allocate_contiguous(2)`

```
// 4. MSRPM — 2 contiguous frames (8 KB), zeroed = no MSR traps.
```

## L91 · `unsafe { core::ptr::write_bytes(msrpm_phys as *mut u8, 0, 2 * 4096); }`

```
// SAFETY: as above.
```

## L94-97 · `unsafe { (iopm_phys as *mut u8).add(0x10).write_volatile(0x01); }`

```
// 5. IOPM bit for port 0x80 — substrate-test guest writes there.
//    APM §15.10.1: bit `port % 8` of byte `port / 8`. Port 0x80
//    → byte 16, bit 0 → IOPM[16] |= 0x01.
// SAFETY: IOPM was just allocated + zeroed; exclusive ours.
```

## L100-106 · `let stub_phys = memory::allocate_frame()`

```
// 6. Guest stub page — 4 KB, write 32-bit prot-mode "OK" stub:
//    B0 4F      mov al, 0x4F  ('O')
//    E6 80      out 0x80, al
//    F4         hlt
//    Five bytes total. With IOIO_PROT + IOPM bit set, the OUT
//    triggers VMEXIT_IOIO (0x7B). Without IOIO intercept (the
//    earlier 12.1.0b path), it would fall through to HLT.
```

## L109 · `unsafe {`

```
// SAFETY: exclusive, identity-mapped.
```

## L113 · `p.add(0).write_volatile(0xB0); // mov al, imm8`

```
// mov al, imm8
```

## L114 · `p.add(1).write_volatile(0x4F); // 'O'`

```
// 'O'
```

## L115 · `p.add(2).write_volatile(0xE6); // out imm8, al`

```
// out imm8, al
```

## L116 · `p.add(3).write_volatile(0x80); // port 0x80`

```
// port 0x80
```

## L117 · `p.add(4).write_volatile(0xF4); // hlt`

```
// hlt
```

## L120-121 · `let npt_root = npt::allocate_identity_npt()?;`

```
// 7. NPT — identity-map 256 MB of guest physical to host physical
//    via 2 MB pages. NCR3 points at the PML4.
```

## L124-126 · `let vmcb = alloc::boxed::Box::new(vmcb::Vmcb::zeroed());`

```
// 8. VMCB — 4 KB, allocated as Vmcb on the kernel heap. Since
//    the kernel heap lies inside the identity-mapped region,
//    the host-virtual address equals the host-physical address.
```

## L133-135 · `let mut regs = vmcb::GuestRegs::default();`

```
// 9. GuestRegs slot for the asm shim. The "OK" stub uses RAX
//    only (CPU saves/loads via VMCB.SAVE.RAX); the other 14
//    GPRs stay zero.
```

## L138-145 · `core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);`

```
// 10. CLGI + VMRUN + STGI. VMRUN takes the VMCB phys in RAX.
//    APM §15.17: VMRUN requires GIF=0 (else #UD). The CPU sets
//    GIF=1 inside the guest, then clears it again on VMEXIT,
//    so we explicitly STGI on return.
// Memory fence: setup_vmcb wrote 200+ scattered bytes into VMCB;
// ensure they're visible to the CPU's VMRUN consistency-check
// path before we hand off. Empirically without this on KVM
// nested SVM the VMCB read-side races into stale zeros.
```

## L148-149 · `let mut hf = crate::microvm::cpu::FpuArea::boxed();`

```
// FPU areas for the in-asm save/restore (the stub doesn't touch
// FPU, but run_guest_once unconditionally brackets vmrun).
```

## L158 · `fn enable_efer_svme() -> Result<(), &'static str> {`

```
// ── Private plumbing ───────────────────────────────────────────────
```

## L160-161 · `fn enable_efer_svme() -> Result<(), &'static str> {`

```
/// Set EFER.SVME if not already set. Once on, stays on for the life
/// of this CPU — no harm in idempotent re-set.
```

## L163-164 · `let efer = unsafe { rdmsr(IA32_EFER) };`

```
// SAFETY: EFER is architectural since K8. SVME bit is the only
// toggle; other bits (LME, LMA, NXE) we leave untouched.
```

## L167-168 · `unsafe { wrmsr(IA32_EFER, efer | EFER_SVME); }`

```
// SAFETY: setting SVME only enables SVM instructions; no
// observable side-effect until the first VMRUN.
```

## L174-175 · `const MAX_CORES: usize = 256;`

```
/// Max host cores we track per-core SVM state for (matches the
/// `[_; 256]` arrays in `smp::per_core`).
```

## L178-193 · `static HOST_SAVE_FRAMES: [AtomicU64; MAX_CORES] =`

```
/// Per-core SVM host state, indexed by core id. Both are
/// per-PHYSICAL-CORE resources, so a single global frame is a
/// correctness bug the moment two cores run VMRUN concurrently
/// (guest SMP / multiple microvms):
///   * `HOST_SAVE_FRAMES` — the VM_HSAVE_PA target. VM_HSAVE_PA is a
///     per-core MSR; the CPU writes host state there on VMRUN and
///     reads it on VMEXIT. Two cores pointing at one frame clobber
///     each other.
///   * `HOST_EXTRA_FRAMES` — the `vmsave`/`vmload` frame for host
///     FS/GS/KernelGS/TR/LDTR/SYSCALL MSRs (`vmrun` preserves none —
///     APM Vol 2 §15.5.2). Concurrent vmsave/vmload against one frame
///     corrupts FS/GS → process-agnostic guest corruption (the
///     historical "A2 regression", now cross-core).
/// Each slot is lazily allocated the first time its core runs a VMRUN
/// (`ensure_core_host_state`), so any core — including future AP vCPU
/// fibers — is set up automatically without a separate init path.
```

## L199-205 · `fn ensure_core_host_state() -> u64 {`

```
/// Ensure THIS core has SVM enabled, its VM_HSAVE_PA programmed to a
/// private host-save frame, and a private host-extra-save frame
/// allocated; return the extra-save frame phys. Idempotent per core
/// (one atomic load + branch on the hot path). MUST run on the core
/// that will execute VMRUN — VM_HSAVE_PA is a per-core MSR. No
/// intra-core race (one core runs this serially); distinct cores
/// touch distinct slots.
```

## L209 · `let _ = enable_efer_svme();`

```
// EFER.SVME on this core (idempotent).
```

## L212 · `unsafe { core::ptr::write_bytes(p as *mut u8, 0, 4096); }`

```
// SAFETY: freshly allocated, identity-mapped, exclusive.
```

## L214-215 · `unsafe { wrmsr(VM_HSAVE_PA, p); }`

```
// SAFETY: VM_HSAVE_PA is architectural + per-core; p is page-aligned
// and identity-mapped. Programs THIS core's host-save area.
```

## L224 · `unsafe { core::ptr::write_bytes(p as *mut u8, 0, 4096); }`

```
// SAFETY: freshly allocated, identity-mapped, exclusive.
```

## L230-238 · `fn setup_vmcb(`

```
/// Initialize a VMCB for the substrate-test guest:
///   * 32-bit protected mode (CR0.PE=1, paging off — segmentation
///     only). CS.base = stub_phys, RIP = 0 → execution starts at
///     the 5-byte "OK" stub: mov al,0x4F; out 0x80,al; hlt.
///   * Intercept HLT + I/O (so VMEXIT fires on `out 0x80, al`).
///   * Intercept VMRUN (mandatory — guest can't run nested SVM
///     because we don't support nested-nested in 12.1).
///   * NPT on (12.1.1a-svm), guest paging off so guest physical =
///     guest linear and NPT translates straight through.
```

## L246 · `vmcb.write_u32(`

```
// ── Control area ──────────────────────────────────────────────
```

## L255-256 · `vmcb.write_u8(vmcb::OFF_TLB_CTL, vmcb::tlb_flush_guest());`

```
// First entry only: drop what a previous run left under ASID 1 on this
// core. `run_slice` clears it after the first VMRUN (see TLB_CTL there).
```

## L258 · `vmcb.write_u64(vmcb::OFF_NESTED_CTL, 1); // NP_ENABLE — NPT on`

```
// NP_ENABLE — NPT on
```

## L261-263 · `vmcb.write_segment(`

```
// ── State save area: 32-bit prot mode, CS at stub_phys ────────
// CS.base = stub_phys; RIP = 0 → fetch starts at stub_phys + 0
// = the `mov al, 0x4F` byte. CS limit=4 GB (G=1, limit=0xFFFFFFFF).
```

## L266 · `);`

```
/* selector */
```

## L267 · `);`

```
/* attrib   */
```

## L268 · `);`

```
/* limit    */
```

## L269 · `);`

```
/* base     */
```

## L271 · `for off in [`

```
// SS/DS/ES: 32-bit data, base=0, limit=4 GB — flat data segments.
```

## L280-282 · `vmcb.write_u32(vmcb::OFF_SAVE_GDTR + 4, 0);`

```
// GDTR / IDTR: 32-bit prot mode normally consults GDTR for
// segment descriptor reloads, but our segments don't reload —
// we set everything in the VMCB once. Leave zero.
```

## L288 · `vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x11); // PE=1, ET=1`

```
// CR registers: 32-bit prot mode, paging off.
```

## L289 · `vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x11); // PE=1, ET=1`

```
// PE=1, ET=1
```

## L293-295 · `vmcb.write_u64(vmcb::OFF_SAVE_EFER, EFER_SVME);`

```
// EFER: APM §15.5.1 requires guest EFER.SVME=1 in the VMCB,
// even when the guest itself doesn't run any SVM instructions.
// (Yes, the guest "owns" SVME from its CR-state perspective.)
```

## L298 · `vmcb.write_u64(vmcb::OFF_SAVE_RFLAGS, 0x0000_0002);`

```
// RFLAGS: bit 1 must always be set (architecturally reserved).
```

## L305-307 · `vmcb.write_u64(vmcb::OFF_SAVE_G_PAT, 0x0007_0406_0007_0406);`

```
// G_PAT — guest Page Attribute Table. Standard reset value;
// matters only when paging is on, but VMRUN consistency check
// wants a sane value here.
```

## L311-322 · `fn run_guest_once(`

```
/// Execute one VMRUN against the supplied VMCB and return the
/// resulting exit-info. CLGI/STGI bracketing per APM §15.17.
///
/// Loads guest GPRs from `regs` before VMRUN, saves them back on
/// VMEXIT. RAX/RSP are auto-handled by the CPU via VMCB.SAVE.{RAX,
/// RSP} + the host-save area, so they're not in `regs`.
///
/// The struct pointer survives across VMRUN by being pushed onto
/// the stack: VMRUN restores host RSP from the host-save area on
/// VMEXIT, returning RSP to its post-push value. The pushed
/// struct pointer is then recovered after we spill all 14 guest
/// GPRs.
```

## L332-334 · `let xcr: [u64; 2] = [guest_xcr0, crate::microvm::cpu::guest_cpuid::host_xcr0()];`

```
// [guest, host] XCR0, switched inside the asm next to the FPU swap
// (KVM `kvm_load_guest_xsave_state`): the guest's XSETBV value must
// never stay live on the host, and the host's must not leak in.
```

## L337-338 · `let host_extra = ensure_core_host_state();`

```
// Per-core: VM_HSAVE_PA + host-extra-save frame for THIS core. Lazily
// sets up any core that runs VMRUN (BSP today, AP vCPU fibers later).
```

## L341-356 · `unsafe {`

```
// SAFETY: EFER.SVME is set (caller guarantee), VM_HSAVE_PA
// points at a valid host-save frame, the VMCB has been
// initialized to a state that passes APM §15.5.1 consistency
// checks. CLGI/STGI bracket the call so no host interrupts
// fire between save+VMRUN and post-VMEXIT state read.
//
// Register dance:
//   in: rdi = struct ptr, rsi = vmcb_phys
//   1. push rdi (struct ptr) — survives VMRUN via host-save RSP
//   2. mov rax, rsi (vmcb_phys into VMRUN operand reg)
//   3. load guest GPRs from struct, rdi LAST
//   4. CLGI; vmrun rax; STGI
//   5. spill all 14 guest GPRs to stack
//   6. recover struct ptr from stack[14*8 = 112]
//   7. pop guest GPRs (LIFO) into struct
//   8. discard struct ptr
```

## L359-360 · `"push rbp",`

```
// ── PROLOGUE: save host callee-saved (clobber_abi("C")
//              expects them preserved across the asm).
```

## L368-369 · `"push rdi",                     // struct ptr`

```
// Save struct ptr, vmcb_phys, host-extra-save phys below
// the callee-saved (push order = struct, vmcb, host).
```

## L370 · `"push rdi",                     // struct ptr`

```
// struct ptr
```

## L371 · `"push rsi",                     // vmcb_phys`

```
// vmcb_phys
```

## L372 · `"push rdx",                     // host_extra_save phys`

```
// host_extra_save phys
```

## L373 · `"push r8",                      // host_fpu  ptr`

```
// host_fpu  ptr
```

## L374 · `"push r9",                      // guest_fpu ptr`

```
// guest_fpu ptr
```

## L375 · `"push r10",                     // xcr pair ptr`

```
// xcr pair ptr
```

## L376-377 · `"mov rcx, [rsp + 16]",          // host_fpu`

```
// Stack now: [rsp+0]=xcr [+8]=guest_fpu [+16]=host_fpu
//   [+24]=host_extra [+32]=vmcb_phys [+40]=struct_ptr
```

## L379-386 · `"mov rcx, [rsp + 16]",          // host_fpu`

```
// ── FPU SAVE/RESTORE (KVM kvm_load_guest_fpu): vmrun
// preserves no x87/SSE/AVX/AVX-512. Must be in-asm,
// adjacent to vmrun — a +avx2-built kernel spills `ymm`
// anywhere between a Rust helper and the asm, clobbering
// the just-restored guest state. Mask = -1 (all
// XCR0-enabled components; guest owns XCR0 via XSETBV).
// Only GPR/vm* instructions sit between this xrstor and
// vmrun, and between vmrun and the paired xsave below.
```

## L387 · `"mov rcx, [rsp + 16]",          // host_fpu`

```
// host_fpu
```

## L390 · `"xsave64 [rcx]",                // save host FPU (host XCR0)`

```
// save host FPU (host XCR0)
```

## L391 · `"mov r11, [rsp + 0]",           // xcr pair`

```
// xcr pair
```

## L392 · `"mov rax, [r11]",               // guest XCR0`

```
// guest XCR0
```

## L398 · `"xsetbv",                       // guest XCR0 on`

```
// guest XCR0 on
```

## L400 · `"mov rcx, [rsp + 8]",           // guest_fpu`

```
// guest_fpu
```

## L403 · `"xrstor64 [rcx]",               // restore guest FPU (guest XCR0)`

```
// restore guest FPU (guest XCR0)
```

## L405 · `"mov rbx, [rdi +   0]",`

```
// ── ENTRY: load guest GPRs from struct ────────────────
```

## L418 · `"mov rsi, [rdi +  24]",         // rsi (was vmcb_phys input)`

```
// rsi (was vmcb_phys input)
```

## L419 · `"mov rdi, [rdi +  32]",         // rdi LAST`

```
// rdi LAST
```

## L421-424 · `"mov rax, [rsp + 24]",          // host_extra_save phys`

```
// ── HOST EXTRA-STATE SAVE + VMRUN + RESTORE ───────────
// vmsave host FS/GS/KernelGS/TR/LDTR/SYSCALL MSRs (vmrun
// does NOT preserve them); vmload them back on THIS core
// right after #VMEXIT, before any GS-relative host access.
```

## L425 · `"mov rax, [rsp + 24]",          // host_extra_save phys`

```
// host_extra_save phys
```

## L426 · `"vmsave rax",                   // save host FS/GS/TR/LDTR/MSRs`

```
// save host FS/GS/TR/LDTR/MSRs
```

## L427 · `"mov rax, [rsp + 32]",          // guest vmcb_phys`

```
// guest vmcb_phys
```

## L429-431 · `"sti",`

```
// Host IF=1 under GIF=0: nothing is taken here, but VMRUN saves
// IF=1, so a pending or arriving host interrupt exits the guest
// (V_INTR_MASKING: the host's IF gates physical interrupts).
```

## L433 · `"vmload rax",                   // load guest FS/GS/KernelGS/STAR/LSTAR/SFMASK/SYSENTER`

```
// load guest FS/GS/KernelGS/STAR/LSTAR/SFMASK/SYSENTER
```

## L435 · `"vmsave rax",                   // save guest's back into the guest VMCB`

```
// save guest's back into the guest VMCB
```

## L436 · `"mov rax, [rsp + 24]",          // host_extra_save phys`

```
// host_extra_save phys
```

## L437 · `"vmload rax",                   // restore host FS/GS/... while GIF=0 (atomic vs IRQs)`

```
// restore host FS/GS/... while GIF=0 (atomic vs IRQs)
```

## L438 · `"cli",                          // taken after entry_irqs_on, not mid-exit-path`

```
// taken after entry_irqs_on, not mid-exit-path
```

## L439 · `"stgi",                         // only NOW open the IRQ window — host state already correct`

```
// only NOW open the IRQ window — host state already correct
```

## L440-441 · `"push rbx",`

```
// After VMEXIT: rsp restored by CPU, all GPRs hold guest
// clobbers; host FS/GS restored by the vmload above.
```

## L443 · `"push rbx",`

```
// ── EXIT: spill 14 guest GPRs to stack ────────────────
```

## L458-460 · `"mov rcx, [rsp + 120]",         // guest_fpu`

```
// Stack now: r15..rbx (14=112B), xcr[+112], guest_fpu[+120],
//   host_fpu[+128], host_extra[+136], vmcb[+144],
//   struct_ptr[+152], host_callee_saved(6).
```

## L462-465 · `"mov rcx, [rsp + 120]",         // guest_fpu`

```
// ── FPU SAVE/RESTORE (paired exit half): no FP since
// vmexit (only vm*/push above). Save guest FPU, restore
// host's. Guest GPRs are on the stack now → rax/rcx/rdx
// free to clobber.
```

## L466 · `"mov rcx, [rsp + 120]",         // guest_fpu`

```
// guest_fpu
```

## L469 · `"xsave64 [rcx]",                // save guest FPU (guest XCR0)`

```
// save guest FPU (guest XCR0)
```

## L470 · `"mov r11, [rsp + 112]",         // xcr pair`

```
// xcr pair
```

## L471 · `"mov rax, [r11 + 8]",           // host XCR0`

```
// host XCR0
```

## L477 · `"xsetbv",                       // host XCR0 back`

```
// host XCR0 back
```

## L479 · `"mov rcx, [rsp + 128]",         // host_fpu`

```
// host_fpu
```

## L482 · `"xrstor64 [rcx]",               // restore host FPU (host XCR0)`

```
// restore host FPU (host XCR0)
```

## L484 · `"mov rax, [rsp + 152]",`

```
// Recover struct ptr (rax free to clobber).
```

## L487 · `"pop rcx", "mov [rax + 104], rcx",      // r15`

```
// Pop in reverse-push order, store at the right offset.
```

## L488 · `"pop rcx", "mov [rax + 104], rcx",      // r15`

```
// r15
```

## L489 · `"pop rcx", "mov [rax +  96], rcx",      // r14`

```
// r14
```

## L490 · `"pop rcx", "mov [rax +  88], rcx",      // r13`

```
// r13
```

## L491 · `"pop rcx", "mov [rax +  80], rcx",      // r12`

```
// r12
```

## L492 · `"pop rcx", "mov [rax +  72], rcx",      // r11`

```
// r11
```

## L493 · `"pop rcx", "mov [rax +  64], rcx",      // r10`

```
// r10
```

## L494 · `"pop rcx", "mov [rax +  56], rcx",      // r9`

```
// r9
```

## L495 · `"pop rcx", "mov [rax +  48], rcx",      // r8`

```
// r8
```

## L496 · `"pop rcx", "mov [rax +  40], rcx",      // rbp`

```
// rbp
```

## L497 · `"pop rcx", "mov [rax +  32], rcx",      // rdi (guest's)`

```
// rdi (guest's)
```

## L498 · `"pop rcx", "mov [rax +  24], rcx",      // rsi`

```
// rsi
```

## L499 · `"pop rcx", "mov [rax +  16], rcx",      // rdx`

```
// rdx
```

## L500 · `"pop rcx", "mov [rax +   8], rcx",      // rcx`

```
// rcx
```

## L501 · `"pop rcx", "mov [rax +   0], rcx",      // rbx`

```
// rbx
```

## L502 · `"add rsp, 48",                          // discard xcr,guest_fpu,host_fpu,host_extra,vmcb,struct`

```
// discard xcr,guest_fpu,host_fpu,host_extra,vmcb,struct
```

## L504 · `"pop r15",`

```
// Restore host callee-saved.
```

## L533 · `const EXIT_INTR: u64 = 0x060;`

```
// ── Linux launcher (12.1.1c-svm) ───────────────────────────────────
```

## L535-536 · `const EXIT_INTR: u64 = 0x060;`

```
// AMD VMEXIT codes used by the Linux loop. APM Vol 2 Appendix C lists
// the full set; we match against the ones we expect Linux to trigger.
```

## L566-567 · `fn inject_exception(vmcb: &mut vmcb::Vmcb, vector: u8, error_code: Option<u32>) {`

```
/// Queue a hardware exception for the next VMRUN (EVENTINJ type 3). RIP is
/// NOT advanced: the faulting instruction is the one reported.
```

## L576-577 · `fn xcr0_valid(v: u64) -> bool {`

```
/// XSETBV rules (SDM/APM, KVM `__kvm_set_xcr`): x87 always on, AVX needs SSE,
/// nothing beyond what the host itself has enabled in XCR0.
```

## L584-593 · `pub enum SliceOutcome {`

```
// ── Re-entrant VM context (Phase 12.4 step 1c — SVM mirror of 1a) ──
//
// Same decomposition as vmx::enable: open() / run_slice(budget) /
// close() so the Core-0 event loop can interleave Shade rendering
// between bounded slices (docs/archive/PHASE12_DISPLAY_BRIDGE.md R1). SVM is
// simpler than VMX: no VMXON/VMCS bracket — EFER.SVME stays on for
// the life of the kernel (by design, see module header) and the VMCB
// is a leaked Box, so close() is a no-op. Step 1c is
// behaviour-preserving: run_linux calls run_slice(u32::MAX) once,
// identical to the old run_linux_loop.
```

## L595 · `pub enum SliceOutcome {`

```
/// Outcome of one bounded slice of guest execution (SVM).
```

## L597-598 · `StillRunning,`

```
/// Budget exhausted, guest still running — caller may re-enter
/// immediately (busy guest).
```

## L600-602 · `Idle,`

```
/// Guest is idle (halted / waiting on its timer) — caller should
/// re-enter but may host-idle first so the dedicated core doesn't
/// spin VMRUN while the guest has nothing to do.
```

## L604 · `Exited(vmcb::LaunchOutcome),`

```
/// Guest exited (HLT / shutdown / NPF / idle / cap).
```

## L608-614 · `fn npf_kind(sh: &VmShared, gpa: u64) -> usize {`

```
/// State shared by ALL vCPUs of one microvm: the single guest address
/// space (RAM + NPT), the device model, and the host-tick/display
/// bookkeeping. Today the one vCPU owns it directly inside `VmContext`;
/// Stage 0c moves it behind a big-VM lock + shared handle so AP vCPU
/// fibers attach to it (guest SMP). Splitting it out now (Stage 0b) is a
/// pure, behaviour-preserving decomposition.
/// Which target a nested page fault at `gpa` hits (`cores` NPF breakdown).
```

## L633-637 · `guest_mem: &'static GuestMem,`

```
/// Shared handle to the active guest memory (owned by `guest_mem`'s
/// `ACTIVE_GM`, freed at close). A reference — NOT the owned `GuestMem` —
/// so the off-vCPU net backend can hold the same `&'static GuestMem` without
/// aliasing this `&mut VmShared` (the borrow governs the pointer, not the
/// `Sync` pointee).
```

## L639-640 · `npt_pml4: u64,`

```
/// NPT PML4 (= NCR3) phys — `close()` passes it to `npt::release`
/// to free demand-faulted frames + demand PTs + NPT tables.
```

## L642-644 · `guest_raw_base: u64,`

```
/// Base of the **contiguous boot-window** allocation (pre-2 MB-
/// align), + the IOPM/MSRPM frames — all freed in close(). The
/// demand region is freed via `npt::release`.
```

## L651 · `pit: crate::microvm::devices::pit8253::Pit,`

```
/// i8254 channel 0 — its output is IRQ0 on the PIC.
```

## L653-654 · `last_cfg_tick: u64,`

```
/// `ticks()` of the last virtio-gpu display config-change IRQ — debounces
/// the resize round-trip against a drag storm (UI timing, 250 ms).
```

## L656 · `last_reap_tick: u64,`

```
/// `ticks()` of the last NAT mapping reap.
```

## L663-665 · `const HALT_POLL_MIN_US: u64 = 5;`

```
/// Bounded adaptive halt-polling (KVM `halt_poll_ns` model) — see the VMX twin.
/// Idle vCPU polls its window for a wake before parking; grows on a caught wake,
/// shrinks on expiry. Replaces the old global `recently_active` spin. µs units.
```

## L669-671 · `pub struct Vcpu {`

```
/// Per-vCPU state: its own VMCB + register file + FPU areas, plus the
/// per-vCPU exit-handling bookkeeping. Each vCPU fiber owns one; guest
/// SMP = N of these against one shared `VmShared`.
```

## L673-678 · `apic_id: u8,`

```
/// This vCPU's physical (x)APIC ID — BSP = 0, APs = 1.. It must be
/// reported identically by CPUID (leaf 1 EBX[31:24], leaf 0xB/0x1F
/// EDX), the emulated LAPIC ID register, the IA32_APICBASE BSP bit
/// (set only when apic_id == 0), and the MP-table entry, or Linux's
/// topology code rejects the CPU. We virtualize it instead of passing
/// the host core's APIC ID through (which would differ per worker core).
```

## L680 · `vmcb: alloc::boxed::Box<vmcb::Vmcb>,`

```
/// Owned (not leaked) so it's reclaimed on drop — relaunch fix.
```

## L684-685 · `host_fpu: alloc::boxed::Box<crate::microvm::cpu::FpuArea>,`

```
/// Host/guest FPU (XSAVE) save areas — `vmrun` preserves neither.
/// See `cpu::FpuArea`. guest_fpu starts zeroed = FPU init state.
```

## L688-692 · `reinject: u64,`

```
/// Event that was mid-delivery when the last #VMEXIT hit (raw
/// VMCB.EXITINTINFO, identical encoding to EVENTINJ). Must be
/// re-injected on the next VMRUN or the guest loses an interrupt
/// mid-vectoring → corrupt guest state (AMD APM §15.20; KVM
/// `svm_complete_interrupts`). 0 = nothing pending.
```

## L696-698 · `lapic: LocalApic,`

```
/// Per-vCPU emulated local APIC (xAPIC MMIO @ 0xFEE00000). Inert
/// while the guest boots `nolapic`; drives the timer + (later) IPIs
/// once Linux enables it. See `svm::lapic`.
```

## L700 · `halt_poll_us: u64,`

```
/// Adaptive halt-poll window (µs), see `HALT_POLL_MIN_US`.
```

## L702-707 · `if_off_halts: u32,`

```
/// Consecutive HLT exits taken with guest RFLAGS.IF=0. The idle path
/// is always `sti; hlt` (IF=1); a sustained `cli; hlt` loop is Linux's
/// no-ACPI poweroff / panic path (we boot `acpi=off`, so there is no
/// ACPI shutdown signal). Crossing the threshold = the guest powered
/// off → exit the VM (tears the window down on browser close instead
/// of leaving a black idle window). Reset on any IF=1 HLT.
```

## L709 · `msrs: super::msr::GuestMsrs,`

```
/// Emulated MSR state (MTRR, SPEC_CTRL, HWCR …) — see `svm::msr`.
```

## L711-712 · `xcr0: u64,`

```
/// Guest XCR0 (reset value 1 = x87). Swapped around VMRUN in
/// `run_guest_once`; the host's XCR0 is never left in the guest's hands.
```

## L716-722 · `pub enum SharedRef {`

```
/// Handle to a microvm's `VmShared`. The BSP vCPU **owns** it, heap-boxed
/// so its address is stable while the BSP's `VmContext` moves on the fiber
/// stack; an AP vCPU (guest SMP, Stage 3b) holds `Borrowed` — a raw pointer
/// to the same box. `Deref`/`DerefMut` make every `self.shared.X` access
/// work unchanged for both. Concurrent access between vCPUs is serialized
/// by `VM_BIG_LOCK` (taken around post-VMRUN device handling), NOT by this
/// handle — it only resolves *which* `VmShared` a vCPU's exit-handler sees.
```

## L725-728 · `Borrowed(*mut VmShared),`

```
/// AP vCPU: aliases the BSP's box. Valid for the VM's lifetime — the
/// BSP frees the box only after the last vCPU has exited (last-one-out
/// refcount), so the pointer never dangles while an AP runs.
/// Constructed in Stage 3b-2 (AP spawn).
```

## L732-734 · `unsafe impl Send for SharedRef {}`

```
// SAFETY: SharedRef::Borrowed is a raw pointer; the type is only moved
// into AP fiber tasks, and all access to the pointee is serialized by
// VM_BIG_LOCK. The Send marker lets it cross into the spawned fiber.
```

## L748 · `SharedRef::Borrowed(p) => unsafe { &**p },`

```
// SAFETY: see the type-level comment — valid for the VM lifetime.
```

## L758 · `SharedRef::Borrowed(p) => unsafe { &mut **p },`

```
// SAFETY: as above; access is VM_BIG_LOCK-serialized.
```

## L764-766 · `pub struct VmContext {`

```
/// Persistent state of one Linux microvm across cooperative slices
/// (SVM backend). Core-agnostic (forward-compat contract #1). Composed
/// of `VmShared` (all-vCPU, behind `SharedRef`) + one `Vcpu`.
```

## L773-776 · `pub fn open(`

```
/// Enable SVM, set up the host-save area, build NPT, place the
/// guest image, configure the VMCB, pre-inject the UART RX FIFO.
/// Mirrors the old `run_linux` setup. No teardown-on-error needed:
/// SVM has no VMXOFF analogue; EFER.SVME staying on is intended.
```

## L785-786 · `enable_efer_svme()?;`

```
// EFER.SVME here; VM_HSAVE_PA + host-extra-save are programmed
// per-core on the first VMRUN (`ensure_core_host_state`).
```

## L799-800 · `let gm = crate::microvm::devices::guest_mem::set_active(gm);`

```
// Install as the active guest memory (out of VmShared); `gm` is now the
// shared `&'static` handle, also reachable by the off-vCPU net backend.
```

## L803-805 · `let iopm_phys = memory::allocate_contiguous(3)`

```
// IOPM: 12 KB all-ones = trap every port. Linux touches dozens
// of unique ports during boot (UART, PIC, PIT, RTC, …).
// Cheaper to trap-all + handle the boring ones in the loop.
```

## L808 · `unsafe { core::ptr::write_bytes(iopm_phys as *mut u8, 0xFF, 3 * 4096); }`

```
// SAFETY: freshly allocated, identity-mapped, exclusive.
```

## L811-813 · `let msrpm_phys = memory::allocate_contiguous(2)`

```
// MSRPM: intercept everything except the VMLOAD/VMSAVE-switched set
// (KVM `svm_recalc_msr_intercepts`). An all-zero map would hand the
// guest the host's MTRRs, TSC, SYSCFG and microcode loader.
```

## L816 · `unsafe { super::msr::init_msrpm(msrpm_phys); }`

```
// SAFETY: freshly allocated, identity-mapped, exclusive 8 KB.
```

## L824-825 · `let mut regs = vmcb::GuestRegs::default();`

```
// Initial GPRs: ESI = boot_params_phys per Linux 32-bit boot
// protocol; the rest zero.
```

## L835-837 · `core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);`

```
// Memory fence — see lesson 2 in project_svm_bringup.md. The
// VMCB writes above must be visible to the CPU's VMRUN
// consistency-check path.
```

## L840-841 · `lapic::reset_posted();`

```
// Fresh IPI state for this VM (IPI_PENDING is a static now, not a
// VmShared field, so it isn't auto-reset when a new VmShared is built).
```

## L843-844 · `crate::microvm::devices::net_backend::reset();`

```
// virtio-net lives in net_backend (a static, out of VmShared) so the
// off-vCPU backend can own it; re-arm it to power-on state per VM.
```

## L858 · `let mut pic = crate::microvm::devices::pic8259::Pic8259::new();`

```
// The I/O APIC's ID follows the vCPUs' in the MP table.
```

## L868 · `apic_id: 0, // BSP`

```
// BSP
```

## L886-893 · `pub fn close(&mut self) {`

```
/// Free the per-VM frame allocations so the VM can be relaunched
/// in the same boot. The VMCB is an owned `Box` and is reclaimed
/// when the VmContext drops (no explicit free here). EFER.SVME
/// stays on for the life of the kernel by design (no analogue to
/// VMXOFF).
///
/// B3: `npt::release` now walks + frees the demand frames, demand
/// PTs, and NPT tables (no longer leaked).
```

## L895-898 · `crate::microvm::devices::net_dataplane::stop_worker();`

```
// Stop the off-vCPU net backend FIRST: it holds &'static GuestMem via
// guest_mem::active(); clear_active() below frees it, so the worker must
// have stopped touching it. Bounded-waits for the fiber to exit;
// idempotent (the teardown path calls it again).
```

## L900-904 · `self.shared.pci.virtio_blk.save();`

```
// Persist the home image to npkFS BEFORE freeing. close() is
// reached on EVERY teardown — crucially the Mod+Q window-close
// path (VM_CLOSE_REQUESTED → break → close), where run_slice's
// own loop-end save() never runs (it returned StillRunning).
// Without this the browser profile is lost on every Mod+Q.
```

## L906 · `npt::release(self.shared.npt_pml4, self.shared.guest_mem.len());`

```
// Demand-faulted frames + demand PTs + NPT tables.
```

## L908 · `memory::deallocate_contiguous(`

```
// Contiguous boot window.
```

## L915-917 · `crate::microvm::devices::guest_mem::clear_active();`

```
// Free the active GuestMem (held outside VmShared). Safe here: the page
// tables are released above and all vCPUs + the net backend have stopped
// by the time close() runs, so no handle is live.
```

## L922-924 · `fn alloc_guest_ram_and_npt(guest_bytes: u64) -> Result<(u64, u64, u64), &'static str> {`

```
/// B3: allocate only the **contiguous boot window** (256 MiB or the
/// whole guest if smaller); `[boot, guest_bytes)` is demand-paged
/// 4 KB. Returns `(boot_base, npt_root=ncr3=pml4, boot_raw_base)`.
```

## L933-937 · `fn setup_vmcb_linux(`

```
/// Configure a VMCB for Linux 32-bit boot protocol entry.
/// Differs from `setup_vmcb` (substrate test) in that:
///   * CS.base = 0 (Linux is at GPA `entry_rip`, not relative to CS).
///   * RIP = entry_rip (typically 0x100000 = code32_start).
///   * Wider intercept set: HLT + IOIO + MSR + CPUID + INTR + SHUTDOWN.
```

## L945 · `vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, vmcb::LINUX_MISC1);`

```
// ── Control area ──────────────────────────────────────────────
```

## L953-954 · `vmcb.write_u8(vmcb::OFF_TLB_CTL, vmcb::tlb_flush_guest());`

```
// First entry only: drop what a previous run left under ASID 1 on this
// core. `run_slice` clears it after the first VMRUN (see TLB_CTL there).
```

## L956 · `vmcb.write_u64(vmcb::OFF_NESTED_CTL, 1); // NP_ENABLE`

```
// NP_ENABLE
```

## L959-962 · `vmcb.write_segment(`

```
// ── State save area: 32-bit prot mode flat segments ─────────────
// CS.base = 0; RIP = entry_rip → fetch starts at GPA entry_rip,
// which our NPT window maps to host_base + entry_rip = the
// protected-mode kernel image copied by bzimage::load_into_guest_ram.
```

## L965 · `);`

```
/* selector */
```

## L966 · `);`

```
/* attrib   */
```

## L967 · `);`

```
/* limit    */
```

## L968 · `);`

```
/* base     */
```

## L973 · `vmcb.write_u32(vmcb::OFF_SAVE_GDTR + 4, 0);`

```
// GDTR/IDTR limits — leave zero, Linux startup_32 reloads its own.
```

## L979 · `vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x11); // PE=1, ET=1`

```
// CR registers: 32-bit prot mode, paging off.
```

## L980 · `vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x11); // PE=1, ET=1`

```
// PE=1, ET=1
```

## L984-985 · `vmcb.write_u64(vmcb::OFF_SAVE_EFER, EFER_SVME);`

```
// EFER: APM §15.5.1 mandates SVME=1 in guest VMCB even when the
// guest itself doesn't run SVM instructions.
```

## L988 · `vmcb.write_u64(vmcb::OFF_SAVE_RFLAGS, 0x0000_0002); // bit 1 reserved-1`

```
// bit 1 reserved-1
```

## L996-1003 · `fn setup_vmcb_ap(`

```
/// Configure an AP vCPU's VMCB for a SIPI start (guest SMP, Stage 3b):
/// 16-bit real mode at CS = `(vector<<8):0000`, i.e. physical entry
/// `vector<<12`, which is where Linux's BSP copied the real-mode AP
/// trampoline. The trampoline takes the AP to protected→long mode and
/// into `start_secondary`. The control area mirrors `setup_vmcb_linux`
/// and **shares** the BSP's NCR3 / IOPM / MSRPM (one guest address space,
/// one device intercept policy). The NPT only gains entries while the
/// guest runs, so the vCPUs need no cross-core NPT shootdown.
```

## L1011 · `vmcb.write_u32(vmcb::OFF_INTERCEPT_MISC1, vmcb::LINUX_MISC1);`

```
// ── Control area — identical to setup_vmcb_linux ────────────────
```

## L1019 · `vmcb.write_u64(vmcb::OFF_NESTED_CTL, 1); // NP_ENABLE`

```
// NP_ENABLE
```

## L1022 · `let cs_base = (sipi_vector as u64) << 12;`

```
// ── State save area: 16-bit real mode, SIPI entry ──────────────
```

## L1026 · `);`

```
/* selector */
```

## L1027 · `);`

```
/* attrib   */
```

## L1028 · `);`

```
/* limit    */
```

## L1029 · `);`

```
/* base     */
```

## L1040 · `vmcb.write_u32(vmcb::OFF_SAVE_GDTR + 4, 0);`

```
// GDTR/IDTR limits — leave zero; the trampoline reloads its own.
```

## L1046-1047 · `vmcb.write_u64(vmcb::OFF_SAVE_CR0, 0x6000_0010);`

```
// CR registers: architectural INIT/reset state (real mode, paging off,
// caches off — Linux's AP path re-enables caching via MTRR init).
```

## L1052-1053 · `vmcb.write_u64(vmcb::OFF_SAVE_EFER, EFER_SVME);`

```
// EFER: SVME mandatory (APM §15.5.1); no LME — the AP enters in real
// mode and its own trampoline sets LME/LMA when it builds long mode.
```

## L1065-1071 · `pub fn open_ap(`

```
/// Build an AP vCPU that **shares** an already-open BSP's `VmShared`
/// (guest SMP, Stage 3b). `shared` aliases the BSP's heap-boxed
/// `VmShared` (valid for the VM lifetime via the last-one-out
/// refcount); `sipi_vector` is the SIPI start page; `apic_id` is the
/// AP's id (1..). No guest RAM / NPT / device allocation here — those
/// belong to the BSP and are shared. EFER.SVME is enabled on this
/// (the AP's) physical core.
```

## L1079-1082 · `let (iopm_phys, msrpm_phys, npt_root) = {`

```
// Read the shared substrate fields (iopm/msrpm/NPT root — set once
// at BSP open, never mutated). Take VM_BIG_LOCK: by now AP_ACTIVE is
// set, so the BSP may be accessing `*shared` under the lock — holding
// it here keeps the `&*shared` borrow from aliasing the BSP's `&mut`.
```

## L1085-1086 · `let s = unsafe { &*shared };`

```
// SAFETY: `shared` is valid for the VM lifetime (see SharedRef),
// and the lock excludes any concurrent `&mut` to `*shared`.
```

## L1119-1120 · `pub fn shared_ptr(&mut self) -> *mut VmShared {`

```
/// Raw pointer to this VM's shared state, for an AP vCPU to alias
/// (`open_ap`). Only valid to call on the BSP's `Owned` context.
```

## L1125-1126 · `pub fn next_timer_deadline_tsc(&self) -> Option<u64> {`

```
/// Host TSC of this vCPU's next timer event — the LAPIC timer, and on the
/// BSP the PIT — the deadline a blocked vCPU parks until (KVM's hrtimer).
```

## L1135 · `if self.shared.pci.virtio_snd.playing() {`

```
// A playing sound stream is serviced every millisecond.
```

## L1147-1148 · `fn collect_device_irqs(&mut self) {`

```
/// Feed every device line and the PIT into the PIC (BSP: in PIC mode the
/// device lines are wired there). Lock held by the caller when APs run.
```

## L1156 · `if crate::microvm::devices::gpu_backend::take_resume(crate::interrupts::rdtsc())`

```
// vblank: a paused controlq runs its next frame (virtio-gpu IRQ 9).
```

## L1175-1176 · `if crate::microvm::devices::gpu_backend::d4_pending()`

```
// Live resize (D4): disconnect, then reconnect after 100 ms; a new
// cycle at most every 250 ms while the window is dragged.
```

## L1194 · `if now != sh.last_reap_tick {`

```
// NAT mapping reaper: an idle scan, once per 10 ms at most.
```

## L1201-1202 · `fn pending_interrupt(&mut self) -> Option<bool> {`

```
/// `kvm_cpu_has_interrupt`: the PIC's INTR if this vCPU takes it (LINT0 in
/// ExtINT, or no LAPIC), else a LAPIC vector above PPR.
```

## L1217-1220 · `fn inject_pending_event(&mut self) {`

```
/// `inject_pending_event`, before every VMRUN: re-inject what was cut off
/// mid-delivery, otherwise the highest-priority deliverable interrupt; if
/// the guest cannot take it now (IF=0, interrupt shadow, or an exception
/// already queued), open the interrupt window so it exits the moment it can.
```

## L1243 · `let vector = if from_pic {`

```
// `kvm_cpu_get_interrupt`: ExtINT first, then the LAPIC.
```

## L1256 · `fn event_pending(&mut self) -> bool {`

```
/// Anything deliverable right now (takes the lock + collects on the BSP).
```

## L1264-1269 · `fn halt_poll(&mut self) -> bool {`

```
/// `kvm_vcpu_halt` with adaptive halt-polling (`halt_poll_ns`): after a
/// guest HLT, true if an event arrived within the poll window (resume at
/// once), false to block. The spin watches only lock-free signals — this
/// core's kick generation (every cross-core producer kicks), a posted IPI,
/// the worker's RX IRQ and the timer deadlines; a hit is confirmed by the
/// next entry's `inject_pending_event`.
```

## L1296-1297 · `self.event_pending()`

```
// Last look after the window: a producer that fired while we stopped
// polling must not wait for the park.
```

## L1302-1303 · `fn enable_irq_window(vmcb: &mut vmcb::Vmcb) {`

```
/// `svm_set_vintr` (`svm_enable_irq_window`): request a VINTR exit the moment
/// the guest can take an interrupt — V_IRQ at top priority, VINTR intercepted.
```

## L1313 · `fn clear_irq_window(vmcb: &mut vmcb::Vmcb) {`

```
/// `svm_clear_vintr`.
```

## L1325-1327 · `struct SerialState {`

```
/// Per-guest serial UART state across exits. Mirrors `vmx::enable
/// ::SerialState` but lives in the SVM tree to keep the two backends
/// independently evolvable.
```

## L1335-1337 · `halt_observed: bool,`

```
/// Set on the first `reboot: System halted` / `Power down` line — the
/// guest shut itself down (LibreWolf X → cage exit → PID-1 halt). Drives
/// the auto-close-and-save so the user doesn't need a second Mod+Q.
```

## L1339-1341 · `rx: [u8; 128],`

```
/// Phase 12.1.4-svm — RX FIFO. Pre-injected by the host before
/// VMRUN; drained when the guest reads RBR (0x3F8 IN). LSR.DR
/// (bit 0) on 0x3FD IN reflects `rx_pos < rx_n`.
```

## L1432-1435 · `fn scan_for_shutdown(&mut self, n: usize) {`

```
/// Detect a clean guest self-shutdown (`reboot: System halted` from PID-1
/// `halt`, or `Power down`). On the first hit, ask the run loop to take the
/// same clean exit as a user Mod+Q (break → save → window close) instead of
/// spinning forever on the guest's `cli;hlt`.
```

## L1438-1440 · `let line = &self.line[..n];`

```
// Require the `reboot: ` prefix (kernel/reboot.c only). PID-1 mirrors
// cage/moz app logs to /dev/kmsg → serial, so a bare "Power down"
// substring could false-trigger; the prefix can't appear in app text.
```

## L1453 · `fn line_contains(hay: &[u8], needle: &[u8]) -> bool {`

```
/// True if `hay` contains `needle` as a contiguous byte substring.
```

## L1459-1460 · `impl VmContext {`

```
/// Linux VMRUN/VMEXIT loop. Caps at MAX_ITERATIONS to bound the
/// shell's response time when the guest never makes progress.
```

## L1462-1465 · `pub fn run_slice(&mut self, budget: u32) -> Result<SliceOutcome, &'static str> {`

```
/// Run the guest for up to `budget` VMEXITs, or until it exits.
/// `Ok(StillRunning)` = budget hit, re-enterable (step 1b);
/// `Ok(Exited(o))` = guest left; `Err` = fault. Body is the old
/// `run_linux_loop` verbatim, `self.`-scoped.
```

## L1474-1475 · `const SLICE_MS: u64 = 3;`

```
// Wall-clock slice cap: return to the fiber scheduler every few ms so the
// core's other fibers run; the exit budget bounds boot bursts.
```

## L1480 · `let host_core = crate::smp::per_core::current_core_id();`

```
// Publish this vCPU's host core so a sender can kick it.
```

## L1496-1497 · `let entry_deadline =`

```
// The guest's next timer, or the slice end, as a host one-shot on this
// core: its fire is the exit that delivers the tick on time.
```

## L1504 · `let hf: *mut crate::microvm::cpu::FpuArea = &mut *self.vcpu.host_fpu;`

```
// Host↔guest FPU save/restore is embedded in run_guest_once's asm.
```

## L1518 · `if exit == EXIT_INTR {`

```
// Guest-RIP profiler: EXIT_INTR samples a running guest.
```

## L1535-1536 · `self.vcpu.vmcb.write_u64(vmcb::OFF_EVENT_INJ, 0);`

```
// KVM clears control.event_inj after every run: under KVM-nested SVM
// the CPU does not clear EVENTINJ.V, and a stale valid bit re-injects.
```

## L1538-1541 · `self.vcpu.vmcb.write_u8(vmcb::OFF_TLB_CTL, vmcb::TLB_DO_NOTHING);`

```
// TLB_CONTROL back to DO_NOTHING (KVM `svm_flush_tlb_*` sets it only on
// demand). The NPT only gains entries while the guest runs — not-present
// to present needs no flush — so the one flush at the first entry is
// all. It was 1 (flush ALL ASIDs, the host's too) on every VMRUN.
```

## L1544-1546 · `let eii = self.vcpu.vmcb.read_u64(vmcb::OFF_EXIT_INT_INFO);`

```
// `svm_complete_interrupts`: an external interrupt / NMI aborted
// mid-vectoring is re-injected; exceptions and software interrupts
// re-occur on their own (the instruction was not retired).
```

## L1553-1556 · `match exit {`

```
// Exits that touch only this vCPU take no VM_BIG_LOCK: a host
// interrupt, the interrupt window, an MSR (x2APIC, PV-EOI) and a
// hypercall (PV IPI). Behind the lock, one vCPU's EOI or IPI waited
// for the other's device work — a framebuffer copy, a 9p write.
```

## L1558-1560 · `EXIT_INTR | EXIT_NMI => {`

```
// A host interrupt (timer, device, kick IPI) or NMI pre-empted the
// guest; the host took it at STGI. Pending guest events are
// injected at the next entry.
```

## L1565-1566 · `EXIT_VINTR => {`

```
// The interrupt window opened (`svm_enable_irq_window`): the guest
// can take the event now — the next entry injects it.
```

## L1573-1574 · `let is_write = outcome.exit_qualification & 1 != 0;`

```
// EXITINFO1 bit 0: 0=RDMSR, 1=WRMSR. Every MSR is intercepted
// and emulated in `svm::msr`; none reaches the host.
```

## L1585 · `match r {`

```
// IA32_APIC_BASE, x2APIC registers, PV-EOI enable.
```

## L1633 · `lapic::phase(self.vcpu.apic_id, lapic::PH_BIGLOCK);`

```
// Serialize VmShared between vCPUs (guest SMP).
```

## L1641 · `if crate::microvm::vm_window() == 0 {`

```
// A headless test guest halting is done.
```

## L1648 · `if self.vcpu.vmcb.read_u64(vmcb::OFF_SAVE_RFLAGS) & (1 << 9) == 0 {`

```
// `cli; hlt` is Linux's no-ACPI poweroff / panic end state.
```

## L1666-1668 · `if !self.halt_poll() {`

```
// `kvm_vcpu_halt`: resume at once if something is deliverable,
// else halt-poll, then block (the fiber parks until the next
// timer deadline or a kick).
```

## L1701-1706 · `EXIT_VMRUN | EXIT_VMLOAD | EXIT_VMSAVE | EXIT_STGI`

```
// SVM instructions: the guest has no SVM (CPUID hides it, EFER.SVME
// writes #GP), so they #UD — KVM `nested_svm_check_permissions`.
// Left unintercepted they would run natively: the VMCB carries
// EFER.SVME=1, and VMLOAD/VMSAVE then take a HOST physical address.
// KVM hypercall (`kvm_emulate_hypercall`): nr in RAX, args in
// RBX/RCX/RDX/RSI, result in RAX. Only from CPL 0.
```

## L1713 · `EXIT_RDPMC => {`

```
// No vPMU (KVM with enable_pmu=0): RDPMC faults.
```

## L1718-1719 · `EXIT_WBINVD | EXIT_INVD => {`

```
// No non-coherent DMA into the guest → both are no-ops
// (KVM `kvm_emulate_wbinvd`, INVD treated as WBINVD).
```

## L1764-1766 · `last_outcome = Some(outcome);`

```
// Deliver a deferred device IRQ (esp. an async 9p
// write-completion) NOW rather than at the next
// EXIT_INTR/EXIT_HLT ~10 ms out — the download rxlat fix.
```

## L1783-1785 · `last_outcome = Some(outcome);`

```
// Deliver the freshly-completed 9p write-reply IRQ NOW
// (latched by drain_async_done at the loop top) instead
// of waiting for the next EXIT_INTR/EXIT_HLT.
```

## L1790 · `if handle_mmio_npf_blk(&mut *self.vcpu.vmcb, &mut self.vcpu.regs, &mut sh.pci.virtio_blk_sqfs, &mut sh.pic, gpa, sh.gues`

```
// Same handler — VirtioBlk carries its own IRQ line.
```

## L1801 · `{`

```
// I/O APIC page (0xFEC00000), NPT-not-present like the LAPIC's.
```

## L1811-1814 · `if (lapic::LAPIC_BASE..lapic::LAPIC_BASE + lapic::LAPIC_SIZE).contains(&gpa) {`

```
// Local APIC MMIO (guest-SMP Stage 1). The LAPIC page
// (0xFEE00000) is left NPT-not-present (npt.rs) so the
// guest's xAPIC accesses #NPF here → trap-and-emulate.
// Inert while booting `nolapic`.
```

## L1824-1828 · `if sh.guest_mem.ensure(gpa) {`

```
// B3: demand-paged guest RAM. #NPF on a gpa inside the
// advertised window but above the contiguous boot
// block = first touch of a 4-KB demand page → fault it
// in + re-enter. Order: MMIO BAR ranges first (above),
// RAM-demand here, fatal dump last.
```

## L1870-1872 · `self.shared.pci.virtio_blk.save();`

```
// Persist the virtio-blk profile-image to npkFS (encrypted at
// rest). Reached only when the loop ended (guest exit / cap), not
// on a StillRunning yield — identical to the old run_linux_loop.
```

## L1882-1888 · `fn advance_rip(vmcb: &mut vmcb::Vmcb) {`

```
/// Advance guest RIP across an *intercept* exit (CPUID / IOIO / MSR /
/// HLT etc.) via NRIP_SAVE. Requires CPUID 8000_000A EDX[3].
///
/// NRIP_SAVE is **undefined for #NPF and other hardware exceptions**
/// (APM Vol 2 §15.7.1) — KVM nested SVM zeroes it, which would land
/// the next VMRUN at RIP=0 and panic the guest. MMIO emulation must
/// use `advance_rip_by_length` instead.
```

## L1895-1896 · `fn clear_interrupt_shadow(vmcb: &mut vmcb::Vmcb) {`

```
/// KVM `__svm_skip_emulated_instruction` → `svm_set_interrupt_shadow(0)`: the
/// skipped instruction was the one an STI / MOV SS shadow covered.
```

## L1902-1903 · `fn advance_rip_by_length(vmcb: &mut vmcb::Vmcb, length: u8) {`

```
/// Advance guest RIP by the decoded instruction length. Used after
/// emulating an #NPF MMIO access (NRIP_SAVE is unreliable there).
```

## L1910-1914 · `fn handle_linux_io(`

```
/// Dispatch one I/O VMEXIT. UART COM1 (0x3F8-0x3FF) gets proper
/// synthetic responses; everything else absorbed (return 0 for IN,
/// no-op for OUT). Mirror of `vmx::handle_linux_io` shape, but
/// reads/writes RAX through VMCB instead of the GPR struct (SVM
/// auto-saves RAX in VMCB.SAVE.RAX on every VMEXIT).
```

## L1934 · `if port == PCI_CONFIG_ADDR`

```
// PCI config-space ports — dispatch to the bus emulator.
```

## L1945 · `if matches!(port, PIC_MASTER_CMD | PIC_MASTER_IMR | PIC_SLAVE_CMD | PIC_SLAVE_IMR`

```
// 8259 PIC + ELCR.
```

## L1956-1959 · `(0x60 | 0x64, true) => Some(0xFF),`

```
// i8254 channel 0 (the other channels are not a tick source).
// i8042: absent. An empty bus reads all-ones, and 0xFF at the status
// port is how Linux sees "No controller found" at once — a 0 there
// made it probe the CTR into timeouts (650 ms of every guest boot).
```

## L1967 · `(0x3F9, false) => None, // IER / DLM — ignore`

```
// IER / DLM — ignore
```

## L1973-1974 · `Some(if !serial.dlab { serial.rx_take() as u64 } else { 0 })`

```
// RBR (DLAB=0): pop one byte from injected RX FIFO.
// DLL (DLAB=1): unmodelled, return 0.
```

## L1977 · `(0x3FA, true) => Some(0x01), // IIR: bit 0 = no IRQ pending`

```
// IIR: bit 0 = no IRQ pending
```

## L1979 · `let dr = if serial.rx_has_data() { 0x01u64 } else { 0 };`

```
// LSR: THR-empty + TSR-empty always set, DR reflects FIFO.
```

## L1983 · `(0x3FE, true) => Some(0xB0), // MSR: CTS+DSR+DCD`

```
// MSR: CTS+DSR+DCD
```

## L1995-2012 · `fn guest_interruptible(vmcb: &vmcb::Vmcb) -> bool {`

```
/// Handle a #NPF on virtio-blk's BAR0 MMIO range. Uses SVM
/// decode-assists (CPUID 8000_000A EDX[7], probed at init) to read the
/// faulting instruction bytes from the VMCB, decodes the MOV form,
/// emulates the access against the device's MMIO model and advances
/// RIP via NRIP_SAVE.
///
/// Returns `true` if the fault was handled. `false` falls through to
/// the generic NPF dump path (decode failure, unsupported opcode).
/// Can the guest take a maskable external interrupt right now? RFLAGS.IF=1
/// AND no interrupt shadow (the 1-instruction block after STI / MOV SS).
/// Mirrors KVM `svm_interrupt_allowed` / the VMX `guest_interruptible`.
///
/// EVENTINJ is a FORCED injection — AMD delivers it regardless of guest IF
/// (APM Vol 2 §15.20). Harmless on a UP guest (spinlocks compile to no-ops),
/// but on an SMP guest, firing a device IRQ into a `spin_lock_irqsave`
/// critical section makes the handler spin on the held qspinlock → deadlock
/// (the guest-SMP 9p-mount livelock). So we only inject when interruptible,
/// and open the interrupt window otherwise (`inject_pending_event`).
```

## L2019-2022 · `fn handle_mmio_npf_ioapic(`

```
/// #NPF on the LAPIC MMIO page (guest-SMP Stage 1). Decode the faulting
/// MOV, service it against the per-vCPU `LocalApic`, advance RIP. xAPIC
/// registers are 32-bit; no device IRQ-kick (unlike the virtio handlers).
/// #NPF on the I/O APIC page → `devices::ioapic` (register window).
```

## L2112-2114 · `let rip = vmcb.read_u64(vmcb::OFF_SAVE_RIP);`

```
// Walk the guest's page tables to fetch the faulting instruction.
// KVM nested SVM doesn't populate decode-assists for #NPF, so we
// can't rely on VMCB.GUEST_INST_BYTES.
```

## L2153 · `pic.pulse(blk.irq_line());`

```
// Inject if interruptible, else defer (SMP qspinlock safety).
```

## L2162-2163 · `fn read_guest_gpr(regs: &vmcb::GuestRegs, rax: u64, idx: u8) -> u64 {`

```
/// Read a guest GPR by ModR/M index 0..15. RAX comes from VMCB.SAVE.RAX
/// (SVM auto-saves it); the rest from the GuestRegs struct we maintain.
```

## L2170 · `4  => 0, // RSP — never an MMIO source on Linux's path`

```
// RSP — never an MMIO source on Linux's path
```

## L2186-2187 · `fn write_guest_gpr(`

```
/// Write a value into a guest GPR by ModR/M index, honouring x86 width
/// rules. RAX writes go to VMCB.SAVE.RAX directly.
```

## L2202 · `4  => {} // RSP — silently drop`

```
// RSP — silently drop
```

## L2218-2220 · `fn handle_mmio_npf_net(`

```
/// Handle a #NPF on virtio-net's BAR0. Mirror of `handle_mmio_npf_blk`
/// for the second device. To be de-duplicated alongside the VMX twin
/// when virtio-gpu lands.
```

## L2252 · `if let Some(v) = crate::microvm::devices::net_backend::mmio_fast(off, dec.is_write) {`

```
// ISR read and TX doorbell: lock-free (the worker may hold the device).
```

## L2272-2274 · `crate::microvm::devices::net_backend::note_tx_kick();`

```
// TX off-vCPU (Stage 2c): hand the TX kick to the worker, which owns
// service_tx + tx_flush on its core — RX and TX then share one core /
// one NET path (no cross-core NET-lock fight delaying ACK egress).
```

## L2278-2280 · `if advanced && !(crate::microvm::devices::net_backend::msix_notify(0)`

```
// `service_queues` may complete TX and inject RX replies in one
// go; under MSI-X both queues' vectors fire (a spurious one is
// harmless: the guest finds nothing new), under INTx IRQ 10.
```

## L2293 · `fn handle_mmio_npf_gpu(`

```
/// Handle a #NPF on virtio-gpu BAR0. Mirror of `handle_mmio_npf_net`.
```

## L2336-2340 · `crate::microvm::devices::gpu_backend::note_gpu_kick(qidx);`

```
// Off-vCPU: defer the heavy ~8 MB copy + write_frame to the GPU worker
// on its own core (it raises IRQ9 + kicks the BSP). The vCPU exit stays
// cheap → no framebuffer copy stealing net cycles. note_gpu_kick is
// lock-free (atomics); the worker briefly waits for this exit to drop
// the gpu_backend lock on return — no cycle (it never takes VM_BIG_LOCK).
```

## L2345 · `pic.pulse(9);`

```
// virtio-gpu IRQ line = 9.
```

## L2355-2356 · `fn handle_mmio_npf_input(`

```
/// Handle a #NPF on virtio-input BAR0. Mirror of `handle_mmio_npf_gpu`
/// — only the device and IRQ line differ.
```

## L2400 · `pic.pulse(12);`

```
// virtio-input IRQ line = 12.
```

## L2409-2410 · `fn handle_mmio_npf_p9(`

```
/// Handle a #NPF on virtio-9p BAR0. Mirror of `handle_mmio_npf_input`
/// — only the device and IRQ line differ (9p = line 6).
```

## L2462 · `fn handle_mmio_npf_snd(`

```
/// Handle a #NPF on virtio-snd BAR0. Mirror of `handle_mmio_npf_p9`.
```

