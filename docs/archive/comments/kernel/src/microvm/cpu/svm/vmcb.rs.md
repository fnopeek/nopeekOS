# `kernel/src/microvm/cpu/svm/vmcb.rs` @ 5e0102684

## L1-21 · `use core::ptr;`

```
//! VMCB (Virtual Machine Control Block) layout + accessors.
//!
//! Reference: AMD64 APM Vol. 2 Appendix B "Layout of VMCB".
//!
//! The VMCB is a 4 KB page split in two:
//!   * Control area, 0x000..0x400 — VMM-controlled, read/written
//!     before VMRUN, exit-info populated by CPU on VMEXIT.
//!   * State save area, 0x400..0x698 — guest CPU state, loaded into
//!     CPU on VMRUN, saved back on VMEXIT.
//!
//! Unlike Intel VMCS which is opaque (VMREAD/VMWRITE only), the
//! VMCB lives in regular memory and is accessed by direct loads /
//! stores. The CPU expects the page to be physically contiguous
//! and 4 KB aligned; the host-save area (separate, pointed to by
//! VM_HSAVE_PA MSR) has the same constraints.
//!
//! For 12.1.0b-svm we touch a minimal subset (asid, tlb_ctl,
//! intercepts, iopm/msrpm bases, exit fields, plus segment/CR/RFLAGS
//! for the guest stub). The full struct is sketched as offset
//! constants — accessor methods on `Vmcb` take an offset and a value
//! so additional fields can be plumbed without touching this file.
```

## L25 · `pub const VMCB_SIZE: usize = 4096;`

```
/// VMCB size — one 4 KB page, must be physically contiguous.
```

## L28 · `#[allow(dead_code)] pub const OFF_INTERCEPT_CR: usize = 0x000;`

```
// ── Control area offsets (APM Vol 2 Table B-1) ─────────────────────
```

## L33 · `pub const OFF_INTERCEPT_MISC1: usize = 0x00C;`

```
/// Misc intercepts vector 1 — INTR/NMI/SMI/INIT/VINTR/.../HLT/IO/MSR.
```

## L35 · `pub const OFF_INTERCEPT_MISC2: usize = 0x010;`

```
/// Misc intercepts vector 2 — VMRUN (mandatory!) /VMMCALL/VMSAVE/...
```

## L43-44 · `pub const TLB_DO_NOTHING: u8 = 0;`

```
/// TLB_CONTROL values (APM 15.16.1): 0 nothing, 1 flush ALL ASIDs,
/// 3 flush this guest's ASID.
```

## L47-48 · `pub fn tlb_flush_guest() -> u8 {`

```
/// The first-entry flush: this guest's ASID where the CPU can
/// (CPUID 8000_000A EDX[6] FlushByAsid), else every ASID — KVM's choice.
```

## L54 · `pub const V_IRQ: u32 = 1 << 8;`

```
/// `int_ctl` bits (APM Vol 2 §15.21.1 / KVM svm.h).
```

## L59-60 · `pub const V_INTR_MASKING: u32 = 1 << 24;`

```
/// Guest RFLAGS.IF masks only VIRTUAL interrupts; physical ones (host timer,
/// kick IPIs) still exit. Without it a guest with IF=0 held our kicks back.
```

## L63-65 · `pub const OFF_INT_STATE: usize = 0x068;`

```
/// Interrupt shadow / guest interrupt mask (APM Vol 2 App. B). Bit 0 =
/// interrupt shadow (set for one instruction after STI / MOV SS). Read to
/// gate IRQ injection (with RFLAGS.IF) — see `guest_interruptible`.
```

## L74-77 · `pub const OFF_NRIP: usize = 0x0C8;`

```
/// Next-instruction RIP — populated by CPU on most non-fault VMEXITs
/// when CPUID 8000_000A EDX[3] (NRIP_SAVE) is set. Lets us advance
/// guest RIP across a CPUID/IOIO/MSR/HLT exit without re-decoding the
/// instruction. APM Vol 2 §15.7.1.
```

## L80-83 · `#[allow(dead_code)] pub const OFF_NUM_INST_BYTES: usize = 0x0D0;`

```
/// Decode-assists: number of valid bytes in `OFF_INST_BYTES`. Populated
/// by the CPU on #NPF when CPUID 8000_000A EDX[7] is set. APM §15.20.
/// Currently unused — KVM nested SVM doesn't populate these for nested
/// NPF. We walk guest page tables instead (`devices::guest_fetch`).
```

## L87 · `pub const INTERCEPT_INTR: u32 = 1 << 0;`

```
// ── Misc-1 intercept bits (APM Vol 2 §15.9) ────────────────────────
```

## L90 · `pub const INTERCEPT_VINTR: u32 = 1 << 4;`

```
/// Virtual interrupt — the interrupt window (`svm_enable_irq_window`).
```

## L93-95 · `pub const INTERCEPT_CPUID: u32 = 1 << 18;`

```
/// CPUID — Linux uses this for early feature detection (CPU vendor,
/// CET, AVX, MWAIT, ...). Required so we can hide CET from the guest
/// the same way the VMX backend does.
```

## L103-105 · `pub const INTERCEPT_SHUTDOWN: u32 = 1 << 31;`

```
/// Shutdown — guest triple-fault path. Without this, a triple-fault
/// reboots the host. Linux uses triple-fault as `emergency_restart`
/// when ACPI/PIIX/EFI reset paths are unavailable.
```

## L108 · `pub const INTERCEPT_VMRUN: u32 = 1 << 0;`

```
// ── Misc-2 intercept bits ──────────────────────────────────────────
```

## L110-112 · `pub const INTERCEPT_VMRUN: u32 = 1 << 0;`

