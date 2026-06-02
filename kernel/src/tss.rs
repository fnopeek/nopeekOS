//! Task State Segment install — Phase 12.1.0d-2a prerequisite for VMLAUNCH.
//!
//! The boot GDT (`boot.s :: gdt64`) is three entries — null, 64-bit
//! code, 64-bit data — and the kernel never executed `ltr`, so TR=0.
//! VMX host-state validation rejects HOST_TR_SELECTOR=0 at VMLAUNCH
//! (SDM Vol. 3C §26.2.3). This module clones the boot GDT into BSS,
//! appends a 16-byte long-mode TSS descriptor, `lgdt`s the new GDT,
//! and `ltr`s the new TSS selector. Single-CPU (BSP-only) — APs keep
//! the boot GDT, which is fine since VMX runs only on Core 0.
//!
//! Reference: Intel SDM Vol. 3A §3.4.5.1 (Code- and Data-Segment
//! Descriptor Types), §7.7 (Task Management in 64-bit Mode);
//! Vol. 3C §26.2.3 (Checks on Host Segment and Descriptor-Table
//! Registers).

use core::sync::atomic::{AtomicBool, Ordering};

/// Maximum cores we install a per-core TSS for (`ensure_core`). The test
/// boxes are ≤8 logical CPUs; 16 leaves headroom without bloating .data.
/// A core id ≥ this is a no-op (it just won't run a VMX guest — only the
/// fiber-mode worker cores need their own TR).
pub const MAX_CORES: usize = 16;

