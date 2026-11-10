//! AArch64 bring-up: from `_start` to a running heap.
//!
//! What runs here is the part of the kernel that is ported so far: the
//! identity map, the console, the clock, the frame allocator and the heap,
//! all from the device tree. The rest of `kmain` follows module by module.

use crate::boot_info::{BootInfo, MemoryRegion, UEFI_CONVENTIONAL_MEMORY, UEFI_LOADER_DATA};
use crate::kprintln;
use crate::platform::dt::Fdt;
use core::sync::atomic::{AtomicU64, Ordering};

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

/// The firmware's framebuffer as a second console beside the UART: on a
/// board without a serial adapter it is the only output.
fn screen(fdt: &Fdt) {
    let Some(Ok(m)) = crate::platform::simplefb::find(fdt) else { return };
    if !super::mmu::map_uncached(m.addr, m.addr + m.size) { return; }
    crate::framebuffer::init_from_firmware(m.addr, m.width, m.height, m.stride, m.format.bpp());
    crate::serial::set_hooks(crate::serial::Hooks {
        write: crate::framebuffer::write_str,
        echo: crate::framebuffer::write_byte,
        key: || None,
        quiet: |_| {},
    });
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

/// GIC and the periodic tick on the boot core, interrupts on, and a
/// check that the tick arrives.
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
    let before = super::timer::fired();
    crate::time::delay_ms(200);
    let n = super::timer::fired() - before;
    kprintln!("[npk] GIC: {} lines; timer on INTID {}: {} ticks in 200 ms ({} Hz)",
        super::gic::irqs(), irq, n, super::timer::HZ);
}

static TASK_CORES: AtomicU64 = AtomicU64::new(0);
static TASKS_DONE: AtomicU64 = AtomicU64::new(0);
static FIBER_CORES: AtomicU64 = AtomicU64::new(0);
static FIBERS_DONE: AtomicU64 = AtomicU64::new(0);

const NAPS: u64 = 20;
const NAP_MS: u64 = 5;

fn native_task(_arg: u64) {
    TASK_CORES.fetch_or(1 << crate::smp::per_core::current_core_id(), Ordering::Relaxed);
    crate::time::delay_ms(20);
    TASKS_DONE.fetch_add(1, Ordering::Release);
}

/// A fiber that sleeps NAPS times: each wake is the core's one-shot timer
/// ending its WFI.
fn napping_fiber(_arg: u64) {
    for _ in 0..NAPS {
        FIBER_CORES.fetch_or(1 << crate::smp::per_core::current_core_id(), Ordering::Relaxed);
        crate::smp::fiber::yield_sleep(NAP_MS);
    }
    FIBERS_DONE.fetch_add(1, Ordering::Release);
}

fn wait_for(counter: &AtomicU64, n: u64, ms: u64) -> Option<u64> {
    let t0 = crate::time::uptime_us();
    while counter.load(Ordering::Acquire) < n {
        if crate::time::uptime_us() - t0 > ms * 1000 { return None; }
        core::hint::spin_loop();
    }
    Some((crate::time::uptime_us() - t0) / 1000)
}

/// The other cores start into the scheduler: native tasks spread over the
/// workers, and fibers that sleep wake from their core's halt.
fn smp_check(fdt: &Fdt) {
    crate::smp::fiber::self_test();
    let cores = crate::smp::init(fdt);
    let workers = crate::smp::scheduler::worker_count() as u64;
    kprintln!("[npk] Cores online: {} (PSCI), {} workers", cores, workers);
    if workers == 0 { return; }

    let n = workers * 2;
    for _ in 0..n {
        crate::smp::scheduler::spawn("bring-up", native_task, 0);
    }
    let t = wait_for(&TASKS_DONE, n, 2000);
    kprintln!("[npk] Scheduler: {} native tasks of 20 ms {} on cores {:#b}",
        n, t.map_or("did NOT finish".into(), |ms| alloc::format!("done in {} ms", ms)),
        TASK_CORES.load(Ordering::Relaxed));

    let halts0: u64 = (1..=workers as usize).map(|c| crate::smp::per_core::halt_snapshot(c).1).sum();
    for _ in 0..n {
        crate::smp::scheduler::spawn_fiber(napping_fiber, 0);
    }
    let t = wait_for(&FIBERS_DONE, n, 3000);
    let halts: u64 = (1..=workers as usize).map(|c| crate::smp::per_core::halt_snapshot(c).1).sum::<u64>() - halts0;
    kprintln!("[npk] Fibers: {} x {} naps of {} ms {} on cores {:#b}, {} halts",
        n, NAPS, NAP_MS, t.map_or("did NOT finish".into(), |ms| alloc::format!("done in {} ms", ms)),
        FIBER_CORES.load(Ordering::Relaxed), halts);
}

