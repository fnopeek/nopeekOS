//! Discovery: which device speaks HID over I2C, on which controller, at
//! which address — all from the DSDT, nothing hardcoded.
//!
//! Ported from Linux:
//!
//! * `drivers/hid/i2c-hid/i2c-hid-acpi.c` — the match table
//!   (`ACPI0C50`/`PNP0C50`), the blacklist and `_DSM` for the HID
//!   descriptor address.
//! * `drivers/i2c/i2c-core-acpi.c` — `i2c_acpi_get_i2c_resource` /
//!   `i2c_acpi_fill_info`: slave address, bus speed and the ACPI path of
//!   the controller from the `I2cSerialBus` descriptor.
//! * `drivers/acpi/acpi_apd.c` — the input clock per controller ID.
//! * `drivers/i2c/busses/i2c-designware-common.c` — `i2c_dw_acpi_params`:
//!   `SSCN`/`FMCN` supply `hcnt`/`lcnt`/`sda_hold` from the firmware.

use aml_core::{crs, path_str, seg, Machine, Namespace, Obj, Path, Value};
use alloc::{format, string::String, vec, vec::Vec};

/// `i2c_hid_acpi_match` — HID/CID values the driver binds to.
const HID_MATCH: [&str; 2] = ["ACPI0C50", "PNP0C50"];

/// `i2c_hid_acpi_blacklist` — carries `PNP0C50` but is not HID-capable.
const HID_BLACKLIST: [&str; 2] = ["CHPN0001", "IDEA5002"];

/// `i2c_hid_guid` — 3cdff6f7-4267-4555-ad05-b30a3d8938de, in the byte
/// order ACPI expects for a GUID in a buffer (first three fields
/// little-endian).
const I2C_HID_GUID: [u8; 16] = [
    0xF7, 0xF6, 0xDF, 0x3C, 0x67, 0x42, 0x55, 0x45,
    0xAD, 0x05, 0xB3, 0x0A, 0x3D, 0x89, 0x38, 0xDE,
];

/// Input clock of the DesignWare block per ACPI ID.
///
/// `acpi_apd.c`: `AMDI0010`/`AMDI0019` → `wt_i2c_desc` (150 MHz),
/// `AMD0010` → `cz_i2c_desc` (133 MHz). For an unknown ID this returns 0,
/// and the firmware must then supply the counts via `SSCN`/`FMCN`;
/// otherwise nothing is computed.
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

/// A controller as the firmware describes it.
#[derive(Clone, Debug)]
pub struct Controller {
    pub path: Path,
    pub ids: Vec<String>,
    pub mmio_base: u32,
    pub mmio_len: u32,
    pub irq: Vec<u32>,
    /// `_ADR` if the controller sits on PCI (Intel LPSS) rather than at
    /// fixed MMIO (AMD FCH). There is then no `Memory32Fixed`, which is not
    /// an error but a different attachment.
    pub pci_adr: Option<u64>,
    /// Does the controller's `_STA` say it is present and functional?
    ///
    /// Asked before the first MMIO access: a disabled block answers with
    /// all ones at best and not at all at worst.
    pub present: bool,
    /// The raw `_STA` value (`None` = no method, counts as present).
    pub sta: Option<u64>,
    /// 0 = unknown; `sscn`/`fmcn` are then the only source.
    pub input_clock_hz: u32,
    /// `(hcnt, lcnt, sda_hold)` from `SSCN` (standard mode).
    pub sscn: Option<(u16, u16, u32)>,
    /// The same from `FMCN` (fast mode).
    pub fmcn: Option<(u16, u16, u32)>,
}

/// A HID-over-I2C device as the firmware describes it.
#[derive(Clone, Debug)]
pub struct HidDevice {
    pub path: Path,
    pub ids: Vec<String>,
    pub slave_address: u16,
    pub bus_speed_hz: u32,
    /// The controller path exactly as it appears in the descriptor.
    pub controller_source: String,
    /// Resolved, if the path exists in the namespace.
    pub controller: Option<Controller>,
    /// Address of the HID descriptor — `_DSM` function 1.
    pub descriptor_address: Option<u16>,
    /// GPIO controller and pin of the "data ready" signal.
    pub gpio_source: String,
    pub gpio_pins: Vec<u16>,
    pub gpio_controller: Option<GpioController>,
    /// The raw interrupt flags of the `GpioInt`.
    ///
    /// They define what "data pending" means at the pin; without them the
    /// polarity would be a guess. ACPICA (`acpi_rs_convert_gpio` in
    /// `rsserial.c`) decodes the same field: bit 0 trigger mode, bits 2:1
    /// polarity, bit 3 shared, bit 4 wake-capable.
    pub gpio_int_flags: u16,
}

impl HidDevice {
    /// Level- rather than edge-triggered (ACPI bit 0, `ACPI_LEVEL_SENSITIVE`).
    ///
    /// Only a level can be polled: it stays asserted until the report is
    /// read. An edge is usually over by the time anyone looks.
    pub fn gpio_level_triggered(&self) -> bool {
        self.gpio_int_flags & 1 == 0
    }

    /// Active low (ACPI bits 2:1, `ACPI_ACTIVE_LOW` = 1).
    pub fn gpio_active_low(&self) -> bool {
        (self.gpio_int_flags >> 1) & 3 == 1
    }
}

