//! The Pi 5's fan following the CPU temperature, as Linux sets it up from
//! the device tree: the `pwm-fan` node the firmware enables when it found
//! the fan spinning at boot, its RP1 PWM (clock, pins, channel), the
//! `brcm,bcm2711-thermal` sensor in the AVS monitor, and the thermal zone
//! that reads it with its trips and cooling maps. One fiber polls the zone
//! every `polling-delay`. The driver is `pi5_core::fan`.

use super::dt::{Fdt, Node};
use crate::hw::Mmio;
use crate::kprintln;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use pi5_core::fan::{self, Action, Fan, Rp1Pwm, Trip, TripType, Zone, MAX_TRIPS};
use pi5_core::hw::{Block, Hw};

/// `PWM_POLARITY_INVERTED` in a `pwms` flags cell.
const PWM_POLARITY_INVERTED: u32 = 1;
/// `RP1_PUD_*`.
const PUD_OFF: u32 = 0;
const PUD_DOWN: u32 = 1;
const PUD_UP: u32 = 2;

/// RP1's blocks and the AVS monitor.
struct FanHw {
    rp1: super::pi5::Board,
    avs: Mmio,
}

impl Hw for FanHw {
    fn read(&mut self, b: Block, off: u64, w: u8) -> u32 {
        if b == Block::Avs { self.avs.r32(off) } else { self.rp1.read(b, off, w) }
    }
    fn write(&mut self, b: Block, off: u64, w: u8, v: u32) {
        if b == Block::Avs { self.avs.w32(off, v) } else { self.rp1.write(b, off, w, v) }
    }
    fn delay_us(&mut self, us: u64) { crate::time::delay_us(us) }
}

struct Setup {
    hw: FanHw,
    fan: Fan,
    zone: Zone,
    trips: [Trip; MAX_TRIPS],
    ntrips: usize,
    slope: i32,
    offset: i32,
    polling_ms: u64,
}

static SETUP: spin::Mutex<Option<Setup>> = spin::Mutex::new(None);
/// Last reading in millidegrees Celsius, `i32::MIN` before one.
static TEMP: AtomicI32 = AtomicI32::new(i32::MIN);
static STATE: AtomicU32 = AtomicU32::new(0);
/// A cooling state held by hand (`fan <level>`), or `AUTO`: the zone's.
static HELD: AtomicU32 = AtomicU32::new(AUTO);
const AUTO: u32 = u32::MAX;

/// `fan <level>` holds the fan at a cooling state until `fan auto` (the
/// thermal zone's mode off and the cooling device's `cur_state` written,
/// as through sysfs). A critical trip still powers off. False for a level
/// above the fan's highest.
pub fn hold(level: Option<u32>) -> bool {
    let mut g = SETUP.lock();
    let Some(s) = g.as_mut() else { return false };
    match level {
        Some(l) => {
            if !s.fan.set_cur_state(&mut s.hw, l) { return false; }
            STATE.store(l, Ordering::Release);
            HELD.store(l, Ordering::Release);
        }
        None => {
            HELD.store(AUTO, Ordering::Release);
            // The zone starts over: its instances decide again on the next poll.
            let trips = s.trips;
            s.zone = Zone::new(&trips[..s.ntrips]);
        }
    }
    true
}

/// The CPU temperature in millidegrees Celsius, the fan's cooling state,
/// its duty (0..=255) and speed in RPM, once the fan runs.
pub fn status() -> Option<(i32, u32, u32, Option<u32>)> {
    let t = TEMP.load(Ordering::Acquire);
    if t == i32::MIN { return None; }
    let mut g = SETUP.lock();
    let s = g.as_mut()?;
    let rpm = s.fan.rpm(&mut s.hw);
    Some((t, STATE.load(Ordering::Acquire), s.fan.pwm_value() as u32, rpm))
}

fn phandle_node(fdt: &Fdt, node: &Node, prop: &[u8], i: usize) -> Option<Node> {
    fdt.find_phandle(fdt.prop_cell(node, prop, i)?)
}

/// `pinctrl-0` of `node` applied as the pinctrl core does at probe: each
/// pin of the state switched to its function, then its bias.
fn apply_pinctrl(fdt: &Fdt, node: &Node, hw: &mut FanHw) -> bool {
    let Some(state) = phandle_node(fdt, node, b"pinctrl-0", 0) else { return true };
    let mut func = [0u8; 32];
    let Some(fl) = fdt.prop_str(&state, b"function", &mut func) else { return false };
    let Ok(func) = core::str::from_utf8(&func[..fl]) else { return false };
    let mut pins = [0u8; 16];
    let Some(pl) = fdt.prop_str(&state, b"pins", &mut pins) else { return false };
    let Some(num) = core::str::from_utf8(&pins[..pl]).ok()
        .and_then(|p| p.strip_prefix("gpio")).and_then(|n| n.parse::<u32>().ok()) else { return false };
    let (Some(pin), Some(fsel)) = (pi5_core::gpio_rp1::Pin::new(num), pi5_core::gpio_rp1::fsel_of(num, func)) else {
        return false;
    };
    pin.set_function(hw, fsel);
    let pud = if fdt.prop(&state, b"bias-pull-down").is_some() { Some(PUD_DOWN) }
        else if fdt.prop(&state, b"bias-pull-up").is_some() { Some(PUD_UP) }
        else if fdt.prop(&state, b"bias-disable").is_some() { Some(PUD_OFF) }
        else { None };
    if let Some(pud) = pud { pin.set_pull(hw, pud); }
    true
}