/// The coherent DMA zone grows, shares a block, and gives everything back.
fn dma_check() {
    use crate::hw::DmaRegion;
    let free0 = crate::memory::stats();
    let a = DmaRegion::alloc_zeroed(1);
    let b = DmaRegion::alloc_zeroed_aligned_below(3, 4, 1 << 32);
    let c = DmaRegion::alloc_zeroed(600);
    let (Some(a), Some(b), Some(c)) = (a, b, c) else {
        kprintln!("[npk] DMA zone: allocation failed");
        return;
    };
    a.w32(0u64, 0xdead_beef);
    c.w64(599u64 * 4096, 0x0123_4567_89ab_cdef);
    let ok = a.r32(0u64) == 0xdead_beef && c.r64(599u64 * 4096) == 0x0123_4567_89ab_cdef
        && b.phys() % (4 * 4096) == 0 && b.phys() < 1 << 32;
    let held = crate::arch::aarch64::dma::zone_bytes();
    a.free();
    b.free();
    c.free();
    kprintln!("[npk] DMA zone (uncached): {} MiB while in use, {} after, frames back: {}, {}",
        held >> 20, crate::arch::aarch64::dma::zone_bytes() >> 20,
        crate::memory::stats() == free0, if ok { "ok" } else { "MISMATCH" });
}

/// What the screen path found, as a count of flashes for a board whose
/// screen stays dark (see `led::code`).
fn screen_code(fdt: &Fdt) -> u32 {
    use crate::platform::simplefb::{self, Error};
    match simplefb::find(fdt) {
        None if simplefb::disabled_node(fdt) => 2,
        None => 1,
        Some(Err(Error::Incomplete)) => 3,
        Some(Err(Error::Format(_))) => 4,
        Some(Err(Error::NoMemory)) => 5,
        Some(Err(Error::TooSmall)) => 6,
        Some(Ok(_)) if !crate::framebuffer::is_available() => 7,
        Some(Ok(_)) => 8,
    }
}

/// A kernel stack in 4 KiB pages above the identity map: its pages map,
/// hold what is written, its guard page stays unmapped, and dropping it
/// unmaps everything and gives the frames back (the page tables a first
/// stack made stay, as on x86).
fn stack_check() {
    drop(crate::mm::stack::KernelStack::new(4096));
    let free0 = crate::memory::stats();
    let Some(st) = crate::mm::stack::KernelStack::new(64 * 1024) else {
        kprintln!("[npk] Kernel stacks: allocation failed");
        return;
    };
    let (lo, top) = (st.lo(), st.top());
    let mut ok = true;
    let mut a = lo;
    while a < top {
        // SAFETY: lo..top is the stack's mapping, owned here.
        unsafe { core::ptr::write_volatile(a as *mut u64, a ^ 0x5a5a) };
        a += 4096;
    }
    let mut a = lo;
    while a < top {
        // SAFETY: as above.
        ok &= unsafe { core::ptr::read_volatile(a as *const u64) } == a ^ 0x5a5a;
        ok &= crate::paging::translate(a).is_some();
        a += 4096;
    }
    let guard_unmapped = crate::paging::translate(lo - 4096).is_none() && crate::mm::stack::is_guard(lo - 4096);
    drop(st);
    let gone = crate::paging::translate(lo).is_none();
    kprintln!("[npk] Kernel stacks (4 KiB pages @ {:#x}): data {}, guard page unmapped {}, unmapped after drop {}, frames back {}",
        lo, if ok { "ok" } else { "WRONG" }, guard_unmapped, gone, crate::memory::stats() == free0);
}

