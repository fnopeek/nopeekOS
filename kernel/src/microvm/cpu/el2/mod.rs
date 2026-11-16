//! The Arm virtualization extensions as KVM/arm64 uses them on a VHE host
//! (`arch/arm64/kvm/`): the kernel owns EL2, a vCPU is EL1 state loaded
//! into the real EL1 registers, the guest's memory is stage-2 translated
//! (`stage2`), its interrupt controller a GICv2 whose distributor is
//! emulated and whose CPU interface is the hardware's (`vgic`), its timer
//! the architected virtual timer, and its firmware PSCI over HVC.
//!
//! Exits (`handle_exit`, KVM `handle_exit.c` and `mmio.c`):
//!   * an interrupt: the host takes it once its own state is back;
//!   * a stage-2 abort: first touch of guest RAM, an instruction fetch
//!     from a page not yet executable, or MMIO, decoded from the syndrome
//!     (ISV), into the distributor, the UART, PCI config space (ECAM) or
//!     a virtio BAR, the same device models the x86 guest uses;
//!   * HVC: PSCI and the SMCCC discovery calls (`psci.c`, `hypercalls.c`);
//!   * SMC: not implemented, as KVM answers it;
//!   * a trapped system register (`sys_regs.c`): sanitized ID registers,
//!     the debug registers, the PMU, RAS and LOR registers;
//!   * WFI: the vCPU blocks until an interrupt is pending or its timer
//!     fires (`kvm_vcpu_wfi`).

pub mod platform;
pub mod stage2;
pub mod switch;
pub mod uart;
pub mod vgic;

use crate::microvm::cpu::Vendor;
use crate::microvm::devices::guest_mem::{GuestMem, RamLayout, SecondLevel};
use crate::mm::memory;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use switch::{rd, GuestRegs, HostCtl, HostSave, SysRegs, Traps};

/// What ended a run, for the log (`exit_reason` = ESR_EL2.EC, or 0x100 +
/// the exit kind; `exit_qualification` = the ESR or the PSCI function).
#[derive(Clone, Copy, Debug, Default)]
pub struct LaunchOutcome {
    pub exit_reason: u64,
    pub exit_qualification: u64,
}

pub enum SliceOutcome {
    StillRunning,
    Idle,
    Exited(LaunchOutcome),
}

/// Exit reasons that are not an exception class.
pub const REASON_SYSTEM_OFF: u64 = 0x200;
pub const REASON_SYSTEM_RESET: u64 = 0x201;
pub const REASON_CPU_OFF: u64 = 0x202;

// HCR_EL2 (`HCR_GUEST_FLAGS`, `HCR_HOST_VHE_FLAGS`).
const HCR_VM: u64 = 1 << 0;
const HCR_SWIO: u64 = 1 << 1;
const HCR_PTW: u64 = 1 << 2;
const HCR_FMO: u64 = 1 << 3;
const HCR_IMO: u64 = 1 << 4;
const HCR_AMO: u64 = 1 << 5;
const HCR_FB: u64 = 1 << 9;
const HCR_BSU_IS: u64 = 1 << 10;
const HCR_TWI: u64 = 1 << 13;
const HCR_TID3: u64 = 1 << 18;
const HCR_TSC: u64 = 1 << 19;
const HCR_TIDCP: u64 = 1 << 20;
const HCR_TACR: u64 = 1 << 21;
const HCR_TSW: u64 = 1 << 22;
const HCR_RW: u64 = 1 << 31;
const HCR_E2H: u64 = 1 << 34;
const HCR_TLOR: u64 = 1 << 35;
const HCR_TERR: u64 = 1 << 36;
const HCR_TEA: u64 = 1 << 37;

// MDCR_EL2 (`kvm_arm_setup_mdcr_el2`): PMU and debug registers trapped.
const MDCR_HPMN_MASK: u64 = 0x1f;
const MDCR_TPMCR: u64 = 1 << 5;
const MDCR_TPM: u64 = 1 << 6;
const MDCR_TDA: u64 = 1 << 9;
const MDCR_TDOSA: u64 = 1 << 10;
const MDCR_TDRA: u64 = 1 << 11;

// CNTHCTL_EL2 with E2H: EL1 physical counter yes, physical timer trapped.
const CNTHCTL_EL1PCTEN: u64 = 1 << 10;
const CNTHCTL_EL1PTEN: u64 = 1 << 11;

/// EL1h with D, A, I and F masked: the reset PSTATE (`kvm_reset_vcpu`).
const PSTATE_RESET: u64 = 0x3c5;
/// SCTLR_EL1 with its RES1 bits only: MMU and caches off.
const SCTLR_RESET: u64 = 0x30d0_0800;

// ESR_ELx exception classes.
const EC_WFX: u64 = 0x01;
const EC_HVC64: u64 = 0x16;
const EC_SMC64: u64 = 0x17;
const EC_SYS64: u64 = 0x18;
const EC_IABT_LOW: u64 = 0x20;
const EC_DABT_LOW: u64 = 0x24;
const EC_SERROR: u64 = 0x2f;

// PSCI 1.0 and SMCCC (`include/uapi/linux/psci.h`, `arm-smccc.h`).
const PSCI_VERSION: u32 = 0x8400_0000;
const PSCI_CPU_SUSPEND: u32 = 0x8400_0001;
const PSCI_CPU_OFF: u32 = 0x8400_0002;
const PSCI_CPU_ON: u32 = 0x8400_0003;
const PSCI_AFFINITY_INFO: u32 = 0x8400_0004;
const PSCI_MIGRATE_INFO_TYPE: u32 = 0x8400_0006;
const PSCI_SYSTEM_OFF: u32 = 0x8400_0008;
const PSCI_SYSTEM_RESET: u32 = 0x8400_0009;
const PSCI_FEATURES: u32 = 0x8400_000a;
const PSCI_64: u32 = 0x4000_0000;
const SMCCC_VERSION: u32 = 0x8000_0000;
const SMCCC_ARCH_FEATURES: u32 = 0x8000_0001;
const PSCI_RET_SUCCESS: u64 = 0;
const PSCI_RET_NOT_SUPPORTED: u64 = -1i64 as u64;
const PSCI_RET_INVALID_PARAMS: u64 = -2i64 as u64;
const PSCI_RET_ALREADY_ON: u64 = -4i64 as u64;
const PSCI_TOS_NOT_PRESENT_MP: u64 = 2;

static READY: AtomicBool = AtomicBool::new(false);
static RAS: AtomicBool = AtomicBool::new(false);
static LOR: AtomicBool = AtomicBool::new(false);
static HOST_ACTLR: AtomicU64 = AtomicU64::new(0);
static VTCR: AtomicU64 = AtomicU64::new(0);
static WHY_NOT: spin::Mutex<&'static str> = spin::Mutex::new("not probed");

/// Is this CPU and its GIC able to run a guest: EL2 with VHE, a stage-2
/// IPA of 40 bits, a GICv2 with its hypervisor interface.
pub fn detect() -> Vendor {
    let why = probe();
    *WHY_NOT.lock() = why.unwrap_or("");
    match why {
        None => Vendor::Arm,
        Some(r) => Vendor::Unknown(r),
    }
}

