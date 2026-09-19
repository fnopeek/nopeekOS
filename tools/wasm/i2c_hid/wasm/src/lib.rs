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
    fn npk_acpi_table(sig: i32, index: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_pointer_inject(dx: i32, dy: i32, buttons: i32, scroll: i32) -> i32;
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

/// "SSDT", wie die vier Zeichen im Speicher stehen (little-endian).
const SIG_SSDT: i32 = i32::from_le_bytes(*b"SSDT");
/// ACPICA laedt ausser SSDT auch PSDT und OSDT in den Namespace
/// (`acpi_tb_load_namespace`). Selten, aber es kostet nichts.
const SIG_PSDT: i32 = i32::from_le_bytes(*b"PSDT");
const SIG_OSDT: i32 = i32::from_le_bytes(*b"OSDT");

const DSDT_MAX: usize = 512 * 1024;
static mut DSDT: [u8; DSDT_MAX] = [0; DSDT_MAX];
/// Platz fuer EINE SSDT auf einmal — der Namespace kopiert heraus, was er
/// braucht, also darf der Puffer danach wieder benutzt werden.
const SSDT_MAX: usize = 256 * 1024;
static mut SSDT: [u8; SSDT_MAX] = [0; SSDT_MAX];

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

    let mut ns = match Namespace::load(table) {
        Ok(ns) => ns,
        Err(_) => { logln("[i2c-hid] DSDT did not parse"); return; }
    };

    // Und JEDE SSDT dazu.
    //
    // Eine Firmware verteilt ihre Deklarationen ueber die DSDT und
    // beliebig viele SSDTs; sie bilden EINEN Namespace, und Linux laedt
    // sie alle. Wer nur die DSDT liest, dem fehlen Namen, die woanders
    // stehen — hier die Basis der Region mit den Freigabebits der
    // I2C-Controller.
    let ssdt_ptr = core::ptr::addr_of_mut!(SSDT) as *mut u8;
    let mut loaded = 0u32;
    for (sig, name) in [(SIG_SSDT, "SSDT"), (SIG_PSDT, "PSDT"), (SIG_OSDT, "OSDT")] {
    for i in 0..32 {
        let n = unsafe { npk_acpi_table(sig, i, ssdt_ptr as i32, SSDT_MAX as i32) };
        if n <= 0 { break; }
        let _ = name;
        if n as usize > SSDT_MAX {
            logln(&alloc::format!("[i2c-hid] SSDT {i} is {n} bytes — bigger than our buffer"));
            continue;
        }
        // SAFETY: der Kernel hat genau `n` Bytes hineingeschrieben.
        let t = unsafe { core::slice::from_raw_parts(ssdt_ptr as *const u8, n as usize) };
        match ns.load_more(t) {
            Ok(()) => loaded += 1,
            Err(e) => logln(&alloc::format!("[i2c-hid] {name} {i} did not parse: {e}")),
        }
    }
    }
    logln(&alloc::format!("[i2c-hid] namespace: DSDT + {loaded} more table(s)"));

    let mut ec = FirmwareAccess;
    // Bedingte Deklarationen auf Scope-Ebene aufloesen — ACPICA FUEHRT die
    // Termliste beim Laden aus, ein `If` dort ist eine Verzweigung. Daran
    // haengt auf diesem Geraet `FRTB`, die Basis der Region mit den
    // Freigabebits der I2C-Controller.
    let (seen, taken) = ns.resolve_conditionals(&mut ec);
    logln(&alloc::format!(
        "[i2c-hid] scope-level conditionals: {seen} seen, {taken} taken"));

    let mut m = Machine::new(&ns, &mut ec);
    m.init();

    let found = i2c_hid_core::discover::find(&ns, &mut m);
    // Sagt ein `_STA` null, noch einmal MIT SPUR: woraus ist die Null
    // entstanden? Nur fuer die Controller, und nur im Zweifelsfall — die
    // Spur ist laut.
    for d in &found {
        if let Some(c) = &d.controller {
            if !c.present {
                logln(&alloc::format!(
                    "[i2c-hid] why is {} absent? tracing its _STA:",
                    aml_core::path_str(&c.path)));
                let mut p = c.path.clone();
                p.push(aml_core::seg("_STA"));
                match m.call_traced(&p, alloc::vec::Vec::new()) {
                    Ok(v) => logln(&alloc::format!("[i2c-hid]   _STA returned {:#x}", v.as_int())),
                    Err(e) => logln(&alloc::format!("[i2c-hid]   _STA failed: {e}")),
                }
            }
        }
    }
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
        Some(c) if c.mmio_base != 0 => c,
        _ => { logln("[i2c-hid]   controller has no fixed MMIO — nothing to map"); return; }
    };

    // `_STA` sperrt hier NICHT mehr, es warnt nur.
    //
    // Unser `_STA` wird auf einem Interpreter gerechnet, der zugegebene
    // Loecher hat: Operationsregionen ohne hinterlegten Speicher liefern
    // eine erfundene 0, und im Log stehen Lesungen an Adressen wie 0x6 —
    // das ist ein Fenster, dessen Basis nie berechnet wurde. Eine so
    // zustande gekommene 0 ueber einen direkten Hardware-Lesezugriff zu
    // stellen, ist die falsche Reihenfolge der Beweise.
    //
    // Die Grenze liegt deshalb zwischen LESEN und SCHREIBEN: die Kennung
    // holen darf man immer (ein Registerlesen im FCH-Bereich antwortet
    // schlimmstenfalls mit lauter Einsen), einrichten erst, wenn sie
    // stimmt.
    if !c.present {
        logln("[i2c-hid]   _STA says absent — reading the signature anyway,                writes only if it checks out");
    }

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
            talk_to_device(&mut bus, &dw, d);
        }
        Err(dw_i2c::Error::NotDesignware(v)) => {
            logln(&alloc::format!(
                "[i2c-hid]   COMP_TYPE {v:#010x}, expected 0x44570140 — wrong address,  or the block is powered down"));
        }
        Err(e) => logln(&alloc::format!("[i2c-hid]   controller setup failed: {e:?}")),
    }
}

