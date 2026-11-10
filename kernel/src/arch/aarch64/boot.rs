//! AArch64 bring-up: from `_start` to a running heap.
//!
//! What runs here is the part of the kernel that is ported so far: the
//! identity map, the console, the clock, the frame allocator and the heap,
//! all from the device tree. The rest of `kmain` follows module by module.

use crate::boot_info::{BootInfo, MemoryRegion, UEFI_CONVENTIONAL_MEMORY, UEFI_LOADER_DATA};
use crate::kprintln;
use crate::platform::dt::Fdt;

const PAGE: u64 = 4096;
const MAX_REGIONS: usize = 128;
const MAX_HOLES: usize = 64;

static mut REGIONS: [MemoryRegion; MAX_REGIONS] = [MemoryRegion::ZERO; MAX_REGIONS];
static mut BOOT_INFO: BootInfo = BootInfo::empty();

unsafe extern "C" {
    static __image_base: u8;
    static __heap_start: u8;
}

/// Every range of RAM the kernel must not hand out: the kernel image, the
/// device tree, the reservation block and /reserved-memory.
fn holes(fdt: &Fdt, out: &mut [(u64, u64); MAX_HOLES]) -> usize {
    let mut n = 0;
    let mut add = |a: u64, s: u64| {
        if n < MAX_HOLES && s > 0 {
            out[n] = (a, a.saturating_add(s));
            n += 1;
        }
    };
    // SAFETY: linker symbols; only their addresses are taken.
    let (lo, hi) = unsafe { (&__image_base as *const u8 as u64, &__heap_start as *const u8 as u64) };
    add(lo, hi - lo);
    add(fdt.addr() as u64, fdt.size() as u64);
    fdt.reservations(|a, s| add(a, s));
    // Whole 2 MiB blocks: `screen` maps them uncached, and no other data
    // may share them (exclusive loads and stores need cacheable memory).
    if let Some(Ok(fb)) = crate::platform::simplefb::find(fdt) {
        let lo = fb.addr & !((2 << 20) - 1);
        add(lo, (fb.addr + fb.size).next_multiple_of(2 << 20) - lo);
    }
    if let Some(rm) = fdt.find_path(b"/reserved-memory") {
        fdt.walk(|node| {
            if node.ancestors().last() == Some(&rm.offset) {
                fdt.reg(node, |a, s| add(a, s));
            }
            true
        });
    }
    n
}

/// The firmware's memory map for the frame allocator: RAM as conventional
/// memory, minus the holes, which stay typed as loader data (RAM that is
/// owned).
fn build_regions(fdt: &Fdt) -> &'static BootInfo {
    let mut hole = [(0u64, 0u64); MAX_HOLES];
    let nh = holes(fdt, &mut hole);
    let hole = &mut hole[..nh];
    hole.sort_unstable_by_key(|h| h.0);

    // SAFETY: single-threaded boot; nothing reads these until it returns.
    let regions = unsafe { &mut *core::ptr::addr_of_mut!(REGIONS) };
    let mut n = 0;
    let mut push = |start: u64, end: u64, ty: u32| {
        let (s, e) = if ty == UEFI_CONVENTIONAL_MEMORY {
            (start.next_multiple_of(PAGE), end & !(PAGE - 1))
        } else {
            (start & !(PAGE - 1), end.next_multiple_of(PAGE))
        };
        if e > s && n < MAX_REGIONS {
            regions[n] = MemoryRegion { physical_start: s, page_count: (e - s) / PAGE, uefi_type: ty };
            n += 1;
        }
    };
    for &(base, size) in crate::arch::aarch64::mmu::ram() {
        let end = base + size;
        let mut at = base;
        for &(hs, he) in hole.iter() {
            if he <= at || hs >= end { continue; }
            if hs > at { push(at, hs, UEFI_CONVENTIONAL_MEMORY); }
            push(hs.max(at), he.min(end), UEFI_LOADER_DATA);
            at = he.min(end);
        }
        if at < end { push(at, end, UEFI_CONVENTIONAL_MEMORY); }
    }
    // SAFETY: as above.
    unsafe {
        let bi = &mut *core::ptr::addr_of_mut!(BOOT_INFO);
        bi.region_count = n as u32;
        bi.regions_ptr = regions.as_ptr();
        &*bi
    }
}

/// Use the console the device tree names, or the first PL011.
fn console(fdt: &Fdt) -> bool {
    let node = fdt.stdout()
        .filter(|n| fdt.prop_has_str(n, b"compatible", b"arm,pl011"))
        .or_else(|| fdt.find_compatible(b"arm,pl011"));
    match node.and_then(|n| fdt.reg0(&n)) {
        Some((base, _)) => {
            crate::drivers::pl011::set_base(base);
            crate::serial::SERIAL.lock().init();
            true
        }
        None => false,
    }
}

