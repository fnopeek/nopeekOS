//! aml.wasm — the AML battery driver.
//!
//! Resident background driver (autostart). Once at startup it fetches the
//! firmware DSDT; each tick it parses it, runs the device's own `_BST`/`_BIF`
//! AML (vendor-independent, via [`aml_core`]) against the embedded controller,
//! and pushes the decoded percentage to the kernel via `npk_battery_report`.
//! The bar reads it through the unchanged `npk_battery()`.
//!
//! No persisted device state: the DSDT is the source of truth, re-parsed each
//! tick (cheap), so the same binary works on any laptop.

#![no_std]

extern crate alloc;

use aml_core::{find_batteries, read_battery, Ec, Namespace};

// The driver needs raw firmware + EC access — declare the HARDWARE capability
// (bit 0x40) so the kernel grants exactly that and nothing else.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x40];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log("[aml] panic");
    loop {}
}

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_acpi_dsdt(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_ec_read(addr: i32) -> i32;
    fn npk_ec_write(addr: i32, val: i32) -> i32;
    fn npk_battery_report(packed: i32);
    fn npk_sleep(ms: i32) -> i32;
    fn npk_log_serial(ptr: i32, len: i32);
}

fn log(s: &str) {
    unsafe { npk_log_serial(s.as_ptr() as i32, s.len() as i32) };
}

// ── bump allocator: reset to zero each tick (re-parse is fully transient) ──
const HEAP_SIZE: usize = 16 * 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_POS: usize = 0;

struct Bump;
unsafe impl core::alloc::GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let pos_ptr = core::ptr::addr_of_mut!(HEAP_POS);
        let current = unsafe { pos_ptr.read() };
        let aligned = (current + layout.align() - 1) & !(layout.align() - 1);
        if aligned + layout.size() > HEAP_SIZE {
            return core::ptr::null_mut();
        }
        unsafe { pos_ptr.write(aligned + layout.size()) };
        unsafe { (core::ptr::addr_of_mut!(HEAP) as *mut u8).add(aligned) }
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}
#[global_allocator]
static ALLOC: Bump = Bump;

fn heap_reset() {
    unsafe { core::ptr::addr_of_mut!(HEAP_POS).write(0) };
}

// DSDT buffer: filled once, persists across heap resets (it's not on the heap).
const DSDT_MAX: usize = 512 * 1024;
static mut DSDT: [u8; DSDT_MAX] = [0; DSDT_MAX];

struct HostEc;
impl Ec for HostEc {
    fn read(&mut self, addr: u8) -> u8 {
        let r = unsafe { npk_ec_read(addr as i32) };
        if r < 0 { 0 } else { r as u8 }
    }
    fn write(&mut self, addr: u8, val: u8) {
        unsafe { npk_ec_write(addr as i32, val as i32) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    log("[aml] battery driver start");

    let dsdt_ptr = core::ptr::addr_of_mut!(DSDT) as *mut u8;
    let len = unsafe { npk_acpi_dsdt(dsdt_ptr as i32, DSDT_MAX as i32) };
    if len <= 0 || len as usize > DSDT_MAX {
        log("[aml] no DSDT (or too large) — driver idle");
        return;
    }
    let table_len = len as usize;

    loop {
        heap_reset();
        let table = unsafe { core::slice::from_raw_parts(dsdt_ptr as *const u8, table_len) };
        let packed = decode(table);
        unsafe { npk_battery_report(packed) };
        unsafe { npk_sleep(10_000) };
    }
}

/// Parse the DSDT and evaluate the first present battery; return the bar's
/// packed encoding ((status<<8)|percent) or -1 if none.
fn decode(table: &[u8]) -> i32 {
    let ns = match Namespace::load(table) {
        Ok(ns) => ns,
        Err(_) => return -1,
    };
    let mut ec = HostEc;
    for bat in find_batteries(&ns) {
        if let Ok(info) = read_battery(&ns, &mut ec, &bat) {
            if info.present {
                // bar status: 0=discharging 1=charging 2=full 3=plugged-idle.
                let status = if info.state & 0x2 != 0 {
                    1
                } else if info.state & 0x1 != 0 {
                    0
                } else if info.percent >= 99 {
                    2
                } else {
                    3
                };
                return (status << 8) | info.percent as i32;
            }
        }
    }
    -1
}
