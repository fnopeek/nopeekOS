//! VMX root-mode entry/exit + VMCS round-trip + VMLAUNCH — 12.1.0b…12.1.1c-3b3b2.
//!
//! Two consumer-facing entry points:
//!   - `enable_and_test()` — real-mode/32-bit-prot substrate test
//!     (9-byte stub `mov al,'O'; out 0x80,al; mov al,'K'; out 0x80,al; hlt`)
//!     used by `microvm test`.
//!   - `run_linux(bzimage, cmdline)` — Linux Boot Protocol 32-bit
//!     entry, used by `microvm linux`.
//!
//! Both share the VMXON region setup, VMCS allocation, VMCLEAR /
//! VMPTRLD, VMXOFF tear-down via `with_vmx_root_and_vmcs`. Guest
//! state, EPT, I/O bitmap, run-loop are caller-supplied.
//!
//! VMXON + VMCS regions are allocated and *kept* (never freed) per
//! call. CR4.VMXE is left set across calls (harmless).
//!
//! Reference: Intel SDM Vol. 3C §23.7 (Enabling VMX), §24.11.3
//! (Initializing a VMCS), §26.2-§26.4 (Host/Guest State), §27
//! (VM Exits).

use super::{ept, rdmsr, vmcs, wrmsr, bzimage};
use crate::mm::memory;

const IA32_FEATURE_CONTROL: u32 = 0x3A;
const IA32_VMX_BASIC: u32 = 0x480;
const IA32_VMX_CR0_FIXED0: u32 = 0x486;
const IA32_VMX_CR0_FIXED1: u32 = 0x487;
const IA32_VMX_CR4_FIXED0: u32 = 0x488;
const IA32_VMX_CR4_FIXED1: u32 = 0x489;

const FEAT_CTRL_LOCK: u64 = 1 << 0;
const FEAT_CTRL_VMX_OUTSIDE_SMX: u64 = 1 << 2;

const CR4_VMXE: u64 = 1 << 13;

const RFLAGS_CF: u64 = 1 << 0;
const RFLAGS_ZF: u64 = 1 << 6;

// ── VMXON / VMCS plumbing (shared by all entry points) ─────────────

/// Run `inner` inside VMX root mode with a fresh, current VMCS.
/// Handles all the VMXON-region / FEATURE_CONTROL / CR0+CR4 fixed-bit
/// dance once, allocates a 4-KB VMCS region, runs VMCLEAR + VMPTRLD,
/// then calls `inner` (which operates on the current VMCS via
/// VMREAD/VMWRITE / EPT / etc.). VMXOFF runs unconditionally on
/// return, even on inner error, so the CPU never strands in VMX
/// root mode.
fn with_vmx_root_and_vmcs<F, T>(inner: F) -> Result<T, &'static str>
where
    F: FnOnce() -> Result<T, &'static str>,
{
    // 1. VMXON region.
    let region_phys = memory::allocate_frame().ok_or("OOM allocating VMXON region")?;
    let basic = unsafe { rdmsr(IA32_VMX_BASIC) };
    let revision_id = (basic & 0x7FFF_FFFF) as u32;

    // SAFETY: identity-mapped, freshly-allocated, exclusive.
    unsafe {
        let region = region_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }

    // 2. FEATURE_CONTROL.
    let feat = unsafe { rdmsr(IA32_FEATURE_CONTROL) };
    if feat & FEAT_CTRL_LOCK == 0 {
        let new = feat | FEAT_CTRL_LOCK | FEAT_CTRL_VMX_OUTSIDE_SMX;
        // SAFETY: writing lock + outside-SMX bits to architectural MSR.
        unsafe { wrmsr(IA32_FEATURE_CONTROL, new); }
    } else if feat & FEAT_CTRL_VMX_OUTSIDE_SMX == 0 {
        return Err("IA32_FEATURE_CONTROL locked with VMX disabled (BIOS lock)");
    }

    // 3. CR0/CR4 fixed bits + CR4.VMXE.
    let cr0_f0 = unsafe { rdmsr(IA32_VMX_CR0_FIXED0) };
    let cr0_f1 = unsafe { rdmsr(IA32_VMX_CR0_FIXED1) };
    let cr4_f0 = unsafe { rdmsr(IA32_VMX_CR4_FIXED0) };
    let cr4_f1 = unsafe { rdmsr(IA32_VMX_CR4_FIXED1) };

    let mut cr0: u64;
    let mut cr4: u64;
    // SAFETY: CR reads cannot fault.
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, preserves_flags));
    }
    cr0 = (cr0 | cr0_f0) & cr0_f1;
    cr4 = ((cr4 | cr4_f0) & cr4_f1) | CR4_VMXE;
    // SAFETY: values satisfy fixed-bit constraints; VMXE is allowed.
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }

    // 4. VMXON.
    let region_addr_slot: u64 = region_phys;
    let rflags: u64;
    // SAFETY: VMXON requires CR4.VMXE (set above) + valid 4-KB
    // region with revision-id (set above).
    unsafe {
        core::arch::asm!(
            "vmxon [{addr}]",
            "pushfq",
            "pop {flags}",
            addr = in(reg) &region_addr_slot,
            flags = lateout(reg) rflags,
        );
    }
    if rflags & RFLAGS_CF != 0 {
        return Err("VMXON returned VMfailInvalid (CF=1)");
    }
    if rflags & RFLAGS_ZF != 0 {
        return Err("VMXON returned VMfailValid (ZF=1) — unexpected on first call");
    }

    // 5. VMCS region + VMCLEAR + VMPTRLD.
    let inner_result = vmcs_setup_then_inner(revision_id, inner);

    // 6. VMXOFF — always runs.
    // SAFETY: in VMX root mode (verified above).
    unsafe {
        core::arch::asm!("vmxoff", options(nostack, preserves_flags));
    }

    inner_result
}