/// Long-mode TSS layout (104 bytes minimum, no I/O bitmap). RSP0/1/2
/// and IST1..IST7 stay zero — we don't use ring transitions or IST
/// stacks today. I/O map base = 104 means "no I/O bitmap follows".
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Tss {
    _reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    _reserved1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

const TSS_LIMIT: u16 = (core::mem::size_of::<Tss>() - 1) as u16;

const TSS_INIT: Tss = Tss {
    _reserved0: 0,
    rsp0: 0, rsp1: 0, rsp2: 0,
    _reserved1: 0,
    ist1: 0, ist2: 0, ist3: 0, ist4: 0, ist5: 0, ist6: 0, ist7: 0,
    _reserved2: 0, _reserved3: 0,
    iomap_base: 104,
};

static mut TSS: Tss = TSS_INIT;

/// Build the 16-byte long-mode TSS descriptor (two GDT slots) for a TSS
/// at `tss_base`. Returns `(lo, hi)`. SDM Vol. 3A §7.2.3.
fn tss_descriptor(tss_base: u64) -> (u64, u64) {
    let limit_lo = TSS_LIMIT as u64;
    let base_lo15 = tss_base & 0xFFFF;
    let base_lo23 = (tss_base >> 16) & 0xFF;
    let base_lo31 = (tss_base >> 24) & 0xFF;
    let access: u64 = 0x89; // P=1, DPL=0, S=0, type=9 (available 64-bit TSS)
    let granularity: u64 = 0; // G=0, AVL=0, limit[19:16]=0
    let desc_lo = limit_lo
        | (base_lo15 << 16)
        | (base_lo23 << 32)
        | (access << 40)
        | (granularity << 48)
        | (base_lo31 << 56);
    let desc_hi = (tss_base >> 32) & 0xFFFF_FFFF;
    (desc_lo, desc_hi)
}

const GDT_INIT: [u64; 5] = [
    0,                       // null
    0x00AF_9A00_0000_FFFF,   // code (0x08, ring0, L=1, P=1, type=0xA)
    0x00CF_9200_0000_FFFF,   // data (0x10, ring0, P=1, type=0x2)
    0,                       // TSS desc lo  — filled at install
    0,                       // TSS desc hi  — filled at install
];

const GDTR_INIT: GdtPointer = GdtPointer { limit: 0, base: 0 };

// ── Per-core (AP) TSS+GDT for fiber-mode VMX ────────────────────────
//
// VMX rejects HOST_TR_SELECTOR=0 at VM-entry (SDM §26.2.3). The BSP gets
// its TR from `init()` at boot; AP/worker cores keep the boot GDT with
// TR=0. That was fine while VMX ran only on Core 0, but fiber-mode runs
// the guest's VMRESUME loop on a worker core — so that worker needs its
// OWN valid TSS + TR before `write_host_state`. Each core gets a private
// TSS + 5-slot GDT (the busy bit `ltr` sets means cores cannot share one
// TSS descriptor). SVM (vmsave) has no such host-state check, so the AMD
// path never needed this — and never calls `ensure_core` (it opens via
// `svm::vm_open`, not `vmx::vm_open`).
#[unsafe(link_section = ".data")]
static mut AP_GDT: [[u64; 5]; MAX_CORES] = [GDT_INIT; MAX_CORES];
#[unsafe(link_section = ".data")]
static mut AP_GDTR: [GdtPointer; MAX_CORES] = [GDTR_INIT; MAX_CORES];
static mut AP_TSS: [Tss; MAX_CORES] = [TSS_INIT; MAX_CORES];
static AP_TSS_INSTALLED: [AtomicBool; MAX_CORES] = {
    const F: AtomicBool = AtomicBool::new(false);
    [F; MAX_CORES]
};

/// Install a private TSS + GDT on the CURRENT core and `ltr` it, so VMX
/// host-state has a valid HOST_TR when the guest runs as a fiber on this
/// worker core. Idempotent per core. No-op for core 0 (the BSP already
/// `ltr`'d via `init()` at boot — must not clobber its GDT) and for ids
/// past `MAX_CORES`. Mirrors `init()` but per-core. Call on the worker
/// core that is about to `vm_open` a VMX guest.
pub fn ensure_core(core_id: usize) {
    if core_id == 0 || core_id >= MAX_CORES {
        return;
    }
    if AP_TSS_INSTALLED[core_id].swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: each core writes only its own slot (disjoint), this core is
    // the only one that ever `ltr`s AP_GDT[core_id]. lgdt keeps CS/SS/DS
    // valid because slots 1+2 match the boot GDT byte-for-byte; we never
    // reload the segment registers.
    unsafe {
        let tss_base = core::ptr::addr_of!(AP_TSS[core_id]) as u64;
        let (desc_lo, desc_hi) = tss_descriptor(tss_base);
        core::ptr::addr_of_mut!(AP_GDT[core_id][3]).write(desc_lo);
        core::ptr::addr_of_mut!(AP_GDT[core_id][4]).write(desc_hi);

        let gdt_base = core::ptr::addr_of!(AP_GDT[core_id]) as u64;
        let gdt_bytes = core::mem::size_of::<[u64; 5]>();
        core::ptr::addr_of_mut!(AP_GDTR[core_id].limit).write((gdt_bytes - 1) as u16);
        core::ptr::addr_of_mut!(AP_GDTR[core_id].base).write(gdt_base);

        core::arch::asm!(
            "lgdt [{ptr}]",
            "ltr {sel:x}",
            ptr = in(reg) core::ptr::addr_of!(AP_GDTR[core_id]),
            sel = in(reg) TSS_SELECTOR,
            options(nostack, preserves_flags),
        );
    }
}

/// 5-slot GDT: null (0), code (1, 0x08), data (2, 0x10), TSS-lo (3,
/// 0x18), TSS-hi (4). Initial values for slots 1+2 mirror `boot.s ::
/// gdt64_code/_data`; slots 3+4 are filled at runtime once we know
/// the TSS virtual address.
#[unsafe(link_section = ".data")]
static mut GDT: [u64; 5] = [
    0,                       // null
    0x00AF_9A00_0000_FFFF,   // code (0x08, ring0, L=1, P=1, type=0xA)
    0x00CF_9200_0000_FFFF,   // data (0x10, ring0, P=1, type=0x2)
    0,                       // TSS desc lo  — filled in init()
    0,                       // TSS desc hi  — filled in init()
];

/// 10-byte pseudo-descriptor consumed by `lgdt`. Filled at init time.
#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

#[unsafe(link_section = ".data")]
static mut GDTR: GdtPointer = GdtPointer { limit: 0, base: 0 };

/// TSS selector for `ltr`: index 3, TI=0, RPL=0.
const TSS_SELECTOR: u16 = 3 << 3;

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Install the TSS and switch to the cloned GDT. Idempotent; calling
/// twice is a no-op so accidental double-invocation is harmless.
pub fn init() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }

    let tss_base: u64 = core::ptr::addr_of!(TSS) as u64;
    let (desc_lo, desc_hi) = tss_descriptor(tss_base);

    // SAFETY: BSP boot path, single-threaded relative to GDT/TSS
    // statics (APs are already running but never touch these symbols).
    // Writes are confined to BSS-resident memory we own exclusively.
    unsafe {
        core::ptr::addr_of_mut!(GDT[3]).write(desc_lo);
        core::ptr::addr_of_mut!(GDT[4]).write(desc_hi);

        let gdt_base = core::ptr::addr_of!(GDT) as u64;
        let gdt_bytes = core::mem::size_of::<[u64; 5]>();
        core::ptr::addr_of_mut!(GDTR.limit).write((gdt_bytes - 1) as u16);
        core::ptr::addr_of_mut!(GDTR.base).write(gdt_base);

        // lgdt loads from a memory operand. Then ltr loads the TSS.
        // The CS/SS/DS/ES/FS/GS selectors stay valid because slots 1
        // and 2 of the new GDT match the boot GDT byte-for-byte.
        core::arch::asm!(
            "lgdt [{ptr}]",
            "ltr {sel:x}",
            ptr = in(reg) core::ptr::addr_of!(GDTR),
            sel = in(reg) TSS_SELECTOR,
            options(nostack, preserves_flags),
        );
    }
}