/// The zone whose `thermal-sensors` is `sensor`: polling delay,
/// coefficients, and its trips with the fan's cooling maps.
fn thermal_zone(fdt: &Fdt, sensor: u32, fan_ph: u32) -> Option<(u64, i32, i32, [Option<Trip>; MAX_TRIPS])> {
    let zones = fdt.find_path(b"/thermal-zones")?;
    let mut zone = None;
    fdt.children(&zones, |z| {
        if fdt.prop_u32(z, b"thermal-sensors") == Some(sensor) { zone = Some(*z); }
        zone.is_none()
    });
    let zone = zone?;
    let polling_ms = fdt.prop_u32(&zone, b"polling-delay").unwrap_or(1000).max(1) as u64;
    // thermal_of: without `coefficients` the slope is 1 and the offset 0.
    let slope = fdt.prop_cell(&zone, b"coefficients", 0).map_or(1, |c| c as i32);
    let offset = fdt.prop_cell(&zone, b"coefficients", 1).map_or(0, |c| c as i32);

    let mut trips: [Option<(u32, Trip)>; MAX_TRIPS] = [None; MAX_TRIPS];
    let mut n = 0;
    let mut nodes = [None; 2];
    fdt.children(&zone, |c| {
        if fdt.name_is(c, b"trips") { nodes[0] = Some(*c); }
        if fdt.name_is(c, b"cooling-maps") { nodes[1] = Some(*c); }
        true
    });
    if let Some(t) = nodes[0] {
        fdt.children(&t, |c| {
            let (Some(temp), Some(hyst)) = (fdt.prop_u32(c, b"temperature"), fdt.prop_u32(c, b"hysteresis")) else { return true };
            let kind = if fdt.prop_has_str(c, b"type", b"critical") { TripType::Critical }
                else if fdt.prop_has_str(c, b"type", b"hot") { TripType::Hot }
                else if fdt.prop_has_str(c, b"type", b"passive") { TripType::Passive }
                else { TripType::Active };
            if n < MAX_TRIPS {
                let ph = fdt.prop_u32(c, b"phandle").unwrap_or(0);
                trips[n] = Some((ph, Trip { temperature: temp as i32, hysteresis: hyst as i32, kind, cooling: None }));
                n += 1;
            }
            true
        });
    }
    if let Some(m) = nodes[1] {
        fdt.children(&m, |c| {
            let (Some(trip), Some(dev)) = (fdt.prop_u32(c, b"trip"), fdt.prop_cell(c, b"cooling-device", 0)) else { return true };
            let (Some(lo), Some(hi)) = (fdt.prop_cell(c, b"cooling-device", 1), fdt.prop_cell(c, b"cooling-device", 2)) else { return true };
            if dev != fan_ph { return true; }
            for (ph, t) in trips.iter_mut().flatten() {
                if *ph == trip { t.cooling = Some((lo, hi)); }
            }
            true
        });
    }
    let mut out = [None; MAX_TRIPS];
    for (o, t) in out.iter_mut().zip(trips.iter()) {
        *o = t.map(|(_, t)| t);
    }
    Some((polling_ms, slope, offset, out))
}