fn vmcs_setup_then_inner<F, T>(revision_id: u32, inner: F) -> Result<T, &'static str>
where
    F: FnOnce() -> Result<T, &'static str>,
{
    let vmcs_phys = memory::allocate_frame().ok_or("OOM allocating VMCS region")?;

    // SAFETY: identity-mapped, freshly-allocated, exclusive.
    unsafe {
        let region = vmcs_phys as *mut u32;
        core::ptr::write_bytes(region as *mut u8, 0, 4096);
        region.write_volatile(revision_id);
    }

    let vmcs_addr_slot: u64 = vmcs_phys;

    // VMCLEAR.
    let rflags_clear: u64;
    // SAFETY: in VMX root mode; valid VMCS region.
    unsafe {
        core::arch::asm!(
            "vmclear [{addr}]",
            "pushfq",
            "pop {flags}",
            addr = in(reg) &vmcs_addr_slot,
            flags = lateout(reg) rflags_clear,
        );
    }
    if rflags_clear & RFLAGS_CF != 0 {
        return Err("VMCLEAR returned VMfailInvalid (CF=1)");
    }
    if rflags_clear & RFLAGS_ZF != 0 {
        return Err("VMCLEAR returned VMfailValid (ZF=1)");
    }

    // VMPTRLD.
    let rflags_load: u64;
    // SAFETY: in VMX root mode; VMCS just successfully VMCLEAR'd.
    unsafe {
        core::arch::asm!(
            "vmptrld [{addr}]",
            "pushfq",
            "pop {flags}",
            addr = in(reg) &vmcs_addr_slot,
            flags = lateout(reg) rflags_load,
        );
    }
    if rflags_load & RFLAGS_CF != 0 {
        return Err("VMPTRLD returned VMfailInvalid (CF=1)");
    }
    if rflags_load & RFLAGS_ZF != 0 {
        return Err("VMPTRLD returned VMfailValid (ZF=1)");
    }

    inner()
}

/// Sample current RSP and write it into HOST_RSP as a placeholder.
/// The real run-loop overrides HOST_RSP just-in-time before each
/// VMLAUNCH/VMRESUME — but the field must be canonical between
/// `setup_host_state` and the launch.
fn write_host_state_with_current_rsp() -> Result<(), &'static str> {
    let host_rsp: u64;
    // SAFETY: pure register read.
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) host_rsp, options(nostack, preserves_flags));
    }
    vmcs::setup_host_state(host_rsp)
}

/// Allocate a fresh 64 MB contiguous host-physical region for the
/// guest, install the EPT mapping it onto guest-phys [0, 64 MB),
/// return (host_base, eptp).
fn alloc_guest_ram_and_ept() -> Result<(u64, u64), &'static str> {
    let raw_base = memory::allocate_contiguous(
        ept::GUEST_RAM_FRAMES + ept::GUEST_RAM_ALIGN_SLACK,
    )
    .ok_or("OOM allocating 64 MB guest RAM (+ slack)")?;
    let host_base = ept::round_up_to_2mb(raw_base);
    let eptp = ept::install_window(host_base)?;
    Ok((host_base, eptp))
}

