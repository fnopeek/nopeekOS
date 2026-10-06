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

use aml_core::{ec_gpe, find_batteries, read_battery, Ec, Namespace};

// The driver needs raw firmware + EC access — declare the HARDWARE capability
// (bit 0x40) so the kernel grants exactly that and nothing else.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x40];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    logln("[aml] panic");
    // Trap, do not spin. `loop {}` would turn every panic into a silent
    // hang; a trap is caught by the kernel and reported on screen.
    core::arch::wasm32::unreachable()
}

use npk_sys::bump::Bump;
use npk_sys::cell::Single;

/// Log to one channel only.
///
/// `npk_print` goes through `kprint!`, i.e. to the screen and the boot log
/// (`dmesg`). `npk_log_serial` writes to the UART and appends `\r\n` to
/// every call, so writing to both would duplicate lines and split them at
/// every number.
fn log(s: &str) {
    npk_sys::print(s.as_bytes());
}
fn logln(s: &str) { log(s); log("\n"); }

/// Two numbers on one line, for `[addr] -> value`; no allocating formatter.
fn lognum2(a: &str, x: u32, b: &str, y: u32) {
    log(a);
    lognum_raw(x);
    log(b);
    lognum_raw(y);
    log("\n");
}

fn loghex2(v: u8) {
    let d = |n: u8| if n < 10 { b'0' + n } else { b'A' + n - 10 };
    let b = [d(v >> 4), d(v & 0xF)];
    if let Ok(t) = core::str::from_utf8(&b) { log(t); }
}

fn lognum_raw(mut v: u32) {
    let mut buf = [0u8; 12];
    let mut i = buf.len();
    if v == 0 { i -= 1; buf[i] = b'0'; }
    while v > 0 { i -= 1; buf[i] = b'0' + (v % 10) as u8; v /= 10; }
    if let Ok(t) = core::str::from_utf8(&buf[i..]) { log(t); }
}

fn lognum(prefix: &str, mut v: u32) {
    let mut buf = [0u8; 12];
    let mut i = buf.len();
    if v == 0 { i -= 1; buf[i] = b'0'; }
    while v > 0 { i -= 1; buf[i] = b'0' + (v % 10) as u8; v /= 10; }
    log(prefix);
    if let Ok(t) = core::str::from_utf8(&buf[i..]) { log(t); }
    log("\n");
}

const WAIT_IRQ: i32 = 2;

// ── bump allocator: reset to zero each tick (re-parse is fully transient) ──
const HEAP_SIZE: usize = 16 * 1024 * 1024;
#[global_allocator]
static ALLOC: Bump<HEAP_SIZE> = Bump::new();

fn heap_reset() {
    ALLOC.reset(0);
}

// DSDT buffer: filled once, persists across heap resets (it's not on the heap).
const DSDT_MAX: usize = 512 * 1024;
static DSDT: Single<[u8; DSDT_MAX]> = Single::new([0; DSDT_MAX]);

