//! nopeekOS Kernel
//!
//! Not Unix. Not POSIX. No legacy.
//! A system built for AI as the operator, with humans as the conductor.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

core::arch::global_asm!(include_str!("boot.s"), options(att_syntax));

mod serial;
mod crypto;
mod csprng;
mod audit;
mod capability;
mod heap;
mod interrupts;
mod memory;
mod paging;
mod pci;
mod virtio_blk;
mod virtio_net;
mod npkfs;
mod net;
mod intent;
mod tls;
mod config;
mod rtc;
mod vga;
mod wasm;
mod shell;
mod keyboard;
mod nvme;
mod blkdev;
mod intel_nic;
mod netdev;
mod setup;
mod framebuffer;
mod gpu;
mod gui;
mod xhci;
#[allow(dead_code)]
mod gpt;
#[allow(dead_code)]
mod fat32;
#[allow(dead_code, unused_imports)]
mod install;
mod acpi;
mod update_key;

use alloc::string::String;
use spin::Mutex;
use core::panic::PanicInfo;

/// GPU boot log held in RAM until master key is available after login.
static GPU_BOOT_LOG: Mutex<Option<String>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kernel_main(multiboot_magic: u32, multiboot_info: u32) -> ! {
    vga::show_boot_banner();

    {
        let mut serial = serial::SERIAL.lock();
        serial.init();
    }

    kprintln!("                                __   ____  _____");
    kprintln!("   ____  ____  ____  ___  ___  / /__/ __ \\/ ___/");
    kprintln!("  / __ \\/ __ \\/ __ \\/ _ \\/ _ \\/ //_/ / / /\\__ \\ ");
    kprintln!(" / / / / /_/ / /_/ /  __/  __/ ,< / /_/ /___/ / ");
    kprintln!("/_/ /_/\\____/ .___/\\___/\\___/_/|_|\\____//____/  ");
    kprintln!("           /_/");
    kprintln!();
    kprintln!("[npk] AI-native Operating System v0.1.0");
    kprintln!("[npk] Booting...");
    kprintln!();

    if multiboot_magic == 0x36d76289 {
        kprintln!("[npk] Multiboot2: verified");
    } else {
        kprintln!("[npk] WARNING: Multiboot2 magic mismatch: {:#x}", multiboot_magic);
    }


    kprintln!("[npk] Initializing IDT + PIC...");
    interrupts::init();

    kprintln!("[npk] Interrupts enabled.");
    interrupts::calibrate_tsc();
    interrupts::init_tsc_ticks();
    kprintln!("[npk] TSC: {} MHz", interrupts::tsc_freq() / 1_000_000);
    keyboard::init();

    kprintln!("[npk] Initializing Physical Memory Manager...");
    memory::init(multiboot_info);


    kprintln!("[npk] Initializing Heap Allocator...");
    heap::init();

    // Start capturing boot log for debug shell (needs heap)
    serial::start_capture();

    kprintln!("[npk] Initializing Virtual Memory Manager...");
    paging::init();


    // Framebuffer init (needs memory + paging for MMIO mapping)
    framebuffer::init_from_multiboot2(multiboot_info);

    // ACPI init: parse Multiboot2 RSDP tag (UEFI), then find FADT for power-off
    acpi::parse_multiboot2_rsdp(multiboot_info);
    acpi::init();

    kprintln!("[npk] Scanning PCI bus...");
    let pci_count = pci::scan();
    kprintln!("[npk] PCI: {} devices", pci_count);
    vga::show_status(b"PCI bus scanned");

    // USB keyboard (xHCI) — before any user input is needed
    if xhci::init() {
        vga::show_status(b"USB keyboard online");
    }

    kprintln!("[npk] Probing block devices...");
    if virtio_blk::init() {
        vga::show_status(b"virtio-blk online");
    }
    if nvme::init() {
        vga::show_status(b"NVMe online");
    }
    if !virtio_blk::is_available() && !nvme::is_available() {
        kprintln!("[npk] No block device found.");
    }

    // RTC: immediate wall clock (no network needed)
    if let Some(t) = rtc::read_unix_time() {
        net::ntp::set_time(t);
        kprintln!("[npk] RTC: {}", net::ntp::format_time(t));
        vga::show_status(b"RTC clock set");
    } else {
        kprintln!("[npk] RTC: read failed");
    }

    kprintln!("[npk] Probing network...");
    let _net_up = virtio_net::init() || intel_nic::init();
    if netdev::is_available() {
        vga::show_status(b"Network online");

        kprintln!("[npk] Running DHCP...");
        if net::dhcp::configure() {
            vga::show_status(b"DHCP configured");
        }

        kprintln!("[npk] Syncing time (NTP)...");
        if net::ntp::sync_via_dns("pool.ntp.org") {
            if let Some(t) = net::ntp::unix_time() {
                kprintln!("[npk] NTP: {}", net::ntp::format_time(t));
            }
            vga::show_status(b"NTP synced");
        } else {
            kprintln!("[npk] NTP: sync failed (using RTC time)");
        }
    } else {
        kprintln!("[npk] No network device found.");
    }

    csprng::init();

    // Select random color scheme for boot messages and login screen
    gui::background::init();
    framebuffer::set_npk_color(gui::background::accent_color());

    // Debug shell disabled — enable when needed:
    // if netdev::is_available() { shell::start_debug_listener(); }

    kprintln!("[npk] Initializing WASM Runtime...");
    wasm::init();
    vga::show_status(b"WASM runtime online (wasmi)");

    // === Identity: Passphrase → Master Key ===
    //
    // First boot:       Setup wizard (storage, name, passphrase, settings)
    // Subsequent boots: Enter passphrase → verify against keycheck
    //
    // No users. No accounts. Your passphrase IS your identity.

    // Try to mount existing npkFS first
    let mut mounted = false;
    if blkdev::is_available() {
        // Installer build (USB stick): always install to NVMe, never ask for passphrase
        if install::has_installer() && nvme::is_available() {
            kprintln!();
            match install::install_to_nvme() {
                Ok(()) => {
                    mounted = true;
                    vga::show_status(b"Installed to NVMe");
                }
                Err(e) => {
                    kprintln!("[npk] Installation failed: {}", e);
                    kprintln!("[npk] System halted.");
                    loop { unsafe { core::arch::asm!("cli; hlt"); } }
                }
            }
        } else {
            // Normal boot: detect GPT partition layout, mount existing npkFS
            if nvme::is_available() {
                if let Some(offset) = gpt::detect_npkfs_offset() {
                    blkdev::set_partition_offset(offset);
                }
            }

            if npkfs::mount().is_ok() {
                mounted = true;
                vga::show_status(b"npkFS mounted");
            } else if blkdev::is_available() {
                // No existing npkFS: format and mount
                kprintln!("[npk] npkfs: not found, formatting...");
                match npkfs::mkfs() {
                    Ok(()) => {
                        if npkfs::mount().is_ok() {
                            mounted = true;
                            vga::show_status(b"npkFS formatted");
                        } else {
                            kprintln!("[npk] npkfs: mount after format failed");
                        }
                    }
                    Err(e) => kprintln!("[npk] npkfs: format failed: {}", e),
                }
            }
        }
    }

    // Per-installation random salt (generated at mkfs, stored in superblock)
    let salt = npkfs::install_salt().unwrap_or_else(|| {
        let mut s = [0u8; 16];
        let hash = blake3::hash(b"nopeekOS.fallback.salt");
        s.copy_from_slice(&hash.as_bytes()[..16]);
        s
    });

    let is_first_boot = !mounted || !npkfs::exists(".npk-keycheck");

    if is_first_boot {
        // === First boot: Setup Wizard (identity, settings) ===
        if !setup::run_fresh_install(&salt) {
            kprintln!("[npk] Setup failed. System halted.");
            loop { unsafe { core::arch::asm!("cli; hlt"); } }
        }
        vga::show_status(b"Setup complete");
    } else {
        // === Subsequent boot: Verify passphrase ===
        if framebuffer::is_available() {
            // Activate native GPU + 4K before login screen
            if gpu::native_detected() {
                // Capture serial output during GPU init for diagnostics
                let _ = serial::stop_capture();
                serial::start_capture();

                match gpu::activate_native() {
                    Ok(fb) => {
                        framebuffer::init_from_gpu();
                        kprintln!("[npk] GPU: {}x{} (native)", fb.width, fb.height);
                    }
                    Err(e) => kprintln!("[npk] GPU: native init failed: {:?}, using GOP", e),
                }

                // Keep GPU log in RAM — save to npkFS after login (needs master key)
                let gpu_log = serial::stop_capture();
                serial::start_capture();
                GPU_BOOT_LOG.lock().replace(gpu_log);
            }
            // Graphical login screen
            let _master_key = gui::login::run(&salt);

            // Now we have the master key — save GPU boot log if present
            if let Some(log) = GPU_BOOT_LOG.lock().take() {
                let name = gpu::next_log_name();
                let _ = npkfs::store(&name, log.as_bytes(), [0u8; 32]);
                kprintln!("[npk] GPU log saved: {}", name);
            }

            // Auto-upgrade to highest refresh rate if monitor is now connected
            if gpu::is_native() && gpu::current_hz() < 60 {
                kprintln!("[npk] GPU: upgrading to 4K@60Hz...");
                match gpu::set_mode(3840, 2160, 60) {
                    Ok(fb) => {
                        framebuffer::init_from_gpu();
                        kprintln!("[npk] GPU: {}x{}@60Hz active", fb.width, fb.height);
                    }
                    Err(_) => kprintln!("[npk] GPU: 4K@60 upgrade failed, staying at 30Hz"),
                }
            }
        } else {
            // Fallback: text-mode login (serial only, no framebuffer)
            text_mode_auth(&salt);
        }
        vga::show_status(b"Identity verified");
    }

    // Load system config (after identity — config is encrypted at rest)
    config::load();

    // Bootstrap WASM modules (after identity — so they are encrypted at rest)
    intent::bootstrap_wasm();

    // Create home directory and set as working directory
    intent::setup_home();

    kprintln!("[npk] Initializing Capability Vault...");
    let (vault_ref, root_id) = capability::Vault::init();
    kprintln!("[npk] Vault online. Root cap: {:08x}", capability::short_id(&root_id));
    vga::show_status(b"Capability Vault online");

    // Delegate a console session from root (no DELEGATE/REVOKE rights)
    let session_id = {
        use capability::{Rights, ResourceKind};
        let mut vault = vault_ref.lock();
        vault.create(
            root_id,
            ResourceKind::Kernel,
            Rights::READ | Rights::WRITE | Rights::EXECUTE | Rights::AUDIT,
            None,
        ).expect("failed to create session capability")
    };
    kprintln!("[npk] Console session: {:08x}", capability::short_id(&session_id));
    vga::show_status(b"Console session issued");

    // Start npk-shell listener (encrypted remote access, port 4444)
    shell::start_listener();

    kprintln!("[npk] Starting Intent Loop...");
    vga::show_status(b"Intent Loop running");
    vga::show_ready();

    kprintln!();
    kprintln!("[npk] ====================================");
    kprintln!("[npk]  System ready. Express your intent.");
    kprintln!("[npk] ====================================");
    kprintln!();

    intent::run_loop(vault_ref, session_id);
}