// ── Substrate test (12.1.1c-3b3a / 3b3b1) ──────────────────────────

/// Real-mode I/O-loop substrate test. Allocates fresh resources,
/// runs the 9-byte `out 0x80, 'O'; out 0x80, 'K'; hlt` stub,
/// returns the final VM-exit outcome. Used by `microvm test`.
pub fn enable_and_test() -> Result<vmcs::LaunchOutcome, &'static str> {
    with_vmx_root_and_vmcs(|| {
        let (host_base, eptp) = alloc_guest_ram_and_ept()?;

        // 9-byte substrate stub at guest-phys 0x10000.
        let stub_host = host_base + 0x10000;
        // SAFETY: host_base is 2-MB-aligned and the [host_base,
        // host_base + 64 MB) window is exclusively ours.
        unsafe {
            let page = stub_host as *mut u8;
            core::ptr::write_bytes(page, 0, 4096);
            page.add(0).write_volatile(0xB0); page.add(1).write_volatile(0x4F); // mov al, 'O'
            page.add(2).write_volatile(0xE6); page.add(3).write_volatile(0x80); // out 0x80, al
            page.add(4).write_volatile(0xB0); page.add(5).write_volatile(0x4B); // mov al, 'K'
            page.add(6).write_volatile(0xE6); page.add(7).write_volatile(0x80); // out 0x80, al
            page.add(8).write_volatile(0xF4);                                    // hlt
        }

        write_host_state_with_current_rsp()?;
        vmcs::setup_guest_state(0x10000)?;
        vmcs::setup_execution_controls(eptp)?;

        run_substrate_loop()
    })
}

/// Loop dispatch for the substrate test: HLT terminates, OUT
/// captures the byte for the "OK" reconstruction, anything else
/// breaks with a log line.
fn run_substrate_loop() -> Result<vmcs::LaunchOutcome, &'static str> {
    use crate::kprintln;

    const MAX_ITERATIONS: u32 = 1024;

    let mut regs = vmcs::GuestRegs::default();
    let mut launched = false;
    let mut last_outcome: Option<vmcs::LaunchOutcome> = None;
    let mut io_count: u32 = 0;
    let mut io_bytes: [u8; 32] = [0; 32];
    let mut io_byte_n: usize = 0;

    for _ in 0..MAX_ITERATIONS {
        let outcome = vmcs::run_guest_once(&mut regs, launched)?;
        launched = true;
        let basic = vmcs::basic_exit_reason(outcome.exit_reason);

        match basic {
            12 => {
                kprintln!("[microvm] guest HLT after {} I/O exit(s)", io_count);
                if io_byte_n > 0 {
                    let mut printable = [0u8; 32];
                    for i in 0..io_byte_n {
                        printable[i] = if io_bytes[i].is_ascii_graphic() || io_bytes[i] == b' ' {
                            io_bytes[i]
                        } else {
                            b'.'
                        };
                    }
                    let s = core::str::from_utf8(&printable[..io_byte_n]).unwrap_or("?");
                    kprintln!("[microvm]   captured byte stream: \"{}\"", s);
                }
                last_outcome = Some(outcome);
                break;
            }
            30 => {
                io_count += 1;
                let (port, dir_in, size) =
                    vmcs::decode_io_exit_qualification(outcome.exit_qualification);
                let value = regs.rax & match size {
                    1 => 0xFF, 2 => 0xFFFF, 4 => 0xFFFF_FFFF, _ => 0xFF,
                };
                let dir = if dir_in { "IN" } else { "OUT" };
                kprintln!(
                    "[microvm]   {} port {:#06x} size={} value={:#x}",
                    dir, port, size, value,
                );
                if !dir_in && size == 1 && io_byte_n < io_bytes.len() {
                    io_bytes[io_byte_n] = value as u8;
                    io_byte_n += 1;
                }
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            _ => {
                kprintln!(
                    "[microvm] guest unhandled exit reason {} qual {:#x}",
                    basic, outcome.exit_qualification,
                );
                last_outcome = Some(outcome);
                break;
            }
        }
    }

    last_outcome.ok_or("guest exceeded max iterations without HLT")
}

