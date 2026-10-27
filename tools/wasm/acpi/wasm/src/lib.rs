//! acpi.wasm — the ACPI driver: runs the firmware's AML (battery, EC events, processor idle states).
//!
//! Resident background driver (autostart). At startup it loads the firmware
//! DSDT into a namespace and initialises it once (`_REG`, `_INI`, then the
//! EC's `_REG`), as ACPICA and Linux do. From then on it runs the `_Qxx` of
//! each EC event the SCI reports, and every 10 s the battery's own
//! `_BST`/`_BIF` AML (vendor-independent, via [`aml_core`]), and pushes the
//! decoded percentage to the kernel via `npk_battery_report`; status items
//! read it through `npk_battery()`.

#![no_std]

extern crate alloc;

use aml_core::{ec_gpe, find_batteries, Ec, Machine, Namespace};

// The driver needs raw firmware + EC access — declare the HARDWARE capability
// (bit 0x40) so the kernel grants exactly that and nothing else.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x40];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    logln("[acpi] panic");
    // Trap, do not spin. `loop {}` would turn every panic into a silent
    // hang; a trap is caught by the kernel and reported on screen.
    core::arch::wasm32::unreachable()
}

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

// The namespace lives as long as the driver; what a method evaluation
// allocates goes back.
#[global_allocator]
static ALLOC: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

/// The buffer an SSDT and the MADT are read into, one at a time.
const TABLE_MAX: usize = 256 * 1024;
static TABLE: Single<[u8; TABLE_MAX]> = Single::new([0; TABLE_MAX]);

/// The processors as Linux sets them up for idle: `_OSC` with the OS's
/// capabilities on each present one (`acpi_early_processor_osc`), `_PDC`
/// for all when that fails (`acpi_early_processor_set_pdc`), then the
/// first `_CST` that reads (`intel_idle_acpi_cst_extract`) to the kernel.
fn processor_power(m: &mut Machine, ns: &Namespace) {
    use aml_core::processor;
    let caps = npk_sys::acpi_proc_caps();
    if caps < 0 {
        return;
    }
    let madt = TABLE.with(|t| {
        let n = npk_sys::acpi_table(*b"APIC", 0, t);
        (n > 0 && n as usize <= TABLE_MAX).then(|| t[..n as usize].to_vec())
    });
    let Some(madt) = madt else {
        logln("[acpi] processors: no MADT");
        return;
    };
    let present: alloc::vec::Vec<processor::Processor> = processor::processors(m, ns).into_iter()
        .filter(|p| processor::physically_present(&madt, p)).collect();
    if present.is_empty() {
        return;
    }
    let mut osc_ok = true;
    for p in &present {
        if let Err(e) = processor::run_osc(m, &p.path, caps as u32) {
            log("[acpi]  ");
            logln(&e);
            osc_ok = false;
            break;
        }
    }
    if !osc_ok {
        for p in &present {
            let _ = processor::set_pdc(m, &p.path, caps as u32);
        }
    }
    let mut cst = None;
    for p in &present {
        match processor::evaluate_cst(m, &p.path) {
            Ok((states, skipped)) => {
                for e in skipped {
                    log("[acpi]  ");
                    logln(&e);
                }
                if !states.is_empty() {
                    cst = Some(states);
                    break;
                }
            }
            Err(e) => if VERBOSE.with(|v| *v) {
                log("[acpi]  ");
                logln(&e);
            },
        }
    }
    log("[acpi] processors: ");
    lognum_raw(present.len() as u32);
    log(if osc_ok { " present, _OSC" } else { " present, _PDC" });
    let Some(states) = cst else {
        logln(", no _CST");
        return;
    };
    log(", _CST ");
    lognum_raw(states.len() as u32);
    logln(" state(s)");
    let mut buf = alloc::vec::Vec::with_capacity(states.len() * 24);
    for c in &states {
        buf.extend_from_slice(&c.kind.to_le_bytes());
        buf.extend_from_slice(&c.latency.to_le_bytes());
        buf.extend_from_slice(&c.power.to_le_bytes());
        buf.extend_from_slice(&[c.space_id, c.bit_offset, c.access_size, 0]);
        buf.extend_from_slice(&c.address.to_le_bytes());
    }
    npk_sys::acpi_cst(&buf);
}

// DSDT buffer: filled once.
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

/// The verbose trace (`set log.drivers 1`) is on for the start and the first
/// battery round.
static VERBOSE: Single<bool> = Single::new(false);

