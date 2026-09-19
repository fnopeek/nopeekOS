//! Entdeckung: welches Geraet spricht HID over I2C, an welchem Controller,
//! unter welcher Adresse — alles aus der DSDT, nichts hartkodiert.
//!
//! Portiert aus Linux 6.18.26:
//!
//! * `drivers/hid/i2c-hid/i2c-hid-acpi.c` — die Treffertabelle
//!   (`ACPI0C50`/`PNP0C50`), die Blockliste und `_DSM` fuer die Adresse des
//!   HID-Deskriptors.
//! * `drivers/i2c/i2c-core-acpi.c` — `i2c_acpi_get_i2c_resource` /
//!   `i2c_acpi_fill_info`: Slave-Adresse, Busfrequenz und der ACPI-PFAD des
//!   Controllers aus dem `I2cSerialBus`-Deskriptor.
//! * `drivers/acpi/acpi_apd.c` — der Eingangstakt je Controller-Kennung.
//! * `drivers/i2c/busses/i2c-designware-common.c` — `i2c_dw_acpi_params`:
//!   `SSCN`/`FMCN` liefern `hcnt`/`lcnt`/`sda_hold` fertig aus der Firmware.

use aml_core::{crs, path_str, seg, Machine, Namespace, Obj, Path, Value};
use alloc::{format, string::String, vec, vec::Vec};

/// `i2c_hid_acpi_match` — HID/CID, auf die der Treiber greift.
const HID_MATCH: [&str; 2] = ["ACPI0C50", "PNP0C50"];

/// `i2c_hid_acpi_blacklist` — traegt `PNP0C50`, ist aber nicht HID-faehig.
const HID_BLACKLIST: [&str; 2] = ["CHPN0001", "IDEA5002"];

/// `i2c_hid_guid` — 3cdff6f7-4267-4555-ad05-b30a3d8938de, in der
/// Bytefolge, in der ACPI eine GUID in einem Puffer erwartet (die ersten
/// drei Felder little-endian).
const I2C_HID_GUID: [u8; 16] = [
    0xF7, 0xF6, 0xDF, 0x3C, 0x67, 0x42, 0x55, 0x45,
    0xAD, 0x05, 0xB3, 0x0A, 0x3D, 0x89, 0x38, 0xDE,
];

/// Eingangstakt des Designware-Blocks je ACPI-Kennung.
///
/// `acpi_apd.c`: `AMDI0010`/`AMDI0019` → `wt_i2c_desc` (150 MHz),
/// `AMD0010` → `cz_i2c_desc` (133 MHz). **Nicht geraten — jede Zahl steht
/// dort.** Ist die Kennung unbekannt, gibt es 0, und dann MUSS die Firmware
/// die Zaehler ueber `SSCN`/`FMCN` liefern; sonst wird nichts gerechnet.
fn input_clock_hz(ids: &[String]) -> u32 {
    for id in ids {
        match id.as_str() {
            "AMDI0010" | "AMDI0019" => return 150_000_000,
            "AMD0010" => return 133_000_000,
            "AMDI0015" => return 125_000_000, // wt_i3c_desc
            _ => {}
        }
    }
    0
}

/// Ein Controller, wie die Firmware ihn beschreibt.
#[derive(Clone, Debug)]
pub struct Controller {
    pub path: Path,
    pub ids: Vec<String>,
    pub mmio_base: u32,
    pub mmio_len: u32,
    pub irq: Vec<u32>,
    /// `_ADR`, wenn der Controller an PCI haengt (Intel LPSS) statt an
    /// fester MMIO (AMD FCH). Dann gibt es kein `Memory32Fixed`, und das
    /// ist kein Fehler, sondern eine andere Anbindung.
    pub pci_adr: Option<u64>,
    /// Sagt `_STA` des CONTROLLERS, dass er da und funktionsfaehig ist?
    ///
    /// Vor dem ersten MMIO-Zugriff gefragt: ein abgeschalteter Block
    /// antwortet im besten Fall mit lauter Einsen und im schlechteren gar
    /// nicht. Die Firmware weiss es, also wird sie gefragt.
    pub present: bool,
    /// Der rohe `_STA`-Wert (`None` = keine Methode, gilt als vorhanden).
    pub sta: Option<u64>,
    /// 0 = unbekannt; dann sind `sscn`/`fmcn` die einzige Quelle.
    pub input_clock_hz: u32,
    /// `(hcnt, lcnt, sda_hold)` aus `SSCN` (Standard Mode).
    pub sscn: Option<(u16, u16, u32)>,
    /// dasselbe aus `FMCN` (Fast Mode).
    pub fmcn: Option<(u16, u16, u32)>,
}

