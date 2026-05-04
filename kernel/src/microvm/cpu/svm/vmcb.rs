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

use core::ptr;

/// VMCB size — one 4 KB page, must be physically contiguous.
pub const VMCB_SIZE: usize = 4096;

// ── Control area offsets (APM Vol 2 Table B-1) ─────────────────────

#[allow(dead_code)] pub const OFF_INTERCEPT_CR: usize = 0x000;
#[allow(dead_code)] pub const OFF_INTERCEPT_DR: usize = 0x004;
#[allow(dead_code)] pub const OFF_INTERCEPT_EXC: usize = 0x008;
/// Misc intercepts vector 1 — INTR/NMI/SMI/INIT/VINTR/.../HLT/IO/MSR.
pub const OFF_INTERCEPT_MISC1: usize = 0x00C;
/// Misc intercepts vector 2 — VMRUN (mandatory!) /VMMCALL/VMSAVE/...
pub const OFF_INTERCEPT_MISC2: usize = 0x010;
#[allow(dead_code)] pub const OFF_PAUSE_FILTER_THRESH: usize = 0x03C;
pub const OFF_IOPM_BASE_PA: usize = 0x040;
pub const OFF_MSRPM_BASE_PA: usize = 0x048;
#[allow(dead_code)] pub const OFF_TSC_OFFSET: usize = 0x050;
pub const OFF_ASID: usize = 0x058;
pub const OFF_TLB_CTL: usize = 0x05C;
#[allow(dead_code)] pub const OFF_INT_CTL: usize = 0x060;
pub const OFF_EXIT_CODE: usize = 0x070;
pub const OFF_EXIT_INFO_1: usize = 0x078;
#[allow(dead_code)] pub const OFF_EXIT_INFO_2: usize = 0x080;
#[allow(dead_code)] pub const OFF_EXIT_INT_INFO: usize = 0x088;
pub const OFF_NESTED_CTL: usize = 0x090;
#[allow(dead_code)] pub const OFF_EVENT_INJ: usize = 0x0A8;
pub const OFF_NCR3: usize = 0x0B0;
#[allow(dead_code)] pub const OFF_VMCB_CLEAN: usize = 0x0C0;
#[allow(dead_code)] pub const OFF_NRIP: usize = 0x0C8;

// ── Misc-1 intercept bits (APM Vol 2 §15.9) ────────────────────────

#[allow(dead_code)] pub const INTERCEPT_INTR: u32 = 1 << 0;
#[allow(dead_code)] pub const INTERCEPT_NMI: u32 = 1 << 1;
pub const INTERCEPT_HLT: u32 = 1 << 24;
#[allow(dead_code)] pub const INTERCEPT_INVLPG: u32 = 1 << 25;
#[allow(dead_code)] pub const INTERCEPT_INVLPGA: u32 = 1 << 26;
#[allow(dead_code)] pub const INTERCEPT_IOIO_PROT: u32 = 1 << 27;
#[allow(dead_code)] pub const INTERCEPT_MSR_PROT: u32 = 1 << 28;
#[allow(dead_code)] pub const INTERCEPT_TASK_SW: u32 = 1 << 29;

// ── Misc-2 intercept bits ──────────────────────────────────────────

/// VMRUN intercept — MANDATORY per APM §15.5.1: "VMRUN must be
/// intercepted, otherwise the CPU generates #UD". It's intercepted
/// from the *guest* — the host runs VMRUN unconditionally.
pub const INTERCEPT_VMRUN: u32 = 1 << 0;
#[allow(dead_code)] pub const INTERCEPT_VMMCALL: u32 = 1 << 1;
#[allow(dead_code)] pub const INTERCEPT_VMSAVE: u32 = 1 << 3;
#[allow(dead_code)] pub const INTERCEPT_VMLOAD: u32 = 1 << 2;

// ── State save area offsets (relative to 0x400 = save base) ────────

#[allow(dead_code)] pub const OFF_SAVE_BASE: usize = 0x400;

