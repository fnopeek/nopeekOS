//! Symmetric Multiprocessing (SMP)
//!
//! Discovers CPU cores via ACPI MADT, boots Application Processors
//! using INIT-SIPI-SIPI, and manages per-core state.
//!
//! Design: Core 0 = Kernel/IRQ (fixed), Cores 1..N = Worker Pool.
//! No hardcoded core limit — scales from 2 to 1024+.

core::arch::global_asm!(include_str!("trampoline.s"), options(att_syntax));

pub mod per_core;
pub mod scheduler;

use crate::kprintln;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

const TRAMPOLINE_BASE: usize = 0x8000;
const AP_STACK_SIZE: usize = 64 * 1024; // 64KB per AP

// Data area offsets within trampoline (must match trampoline.s)
const OFF_GDT64: usize = 0xE0;
const OFF_CR3: usize = 0xF0;
const OFF_STACK: usize = 0xF8;
const OFF_ENTRY: usize = 0x100;
const OFF_CORE_ID: usize = 0x108;
const OFF_RUNNING: usize = 0x10C;
const OFF_IDTR: usize = 0x110;

/// Counter incremented by each AP when it reaches Rust entry
static AP_STARTED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    static smp_trampoline_start: u8;
    static smp_trampoline_end: u8;
}

/// Initialize SMP: discover cores via MADT, boot all APs.
pub fn init() {
    let apic_base = read_apic_base();
    if apic_base == 0 {
        kprintln!("[npk] smp: no Local APIC");
        return;
    }

    // Ensure APIC MMIO page is accessible
    let _ = crate::paging::map_page(
        apic_base, apic_base,
        crate::paging::PageFlags::PRESENT
            | crate::paging::PageFlags::WRITABLE
            | crate::paging::PageFlags::NO_CACHE,
    );

    let bsp_id = read_apic_id(apic_base);
    per_core::register_bsp(bsp_id);

    // Enable HWP (hardware frequency scaling) on BSP
    if per_core::enable_hwp() {
        per_core::update_core_freq(0);
        kprintln!("[npk] HWP: {}-{} MHz (auto-scaling)",
            per_core::min_eff_mhz(), per_core::max_turbo_mhz());
    }

    // Discover APs from ACPI MADT
    let ap_ids = parse_madt(bsp_id);
    if ap_ids.is_empty() {
        kprintln!("[npk] smp: 1 core (BSP only)");
        return;
    }

    kprintln!("[npk] smp: {} cores detected (BSP + {} APs)",
        ap_ids.len() + 1, ap_ids.len());

    // Prepare trampoline at 0x8000
    setup_trampoline(apic_base);

    // Boot each AP sequentially
    let mut online = 0u32;
    for (i, &ap_apic_id) in ap_ids.iter().enumerate() {
        let core_id = (i + 1) as u32;
        if boot_ap(apic_base, ap_apic_id, core_id) {
            per_core::register_ap(ap_apic_id, core_id);
            online += 1;
        } else {
            kprintln!("[npk] smp: core {} (APIC {}) failed", core_id, ap_apic_id);
            per_core::mark_failed(ap_apic_id);
        }
    }

    kprintln!("[npk] smp: {}/{} APs online", online, ap_ids.len());

    if online > 0 {
        // Initialize scheduler and wake APs into their work loops
        scheduler::init(online as usize);
        per_core::start_scheduler();

        let wakeup = if per_core::has_mwait() { "MONITOR/MWAIT" } else { "HLT" };
        kprintln!("[npk] smp: scheduler ready (work-stealing, {})", wakeup);
    }
}

/// Read Local APIC base from MSR 0x1B
fn read_apic_base() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: MSR 0x1B is the APIC base, always readable on x86_64
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }
    ((hi as u64) << 32 | lo as u64) & 0xFFFF_FFFF_F000
}

/// Read this core's APIC ID from the Local APIC register
fn read_apic_id(apic_base: u64) -> u32 {
    // SAFETY: APIC page is mapped, register at offset 0x20 is read-only
    let raw = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) };
    raw >> 24
}

