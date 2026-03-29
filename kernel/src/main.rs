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
mod audit;
mod capability;
mod heap;
mod interrupts;
mod memory;
mod intent;
mod store;
mod vga;

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

    kprintln!("[npk] Initializing Capability Vault...");
    let (vault_ref, root_id) = capability::Vault::init();
    kprintln!("[npk] Vault online. Root cap: {:08x}", capability::short_id(root_id));
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
    kprintln!("[npk] Console session: {:08x}", capability::short_id(session_id));
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
    if let Some(message) = info.message().as_str() {
        kprintln!("[npk] {}", message);
    }
    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}