// ── Linux launcher (12.1.1c-3b3b2) ─────────────────────────────────

/// Boot a Linux bzImage in our MicroVM substrate. Loads the bzImage
/// parts into a fresh 64 MB guest, configures 32-bit-prot-mode
/// entry per Linux Boot Protocol, runs a serial-aware exit loop
/// that captures Linux's earlyprintk output via the I/O bitmap.
///
/// `bzimage` is the raw bzImage bytes. `cmdline` is the kernel
/// command line (no NUL — loader appends one).
pub fn run_linux(bzimage: &[u8], cmdline: &[u8]) -> Result<vmcs::LaunchOutcome, &'static str> {
    with_vmx_root_and_vmcs(|| {
        let (host_base, eptp) = alloc_guest_ram_and_ept()?;

        let load = bzimage::load_into_guest_ram(host_base, bzimage, cmdline)?;

        write_host_state_with_current_rsp()?;
        vmcs::setup_guest_state(load.entry_rip)?;
        vmcs::setup_execution_controls(eptp)?;

        run_linux_loop(load.boot_params_phys, host_base)
    })
}

/// Per-guest serial UART state across exits.
struct SerialState {
    /// LCR.DLAB bit. When set, OUT to 0x3F8 / 0x3F9 means
    /// divisor-latch low/high (we ignore). When clear, 0x3F8 is
    /// THR (the byte the kernel wants to print).
    dlab: bool,
    /// Buffered output line — flushed via kprintln on '\n' or
    /// when the buffer is full. Linux's printk emits one line at
    /// a time so this rarely fills.
    line: [u8; 256],
    line_n: usize,
}

impl SerialState {
    fn new() -> Self {
        Self { dlab: false, line: [0; 256], line_n: 0 }
    }
}

impl SerialState {
    fn put_char(&mut self, byte: u8) {
        use crate::kprintln;
        if byte == b'\n' || self.line_n == self.line.len() {
            let n = self.line_n;
            let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
            kprintln!("[guest] {}", s);
            self.line_n = 0;
            return;
        }
        if byte != b'\r' {
            self.line[self.line_n] = byte;
            self.line_n += 1;
        }
    }

    fn flush(&mut self) {
        use crate::kprintln;
        if self.line_n > 0 {
            let n = self.line_n;
            let s = core::str::from_utf8(&self.line[..n]).unwrap_or("?");
            kprintln!("[guest] {}", s);
            self.line_n = 0;
        }
    }
}

/// Per-port I/O exit counter. Linux's boot touches dozens of unique
/// ports (PCI config, PIC, PIT, RTC, serial, keyboard, etc.).
/// Counting them tells us what the guest actually did when no
/// `[guest]` lines appeared.
struct IoStats {
    counts: [(u16, u32); 64],
    n: usize,
    /// First N bytes written to UART THR (port 0x3F8 with DLAB=0).
    serial_bytes: [u8; 256],
    serial_n: usize,
}

impl IoStats {
    fn new() -> Self {
        Self {
            counts: [(0, 0); 64],
            n: 0,
            serial_bytes: [0; 256],
            serial_n: 0,
        }
    }
    fn record(&mut self, port: u16, _dir_in: bool) {
        for i in 0..self.n {
            if self.counts[i].0 == port {
                self.counts[i].1 += 1;
                return;
            }
        }
        if self.n < self.counts.len() {
            self.counts[self.n] = (port, 1);
            self.n += 1;
        }
    }
    fn record_serial_byte(&mut self, byte: u8) {
        if self.serial_n < self.serial_bytes.len() {
            self.serial_bytes[self.serial_n] = byte;
            self.serial_n += 1;
        }
    }
    fn dump(&self) {
        use crate::kprintln;
        kprintln!("[microvm] I/O port summary ({} unique):", self.n);
        for i in 0..self.n {
            kprintln!("[microvm]   port {:#06x}: {:>5} accesses", self.counts[i].0, self.counts[i].1);
        }
        if self.serial_n > 0 {
            kprintln!("[microvm] {} bytes written to 0x3F8 (DLAB=0):", self.serial_n);
            // Print as ASCII-safe + hex-on-non-printable
            let mut buf: [u8; 256] = [0; 256];
            for i in 0..self.serial_n {
                let b = self.serial_bytes[i];
                buf[i] = if b.is_ascii_graphic() || b == b' ' || b == b'\n' { b } else { b'.' };
            }
            let s = core::str::from_utf8(&buf[..self.serial_n]).unwrap_or("?");
            kprintln!("[microvm]   '{}'", s);
        } else {
            kprintln!("[microvm] zero bytes ever reached 0x3F8 (DLAB=0)");
        }
    }
}