/// Copy trampoline to 0x8000 and fill in shared data (CR3, GDT, IDT, entry)
fn setup_trampoline(_apic_base: u64) {
    unsafe {
        let start = &smp_trampoline_start as *const u8;
        let end = &smp_trampoline_end as *const u8;
        let size = end as usize - start as usize;

        // SAFETY: 0x8000 is in first 1MB (reserved, identity-mapped, not used by kernel)
        core::ptr::copy_nonoverlapping(start, TRAMPOLINE_BASE as *mut u8, size);

        // CR3 — BSP's page table root
        // Trampoline loads CR3 from 32-bit protected mode (only 32-bit mov available).
        // PML4 is in kernel BSS (~7MB) — well below 4GB. Assert guards against future
        // changes where frame allocator might place page tables in high RAM.
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        assert!(cr3 < 0x1_0000_0000, "PML4 above 4GB — AP trampoline cannot load it");
        *((TRAMPOLINE_BASE + OFF_CR3) as *mut u32) = cr3 as u32;

        // GDT64 pointer — copy BSP's GDTR (SGDT stores 10 bytes in 64-bit mode)
        // Trampoline uses lgdt in 32-bit mode (reads 2+4 bytes). GDT base is in kernel
        // .rodata (~1MB), fits in 32 bits. Same assert for safety.
        let mut gdtr = [0u8; 10];
        core::arch::asm!("sgdt [{}]", in(reg) gdtr.as_mut_ptr());
        let gdt_limit = u16::from_le_bytes([gdtr[0], gdtr[1]]);
        let gdt_base = u64::from_le_bytes([
            gdtr[2], gdtr[3], gdtr[4], gdtr[5], gdtr[6], gdtr[7], gdtr[8], gdtr[9],
        ]);
        assert!(gdt_base < 0x1_0000_0000, "GDT above 4GB — AP trampoline cannot load it");
        *((TRAMPOLINE_BASE + OFF_GDT64) as *mut u16) = gdt_limit;
        *((TRAMPOLINE_BASE + OFF_GDT64 + 2) as *mut u32) = gdt_base as u32;

        // IDTR — copy BSP's IDT register (SIDT stores 10 bytes)
        let mut idtr = [0u8; 10];
        core::arch::asm!("sidt [{}]", in(reg) idtr.as_mut_ptr());
        core::ptr::copy_nonoverlapping(
            idtr.as_ptr(),
            (TRAMPOLINE_BASE + OFF_IDTR) as *mut u8,
            10,
        );

        // Rust AP entry point
        *((TRAMPOLINE_BASE + OFF_ENTRY) as *mut u64) =
            per_core::smp_ap_entry as *const () as u64;
    }
}

