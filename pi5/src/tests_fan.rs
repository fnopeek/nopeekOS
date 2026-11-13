//! The fan against Linux's arithmetic: the Pi 5's `cooling-levels`, trips
//! and `pwms` cells from `bcm2712-rpi-5-b.dts` / `bcm2712-ds.dtsi`.

use crate::fan::{get_temp, Action, Fan, Rp1Pwm, Trip, TripType, Zone};
use crate::hw::{Block, Hw};
use std::collections::BTreeMap;

#[derive(Default)]
struct Regs(BTreeMap<(u8, u64), u32>, Vec<(u64, u32)>);

fn key(b: Block) -> u8 { if b == Block::Avs { 1 } else { 0 } }

impl Hw for Regs {
    fn read(&mut self, b: Block, off: u64, _: u8) -> u32 { *self.0.get(&(key(b), off)).unwrap_or(&0) }
    fn write(&mut self, b: Block, off: u64, _: u8, v: u32) {
        self.0.insert((key(b), off), v);
        if b == Block::Rp1Bar1 { self.1.push((off, v)); }
    }
    fn delay_us(&mut self, _: u64) {}
}

const BASE: u64 = 0x9_c000;
const LEVELS: [u32; 5] = [0, 75, 125, 175, 250];

fn pi5_trips() -> [Trip; 5] {
    let a = |t, s| Trip { temperature: t, hysteresis: 5000, kind: TripType::Active, cooling: Some((s, s)) };
    [
        Trip { temperature: 110_000, hysteresis: 0, kind: TripType::Critical, cooling: Some((4, 4)) },
        a(50_000, 1), a(60_000, 2), a(67_500, 3), a(75_000, 4),
    ]
}

#[test]
fn temperature_is_the_coefficients_line() {
    let mut r = Regs::default();
    r.0.insert((1, 0x200), (1 << 16) | (1 << 10) | 740);
    assert_eq!(get_temp(&mut r, -550, 450_000), Some(450_000 - 550 * 740));
    r.0.insert((1, 0x200), 740);
    assert_eq!(get_temp(&mut r, -550, 450_000), None);
}

#[test]
fn probe_runs_the_fan_at_the_top_level() {
    let mut r = Regs::default();
    let pwm = Rp1Pwm::request(&mut r, BASE, 3, 50_000_000);
    let fan = Fan::probe(&mut r, pwm, 41566, true, &LEVELS, Some(0x3c)).unwrap();
    assert_eq!(fan.state, 4);
    // Channel 3: CTRL 0x44, RANGE 0x48, DUTY 0x50.
    assert_eq!(r.0[&(0, BASE + 0x48)], 2078);
    // DIV_ROUND_UP(250 * 41565, 255) = 40750 ns, / 20 ns rounded.
    assert_eq!(r.0[&(0, BASE + 0x50)], 2038);
    assert_eq!(r.0[&(0, BASE + 0x44)], (1 << 8) | 1 | (1 << 3));
    assert_eq!(r.0[&(0, BASE)] & 0x7fff_ffff, 1 << 3);
    assert!(r.0[&(0, BASE)] & (1 << 31) != 0);
}

#[test]
fn the_zone_steps_with_hysteresis() {
    let mut r = Regs::default();
    let pwm = Rp1Pwm::request(&mut r, BASE, 3, 50_000_000);
    let mut fan = Fan::probe(&mut r, pwm, 41566, true, &LEVELS, None).unwrap();
    let mut z = Zone::new(&pi5_trips());
    let mut step = |z: &mut Zone, fan: &mut Fan, t| {
        if let Action::Cool(s) = z.update(t, fan.state) { fan.set_cur_state(&mut r, s); }
        fan.state
    };
    // Cool at boot: off after the first poll.
    assert_eq!(step(&mut z, &mut fan, 45_000), 0);
    assert_eq!(step(&mut z, &mut fan, 51_000), 1);
    assert_eq!(step(&mut z, &mut fan, 61_000), 2);
    // Below 60 but above 55, falling: stays.
    assert_eq!(step(&mut z, &mut fan, 57_000), 2);
    // Below the hysteresis and falling: the warm trip lets go.
    assert_eq!(step(&mut z, &mut fan, 54_000), 1);
    assert_eq!(step(&mut z, &mut fan, 44_000), 0);
    assert_eq!(step(&mut z, &mut fan, 80_000), 4);
    assert_eq!(z.update(111_000, 4), Action::Critical(111_000));
}

#[test]
fn the_fan_pin_is_pwm1_on_function_0() {
    assert_eq!(crate::gpio_rp1::fsel_of(45, "pwm1"), Some(0));
    assert_eq!(crate::gpio_rp1::fsel_of(45, "gpio"), Some(5));
    assert_eq!(crate::gpio_rp1::fsel_of(45, "dpi"), None);
}
