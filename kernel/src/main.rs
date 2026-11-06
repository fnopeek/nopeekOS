//! nopeekOS Kernel
//!
//! Not Unix. Not POSIX. No legacy.
//!
//! Modules marked `cfg(target_arch = "x86_64")` are not ported to AArch64
//! yet; each loses the mark when it builds there.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(include_str!("boot.s"), options(att_syntax));

// Boot info handed off from the firmware stub to kernel_main.
pub mod boot_info;
#[cfg(target_arch = "x86_64")]
mod boot_uefi;
#[cfg(target_arch = "x86_64")]
mod kmain;

// ── Module groups ──────────────────────────────────────────────
mod arch;
mod platform;
mod drivers;
pub use drivers::serial;
#[cfg(target_arch = "x86_64")]
pub use drivers::{pci, nvme, virtio_blk, virtio_net, intel_nic, rtl8153};
#[cfg(target_arch = "x86_64")]
pub use drivers::{xhci, usb_storage, keyboard, framebuffer, rtc, blkdev, netdev, acpi, smbus, battery, ec, sci};
#[cfg(target_arch = "x86_64")]
pub use drivers::wifi;

mod mm;
pub use mm::{memory, heap};
#[cfg(target_arch = "x86_64")]
pub use mm::paging;

#[cfg(target_arch = "x86_64")]
mod security;
#[cfg(target_arch = "x86_64")]
pub use security::{capability, audit, csprng, disk_key};

#[cfg(target_arch = "x86_64")]
mod crypto;
#[cfg(target_arch = "x86_64")]
pub use crypto::{tls, update_key};

#[cfg(target_arch = "x86_64")]
mod storage;
#[cfg(target_arch = "x86_64")]
pub use storage::{npkfs, fat32, gpt, ext};

#[cfg(target_arch = "x86_64")]
mod gui;
#[cfg(target_arch = "x86_64")]
pub use gui::theme;

// ── Standalone modules ────────────────────────────────────────
mod fatal;
mod time;
#[cfg(target_arch = "x86_64")]
mod interrupts;
#[cfg(target_arch = "x86_64")]
mod irq;
#[cfg(target_arch = "x86_64")]
mod ioapic;
#[cfg(target_arch = "x86_64")]
mod notify;
#[cfg(target_arch = "x86_64")]
mod status;
#[cfg(target_arch = "x86_64")]
mod input;
#[cfg(target_arch = "x86_64")]
mod net;
#[cfg(target_arch = "x86_64")]
mod audio;
#[cfg(target_arch = "x86_64")]
mod capture;
#[cfg(target_arch = "x86_64")]
mod camfeed;
#[cfg(target_arch = "x86_64")]
mod devices;
#[cfg(target_arch = "x86_64")]
mod intent;
#[cfg(target_arch = "x86_64")]
mod config;
#[cfg(target_arch = "x86_64")]
mod settings;
#[cfg(target_arch = "x86_64")]
mod shutdown;
#[cfg(target_arch = "x86_64")]
mod vga;
#[cfg(target_arch = "x86_64")]
mod hw;
#[cfg(target_arch = "x86_64")]
mod forge_rt;
#[cfg(target_arch = "x86_64")]
mod forge_tests;
#[cfg(target_arch = "x86_64")]
mod wasm;
#[cfg(target_arch = "x86_64")]
mod wasi;
#[cfg(target_arch = "x86_64")]
mod shell;
#[cfg(target_arch = "x86_64")]
mod setup;
#[cfg(target_arch = "x86_64")]
mod gpu;
#[cfg(target_arch = "x86_64")]
mod shade;
#[cfg(target_arch = "x86_64")]
mod smp;
#[cfg(target_arch = "x86_64")]
mod cpu_errata;
#[cfg(target_arch = "x86_64")]
mod cpu_vulns;
#[cfg(target_arch = "x86_64")]
mod spec_mitigations;
#[cfg(target_arch = "x86_64")]
mod process;
#[cfg(target_arch = "x86_64")]
mod tss;
#[cfg(target_arch = "x86_64")]
mod microvm;
#[cfg(target_arch = "x86_64")]
#[allow(dead_code, unused_imports)]
mod install;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    match info.location() {
        Some(l) => crate::fatal::die(format_args!("KERNEL PANIC !!!\n[npk] at {}:{}\n[npk] {}",
            l.file(), l.line(), info.message())),
        None => crate::fatal::die(format_args!("KERNEL PANIC !!!\n[npk] {}", info.message())),
    }
}