/// Per-iteration exit trace recorded for post-mortem on unhandled
/// exits. Keeps the last 32 (reason, qual_low32) tuples so we can
/// see what Linux was doing in the run-up to a triple-fault.
struct ExitTrace {
    items: [(u16, u32); 32],
    n: usize,
}

impl ExitTrace {
    fn new() -> Self {
        Self { items: [(0, 0); 32], n: 0 }
    }
    fn record(&mut self, reason: u16, qual: u64) {
        let idx = self.n % 32;
        self.items[idx] = (reason, qual as u32);
        self.n += 1;
    }
    fn dump(&self) {
        use crate::kprintln;
        let count = self.n.min(32);
        let start = if self.n > 32 { self.n - 32 } else { 0 };
        kprintln!("[microvm-trace] last {} exits:", count);
        for i in 0..count {
            let (r, q) = self.items[(start + i) % 32];
            kprintln!("[microvm-trace]   #{}: reason {:>3} qual {:#010x}", start + i, r, q);
        }
    }
}

/// Walk guest's 4-level page tables for `virt`, print each level's
/// entry. EPT identity-shifts guest-phys X → host-phys host_base+X
/// within the 64 MB window, so we just offset.
fn dump_page_walk(host_base: u64, cr3: u64, virt: u64) {
    use crate::kprintln;
    const WINDOW: u64 = 64 * 1024 * 1024;
    const PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    let l4 = ((virt >> 39) & 0x1FF) as usize;
    let l3 = ((virt >> 30) & 0x1FF) as usize;
    let l2 = ((virt >> 21) & 0x1FF) as usize;
    let l1 = ((virt >> 12) & 0x1FF) as usize;
    kprintln!("[microvm-walk] CR3 = {:#018x}, virt = {:#018x}", cr3, virt);
    kprintln!("[microvm-walk] indices: L4={} L3={} L2={} L1={}", l4, l3, l2, l1);

    let pml4_phys = cr3 & PHYS_MASK;
    if pml4_phys >= WINDOW {
        kprintln!("[microvm-walk] PML4 phys {:#x} outside 64 MB window", pml4_phys);
        return;
    }
    let pml4_e = unsafe {
        ((host_base + pml4_phys) as *const u64).add(l4).read_volatile()
    };
    kprintln!("[microvm-walk]   PML4[{}] = {:#018x}", l4, pml4_e);
    if pml4_e & 1 == 0 { kprintln!("[microvm-walk]     L4 not present"); return; }

    let pdpt_phys = pml4_e & PHYS_MASK;
    if pdpt_phys >= WINDOW {
        kprintln!("[microvm-walk]   PDPT phys {:#x} outside window", pdpt_phys);
        return;
    }
    let pdpt_e = unsafe {
        ((host_base + pdpt_phys) as *const u64).add(l3).read_volatile()
    };
    kprintln!("[microvm-walk]   PDPT[{}] = {:#018x}", l3, pdpt_e);
    if pdpt_e & 1 == 0 { kprintln!("[microvm-walk]     L3 not present"); return; }
    if pdpt_e & (1 << 7) != 0 { kprintln!("[microvm-walk]     1 GB leaf"); return; }

    let pd_phys = pdpt_e & PHYS_MASK;
    if pd_phys >= WINDOW {
        kprintln!("[microvm-walk]   PD phys {:#x} outside window", pd_phys);
        return;
    }
    let pd_e = unsafe {
        ((host_base + pd_phys) as *const u64).add(l2).read_volatile()
    };
    kprintln!("[microvm-walk]   PD[{}] = {:#018x}", l2, pd_e);
    if pd_e & 1 == 0 { kprintln!("[microvm-walk]     L2 not present"); return; }
    if pd_e & (1 << 7) != 0 { kprintln!("[microvm-walk]     2 MB leaf"); return; }

    let pt_phys = pd_e & PHYS_MASK;
    if pt_phys >= WINDOW {
        kprintln!("[microvm-walk]   PT phys {:#x} outside window", pt_phys);
        return;
    }
    let pt_e = unsafe {
        ((host_base + pt_phys) as *const u64).add(l1).read_volatile()
    };
    kprintln!("[microvm-walk]   PT[{}] = {:#018x}", l1, pt_e);
    if pt_e & 1 == 0 { kprintln!("[microvm-walk]     L1 not present"); }
}