fn probe() -> Option<&'static str> {
    if crate::arch::exception_level() != 2 {
        return Some("the kernel does not own EL2");
    }
    let Some(vtcr) = stage2::vtcr() else { return Some("fewer than 40 physical address bits") };
    let Some(fdt) = crate::platform::dt::boot() else { return Some("no device tree") };
    if !vgic::probe(&fdt) {
        return Some("the GIC has no virtualization interface in the device tree");
    }
    if vgic::nr_lr() == 0 {
        return Some("the GIC reports no list registers");
    }
    VTCR.store(vtcr, Ordering::Relaxed);
    let pfr0 = rd!("ID_AA64PFR0_EL1");
    RAS.store((pfr0 >> 28) & 0xf != 0, Ordering::Relaxed);
    let mmfr1 = rd!("ID_AA64MMFR1_EL1");
    LOR.store((mmfr1 >> 16) & 0xf != 0, Ordering::Relaxed);
    HOST_ACTLR.store(rd!("ACTLR_EL1"), Ordering::Relaxed);
    None
}

pub fn init() {
    READY.store(true, Ordering::Release);
    crate::kprintln!("[microvm] EL2 (VHE): stage 2 with {}-bit IPA, GICv2 with {} list registers, maintenance IRQ {}",
        stage2::IPA_BITS, vgic::nr_lr(), vgic::maintenance_irq());
}

pub fn report() {
    if READY.load(Ordering::Acquire) {
        crate::kprintln!("[microvm] Arm virtualization: EL2 + VHE, {} list registers, RAS {}",
            vgic::nr_lr(), if RAS.load(Ordering::Relaxed) { "yes" } else { "no" });
    } else {
        crate::kprintln!("[microvm] Arm virtualization unavailable: {}", *WHY_NOT.lock());
    }
}

/// Spectre-BHB on exit (`spectre_bhb_loop_affected`): the loop count of
/// this core, by MIDR.
pub fn arm_exit_mitigations() {
    let midr = rd!("MIDR_EL1");
    let implementer = (midr >> 24) & 0xff;
    let part = (midr >> 4) & 0xfff;
    let k = match (implementer, part) {
        // Cortex-A76, A77, Neoverse N1.
        (0x41, 0xd0b) | (0x41, 0xd0d) | (0x41, 0xd0c) => 24,
        // Cortex-A78, A78C, X1, Neoverse V1.
        (0x41, 0xd41) | (0x41, 0xd4b) | (0x41, 0xd44) | (0x41, 0xd40) => 32,
        // Cortex-A72, A57.
        (0x41, 0xd08) | (0x41, 0xd07) => 8,
        _ => 0,
    };
    switch::NPK_EL2_BHB_K.store(k, Ordering::Release);
    crate::kprintln!("[microvm] branch history cleared on VM exit: {}",
        if k != 0 { "yes" } else { "not needed" });
}

// ── Per-VM state ────────────────────────────────────────────────────

pub struct VmShared {
    guest_mem: &'static GuestMem,
    root: u64,
    guest_raw_base: u64,
    vcpus: u8,
    cntvoff: u64,
    dev: spin::Mutex<VmDevices>,
}

struct VmDevices {
    uart: uart::Uart,
    pci: crate::microvm::devices::PciBus,
    last_reap_tick: u64,
}

static AP_SHARED: spin::Mutex<Option<Arc<VmShared>>> = spin::Mutex::new(None);

pub fn clear_ap_shared() {
    let old = AP_SHARED.lock().take();
    drop(old);
}

/// PSCI power state of each vCPU (`kvm_arm_vcpu_power_off`).
const POWER_OFF: u8 = 0;
const POWER_ON_PENDING: u8 = 1;
const POWER_ON: u8 = 2;
static POWER: [AtomicU8; vgic::MAX_VCPUS] = [const { AtomicU8::new(POWER_OFF) }; vgic::MAX_VCPUS];
/// CPU_ON's entry point and context id, per vCPU.
static BOOT_ENTRY: [AtomicU64; vgic::MAX_VCPUS] = [const { AtomicU64::new(0) }; vgic::MAX_VCPUS];
static BOOT_CTX: [AtomicU64; vgic::MAX_VCPUS] = [const { AtomicU64::new(0) }; vgic::MAX_VCPUS];
/// The host core a vCPU runs on (`usize::MAX` = none), for kicks.
static VCPU_CORE: [AtomicUsize; vgic::MAX_VCPUS] = [const { AtomicUsize::new(usize::MAX) }; vgic::MAX_VCPUS];
/// Each vCPU's priority mask and timer deadline, for wake checks made from
/// outside its context (`wake_pending`).
static VCPU_PMR: [AtomicU8; vgic::MAX_VCPUS] = [const { AtomicU8::new(0) }; vgic::MAX_VCPUS];
static VTIMER_DEADLINE: [AtomicU64; vgic::MAX_VCPUS] = [const { AtomicU64::new(0) }; vgic::MAX_VCPUS];

/// Kick the vCPUs in `mask` other than `me` (`kvm_vcpu_kick`).
fn kick_mask(mask: u8, me: u8) {
    for v in 0..vgic::MAX_VCPUS as u8 {
        if mask & (1 << v) == 0 || v == me { continue; }
        let c = VCPU_CORE[v as usize].load(Ordering::Acquire);
        if c != usize::MAX {
            crate::smp::kick_host_core(c);
        }
    }
}

/// Raise device line `line` (an SPI) and wake whoever it targets.
fn raise_line(line: u8, me: u8) {
    if let Some(t) = vgic::pulse_spi(line as u32) {
        kick_mask(1 << t, me);
    }
}

/// Something is pending for vCPU `id` while it waits (`kvm_arch_vcpu_runnable`).
pub fn wake_pending(id: u8) -> bool {
    let i = id as usize;
    if i >= vgic::MAX_VCPUS { return false; }
    if POWER[i].load(Ordering::Acquire) == POWER_ON_PENDING { return true; }
    let dl = VTIMER_DEADLINE[i].load(Ordering::Acquire);
    if dl != 0 && crate::time::counter() >= dl { return true; }
    vgic::pending_for(id, &pmr_only(VCPU_PMR[i].load(Ordering::Acquire)))
}

fn pmr_only(pmr: u8) -> vgic::CpuIf {
    let mut c = vgic::CpuIf::new();
    c.set_pmr(pmr);
    c
}

pub struct Vcpu {
    id: u8,
    regs: Box<GuestRegs>,
    host: Box<HostSave>,
    sys: SysRegs,
    traps: Traps,
    host_ctl: HostCtl,
    cif: vgic::CpuIf,
    dev_poll: crate::microvm::cpu::DevPoll,
    iter: u64,
    exit_tsc: u64,
    halt_tsc: u64,
    /// Emulated debug state (`MDSCR_EL1`, the OS lock).
    mdscr: u64,
    os_lock: bool,
    /// The trapped physical timer: kept, never fires (the guest uses the
    /// virtual one).
    ptimer_ctl: u64,
    ptimer_cval: u64,
    undef_logged: u32,
}