/// Ein HID-over-I2C-Geraet, wie die Firmware es beschreibt.
#[derive(Clone, Debug)]
pub struct HidDevice {
    pub path: Path,
    pub ids: Vec<String>,
    pub slave_address: u16,
    pub bus_speed_hz: u32,
    /// Der Pfad des Controllers, WIE ER IM DESKRIPTOR STEHT.
    pub controller_source: String,
    /// Aufgeloest, wenn der Pfad im Namespace steht.
    pub controller: Option<Controller>,
    /// Adresse des HID-Deskriptors — `_DSM` Funktion 1.
    pub descriptor_address: Option<u16>,
    /// GPIO-Controller und Pin des „Daten liegen bereit"-Signals.
    pub gpio_source: String,
    pub gpio_pins: Vec<u16>,
    pub gpio_controller: Option<GpioController>,
}

/// Der GPIO-Block, auf den der `GpioInt` des Geraets zeigt.
#[derive(Clone, Debug)]
pub struct GpioController {
    pub path: Path,
    pub ids: Vec<String>,
    pub mmio_base: u32,
    pub mmio_len: u32,
}

/// Einen ACPI-Pfad aus einer Zeichenkette in einen Namespace-Pfad wandeln.
///
/// `resource_source` ist in der Praxis absolut (`\_SB.I2CB`), darf aber
/// relativ sein; `acpi_get_handle` loest es dann gegen das Geraet auf.
/// Fuehrende `^` steigen je eine Ebene.
pub fn parse_acpi_path(ns: &Namespace, scope: &Path, s: &str) -> Option<Path> {
    let b = s.as_bytes();
    let mut i = 0;
    let rooted = !b.is_empty() && b[0] == b'\\';
    if rooted {
        i = 1;
    }
    let mut carets = 0usize;
    while i < b.len() && b[i] == b'^' {
        carets += 1;
        i += 1;
    }
    let rest = &s[i..];
    if rest.is_empty() {
        return if rooted { Some(Vec::new()) } else { None };
    }
    let mut segs: Vec<[u8; 4]> = Vec::new();
    for part in rest.split('.') {
        if part.is_empty() {
            continue;
        }
        segs.push(seg(part));
    }
    ns.resolve(scope, rooted, carets, &segs)
}

/// `_CRS` eines Knotens holen und zerlegen.
fn resources(m: &mut Machine, dev: &Path) -> Vec<crs::Resource> {
    match m.eval_child(dev, "_CRS") {
        Ok(Value::Buffer(b)) => crs::parse(&b),
        _ => Vec::new(),
    }
}

/// `i2c_dw_acpi_params` — `SSCN`/`FMCN`/`FPCN`/`HSCN` liefern ein Package
/// aus drei Zahlen: `hcnt`, `lcnt`, `sda_hold`.
fn scl_params(m: &mut Machine, dev: &Path, name: &str) -> Option<(u16, u16, u32)> {
    match m.eval_child(dev, name) {
        Ok(Value::Package(e)) if e.len() == 3 => Some((
            e[0].borrow().as_int() as u16,
            e[1].borrow().as_int() as u16,
            e[2].borrow().as_int() as u32,
        )),
        _ => None,
    }
}

fn read_controller(m: &mut Machine, ns: &Namespace, path: &Path) -> Controller {
    let ids = m.device_ids(path);
    let mut mmio_base = 0u32;
    let mut mmio_len = 0u32;
    let mut irq = Vec::new();
    for r in resources(m, path) {
        match r {
            crs::Resource::Memory32Fixed { address, length } if mmio_base == 0 => {
                mmio_base = address;
                mmio_len = length;
            }
            crs::Resource::ExtendedIrq { interrupts, .. } => irq.extend(interrupts),
            _ => {}
        }
    }
    let _ = ns;
    let pci_adr = match m.eval_child(path, "_ADR") {
        Ok(v) => Some(v.as_int()),
        Err(_) => None,
    };
    Controller {
        pci_adr,
        present: m.device_present(path),
        sta: m.device_status(path),
        input_clock_hz: input_clock_hz(&ids),
        sscn: scl_params(m, path, "SSCN"),
        fmcn: scl_params(m, path, "FMCN"),
        path: path.clone(),
        ids,
        mmio_base,
        mmio_len,
        irq,
    }
}