/// Linux run-loop. Initial guest GPR ESI = boot_params_phys per
/// 32-bit boot protocol; other GPRs zero. Caps at 100k iterations.
fn run_linux_loop(
    boot_params_phys: u64,
    host_base: u64,
) -> Result<vmcs::LaunchOutcome, &'static str> {
    use crate::kprintln;

    const MAX_ITERATIONS: u32 = 100_000;

    let mut regs = vmcs::GuestRegs::default();
    regs.rsi = boot_params_phys;

    let mut serial = SerialState::new();
    let mut trace = ExitTrace::new();
    let mut io_stats = IoStats::new();
    let mut launched = false;
    let mut last_outcome: Option<vmcs::LaunchOutcome> = None;
    let mut iter: u32 = 0;
    let mut io_dropped: u32 = 0;

    while iter < MAX_ITERATIONS {
        iter += 1;
        // Before each entry, sync the IA-32e-mode-guest control to
        // the current GUEST_IA32_EFER.LMA — once Linux flips into
        // long mode (after CR0.PG=1 with EFER.LME=1), the entry
        // control must match or VMX rejects the entry.
        if launched {
            vmcs::sync_entry_ia32e_with_efer()?;
        }
        let outcome = vmcs::run_guest_once(&mut regs, launched)?;
        launched = true;
        let basic = vmcs::basic_exit_reason(outcome.exit_reason);
        trace.record(basic, outcome.exit_qualification);

        match basic {
            0 => {
                // Exception/NMI. EXCEPTION_BITMAP=0 in production —
                // exceptions go to Linux's IDT directly. This arm
                // only fires for NMIs (which we don't generate
                // intentionally) or if Linux somehow re-enables
                // exception trapping. Kept as a safety net + the
                // dump remains useful if it ever fires.
                serial.flush();
                let info = vmcs::read_exit_intr_info().unwrap_or(0);
                let vector = info & 0xFF;
                let intr_type = (info >> 8) & 0x7;
                let err_valid = (info >> 11) & 0x1 != 0;
                let err_code = if err_valid {
                    vmcs::read_exit_intr_error_code().unwrap_or(0)
                } else {
                    0
                };
                let mnemonic = match vector {
                    0 => "DE", 1 => "DB", 2 => "NMI", 3 => "BP",
                    4 => "OF", 5 => "BR", 6 => "UD", 7 => "NM",
                    8 => "DF", 10 => "TS", 11 => "NP", 12 => "SS",
                    13 => "GP", 14 => "PF", 16 => "MF", 17 => "AC",
                    18 => "MC", 19 => "XM", 20 => "VE", 21 => "CP",
                    _ => "??",
                };
                kprintln!(
                    "[microvm] guest exception #{} ({}) type={} qual={:#x} err_valid={} err_code={:#x}",
                    vector, mnemonic, intr_type, outcome.exit_qualification,
                    err_valid, err_code,
                );
                let rip   = vmcs::read_guest_rip().unwrap_or(0);
                let cr0   = vmcs::read_guest_cr0().unwrap_or(0);
                let cr4   = vmcs::read_guest_cr4().unwrap_or(0);
                let efer  = vmcs::read_guest_efer().unwrap_or(0);
                let cs    = vmcs::read_guest_cs_selector().unwrap_or(0);
                let entry = vmcs::read_vm_entry_controls().unwrap_or(0);
                kprintln!(
                    "[microvm]   GUEST_RIP  = {:#018x}  GUEST_CS = {:#06x}",
                    rip, cs,
                );
                kprintln!(
                    "[microvm]   GUEST_CR0  = {:#018x}  GUEST_CR4 = {:#018x}",
                    cr0, cr4,
                );
                kprintln!(
                    "[microvm]   GUEST_EFER = {:#018x}  ENTRY_CTLS = {:#010x}",
                    efer, entry,
                );
                if vector == 14 {
                    let cr3 = vmcs::read_guest_cr3().unwrap_or(0);
                    dump_page_walk(host_base, cr3, outcome.exit_qualification);
                }
                trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            12 => {
                serial.flush();
                kprintln!("[microvm] guest HLT after {} VM-exits", iter);
                io_stats.dump();
                last_outcome = Some(outcome);
                break;
            }
            10 => {
                // CPUID — VMX always exits on CPUID. Pass through
                // to host; guest sees real CPU features. Linux uses
                // this for early feature detection.
                let leaf = regs.rax as u32;
                let subleaf = regs.rcx as u32;
                let (eax, ebx, ecx, edx) = vmcs::host_cpuid(leaf, subleaf);
                regs.rax = eax as u64;
                regs.rbx = ebx as u64;
                regs.rcx = ecx as u64;
                regs.rdx = edx as u64;
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            28 => {
                // Control-register access. Most commonly Linux's
                // startup_32 doing MOV CR3, reg to load its own
                // page tables — IA32_VMX_PROCBASED_CTLS may force
                // CR3-load/store-exiting on this CPU even with EPT.
                let qual = outcome.exit_qualification;
                let cr_num = (qual & 0xF) as u8;
                let access_type = ((qual >> 4) & 0x3) as u8;
                let gp_reg = ((qual >> 8) & 0xF) as u8;

                if cr_num != 3 {
                    serial.flush();
                    kprintln!(
                        "[microvm] unhandled CR{} access (type {}, reg {}, qual {:#x})",
                        cr_num, access_type, gp_reg, qual,
                    );
                    last_outcome = Some(outcome);
                    break;
                }
                match access_type {
                    0 => {
                        // MOV to CR3 (set page-table base).
                        let val = read_gpr(&regs, gp_reg)?;
                        vmcs::write_guest_cr3(val)?;
                    }
                    1 => {
                        // MOV from CR3.
                        let val = vmcs::read_guest_cr3()?;
                        write_gpr(&mut regs, gp_reg, val)?;
                    }
                    _ => {
                        serial.flush();
                        kprintln!(
                            "[microvm] CR3 unusual access type {} (qual {:#x})",
                            access_type, qual,
                        );
                        last_outcome = Some(outcome);
                        break;
                    }
                }
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            30 => {
                let (port, dir_in, size) =
                    vmcs::decode_io_exit_qualification(outcome.exit_qualification);
                io_stats.record(port, dir_in);
                if port == 0x3F8 && !dir_in && !serial.dlab && size == 1 {
                    io_stats.record_serial_byte((regs.rax & 0xFF) as u8);
                }
                handle_linux_io(&mut serial, &mut regs, port, dir_in, size, &mut io_dropped);
                vmcs::advance_guest_rip()?;
                last_outcome = Some(outcome);
            }
            48 => {
                // EPT violation — guest tried to access a guest-phys
                // address outside our 64 MB window (or with insufficient
                // EPT permissions). The qual decodes which permission
                // was missing; GUEST_PHYSICAL_ADDRESS is the actual
                // address.
                serial.flush();
                let gpa  = vmcs::read_guest_phys_addr().unwrap_or(0);
                let gla  = vmcs::read_guest_linear_addr().unwrap_or(0);
                let q    = outcome.exit_qualification;
                let read = q & 1 != 0;
                let write = q & 2 != 0;
                let fetch = q & 4 != 0;
                kprintln!(
                    "[microvm] EPT violation: gpa={:#018x} gla={:#018x} qual={:#x}",
                    gpa, gla, q,
                );
                kprintln!(
                    "[microvm]   access: {}{}{}",
                    if read { "R" } else { "" },
                    if write { "W" } else { "" },
                    if fetch { "X" } else { "" },
                );
                io_stats.dump();
                trace.dump();
                last_outcome = Some(outcome);
                break;
            }
            _ => {
                serial.flush();
                kprintln!(
                    "[microvm] unhandled exit reason {} qual {:#x} after {} iters",
                    basic, outcome.exit_qualification, iter,
                );
                io_stats.dump();
                trace.dump();
                last_outcome = Some(outcome);
                break;
            }
        }
    }

    if iter >= MAX_ITERATIONS {
        serial.flush();
        kprintln!(
            "[microvm] iteration cap ({}) reached — guest still running, ({} I/O drops)",
            MAX_ITERATIONS, io_dropped,
        );
        io_stats.dump();
        trace.dump();
    }

    last_outcome.ok_or("Linux guest exceeded max iterations without first VM-exit")
}

/// Read a guest GPR by ABI register index (0=rax, 1=rcx, 2=rdx,
/// 3=rbx, 4=rsp, 5=rbp, 6=rsi, 7=rdi, 8..15=r8..r15) for CR-access
/// VM-exit decoding. RSP comes from VMCS, the rest from the saved
/// GuestRegs struct.
fn read_gpr(regs: &vmcs::GuestRegs, idx: u8) -> Result<u64, &'static str> {
    Ok(match idx {
        0 => regs.rax,
        1 => regs.rcx,
        2 => regs.rdx,
        3 => regs.rbx,
        4 => vmcs::read_guest_rsp()?,
        5 => regs.rbp,
        6 => regs.rsi,
        7 => regs.rdi,
        8 => regs.r8,
        9 => regs.r9,
        10 => regs.r10,
        11 => regs.r11,
        12 => regs.r12,
        13 => regs.r13,
        14 => regs.r14,
        15 => regs.r15,
        _ => return Err("invalid GPR index"),
    })
}

/// Write a guest GPR by ABI register index. RSP goes to VMCS, the
/// rest to the saved GuestRegs struct.
fn write_gpr(regs: &mut vmcs::GuestRegs, idx: u8, value: u64) -> Result<(), &'static str> {
    match idx {
        0 => regs.rax = value,
        1 => regs.rcx = value,
        2 => regs.rdx = value,
        3 => regs.rbx = value,
        4 => vmcs::write_guest_rsp(value)?,
        5 => regs.rbp = value,
        6 => regs.rsi = value,
        7 => regs.rdi = value,
        8 => regs.r8 = value,
        9 => regs.r9 = value,
        10 => regs.r10 = value,
        11 => regs.r11 = value,
        12 => regs.r12 = value,
        13 => regs.r13 = value,
        14 => regs.r14 = value,
        15 => regs.r15 = value,
        _ => return Err("invalid GPR index"),
    }
    Ok(())
}