pub struct VmContext {
    shared: Arc<VmShared>,
    vcpu: Vcpu,
}

fn new_vcpu(id: u8, vcpus: u8, root: u64, cntvoff: u64) -> Vcpu {
    let host_ctl = switch::host_ctl();
    let mut hcr = HCR_VM | HCR_SWIO | HCR_PTW | HCR_FMO | HCR_IMO | HCR_AMO | HCR_FB | HCR_BSU_IS
        | HCR_TWI | HCR_TID3 | HCR_TSC | HCR_TIDCP | HCR_TACR | HCR_TSW | HCR_RW | HCR_E2H;
    if RAS.load(Ordering::Relaxed) { hcr |= HCR_TERR | HCR_TEA; }
    if LOR.load(Ordering::Relaxed) { hcr |= HCR_TLOR; }
    let pmcr_n = (rd!("PMCR_EL0") >> 11) & MDCR_HPMN_MASK;
    let _ = vcpus;
    Vcpu {
        id,
        regs: Box::new(GuestRegs::new()),
        host: Box::new(HostSave::new()),
        sys: SysRegs { sctlr: SCTLR_RESET, ..SysRegs::default() },
        traps: Traps {
            hcr,
            vttbr: stage2::vttbr(root),
            vtcr: VTCR.load(Ordering::Relaxed),
            cnthctl: (host_ctl.cnthctl | CNTHCTL_EL1PCTEN) & !CNTHCTL_EL1PTEN,
            mdcr: pmcr_n | MDCR_TPMCR | MDCR_TPM | MDCR_TDA | MDCR_TDOSA | MDCR_TDRA,
            vpidr: rd!("MIDR_EL1"),
            vmpidr: platform::mpidr(id),
            cntvoff,
        },
        host_ctl,
        cif: vgic::CpuIf::new(),
        dev_poll: crate::microvm::cpu::DevPoll::new(),
        iter: 0,
        exit_tsc: 0,
        halt_tsc: 0,
        mdscr: 0,
        os_lock: true,
        ptimer_ctl: 0,
        ptimer_cval: 0,
        undef_logged: 0,
    }
}

/// The maintenance interrupt and the virtual timer's PPI need nothing from
/// the host: taking them is what ends the guest's run.
fn exit_only(_irq: u32) {}

fn enable_ppis_here() {
    crate::arch::aarch64::gic::enable(vgic::maintenance_irq(), exit_only);
    crate::arch::aarch64::gic::enable(platform::VTIMER_INTID, exit_only);
}

pub fn vm_open(image: &[u8], cmdline: &[u8], initramfs: Option<&[u8]>, inject: &[u8]) -> Result<VmContext, &'static str> {
    VmContext::open(image, cmdline, initramfs, inject)
}

pub fn vm_open_ap(id: u8) -> Result<VmContext, &'static str> {
    VmContext::open_ap(id)
}

impl VmContext {
    pub fn open(image: &[u8], cmdline: &[u8], initramfs: Option<&[u8]>, inject: &[u8]) -> Result<VmContext, &'static str> {
        if !READY.load(Ordering::Acquire) {
            return Err("Arm virtualization unavailable");
        }
        let vcpus = crate::microvm::cpu::guest_vcpus().min(vgic::MAX_VCPUS as u8);
        let guest_bytes = crate::microvm::cpu::choose_guest_ram_bytes();
        let layout = RamLayout::new(guest_bytes);
        crate::microvm::devices::hostmem::configure(layout.top());

        let frames = stage2::boot_frames_for(guest_bytes);
        let raw = memory::allocate_contiguous(frames).ok_or("OOM allocating the guest boot window")?;
        // The guest can read every byte of its window: nothing the host kept
        // there may stay (as `svm::alloc_guest_ram_and_npt`).
        // SAFETY: frames just allocated to this VM, identity-mapped, unshared.
        unsafe { core::ptr::write_bytes(raw as *mut u8, 0, frames * 4096) };
        let boot_base = stage2::round_up_to_2mb(raw);
        let root = match stage2::allocate_window(boot_base, guest_bytes) {
            Ok(r) => r,
            Err(e) => {
                memory::deallocate_contiguous(raw, frames);
                return Err(e);
            }
        };
        stage2::flush_vm(root);
        if let Err(e) = stage2::map_device(root, platform::CPU_BASE, vgic::gicv_base(), platform::CPU_SIZE) {
            stage2::release(root, guest_bytes);
            memory::deallocate_contiguous(raw, frames);
            return Err(e);
        }
        let boot_bytes = stage2::boot_window_bytes(guest_bytes);
        let gm = GuestMem::new(boot_base, boot_bytes, guest_bytes, root, SecondLevel::Stage2);