/// Text-mode passphrase authentication (fallback when no framebuffer).
fn text_mode_auth(salt: &[u8; 16]) {
    kprintln!();
    kprintln!("[npk] ─────────────────────────────────");
    kprintln!("[npk]  Identity required.");
    kprintln!("[npk]  Your passphrase IS your identity.");
    #[cfg(feature = "installer")]
    kprintln!("[npk]  Type RESET to wipe and reinstall.");
    kprintln!("[npk] ─────────────────────────────────");
    kprintln!();

    let mut attempts: u32 = 0;
    loop {
        if attempts > 0 {
            let delay_secs = 1u64 << attempts.min(5);
            kprintln!("[npk] Wait {} seconds...", delay_secs);
            let start = interrupts::ticks();
            let delay_ticks = delay_secs * 100;
            while interrupts::ticks() - start < delay_ticks {
                core::hint::spin_loop();
            }
        }

        kprint!("[npk] Passphrase: ");
        let mut buf = [0u8; 128];
        let len = { serial::SERIAL.lock().read_line_masked(&mut buf) };
        if len == 0 {
            kprintln!("[npk] Passphrase cannot be empty.");
            continue;
        }

        #[cfg(feature = "installer")]
        if len == 5 && &buf[..5] == b"RESET" {
            kprintln!();
            kprintln!("[npk] !! FACTORY RESET !!");
            kprintln!("[npk] This will ERASE ALL DATA.");
            kprint!("[npk] Type YES to confirm: ");
            let mut confirm = [0u8; 16];
            let clen = { serial::SERIAL.lock().read_line(&mut confirm) };
            if clen == 3 && &confirm[..3] == b"YES" {
                for b in buf.iter_mut() { *b = 0; }
                if install::has_installer() && nvme::is_available() {
                    match install::install_to_nvme() {
                        Ok(()) => {}
                        Err(e) => {
                            kprintln!("[npk] Installation failed: {}", e);
                            kprintln!("[npk] System halted.");
                            loop { unsafe { core::arch::asm!("cli; hlt"); } }
                        }
                    }
                } else {
                    kprintln!("[npk] Formatting...");
                    let _ = npkfs::mkfs();
                    let _ = npkfs::mount();
                }
                let salt = npkfs::install_salt().unwrap_or_else(|| {
                    let mut s = [0u8; 16];
                    let hash = blake3::hash(b"nopeekOS.fallback.salt");
                    s.copy_from_slice(&hash.as_bytes()[..16]);
                    s
                });
                kprintln!();
                if !setup::run_fresh_install(&salt) {
                    kprintln!("[npk] Setup failed. System halted.");
                    loop { unsafe { core::arch::asm!("cli; hlt"); } }
                }
                return;
            } else {
                kprintln!("[npk] Reset cancelled.");
                continue;
            }
        }

        let key = crypto::derive_master_key(&buf[..len], salt);
        for b in buf.iter_mut() { *b = 0; }

        crypto::set_master_key(key);

        match npkfs::fetch(".npk-keycheck") {
            Ok((data, _)) if &data[..] == b"nopeekOS.keycheck.v1.valid" => {
                config::load();
                if let Some(name) = config::get("name") {
                    kprintln!("[npk] Welcome back, {}.", name);
                } else {
                    kprintln!("[npk] Identity verified.");
                }
                return;
            }
            _ => {
                kprintln!("[npk] Wrong passphrase.");
            }
        }

        crypto::clear_master_key();
        attempts += 1;

        if attempts >= 10 {
            kprintln!("[npk] Too many failed attempts. System halted.");
            loop { unsafe { core::arch::asm!("cli; hlt"); } }
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kprintln!();
    kprintln!("[npk] !!! KERNEL PANIC !!!");
    if let Some(location) = info.location() {
        kprintln!("[npk] at {}:{}", location.file(), location.line());
    }
    kprintln!("[npk] {}", info.message());
    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}
