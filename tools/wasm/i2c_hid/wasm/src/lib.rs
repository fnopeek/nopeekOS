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
//!
//! Gelesen wird nur, wenn der Interrupt-Pin sagt, dass etwas anliegt
//! ([`Gate`]) — blind zu lesen kostete 1,4 ms Busarbeit je Versuch und
//! damit einen halben Kern im Leerlauf.

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
    fn npk_pointer_inject(dx: i32, dy: i32, buttons: i32, scroll: i32, hscroll: i32) -> i32;
    fn npk_sleep(ms: i32) -> i32;
    fn npk_irq_register_gsi(gsi: i32, flags: i32) -> i32;
    fn npk_wait(mask: i32, timeout_ms: i32) -> i32;
    fn npk_sys_info(key: i32) -> i64;
}

// ── Diagnosezeilen: gebaut, aber im Normalbetrieb still ──────────────
//
// Jeder Fund an diesem Treiber haengt an einer dieser Zeilen — der rohe
// Deskriptor, die ersten Berichte, die Zehn-Sekunden-Buchfuehrung. Sie
// gehoeren deshalb nicht geloescht, sondern geschaltet. EINMAL beim Start
// gefragt (`npk_sys_info(50)` = Konfigwert `log.drivers`), danach kostet
// es einen Vergleich.
//
// Was NICHT hier haengt: was der Treiber ENTSCHEIDET. Welches Geraet
// gefunden wurde, ob der Praezisionsmodus griff, woran das Tor haengt und
// jeder Fehler — das steht immer im Log, sonst ist ein Geraetelauf ohne
// Aussage.
static mut VERBOSE: bool = false;

fn verbose() -> bool {
    // SAFETY: ein Faden, ein Lauf.
    unsafe { core::ptr::addr_of!(VERBOSE).read() }
}