/// The GPIO block the device's `GpioInt` points to.
#[derive(Clone, Debug)]
pub struct GpioController {
    pub path: Path,
    pub ids: Vec<String>,
    pub mmio_base: u32,
    pub mmio_len: u32,
    /// Its own interrupt line — the first `ExtendedIrq` of its `_CRS` and
    /// the descriptor's raw flags (ACPI 6.4.3.6: bit 1 edge, bit 2 active
    /// low). One line for all pins; `pinctrl-amd` takes it as
    /// `platform_get_irq(pdev, 0)`.
    pub irq: Option<(u32, u8)>,
}

/// Convert an ACPI path string into a namespace path.
///
/// `resource_source` is absolute in practice (`\_SB.I2CB`) but may be
/// relative; `acpi_get_handle` then resolves it against the device.
/// Each leading `^` goes up one level.
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

/// Fetch and decode a node's `_CRS`.
fn resources(m: &mut Machine, dev: &Path) -> Vec<crs::Resource> {
    match m.eval_child(dev, "_CRS") {
        Ok(Value::Buffer(b)) => crs::parse(&b),
        _ => Vec::new(),
    }
}

/// `i2c_dw_acpi_params` — `SSCN`/`FMCN`/`FPCN`/`HSCN` return a package of
/// three numbers: `hcnt`, `lcnt`, `sda_hold`.
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
    let mut irq = None;
    for r in resources(m, path) {
        match r {
            crs::Resource::Memory32Fixed { address, length } if mmio_base == 0 => {
                mmio_base = address;
                mmio_len = length;
            }
            crs::Resource::ExtendedIrq { flags, interrupts } if irq.is_none() => {
                irq = interrupts.first().map(|&g| (g, flags));
            }
            _ => {}
        }
    }
    GpioController { path: path.clone(), ids, mmio_base, mmio_len, irq }
}

/// `i2c_hid_acpi_get_descriptor` — `_DSM(guid, 1, 1, {})` returns the HID
/// descriptor address as an integer.
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
            // A `_DSM` that does not know the function returns an empty
            // buffer; that reads as 0, which is not a valid register address.
            if n == 0 { None } else { Some(n as u16) }
        }
        Err(_) => None,
    }
}

/// The full walk: every device with an ID that matches the HID-over-I2C
/// table, with its controller resolved.
pub fn find(ns: &Namespace, m: &mut Machine) -> Vec<HidDevice> {
    let mut out = Vec::new();
    for dev in aml_core::devices_with_ids(ns) {
        let ids = m.device_ids(&dev);
        if !ids.iter().any(|i| HID_MATCH.contains(&i.as_str())) {
            continue;
        }
        // From here on it is a candidate, and every exit says why; a silent
        // skip would hide everything above it.
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
        let mut gpio_int_flags = 0u16;
        for r in res {
            match r {
                // `i2c_acpi_fill_info`: the first I2cSerialBus counts.
                crs::Resource::I2cSerialBus {
                    slave_address: a, connection_speed, source, ..
                } if slave_address == 0 => {
                    slave_address = a;
                    bus_speed_hz = connection_speed;
                    controller_source = source;
                }
                crs::Resource::Gpio { connection_type, pins, source, int_flags, .. }
                    if connection_type == crs::GPIO_CONN_INTERRUPT && gpio_pins.is_empty() =>
                {
                    gpio_pins = pins;
                    gpio_source = source;
                    gpio_int_flags = int_flags;
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
            gpio_int_flags,
        });
    }
    out
}

/// One line per item: the report the module writes to the log.
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
                    // Intel LPSS: the controller is a PCI device with its
                    // registers in a BAR. Not an error.
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
        out.push(format!(
            "i2c-hid:   GpioInt pin {:?} {} active-{} on {}",
            d.gpio_pins,
            if d.gpio_level_triggered() { "level" } else { "edge" },
            if d.gpio_active_low() { "low" } else { "high" },
            g
        ));
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

    /// Real HP DSDT (Intel notebook, Synaptics touchpad on I2C1).
    ///
    /// Every value here comes from the firmware, none from the code. If one
    /// disappears, part of the ACPI path is broken. Each of these exercises
    /// a specific interpreter feature:
    ///
    /// * `PNP0C50` only appears with `_HID` as a method,
    /// * the `_CRS` needs `ConcatenateResTemplate`,
    /// * its 75 bytes need `_OSI` as a name (otherwise the firmware takes
    ///   its path for a pre-2012 OS),
    /// * the descriptor address needs `CreateWordField`.
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

        // The GPIO block whose pin carries "data ready".
        let g = d.gpio_controller.as_ref().expect("GPIO controller");
        assert!(g.ids.iter().any(|i| i == "INT34BB"));
        assert_eq!(g.mmio_len, 0x10000);

        // What "data pending" means at the pin, from real firmware rather
        // than an assumption. 0x12 = level (bit 0 = 0), active low
        // (bits 2:1 = 1), wake-capable.
        assert_eq!(d.gpio_int_flags, 0x0012);
        assert!(d.gpio_level_triggered());
        assert!(d.gpio_active_low());

        // This block is an Intel block: its register layout is not the one
        // `gpio.rs` computes, so it must not be touched.
        assert!(!crate::gpio::is_amd_block(&g.ids));
    }

    /// The same table, with conditionals at scope level resolved.
    ///
    /// `Device (I2C1)` declares its `_HID` inside `If ((SMD1 != One))`. A
    /// loader that skips conditionals sees a controller without an ID, and
    /// without an ID there is no input clock, no SCL counts and no bus.
    ///
    /// Not tested: what lies behind the conditionals depends on firmware
    /// NVS values a harness cannot provide; only what is independent of
    /// them is checked.
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
