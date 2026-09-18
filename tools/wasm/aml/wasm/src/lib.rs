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
    logln("[aml] panic");
    // TRAPPEN, nicht drehen. `loop {}` verwandelte jeden Absturz in einen
    // stillen Haenger: die Meldung ging in einen seriellen Port, den ein
    // Notebook nicht hat, und danach drehte die Faser fuer immer. Ein Trap
    // faengt der Kernel ab und druckt "forge: aml endete mit unreachable"
    // ueber kprintln — also auf den Bildschirm.
    core::arch::wasm32::unreachable()
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
    fn npk_print(ptr: i32, len: i32);
}

/// In BEIDE Kanaele.
///
/// `npk_log_serial` geht nur in den UART und den Fernspiegel — auf einem
/// Notebook ohne seriellen Port ist das unsichtbar, und haengt die Maschine,
/// kommt auch der Spiegel nicht mehr heraus. `npk_print` laeuft ueber
/// `kprint!`, also auf den BILDSCHIRM und in den Bootlog-Mitschnitt (und
/// damit in `dmesg`). Deshalb beides: welcher Kanal lebt, weiss man erst
/// hinterher.
/// Ein Kanal, nicht zwei.
///
/// `npk_print` laeuft ueber `kprint!`, also auf den Bildschirm UND in den
/// Bootlog-Mitschnitt (`dmesg`). `npk_log_serial` schreibt daneben in den
/// UART und haengt an JEDEN Aufruf ein `\r\n` — in beide zu schreiben gab
/// jede Zeile doppelt und zerriss sie an jeder Zahl. Ein sichtbarer Kanal
/// genuegt.
fn log(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32) };
}
fn logln(s: &str) { log(s); log("\n"); }

/// `log` mit einer Zahl dahinter — ohne Formatierer, der Allokation braucht.
/// Zwei Zahlen in einer Zeile — fuer `[addr] -> wert`.
fn lognum2(a: &str, x: u32, b: &str, y: u32) {
    log(a);
    lognum_raw(x);
    log(b);
    lognum_raw(y);
    log("\n");
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

/// Der EC-Zugang des Treibers.
///
/// `read` MUSS ein Byte liefern — die Schnittstelle des Interpreters laesst
/// kein "keine Antwort" zu, und eine 0 ist an dieser Stelle eine plausible
/// Luege: die DSDT rechnet damit weiter und schliesst auf "kein Akku".
/// Deshalb wird wenigstens GEZAEHLT, wie oft das passiert, und der Zaehler
/// steht danach im Log. Ohne ihn sieht ein stummer EC genauso aus wie eine
/// Firmware, die wirklich keinen Akku meldet.
struct HostEc { reads: u32, fails: u32, verbose: bool }
impl Ec for HostEc {
    fn read(&mut self, addr: u8) -> u8 {
        let r = unsafe { npk_ec_read(addr as i32) };
        self.reads += 1;
        let v = if r < 0 { self.fails += 1; 0 } else { r as u8 };
        // Jedes gelesene BYTE zeigen, nicht nur zaehlen.
        //
        // "failed=0" heisst nur "kein Fehlercode" — nicht "sinnvoller Wert".
        // Liefert der EC lauter Nullen, sieht das fuer die DSDT aus wie eine
        // echte Messung, und sie schliesst auf "kein Akku". Genau diese zwei
        // Faelle liessen sich bisher nicht trennen.
        if self.verbose {
            lognum2("[aml]   ec.read  [", addr as u32, "] -> ", v as u32);
        }
        v
    }
    fn write(&mut self, addr: u8, val: u8) {
        unsafe { npk_ec_write(addr as i32, val as i32) };
        if self.verbose {
            lognum2("[aml]   ec.write [", addr as u32, "] <- ", val as u32);
        }
    }
    fn sleep_ms(&mut self, ms: u32) {
        unsafe { npk_sleep(ms as i32) };
    }
    fn note(&mut self, s: &str) {
        if self.verbose { logln(s); }
    }
    fn note_num(&mut self, s: &str, v: u64) {
        if self.verbose { lognum(s, v as u32); }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    logln("[aml] battery driver start");

    let dsdt_ptr = core::ptr::addr_of_mut!(DSDT) as *mut u8;
    let len = unsafe { npk_acpi_dsdt(dsdt_ptr as i32, DSDT_MAX as i32) };
    lognum("[aml] DSDT bytes reported: ", len as u32);
    if len <= 0 || len as usize > DSDT_MAX {
        lognum("[aml] no DSDT, or larger than the buffer of ", DSDT_MAX as u32);
        logln("[aml] driver idle");
        return;
    }
    let table_len = len as usize;

    // Die Runde zaehlen. Haengt der Interpreter, sagt die letzte gedruckte
    // Zahl, ob es beim ERSTEN Durchgang passiert oder erst spaeter — und
    // das sind zwei ganz verschiedene Fehler.
    // Die erste Runde erzaehlt, danach nur noch, wenn sich das ERGEBNIS
    // aendert. Eine Wegmarke je Runde ist beim Suchen richtig und im
    // Betrieb eine Flut — der Treiber laeuft fuer immer.
    let mut round = 0u32;
    let mut last = i32::MIN;
    loop {
        round += 1;
        heap_reset();
        let table = unsafe { core::slice::from_raw_parts(dsdt_ptr as *const u8, table_len) };
        let packed = decode(table, round == 1);
        if packed != last {
            if packed < 0 {
                logln("[aml] no usable battery (packed=-1)");
            } else {
                lognum("[aml] battery percent=", (packed & 0xFF) as u32);
            }
            last = packed;
        }
        unsafe { npk_battery_report(packed) };
        unsafe { npk_sleep(10_000) };
    }
}

/// Parse the DSDT and evaluate the first present battery; return the bar's
/// packed encoding ((status<<8)|percent) or -1 if none.
fn decode(table: &[u8], verbose: bool) -> i32 {
    let ns = match Namespace::load(table) {
        Ok(ns) => ns,
        Err(_) => { logln("[aml]  Namespace::load failed"); return -1; }
    };
    if verbose { logln("[aml]  namespace loaded, looking for batteries"); }
    let mut ec = HostEc { reads: 0, fails: 0, verbose };
    let mut n = 0u32;
    for bat in find_batteries(&ns) {
        n += 1;
        // Warum -1? Bisher fielen "Methode/EC hat gemeckert" und "Akku
        // meldet sich als nicht vorhanden" in dieselbe stille -1, und das
        // sind zwei ganz verschiedene Fehler. `read_battery` traegt einen
        // Fehlertext — der wurde weggeworfen.
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
    }
    if verbose { lognum("[aml]  batteries seen: ", n); }
    -1
}