/// Find the fan, its PWM and the zone, probe them as Linux does, and
/// start the fiber that polls the zone. Needs RP1 up.
pub fn init(fdt: &Fdt) -> bool {
    let Some(fan_node) = fdt.find_compatible(b"pwm-fan") else {
        kprintln!("[npk] fan: none enabled in the device tree (the firmware found no fan spinning)");
        return false;
    };
    let Some(pwm_node) = phandle_node(fdt, &fan_node, b"pwms", 0) else {
        kprintln!("[npk] fan: no PWM");
        return false;
    };
    let (Some(channel), Some(period)) = (fdt.prop_cell(&fan_node, b"pwms", 1), fdt.prop_cell(&fan_node, b"pwms", 2)) else {
        kprintln!("[npk] fan: pwms cells unreadable");
        return false;
    };
    let inverted = fdt.prop_cell(&fan_node, b"pwms", 3).unwrap_or(0) & PWM_POLARITY_INVERTED != 0;
    let Some(base) = fdt.reg0(&pwm_node).and_then(|(cpu, _)| super::pi5::rp1_bar1_offset(cpu)) else {
        kprintln!("[npk] fan: RP1 is not up");
        return false;
    };
    let Some((_, board, _)) = super::pi5::rp1() else {
        kprintln!("[npk] fan: RP1 is not up");
        return false;
    };
    let Some(sensor) = fdt.find_compatible(b"brcm,bcm2711-thermal") else {
        kprintln!("[npk] fan: no temperature sensor in the device tree");
        return false;
    };
    // The sensor's regmap is its syscon parent's.
    let Some((avs_pa, avs_len)) = fdt.find_compatible(b"brcm,bcm2711-avs-monitor").and_then(|n| fdt.reg0(&n)) else {
        kprintln!("[npk] fan: no AVS monitor");
        return false;
    };
    // SAFETY: the AVS monitor's `reg` window from the device tree, device
    // memory nothing else maps.
    let Some(avs) = (unsafe { Mmio::map(avs_pa, avs_len) }).ok() else {
        kprintln!("[npk] fan: AVS monitor not mappable");
        return false;
    };
    let fan_ph = fdt.prop_u32(&fan_node, b"phandle").unwrap_or(0);
    let Some((polling_ms, slope, offset, trips)) = fdt.prop_u32(&sensor, b"phandle")
        .and_then(|ph| thermal_zone(fdt, ph, fan_ph)) else {
        kprintln!("[npk] fan: no thermal zone for the sensor");
        return false;
    };

    let mut levels = [0u32; 8];
    let mut nl = 0;
    while nl < levels.len() {
        let Some(l) = fdt.prop_cell(&fan_node, b"cooling-levels", nl) else { break };
        levels[nl] = l;
        nl += 1;
    }
    let rpm_offset = fdt.prop_u32(&fan_node, b"rpm-offset").map(|o| o as u64)
        .filter(|_| fdt.prop(&fan_node, b"rpm-regmap").is_some());

    let mut hw = FanHw { rp1: board, avs };
    // rp1_pwm probe: pins, assigned clock rate, clock on.
    if !apply_pinctrl(fdt, &pwm_node, &mut hw) {
        kprintln!("[npk] fan: PWM pins unreadable");
        return false;
    }
    let rate = fdt.prop_u32(&pwm_node, b"assigned-clock-rates").unwrap_or(pi5_core::clk_rp1::XOSC_HZ as u32) as u64;
    let clk_rate = match pi5_core::clk_rp1::pwm1_clock(&mut hw, rate) {
        Ok(r) if r != 0 => r,
        r => { kprintln!("[npk] fan: PWM clock: {:?}", r); return false; }
    };
    // pwm-fan probe.
    let pwm = Rp1Pwm::request(&mut hw, base, channel, clk_rate);
    let Some(fan) = Fan::probe(&mut hw, pwm, period as u64, inverted, &levels[..nl], rpm_offset) else {
        kprintln!("[npk] fan: no cooling levels");
        return false;
    };
    let mut ts = [Trip { temperature: 0, hysteresis: 0, kind: TripType::Active, cooling: None }; MAX_TRIPS];
    let mut nt = 0;
    for t in trips.iter().flatten() { ts[nt] = *t; nt += 1; }
    let zone = Zone::new(&ts[..nt]);
    STATE.store(fan.state, Ordering::Release);
    let temp = fan::get_temp(&mut hw, slope, offset);
    *SETUP.lock() = Some(Setup { hw, fan, zone, trips: ts, ntrips: nt, slope, offset, polling_ms });
    match temp {
        Some(t) => kprintln!("[npk] fan: PWM1 channel {} at {} Hz, {} levels, {} trips; CPU {}.{} C",
            channel, clk_rate, nl, nt, t / 1000, (t % 1000).abs() / 100),
        None => kprintln!("[npk] fan: PWM1 channel {} at {} Hz, {} levels, {} trips; no reading yet",
            channel, clk_rate, nl, nt),
    }
    crate::smp::scheduler::spawn_fiber(fan_fiber, 0);
    true
}

/// `thermal_zone_device_check` every `polling-delay`.
fn fan_fiber(_: u64) {
    loop {
        let (polling_ms, critical) = {
            let mut g = SETUP.lock();
            let Some(s) = g.as_mut() else { return };
            let mut critical = None;
            if let Some(t) = fan::get_temp(&mut s.hw, s.slope, s.offset) {
                TEMP.store(t, Ordering::Release);
                let action = s.zone.update(t, s.fan.state);
                let held = HELD.load(Ordering::Acquire) != AUTO;
                match action {
                    Action::None => {}
                    Action::Cool(_) if held => {}
                    Action::Cool(state) => {
                        let old = s.fan.state;
                        if s.fan.set_cur_state(&mut s.hw, state) && state != old {
                            STATE.store(state, Ordering::Release);
                            crate::kdebug!("[npk] fan: CPU {} mC, state {} -> {}", t, old, state);
                        }
                    }
                    Action::Critical(t) => critical = Some(t),
                }
            }
            (s.polling_ms, critical)
        };
        if let Some(t) = critical {
            kprintln!("[npk] fan: CPU at {} C, critical: powering off", t / 1000);
            crate::intent::system::intent_halt();
        }
        let deadline = crate::time::counter() + polling_ms * (crate::time::counter_hz() / 1000);
        if crate::smp::fiber::wait(0, deadline).is_none() {
            return;
        }
    }
}
