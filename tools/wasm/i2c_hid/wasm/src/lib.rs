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
use i2c_hid_core::report;

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
    fn npk_sleep(ms: i32) -> i32;
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
        // Ab einer Millisekunde ABGEBEN, nicht drehen.
        //
        // wasmi zaehlt je WASM-Befehl, und `run` gibt einem Modul zehn
        // Milliarden davon. Eine Warteschleife gegen die Uhr verbraucht
        // sie in Sekunden — der erste Lauf mit lebendem Zeiger endete
        // nach zehn Sekunden mit „fuel exhausted". `npk_sleep` gibt an den
        // Scheduler ab und kostet EINEN Befehl.
        //
        // Darunter bleibt das Drehen: `npk_sleep` rechnet in Millisekunden,
        // und ein I2C-Zyklus dauert 2,5 us.
        if us >= 1000 {
            unsafe { npk_sleep((us / 1000) as i32) };
            return;
        }
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
    let mut live: alloc::vec::Vec<Live> = alloc::vec::Vec::new();
    for d in &found {
        for line in i2c_hid_core::discover::report(d) {
            logln(&line);
        }
        if let Some(l) = probe_bus(d) { live.push(l); }
    }
    if live.is_empty() {
        logln("[i2c-hid] no pointer device came up — idle");
        return;
    }
    logln(&alloc::format!("[i2c-hid] {} pointer device(s) live", live.len()));

    // Dauerbetrieb. Ein Treiber kehrt nicht zurueck — er horcht.
    //
    // 5 ms Abstand: ein Touchpad meldet mit etwa 100-200 Hz, und
    // `npk_sleep` gibt dazwischen an den Scheduler ab, kostet also weder
    // Kern noch Treibstoff.
    let mut buf = [0u8; 64];
    let mut rounds = 0u32;
    loop {
        rounds += 1;
        // Nach etwa zwei Sekunden: hat ein Geraet, das wir umgeschaltet
        // haben, ueberhaupt etwas gesagt? Wenn nicht, hat der Schalter es
        // verstummen lassen — dann zurueck in die Maus-Nachahmung, statt
        // auf den naechsten Release zu warten. Ein Treiber, der sich
        // selbst aussperrt, muss sich selbst zurueckholen.
        if rounds == 400 {
            for l in live.iter_mut() {
                if l.seen == 0 {
                    if let Some(rid) = l.switched.take() {
                        logln("[i2c-hid] silent since the mode switch — going back to mouse mode");
                        let (dw, desc, addr) = (
                            i2c_hid_core::dw_i2c::Dw { ..l.dw }, l.desc, l.addr);
                        let _ = i2c_hid_core::hid::set_report(
                            &mut l.bus, &dw, addr, &desc,
                            i2c_hid_core::hid::REPORT_TYPE_FEATURE, rid, &[0]);
                    }
                }
            }
        }
        let mut alive = false;
        for l in live.iter_mut() {
            // Die Leitung LEER holen, nicht einen Bericht je Runde.
            //
            // Ein Bild aus zwei Berichten braucht sonst zwei Runden, und
            // bei 5 ms Abstand liegt das genau auf der Melderate des
            // Geraets — ein Bericht geht verloren, sobald es einmal
            // schneller ist als wir. Acht ist der Deckel, damit ein
            // schwatzendes Geraet die Runde nicht besetzt.
            for _ in 0..8 {
                match poll_live(l, &mut buf) {
                    Step::Data => { alive = true; }
                    Step::Empty => { alive = true; break; }
                    Step::Dead => break,
                }
            }
        }
        if !alive {
            logln("[i2c-hid] all devices stopped answering — giving up");
            return;
        }
        unsafe { npk_sleep(5) };
    }
}