/// The hardware RNG seeds the CSPRNG; AES-256-GCM and SHA-256 against
/// known answers, then AES-GCM over 1 MiB, timed.
fn crypto_check(fdt: &Fdt) {
    let rng = crate::platform::rng200::init(fdt);
    crate::csprng::init();
    kprintln!("[npk] CSPRNG: {}seeded{}; {:016x} {:016x}",
        if crate::csprng::hardware_seeded() { "hardware-" } else { "jitter-" },
        if rng { " (RNG200)" } else { ", no RNG200 in the device tree" },
        crate::csprng::random_u64(), crate::csprng::random_u64());

    // McGrew/Viega, The Galois/Counter Mode of Operation, test case 14.
    let ct = crate::crypto::aead_encrypt_aes(&[0u8; 32], &[0u8; 12], &[0u8; 16]);
    let want: [u8; 32] = [
        0xce, 0xa7, 0x40, 0x3d, 0x4d, 0x60, 0x6b, 0x6e, 0x07, 0x4e, 0xc5, 0xd3, 0xba, 0xf3, 0x9d, 0x18,
        0xd0, 0xd1, 0xc8, 0xa7, 0x99, 0x99, 0x6b, 0xf0, 0x26, 0x5b, 0x98, 0xb5, 0xd4, 0x8a, 0xb9, 0x19,
    ];
    use sha2::Digest;
    let abc = sha2::Sha256::digest(b"abc");
    let abc_want: [u8; 32] = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
        0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
    ];

    let key = crate::csprng::random_256();
    let mut nonce = [0u8; 12];
    crate::csprng::fill(&mut nonce);
    let data: alloc::vec::Vec<u8> = (0..1usize << 20).map(|i| i as u8).collect();
    let t0 = crate::time::uptime_us();
    let mut buf = crate::crypto::aead_encrypt_aes(&key, &nonce, &data);
    let t1 = crate::time::uptime_us();
    let ok = crate::crypto::aead_decrypt_aes_in_place(&key, &nonce, &mut buf).is_some() && buf == data;
    let t2 = crate::time::uptime_us();
    let mbs = |us: u64| (1u64 << 20) / us.max(1);
    kprintln!("[npk] Crypto: AES-256-GCM known answer {}, SHA-256 known answer {}, 1 MiB round trip {}: \
        {} MB/s encrypt, {} MB/s decrypt",
        if ct[..] == want[..] { "ok" } else { "WRONG" },
        if abc[..] == abc_want[..] { "ok" } else { "WRONG" },
        if ok { "ok" } else { "FAILED" }, mbs(t1 - t0), mbs(t2 - t1));
}

/// The kernel's entry on AArch64, from `_start` with the MMU on.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb: usize) -> ! {
    // SAFETY: `_start` passes the loader's device tree, mapped by `early_mmu`.
    let Some(fdt) = (unsafe { Fdt::new(dtb) }) else { crate::arch::halt_forever() };
    super::cpu::set_core_index(0);
    super::exceptions::init();
    super::mmu::init(&fdt);
    let led = crate::platform::led::init(&fdt);
    crate::platform::led::set(true);
    console(&fdt);
    super::time::calibrate_counter();
    crate::time::init_clock();
    let bi = build_regions(&fdt);
    crate::memory::init(bi);
    crate::heap::init();
    crate::serial::start_capture();
    screen(&fdt);

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

    let v: alloc::vec::Vec<u64> = (1..=1000).collect();
    kprintln!("[npk] Heap online: sum 1..1000 = {}", v.iter().sum::<u64>());
    dma_check();
    crypto_check(&fdt);
    stack_check();
    smp_check(&fdt);
    let keyboard = crate::platform::pi5::init(&fdt);
    if let Some(id) = crate::platform::pi5_sd::init(&fdt) {
        crate::platform::pi5_sd::system_disk(id);
    }
    kprintln!("[npk] Uptime: {} ms", crate::time::uptime_us() / 1000);
    kprintln!();
    if keyboard {
        crate::platform::pi5::echo_keys(led);
    }
    if led {
        let code = screen_code(&fdt);
        kprintln!("[npk] AArch64 bring-up reached its end. Activity LED: screen code {}.", code);
        crate::platform::led::code(code)
    }
    kprintln!("[npk] AArch64 bring-up reached its end. Halting.");
    crate::arch::halt_forever()
}
