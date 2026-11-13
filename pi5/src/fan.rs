//! The Pi 5's fan and the CPU temperature it follows, after Linux:
//! `drivers/thermal/broadcom/bcm2711_thermal.c` (the AVS ring oscillator
//! sensor), the thermal core's trip handling with the `step_wise` governor
//! of the Raspberry Pi tree (`drivers/thermal/thermal_core.c`,
//! `gov_step_wise.c`), `drivers/pwm/pwm-rp1.c` and
//! `drivers/hwmon/pwm-fan.c` as the cooling device.
//!
//! [`Fan::probe`] is the probe order of `rp1_pwm` (its clock and pins are
//! set up by the caller, see `clk_rp1::pwm1_clock` and `gpio_rp1`) and
//! `pwm-fan`; [`Zone::update`] is one poll of the thermal zone.

use crate::hw::{Block, Hw};

/// `AVS_RO_TEMP_STATUS` in the `brcm,bcm2711-avs-monitor` syscon.
const AVS_RO_TEMP_STATUS: u64 = 0x200;
const AVS_RO_TEMP_STATUS_VALID_MSK: u32 = (1 << 16) | (1 << 10);
const AVS_RO_TEMP_STATUS_DATA_MSK: u32 = 0x3ff;

/// `bcm2711_get_temp`: millidegrees Celsius, `None` while the sensor has
/// no valid reading (-EIO). `slope` and `offset` are the zone's
/// `coefficients`.
pub fn get_temp(hw: &mut impl Hw, slope: i32, offset: i32) -> Option<i32> {
    let val = hw.r32(Block::Avs, AVS_RO_TEMP_STATUS);
    if val & AVS_RO_TEMP_STATUS_VALID_MSK == 0 {
        return None;
    }
    Some(slope * (val & AVS_RO_TEMP_STATUS_DATA_MSK) as i32 + offset)
}

/// `thermal_trip_type`, as far as the governor tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TripType {
    Active,
    Passive,
    Hot,
    Critical,
}

/// One trip of the zone with the cooling state its `cooling-maps` entry
/// asks of the fan (`cooling-device = <&fan lower upper>`), if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trip {
    pub temperature: i32,
    pub hysteresis: i32,
    pub kind: TripType,
    /// `(lower, upper)` of the thermal instance on the fan.
    pub cooling: Option<(u32, u32)>,
}

/// A thermal instance's state (`thermal_instance`): `target` is
/// `THERMAL_NO_TARGET` as `None`.
#[derive(Clone, Copy, Debug, Default)]
struct Instance {
    target: Option<u32>,
    initialized: bool,
}

/// `thermal_trend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trend {
    Stable,
    Raising,
    Dropping,
}

/// What a poll of the zone asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing changes.
    None,
    /// Set the fan's cooling state (`thermal_cdev_set_cur_state`).
    Cool(u32),
    /// A critical trip is reached: `thermal_zone_device_critical`, the
    /// orderly power-off.
    Critical(i32),
}

/// Maximum trips a zone carries here.
pub const MAX_TRIPS: usize = 8;

/// The thermal zone, polled (`polling-delay`), with one cooling device.
pub struct Zone {
    trips: [Option<Trip>; MAX_TRIPS],
    instances: [Instance; MAX_TRIPS],
    last_temperature: Option<i32>,
    /// `cdev->updated` is false: some instance changed its target.
    cdev_stale: bool,
}

impl Zone {
    pub fn new(trips: &[Trip]) -> Zone {
        let mut t = [None; MAX_TRIPS];
        for (slot, trip) in t.iter_mut().zip(trips.iter()) {
            *slot = Some(*trip);
        }
        Zone { trips: t, instances: [Instance::default(); MAX_TRIPS], last_temperature: None, cdev_stale: false }
    }

    /// `__thermal_zone_get_trend` without a driver `get_trend`.
    fn trend(&self, temp: i32) -> Trend {
        match self.last_temperature {
            None => Trend::Raising,
            Some(last) if temp > last => Trend::Raising,
            Some(last) if temp < last => Trend::Dropping,
            Some(_) => Trend::Stable,
        }
    }