/// Wie `logln`, aber nur wenn `set log.drivers 1` gesetzt ist.
fn dbgln(s: &str) {
    if verbose() { logln(s); }
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
    fn note(&mut self, s: &str) { dbgln(s); }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // SAFETY: ein Faden, ein Lauf — einmal gesetzt, danach nur gelesen.
    unsafe {
        core::ptr::addr_of_mut!(VERBOSE).write(npk_sys_info(50) == 1);
    }
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
                dbgln(&alloc::format!(
                    "[i2c-hid] why is {} absent? tracing its _STA:",
                    aml_core::path_str(&c.path)));
                let mut p = c.path.clone();
                p.push(aml_core::seg("_STA"));
                match m.call_traced(&p, alloc::vec::Vec::new()) {
                    Ok(v) => dbgln(&alloc::format!("[i2c-hid]   _STA returned {:#x}", v.as_int())),
                    Err(e) => dbgln(&alloc::format!("[i2c-hid]   _STA failed: {e}")),
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
    let mut irq = arm_irq(&found, &live);

    // Dauerbetrieb. Ein Treiber kehrt nicht zurueck — er horcht.
    //
    // 5 ms Abstand: ein Touchpad meldet mit etwa 100-200 Hz, und
    // `npk_sleep` gibt dazwischen an den Scheduler ab, kostet also weder
    // Kern noch Treibstoff.
    let mut buf = [0u8; 64];
    // Drei Minuten Buchfuehrung, dann Ruhe.
    let mut stat_lines_left = 18u32;
    let mut next_stat_us = { let t = unsafe { npk_now_us() }; if t < 0 { 0 } else { t as u64 } }
        + 10_000_000;
    loop {
        // Hat der Umschalter in den Praezisionsmodus gegriffen?
        //
        // NICHT ueber die Zeit. 0.15.0 fragte nach zwei Sekunden „hat das
        // Geraet etwas gesagt?" und schaltete sonst zurueck — und ein
        // Touchpad, das niemand beruehrt, sagt NICHTS. Der Wachhund lief
        // also jedesmal, bevor der erste Finger aufsetzte, und nahm den
        // Modus wieder weg. Genau deshalb kam am Geraet nur Bericht 1.
        //
        // Die Frage, die sich beantworten laesst, ist eine andere: kommen
        // Berichte, aber NIE der des Touchpads? Dann hat der Schalter
        // nicht gegriffen. Schweigen beweist gar nichts und darf deshalb
        // auch nichts ausloesen.
        for l in live.iter_mut() {
            if l.touch_rid.is_some() && !l.saw_touch && l.other_seen >= 64 {
                if let Some(rid) = l.switched.take() {
                    logln(&alloc::format!(
                        "[i2c-hid] {:#04x}: 64 reports and none is the touchpad one — \
                         the mode switch did not take, back to mouse mode", l.addr));
                    let (dw, desc, addr) = (
                        i2c_hid_core::dw_i2c::Dw { ..l.dw }, l.desc, l.addr);
                    let zero = alloc::vec![0u8; l.mode_len];
                    let _ = i2c_hid_core::hid::set_report(
                        &mut l.bus, &dw, addr, &desc,
                        i2c_hid_core::hid::REPORT_TYPE_FEATURE, rid, &zero);
                }
            }
        }
        let mut alive = false;
        for l in live.iter_mut() {
            // Erst den PIN fragen, dann den Bus anfassen.
            //
            // Ein Leseversuch holt `wMaxInputLength` Bytes — bis zu 64,
            // bei 400 kHz also 1,4 ms auf dem Bus. Der Pin kostet ein
            // Register.
            let (poll_now, gate_said_no) = gate_check(l);
            if !poll_now {
                // Wer nicht gefragt wurde, kann nicht schweigen — es gilt
                // der letzte echte Befund.
                l.skips += 1;
                if !l.dead { alive = true; }
                continue;
            }
            let mut got = false;
            let mut answered = false;
            // Die Leitung LEER holen, nicht einen Bericht je Runde.
            //
            // Ein Bild aus zwei Berichten braucht sonst zwei Runden, und
            // bei 5 ms Abstand liegt das genau auf der Melderate des
            // Geraets — ein Bericht geht verloren, sobald es einmal
            // schneller ist als wir. Acht ist der Deckel, damit ein
            // schwatzendes Geraet die Runde nicht besetzt.
            for i in 0..8 {
                l.polls += 1;
                match poll_live(l, &mut buf) {
                    Step::Data => {
                        answered = true;
                        got = true;
                        l.datas += 1;
                        if i == 7 { l.capped += 1; }
                        // Der Pegel steht, bis der Bericht geholt ist —
                        // ist er weg, liegt nichts mehr an. Die leere
                        // Nachlese waere sonst eine ganze Uebertragung.
                        if !gate_asserted_now(l) { break; }
                    }
                    Step::Empty => { answered = true; l.empties += 1; break; }
                    Step::Junk => { answered = true; l.junk += 1; break; }
                    Step::Dead => { l.errs += 1; break; }
                }
            }
            l.dead = !answered;
            if answered { alive = true; }
            gate_verdict(l, gate_said_no, got);
        }
        if !alive {
            logln("[i2c-hid] all devices stopped answering — giving up");
            return;
        }

        // Alle zehn Sekunden sagen, was die Runde wirklich gekostet hat —
        // und dann von selbst aufhoeren. Sonst bleibt ein Treiber, der
        // einen Kern frisst, eine Ratesache.
        let now = { let t = unsafe { npk_now_us() }; if t < 0 { 0 } else { t as u64 } };

        // Das Antippen drueckt sofort und laesst SPAETER los — sonst
        // koennte daraus nie ein Ziehen werden. Der Tritt dafuer steht
        // HIER und nicht im Berichtspfad: ein Touchpad, das niemand mehr
        // beruehrt, schickt keinen Bericht, und die Taste bliebe unten.
        // Genau dieser Fehler ist 0.13.0 ausgeliefert worden.
        for l in live.iter_mut() {
            l.track.tick(now / 1000);
            push_buttons(l);
        }

        if stat_lines_left > 0 && now >= next_stat_us {
            next_stat_us = now + 10_000_000;
            stat_lines_left -= 1;
            for l in live.iter_mut() {
                dbgln(&alloc::format!(
                    "[i2c-hid] {:#04x}: 10 s — {} read(s): {} data, {} empty, \
                     {} undeclared, {} FAILED · {} rounds skipped by the pin · \
                     {} drain caps",
                    l.addr, l.polls, l.datas, l.empties, l.junk, l.errs,
                    l.skips, l.capped));
                l.polls = 0; l.datas = 0; l.empties = 0; l.junk = 0;
                l.errs = 0; l.skips = 0; l.capped = 0;
            }
        }
        match &mut irq {
            Some(q) => {
                // `do_amd_gpio_irq_handler`: every pending pin acknowledged
                // (writing back what was read clears its status bits), then
                // the EOI to the GPIO unit. The level line the kernel masked
                // is released when we wait again.
                use i2c_hid_core::gpio;
                // ALL pending pins of the block, as `do_amd_gpio_irq_handler`
                // does — the line is shared by every pin. 0.28.0 acked only
                // ours; a pin the firmware enabled (lid, hotkeys, EC) with
                // its status standing kept the level line up for good, and
                // the driver span. Ours are acknowledged; any other pending
                // pin is not an interrupt anybody here handles, so it is
                // masked — Linux: "Disabling spurious GPIO IRQ".
                let rd = |o: u32| unsafe { npk_mmio_read32(q.handle, o as i32) } as u32;
                let wr = |o: u32, v: u32| unsafe { npk_mmio_write32(q.handle, o as i32, v as i32) };
                let status = ((rd(q.block_off + gpio::WAKE_INT_STATUS_REG1) as u64) << 32
                    | rd(q.block_off + gpio::WAKE_INT_STATUS_REG0) as u64)
                    & ((1u64 << 46) - 1);
                for bit in 0..46u32 {
                    if status & (1u64 << bit) == 0 { continue; }
                    for i in 0..4u32 {
                        let off = q.block_off + (bit * 4 + i) * 4;
                        let v = rd(off);
                        if v & gpio::PIN_IRQ_PENDING == 0 || v & gpio::INTERRUPT_MASK == 0 {
                            continue;
                        }
                        if q.pins.contains(&off) {
                            wr(off, v);
                        } else {
                            wr(off, v & !gpio::INTERRUPT_MASK);
                            if q.spurious_logged < 8 {
                                logln(&alloc::format!(
                                    "[i2c-hid] interrupt: GPIO pin {} pending but nobody's — masked",
                                    bit * 4 + i));
                            }
                            q.spurious_logged += 1;
                        }
                    }
                }
                let mr = q.block_off + gpio::WAKE_INT_MASTER_REG;
                let m = unsafe { npk_mmio_read32(q.handle, mr as i32) } as u32;
                unsafe { npk_mmio_write32(q.handle, mr as i32, (m | gpio::EOI_MASK) as i32) };

                // **Sleep until the pad reports** (docs/plan/CORES_AND_EVENTS.md).
                // It was every 5 ms — 200 wakes a second on an untouched pad.
                // Awake early only for our own timers: an open tap releases
                // its button after TAP_MS, and the first minutes of stats.
                // At most a second: a lost interrupt shows as lag, not as a
                // dead pointer.
                let now = { let t = unsafe { npk_now_us() }; if t < 0 { 0 } else { t as u64 } };
                let now_ms = now / 1000;
                let mut wait_ms: u64 = 1000;
                for l in live.iter() {
                    if let Some(d) = l.track.next_deadline() {
                        wait_ms = wait_ms.min(d.saturating_sub(now_ms).max(1));
                    }
                }
                if stat_lines_left > 0 {
                    wait_ms = wait_ms.min((next_stat_us.saturating_sub(now) / 1000).max(1));
                }
                const WAIT_IRQ: i32 = 2;
                unsafe { npk_wait(WAIT_IRQ, wait_ms as i32) };
            }
            None => unsafe { let _ = npk_sleep(5); },
        }
    }
}

/// The GPIO controller's interrupt, set up for every live pad.
struct IrqMode {
    handle: i32,
    /// Offset of the GPIO block inside the mapped page.
    block_off: u32,
    /// Each pad's pin register (page offset).
    pins: alloc::vec::Vec<u32>,
    /// How many foreign pending pins were masked (first few are logged).
    spurious_logged: u32,
}

/// Wait on the GPIO controller's interrupt instead of polling — or say why
/// not. Needs every live pad gated on a pin of the SAME AMD block, and that
/// block's own line in its `_CRS`. Pin setup as `amd_gpio_irq_set_type`
/// (level, polarity, clear status, the enable-and-wait-for-debounce dance)
/// followed by `amd_gpio_irq_enable` (enable + unmask).
fn arm_irq(found: &[i2c_hid_core::discover::HidDevice], live: &[Live]) -> Option<IrqMode> {
    use i2c_hid_core::gpio;
    let mut handle = -1;
    let mut pins = alloc::vec::Vec::new();
    let mut lows = alloc::vec::Vec::new();
    for l in live {
        let Gate::Pin { handle: h, reg_off, active_low, .. } = &l.gate else {
            logln("[i2c-hid] interrupt: a pad reads blind — staying on the 5 ms poll");
            return None;
        };
        if handle >= 0 && *h != handle {
            logln("[i2c-hid] interrupt: pads on different GPIO blocks — staying on the 5 ms poll");
            return None;
        }
        handle = *h;
        pins.push(*reg_off);
        lows.push(*active_low);
    }
    let g = found.iter().find_map(|d| d.gpio_controller.as_ref())?;
    let Some((gsi, flags)) = g.irq else {
        logln("[i2c-hid] interrupt: the GPIO block names no line in its _CRS — staying on the 5 ms poll");
        return None;
    };
    let level = flags & 0x02 == 0;
    let low = flags & 0x04 != 0;
    let v = unsafe { npk_irq_register_gsi(gsi as i32, (level as i32) | ((low as i32) << 1)) };
    if v < 0 {
        logln(&alloc::format!(
            "[i2c-hid] interrupt: GSI {gsi} refused (taken, or no I/O APIC) — staying on the 5 ms poll"));
        return None;
    }
    for (&off, &active_low) in pins.iter().zip(lows.iter()) {
        let o = off as i32;
        let cfg = gpio::irq_level_config(unsafe { npk_mmio_read32(handle, o) } as u32, active_low);
        // Enable while still masked, wait for the enable bit to read back
        // (the debounce settles), then write the plain configuration.
        unsafe { npk_mmio_write32(handle, o, ((cfg | gpio::INTERRUPT_ENABLE) & !gpio::INTERRUPT_MASK) as i32) };
        for _ in 0..100_000 {
            if unsafe { npk_mmio_read32(handle, o) } as u32 & gpio::INTERRUPT_ENABLE != 0 { break; }
        }
        unsafe { npk_mmio_write32(handle, o, cfg as i32) };
        // `amd_gpio_irq_enable`.
        let r = unsafe { npk_mmio_read32(handle, o) } as u32;
        unsafe { npk_mmio_write32(handle, o, (r | gpio::INTERRUPT_ENABLE | gpio::INTERRUPT_MASK) as i32) };
    }
    let block_off = g.mmio_base & 0xFFF;
    let mr = (block_off + gpio::WAKE_INT_MASTER_REG) as i32;
    let m = unsafe { npk_mmio_read32(handle, mr) } as u32;
    unsafe { npk_mmio_write32(handle, mr, (m | gpio::EOI_MASK) as i32) };
    logln(&alloc::format!(
        "[i2c-hid] interrupt: GSI {gsi} ({}, active-{}) on vector {v} — the pads wake the driver",
        if level { "level" } else { "edge" }, if low { "low" } else { "high" }));
    Some(IrqMode { handle, block_off, pins, spurious_logged: 0 })
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
        logln("[i2c-hid]   _STA says absent — reading the signature anyway, \
               writes only if it checks out");
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
            dbgln(&alloc::format!("[i2c-hid]   {}", dw.describe()));
            dbgln("[i2c-hid]   Designware signature OK — the controller is really there");
            let mut live = talk_to_device(&mut bus, &dw, d)?;
            live.gate = arm_gate(d);
            return Some(live);
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
    /// Wie lang dieser Feature-Bericht ist — die Ruecknahme muss dieselbe
    /// Laenge haben wie das Setzen, sonst wird auch sie verworfen.
    mode_len: usize,
    /// Wieviele Berichte sind bisher gekommen?
    seen: u32,
    /// Die Nummer des Touchpad-Berichts, falls es einen gibt.
    touch_rid: Option<u8>,
    /// Ist er je gekommen? Das ist der BEWEIS, dass der Umschalter griff.
    saw_touch: bool,
    /// Wieviele Berichte kamen, die NICHT der des Touchpads sind?
    other_seen: u32,
    /// Wieviele Kontaktlagen wurden schon gemeldet? Die ersten paar
    /// gehoeren ins Log: ob ZWEI Finger ankommen, sagt sonst niemand.
    touch_logged: u32,
    /// Die ersten Berichte ROH. Was das Geraet wirklich schickt, sagt
    /// keine abgeleitete Zahl.
    raw_logged: u32,
    /// Die ersten Rollentscheidungen.
    scroll_logged: u32,
    /// Die ersten Antipper.
    tap_logged: u32,

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
    /// Die PHYSISCHEN Tasten aus dem letzten Bericht, ohne das, was ein
    /// Antippen gerade haelt.
    ///
    /// Beides getrennt zu fuehren ist noetig, weil sie zu verschiedenen
    /// Zeiten kommen: die physische Lage steht im Bericht, die gehaltene
    /// laeuft an einem Zeitgeber ab — und der tickt auch dann, wenn das
    /// Geraet schweigt.
    hw_buttons: i32,

    /// Fragen wir den Pin, bevor wir den Bus anfassen?
    gate: Gate,
    // ── Was diese zehn Sekunden gekostet haben ───────────────────
    //
    // Ein Treiber, der 90 % eines Kerns frisst und nichts sagt, laesst
    // nur raten. Diese Zeilen sind die Zahlen dazu, und sie hoeren von
    // selbst wieder auf.
    polls: u32,
    datas: u32,
    empties: u32,
    junk: u32,
    errs: u32,
    skips: u32,
    capped: u32,
    /// Die ersten paar Fehlschlaege MIT Grund. Ein stiller Fehlschlag ist
    /// der teuerste Zustand ueberhaupt: xfer wartet bis zu einer Sekunde.
    err_logged: u32,
    /// Hat dieses Geraet beim letzten ECHTEN Leseversuch geschwiegen?
    ///
    /// Eine uebersprungene Runde ist kein Schweigen — es wurde gar nicht
    /// gefragt. Ohne diesen Merker haette das Tor die Notbremse
    /// ausgehebelt: ein toter Bus saehe aus wie ein ruhiges Touchpad.
    dead: bool,
}

/// Der Pin, der sagt, ob ueberhaupt ein Bericht anliegt.
///
/// **Warum es das gibt.** Ein Leseversuch ist nicht billig: geholt wird
/// `wMaxInputLength`, bei uns bis zu 64 Bytes (der Puffer deckelt dort),
/// und bei 400 kHz sind das 1,4 ms, in denen der Kern auf dem Bus wartet.
/// Zweihundertmal je Sekunde, fuer zwei Geraete. Das ist der Grund, warum
/// dieses Modul im Leerlauf einen halben Kern verbraucht hat.
///
/// Linux liest deshalb NIE blind: `i2c_hid_get_input` haengt dort
/// ausschliesslich an `i2c_hid_irq`. Wir haben keinen Interrupt, aber der
/// Pegel steht an, bis der Bericht geholt ist — also laesst er sich
/// abfragen, und das kostet ein Register statt einer Uebertragung.
enum Gate {
    /// Kein Pin, kein bekannter Block, oder er hat sich als falsch
    /// erwiesen: lesen wie bisher.
    Blind,
    /// Der Pin steht und wird gefragt.
    Pin {
        handle: i32,
        reg_off: u32,
        active_low: bool,
        /// Wieviele Runden hintereinander sagte er „nichts da"?
        skipped: u32,
        /// Wie oft kam trotzdem ein Bericht, als er „nichts da" sagte?
        contradictions: u32,
        /// Hat er je RICHTIG einen Bericht angesagt? Steht einmal im Log.
        proved: bool,
    },
}

/// Gegenprobe: so viele uebersprungene Runden, dann wird trotzdem gelesen.
///
/// **Schweigen beweist nichts** — ein ruhendes Touchpad sagt nichts, und
/// ein Pin, der immer „nichts da" meldet, sieht genauso aus. Was etwas
/// beweist, ist der umgekehrte Fall: ein Bericht, der ankommt, OBWOHL der
/// Pin nein sagte. Alle 100 ms wird deshalb blind gelesen, und drei solche
/// Widersprueche hintereinander schalten das Tor dauerhaft ab.
const GATE_CROSS_CHECK_ROUNDS: u32 = 20;
/// Hat der Pin einen Bericht einmal richtig ANGESAGT, taugt er — dann
/// reicht ein Herzschlag je Sekunde, und die Gegenprobe kostet nichts mehr.
const GATE_CROSS_CHECK_PROVED: u32 = 200;
const GATE_MAX_CONTRADICTIONS: u32 = 3;

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
    dbgln("[i2c-hid]   master initialised");

    match hid::probe_address(bus, dw, addr) {
        Ok(()) => dbgln(&alloc::format!("[i2c-hid]   device at {addr:#04x} answers")),
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
        Ok(()) => dbgln("[i2c-hid]   power on + reset done"),
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
    dbgln(&alloc::format!("[i2c-hid]   {}", map.describe()));

    // Den Deskriptor ROH ins Log, wenn er klein genug ist.
    //
    // Mein Parser findet auf diesem Geraet EINEN Kontaktplatz, wo ein
    // Praezisions-Touchpad fuenf deklariert. Das laesst sich nicht
    // erraten — es steht in diesen Bytes, und sie sind die Grundwahrheit,
    // nicht meine Auslegung davon. 381 Bytes sind 16 Zeilen; die 893 der
    // Wacom bleiben draussen.
    if n <= 512 {
        dbgln(&alloc::format!("[i2c-hid]   raw report descriptor, {n} bytes:"));
        for (i, chunk) in rd.chunks(24).enumerate() {
            dbgln(&alloc::format!("[i2c-hid]   rd {:03x} {:02x?}", i * 24, chunk));
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
    let mut mode_len: usize = 1;
    if let Some(im) = map.find_feature(report::PAGE_DIGITIZER, report::USAGE_INPUT_MODE) {
        let im = *im;
        // Die Laenge kommt aus dem DESKRIPTOR, nicht aus dem Bauch.
        //
        // Florians Elan fuehrt `Input Mode` mit `Report Size 16` — der
        // Feature-Bericht ist ZWEI Bytes lang. Wir schickten eines. Auf
        // dem Bus quittiert das Geraet, der Bericht ist aber zu kurz und
        // wird verworfen: kein Fehler, keine Wirkung, und danach kommt
        // ewig nur die Maus-Nachahmung. Fuellbits zaehlen mit, deshalb
        // rechnet `report_bytes` und nicht die Summe der Felder.
        let n = map.report_bytes(report::Kind::Feature, im.report_id).max(1);
        mode_len = n;
        let mut payload = alloc::vec![0u8; n];
        report::insert(&mut payload, &im, 3);
        match hid::set_report(bus, dw, addr, &desc, hid::REPORT_TYPE_FEATURE, im.report_id, &payload) {
            Ok(()) => {
                logln(&alloc::format!(
                    "[i2c-hid]   device mode -> 3 (precision touchpad), feature report {} \
                     ({n} byte(s): {payload:02x?})",
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
    let mut hscroll_step = 1i32;
    let mut tap_move = 1i32;
    let mut pin_move = 1i32;

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
            // Soweit darf ein Finger wandern und es bleibt ein Tippen:
            // rund ein Achtzigstel der Padbreite, also etwa 1,3 mm —
            // derselbe Wert, den libinput nimmt. Aus dem Geraet
            // hergeleitet, nicht in Pixeln geraten.
            tap_move = ((xs[0].logical_max - xs[0].logical_min).max(1) / 80).max(1);
            // Und soweit darf er unter einer GEDRUECKTEN Taste wandern,
            // bevor der Zeiger ihm wieder folgt: doppelt so weit, also
            // rund 2,6 mm. Wer durchdrueckt, verformt die Fingerkuppe,
            // und ihr wandernder Schwerpunkt ist keine Zeigerbewegung.
            // Aus unserem eigenen Tippmass hergeleitet und nicht aus
            // einer Millimeterzahl geraten.
            pin_move = (tap_move * 2).max(1);
            // Die quere Raste kommt aus der BREITE, nicht aus der Hoehe:
            // das Pad ist breiter als hoch, und ein Schritt aus der Hoehe
            // liefe quer zu fein.
            hscroll_step = (((xs[0].logical_max - xs[0].logical_min).max(1)) / 40).max(1);
            logln(&alloc::format!(
                "[i2c-hid]   report {id}: touchpad, {n} contact slot(s), \
                 scroll step {step}/{hscroll_step}, tap move {tap_move}, \
                 pin move {pin_move}, ids {}",
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
        touch_rid: decoders.iter()
            .find(|d| matches!(d.mode, Mode::Touchpad { .. }))
            .map(|d| d.rid),
        decoders,
        unknown_logged: 0,
        switched,
        mode_len,
        seen: 0,
        saw_touch: false,
        other_seen: 0,
        touch_logged: 0,
        raw_logged: 0, scroll_logged: 0, tap_logged: 0,
        track: i2c_hid_core::gesture::Tracker::new(scroll_step, hscroll_step, tap_move, pin_move),
        have_ref: false, rx: 0, ry: 0,
        last_buttons: 0, hw_buttons: 0,
        gate: Gate::Blind,
        dead: false,
        polls: 0, datas: 0, empties: 0, junk: 0, errs: 0, skips: 0, capped: 0,
        err_logged: 0,
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
    /// Ein Bericht kam, und wir konnten ihn lesen — es kann sofort noch
    /// einer dahinter liegen.
    Data,
    /// Nichts da. Das Geraet lebt, hat aber gerade nichts zu sagen.
    Empty,
    /// **Etwas kam, aber es ist kein Bericht.**
    ///
    /// Eine Nummer, die der Deskriptor des Geraets SELBST nicht fuehrt.
    /// Florians Wacom antwortet auf eine Lesung ohne anliegende Daten mit
    /// ID 255 — seine eigenen sind 28, 19, 20, 11, 16, 31, 1 —, der Elan
    /// mit ID 0, statt mit Laenge 0, wie HID over I2C es vorsieht. Genau
    /// diese Frage stellt `docs/plan/INPUT_I2C_HID.md` seit je: was sagt
    /// ein Geraet, wenn man es ohne Grund anspricht.
    ///
    /// Das ist KEINE Information. Es darf deshalb weder die Drainschleife
    /// weiterlaufen lassen (acht Uebertragungen je Runde, fuer nichts)
    /// noch als Beweis GEGEN den Interrupt-Pin zaehlen — und genau das
    /// hat es in 0.23/0.24 getan: drei solche Antworten haben ein
    /// funktionierendes Tor abgeschaltet.
    Junk,
    /// Der Bus antwortet nicht mehr.
    Dead,
}

// ── Das Tor am Interrupt-Pin ─────────────────────────────────────────
//
// Eine Abbildung je GPIO-Block, nicht je Geraet: beide Geraete dieses
// Notebooks haengen am selben, und `MAX_MMIO_MAPS` ist vier.
const MAX_GPIO_MAPS: usize = 2;
static mut GPIO_MAPS: [(u32, u32, i32); MAX_GPIO_MAPS] = [(0, 0, -1); MAX_GPIO_MAPS];
static mut GPIO_MAP_N: usize = 0;

fn map_gpio_page(base: u32, pages: u32) -> i32 {
    // SAFETY: ein Faden, ein Lauf — das Modul hat keine Nebenlaeufigkeit.
    let maps = unsafe { &mut *core::ptr::addr_of_mut!(GPIO_MAPS) };
    let n = unsafe { core::ptr::addr_of!(GPIO_MAP_N).read() };
    for (b, p, h) in maps.iter().take(n) {
        if *b == base && *p >= pages { return *h; }
    }
    let h = unsafe { npk_mmio_map_phys(0, base as i32, pages as i32) };
    if h >= 0 && n < MAX_GPIO_MAPS {
        maps[n] = (base, pages, h);
        unsafe { core::ptr::addr_of_mut!(GPIO_MAP_N).write(n + 1) };
    }
    h
}

/// Das Tor scharf machen — oder begruenden, warum nicht.
///
/// Jede Absage steht im Log. Ein Treiber, der still blind pollt, sieht
/// genauso aus wie einer, der es nicht tut.
fn arm_gate(d: &i2c_hid_core::discover::HidDevice) -> Gate {
    use i2c_hid_core::gpio;

    let addr = d.slave_address;
    let Some(&pin) = d.gpio_pins.first() else {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: no GpioInt in _CRS — reading blind every 5 ms"));
        return Gate::Blind;
    };
    let Some(g) = d.gpio_controller.as_ref() else {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: GpioInt names \"{}\", which is not in the namespace — \
             reading blind", d.gpio_source));
        return Gate::Blind;
    };
    // Der Registeraufbau ist AMD-eigen. Ein fremder Block an derselben
    // Stelle fuehrt etwas anderes, und ein geratenes Bit 16 waere
    // schlimmer als gar keine Abfrage.
    if !gpio::is_amd_block(&g.ids) {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: GPIO block [{}] is not one whose registers we know — \
             reading blind", g.ids.join(",")));
        return Gate::Blind;
    }
    // Eine FLANKE laesst sich nicht abfragen: im Augenblick des Hinsehens
    // ist sie vorbei. Nur ein Pegel steht an, bis der Bericht geholt ist.
    if !d.gpio_level_triggered() {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: GpioInt is edge-triggered — a level is what can be \
             polled, reading blind"));
        return Gate::Blind;
    }
    let Some(w) = gpio::pin_window(g.mmio_base, g.mmio_len, pin) else {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: pin {pin} lies outside {:#010x}+{:#x} — reading blind",
            g.mmio_base, g.mmio_len));
        return Gate::Blind;
    };
    let handle = map_gpio_page(w.map_base, w.pages);
    if handle < 0 {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: GPIO MMIO {:#010x} not mappable — reading blind",
            w.map_base));
        return Gate::Blind;
    }
    let active_low = d.gpio_active_low();
    let v = unsafe { npk_mmio_read32(handle, w.reg_off as i32) } as u32;
    if v == u32::MAX {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: pin register reads all ones — nobody answered there, \
             reading blind"));
        return Gate::Blind;
    }
    logln(&alloc::format!(
        "[i2c-hid] {addr:#04x}: gating on GPIO pin {pin} ({:#010x}+{:#x}, active-{}, \
         now {}) — the bus is only touched when it says so",
        w.map_base, w.reg_off,
        if active_low { "low" } else { "high" },
        if gpio::asserted(v, active_low) { "asserted" } else { "idle" }));
    Gate::Pin { handle, reg_off: w.reg_off, active_low, skipped: 0, contradictions: 0, proved: false }
}

/// Liegt gerade etwas an? Ohne Tor lautet die Antwort immer ja.
fn gate_asserted_now(l: &mut Live) -> bool {
    match &l.gate {
        Gate::Blind => true,
        Gate::Pin { handle, reg_off, active_low, .. } => {
            let v = unsafe { npk_mmio_read32(*handle, *reg_off as i32) } as u32;
            i2c_hid_core::gpio::asserted(v, *active_low)
        }
    }
}

/// Soll diese Runde gelesen werden — und sagte der Pin dabei nein?
fn gate_check(l: &mut Live) -> (bool, bool) {
    let Gate::Pin { handle, reg_off, active_low, skipped, proved, .. } = &mut l.gate else {
        return (true, false);
    };
    let v = unsafe { npk_mmio_read32(*handle, *reg_off as i32) } as u32;
    if i2c_hid_core::gpio::asserted(v, *active_low) {
        *skipped = 0;
        return (true, false);
    }
    *skipped += 1;
    let every = if *proved { GATE_CROSS_CHECK_PROVED } else { GATE_CROSS_CHECK_ROUNDS };
    if *skipped >= every {
        *skipped = 0;
        (true, true)
    } else {
        (false, true)
    }
}

/// Was die Gegenprobe ergeben hat.
///
/// Abgeschaltet wird das Tor nur durch einen WIDERSPRUCH — ein Bericht,
/// der ankam, obwohl der Pin nichts meldete. Dass nichts kommt, beweist
/// gar nichts: ein unberuehrtes Touchpad schweigt.
fn gate_verdict(l: &mut Live, gate_said_no: bool, got_data: bool) {
    if !got_data { return; }
    let addr = l.addr;
    let n = match &mut l.gate {
        Gate::Blind => return,
        Gate::Pin { contradictions, proved, .. } => {
            if !gate_said_no {
                // Eine RICHTIGE Ansage loescht die Widersprueche.
                //
                // Ein Widerspruch kann auch ein Wettlauf sein: der Finger
                // setzt genau in der Gegenprobe auf, Mikrosekunden nachdem
                // der Pin gelesen wurde. Das passiert einzeln. Ein FALSCHES
                // Tor dagegen widerspricht bei jeder Gegenprobe und sagt
                // nie etwas richtig an — nur DAS soll es abschalten.
                *contradictions = 0;
                if !*proved {
                    *proved = true;
                    dbgln(&alloc::format!(
                        "[i2c-hid] {addr:#04x}: the pin announced a report — the gate holds"));
                }
                return;
            }
            *contradictions += 1;
            *contradictions
        }
    };
    if n >= GATE_MAX_CONTRADICTIONS {
        l.gate = Gate::Blind;
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: {n} reports in a row arrived while the pin said \
             nothing — the gate is wrong, back to reading blind"));
    } else {
        logln(&alloc::format!(
            "[i2c-hid] {addr:#04x}: a report arrived while the pin said nothing \
             ({n}/{GATE_MAX_CONTRADICTIONS})"));
    }
}

fn poll_live(l: &mut Live, buf: &mut [u8]) -> Step {
    use i2c_hid_core::{hid, report};
    let r = match hid::get_input(&mut l.bus, &l.dw, l.addr, &l.desc, buf) {
        Ok(Some(r)) => r,
        Ok(None) => return Step::Empty,
        Err(e) => {
            // Der Grund wurde bisher WEGGEWORFEN. Ein Timeout und ein
            // AddrNack sehen von aussen gleich aus und kosten das
            // Tausendfache voneinander: der eine kehrt sofort zurueck,
            // der andere haelt xfer bis zu einer Sekunde fest.
            if l.err_logged < 8 {
                l.err_logged += 1;
                logln(&alloc::format!(
                    "[i2c-hid] {:#04x}: input read failed: {e:?}", l.addr));
            }
            return Step::Dead;
        }
    };
    let (id, data) = if l.uses_ids && !r.is_empty() { (r[0], &r[1..]) } else { (0u8, r) };

    // Den Decoder zu DIESER Nummer nehmen. Kennt ihn keiner, einmal
    // sagen, welche Nummer kam — das ist die Auskunft, die fehlt, wenn
    // sich nichts bewegt.
    let Some(d) = l.decoders.iter().find(|d| d.rid == id) else {
        if l.unknown_logged < 3 {
            l.unknown_logged += 1;
            logln(&alloc::format!(
                "[i2c-hid]   report id {id} arrived, {} bytes — the descriptor does not \
                 declare that id; taking it as nothing", r.len()));
        }
        return Step::Junk;
    };
    let (mode, btn) = (&d.mode, &d.btn);
    l.seen += 1;
    if Some(id) == l.touch_rid {
        if !l.saw_touch {
            l.saw_touch = true;
            dbgln(&alloc::format!(
                "[i2c-hid] {:#04x}: touchpad report {id} is live — precision mode took",
                l.addr));
        }
    } else {
        l.other_seen += 1;
    }

    if l.raw_logged < 4 {
        l.raw_logged += 1;
        dbgln(&alloc::format!("[i2c-hid] {:#04x} in {id}: {:02x?}", l.addr, data));
    }

    // Die physische Tastenlage merken — aber nur aus einem Bericht, der
    // ueberhaupt Tasten FUEHRT. Ein Geraet, das seine Taste in einem
    // eigenen Bericht meldet, setzte sie sonst mit dem naechsten
    // Kontaktbericht still wieder zurueck, und das Festhalten unter dem
    // Druck waere wirkungslos, ohne dass es irgendwo auffiele.
    if !btn.is_empty() {
        let mut buttons = 0i32;
        for (i, f) in btn.iter().enumerate() {
            if report::extract(data, f) != 0 { buttons |= 1 << i; }
        }
        l.hw_buttons = buttons;
    }
    let buttons = l.hw_buttons;

    let (dx, dy, scroll, hscroll, tap) = match mode {
        Mode::Mouse { fx, fy, wheel } => {
            let x = report::extract(data, fx);
            let y = report::extract(data, fy);
            // Das Rad meldet immer RELATIV — Rasten, keine Position.
            let s = wheel.as_ref().map(|w| report::extract(data, w)).unwrap_or(0);
            if fx.relative {
                (x, y, s, 0, 0)
            } else {
                let d = if l.have_ref { (x - l.rx, y - l.ry) } else { (0, 0) };
                l.have_ref = true;
                l.rx = x; l.ry = y;
                (d.0, d.1, s, 0, 0)
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
            let now_ms = { let t = unsafe { npk_now_us() }; if t < 0 { 0 } else { t as u64 / 1000 } };
            // Die Taste faehrt MIT: ein durchgedruecktes Pad haelt die
            // Finger fest, und eine Beruehrung unter der Taste ist kein
            // Antippen. Beides gehoert in den Tracker, weil es dort
            // Tests hat.
            match l.track.feed(cc, &present[..np], contacts.len(), now_ms, buttons != 0) {
                gesture::Out::Pending => (0, 0, 0, 0, 0),
                gesture::Out::Frame { n, gesture, dx, dy, scroll, hscroll, tap } => {
                    if n > 0 && l.touch_logged < 3 {
                        l.touch_logged += 1;
                        dbgln(&alloc::format!(
                            "[i2c-hid]   frame: {n} finger(s), contact-count {cc}, \
                             gesture {gesture}, {:?}",
                            &l.track.frame()[..n.min(2)]));
                    }
                    if (scroll != 0 || hscroll != 0) && l.scroll_logged < 3 {
                        l.scroll_logged += 1;
                        dbgln(&alloc::format!(
                            "[i2c-hid]   scroll: {scroll} up, {hscroll} right"));
                    }
                    if tap > 0 && l.tap_logged < 2 {
                        l.tap_logged += 1;
                        dbgln(&alloc::format!("[i2c-hid]   tap: {tap} finger(s)"));
                    }
                    (dx, dy, scroll, hscroll, tap)
                }
            }
        }
    };

    // Erst die LAGE, dann der Impuls, dann der Weg.
    //
    // Die Reihenfolge ist nicht beliebig: beim zweiten Antippen einer
    // Reihe gibt der Tracker im selben Bild „Haltetaste auf" UND „ein
    // ganzer Klick". Kaeme der Klick zuerst, stuende er IN der noch
    // gedrueckten Taste und der Compositor saehe nur einen.
    push_buttons(l);
    if tap > 0 {
        let b = 1i32 << (tap - 1);
        unsafe { npk_pointer_inject(0, 0, l.last_buttons | b, 0, 0) };
        unsafe { npk_pointer_inject(0, 0, l.last_buttons, 0, 0) };
    }
    if dx != 0 || dy != 0 || scroll != 0 || hscroll != 0 {
        unsafe { npk_pointer_inject(dx, dy, l.last_buttons, scroll, hscroll) };
    }
    Step::Data
}

/// Die Tastenlage melden, wenn sie sich geaendert hat.
///
/// **Die einzige Stelle, die sie bildet.** Sie kommt aus zwei Quellen —
/// den physischen Tasten des letzten Berichts und der Taste, die ein
/// Antippen gerade haelt —, und zwei Stellen, die das je fuer sich
/// zusammenrechnen, waeren zwei Semantiken.
fn push_buttons(l: &mut Live) {
    let want = l.hw_buttons | l.track.hold() as i32;
    if want != l.last_buttons {
        unsafe { npk_pointer_inject(0, 0, want, 0, 0) };
        l.last_buttons = want;
    }
}