/// Den Controller ANFASSEN: abbilden, Kennung lesen, Zaehler rechnen.
///
/// Das ist der erste Schritt, der die Hardware beruehrt — und die Kennung
/// ist die billigste Probe, dass Abbildung und Adresse stimmen. Steht dort
/// `0x44570140` ("DW" + 0x0140), ist der ganze Weg bis hierher richtig:
/// DSDT gelesen, `_CRS` ausgewertet, MMIO abgebildet.
fn probe_bus(d: &i2c_hid_core::discover::HidDevice) -> Option<Live> {
    use i2c_hid_core::dw_i2c;

    let c = match &d.controller {
        Some(c) if c.mmio_base != 0 => c,
        _ => { logln("[i2c-hid]   controller has no fixed MMIO — nothing to map"); return None; }
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
        return None;
    }

    let mut bus = HostBus { handle };
    let clk_khz = c.input_clock_hz / 1000;
    match dw_i2c::Dw::setup(&mut bus, d.bus_speed_hz, clk_khz, c.sscn, c.fmcn) {
        Ok(dw) => {
            logln(&alloc::format!("[i2c-hid]   {}", dw.describe()));
            logln("[i2c-hid]   Designware signature OK — the controller is really there");
            return talk_to_device(&mut bus, &dw, d);
        }
        Err(dw_i2c::Error::NotDesignware(v)) => {
            logln(&alloc::format!(
                "[i2c-hid]   COMP_TYPE {v:#010x}, expected 0x44570140 — wrong address,  or the block is powered down"));
        }
        Err(e) => logln(&alloc::format!("[i2c-hid]   controller setup failed: {e:?}")),
    }
    None
}

/// Ein eingerichtetes Geraet, aus dem sich Zeigerbewegung lesen laesst.
/// Wie dieses Geraet seine Zeigerdaten meldet.
enum Mode {
    /// Maus-Nachahmung: ein X, ein Y, Tasten, vielleicht ein Rad.
    Mouse {
        fx: report::Field,
        fy: report::Field,
        wheel: Option<report::Field>,
    },
    /// Praezisions-Touchpad: KONTAKTPUNKTE. Je Finger ein Tip-Switch, ein
    /// X und ein Y — daraus entstehen Gesten, die kein Geraet meldet.
    ///
    /// Die KENNUNG (`Contact Identifier`) faehrt mit, und sie ist kein
    /// Beiwerk: liegen zwei Finger auf, muss der Weg aus DEMSELBEN Finger
    /// gerechnet werden. Ohne sie ist der Bezugspunkt der „erste Kontakt
    /// im Bild", und wenn das Geraet die Reihenfolge einmal tauscht,
    /// springt die Strecke um den Fingerabstand.
    Touchpad {
        contacts: alloc::vec::Vec<Contact>,
        count: Option<report::Field>,
    },
}

/// Ein Kontaktplatz im Bericht: liegt er auf, wer ist er, wo ist er.
struct Contact {
    tip: report::Field,
    id: Option<report::Field>,
    x: report::Field,
    y: report::Field,
}

/// Ein Bericht und wie er zu lesen ist.
struct Decoder {
    rid: u8,
    mode: Mode,
    btn: alloc::vec::Vec<report::Field>,
}

/// Ein eingerichtetes Geraet, aus dem sich Zeigerbewegung lesen laesst.
struct Live {
    bus: HostBus,
    dw: i2c_hid_core::dw_i2c::Dw,
    addr: u16,
    desc: i2c_hid_core::hid::HidDesc,
    uses_ids: bool,
    /// **Alle** Berichte, die wir lesen koennen — nicht einer.
    ///
    /// 0.14.0 legte sich auf den Touchpad-Bericht fest und warf jeden
    /// anderen weg. Greift der Umschalter auf den Praezisionsmodus nicht,
    /// sendet das Geraet weiter seinen MAUS-Bericht — und der Zeiger stand
    /// still. Linux verteilt eingehende Berichte nach ihrer NUMMER an den
    /// passenden Decoder, statt eine Nummer zu erwarten; das ist der
    /// Unterschied zwischen „laeuft" und „laeuft, wenn ich richtig
    /// geraten habe".
    decoders: alloc::vec::Vec<Decoder>,
    /// Die ersten paar unbekannten Berichtsnummern melden.
    unknown_logged: u32,
    /// Haben wir auf den Praezisionsmodus umgeschaltet, und in welchem
    /// Feature-Bericht steht der Schalter?
    switched: Option<u8>,
    /// Wieviele Berichte sind bisher gekommen?
    seen: u32,
    /// Wieviele Kontaktlagen wurden schon gemeldet? Die ersten paar
    /// gehoeren ins Log: ob ZWEI Finger ankommen, sagt sonst niemand.
    touch_logged: u32,
    /// Die ersten Berichte ROH. Was das Geraet wirklich schickt, sagt
    /// keine abgeleitete Zahl.
    raw_logged: u32,
    /// Die ersten Rollentscheidungen.
    scroll_logged: u32,

