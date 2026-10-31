//! fbench: how far forge's code is from native, workload by workload.
//!
//! Run as `forge run fbench` (or `run fbench` for the default engine). The
//! same workloads build natively with `cargo run --release --example native`
//! on the host; each line prints the time and a checksum, and the checksums
//! must match across the builds.

// Natively only the example builds, from `work.rs` alone.
#![cfg(target_arch = "wasm32")]
#![no_std]
#![feature(wasm_numeric_instr)]

extern crate alloc;

use alloc::format;
use nopeek_widgets::host::{print, sys_info};

mod work;

#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x04]; // EXEC

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

/// Raw TSC ticks and the TSC rate in MHz (`npk_sys_info` keys 19 and 10).
fn tsc() -> u64 {
    sys_info(19) as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let mhz = sys_info(10) as u64;
    if mhz == 0 {
        print("[fbench] no TSC rate\n");
        return;
    }
    print(concat!("[fbench] ", env!("CARGO_PKG_VERSION"), "\n"));
    let mut total = 0u64;
    for w in work::ALL {
        let t0 = tsc();
        let sum = (w.run)();
        let us = (tsc() - t0) / mhz;
        total += us;
        print(&format!("{:<10} {:>8}.{} ms  ({:016x})\n", w.name, us / 1000, (us % 1000) / 100, sum));
    }
    print(&format!("total      {:>8}.{} ms\n", total / 1000, (total % 1000) / 100));
}