/// EC events fetched (`QR_EC`) the moment the SCI reported them: this EC
/// drops SCI_EVT again while other transactions run, and an event asked for
/// only afterwards is lost. Linux too queries at once
/// (`acpi_ec_submit_query`).
struct Pending { q: [u8; 64], n: usize, next: usize }
/// The EC event numbers already logged, one bit each.
static SEEN_EVENTS: Single<[u64; 4]> = Single::new([0; 4]);
static PENDING: Single<Pending> = Single::new(Pending { q: [0; 64], n: 0, next: 0 });

/// Fetch every event the EC has now.
fn fetch_ec_events() {
    PENDING.with(|p| {
        while p.n < p.q.len() {
            let q = npk_sys::ec_query();
            if q <= 0 { break; }
            p.q[p.n] = q as u8;
            p.n += 1;
        }
    });
}

impl HostEc {
    fn loud(&self) -> bool {
        self.verbose || VERBOSE.with(|v| *v)
    }
}

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
        if self.loud() {
            lognum2("[acpi]   ec.read  [", addr as u32, "] -> ", v as u32);
        }
        v
    }
    fn write(&mut self, addr: u8, val: u8) {
        npk_sys::ec_write(addr as i32, val as i32);
        if self.loud() {
            lognum2("[acpi]   ec.write [", addr as u32, "] <- ", val as u32);
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
        let fetched = PENDING.with(|p| {
            if p.next < p.n {
                p.next += 1;
                return Some(p.q[p.next - 1]);
            }
            p.n = 0;
            p.next = 0;
            None
        });
        if fetched.is_some() {
            return fetched;
        }
        let q = npk_sys::ec_query();
        if q <= 0 { None } else { Some(q as u8) }
    }
    fn note(&mut self, s: &str) {
        if self.loud() { logln(s); }
    }
    fn ec_event(&mut self, q: u8, handled: bool) {
        // Each event number once (a new hotkey shows up this way); some
        // fire every second, and Linux logs none of them.
        let first = SEEN_EVENTS.with(|s| {
            let bit = 1u64 << (q % 64);
            let fresh = s[(q / 64) as usize] & bit == 0;
            s[(q / 64) as usize] |= bit;
            fresh
        });
        if !first && !self.loud() {
            return;
        }
        log("[acpi] EC event _Q");
        loghex2(q);
        logln(match (handled, first) {
            (true, true) => " (ran; further ones of it silent)",
            (true, false) => " (ran)",
            (false, _) => " (no handler)",
        });
    }
    fn notify(&mut self, path: &[[u8; 4]], value: u64) {
        if !self.loud() {
            return;
        }
        log("[acpi]   Notify(\\");
        for (k, seg) in path.iter().enumerate() {
            if k > 0 { log("."); }
            if let Ok(t) = core::str::from_utf8(seg) { log(t); }
        }
        log(", 0x");
        loghex2(value as u8);
        logln(")");
    }
    fn video_output_notify(&mut self, value: u64) {
        npk_sys::acpi_video_notify(value as i32);
    }
    fn note_num(&mut self, s: &str, v: u64) {
        if self.loud() { lognum(s, v as u32); }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    logln("[acpi] battery driver start");
    // Verbose trace of the first round (every region, every EC access,
    // every `_BIF` field). Off in normal operation; `set log.drivers 1`
    // enables it.
    let loud = npk_sys::sys_info(50) == 1;

    let len = DSDT.with(|d| npk_sys::acpi_dsdt(d));
    lognum("[acpi] DSDT bytes reported: ", len as u32);
    if len <= 0 || len as usize > DSDT_MAX {
        lognum("[acpi] no DSDT, or larger than the buffer of ", DSDT_MAX as u32);
        logln("[acpi] driver idle");
        return;
    }
    let table_len = len as usize;
    VERBOSE.with(|v| *v = loud);

    let Some(mut ns) = DSDT.with(|d| Namespace::load(&d[..table_len]).ok()) else {
        logln("[acpi] Namespace::load failed — driver idle");
        return;
    };
    // Plus every SSDT: together they form one namespace, and Linux loads
    // them all (the processors' _OSC lives in one).
    let mut ssdts = 0u32;
    for i in 0..64 {
        let n = TABLE.with(|t| {
            let n = npk_sys::acpi_table(*b"SSDT", i, t);
            if n > 0 && n as usize <= TABLE_MAX && ns.load_more(&t[..n as usize]).is_ok() {
                ssdts += 1;
            }
            n
        });
        if n <= 0 { break; }
    }
    lognum("[acpi] namespace: DSDT + SSDTs ", ssdts);
    let ns: &'static Namespace = alloc::boxed::Box::leak(alloc::boxed::Box::new(ns));
    let batteries = find_batteries(ns);

    // Take the SCI: the EC signals via its GPE, and a hotkey event must be
    // fetched within milliseconds; some ECs stop answering QR_EC (return 0)
    // if events are left pending for seconds. Without SCI the EC is polled
    // every 10 s.
    let sci_vec = match ec_gpe(ns) {
        Some(gpe) => {
            let v = npk_sys::sci_arm(gpe as i32);
            if v > 0 {
                lognum("[acpi] EC events by SCI, GPE ", gpe);
            } else {
                logln("[acpi] SCI not available — EC polled every 10 s");
            }
            v
        }
        None => { logln("[acpi] no EC _GPE — EC polled every 10 s"); -1 }
    };

    let ec: &'static mut HostEc = alloc::boxed::Box::leak(alloc::boxed::Box::new(HostEc { reads: 0, fails: 0, verbose: false }));
    let mut m = Machine::new(ns, ec);
    // The order of acpi_bus_init: objects, the processors' _OSC, the EC.
    m.init_objects();
    processor_power(&mut m, ns);
    m.init_ec();
    // acpi_ec_clear: what the EC collected before anyone listened.
    m.drain_events();

    let mut last = i32::MIN;
    // When the battery is next due (TSC). An SCI alone is no reason to work:
    // the EC raises its GPE after each of its own reads/writes too, so
    // reading the battery on every SCI would keep waking ourselves. Events
    // run only on SCI_EVT, the battery only when due.
    let tsc_per_ms = (npk_sys::sys_info(10) as u64).max(1) * 1000;
    let now = || npk_sys::sys_info(19) as u64;
    let mut battery_due = 0u64;
    loop {
        if now() >= battery_due {
            battery_due = now() + 10_000 * tsc_per_ms;
            if sci_vec <= 0 {
                m.drain_events();
            }
            let (packed, info) = battery(&mut m, &batteries);
            VERBOSE.with(|v| *v = false);
            if packed != last {
                if packed < 0 {
                    // A battery that stops reporting is worth logging.
                    logln("[acpi] no usable battery (packed=-1)");
                } else if loud {
                    // The percentage is not: it changes about a hundred
                    // times per discharge, and the bar shows it anyway.
                    lognum("[acpi] battery percent=", (packed & 0xFF) as u32);
                }
                last = packed;
            }
            npk_sys::battery_report(packed);
            // Raw values for `battery` (whole-system draw); the bar needs
            // only the percentage, a power reading needs more.
            if let Some(i) = info {
                npk_sys::battery_detail(i.rate as i32, i.remaining_mah as i32,
                    i.full_charge_mah as i32, i.voltage_mv as i32, i.power_unit as i32);
            }
        }
        if sci_vec <= 0 {
            npk_sys::sleep(10_000);
            continue;
        }
        npk_sys::wait(WAIT_IRQ, 10_000);
        let fired = npk_sys::sci_service();
        if fired > 0 && (fired >> 16) & 0x100 != 0 {
            logln("[acpi] power button (PM1 PWRBTN_STS)");
        }
        if fired > 0 && fired & 2 != 0 {
            fetch_ec_events();
            m.drain_events();
        }
    }
}

/// The first present battery: the bar's packed encoding ((status<<8)|percent)
/// or -1 if none, and the decoded battery when there is one.
fn battery(m: &mut Machine, batteries: &[aml_core::Path]) -> (i32, Option<aml_core::BatteryInfo>) {
    let verbose = VERBOSE.with(|v| *v);
    for bat in batteries {
        // Why -1? "method/EC failed" and "battery reports not present" are
        // different failures; the error carries a text, so log it.
        match m.battery(bat) {
            Err(e) => if verbose {
                log("[acpi]  _BST/_BIF failed: ");
                logln(&e);
            },
            Ok(info) => {
            if verbose {
                lognum("[acpi]  present=", info.present as u32);
                lognum("[acpi]  state=", info.state);
                lognum("[acpi]  remaining_mah=", info.remaining_mah);
                lognum("[acpi]  full_mah=", info.full_charge_mah);
                lognum("[acpi]  percent=", info.percent as u32);
            }
            // A battery whose remaining capacity the firmware does not state
            // has no percentage; an invented number in the bar is worse than
            // no cell at all.
            if info.present && info.remaining_mah == 0xFFFF_FFFF {
                if verbose {
                    logln("[acpi]  battery present, but _BST reports UNKNOWN remaining");
                    logln("[acpi]  -> kein Prozentwert; die Bar zeigt nichts statt etwas Falsches");
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
    if verbose { lognum("[acpi]  batteries seen: ", batteries.len() as u32); }
    (-1, None)
}