/// The driver's EC access.
///
/// `read` must return a byte: the interpreter's interface has no "no
/// answer", and a 0 is a plausible lie — the DSDT keeps computing with it
/// and concludes "no battery". So failures are counted and the count is
/// logged; otherwise a silent EC looks like firmware that really reports no
/// battery.
struct HostEc { reads: u32, fails: u32, verbose: bool }
impl Ec for HostEc {
    fn read(&mut self, addr: u8) -> u8 {
        let r = npk_sys::ec_read(addr as i32);
        self.reads += 1;
        let v = if r < 0 { self.fails += 1; 0 } else { r as u8 };
        // Log every byte read, not just the count.
        //
        // "failed=0" only means "no error code", not "meaningful value". An
        // EC returning all zeros looks to the DSDT like a real reading and it
        // concludes "no battery".
        if self.verbose {
            lognum2("[aml]   ec.read  [", addr as u32, "] -> ", v as u32);
        }
        v
    }
    fn write(&mut self, addr: u8, val: u8) {
        npk_sys::ec_write(addr as i32, val as i32);
        if self.verbose {
            lognum2("[aml]   ec.write [", addr as u32, "] <- ", val as u32);
        }
    }
    fn sleep_ms(&mut self, ms: u32) {
        npk_sys::sleep(ms as i32);
    }
    fn now_ms(&mut self) -> Option<u64> {
        Some(npk_sys::ticks().max(0) as u64)
    }
    fn mem_read(&mut self, addr: u64) -> Option<u8> {
        let hi = (addr >> 32) as u32 as i32;
        let lo = (addr & 0xFFFF_FFFF) as u32 as i32;
        let r = npk_sys::acpi_mem_read(hi, lo);
        if r < 0 { None } else { Some(r as u8) }
    }
    fn query(&mut self) -> Option<u8> {
        let q = npk_sys::ec_query();
        if q < 0 { None } else { Some(q as u8) }
    }
    fn note(&mut self, s: &str) {
        if self.verbose { logln(s); }
    }
    fn ec_event(&mut self, q: u8, handled: bool) {
        log("[aml] EC event _Q");
        loghex2(q);
        logln(if handled { " (ran)" } else { " (no handler)" });
    }
    fn notify(&mut self, path: &[[u8; 4]], value: u64) {
        log("[aml]   Notify(\\");
        for (k, seg) in path.iter().enumerate() {
            if k > 0 { log("."); }
            if let Ok(t) = core::str::from_utf8(seg) { log(t); }
        }
        log(", 0x");
        loghex2(value as u8);
        logln(")");
    }
    fn note_num(&mut self, s: &str, v: u64) {
        if self.verbose { lognum(s, v as u32); }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    logln("[aml] battery driver start");
    // Verbose trace of the first round (every region, every EC access,
    // every `_BIF` field). Off in normal operation; `set log.drivers 1`
    // enables it.
    let loud = npk_sys::sys_info(50) == 1;

    let len = DSDT.with(|d| npk_sys::acpi_dsdt(d));
    lognum("[aml] DSDT bytes reported: ", len as u32);
    if len <= 0 || len as usize > DSDT_MAX {
        lognum("[aml] no DSDT, or larger than the buffer of ", DSDT_MAX as u32);
        logln("[aml] driver idle");
        return;
    }
    let table_len = len as usize;

    // Count rounds: if the interpreter hangs, the last printed number shows
    // whether it happened in the first pass or later. The first round is
    // verbose; afterwards only changes of the result are logged, since the
    // driver runs forever.
    //
    // Take the SCI: the EC signals via its GPE, and a hotkey event must be
    // fetched within milliseconds; some ECs stop answering QR_EC (return 0)
    // if events are left pending for seconds. Without SCI the EC is polled
    // every 10 s.
    let sci_vec = {
        heap_reset();
        let gpe = DSDT.with(|d| Namespace::load(&d[..table_len]).ok().and_then(|ns| ec_gpe(&ns)));
        match gpe {
            Some(gpe) => {
                let v = npk_sys::sci_arm(gpe as i32);
                if v > 0 {
                    lognum("[aml] EC events by SCI, GPE ", gpe);
                } else {
                    logln("[aml] SCI not available — EC polled every 10 s");
                }
                v
            }
            None => { logln("[aml] no EC _GPE — EC polled every 10 s"); -1 }
        }
    };

    let mut round = 0u32;
    let mut last = i32::MIN;
    // When the battery is next due (TSC). An SCI alone is no reason to work:
    // the EC raises its GPE after each of its own reads/writes too, so
    // reading the battery on every SCI would keep waking ourselves. Work
    // happens only on SCI_EVT (a real event) or when the battery is due.
    let tsc_per_ms = (npk_sys::sys_info(10) as u64).max(1) * 1000;
    let now = || npk_sys::sys_info(19) as u64;
    let mut battery_due = 0u64;
    let mut work = true;
    loop {
        if !work {
            npk_sys::wait(WAIT_IRQ, 10_000);
            let fired = npk_sys::sci_service();
            if fired > 0 && (fired >> 16) & 0x100 != 0 {
                logln("[aml] power button (PM1 PWRBTN_STS)");
            }
            work = (fired > 0 && fired & 2 != 0) || now() >= battery_due;
            continue;
        }
        work = sci_vec <= 0;
        battery_due = now() + 10_000 * tsc_per_ms;
        round += 1;
        heap_reset();
        let (packed, info) = DSDT.with(|d| decode(&d[..table_len], loud && round == 1));
        if packed != last {
            if packed < 0 {
                // A battery that stops reporting is worth logging.
                logln("[aml] no usable battery (packed=-1)");
            } else if loud {
                // The percentage is not: it changes about a hundred times
                // per discharge, and the bar shows it anyway.
                lognum("[aml] battery percent=", (packed & 0xFF) as u32);
            }
            last = packed;
        }
        npk_sys::battery_report(packed);
        // Raw values for `battery` (whole-system draw); the bar needs only
        // the percentage, a power reading needs more.
        if let Some(i) = info {
            npk_sys::battery_detail(i.rate as i32, i.remaining_mah as i32,
                i.full_charge_mah as i32, i.voltage_mv as i32, i.power_unit as i32);
        }
        if sci_vec <= 0 {
            npk_sys::sleep(10_000);
        }
    }
}

/// Parse the DSDT and evaluate the first present battery; return the bar's
/// packed encoding ((status<<8)|percent) or -1 if none, and the decoded
/// battery when there is one.
fn decode(table: &[u8], verbose: bool) -> (i32, Option<aml_core::BatteryInfo>) {
    let ns = match Namespace::load(table) {
        Ok(ns) => ns,
        Err(_) => { logln("[aml]  Namespace::load failed"); return (-1, None); }
    };
    if verbose { logln("[aml]  namespace loaded, looking for batteries"); }
    let mut ec = HostEc { reads: 0, fails: 0, verbose };
    let mut n = 0u32;
    for bat in find_batteries(&ns) {
        n += 1;
        // Why -1? "method/EC failed" and "battery reports not present" are
        // different failures; `read_battery` carries an error text, so log
        // it.
        match read_battery(&ns, &mut ec, &bat) {
            Err(e) => if verbose {
                log("[aml]  _BST/_BIF failed: ");
                logln(&e);
            },
            Ok(info) => {
            if verbose {
                lognum("[aml]  present=", info.present as u32);
                lognum("[aml]  state=", info.state);
                lognum("[aml]  remaining_mah=", info.remaining_mah);
                lognum("[aml]  full_mah=", info.full_charge_mah);
                lognum("[aml]  percent=", info.percent as u32);
                lognum("[aml]  EC reads=", ec.reads);
                lognum("[aml]  EC failed=", ec.fails);
            }
            // A battery whose remaining capacity the firmware does not state
            // has no percentage; an invented number in the bar is worse than
            // no cell at all.
            if info.present && info.remaining_mah == 0xFFFF_FFFF {
                if verbose {
                    logln("[aml]  battery present, but _BST reports UNKNOWN remaining");
                    logln("[aml]  -> kein Prozentwert; die Bar zeigt nichts statt etwas Falsches");
                }
                return (-1, Some(info));
            }
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
                return ((status << 8) | info.percent as i32, Some(info));
            }
            }
        }
    }
    if verbose { lognum("[aml]  batteries seen: ", n); }
    (-1, None)
}
