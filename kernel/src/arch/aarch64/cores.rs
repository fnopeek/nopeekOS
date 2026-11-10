//! Starting the other cores through PSCI CPU_ON (Linux
//! `arch/arm64/kernel/smp.c`).
//!
//! The device tree lists the cores in `/cpus` with their MPIDR in `reg`;
//! as Linux's `smp_init_cpus`, the boot core is logical 0 and the others
//! follow in tree order.

use crate::platform::dt::Fdt;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::psci::{PSCI_0_2_FN64_CPU_ON, PSCI_RET_ALREADY_ON, PSCI_RET_SUCCESS};

pub const MAX_CORES: usize = 16;
/// 2 MiB, as an x86 AP's: native intents run on this stack.
const STACK_PAGES: usize = 512;

/// What a starting core needs before its MMU is on: stack, translation
/// regime and SCTLR, read by `secondary_entry` in boot.s at these offsets.
#[repr(C, align(64))]
struct SecondaryBoot {
    sp: u64,
    mair: u64,
    tcr: u64,
    ttbr: u64,
    sctlr: u64,
}

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
static mut secondary_boot: SecondaryBoot = SecondaryBoot { sp: 0, mair: 0, tcr: 0, ttbr: 0, sctlr: 0 };

unsafe extern "C" {
    fn secondary_entry();
}

static ONLINE: AtomicUsize = AtomicUsize::new(1);
/// Where a started core goes once it can take interrupts, a
/// `fn(usize) -> !` as usize, entered with them masked.
static ENTRY: AtomicUsize = AtomicUsize::new(0);

/// Cores running, the boot core included.
pub fn online() -> usize {
    ONLINE.load(Ordering::Acquire)
}

/// MPIDRs of the cores in logical order: the boot core first.
fn cores(fdt: &Fdt, out: &mut [u64; MAX_CORES]) -> usize {
    let me = super::cpu::mpidr();
    out[0] = me;
    let mut n = 1;
    let Some(cpus) = fdt.find_path(b"/cpus") else { return n };
    fdt.walk(|node| {
        if node.ancestors().last() == Some(&cpus.offset) && fdt.prop_has_str(node, b"device_type", b"cpu") {
            if let Some((o, len)) = fdt.prop(node, b"reg") {
                let id = if len >= 8 {
                    (fdt.cell_u32(o) as u64) << 32 | fdt.cell_u32(o + 4) as u64
                } else {
                    fdt.cell_u32(o) as u64
                };
                if id != me && n < MAX_CORES {
                    out[n] = id;
                    n += 1;
                }
            }
        }
        true
    });
    n
}

fn sysreg_state() -> (u64, u64, u64, u64) {
    let (mair, tcr, ttbr, sctlr): (u64, u64, u64, u64);
    // SAFETY: reads the running core's translation registers, no effect.
    unsafe {
        core::arch::asm!("mrs {}, MAIR_EL1", out(reg) mair, options(nomem, nostack));
        core::arch::asm!("mrs {}, TCR_EL1", out(reg) tcr, options(nomem, nostack));
        core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr, options(nomem, nostack));
        core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr, options(nomem, nostack));
    }
    (mair, tcr, ttbr, sctlr)
}

/// Start every core the device tree lists, each into `entry` with its
/// logical index (1, 2, ... in the order they came up); returns how many
/// run, the boot core included.
pub fn start(fdt: &Fdt, entry: fn(usize) -> !) -> usize {
    super::cpu::set_core_index(0);
    ENTRY.store(entry as usize, Ordering::Release);
    if !super::psci::init(fdt) { return online() };
    let mut ids = [0u64; MAX_CORES];
    let n = cores(fdt, &mut ids);
    let (mair, tcr, ttbr, sctlr) = sysreg_state();
    for &id in ids.iter().take(n).skip(1) {
        let Some(stack) = crate::memory::allocate_contiguous(STACK_PAGES) else { break };
        // SAFETY: only the boot core writes, and only while no core it
        // started is still reading (each one is waited for below).
        unsafe {
            let b = &mut *core::ptr::addr_of_mut!(secondary_boot);
            b.sp = stack + (STACK_PAGES as u64) * 4096;
            b.mair = mair;
            b.tcr = tcr;
            b.ttbr = ttbr;
            b.sctlr = sctlr;
            // The new core reads it with its caches off: write it out.
            core::arch::asm!("dc cvac, {}", "dsb sy", in(reg) b as *const SecondaryBoot as u64, options(nostack));
        }
        let before = online();
        let r = super::psci::call(PSCI_0_2_FN64_CPU_ON, id, secondary_entry as usize as u64, before as u64);
        if r != PSCI_RET_SUCCESS && r != PSCI_RET_ALREADY_ON {
            crate::kprintln!("[npk] PSCI: CPU_ON {:#x} failed ({})", id, r);
            crate::memory::deallocate_contiguous(stack, STACK_PAGES);
            continue;
        }
        let deadline = crate::time::uptime_us() + 1_000_000;
        while online() == before && crate::time::uptime_us() < deadline {
            core::hint::spin_loop();
        }
        if online() == before {
            // Its index would be the next core's: start no further one.
            crate::kprintln!("[npk] PSCI: core {:#x} did not come up", id);
            break;
        }
    }
    online()
}

/// A started core, with its MMU on and its own stack.
#[unsafe(no_mangle)]
extern "C" fn secondary_main(index: usize) -> ! {
    super::exceptions::init();
    super::gic::cpu_init();
    super::timer::start_this_core();
    ONLINE.fetch_add(1, Ordering::AcqRel);
    let e = ENTRY.load(Ordering::Acquire);
    if e == 0 { super::halt_forever(); }
    // SAFETY: only `start` stores here, always a `fn(usize) -> !`.
    let entry: fn(usize) -> ! = unsafe { core::mem::transmute(e) };
    entry(index)
}