    /// One `thermal_zone_device_update` at `temp`, with the fan's current
    /// state `cur_state`: a critical trip first, then `step_wise_manage`
    /// and `thermal_cdev_update` (the highest target of the fan's
    /// instances, 0 without one).
    pub fn update(&mut self, temp: i32, cur_state: u32) -> Action {
        let trend = self.trend(temp);
        self.last_temperature = Some(temp);
        for trip in self.trips.iter().flatten() {
            if trip.kind == TripType::Critical && temp >= trip.temperature {
                return Action::Critical(temp);
            }
        }
        for (trip, inst) in self.trips.iter().zip(self.instances.iter_mut()) {
            let Some(trip) = trip else { continue };
            if matches!(trip.kind, TripType::Critical | TripType::Hot) {
                continue;
            }
            let Some((lower, upper)) = trip.cooling else { continue };
            let hyst_temp = trip.temperature - trip.hysteresis;
            let old_target = inst.target;
            let throttle = temp >= trip.temperature
                || (temp >= hyst_temp && old_target == Some(upper));
            inst.target = target_state(inst, cur_state, lower, upper, trend, throttle);
            if inst.initialized && old_target == inst.target {
                continue;
            }
            inst.initialized = true;
            self.cdev_stale = true;
        }
        if !self.cdev_stale {
            return Action::None;
        }
        self.cdev_stale = false;
        let target = self.instances.iter().filter_map(|i| i.target).max().unwrap_or(0);
        Action::Cool(target)
    }
}

/// `get_target_state` of the Raspberry Pi tree's `gov_step_wise.c`.
fn target_state(inst: &Instance, cur_state: u32, lower: u32, upper: u32, trend: Trend, throttle: bool) -> Option<u32> {
    if !inst.initialized {
        return throttle.then(|| (cur_state + 1).clamp(lower, upper));
    }
    if throttle {
        match trend {
            Trend::Raising => return Some((cur_state + 1).clamp(lower, upper)),
            Trend::Dropping => {
                return Some(cur_state.saturating_sub(1).clamp((lower + 1).min(upper), upper));
            }
            Trend::Stable => {}
        }
    } else if trend == Trend::Dropping {
        return None;
    }
    inst.target
}

/// RP1's PWM block (`raspberrypi,rp1-pwm`) at `base` in BAR1.
const PWM_GLOBAL_CTRL: u64 = 0x000;
const fn pwm_channel_ctrl(x: u32) -> u64 { 0x014 + x as u64 * 16 }
const fn pwm_range(x: u32) -> u64 { 0x018 + x as u64 * 16 }
const fn pwm_duty(x: u32) -> u64 { 0x020 + x as u64 * 16 }
/// FIFO_POP_MASK and trailing-edge M/S modulation.
const PWM_CHANNEL_DEFAULT: u32 = (1 << 8) | (1 << 0);
const PWM_POLARITY: u32 = 1 << 3;
const SET_UPDATE: u32 = 1 << 31;
const NSEC_PER_SEC: u64 = 1_000_000_000;

/// `pwm_state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PwmState {
    pub period: u64,
    pub duty_cycle: u64,
    pub inverted: bool,
    pub enabled: bool,
}

/// One channel of an RP1 PWM block, as `pwm_get` hands it to a consumer.
pub struct Rp1Pwm {
    pub base: u64,
    pub channel: u32,
    pub clk_rate: u64,
    /// `pwm->state`: what the core applied last.
    state: Option<PwmState>,
}

fn div_round_closest(x: u64, d: u64) -> u64 { (x + d / 2) / d }

impl Rp1Pwm {
    /// `rp1_pwm_request`, from `pwm_get`.
    pub fn request(hw: &mut impl Hw, base: u64, channel: u32, clk_rate: u64) -> Rp1Pwm {
        hw.w32(Block::Rp1Bar1, base + pwm_channel_ctrl(channel), PWM_CHANNEL_DEFAULT);
        Rp1Pwm { base, channel, clk_rate, state: None }
    }

    fn rd(&self, hw: &mut impl Hw, reg: u64) -> u32 { hw.r32(Block::Rp1Bar1, self.base + reg) }
    fn wr(&self, hw: &mut impl Hw, reg: u64, v: u32) { hw.w32(Block::Rp1Bar1, self.base + reg, v) }