        let rd_len = initramfs.map_or(0, |r| r.len());
        let fail = |e: &'static str| {
            stage2::release(root, guest_bytes);
            memory::deallocate_contiguous(raw, frames);
            Err(e)
        };
        let placed = match crate::microvm::linux::arm64::place(image, rd_len, boot_bytes) {
            Ok(p) => p,
            Err(e) => return fail(e),
        };
        let cmd = core::str::from_utf8(cmdline).unwrap_or("console=ttyS0");
        let initrd = initramfs.map(|r| (placed.initrd, placed.initrd + r.len() as u64));
        let dtb = platform::build_dtb(layout, vcpus, cmd, initrd);
        let loaded = match crate::microvm::linux::arm64::load(&gm, &placed, image, &dtb, initramfs) {
            Ok(l) => l,
            Err(e) => return fail(e),
        };
        // The guest starts with its caches off: the window it reads first is
        // what is in memory, and no stale instruction is left for it.
        crate::mm::clflush_range(boot_base, boot_bytes);
        stage2::invalidate_icache();
        crate::kprintln!("[microvm] arm64 Image at {:#x}..{:#x}, device tree at {:#x} ({} bytes), {} vCPUs, {} MB RAM",
            placed.image, placed.image_end, loaded.dtb_gpa, dtb.len(), vcpus, guest_bytes >> 20);
        let gm = crate::microvm::devices::guest_mem::set_active(gm);

        vgic::reset(vcpus);
        crate::microvm::devices::net_backend::reset();
        crate::microvm::devices::gpu_backend::reset();
        for i in 0..vgic::MAX_VCPUS {
            POWER[i].store(POWER_OFF, Ordering::Release);
            VCPU_CORE[i].store(usize::MAX, Ordering::Release);
            VTIMER_DEADLINE[i].store(0, Ordering::Release);
        }
        POWER[0].store(POWER_ON, Ordering::Release);

        let mut u = uart::Uart::new();
        if !inject.is_empty() {
            u.inject(inject);
        }
        let cntvoff = crate::time::counter();
        let mut vcpu = new_vcpu(0, vcpus, root, cntvoff);
        vcpu.regs.pc = loaded.entry;
        vcpu.regs.pstate = PSTATE_RESET;
        vcpu.regs.x[0] = loaded.dtb_gpa;
        enable_ppis_here();
        Ok(VmContext {
            shared: Arc::new(VmShared {
                guest_mem: gm,
                root,
                guest_raw_base: raw,
                vcpus,
                cntvoff,
                dev: spin::Mutex::new(VmDevices {
                    uart: u,
                    pci: crate::microvm::devices::PciBus::new(),
                    last_reap_tick: 0,
                }),
            }),
            vcpu,
        })
    }

    /// A secondary vCPU joining the open VM after PSCI CPU_ON.
    pub fn open_ap(id: u8) -> Result<VmContext, &'static str> {
        let shared = AP_SHARED.lock().clone().ok_or("no open VM for an AP to join")?;
        if id == 0 || id >= shared.vcpus {
            return Err("no such vCPU");
        }
        let mut vcpu = new_vcpu(id, shared.vcpus, shared.root, shared.cntvoff);
        vcpu.regs.pstate = PSTATE_RESET;
        enable_ppis_here();
        Ok(VmContext { shared, vcpu })
    }

    pub fn publish_for_aps(&self) {
        *AP_SHARED.lock() = Some(self.shared.clone());
    }

    pub fn yield_runnable(&mut self) {
        crate::smp::fiber::yield_ready();
    }

    /// The vCPU's next timer event in host counter units, and on vCPU 0 the
    /// devices' (`svm::VmContext::next_timer_deadline_tsc`).
    pub fn next_timer_deadline_tsc(&self) -> Option<u64> {
        let t = vtimer_deadline(&self.vcpu.sys, self.shared.cntvoff);
        let d = if self.vcpu.id == 0 { self.device_deadline() } else { None };
        match (t, d) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn device_deadline(&self) -> Option<u64> {
        let sh = self.shared.dev.try_lock()?;
        let mut dev = crate::microvm::devices::gpu_backend::resume_tsc();
        if sh.pci.virtio_snd.playing() {
            let t = crate::time::counter() + crate::time::counter_hz() / 1000;
            dev = Some(dev.map_or(t, |d| d.min(t)));
        }
        dev
    }

    pub fn close(&mut self) {
        let workers_gone = crate::microvm::devices::net_dataplane::stop_worker();
        {
            let mut dev = self.shared.dev.lock();
            dev.uart.flush();
            dev.pci.virtio_blk.save();
            dev.pci.virtio_blk_sqfs.save();
            let (reports, pages) = dev.pci.virtio_balloon.stats();
            crate::kprintln!("[virtio-balloon] guest gave back {} MB in {} reports",
                pages * 4096 / (1024 * 1024), reports);
        }
        crate::arch::aarch64::gic::mask(platform::VTIMER_INTID);
        if !workers_gone {
            crate::kprintln!("[microvm] a device worker did not stop — guest memory is kept, not freed");
            return;
        }
        let len = self.shared.guest_mem.len();
        stage2::release(self.shared.root, len);
        memory::deallocate_contiguous(self.shared.guest_raw_base, stage2::boot_frames_for(len));
        crate::microvm::devices::guest_mem::clear_active();
    }

    /// One slice of the guest; host interrupts are masked again on return.
    pub fn run_slice(&mut self, budget: u32) -> Result<SliceOutcome, &'static str> {
        let r = self.run_slice_inner(budget);
        crate::arch::irq_disable();
        r
    }

    fn run_slice_inner(&mut self, budget: u32) -> Result<SliceOutcome, &'static str> {
        const SLICE_MS: u64 = 3;
        let host_core = crate::smp::per_core::current_core_id();
        let id = self.vcpu.id;
        VCPU_CORE[id as usize].store(host_core, Ordering::Release);
        let slice_deadline = crate::microvm::cpu::slice_deadline_with_peers(host_core,
            crate::time::counter() + (crate::time::counter_hz() / 1000) * SLICE_MS);
        let mut n = 0u32;
        loop {
            if n >= budget || crate::time::counter() >= slice_deadline
                || crate::smp::per_core::take_peer_wake(host_core)
            {
                return Ok(SliceOutcome::StillRunning);
            }
            n += 1;
            self.vcpu.iter += 1;

            // PSCI power state: an AP waits for CPU_ON.
            match POWER[id as usize].load(Ordering::Acquire) {
                POWER_ON => {}
                POWER_ON_PENDING => self.power_on(),
                _ => {
                    VTIMER_DEADLINE[id as usize].store(0, Ordering::Release);
                    return Ok(SliceOutcome::Idle);
                }
            }

            let kick_gen = crate::smp::fiber::net_kick_gen(host_core);
            if id == 0 && self.vcpu.dev_poll.due(0) {
                let snap = crate::microvm::cpu::DevPoll::snapshot();
                {
                    let mut dev = self.shared.dev.lock();
                    collect_device_irqs(&mut dev, self.shared.guest_mem, id);
                }
                self.vcpu.dev_poll.done(snap, false);
            }
            self.update_vtimer();

            // The virtual timer interrupts the guest itself (its PPI); the
            // host one-shot only ends the slice or serves a device deadline.
            let entry_deadline = match (id, self.device_deadline()) {
                (0, Some(d)) => d.min(slice_deadline),
                _ => slice_deadline,
            };
            if !crate::microvm::cpu::entry_irqs_off(kick_gen, host_core, entry_deadline) {
                continue;
            }
            let _ = crate::microvm::devices::hostmem::flush_before_entry(host_core);
            vgic::flush(id, &mut self.vcpu.cif);
            let t_entry = crate::time::counter();
            if self.vcpu.exit_tsc != 0 {
                crate::microvm::cpu::record_vcpu_time(id, crate::microvm::cpu::VT_OUTSIDE,
                    t_entry.wrapping_sub(self.vcpu.exit_tsc));
            }
            if self.vcpu.halt_tsc != 0 {
                crate::microvm::cpu::record_vcpu_time(id, crate::microvm::cpu::VT_HALTED,
                    t_entry.wrapping_sub(self.vcpu.halt_tsc));
                self.vcpu.halt_tsc = 0;
            }
            let v = &mut self.vcpu;
            let (kind, fault) = switch::run(&mut v.regs, &mut v.host, &mut v.sys, &v.traps, &v.host_ctl,
                RAS.load(Ordering::Relaxed));
            vgic::fold(id, &mut v.cif);
            VCPU_PMR[id as usize].store(v.cif.pmr_value(), Ordering::Release);
            crate::microvm::cpu::note_guest_exit(host_core);
            crate::microvm::cpu::entry_irqs_on();
            let t_exit = crate::time::counter();
            crate::microvm::cpu::record_vcpu_time(id, crate::microvm::cpu::VT_GUEST, t_exit.wrapping_sub(t_entry));
            self.vcpu.exit_tsc = t_exit;

            if fault.disr != 0 || kind == switch::EXIT_SERROR {
                self.shared.dev.lock().uart.flush();
                crate::kprintln!("[microvm] SError from the guest (ESR {:#x}, DISR {:#x}) at pc {:#x}",
                    fault.esr, fault.disr, self.vcpu.regs.pc);
                return Ok(SliceOutcome::Exited(LaunchOutcome { exit_reason: EC_SERROR, exit_qualification: fault.esr }));
            }
            if kind != switch::EXIT_SYNC {
                crate::microvm::cpu::record_vm_exit(id, crate::microvm::cpu::VMX_INTR);
                continue;
            }
            match self.handle_sync(fault)? {
                Step::Continue => {}
                Step::Idle => {
                    self.vcpu.halt_tsc = crate::time::counter();
                    return Ok(SliceOutcome::Idle);
                }
                Step::Exit(o) => return Ok(SliceOutcome::Exited(o)),
            }
        }
    }

    /// CPU_ON reached this vCPU: start at its entry with x0 = context id,
    /// everything else at reset (`kvm_reset_vcpu`).
    fn power_on(&mut self) {
        let i = self.vcpu.id as usize;
        let regs = &mut self.vcpu.regs;
        **regs = GuestRegs::new();
        regs.pc = BOOT_ENTRY[i].load(Ordering::Acquire);
        regs.x[0] = BOOT_CTX[i].load(Ordering::Acquire);
        regs.pstate = PSTATE_RESET;
        self.vcpu.sys = SysRegs { sctlr: SCTLR_RESET, ..SysRegs::default() };
        POWER[i].store(POWER_ON, Ordering::Release);
    }

    /// The virtual timer's line, sampled before every entry
    /// (`kvm_timer_update_irq` without a hardware-mapped interrupt). While
    /// it is high the host masks the timer's PPI on this core, so the guest
    /// takes the virtual interrupt instead of exiting on the physical one;
    /// the EOI maintenance interrupt brings it back for the next sample.
    fn update_vtimer(&mut self) {
        let id = self.vcpu.id;
        let level = vtimer_level(&self.vcpu.sys, self.shared.cntvoff);
        vgic::set_ppi_level(id, platform::VTIMER_INTID, level);
        if level {
            crate::arch::aarch64::gic::mask(platform::VTIMER_INTID);
        } else {
            crate::arch::aarch64::gic::unmask(platform::VTIMER_INTID);
        }
        VTIMER_DEADLINE[id as usize].store(vtimer_deadline(&self.vcpu.sys, self.shared.cntvoff).unwrap_or(0),
            Ordering::Release);
    }

    fn handle_sync(&mut self, f: switch::Fault) -> Result<Step, &'static str> {
        let ec = (f.esr >> 26) & 0x3f;
        let id = self.vcpu.id;
        match ec {
            EC_WFX => {
                crate::microvm::cpu::record_vm_exit(id, crate::microvm::cpu::VMX_HLT);
                self.vcpu.regs.pc += 4;
                // WFE (ISS bit 0) is not trapped; a WFI blocks.
                self.update_vtimer();
                if f.esr & 1 != 0 || vgic::pending_for(id, &self.vcpu.cif)
                    || vtimer_level(&self.vcpu.sys, self.shared.cntvoff)
                {
                    return Ok(Step::Continue);
                }
                Ok(Step::Idle)
            }
            EC_HVC64 => {
                crate::microvm::cpu::record_vm_exit(id, crate::microvm::cpu::VMX_OTHER);
                Ok(self.hvc())
            }
            EC_SMC64 => {
                // `handle_smc`: not implemented, x0 = -1, past the SMC.
                self.vcpu.regs.x[0] = PSCI_RET_NOT_SUPPORTED;
                self.vcpu.regs.pc += 4;
                Ok(Step::Continue)
            }
            EC_SYS64 => {
                crate::microvm::cpu::record_vm_exit(id, crate::microvm::cpu::VMX_MSR);
                self.sysreg(f.esr);
                Ok(Step::Continue)
            }
            EC_IABT_LOW | EC_DABT_LOW => {
                crate::microvm::cpu::record_vm_exit(id, crate::microvm::cpu::VMX_MMIO);
                self.abort(ec, f)
            }
            _ => {
                self.shared.dev.lock().uart.flush();
                crate::kprintln!("[microvm] unhandled guest exception: ESR {:#x} (EC {:#x}) FAR {:#x} at pc {:#x}",
                    f.esr, ec, f.far, self.vcpu.regs.pc);
                Ok(Step::Exit(LaunchOutcome { exit_reason: ec, exit_qualification: f.esr }))
            }
        }
    }

    /// PSCI and SMCCC (`kvm_psci_call`, `kvm_smccc_call_handler`).
    fn hvc(&mut self) -> Step {
        let fid = self.vcpu.regs.x[0] as u32;
        let a1 = self.vcpu.regs.x[1];
        let a2 = self.vcpu.regs.x[2];
        let a3 = self.vcpu.regs.x[3];
        let base = fid & !PSCI_64;
        let ret = match (fid, base) {
            (PSCI_VERSION, _) => 0x1_0000,
            (SMCCC_VERSION, _) => 0x1_0001,
            (SMCCC_ARCH_FEATURES, _) => PSCI_RET_NOT_SUPPORTED,
            (PSCI_FEATURES, _) => {
                let q = a1 as u32;
                match q & !PSCI_64 {
                    PSCI_VERSION | PSCI_CPU_SUSPEND | PSCI_CPU_OFF | PSCI_CPU_ON | PSCI_AFFINITY_INFO
                    | PSCI_MIGRATE_INFO_TYPE | PSCI_SYSTEM_OFF | PSCI_SYSTEM_RESET | PSCI_FEATURES => 0,
                    _ if q == SMCCC_VERSION => 0,
                    _ => PSCI_RET_NOT_SUPPORTED,
                }
            }
            (_, PSCI_CPU_SUSPEND) => {
                // A standby state: as WFI (`kvm_psci_vcpu_suspend`).
                self.vcpu.regs.x[0] = PSCI_RET_SUCCESS;
                if vgic::pending_for(self.vcpu.id, &self.vcpu.cif) {
                    return Step::Continue;
                }
                return Step::Idle;
            }
            (PSCI_CPU_OFF, _) => {
                POWER[self.vcpu.id as usize].store(POWER_OFF, Ordering::Release);
                return Step::Idle;
            }
            (_, PSCI_CPU_ON) => self.cpu_on(a1, a2, a3),
            (_, PSCI_AFFINITY_INFO) => {
                let t = (a1 & 0xff) as usize;
                if a1 & !0xff != 0 || t >= self.shared.vcpus as usize || a2 != 0 {
                    PSCI_RET_INVALID_PARAMS
                } else if POWER[t].load(Ordering::Acquire) == POWER_OFF {
                    1
                } else {
                    0
                }
            }
            (PSCI_MIGRATE_INFO_TYPE, _) => PSCI_TOS_NOT_PRESENT_MP,
            (PSCI_SYSTEM_OFF, _) | (PSCI_SYSTEM_RESET, _) => {
                self.shared.dev.lock().uart.flush();
                let off = fid == PSCI_SYSTEM_OFF;
                crate::kprintln!("[microvm] guest called PSCI SYSTEM_{} after {} exits",
                    if off { "OFF" } else { "RESET" }, self.vcpu.iter);
                crate::microvm::cpu::note_guest_shutdown();
                return Step::Exit(LaunchOutcome {
                    exit_reason: if off { REASON_SYSTEM_OFF } else { REASON_SYSTEM_RESET },
                    exit_qualification: fid as u64,
                });
            }
            _ => PSCI_RET_NOT_SUPPORTED,
        };
        self.vcpu.regs.x[0] = ret;
        Step::Continue
    }

    /// `kvm_psci_vcpu_on`.
    fn cpu_on(&mut self, target: u64, entry: u64, ctx: u64) -> u64 {
        let t = (target & 0xff) as usize;
        if target & !0xff != 0 || t >= self.shared.vcpus as usize {
            return PSCI_RET_INVALID_PARAMS;
        }
        if POWER[t].load(Ordering::Acquire) != POWER_OFF {
            return PSCI_RET_ALREADY_ON;
        }
        BOOT_ENTRY[t].store(entry, Ordering::Release);
        BOOT_CTX[t].store(ctx, Ordering::Release);
        POWER[t].store(POWER_ON_PENDING, Ordering::Release);
        let c = VCPU_CORE[t].load(Ordering::Acquire);
        if c == usize::MAX {
            crate::microvm::cpu::request_ap_spawn(t as u8, 0);
        } else {
            crate::smp::kick_host_core(c);
        }
        PSCI_RET_SUCCESS
    }

    /// A stage-2 abort (`kvm_handle_guest_abort`).
    fn abort(&mut self, ec: u64, f: switch::Fault) -> Result<Step, &'static str> {
        let iss = f.esr & 0x1ff_ffff;
        // The guest changed its stage 1 under the fault: let it retry
        // (`kvm_handle_guest_abort` is never reached for it).
        let Some(hpfar) = f.hpfar else { return Ok(Step::Continue) };
        let ipa = ((hpfar >> 4) << 12) | (f.far & 0xfff);
        let fsc = iss & 0x3f;
        let gm = self.shared.guest_mem;
        let exec = ec == EC_IABT_LOW;
        if gm.layout().contains(ipa) {
            // Permission fault on an instruction fetch: the page is mapped
            // XN until the first fetch; translation faults bring it in.
            if exec && fsc & 0x3c == 0x0c {
                if gm.exec_fault(ipa) {
                    return Ok(Step::Continue);
                }
            } else if gm.ensure(ipa) {
                crate::microvm::cpu::record_npf(crate::microvm::cpu::NPF_RAM);
                if exec {
                    let _ = gm.exec_fault(ipa);
                }
                return Ok(Step::Continue);
            }
            self.shared.dev.lock().uart.flush();
            crate::kprintln!("[microvm] guest RAM fault not served: IPA {:#x} ESR {:#x} at pc {:#x}",
                ipa, f.esr, self.vcpu.regs.pc);
            return Ok(Step::Exit(LaunchOutcome { exit_reason: ec, exit_qualification: f.esr }));
        }
        if exec {
            self.shared.dev.lock().uart.flush();
            crate::kprintln!("[microvm] guest executes outside RAM: IPA {:#x} at pc {:#x}", ipa, self.vcpu.regs.pc);
            return Ok(Step::Exit(LaunchOutcome { exit_reason: ec, exit_qualification: f.esr }));
        }
        // MMIO: the syndrome describes the access (`kvm_decode_mmio`).
        if iss & (1 << 24) == 0 {
            self.shared.dev.lock().uart.flush();
            crate::kprintln!("[microvm] MMIO at IPA {:#x} without a valid syndrome (ESR {:#x}) at pc {:#x}",
                ipa, f.esr, self.vcpu.regs.pc);
            return Ok(Step::Exit(LaunchOutcome { exit_reason: ec, exit_qualification: f.esr }));
        }
        let len = 1u8 << ((iss >> 22) & 3);
        let sse = iss & (1 << 21) != 0;
        let srt = ((iss >> 16) & 0x1f) as usize;
        let sf = iss & (1 << 15) != 0;
        let write = iss & (1 << 6) != 0;
        let mask = if len == 8 { u64::MAX } else { (1u64 << (len as u32 * 8)) - 1 };
        let wval = self.vcpu.regs.get(srt) & mask;
        let Some(rval) = self.mmio(ipa, len, write, wval) else {
            self.shared.dev.lock().uart.flush();
            crate::kprintln!("[microvm] MMIO {} of {} bytes at IPA {:#x} hits no device (pc {:#x})",
                if write { "write" } else { "read" }, len, ipa, self.vcpu.regs.pc);
            return Ok(Step::Exit(LaunchOutcome { exit_reason: ec, exit_qualification: f.esr }));
        };
        if !write {
            let mut v = rval & mask;
            if sse && len < 8 {
                let bits = len as u32 * 8;
                v = (((v << (64 - bits)) as i64) >> (64 - bits)) as u64;
            }
            if !sf {
                v &= 0xffff_ffff;
            }
            self.vcpu.regs.set(srt, v);
        }
        self.vcpu.regs.pc += 4;
        Ok(Step::Continue)
    }

    /// One MMIO access, routed by address. None if nothing is there.
    fn mmio(&mut self, ipa: u64, len: u8, write: bool, val: u64) -> Option<u64> {
        use crate::microvm::devices::{gpu_backend, hostmem, net_backend, virtio_gpu_pci, virtio_net_dev, MmioDevice};
        let id = self.vcpu.id;
        let gm = self.shared.guest_mem;
        if (platform::DIST_BASE..platform::DIST_BASE + platform::DIST_SIZE).contains(&ipa) {
            let off = (ipa - platform::DIST_BASE) as u32;
            if write {
                let kick = vgic::dist_write(id, off, len, val);
                kick_mask(kick, id);
                return Some(0);
            }
            return Some(vgic::dist_read(id, off, len));
        }
        if id == 0 {
            self.vcpu.dev_poll.dirty = true;
        } else {
            crate::microvm::cpu::note_device_work();
        }
        // virtio-net and virtio-gpu sit outside the device lock (their
        // workers own them); their fast paths are lock-free.
        if net_backend::bar0_in_range(ipa) {
            let off = (ipa - virtio_net_dev::BAR0_BASE) as u32;
            if let Some(v) = net_backend::mmio_fast(off, write) {
                return Some(v);
            }
            let mut net = net_backend::lock();
            let r = if write { net.mmio_write(off, len, val); 0 } else { net.mmio_read(off, len) };
            if let Some(q) = net.take_pending_kick() {
                if net_backend::full_active() && q == 1 {
                    net_backend::note_tx_kick();
                } else if net.service_queues(q, gm) {
                    drop(net);
                    raise_line(virtio_net_dev::IRQ_LINE, id);
                }
            }
            return Some(r);
        }
        if gpu_backend::bar0_in_range(ipa) {
            let off = (ipa - virtio_gpu_pci::BAR0_BASE) as u32;
            if let Some(v) = gpu_backend::mmio_fast(off, write.then_some(val as u32)) {
                return Some(v);
            }
            let mut gpu = gpu_backend::lock();
            let r = if write { gpu.mmio_write(off, len, val); 0 } else { gpu.mmio_read(off, len) };
            if let Some(q) = gpu.take_pending_kick() {
                if gpu_backend::full_active() {
                    gpu_backend::note_gpu_kick(q);
                } else {
                    let mut advanced = gpu.service_queues(q, gm);
                    if q == 0 {
                        advanced |= gpu_backend::after_inline_service(&mut gpu, gm);
                    }
                    drop(gpu);
                    if advanced && !gpu_backend::notify_queue(q) {
                        raise_line(virtio_gpu_pci::IRQ_LINE, id);
                    }
                }
            }
            return Some(r);
        }
        let mut dev = self.shared.dev.lock();
        let sh = &mut *dev;
        if (platform::UART_BASE..platform::UART_BASE + platform::UART_SIZE).contains(&ipa) {
            let off = ipa - platform::UART_BASE;
            if write {
                sh.uart.write(off, val as u8);
                return Some(0);
            }
            return Some(sh.uart.read(off) as u64);
        }
        if (platform::ECAM_BASE..platform::ECAM_BASE + platform::ECAM_SIZE).contains(&ipa) {
            let off = ipa - platform::ECAM_BASE;
            return Some(crate::microvm::devices::pci_bus::ecam_access(&mut sh.pci, off, write, len, val as u32) as u64);
        }
        fn dev_access<D: MmioDevice>(d: &mut D, ipa: u64, len: u8, write: bool, val: u64, gm: &GuestMem) -> (u64, Option<u8>) {
            let off = (ipa - d.bar0_base()) as u32;
            let r = if write { d.mmio_write(off, len, val); 0 } else { d.mmio_read(off, len) };
            let mut line = None;
            if let Some(q) = d.take_pending_kick() {
                if d.service_queues(q, gm) {
                    line = Some(d.irq_line());
                }
            }
            (r, line)
        }
        let p = &mut sh.pci;
        let hit = if p.virtio_blk.bar0_in_range(ipa) {
            Some(dev_access(&mut p.virtio_blk, ipa, len, write, val, gm))
        } else if p.virtio_blk_sqfs.bar0_in_range(ipa) {
            Some(dev_access(&mut p.virtio_blk_sqfs, ipa, len, write, val, gm))
        } else if p.virtio_input.bar0_in_range(ipa) {
            Some(dev_access(&mut p.virtio_input, ipa, len, write, val, gm))
        } else if p.virtio_9p.bar0_in_range(ipa) {
            Some(dev_access(&mut p.virtio_9p, ipa, len, write, val, gm))
        } else if p.virtio_snd.bar0_in_range(ipa) {
            Some(dev_access(&mut p.virtio_snd, ipa, len, write, val, gm))
        } else if p.virtio_console.bar0_in_range(ipa) {
            Some(dev_access(&mut p.virtio_console, ipa, len, write, val, gm))
        } else if p.virtio_balloon.bar0_in_range(ipa) {
            Some(dev_access(&mut p.virtio_balloon, ipa, len, write, val, gm))
        } else if hostmem::contains(ipa) && !hostmem::is_mapped(ipa) {
            Some(dev_access(&mut hostmem::Hole, ipa, len, write, val, gm))
        } else if hostmem::contains(ipa) {
            // Mapped meanwhile: the access is retried by the guest.
            Some((0, None))
        } else if (platform::PCI_MMIO_BASE..platform::PCI_MMIO_BASE + platform::PCI_MMIO_SIZE).contains(&ipa) {
            // Unassigned PCI memory reads all-ones (a master abort).
            Some((u64::MAX, None))
        } else {
            None
        };
        drop(dev);
        let (r, line) = hit?;
        if let Some(l) = line {
            raise_line(l, id);
        }
        Some(r)
    }

    /// A trapped MRS/MSR (`kvm_handle_sys_reg`); the PC moves past it
    /// unless an UNDEF was injected.
    fn sysreg(&mut self, esr: u64) {
        let iss = esr & 0x1ff_ffff;
        let read = iss & 1 != 0;
        let crm = (iss >> 1) & 0xf;
        let rt = ((iss >> 5) & 0x1f) as usize;
        let crn = (iss >> 10) & 0xf;
        let op1 = (iss >> 14) & 0x7;
        let op2 = (iss >> 17) & 0x7;
        let op0 = (iss >> 20) & 0x3;
        let wval = self.vcpu.regs.get(rt);
        let r: Option<u64> = match (op0, op1, crn, crm, op2) {
            // The ID registers (TID3), sanitized.
            (3, 0, 0, 1..=7, _) => {
                if !read { None } else { Some(sanitized_id(crm, op2)) }
            }
            // ACTLR_EL1: the host's value, writes ignored (`access_actlr`).
            (3, 0, 1, 0, 1) => Some(if read { HOST_ACTLR.load(Ordering::Relaxed) } else { 0 }),
            // MDSCR_EL1.
            (2, 0, 0, 2, 2) => {
                if !read { self.vcpu.mdscr = wval; }
                Some(self.vcpu.mdscr)
            }
            // OSLAR_EL1 / OSLSR_EL1 (`trap_oslar_el1`, `trap_oslsr_el1`).
            (2, 0, 1, 0, 4) => {
                if !read { self.vcpu.os_lock = wval & 1 != 0; }
                Some(0)
            }
            (2, 0, 1, 1, 4) => Some(if read { 0x8 | (self.vcpu.os_lock as u64) << 1 } else { 0 }),
            // The rest of the debug registers: RAZ/WI (`trap_raz_wi`).
            (2, _, _, _, _) => Some(0),
            // The PMU (PMUVer 0 in the ID registers): RAZ/WI.
            (3, 0, 9, 12..=14, _) | (3, 3, 9, 12..=14, _) | (3, 3, 14, 8..=15, _) => Some(0),
            // RAS error records: RAZ/WI (`trap_raz_wi`).
            (3, 0, 5, 3..=5, _) => Some(0),
            // The EL1 physical timer, trapped: kept, never fires.
            (3, 3, 14, 2, 0) => {
                let now = crate::time::counter();
                if !read { self.vcpu.ptimer_cval = now.wrapping_add(wval as i32 as i64 as u64); }
                Some(self.vcpu.ptimer_cval.wrapping_sub(now) & 0xffff_ffff)
            }
            (3, 3, 14, 2, 1) => {
                if !read { self.vcpu.ptimer_ctl = wval & 3; }
                Some(self.vcpu.ptimer_ctl)
            }
            (3, 3, 14, 2, 2) => {
                if !read { self.vcpu.ptimer_cval = wval; }
                Some(self.vcpu.ptimer_cval)
            }
            // DC ISW/CSW/CISW (TSW): with stage 2 on, set/way cleaning
            // reaches only this core's caches; the guest memory is coherent
            // for every other access, so there is nothing to emulate.
            (1, 0, 7, 6 | 10 | 14, 2) => Some(0),
            _ => None,
        };
        match r {
            Some(v) => {
                if read { self.vcpu.regs.set(rt, v); }
                self.vcpu.regs.pc += 4;
            }
            None => {
                if self.vcpu.undef_logged < 8 {
                    self.vcpu.undef_logged += 1;
                    crate::kprintln!("[microvm] guest {} of sysreg op0={} op1={} CRn={} CRm={} op2={} → UNDEF",
                        if read { "read" } else { "write" }, op0, op1, crn, crm, op2);
                }
                self.inject_undef();
            }
        }
    }

    /// `inject_undef64` → `enter_exception64` (EL1h, synchronous).
    fn inject_undef(&mut self) {
        let regs = &mut self.vcpu.regs;
        let s = &mut self.vcpu.sys;
        let old = regs.pstate;
        let offset = match old & 0x1f {
            0b00101 => 0x200,
            0b00100 => 0x000,
            m if m & 0x10 != 0 => 0x600,
            _ => 0x400,
        };
        s.esr = 1 << 25;
        s.elr = regs.pc;
        s.spsr = old;
        regs.pc = s.vbar + offset;
        const PSR_PAN: u64 = 1 << 22;
        const PSR_SSBS: u64 = 1 << 12;
        const PSR_DIT: u64 = 1 << 24;
        const SCTLR_SPAN: u64 = 1 << 23;
        const SCTLR_DSSBS: u64 = 1 << 44;
        let mut new = 0x3c5 | (old & PSR_DIT) | (old & PSR_PAN);
        if s.sctlr & SCTLR_SPAN == 0 { new |= PSR_PAN; }
        if s.sctlr & SCTLR_DSSBS != 0 { new |= PSR_SSBS; }
        regs.pstate = new;
    }
}