/// Mit dem GERAET reden: Bus einrichten, Adresse antippen, HID-Deskriptor
/// holen, aufwecken und zuruecksetzen.
///
/// Ab hier wird GESCHRIEBEN. Die Rechtfertigung ist der Registerwert, den
/// der Controller gerade selbst geliefert hat — nicht eine Firmware-Flagge,
/// die wir aus einem Namen errechnen, den wir nirgends finden.
fn talk_to_device(
    bus: &mut HostBus,
    dw: &i2c_hid_core::dw_i2c::Dw,
    d: &i2c_hid_core::discover::HidDevice,
) {
    use i2c_hid_core::{dw_i2c, hid, report};
    use i2c_hid_core::dw_i2c::Bus as _;

    let addr = d.slave_address;
    let desc_reg = match d.descriptor_address {
        Some(r) => r,
        None => { logln("[i2c-hid]   no descriptor register — cannot talk to it"); return; }
    };

    dw_i2c::init_master(bus, dw);
    logln("[i2c-hid]   master initialised");

    match hid::probe_address(bus, dw, addr) {
        Ok(()) => logln(&alloc::format!("[i2c-hid]   device at {addr:#04x} answers")),
        Err(e) => {
            logln(&alloc::format!("[i2c-hid]   device at {addr:#04x} does not answer: {e:?}"));
            return;
        }
    }

    let desc = match hid::fetch_descriptor(bus, dw, addr, desc_reg) {
        Ok(x) => x,
        Err(e) => { logln(&alloc::format!("[i2c-hid]   {e}")); return; }
    };
    logln(&alloc::format!("[i2c-hid]   {}", desc.describe()));

    match hid::reset(bus, dw, addr, &desc) {
        Ok(()) => logln("[i2c-hid]   power on + reset done"),
        Err(e) => { logln(&alloc::format!("[i2c-hid]   {e}")); return; }
    }

    // Den REPORT-DESKRIPTOR holen und AUSWERTEN. Was ein Byte im Bericht
    // bedeutet, steht dort und nirgends sonst — ohne ihn gilt ein Treiber
    // fuer genau ein Modell.
    let n = desc.report_desc_length as usize;
    if n == 0 || n > 4096 {
        logln("[i2c-hid]   no usable report descriptor length");
        return;
    }
    let mut rd = alloc::vec![0u8; n];
    if let Err(e) = hid::read_register(bus, dw, addr, desc.report_desc_register, &mut rd) {
        logln(&alloc::format!("[i2c-hid]   report descriptor read failed: {e:?}"));
        return;
    }
    let map = report::parse(&rd);
    logln(&alloc::format!("[i2c-hid]   {}", map.describe()));

    let Some(rid) = map.pointer_report() else {
        logln("[i2c-hid]   no report carries X and Y — not a pointer");
        return;
    };
    let fx = *map.find(rid, report::PAGE_GENERIC_DESKTOP, report::USAGE_X).unwrap();
    let fy = *map.find(rid, report::PAGE_GENERIC_DESKTOP, report::USAGE_Y).unwrap();
    let tip = map.find(rid, report::PAGE_DIGITIZER, report::USAGE_TIP_SWITCH).copied();
    let btn: alloc::vec::Vec<report::Field> = (1u16..=3)
        .filter_map(|u| map.find(rid, report::PAGE_BUTTON, u).copied())
        .collect();

    logln("[i2c-hid]   POINTER LIVE — move your finger for 10 s");

    // Absolut gegen relativ: eine Maus meldet Wege, ein Touchpad Orte.
    // Aus Orten wird ein Weg, indem man den vorigen abzieht — und die
    // ERSTE Beruehrung liefert keinen, sonst spraenge der Zeiger dorthin,
    // wo der Finger aufsetzt.
    let mut buf = [0u8; 64];
    let mut have_ref = false;
    let (mut rx, mut ry) = (0i32, 0i32);
    let mut got = 0u32;
    let mut moved = 0u32;

    for _ in 0..2000 {
        match hid::get_input(bus, dw, addr, &desc, &mut buf) {
            Ok(Some(r)) => {
                // Das erste Byte ist die Report-ID, wenn der Deskriptor
                // welche benutzt; die Feldversaetze zaehlen ohne sie.
                let (id, data) = if map.uses_ids && !r.is_empty() {
                    (r[0], &r[1..])
                } else {
                    (0u8, r)
                };
                if id != rid { continue; }
                got += 1;

                let x = report::extract(data, &fx);
                let y = report::extract(data, &fy);
                let touching = match &tip {
                    Some(t) => report::extract(data, t) != 0,
                    None => true,
                };
                let mut buttons = 0i32;
                for (i, f) in btn.iter().enumerate() {
                    if report::extract(data, f) != 0 { buttons |= 1 << i; }
                }

                let (dx, dy) = if fx.relative {
                    (x, y)
                } else if !touching {
                    have_ref = false;
                    (0, 0)
                } else if have_ref {
                    (x - rx, y - ry)
                } else {
                    have_ref = true;
                    (0, 0)
                };
                if !fx.relative { rx = x; ry = y; }

                if dx != 0 || dy != 0 || buttons != 0 {
                    moved += 1;
                    unsafe { npk_pointer_inject(dx, dy, buttons, 0) };
                }
                if moved <= 3 && (dx != 0 || dy != 0) {
                    logln(&alloc::format!(
                        "[i2c-hid]   report id {id}: x={x} y={y} -> dx={dx} dy={dy} btn={buttons}"));
                }
            }
            Ok(None) => {}
            Err(e) => {
                logln(&alloc::format!("[i2c-hid]   input read failed: {e:?}"));
                break;
            }
        }
        bus.udelay(5_000);
    }
    logln(&alloc::format!(
        "[i2c-hid]   {got} report(s), {moved} with movement"));
}