    /// `pwm_apply_might_sleep`: the core skips a state it applied already,
    /// then `rp1_pwm_apply`.
    pub fn apply(&mut self, hw: &mut impl Hw, state: &PwmState) {
        if self.state == Some(*state) {
            return;
        }
        let ch = self.channel;
        let clk_period = div_round_closest(NSEC_PER_SEC, self.clk_rate);
        self.wr(hw, pwm_duty(ch), div_round_closest(state.duty_cycle, clk_period) as u32);
        self.wr(hw, pwm_range(ch), div_round_closest(state.period, clk_period) as u32);
        let mut v = self.rd(hw, pwm_channel_ctrl(ch));
        if state.inverted { v |= PWM_POLARITY } else { v &= !PWM_POLARITY }
        self.wr(hw, pwm_channel_ctrl(ch), v);
        let mut v = self.rd(hw, PWM_GLOBAL_CTRL);
        if state.enabled { v |= 1 << ch } else { v &= !(1 << ch) }
        self.wr(hw, PWM_GLOBAL_CTRL, v);
        // rp1_pwm_apply_config
        let v = self.rd(hw, PWM_GLOBAL_CTRL);
        self.wr(hw, PWM_GLOBAL_CTRL, v | SET_UPDATE);
        self.state = Some(*state);
    }
}

const MAX_PWM: u64 = 255;

/// `pwm_fan_ctx` without a regulator, tachometer interrupt or
/// `fan-stop-to-start-percent` (none on the Pi 5): enable mode
/// `pwm_disable_reg_enable`, the speed read from `rpm-regmap`.
pub struct Fan {
    pub pwm: Rp1Pwm,
    pwm_state: PwmState,
    enabled: bool,
    pwm_value: u64,
    levels: [u32; 8],
    max_state: u32,
    pub state: u32,
    /// `rpm-offset` in the PWM block.
    rpm_offset: Option<u64>,
}

impl Fan {
    /// `pwm_fan_probe`: `pwm_init_state` from the `pwms` cells, the
    /// highest cooling level applied, the state at the maximum.
    pub fn probe(hw: &mut impl Hw, pwm: Rp1Pwm, period: u64, inverted: bool, levels: &[u32], rpm_offset: Option<u64>) -> Option<Fan> {
        if levels.is_empty() || levels.len() > 8 {
            return None;
        }
        let mut l = [0u32; 8];
        l[..levels.len()].copy_from_slice(levels);
        let max_state = levels.len() as u32 - 1;
        let mut fan = Fan {
            pwm,
            pwm_state: PwmState { period, duty_cycle: 0, inverted, enabled: false },
            enabled: false,
            pwm_value: 0,
            levels: l,
            max_state,
            state: 0,
            rpm_offset,
        };
        fan.set_pwm(hw, l[max_state as usize] as u64);
        fan.state = max_state;
        Some(fan)
    }

    /// `__set_pwm`.
    fn set_pwm(&mut self, hw: &mut impl Hw, pwm: u64) {
        if pwm > 0 {
            let period = self.pwm_state.period;
            self.pwm_state.duty_cycle = (pwm * (period - 1)).div_ceil(MAX_PWM);
            self.pwm.apply(hw, &self.pwm_state);
            // pwm_fan_power_on
            if !self.enabled {
                self.pwm_state.enabled = true;
                self.pwm.apply(hw, &self.pwm_state);
                self.enabled = true;
            }
        } else if self.enabled {
            // pwm_fan_power_off with pwm_disable_reg_enable
            self.pwm_state.enabled = false;
            self.pwm_state.duty_cycle = 0;
            self.pwm.apply(hw, &self.pwm_state);
            self.enabled = false;
        }
        self.pwm_value = pwm;
    }

    /// `pwm_fan_set_cur_state`.
    pub fn set_cur_state(&mut self, hw: &mut impl Hw, state: u32) -> bool {
        if state > self.max_state {
            return false;
        }
        if state == self.state {
            return true;
        }
        self.set_pwm(hw, self.levels[state as usize] as u64);
        self.state = state;
        true
    }

    /// The duty value 0..=255 last set (`pwm1` in hwmon).
    pub fn pwm_value(&self) -> u64 { self.pwm_value }

    /// `fan1_input` from the `rpm-regmap` register.
    pub fn rpm(&self, hw: &mut impl Hw) -> Option<u32> {
        self.rpm_offset.map(|o| hw.r32(Block::Rp1Bar1, self.pwm.base + o))
    }
}
