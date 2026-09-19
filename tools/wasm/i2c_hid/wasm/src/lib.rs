//! i2c_hid.wasm — das Touchpad, das nicht auf PCI liegt.
//!
//! Stufe 1: **berichten, was die Firmware sagt.** Das Modul holt die DSDT,
//! sucht darin jedes HID-over-I2C-Geraet und schreibt Controller, Adresse,
//! Busfrequenz, Deskriptor-Register und GPIO-Pin ins Log. Ein Zeiger
//! bewegt sich damit noch nicht — aber erst diese Zeilen sagen, WELCHE
//! Register der Bustreiber danach anfassen muss.
//!
//! Der ganze ACPI-Teil liegt in [`i2c_hid_core`] und ist host-seitig gegen
//! eine echte Firmware-Tabelle geprueft (`hp_dsdt_finds_the_touchpad`).

#![no_std]

extern crate alloc;

use aml_core::{Ec, Machine, Namespace};

// Rohzugriff auf Firmware und Hardware: HARDWARE (Bit 0x40). Wer eine
// `.npk.caps`-Sektion schreibt, ERSETZT die Vorgabe und muss READ selbst
// mitnennen, wenn er es behalten will — hier wird keines gebraucht.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x40];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    logln("[i2c-hid] panic");
    // Trappen, nicht drehen: der Kernel faengt es ab und sagt es auf dem
    // Bildschirm. `loop {}` waere ein stiller Haenger.
    core::arch::wasm32::unreachable()
}

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_acpi_dsdt(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_print(ptr: i32, len: i32);
    fn npk_mmio_map_phys(hi: i32, lo: i32, pages: i32) -> i32;
    fn npk_mmio_read32(handle: i32, offset: i32) -> i32;
    fn npk_mmio_write32(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_now_us() -> i64;
    fn npk_acpi_mem_read(hi: i32, lo: i32) -> i32;
}

/// Der Hardwarezugang des Bustreibers.
///
/// Der Treiber rechnet, diese Huelle greift zu — deshalb laeuft derselbe
/// Code im Pruefstand gegen einen Mock.
struct HostBus { handle: i32 }

impl i2c_hid_core::dw_i2c::Bus for HostBus {
    fn read32(&mut self, off: u32) -> u32 {
        unsafe { npk_mmio_read32(self.handle, off as i32) as u32 }
    }
    fn write32(&mut self, off: u32, val: u32) {
        unsafe { npk_mmio_write32(self.handle, off as i32, val as i32) };
    }
    fn now_us(&mut self) -> u64 {
        let t = unsafe { npk_now_us() };
        if t < 0 { 0 } else { t as u64 }
    }
    fn udelay(&mut self, us: u32) {
        // Kein Schlaf unter einer Millisekunde: `npk_sleep` rechnet in ms,
        // und ein I2C-Zyklus dauert 2,5 us. Also gegen die Uhr drehen.
        let end = self.now_us() + us as u64;
        while self.now_us() < end { core::hint::spin_loop(); }
    }
    fn note(&mut self, s: &str) { logln(s); }
}

fn log(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32) };
}
fn logln(s: &str) { log(s); log("\n"); }

// ── Bump-Allokator: der Lauf ist einmalig und ganz voruebergehend ──
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
    unsafe fn dealloc(&self, _p: *mut u8, _l: core::alloc::Layout) {}
}
#[global_allocator]
static ALLOC: Bump = Bump;

const DSDT_MAX: usize = 512 * 1024;
static mut DSDT: [u8; DSDT_MAX] = [0; DSDT_MAX];