fn cpu_count(fdt: &Fdt) -> usize {
    let Some(cpus) = fdt.find_path(b"/cpus") else { return 0 };
    let mut n = 0;
    fdt.walk(|node| {
        if node.ancestors().last() == Some(&cpus.offset) && fdt.prop_has_str(node, b"device_type", b"cpu") {
            n += 1;
        }
        true
    });
    n
}

/// The console's accent on the Pi: the `[npk]` tag and the berry above
/// the banner, in raspberry red.
const RASPBERRY: u32 = 0x00C5_1A4A;

/// A raspberry, leaves on top.
const BERRY: [&str; 8] = [
    r"            \|/",
    r"        .-(o)(o)-.",
    r"       (o)(o)(o)(o)",
    r"      (o)(o)(o)(o)(o)",
    r"       (o)(o)(o)(o)",
    r"        (o)(o)(o)",
    r"          (o)(o)",
    r"            ''",
];

/// Console output to the text framebuffer and the remote mirror.
fn console_screen(s: &str) {
    crate::framebuffer::write_str(s);
    crate::serial::mirror_push(s);
}

/// The firmware's framebuffer as a second console beside the UART: on a
/// board without a serial adapter it is the only output.
fn screen(fdt: &Fdt) {
    let Some(Ok(m)) = crate::platform::simplefb::find(fdt) else { return };
    if !super::mmu::map_uncached(m.addr, m.addr + m.size) { return; }
    crate::framebuffer::init_from_firmware(m.addr, m.width, m.height, m.stride, m.format.bpp());
}

/// What `screen` found, once the console is up to show it.
fn report_screen(fdt: &Fdt) {
    use crate::platform::simplefb::Error;
    match crate::platform::simplefb::find(fdt) {
        None => kprintln!("[npk] Framebuffer: none in the device tree"),
        Some(Ok(m)) if crate::framebuffer::is_available() => kprintln!(
            "[npk] Framebuffer: {}x{} {:?}, stride {} @ {:#x} (firmware)",
            m.width, m.height, m.format, m.stride, m.addr),
        Some(Ok(m)) => kprintln!("[npk] Framebuffer: {:#x} cannot be mapped uncached", m.addr),
        Some(Err(Error::Format(name))) => {
            let n = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            kprintln!("[npk] Framebuffer: format {:?} not drawn", core::str::from_utf8(&name[..n]).unwrap_or("?"));
        }
        Some(Err(e)) => kprintln!("[npk] Framebuffer: unusable node ({:?})", e),
    }
}

/// GIC and the periodic tick on the boot core, interrupts on.
fn interrupts(fdt: &Fdt) {
    if !super::gic::init(fdt) {
        kprintln!("[npk] GIC: none in the device tree");
        return;
    }
    let Some(irq) = super::timer::start(fdt) else {
        kprintln!("[npk] GIC: {} lines; timer interrupt not in the device tree", super::gic::irqs());
        return;
    };
    // SAFETY: vectors, GIC and timer handler are in place.
    unsafe { core::arch::asm!("msr DAIFClr, #2", options(nomem, nostack)) };
    kprintln!("[npk] GIC: {} lines; timer on INTID {} ({} Hz)", super::gic::irqs(), irq, super::timer::HZ);
}

/// The hardware RNG seeds the CSPRNG.
fn rng(fdt: &Fdt) {
    let rng = crate::platform::rng200::init(fdt);
    crate::csprng::init();
    kprintln!("[npk] CSPRNG: {}seeded{}",
        if crate::csprng::hardware_seeded() { "hardware-" } else { "jitter-" },
        if rng { " (RNG200)" } else { ", no RNG200 in the device tree" });
}

/// npkFS on the system disk, then the identity on it: the setup on a
/// fresh disk, the passphrase on one set up before (kmain's order).
fn identity() {
    let mounted = !matches!(crate::npkfs::bring_up(true), crate::npkfs::BroughtUp::NotMounted);
    if mounted {
        kprintln!("[npk] npkfs: mounted on the SD card");
    }
    if !mounted || !crate::npkfs::has_keyslot() {
        if !crate::setup::run_fresh_install() {
            kprintln!("[npk] Setup failed. System halted.");
            crate::arch::halt_forever()
        }
    } else {
        crate::setup::text_unlock();
    }
}