fn read_gpio_controller(m: &mut Machine, path: &Path) -> GpioController {
    let ids = m.device_ids(path);
    let mut mmio_base = 0u32;
    let mut mmio_len = 0u32;
    for r in resources(m, path) {
        if let crs::Resource::Memory32Fixed { address, length } = r {
            if mmio_base == 0 {
                mmio_base = address;
                mmio_len = length;
            }
        }
    }
    GpioController { path: path.clone(), ids, mmio_base, mmio_len }
}

/// `i2c_hid_acpi_get_descriptor` — `_DSM(guid, 1, 1, {})` gibt die Adresse
/// des HID-Deskriptors als Zahl zurueck.
fn dsm_descriptor_address(m: &mut Machine, dev: &Path) -> Option<u16> {
    let mut p = dev.clone();
    p.push(seg("_DSM"));
    if !m.has(&p) {
        return None;
    }
    let args: Vec<Obj> = vec![
        aml_core::obj(Value::Buffer(I2C_HID_GUID.to_vec())),
        aml_core::obj(Value::Int(1)),
        aml_core::obj(Value::Int(1)),
        aml_core::obj(Value::Package(Vec::new())),
    ];
    match m.call(&p, args) {
        Ok(v) => {
            let n = v.as_int();
            m.note(&format!("i2c-hid: _DSM(func 1) -> {:#x}", n));
            // Ein `_DSM`, das die Funktion nicht kennt, gibt einen leeren
            // Puffer zurueck — der liest sich als 0, und 0 ist keine
            // gueltige Registeradresse.
            if n == 0 { None } else { Some(n as u16) }
        }
        Err(_) => None,
    }
}

/// Der ganze Gang: jedes Geraet mit einer Kennung, das auf die
/// HID-over-I2C-Tabelle passt, samt aufgeloestem Controller.
pub fn find(ns: &Namespace, m: &mut Machine) -> Vec<HidDevice> {
    let mut out = Vec::new();
    for dev in aml_core::devices_with_ids(ns) {
        let ids = m.device_ids(&dev);
        if !ids.iter().any(|i| HID_MATCH.contains(&i.as_str())) {
            continue;
        }
        // Ab hier IST es ein Kandidat, und jeder Ausstieg sagt, warum.
        // Ein stiller Uebersprung verdeckt alles, was darueber liegt.
        let name = path_str(&dev);
        if ids.iter().any(|i| HID_BLACKLIST.contains(&i.as_str())) {
            m.note(&format!("i2c-hid: {name} is blacklisted (not HID capable)"));
            continue;
        }
        if !m.device_present(&dev) {
            m.note(&format!("i2c-hid: {name} _STA says absent/not functioning"));
            continue;
        }

        let crs_val = m.eval_child(&dev, "_CRS");
        let res = match &crs_val {
            Ok(Value::Buffer(b)) => {
                m.note(&format!("i2c-hid: {name} _CRS = {} bytes", b.len()));
                crs::parse(b)
            }
            Ok(_) => {
                m.note(&format!("i2c-hid: {name} _CRS did not return a buffer"));
                Vec::new()
            }
            Err(e) => {
                m.note(&format!("i2c-hid: {name} _CRS failed: {e}"));
                Vec::new()
            }
        };

        let mut slave_address = 0u16;
        let mut bus_speed_hz = 0u32;
        let mut controller_source = String::new();
        let mut gpio_source = String::new();
        let mut gpio_pins = Vec::new();
        for r in res {
            match r {
                // `i2c_acpi_fill_info`: der ERSTE I2cSerialBus zaehlt.
                crs::Resource::I2cSerialBus {
                    slave_address: a, connection_speed, source, ..
                } if slave_address == 0 => {
                    slave_address = a;
                    bus_speed_hz = connection_speed;
                    controller_source = source;
                }
                crs::Resource::Gpio { connection_type, pins, source, .. }
                    if connection_type == crs::GPIO_CONN_INTERRUPT && gpio_pins.is_empty() =>
                {
                    gpio_pins = pins;
                    gpio_source = source;
                }
                _ => {}
            }
        }
        if controller_source.is_empty() {
            m.note(&format!("i2c-hid: {name} has no I2cSerialBus in _CRS"));
            continue;
        }

        let scope = {
            let mut s = dev.clone();
            s.pop();
            s
        };
        let controller = parse_acpi_path(ns, &scope, &controller_source)
            .map(|p| read_controller(m, ns, &p));
        let gpio_controller = if gpio_source.is_empty() {
            None
        } else {
            parse_acpi_path(ns, &scope, &gpio_source).map(|p| read_gpio_controller(m, &p))
        };
        let descriptor_address = dsm_descriptor_address(m, &dev);

        out.push(HidDevice {
            path: dev,
            ids,
            slave_address,
            bus_speed_hz,
            controller_source,
            controller,
            descriptor_address,
            gpio_source,
            gpio_pins,
            gpio_controller,
        });
    }
    out
}