/// Dispatch a single I/O VM-exit. UART COM1 (0x3F8-0x3FF) gets
/// proper synthetic responses so Linux's earlyprintk poll-loop
/// thinks the transmitter is always ready; everything else is
/// silently absorbed (return 0 for IN, no-op for OUT).
fn handle_linux_io(
    serial: &mut SerialState,
    regs: &mut vmcs::GuestRegs,
    port: u16,
    dir_in: bool,
    size: u8,
    io_dropped: &mut u32,
) {
    let mask: u64 = match size { 1 => 0xFF, 2 => 0xFFFF, 4 => 0xFFFF_FFFF, _ => 0xFF };
    let val_out = (regs.rax & mask) as u32;

    match (port, dir_in) {
        // COM1 OUT.
        (0x3F8, false) => {
            if !serial.dlab {
                serial.put_char(val_out as u8);
            }
            // else: divisor-latch low byte, ignored.
        }
        (0x3F9, false) => {
            // IER (DLAB=0) or DLM (DLAB=1) — both ignored.
        }
        (0x3FB, false) => {
            // LCR — track DLAB bit.
            serial.dlab = (val_out & 0x80) != 0;
        }
        // COM1 IN — synthetic responses.
        (0x3F8, true) => {
            // RBR (no incoming data) or DLL — return 0.
            regs.rax = (regs.rax & !mask) | (0u64 & mask);
        }
        (0x3FA, true) => {
            // IIR: bit 0 = "no interrupt pending" (which on read
            // also sources type=0 = no FIFO).
            regs.rax = (regs.rax & !mask) | (0x01u64 & mask);
        }
        (0x3FD, true) => {
            // LSR: bit 5 = THR empty, bit 6 = TSR empty.
            // Always ready → polling loops never spin.
            regs.rax = (regs.rax & !mask) | (0x60u64 & mask);
        }
        (0x3FE, true) => {
            // MSR: CTS asserted (bit 4) + DSR (bit 5) + DCD (bit 7).
            regs.rax = (regs.rax & !mask) | (0xB0u64 & mask);
        }
        // Other UART regs (0x3FC MCR, 0x3FF SCR): default 0.
        (0x3FA..=0x3FF, true) => {
            regs.rax = (regs.rax & !mask) | (0u64 & mask);
        }
        // Default IN: zero. Default OUT: drop.
        (_, true) => {
            regs.rax = (regs.rax & !mask) | (0u64 & mask);
            *io_dropped += 1;
        }
        (_, false) => {
            *io_dropped += 1;
        }
    }
}