/// The kernel's entry on AArch64, from `_start` with the MMU on.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb: usize) -> ! {
    // SAFETY: `_start` passes the loader's device tree, mapped by `early_mmu`.
    let Some(fdt) = (unsafe { Fdt::new(dtb) }) else { crate::arch::halt_forever() };
    super::cpu::set_core_index(0);
    super::exceptions::init();
    super::mmu::init(&fdt);
    if crate::platform::led::init(&fdt) {
        crate::platform::led::set(true);
    }
    console(&fdt);
    super::time::calibrate_counter();
    crate::time::init_clock();
    let bi = build_regions(&fdt);
    crate::memory::init(bi);
    crate::heap::init();
    crate::serial::start_capture();
    screen(&fdt);
    crate::serial::set_hooks(crate::serial::Hooks {
        write: console_screen,
        echo: crate::framebuffer::write_byte,
        key: crate::input::read_key,
        quiet: crate::serial::mirror_push,
    });
    // The UART and the text framebuffer are the shell's only screen.
    crate::serial::set_line_repair(true);

    crate::framebuffer::set_npk_color(RASPBERRY);
    crate::framebuffer::with_text_color(RASPBERRY, || {
        for l in BERRY { kprintln!("{}", l); }
    });
    kprintln!("                                __   ____  _____");
    kprintln!("   ____  ____  ____  ___  ___  / /__/ __ \\/ ___/");
    kprintln!("  / __ \\/ __ \\/ __ \\/ _ \\/ _ \\/ //_/ / / /\\__ \\ ");
    kprintln!(" / / / / /_/ / /_/ /  __/  __/ ,< / /_/ /___/ / ");
    kprintln!("/_/ /_/\\____/ .___/\\___/\\___/_/|_|\\____//____/  ");
    kprintln!("           /_/");
    kprintln!();
    kprintln!("[npk] nopeekOS v{} on AArch64", env!("CARGO_PKG_VERSION"));
    kprintln!();

    let mut model = [0u8; 96];
    let root = fdt.find_path(b"/");
    let m = root.and_then(|r| fdt.prop_str(&r, b"model", &mut model)).unwrap_or(0);
    if m > 0 {
        kprintln!("[npk] Board: {}", core::str::from_utf8(&model[..m]).unwrap_or("?"));
    }
    let el = super::cpu::exception_level();
    kprintln!("[npk] Exception level: EL{}{}", el, if el == 2 { " (VHE)" } else { "" });
    kprintln!("[npk] CPUs in the device tree: {}", cpu_count(&fdt));

    kprintln!("[npk] Generic timer: {} MHz", crate::time::counter_hz() / 1_000_000);

    for &(b, s) in super::mmu::ram() {
        kprintln!("[npk] RAM: {:#x}..{:#x} ({} MB)", b, b + s, s >> 20);
    }
    report_screen(&fdt);
    interrupts(&fdt);

    rng(&fdt);
    let cores = crate::smp::init(&fdt);
    kprintln!("[npk] Cores online: {} (PSCI), {} workers", cores, crate::smp::scheduler::worker_count());
    crate::platform::pi5::init(&fdt);
    crate::platform::pi5_eth::init(&fdt);
    let Some(id) = crate::platform::pi5_sd::init(&fdt) else {
        kprintln!("[npk] No SD card: no system disk. System halted.");
        crate::arch::halt_forever()
    };
    let Some((off, blocks)) = crate::platform::pi5_sd::system_disk(id) else {
        kprintln!("[npk] No system disk on the SD card. System halted.");
        crate::arch::halt_forever()
    };
    crate::blkdev::use_sd(id);
    crate::blkdev::set_partition_offset(off);
    crate::blkdev::set_partition_size(blocks);
    identity();
    shell()
}

/// After unlock, as on a PC (`kmain`): the trust store, the home
/// directory, the console session's capability, then the intent loop as a
/// fiber on core 0.
fn shell() -> ! {
    crate::tls::certstore::load_store();
    crate::intent::setup_home();
    let (vault, root) = crate::capability::Vault::init();
    let session = {
        use crate::capability::{ResourceKind, Rights};
        vault.lock().create(
            root,
            ResourceKind::Kernel,
            Rights::READ | Rights::WRITE | Rights::EXECUTE | Rights::AUDIT,
            None,
            "loop",
        ).expect("failed to create session capability")
    };
    kprintln!("[npk] System ready.");
    crate::intent::system::persist_boot_log();
    *SHELL_ARGS.lock() = Some((vault, session));
    crate::smp::fiber::admit_with_stack(0, shell_fiber, 0, 2 * 1024 * 1024);
    crate::smp::per_core::core0_loop()
}

static SHELL_ARGS: spin::Mutex<Option<(&'static spin::Mutex<crate::capability::Vault>, crate::capability::CapId)>> =
    spin::Mutex::new(None);

fn shell_fiber(_: u64) {
    let args = SHELL_ARGS.lock().take();
    if let Some((vault, session)) = args {
        crate::intent::run_loop(vault, session);
    }
}