enum Step {
    Continue,
    Idle,
    Exit(LaunchOutcome),
}

/// `kvm_timer_should_fire` for the virtual timer.
fn vtimer_level(s: &SysRegs, cntvoff: u64) -> bool {
    const ENABLE: u64 = 1;
    const IMASK: u64 = 2;
    s.cntv_ctl & ENABLE != 0 && s.cntv_ctl & IMASK == 0
        && s.cntv_cval <= crate::time::counter().wrapping_sub(cntvoff)
}

/// The host counter value at which the virtual timer fires, if armed and
/// not fired yet.
fn vtimer_deadline(s: &SysRegs, cntvoff: u64) -> Option<u64> {
    (s.cntv_ctl & 1 != 0 && s.cntv_ctl & 2 == 0 && !vtimer_level(s, cntvoff))
        .then(|| s.cntv_cval.wrapping_add(cntvoff))
}

/// Feed the device lines into the distributor (vCPU 0, `dev` held), as
/// `svm::collect_device_irqs` feeds the PIC.
fn collect_device_irqs(sh: &mut VmDevices, gm: &GuestMem, me: u8) {
    use crate::microvm::devices::*;
    if net_backend::take_irq() {
        raise_line(virtio_net_dev::IRQ_LINE, me);
        nat::note_net_irq();
    }
    if gpu_backend::take_irq() {
        raise_line(virtio_gpu_pci::IRQ_LINE, me);
    }
    if gpu_backend::vblank(crate::time::counter(), gm) {
        raise_line(virtio_gpu_pci::IRQ_LINE, me);
    }
    if sh.pci.virtio_snd.pump(gm) {
        raise_line(sh.pci.virtio_snd.irq_line(), me);
    }
    if sh.pci.virtio_9p.drain_async_done(gm) {
        raise_line(sh.pci.virtio_9p.irq_line(), me);
    }
    if sh.pci.virtio_input.drain_injected(gm) {
        raise_line(virtio_input_pci::IRQ_LINE, me);
    }
    if sh.pci.virtio_console.pump(gm) {
        raise_line(virtio_console_pci::IRQ_LINE, me);
    }
    if let Some(r) = virtio_console_pci::take_disk_report() {
        if sh.pci.virtio_blk.grow_home(&r) {
            raise_line(virtio_blk_pci::HOME_IRQ_LINE, me);
        }
    }
    if gpu_backend::display_tick_intx() {
        raise_line(virtio_gpu_pci::IRQ_LINE, me);
    }
    let now = crate::time::ticks();
    if now != sh.last_reap_tick {
        sh.last_reap_tick = now;
        nat::housekeep();
    }
}