// Segments are 16 bytes each: selector(2), attrib(2), limit(4), base(8)
pub const OFF_SAVE_ES: usize = 0x400 + 0x000;
pub const OFF_SAVE_CS: usize = 0x400 + 0x010;
pub const OFF_SAVE_SS: usize = 0x400 + 0x020;
pub const OFF_SAVE_DS: usize = 0x400 + 0x030;
#[allow(dead_code)] pub const OFF_SAVE_FS: usize = 0x400 + 0x040;
#[allow(dead_code)] pub const OFF_SAVE_GS: usize = 0x400 + 0x050;
pub const OFF_SAVE_GDTR: usize = 0x400 + 0x060;
#[allow(dead_code)] pub const OFF_SAVE_LDTR: usize = 0x400 + 0x070;
pub const OFF_SAVE_IDTR: usize = 0x400 + 0x080;
#[allow(dead_code)] pub const OFF_SAVE_TR: usize = 0x400 + 0x090;
pub const OFF_SAVE_CPL: usize = 0x400 + 0x0CB;
pub const OFF_SAVE_EFER: usize = 0x400 + 0x0D0;
pub const OFF_SAVE_CR4: usize = 0x400 + 0x148;
pub const OFF_SAVE_CR3: usize = 0x400 + 0x150;
pub const OFF_SAVE_CR0: usize = 0x400 + 0x158;
#[allow(dead_code)] pub const OFF_SAVE_DR7: usize = 0x400 + 0x160;
#[allow(dead_code)] pub const OFF_SAVE_DR6: usize = 0x400 + 0x168;
pub const OFF_SAVE_RFLAGS: usize = 0x400 + 0x170;
pub const OFF_SAVE_RIP: usize = 0x400 + 0x178;
pub const OFF_SAVE_RSP: usize = 0x400 + 0x1D8;
pub const OFF_SAVE_RAX: usize = 0x400 + 0x1F8;
#[allow(dead_code)] pub const OFF_SAVE_CR2: usize = 0x400 + 0x240;
pub const OFF_SAVE_G_PAT: usize = 0x400 + 0x268;

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

/// Real-mode 16-bit code segment: P=1, S=1, type=Code/Read/Accessed (1011).
pub const ATTR_CODE_RM: u16 = 0x9B;
/// Real-mode 16-bit data segment: P=1, S=1, type=Data/Write/Accessed (0011).
pub const ATTR_DATA_RM: u16 = 0x93;

// ── VMCB wrapper ────────────────────────────────────────────────────

/// 4 KB page-aligned VMCB. Lives in the kernel heap (allocated
/// physically contiguous via `mm::memory::alloc_contiguous`).
#[repr(C, align(4096))]
pub struct Vmcb {
    pub bytes: [u8; VMCB_SIZE],
}

impl Vmcb {
    pub const fn zeroed() -> Self {
        Self { bytes: [0; VMCB_SIZE] }
    }

    /// Write a u8 at offset.
    pub fn write_u8(&mut self, off: usize, val: u8) {
        self.bytes[off] = val;
    }

    /// Write a little-endian u16 at offset.
    pub fn write_u16(&mut self, off: usize, val: u16) {
        self.bytes[off..off + 2].copy_from_slice(&val.to_le_bytes());
    }

    /// Write a little-endian u32 at offset.
    pub fn write_u32(&mut self, off: usize, val: u32) {
        self.bytes[off..off + 4].copy_from_slice(&val.to_le_bytes());
    }

    /// Write a little-endian u64 at offset.
    pub fn write_u64(&mut self, off: usize, val: u64) {
        self.bytes[off..off + 8].copy_from_slice(&val.to_le_bytes());
    }

    /// Read a little-endian u32 at offset.
    #[allow(dead_code)]
    pub fn read_u32(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.bytes[off..off + 4].try_into().unwrap())
    }

    /// Read a little-endian u64 at offset.
    pub fn read_u64(&self, off: usize) -> u64 {
        u64::from_le_bytes(self.bytes[off..off + 8].try_into().unwrap())
    }

    /// Initialize a segment slot (16 bytes) with selector / attrib /
    /// limit / base. Used for both real-mode and protected-mode
    /// segments — the encoding is uniform.
    pub fn write_segment(
        &mut self,
        off: usize,
        selector: u16,
        attrib: u16,
        limit: u32,
        base: u64,
    ) {
        self.write_u16(off + 0, selector);
        self.write_u16(off + 2, attrib);
        self.write_u32(off + 4, limit);
        self.write_u64(off + 8, base);
    }

    /// Physical address of this VMCB. SAFETY: caller guarantees
    /// the VMCB was allocated from the kernel's identity-mapped
    /// contiguous region (every Vmcb in 12.1.0b lives there).
    pub fn phys_addr(&self) -> u64 {
        ptr::addr_of!(self.bytes) as u64
    }
}

/// Outcome of one VMRUN dispatch — populated by the asm shim from
/// VMCB control-area exit fields. Mirrors `vmx::vmcs::LaunchOutcome`.
pub use super::super::LaunchOutcome;

/// Guest GPRs — the asm shim spills these on every VMEXIT and
/// reloads them on VMRUN. RAX is special because the CPU itself
/// saves/restores it in VMCB.SAVE.RAX during VMRUN; the 14 other
/// GPRs are shadowed through this struct. Layout matches the asm
/// offsets in `enable::run_guest_once`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct GuestRegs {
    pub rbx: u64,    //   0
    pub rcx: u64,    //   8
    pub rdx: u64,    //  16
    pub rsi: u64,    //  24
    pub rdi: u64,    //  32
    pub rbp: u64,    //  40
    pub r8:  u64,    //  48
    pub r9:  u64,    //  56
    pub r10: u64,    //  64
    pub r11: u64,    //  72
    pub r12: u64,    //  80
    pub r13: u64,    //  88
    pub r14: u64,    //  96
    pub r15: u64,    // 104
}