/// Ein Fund in einer Zeile je Sache — das ist der Bericht, den das Modul am
/// Geraet ins Log schreibt.
pub fn report(d: &HidDevice) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "i2c-hid: {} [{}] addr={:#04x} speed={} Hz",
        path_str(&d.path),
        d.ids.join(","),
        d.slave_address,
        d.bus_speed_hz
    ));
    match &d.descriptor_address {
        Some(a) => out.push(format!("i2c-hid:   HID descriptor register {:#06x} (_DSM)", a)),
        None => out.push(String::from("i2c-hid:   no _DSM descriptor address — cannot probe")),
    }
    match &d.controller {
        Some(c) => {
            out.push(format!(
                "i2c-hid:   controller {} [{}] mmio {:#010x}+{:#x} irq {:?} clk {} Hz",
                path_str(&c.path),
                c.ids.join(","),
                c.mmio_base,
                c.mmio_len,
                c.irq,
                c.input_clock_hz
            ));
            if let Some((h, l, s)) = c.sscn {
                out.push(format!("i2c-hid:   SSCN hcnt={} lcnt={} sda_hold={}", h, l, s));
            }
            if let Some((h, l, s)) = c.fmcn {
                out.push(format!("i2c-hid:   FMCN hcnt={} lcnt={} sda_hold={}", h, l, s));
            }
            if !c.present {
                out.push(format!(
                    "i2c-hid:   controller _STA = {:#x} — absent or not functioning",
                    c.sta.unwrap_or(0)));
            }
            if c.mmio_base == 0 {
                match c.pci_adr {
                    // Intel LPSS: der Controller IST ein PCI-Geraet, seine
                    // Register stehen in einem BAR. Kein Fehler.
                    Some(adr) => out.push(format!(
                        "i2c-hid:   controller is PCI-attached (_ADR {:#x} = dev {} fn {}) — no fixed MMIO",
                        adr,
                        (adr >> 16) & 0xFFFF,
                        adr & 0xFFFF
                    )),
                    None if c.ids.is_empty() => out.push(String::from(
                        "i2c-hid:   controller carries no _HID and no _ADR (Intel tables put both inside an If() at scope level, which our loader skips)",
                    )),
                    None => out.push(String::from(
                        "i2c-hid:   controller has NEITHER fixed MMIO NOR _ADR",
                    )),
                }
            }
            if c.input_clock_hz == 0 && c.sscn.is_none() && c.fmcn.is_none() {
                out.push(String::from(
                    "i2c-hid:   NEITHER a known input clock NOR SSCN/FMCN — timings unknown",
                ));
            }
        }
        None => out.push(format!(
            "i2c-hid:   controller \"{}\" not found in namespace",
            d.controller_source
        )),
    }
    if d.gpio_pins.is_empty() {
        out.push(String::from("i2c-hid:   no GpioInt — will have to poll blind"));
    } else {
        let g = match &d.gpio_controller {
            Some(g) => format!(
                "{} [{}] mmio {:#010x}+{:#x}",
                path_str(&g.path),
                g.ids.join(","),
                g.mmio_base,
                g.mmio_len
            ),
            None => format!("\"{}\" not found", d.gpio_source),
        };
        out.push(format!("i2c-hid:   GpioInt pin {:?} on {}", d.gpio_pins, g));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aml_core::Ec;

    struct NoEc;
    impl Ec for NoEc {
        fn read(&mut self, _a: u8) -> u8 { 0 }
        fn write(&mut self, _a: u8, _v: u8) {}
    }

    /// Echte HP-DSDT (Intel-Notebook, Synaptics-Touchpad auf I2C1).
    ///
    /// Das ist der Pruefstand ohne Geraet: jede Zahl hier kommt aus der
    /// Firmware, keine steht im Code. Faellt einer der Werte weg, ist ein
    /// Stueck des ACPI-Wegs kaputt — und GENAU diese vier Stellen haben
    /// beim Bauen je einen Fehler aufgedeckt:
    ///
    /// * `PNP0C50` kam nur mit `_HID` ALS METHODE heraus,
    /// * das `_CRS` brauchte `ConcatenateResTemplate`,
    /// * die 75 Bytes darin brauchten `_OSI` als NAMEN (sonst nimmt die
    ///   Firmware ihren Pfad fuer ein System von vor 2012),
    /// * die Deskriptor-Adresse brauchte `CreateWordField`.
    #[test]
    fn hp_dsdt_finds_the_touchpad() {
        let table = include_bytes!("../../../aml/dev/DSDT.aml");
        let ns = Namespace::load(table).expect("load");
        let mut ec = NoEc;
        let mut m = Machine::new(&ns, &mut ec);
        m.init();

        let found = find(&ns, &mut m);
        assert_eq!(found.len(), 1, "expected exactly one HID-over-I2C device");
        let d = &found[0];

        assert!(d.ids.iter().any(|i| i == "PNP0C50"));
        assert!(d.ids.iter().any(|i| i == "SYNA30A1"));
        assert_eq!(d.slave_address, 0x2C);
        assert_eq!(d.bus_speed_hz, 400_000);
        assert_eq!(d.descriptor_address, Some(0x0020));
        assert_eq!(d.controller_source, "\\_SB.PCI0.I2C1");
        assert!(d.controller.is_some(), "controller path must resolve");

        // Der GPIO-Block, dessen Pin das „Daten liegen bereit" traegt.
        let g = d.gpio_controller.as_ref().expect("GPIO controller");
        assert!(g.ids.iter().any(|i| i == "INT34BB"));
        assert_eq!(g.mmio_len, 0x10000);
    }

    /// Dieselbe Tabelle, aber mit aufgeloesten Bedingungen auf Scope-Ebene.
    ///
    /// `Device (I2C1)` deklariert sein `_HID` INNERHALB eines
    /// `If ((SMD1 != One))`. Ein Lader, der Bedingungen ueberspringt, sieht
    /// einen Controller ohne Kennung — und ohne Kennung gibt es keinen
    /// Eingangstakt, also keine SCL-Zaehler, also keinen Bus.
    ///
    /// **Was der Test NICHT prueft:** was hinter den Bedingungen steht,
    /// haengt an NVS-Werten der Firmware, und die kann ein Pruefstand ohne
    /// Geraet nicht hinterlegen. Deshalb hier nur das, was von ihnen
    /// unabhaengig ist.
    #[test]
    fn scope_level_conditionals_declare_the_controller_hid() {
        let table = include_bytes!("../../../aml/dev/DSDT.aml");
        let mut ns = Namespace::load(table).expect("load");
        let mut ec = NoEc;
        let (seen, taken) = ns.resolve_conditionals(&mut ec);
        assert!(seen > 0, "die Tabelle hat bedingte Deklarationen auf Scope-Ebene");
        assert!(taken > 0, "und mindestens eine gilt");

        let mut m = Machine::new(&ns, &mut ec);
        m.init();
        let found = find(&ns, &mut m);
        assert_eq!(found.len(), 1);
        let c = found[0].controller.as_ref().expect("controller");
        assert!(c.ids.iter().any(|i| i == "INT34B3"),
            "das _HID steht im If — ohne aufgeloeste Bedingung fehlt es: {:?}", c.ids);
    }
}