    // ── Aus Orten werden Wege und Gesten ─────────────────────────
    //
    // Das steht in `i2c_hid_core::gesture` und nicht hier, weil genau
    // diese Logik zweimal falsch ausgeliefert wurde und beide Male erst
    // am Geraet auffiel. Dort haengen Tests daran.
    track: i2c_hid_core::gesture::Tracker,
    /// Bezugspunkt fuer eine Maus, die ORTE statt Wege meldet.
    have_ref: bool,
    rx: i32,
    ry: i32,
    /// Die zuletzt gemeldete Tastenlage.
    ///
    /// Ein LOSLASSEN ist ein Ereignis wie ein Druck: wer nur bei
    /// `buttons != 0` einspeist, meldet den Druck und nie das Ende — und
    /// der Compositor haelt die Taste fuer immer fuer gedrueckt.
    last_buttons: i32,
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
) -> Option<Live> {
    use i2c_hid_core::{dw_i2c, hid, report};

    let addr = d.slave_address;
    let desc_reg = match d.descriptor_address {
        Some(r) => r,
        None => { logln("[i2c-hid]   no descriptor register — cannot talk to it"); return None; }
    };

    dw_i2c::init_master(bus, dw);
    logln("[i2c-hid]   master initialised");

    match hid::probe_address(bus, dw, addr) {
        Ok(()) => logln(&alloc::format!("[i2c-hid]   device at {addr:#04x} answers")),
        Err(e) => {
            logln(&alloc::format!("[i2c-hid]   device at {addr:#04x} does not answer: {e:?}"));
            return None;
        }
    }

    let desc = match hid::fetch_descriptor(bus, dw, addr, desc_reg) {
        Ok(x) => x,
        Err(e) => { logln(&alloc::format!("[i2c-hid]   {e}")); return None; }
    };
    logln(&alloc::format!("[i2c-hid]   {}", desc.describe()));

    match hid::reset(bus, dw, addr, &desc) {
        Ok(()) => logln("[i2c-hid]   power on + reset done"),
        Err(e) => { logln(&alloc::format!("[i2c-hid]   {e}")); return None; }
    }


    // Den REPORT-DESKRIPTOR holen und AUSWERTEN. Was ein Byte im Bericht
    // bedeutet, steht dort und nirgends sonst — ohne ihn gilt ein Treiber
    // fuer genau ein Modell.
    let n = desc.report_desc_length as usize;
    if n == 0 || n > 4096 {
        logln("[i2c-hid]   no usable report descriptor length");
        return None;
    }
    let mut rd = alloc::vec![0u8; n];
    if let Err(e) = hid::read_register(bus, dw, addr, desc.report_desc_register, &mut rd) {
        logln(&alloc::format!("[i2c-hid]   report descriptor read failed: {e:?}"));
        return None;
    }
    let map = report::parse(&rd);
    logln(&alloc::format!("[i2c-hid]   {}", map.describe()));

    // Den Deskriptor ROH ins Log, wenn er klein genug ist.
    //
    // Mein Parser findet auf diesem Geraet EINEN Kontaktplatz, wo ein
    // Praezisions-Touchpad fuenf deklariert. Das laesst sich nicht
    // erraten — es steht in diesen Bytes, und sie sind die Grundwahrheit,
    // nicht meine Auslegung davon. 381 Bytes sind 16 Zeilen; die 893 der
    // Wacom bleiben draussen.
    if n <= 512 {
        logln(&alloc::format!("[i2c-hid]   raw report descriptor, {n} bytes:"));
        for (i, chunk) in rd.chunks(24).enumerate() {
            logln(&alloc::format!("[i2c-hid]   rd {:03x} {:02x?}", i * 24, chunk));
        }
    }

    // Wenn das Geraet einen „Device Mode" fuehrt, auf 3 stellen.
    //
    // Ein Praezisions-Touchpad startet in der MAUS-Nachahmung: ein X, ein
    // Y, Tasten — und keine Kontaktpunkte. Zweifinger-Scrollen ist in
    // diesem Zustand nicht schwer, sondern unmoeglich, weil der zweite
    // Finger gar nicht gemeldet wird. Der Schalter steht in einem
    // Feature-Bericht (Digitizer 0x52).
    let mut switched: Option<u8> = None;
    if let Some(im) = map.find_feature(report::PAGE_DIGITIZER, report::USAGE_INPUT_MODE) {
        let im = *im;
        match hid::set_report(bus, dw, addr, &desc, hid::REPORT_TYPE_FEATURE, im.report_id, &[3]) {
            Ok(()) => {
                logln(&alloc::format!(
                    "[i2c-hid]   device mode -> 3 (precision touchpad), feature report {}",
                    im.report_id));
                switched = Some(im.report_id);
            }
            Err(e) => logln(&alloc::format!(
                "[i2c-hid]   device mode switch failed: {e:?} — staying in mouse mode")),
        }
    }

    // JEDEN Bericht einrichten, den wir lesen koennen — Touchpad und
    // Maus. Welcher kommt, entscheidet das Geraet, nicht wir.
    let mut decoders: alloc::vec::Vec<Decoder> = alloc::vec::Vec::new();
    let mut scroll_step = 1i32;

    if let Some(id) = map.touchpad_report() {
        let tips = map.find_all(report::Kind::Input, id, report::PAGE_DIGITIZER, report::USAGE_TIP_SWITCH);
        let xs = map.find_all(report::Kind::Input, id, report::PAGE_GENERIC_DESKTOP, report::USAGE_X);
        let ys = map.find_all(report::Kind::Input, id, report::PAGE_GENERIC_DESKTOP, report::USAGE_Y);
        let ids = map.find_all(report::Kind::Input, id, report::PAGE_DIGITIZER, report::USAGE_CONTACT_ID);
        let n = tips.len().min(xs.len()).min(ys.len());
        if n > 0 {
            let contacts: alloc::vec::Vec<_> = (0..n)
                .map(|i| Contact {
                    tip: *tips[i],
                    id: ids.get(i).map(|f| **f),
                    x: *xs[i],
                    y: *ys[i],
                })
                .collect();
            // Ein Scrollschritt aus dem logischen Bereich: etwa ein
            // Vierzigstel der Padhoehe je Raste. Geraeteunabhaengig, weil
            // die Zahl aus dem Geraet selbst kommt.
            let span = (ys[0].logical_max - ys[0].logical_min).max(1);
            let step = (span / 40).max(1);
            logln(&alloc::format!(
                "[i2c-hid]   report {id}: touchpad, {n} contact slot(s), scroll step {step}, ids {}",
                if ids.is_empty() { "no" } else { "yes" }));
            scroll_step = step;
            decoders.push(Decoder {
                rid: id,
                mode: Mode::Touchpad {
                    contacts,
                    count: map.find(id, report::PAGE_DIGITIZER, report::USAGE_CONTACT_COUNT).copied(),
                },
                btn: (1u16..=3).filter_map(|u| map.find(id, report::PAGE_BUTTON, u).copied()).collect(),
            });
        }
    }

    for id in map.report_ids() {
        if decoders.iter().any(|d| d.rid == id) { continue; }
        let (Some(fx), Some(fy)) = (
            map.find(id, report::PAGE_GENERIC_DESKTOP, report::USAGE_X),
            map.find(id, report::PAGE_GENERIC_DESKTOP, report::USAGE_Y),
        ) else { continue };
        let (fx, fy) = (*fx, *fy);
        let wheel = map.find(id, report::PAGE_GENERIC_DESKTOP, report::USAGE_WHEEL).copied();
        logln(&alloc::format!(
            "[i2c-hid]   report {id}: mouse ({}), wheel {}",
            if fx.relative { "relative" } else { "absolute" },
            if wheel.is_some() { "yes" } else { "no" }));
        decoders.push(Decoder {
            rid: id,
            mode: Mode::Mouse { fx, fy, wheel },
            btn: (1u16..=3).filter_map(|u| map.find(id, report::PAGE_BUTTON, u).copied()).collect(),
        });
    }

    if decoders.is_empty() {
        logln("[i2c-hid]   no report carries X and Y — not a pointer");
        return None;
    }
    logln(&alloc::format!("[i2c-hid]   {} decoder(s), ready", decoders.len()));

    Some(Live {
        bus: HostBus { handle: bus.handle },
        dw: i2c_hid_core::dw_i2c::Dw { ..*dw },
        addr, desc,
        uses_ids: map.uses_ids,
        decoders,
        unknown_logged: 0,
        switched,
        seen: 0,
        touch_logged: 0,
        raw_logged: 0, scroll_logged: 0,
        track: i2c_hid_core::gesture::Tracker::new(scroll_step),
        have_ref: false, rx: 0, ry: 0,
        last_buttons: 0,
    })
}