/// Der Firmware-Zugang des Interpreters.
///
/// Einen Embedded Controller fragt ein I2C-HID-Geraet nicht — aber
/// SystemMemory schon, und das ist hier der Unterschied zwischen „laeuft"
/// und „laeuft nicht": das `_STA` der I2C-Controller liest ein
/// Konfigurationsbyte aus dem NVS-Fenster der Firmware. Ohne diesen Zugang
/// erfindet der Interpreter dort eine 0, und die Firmware schliesst
/// pflichtgemaess auf „abgeschaltet" — gemessen an Florians IdeaPad, wo
/// beide Controller als absent gemeldet wurden, obwohl beide laufen.
///
/// Dasselbe Loch hatte der Akku-Treiber, und es steht dort seit
/// Kernel 0.365.0 offen. `npk_acpi_mem_read` ist nur LESEND und lehnt
/// jede Adresse in der RAM-Karte ab.
struct FirmwareAccess;
impl Ec for FirmwareAccess {
    fn read(&mut self, _a: u8) -> u8 { 0 }
    fn write(&mut self, _a: u8, _v: u8) {}
    fn mem_read(&mut self, addr: u64) -> Option<u8> {
        let v = unsafe { npk_acpi_mem_read((addr >> 32) as i32, addr as u32 as i32) };
        if v < 0 { None } else { Some(v as u8) }
    }
    fn note(&mut self, s: &str) { logln(s); }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    logln("[i2c-hid] looking for a HID-over-I2C device in the firmware tables");

    let dsdt_ptr = core::ptr::addr_of_mut!(DSDT) as *mut u8;
    let len = unsafe { npk_acpi_dsdt(dsdt_ptr as i32, DSDT_MAX as i32) };
    if len <= 0 || len as usize > DSDT_MAX {
        logln("[i2c-hid] no DSDT, or bigger than our buffer — nothing to do");
        return;
    }
    // SAFETY: der Kernel hat genau `len` Bytes hineingeschrieben.
    let table = unsafe { core::slice::from_raw_parts(dsdt_ptr as *const u8, len as usize) };

    let ns = match Namespace::load(table) {
        Ok(ns) => ns,
        Err(_) => { logln("[i2c-hid] DSDT did not parse"); return; }
    };

    let mut ec = FirmwareAccess;
    let mut m = Machine::new(&ns, &mut ec);
    m.init();

    let found = i2c_hid_core::discover::find(&ns, &mut m);
    if found.is_empty() {
        // Das ist eine ANTWORT, keine Panne: eine Maschine ohne Touchpad
        // (QEMU, die NUC) sagt genau das.
        logln("[i2c-hid] no HID-over-I2C device declared — idle");
        return;
    }
    for d in &found {
        for line in i2c_hid_core::discover::report(d) {
            logln(&line);
        }
        probe_bus(d);
    }
    logln("[i2c-hid] done");
}

/// Den Controller ANFASSEN: abbilden, Kennung lesen, Zaehler rechnen.
///
/// Das ist der erste Schritt, der die Hardware beruehrt — und die Kennung
/// ist die billigste Probe, dass Abbildung und Adresse stimmen. Steht dort
/// `0x44570140` ("DW" + 0x0140), ist der ganze Weg bis hierher richtig:
/// DSDT gelesen, `_CRS` ausgewertet, MMIO abgebildet.
fn probe_bus(d: &i2c_hid_core::discover::HidDevice) {
    use i2c_hid_core::dw_i2c;

    let c = match &d.controller {
        Some(c) if c.mmio_base != 0 && c.present => c,
        Some(c) if !c.present => {
            logln("[i2c-hid]   controller _STA says absent — not touching its registers");
            return;
        }
        _ => { logln("[i2c-hid]   controller has no fixed MMIO — nothing to map"); return; }
    };

    let pages = ((c.mmio_len as usize).max(4096) + 4095) / 4096;
    let handle = unsafe { npk_mmio_map_phys(0, c.mmio_base as i32, pages.min(16) as i32) };
    if handle < 0 {
        logln("[i2c-hid]   MMIO mapping REFUSED — no HARDWARE right, or the range is RAM");
        return;
    }

    let mut bus = HostBus { handle };
    let clk_khz = c.input_clock_hz / 1000;
    match dw_i2c::Dw::setup(&mut bus, d.bus_speed_hz, clk_khz, c.sscn, c.fmcn) {
        Ok(dw) => {
            logln(&alloc::format!("[i2c-hid]   {}", dw.describe()));
            logln("[i2c-hid]   Designware signature OK — the controller is really there");
        }
        Err(dw_i2c::Error::NotDesignware(v)) => {
            logln(&alloc::format!(
                "[i2c-hid]   COMP_TYPE {v:#010x}, expected 0x44570140 — wrong address, \
                 or the block is powered down"));
        }
        Err(e) => logln(&alloc::format!("[i2c-hid]   controller setup failed: {e:?}")),
    }
}