/// Boot a single AP: allocate stack, write per-AP data, send INIT-SIPI-SIPI
fn boot_ap(apic_base: u64, target_apic_id: u32, core_id: u32) -> bool {
    let stack_top = allocate_ap_stack();
    if stack_top == 0 {
        return false;
    }

    // Write per-AP fields to trampoline data area
    // SAFETY: trampoline at 0x8000 is set up, no AP is using it yet
    //         (we boot APs sequentially)
    unsafe {
        *((TRAMPOLINE_BASE + OFF_STACK) as *mut u64) = stack_top;
        *((TRAMPOLINE_BASE + OFF_CORE_ID) as *mut u32) = core_id;
        *((TRAMPOLINE_BASE + OFF_RUNNING) as *mut u32) = 0;

        // Ensure all writes are visible before SIPI
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    let started_before = AP_STARTED.load(Ordering::Acquire);
    let vector = (TRAMPOLINE_BASE / 0x1000) as u32; // SIPI vector = page number

    // INIT IPI
    send_ipi(apic_base, target_apic_id, 0x0000_4500);
    crate::interrupts::delay_ms(10);

    // SIPI #1
    send_ipi(apic_base, target_apic_id, 0x0000_4600 | vector);
    crate::interrupts::delay_ms(1);

    // Check if AP started
    if AP_STARTED.load(Ordering::Acquire) > started_before {
        return true;
    }

    // SIPI #2 (spec says retry once)
    send_ipi(apic_base, target_apic_id, 0x0000_4600 | vector);

    // Wait up to 100ms
    let timeout = crate::interrupts::tsc_freq() / 10;
    let t0 = crate::interrupts::rdtsc();
    while crate::interrupts::rdtsc() - t0 < timeout {
        if AP_STARTED.load(Ordering::Acquire) > started_before {
            return true;
        }
        core::hint::spin_loop();
    }

    false
}

/// Send IPI via Local APIC ICR (wait for idle first)
fn send_ipi(apic_base: u64, target_apic_id: u32, icr_low: u32) {
    // SAFETY: APIC MMIO is mapped. ICR write triggers IPI.
    unsafe {
        // Wait for delivery status = idle
        while core::ptr::read_volatile((apic_base + 0x300) as *const u32) & (1 << 12) != 0 {
            core::hint::spin_loop();
        }
        // Destination APIC ID (bits 24-31 of ICR high)
        core::ptr::write_volatile((apic_base + 0x310) as *mut u32, target_apic_id << 24);
        // Command (writing ICR low triggers the IPI)
        core::ptr::write_volatile((apic_base + 0x300) as *mut u32, icr_low);
    }
}

/// Allocate 64KB stack for an AP. Returns stack top (stacks grow down).
fn allocate_ap_stack() -> u64 {
    let pages = AP_STACK_SIZE / crate::memory::PAGE_SIZE;
    match crate::memory::allocate_contiguous(pages) {
        Some(base) => base + AP_STACK_SIZE as u64,
        None => 0,
    }
}

// ── ACPI MADT Parsing ──────────────────────────────────────────

/// Parse MADT (signature "APIC") and return all AP APIC IDs
fn parse_madt(bsp_apic_id: u32) -> Vec<u32> {
    let mut ap_ids = Vec::new();

    let madt_addr = match crate::acpi::find_table(b"APIC") {
        Some(addr) => addr,
        None => {
            kprintln!("[npk] smp: MADT not found");
            return ap_ids;
        }
    };

    // SAFETY: MADT is in identity-mapped memory. We validate bounds before reads.
    let madt_len = unsafe { *((madt_addr + 4) as *const u32) } as usize;
    if madt_len < 44 || madt_len > 0x10000 {
        return ap_ids;
    }
    crate::acpi::ensure_mapped_pub(madt_addr, madt_len);

    // Walk Interrupt Controller Structures (start at offset 44)
    let mut offset = 44;
    while offset + 2 <= madt_len {
        let entry_type = unsafe { *((madt_addr + offset) as *const u8) };
        let entry_len = unsafe { *((madt_addr + offset + 1) as *const u8) } as usize;
        if entry_len < 2 || offset + entry_len > madt_len { break; }

        match entry_type {
            // Type 0: Processor Local APIC (8-bit APIC ID)
            0 if entry_len >= 8 => {
                let apic_id = unsafe { *((madt_addr + offset + 3) as *const u8) } as u32;
                let flags = unsafe { *((madt_addr + offset + 4) as *const u32) };
                // bit 0 = Enabled, bit 1 = Online Capable
                if (flags & 0x03) != 0 && apic_id != bsp_apic_id {
                    ap_ids.push(apic_id);
                }
            }
            // Type 9: Processor Local x2APIC (32-bit APIC ID, for >255 cores)
            9 if entry_len >= 16 => {
                let x2apic_id = unsafe { *((madt_addr + offset + 4) as *const u32) };
                let flags = unsafe { *((madt_addr + offset + 8) as *const u32) };
                if (flags & 0x03) != 0 && x2apic_id != bsp_apic_id {
                    ap_ids.push(x2apic_id);
                }
            }
            _ => {}
        }

        offset += entry_len;
    }

    ap_ids
}