/// Einen Eingabebericht abholen und als Zeiger oder Geste einspeisen.
///
/// Der Unterschied, um den sich alles dreht: eine Maus meldet WEGE, ein
/// Touchpad ORTE. Aus Orten wird ein Weg, indem man den vorigen abzieht —
/// und die ERSTE Beruehrung liefert keinen, sonst spraenge der Zeiger
/// dorthin, wo der Finger aufsetzt.
///
/// Und eine GESTE meldet niemand. „Zwei Finger wandern parallel" steht in
/// keinem Bericht; es entsteht erst hier, aus der Zahl der aufliegenden
/// Kontaktpunkte und ihrer Bewegung. Unter Linux macht das libinput.
/// Was ein einzelner Leseversuch ergeben hat.
enum Step {
    /// Ein Bericht kam — es kann sofort noch einer dahinter liegen.
    Data,
    /// Nichts da. Das Geraet lebt, hat aber gerade nichts zu sagen.
    Empty,
    /// Der Bus antwortet nicht mehr.
    Dead,
}

fn poll_live(l: &mut Live, buf: &mut [u8]) -> Step {
    use i2c_hid_core::{hid, report};
    let r = match hid::get_input(&mut l.bus, &l.dw, l.addr, &l.desc, buf) {
        Ok(Some(r)) => r,
        Ok(None) => return Step::Empty,
        Err(_) => return Step::Dead,
    };
    let (id, data) = if l.uses_ids && !r.is_empty() { (r[0], &r[1..]) } else { (0u8, r) };

    // Den Decoder zu DIESER Nummer nehmen. Kennt ihn keiner, einmal
    // sagen, welche Nummer kam — das ist die Auskunft, die fehlt, wenn
    // sich nichts bewegt.
    let Some(d) = l.decoders.iter().find(|d| d.rid == id) else {
        if l.unknown_logged < 3 {
            l.unknown_logged += 1;
            logln(&alloc::format!(
                "[i2c-hid]   report id {id} arrived, {} bytes — no decoder for it", r.len()));
        }
        return Step::Data;
    };
    let (mode, btn) = (&d.mode, &d.btn);
    l.seen += 1;

    if l.raw_logged < 16 {
        l.raw_logged += 1;
        logln(&alloc::format!("[i2c-hid]   in {id}: {:02x?}", data));
    }

    let mut buttons = 0i32;
    for (i, f) in btn.iter().enumerate() {
        if report::extract(data, f) != 0 { buttons |= 1 << i; }
    }

    let (dx, dy, scroll) = match mode {
        Mode::Mouse { fx, fy, wheel } => {
            let x = report::extract(data, fx);
            let y = report::extract(data, fy);
            // Das Rad meldet immer RELATIV — Rasten, keine Position.
            let s = wheel.as_ref().map(|w| report::extract(data, w)).unwrap_or(0);
            if fx.relative {
                (x, y, s)
            } else {
                let d = if l.have_ref { (x - l.rx, y - l.ry) } else { (0, 0) };
                l.have_ref = true;
                l.rx = x; l.ry = y;
                (d.0, d.1, s)
            }
        }
        Mode::Touchpad { contacts, count } => {
            use i2c_hid_core::gesture;
            let cc = count.as_ref().map(|c| report::extract(data, c)).unwrap_or(-1);
            let mut present = [(0i32, 0i32, 0i32); 8];
            let mut np = 0usize;
            for (i, c) in contacts.iter().enumerate() {
                if report::extract(data, &c.tip) != 0 && np < present.len() {
                    // Ohne Kennungsfeld ist der PLATZ die Kennung.
                    let cid = c.id.as_ref()
                        .map(|f| report::extract(data, f))
                        .unwrap_or(i as i32);
                    present[np] =
                        (cid, report::extract(data, &c.x), report::extract(data, &c.y));
                    np += 1;
                }
            }
            match l.track.feed(cc, &present[..np], contacts.len()) {
                gesture::Out::Pending => (0, 0, 0),
                gesture::Out::Frame { n, gesture, dx, dy, scroll } => {
                    if n > 0 && l.touch_logged < 8 {
                        l.touch_logged += 1;
                        logln(&alloc::format!(
                            "[i2c-hid]   frame: {n} finger(s), contact-count {cc}, \
                             gesture {gesture}, {:?}",
                            &l.track.frame()[..n.min(2)]));
                    }
                    if scroll != 0 && l.scroll_logged < 8 {
                        l.scroll_logged += 1;
                        logln(&alloc::format!("[i2c-hid]   scroll: {scroll} click(s)"));
                    }
                    (dx, dy, scroll)
                }
            }
        }
    };

    if dx != 0 || dy != 0 || scroll != 0 || buttons != l.last_buttons {
        unsafe { npk_pointer_inject(dx, dy, buttons, scroll) };
    }
    l.last_buttons = buttons;
    Step::Data
}
