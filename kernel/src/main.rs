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
mod vga;
mod wasm;

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn kernel_main(multiboot_magic: u32, multiboot_info: u32) -> ! {
    vga::show_boot_banner();

    {
        let serial = serial::SERIAL.lock();
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
        vga::show_status(b"Multiboot2 verified");
    } else {
        kprintln!("[npk] WARNING: Multiboot2 magic mismatch: {:#x}", multiboot_magic);
    }

    kprintln!("[npk] Initializing IDT + PIC...");
    interrupts::init();
    kprintln!("[npk] Interrupts enabled.");
    vga::show_status(b"Interrupts enabled (IDT + PIC)");

    kprintln!("[npk] Initializing Physical Memory Manager...");
    memory::init(multiboot_info);
    vga::show_status(b"Physical memory mapped");

    kprintln!("[npk] Initializing Heap Allocator...");
    heap::init();
    vga::show_status(b"Heap allocator online");

    kprintln!("[npk] Initializing Virtual Memory Manager...");
    paging::init();
    vga::show_status(b"Virtual memory online");

    kprintln!("[npk] Scanning PCI bus...");
    let pci_count = pci::scan();
    kprintln!("[npk] PCI: {} devices", pci_count);
    vga::show_status(b"PCI bus scanned");

    kprintln!("[npk] Probing virtio-blk...");
    if virtio_blk::init() {
        vga::show_status(b"virtio-blk online");
    } else {
        kprintln!("[npk] virtio-blk: not available (no disk attached)");
    }

    kprintln!("[npk] Probing virtio-net...");
    if virtio_net::init() {
        vga::show_status(b"virtio-net online");

        kprintln!("[npk] Running DHCP...");
        if net::dhcp::configure() {
            vga::show_status(b"DHCP configured");
        }

        kprintln!("[npk] Syncing time (NTP)...");
        // NTP server: QEMU user-mode routes to host's NTP
        if net::ntp::sync([10, 0, 2, 3]) {
            if let Some(t) = net::ntp::unix_time() {
                kprintln!("[npk] Time: {}", net::ntp::format_time(t));
            }
            vga::show_status(b"NTP synced");
        } else {
            kprintln!("[npk] NTP: sync failed (non-critical)");
        }
    } else {
        kprintln!("[npk] virtio-net: not available");
    }

    csprng::init();

    if virtio_blk::is_available() {
        kprintln!("[npk] Mounting npkFS...");
        match npkfs::mount() {
            Ok(()) => vga::show_status(b"npkFS mounted"),
            Err(_) => {
                kprintln!("[npk] npkFS: not formatted, formatting...");
                match npkfs::mkfs().and_then(|_| npkfs::mount()) {
                    Ok(()) => vga::show_status(b"npkFS formatted + mounted"),
                    Err(e) => kprintln!("[npk] npkFS: failed: {}", e),
                }
            }
        }
    }

    kprintln!("[npk] Initializing WASM Runtime...");
    wasm::init();
    vga::show_status(b"WASM runtime online (wasmi)");

    // === Identity: Passphrase → Master Key ===
    //
    // First boot (setup):  Choose passphrase → store keycheck object
    // Subsequent boots:    Enter passphrase → verify against keycheck
    //
    // No users. No accounts. Your passphrase IS your identity.

    // Per-installation random salt (generated at mkfs, stored in superblock)
    let salt = npkfs::install_salt().unwrap_or_else(|| {
        let mut s = [0u8; 16];
        let hash = blake3::hash(b"nopeekOS.fallback.salt");
        s.copy_from_slice(&hash.as_bytes()[..16]);
        s
    });

    let is_setup = !npkfs::exists(".npk-keycheck");

    if is_setup {
        // === First boot: Setup ===
        kprintln!();
        kprintln!("[npk] ══════════════════════════════════");
        kprintln!("[npk]  Welcome to nopeekOS.");
        kprintln!("[npk]  No users. No root. No legacy.");
        kprintln!("[npk]  Choose a passphrase to protect");
        kprintln!("[npk]  this system. It cannot be recovered.");
        kprintln!("[npk] ══════════════════════════════════");
        kprintln!();

        let _master_key = loop {
            kprint!("[npk] New passphrase: ");
            let mut buf1 = [0u8; 128];
            let len1 = { serial::SERIAL.lock().read_line_masked(&mut buf1) };
            if len1 < 8 {
                kprintln!("[npk] Too short. Minimum 8 characters.");
                continue;
            }

            kprint!("[npk] Confirm passphrase: ");
            let mut buf2 = [0u8; 128];
            let len2 = { serial::SERIAL.lock().read_line_masked(&mut buf2) };

            if len1 != len2 || buf1[..len1] != buf2[..len2] {
                kprintln!("[npk] Passphrases do not match. Try again.");
                for b in buf1.iter_mut() { *b = 0; }
                for b in buf2.iter_mut() { *b = 0; }
                continue;
            }

            let key = crypto::derive_master_key(&buf1[..len1], &salt);
            for b in buf1.iter_mut() { *b = 0; }
            for b in buf2.iter_mut() { *b = 0; }
            break key;
        };

        // Store master key and write keycheck object
        crypto::set_master_key(_master_key);

        // The keycheck is a known plaintext that we can verify on next boot
        let keycheck_data = b"nopeekOS.keycheck.v1.valid";
        match npkfs::store(".npk-keycheck", keycheck_data, capability::CAP_NULL) {
            Ok(_) => kprintln!("[npk] Identity configured. System is yours."),
            Err(e) => kprintln!("[npk] WARNING: Could not store keycheck: {}", e),
        }
        vga::show_status(b"Identity configured");
    } else {
        // === Subsequent boot: Verify ===
        kprintln!();
        kprintln!("[npk] ─────────────────────────────────");
        kprintln!("[npk]  Identity required.");
        kprintln!("[npk]  Your passphrase IS your identity.");
        kprintln!("[npk] ─────────────────────────────────");
        kprintln!();

        let mut attempts: u32 = 0;
        let _master_key = loop {
            // Exponential backoff: 0, 2, 4, 8, 16... seconds
            if attempts > 0 {
                let delay_secs = 1u64 << attempts.min(5); // max 32s
                kprintln!("[npk] Wait {} seconds...", delay_secs);
                let start = interrupts::ticks();
                let delay_ticks = delay_secs * 100; // 100Hz timer
                while interrupts::ticks() - start < delay_ticks {
                    // SAFETY: hlt until next timer interrupt
                    unsafe { core::arch::asm!("hlt"); }
                }
            }

            kprint!("[npk] Passphrase: ");
            let mut buf = [0u8; 128];
            let len = { serial::SERIAL.lock().read_line_masked(&mut buf) };
            if len == 0 {
                kprintln!("[npk] Passphrase cannot be empty.");
                continue;
            }

            let key = crypto::derive_master_key(&buf[..len], &salt);
            for b in buf.iter_mut() { *b = 0; }

            // Temporarily set key to test decryption
            crypto::set_master_key(key);

            match npkfs::fetch(".npk-keycheck") {
                Ok((data, _)) => {
                    if &data[..] == b"nopeekOS.keycheck.v1.valid" {
                        kprintln!("[npk] Identity verified.");
                        break key;
                    }
                    // AEAD passed but content wrong — shouldn't happen
                    kprintln!("[npk] Keycheck mismatch.");
                }
                Err(_) => {
                    kprintln!("[npk] Wrong passphrase.");
                }
            }

            // Wrong passphrase — clear key and retry
            crypto::clear_master_key();
            attempts += 1;

            if attempts >= 10 {
                kprintln!("[npk] Too many failed attempts. System halted.");
                loop { unsafe { core::arch::asm!("cli; hlt"); } }
            }
        };

        vga::show_status(b"Identity verified");
    }

    // Bootstrap WASM modules (after identity — so they are encrypted at rest)
    intent::bootstrap_wasm();

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