/// Read ID register (3, 0, 0, crm, op2) of this core.
fn host_id(crm: u64, op2: u64) -> u64 {
    macro_rules! table {
        ($($m:literal $o:literal)*) => {
            match (crm, op2) {
                $(($m, $o) => {
                    let v: u64;
                    // SAFETY: reads an ID register, no side effect.
                    unsafe { core::arch::asm!(concat!("mrs {}, S3_0_C0_C", $m, "_", $o), out(reg) v,
                        options(nomem, nostack, preserves_flags)) };
                    v
                })*
                _ => 0,
            }
        };
    }
    table!(
        1 0 1 1 1 2 1 3 1 4 1 5 1 6 1 7
        2 0 2 1 2 2 2 3 2 4 2 5 2 6 2 7
        3 0 3 1 3 2 3 3 3 4 3 5 3 6 3 7
        4 0 4 1 4 2 4 3 4 4 4 5 4 6 4 7
        5 0 5 1 5 2 5 3 5 4 5 5 5 6 5 7
        6 0 6 1 6 2 6 3 6 4 6 5 6 6 6 7
        7 0 7 1 7 2 7 3 7 4 7 5 7 6 7 7
    )
}

/// `read_sanitised_id_reg`: the host's value without what the guest is not
/// given — no EL2 (no nested virtualization), no SVE, SME, MTE, AMU, MPAM,
/// no GIC system registers (the GIC is a GICv2), no PMU, statistical
/// profiling or trace, no VHE and no LORegions.
fn sanitized_id(crm: u64, op2: u64) -> u64 {
    let v = host_id(crm, op2);
    let clear = |v: u64, shift: u32| v & !(0xf << shift);
    match (crm, op2) {
        // ID_DFR0_EL1: PerfMon.
        (1, 2) => clear(v, 24),
        // ID_AA64PFR0_EL1: EL2, GIC, SVE, MPAM, AMU, SEL2.
        (4, 0) => {
            let mut v = clear(v, 8);
            for s in [24, 32, 36, 40, 44] { v = clear(v, s); }
            v
        }
        // ID_AA64PFR1_EL1: MTE, SME, MPAM_frac.
        (4, 1) => clear(clear(clear(v, 8), 24), 16),
        // ID_AA64ZFR0_EL1, ID_AA64SMFR0_EL1.
        (4, 4) | (4, 5) => 0,
        // ID_AA64DFR0_EL1: TraceVer, PMUVer, PMSVer, TraceFilt; DebugVer 8.0.
        (5, 0) => {
            let mut v = v;
            for s in [4, 8, 32, 40] { v = clear(v, s); }
            (v & !0xf) | 6
        }
        // ID_AA64MMFR0_EL1: PARange at most the 40-bit IPA.
        (7, 0) => (v & !0xf) | (v & 0xf).min(2),
        // ID_AA64MMFR1_EL1: VH, LO.
        (7, 1) => clear(clear(v, 8), 16),
        _ => v,
    }
}