```
/// VMRUN intercept — MANDATORY per APM §15.5.1: "VMRUN must be
/// intercepted, otherwise the CPU generates #UD". It's intercepted
/// from the *guest* — the host runs VMRUN unconditionally.
```

## L129-130 · `pub const LINUX_MISC1: u32 = INTERCEPT_INTR | INTERCEPT_NMI | INTERCEPT_RDPMC`

```
/// Linux guest intercept set — KVM `init_vmcb` minus what NPT makes moot
/// (CR/INVLPG) and what we do not virtualize differently (PAUSE, SMI).
```

## L139 · `#[allow(dead_code)] pub const OFF_SAVE_BASE: usize = 0x400;`

```
// ── State save area offsets (relative to 0x400 = save base) ────────
```

## L143 · `pub const OFF_SAVE_ES: usize = 0x400 + 0x000;`

```
// Segments are 16 bytes each: selector(2), attrib(2), limit(4), base(8)
```

## L168-181 · `#[allow(dead_code)] pub const ATTR_CODE_RM: u16 = 0x9B;`

```
// ── Segment attribute encodings (APM §15.5.1) ──────────────────────
//
// SVM stores segment attributes as a 12-bit packed format:
//   bits  0..3 : type
//   bit   4    : S
//   bits  5..6 : DPL
//   bit   7    : P
//   bit   8    : AVL
//   bit   9    : L (long mode)
//   bit  10    : DB
//   bit  11    : G
//
// This packs the 4 attribute bytes of a normal x86 segment descriptor
// (which span access-byte + flags-nibble) into 12 contiguous bits.
```

## L183 · `#[allow(dead_code)] pub const ATTR_CODE_RM: u16 = 0x9B;`

```
/// Real-mode 16-bit code segment: P=1, S=1, type=Code/Read/Accessed (1011).
```

## L185 · `#[allow(dead_code)] pub const ATTR_DATA_RM: u16 = 0x93;`

```
/// Real-mode 16-bit data segment: P=1, S=1, type=Data/Write/Accessed (0011).
```

## L187-188 · `pub const ATTR_CODE_PM32: u16 = 0xC9B;`

```
/// 32-bit prot-mode code segment: ATTR_CODE_RM + D=1 (32-bit) + G=1
/// (4 KB granularity). Used by 12.1.1b-svm "OK" port-0x80 stub.
```

## L190 · `pub const ATTR_DATA_PM32: u16 = 0xC93;`

```
/// 32-bit prot-mode data segment: ATTR_DATA_RM + D=1 + G=1.
```

## L193 · `#[repr(C, align(4096))]`

```
// ── VMCB wrapper ────────────────────────────────────────────────────
```

## L195-196 · `#[repr(C, align(4096))]`

```
/// 4 KB page-aligned VMCB. Lives in the kernel heap (allocated
/// physically contiguous via `mm::memory::alloc_contiguous`).
```

## L207 · `pub fn write_u8(&mut self, off: usize, val: u8) {`

```
/// Write a u8 at offset.
```

## L212 · `pub fn write_u16(&mut self, off: usize, val: u16) {`

```
/// Write a little-endian u16 at offset.
```

## L217 · `pub fn write_u32(&mut self, off: usize, val: u32) {`

```
/// Write a little-endian u32 at offset.
```

## L222 · `pub fn write_u64(&mut self, off: usize, val: u64) {`

```
/// Write a little-endian u64 at offset.
```

## L227 · `#[allow(dead_code)]`

```
/// Read a u8 at offset.
```

## L233 · `#[allow(dead_code)]`

```
/// Read a little-endian u32 at offset.
```

## L239 · `pub fn read_u64(&self, off: usize) -> u64 {`

```
/// Read a little-endian u64 at offset.
```

## L244-246 · `pub fn write_segment(`

```
/// Initialize a segment slot (16 bytes) with selector / attrib /
/// limit / base. Used for both real-mode and protected-mode
/// segments — the encoding is uniform.
```

## L261-263 · `pub fn phys_addr(&self) -> u64 {`

```
/// Physical address of this VMCB. SAFETY: caller guarantees
/// the VMCB was allocated from the kernel's identity-mapped
/// contiguous region (every Vmcb in 12.1.0b lives there).
```

## L269-270 · `pub use super::super::LaunchOutcome;`

```
/// Outcome of one VMRUN dispatch — populated by the asm shim from
/// VMCB control-area exit fields. Mirrors `vmx::vmcs::LaunchOutcome`.
```

## L273-277 · `#[repr(C)]`

```
/// Guest GPRs — the asm shim spills these on every VMEXIT and
/// reloads them on VMRUN. RAX is special because the CPU itself
/// saves/restores it in VMCB.SAVE.RAX during VMRUN; the 14 other
/// GPRs are shadowed through this struct. Layout matches the asm
/// offsets in `enable::run_guest_once`.
```

## L281 · `pub rbx: u64,    //   0`

```
//   0
```

## L282 · `pub rcx: u64,    //   8`

```
//   8
```

## L283 · `pub rdx: u64,    //  16`

```
//  16
```

## L284 · `pub rsi: u64,    //  24`

```
//  24
```

## L285 · `pub rdi: u64,    //  32`

```
//  32
```

## L286 · `pub rbp: u64,    //  40`

```
//  40
```

## L287 · `pub r8:  u64,    //  48`

```
//  48
```

## L288 · `pub r9:  u64,    //  56`

```
//  56
```

## L289 · `pub r10: u64,    //  64`

```
//  64
```

## L290 · `pub r11: u64,    //  72`

```
//  72
```

## L291 · `pub r12: u64,    //  80`

```
//  80
```

## L292 · `pub r13: u64,    //  88`

```
//  88
```

## L293 · `pub r14: u64,    //  96`

```
//  96
```

## L294 · `pub r15: u64,    // 104`

```
// 104
```

